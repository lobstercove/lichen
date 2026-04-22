// Lichen Core - State Management with Column Families

use crate::account::{Account, Pubkey};
use crate::contract::ContractEvent;
use crate::evm::EvmAccount;
use crate::evm::{EvmReceipt, EvmTxRecord};
use crate::hash::Hash;
use crate::mossstake::MossStakePool;
use crate::transaction::Transaction;
use alloy_primitives::U256;
use ed25519_dalek::VerifyingKey;
use rocksdb::{Direction, WriteBatch, DB};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::Mutex;

mod account_state;
mod archive_state;
mod batch_state;
mod cold_storage;
mod contract_state;
mod evm_state;
mod ledger_state;
mod merkle_state;
mod metrics_state;
mod mossstake_state;
mod nft_state;
mod program_state;
mod secondary_indexes;
mod shielded_state;
mod snapshot_io;
mod stats_metadata;
mod storage_bootstrap;
mod validator_state;

pub use merkle_state::{AccountProof, MerkleProof};
pub use metrics_state::{Metrics, MetricsStore};
pub use snapshot_io::CheckpointMeta;

#[cfg(test)]
use merkle_state::{build_merkle_tree, generate_proof};

/// Type alias for bulk key-value export results to satisfy clippy::type_complexity.
pub type KvEntries = Vec<(Vec<u8>, Vec<u8>)>;

/// Page of key-value entries returned by paginated export functions.
pub struct KvPage {
    /// The entries in this page.
    pub entries: KvEntries,
    /// Total number of entries in the column family (across all pages).
    pub total: u64,
    /// Cursor key for the next page (exclusive). None when there are no more pages.
    pub next_cursor: Option<Vec<u8>>,
    /// Whether more entries are available after this page.
    pub has_more: bool,
}

/// Canonical contract-storage stats for a single program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContractStorageStats {
    /// Number of key/value entries stored under the program prefix.
    pub entry_count: usize,
    /// Sum of canonical value lengths in bytes.
    pub total_value_size: usize,
}

/// Column family names
const CF_ACCOUNTS: &str = "accounts";
const CF_BLOCKS: &str = "blocks";
const CF_TRANSACTIONS: &str = "transactions";
const CF_ACCOUNT_TXS: &str = "account_txs";
const CF_SLOTS: &str = "slots";
const CF_VALIDATORS: &str = "validators";
const CF_STATS: &str = "stats";
const CF_EVM_MAP: &str = "evm_map"; // EVM address → Native pubkey mapping
const CF_EVM_ACCOUNTS: &str = "evm_accounts"; // EVM address → account info
const CF_EVM_STORAGE: &str = "evm_storage"; // EVM address + slot → value
const CF_EVM_TXS: &str = "evm_txs"; // EVM tx hash → metadata
const CF_EVM_RECEIPTS: &str = "evm_receipts"; // EVM tx hash → receipt
const CF_MOSSSTAKE: &str = "mossstake"; // MossStake liquid staking pool
const CF_STAKE_POOL: &str = "stake_pool"; // Validator stake pool
const CF_NFT_BY_OWNER: &str = "nft_by_owner"; // Owner pubkey + token pubkey
const CF_NFT_BY_COLLECTION: &str = "nft_by_collection"; // Collection pubkey + token pubkey
const CF_NFT_ACTIVITY: &str = "nft_activity"; // Collection pubkey + slot + seq + token
const CF_PROGRAMS: &str = "programs"; // Program pubkey
const CF_PROGRAM_CALLS: &str = "program_calls"; // Program pubkey + slot + seq + tx
const CF_MARKET_ACTIVITY: &str = "market_activity"; // Collection pubkey + slot + seq + tx
const CF_SYMBOL_REGISTRY: &str = "symbol_registry"; // Symbol -> program registry
const CF_EVENTS: &str = "events"; // Contract events (program + slot + seq)
const CF_TOKEN_BALANCES: &str = "token_balances"; // Token program + holder -> balance
const CF_TOKEN_TRANSFERS: &str = "token_transfers"; // Token program + slot + seq -> transfer
const CF_TX_BY_SLOT: &str = "tx_by_slot"; // Slot + seq -> tx hash
const CF_TX_TO_SLOT: &str = "tx_to_slot"; // tx hash -> slot (reverse index for O(1) lookup)
const CF_HOLDER_TOKENS: &str = "holder_tokens"; // Holder + token_program -> balance (reverse index)
const CF_SOLANA_TOKEN_ACCOUNTS: &str = "solana_token_accounts"; // token_account(32) -> token_program(32) + holder(32)
const CF_SOLANA_HOLDER_TOKEN_ACCOUNTS: &str = "solana_holder_token_accounts"; // holder(32) + token_account(32) -> token_program(32)
const CF_SYMBOL_BY_PROGRAM: &str = "symbol_by_program"; // Program pubkey -> symbol (reverse index for O(1) lookup)
const CF_EVENTS_BY_SLOT: &str = "events_by_slot"; // slot(8,BE) + seq(8,BE) -> event_key (secondary index)
const CF_CONTRACT_STORAGE: &str = "contract_storage"; // Contract storage (LichenID reputation etc.)
const CF_MERKLE_LEAVES: &str = "merkle_leaves"; // pubkey(32) -> leaf_hash(32) (incremental Merkle cache)
const CF_CONTRACT_MERKLE_LEAVES: &str = "contract_merkle_leaves"; // full_key(32+N) hash -> leaf_hash(32) (contract storage Merkle cache)
                                                                  // Shielded pool (ZK privacy layer)
const CF_SHIELDED_COMMITMENTS: &str = "shielded_commitments"; // index(8,LE) -> commitment_leaf(32)
const CF_SHIELDED_NULLIFIERS: &str = "shielded_nullifiers"; // nullifier(32) -> 0x01 (spent flag)
const CF_SHIELDED_POOL: &str = "shielded_pool"; // singleton key "state" -> ShieldedPoolState (JSON)
const CF_EVM_LOGS_BY_SLOT: &str = "evm_logs_by_slot"; // slot(8,BE) -> Vec<EvmLogEntry> (Task 3.4)
const CF_ACCOUNT_SNAPSHOTS: &str = "account_snapshots"; // pubkey(32)+slot(8,BE) -> Account (Task 3.9 archive mode)
const CF_PENDING_VALIDATOR_CHANGES: &str = "pending_validator_changes"; // epoch(8,BE)+slot(8,BE)+pubkey(8) -> PendingValidatorChange
const CF_TX_META: &str = "tx_meta"; // tx_hash(32) -> compute_units_used(8,LE) — execution metadata

const SOLANA_TOKEN_PROGRAM_ID_B58: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const SOLANA_ASSOCIATED_TOKEN_PROGRAM_ID_B58: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

fn decode_const_pubkey(base58: &str, label: &str) -> Result<Pubkey, String> {
    Pubkey::from_base58(base58).map_err(|e| format!("Invalid {} constant: {}", label, e))
}

fn find_program_address(seeds: &[&[u8]], program_id: &Pubkey) -> Result<Pubkey, String> {
    for bump in (0u8..=255u8).rev() {
        let mut hasher = Sha256::new();
        for seed in seeds {
            hasher.update(seed);
        }
        hasher.update([bump]);
        hasher.update(program_id.0);
        hasher.update(b"ProgramDerivedAddress");
        let hash = hasher.finalize();
        let bytes: [u8; 32] = hash
            .as_slice()
            .try_into()
            .map_err(|_| "Failed to derive Solana PDA".to_string())?;
        if VerifyingKey::from_bytes(&bytes).is_err() {
            return Ok(Pubkey(bytes));
        }
    }

    Err("Failed to derive Solana associated token address".to_string())
}

pub fn derive_solana_associated_token_address(
    owner: &Pubkey,
    mint: &Pubkey,
) -> Result<Pubkey, String> {
    let token_program = decode_const_pubkey(SOLANA_TOKEN_PROGRAM_ID_B58, "Solana token program")?;
    let associated_program = decode_const_pubkey(
        SOLANA_ASSOCIATED_TOKEN_PROGRAM_ID_B58,
        "Solana associated token program",
    )?;
    let seeds: [&[u8]; 3] = [&owner.0, &token_program.0, &mint.0];
    find_program_address(&seeds, &associated_program)
}

fn solana_token_account_binding_bytes(token_program: &Pubkey, holder: &Pubkey) -> [u8; 64] {
    let mut bytes = [0u8; 64];
    bytes[..32].copy_from_slice(&token_program.0);
    bytes[32..].copy_from_slice(&holder.0);
    bytes
}

fn parse_solana_token_account_binding(data: &[u8]) -> Option<(Pubkey, Pubkey)> {
    if data.len() != 64 {
        return None;
    }

    let mut token_program = [0u8; 32];
    token_program.copy_from_slice(&data[..32]);
    let mut holder = [0u8; 32];
    holder.copy_from_slice(&data[32..]);
    Some((Pubkey(token_program), Pubkey(holder)))
}

fn solana_holder_token_account_key(holder: &Pubkey, token_account: &Pubkey) -> [u8; 64] {
    let mut key = [0u8; 64];
    key[..32].copy_from_slice(&holder.0);
    key[32..].copy_from_slice(&token_account.0);
    key
}

// ─── P2-3: Cold storage column family names ─────────────────────────────────
// Cold DB mirrors a subset of hot CFs for archival data (old blocks, txns).
const COLD_CF_BLOCKS: &str = "blocks";
const COLD_CF_TRANSACTIONS: &str = "transactions";
const COLD_CF_TX_TO_SLOT: &str = "tx_to_slot";
const COLD_CF_ACCOUNT_TXS: &str = "account_txs";
const COLD_CF_EVENTS: &str = "events";
const COLD_CF_TOKEN_TRANSFERS: &str = "token_transfers";
const COLD_CF_PROGRAM_CALLS: &str = "program_calls";

/// Default number of slots to retain in the hot DB before migration-eligible.
/// Blocks older than `current_slot - COLD_RETENTION_SLOTS` can be moved.
pub const COLD_RETENTION_SLOTS: u64 = 100_000;

// ─── PERF-OPT 3: In-process blockhash cache ─────────────────────────────────

/// Cached (slot, hash) pairs for the recent 300 slots.
struct BlockhashCache {
    /// Sorted by slot (oldest first). Capped to ~300 entries.
    entries: Vec<(u64, Hash)>,
}

// AUDIT-FIX C-7: Blockhash cache moved from static global to StateStore instance field
// so that each store instance has its own cache (avoids cross-instance pollution in tests).

/// Token symbol registry entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolRegistryEntry {
    pub symbol: String,
    pub program: Pubkey,
    pub owner: Pubkey,
    pub name: Option<String>,
    pub template: Option<String>,
    pub metadata: Option<Value>,
    #[serde(default)]
    pub decimals: Option<u8>,
}

/// Token transfer record stored in CF_TOKEN_TRANSFERS
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenTransfer {
    pub token_program: String,
    pub from: String,
    pub to: String,
    pub amount: u64,
    pub slot: u64,
    pub tx_hash: Option<String>,
}

/// State store using RocksDB with column families
#[derive(Clone)]
pub struct StateStore {
    db: Arc<DB>,
    /// Optional cold/archival DB for historical blocks and transactions.
    /// When present, `get_block_by_slot` and `get_transaction` fall through
    /// to cold storage if the key is missing from the hot DB. Populated by
    /// `migrate_to_cold()` which moves old data out of the hot DB.
    cold_db: Option<Arc<DB>>,
    metrics: Arc<MetricsStore>,
    /// AUDIT-FIX H6: Mutex to serialize next_event_seq read-modify-write operations,
    /// preventing duplicate sequence numbers under concurrent access.
    event_seq_lock: Arc<std::sync::Mutex<()>>,
    /// AUDIT-FIX CP-8: Mutex to serialize next_transfer_seq read-modify-write operations,
    /// preventing duplicate transfer sequence numbers under concurrent access.
    transfer_seq_lock: Arc<std::sync::Mutex<()>>,
    /// PHASE1-FIX S-2: Mutex to serialize next_tx_slot_seq read-modify-write operations,
    /// preventing duplicate tx sequence numbers under concurrent block processing.
    tx_slot_seq_lock: Arc<std::sync::Mutex<()>>,
    /// Serialize canonical block writes so tip metadata cannot move backward
    /// when adjacent heights are persisted by competing runtime paths.
    block_write_lock: Arc<std::sync::Mutex<()>>,
    /// P10-CORE-01: Mutex to serialize add_burned read-modify-write operations,
    /// preventing lost updates under concurrent access.
    burned_lock: Arc<std::sync::Mutex<()>>,
    /// Mutex to serialize add_minted read-modify-write operations,
    /// preventing lost updates under concurrent access.
    minted_lock: Arc<std::sync::Mutex<()>>,
    /// AUDIT-FIX B-1: Mutex to serialize treasury read-modify-write in charge_fee_direct,
    /// preventing lost-update race when parallel TX groups credit fees concurrently.
    treasury_lock: Arc<std::sync::Mutex<()>>,
    /// AUDIT-FIX C-7: Per-instance blockhash cache (was previously a static global).
    /// Populated lazily on first `get_recent_blockhashes`, kept warm by `push_blockhash_cache`.
    blockhash_cache: Arc<Mutex<Option<BlockhashCache>>>,
    /// Task 3.9: When true, every `put_account` also writes a snapshot to
    /// CF_ACCOUNT_SNAPSHOTS keyed by `pubkey(32) + slot(8,BE)`, enabling
    /// historical state queries via `get_account_at_slot`.
    archive_mode: Arc<std::sync::atomic::AtomicBool>,
}

/// Atomic write batch for transaction processing (T1.4/T3.1).
///
/// Accumulates all state mutations (accounts, transactions, pools, etc.) in
/// memory. Nothing is written to RocksDB until `commit()` is called, which
/// flushes everything in a single atomic `WriteBatch`. If the batch is dropped
/// without committing, all mutations are discarded (implicit rollback).
///
/// The overlay `HashMap` ensures reads-after-writes within the same transaction
/// see the updated values without hitting disk.
pub struct StateBatch {
    /// The underlying RocksDB WriteBatch (accumulates puts)
    batch: WriteBatch,
    /// In-memory overlay for accounts modified in this batch.
    /// Reads check here first, then fall through to on-disk state.
    account_overlay: std::collections::HashMap<Pubkey, Account>,
    /// In-memory overlay for stake pool (set on put, read on get)
    stake_pool_overlay: Option<crate::consensus::StakePool>,
    /// In-memory overlay for MossStake pool (set on put, read on get)
    mossstake_pool_overlay: Option<MossStakePool>,
    /// Metric deltas accumulated during the batch (applied on commit)
    new_accounts: i64,
    active_account_delta: i64,
    /// Accumulated burned amount delta (applied atomically on commit)
    burned_delta: u64,
    /// Accumulated minted amount delta (applied atomically on commit)
    minted_delta: u64,
    /// AUDIT-FIX 1.15: Track NFT token_ids indexed within this batch for TOCTOU-safe uniqueness
    nft_token_id_overlay: std::collections::HashSet<Vec<u8>>,
    /// AUDIT-FIX CP-7: Track symbols registered within this batch to catch duplicates
    symbol_overlay: std::collections::HashSet<String>,
    /// Track nullifiers marked spent inside this batch so reads are batch-consistent.
    spent_nullifier_overlay: std::collections::HashSet<[u8; 32]>,
    /// Track shielded commitments inserted inside this batch so reads and
    /// Merkle rebuilds stay batch-consistent.
    shielded_commitment_overlay: std::collections::BTreeMap<u64, [u8; 32]>,
    /// Track singleton shielded pool state updates inside this batch so
    /// repeated shielded ops see prior in-flight pool mutations.
    shielded_pool_overlay: Option<crate::zk::ShieldedPoolState>,
    /// AUDIT-FIX H-1: Governed proposal overlay so proposals participate in batch atomicity.
    governed_proposal_overlay: std::collections::HashMap<u64, crate::multisig::GovernedProposal>,
    /// AUDIT-FIX H-1: Governed proposal counter override (set on first alloc in this batch).
    governed_proposal_counter: Option<u64>,
    /// Daily governed transfer volume overlay for batch-consistent velocity limits.
    governed_transfer_volume_overlay: std::collections::HashMap<String, u64>,
    /// Governance proposal overlay so protocol-governance actions commit atomically.
    governance_proposal_overlay:
        std::collections::HashMap<u64, crate::governance::GovernanceProposal>,
    /// Governance proposal counter override (set on first alloc in this batch).
    governance_proposal_counter: Option<u64>,
    /// Pending governance parameter changes queued inside this batch.
    pending_governance_change_overlay: std::collections::HashMap<u8, u64>,
    /// Per-deployer contract deploy nonce overlay for batch-safe address
    /// allocation.
    contract_deploy_nonce_overlay: std::collections::HashMap<Pubkey, u64>,
    /// Track newly indexed programs in this batch (applied on commit)
    new_programs: i64,
    /// Auto-incrementing sequence counter for event key uniqueness (T2.13)
    event_seq: u64,
    /// Track dirty contract storage keys for incremental Merkle recomputation
    dirty_contract_keys: Vec<Vec<u8>>,
    /// Task 3.9: Slot number for archive snapshots (0 = archive disabled for this batch)
    archive_slot: u64,
    /// Reference to the DB (needed for cf_handle lookups during put)
    db: Arc<DB>,
}

#[cfg(test)]
mod tests {
    use super::storage_bootstrap::{
        archival_tuning_profile, hot_db_tuning_profile, point_lookup_tuning_profile,
        prefix_scan_tuning_profile, small_cf_tuning_profile, write_heavy_tuning_profile,
        RocksDbCompressionProfile,
    };
    use super::*;
    use crate::block::Block;
    use tempfile::tempdir;

    #[test]
    fn test_hot_db_tuning_profile_pins_wal_and_background_settings() {
        let tuning = hot_db_tuning_profile();

        assert_eq!(tuning.max_open_files, 4096);
        assert_eq!(tuning.keep_log_file_num, 5);
        assert_eq!(tuning.max_total_wal_size, 256 * 1024 * 1024);
        assert_eq!(tuning.wal_bytes_per_sync, 1024 * 1024);
        assert_eq!(tuning.bytes_per_sync, 1024 * 1024);
        assert_eq!(tuning.max_background_jobs, 4);
    }

    #[test]
    fn test_cf_tuning_profiles_pin_compaction_presets() {
        let point_lookup = point_lookup_tuning_profile(32);
        assert_eq!(point_lookup.compression, RocksDbCompressionProfile::Lz4);
        assert_eq!(point_lookup.block_size, 16 * 1024);
        assert_eq!(point_lookup.write_buffer_size, 64 * 1024 * 1024);
        assert_eq!(point_lookup.max_write_buffer_number, 3);
        assert_eq!(point_lookup.min_write_buffer_number_to_merge, 2);
        assert!(point_lookup.dynamic_level_bytes);
        assert_eq!(point_lookup.target_file_size_base, 64 * 1024 * 1024);
        assert!(point_lookup.enable_bloom_filter);
        assert!(point_lookup.cache_index_and_filter_blocks);
        assert!(point_lookup.pin_l0_filter_and_index_blocks_in_cache);

        let prefix_scan = prefix_scan_tuning_profile(32);
        assert_eq!(prefix_scan.write_buffer_size, 32 * 1024 * 1024);
        assert_eq!(prefix_scan.memtable_prefix_bloom_ratio_per_mille, 100);
        assert_eq!(prefix_scan.prefix_len, 32);

        let write_heavy = write_heavy_tuning_profile(0);
        assert_eq!(write_heavy.write_buffer_size, 128 * 1024 * 1024);
        assert_eq!(write_heavy.max_write_buffer_number, 4);
        assert_eq!(write_heavy.target_file_size_base, 128 * 1024 * 1024);
        assert!(write_heavy.dynamic_level_bytes);

        let archival = archival_tuning_profile(32);
        assert_eq!(archival.compression, RocksDbCompressionProfile::Zstd);
        assert_eq!(archival.block_size, 32 * 1024);
        assert_eq!(archival.write_buffer_size, 32 * 1024 * 1024);
        assert_eq!(archival.target_file_size_base, 128 * 1024 * 1024);

        let small_cf = small_cf_tuning_profile();
        assert_eq!(small_cf.write_buffer_size, 4 * 1024 * 1024);
        assert_eq!(small_cf.max_write_buffer_number, 2);
        assert!(!small_cf.dynamic_level_bytes);
        assert_eq!(small_cf.target_file_size_base, 0);
    }

    #[test]
    fn test_checkpoint_lifecycle_roundtrip() {
        let temp = tempdir().unwrap();
        let state_dir = temp.path().join("state");
        let checkpoint_dir = state_dir.join("checkpoints").join("slot-7");
        let state = StateStore::open(&state_dir).unwrap();

        let pk = Pubkey([0x7A; 32]);
        let acct = Account::new(3, pk);
        state.put_account(&pk, &acct).unwrap();

        let meta = state
            .create_checkpoint(checkpoint_dir.to_str().unwrap(), 7)
            .unwrap();
        assert_eq!(meta.slot, 7);
        assert_eq!(meta.total_accounts, 1);

        let checkpoints = StateStore::list_checkpoints(state_dir.to_str().unwrap());
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].0, 7);

        let latest = StateStore::latest_checkpoint(state_dir.to_str().unwrap()).unwrap();
        assert_eq!(latest.0.slot, 7);

        let checkpoint_state = StateStore::open_checkpoint(&latest.1).unwrap();
        let loaded = checkpoint_state.get_account(&pk).unwrap().unwrap();
        assert_eq!(loaded.spores, acct.spores);
        assert_eq!(loaded.owner, acct.owner);

        assert_eq!(
            StateStore::prune_checkpoints(state_dir.to_str().unwrap(), 0).unwrap(),
            1
        );
        assert!(StateStore::list_checkpoints(state_dir.to_str().unwrap()).is_empty());
    }

    #[test]
    fn test_export_import_accounts_roundtrip() {
        let source_dir = tempdir().unwrap();
        let dest_dir = tempdir().unwrap();
        let source = StateStore::open(source_dir.path()).unwrap();
        let dest = StateStore::open(dest_dir.path()).unwrap();

        let pk = Pubkey([0x5C; 32]);
        let acct = Account::new(9, pk);
        source.put_account(&pk, &acct).unwrap();

        let page = source.export_accounts_iter(0, 10).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.entries.len(), 1);
        assert!(!page.has_more);

        assert_eq!(dest.import_accounts(&page.entries).unwrap(), 1);
        let loaded = dest.get_account(&pk).unwrap().unwrap();
        assert_eq!(loaded.spores, acct.spores);
        assert_eq!(loaded.owner, acct.owner);
    }

    #[test]
    fn test_state_store() {
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();

        let pubkey = Pubkey([1u8; 32]);
        let account = Account::new(100, pubkey);

        // Store
        state.put_account(&pubkey, &account).unwrap();

        // Retrieve
        let retrieved = state.get_account(&pubkey).unwrap().unwrap();
        assert_eq!(retrieved.spores, Account::licn_to_spores(100));
    }

    #[test]
    fn test_transfer() {
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();

        let alice = Pubkey([1u8; 32]);
        let bob = Pubkey([2u8; 32]);

        // Create Alice with 1000 LICN
        let alice_account = Account::new(1000, alice);
        state.put_account(&alice, &alice_account).unwrap();

        // Transfer 100 LICN to Bob
        let spores = Account::licn_to_spores(100);
        state.transfer(&alice, &bob, spores).unwrap();

        // Check balances
        assert_eq!(
            state.get_balance(&alice).unwrap(),
            Account::licn_to_spores(900)
        );
        assert_eq!(
            state.get_balance(&bob).unwrap(),
            Account::licn_to_spores(100)
        );
    }

    #[test]
    fn test_state_root_deterministic() {
        let temp1 = tempdir().unwrap();
        let state1 = StateStore::open(temp1.path()).unwrap();

        let temp2 = tempdir().unwrap();
        let state2 = StateStore::open(temp2.path()).unwrap();

        // Same accounts in both states
        let pk_a = Pubkey([1u8; 32]);
        let pk_b = Pubkey([2u8; 32]);
        state1.put_account(&pk_a, &Account::new(100, pk_a)).unwrap();
        state1.put_account(&pk_b, &Account::new(200, pk_b)).unwrap();

        state2.put_account(&pk_a, &Account::new(100, pk_a)).unwrap();
        state2.put_account(&pk_b, &Account::new(200, pk_b)).unwrap();

        let root1 = state1.compute_state_root();
        let root2 = state2.compute_state_root();
        assert_eq!(root1, root2, "Same accounts should produce same state root");
    }

    #[test]
    fn test_state_root_changes_on_mutation() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey([1u8; 32]);
        state.put_account(&pk, &Account::new(100, pk)).unwrap();
        let root1 = state.compute_state_root();

        state.put_account(&pk, &Account::new(200, pk)).unwrap();
        let root2 = state.compute_state_root();

        assert_ne!(
            root1, root2,
            "Changed balance should produce different state root"
        );
    }

    #[test]
    fn test_invalidate_merkle_cache_forces_rebuild() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let account_pk = Pubkey([7u8; 32]);
        let contract_pk = Pubkey([8u8; 32]);
        state
            .put_account(&account_pk, &Account::new(100, account_pk))
            .unwrap();
        state
            .put_contract_storage(&contract_pk, b"vault:key", b"value")
            .unwrap();

        let initial_root = state.compute_state_root();
        let cf_stats = state.db.cf_handle(CF_STATS).expect("stats column family");
        let read_count = |key: &[u8]| -> u64 {
            state
                .db
                .get_cf(&cf_stats, key)
                .expect("read stats key")
                .map(|value| {
                    let raw: &[u8] = value.as_ref();
                    let mut bytes = [0u8; 8];
                    bytes.copy_from_slice(&raw[..8]);
                    u64::from_le_bytes(bytes)
                })
                .unwrap_or(0)
        };

        assert!(read_count(b"merkle_leaf_count") > 0);
        assert!(read_count(b"contract_merkle_leaf_count") > 0);

        state.invalidate_merkle_cache();
        assert_eq!(read_count(b"merkle_leaf_count"), 0);
        assert_eq!(read_count(b"contract_merkle_leaf_count"), 0);

        let rebuilt_root = state.compute_state_root();
        assert_eq!(rebuilt_root, initial_root);
        assert!(read_count(b"merkle_leaf_count") > 0);
        assert!(read_count(b"contract_merkle_leaf_count") > 0);
    }

    #[test]
    fn test_contract_storage_stats_track_canonical_entries_only() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let program = Pubkey([8u8; 32]);
        let other_program = Pubkey([9u8; 32]);
        state
            .put_contract_storage(&program, b"alpha", b"one")
            .unwrap();
        state
            .put_contract_storage(&program, b"beta", b"three")
            .unwrap();
        state
            .put_contract_storage(&other_program, b"gamma", b"shadow")
            .unwrap();

        let stats = state.get_contract_storage_stats(&program).unwrap();
        assert_eq!(stats.entry_count, 2);
        assert_eq!(stats.total_value_size, b"one".len() + b"three".len());
    }

    #[test]
    fn test_contract_storage_cursor_and_symbol_lookup_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let program = Pubkey([0x31; 32]);
        let entry = SymbolRegistryEntry {
            symbol: String::new(),
            program,
            owner: Pubkey([0x32; 32]),
            name: Some("Vault".to_string()),
            template: None,
            metadata: None,
            decimals: Some(9),
        };

        state.register_symbol("vault", entry).unwrap();
        state
            .put_contract_storage(&program, b"alpha", &11u64.to_le_bytes())
            .unwrap();
        state
            .put_contract_storage(&program, b"beta", b"three")
            .unwrap();

        assert_eq!(
            state.get_contract_storage(&program, b"beta").unwrap(),
            Some(b"three".to_vec())
        );
        assert_eq!(state.get_contract_storage_u64(&program, b"alpha"), 11);
        assert_eq!(
            state.get_program_storage("vault", b"beta"),
            Some(b"three".to_vec())
        );

        let first_page = state
            .get_contract_storage_entries(&program, 1, None)
            .unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].0, b"alpha".to_vec());

        let second_page = state
            .get_contract_storage_entries(&program, 10, Some(first_page[0].0.clone()))
            .unwrap();
        assert_eq!(second_page, vec![(b"beta".to_vec(), b"three".to_vec())]);

        let all_entries = state.load_contract_storage_map(&program).unwrap();
        assert_eq!(all_entries.len(), 2);

        state.delete_contract_storage(&program, b"beta").unwrap();
        assert_eq!(state.get_contract_storage(&program, b"beta").unwrap(), None);
    }

    #[test]
    fn test_contract_events_index_by_program_and_slot() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let program_a = Pubkey([0x41; 32]);
        let program_b = Pubkey([0x42; 32]);

        let event_a1 = ContractEvent {
            program: program_a,
            name: "Mint".to_string(),
            data: std::collections::HashMap::from([("amount".to_string(), "10".to_string())]),
            slot: 42,
        };
        let event_a2 = ContractEvent {
            program: program_a,
            name: "Burn".to_string(),
            data: std::collections::HashMap::new(),
            slot: 42,
        };
        let event_b = ContractEvent {
            program: program_b,
            name: "Swap".to_string(),
            data: std::collections::HashMap::new(),
            slot: 42,
        };

        state.put_contract_event(&program_a, &event_a1).unwrap();
        state.put_contract_event(&program_a, &event_a2).unwrap();
        state.put_contract_event(&program_b, &event_b).unwrap();

        let by_program = state.get_events_by_program(&program_a, 10, None).unwrap();
        assert_eq!(by_program.len(), 2);
        assert!(by_program.iter().any(|event| event.name == "Mint"));
        assert!(by_program.iter().any(|event| event.name == "Burn"));

        let by_slot = state.get_events_by_slot(42, 10).unwrap();
        assert_eq!(by_slot.len(), 3);
        assert!(by_slot
            .iter()
            .any(|event| event.program == program_b && event.name == "Swap"));

        let logs = state.get_contract_logs(&program_a, 10, None).unwrap();
        assert_eq!(logs.len(), 2);
    }

    #[test]
    fn test_token_balance_secondary_indexes_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let token_program = Pubkey([0x51; 32]);
        let holder = Pubkey([0x52; 32]);
        let expected_token_account =
            derive_solana_associated_token_address(&holder, &token_program).unwrap();

        state
            .update_token_balance(&token_program, &holder, 55)
            .unwrap();

        assert_eq!(
            state.get_token_balance(&token_program, &holder).unwrap(),
            55
        );
        assert_eq!(
            state.get_token_holders(&token_program, 10, None).unwrap(),
            vec![(holder, 55)]
        );
        assert_eq!(
            state
                .get_solana_token_accounts_by_owner(&holder, 10)
                .unwrap(),
            vec![(expected_token_account, token_program)]
        );
        assert_eq!(
            state
                .get_solana_token_account_binding(&expected_token_account)
                .unwrap(),
            Some((token_program, holder))
        );

        let ensured = state
            .ensure_solana_token_account_binding(&token_program, &holder)
            .unwrap();
        assert_eq!(ensured, expected_token_account);

        state
            .update_token_balance(&token_program, &holder, 0)
            .unwrap();
        assert_eq!(state.get_token_balance(&token_program, &holder).unwrap(), 0);
        assert!(state
            .get_token_holders(&token_program, 10, None)
            .unwrap()
            .is_empty());
        assert_eq!(
            state
                .get_solana_token_account_binding(&expected_token_account)
                .unwrap(),
            Some((token_program, holder))
        );
    }

    #[test]
    fn test_token_transfer_and_tx_slot_indexes_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let token_program = Pubkey([0x61; 32]);
        let transfer_a = TokenTransfer {
            token_program: "token-a".to_string(),
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount: 10,
            slot: 5,
            tx_hash: Some("tx-a".to_string()),
        };
        let transfer_b = TokenTransfer {
            token_program: "token-a".to_string(),
            from: "carol".to_string(),
            to: "dave".to_string(),
            amount: 20,
            slot: 7,
            tx_hash: Some("tx-b".to_string()),
        };

        state
            .put_token_transfer(&token_program, &transfer_a)
            .unwrap();
        state
            .put_token_transfer(&token_program, &transfer_b)
            .unwrap();

        let transfers = state.get_token_transfers(&token_program, 10, None).unwrap();
        assert_eq!(transfers.len(), 2);
        assert_eq!(transfers[0].slot, 7);
        assert_eq!(transfers[1].slot, 5);

        let paged = state
            .get_token_transfers(&token_program, 10, Some(7))
            .unwrap();
        assert_eq!(paged.len(), 1);
        assert_eq!(paged[0].slot, transfer_a.slot);
        assert_eq!(paged[0].amount, transfer_a.amount);
        assert_eq!(paged[0].tx_hash.as_deref(), transfer_a.tx_hash.as_deref());

        let tx_hash_a = Hash([0x71; 32]);
        let tx_hash_b = Hash([0x72; 32]);
        state.index_tx_by_slot(9, &tx_hash_a).unwrap();
        state.index_tx_by_slot(9, &tx_hash_b).unwrap();
        state.index_tx_to_slot(&tx_hash_a, 9).unwrap();
        state.index_tx_to_slot(&tx_hash_b, 9).unwrap();

        assert_eq!(
            state.get_txs_by_slot(9, 10).unwrap(),
            vec![tx_hash_a, tx_hash_b]
        );
        assert_eq!(state.get_txs_by_slot(9, 1).unwrap(), vec![tx_hash_a]);
        assert_eq!(state.get_tx_slot(&tx_hash_a).unwrap(), Some(9));
        assert_eq!(state.get_tx_slot(&tx_hash_b).unwrap(), Some(9));
        assert_eq!(state.get_tx_slot(&Hash([0x73; 32])).unwrap(), None);
    }

    #[test]
    fn test_account_tx_and_recent_index_queries_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let tracked = Pubkey([0x81; 32]);
        let other_a = Pubkey([0x82; 32]);
        let other_b = Pubkey([0x83; 32]);
        let tx_a = crate::transaction::Transaction::new(crate::transaction::Message::new(
            vec![crate::transaction::Instruction {
                program_id: Pubkey([0x84; 32]),
                accounts: vec![tracked, other_a],
                data: vec![1],
            }],
            Hash::hash(b"recent-a"),
        ));
        let tx_b = crate::transaction::Transaction::new(crate::transaction::Message::new(
            vec![crate::transaction::Instruction {
                program_id: Pubkey([0x85; 32]),
                accounts: vec![tracked, other_b],
                data: vec![2],
            }],
            Hash::hash(b"recent-b"),
        ));
        let tx_a_hash = tx_a.signature();
        let tx_b_hash = tx_b.signature();

        let block_a = crate::Block::new_with_timestamp(
            5,
            Hash::default(),
            Hash::default(),
            [0u8; 32],
            vec![tx_a],
            111,
        );
        let block_b = crate::Block::new_with_timestamp(
            7,
            block_a.hash(),
            Hash::default(),
            [0u8; 32],
            vec![tx_b],
            222,
        );

        state.put_block(&block_a).unwrap();
        state.put_block(&block_b).unwrap();

        assert_eq!(state.count_account_txs(&tracked).unwrap(), 2);
        assert_eq!(
            state.get_account_tx_signatures(&tracked, 10).unwrap(),
            vec![(tx_b_hash, 7), (tx_a_hash, 5)]
        );
        assert_eq!(
            state
                .get_account_tx_signatures_paginated(&tracked, 1, None)
                .unwrap(),
            vec![(tx_b_hash, 7)]
        );
        assert_eq!(
            state
                .get_account_tx_signatures_paginated(&tracked, 10, Some(7))
                .unwrap(),
            vec![(tx_a_hash, 5)]
        );
        assert_eq!(
            state.get_recent_txs(10, None).unwrap(),
            vec![(tx_b_hash, 7), (tx_a_hash, 5)]
        );
        assert_eq!(
            state.get_recent_txs(10, Some(7)).unwrap(),
            vec![(tx_a_hash, 5)]
        );

        let token_a = Pubkey([0x91; 32]);
        let token_b = Pubkey([0x92; 32]);
        state.update_token_balance(&token_a, &tracked, 11).unwrap();
        state.update_token_balance(&token_b, &tracked, 22).unwrap();

        assert_eq!(
            state.get_holder_token_balances(&tracked, 10).unwrap(),
            vec![(token_a, 11), (token_b, 22)]
        );
    }

    #[test]
    fn test_nft_indexes_and_token_id_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let collection = Pubkey([0x74; 32]);
        let token = Pubkey([0x75; 32]);
        let owner_a = Pubkey([0x76; 32]);
        let owner_b = Pubkey([0x77; 32]);

        state.index_nft_mint(&collection, &token, &owner_a).unwrap();
        state.index_nft_token_id(&collection, 7, &token).unwrap();

        assert_eq!(
            state.get_nft_tokens_by_owner(&owner_a, 10).unwrap(),
            vec![token]
        );
        assert!(state
            .get_nft_tokens_by_owner(&owner_b, 10)
            .unwrap()
            .is_empty());
        assert_eq!(
            state.get_nft_tokens_by_collection(&collection, 10).unwrap(),
            vec![token]
        );
        assert!(state.nft_token_id_exists(&collection, 7).unwrap());
        assert!(!state.nft_token_id_exists(&collection, 8).unwrap());

        state
            .index_nft_transfer(&collection, &token, &owner_a, &owner_b)
            .unwrap();

        assert!(state
            .get_nft_tokens_by_owner(&owner_a, 10)
            .unwrap()
            .is_empty());
        assert_eq!(
            state.get_nft_tokens_by_owner(&owner_b, 10).unwrap(),
            vec![token]
        );
        assert_eq!(
            state.get_nft_tokens_by_collection(&collection, 10).unwrap(),
            vec![token]
        );
    }

    #[test]
    fn test_program_index_and_symbol_registry_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let program_a = Pubkey([0x78; 32]);
        let program_b = Pubkey([0x79; 32]);
        let entry = SymbolRegistryEntry {
            symbol: String::new(),
            program: program_b,
            owner: Pubkey([0x7A; 32]),
            name: Some("Router".to_string()),
            template: None,
            metadata: None,
            decimals: Some(9),
        };

        state.index_program(&program_b).unwrap();
        state.index_program(&program_a).unwrap();

        let programs = state.get_programs(10).unwrap();
        assert_eq!(programs, vec![program_a, program_b]);
        assert_eq!(
            state.get_programs_paginated(10, Some(&program_a)).unwrap(),
            vec![program_b]
        );

        state.register_symbol("router", entry).unwrap();

        let by_symbol = state.get_symbol_registry("ROUTER").unwrap().unwrap();
        assert_eq!(by_symbol.symbol, "ROUTER");
        assert_eq!(by_symbol.program, program_b);

        let by_program = state
            .get_symbol_registry_by_program(&program_b)
            .unwrap()
            .unwrap();
        assert_eq!(by_program.symbol, "ROUTER");
        assert_eq!(by_program.owner, Pubkey([0x7A; 32]));

        let listed = state.get_all_symbol_registry(10).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].symbol, "ROUTER");

        let paged = state
            .get_all_symbol_registry_paginated(10, Some("router"))
            .unwrap();
        assert!(paged.is_empty());
    }

    #[test]
    fn test_program_listing_pagination_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let program_a = Pubkey([0x11; 32]);
        let program_b = Pubkey([0x12; 32]);

        state.index_program(&program_b).unwrap();
        state.index_program(&program_a).unwrap();

        let listed = state.get_all_programs(10).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].0, program_a);
        assert_eq!(listed[1].0, program_b);
        assert!(listed.iter().all(|(_, metadata)| metadata.is_null()));

        let paginated = state
            .get_all_programs_paginated(10, Some(&program_a))
            .unwrap();
        assert_eq!(paginated.len(), 1);
        assert_eq!(paginated[0].0, program_b);
        assert!(paginated[0].1.is_null());
    }

    #[test]
    fn test_set_spendable_balance_recomputes_total_balance() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let owner = Pubkey([0x21; 32]);
        let mut account = Account::new(0, owner);
        account.staked = 700;
        account.locked = 900;
        state.put_account(&owner, &account).unwrap();

        state.set_spendable_balance(&owner, 500).unwrap();

        let updated = state.get_account(&owner).unwrap().unwrap();
        assert_eq!(updated.spendable, 500);
        assert_eq!(updated.staked, 700);
        assert_eq!(updated.locked, 900);
        assert_eq!(updated.spores, 2_100);
    }

    #[test]
    fn test_metrics_accessors_and_reconciliation_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let active_owner = Pubkey([0x23; 32]);
        let inactive_owner = Pubkey([0x24; 32]);
        let program = Pubkey([0x25; 32]);

        state
            .put_account(&active_owner, &Account::new(2, active_owner))
            .unwrap();
        state
            .put_account(&inactive_owner, &Account::new(0, inactive_owner))
            .unwrap();
        state.index_program(&program).unwrap();

        assert_eq!(state.get_program_count(), 1);
        assert_eq!(state.count_accounts().unwrap(), 2);
        assert_eq!(state.count_active_accounts().unwrap(), 1);

        state.metrics.set_total_accounts(99);
        state.metrics.set_active_accounts(77);

        state.reconcile_account_count().unwrap();
        state.reconcile_active_account_count().unwrap();

        let metrics = state.get_metrics();
        assert_eq!(metrics.total_accounts, 2);
        assert_eq!(metrics.active_accounts, 1);
        assert_eq!(metrics.total_transactions, 0);
        assert_eq!(metrics.total_blocks, 0);

        drop(state);

        let reopened = StateStore::open(temp.path()).unwrap();
        let reopened_metrics = reopened.get_metrics();
        assert_eq!(reopened_metrics.total_accounts, 2);
        assert_eq!(reopened_metrics.active_accounts, 1);
        assert_eq!(reopened.get_program_count(), 1);
    }

    #[test]
    fn test_fee_config_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let config = crate::FeeConfig {
            base_fee: 5_000,
            contract_deploy_fee: 1_000_000,
            contract_upgrade_fee: 500_000,
            nft_mint_fee: 100_000,
            nft_collection_fee: 200_000,
            fee_burn_percent: 40,
            fee_producer_percent: 30,
            fee_voters_percent: 10,
            fee_treasury_percent: 10,
            fee_community_percent: 10,
            fee_exempt_contracts: Vec::new(),
        };

        state.set_fee_config_full(&config).unwrap();

        let loaded = state.get_fee_config().unwrap();
        assert_eq!(loaded.base_fee, 5_000);
        assert_eq!(loaded.fee_burn_percent, 40);
        assert_eq!(loaded.fee_producer_percent, 30);
        assert_eq!(loaded.fee_voters_percent, 10);
        assert_eq!(loaded.fee_treasury_percent, 10);
        assert_eq!(loaded.fee_community_percent, 10);
    }

    #[test]
    fn test_validator_state_roundtrip_and_pending_queue() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let validator_a = crate::consensus::ValidatorInfo::new(Pubkey([0x31; 32]), 7);
        let mut validator_b = crate::consensus::ValidatorInfo::new(Pubkey([0x32; 32]), 9);
        validator_b.stake = 55;

        state.put_validator(&validator_a).unwrap();
        assert_eq!(state.get_validator_count(), 1);

        state.put_validator(&validator_a).unwrap();
        assert_eq!(state.get_validator_count(), 1);

        state.put_validator(&validator_b).unwrap();
        assert_eq!(state.get_validator_count(), 2);
        assert_eq!(
            state
                .get_validator(&validator_b.pubkey)
                .unwrap()
                .unwrap()
                .stake,
            55
        );

        let pending_a = crate::consensus::PendingValidatorChange {
            pubkey: validator_a.pubkey,
            change_type: crate::consensus::ValidatorChangeType::Add,
            queued_at_slot: 12,
            effective_epoch: 3,
        };
        let pending_b = crate::consensus::PendingValidatorChange {
            pubkey: validator_b.pubkey,
            change_type: crate::consensus::ValidatorChangeType::StakeUpdate { new_stake: 77 },
            queued_at_slot: 14,
            effective_epoch: 3,
        };
        let other_epoch = crate::consensus::PendingValidatorChange {
            pubkey: Pubkey([0x33; 32]),
            change_type: crate::consensus::ValidatorChangeType::Remove,
            queued_at_slot: 15,
            effective_epoch: 4,
        };

        state.queue_pending_validator_change(&pending_b).unwrap();
        state.queue_pending_validator_change(&other_epoch).unwrap();
        state.queue_pending_validator_change(&pending_a).unwrap();

        let epoch_three = state.get_pending_validator_changes(3).unwrap();
        assert_eq!(epoch_three.len(), 2);
        assert_eq!(epoch_three[0].queued_at_slot, 12);
        assert_eq!(epoch_three[1].queued_at_slot, 14);
        assert_eq!(epoch_three[1].pubkey, validator_b.pubkey);

        state.clear_pending_validator_changes(3).unwrap();
        assert!(state.get_pending_validator_changes(3).unwrap().is_empty());
        assert_eq!(state.get_pending_validator_changes(4).unwrap().len(), 1);

        state.delete_validator(&validator_a.pubkey).unwrap();
        state.delete_validator(&validator_a.pubkey).unwrap();
        assert_eq!(state.get_validator_count(), 1);
        assert!(state.get_validator(&validator_a.pubkey).unwrap().is_none());
    }

    #[test]
    fn test_apply_pending_governance_changes_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        state
            .queue_governance_param_change(crate::processor::GOV_PARAM_MIN_VALIDATOR_STAKE, 123)
            .unwrap();
        state
            .queue_governance_param_change(crate::processor::GOV_PARAM_EPOCH_SLOTS, 456)
            .unwrap();

        let pending = state.get_pending_governance_changes().unwrap();
        assert_eq!(pending.len(), 2);
        assert!(pending.contains(&(crate::processor::GOV_PARAM_MIN_VALIDATOR_STAKE, 123)));
        assert!(pending.contains(&(crate::processor::GOV_PARAM_EPOCH_SLOTS, 456)));

        assert_eq!(state.apply_pending_governance_changes().unwrap(), 2);
        assert_eq!(state.get_min_validator_stake().unwrap(), Some(123));
        assert_eq!(state.get_epoch_slots().unwrap(), Some(456));
        assert!(state.get_pending_governance_changes().unwrap().is_empty());
    }

    #[test]
    fn test_oracle_attestations_and_consensus_price_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let fresh_validator = Pubkey([0x55; 32]);
        let stale_validator = Pubkey([0x56; 32]);

        state
            .put_oracle_attestation("LICN", &fresh_validator, 12_345, 6, 1_000, 95)
            .unwrap();
        state
            .put_oracle_attestation("LICN", &stale_validator, 99_999, 6, 900, 80)
            .unwrap();

        let attestations = state.get_oracle_attestations("LICN", 100, 10).unwrap();
        assert_eq!(attestations.len(), 1);
        assert_eq!(attestations[0].validator, fresh_validator);
        assert_eq!(attestations[0].price, 12_345);
        assert_eq!(attestations[0].decimals, 6);

        state
            .put_oracle_consensus_price("LICN", 12_400, 6, 100, 3)
            .unwrap();

        let consensus = state.get_oracle_consensus_price("LICN").unwrap().unwrap();
        assert_eq!(consensus.asset, "LICN");
        assert_eq!(consensus.price, 12_400);
        assert_eq!(consensus.decimals, 6);
        assert_eq!(consensus.slot, 100);
        assert_eq!(consensus.attestation_count, 3);
    }

    #[test]
    fn test_recent_blockhashes() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Store a few blocks
        let h1 = Hash::hash(b"block1");
        let _h2 = Hash::hash(b"block2");
        let block1 = crate::Block::new_with_timestamp(
            1,
            Hash::default(),
            Hash::default(),
            [0u8; 32],
            vec![],
            100,
        );
        let block2 =
            crate::Block::new_with_timestamp(2, h1, Hash::default(), [0u8; 32], vec![], 200);

        state.put_block(&block1).unwrap();
        state.put_block(&block2).unwrap();
        state.set_last_slot(2).unwrap();

        let recent = state.get_recent_blockhashes(10).unwrap();
        // Should contain the block hashes we stored (not Hash::default() anymore — T1.3)
        assert!(
            recent.len() >= 2,
            "Should contain at least the 2 stored block hashes"
        );
        assert!(
            recent.contains(&block1.hash()),
            "Should contain block1's hash"
        );
        assert!(
            recent.contains(&block2.hash()),
            "Should contain block2's hash"
        );
    }

    #[test]
    fn test_tx_meta_roundtrip_supports_legacy_and_full_formats() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let legacy_sig = Hash([0x41; 32]);
        state.put_tx_meta(&legacy_sig, 321).unwrap();

        assert_eq!(state.get_tx_meta_cu(&legacy_sig).unwrap(), Some(321));
        let legacy_full = state.get_tx_meta_full(&legacy_sig).unwrap().unwrap();
        assert_eq!(legacy_full.compute_units_used, 321);
        assert_eq!(legacy_full.return_code, None);
        assert!(legacy_full.return_data.is_empty());
        assert!(legacy_full.logs.is_empty());

        let full_sig = Hash([0x42; 32]);
        let full_meta = crate::processor::TxMeta {
            compute_units_used: 654,
            return_code: Some(7),
            return_data: vec![1, 2, 3],
            logs: vec!["first".to_string(), "second".to_string()],
        };
        state.put_tx_meta_full(&full_sig, &full_meta).unwrap();

        assert_eq!(state.get_tx_meta_cu(&full_sig).unwrap(), Some(654));
        let loaded_full = state.get_tx_meta_full(&full_sig).unwrap().unwrap();
        assert_eq!(loaded_full.compute_units_used, full_meta.compute_units_used);
        assert_eq!(loaded_full.return_code, full_meta.return_code);
        assert_eq!(loaded_full.return_data, full_meta.return_data);
        assert_eq!(loaded_full.logs, full_meta.logs);
    }

    #[test]
    fn test_batch_transaction_and_tx_meta_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let instruction = crate::transaction::Instruction {
            program_id: Pubkey([0x61; 32]),
            accounts: vec![Pubkey([0x62; 32]), Pubkey([0x63; 32])],
            data: vec![4, 5, 6],
        };
        let tx = crate::transaction::Transaction::new(crate::transaction::Message::new(
            vec![instruction],
            Hash::hash(b"batch-tx-meta"),
        ));
        let sig = tx.signature();
        let meta = crate::processor::TxMeta {
            compute_units_used: 777,
            return_code: Some(12),
            return_data: vec![7, 7, 7],
            logs: vec!["batched".to_string()],
        };

        let mut batch = state.begin_batch();
        batch.put_transaction(&tx).unwrap();
        batch.put_tx_meta_full(&sig, &meta).unwrap();

        assert!(state.get_transaction(&sig).unwrap().is_none());
        assert!(state.get_tx_meta_full(&sig).unwrap().is_none());

        state.commit_batch(batch).unwrap();

        let stored_tx = state.get_transaction(&sig).unwrap().unwrap();
        assert_eq!(stored_tx.signature(), sig);
        assert_eq!(
            state.get_tx_meta_cu(&sig).unwrap(),
            Some(meta.compute_units_used)
        );

        let stored_meta = state.get_tx_meta_full(&sig).unwrap().unwrap();
        assert_eq!(stored_meta.compute_units_used, meta.compute_units_used);
        assert_eq!(stored_meta.return_code, meta.return_code);
        assert_eq!(stored_meta.return_data, meta.return_data);
        assert_eq!(stored_meta.logs, meta.logs);
    }

    // ── H3 tests: StateBatch::apply_evm_changes ──

    #[test]
    fn test_apply_evm_changes_writes_account_and_storage() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let changes = vec![crate::evm::EvmStateChange {
            evm_address: [0xAA; 20],
            account: Some(crate::evm::EvmAccount {
                nonce: 5,
                balance: [0u8; 32],
                code: vec![0xAB, 0xCD],
            }),
            storage_changes: vec![
                ([0x01; 32], Some(alloy_primitives::U256::from(42u64))),
                ([0x02; 32], Some(alloy_primitives::U256::from(99u64))),
            ],
            native_balance_update: None,
        }];

        let mut batch = state.begin_batch();
        batch.apply_evm_changes(&changes).unwrap();
        state.commit_batch(batch).unwrap();

        // Verify the EVM account was written
        let stored = state.get_evm_account(&[0xAA; 20]).unwrap();
        assert!(stored.is_some());
        let acct = stored.unwrap();
        assert_eq!(acct.nonce, 5);
        assert_eq!(acct.code, vec![0xABu8, 0xCD]);

        // Verify storage (returns U256::ZERO for missing, non-zero for present)
        let val1 = state.get_evm_storage(&[0xAA; 20], &[0x01; 32]).unwrap();
        assert_ne!(val1, alloy_primitives::U256::ZERO);
        let val2 = state.get_evm_storage(&[0xAA; 20], &[0x02; 32]).unwrap();
        assert_ne!(val2, alloy_primitives::U256::ZERO);
    }

    #[test]
    fn test_apply_evm_changes_clears_account() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // First write an account
        let create = vec![crate::evm::EvmStateChange {
            evm_address: [0xBB; 20],
            account: Some(crate::evm::EvmAccount {
                nonce: 1,
                balance: [0u8; 32],
                code: vec![],
            }),
            storage_changes: vec![([0x01; 32], Some(alloy_primitives::U256::from(10u64)))],
            native_balance_update: None,
        }];
        let mut batch = state.begin_batch();
        batch.apply_evm_changes(&create).unwrap();
        state.commit_batch(batch).unwrap();
        assert!(state.get_evm_account(&[0xBB; 20]).unwrap().is_some());

        // Now clear it (account = None → self-destruct)
        let clear = vec![crate::evm::EvmStateChange {
            evm_address: [0xBB; 20],
            account: None,
            storage_changes: vec![],
            native_balance_update: None,
        }];
        let mut batch2 = state.begin_batch();
        batch2.apply_evm_changes(&clear).unwrap();
        state.commit_batch(batch2).unwrap();

        // Account and storage should be gone
        assert!(state.get_evm_account(&[0xBB; 20]).unwrap().is_none());
        assert_eq!(
            state.get_evm_storage(&[0xBB; 20], &[0x01; 32]).unwrap(),
            alloy_primitives::U256::ZERO
        );
    }

    #[test]
    fn test_apply_evm_changes_native_balance_update() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey([0x77; 32]);
        state.put_account(&pk, &Account::new(100, pk)).unwrap();

        let new_spendable = 500_000_000u64; // 0.5 LICN in spores
        let changes = vec![crate::evm::EvmStateChange {
            evm_address: [0xCC; 20],
            account: Some(crate::evm::EvmAccount {
                nonce: 0,
                balance: [0u8; 32],
                code: vec![],
            }),
            storage_changes: vec![],
            native_balance_update: Some((pk, new_spendable)),
        }];

        let mut batch = state.begin_batch();
        batch.apply_evm_changes(&changes).unwrap();
        state.commit_batch(batch).unwrap();

        let acct = state.get_account(&pk).unwrap().unwrap();
        assert_eq!(acct.spendable, new_spendable);
    }

    /// AUDIT-FIX C-1: prune_slot_stats correctly handles dirty_acct keys
    /// whose format is "dirty_acct:{pubkey}" (43 bytes, no slot).
    /// Pruning must not corrupt state by misinterpreting pubkey bytes as slots.
    #[test]
    fn test_prune_dirty_acct_correct_format() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Write some dirty_acct markers
        let pk1 = Pubkey([0xAA; 32]);
        let pk2 = Pubkey([0xBB; 32]);
        state.mark_account_dirty_with_key(&pk1);
        state.mark_account_dirty_with_key(&pk2);

        // Verify they exist
        let cf = state.db.cf_handle(CF_STATS).unwrap();
        let mut key1 = [0u8; 43];
        key1[..11].copy_from_slice(b"dirty_acct:");
        key1[11..43].copy_from_slice(&pk1.0);
        assert!(state.db.get_cf(&cf, key1).unwrap().is_some());

        // Prune with a high current_slot (should clean all dirty markers)
        let deleted = state.prune_slot_stats(10000, 100).unwrap();
        assert!(
            deleted >= 2,
            "Should have pruned at least 2 dirty_acct keys, got {}",
            deleted
        );

        // Dirty markers should be gone
        assert!(state.db.get_cf(&cf, key1).unwrap().is_none());
    }

    #[test]
    fn test_evm_address_mapping_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let evm_address =
            StateStore::parse_evm_address("0x1111111111111111111111111111111111111111").unwrap();
        let native_pubkey = Pubkey([0x44; 32]);

        state
            .register_evm_address(&evm_address, &native_pubkey)
            .unwrap();

        assert_eq!(
            state.lookup_evm_address(&evm_address).unwrap(),
            Some(native_pubkey)
        );
        assert_eq!(
            state.lookup_native_to_evm(&native_pubkey).unwrap(),
            Some(evm_address)
        );
    }

    #[test]
    fn test_evm_state_roundtrip_and_clear_paths() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let evm_address = [0x22; 20];
        let storage_slot_a = [0x01; 32];
        let storage_slot_b = [0x02; 32];
        let tx_hash = [0x33; 32];
        let block_hash = Hash([0x55; 32]);

        let mut account = EvmAccount::new();
        account.nonce = 7;
        account.code = vec![0xAA, 0xBB];
        account.set_balance_u256(U256::from(123u64));

        state.put_evm_account(&evm_address, &account).unwrap();
        state
            .put_evm_storage(&evm_address, &storage_slot_a, U256::from(41u64))
            .unwrap();
        state
            .put_evm_storage(&evm_address, &storage_slot_b, U256::from(99u64))
            .unwrap();

        state
            .put_evm_tx(&EvmTxRecord {
                evm_hash: tx_hash,
                native_hash: [0x34; 32],
                from: evm_address,
                to: Some([0x35; 20]),
                value: U256::from(5u64).to_be_bytes::<32>(),
                gas_limit: 21_000,
                gas_price: U256::from(3u64).to_be_bytes::<32>(),
                nonce: 9,
                data: vec![0x01, 0x02],
                status: Some(true),
                gas_used: Some(21_000),
                block_slot: None,
                block_hash: None,
            })
            .unwrap();
        state
            .mark_evm_tx_included(&tx_hash, 77, &block_hash)
            .unwrap();

        state
            .put_evm_receipt(&EvmReceipt {
                evm_hash: tx_hash,
                status: true,
                gas_used: 21_000,
                block_slot: Some(77),
                block_hash: Some(block_hash.0),
                contract_address: Some([0x36; 20]),
                logs: vec![vec![0x99]],
                structured_logs: Vec::new(),
            })
            .unwrap();

        let stored_account = state.get_evm_account(&evm_address).unwrap().unwrap();
        assert_eq!(stored_account.nonce, 7);
        assert_eq!(stored_account.code, vec![0xAA, 0xBB]);
        assert_eq!(stored_account.balance_u256(), U256::from(123u64));

        assert_eq!(
            state
                .get_evm_storage(&evm_address, &storage_slot_a)
                .unwrap(),
            U256::from(41u64)
        );
        assert_eq!(
            state
                .get_evm_storage(&evm_address, &storage_slot_b)
                .unwrap(),
            U256::from(99u64)
        );

        let stored_tx = state.get_evm_tx(&tx_hash).unwrap().unwrap();
        assert_eq!(stored_tx.block_slot, Some(77));
        assert_eq!(stored_tx.block_hash, Some(block_hash.0));
        assert_eq!(stored_tx.status, Some(true));

        let stored_receipt = state.get_evm_receipt(&tx_hash).unwrap().unwrap();
        assert!(stored_receipt.status);
        assert_eq!(stored_receipt.block_slot, Some(77));
        assert_eq!(stored_receipt.block_hash, Some(block_hash.0));

        state
            .clear_evm_storage_slot(&evm_address, &storage_slot_a)
            .unwrap();
        assert_eq!(
            state
                .get_evm_storage(&evm_address, &storage_slot_a)
                .unwrap(),
            U256::ZERO
        );

        state.clear_evm_storage(&evm_address).unwrap();
        assert_eq!(
            state
                .get_evm_storage(&evm_address, &storage_slot_b)
                .unwrap(),
            U256::ZERO
        );

        state.clear_evm_account(&evm_address).unwrap();
        assert!(state.get_evm_account(&evm_address).unwrap().is_none());
    }

    /// AUDIT-FIX C-2: dirty_account_count is only reset when dirty keys
    /// were actually pruned, and new writes after pruning re-set the flag.
    #[test]
    fn test_prune_dirty_count_not_unconditional_reset() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Create a fee_dist entry so pruning has something to delete even
        // without dirty_acct keys
        let cf = state.db.cf_handle(CF_STATS).unwrap();
        let _ = state.db.put_cf(&cf, b"fee_dist:1", b"data");

        // Set dirty_account_count to 1 (simulating a concurrent write)
        let _ = state
            .db
            .put_cf(&cf, b"dirty_account_count", 1u64.to_le_bytes());

        // Prune — should delete fee_dist:1 but NOT reset dirty_account_count
        // because no dirty_acct keys were pruned
        let _ = state.prune_slot_stats(10000, 100).unwrap();

        // dirty_account_count should still be 1 (not reset to 0)
        let val = state
            .db
            .get_cf(&cf, b"dirty_account_count")
            .unwrap()
            .map(|v| u64::from_le_bytes(v.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);
        assert_eq!(
            val, 1,
            "dirty_account_count must not be reset when no dirty_acct keys were pruned"
        );
    }

    /// AUDIT-FIX C-3: commit_batch holds burned_lock during RMW to prevent
    /// concurrent add_burned() from losing updates.
    #[test]
    fn test_commit_batch_burned_lock_serializes() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Direct add_burned to set baseline
        state.add_burned(100).unwrap();
        assert_eq!(state.get_total_burned().unwrap(), 100);

        // Now commit a batch with burned_delta = 50
        let mut batch = state.begin_batch();
        batch.add_burned(50);
        state.commit_batch(batch).unwrap();

        // Total should be 150, not 50 (which would happen if lock was missing
        // and the batch read a stale value overwriting the direct add)
        assert_eq!(state.get_total_burned().unwrap(), 150);

        // And another direct add should also serialize
        state.add_burned(25).unwrap();
        assert_eq!(state.get_total_burned().unwrap(), 175);
    }

    /// AUDIT-FIX C-4: atomic_put_accounts holds burned_lock during RMW to
    /// prevent lost updates to the burned counter.
    #[test]
    fn test_atomic_put_accounts_burned_lock_serializes() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Set baseline
        state.add_burned(200).unwrap();

        // Put accounts with a burn_delta
        let pk = Pubkey([0xCC; 32]);
        let acct = Account::new(10, pk); // 10 LICN
        state.atomic_put_accounts(&[(&pk, &acct)], 80).unwrap();

        // Total burned should be 280, not 80
        assert_eq!(state.get_total_burned().unwrap(), 280);

        // Verify account was also written
        let loaded = state.get_account(&pk).unwrap().unwrap();
        assert_eq!(loaded.spores, 10_000_000_000);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // TOKENOMICS OVERHAUL: All 6 wallet pubkey accessors
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_all_wallet_pubkeys_stored_and_retrievable() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Simulate genesis: store all 6 wallet entries
        let wallets: Vec<(String, Pubkey, u64, u8)> = vec![
            (
                "validator_rewards".into(),
                Pubkey([0x01; 32]),
                100_000_000,
                10,
            ),
            (
                "community_treasury".into(),
                Pubkey([0x02; 32]),
                250_000_000,
                25,
            ),
            ("builder_grants".into(), Pubkey([0x03; 32]), 350_000_000, 35),
            (
                "founding_symbionts".into(),
                Pubkey([0x04; 32]),
                100_000_000,
                10,
            ),
            (
                "ecosystem_partnerships".into(),
                Pubkey([0x05; 32]),
                100_000_000,
                10,
            ),
            ("reserve_pool".into(), Pubkey([0x06; 32]), 100_000_000, 10),
        ];
        state.set_genesis_accounts(&wallets).unwrap();

        // Also set treasury_pubkey (legacy path)
        state.set_treasury_pubkey(&Pubkey([0x01; 32])).unwrap();

        // Verify treasury (legacy path)
        let treasury = state.get_treasury_pubkey().unwrap();
        assert_eq!(treasury, Some(Pubkey([0x01; 32])));

        // Verify all 6 wallet role-based accessors
        assert_eq!(
            state.get_wallet_pubkey("validator_rewards").unwrap(),
            Some(Pubkey([0x01; 32]))
        );
        assert_eq!(
            state.get_community_treasury_pubkey().unwrap(),
            Some(Pubkey([0x02; 32]))
        );
        assert_eq!(
            state.get_builder_grants_pubkey().unwrap(),
            Some(Pubkey([0x03; 32]))
        );
        assert_eq!(
            state.get_founding_symbionts_pubkey().unwrap(),
            Some(Pubkey([0x04; 32]))
        );
        assert_eq!(
            state.get_ecosystem_partnerships_pubkey().unwrap(),
            Some(Pubkey([0x05; 32]))
        );
        assert_eq!(
            state.get_reserve_pool_pubkey().unwrap(),
            Some(Pubkey([0x06; 32]))
        );

        // Unknown role returns None
        assert_eq!(state.get_wallet_pubkey("nonexistent").unwrap(), None);

        // Verify count and ordering via get_genesis_accounts
        let loaded = state.get_genesis_accounts().unwrap();
        assert_eq!(loaded.len(), 6);
        let total: u64 = loaded.iter().map(|(_, _, amt, _)| amt).sum();
        assert_eq!(total, 1_000_000_000, "All 6 wallets must sum to 1B LICN");
    }

    #[test]
    fn test_dao_treasury_wired_to_community_treasury() {
        // Verify that community_treasury pubkey is fetchable and distinct,
        // confirming it can be used as the DAO treasury address at genesis.
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let community_pk = Pubkey([0xCC; 32]);
        let validator_rewards_pk = Pubkey([0xAA; 32]);

        // Store genesis accounts with community_treasury (4th element is percentage u8)
        let accounts: Vec<(String, Pubkey, u64, u8)> = vec![
            (
                "validator_rewards".to_string(),
                validator_rewards_pk,
                100_000_000,
                10,
            ),
            (
                "community_treasury".to_string(),
                community_pk,
                250_000_000,
                25,
            ),
        ];
        state.set_genesis_accounts(&accounts).unwrap();
        state.set_treasury_pubkey(&validator_rewards_pk).unwrap();

        // DAO should use community_treasury, NOT validator_rewards
        let dao_treasury = state
            .get_community_treasury_pubkey()
            .unwrap()
            .expect("community_treasury must be set");
        assert_eq!(
            dao_treasury, community_pk,
            "DAO treasury must be community_treasury wallet"
        );
        assert_ne!(
            dao_treasury, validator_rewards_pk,
            "DAO treasury must NOT be validator_rewards"
        );
    }

    // ─── Shielded pool state tests ──────────────────────────────────

    #[test]
    fn test_shielded_commitment_insert_and_get() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let commitment = [0xABu8; 32];
        state.insert_shielded_commitment(0, &commitment).unwrap();

        let retrieved = state.get_shielded_commitment(0).unwrap();
        assert_eq!(retrieved, Some(commitment));

        // Non-existent index
        assert_eq!(state.get_shielded_commitment(1).unwrap(), None);
    }

    #[test]
    fn test_shielded_commitment_multiple() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        for i in 0u64..5 {
            let mut c = [0u8; 32];
            c[0] = i as u8;
            state.insert_shielded_commitment(i, &c).unwrap();
        }

        let all = state.get_all_shielded_commitments(5).unwrap();
        assert_eq!(all.len(), 5);
        for (i, entry) in all.iter().enumerate() {
            assert_eq!(entry[0], i as u8);
        }
    }

    #[test]
    fn test_nullifier_spent_tracking() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let nullifier = [0xFFu8; 32];

        assert!(!state.is_nullifier_spent(&nullifier).unwrap());
        state.mark_nullifier_spent(&nullifier).unwrap();
        assert!(state.is_nullifier_spent(&nullifier).unwrap());

        // Different nullifier is not spent
        let other = [0x01u8; 32];
        assert!(!state.is_nullifier_spent(&other).unwrap());
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shielded_pool_state_default() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pool = state.get_shielded_pool_state().unwrap();
        assert_eq!(pool.commitment_count, 0);
        assert_eq!(pool.total_shielded, 0);
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shielded_pool_state_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let mut pool = crate::zk::ShieldedPoolState::new();
        pool.commitment_count = 42;
        pool.total_shielded = 1_000_000;
        pool.merkle_root = [0xEE; 32];

        state.put_shielded_pool_state(&pool).unwrap();
        let loaded = state.get_shielded_pool_state().unwrap();

        assert_eq!(loaded.commitment_count, 42);
        assert_eq!(loaded.total_shielded, 1_000_000);
        assert_eq!(loaded.merkle_root, [0xEE; 32]);
    }

    #[test]
    fn test_shielded_batch_commitment_and_nullifier() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let mut batch = state.begin_batch();

        // Insert commitment via batch
        let commitment = [0xBBu8; 32];
        batch.insert_shielded_commitment(0, &commitment).unwrap();

        // Mark nullifier via batch
        let nullifier = [0xCCu8; 32];
        batch.mark_nullifier_spent(&nullifier).unwrap();

        // Batch view must see in-flight nullifier spend immediately
        assert!(batch.is_nullifier_spent(&nullifier).unwrap());

        // Before commit, disk has nothing
        assert_eq!(state.get_shielded_commitment(0).unwrap(), None);
        assert!(!state.is_nullifier_spent(&nullifier).unwrap());

        // Commit the batch
        state.commit_batch(batch).unwrap();

        // Now disk has the data
        assert_eq!(state.get_shielded_commitment(0).unwrap(), Some(commitment));
        assert!(state.is_nullifier_spent(&nullifier).unwrap());
    }

    #[test]
    fn test_shielded_batch_get_all_commitments_includes_overlay() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let committed = [0x11u8; 32];
        state.insert_shielded_commitment(0, &committed).unwrap();

        let mut batch = state.begin_batch();
        let pending = [0x22u8; 32];
        batch.insert_shielded_commitment(1, &pending).unwrap();

        let leaves = batch.get_all_shielded_commitments(2).unwrap();
        assert_eq!(leaves, vec![committed, pending]);
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shielded_batch_pool_state() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let mut batch = state.begin_batch();

        let mut pool = batch.get_shielded_pool_state().unwrap();
        pool.commitment_count = 10;
        pool.total_shielded = 5_000;
        batch.put_shielded_pool_state(&pool).unwrap();

        // Commit
        state.commit_batch(batch).unwrap();

        let loaded = state.get_shielded_pool_state().unwrap();
        assert_eq!(loaded.commitment_count, 10);
        assert_eq!(loaded.total_shielded, 5_000);
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shielded_batch_pool_state_reads_overlay() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let mut batch = state.begin_batch();
        let mut pool = batch.get_shielded_pool_state().unwrap();
        pool.commitment_count = 2;
        pool.total_shielded = 300;
        batch.put_shielded_pool_state(&pool).unwrap();

        let reread = batch.get_shielded_pool_state().unwrap();
        assert_eq!(reread.commitment_count, 2);
        assert_eq!(reread.total_shielded, 300);
    }

    // ── P2-3: Cold storage tests ──

    fn make_test_block(slot: u64) -> Block {
        Block::new(
            slot,
            Hash::default(),
            Hash::default(),
            [0u8; 32],
            Vec::new(),
        )
    }

    #[test]
    fn test_cold_storage_open_and_attach() {
        let hot_dir = tempdir().unwrap();
        let cold_dir = tempdir().unwrap();
        let mut state = StateStore::open(hot_dir.path()).unwrap();
        assert!(!state.has_cold_storage());

        state.open_cold_store(cold_dir.path()).unwrap();
        assert!(state.has_cold_storage());
    }

    #[test]
    fn test_put_block_atomic_persists_slot_and_finality_metadata() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let block = make_test_block(7);
        state.put_block_atomic(&block, Some(7), Some(7)).unwrap();

        assert_eq!(state.get_last_slot().unwrap(), 7);
        assert_eq!(state.get_last_confirmed_slot().unwrap(), 7);
        assert_eq!(state.get_last_finalized_slot().unwrap(), 7);
        assert_eq!(state.get_block_by_slot(7).unwrap().unwrap().header.slot, 7);
    }

    #[test]
    fn test_put_block_atomic_does_not_regress_tip_metadata() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let newer_block = make_test_block(11);
        state
            .put_block_atomic(&newer_block, Some(11), Some(11))
            .unwrap();

        let older_block = make_test_block(10);
        state
            .put_block_atomic(&older_block, Some(10), Some(10))
            .unwrap();

        assert_eq!(state.get_last_slot().unwrap(), 11);
        assert_eq!(state.get_last_confirmed_slot().unwrap(), 11);
        assert_eq!(state.get_last_finalized_slot().unwrap(), 11);
        assert_eq!(
            state.get_block_by_slot(10).unwrap().unwrap().header.slot,
            10
        );
        assert_eq!(
            state.get_block_by_slot(11).unwrap().unwrap().header.slot,
            11
        );
    }

    #[test]
    fn test_put_block_atomic_does_not_persist_tx_slot_seq_side_counter() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let instruction = crate::transaction::Instruction {
            program_id: Pubkey([9u8; 32]),
            accounts: vec![Pubkey([1u8; 32]), Pubkey([2u8; 32])],
            data: vec![1, 2, 3],
        };
        let message = crate::transaction::Message::new(vec![instruction], Hash::hash(b"recent"));
        let tx = crate::transaction::Transaction::new(message);
        let tx_hash = tx.signature();
        let block = crate::Block::new_with_timestamp(
            8,
            Hash::default(),
            Hash::default(),
            [0u8; 32],
            vec![tx],
            123,
        );

        state.put_block_atomic(&block, Some(8), Some(8)).unwrap();

        let cf_stats = state.db.cf_handle(CF_STATS).unwrap();
        let mut counter_key = Vec::with_capacity(12);
        counter_key.extend_from_slice(b"txs:");
        counter_key.extend_from_slice(&8u64.to_be_bytes());
        assert!(state.db.get_cf(&cf_stats, &counter_key).unwrap().is_none());

        let cf_tx_by_slot = state.db.cf_handle(CF_TX_BY_SLOT).unwrap();
        let mut first_tx_key = Vec::with_capacity(16);
        first_tx_key.extend_from_slice(&8u64.to_be_bytes());
        first_tx_key.extend_from_slice(&0u64.to_be_bytes());
        assert_eq!(
            state
                .db
                .get_cf(&cf_tx_by_slot, &first_tx_key)
                .unwrap()
                .unwrap(),
            tx_hash.0.to_vec()
        );

        let mut second_tx_key = Vec::with_capacity(16);
        second_tx_key.extend_from_slice(&8u64.to_be_bytes());
        second_tx_key.extend_from_slice(&1u64.to_be_bytes());
        assert!(state
            .db
            .get_cf(&cf_tx_by_slot, &second_tx_key)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_cold_storage_migrate_and_fallthrough() {
        let hot_dir = tempdir().unwrap();
        let cold_dir = tempdir().unwrap();
        let mut state = StateStore::open(hot_dir.path()).unwrap();
        state.open_cold_store(cold_dir.path()).unwrap();

        // Store blocks at slots 0..10
        for slot in 0..10u64 {
            let block = make_test_block(slot);
            state.put_block(&block).unwrap();
        }

        // All blocks readable from hot
        for slot in 0..10u64 {
            assert!(state.get_block_by_slot(slot).unwrap().is_some());
        }

        // Migrate blocks older than slot 5
        let migrated = state.migrate_to_cold(5).unwrap();
        assert_eq!(migrated, 5);

        // Slots 0..5 are now only in cold (fall-through read)
        for slot in 0..5u64 {
            let block = state.get_block_by_slot(slot).unwrap();
            assert!(block.is_some(), "slot {} should fall through to cold", slot);
            assert_eq!(block.unwrap().header.slot, slot);
        }

        // Slots 5..10 remain in hot
        for slot in 5..10u64 {
            let block = state.get_block_by_slot(slot).unwrap();
            assert!(block.is_some(), "slot {} should still be in hot", slot);
        }
    }

    #[test]
    fn test_cold_migration_without_cold_db_errors() {
        let hot_dir = tempdir().unwrap();
        let state = StateStore::open(hot_dir.path()).unwrap();
        let result = state.migrate_to_cold(100);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not attached"));
    }

    #[test]
    fn test_cold_migration_nothing_to_migrate() {
        let hot_dir = tempdir().unwrap();
        let cold_dir = tempdir().unwrap();
        let mut state = StateStore::open(hot_dir.path()).unwrap();
        state.open_cold_store(cold_dir.path()).unwrap();

        // No blocks stored — nothing to migrate
        let migrated = state.migrate_to_cold(100).unwrap();
        assert_eq!(migrated, 0);
    }

    #[test]
    fn test_cold_migration_idempotent() {
        let hot_dir = tempdir().unwrap();
        let cold_dir = tempdir().unwrap();
        let mut state = StateStore::open(hot_dir.path()).unwrap();
        state.open_cold_store(cold_dir.path()).unwrap();

        for slot in 0..5u64 {
            state.put_block(&make_test_block(slot)).unwrap();
        }

        // First migration moves 3 blocks
        let migrated1 = state.migrate_to_cold(3).unwrap();
        assert_eq!(migrated1, 3);

        // Second migration with same cutoff: nothing to move (already in cold)
        let migrated2 = state.migrate_to_cold(3).unwrap();
        assert_eq!(migrated2, 0);

        // All blocks still readable
        for slot in 0..5u64 {
            assert!(state.get_block_by_slot(slot).unwrap().is_some());
        }
    }

    #[test]
    fn test_cold_index_migration_moves_token_transfers() {
        let hot_dir = tempdir().unwrap();
        let cold_dir = tempdir().unwrap();
        let mut state = StateStore::open(hot_dir.path()).unwrap();
        state.open_cold_store(cold_dir.path()).unwrap();

        let token_program = Pubkey([0x31; 32]);
        let transfer = TokenTransfer {
            token_program: "token-a".to_string(),
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount: 42,
            slot: 4,
            tx_hash: Some("tx-a".to_string()),
        };
        state.put_token_transfer(&token_program, &transfer).unwrap();

        let hot_cf = state.db.cf_handle(CF_TOKEN_TRANSFERS).unwrap();
        let hot_before = state
            .db
            .iterator_cf(&hot_cf, rocksdb::IteratorMode::Start)
            .flatten()
            .count();
        assert_eq!(hot_before, 1);

        let migrated = state.migrate_indexes_to_cold(5).unwrap();
        assert_eq!(migrated, 1);

        let hot_after = state
            .db
            .iterator_cf(&hot_cf, rocksdb::IteratorMode::Start)
            .flatten()
            .count();
        assert_eq!(hot_after, 0);

        let cold = state.cold_db.as_ref().unwrap();
        let cold_cf = cold.cf_handle(COLD_CF_TOKEN_TRANSFERS).unwrap();
        let cold_entries: Vec<_> = cold
            .iterator_cf(&cold_cf, rocksdb::IteratorMode::Start)
            .flatten()
            .collect();
        assert_eq!(cold_entries.len(), 1);

        let stored: TokenTransfer = serde_json::from_slice(&cold_entries[0].1).unwrap();
        assert_eq!(stored.slot, transfer.slot);
        assert_eq!(stored.amount, transfer.amount);
        assert_eq!(stored.tx_hash.as_deref(), transfer.tx_hash.as_deref());
    }

    #[test]
    fn test_cold_clone_shares_cold_db() {
        let hot_dir = tempdir().unwrap();
        let cold_dir = tempdir().unwrap();
        let mut state = StateStore::open(hot_dir.path()).unwrap();
        state.open_cold_store(cold_dir.path()).unwrap();

        // Store and migrate a block
        state.put_block(&make_test_block(0)).unwrap();
        state.migrate_to_cold(1).unwrap();

        // Clone should share the same cold DB
        let cloned = state.clone();
        assert!(cloned.has_cold_storage());
        let block = cloned.get_block_by_slot(0).unwrap();
        assert!(block.is_some(), "clone should read from shared cold DB");
    }

    // ─── Merkle proof tests (Task 1.3) ──────────────────────────────

    #[test]
    fn test_build_merkle_tree_empty() {
        let tree = build_merkle_tree(&[]);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0][0], Hash::default());
    }

    #[test]
    fn test_build_merkle_tree_single_leaf() {
        let leaf = Hash::hash(b"single");
        let tree = build_merkle_tree(&[leaf]);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0][0], leaf);
    }

    #[test]
    fn test_build_merkle_tree_two_leaves() {
        let a = Hash::hash(b"a");
        let b = Hash::hash(b"b");
        let tree = build_merkle_tree(&[a, b]);
        assert_eq!(tree.len(), 2); // leaves + root
        assert_eq!(tree[0].len(), 2);
        assert_eq!(tree[1].len(), 1);
        // Root = H(a || b)
        let mut combined = [0u8; 64];
        combined[..32].copy_from_slice(&a.0);
        combined[32..].copy_from_slice(&b.0);
        assert_eq!(tree[1][0], Hash::hash(&combined));
    }

    #[test]
    fn test_build_merkle_tree_three_leaves_odd() {
        let a = Hash::hash(b"a");
        let b = Hash::hash(b"b");
        let c = Hash::hash(b"c");
        let tree = build_merkle_tree(&[a, b, c]);
        // Level 0: [a, b, c]
        // Level 1: [H(a||b), H(c||c)]  (odd leaf duplicated)
        // Level 2: [H(H(a||b) || H(c||c))]
        assert_eq!(tree.len(), 3);
        assert_eq!(tree[0].len(), 3);
        assert_eq!(tree[1].len(), 2);
        assert_eq!(tree[2].len(), 1);
    }

    #[test]
    fn test_build_merkle_tree_matches_merkle_root_from_leaves() {
        // Verify build_merkle_tree root matches the existing merkle_root_from_leaves
        let leaves: Vec<Hash> = (0..10u8).map(|i| Hash::hash(&[i])).collect();
        let tree = build_merkle_tree(&leaves);
        let tree_root = tree.last().unwrap()[0];
        let existing_root = StateStore::merkle_root_from_leaves(&leaves);
        assert_eq!(tree_root, existing_root);
    }

    #[test]
    fn test_generate_proof_single_leaf() {
        let leaf = Hash::hash(b"only");
        let tree = build_merkle_tree(&[leaf]);
        let proof = generate_proof(&tree, 0).unwrap();
        assert_eq!(proof.leaf_hash, leaf);
        assert!(proof.siblings.is_empty());
        assert!(proof.path.is_empty());
        assert!(proof.verify(&leaf)); // root == leaf when single
    }

    #[test]
    fn test_proof_verify_two_leaves() {
        let a = Hash::hash(b"left");
        let b = Hash::hash(b"right");
        let tree = build_merkle_tree(&[a, b]);
        let root = tree.last().unwrap()[0];

        // Proof for leaf 0 (left)
        let proof_a = generate_proof(&tree, 0).unwrap();
        assert!(proof_a.verify(&root));
        assert_eq!(proof_a.siblings.len(), 1);
        assert!(proof_a.path[0]); // left child

        // Proof for leaf 1 (right)
        let proof_b = generate_proof(&tree, 1).unwrap();
        assert!(proof_b.verify(&root));
        assert!(!proof_b.path[0]); // right child
    }

    #[test]
    fn test_proof_verify_many_leaves() {
        let leaves: Vec<Hash> = (0..17u8).map(|i| Hash::hash(&[i])).collect();
        let tree = build_merkle_tree(&leaves);
        let root = tree.last().unwrap()[0];

        // Every leaf should produce a valid proof
        for i in 0..leaves.len() {
            let proof = generate_proof(&tree, i).unwrap();
            assert!(proof.verify(&root), "Proof for leaf {} failed to verify", i);
        }
    }

    #[test]
    fn test_proof_verify_rejects_wrong_root() {
        let a = Hash::hash(b"x");
        let b = Hash::hash(b"y");
        let tree = build_merkle_tree(&[a, b]);
        let proof = generate_proof(&tree, 0).unwrap();
        let wrong_root = Hash::hash(b"wrong");
        assert!(!proof.verify(&wrong_root));
    }

    #[test]
    fn test_proof_verify_account_data() {
        let pk = Pubkey([42u8; 32]);
        let data = b"account data";
        let leaf = Hash::hash_two_parts(&pk.0, data);
        let other_leaf = Hash::hash(b"other");
        let tree = build_merkle_tree(&[leaf, other_leaf]);
        let root = tree.last().unwrap()[0];

        let proof = generate_proof(&tree, 0).unwrap();
        assert!(proof.verify_account(&root, &pk, data));
        // Wrong data should fail
        assert!(!proof.verify_account(&root, &pk, b"wrong data"));
        // Wrong pubkey should fail
        let wrong_pk = Pubkey([99u8; 32]);
        assert!(!proof.verify_account(&root, &wrong_pk, data));
    }

    #[test]
    fn test_proof_out_of_bounds() {
        let leaves = vec![Hash::hash(b"a"), Hash::hash(b"b")];
        let tree = build_merkle_tree(&leaves);
        assert!(generate_proof(&tree, 2).is_none());
        assert!(generate_proof(&tree, 100).is_none());
    }

    #[test]
    fn test_merkle_proof_serde_roundtrip() {
        let proof = MerkleProof {
            leaf_hash: Hash::hash(b"leaf"),
            siblings: vec![Hash::hash(b"sib1"), Hash::hash(b"sib2")],
            path: vec![true, false],
        };
        let json = serde_json::to_string(&proof).unwrap();
        let restored: MerkleProof = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.leaf_hash, proof.leaf_hash);
        assert_eq!(restored.siblings.len(), 2);
        assert_eq!(restored.path, proof.path);
    }

    #[test]
    fn test_get_account_proof_integration() {
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();

        // Create some accounts
        let pk1 = Pubkey([1u8; 32]);
        let pk2 = Pubkey([2u8; 32]);
        let pk3 = Pubkey([3u8; 32]);

        let mut a1 = Account::new(1_000_000, pk1);
        a1.spores = 1_000_000;
        let mut a2 = Account::new(2_000_000, pk2);
        a2.spores = 2_000_000;
        let mut a3 = Account::new(3_000_000, pk3);
        a3.spores = 3_000_000;

        state.put_account(&pk1, &a1).unwrap();
        state.put_account(&pk2, &a2).unwrap();
        state.put_account(&pk3, &a3).unwrap();

        // Compute state root to populate leaf cache
        let composite_root = state.compute_state_root();
        let accounts_root = state.compute_accounts_root();
        assert_ne!(accounts_root, Hash::default());

        // Get proof for pk2
        let proof = state.get_account_proof(&pk2);
        assert!(proof.is_some(), "Should produce an account proof");

        let ap = proof.unwrap();
        assert_eq!(ap.pubkey, pk2);
        // state_root in the proof is now the composite root (accounts + contract storage)
        assert_eq!(ap.state_root, composite_root);

        // The merkle proof itself verifies against the accounts sub-root
        assert!(ap.proof.verify(&accounts_root));
        assert!(ap
            .proof
            .verify_account(&accounts_root, &pk2, &ap.account_data));

        // Standalone verification against accounts sub-root
        assert!(StateStore::verify_account_proof(
            &accounts_root,
            &pk2,
            &ap.account_data,
            &ap.proof
        ));
    }

    #[test]
    fn test_get_account_proof_nonexistent() {
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();

        let pk = Pubkey([99u8; 32]);
        assert!(state.get_account_proof(&pk).is_none());
    }

    #[test]
    fn test_proof_consistency_after_state_change() {
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();

        let pk1 = Pubkey([10u8; 32]);
        let pk2 = Pubkey([20u8; 32]);
        state.put_account(&pk1, &Account::new(100, pk1)).unwrap();
        state.put_account(&pk2, &Account::new(200, pk2)).unwrap();

        let _composite1 = state.compute_state_root();
        let root1 = state.compute_accounts_root();
        let proof1 = state.get_account_proof(&pk1).unwrap();
        assert!(proof1.proof.verify(&root1));

        // Modify pk2 — pk1's proof should now be invalid against new root
        let mut a2 = Account::new(300, pk2);
        a2.spores = 300;
        state.put_account(&pk2, &a2).unwrap();
        let _composite2 = state.compute_state_root();
        let root2 = state.compute_accounts_root();
        assert_ne!(root1, root2);

        // Old proof should NOT verify against new root
        assert!(!proof1.proof.verify(&root2));

        // New proof for pk1 should verify against new root
        let proof1_new = state.get_account_proof(&pk1).unwrap();
        assert!(proof1_new.proof.verify(&root2));
    }

    // ─── Dormancy Tests ──────────────────────────────────────────────────────

    #[test]
    fn test_dormant_account_excluded_from_state_root() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();

        let pk1 = Pubkey([1u8; 32]);
        let pk2 = Pubkey([2u8; 32]);

        // Create two active accounts
        let a1 = Account::new(100, pk1);
        let a2 = Account::new(200, pk2);
        state.put_account(&pk1, &a1).unwrap();
        state.put_account(&pk2, &a2).unwrap();
        let root_both = state.compute_state_root();
        assert_ne!(root_both, Hash::default());

        // Mark pk2 as dormant
        let mut a2_dormant = a2.clone();
        a2_dormant.dormant = true;
        state.put_account(&pk2, &a2_dormant).unwrap();
        let root_one = state.compute_state_root();

        // Root should change (dormant account excluded)
        assert_ne!(root_both, root_one);

        // Root should equal what you'd get with only pk1
        let dir2 = tempdir().unwrap();
        let state2 = StateStore::open(dir2.path()).unwrap();
        state2.put_account(&pk1, &a1).unwrap();
        let root_pk1_only = state2.compute_state_root();
        assert_eq!(root_one, root_pk1_only);
    }

    #[test]
    fn test_dormant_account_reactivated_on_transfer() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();

        let funder = Pubkey([1u8; 32]);
        let dormant_pk = Pubkey([2u8; 32]);

        // Create funder with sufficient balance
        let funder_acc = Account::new(1000, funder);
        state.put_account(&funder, &funder_acc).unwrap();

        // Create dormant account
        let mut dormant_acc = Account::new(0, dormant_pk);
        dormant_acc.dormant = true;
        dormant_acc.missed_rent_epochs = 3;
        state.put_account(&dormant_pk, &dormant_acc).unwrap();

        // Transfer should reactivate
        state.transfer(&funder, &dormant_pk, 500_000_000).unwrap();

        let reactivated = state.get_account(&dormant_pk).unwrap().unwrap();
        assert!(!reactivated.dormant);
        assert_eq!(reactivated.missed_rent_epochs, 0);
        assert_eq!(reactivated.spendable, 500_000_000);
    }

    #[test]
    fn test_dormant_account_reactivated_included_in_state_root() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();

        let funder = Pubkey([1u8; 32]);
        let target = Pubkey([2u8; 32]);

        let funder_acc = Account::new(1000, funder);
        state.put_account(&funder, &funder_acc).unwrap();

        // Start with dormant target
        let mut target_acc = Account::new(0, target);
        target_acc.dormant = true;
        target_acc.missed_rent_epochs = 5;
        state.put_account(&target, &target_acc).unwrap();
        let root_dormant = state.compute_state_root();

        // Transfer reactivates
        state.transfer(&funder, &target, 100_000_000).unwrap();
        let root_reactivated = state.compute_state_root();

        // Roots differ because target is now included
        assert_ne!(root_dormant, root_reactivated);
    }

    #[test]
    fn test_batch_transfer_reactivates_dormant() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();

        let funder = Pubkey([1u8; 32]);
        let dormant_pk = Pubkey([2u8; 32]);

        let funder_acc = Account::new(1000, funder);
        state.put_account(&funder, &funder_acc).unwrap();

        let mut dormant_acc = Account::new(0, dormant_pk);
        dormant_acc.dormant = true;
        dormant_acc.missed_rent_epochs = 2;
        state.put_account(&dormant_pk, &dormant_acc).unwrap();

        // Use batch transfer
        let mut batch = state.begin_batch();
        batch.transfer(&funder, &dormant_pk, 200_000_000).unwrap();
        state.commit_batch(batch).unwrap();

        let reactivated = state.get_account(&dormant_pk).unwrap().unwrap();
        assert!(!reactivated.dormant);
        assert_eq!(reactivated.missed_rent_epochs, 0);
    }

    #[test]
    fn test_deserialize_account_check_dormant() {
        // Active account
        let active = Account::new(100, Pubkey([1u8; 32]));
        let mut active_bytes = vec![0xBC];
        bincode::serialize_into(&mut active_bytes, &active).unwrap();
        assert!(!StateStore::deserialize_account_check_dormant(
            &active_bytes
        ));

        // Dormant account
        let mut dormant = Account::new(0, Pubkey([2u8; 32]));
        dormant.dormant = true;
        let mut dormant_bytes = vec![0xBC];
        bincode::serialize_into(&mut dormant_bytes, &dormant).unwrap();
        assert!(StateStore::deserialize_account_check_dormant(
            &dormant_bytes
        ));

        // Invalid bytes — should return false (treat as active)
        assert!(!StateStore::deserialize_account_check_dormant(&[
            0xBC, 0xFF
        ]));
    }

    // ── Task 3.4: EVM Log Storage tests ──

    #[test]
    fn test_put_get_evm_logs_for_slot_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();

        let logs = vec![
            crate::evm::EvmLogEntry {
                tx_hash: [0xAA; 32],
                tx_index: 0,
                log_index: 0,
                log: crate::evm::EvmLog {
                    address: [0x11; 20],
                    topics: vec![[0x01; 32], [0x02; 32]],
                    data: vec![0xFF, 0xFE],
                },
            },
            crate::evm::EvmLogEntry {
                tx_hash: [0xAA; 32],
                tx_index: 0,
                log_index: 1,
                log: crate::evm::EvmLog {
                    address: [0x22; 20],
                    topics: vec![[0x03; 32]],
                    data: vec![],
                },
            },
        ];

        state.put_evm_logs_for_slot(100, &logs).unwrap();
        let retrieved = state.get_evm_logs_for_slot(100).unwrap();
        assert_eq!(retrieved.len(), 2);
        assert_eq!(retrieved[0].tx_hash, [0xAA; 32]);
        assert_eq!(retrieved[0].log.address, [0x11; 20]);
        assert_eq!(retrieved[0].log.topics.len(), 2);
        assert_eq!(retrieved[1].log_index, 1);
        assert_eq!(retrieved[1].log.address, [0x22; 20]);
    }

    #[test]
    fn test_evm_logs_empty_slot_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();

        let logs = state.get_evm_logs_for_slot(999).unwrap();
        assert!(logs.is_empty());
    }

    #[test]
    fn test_evm_logs_append_multiple_txs_in_slot() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();

        // First tx with 1 log
        let logs1 = vec![crate::evm::EvmLogEntry {
            tx_hash: [0x01; 32],
            tx_index: 0,
            log_index: 0,
            log: crate::evm::EvmLog {
                address: [0xAA; 20],
                topics: vec![[0x10; 32]],
                data: vec![1],
            },
        }];
        state.put_evm_logs_for_slot(50, &logs1).unwrap();

        // Second tx with 2 logs (appends to same slot)
        let logs2 = vec![
            crate::evm::EvmLogEntry {
                tx_hash: [0x02; 32],
                tx_index: 1,
                log_index: 1,
                log: crate::evm::EvmLog {
                    address: [0xBB; 20],
                    topics: vec![[0x20; 32]],
                    data: vec![2],
                },
            },
            crate::evm::EvmLogEntry {
                tx_hash: [0x02; 32],
                tx_index: 1,
                log_index: 2,
                log: crate::evm::EvmLog {
                    address: [0xCC; 20],
                    topics: vec![[0x30; 32]],
                    data: vec![3],
                },
            },
        ];
        state.put_evm_logs_for_slot(50, &logs2).unwrap();

        // Should have all 3 logs
        let all = state.get_evm_logs_for_slot(50).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].tx_hash, [0x01; 32]);
        assert_eq!(all[1].tx_hash, [0x02; 32]);
        assert_eq!(all[2].tx_hash, [0x02; 32]);
    }

    #[test]
    fn test_evm_logs_empty_vec_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();

        // Storing empty logs should be a no-op
        state.put_evm_logs_for_slot(200, &[]).unwrap();
        let logs = state.get_evm_logs_for_slot(200).unwrap();
        assert!(logs.is_empty());
    }

    #[test]
    fn test_evm_logs_different_slots_independent() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();

        let log_a = vec![crate::evm::EvmLogEntry {
            tx_hash: [0xAA; 32],
            tx_index: 0,
            log_index: 0,
            log: crate::evm::EvmLog {
                address: [0x11; 20],
                topics: vec![[0x01; 32]],
                data: vec![0xAA],
            },
        }];
        let log_b = vec![crate::evm::EvmLogEntry {
            tx_hash: [0xBB; 32],
            tx_index: 0,
            log_index: 0,
            log: crate::evm::EvmLog {
                address: [0x22; 20],
                topics: vec![[0x02; 32]],
                data: vec![0xBB],
            },
        }];

        state.put_evm_logs_for_slot(10, &log_a).unwrap();
        state.put_evm_logs_for_slot(20, &log_b).unwrap();

        let slot10 = state.get_evm_logs_for_slot(10).unwrap();
        let slot20 = state.get_evm_logs_for_slot(20).unwrap();
        assert_eq!(slot10.len(), 1);
        assert_eq!(slot20.len(), 1);
        assert_eq!(slot10[0].log.data, vec![0xAA]);
        assert_eq!(slot20[0].log.data, vec![0xBB]);
    }

    // ─── Task 3.9: Archive Mode Tests ───────────────────────────────

    #[test]
    fn test_archive_put_and_get_account_at_slot() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey([0x01; 32]);
        let acc_v1 = Account::new(1, pk); // 1 LICN = 1_000_000_000 spores
        let acc_v2 = Account::new(2, pk); // 2 LICN
        let acc_v3 = Account::new(3, pk); // 3 LICN

        // Write snapshots at slots 10, 20, 30
        state.put_account_snapshot(&pk, &acc_v1, 10).unwrap();
        state.put_account_snapshot(&pk, &acc_v2, 20).unwrap();
        state.put_account_snapshot(&pk, &acc_v3, 30).unwrap();

        // Exact slot lookups
        let r = state.get_account_at_slot(&pk, 10).unwrap().unwrap();
        assert_eq!(r.spores, 1_000_000_000);
        let r = state.get_account_at_slot(&pk, 20).unwrap().unwrap();
        assert_eq!(r.spores, 2_000_000_000);
        let r = state.get_account_at_slot(&pk, 30).unwrap().unwrap();
        assert_eq!(r.spores, 3_000_000_000);

        // Intermediate slot: slot 25 → should return snapshot at slot 20
        let r = state.get_account_at_slot(&pk, 25).unwrap().unwrap();
        assert_eq!(r.spores, 2_000_000_000);

        // Future slot: slot 100 → should return latest snapshot at slot 30
        let r = state.get_account_at_slot(&pk, 100).unwrap().unwrap();
        assert_eq!(r.spores, 3_000_000_000);
    }

    #[test]
    fn test_archive_no_snapshot_before_slot() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey([0x02; 32]);
        let acc = Account::new(5, pk); // 5 LICN
        state.put_account_snapshot(&pk, &acc, 50).unwrap();

        // Before any snapshot exists → None
        let r = state.get_account_at_slot(&pk, 49).unwrap();
        assert!(r.is_none());

        // At slot 50 → found
        let r = state.get_account_at_slot(&pk, 50).unwrap();
        assert!(r.is_some());
    }

    #[test]
    fn test_archive_unknown_pubkey() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let unknown = Pubkey([0xFF; 32]);
        let r = state.get_account_at_slot(&unknown, 999).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn test_archive_mode_toggle() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        assert!(!state.is_archive_mode());
        state.set_archive_mode(true);
        assert!(state.is_archive_mode());
        state.set_archive_mode(false);
        assert!(!state.is_archive_mode());
    }

    #[test]
    fn test_archive_put_account_writes_snapshot_when_enabled() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey([0x03; 32]);
        let acc = Account::new(10, pk); // 10 LICN

        // Set slot to 42
        state.set_last_slot(42).unwrap();

        // Without archive mode → no snapshot
        state.put_account(&pk, &acc).unwrap();
        let r = state.get_account_at_slot(&pk, 42).unwrap();
        assert!(r.is_none());

        // Enable archive mode
        state.set_archive_mode(true);

        let acc2 = Account::new(20, pk); // 20 LICN
        state.put_account(&pk, &acc2).unwrap();
        let r = state.get_account_at_slot(&pk, 42).unwrap().unwrap();
        assert_eq!(r.spores, 20_000_000_000);
    }

    #[test]
    fn test_archive_batch_writes_snapshot_when_enabled() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey([0x04; 32]);

        // Set slot and enable archive
        state.set_last_slot(100).unwrap();
        state.set_archive_mode(true);

        let acc = Account::new(50, pk); // 50 LICN
        let mut batch = state.begin_batch();
        batch.put_account(&pk, &acc).unwrap();
        state.commit_batch(batch).unwrap();

        // Snapshot should exist at slot 100
        let r = state.get_account_at_slot(&pk, 100).unwrap().unwrap();
        assert_eq!(r.spores, 50_000_000_000);
    }

    #[test]
    fn test_archive_batch_no_snapshot_when_disabled() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey([0x05; 32]);
        state.set_last_slot(200).unwrap();
        // archive_mode defaults to false

        let acc = Account::new(30, pk); // 30 LICN
        let mut batch = state.begin_batch();
        batch.put_account(&pk, &acc).unwrap();
        state.commit_batch(batch).unwrap();

        // No snapshot expected
        let r = state.get_account_at_slot(&pk, 200).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn test_archive_prune_snapshots() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey([0x06; 32]);
        for slot in (10..=100).step_by(10) {
            let acc = Account::new(slot, pk); // slot LICN each
            state.put_account_snapshot(&pk, &acc, slot).unwrap();
        }

        // Prune everything before slot 50
        let pruned = state.prune_account_snapshots(50).unwrap();
        assert_eq!(pruned, 4); // slots 10, 20, 30, 40

        // Slot 40 should be gone
        let r = state.get_account_at_slot(&pk, 40).unwrap();
        assert!(r.is_none());

        // Slot 50 should still exist
        let r = state.get_account_at_slot(&pk, 50).unwrap().unwrap();
        assert_eq!(r.spores, 50_000_000_000); // 50 LICN

        // Oldest snapshot should be 50
        let oldest = state.get_oldest_snapshot_slot().unwrap().unwrap();
        assert_eq!(oldest, 50);
    }

    #[test]
    fn test_archive_oldest_snapshot_slot_empty() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let oldest = state.get_oldest_snapshot_slot().unwrap();
        assert!(oldest.is_none());
    }

    #[test]
    fn test_archive_multiple_accounts_isolation() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk_a = Pubkey([0x0A; 32]);
        let pk_b = Pubkey([0x0B; 32]);

        let acc_a = Account::new(1, pk_a); // 1 LICN
        let acc_b = Account::new(2, pk_b); // 2 LICN

        state.put_account_snapshot(&pk_a, &acc_a, 10).unwrap();
        state.put_account_snapshot(&pk_b, &acc_b, 10).unwrap();

        let r_a = state.get_account_at_slot(&pk_a, 10).unwrap().unwrap();
        let r_b = state.get_account_at_slot(&pk_b, 10).unwrap().unwrap();
        assert_eq!(r_a.spores, 1_000_000_000);
        assert_eq!(r_b.spores, 2_000_000_000);

        // Cross-account isolation: querying pk_a at slot 10 should not return pk_b's data
        assert_eq!(r_a.owner, pk_a);
        assert_eq!(r_b.owner, pk_b);
    }

    #[test]
    fn test_archive_seek_for_prev_boundary() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey([0x07; 32]);

        // Only one snapshot at slot 1000
        state
            .put_account_snapshot(&pk, &Account::new(1, pk), 1000)
            .unwrap();

        // Querying any slot >= 1000 returns it
        assert!(state.get_account_at_slot(&pk, 1000).unwrap().is_some());
        assert!(state.get_account_at_slot(&pk, u64::MAX).unwrap().is_some());

        // Querying slot < 1000 returns None
        assert!(state.get_account_at_slot(&pk, 999).unwrap().is_none());
        assert!(state.get_account_at_slot(&pk, 0).unwrap().is_none());
    }
}
