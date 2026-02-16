// MoltChain Core - Transaction Processor

use crate::account::{Account, Pubkey};
use crate::contract::{ContractAbi, ContractAccount, ContractContext, ContractRuntime};
use crate::contract_instruction::ContractInstruction;
use crate::evm::{
    decode_evm_transaction, execute_evm_transaction, u256_is_multiple_of_shell, u256_to_shells,
    EvmReceipt, EvmTxRecord, EVM_PROGRAM_ID,
};
use crate::state::{StateBatch, StateStore, SymbolRegistryEntry};
use crate::Hash;
use crate::transaction::{Instruction, Transaction};
use alloy_primitives::U256;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Mutex;

/// Transaction execution result
#[derive(Debug, Clone)]
pub struct TxResult {
    pub success: bool,
    pub fee_paid: u64,
    pub error: Option<String>,
}

/// Simulation result (dry-run)
#[derive(Debug, Clone, serde::Serialize)]
pub struct SimulationResult {
    pub success: bool,
    pub fee: u64,
    pub logs: Vec<String>,
    pub error: Option<String>,
    pub compute_used: u64,
    pub return_data: Option<Vec<u8>>,
}

fn is_evm_instruction(tx: &Transaction) -> bool {
    tx.message
        .instructions
        .first()
        .map(|ix| ix.program_id == EVM_PROGRAM_ID)
        .unwrap_or(false)
}

/// System program ID (all zeros)
pub const SYSTEM_PROGRAM_ID: Pubkey = Pubkey([0u8; 32]);
use crate::nft::{
    decode_collection_state, decode_create_collection_data, decode_mint_nft_data,
    decode_token_state, encode_collection_state, encode_token_state, CollectionState, TokenState,
    NFT_COLLECTION_VERSION, NFT_TOKEN_VERSION,
};

/// Smart contract program ID (all ones)
pub const CONTRACT_PROGRAM_ID: Pubkey = Pubkey([0xFFu8; 32]);

/// Slot-based month length (400ms slots, 216,000 per day)
pub const SLOTS_PER_MONTH: u64 = 216_000 * 30;
/// Base transaction fee (0.001 MOLT = 1,000,000 shells)
/// At $0.10/MOLT: $0.0001 per tx  |  At $1.00/MOLT: $0.001 per tx
/// Solana ~$0.00025/tx — MoltChain is 2.5x cheaper at $0.10/MOLT
pub const BASE_FEE: u64 = 1_000_000;

/// Contract deployment fee (25 MOLT = 25,000,000,000 shells)
/// At $0.10/MOLT: $2.50 per deploy  |  At $1.00/MOLT: $25 per deploy
pub const CONTRACT_DEPLOY_FEE: u64 = 25_000_000_000;

/// Contract upgrade fee (10 MOLT = 10,000,000,000 shells)
/// At $0.10/MOLT: $1.00 per upgrade  |  At $1.00/MOLT: $10 per upgrade
pub const CONTRACT_UPGRADE_FEE: u64 = 10_000_000_000;

/// NFT mint fee (0.5 MOLT = 500,000,000 shells)
/// At $0.10/MOLT: $0.05 per mint  |  At $1.00/MOLT: $0.50 per mint
pub const NFT_MINT_FEE: u64 = 500_000_000;

/// NFT collection creation fee (1,000 MOLT = 1,000,000,000,000 shells)
/// At $0.10/MOLT: $100 per collection  |  At $1.00/MOLT: $1,000 per collection
pub const NFT_COLLECTION_FEE: u64 = 1_000_000_000_000;

#[derive(Debug, Clone, Copy)]
pub struct FeeConfig {
    pub base_fee: u64,
    pub contract_deploy_fee: u64,
    pub contract_upgrade_fee: u64,
    pub nft_mint_fee: u64,
    pub nft_collection_fee: u64,
    /// Percentage of fees to burn (0-100)
    pub fee_burn_percent: u64,
    /// Percentage of fees to block producer (0-100)
    pub fee_producer_percent: u64,
    /// Percentage of fees to voters (0-100)
    pub fee_voters_percent: u64,
    /// Percentage of fees to treasury (0-100) per whitepaper: 10%
    pub fee_treasury_percent: u64,
}

impl FeeConfig {
    pub fn default_from_constants() -> Self {
        FeeConfig {
            base_fee: BASE_FEE,
            contract_deploy_fee: CONTRACT_DEPLOY_FEE,
            contract_upgrade_fee: CONTRACT_UPGRADE_FEE,
            nft_mint_fee: NFT_MINT_FEE,
            nft_collection_fee: NFT_COLLECTION_FEE,
            fee_burn_percent: 50,
            fee_producer_percent: 30,
            fee_voters_percent: 10,
            fee_treasury_percent: 10,
        }
    }
}

/// Transaction processor
pub struct TxProcessor {
    state: StateStore,
    batch: Mutex<Option<StateBatch>>,
}

impl TxProcessor {
    pub fn new(state: StateStore) -> Self {
        TxProcessor {
            state,
            batch: Mutex::new(None),
        }
    }

    /// Calculate total fees for a transaction (base + program-specific)
    /// Applies reputation-based fee discount per whitepaper:
    ///   500+ reputation → 10% discount
    ///   750+ reputation → 20% discount
    ///   1000+ reputation → 30% discount
    pub fn compute_transaction_fee(tx: &Transaction, fee_config: &FeeConfig) -> u64 {
        // Internal system transaction types 2-5 are fee-free:
        //   2 = Reward distribution (validator block rewards from treasury)
        //   3 = Grant/debt repayment (validator grant repayment to treasury)
        //   4 = Genesis transfer (initial treasury funding)
        //   5 = Genesis mint (initial supply creation)
        // These are created by the validator itself and must not be charged fees.
        if let Some(first_ix) = tx.message.instructions.first() {
            if first_ix.program_id == SYSTEM_PROGRAM_ID {
                if let Some(&kind) = first_ix.data.first() {
                    if matches!(kind, 2..=5) {
                        return 0;
                    }
                }
            }
            if first_ix.program_id == EVM_PROGRAM_ID {
                // EVM transactions: estimate fee from the embedded gas parameters.
                // The actual charge at execution time is gas_price * gas_used, but
                // at simulation time we don't know gas_used yet, so we estimate
                // with gas_price * gas_limit (the maximum possible charge).
                if let Ok(evm_tx) = decode_evm_transaction(&first_ix.data) {
                    let estimated = u256_to_shells(
                        &(evm_tx.gas_price * U256::from(evm_tx.gas_limit)),
                    );
                    return if estimated > 0 { estimated } else { fee_config.base_fee };
                }
                return fee_config.base_fee;
            }
        }

        let mut total = fee_config.base_fee;

        for ix in &tx.message.instructions {
            if ix.program_id == SYSTEM_PROGRAM_ID {
                if let Some(kind) = ix.data.first() {
                    match *kind {
                        6 => total = total.saturating_add(fee_config.nft_collection_fee),
                        7 => total = total.saturating_add(fee_config.nft_mint_fee),
                        _ => {}
                    }
                }
            }
            if ix.program_id == CONTRACT_PROGRAM_ID {
                if let Ok(contract_ix) = ContractInstruction::deserialize(&ix.data) {
                    match contract_ix {
                        ContractInstruction::Deploy { .. } => {
                            total = total.saturating_add(fee_config.contract_deploy_fee)
                        }
                        ContractInstruction::Upgrade { .. } => {
                            total = total.saturating_add(fee_config.contract_upgrade_fee)
                        }
                        _ => {}
                    }
                }
            }
        }

        total
    }

    /// Apply reputation-based fee discount per whitepaper:
    ///   reputation 500-749  → 10% discount
    ///   reputation 750-999  → 20% discount
    ///   reputation 1000+    → 30% discount
    pub fn apply_reputation_fee_discount(base_fee: u64, reputation: u64) -> u64 {
        let discount_percent = if reputation >= 1000 {
            30
        } else if reputation >= 750 {
            20
        } else if reputation >= 500 {
            10
        } else {
            0
        };
        base_fee.saturating_sub(base_fee * discount_percent / 100)
    }

    // ========================================================================
    // RATE LIMITING (per whitepaper: reputation-based tx throughput)
    // ========================================================================

    /// Default tx-per-epoch limit for accounts with no reputation
    const BASE_TX_LIMIT_PER_EPOCH: u64 = 100;

    /// Check if an account has exceeded its per-epoch rate limit.
    /// Reputation increases the limit:
    ///   0-499 rep   → 100 tx/epoch
    ///   500-999 rep → 200 tx/epoch
    ///   1000+ rep   → 500 tx/epoch
    /// Returns Ok(()) if under limit, Err with message if exceeded.
    pub fn check_rate_limit_static(
        reputation: u64,
        tx_count_this_epoch: u64,
    ) -> Result<(), String> {
        let limit = if reputation >= 1000 {
            Self::BASE_TX_LIMIT_PER_EPOCH * 5
        } else if reputation >= 500 {
            Self::BASE_TX_LIMIT_PER_EPOCH * 2
        } else {
            Self::BASE_TX_LIMIT_PER_EPOCH
        };

        if tx_count_this_epoch >= limit {
            return Err(format!(
                "Rate limit exceeded: {} tx this epoch (limit {})",
                tx_count_this_epoch, limit
            ));
        }

        Ok(())
    }

    /// Get the rate limit for a given reputation level
    pub fn rate_limit_for_reputation(reputation: u64) -> u64 {
        if reputation >= 1000 {
            Self::BASE_TX_LIMIT_PER_EPOCH * 5
        } else if reputation >= 500 {
            Self::BASE_TX_LIMIT_PER_EPOCH * 2
        } else {
            Self::BASE_TX_LIMIT_PER_EPOCH
        }
    }

    // ─── Batch-aware state accessors (T1.4/T3.1) ───────────────────

    fn b_get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_account(pubkey)
        } else {
            self.state.get_account(pubkey)
        }
    }

    fn b_put_account(&self, pubkey: &Pubkey, account: &Account) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_account(pubkey, account)
        } else {
            self.state.put_account(pubkey, account)
        }
    }

    fn b_transfer(&self, from: &Pubkey, to: &Pubkey, amount: u64) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.transfer(from, to, amount)
        } else {
            self.state.transfer(from, to, amount)
        }
    }

    fn b_put_transaction(&self, tx: &Transaction) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_transaction(tx)
        } else {
            self.state.put_transaction(tx)
        }
    }

    fn b_put_stake_pool(&self, pool: &crate::consensus::StakePool) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_stake_pool(pool)
        } else {
            self.state.put_stake_pool(pool)
        }
    }

    fn b_get_stake_pool(&self) -> Result<crate::consensus::StakePool, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_stake_pool()
        } else {
            self.state.get_stake_pool()
        }
    }

    fn b_put_reefstake_pool(&self, pool: &crate::reefstake::ReefStakePool) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_reefstake_pool(pool)
        } else {
            self.state.put_reefstake_pool(pool)
        }
    }

    fn b_get_reefstake_pool(&self) -> Result<crate::reefstake::ReefStakePool, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_reefstake_pool()
        } else {
            self.state.get_reefstake_pool()
        }
    }

    fn b_put_contract_event(
        &self,
        program: &Pubkey,
        event: &crate::contract::ContractEvent,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_contract_event(program, event)
        } else {
            self.state.put_contract_event(program, event)
        }
    }

    /// Write contract storage change to CF_CONTRACT_STORAGE for fast-path access.
    fn b_put_contract_storage(
        &self,
        program: &Pubkey,
        storage_key: &[u8],
        value: &[u8],
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_contract_storage(program, storage_key, value)
        } else {
            self.state.put_contract_storage(program, storage_key, value)
        }
    }

    /// Delete contract storage key from CF_CONTRACT_STORAGE.
    fn b_delete_contract_storage(
        &self,
        program: &Pubkey,
        storage_key: &[u8],
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.delete_contract_storage(program, storage_key)
        } else {
            self.state.delete_contract_storage(program, storage_key)
        }
    }

    fn b_put_evm_tx(&self, record: &crate::evm::EvmTxRecord) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_evm_tx(record)
        } else {
            self.state.put_evm_tx(record)
        }
    }

    fn b_put_evm_receipt(&self, receipt: &crate::evm::EvmReceipt) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.put_evm_receipt(receipt)
        } else {
            self.state.put_evm_receipt(receipt)
        }
    }

    /// H3 fix: Apply deferred EVM state changes through the active batch.
    fn b_apply_evm_state_changes(
        &self,
        changes: &crate::evm::EvmStateChanges,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        let batch = guard
            .as_mut()
            .ok_or("No active batch for b_apply_evm_state_changes")?;
        batch.apply_evm_changes(&changes.changes)
    }

    fn b_register_evm_address(
        &self,
        evm_address: &[u8; 20],
        native: &Pubkey,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.register_evm_address(evm_address, native)
        } else {
            self.state.register_evm_address(evm_address, native)
        }
    }

    fn b_index_nft_mint(
        &self,
        collection: &Pubkey,
        token: &Pubkey,
        owner: &Pubkey,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.index_nft_mint(collection, token, owner)
        } else {
            self.state.index_nft_mint(collection, token, owner)
        }
    }

    fn b_index_nft_transfer(
        &self,
        collection: &Pubkey,
        token: &Pubkey,
        from: &Pubkey,
        to: &Pubkey,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.index_nft_transfer(collection, token, from, to)
        } else {
            self.state.index_nft_transfer(collection, token, from, to)
        }
    }

    /// M6 fix: index NFT token_id through batch for atomicity
    fn b_index_nft_token_id(
        &self,
        collection: &Pubkey,
        token_id: u64,
        token_account: &Pubkey,
    ) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.index_nft_token_id(collection, token_id, token_account)
        } else {
            self.state
                .index_nft_token_id(collection, token_id, token_account)
        }
    }

    /// AUDIT-FIX 1.15: Check token_id uniqueness against batch overlay + committed state
    fn b_nft_token_id_exists(
        &self,
        collection: &Pubkey,
        token_id: u64,
    ) -> Result<bool, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.nft_token_id_exists(collection, token_id)
        } else {
            self.state.nft_token_id_exists(collection, token_id)
        }
    }

    fn b_index_program(&self, program: &Pubkey) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.index_program(program)
        } else {
            self.state.index_program(program)
        }
    }

    fn b_register_symbol(&self, symbol: &str, entry: SymbolRegistryEntry) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_mut() {
            batch.register_symbol(symbol, &entry)
        } else {
            self.state.register_symbol(symbol, entry)
        }
    }

    fn b_get_last_slot(&self) -> Result<u64, String> {
        let guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(batch) = guard.as_ref() {
            batch.get_last_slot()
        } else {
            self.state.get_last_slot()
        }
    }

    /// Accumulate burned amount in the current batch (H3/H4 fix — atomic with tx state)
    /// NOTE: Currently unused — fee charging goes through charge_fee_direct() which
    /// handles burn tracking at the block level via validator/src/main.rs. Retained
    /// for potential future use with batch-scoped burn tracking.
    #[allow(dead_code)]
    fn b_add_burned(&self, amount: u64) -> Result<(), String> {
        let mut guard = self.batch.lock().unwrap_or_else(|e| e.into_inner());
        let batch = guard.as_mut().ok_or("No active batch for b_add_burned")?;
        batch.add_burned(amount);
        Ok(())
    }

    /// Start an atomic batch for the current transaction.
    fn begin_batch(&self) {
        *self.batch.lock().unwrap_or_else(|e| e.into_inner()) = Some(self.state.begin_batch());
    }

    /// Commit the current batch atomically. Clears the active batch.
    fn commit_batch(&self) -> Result<(), String> {
        let batch = self
            .batch
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| "No active batch to commit".to_string())?;
        self.state.commit_batch(batch)
    }

    /// Drop the current batch without committing (implicit rollback).
    fn rollback_batch(&self) {
        self.batch.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    /// Process a transaction
    pub fn process_transaction(&self, tx: &Transaction, _validator: &Pubkey) -> TxResult {
        self.process_transaction_inner(tx, _validator, None)
    }

    /// Process a transaction with optional pre-cached blockhashes (PERF-FIX 10).
    /// When called from process_transactions_parallel, the blockhash set is fetched ONCE
    /// for the entire batch instead of per-TX (avoids N × RocksDB reads).
    fn process_transaction_inner(
        &self,
        tx: &Transaction,
        _validator: &Pubkey,
        cached_blockhashes: Option<&HashSet<Hash>>,
    ) -> TxResult {
        // T1.7: Validate transaction structure (size limits)
        if let Err(e) = tx.validate_structure() {
            return TxResult {
                success: false,
                fee_paid: 0,
                error: Some(format!("Invalid transaction structure: {}", e)),
            };
        }

        // T1.3: Reject transactions with zero blockhash (no bypass)
        if tx.message.recent_blockhash == crate::hash::Hash::default() {
            return TxResult {
                success: false,
                fee_paid: 0,
                error: Some("Zero blockhash is not valid for replay protection".to_string()),
            };
        }

        // Reject replayed transactions
        let tx_hash = tx.hash();
        if let Ok(Some(_)) = self.state.get_transaction(&tx_hash) {
            return TxResult {
                success: false,
                fee_paid: 0,
                error: Some("Transaction already processed".to_string()),
            };
        }

        // Validate recent_blockhash for replay protection
        // PERF-FIX 10: Use cached blockhashes when available (from parallel batch)
        {
            let valid = if let Some(hashes) = cached_blockhashes {
                hashes.contains(&tx.message.recent_blockhash)
            } else {
                let recent = self.state.get_recent_blockhashes(300).unwrap_or_default();
                recent.contains(&tx.message.recent_blockhash)
            };
            if !valid {
                return TxResult {
                    success: false,
                    fee_paid: 0,
                    error: Some("Blockhash not found or too old".to_string()),
                };
            }
        }

        if is_evm_instruction(tx) {
            return self.process_evm_transaction(tx);
        }

        // 1. Verify signatures
        if tx.signatures.is_empty() {
            return TxResult {
                success: false,
                fee_paid: 0,
                error: Some("No signatures".to_string()),
            };
        }

        if tx.message.instructions.is_empty() {
            return TxResult {
                success: false,
                fee_paid: 0,
                error: Some("No instructions".to_string()),
            };
        }

        // Collect all unique signer accounts (first account of each instruction)
        let mut required_signers = HashSet::new();
        for ix in &tx.message.instructions {
            if let Some(first_acc) = ix.accounts.first() {
                required_signers.insert(*first_acc);
            } else {
                return TxResult {
                    success: false,
                    fee_paid: 0,
                    error: Some("Instruction has no accounts".to_string()),
                };
            }
        }

        // We need at least as many signatures as unique signers
        if tx.signatures.len() < required_signers.len() {
            return TxResult {
                success: false,
                fee_paid: 0,
                error: Some(format!(
                    "Insufficient signatures: got {}, need {}",
                    tx.signatures.len(),
                    required_signers.len()
                )),
            };
        }

        // Verify all signatures against the transaction message and build verified set
        let message_bytes = tx.message.serialize();
        use ed25519_dalek::{Verifier, VerifyingKey, Signature as EdSignature};
        let mut verified_signers: HashSet<Pubkey> = HashSet::new();

        // PERF-FIX 3: Pre-decompress all verifying keys once to avoid redundant
        // curve point decompression (VerifyingKey::from_bytes) per sig check.
        // Each decompression costs ~30µs; for N signers × M signatures this
        // reduces from N×M to just N decompressions.
        let mut vkeys: Vec<(Pubkey, VerifyingKey)> = Vec::with_capacity(required_signers.len());
        for signer in &required_signers {
            if let Ok(vk) = VerifyingKey::from_bytes(&signer.0) {
                vkeys.push((*signer, vk));
            }
        }

        // Fast path: single-sig TX (most common case) — skip inner loop entirely
        if tx.signatures.len() == 1 && vkeys.len() == 1 {
            let sig = EdSignature::from_bytes(&tx.signatures[0]);
            if vkeys[0].1.verify(&message_bytes, &sig).is_ok() {
                verified_signers.insert(vkeys[0].0);
            }
        } else {
            // Multi-sig: match signatures to pre-decompressed signers.
            // Each successful verify removes the signer, reducing work.
            let mut unmatched = vkeys;
            for sig_bytes in &tx.signatures {
                let sig = EdSignature::from_bytes(sig_bytes);
                let mut matched_idx = None;
                for (i, (pk, vk)) in unmatched.iter().enumerate() {
                    if vk.verify(&message_bytes, &sig).is_ok() {
                        verified_signers.insert(*pk);
                        matched_idx = Some(i);
                        break;
                    }
                }
                if let Some(idx) = matched_idx {
                    unmatched.swap_remove(idx);
                }
            }
        }

        // Ensure all required signers have a valid signature
        for signer in &required_signers {
            if !verified_signers.contains(signer) {
                return TxResult {
                    success: false,
                    fee_paid: 0,
                    error: Some(format!(
                        "Missing or invalid signature for account {}",
                        signer
                    )),
                };
            }
        }

        // Fee payer is the first account of the first instruction (must be verified)
        let fee_payer = tx.message.instructions[0].accounts[0];

        // 2. Charge fee (with reputation-based discount per whitepaper)
        let fee_config = self
            .state
            .get_fee_config()
            .unwrap_or_else(|_| FeeConfig::default_from_constants());
        let base_fee = Self::compute_transaction_fee(tx, &fee_config);
        // Apply reputation-based fee discount
        let payer_reputation = self.state.get_reputation(&fee_payer).unwrap_or(0);
        let total_fee = Self::apply_reputation_fee_discount(base_fee, payer_reputation);

        // M4 fix: charge fee BEFORE beginning the instruction batch.
        // This ensures fees are always collected even when instructions fail,
        // preventing free-compute DoS attacks via intentionally-failing TXs.
        if total_fee > 0 {
            if let Err(e) = self.charge_fee_direct(&fee_payer, total_fee) {
                return TxResult {
                    success: false,
                    fee_paid: 0,
                    error: Some(format!("Fee error: {}", e)),
                };
            }
        }

        // Begin atomic batch — all state mutations go through WriteBatch
        self.begin_batch();

        // 3. Apply rent for involved accounts
        if let Err(e) = self.apply_rent(tx) {
            self.rollback_batch();
            return TxResult {
                success: false,
                fee_paid: total_fee, // M4: fee already charged directly
                error: Some(format!("Rent error: {}", e)),
            };
        }

        // 4. Execute each instruction
        for instruction in &tx.message.instructions {
            if let Err(e) = self.execute_instruction(instruction) {
                self.rollback_batch();
                return TxResult {
                    success: false,
                    fee_paid: total_fee, // M4: fee already charged directly
                    error: Some(format!("Execution error: {}", e)),
                };
            }
        }

        if let Err(e) = self.b_put_transaction(tx) {
            self.rollback_batch();
            return TxResult {
                success: false,
                fee_paid: total_fee, // M4: fee already charged directly
                error: Some(format!("Transaction storage error: {}", e)),
            };
        }

        if let Err(e) = self.commit_batch() {
            self.rollback_batch();
            return TxResult {
                success: false,
                fee_paid: total_fee, // M4: fee already charged directly
                error: Some(format!("Atomic commit failed: {}", e)),
            };
        }

        TxResult {
            success: true,
            fee_paid: total_fee,
            error: None,
        }
    }

    /// Process multiple transactions in parallel where possible.
    /// Transactions are grouped by account access patterns using union-find:
    /// - Transactions touching disjoint account sets run in parallel (rayon)
    /// - Transactions touching overlapping accounts run sequentially within
    ///   the same group to preserve causal ordering.
    ///
    /// Each parallel group gets its own TxProcessor (sharing the same
    /// underlying RocksDB via Arc<DB>) so batches don't contend.
    pub fn process_transactions_parallel(
        &self,
        txs: &[Transaction],
        validator: &Pubkey,
    ) -> Vec<TxResult> {
        // PERF-FIX 10: Cache blockhashes ONCE for the entire batch
        let cached_blockhashes: HashSet<Hash> = self.state.get_recent_blockhashes(300).unwrap_or_default().into_iter().collect();

        if txs.len() <= 1 {
            return txs
                .iter()
                .map(|tx| self.process_transaction_inner(tx, validator, Some(&cached_blockhashes)))
                .collect();
        }

        let n = txs.len();

        // Phase 1: Identify account access sets for each transaction.
        // PERF-CRITICAL: Exclude the shared CONTRACT_PROGRAM_ID from conflict detection.
        // All contract calls use the same program_id (0xFF..FF) as a dispatch entry point,
        // but they operate on DIFFERENT contract addresses (ix.accounts[1]). Including program_id
        // in the conflict set would merge ALL contract TXs into one sequential group, defeating
        // rayon parallelism entirely. Only actual data accounts (caller, contract address) matter
        // for conflict detection — like Solana's Sealevel scheduler.
        let tx_accounts: Vec<HashSet<Pubkey>> = txs
            .iter()
            .map(|tx| {
                let mut accounts = HashSet::new();
                for ix in &tx.message.instructions {
                    // Do NOT add program_id to conflict set — it's shared infrastructure
                    // that dispatches to independent contract instances.
                    // Only the actual accounts (caller + contract address) determine conflicts.
                    if ix.program_id != CONTRACT_PROGRAM_ID {
                        accounts.insert(ix.program_id);
                    }
                    for key in &ix.accounts {
                        accounts.insert(*key);
                    }
                }
                accounts
            })
            .collect();

        // Phase 2: Build conflict graph using union-find.
        // Two transactions conflict if they share any account key.
        // Conflicting TXs are merged into the same group.
        let mut parent: Vec<usize> = (0..n).collect();

        fn uf_find(parent: &mut Vec<usize>, x: usize) -> usize {
            let mut r = x;
            while parent[r] != r {
                r = parent[r];
            }
            // Path compression
            let mut c = x;
            while c != r {
                let next = parent[c];
                parent[c] = r;
                c = next;
            }
            r
        }

        fn uf_union(parent: &mut Vec<usize>, a: usize, b: usize) {
            let ra = uf_find(parent, a);
            let rb = uf_find(parent, b);
            if ra != rb {
                parent[ra] = rb;
            }
        }

        // PERF-OPT 4: Build inverted index (account → tx indices) for O(total_accounts)
        // conflict detection instead of O(n²) pairwise disjoint checks.
        {
            let mut account_to_txs: std::collections::HashMap<Pubkey, Vec<usize>> =
                std::collections::HashMap::new();
            for (i, accounts) in tx_accounts.iter().enumerate() {
                for account in accounts {
                    account_to_txs.entry(*account).or_default().push(i);
                }
            }
            for tx_indices in account_to_txs.values() {
                for window in tx_indices.windows(2) {
                    uf_union(&mut parent, window[0], window[1]);
                }
            }
        }

        // Collect groups by root (preserving original order within each group)
        let mut group_map: std::collections::HashMap<usize, Vec<usize>> =
            std::collections::HashMap::new();
        for i in 0..n {
            let root = uf_find(&mut parent, i);
            group_map.entry(root).or_default().push(i);
        }
        let groups: Vec<Vec<usize>> = group_map.into_values().collect();

        // Phase 3: Execute groups in parallel with rayon.
        // Each group gets a fresh TxProcessor (own batch) backed by the same
        // StateStore (Arc<DB>). TXs within a group run sequentially because
        // they share accounts and may depend on each other's state changes.
        use rayon::prelude::*;

        // PERF-OPT 10: Initialize with None errors to avoid N heap allocations
        // for the "not processed" strings that will be overwritten anyway.
        let results_mu: std::sync::Mutex<Vec<TxResult>> = std::sync::Mutex::new(
            (0..n)
                .map(|_| TxResult {
                    success: false,
                    fee_paid: 0,
                    error: None,
                })
                .collect(),
        );

        groups.par_iter().for_each(|group| {
            // Each parallel group gets an independent TxProcessor so that
            // their StateBatch locks do not contend with each other.
            let group_proc = TxProcessor::new(self.state.clone());
            let mut group_results: Vec<(usize, TxResult)> = Vec::with_capacity(group.len());
            for &idx in group {
                let result = group_proc.process_transaction_inner(&txs[idx], validator, Some(&cached_blockhashes));
                group_results.push((idx, result));
            }
            let mut r = results_mu.lock().unwrap();
            for (idx, result) in group_results {
                r[idx] = result;
            }
        });

        results_mu.into_inner().unwrap()
    }

    /// Simulate a transaction (dry run) — validates everything without persisting.
    /// Returns the result with estimated fee, logs, and any errors.
    pub fn simulate_transaction(&self, tx: &Transaction) -> SimulationResult {
        let mut logs = Vec::new();

        // Validate blockhash
        {
            let recent = self.state.get_recent_blockhashes(300).unwrap_or_default();
            if !recent.contains(&tx.message.recent_blockhash) {
                return SimulationResult {
                    success: false,
                    fee: 0,
                    logs,
                    error: Some("Blockhash not found or too old".to_string()),
                    compute_used: 0,
                    return_data: None,
                };
            }
        }

        if tx.signatures.is_empty() || tx.message.instructions.is_empty() {
            return SimulationResult {
                success: false,
                fee: 0,
                logs,
                error: Some("Missing signatures or instructions".to_string()),
                compute_used: 0,
                return_data: None,
            };
        }

        // Verify signatures
        let mut required_signers = HashSet::new();
        for ix in &tx.message.instructions {
            if let Some(first_acc) = ix.accounts.first() {
                required_signers.insert(*first_acc);
            }
        }

        let message_bytes = tx.message.serialize();
        use crate::account::Keypair;
        let mut verified_signers: HashSet<Pubkey> = HashSet::new();
        for sig in &tx.signatures {
            for signer in &required_signers {
                if !verified_signers.contains(signer)
                    && Keypair::verify(signer, &message_bytes, sig)
                {
                    verified_signers.insert(*signer);
                    break;
                }
            }
        }
        for signer in &required_signers {
            if !verified_signers.contains(signer) {
                return SimulationResult {
                    success: false,
                    fee: 0,
                    logs,
                    error: Some(format!("Missing or invalid signature for {}", signer)),
                    compute_used: 0,
                    return_data: None,
                };
            }
        }

        // Compute fee (T2.12 fix: include reputation discount, same as process_transaction)
        let fee_config = self
            .state
            .get_fee_config()
            .unwrap_or_else(|_| FeeConfig::default_from_constants());
        let base_fee = Self::compute_transaction_fee(tx, &fee_config);
        let fee_payer = tx.message.instructions[0].accounts[0];
        let payer_reputation = self.state.get_reputation(&fee_payer).unwrap_or(0);
        let total_fee = Self::apply_reputation_fee_discount(base_fee, payer_reputation);

        // Check fee payer balance
        let balance = self.state.get_balance(&fee_payer).unwrap_or(0);
        if balance < total_fee {
            return SimulationResult {
                success: false,
                fee: total_fee,
                logs,
                error: Some(format!(
                    "Insufficient balance for fee: need {} have {}",
                    total_fee, balance
                )),
                compute_used: 0,
                return_data: None,
            };
        }
        logs.push(format!("Fee estimate: {} shells", total_fee));

        // Simulate each instruction (read-only)
        let mut total_compute = 0u64;
        let mut last_return_data: Option<Vec<u8>> = None;

        for (idx, instruction) in tx.message.instructions.iter().enumerate() {
            if instruction.program_id == CONTRACT_PROGRAM_ID {
                // Contract calls: do a dry-run execution
                if let Ok(contract_ix) = ContractInstruction::deserialize(&instruction.data) {
                    match contract_ix {
                        ContractInstruction::Call {
                            function,
                            args,
                            value,
                        } => {
                            if instruction.accounts.len() >= 2 {
                                let caller = &instruction.accounts[0];
                                let contract_addr = &instruction.accounts[1];

                                match self.state.get_account(contract_addr) {
                                    Ok(Some(account)) if account.executable => {
                                        if let Ok(contract) =
                                            serde_json::from_slice::<ContractAccount>(&account.data)
                                        {
                                            let current_slot =
                                                self.state.get_last_slot().unwrap_or(0);
                                            let context = ContractContext::with_args(
                                                *caller,
                                                *contract_addr,
                                                value,
                                                current_slot,
                                                contract.storage.clone(),
                                                args.clone(),
                                            );
                                            let mut runtime = ContractRuntime::get_pooled();
                                            let exec_result = runtime
                                                .execute(&contract, &function, &args, context);
                                            runtime.return_to_pool();
                                            match exec_result
                                            {
                                                Ok(result) => {
                                                    total_compute += result.compute_used;
                                                    for log in &result.logs {
                                                        logs.push(format!("[ix{}] {}", idx, log));
                                                    }
                                                    if !result.return_data.is_empty() {
                                                        last_return_data =
                                                            Some(result.return_data.clone());
                                                    }
                                                    if !result.success {
                                                        return SimulationResult {
                                                            success: false,
                                                            fee: total_fee,
                                                            logs,
                                                            error: result.error,
                                                            compute_used: total_compute,
                                                            return_data: last_return_data,
                                                        };
                                                    }
                                                    logs.push(format!(
                                                        "[ix{}] Contract call '{}' OK, compute: {}",
                                                        idx, function, result.compute_used
                                                    ));
                                                }
                                                Err(e) => {
                                                    return SimulationResult {
                                                        success: false,
                                                        fee: total_fee,
                                                        logs,
                                                        error: Some(format!(
                                                            "Contract execution error: {}",
                                                            e
                                                        )),
                                                        compute_used: total_compute,
                                                        return_data: last_return_data,
                                                    };
                                                }
                                            }
                                        }
                                    }
                                    Ok(Some(_)) => {
                                        logs.push(format!("[ix{}] Account is not executable", idx));
                                    }
                                    _ => {
                                        logs.push(format!("[ix{}] Contract not found", idx));
                                    }
                                }
                            }
                        }
                        ContractInstruction::Deploy { .. } => {
                            logs.push(format!(
                                "[ix{}] Deploy instruction (would deploy contract)",
                                idx
                            ));
                        }
                        ContractInstruction::Upgrade { .. } => {
                            logs.push(format!(
                                "[ix{}] Upgrade instruction (would upgrade contract)",
                                idx
                            ));
                        }
                        ContractInstruction::Close => {
                            logs.push(format!(
                                "[ix{}] Close instruction (would close contract)",
                                idx
                            ));
                        }
                    }
                }
            } else if instruction.program_id == SYSTEM_PROGRAM_ID {
                logs.push(format!("[ix{}] System instruction", idx));
            } else if instruction.program_id == EVM_PROGRAM_ID {
                logs.push(format!(
                    "[ix{}] EVM instruction (use eth_call for simulation)",
                    idx
                ));
            } else {
                logs.push(format!(
                    "[ix{}] Unknown program: {}",
                    idx, instruction.program_id
                ));
            }
        }

        SimulationResult {
            success: true,
            fee: total_fee,
            logs,
            error: None,
            compute_used: total_compute,
            return_data: last_return_data,
        }
    }

    /// Process an EVM transaction.
    ///
    /// H3 fix: EVM state changes are now deferred — `execute_evm_transaction`
    /// uses `transact()` (not `transact_commit`) and returns the state changes.
    /// All writes (EVM accounts, storage, native balances, tx/receipt records,
    /// fees) go through a single `StateBatch` and commit atomically.
    fn process_evm_transaction(&self, tx: &Transaction) -> TxResult {
        if tx.message.instructions.len() != 1 {
            return TxResult {
                success: false,
                fee_paid: 0,
                error: Some("Invalid EVM transaction format".to_string()),
            };
        }

        let instruction = &tx.message.instructions[0];
        let raw = &instruction.data;

        let evm_tx = match decode_evm_transaction(raw) {
            Ok(tx) => tx,
            Err(err) => {
                return TxResult {
                    success: false,
                    fee_paid: 0,
                    error: Some(err),
                }
            }
        };

        if !u256_is_multiple_of_shell(&evm_tx.value) {
            return TxResult {
                success: false,
                fee_paid: 0,
                error: Some("EVM value must be multiple of 1e9 wei".to_string()),
            };
        }

        let from_address: [u8; 20] = evm_tx.from.into();
        let mapping = match self.state.lookup_evm_address(&from_address) {
            Ok(value) => value,
            Err(err) => {
                return TxResult {
                    success: false,
                    fee_paid: 0,
                    error: Some(err),
                }
            }
        };

        if mapping.is_none() {
            return TxResult {
                success: false,
                fee_paid: 0,
                error: Some("EVM address not registered".to_string()),
            };
        }

        let chain_id = evm_tx.chain_id.unwrap_or(0);
        let (result, evm_state_changes) =
            match execute_evm_transaction(self.state.clone(), &evm_tx, chain_id) {
                Ok(res) => res,
                Err(err) => {
                    return TxResult {
                        success: false,
                        fee_paid: 0,
                        error: Some(err),
                    }
                }
            };

        let evm_hash: [u8; 32] = evm_tx.hash.into();
        let native_hash = tx.hash().0;

        let record = EvmTxRecord {
            evm_hash,
            native_hash,
            from: from_address,
            to: evm_tx.to.map(|addr| addr.into()),
            value: evm_tx.value.to_be_bytes(),
            gas_limit: evm_tx.gas_limit,
            gas_price: evm_tx.gas_price.to_be_bytes(),
            nonce: evm_tx.nonce,
            data: evm_tx.data.to_vec(),
            status: Some(result.success),
            gas_used: Some(result.gas_used),
            block_slot: None,
            block_hash: None,
        };

        let receipt = EvmReceipt {
            evm_hash,
            status: result.success,
            gas_used: result.gas_used,
            block_slot: None,
            block_hash: None,
            contract_address: result.created_address,
            logs: result.logs.clone(),
        };

        // AUDIT-FIX 0.7: Charge EVM fee BEFORE the batch, so rollback can't erase it.
        // This prevents free-compute DoS via intentionally-failing EVM transactions.
        let fee_paid = u256_to_shells(&(evm_tx.gas_price * U256::from(result.gas_used)));
        if fee_paid > 0 {
            let native_payer = mapping.unwrap(); // guaranteed Some from earlier check
            if let Err(e) = self.charge_fee_direct(&native_payer, fee_paid) {
                return TxResult {
                    success: false,
                    fee_paid: 0,
                    error: Some(format!("EVM fee charge error: {}", e)),
                };
            }
        }

        // Begin atomic batch for EVM state writes
        self.begin_batch();

        if let Err(e) = self.b_put_evm_tx(&record) {
            self.rollback_batch();
            return TxResult {
                success: false,
                fee_paid,
                error: Some(format!("EVM tx storage error: {}", e)),
            };
        }
        if let Err(e) = self.b_put_evm_receipt(&receipt) {
            self.rollback_batch();
            return TxResult {
                success: false,
                fee_paid,
                error: Some(format!("EVM receipt storage error: {}", e)),
            };
        }

        if let Err(e) = self.b_put_transaction(tx) {
            self.rollback_batch();
            return TxResult {
                success: false,
                fee_paid,
                error: Some(format!("Transaction storage error: {}", e)),
            };
        }

        // Fee already charged via charge_fee_direct before batch (AUDIT-FIX 0.7)

        // H3 fix: Apply deferred EVM state changes (accounts, storage, native balances)
        // through the same atomic batch. This guarantees all-or-nothing commit.
        if let Err(e) = self.b_apply_evm_state_changes(&evm_state_changes) {
            self.rollback_batch();
            return TxResult {
                success: false,
                fee_paid,
                error: Some(format!("EVM state apply error: {}", e)),
            };
        }

        if let Err(e) = self.commit_batch() {
            self.rollback_batch();
            return TxResult {
                success: false,
                fee_paid,
                error: Some(format!("Atomic commit failed: {}", e)),
            };
        }

        TxResult {
            success: result.success,
            fee_paid,
            error: if result.success {
                None
            } else {
                Some("EVM execution reverted".to_string())
            },
        }
    }

    /// Charge transaction fee from spendable balance only (not staked/locked)
    /// Fee is split per FeeConfig: burn / producer / voters / treasury percentages.
    /// NOTE: Currently unused — fee charging goes through charge_fee_direct() which
    /// handles the split at the block level. Retained for potential future use with
    /// batch-scoped fee splitting.
    #[allow(dead_code)]
    fn charge_fee(&self, payer: &Pubkey, fee: u64) -> Result<(), String> {
        let mut payer_account = self
            .b_get_account(payer)?
            .ok_or_else(|| "Payer account not found".to_string())?;

        // T1.1 fix: Deduct from spendable balance, not total shells.
        // This prevents spending staked or locked funds on fees.
        payer_account.deduct_spendable(fee)?;
        self.b_put_account(payer, &payer_account)?;

        // Split fee according to configured percentages
        let fee_config = self
            .state
            .get_fee_config()
            .unwrap_or_else(|_| FeeConfig::default_from_constants());

        let burn_amount = fee * fee_config.fee_burn_percent / 100;
        let producer_amount = fee * fee_config.fee_producer_percent / 100;
        let voters_amount = fee * fee_config.fee_voters_percent / 100;
        // AUDIT-FIX 0.8: Use saturating_sub to prevent underflow if percentages exceed 100
        let allocated = burn_amount.saturating_add(producer_amount).saturating_add(voters_amount);
        let treasury_amount = fee.saturating_sub(allocated);

        // Burn portion: permanently remove from circulation (via batch — atomic)
        if burn_amount > 0 {
            self.b_add_burned(burn_amount)?;
        }

        // Producer and voters portions go to treasury for now
        // (block producer/voter identities are not available in this scope;
        //  validator/src/main.rs distribute_fees handles the actual split at block level)
        let total_to_treasury = treasury_amount + producer_amount + voters_amount;

        if total_to_treasury > 0 {
            let treasury_pubkey = self
                .state
                .get_treasury_pubkey()?
                .ok_or_else(|| "Treasury pubkey not set".to_string())?;
            let mut treasury_account = self
                .b_get_account(&treasury_pubkey)?
                .unwrap_or_else(|| Account::new(0, treasury_pubkey));
            treasury_account.add_spendable(total_to_treasury)?;
            self.b_put_account(&treasury_pubkey, &treasury_account)?;
        }

        Ok(())
    }

    /// M4 fix: charge fee directly to state (not through batch), so it persists
    /// even if the instruction batch is later rolled back. This prevents
    /// free-compute DoS via intentionally-failing transactions.
    fn charge_fee_direct(&self, payer: &Pubkey, fee: u64) -> Result<(), String> {
        let mut payer_account = self
            .state
            .get_account(payer)?
            .ok_or_else(|| "Payer account not found".to_string())?;

        payer_account.deduct_spendable(fee)?;
        self.state.put_account(payer, &payer_account)?;

        // Split fee according to configured percentages
        let fee_config = self
            .state
            .get_fee_config()
            .unwrap_or_else(|_| FeeConfig::default_from_constants());

        let burn_amount = fee * fee_config.fee_burn_percent / 100;
        let producer_amount = fee * fee_config.fee_producer_percent / 100;
        let voters_amount = fee * fee_config.fee_voters_percent / 100;
        // AUDIT-FIX 0.8: Use saturating_sub to prevent underflow if percentages exceed 100
        let allocated = burn_amount.saturating_add(producer_amount).saturating_add(voters_amount);
        let treasury_amount = fee.saturating_sub(allocated);

        if burn_amount > 0 {
            self.state.add_burned(burn_amount)?;
        }

        let total_to_treasury = treasury_amount + producer_amount + voters_amount;
        if total_to_treasury > 0 {
            let treasury_pubkey = self
                .state
                .get_treasury_pubkey()?
                .ok_or_else(|| "Treasury pubkey not set".to_string())?;
            let mut treasury_account = self
                .state
                .get_account(&treasury_pubkey)?
                .unwrap_or_else(|| Account::new(0, treasury_pubkey));
            treasury_account.add_spendable(total_to_treasury)?;
            self.state
                .put_account(&treasury_pubkey, &treasury_account)?;
        }

        Ok(())
    }

    /// Execute a single instruction
    fn execute_instruction(&self, ix: &Instruction) -> Result<(), String> {
        if ix.program_id == SYSTEM_PROGRAM_ID {
            self.execute_system_program(ix)
        } else if ix.program_id == CONTRACT_PROGRAM_ID {
            self.execute_contract_program(ix)
        } else {
            Err(format!("Unknown program: {}", ix.program_id))
        }
    }

    /// Execute system program instruction
    fn execute_system_program(&self, ix: &Instruction) -> Result<(), String> {
        if ix.data.is_empty() {
            return Err("Empty instruction data".to_string());
        }

        let instruction_type = ix.data[0];
        match instruction_type {
            0 => self.system_transfer(ix),
            // H1 fix: types 2-5 are fee-free internal txs — verify sender is treasury
            2..=5 => {
                if let Some(sender) = ix.accounts.first() {
                    let is_treasury = self
                        .state
                        .get_treasury_pubkey()
                        .ok()
                        .flatten()
                        .map(|t| t == *sender)
                        .unwrap_or(false);
                    if !is_treasury {
                        return Err(format!(
                            "Instruction type {} restricted to treasury account",
                            instruction_type
                        ));
                    }
                }
                self.system_transfer(ix)
            }
            1 => self.system_create_account(ix),
            6 => self.system_create_collection(ix),
            7 => self.system_mint_nft(ix),
            8 => self.system_transfer_nft(ix),
            9 => self.system_stake(ix),
            10 => self.system_request_unstake(ix),
            11 => self.system_claim_unstake(ix),
            12 => self.system_register_evm_address(ix),
            13 => self.system_reefstake_deposit(ix),
            14 => self.system_reefstake_unstake(ix),
            15 => self.system_reefstake_claim(ix),
            16 => self.system_reefstake_transfer(ix),
            // H16 fix: consensus-safe instruction types for state-mutating operations
            17 => self.system_deploy_contract(ix),
            18 => self.system_set_contract_abi(ix),
            19 => self.system_faucet_airdrop(ix),
            _ => Err(format!("Unknown system instruction: {}", instruction_type)),
        }
    }

    /// System program: Transfer shells
    fn system_transfer(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Transfer requires 2 accounts".to_string());
        }

        if ix.data.len() < 9 {
            return Err("Invalid transfer data".to_string());
        }

        let from = &ix.accounts[0];
        let to = &ix.accounts[1];

        let amount_bytes: [u8; 8] = ix.data[1..9]
            .try_into()
            .map_err(|_| "Invalid amount encoding".to_string())?;
        let amount = u64::from_le_bytes(amount_bytes);

        self.b_transfer(from, to, amount)
    }

    /// System program: Create account
    fn system_create_account(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("Create account requires at least 1 account".to_string());
        }

        let pubkey = &ix.accounts[0];
        if self.b_get_account(pubkey)?.is_some() {
            return Err("Account already exists".to_string());
        }

        let account = Account::new(0, *pubkey);
        self.b_put_account(pubkey, &account)?;

        Ok(())
    }

    /// System program: Register EVM address mapping
    fn system_register_evm_address(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("Register EVM address requires signer account".to_string());
        }

        if ix.data.len() != 21 {
            return Err("Invalid EVM address data".to_string());
        }

        let mut evm_address = [0u8; 20];
        evm_address.copy_from_slice(&ix.data[1..21]);

        let native_pubkey = ix.accounts[0];
        if let Some(existing) = self.state.lookup_evm_address(&evm_address)? {
            if existing != native_pubkey {
                return Err("EVM address already mapped".to_string());
            }
            return Ok(());
        }

        self.b_register_evm_address(&evm_address, &native_pubkey)
    }

    /// System program: Create NFT collection
    fn system_create_collection(&self, ix: &Instruction) -> Result<(), String> {
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
    fn system_mint_nft(&self, ix: &Instruction) -> Result<(), String> {
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
        // M6 fix: index token_id through batch for atomicity (was direct state write before)
        if let Err(e) =
            self.b_index_nft_token_id(&collection_account, mint_data.token_id, &token_account)
        {
            eprintln!("Warning: failed to index NFT token_id: {}", e);
        }

        Ok(())
    }

    /// System program: Transfer NFT
    fn system_transfer_nft(&self, ix: &Instruction) -> Result<(), String> {
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

    /// System program: Stake MOLT
    fn system_stake(&self, ix: &Instruction) -> Result<(), String> {
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
        // AUDIT-FIX 3.11: Verify validator exists in the stake pool before
        // allowing new stake. Prevents users from locking funds to arbitrary
        // non-validator pubkeys where rewards will never be earned.
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
    fn system_request_unstake(&self, ix: &Instruction) -> Result<(), String> {
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

    /// System program: Claim unstaked MOLT
    fn system_claim_unstake(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Claim unstake requires staker and validator accounts".to_string());
        }

        // T2.6 fix: account layout is [staker, validator] — same order as
        // request_unstake — so the staker (accounts[0]) is the signer and
        // can claim their own funds without the validator's signature.
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
    // REEFSTAKE — Liquid Staking (T6.1: wired to processor)
    // ========================================================================

    /// System program: ReefStake deposit (instruction type 13)
    /// data: [13, amount(8)]
    /// accounts: [depositor]
    fn system_reefstake_deposit(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("ReefStake deposit requires depositor account".to_string());
        }
        if ix.data.len() < 9 {
            return Err("Invalid ReefStake deposit data".to_string());
        }

        let depositor = ix.accounts[0];
        let amount_bytes: [u8; 8] = ix.data[1..9]
            .try_into()
            .map_err(|_| "Invalid amount encoding".to_string())?;
        let amount = u64::from_le_bytes(amount_bytes);

        if amount == 0 {
            return Err("Cannot deposit 0 MOLT".to_string());
        }

        // Deduct from depositor's spendable balance
        let mut account = self
            .b_get_account(&depositor)?
            .ok_or_else(|| "Depositor account not found".to_string())?;
        account.deduct_spendable(amount)?;
        self.b_put_account(&depositor, &account)?;

        // Stake into ReefStake pool and mint stMOLT
        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut pool = self.b_get_reefstake_pool()?;
        let _st_molt = pool.stake(depositor, amount, current_slot)?;
        self.b_put_reefstake_pool(&pool)?;

        Ok(())
    }

    /// System program: ReefStake request unstake (instruction type 14)
    /// data: [14, st_molt_amount(8)]
    /// accounts: [user]
    fn system_reefstake_unstake(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("ReefStake unstake requires user account".to_string());
        }
        if ix.data.len() < 9 {
            return Err("Invalid ReefStake unstake data".to_string());
        }

        let user = ix.accounts[0];
        let amount_bytes: [u8; 8] = ix.data[1..9]
            .try_into()
            .map_err(|_| "Invalid amount encoding".to_string())?;
        let st_molt_amount = u64::from_le_bytes(amount_bytes);

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut pool = self.b_get_reefstake_pool()?;
        let _request = pool.request_unstake(user, st_molt_amount, current_slot)?;
        self.b_put_reefstake_pool(&pool)?;

        Ok(())
    }

    /// System program: ReefStake claim (instruction type 15)
    /// data: [15]
    /// accounts: [user]
    fn system_reefstake_claim(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.is_empty() {
            return Err("ReefStake claim requires user account".to_string());
        }

        let user = ix.accounts[0];
        let current_slot = self.b_get_last_slot().unwrap_or(0);

        let mut pool = self.b_get_reefstake_pool()?;
        let molt_claimed = pool.claim_unstake(user, current_slot)?;
        self.b_put_reefstake_pool(&pool)?;

        if molt_claimed == 0 {
            return Err("No claimable MOLT (cooldown not complete)".to_string());
        }

        // Credit the MOLT back to user's spendable balance
        let mut account = self
            .b_get_account(&user)?
            .ok_or_else(|| "User account not found".to_string())?;
        account.add_spendable(molt_claimed)?;
        self.b_put_account(&user, &account)?;

        Ok(())
    }

    /// System program: ReefStake stMOLT transfer (instruction type 16)
    /// data: [16, st_molt_amount(8)]
    /// accounts: [from, to]
    fn system_reefstake_transfer(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("ReefStake transfer requires sender and receiver accounts".to_string());
        }
        if ix.data.len() < 9 {
            return Err("Invalid ReefStake transfer data".to_string());
        }

        let from = ix.accounts[0];
        let to = ix.accounts[1];
        let amount_bytes: [u8; 8] = ix.data[1..9]
            .try_into()
            .map_err(|_| "Invalid amount encoding".to_string())?;
        let st_molt_amount = u64::from_le_bytes(amount_bytes);

        // Ensure receiver account exists on-chain (create if needed)
        if self.b_get_account(&to)?.is_none() {
            self.b_put_account(&to, &crate::Account::new(0, SYSTEM_PROGRAM_ID))?;
        }

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let mut pool = self.b_get_reefstake_pool()?;
        pool.transfer(from, to, st_molt_amount, current_slot)?;
        self.b_put_reefstake_pool(&pool)?;

        Ok(())
    }

    /// H16 fix: Deploy contract through consensus (instruction type 17).
    /// Instruction data: [17 | code_length(4 LE) | code_bytes | init_data_bytes]
    /// Accounts: [deployer, treasury]
    /// The deployer must be a transaction signer. Deploy fee charged from deployer.
    fn system_deploy_contract(&self, ix: &Instruction) -> Result<(), String> {
        use sha2::{Digest, Sha256};

        if ix.accounts.len() < 2 {
            return Err("DeployContract requires [deployer, treasury] accounts".to_string());
        }
        if ix.data.len() < 6 {
            return Err("DeployContract instruction data too short".to_string());
        }

        let deployer = ix.accounts[0];
        let treasury = ix.accounts[1];

        // Parse code length and code bytes
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

        // Verify treasury is correct
        let actual_treasury = self
            .state
            .get_treasury_pubkey()?
            .ok_or_else(|| "Treasury pubkey not set".to_string())?;
        if treasury != actual_treasury {
            return Err("DeployContract: incorrect treasury account".to_string());
        }

        // Derive program address: SHA-256(deployer + optional_name + code)
        let contract_name: Option<String> = if !init_data_bytes.is_empty() {
            serde_json::from_slice::<serde_json::Value>(init_data_bytes)
                .ok()
                .and_then(|v| {
                    v.get("name")
                        .or_else(|| v.get("symbol"))
                        .and_then(|n| n.as_str().map(|s| s.to_string()))
                })
        } else {
            None
        };

        let mut addr_hasher = Sha256::new();
        addr_hasher.update(deployer.0);
        if let Some(ref name) = contract_name {
            addr_hasher.update(name.as_bytes());
        }
        addr_hasher.update(code_bytes);
        let addr_hash = addr_hasher.finalize();
        let mut addr_bytes = [0u8; 32];
        addr_bytes.copy_from_slice(&addr_hash[..32]);
        let program_pubkey = crate::Pubkey(addr_bytes);

        // Reject if already deployed
        if self.b_get_account(&program_pubkey)?.is_some() {
            return Err(format!(
                "Contract already exists at {}",
                program_pubkey.to_base58()
            ));
        }

        // Charge deploy fee
        // AUDIT-FIX 3.8: Use fee_config from state instead of hardcoded constant
        // so governance-updated fees take effect.
        let deploy_fee_config = self
            .state
            .get_fee_config()
            .unwrap_or_else(|_| FeeConfig::default_from_constants());
        let deploy_fee = deploy_fee_config.contract_deploy_fee;
        let mut deployer_account = self
            .b_get_account(&deployer)?
            .ok_or_else(|| "Deployer account not found".to_string())?;
        deployer_account
            .deduct_spendable(deploy_fee)
            .map_err(|e| format!("Failed to deduct deploy fee: {}", e))?;
        self.b_put_account(&deployer, &deployer_account)?;

        // Credit fee to treasury
        let mut treasury_account = self
            .b_get_account(&treasury)?
            .unwrap_or_else(|| crate::Account::new(0, treasury));
        treasury_account
            .add_spendable(deploy_fee)
            .map_err(|e| format!("Treasury balance overflow: {}", e))?;
        self.b_put_account(&treasury, &treasury_account)?;

        // Create contract account
        let contract = crate::ContractAccount::new(code_bytes.to_vec(), deployer);
        let mut account = crate::Account::new(0, program_pubkey);
        account.data = serde_json::to_vec(&contract)
            .map_err(|e| format!("Failed to serialize contract: {}", e))?;
        account.executable = true;
        self.b_put_account(&program_pubkey, &account)?;

        // Index program
        // AUDIT-FIX 2.5: Route through batch to prevent phantom entries on rollback
        if let Err(e) = self.b_index_program(&program_pubkey) {
            eprintln!("system_deploy_contract: index_program failed: {}", e);
        }

        // Process init_data for symbol registry
        if !init_data_bytes.is_empty() {
            if let Ok(raw) = std::str::from_utf8(init_data_bytes) {
                if let Ok(registry_data) = serde_json::from_str::<serde_json::Value>(raw) {
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
                        };
                        // AUDIT-FIX 2.5: Route through batch
                        if let Err(e) = self.b_register_symbol(symbol, entry) {
                            eprintln!("system_deploy_contract: register_symbol failed: {}", e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// H16 fix: Set contract ABI through consensus (instruction type 18).
    /// Instruction data: [18 | abi_json_bytes]
    /// Accounts: [contract_owner, contract_id]
    /// Only the contract owner/deployer can set the ABI.
    fn system_set_contract_abi(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("SetContractAbi requires [owner, contract_id] accounts".to_string());
        }
        if ix.data.len() < 2 {
            return Err("SetContractAbi: missing ABI data".to_string());
        }

        let owner = ix.accounts[0];
        let contract_id = ix.accounts[1];
        let abi_bytes = &ix.data[1..];

        let abi: crate::ContractAbi =
            serde_json::from_slice(abi_bytes).map_err(|e| format!("Invalid ABI format: {}", e))?;

        let mut account = self
            .b_get_account(&contract_id)?
            .ok_or_else(|| "Contract not found".to_string())?;
        if !account.executable {
            return Err("Account is not a contract".to_string());
        }

        let mut contract: crate::ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to decode contract: {}", e))?;

        // Verify caller is the contract deployer/owner
        if contract.owner != owner {
            return Err(format!(
                "Only the contract deployer ({}) can set the ABI",
                contract.owner.to_base58()
            ));
        }

        contract.abi = Some(abi);
        account.data = serde_json::to_vec(&contract)
            .map_err(|e| format!("Failed to serialize contract: {}", e))?;
        self.b_put_account(&contract_id, &account)?;

        Ok(())
    }

    /// H16 fix: Faucet airdrop through consensus (instruction type 19).
    /// Instruction data: [19 | amount_shells(8 LE)]
    /// Accounts: [treasury, recipient]
    /// Treasury must be a signer. Amount capped at 100 MOLT.
    fn system_faucet_airdrop(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("FaucetAirdrop requires [treasury, recipient] accounts".to_string());
        }
        if ix.data.len() < 9 {
            return Err("FaucetAirdrop: missing amount data".to_string());
        }

        let treasury = ix.accounts[0];
        let recipient = ix.accounts[1];

        // Verify sender is treasury
        let actual_treasury = self
            .state
            .get_treasury_pubkey()?
            .ok_or_else(|| "Treasury pubkey not set".to_string())?;
        if treasury != actual_treasury {
            return Err("FaucetAirdrop: sender must be treasury".to_string());
        }

        let amount_shells = u64::from_le_bytes(
            ix.data[1..9]
                .try_into()
                .map_err(|_| "Invalid amount encoding".to_string())?,
        );

        // Cap at 100 MOLT
        let max_airdrop = 100u64 * 1_000_000_000;
        if amount_shells == 0 || amount_shells > max_airdrop {
            return Err(format!(
                "FaucetAirdrop: amount must be between 1 shell and {} shells (100 MOLT)",
                max_airdrop
            ));
        }

        // Debit treasury
        let mut treasury_account = self
            .b_get_account(&treasury)?
            .ok_or_else(|| "Treasury account not found".to_string())?;
        treasury_account
            .deduct_spendable(amount_shells)
            .map_err(|e| format!("Insufficient treasury balance: {}", e))?;
        self.b_put_account(&treasury, &treasury_account)?;

        // Credit recipient
        let mut recipient_account = self
            .b_get_account(&recipient)?
            .unwrap_or_else(|| crate::Account::new(0, SYSTEM_PROGRAM_ID));
        recipient_account
            .add_spendable(amount_shells)
            .map_err(|e| format!("Recipient balance overflow: {}", e))?;
        self.b_put_account(&recipient, &recipient_account)?;

        Ok(())
    }

    /// Execute smart contract program instruction
    fn execute_contract_program(&self, ix: &Instruction) -> Result<(), String> {
        let contract_ix = ContractInstruction::deserialize(&ix.data)?;

        match contract_ix {
            ContractInstruction::Deploy { code, init_data } => {
                self.contract_deploy(ix, code, init_data)
            }
            ContractInstruction::Call {
                function,
                args,
                value,
            } => self.contract_call(ix, function, args, value),
            ContractInstruction::Upgrade { code } => self.contract_upgrade(ix, code),
            ContractInstruction::Close => self.contract_close(ix),
        }
    }

    /// Deploy smart contract
    fn contract_deploy(
        &self,
        ix: &Instruction,
        code: Vec<u8>,
        init_data: Vec<u8>,
    ) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Deploy requires deployer and contract accounts".to_string());
        }

        let deployer = &ix.accounts[0];
        let contract_address = &ix.accounts[1];

        if self.b_get_account(contract_address)?.is_some() {
            return Err("Contract account already exists".to_string());
        }

        let mut runtime = ContractRuntime::get_pooled();
        let deploy_result = runtime.deploy(&code);
        runtime.return_to_pool();
        deploy_result?;

        let mut owner = *deployer;
        let mut make_public = true;
        let mut deployer_abi: Option<ContractAbi> = None;

        if let Some(registry) = DeployRegistryData::from_init_data(&init_data) {
            if let Some(raw_owner) = registry.upgrade_authority.clone() {
                if raw_owner == "none" {
                    owner = SYSTEM_PROGRAM_ID;
                } else if let Ok(custom_owner) = Pubkey::from_base58(&raw_owner) {
                    owner = custom_owner;
                }
            }

            if let Some(flag) = registry.make_public {
                make_public = flag;
            }

            deployer_abi = registry.abi.clone();

            if let Some(symbol) = registry.symbol.clone() {
                let entry = SymbolRegistryEntry {
                    symbol,
                    program: *contract_address,
                    owner,
                    name: registry.name.clone(),
                    template: registry.template.clone(),
                    metadata: registry.metadata.clone(),
                };
                self.b_register_symbol(&entry.symbol.clone(), entry)?;
            }
        }

        let mut contract = ContractAccount::new(code, owner);

        // If the deployer supplied an explicit ABI, use it (overrides auto-extracted)
        if let Some(abi) = deployer_abi {
            contract.abi = Some(abi);
        }

        let mut account = Account::new(0, *contract_address);
        account.data = serde_json::to_vec(&contract)
            .map_err(|e| format!("Failed to serialize contract: {}", e))?;
        account.executable = true;

        self.b_put_account(contract_address, &account)?;
        if make_public {
            self.b_index_program(contract_address)?;
        }

        Ok(())
    }

    /// Call smart contract function
    fn contract_call(
        &self,
        ix: &Instruction,
        function: String,
        args: Vec<u8>,
        value: u64,
    ) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Call requires caller and contract accounts".to_string());
        }

        let caller = &ix.accounts[0];
        let contract_address = &ix.accounts[1];

        let account = self
            .b_get_account(contract_address)?
            .ok_or("Contract not found")?;

        if !account.executable {
            return Err("Account is not a contract".to_string());
        }

        let mut contract: ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to deserialize contract: {}", e))?;

        if value > 0 {
            self.b_transfer(caller, contract_address, value)?;
        }

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        let context = ContractContext::with_args(
            *caller,
            *contract_address,
            value,
            current_slot,
            contract.storage.clone(),
            args,
        );

        let mut runtime = ContractRuntime::get_pooled();
        let result = runtime.execute(&contract, &function, &context.args.clone(), context)?;

        // Return runtime to thread-local pool for reuse
        runtime.return_to_pool();

        if !result.success {
            return Err(result
                .error
                .unwrap_or("Contract execution failed".to_string()));
        }

        // Store contract events
        for event in &result.events {
            self.b_put_contract_event(contract_address, event)?;
        }

        // Apply storage changes from execution back to contract account
        if !result.storage_changes.is_empty() {
            for (key, value_opt) in &result.storage_changes {
                match value_opt {
                    Some(val) => {
                        contract.set_storage(key.clone(), val.clone());
                        // Also write to CF_CONTRACT_STORAGE for fast-path reads
                        self.b_put_contract_storage(contract_address, key, val)?;
                    }
                    None => {
                        contract.remove_storage(key);
                        // Also remove from CF_CONTRACT_STORAGE
                        self.b_delete_contract_storage(contract_address, key)?;
                    }
                }
            }
            // Persist updated contract
            let mut account = self
                .b_get_account(contract_address)?
                .ok_or("Contract not found after execution")?;
            account.data = serde_json::to_vec(&contract)
                .map_err(|e| format!("Failed to serialize contract: {}", e))?;
            self.b_put_account(contract_address, &account)?;
        }

        Ok(())
    }

    /// Upgrade contract (owner only)
    fn contract_upgrade(&self, ix: &Instruction, new_code: Vec<u8>) -> Result<(), String> {
        if ix.accounts.len() < 2 {
            return Err("Upgrade requires owner and contract accounts".to_string());
        }

        let owner = &ix.accounts[0];
        let contract_address = &ix.accounts[1];

        let account = self
            .b_get_account(contract_address)?
            .ok_or("Contract not found")?;

        let mut contract: ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to deserialize contract: {}", e))?;

        if contract.owner != *owner {
            return Err("Only contract owner can upgrade".to_string());
        }

        let mut runtime = ContractRuntime::get_pooled();
        let new_hash_result = runtime.deploy(&new_code);
        runtime.return_to_pool();
        let new_hash = new_hash_result?;

        contract.code = new_code;
        contract.code_hash = new_hash;
        // AUDIT-FIX 3.7: Clear stale ABI from previous code version — the new
        // code may have different exports/params. ABI should be re-published.
        contract.abi = None;

        let mut updated_account = account;
        updated_account.data = serde_json::to_vec(&contract)
            .map_err(|e| format!("Failed to serialize contract: {}", e))?;

        self.b_put_account(contract_address, &updated_account)?;

        Ok(())
    }

    /// Close contract and withdraw balance
    fn contract_close(&self, ix: &Instruction) -> Result<(), String> {
        if ix.accounts.len() < 3 {
            return Err("Close requires owner, contract, and destination accounts".to_string());
        }

        let owner = &ix.accounts[0];
        let contract_address = &ix.accounts[1];
        let destination = &ix.accounts[2];

        let account = self
            .b_get_account(contract_address)?
            .ok_or("Contract not found")?;

        let contract: ContractAccount = serde_json::from_slice(&account.data)
            .map_err(|e| format!("Failed to deserialize contract: {}", e))?;

        if contract.owner != *owner {
            return Err("Only contract owner can close".to_string());
        }

        // T2.10 fix: Refuse to close if staked or locked balance exists.
        // Those balances follow their own lifecycle (unstake cooldown, etc.)
        // and must be claimed before the contract can be closed.
        if account.staked > 0 {
            return Err(format!(
                "Cannot close contract with {} staked shells — unstake first",
                account.staked
            ));
        }
        if account.locked > 0 {
            return Err(format!(
                "Cannot close contract with {} locked shells — claim unstake first",
                account.locked
            ));
        }

        // Transfer only spendable balance (not staked/locked) to destination
        let spendable = account.spendable;
        if spendable > 0 {
            self.b_transfer(contract_address, destination, spendable)?;
        }

        // Mark contract as non-executable and clear code data
        let mut closed_account = self.b_get_account(contract_address)?.unwrap_or(account);
        closed_account.executable = false;
        closed_account.data = Vec::new();
        self.b_put_account(contract_address, &closed_account)?;

        Ok(())
    }

    fn apply_rent(&self, tx: &Transaction) -> Result<(), String> {
        let current_slot = self.b_get_last_slot()?;
        if current_slot == 0 {
            return Ok(());
        }

        let mut accounts = HashSet::new();
        for ix in &tx.message.instructions {
            for account in &ix.accounts {
                accounts.insert(*account);
            }
        }

        let (rent_rate, rent_free_kb) = self.state.get_rent_params()?;

        // Accumulate total rent collected to credit treasury afterwards
        let mut total_rent_collected: u64 = 0;

        for pubkey in accounts {
            let mut account = match self.b_get_account(&pubkey)? {
                Some(acc) => acc,
                None => continue,
            };

            if account.rent_epoch == 0 {
                account.rent_epoch = current_slot;
                self.b_put_account(&pubkey, &account)?;
                continue;
            }

            let elapsed_slots = current_slot.saturating_sub(account.rent_epoch);
            if elapsed_slots < SLOTS_PER_MONTH {
                continue;
            }

            let months = elapsed_slots / SLOTS_PER_MONTH;
            let data_len = account.data.len() as u64;
            let free_bytes = rent_free_kb.saturating_mul(1024);

            if data_len <= free_bytes {
                account.rent_epoch = current_slot;
                self.b_put_account(&pubkey, &account)?;
                continue;
            }

            let billable_bytes = data_len - free_bytes;
            let billable_kb = billable_bytes.div_ceil(1024);
            let rent_due = months.saturating_mul(billable_kb).saturating_mul(rent_rate);

            if rent_due > 0 {
                // AUDIT-FIX 3.9: Graceful rent — collect up to what is available.
                // Zero-balance accounts persist indefinitely (rent is clamped to 0).
                // Account eviction is NOT implemented to avoid data loss risks.
                // Future: consider garbage collection of zero-balance + zero-data accounts.
                let actual_rent = rent_due.min(account.spendable);
                if actual_rent > 0 {
                    account
                        .deduct_spendable(actual_rent)
                        .map_err(|e| format!("Rent deduction failed: {}", e))?;
                    total_rent_collected = total_rent_collected.saturating_add(actual_rent);
                }
            }

            account.rent_epoch = current_slot;
            self.b_put_account(&pubkey, &account)?;
        }

        // Credit collected rent to treasury (prevents supply leak)
        if total_rent_collected > 0 {
            let treasury_pubkey = self
                .state
                .get_treasury_pubkey()?
                .ok_or_else(|| "Treasury pubkey not set for rent credit".to_string())?;
            let mut treasury = self
                .b_get_account(&treasury_pubkey)?
                .unwrap_or_else(|| Account::new(0, treasury_pubkey));
            treasury.add_spendable(total_rent_collected)?;
            self.b_put_account(&treasury_pubkey, &treasury)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
struct DeployRegistryData {
    symbol: Option<String>,
    name: Option<String>,
    template: Option<String>,
    metadata: Option<serde_json::Value>,
    upgrade_authority: Option<String>,
    make_public: Option<bool>,
    /// Explicit ABI provided by the deployer (takes priority over auto-extracted)
    abi: Option<ContractAbi>,
}

impl DeployRegistryData {
    fn from_init_data(init_data: &[u8]) -> Option<Self> {
        if init_data.is_empty() {
            return None;
        }
        let raw = std::str::from_utf8(init_data).ok()?;
        serde_json::from_str(raw).ok()
    }
}

/// MoltyID trust tier calculation (matches contract implementation)
/// Tier 0: Newcomer (rep < 100)
/// Tier 1: Known (rep 100-499)
/// Tier 2: Trusted (rep 500-999)
/// Tier 3: Established (rep 1000-4999)
/// Tier 4: Veteran (rep 5000-9999)
/// Tier 5: Legendary (rep 10000+)
pub fn get_trust_tier(reputation: u64) -> u8 {
    if reputation >= 10_000 {
        5
    } else if reputation >= 5_000 {
        4
    } else if reputation >= 1_000 {
        3
    } else if reputation >= 500 {
        2
    } else if reputation >= 100 {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Hash;
    use crate::Keypair;
    use tempfile::tempdir;

    /// Helper: set up a processor with treasury, funded alice account, and a genesis block.
    /// Returns genesis block hash for use as recent_blockhash in test transactions.
    fn setup() -> (TxProcessor, StateStore, Keypair, Pubkey, Pubkey, Hash) {
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();
        let processor = TxProcessor::new(state.clone());

        let alice_keypair = Keypair::generate();
        let alice = alice_keypair.pubkey();
        let treasury = Pubkey([3u8; 32]);

        state.set_treasury_pubkey(&treasury).unwrap();
        state
            .put_account(&treasury, &Account::new(0, treasury))
            .unwrap();

        // Fund alice with 1000 MOLT
        let alice_account = Account::new(1000, alice);
        state.put_account(&alice, &alice_account).unwrap();

        // Store a genesis block so get_recent_blockhashes returns a real hash
        let genesis = crate::Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::default(),
            [0u8; 32],
            Vec::new(),
            0,
        );
        let genesis_hash = genesis.hash();
        state.put_block(&genesis).unwrap();
        state.set_last_slot(0).unwrap();

        (
            processor,
            state,
            alice_keypair,
            alice,
            treasury,
            genesis_hash,
        )
    }

    /// Helper: build and sign a transfer tx
    fn make_transfer_tx(
        from_kp: &Keypair,
        from: Pubkey,
        to: Pubkey,
        amount_molt: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![0u8];
        data.extend_from_slice(&Account::molt_to_shells(amount_molt).to_le_bytes());

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![from, to],
            data,
        };

        let message = crate::transaction::Message::new(vec![ix], recent_blockhash);
        let mut tx = Transaction::new(message);
        let sig = from_kp.sign(&tx.message.serialize());
        tx.signatures.push(sig);
        tx
    }

    #[test]
    fn test_transfer() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        let tx = make_transfer_tx(&alice_kp, alice, bob, 100, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);

        assert!(result.success);
        assert_eq!(result.fee_paid, BASE_FEE);
        assert_eq!(
            state.get_balance(&bob).unwrap(),
            Account::molt_to_shells(100)
        );
    }

    #[test]
    fn test_replay_protection_rejects_bad_blockhash() {
        let (processor, _state, alice_kp, alice, _treasury, _genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        // Use a random blockhash that's not in recent history
        let bad_hash = Hash::hash(b"nonexistent_block");
        let tx = make_transfer_tx(&alice_kp, alice, bob, 10, bad_hash);
        let result = processor.process_transaction(&tx, &validator);

        assert!(
            !result.success,
            "Tx with invalid recent_blockhash should be rejected"
        );
        assert!(result.error.unwrap().contains("Blockhash not found"));
    }

    #[test]
    fn test_replay_protection_accepts_genesis_hash() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        // Real genesis block hash is valid (stored in recent blockhashes)
        let tx = make_transfer_tx(&alice_kp, alice, bob, 10, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);

        assert!(
            result.success,
            "Tx with genesis blockhash should be accepted"
        );
    }

    #[test]
    fn test_unsigned_tx_rejected() {
        let (processor, _state, _alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        // Build tx but DON'T sign it
        let mut data = vec![0u8];
        data.extend_from_slice(&Account::molt_to_shells(10).to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, bob],
            data,
        };
        let message = crate::transaction::Message::new(vec![ix], genesis_hash);
        let tx = Transaction::new(message);

        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Unsigned tx should be rejected");
    }

    #[test]
    fn test_wrong_signer_rejected() {
        let (processor, _state, _alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        // Sign with a DIFFERENT key
        let eve_kp = Keypair::generate();

        let mut data = vec![0u8];
        data.extend_from_slice(&Account::molt_to_shells(10).to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, bob],
            data,
        };
        let message = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(message);
        let sig = eve_kp.sign(&tx.message.serialize());
        tx.signatures.push(sig);

        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Tx signed by wrong key should be rejected");
    }

    #[test]
    fn test_multi_instruction_tx() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let charlie = Pubkey([4u8; 32]);
        let validator = Pubkey([42u8; 32]);

        // Two instructions, both from alice
        let mut data1 = vec![0u8];
        data1.extend_from_slice(&Account::molt_to_shells(10).to_le_bytes());
        let ix1 = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, bob],
            data: data1,
        };

        let mut data2 = vec![0u8];
        data2.extend_from_slice(&Account::molt_to_shells(20).to_le_bytes());
        let ix2 = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, charlie],
            data: data2,
        };

        let message = crate::transaction::Message::new(vec![ix1, ix2], genesis_hash);
        let mut tx = Transaction::new(message);
        let sig = alice_kp.sign(&tx.message.serialize());
        tx.signatures.push(sig);

        let result = processor.process_transaction(&tx, &validator);
        assert!(
            result.success,
            "Multi-instruction tx from same signer should work"
        );

        assert_eq!(
            state.get_balance(&bob).unwrap(),
            Account::molt_to_shells(10)
        );
        assert_eq!(
            state.get_balance(&charlie).unwrap(),
            Account::molt_to_shells(20)
        );
    }

    #[test]
    fn test_fee_deducted_from_payer() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        let initial_balance = state.get_balance(&alice).unwrap();
        let transfer_amount = Account::molt_to_shells(50);
        let tx = make_transfer_tx(&alice_kp, alice, bob, 50, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);

        assert!(result.success);
        let final_balance = state.get_balance(&alice).unwrap();
        assert_eq!(final_balance, initial_balance - transfer_amount - BASE_FEE);
    }

    #[test]
    fn test_insufficient_balance_rejected() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        // Alice has 1000 MOLT, try to send 2000
        let tx = make_transfer_tx(&alice_kp, alice, bob, 2000, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);

        assert!(!result.success, "Oversized transfer should be rejected");
    }

    // ─── ReefStake instruction tests ──────────────────────────────────

    /// Helper: build and sign a ReefStake deposit tx (instruction type 13)
    fn make_reefstake_deposit_tx(
        kp: &Keypair,
        user: Pubkey,
        amount_shells: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![13u8];
        data.extend_from_slice(&amount_shells.to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![user],
            data,
        };
        let message = crate::transaction::Message::new(vec![ix], recent_blockhash);
        let mut tx = Transaction::new(message);
        tx.signatures.push(kp.sign(&tx.message.serialize()));
        tx
    }

    /// Helper: build and sign a ReefStake unstake tx (instruction type 14)
    fn make_reefstake_unstake_tx(
        kp: &Keypair,
        user: Pubkey,
        st_molt_amount: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![14u8];
        data.extend_from_slice(&st_molt_amount.to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![user],
            data,
        };
        let message = crate::transaction::Message::new(vec![ix], recent_blockhash);
        let mut tx = Transaction::new(message);
        tx.signatures.push(kp.sign(&tx.message.serialize()));
        tx
    }

    /// Helper: build and sign a ReefStake claim tx (instruction type 15)
    fn make_reefstake_claim_tx(kp: &Keypair, user: Pubkey, recent_blockhash: Hash) -> Transaction {
        let data = vec![15u8];
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![user],
            data,
        };
        let message = crate::transaction::Message::new(vec![ix], recent_blockhash);
        let mut tx = Transaction::new(message);
        tx.signatures.push(kp.sign(&tx.message.serialize()));
        tx
    }

    #[test]
    fn test_reefstake_deposit_reduces_balance() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let deposit_amount = Account::molt_to_shells(100);
        let initial_balance = state.get_balance(&alice).unwrap();

        let tx = make_reefstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);

        assert!(
            result.success,
            "ReefStake deposit should succeed: {:?}",
            result.error
        );

        let final_balance = state.get_balance(&alice).unwrap();
        // Balance should decrease by deposit + fee
        assert_eq!(
            final_balance,
            initial_balance - deposit_amount - result.fee_paid
        );

        // Pool should have the staked amount
        let pool = state.get_reefstake_pool().unwrap();
        assert_eq!(pool.st_molt_token.total_molt_staked, deposit_amount);
        assert!(pool.positions.contains_key(&alice));
    }

    #[test]
    fn test_reefstake_deposit_zero_rejected() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let tx = make_reefstake_deposit_tx(&alice_kp, alice, 0, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);

        assert!(!result.success, "Zero deposit should be rejected");
    }

    #[test]
    fn test_reefstake_deposit_insufficient_balance() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Alice has 1000 MOLT, try to deposit 2000
        let tx = make_reefstake_deposit_tx(
            &alice_kp,
            alice,
            Account::molt_to_shells(2000),
            genesis_hash,
        );
        let result = processor.process_transaction(&tx, &validator);

        assert!(!result.success, "Over-balance deposit should be rejected");
    }

    #[test]
    fn test_reefstake_unstake_creates_request() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // First deposit
        let deposit_amount = Account::molt_to_shells(200);
        let tx = make_reefstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(result.success, "Deposit should succeed");

        // Get the stMOLT minted (1:1 on first deposit)
        let pool = state.get_reefstake_pool().unwrap();
        let st_molt = pool.positions.get(&alice).unwrap().st_molt_amount;
        assert_eq!(st_molt, deposit_amount);

        // Request unstake
        let tx = make_reefstake_unstake_tx(&alice_kp, alice, st_molt, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(result.success, "Unstake should succeed: {:?}", result.error);

        // Check pending unstake request exists
        let pool = state.get_reefstake_pool().unwrap();
        let requests = pool.get_unstake_requests(&alice);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].molt_to_receive, deposit_amount);
    }

    #[test]
    fn test_reefstake_claim_before_cooldown_fails() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Deposit then unstake
        let deposit_amount = Account::molt_to_shells(100);
        let tx = make_reefstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        assert!(processor.process_transaction(&tx, &validator).success);

        let pool = state.get_reefstake_pool().unwrap();
        let st_molt = pool.positions.get(&alice).unwrap().st_molt_amount;

        let tx = make_reefstake_unstake_tx(&alice_kp, alice, st_molt, genesis_hash);
        assert!(processor.process_transaction(&tx, &validator).success);

        // Try claim immediately (slot 0, cooldown is 151200 slots)
        let tx = make_reefstake_claim_tx(&alice_kp, alice, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Claim before cooldown should fail");
    }

    #[test]
    fn test_reefstake_claim_after_cooldown_succeeds() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let initial_balance = state.get_balance(&alice).unwrap();

        // Deposit
        let deposit_amount = Account::molt_to_shells(100);
        let tx = make_reefstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        let r1 = processor.process_transaction(&tx, &validator);
        assert!(r1.success);

        // Unstake
        let pool = state.get_reefstake_pool().unwrap();
        let st_molt = pool.positions.get(&alice).unwrap().st_molt_amount;
        let tx = make_reefstake_unstake_tx(&alice_kp, alice, st_molt, genesis_hash);
        let r2 = processor.process_transaction(&tx, &validator);
        assert!(r2.success);

        // Advance the slot beyond cooldown (1,512,000 = 7 days at 400ms/slot)
        // Create a new block at a slot past the cooldown period
        let future_block = crate::Block::new_with_timestamp(
            2_000_000,
            genesis_hash,
            Hash::hash(b"future_state"),
            [0u8; 32],
            Vec::new(),
            999_999,
        );
        let future_hash = future_block.hash();
        state.put_block(&future_block).unwrap();
        state.set_last_slot(2_000_000).unwrap();

        // Claim should succeed now
        let tx = make_reefstake_claim_tx(&alice_kp, alice, future_hash);
        let r3 = processor.process_transaction(&tx, &validator);
        assert!(
            r3.success,
            "Claim after cooldown should succeed: {:?}",
            r3.error
        );

        // Balance should be restored minus all fees
        let final_balance = state.get_balance(&alice).unwrap();
        let total_fees = r1.fee_paid + r2.fee_paid + r3.fee_paid;
        assert_eq!(final_balance, initial_balance - total_fees);
    }

    #[test]
    fn test_reefstake_unstake_more_than_staked_fails() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Deposit 100 MOLT
        let deposit_amount = Account::molt_to_shells(100);
        let tx = make_reefstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        assert!(processor.process_transaction(&tx, &validator).success);

        // Try to unstake 200 MOLT worth of stMOLT
        let too_much = Account::molt_to_shells(200);
        let tx = make_reefstake_unstake_tx(&alice_kp, alice, too_much, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Unstaking more than staked should fail");
    }

    // ── H16 tests: system instruction types 17, 18, 19 ──

    #[test]
    fn test_system_deploy_contract_success() {
        let (processor, state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Fund treasury for test
        let mut treasury_acct = state.get_account(&treasury).unwrap().unwrap();
        treasury_acct
            .add_spendable(Account::molt_to_shells(100))
            .unwrap();
        state.put_account(&treasury, &treasury_acct).unwrap();

        // Build deploy instruction: [17 | code_length(4 LE) | code_bytes]
        let code = vec![0x00, 0x61, 0x73, 0x6D]; // fake WASM magic
        let mut data = vec![17u8];
        data.extend_from_slice(&(code.len() as u32).to_le_bytes());
        data.extend_from_slice(&code);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury],
            data,
        };
        let message = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(message);
        let sig = alice_kp.sign(&tx.message.serialize());
        tx.signatures.push(sig);

        let result = processor.process_transaction(&tx, &validator);
        assert!(result.success, "Deploy should succeed: {:?}", result.error);
    }

    #[test]
    fn test_system_deploy_contract_insufficient_funds() {
        let (processor, _state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Alice has 1000 MOLT but we drain her to 1 MOLT
        let low_alice = Account::new(1, alice);
        _state.put_account(&alice, &low_alice).unwrap();

        let code = vec![0x00; 100];
        let mut data = vec![17u8];
        data.extend_from_slice(&(code.len() as u32).to_le_bytes());
        data.extend_from_slice(&code);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury],
            data,
        };
        let message = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(message);
        let sig = alice_kp.sign(&tx.message.serialize());
        tx.signatures.push(sig);

        let result = processor.process_transaction(&tx, &validator);
        assert!(
            !result.success,
            "Deploy with insufficient funds should fail"
        );
    }

    #[test]
    fn test_system_set_contract_abi() {
        let (processor, state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // First deploy a contract
        let code = vec![0x00, 0x61, 0x73, 0x6D];
        let mut deploy_data = vec![17u8];
        deploy_data.extend_from_slice(&(code.len() as u32).to_le_bytes());
        deploy_data.extend_from_slice(&code);

        let deploy_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury],
            data: deploy_data.clone(),
        };
        let msg = crate::transaction::Message::new(vec![deploy_ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        let sig = alice_kp.sign(&tx.message.serialize());
        tx.signatures.push(sig);
        let r = processor.process_transaction(&tx, &validator);
        assert!(
            r.success,
            "Deploy for ABI test should succeed: {:?}",
            r.error
        );

        // Find the deployed program address
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(alice.0);
        hasher.update(&code);
        let hash = hasher.finalize();
        let mut addr_bytes = [0u8; 32];
        addr_bytes.copy_from_slice(&hash[..32]);
        let program_pubkey = Pubkey(addr_bytes);

        // Now set ABI
        let abi = serde_json::json!({
            "version": "1.0",
            "name": "TestContract",
            "functions": []
        });
        let abi_bytes = serde_json::to_vec(&abi).unwrap();
        let mut abi_data = vec![18u8];
        abi_data.extend_from_slice(&abi_bytes);

        let abi_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, program_pubkey],
            data: abi_data,
        };
        let msg2 = crate::transaction::Message::new(vec![abi_ix], genesis_hash);
        let mut tx2 = Transaction::new(msg2);
        let sig2 = alice_kp.sign(&tx2.message.serialize());
        tx2.signatures.push(sig2);
        let result = processor.process_transaction(&tx2, &validator);
        assert!(
            result.success,
            "SetContractAbi should succeed: {:?}",
            result.error
        );

        // Verify ABI is stored
        let acct = state.get_account(&program_pubkey).unwrap().unwrap();
        let contract: crate::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        assert!(contract.abi.is_some());
    }

    #[test]
    fn test_system_set_contract_abi_wrong_owner_fails() {
        let (processor, state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Deploy a contract as alice
        let code = vec![0x00, 0x61, 0x73, 0x6E];
        let mut deploy_data = vec![17u8];
        deploy_data.extend_from_slice(&(code.len() as u32).to_le_bytes());
        deploy_data.extend_from_slice(&code);
        let deploy_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury],
            data: deploy_data,
        };
        let msg = crate::transaction::Message::new(vec![deploy_ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        assert!(processor.process_transaction(&tx, &validator).success);

        // Compute program address
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(alice.0);
        hasher.update(&code);
        let hash = hasher.finalize();
        let mut addr_bytes = [0u8; 32];
        addr_bytes.copy_from_slice(&hash[..32]);
        let program_pubkey = Pubkey(addr_bytes);

        // Try setting ABI as a different user (bob)
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        state.put_account(&bob, &Account::new(100, bob)).unwrap();

        let abi_bytes = b"{\"version\":\"1.0\"}";
        let mut abi_data = vec![18u8];
        abi_data.extend_from_slice(abi_bytes);
        let abi_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob, program_pubkey],
            data: abi_data,
        };
        let msg2 = crate::transaction::Message::new(vec![abi_ix], genesis_hash);
        let mut tx2 = Transaction::new(msg2);
        tx2.signatures.push(bob_kp.sign(&tx2.message.serialize()));
        let r = processor.process_transaction(&tx2, &validator);
        assert!(!r.success, "SetContractAbi by non-owner should fail");
    }

    #[test]
    fn test_system_faucet_airdrop() {
        let (processor, state, _alice_kp, _alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Fund treasury
        let mut t = state.get_account(&treasury).unwrap().unwrap();
        t.add_spendable(Account::molt_to_shells(1000)).unwrap();
        state.put_account(&treasury, &t).unwrap();

        let recipient = Pubkey([0x99; 32]);
        let amount: u64 = Account::molt_to_shells(10);

        let mut data = vec![19u8];
        data.extend_from_slice(&amount.to_le_bytes());

        let _ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![treasury, recipient],
            data,
        };
        // Faucet airdrop needs to be signed by treasury — we use a keypair for the test
        let treasury_kp = Keypair::from_seed(&[3u8; 32]);
        // Re-set treasury pubkey to match the keyed treasury
        state.set_treasury_pubkey(&treasury_kp.pubkey()).unwrap();
        let treasury_pk = treasury_kp.pubkey();
        let tacct = state.get_account(&treasury).unwrap().unwrap();
        state.put_account(&treasury_pk, &tacct).unwrap();

        let ix2 = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![treasury_pk, recipient],
            data: {
                let mut d = vec![19u8];
                d.extend_from_slice(&amount.to_le_bytes());
                d
            },
        };
        let msg = crate::transaction::Message::new(vec![ix2], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures
            .push(treasury_kp.sign(&tx.message.serialize()));
        let result = processor.process_transaction(&tx, &validator);
        assert!(
            result.success,
            "Faucet airdrop should succeed: {:?}",
            result.error
        );

        let r = state.get_account(&recipient).unwrap();
        assert!(r.is_some());
        assert_eq!(r.unwrap().spendable, amount);
    }

    #[test]
    fn test_system_faucet_airdrop_cap_exceeded() {
        let (processor, state, _alice_kp, _alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let mut t = state.get_account(&treasury).unwrap().unwrap();
        t.add_spendable(Account::molt_to_shells(10000)).unwrap();
        state.put_account(&treasury, &t).unwrap();

        let recipient = Pubkey([0xBB; 32]);
        // 200 MOLT exceeds 100 MOLT cap
        let amount: u64 = 200u64 * 1_000_000_000;

        let mut data = vec![19u8];
        data.extend_from_slice(&amount.to_le_bytes());

        let _ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![treasury, recipient],
            data,
        };
        let treasury_kp = Keypair::from_seed(&[3u8; 32]);
        state.set_treasury_pubkey(&treasury_kp.pubkey()).unwrap();
        state.put_account(&treasury_kp.pubkey(), &t).unwrap();

        let ix2 = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![treasury_kp.pubkey(), recipient],
            data: {
                let mut d = vec![19u8];
                d.extend_from_slice(&amount.to_le_bytes());
                d
            },
        };
        let msg = crate::transaction::Message::new(vec![ix2], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures
            .push(treasury_kp.sign(&tx.message.serialize()));
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Airdrop > 100 MOLT should fail");
    }
}
