use crate::client::ReadonlyContractResult;
use crate::{Client, Error, Keypair, Pubkey, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

const PROGRAM_SYMBOL_CANDIDATES: [&str; 5] = ["SPOREVAULT", "sporevault", "SporeVault", "VAULT", "vault"];
const VAULT_STATS_SIZE: usize = 48;
const USER_POSITION_SIZE: usize = 16;
const STRATEGY_INFO_SIZE: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SporeVaultVaultStats {
    pub total_assets: u64,
    pub total_shares: u64,
    pub share_price_e9: u64,
    pub strategy_count: u64,
    pub total_earned: u64,
    pub fees_earned: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SporeVaultUserPosition {
    pub shares: u64,
    pub estimated_value: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SporeVaultStrategyInfo {
    pub strategy_type: u64,
    pub allocation_percent: u64,
    pub deployed_amount: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SporeVaultStats {
    pub total_assets: u64,
    pub total_shares: u64,
    pub strategy_count: u64,
    pub total_earned: u64,
    pub fees_earned: u64,
    pub protocol_fees: u64,
    pub paused: bool,
}

#[derive(Debug, Clone)]
pub struct SporeVaultClient {
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

fn encode_user_amount_args(user: &Pubkey, amount: u64) -> Vec<u8> {
    build_layout_args(&[0x20, 0x08], &[user.as_ref().to_vec(), amount.to_le_bytes().to_vec()])
}

fn encode_user_lookup_args(user: &Pubkey) -> Vec<u8> {
    build_layout_args(&[0x20], &[user.as_ref().to_vec()])
}

fn encode_index_args(index: u64) -> Vec<u8> {
    build_layout_args(&[0x08], &[index.to_le_bytes().to_vec()])
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
                .unwrap_or_else(|| format!("SporeVault {} returned code {}", function_name, code)),
        ));
    }
    if !result.success {
        return Err(Error::RpcError(
            result
                .error
                .clone()
                .unwrap_or_else(|| format!("SporeVault {} failed", function_name)),
        ));
    }
    Ok(())
}

fn decode_return_data(result: &ReadonlyContractResult, function_name: &str) -> Result<Vec<u8>> {
    let Some(return_data) = &result.return_data else {
        return Err(Error::ParseError(format!(
            "SporeVault {} did not return payload data",
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
            "SporeVault {} payload was shorter than expected",
            function_name,
        )));
    }
    let slice: [u8; 8] = bytes[start..end]
        .try_into()
        .map_err(|_| Error::ParseError(format!("SporeVault {} payload was malformed", function_name)))?;
    Ok(u64::from_le_bytes(slice))
}

fn decode_vault_stats(result: &ReadonlyContractResult) -> Result<SporeVaultVaultStats> {
    ensure_readonly_success(result, "get_vault_stats", &[0])?;
    let bytes = decode_return_data(result, "get_vault_stats")?;
    if bytes.len() < VAULT_STATS_SIZE {
        return Err(Error::ParseError(
            "SporeVault get_vault_stats payload was shorter than expected".into(),
        ));
    }

    Ok(SporeVaultVaultStats {
        total_assets: decode_u64(&bytes, 0, "get_vault_stats")?,
        total_shares: decode_u64(&bytes, 8, "get_vault_stats")?,
        share_price_e9: decode_u64(&bytes, 16, "get_vault_stats")?,
        strategy_count: decode_u64(&bytes, 24, "get_vault_stats")?,
        total_earned: decode_u64(&bytes, 32, "get_vault_stats")?,
        fees_earned: decode_u64(&bytes, 40, "get_vault_stats")?,
    })
}

fn decode_user_position(result: &ReadonlyContractResult) -> Result<SporeVaultUserPosition> {
    ensure_readonly_success(result, "get_user_position", &[0])?;
    let bytes = decode_return_data(result, "get_user_position")?;
    if bytes.len() < USER_POSITION_SIZE {
        return Err(Error::ParseError(
            "SporeVault get_user_position payload was shorter than expected".into(),
        ));
    }

    Ok(SporeVaultUserPosition {
        shares: decode_u64(&bytes, 0, "get_user_position")?,
        estimated_value: decode_u64(&bytes, 8, "get_user_position")?,
    })
}

fn decode_strategy_info(result: &ReadonlyContractResult) -> Result<SporeVaultStrategyInfo> {
    ensure_readonly_success(result, "get_strategy_info", &[0])?;
    let bytes = decode_return_data(result, "get_strategy_info")?;
    if bytes.len() < STRATEGY_INFO_SIZE {
        return Err(Error::ParseError(
            "SporeVault get_strategy_info payload was shorter than expected".into(),
        ));
    }

    Ok(SporeVaultStrategyInfo {
        strategy_type: decode_u64(&bytes, 0, "get_strategy_info")?,
        allocation_percent: decode_u64(&bytes, 8, "get_strategy_info")?,
        deployed_amount: decode_u64(&bytes, 16, "get_strategy_info")?,
    })
}

impl SporeVaultClient {
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
            .map_err(|_| Error::ConfigError("SporeVaultClient program cache lock poisoned".into()))?
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
                .map_err(|_| Error::ConfigError("SporeVaultClient program cache lock poisoned".into()))? = Some(program_id);
            return Ok(program_id);
        }

        Err(Error::ConfigError(
            "Unable to resolve the SporeVault program via getSymbolRegistry(\"SPOREVAULT\")".into(),
        ))
    }

    pub async fn get_vault_stats(&self) -> Result<SporeVaultVaultStats> {
        let result = self
            .client
            .call_readonly_contract(&self.get_program_id().await?, "get_vault_stats", Vec::new(), None)
            .await?;
        decode_vault_stats(&result)
    }

    pub async fn get_user_position(&self, user: &Pubkey) -> Result<SporeVaultUserPosition> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_user_position",
                encode_user_lookup_args(user),
                None,
            )
            .await?;
        decode_user_position(&result)
    }

    pub async fn get_strategy_info(&self, index: u64) -> Result<Option<SporeVaultStrategyInfo>> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_strategy_info",
                encode_index_args(index),
                None,
            )
            .await?;

        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }

        decode_strategy_info(&result).map(Some)
    }

    pub async fn get_stats(&self) -> Result<SporeVaultStats> {
        let value = self.client.get_sporevault_stats().await?;
        serde_json::from_value(value).map_err(|err| Error::ParseError(err.to_string()))
    }

    pub async fn deposit(&self, depositor: &Keypair, amount: u64) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                depositor,
                &program_id,
                "deposit",
                encode_user_amount_args(&depositor.pubkey(), amount),
                amount,
            )
            .await
    }

    pub async fn withdraw(&self, depositor: &Keypair, shares_to_burn: u64) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                depositor,
                &program_id,
                "withdraw",
                encode_user_amount_args(&depositor.pubkey(), shares_to_burn),
                0,
            )
            .await
    }

    pub async fn harvest(&self, caller: &Keypair) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(caller, &program_id, "harvest", Vec::new(), 0)
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
    fn user_amount_encoding_matches_named_export_layout() {
        let user = Pubkey([7u8; 32]);
        let encoded = encode_user_amount_args(&user, 1_000);

        assert_eq!(&encoded[..3], &[0xAB, 0x20, 0x08]);
        assert_eq!(&encoded[3..35], &[7u8; 32]);
        assert_eq!(u64::from_le_bytes(encoded[35..43].try_into().unwrap()), 1_000);
    }

    #[test]
    fn index_encoding_matches_named_export_layout() {
        let encoded = encode_index_args(3);

        assert_eq!(&encoded[..2], &[0xAB, 0x08]);
        assert_eq!(u64::from_le_bytes(encoded[2..10].try_into().unwrap()), 3);
    }

    #[test]
    fn vault_stats_and_user_position_decoding_match_contract_layouts() {
        let vault_result = readonly_result(
            0,
            [
                5_000u64.to_le_bytes().as_slice(),
                4_500u64.to_le_bytes().as_slice(),
                1_111_111_111u64.to_le_bytes().as_slice(),
                2u64.to_le_bytes().as_slice(),
                900u64.to_le_bytes().as_slice(),
                100u64.to_le_bytes().as_slice(),
            ]
            .concat(),
        );

        let user_result = readonly_result(
            0,
            [200u64.to_le_bytes().as_slice(), 222u64.to_le_bytes().as_slice()].concat(),
        );

        let vault_stats = decode_vault_stats(&vault_result).unwrap();
        let user_position = decode_user_position(&user_result).unwrap();

        assert_eq!(
            vault_stats,
            SporeVaultVaultStats {
                total_assets: 5_000,
                total_shares: 4_500,
                share_price_e9: 1_111_111_111,
                strategy_count: 2,
                total_earned: 900,
                fees_earned: 100,
            }
        );
        assert_eq!(
            user_position,
            SporeVaultUserPosition {
                shares: 200,
                estimated_value: 222,
            }
        );
    }

    #[test]
    fn strategy_info_decoding_matches_contract_layout() {
        let result = readonly_result(
            0,
            [1u64.to_le_bytes().as_slice(), 60u64.to_le_bytes().as_slice(), 3_000u64.to_le_bytes().as_slice()].concat(),
        );

        let strategy = decode_strategy_info(&result).unwrap();

        assert_eq!(
            strategy,
            SporeVaultStrategyInfo {
                strategy_type: 1,
                allocation_percent: 60,
                deployed_amount: 3_000,
            }
        );
    }
}