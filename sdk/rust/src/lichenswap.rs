use crate::client::ReadonlyContractResult;
use crate::{Client, Error, Keypair, Pubkey, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

const PROGRAM_SYMBOL_CANDIDATES: [&str; 2] = ["LICHENSWAP", "lichenswap"];
const POOL_INFO_SIZE: usize = 24;
const TWAP_CUMULATIVES_SIZE: usize = 24;
const VOLUME_TOTALS_SIZE: usize = 16;
const SWAP_STATS_SIZE: usize = 40;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LichenSwapPoolInfo {
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub total_liquidity: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LichenSwapVolumeTotals {
    pub volume_a: u64,
    pub volume_b: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LichenSwapProtocolFees {
    pub fees_a: u64,
    pub fees_b: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LichenSwapTwapCumulatives {
    pub cumulative_price_a: u64,
    pub cumulative_price_b: u64,
    pub last_updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LichenSwapSwapStats {
    pub swap_count: u64,
    pub volume_a: u64,
    pub volume_b: u64,
    pub pool_count: u64,
    pub total_liquidity: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LichenSwapStats {
    pub swap_count: u64,
    pub volume_a: u64,
    pub volume_b: u64,
    pub paused: bool,
}

#[derive(Debug, Clone)]
pub struct CreatePoolParams {
    pub token_a: Pubkey,
    pub token_b: Pubkey,
}

#[derive(Debug, Clone)]
pub struct AddLiquidityParams {
    pub amount_a: u64,
    pub amount_b: u64,
    pub min_liquidity: u64,
    pub value_spores: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SwapParams {
    pub amount_in: u64,
    pub min_amount_out: u64,
    pub value_spores: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SwapWithDeadlineParams {
    pub amount_in: u64,
    pub min_amount_out: u64,
    pub deadline: u64,
    pub value_spores: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct LichenSwapClient {
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

fn encode_create_pool_args(params: &CreatePoolParams) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x20],
        &[
            params.token_a.as_ref().to_vec(),
            params.token_b.as_ref().to_vec(),
        ],
    )
}

fn encode_add_liquidity_args(provider: &Pubkey, params: &AddLiquidityParams) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x08, 0x08, 0x08],
        &[
            provider.as_ref().to_vec(),
            params.amount_a.to_le_bytes().to_vec(),
            params.amount_b.to_le_bytes().to_vec(),
            params.min_liquidity.to_le_bytes().to_vec(),
        ],
    )
}

fn encode_swap_args(params: &SwapParams, a_to_b: bool) -> Vec<u8> {
    build_layout_args(
        &[0x08, 0x08, 0x04],
        &[
            params.amount_in.to_le_bytes().to_vec(),
            params.min_amount_out.to_le_bytes().to_vec(),
            (u32::from(a_to_b)).to_le_bytes().to_vec(),
        ],
    )
}

fn encode_directional_swap_args(params: &SwapParams) -> Vec<u8> {
    build_layout_args(
        &[0x08, 0x08],
        &[
            params.amount_in.to_le_bytes().to_vec(),
            params.min_amount_out.to_le_bytes().to_vec(),
        ],
    )
}

fn encode_directional_swap_with_deadline_args(params: &SwapWithDeadlineParams) -> Vec<u8> {
    build_layout_args(
        &[0x08, 0x08, 0x08],
        &[
            params.amount_in.to_le_bytes().to_vec(),
            params.min_amount_out.to_le_bytes().to_vec(),
            params.deadline.to_le_bytes().to_vec(),
        ],
    )
}

fn encode_quote_args(amount_in: u64, a_to_b: bool) -> Vec<u8> {
    build_layout_args(
        &[0x08, 0x04],
        &[
            amount_in.to_le_bytes().to_vec(),
            (u32::from(a_to_b)).to_le_bytes().to_vec(),
        ],
    )
}

fn encode_provider_args(provider: &Pubkey) -> Vec<u8> {
    build_layout_args(&[0x20], &[provider.as_ref().to_vec()])
}

fn encode_amount_args(amount: u64) -> Vec<u8> {
    build_layout_args(&[0x08], &[amount.to_le_bytes().to_vec()])
}

fn ensure_readonly_success(
    result: &ReadonlyContractResult,
    function_name: &str,
    allowed_codes: &[u32],
) -> Result<()> {
    let code = result.return_code.unwrap_or(0);
    if !allowed_codes.contains(&code) {
        return Err(Error::RpcError(result.error.clone().unwrap_or_else(|| {
            format!("LichenSwap {} returned code {}", function_name, code)
        })));
    }
    if !result.success {
        return Err(Error::RpcError(
            result
                .error
                .clone()
                .unwrap_or_else(|| format!("LichenSwap {} failed", function_name)),
        ));
    }
    Ok(())
}

fn decode_return_data(result: &ReadonlyContractResult, function_name: &str) -> Result<Vec<u8>> {
    let Some(return_data) = &result.return_data else {
        return Err(Error::ParseError(format!(
            "LichenSwap {} did not return payload data",
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
            "LichenSwap {} payload was shorter than expected",
            function_name,
        )));
    }
    let slice: [u8; 8] = bytes[start..end].try_into().map_err(|_| {
        Error::ParseError(format!(
            "LichenSwap {} payload was malformed",
            function_name
        ))
    })?;
    Ok(u64::from_le_bytes(slice))
}

fn decode_u64_result(result: &ReadonlyContractResult, function_name: &str) -> Result<u64> {
    ensure_readonly_success(result, function_name, &[0])?;
    let bytes = decode_return_data(result, function_name)?;
    decode_u64(&bytes, 0, function_name)
}

fn decode_pool_info(result: &ReadonlyContractResult) -> Result<LichenSwapPoolInfo> {
    ensure_readonly_success(result, "get_pool_info", &[0, 1])?;
    let bytes = decode_return_data(result, "get_pool_info")?;
    if bytes.len() < POOL_INFO_SIZE {
        return Err(Error::ParseError(
            "LichenSwap get_pool_info payload was shorter than expected".into(),
        ));
    }

    Ok(LichenSwapPoolInfo {
        reserve_a: decode_u64(&bytes, 0, "get_pool_info")?,
        reserve_b: decode_u64(&bytes, 8, "get_pool_info")?,
        total_liquidity: decode_u64(&bytes, 16, "get_pool_info")?,
    })
}

fn decode_twap_cumulatives(result: &ReadonlyContractResult) -> Result<LichenSwapTwapCumulatives> {
    ensure_readonly_success(result, "get_twap_cumulatives", &[0])?;
    let bytes = decode_return_data(result, "get_twap_cumulatives")?;
    if bytes.len() < TWAP_CUMULATIVES_SIZE {
        return Err(Error::ParseError(
            "LichenSwap get_twap_cumulatives payload was shorter than expected".into(),
        ));
    }

    Ok(LichenSwapTwapCumulatives {
        cumulative_price_a: decode_u64(&bytes, 0, "get_twap_cumulatives")?,
        cumulative_price_b: decode_u64(&bytes, 8, "get_twap_cumulatives")?,
        last_updated_at: decode_u64(&bytes, 16, "get_twap_cumulatives")?,
    })
}

fn decode_protocol_fees(result: &ReadonlyContractResult) -> Result<LichenSwapProtocolFees> {
    ensure_readonly_success(result, "get_protocol_fees", &[0])?;
    let bytes = decode_return_data(result, "get_protocol_fees")?;
    if bytes.len() < VOLUME_TOTALS_SIZE {
        return Err(Error::ParseError(
            "LichenSwap get_protocol_fees payload was shorter than expected".into(),
        ));
    }

    Ok(LichenSwapProtocolFees {
        fees_a: decode_u64(&bytes, 0, "get_protocol_fees")?,
        fees_b: decode_u64(&bytes, 8, "get_protocol_fees")?,
    })
}

fn decode_volume_totals(
    result: &ReadonlyContractResult,
    function_name: &str,
) -> Result<LichenSwapVolumeTotals> {
    ensure_readonly_success(result, function_name, &[0])?;
    let bytes = decode_return_data(result, function_name)?;
    if bytes.len() < VOLUME_TOTALS_SIZE {
        return Err(Error::ParseError(format!(
            "LichenSwap {} payload was shorter than expected",
            function_name,
        )));
    }

    Ok(LichenSwapVolumeTotals {
        volume_a: decode_u64(&bytes, 0, function_name)?,
        volume_b: decode_u64(&bytes, 8, function_name)?,
    })
}

fn decode_swap_stats(result: &ReadonlyContractResult) -> Result<LichenSwapSwapStats> {
    ensure_readonly_success(result, "get_swap_stats", &[0])?;
    let bytes = decode_return_data(result, "get_swap_stats")?;
    if bytes.len() < SWAP_STATS_SIZE {
        return Err(Error::ParseError(
            "LichenSwap get_swap_stats payload was shorter than expected".into(),
        ));
    }

    Ok(LichenSwapSwapStats {
        swap_count: decode_u64(&bytes, 0, "get_swap_stats")?,
        volume_a: decode_u64(&bytes, 8, "get_swap_stats")?,
        volume_b: decode_u64(&bytes, 16, "get_swap_stats")?,
        pool_count: decode_u64(&bytes, 24, "get_swap_stats")?,
        total_liquidity: decode_u64(&bytes, 32, "get_swap_stats")?,
    })
}

impl LichenSwapClient {
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
            .map_err(|_| Error::ConfigError("LichenSwapClient program cache lock poisoned".into()))?
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
                Error::ConfigError("LichenSwapClient program cache lock poisoned".into())
            })? = Some(program_id);
            return Ok(program_id);
        }

        Err(Error::ConfigError(
            "Unable to resolve the LichenSwap program via getSymbolRegistry(\"LICHENSWAP\")".into(),
        ))
    }

    pub async fn get_pool_info(&self) -> Result<LichenSwapPoolInfo> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_pool_info",
                Vec::new(),
                None,
            )
            .await?;
        decode_pool_info(&result)
    }

    pub async fn get_quote(&self, amount_in: u64, a_to_b: bool) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_quote",
                encode_quote_args(amount_in, a_to_b),
                None,
            )
            .await?;
        decode_u64_result(&result, "get_quote")
    }

    pub async fn get_liquidity_balance(&self, provider: &Pubkey) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_liquidity_balance",
                encode_provider_args(provider),
                None,
            )
            .await?;
        decode_u64_result(&result, "get_liquidity_balance")
    }

    pub async fn get_total_liquidity(&self) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_total_liquidity",
                Vec::new(),
                None,
            )
            .await?;
        decode_u64_result(&result, "get_total_liquidity")
    }

    pub async fn get_flash_loan_fee(&self, amount: u64) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_flash_loan_fee",
                encode_amount_args(amount),
                None,
            )
            .await?;
        decode_u64_result(&result, "get_flash_loan_fee")
    }

    pub async fn get_twap_cumulatives(&self) -> Result<LichenSwapTwapCumulatives> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_twap_cumulatives",
                Vec::new(),
                None,
            )
            .await?;
        decode_twap_cumulatives(&result)
    }

    pub async fn get_twap_snapshot_count(&self) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_twap_snapshot_count",
                Vec::new(),
                None,
            )
            .await?;
        decode_u64_result(&result, "get_twap_snapshot_count")
    }

    pub async fn get_protocol_fees(&self) -> Result<LichenSwapProtocolFees> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_protocol_fees",
                Vec::new(),
                None,
            )
            .await?;
        decode_protocol_fees(&result)
    }

    pub async fn get_pool_count(&self) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_pool_count",
                Vec::new(),
                None,
            )
            .await?;
        decode_u64_result(&result, "get_pool_count")
    }

    pub async fn get_swap_count(&self) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_swap_count",
                Vec::new(),
                None,
            )
            .await?;
        decode_u64_result(&result, "get_swap_count")
    }

    pub async fn get_total_volume(&self) -> Result<LichenSwapVolumeTotals> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_total_volume",
                Vec::new(),
                None,
            )
            .await?;
        decode_volume_totals(&result, "get_total_volume")
    }

    pub async fn get_swap_stats(&self) -> Result<LichenSwapSwapStats> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_swap_stats",
                Vec::new(),
                None,
            )
            .await?;
        decode_swap_stats(&result)
    }

    pub async fn get_stats(&self) -> Result<LichenSwapStats> {
        let value = self.client.get_lichenswap_stats().await?;
        serde_json::from_value(value).map_err(|err| Error::ParseError(err.to_string()))
    }

    pub async fn create_pool(&self, owner: &Keypair, params: CreatePoolParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                owner,
                &program_id,
                "create_pool",
                encode_create_pool_args(&params),
                0,
            )
            .await
    }

    pub async fn add_liquidity(
        &self,
        provider: &Keypair,
        params: AddLiquidityParams,
    ) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let value = match params.value_spores {
            Some(value) => value,
            None => params
                .amount_a
                .checked_add(params.amount_b)
                .ok_or_else(|| {
                    Error::BuildError(
                        "LichenSwap add_liquidity default value overflowed u64".into(),
                    )
                })?,
        };
        self.client
            .call_contract(
                provider,
                &program_id,
                "add_liquidity",
                encode_add_liquidity_args(&provider.pubkey(), &params),
                value,
            )
            .await
    }

    pub async fn swap(&self, trader: &Keypair, params: SwapParams, a_to_b: bool) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let value = params.value_spores.unwrap_or(params.amount_in);
        self.client
            .call_contract(
                trader,
                &program_id,
                "swap",
                encode_swap_args(&params, a_to_b),
                value,
            )
            .await
    }

    pub async fn swap_a_for_b(&self, trader: &Keypair, params: SwapParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let value = params.value_spores.unwrap_or(params.amount_in);
        self.client
            .call_contract(
                trader,
                &program_id,
                "swap_a_for_b",
                encode_directional_swap_args(&params),
                value,
            )
            .await
    }

    pub async fn swap_b_for_a(&self, trader: &Keypair, params: SwapParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let value = params.value_spores.unwrap_or(params.amount_in);
        self.client
            .call_contract(
                trader,
                &program_id,
                "swap_b_for_a",
                encode_directional_swap_args(&params),
                value,
            )
            .await
    }

    pub async fn swap_a_for_b_with_deadline(
        &self,
        trader: &Keypair,
        params: SwapWithDeadlineParams,
    ) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let value = params.value_spores.unwrap_or(params.amount_in);
        self.client
            .call_contract(
                trader,
                &program_id,
                "swap_a_for_b_with_deadline",
                encode_directional_swap_with_deadline_args(&params),
                value,
            )
            .await
    }

    pub async fn swap_b_for_a_with_deadline(
        &self,
        trader: &Keypair,
        params: SwapWithDeadlineParams,
    ) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let value = params.value_spores.unwrap_or(params.amount_in);
        self.client
            .call_contract(
                trader,
                &program_id,
                "swap_b_for_a_with_deadline",
                encode_directional_swap_with_deadline_args(&params),
                value,
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
    fn create_pool_encoding_matches_named_export_layout() {
        let params = CreatePoolParams {
            token_a: Pubkey([1u8; 32]),
            token_b: Pubkey([2u8; 32]),
        };

        let encoded = encode_create_pool_args(&params);

        assert_eq!(&encoded[..3], &[0xAB, 0x20, 0x20]);
        assert_eq!(&encoded[3..35], &[1u8; 32]);
        assert_eq!(&encoded[35..67], &[2u8; 32]);
    }

    #[test]
    fn add_liquidity_encoding_includes_provider_and_three_u64_values() {
        let provider = Pubkey([3u8; 32]);
        let encoded = encode_add_liquidity_args(
            &provider,
            &AddLiquidityParams {
                amount_a: 50,
                amount_b: 75,
                min_liquidity: 10,
                value_spores: None,
            },
        );

        assert_eq!(&encoded[..5], &[0xAB, 0x20, 0x08, 0x08, 0x08]);
        assert_eq!(&encoded[5..37], &[3u8; 32]);
        assert_eq!(u64::from_le_bytes(encoded[37..45].try_into().unwrap()), 50);
        assert_eq!(u64::from_le_bytes(encoded[45..53].try_into().unwrap()), 75);
        assert_eq!(u64::from_le_bytes(encoded[53..61].try_into().unwrap()), 10);
    }

    #[test]
    fn swap_encoding_includes_direction_flag() {
        let encoded = encode_swap_args(
            &SwapParams {
                amount_in: 40,
                min_amount_out: 35,
                value_spores: None,
            },
            false,
        );

        assert_eq!(&encoded[..4], &[0xAB, 0x08, 0x08, 0x04]);
        assert_eq!(u64::from_le_bytes(encoded[4..12].try_into().unwrap()), 40);
        assert_eq!(u64::from_le_bytes(encoded[12..20].try_into().unwrap()), 35);
        assert_eq!(u32::from_le_bytes(encoded[20..24].try_into().unwrap()), 0);
    }

    #[test]
    fn pool_info_decoding_allows_success_code_one() {
        let result = readonly_result(
            1,
            [
                1_000u64.to_le_bytes().as_slice(),
                2_000u64.to_le_bytes().as_slice(),
                3_000u64.to_le_bytes().as_slice(),
            ]
            .concat(),
        );

        let pool = decode_pool_info(&result).unwrap();

        assert_eq!(
            pool,
            LichenSwapPoolInfo {
                reserve_a: 1_000,
                reserve_b: 2_000,
                total_liquidity: 3_000,
            }
        );
    }

    #[test]
    fn swap_stats_decoding_matches_contract_payload_layout() {
        let result = readonly_result(
            0,
            [
                9u64.to_le_bytes().as_slice(),
                100u64.to_le_bytes().as_slice(),
                200u64.to_le_bytes().as_slice(),
                2u64.to_le_bytes().as_slice(),
                3_000u64.to_le_bytes().as_slice(),
            ]
            .concat(),
        );

        let stats = decode_swap_stats(&result).unwrap();

        assert_eq!(
            stats,
            LichenSwapSwapStats {
                swap_count: 9,
                volume_a: 100,
                volume_b: 200,
                pool_count: 2,
                total_liquidity: 3_000,
            }
        );
    }
}
