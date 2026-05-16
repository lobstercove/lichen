use crate::client::ReadonlyContractResult;
use crate::{Client, Error, Keypair, Pubkey, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

const PROGRAM_SYMBOL_CANDIDATES: [&str; 4] = ["LEND", "lend", "THALLLEND", "thalllend"];
const ACCOUNT_INFO_SIZE: usize = 24;
const PROTOCOL_STATS_SIZE: usize = 32;
const INTEREST_RATE_SIZE: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThallLendAccountInfo {
    pub deposit: u64,
    pub borrow: u64,
    pub health_factor_bps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThallLendProtocolStats {
    pub total_deposits: u64,
    pub total_borrows: u64,
    pub utilization_pct: u64,
    pub reserves: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThallLendInterestRate {
    pub rate_per_slot: u64,
    pub utilization_pct: u64,
    pub total_available: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThallLendStats {
    pub total_deposits: u64,
    pub total_borrows: u64,
    pub reserves: u64,
    pub deposit_count: u64,
    pub borrow_count: u64,
    pub liquidation_count: u64,
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiquidateParams {
    pub borrower: Pubkey,
    pub repay_amount: u64,
}

#[derive(Debug, Clone)]
pub struct ThallLendClient {
    client: Client,
    program_id: Arc<Mutex<Option<Pubkey>>>,
}

fn build_layout_args(layout: &[u8], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        1 + layout.len() + chunks.iter().map(|chunk| chunk.len()).sum::<usize>(),
    );
    out.push(0xAB);
    out.extend_from_slice(layout);
    for chunk in chunks {
        out.extend_from_slice(chunk);
    }
    out
}

fn encode_user_amount_args(user: &Pubkey, amount: u64) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x08],
        &[user.as_ref().to_vec(), amount.to_le_bytes().to_vec()],
    )
}

fn encode_user_lookup_args(user: &Pubkey) -> Vec<u8> {
    build_layout_args(&[0x20], &[user.as_ref().to_vec()])
}

fn encode_liquidate_args(liquidator: &Pubkey, params: &LiquidateParams) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x20, 0x08],
        &[
            liquidator.as_ref().to_vec(),
            params.borrower.as_ref().to_vec(),
            params.repay_amount.to_le_bytes().to_vec(),
        ],
    )
}

fn ensure_readonly_success(
    result: &ReadonlyContractResult,
    function_name: &str,
    allowed_codes: &[u32],
) -> Result<()> {
    let code = result.return_code.unwrap_or(0);
    if !allowed_codes.contains(&code) {
        return Err(Error::RpcError(result.error.clone().unwrap_or_else(|| {
            format!("ThallLend {} returned code {}", function_name, code)
        })));
    }
    if !result.success {
        return Err(Error::RpcError(
            result
                .error
                .clone()
                .unwrap_or_else(|| format!("ThallLend {} failed", function_name)),
        ));
    }
    Ok(())
}

fn decode_return_data(result: &ReadonlyContractResult, function_name: &str) -> Result<Vec<u8>> {
    let Some(return_data) = &result.return_data else {
        return Err(Error::ParseError(format!(
            "ThallLend {} did not return payload data",
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
            "ThallLend {} payload was shorter than expected",
            function_name,
        )));
    }
    let slice: [u8; 8] = bytes[start..end].try_into().map_err(|_| {
        Error::ParseError(format!("ThallLend {} payload was malformed", function_name))
    })?;
    Ok(u64::from_le_bytes(slice))
}

fn decode_u64_result(result: &ReadonlyContractResult, function_name: &str) -> Result<u64> {
    ensure_readonly_success(result, function_name, &[0])?;
    let bytes = decode_return_data(result, function_name)?;
    decode_u64(&bytes, 0, function_name)
}

fn decode_account_info(result: &ReadonlyContractResult) -> Result<ThallLendAccountInfo> {
    ensure_readonly_success(result, "get_account_info", &[0])?;
    let bytes = decode_return_data(result, "get_account_info")?;
    if bytes.len() < ACCOUNT_INFO_SIZE {
        return Err(Error::ParseError(
            "ThallLend get_account_info payload was shorter than expected".into(),
        ));
    }

    Ok(ThallLendAccountInfo {
        deposit: decode_u64(&bytes, 0, "get_account_info")?,
        borrow: decode_u64(&bytes, 8, "get_account_info")?,
        health_factor_bps: decode_u64(&bytes, 16, "get_account_info")?,
    })
}

fn decode_protocol_stats(result: &ReadonlyContractResult) -> Result<ThallLendProtocolStats> {
    ensure_readonly_success(result, "get_protocol_stats", &[0])?;
    let bytes = decode_return_data(result, "get_protocol_stats")?;
    if bytes.len() < PROTOCOL_STATS_SIZE {
        return Err(Error::ParseError(
            "ThallLend get_protocol_stats payload was shorter than expected".into(),
        ));
    }

    Ok(ThallLendProtocolStats {
        total_deposits: decode_u64(&bytes, 0, "get_protocol_stats")?,
        total_borrows: decode_u64(&bytes, 8, "get_protocol_stats")?,
        utilization_pct: decode_u64(&bytes, 16, "get_protocol_stats")?,
        reserves: decode_u64(&bytes, 24, "get_protocol_stats")?,
    })
}

fn decode_interest_rate(result: &ReadonlyContractResult) -> Result<ThallLendInterestRate> {
    ensure_readonly_success(result, "get_interest_rate", &[0])?;
    let bytes = decode_return_data(result, "get_interest_rate")?;
    if bytes.len() < INTEREST_RATE_SIZE {
        return Err(Error::ParseError(
            "ThallLend get_interest_rate payload was shorter than expected".into(),
        ));
    }

    Ok(ThallLendInterestRate {
        rate_per_slot: decode_u64(&bytes, 0, "get_interest_rate")?,
        utilization_pct: decode_u64(&bytes, 8, "get_interest_rate")?,
        total_available: decode_u64(&bytes, 16, "get_interest_rate")?,
    })
}

impl ThallLendClient {
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
            .map_err(|_| Error::ConfigError("ThallLendClient program cache lock poisoned".into()))?
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
            *self.program_id.lock().map_err(|_| {
                Error::ConfigError("ThallLendClient program cache lock poisoned".into())
            })? = Some(program_id);
            return Ok(program_id);
        }

        Err(Error::ConfigError(
            "Unable to resolve the ThallLend program via getSymbolRegistry(\"LEND\")".into(),
        ))
    }

    pub async fn get_account_info(&self, user: &Pubkey) -> Result<ThallLendAccountInfo> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_account_info",
                encode_user_lookup_args(user),
                None,
            )
            .await?;
        decode_account_info(&result)
    }

    pub async fn get_protocol_stats(&self) -> Result<ThallLendProtocolStats> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_protocol_stats",
                Vec::new(),
                None,
            )
            .await?;
        decode_protocol_stats(&result)
    }

    pub async fn get_interest_rate(&self) -> Result<ThallLendInterestRate> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_interest_rate",
                Vec::new(),
                None,
            )
            .await?;
        decode_interest_rate(&result)
    }

    pub async fn get_deposit_count(&self) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_deposit_count",
                Vec::new(),
                None,
            )
            .await?;
        decode_u64_result(&result, "get_deposit_count")
    }

    pub async fn get_borrow_count(&self) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_borrow_count",
                Vec::new(),
                None,
            )
            .await?;
        decode_u64_result(&result, "get_borrow_count")
    }

    pub async fn get_liquidation_count(&self) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_liquidation_count",
                Vec::new(),
                None,
            )
            .await?;
        decode_u64_result(&result, "get_liquidation_count")
    }

    pub async fn get_stats(&self) -> Result<ThallLendStats> {
        let value = self.client.get_thalllend_stats().await?;
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

    pub async fn withdraw(&self, depositor: &Keypair, amount: u64) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                depositor,
                &program_id,
                "withdraw",
                encode_user_amount_args(&depositor.pubkey(), amount),
                0,
            )
            .await
    }

    pub async fn borrow(&self, borrower: &Keypair, amount: u64) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                borrower,
                &program_id,
                "borrow",
                encode_user_amount_args(&borrower.pubkey(), amount),
                0,
            )
            .await
    }

    pub async fn repay(&self, borrower: &Keypair, amount: u64) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                borrower,
                &program_id,
                "repay",
                encode_user_amount_args(&borrower.pubkey(), amount),
                amount,
            )
            .await
    }

    pub async fn liquidate(&self, liquidator: &Keypair, params: LiquidateParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                liquidator,
                &program_id,
                "liquidate",
                encode_liquidate_args(&liquidator.pubkey(), &params),
                params.repay_amount,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readonly_result(bytes: Vec<u8>) -> ReadonlyContractResult {
        ReadonlyContractResult {
            success: true,
            return_data: Some(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                bytes,
            )),
            return_code: Some(0),
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
        assert_eq!(
            u64::from_le_bytes(encoded[35..43].try_into().unwrap()),
            1_000
        );
    }

    #[test]
    fn liquidate_encoding_includes_borrower_and_repay_amount() {
        let liquidator = Pubkey([8u8; 32]);
        let params = LiquidateParams {
            borrower: Pubkey([9u8; 32]),
            repay_amount: 250,
        };

        let encoded = encode_liquidate_args(&liquidator, &params);

        assert_eq!(&encoded[..4], &[0xAB, 0x20, 0x20, 0x08]);
        assert_eq!(&encoded[4..36], &[8u8; 32]);
        assert_eq!(&encoded[36..68], &[9u8; 32]);
        assert_eq!(u64::from_le_bytes(encoded[68..76].try_into().unwrap()), 250);
    }

    #[test]
    fn account_info_decoding_matches_contract_layout() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1_000_u64.to_le_bytes());
        payload.extend_from_slice(&400_u64.to_le_bytes());
        payload.extend_from_slice(&21_250_u64.to_le_bytes());

        let decoded = decode_account_info(&readonly_result(payload)).unwrap();

        assert_eq!(decoded.deposit, 1_000);
        assert_eq!(decoded.borrow, 400);
        assert_eq!(decoded.health_factor_bps, 21_250);
    }

    #[test]
    fn protocol_stats_and_interest_rate_decoding_match_contract_layouts() {
        let mut protocol_payload = Vec::new();
        protocol_payload.extend_from_slice(&5_000_u64.to_le_bytes());
        protocol_payload.extend_from_slice(&2_000_u64.to_le_bytes());
        protocol_payload.extend_from_slice(&40_u64.to_le_bytes());
        protocol_payload.extend_from_slice(&150_u64.to_le_bytes());

        let protocol = decode_protocol_stats(&readonly_result(protocol_payload)).unwrap();
        assert_eq!(protocol.total_deposits, 5_000);
        assert_eq!(protocol.total_borrows, 2_000);
        assert_eq!(protocol.utilization_pct, 40);
        assert_eq!(protocol.reserves, 150);

        let mut rate_payload = Vec::new();
        rate_payload.extend_from_slice(&254_u64.to_le_bytes());
        rate_payload.extend_from_slice(&40_u64.to_le_bytes());
        rate_payload.extend_from_slice(&3_000_u64.to_le_bytes());

        let rate = decode_interest_rate(&readonly_result(rate_payload)).unwrap();
        assert_eq!(rate.rate_per_slot, 254);
        assert_eq!(rate.utilization_pct, 40);
        assert_eq!(rate.total_available, 3_000);
    }
}
