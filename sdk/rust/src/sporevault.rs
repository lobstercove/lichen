use crate::client::ReadonlyContractResult;
use crate::{Client, Error, Keypair, Pubkey, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

const PROGRAM_SYMBOL_CANDIDATES: [&str; 5] =
    ["SPOREVAULT", "sporevault", "SporeVault", "VAULT", "vault"];
const VAULT_STATS_SIZE: usize = 48;
const USER_POSITION_SIZE: usize = 16;
const STRATEGY_INFO_SIZE: usize = 24;
const VAULT_STATUS_SIZE: usize = 23 * 8;

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
pub struct SporeVaultStatus {
    pub accounting_version: u64,
    pub paused: bool,
    pub licn_config_present: bool,
    pub licn_config_valid: bool,
    pub native_licn: bool,
    pub thalllend_config_present: bool,
    pub thalllend_config_valid: bool,
    pub strategy_registry_valid: bool,
    pub idle_assets: u64,
    pub lending_assets: u64,
    pub total_assets: u64,
    pub total_shares: u64,
    pub protocol_fees: u64,
    pub real_liquid_custody: u64,
    pub custody_query_ok: bool,
    pub liquid_custody_covers_accounting: bool,
    pub deposit_fee_bps: u64,
    pub withdrawal_fee_bps: u64,
    pub deposit_cap: u64,
    pub risk_tier: u64,
    pub performance_fee_percent: u64,
    pub management_fee_bps: u64,
    pub target_slots_per_year: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SporeVaultStats {
    pub total_assets: u64,
    pub total_shares: u64,
    pub strategy_count: u64,
    pub total_earned: u64,
    pub fees_earned: u64,
    pub protocol_fees: u64,
    #[serde(default)]
    pub idle_assets: u64,
    #[serde(default)]
    pub lending_assets: u64,
    #[serde(default)]
    pub accounting_version: u64,
    #[serde(default)]
    pub deposit_fee_bps: u64,
    #[serde(default)]
    pub withdrawal_fee_bps: u64,
    #[serde(default)]
    pub deposit_cap: u64,
    #[serde(default)]
    pub risk_tier: u64,
    #[serde(default)]
    pub active_lending_strategies: u64,
    #[serde(default)]
    pub lending_strategy_rows: u64,
    #[serde(default)]
    pub strategy_registry_bounded: bool,
    #[serde(default)]
    pub strategy_registry_valid: bool,
    #[serde(default)]
    pub total_strategy_allocation: u64,
    #[serde(default)]
    pub native_licn: bool,
    #[serde(default)]
    pub thalllend_config_valid: bool,
    #[serde(default)]
    pub components_match_total: bool,
    #[serde(default)]
    pub share_state_consistent: bool,
    #[serde(default)]
    pub liquid_custody_covers_accounting: bool,
    pub paused: bool,
    #[serde(default)]
    pub operational: bool,
}

#[derive(Debug, Clone)]
pub struct SporeVaultClient {
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

fn encode_index_args(index: u64) -> Vec<u8> {
    build_layout_args(&[0x08], &[index.to_le_bytes().to_vec()])
}

fn encode_admin_u64_args(admin: &Pubkey, value: u64) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x08],
        &[admin.as_ref().to_vec(), value.to_le_bytes().to_vec()],
    )
}

fn encode_admin_u8_args(admin: &Pubkey, value: u8) -> Vec<u8> {
    build_layout_args(&[0x20, 0x01], &[admin.as_ref().to_vec(), vec![value]])
}

fn encode_admin_strategy_args(admin: &Pubkey, strategy_type: u8, allocation: u64) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x01, 0x08],
        &[
            admin.as_ref().to_vec(),
            vec![strategy_type],
            allocation.to_le_bytes().to_vec(),
        ],
    )
}

fn encode_admin_two_u64_args(admin: &Pubkey, first: u64, second: u64) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x08, 0x08],
        &[
            admin.as_ref().to_vec(),
            first.to_le_bytes().to_vec(),
            second.to_le_bytes().to_vec(),
        ],
    )
}

fn encode_admin_address_args(admin: &Pubkey, address: &Pubkey) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x20],
        &[admin.as_ref().to_vec(), address.as_ref().to_vec()],
    )
}

fn encode_protocol_address_args(
    admin: &Pubkey,
    thalllend: &Pubkey,
    lichenswap: &Pubkey,
) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x20, 0x20],
        &[
            admin.as_ref().to_vec(),
            thalllend.as_ref().to_vec(),
            lichenswap.as_ref().to_vec(),
        ],
    )
}

fn encode_legacy_strategy_retirement_args(
    admin: &Pubkey,
    index: u64,
    expected_type: u8,
    expected_allocation: u64,
    expected_deployed: u64,
) -> Vec<u8> {
    build_layout_args(
        &[0x20, 0x08, 0x01, 0x08, 0x08],
        &[
            admin.as_ref().to_vec(),
            index.to_le_bytes().to_vec(),
            vec![expected_type],
            expected_allocation.to_le_bytes().to_vec(),
            expected_deployed.to_le_bytes().to_vec(),
        ],
    )
}

fn ensure_readonly_success(
    result: &ReadonlyContractResult,
    function_name: &str,
    allowed_codes: &[i64],
) -> Result<()> {
    let code = result.return_code.unwrap_or(0);
    if !allowed_codes.contains(&code) {
        return Err(Error::RpcError(result.error.clone().unwrap_or_else(|| {
            format!("SporeVault {} returned code {}", function_name, code)
        })));
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
    let slice: [u8; 8] = bytes[start..end].try_into().map_err(|_| {
        Error::ParseError(format!(
            "SporeVault {} payload was malformed",
            function_name
        ))
    })?;
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

fn decode_vault_status(result: &ReadonlyContractResult) -> Result<SporeVaultStatus> {
    ensure_readonly_success(result, "get_vault_status", &[0])?;
    let bytes = decode_return_data(result, "get_vault_status")?;
    if bytes.len() < VAULT_STATUS_SIZE {
        return Err(Error::ParseError(
            "SporeVault get_vault_status payload was shorter than expected".into(),
        ));
    }
    let value = |index: usize| decode_u64(&bytes, index * 8, "get_vault_status");
    Ok(SporeVaultStatus {
        accounting_version: value(0)?,
        paused: value(1)? != 0,
        licn_config_present: value(2)? != 0,
        licn_config_valid: value(3)? != 0,
        native_licn: value(4)? != 0,
        thalllend_config_present: value(5)? != 0,
        thalllend_config_valid: value(6)? != 0,
        strategy_registry_valid: value(7)? != 0,
        idle_assets: value(8)?,
        lending_assets: value(9)?,
        total_assets: value(10)?,
        total_shares: value(11)?,
        protocol_fees: value(12)?,
        real_liquid_custody: value(13)?,
        custody_query_ok: value(14)? != 0,
        liquid_custody_covers_accounting: value(15)? != 0,
        deposit_fee_bps: value(16)?,
        withdrawal_fee_bps: value(17)?,
        deposit_cap: value(18)?,
        risk_tier: value(19)?,
        performance_fee_percent: value(20)?,
        management_fee_bps: value(21)?,
        target_slots_per_year: value(22)?,
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
        if let Some(program_id) = *self.program_id.lock().map_err(|_| {
            Error::ConfigError("SporeVaultClient program cache lock poisoned".into())
        })? {
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
                Error::ConfigError("SporeVaultClient program cache lock poisoned".into())
            })? = Some(program_id);
            return Ok(program_id);
        }

        Err(Error::ConfigError(
            "Unable to resolve the SporeVault program via getSymbolRegistry(\"SPOREVAULT\")".into(),
        ))
    }

    pub async fn get_vault_stats(&self) -> Result<SporeVaultVaultStats> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_vault_stats",
                Vec::new(),
                None,
            )
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

    pub async fn get_vault_status(&self) -> Result<SporeVaultStatus> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_vault_status",
                Vec::new(),
                None,
            )
            .await?;
        decode_vault_status(&result)
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

    pub async fn deposit_mt20(&self, depositor: &Keypair, amount: u64) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(
                depositor,
                &program_id,
                "deposit",
                encode_user_amount_args(&depositor.pubkey(), amount),
                0,
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

    pub async fn rebalance(&self, caller: &Keypair) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(caller, &program_id, "rebalance", Vec::new(), 0)
            .await
    }

    async fn call_admin(
        &self,
        admin: &Keypair,
        function_name: &str,
        args: Vec<u8>,
    ) -> Result<String> {
        let program_id = self.get_program_id().await?;
        self.client
            .call_contract(admin, &program_id, function_name, args, 0)
            .await
    }

    pub async fn pause(&self, admin: &Keypair) -> Result<String> {
        self.call_admin(
            admin,
            "cv_pause",
            build_layout_args(&[0x20], &[admin.pubkey().as_ref().to_vec()]),
        )
        .await
    }

    pub async fn unpause(&self, admin: &Keypair) -> Result<String> {
        self.call_admin(
            admin,
            "cv_unpause",
            build_layout_args(&[0x20], &[admin.pubkey().as_ref().to_vec()]),
        )
        .await
    }

    pub async fn set_deposit_fee(&self, admin: &Keypair, fee_bps: u64) -> Result<String> {
        self.call_admin(
            admin,
            "set_deposit_fee",
            encode_admin_u64_args(&admin.pubkey(), fee_bps),
        )
        .await
    }

    pub async fn set_withdrawal_fee(&self, admin: &Keypair, fee_bps: u64) -> Result<String> {
        self.call_admin(
            admin,
            "set_withdrawal_fee",
            encode_admin_u64_args(&admin.pubkey(), fee_bps),
        )
        .await
    }

    pub async fn set_deposit_cap(&self, admin: &Keypair, cap: u64) -> Result<String> {
        self.call_admin(
            admin,
            "set_deposit_cap",
            encode_admin_u64_args(&admin.pubkey(), cap),
        )
        .await
    }

    pub async fn set_risk_tier(&self, admin: &Keypair, tier: u8) -> Result<String> {
        self.call_admin(
            admin,
            "set_risk_tier",
            encode_admin_u8_args(&admin.pubkey(), tier),
        )
        .await
    }

    pub async fn add_strategy(
        &self,
        admin: &Keypair,
        strategy_type: u8,
        allocation_percent: u64,
    ) -> Result<String> {
        self.call_admin(
            admin,
            "add_strategy",
            encode_admin_strategy_args(&admin.pubkey(), strategy_type, allocation_percent),
        )
        .await
    }

    pub async fn remove_strategy(&self, admin: &Keypair, index: u64) -> Result<String> {
        self.call_admin(
            admin,
            "remove_strategy",
            encode_admin_u64_args(&admin.pubkey(), index),
        )
        .await
    }

    pub async fn update_strategy_allocation(
        &self,
        admin: &Keypair,
        index: u64,
        allocation_percent: u64,
    ) -> Result<String> {
        self.call_admin(
            admin,
            "update_strategy_allocation",
            encode_admin_two_u64_args(&admin.pubkey(), index, allocation_percent),
        )
        .await
    }

    pub async fn withdraw_protocol_fees(&self, admin: &Keypair) -> Result<String> {
        self.call_admin(
            admin,
            "withdraw_protocol_fees",
            build_layout_args(&[0x20], &[admin.pubkey().as_ref().to_vec()]),
        )
        .await
    }

    pub async fn set_protocol_addresses(
        &self,
        admin: &Keypair,
        thalllend: &Pubkey,
        lichenswap: Option<&Pubkey>,
    ) -> Result<String> {
        let zero = Pubkey([0u8; 32]);
        self.call_admin(
            admin,
            "set_protocol_addresses",
            encode_protocol_address_args(&admin.pubkey(), thalllend, lichenswap.unwrap_or(&zero)),
        )
        .await
    }

    pub async fn set_licn_token(&self, admin: &Keypair, token: &Pubkey) -> Result<String> {
        self.call_admin(
            admin,
            "set_licn_token",
            encode_admin_address_args(&admin.pubkey(), token),
        )
        .await
    }

    pub async fn migrate_accounting_v2(
        &self,
        admin: &Keypair,
        expected_idle_assets: u64,
        expected_lending_assets: u64,
    ) -> Result<String> {
        self.call_admin(
            admin,
            "migrate_accounting_v2",
            encode_admin_two_u64_args(
                &admin.pubkey(),
                expected_idle_assets,
                expected_lending_assets,
            ),
        )
        .await
    }

    pub async fn retire_legacy_strategy(
        &self,
        admin: &Keypair,
        index: u64,
        expected_type: u8,
        expected_allocation: u64,
        expected_deployed: u64,
    ) -> Result<String> {
        self.call_admin(
            admin,
            "retire_legacy_strategy",
            encode_legacy_strategy_retirement_args(
                &admin.pubkey(),
                index,
                expected_type,
                expected_allocation,
                expected_deployed,
            ),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readonly_result(return_code: i64, bytes: Vec<u8>) -> ReadonlyContractResult {
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
        assert_eq!(
            u64::from_le_bytes(encoded[35..43].try_into().unwrap()),
            1_000
        );
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
            [
                200u64.to_le_bytes().as_slice(),
                222u64.to_le_bytes().as_slice(),
            ]
            .concat(),
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
            [
                1u64.to_le_bytes().as_slice(),
                60u64.to_le_bytes().as_slice(),
                3_000u64.to_le_bytes().as_slice(),
            ]
            .concat(),
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

    #[test]
    fn vault_status_decoding_matches_contract_layout() {
        let values = [
            2u64, 0, 1, 1, 1, 1, 1, 1, 4_000, 6_000, 10_000, 9_000, 250, 4_250, 1, 1, 10, 30,
            50_000, 1, 10, 200, 78_894_000,
        ];
        let result = readonly_result(
            0,
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        );

        let status = decode_vault_status(&result).unwrap();
        assert_eq!(status.accounting_version, 2);
        assert!(status.native_licn);
        assert!(status.thalllend_config_valid);
        assert_eq!(status.real_liquid_custody, 4_250);
        assert!(status.liquid_custody_covers_accounting);
        assert_eq!(status.target_slots_per_year, 78_894_000);
    }

    #[test]
    fn admin_encodings_match_named_export_layouts() {
        let admin = Pubkey([7u8; 32]);
        let strategy = encode_admin_strategy_args(&admin, 1, 33);
        assert_eq!(&strategy[..4], &[0xAB, 0x20, 0x01, 0x08]);
        assert_eq!(strategy[36], 1);
        assert_eq!(u64::from_le_bytes(strategy[37..45].try_into().unwrap()), 33);

        let migration = encode_admin_two_u64_args(&admin, 4_000, 6_000);
        assert_eq!(&migration[..4], &[0xAB, 0x20, 0x08, 0x08]);
        assert_eq!(
            u64::from_le_bytes(migration[36..44].try_into().unwrap()),
            4_000
        );
        assert_eq!(
            u64::from_le_bytes(migration[44..52].try_into().unwrap()),
            6_000
        );
    }
}
