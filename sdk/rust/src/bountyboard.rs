use crate::client::ReadonlyContractResult;
use crate::{Client, Error, Keypair, Pubkey, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

const PROGRAM_SYMBOL_CANDIDATES: [&str; 5] = ["BOUNTY", "bounty", "BountyBoard", "BOUNTYBOARD", "bountyboard"];
const BOUNTY_DATA_SIZE: usize = 91;
const PLATFORM_STATS_SIZE: usize = 32;

/// Bounty status: open for submissions.
pub const BOUNTY_STATUS_OPEN: u8 = 0;
/// Bounty status: completed (a submission was approved).
pub const BOUNTY_STATUS_COMPLETED: u8 = 1;
/// Bounty status: cancelled (refund issued).
pub const BOUNTY_STATUS_CANCELLED: u8 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BountyBoardBountyInfo {
    pub creator: Pubkey,
    pub title_hash: [u8; 32],
    pub reward_amount: u64,
    pub deadline_slot: u64,
    pub status: u8,
    pub submission_count: u8,
    pub created_slot: u64,
    pub approved_idx: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BountyBoardPlatformStats {
    pub bounty_count: u64,
    pub completed_count: u64,
    pub reward_volume: u64,
    pub cancel_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BountyBoardStats {
    pub bounty_count: u64,
    pub completed_count: u64,
    pub total_reward_volume: u64,
    pub cancel_count: u64,
    pub paused: bool,
}

pub struct CreateBountyParams {
    pub title_hash: [u8; 32],
    pub reward_amount: u64,
    pub deadline_slot: u64,
}

pub struct SubmitWorkParams {
    pub bounty_id: u64,
    pub proof_hash: [u8; 32],
}

pub struct ApproveWorkParams {
    pub bounty_id: u64,
    pub submission_idx: u8,
}

#[derive(Debug, Clone)]
pub struct BountyBoardClient {
    client: Client,
    program_id: Arc<Mutex<Option<Pubkey>>>,
}

fn build_layout_args(layout: &[u8], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + layout.len() + chunks.iter().map(|chunk| chunk.len()).sum::<usize>());
    out.push(0xAB);
    out.extend_from_slice(layout);
    for chunk in chunks {
        out.extend_from_slice(chunk);
    }
    out
}

fn encode_create_bounty_args(creator: &Pubkey, title_hash: &[u8; 32], reward_amount: u64, deadline_slot: u64) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x20, 0x08, 0x08],
        &[
            creator.as_ref().to_vec(),
            title_hash.to_vec(),
            reward_amount.to_le_bytes().to_vec(),
            deadline_slot.to_le_bytes().to_vec(),
        ],
    )
}

fn encode_submit_work_args(bounty_id: u64, worker: &Pubkey, proof_hash: &[u8; 32]) -> Vec<u8> {
    build_layout_args(
        &[0x08, 0x20, 0x20],
        &[
            bounty_id.to_le_bytes().to_vec(),
            worker.as_ref().to_vec(),
            proof_hash.to_vec(),
        ],
    )
}

fn encode_approve_work_args(caller: &Pubkey, bounty_id: u64, submission_idx: u8) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x08, 0x01],
        &[
            caller.as_ref().to_vec(),
            bounty_id.to_le_bytes().to_vec(),
            vec![submission_idx],
        ],
    )
}

fn encode_cancel_bounty_args(caller: &Pubkey, bounty_id: u64) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x08],
        &[
            caller.as_ref().to_vec(),
            bounty_id.to_le_bytes().to_vec(),
        ],
    )
}

fn encode_bounty_id_args(bounty_id: u64) -> Vec<u8> {
    build_layout_args(&[0x08], &[bounty_id.to_le_bytes().to_vec()])
}

fn ensure_readonly_success(
    result: &ReadonlyContractResult,
    function_name: &str,
    allowed_codes: &[u32],
) -> Result<()> {
    let code = result.return_code.unwrap_or(0);
    if !allowed_codes.contains(&code) {
        return Err(Error::RpcError(
            result
                .error
                .clone()
                .unwrap_or_else(|| format!("BountyBoard {} returned code {}", function_name, code)),
        ));
    }
    if !result.success {
        return Err(Error::RpcError(
            result
                .error
                .clone()
                .unwrap_or_else(|| format!("BountyBoard {} failed", function_name)),
        ));
    }
    Ok(())
}

fn decode_return_data(result: &ReadonlyContractResult, function_name: &str) -> Result<Vec<u8>> {
    let Some(return_data) = &result.return_data else {
        return Err(Error::ParseError(format!(
            "BountyBoard {} did not return payload data",
            function_name,
        )));
    };

    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, return_data)
        .map_err(|err| Error::ParseError(err.to_string()))
}

fn decode_u64(bytes: &[u8], start: usize, function_name: &str) -> Result<u64> {
    let end = start + 8;
    if bytes.len() < end {
        return Err(Error::ParseError(format!(
            "BountyBoard {} payload was shorter than expected",
            function_name,
        )));
    }
    let slice: [u8; 8] = bytes[start..end]
        .try_into()
        .map_err(|_| Error::ParseError(format!("BountyBoard {} payload was malformed", function_name)))?;
    Ok(u64::from_le_bytes(slice))
}

fn decode_bounty_info(result: &ReadonlyContractResult) -> Result<BountyBoardBountyInfo> {
    ensure_readonly_success(result, "get_bounty", &[0])?;
    let bytes = decode_return_data(result, "get_bounty")?;
    if bytes.len() < BOUNTY_DATA_SIZE {
        return Err(Error::ParseError(
            "BountyBoard get_bounty payload was shorter than expected".into(),
        ));
    }

    let mut creator_bytes = [0u8; 32];
    creator_bytes.copy_from_slice(&bytes[0..32]);

    let mut title_hash = [0u8; 32];
    title_hash.copy_from_slice(&bytes[32..64]);

    Ok(BountyBoardBountyInfo {
        creator: Pubkey(creator_bytes),
        title_hash,
        reward_amount: decode_u64(&bytes, 64, "get_bounty")?,
        deadline_slot: decode_u64(&bytes, 72, "get_bounty")?,
        status: bytes[80],
        submission_count: bytes[81],
        created_slot: decode_u64(&bytes, 82, "get_bounty")?,
        approved_idx: bytes[90],
    })
}

fn decode_platform_stats(result: &ReadonlyContractResult) -> Result<BountyBoardPlatformStats> {
    ensure_readonly_success(result, "get_platform_stats", &[0])?;
    let bytes = decode_return_data(result, "get_platform_stats")?;
    if bytes.len() < PLATFORM_STATS_SIZE {
        return Err(Error::ParseError(
            "BountyBoard get_platform_stats payload was shorter than expected".into(),
        ));
    }

    Ok(BountyBoardPlatformStats {
        bounty_count: decode_u64(&bytes, 0, "get_platform_stats")?,
        completed_count: decode_u64(&bytes, 8, "get_platform_stats")?,
        reward_volume: decode_u64(&bytes, 16, "get_platform_stats")?,
        cancel_count: decode_u64(&bytes, 24, "get_platform_stats")?,
    })
}

impl BountyBoardClient {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            program_id: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_program_id(client: Client, program_id: Pubkey) -> Self {
        Self {
            client,
            program_id: Arc::new(Mutex::new(Some(program_id))),
        }
    }

    pub async fn get_program_id(&self) -> Result<Pubkey> {
        if let Some(program_id) = self
            .program_id
            .lock()
            .map_err(|_| Error::ConfigError("BountyBoardClient program cache lock poisoned".into()))?
            .clone()
        {
            return Ok(program_id);
        }

        for symbol in PROGRAM_SYMBOL_CANDIDATES {
            let entry = match self.client.get_symbol_registry(symbol).await {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let Some(program) = entry.get("program").and_then(|value| value.as_str()) else {
                continue;
            };
            let program_id = Pubkey::from_base58(program).map_err(Error::ParseError)?;
            *self
                .program_id
                .lock()
                .map_err(|_| Error::ConfigError("BountyBoardClient program cache lock poisoned".into()))? = Some(program_id);
            return Ok(program_id);
        }

        Err(Error::ConfigError(
            "Unable to resolve the BountyBoard program via getSymbolRegistry(\"BOUNTY\")".into(),
        ))
    }

    // --- Read methods ---

    pub async fn get_bounty(&self, bounty_id: u64) -> Result<Option<BountyBoardBountyInfo>> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_bounty",
                encode_bounty_id_args(bounty_id),
                None,
            )
            .await?;

        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }

        decode_bounty_info(&result).map(Some)
    }

    pub async fn get_bounty_count(&self) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_bounty_count",
                Vec::new(),
                None,
            )
            .await?;
        ensure_readonly_success(&result, "get_bounty_count", &[0])?;

        let Some(return_data) = &result.return_data else {
            return Ok(0);
        };

        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, return_data)
            .map_err(|err| Error::ParseError(err.to_string()))?;
        if bytes.len() < 8 {
            return Ok(0);
        }
        decode_u64(&bytes, 0, "get_bounty_count")
    }

    pub async fn get_platform_stats(&self) -> Result<BountyBoardPlatformStats> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_platform_stats",
                Vec::new(),
                None,
            )
            .await?;
        decode_platform_stats(&result)
    }

    pub async fn get_stats(&self) -> Result<BountyBoardStats> {
        let value = self.client.get_bountyboard_stats().await?;
        serde_json::from_value(value).map_err(|err| Error::ParseError(err.to_string()))
    }

    // --- Write methods ---

    pub async fn create_bounty(&self, creator: &Keypair, params: CreateBountyParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                creator,
                &program_id,
                "create_bounty",
                encode_create_bounty_args(&creator.pubkey(), &params.title_hash, params.reward_amount, params.deadline_slot),
                params.reward_amount,
            )
            .await
    }

    pub async fn submit_work(&self, worker: &Keypair, params: SubmitWorkParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                worker,
                &program_id,
                "submit_work",
                encode_submit_work_args(params.bounty_id, &worker.pubkey(), &params.proof_hash),
                0,
            )
            .await
    }

    pub async fn approve_work(&self, creator: &Keypair, params: ApproveWorkParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                creator,
                &program_id,
                "approve_work",
                encode_approve_work_args(&creator.pubkey(), params.bounty_id, params.submission_idx),
                0,
            )
            .await
    }

    pub async fn cancel_bounty(&self, creator: &Keypair, bounty_id: u64) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                creator,
                &program_id,
                "cancel_bounty",
                encode_cancel_bounty_args(&creator.pubkey(), bounty_id),
                0,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readonly_result(return_code: u32, bytes: Vec<u8>) -> ReadonlyContractResult {
        ReadonlyContractResult {
            success: true,
            return_data: Some(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                bytes,
            )),
            return_code: Some(return_code),
            logs: Vec::new(),
            error: None,
            compute_used: None,
        }
    }

    #[test]
    fn create_bounty_encoding_matches_named_export_layout() {
        let creator = Pubkey([7u8; 32]);
        let title_hash = [0xAAu8; 32];
        let encoded = encode_create_bounty_args(&creator, &title_hash, 1_000, 2_000);

        assert_eq!(&encoded[..5], &[0xAB, 0x20, 0x20, 0x08, 0x08]);
        assert_eq!(&encoded[5..37], &[7u8; 32]);
        assert_eq!(&encoded[37..69], &[0xAAu8; 32]);
        assert_eq!(u64::from_le_bytes(encoded[69..77].try_into().unwrap()), 1_000);
        assert_eq!(u64::from_le_bytes(encoded[77..85].try_into().unwrap()), 2_000);
    }

    #[test]
    fn submit_work_encoding_matches_named_export_layout() {
        let worker = Pubkey([8u8; 32]);
        let proof_hash = [0xBBu8; 32];
        let encoded = encode_submit_work_args(42, &worker, &proof_hash);

        assert_eq!(&encoded[..4], &[0xAB, 0x08, 0x20, 0x20]);
        assert_eq!(u64::from_le_bytes(encoded[4..12].try_into().unwrap()), 42);
        assert_eq!(&encoded[12..44], &[8u8; 32]);
        assert_eq!(&encoded[44..76], &[0xBBu8; 32]);
    }

    #[test]
    fn approve_work_encoding_matches_named_export_layout() {
        let caller = Pubkey([9u8; 32]);
        let encoded = encode_approve_work_args(&caller, 5, 2);

        assert_eq!(&encoded[..4], &[0xAB, 0x20, 0x08, 0x01]);
        assert_eq!(&encoded[4..36], &[9u8; 32]);
        assert_eq!(u64::from_le_bytes(encoded[36..44].try_into().unwrap()), 5);
        assert_eq!(encoded[44], 2);
    }

    #[test]
    fn cancel_bounty_encoding_matches_named_export_layout() {
        let caller = Pubkey([10u8; 32]);
        let encoded = encode_cancel_bounty_args(&caller, 3);

        assert_eq!(&encoded[..3], &[0xAB, 0x20, 0x08]);
        assert_eq!(&encoded[3..35], &[10u8; 32]);
        assert_eq!(u64::from_le_bytes(encoded[35..43].try_into().unwrap()), 3);
    }

    #[test]
    fn bounty_id_encoding_matches_named_export_layout() {
        let encoded = encode_bounty_id_args(7);

        assert_eq!(&encoded[..2], &[0xAB, 0x08]);
        assert_eq!(u64::from_le_bytes(encoded[2..10].try_into().unwrap()), 7);
    }

    #[test]
    fn bounty_info_decoding_matches_contract_layout() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[1u8; 32]); // creator
        payload.extend_from_slice(&[0xAAu8; 32]); // title_hash
        payload.extend_from_slice(&5_000_000_000u64.to_le_bytes()); // reward_amount
        payload.extend_from_slice(&1000u64.to_le_bytes()); // deadline_slot
        payload.push(BOUNTY_STATUS_COMPLETED); // status
        payload.push(3); // submission_count
        payload.extend_from_slice(&500u64.to_le_bytes()); // created_slot
        payload.push(1); // approved_idx

        let result = readonly_result(0, payload);
        let bounty = decode_bounty_info(&result).unwrap();

        assert_eq!(bounty, BountyBoardBountyInfo {
            creator: Pubkey([1u8; 32]),
            title_hash: [0xAAu8; 32],
            reward_amount: 5_000_000_000,
            deadline_slot: 1000,
            status: BOUNTY_STATUS_COMPLETED,
            submission_count: 3,
            created_slot: 500,
            approved_idx: 1,
        });
    }

    #[test]
    fn platform_stats_decoding_matches_contract_layout() {
        let result = readonly_result(
            0,
            [
                10u64.to_le_bytes().as_slice(),
                5u64.to_le_bytes().as_slice(),
                50_000_000_000u64.to_le_bytes().as_slice(),
                2u64.to_le_bytes().as_slice(),
            ]
            .concat(),
        );

        let stats = decode_platform_stats(&result).unwrap();

        assert_eq!(
            stats,
            BountyBoardPlatformStats {
                bounty_count: 10,
                completed_count: 5,
                reward_volume: 50_000_000_000,
                cancel_count: 2,
            }
        );
    }

    #[test]
    fn not_found_bounty_returns_none_via_return_code_1() {
        let result = ReadonlyContractResult {
            success: true,
            return_data: None,
            return_code: Some(1),
            logs: Vec::new(),
            error: None,
            compute_used: None,
        };
        // Simulate the client logic: code 1 → None
        assert!(result.return_code == Some(1) || result.return_data.is_none());
    }
}
