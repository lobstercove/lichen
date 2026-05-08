// Lichen Core - Transaction Processor

use crate::account::{Account, Pubkey};
use crate::consensus::{slot_to_epoch, SLOTS_PER_EPOCH};
use crate::contract::{
    build_top_level_call_context, contract_lifecycle_status_for_restriction_mode,
    derive_contract_lifecycle_from_state_store, ContractAbi, ContractAccount, ContractContext,
    ContractEvent, ContractRuntime, NativeAccountOp,
};
use crate::contract_instruction::ContractInstruction;
use crate::evm::{
    decode_evm_transaction, execute_evm_transaction, u256_is_multiple_of_spore, u256_to_spores,
    EvmReceipt, EvmTxRecord, EVM_PROGRAM_ID,
};
use crate::governance::{GovernanceAction, GovernanceProposal};
use crate::state::{StateBatch, StateStore, SymbolRegistryEntry};
use crate::transaction::{Instruction, Transaction};
use crate::{Hash, MAX_CONTRACT_CODE};
use alloy_primitives::U256;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;

mod achievement_detection;
mod batch_access;
mod contract_execution;
mod contract_lifecycle;
mod contract_metadata;
mod execution;
mod fees;
mod governance_authorities;
mod governance_lifecycle;
mod governance_oracle;
mod governance_parsing;
mod governance_policies;
mod governed_transfers;
mod nonce_handlers;
mod rent_collection;
mod shielded_handlers;
mod system_basics;
mod system_extended;
mod trust_tier;
mod validator_lifecycle;

pub use trust_tier::get_trust_tier;

/// Transaction execution result
#[derive(Debug, Clone)]
pub struct TxResult {
    pub success: bool,
    pub fee_paid: u64,
    pub error: Option<String>,
    /// Compute units consumed by this transaction (native + WASM).
    pub compute_units_used: u64,
    /// Contract return code (if the transaction includes a contract call).
    /// This is the raw WASM function return value — interpretation depends on the
    /// contract's ABI. For LichenID: 0=success, 1=bad input, 2=identity not found, etc.
    pub return_code: Option<i64>,
    /// Log messages emitted by the contract during execution.
    pub contract_logs: Vec<String>,
    /// Return data set by the contract via `set_return_data()`.
    pub return_data: Vec<u8>,
}

/// Persistent transaction execution metadata stored in CF_TX_META.
/// Extends the old 8-byte CU-only format with full contract result data.
#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug)]
pub struct TxMeta {
    pub compute_units_used: u64,
    pub return_code: Option<i64>,
    pub return_data: Vec<u8>,
    pub logs: Vec<String>,
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
    /// Contract function return code (if a contract call was simulated).
    pub return_code: Option<i64>,
    /// Number of storage changes that would be produced by the TX.
    /// Used by preflight to detect silent failures (success=true, 0 changes).
    pub state_changes: usize,
}

#[derive(Debug, Clone)]
struct SymbolRegistrationSpec {
    symbol: String,
    name: Option<String>,
    template: Option<String>,
    metadata: Option<serde_json::Value>,
    decimals: Option<u8>,
}

const MAX_SYMBOL_REGISTRY_SYMBOL_LEN: usize = 32;
const MAX_SYMBOL_REGISTRY_NAME_LEN: usize = 128;
const MAX_SYMBOL_REGISTRY_TEMPLATE_LEN: usize = 32;
const MAX_SYMBOL_REGISTRY_METADATA_KEY_LEN: usize = 64;

fn validate_symbol_registry_field_length(
    field: &str,
    value: &str,
    max_len: usize,
) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("RegisterSymbol: '{}' cannot be empty", field));
    }
    if value.len() > max_len {
        return Err(format!(
            "RegisterSymbol: '{}' exceeds {} bytes",
            field, max_len
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct GovernedTransferExecutionPolicy {
    threshold: u8,
    execute_after_epoch: u64,
    velocity_tier: crate::multisig::GovernedTransferVelocityTier,
    daily_cap_spores: u64,
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

/// P9-RPC-01: EVM sentinel blockhash — used by `eth_sendRawTransaction` to
/// mark EVM-wrapped transactions.  The EVM layer provides its own replay
/// protection via nonces + ECDSA signatures, so native blockhash validation
/// is skipped for these TXs.  Non-EVM transactions MUST NOT use this hash;
/// doing so is rejected as an attempted bypass.
pub const EVM_SENTINEL_BLOCKHASH: Hash = Hash([0xEE; 32]);

/// Slot-based month length (400ms slots, 216,000 per day)
pub const SLOTS_PER_MONTH: u64 = 216_000 * 30;

/// Free tier: accounts with data ≤ 2KB are exempt from rent
pub const RENT_FREE_BYTES: u64 = 2048;

/// Number of consecutive missed rent epochs before an account becomes dormant
pub const DORMANCY_THRESHOLD_EPOCHS: u64 = 2;

const SECONDS_PER_DAY: u64 = 86_400;

/// Maximum age in blocks for a transaction's recent_blockhash.
/// Transactions referencing a blockhash older than this are rejected.
pub const MAX_TX_AGE_BLOCKS: u64 = 300;
/// Base transaction fee (0.001 LICN = 1,000,000 spores)
/// At $0.10/LICN: $0.0001 per tx  |  At $1.00/LICN: $0.001 per tx
/// Solana ~$0.00025/tx — Lichen is 2.5x cheaper at $0.10/LICN
pub const BASE_FEE: u64 = 1_000_000;

/// Contract deployment fee (25 LICN = 25,000,000,000 spores)
/// At $0.10/LICN: $2.50 per deploy  |  At $1.00/LICN: $25 per deploy
pub const CONTRACT_DEPLOY_FEE: u64 = 25_000_000_000;

/// Contract upgrade fee (10 LICN = 10,000,000,000 spores)
/// At $0.10/LICN: $1.00 per upgrade  |  At $1.00/LICN: $10 per upgrade
pub const CONTRACT_UPGRADE_FEE: u64 = 10_000_000_000;

/// NFT mint fee (0.5 LICN = 500,000,000 spores)
/// At $0.10/LICN: $0.05 per mint  |  At $1.00/LICN: $0.50 per mint
pub const NFT_MINT_FEE: u64 = 500_000_000;

/// NFT collection creation fee (1,000 LICN = 1,000,000,000,000 spores)
/// At $0.10/LICN: $100 per collection  |  At $1.00/LICN: $1,000 per collection
pub const NFT_COLLECTION_FEE: u64 = 1_000_000_000_000;

/// Minimum balance required to create a nonce account (0.01 LICN = 10,000,000 spores).
/// Keeps nonce accounts rent-exempt while preventing spam creation.
pub const NONCE_ACCOUNT_MIN_BALANCE: u64 = 10_000_000;

/// Magic marker stored at data[0] to identify nonce accounts.
pub const NONCE_ACCOUNT_MARKER: u8 = 0xDA;

// ── Virtual conflict keys for parallel TX scheduling ──
// These sentinel Pubkeys force transactions that touch the same singleton
// state (stake pool, MossStake pool, governance counter) into the same
// scheduling group, preventing lost-update races in parallel execution.
// Values are chosen to never collide with real versioned Lichen addresses.

/// Virtual key: any TX that reads/writes the stake pool (opcodes 9, 10, 11, 26, 27, 31).
pub const CONFLICT_KEY_STAKE_POOL: Pubkey = Pubkey([0xFE; 32]);
/// Virtual key: any TX that reads/writes the MossStake pool (opcodes 13, 14, 15, 16).
pub const CONFLICT_KEY_MOSSSTAKE_POOL: Pubkey = Pubkey([0xFD; 32]);
/// Virtual key: any TX that allocates/reads governed proposal IDs (opcode 21).
pub const CONFLICT_KEY_GOVERNED_PROPOSALS: Pubkey = Pubkey([0xFC; 32]);
/// Virtual key: any TX that allocates/reads protocol-governance proposal IDs (opcodes 34-37).
pub const CONFLICT_KEY_GOVERNANCE_PROPOSALS: Pubkey = Pubkey([0xFB; 32]);
/// Virtual key: any TX that reads/writes native oracle attestation/consensus state (opcode 30).
pub const CONFLICT_KEY_ORACLE: Pubkey = Pubkey([0xFA; 32]);

pub const GOVERNANCE_ACTION_TREASURY_TRANSFER: u8 = 0;
pub const GOVERNANCE_ACTION_PARAM_CHANGE: u8 = 1;
pub const GOVERNANCE_ACTION_CONTRACT_UPGRADE: u8 = 2;
pub const GOVERNANCE_ACTION_SET_UPGRADE_TIMELOCK: u8 = 3;
pub const GOVERNANCE_ACTION_EXECUTE_UPGRADE: u8 = 4;
pub const GOVERNANCE_ACTION_VETO_UPGRADE: u8 = 5;
pub const GOVERNANCE_ACTION_CONTRACT_CLOSE: u8 = 6;
pub const GOVERNANCE_ACTION_REGISTER_SYMBOL: u8 = 7;
pub const GOVERNANCE_ACTION_SET_CONTRACT_ABI: u8 = 8;
pub const GOVERNANCE_ACTION_CONTRACT_CALL: u8 = 9;
pub const GOVERNANCE_ACTION_RESTRICT: u8 = 10;
pub const GOVERNANCE_ACTION_LIFT_RESTRICTION: u8 = 11;
pub const GOVERNANCE_ACTION_EXTEND_RESTRICTION: u8 = 12;

/// base_fee (spores per transaction)
pub const GOV_PARAM_BASE_FEE: u8 = 0;
/// fee_burn_percent (0-100)
pub const GOV_PARAM_FEE_BURN_PERCENT: u8 = 1;
/// fee_producer_percent (0-100)
pub const GOV_PARAM_FEE_PRODUCER_PERCENT: u8 = 2;
/// fee_voters_percent (0-100)
pub const GOV_PARAM_FEE_VOTERS_PERCENT: u8 = 3;
/// fee_treasury_percent (0-100)
pub const GOV_PARAM_FEE_TREASURY_PERCENT: u8 = 4;
/// fee_community_percent (0-100)
pub const GOV_PARAM_FEE_COMMUNITY_PERCENT: u8 = 5;
/// min_validator_stake (spores)
pub const GOV_PARAM_MIN_VALIDATOR_STAKE: u8 = 6;
/// epoch_slots (slots per epoch)
pub const GOV_PARAM_EPOCH_SLOTS: u8 = 7;

pub const CU_TRANSFER: u64 = 100;
pub const CU_CREATE_ACCOUNT: u64 = 200;
pub const CU_CREATE_COLLECTION: u64 = 500;
pub const CU_MINT_NFT: u64 = 1_000;
pub const CU_TRANSFER_NFT: u64 = 200;
pub const CU_STAKE: u64 = 500;
pub const CU_UNSTAKE: u64 = 500;
pub const CU_CLAIM_UNSTAKE: u64 = 300;
pub const CU_REGISTER_EVM: u64 = 200;
pub const CU_MOSSSTAKE: u64 = 500;
pub const CU_DEPLOY_CONTRACT: u64 = 5_000;
pub const CU_SET_CONTRACT_ABI: u64 = 1_000;
pub const CU_FAUCET_AIRDROP: u64 = 100;
pub const CU_REGISTER_SYMBOL: u64 = 300;
pub const CU_GOVERNED_PROPOSAL: u64 = 1_000;
pub const CU_ZK_SHIELD: u64 = 100_000;
pub const CU_ZK_TRANSFER: u64 = 200_000;
pub const CU_REGISTER_VALIDATOR: u64 = 500;
pub const CU_SLASH_VALIDATOR: u64 = 500;
pub const CU_NONCE: u64 = 200;
pub const CU_GOVERNANCE_PARAM: u64 = 300;
pub const CU_GOVERNANCE_ACTION: u64 = 1_000;
pub const CU_ORACLE_ATTESTATION: u64 = 500;
pub const CU_DEREGISTER_VALIDATOR: u64 = 500;

/// Minimum number of assets name bytes (e.g. "BTC" = 3).
pub const ORACLE_ASSET_MIN_LEN: usize = 1;
/// Maximum asset name length for oracle attestations.
pub const ORACLE_ASSET_MAX_LEN: usize = 16;
/// Oracle attestation staleness window in slots (~1 hour at 400ms/slot).
pub const ORACLE_STALENESS_SLOTS: u64 = 9_000;

/// Look up the compute-unit cost for a system program instruction by its type byte.
pub fn compute_units_for_system_ix(instruction_type: u8) -> u64 {
    match instruction_type {
        0 | 2..=5 => CU_TRANSFER,
        1 => CU_CREATE_ACCOUNT,
        6 => CU_CREATE_COLLECTION,
        7 => CU_MINT_NFT,
        8 => CU_TRANSFER_NFT,
        9 => CU_STAKE,
        10 => CU_UNSTAKE,
        11 => CU_CLAIM_UNSTAKE,
        12 => CU_REGISTER_EVM,
        13..=16 => CU_MOSSSTAKE,
        17 => CU_DEPLOY_CONTRACT,
        18 => CU_SET_CONTRACT_ABI,
        19 => CU_FAUCET_AIRDROP,
        20 => CU_REGISTER_SYMBOL,
        21 | 22 => CU_GOVERNED_PROPOSAL,
        23 => CU_ZK_SHIELD,
        24 | 25 => CU_ZK_TRANSFER,
        26 => CU_REGISTER_VALIDATOR,
        27 => CU_SLASH_VALIDATOR,
        28 => CU_NONCE,
        29 => CU_GOVERNANCE_PARAM,
        30 => CU_ORACLE_ATTESTATION,
        31 => CU_DEREGISTER_VALIDATOR,
        32 | 33 => CU_GOVERNED_PROPOSAL,
        34..=37 => CU_GOVERNANCE_ACTION,
        _ => 100,
    }
}

/// Compute total compute units for all instructions in a transaction.
pub fn compute_units_for_tx(tx: &Transaction) -> u64 {
    let mut total = 0u64;
    for ix in &tx.message.instructions {
        if ix.program_id == SYSTEM_PROGRAM_ID {
            if let Some(&instruction_type) = ix.data.first() {
                total += compute_units_for_system_ix(instruction_type);
            }
        }
    }
    total
}

/// A single validator oracle price attestation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OracleAttestation {
    pub validator: Pubkey,
    pub price: u64,
    pub decimals: u8,
    pub stake: u64,
    pub slot: u64,
}

/// Consensus oracle price derived from validator attestations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OracleConsensusPrice {
    pub asset: String,
    pub price: u64,
    pub decimals: u8,
    pub slot: u64,
    pub attestation_count: u32,
}

/// Compute the stake-weighted median price from a set of attestations.
pub fn compute_stake_weighted_median(attestations: &[OracleAttestation]) -> u64 {
    if attestations.is_empty() {
        return 0;
    }
    if attestations.len() == 1 {
        return attestations[0].price;
    }

    let mut sorted: Vec<(u64, u64)> = attestations.iter().map(|a| (a.price, a.stake)).collect();
    sorted.sort_by_key(|&(price, _)| price);

    let total_stake: u128 = sorted.iter().map(|&(_, stake)| stake as u128).sum();
    let half = total_stake / 2;

    let mut cumulative: u128 = 0;
    for &(price, stake) in &sorted {
        cumulative += stake as u128;
        if cumulative > half {
            return price;
        }
    }

    sorted.last().unwrap().0
}

/// Durable nonce account state — serialized into the account's `data` field.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct NonceState {
    pub authority: Pubkey,
    pub blockhash: Hash,
    pub fee_per_signature: u64,
}

/// Compute graduated rent for an account based on its data size.
pub fn compute_graduated_rent(data_len: u64, rate_per_kb_per_epoch: u64) -> u64 {
    if data_len <= RENT_FREE_BYTES {
        return 0;
    }
    let billable = data_len - RENT_FREE_BYTES;

    const TIER1_CAP: u64 = 8 * 1024;
    const TIER2_CAP: u64 = 98 * 1024;

    let tier1_bytes = billable.min(TIER1_CAP);
    let tier2_bytes = billable
        .saturating_sub(TIER1_CAP)
        .min(TIER2_CAP - TIER1_CAP);
    let tier3_bytes = billable.saturating_sub(TIER2_CAP);

    let tier1_kb = tier1_bytes.div_ceil(1024);
    let tier2_kb = tier2_bytes.div_ceil(1024);
    let tier3_kb = tier3_bytes.div_ceil(1024);

    tier1_kb
        .saturating_mul(rate_per_kb_per_epoch)
        .saturating_add(tier2_kb.saturating_mul(rate_per_kb_per_epoch.saturating_mul(2)))
        .saturating_add(tier3_kb.saturating_mul(rate_per_kb_per_epoch.saturating_mul(4)))
}

#[derive(Debug, Clone)]
pub struct FeeConfig {
    pub base_fee: u64,
    pub contract_deploy_fee: u64,
    pub contract_upgrade_fee: u64,
    pub nft_mint_fee: u64,
    pub nft_collection_fee: u64,
    pub fee_burn_percent: u64,
    pub fee_producer_percent: u64,
    pub fee_voters_percent: u64,
    pub fee_treasury_percent: u64,
    pub fee_community_percent: u64,
    pub fee_exempt_contracts: Vec<Pubkey>,
}

impl FeeConfig {
    pub fn default_from_constants() -> Self {
        FeeConfig {
            base_fee: BASE_FEE,
            contract_deploy_fee: CONTRACT_DEPLOY_FEE,
            contract_upgrade_fee: CONTRACT_UPGRADE_FEE,
            nft_mint_fee: NFT_MINT_FEE,
            nft_collection_fee: NFT_COLLECTION_FEE,
            fee_burn_percent: 40,
            fee_producer_percent: 30,
            fee_voters_percent: 10,
            fee_treasury_percent: 10,
            fee_community_percent: 10,
            fee_exempt_contracts: Vec::new(),
        }
    }

    pub fn apply_governance_param(&mut self, param_id: u8, value: u64) -> bool {
        match param_id {
            GOV_PARAM_BASE_FEE => {
                self.base_fee = value;
                true
            }
            GOV_PARAM_FEE_BURN_PERCENT => {
                self.fee_burn_percent = value;
                true
            }
            GOV_PARAM_FEE_PRODUCER_PERCENT => {
                self.fee_producer_percent = value;
                true
            }
            GOV_PARAM_FEE_VOTERS_PERCENT => {
                self.fee_voters_percent = value;
                true
            }
            GOV_PARAM_FEE_TREASURY_PERCENT => {
                self.fee_treasury_percent = value;
                true
            }
            GOV_PARAM_FEE_COMMUNITY_PERCENT => {
                self.fee_community_percent = value;
                true
            }
            _ => false,
        }
    }

    pub fn validate_distribution(&self) -> Result<(), String> {
        for (label, value) in [
            ("burn", self.fee_burn_percent),
            ("producer", self.fee_producer_percent),
            ("voters", self.fee_voters_percent),
            ("treasury", self.fee_treasury_percent),
            ("community", self.fee_community_percent),
        ] {
            if value > 100 {
                return Err(format!("FeeConfig: {} percentage must be 0..=100", label));
            }
        }

        let total = self
            .fee_burn_percent
            .saturating_add(self.fee_producer_percent)
            .saturating_add(self.fee_voters_percent)
            .saturating_add(self.fee_treasury_percent)
            .saturating_add(self.fee_community_percent);
        if total != 100 {
            return Err(format!(
                "FeeConfig: fee percentages must sum to 100, got {}",
                total
            ));
        }

        Ok(())
    }
}

/// Transaction processor
pub struct TxProcessor {
    state: StateStore,
    batch: Mutex<Option<StateBatch>>,
    #[allow(clippy::type_complexity)]
    contract_meta: Mutex<(Option<i64>, Vec<String>, u64, Vec<u8>)>,
    tx_compute_budget: Mutex<u64>,
    #[cfg(feature = "zk")]
    zk_verifier: Mutex<crate::zk::Verifier>,
}

impl TxProcessor {
    pub fn new(state: StateStore) -> Self {
        TxProcessor {
            state,
            batch: Mutex::new(None),
            contract_meta: Mutex::new((None, Vec::new(), 0, Vec::new())),
            tx_compute_budget: Mutex::new(0),
            #[cfg(feature = "zk")]
            zk_verifier: Mutex::new(crate::zk::Verifier::new()),
        }
    }

    fn verify_transaction_signatures(tx: &Transaction) -> Result<(), String> {
        tx.verify_required_signatures().map(|_| ())
    }

    fn drain_contract_meta(&self) -> (Option<i64>, Vec<String>, u64, Vec<u8>) {
        let mut meta = self.contract_meta.lock().unwrap_or_else(|e| e.into_inner());
        (
            meta.0.take(),
            std::mem::take(&mut meta.1),
            std::mem::replace(&mut meta.2, 0),
            std::mem::take(&mut meta.3),
        )
    }

    fn make_result(
        &self,
        success: bool,
        fee_paid: u64,
        error: Option<String>,
        compute_units_used: u64,
    ) -> TxResult {
        let (return_code, contract_logs, _meta_cu, return_data) = self.drain_contract_meta();
        TxResult {
            success,
            fee_paid,
            error,
            compute_units_used,
            return_code,
            contract_logs,
            return_data,
        }
    }

    /// Check if a transaction is a valid durable nonce transaction.
    fn check_durable_nonce(tx: &Transaction, state: &StateStore) -> bool {
        let first_ix = match tx.message.instructions.first() {
            Some(ix) => ix,
            None => return false,
        };

        if first_ix.program_id != SYSTEM_PROGRAM_ID {
            return false;
        }
        if first_ix.data.len() < 2 || first_ix.data[0] != 28 || first_ix.data[1] != 1 {
            return false;
        }

        let nonce_pk = match first_ix.accounts.get(1) {
            Some(pk) => pk,
            None => return false,
        };

        let nonce_account = match state.get_account(nonce_pk) {
            Ok(Some(account)) => account,
            _ => return false,
        };

        match Self::decode_nonce_state(&nonce_account.data) {
            Ok(nonce_state) => nonce_state.blockhash == tx.message.recent_blockhash,
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::MIN_VALIDATOR_STAKE;
    use crate::restrictions::{
        ProtocolModuleId, RestrictionLiftReason, RestrictionMode, RestrictionReason,
        RestrictionRecord, RestrictionStatus, RestrictionTarget, GUARDIAN_RESTRICTION_MAX_SLOTS,
        NATIVE_LICN_ASSET_ID,
    };
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

        // Fund alice with 1000 LICN
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

    fn advance_test_slot(state: &StateStore, slot: u64) -> Hash {
        let parent_hash = state
            .get_block_by_slot(state.get_last_slot().unwrap())
            .unwrap()
            .map(|block| block.hash())
            .unwrap_or_default();
        let block = crate::Block::new_with_timestamp(
            slot,
            parent_hash,
            Hash::default(),
            [0u8; 32],
            Vec::new(),
            slot,
        );
        let blockhash = block.hash();
        state.put_block(&block).unwrap();
        state.set_last_slot(slot).unwrap();
        blockhash
    }

    /// Helper: build and sign a transfer tx
    fn make_transfer_tx(
        from_kp: &Keypair,
        from: Pubkey,
        to: Pubkey,
        amount_licn: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![0u8];
        data.extend_from_slice(&Account::licn_to_spores(amount_licn).to_le_bytes());

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
            Account::licn_to_spores(100)
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
        data.extend_from_slice(&Account::licn_to_spores(10).to_le_bytes());
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
        data.extend_from_slice(&Account::licn_to_spores(10).to_le_bytes());
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
        data1.extend_from_slice(&Account::licn_to_spores(10).to_le_bytes());
        let ix1 = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, bob],
            data: data1,
        };

        let mut data2 = vec![0u8];
        data2.extend_from_slice(&Account::licn_to_spores(20).to_le_bytes());
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
            Account::licn_to_spores(10)
        );
        assert_eq!(
            state.get_balance(&charlie).unwrap(),
            Account::licn_to_spores(20)
        );
    }

    #[test]
    fn test_fee_deducted_from_payer() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        let initial_balance = state.get_balance(&alice).unwrap();
        let transfer_amount = Account::licn_to_spores(50);
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

        // Alice has 1000 LICN, try to send 2000
        let tx = make_transfer_tx(&alice_kp, alice, bob, 2000, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);

        assert!(!result.success, "Oversized transfer should be rejected");
    }

    // ─── MossStake instruction tests ──────────────────────────────────

    /// Helper: build and sign a MossStake deposit tx (instruction type 13)
    fn make_mossstake_deposit_tx(
        kp: &Keypair,
        user: Pubkey,
        amount_spores: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![13u8];
        data.extend_from_slice(&amount_spores.to_le_bytes());
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

    /// Helper: build and sign a MossStake unstake tx (instruction type 14)
    fn make_mossstake_unstake_tx(
        kp: &Keypair,
        user: Pubkey,
        st_licn_amount: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![14u8];
        data.extend_from_slice(&st_licn_amount.to_le_bytes());
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

    /// Helper: build and sign a MossStake claim tx (instruction type 15)
    fn make_mossstake_claim_tx(kp: &Keypair, user: Pubkey, recent_blockhash: Hash) -> Transaction {
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

    fn make_mossstake_transfer_tx(
        kp: &Keypair,
        from: Pubkey,
        to: Pubkey,
        st_licn_amount: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![16u8];
        data.extend_from_slice(&st_licn_amount.to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![from, to],
            data,
        };
        let message = crate::transaction::Message::new(vec![ix], recent_blockhash);
        let mut tx = Transaction::new(message);
        tx.signatures.push(kp.sign(&tx.message.serialize()));
        tx
    }

    #[test]
    fn test_mossstake_deposit_reduces_balance() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let deposit_amount = Account::licn_to_spores(100);
        let initial_balance = state.get_balance(&alice).unwrap();

        let tx = make_mossstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);

        assert!(
            result.success,
            "MossStake deposit should succeed: {:?}",
            result.error
        );

        let final_balance = state.get_balance(&alice).unwrap();
        // Balance should decrease by deposit + fee
        assert_eq!(
            final_balance,
            initial_balance - deposit_amount - result.fee_paid
        );

        // Pool should have the staked amount
        let pool = state.get_mossstake_pool().unwrap();
        assert_eq!(pool.st_licn_token.total_licn_staked, deposit_amount);
        assert!(pool.positions.contains_key(&alice));
    }

    #[test]
    fn test_mossstake_deposit_zero_rejected() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let tx = make_mossstake_deposit_tx(&alice_kp, alice, 0, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);

        assert!(!result.success, "Zero deposit should be rejected");
    }

    #[test]
    fn test_mossstake_deposit_insufficient_balance() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Alice has 1000 LICN, try to deposit 2000
        let tx = make_mossstake_deposit_tx(
            &alice_kp,
            alice,
            Account::licn_to_spores(2000),
            genesis_hash,
        );
        let result = processor.process_transaction(&tx, &validator);

        assert!(!result.success, "Over-balance deposit should be rejected");
    }

    #[test]
    fn test_mossstake_unstake_creates_request() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // First deposit
        let deposit_amount = Account::licn_to_spores(200);
        let tx = make_mossstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(result.success, "Deposit should succeed");

        // Get the stLICN minted (1:1 on first deposit)
        let pool = state.get_mossstake_pool().unwrap();
        let st_licn = pool.positions.get(&alice).unwrap().st_licn_amount;
        assert_eq!(st_licn, deposit_amount);

        // Request unstake
        let tx = make_mossstake_unstake_tx(&alice_kp, alice, st_licn, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(result.success, "Unstake should succeed: {:?}", result.error);

        // Check pending unstake request exists
        let pool = state.get_mossstake_pool().unwrap();
        let requests = pool.get_unstake_requests(&alice);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].licn_to_receive, deposit_amount);
    }

    #[test]
    fn test_mossstake_claim_before_cooldown_fails() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Deposit then unstake
        let deposit_amount = Account::licn_to_spores(100);
        let tx = make_mossstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        assert!(processor.process_transaction(&tx, &validator).success);

        let pool = state.get_mossstake_pool().unwrap();
        let st_licn = pool.positions.get(&alice).unwrap().st_licn_amount;

        let tx = make_mossstake_unstake_tx(&alice_kp, alice, st_licn, genesis_hash);
        assert!(processor.process_transaction(&tx, &validator).success);

        // Try claim immediately (slot 0, cooldown is 151200 slots)
        let tx = make_mossstake_claim_tx(&alice_kp, alice, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Claim before cooldown should fail");
    }

    #[test]
    fn test_mossstake_claim_after_cooldown_succeeds() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let initial_balance = state.get_balance(&alice).unwrap();

        // Deposit
        let deposit_amount = Account::licn_to_spores(100);
        let tx = make_mossstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        let r1 = processor.process_transaction(&tx, &validator);
        assert!(r1.success);

        // Unstake
        let pool = state.get_mossstake_pool().unwrap();
        let st_licn = pool.positions.get(&alice).unwrap().st_licn_amount;
        let tx = make_mossstake_unstake_tx(&alice_kp, alice, st_licn, genesis_hash);
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
        let tx = make_mossstake_claim_tx(&alice_kp, alice, future_hash);
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
    fn test_mossstake_unstake_more_than_staked_fails() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Deposit 100 LICN
        let deposit_amount = Account::licn_to_spores(100);
        let tx = make_mossstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        assert!(processor.process_transaction(&tx, &validator).success);

        // Try to unstake 200 LICN worth of stLICN
        let too_much = Account::licn_to_spores(200);
        let tx = make_mossstake_unstake_tx(&alice_kp, alice, too_much, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Unstaking more than staked should fail");
    }

    #[test]
    fn test_mossstake_deposit_rejects_outgoing_restricted_depositor() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let before_pool = state.get_mossstake_pool().unwrap();

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::OutgoingOnly,
        );

        let tx =
            make_mossstake_deposit_tx(&alice_kp, alice, Account::licn_to_spores(100), genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("MossStakeDeposit blocked by active depositor account restriction"));

        let after_pool = state.get_mossstake_pool().unwrap();
        assert_eq!(after_pool.st_licn_token.total_licn_staked, 0);
        assert_eq!(after_pool.positions.len(), before_pool.positions.len());
        assert!(!after_pool.positions.contains_key(&alice));
    }

    #[test]
    fn test_mossstake_deposit_rejects_native_frozen_amount() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let authority_spendable = state.get_account(&alice).unwrap().unwrap().spendable;

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::AccountAsset {
                account: alice,
                asset: NATIVE_LICN_ASSET_ID,
            },
            RestrictionMode::FrozenAmount {
                amount: authority_spendable,
            },
        );

        let tx =
            make_mossstake_deposit_tx(&alice_kp, alice, Account::licn_to_spores(100), genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains(
            "MossStakeDeposit blocked by active depositor native account-asset restriction"
        ));

        let pool = state.get_mossstake_pool().unwrap();
        assert_eq!(pool.st_licn_token.total_licn_staked, 0);
        assert!(!pool.positions.contains_key(&alice));
    }

    #[test]
    fn test_mossstake_protocol_pause_rejects_deposit_without_position() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::ProtocolModule(ProtocolModuleId::MossStake),
            RestrictionMode::ProtocolPaused,
        );

        let tx =
            make_mossstake_deposit_tx(&alice_kp, alice, Account::licn_to_spores(100), genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("MossStakeDeposit blocked by active MossStake protocol pause"));

        let pool = state.get_mossstake_pool().unwrap();
        assert_eq!(pool.st_licn_token.total_licn_staked, 0);
        assert!(!pool.positions.contains_key(&alice));
    }

    #[test]
    fn test_mossstake_unstake_rejects_outgoing_restricted_position_owner() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let deposit_amount = Account::licn_to_spores(100);
        let deposit_tx = make_mossstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        assert!(
            processor
                .process_transaction(&deposit_tx, &validator)
                .success
        );

        let before_pool = state.get_mossstake_pool().unwrap();
        let st_licn = before_pool.positions.get(&alice).unwrap().st_licn_amount;
        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::OutgoingOnly,
        );

        let tx = make_mossstake_unstake_tx(&alice_kp, alice, st_licn, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("MossStakeUnstake blocked by active position owner account restriction"));

        let after_pool = state.get_mossstake_pool().unwrap();
        assert_eq!(
            after_pool.positions.get(&alice).unwrap().st_licn_amount,
            st_licn
        );
        assert!(after_pool.get_unstake_requests(&alice).is_empty());
    }

    #[test]
    fn test_mossstake_claim_rejects_incoming_restricted_user_without_dropping_request() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let deposit_amount = Account::licn_to_spores(100);
        let deposit_tx = make_mossstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        assert!(
            processor
                .process_transaction(&deposit_tx, &validator)
                .success
        );

        let pool = state.get_mossstake_pool().unwrap();
        let st_licn = pool.positions.get(&alice).unwrap().st_licn_amount;
        let unstake_tx = make_mossstake_unstake_tx(&alice_kp, alice, st_licn, genesis_hash);
        assert!(
            processor
                .process_transaction(&unstake_tx, &validator)
                .success
        );
        let before_requests = state
            .get_mossstake_pool()
            .unwrap()
            .get_unstake_requests(&alice);
        assert_eq!(before_requests.len(), 1);

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::IncomingOnly,
        );
        let future_hash = advance_test_slot(&state, crate::consensus::UNSTAKE_COOLDOWN_SLOTS + 1);
        let claim_tx = make_mossstake_claim_tx(&alice_kp, alice, future_hash);
        let result = processor.process_transaction(&claim_tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("MossStakeClaim blocked by active user account restriction"));

        let after_requests = state
            .get_mossstake_pool()
            .unwrap()
            .get_unstake_requests(&alice);
        assert_eq!(after_requests.len(), before_requests.len());
        assert_eq!(
            after_requests[0].licn_to_receive,
            before_requests[0].licn_to_receive
        );
    }

    #[test]
    fn test_mossstake_transfer_rejects_outgoing_restricted_sender() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob = Pubkey([0xB2; 32]);
        let deposit_amount = Account::licn_to_spores(100);
        let deposit_tx = make_mossstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        assert!(
            processor
                .process_transaction(&deposit_tx, &validator)
                .success
        );
        let st_licn = state
            .get_mossstake_pool()
            .unwrap()
            .positions
            .get(&alice)
            .unwrap()
            .st_licn_amount;

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::OutgoingOnly,
        );

        let tx = make_mossstake_transfer_tx(&alice_kp, alice, bob, st_licn / 2, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("MossStakeTransfer blocked by active sender account restriction"));

        let pool = state.get_mossstake_pool().unwrap();
        assert_eq!(pool.positions.get(&alice).unwrap().st_licn_amount, st_licn);
        assert!(!pool.positions.contains_key(&bob));
        assert!(state.get_account(&bob).unwrap().is_none());
    }

    #[test]
    fn test_mossstake_transfer_rejects_incoming_restricted_recipient() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob = Pubkey([0xB3; 32]);
        let deposit_amount = Account::licn_to_spores(100);
        let deposit_tx = make_mossstake_deposit_tx(&alice_kp, alice, deposit_amount, genesis_hash);
        assert!(
            processor
                .process_transaction(&deposit_tx, &validator)
                .success
        );
        let st_licn = state
            .get_mossstake_pool()
            .unwrap()
            .positions
            .get(&alice)
            .unwrap()
            .st_licn_amount;

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(bob),
            RestrictionMode::IncomingOnly,
        );

        let tx = make_mossstake_transfer_tx(&alice_kp, alice, bob, st_licn / 2, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("MossStakeTransfer blocked by active recipient account restriction"));

        let pool = state.get_mossstake_pool().unwrap();
        assert_eq!(pool.positions.get(&alice).unwrap().st_licn_amount, st_licn);
        assert!(!pool.positions.contains_key(&bob));
        assert!(state.get_account(&bob).unwrap().is_none());
    }

    // ── H16 tests: system instruction types 17, 18, 19 ──

    #[test]
    fn test_system_deploy_contract_success() {
        let (processor, state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Fund treasury for test
        let mut treasury_acct = state.get_account(&treasury).unwrap().unwrap();
        treasury_acct
            .add_spendable(Account::licn_to_spores(100))
            .unwrap();
        state.put_account(&treasury, &treasury_acct).unwrap();

        // Build deploy instruction: [17 | code_length(4 LE) | code_bytes]
        // Valid WASM: magic (4 bytes) + version (4 bytes)
        let code = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
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
    fn test_system_deploy_contract_allows_same_code_multiple_times_via_nonce() {
        let (processor, state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let mut treasury_acct = state.get_account(&treasury).unwrap().unwrap();
        treasury_acct
            .add_spendable(Account::licn_to_spores(200))
            .unwrap();
        state.put_account(&treasury, &treasury_acct).unwrap();

        let code = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        let build_tx = |blockhash| {
            let mut data = vec![17u8];
            data.extend_from_slice(&(code.len() as u32).to_le_bytes());
            data.extend_from_slice(&code);
            let ix = Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![alice, treasury],
                data,
            };
            let mut tx = Transaction::new(crate::transaction::Message::new(vec![ix], blockhash));
            tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
            tx
        };

        let result1 = processor.process_transaction(&build_tx(genesis_hash), &validator);
        assert!(
            result1.success,
            "first deploy should succeed: {:?}",
            result1.error
        );

        let recent_block = crate::Block::new_with_timestamp(
            1,
            genesis_hash,
            Hash::hash(b"deploy-second-state"),
            [0u8; 32],
            Vec::new(),
            1,
        );
        let second_hash = recent_block.hash();
        state.put_block(&recent_block).unwrap();
        state.set_last_slot(1).unwrap();
        let result2 = processor.process_transaction(&build_tx(second_hash), &validator);
        assert!(
            result2.success,
            "second deploy should succeed: {:?}",
            result2.error
        );

        let programs = state.get_programs(10).unwrap();
        assert_eq!(
            programs.len(),
            2,
            "same code should deploy to two unique addresses"
        );
        assert_ne!(programs[0], programs[1]);
    }

    #[test]
    fn test_system_deploy_contract_deterministic_mode_rejects_duplicate_address() {
        let (processor, state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let mut treasury_acct = state.get_account(&treasury).unwrap().unwrap();
        treasury_acct
            .add_spendable(Account::licn_to_spores(200))
            .unwrap();
        state.put_account(&treasury, &treasury_acct).unwrap();

        let code = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        let init = serde_json::json!({
            "name": "deterministic-demo",
            "deploy_deterministic": true
        })
        .to_string();
        let build_tx = |blockhash| {
            let mut data = vec![17u8];
            data.extend_from_slice(&(code.len() as u32).to_le_bytes());
            data.extend_from_slice(&code);
            data.extend_from_slice(init.as_bytes());
            let ix = Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![alice, treasury],
                data,
            };
            let mut tx = Transaction::new(crate::transaction::Message::new(vec![ix], blockhash));
            tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
            tx
        };

        let result1 = processor.process_transaction(&build_tx(genesis_hash), &validator);
        assert!(
            result1.success,
            "first deterministic deploy should succeed: {:?}",
            result1.error
        );

        let recent_block = crate::Block::new_with_timestamp(
            1,
            genesis_hash,
            Hash::hash(b"deploy-deterministic-second-state"),
            [0u8; 32],
            Vec::new(),
            1,
        );
        let second_hash = recent_block.hash();
        state.put_block(&recent_block).unwrap();
        state.set_last_slot(1).unwrap();
        let result2 = processor.process_transaction(&build_tx(second_hash), &validator);
        assert!(!result2.success, "duplicate deterministic deploy must fail");
        assert!(
            result2
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("Contract already exists"),
            "unexpected: {:?}",
            result2.error
        );
    }

    /// AUDIT-FIX B-2: System deploy (type 17) charges contract_deploy_fee.
    #[test]
    fn test_system_deploy_charges_deploy_fee() {
        let (processor, state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Fund treasury
        let mut treasury_acct = state.get_account(&treasury).unwrap().unwrap();
        treasury_acct
            .add_spendable(Account::licn_to_spores(100))
            .unwrap();
        state.put_account(&treasury, &treasury_acct).unwrap();

        let before = state.get_account(&alice).unwrap().unwrap().spendable;

        // Valid WASM module
        let code = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
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

        // The fee should include contract_deploy_fee (25 LICN) + base_fee (0.001 LICN)
        let after = state.get_account(&alice).unwrap().unwrap().spendable;
        let charged = before - after;
        // contract_deploy_fee = 25_000_000_000 spores, base_fee = 1_000_000 spores
        assert!(
            charged >= 25_000_000_000,
            "Expected at least 25 LICN fee for deploy, got {} spores charged",
            charged
        );
    }

    /// AUDIT-FIX B-2: An account with only 1 LICN cannot pay the 25 LICN deploy fee.
    #[test]
    fn test_system_deploy_rejects_underfunded() {
        let (processor, state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Set Alice to only 1 LICN — cannot afford 25 LICN deploy fee
        let low = Account::new(1, alice);
        state.put_account(&alice, &low).unwrap();

        let code = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
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
            "Deploy with only 1 LICN should fail due to 25 LICN fee"
        );
    }

    #[test]
    fn test_system_deploy_contract_invalid_wasm_magic() {
        let (processor, state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let mut treasury_acct = state.get_account(&treasury).unwrap().unwrap();
        treasury_acct
            .add_spendable(Account::licn_to_spores(100))
            .unwrap();
        state.put_account(&treasury, &treasury_acct).unwrap();

        // Invalid magic bytes (not WASM)
        let code = vec![0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x00, 0x00, 0x00];
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
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &validator);
        assert!(
            !result.success,
            "Deploy with invalid WASM magic should fail"
        );
        assert!(result.error.unwrap().contains("bad magic number"));
    }

    #[test]
    fn test_system_deploy_contract_too_small() {
        let (processor, state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let mut treasury_acct = state.get_account(&treasury).unwrap().unwrap();
        treasury_acct
            .add_spendable(Account::licn_to_spores(100))
            .unwrap();
        state.put_account(&treasury, &treasury_acct).unwrap();

        // Only 4 bytes — below 8-byte minimum
        let code = vec![0x00, 0x61, 0x73, 0x6D];
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
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Deploy with code too small should fail");
        assert!(result.error.unwrap().contains("too small"));
    }

    #[test]
    fn test_code_hash_deploy_block_rejects_system_deploy_contract() {
        let (processor, state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let mut treasury_acct = state.get_account(&treasury).unwrap().unwrap();
        treasury_acct
            .add_spendable(Account::licn_to_spores(100))
            .unwrap();
        state.put_account(&treasury, &treasury_acct).unwrap();

        let code = valid_wasm_code(0x40);
        let code_hash = Hash::hash(&code);
        let restriction_id = put_active_processor_test_restriction(
            &state,
            RestrictionTarget::CodeHash(code_hash),
            RestrictionMode::DeployBlocked,
        );

        let mut data = vec![17u8];
        data.extend_from_slice(&(code.len() as u32).to_le_bytes());
        data.extend_from_slice(&code);
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury],
            data,
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);

        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Banned code hash deploy must fail");
        let error = result.error.as_deref().unwrap_or_default();
        assert!(error.contains("DeployContract rejected"));
        assert!(error.contains("DeployBlocked"));
        assert!(error.contains(&restriction_id.to_string()));
        assert!(
            state.get_programs(10).unwrap().is_empty(),
            "blocked deploy must not index a program"
        );
    }

    #[test]
    fn test_code_hash_deploy_block_rejects_contract_program_deploy() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let code = valid_wasm_code(0x41);
        let code_hash = Hash::hash(&code);
        let restriction_id = put_active_processor_test_restriction(
            &state,
            RestrictionTarget::CodeHash(code_hash),
            RestrictionMode::DeployBlocked,
        );
        let contract_addr = Pubkey([0x41; 32]);

        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Deploy {
                code,
                init_data: Vec::new(),
            },
            genesis_hash,
            &validator,
        );
        assert!(!result.success, "Banned code hash deploy must fail");
        let error = result.error.as_deref().unwrap_or_default();
        assert!(error.contains("Deploy rejected"));
        assert!(error.contains("DeployBlocked"));
        assert!(error.contains(&restriction_id.to_string()));
        assert!(
            state.get_account(&contract_addr).unwrap().is_none(),
            "blocked deploy must not create a contract account"
        );
    }

    /// Test: ContractInstruction::Deploy via CONTRACT_PROGRAM_ID with init_data
    /// populates the symbol registry atomically.
    #[test]
    fn test_contract_program_deploy_with_symbol_registry() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Valid WASM module (magic + version)
        let code = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];

        // Build init_data JSON with symbol registration metadata
        let init_data = serde_json::json!({
            "symbol": "TESTCOIN",
            "name": "Test Coin",
            "template": "token",
            "decimals": 9,
            "metadata": {
                "description": "A test token for unit testing",
                "website": "https://example.com",
                "mintable": "true"
            }
        });
        let init_data_bytes = serde_json::to_vec(&init_data).unwrap();

        // Compute contract address like the CLI does
        let code_hash = Hash::hash(&code);
        let mut addr_bytes = [0u8; 32];
        addr_bytes[..16].copy_from_slice(&alice.0[..16]);
        addr_bytes[16..].copy_from_slice(&code_hash.0[..16]);
        let contract_addr = Pubkey(addr_bytes);

        // Create deploy instruction via CONTRACT_PROGRAM_ID
        let contract_ix = crate::ContractInstruction::Deploy {
            code: code.clone(),
            init_data: init_data_bytes.clone(),
        };
        let ix = Instruction {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: vec![alice, contract_addr],
            data: contract_ix.serialize().unwrap(),
        };

        let message = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(message);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &validator);
        assert!(
            result.success,
            "ContractProgram Deploy should succeed: {:?}",
            result.error
        );

        // Verify contract account exists and is executable
        let acct = state.get_account(&contract_addr).unwrap();
        assert!(acct.is_some(), "Contract account should exist");
        assert!(acct.unwrap().executable, "Contract should be executable");

        // Verify symbol registry entry was written
        let entry = state.get_symbol_registry("TESTCOIN").unwrap();
        assert!(
            entry.is_some(),
            "Symbol TESTCOIN should be in the registry after deploy"
        );
        let entry = entry.unwrap();
        assert_eq!(entry.symbol, "TESTCOIN");
        assert_eq!(entry.program, contract_addr);
        assert_eq!(entry.owner, alice);
        assert_eq!(entry.name, Some("Test Coin".to_string()));
        assert_eq!(entry.template, Some("token".to_string()));
        assert_eq!(entry.decimals, Some(9));
        assert!(entry.metadata.is_some());
        let meta = entry.metadata.unwrap();
        assert_eq!(
            meta.get("description").and_then(|v| v.as_str()),
            Some("A test token for unit testing")
        );
    }

    #[test]
    fn test_validate_and_sanitize_metadata_accepts_scalar_values() {
        let metadata = Some(serde_json::json!({
            "description": "Community token",
            "decimals": 9,
            "mintable": true,
            "burnable": false
        }));

        let sanitized = TxProcessor::validate_and_sanitize_metadata(&metadata)
            .expect("scalar metadata should be accepted")
            .expect("metadata should remain present");

        assert_eq!(
            sanitized
                .get("description")
                .and_then(|value| value.as_str()),
            Some("Community token")
        );
        assert_eq!(
            sanitized.get("decimals").and_then(|value| value.as_u64()),
            Some(9)
        );
        assert_eq!(
            sanitized.get("mintable").and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            sanitized.get("burnable").and_then(|value| value.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn test_validate_and_sanitize_metadata_rejects_nested_values() {
        let metadata = Some(serde_json::json!({
            "social_urls": {
                "twitter": "https://x.com/lichen"
            }
        }));

        let err = TxProcessor::validate_and_sanitize_metadata(&metadata)
            .expect_err("nested metadata must be rejected");
        assert!(err.contains("string, number, or boolean"));
    }

    /// Test: Deploy fee premium is refunded when deploy instruction itself fails.
    #[test]
    fn test_contract_program_deploy_failure_refunds_premium() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let initial_balance = state.get_balance(&alice).unwrap();

        // Invalid WASM (bad magic bytes) — deploy should fail
        let bad_code = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x00, 0x00, 0x00];

        let code_hash = Hash::hash(&bad_code);
        let mut addr_bytes = [0u8; 32];
        addr_bytes[..16].copy_from_slice(&alice.0[..16]);
        addr_bytes[16..].copy_from_slice(&code_hash.0[..16]);
        let contract_addr = Pubkey(addr_bytes);

        let contract_ix = crate::ContractInstruction::Deploy {
            code: bad_code,
            init_data: vec![],
        };
        let ix = Instruction {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: vec![alice, contract_addr],
            data: contract_ix.serialize().unwrap(),
        };

        let message = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(message);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Deploy with bad WASM should fail");

        // Verify only base fee was kept (premium refunded)
        let final_balance = state.get_balance(&alice).unwrap();
        let fee_kept = initial_balance - final_balance;
        // base_fee = 1_000_000 spores (0.001 LICN), deploy premium = 25_000_000_000
        assert!(
            fee_kept < 25_000_000_000,
            "Premium should be refunded on failed deploy, but {} spores kept",
            fee_kept
        );
    }

    #[test]
    fn test_failed_premium_fee_refund_bypasses_incoming_restriction() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::IncomingOnly,
        );
        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::AccountAsset {
                account: alice,
                asset: NATIVE_LICN_ASSET_ID,
            },
            RestrictionMode::IncomingOnly,
        );

        let initial_balance = state.get_balance(&alice).unwrap();
        let bad_code = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x00, 0x00, 0x00];
        let code_hash = Hash::hash(&bad_code);
        let mut addr_bytes = [0u8; 32];
        addr_bytes[..16].copy_from_slice(&alice.0[..16]);
        addr_bytes[16..].copy_from_slice(&code_hash.0[..16]);
        let contract_addr = Pubkey(addr_bytes);

        let ix = Instruction {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: vec![alice, contract_addr],
            data: crate::ContractInstruction::Deploy {
                code: bad_code,
                init_data: vec![],
            }
            .serialize()
            .unwrap(),
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Deploy with bad WASM should fail");
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("bad magic number"));

        let final_balance = state.get_balance(&alice).unwrap();
        assert_eq!(initial_balance - final_balance, result.fee_paid);
        assert_eq!(result.fee_paid, BASE_FEE);
    }

    #[test]
    fn test_system_set_contract_abi() {
        let (processor, state, alice_kp, alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // First deploy a contract
        // Valid WASM: magic (4 bytes) + version (4 bytes)
        let code = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
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

        let programs = state.get_programs(10).unwrap();
        assert_eq!(programs.len(), 1, "expected a single deployed program");
        let program_pubkey = programs[0];

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
        // Valid WASM: magic (4 bytes) + version (4 bytes)
        let code = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
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

        let programs = state.get_programs(10).unwrap();
        assert_eq!(programs.len(), 1, "expected a single deployed program");
        let program_pubkey = programs[0];

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
        t.add_spendable(Account::licn_to_spores(1000)).unwrap();
        state.put_account(&treasury, &t).unwrap();

        let recipient = Pubkey([0x99; 32]);
        let amount: u64 = Account::licn_to_spores(10);

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
    fn test_fee_split_no_overflow_large_values() {
        // L6-01: Verify u128 intermediate prevents overflow when fee * percent > u64::MAX
        let (processor, state, _alice_kp, alice, treasury, _genesis_hash) = setup();

        // Give alice a huge balance
        let mut a = state.get_account(&alice).unwrap().unwrap();
        let initial_spendable = a.spendable;
        a.add_spendable(u64::MAX / 2).unwrap();
        state.put_account(&alice, &a).unwrap();

        // A fee of 1e18 (~1e9 LICN) times percent 50 would overflow u64 multiply
        let large_fee: u64 = 1_000_000_000_000_000_000; // 1e18 spores
        let result = processor.charge_fee_direct(&alice, large_fee);
        assert!(
            result.is_ok(),
            "Large fee should not overflow: {:?}",
            result.err()
        );

        // Verify payer was debited
        let a_after = state.get_account(&alice).unwrap().unwrap();
        assert_eq!(
            a_after.spendable,
            initial_spendable + u64::MAX / 2 - large_fee,
            "Payer should be debited exactly the fee amount"
        );

        // Verify treasury received the non-burned portion
        let t = state.get_account(&treasury).unwrap().unwrap();
        assert!(t.spendable > 0, "Treasury should have received fee portion");
    }

    #[test]
    fn test_system_faucet_airdrop_cap_exceeded() {
        let (processor, state, _alice_kp, _alice, treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let mut t = state.get_account(&treasury).unwrap().unwrap();
        t.add_spendable(Account::licn_to_spores(10000)).unwrap();
        state.put_account(&treasury, &t).unwrap();

        let recipient = Pubkey([0xBB; 32]);
        // 200 LICN exceeds 10 LICN cap
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
        assert!(!result.success, "Airdrop > 10 LICN should fail");
    }

    // ═════════════════════════════════════════════════════════════════════════
    // K1-01: Parallel transaction processing & conflict detection tests
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_parallel_disjoint_txs_succeed() {
        // Two transfers to different recipients should both succeed in parallel
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let carol = Pubkey([4u8; 32]);
        let validator = Pubkey([42u8; 32]);

        // Fund alice enough for both transfers + fees
        let alice_account = Account::new(500, alice);
        state.put_account(&alice, &alice_account).unwrap();

        // Both txs FROM alice → different targets: they SHARE alice and will be in same group
        let tx1 = make_transfer_tx(&alice_kp, alice, bob, 10, genesis_hash);
        let tx2 = make_transfer_tx(&alice_kp, alice, carol, 10, genesis_hash);

        let results = processor.process_transactions_parallel(&[tx1, tx2], &validator);
        assert_eq!(results.len(), 2);
        assert!(
            results[0].success,
            "tx1 (alice→bob) should succeed: {:?}",
            results[0].error
        );
        assert!(
            results[1].success,
            "tx2 (alice→carol) should succeed: {:?}",
            results[1].error
        );
    }

    #[test]
    fn test_parallel_truly_disjoint_txs() {
        // Two completely independent senders → should run in separate parallel groups
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();
        let processor = TxProcessor::new(state.clone());
        let validator = Pubkey([42u8; 32]);

        let alice_kp = Keypair::generate();
        let alice = alice_kp.pubkey();
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let carol = Pubkey([4u8; 32]);
        let dave = Pubkey([5u8; 32]);
        let treasury = Pubkey([3u8; 32]);

        state.set_treasury_pubkey(&treasury).unwrap();
        state
            .put_account(&treasury, &Account::new(0, treasury))
            .unwrap();
        state
            .put_account(&alice, &Account::new(500, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(500, bob)).unwrap();

        let genesis = crate::Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::default(),
            [0u8; 32],
            Vec::new(),
            0,
        );
        state.put_block(&genesis).unwrap();
        state.set_last_slot(0).unwrap();
        let genesis_hash = genesis.hash();

        // alice→carol and bob→dave are fully disjoint — parallel groups
        let tx1 = make_transfer_tx(&alice_kp, alice, carol, 10, genesis_hash);
        let tx2 = make_transfer_tx(&bob_kp, bob, dave, 10, genesis_hash);

        let results = processor.process_transactions_parallel(&[tx1, tx2], &validator);
        assert_eq!(results.len(), 2);
        assert!(
            results[0].success,
            "alice→carol should succeed: {:?}",
            results[0].error
        );
        assert!(
            results[1].success,
            "bob→dave should succeed: {:?}",
            results[1].error
        );
    }

    #[test]
    fn test_parallel_fee_charging_preserves_all_treasury_credits() {
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();
        let processor = TxProcessor::new(state.clone());
        let validator = Pubkey([42u8; 32]);
        let treasury = Pubkey([3u8; 32]);

        state.set_treasury_pubkey(&treasury).unwrap();
        state
            .put_account(&treasury, &Account::new(0, treasury))
            .unwrap();

        let genesis = crate::Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::default(),
            [0u8; 32],
            Vec::new(),
            0,
        );
        state.put_block(&genesis).unwrap();
        state.set_last_slot(0).unwrap();
        let genesis_hash = genesis.hash();

        let tx_count = 128usize;
        let mut txs = Vec::with_capacity(tx_count);
        for i in 0..tx_count {
            let payer_kp = Keypair::generate();
            let payer = payer_kp.pubkey();
            let mut recipient_bytes = [0x80u8; 32];
            recipient_bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
            let recipient = Pubkey(recipient_bytes);
            state
                .put_account(&payer, &Account::new(100, payer))
                .unwrap();
            txs.push(make_transfer_tx(
                &payer_kp,
                payer,
                recipient,
                1,
                genesis_hash,
            ));
        }

        let results = processor.process_transactions_parallel(&txs, &validator);
        for (idx, result) in results.iter().enumerate() {
            assert!(
                result.success,
                "parallel fee tx {} failed: {:?}",
                idx, result.error
            );
        }

        let fee_config = FeeConfig::default_from_constants();
        let burned_per_tx =
            (fee_config.base_fee as u128 * fee_config.fee_burn_percent as u128 / 100) as u64;
        let expected_treasury_per_tx = fee_config.base_fee.saturating_sub(burned_per_tx);
        assert_eq!(
            state.get_balance(&treasury).unwrap(),
            expected_treasury_per_tx * tx_count as u64,
            "parallel fee charging must not lose treasury credits"
        );
    }

    #[test]
    fn test_parallel_conflicting_txs_sequential() {
        // Two senders sending TO the same recipient share an account
        // They should still both succeed (processed sequentially within group)
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();
        let processor = TxProcessor::new(state.clone());
        let validator = Pubkey([42u8; 32]);

        let alice_kp = Keypair::generate();
        let alice = alice_kp.pubkey();
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let shared_recipient = Pubkey([99u8; 32]);
        let treasury = Pubkey([3u8; 32]);

        state.set_treasury_pubkey(&treasury).unwrap();
        state
            .put_account(&treasury, &Account::new(0, treasury))
            .unwrap();
        state
            .put_account(&alice, &Account::new(500, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(500, bob)).unwrap();

        let genesis = crate::Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::default(),
            [0u8; 32],
            Vec::new(),
            0,
        );
        state.put_block(&genesis).unwrap();
        state.set_last_slot(0).unwrap();
        let genesis_hash = genesis.hash();

        // Both send to shared_recipient → merged into same group
        let tx1 = make_transfer_tx(&alice_kp, alice, shared_recipient, 10, genesis_hash);
        let tx2 = make_transfer_tx(&bob_kp, bob, shared_recipient, 10, genesis_hash);

        let results = processor.process_transactions_parallel(&[tx1, tx2], &validator);
        assert_eq!(results.len(), 2);
        assert!(
            results[0].success,
            "tx1 should succeed in sequential group: {:?}",
            results[0].error
        );
        assert!(
            results[1].success,
            "tx2 should succeed in sequential group: {:?}",
            results[1].error
        );

        // Verify both actually transferred
        let r = state.get_account(&shared_recipient).unwrap().unwrap();
        let alice_sent = Account::licn_to_spores(10);
        let bob_sent = Account::licn_to_spores(10);
        assert!(
            r.spendable >= alice_sent + bob_sent,
            "Recipient should have both transfers"
        );
    }

    #[test]
    fn test_parallel_result_ordering_preserved() {
        // Ensure results[i] corresponds to txs[i] even when groups are reordered
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();
        let processor = TxProcessor::new(state.clone());
        let validator = Pubkey([42u8; 32]);

        let treasury = Pubkey([3u8; 32]);
        state.set_treasury_pubkey(&treasury).unwrap();
        state
            .put_account(&treasury, &Account::new(0, treasury))
            .unwrap();

        let genesis = crate::Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::default(),
            [0u8; 32],
            Vec::new(),
            0,
        );
        state.put_block(&genesis).unwrap();
        state.set_last_slot(0).unwrap();
        let genesis_hash = genesis.hash();

        // Create 4 independent senders for 4 disjoint txs
        let mut txs = Vec::new();
        let mut kps = Vec::new();
        for i in 0..4u8 {
            let kp = Keypair::generate();
            let pk = kp.pubkey();
            state.put_account(&pk, &Account::new(100, pk)).unwrap();
            let recipient = Pubkey([100 + i; 32]);
            txs.push(make_transfer_tx(&kp, pk, recipient, 5, genesis_hash));
            kps.push(kp);
        }

        let results = processor.process_transactions_parallel(&txs, &validator);
        assert_eq!(results.len(), 4);
        for (i, res) in results.iter().enumerate() {
            assert!(res.success, "tx[{}] should succeed: {:?}", i, res.error);
        }
    }

    #[test]
    fn test_parallel_single_tx_fallback() {
        // A single transaction should work fine (no parallelism needed)
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        let tx = make_transfer_tx(&alice_kp, alice, bob, 10, genesis_hash);
        let results = processor.process_transactions_parallel(&[tx], &validator);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].success,
            "Single tx should succeed: {:?}",
            results[0].error
        );
    }

    #[test]
    fn test_parallel_empty_batch() {
        let (processor, _state, _alice_kp, _alice, _treasury, _genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let results = processor.process_transactions_parallel(&[], &validator);
        assert_eq!(results.len(), 0);
    }

    /// P9-RPC-01: Non-EVM TXs with the EVM sentinel blockhash must be rejected.
    #[test]
    fn test_sentinel_blockhash_rejected_for_non_evm_tx() {
        let (processor, _state, alice_kp, alice, _treasury, _genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Build a normal transfer using the sentinel blockhash
        let ix = crate::transaction::Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, Pubkey([5u8; 32])],
            data: {
                let mut d = vec![0u8]; // Transfer
                d.extend_from_slice(&100u64.to_le_bytes());
                d
            },
        };
        let msg = crate::transaction::Message {
            instructions: vec![ix],
            recent_blockhash: EVM_SENTINEL_BLOCKHASH,
            compute_budget: None,
            compute_unit_price: None,
        };
        let sig = alice_kp.sign(&msg.serialize());
        let tx = Transaction {
            signatures: vec![sig],
            message: msg,
            tx_type: Default::default(),
        };
        let result = processor.process_transaction(&tx, &validator);
        assert!(
            !result.success,
            "Non-EVM TX with sentinel blockhash should be rejected"
        );
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("EVM sentinel blockhash"),
            "Error should mention the sentinel: {:?}",
            result.error,
        );
    }

    /// P9-RPC-01: EVM TX with sentinel blockhash must be accepted (routed to EVM path).
    /// It will fail at the EVM decode stage (no valid RLP in dummy data) but must
    /// NOT be rejected at the sentinel/blockhash check itself.
    #[test]
    fn test_sentinel_blockhash_accepted_for_evm_tx() {
        let (processor, _state, _alice_kp, alice, _treasury, _genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Build an EVM-program TX with sentinel blockhash and dummy data
        let ix = crate::transaction::Instruction {
            program_id: crate::evm::EVM_PROGRAM_ID,
            accounts: vec![alice],
            data: vec![0xDE, 0xAD], // invalid EVM payload — will fail decoding, not sentinel check
        };
        let msg = crate::transaction::Message {
            instructions: vec![ix],
            recent_blockhash: EVM_SENTINEL_BLOCKHASH,
            compute_budget: None,
            compute_unit_price: None,
        };
        let tx = Transaction {
            signatures: vec![crate::PqSignature::test_fixture(0)],
            message: msg,
            tx_type: Default::default(),
        };
        let result = processor.process_transaction(&tx, &validator);
        // Should fail with EVM decode error — NOT with "sentinel blockhash" error
        assert!(!result.success);
        let err = result.error.as_deref().unwrap_or("");
        assert!(
            !err.contains("sentinel blockhash"),
            "EVM TX should pass the sentinel check; got: {err}",
        );
    }

    /// AUDIT-FIX B-1: Treasury lock serializes concurrent fee charging.
    /// Two parallel groups charging fees must not lose updates — both debits
    /// must be reflected in the final treasury balance.
    #[test]
    fn test_treasury_lock_prevents_lost_updates() {
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();
        let treasury = Pubkey([3u8; 32]);
        state.set_treasury_pubkey(&treasury).unwrap();
        state
            .put_account(&treasury, &Account::new(0, treasury))
            .unwrap();

        // Create two payers each with 10 LICN (10_000_000_000 spores)
        let kp_a = Keypair::generate();
        let kp_b = Keypair::generate();
        let payer_a = kp_a.pubkey();
        let payer_b = kp_b.pubkey();
        let initial_spores = Account::licn_to_spores(10);
        state
            .put_account(&payer_a, &Account::new(10, payer_a))
            .unwrap();
        state
            .put_account(&payer_b, &Account::new(10, payer_b))
            .unwrap();

        let fee = Account::licn_to_spores(1); // 1 LICN = 1_000_000_000 spores

        // Simulate two parallel groups charging fees concurrently.
        // With the treasury_lock, the second group must see the first's write.
        let state_a = state.clone();
        let state_b = state.clone();

        let proc_a = TxProcessor::new(state_a);
        let proc_b = TxProcessor::new(state_b);

        // Group A charges fee
        proc_a.charge_fee_direct(&payer_a, fee).unwrap();

        // Group B charges fee — must see group A's treasury credit
        proc_b.charge_fee_direct(&payer_b, fee).unwrap();

        // Treasury should have received BOTH fee credits (minus burned portion)
        let final_treasury = state.get_account(&treasury).unwrap().unwrap();
        assert!(
            final_treasury.spores > 0,
            "Treasury must have received fee credits"
        );
        // Both payers should have been debited exactly 1 LICN
        let payer_a_bal = state.get_account(&payer_a).unwrap().unwrap().spores;
        let payer_b_bal = state.get_account(&payer_b).unwrap().unwrap().spores;
        assert_eq!(payer_a_bal, initial_spores - fee);
        assert_eq!(payer_b_bal, initial_spores - fee);
    }

    /// AUDIT-FIX B-5: Fee split percentages are capped so total distributed
    /// never exceeds the original fee amount.
    #[test]
    fn test_fee_split_capped_no_spore_creation() {
        let (processor, state, _alice_kp, _alice, treasury, _genesis_hash) = setup();

        // Set up a payer with known balance (10 LICN)
        let payer = Pubkey([99u8; 32]);
        state.put_account(&payer, &Account::new(10, payer)).unwrap();

        let fee = Account::licn_to_spores(1); // 1 LICN
        let treasury_before = state.get_account(&treasury).unwrap().unwrap().spores;

        processor.charge_fee_direct(&payer, fee).unwrap();

        let treasury_after = state.get_account(&treasury).unwrap().unwrap().spores;
        let treasury_gain = treasury_after - treasury_before;
        let burned = state.get_total_burned().unwrap_or(0);

        // Treasury gain + burned must not exceed the fee charged
        assert!(
            treasury_gain.saturating_add(burned) <= fee,
            "Treasury gain ({}) + burned ({}) must not exceed fee ({})",
            treasury_gain,
            burned,
            fee
        );
    }

    // ====================================================================
    // SYSTEM CREATE ACCOUNT (type 1)
    // ====================================================================

    /// Helper: wrap a single instruction into a signed transaction
    fn make_signed_tx(kp: &Keypair, ix: Instruction, recent_blockhash: Hash) -> Transaction {
        let message = crate::transaction::Message::new(vec![ix], recent_blockhash);
        let mut tx = Transaction::new(message);
        let sig = kp.sign(&tx.message.serialize());
        tx.signatures.push(sig);
        tx
    }

    #[test]
    fn test_create_account_success() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let new_kp = Keypair::generate();
        let new_acct = new_kp.pubkey();
        let validator = Pubkey([42u8; 32]);

        // Two instructions: 1-spore transfer (fee payer = alice), create_account (signer = new_acct)
        let message = crate::transaction::Message::new(
            vec![
                Instruction {
                    program_id: SYSTEM_PROGRAM_ID,
                    accounts: vec![alice, alice],
                    data: {
                        let mut d = vec![0u8];
                        d.extend_from_slice(&1u64.to_le_bytes());
                        d
                    },
                },
                Instruction {
                    program_id: SYSTEM_PROGRAM_ID,
                    accounts: vec![new_acct],
                    data: vec![1],
                },
            ],
            genesis_hash,
        );
        let mut tx = Transaction::new(message);
        let msg_bytes = tx.message.serialize();
        tx.signatures.push(alice_kp.sign(&msg_bytes));
        tx.signatures.push(new_kp.sign(&msg_bytes));

        let result = processor.process_transaction(&tx, &validator);
        assert!(
            result.success,
            "Create account should succeed: {:?}",
            result.error
        );

        let acct = state.get_account(&new_acct).unwrap();
        assert!(acct.is_some(), "New account must exist after creation");
        assert_eq!(acct.unwrap().spores, 0, "New account should have 0 balance");
    }

    #[test]
    fn test_create_account_already_exists() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let existing_kp = Keypair::generate();
        let existing = existing_kp.pubkey();
        let validator = Pubkey([42u8; 32]);

        // Pre-create the account
        state
            .put_account(&existing, &Account::new(10, existing))
            .unwrap();

        let message = crate::transaction::Message::new(
            vec![
                Instruction {
                    program_id: SYSTEM_PROGRAM_ID,
                    accounts: vec![alice, alice],
                    data: {
                        let mut d = vec![0u8];
                        d.extend_from_slice(&1u64.to_le_bytes());
                        d
                    },
                },
                Instruction {
                    program_id: SYSTEM_PROGRAM_ID,
                    accounts: vec![existing],
                    data: vec![1],
                },
            ],
            genesis_hash,
        );
        let mut tx = Transaction::new(message);
        let msg_bytes = tx.message.serialize();
        tx.signatures.push(alice_kp.sign(&msg_bytes));
        tx.signatures.push(existing_kp.sign(&msg_bytes));

        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Create existing account should fail");
        assert!(
            result.error.as_ref().unwrap().contains("already exists"),
            "Expected 'already exists', got: {:?}",
            result.error
        );
    }

    // ====================================================================
    // TREASURY TRANSFERS (types 2-5)
    // ====================================================================

    #[test]
    fn test_treasury_transfer_from_treasury_succeeds() {
        let (processor, state, _alice_kp, _alice, treasury, genesis_hash) = setup();
        let bob = Pubkey([52u8; 32]);
        let validator = Pubkey([42u8; 32]);

        // Fund treasury
        state
            .put_account(&treasury, &Account::new(1_000_000, treasury))
            .unwrap();

        // Treasury keypair needed to sign
        let treasury_kp = Keypair::generate();
        let treasury_pub = treasury_kp.pubkey();
        state.set_treasury_pubkey(&treasury_pub).unwrap();
        let t_acct2 = Account::new(1_000_000, treasury_pub);
        state.put_account(&treasury_pub, &t_acct2).unwrap();

        let amount = Account::licn_to_spores(100);
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![treasury_pub, bob],
            data: {
                let mut d = vec![2u8]; // type 2 = treasury transfer
                d.extend_from_slice(&amount.to_le_bytes());
                d
            },
        };
        let tx = make_signed_tx(&treasury_kp, ix, genesis_hash);

        let result = processor.process_transaction(&tx, &validator);
        assert!(
            result.success,
            "Treasury transfer should succeed: {:?}",
            result.error
        );
        assert_eq!(state.get_balance(&bob).unwrap(), amount);
    }

    #[test]
    fn test_treasury_transfer_from_non_treasury_rejected() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([53u8; 32]);
        let validator = Pubkey([42u8; 32]);

        let amount = Account::licn_to_spores(10);
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, bob],
            data: {
                let mut d = vec![3u8]; // type 3 = treasury transfer
                d.extend_from_slice(&amount.to_le_bytes());
                d
            },
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);

        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Non-treasury should not use types 2-5");
        assert!(result.error.unwrap().contains("restricted to treasury"));
    }

    // ====================================================================
    // NFT OPERATIONS (types 6, 7, 8)
    // ====================================================================

    /// Helper: create a collection and return the collection account pubkey.
    /// NOTE: Funds the creator with extra LICN to cover the 1000 LICN collection fee.
    fn create_test_collection(
        processor: &TxProcessor,
        state: &StateStore,
        creator_kp: &Keypair,
        creator: Pubkey,
        collection_addr: Pubkey,
        genesis_hash: Hash,
    ) -> TxResult {
        // Ensure creator has enough for the collection fee (1000 LICN) + base fee
        state
            .put_account(&creator, &Account::new(10_000, creator))
            .unwrap();
        let col_data = crate::nft::CreateCollectionData {
            name: "TestCollection".to_string(),
            symbol: "TNFT".to_string(),
            royalty_bps: 500,
            max_supply: 100,
            public_mint: true,
            mint_authority: None,
        };
        let encoded = bincode::serialize(&col_data).unwrap();
        let mut data = vec![6u8];
        data.extend_from_slice(&encoded);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![creator, collection_addr],
            data,
        };
        let tx = make_signed_tx(creator_kp, ix, genesis_hash);
        processor.process_transaction(&tx, &Pubkey([42u8; 32]))
    }

    #[test]
    fn test_create_collection_success() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let collection = Pubkey([60u8; 32]);

        let result = create_test_collection(
            &processor,
            &state,
            &alice_kp,
            alice,
            collection,
            genesis_hash,
        );
        assert!(
            result.success,
            "Collection creation should succeed: {:?}",
            result.error
        );

        let acct = state.get_account(&collection).unwrap().unwrap();
        let col_state = crate::nft::decode_collection_state(&acct.data).unwrap();
        assert_eq!(col_state.name, "TestCollection");
        assert_eq!(col_state.symbol, "TNFT");
        assert_eq!(col_state.creator, alice);
        assert_eq!(col_state.max_supply, 100);
        assert_eq!(col_state.minted, 0);
    }

    #[test]
    fn test_create_collection_duplicate_rejected() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let collection = Pubkey([61u8; 32]);

        // First creation succeeds
        let r1 = create_test_collection(
            &processor,
            &state,
            &alice_kp,
            alice,
            collection,
            genesis_hash,
        );
        assert!(r1.success, "First creation should succeed: {:?}", r1.error);

        // Ensure alice has balance for the second attempt
        state
            .put_account(&alice, &Account::new(10_000, alice))
            .unwrap();

        // Try to create again with slightly different data to avoid replay protection
        let col_data = crate::nft::CreateCollectionData {
            name: "TestCollection2".to_string(),
            symbol: "TNFT".to_string(),
            royalty_bps: 500,
            max_supply: 100,
            public_mint: true,
            mint_authority: None,
        };
        let encoded = bincode::serialize(&col_data).unwrap();
        let mut data = vec![6u8];
        data.extend_from_slice(&encoded);
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, collection],
            data,
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let r2 = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!r2.success, "Duplicate collection should fail");
        assert!(
            r2.error.as_ref().unwrap().contains("already exists"),
            "Expected 'already exists', got: {:?}",
            r2.error
        );
    }

    #[test]
    fn test_mint_nft_success() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let collection = Pubkey([62u8; 32]);
        let token_addr = Pubkey([63u8; 32]);

        // Create collection first
        let r = create_test_collection(
            &processor,
            &state,
            &alice_kp,
            alice,
            collection,
            genesis_hash,
        );
        assert!(
            r.success,
            "Setup: collection creation failed: {:?}",
            r.error
        );

        // Mint NFT
        let mint_data = crate::nft::MintNftData {
            token_id: 1,
            metadata_uri: "https://example.com/nft/1.json".to_string(),
        };
        let encoded = bincode::serialize(&mint_data).unwrap();
        let mut data = vec![7u8];
        data.extend_from_slice(&encoded);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, collection, token_addr, alice], // minter, collection, token, owner
            data,
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(result.success, "Mint should succeed: {:?}", result.error);

        // Verify token state
        let token_acct = state.get_account(&token_addr).unwrap().unwrap();
        let token_state = crate::nft::decode_token_state(&token_acct.data).unwrap();
        assert_eq!(token_state.owner, alice);
        assert_eq!(token_state.collection, collection);
        assert_eq!(token_state.token_id, 1);

        // Verify collection minted count incremented
        let col_acct = state.get_account(&collection).unwrap().unwrap();
        let col_state = crate::nft::decode_collection_state(&col_acct.data).unwrap();
        assert_eq!(col_state.minted, 1);
    }

    #[test]
    fn test_mint_nft_duplicate_token_id_rejected() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let collection = Pubkey([64u8; 32]);
        let token1 = Pubkey([65u8; 32]);
        let token2 = Pubkey([66u8; 32]);

        // Create collection + mint token_id=1
        create_test_collection(
            &processor,
            &state,
            &alice_kp,
            alice,
            collection,
            genesis_hash,
        );
        let mint_data = crate::nft::MintNftData {
            token_id: 1,
            metadata_uri: "https://example.com/1.json".to_string(),
        };
        let encoded = bincode::serialize(&mint_data).unwrap();
        let mut data = vec![7u8];
        data.extend_from_slice(&encoded);

        let ix1 = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, collection, token1, alice],
            data: data.clone(),
        };
        let tx1 = make_signed_tx(&alice_kp, ix1, genesis_hash);
        let r1 = processor.process_transaction(&tx1, &Pubkey([42u8; 32]));
        assert!(r1.success, "First mint should succeed");

        // Mint with same token_id=1 but different token address
        let ix2 = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, collection, token2, alice],
            data,
        };
        let tx2 = make_signed_tx(&alice_kp, ix2, genesis_hash);
        let r2 = processor.process_transaction(&tx2, &Pubkey([42u8; 32]));
        assert!(!r2.success, "Duplicate token_id should fail");
        assert!(r2.error.unwrap().contains("already exists"));
    }

    #[test]
    fn test_transfer_nft_success() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([67u8; 32]);
        let collection = Pubkey([68u8; 32]);
        let token_addr = Pubkey([69u8; 32]);

        // Create collection + mint
        create_test_collection(
            &processor,
            &state,
            &alice_kp,
            alice,
            collection,
            genesis_hash,
        );
        let mint_data = crate::nft::MintNftData {
            token_id: 1,
            metadata_uri: "https://example.com/1.json".to_string(),
        };
        let mut mdata = vec![7u8];
        mdata.extend_from_slice(&bincode::serialize(&mint_data).unwrap());
        let ix_mint = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, collection, token_addr, alice],
            data: mdata,
        };
        let tx_mint = make_signed_tx(&alice_kp, ix_mint, genesis_hash);
        let r = processor.process_transaction(&tx_mint, &Pubkey([42u8; 32]));
        assert!(r.success, "Mint failed: {:?}", r.error);

        // Transfer NFT from alice to bob
        let ix_transfer = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, token_addr, bob],
            data: vec![8u8],
        };
        let tx_transfer = make_signed_tx(&alice_kp, ix_transfer, genesis_hash);
        let result = processor.process_transaction(&tx_transfer, &Pubkey([42u8; 32]));
        assert!(
            result.success,
            "NFT transfer should succeed: {:?}",
            result.error
        );

        let token_acct = state.get_account(&token_addr).unwrap().unwrap();
        let token_state = crate::nft::decode_token_state(&token_acct.data).unwrap();
        assert_eq!(token_state.owner, bob, "Owner should be bob after transfer");
    }

    #[test]
    fn test_transfer_nft_unauthorized_rejected() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let collection = Pubkey([70u8; 32]);
        let token_addr = Pubkey([71u8; 32]);
        let bob = Pubkey([72u8; 32]);
        let eve_kp = Keypair::generate();
        let eve = eve_kp.pubkey();
        state.put_account(&eve, &Account::new(100, eve)).unwrap();

        // Create + mint (alice owns)
        create_test_collection(
            &processor,
            &state,
            &alice_kp,
            alice,
            collection,
            genesis_hash,
        );
        let mint_data = crate::nft::MintNftData {
            token_id: 1,
            metadata_uri: "uri".to_string(),
        };
        let mut mdata = vec![7u8];
        mdata.extend_from_slice(&bincode::serialize(&mint_data).unwrap());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, collection, token_addr, alice],
            data: mdata,
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let r = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(r.success, "Mint should succeed: {:?}", r.error);

        // Eve tries to transfer alice's NFT
        let ix_transfer = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![eve, token_addr, bob],
            data: vec![8u8],
        };
        let tx_transfer = make_signed_tx(&eve_kp, ix_transfer, genesis_hash);
        let result = processor.process_transaction(&tx_transfer, &Pubkey([42u8; 32]));
        assert!(!result.success, "Eve should not transfer alice's NFT");
        assert!(
            result.error.as_ref().unwrap().contains("Unauthorized"),
            "Expected 'Unauthorized', got: {:?}",
            result.error
        );
    }

    // ====================================================================
    // STAKING OPERATIONS (types 9, 10, 11)
    // ====================================================================

    /// Helper: set up a validator in the stake pool so staking tests can run
    fn setup_validator_in_pool(state: &StateStore, validator: Pubkey) {
        let mut pool = state.get_stake_pool().unwrap_or_default();
        // Insert validator with MIN_VALIDATOR_STAKE so the validator entry exists
        pool.upsert_stake(validator, crate::consensus::MIN_VALIDATOR_STAKE, 0);
        state.put_stake_pool(&pool).unwrap();
    }

    fn make_stake_tx(
        kp: &Keypair,
        staker: Pubkey,
        validator: Pubkey,
        amount: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![staker, validator],
            data: {
                let mut d = vec![9u8];
                d.extend_from_slice(&amount.to_le_bytes());
                d
            },
        };
        make_signed_tx(kp, ix, recent_blockhash)
    }

    fn make_request_unstake_tx(
        kp: &Keypair,
        staker: Pubkey,
        validator: Pubkey,
        amount: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![staker, validator],
            data: {
                let mut d = vec![10u8];
                d.extend_from_slice(&amount.to_le_bytes());
                d
            },
        };
        make_signed_tx(kp, ix, recent_blockhash)
    }

    fn make_claim_unstake_tx(
        kp: &Keypair,
        staker: Pubkey,
        validator: Pubkey,
        recent_blockhash: Hash,
    ) -> Transaction {
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![staker, validator],
            data: vec![11u8],
        };
        make_signed_tx(kp, ix, recent_blockhash)
    }

    fn make_register_validator_tx(
        kp: &Keypair,
        validator: Pubkey,
        fingerprint: [u8; 32],
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![26u8];
        data.extend_from_slice(&fingerprint);
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![validator],
            data,
        };
        make_signed_tx(kp, ix, recent_blockhash)
    }

    fn make_deregister_validator_tx(
        kp: &Keypair,
        validator: Pubkey,
        recent_blockhash: Hash,
    ) -> Transaction {
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![validator],
            data: vec![31u8],
        };
        make_signed_tx(kp, ix, recent_blockhash)
    }

    fn fund_treasury_for_validator_bootstrap(state: &StateStore, treasury: Pubkey) {
        state
            .put_account(&treasury, &Account::new(500_000, treasury))
            .unwrap();
    }

    fn assert_validator_registration_not_granted(
        state: &StateStore,
        treasury: Pubkey,
        before_treasury: &Account,
        validator: Pubkey,
        fingerprint: [u8; 32],
    ) {
        let after_treasury = state.get_account(&treasury).unwrap().unwrap();
        assert_eq!(after_treasury.spores, before_treasury.spores);
        assert_eq!(after_treasury.spendable, before_treasury.spendable);
        assert!(state.get_account(&validator).unwrap().is_none());
        let pool = state.get_stake_pool().unwrap();
        assert_eq!(pool.bootstrap_grants_issued(), 0);
        assert!(pool.get_stake(&validator).is_none());
        assert!(pool.fingerprint_owner(&fingerprint).is_none());
    }

    #[test]
    fn test_stake_success() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Register validator in pool
        setup_validator_in_pool(&state, validator);

        // Fund alice with enough for MIN_VALIDATOR_STAKE (75K LICN)
        state
            .put_account(&alice, &Account::new(100_000, alice))
            .unwrap();

        // Stake at MIN_VALIDATOR_STAKE
        let amount = crate::consensus::MIN_VALIDATOR_STAKE;
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, validator],
            data: {
                let mut d = vec![9u8];
                d.extend_from_slice(&amount.to_le_bytes());
                d
            },
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(result.success, "Staking should succeed: {:?}", result.error);

        // Verify alice's staked balance
        let acct = state.get_account(&alice).unwrap().unwrap();
        assert_eq!(
            acct.staked, amount,
            "Staked balance should equal MIN_VALIDATOR_STAKE"
        );

        // Verify stake pool updated
        let pool = state.get_stake_pool().unwrap();
        let stake_info = pool.get_stake(&validator).unwrap();
        assert!(
            stake_info.amount >= amount,
            "Stake pool should reflect the staked amount"
        );
    }

    #[test]
    fn test_stake_to_unregistered_validator_rejected() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let fake_validator = Pubkey([99u8; 32]); // Not in stake pool

        let amount = Account::licn_to_spores(100);
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, fake_validator],
            data: {
                let mut d = vec![9u8];
                d.extend_from_slice(&amount.to_le_bytes());
                d
            },
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(
            !result.success,
            "Staking to unregistered validator should fail"
        );
        assert!(result.error.unwrap().contains("not registered"));
    }

    #[test]
    fn test_stake_rejects_outgoing_restricted_staker_without_pool_mutation() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        setup_validator_in_pool(&state, validator);
        state
            .put_account(&alice, &Account::new(100_000, alice))
            .unwrap();
        let before_pool_stake = state
            .get_stake_pool()
            .unwrap()
            .get_stake(&validator)
            .unwrap()
            .amount;

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::OutgoingOnly,
        );

        let tx = make_stake_tx(
            &alice_kp,
            alice,
            validator,
            crate::consensus::MIN_VALIDATOR_STAKE,
            genesis_hash,
        );
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Stake blocked by active staker account restriction"));

        let after_account = state.get_account(&alice).unwrap().unwrap();
        assert_eq!(after_account.staked, 0);
        assert_eq!(after_account.locked, 0);
        let after_pool_stake = state
            .get_stake_pool()
            .unwrap()
            .get_stake(&validator)
            .unwrap()
            .amount;
        assert_eq!(after_pool_stake, before_pool_stake);
    }

    #[test]
    fn test_stake_rejects_native_frozen_amount_without_pool_mutation() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        setup_validator_in_pool(&state, validator);
        state
            .put_account(&alice, &Account::new(100_000, alice))
            .unwrap();
        let before_pool_stake = state
            .get_stake_pool()
            .unwrap()
            .get_stake(&validator)
            .unwrap()
            .amount;
        let frozen_amount = state.get_account(&alice).unwrap().unwrap().spendable;

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::AccountAsset {
                account: alice,
                asset: NATIVE_LICN_ASSET_ID,
            },
            RestrictionMode::FrozenAmount {
                amount: frozen_amount,
            },
        );

        let tx = make_stake_tx(
            &alice_kp,
            alice,
            validator,
            crate::consensus::MIN_VALIDATOR_STAKE,
            genesis_hash,
        );
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Stake blocked by active staker native account-asset restriction"));

        let after_account = state.get_account(&alice).unwrap().unwrap();
        assert_eq!(after_account.staked, 0);
        assert_eq!(
            state
                .get_stake_pool()
                .unwrap()
                .get_stake(&validator)
                .unwrap()
                .amount,
            before_pool_stake
        );
    }

    #[test]
    fn test_staking_protocol_pause_rejects_stake_without_pool_mutation() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        setup_validator_in_pool(&state, validator);
        state
            .put_account(&alice, &Account::new(100_000, alice))
            .unwrap();
        let before_pool_stake = state
            .get_stake_pool()
            .unwrap()
            .get_stake(&validator)
            .unwrap()
            .amount;

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::ProtocolModule(ProtocolModuleId::Staking),
            RestrictionMode::ProtocolPaused,
        );

        let tx = make_stake_tx(
            &alice_kp,
            alice,
            validator,
            crate::consensus::MIN_VALIDATOR_STAKE,
            genesis_hash,
        );
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Stake blocked by active Staking protocol pause"));

        let after_account = state.get_account(&alice).unwrap().unwrap();
        assert_eq!(after_account.staked, 0);
        assert_eq!(
            state
                .get_stake_pool()
                .unwrap()
                .get_stake(&validator)
                .unwrap()
                .amount,
            before_pool_stake
        );
    }

    #[test]
    fn test_request_unstake_success() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        setup_validator_in_pool(&state, validator);

        // Fund alice
        state
            .put_account(&alice, &Account::new(100_000, alice))
            .unwrap();

        // Stake MIN_VALIDATOR_STAKE first
        let amount = crate::consensus::MIN_VALIDATOR_STAKE;
        let ix_stake = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, validator],
            data: {
                let mut d = vec![9u8];
                d.extend_from_slice(&amount.to_le_bytes());
                d
            },
        };
        let tx_stake = make_signed_tx(&alice_kp, ix_stake, genesis_hash);
        let r = processor.process_transaction(&tx_stake, &validator);
        assert!(r.success, "Stake should succeed: {:?}", r.error);

        // Request unstake — partial amount to avoid going below minimum
        let unstake_amount = amount / 2;
        let ix_unstake = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, validator],
            data: {
                let mut d = vec![10u8];
                d.extend_from_slice(&unstake_amount.to_le_bytes());
                d
            },
        };
        let tx_unstake = make_signed_tx(&alice_kp, ix_unstake, genesis_hash);
        let result = processor.process_transaction(&tx_unstake, &validator);
        assert!(result.success, "Unstake should succeed: {:?}", result.error);

        // Verify staked balance decreased and locked increased
        let acct = state.get_account(&alice).unwrap().unwrap();
        assert_eq!(
            acct.staked,
            amount - unstake_amount,
            "Staked should be reduced"
        );
        assert_eq!(
            acct.locked, unstake_amount,
            "Locked should equal unstaked amount"
        );
    }

    #[test]
    fn test_request_unstake_rejects_outgoing_restricted_staker_without_request() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        setup_validator_in_pool(&state, validator);
        state
            .put_account(&alice, &Account::new(100_000, alice))
            .unwrap();

        let amount = crate::consensus::MIN_VALIDATOR_STAKE;
        let stake_tx = make_stake_tx(&alice_kp, alice, validator, amount, genesis_hash);
        assert!(processor.process_transaction(&stake_tx, &validator).success);
        let before_account = state.get_account(&alice).unwrap().unwrap();
        let before_pool = state.get_stake_pool().unwrap();
        assert!(before_pool.get_unstake_request(&validator).is_none());

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::OutgoingOnly,
        );

        let unstake_amount = amount / 2;
        let tx = make_request_unstake_tx(&alice_kp, alice, validator, unstake_amount, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("RequestUnstake blocked by active staker account restriction"));

        let after_account = state.get_account(&alice).unwrap().unwrap();
        assert_eq!(after_account.staked, before_account.staked);
        assert_eq!(after_account.locked, before_account.locked);
        let after_pool = state.get_stake_pool().unwrap();
        assert!(after_pool.get_unstake_request(&validator).is_none());
        assert_eq!(
            after_pool.get_stake(&validator).unwrap().amount,
            before_pool.get_stake(&validator).unwrap().amount
        );
    }

    #[test]
    fn test_request_unstake_insufficient_rejected() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        setup_validator_in_pool(&state, validator);

        // Fund alice
        state
            .put_account(&alice, &Account::new(100_000, alice))
            .unwrap();

        // Stake MIN_VALIDATOR_STAKE
        let stake_amount = crate::consensus::MIN_VALIDATOR_STAKE;
        let ix_stake = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, validator],
            data: {
                let mut d = vec![9u8];
                d.extend_from_slice(&stake_amount.to_le_bytes());
                d
            },
        };
        let tx = make_signed_tx(&alice_kp, ix_stake, genesis_hash);
        let r = processor.process_transaction(&tx, &validator);
        assert!(r.success, "Stake should succeed: {:?}", r.error);

        // Try to unstake more than staked
        let too_much = Account::licn_to_spores(100_000);
        let ix_unstake = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, validator],
            data: {
                let mut d = vec![10u8];
                d.extend_from_slice(&too_much.to_le_bytes());
                d
            },
        };
        let tx2 = make_signed_tx(&alice_kp, ix_unstake, genesis_hash);
        let result = processor.process_transaction(&tx2, &validator);
        assert!(!result.success, "Unstaking more than staked should fail");
        assert!(
            result.error.as_ref().unwrap().contains("Insufficient"),
            "Expected 'Insufficient', got: {:?}",
            result.error
        );
    }

    #[test]
    fn test_claim_unstake_before_cooldown_rejected() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        setup_validator_in_pool(&state, validator);

        // Fund alice
        state
            .put_account(&alice, &Account::new(200_000, alice))
            .unwrap();

        // Stake MIN_VALIDATOR_STAKE
        let amount = crate::consensus::MIN_VALIDATOR_STAKE;
        let ix_s = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, validator],
            data: {
                let mut d = vec![9u8];
                d.extend_from_slice(&amount.to_le_bytes());
                d
            },
        };
        let r = processor
            .process_transaction(&make_signed_tx(&alice_kp, ix_s, genesis_hash), &validator);
        assert!(r.success, "Stake failed: {:?}", r.error);

        // Request unstake — half
        let unstake_amount = amount / 2;
        let ix_u = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, validator],
            data: {
                let mut d = vec![10u8];
                d.extend_from_slice(&unstake_amount.to_le_bytes());
                d
            },
        };
        let r2 = processor
            .process_transaction(&make_signed_tx(&alice_kp, ix_u, genesis_hash), &validator);
        assert!(r2.success, "Unstake request failed: {:?}", r2.error);

        // Immediately try to claim (cooldown not passed — slot is still 0)
        let ix_claim = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, validator],
            data: vec![11u8],
        };
        let tx_claim = make_signed_tx(&alice_kp, ix_claim, genesis_hash);
        let result = processor.process_transaction(&tx_claim, &validator);
        assert!(!result.success, "Claim before cooldown should fail");
    }

    #[test]
    fn test_claim_unstake_rejects_incoming_restricted_staker_without_unlocking() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        setup_validator_in_pool(&state, validator);
        state
            .put_account(&alice, &Account::new(200_000, alice))
            .unwrap();

        let amount = crate::consensus::MIN_VALIDATOR_STAKE;
        let stake_tx = make_stake_tx(&alice_kp, alice, validator, amount, genesis_hash);
        assert!(processor.process_transaction(&stake_tx, &validator).success);
        let unstake_amount = amount / 2;
        let unstake_tx =
            make_request_unstake_tx(&alice_kp, alice, validator, unstake_amount, genesis_hash);
        assert!(
            processor
                .process_transaction(&unstake_tx, &validator)
                .success
        );
        let before_account = state.get_account(&alice).unwrap().unwrap();
        assert_eq!(before_account.locked, unstake_amount);
        assert!(state
            .get_stake_pool()
            .unwrap()
            .get_unstake_request(&validator)
            .is_some());

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::IncomingOnly,
        );
        let future_hash = advance_test_slot(&state, crate::consensus::UNSTAKE_COOLDOWN_SLOTS + 1);
        let claim_tx = make_claim_unstake_tx(&alice_kp, alice, validator, future_hash);
        let result = processor.process_transaction(&claim_tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("ClaimUnstake blocked by active staker account restriction"));

        let after_account = state.get_account(&alice).unwrap().unwrap();
        assert_eq!(after_account.staked, before_account.staked);
        assert_eq!(after_account.locked, before_account.locked);
        assert!(state
            .get_stake_pool()
            .unwrap()
            .get_unstake_request(&validator)
            .is_some());
    }

    #[test]
    fn test_register_validator_rejects_treasury_outgoing_restriction_without_grant() {
        let (processor, state, _alice_kp, _alice, treasury, genesis_hash) = setup();
        let block_producer = Pubkey([42u8; 32]);
        fund_treasury_for_validator_bootstrap(&state, treasury);
        let before_treasury = state.get_account(&treasury).unwrap().unwrap();
        let validator_kp = Keypair::generate();
        let validator = validator_kp.pubkey();
        let fingerprint = [0x31; 32];

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(treasury),
            RestrictionMode::OutgoingOnly,
        );

        let tx = make_register_validator_tx(&validator_kp, validator, fingerprint, genesis_hash);
        let result = processor.process_transaction(&tx, &block_producer);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("RegisterValidator blocked by active treasury account restriction"));
        assert_validator_registration_not_granted(
            &state,
            treasury,
            &before_treasury,
            validator,
            fingerprint,
        );
    }

    #[test]
    fn test_register_validator_rejects_treasury_native_frozen_amount_without_grant() {
        let (processor, state, _alice_kp, _alice, treasury, genesis_hash) = setup();
        let block_producer = Pubkey([42u8; 32]);
        fund_treasury_for_validator_bootstrap(&state, treasury);
        let before_treasury = state.get_account(&treasury).unwrap().unwrap();
        let validator_kp = Keypair::generate();
        let validator = validator_kp.pubkey();
        let fingerprint = [0x32; 32];

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::AccountAsset {
                account: treasury,
                asset: NATIVE_LICN_ASSET_ID,
            },
            RestrictionMode::FrozenAmount {
                amount: before_treasury.spendable,
            },
        );

        let tx = make_register_validator_tx(&validator_kp, validator, fingerprint, genesis_hash);
        let result = processor.process_transaction(&tx, &block_producer);
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains(
            "RegisterValidator blocked by active treasury native account-asset restriction"
        ));
        assert_validator_registration_not_granted(
            &state,
            treasury,
            &before_treasury,
            validator,
            fingerprint,
        );
    }

    #[test]
    fn test_register_validator_rejects_incoming_restricted_validator_without_grant() {
        let (processor, state, _alice_kp, _alice, treasury, genesis_hash) = setup();
        let block_producer = Pubkey([42u8; 32]);
        fund_treasury_for_validator_bootstrap(&state, treasury);
        let before_treasury = state.get_account(&treasury).unwrap().unwrap();
        let validator_kp = Keypair::generate();
        let validator = validator_kp.pubkey();
        let fingerprint = [0x33; 32];

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(validator),
            RestrictionMode::IncomingOnly,
        );

        let tx = make_register_validator_tx(&validator_kp, validator, fingerprint, genesis_hash);
        let result = processor.process_transaction(&tx, &block_producer);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("RegisterValidator blocked by active validator account restriction"));
        assert_validator_registration_not_granted(
            &state,
            treasury,
            &before_treasury,
            validator,
            fingerprint,
        );
    }

    #[test]
    fn test_register_validator_protocol_pause_rejects_without_grant() {
        let (processor, state, _alice_kp, _alice, treasury, genesis_hash) = setup();
        let block_producer = Pubkey([42u8; 32]);
        fund_treasury_for_validator_bootstrap(&state, treasury);
        let before_treasury = state.get_account(&treasury).unwrap().unwrap();
        let validator_kp = Keypair::generate();
        let validator = validator_kp.pubkey();
        let fingerprint = [0x34; 32];

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::ProtocolModule(ProtocolModuleId::Staking),
            RestrictionMode::ProtocolPaused,
        );

        let tx = make_register_validator_tx(&validator_kp, validator, fingerprint, genesis_hash);
        let result = processor.process_transaction(&tx, &block_producer);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("RegisterValidator blocked by active Staking protocol pause"));
        assert_validator_registration_not_granted(
            &state,
            treasury,
            &before_treasury,
            validator,
            fingerprint,
        );
    }

    #[test]
    fn test_deregister_validator_protocol_pause_rejects_without_deactivation() {
        let (processor, state, _alice_kp, _alice, _treasury, genesis_hash) = setup();
        let block_producer = Pubkey([42u8; 32]);
        let validator_kp = Keypair::generate();
        let validator = validator_kp.pubkey();
        setup_active_validator(&state, &validator, MIN_VALIDATOR_STAKE);
        assert!(
            state
                .get_stake_pool()
                .unwrap()
                .get_stake(&validator)
                .unwrap()
                .is_active
        );

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::ProtocolModule(ProtocolModuleId::Staking),
            RestrictionMode::ProtocolPaused,
        );

        let tx = make_deregister_validator_tx(&validator_kp, validator, genesis_hash);
        let result = processor.process_transaction(&tx, &block_producer);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("DeregisterValidator blocked by active Staking protocol pause"));

        let pool = state.get_stake_pool().unwrap();
        assert!(pool.get_stake(&validator).unwrap().is_active);
        assert!(state.get_pending_validator_changes(1).unwrap().is_empty());
    }

    // ====================================================================
    // EVM ADDRESS REGISTRATION (type 12)
    // ====================================================================

    #[test]
    fn test_register_evm_address_success() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();

        let evm_addr: [u8; 20] = [
            0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99,
            0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
        ];

        let mut data = vec![12u8];
        data.extend_from_slice(&evm_addr);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data,
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(
            result.success,
            "EVM registration should succeed: {:?}",
            result.error
        );

        // Verify mapping exists
        let mapped = state.lookup_evm_address(&evm_addr).unwrap();
        assert_eq!(mapped, Some(alice));
    }

    #[test]
    fn test_register_evm_address_duplicate_rejected() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        state.put_account(&bob, &Account::new(100, bob)).unwrap();

        let evm_addr: [u8; 20] = [0x11; 20];

        // Alice registers
        let mut data = vec![12u8];
        data.extend_from_slice(&evm_addr);
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: data.clone(),
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let r1 = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(r1.success);

        // Bob tries to register same EVM address
        let ix2 = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data,
        };
        let tx2 = make_signed_tx(&bob_kp, ix2, genesis_hash);
        let r2 = processor.process_transaction(&tx2, &Pubkey([42u8; 32]));
        assert!(!r2.success, "Duplicate EVM mapping should fail");
        assert!(r2.error.unwrap().contains("already mapped"));
    }

    #[test]
    fn test_register_evm_address_invalid_data_rejected() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();

        // Only 10 bytes instead of required 21 (type + 20 addr bytes)
        let mut data = vec![12u8];
        data.extend_from_slice(&[0xAA; 10]);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data,
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!result.success, "Invalid EVM data should fail");
        assert!(
            result
                .error
                .as_ref()
                .unwrap()
                .contains("Invalid EVM address data"),
            "Expected 'Invalid EVM address data', got: {:?}",
            result.error
        );
    }

    // ====================================================================
    // MOSSSTAKE TRANSFER (type 16)
    // ====================================================================

    #[test]
    fn test_mossstake_transfer_success() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([80u8; 32]);

        // Deposit first: alice deposits 100 LICN into MossStake
        let deposit_amount = Account::licn_to_spores(100);
        let ix_deposit = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: {
                let mut d = vec![13u8]; // MossStake deposit
                d.extend_from_slice(&deposit_amount.to_le_bytes());
                d
            },
        };
        let tx_dep = make_signed_tx(&alice_kp, ix_deposit, genesis_hash);
        let r = processor.process_transaction(&tx_dep, &Pubkey([42u8; 32]));
        assert!(r.success, "Deposit should succeed: {:?}", r.error);

        // Get alice's stLICN balance
        let pool = state.get_mossstake_pool().unwrap();
        let (alice_pos, _) = pool
            .get_position(&alice)
            .expect("Alice should have a position after deposit");
        let alice_st_licn = alice_pos.st_licn_amount;
        assert!(alice_st_licn > 0, "Alice should have stLICN after deposit");

        // Transfer half the stLICN to bob
        let transfer_amount = alice_st_licn / 2;
        let ix_transfer = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, bob],
            data: {
                let mut d = vec![16u8]; // MossStake transfer
                d.extend_from_slice(&transfer_amount.to_le_bytes());
                d
            },
        };
        let tx_xfer = make_signed_tx(&alice_kp, ix_transfer, genesis_hash);
        let result = processor.process_transaction(&tx_xfer, &Pubkey([42u8; 32]));
        assert!(
            result.success,
            "MossStake transfer should succeed: {:?}",
            result.error
        );

        // Verify balances
        let pool2 = state.get_mossstake_pool().unwrap();
        let (bob_pos, _) = pool2
            .get_position(&bob)
            .expect("Bob should have a position after transfer");
        let bob_st_licn = bob_pos.st_licn_amount;
        assert_eq!(
            bob_st_licn, transfer_amount,
            "Bob should have received stLICN"
        );
    }

    // ====================================================================
    // REGISTER SYMBOL (type 20)
    // ====================================================================

    /// Helper: create a fake deployed contract account for symbol registration
    fn deploy_fake_contract(state: &StateStore, owner: Pubkey, contract_id: Pubkey) {
        let contract = crate::ContractAccount {
            code: vec![0x00, 0x61, 0x73, 0x6d], // Minimal WASM header
            storage: std::collections::HashMap::new(),
            owner,
            code_hash: Hash::hash(b"test_code"),
            abi: None,
            version: 1,
            previous_code_hash: None,
            upgrade_timelock_epochs: None,
            pending_upgrade: None,
            lifecycle_status: crate::ContractLifecycleStatus::Active,
            lifecycle_updated_slot: 0,
            lifecycle_restriction_id: None,
        };
        let mut acct = Account::new(0, contract_id);
        acct.executable = true;
        acct.data = serde_json::to_vec(&contract).unwrap();
        state.put_account(&contract_id, &acct).unwrap();
    }

    fn register_contract_symbol_for_test(
        state: &StateStore,
        owner: Pubkey,
        contract_id: Pubkey,
        symbol: &str,
    ) {
        state
            .register_symbol(
                symbol,
                SymbolRegistryEntry {
                    symbol: symbol.to_string(),
                    program: contract_id,
                    owner,
                    name: Some(symbol.to_string()),
                    template: Some("contract".to_string()),
                    metadata: None,
                    decimals: None,
                },
            )
            .unwrap();
    }

    fn configure_incident_guardian_for_test(
        state: &StateStore,
        governance_authority: Pubkey,
        threshold: u8,
        signers: Vec<Pubkey>,
    ) -> Pubkey {
        let guardian_authority =
            crate::multisig::derive_incident_guardian_authority(&governance_authority);
        state
            .set_incident_guardian_authority(&guardian_authority)
            .unwrap();
        state
            .set_governed_wallet_config(
                &guardian_authority,
                &crate::multisig::GovernedWalletConfig::new(
                    threshold,
                    signers,
                    crate::multisig::INCIDENT_GUARDIAN_LABEL,
                ),
            )
            .unwrap();
        guardian_authority
    }

    fn configure_treasury_executor_for_test(
        state: &StateStore,
        governance_authority: Pubkey,
        threshold: u8,
        signers: Vec<Pubkey>,
    ) -> Pubkey {
        let authority = crate::multisig::derive_treasury_executor_authority(&governance_authority);
        state.set_treasury_executor_authority(&authority).unwrap();
        state
            .set_governed_wallet_config(
                &authority,
                &crate::multisig::GovernedWalletConfig::new(
                    threshold,
                    signers,
                    crate::multisig::TREASURY_EXECUTOR_LABEL,
                )
                .with_timelock(1),
            )
            .unwrap();
        authority
    }

    fn configure_bridge_committee_admin_for_test(
        state: &StateStore,
        governance_authority: Pubkey,
        threshold: u8,
        signers: Vec<Pubkey>,
    ) -> Pubkey {
        let authority =
            crate::multisig::derive_bridge_committee_admin_authority(&governance_authority);
        state
            .set_bridge_committee_admin_authority(&authority)
            .unwrap();
        state
            .set_governed_wallet_config(
                &authority,
                &crate::multisig::GovernedWalletConfig::new(
                    threshold,
                    signers,
                    crate::multisig::BRIDGE_COMMITTEE_ADMIN_LABEL,
                )
                .with_timelock(1),
            )
            .unwrap();
        authority
    }

    fn configure_oracle_committee_admin_for_test(
        state: &StateStore,
        governance_authority: Pubkey,
        threshold: u8,
        signers: Vec<Pubkey>,
    ) -> Pubkey {
        let authority =
            crate::multisig::derive_oracle_committee_admin_authority(&governance_authority);
        state
            .set_oracle_committee_admin_authority(&authority)
            .unwrap();
        state
            .set_governed_wallet_config(
                &authority,
                &crate::multisig::GovernedWalletConfig::new(
                    threshold,
                    signers,
                    crate::multisig::ORACLE_COMMITTEE_ADMIN_LABEL,
                )
                .with_timelock(1),
            )
            .unwrap();
        authority
    }

    fn configure_upgrade_proposer_for_test(
        state: &StateStore,
        governance_authority: Pubkey,
        threshold: u8,
        signers: Vec<Pubkey>,
    ) -> Pubkey {
        let authority = crate::multisig::derive_upgrade_proposer_authority(&governance_authority);
        state.set_upgrade_proposer_authority(&authority).unwrap();
        state
            .set_governed_wallet_config(
                &authority,
                &crate::multisig::GovernedWalletConfig::new(
                    threshold,
                    signers,
                    crate::multisig::UPGRADE_PROPOSER_LABEL,
                )
                .with_timelock(1),
            )
            .unwrap();
        authority
    }

    fn configure_upgrade_veto_guardian_for_test(
        state: &StateStore,
        governance_authority: Pubkey,
        threshold: u8,
        signers: Vec<Pubkey>,
    ) -> Pubkey {
        let authority =
            crate::multisig::derive_upgrade_veto_guardian_authority(&governance_authority);
        state
            .set_upgrade_veto_guardian_authority(&authority)
            .unwrap();
        state
            .set_governed_wallet_config(
                &authority,
                &crate::multisig::GovernedWalletConfig::new(
                    threshold,
                    signers,
                    crate::multisig::UPGRADE_VETO_GUARDIAN_LABEL,
                ),
            )
            .unwrap();
        authority
    }

    #[test]
    fn test_register_symbol_success() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let contract_id = Pubkey([90u8; 32]);

        deploy_fake_contract(&state, alice, contract_id);

        let json_payload = r#"{"symbol":"TLICN","name":"TestLicn","template":"token"}"#;
        let mut data = vec![20u8];
        data.extend_from_slice(json_payload.as_bytes());

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, contract_id],
            data,
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(
            result.success,
            "Symbol registration should succeed: {:?}",
            result.error
        );

        // Verify symbol is registered
        let entry = state.get_symbol_registry("TLICN").unwrap();
        assert!(entry.is_some(), "Symbol TLICN should be in registry");
        let e = entry.unwrap();
        assert_eq!(e.program, contract_id);
        assert_eq!(e.owner, alice);
    }

    #[test]
    fn test_register_symbol_wrong_owner_rejected() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let eve_kp = Keypair::generate();
        let eve = eve_kp.pubkey();
        state.put_account(&eve, &Account::new(100, eve)).unwrap();

        let contract_id = Pubkey([91u8; 32]);
        // Eve owns the contract, but alice tries to register
        deploy_fake_contract(&state, eve, contract_id);

        let json_payload = r#"{"symbol":"EVIL","name":"Evil Token"}"#;
        let mut data = vec![20u8];
        data.extend_from_slice(json_payload.as_bytes());

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, contract_id],
            data,
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!result.success, "Wrong owner should fail");
        assert!(result.error.unwrap().contains("Only the contract owner"));
    }

    #[test]
    fn test_register_symbol_duplicate_different_program_rejected() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let contract1 = Pubkey([92u8; 32]);
        let contract2 = Pubkey([93u8; 32]);

        deploy_fake_contract(&state, alice, contract1);
        deploy_fake_contract(&state, alice, contract2);

        // Register symbol for contract1
        let json = r#"{"symbol":"DUP","name":"Dup Token"}"#;
        let mut data = vec![20u8];
        data.extend_from_slice(json.as_bytes());

        let ix1 = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, contract1],
            data: data.clone(),
        };
        let tx1 = make_signed_tx(&alice_kp, ix1, genesis_hash);
        let r1 = processor.process_transaction(&tx1, &Pubkey([42u8; 32]));
        assert!(
            r1.success,
            "First registration should succeed: {:?}",
            r1.error
        );

        // Try to register same symbol for contract2
        let ix2 = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, contract2],
            data,
        };
        let tx2 = make_signed_tx(&alice_kp, ix2, genesis_hash);
        let r2 = processor.process_transaction(&tx2, &Pubkey([42u8; 32]));
        assert!(
            !r2.success,
            "Duplicate symbol on different contract should fail"
        );
        assert!(r2.error.unwrap().contains("already registered"));
    }

    #[test]
    fn test_register_symbol_rejects_overlong_fields() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let contract_id = Pubkey([94u8; 32]);

        deploy_fake_contract(&state, alice, contract_id);

        let payload = serde_json::json!({
            "symbol": "S".repeat(MAX_SYMBOL_REGISTRY_SYMBOL_LEN + 1),
            "name": "N".repeat(MAX_SYMBOL_REGISTRY_NAME_LEN + 1),
            "template": "T".repeat(MAX_SYMBOL_REGISTRY_TEMPLATE_LEN + 1),
            "metadata": {
                "k".repeat(MAX_SYMBOL_REGISTRY_METADATA_KEY_LEN + 1): "value"
            }
        });
        let mut data = vec![20u8];
        data.extend_from_slice(payload.to_string().as_bytes());

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, contract_id],
            data,
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!result.success, "overlong symbol registration must fail");
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("exceeds"),
            "unexpected: {:?}",
            result.error
        );
    }

    // ====================================================================
    // UTILITY FUNCTIONS
    // ====================================================================

    // AUDIT-FIX INFO-01: test_reputation_fee_discount_removed removed along with the function.

    #[test]
    fn test_get_trust_tier() {
        assert_eq!(get_trust_tier(0), 0);
        assert_eq!(get_trust_tier(99), 0);
        assert_eq!(get_trust_tier(100), 1);
        assert_eq!(get_trust_tier(499), 1);
        assert_eq!(get_trust_tier(500), 2);
        assert_eq!(get_trust_tier(999), 2);
        assert_eq!(get_trust_tier(1000), 3);
        assert_eq!(get_trust_tier(4999), 3);
        assert_eq!(get_trust_tier(5000), 4);
        assert_eq!(get_trust_tier(9999), 4);
        assert_eq!(get_trust_tier(10000), 5);
        assert_eq!(get_trust_tier(99999), 5);
    }

    // ====================================================================
    // SIMULATE TRANSACTION
    // ====================================================================

    #[test]
    fn test_simulate_valid_transfer() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);

        let tx = make_transfer_tx(&alice_kp, alice, bob, 10, genesis_hash);
        let sim = processor.simulate_transaction(&tx);

        assert!(
            sim.success,
            "Simulation should succeed for valid tx: {:?}",
            sim.error
        );
        assert!(sim.fee > 0, "Fee should be non-zero");
        assert!(!sim.logs.is_empty(), "Logs should be populated");
    }

    #[test]
    fn test_simulate_zero_blockhash_rejected() {
        let (processor, _state, alice_kp, alice, _treasury, _genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);

        let tx = make_transfer_tx(&alice_kp, alice, bob, 10, Hash::default());
        let sim = processor.simulate_transaction(&tx);

        assert!(
            !sim.success,
            "Zero blockhash should be rejected in simulation"
        );
        assert!(sim.error.unwrap().contains("Zero blockhash"));
    }

    #[test]
    fn test_simulate_bad_blockhash_rejected() {
        let (processor, _state, alice_kp, alice, _treasury, _genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);

        let tx = make_transfer_tx(&alice_kp, alice, bob, 10, Hash::hash(b"not_a_real_block"));
        let sim = processor.simulate_transaction(&tx);

        assert!(
            !sim.success,
            "Invalid blockhash should be rejected in simulation"
        );
        assert!(sim.error.unwrap().contains("Blockhash not found"));
    }

    #[test]
    fn test_simulate_unsigned_rejected() {
        let (processor, _state, _alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);

        let mut data = vec![0u8];
        data.extend_from_slice(&Account::licn_to_spores(10).to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, bob],
            data,
        };
        let message = crate::transaction::Message::new(vec![ix], genesis_hash);
        let tx = Transaction::new(message); // No signatures

        let sim = processor.simulate_transaction(&tx);
        assert!(!sim.success, "Unsigned tx should fail simulation");
        assert!(sim.error.unwrap().contains("Missing"));
    }

    #[test]
    fn test_simulate_insufficient_balance() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);

        // Drain alice's balance
        let mut acct = state.get_account(&alice).unwrap().unwrap();
        acct.spores = 0;
        acct.spendable = 0;
        state.put_account(&alice, &acct).unwrap();

        let tx = make_transfer_tx(&alice_kp, alice, bob, 10, genesis_hash);
        let sim = processor.simulate_transaction(&tx);

        assert!(!sim.success, "Should fail with insufficient balance");
        assert!(sim.error.unwrap().contains("Insufficient balance"));
    }

    #[test]
    fn test_simulate_contract_call_uses_top_level_runtime_context() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let lichenid_program = Pubkey([44u8; 32]);
        let rep_key = crate::contract::lichenid_reputation_storage_key(&alice);
        let rep_data = 42u64.to_le_bytes().to_vec();
        let contract_addr =
            install_test_contract_account(&state, alice, reputation_reader_contract_code(&rep_key));

        state
            .put_account(&alice, &Account::new(Account::licn_to_spores(10), alice))
            .unwrap();
        state
            .put_contract_storage(&contract_addr, b"pm_lichenid_addr", &lichenid_program.0)
            .unwrap();
        state
            .put_contract_storage(&lichenid_program, &rep_key, &rep_data)
            .unwrap();

        let ix = Instruction {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: vec![alice, contract_addr],
            data: crate::ContractInstruction::Call {
                function: "read_reputation".to_string(),
                args: Vec::new(),
                value: 0,
            }
            .serialize()
            .unwrap(),
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let sim = processor.simulate_transaction(&tx);

        assert!(sim.success, "simulation should succeed: {:?}", sim.error);
        assert_eq!(sim.return_data, Some(rep_data));
    }

    // ====================================================================
    // UNKNOWN INSTRUCTION TYPE
    // ====================================================================

    #[test]
    fn test_unknown_system_instruction_rejected() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: vec![255u8], // Unknown type
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!result.success, "Unknown instruction type should fail");
        assert!(result.error.unwrap().contains("Unknown system instruction"));
    }

    #[test]
    fn test_empty_instruction_data_rejected() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: vec![],
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!result.success, "Empty instruction data should fail");
        assert!(result.error.unwrap().contains("Empty instruction data"));
    }

    #[test]
    fn test_fee_split_sums_to_100() {
        let cfg = FeeConfig::default_from_constants();
        let total = cfg.fee_burn_percent
            + cfg.fee_producer_percent
            + cfg.fee_voters_percent
            + cfg.fee_treasury_percent
            + cfg.fee_community_percent;
        assert_eq!(
            total, 100,
            "fee split percentages must sum to 100, got {total}"
        );
        // Verify individual values match design spec (40/30/10/10/10)
        assert_eq!(cfg.fee_burn_percent, 40);
        assert_eq!(cfg.fee_producer_percent, 30);
        assert_eq!(cfg.fee_voters_percent, 10);
        assert_eq!(cfg.fee_treasury_percent, 10);
        assert_eq!(cfg.fee_community_percent, 10);
    }

    // ====================================================================
    // GOVERNED WALLET MULTI-SIG TESTS
    // ====================================================================

    #[test]
    fn test_ecosystem_grant_requires_multisig() {
        // Standard transfer from a governed wallet must be rejected.
        let (processor, state, _alice_kp, alice, _treasury, genesis_hash) = setup();
        let eco_kp = Keypair::generate();
        let eco = eco_kp.pubkey();
        let recipient = Pubkey([99u8; 32]);

        // Fund the ecosystem wallet
        let eco_acct = Account::new(Account::licn_to_spores(1000), Pubkey([0u8; 32]));
        state.put_account(&eco, &eco_acct).unwrap();

        // Configure as governed wallet (threshold=2, signers=[alice, eco])
        let config = crate::multisig::GovernedWalletConfig::new(
            2,
            vec![alice, eco],
            "ecosystem_partnerships",
        );
        state.set_governed_wallet_config(&eco, &config).unwrap();

        // Standard transfer (type 0) from governed wallet → REJECTED
        let tx = make_transfer_tx(&eco_kp, eco, recipient, 100, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(
            !result.success,
            "Standard transfer from governed wallet should be rejected"
        );
        assert!(
            result
                .error
                .as_ref()
                .unwrap()
                .contains("multi-sig proposal"),
            "Error should mention multi-sig requirement, got: {}",
            result.error.unwrap()
        );

        // Recipient should NOT have received anything
        assert_eq!(state.get_balance(&recipient).unwrap(), 0);
    }

    fn put_active_processor_test_restriction(
        state: &StateStore,
        target: RestrictionTarget,
        mode: RestrictionMode,
    ) -> u64 {
        let id = state.next_restriction_id().unwrap();
        let record = RestrictionRecord {
            id,
            target,
            mode,
            status: RestrictionStatus::Active,
            reason: RestrictionReason::TestnetDrill,
            evidence_hash: None,
            evidence_uri_hash: None,
            proposer: Pubkey([0xA1; 32]),
            authority: Pubkey([0xA2; 32]),
            approval_authority: None,
            created_slot: 0,
            created_epoch: 0,
            expires_at_slot: None,
            supersedes: None,
            lifted_by: None,
            lifted_slot: None,
            lift_reason: None,
        };
        state.put_restriction(&record).unwrap();
        id
    }

    fn lift_processor_test_restriction(state: &StateStore, restriction_id: u64, lifted_by: Pubkey) {
        let mut record = state
            .get_restriction(restriction_id)
            .unwrap()
            .expect("restriction should exist");
        record.status = RestrictionStatus::Lifted;
        record.lifted_by = Some(lifted_by);
        record.lifted_slot = Some(state.get_last_slot().unwrap());
        record.lift_reason = Some(RestrictionLiftReason::TestnetDrillComplete);
        state.put_restriction(&record).unwrap();
    }

    fn governed_transfer_propose_tx(
        proposer_kp: &Keypair,
        proposer: Pubkey,
        source: Pubkey,
        recipient: Pubkey,
        amount: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![21u8];
        data.extend_from_slice(&amount.to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![proposer, source, recipient],
            data,
        };
        make_signed_tx(proposer_kp, ix, recent_blockhash)
    }

    fn governed_transfer_control_tx(
        signer_kp: &Keypair,
        signer: Pubkey,
        opcode: u8,
        proposal_id: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![opcode];
        data.extend_from_slice(&proposal_id.to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![signer],
            data,
        };
        make_signed_tx(signer_kp, ix, recent_blockhash)
    }

    #[test]
    fn test_governed_wallet_direct_transfer_still_requires_proposal_when_restricted() {
        let (processor, state, _alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let gov_kp = Keypair::generate();
        let gov = gov_kp.pubkey();
        let recipient = Pubkey([0x91; 32]);

        state.put_account(&gov, &Account::new(1_000, gov)).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, gov],
                    "ecosystem_partnerships",
                ),
            )
            .unwrap();
        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(gov),
            RestrictionMode::OutgoingOnly,
        );

        let tx = make_transfer_tx(&gov_kp, gov, recipient, 10, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("multi-sig proposal"));
        assert!(!result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("restriction"));
        assert_eq!(state.get_balance(&recipient).unwrap(), 0);
    }

    #[test]
    fn test_governed_transfer_source_restriction_blocks_execution_without_losing_proposal() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov = Pubkey([0x92; 32]);
        let recipient = Pubkey([0x93; 32]);
        let amount = Account::licn_to_spores(50);

        state.put_account(&bob, &Account::new(1_000, bob)).unwrap();
        state.put_account(&gov, &Account::new(1_000, gov)).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "ecosystem_partnerships",
                )
                .with_timelock(1)
                .with_transfer_velocity_policy(
                    crate::multisig::GovernedTransferVelocityPolicy::new(
                        amount * 10,
                        amount * 2,
                        0,
                        0,
                        0,
                        0,
                    ),
                ),
            )
            .unwrap();
        let restriction_id = put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(gov),
            RestrictionMode::OutgoingOnly,
        );

        let propose_tx =
            governed_transfer_propose_tx(&alice_kp, alice, gov, recipient, amount, genesis_hash);
        let result = processor.process_transaction(&propose_tx, &validator);
        assert!(result.success, "proposal failed: {:?}", result.error);

        let approve_tx = governed_transfer_control_tx(&bob_kp, bob, 22, 1, genesis_hash);
        let result = processor.process_transaction(&approve_tx, &validator);
        assert!(result.success, "approval failed: {:?}", result.error);
        let proposal = state.get_governed_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.approvals.len(), 2);
        assert!(!proposal.executed);

        let execute_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);
        let execute_tx = governed_transfer_control_tx(&alice_kp, alice, 32, 1, execute_blockhash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("sender account restriction"));

        let proposal = state.get_governed_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.approvals.len(), 2);
        assert!(!proposal.executed);
        assert_eq!(state.get_balance(&recipient).unwrap(), 0);
        let day_bucket = SLOTS_PER_EPOCH / SECONDS_PER_DAY;
        assert_eq!(
            state
                .get_governed_transfer_day_volume(&gov, day_bucket)
                .unwrap(),
            0
        );

        lift_processor_test_restriction(&state, restriction_id, alice);
        let retry_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH + 1);
        let retry_tx = governed_transfer_control_tx(&bob_kp, bob, 32, 1, retry_blockhash);
        let result = processor.process_transaction(&retry_tx, &validator);
        assert!(result.success, "retry failed: {:?}", result.error);
        let proposal = state.get_governed_proposal(1).unwrap().unwrap();
        assert!(proposal.executed);
        assert_eq!(state.get_balance(&recipient).unwrap(), amount);
        assert_eq!(
            state
                .get_governed_transfer_day_volume(&gov, day_bucket)
                .unwrap(),
            amount
        );
    }

    #[test]
    fn test_governed_transfer_recipient_restriction_blocks_execution_without_losing_proposal() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov = Pubkey([0x94; 32]);
        let recipient = Pubkey([0x95; 32]);
        let amount = Account::licn_to_spores(50);

        state.put_account(&bob, &Account::new(1_000, bob)).unwrap();
        state.put_account(&gov, &Account::new(1_000, gov)).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "ecosystem_partnerships",
                )
                .with_timelock(1)
                .with_transfer_velocity_policy(
                    crate::multisig::GovernedTransferVelocityPolicy::new(
                        amount * 10,
                        amount * 2,
                        0,
                        0,
                        0,
                        0,
                    ),
                ),
            )
            .unwrap();
        let restriction_id = put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(recipient),
            RestrictionMode::IncomingOnly,
        );

        let propose_tx =
            governed_transfer_propose_tx(&alice_kp, alice, gov, recipient, amount, genesis_hash);
        let result = processor.process_transaction(&propose_tx, &validator);
        assert!(result.success, "proposal failed: {:?}", result.error);

        let approve_tx = governed_transfer_control_tx(&bob_kp, bob, 22, 1, genesis_hash);
        let result = processor.process_transaction(&approve_tx, &validator);
        assert!(result.success, "approval failed: {:?}", result.error);

        let execute_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);
        let execute_tx = governed_transfer_control_tx(&alice_kp, alice, 32, 1, execute_blockhash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("recipient account restriction"));

        let proposal = state.get_governed_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.approvals.len(), 2);
        assert!(!proposal.executed);
        assert_eq!(state.get_balance(&recipient).unwrap(), 0);
        let day_bucket = SLOTS_PER_EPOCH / SECONDS_PER_DAY;
        assert_eq!(
            state
                .get_governed_transfer_day_volume(&gov, day_bucket)
                .unwrap(),
            0
        );

        lift_processor_test_restriction(&state, restriction_id, alice);
        let retry_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH + 1);
        let retry_tx = governed_transfer_control_tx(&bob_kp, bob, 32, 1, retry_blockhash);
        let result = processor.process_transaction(&retry_tx, &validator);
        assert!(result.success, "retry failed: {:?}", result.error);
        let proposal = state.get_governed_proposal(1).unwrap().unwrap();
        assert!(proposal.executed);
        assert_eq!(state.get_balance(&recipient).unwrap(), amount);
        assert_eq!(
            state
                .get_governed_transfer_day_volume(&gov, day_bucket)
                .unwrap(),
            amount
        );
    }

    #[test]
    fn test_governed_proposal_lifecycle() {
        // Propose → approve → auto-execute lifecycle for governed wallet.
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let eco_kp = Keypair::generate();
        let eco = eco_kp.pubkey();
        let recipient = Pubkey([99u8; 32]);

        // Fund participants
        let fund = Account::licn_to_spores(1000);
        state
            .put_account(&eco, &Account::new(fund, Pubkey([0u8; 32])))
            .unwrap();
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();

        // Configure governed wallet (threshold=2, signers=[alice, bob, eco])
        let config = crate::multisig::GovernedWalletConfig::new(
            2,
            vec![alice, bob, eco],
            "ecosystem_partnerships",
        );
        state.set_governed_wallet_config(&eco, &config).unwrap();

        let transfer_amount = Account::licn_to_spores(50);

        // Step 1: Alice proposes a governed transfer (type 21)
        let mut propose_data = vec![21u8];
        propose_data.extend_from_slice(&transfer_amount.to_le_bytes());
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, eco, recipient],
            data: propose_data,
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let result = processor.process_transaction(&propose_tx, &Pubkey([42u8; 32]));
        assert!(
            result.success,
            "Proposal should succeed: {:?}",
            result.error
        );

        // Verify proposal exists but is NOT executed yet
        let proposal = state.get_governed_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.approvals.len(), 1);
        assert_eq!(proposal.approvals[0], alice);
        assert!(
            !proposal.executed,
            "Proposal should not be executed with only 1 approval"
        );
        assert_eq!(
            state.get_balance(&recipient).unwrap(),
            0,
            "Recipient should not have funds yet"
        );

        // Step 2: Bob approves (type 22) → reaches threshold → auto-executes
        let mut approve_data = vec![22u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes()); // proposal_id = 1
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let result = processor.process_transaction(&approve_tx, &Pubkey([42u8; 32]));
        assert!(
            result.success,
            "Approval should succeed: {:?}",
            result.error
        );

        // Verify proposal is now executed
        let proposal = state.get_governed_proposal(1).unwrap().unwrap();
        assert!(
            proposal.executed,
            "Proposal should be executed after meeting threshold"
        );
        assert_eq!(proposal.approvals.len(), 2);

        // Verify transfer happened
        assert_eq!(
            state.get_balance(&recipient).unwrap(),
            transfer_amount,
            "Recipient should have received the transfer"
        );
    }

    #[test]
    fn test_governed_proposal_timelock_requires_execute() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let eco_kp = Keypair::generate();
        let eco = eco_kp.pubkey();
        let recipient = Pubkey([98u8; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&eco, &Account::new(fund, Pubkey([0u8; 32])))
            .unwrap();
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.set_last_slot(0).unwrap();

        let config = crate::multisig::GovernedWalletConfig::new(
            2,
            vec![alice, bob, eco],
            "community_treasury",
        )
        .with_timelock(1);
        state.set_governed_wallet_config(&eco, &config).unwrap();

        let transfer_amount = Account::licn_to_spores(25);

        let mut propose_data = vec![21u8];
        propose_data.extend_from_slice(&transfer_amount.to_le_bytes());
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, eco, recipient],
            data: propose_data,
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let result = processor.process_transaction(&propose_tx, &Pubkey([42u8; 32]));
        assert!(
            result.success,
            "Proposal should succeed: {:?}",
            result.error
        );

        let mut approve_data = vec![22u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let result = processor.process_transaction(&approve_tx, &Pubkey([42u8; 32]));
        assert!(
            result.success,
            "Approval should succeed: {:?}",
            result.error
        );

        let proposal = state.get_governed_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.execute_after_epoch, 1);
        assert!(!proposal.executed, "Proposal should remain timelocked");
        assert_eq!(state.get_balance(&recipient).unwrap(), 0);

        let mut execute_data = vec![32u8];
        execute_data.extend_from_slice(&1u64.to_le_bytes());
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: execute_data.clone(),
        };
        let execute_tx = make_signed_tx(&bob_kp, execute_ix, genesis_hash);
        let result = processor.process_transaction(&execute_tx, &Pubkey([42u8; 32]));
        assert!(
            !result.success,
            "Execution should fail before timelock expires"
        );
        assert!(result.error.as_deref().unwrap_or("").contains("timelocked"));

        let fresh_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);

        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data,
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, fresh_blockhash);
        let result = processor.process_transaction(&execute_tx, &Pubkey([42u8; 32]));
        assert!(
            result.success,
            "Execution should succeed: {:?}",
            result.error
        );

        let proposal = state.get_governed_proposal(1).unwrap().unwrap();
        assert!(
            proposal.executed,
            "Proposal should be executed after timelock"
        );
        assert_eq!(state.get_balance(&recipient).unwrap(), transfer_amount);
    }

    #[test]
    fn test_governed_transfer_velocity_policy_rejects_amount_over_cap() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let governed_wallet = Pubkey([0x71; 32]);
        let recipient = Pubkey([0x72; 32]);

        state
            .put_account(&governed_wallet, &Account::new(1_000, governed_wallet))
            .unwrap();
        state
            .set_governed_wallet_config(
                &governed_wallet,
                &crate::multisig::GovernedWalletConfig::new(
                    1,
                    vec![alice],
                    "ecosystem_partnerships",
                )
                .with_transfer_velocity_policy(
                    crate::multisig::GovernedTransferVelocityPolicy::new(50, 100, 0, 0, 0, 0),
                ),
            )
            .unwrap();

        let mut propose_data = vec![21u8];
        propose_data.extend_from_slice(&60u64.to_le_bytes());
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, governed_wallet, recipient],
            data: propose_data,
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let result = processor.process_transaction(&propose_tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("per-transfer cap"));
    }

    #[test]
    fn test_governed_transfer_velocity_policy_snapshots_escalation() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let governed_wallet = Pubkey([0x73; 32]);
        let recipient = Pubkey([0x74; 32]);

        state.put_account(&bob, &Account::new(1_000, bob)).unwrap();
        state
            .put_account(&governed_wallet, &Account::new(1_000, governed_wallet))
            .unwrap();
        state
            .set_governed_wallet_config(
                &governed_wallet,
                &crate::multisig::GovernedWalletConfig::new(
                    1,
                    vec![alice, bob],
                    "community_treasury",
                )
                .with_transfer_velocity_policy(
                    crate::multisig::GovernedTransferVelocityPolicy::new(200, 200, 50, 90, 1, 3),
                ),
            )
            .unwrap();

        let mut propose_data = vec![21u8];
        propose_data.extend_from_slice(&60u64.to_le_bytes());
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, governed_wallet, recipient],
            data: propose_data,
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let result = processor.process_transaction(&propose_tx, &validator);
        assert!(result.success, "proposal failed: {:?}", result.error);

        let proposal = state.get_governed_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.threshold, 2);
        assert_eq!(proposal.execute_after_epoch, 1);
        assert_eq!(
            proposal.velocity_tier,
            crate::multisig::GovernedTransferVelocityTier::Elevated
        );
        assert_eq!(proposal.daily_cap_spores, 200);
        assert!(!proposal.executed);

        let mut approve_data = vec![22u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let result = processor.process_transaction(&approve_tx, &validator);
        assert!(result.success, "approval failed: {:?}", result.error);
        assert!(!state.get_governed_proposal(1).unwrap().unwrap().executed);

        let fresh_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);
        let mut execute_data = vec![32u8];
        execute_data.extend_from_slice(&1u64.to_le_bytes());
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data,
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, fresh_blockhash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(result.success, "execution failed: {:?}", result.error);
        assert_eq!(state.get_balance(&recipient).unwrap(), 60);
    }

    #[test]
    fn test_governed_transfer_daily_cap_defers_until_next_day() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let governed_wallet = Pubkey([0x75; 32]);
        let first_recipient = Pubkey([0x76; 32]);
        let second_recipient = Pubkey([0x77; 32]);

        state
            .put_account(&governed_wallet, &Account::new(1_000, governed_wallet))
            .unwrap();
        state
            .set_governed_wallet_config(
                &governed_wallet,
                &crate::multisig::GovernedWalletConfig::new(1, vec![alice], "community_treasury")
                    .with_transfer_velocity_policy(
                        crate::multisig::GovernedTransferVelocityPolicy::new(200, 100, 0, 0, 0, 0),
                    ),
            )
            .unwrap();

        let mut first_propose_data = vec![21u8];
        first_propose_data.extend_from_slice(&60u64.to_le_bytes());
        let first_propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, governed_wallet, first_recipient],
            data: first_propose_data,
        };
        let first_propose_tx = make_signed_tx(&alice_kp, first_propose_ix, genesis_hash);
        let result = processor.process_transaction(&first_propose_tx, &validator);
        assert!(result.success, "first transfer failed: {:?}", result.error);
        assert!(state.get_governed_proposal(1).unwrap().unwrap().executed);

        let mut second_propose_data = vec![21u8];
        second_propose_data.extend_from_slice(&50u64.to_le_bytes());
        let second_propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, governed_wallet, second_recipient],
            data: second_propose_data,
        };
        let second_propose_tx = make_signed_tx(&alice_kp, second_propose_ix, genesis_hash);
        let result = processor.process_transaction(&second_propose_tx, &validator);
        assert!(result.success, "second proposal failed: {:?}", result.error);

        let second_proposal = state.get_governed_proposal(2).unwrap().unwrap();
        assert!(!second_proposal.executed);
        assert_eq!(state.get_balance(&second_recipient).unwrap(), 0);
        assert_eq!(
            state
                .get_governed_transfer_day_volume(&governed_wallet, 0)
                .unwrap(),
            60
        );

        let mut execute_data = vec![32u8];
        execute_data.extend_from_slice(&2u64.to_le_bytes());
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data.clone(),
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, genesis_hash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("daily cap"));

        let fresh_blockhash = advance_test_slot(&state, SECONDS_PER_DAY);
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data,
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, fresh_blockhash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(
            result.success,
            "deferred execute failed: {:?}",
            result.error
        );
        assert!(state.get_governed_proposal(2).unwrap().unwrap().executed);
        assert_eq!(state.get_balance(&second_recipient).unwrap(), 50);
        assert_eq!(
            state
                .get_governed_transfer_day_volume(&governed_wallet, 1)
                .unwrap(),
            50
        );
    }

    #[test]
    fn test_reserve_pool_requires_supermajority() {
        // Reserve pool with threshold=3 requires more approvals than ecosystem (threshold=2).
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let reserve_kp = Keypair::generate();
        let reserve = reserve_kp.pubkey();
        let recipient = Pubkey([88u8; 32]);

        // Fund participants
        let fund = Account::licn_to_spores(1000);
        state
            .put_account(&reserve, &Account::new(fund, Pubkey([0u8; 32])))
            .unwrap();
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();

        // Configure reserve_pool as governed wallet (threshold=3 — supermajority)
        let config = crate::multisig::GovernedWalletConfig::new(
            3,
            vec![alice, bob, reserve],
            "reserve_pool",
        );
        state.set_governed_wallet_config(&reserve, &config).unwrap();

        let transfer_amount = Account::licn_to_spores(10);

        // Propose
        let mut data = vec![21u8];
        data.extend_from_slice(&transfer_amount.to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, reserve, recipient],
            data,
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(result.success);

        // First approval (Bob) — still not enough (2 of 3)
        let mut data = vec![22u8];
        data.extend_from_slice(&1u64.to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data,
        };
        let tx = make_signed_tx(&bob_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(result.success);

        // Verify NOT executed yet (2 approvals, need 3)
        let proposal = state.get_governed_proposal(1).unwrap().unwrap();
        assert!(
            !proposal.executed,
            "Should NOT be executed with only 2/3 approvals"
        );
        assert_eq!(state.get_balance(&recipient).unwrap(), 0);

        // Third approval (reserve keypair) → threshold met → auto-execute
        let mut data = vec![22u8];
        data.extend_from_slice(&1u64.to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![reserve],
            data,
        };
        let tx = make_signed_tx(&reserve_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(
            result.success,
            "Third approval should succeed: {:?}",
            result.error
        );

        // Verify executed
        let proposal = state.get_governed_proposal(1).unwrap().unwrap();
        assert!(proposal.executed, "Should be executed with 3/3 approvals");
        assert_eq!(state.get_balance(&recipient).unwrap(), transfer_amount);
    }

    // ─── Shielded pool processor tests ──────────────────────────────

    #[cfg(feature = "zk")]
    #[test]
    fn test_shield_rejects_short_data() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();

        // Only 21 bytes provided (need at least 42)
        let mut data = vec![23u8];
        data.extend_from_slice(&[0u8; 20]);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data,
        };
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!result.success);
        assert!(
            result.error.as_ref().unwrap().contains("insufficient data"),
            "Expected insufficient data error, got: {:?}",
            result.error
        );
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shield_rejects_zero_amount() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();

        let mut data = vec![23u8];
        data.extend_from_slice(&0u64.to_le_bytes()); // zero amount
        data.extend_from_slice(&[0xAA; 32]); // commitment
        data.extend_from_slice(&[0xBB; 128]); // fake proof

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data,
        };
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!result.success);
        assert!(
            result.error.as_ref().unwrap().contains("non-zero"),
            "Expected non-zero error, got: {:?}",
            result.error
        );
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shield_rejects_no_accounts() {
        let (processor, _state, alice_kp, _alice, _treasury, genesis_hash) = setup();

        let mut data = vec![23u8];
        data.extend_from_slice(&100u64.to_le_bytes());
        data.extend_from_slice(&[0xAA; 32]);
        data.extend_from_slice(&[0xBB; 128]);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![], // no accounts!
            data,
        };
        // We still need at least one account for fee payer, so we put alice in a second ix
        // Actually the processor checks accounts on the instruction level — let's just test
        // that the error message is correct
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!result.success);
        // It might fail at fee payer extraction or at the shield handler
        assert!(result.error.is_some());
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shield_rejects_invalid_proof_bytes() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();

        let mut data = vec![23u8];
        data.extend_from_slice(&100u64.to_le_bytes());
        data.extend_from_slice(&[0xAA; 32]); // bogus commitment
        data.extend_from_slice(&[0xFF; 7]); // invalid proof bytes

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data,
        };
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!result.success, "Invalid proof bytes should fail");
        assert!(
            result.error.as_ref().unwrap().contains("proof"),
            "Expected proof-related error, got: {:?}",
            result.error
        );
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shield_accepts_native_proof_without_verifier_keys() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        use crate::zk::{
            circuits::shield::ShieldCircuit, commitment_hash, random_scalar_bytes, Prover,
        };

        let amount = 100u64;
        let blinding = random_scalar_bytes();
        let commitment = commitment_hash(amount, &blinding);
        let circuit = ShieldCircuit::new_bytes(amount, amount, blinding, commitment);
        let proof = Prover::new().prove_shield(circuit).expect("prove shield");

        let mut data = vec![23u8];
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&commitment);
        data.extend_from_slice(&proof.proof_bytes);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data,
        };
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(result.success, "native STARK verifier should not need VKs");
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shield_full_e2e_with_processor() {
        use crate::zk::{
            circuits::shield::ShieldCircuit, commitment_hash, random_scalar_bytes, Prover,
        };

        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup_();

        // 1. Build shield witness
        let amount = 500_000_000u64; // 0.5 LICN in spores
        let blinding = random_scalar_bytes();
        let commitment = commitment_hash(amount, &blinding);

        let circuit = ShieldCircuit::new_bytes(amount, amount, blinding, commitment);

        // 2. Generate proof
        let zk_proof = Prover::new().prove_shield(circuit).unwrap();

        // 3. Build instruction data
        let mut data = vec![23u8];
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&commitment);
        data.extend_from_slice(&zk_proof.proof_bytes);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data,
        };
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        // 4. Process transaction
        let alice_balance_before = state.get_balance(&alice).unwrap();
        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(result.success, "Shield should succeed: {:?}", result.error);

        // 5. Verify state changes
        let alice_balance_after = state.get_balance(&alice).unwrap();
        // Alice should have less balance (amount + fee deducted)
        assert!(
            alice_balance_after < alice_balance_before,
            "Alice balance should decrease after shield"
        );
        assert_eq!(
            alice_balance_before - alice_balance_after - result.fee_paid,
            amount,
            "Balance decrease minus fee should equal shielded amount"
        );

        // Pool state should be updated
        let pool = state.get_shielded_pool_state().unwrap();
        assert_eq!(pool.commitment_count, 1);
        assert_eq!(pool.total_shielded, amount);

        // Commitment should be stored
        let stored_commitment = state.get_shielded_commitment(0).unwrap();
        assert_eq!(stored_commitment, Some(commitment));

        // Merkle root should be updated to reflect the single leaf
        let mut expected_tree = crate::zk::MerkleTree::new();
        expected_tree.insert(commitment);
        assert_eq!(pool.merkle_root, expected_tree.root());
    }

    /// Renamed setup helper for shielded tests to avoid name collision
    #[cfg(feature = "zk")]
    fn setup_() -> (TxProcessor, StateStore, Keypair, Pubkey, Pubkey, Hash) {
        setup()
    }

    #[cfg(feature = "zk")]
    fn make_invalid_shield_tx(
        kp: &Keypair,
        sender: Pubkey,
        amount: u64,
        commitment: [u8; 32],
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![23u8];
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&commitment);
        data.extend_from_slice(&[0xFF; 7]);

        make_signed_tx(
            kp,
            Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![sender],
                data,
            },
            recent_blockhash,
        )
    }

    #[cfg(feature = "zk")]
    fn make_invalid_unshield_tx(
        kp: &Keypair,
        recipient: Pubkey,
        amount: u64,
        nullifier: [u8; 32],
        merkle_root: [u8; 32],
        recent_blockhash: Hash,
    ) -> Transaction {
        use crate::zk::{recipient_hash, recipient_preimage_from_bytes};

        let recipient_bytes = recipient_hash(&recipient_preimage_from_bytes(recipient.0));
        let mut data = vec![24u8];
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&nullifier);
        data.extend_from_slice(&merkle_root);
        data.extend_from_slice(&recipient_bytes);
        data.extend_from_slice(&[0xFF; 7]);

        make_signed_tx(
            kp,
            Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![recipient],
                data,
            },
            recent_blockhash,
        )
    }

    #[cfg(feature = "zk")]
    fn make_invalid_shielded_transfer_tx(
        kp: &Keypair,
        fee_payer: Pubkey,
        nullifier_a: [u8; 32],
        nullifier_b: [u8; 32],
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![25u8];
        data.extend_from_slice(&nullifier_a);
        data.extend_from_slice(&nullifier_b);
        data.extend_from_slice(&[0xC1; 32]);
        data.extend_from_slice(&[0xC2; 32]);
        data.extend_from_slice(&[0u8; 32]);
        data.extend_from_slice(&[0xFF; 7]);

        make_signed_tx(
            kp,
            Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![fee_payer],
                data,
            },
            recent_blockhash,
        )
    }

    #[cfg(feature = "zk")]
    fn assert_shielded_pool_unchanged(state: &StateStore, before: &crate::zk::ShieldedPoolState) {
        let after = state.get_shielded_pool_state().unwrap();
        assert_eq!(after.merkle_root, before.merkle_root);
        assert_eq!(after.commitment_count, before.commitment_count);
        assert_eq!(after.total_shielded, before.total_shielded);
        assert_eq!(after.nullifier_count, before.nullifier_count);
        assert_eq!(after.shield_count, before.shield_count);
        assert_eq!(after.unshield_count, before.unshield_count);
        assert_eq!(after.transfer_count, before.transfer_count);
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shield_rejects_outgoing_restricted_sender_without_pool_mutation() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup_();
        let validator = Pubkey([42u8; 32]);
        let before_pool = state.get_shielded_pool_state().unwrap();
        let before_balance = state.get_balance(&alice).unwrap();

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::OutgoingOnly,
        );

        let commitment = [0xA7; 32];
        let tx = make_invalid_shield_tx(&alice_kp, alice, 100, commitment, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Shield blocked by active sender account restriction"));

        assert_shielded_pool_unchanged(&state, &before_pool);
        assert_eq!(state.get_shielded_commitment(0).unwrap(), None);
        let after_balance = state.get_balance(&alice).unwrap();
        assert_eq!(before_balance - after_balance, result.fee_paid);
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shield_rejects_native_frozen_amount_without_pool_mutation() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup_();
        let validator = Pubkey([42u8; 32]);
        let before_pool = state.get_shielded_pool_state().unwrap();
        let before_balance = state.get_balance(&alice).unwrap();
        let spendable = state.get_account(&alice).unwrap().unwrap().spendable;

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::AccountAsset {
                account: alice,
                asset: NATIVE_LICN_ASSET_ID,
            },
            RestrictionMode::FrozenAmount { amount: spendable },
        );

        let commitment = [0xA8; 32];
        let tx = make_invalid_shield_tx(&alice_kp, alice, 100, commitment, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Shield blocked by active sender native account-asset restriction"));

        assert_shielded_pool_unchanged(&state, &before_pool);
        assert_eq!(state.get_shielded_commitment(0).unwrap(), None);
        let after_balance = state.get_balance(&alice).unwrap();
        assert_eq!(before_balance - after_balance, result.fee_paid);
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shield_protocol_pause_rejects_deposit_without_pool_mutation() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup_();
        let validator = Pubkey([42u8; 32]);
        let before_pool = state.get_shielded_pool_state().unwrap();
        let before_balance = state.get_balance(&alice).unwrap();

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::ProtocolModule(ProtocolModuleId::Shielded),
            RestrictionMode::ProtocolPaused,
        );

        let commitment = [0xA9; 32];
        let tx = make_invalid_shield_tx(&alice_kp, alice, 100, commitment, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Shield blocked by active Shielded protocol pause"));

        assert_shielded_pool_unchanged(&state, &before_pool);
        assert_eq!(state.get_shielded_commitment(0).unwrap(), None);
        let after_balance = state.get_balance(&alice).unwrap();
        assert_eq!(before_balance - after_balance, result.fee_paid);
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_unshield_rejects_incoming_restricted_recipient_without_spending_nullifier() {
        use crate::zk::random_scalar_bytes;

        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup_();
        let validator = Pubkey([42u8; 32]);
        let before_pool = state.get_shielded_pool_state().unwrap();
        let before_balance = state.get_balance(&alice).unwrap();
        let nullifier = random_scalar_bytes();

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::IncomingOnly,
        );

        let tx =
            make_invalid_unshield_tx(&alice_kp, alice, 100, nullifier, [0xEE; 32], genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Unshield blocked by active recipient account restriction"));

        assert!(!state.is_nullifier_spent(&nullifier).unwrap());
        assert_shielded_pool_unchanged(&state, &before_pool);
        let after_balance = state.get_balance(&alice).unwrap();
        assert_eq!(before_balance - after_balance, result.fee_paid);
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_unshield_rejects_native_incoming_restricted_recipient_without_spending_nullifier() {
        use crate::zk::random_scalar_bytes;

        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup_();
        let validator = Pubkey([42u8; 32]);
        let before_pool = state.get_shielded_pool_state().unwrap();
        let before_balance = state.get_balance(&alice).unwrap();
        let nullifier = random_scalar_bytes();

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::AccountAsset {
                account: alice,
                asset: NATIVE_LICN_ASSET_ID,
            },
            RestrictionMode::IncomingOnly,
        );

        let tx =
            make_invalid_unshield_tx(&alice_kp, alice, 100, nullifier, [0xEF; 32], genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Unshield blocked by active recipient native account-asset restriction"));

        assert!(!state.is_nullifier_spent(&nullifier).unwrap());
        assert_shielded_pool_unchanged(&state, &before_pool);
        let after_balance = state.get_balance(&alice).unwrap();
        assert_eq!(before_balance - after_balance, result.fee_paid);
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shielded_transfer_protocol_pause_rejects_before_nullifier_mutation() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup_();
        let validator = Pubkey([42u8; 32]);
        let before_pool = state.get_shielded_pool_state().unwrap();
        let nullifier_a = [0xF1; 32];
        let nullifier_b = [0xF2; 32];

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::ProtocolModule(ProtocolModuleId::Shielded),
            RestrictionMode::ProtocolPaused,
        );

        let tx = make_invalid_shielded_transfer_tx(
            &alice_kp,
            alice,
            nullifier_a,
            nullifier_b,
            genesis_hash,
        );
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("ShieldedTransfer blocked by active Shielded protocol pause"));

        assert!(!state.is_nullifier_spent(&nullifier_a).unwrap());
        assert!(!state.is_nullifier_spent(&nullifier_b).unwrap());
        assert_shielded_pool_unchanged(&state, &before_pool);
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_unshield_rejects_short_data() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();

        let mut data = vec![24u8];
        data.extend_from_slice(&[0u8; 50]); // too short (need at least 106 bytes total)

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data,
        };
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("insufficient data"));
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shield_batch_updates_merkle_root_with_prior_batch_commitments() {
        use crate::zk::{
            circuits::shield::ShieldCircuit, commitment_hash, random_scalar_bytes, Prover,
        };

        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup_();
        let validator = Pubkey([42u8; 32]);

        let amount_a = 100u64;
        let blinding_a = random_scalar_bytes();
        let commitment_a = commitment_hash(amount_a, &blinding_a);
        let proof_a = Prover::new()
            .prove_shield(ShieldCircuit::new_bytes(
                amount_a,
                amount_a,
                blinding_a,
                commitment_a,
            ))
            .unwrap();

        let amount_b = 200u64;
        let blinding_b = random_scalar_bytes();
        let commitment_b = commitment_hash(amount_b, &blinding_b);
        let proof_b = Prover::new()
            .prove_shield(ShieldCircuit::new_bytes(
                amount_b,
                amount_b,
                blinding_b,
                commitment_b,
            ))
            .unwrap();

        let mut data_a = vec![23u8];
        data_a.extend_from_slice(&amount_a.to_le_bytes());
        data_a.extend_from_slice(&commitment_a);
        data_a.extend_from_slice(&proof_a.proof_bytes);

        let mut data_b = vec![23u8];
        data_b.extend_from_slice(&amount_b.to_le_bytes());
        data_b.extend_from_slice(&commitment_b);
        data_b.extend_from_slice(&proof_b.proof_bytes);

        let msg = crate::transaction::Message::new(
            vec![
                Instruction {
                    program_id: SYSTEM_PROGRAM_ID,
                    accounts: vec![alice],
                    data: data_a,
                },
                Instruction {
                    program_id: SYSTEM_PROGRAM_ID,
                    accounts: vec![alice],
                    data: data_b,
                },
            ],
            genesis_hash,
        );
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &validator);
        assert!(
            result.success,
            "batched shield deposits should succeed: {:?}",
            result.error
        );

        let pool = state.get_shielded_pool_state().unwrap();
        assert_eq!(pool.commitment_count, 2);
        let mut tree = crate::zk::MerkleTree::new();
        tree.insert(commitment_a);
        tree.insert(commitment_b);
        assert_eq!(pool.merkle_root, tree.root());
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_unshield_rejects_recipient_mismatch() {
        use crate::zk::{recipient_hash, recipient_preimage_from_bytes};

        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();

        // Build valid-length unshield payload but with recipient input bound to a different account.
        let amount = 100u64;
        let nullifier = [0x11u8; 32];
        let merkle_root = [0u8; 32];

        // Deliberately mismatch by hashing a different pubkey than `alice`.
        let other_pubkey = Pubkey([0x22u8; 32]);
        let other_recipient = recipient_hash(&recipient_preimage_from_bytes(other_pubkey.0));

        let mut data = vec![24u8];
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&nullifier);
        data.extend_from_slice(&merkle_root);
        data.extend_from_slice(&other_recipient);
        data.extend_from_slice(&[0u8; 128]);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data,
        };
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!result.success);
        assert!(
            result
                .error
                .as_ref()
                .unwrap()
                .contains("recipient public input does not match recipient account"),
            "unexpected error: {:?}",
            result.error
        );
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_transfer_rejects_short_data() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();

        let mut data = vec![25u8];
        data.extend_from_slice(&[0u8; 100]); // too short (need at least 162 bytes total)

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data,
        };
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &Pubkey([42u8; 32]));
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("insufficient data"));
    }

    // ─── Graduated Rent Tests ────────────────────────────────────────────────

    #[test]
    fn test_graduated_rent_below_free_tier() {
        // Accounts with ≤ 2KB data pay zero rent
        assert_eq!(compute_graduated_rent(0, 100), 0);
        assert_eq!(compute_graduated_rent(1024, 100), 0);
        assert_eq!(compute_graduated_rent(2048, 100), 0);
    }

    #[test]
    fn test_graduated_rent_tier1() {
        // 3KB total → 1KB billable → 1KB × 1× rate
        assert_eq!(compute_graduated_rent(3 * 1024, 100), 100);
        // 10KB total → 8KB billable → 8KB × 1× rate
        assert_eq!(compute_graduated_rent(10 * 1024, 100), 800);
    }

    #[test]
    fn test_graduated_rent_tier2() {
        // 11KB total → 9KB billable → 8KB @1x + 1KB @2x
        assert_eq!(compute_graduated_rent(11 * 1024, 100), 800 + 200);
        // 50KB total → 48KB billable → 8KB @1x + 40KB @2x
        assert_eq!(compute_graduated_rent(50 * 1024, 100), 800 + 8000);
        // 100KB total → 98KB billable → 8KB @1x + 90KB @2x
        assert_eq!(compute_graduated_rent(100 * 1024, 100), 800 + 18000);
    }

    #[test]
    fn test_graduated_rent_tier3() {
        // 101KB total → 99KB billable → 8KB @1x + 90KB @2x + 1KB @4x
        assert_eq!(compute_graduated_rent(101 * 1024, 100), 800 + 18000 + 400);
        // 200KB total → 198KB billable → 8KB @1x + 90KB @2x + 100KB @4x
        assert_eq!(compute_graduated_rent(200 * 1024, 100), 800 + 18000 + 40000);
    }

    #[test]
    fn test_graduated_rent_partial_kb() {
        // 2049 bytes → 1 byte over free tier → rounds up to 1KB
        assert_eq!(compute_graduated_rent(2049, 100), 100);
        // 2048 + 512 = 2560 → 512 bytes over → rounds up to 1KB
        assert_eq!(compute_graduated_rent(2560, 100), 100);
    }

    #[test]
    fn test_graduated_rent_zero_rate() {
        assert_eq!(compute_graduated_rent(100 * 1024, 0), 0);
    }

    // ======== Durable Nonce Tests ========

    /// Helper: create a nonce-initialize instruction
    fn make_nonce_init_ix(funder: Pubkey, nonce_pk: Pubkey, authority: Pubkey) -> Instruction {
        let mut data = vec![28u8, 0u8]; // type=28, sub=0 (Initialize)
        data.extend_from_slice(&authority.0);
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![funder, nonce_pk],
            data,
        }
    }

    /// Helper: create a nonce-advance instruction
    fn make_nonce_advance_ix(authority: Pubkey, nonce_pk: Pubkey) -> Instruction {
        let data = vec![28u8, 1u8]; // type=28, sub=1 (Advance)
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![authority, nonce_pk],
            data,
        }
    }

    fn make_nonce_withdraw_ix(
        authority: Pubkey,
        nonce_pk: Pubkey,
        recipient: Pubkey,
        amount: u64,
    ) -> Instruction {
        let mut data = vec![28u8, 2u8];
        data.extend_from_slice(&amount.to_le_bytes());
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![authority, nonce_pk, recipient],
            data,
        }
    }

    fn initialize_test_nonce(
        processor: &TxProcessor,
        funder_kp: &Keypair,
        funder: Pubkey,
        nonce_pk: Pubkey,
        authority: Pubkey,
        recent_blockhash: Hash,
        validator: &Pubkey,
    ) {
        let ix = make_nonce_init_ix(funder, nonce_pk, authority);
        let tx = make_signed_tx(funder_kp, ix, recent_blockhash);
        let result = processor.process_transaction(&tx, validator);
        assert!(
            result.success,
            "Nonce initialization should succeed: {:?}",
            result.error
        );
    }

    fn assert_failed_nonce_withdraw_keeps_nonce_open(
        state: &StateStore,
        nonce_pk: Pubkey,
        recipient: Pubkey,
        expected_error: &Option<String>,
        expected_error_fragment: &str,
        before_nonce_account: &Account,
    ) {
        assert!(
            expected_error
                .as_ref()
                .unwrap()
                .contains(expected_error_fragment),
            "Expected error containing '{}', got: {:?}",
            expected_error_fragment,
            expected_error
        );
        let after_nonce_account = state.get_account(&nonce_pk).unwrap().unwrap();
        assert_eq!(after_nonce_account.spores, before_nonce_account.spores);
        assert_eq!(
            after_nonce_account.spendable,
            before_nonce_account.spendable
        );
        assert_eq!(after_nonce_account.data, before_nonce_account.data);
        assert_eq!(after_nonce_account.data[0], NONCE_ACCOUNT_MARKER);
        assert_eq!(state.get_balance(&recipient).unwrap_or(0), 0);
    }

    #[test]
    fn test_nonce_initialize() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);

        let ix = make_nonce_init_ix(alice, nonce_pk, alice);
        let message = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(message);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &validator);
        assert!(
            result.success,
            "NonceInit should succeed: {:?}",
            result.error
        );

        // Verify nonce account exists with expected state
        let nonce_acct = state.get_account(&nonce_pk).unwrap().unwrap();
        assert_eq!(nonce_acct.spores, NONCE_ACCOUNT_MIN_BALANCE);
        assert_eq!(nonce_acct.owner, SYSTEM_PROGRAM_ID);
        assert_eq!(nonce_acct.data[0], NONCE_ACCOUNT_MARKER);

        let ns = TxProcessor::decode_nonce_state(&nonce_acct.data).unwrap();
        assert_eq!(ns.authority, alice);
        assert_eq!(ns.blockhash, genesis_hash);
        assert_eq!(ns.fee_per_signature, BASE_FEE);
    }

    #[test]
    fn test_nonce_initialize_rejects_existing_account() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);

        // Pre-create the nonce account
        state
            .put_account(&nonce_pk, &Account::new(0, nonce_pk))
            .unwrap();

        let ix = make_nonce_init_ix(alice, nonce_pk, alice);
        let message = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(message);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(
            result.error.as_ref().unwrap().contains("already exists"),
            "Expected 'already exists' error, got: {:?}",
            result.error
        );
    }

    #[test]
    fn test_nonce_initialize_rejects_insufficient_funds() {
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();
        let processor = TxProcessor::new(state.clone());
        let treasury = Pubkey([3u8; 32]);
        state.set_treasury_pubkey(&treasury).unwrap();
        state
            .put_account(&treasury, &Account::new(0, treasury))
            .unwrap();

        // Poor alice with only 1 spore
        let alice_kp = Keypair::generate();
        let alice = alice_kp.pubkey();
        let mut poor_account = Account::new(0, alice);
        poor_account.spores = 1;
        poor_account.spendable = 1;
        state.put_account(&alice, &poor_account).unwrap();

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

        let nonce_pk = Pubkey([99u8; 32]);
        let ix = make_nonce_init_ix(alice, nonce_pk, alice);
        let message = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(message);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let validator = Pubkey([42u8; 32]);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
    }

    #[test]
    fn test_nonce_advance() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);

        // Step 1: Initialize nonce
        let ix = make_nonce_init_ix(alice, nonce_pk, alice);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(r.success, "Init failed: {:?}", r.error);

        // Step 2: Advance the nonce — need a new block so blockhash changes
        let block1 = crate::Block::new_with_timestamp(
            1,
            genesis_hash,
            Hash::default(),
            [0u8; 32],
            Vec::new(),
            1,
        );
        let block1_hash = block1.hash();
        state.put_block(&block1).unwrap();
        state.set_last_slot(1).unwrap();

        let advance_ix = make_nonce_advance_ix(alice, nonce_pk);
        let msg2 = crate::transaction::Message::new(vec![advance_ix], block1_hash);
        let mut tx2 = Transaction::new(msg2);
        tx2.signatures.push(alice_kp.sign(&tx2.message.serialize()));
        let r2 = processor.process_transaction(&tx2, &validator);
        assert!(r2.success, "Advance failed: {:?}", r2.error);

        // Verify blockhash updated
        let nonce_acct = state.get_account(&nonce_pk).unwrap().unwrap();
        let ns = TxProcessor::decode_nonce_state(&nonce_acct.data).unwrap();
        assert_eq!(ns.blockhash, block1_hash);
    }

    #[test]
    fn test_nonce_advance_rejects_same_blockhash() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);

        // Initialize nonce (stores genesis_hash)
        let ix = make_nonce_init_ix(alice, nonce_pk, alice);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        assert!(processor.process_transaction(&tx, &validator).success);

        // Try to advance without a new block — blockhash hasn't changed
        let advance_ix = make_nonce_advance_ix(alice, nonce_pk);
        let msg2 = crate::transaction::Message::new(vec![advance_ix], genesis_hash);
        let mut tx2 = Transaction::new(msg2);
        tx2.signatures.push(alice_kp.sign(&tx2.message.serialize()));
        let r = processor.process_transaction(&tx2, &validator);
        assert!(!r.success);
        assert!(r.error.as_ref().unwrap().contains("has not changed"));
    }

    #[test]
    fn test_durable_tx_with_nonce_blockhash() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);
        let bob = Pubkey([2u8; 32]);

        // Step 1: Initialize nonce (stores genesis_hash)
        let ix = make_nonce_init_ix(alice, nonce_pk, alice);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        assert!(processor.process_transaction(&tx, &validator).success);

        // Step 2: Create many new blocks to push genesis_hash out of the recent window
        let mut prev_hash = genesis_hash;
        for slot in 1..=350 {
            let block = crate::Block::new_with_timestamp(
                slot,
                prev_hash,
                Hash::default(),
                [0u8; 32],
                Vec::new(),
                slot,
            );
            prev_hash = block.hash();
            state.put_block(&block).unwrap();
            state.set_last_slot(slot).unwrap();
        }

        // Confirm genesis_hash is now too old for a normal tx
        let normal_tx = make_transfer_tx(&alice_kp, alice, bob, 1, genesis_hash);
        let normal_result = processor.process_transaction(&normal_tx, &validator);
        assert!(
            !normal_result.success,
            "Normal tx with old blockhash should fail"
        );
        assert!(normal_result
            .error
            .as_ref()
            .unwrap()
            .contains("Blockhash not found or too old"));

        // Step 3: Build a durable tx using the nonce's stored blockhash (genesis_hash)
        // First instruction = AdvanceNonce, second = Transfer
        let advance_ix = make_nonce_advance_ix(alice, nonce_pk);
        let mut transfer_data = vec![0u8];
        transfer_data.extend_from_slice(&Account::licn_to_spores(1).to_le_bytes());
        let transfer_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, bob],
            data: transfer_data,
        };

        let msg = crate::transaction::Message::new(vec![advance_ix, transfer_ix], genesis_hash);
        let mut durable_tx = Transaction::new(msg);
        durable_tx
            .signatures
            .push(alice_kp.sign(&durable_tx.message.serialize()));

        let durable_result = processor.process_transaction(&durable_tx, &validator);
        assert!(
            durable_result.success,
            "Durable nonce tx should succeed: {:?}",
            durable_result.error,
        );

        // Bob should have received 1 LICN
        assert_eq!(state.get_balance(&bob).unwrap(), Account::licn_to_spores(1));

        // Nonce should be advanced to latest blockhash
        let nonce_acct = state.get_account(&nonce_pk).unwrap().unwrap();
        let ns = TxProcessor::decode_nonce_state(&nonce_acct.data).unwrap();
        assert_eq!(ns.blockhash, prev_hash);
    }

    #[test]
    fn test_nonce_withdraw() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);
        let bob = Pubkey([2u8; 32]);

        // Initialize nonce
        initialize_test_nonce(
            &processor,
            &alice_kp,
            alice,
            nonce_pk,
            alice,
            genesis_hash,
            &validator,
        );

        // Withdraw funds to bob
        let withdraw_ix = make_nonce_withdraw_ix(alice, nonce_pk, bob, NONCE_ACCOUNT_MIN_BALANCE);
        let tx2 = make_signed_tx(&alice_kp, withdraw_ix, genesis_hash);
        let r = processor.process_transaction(&tx2, &validator);
        assert!(r.success, "Withdraw failed: {:?}", r.error);

        // Bob should have received the nonce balance
        let bob_balance = state.get_balance(&bob).unwrap();
        assert_eq!(bob_balance, NONCE_ACCOUNT_MIN_BALANCE);

        // Nonce account data should be cleared (closed)
        let nonce_acct = state.get_account(&nonce_pk).unwrap().unwrap();
        assert!(nonce_acct.data.is_empty());
    }

    #[test]
    fn test_nonce_withdraw_authority_restriction_blocks_value_exit() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);
        let bob = Pubkey([2u8; 32]);

        initialize_test_nonce(
            &processor,
            &alice_kp,
            alice,
            nonce_pk,
            alice,
            genesis_hash,
            &validator,
        );
        let before_nonce_account = state.get_account(&nonce_pk).unwrap().unwrap();

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::OutgoingOnly,
        );

        let withdraw_ix = make_nonce_withdraw_ix(alice, nonce_pk, bob, NONCE_ACCOUNT_MIN_BALANCE);
        let tx = make_signed_tx(&alice_kp, withdraw_ix, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert_failed_nonce_withdraw_keeps_nonce_open(
            &state,
            nonce_pk,
            bob,
            &result.error,
            "authority value exit blocked by active account restriction",
            &before_nonce_account,
        );
    }

    #[test]
    fn test_nonce_withdraw_authority_native_frozen_amount_blocks_value_exit() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);
        let bob = Pubkey([2u8; 32]);

        initialize_test_nonce(
            &processor,
            &alice_kp,
            alice,
            nonce_pk,
            alice,
            genesis_hash,
            &validator,
        );
        let before_nonce_account = state.get_account(&nonce_pk).unwrap().unwrap();
        let authority_spendable = state
            .get_account(&alice)
            .unwrap()
            .expect("authority account should exist")
            .spendable;

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::AccountAsset {
                account: alice,
                asset: NATIVE_LICN_ASSET_ID,
            },
            RestrictionMode::FrozenAmount {
                amount: authority_spendable,
            },
        );

        let withdraw_ix = make_nonce_withdraw_ix(alice, nonce_pk, bob, NONCE_ACCOUNT_MIN_BALANCE);
        let tx = make_signed_tx(&alice_kp, withdraw_ix, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert_failed_nonce_withdraw_keeps_nonce_open(
            &state,
            nonce_pk,
            bob,
            &result.error,
            "authority value exit blocked by active account-asset restriction",
            &before_nonce_account,
        );
    }

    #[test]
    fn test_nonce_withdraw_nonce_account_restriction_blocks_value_exit() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);
        let bob = Pubkey([2u8; 32]);

        initialize_test_nonce(
            &processor,
            &alice_kp,
            alice,
            nonce_pk,
            alice,
            genesis_hash,
            &validator,
        );
        let before_nonce_account = state.get_account(&nonce_pk).unwrap().unwrap();

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(nonce_pk),
            RestrictionMode::OutgoingOnly,
        );

        let withdraw_ix = make_nonce_withdraw_ix(alice, nonce_pk, bob, NONCE_ACCOUNT_MIN_BALANCE);
        let tx = make_signed_tx(&alice_kp, withdraw_ix, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert_failed_nonce_withdraw_keeps_nonce_open(
            &state,
            nonce_pk,
            bob,
            &result.error,
            "sender account restriction",
            &before_nonce_account,
        );
    }

    #[test]
    fn test_nonce_withdraw_recipient_restriction_blocks_without_closing_nonce() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);
        let bob = Pubkey([2u8; 32]);

        initialize_test_nonce(
            &processor,
            &alice_kp,
            alice,
            nonce_pk,
            alice,
            genesis_hash,
            &validator,
        );
        let before_nonce_account = state.get_account(&nonce_pk).unwrap().unwrap();

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(bob),
            RestrictionMode::IncomingOnly,
        );

        let withdraw_ix = make_nonce_withdraw_ix(alice, nonce_pk, bob, NONCE_ACCOUNT_MIN_BALANCE);
        let tx = make_signed_tx(&alice_kp, withdraw_ix, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert_failed_nonce_withdraw_keeps_nonce_open(
            &state,
            nonce_pk,
            bob,
            &result.error,
            "recipient account restriction",
            &before_nonce_account,
        );
    }

    #[test]
    fn test_nonce_authorize() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);
        let new_auth = Pubkey([77u8; 32]);

        // Initialize nonce with alice as authority
        let ix = make_nonce_init_ix(alice, nonce_pk, alice);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        assert!(processor.process_transaction(&tx, &validator).success);

        // Change authority to new_auth
        let mut auth_data = vec![28u8, 3u8];
        auth_data.extend_from_slice(&new_auth.0);
        let auth_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, nonce_pk],
            data: auth_data,
        };
        let msg2 = crate::transaction::Message::new(vec![auth_ix], genesis_hash);
        let mut tx2 = Transaction::new(msg2);
        tx2.signatures.push(alice_kp.sign(&tx2.message.serialize()));
        let r = processor.process_transaction(&tx2, &validator);
        assert!(r.success, "Authorize failed: {:?}", r.error);

        // Verify authority changed
        let nonce_acct = state.get_account(&nonce_pk).unwrap().unwrap();
        let ns = TxProcessor::decode_nonce_state(&nonce_acct.data).unwrap();
        assert_eq!(ns.authority, new_auth);
    }

    #[test]
    fn test_nonce_authorize_rejects_zero_authority() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);

        // Initialize
        let ix = make_nonce_init_ix(alice, nonce_pk, alice);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        assert!(processor.process_transaction(&tx, &validator).success);

        // Try to set zero authority
        let mut auth_data = vec![28u8, 3u8];
        auth_data.extend_from_slice(&[0u8; 32]);
        let auth_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, nonce_pk],
            data: auth_data,
        };
        let msg2 = crate::transaction::Message::new(vec![auth_ix], genesis_hash);
        let mut tx2 = Transaction::new(msg2);
        tx2.signatures.push(alice_kp.sign(&tx2.message.serialize()));
        let r = processor.process_transaction(&tx2, &validator);
        assert!(!r.success);
        assert!(r.error.as_ref().unwrap().contains("zero pubkey"));
    }

    #[test]
    fn test_decode_nonce_state_invalid_data() {
        // Empty data
        assert!(TxProcessor::decode_nonce_state(&[]).is_err());
        // Wrong marker
        assert!(TxProcessor::decode_nonce_state(&[0x00, 0x01]).is_err());
        // Correct marker but garbage
        assert!(TxProcessor::decode_nonce_state(&[NONCE_ACCOUNT_MARKER, 0xFF]).is_err());
    }

    #[test]
    fn test_nonce_unknown_sub_opcode() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let nonce_pk = Pubkey([99u8; 32]);

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, nonce_pk],
            data: vec![28u8, 99u8], // unknown sub-opcode
        };
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(r.error.as_ref().unwrap().contains("unknown sub-opcode"));
    }

    // ── Governance parameter change tests (system instruction type 29) ──

    /// Helper: build a governance param change instruction
    fn make_gov_param_ix(signer: Pubkey, param_id: u8, value: u64) -> Instruction {
        let mut data = vec![29u8, param_id];
        data.extend_from_slice(&value.to_le_bytes());
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![signer],
            data,
        }
    }

    #[test]
    fn test_governance_param_change_base_fee() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Set alice as governance authority
        state.set_governance_authority(&alice).unwrap();

        // Change base_fee to 2,000,000 spores (0.002 LICN)
        let new_base_fee = 2_000_000u64;
        let ix = make_gov_param_ix(alice, GOV_PARAM_BASE_FEE, new_base_fee);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(r.success, "failed: {:?}", r.error);

        // Verify it's queued but not yet applied
        let pending = state.get_pending_governance_changes().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0], (GOV_PARAM_BASE_FEE, new_base_fee));

        // Apply pending changes (simulating epoch boundary)
        let applied = state.apply_pending_governance_changes().unwrap();
        assert_eq!(applied, 1);

        // Verify the fee config was updated
        let fee_config = state.get_fee_config().unwrap();
        assert_eq!(fee_config.base_fee, new_base_fee);

        // Pending changes should be cleared
        let pending = state.get_pending_governance_changes().unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn test_governance_param_change_fee_percentages() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        state.set_governance_authority(&alice).unwrap();

        // Change burn percent to 50% and producer percent to 20%
        let ix1 = make_gov_param_ix(alice, GOV_PARAM_FEE_BURN_PERCENT, 50);
        let ix2 = make_gov_param_ix(alice, GOV_PARAM_FEE_PRODUCER_PERCENT, 20);

        // Submit both in one tx
        let msg = crate::transaction::Message::new(vec![ix1, ix2], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(r.success, "failed: {:?}", r.error);

        let pending = state.get_pending_governance_changes().unwrap();
        assert_eq!(pending.len(), 2);

        let applied = state.apply_pending_governance_changes().unwrap();
        assert_eq!(applied, 2);

        let fee_config = state.get_fee_config().unwrap();
        assert_eq!(fee_config.fee_burn_percent, 50);
        assert_eq!(fee_config.fee_producer_percent, 20);
    }

    #[test]
    fn test_governance_param_change_min_validator_stake() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        state.set_governance_authority(&alice).unwrap();

        // Change min_validator_stake to 100 LICN
        let new_stake = 100_000_000_000u64; // 100 LICN in spores
        let ix = make_gov_param_ix(alice, GOV_PARAM_MIN_VALIDATOR_STAKE, new_stake);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(r.success, "failed: {:?}", r.error);

        let applied = state.apply_pending_governance_changes().unwrap();
        assert_eq!(applied, 1);

        let stored = state.get_min_validator_stake().unwrap();
        assert_eq!(stored, Some(new_stake));
    }

    #[test]
    fn test_governance_param_change_epoch_slots() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        state.set_governance_authority(&alice).unwrap();

        // Change epoch_slots to 100,000
        let new_epoch = 100_000u64;
        let ix = make_gov_param_ix(alice, GOV_PARAM_EPOCH_SLOTS, new_epoch);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(r.success, "failed: {:?}", r.error);

        let applied = state.apply_pending_governance_changes().unwrap();
        assert_eq!(applied, 1);

        let stored = state.get_epoch_slots().unwrap();
        assert_eq!(stored, Some(new_epoch));
    }

    #[test]
    fn test_governance_param_change_rejects_non_authority() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Set a different pubkey as governance authority (not alice)
        let gov_auth = Pubkey([77u8; 32]);
        state.set_governance_authority(&gov_auth).unwrap();

        // Alice tries to submit governance change — should be rejected
        let ix = make_gov_param_ix(alice, GOV_PARAM_BASE_FEE, 2_000_000);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(
            r.error
                .as_ref()
                .unwrap()
                .contains("not the governance authority"),
            "unexpected: {:?}",
            r.error
        );
    }

    #[test]
    fn test_governance_param_change_rejects_no_authority_configured() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // No governance authority configured
        let ix = make_gov_param_ix(alice, GOV_PARAM_BASE_FEE, 2_000_000);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(
            r.error
                .as_ref()
                .unwrap()
                .contains("no governance authority configured"),
            "unexpected: {:?}",
            r.error
        );
    }

    #[test]
    fn test_governance_param_change_rejects_invalid_base_fee() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        state.set_governance_authority(&alice).unwrap();

        // base_fee = 0 (too low)
        let ix = make_gov_param_ix(alice, GOV_PARAM_BASE_FEE, 0);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(
            r.error.as_ref().unwrap().contains("base_fee must be"),
            "unexpected: {:?}",
            r.error
        );
    }

    #[test]
    fn test_governance_param_change_rejects_invalid_percentage() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        state.set_governance_authority(&alice).unwrap();

        // fee_burn_percent = 101 (too high)
        let ix = make_gov_param_ix(alice, GOV_PARAM_FEE_BURN_PERCENT, 101);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(
            r.error.as_ref().unwrap().contains("fee percentage must be"),
            "unexpected: {:?}",
            r.error
        );
    }

    #[test]
    fn test_governance_param_change_rejects_fee_split_sum_over_100() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        state.set_governance_authority(&alice).unwrap();

        let burn = make_gov_param_ix(alice, GOV_PARAM_FEE_BURN_PERCENT, 80);
        let producer = make_gov_param_ix(alice, GOV_PARAM_FEE_PRODUCER_PERCENT, 30);
        let msg = crate::transaction::Message::new(vec![burn, producer], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success, "invalid fee split must be rejected");
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("sum to 100"),
            "unexpected: {:?}",
            result.error
        );
    }

    #[test]
    fn test_governance_param_change_rejects_unknown_param() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        state.set_governance_authority(&alice).unwrap();

        // param_id = 99 (unknown)
        let ix = make_gov_param_ix(alice, 99, 1000);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(
            r.error.as_ref().unwrap().contains("unknown param_id"),
            "unexpected: {:?}",
            r.error
        );
    }

    #[test]
    fn test_governance_param_change_data_too_short() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        state.set_governance_authority(&alice).unwrap();

        // Only 2 bytes (no value)
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: vec![29u8, 0u8],
        };
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(
            r.error.as_ref().unwrap().contains("data too short"),
            "unexpected: {:?}",
            r.error
        );
    }

    #[test]
    fn test_governance_param_overwrite_pending() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        state.set_governance_authority(&alice).unwrap();

        // Queue base_fee = 2M
        let ix = make_gov_param_ix(alice, GOV_PARAM_BASE_FEE, 2_000_000);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(r.success, "failed: {:?}", r.error);

        // Overwrite with base_fee = 3M
        let ix2 = make_gov_param_ix(alice, GOV_PARAM_BASE_FEE, 3_000_000);
        let msg2 = crate::transaction::Message::new(vec![ix2], genesis_hash);
        let mut tx2 = Transaction::new(msg2);
        tx2.signatures.push(alice_kp.sign(&tx2.message.serialize()));
        let r2 = processor.process_transaction(&tx2, &validator);
        assert!(r2.success, "failed: {:?}", r2.error);

        // Only 1 pending change (overwritten), and it's the latest value
        let pending = state.get_pending_governance_changes().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0], (GOV_PARAM_BASE_FEE, 3_000_000));
    }

    #[test]
    fn test_governance_param_change_via_governed_authority_proposal_flow() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov_kp = Keypair::generate();
        let gov = gov_kp.pubkey();

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_last_slot(0).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(1),
            )
            .unwrap();

        let direct_ix = make_gov_param_ix(gov, GOV_PARAM_BASE_FEE, 2_000_000);
        let direct_msg = crate::transaction::Message::new(vec![direct_ix], genesis_hash);
        let mut direct_tx = Transaction::new(direct_msg);
        direct_tx
            .signatures
            .push(gov_kp.sign(&direct_tx.message.serialize()));
        let direct_result = processor.process_transaction(&direct_tx, &validator);
        assert!(!direct_result.success);
        assert!(direct_result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("proposal flow"));

        let mut propose_data = vec![34u8, GOVERNANCE_ACTION_PARAM_CHANGE, GOV_PARAM_BASE_FEE];
        propose_data.extend_from_slice(&2_000_000u64.to_le_bytes());
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov],
            data: propose_data,
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let result = processor.process_transaction(&propose_tx, &validator);
        assert!(
            result.success,
            "Proposal should succeed: {:?}",
            result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.action_label, "governance_param_change");
        assert_eq!(proposal.approval_authority, None);
        assert!(!proposal.executed);
        assert_eq!(proposal.execute_after_epoch, 1);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            result.success,
            "Approval should succeed: {:?}",
            result.error
        );
        assert!(state.get_pending_governance_changes().unwrap().is_empty());

        let mut execute_data = vec![36u8];
        execute_data.extend_from_slice(&1u64.to_le_bytes());
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data.clone(),
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, genesis_hash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("timelocked"));

        let fresh_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);

        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: execute_data,
        };
        let execute_tx = make_signed_tx(&bob_kp, execute_ix, fresh_blockhash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(
            result.success,
            "Execution should succeed: {:?}",
            result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(proposal.executed);
        let pending = state.get_pending_governance_changes().unwrap();
        assert_eq!(pending, vec![(GOV_PARAM_BASE_FEE, 2_000_000)]);
    }

    #[test]
    fn test_governance_treasury_transfer_velocity_policy_snapshots_escalation() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let carol_kp = Keypair::generate();
        let carol = carol_kp.pubkey();
        let gov = Pubkey([0x81; 32]);
        let recipient = Pubkey([0x82; 32]);

        state.put_account(&bob, &Account::new(1_000, bob)).unwrap();
        state
            .put_account(&carol, &Account::new(1_000, carol))
            .unwrap();
        state.put_account(&gov, &Account::new(1_000, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    1,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5)
                .with_transfer_velocity_policy(
                    crate::multisig::GovernedTransferVelocityPolicy::new(200, 200, 50, 90, 1, 3),
                ),
            )
            .unwrap();
        let treasury_authority =
            configure_treasury_executor_for_test(&state, gov, 2, vec![alice, bob, carol]);

        let mut propose_data = vec![34u8, GOVERNANCE_ACTION_TREASURY_TRANSFER];
        propose_data.extend_from_slice(&60u64.to_le_bytes());
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury_authority, recipient],
            data: propose_data,
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let result = processor.process_transaction(&propose_tx, &validator);
        assert!(result.success, "proposal failed: {:?}", result.error);

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(treasury_authority));
        assert_eq!(proposal.threshold, 3);
        assert_eq!(proposal.execute_after_epoch, 2);
        assert_eq!(
            proposal.velocity_tier,
            crate::multisig::GovernedTransferVelocityTier::Elevated
        );
        assert_eq!(proposal.daily_cap_spores, 200);
        assert!(!proposal.executed);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let result = processor.process_transaction(&approve_tx, &validator);
        assert!(result.success, "approval failed: {:?}", result.error);
        assert!(!state.get_governance_proposal(1).unwrap().unwrap().executed);

        let mut final_approve_data = vec![35u8];
        final_approve_data.extend_from_slice(&1u64.to_le_bytes());
        let final_approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![carol],
            data: final_approve_data,
        };
        let final_approve_tx = make_signed_tx(&carol_kp, final_approve_ix, genesis_hash);
        let result = processor.process_transaction(&final_approve_tx, &validator);
        assert!(result.success, "final approval failed: {:?}", result.error);
        assert!(!state.get_governance_proposal(1).unwrap().unwrap().executed);

        let mid_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);
        let mut execute_data = vec![36u8];
        execute_data.extend_from_slice(&1u64.to_le_bytes());
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data.clone(),
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, mid_blockhash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("timelocked"));

        let fresh_blockhash = advance_test_slot(&state, 2 * SLOTS_PER_EPOCH);
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data,
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, fresh_blockhash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(result.success, "execution failed: {:?}", result.error);
        assert_eq!(state.get_balance(&recipient).unwrap(), 60);
    }

    #[test]
    fn test_governance_treasury_daily_cap_defers_until_next_day() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let gov = Pubkey([0x83; 32]);
        let first_recipient = Pubkey([0x84; 32]);
        let second_recipient = Pubkey([0x85; 32]);
        let treasury_authority = crate::multisig::derive_treasury_executor_authority(&gov);

        state.put_account(&gov, &Account::new(1_000, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(1, vec![alice], "community_treasury")
                    .with_transfer_velocity_policy(
                        crate::multisig::GovernedTransferVelocityPolicy::new(200, 100, 0, 0, 0, 0),
                    ),
            )
            .unwrap();
        state
            .set_treasury_executor_authority(&treasury_authority)
            .unwrap();
        state
            .set_governed_wallet_config(
                &treasury_authority,
                &crate::multisig::GovernedWalletConfig::new(
                    1,
                    vec![alice],
                    crate::multisig::TREASURY_EXECUTOR_LABEL,
                ),
            )
            .unwrap();

        let mut first_propose_data = vec![34u8, GOVERNANCE_ACTION_TREASURY_TRANSFER];
        first_propose_data.extend_from_slice(&60u64.to_le_bytes());
        let first_propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury_authority, first_recipient],
            data: first_propose_data,
        };
        let first_propose_tx = make_signed_tx(&alice_kp, first_propose_ix, genesis_hash);
        let result = processor.process_transaction(&first_propose_tx, &validator);
        assert!(result.success, "first transfer failed: {:?}", result.error);
        assert!(state.get_governance_proposal(1).unwrap().unwrap().executed);

        let mut second_propose_data = vec![34u8, GOVERNANCE_ACTION_TREASURY_TRANSFER];
        second_propose_data.extend_from_slice(&50u64.to_le_bytes());
        let second_propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury_authority, second_recipient],
            data: second_propose_data,
        };
        let second_propose_tx = make_signed_tx(&alice_kp, second_propose_ix, genesis_hash);
        let result = processor.process_transaction(&second_propose_tx, &validator);
        assert!(result.success, "second proposal failed: {:?}", result.error);

        let second_proposal = state.get_governance_proposal(2).unwrap().unwrap();
        assert!(!second_proposal.executed);
        assert_eq!(state.get_balance(&second_recipient).unwrap(), 0);
        assert_eq!(state.get_governed_transfer_day_volume(&gov, 0).unwrap(), 60);

        let mut execute_data = vec![36u8];
        execute_data.extend_from_slice(&2u64.to_le_bytes());
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data.clone(),
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, genesis_hash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("daily cap"));

        let fresh_blockhash = advance_test_slot(&state, SECONDS_PER_DAY);
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data,
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, fresh_blockhash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(
            result.success,
            "deferred execute failed: {:?}",
            result.error
        );
        assert!(state.get_governance_proposal(2).unwrap().unwrap().executed);
        assert_eq!(state.get_balance(&second_recipient).unwrap(), 50);
        assert_eq!(state.get_governed_transfer_day_volume(&gov, 1).unwrap(), 50);
    }

    #[test]
    fn test_governance_treasury_transfer_rejects_general_governance_authority_when_split_is_configured(
    ) {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob = Pubkey([0xA7; 32]);
        let gov = Pubkey([0xA8; 32]);
        let recipient = Pubkey([0xA9; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_transfer_velocity_policy(
                    crate::multisig::GovernedTransferVelocityPolicy::community_treasury_defaults(),
                ),
            )
            .unwrap();
        configure_treasury_executor_for_test(&state, gov, 2, vec![alice, bob]);

        let mut propose_data = vec![34u8, GOVERNANCE_ACTION_TREASURY_TRANSFER];
        propose_data.extend_from_slice(&Account::licn_to_spores(10).to_le_bytes());
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, recipient],
            data: propose_data,
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let result = processor.process_transaction(&propose_tx, &validator);
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains(
            "Protocol fund movement governance actions must use the treasury executor approval authority"
        ));
    }

    // ──────────────────────────────────────────────────────────────
    // Compute-unit metering tests (Task 2.12)
    // ──────────────────────────────────────────────────────────────

    #[test]
    fn test_cu_lookup_transfer() {
        assert_eq!(compute_units_for_system_ix(0), CU_TRANSFER);
        // Multi-transfer variants (types 2-5) should match
        for t in 2..=5u8 {
            assert_eq!(compute_units_for_system_ix(t), CU_TRANSFER);
        }
    }

    #[test]
    fn test_cu_lookup_stake_unstake() {
        assert_eq!(compute_units_for_system_ix(9), CU_STAKE);
        assert_eq!(compute_units_for_system_ix(10), CU_UNSTAKE);
        assert_eq!(compute_units_for_system_ix(11), CU_CLAIM_UNSTAKE);
    }

    #[test]
    fn test_cu_lookup_nft() {
        assert_eq!(compute_units_for_system_ix(7), CU_MINT_NFT);
        assert_eq!(compute_units_for_system_ix(8), CU_TRANSFER_NFT);
    }

    #[test]
    fn test_cu_lookup_zk() {
        assert_eq!(compute_units_for_system_ix(23), CU_ZK_SHIELD);
        assert_eq!(compute_units_for_system_ix(24), CU_ZK_TRANSFER);
        assert_eq!(compute_units_for_system_ix(25), CU_ZK_TRANSFER);
    }

    #[test]
    fn test_cu_lookup_deploy_contract() {
        assert_eq!(compute_units_for_system_ix(17), CU_DEPLOY_CONTRACT);
    }

    #[test]
    fn test_cu_lookup_governance() {
        assert_eq!(compute_units_for_system_ix(29), CU_GOVERNANCE_PARAM);
    }

    #[test]
    fn test_cu_lookup_unknown_defaults_to_100() {
        assert_eq!(compute_units_for_system_ix(200), 100);
        assert_eq!(compute_units_for_system_ix(255), 100);
    }

    #[test]
    fn test_cu_for_tx_single_transfer() {
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![Pubkey([1; 32]), Pubkey([2; 32])],
            data: vec![0u8, 0, 0, 0, 0, 0, 0, 0, 0], // type 0 = transfer
        };
        let msg = crate::transaction::Message::new(vec![ix], Hash::default());
        let tx = Transaction::new(msg);
        assert_eq!(compute_units_for_tx(&tx), CU_TRANSFER);
    }

    #[test]
    fn test_cu_for_tx_multi_ix_sums() {
        let ix_transfer = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![Pubkey([1; 32]), Pubkey([2; 32])],
            data: vec![0u8, 0, 0, 0, 0, 0, 0, 0, 0],
        };
        let ix_stake = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![Pubkey([1; 32])],
            data: vec![9u8, 0, 0, 0, 0, 0, 0, 0, 0],
        };
        let msg = crate::transaction::Message::new(vec![ix_transfer, ix_stake], Hash::default());
        let tx = Transaction::new(msg);
        assert_eq!(compute_units_for_tx(&tx), CU_TRANSFER + CU_STAKE);
    }

    #[test]
    fn test_cu_for_tx_ignores_contract_ix() {
        let ix_system = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![Pubkey([1; 32]), Pubkey([2; 32])],
            data: vec![0u8, 0, 0, 0, 0, 0, 0, 0, 0],
        };
        let ix_contract = Instruction {
            program_id: Pubkey([0xFF; 32]), // CONTRACT_PROGRAM_ID
            accounts: vec![Pubkey([3; 32])],
            data: vec![1, 2, 3],
        };
        let msg = crate::transaction::Message::new(vec![ix_system, ix_contract], Hash::default());
        let tx = Transaction::new(msg);
        // Only the system instruction counts — contract CU is tracked by WASM runtime
        assert_eq!(compute_units_for_tx(&tx), CU_TRANSFER);
    }

    #[test]
    fn test_tx_result_has_compute_units_after_transfer() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        let tx = make_transfer_tx(&alice_kp, alice, bob, 10, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);

        assert!(
            result.success,
            "transfer should succeed: {:?}",
            result.error
        );
        assert_eq!(result.compute_units_used, CU_TRANSFER);
    }

    // ────────────────────────────────────────────────────────────────────────
    // Task 3.6 — Oracle Multi-Source Attestation Tests
    // ────────────────────────────────────────────────────────────────────────

    /// Helper: build an oracle attestation instruction
    fn make_oracle_attestation_ix(
        signer: Pubkey,
        asset: &str,
        price: u64,
        decimals: u8,
    ) -> Instruction {
        let asset_bytes = asset.as_bytes();
        let mut data = vec![30u8, asset_bytes.len() as u8];
        data.extend_from_slice(asset_bytes);
        data.extend_from_slice(&price.to_le_bytes());
        data.push(decimals);
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![signer],
            data,
        }
    }

    /// Helper: set up a validator with active stake in the stake pool
    fn setup_active_validator(state: &StateStore, pubkey: &Pubkey, stake_spores: u64) {
        let mut pool = state
            .get_stake_pool()
            .unwrap_or_else(|_| crate::consensus::StakePool::new());
        // Use stake() which requires >= MIN_VALIDATOR_STAKE
        pool.stake(*pubkey, stake_spores, 0).unwrap();
        state.put_stake_pool(&pool).unwrap();
    }

    #[test]
    fn test_oracle_attestation_basic_submit() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Make alice an active validator
        setup_active_validator(&state, &alice, MIN_VALIDATOR_STAKE);

        // Submit price attestation: LICN = 1.50 (150_000_000 at 8 decimals)
        let ix = make_oracle_attestation_ix(alice, "LICN", 150_000_000, 8);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(r.success, "Attestation should succeed: {:?}", r.error);

        // Verify attestation was stored
        let attestations = state
            .get_oracle_attestations("LICN", 0, ORACLE_STALENESS_SLOTS)
            .unwrap();
        assert_eq!(attestations.len(), 1);
        assert_eq!(attestations[0].price, 150_000_000);
        assert_eq!(attestations[0].decimals, 8);
        assert_eq!(attestations[0].validator, alice);
    }

    #[test]
    fn test_oracle_attestation_rejects_non_validator() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Alice is NOT a validator (no stake)
        let ix = make_oracle_attestation_ix(alice, "LICN", 150_000_000, 8);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(
            r.error.as_ref().unwrap().contains("no stake"),
            "unexpected: {:?}",
            r.error
        );
    }

    #[test]
    fn test_oracle_attestation_rejects_zero_price() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        setup_active_validator(&state, &alice, MIN_VALIDATOR_STAKE);

        let ix = make_oracle_attestation_ix(alice, "LICN", 0, 8);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(
            r.error.as_ref().unwrap().contains("price must be > 0"),
            "unexpected: {:?}",
            r.error
        );
    }

    #[test]
    fn test_oracle_attestation_rejects_invalid_decimals() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        setup_active_validator(&state, &alice, MIN_VALIDATOR_STAKE);

        let ix = make_oracle_attestation_ix(alice, "LICN", 100, 19);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(
            r.error.as_ref().unwrap().contains("decimals must be"),
            "unexpected: {:?}",
            r.error
        );
    }

    #[test]
    fn test_oracle_attestation_rejects_empty_asset() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        setup_active_validator(&state, &alice, MIN_VALIDATOR_STAKE);

        // Build manually with asset_len = 0
        let mut data = vec![30u8, 0u8]; // asset_len = 0
        data.extend_from_slice(&100u64.to_le_bytes());
        data.push(8);
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data,
        };
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(
            r.error.as_ref().unwrap().contains("asset name length"),
            "unexpected: {:?}",
            r.error
        );
    }

    #[test]
    fn test_oracle_attestation_rejects_too_long_asset() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        setup_active_validator(&state, &alice, MIN_VALIDATOR_STAKE);

        // Asset name = 17 bytes (over max 16)
        let long_asset = "ABCDEFGHIJKLMNOPQ"; // 17 chars
        let ix = make_oracle_attestation_ix(alice, long_asset, 100, 8);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(
            r.error.as_ref().unwrap().contains("asset name length"),
            "unexpected: {:?}",
            r.error
        );
    }

    #[test]
    fn test_oracle_attestation_data_too_short() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        setup_active_validator(&state, &alice, MIN_VALIDATOR_STAKE);

        // Only 3 bytes (opcode + asset_len + 1 byte of asset, missing price + decimals)
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: vec![30u8, 4u8, b'M', b'O'],
        };
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(!r.success);
        assert!(
            r.error.as_ref().unwrap().contains("data too short"),
            "unexpected: {:?}",
            r.error
        );
    }

    #[test]
    fn test_oracle_quorum_consensus_price() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();

        // Three validators with UNEQUAL stakes to test 2/3 threshold boundary.
        // Alice: 1 MIN, Bob: 1 MIN, Carol: 4 MIN → total = 6 MIN, threshold = 4 MIN.
        // Alice alone (1 MIN) < threshold (4 MIN) → no quorum.
        // Alice + Bob (2 MIN) < threshold (4 MIN) → still no quorum.
        // Alice + Bob + Carol (6 MIN) >= threshold → quorum (and >= 2 attestors).
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let carol_kp = Keypair::generate();
        let carol = carol_kp.pubkey();

        // Fund bob and carol
        state.put_account(&bob, &Account::new(1000, bob)).unwrap();
        state
            .put_account(&carol, &Account::new(1000, carol))
            .unwrap();

        let stake = MIN_VALIDATOR_STAKE;
        {
            let mut pool = crate::consensus::StakePool::new();
            pool.stake(alice, stake, 0).unwrap();
            pool.stake(bob, stake, 0).unwrap();
            pool.stake(carol, stake * 4, 0).unwrap();
            state.put_stake_pool(&pool).unwrap();
        }
        // total = 6*stake, threshold = 6*stake*2/3 = 4*stake

        let block_producer = Pubkey([42u8; 32]);

        // Alice attests: LICN = 150 (stake = 1 MIN < threshold 4 MIN → no quorum)
        let ix = make_oracle_attestation_ix(alice, "LICN", 150, 8);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &block_producer);
        assert!(r.success, "Alice attestation failed: {:?}", r.error);

        // 1 MIN < 4 MIN threshold → no consensus
        let cp = state.get_oracle_consensus_price("LICN").unwrap();
        assert!(
            cp.is_none(),
            "Should NOT have consensus below 2/3 threshold"
        );

        // Bob attests: LICN = 160 (combined stake = 2 MIN < threshold 4 MIN → still no quorum)
        let ix = make_oracle_attestation_ix(bob, "LICN", 160, 8);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(bob_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &block_producer);
        assert!(r.success, "Bob attestation failed: {:?}", r.error);

        // 2 MIN < 4 MIN threshold → still no consensus with only 2 small validators
        let cp = state.get_oracle_consensus_price("LICN").unwrap();
        assert!(
            cp.is_none(),
            "Should NOT have consensus below 2/3 threshold (2 of 6 stake)"
        );

        // Carol attests: LICN = 155 (combined stake = 6 MIN >= threshold 4 MIN → quorum)
        let ix = make_oracle_attestation_ix(carol, "LICN", 155, 8);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(carol_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &block_producer);
        assert!(r.success, "Carol attestation failed: {:?}", r.error);

        // 6 MIN >= 4 MIN threshold, 3 attestors >= 2 → consensus reached
        let cp = state.get_oracle_consensus_price("LICN").unwrap();
        assert!(cp.is_some(), "Should have consensus with all validators");
        let cp = cp.unwrap();
        assert_eq!(cp.attestation_count, 3);
        // Sorted: [150 (1 MIN), 155 (4 MIN), 160 (1 MIN)].
        // Total stake = 6 MIN, half = 3 MIN.
        // Cumulative: 150→1 MIN (<3), 155→5 MIN (>=3) → median = 155
        assert_eq!(
            cp.price, 155,
            "Stake-weighted median of [150,155,160] with unequal stakes"
        );
    }

    #[test]
    fn test_parallel_oracle_attestations_are_scheduled_sequentially() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let carol_kp = Keypair::generate();
        let carol = carol_kp.pubkey();
        let block_producer = Pubkey([42u8; 32]);

        state
            .put_account(&bob, &Account::new(1_000_000_000_000, bob))
            .unwrap();
        state
            .put_account(&carol, &Account::new(1_000_000_000_000, carol))
            .unwrap();

        setup_active_validator(&state, &alice, MIN_VALIDATOR_STAKE);
        setup_active_validator(&state, &bob, MIN_VALIDATOR_STAKE);
        setup_active_validator(&state, &carol, MIN_VALIDATOR_STAKE);

        let mut txs = Vec::new();
        for (kp, signer, price) in [
            (&alice_kp, alice, 100u64),
            (&bob_kp, bob, 200u64),
            (&carol_kp, carol, 300u64),
        ] {
            let ix = make_oracle_attestation_ix(signer, "LICN", price, 8);
            let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
            let mut tx = Transaction::new(msg);
            tx.signatures.push(kp.sign(&tx.message.serialize()));
            txs.push(tx);
        }

        let results = processor.process_transactions_parallel(&txs, &block_producer);
        for (idx, result) in results.iter().enumerate() {
            assert!(
                result.success,
                "oracle attestation tx {} failed: {:?}",
                idx, result.error
            );
        }

        let consensus = state
            .get_oracle_consensus_price("LICN")
            .unwrap()
            .expect("oracle quorum should be reached after three attestations");
        assert_eq!(consensus.price, 200);
        assert_eq!(consensus.attestation_count, 3);
    }

    #[test]
    fn test_oracle_validator_replaces_own_attestation() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        setup_active_validator(&state, &alice, MIN_VALIDATOR_STAKE);

        // First attestation: price = 100
        let ix = make_oracle_attestation_ix(alice, "LICN", 100, 8);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(r.success, "first: {:?}", r.error);

        // Second attestation: price = 200 (should replace)
        let ix = make_oracle_attestation_ix(alice, "LICN", 200, 8);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(r.success, "second: {:?}", r.error);

        // Should only have 1 attestation (replaced, not appended)
        let atts = state
            .get_oracle_attestations("LICN", 0, ORACLE_STALENESS_SLOTS)
            .unwrap();
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].price, 200);
    }

    #[test]
    fn test_oracle_multi_asset_independence() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        setup_active_validator(&state, &alice, MIN_VALIDATOR_STAKE);

        // Attest LICN
        let ix = make_oracle_attestation_ix(alice, "LICN", 150, 8);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(r.success, "LICN: {:?}", r.error);

        // Attest wETH
        let ix = make_oracle_attestation_ix(alice, "wETH", 345_000, 8);
        let msg = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(msg);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));
        let r = processor.process_transaction(&tx, &validator);
        assert!(r.success, "wETH: {:?}", r.error);

        // Check each asset independently
        let licn_atts = state
            .get_oracle_attestations("LICN", 0, ORACLE_STALENESS_SLOTS)
            .unwrap();
        let weth_atts = state
            .get_oracle_attestations("wETH", 0, ORACLE_STALENESS_SLOTS)
            .unwrap();
        assert_eq!(licn_atts.len(), 1);
        assert_eq!(weth_atts.len(), 1);
        assert_eq!(licn_atts[0].price, 150);
        assert_eq!(weth_atts[0].price, 345_000);
    }

    #[test]
    fn test_oracle_compute_units() {
        assert_eq!(compute_units_for_system_ix(30), CU_ORACLE_ATTESTATION);
    }

    #[test]
    fn test_stake_weighted_median_single() {
        let atts = vec![OracleAttestation {
            validator: Pubkey([1u8; 32]),
            price: 100,
            decimals: 8,
            stake: 1000,
            slot: 0,
        }];
        assert_eq!(compute_stake_weighted_median(&atts), 100);
    }

    #[test]
    fn test_stake_weighted_median_equal_stakes() {
        let atts = vec![
            OracleAttestation {
                validator: Pubkey([1u8; 32]),
                price: 100,
                decimals: 8,
                stake: 1000,
                slot: 0,
            },
            OracleAttestation {
                validator: Pubkey([2u8; 32]),
                price: 200,
                decimals: 8,
                stake: 1000,
                slot: 0,
            },
            OracleAttestation {
                validator: Pubkey([3u8; 32]),
                price: 300,
                decimals: 8,
                stake: 1000,
                slot: 0,
            },
        ];
        // Sorted: [100, 200, 300], total=3000, half=1500
        // Cumulative: 1000, 2000, 3000 → crosses at 200
        assert_eq!(compute_stake_weighted_median(&atts), 200);
    }

    #[test]
    fn test_stake_weighted_median_unequal_stakes() {
        let atts = vec![
            OracleAttestation {
                validator: Pubkey([1u8; 32]),
                price: 100,
                decimals: 8,
                stake: 100,
                slot: 0,
            },
            OracleAttestation {
                validator: Pubkey([2u8; 32]),
                price: 200,
                decimals: 8,
                stake: 100,
                slot: 0,
            },
            OracleAttestation {
                validator: Pubkey([3u8; 32]),
                price: 300,
                decimals: 8,
                stake: 800,
                slot: 0,
            },
        ];
        // Sorted: [100, 200, 300], total=1000, half=500
        // Cumulative: 100, 200, 1000 → crosses at 300 (the whale's price dominates)
        assert_eq!(compute_stake_weighted_median(&atts), 300);
    }

    #[test]
    fn test_stake_weighted_median_empty() {
        let atts: Vec<OracleAttestation> = vec![];
        assert_eq!(compute_stake_weighted_median(&atts), 0);
    }

    // ────────────────────────────────────────────────────────────────────────
    // Task 3.3 — Contract Upgrade Timelock Tests
    // ────────────────────────────────────────────────────────────────────────

    /// Helper: deploy a minimal WASM contract and return the contract address and loaded ContractAccount.
    fn deploy_test_contract_with_code(
        processor: &TxProcessor,
        state: &StateStore,
        deployer_kp: &crate::Keypair,
        deployer: Pubkey,
        code: Vec<u8>,
        genesis_hash: Hash,
        validator: &Pubkey,
    ) -> Pubkey {
        let code_hash = Hash::hash(&code);
        let mut addr_bytes = [0u8; 32];
        addr_bytes[..16].copy_from_slice(&deployer.0[..16]);
        addr_bytes[16..].copy_from_slice(&code_hash.0[..16]);
        let contract_addr = Pubkey(addr_bytes);

        let contract_ix = crate::ContractInstruction::Deploy {
            code,
            init_data: Vec::new(),
        };
        let ix = Instruction {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: vec![deployer, contract_addr],
            data: contract_ix.serialize().unwrap(),
        };
        let message = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(message);
        tx.signatures
            .push(deployer_kp.sign(&tx.message.serialize()));
        let result = processor.process_transaction(&tx, validator);
        assert!(result.success, "deploy should succeed: {:?}", result.error);

        let acct = state.get_account(&contract_addr).unwrap();
        assert!(acct.is_some() && acct.unwrap().executable);
        contract_addr
    }

    fn deploy_test_contract(
        processor: &TxProcessor,
        state: &StateStore,
        deployer_kp: &crate::Keypair,
        deployer: Pubkey,
        genesis_hash: Hash,
        validator: &Pubkey,
    ) -> Pubkey {
        deploy_test_contract_with_code(
            processor,
            state,
            deployer_kp,
            deployer,
            vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00],
            genesis_hash,
            validator,
        )
    }

    fn install_test_contract_account(state: &StateStore, owner: Pubkey, code: Vec<u8>) -> Pubkey {
        let code_hash = Hash::hash(&code);
        let mut addr_bytes = [0u8; 32];
        addr_bytes[..16].copy_from_slice(&owner.0[..16]);
        addr_bytes[16..].copy_from_slice(&code_hash.0[..16]);
        let contract_addr = Pubkey(addr_bytes);

        let contract = crate::ContractAccount::new(code, owner);
        let mut account = Account::new(0, contract_addr);
        account.data = serde_json::to_vec(&contract).unwrap();
        account.executable = true;
        state.put_account(&contract_addr, &account).unwrap();
        contract_addr
    }

    fn set_contract_lifecycle_status_for_test(
        state: &StateStore,
        contract_addr: Pubkey,
        status: crate::ContractLifecycleStatus,
    ) {
        let mut account = state.get_account(&contract_addr).unwrap().unwrap();
        let mut contract: crate::ContractAccount = serde_json::from_slice(&account.data).unwrap();
        contract.lifecycle_status = status;
        contract.lifecycle_updated_slot = 99;
        contract.lifecycle_restriction_id = Some(7);
        account.data = serde_json::to_vec(&contract).unwrap();
        state.put_account(&contract_addr, &account).unwrap();
    }

    fn load_contract_account_for_test(
        state: &StateStore,
        contract_addr: Pubkey,
    ) -> crate::ContractAccount {
        let account = state.get_account(&contract_addr).unwrap().unwrap();
        serde_json::from_slice(&account.data).unwrap()
    }

    /// Helper: build and submit a contract instruction tx.
    fn submit_contract_ix(
        processor: &TxProcessor,
        signer_kp: &crate::Keypair,
        accounts: Vec<Pubkey>,
        contract_ix: crate::ContractInstruction,
        genesis_hash: Hash,
        validator: &Pubkey,
    ) -> crate::TxResult {
        let ix = Instruction {
            program_id: CONTRACT_PROGRAM_ID,
            accounts,
            data: contract_ix.serialize().unwrap(),
        };
        let message = crate::transaction::Message::new(vec![ix], genesis_hash);
        let mut tx = Transaction::new(message);
        tx.signatures.push(signer_kp.sign(&tx.message.serialize()));
        processor.process_transaction(&tx, validator)
    }

    /// Helper: build a valid minimal WASM module distinct from the base module.
    /// Appends a custom section with the given tag byte so each call produces a
    /// different (but valid) WASM binary.
    fn valid_wasm_code(tag: u8) -> Vec<u8> {
        // magic + version + custom section (id=0, payload_len=2, name_len=1, name=tag)
        vec![
            0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x01, tag,
        ]
    }

    fn governance_test_contract_code() -> Vec<u8> {
        wat::parse_str(
            r#"(module
                (import "env" "storage_write" (func $storage_write (param i32 i32 i32 i32) (result i32)))
                (import "env" "get_caller" (func $get_caller (param i32) (result i32)))
                (import "env" "get_args_len" (func $get_args_len (result i32)))
                (import "env" "get_args" (func $get_args (param i32 i32) (result i32)))
                (memory (export "memory") 1)
                (data (i32.const 0) "last_caller")
                (data (i32.const 16) "last_args")
                (func $record_call_impl
                    (local $args_len i32)
                    (drop (call $get_caller (i32.const 64)))
                    (drop (call $storage_write (i32.const 0) (i32.const 11) (i32.const 64) (i32.const 32)))
                    (local.set $args_len (call $get_args_len))
                    (drop (call $get_args (i32.const 96) (local.get $args_len)))
                    (drop (call $storage_write (i32.const 16) (i32.const 9) (i32.const 96) (local.get $args_len))))
                (func (export "record_call")
                    (call $record_call_impl))
                (func (export "add_bridge_validator")
                    (call $record_call_impl))
                (func (export "set_required_confirmations")
                    (call $record_call_impl))
                (func (export "set_request_timeout")
                    (call $record_call_impl))
                (func (export "add_price_feeder")
                    (call $record_call_impl))
                (func (export "set_authorized_attester")
                    (call $record_call_impl))
                (func (export "mb_pause")
                    (call $record_call_impl))
                (func (export "mb_unpause")
                    (call $record_call_impl))
                (func (export "cv_pause")
                    (call $record_call_impl))
                (func (export "cv_unpause")
                    (call $record_call_impl))
                (func (export "ms_pause")
                    (call $record_call_impl))
                (func (export "ms_unpause")
                    (call $record_call_impl))
                (func (export "pause")
                    (call $record_call_impl))
                (func (export "unpause")
                    (call $record_call_impl))
                (func (export "bb_pause")
                    (call $record_call_impl))
                (func (export "bb_unpause")
                    (call $record_call_impl))
                (func (export "emergency_pause")
                    (call $record_call_impl))
                (func (export "emergency_unpause")
                    (call $record_call_impl))
                (func (export "pause_pair")
                    (call $record_call_impl))
                (func (export "call")
                    (call $record_call_impl))
            )"#,
        )
        .expect("governance test contract should compile")
    }

    fn wat_bytes(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("\\{:02x}", byte)).collect()
    }

    fn reputation_reader_contract_code(rep_key: &[u8]) -> Vec<u8> {
        wat::parse_str(format!(
            r#"(module
                (import "env" "storage_read" (func $storage_read (param i32 i32 i32 i32) (result i32)))
                (import "env" "set_return_data" (func $set_return_data (param i32 i32) (result i32)))
                (memory (export "memory") 1)
                (data (i32.const 0) "{rep_key_data}")
                (func (export "read_reputation") (result i32)
                    (local $written i32)
                    (local.set $written
                        (call $storage_read (i32.const 0) (i32.const {rep_key_len}) (i32.const 96) (i32.const 8)))
                    (drop (call $set_return_data (i32.const 96) (local.get $written)))
                    (i32.const 0))
            )"#,
            rep_key_data = wat_bytes(rep_key),
            rep_key_len = rep_key.len(),
        ))
        .expect("reputation reader contract should compile")
    }

    fn assert_governed_committee_contract_call_requires_proposal(
        function: &str,
        call_args: Vec<u8>,
    ) {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov_kp = Keypair::generate();
        let gov = gov_kp.pubkey();

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_last_slot(0).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(1),
            )
            .unwrap();

        let contract_addr =
            install_test_contract_account(&state, alice, governance_test_contract_code());

        let direct = submit_contract_ix(
            &processor,
            &gov_kp,
            vec![gov, contract_addr],
            crate::ContractInstruction::Call {
                function: function.to_string(),
                args: call_args.clone(),
                value: 0,
            },
            genesis_hash,
            &validator,
        );
        assert!(!direct.success);
        assert!(direct
            .error
            .as_deref()
            .unwrap_or("")
            .contains("proposal flow"));

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_addr],
            data: make_governance_contract_call_data(function, &call_args, 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let result = processor.process_transaction(&propose_tx, &validator);
        assert!(
            result.success,
            "Proposal should succeed: {:?}",
            result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.approval_authority, None);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            result.success,
            "Approval should succeed: {:?}",
            result.error
        );

        let mut execute_data = vec![36u8];
        execute_data.extend_from_slice(&1u64.to_le_bytes());
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data.clone(),
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, genesis_hash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("timelocked"));

        let fresh_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);

        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: execute_data,
        };
        let execute_tx = make_signed_tx(&bob_kp, execute_ix, fresh_blockhash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(
            result.success,
            "Execution should succeed: {:?}",
            result.error
        );

        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_caller")
                .unwrap()
                .unwrap(),
            gov.0.to_vec()
        );
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_args")
                .unwrap()
                .unwrap(),
            call_args
        );
    }

    fn make_governance_contract_call_data(function: &str, args: &[u8], value: u64) -> Vec<u8> {
        let function_bytes = function.as_bytes();
        assert!(u16::try_from(function_bytes.len()).is_ok());
        let mut data = vec![34u8, GOVERNANCE_ACTION_CONTRACT_CALL];
        data.extend_from_slice(&value.to_le_bytes());
        data.extend_from_slice(&(function_bytes.len() as u16).to_le_bytes());
        data.extend_from_slice(function_bytes);
        data.extend_from_slice(&(args.len() as u32).to_le_bytes());
        data.extend_from_slice(args);
        data
    }

    fn assert_contract_record_call_not_mutated(state: &StateStore, contract_addr: Pubkey) {
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_args")
                .unwrap(),
            None
        );
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_caller")
                .unwrap(),
            None
        );
    }

    #[test]
    fn test_contract_lifecycle_active_allows_state_changing_wasm_execution() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let contract_addr =
            install_test_contract_account(&state, alice, governance_test_contract_code());
        let args = vec![1, 2, 3];

        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Call {
                function: "record_call".to_string(),
                args: args.clone(),
                value: 0,
            },
            genesis_hash,
            &validator,
        );

        assert!(
            result.success,
            "active call should succeed: {:?}",
            result.error
        );
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_args")
                .unwrap()
                .unwrap(),
            args
        );
    }

    #[test]
    fn test_contract_lifecycle_suspended_rejects_state_changing_wasm_before_execution() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let contract_addr =
            install_test_contract_account(&state, alice, governance_test_contract_code());
        set_contract_lifecycle_status_for_test(
            &state,
            contract_addr,
            crate::ContractLifecycleStatus::Suspended,
        );

        let before_caller_balance = state.get_balance(&alice).unwrap();
        let before_contract_balance = state.get_balance(&contract_addr).unwrap_or(0);
        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Call {
                function: "record_call".to_string(),
                args: vec![4, 5, 6],
                value: 123,
            },
            genesis_hash,
            &validator,
        );

        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("lifecycle suspended"));
        assert_contract_record_call_not_mutated(&state, contract_addr);
        assert_eq!(
            state.get_balance(&contract_addr).unwrap_or(0),
            before_contract_balance
        );
        assert_eq!(
            before_caller_balance - state.get_balance(&alice).unwrap(),
            result.fee_paid
        );
    }

    #[test]
    fn test_contract_lifecycle_quarantined_rejects_wasm_before_execution() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let contract_addr =
            install_test_contract_account(&state, alice, governance_test_contract_code());
        set_contract_lifecycle_status_for_test(
            &state,
            contract_addr,
            crate::ContractLifecycleStatus::Quarantined,
        );

        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Call {
                function: "record_call".to_string(),
                args: vec![7],
                value: 0,
            },
            genesis_hash,
            &validator,
        );

        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("lifecycle quarantined"));
        assert_contract_record_call_not_mutated(&state, contract_addr);
    }

    #[test]
    fn test_contract_lifecycle_terminated_rejects_wasm_before_execution() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let contract_addr =
            install_test_contract_account(&state, alice, governance_test_contract_code());
        set_contract_lifecycle_status_for_test(
            &state,
            contract_addr,
            crate::ContractLifecycleStatus::Terminated,
        );

        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Call {
                function: "record_call".to_string(),
                args: vec![8],
                value: 0,
            },
            genesis_hash,
            &validator,
        );

        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("lifecycle terminated"));
        assert_contract_record_call_not_mutated(&state, contract_addr);
    }

    #[test]
    fn test_contract_lifecycle_simulation_rejects_blocked_contract_before_execution() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let contract_addr =
            install_test_contract_account(&state, alice, governance_test_contract_code());
        set_contract_lifecycle_status_for_test(
            &state,
            contract_addr,
            crate::ContractLifecycleStatus::Suspended,
        );

        let ix = Instruction {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: vec![alice, contract_addr],
            data: crate::ContractInstruction::Call {
                function: "record_call".to_string(),
                args: vec![9],
                value: 0,
            }
            .serialize()
            .unwrap(),
        };
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.simulate_transaction(&tx);

        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("lifecycle suspended"));
        assert_eq!(result.state_changes, 0);
        assert_contract_record_call_not_mutated(&state, contract_addr);
    }

    #[test]
    fn restriction_governance_contract_suspend_and_lift_drive_lifecycle_without_owner_spoofing() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 0);
        let validator = Pubkey([42u8; 32]);
        let contract_addr =
            install_test_contract_account(&state, alice, governance_test_contract_code());

        let restrict_data = make_restrict_action_data(
            target_pubkey_payload(3, contract_addr),
            &RestrictionMode::StateChangingBlocked,
            RestrictionReason::TestnetDrill,
            None,
            None,
            None,
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            restrict_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "contract restriction proposal should be stored: {:?}",
            result.error
        );
        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 1, genesis_hash, &validator);
        assert!(
            result.success,
            "contract restriction approval should execute: {:?}",
            result.error
        );

        let contract = load_contract_account_for_test(&state, contract_addr);
        assert_eq!(
            contract.lifecycle_status,
            crate::ContractLifecycleStatus::Suspended
        );
        assert_eq!(contract.lifecycle_restriction_id, Some(1));
        assert_eq!(contract.owner, alice);

        let destination = Pubkey([0xE1; 32]);
        let close_result = submit_contract_ix(
            &processor,
            &bob_kp,
            vec![bob, contract_addr, destination],
            crate::ContractInstruction::Close,
            genesis_hash,
            &validator,
        );
        assert!(!close_result.success);
        assert!(close_result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Only contract owner can close"));

        let blocked_call = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Call {
                function: "record_call".to_string(),
                args: vec![4, 5, 6],
                value: 0,
            },
            genesis_hash,
            &validator,
        );
        assert!(!blocked_call.success);
        assert!(blocked_call
            .error
            .as_deref()
            .unwrap_or("")
            .contains("lifecycle suspended"));
        assert_contract_record_call_not_mutated(&state, contract_addr);

        let lift_data =
            make_lift_restriction_action_data(1, RestrictionLiftReason::IncidentResolved);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            lift_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "contract restriction lift proposal should be stored: {:?}",
            result.error
        );
        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 2, genesis_hash, &validator);
        assert!(
            result.success,
            "contract restriction lift should execute: {:?}",
            result.error
        );

        let contract = load_contract_account_for_test(&state, contract_addr);
        assert_eq!(
            contract.lifecycle_status,
            crate::ContractLifecycleStatus::Active
        );
        assert_eq!(contract.lifecycle_restriction_id, None);

        let args = vec![1, 2, 3];
        let allowed_call = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Call {
                function: "record_call".to_string(),
                args: args.clone(),
                value: 0,
            },
            genesis_hash,
            &validator,
        );
        assert!(
            allowed_call.success,
            "lifted contract call should succeed: {:?}",
            allowed_call.error
        );
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_args")
                .unwrap()
                .unwrap(),
            args
        );
    }

    #[test]
    fn restriction_governance_contract_temporary_restriction_expires_and_resumes_on_next_call() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 5);
        let validator = Pubkey([42u8; 32]);
        let guardian_authority =
            configure_incident_guardian_for_test(&state, gov, 2, vec![alice, bob]);
        let contract_addr =
            install_test_contract_account(&state, alice, governance_test_contract_code());

        create_active_guardian_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            guardian_authority,
            genesis_hash,
            &validator,
            1,
            1,
            target_pubkey_payload(3, contract_addr),
            RestrictionMode::StateChangingBlocked,
            5,
        );
        let contract = load_contract_account_for_test(&state, contract_addr);
        assert_eq!(
            contract.lifecycle_status,
            crate::ContractLifecycleStatus::Suspended
        );
        assert_eq!(contract.lifecycle_restriction_id, Some(1));

        let blocked_call = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Call {
                function: "record_call".to_string(),
                args: vec![9],
                value: 0,
            },
            genesis_hash,
            &validator,
        );
        assert!(!blocked_call.success);
        assert!(blocked_call
            .error
            .as_deref()
            .unwrap_or("")
            .contains("lifecycle suspended"));

        let fresh_blockhash = advance_test_slot(&state, 5);
        let args = vec![10, 11];
        let resumed_call = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Call {
                function: "record_call".to_string(),
                args: args.clone(),
                value: 0,
            },
            fresh_blockhash,
            &validator,
        );
        assert!(
            resumed_call.success,
            "expired restriction should resume on next call: {:?}",
            resumed_call.error
        );

        let contract = load_contract_account_for_test(&state, contract_addr);
        assert_eq!(
            contract.lifecycle_status,
            crate::ContractLifecycleStatus::Active
        );
        assert_eq!(contract.lifecycle_restriction_id, None);
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_args")
                .unwrap()
                .unwrap(),
            args
        );
    }

    #[test]
    fn restriction_governance_contract_termination_is_permanent_and_preserves_state() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 1);
        let validator = Pubkey([42u8; 32]);
        let contract_addr =
            install_test_contract_account(&state, alice, governance_test_contract_code());
        let preserved_balance = Account::licn_to_spores(25);
        let preserved_storage = b"audit_value".to_vec();
        let mut account = state.get_account(&contract_addr).unwrap().unwrap();
        account.spores = preserved_balance;
        account.spendable = preserved_balance;
        state.put_account(&contract_addr, &account).unwrap();
        state
            .put_contract_storage(&contract_addr, b"audit_key", &preserved_storage)
            .unwrap();

        let terminate_data = make_restrict_action_data(
            target_pubkey_payload(3, contract_addr),
            &RestrictionMode::Terminated,
            RestrictionReason::TestnetDrill,
            None,
            None,
            None,
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            terminate_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "termination proposal should be stored: {:?}",
            result.error
        );
        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 1, genesis_hash, &validator);
        assert!(
            result.success,
            "termination approval should be recorded: {:?}",
            result.error
        );
        let early_execute =
            process_governance_control(&processor, &bob_kp, bob, 36, 1, genesis_hash, &validator);
        assert!(!early_execute.success);
        assert!(early_execute
            .error
            .as_deref()
            .unwrap_or("")
            .contains("timelocked"));

        let terminate_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);
        let result = process_governance_control(
            &processor,
            &bob_kp,
            bob,
            36,
            1,
            terminate_blockhash,
            &validator,
        );
        assert!(
            result.success,
            "termination should execute after timelock: {:?}",
            result.error
        );

        let account = state.get_account(&contract_addr).unwrap().unwrap();
        assert!(account.executable);
        assert!(!account.data.is_empty());
        assert_eq!(account.spores, preserved_balance);
        assert_eq!(account.spendable, preserved_balance);
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"audit_key")
                .unwrap()
                .unwrap(),
            preserved_storage
        );
        let contract = load_contract_account_for_test(&state, contract_addr);
        assert_eq!(
            contract.lifecycle_status,
            crate::ContractLifecycleStatus::Terminated
        );
        assert_eq!(contract.lifecycle_restriction_id, Some(1));
        assert_eq!(contract.owner, alice);

        let blocked_call = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Call {
                function: "record_call".to_string(),
                args: vec![12],
                value: 0,
            },
            terminate_blockhash,
            &validator,
        );
        assert!(!blocked_call.success);
        assert!(blocked_call
            .error
            .as_deref()
            .unwrap_or("")
            .contains("lifecycle terminated"));
        assert_contract_record_call_not_mutated(&state, contract_addr);

        let lift_data =
            make_lift_restriction_action_data(1, RestrictionLiftReason::IncidentResolved);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            lift_data,
            terminate_blockhash,
            &validator,
        );
        assert!(
            result.success,
            "terminated lift proposal should be stored until execution gate: {:?}",
            result.error
        );
        let result = process_governance_control(
            &processor,
            &bob_kp,
            bob,
            35,
            2,
            terminate_blockhash,
            &validator,
        );
        assert!(
            result.success,
            "terminated lift approval should be recorded: {:?}",
            result.error
        );
        let lift_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH * 2);
        let result =
            process_governance_control(&processor, &bob_kp, bob, 36, 2, lift_blockhash, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("cannot be lifted"));

        let extend_data = make_extend_restriction_action_data(1, Some(SLOTS_PER_EPOCH * 4), None);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            extend_data,
            lift_blockhash,
            &validator,
        );
        assert!(
            result.success,
            "terminated extension proposal should be stored until execution gate: {:?}",
            result.error
        );
        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 3, lift_blockhash, &validator);
        assert!(
            result.success,
            "terminated extension approval should be recorded: {:?}",
            result.error
        );
        let extend_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH * 3);
        let result = process_governance_control(
            &processor,
            &bob_kp,
            bob,
            36,
            3,
            extend_blockhash,
            &validator,
        );
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("cannot be extended"));

        let account = state.get_account(&contract_addr).unwrap().unwrap();
        assert!(account.executable);
        assert_eq!(account.spendable, preserved_balance);
        let contract = load_contract_account_for_test(&state, contract_addr);
        assert_eq!(
            contract.lifecycle_status,
            crate::ContractLifecycleStatus::Terminated
        );
        assert_eq!(contract.lifecycle_restriction_id, Some(1));
    }

    fn setup_restriction_governance(
        threshold: u8,
        timelock_epochs: u32,
    ) -> (
        TxProcessor,
        StateStore,
        Keypair,
        Pubkey,
        Keypair,
        Pubkey,
        Pubkey,
        Hash,
    ) {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov = Pubkey([0xA7; 32]);
        let fund = Account::licn_to_spores(1_000);

        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_last_slot(0).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    threshold,
                    vec![alice, bob],
                    "community_treasury",
                )
                .with_timelock(timelock_epochs),
            )
            .unwrap();

        (
            processor,
            state,
            alice_kp,
            alice,
            bob_kp,
            bob,
            gov,
            genesis_hash,
        )
    }

    fn make_governance_proposal_ix(
        proposer: Pubkey,
        authority: Pubkey,
        data: Vec<u8>,
    ) -> Instruction {
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![proposer, authority],
            data,
        }
    }

    fn make_governance_proposal_control_data(instruction_type: u8, proposal_id: u64) -> Vec<u8> {
        let mut data = vec![instruction_type];
        data.extend_from_slice(&proposal_id.to_le_bytes());
        data
    }

    fn process_governance_proposal(
        processor: &TxProcessor,
        signer: &Keypair,
        signer_pubkey: Pubkey,
        authority: Pubkey,
        data: Vec<u8>,
        recent_blockhash: Hash,
        validator: &Pubkey,
    ) -> TxResult {
        let ix = make_governance_proposal_ix(signer_pubkey, authority, data);
        let tx = make_signed_tx(signer, ix, recent_blockhash);
        processor.process_transaction(&tx, validator)
    }

    fn process_governance_control(
        processor: &TxProcessor,
        signer: &Keypair,
        signer_pubkey: Pubkey,
        instruction_type: u8,
        proposal_id: u64,
        recent_blockhash: Hash,
        validator: &Pubkey,
    ) -> TxResult {
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![signer_pubkey],
            data: make_governance_proposal_control_data(instruction_type, proposal_id),
        };
        let tx = make_signed_tx(signer, ix, recent_blockhash);
        processor.process_transaction(&tx, validator)
    }

    fn parse_governance_action_with_accounts(
        data: Vec<u8>,
        accounts: Vec<Pubkey>,
    ) -> Result<GovernanceAction, String> {
        let (processor, _state, _alice_kp, _alice, _treasury, _genesis_hash) = setup();
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts,
            data,
        };
        processor
            .parse_governance_action(&ix)
            .map(|(_proposer, _authority, action)| action)
    }

    fn parse_restriction_governance_action(data: Vec<u8>) -> Result<GovernanceAction, String> {
        parse_governance_action_with_accounts(data, vec![Pubkey([0xA0; 32]), Pubkey([0xA1; 32])])
    }

    fn target_account_payload(account: Pubkey) -> Vec<u8> {
        let mut payload = vec![0u8];
        payload.extend_from_slice(&account.0);
        payload
    }

    fn target_account_asset_payload(account: Pubkey, asset: Pubkey) -> Vec<u8> {
        let mut payload = vec![1u8];
        payload.extend_from_slice(&account.0);
        payload.extend_from_slice(&asset.0);
        payload
    }

    fn target_pubkey_payload(target_type: u8, pubkey: Pubkey) -> Vec<u8> {
        let mut payload = vec![target_type];
        payload.extend_from_slice(&pubkey.0);
        payload
    }

    fn target_code_hash_payload(code_hash: Hash) -> Vec<u8> {
        let mut payload = vec![4u8];
        payload.extend_from_slice(&code_hash.0);
        payload
    }

    fn push_limited_string(payload: &mut Vec<u8>, value: &str) {
        let value_bytes = value.as_bytes();
        assert!(u16::try_from(value_bytes.len()).is_ok());
        payload.extend_from_slice(&(value_bytes.len() as u16).to_le_bytes());
        payload.extend_from_slice(value_bytes);
    }

    fn target_bridge_route_payload(chain_id: &str, asset: &str) -> Vec<u8> {
        let mut payload = vec![5u8];
        push_limited_string(&mut payload, chain_id);
        push_limited_string(&mut payload, asset);
        payload
    }

    fn target_protocol_module_payload(module: ProtocolModuleId) -> Vec<u8> {
        vec![6u8, module.as_u8()]
    }

    fn frozen_mode_amount(mode: &RestrictionMode) -> Option<u64> {
        match mode {
            RestrictionMode::FrozenAmount { amount } => Some(*amount),
            _ => None,
        }
    }

    fn make_restrict_action_data(
        target_payload: Vec<u8>,
        mode: &RestrictionMode,
        reason: RestrictionReason,
        evidence_hash: Option<Hash>,
        evidence_uri_hash: Option<Hash>,
        expires_at_slot: Option<u64>,
    ) -> Vec<u8> {
        let mut data = vec![34u8, GOVERNANCE_ACTION_RESTRICT];
        data.extend(target_payload);
        data.push(mode.mode_id());
        if let Some(amount) = frozen_mode_amount(mode) {
            data.extend_from_slice(&amount.to_le_bytes());
        }
        data.push(reason.as_u8());
        let mut flags = 0u8;
        if evidence_hash.is_some() {
            flags |= 0x01;
        }
        if evidence_uri_hash.is_some() {
            flags |= 0x02;
        }
        if expires_at_slot.is_some() {
            flags |= 0x04;
        }
        data.push(flags);
        if let Some(hash) = evidence_hash {
            data.extend_from_slice(&hash.0);
        }
        if let Some(hash) = evidence_uri_hash {
            data.extend_from_slice(&hash.0);
        }
        if let Some(slot) = expires_at_slot {
            data.extend_from_slice(&slot.to_le_bytes());
        }
        data
    }

    fn make_lift_restriction_action_data(
        restriction_id: u64,
        reason: RestrictionLiftReason,
    ) -> Vec<u8> {
        let mut data = vec![34u8, GOVERNANCE_ACTION_LIFT_RESTRICTION];
        data.extend_from_slice(&restriction_id.to_le_bytes());
        data.push(reason.as_u8());
        data
    }

    fn make_extend_restriction_action_data(
        restriction_id: u64,
        new_expires_at_slot: Option<u64>,
        evidence_hash: Option<Hash>,
    ) -> Vec<u8> {
        let mut data = vec![34u8, GOVERNANCE_ACTION_EXTEND_RESTRICTION];
        data.extend_from_slice(&restriction_id.to_le_bytes());
        let mut flags = 0u8;
        if new_expires_at_slot.is_some() {
            flags |= 0x01;
        }
        if evidence_hash.is_some() {
            flags |= 0x02;
        }
        data.push(flags);
        if let Some(slot) = new_expires_at_slot {
            data.extend_from_slice(&slot.to_le_bytes());
        }
        if let Some(hash) = evidence_hash {
            data.extend_from_slice(&hash.0);
        }
        data
    }

    #[allow(clippy::too_many_arguments)]
    fn create_active_test_restriction(
        processor: &TxProcessor,
        state: &StateStore,
        alice_kp: &Keypair,
        alice: Pubkey,
        bob_kp: &Keypair,
        bob: Pubkey,
        gov: Pubkey,
        genesis_hash: Hash,
        validator: &Pubkey,
        expires_at_slot: Option<u64>,
    ) {
        let target = Pubkey([0xC1; 32]);
        let data = make_restrict_action_data(
            target_account_payload(target),
            &RestrictionMode::OutgoingOnly,
            RestrictionReason::TestnetDrill,
            None,
            None,
            expires_at_slot,
        );
        let result = process_governance_proposal(
            processor,
            alice_kp,
            alice,
            gov,
            data,
            genesis_hash,
            validator,
        );
        assert!(
            result.success,
            "Restriction proposal should succeed: {:?}",
            result.error
        );
        assert!(state.get_restriction(1).unwrap().is_none());

        let result =
            process_governance_control(processor, bob_kp, bob, 35, 1, genesis_hash, validator);
        assert!(
            result.success,
            "Restriction approval should execute: {:?}",
            result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(proposal.executed);
        assert_eq!(
            state.get_restriction(1).unwrap().unwrap().status,
            RestrictionStatus::Active
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn create_active_bridge_split_restriction(
        processor: &TxProcessor,
        state: &StateStore,
        alice_kp: &Keypair,
        alice: Pubkey,
        bob_kp: &Keypair,
        bob: Pubkey,
        bridge_authority: Pubkey,
        genesis_hash: Hash,
        validator: &Pubkey,
    ) -> Hash {
        let data = make_restrict_action_data(
            target_bridge_route_payload("neo-x-testnet", "USDT"),
            &RestrictionMode::RoutePaused,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(SLOTS_PER_EPOCH * 4),
        );
        let result = process_governance_proposal(
            processor,
            alice_kp,
            alice,
            bridge_authority,
            data,
            genesis_hash,
            validator,
        );
        assert!(
            result.success,
            "Bridge split restriction proposal should succeed: {:?}",
            result.error
        );

        let result =
            process_governance_control(processor, bob_kp, bob, 35, 1, genesis_hash, validator);
        assert!(
            result.success,
            "Bridge split restriction approval should succeed: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(!proposal.executed);

        let fresh_blockhash = advance_test_slot(state, SLOTS_PER_EPOCH);
        let result =
            process_governance_control(processor, bob_kp, bob, 36, 1, fresh_blockhash, validator);
        assert!(
            result.success,
            "Bridge split restriction execution should succeed: {:?}",
            result.error
        );

        let record = state.get_restriction(1).unwrap().unwrap();
        assert_eq!(record.status, RestrictionStatus::Active);
        assert_eq!(record.approval_authority, Some(bridge_authority));
        fresh_blockhash
    }

    #[allow(clippy::too_many_arguments)]
    fn create_active_guardian_test_restriction(
        processor: &TxProcessor,
        state: &StateStore,
        alice_kp: &Keypair,
        alice: Pubkey,
        bob_kp: &Keypair,
        bob: Pubkey,
        guardian_authority: Pubkey,
        genesis_hash: Hash,
        validator: &Pubkey,
        proposal_id: u64,
        restriction_id: u64,
        target_payload: Vec<u8>,
        mode: RestrictionMode,
        expires_at_slot: u64,
    ) -> RestrictionRecord {
        let data = make_restrict_action_data(
            target_payload,
            &mode,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(expires_at_slot),
        );
        let result = process_governance_proposal(
            processor,
            alice_kp,
            alice,
            guardian_authority,
            data,
            genesis_hash,
            validator,
        );
        assert!(
            result.success,
            "Guardian restriction proposal should succeed: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(proposal_id).unwrap().unwrap();
        assert_eq!(
            proposal.authority,
            state.get_governance_authority().unwrap().unwrap()
        );
        assert_eq!(proposal.approval_authority, Some(guardian_authority));
        assert!(!proposal.executed);

        let result = process_governance_control(
            processor,
            bob_kp,
            bob,
            35,
            proposal_id,
            genesis_hash,
            validator,
        );
        assert!(
            result.success,
            "Guardian restriction approval should execute: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(proposal_id).unwrap().unwrap();
        assert!(proposal.executed);

        let record = state.get_restriction(restriction_id).unwrap().unwrap();
        assert_eq!(record.id, restriction_id);
        assert_eq!(record.mode, mode);
        assert_eq!(record.status, RestrictionStatus::Active);
        assert_eq!(record.approval_authority, Some(guardian_authority));
        assert_eq!(record.expires_at_slot, Some(expires_at_slot));
        record
    }

    fn find_system_event<'a>(
        events: &'a [ContractEvent],
        event_name: &str,
        proposal_id: u64,
    ) -> &'a ContractEvent {
        let proposal_id = proposal_id.to_string();
        events
            .iter()
            .find(|event| {
                event.program == SYSTEM_PROGRAM_ID
                    && event.name == event_name
                    && event.data.get("proposal_id").map(String::as_str)
                        == Some(proposal_id.as_str())
            })
            .unwrap_or_else(|| panic!("missing {} for proposal {}", event_name, proposal_id))
    }

    fn assert_restrict_action(
        action: GovernanceAction,
        expected_target: RestrictionTarget,
        expected_mode: RestrictionMode,
        expected_reason: RestrictionReason,
        expected_evidence_hash: Option<Hash>,
        expected_evidence_uri_hash: Option<Hash>,
        expected_expires_at_slot: Option<u64>,
    ) {
        match action {
            GovernanceAction::Restrict {
                target,
                mode,
                reason,
                evidence_hash,
                evidence_uri_hash,
                expires_at_slot,
            } => {
                assert_eq!(target, expected_target);
                assert_eq!(mode, expected_mode);
                assert_eq!(reason, expected_reason);
                assert_eq!(evidence_hash, expected_evidence_hash);
                assert_eq!(evidence_uri_hash, expected_evidence_uri_hash);
                assert_eq!(expires_at_slot, expected_expires_at_slot);
            }
            other => panic!("expected Restrict action, got {:?}", other),
        }
    }

    fn assert_parse_error(data: Vec<u8>, expected: &str) {
        let err = parse_restriction_governance_action(data).expect_err("parse should fail");
        assert!(
            err.contains(expected),
            "expected error containing {:?}, got {:?}",
            expected,
            err
        );
    }

    #[test]
    fn test_restriction_governance_action_subtypes_are_append_only() {
        assert_eq!(GOVERNANCE_ACTION_TREASURY_TRANSFER, 0);
        assert_eq!(GOVERNANCE_ACTION_PARAM_CHANGE, 1);
        assert_eq!(GOVERNANCE_ACTION_CONTRACT_UPGRADE, 2);
        assert_eq!(GOVERNANCE_ACTION_SET_UPGRADE_TIMELOCK, 3);
        assert_eq!(GOVERNANCE_ACTION_EXECUTE_UPGRADE, 4);
        assert_eq!(GOVERNANCE_ACTION_VETO_UPGRADE, 5);
        assert_eq!(GOVERNANCE_ACTION_CONTRACT_CLOSE, 6);
        assert_eq!(GOVERNANCE_ACTION_REGISTER_SYMBOL, 7);
        assert_eq!(GOVERNANCE_ACTION_SET_CONTRACT_ABI, 8);
        assert_eq!(GOVERNANCE_ACTION_CONTRACT_CALL, 9);
        assert_eq!(GOVERNANCE_ACTION_RESTRICT, 10);
        assert_eq!(GOVERNANCE_ACTION_LIFT_RESTRICTION, 11);
        assert_eq!(GOVERNANCE_ACTION_EXTEND_RESTRICTION, 12);

        let proposer = Pubkey([0x01; 32]);
        let authority = Pubkey([0x02; 32]);
        let recipient = Pubkey([0x03; 32]);
        let mut treasury_data = vec![34u8, GOVERNANCE_ACTION_TREASURY_TRANSFER];
        treasury_data.extend_from_slice(&500u64.to_le_bytes());
        match parse_governance_action_with_accounts(
            treasury_data,
            vec![proposer, authority, recipient],
        )
        .expect("legacy treasury subtype parses")
        {
            GovernanceAction::TreasuryTransfer {
                recipient: parsed_recipient,
                amount,
            } => {
                assert_eq!(parsed_recipient, recipient);
                assert_eq!(amount, 500);
            }
            other => panic!("expected TreasuryTransfer, got {:?}", other),
        }

        let contract = Pubkey([0x04; 32]);
        match parse_governance_action_with_accounts(
            make_governance_contract_call_data("record_call", &[1, 2, 3], 7),
            vec![proposer, authority, contract],
        )
        .expect("legacy contract call subtype parses")
        {
            GovernanceAction::ContractCall {
                contract: parsed_contract,
                function,
                args,
                value,
            } => {
                assert_eq!(parsed_contract, contract);
                assert_eq!(function, "record_call");
                assert_eq!(args, vec![1, 2, 3]);
                assert_eq!(value, 7);
            }
            other => panic!("expected ContractCall, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_restrict_governance_action_target_forms() {
        let cases = vec![
            (
                target_account_payload(Pubkey([0x10; 32])),
                RestrictionTarget::Account(Pubkey([0x10; 32])),
                RestrictionMode::OutgoingOnly,
                RestrictionReason::TestnetDrill,
                None,
                None,
                Some(101),
            ),
            (
                target_account_asset_payload(Pubkey([0x11; 32]), Pubkey([0x12; 32])),
                RestrictionTarget::AccountAsset {
                    account: Pubkey([0x11; 32]),
                    asset: Pubkey([0x12; 32]),
                },
                RestrictionMode::FrozenAmount { amount: 55 },
                RestrictionReason::StolenFunds,
                Some(Hash([0x21; 32])),
                None,
                Some(102),
            ),
            (
                target_pubkey_payload(2, Pubkey([0x13; 32])),
                RestrictionTarget::Asset(Pubkey([0x13; 32])),
                RestrictionMode::AssetPaused,
                RestrictionReason::CustodyIncident,
                None,
                Some(Hash([0x22; 32])),
                Some(103),
            ),
            (
                target_pubkey_payload(3, Pubkey([0x14; 32])),
                RestrictionTarget::Contract(Pubkey([0x14; 32])),
                RestrictionMode::Quarantined,
                RestrictionReason::ScamContract,
                Some(Hash([0x23; 32])),
                None,
                Some(104),
            ),
            (
                target_code_hash_payload(Hash([0x15; 32])),
                RestrictionTarget::CodeHash(Hash([0x15; 32])),
                RestrictionMode::DeployBlocked,
                RestrictionReason::MaliciousCodeHash,
                Some(Hash([0x24; 32])),
                None,
                Some(105),
            ),
            (
                target_bridge_route_payload("neo-x-testnet", "WETH"),
                RestrictionTarget::BridgeRoute {
                    chain_id: "neo-x-testnet".to_string(),
                    asset: "WETH".to_string(),
                },
                RestrictionMode::RoutePaused,
                RestrictionReason::BridgeCompromise,
                Some(Hash([0x25; 32])),
                None,
                Some(106),
            ),
            (
                target_protocol_module_payload(ProtocolModuleId::Mempool),
                RestrictionTarget::ProtocolModule(ProtocolModuleId::Mempool),
                RestrictionMode::ProtocolPaused,
                RestrictionReason::ProtocolBug,
                Some(Hash([0x26; 32])),
                None,
                Some(107),
            ),
        ];

        for (
            target_payload,
            target,
            mode,
            reason,
            evidence_hash,
            evidence_uri_hash,
            expires_at_slot,
        ) in cases
        {
            let data = make_restrict_action_data(
                target_payload,
                &mode,
                reason,
                evidence_hash,
                evidence_uri_hash,
                expires_at_slot,
            );
            let action = parse_restriction_governance_action(data).expect("restrict parses");
            assert_restrict_action(
                action,
                target,
                mode,
                reason,
                evidence_hash,
                evidence_uri_hash,
                expires_at_slot,
            );
        }
    }

    #[test]
    fn test_parse_restrict_governance_action_modes() {
        let cases = vec![
            (
                target_account_payload(Pubkey([0x30; 32])),
                RestrictionMode::OutgoingOnly,
            ),
            (
                target_account_payload(Pubkey([0x31; 32])),
                RestrictionMode::IncomingOnly,
            ),
            (
                target_account_payload(Pubkey([0x32; 32])),
                RestrictionMode::Bidirectional,
            ),
            (
                target_account_asset_payload(Pubkey([0x33; 32]), Pubkey([0x34; 32])),
                RestrictionMode::FrozenAmount { amount: 77 },
            ),
            (
                target_pubkey_payload(2, Pubkey([0x35; 32])),
                RestrictionMode::AssetPaused,
            ),
            (
                target_pubkey_payload(3, Pubkey([0x36; 32])),
                RestrictionMode::ExecuteBlocked,
            ),
            (
                target_pubkey_payload(3, Pubkey([0x37; 32])),
                RestrictionMode::StateChangingBlocked,
            ),
            (
                target_pubkey_payload(3, Pubkey([0x38; 32])),
                RestrictionMode::Quarantined,
            ),
            (
                target_code_hash_payload(Hash([0x39; 32])),
                RestrictionMode::DeployBlocked,
            ),
            (
                target_bridge_route_payload("eth-mainnet", "USDT"),
                RestrictionMode::RoutePaused,
            ),
            (
                target_protocol_module_payload(ProtocolModuleId::Bridge),
                RestrictionMode::ProtocolPaused,
            ),
            (
                target_pubkey_payload(3, Pubkey([0x3A; 32])),
                RestrictionMode::Terminated,
            ),
        ];

        for (target_payload, mode) in cases {
            let data = make_restrict_action_data(
                target_payload,
                &mode,
                RestrictionReason::TestnetDrill,
                None,
                None,
                None,
            );
            match parse_restriction_governance_action(data).expect("mode parses") {
                GovernanceAction::Restrict {
                    mode: parsed_mode, ..
                } => assert_eq!(parsed_mode, mode),
                other => panic!("expected Restrict action, got {:?}", other),
            }
        }
    }

    #[test]
    fn test_parse_lift_and_extend_restriction_governance_actions() {
        match parse_restriction_governance_action(make_lift_restriction_action_data(
            42,
            RestrictionLiftReason::FalsePositive,
        ))
        .expect("lift parses")
        {
            GovernanceAction::LiftRestriction {
                restriction_id,
                reason,
            } => {
                assert_eq!(restriction_id, 42);
                assert_eq!(reason, RestrictionLiftReason::FalsePositive);
            }
            other => panic!("expected LiftRestriction, got {:?}", other),
        }

        match parse_restriction_governance_action(make_extend_restriction_action_data(
            43,
            Some(1_000),
            Some(Hash([0x44; 32])),
        ))
        .expect("extend parses")
        {
            GovernanceAction::ExtendRestriction {
                restriction_id,
                new_expires_at_slot,
                evidence_hash,
            } => {
                assert_eq!(restriction_id, 43);
                assert_eq!(new_expires_at_slot, Some(1_000));
                assert_eq!(evidence_hash, Some(Hash([0x44; 32])));
            }
            other => panic!("expected ExtendRestriction, got {:?}", other),
        }

        match parse_restriction_governance_action(make_extend_restriction_action_data(
            44, None, None,
        ))
        .expect("no-op shaped extend parses for execution-time validation")
        {
            GovernanceAction::ExtendRestriction {
                restriction_id,
                new_expires_at_slot,
                evidence_hash,
            } => {
                assert_eq!(restriction_id, 44);
                assert_eq!(new_expires_at_slot, None);
                assert_eq!(evidence_hash, None);
            }
            other => panic!("expected ExtendRestriction, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_restriction_governance_action_rejects_malformed_payloads() {
        assert_parse_error(vec![34u8, 13], "Unknown governance action type 13");

        let mut unknown_target = vec![34u8, GOVERNANCE_ACTION_RESTRICT, 99];
        unknown_target.push(RestrictionMode::OutgoingOnly.mode_id());
        unknown_target.push(RestrictionReason::TestnetDrill.as_u8());
        unknown_target.push(0);
        assert_parse_error(unknown_target, "unknown restriction target type 99");

        let mut unknown_mode = vec![34u8, GOVERNANCE_ACTION_RESTRICT];
        unknown_mode.extend(target_account_payload(Pubkey([0x50; 32])));
        unknown_mode.push(99);
        unknown_mode.push(RestrictionReason::TestnetDrill.as_u8());
        unknown_mode.push(0);
        assert_parse_error(unknown_mode, "unknown restriction mode 99");

        let mut missing_frozen_amount = vec![34u8, GOVERNANCE_ACTION_RESTRICT];
        missing_frozen_amount.extend(target_account_asset_payload(
            Pubkey([0x51; 32]),
            Pubkey([0x52; 32]),
        ));
        missing_frozen_amount.push(RestrictionMode::FrozenAmount { amount: 1 }.mode_id());
        assert_parse_error(missing_frozen_amount, "payload truncated at frozen_amount");

        assert_parse_error(
            make_restrict_action_data(
                target_account_asset_payload(Pubkey([0x53; 32]), Pubkey([0x54; 32])),
                &RestrictionMode::FrozenAmount { amount: 0 },
                RestrictionReason::TestnetDrill,
                None,
                None,
                None,
            ),
            "FrozenAmount restriction amount must be > 0",
        );

        assert_parse_error(
            make_restrict_action_data(
                target_account_payload(Pubkey([0x55; 32])),
                &RestrictionMode::OutgoingOnly,
                RestrictionReason::StolenFunds,
                None,
                None,
                None,
            ),
            "requires evidence_hash or evidence_uri_hash",
        );

        let mut unexpected_restrict_flags = vec![34u8, GOVERNANCE_ACTION_RESTRICT];
        unexpected_restrict_flags.extend(target_account_payload(Pubkey([0x56; 32])));
        unexpected_restrict_flags.push(RestrictionMode::OutgoingOnly.mode_id());
        unexpected_restrict_flags.push(RestrictionReason::TestnetDrill.as_u8());
        unexpected_restrict_flags.push(0x08);
        assert_parse_error(unexpected_restrict_flags, "unexpected flags 0x08");

        let mut trailing_restrict = make_restrict_action_data(
            target_account_payload(Pubkey([0x57; 32])),
            &RestrictionMode::OutgoingOnly,
            RestrictionReason::TestnetDrill,
            None,
            None,
            None,
        );
        trailing_restrict.push(0xAA);
        assert_parse_error(trailing_restrict, "trailing bytes");

        let mut empty_route_chain = vec![34u8, GOVERNANCE_ACTION_RESTRICT, 5];
        empty_route_chain.extend_from_slice(&0u16.to_le_bytes());
        assert_parse_error(empty_route_chain, "chain_id cannot be empty");

        let mut invalid_route_utf8 = vec![34u8, GOVERNANCE_ACTION_RESTRICT, 5];
        invalid_route_utf8.extend_from_slice(&1u16.to_le_bytes());
        invalid_route_utf8.push(0xFF);
        assert_parse_error(invalid_route_utf8, "chain_id must be valid UTF-8");

        let mut too_long_route = vec![34u8, GOVERNANCE_ACTION_RESTRICT, 5];
        too_long_route.extend_from_slice(&257u16.to_le_bytes());
        too_long_route.extend(std::iter::repeat_n(b'a', 257));
        assert_parse_error(too_long_route, "chain_id length 257 exceeds 256");

        let mut unknown_module = vec![34u8, GOVERNANCE_ACTION_RESTRICT];
        unknown_module.extend([6u8, 99u8]);
        assert_parse_error(unknown_module, "unknown protocol module id 99");

        let mut bad_lift_id = vec![34u8, GOVERNANCE_ACTION_LIFT_RESTRICTION];
        bad_lift_id.extend_from_slice(&0u64.to_le_bytes());
        bad_lift_id.push(RestrictionLiftReason::IncidentResolved.as_u8());
        assert_parse_error(bad_lift_id, "restriction_id must be greater than zero");

        let mut bad_lift_reason = vec![34u8, GOVERNANCE_ACTION_LIFT_RESTRICTION];
        bad_lift_reason.extend_from_slice(&1u64.to_le_bytes());
        bad_lift_reason.push(99);
        assert_parse_error(bad_lift_reason, "unknown restriction lift reason 99");

        let mut trailing_lift =
            make_lift_restriction_action_data(1, RestrictionLiftReason::IncidentResolved);
        trailing_lift.push(0xAA);
        assert_parse_error(trailing_lift, "trailing bytes");

        let mut bad_extend_id = vec![34u8, GOVERNANCE_ACTION_EXTEND_RESTRICTION];
        bad_extend_id.extend_from_slice(&0u64.to_le_bytes());
        bad_extend_id.push(0);
        assert_parse_error(bad_extend_id, "restriction_id must be greater than zero");

        let mut bad_extend_flags = vec![34u8, GOVERNANCE_ACTION_EXTEND_RESTRICTION];
        bad_extend_flags.extend_from_slice(&1u64.to_le_bytes());
        bad_extend_flags.push(0x04);
        assert_parse_error(bad_extend_flags, "unexpected flags 0x04");

        let mut truncated_extend = vec![34u8, GOVERNANCE_ACTION_EXTEND_RESTRICTION];
        truncated_extend.extend_from_slice(&1u64.to_le_bytes());
        truncated_extend.push(0x02);
        truncated_extend.extend_from_slice(&[0xAB; 31]);
        assert_parse_error(truncated_extend, "payload truncated at evidence_hash");
    }

    #[test]
    fn test_restriction_governance_create_executes_through_proposal_lifecycle() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 1);
        let validator = Pubkey([42u8; 32]);
        let target = Pubkey([0xA1; 32]);
        let expires_at_slot = SLOTS_PER_EPOCH * 2;
        let data = make_restrict_action_data(
            target_account_payload(target),
            &RestrictionMode::OutgoingOnly,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(expires_at_slot),
        );

        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Restriction proposal should succeed: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.approvals, vec![alice]);
        assert_eq!(proposal.execute_after_epoch, 1);
        assert!(!proposal.executed);
        assert!(state.get_restriction(1).unwrap().is_none());

        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 1, genesis_hash, &validator);
        assert!(
            result.success,
            "Restriction approval should succeed: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.approvals, vec![alice, bob]);
        assert!(!proposal.executed);
        assert!(state.get_restriction(1).unwrap().is_none());

        let result = process_governance_control(
            &processor,
            &alice_kp,
            alice,
            36,
            1,
            genesis_hash,
            &validator,
        );
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("timelocked"));

        let fresh_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);
        let result = process_governance_control(
            &processor,
            &bob_kp,
            bob,
            36,
            1,
            fresh_blockhash,
            &validator,
        );
        assert!(
            result.success,
            "Restriction execution should succeed: {:?}",
            result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(proposal.executed);
        let record = state.get_restriction(1).unwrap().unwrap();
        assert_eq!(record.id, 1);
        assert_eq!(record.target, RestrictionTarget::Account(target));
        assert_eq!(record.mode, RestrictionMode::OutgoingOnly);
        assert_eq!(record.status, RestrictionStatus::Active);
        assert_eq!(record.reason, RestrictionReason::TestnetDrill);
        assert_eq!(record.proposer, alice);
        assert_eq!(record.authority, gov);
        assert_eq!(record.approval_authority, None);
        assert_eq!(record.created_slot, SLOTS_PER_EPOCH);
        assert_eq!(record.created_epoch, 1);
        assert_eq!(record.expires_at_slot, Some(expires_at_slot));
        assert_eq!(record.supersedes, None);
    }

    #[test]
    fn test_restriction_governance_lift_preserves_record_id() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 0);
        let validator = Pubkey([42u8; 32]);
        create_active_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            gov,
            genesis_hash,
            &validator,
            Some(100),
        );

        let data = make_lift_restriction_action_data(1, RestrictionLiftReason::FalsePositive);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Lift proposal should succeed: {:?}",
            result.error
        );
        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 2, genesis_hash, &validator);
        assert!(
            result.success,
            "Lift approval should execute: {:?}",
            result.error
        );

        let proposal = state.get_governance_proposal(2).unwrap().unwrap();
        assert!(proposal.executed);
        let record = state.get_restriction(1).unwrap().unwrap();
        assert_eq!(record.id, 1);
        assert_eq!(record.status, RestrictionStatus::Lifted);
        assert_eq!(record.lifted_by, Some(gov));
        assert_eq!(record.lifted_slot, Some(0));
        assert_eq!(
            record.lift_reason,
            Some(RestrictionLiftReason::FalsePositive)
        );
        assert_eq!(state.get_restriction(2).unwrap(), None);
    }

    #[test]
    fn test_restriction_governance_extend_supersedes_and_creates_successor() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 0);
        let validator = Pubkey([42u8; 32]);
        create_active_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            gov,
            genesis_hash,
            &validator,
            Some(100),
        );

        let replacement_evidence = Hash([0xE2; 32]);
        let data = make_extend_restriction_action_data(1, Some(200), Some(replacement_evidence));
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Extend proposal should succeed: {:?}",
            result.error
        );
        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 2, genesis_hash, &validator);
        assert!(
            result.success,
            "Extend approval should execute: {:?}",
            result.error
        );

        let old_record = state.get_restriction(1).unwrap().unwrap();
        assert_eq!(old_record.status, RestrictionStatus::Superseded);
        assert_eq!(old_record.expires_at_slot, Some(100));
        assert_eq!(old_record.lifted_by, None);

        let successor = state.get_restriction(2).unwrap().unwrap();
        assert_eq!(successor.id, 2);
        assert_eq!(successor.status, RestrictionStatus::Active);
        assert_eq!(successor.target, old_record.target);
        assert_eq!(successor.mode, old_record.mode);
        assert_eq!(successor.reason, old_record.reason);
        assert_eq!(successor.evidence_hash, Some(replacement_evidence));
        assert_eq!(successor.proposer, alice);
        assert_eq!(successor.authority, gov);
        assert_eq!(successor.created_slot, 0);
        assert_eq!(successor.created_epoch, 0);
        assert_eq!(successor.expires_at_slot, Some(200));
        assert_eq!(successor.supersedes, Some(1));
    }

    #[test]
    fn test_restriction_governance_cancel_leaves_state_unmutated() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 0);
        let validator = Pubkey([42u8; 32]);
        let data = make_restrict_action_data(
            target_account_payload(Pubkey([0xB1; 32])),
            &RestrictionMode::IncomingOnly,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(100),
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Restriction proposal should succeed: {:?}",
            result.error
        );
        assert!(state.get_restriction(1).unwrap().is_none());

        let result =
            process_governance_control(&processor, &bob_kp, bob, 37, 1, genesis_hash, &validator);
        assert!(
            result.success,
            "Cancellation should succeed: {:?}",
            result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(proposal.cancelled);
        assert!(!proposal.executed);
        assert!(state.get_restriction(1).unwrap().is_none());
    }

    #[test]
    fn test_restriction_governance_rejects_invalid_transitions_atomically() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 0);
        let validator = Pubkey([42u8; 32]);
        create_active_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            gov,
            genesis_hash,
            &validator,
            Some(100),
        );

        let data = make_extend_restriction_action_data(1, Some(100), None);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Invalid extend proposal should still be stored: {:?}",
            result.error
        );
        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 2, genesis_hash, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("must be greater than current expiry"));

        let proposal = state.get_governance_proposal(2).unwrap().unwrap();
        assert_eq!(proposal.approvals, vec![alice]);
        assert!(!proposal.executed);
        assert_eq!(
            state.get_restriction(1).unwrap().unwrap().status,
            RestrictionStatus::Active
        );
        assert!(state.get_restriction(2).unwrap().is_none());

        let fresh_blockhash = advance_test_slot(&state, 100);
        let data = make_lift_restriction_action_data(1, RestrictionLiftReason::IncidentResolved);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            data,
            fresh_blockhash,
            &validator,
        );
        assert!(
            result.success,
            "Expired lift proposal should still be stored: {:?}",
            result.error
        );
        let result = process_governance_control(
            &processor,
            &bob_kp,
            bob,
            35,
            3,
            fresh_blockhash,
            &validator,
        );
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("not active"));

        let proposal = state.get_governance_proposal(3).unwrap().unwrap();
        assert_eq!(proposal.approvals, vec![alice]);
        assert!(!proposal.executed);
        let record = state.get_restriction(1).unwrap().unwrap();
        assert_eq!(record.status, RestrictionStatus::Active);
        assert_eq!(
            state
                .get_effective_restriction_record(1, 100)
                .unwrap()
                .unwrap()
                .effective_status,
            RestrictionStatus::Expired
        );
        assert!(state.get_restriction(2).unwrap().is_none());
    }

    #[test]
    fn test_restriction_governance_extend_rejects_indefinite_records() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 0);
        let validator = Pubkey([42u8; 32]);
        create_active_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            gov,
            genesis_hash,
            &validator,
            None,
        );

        let data = make_extend_restriction_action_data(1, Some(200), None);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Indefinite extend proposal should still be stored: {:?}",
            result.error
        );
        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 2, genesis_hash, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("has no expiry to extend"));

        let proposal = state.get_governance_proposal(2).unwrap().unwrap();
        assert_eq!(proposal.approvals, vec![alice]);
        assert!(!proposal.executed);
        assert_eq!(
            state.get_restriction(1).unwrap().unwrap().status,
            RestrictionStatus::Active
        );
        assert!(state.get_restriction(2).unwrap().is_none());
    }

    #[test]
    fn test_restriction_governance_lifecycle_events_use_stored_record_metadata() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 0);
        let validator = Pubkey([42u8; 32]);
        create_active_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            gov,
            genesis_hash,
            &validator,
            Some(100),
        );

        let events = state
            .get_events_by_program(&SYSTEM_PROGRAM_ID, 20, None)
            .unwrap();
        let created = find_system_event(&events, "RestrictionCreated", 1);
        assert_eq!(
            created.data.get("action").map(String::as_str),
            Some("restrict")
        );
        assert_eq!(created.data.get("actor"), Some(&bob.to_base58()));
        assert_eq!(created.data.get("authority"), Some(&gov.to_base58()));
        assert_eq!(created.data.get("proposer"), Some(&alice.to_base58()));
        assert_eq!(created.data.get("approvals").map(String::as_str), Some("2"));
        assert_eq!(created.data.get("threshold").map(String::as_str), Some("2"));
        assert_eq!(
            created.data.get("executed").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            created.data.get("restriction_id").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            created.data.get("restriction_status").map(String::as_str),
            Some("active")
        );
        assert_eq!(
            created
                .data
                .get("restriction_target_type")
                .map(String::as_str),
            Some("account")
        );
        assert_eq!(
            created.data.get("restriction_mode").map(String::as_str),
            Some("outgoing_only")
        );
        assert_eq!(
            created.data.get("restriction_reason").map(String::as_str),
            Some("testnet_drill")
        );
        assert_eq!(
            created.data.get("created_slot").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            created.data.get("created_epoch").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            created.data.get("expires_at_slot").map(String::as_str),
            Some("100")
        );

        let extend_data = make_extend_restriction_action_data(1, Some(200), None);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            extend_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Extend proposal should succeed: {:?}",
            result.error
        );
        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 2, genesis_hash, &validator);
        assert!(
            result.success,
            "Extend approval should execute: {:?}",
            result.error
        );

        let events = state
            .get_events_by_program(&SYSTEM_PROGRAM_ID, 40, None)
            .unwrap();
        let extended = find_system_event(&events, "RestrictionExtended", 2);
        assert_eq!(
            extended.data.get("restriction_id").map(String::as_str),
            Some("2")
        );
        assert_eq!(
            extended.data.get("restriction_status").map(String::as_str),
            Some("active")
        );
        assert_eq!(
            extended.data.get("supersedes").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            extended.data.get("expires_at_slot").map(String::as_str),
            Some("200")
        );
        assert_eq!(
            extended.data.get("restriction_mode").map(String::as_str),
            Some("outgoing_only")
        );

        let lift_data =
            make_lift_restriction_action_data(2, RestrictionLiftReason::IncidentResolved);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            lift_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Lift proposal should succeed: {:?}",
            result.error
        );
        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 3, genesis_hash, &validator);
        assert!(
            result.success,
            "Lift approval should execute: {:?}",
            result.error
        );

        let events = state
            .get_events_by_program(&SYSTEM_PROGRAM_ID, 60, None)
            .unwrap();
        let lifted = find_system_event(&events, "RestrictionLifted", 3);
        assert_eq!(
            lifted.data.get("restriction_id").map(String::as_str),
            Some("2")
        );
        assert_eq!(
            lifted.data.get("restriction_status").map(String::as_str),
            Some("lifted")
        );
        assert_eq!(lifted.data.get("lifted_by"), Some(&gov.to_base58()));
        assert_eq!(
            lifted.data.get("lifted_slot").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            lifted.data.get("lift_reason").map(String::as_str),
            Some("incident_resolved")
        );
        assert_eq!(
            lifted.data.get("restriction_reason").map(String::as_str),
            Some("testnet_drill")
        );
    }

    #[test]
    fn test_restriction_governance_failed_execution_emits_no_lifecycle_event() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 0);
        let validator = Pubkey([42u8; 32]);
        create_active_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            gov,
            genesis_hash,
            &validator,
            Some(100),
        );

        let invalid_extend_data = make_extend_restriction_action_data(1, Some(100), None);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            invalid_extend_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Invalid extend proposal should be stored: {:?}",
            result.error
        );
        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 2, genesis_hash, &validator);
        assert!(!result.success);

        let events = state
            .get_events_by_program(&SYSTEM_PROGRAM_ID, 40, None)
            .unwrap();
        assert!(!events.iter().any(|event| {
            event.name == "RestrictionExtended"
                && event.data.get("proposal_id").map(String::as_str) == Some("2")
        }));
    }

    #[test]
    fn test_restriction_governance_main_authority_remains_higher_authority_for_split_targets() {
        let (processor, state, alice_kp, alice, _bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 5);
        let validator = Pubkey([42u8; 32]);
        configure_bridge_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);
        configure_oracle_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);

        let bridge_data = make_restrict_action_data(
            target_bridge_route_payload("neo-x-testnet", "USDT"),
            &RestrictionMode::RoutePaused,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(SLOTS_PER_EPOCH * 4),
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            bridge_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Main governance bridge restriction proposal should succeed: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, None);
        assert_eq!(proposal.execute_after_epoch, 5);

        let oracle_data = make_restrict_action_data(
            target_protocol_module_payload(ProtocolModuleId::Oracle),
            &RestrictionMode::ProtocolPaused,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(SLOTS_PER_EPOCH * 4),
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            oracle_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Main governance oracle restriction proposal should succeed: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(2).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, None);
        assert_eq!(proposal.execute_after_epoch, 5);
    }

    #[test]
    fn test_restriction_governance_split_roles_route_scoped_creates() {
        let (processor, state, alice_kp, alice, _bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 5);
        let validator = Pubkey([42u8; 32]);
        let bridge_authority =
            configure_bridge_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);
        let oracle_authority =
            configure_oracle_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);

        let route_data = make_restrict_action_data(
            target_bridge_route_payload("neo-x-testnet", "USDT"),
            &RestrictionMode::RoutePaused,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(SLOTS_PER_EPOCH * 4),
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            bridge_authority,
            route_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Bridge route restriction should route to bridge split authority: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(bridge_authority));
        assert_eq!(proposal.execute_after_epoch, 1);

        let bridge_module_data = make_restrict_action_data(
            target_protocol_module_payload(ProtocolModuleId::Bridge),
            &RestrictionMode::ProtocolPaused,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(SLOTS_PER_EPOCH * 4),
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            bridge_authority,
            bridge_module_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Bridge protocol restriction should route to bridge split authority: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(2).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(bridge_authority));
        assert_eq!(proposal.execute_after_epoch, 1);

        let oracle_data = make_restrict_action_data(
            target_protocol_module_payload(ProtocolModuleId::Oracle),
            &RestrictionMode::ProtocolPaused,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(SLOTS_PER_EPOCH * 4),
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            oracle_authority,
            oracle_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Oracle protocol restriction should route to oracle split authority: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(3).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(oracle_authority));
        assert_eq!(proposal.execute_after_epoch, 1);
    }

    #[test]
    fn test_restriction_governance_rejects_wrong_split_routing() {
        let (processor, state, alice_kp, alice, _bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 5);
        let validator = Pubkey([42u8; 32]);
        let bridge_authority =
            configure_bridge_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);
        let oracle_authority =
            configure_oracle_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);

        let oracle_data = make_restrict_action_data(
            target_protocol_module_payload(ProtocolModuleId::Oracle),
            &RestrictionMode::ProtocolPaused,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(SLOTS_PER_EPOCH * 4),
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            bridge_authority,
            oracle_data,
            genesis_hash,
            &validator,
        );
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Governance action authority account mismatch"));

        let route_data = make_restrict_action_data(
            target_bridge_route_payload("neo-x-testnet", "USDT"),
            &RestrictionMode::RoutePaused,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(SLOTS_PER_EPOCH * 4),
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            oracle_authority,
            route_data,
            genesis_hash,
            &validator,
        );
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Governance action authority account mismatch"));
    }

    #[test]
    fn test_restriction_governance_guardian_can_create_and_lift_temporary_account_restriction() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 5);
        let validator = Pubkey([42u8; 32]);
        let guardian_authority =
            configure_incident_guardian_for_test(&state, gov, 2, vec![alice, bob]);

        let record = create_active_guardian_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            guardian_authority,
            genesis_hash,
            &validator,
            1,
            1,
            target_account_payload(Pubkey([0xD1; 32])),
            RestrictionMode::OutgoingOnly,
            100,
        );
        assert_eq!(record.authority, gov);
        assert_eq!(record.created_slot, 0);

        let lift_data =
            make_lift_restriction_action_data(1, RestrictionLiftReason::IncidentResolved);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            guardian_authority,
            lift_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Guardian should be able to propose lift for its restriction: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(2).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(guardian_authority));
        assert_eq!(proposal.execute_after_epoch, 0);

        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 2, genesis_hash, &validator);
        assert!(
            result.success,
            "Guardian lift approval should execute: {:?}",
            result.error
        );
        let lifted = state.get_restriction(1).unwrap().unwrap();
        assert_eq!(lifted.status, RestrictionStatus::Lifted);
        assert_eq!(
            lifted.lift_reason,
            Some(RestrictionLiftReason::IncidentResolved)
        );
        assert_eq!(lifted.approval_authority, Some(guardian_authority));
    }

    #[test]
    fn test_restriction_governance_guardian_rejects_unbounded_and_disallowed_restrictions() {
        let (processor, state, alice_kp, alice, _bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 5);
        let validator = Pubkey([42u8; 32]);
        let guardian_authority =
            configure_incident_guardian_for_test(&state, gov, 2, vec![alice, bob]);

        let unbounded_data = make_restrict_action_data(
            target_account_payload(Pubkey([0xD2; 32])),
            &RestrictionMode::OutgoingOnly,
            RestrictionReason::TestnetDrill,
            None,
            None,
            None,
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            guardian_authority,
            unbounded_data,
            genesis_hash,
            &validator,
        );
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("must include expires_at_slot"));

        let account_freeze_data = make_restrict_action_data(
            target_account_payload(Pubkey([0xD3; 32])),
            &RestrictionMode::Bidirectional,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(100),
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            guardian_authority,
            account_freeze_data,
            genesis_hash,
            &validator,
        );
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("target/mode is not allowed"));

        let contract_terminated_data = make_restrict_action_data(
            target_pubkey_payload(3, Pubkey([0xD4; 32])),
            &RestrictionMode::Terminated,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(100),
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            guardian_authority,
            contract_terminated_data,
            genesis_hash,
            &validator,
        );
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("target/mode is not allowed"));

        let native_pause_data = make_restrict_action_data(
            target_protocol_module_payload(ProtocolModuleId::Native),
            &RestrictionMode::ProtocolPaused,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(100),
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            guardian_authority,
            native_pause_data,
            genesis_hash,
            &validator,
        );
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("target/mode is not allowed"));
    }

    #[test]
    fn test_restriction_governance_guardian_ttl_cap_enforced() {
        let (processor, state, alice_kp, alice, _bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 5);
        let validator = Pubkey([42u8; 32]);
        let guardian_authority =
            configure_incident_guardian_for_test(&state, gov, 2, vec![alice, bob]);

        let data = make_restrict_action_data(
            target_account_payload(Pubkey([0xD5; 32])),
            &RestrictionMode::OutgoingOnly,
            RestrictionReason::TestnetDrill,
            None,
            None,
            Some(GUARDIAN_RESTRICTION_MAX_SLOTS + 1),
        );
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            guardian_authority,
            data,
            genesis_hash,
            &validator,
        );
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("exceeds guardian TTL cap"));
    }

    #[test]
    fn test_restriction_governance_guardian_can_create_code_hash_and_contract_blocks() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 5);
        let validator = Pubkey([42u8; 32]);
        let guardian_authority =
            configure_incident_guardian_for_test(&state, gov, 2, vec![alice, bob]);

        let code_record = create_active_guardian_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            guardian_authority,
            genesis_hash,
            &validator,
            1,
            1,
            target_code_hash_payload(Hash([0xD6; 32])),
            RestrictionMode::DeployBlocked,
            100,
        );
        assert!(matches!(code_record.target, RestrictionTarget::CodeHash(_)));

        let contract_target = Pubkey([0xD7; 32]);
        deploy_fake_contract(&state, alice, contract_target);
        let contract_record = create_active_guardian_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            guardian_authority,
            genesis_hash,
            &validator,
            2,
            2,
            target_pubkey_payload(3, contract_target),
            RestrictionMode::StateChangingBlocked,
            120,
        );
        assert!(matches!(
            contract_record.target,
            RestrictionTarget::Contract(_)
        ));
    }

    #[test]
    fn test_restriction_governance_guardian_can_extend_own_temporary_restriction_once() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 5);
        let validator = Pubkey([42u8; 32]);
        let guardian_authority =
            configure_incident_guardian_for_test(&state, gov, 2, vec![alice, bob]);

        create_active_guardian_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            guardian_authority,
            genesis_hash,
            &validator,
            1,
            1,
            target_account_payload(Pubkey([0xD8; 32])),
            RestrictionMode::OutgoingOnly,
            100,
        );

        let over_cap_data =
            make_extend_restriction_action_data(1, Some(GUARDIAN_RESTRICTION_MAX_SLOTS + 1), None);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            guardian_authority,
            over_cap_data,
            genesis_hash,
            &validator,
        );
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("exceeds guardian TTL cap"));

        let extend_data = make_extend_restriction_action_data(1, Some(200), None);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            guardian_authority,
            extend_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Guardian should be able to propose one extension: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(2).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(guardian_authority));

        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 2, genesis_hash, &validator);
        assert!(
            result.success,
            "Guardian extension approval should execute: {:?}",
            result.error
        );
        let old_record = state.get_restriction(1).unwrap().unwrap();
        assert_eq!(old_record.status, RestrictionStatus::Superseded);
        let successor = state.get_restriction(2).unwrap().unwrap();
        assert_eq!(successor.status, RestrictionStatus::Active);
        assert_eq!(successor.approval_authority, Some(guardian_authority));
        assert_eq!(successor.supersedes, Some(1));
        assert_eq!(successor.expires_at_slot, Some(200));

        let main_lift_data =
            make_lift_restriction_action_data(2, RestrictionLiftReason::IncidentResolved);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            main_lift_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Main governance should be able to propose guardian-created lift: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(3).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, None);
    }

    #[test]
    fn test_restriction_governance_guardian_rejects_second_extension_and_non_owned_lift() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 5);
        let validator = Pubkey([42u8; 32]);
        let guardian_authority =
            configure_incident_guardian_for_test(&state, gov, 2, vec![alice, bob]);

        create_active_guardian_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            guardian_authority,
            genesis_hash,
            &validator,
            1,
            1,
            target_account_payload(Pubkey([0xD9; 32])),
            RestrictionMode::OutgoingOnly,
            100,
        );
        let extend_data = make_extend_restriction_action_data(1, Some(200), None);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            guardian_authority,
            extend_data,
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "First guardian extension should be proposed: {:?}",
            result.error
        );
        let result =
            process_governance_control(&processor, &bob_kp, bob, 35, 2, genesis_hash, &validator);
        assert!(
            result.success,
            "First guardian extension should execute: {:?}",
            result.error
        );

        let second_extend_data = make_extend_restriction_action_data(2, Some(300), None);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            guardian_authority,
            second_extend_data,
            genesis_hash,
            &validator,
        );
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("only once"));

        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 0);
        let guardian_authority =
            configure_incident_guardian_for_test(&state, gov, 2, vec![alice, bob]);
        create_active_test_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            gov,
            genesis_hash,
            &validator,
            Some(100),
        );

        let lift_data =
            make_lift_restriction_action_data(1, RestrictionLiftReason::IncidentResolved);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            guardian_authority,
            lift_data,
            genesis_hash,
            &validator,
        );
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("only lift or extend restrictions it created"));
    }

    #[test]
    fn test_restriction_governance_split_created_records_allow_stored_and_main_followups() {
        let (processor, state, alice_kp, alice, bob_kp, bob, gov, genesis_hash) =
            setup_restriction_governance(2, 5);
        let validator = Pubkey([42u8; 32]);
        let bridge_authority =
            configure_bridge_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);
        let fresh_blockhash = create_active_bridge_split_restriction(
            &processor,
            &state,
            &alice_kp,
            alice,
            &bob_kp,
            bob,
            bridge_authority,
            genesis_hash,
            &validator,
        );

        let split_lift_data =
            make_lift_restriction_action_data(1, RestrictionLiftReason::IncidentResolved);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            bridge_authority,
            split_lift_data,
            fresh_blockhash,
            &validator,
        );
        assert!(
            result.success,
            "Stored split authority should be able to propose lift: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(2).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(bridge_authority));

        let split_extend_data =
            make_extend_restriction_action_data(1, Some(SLOTS_PER_EPOCH * 5), None);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            bridge_authority,
            split_extend_data,
            fresh_blockhash,
            &validator,
        );
        assert!(
            result.success,
            "Stored split authority should be able to propose extension: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(3).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(bridge_authority));

        let main_lift_data =
            make_lift_restriction_action_data(1, RestrictionLiftReason::IncidentResolved);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            main_lift_data,
            fresh_blockhash,
            &validator,
        );
        assert!(
            result.success,
            "Main governance should be able to propose split-created lift: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(4).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, None);

        let main_extend_data =
            make_extend_restriction_action_data(1, Some(SLOTS_PER_EPOCH * 5), None);
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            gov,
            main_extend_data,
            fresh_blockhash,
            &validator,
        );
        assert!(
            result.success,
            "Main governance should be able to propose split-created extension: {:?}",
            result.error
        );
        let proposal = state.get_governance_proposal(5).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, None);
    }

    #[test]
    fn test_upgrade_timelock_set_and_stage() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        // Deploy contract
        let contract_addr = deploy_test_contract(
            &processor,
            &state,
            &alice_kp,
            alice,
            genesis_hash,
            &validator,
        );

        // Set 3-epoch timelock
        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::SetUpgradeTimelock { epochs: 3 },
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "SetUpgradeTimelock should succeed: {:?}",
            result.error
        );

        // Verify timelock is stored
        let acct = state.get_account(&contract_addr).unwrap().unwrap();
        let ca: crate::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        assert_eq!(ca.upgrade_timelock_epochs, Some(3));
        assert!(ca.pending_upgrade.is_none());

        // Submit upgrade — should be staged, not applied immediately
        let new_code = valid_wasm_code(0x01);
        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Upgrade {
                code: new_code.clone(),
            },
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Timelocked upgrade should succeed (staged): {:?}",
            result.error
        );

        // Verify pending upgrade exists but code not applied yet
        let acct = state.get_account(&contract_addr).unwrap().unwrap();
        let ca: crate::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        assert!(ca.pending_upgrade.is_some(), "Should have pending upgrade");
        assert_eq!(ca.version, 1, "Version should NOT have bumped yet");
        let pending = ca.pending_upgrade.unwrap();
        assert_eq!(pending.code, new_code);
        assert_eq!(pending.execute_after_epoch, pending.submitted_epoch + 3);
    }

    #[test]
    fn test_code_hash_deploy_block_rejects_contract_upgrade_stage_and_execute() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let contract_addr = deploy_test_contract(
            &processor,
            &state,
            &alice_kp,
            alice,
            genesis_hash,
            &validator,
        );

        let banned_immediate_code = valid_wasm_code(0x42);
        let banned_immediate_hash = Hash::hash(&banned_immediate_code);
        let immediate_restriction_id = put_active_processor_test_restriction(
            &state,
            RestrictionTarget::CodeHash(banned_immediate_hash),
            RestrictionMode::DeployBlocked,
        );
        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Upgrade {
                code: banned_immediate_code,
            },
            genesis_hash,
            &validator,
        );
        assert!(
            !result.success,
            "Immediate upgrade to a banned code hash must fail"
        );
        let error = result.error.as_deref().unwrap_or_default();
        assert!(error.contains("ContractUpgrade rejected"));
        assert!(error.contains("DeployBlocked"));
        assert!(error.contains(&immediate_restriction_id.to_string()));
        let contract = load_contract_account_for_test(&state, contract_addr);
        assert_eq!(contract.version, 1);
        assert!(contract.pending_upgrade.is_none());
        assert_ne!(contract.code_hash, banned_immediate_hash);

        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::SetUpgradeTimelock { epochs: 1 },
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "SetUpgradeTimelock should succeed: {:?}",
            result.error
        );

        let pending_code = valid_wasm_code(0x43);
        let pending_hash = Hash::hash(&pending_code);
        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Upgrade { code: pending_code },
            genesis_hash,
            &validator,
        );
        assert!(result.success, "Upgrade should stage: {:?}", result.error);
        let pending_restriction_id = put_active_processor_test_restriction(
            &state,
            RestrictionTarget::CodeHash(pending_hash),
            RestrictionMode::DeployBlocked,
        );
        let execute_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH * 2);

        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::ExecuteUpgrade,
            execute_blockhash,
            &validator,
        );
        assert!(
            !result.success,
            "Executing a pending upgrade to a banned code hash must fail"
        );
        let error = result.error.as_deref().unwrap_or_default();
        assert!(error.contains("ExecuteContractUpgrade rejected"));
        assert!(error.contains("DeployBlocked"));
        assert!(error.contains(&pending_restriction_id.to_string()));
        let contract = load_contract_account_for_test(&state, contract_addr);
        assert_eq!(contract.version, 1);
        assert_eq!(
            contract
                .pending_upgrade
                .as_ref()
                .map(|pending| pending.code_hash),
            Some(pending_hash)
        );
        assert_ne!(contract.code_hash, pending_hash);
    }

    #[test]
    fn test_upgrade_without_timelock_is_instant() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let contract_addr = deploy_test_contract(
            &processor,
            &state,
            &alice_kp,
            alice,
            genesis_hash,
            &validator,
        );

        // No timelock set — upgrade should be instant
        let new_code = valid_wasm_code(0x02);
        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Upgrade {
                code: new_code.clone(),
            },
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Instant upgrade should succeed: {:?}",
            result.error
        );

        let acct = state.get_account(&contract_addr).unwrap().unwrap();
        let ca: crate::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        assert_eq!(ca.version, 2, "Version should be bumped immediately");
        assert!(ca.pending_upgrade.is_none());
        assert_eq!(ca.code, new_code);
    }

    #[test]
    fn test_contract_upgrade_uses_split_upgrade_proposer_when_owner_is_governed() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov_kp = Keypair::generate();
        let gov = gov_kp.pubkey();

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_last_slot(0).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(1),
            )
            .unwrap();
        let upgrade_authority =
            configure_upgrade_proposer_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            deploy_test_contract(&processor, &state, &gov_kp, gov, genesis_hash, &validator);

        let direct = submit_contract_ix(
            &processor,
            &gov_kp,
            vec![gov, contract_addr],
            crate::ContractInstruction::Upgrade {
                code: valid_wasm_code(0x22),
            },
            genesis_hash,
            &validator,
        );
        assert!(!direct.success);
        assert!(direct
            .error
            .as_deref()
            .unwrap_or("")
            .contains("proposal flow"));

        let code = valid_wasm_code(0x23);
        let mut propose_data = vec![34u8, GOVERNANCE_ACTION_CONTRACT_UPGRADE];
        propose_data.extend_from_slice(&(code.len() as u32).to_le_bytes());
        propose_data.extend_from_slice(&code);
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, upgrade_authority, contract_addr],
            data: propose_data,
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let result = processor.process_transaction(&propose_tx, &validator);
        assert!(
            result.success,
            "Proposal should succeed: {:?}",
            result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(upgrade_authority));
        assert_eq!(proposal.execute_after_epoch, 1);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            result.success,
            "Approval should succeed: {:?}",
            result.error
        );

        let mut execute_data = vec![36u8];
        execute_data.extend_from_slice(&1u64.to_le_bytes());
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data.clone(),
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, genesis_hash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("timelocked"));

        let fresh_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);

        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: execute_data,
        };
        let execute_tx = make_signed_tx(&bob_kp, execute_ix, fresh_blockhash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(
            result.success,
            "Execution should succeed: {:?}",
            result.error
        );

        let acct = state.get_account(&contract_addr).unwrap().unwrap();
        let ca: crate::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        assert_eq!(ca.version, 2);
        assert!(ca.pending_upgrade.is_none());
        assert_eq!(ca.code, code);
    }

    #[test]
    fn test_contract_upgrade_rejects_general_governance_authority_when_split_is_configured() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob = Pubkey([0x34; 32]);
        let gov_kp = Keypair::generate();
        let gov = gov_kp.pubkey();

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(1),
            )
            .unwrap();
        configure_upgrade_proposer_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            deploy_test_contract(&processor, &state, &gov_kp, gov, genesis_hash, &validator);
        let code = valid_wasm_code(0x24);
        let mut propose_data = vec![34u8, GOVERNANCE_ACTION_CONTRACT_UPGRADE];
        propose_data.extend_from_slice(&(code.len() as u32).to_le_bytes());
        propose_data.extend_from_slice(&code);
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_addr],
            data: propose_data,
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(!propose_result.success);
        assert!(propose_result.error.as_deref().unwrap_or("").contains(
            "Upgrade governance actions must use the upgrade proposer approval authority"
        ));
    }

    #[test]
    fn test_contract_call_requires_governance_proposal_when_authority_is_governed() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov_kp = Keypair::generate();
        let gov = gov_kp.pubkey();

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_last_slot(0).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(1),
            )
            .unwrap();

        let contract_addr =
            install_test_contract_account(&state, alice, governance_test_contract_code());
        let call_args = b"pause".to_vec();

        let direct = submit_contract_ix(
            &processor,
            &gov_kp,
            vec![gov, contract_addr],
            crate::ContractInstruction::Call {
                function: "record_call".to_string(),
                args: call_args.clone(),
                value: 0,
            },
            genesis_hash,
            &validator,
        );
        assert!(!direct.success);
        assert!(direct
            .error
            .as_deref()
            .unwrap_or("")
            .contains("proposal flow"));

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_addr],
            data: make_governance_contract_call_data("record_call", &call_args, 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let result = processor.process_transaction(&propose_tx, &validator);
        assert!(
            result.success,
            "Proposal should succeed: {:?}",
            result.error
        );

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            result.success,
            "Approval should succeed: {:?}",
            result.error
        );

        let mut execute_data = vec![36u8];
        execute_data.extend_from_slice(&1u64.to_le_bytes());
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data.clone(),
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, genesis_hash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(!result.success);
        assert!(result.error.as_deref().unwrap_or("").contains("timelocked"));

        let fresh_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);

        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: execute_data,
        };
        let execute_tx = make_signed_tx(&bob_kp, execute_ix, fresh_blockhash);
        let result = processor.process_transaction(&execute_tx, &validator);
        assert!(
            result.success,
            "Execution should succeed: {:?}",
            result.error
        );

        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_caller")
                .unwrap()
                .unwrap(),
            gov.0.to_vec()
        );
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_args")
                .unwrap()
                .unwrap(),
            call_args
        );
    }

    #[test]
    fn test_generic_admin_and_authority_rotation_contract_calls_remain_on_cold_governance_root() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob = Pubkey([0x47; 32]);
        let gov = Pubkey([0x48; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_last_slot(0).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();
        let treasury_authority =
            configure_treasury_executor_for_test(&state, gov, 2, vec![alice, bob]);
        configure_bridge_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);
        configure_oracle_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);
        configure_upgrade_proposer_for_test(&state, gov, 2, vec![alice, bob]);
        configure_upgrade_veto_guardian_for_test(&state, gov, 2, vec![alice, bob]);
        configure_incident_guardian_for_test(&state, gov, 2, vec![alice, bob]);

        let admin_contract =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, admin_contract, "DEXREWARDS");

        let mut admin_args = vec![0u8; 65];
        admin_args[32] = 0x11;
        admin_args[64] = 1;
        let admin_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, admin_contract],
            data: make_governance_contract_call_data("set_authorized_caller", &admin_args, 0),
        };
        let admin_tx = make_signed_tx(&alice_kp, admin_ix, genesis_hash);
        let admin_result = processor.process_transaction(&admin_tx, &validator);
        assert!(
            admin_result.success,
            "Generic admin proposal should stay on cold governance root: {:?}",
            admin_result.error
        );

        let admin_proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(admin_proposal.authority, gov);
        assert_eq!(admin_proposal.approval_authority, None);
        assert_eq!(admin_proposal.execute_after_epoch, 5);

        let rotation_contract =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, rotation_contract, "LUSD");

        let mut rotation_args = vec![0u8; 64];
        rotation_args[32] = 0x22;
        let rotation_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, rotation_contract],
            data: make_governance_contract_call_data("transfer_admin", &rotation_args, 0),
        };
        let rotation_tx = make_signed_tx(&alice_kp, rotation_ix, genesis_hash);
        let rotation_result = processor.process_transaction(&rotation_tx, &validator);
        assert!(
            rotation_result.success,
            "Authority rotation proposal should stay on cold governance root: {:?}",
            rotation_result.error
        );

        let rotation_proposal = state.get_governance_proposal(2).unwrap().unwrap();
        assert_eq!(rotation_proposal.authority, gov);
        assert_eq!(rotation_proposal.approval_authority, None);
        assert_eq!(rotation_proposal.execute_after_epoch, 5);

        let minter_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury_authority, rotation_contract],
            data: make_governance_contract_call_data("set_minter", &rotation_args, 0),
        };
        let minter_tx = make_signed_tx(&alice_kp, minter_ix, genesis_hash);
        let minter_result = processor.process_transaction(&minter_tx, &validator);
        assert!(
            minter_result.success,
            "Wrapped-token minter rotation should use treasury executor approvals: {:?}",
            minter_result.error
        );

        let minter_proposal = state.get_governance_proposal(3).unwrap().unwrap();
        assert_eq!(minter_proposal.authority, gov);
        assert_eq!(minter_proposal.approval_authority, Some(treasury_authority));
        assert_eq!(minter_proposal.execute_after_epoch, 1);

        let attester_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![
                alice,
                state
                    .get_oracle_committee_admin_authority()
                    .unwrap()
                    .unwrap(),
                rotation_contract,
            ],
            data: make_governance_contract_call_data("set_attester", &rotation_args, 0),
        };
        let attester_tx = make_signed_tx(&alice_kp, attester_ix, genesis_hash);
        let attester_result = processor.process_transaction(&attester_tx, &validator);
        assert!(
            attester_result.success,
            "Wrapped-token attester rotation should use oracle committee approvals: {:?}",
            attester_result.error
        );

        let attester_proposal = state.get_governance_proposal(4).unwrap().unwrap();
        assert_eq!(attester_proposal.authority, gov);
        assert_eq!(
            attester_proposal.approval_authority,
            state.get_oracle_committee_admin_authority().unwrap()
        );
        assert_eq!(attester_proposal.execute_after_epoch, 1);

        let wrong_role_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury_authority, rotation_contract],
            data: make_governance_contract_call_data("transfer_admin", &rotation_args, 0),
        };
        let wrong_role_tx = make_signed_tx(&alice_kp, wrong_role_ix, genesis_hash);
        let wrong_role_result = processor.process_transaction(&wrong_role_tx, &validator);
        assert!(!wrong_role_result.success);
        assert!(wrong_role_result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Governance action authority account mismatch"));
    }

    #[test]
    fn test_allowlisted_emergency_pause_contract_call_uses_incident_guardian_without_timelock() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov = Pubkey([0xB1; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();
        let guardian_authority =
            configure_incident_guardian_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "BRIDGE");

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, guardian_authority, contract_addr],
            data: make_governance_contract_call_data("mb_pause", &[], 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(
            propose_result.success,
            "Proposal should succeed: {:?}",
            propose_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(guardian_authority));
        assert_eq!(proposal.execute_after_epoch, 0);
        assert!(!proposal.executed);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should execute the pause immediately: {:?}",
            approve_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(proposal.executed);
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_caller")
                .unwrap()
                .unwrap(),
            gov.0.to_vec()
        );
    }

    #[test]
    fn test_allowlisted_emergency_pause_contract_call_stays_timelocked_on_governance_authority() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov = Pubkey([0xB4; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();
        configure_incident_guardian_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "BRIDGE");

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_addr],
            data: make_governance_contract_call_data("mb_pause", &[], 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        assert!(
            processor
                .process_transaction(&propose_tx, &validator)
                .success
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, None);
        assert_eq!(proposal.execute_after_epoch, 5);
        assert!(!proposal.executed);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should keep the proposal pending behind the governance timelock: {:?}",
            approve_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(!proposal.executed);
    }

    #[test]
    fn test_non_allowlisted_emergency_pause_contract_call_stays_timelocked() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov = Pubkey([0xB2; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "LUSD");

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_addr],
            data: make_governance_contract_call_data("emergency_pause", &[], 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        assert!(
            processor
                .process_transaction(&propose_tx, &validator)
                .success
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.execute_after_epoch, 5);
        assert!(!proposal.executed);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should keep the proposal pending behind the timelock: {:?}",
            approve_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(!proposal.executed);
    }

    #[test]
    fn test_incident_guardian_rejects_non_allowlisted_contract_call() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob = Pubkey([0x35; 32]);
        let gov = Pubkey([0xB5; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();
        let guardian_authority =
            configure_incident_guardian_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "LUSD");

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, guardian_authority, contract_addr],
            data: make_governance_contract_call_data("emergency_pause", &[], 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(!propose_result.success);
        assert!(propose_result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Incident guardian authority may only submit allowlisted immediate risk-reduction proposals"));
    }

    #[test]
    fn test_allowlisted_unpause_contract_call_remains_timelocked() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov = Pubkey([0xB3; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "BRIDGE");

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_addr],
            data: make_governance_contract_call_data("mb_unpause", &[], 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        assert!(
            processor
                .process_transaction(&propose_tx, &validator)
                .success
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.execute_after_epoch, 5);
        assert!(!proposal.executed);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should leave unpause behind the timelock: {:?}",
            approve_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(!proposal.executed);
    }

    #[test]
    fn test_bridge_committee_admin_contract_call_uses_split_approval_authority_and_timelock() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov = Pubkey([0xB8; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();
        let bridge_authority =
            configure_bridge_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "BRIDGE");

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, bridge_authority, contract_addr],
            data: make_governance_contract_call_data("set_required_confirmations", &[], 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(
            propose_result.success,
            "Proposal should succeed: {:?}",
            propose_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(bridge_authority));
        assert_eq!(proposal.execute_after_epoch, 1);
        assert!(!proposal.executed);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should leave the proposal behind the committee timelock: {:?}",
            approve_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(!proposal.executed);
    }

    #[test]
    fn test_bridge_committee_admin_contract_call_rejects_governance_authority_direct_path() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob = Pubkey([0x36; 32]);
        let gov = Pubkey([0xB9; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();
        configure_bridge_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "BRIDGE");

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_addr],
            data: make_governance_contract_call_data("set_request_timeout", &[], 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(!propose_result.success);
        assert!(propose_result.error.as_deref().unwrap_or("").contains(
            "Bridge governance actions must use the bridge committee admin approval authority"
        ));
    }

    #[test]
    fn test_oracle_committee_admin_contract_call_uses_split_approval_authority_and_timelock() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov = Pubkey([0xBA; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();
        let oracle_authority =
            configure_oracle_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "ORACLE");

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, oracle_authority, contract_addr],
            data: make_governance_contract_call_data("set_authorized_attester", &[], 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(
            propose_result.success,
            "Proposal should succeed: {:?}",
            propose_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(oracle_authority));
        assert_eq!(proposal.execute_after_epoch, 1);
        assert!(!proposal.executed);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should leave the proposal behind the committee timelock: {:?}",
            approve_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(!proposal.executed);
    }

    #[test]
    fn test_upgrade_governance_actions_use_split_role_policies() {
        let (processor, _state, _alice_kp, _alice, _treasury, _genesis_hash) = setup();
        let contract = Pubkey([0xBD; 32]);

        assert!(
            processor.governance_action_requires_upgrade_proposer_policy(
                &GovernanceAction::ContractUpgrade {
                    contract,
                    code: vec![1, 2, 3],
                }
            )
        );
        assert!(
            processor.governance_action_requires_upgrade_proposer_policy(
                &GovernanceAction::SetContractUpgradeTimelock {
                    contract,
                    epochs: 2,
                }
            )
        );
        assert!(
            processor.governance_action_requires_upgrade_proposer_policy(
                &GovernanceAction::ExecuteContractUpgrade { contract }
            )
        );
        assert!(
            !processor.governance_action_requires_upgrade_proposer_policy(
                &GovernanceAction::VetoContractUpgrade { contract }
            )
        );
        assert!(
            processor.governance_action_requires_upgrade_veto_guardian_policy(
                &GovernanceAction::VetoContractUpgrade { contract }
            )
        );
    }

    #[test]
    fn test_treasury_executor_policy_covers_protocol_outflow_contract_calls() {
        let (processor, state, _alice_kp, _alice, _treasury, _genesis_hash) = setup();
        let owner = Pubkey([0xC4; 32]);
        let margin_contract = Pubkey([0xC5; 32]);
        let lend_contract = Pubkey([0xC6; 32]);
        let vault_contract = Pubkey([0xC7; 32]);
        let pump_contract = Pubkey([0xC8; 32]);
        let amm_contract = Pubkey([0xC9; 32]);
        let generic_contract = Pubkey([0xCA; 32]);

        register_contract_symbol_for_test(&state, owner, margin_contract, "DEXMARGIN");
        register_contract_symbol_for_test(&state, owner, lend_contract, "LEND");
        register_contract_symbol_for_test(&state, owner, vault_contract, "SPOREVAULT");
        register_contract_symbol_for_test(&state, owner, pump_contract, "SPOREPUMP");
        register_contract_symbol_for_test(&state, owner, amm_contract, "DEXAMM");
        register_contract_symbol_for_test(&state, owner, generic_contract, "GENERIC");

        let mut insurance_args = vec![0u8; 73];
        insurance_args[0] = 9u8;
        insurance_args[1] = 0x44;
        insurance_args[33..41].copy_from_slice(&500_000u64.to_le_bytes());
        insurance_args[41] = 0x99;

        let mut amm_outflow_args = vec![0u8; 41];
        amm_outflow_args[0] = 21u8;
        amm_outflow_args[1] = 0x55;
        amm_outflow_args[33..41].copy_from_slice(&7u64.to_le_bytes());

        let mut amm_admin_args = vec![0u8; 65];
        amm_admin_args[0] = 20u8;
        amm_admin_args[1] = 0x11;
        amm_admin_args[33] = 0x22;

        let mut margin_admin_args = vec![0u8; 49];
        margin_admin_args[0] = 7u8;

        assert!(processor
            .governance_action_requires_treasury_executor_policy(
                &GovernanceAction::TreasuryTransfer {
                    recipient: Pubkey([0xCA; 32]),
                    amount: 1,
                }
            )
            .unwrap());
        assert!(processor
            .governance_action_requires_treasury_executor_policy(&GovernanceAction::ContractCall {
                contract: margin_contract,
                function: "call".to_string(),
                args: insurance_args,
                value: 0,
            })
            .unwrap());
        assert!(processor
            .governance_action_requires_treasury_executor_policy(&GovernanceAction::ContractCall {
                contract: lend_contract,
                function: "withdraw_reserves".to_string(),
                args: vec![0u8; 8],
                value: 0,
            })
            .unwrap());
        assert!(processor
            .governance_action_requires_treasury_executor_policy(&GovernanceAction::ContractCall {
                contract: vault_contract,
                function: "withdraw_protocol_fees".to_string(),
                args: vec![],
                value: 0,
            })
            .unwrap());
        assert!(processor
            .governance_action_requires_treasury_executor_policy(&GovernanceAction::ContractCall {
                contract: amm_contract,
                function: "call".to_string(),
                args: amm_outflow_args,
                value: 0,
            })
            .unwrap());
        assert!(processor
            .governance_action_requires_treasury_executor_policy(&GovernanceAction::ContractCall {
                contract: pump_contract,
                function: "withdraw_fees".to_string(),
                args: 500_000u64.to_le_bytes().to_vec(),
                value: 0,
            })
            .unwrap());
        assert!(!processor
            .governance_action_requires_treasury_executor_policy(&GovernanceAction::ContractCall {
                contract: margin_contract,
                function: "call".to_string(),
                args: margin_admin_args,
                value: 0,
            })
            .unwrap());
        assert!(!processor
            .governance_action_requires_treasury_executor_policy(&GovernanceAction::ContractCall {
                contract: amm_contract,
                function: "call".to_string(),
                args: amm_admin_args,
                value: 0,
            })
            .unwrap());
        assert!(!processor
            .governance_action_requires_treasury_executor_policy(&GovernanceAction::ContractCall {
                contract: generic_contract,
                function: "withdraw_fees".to_string(),
                args: vec![],
                value: 0,
            })
            .unwrap());
    }

    #[test]
    fn test_restriction_governance_actions_do_not_match_legacy_split_role_policies() {
        let (processor, _state, _alice_kp, _alice, _treasury, _genesis_hash) = setup();
        let actions = vec![
            GovernanceAction::Restrict {
                target: RestrictionTarget::Account(Pubkey([0xD0; 32])),
                mode: RestrictionMode::OutgoingOnly,
                reason: RestrictionReason::TestnetDrill,
                evidence_hash: None,
                evidence_uri_hash: None,
                expires_at_slot: Some(100),
            },
            GovernanceAction::LiftRestriction {
                restriction_id: 1,
                reason: RestrictionLiftReason::IncidentResolved,
            },
            GovernanceAction::ExtendRestriction {
                restriction_id: 1,
                new_expires_at_slot: Some(200),
                evidence_hash: None,
            },
        ];

        for action in actions {
            assert!(
                !processor
                    .governance_action_requires_treasury_executor_policy(&action)
                    .unwrap(),
                "{:?} must not use treasury executor routing",
                action
            );
            assert!(
                !processor.governance_action_requires_upgrade_proposer_policy(&action),
                "{:?} must not use upgrade proposer routing",
                action
            );
            assert!(
                !processor.governance_action_requires_upgrade_veto_guardian_policy(&action),
                "{:?} must not use upgrade veto guardian routing",
                action
            );
        }
    }

    #[test]
    fn test_veto_upgrade_governance_action_uses_split_veto_guardian_authority() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov = Pubkey([0xBE; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_last_slot(0).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(1),
            )
            .unwrap();
        let veto_authority =
            configure_upgrade_veto_guardian_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr = deploy_test_contract(
            &processor,
            &state,
            &alice_kp,
            alice,
            genesis_hash,
            &validator,
        );

        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::SetUpgradeTimelock { epochs: 2 },
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "Timelock should succeed: {:?}",
            result.error
        );

        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Upgrade {
                code: valid_wasm_code(0x31),
            },
            genesis_hash,
            &validator,
        );
        assert!(result.success, "Upgrade should stage: {:?}", result.error);

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, veto_authority, contract_addr],
            data: vec![34u8, GOVERNANCE_ACTION_VETO_UPGRADE],
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(
            propose_result.success,
            "Proposal should succeed: {:?}",
            propose_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(veto_authority));
        assert_eq!(proposal.execute_after_epoch, 0);
        assert!(!proposal.executed);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should execute the veto immediately: {:?}",
            approve_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(proposal.executed);

        let acct = state.get_account(&contract_addr).unwrap().unwrap();
        let ca: crate::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        assert!(ca.pending_upgrade.is_none());
        assert_eq!(ca.version, 1);
    }

    #[test]
    fn test_veto_upgrade_rejects_general_governance_authority_when_split_is_configured() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob = Pubkey([0x35; 32]);
        let gov = Pubkey([0xBF; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(1),
            )
            .unwrap();
        configure_upgrade_veto_guardian_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr = deploy_test_contract(
            &processor,
            &state,
            &alice_kp,
            alice,
            genesis_hash,
            &validator,
        );
        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::SetUpgradeTimelock { epochs: 1 },
            genesis_hash,
            &validator,
        );
        assert!(result.success);
        let result = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Upgrade {
                code: valid_wasm_code(0x32),
            },
            genesis_hash,
            &validator,
        );
        assert!(result.success);

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_addr],
            data: vec![34u8, GOVERNANCE_ACTION_VETO_UPGRADE],
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(!propose_result.success);
        assert!(propose_result.error.as_deref().unwrap_or("").contains(
            "Upgrade veto governance actions must use the upgrade veto guardian approval authority"
        ));
    }

    #[test]
    fn test_marketplace_pause_entries_are_allowlisted() {
        let (processor, state, _alice_kp, _alice, _treasury, _genesis_hash) = setup();
        let owner = Pubkey([0xB6; 32]);
        let auction_contract = Pubkey([0xC1; 32]);
        let market_contract = Pubkey([0xC2; 32]);

        register_contract_symbol_for_test(&state, owner, auction_contract, "AUCTION");
        register_contract_symbol_for_test(&state, owner, market_contract, "MARKET");

        assert!(processor
            .governance_action_uses_immediate_risk_reduction_policy(
                &GovernanceAction::ContractCall {
                    contract: auction_contract,
                    function: "ma_pause".to_string(),
                    args: vec![],
                    value: 0,
                }
            )
            .unwrap());
        assert!(processor
            .governance_action_uses_immediate_risk_reduction_policy(
                &GovernanceAction::ContractCall {
                    contract: market_contract,
                    function: "mm_pause".to_string(),
                    args: vec![],
                    value: 0,
                }
            )
            .unwrap());
    }

    #[test]
    fn test_additional_pause_safe_contract_entries_are_allowlisted() {
        let (processor, state, _alice_kp, _alice, _treasury, _genesis_hash) = setup();
        let owner = Pubkey([0xB8; 32]);
        let compute_contract = Pubkey([0xD1; 32]);
        let predict_contract = Pubkey([0xD2; 32]);
        let pump_contract = Pubkey([0xD3; 32]);

        register_contract_symbol_for_test(&state, owner, compute_contract, "COMPUTE");
        register_contract_symbol_for_test(&state, owner, predict_contract, "PREDICT");
        register_contract_symbol_for_test(&state, owner, pump_contract, "SPOREPUMP");

        assert!(processor
            .governance_action_uses_immediate_risk_reduction_policy(
                &GovernanceAction::ContractCall {
                    contract: compute_contract,
                    function: "cm_pause".to_string(),
                    args: vec![],
                    value: 0,
                }
            )
            .unwrap());
        assert!(processor
            .governance_action_uses_immediate_risk_reduction_policy(
                &GovernanceAction::ContractCall {
                    contract: predict_contract,
                    function: "emergency_pause".to_string(),
                    args: vec![],
                    value: 0,
                }
            )
            .unwrap());
        assert!(processor
            .governance_action_uses_immediate_risk_reduction_policy(
                &GovernanceAction::ContractCall {
                    contract: pump_contract,
                    function: "pause".to_string(),
                    args: vec![],
                    value: 0,
                }
            )
            .unwrap());
    }

    #[test]
    fn test_dex_pause_pair_entry_remains_allowlisted() {
        let (processor, state, _alice_kp, _alice, _treasury, _genesis_hash) = setup();
        let owner = Pubkey([0xB7; 32]);
        let dex_contract = Pubkey([0xC3; 32]);

        register_contract_symbol_for_test(&state, owner, dex_contract, "DEX");

        assert!(processor
            .governance_action_uses_immediate_risk_reduction_policy(
                &GovernanceAction::ContractCall {
                    contract: dex_contract,
                    function: "pause_pair".to_string(),
                    args: vec![],
                    value: 0,
                }
            )
            .unwrap());
    }

    #[test]
    fn test_margin_price_updates_use_oracle_committee_immediate_policy() {
        let (processor, state, _alice_kp, _alice, _treasury, _genesis_hash) = setup();
        let owner = Pubkey([0xB9; 32]);
        let margin_contract = Pubkey([0xD4; 32]);

        register_contract_symbol_for_test(&state, owner, margin_contract, "DEXMARGIN");

        for opcode in [1u8, 31u8] {
            let action = GovernanceAction::ContractCall {
                contract: margin_contract,
                function: "call".to_string(),
                args: vec![opcode],
                value: 0,
            };
            assert!(processor
                .governance_action_requires_oracle_committee_admin_policy(&action)
                .unwrap());
            assert!(processor
                .governance_action_uses_immediate_risk_reduction_policy(&action)
                .unwrap());
        }
    }

    #[test]
    fn test_bridge_validator_change_requires_governance_proposal_when_authority_is_governed() {
        let mut call_args = vec![0u8; 64];
        call_args[32] = 0x55;
        assert_governed_committee_contract_call_requires_proposal(
            "add_bridge_validator",
            call_args,
        );
    }

    #[test]
    fn test_bridge_threshold_change_requires_governance_proposal_when_authority_is_governed() {
        let mut call_args = vec![0u8; 40];
        call_args[0] = 0x11;
        call_args[32..40].copy_from_slice(&3u64.to_le_bytes());
        assert_governed_committee_contract_call_requires_proposal(
            "set_required_confirmations",
            call_args,
        );
    }

    #[test]
    fn test_bridge_timeout_change_requires_governance_proposal_when_authority_is_governed() {
        let mut call_args = vec![0u8; 40];
        call_args[0] = 0x22;
        call_args[32..40].copy_from_slice(&1_000u64.to_le_bytes());
        assert_governed_committee_contract_call_requires_proposal("set_request_timeout", call_args);
    }

    #[test]
    fn test_oracle_feeder_change_requires_governance_proposal_when_authority_is_governed() {
        let mut call_args = vec![0u8; 40];
        call_args[0] = 0x88;
        call_args[32..36].copy_from_slice(b"LICN");
        call_args[36..40].copy_from_slice(&4u32.to_le_bytes());
        assert_governed_committee_contract_call_requires_proposal("add_price_feeder", call_args);
    }

    #[test]
    fn test_oracle_attester_change_requires_governance_proposal_when_authority_is_governed() {
        let mut call_args = vec![0u8; 36];
        call_args[0] = 0x77;
        call_args[32..36].copy_from_slice(&1u32.to_le_bytes());
        assert_governed_committee_contract_call_requires_proposal(
            "set_authorized_attester",
            call_args,
        );
    }

    #[test]
    fn test_margin_price_update_contract_call_uses_oracle_committee_approval_authority_and_executes_immediately(
    ) {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov_kp = Keypair::generate();
        let gov = gov_kp.pubkey();

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_last_slot(0).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();
        let oracle_authority =
            configure_oracle_committee_admin_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "DEXMARGIN");

        let mut call_args = vec![0u8; 49];
        call_args[0] = 1u8;
        call_args[1] = 0x44;
        call_args[33..41].copy_from_slice(&7u64.to_le_bytes());
        call_args[41..49].copy_from_slice(&1_000_000u64.to_le_bytes());

        let direct = submit_contract_ix(
            &processor,
            &gov_kp,
            vec![gov, contract_addr],
            crate::ContractInstruction::Call {
                function: "call".to_string(),
                args: call_args.clone(),
                value: 0,
            },
            genesis_hash,
            &validator,
        );
        assert!(!direct.success);
        assert!(direct
            .error
            .as_deref()
            .unwrap_or("")
            .contains("proposal flow"));

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, oracle_authority, contract_addr],
            data: make_governance_contract_call_data("call", &call_args, 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(
            propose_result.success,
            "Proposal should succeed: {:?}",
            propose_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(oracle_authority));
        assert_eq!(proposal.execute_after_epoch, 0);
        assert!(!proposal.executed);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should execute immediately: {:?}",
            approve_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert!(proposal.executed);
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_caller")
                .unwrap()
                .unwrap(),
            gov.0.to_vec()
        );
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_args")
                .unwrap()
                .unwrap(),
            call_args
        );
    }

    #[test]
    fn test_margin_insurance_withdraw_contract_call_uses_treasury_executor_approval_authority_and_timelock(
    ) {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov_kp = Keypair::generate();
        let gov = gov_kp.pubkey();

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_last_slot(0).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();
        let treasury_authority =
            configure_treasury_executor_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "DEXMARGIN");

        let mut call_args = vec![0u8; 73];
        call_args[0] = 9u8;
        call_args[1] = 0x44;
        call_args[33..41].copy_from_slice(&500_000u64.to_le_bytes());
        call_args[41] = 0x99;

        let direct = submit_contract_ix(
            &processor,
            &gov_kp,
            vec![gov, contract_addr],
            crate::ContractInstruction::Call {
                function: "call".to_string(),
                args: call_args.clone(),
                value: 0,
            },
            genesis_hash,
            &validator,
        );
        assert!(!direct.success);
        assert!(direct
            .error
            .as_deref()
            .unwrap_or("")
            .contains("proposal flow"));

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury_authority, contract_addr],
            data: make_governance_contract_call_data("call", &call_args, 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(
            propose_result.success,
            "Proposal should succeed: {:?}",
            propose_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(treasury_authority));
        assert_eq!(proposal.execute_after_epoch, 1);
        assert!(!proposal.executed);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should succeed: {:?}",
            approve_result.error
        );

        let mut execute_data = vec![36u8];
        execute_data.extend_from_slice(&1u64.to_le_bytes());
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data.clone(),
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, genesis_hash);
        let execute_result = processor.process_transaction(&execute_tx, &validator);
        assert!(!execute_result.success);
        assert!(execute_result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("timelocked"));

        let fresh_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: execute_data,
        };
        let execute_tx = make_signed_tx(&bob_kp, execute_ix, fresh_blockhash);
        let execute_result = processor.process_transaction(&execute_tx, &validator);
        assert!(
            execute_result.success,
            "Execution should succeed: {:?}",
            execute_result.error
        );

        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_caller")
                .unwrap()
                .unwrap(),
            gov.0.to_vec()
        );
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_args")
                .unwrap()
                .unwrap(),
            call_args
        );
    }

    #[test]
    fn test_amm_protocol_fee_collection_contract_call_uses_treasury_executor_approval_authority_and_timelock(
    ) {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov_kp = Keypair::generate();
        let gov = gov_kp.pubkey();

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_last_slot(0).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();
        let treasury_authority =
            configure_treasury_executor_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "DEXAMM");

        let mut call_args = vec![0u8; 41];
        call_args[0] = 21u8;
        call_args[1] = 0x44;
        call_args[33..41].copy_from_slice(&7u64.to_le_bytes());

        let direct = submit_contract_ix(
            &processor,
            &gov_kp,
            vec![gov, contract_addr],
            crate::ContractInstruction::Call {
                function: "call".to_string(),
                args: call_args.clone(),
                value: 0,
            },
            genesis_hash,
            &validator,
        );
        assert!(!direct.success);
        assert!(direct
            .error
            .as_deref()
            .unwrap_or("")
            .contains("proposal flow"));

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury_authority, contract_addr],
            data: make_governance_contract_call_data("call", &call_args, 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(
            propose_result.success,
            "Proposal should succeed: {:?}",
            propose_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.authority, gov);
        assert_eq!(proposal.approval_authority, Some(treasury_authority));
        assert_eq!(proposal.execute_after_epoch, 1);
        assert!(!proposal.executed);

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should succeed: {:?}",
            approve_result.error
        );

        let mut execute_data = vec![36u8];
        execute_data.extend_from_slice(&1u64.to_le_bytes());
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data: execute_data.clone(),
        };
        let execute_tx = make_signed_tx(&alice_kp, execute_ix, genesis_hash);
        let execute_result = processor.process_transaction(&execute_tx, &validator);
        assert!(!execute_result.success);
        assert!(execute_result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("timelocked"));

        let fresh_blockhash = advance_test_slot(&state, SLOTS_PER_EPOCH);
        let execute_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: execute_data,
        };
        let execute_tx = make_signed_tx(&bob_kp, execute_ix, fresh_blockhash);
        let execute_result = processor.process_transaction(&execute_tx, &validator);
        assert!(
            execute_result.success,
            "Execution should succeed: {:?}",
            execute_result.error
        );

        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_caller")
                .unwrap()
                .unwrap(),
            gov.0.to_vec()
        );
        assert_eq!(
            state
                .get_contract_storage(&contract_addr, b"last_args")
                .unwrap()
                .unwrap(),
            call_args
        );
    }

    #[test]
    fn test_amm_protocol_fee_collection_rejects_governance_authority_direct_path() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob = Pubkey([0x49; 32]);
        let gov = Pubkey([0x4A; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();
        configure_treasury_executor_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "DEXAMM");

        let mut call_args = vec![0u8; 41];
        call_args[0] = 21u8;
        call_args[1] = 0x44;
        call_args[33..41].copy_from_slice(&7u64.to_le_bytes());
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_addr],
            data: make_governance_contract_call_data("call", &call_args, 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(!propose_result.success);
        assert!(propose_result.error.as_deref().unwrap_or("").contains(
            "Protocol fund movement governance actions must use the treasury executor approval authority"
        ));
    }

    #[test]
    fn test_protocol_outflow_contract_calls_reject_governance_authority_direct_path() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob = Pubkey([0x45; 32]);
        let gov = Pubkey([0x46; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                )
                .with_timelock(5),
            )
            .unwrap();
        configure_treasury_executor_for_test(&state, gov, 2, vec![alice, bob]);

        let contract_addr =
            install_test_contract_account(&state, gov, governance_test_contract_code());
        register_contract_symbol_for_test(&state, gov, contract_addr, "LEND");

        let mut call_args = vec![0u8; 8];
        call_args.copy_from_slice(&500_000u64.to_le_bytes());
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_addr],
            data: make_governance_contract_call_data("withdraw_reserves", &call_args, 0),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        let propose_result = processor.process_transaction(&propose_tx, &validator);
        assert!(!propose_result.success);
        assert!(propose_result.error.as_deref().unwrap_or("").contains(
            "Protocol fund movement governance actions must use the treasury executor approval authority"
        ));
    }

    #[test]
    fn test_register_symbol_requires_governance_proposal_when_owner_is_governed() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov_kp = Keypair::generate();
        let gov = gov_kp.pubkey();
        let contract_id = Pubkey([0xA1; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                ),
            )
            .unwrap();
        deploy_fake_contract(&state, gov, contract_id);

        let json_payload = r#"{"symbol":"GOVSYM","name":"Governed Symbol","template":"token"}"#;
        let mut direct_data = vec![20u8];
        direct_data.extend_from_slice(json_payload.as_bytes());
        let direct_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![gov, contract_id],
            data: direct_data,
        };
        let direct_tx = make_signed_tx(&gov_kp, direct_ix, genesis_hash);
        let direct_result = processor.process_transaction(&direct_tx, &validator);
        assert!(!direct_result.success);
        assert!(direct_result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("proposal flow"));

        let mut propose_data = vec![34u8, GOVERNANCE_ACTION_REGISTER_SYMBOL];
        propose_data.extend_from_slice(&(json_payload.len() as u32).to_le_bytes());
        propose_data.extend_from_slice(json_payload.as_bytes());
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_id],
            data: propose_data,
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        assert!(
            processor
                .process_transaction(&propose_tx, &validator)
                .success
        );

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should execute symbol registration: {:?}",
            approve_result.error
        );

        let entry = state.get_symbol_registry("GOVSYM").unwrap().unwrap();
        assert_eq!(entry.program, contract_id);
        assert_eq!(entry.owner, gov);

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.approval_authority, None);
        assert!(proposal.executed);
        assert_eq!(proposal.action_label, "register_contract_symbol");
    }

    #[test]
    fn test_set_contract_abi_requires_governance_proposal_when_owner_is_governed() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov_kp = Keypair::generate();
        let gov = gov_kp.pubkey();
        let contract_id = Pubkey([0xA2; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                ),
            )
            .unwrap();
        deploy_fake_contract(&state, gov, contract_id);

        let abi = serde_json::json!({
            "version": "1.0",
            "name": "GovernedAbi",
            "functions": []
        });
        let abi_bytes = serde_json::to_vec(&abi).unwrap();

        let mut direct_data = vec![18u8];
        direct_data.extend_from_slice(&abi_bytes);
        let direct_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![gov, contract_id],
            data: direct_data,
        };
        let direct_tx = make_signed_tx(&gov_kp, direct_ix, genesis_hash);
        let direct_result = processor.process_transaction(&direct_tx, &validator);
        assert!(!direct_result.success);
        assert!(direct_result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("proposal flow"));

        let mut propose_data = vec![34u8, GOVERNANCE_ACTION_SET_CONTRACT_ABI];
        propose_data.extend_from_slice(&(abi_bytes.len() as u32).to_le_bytes());
        propose_data.extend_from_slice(&abi_bytes);
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_id],
            data: propose_data,
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        assert!(
            processor
                .process_transaction(&propose_tx, &validator)
                .success
        );

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should execute ABI update: {:?}",
            approve_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.approval_authority, None);

        let acct = state.get_account(&contract_id).unwrap().unwrap();
        let contract: crate::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        let abi = contract.abi.expect("governance proposal should set ABI");
        assert_eq!(abi.name, "GovernedAbi");
    }

    #[test]
    fn test_contract_close_owner_semantics_preserved_for_non_active_lifecycle_statuses() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        for (idx, status) in [
            crate::ContractLifecycleStatus::Suspended,
            crate::ContractLifecycleStatus::Quarantined,
            crate::ContractLifecycleStatus::Terminated,
        ]
        .into_iter()
        .enumerate()
        {
            let contract_id = Pubkey([0xB0 + idx as u8; 32]);
            let destination = Pubkey([0xC0 + idx as u8; 32]);
            let close_amount = Account::licn_to_spores(10 + idx as u64);

            deploy_fake_contract(&state, alice, contract_id);
            set_contract_lifecycle_status_for_test(&state, contract_id, status);
            let mut contract_account = state.get_account(&contract_id).unwrap().unwrap();
            contract_account.spores = close_amount;
            contract_account.spendable = close_amount;
            state.put_account(&contract_id, &contract_account).unwrap();

            let result = submit_contract_ix(
                &processor,
                &alice_kp,
                vec![alice, contract_id, destination],
                crate::ContractInstruction::Close,
                genesis_hash,
                &validator,
            );

            assert!(
                result.success,
                "owner close should succeed for {:?}: {:?}",
                status, result.error
            );
            let closed = state.get_account(&contract_id).unwrap().unwrap();
            assert!(!closed.executable);
            assert!(closed.data.is_empty());
            assert_eq!(closed.spendable, 0);
            assert_eq!(state.get_balance(&destination).unwrap(), close_amount);
        }
    }

    #[test]
    fn test_contract_close_non_owner_still_rejected_for_non_active_lifecycle_contract() {
        let (processor, state, _alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let eve_kp = Keypair::generate();
        let eve = eve_kp.pubkey();
        let contract_id = Pubkey([0xB5; 32]);
        let destination = Pubkey([0xC5; 32]);

        state
            .put_account(&eve, &Account::new(Account::licn_to_spores(1_000), eve))
            .unwrap();
        deploy_fake_contract(&state, alice, contract_id);
        set_contract_lifecycle_status_for_test(
            &state,
            contract_id,
            crate::ContractLifecycleStatus::Terminated,
        );
        let mut contract_account = state.get_account(&contract_id).unwrap().unwrap();
        contract_account.spores = Account::licn_to_spores(15);
        contract_account.spendable = Account::licn_to_spores(15);
        state.put_account(&contract_id, &contract_account).unwrap();

        let result = submit_contract_ix(
            &processor,
            &eve_kp,
            vec![eve, contract_id, destination],
            crate::ContractInstruction::Close,
            genesis_hash,
            &validator,
        );

        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Only contract owner can close"));
        let contract_account = state.get_account(&contract_id).unwrap().unwrap();
        assert!(contract_account.executable);
        let contract: crate::ContractAccount =
            serde_json::from_slice(&contract_account.data).unwrap();
        assert_eq!(
            contract.lifecycle_status,
            crate::ContractLifecycleStatus::Terminated
        );
        assert_eq!(state.get_balance(&destination).unwrap_or(0), 0);
    }

    #[test]
    fn test_contract_close_requires_governance_proposal_when_owner_is_governed() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov_kp = Keypair::generate();
        let gov = gov_kp.pubkey();
        let contract_id = Pubkey([0xA3; 32]);
        let destination = Pubkey([0xA4; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob, gov],
                    "community_treasury",
                ),
            )
            .unwrap();
        deploy_fake_contract(&state, gov, contract_id);
        set_contract_lifecycle_status_for_test(
            &state,
            contract_id,
            crate::ContractLifecycleStatus::Quarantined,
        );

        let mut contract_account = state.get_account(&contract_id).unwrap().unwrap();
        contract_account.spores = Account::licn_to_spores(25);
        contract_account.spendable = Account::licn_to_spores(25);
        state.put_account(&contract_id, &contract_account).unwrap();

        let direct_ix = Instruction {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: vec![gov, contract_id, destination],
            data: crate::ContractInstruction::Close.serialize().unwrap(),
        };
        let direct_tx = make_signed_tx(&gov_kp, direct_ix, genesis_hash);
        let direct_result = processor.process_transaction(&direct_tx, &validator);
        assert!(!direct_result.success);
        assert!(direct_result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("proposal flow"));

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_id, destination],
            data: vec![34u8, GOVERNANCE_ACTION_CONTRACT_CLOSE],
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        assert!(
            processor
                .process_transaction(&propose_tx, &validator)
                .success
        );

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        let approve_result = processor.process_transaction(&approve_tx, &validator);
        assert!(
            approve_result.success,
            "Approval should execute contract close: {:?}",
            approve_result.error
        );

        let proposal = state.get_governance_proposal(1).unwrap().unwrap();
        assert_eq!(proposal.approval_authority, None);

        let closed = state.get_account(&contract_id).unwrap().unwrap();
        assert!(!closed.executable);
        assert!(closed.data.is_empty());
        assert_eq!(
            state.get_balance(&destination).unwrap(),
            Account::licn_to_spores(25)
        );
    }

    #[test]
    fn test_governance_proposal_lifecycle_events_are_emitted() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov = Pubkey([0xA6; 32]);
        let recipient = Pubkey([0xA5; 32]);
        let treasury_authority = crate::multisig::derive_treasury_executor_authority(&gov);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob],
                    "community_treasury",
                ),
            )
            .unwrap();
        state
            .set_treasury_executor_authority(&treasury_authority)
            .unwrap();
        state
            .set_governed_wallet_config(
                &treasury_authority,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob],
                    crate::multisig::TREASURY_EXECUTOR_LABEL,
                ),
            )
            .unwrap();

        let amount = Account::licn_to_spores(10);
        let mut propose_data = vec![34u8, GOVERNANCE_ACTION_TREASURY_TRANSFER];
        propose_data.extend_from_slice(&amount.to_le_bytes());
        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, treasury_authority, recipient],
            data: propose_data,
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        assert!(
            processor
                .process_transaction(&propose_tx, &validator)
                .success
        );

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        assert!(
            processor
                .process_transaction(&approve_tx, &validator)
                .success
        );

        let events = state
            .get_events_by_program(&SYSTEM_PROGRAM_ID, 10, None)
            .unwrap();
        let proposal_events: Vec<_> = events
            .into_iter()
            .filter(|event| event.data.get("proposal_id").map(String::as_str) == Some("1"))
            .collect();

        let event_names: Vec<_> = proposal_events
            .iter()
            .map(|event| event.name.as_str())
            .collect();
        assert!(event_names.contains(&"GovernanceProposalCreated"));
        assert!(event_names.contains(&"GovernanceProposalApproved"));
        assert!(event_names.contains(&"GovernanceProposalExecuted"));
        assert!(proposal_events
            .iter()
            .all(|event| event.program == SYSTEM_PROGRAM_ID));
        assert!(proposal_events
            .iter()
            .all(|event| { event.data.get("action") == Some(&"treasury_transfer".to_string()) }));
    }

    #[test]
    fn test_governance_contract_call_events_include_structured_call_hints() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let gov = Pubkey([0xA7; 32]);

        let fund = Account::licn_to_spores(1_000);
        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.put_account(&bob, &Account::new(fund, bob)).unwrap();
        state.put_account(&gov, &Account::new(fund, gov)).unwrap();
        state.set_governance_authority(&gov).unwrap();
        state
            .set_governed_wallet_config(
                &gov,
                &crate::multisig::GovernedWalletConfig::new(
                    2,
                    vec![alice, bob],
                    "community_treasury",
                ),
            )
            .unwrap();

        let contract_addr =
            install_test_contract_account(&state, alice, governance_test_contract_code());
        let call_args = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let call_value = 7u64;

        let propose_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, gov, contract_addr],
            data: make_governance_contract_call_data("record_call", &call_args, call_value),
        };
        let propose_tx = make_signed_tx(&alice_kp, propose_ix, genesis_hash);
        assert!(
            processor
                .process_transaction(&propose_tx, &validator)
                .success
        );

        let mut approve_data = vec![35u8];
        approve_data.extend_from_slice(&1u64.to_le_bytes());
        let approve_ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![bob],
            data: approve_data,
        };
        let approve_tx = make_signed_tx(&bob_kp, approve_ix, genesis_hash);
        assert!(
            processor
                .process_transaction(&approve_tx, &validator)
                .success
        );

        let events = state
            .get_events_by_program(&SYSTEM_PROGRAM_ID, 10, None)
            .unwrap();
        let proposal_events: Vec<_> = events
            .into_iter()
            .filter(|event| event.data.get("proposal_id").map(String::as_str) == Some("1"))
            .collect();

        let event_names: Vec<_> = proposal_events
            .iter()
            .map(|event| event.name.as_str())
            .collect();
        assert!(event_names.contains(&"GovernanceProposalCreated"));
        assert!(event_names.contains(&"GovernanceProposalApproved"));
        assert!(event_names.contains(&"GovernanceProposalExecuted"));

        let target_contract = contract_addr.to_base58();
        let call_args_len = call_args.len().to_string();
        let call_value = call_value.to_string();
        assert!(proposal_events.iter().all(|event| {
            event.data.get("target_contract") == Some(&target_contract)
                && event.data.get("target_function") == Some(&"record_call".to_string())
                && event.data.get("call_args_len") == Some(&call_args_len)
                && event.data.get("call_value_spores") == Some(&call_value)
        }));
    }

    #[test]
    fn test_upgrade_timelock_rejects_double_stage() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let contract_addr = deploy_test_contract(
            &processor,
            &state,
            &alice_kp,
            alice,
            genesis_hash,
            &validator,
        );

        // Set timelock
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::SetUpgradeTimelock { epochs: 2 },
            genesis_hash,
            &validator,
        );
        assert!(r.success);

        // First upgrade → staged
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Upgrade {
                code: valid_wasm_code(0x03),
            },
            genesis_hash,
            &validator,
        );
        assert!(r.success);

        // Second upgrade while first is pending → should fail
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Upgrade {
                code: valid_wasm_code(0x04),
            },
            genesis_hash,
            &validator,
        );
        assert!(!r.success, "Double-stage should be rejected");
        assert!(r
            .error
            .as_deref()
            .unwrap_or("")
            .contains("already has a pending upgrade"));
    }

    #[test]
    fn test_execute_upgrade_before_timelock_expires_fails() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let contract_addr = deploy_test_contract(
            &processor,
            &_state,
            &alice_kp,
            alice,
            genesis_hash,
            &validator,
        );

        // Set 5-epoch timelock (current slot = 0 → epoch 0, needs > epoch 5)
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::SetUpgradeTimelock { epochs: 5 },
            genesis_hash,
            &validator,
        );
        assert!(r.success);

        // Stage upgrade
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Upgrade {
                code: valid_wasm_code(0x05),
            },
            genesis_hash,
            &validator,
        );
        assert!(r.success);

        // Try execute immediately (epoch 0, needs > epoch 5) → should fail
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::ExecuteUpgrade,
            genesis_hash,
            &validator,
        );
        assert!(!r.success, "Should fail: timelock not expired");
        assert!(r
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Timelock has not expired"));
    }

    #[test]
    fn test_execute_upgrade_no_pending_fails() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let contract_addr = deploy_test_contract(
            &processor,
            &_state,
            &alice_kp,
            alice,
            genesis_hash,
            &validator,
        );

        // Try execute with no pending upgrade → should fail
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::ExecuteUpgrade,
            genesis_hash,
            &validator,
        );
        assert!(!r.success, "Should fail: no pending upgrade");
        assert!(r
            .error
            .as_deref()
            .unwrap_or("")
            .contains("No pending upgrade"));
    }

    #[test]
    fn test_veto_upgrade_by_governance_authority() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let contract_addr = deploy_test_contract(
            &processor,
            &state,
            &alice_kp,
            alice,
            genesis_hash,
            &validator,
        );

        // Set governance authority
        let gov_kp = crate::Keypair::generate();
        let gov = gov_kp.pubkey();
        state.set_governance_authority(&gov).unwrap();
        // Fund governance account (10 LICN)
        let gov_acct = crate::Account::new(10, gov);
        state.put_account(&gov, &gov_acct).unwrap();

        // Set timelock + stage upgrade
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::SetUpgradeTimelock { epochs: 2 },
            genesis_hash,
            &validator,
        );
        assert!(r.success);

        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Upgrade {
                code: valid_wasm_code(0x06),
            },
            genesis_hash,
            &validator,
        );
        assert!(r.success);

        // Verify pending exists
        let acct = state.get_account(&contract_addr).unwrap().unwrap();
        let ca: crate::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        assert!(ca.pending_upgrade.is_some());

        // Governance authority vetoes
        let r = submit_contract_ix(
            &processor,
            &gov_kp,
            vec![gov, contract_addr],
            crate::ContractInstruction::VetoUpgrade,
            genesis_hash,
            &validator,
        );
        assert!(r.success, "Veto should succeed: {:?}", r.error);

        // Verify pending is cleared
        let acct = state.get_account(&contract_addr).unwrap().unwrap();
        let ca: crate::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        assert!(
            ca.pending_upgrade.is_none(),
            "Pending upgrade should be cleared"
        );
        assert_eq!(ca.version, 1, "Version should NOT change after veto");
    }

    #[test]
    fn test_veto_by_non_governance_fails() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let contract_addr = deploy_test_contract(
            &processor,
            &state,
            &alice_kp,
            alice,
            genesis_hash,
            &validator,
        );

        // Set governance authority to someone else
        let gov_kp = crate::Keypair::generate();
        let gov = gov_kp.pubkey();
        state.set_governance_authority(&gov).unwrap();

        // Set timelock + stage upgrade
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::SetUpgradeTimelock { epochs: 1 },
            genesis_hash,
            &validator,
        );
        assert!(r.success);

        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Upgrade {
                code: valid_wasm_code(0x07),
            },
            genesis_hash,
            &validator,
        );
        assert!(r.success);

        // Alice (not governance) tries to veto → should fail
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::VetoUpgrade,
            genesis_hash,
            &validator,
        );
        assert!(!r.success, "Non-governance should not be able to veto");
        assert!(r
            .error
            .as_deref()
            .unwrap_or("")
            .contains("governance authority"));
    }

    #[test]
    fn test_cannot_remove_timelock_while_upgrade_pending() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let contract_addr = deploy_test_contract(
            &processor,
            &_state,
            &alice_kp,
            alice,
            genesis_hash,
            &validator,
        );

        // Set timelock
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::SetUpgradeTimelock { epochs: 2 },
            genesis_hash,
            &validator,
        );
        assert!(r.success);

        // Stage upgrade
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::Upgrade {
                code: valid_wasm_code(0x08),
            },
            genesis_hash,
            &validator,
        );
        assert!(r.success);

        // Try to remove timelock while upgrade is pending → should fail
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::SetUpgradeTimelock { epochs: 0 },
            genesis_hash,
            &validator,
        );
        assert!(
            !r.success,
            "Should not remove timelock while upgrade pending"
        );
        assert!(r.error.as_deref().unwrap_or("").contains("pending"));
    }

    #[test]
    fn test_set_timelock_zero_removes_it() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);

        let contract_addr = deploy_test_contract(
            &processor,
            &state,
            &alice_kp,
            alice,
            genesis_hash,
            &validator,
        );

        // Set timelock
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::SetUpgradeTimelock { epochs: 5 },
            genesis_hash,
            &validator,
        );
        assert!(r.success);

        let acct = state.get_account(&contract_addr).unwrap().unwrap();
        let ca: crate::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        assert_eq!(ca.upgrade_timelock_epochs, Some(5));

        // Remove timelock (no pending upgrade)
        let r = submit_contract_ix(
            &processor,
            &alice_kp,
            vec![alice, contract_addr],
            crate::ContractInstruction::SetUpgradeTimelock { epochs: 0 },
            genesis_hash,
            &validator,
        );
        assert!(r.success, "Remove timelock should succeed: {:?}", r.error);

        let acct = state.get_account(&contract_addr).unwrap().unwrap();
        let ca: crate::ContractAccount = serde_json::from_slice(&acct.data).unwrap();
        assert_eq!(ca.upgrade_timelock_epochs, None);
    }

    #[test]
    fn test_contract_account_serde_backward_compat_no_timelock() {
        // Legacy contract data without timelock fields should deserialize with defaults
        let owner_bytes: Vec<u8> = vec![1u8; 32];
        let hash_bytes: Vec<u8> = vec![0u8; 32];
        let json = serde_json::json!({
            "code": [0, 0x61, 0x73, 0x6D],
            "storage": {},
            "owner": owner_bytes,
            "code_hash": hash_bytes,
            "version": 1
        });
        let ca: crate::ContractAccount = serde_json::from_value(json).unwrap();
        assert_eq!(ca.upgrade_timelock_epochs, None);
        assert!(ca.pending_upgrade.is_none());
    }

    // ─── CU Budget & Priority Fee Tests ───────────────────────────────

    /// Helper: build a transfer TX with custom compute_budget and compute_unit_price
    fn make_transfer_tx_with_cu(
        from_kp: &Keypair,
        from: Pubkey,
        to: Pubkey,
        amount_licn: u64,
        recent_blockhash: Hash,
        compute_budget: Option<u64>,
        compute_unit_price: Option<u64>,
    ) -> Transaction {
        let mut data = vec![0u8];
        data.extend_from_slice(&Account::licn_to_spores(amount_licn).to_le_bytes());

        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![from, to],
            data,
        };

        let mut message = crate::transaction::Message::new(vec![ix], recent_blockhash);
        message.compute_budget = compute_budget;
        message.compute_unit_price = compute_unit_price;
        let mut tx = Transaction::new(message);
        let sig = from_kp.sign(&tx.message.serialize());
        tx.signatures.push(sig);
        tx
    }

    #[test]
    fn test_default_compute_budget_applied() {
        let msg = crate::transaction::Message::new(vec![], Hash::default());
        assert_eq!(
            msg.effective_compute_budget(),
            crate::transaction::DEFAULT_COMPUTE_BUDGET
        );
    }

    #[test]
    fn test_custom_compute_budget_applied() {
        let mut msg = crate::transaction::Message::new(vec![], Hash::default());
        msg.compute_budget = Some(500_000);
        assert_eq!(msg.effective_compute_budget(), 500_000);
    }

    #[test]
    fn test_compute_budget_capped_at_max() {
        let mut msg = crate::transaction::Message::new(vec![], Hash::default());
        msg.compute_budget = Some(2_000_000);
        assert_eq!(
            msg.effective_compute_budget(),
            crate::transaction::MAX_COMPUTE_BUDGET
        );
    }

    #[test]
    fn test_zero_compute_budget_uses_default() {
        let mut msg = crate::transaction::Message::new(vec![], Hash::default());
        msg.compute_budget = Some(0);
        assert_eq!(
            msg.effective_compute_budget(),
            crate::transaction::DEFAULT_COMPUTE_BUDGET
        );
    }

    #[test]
    fn test_priority_fee_computation_zero_price() {
        let (_, _, alice_kp, alice, _, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let tx = make_transfer_tx(&alice_kp, alice, bob, 10, genesis_hash);
        let priority = TxProcessor::compute_priority_fee(&tx);
        assert_eq!(priority, 0);
    }

    #[test]
    fn test_priority_fee_computation_with_price() {
        let (_, _, alice_kp, alice, _, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        // compute_unit_price = 1000 μspores/CU, budget = 200_000 CU (default)
        // priority = 200_000 × 1000 / 1_000_000 = 200 spores
        let tx =
            make_transfer_tx_with_cu(&alice_kp, alice, bob, 10, genesis_hash, None, Some(1000));
        let priority = TxProcessor::compute_priority_fee(&tx);
        assert_eq!(priority, 200);
    }

    #[test]
    fn test_priority_fee_with_custom_budget() {
        let (_, _, alice_kp, alice, _, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        // compute_unit_price = 5000 μspores/CU, budget = 400_000 CU
        // priority = 400_000 × 5000 / 1_000_000 = 2000 spores
        let tx = make_transfer_tx_with_cu(
            &alice_kp,
            alice,
            bob,
            10,
            genesis_hash,
            Some(400_000),
            Some(5000),
        );
        let priority = TxProcessor::compute_priority_fee(&tx);
        assert_eq!(priority, 2000);
    }

    #[test]
    fn test_total_fee_includes_priority() {
        let (_, _, alice_kp, alice, _, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let fee_config = FeeConfig::default_from_constants();

        let tx_no_prio = make_transfer_tx(&alice_kp, alice, bob, 10, genesis_hash);
        let base = TxProcessor::compute_base_fee(&tx_no_prio, &fee_config);
        let total_no_prio = TxProcessor::compute_transaction_fee(&tx_no_prio, &fee_config);
        assert_eq!(total_no_prio, base);

        let tx_with_prio =
            make_transfer_tx_with_cu(&alice_kp, alice, bob, 10, genesis_hash, None, Some(1000));
        let total_with_prio = TxProcessor::compute_transaction_fee(&tx_with_prio, &fee_config);
        let priority = TxProcessor::compute_priority_fee(&tx_with_prio);
        assert_eq!(total_with_prio, base + priority);
        assert!(total_with_prio > total_no_prio);
    }

    #[test]
    fn test_priority_fee_charged_on_transfer() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        let initial_balance = state.get_balance(&alice).unwrap();
        let transfer_amount = Account::licn_to_spores(10);

        // cu_price=1000 μspores/CU, default budget=200K → priority=200 spores
        let tx =
            make_transfer_tx_with_cu(&alice_kp, alice, bob, 10, genesis_hash, None, Some(1000));

        let fee_config = FeeConfig::default_from_constants();
        let expected_total = TxProcessor::compute_transaction_fee(&tx, &fee_config);
        let expected_priority = TxProcessor::compute_priority_fee(&tx);
        assert_eq!(expected_priority, 200);

        let result = processor.process_transaction(&tx, &validator);
        assert!(result.success);
        assert_eq!(result.fee_paid, expected_total);

        let final_balance = state.get_balance(&alice).unwrap();
        assert_eq!(
            final_balance,
            initial_balance - transfer_amount - expected_total
        );
    }

    #[test]
    fn test_fee_debit_bypasses_account_and_native_asset_restrictions() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let spendable = state.get_account(&alice).unwrap().unwrap().spendable;

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::OutgoingOnly,
        );
        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::AccountAsset {
                account: alice,
                asset: NATIVE_LICN_ASSET_ID,
            },
            RestrictionMode::FrozenAmount { amount: spendable },
        );

        let evm_address = [0xAB; 20];
        let mut data = vec![12u8];
        data.extend_from_slice(&evm_address);
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice],
            data,
        };
        let before_balance = state.get_balance(&alice).unwrap();
        let tx = make_signed_tx(&alice_kp, ix, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(
            result.success,
            "restricted signer should still pay fee for non-value action: {:?}",
            result.error
        );

        let after_balance = state.get_balance(&alice).unwrap();
        assert_eq!(before_balance - after_balance, result.fee_paid);
        assert_eq!(state.lookup_evm_address(&evm_address).unwrap(), Some(alice));
    }

    #[test]
    fn test_restricted_transfer_failure_keeps_only_fee_debit() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([0xB4; 32]);
        let validator = Pubkey([42u8; 32]);

        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::OutgoingOnly,
        );

        let before_balance = state.get_balance(&alice).unwrap();
        let tx = make_transfer_tx(&alice_kp, alice, bob, 10, genesis_hash);
        let result = processor.process_transaction(&tx, &validator);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Native transfer blocked by active sender account restriction"));

        let after_balance = state.get_balance(&alice).unwrap();
        assert_eq!(before_balance - after_balance, result.fee_paid);
        assert_eq!(state.get_balance(&bob).unwrap_or(0), 0);
    }

    #[test]
    fn test_restricted_governance_authority_can_pay_fee_for_lift_remediation() {
        let (processor, state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let validator = Pubkey([42u8; 32]);
        let fund = Account::licn_to_spores(1_000);

        state
            .put_account(&alice, &Account::new(fund, alice))
            .unwrap();
        state.set_last_slot(0).unwrap();
        state.set_governance_authority(&alice).unwrap();
        state
            .set_governed_wallet_config(
                &alice,
                &crate::multisig::GovernedWalletConfig::new(1, vec![alice], "governance_authority"),
            )
            .unwrap();

        let target_restriction_id = put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(Pubkey([0xD8; 32])),
            RestrictionMode::OutgoingOnly,
        );
        let spendable = state.get_account(&alice).unwrap().unwrap().spendable;
        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::Account(alice),
            RestrictionMode::OutgoingOnly,
        );
        put_active_processor_test_restriction(
            &state,
            RestrictionTarget::AccountAsset {
                account: alice,
                asset: NATIVE_LICN_ASSET_ID,
            },
            RestrictionMode::FrozenAmount { amount: spendable },
        );

        let before_balance = state.get_balance(&alice).unwrap();
        let result = process_governance_proposal(
            &processor,
            &alice_kp,
            alice,
            alice,
            make_lift_restriction_action_data(
                target_restriction_id,
                RestrictionLiftReason::FalsePositive,
            ),
            genesis_hash,
            &validator,
        );
        assert!(
            result.success,
            "restricted governance authority should lift via governed flow: {:?}",
            result.error
        );

        let lifted = state
            .get_restriction(target_restriction_id)
            .unwrap()
            .unwrap();
        assert_eq!(lifted.status, RestrictionStatus::Lifted);
        assert_eq!(lifted.lifted_by, Some(alice));
        let after_balance = state.get_balance(&alice).unwrap();
        assert_eq!(before_balance - after_balance, result.fee_paid);
    }

    #[test]
    fn test_compute_budget_capped_succeeds() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        let mut data = vec![0u8];
        data.extend_from_slice(&Account::licn_to_spores(10).to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, bob],
            data,
        };
        let mut message = crate::transaction::Message::new(vec![ix], genesis_hash);
        message.compute_budget = Some(crate::transaction::MAX_COMPUTE_BUDGET + 1);
        let mut tx = Transaction::new(message);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let result = processor.process_transaction(&tx, &validator);
        // effective_compute_budget() caps at MAX, so this should succeed
        assert!(
            result.success,
            "Budget capped at MAX should succeed: {:?}",
            result.error
        );
    }

    #[test]
    fn test_backward_compat_no_cu_fields() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);
        let validator = Pubkey([42u8; 32]);

        let tx = make_transfer_tx(&alice_kp, alice, bob, 10, genesis_hash);
        assert!(tx.message.compute_budget.is_none());
        assert!(tx.message.compute_unit_price.is_none());

        let result = processor.process_transaction(&tx, &validator);
        assert!(result.success);
        assert_eq!(result.fee_paid, BASE_FEE);
    }

    #[test]
    fn test_simulation_fee_includes_priority() {
        let (processor, _state, alice_kp, alice, _treasury, genesis_hash) = setup();
        let bob = Pubkey([2u8; 32]);

        let tx = make_transfer_tx_with_cu(
            &alice_kp,
            alice,
            bob,
            10,
            genesis_hash,
            Some(300_000),
            Some(500),
        );
        let sim = processor.simulate_transaction(&tx);
        assert!(sim.success, "Simulation should succeed: {:?}", sim.error);
        assert!(sim.compute_used > 0, "Should report compute used");
        let fee_config = FeeConfig::default_from_constants();
        let expected_fee = TxProcessor::compute_transaction_fee(&tx, &fee_config);
        assert_eq!(sim.fee, expected_fee);
    }

    #[test]
    fn test_fee_free_txs_zero_base_with_priority() {
        let (_, _, alice_kp, alice, _, genesis_hash) = setup();
        let fee_config = FeeConfig::default_from_constants();

        // Type 4 = Genesis transfer (fee-free)
        let mut data = vec![4u8];
        data.extend_from_slice(&Account::licn_to_spores(10).to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![alice, Pubkey([9u8; 32])],
            data,
        };
        let mut message = crate::transaction::Message::new(vec![ix], genesis_hash);
        message.compute_unit_price = Some(1000);
        let mut tx = Transaction::new(message);
        tx.signatures.push(alice_kp.sign(&tx.message.serialize()));

        let base = TxProcessor::compute_base_fee(&tx, &fee_config);
        assert_eq!(base, 0, "Fee-free tx should have 0 base fee");
        let priority = TxProcessor::compute_priority_fee(&tx);
        assert_eq!(priority, 200); // 200K CU × 1000 μspores / 1M
    }

    #[test]
    fn test_mempool_cu_price_ordering() {
        use crate::Mempool;
        let mut pool = Mempool::new(100, 300);
        let kp1 = Keypair::generate();
        let kp2 = Keypair::generate();
        let kp3 = Keypair::generate();
        let hash = Hash::hash(b"test");

        let tx1 = {
            let ix = Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![kp1.pubkey()],
                data: vec![0u8],
            };
            let msg = crate::transaction::Message::new(vec![ix], hash);
            let mut tx = Transaction::new(msg);
            tx.signatures.push(kp1.sign(&tx.message.serialize()));
            tx
        };

        let tx2 = {
            let ix = Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![kp2.pubkey()],
                data: vec![0u8],
            };
            let mut msg = crate::transaction::Message::new(vec![ix], hash);
            msg.compute_unit_price = Some(1000);
            let mut tx = Transaction::new(msg);
            tx.signatures.push(kp2.sign(&tx.message.serialize()));
            tx
        };

        let tx3 = {
            let ix = Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![kp3.pubkey()],
                data: vec![0u8],
            };
            let mut msg = crate::transaction::Message::new(vec![ix], hash);
            msg.compute_unit_price = Some(5000);
            let mut tx = Transaction::new(msg);
            tx.signatures.push(kp3.sign(&tx.message.serialize()));
            tx
        };

        let fee_config = FeeConfig::default_from_constants();
        let fee1 = TxProcessor::compute_transaction_fee(&tx1, &fee_config);
        let fee2 = TxProcessor::compute_transaction_fee(&tx2, &fee_config);
        let fee3 = TxProcessor::compute_transaction_fee(&tx3, &fee_config);

        pool.add_transaction(tx1, fee1, 0).unwrap();
        pool.add_transaction(tx2, fee2, 0).unwrap();
        pool.add_transaction(tx3, fee3, 0).unwrap();

        let top = pool.get_top_transactions(3);
        assert_eq!(top.len(), 3);
        assert_eq!(top[0].sender(), kp3.pubkey(), "Highest CU price first");
        assert_eq!(top[1].sender(), kp2.pubkey(), "Medium CU price second");
        assert_eq!(top[2].sender(), kp1.pubkey(), "No CU price last");
    }

    #[test]
    fn test_message_serde_with_cu_fields() {
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![Pubkey([1u8; 32])],
            data: vec![0u8],
        };
        let mut msg = crate::transaction::Message::new(vec![ix], Hash::default());
        msg.compute_budget = Some(500_000);
        msg.compute_unit_price = Some(2000);

        let serialized = msg.serialize();
        let deserialized: crate::transaction::Message = bincode::deserialize(&serialized).unwrap();
        assert_eq!(deserialized.compute_budget, Some(500_000));
        assert_eq!(deserialized.compute_unit_price, Some(2000));
        assert_eq!(deserialized.effective_compute_budget(), 500_000);
        assert_eq!(deserialized.effective_compute_unit_price(), 2000);
    }

    #[test]
    fn test_message_serde_backward_compat() {
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![Pubkey([1u8; 32])],
            data: vec![0u8],
        };
        let msg = crate::transaction::Message::new(vec![ix], Hash::default());
        assert!(msg.compute_budget.is_none());
        assert!(msg.compute_unit_price.is_none());
        assert_eq!(
            msg.effective_compute_budget(),
            crate::transaction::DEFAULT_COMPUTE_BUDGET
        );
        assert_eq!(msg.effective_compute_unit_price(), 0);
    }
}
