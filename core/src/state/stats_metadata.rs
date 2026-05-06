use super::*;

impl StateStore {
    /// Store treasury public key
    pub fn set_treasury_pubkey(&self, pubkey: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"treasury_pubkey", pubkey.0)
            .map_err(|e| format!("Failed to store treasury pubkey: {}", e))
    }

    /// Store genesis public key
    pub fn set_genesis_pubkey(&self, pubkey: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"genesis_pubkey", pubkey.0)
            .map_err(|e| format!("Failed to store genesis pubkey: {}", e))
    }

    /// Store all genesis distribution accounts (role → pubkey mapping)
    /// Serialized as JSON array: [{"role":"...","pubkey":"...","amount_licn":N,"percentage":N}]
    pub fn set_genesis_accounts(
        &self,
        accounts: &[(String, Pubkey, u64, u8)],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let entries: Vec<serde_json::Value> = accounts
            .iter()
            .map(|(role, pubkey, amount_licn, percentage)| {
                serde_json::json!({
                    "role": role,
                    "pubkey": pubkey.to_base58(),
                    "amount_licn": amount_licn,
                    "percentage": percentage,
                })
            })
            .collect();

        let json = serde_json::to_vec(&entries)
            .map_err(|e| format!("Failed to serialize genesis accounts: {}", e))?;

        self.db
            .put_cf(&cf, b"genesis_accounts", json)
            .map_err(|e| format!("Failed to store genesis accounts: {}", e))
    }

    /// Load all genesis distribution accounts
    pub fn get_genesis_accounts(&self) -> Result<Vec<(String, Pubkey, u64, u8)>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        match self.db.get_cf(&cf, b"genesis_accounts") {
            Ok(Some(data)) => {
                let entries: Vec<serde_json::Value> = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize genesis accounts: {}", e))?;
                let mut result = Vec::new();
                for entry in entries {
                    let role = entry["role"].as_str().unwrap_or("").to_string();
                    let pubkey_str = entry["pubkey"].as_str().unwrap_or("");
                    let pubkey = Pubkey::from_base58(pubkey_str)
                        .map_err(|e| format!("Invalid pubkey '{}': {}", pubkey_str, e))?;
                    let amount_licn = entry["amount_licn"].as_u64().unwrap_or(0);
                    let percentage = entry["percentage"].as_u64().unwrap_or(0) as u8;
                    result.push((role, pubkey, amount_licn, percentage));
                }
                Ok(result)
            }
            Ok(None) => Ok(Vec::new()),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Look up a specific genesis wallet pubkey by role name.
    ///
    /// Valid roles: "validator_rewards", "community_treasury", "builder_grants",
    /// "founding_symbionts", "ecosystem_partnerships", "reserve_pool".
    pub fn get_wallet_pubkey(&self, role: &str) -> Result<Option<Pubkey>, String> {
        let accounts = self.get_genesis_accounts()?;
        Ok(accounts
            .into_iter()
            .find(|(r, _, _, _)| r == role)
            .map(|(_, pk, _, _)| pk))
    }

    /// Get community treasury wallet pubkey.
    pub fn get_community_treasury_pubkey(&self) -> Result<Option<Pubkey>, String> {
        self.get_wallet_pubkey("community_treasury")
    }

    /// Get builder grants wallet pubkey.
    pub fn get_builder_grants_pubkey(&self) -> Result<Option<Pubkey>, String> {
        self.get_wallet_pubkey("builder_grants")
    }

    /// Get founding symbionts wallet pubkey.
    pub fn get_founding_symbionts_pubkey(&self) -> Result<Option<Pubkey>, String> {
        self.get_wallet_pubkey("founding_symbionts")
    }

    /// Get ecosystem partnerships wallet pubkey.
    pub fn get_ecosystem_partnerships_pubkey(&self) -> Result<Option<Pubkey>, String> {
        self.get_wallet_pubkey("ecosystem_partnerships")
    }

    /// Get reserve pool wallet pubkey.
    pub fn get_reserve_pool_pubkey(&self) -> Result<Option<Pubkey>, String> {
        self.get_wallet_pubkey("reserve_pool")
    }

    /// Store founding symbionts vesting parameters (absolute Unix timestamps + total amount).
    ///
    /// `cliff_end`: Unix timestamp when the 6-month cliff ends (first unlock).
    /// `vest_end`: Unix timestamp when vesting is fully complete (month 24).
    /// `total_amount_spores`: Total founding symbionts allocation in spores.
    pub fn set_founding_vesting_params(
        &self,
        cliff_end: u64,
        vest_end: u64,
        total_amount_spores: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(&cf, b"founding_vest_cliff_end", cliff_end.to_le_bytes());
        batch.put_cf(&cf, b"founding_vest_end", vest_end.to_le_bytes());
        batch.put_cf(
            &cf,
            b"founding_vest_total_amount",
            total_amount_spores.to_le_bytes(),
        );

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to store founding vesting params: {}", e))
    }

    /// Load founding symbionts vesting parameters.
    /// Returns `Ok(Some((cliff_end, vest_end, total_amount_spores)))` if set.
    pub fn get_founding_vesting_params(&self) -> Result<Option<(u64, u64, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let cliff_end = match self.db.get_cf(&cf, b"founding_vest_cliff_end") {
            Ok(Some(data)) if data.len() == 8 => u64::from_le_bytes(data[..8].try_into().unwrap()),
            _ => return Ok(None),
        };
        let vest_end = match self.db.get_cf(&cf, b"founding_vest_end") {
            Ok(Some(data)) if data.len() == 8 => u64::from_le_bytes(data[..8].try_into().unwrap()),
            _ => return Ok(None),
        };
        let total_amount = match self.db.get_cf(&cf, b"founding_vest_total_amount") {
            Ok(Some(data)) if data.len() == 8 => u64::from_le_bytes(data[..8].try_into().unwrap()),
            _ => return Ok(None),
        };

        Ok(Some((cliff_end, vest_end, total_amount)))
    }

    // ========================================================================
    // GOVERNED WALLET MULTI-SIG SYSTEM
    // ========================================================================

    /// Store a governed wallet configuration (multi-sig config for distribution wallets).
    /// Key: `governed_wallet:<base58_pubkey>` in CF_STATS.
    pub fn set_governed_wallet_config(
        &self,
        wallet_pubkey: &Pubkey,
        config: &crate::multisig::GovernedWalletConfig,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governed_wallet:{}", wallet_pubkey.to_base58());
        let data = serde_json::to_vec(config)
            .map_err(|e| format!("Failed to serialize governed wallet config: {}", e))?;
        self.db
            .put_cf(&cf, key.as_bytes(), data)
            .map_err(|e| format!("Failed to store governed wallet config: {}", e))
    }

    /// Load governed wallet configuration. Returns None if wallet is not governed.
    pub fn get_governed_wallet_config(
        &self,
        wallet_pubkey: &Pubkey,
    ) -> Result<Option<crate::multisig::GovernedWalletConfig>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governed_wallet:{}", wallet_pubkey.to_base58());
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                let config: crate::multisig::GovernedWalletConfig =
                    serde_json::from_slice(&data)
                        .map_err(|e| format!("Failed to deserialize governed config: {}", e))?;
                Ok(Some(config))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("DB error loading governed wallet config: {}", e)),
        }
    }

    /// Get the next governed proposal ID (auto-incrementing counter).
    pub fn next_governed_proposal_id(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let current = match self.db.get_cf(&cf, b"governed_proposal_counter") {
            Ok(Some(data)) if data.len() == 8 => u64::from_le_bytes(data[..8].try_into().unwrap()),
            _ => 0,
        };
        let next = current + 1;
        self.db
            .put_cf(&cf, b"governed_proposal_counter", next.to_le_bytes())
            .map_err(|e| format!("Failed to update proposal counter: {}", e))?;
        Ok(next)
    }

    /// Get the next governance proposal ID (auto-incrementing counter).
    pub fn next_governance_proposal_id(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let current = match self.db.get_cf(&cf, b"governance_proposal_counter") {
            Ok(Some(data)) if data.len() == 8 => u64::from_le_bytes(data[..8].try_into().unwrap()),
            _ => 0,
        };
        let next = current + 1;
        self.db
            .put_cf(&cf, b"governance_proposal_counter", next.to_le_bytes())
            .map_err(|e| format!("Failed to update governance proposal counter: {}", e))?;
        Ok(next)
    }

    /// Store a governed transfer proposal.
    pub fn set_governed_proposal(
        &self,
        proposal: &crate::multisig::GovernedProposal,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governed_proposal:{}", proposal.id);
        let data = serde_json::to_vec(proposal)
            .map_err(|e| format!("Failed to serialize governed proposal: {}", e))?;
        self.db
            .put_cf(&cf, key.as_bytes(), data)
            .map_err(|e| format!("Failed to store governed proposal: {}", e))
    }

    /// Load a governed transfer proposal by ID.
    pub fn get_governed_proposal(
        &self,
        id: u64,
    ) -> Result<Option<crate::multisig::GovernedProposal>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governed_proposal:{}", id);
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                let proposal: crate::multisig::GovernedProposal = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize proposal: {}", e))?;
                Ok(Some(proposal))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("DB error loading governed proposal: {}", e)),
        }
    }

    /// Load the executed governed-transfer volume for a wallet on a UTC day.
    pub fn get_governed_transfer_day_volume(
        &self,
        wallet_pubkey: &Pubkey,
        day_start: u64,
    ) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!(
            "governed_transfer_volume:{}:{}",
            wallet_pubkey.to_base58(),
            day_start
        );
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) if data.len() == 8 => {
                Ok(u64::from_le_bytes(data[..8].try_into().unwrap()))
            }
            Ok(Some(_)) | Ok(None) => Ok(0),
            Err(e) => Err(format!("DB error loading governed transfer volume: {}", e)),
        }
    }

    /// Store the executed governed-transfer volume for a wallet on a UTC day.
    pub fn set_governed_transfer_day_volume(
        &self,
        wallet_pubkey: &Pubkey,
        day_start: u64,
        volume: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!(
            "governed_transfer_volume:{}:{}",
            wallet_pubkey.to_base58(),
            day_start
        );
        self.db
            .put_cf(&cf, key.as_bytes(), volume.to_le_bytes())
            .map_err(|e| format!("Failed to store governed transfer volume: {}", e))
    }

    /// Store a governance proposal.
    pub fn set_governance_proposal(
        &self,
        proposal: &crate::governance::GovernanceProposal,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governance_proposal:{}", proposal.id);
        let data = serde_json::to_vec(proposal)
            .map_err(|e| format!("Failed to serialize governance proposal: {}", e))?;
        self.db
            .put_cf(&cf, key.as_bytes(), data)
            .map_err(|e| format!("Failed to store governance proposal: {}", e))
    }

    /// Load a governance proposal by ID.
    pub fn get_governance_proposal(
        &self,
        id: u64,
    ) -> Result<Option<crate::governance::GovernanceProposal>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governance_proposal:{}", id);
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                let proposal: crate::governance::GovernanceProposal = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize governance proposal: {}", e))?;
                Ok(Some(proposal))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("DB error loading governance proposal: {}", e)),
        }
    }

    /// Store rent parameters
    /// PHASE1-FIX S-6: Atomic WriteBatch for both rent parameters.
    pub fn set_rent_params(
        &self,
        rate_spores_per_kb_month: u64,
        free_kb: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(
            &cf,
            b"rent_rate_spores_per_kb_month",
            rate_spores_per_kb_month.to_le_bytes(),
        );
        batch.put_cf(&cf, b"rent_free_kb", free_kb.to_le_bytes());

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to store rent params: {}", e))
    }

    /// Load rent parameters (defaults if missing)
    pub fn get_rent_params(&self) -> Result<(u64, u64), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let rate = match self.db.get_cf(&cf, b"rent_rate_spores_per_kb_month") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid rent rate data".to_string())?;
                u64::from_le_bytes(bytes)
            }
            Ok(None) => 1_000,
            Err(e) => return Err(format!("Database error: {}", e)),
        };

        let free_kb = match self.db.get_cf(&cf, b"rent_free_kb") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid rent free tier data".to_string())?;
                u64::from_le_bytes(bytes)
            }
            Ok(None) => 1,
            Err(e) => return Err(format!("Database error: {}", e)),
        };

        Ok((rate, free_kb))
    }

    /// Store fee configuration
    pub fn set_fee_config(
        &self,
        base_fee: u64,
        contract_deploy_fee: u64,
        contract_upgrade_fee: u64,
        nft_mint_fee: u64,
        nft_collection_fee: u64,
    ) -> Result<(), String> {
        let config = crate::FeeConfig {
            base_fee,
            contract_deploy_fee,
            contract_upgrade_fee,
            nft_mint_fee,
            nft_collection_fee,
            ..crate::FeeConfig::default_from_constants()
        };
        self.set_fee_config_full(&config)
    }

    /// Store complete fee configuration including distribution percentages
    /// PHASE1-FIX S-5: Single atomic WriteBatch for all 9 fee config keys.
    pub fn set_fee_config_full(&self, config: &crate::FeeConfig) -> Result<(), String> {
        config.validate_distribution()?;

        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(&cf, b"fee_base_spores", config.base_fee.to_le_bytes());
        batch.put_cf(
            &cf,
            b"fee_contract_deploy_spores",
            config.contract_deploy_fee.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_contract_upgrade_spores",
            config.contract_upgrade_fee.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_nft_mint_spores",
            config.nft_mint_fee.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_nft_collection_spores",
            config.nft_collection_fee.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_burn_percent",
            config.fee_burn_percent.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_producer_percent",
            config.fee_producer_percent.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_voters_percent",
            config.fee_voters_percent.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_treasury_percent",
            config.fee_treasury_percent.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_community_percent",
            config.fee_community_percent.to_le_bytes(),
        );

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to store fee config: {}", e))
    }

    // ── Fee-exempt contract set ────────────────────────────────────────
    // Protocol-level contracts (DEX, AMM, router, etc.) whose Call
    // transactions are exempt from the base transaction fee.
    // Written at genesis; modifiable only via governance.
    // Storage format: concatenated 32-byte pubkeys under CF_STATS key
    // "fee_exempt_contracts".

    /// Store the set of fee-exempt contract addresses.
    pub fn set_fee_exempt_contracts(&self, contracts: &[Pubkey]) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut data = Vec::with_capacity(contracts.len() * 32);
        for pk in contracts {
            data.extend_from_slice(&pk.0);
        }
        self.db
            .put_cf(&cf, b"fee_exempt_contracts", &data)
            .map_err(|e| format!("Failed to store fee exempt contracts: {}", e))
    }

    /// Load the set of fee-exempt contract addresses (empty if unset).
    pub fn get_fee_exempt_contracts(&self) -> Vec<Pubkey> {
        let cf = match self.db.cf_handle(CF_STATS) {
            Some(cf) => cf,
            None => return Vec::new(),
        };
        match self.db.get_cf(&cf, b"fee_exempt_contracts") {
            Ok(Some(data)) => {
                let mut result = Vec::with_capacity(data.len() / 32);
                for chunk in data.chunks_exact(32) {
                    let mut bytes = [0u8; 32];
                    bytes.copy_from_slice(chunk);
                    result.push(Pubkey(bytes));
                }
                result
            }
            _ => Vec::new(),
        }
    }

    /// Load fee configuration (defaults if missing)
    pub fn get_fee_config(&self) -> Result<crate::FeeConfig, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let read_u64 = |key: &[u8]| -> Result<Option<u64>, String> {
            match self.db.get_cf(&cf, key) {
                Ok(Some(data)) => {
                    let bytes: [u8; 8] = data
                        .as_slice()
                        .try_into()
                        .map_err(|_| "Invalid fee config data".to_string())?;
                    Ok(Some(u64::from_le_bytes(bytes)))
                }
                Ok(None) => Ok(None),
                Err(e) => Err(format!("Database error: {}", e)),
            }
        };

        let defaults = crate::FeeConfig::default_from_constants();

        Ok(crate::FeeConfig {
            base_fee: read_u64(b"fee_base_spores")?.unwrap_or(defaults.base_fee),
            contract_deploy_fee: read_u64(b"fee_contract_deploy_spores")?
                .unwrap_or(defaults.contract_deploy_fee),
            contract_upgrade_fee: read_u64(b"fee_contract_upgrade_spores")?
                .unwrap_or(defaults.contract_upgrade_fee),
            nft_mint_fee: read_u64(b"fee_nft_mint_spores")?.unwrap_or(defaults.nft_mint_fee),
            nft_collection_fee: read_u64(b"fee_nft_collection_spores")?
                .unwrap_or(defaults.nft_collection_fee),
            fee_burn_percent: read_u64(b"fee_burn_percent")?.unwrap_or(defaults.fee_burn_percent),
            fee_producer_percent: read_u64(b"fee_producer_percent")?
                .unwrap_or(defaults.fee_producer_percent),
            fee_voters_percent: read_u64(b"fee_voters_percent")?
                .unwrap_or(defaults.fee_voters_percent),
            fee_treasury_percent: read_u64(b"fee_treasury_percent")?
                .unwrap_or(defaults.fee_treasury_percent),
            fee_community_percent: read_u64(b"fee_community_percent")?
                .unwrap_or(defaults.fee_community_percent),
            fee_exempt_contracts: self.get_fee_exempt_contracts(),
        })
    }
    /// Store slot_duration_ms in CF_STATS at genesis boot.
    pub fn set_slot_duration_ms(&self, ms: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf, b"slot_duration_ms", ms.to_le_bytes())
            .map_err(|e| format!("Failed to store slot_duration_ms: {}", e))
    }

    /// Read slot_duration_ms from CF_STATS (defaults to 400 if not set).
    pub fn get_slot_duration_ms(&self) -> u64 {
        let cf = match self.db.cf_handle(CF_STATS) {
            Some(cf) => cf,
            None => return 400,
        };
        match self.db.get_cf(&cf, b"slot_duration_ms") {
            Ok(Some(data)) if data.len() == 8 => {
                let bytes: [u8; 8] = data.as_slice().try_into().unwrap_or([0; 8]);
                u64::from_le_bytes(bytes)
            }
            _ => 400,
        }
    }

    // ── Governance parameter changes (Task 2.11) ──

    /// Store the governance authority pubkey (the account authorized to submit
    /// GovernanceParamChange instructions — typically the LichenDAO contract or
    /// a designated multisig).
    pub fn set_governance_authority(&self, authority: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf, b"governance_authority", authority.0)
            .map_err(|e| format!("Failed to store governance authority: {}", e))
    }

    /// Load the governance authority pubkey. Returns None if not set.
    pub fn get_governance_authority(&self) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        match self.db.get_cf(&cf, b"governance_authority") {
            Ok(Some(data)) if data.len() == 32 => {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(bytes)))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(format!("Failed to load governance authority: {}", e)),
        }
    }

    /// Store the treasury executor authority pubkey used for privileged
    /// treasury-transfer governance approvals.
    pub fn set_treasury_executor_authority(&self, authority: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf, b"treasury_executor_authority", authority.0)
            .map_err(|e| format!("Failed to store treasury executor authority: {}", e))
    }

    /// Load the treasury executor authority pubkey. Returns None if not set.
    pub fn get_treasury_executor_authority(&self) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        match self.db.get_cf(&cf, b"treasury_executor_authority") {
            Ok(Some(data)) if data.len() == 32 => {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(bytes)))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(format!("Failed to load treasury executor authority: {}", e)),
        }
    }

    /// Store the incident guardian authority pubkey used for allowlisted fast
    /// risk-reduction proposals.
    pub fn set_incident_guardian_authority(&self, authority: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf, b"incident_guardian_authority", authority.0)
            .map_err(|e| format!("Failed to store incident guardian authority: {}", e))
    }

    /// Load the incident guardian authority pubkey. Returns None if not set.
    pub fn get_incident_guardian_authority(&self) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        match self.db.get_cf(&cf, b"incident_guardian_authority") {
            Ok(Some(data)) if data.len() == 32 => {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(bytes)))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(format!("Failed to load incident guardian authority: {}", e)),
        }
    }

    /// Store the bridge committee admin authority pubkey used for privileged
    /// bridge control-plane governance proposals.
    pub fn set_bridge_committee_admin_authority(&self, authority: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf, b"bridge_committee_admin_authority", authority.0)
            .map_err(|e| format!("Failed to store bridge committee admin authority: {}", e))
    }

    /// Load the bridge committee admin authority pubkey. Returns None if not set.
    pub fn get_bridge_committee_admin_authority(&self) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        match self.db.get_cf(&cf, b"bridge_committee_admin_authority") {
            Ok(Some(data)) if data.len() == 32 => {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(bytes)))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(format!(
                "Failed to load bridge committee admin authority: {}",
                e
            )),
        }
    }

    /// Store the oracle committee admin authority pubkey used for privileged
    /// oracle control-plane governance proposals.
    pub fn set_oracle_committee_admin_authority(&self, authority: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf, b"oracle_committee_admin_authority", authority.0)
            .map_err(|e| format!("Failed to store oracle committee admin authority: {}", e))
    }

    /// Load the oracle committee admin authority pubkey. Returns None if not set.
    pub fn get_oracle_committee_admin_authority(&self) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        match self.db.get_cf(&cf, b"oracle_committee_admin_authority") {
            Ok(Some(data)) if data.len() == 32 => {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(bytes)))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(format!(
                "Failed to load oracle committee admin authority: {}",
                e
            )),
        }
    }

    /// Store the upgrade proposer authority pubkey used for privileged
    /// upgrade proposal, timelock, and execution governance approvals.
    pub fn set_upgrade_proposer_authority(&self, authority: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf, b"upgrade_proposer_authority", authority.0)
            .map_err(|e| format!("Failed to store upgrade proposer authority: {}", e))
    }

    /// Load the upgrade proposer authority pubkey. Returns None if not set.
    pub fn get_upgrade_proposer_authority(&self) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        match self.db.get_cf(&cf, b"upgrade_proposer_authority") {
            Ok(Some(data)) if data.len() == 32 => {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(bytes)))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(format!("Failed to load upgrade proposer authority: {}", e)),
        }
    }

    /// Store the upgrade veto guardian authority pubkey used for privileged
    /// upgrade veto governance approvals.
    pub fn set_upgrade_veto_guardian_authority(&self, authority: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf, b"upgrade_veto_guardian_authority", authority.0)
            .map_err(|e| format!("Failed to store upgrade veto guardian authority: {}", e))
    }

    /// Load the upgrade veto guardian authority pubkey. Returns None if not set.
    pub fn get_upgrade_veto_guardian_authority(&self) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        match self.db.get_cf(&cf, b"upgrade_veto_guardian_authority") {
            Ok(Some(data)) if data.len() == 32 => {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(bytes)))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(format!(
                "Failed to load upgrade veto guardian authority: {}",
                e
            )),
        }
    }

    /// Queue a governance parameter change to take effect at the next epoch
    /// boundary.  Each param_id can have at most one pending value; a newer
    /// submission overwrites any previous pending value for the same param.
    pub fn queue_governance_param_change(&self, param_id: u8, value: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("pending_gov_{}", param_id);
        self.db
            .put_cf(&cf, key.as_bytes(), value.to_le_bytes())
            .map_err(|e| format!("Failed to queue governance param change: {}", e))
    }

    pub fn next_contract_deploy_nonce(&self, deployer: &Pubkey) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("contract_deploy_nonce:{}", deployer.to_base58());
        let current = match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) if data.len() == 8 => {
                u64::from_le_bytes(data.as_slice().try_into().unwrap_or([0; 8]))
            }
            Ok(_) => 0,
            Err(e) => return Err(format!("Database error loading deploy nonce: {}", e)),
        };
        let next = current
            .checked_add(1)
            .ok_or_else(|| "Contract deploy nonce overflow".to_string())?;
        self.db
            .put_cf(&cf, key.as_bytes(), next.to_le_bytes())
            .map_err(|e| format!("Failed to store deploy nonce: {}", e))?;
        Ok(current)
    }

    /// Retrieve all pending governance parameter changes.
    /// Returns a list of (param_id, value) tuples.
    pub fn get_pending_governance_changes(&self) -> Result<Vec<(u8, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let mut changes = Vec::new();
        // Governance param IDs 0–7 are defined; iterate them.
        for param_id in 0..=7u8 {
            let key = format!("pending_gov_{}", param_id);
            if let Ok(Some(data)) = self.db.get_cf(&cf, key.as_bytes()) {
                if data.len() == 8 {
                    let bytes: [u8; 8] = data.as_slice().try_into().unwrap_or([0; 8]);
                    changes.push((param_id, u64::from_le_bytes(bytes)));
                }
            }
        }
        Ok(changes)
    }

    /// Clear all pending governance parameter changes (called after applying
    /// them at an epoch boundary).
    pub fn clear_pending_governance_changes(&self) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let mut batch = rocksdb::WriteBatch::default();
        for param_id in 0..=7u8 {
            let key = format!("pending_gov_{}", param_id);
            batch.delete_cf(&cf, key.as_bytes());
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to clear pending governance changes: {}", e))
    }

    /// Apply all pending governance parameter changes, updating the fee config
    /// and consensus params in state. Called by the validator at epoch boundaries.
    /// Returns the number of parameters changed.
    pub fn apply_pending_governance_changes(&self) -> Result<usize, String> {
        let changes = self.get_pending_governance_changes()?;
        if changes.is_empty() {
            return Ok(0);
        }

        let mut fee_config = self.get_fee_config()?;
        let mut fee_changed = false;
        let mut count = 0;

        for (param_id, value) in &changes {
            if fee_config.apply_governance_param(*param_id, *value) {
                fee_changed = true;
            } else {
                match *param_id {
                    crate::processor::GOV_PARAM_MIN_VALIDATOR_STAKE => {
                        self.set_min_validator_stake(*value)?;
                    }
                    crate::processor::GOV_PARAM_EPOCH_SLOTS => {
                        self.set_epoch_slots(*value)?;
                    }
                    _ => {}
                }
            }
            count += 1;
        }

        if fee_changed {
            self.set_fee_config_full(&fee_config)?;
        }

        self.clear_pending_governance_changes()?;

        Ok(count)
    }

    /// Store min_validator_stake in CF_STATS (governance-mutable).
    pub fn set_min_validator_stake(&self, stake: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf, b"min_validator_stake", stake.to_le_bytes())
            .map_err(|e| format!("Failed to store min_validator_stake: {}", e))
    }

    /// Load min_validator_stake from CF_STATS.
    /// Returns None if not explicitly set (caller should fall back to genesis default).
    pub fn get_min_validator_stake(&self) -> Result<Option<u64>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        match self.db.get_cf(&cf, b"min_validator_stake") {
            Ok(Some(data)) if data.len() == 8 => {
                let bytes: [u8; 8] = data.as_slice().try_into().unwrap_or([0; 8]);
                Ok(Some(u64::from_le_bytes(bytes)))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(format!("Failed to load min_validator_stake: {}", e)),
        }
    }

    /// Store epoch_slots in CF_STATS (governance-mutable).
    pub fn set_epoch_slots(&self, slots: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf, b"epoch_slots", slots.to_le_bytes())
            .map_err(|e| format!("Failed to store epoch_slots: {}", e))
    }

    /// Load epoch_slots from CF_STATS.
    /// Returns None if not explicitly set (caller should fall back to SLOTS_PER_EPOCH).
    pub fn get_epoch_slots(&self) -> Result<Option<u64>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        match self.db.get_cf(&cf, b"epoch_slots") {
            Ok(Some(data)) if data.len() == 8 => {
                let bytes: [u8; 8] = data.as_slice().try_into().unwrap_or([0; 8]);
                Ok(Some(u64::from_le_bytes(bytes)))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(format!("Failed to load epoch_slots: {}", e)),
        }
    }
    /// Generic metadata store/retrieve for consensus markers (e.g. slashing
    /// idempotency keys).  Uses CF_STATS to avoid adding a new column family.
    pub fn put_metadata(&self, key: &str, value: &[u8]) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf, key.as_bytes(), value)
            .map_err(|e| format!("put_metadata({}): {}", key, e))
    }

    /// Retrieve a generic metadata value.  Returns Ok(None) if the key
    /// does not exist.
    pub fn get_metadata(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .get_cf(&cf, key.as_bytes())
            .map_err(|e| format!("get_metadata({}): {}", key, e))
    }

    /// AUDIT-FIX M7: Persist slashing tracker to RocksDB for restart-proof evidence.
    pub fn put_slashing_tracker(
        &self,
        tracker: &crate::consensus::SlashingTracker,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let data = bincode::serialize(tracker)
            .map_err(|e| format!("Failed to serialize slashing tracker: {}", e))?;
        self.db
            .put_cf(&cf, b"slashing_tracker", &data)
            .map_err(|e| format!("Failed to persist slashing tracker: {}", e))
    }

    /// AUDIT-FIX M7: Load slashing tracker from RocksDB.
    /// Returns default empty tracker if not found or on deserialization error.
    pub fn get_slashing_tracker(&self) -> crate::consensus::SlashingTracker {
        let cf = match self.db.cf_handle(CF_STATS) {
            Some(cf) => cf,
            None => return crate::consensus::SlashingTracker::new(),
        };
        match self.db.get_cf(&cf, b"slashing_tracker") {
            Ok(Some(data)) => bincode::deserialize(&data).unwrap_or_else(|e| {
                tracing::warn!(
                    "Failed to deserialize slashing tracker, starting fresh: {}",
                    e
                );
                crate::consensus::SlashingTracker::new()
            }),
            _ => crate::consensus::SlashingTracker::new(),
        }
    }

    /// Load treasury public key
    pub fn get_treasury_pubkey(&self) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        match self.db.get_cf(&cf, b"treasury_pubkey") {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err("Invalid treasury pubkey length".to_string());
                }
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(bytes)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Acquire the fee-account lock to serialize concurrent payer/treasury
    /// read-modify-write operations during parallel fee charging.
    /// Returns a MutexGuard that must be held through the final RocksDB write.
    pub fn lock_treasury(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        self.treasury_lock
            .lock()
            .map_err(|e| format!("treasury_lock poisoned: {}", e))
    }

    /// Load genesis public key
    pub fn get_genesis_pubkey(&self) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        match self.db.get_cf(&cf, b"genesis_pubkey") {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err("Invalid genesis pubkey length".to_string());
                }
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(bytes)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Check if fee distribution already applied for a slot
    pub fn get_fee_distribution_hash(&self, slot: u64) -> Result<Option<Hash>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("fee_dist:{}", slot);
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err("Invalid fee distribution hash length".to_string());
                }
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Hash(bytes)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Mark fee distribution applied for a slot
    pub fn set_fee_distribution_hash(&self, slot: u64, hash: &Hash) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("fee_dist:{}", slot);
        self.db
            .put_cf(&cf, key.as_bytes(), hash.0)
            .map_err(|e| format!("Failed to store fee distribution hash: {}", e))
    }

    /// Check if reward distribution already applied for a slot
    pub fn get_reward_distribution_hash(&self, slot: u64) -> Result<Option<Hash>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("reward_dist:{}", slot);
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err("Invalid reward distribution hash length".to_string());
                }
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Hash(bytes)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Mark reward distribution applied for a slot
    pub fn set_reward_distribution_hash(&self, slot: u64, hash: &Hash) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("reward_dist:{}", slot);
        self.db
            .put_cf(&cf, key.as_bytes(), hash.0)
            .map_err(|e| format!("Failed to store reward distribution hash: {}", e))
    }

    /// Clear reward distribution record for a slot (used by fork choice).
    pub fn clear_reward_distribution_hash(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("reward_dist:{}", slot);
        self.db
            .delete_cf(&cf, key.as_bytes())
            .map_err(|e| format!("Failed to clear reward distribution hash: {}", e))
    }

    /// Clear fee distribution record for a slot (used by fork choice).
    pub fn clear_fee_distribution_hash(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("fee_dist:{}", slot);
        self.db
            .delete_cf(&cf, key.as_bytes())
            .map_err(|e| format!("Failed to clear fee distribution hash: {}", e))
    }

    // ─── Stats Pruning (Bounded Retention) ──────────────────────────────────

    /// Prune per-slot stats keys older than `retain_slots` behind `current_slot`.
    /// Removes: fee_dist:*, reward_dist:*, esq:*, tsq:*, txs:* entries for old slots.
    /// Call periodically (e.g., every 1000 slots) to bound CF_STATS growth.
    /// At 1M blocks with 10K retention, this prevents ~990K stale sequence keys.
    pub fn prune_slot_stats(&self, current_slot: u64, retain_slots: u64) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        if current_slot <= retain_slots {
            return Ok(0);
        }
        let cutoff = current_slot - retain_slots;
        let mut batch = WriteBatch::default();
        let mut deleted = 0u64;

        // 1. Prune fee_dist:{slot} (text-format slot in key)
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"fee_dist:", Direction::Forward),
        );
        for item in iter.flatten() {
            if !item.0.starts_with(b"fee_dist:") {
                break;
            }
            if let Ok(s) = std::str::from_utf8(&item.0[9..]) {
                if let Ok(slot) = s.parse::<u64>() {
                    if slot < cutoff {
                        batch.delete_cf(&cf, &item.0);
                        deleted += 1;
                    }
                }
            }
        }

        // 2. Prune reward_dist:{slot} (text-format slot in key)
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"reward_dist:", Direction::Forward),
        );
        for item in iter.flatten() {
            if !item.0.starts_with(b"reward_dist:") {
                break;
            }
            if let Ok(s) = std::str::from_utf8(&item.0[12..]) {
                if let Ok(slot) = s.parse::<u64>() {
                    if slot < cutoff {
                        batch.delete_cf(&cf, &item.0);
                        deleted += 1;
                    }
                }
            }
        }

        // 3. Prune esq:{program}{slot} (binary: 4 prefix + 32 pubkey + 8 BE slot = 44 bytes)
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"esq:", Direction::Forward),
        );
        for item in iter.flatten() {
            if !item.0.starts_with(b"esq:") {
                break;
            }
            if item.0.len() == 44 {
                let slot = u64::from_be_bytes(item.0[36..44].try_into().unwrap());
                if slot < cutoff {
                    batch.delete_cf(&cf, &item.0);
                    deleted += 1;
                }
            }
        }

        // 4. Prune tsq:{token}{slot} (binary: 4 prefix + 32 pubkey + 8 BE slot = 44 bytes)
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"tsq:", Direction::Forward),
        );
        for item in iter.flatten() {
            if !item.0.starts_with(b"tsq:") {
                break;
            }
            if item.0.len() == 44 {
                let slot = u64::from_be_bytes(item.0[36..44].try_into().unwrap());
                if slot < cutoff {
                    batch.delete_cf(&cf, &item.0);
                    deleted += 1;
                }
            }
        }

        // 5. Prune txs:{slot} (binary: 4 prefix + 8 BE slot = 12 bytes)
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"txs:", Direction::Forward),
        );
        for item in iter.flatten() {
            if !item.0.starts_with(b"txs:") {
                break;
            }
            if item.0.len() == 12 {
                let slot = u64::from_be_bytes(item.0[4..12].try_into().unwrap());
                if slot < cutoff {
                    batch.delete_cf(&cf, &item.0);
                    deleted += 1;
                }
            }
        }

        // 6. Prune dirty_acct:* keys (already processed by compute_state_root)
        // AUDIT-FIX C-1: dirty_acct keys have format "dirty_acct:{pubkey}" (43 bytes total)
        // with NO slot component. We prune ALL dirty_acct keys since they are only
        // relevant for the state root computation of the current/recent block, which
        // has already been computed by the time pruning runs.
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"dirty_acct:", Direction::Forward),
        );
        let mut dirty_deleted = 0u64;
        for item in iter.flatten() {
            if !item.0.starts_with(b"dirty_acct:") {
                break;
            }
            // Only prune if key length matches expected format (11 prefix + 32 pubkey)
            // to avoid accidentally deleting unrelated keys
            if item.0.len() == 43 {
                batch.delete_cf(&cf, &item.0);
                dirty_deleted += 1;
                deleted += 1;
            }
        }

        // Apply batch delete atomically
        if deleted > 0 {
            self.db
                .write(batch)
                .map_err(|e| format!("Failed to prune stats: {}", e))?;

            // AUDIT-FIX C-2: Only reset dirty counter if we actually pruned dirty
            // keys, and only to 0 (meaning "no outstanding dirty markers"). The
            // mark_account_dirty_with_key() function uses a non-zero marker (1)
            // so any concurrent writes will re-set it to 1 after this reset.
            // This is safe because the dirty flag is a simple "has any dirty"
            // indicator, not a count.
            if dirty_deleted > 0 {
                if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
                    if let Err(e) =
                        self.db
                            .put_cf(&cf_stats, b"dirty_account_count", 0u64.to_le_bytes())
                    {
                        tracing::error!("Failed to reset dirty_account_count after prune: {e}");
                    }
                }
            }
        }

        Ok(deleted)
    }

    /// Task 3.6: Store a single validator oracle price attestation.
    ///
    /// Key: "oracle_att_{asset}_{validator_hex}" in CF_STATS.
    /// Value: JSON-serialized OracleAttestation.
    pub fn put_oracle_attestation(
        &self,
        asset: &str,
        validator: &Pubkey,
        price: u64,
        decimals: u8,
        stake: u64,
        slot: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let val_hex = hex::encode(validator.0);
        let key = format!("oracle_att_{}_{}", asset, val_hex);

        let att = crate::processor::OracleAttestation {
            validator: *validator,
            price,
            decimals,
            stake,
            slot,
        };
        let data = serde_json::to_vec(&att)
            .map_err(|e| format!("Failed to serialize oracle attestation: {}", e))?;
        self.db
            .put_cf(&cf, key.as_bytes(), data)
            .map_err(|e| format!("Failed to store oracle attestation: {}", e))
    }

    /// Task 3.6: Get all non-stale oracle attestations for an asset.
    ///
    /// Scans CF_STATS for keys matching "oracle_att_{asset}_*" and filters
    /// out any older than `staleness_window` slots from `current_slot`.
    pub fn get_oracle_attestations(
        &self,
        asset: &str,
        current_slot: u64,
        staleness_window: u64,
    ) -> Result<Vec<crate::processor::OracleAttestation>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let prefix = format!("oracle_att_{}_", asset);
        let mut results = Vec::new();

        let iter = self.db.prefix_iterator_cf(&cf, prefix.as_bytes());
        for item in iter {
            let (key, value) = item.map_err(|e| format!("DB iterator error: {}", e))?;
            let key_str = std::str::from_utf8(&key).unwrap_or("");
            if !key_str.starts_with(&prefix) {
                break;
            }
            if let Ok(att) = serde_json::from_slice::<crate::processor::OracleAttestation>(&value) {
                if current_slot.saturating_sub(att.slot) <= staleness_window {
                    results.push(att);
                }
            }
        }
        Ok(results)
    }

    /// Task 3.6: Store the consensus oracle price for an asset.
    ///
    /// Key: "oracle_price_{asset}" in CF_STATS.
    pub fn put_oracle_consensus_price(
        &self,
        asset: &str,
        price: u64,
        decimals: u8,
        slot: u64,
        attestation_count: u32,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("oracle_price_{}", asset);
        let cp = crate::processor::OracleConsensusPrice {
            asset: asset.to_string(),
            price,
            decimals,
            slot,
            attestation_count,
        };
        let data = serde_json::to_vec(&cp)
            .map_err(|e| format!("Failed to serialize consensus price: {}", e))?;
        self.db
            .put_cf(&cf, key.as_bytes(), data)
            .map_err(|e| format!("Failed to store consensus price: {}", e))
    }

    /// Task 3.6: Get the consensus oracle price for an asset.
    pub fn get_oracle_consensus_price(
        &self,
        asset: &str,
    ) -> Result<Option<crate::processor::OracleConsensusPrice>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("oracle_price_{}", asset);
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                let cp = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize consensus price: {}", e))?;
                Ok(Some(cp))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Get total spores burned (fee burn)
    pub fn get_total_burned(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        match self.db.get_cf(&cf, b"total_burned") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid burned data".to_string())?;
                Ok(u64::from_le_bytes(bytes))
            }
            Ok(None) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Add to total burned amount.
    ///
    /// P10-CORE-01 FIX: The read-modify-write is protected by `burned_lock` to
    /// prevent lost updates when called concurrently. The primary burn path
    /// goes through `StateBatch::add_burned()` (which accumulates a delta and
    /// commits atomically), but this direct method is also used in tests and
    /// non-batch code paths.
    pub fn add_burned(&self, amount: u64) -> Result<(), String> {
        let _guard = self
            .burned_lock
            .lock()
            .map_err(|e| format!("burned_lock poisoned: {}", e))?;

        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let current = self.get_total_burned()?;
        let new_total = current.saturating_add(amount);

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(&cf, b"total_burned", new_total.to_le_bytes());
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to store burned amount: {}", e))
    }

    /// Get total spores minted (block rewards)
    pub fn get_total_minted(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        match self.db.get_cf(&cf, b"total_minted") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid minted data".to_string())?;
                Ok(u64::from_le_bytes(bytes))
            }
            Ok(None) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Add to total minted amount.
    ///
    /// Protected by `minted_lock` to prevent lost updates under concurrent
    /// access. The primary mint path goes through `StateBatch::add_minted()`
    /// (which accumulates a delta and commits atomically), but this direct
    /// method is available for tests and non-batch code paths.
    pub fn add_minted(&self, amount: u64) -> Result<(), String> {
        let _guard = self
            .minted_lock
            .lock()
            .map_err(|e| format!("minted_lock poisoned: {}", e))?;

        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let current = self.get_total_minted()?;
        let new_total = current.saturating_add(amount);

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(&cf, b"total_minted", new_total.to_le_bytes());
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to store minted amount: {}", e))
    }
}
