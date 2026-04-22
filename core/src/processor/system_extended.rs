use super::*;

impl TxProcessor {
    /// System program: Create NFT collection
    pub(super) fn system_create_collection(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Create collection requires creator and collection accounts".to_string());
        }

        let creator = ix.accounts[0];
        let collection_account = ix.accounts[1];

        if self.b_get_account(&collection_account)?.is_some() {
            return Err("Collection account already exists".to_string());
        }

        if ix.data.len() < 2 {
            return Err("Invalid collection data".to_string());
        }

        let mut data = decode_create_collection_data(&ix.data[1..])?;
        if !data.public_mint && data.mint_authority.is_none() {
            data.mint_authority = Some(creator);
        }

        let state = CollectionState {
            version: NFT_COLLECTION_VERSION,
            name: data.name,
            symbol: data.symbol,
            creator,
            royalty_bps: data.royalty_bps,
            max_supply: data.max_supply,
            minted: 0,
            public_mint: data.public_mint,
            mint_authority: data.mint_authority,
        };

        let mut account = Account::new(0, SYSTEM_PROGRAM_ID);
        account.data = encode_collection_state(&state)?;

        self.b_put_account(&collection_account, &account)?;

        Ok(())
    }

    /// System program: Mint NFT
    pub(super) fn system_mint_nft(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 4 {
            return Err("Mint requires minter, collection, token, and owner accounts".to_string());
        }

        let minter = ix.accounts[0];
        let collection_account = ix.accounts[1];
        let token_account = ix.accounts[2];
        let owner = ix.accounts[3];

        if self.b_get_account(&token_account)?.is_some() {
            return Err("Token account already exists".to_string());
        }

        if ix.data.len() < 2 {
            return Err("Invalid mint data".to_string());
        }

        let mint_data = decode_mint_nft_data(&ix.data[1..])?;
        let collection = self
            .b_get_account(&collection_account)?
            .ok_or_else(|| "Collection not found".to_string())?;
        let mut collection_state = decode_collection_state(&collection.data)?;

        if collection_state.max_supply > 0 && collection_state.minted >= collection_state.max_supply
        {
            return Err("Collection supply exhausted".to_string());
        }

        if !collection_state.public_mint {
            let authority = collection_state
                .mint_authority
                .unwrap_or(collection_state.creator);
            if authority != minter {
                return Err("Unauthorized minter".to_string());
            }
        }

        // T2.11 fix: Enforce token_id uniqueness within the collection
        // AUDIT-FIX 1.15: Use batch-aware check to prevent TOCTOU race in same block
        if self
            .b_nft_token_id_exists(&collection_account, mint_data.token_id)
            .unwrap_or(false)
        {
            return Err(format!(
                "Token ID {} already exists in collection {}",
                mint_data.token_id,
                collection_account.to_base58()
            ));
        }

        let token_state = TokenState {
            version: NFT_TOKEN_VERSION,
            collection: collection_account,
            token_id: mint_data.token_id,
            owner,
            metadata_uri: mint_data.metadata_uri,
        };

        let mut token_account_data = Account::new(0, SYSTEM_PROGRAM_ID);
        token_account_data.data = encode_token_state(&token_state)?;

        collection_state.minted = collection_state.minted.saturating_add(1);
        let mut updated_collection = collection;
        updated_collection.data = encode_collection_state(&collection_state)?;

        self.b_put_account(&collection_account, &updated_collection)?;
        self.b_put_account(&token_account, &token_account_data)?;
        self.b_index_nft_mint(&collection_account, &token_account, &owner)?;
        // AUDIT-FIX B-3: Propagate token_id index error instead of swallowing it.
        // A successful mint without an index is invisible to query APIs.
        self.b_index_nft_token_id(&collection_account, mint_data.token_id, &token_account)?;

        Ok(())
    }

    /// System program: Transfer NFT
    pub(super) fn system_transfer_nft(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 3 {
            return Err("Transfer NFT requires owner, token, and recipient accounts".to_string());
        }

        let owner = ix.accounts[0];
        let token_account = ix.accounts[1];
        let recipient = ix.accounts[2];

        let token = self
            .b_get_account(&token_account)?
            .ok_or_else(|| "Token account not found".to_string())?;
        let mut token_state = decode_token_state(&token.data)?;

        if token_state.owner != owner {
            return Err("Unauthorized NFT transfer".to_string());
        }

        token_state.owner = recipient;

        let mut updated_token = token;
        updated_token.data = encode_token_state(&token_state)?;

        self.b_put_account(&token_account, &updated_token)?;
        self.b_index_nft_transfer(&token_state.collection, &token_account, &owner, &recipient)?;

        Ok(())
    }

    /// System program: Stake LICN
    pub(super) fn system_stake(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Stake requires staker and validator accounts".to_string());
        }

        if ix.data.len() < 9 {
            return Err("Invalid stake data".to_string());
        }

        let staker = ix.accounts[0];
        let validator = ix.accounts[1];

        let amount_bytes: [u8; 8] = ix.data[1..9]
            .try_into()
            .map_err(|_| "Invalid amount encoding".to_string())?;
        let amount = u64::from_le_bytes(amount_bytes);

        let mut account = self
            .b_get_account(&staker)?
            .ok_or_else(|| "Staker account not found".to_string())?;
        account.stake(amount)?;
        self.b_put_account(&staker, &account)?;

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut pool = self.b_get_stake_pool()?;
        if pool.get_stake(&validator).is_none() {
            return Err(format!(
                "Validator {} is not registered in the stake pool",
                validator.to_base58()
            ));
        }
        pool.stake(validator, amount, current_slot)?;
        self.b_put_stake_pool(&pool)?;

        Ok(())
    }

    /// System program: Request unstake
    pub(super) fn system_request_unstake(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Unstake requires staker and validator accounts".to_string());
        }

        if ix.data.len() < 9 {
            return Err("Invalid unstake data".to_string());
        }

        let staker = ix.accounts[0];
        let validator = ix.accounts[1];

        let amount_bytes: [u8; 8] = ix.data[1..9]
            .try_into()
            .map_err(|_| "Invalid amount encoding".to_string())?;
        let amount = u64::from_le_bytes(amount_bytes);

        let mut account = self
            .b_get_account(&staker)?
            .ok_or_else(|| "Staker account not found".to_string())?;
        if amount > account.staked {
            return Err("Insufficient staked balance".to_string());
        }

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut pool = self.b_get_stake_pool()?;
        pool.request_unstake(&validator, amount, current_slot, staker)?;
        self.b_put_stake_pool(&pool)?;

        account.unstake(amount)?;
        account.lock(amount)?;
        self.b_put_account(&staker, &account)?;

        Ok(())
    }

    /// System program: Claim unstaked LICN
    pub(super) fn system_claim_unstake(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Claim unstake requires staker and validator accounts".to_string());
        }

        let staker = ix.accounts[0];
        let validator = ix.accounts[1];

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut pool = self.b_get_stake_pool()?;
        let amount = pool.claim_unstake(&validator, current_slot, &staker)?;
        self.b_put_stake_pool(&pool)?;

        let mut account = self
            .b_get_account(&staker)?
            .ok_or_else(|| "Staker account not found".to_string())?;
        if amount > account.locked {
            return Err("Insufficient locked balance".to_string());
        }
        account.unlock(amount)?;
        self.b_put_account(&staker, &account)?;

        Ok(())
    }

    // ========================================================================
    // MOSSSTAKE — Liquid Staking (T6.1: wired to processor)
    // ========================================================================

    /// System program: MossStake deposit (instruction type 13)
    /// data: [13, amount(8)]
    /// accounts: [depositor]
    pub(super) fn system_mossstake_deposit(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("MossStake deposit requires depositor account".to_string());
        }
        if ix.data.len() < 9 {
            return Err("Invalid MossStake deposit data".to_string());
        }

        let depositor = ix.accounts[0];
        let amount_bytes: [u8; 8] = ix.data[1..9]
            .try_into()
            .map_err(|_| "Invalid amount encoding".to_string())?;
        let amount = u64::from_le_bytes(amount_bytes);

        if amount == 0 {
            return Err("Cannot deposit 0 LICN".to_string());
        }

        let tier_byte = ix.data.get(9).copied().unwrap_or(0);
        let tier = crate::mossstake::LockTier::from_u8(tier_byte)
            .ok_or_else(|| format!("Invalid lock tier: {}", tier_byte))?;

        let mut account = self
            .b_get_account(&depositor)?
            .ok_or_else(|| "Depositor account not found".to_string())?;
        account.deduct_spendable(amount)?;
        self.b_put_account(&depositor, &account)?;

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut pool = self.b_get_mossstake_pool()?;
        let _st_licn = pool.stake_with_tier(depositor, amount, current_slot, tier)?;
        self.b_put_mossstake_pool(&pool)?;

        Ok(())
    }

    /// System program: MossStake request unstake (instruction type 14)
    /// data: [14, st_licn_amount(8)]
    /// accounts: [user]
    pub(super) fn system_mossstake_unstake(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("MossStake unstake requires user account".to_string());
        }
        if ix.data.len() < 9 {
            return Err("Invalid MossStake unstake data".to_string());
        }

        let user = ix.accounts[0];
        let amount_bytes: [u8; 8] = ix.data[1..9]
            .try_into()
            .map_err(|_| "Invalid amount encoding".to_string())?;
        let st_licn_amount = u64::from_le_bytes(amount_bytes);

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut pool = self.b_get_mossstake_pool()?;
        let _request = pool.request_unstake(user, st_licn_amount, current_slot)?;
        self.b_put_mossstake_pool(&pool)?;

        Ok(())
    }

    /// System program: MossStake claim (instruction type 15)
    /// data: [15]
    /// accounts: [user]
    pub(super) fn system_mossstake_claim(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("MossStake claim requires user account".to_string());
        }

        let user = ix.accounts[0];
        let current_slot = self.b_get_last_slot().unwrap_or(0);

        let mut pool = self.b_get_mossstake_pool()?;
        let licn_claimed = pool.claim_unstake(user, current_slot)?;
        self.b_put_mossstake_pool(&pool)?;

        if licn_claimed == 0 {
            return Err("No claimable LICN (cooldown not complete)".to_string());
        }

        let mut account = self
            .b_get_account(&user)?
            .ok_or_else(|| "User account not found".to_string())?;
        account.add_spendable(licn_claimed)?;
        self.b_put_account(&user, &account)?;

        Ok(())
    }

    /// System program: MossStake stLICN transfer (instruction type 16)
    /// data: [16, st_licn_amount(8)]
    /// accounts: [from, to]
    pub(super) fn system_mossstake_transfer(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("MossStake transfer requires sender and receiver accounts".to_string());
        }
        if ix.data.len() < 9 {
            return Err("Invalid MossStake transfer data".to_string());
        }

        let from = ix.accounts[0];
        let to = ix.accounts[1];
        let amount_bytes: [u8; 8] = ix.data[1..9]
            .try_into()
            .map_err(|_| "Invalid amount encoding".to_string())?;
        let st_licn_amount = u64::from_le_bytes(amount_bytes);

        if self.b_get_account(&to)?.is_none() {
            self.b_put_account(&to, &crate::Account::new(0, SYSTEM_PROGRAM_ID))?;
        }

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut pool = self.b_get_mossstake_pool()?;
        pool.transfer(from, to, st_licn_amount, current_slot)?;
        self.b_put_mossstake_pool(&pool)?;

        Ok(())
    }

    /// H16 fix: Deploy contract through consensus (instruction type 17).
    /// Instruction data: [17 | code_length(4 LE) | code_bytes | init_data_bytes]
    /// Accounts: [deployer, treasury]
    /// The deployer must be a transaction signer. Deploy fee charged from deployer.
    pub(super) fn system_deploy_contract(&self, ix: &Instruction) -> Result<(), String> {
        use sha2::{Digest, Sha256};

        if ix.accounts.len() < 2 {
            return Err("DeployContract requires [deployer, treasury] accounts".to_string());
        }
        if ix.data.len() < 6 {
            return Err("DeployContract instruction data too short".to_string());
        }

        let deployer = ix.accounts[0];
        let treasury = ix.accounts[1];

        let code_len = u32::from_le_bytes(
            ix.data[1..5]
                .try_into()
                .map_err(|_| "Invalid code length encoding".to_string())?,
        ) as usize;
        if ix.data.len() < 5 + code_len {
            return Err(
                "DeployContract: instruction data shorter than declared code_length".to_string(),
            );
        }
        let code_bytes = &ix.data[5..5 + code_len];
        let init_data_bytes = if ix.data.len() > 5 + code_len {
            &ix.data[5 + code_len..]
        } else {
            &[]
        };

        if code_bytes.is_empty() {
            return Err("DeployContract: code cannot be empty".to_string());
        }

        if code_bytes.len() > MAX_CONTRACT_CODE {
            return Err(format!(
                "DeployContract: code size {} exceeds maximum {} bytes",
                code_bytes.len(),
                MAX_CONTRACT_CODE
            ));
        }

        if code_bytes.len() < 8 {
            return Err("DeployContract: code too small to be valid WASM".to_string());
        }
        const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];
        if code_bytes[..4] != WASM_MAGIC {
            return Err("DeployContract: invalid WASM module (bad magic number)".to_string());
        }

        let actual_treasury = self
            .state
            .get_treasury_pubkey()?
            .ok_or_else(|| "Treasury pubkey not set".to_string())?;
        if treasury != actual_treasury {
            return Err("DeployContract: incorrect treasury account".to_string());
        }

        let init_payload = if init_data_bytes.is_empty() {
            None
        } else {
            serde_json::from_slice::<serde_json::Value>(init_data_bytes).ok()
        };

        let contract_name = init_payload.as_ref().and_then(|v| {
            v.get("name")
                .or_else(|| v.get("symbol"))
                .and_then(|n| n.as_str().map(|s| s.to_string()))
        });
        let deploy_salt = init_payload.as_ref().and_then(|v| {
            v.get("deploy_salt")
                .or_else(|| v.get("deploySalt"))
                .and_then(|value| value.as_str().map(|s| s.to_string()))
        });
        if let Some(ref salt) = deploy_salt {
            if salt.is_empty() {
                return Err("DeployContract: deploy_salt cannot be empty".to_string());
            }
            if salt.len() > 64 {
                return Err("DeployContract: deploy_salt exceeds 64 bytes".to_string());
            }
        }
        let deterministic_address = init_payload
            .as_ref()
            .and_then(|v| {
                v.get("deploy_deterministic")
                    .or_else(|| v.get("deployDeterministic"))
                    .and_then(|value| value.as_bool())
            })
            .unwrap_or(false);
        let deploy_nonce = if deploy_salt.is_none() && !deterministic_address {
            Some(self.b_next_contract_deploy_nonce(&deployer)?)
        } else {
            None
        };

        let mut addr_hasher = Sha256::new();
        addr_hasher.update(b"lichen_contract_deploy_v2");
        addr_hasher.update(deployer.0);
        if let Some(ref salt) = deploy_salt {
            addr_hasher.update(b"salt");
            addr_hasher.update((salt.len() as u32).to_le_bytes());
            addr_hasher.update(salt.as_bytes());
        } else if let Some(nonce) = deploy_nonce {
            addr_hasher.update(b"nonce");
            addr_hasher.update(nonce.to_le_bytes());
        } else {
            addr_hasher.update(b"deterministic");
        }
        if let Some(ref name) = contract_name {
            addr_hasher.update(name.as_bytes());
        }
        addr_hasher.update(code_bytes);
        let addr_hash = addr_hasher.finalize();
        let mut addr_bytes = [0u8; 32];
        addr_bytes.copy_from_slice(&addr_hash[..32]);
        let program_pubkey = crate::Pubkey(addr_bytes);

        if self.b_get_account(&program_pubkey)?.is_some() {
            return Err(format!(
                "Contract already exists at {}",
                program_pubkey.to_base58()
            ));
        }

        let contract = crate::ContractAccount::new(code_bytes.to_vec(), deployer);
        let mut account = crate::Account::new(0, program_pubkey);
        account.data = serde_json::to_vec(&contract)
            .map_err(|e| format!("Failed to serialize contract: {}", e))?;
        account.executable = true;
        self.b_put_account(&program_pubkey, &account)?;

        self.b_index_program(&program_pubkey)?;

        if let Some(registry_data) = init_payload.as_ref() {
            if let Some(symbol) = registry_data.get("symbol").and_then(|s| s.as_str()) {
                let entry = crate::SymbolRegistryEntry {
                    symbol: symbol.to_string(),
                    program: program_pubkey,
                    owner: deployer,
                    name: registry_data
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string()),
                    template: registry_data
                        .get("template")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string()),
                    metadata: registry_data.get("metadata").cloned(),
                    decimals: registry_data
                        .get("decimals")
                        .and_then(|d| d.as_u64())
                        .map(|d| d as u8),
                };
                self.b_register_symbol(symbol, entry)?;
            }
        }

        Ok(())
    }

    /// H16 fix: Faucet airdrop through consensus (instruction type 19).
    /// Instruction data: [19 | amount_spores(8 LE)]
    /// Accounts: [treasury, recipient]
    /// Treasury must be a signer. Amount capped at 10 LICN.
    pub(super) fn system_faucet_airdrop(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("FaucetAirdrop requires [treasury, recipient] accounts".to_string());
        }
        if ix.data.len() < 9 {
            return Err("FaucetAirdrop: missing amount data".to_string());
        }

        let treasury = ix.accounts[0];
        let recipient = ix.accounts[1];

        let actual_treasury = self
            .state
            .get_treasury_pubkey()?
            .ok_or_else(|| "Treasury pubkey not set".to_string())?;
        if treasury != actual_treasury {
            return Err("FaucetAirdrop: sender must be treasury".to_string());
        }

        let amount_spores = u64::from_le_bytes(
            ix.data[1..9]
                .try_into()
                .map_err(|_| "Invalid amount encoding".to_string())?,
        );

        let max_airdrop = 10u64 * 1_000_000_000;
        if amount_spores == 0 || amount_spores > max_airdrop {
            return Err(format!(
                "FaucetAirdrop: amount must be between 1 spore and {} spores (10 LICN)",
                max_airdrop
            ));
        }

        let mut treasury_account = self
            .b_get_account(&treasury)?
            .ok_or_else(|| "Treasury account not found".to_string())?;
        treasury_account
            .deduct_spendable(amount_spores)
            .map_err(|e| format!("Insufficient treasury balance: {}", e))?;
        self.b_put_account(&treasury, &treasury_account)?;

        let mut recipient_account = self
            .b_get_account(&recipient)?
            .unwrap_or_else(|| crate::Account::new(0, SYSTEM_PROGRAM_ID));
        recipient_account
            .add_spendable(amount_spores)
            .map_err(|e| format!("Recipient balance overflow: {}", e))?;
        self.b_put_account(&recipient, &recipient_account)?;

        Ok(())
    }
}
