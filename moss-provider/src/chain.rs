use crate::merkle::{proof_for_file, CHUNK_BYTES};
use base64::Engine as _;
use lichen_client_sdk::{Client, Keypair};
use lichen_core::{KeypairFile, Pubkey};
use serde_json::json;
use std::path::Path;

const WIDE_LAYOUT_MARKER: u8 = 0xAC;
const CHALLENGE_STATUS_OPEN: u8 = 0;

#[derive(Debug, Clone)]
pub struct StorageInfo {
    pub owner: Pubkey,
    pub size: u64,
    pub replication: u8,
    pub expiry_slot: u64,
    pub providers: Vec<Pubkey>,
}

#[derive(Debug, Clone, Copy)]
pub struct Challenge {
    pub deadline_slot: u64,
    pub status: u8,
    pub effective_nonce: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderStatus {
    pub capacity: u64,
    pub used: u64,
    pub stored_count: u64,
    pub active: bool,
    pub collateral: u64,
    pub price: u64,
    pub remaining_obligations: u64,
    pub required_collateral: u64,
}

impl ProviderStatus {
    pub fn operational(self) -> bool {
        self.active && self.price > 0
    }

    pub fn accepting_assignments(self) -> bool {
        self.operational() && self.collateral >= self.required_collateral
    }
}

#[derive(Debug)]
pub enum ChallengeState {
    Missing,
    WaitingForEntropy,
    Ready(Challenge),
}

pub struct ChainClient {
    client: Client,
    signer: Keypair,
    contract: Pubkey,
}

impl ChainClient {
    pub fn load(rpc_url: &str, contract: Pubkey, keypair_path: &Path) -> Result<Self, String> {
        let keypair_file = KeypairFile::load(keypair_path)
            .map_err(|error| format!("load Moss provider keypair: {error}"))?;
        let core_keypair = keypair_file
            .to_keypair()
            .map_err(|error| format!("decode Moss provider keypair: {error}"))?;
        let signer = Keypair::from_seed(&core_keypair.to_seed());
        Ok(Self {
            client: Client::new(rpc_url),
            signer,
            contract,
        })
    }

    pub fn provider(&self) -> Pubkey {
        self.signer.pubkey()
    }

    pub async fn current_slot(&self) -> Result<u64, String> {
        self.client
            .get_slot()
            .await
            .map_err(|error| format!("get Moss chain slot: {error}"))
    }

    async fn readonly(
        &self,
        function: &str,
        args: Vec<u8>,
    ) -> Result<lichen_client_sdk::ReadonlyContractResult, String> {
        self.client
            .call_readonly_contract(&self.contract, function, args, Some(&self.signer.pubkey()))
            .await
            .map_err(|error| format!("Moss {function} query failed: {error}"))
    }

    pub async fn storage_info(&self, hash: &str) -> Result<Option<StorageInfo>, String> {
        let args = serde_json::to_vec(&json!([hash]))
            .map_err(|error| format!("encode Moss storage query: {error}"))?;
        let result = self.readonly("get_storage_info", args).await?;
        match result.return_code {
            Some(1) => return Ok(None),
            Some(0) if result.success => {}
            code => {
                return Err(format!(
                    "Moss get_storage_info returned {:?}: {}",
                    code,
                    result.error.unwrap_or_else(|| "unknown error".to_string())
                ))
            }
        }
        let data = decode_return_data(&result, "get_storage_info")?;
        if data.len() < 59 {
            return Err("Moss storage entry is truncated".to_string());
        }
        let owner = pubkey_from_slice(&data[0..32])?;
        let size = u64::from_le_bytes(data[32..40].try_into().map_err(|_| "storage size")?);
        let replication = data[40];
        let expiry_slot =
            u64::from_le_bytes(data[42..50].try_into().map_err(|_| "storage expiry")?);
        let provider_count = data[58] as usize;
        let required = 59usize
            .checked_add(
                provider_count
                    .checked_mul(32)
                    .ok_or_else(|| "provider count overflow".to_string())?,
            )
            .ok_or_else(|| "provider layout overflow".to_string())?;
        if provider_count > 16 || data.len() < required {
            return Err("Moss storage provider layout is invalid".to_string());
        }
        let mut providers = Vec::with_capacity(provider_count);
        for provider in data[59..required].as_chunks::<32>().0 {
            providers.push(pubkey_from_slice(provider)?);
        }
        Ok(Some(StorageInfo {
            owner,
            size,
            replication,
            expiry_slot,
            providers,
        }))
    }

    pub async fn provider_status(&self) -> Result<Option<ProviderStatus>, String> {
        let args = serde_json::to_vec(&json!([self.provider().to_base58()]))
            .map_err(|error| format!("encode Moss provider query: {error}"))?;
        let result = self.readonly("get_provider_info", args).await?;
        match result.return_code {
            Some(1) => return Ok(None),
            Some(0) if result.success => {}
            code => return Err(format!("Moss get_provider_info returned {code:?}")),
        }
        let data = decode_return_data(&result, "get_provider_info")?;
        if data.len() != 65 {
            return Err("Moss provider info has an invalid length".to_string());
        }
        Ok(Some(ProviderStatus {
            capacity: u64::from_le_bytes(data[0..8].try_into().map_err(|_| "capacity")?),
            used: u64::from_le_bytes(data[8..16].try_into().map_err(|_| "used")?),
            stored_count: u64::from_le_bytes(data[16..24].try_into().map_err(|_| "stored count")?),
            active: data[24] == 1,
            collateral: u64::from_le_bytes(data[33..41].try_into().map_err(|_| "collateral")?),
            price: u64::from_le_bytes(data[41..49].try_into().map_err(|_| "price")?),
            remaining_obligations: u64::from_le_bytes(
                data[49..57].try_into().map_err(|_| "obligations")?,
            ),
            required_collateral: u64::from_le_bytes(
                data[57..65].try_into().map_err(|_| "required collateral")?,
            ),
        }))
    }

    pub async fn challenge(&self, hash: &str) -> Result<ChallengeState, String> {
        let args = serde_json::to_vec(&json!([hash, self.provider().to_base58()]))
            .map_err(|error| format!("encode Moss challenge query: {error}"))?;
        let result = self.readonly("get_challenge", args).await?;
        match result.return_code {
            Some(1) => return Ok(ChallengeState::Missing),
            Some(7) => return Ok(ChallengeState::WaitingForEntropy),
            Some(0) if result.success => {}
            code => return Err(format!("Moss get_challenge returned {code:?}")),
        }
        let data = decode_return_data(&result, "get_challenge")?;
        if data.len() < 33 {
            return Err("Moss challenge response is truncated".to_string());
        }
        Ok(ChallengeState::Ready(Challenge {
            deadline_slot: u64::from_le_bytes(
                data[8..16].try_into().map_err(|_| "challenge deadline")?,
            ),
            status: data[24],
            effective_nonce: u64::from_le_bytes(
                data[data.len() - 8..]
                    .try_into()
                    .map_err(|_| "challenge nonce")?,
            ),
        }))
    }

    pub async fn confirm_storage(&self, hash: &str) -> Result<String, String> {
        let args = serde_json::to_vec(&json!([self.provider().to_base58(), hash]))
            .map_err(|error| format!("encode Moss confirmation: {error}"))?;
        self.call("confirm_storage", args).await
    }

    pub async fn respond_to_challenge(
        &self,
        hash: &str,
        path: &Path,
        size: u64,
        effective_nonce: u64,
    ) -> Result<String, String> {
        let chunk_count = size
            .checked_add(CHUNK_BYTES as u64 - 1)
            .ok_or_else(|| "Moss chunk count overflow".to_string())?
            / CHUNK_BYTES as u64;
        if chunk_count == 0 {
            return Err("cannot prove an empty Moss object".to_string());
        }
        let target_index = effective_nonce % chunk_count;
        let (chunk, proof, actual_size) = proof_for_file(path, target_index).await?;
        if actual_size != size {
            return Err("Moss object size changed before challenge response".to_string());
        }
        let provider = self.provider().0.to_vec();
        let data_hash = crate::content::decode_hash(hash)?.to_vec();
        if size <= CHUNK_BYTES as u64 {
            let args = encode_wide_layout(vec![
                WideArgument::Pointer(provider),
                WideArgument::Pointer(data_hash),
                WideArgument::Pointer(chunk),
            ])?;
            self.call("respond_challenge", args).await
        } else {
            let args = encode_wide_layout(vec![
                WideArgument::Pointer(provider),
                WideArgument::Pointer(data_hash),
                WideArgument::Pointer(chunk.clone()),
                WideArgument::U32(
                    u32::try_from(chunk.len()).map_err(|_| "Moss chunk is too large")?,
                ),
                WideArgument::Pointer(proof.clone()),
                WideArgument::U32(
                    u32::try_from(proof.len()).map_err(|_| "Moss proof is too large")?,
                ),
            ])?;
            self.call("respond_challenge_merkle", args).await
        }
    }

    pub async fn is_closed(&self, hash: &str) -> Result<bool, String> {
        let args = serde_json::to_vec(&json!([hash]))
            .map_err(|error| format!("encode Moss close query: {error}"))?;
        let result = self.readonly("is_storage_closed", args).await?;
        Ok(matches!(result.return_code, Some(1)))
    }

    pub async fn close_storage(&self, owner: &Pubkey, hash: &str) -> Result<String, String> {
        let args = serde_json::to_vec(&json!([owner.to_base58(), hash]))
            .map_err(|error| format!("encode Moss close call: {error}"))?;
        self.call("close_storage", args).await
    }

    async fn call(&self, function: &str, args: Vec<u8>) -> Result<String, String> {
        self.client
            .call_contract(&self.signer, &self.contract, function, args, 0)
            .await
            .map_err(|error| format!("submit Moss {function}: {error}"))
    }
}

fn decode_return_data(
    result: &lichen_client_sdk::ReadonlyContractResult,
    function: &str,
) -> Result<Vec<u8>, String> {
    let encoded = result
        .return_data
        .as_deref()
        .ok_or_else(|| format!("Moss {function} returned no data"))?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("decode Moss {function} data: {error}"))
}

fn pubkey_from_slice(data: &[u8]) -> Result<Pubkey, String> {
    let bytes: [u8; 32] = data
        .try_into()
        .map_err(|_| "invalid Moss public key length".to_string())?;
    Ok(Pubkey(bytes))
}

enum WideArgument {
    Pointer(Vec<u8>),
    U32(u32),
}

fn encode_wide_layout(arguments: Vec<WideArgument>) -> Result<Vec<u8>, String> {
    let mut strides = Vec::with_capacity(arguments.len());
    let mut payloads = Vec::with_capacity(arguments.len());
    for argument in arguments {
        match argument {
            WideArgument::Pointer(mut bytes) => {
                if bytes.is_empty() {
                    return Err("wide pointer argument cannot be empty".to_string());
                }
                let padded = bytes
                    .len()
                    .max(32)
                    .div_ceil(32)
                    .checked_mul(32)
                    .ok_or_else(|| "wide pointer stride overflow".to_string())?;
                let stride = u32::try_from(padded)
                    .map_err(|_| "wide pointer argument exceeds u32".to_string())?;
                bytes.resize(padded, 0);
                strides.push(stride);
                payloads.push(bytes);
            }
            WideArgument::U32(value) => {
                strides.push(4);
                payloads.push(value.to_le_bytes().to_vec());
            }
        }
    }
    let payload_len = payloads.iter().try_fold(0usize, |total, payload| {
        total
            .checked_add(payload.len())
            .ok_or_else(|| "wide argument payload overflow".to_string())
    })?;
    let mut output = Vec::with_capacity(1 + strides.len() * 4 + payload_len);
    output.push(WIDE_LAYOUT_MARKER);
    for stride in strides {
        output.extend_from_slice(&stride.to_le_bytes());
    }
    for payload in payloads {
        output.extend_from_slice(&payload);
    }
    Ok(output)
}

pub fn challenge_is_open(challenge: Challenge) -> bool {
    challenge.status == CHALLENGE_STATUS_OPEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_layout_preserves_full_moss_chunk() {
        let chunk = vec![0xA5; CHUNK_BYTES];
        let encoded = encode_wide_layout(vec![
            WideArgument::Pointer(vec![1; 32]),
            WideArgument::Pointer(vec![2; 32]),
            WideArgument::Pointer(chunk.clone()),
            WideArgument::U32(CHUNK_BYTES as u32),
            WideArgument::Pointer(vec![3; 64]),
            WideArgument::U32(64),
        ])
        .unwrap();
        assert_eq!(encoded[0], WIDE_LAYOUT_MARKER);
        assert_eq!(
            u32::from_le_bytes(encoded[9..13].try_into().unwrap()),
            CHUNK_BYTES as u32
        );
        let data_start = 1 + 6 * 4;
        assert_eq!(
            &encoded[data_start + 64..data_start + 64 + CHUNK_BYTES],
            chunk
        );
    }

    #[test]
    fn provider_readiness_uses_rounded_capacity_collateral() {
        let mut status = ProviderStatus {
            capacity: 1_073_741_825,
            used: 0,
            stored_count: 0,
            active: true,
            collateral: 19_999_999,
            price: 10,
            remaining_obligations: 0,
            required_collateral: 20_000_000,
        };
        assert!(status.operational());
        assert!(!status.accepting_assignments());
        status.collateral = 20_000_000;
        assert!(status.accepting_assignments());
    }
}
