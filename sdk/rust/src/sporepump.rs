use crate::{Client, Error, Keypair, Pubkey, Result};
use serde::{Deserialize, Serialize};

const PROGRAM_SYMBOL_CANDIDATES: [&str; 2] = ["SPOREPUMP", "sporepump"];
pub const SPOREPUMP_CREATION_FEE: u64 = 10_000_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SporePumpTokenInfo {
    pub supply_sold: u64,
    pub licn_raised: u64,
    pub current_price: u64,
    pub market_cap: u64,
    pub graduated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SporePumpTokenMetadata {
    pub name: String,
    pub symbol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SporePumpPlatformStats {
    pub token_count: u64,
    pub platform_fees: u64,
    pub curve_reserve: u64,
    pub creator_liability: u64,
    pub cumulative_graduation_revenue: u64,
    pub graduated_count: u64,
    pub accounting_version: u64,
    pub migration_expected: u64,
    pub migration_cursor: u64,
    pub migration_locked: bool,
    pub creator_royalty_bps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SporePumpCustodyStatus {
    pub balance: u64,
    pub obligations: u64,
    pub recoverable_surplus: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SporePumpAccountingMigrationToken {
    pub creator: Pubkey,
    pub supply_sold: u64,
    pub licn_raised: u64,
    pub max_supply: u64,
    pub created_slot: u64,
    pub lifecycle_state: u8,
    pub creator_royalty: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SporePumpGraduationStatus {
    pub state: u8,
    pub eligibility_slot: u64,
    pub migration_boundary_slot: u64,
    pub candidate: Option<Pubkey>,
    pub pair_id: u64,
    pub pool_id: u64,
    pub forward_route_id: u64,
    pub reverse_route_id: u64,
    pub position_id: u64,
    pub licn_liquidity: u64,
    pub token_liquidity: u64,
    pub protocol_token_inventory: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SporePumpGraduationInfo {
    pub cumulative_revenue: u64,
    pub dex_core_configured: bool,
    pub dex_amm_configured: bool,
    pub dex_router_configured: bool,
    pub token_template_configured: bool,
    pub governance_configured: bool,
    pub accounting_ready: bool,
    pub tick_size: u64,
    pub lot_size: u64,
    pub minimum_order: u64,
    pub amm_fee_tier: u64,
}

#[derive(Debug, Clone)]
pub struct CreateSporePumpTokenParams {
    pub name: String,
    pub symbol: String,
}

#[derive(Debug, Clone)]
pub struct SporePumpGraduationConfig {
    pub router: Pubkey,
    pub token_template_hash: Pubkey,
    pub tick_size: u64,
    pub lot_size: u64,
    pub minimum_order: u64,
    pub amm_fee_tier: u32,
}

#[derive(Debug, Clone)]
pub struct SporePumpClient {
    client: Client,
    program_id: std::sync::Arc<std::sync::Mutex<Option<Pubkey>>>,
}

fn layout_args(chunks: &[Vec<u8>]) -> Result<Vec<u8>> {
    if chunks.iter().any(|chunk| chunk.len() > u8::MAX as usize) {
        return Err(Error::ConfigError(
            "SporePump ABI stride exceeded one byte".into(),
        ));
    }
    let payload_len = chunks.iter().map(Vec::len).sum::<usize>();
    let mut out = Vec::with_capacity(1 + chunks.len() + payload_len);
    out.push(0xAB);
    out.extend(chunks.iter().map(|chunk| chunk.len() as u8));
    for chunk in chunks {
        out.extend_from_slice(chunk);
    }
    Ok(out)
}

fn u64_args(values: &[u64]) -> Result<Vec<u8>> {
    layout_args(
        &values
            .iter()
            .map(|value| value.to_le_bytes().to_vec())
            .collect::<Vec<_>>(),
    )
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| Error::ParseError("SporePump payload was shorter than expected".into()))?;
    Ok(u64::from_le_bytes(value.try_into().unwrap()))
}

fn ensure_readonly_success(
    result: &crate::client::ReadonlyContractResult,
    function_name: &str,
    require_zero_code: bool,
) -> Result<()> {
    let code = result.return_code.unwrap_or(0);
    if !result.success || (require_zero_code && code != 0) {
        return Err(Error::RpcError(result.error.clone().unwrap_or_else(|| {
            format!("SporePump {function_name} returned code {code}")
        })));
    }
    Ok(())
}

fn decode_return_data(
    result: &crate::client::ReadonlyContractResult,
    function_name: &str,
    require_zero_code: bool,
) -> Result<Vec<u8>> {
    ensure_readonly_success(result, function_name, require_zero_code)?;
    let encoded = result.return_data.as_ref().ok_or_else(|| {
        Error::ParseError(format!(
            "SporePump {function_name} did not return payload data"
        ))
    })?;
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
        .map_err(|error| Error::ParseError(error.to_string()))
}

fn decode_u64_result(
    result: &crate::client::ReadonlyContractResult,
    function_name: &str,
) -> Result<u64> {
    let bytes = decode_return_data(result, function_name, false)?;
    if bytes.len() != 8 {
        return Err(Error::ParseError(format!(
            "SporePump {function_name} returned a non-u64 payload"
        )));
    }
    read_u64(&bytes, 0)
}

fn decode_accounting_migration_token(bytes: &[u8]) -> Result<SporePumpAccountingMigrationToken> {
    if bytes.len() != 73
        || !matches!(bytes[64], 0 | 1 | 3)
        || bytes[0..32].iter().all(|byte| *byte == 0)
    {
        return Err(Error::ParseError(
            "SporePump accounting-migration token payload was malformed".into(),
        ));
    }
    let supply_sold = read_u64(bytes, 32)?;
    let max_supply = read_u64(bytes, 48)?;
    if supply_sold > max_supply {
        return Err(Error::ParseError(
            "SporePump accounting-migration token supply exceeds its cap".into(),
        ));
    }
    Ok(SporePumpAccountingMigrationToken {
        creator: Pubkey(bytes[0..32].try_into().unwrap()),
        supply_sold,
        licn_raised: read_u64(bytes, 40)?,
        max_supply,
        created_slot: read_u64(bytes, 56)?,
        lifecycle_state: bytes[64],
        creator_royalty: read_u64(bytes, 65)?,
    })
}

fn metadata_args(creator: &Pubkey, params: &CreateSporePumpTokenParams) -> Result<Vec<u8>> {
    let name = params.name.trim();
    let symbol = params.symbol.trim().to_ascii_uppercase();
    let name_bytes = name.as_bytes();
    let symbol_bytes = symbol.as_bytes();
    if name_bytes.is_empty()
        || name_bytes.len() > 64
        || name.chars().any(char::is_control)
        || !(2..=12).contains(&symbol_bytes.len())
        || !symbol_bytes[0].is_ascii_alphabetic()
        || !symbol_bytes.iter().all(u8::is_ascii_alphanumeric)
    {
        return Err(Error::ConfigError(
            "invalid SporePump token name or symbol".into(),
        ));
    }
    let name_stride = name_bytes.len().max(32);
    let symbol_stride = symbol_bytes.len().max(32);
    let mut padded_name = vec![0u8; name_stride];
    let mut padded_symbol = vec![0u8; symbol_stride];
    padded_name[..name_bytes.len()].copy_from_slice(name_bytes);
    padded_symbol[..symbol_bytes.len()].copy_from_slice(symbol_bytes);
    layout_args(&[
        creator.as_ref().to_vec(),
        padded_name,
        (name_bytes.len() as u32).to_le_bytes().to_vec(),
        padded_symbol,
        (symbol_bytes.len() as u32).to_le_bytes().to_vec(),
        SPOREPUMP_CREATION_FEE.to_le_bytes().to_vec(),
    ])
}

impl SporePumpClient {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            program_id: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub fn with_program_id(client: Client, program_id: Pubkey) -> Self {
        Self {
            client,
            program_id: std::sync::Arc::new(std::sync::Mutex::new(Some(program_id))),
        }
    }

    pub async fn get_program_id(&self) -> Result<Pubkey> {
        if let Some(program_id) = *self
            .program_id
            .lock()
            .map_err(|_| Error::ConfigError("SporePump program cache lock poisoned".into()))?
        {
            return Ok(program_id);
        }
        for symbol in PROGRAM_SYMBOL_CANDIDATES {
            let Ok(entry) = self.client.get_symbol_registry(symbol).await else {
                continue;
            };
            let Some(program) = entry.get("program").and_then(|value| value.as_str()) else {
                continue;
            };
            let program_id = Pubkey::from_base58(program).map_err(Error::ParseError)?;
            *self.program_id.lock().map_err(|_| {
                Error::ConfigError("SporePump program cache lock poisoned".into())
            })? = Some(program_id);
            return Ok(program_id);
        }
        Err(Error::ConfigError(
            "Unable to resolve SporePump via getSymbolRegistry(\"SPOREPUMP\")".into(),
        ))
    }

    async fn readonly(
        &self,
        function_name: &str,
        args: Vec<u8>,
    ) -> Result<crate::client::ReadonlyContractResult> {
        self.client
            .call_readonly_contract(&self.get_program_id().await?, function_name, args, None)
            .await
    }

    async fn write(
        &self,
        signer: &Keypair,
        function_name: &str,
        args: Vec<u8>,
        value: u64,
    ) -> Result<String> {
        self.client
            .call_contract(
                signer,
                &self.get_program_id().await?,
                function_name,
                args,
                value,
            )
            .await
    }

    pub async fn get_token_info(&self, token_id: u64) -> Result<Option<SporePumpTokenInfo>> {
        let result = self
            .readonly("get_token_info", u64_args(&[token_id])?)
            .await?;
        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }
        let bytes = decode_return_data(&result, "get_token_info", true)?;
        if bytes.len() != 33 || bytes[32] > 1 {
            return Err(Error::ParseError(
                "SporePump token-info payload was malformed".into(),
            ));
        }
        Ok(Some(SporePumpTokenInfo {
            supply_sold: read_u64(&bytes, 0)?,
            licn_raised: read_u64(&bytes, 8)?,
            current_price: read_u64(&bytes, 16)?,
            market_cap: read_u64(&bytes, 24)?,
            graduated: bytes[32] == 1,
        }))
    }

    pub async fn get_token_metadata(
        &self,
        token_id: u64,
    ) -> Result<Option<SporePumpTokenMetadata>> {
        let result = self
            .readonly("get_token_metadata", u64_args(&[token_id])?)
            .await?;
        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }
        let bytes = decode_return_data(&result, "get_token_metadata", true)?;
        if bytes.len() < 4 {
            return Err(Error::ParseError(
                "SporePump metadata payload was malformed".into(),
            ));
        }
        let name_len = u16::from_le_bytes(bytes[0..2].try_into().unwrap()) as usize;
        let symbol_offset = 2usize
            .checked_add(name_len)
            .ok_or_else(|| Error::ParseError("SporePump metadata length overflow".into()))?;
        let symbol_len_bytes = bytes.get(symbol_offset..symbol_offset + 2).ok_or_else(|| {
            Error::ParseError("SporePump metadata name length was invalid".into())
        })?;
        let symbol_len = u16::from_le_bytes(symbol_len_bytes.try_into().unwrap()) as usize;
        if bytes.len() != symbol_offset + 2 + symbol_len {
            return Err(Error::ParseError(
                "SporePump metadata symbol length was invalid".into(),
            ));
        }
        Ok(Some(SporePumpTokenMetadata {
            name: String::from_utf8(bytes[2..symbol_offset].to_vec())
                .map_err(|error| Error::ParseError(error.to_string()))?,
            symbol: String::from_utf8(bytes[symbol_offset + 2..].to_vec())
                .map_err(|error| Error::ParseError(error.to_string()))?,
        }))
    }

    async fn quote(&self, function_name: &str, token_id: u64, amount: u64) -> Result<u64> {
        let result = self
            .readonly(function_name, u64_args(&[token_id, amount])?)
            .await?;
        decode_u64_result(&result, function_name)
    }

    pub async fn get_buy_quote(&self, token_id: u64, licn_amount: u64) -> Result<u64> {
        self.quote("get_buy_quote", token_id, licn_amount).await
    }

    pub async fn get_sell_quote(&self, token_id: u64, token_amount: u64) -> Result<u64> {
        self.quote("get_sell_quote", token_id, token_amount).await
    }

    pub async fn get_token_count(&self) -> Result<u64> {
        let result = self.readonly("get_token_count", Vec::new()).await?;
        decode_u64_result(&result, "get_token_count")
    }

    pub async fn get_creator_royalty_balance(
        &self,
        token_id: u64,
        creator: &Pubkey,
    ) -> Result<u64> {
        let result = self
            .readonly(
                "get_creator_royalty_balance",
                layout_args(&[token_id.to_le_bytes().to_vec(), creator.as_ref().to_vec()])?,
            )
            .await?;
        decode_u64_result(&result, "get_creator_royalty_balance")
    }

    pub async fn get_platform_stats(&self) -> Result<SporePumpPlatformStats> {
        let result = self.readonly("get_platform_stats", Vec::new()).await?;
        let bytes = decode_return_data(&result, "get_platform_stats", true)?;
        if bytes.len() != 88 {
            return Err(Error::ParseError(
                "SporePump platform-stats payload was malformed".into(),
            ));
        }
        let migration_locked = read_u64(&bytes, 72)?;
        let creator_royalty_bps = read_u64(&bytes, 80)?;
        if migration_locked > 1 || creator_royalty_bps > 1_000 {
            return Err(Error::ParseError(
                "SporePump platform-stats control values were malformed".into(),
            ));
        }
        Ok(SporePumpPlatformStats {
            token_count: read_u64(&bytes, 0)?,
            platform_fees: read_u64(&bytes, 8)?,
            curve_reserve: read_u64(&bytes, 16)?,
            creator_liability: read_u64(&bytes, 24)?,
            cumulative_graduation_revenue: read_u64(&bytes, 32)?,
            graduated_count: read_u64(&bytes, 40)?,
            accounting_version: read_u64(&bytes, 48)?,
            migration_expected: read_u64(&bytes, 56)?,
            migration_cursor: read_u64(&bytes, 64)?,
            migration_locked: migration_locked == 1,
            creator_royalty_bps,
        })
    }

    pub async fn get_custody_status(&self) -> Result<SporePumpCustodyStatus> {
        let result = self.readonly("get_custody_status", Vec::new()).await?;
        let bytes = decode_return_data(&result, "get_custody_status", true)?;
        if bytes.len() != 24 {
            return Err(Error::ParseError(
                "SporePump custody payload was malformed".into(),
            ));
        }
        Ok(SporePumpCustodyStatus {
            balance: read_u64(&bytes, 0)?,
            obligations: read_u64(&bytes, 8)?,
            recoverable_surplus: read_u64(&bytes, 16)?,
        })
    }

    pub async fn get_accounting_migration_token(
        &self,
        token_id: u64,
    ) -> Result<Option<SporePumpAccountingMigrationToken>> {
        let result = self
            .readonly("get_accounting_migration_token", u64_args(&[token_id])?)
            .await?;
        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }
        let bytes = decode_return_data(&result, "get_accounting_migration_token", true)?;
        decode_accounting_migration_token(&bytes).map(Some)
    }

    pub async fn get_graduation_status(
        &self,
        token_id: u64,
    ) -> Result<Option<SporePumpGraduationStatus>> {
        let result = self
            .readonly("get_graduation_status", u64_args(&[token_id])?)
            .await?;
        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }
        let bytes = decode_return_data(&result, "get_graduation_status", true)?;
        if bytes.len() != 113 || bytes[0] > 3 {
            return Err(Error::ParseError(
                "SporePump graduation-status payload was malformed".into(),
            ));
        }
        let candidate_bytes: [u8; 32] = bytes[17..49].try_into().unwrap();
        Ok(Some(SporePumpGraduationStatus {
            state: bytes[0],
            eligibility_slot: read_u64(&bytes, 1)?,
            migration_boundary_slot: read_u64(&bytes, 9)?,
            candidate: candidate_bytes
                .iter()
                .any(|byte| *byte != 0)
                .then_some(Pubkey(candidate_bytes)),
            pair_id: read_u64(&bytes, 49)?,
            pool_id: read_u64(&bytes, 57)?,
            forward_route_id: read_u64(&bytes, 65)?,
            reverse_route_id: read_u64(&bytes, 73)?,
            position_id: read_u64(&bytes, 81)?,
            licn_liquidity: read_u64(&bytes, 89)?,
            token_liquidity: read_u64(&bytes, 97)?,
            protocol_token_inventory: read_u64(&bytes, 105)?,
        }))
    }

    pub async fn get_graduation_info(&self) -> Result<SporePumpGraduationInfo> {
        let result = self.readonly("get_graduation_info", Vec::new()).await?;
        let bytes = decode_return_data(&result, "get_graduation_info", true)?;
        if bytes.len() != 46 || bytes[8..14].iter().any(|flag| *flag > 1) {
            return Err(Error::ParseError(
                "SporePump graduation-info payload was malformed".into(),
            ));
        }
        Ok(SporePumpGraduationInfo {
            cumulative_revenue: read_u64(&bytes, 0)?,
            dex_core_configured: bytes[8] == 1,
            dex_amm_configured: bytes[9] == 1,
            dex_router_configured: bytes[10] == 1,
            token_template_configured: bytes[11] == 1,
            governance_configured: bytes[12] == 1,
            accounting_ready: bytes[13] == 1,
            tick_size: read_u64(&bytes, 14)?,
            lot_size: read_u64(&bytes, 22)?,
            minimum_order: read_u64(&bytes, 30)?,
            amm_fee_tier: read_u64(&bytes, 38)?,
        })
    }

    pub async fn create_token(
        &self,
        creator: &Keypair,
        metadata: Option<CreateSporePumpTokenParams>,
    ) -> Result<String> {
        let (function, args) = match metadata {
            Some(metadata) => (
                "create_token_with_metadata",
                metadata_args(&creator.pubkey(), &metadata)?,
            ),
            None => (
                "create_token",
                layout_args(&[
                    creator.pubkey().as_ref().to_vec(),
                    SPOREPUMP_CREATION_FEE.to_le_bytes().to_vec(),
                ])?,
            ),
        };
        self.write(creator, function, args, SPOREPUMP_CREATION_FEE)
            .await
    }

    pub async fn buy(
        &self,
        buyer: &Keypair,
        token_id: u64,
        licn_amount: u64,
        minimum_tokens_out: u64,
    ) -> Result<String> {
        let args = layout_args(&[
            buyer.pubkey().as_ref().to_vec(),
            token_id.to_le_bytes().to_vec(),
            licn_amount.to_le_bytes().to_vec(),
            minimum_tokens_out.to_le_bytes().to_vec(),
        ])?;
        self.write(buyer, "buy_with_min_output", args, licn_amount)
            .await
    }

    pub async fn sell(
        &self,
        seller: &Keypair,
        token_id: u64,
        token_amount: u64,
        minimum_licn_out: u64,
    ) -> Result<String> {
        let args = layout_args(&[
            seller.pubkey().as_ref().to_vec(),
            token_id.to_le_bytes().to_vec(),
            token_amount.to_le_bytes().to_vec(),
            minimum_licn_out.to_le_bytes().to_vec(),
        ])?;
        self.write(seller, "sell_with_min_output", args, 0).await
    }

    pub async fn claim_creator_royalty(
        &self,
        creator: &Keypair,
        token_id: u64,
        amount: u64,
    ) -> Result<String> {
        self.write(
            creator,
            "claim_creator_royalty",
            layout_args(&[
                creator.pubkey().as_ref().to_vec(),
                token_id.to_le_bytes().to_vec(),
                amount.to_le_bytes().to_vec(),
            ])?,
            0,
        )
        .await
    }

    async fn signer_only(&self, signer: &Keypair, function_name: &str) -> Result<String> {
        self.write(
            signer,
            function_name,
            layout_args(&[signer.pubkey().as_ref().to_vec()])?,
            0,
        )
        .await
    }

    async fn signer_u64(
        &self,
        signer: &Keypair,
        function_name: &str,
        value: u64,
    ) -> Result<String> {
        self.write(
            signer,
            function_name,
            layout_args(&[
                signer.pubkey().as_ref().to_vec(),
                value.to_le_bytes().to_vec(),
            ])?,
            0,
        )
        .await
    }

    pub async fn pause(&self, admin: &Keypair) -> Result<String> {
        self.signer_only(admin, "pause").await
    }
    pub async fn unpause(&self, admin: &Keypair) -> Result<String> {
        self.signer_only(admin, "unpause").await
    }
    pub async fn freeze_token(&self, admin: &Keypair, token_id: u64) -> Result<String> {
        self.signer_u64(admin, "freeze_token", token_id).await
    }
    pub async fn unfreeze_token(&self, admin: &Keypair, token_id: u64) -> Result<String> {
        self.signer_u64(admin, "unfreeze_token", token_id).await
    }
    pub async fn set_buy_cooldown(&self, admin: &Keypair, slots: u64) -> Result<String> {
        self.signer_u64(admin, "set_buy_cooldown", slots).await
    }
    pub async fn set_sell_cooldown(&self, admin: &Keypair, slots: u64) -> Result<String> {
        self.signer_u64(admin, "set_sell_cooldown", slots).await
    }
    pub async fn set_max_buy(&self, admin: &Keypair, amount: u64) -> Result<String> {
        self.signer_u64(admin, "set_max_buy", amount).await
    }
    pub async fn set_creator_royalty(&self, admin: &Keypair, bps: u64) -> Result<String> {
        self.signer_u64(admin, "set_creator_royalty", bps).await
    }
    pub async fn withdraw_fees(&self, admin: &Keypair, amount: u64) -> Result<String> {
        self.signer_u64(admin, "withdraw_fees", amount).await
    }
    pub async fn recover_custody_surplus(&self, admin: &Keypair, amount: u64) -> Result<String> {
        self.signer_u64(admin, "recover_custody_surplus", amount)
            .await
    }
    pub async fn begin_accounting_v3_migration(
        &self,
        admin: &Keypair,
        expected_tokens: u64,
    ) -> Result<String> {
        self.signer_u64(admin, "begin_accounting_v3_migration", expected_tokens)
            .await
    }
    pub async fn migrate_accounting_v3_token(
        &self,
        keeper: &Keypair,
        token_id: u64,
    ) -> Result<String> {
        self.write(
            keeper,
            "migrate_accounting_v3_token",
            u64_args(&[token_id])?,
            0,
        )
        .await
    }
    pub async fn complete_accounting_v3_migration(&self, admin: &Keypair) -> Result<String> {
        self.signer_only(admin, "complete_accounting_v3_migration")
            .await
    }
    pub async fn propose_admin(&self, admin: &Keypair, next_admin: &Pubkey) -> Result<String> {
        self.write(
            admin,
            "propose_admin",
            layout_args(&[
                admin.pubkey().as_ref().to_vec(),
                next_admin.as_ref().to_vec(),
            ])?,
            0,
        )
        .await
    }
    pub async fn accept_admin(&self, next_admin: &Keypair) -> Result<String> {
        self.signer_only(next_admin, "accept_admin").await
    }
    pub async fn set_dex_addresses(
        &self,
        admin: &Keypair,
        core: &Pubkey,
        amm: &Pubkey,
    ) -> Result<String> {
        self.write(
            admin,
            "set_dex_addresses",
            layout_args(&[
                admin.pubkey().as_ref().to_vec(),
                core.as_ref().to_vec(),
                amm.as_ref().to_vec(),
            ])?,
            0,
        )
        .await
    }
    pub async fn set_graduation_governance(
        &self,
        admin: &Keypair,
        governance: &Pubkey,
    ) -> Result<String> {
        self.write(
            admin,
            "set_graduation_governance",
            layout_args(&[
                admin.pubkey().as_ref().to_vec(),
                governance.as_ref().to_vec(),
            ])?,
            0,
        )
        .await
    }
    pub async fn set_graduation_config(
        &self,
        governance: &Keypair,
        config: &SporePumpGraduationConfig,
    ) -> Result<String> {
        self.write(
            governance,
            "set_graduation_config",
            layout_args(&[
                governance.pubkey().as_ref().to_vec(),
                config.router.as_ref().to_vec(),
                config.token_template_hash.as_ref().to_vec(),
                config.tick_size.to_le_bytes().to_vec(),
                config.lot_size.to_le_bytes().to_vec(),
                config.minimum_order.to_le_bytes().to_vec(),
                config.amm_fee_tier.to_le_bytes().to_vec(),
            ])?,
            0,
        )
        .await
    }
    pub async fn begin_graduation(
        &self,
        keeper: &Keypair,
        token_id: u64,
        candidate: &Pubkey,
    ) -> Result<String> {
        self.write(
            keeper,
            "begin_migration",
            layout_args(&[
                keeper.pubkey().as_ref().to_vec(),
                token_id.to_le_bytes().to_vec(),
                candidate.as_ref().to_vec(),
            ])?,
            0,
        )
        .await
    }
    pub async fn abort_graduation(&self, keeper: &Keypair, token_id: u64) -> Result<String> {
        self.signer_u64(keeper, "abort_migration", token_id).await
    }
    pub async fn finalize_graduation(&self, keeper: &Keypair, token_id: u64) -> Result<String> {
        self.signer_u64(keeper, "finalize_migration", token_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_encoding_is_normalized_and_abi_framed() {
        let creator = Pubkey([7u8; 32]);
        let args = metadata_args(
            &creator,
            &CreateSporePumpTokenParams {
                name: "  My Token  ".into(),
                symbol: " moss ".into(),
            },
        )
        .unwrap();
        assert_eq!(&args[0..7], &[0xAB, 32, 32, 4, 32, 4, 8]);
        assert_eq!(&args[7..39], creator.as_ref());
        assert_eq!(&args[39..47], b"My Token");
        assert!(args.windows(4).any(|window| window == b"MOSS"));
    }

    #[test]
    fn malformed_metadata_and_oversized_stride_are_rejected() {
        let creator = Pubkey([7u8; 32]);
        assert!(metadata_args(
            &creator,
            &CreateSporePumpTokenParams {
                name: "ok".into(),
                symbol: "1bad".into(),
            },
        )
        .is_err());
        assert!(layout_args(&[vec![0u8; 256]]).is_err());
    }

    #[test]
    fn exact_status_layout_decodes_every_graduation_identifier() {
        let mut bytes = vec![0u8; 113];
        bytes[0] = 3;
        bytes[17..49].copy_from_slice(&[9u8; 32]);
        for (offset, value) in [
            (1, 1u64),
            (9, 2),
            (49, 3),
            (57, 4),
            (65, 5),
            (73, 6),
            (81, 7),
            (89, 8),
            (97, 9),
            (105, 10),
        ] {
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        assert_eq!(read_u64(&bytes, 73).unwrap(), 6);
        assert_eq!(read_u64(&bytes, 105).unwrap(), 10);
        assert_eq!(Pubkey(bytes[17..49].try_into().unwrap()), Pubkey([9u8; 32]));
    }

    #[test]
    fn exact_accounting_migration_token_layout_is_validated() {
        let mut bytes = vec![0u8; 73];
        bytes[0..32].copy_from_slice(&[7u8; 32]);
        for (offset, value) in [(32, 12u64), (40, 13), (48, 14), (56, 15), (65, 16)] {
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        bytes[64] = 1;
        let token = decode_accounting_migration_token(&bytes).unwrap();
        assert_eq!(token.creator, Pubkey([7u8; 32]));
        assert_eq!(token.creator_royalty, 16);
        bytes[48..56].copy_from_slice(&11u64.to_le_bytes());
        assert!(decode_accounting_migration_token(&bytes).is_err());
    }
}
