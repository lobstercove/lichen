// Lichen Validator with BFT Consensus + P2P Network + RPC Server
// Week 4: Multi-validator networking with QUIC transport + RPC integration
// Week 5: Block broadcasting, mempool, and multi-validator consensus
//
// AUDIT-FIX 1.23: Global Lock Ordering Contract
// All async code MUST acquire locks in this order to prevent deadlocks:
//   1. vote_aggregator (VoteAggregator) — RwLock
//   2. validator_set   (ValidatorSet)   — RwLock
//   3. stake_pool      (StakePool)      — RwLock
//   4. slashing_tracker (SlashingTracker) — Mutex
//   5. mempool         (Mempool)          — Mutex
// NEVER acquire a lower-numbered lock while holding a higher-numbered one.
// If only a subset is needed, the relative order must still be respected.
// PERF: 1-3 use RwLock — reads never block each other.

pub mod block_producer;
pub mod block_receiver;
pub mod consensus;
mod keypair_loader;
#[allow(dead_code)]
mod sync;
mod threshold_signer;
pub mod updater;
pub mod wal;

use futures_util::{SinkExt, StreamExt};
use lichen_core::nft::decode_token_state;
use lichen_core::{
    compute_bft_timestamp, compute_validators_hash, evm_tx_hash, Account, Block,
    ContractInstruction, FeeConfig, FinalityTracker, ForkChoice, GenesisConfig, GenesisWallet,
    Hash, Keypair, MarketActivity, MarketActivityKind, Mempool, NftActivity, NftActivityKind,
    Precommit, Prevote, ProgramCallActivity, Proposal, Pubkey, RoundStep, SlashingEvidence,
    SlashingOffense, StakePool, StateStore, Transaction, TxProcessor, ValidatorInfo, ValidatorSet,
    Vote, VoteAggregator, VoteAuthority, BASE_FEE, BOOTSTRAP_GRANT_AMOUNT, CONTRACT_DEPLOY_FEE,
    CONTRACT_UPGRADE_FEE, EVM_PROGRAM_ID, GENESIS_SUPPLY_SPORES, MAX_TX_AGE_BLOCKS,
    NFT_COLLECTION_FEE, NFT_MINT_FEE, SLOTS_PER_EPOCH, SYSTEM_PROGRAM_ID as CORE_SYSTEM_PROGRAM_ID,
};
use lichen_genesis::{
    derive_contract_address, genesis_auto_deploy, genesis_create_trading_pairs,
    genesis_initialize_contracts, genesis_licn_price_8dec, genesis_seed_analytics_prices,
    genesis_seed_margin_prices, genesis_seed_oracle, genesis_wbnb_price_8dec,
    genesis_weth_price_8dec, genesis_wsol_price_8dec, GENESIS_LICN_PRICE_8DEC,
};
use lichen_p2p::{
    validator_announcement_signing_message, ConsistencyReportMsg, MessageType, NodeRole, P2PConfig,
    P2PMessage, P2PNetwork, SnapshotKind, SnapshotRequestMsg, SnapshotResponseMsg,
    StatusRequestMsg, StatusResponseMsg,
};
use lichen_rpc::start_rpc_server;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use sync::SyncManager;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time;
use tokio_tungstenite::tungstenite;
use tracing::{debug, error, info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::consensus::{ConsensusAction, ConsensusEngine};

const SYSTEM_ACCOUNT_OWNER: Pubkey = Pubkey([0x01; 32]);
const LEGACY_CONTRACT_DEPLOY_FEE_SPORES: u64 = 2_500_000_000;
/// Treasury reserve funded at genesis for bootstrap grants (50M LICN = 10% of 500M genesis).
/// Block rewards are now minted via the inflation curve, not debited from treasury.
const TREASURY_RESERVE_LICN: u64 = 50_000_000;

/// Exit code used by the internal health watchdog to signal the supervisor
/// that the validator should be restarted (deadlock/stall detected).
const EXIT_CODE_RESTART: i32 = 75;

/// Default number of seconds with no block activity before the watchdog
/// triggers a restart.  Override with `--watchdog-timeout <secs>`.
/// Set to 120s to allow sufficient time for sync recovery under load.
/// The watchdog is also sync-aware: it won't fire while the node has
/// pending blocks or is actively syncing.
const DEFAULT_WATCHDOG_TIMEOUT_SECS: u64 = 120;

// =========================================================================
//  SHARED ORACLE PRICES — Thread-safe container for external feeder data
//
//  The background oracle price feeder (WebSocket + REST) updates these
//  atomics. The feeder turns those observations into signed native oracle
//  attestation transactions. Downstream DEX/analytics state mirrors the
//  finalized consensus oracle rather than raw proposer snapshots.
// =========================================================================

/// Thread-safe container for oracle prices fetched from external sources.
/// The background oracle feeder updates these atomics before submitting
/// native oracle attestation transactions into the mempool.
#[derive(Clone)]
struct SharedOraclePrices {
    wsol_micro: Arc<AtomicU64>,
    weth_micro: Arc<AtomicU64>,
    wbnb_micro: Arc<AtomicU64>,
    ws_healthy: Arc<AtomicBool>,
}

impl SharedOraclePrices {
    fn new() -> Self {
        Self {
            wsol_micro: Arc::new(AtomicU64::new(0)),
            weth_micro: Arc::new(AtomicU64::new(0)),
            wbnb_micro: Arc::new(AtomicU64::new(0)),
            ws_healthy: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Sync request fanout: send block-range requests to top-N peers by score
/// instead of broadcasting to all peers.
const SYNC_REQUEST_FANOUT: usize = 3;

/// QoS: per-peer block-range serving token bucket, measured in blocks.
const BLOCK_RANGE_SERVE_BURST_BLOCKS: u64 = 5000;
const BLOCK_RANGE_SERVE_REFILL_BLOCKS_PER_SEC: u64 = 1000;

/// QoS: per-peer snapshot serving token bucket, measured in request units.
const SNAPSHOT_SERVE_BURST_UNITS: u64 = 32;
const SNAPSHOT_SERVE_REFILL_UNITS_PER_SEC: u64 = 8;
const MAX_SNAPSHOT_CHUNK_SIZE: u64 = 2000;

/// Maximum number of automatic restarts before the supervisor gives up.
/// Override with `--max-restarts <n>`.
const DEFAULT_MAX_RESTARTS: u32 = 50;
const MIN_SUPPORTED_VALIDATOR_VERSION: &str = updater::VERSION;

/// Collect a machine fingerprint for anti-Sybil protection.
///
/// Computes SHA-256(platform_uuid || primary_mac_address).
/// - macOS: reads IOPlatformUUID via `ioreg` and MAC from `ifconfig en0`
/// - Linux: reads `/sys/class/dmi/id/product_uuid` (or `/etc/machine-id`) and MAC from `/sys/class/net/*/address`
///
/// Returns `[0u8; 32]` if unable to collect (dev mode fallback).
fn collect_machine_fingerprint() -> [u8; 32] {
    let mut hasher = Sha256::new();
    let mut got_uuid = false;
    let mut got_mac = false;

    // ── Platform UUID ──────────────────────────────────────────────
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("ioreg")
            .args(["-rd1", "-c", "IOPlatformExpertDevice"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("IOPlatformUUID") {
                    if let Some(uuid) = line.split('"').nth(3) {
                        hasher.update(uuid.as_bytes());
                        got_uuid = true;
                        break;
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try DMI product UUID first (requires root), then machine-id
        if let Ok(uuid) = std::fs::read_to_string("/sys/class/dmi/id/product_uuid") {
            hasher.update(uuid.trim().as_bytes());
            got_uuid = true;
        } else if let Ok(machine_id) = std::fs::read_to_string("/etc/machine-id") {
            hasher.update(machine_id.trim().as_bytes());
            got_uuid = true;
        }
    }

    // ── Primary MAC address ────────────────────────────────────────
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("ifconfig").arg("en0").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("ether ") {
                    let mac = trimmed.trim_start_matches("ether ").trim();
                    hasher.update(mac.as_bytes());
                    got_mac = true;
                    break;
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Read the first non-loopback, non-virtual MAC
        if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
            let mut macs: Vec<String> = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == "lo" || name.starts_with("veth") || name.starts_with("docker") {
                    continue;
                }
                let addr_path = entry.path().join("address");
                if let Ok(mac) = std::fs::read_to_string(&addr_path) {
                    let mac = mac.trim().to_string();
                    if mac != "00:00:00:00:00:00" {
                        macs.push(mac);
                    }
                }
            }
            macs.sort(); // Deterministic — pick first alphabetically
            if let Some(mac) = macs.first() {
                hasher.update(mac.as_bytes());
                got_mac = true;
            }
        }
    }

    if !got_uuid && !got_mac {
        // Unable to fingerprint this machine — return zeros (dev/test fallback)
        return [0u8; 32];
    }

    let result = hasher.finalize();
    let mut fingerprint = [0u8; 32];
    fingerprint.copy_from_slice(&result);
    fingerprint
}

fn parse_validator_version(version: &str) -> Result<Version, String> {
    let trimmed = version.trim();
    if trimmed.is_empty() {
        return Err("missing validator version".to_string());
    }

    let normalized = trimmed.strip_prefix('v').unwrap_or(trimmed);
    Version::parse(normalized)
        .map_err(|error| format!("invalid validator version '{}': {}", version, error))
}

fn validate_new_validator_version(version: &str) -> Result<Version, String> {
    let announced = parse_validator_version(version)?;
    let minimum = Version::parse(MIN_SUPPORTED_VALIDATOR_VERSION)
        .expect("MIN_SUPPORTED_VALIDATOR_VERSION must be valid semver");

    if announced < minimum {
        return Err(format!(
            "validator version {} is below minimum supported {}",
            version, MIN_SUPPORTED_VALIDATOR_VERSION
        ));
    }

    Ok(announced)
}

fn verify_validator_announcement_signature(
    pubkey: &Pubkey,
    stake: u64,
    current_slot: u64,
    version: &str,
    signature: &[u8; 64],
    machine_fingerprint: &[u8; 32],
    require_version_binding: bool,
) -> bool {
    let version_bound_valid = validator_announcement_signing_message(
        pubkey,
        stake,
        current_slot,
        machine_fingerprint,
        Some(version),
    )
    .ok()
    .map(|message| Keypair::verify(pubkey, &message, signature))
    .unwrap_or(false);

    if version_bound_valid {
        return true;
    }

    if require_version_binding {
        return false;
    }

    validator_announcement_signing_message(pubkey, stake, current_slot, machine_fingerprint, None)
        .ok()
        .map(|message| Keypair::verify(pubkey, &message, signature))
        .unwrap_or(false)
}

#[derive(Debug, Clone)]
struct TokenBucket {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last_refill: std::time::Instant,
}

impl TokenBucket {
    fn new(capacity: u64, refill_per_sec: u64) -> Self {
        Self {
            capacity: capacity as f64,
            tokens: capacity as f64,
            refill_per_sec: refill_per_sec as f64,
            last_refill: std::time::Instant::now(),
        }
    }

    fn try_consume(&mut self, cost: u64) -> bool {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);

        let cost_f = cost as f64;
        if self.tokens >= cost_f {
            self.tokens -= cost_f;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Deserialize)]
struct SeedsFile {
    testnet: Option<SeedNetwork>,
    mainnet: Option<SeedNetwork>,
    devnet: Option<SeedNetwork>,
}

#[derive(Debug, Deserialize)]
struct SeedNetwork {
    /// Retained for seeds.json schema completeness (deserialized but not read directly).
    #[allow(dead_code)]
    chain_id: String,
    #[serde(default)]
    bootstrap_peers: Vec<String>,
    #[serde(default)]
    seeds: Vec<SeedEntry>,
}

#[derive(Debug, Deserialize)]
struct SeedEntry {
    address: String,
}

fn resolve_peer_list(peers: &[String]) -> Vec<SocketAddr> {
    let mut resolved = Vec::new();
    for peer in peers {
        if let Ok(addr) = peer.parse::<SocketAddr>() {
            resolved.push(addr);
            continue;
        }
        if let Ok(addrs) = peer.to_socket_addrs() {
            resolved.extend(addrs);
        }
    }
    resolved
}

fn try_load_runtime_zk_verification_keys(processor: &TxProcessor, _data_dir: &Path) {
    // ZK keys are cached in a shared location (~/.lichen/zk/) so they
    // survive blockchain resets.  Release tarballs ship pre-generated keys
    // in a `zk/` directory next to the binary — those are copied into the
    // shared cache on first run so the expensive Groth16 setup never needs
    // to happen on the operator's machine.
    //
    // Priority: env vars > ~/.lichen/zk/ (shared cache) > bundled (next to exe) > auto-generate
    let shared_zk_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".lichen")
        .join("zk");

    let shield_path = env::var("LICHEN_ZK_SHIELD_VK_PATH")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| shared_zk_dir.join("vk_shield.bin"));
    let unshield_path = env::var("LICHEN_ZK_UNSHIELD_VK_PATH")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| shared_zk_dir.join("vk_unshield.bin"));
    let transfer_path = env::var("LICHEN_ZK_TRANSFER_VK_PATH")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| shared_zk_dir.join("vk_transfer.bin"));

    // If keys are missing from the shared cache, search well-known locations
    // and copy them into the shared cache so all future starts find them.
    //
    // Search order:
    //   1. zk/ next to the binary     (release tarball layout)
    //   2. zk-keys/ in CWD            (source-build / repo root)
    //   3. zk-keys/ next to binary    (uncommon but consistent)
    if !shield_path.exists() || !unshield_path.exists() || !transfer_path.exists() {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(exe_path) = env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                candidates.push(exe_dir.join("zk"));
                candidates.push(exe_dir.join("zk-keys"));
            }
        }
        candidates.push(PathBuf::from("zk-keys"));

        for candidate in &candidates {
            if candidate.is_dir()
                && candidate.join("vk_shield.bin").exists()
                && candidate.join("vk_unshield.bin").exists()
                && candidate.join("vk_transfer.bin").exists()
            {
                info!(
                    "🔑 Installing ZK keys from {} → {}",
                    candidate.display(),
                    shared_zk_dir.display()
                );
                if let Err(e) = fs::create_dir_all(&shared_zk_dir) {
                    warn!(
                        "⚠️  Failed creating ZK directory {}: {}",
                        shared_zk_dir.display(),
                        e
                    );
                    break;
                }
                if let Ok(entries) = fs::read_dir(candidate) {
                    for entry in entries.flatten() {
                        let dest = shared_zk_dir.join(entry.file_name());
                        if let Err(e) = fs::copy(entry.path(), &dest) {
                            warn!("⚠️  Failed copying {}: {}", entry.path().display(), e);
                        }
                    }
                    info!("✅ ZK keys installed to {}", shared_zk_dir.display());
                }
                break;
            }
        }
    }

    // If keys are still missing after checking bundled directory, log a clear
    // error.  We no longer auto-generate keys at runtime — the canonical
    // ceremony keys are committed to the repo and shipped in every release.
    if !shield_path.exists() || !unshield_path.exists() || !transfer_path.exists() {
        warn!(
            "⚠️  ZK verification keys not found at {} — shielded transactions unavailable. \
             Install from a release tarball or run `zk-setup --output {}` manually.",
            shared_zk_dir.display(),
            shared_zk_dir.display()
        );
        return;
    } else {
        info!(
            "🔑 ZK verification keys loaded from {}",
            shared_zk_dir.display()
        );
    }

    // Read all three VK files
    let shield_vk = match fs::read(&shield_path) {
        Ok(b) => b,
        Err(e) => {
            warn!(
                "⚠️  Failed reading shield VK at {}: {}",
                shield_path.display(),
                e
            );
            return;
        }
    };
    let unshield_vk = match fs::read(&unshield_path) {
        Ok(b) => b,
        Err(e) => {
            warn!(
                "⚠️  Failed reading unshield VK at {}: {}",
                unshield_path.display(),
                e
            );
            return;
        }
    };
    let transfer_vk = match fs::read(&transfer_path) {
        Ok(b) => b,
        Err(e) => {
            warn!(
                "⚠️  Failed reading transfer VK at {}: {}",
                transfer_path.display(),
                e
            );
            return;
        }
    };

    match processor.load_zk_verification_keys(&shield_vk, &unshield_vk, &transfer_vk) {
        Ok(_) => {
            info!(
                "✓ Loaded ZK verification keys (shield={}, unshield={}, transfer={})",
                shield_path.display(),
                unshield_path.display(),
                transfer_path.display()
            );
            // Persist VK hashes to pool state so the explorer / RPC
            // can confirm the verifier is initialised (vkShieldHash ≠ 0x00…).
            match processor.persist_vk_hashes_to_pool_state(&shield_vk, &unshield_vk, &transfer_vk)
            {
                Ok(_) => info!("✓ VK hashes persisted to shielded pool state"),
                Err(e) => warn!("⚠️  Failed persisting VK hashes: {}", e),
            }
        }
        Err(e) => warn!("⚠️  Failed loading ZK verification keys: {}", e),
    }
}

/// Discover companion binaries (faucet, custody, cli) installed alongside
/// the validator.  Only returns entries for binaries that actually exist on
/// disk — this way an agent running just the validator won't try to update
/// services it doesn't have.
fn discover_companion_binaries() -> Vec<(String, PathBuf)> {
    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let exe_dir = match exe_path.parent() {
        Some(d) => d,
        None => return Vec::new(),
    };

    let companions = [
        ("lichen-faucet", "lichen-faucet"),
        ("lichen-custody", "lichen-custody"),
        ("lichen-cli", "lichen-cli"),
    ];

    let mut found = Vec::new();
    for (name, filename) in &companions {
        let path = exe_dir.join(filename);
        if path.exists() {
            info!(
                "🔄 Auto-updater: companion binary found — {}",
                path.display()
            );
            found.push((name.to_string(), path));
        }
    }
    found
}

fn has_persistent_p2p_identity(runtime_home: &Path) -> bool {
    let lichen_dir = runtime_home.join(".lichen");
    lichen_dir.join("node_cert.der").exists() && lichen_dir.join("node_key.der").exists()
}

fn resolve_validator_runtime_home(data_dir: &Path) -> PathBuf {
    if let Ok(explicit_home) = env::var("LICHEN_HOME") {
        let explicit_path = PathBuf::from(&explicit_home);
        if !explicit_path.as_os_str().is_empty() {
            info!(
                "🏠 Runtime home: {} (from LICHEN_HOME env)",
                explicit_path.display()
            );
            return explicit_path;
        }
    }

    let state_home = data_dir.join("home");
    if has_persistent_p2p_identity(&state_home) {
        info!(
            "🏠 Runtime home: {} (existing P2P identity in data dir)",
            state_home.display()
        );
        return state_home;
    }

    if let Some(user_home) = dirs::home_dir() {
        if has_persistent_p2p_identity(&user_home) {
            info!(
                "🏠 Runtime home: {} (existing P2P identity in user home)",
                user_home.display()
            );
            return user_home;
        }
    }

    info!(
        "🏠 Runtime home: {} (default — new node, no existing identity)",
        state_home.display()
    );
    state_home
}

/// Run the Groth16 trusted setup and write VK + PK files to `zk_dir`.
///
/// Each circuit is set up independently and written to disk before the next
/// one starts.  This keeps peak memory usage at ~300MB instead of ~900MB
/// (which triggers the macOS OOM killer / jetsam for the transfer circuit's
/// 32-level Merkle path constraints).
fn load_seed_peers(chain_id: &str, seeds_path: &Path) -> Vec<String> {
    let contents = match fs::read_to_string(seeds_path) {
        Ok(data) => data,
        Err(_) => {
            // seeds.json not found — fall back to compile-time embedded seeds
            info!("📖 seeds.json not found, using embedded bootstrap peers");
            return load_embedded_seed_peers(chain_id);
        }
    };

    let seeds: SeedsFile = match serde_json::from_str(&contents) {
        Ok(value) => value,
        Err(e) => {
            warn!(
                "⚠️  Failed to parse {}: {} — using embedded bootstrap peers",
                seeds_path.display(),
                e
            );
            return load_embedded_seed_peers(chain_id);
        }
    };

    let network = if chain_id.contains("mainnet") {
        seeds.mainnet
    } else if chain_id.contains("testnet") {
        seeds.testnet
    } else if chain_id.contains("devnet") {
        seeds.devnet
    } else {
        None
    };

    let mut peers = Vec::new();
    if let Some(network) = network {
        peers.extend(network.bootstrap_peers);
        peers.extend(network.seeds.into_iter().map(|seed| seed.address));
    }

    // If seeds.json exists but the network section was empty, fall back to embedded
    if peers.is_empty() {
        return load_embedded_seed_peers(chain_id);
    }

    peers
}

/// Compile-time fallback bootstrap peers from core/src/network.rs
fn load_embedded_seed_peers(chain_id: &str) -> Vec<String> {
    use lichen_core::network::{NetworkType, SeedsConfig};
    let config = SeedsConfig::default_embedded();
    let network_type = if chain_id.contains("mainnet") {
        NetworkType::Mainnet
    } else if chain_id.contains("testnet") {
        NetworkType::Testnet
    } else {
        NetworkType::Devnet
    };
    config.get_all_peers(network_type)
}

#[derive(Serialize)]
struct ValidatorHashEntry {
    pubkey: Pubkey,
    stake: u64,
    pending_activation: bool,
}

fn hash_validator_set(set: &ValidatorSet) -> Hash {
    let entries: Vec<ValidatorHashEntry> = set
        .sorted_validators()
        .into_iter()
        .map(|validator| ValidatorHashEntry {
            pubkey: validator.pubkey,
            stake: validator.stake,
            pending_activation: validator.pending_activation,
        })
        .collect();

    let data = serde_json::to_vec(&entries).unwrap_or_default();
    Hash::hash(&data)
}

fn make_sync_observed_validator_info(
    producer: Pubkey,
    slot: u64,
    stake_amount: u64,
    transaction_count: usize,
    reward_already: bool,
) -> ValidatorInfo {
    ValidatorInfo {
        pubkey: producer,
        stake: stake_amount,
        reputation: 100,
        blocks_proposed: if reward_already { 0 } else { 1 },
        votes_cast: 0,
        correct_votes: 0,
        joined_slot: slot,
        last_active_slot: slot,
        commission_rate: 500,
        transactions_processed: if reward_already {
            0
        } else {
            transaction_count as u64
        },
        pending_activation: false,
    }
}

fn load_local_account_stake(state: &StateStore, validator: &Pubkey) -> Option<u64> {
    state
        .get_account(validator)
        .ok()
        .flatten()
        .map(|account| account.staked)
}

fn load_local_stake_pool_amount(stake_pool: &StakePool, validator: &Pubkey) -> Option<u64> {
    stake_pool
        .get_stake(validator)
        .map(|stake| stake.total_stake())
}

fn hash_stake_pool(pool: &StakePool) -> Hash {
    let entries = pool.stake_entries();
    let data = serde_json::to_vec(&entries).unwrap_or_default();
    Hash::hash(&data)
}

#[derive(Deserialize)]
struct TreasuryKeyFile {
    secret_key: String,
}

fn resolve_treasury_keypair_path(
    genesis_wallet: Option<&GenesisWallet>,
    data_dir: &Path,
    keys_dir: &Path,
    chain_id: &str,
) -> Option<PathBuf> {
    // Check treasury_keypair_path from genesis-wallet.json, resolved relative to data_dir
    if let Some(wallet) = genesis_wallet {
        if let Some(path) = wallet.treasury_keypair_path.as_ref() {
            let candidate = PathBuf::from(path);
            // If absolute, use as-is; otherwise resolve relative to data_dir
            let resolved = if candidate.is_absolute() {
                candidate
            } else {
                data_dir.join(&candidate)
            };
            if resolved.exists() {
                return Some(resolved);
            }
        }
    }

    // Fallback: look directly in genesis-keys/
    let candidate = keys_dir.join(format!("treasury-{}.json", chain_id));
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

fn load_treasury_keypair(
    genesis_wallet: Option<&GenesisWallet>,
    data_dir: &Path,
    keys_dir: &Path,
    chain_id: &str,
) -> Option<Keypair> {
    let path = resolve_treasury_keypair_path(genesis_wallet, data_dir, keys_dir, chain_id)?;
    let contents = match fs::read_to_string(&path) {
        Ok(data) => data,
        Err(e) => {
            warn!(
                "⚠️  Failed to read treasury keypair {}: {}",
                path.display(),
                e
            );
            return None;
        }
    };

    let parsed: TreasuryKeyFile = match serde_json::from_str(&contents) {
        Ok(file) => file,
        Err(e) => {
            warn!(
                "⚠️  Failed to parse treasury keypair {}: {}",
                path.display(),
                e
            );
            return None;
        }
    };

    let bytes = match hex::decode(parsed.secret_key) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!(
                "⚠️  Failed to decode treasury keypair {}: {}",
                path.display(),
                e
            );
            return None;
        }
    };

    if bytes.len() != 32 {
        warn!(
            "⚠️  Treasury keypair {} has invalid length {}",
            path.display(),
            bytes.len()
        );
        return None;
    }

    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes[..32]);
    let keypair = Keypair::from_seed(&seed);
    // P10-VAL-06: Zeroize seed bytes after use to minimize key material exposure
    seed.iter_mut().for_each(|b| *b = 0);
    info!("🔐 Loaded treasury keypair from {}", path.display());
    Some(keypair)
}

fn is_reward_or_debt_tx(tx: &Transaction) -> bool {
    let Some(ix) = tx.message.instructions.first() else {
        return false;
    };

    if ix.program_id != CORE_SYSTEM_PROGRAM_ID {
        return false;
    }

    matches!(ix.data.first(), Some(2) | Some(3))
}

fn block_has_user_transactions(block: &Block) -> bool {
    // With protocol-level rewards (coinbase model), blocks only contain user txs.
    // Keep the is_reward_or_debt_tx filter for backward-compat with legacy blocks.
    block
        .transactions
        .iter()
        .any(|tx| !is_reward_or_debt_tx(tx))
}

fn validate_p2p_transaction_signatures(tx: &Transaction) -> bool {
    if tx.signatures.is_empty() || tx.message.instructions.is_empty() {
        return false;
    }

    let mut required_signers: HashSet<Pubkey> = HashSet::new();
    for ix in &tx.message.instructions {
        let Some(first_acc) = ix.accounts.first() else {
            return false;
        };
        required_signers.insert(*first_acc);
    }

    let message_bytes = tx.message.serialize();
    let mut verified_signers: HashSet<Pubkey> = HashSet::new();
    for sig in &tx.signatures {
        for signer in &required_signers {
            if !verified_signers.contains(signer) && Keypair::verify(signer, &message_bytes, sig) {
                verified_signers.insert(*signer);
                break;
            }
        }
    }

    verified_signers.len() == required_signers.len()
}

/// AUDIT-FIX 3.14: Returns count of indexing errors so callers can track failure rate.
fn record_block_activity(state: &StateStore, block: &Block) -> u32 {
    let mut activity_seq: u32 = 0;
    let mut error_count: u32 = 0;

    for tx in &block.transactions {
        let tx_signature = tx.signature();
        for ix in &tx.message.instructions {
            if ix.program_id == CORE_SYSTEM_PROGRAM_ID {
                match ix.data.first() {
                    Some(7) => {
                        if ix.accounts.len() < 4 {
                            continue;
                        }

                        let collection = ix.accounts[1];
                        let token = ix.accounts[2];
                        let owner = ix.accounts[3];

                        let activity = NftActivity {
                            slot: block.header.slot,
                            timestamp: block.header.timestamp,
                            kind: NftActivityKind::Mint,
                            collection,
                            token,
                            from: None,
                            to: owner,
                            tx_signature,
                        };

                        if let Err(e) = state.record_nft_activity(&activity, activity_seq) {
                            warn!("⚠️  Failed to record NFT mint activity: {}", e);
                            error_count += 1;
                        }

                        activity_seq = activity_seq.saturating_add(1);
                    }
                    Some(8) => {
                        if ix.accounts.len() < 3 {
                            continue;
                        }

                        let from = ix.accounts[0];
                        let token = ix.accounts[1];
                        let to = ix.accounts[2];

                        let token_account = match state.get_account(&token) {
                            Ok(Some(account)) => account,
                            _ => continue,
                        };

                        let token_state = match decode_token_state(&token_account.data) {
                            Ok(state) => state,
                            Err(_) => continue,
                        };

                        let activity = NftActivity {
                            slot: block.header.slot,
                            timestamp: block.header.timestamp,
                            kind: NftActivityKind::Transfer,
                            collection: token_state.collection,
                            token,
                            from: Some(from),
                            to,
                            tx_signature,
                        };

                        if let Err(e) = state.record_nft_activity(&activity, activity_seq) {
                            warn!("⚠️  Failed to record NFT transfer activity: {}", e);
                            error_count += 1;
                        }

                        activity_seq = activity_seq.saturating_add(1);
                    }
                    _ => {}
                }
            } else if ix.program_id == lichen_core::CONTRACT_PROGRAM_ID {
                if let Ok(ContractInstruction::Call {
                    function,
                    args,
                    value,
                }) = ContractInstruction::deserialize(&ix.data)
                {
                    if ix.accounts.len() < 2 {
                        continue;
                    }

                    let caller = ix.accounts[0];
                    let program = ix.accounts[1];

                    let activity = ProgramCallActivity {
                        slot: block.header.slot,
                        timestamp: block.header.timestamp,
                        program,
                        caller,
                        function: function.clone(),
                        value,
                        tx_signature,
                    };

                    if let Err(e) = state.record_program_call(&activity, activity_seq) {
                        warn!("⚠️  Failed to record program call: {}", e);
                        error_count += 1;
                    }
                    // AUDIT-FIX E-8: Increment activity_seq after each record to prevent
                    // duplicate keys when both program_call and market_activity are recorded
                    activity_seq = activity_seq.saturating_add(1);

                    let market_kind = match function.as_str() {
                        "list_nft" | "list_nft_with_royalty" => Some(MarketActivityKind::Listing),
                        "buy_nft" => Some(MarketActivityKind::Sale),
                        "cancel_listing" => Some(MarketActivityKind::Cancel),
                        "make_offer" | "make_offer_with_expiry" => Some(MarketActivityKind::Offer),
                        "accept_offer" => Some(MarketActivityKind::OfferAccepted),
                        "cancel_offer" => Some(MarketActivityKind::OfferCancelled),
                        "update_listing_price" => Some(MarketActivityKind::PriceUpdate),
                        "create_auction" => Some(MarketActivityKind::AuctionCreated),
                        "place_bid" => Some(MarketActivityKind::AuctionBid),
                        "settle_auction" => Some(MarketActivityKind::AuctionSettled),
                        "cancel_auction" => Some(MarketActivityKind::AuctionCancelled),
                        "make_collection_offer" => Some(MarketActivityKind::CollectionOffer),
                        "accept_collection_offer" => {
                            Some(MarketActivityKind::CollectionOfferAccepted)
                        }
                        "cancel_collection_offer" => Some(MarketActivityKind::OfferCancelled),
                        _ => None,
                    };

                    if let Some(kind) = market_kind {
                        let market_activity = build_market_activity(
                            kind,
                            function,
                            program,
                            caller,
                            &args,
                            block.header.slot,
                            block.header.timestamp,
                            tx_signature,
                        );

                        if let Err(e) = state.record_market_activity(&market_activity, activity_seq)
                        {
                            warn!("⚠️  Failed to record market activity: {}", e);
                            error_count += 1;
                        }

                        activity_seq = activity_seq.saturating_add(1);
                    } else {
                        activity_seq = activity_seq.saturating_add(1);
                    }
                }
            }
        }
    }
    error_count
}

struct ParsedMarketArgs {
    collection: Option<Pubkey>,
    token: Option<Pubkey>,
    token_id: Option<u64>,
    price: Option<u64>,
    seller: Option<Pubkey>,
    buyer: Option<Pubkey>,
}

fn parse_marketplace_args(args: &[u8]) -> ParsedMarketArgs {
    let mut parsed = ParsedMarketArgs {
        collection: None,
        token: None,
        token_id: None,
        price: None,
        seller: None,
        buyer: None,
    };

    if args.is_empty() {
        return parsed;
    }

    let Ok(value) = serde_json::from_slice::<JsonValue>(args) else {
        return parsed;
    };

    let Some(obj) = value.as_object() else {
        return parsed;
    };

    let parse_pubkey = |val: &JsonValue| -> Option<Pubkey> {
        let s = val.as_str()?;
        Pubkey::from_base58(s).ok()
    };

    let parse_u64 = |val: &JsonValue| -> Option<u64> {
        if let Some(num) = val.as_u64() {
            return Some(num);
        }
        val.as_str().and_then(|s| s.parse::<u64>().ok())
    };

    if let Some(val) = obj
        .get("collection")
        .or_else(|| obj.get("nft_contract"))
        .or_else(|| obj.get("nftContract"))
    {
        parsed.collection = parse_pubkey(val);
    }

    if let Some(val) = obj.get("token") {
        parsed.token = parse_pubkey(val);
        if parsed.token.is_none() {
            parsed.token_id = parse_u64(val);
        }
    }

    if let Some(val) = obj.get("token_id").or_else(|| obj.get("tokenId")) {
        parsed.token_id = parse_u64(val);
    }

    if let Some(val) = obj.get("price") {
        parsed.price = parse_u64(val);
    }

    if let Some(val) = obj.get("seller") {
        parsed.seller = parse_pubkey(val);
    }

    if let Some(val) = obj.get("buyer") {
        parsed.buyer = parse_pubkey(val);
    }

    parsed
}

#[allow(clippy::too_many_arguments)]
fn build_market_activity(
    kind: MarketActivityKind,
    function: String,
    program: Pubkey,
    caller: Pubkey,
    args: &[u8],
    slot: u64,
    timestamp: u64,
    tx_signature: Hash,
) -> MarketActivity {
    let parsed = parse_marketplace_args(args);

    let (seller, buyer) = match kind {
        MarketActivityKind::Listing | MarketActivityKind::Cancel => {
            (parsed.seller.or(Some(caller)), parsed.buyer)
        }
        MarketActivityKind::Sale => (parsed.seller, parsed.buyer.or(Some(caller))),
        _ => (parsed.seller, parsed.buyer),
    };

    MarketActivity {
        slot,
        timestamp,
        kind,
        program,
        collection: parsed.collection,
        token: parsed.token,
        token_id: parsed.token_id,
        price: parsed.price,
        seller,
        buyer,
        function,
        tx_signature,
    }
}

fn emit_dex_events(
    state: &StateStore,
    dex_broadcaster: &lichen_rpc::dex_ws::DexEventBroadcaster,
    from_trade: u64,
    to_trade: u64,
    slot: u64,
) {
    const PRICE_SCALE: f64 = 1_000_000_000.0;

    // Emit events for each new trade
    let mut affected_pairs = std::collections::HashSet::new();
    for trade_id in (from_trade + 1)..=to_trade {
        let key = format!("dex_trade_{}", trade_id);
        if let Some(data) = state.get_program_storage("DEX", key.as_bytes()) {
            if data.len() >= 80 {
                // Trade layout: trade_id[0:8], pair_id[8:16], price[16:24], qty[24:32],
                //               taker[32:64], maker_order_id[64:72], slot[72:80]
                let pair_id = u64::from_le_bytes(data[8..16].try_into().unwrap_or([0; 8]));
                let price_raw = u64::from_le_bytes(data[16..24].try_into().unwrap_or([0; 8]));
                let quantity = u64::from_le_bytes(data[24..32].try_into().unwrap_or([0; 8]));
                let maker_order_id = u64::from_le_bytes(data[64..72].try_into().unwrap_or([0; 8]));
                let price = price_raw as f64 / PRICE_SCALE;

                // Infer side from maker order
                let side = {
                    let maker_key = format!("dex_order_{}", maker_order_id);
                    if let Some(order_data) = state.get_program_storage("DEX", maker_key.as_bytes())
                    {
                        if order_data.len() > 40 {
                            // Byte 40 = side (0=buy, 1=sell); taker is opposite
                            if order_data[40] == 0 {
                                "sell"
                            } else {
                                "buy"
                            }
                        } else {
                            "buy"
                        }
                    } else {
                        "buy"
                    }
                };

                dex_broadcaster.emit_trade(trade_id, pair_id, price, quantity, side, slot);
                affected_pairs.insert(pair_id);
            }
        }
    }

    // Emit orderbook + ticker updates for affected pairs
    let order_count = state.get_program_storage_u64("DEX", b"dex_order_count");
    for pair_id in &affected_pairs {
        // P9-VAL-06: Read per-pair last price (ana_lp_{pair_id}) instead of global last trade
        let lp_key = format!("ana_lp_{}", pair_id);
        if let Some(data) = state.get_program_storage("ANALYTICS", lp_key.as_bytes()) {
            if data.len() >= 8 {
                let price_raw = u64::from_le_bytes(data[0..8].try_into().unwrap_or([0; 8]));
                let last_price = price_raw as f64 / PRICE_SCALE;

                // Read 24h stats for volume/change
                let stats_key = format!("ana_24h_{}", pair_id);
                let (volume_24h, change_24h) = if let Some(stats_data) =
                    state.get_program_storage("ANALYTICS", stats_key.as_bytes())
                {
                    if stats_data.len() >= 48 {
                        let vol = u64::from_le_bytes(stats_data[0..8].try_into().unwrap_or([0; 8]));
                        let open_raw =
                            u64::from_le_bytes(stats_data[24..32].try_into().unwrap_or([0; 8]));
                        let open = open_raw as f64 / PRICE_SCALE;
                        let change = if open > 0.0 {
                            ((last_price - open) / open) * 100.0
                        } else {
                            0.0
                        };
                        (vol, change)
                    } else {
                        (0, 0.0)
                    }
                } else {
                    (0, 0.0)
                };

                dex_broadcaster.emit_ticker(
                    *pair_id, last_price, last_price, last_price, volume_24h, change_24h,
                );
            }
        }

        // ── WS broadcast: orderbook snapshot for affected pair ──
        let mut bids: std::collections::HashMap<u64, (u64, u32)> = std::collections::HashMap::new();
        let mut asks: std::collections::HashMap<u64, (u64, u32)> = std::collections::HashMap::new();
        for oid in 1..=order_count {
            let okey = format!("dex_order_{}", oid);
            if let Some(od) = state.get_program_storage("DEX", okey.as_bytes()) {
                if od.len() >= 128 {
                    let opid = u64::from_le_bytes(od[32..40].try_into().unwrap_or([0; 8]));
                    if opid != *pair_id {
                        continue;
                    }
                    let ostatus = od[66];
                    if ostatus != 0 && ostatus != 1 {
                        continue;
                    } // only open/partial
                    let oqty = u64::from_le_bytes(od[50..58].try_into().unwrap_or([0; 8]));
                    let ofilled = u64::from_le_bytes(od[58..66].try_into().unwrap_or([0; 8]));
                    let remaining = oqty.saturating_sub(ofilled);
                    if remaining == 0 {
                        continue;
                    }
                    let oprice = u64::from_le_bytes(od[42..50].try_into().unwrap_or([0; 8]));
                    let side_byte = od[40];
                    let entry = if side_byte == 0 {
                        bids.entry(oprice).or_insert((0, 0))
                    } else {
                        asks.entry(oprice).or_insert((0, 0))
                    };
                    entry.0 += remaining;
                    entry.1 += 1;
                }
            }
        }
        let bid_levels: Vec<lichen_rpc::dex_ws::PriceLevel> = {
            let mut v: Vec<_> = bids
                .into_iter()
                .map(|(p, (q, c))| lichen_rpc::dex_ws::PriceLevel {
                    price: p as f64 / PRICE_SCALE,
                    quantity: q,
                    orders: c,
                })
                .collect();
            v.sort_by(|a, b| {
                b.price
                    .partial_cmp(&a.price)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            v.truncate(20);
            v
        };
        let ask_levels: Vec<lichen_rpc::dex_ws::PriceLevel> = {
            let mut v: Vec<_> = asks
                .into_iter()
                .map(|(p, (q, c))| lichen_rpc::dex_ws::PriceLevel {
                    price: p as f64 / PRICE_SCALE,
                    quantity: q,
                    orders: c,
                })
                .collect();
            v.sort_by(|a, b| {
                a.price
                    .partial_cmp(&b.price)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            v.truncate(20);
            v
        };
        dex_broadcaster.emit_orderbook(*pair_id, bid_levels, ask_levels, slot);
    }

    // ── WS broadcast: order status updates for affected orders ──
    for trade_id in (from_trade + 1)..=to_trade {
        let key = format!("dex_trade_{}", trade_id);
        if let Some(data) = state.get_program_storage("DEX", key.as_bytes()) {
            if data.len() >= 80 {
                let maker_order_id = u64::from_le_bytes(data[64..72].try_into().unwrap_or([0; 8]));
                let maker_key = format!("dex_order_{}", maker_order_id);
                if let Some(od) = state.get_program_storage("DEX", maker_key.as_bytes()) {
                    if od.len() >= 128 {
                        let trader = hex::encode(&od[0..32]);
                        let qty = u64::from_le_bytes(od[50..58].try_into().unwrap_or([0; 8]));
                        let filled = u64::from_le_bytes(od[58..66].try_into().unwrap_or([0; 8]));
                        let status = match od[66] {
                            0 => "open",
                            1 => "partial",
                            2 => "filled",
                            3 => "cancelled",
                            _ => "expired",
                        };
                        dex_broadcaster.emit_order_update(
                            maker_order_id,
                            &trader,
                            status,
                            filled,
                            qty.saturating_sub(filled),
                            slot,
                        );
                    }
                }
            }
        }
    }
}

// ========================================================================
//  TRADE BRIDGE — dex_core → dex_analytics
//
//  After each block, iterates new dex_trade_* records from the DEX matching
//  engine and writes trade-driven analytics data directly to dex_analytics
//  contract storage:
//    • ana_lp_{pair_id}           — last trade price (overrides oracle)
//    • ana_24h_{pair_id}          — 24h volume, OHLC, trade count
//    • ana_24h_ts_{pair_id}       — unix timestamp of last 24h window reset
//    • ana_c_{pair_id}_{iv}_{idx} — candles for all 9 intervals
//    • ana_last_trade_ts_{pair_id}— unix timestamp of last real trade
//                                   (used by oracle feeder to skip writes)
//
//  This makes real trades drive displayed prices, charts, and volume.
//  The oracle feeder (Phase B) checks ana_last_trade_ts and only writes
//  indicative prices when no real trade occurred within 60 seconds.
// ========================================================================

/// Rolling 24h window reset — called every block.
/// Checks each trading pair's 24h stats window. If >86400 seconds have elapsed
/// since the last reset, sets open=current close, zeroes volume/trades,
/// resets high/low to current price.  This gives the user a true rolling 24h view.
/// P9-VAL-05: Accept deterministic block timestamp instead of SystemTime::now()
fn reset_24h_stats_if_expired(state: &StateStore, block_ts: u64) {
    let analytics_pk = match state.get_symbol_registry("ANALYTICS") {
        Ok(Some(entry)) => entry.program,
        _ => return,
    };

    let now_ts = block_ts;

    let pair_count = state.get_program_storage_u64("DEX", b"dex_pair_count");
    for pair_id in 1..=pair_count {
        let ts_key = format!("ana_24h_ts_{}", pair_id);
        let last_reset = match state.get_contract_storage(&analytics_pk, ts_key.as_bytes()) {
            Ok(Some(d)) if d.len() >= 8 => u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])),
            _ => 0,
        };

        // If never reset, seed the timestamp but don't clear stats (first boot)
        if last_reset == 0 {
            let _ =
                state.put_contract_storage(&analytics_pk, ts_key.as_bytes(), &now_ts.to_le_bytes());
            continue;
        }

        // Check if 24 hours have elapsed
        if now_ts.saturating_sub(last_reset) < 86400 {
            continue;
        }

        // Window expired — read current close price, then reset
        let stats_key = format!("ana_24h_{}", pair_id);
        let current_close = match state.get_contract_storage(&analytics_pk, stats_key.as_bytes()) {
            Ok(Some(d)) if d.len() >= 48 => {
                u64::from_le_bytes(d[32..40].try_into().unwrap_or([0; 8]))
            }
            _ => {
                // Fallback: try ana_lp_ (last traded price)
                let lp_key = format!("ana_lp_{}", pair_id);
                match state.get_contract_storage(&analytics_pk, lp_key.as_bytes()) {
                    Ok(Some(d)) if d.len() >= 8 => {
                        u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8]))
                    }
                    _ => 0,
                }
            }
        };

        // Reset: open = current close, volume = 0, trades = 0, high = close, low = close
        let mut stats = Vec::with_capacity(48);
        stats.extend_from_slice(&0u64.to_le_bytes()); // volume = 0
        stats.extend_from_slice(&current_close.to_le_bytes()); // high = close
        stats.extend_from_slice(&current_close.to_le_bytes()); // low = close
        stats.extend_from_slice(&current_close.to_le_bytes()); // open = close
        stats.extend_from_slice(&current_close.to_le_bytes()); // close = close
        stats.extend_from_slice(&0u64.to_le_bytes()); // trades = 0
        let _ = state.put_contract_storage(&analytics_pk, stats_key.as_bytes(), &stats);

        // Update reset timestamp
        let _ = state.put_contract_storage(&analytics_pk, ts_key.as_bytes(), &now_ts.to_le_bytes());

        debug!("📊 24h stats reset for pair {} (window expired)", pair_id);
    }
}

// ============================================================================
// STOP-LOSS / TAKE-PROFIT TRIGGER ENGINE
// ============================================================================
// After each block, check dormant stop-limit orders and margin position SL/TP
// levels. If conditions are met, activate orders and close positions by directly
// modifying contract storage (deterministic, all validators produce same result).

fn run_sltp_trigger_engine(state: &StateStore, from_trade: u64, to_trade: u64) {
    if from_trade >= to_trade {
        return;
    }

    let dex_pk = match state.get_symbol_registry("DEX") {
        Ok(Some(entry)) => entry.program,
        _ => return,
    };

    // Collect latest trade price per pair from new trades
    let mut pair_last_prices: std::collections::HashMap<u64, u64> =
        std::collections::HashMap::new();
    for trade_id in (from_trade + 1)..=to_trade {
        let key = format!("dex_trade_{}", trade_id);
        if let Some(data) = state.get_program_storage("DEX", key.as_bytes()) {
            if data.len() >= 32 {
                let pair_id = u64::from_le_bytes(data[8..16].try_into().unwrap_or([0; 8]));
                let price = u64::from_le_bytes(data[16..24].try_into().unwrap_or([0; 8]));
                if price > 0 {
                    pair_last_prices.insert(pair_id, price);
                }
            }
        }
    }

    if pair_last_prices.is_empty() {
        return;
    }

    // --- Part 1: Activate dormant stop-limit orders ---
    let order_count = state.get_program_storage_u64("DEX", b"dex_order_count");
    let mut triggered_count: u64 = 0;

    for oid in 1..=order_count {
        let ok = format!("dex_order_{}", oid);
        let data = match state.get_program_storage("DEX", ok.as_bytes()) {
            Some(d) if d.len() >= 128 => d,
            _ => continue,
        };

        // Check if dormant (status byte at offset 66, STATUS_DORMANT = 5)
        if data[66] != 5 {
            continue;
        }

        let pair_id = u64::from_le_bytes(data[32..40].try_into().unwrap_or([0; 8]));
        let last_price = match pair_last_prices.get(&pair_id) {
            Some(&p) => p,
            None => continue,
        };

        // Trigger price at bytes 91..99
        let trigger_price = u64::from_le_bytes(data[91..99].try_into().unwrap_or([0; 8]));
        if trigger_price == 0 {
            continue;
        }

        let side = data[40]; // 0=buy, 1=sell

        // Check trigger condition
        let should_trigger = if side == 1 {
            // Sell-stop: triggers when price falls to or below trigger
            last_price <= trigger_price
        } else {
            // Buy-stop: triggers when price rises to or above trigger
            last_price >= trigger_price
        };

        if !should_trigger {
            continue;
        }

        // Activate: set status to STATUS_OPEN (0)
        let mut new_data = data.clone();
        new_data[66] = 0; // STATUS_OPEN

        // Write activated order back
        let _ = state.put_contract_storage(&dex_pk, ok.as_bytes(), &new_data);

        // Add to order book level (the matching engine will process it on next trade)
        let price = u64::from_le_bytes(new_data[42..50].try_into().unwrap_or([0; 8]));
        let book_side_key = if side == 0 {
            format!("dex_bid_{}_{}", pair_id, price)
        } else {
            format!("dex_ask_{}_{}", pair_id, price)
        };

        // Append order ID to the price level's order queue
        if let Ok(Some(existing)) = state.get_contract_storage(&dex_pk, book_side_key.as_bytes()) {
            let mut updated = existing;
            updated.extend_from_slice(&oid.to_le_bytes());
            let _ = state.put_contract_storage(&dex_pk, book_side_key.as_bytes(), &updated);
        } else {
            let _ =
                state.put_contract_storage(&dex_pk, book_side_key.as_bytes(), &oid.to_le_bytes());
        }

        // Update best bid/ask if needed
        if side == 0 {
            // Buy order: update best bid if higher
            let best_bid = state
                .get_program_storage_u64("DEX", format!("dex_best_bid_{}", pair_id).as_bytes());
            if price > best_bid {
                let _ = state.put_contract_storage(
                    &dex_pk,
                    format!("dex_best_bid_{}", pair_id).as_bytes(),
                    &price.to_le_bytes(),
                );
            }
        } else {
            // Sell order: update best ask if lower
            let best_ask = state
                .get_program_storage_u64("DEX", format!("dex_best_ask_{}", pair_id).as_bytes());
            if best_ask == 0 || best_ask == u64::MAX || price < best_ask {
                let _ = state.put_contract_storage(
                    &dex_pk,
                    format!("dex_best_ask_{}", pair_id).as_bytes(),
                    &price.to_le_bytes(),
                );
            }
        }

        triggered_count += 1;
    }

    if triggered_count > 0 {
        info!(
            "🎯 Trigger engine: activated {} dormant stop-limit order(s)",
            triggered_count
        );
    }

    // --- Part 2: Check margin position SL/TP ---
    let margin_pk = match state.get_symbol_registry("DEXMARGIN") {
        Ok(Some(entry)) => entry.program,
        _ => return,
    };

    let pos_count = state.get_program_storage_u64("DEXMARGIN", b"mrg_pos_count");
    let mut sltp_closed: u64 = 0;

    for pid in 1..=pos_count {
        let pk = format!("mrg_pos_{}", pid);
        let data = match state.get_program_storage("DEXMARGIN", pk.as_bytes()) {
            Some(d) if d.len() >= 114 => d,
            _ => continue,
        };

        // Only open positions (status byte at offset 49, POS_OPEN = 0)
        if data[49] != 0 {
            continue;
        }

        let pair_id = u64::from_le_bytes(data[40..48].try_into().unwrap_or([0; 8]));
        let last_price = match pair_last_prices.get(&pair_id) {
            Some(&p) => p,
            None => continue,
        };

        // Read SL/TP from position data (bytes 106..114 = sl, 114..122 = tp)
        let sl_price = if data.len() >= 114 {
            u64::from_le_bytes(data[106..114].try_into().unwrap_or([0; 8]))
        } else {
            0
        };
        let tp_price = if data.len() >= 122 {
            u64::from_le_bytes(data[114..122].try_into().unwrap_or([0; 8]))
        } else {
            0
        };

        if sl_price == 0 && tp_price == 0 {
            continue;
        }

        let side = data[48]; // 0=long, 1=short
        let mut should_close = false;

        // Stop-loss check
        if sl_price > 0 {
            if side == 0 && last_price <= sl_price {
                // Long position: SL hit (price fell)
                should_close = true;
            } else if side == 1 && last_price >= sl_price {
                // Short position: SL hit (price rose)
                should_close = true;
            }
        }

        // Take-profit check
        if !should_close && tp_price > 0 {
            if side == 0 && last_price >= tp_price {
                // Long position: TP hit (price rose)
                should_close = true;
            } else if side == 1 && last_price <= tp_price {
                // Short position: TP hit (price fell)
                should_close = true;
            }
        }

        if !should_close {
            continue;
        }

        // P9-VAL-08: Re-read position status to prevent double-close race.
        // A user transaction processed in the same block may have already closed
        // this position between our initial read and this write.
        let fresh_data = match state.get_program_storage("DEXMARGIN", pk.as_bytes()) {
            Some(d) if d.len() >= 114 => d,
            _ => continue,
        };
        if fresh_data[49] != 0 {
            // Position was closed by a user TX in this block — skip
            continue;
        }

        // Close the position: set status to POS_CLOSED (1)
        let mut new_data = fresh_data.clone();
        new_data[49] = 1; // POS_CLOSED

        // Calculate realized PnL using the last trade price
        let entry_price = u64::from_le_bytes(new_data[66..74].try_into().unwrap_or([0; 8]));
        let size = u64::from_le_bytes(new_data[50..58].try_into().unwrap_or([0; 8]));
        let margin = u64::from_le_bytes(new_data[58..66].try_into().unwrap_or([0; 8]));

        // PnL = (exit_price - entry_price) * size / 1e9 for longs
        // PnL = (entry_price - exit_price) * size / 1e9 for shorts
        // Stored as biased: actual_pnl + BIAS where BIAS = 1 << 63 (matches dex_margin contract)
        const BIAS: u64 = 1u64 << 63;
        let pnl_raw: i64 = if side == 0 {
            // Long
            ((last_price as i128 - entry_price as i128) * size as i128 / 1_000_000_000i128) as i64
        } else {
            // Short
            ((entry_price as i128 - last_price as i128) * size as i128 / 1_000_000_000i128) as i64
        };
        let biased_pnl = (pnl_raw as i128 + BIAS as i128) as u64;
        new_data[90..98].copy_from_slice(&biased_pnl.to_le_bytes()); // realized_pnl

        let _ = state.put_contract_storage(&margin_pk, pk.as_bytes(), &new_data);

        // P9-VAL-02 FIX: Settle PnL through the insurance fund instead of
        // creating money from nothing.  Losses are credited to the fund;
        // profits are debited from the fund (capped at fund balance).
        let trader: [u8; 32] = new_data[0..32].try_into().unwrap_or([0u8; 32]);
        let abs_pnl = pnl_raw.unsigned_abs();

        // Read current insurance fund balance
        let insurance_fund = state.get_program_storage_u64("DEXMARGIN", b"mrg_insurance");

        let return_amount = if pnl_raw >= 0 {
            // Profitable close: pay profit from insurance fund (cap at fund balance)
            let capped_profit = abs_pnl.min(insurance_fund);
            // Debit insurance fund
            let _ = state.put_contract_storage(
                &margin_pk,
                b"mrg_insurance",
                &insurance_fund.saturating_sub(capped_profit).to_le_bytes(),
            );
            // Track cumulative profit
            let prev_profit = state.get_program_storage_u64("DEXMARGIN", b"mrg_pnl_profit");
            let _ = state.put_contract_storage(
                &margin_pk,
                b"mrg_pnl_profit",
                &prev_profit.saturating_add(capped_profit).to_le_bytes(),
            );
            margin.saturating_add(capped_profit)
        } else {
            // Loss close: credit insurance fund with the loss
            let loss = abs_pnl.min(margin); // can't lose more than margin
            let _ = state.put_contract_storage(
                &margin_pk,
                b"mrg_insurance",
                &insurance_fund.saturating_add(loss).to_le_bytes(),
            );
            // Track cumulative loss
            let prev_loss = state.get_program_storage_u64("DEXMARGIN", b"mrg_pnl_loss");
            let _ = state.put_contract_storage(
                &margin_pk,
                b"mrg_pnl_loss",
                &prev_loss.saturating_add(loss).to_le_bytes(),
            );
            margin.saturating_sub(loss)
        };

        // P9-VAL-03 FIX: Use saturating_add to prevent overflow
        let balance_key = format!("balance_{}", hex::encode(trader));
        let current_bal = state.get_program_storage_u64("LICHENCOIN", balance_key.as_bytes());
        let _ = state.put_contract_storage(
            &match state.get_symbol_registry("LICHENCOIN") {
                Ok(Some(e)) => e.program,
                _ => continue,
            },
            balance_key.as_bytes(),
            &current_bal.saturating_add(return_amount).to_le_bytes(),
        );

        let trigger_type = if sl_price > 0
            && ((side == 0 && last_price <= sl_price) || (side == 1 && last_price >= sl_price))
        {
            "SL"
        } else {
            "TP"
        };
        info!(
            "🎯 Margin {} triggered: position {} closed at price {} (entry {})",
            trigger_type, pid, last_price, entry_price
        );
        sltp_closed += 1;
    }

    if sltp_closed > 0 {
        info!(
            "🎯 Trigger engine: closed {} margin position(s) via SL/TP",
            sltp_closed
        );
    }
}

/// State-driven SL/TP trigger wrapper — reads a persistent cursor from state so that
/// both block producers AND block receivers execute triggers deterministically.
/// Previously, `run_sltp_trigger_engine` was only called in the block-production loop,
/// causing state divergence across validators (P9-VAL-01 fix).
fn run_sltp_triggers_from_state(state: &StateStore) {
    let cursor = state.get_program_storage_u64("DEX", b"dex_sltp_trigger_cursor");
    let current = state.get_program_storage_u64("DEX", b"dex_trade_count");
    if current > cursor {
        run_sltp_trigger_engine(state, cursor, current);
        // Persist the new cursor so subsequent blocks pick up from here
        if let Ok(Some(dex_entry)) = state.get_symbol_registry("DEX") {
            let _ = state.put_contract_storage(
                &dex_entry.program,
                b"dex_sltp_trigger_cursor",
                &current.to_le_bytes(),
            );
        }
    }
}

/// P9-VAL-04: Deterministic analytics bridge — uses state-persisted cursor
/// so both producers and receivers execute the same analytics writes.
fn run_analytics_bridge_from_state(state: &StateStore, slot: u64, slot_duration_ms: u64) {
    let cursor = state.get_program_storage_u64("DEX", b"dex_analytics_bridge_cursor");
    let current = state.get_program_storage_u64("DEX", b"dex_trade_count");
    if current > cursor {
        bridge_dex_trades_to_analytics(state, cursor, current, slot, slot_duration_ms);
        // Persist the new cursor so subsequent blocks pick up from here
        if let Ok(Some(dex_entry)) = state.get_symbol_registry("DEX") {
            let _ = state.put_contract_storage(
                &dex_entry.program,
                b"dex_analytics_bridge_cursor",
                &current.to_le_bytes(),
            );
        }
    }
}

fn bridge_dex_trades_to_analytics(
    state: &StateStore,
    from_trade: u64,
    to_trade: u64,
    slot: u64,
    slot_duration_ms: u64,
) {
    const PRICE_SCALE: f64 = 1_000_000_000.0;

    // Resolve ANALYTICS pubkey via symbol registry
    let analytics_pk = match state.get_symbol_registry("ANALYTICS") {
        Ok(Some(entry)) => entry.program,
        _ => return, // no analytics contract deployed
    };

    // P9-VAL-04: Use deterministic slot-derived timestamp instead of SystemTime::now()
    let genesis_ts = state
        .get_block_by_slot(0)
        .ok()
        .flatten()
        .map(|b| b.header.timestamp)
        .unwrap_or(0);
    // AUDIT-FIX E-1: Use passed-in slot_duration_ms from genesis config
    let now_ts = genesis_ts + (slot * slot_duration_ms / 1000);

    // Candle intervals matching dex_analytics: 1m, 5m, 15m, 1h, 4h, 1d, 3d, 1w, 1y
    const CANDLE_INTERVALS: [u64; 9] = [60, 300, 900, 3600, 14400, 86400, 259200, 604800, 31536000];

    // Collect per-pair trade summaries for this block
    // (pair_id → (last_price, total_volume, trade_count, high, low))
    let mut pair_trades: std::collections::HashMap<u64, (u64, u64, u64, u64, u64)> =
        std::collections::HashMap::new();

    for trade_id in (from_trade + 1)..=to_trade {
        let key = format!("dex_trade_{}", trade_id);
        if let Some(data) = state.get_program_storage("DEX", key.as_bytes()) {
            if data.len() >= 80 {
                // Trade layout: trade_id[0:8], pair_id[8:16], price[16:24], qty[24:32],
                //               taker[32:64], maker_order_id[64:72], slot[72:80]
                let pair_id = u64::from_le_bytes(data[8..16].try_into().unwrap_or([0; 8]));
                let price = u64::from_le_bytes(data[16..24].try_into().unwrap_or([0; 8]));
                let quantity = u64::from_le_bytes(data[24..32].try_into().unwrap_or([0; 8]));

                // Notional value = price * quantity / 1e9 (scaled)
                let notional = (price as u128 * quantity as u128 / 1_000_000_000) as u64;

                let entry = pair_trades.entry(pair_id).or_insert((0, 0, 0, 0, u64::MAX));
                entry.0 = price; // last price
                entry.1 = entry.1.saturating_add(notional); // cumulative volume
                entry.2 += 1; // trade count
                if price > entry.3 {
                    entry.3 = price;
                } // high
                if price < entry.4 {
                    entry.4 = price;
                } // low
            }
        }
    }

    // Write analytics for each pair that had trades
    //
    // ANALYTICS-FIX: Also update the global counters (ana_rec_count,
    // ana_total_volume, ana_pairs_tracked) that the RPC endpoint reads.
    // Without this, getDexAnalyticsStats always shows the initial values
    // because the bridge bypassed the contract's record_trade() function.
    let total_new_trades: u64 = pair_trades.values().map(|(_, _, tc, _, _)| tc).sum();
    let total_new_volume: u64 = pair_trades.values().map(|(_, vol, _, _, _)| vol).sum();
    let pairs_count = pair_trades.len() as u64;

    if total_new_trades > 0 {
        // Read current counters and increment
        let prev_rec = match state.get_contract_storage(&analytics_pk, b"ana_rec_count") {
            Ok(Some(d)) if d.len() >= 8 => u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])),
            _ => 0,
        };
        let prev_vol = match state.get_contract_storage(&analytics_pk, b"ana_total_volume") {
            Ok(Some(d)) if d.len() >= 8 => u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])),
            _ => 0,
        };
        let prev_pairs = match state.get_contract_storage(&analytics_pk, b"ana_trader_count") {
            Ok(Some(d)) if d.len() >= 8 => u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])),
            _ => 0,
        };

        let _ = state.put_contract_storage(
            &analytics_pk,
            b"ana_rec_count",
            &prev_rec.saturating_add(total_new_trades).to_le_bytes(),
        );
        let _ = state.put_contract_storage(
            &analytics_pk,
            b"ana_total_volume",
            &prev_vol.saturating_add(total_new_volume).to_le_bytes(),
        );
        // Use max of pairs_count vs prev — tracks unique pairs seen over time
        let _ = state.put_contract_storage(
            &analytics_pk,
            b"ana_trader_count",
            &prev_pairs.max(pairs_count).to_le_bytes(),
        );
    }

    for (pair_id, (last_price, volume, new_trades, high, low)) in &pair_trades {
        // ── ana_lp_{pair_id}: last trade price ──
        let lp_key = format!("ana_lp_{}", pair_id);
        let _ =
            state.put_contract_storage(&analytics_pk, lp_key.as_bytes(), &last_price.to_le_bytes());

        // ── ana_last_trade_ts_{pair_id}: unix timestamp for oracle fallback ──
        let ts_key = format!("ana_last_trade_ts_{}", pair_id);
        let _ = state.put_contract_storage(&analytics_pk, ts_key.as_bytes(), &now_ts.to_le_bytes());

        // ── ana_24h_{pair_id}: read-modify-write 24h stats ──
        // Layout: volume(8) + high(8) + low(8) + open(8) + close(8) + trades(8) = 48
        let stats_key = format!("ana_24h_{}", pair_id);
        let (prev_vol, mut prev_high, mut prev_low, prev_open, _prev_close, prev_trades) =
            match state.get_contract_storage(&analytics_pk, stats_key.as_bytes()) {
                Ok(Some(d)) if d.len() >= 48 => (
                    u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])),
                    u64::from_le_bytes(d[8..16].try_into().unwrap_or([0; 8])),
                    u64::from_le_bytes(d[16..24].try_into().unwrap_or([0; 8])),
                    u64::from_le_bytes(d[24..32].try_into().unwrap_or([0; 8])),
                    u64::from_le_bytes(d[32..40].try_into().unwrap_or([0; 8])),
                    u64::from_le_bytes(d[40..48].try_into().unwrap_or([0; 8])),
                ),
                _ => (0, 0, u64::MAX, *last_price, *last_price, 0),
            };

        if *high > prev_high {
            prev_high = *high;
        }
        if *low < prev_low {
            prev_low = *low;
        }

        // If open was zero (fresh 24h window), set it from first trade
        let open = if prev_open == 0 {
            *last_price
        } else {
            prev_open
        };

        let mut stats = Vec::with_capacity(48);
        stats.extend_from_slice(&prev_vol.saturating_add(*volume).to_le_bytes()); // volume
        stats.extend_from_slice(&prev_high.to_le_bytes()); // high
        stats.extend_from_slice(&prev_low.to_le_bytes()); // low
        stats.extend_from_slice(&open.to_le_bytes()); // open
        stats.extend_from_slice(&last_price.to_le_bytes()); // close = last trade
        stats.extend_from_slice(&prev_trades.saturating_add(*new_trades).to_le_bytes()); // trades
        let _ = state.put_contract_storage(&analytics_pk, stats_key.as_bytes(), &stats);

        // ── Candles: update all 9 intervals with real trade data ──
        for &interval in &CANDLE_INTERVALS {
            bridge_update_candle(
                state,
                &analytics_pk,
                *pair_id,
                interval,
                *last_price,
                *high,
                *low,
                *volume,
                slot,
                now_ts,
            );
        }

        let display_price = *last_price as f64 / PRICE_SCALE;
        info!(
            "📊 Trade bridge: pair {} → price {:.4}, vol {}, trades {}",
            pair_id, display_price, volume, new_trades
        );
    }
}

/// Update a candle for trade-bridged data.
/// Unlike oracle_update_candle which has volume=0, this writes real volume
/// and properly updates OHLC from actual trade price ranges.
#[allow(clippy::too_many_arguments)]
fn bridge_update_candle(
    state: &StateStore,
    analytics_pk: &Pubkey,
    pair_id: u64,
    interval: u64,
    close_price: u64,
    high_price: u64,
    low_price: u64,
    volume: u64,
    _current_slot: u64,
    unix_ts: u64,
) {
    // Use unix timestamp (not slot) for period grouping so candle boundaries
    // align with wall-clock seconds (60s, 300s, 3600s, etc.).
    let candle_start = (unix_ts / interval) * interval;

    // Read current candle's start slot
    let cur_key = format!("ana_cur_{}_{}", pair_id, interval);
    let stored_start = match state.get_contract_storage(analytics_pk, cur_key.as_bytes()) {
        Ok(Some(d)) if d.len() >= 8 => {
            Some(u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])))
        }
        _ => None,
    };

    let count_key = format!("ana_cc_{}_{}", pair_id, interval);

    if stored_start == Some(candle_start) {
        // Same candle period — update OHLC in-place
        let candle_count = match state.get_contract_storage(analytics_pk, count_key.as_bytes()) {
            Ok(Some(d)) if d.len() >= 8 => u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])),
            _ => 0,
        };
        if candle_count == 0 {
            return;
        }
        let idx = candle_count - 1;
        let candle_key = format!("ana_c_{}_{}_{}", pair_id, interval, idx);

        if let Ok(Some(mut data)) = state.get_contract_storage(analytics_pk, candle_key.as_bytes())
        {
            if data.len() >= 48 {
                // Candle layout: open(8)+high(8)+low(8)+close(8)+volume(8)+slot(8)
                let existing_high = u64::from_le_bytes(data[8..16].try_into().unwrap_or([0; 8]));
                let existing_low = u64::from_le_bytes(data[16..24].try_into().unwrap_or([0; 8]));
                let existing_vol = u64::from_le_bytes(data[32..40].try_into().unwrap_or([0; 8]));

                if high_price > existing_high {
                    data[8..16].copy_from_slice(&high_price.to_le_bytes());
                }
                if low_price < existing_low {
                    data[16..24].copy_from_slice(&low_price.to_le_bytes());
                }
                // Close = last trade price
                data[24..32].copy_from_slice(&close_price.to_le_bytes());
                // Accumulate real volume
                let new_vol = existing_vol.saturating_add(volume);
                data[32..40].copy_from_slice(&new_vol.to_le_bytes());
                // Keep timestamp as the period-start (don't overwrite with current time)

                let _ = state.put_contract_storage(analytics_pk, candle_key.as_bytes(), &data);
            }
        }
    } else {
        // New candle period — create a new candle with real trade data
        let candle_count = match state.get_contract_storage(analytics_pk, count_key.as_bytes()) {
            Ok(Some(d)) if d.len() >= 8 => u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])),
            _ => 0,
        };

        // open = close_price (first trade of new period)
        let mut candle = Vec::with_capacity(48);
        candle.extend_from_slice(&close_price.to_le_bytes()); // open = first trade price in period
        candle.extend_from_slice(&high_price.to_le_bytes()); // high
        candle.extend_from_slice(&low_price.to_le_bytes()); // low
        candle.extend_from_slice(&close_price.to_le_bytes()); // close
        candle.extend_from_slice(&volume.to_le_bytes()); // real trade volume
        candle.extend_from_slice(&candle_start.to_le_bytes()); // period-start time (aligned)

        let new_idx = candle_count;
        let candle_key = format!("ana_c_{}_{}_{}", pair_id, interval, new_idx);
        let _ = state.put_contract_storage(analytics_pk, candle_key.as_bytes(), &candle);

        // Update count
        let _ = state.put_contract_storage(
            analytics_pk,
            count_key.as_bytes(),
            &(new_idx + 1).to_le_bytes(),
        );

        // Store current candle start slot
        let _ = state.put_contract_storage(
            analytics_pk,
            cur_key.as_bytes(),
            &candle_start.to_le_bytes(),
        );
    }
}

fn emit_program_and_nft_events(
    state: &StateStore,
    ws_event_tx: &tokio::sync::broadcast::Sender<lichen_rpc::ws::Event>,
    block: &Block,
) {
    // AUDIT-FIX 3.14: Track indexing errors for monitoring
    let activity_errors = record_block_activity(state, block);
    if activity_errors > 0 {
        warn!(
            "⚠️  Block {} had {} activity indexing errors",
            block.header.slot, activity_errors
        );
    }

    for tx in &block.transactions {
        // Emit Transaction event for every tx in the block
        let _ = ws_event_tx.send(lichen_rpc::ws::Event::Transaction(tx.clone()));

        // Emit AccountChange events for all accounts touched by this tx
        let mut seen_accounts = std::collections::HashSet::new();
        for ix in &tx.message.instructions {
            for account_pubkey in &ix.accounts {
                if seen_accounts.insert(*account_pubkey) {
                    if let Ok(Some(acct)) = state.get_account(account_pubkey) {
                        let _ = ws_event_tx.send(lichen_rpc::ws::Event::AccountChange {
                            pubkey: *account_pubkey,
                            balance: acct.spores,
                        });
                    }
                }
            }

            if ix.program_id == CORE_SYSTEM_PROGRAM_ID {
                match ix.data.first() {
                    Some(7) => {
                        if ix.accounts.len() < 4 {
                            continue;
                        }

                        let collection = ix.accounts[1];
                        let _ = ws_event_tx.send(lichen_rpc::ws::Event::NftMint { collection });
                    }
                    Some(8) => {
                        if ix.accounts.len() < 3 {
                            continue;
                        }

                        let token = ix.accounts[1];

                        let token_account = match state.get_account(&token) {
                            Ok(Some(account)) => account,
                            _ => continue,
                        };

                        let token_state = match decode_token_state(&token_account.data) {
                            Ok(state) => state,
                            Err(_) => continue,
                        };

                        let _ = ws_event_tx.send(lichen_rpc::ws::Event::NftTransfer {
                            collection: token_state.collection,
                        });
                    }
                    _ => {}
                }
            } else if ix.program_id == lichen_core::CONTRACT_PROGRAM_ID {
                if let Ok(contract_ix) = ContractInstruction::deserialize(&ix.data) {
                    match contract_ix {
                        ContractInstruction::Deploy { .. } => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(lichen_rpc::ws::Event::ProgramUpdate {
                                    program: *program,
                                    kind: "deploy".to_string(),
                                });
                            }
                        }
                        ContractInstruction::Upgrade { .. } => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(lichen_rpc::ws::Event::ProgramUpdate {
                                    program: *program,
                                    kind: "upgrade".to_string(),
                                });
                            }
                        }
                        ContractInstruction::Close => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(lichen_rpc::ws::Event::ProgramUpdate {
                                    program: *program,
                                    kind: "close".to_string(),
                                });
                            }
                        }
                        ContractInstruction::SetUpgradeTimelock { .. } => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(lichen_rpc::ws::Event::ProgramUpdate {
                                    program: *program,
                                    kind: "set_timelock".to_string(),
                                });
                            }
                        }
                        ContractInstruction::ExecuteUpgrade => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(lichen_rpc::ws::Event::ProgramUpdate {
                                    program: *program,
                                    kind: "execute_upgrade".to_string(),
                                });
                            }
                        }
                        ContractInstruction::VetoUpgrade => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(lichen_rpc::ws::Event::ProgramUpdate {
                                    program: *program,
                                    kind: "veto_upgrade".to_string(),
                                });
                            }
                        }
                        ContractInstruction::Call { function, args, .. } => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx
                                    .send(lichen_rpc::ws::Event::ProgramCall { program: *program });

                                // Emit Log event for contract call
                                let _ = ws_event_tx.send(lichen_rpc::ws::Event::Log {
                                    contract: *program,
                                    message: format!("call:{}", function),
                                });

                                // Emit contract events from DB if stored during processing
                                if let Ok(events) = state.get_contract_logs(program, 50, None) {
                                    for event in &events {
                                        if event.slot == block.header.slot {
                                            let _ = ws_event_tx.send(lichen_rpc::ws::Event::Log {
                                                contract: event.program,
                                                message: format!(
                                                    "event:{}:{}",
                                                    event.name,
                                                    serde_json::to_string(&event.data)
                                                        .unwrap_or_default()
                                                ),
                                            });
                                        }
                                    }
                                }

                                let kind = match function.as_str() {
                                    "list_nft" => Some(MarketActivityKind::Listing),
                                    "buy_nft" => Some(MarketActivityKind::Sale),
                                    _ => None,
                                };

                                if let (Some(kind), Some(caller)) =
                                    (kind, ix.accounts.first().copied())
                                {
                                    let activity = build_market_activity(
                                        kind.clone(),
                                        function.clone(),
                                        *program,
                                        caller,
                                        &args,
                                        block.header.slot,
                                        block.header.timestamp,
                                        tx.signature(),
                                    );

                                    let _ = match kind {
                                        MarketActivityKind::Listing => {
                                            ws_event_tx.send(lichen_rpc::ws::Event::MarketListing {
                                                activity,
                                            })
                                        }
                                        MarketActivityKind::Sale => ws_event_tx
                                            .send(lichen_rpc::ws::Event::MarketSale { activity }),
                                        MarketActivityKind::Cancel => Ok(0),
                                        _ => Ok(0),
                                    };
                                }

                                // Emit bridge events for lock/mint calls
                                match function.as_str() {
                                    "lock" | "bridge_lock" => {
                                        let sender = ix
                                            .accounts
                                            .first()
                                            .map(|p| p.to_base58())
                                            .unwrap_or_default();
                                        let recipient = ix
                                            .accounts
                                            .get(2)
                                            .copied()
                                            .unwrap_or(lichen_core::Pubkey([0; 32]));
                                        // Parse args from JSON bytes
                                        let parsed =
                                            serde_json::from_slice::<serde_json::Value>(&args)
                                                .unwrap_or_default();
                                        let amount = parsed
                                            .get("amount")
                                            .and_then(|v| {
                                                v.as_u64().or_else(|| {
                                                    v.as_str().and_then(|s| s.parse().ok())
                                                })
                                            })
                                            .unwrap_or(0);
                                        let dest_chain = parsed
                                            .get("dest_chain")
                                            .or_else(|| parsed.get("chain"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown")
                                            .to_string();
                                        let asset = parsed
                                            .get("asset")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("lichen")
                                            .to_string();
                                        let _ =
                                            ws_event_tx.send(lichen_rpc::ws::Event::BridgeLock {
                                                chain: dest_chain,
                                                asset,
                                                amount,
                                                sender,
                                                recipient,
                                            });
                                    }
                                    "mint" | "bridge_mint" => {
                                        let recipient = ix
                                            .accounts
                                            .get(1)
                                            .copied()
                                            .unwrap_or(lichen_core::Pubkey([0; 32]));
                                        // Parse args from JSON bytes
                                        let parsed =
                                            serde_json::from_slice::<serde_json::Value>(&args)
                                                .unwrap_or_default();
                                        let amount = parsed
                                            .get("amount")
                                            .and_then(|v| {
                                                v.as_u64().or_else(|| {
                                                    v.as_str().and_then(|s| s.parse().ok())
                                                })
                                            })
                                            .unwrap_or(0);
                                        let source_chain = parsed
                                            .get("source_chain")
                                            .or_else(|| parsed.get("chain"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown")
                                            .to_string();
                                        let asset = parsed
                                            .get("asset")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("musd")
                                            .to_string();
                                        let tx_hash = hex::encode(tx.signature().0);
                                        let _ =
                                            ws_event_tx.send(lichen_rpc::ws::Event::BridgeMint {
                                                chain: source_chain,
                                                asset,
                                                amount,
                                                recipient,
                                                tx_hash,
                                            });
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Default)]
struct SnapshotSync {
    validator_set: bool,
    stake_pool: bool,
}

const MIN_WARP_CHECKPOINT_ANCHOR_PEERS: usize = 2;

impl SnapshotSync {
    fn new(is_joining_network: bool) -> Self {
        if is_joining_network {
            Self::default()
        } else {
            Self {
                validator_set: true,
                stake_pool: true,
            }
        }
    }

    fn is_ready(&self) -> bool {
        self.validator_set && self.stake_pool
    }
}

fn block_vote_weight(
    slot: u64,
    block_hash: &Hash,
    vote_agg: &VoteAggregator,
    validator_set: &ValidatorSet,
    stake_pool: &StakePool,
) -> u64 {
    if let Some(votes) = vote_agg.get_votes(slot, block_hash) {
        let total_stake = stake_pool.total_stake();
        if total_stake == 0 {
            return votes
                .iter()
                .filter_map(|vote| validator_set.get_validator(&vote.validator))
                .map(|v| v.voting_weight())
                .sum();
        }

        return votes
            .iter()
            .filter_map(|vote| stake_pool.get_stake(&vote.validator))
            .map(|stake_info| stake_info.total_stake())
            .sum();
    }

    0
}

fn block_fee_at_index(
    state: &StateStore,
    block: &Block,
    tx_index: usize,
    fee_config: &FeeConfig,
) -> u64 {
    let Some(tx) = block.transactions.get(tx_index) else {
        return 0;
    };

    if let Some(exact_fee) = TxProcessor::exact_transaction_fee_from_state(state, tx, fee_config) {
        return exact_fee;
    }

    let fallback_fee = TxProcessor::compute_transaction_fee(tx, fee_config);
    if block.tx_fees_paid.len() == block.transactions.len() {
        block
            .tx_fees_paid
            .get(tx_index)
            .copied()
            .unwrap_or(fallback_fee)
    } else {
        fallback_fee
    }
}

fn block_total_fees_paid(state: &StateStore, block: &Block, fee_config: &FeeConfig) -> u64 {
    block
        .transactions
        .iter()
        .enumerate()
        .map(|(tx_index, tx)| {
            TxProcessor::exact_transaction_fee_from_state(state, tx, fee_config).unwrap_or_else(
                || {
                    if block.tx_fees_paid.len() == block.transactions.len() {
                        block
                            .tx_fees_paid
                            .get(tx_index)
                            .copied()
                            .unwrap_or_else(|| TxProcessor::compute_transaction_fee(tx, fee_config))
                    } else {
                        TxProcessor::compute_transaction_fee(tx, fee_config)
                    }
                },
            )
        })
        .sum()
}

// =========================================================================
//  CONSENSUS ORACLE MIRROR — Deterministic derived state from finalized prices
//
//  Validators submit signed native oracle attestation transactions. After a
//  block executes, every validator reads the same finalized consensus oracle
//  state and mirrors it into legacy contract storage layouts used by DEX,
//  analytics, and compatibility surfaces.
// =========================================================================

/// Apply consensus-oracle derived state after a block is processed.
///
/// Called on ALL validators after `apply_block_effects`. This function does
/// not trust proposer-carried oracle payloads. Instead, it reads the native
/// consensus oracle state finalized by the block's transactions and mirrors
/// that data into legacy contract storage layouts.
fn apply_oracle_from_block(state: &StateStore, block: &Block) {
    if block.header.slot == 0 {
        return;
    }

    let slot = block.header.slot;
    let now_ts = block.header.timestamp;

    // Resolve contract pubkeys via symbol registry
    let oracle_pk = match state.get_symbol_registry("ORACLE") {
        Ok(Some(entry)) => entry.program,
        _ => return,
    };
    let analytics_pk = match state.get_symbol_registry("ANALYTICS") {
        Ok(Some(entry)) => entry.program,
        _ => return,
    };
    let dex_pk = match state.get_symbol_registry("DEX") {
        Ok(Some(entry)) => entry.program,
        _ => Pubkey([0u8; 32]),
    };
    let feeder = match state.get_genesis_pubkey() {
        Ok(Some(gpk)) => gpk.0,
        _ => return,
    };

    const PRICE_SCALE: u64 = 1_000_000_000; // 1e9 for DEX price scaling
    const ORACLE_DECIMALS: u8 = 8;

    let wsol_usd =
        lichen_core::consensus::consensus_oracle_price_from_state(state, "wSOL").unwrap_or(0.0);
    let weth_usd =
        lichen_core::consensus::consensus_oracle_price_from_state(state, "wETH").unwrap_or(0.0);
    let wbnb_usd =
        lichen_core::consensus::consensus_oracle_price_from_state(state, "wBNB").unwrap_or(0.0);

    if wsol_usd <= 0.0 && weth_usd <= 0.0 && wbnb_usd <= 0.0 {
        return;
    }

    let licn_usd = lichen_core::consensus::licn_price_from_state(state);

    // ── Phase A: Mirror consensus prices into ORACLE compatibility storage ──
    for asset in ["LICN", "wSOL", "wETH", "wBNB"] {
        let consensus_feed =
            lichen_core::consensus::read_consensus_oracle_price_from_state(state, asset)
                .map(|(price_raw, decimals, _)| (price_raw, decimals))
                .or_else(|| {
                    if asset == "LICN" {
                        Some((genesis_licn_price_8dec(), ORACLE_DECIMALS))
                    } else {
                        None
                    }
                });
        let Some((price_raw, decimals)) = consensus_feed else {
            continue;
        };

        // Build 49-byte oracle feed: price(8)+timestamp(8)+decimals(1)+feeder(32)
        let mut feed = Vec::with_capacity(49);
        feed.extend_from_slice(&price_raw.to_le_bytes());
        feed.extend_from_slice(&now_ts.to_le_bytes());
        feed.push(decimals);
        feed.extend_from_slice(&feeder);

        let price_key = format!("price_{}", asset);
        let _ = state.put_contract_storage(&oracle_pk, price_key.as_bytes(), &feed);

        // Also write indexed key for aggregation
        let indexed_key = format!("{}_0", price_key);
        let _ = state.put_contract_storage(&oracle_pk, indexed_key.as_bytes(), &feed);
    }

    // ── Phase B: Write DEX price bands to DEX contract ──
    // dex_band_{pair_id}: 16 bytes = reference_price(8) + slot(8)
    // The dex_core contract reads this during place_order to enforce
    // ±5% (market) / ±10% (limit) price band protection.
    let pair_prices: [(u64, f64); 7] = [
        (1, licn_usd),
        (2, wsol_usd),
        (3, weth_usd),
        (
            4,
            if licn_usd > 0.0 {
                wsol_usd / licn_usd
            } else {
                0.0
            },
        ),
        (
            5,
            if licn_usd > 0.0 {
                weth_usd / licn_usd
            } else {
                0.0
            },
        ),
        (6, wbnb_usd),
        (
            7,
            if licn_usd > 0.0 {
                wbnb_usd / licn_usd
            } else {
                0.0
            },
        ),
    ];

    if dex_pk.0 != [0u8; 32] {
        for (pair_id, price_f64) in &pair_prices {
            if *price_f64 <= 0.0 {
                continue;
            }
            let price_scaled = (*price_f64 * PRICE_SCALE as f64) as u64;
            let band_key = format!("dex_band_{}", pair_id);
            let mut band_data = Vec::with_capacity(16);
            band_data.extend_from_slice(&price_scaled.to_le_bytes());
            band_data.extend_from_slice(&slot.to_le_bytes());
            let _ = state.put_contract_storage(&dex_pk, band_key.as_bytes(), &band_data);
        }
    }

    // ── Phase C: Write analytics indicative prices + CANDLES ──
    // Every validator reads the same finalized consensus oracle prices and
    // writes identical derived analytics state.
    // Candle intervals: 1m, 5m, 15m, 1h, 4h, 1d, 3d, 1w, 1y
    const CANDLE_INTERVALS: [u64; 9] = [60, 300, 900, 3600, 14400, 86400, 259200, 604800, 31536000];

    for (pair_id, price_f64) in &pair_prices {
        if *price_f64 <= 0.0 {
            continue;
        }
        let price_scaled = (*price_f64 * PRICE_SCALE as f64) as u64;

        // Check if a real trade occurred within 60 seconds
        let ts_key = format!("ana_last_trade_ts_{}", pair_id);
        let last_trade_ts: u64 = match state.get_contract_storage(&analytics_pk, ts_key.as_bytes())
        {
            Ok(Some(d)) if d.len() >= 8 => u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])),
            _ => 0,
        };
        let trade_active = last_trade_ts > 0 && now_ts.saturating_sub(last_trade_ts) < 60;

        if trade_active {
            continue; // Active market: trades drive displayed prices + candles
        }

        // Inactive market: write indicative price from oracle
        let lp_key = format!("ana_lp_{}", pair_id);
        let _ = state.put_contract_storage(
            &analytics_pk,
            lp_key.as_bytes(),
            &price_scaled.to_le_bytes(),
        );

        // Update 24h stats (read-modify-write)
        let stats_key = format!("ana_24h_{}", pair_id);
        let (vol, mut high, mut low, open, _close, trades) =
            match state.get_contract_storage(&analytics_pk, stats_key.as_bytes()) {
                Ok(Some(d)) if d.len() >= 48 => (
                    u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])),
                    u64::from_le_bytes(d[8..16].try_into().unwrap_or([0; 8])),
                    u64::from_le_bytes(d[16..24].try_into().unwrap_or([0; 8])),
                    u64::from_le_bytes(d[24..32].try_into().unwrap_or([0; 8])),
                    u64::from_le_bytes(d[32..40].try_into().unwrap_or([0; 8])),
                    u64::from_le_bytes(d[40..48].try_into().unwrap_or([0; 8])),
                ),
                _ => (0, 0, u64::MAX, price_scaled, price_scaled, 0),
            };

        if price_scaled > high {
            high = price_scaled;
        }
        if price_scaled < low {
            low = price_scaled;
        }

        let mut stats = Vec::with_capacity(48);
        stats.extend_from_slice(&vol.to_le_bytes());
        stats.extend_from_slice(&high.to_le_bytes());
        stats.extend_from_slice(&low.to_le_bytes());
        stats.extend_from_slice(&open.to_le_bytes());
        stats.extend_from_slice(&price_scaled.to_le_bytes()); // close = current
        stats.extend_from_slice(&trades.to_le_bytes());
        let _ = state.put_contract_storage(&analytics_pk, stats_key.as_bytes(), &stats);

        // ── Candles: update all 9 intervals with oracle price ──
        // This is consensus-deterministic: every validator processing this block
        // writes the same candle data from the same oracle prices.
        for &interval in &CANDLE_INTERVALS {
            oracle_update_candle(
                state,
                &analytics_pk,
                *pair_id,
                interval,
                price_scaled,
                slot,
                now_ts,
            );
        }
    }
}

/// Replay transactions from a received P2P block to update local state.
/// The producing validator already executed these transactions; receivers
/// must replay them so that fee charges and balance mutations are applied
/// identically, preventing state divergence across the network.
/// Compute BFT timestamp for a new block proposal.
///
/// Looks up the parent block from state and computes the stake-weighted
/// median of its commit vote timestamps (CometBFT BFT Time model).
/// Falls back to wall-clock time if the parent has no commit signatures
/// (genesis or first blocks before BFT activation).
fn compute_proposed_timestamp(
    state: &StateStore,
    parent_hash: &Hash,
    validator_set: &ValidatorSet,
    stake_pool: &StakePool,
) -> Option<u64> {
    // Find parent block by hash
    let parent_slot = match state.get_last_slot() {
        Ok(s) => s,
        Err(_) => return None,
    };
    // Search recent blocks for the parent
    let parent_block = if parent_slot == 0 {
        state.get_block_by_slot(0).ok().flatten()
    } else {
        // The parent is at parent_slot (tip)
        let block = state.get_block_by_slot(parent_slot).ok().flatten();
        match block {
            Some(b) if b.hash() == *parent_hash => Some(b),
            _ => None,
        }
    };

    let parent = parent_block?;

    if parent.commit_signatures.is_empty() {
        return None;
    }

    let bft_ts = compute_bft_timestamp(
        &parent.commit_signatures,
        validator_set,
        stake_pool,
        Some(parent.header.timestamp),
    )?;

    // Clamp BFT timestamp to wall-clock + 1s to prevent future-drift blocks.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Some(bft_ts.min(now + 1))
}

/// Genesis-block transactions (slot 0) are created with special
/// signatures and a zero blockhash, so they cannot pass the normal
/// validation pipeline — the genesis state was applied directly.
///
/// Uses parallel processing (rayon) for non-conflicting TXs to speed up
/// block replay during chain sync (FIX-2).
fn replay_block_transactions(processor: &TxProcessor, block: &Block) {
    if block.header.slot == 0 {
        return; // genesis txs use zero blockhash + dummy signatures
    }
    let producer_pubkey = Pubkey(block.header.validator);
    let tx_count = block.transactions.len();
    if tx_count > 0 {
        info!(
            "🔄 Replaying {} tx(s) for slot {} (producer={})",
            tx_count,
            block.header.slot,
            producer_pubkey.to_base58()
        );
    }
    let results = processor.process_transactions_parallel(&block.transactions, &producer_pubkey);
    let exact_fees_present = block.tx_fees_paid.len() == block.transactions.len();
    for (tx, result) in block.transactions.iter().zip(results.iter()) {
        if result.success {
            info!(
                "✅ Tx replay OK in slot {}: {}",
                block.header.slot,
                tx.signature().to_hex()
            );
        } else {
            warn!(
                "⚠️  Tx replay failed in slot {}: {} ({})",
                block.header.slot,
                tx.signature().to_hex(),
                result.error.as_deref().unwrap_or_default()
            );
        }
    }

    if exact_fees_present {
        for (tx_index, result) in results.iter().enumerate() {
            if let Some(block_fee) = block.tx_fees_paid.get(tx_index) {
                if *block_fee != result.fee_paid {
                    warn!(
                        "⚠️  Slot {} tx {} fee metadata mismatch: block={} local={}",
                        block.header.slot, tx_index, block_fee, result.fee_paid,
                    );
                }
            }
        }
    } else if !block.tx_fees_paid.is_empty() {
        warn!(
            "⚠️  Slot {} fee metadata length mismatch: block has {} entries for {} transactions",
            block.header.slot,
            block.tx_fees_paid.len(),
            block.transactions.len(),
        );
    }
}

/// Reverse the financial effects of a replaced block during fork choice.
/// Attempts to debit the old producer's reward and credit treasury back.
/// Fee distribution reversal is approximate — voter shares remain (small
/// amounts relative to block reward). This prevents the worst case of the
/// wrong producer keeping an entire block reward.
async fn revert_block_effects(
    state: &StateStore,
    validator_set: &Arc<RwLock<ValidatorSet>>,
    stake_pool: &Arc<RwLock<StakePool>>,
    old_block: &Block,
) {
    // AUDIT-FIX 2.20: Read-all → compute-all → write-all pattern to prevent
    // TOCTOU races from concurrent revert/apply operations.
    let old_producer = Pubkey(old_block.header.validator);
    let slot = old_block.header.slot;

    // Phase 1: Read all needed state
    let treasury_pubkey = match state.get_treasury_pubkey() {
        Ok(Some(pk)) => pk,
        _ => {
            warn!("revert_block_effects: no treasury pubkey, skipping");
            return;
        }
    };

    let mut producer_account = match state.get_account(&old_producer) {
        Ok(Some(acc)) => acc,
        _ => {
            warn!("revert_block_effects: producer account not found, skipping");
            return;
        }
    };

    let mut treasury_account = match state.get_account(&treasury_pubkey) {
        Ok(Some(acc)) => acc,
        _ => {
            warn!("revert_block_effects: treasury account not found, skipping");
            return;
        }
    };

    // NOTE: No per-slot inflation reward reversal needed — inflation is now
    // distributed at epoch boundaries, not per-block. Only fee reversal applies.

    // Compute fee reversal — fees are still treasury-sourced
    let fee_config = state
        .get_fee_config()
        .unwrap_or_else(|_| lichen_core::FeeConfig::default_from_constants());
    let total_fee = block_total_fees_paid(state, old_block, &fee_config);

    if total_fee > 0 {
        let producer_share = total_fee * fee_config.fee_producer_percent / 100;
        if producer_share > 0 {
            let fee_debit = producer_share.min(producer_account.spendable);
            producer_account.spores = producer_account.spores.saturating_sub(fee_debit);
            producer_account.spendable = producer_account.spendable.saturating_sub(fee_debit);
            treasury_account.spores = treasury_account.spores.saturating_add(fee_debit);
            treasury_account.spendable = treasury_account.spendable.saturating_add(fee_debit);
        }
    }

    // Phase 3: Write all changes atomically via batch
    let mut batch = state.begin_batch();
    if let Err(e) = batch.put_account(&old_producer, &producer_account) {
        warn!("revert_block_effects: failed to batch-put producer: {}", e);
    }
    if let Err(e) = batch.put_account(&treasury_pubkey, &treasury_account) {
        warn!("revert_block_effects: failed to batch-put treasury: {}", e);
    }
    if let Err(e) = state.commit_batch(batch) {
        warn!("revert_block_effects: failed to commit batch: {}", e);
    }

    // Clear distribution hashes so apply_block_effects can run for the new block
    if let Err(e) = state.clear_reward_distribution_hash(slot) {
        warn!(
            "revert_block_effects: failed to clear reward hash for slot {}: {}",
            slot, e
        );
    }
    if let Err(e) = state.clear_fee_distribution_hash(slot) {
        warn!(
            "revert_block_effects: failed to clear fee hash for slot {}: {}",
            slot, e
        );
    }

    // Keep validator production counters aligned with canonical chain.
    {
        let mut vs = validator_set.write().await;
        if let Some(val_info) = vs.get_validator_mut(&old_producer) {
            val_info.blocks_proposed = val_info.blocks_proposed.saturating_sub(1);
        }

        let vs_snapshot = vs.clone();
        drop(vs);
        if let Err(e) = state.save_validator_set(&vs_snapshot) {
            warn!(
                "⚠️  Failed to persist validator set counter revert for {}: {}",
                old_producer.to_base58(),
                e
            );
        }
    }

    {
        let mut pool = stake_pool.write().await;
        if let Some(stake_info) = pool.get_stake_mut(&old_producer) {
            stake_info.blocks_produced = stake_info.blocks_produced.saturating_sub(1);
        }

        let pool_snapshot = pool.clone();
        drop(pool);
        if let Err(e) = state.put_stake_pool(&pool_snapshot) {
            warn!(
                "⚠️  Failed to persist stake pool counter revert for {}: {}",
                old_producer.to_base58(),
                e
            );
        }
    }

    info!(
        "⚖️  Reverted block effects for slot {} (old producer: {})",
        slot,
        old_producer.to_base58()
    );
}

/// C7 fix: Reverse user transaction effects of a replaced block during fork choice.
/// For each transaction: reverse transfer instructions, refund fees, remove tx record
/// so the new block's transactions can be properly replayed.
/// For non-revertible instructions (contract calls, NFT, staking), attempts to
/// restore affected accounts from the nearest RocksDB checkpoint.
fn revert_block_transactions(state: &StateStore, old_block: &Block, data_dir: &str) {
    use lichen_core::SYSTEM_PROGRAM_ID;

    if old_block.header.slot == 0 {
        return;
    }

    let fee_config = state
        .get_fee_config()
        .unwrap_or_else(|_| lichen_core::FeeConfig::default_from_constants());

    // AUDIT-FIX C7: Collect accounts touched by non-revertible instructions
    // so we can restore them from checkpoint if needed.
    let mut non_revertible_accounts: Vec<lichen_core::Pubkey> = Vec::new();

    for (tx_index, tx) in old_block.transactions.iter().enumerate().rev() {
        // AUDIT-FIX 0.5: Detect non-system-transfer instructions that can't be reverted
        let has_non_revertible = tx.message.instructions.iter().any(|ix| {
            if ix.program_id != SYSTEM_PROGRAM_ID {
                return true; // Contract call — can't revert
            }
            if ix.data.is_empty() {
                return false;
            }
            // Only types 0,2,3,4,5 (transfers) are revertible
            !matches!(ix.data[0], 0 | 2 | 3 | 4 | 5)
        });
        if has_non_revertible {
            // AUDIT-FIX C7: Collect all accounts from non-revertible instructions
            // for checkpoint-based restoration instead of best-effort field reversal.
            for ix in &tx.message.instructions {
                if ix.program_id != SYSTEM_PROGRAM_ID
                    || (!ix.data.is_empty() && !matches!(ix.data[0], 0 | 2 | 3 | 4 | 5))
                {
                    for acct in &ix.accounts {
                        non_revertible_accounts.push(*acct);
                    }
                    // Also include the contract/program itself
                    non_revertible_accounts.push(ix.program_id);
                }
            }
            warn!(
                "⚠️  Block {} contains non-revertible instructions — will restore from checkpoint. Tx: {}",
                old_block.header.slot,
                tx.hash().to_hex()
            );
        }

        // 1. Reverse each system transfer instruction
        // L4-01 fix: collect all account mutations in an overlay, then flush
        // them atomically via a single WriteBatch to prevent partial reversals.
        let mut overlay: HashMap<lichen_core::Pubkey, Account> = HashMap::new();

        for ix in &tx.message.instructions {
            if ix.program_id == SYSTEM_PROGRAM_ID && !ix.data.is_empty() {
                let ix_type = ix.data[0];
                // Types 0,2,3,4,5 are all transfers
                if matches!(ix_type, 0 | 2 | 3 | 4 | 5)
                    && ix.accounts.len() >= 2
                    && ix.data.len() >= 9
                {
                    let from = ix.accounts[0]; // original sender
                    let to = ix.accounts[1]; // original receiver
                    let amount_bytes: [u8; 8] = match ix.data[1..9].try_into() {
                        Ok(b) => b,
                        Err(_) => continue,
                    };
                    let amount = u64::from_le_bytes(amount_bytes);

                    // Reverse: credit sender, debit receiver
                    if amount > 0 {
                        let receiver = overlay.entry(to).or_insert_with(|| {
                            state
                                .get_account(&to)
                                .ok()
                                .flatten()
                                .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER))
                        });
                        let debit = amount.min(receiver.spendable);
                        receiver.spores = receiver.spores.saturating_sub(debit);
                        receiver.spendable = receiver.spendable.saturating_sub(debit);

                        let sender = overlay.entry(from).or_insert_with(|| {
                            state
                                .get_account(&from)
                                .ok()
                                .flatten()
                                .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER))
                        });
                        sender.spores = sender.spores.saturating_add(debit);
                        sender.spendable = sender.spendable.saturating_add(debit);
                    }
                }
            }
        }

        // 2. Refund fee to fee payer
        if let Some(first_ix) = tx.message.instructions.first() {
            if let Some(&fee_payer) = first_ix.accounts.first() {
                let fee = block_fee_at_index(state, old_block, tx_index, &fee_config);
                if fee > 0 {
                    let payer = overlay.entry(fee_payer).or_insert_with(|| {
                        state
                            .get_account(&fee_payer)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER))
                    });
                    payer.spores = payer.spores.saturating_add(fee);
                    payer.spendable = payer.spendable.saturating_add(fee);
                }
            }
        }

        // Flush all modified accounts atomically
        if !overlay.is_empty() {
            let batch_accounts: Vec<(&lichen_core::Pubkey, &Account)> = overlay.iter().collect();
            if let Err(e) = state.atomic_put_accounts(&batch_accounts, 0) {
                warn!("⚠️  Failed to atomically revert tx accounts: {}", e);
            }
        }

        // 3. Remove transaction record so new block's txs can be replayed
        let tx_hash = tx.hash();
        state.delete_transaction(&tx_hash).ok();
    }

    // AUDIT-FIX C7: Restore non-revertible accounts from nearest checkpoint.
    // This ensures contract storage, NFT state, staking mutations, etc.
    // are properly rolled back during a fork switch.
    if !non_revertible_accounts.is_empty() {
        non_revertible_accounts.sort_by(|a, b| a.0.cmp(&b.0));
        non_revertible_accounts.dedup();

        // Find the nearest checkpoint at or below the reverted block's slot
        let checkpoints = StateStore::list_checkpoints(data_dir);
        let nearest = checkpoints
            .iter()
            .rev()
            .find(|(cp_slot, _)| *cp_slot < old_block.header.slot);

        if let Some((cp_slot, cp_path)) = nearest {
            match StateStore::open_checkpoint(cp_path) {
                Ok(checkpoint_store) => {
                    // L4-01 fix: collect all restored accounts, then flush atomically
                    let mut restore_accounts: Vec<(lichen_core::Pubkey, Account)> = Vec::new();
                    let mut skipped = 0usize;
                    for acct_key in &non_revertible_accounts {
                        match checkpoint_store.get_account(acct_key) {
                            Ok(Some(cp_account)) => {
                                restore_accounts.push((*acct_key, cp_account));
                            }
                            Ok(None) => {
                                // Account didn't exist at checkpoint time — zero it out
                                // (it was created by the reverted block's contract call)
                                let zeroed = lichen_core::Account {
                                    spores: 0,
                                    spendable: 0,
                                    staked: 0,
                                    locked: 0,
                                    data: Vec::new(),
                                    owner: SYSTEM_ACCOUNT_OWNER,
                                    executable: false,
                                    rent_epoch: 0,
                                    dormant: false,
                                    missed_rent_epochs: 0,
                                };
                                restore_accounts.push((*acct_key, zeroed));
                            }
                            Err(e) => {
                                warn!(
                                    "⚠️  Failed to read account {} from checkpoint: {}",
                                    acct_key.to_base58(),
                                    e
                                );
                                skipped += 1;
                            }
                        }
                    }
                    if !restore_accounts.is_empty() {
                        let batch_refs: Vec<(&lichen_core::Pubkey, &Account)> =
                            restore_accounts.iter().map(|(k, v)| (k, v)).collect();
                        match state.atomic_put_accounts(&batch_refs, 0) {
                            Ok(()) => {
                                info!(
                                    "🔄 AUDIT-FIX C7+L4-01: Atomically restored {}/{} non-revertible accounts from checkpoint slot {}{}",
                                    restore_accounts.len(), non_revertible_accounts.len(), cp_slot,
                                    if skipped > 0 { format!(" ({} skipped)", skipped) } else { String::new() }
                                );
                            }
                            Err(e) => {
                                error!("🚨 CRITICAL: Atomic checkpoint restore failed: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(
                        "🚨 CRITICAL: Failed to open checkpoint at {} for fork-switch account restoration: {}",
                        cp_path, e
                    );
                }
            }
        } else {
            error!(
                "🚨 CRITICAL: No checkpoint available before slot {} for fork-switch restoration. \
                 {} accounts may have inconsistent state from non-revertible instructions.",
                old_block.header.slot,
                non_revertible_accounts.len()
            );
        }
    }

    info!(
        "⚖️  Reverted {} user transactions for slot {}{}",
        old_block.transactions.len(),
        old_block.header.slot,
        if non_revertible_accounts.is_empty() {
            String::new()
        } else {
            format!(
                " (restored {} accounts from checkpoint)",
                non_revertible_accounts.len()
            )
        }
    );
}

async fn apply_block_effects(
    state: &StateStore,
    validator_set: &Arc<RwLock<ValidatorSet>>,
    stake_pool: &Arc<RwLock<StakePool>>,
    vote_aggregator: &Arc<RwLock<VoteAggregator>>,
    block: &Block,
    skip_rewards: bool,
    _min_validator_stake: u64,
) {
    if block.header.slot == 0 || block.header.validator == [0u8; 32] {
        return;
    }

    // Reload in-memory stake pool from on-chain state to pick up effects
    // from consensus-processed transactions (e.g., RegisterValidator opcode 26,
    // Stake, Unstake). Without this, the in-memory pool would miss changes
    // applied by TxProcessor during block transaction processing.
    if let Ok(fresh_pool) = state.get_stake_pool() {
        let entry_count = fresh_pool.stake_entries().len();
        let mut pool = stake_pool.write().await;
        *pool = fresh_pool;
        drop(pool);
        if block_has_user_transactions(block) || entry_count > 1 {
            info!(
                "📊 apply_block_effects slot {}: reloaded pool with {} entries from state",
                block.header.slot, entry_count
            );
        }
    }

    let producer = Pubkey(block.header.validator);
    let slot = block.header.slot;

    let has_user_transactions = block_has_user_transactions(block);
    let is_heartbeat = !has_user_transactions;
    let reward_already = if !skip_rewards {
        match state.get_reward_distribution_hash(slot) {
            Ok(Some(_)) => true, // per-slot guard: any reward for this slot = skip
            Ok(None) => false,
            Err(e) => {
                warn!("⚠️  Failed to read reward distribution hash: {}", e);
                false
            }
        }
    } else {
        false
    };

    let stake_amount = {
        let pool = stake_pool.read().await;
        pool.get_stake(&producer)
            .map(|stake_info| stake_info.total_stake())
            .unwrap_or(0)
    };

    {
        let mut vs = validator_set.write().await;
        if let Some(val_info) = vs.get_validator_mut(&producer) {
            if !reward_already {
                val_info.blocks_proposed += 1;
                val_info.transactions_processed += block.transactions.len() as u64;
            }
            val_info.last_active_slot = slot;
            val_info.update_reputation(true);
        } else {
            // Header-first sync can observe legitimate historical producers
            // before their RegisterValidator transaction is replayed locally.
            // Track the producer for activity/reputation, but never infer
            // bootstrap stake or local voting power from that observation.
            let new_validator = make_sync_observed_validator_info(
                producer,
                slot,
                stake_amount,
                block.transactions.len(),
                reward_already,
            );
            vs.add_validator(new_validator);
        }

        // PERF-OPT 4: Clone under lock, persist AFTER dropping write guard.
        // This frees the RwLock while RocksDB I/O runs, unblocking all readers.
        let vs_snapshot = vs.clone();
        drop(vs);
        if let Err(e) = state.save_validator_set(&vs_snapshot) {
            warn!("⚠️  Failed to persist validator set update: {}", e);
        }
    }

    // ── Protocol-level epoch rewards (Solana model) ───────────────────
    // Inflation is NOT distributed per-slot. Instead, at each epoch boundary,
    // the total epoch mint is computed and distributed to ALL active stakers
    // proportionally by stake weight. Block producers still earn tx fees per-block.
    // Every validator deterministically applies the same rewards.
    let block_hash = block.hash();
    if !skip_rewards && !reward_already {
        // Write the reward distribution guard hash FIRST, before any account
        // modifications.  If we crash after this write but before crediting
        // accounts the worst case is a single slot's rewards are lost (minor,
        // self-correcting).  The old order (hash AFTER credits) risked
        // double-crediting on restart — an inflation bug.
        if let Err(e) = state.set_reward_distribution_hash(slot, &block_hash) {
            warn!(
                "⚠️  Failed to record reward distribution for slot {}: {}",
                slot, e
            );
        }

        // Compute total supply for inflation curve: genesis + minted - burned
        let total_supply = GENESIS_SUPPLY_SPORES
            .saturating_add(state.get_total_minted().unwrap_or(0))
            .saturating_sub(state.get_total_burned().unwrap_or(0));

        // ── Per-block: record block production (no per-slot inflation) ──
        // Inflation is distributed at epoch boundaries to ALL stakers proportionally.
        // Block producers still earn transaction fees per-block (below).
        {
            let mut pool = stake_pool.write().await;
            // distribute_block_reward now only updates last_reward_slot (returns 0)
            pool.distribute_block_reward(&producer, slot, is_heartbeat, total_supply);
            pool.record_block_produced(&producer);
            let pool_snapshot = pool.clone();
            drop(pool);
            if let Err(e) = state.put_stake_pool(&pool_snapshot) {
                warn!(
                    "⚠️  Failed to persist stake pool block-production update: {}",
                    e
                );
            }
        }

        // ── Epoch boundary: distribute inflation to ALL stakers proportionally ──
        // At the start of each new epoch, mint the previous epoch's inflation
        // and distribute to every active staker by stake weight, routed through
        // the vesting pipeline (bootstrap debt repayment).
        if lichen_core::is_epoch_boundary(slot) && slot > 0 {
            let completed_epoch_start = lichen_core::epoch_start_slot(
                lichen_core::consensus::slot_to_epoch(slot).saturating_sub(1),
            );
            let epoch_mint = lichen_core::compute_epoch_mint(completed_epoch_start, total_supply);
            let moss_reward_pool = match state.get_mossstake_pool() {
                Ok(moss_pool) if moss_pool.st_licn_token.total_supply > 0 => {
                    let (_, moss_reward_pool) = lichen_core::consensus::split_epoch_mint(
                        completed_epoch_start,
                        total_supply,
                    );
                    moss_reward_pool
                }
                Ok(_) => 0,
                Err(e) => {
                    warn!(
                        "⚠️  Failed to load MossStake pool before epoch distribution: {}",
                        e
                    );
                    0
                }
            };
            let staker_reward_pool = if moss_reward_pool > 0 {
                epoch_mint.saturating_sub(moss_reward_pool)
            } else {
                epoch_mint
            };

            let (total_minted, distributions) = {
                let mut pool = stake_pool.write().await;
                let result = pool.distribute_epoch_staker_rewards_from_pool(
                    staker_reward_pool,
                    completed_epoch_start,
                );
                let pool_snapshot = pool.clone();
                drop(pool);
                if let Err(e) = state.put_stake_pool(&pool_snapshot) {
                    warn!(
                        "⚠️  Failed to persist stake pool epoch reward update: {}",
                        e
                    );
                }
                result
            };

            if total_minted > 0 {
                // Credit each validator's liquid reward to their on-chain account
                let mut mint_pairs: Vec<(Pubkey, Account)> = Vec::new();
                for (validator_pk, _reward, liquid, _debt_payment) in &distributions {
                    if *liquid > 0 {
                        let mut account = state
                            .get_account(validator_pk)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));
                        account.add_spendable(*liquid).unwrap_or_else(|e| {
                            warn!(
                                "⚠️  Overflow crediting epoch reward to {}: {}",
                                validator_pk, e
                            );
                        });
                        mint_pairs.push((*validator_pk, account));
                    }
                }

                // Build reference slice for atomic_mint_accounts
                let refs: Vec<(&Pubkey, &Account)> =
                    mint_pairs.iter().map(|(pk, acc)| (pk, acc)).collect();
                if let Err(e) = state.atomic_mint_accounts(&refs, total_minted) {
                    warn!("⚠️  Failed to persist epoch staker rewards: {}", e);
                }

                let epoch = lichen_core::consensus::slot_to_epoch(slot);
                info!(
                    "🏛️  Epoch {} rewards: minted {:.3} LICN to {} stakers",
                    epoch.saturating_sub(1),
                    total_minted as f64 / 1_000_000_000.0,
                    distributions.len(),
                );
                for (pk, reward, liquid, debt) in &distributions {
                    debug!(
                        "  └─ {} : reward {:.6}, liquid {:.6}, debt {:.6}",
                        pk,
                        *reward as f64 / 1_000_000_000.0,
                        *liquid as f64 / 1_000_000_000.0,
                        *debt as f64 / 1_000_000_000.0,
                    );
                }

                // ── MossStake liquid staking reward distribution ──
                // Allocate MOSSSTAKE_BLOCK_SHARE_BPS (10%) of epoch inflation
                // to the MossStake pool, funding stLICN yield.
                if moss_reward_pool > 0 {
                    match state.get_mossstake_pool() {
                        Ok(mut moss_pool) => {
                            if moss_pool.st_licn_token.total_supply > 0 {
                                moss_pool.distribute_rewards(moss_reward_pool);
                                if let Err(e) =
                                    state.atomic_mint_mossstake(&moss_pool, moss_reward_pool)
                                {
                                    warn!(
                                        "⚠️  Failed to persist MossStake epoch distribution: {}",
                                        e
                                    );
                                } else {
                                    debug!(
                                        "🌊 MossStake: minted {:.6} LICN to {} stakers (epoch)",
                                        moss_reward_pool as f64 / 1_000_000_000.0,
                                        moss_pool.positions.len(),
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!("⚠️  Failed to load MossStake pool: {}", e);
                        }
                    }
                }
            }

            // ── Apply pending governance parameter changes at epoch boundary ──
            match state.apply_pending_governance_changes() {
                Ok(0) => {} // No pending changes
                Ok(n) => {
                    let epoch = lichen_core::consensus::slot_to_epoch(slot);
                    info!(
                        "🏛️  Epoch {} governance: applied {} parameter change(s)",
                        epoch, n,
                    );
                }
                Err(e) => {
                    warn!("⚠️  Failed to apply governance parameter changes: {}", e);
                }
            }
        }
    }

    let fee_config = state
        .get_fee_config()
        .unwrap_or_else(|_| lichen_core::FeeConfig::default_from_constants());
    let total_fee = block_total_fees_paid(state, block, &fee_config);

    if total_fee == 0 {
        return;
    }

    if let Ok(Some(existing)) = state.get_fee_distribution_hash(slot) {
        if existing == block_hash {
            return;
        }
        warn!(
            "⚠️  Fee distribution already recorded for slot {} with different hash",
            slot
        );
        return;
    }

    let treasury_pubkey = match state.get_treasury_pubkey() {
        Ok(Some(pubkey)) => pubkey,
        _ => {
            warn!("⚠️  Treasury pubkey missing; skipping fee distribution");
            return;
        }
    };

    let mut treasury_account = match state.get_account(&treasury_pubkey) {
        Ok(Some(account)) => account,
        _ => Account::new(0, treasury_pubkey),
    };

    if treasury_account.spores < total_fee {
        warn!(
            "⚠️  Treasury balance {} < total fees {}, skipping distribution",
            treasury_account.spores, total_fee
        );
        return;
    }

    let burn = total_fee * fee_config.fee_burn_percent / 100;
    let producer_share = total_fee * fee_config.fee_producer_percent / 100;
    let voters_share = total_fee * fee_config.fee_voters_percent / 100;
    let community_share = total_fee * fee_config.fee_community_percent / 100;
    let mut voters_paid: u64 = 0;
    let mut fee_liquid: u64 = 0; // actual liquid amount after vesting split

    // NOTE: burn was already applied in charge_fee (processor.rs) during
    // transaction processing.  Do NOT call add_burned again here — that
    // caused a double-burn destroying twice the intended supply.

    // AUDIT-FIX 0.6: All fee distribution writes go through an atomic
    // WriteBatch. Nothing hits disk until commit_batch() succeeds, so a
    // crash mid-distribution cannot leave state half-credited.
    let mut batch = state.begin_batch();

    if producer_share > 0 {
        // Route producer fee share through vesting pipeline (same as block rewards).
        // distribute_fees() → add_reward() → claim_rewards() → vesting split.
        // While bootstrap_debt > 0 only ~50% is liquid; rest repays debt.
        let (liquid, _debt) = {
            let mut pool = stake_pool.write().await;
            let is_active = pool
                .get_stake(&producer)
                .map(|info| info.is_active)
                .unwrap_or(false);
            if is_active {
                pool.distribute_fees(&producer, producer_share, slot);
                let result = pool.claim_rewards(&producer, slot);
                let pool_snapshot = pool.clone();
                drop(pool);
                if let Err(e) = state.put_stake_pool(&pool_snapshot) {
                    warn!("⚠️  Failed to persist stake pool fee update: {}", e);
                }
                result
            } else {
                drop(pool);
                (0u64, 0u64)
            }
        };
        fee_liquid = liquid;

        if fee_liquid > 0 {
            let mut producer_account = match state.get_account(&producer) {
                Ok(Some(account)) => account,
                _ => Account::new(0, SYSTEM_ACCOUNT_OWNER),
            };
            producer_account
                .add_spendable(fee_liquid)
                .unwrap_or_else(|e| {
                    warn!("\u{26a0}\u{fe0f}  Overflow crediting producer fees: {}", e);
                });
            if let Err(e) = batch.put_account(&producer, &producer_account) {
                warn!(
                    "⚠️  Failed to credit producer fees for {}: {}",
                    producer.to_base58(),
                    e
                );
            }
        }
    }

    if voters_share > 0 {
        let voters = {
            let agg = vote_aggregator.read().await;
            match agg.get_votes(slot, &block_hash) {
                Some(votes) => votes.clone(),
                None => Vec::new(),
            }
        };

        let mut voter_pubkeys: Vec<Pubkey> = voters
            .iter()
            .map(|vote| vote.validator)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        // Deterministic ordering is consensus-critical: the last voter
        // receives the integer-rounding remainder, so all validators
        // must iterate in the same order.
        voter_pubkeys.sort_by_key(|pk| pk.0);

        if !voter_pubkeys.is_empty() {
            let pool = stake_pool.read().await;
            // AUDIT-FIX A7-02: Exclude slashed validators from fee distribution
            // Only count stake from non-slashed validators for proportional sharing
            let total_voter_stake: u64 = voter_pubkeys
                .iter()
                .filter(|validator| {
                    pool.get_stake(validator)
                        .map(|s| s.is_active)
                        .unwrap_or(false)
                })
                .filter_map(|validator| pool.get_stake(validator))
                .map(|stake_info| stake_info.total_stake())
                .sum();

            let mut remaining = voters_share;
            for (idx, validator) in voter_pubkeys.iter().enumerate() {
                // AUDIT-FIX A7-02: Skip slashed/inactive validators
                let is_active = pool
                    .get_stake(validator)
                    .map(|s| s.is_active)
                    .unwrap_or(false);
                if !is_active {
                    continue;
                }

                let share = if total_voter_stake > 0 {
                    let stake = pool
                        .get_stake(validator)
                        .map(|stake_info| stake_info.total_stake())
                        .unwrap_or(0);
                    (voters_share * stake / total_voter_stake).min(remaining)
                } else {
                    let remaining_validators = (voter_pubkeys.len() - idx) as u64;
                    (remaining / remaining_validators).min(remaining)
                };

                if share == 0 {
                    continue;
                }

                let mut voter_account = match batch.get_account(validator) {
                    Ok(Some(account)) => account,
                    _ => match state.get_account(validator) {
                        Ok(Some(account)) => account,
                        _ => Account::new(0, SYSTEM_ACCOUNT_OWNER),
                    },
                };
                voter_account.add_spendable(share).unwrap_or_else(|e| {
                    warn!("\u{26a0}\u{fe0f}  Overflow crediting voter fees: {}", e);
                });
                if let Err(e) = batch.put_account(validator, &voter_account) {
                    warn!(
                        "⚠️  Failed to credit voter fees for {}: {}",
                        validator.to_base58(),
                        e
                    );
                }
                remaining = remaining.saturating_sub(share);
                voters_paid = voters_paid.saturating_add(share);
            }
            drop(pool);
        }
    }

    let treasury_share =
        total_fee.saturating_sub(burn + fee_liquid + voters_paid + community_share);

    // Credit community treasury wallet
    if community_share > 0 {
        if let Ok(Some(community_pubkey)) = state.get_community_treasury_pubkey() {
            let mut community_account = match batch.get_account(&community_pubkey) {
                Ok(Some(account)) => account,
                _ => match state.get_account(&community_pubkey) {
                    Ok(Some(account)) => account,
                    _ => Account::new(0, SYSTEM_ACCOUNT_OWNER),
                },
            };
            community_account
                .add_spendable(community_share)
                .unwrap_or_else(|e| {
                    warn!("⚠️  Overflow crediting community treasury fees: {}", e);
                });
            if let Err(e) = batch.put_account(&community_pubkey, &community_account) {
                warn!("⚠️  Failed to credit community treasury fees: {}", e);
            }
        } else {
            warn!("⚠️  Community treasury pubkey not found — community share stays in validator_rewards");
        }
    }

    // charge_fee credited treasury with (fee − burn) for each tx.
    // We debit what we're distributing out: fee_liquid + voters_paid + community_share.
    // fee_liquid is the vesting-adjusted producer share (≤ producer_share).
    // The debt repayment portion stays in treasury as internal bookkeeping.
    // Treasury retains its own share (≈10%) automatically.
    treasury_account.spores = treasury_account
        .spores
        .saturating_sub(fee_liquid + voters_paid + community_share);
    treasury_account.spendable = treasury_account
        .spendable
        .saturating_sub(fee_liquid + voters_paid + community_share);
    if let Err(e) = batch.put_account(&treasury_pubkey, &treasury_account) {
        warn!("⚠️  Failed to update treasury account: {}", e);
        return;
    }

    if let Err(e) = batch.set_fee_distribution_hash(slot, &block_hash) {
        warn!(
            "⚠️  Failed to record fee distribution hash in batch for slot {}: {}",
            slot, e
        );
        return;
    }

    // Commit all fee distribution writes atomically
    if let Err(e) = state.commit_batch(batch) {
        warn!(
            "⚠️  CRITICAL: Failed to commit fee distribution batch for slot {}: {}",
            slot, e
        );
        return;
    }

    if treasury_share > 0 {
        info!(
            "🏦 Treasury fees retained: {:.6} LICN",
            treasury_share as f64 / 1_000_000_000.0
        );
    }

    // ── Founding symbionts periodic vesting unlock ──
    // Check if any locked founding symbionts should be unlocked based on block timestamp.
    // Schedule: 6-month cliff, then 18-month linear vest to month 24.
    if let Ok(Some((cliff_end, vest_end, total_amount))) = state.get_founding_vesting_params() {
        let block_time = block.header.timestamp;
        if block_time >= cliff_end {
            if let Ok(Some(fm_pubkey)) = state.get_founding_symbionts_pubkey() {
                if let Ok(Some(mut fm_acct)) = state.get_account(&fm_pubkey) {
                    let target_unlocked = lichen_core::consensus::founding_vesting_unlocked(
                        total_amount,
                        cliff_end,
                        vest_end,
                        block_time,
                    );
                    let already_unlocked = total_amount.saturating_sub(fm_acct.locked);
                    if target_unlocked > already_unlocked {
                        let new_unlock = target_unlocked - already_unlocked;
                        fm_acct.spendable = fm_acct.spendable.saturating_add(new_unlock);
                        fm_acct.locked = fm_acct.locked.saturating_sub(new_unlock);
                        if let Err(e) = state.put_account(&fm_pubkey, &fm_acct) {
                            warn!("⚠️  Failed to update founding symbionts vesting: {}", e);
                        } else if new_unlock > 1_000_000_000 {
                            // Only log for significant unlocks (> 1 LICN)
                            info!(
                                "🔓 Founding symbionts vest: unlocked {:.2} LICN (total {:.2}M / {:.2}M)",
                                new_unlock as f64 / 1_000_000_000.0,
                                target_unlocked as f64 / 1_000_000_000_000_000_000.0,
                                total_amount as f64 / 1_000_000_000_000_000_000.0,
                            );
                        }
                    }
                }
            }
        }
    }

    // record_block_activity is called in emit_program_and_nft_events, not here
}

async fn activate_pending_validators_for_height(
    state: &StateStore,
    validator_set: &Arc<RwLock<ValidatorSet>>,
    height_pool: &StakePool,
    new_height: u64,
    min_validator_stake: u64,
) {
    let validator_snapshot: Vec<(Pubkey, u64, bool, u64)> = validator_set
        .read()
        .await
        .validators()
        .iter()
        .map(|validator| {
            (
                validator.pubkey,
                validator.stake,
                validator.pending_activation,
                validator.joined_slot,
            )
        })
        .collect();

    if validator_snapshot.is_empty() {
        return;
    }

    let mut reconciled = Vec::new();
    for (pubkey, current_stake, pending_activation, joined_slot) in validator_snapshot {
        // Resolve stake: pool first, then on-chain account, then keep current.
        // The pool may not have the validator yet (P2P announcement arrives
        // before RegisterValidator tx is processed), so fall back to the
        // on-chain staked balance which is authoritative.
        let resolved_stake = height_pool
            .get_stake(&pubkey)
            .map(|stake| stake.total_stake())
            .or_else(|| {
                state
                    .get_account(&pubkey)
                    .ok()
                    .flatten()
                    .map(|account| account.staked)
            })
            .unwrap_or(current_stake);

        // Always include pending validators so they get checked for activation
        // every height, even if their stake hasn't changed yet.
        if resolved_stake != current_stake || pending_activation {
            reconciled.push((pubkey, resolved_stake, pending_activation, joined_slot));
        }
    }

    if reconciled.is_empty() {
        return;
    }

    let mut vs = validator_set.write().await;
    let mut activated = Vec::new();
    let mut changed = false;
    for (pubkey, resolved_stake, pending_activation, joined_slot) in reconciled {
        if let Some(validator) = vs.get_validator_mut(&pubkey) {
            if validator.stake != resolved_stake {
                validator.stake = resolved_stake;
                changed = true;
            }
            if pending_activation
                && validator.pending_activation
                && resolved_stake >= min_validator_stake
                && new_height > joined_slot.saturating_add(1)
            {
                validator.pending_activation = false;
                changed = true;
                activated.push(pubkey);
            }
        }
    }

    if !changed {
        return;
    }

    let snapshot = vs.clone();
    drop(vs);

    if let Err(e) = state.save_validator_set(&snapshot) {
        warn!(
            "⚠️  Failed to persist validator set after height-boundary activation: {}",
            e
        );
        return;
    }

    for pubkey in activated {
        info!(
            "🔓 Height {}: Activated validator {} for consensus",
            new_height,
            pubkey.to_base58()
        );
    }
}

async fn freeze_consensus_snapshot_for_height(
    state: &StateStore,
    validator_set: &Arc<RwLock<ValidatorSet>>,
    stake_pool: &Arc<RwLock<StakePool>>,
    height: u64,
    min_validator_stake: u64,
) -> (ValidatorSet, StakePool) {
    let height_pool = stake_pool.read().await.clone();

    if lichen_core::is_epoch_boundary(height) {
        let epoch = lichen_core::slot_to_epoch(height);
        {
            let mut vs = validator_set.write().await;

            if let Ok(pending) = state.get_pending_validator_changes(epoch) {
                for change in &pending {
                    if change.change_type == lichen_core::ValidatorChangeType::Remove {
                        vs.remove_validator(&change.pubkey);
                        if let Err(e) = state.delete_validator(&change.pubkey) {
                            warn!(
                                "⚠️  Failed to remove deregistered validator {} from state: {}",
                                change.pubkey.to_base58(),
                                e
                            );
                        }
                        info!(
                            "🔒 Epoch {}: Deregistered validator {} (voluntary exit)",
                            epoch,
                            change.pubkey.to_base58()
                        );
                    }
                }
            }

            if let Err(e) = state.save_validator_set(&vs) {
                warn!(
                    "⚠️  Failed to persist validator set after epoch transition: {}",
                    e
                );
            }
        }
        if let Err(e) = state.clear_pending_validator_changes(epoch) {
            warn!(
                "⚠️  Failed to clear pending validator changes for epoch {}: {}",
                epoch, e
            );
        }

        activate_pending_validators_for_height(
            state,
            validator_set,
            &height_pool,
            height,
            min_validator_stake,
        )
        .await;

        let (mut frozen, pending_count) = {
            let vs = validator_set.read().await;
            (vs.consensus_set(), vs.pending_count())
        };
        frozen.set_frozen_epoch(epoch);
        info!(
            "🧊 Epoch {}: Froze validator set ({} active, {} pending)",
            epoch,
            frozen.validators().len(),
            pending_count,
        );
        return (frozen, height_pool);
    }

    activate_pending_validators_for_height(
        state,
        validator_set,
        &height_pool,
        height,
        min_validator_stake,
    )
    .await;

    (validator_set.read().await.consensus_set(), height_pool)
}

/// Periodic checkpoint creation — called after every block to check if
/// the current slot should trigger a RocksDB checkpoint.
/// Checkpoints are created every CHECKPOINT_INTERVAL (10K) slots and
/// provide O(1) state snapshots for new validator catch-up.
async fn maybe_create_checkpoint(
    state: &StateStore,
    slot: u64,
    data_dir: &str,
    sync_manager: &Arc<SyncManager>,
) {
    use crate::sync::SyncManager;
    if !SyncManager::should_checkpoint(slot) {
        return;
    }
    let checkpoint_path = format!("{}/checkpoints/slot-{}", data_dir, slot);
    match state.create_checkpoint(&checkpoint_path, slot) {
        Ok(meta) => {
            info!(
                "📸 Checkpoint created at slot {} ({} accounts, interval: every {} slots)",
                meta.slot,
                meta.total_accounts,
                SyncManager::checkpoint_interval()
            );
            // Record the checkpoint in SyncManager for fast bootstrapping
            sync_manager.set_checkpoint(slot).await;
            // Prune old checkpoints — keep only the 3 most recent
            if let Err(e) = StateStore::prune_checkpoints(data_dir, 3) {
                warn!("⚠️  Failed to prune old checkpoints: {}", e);
            }
        }
        Err(e) => {
            warn!("⚠️  Failed to create checkpoint at slot {}: {}", slot, e);
        }
    }
}

fn latest_verified_checkpoint(
    data_dir: &str,
    state: &StateStore,
    validator_set: &ValidatorSet,
    stake_pool: &StakePool,
) -> Option<(lichen_core::CheckpointMeta, String, Block)> {
    let (meta, path) = StateStore::latest_checkpoint(data_dir)?;
    let finalized_slot = state.get_last_finalized_slot().ok()?;
    if meta.slot == 0 || meta.slot > finalized_slot {
        return None;
    }

    let block = state.get_block_by_slot(meta.slot).ok().flatten()?;
    if block.header.state_root.0 != meta.state_root {
        warn!(
            "⚠️  Rejecting checkpoint at slot {}: state root does not match committed block",
            meta.slot
        );
        return None;
    }
    if let Err(err) = verify_committed_block_authenticity(&block, validator_set, stake_pool) {
        warn!(
            "⚠️  Rejecting checkpoint at slot {}: commit verification failed: {}",
            meta.slot, err
        );
        return None;
    }

    Some((meta, path, block))
}

fn verify_committed_block_authenticity(
    block: &Block,
    validator_set: &ValidatorSet,
    stake_pool: &StakePool,
) -> Result<(), String> {
    if block.header.slot == 0 {
        return Ok(());
    }

    if block.commit_signatures.is_empty() {
        return Err(format!(
            "block {} has no commit certificate",
            block.header.slot
        ));
    }

    block.verify_commit(validator_set, stake_pool)
}

fn verify_checkpoint_anchor(
    slot: u64,
    state_root: [u8; 32],
    checkpoint_header: Option<&lichen_core::BlockHeader>,
    commit_round: u32,
    commit_signatures: &[lichen_core::CommitSignature],
    validator_set: &ValidatorSet,
    stake_pool: &StakePool,
) -> Result<(), String> {
    let header = checkpoint_header.ok_or_else(|| "missing checkpoint header".to_string())?;
    if header.slot != slot {
        return Err(format!(
            "checkpoint header slot mismatch: expected {}, got {}",
            slot, header.slot
        ));
    }
    if header.state_root.0 != state_root {
        return Err("checkpoint header state root mismatch".to_string());
    }

    let block = Block {
        header: header.clone(),
        transactions: Vec::new(),
        tx_fees_paid: Vec::new(),
        oracle_prices: Vec::new(),
        commit_round,
        commit_signatures: commit_signatures.to_vec(),
    };
    if !block.verify_signature() {
        return Err("checkpoint header signature verification failed".to_string());
    }
    verify_committed_block_authenticity(&block, validator_set, stake_pool)
}

fn verify_block_validators_hash(
    block: &Block,
    validator_set: &ValidatorSet,
    stake_pool: &StakePool,
    min_validator_stake: u64,
) -> Result<(), String> {
    if block.header.validators_hash == Hash([0u8; 32]) {
        return Ok(());
    }

    let consensus_set = validator_set.consensus_set();
    let expected = compute_validators_hash(&consensus_set, stake_pool);
    if block.header.validators_hash != expected {
        if let Some(promoted_expected) =
            compute_promoted_pending_validators_hash(validator_set, stake_pool, min_validator_stake)
        {
            if block.header.validators_hash == promoted_expected {
                return Ok(());
            }
        }

        return Err(format!(
            "validators_hash mismatch (block={}, local={})",
            block.header.validators_hash.to_hex(),
            expected.to_hex(),
        ));
    }

    Ok(())
}

fn compute_promoted_pending_validators_hash(
    validator_set: &ValidatorSet,
    stake_pool: &StakePool,
    min_validator_stake: u64,
) -> Option<Hash> {
    let mut promoted = validator_set.clone();
    let mut changed = false;

    for validator in promoted.validators_mut() {
        if !validator.pending_activation {
            continue;
        }

        let resolved_stake = stake_pool
            .get_stake(&validator.pubkey)
            .map(|stake| stake.total_stake())
            .unwrap_or(validator.stake);

        if resolved_stake >= min_validator_stake {
            validator.stake = resolved_stake;
            validator.pending_activation = false;
            changed = true;
        }
    }

    if !changed {
        return None;
    }

    Some(compute_validators_hash(
        &promoted.consensus_set(),
        stake_pool,
    ))
}

fn should_add_local_validator_as_pending(is_joining_network: bool, current_tip: u64) -> bool {
    // A validator discovered after genesis must cross at least one full local
    // height boundary before it can enter the frozen consensus snapshot.
    // This keeps restart and late-discovery behavior aligned across nodes.
    is_joining_network || current_tip > 0
}

fn should_add_announced_validator_as_pending(
    local_tip: u64,
    local_stake: u64,
    min_validator_stake: u64,
) -> bool {
    // Announcements are asynchronous relative to block commits. Even if stake
    // is already visible locally, a validator first discovered after genesis
    // must wait for the next locally completed height before activation.
    local_tip > 0 || local_stake < min_validator_stake
}

fn checkpoint_anchor_support(
    anchors: &HashMap<SocketAddr, (u64, [u8; 32])>,
    slot: u64,
    state_root: [u8; 32],
) -> usize {
    anchors
        .values()
        .filter(|(anchor_slot, anchor_root)| *anchor_slot == slot && *anchor_root == state_root)
        .count()
}

// ========================================================================
// FIRST-BOOT CONTRACT AUTO-DEPLOY
// ========================================================================
//  BACKGROUND ORACLE PRICE FEEDER — Real-time Binance WebSocket price feed
//  with REST API fallback. Submits native oracle attestations and broadcasts
//  consensus-derived analytics updates.
//
//  Architecture:
//    1. WebSocket reader: connects to Binance aggTrade streams for SOL/ETH,
//       stores latest prices in lock-free AtomicU64 (microdollars).
//    2. Attestation writer: periodic tick reads atomics and submits signed
//       oracle-attestation transactions when prices change or need refresh.
//    3. REST fallback: if WebSocket is unhealthy (no message in 30s),
//       fetches prices from Binance REST API as backup.
//    4. Auto-reconnect: exponential backoff 1s → 2s → 4s → ... → 30s max.
// ========================================================================

/// Price stored as microdollars in AtomicU64 (price * 1_000_000).
/// This gives 6 decimal precision, far exceeding oracle's 8-decimal format.
const MICRO_SCALE: f64 = 1_000_000.0;

/// Default Binance WebSocket aggTrade stream URL for SOL, ETH, and BNB.
/// Override via LICHEN_ORACLE_WS_URL (e.g. for Binance US: wss://stream.binance.us:9443/ws/...)
const DEFAULT_BINANCE_WS_URL: &str =
    "wss://stream.binance.com:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade";

/// Default Binance REST fallback URL.
/// Override via LICHEN_ORACLE_REST_URL (e.g. for Binance US: https://api.binance.us/api/v3/...)
const DEFAULT_BINANCE_REST_URL: &str =
    "https://api.binance.com/api/v3/ticker/price?symbols=[%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22]";

/// REST ticker response
#[derive(Deserialize)]
struct BinanceTicker {
    symbol: String,
    price: String,
}

fn seed_bootstrap_consensus_oracle_prices(state: &StateStore, slot: u64) {
    for (asset, price_raw) in [
        ("LICN", genesis_licn_price_8dec()),
        ("wSOL", genesis_wsol_price_8dec()),
        ("wETH", genesis_weth_price_8dec()),
        ("wBNB", genesis_wbnb_price_8dec()),
    ] {
        let has_price = state
            .get_oracle_consensus_price(asset)
            .ok()
            .flatten()
            .is_some();
        if has_price {
            continue;
        }
        let _ = state.put_oracle_consensus_price(asset, price_raw, 8, slot, 0);
    }
}

fn build_oracle_attestation_tx(
    state: &StateStore,
    validator_seed: &[u8; 32],
    validator_pubkey: Pubkey,
    asset: &str,
    price_raw: u64,
    decimals: u8,
) -> Result<Transaction, String> {
    if price_raw == 0 {
        return Err("oracle attestation price must be > 0".to_string());
    }
    if decimals > 18 {
        return Err("oracle attestation decimals must be 0..=18".to_string());
    }

    let pool = state.get_stake_pool()?;
    let stake_info = pool
        .get_stake(&validator_pubkey)
        .ok_or_else(|| "validator has no stake for oracle attestation".to_string())?;
    if !stake_info.is_active || !stake_info.meets_minimum() {
        return Err("validator is not active for oracle attestation".to_string());
    }

    let tip = state.get_last_slot().unwrap_or(0);
    let recent_blockhash = state
        .get_block_by_slot(tip)?
        .map(|block| block.hash())
        .ok_or_else(|| "oracle attestation requires a recent blockhash".to_string())?;

    let asset_bytes = asset.as_bytes();
    let mut data = Vec::with_capacity(2 + asset_bytes.len() + 8 + 1);
    data.push(30u8);
    data.push(asset_bytes.len() as u8);
    data.extend_from_slice(asset_bytes);
    data.extend_from_slice(&price_raw.to_le_bytes());
    data.push(decimals);

    let ix = lichen_core::Instruction {
        program_id: CORE_SYSTEM_PROGRAM_ID,
        accounts: vec![validator_pubkey],
        data,
    };
    let msg = lichen_core::Message::new(vec![ix], recent_blockhash);
    let mut tx = Transaction::new(msg);
    let kp = Keypair::from_seed(validator_seed);
    tx.signatures.push(kp.sign(&tx.message.serialize()));
    Ok(tx)
}

#[derive(Clone)]
struct OracleFeedTxContext {
    mempool: Arc<Mutex<Mempool>>,
    p2p_peer_manager: Option<Arc<lichen_p2p::PeerManager>>,
    p2p_config: P2PConfig,
    validator_seed: [u8; 32],
    validator_pubkey: Pubkey,
}

async fn submit_oracle_attestation_tx(
    state: &StateStore,
    tx_context: &OracleFeedTxContext,
    asset: &str,
    price_raw: u64,
    decimals: u8,
) -> bool {
    let tx = match build_oracle_attestation_tx(
        state,
        &tx_context.validator_seed,
        tx_context.validator_pubkey,
        asset,
        price_raw,
        decimals,
    ) {
        Ok(tx) => tx,
        Err(e) => {
            debug!("Skipping oracle attestation for {}: {}", asset, e);
            return false;
        }
    };

    {
        let fee_config = FeeConfig::default_from_constants();
        let computed_fee = TxProcessor::compute_transaction_fee(&tx, &fee_config);
        let mut pool = tx_context.mempool.lock().await;
        if let Err(e) = pool.add_transaction(tx.clone(), computed_fee, 0) {
            debug!(
                "Failed to add oracle attestation tx for {} to mempool: {}",
                asset, e
            );
            return false;
        }
    }

    if let Some(peer_mgr) = &tx_context.p2p_peer_manager {
        let target_id = tx.hash().0;
        let msg = lichen_p2p::P2PMessage::new(
            lichen_p2p::MessageType::Transaction(tx),
            tx_context.p2p_config.listen_addr,
        );
        peer_mgr
            .route_to_closest(&target_id, lichen_p2p::NON_CONSENSUS_FANOUT, msg)
            .await;
    }

    true
}

fn spawn_oracle_price_feeder(
    state: StateStore,
    shared_prices: SharedOraclePrices,
    dex_broadcaster: std::sync::Arc<lichen_rpc::dex_ws::DexEventBroadcaster>,
    tx_context: OracleFeedTxContext,
) {
    tokio::spawn(async move {
        // Configurable Binance endpoints via env vars (for geo-blocked regions)
        let oracle_ws_url: String = std::env::var("LICHEN_ORACLE_WS_URL")
            .unwrap_or_else(|_| DEFAULT_BINANCE_WS_URL.to_string());
        let oracle_rest_url: String = std::env::var("LICHEN_ORACLE_REST_URL")
            .unwrap_or_else(|_| DEFAULT_BINANCE_REST_URL.to_string());
        info!("🔮 Oracle WS: {}", oracle_ws_url);
        info!("🔮 Oracle REST: {}", oracle_rest_url);

        // Use the shared atomics to source validator oracle attestations.
        let wsol_micro = shared_prices.wsol_micro.clone();
        let weth_micro = shared_prices.weth_micro.clone();
        let wbnb_micro = shared_prices.wbnb_micro.clone();
        let ws_healthy = shared_prices.ws_healthy.clone();

        // Spawn WebSocket reader task FIRST so prices start flowing immediately
        // even while we wait for ANALYTICS symbol registry (joining node sync).
        {
            let ws_wsol = wsol_micro.clone();
            let ws_weth = weth_micro.clone();
            let ws_wbnb = wbnb_micro.clone();
            let ws_flag = ws_healthy.clone();
            let ws_url = oracle_ws_url.clone();
            tokio::spawn(async move {
                binance_ws_loop(ws_wsol, ws_weth, ws_wbnb, ws_flag, ws_url).await;
            });
        }

        // Resolve analytics contract pubkey — retry up to 60s for joining nodes
        // that haven't synced the symbol registry yet.
        let analytics_pk = {
            let mut resolved = None;
            for attempt in 0..12 {
                match state.get_symbol_registry("ANALYTICS") {
                    Ok(Some(entry)) => {
                        resolved = Some(entry.program);
                        break;
                    }
                    _ => {
                        if attempt == 0 {
                            info!("🔮 Oracle price feeder: waiting for ANALYTICS symbol registry (joining node sync)...");
                        }
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
            match resolved {
                Some(pk) => pk,
                None => {
                    warn!("🔮 Oracle price feeder: ANALYTICS symbol not found after 60s, aborting");
                    return;
                }
            }
        };

        // REST fallback HTTP client (used only when WebSocket is unhealthy)
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        let rest_url = oracle_rest_url.clone();

        info!("🔮 Oracle price feeder started (WebSocket real-time → signed oracle attestations)");

        let candle_intervals: [u64; 9] =
            [60, 300, 900, 3600, 14400, 86400, 259200, 604800, 31536000];
        let mut last_attested: HashMap<&'static str, (u64, Instant)> = HashMap::new();

        const PRICE_SCALE_F: f64 = 1_000_000_000.0; // 1e9 for DEX price scaling

        // Candle writer loop: 5-second tick (WS broadcasts only — state is
        // written by apply_oracle_from_block during consensus block processing).
        // Oracle feeds + DEX bands + candles are now written deterministically in
        // apply_oracle_from_block() during block effects — NOT here.
        let mut write_tick = time::interval(Duration::from_secs(5));

        loop {
            write_tick.tick().await;

            // Read current prices from atomics
            let mut cur_wsol = wsol_micro.load(Ordering::Relaxed);
            let mut cur_weth = weth_micro.load(Ordering::Relaxed);
            let mut cur_wbnb = wbnb_micro.load(Ordering::Relaxed);

            // REST fallback if WebSocket is not healthy or no prices yet
            if !ws_healthy.load(Ordering::Relaxed)
                || (cur_wsol == 0 && cur_weth == 0 && cur_wbnb == 0)
            {
                if let Ok(resp) = http.get(&rest_url).send().await {
                    if let Ok(tickers) = resp.json::<Vec<BinanceTicker>>().await {
                        for t in &tickers {
                            let p: f64 = t.price.parse().unwrap_or(0.0);
                            if p <= 0.0 {
                                continue;
                            }
                            let micro = (p * MICRO_SCALE) as u64;
                            match t.symbol.as_str() {
                                "SOLUSDT" => {
                                    wsol_micro.store(micro, Ordering::Relaxed);
                                    cur_wsol = micro;
                                }
                                "ETHUSDT" => {
                                    weth_micro.store(micro, Ordering::Relaxed);
                                    cur_weth = micro;
                                }
                                "BNBUSDT" => {
                                    wbnb_micro.store(micro, Ordering::Relaxed);
                                    cur_wbnb = micro;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }

            // NOTE: Oracle feed writes, DEX price band writes, candle writes,
            // last-price writes, and 24h stats writes are ALL handled
            // deterministically in apply_oracle_from_block() during block effects.
            // This feeder submits signed native oracle attestation txs and
            // reads consensus-written state to broadcast WS events.

            for (asset, price_raw) in [
                ("LICN", GENESIS_LICN_PRICE_8DEC),
                ("wSOL", cur_wsol.saturating_mul(100)),
                ("wETH", cur_weth.saturating_mul(100)),
                ("wBNB", cur_wbnb.saturating_mul(100)),
            ] {
                if price_raw == 0 {
                    continue;
                }
                let should_submit = match last_attested.get(asset) {
                    Some((last_price, last_at)) => {
                        *last_price != price_raw || last_at.elapsed() >= Duration::from_secs(60)
                    }
                    None => true,
                };
                if !should_submit {
                    continue;
                }
                if submit_oracle_attestation_tx(&state, &tx_context, asset, price_raw, 8).await {
                    last_attested.insert(asset, (price_raw, Instant::now()));
                }
            }

            let wsol_usd = cur_wsol as f64 / MICRO_SCALE;
            let weth_usd = cur_weth as f64 / MICRO_SCALE;
            let wbnb_usd = cur_wbnb as f64 / MICRO_SCALE;

            if wsol_usd <= 0.0 && weth_usd <= 0.0 && wbnb_usd <= 0.0 {
                continue;
            }

            // WS broadcasts — read consensus state and emit to WebSocket clients
            let current_slot = state.get_last_slot().unwrap_or(0);

            let licn_usd: f64 = 0.10;
            let pair_prices: [(u64, f64); 7] = [
                (1, licn_usd),
                (2, wsol_usd),
                (3, weth_usd),
                (
                    4,
                    if licn_usd > 0.0 {
                        wsol_usd / licn_usd
                    } else {
                        0.0
                    },
                ),
                (
                    5,
                    if licn_usd > 0.0 {
                        weth_usd / licn_usd
                    } else {
                        0.0
                    },
                ),
                (6, wbnb_usd),
                (
                    7,
                    if licn_usd > 0.0 {
                        wbnb_usd / licn_usd
                    } else {
                        0.0
                    },
                ),
            ];

            for (pair_id, price_f64) in &pair_prices {
                if *price_f64 <= 0.0 {
                    continue;
                }

                // ── WS broadcast: read consensus-written state and emit ──
                // Candles, last price, and 24h stats are written deterministically
                // by apply_oracle_from_block() during block processing. This feeder
                // only READS that data and broadcasts it via WebSocket.

                // Read 24h stats written by consensus
                let stats_key = format!("ana_24h_{}", pair_id);
                let stats = match state.get_contract_storage(&analytics_pk, stats_key.as_bytes()) {
                    Ok(Some(d)) if d.len() >= 48 => d,
                    _ => vec![0u8; 48],
                };

                // ── WS broadcast: ticker update for this pair ──
                let volume_24h = u64::from_le_bytes(stats[0..8].try_into().unwrap_or([0; 8]));
                let open_raw = u64::from_le_bytes(stats[24..32].try_into().unwrap_or([0; 8]));
                let open_f = open_raw as f64 / PRICE_SCALE_F;
                let change_24h = if open_f > 0.0 {
                    ((*price_f64 - open_f) / open_f) * 100.0
                } else {
                    0.0
                };
                dex_broadcaster.emit_ticker(
                    *pair_id, *price_f64, *price_f64, *price_f64, volume_24h, change_24h,
                );

                // ── WS broadcast: candle updates for all intervals ──
                // Read consensus-written candles and broadcast via WebSocket
                for &ci in &candle_intervals {
                    let count_key_c = format!("ana_cc_{}_{}", pair_id, ci);
                    let candle_count_c: u64 =
                        match state.get_contract_storage(&analytics_pk, count_key_c.as_bytes()) {
                            Ok(Some(d)) if d.len() >= 8 => {
                                u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8]))
                            }
                            _ => 0,
                        };
                    if candle_count_c > 0 {
                        let idx_c = candle_count_c - 1;
                        let ck = format!("ana_c_{}_{}_{}", pair_id, ci, idx_c);
                        if let Ok(Some(cd)) =
                            state.get_contract_storage(&analytics_pk, ck.as_bytes())
                        {
                            if cd.len() >= 48 {
                                let o = u64::from_le_bytes(cd[0..8].try_into().unwrap_or([0; 8]))
                                    as f64
                                    / PRICE_SCALE_F;
                                let h = u64::from_le_bytes(cd[8..16].try_into().unwrap_or([0; 8]))
                                    as f64
                                    / PRICE_SCALE_F;
                                let l = u64::from_le_bytes(cd[16..24].try_into().unwrap_or([0; 8]))
                                    as f64
                                    / PRICE_SCALE_F;
                                let c = u64::from_le_bytes(cd[24..32].try_into().unwrap_or([0; 8]))
                                    as f64
                                    / PRICE_SCALE_F;
                                let v = u64::from_le_bytes(cd[32..40].try_into().unwrap_or([0; 8]));
                                dex_broadcaster.emit_candle(
                                    *pair_id,
                                    ci,
                                    o,
                                    h,
                                    l,
                                    c,
                                    v,
                                    current_slot,
                                );
                            }
                        }
                    }
                }
            }

            debug!(
                "🔮 Oracle candles updated: wSOL=${:.2} wETH=${:.2} wBNB=${:.2}",
                wsol_usd, weth_usd, wbnb_usd
            );
        }
    });
}

/// Binance WebSocket reader loop with auto-reconnect.
/// Connects to aggTrade streams, parses prices, stores in atomics.
/// On disconnect, retries with exponential backoff (1s → 30s max).
async fn binance_ws_loop(
    wsol: Arc<AtomicU64>,
    weth: Arc<AtomicU64>,
    wbnb: Arc<AtomicU64>,
    healthy: Arc<AtomicBool>,
    ws_url: String,
) {
    let mut backoff_secs: u64 = 1;

    loop {
        info!("🔮 Binance WebSocket connecting to {}...", ws_url);
        healthy.store(false, Ordering::Relaxed);

        match tokio_tungstenite::connect_async(ws_url.as_str()).await {
            Ok((ws_stream, _)) => {
                info!("🔮 Binance WebSocket connected (real-time aggTrade feed)");
                backoff_secs = 1; // reset backoff on successful connect
                healthy.store(true, Ordering::Relaxed);

                let (mut write, mut read) = ws_stream.split();

                while let Some(msg_result) = read.next().await {
                    match msg_result {
                        Ok(tungstenite::Message::Text(ref text)) => {
                            // aggTrade format: {"e":"aggTrade","s":"SOLUSDT","p":"82.30",...}
                            if let Ok(trade) = serde_json::from_str::<serde_json::Value>(text) {
                                if let (Some(sym), Some(price_str)) =
                                    (trade["s"].as_str(), trade["p"].as_str())
                                {
                                    let price: f64 = price_str.parse().unwrap_or(0.0);
                                    if price > 0.0 {
                                        let micro = (price * MICRO_SCALE) as u64;
                                        match sym {
                                            "SOLUSDT" => wsol.store(micro, Ordering::Relaxed),
                                            "ETHUSDT" => weth.store(micro, Ordering::Relaxed),
                                            "BNBUSDT" => wbnb.store(micro, Ordering::Relaxed),
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        Ok(tungstenite::Message::Ping(data)) => {
                            // Respond to keep connection alive
                            if write.send(tungstenite::Message::Pong(data)).await.is_err() {
                                warn!("🔮 Binance WebSocket pong send failed");
                                break;
                            }
                        }
                        Ok(tungstenite::Message::Close(_)) => {
                            info!("🔮 Binance WebSocket closed by server");
                            break;
                        }
                        Err(e) => {
                            warn!("🔮 Binance WebSocket read error: {}", e);
                            break;
                        }
                        _ => {} // Binary, Pong, Frame — ignore
                    }
                }

                healthy.store(false, Ordering::Relaxed);
                warn!("🔮 Binance WebSocket disconnected, reconnecting...");
            }
            Err(e) => {
                warn!("🔮 Binance WebSocket connect failed: {}", e);
            }
        }

        // Exponential backoff: 1 → 2 → 4 → 8 → 16 → 30 (capped)
        let delay = backoff_secs.min(30);
        tokio::time::sleep(Duration::from_secs(delay)).await;
        backoff_secs = (backoff_secs * 2).min(60);
    }
}

/// Update a single candle for an oracle-priced pair.
/// Mirrors the logic in dex_analytics `update_candle` but runs directly
/// against the state store from the validator background task.
fn oracle_update_candle(
    state: &StateStore,
    analytics_pk: &Pubkey,
    pair_id: u64,
    interval: u64,
    price: u64,
    _current_slot: u64,
    unix_ts: u64,
) {
    // Use unix timestamp (not slot) for period grouping so candle boundaries
    // align with wall-clock seconds (60s, 300s, 3600s, etc.).
    let candle_start = (unix_ts / interval) * interval;

    // Read current candle's start slot (use Option to distinguish missing from 0)
    let cur_key = format!("ana_cur_{}_{}", pair_id, interval);
    let stored_start = match state.get_contract_storage(analytics_pk, cur_key.as_bytes()) {
        Ok(Some(d)) if d.len() >= 8 => {
            Some(u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])))
        }
        _ => None,
    };

    let count_key = format!("ana_cc_{}_{}", pair_id, interval);

    if stored_start == Some(candle_start) {
        // Same candle period — update OHLC in-place
        let candle_count = match state.get_contract_storage(analytics_pk, count_key.as_bytes()) {
            Ok(Some(d)) if d.len() >= 8 => u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])),
            _ => 0,
        };
        if candle_count == 0 {
            return;
        }
        let idx = candle_count - 1;
        let candle_key = format!("ana_c_{}_{}_{}", pair_id, interval, idx);

        if let Ok(Some(mut data)) = state.get_contract_storage(analytics_pk, candle_key.as_bytes())
        {
            if data.len() >= 48 {
                let high = u64::from_le_bytes(data[8..16].try_into().unwrap_or([0; 8]));
                let low = u64::from_le_bytes(data[16..24].try_into().unwrap_or([0; 8]));
                if price > high {
                    data[8..16].copy_from_slice(&price.to_le_bytes());
                }
                if price < low {
                    data[16..24].copy_from_slice(&price.to_le_bytes());
                }
                // Update close price
                data[24..32].copy_from_slice(&price.to_le_bytes());
                // Keep timestamp as the period-start (don't overwrite with current time)
                let _ = state.put_contract_storage(analytics_pk, candle_key.as_bytes(), &data);
            }
        }
    } else {
        // New candle period — create a new candle
        let candle_count = match state.get_contract_storage(analytics_pk, count_key.as_bytes()) {
            Ok(Some(d)) if d.len() >= 8 => u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8])),
            _ => 0,
        };

        // Build new candle: open(8)+high(8)+low(8)+close(8)+volume(8)+timestamp(8) = 48
        let mut candle = Vec::with_capacity(48);
        candle.extend_from_slice(&price.to_le_bytes()); // open
        candle.extend_from_slice(&price.to_le_bytes()); // high
        candle.extend_from_slice(&price.to_le_bytes()); // low
        candle.extend_from_slice(&price.to_le_bytes()); // close
        candle.extend_from_slice(&0u64.to_le_bytes()); // volume (oracle updates have 0 volume)
        candle.extend_from_slice(&candle_start.to_le_bytes()); // period-start time (aligned)

        let new_idx = candle_count;
        let candle_key = format!("ana_c_{}_{}_{}", pair_id, interval, new_idx);
        let _ = state.put_contract_storage(analytics_pk, candle_key.as_bytes(), &candle);

        // Update count
        let _ = state.put_contract_storage(
            analytics_pk,
            count_key.as_bytes(),
            &(new_idx + 1).to_le_bytes(),
        );

        // Store current candle start slot
        let _ = state.put_contract_storage(
            analytics_pk,
            cur_key.as_bytes(),
            &candle_start.to_le_bytes(),
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  LOG MANAGEMENT — background task that prunes old daily log files.
//  Runs immediately on spawn then every 6 hours while the validator is alive.
// ═══════════════════════════════════════════════════════════════════════

/// Spawn a background task that periodically removes log files older than
/// `max_age_days` from `log_dir`.  Targets files matching the
/// `validator.log.YYYY-MM-DD` pattern produced by
/// `tracing_appender::rolling::daily`.
fn spawn_log_cleanup_task(log_dir: PathBuf, max_age_days: u64) {
    tokio::spawn(async move {
        let sweep_interval = tokio::time::Duration::from_secs(3 * 3600); // 3 hours
        loop {
            let cutoff =
                std::time::SystemTime::now() - std::time::Duration::from_secs(max_age_days * 86400);
            if let Ok(entries) = fs::read_dir(&log_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = match path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    if !name.starts_with("validator.log.") {
                        continue;
                    }
                    if let Ok(meta) = fs::metadata(&path) {
                        if let Ok(modified) = meta.modified() {
                            if modified < cutoff {
                                match fs::remove_file(&path) {
                                    Ok(_) => info!("🗑️  Removed old log file: {}", name),
                                    Err(e) => warn!("Failed to remove old log {}: {}", name, e),
                                }
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(sweep_interval).await;
        }
    });
}

// ═══════════════════════════════════════════════════════════════════════
//  CLI ARGUMENT HELPERS — support both `--flag value` and `--flag=value`
// ═══════════════════════════════════════════════════════════════════════

/// Find the value for a CLI flag, supporting both `--flag value` and `--flag=value`.
/// For flags with aliases, pass all names (e.g. `&["--db-path", "--db", "--data-dir"]`).
fn get_flag_value<'a>(args: &'a [String], names: &[&str]) -> Option<&'a str> {
    for (i, arg) in args.iter().enumerate() {
        for name in names {
            if arg == *name {
                // --flag value
                return args.get(i + 1).map(|s| s.as_str());
            }
            if let Some(val) = arg.strip_prefix(&format!("{}=", name)) {
                // --flag=value
                return Some(val);
            }
        }
    }
    None
}

/// Check if a boolean flag is present, supporting `--flag` and `--flag=true`.
fn has_flag(args: &[String], name: &str) -> bool {
    args.iter()
        .any(|a| a == name || a.starts_with(&format!("{}=", name)))
}

// ═══════════════════════════════════════════════════════════════════════
//  SUPERVISOR — wraps the validator in a restart loop.
//  When the internal watchdog detects a stall it exits with EXIT_CODE_RESTART;
//  the supervisor catches that and relaunches the process automatically.
//  Pass --no-watchdog to disable the supervisor entirely (e.g. when using
//  systemd Restart=on-failure which already handles restarts).
// ═══════════════════════════════════════════════════════════════════════

fn main() {
    let args: Vec<String> = env::args().collect();

    // If we're the child (worker) process, go straight to the async validator.
    if has_flag(&args, "--supervised") {
        return run_validator_sync();
    }

    // If the user opted out of the built-in supervisor, also run directly.
    if has_flag(&args, "--no-watchdog") {
        return run_validator_sync();
    }

    // Parse supervisor-specific flags
    let max_restarts = get_flag_value(&args, &["--max-restarts"])
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_RESTARTS);

    // ── Supervisor loop ─────────────────────────────────────────────
    // Re-exec ourselves with --supervised so the child enters run_validator()
    // directly.  On EXIT_CODE_RESTART → restart.  On 0 or SIGTERM → stop.
    let exe = match env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("Cannot determine own executable path: {}", err);
            std::process::exit(1);
        }
    };

    // Build child args: forward everything except supervisor-only flags,
    // then append --supervised.
    let child_args: Vec<String> = args[1..]
        .iter()
        .filter(|a| {
            let s = a.as_str();
            !(s == "--no-watchdog"
                || s == "--supervised"
                || s == "--max-restarts"
                || s.starts_with("--max-restarts=")
                || s.starts_with("--no-watchdog=")
                || s.starts_with("--supervised="))
        })
        .cloned()
        .collect();

    let mut restart_count: u32 = 0;
    let mut backoff_secs: u64 = 1;

    // Initialize minimal logging for supervisor messages (stdout only)
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(true))
        .with(tracing_subscriber::filter::LevelFilter::INFO)
        .init();

    info!(
        "🐺 Lichen Supervisor started (max restarts: {})",
        max_restarts
    );

    loop {
        info!(
            "🚀 Launching validator (attempt {}/{})",
            restart_count + 1,
            max_restarts
        );

        let child_start = std::time::Instant::now();
        let mut child = match std::process::Command::new(&exe)
            .args(&child_args)
            .arg("--supervised")
            .stdin(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                error!("Failed to spawn validator process: {}", err);
                restart_count += 1;
                if restart_count >= max_restarts {
                    error!(
                        "❌ Max restarts ({}) reached after spawn failures — giving up",
                        max_restarts
                    );
                    std::process::exit(1);
                }
                let sleep_for = Duration::from_secs(backoff_secs);
                warn!(
                    "⏳ Retrying spawn in {}s (attempt {}/{})",
                    backoff_secs, restart_count, max_restarts
                );
                std::thread::sleep(sleep_for);
                backoff_secs = (backoff_secs * 2).min(60);
                continue;
            }
        };

        let status = match child.wait() {
            Ok(status) => status,
            Err(err) => {
                error!("Failed to wait on validator process: {}", err);
                restart_count += 1;
                if restart_count >= max_restarts {
                    error!(
                        "❌ Max restarts ({}) reached after wait failures — giving up",
                        max_restarts
                    );
                    std::process::exit(1);
                }
                let sleep_for = Duration::from_secs(backoff_secs);
                warn!(
                    "⏳ Retrying after wait failure in {}s (attempt {}/{})",
                    backoff_secs, restart_count, max_restarts
                );
                std::thread::sleep(sleep_for);
                backoff_secs = (backoff_secs * 2).min(60);
                continue;
            }
        };

        // L7 fix: reset backoff if child ran successfully for >3 minutes
        let runtime = child_start.elapsed();
        if runtime > Duration::from_secs(180) {
            backoff_secs = 1;
            restart_count = 0;
            info!(
                "🔄 Validator ran for {}s — resetting backoff",
                runtime.as_secs()
            );
        }

        match status.code() {
            Some(0) => {
                info!("✅ Validator exited cleanly (code 0) — shutting down supervisor");
                break;
            }
            Some(EXIT_CODE_RESTART) => {
                restart_count += 1;
                if restart_count >= max_restarts {
                    error!(
                        "❌ Validator requested restart but max restarts ({}) reached — giving up",
                        max_restarts
                    );
                    std::process::exit(1);
                }
                warn!(
                    "🔄 Validator stall detected (exit {}) — restarting in {}s (restart {}/{})",
                    EXIT_CODE_RESTART, backoff_secs, restart_count, max_restarts
                );
                std::thread::sleep(Duration::from_secs(backoff_secs));
                // Exponential backoff capped at 30s, reset after 3 successful minutes
                backoff_secs = (backoff_secs * 2).min(30);
            }
            Some(code) => {
                restart_count += 1;
                if restart_count >= max_restarts {
                    error!(
                        "❌ Validator crashed (exit {}) and max restarts ({}) reached — giving up",
                        code, max_restarts
                    );
                    std::process::exit(code);
                }
                warn!(
                    "💥 Validator crashed (exit {}) — restarting in {}s (restart {}/{})",
                    code, backoff_secs, restart_count, max_restarts
                );
                std::thread::sleep(Duration::from_secs(backoff_secs));
                backoff_secs = (backoff_secs * 2).min(30);
            }
            None => {
                // Killed by signal (SIGTERM, SIGKILL, etc.)
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(sig) = status.signal() {
                        if sig == 15 || sig == 2 {
                            // SIGTERM or SIGINT — graceful shutdown
                            info!(
                                "🛑 Validator terminated by signal {} — shutting down supervisor",
                                sig
                            );
                            break;
                        }
                        warn!("💥 Validator killed by signal {} — restarting", sig);
                    }
                }
                restart_count += 1;
                if restart_count >= max_restarts {
                    error!("❌ Max restarts reached after signal kill — giving up");
                    std::process::exit(1);
                }
                std::thread::sleep(Duration::from_secs(backoff_secs));
                backoff_secs = (backoff_secs * 2).min(30);
            }
        }
    }
}

/// Synchronous wrapper that sets up the tokio runtime and runs the validator.
fn run_validator_sync() {
    // Use at least 4 worker threads, or the number of CPU cores, whichever is
    // greater. This ensures the RPC server, P2P layer, and block production
    // each get dedicated threads and don't starve each other under load.
    let worker_threads = std::thread::available_parallelism()
        .map(|n| n.get().max(4))
        .unwrap_or(4);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(worker_threads)
        .build();
    let rt = match rt {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("Failed to build tokio runtime: {}", err);
            return;
        }
    };
    eprintln!("Tokio runtime: {} worker threads", worker_threads);
    rt.block_on(run_validator());
}

/// The actual validator entrypoint — all existing logic lives here.
async fn run_validator() {
    // ── Logging ──
    // Parse data-dir early so we can place log files inside it.
    let pre_args: Vec<String> = env::args().collect();
    let pre_data_dir = get_flag_value(&pre_args, &["--db-path", "--db", "--data-dir"])
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let port = get_flag_value(&pre_args, &["--p2p-port"])
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(7001);
            format!("./data/state-{}", port)
        });
    // Canonicalize early so logs go to the same absolute path as the DB
    let pre_data_dir = {
        let p = PathBuf::from(&pre_data_dir);
        let _ = fs::create_dir_all(&p);
        std::fs::canonicalize(&p).unwrap_or_else(|_| {
            if p.is_absolute() {
                p
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(&p)
            }
        })
    };
    let log_dir = pre_data_dir.join("logs");
    let _ = fs::create_dir_all(&log_dir);

    // Rolling daily file appender — creates files like validator.2026-02-15.log
    let file_appender = tracing_appender::rolling::daily(&log_dir, "validator.log");
    let (non_blocking_writer, _guard) = tracing_appender::non_blocking(file_appender);

    // Layered subscriber: stdout (with ANSI colors) + rolling file (plain text)
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_ansi(true))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(non_blocking_writer),
        )
        .with(tracing_subscriber::filter::LevelFilter::INFO)
        .try_init();

    // Background task: sweep log files older than 7 days every 3 hours
    spawn_log_cleanup_task(log_dir.clone(), 7);

    info!("🦞 Lichen Validator starting...");

    // Parse CLI args for P2P configuration
    let args: Vec<String> = env::args().collect();

    // Parse --genesis flag
    let genesis_path = get_flag_value(&args, &["--genesis"]).map(|s| s.to_string());

    // Parse --network flag (testnet | mainnet)
    let network_arg = get_flag_value(&args, &["--network"]).map(|s| s.to_lowercase());

    // Parse --p2p-port flag properly
    let p2p_port = get_flag_value(&args, &["--p2p-port"])
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(7001);

    // Parse --db-path / --db / --data-dir flag or use default based on port
    let data_dir = get_flag_value(&args, &["--db-path", "--db", "--data-dir"])
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("./data/state-{}", p2p_port));
    // Canonicalize to absolute path to prevent CWD-dependent state location
    let data_dir_path = std::fs::canonicalize(&data_dir).unwrap_or_else(|_| {
        // Directory doesn't exist yet — resolve parent + leaf
        let p = PathBuf::from(&data_dir);
        if p.is_absolute() {
            p
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(&p)
        }
    });
    let data_dir = data_dir_path.to_string_lossy().to_string();
    info!("📂 Data directory: {}", data_dir);

    let signer_bind = match env::var("LICHEN_SIGNER_BIND") {
        Ok(value) if value.eq_ignore_ascii_case("off") => None,
        Ok(value) => Some(value),
        Err(_) => {
            let offset = p2p_port % 1000;
            let derived_port = 9200u16.saturating_add(offset);
            Some(format!("127.0.0.1:{}", derived_port))
        }
    };

    if let Some(bind) = signer_bind {
        if let Ok(addr) = bind.parse::<SocketAddr>() {
            if !addr.ip().is_loopback() {
                warn!(
                    "LICHEN_SIGNER_BIND is exposed on {}. Use loopback or a private interface only.",
                    addr
                );
            }
            let signer_data_dir = data_dir_path.clone();
            tokio::spawn(async move {
                threshold_signer::start_signer_server(addr, &signer_data_dir).await;
            });
        } else {
            warn!("Invalid LICHEN_SIGNER_BIND value: {}", bind);
        }
    }

    // Parse --cache-size-mb flag for RocksDB shared block cache
    let cache_size_mb: Option<usize> =
        get_flag_value(&args, &["--cache-size-mb"]).and_then(|s| s.parse().ok());

    // Open state database
    let mut state = match StateStore::open_with_cache_mb(&data_dir, cache_size_mb) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open state: {}", e);
            return;
        }
    };

    // ── P2-3: Open cold/archival storage if --cold-store is given ──
    let cold_store_path: Option<String> =
        get_flag_value(&args, &["--cold-store"]).map(|s| s.to_string());

    if let Some(ref cold_path) = cold_store_path {
        if let Err(e) = state.open_cold_store(cold_path) {
            error!("Failed to open cold store at {}: {}", cold_path, e);
            return;
        }
    }

    // Create transaction processor
    let processor = Arc::new(TxProcessor::new(state.clone()));

    // Load ZK verification keys at runtime (if present) so shielded tx
    // verification is available outside tests.
    try_load_runtime_zk_verification_keys(&processor, &data_dir_path);

    // ========================================================================
    // GENESIS CONFIGURATION
    // ========================================================================

    // Load genesis configuration from file or use defaults
    let data_dir_genesis = data_dir_path.join("genesis.json");
    let effective_genesis_path = genesis_path.clone().or_else(|| {
        data_dir_genesis
            .exists()
            .then(|| data_dir_genesis.display().to_string())
    });

    let genesis_config = if let Some(ref genesis_file) = effective_genesis_path {
        info!("📜 Loading genesis from: {}", genesis_file);
        match GenesisConfig::from_file(genesis_file) {
            Ok(config) => {
                info!("✓ Genesis loaded successfully");
                info!("  Chain ID: {}", config.chain_id);
                info!("  Total supply: {} LICN", config.total_supply_licn());
                info!("  Initial validators: {}", config.initial_validators.len());
                config
            }
            Err(e) => {
                error!("Failed to load genesis: {}", e);
                return;
            }
        }
    } else {
        match network_arg.as_deref() {
            Some("mainnet") => {
                info!("⚠️  No genesis file specified, using default mainnet genesis");
                GenesisConfig::default_mainnet()
            }
            Some("testnet") | None => {
                info!("⚠️  No genesis file specified, using default testnet genesis");
                GenesisConfig::default_testnet()
            }
            Some(other) => {
                warn!(
                    "⚠️  Unknown network '{}', defaulting to testnet genesis",
                    other
                );
                GenesisConfig::default_testnet()
            }
        }
    };

    // P2P NETWORK SETUP - do this early to check if joining existing network
    info!("🦞 Initializing P2P network...");

    // Parse seed peers from CLI
    // Supports:
    //   --bootstrap <host:port>
    //   --bootstrap-peers <host:port,host:port>
    //   positional peers (legacy)
    let mut seed_peer_strings: Vec<String> = Vec::new();
    let mut explicit_seed_peer_strings: Vec<String> = Vec::new();

    // Known flags that take a value — used to skip their arguments in the
    // positional-peer fallback below.
    const VALUE_FLAGS: &[&str] = &[
        "--bootstrap",
        "--bootstrap-peers",
        "--rpc-port",
        "--ws-port",
        "--p2p-port",
        "--db-path",
        "--db",
        "--data-dir",
        "--genesis",
        "--keypair",
        "--import-key",
        "--network",
        "--admin-token",
        "--watchdog-timeout",
        "--max-restarts",
        "--listen-addr",
        "--auto-update",
        "--update-check-interval",
        "--update-channel",
        "--cache-size-mb",
        "--cold-store",
    ];
    const BOOL_FLAGS: &[&str] = &[
        "--supervised",
        "--no-watchdog",
        "--no-auto-restart",
        "--dev-mode",
    ];

    // Extract --bootstrap / --bootstrap-peers via get_flag_value helpers
    if let Some(val) = get_flag_value(&args, &["--bootstrap"]) {
        seed_peer_strings.push(val.to_string());
        explicit_seed_peer_strings.push(val.to_string());
    }
    if let Some(val) = get_flag_value(&args, &["--bootstrap-peers"]) {
        for part in val.split(',') {
            seed_peer_strings.push(part.to_string());
            explicit_seed_peer_strings.push(part.to_string());
        }
    }

    // Env var fallback — LICHEN_BOOTSTRAP_PEERS provides reliable delivery
    // of bootstrap peers without systemd word-splitting issues that can break
    // LICHEN_EXTRA_ARGS expansion in ExecStart.
    if explicit_seed_peer_strings.is_empty() {
        if let Ok(peers) = std::env::var("LICHEN_BOOTSTRAP_PEERS") {
            for part in peers.split(',') {
                let trimmed = part.trim();
                if !trimmed.is_empty() {
                    seed_peer_strings.push(trimmed.to_string());
                    explicit_seed_peer_strings.push(trimmed.to_string());
                }
            }
            if !explicit_seed_peer_strings.is_empty() {
                info!(
                    "📡 Loaded {} bootstrap peer(s) from LICHEN_BOOTSTRAP_PEERS env var",
                    explicit_seed_peer_strings.len()
                );
            }
        }
    }

    // Collect positional peer arguments (legacy)
    let mut skip_next = false;
    for (i, arg) in args.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if i == 0 {
            continue; // binary name
        }
        // Check if this arg is a known flag (either --flag or --flag=value)
        let is_flag = VALUE_FLAGS
            .iter()
            .any(|f| arg == *f || arg.starts_with(&format!("{}=", f)))
            || BOOL_FLAGS
                .iter()
                .any(|f| arg == *f || arg.starts_with(&format!("{}=", f)));
        if is_flag {
            // If it's a space-separated value flag (not --flag=...), skip next arg
            if VALUE_FLAGS.iter().any(|f| arg == *f) {
                skip_next = true;
            }
            continue;
        }
        seed_peer_strings.push(arg.to_string());
        explicit_seed_peer_strings.push(arg.to_string());
    }

    // Parse --listen-addr flag for P2P bind address.
    // Default 0.0.0.0 — binding to loopback prevents outbound QUIC connections
    // from reaching external peers (sendmsg EADDRNOTAVAIL).
    let listen_host = get_flag_value(&args, &["--listen-addr"])
        .unwrap_or("0.0.0.0")
        .to_string();

    // ── Auto-Update Configuration ───────────────────────────────────────
    let auto_update_mode = get_flag_value(&args, &["--auto-update"])
        .map(updater::UpdateMode::parse_mode)
        .unwrap_or(updater::UpdateMode::Off);

    let update_check_interval = get_flag_value(&args, &["--update-check-interval"])
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(300);

    let update_channel = get_flag_value(&args, &["--update-channel"])
        .unwrap_or("stable")
        .to_string();

    let no_auto_restart = has_flag(&args, "--no-auto-restart");

    let update_config = updater::UpdateConfig {
        mode: auto_update_mode,
        check_interval_secs: update_check_interval,
        channel: update_channel,
        no_auto_restart,
        jitter_max_secs: 60,
        target_binary: "lichen-validator".to_string(),
        companion_binaries: discover_companion_binaries(),
    };

    // Spawn auto-updater background task
    info!("🔄 Validator version: v{}", updater::VERSION);
    let _updater_handle = updater::spawn_update_checker(update_config);

    let data_dir_path = Path::new(&data_dir);
    let validator_runtime_home = resolve_validator_runtime_home(data_dir_path);
    if let Err(err) = std::fs::create_dir_all(&validator_runtime_home) {
        warn!(
            "Failed to create validator runtime home {}: {}",
            validator_runtime_home.display(),
            err
        );
    }
    info!(
        "🏠 Validator runtime home: {}",
        validator_runtime_home.display()
    );
    let peer_store_path = data_dir_path.join("known-peers.json");
    let listen_addr: SocketAddr = match format!("{}:{}", listen_host, p2p_port).parse() {
        Ok(addr) => addr,
        Err(err) => {
            warn!(
                "Invalid listen address '{}:{}' ({}); falling back to 127.0.0.1:{}",
                listen_host, p2p_port, err, p2p_port
            );
            SocketAddr::from(([127, 0, 0, 1], p2p_port))
        }
    };

    let mut seed_peers = resolve_peer_list(&seed_peer_strings);
    let explicit_seed_peers = resolve_peer_list(&explicit_seed_peer_strings);
    // Search seeds.json in multiple locations
    let seeds_candidates = [
        data_dir_path.join("seeds.json"),
        PathBuf::from("/etc/lichen/seeds.json"),
        PathBuf::from("seeds.json"),
    ];
    let seeds_path = seeds_candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| PathBuf::from("seeds.json"));
    let local_only = listen_addr.ip().is_loopback();
    let cached_peers = if explicit_seed_peers.is_empty() && !local_only {
        let seed_file_peers = load_seed_peers(&genesis_config.chain_id, &seeds_path);
        if !seed_file_peers.is_empty() {
            info!(
                "📖 Loaded {} seed peers from {}",
                seed_file_peers.len(),
                seeds_path.display()
            );
        }
        seed_peers.extend(resolve_peer_list(&seed_file_peers));
        let cached = lichen_p2p::PeerStore::load_from_path(&peer_store_path);
        seed_peers.extend(cached.iter().copied());
        cached
    } else if !explicit_seed_peers.is_empty() {
        info!("🔒 Using explicit bootstrap peers (--bootstrap-peers)");
        // Still load seeds.json for additional connectivity
        let seed_file_peers = load_seed_peers(&genesis_config.chain_id, &seeds_path);
        seed_peers.extend(resolve_peer_list(&seed_file_peers));
        Vec::new()
    } else {
        info!("🔒 Local-only mode: external seed peers disabled");
        Vec::new()
    };

    // Collect all local IP addresses so we can filter out self-referencing seeds
    let local_ips: HashSet<std::net::IpAddr> = {
        let mut ips = HashSet::new();
        ips.insert(listen_addr.ip());
        ips.insert(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        ips.insert(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        // Discover interface IPs
        if let Ok(addrs) = std::net::UdpSocket::bind("0.0.0.0:0") {
            // Try connecting to a public IP to find our outbound address
            if addrs.connect("8.8.8.8:80").is_ok() {
                if let Ok(local) = addrs.local_addr() {
                    ips.insert(local.ip());
                }
            }
        }
        // Also check all network interfaces via hostname lookup
        if let Ok(hostname) = std::process::Command::new("hostname").arg("-I").output() {
            if let Ok(output) = std::str::from_utf8(&hostname.stdout) {
                for part in output.split_whitespace() {
                    if let Ok(ip) = part.parse::<std::net::IpAddr>() {
                        ips.insert(ip);
                    }
                }
            }
        }
        ips
    };

    let mut seen = HashSet::new();
    seed_peers.retain(|addr| {
        // Filter out ourselves — by listen addr match OR by matching any local IP + port
        if *addr == listen_addr {
            return false;
        }
        if local_ips.contains(&addr.ip()) && addr.port() == listen_addr.port() {
            return false;
        }
        seen.insert(*addr)
    });

    let p2p_config = P2PConfig {
        listen_addr,
        seed_peers: seed_peers.clone(),
        gossip_interval: 10,
        cleanup_timeout: 300,
        runtime_home: Some(validator_runtime_home),
        peer_store_path: Some(peer_store_path.clone()),
        max_known_peers: 200,
        // P2P role: read from LICHEN_P2P_ROLE env var, default to Validator
        role: std::env::var("LICHEN_P2P_ROLE")
            .ok()
            .and_then(|s| s.parse::<NodeRole>().ok())
            .unwrap_or(NodeRole::Validator),
        // P2P max_peers: read from LICHEN_P2P_MAX_PEERS env var, or auto-set by role
        max_peers: std::env::var("LICHEN_P2P_MAX_PEERS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok()),
        // P2P reserved relay peers: read from LICHEN_P2P_RESERVED_PEERS env var (comma-separated)
        reserved_relay_peers: std::env::var("LICHEN_P2P_RESERVED_PEERS")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        // P3-6: External address for NAT traversal (from LICHEN_EXTERNAL_ADDR env var)
        external_addr: std::env::var("LICHEN_EXTERNAL_ADDR")
            .ok()
            .and_then(|s| s.parse::<std::net::SocketAddr>().ok()),
    };

    let has_genesis_block = state.get_block_by_slot(0).unwrap_or(None).is_some();
    let last_slot = state.get_last_slot().unwrap_or(0);
    let has_any_seed_peers =
        !explicit_seed_peers.is_empty() || !cached_peers.is_empty() || !seed_peers.is_empty();

    // ────────────────────────────────────────────────────────────────
    // STARTUP MODE: RESUME vs JOIN
    // ────────────────────────────────────────────────────────────────
    // How every blockchain works:
    //   - State on disk (last_slot > 0)  → RESUME. Load state, start consensus.
    //   - No state, seeds exist          → JOIN.   Sync from peers first.
    //   - No state, no seeds             → ERROR.  Can't start.
    //
    // That's it. No metadata checks, no join_complete flags, no special
    // cases for --bootstrap-peers on restart. If the node has blocks,
    // it resumes from where it left off. Period.
    // ────────────────────────────────────────────────────────────────
    let is_joining_network = if last_slot > 0 || has_genesis_block {
        // Node has state on disk — resume consensus.
        info!(
            "🔄 Resuming from slot {} (genesis: {})",
            last_slot,
            if has_genesis_block {
                "present"
            } else {
                "missing"
            }
        );
        false
    } else if has_any_seed_peers {
        // Fresh node with no state — join the existing network
        info!("🔄 Fresh node — will sync from existing network");
        info!(
            "   Seeds: {} explicit, {} from seeds.json, {} cached",
            explicit_seed_peers.len(),
            seed_peers.len().saturating_sub(explicit_seed_peers.len()),
            cached_peers.len(),
        );
        true
    } else {
        // No state, no seeds — can't start
        error!("❌ No blocks on disk and no seed peers available.");
        error!("   Run lichen-genesis first, or provide --bootstrap-peers / seeds.json.");
        std::process::exit(1);
    };

    // ========================================================================
    // GENESIS STATE INITIALIZATION
    // ========================================================================

    // Genesis wallet path
    let genesis_wallet_path = data_dir_path.join("genesis-wallet.json");
    let genesis_keypairs_dir = data_dir_path.join("genesis-keys");
    std::fs::create_dir_all(&genesis_keypairs_dir).ok();

    // Load genesis wallet from disk (if present)
    let (genesis_wallet, genesis_pubkey) = if has_genesis_block {
        if genesis_wallet_path.exists() {
            match GenesisWallet::load(&genesis_wallet_path) {
                Ok(wallet) => (Some(wallet.clone()), Some(wallet.pubkey)),
                Err(e) => {
                    warn!("⚠️  Failed to load genesis wallet: {}", e);
                    (None, None)
                }
            }
        } else {
            warn!("⚠️  Genesis wallet not found; genesis will not be regenerated");
            (None, None)
        }
    } else {
        // Joining network — will sync genesis from peers
        info!("🔄 Joining existing network — genesis wallet will sync from peers");
        (None, None)
    };

    let genesis_exists = has_genesis_block;

    // --- Migration: ensure genesis/treasury pubkeys are stored in DB ---
    // Older DBs may not have these keys set. Backfill from genesis-wallet.json.
    if genesis_exists {
        if let Some(ref gpk) = genesis_pubkey {
            if state.get_genesis_pubkey().ok().flatten().is_none() {
                if let Err(e) = state.set_genesis_pubkey(gpk) {
                    warn!("⚠️  Migration: failed to set genesis pubkey: {}", e);
                } else {
                    info!("  ✓ Migration: stored genesis pubkey in DB");
                }
            }
        }
        if let Some(ref gw) = genesis_wallet {
            if let Some(ref tpk) = gw.treasury_pubkey {
                if state.get_treasury_pubkey().ok().flatten().is_none() {
                    if let Err(e) = state.set_treasury_pubkey(tpk) {
                        warn!("⚠️  Migration: failed to set treasury pubkey: {}", e);
                    } else {
                        info!("  ✓ Migration: stored treasury pubkey in DB");
                    }
                }
            }
        }
        // Backfill genesis accounts from wallet if missing in DB
        if state
            .get_genesis_accounts()
            .map(|v| v.is_empty())
            .unwrap_or(true)
        {
            if let Some(ref gw) = genesis_wallet {
                if let Some(ref dist_wallets) = gw.distribution_wallets {
                    let ga_entries: Vec<(String, Pubkey, u64, u8)> = dist_wallets
                        .iter()
                        .map(|dw| (dw.role.clone(), dw.pubkey, dw.amount_licn, dw.percentage))
                        .collect();
                    if let Err(e) = state.set_genesis_accounts(&ga_entries) {
                        warn!("⚠️  Migration: failed to store genesis accounts: {}", e);
                    } else {
                        info!(
                            "  ✓ Migration: stored {} genesis accounts in DB",
                            ga_entries.len()
                        );
                    }
                }
            }
        }
    }

    // --- Fetch genesis accounts from bootstrap peer if still missing ---
    // This handles V2/V3 joining the network without genesis-wallet.json
    if state
        .get_genesis_accounts()
        .map(|v| v.is_empty())
        .unwrap_or(true)
        && !explicit_seed_peer_strings.is_empty()
    {
        info!("  🔄 Fetching genesis accounts from bootstrap peer...");
        for peer in &explicit_seed_peer_strings {
            // Derive RPC port from P2P port
            let parts: Vec<&str> = peer.split(':').collect();
            if let (Some(host), Some(p2p_port_str)) = (parts.first(), parts.get(1)) {
                if let Ok(peer_p2p) = p2p_port_str.parse::<u16>() {
                    // AUDIT-FIX V5.1: Use the same port derivation formula
                    // as the RPC server binding (L6410). The previous formula
                    // used `peer_p2p % 1000` which produced wrong ports for
                    // V2/V3 validators (e.g. p2p=7002 → rpc=8901).
                    let base_p2p = if peer_p2p >= 8000 { 8001u16 } else { 7001u16 };
                    let base_rpc = if peer_p2p >= 8000 { 9899u16 } else { 8899u16 };
                    let offset = peer_p2p.saturating_sub(base_p2p);
                    let peer_rpc = base_rpc.saturating_add(offset.saturating_mul(2));
                    let url = format!("http://{}:{}/", host, peer_rpc);
                    let body = serde_json::json!({
                        "jsonrpc": "2.0", "id": 1, "method": "getGenesisAccounts"
                    });
                    match reqwest::Client::new()
                        .post(&url)
                        .json(&body)
                        .timeout(std::time::Duration::from_secs(5))
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            if let Ok(json) = resp.json::<serde_json::Value>().await {
                                if let Some(accounts) = json["result"]["accounts"].as_array() {
                                    let mut ga_entries = Vec::new();
                                    for acc in accounts {
                                        let role = acc["role"].as_str().unwrap_or("").to_string();
                                        if role == "genesis" {
                                            continue; // Skip the genesis signer entry
                                        }
                                        let pk_str = acc["pubkey"].as_str().unwrap_or("");
                                        if let Ok(pk) = Pubkey::from_base58(pk_str) {
                                            let amt = acc["amount_licn"].as_u64().unwrap_or(0);
                                            let pct = acc["percentage"].as_u64().unwrap_or(0) as u8;
                                            ga_entries.push((role, pk, amt, pct));
                                        }
                                    }
                                    if !ga_entries.is_empty() {
                                        if let Err(e) = state.set_genesis_accounts(&ga_entries) {
                                            warn!(
                                                "⚠️  Failed to store fetched genesis accounts: {}",
                                                e
                                            );
                                        } else {
                                            info!(
                                                "  ✓ Fetched {} genesis accounts from {}",
                                                ga_entries.len(),
                                                peer
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "  ⚠️  Failed to fetch genesis accounts from {}: {}",
                                peer, e
                            );
                        }
                    }
                }
            }
        }
    }

    if genesis_exists {
        info!("✓ Genesis state already exists");
        let last_slot = state.get_last_slot().unwrap_or(0);
        info!("  Resuming from slot {}", last_slot);

        // Account reconciliation disabled on startup (too slow for large databases)
        // Use CLI command `lichen admin reconcile-accounts` if needed
        let metrics = state.get_metrics();
        info!("  Total accounts (counter): {}", metrics.total_accounts);

        if let Some(wallet) = genesis_wallet.as_ref() {
            if let Some(treasury_pubkey) = wallet.treasury_pubkey {
                // Only set if not already stored (avoid overwriting canonical pubkey)
                if state.get_treasury_pubkey().ok().flatten().is_none() {
                    state.set_treasury_pubkey(&treasury_pubkey).ok();
                }
                if let Ok(None) = state.get_account(&treasury_pubkey) {
                    let treasury_account = Account::new(0, SYSTEM_ACCOUNT_OWNER);
                    state.put_account(&treasury_pubkey, &treasury_account).ok();
                }
            }
        }

        // ================================================================
        // MIGRATION: Auto-generate treasury keypair if missing
        // Handles genesis wallets created by older code versions that
        // did not generate a separate treasury keypair.
        // ================================================================
        let needs_treasury_migration = genesis_wallet
            .as_ref()
            .map(|w| w.treasury_pubkey.is_none())
            .unwrap_or(false);

        if needs_treasury_migration && state.get_treasury_pubkey().ok().flatten().is_none() {
            info!("🔄 MIGRATION: Genesis wallet missing treasury keypair — generating...");

            let treasury_keypair = Keypair::generate();
            let treasury_pubkey = treasury_keypair.pubkey();

            // 1. Save treasury keypair to disk
            match GenesisWallet::save_treasury_keypair(
                &treasury_keypair,
                &genesis_keypairs_dir,
                &genesis_config.chain_id,
            ) {
                Ok(path) => info!("  ✓ Treasury keypair saved: {}", path),
                Err(e) => error!("  ✗ Failed to save treasury keypair: {}", e),
            }

            // 2. Set treasury pubkey in state
            if let Err(e) = state.set_treasury_pubkey(&treasury_pubkey) {
                error!("  ✗ Failed to set treasury pubkey in state: {}", e);
            }

            // 3. Create treasury account
            let mut treasury_account = Account::new(0, SYSTEM_ACCOUNT_OWNER);

            // 4. Fund treasury from genesis account (treasury reserve for bootstrap grants)
            let reward_spores = Account::licn_to_spores(TREASURY_RESERVE_LICN.min(1_000_000_000));
            if let Some(genesis_pk) = genesis_wallet.as_ref().map(|w| w.pubkey) {
                if let Ok(Some(mut genesis_acct)) = state.get_account(&genesis_pk) {
                    if genesis_acct.spendable >= reward_spores {
                        genesis_acct.spores = genesis_acct.spores.saturating_sub(reward_spores);
                        genesis_acct.spendable =
                            genesis_acct.spendable.saturating_sub(reward_spores);
                        treasury_account.spores = reward_spores;
                        treasury_account.spendable = reward_spores;
                        state.put_account(&genesis_pk, &genesis_acct).ok();
                        info!(
                            "  ✓ Funded treasury with {} LICN from genesis (bootstrap reserve)",
                            TREASURY_RESERVE_LICN
                        );
                    } else {
                        warn!(
                            "  ⚠️  Genesis account has insufficient spendable balance ({} < {})",
                            genesis_acct.spendable, reward_spores
                        );
                    }
                }
            }

            state.put_account(&treasury_pubkey, &treasury_account).ok();
            info!("  ✓ Treasury account: {}", treasury_pubkey.to_base58());

            // 5. Update genesis wallet JSON with treasury info
            if let Some(mut wallet) = genesis_wallet.clone() {
                wallet.treasury_pubkey = Some(treasury_pubkey);
                wallet.treasury_keypair_path = Some(format!(
                    "genesis-keys/treasury-{}.json",
                    genesis_config.chain_id
                ));
                if let Err(e) = wallet.save(&genesis_wallet_path) {
                    error!("  ✗ Failed to update genesis wallet: {}", e);
                } else {
                    info!("  ✓ Updated genesis-wallet.json with treasury info");
                }
            }

            // 6. Persist fee config only if not already present
            {
                if state.get_fee_config().is_err() {
                    let fee_config = FeeConfig::default_from_constants();
                    if let Err(e) = state.set_fee_config_full(&fee_config) {
                        warn!("  ⚠️  Failed to persist fee config: {}", e);
                    } else {
                        info!("  ✓ Fee config persisted");
                    }
                }
            }

            info!("✅ Treasury migration complete");
        }

        // ================================================================
        // STARTUP RECONCILIATION: Correct legacy deploy-fee typo in DB.
        // Some nodes persisted 2.5 LICN instead of canonical 25 LICN.
        // ================================================================
        {
            match state.get_fee_config() {
                Ok(mut cfg) if cfg.contract_deploy_fee == LEGACY_CONTRACT_DEPLOY_FEE_SPORES => {
                    warn!(
                        "🔧 RECONCILE: correcting legacy contract deploy fee {} -> {} spores",
                        cfg.contract_deploy_fee, CONTRACT_DEPLOY_FEE
                    );
                    cfg.contract_deploy_fee = CONTRACT_DEPLOY_FEE;
                    if let Err(e) = state.set_fee_config_full(&cfg) {
                        error!("  ✗ Failed to reconcile contract deploy fee: {}", e);
                    } else {
                        info!(
                            "  ✓ Contract deploy fee reconciled to {} spores (25 LICN)",
                            CONTRACT_DEPLOY_FEE
                        );
                    }
                }
                Ok(_) => {}
                Err(e) => warn!("⚠️  Unable to read fee config for reconciliation: {}", e),
            }
        }

        // ================================================================
        // STARTUP RECONCILIATION: Seed analytics prices if missing.
        // genesis_seed_analytics_prices was added after genesis block 0
        // was already created, so the data was never written. This check
        // runs on every startup and writes the seed data exactly once.
        // Also reconciles oracle price feeds if missing.
        // ================================================================
        {
            let genesis_pk = genesis_wallet
                .as_ref()
                .map(|w| w.pubkey)
                .unwrap_or(Pubkey([0u8; 32]));
            let genesis_timestamp = state
                .get_block_by_slot(0)
                .ok()
                .flatten()
                .map(|block| block.header.timestamp)
                .unwrap_or(0);

            // Check if analytics seed data is present (ana_lp_1 = LICN/lUSD)
            let ana_lp_1_exists = state
                .get_program_storage("ANALYTICS", b"ana_lp_1")
                .is_some();

            if !ana_lp_1_exists {
                info!("🔄 RECONCILE: Analytics price seeds missing — writing initial prices");
                genesis_seed_analytics_prices(&state, &genesis_pk, genesis_timestamp);
                info!("  ✓ Analytics prices seeded for pairs 1-5");
            }

            seed_bootstrap_consensus_oracle_prices(&state, state.get_last_slot().unwrap_or(0));

            // Check if oracle price feeds are present (price_LICN)
            let licn_price_exists = state.get_program_storage("ORACLE", b"price_LICN").is_some();

            // Check if margin prices are present (mrg_mark_1 = LICN/lUSD)
            let mrg_mark_1_exists = state.get_program_storage("MARGIN", b"mrg_mark_1").is_some();

            if !mrg_mark_1_exists {
                info!("🔄 RECONCILE: Margin prices missing — seeding mark/index prices");
                genesis_seed_margin_prices(&state, &genesis_pk, genesis_timestamp);
                info!("  ✓ Margin prices seeded for pairs 1-5");
            }

            if !licn_price_exists {
                info!("🔄 RECONCILE: Oracle price feeds missing — seeding initial prices");
                // Write oracle prices directly to contract storage
                // (WASM calls may not work on existing DB, so use direct writes)
                if let Some(oracle_pk) = derive_contract_address(&genesis_pk, "lichenoracle") {
                    const ORACLE_DECIMALS: u8 = 8;
                    let oracle_feeds: &[(&str, u64)] = &[
                        ("LICN", genesis_licn_price_8dec()),
                        ("wSOL", genesis_wsol_price_8dec()),
                        ("wETH", genesis_weth_price_8dec()),
                        ("wBNB", genesis_wbnb_price_8dec()),
                    ];

                    for (asset, price) in oracle_feeds {
                        // price_{asset}: 8 bytes LE (u64)
                        let price_key = format!("price_{}", asset);
                        let _ = state.put_contract_storage(
                            &oracle_pk,
                            price_key.as_bytes(),
                            &price.to_le_bytes(),
                        );

                        // price_{asset}_ts: 8 bytes LE (u64 timestamp)
                        let ts_key = format!("price_{}_ts", asset);
                        let _ = state.put_contract_storage(
                            &oracle_pk,
                            ts_key.as_bytes(),
                            &genesis_timestamp.to_le_bytes(),
                        );

                        // price_{asset}_dec: 1 byte (decimals)
                        let dec_key = format!("price_{}_dec", asset);
                        let _ = state.put_contract_storage(
                            &oracle_pk,
                            dec_key.as_bytes(),
                            &[ORACLE_DECIMALS],
                        );

                        info!(
                            "  ✓ Oracle price seeded: {} = {} ({}dec)",
                            asset, price, ORACLE_DECIMALS
                        );
                    }
                }
            }
        }
    }

    // Treasury keypair kept for governance/manual operations and airdrop signing.
    // Block rewards use protocol-level coinbase (no signing needed).
    let treasury_keypair = load_treasury_keypair(
        genesis_wallet.as_ref(),
        data_dir_path,
        &genesis_keypairs_dir,
        &genesis_config.chain_id,
    );
    let min_validator_stake = genesis_config.consensus.min_validator_stake;

    // ========================================================================
    // VALIDATOR IDENTITY
    // ========================================================================

    // Parse --dev-mode flag (disables machine fingerprint, blocks mainnet)
    let dev_mode = has_flag(&args, "--dev-mode");
    if dev_mode {
        info!("🔧 Developer mode enabled — machine fingerprint disabled");
        if genesis_config.chain_id.contains("mainnet") {
            error!("❌ --dev-mode cannot be used on mainnet — aborting");
            std::process::exit(1);
        }
    }

    // Parse --import-key: copy an existing keypair file into the validator data directory,
    // then use it as the validator identity. This is for machine migration.
    if let Some(import_pos) = args
        .iter()
        .position(|arg| arg == "--import-key" || arg.starts_with("--import-key="))
    {
        let import_path = if args[import_pos].starts_with("--import-key=") {
            args[import_pos]
                .strip_prefix("--import-key=")
                .unwrap()
                .to_string()
        } else if let Some(p) = args.get(import_pos + 1) {
            p.to_string()
        } else {
            error!("❌ --import-key requires a file path argument");
            std::process::exit(1);
        };
        let source = Path::new(&import_path);
        if !source.exists() {
            error!("❌ --import-key file not found: {}", import_path);
            std::process::exit(1);
        }
        let dest = keypair_loader::default_validator_keypair_path(p2p_port);
        if dest.exists() {
            // Back up existing keypair before overwriting
            let backup = dest.with_extension("json.bak");
            info!("📋 Backing up existing keypair to {:?}", backup);
            if let Err(e) = fs::copy(&dest, &backup) {
                warn!("⚠️  Failed to backup existing keypair: {}", e);
            }
        }
        info!("🔑 Importing keypair from {:?} → {:?}", source, dest);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).ok();
        }
        if let Err(e) = fs::copy(source, &dest) {
            error!("❌ Failed to copy keypair file for --import-key: {}", e);
            std::process::exit(1);
        }
        // Set restrictive permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).ok();
        }
        info!(
            "✅ Keypair imported successfully — this validator will resume the imported identity"
        );
    }

    // Load validator keypair from file (production-ready)
    // Priority order:
    // 1. --keypair CLI argument
    // 2. LICHEN_VALIDATOR_KEYPAIR env var
    // 3. ~/.lichen/validators/validator-{port}.json
    // 4. Generate new and save

    let keypair_path = get_flag_value(&args, &["--keypair"]);

    let validator_keypair = match keypair_loader::load_or_generate_keypair(
        keypair_path,
        p2p_port,
        Some(data_dir_path),
        network_arg.as_deref(),
    ) {
        Ok(keypair) => keypair,
        Err(err) => {
            error!("Failed to load or generate validator keypair: {}", err);
            std::process::exit(1);
        }
    };

    let validator_pubkey = validator_keypair.pubkey();
    info!("🦞 Validator identity: {}", validator_pubkey.to_base58());
    info!("   Port: {}, Keypair loaded successfully", p2p_port);

    // ========================================================================
    // MACHINE FINGERPRINT (Anti-Sybil)
    // ========================================================================

    let machine_fingerprint = if dev_mode {
        // Dev mode: SHA-256(pubkey) — unique per key, not per machine.
        // This allows multi-validator on one machine while still tracking per-key.
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(validator_pubkey.0);
        let result = hasher.finalize();
        let mut fp = [0u8; 32];
        fp.copy_from_slice(&result);
        info!(
            "🔧 Dev mode: fingerprint = SHA-256(pubkey) — {}..{}",
            hex::encode(&fp[..4]),
            hex::encode(&fp[28..])
        );
        fp
    } else {
        let fp = collect_machine_fingerprint();
        if fp == [0u8; 32] {
            warn!(
                "⚠️  Could not collect machine fingerprint — running without anti-Sybil protection"
            );
        } else {
            info!(
                "🔒 Machine fingerprint: {}..{}",
                hex::encode(&fp[..4]),
                hex::encode(&fp[28..])
            );
        }
        fp
    };

    // ========================================================================
    // VALIDATOR SET INITIALIZATION
    // ========================================================================

    // Load or initialize validator set (shared across tasks)
    let validator_set = Arc::new(RwLock::new({
        let mut set = state
            .load_validator_set()
            .unwrap_or_else(|_| ValidatorSet::new());

        if set.validators().is_empty() {
            // Add genesis validators from configuration
            for validator_info in &genesis_config.initial_validators {
                let pubkey = match Pubkey::from_base58(&validator_info.pubkey) {
                    Ok(pk) => pk,
                    Err(e) => {
                        warn!(
                            "Skipping initial validator with invalid pubkey {}: {e}",
                            validator_info.pubkey
                        );
                        continue;
                    }
                };

                let validator = ValidatorInfo {
                    pubkey,
                    stake: Account::licn_to_spores(validator_info.stake_licn),
                    reputation: validator_info.reputation,
                    blocks_proposed: 0,
                    votes_cast: 0,
                    correct_votes: 0,
                    last_active_slot: 0,
                    joined_slot: 0,
                    commission_rate: 500,
                    transactions_processed: 0,
                    pending_activation: false, // Genesis validators active immediately
                };

                set.add_validator(validator);
            }
        }

        // Add this validator if not already in genesis set
        // ⚠️ CRITICAL: Prevent genesis wallet from becoming a validator
        // Use on-chain stake (from RegisterValidator tx) — NOT BOOTSTRAP_GRANT_AMOUNT.
        // Validators must register through consensus; ValidatorSet is for peer routing only.
        let on_chain_stake = state
            .get_account(&validator_pubkey)
            .unwrap_or(None)
            .map(|a| a.staked)
            .unwrap_or(0);
        // Use current chain tip so snapshots shared with peers pass the
        // slot-drift check (MAX_SLOT_DRIFT_FOR_NEW_VALIDATOR = 500).
        let current_tip = state.get_last_slot().unwrap_or(0);

        if let Some(genesis_pubkey) = genesis_pubkey {
            if validator_pubkey != genesis_pubkey {
                if !genesis_config
                    .initial_validators
                    .iter()
                    .any(|v| v.pubkey == validator_pubkey.to_base58())
                {
                    let pending =
                        should_add_local_validator_as_pending(is_joining_network, current_tip);
                    info!("📋 This validator not in genesis set, adding for peer routing (on-chain stake: {} LICN, pending: {})",
                        on_chain_stake / 1_000_000_000, pending);
                    set.add_validator(ValidatorInfo {
                        pubkey: validator_pubkey,
                        stake: on_chain_stake,
                        reputation: 100,
                        blocks_proposed: 0,
                        votes_cast: 0,
                        correct_votes: 0,
                        last_active_slot: current_tip,
                        joined_slot: current_tip,
                        commission_rate: 500,
                        transactions_processed: 0,
                        pending_activation: pending,
                    });
                }
            } else {
                info!("🚫 Genesis wallet cannot be a validator");
            }
        } else if !genesis_config
            .initial_validators
            .iter()
            .any(|v| v.pubkey == validator_pubkey.to_base58())
        {
            let pending = should_add_local_validator_as_pending(is_joining_network, current_tip);
            info!("📋 This validator not in genesis set, adding for peer routing (on-chain stake: {} LICN, pending: {})",
                on_chain_stake / 1_000_000_000, pending);
            set.add_validator(ValidatorInfo {
                pubkey: validator_pubkey,
                stake: on_chain_stake,
                reputation: 100,
                blocks_proposed: 0,
                votes_cast: 0,
                correct_votes: 0,
                last_active_slot: current_tip,
                joined_slot: current_tip,
                commission_rate: 500,
                transactions_processed: 0,
                pending_activation: pending,
            });
        }

        set
    }));

    // CRITICAL: Remove genesis wallet from validator set if it exists (cleanup for old bug)
    if let Some(genesis_pubkey) = genesis_pubkey {
        if let Ok(Some(_)) = state.get_validator(&genesis_pubkey) {
            info!("🧹 Cleaning up: Removing genesis wallet from validator set");
            if let Err(e) = state.delete_validator(&genesis_pubkey) {
                eprintln!("Failed to delete genesis validator: {e}");
            }
        }
    }

    // ── Ghost validator purge ──
    // Remove validators that have NEVER produced a block and were never active.
    // These are artifacts from keypair changes, duplicate identities, or stale
    // bootstrap entries. Return their bootstrap grants to the treasury.
    {
        let current_slot = state.get_last_slot().unwrap_or(0);
        // Only purge after the initial bootstrap window (first 100 slots)
        if current_slot > 100 {
            let mut vs = validator_set.write().await;
            let ghost_pubkeys: Vec<Pubkey> = vs
                .validators()
                .iter()
                .filter(|v| {
                    v.pubkey != validator_pubkey
                        && v.blocks_proposed == 0
                        && v.last_active_slot == 0
                })
                .map(|v| v.pubkey)
                .collect();

            for ghost_pk in &ghost_pubkeys {
                vs.remove_validator(ghost_pk);
                info!(
                    "🧹 Purged ghost validator {} (never active, 0 blocks)",
                    ghost_pk.to_base58()
                );

                // Return bootstrap grant to treasury if the ghost received one
                if let Ok(Some(ghost_account)) = state.get_account(ghost_pk) {
                    if ghost_account.staked > 0 {
                        if let Ok(Some(tpk)) = state.get_treasury_pubkey() {
                            if let Ok(Some(mut treasury)) = state.get_account(&tpk) {
                                treasury.add_spendable(ghost_account.staked).ok();
                                if let Err(e) = state.put_account(&tpk, &treasury) {
                                    warn!("⚠️  Failed to return bootstrap grant to treasury: {e}");
                                } else {
                                    info!(
                                        "💰 Returned {} LICN bootstrap grant to treasury from ghost {}",
                                        ghost_account.staked / 1_000_000_000,
                                        ghost_pk.to_base58()
                                    );
                                }
                            }
                        }
                        // Zero out the ghost's account
                        let zeroed = Account::new(0, SYSTEM_ACCOUNT_OWNER);
                        state.put_account(ghost_pk, &zeroed).ok();
                    }
                }
            }

            if !ghost_pubkeys.is_empty() {
                info!(
                    "🧹 Purged {} ghost validator(s) on startup",
                    ghost_pubkeys.len()
                );
            }
        }
    }

    // Save validator set to RocksDB on EVERY boot.
    // clear_all_validators() inside save_validator_set removes ghost entries from old
    // keypairs while preserving reputation/metrics for current validators via the
    // in-memory set that was loaded from DB above.
    if let Err(e) = state.save_validator_set(&*validator_set.read().await) {
        eprintln!("Failed to save validator set: {e}");
    }

    info!(
        "✓ Validator set initialized with {} validators",
        validator_set.read().await.validators().len()
    );

    // ============================================================================
    // VALIDATOR ACCOUNT CREATION / BOOTSTRAP GRANT
    // ============================================================================

    // IDENTITY-FIX: Check if this machine fingerprint is already registered
    // to a DIFFERENT pubkey. This catches the case where HOME changed and a new
    // keypair was generated, but the machine already has a validator identity.
    if machine_fingerprint != [0u8; 32] {
        let persisted_pool = state.get_stake_pool().unwrap_or_else(|_| StakePool::new());
        if let Some(existing_pubkey) = persisted_pool.fingerprint_owner(&machine_fingerprint) {
            if existing_pubkey != &validator_pubkey {
                warn!(
                    "⚠️  IDENTITY CONFLICT: This machine already has validator identity {}",
                    existing_pubkey.to_base58()
                );
                warn!(
                    "   Current keypair produces {}, but fingerprint maps to {}",
                    validator_pubkey.to_base58(),
                    existing_pubkey.to_base58()
                );
                warn!("   This likely means HOME changed and a new keypair was generated.");
                warn!("   To fix: use --import-key to restore the old keypair, or");
                warn!(
                    "   copy the old validator-*.json keypair to {}/validator-keypair.json",
                    data_dir
                );
                error!("❌ Refusing to start with duplicate identity — each machine gets ONE validator grant.");
                std::process::exit(1);
            }
        }
    }

    // ============================================================================
    // VALIDATOR REGISTRATION CHECK
    // ============================================================================
    // Validator accounts are NO LONGER created directly at startup.
    // Instead, validators register through consensus via a RegisterValidator
    // transaction (opcode 26). This ensures ALL nodes see identical state.
    //
    // Flow for joining validators:
    //   1. Start, sync chain from peers
    //   2. After sync, auto-submit RegisterValidator transaction
    //   3. Current block producer includes it in a block
    //   4. All nodes process identically: treasury debited, account created, staked
    //
    // Genesis validators are created by the genesis tool — no registration needed.
    let is_genesis_validator = genesis_config
        .initial_validators
        .iter()
        .any(|validator| validator.pubkey == validator_pubkey.to_base58());
    let mut needs_on_chain_registration = {
        let validator_account = state.get_account(&validator_pubkey).unwrap_or_else(|e| {
            eprintln!("Failed to read validator account: {e}");
            None
        });
        if is_genesis_validator {
            match validator_account {
                Some(account) if account.staked >= BOOTSTRAP_GRANT_AMOUNT => {
                    info!(
                        "✓ Genesis validator account exists: {} LICN",
                        account.balance_licn()
                    );
                }
                Some(account) => {
                    warn!(
                        "⚠️  Genesis validator account stake is below expected bootstrap amount ({:.2} LICN)",
                        account.staked as f64 / 1_000_000_000.0,
                    );
                    warn!(
                        "   Skipping RegisterValidator auto-submit because genesis validators must come from genesis state"
                    );
                }
                None => {
                    warn!(
                        "⚠️  Genesis validator account missing from local state; skipping RegisterValidator auto-submit because genesis validators must come from genesis state"
                    );
                }
            }
            false
        } else {
            match validator_account {
                Some(account) if account.staked >= BOOTSTRAP_GRANT_AMOUNT => {
                    info!(
                        "✓ Validator account exists: {} LICN",
                        account.balance_licn()
                    );
                    info!(
                        "   Spendable: {:.2} | Staked: {:.2} | Locked: {:.2}",
                        account.spendable as f64 / 1_000_000_000.0,
                        account.staked as f64 / 1_000_000_000.0,
                        account.locked as f64 / 1_000_000_000.0
                    );
                    false
                }
                Some(account) => {
                    info!(
                        "⚠️  Validator account exists but insufficient stake ({:.2} LICN < {} LICN required)",
                        account.staked as f64 / 1_000_000_000.0,
                        BOOTSTRAP_GRANT_AMOUNT / 1_000_000_000
                    );
                    info!("   Will auto-submit RegisterValidator transaction after sync");
                    true
                }
                None => {
                    info!(
                        "📋 No validator account found — will auto-submit RegisterValidator transaction after sync"
                    );
                    true
                }
            }
        }
    };

    // Initialize vote aggregator for BFT consensus
    let vote_aggregator = Arc::new(RwLock::new(VoteAggregator::new()));
    info!("🗳️  BFT voting system initialized");

    // Initialize finality tracker — lock-free commitment level tracking
    let initial_confirmed = state.get_last_confirmed_slot().unwrap_or(0);
    let initial_finalized = state.get_last_finalized_slot().unwrap_or(0);
    let finality_tracker = FinalityTracker::new(initial_confirmed, initial_finalized);
    info!(
        "🔒 Finality tracker initialized (confirmed={}, finalized={})",
        finality_tracker.confirmed_slot(),
        finality_tracker.finalized_slot()
    );

    // AUDIT-FIX M7: Load slashing tracker from disk for restart-proof evidence
    let slashing_tracker = Arc::new(Mutex::new(state.get_slashing_tracker()));
    {
        let tracker = slashing_tracker.lock().await;
        let evidence_count: usize = tracker.evidence_count();
        if evidence_count > 0 {
            info!(
                "⚔️  Slashing system initialized — loaded {} evidence records from disk",
                evidence_count
            );
        } else {
            info!("⚔️  Slashing system initialized (clean)");
        }
    }

    // Initialize stake pool for economic security
    let stake_pool = Arc::new(RwLock::new(
        state.get_stake_pool().unwrap_or_else(|_| StakePool::new()),
    ));
    info!("💰 Stake pool initialized");

    // Periodically persist stake pool to disk
    let stake_pool_for_save = stake_pool.clone();
    let state_for_stake_save = state.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let pool = stake_pool_for_save.read().await;
            if let Err(e) = state_for_stake_save.put_stake_pool(&pool) {
                warn!("⚠️  Failed to persist stake pool: {}", e);
            }
        }
    });

    // Stake tokens for this validator (100,000 LICN minimum)
    // Uses get_stake() to avoid accumulating on every restart
    {
        let mut pool = stake_pool.write().await;
        let current_slot = state.get_last_slot().unwrap_or(0);
        let existing = pool
            .get_stake(&validator_pubkey)
            .map(|s| s.amount)
            .unwrap_or(0);
        if existing >= BOOTSTRAP_GRANT_AMOUNT {
            info!("✅ Already staked: {} LICN", existing / 1_000_000_000);

            // Ensure fingerprint is registered (may be missing from pre-graduation validators)
            if machine_fingerprint != [0u8; 32] {
                match pool.register_fingerprint(&validator_pubkey, machine_fingerprint) {
                    Ok(()) => info!("🔒 Machine fingerprint registered"),
                    Err(e) => {
                        // Check if this is a migration (known key, new machine)
                        let existing_fp = pool
                            .get_stake(&validator_pubkey)
                            .map(|s| s.machine_fingerprint)
                            .unwrap_or([0u8; 32]);
                        if existing_fp != [0u8; 32] && existing_fp != machine_fingerprint {
                            info!("🔄 Machine migration detected — updating fingerprint");
                            match pool.migrate_fingerprint(
                                &validator_pubkey,
                                machine_fingerprint,
                                current_slot,
                            ) {
                                Ok(()) => info!("✅ Fingerprint migrated successfully"),
                                Err(e) => warn!("⚠️  Fingerprint migration failed: {}", e),
                            }
                        } else {
                            warn!("⚠️  Fingerprint registration failed: {}", e);
                        }
                    }
                }
            }
        } else if needs_on_chain_registration {
            // ── GENESIS BOOTSTRAP ──
            // On a fresh chain (only genesis block exists, stake pool empty), the
            // first validator must self-register directly — there's no block producer
            // to include a RegisterValidator tx yet.  This is exactly how Ethereum
            // and Cosmos handle genesis: the genesis state includes the initial
            // validator set.  Here we do it lazily on first start.
            //
            // Safety: slot 0, 0 existing validators ⇒ no consensus to disagree with.
            // Guard: NEVER self-register on a joining node — joining nodes must
            // go through the consensus RegisterValidator path after sync.
            //
            // Belt-and-suspenders: explicit_seed_peers.is_empty() ensures that
            // a node started with --bootstrap-peers can NEVER trigger genesis
            // bootstrap, even if a supervisor restart sets is_joining_network=false
            // (because has_genesis_block=true after syncing genesis from the network).
            let genesis_bootstrap = {
                let last = state.get_last_slot().unwrap_or(0);
                last == 0
                    && pool.bootstrap_grants_issued() == 0
                    && !is_joining_network
                    && explicit_seed_peers.is_empty()
            };

            if genesis_bootstrap {
                info!("🌱 GENESIS BOOTSTRAP: Fresh chain with 0 validators — self-registering as founding validator");
                let treasury_pubkey = state.get_treasury_pubkey().ok().flatten();
                let grant = BOOTSTRAP_GRANT_AMOUNT;
                let mut funded = false;
                if let Some(tpk) = treasury_pubkey {
                    if let Ok(Some(mut treasury_acct)) = state.get_account(&tpk) {
                        if treasury_acct.deduct_spendable(grant).is_ok() {
                            let _ = state.put_account(&tpk, &treasury_acct);
                            // Create/update validator account
                            let mut acct = state
                                .get_account(&validator_pubkey)
                                .unwrap_or(None)
                                .unwrap_or_else(|| Account {
                                    spores: 0,
                                    spendable: 0,
                                    staked: 0,
                                    locked: 0,
                                    data: Vec::new(),
                                    owner: Pubkey([0x01; 32]),
                                    executable: false,
                                    rent_epoch: 0,
                                    dormant: false,
                                    missed_rent_epochs: 0,
                                });
                            acct.spores = acct.spores.saturating_add(grant);
                            acct.staked = acct.staked.saturating_add(grant);
                            let _ = state.put_account(&validator_pubkey, &acct);
                            // Add to stake pool
                            // Use [0u8; 32] fingerprint (not machine_fingerprint) so that
                            // joining nodes can replicate the exact same StakeInfo bytes
                            // without knowing the genesis machine's fingerprint.
                            match pool.try_bootstrap_with_fingerprint(
                                validator_pubkey,
                                grant,
                                current_slot,
                                [0u8; 32],
                            ) {
                                Ok((idx, _)) => {
                                    info!("  ✅ Founding validator registered: {} LICN staked (bootstrap #{})",
                                        grant / 1_000_000_000, idx);
                                    funded = true;
                                    needs_on_chain_registration = false;
                                }
                                Err(e) => warn!("  ⚠️  Stake pool bootstrap failed: {}", e),
                            }
                            if funded {
                                if let Err(e) = state.put_stake_pool(&pool) {
                                    warn!("  ⚠️  Failed to persist stake pool: {}", e);
                                }
                            }
                        } else {
                            warn!("  ⚠️  Treasury has insufficient funds for genesis bootstrap");
                        }
                    }
                }
                if !funded {
                    warn!("🌱 Genesis bootstrap failed — chain will stall until manually resolved");
                }
            } else {
                // ── CONSENSUS-ONLY PATH ──
                // Do NOT add to in-memory stake pool here. The RegisterValidator
                // transaction (opcode 26) will be submitted through consensus.
                // When confirmed, apply_block_effects reloads the stake pool from
                // RocksDB, giving all nodes identical state simultaneously.
                // Until then, this validator has 0 stake: it syncs blocks but
                // does not vote or produce (exactly like Ethereum/Solana).
                info!(
                    "📋 Validator has no on-chain stake — waiting for RegisterValidator tx through consensus"
                );
                info!("   Will begin voting/producing after tx confirmed in a block");
            }
        } else {
            // Edge case: validator has on-chain account but the in-memory
            // stake pool lost its entry (e.g., pool corruption/reset).
            // Re-add from the verified on-chain state.
            let on_chain_stake = state
                .get_account(&validator_pubkey)
                .unwrap_or(None)
                .map(|a| a.staked)
                .unwrap_or(0);
            if on_chain_stake >= min_validator_stake {
                match pool.try_bootstrap_with_fingerprint(
                    validator_pubkey,
                    on_chain_stake,
                    current_slot,
                    machine_fingerprint,
                ) {
                    Ok(_) => {
                        info!(
                            "🔄 Restored stake pool entry from on-chain state: {} LICN",
                            on_chain_stake / 1_000_000_000
                        );
                    }
                    Err(e) => {
                        warn!("⚠️  Failed to restore stake pool entry: {}", e);
                    }
                }
            }
        }

        // Migrate legacy validators that were staked before bootstrap system existed
        let migrated = pool.migrate_legacy_bootstrap_indices();
        if migrated > 0 {
            info!(
                "🔄 Migrated {} validator(s) to bootstrap debt system",
                migrated
            );
            if let Err(e) = state.put_stake_pool(&pool) {
                warn!("⚠️  Failed to persist bootstrap migration: {}", e);
            }
        }
    };

    // Get starting slot (resume from last + 1)
    let last_slot = state.get_last_slot().unwrap_or(0);
    let slot = if last_slot == 0 { 1 } else { last_slot + 1 };
    info!("Starting from slot {}", slot);

    // Parent hash — set properly when BFT starts each height
    #[allow(unused_assignments)]
    let mut parent_hash = Hash::default();

    let needs_genesis = is_joining_network; // Track if we need to request genesis

    // Create channels for P2P communication
    // M11: Bounded channels prevent memory exhaustion from slow consumers.
    // Capacity tiers: high-throughput (txs/votes) → larger, control msgs → smaller.
    // Block channel sized at 2000 to absorb sync bursts without backpressure
    // killing the P2P message loop (the old 500 was too small during catch-up).
    let (block_tx, mut block_rx) = mpsc::channel(10_000);
    let block_tx_for_compact = block_tx.clone(); // P3-3: sender for reconstructed compact blocks
    let block_tx_for_erasure = block_tx.clone(); // P3-4: sender for erasure-reconstructed blocks
                                                 // Dedicated sync channel: BlockRangeResponse / BlockResponse blocks arrive here
                                                 // so they are never starved by live BFT compact blocks during catch-up.
    let (sync_block_tx, mut sync_block_rx) = mpsc::channel::<Block>(10_000);
    let (vote_tx, mut vote_rx) = mpsc::channel(2_000);
    let (transaction_tx, mut transaction_rx) = mpsc::channel(5_000);
    let (validator_announce_tx, mut validator_announce_rx) = mpsc::channel(100);
    let (block_range_request_tx, mut block_range_request_rx) = mpsc::channel(200);
    let (status_request_tx, mut status_request_rx) = mpsc::channel::<StatusRequestMsg>(100);
    let (status_response_tx, mut status_response_rx) = mpsc::channel::<StatusResponseMsg>(100);
    let (consistency_report_tx, mut consistency_report_rx) =
        mpsc::channel::<ConsistencyReportMsg>(50);
    let (snapshot_request_tx, mut snapshot_request_rx) = mpsc::channel::<SnapshotRequestMsg>(50);
    let (snapshot_response_tx, mut snapshot_response_rx) =
        mpsc::channel::<SnapshotResponseMsg>(500);
    let (slashing_evidence_tx, mut slashing_evidence_rx) =
        mpsc::channel::<lichen_core::SlashingEvidence>(100);
    let (compact_block_tx, mut compact_block_rx) =
        mpsc::channel::<lichen_p2p::CompactBlockMsg>(1_000);
    let (get_block_txs_tx, mut get_block_txs_rx) = mpsc::channel::<lichen_p2p::GetBlockTxsMsg>(200);
    let (erasure_shard_request_tx, mut erasure_shard_request_rx) =
        mpsc::channel::<lichen_p2p::ErasureShardRequestMsg>(200);
    let (erasure_shard_response_tx, mut erasure_shard_response_rx) =
        mpsc::channel::<lichen_p2p::ErasureShardResponseMsg>(200);

    // BFT consensus channels (Tendermint-style propose/prevote/precommit)
    // Sized for burst tolerance during sync catch-up with 3+ validators
    let (proposal_tx, mut proposal_rx) = mpsc::channel::<Proposal>(2_000);
    let (prevote_tx, mut prevote_rx) = mpsc::channel::<Prevote>(5_000);
    let (precommit_tx, mut precommit_rx) = mpsc::channel::<Precommit>(5_000);

    // Create mempool
    let mempool = Arc::new(Mutex::new(Mempool::new(50_000, 300))); // 50K tx max, 300s expiration — handles 5000 concurrent trader bursts

    // Start P2P network - need to extract peer manager before starting
    let (p2p_peer_manager, _p2p_handle) = match P2PNetwork::new(
        p2p_config.clone(),
        block_tx,
        sync_block_tx,
        vote_tx,
        transaction_tx,
        validator_announce_tx,
        block_range_request_tx,
        status_request_tx,
        status_response_tx,
        consistency_report_tx,
        snapshot_request_tx,
        snapshot_response_tx,
        slashing_evidence_tx,
        compact_block_tx,
        get_block_txs_tx,
        erasure_shard_request_tx,
        erasure_shard_response_tx,
        proposal_tx,
        prevote_tx,
        precommit_tx,
    )
    .await
    {
        Ok(network) => {
            info!("✅ P2P network initialized on port {}", p2p_port);

            // Get peer manager reference before network moves into spawn
            let peer_manager = network.peer_manager.clone();

            // Start accepting incoming connections
            peer_manager.start_accepting().await;
            info!("🔌 P2P: Started accepting incoming connections");

            // Start network message processing (consumes network)
            let handle = tokio::spawn(async move {
                network.start().await;
            });

            (Some(peer_manager), Some(handle))
        }
        Err(e) => {
            warn!("⚠️  P2P network failed to start: {}", e);
            warn!("⚠️  Running in single-validator mode");
            (None, None)
        }
    };

    // Create sync manager
    let sync_manager = Arc::new(SyncManager::new());
    // Genesis validators start in LiveSync — they ARE the network, no catch-up.
    // Restarted nodes that already progressed past genesis (tip > 0) also go to
    // LiveSync — they'll quickly re-sync the few blocks they missed.
    // Joining nodes AND restarted nodes stuck at tip=0 stay in InitialSync to
    // ensure fast catch-up behaviour (500ms sync cooldown vs 2s in LiveSync).
    let current_tip = state.get_last_slot().unwrap_or(0);
    if !is_joining_network && current_tip > 0 {
        // Use block_on-safe approach: spawn a task that transitions immediately
        let sm_init = sync_manager.clone();
        tokio::spawn(async move {
            sm_init.transition_to_live().await;
        });
    }
    let snapshot_sync = Arc::new(Mutex::new(SnapshotSync::new(is_joining_network)));

    // FIX-FORK-1: Shared set of slots where we received a valid block from the
    // network.  The block-receiver task inserts here; the production loop checks
    // before creating its own block, closing the TOCTOU race between the early
    // `get_block_by_slot` guard and the actual `Block::new` call.
    let received_network_slots: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
    let received_network_slots_for_blocks = received_network_slots.clone();

    // Track last block time for leader timeout handling
    let last_block_time = Arc::new(Mutex::new(std::time::Instant::now()));
    let last_block_time_for_blocks = last_block_time.clone();
    let last_block_time_for_local = last_block_time.clone();
    let global_last_user_tx_activity = Arc::new(Mutex::new(std::time::Instant::now()));
    let global_last_user_tx_activity_for_blocks = global_last_user_tx_activity.clone();

    // PERF-OPT 1: Tip-advance notification.  The block receiver task signals
    // this Notify whenever a new block advances the chain tip.  The production
    // loop waits on it instead of busy-polling every 5ms, cutting latency from
    // avg 2.5ms to ~0ms when a new block arrives.
    let tip_notify = Arc::new(tokio::sync::Notify::new());
    let tip_notify_for_blocks = tip_notify.clone();
    let tip_notify_for_producer = tip_notify.clone();

    let slot_duration_ms = genesis_config.consensus.slot_duration_ms.max(1);

    // AUDIT-FIX A2-01: Derive genesis_time as Unix seconds for deterministic
    // block timestamp derivation: timestamp = genesis_time + slot * slot_duration / 1000.
    // Read from the stored genesis block (slot 0) which has the authoritative timestamp.
    let genesis_time_secs: u64 = match state.get_block_by_slot(0) {
        Ok(Some(genesis_block)) => genesis_block.header.timestamp,
        _ => {
            // Fallback: parse from genesis config (RFC 3339 string)
            // Manual RFC 3339 parsing to avoid adding chrono dependency.
            // Format: "2025-02-20T12:00:00Z" or "2025-02-20T12:00:00+00:00"
            let gt = &genesis_config.genesis_time;
            if gt.len() >= 19 {
                // Try to parse YYYY-MM-DDTHH:MM:SS (ignore timezone, assume UTC)
                let parts: Vec<&str> = gt.split('T').collect();
                if parts.len() == 2 {
                    let date_parts: Vec<u64> =
                        parts[0].split('-').filter_map(|s| s.parse().ok()).collect();
                    let time_str = parts[1]
                        .trim_end_matches('Z')
                        .split('+')
                        .next()
                        .unwrap_or("");
                    let time_parts: Vec<u64> =
                        time_str.split(':').filter_map(|s| s.parse().ok()).collect();
                    if date_parts.len() == 3 && time_parts.len() >= 2 {
                        // Approximate Unix timestamp (good enough for bounded-window checks)
                        let year = date_parts[0];
                        let month = date_parts[1];
                        let day = date_parts[2];
                        let hour = time_parts[0];
                        let minute = time_parts[1];
                        let second = if time_parts.len() >= 3 {
                            time_parts[2]
                        } else {
                            0
                        };
                        // Days from 1970 to year (approximate, ignoring leap seconds)
                        let mut days: u64 = 0;
                        for y in 1970..year {
                            days += if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                                366
                            } else {
                                365
                            };
                        }
                        let month_days = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
                        if (month as usize) >= 1 && (month as usize) <= 12 {
                            days += month_days[(month - 1) as usize];
                            // Leap day adjustment
                            if month > 2
                                && year.is_multiple_of(4)
                                && (!year.is_multiple_of(100) || year.is_multiple_of(400))
                            {
                                days += 1;
                            }
                        }
                        days += day.saturating_sub(1);
                        days * 86400 + hour * 3600 + minute * 60 + second
                    } else {
                        warn!(
                            "⚠️  Cannot parse genesis_time '{}' — timestamps will be slot-relative",
                            gt
                        );
                        0
                    }
                } else {
                    gt.parse::<u64>().unwrap_or(0)
                }
            } else {
                gt.parse::<u64>().unwrap_or(0)
            }
        }
    };
    info!(
        "⏱  Deterministic timestamps: genesis_time={}s, slot_duration={}ms",
        genesis_time_secs, slot_duration_ms
    );

    // view_timeout is no longer used for leader election (replaced by
    // deterministic slot-based view in FIX-FORK-1), but keep the value
    // available for future watchdog/diagnostic use.
    let _view_timeout = Duration::from_millis(slot_duration_ms * 3);

    // If joining network, immediately request genesis block (slot 0)
    if needs_genesis {
        if let Some(ref pm) = p2p_peer_manager {
            info!("📡 Requesting genesis block (slot 0) from network");
            let request_msg = P2PMessage::new(
                MessageType::BlockRangeRequest {
                    start_slot: 0,
                    end_slot: 0,
                },
                p2p_config.listen_addr,
            );
            pm.broadcast(request_msg).await;
            sync_manager.mark_requested(0).await;
        }
    }

    if needs_genesis {
        if let Some(ref pm) = p2p_peer_manager {
            let state_for_genesis_retry = state.clone();
            let peer_mgr_for_genesis_retry = pm.clone();
            let local_addr_for_genesis_retry = p2p_config.listen_addr;
            tokio::spawn(async move {
                let mut interval = time::interval(Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    if let Ok(Some(_)) = state_for_genesis_retry.get_block_by_slot(0) {
                        break;
                    }

                    // Always re-request genesis block — the initial broadcast
                    // may have fired before P2P connections were established,
                    // so we must retry unconditionally until it arrives.
                    let request = P2PMessage::new(
                        MessageType::BlockRangeRequest {
                            start_slot: 0,
                            end_slot: 0,
                        },
                        local_addr_for_genesis_retry,
                    );
                    peer_mgr_for_genesis_retry.broadcast(request).await;
                }
            });
        }
    }

    if is_joining_network {
        if let Some(ref pm) = p2p_peer_manager {
            let peer_mgr_for_snapshot_retry = pm.clone();
            let local_addr_for_snapshot_retry = p2p_config.listen_addr;
            let snapshot_sync_for_retry = snapshot_sync.clone();
            tokio::spawn(async move {
                let mut interval = time::interval(Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    if snapshot_sync_for_retry.lock().await.is_ready() {
                        break;
                    }

                    let validator_request = P2PMessage::new(
                        MessageType::SnapshotRequest {
                            kind: SnapshotKind::ValidatorSet,
                        },
                        local_addr_for_snapshot_retry,
                    );
                    peer_mgr_for_snapshot_retry
                        .broadcast(validator_request)
                        .await;

                    let pool_request = P2PMessage::new(
                        MessageType::SnapshotRequest {
                            kind: SnapshotKind::StakePool,
                        },
                        local_addr_for_snapshot_retry,
                    );
                    peer_mgr_for_snapshot_retry.broadcast(pool_request).await;
                }
            });
        }
    }

    // VOTE-AUTHORITY: Single Vote Gatekeeper (like Eth2 Slashing Protection DB).
    // Replaces the old voted_slots dedup bandaid with an architectural guarantee:
    // VoteAuthority is the ONLY code path that can create a signed vote.
    // It atomically checks whether we've already voted for a slot before signing.
    // Shared between the block PRODUCER (self-vote) and the block RECEIVER
    // (received-block vote) via Arc<Mutex<>>. Prevents all DoubleVote scenarios:
    // 1. P2P echo (own block comes back through network)
    // 2. Fork re-evaluation (FIX-FORK-2 lets competing block through)
    // 3. View rotation race (producer + receiver try to vote concurrently)
    let vote_authority: Arc<tokio::sync::Mutex<VoteAuthority>> = Arc::new(tokio::sync::Mutex::new(
        VoteAuthority::new(validator_keypair.to_seed(), validator_pubkey),
    ));

    // Start incoming block handler with voting
    if let Some(ref p2p_pm) = p2p_peer_manager {
        let state_for_blocks = state.clone();
        let processor_for_blocks = processor.clone();
        let _validator_pubkey_for_blocks = validator_pubkey;
        // VOTE-AUTHORITY: Signing key is now owned by VoteAuthority — no need
        // for validator_seed in the block receiver task.
        let sync_mgr = sync_manager.clone();
        let peer_mgr_for_sync = p2p_pm.clone();
        let vote_agg_for_blocks = vote_aggregator.clone();
        let validator_set_for_blocks = validator_set.clone();
        let stake_pool_for_blocks = stake_pool.clone();
        let vote_agg_for_effects = vote_aggregator.clone();
        let local_addr = p2p_config.listen_addr;
        let last_block_time_for_blocks = last_block_time_for_blocks.clone();
        let global_last_user_tx_activity_for_blocks =
            global_last_user_tx_activity_for_blocks.clone();
        let genesis_config_for_blocks = genesis_config.clone();
        // genesis_time_secs_for_blocks and slot_duration_ms_for_blocks removed:
        // Timestamp validation now uses wall-clock only, not slot-derived timestamps.
        let slashing_for_blocks = slashing_tracker.clone();
        let validator_pubkey_for_block_slash = validator_pubkey;
        let received_slots_for_rx = received_network_slots_for_blocks.clone();
        let tip_notify_for_blocks = tip_notify_for_blocks.clone();
        let data_dir_for_blocks = data_dir.clone();
        let finality_for_blocks = finality_tracker.clone();
        let vote_authority_for_rx = vote_authority.clone();
        // PHASE-3: Clones needed for consensus-based slashing (opcode 27)
        let mempool_for_slash_blocks = mempool.clone();
        let slash_keypair_seed_for_blocks = validator_keypair.to_seed();
        let p2p_config_for_slash_blocks = p2p_config.clone();
        let p2p_pm_for_slash_blocks = p2p_pm.clone();
        tokio::spawn(async move {
            info!("🔄 Block receiver started");
            // 1.7: Track (slot, validator) → block_hash to detect double-block equivocation
            let mut seen_blocks: HashMap<(u64, [u8; 32]), Hash> = HashMap::new();
            // VOTE-AUTHORITY: The shared VoteAuthority (Arc<Mutex<VoteAuthority>>)
            // is the sole gatekeeper for vote signing. Both the block receiver and
            // block producer use it. If slot is already voted, try_vote returns None.
            // A5-02: Fork choice oracle — tracks competing chain heads by
            // cumulative stake weight. Used to break ties when multiple valid
            // blocks exist for the same slot.
            let mut fork_choice = ForkChoice::new();
            // Periodically prune old entries (keep last 1000 slots)
            let mut prune_below_slot: u64 = 0;
            // Periodic sync check: every 5 seconds, check if we're behind peers
            // and trigger sync even when no blocks are arriving. This prevents a
            // stalled chain from permanently blocking catch-up (Tendermint-style
            // "blockchain reactor" pattern — periodic peer polling).
            let mut sync_check_interval = time::interval(Duration::from_secs(5));
            sync_check_interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
            // Priority select: drain sync-response blocks (BlockRangeResponse /
            // BlockResponse) before live blocks so catch-up is never starved.
            loop {
                let block = tokio::select! {
                    biased;
                    Some(b) = sync_block_rx.recv() => b,
                    Some(b) = block_rx.recv() => b,
                    // Periodic sync check: fires every 5s to trigger catch-up
                    // when the chain is stalled and no blocks are arriving.
                    _ = sync_check_interval.tick() => {
                        let current_slot = state_for_blocks.get_last_slot().unwrap_or(0);
                        if let Some((start, end)) = sync_mgr.should_sync(current_slot).await {
                            let gap = end.saturating_sub(current_slot);
                            if gap > sync::WARP_SYNC_THRESHOLD {
                                sync_mgr.set_sync_mode(sync::SyncMode::Warp).await;
                            } else if gap > sync::HEADER_SYNC_FULL_EXECUTION_WINDOW * 2 {
                                sync_mgr.set_sync_mode(sync::SyncMode::HeaderOnly).await;
                            } else {
                                sync_mgr.set_sync_mode(sync::SyncMode::Full).await;
                            }
                            info!("🔄 Periodic sync check: behind by {} blocks ({} to {})", gap, start, end);
                            sync_mgr.start_sync(start, end).await;
                            let mut peer_infos = peer_mgr_for_sync.get_peer_infos();
                            peer_infos.sort_by(|a, b| {
                                b.1.cmp(&a.1)
                                    .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
                            });
                            let all_peers: Vec<std::net::SocketAddr> = peer_infos
                                .into_iter()
                                .take(SYNC_REQUEST_FANOUT.max(1))
                                .map(|(addr, _)| addr)
                                .collect();
                            let mut chunk_start = start;
                            let mut chunk_idx: usize = 0;
                            while chunk_start <= end {
                                let chunk_end = std::cmp::min(
                                    chunk_start + sync::P2P_BLOCK_RANGE_LIMIT - 1,
                                    end,
                                );
                                if all_peers.is_empty() {
                                    let request_msg = P2PMessage::new(
                                        MessageType::BlockRangeRequest {
                                            start_slot: chunk_start,
                                            end_slot: chunk_end,
                                        },
                                        local_addr,
                                    );
                                    peer_mgr_for_sync.broadcast(request_msg).await;
                                } else {
                                    let peer_addr = &all_peers[chunk_idx % all_peers.len()];
                                    let request_msg = P2PMessage::new(
                                        MessageType::BlockRangeRequest {
                                            start_slot: chunk_start,
                                            end_slot: chunk_end,
                                        },
                                        local_addr,
                                    );
                                    let _ = peer_mgr_for_sync
                                        .send_to_peer(peer_addr, request_msg)
                                        .await;
                                }
                                chunk_start = chunk_end + 1;
                                chunk_idx += 1;
                            }
                            for slot in start..=end {
                                sync_mgr.mark_requested(slot).await;
                            }

                            // Spawn completion handler (same pattern as main sync path).
                            // Without this, is_syncing stays true forever and blocks
                            // all future sync attempts.
                            let sync_mgr_complete = sync_mgr.clone();
                            let state_for_sync_check = state_for_blocks.clone();
                            let sync_start_slot = current_slot;
                            let sync_end = end;
                            tokio::spawn(async move {
                                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                                let progress = sync_mgr_complete.get_last_progress_slot().await;
                                if progress > sync_start_slot {
                                    sync_mgr_complete.record_sync_success().await;
                                    let current = state_for_sync_check.get_last_slot().unwrap_or(0);
                                    if current < sync_end {
                                        info!(
                                            "🔄 Periodic sync progress: {} → {} (target {})",
                                            sync_start_slot, current, sync_end
                                        );
                                    }
                                } else {
                                    sync_mgr_complete.record_sync_failure().await;
                                }
                                sync_mgr_complete.complete_sync().await;
                            });
                        }
                        continue;
                    },
                    else => break,
                };
                let block_slot = block.header.slot;
                let block_has_user_transactions = !block.transactions.is_empty();

                // ── Block validation (T2.2) ──────────────────────────
                // Verify producer signature and structural limits BEFORE
                // accepting any block into local state.
                if !block.verify_signature() {
                    warn!(
                        "⚠️  Rejecting block {} — invalid signature from {}",
                        block_slot,
                        Pubkey(block.header.validator).to_base58()
                    );
                    continue;
                }
                if let Err(e) = block.validate_structure() {
                    warn!("⚠️  Rejecting block {} — {}", block_slot, e);
                    continue;
                }

                // Timestamp validation: reject blocks with timestamps
                // more than 120s IN THE FUTURE.  Past blocks are accepted
                // because late-joining validators need to sync historical
                // blocks whose wall-clock time has long passed.
                if block_slot > 0 {
                    let now_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    if block.header.timestamp > now_secs + 120 {
                        warn!(
                            "⚠️  Rejecting block {} — timestamp {} is {}s in the future (wall-clock {})",
                            block_slot, block.header.timestamp,
                            block.header.timestamp - now_secs, now_secs
                        );
                        continue;
                    }
                }

                // AUDIT-FIX C5: Reject blocks from non-member validators BEFORE
                // note_seen / fork-choice to prevent outsiders from influencing
                // sync target or fork selection.
                // Genesis block (slot 0) uses SYSTEM_ACCOUNT_OWNER as validator,
                // which is not in the active set — allow it through.
                //
                // During header-first sync, skip this check for blocks outside
                // the full-execution window.  RegisterValidator TXs are not
                // replayed for those blocks, so the in-memory validator set
                // only contains the genesis producer.  The parent-hash chain
                // and signature verification still protect against invalid
                // blocks; the active-set gate is enforced once the node enters
                // the full-execution window where TXs are replayed.
                if block_slot > 0 && sync_mgr.should_full_validate(block_slot).await {
                    let vs = validator_set_for_blocks.read().await;
                    if vs.get_validator(&Pubkey(block.header.validator)).is_none() {
                        warn!(
                            "⚠️  Rejecting block {} — validator {} not in active set",
                            block_slot,
                            Pubkey(block.header.validator).to_base58()
                        );
                        continue;
                    }

                    // G-3 fix: Verify commit certificate for blocks received
                    // during sync or P2P. Non-genesis blocks must have valid
                    // commit signatures from 2/3+ of stake. This prevents
                    // accepting forged blocks that were never actually committed
                    // by the BFT quorum. Skip for blocks in header-only sync
                    // window since those are verified by parent-hash chain.
                    let pool = stake_pool_for_blocks.read().await;
                    if let Err(err) = verify_committed_block_authenticity(&block, &vs, &pool) {
                        warn!("⚠️  Rejecting block {} — {}", block_slot, err);
                        continue;
                    }

                    // Cross-reference validators_hash: committed blocks that
                    // advertise a validator-set commitment must match our local
                    // active-set view or they are rejected as unauthenticated
                    // state-divergence candidates.
                    if let Err(err) =
                        verify_block_validators_hash(&block, &vs, &pool, min_validator_stake)
                    {
                        warn!("⚠️  Rejecting block {} — {}", block_slot, err);
                        continue;
                    }
                }

                // 1.7: Double-block equivocation detection
                {
                    let key = (block_slot, block.header.validator);
                    let block_hash = block.hash();
                    if let Some(prev_hash) = seen_blocks.get(&key) {
                        if *prev_hash != block_hash {
                            error!(
                                "🚨 CRITICAL: Double-block equivocation detected! Validator {} produced two different blocks for slot {} (hash1={}, hash2={})",
                                Pubkey(block.header.validator).to_base58(),
                                block_slot,
                                prev_hash.to_hex(),
                                block_hash.to_hex(),
                            );

                            // Create slashing evidence and submit to tracker
                            let evidence = SlashingEvidence::new(
                                SlashingOffense::DoubleBlock {
                                    slot: block_slot,
                                    block_hash_1: *prev_hash,
                                    block_hash_2: block_hash,
                                },
                                Pubkey(block.header.validator),
                                block_slot,
                                validator_pubkey_for_block_slash,
                                block.header.timestamp,
                            );

                            let mut slasher = slashing_for_blocks.lock().await;
                            if slasher.add_evidence(evidence.clone()) {
                                info!(
                                    "⚔️  DoubleBlock slashing evidence recorded for {}",
                                    Pubkey(block.header.validator).to_base58()
                                );
                                // Broadcast evidence to network
                                let evidence_msg = P2PMessage::new(
                                    MessageType::SlashingEvidence(evidence.clone()),
                                    local_addr,
                                );
                                peer_mgr_for_sync.broadcast(evidence_msg).await;

                                // PHASE-3: Submit SlashValidator tx through consensus
                                // (opcode 27) so all nodes apply the same penalty
                                if let Ok(evidence_bytes) = bincode::serialize(&evidence) {
                                    let mut ix_data = vec![27u8];
                                    ix_data.extend_from_slice(&evidence_bytes);
                                    let tip = state_for_blocks.get_last_slot().unwrap_or(0);
                                    if let Ok(Some(tip_block)) =
                                        state_for_blocks.get_block_by_slot(tip)
                                    {
                                        let ix = lichen_core::Instruction {
                                            program_id: lichen_core::processor::SYSTEM_PROGRAM_ID,
                                            accounts: vec![Pubkey(block.header.validator)],
                                            data: ix_data,
                                        };
                                        let msg =
                                            lichen_core::Message::new(vec![ix], tip_block.hash());
                                        let mut tx = Transaction::new(msg);
                                        let kp = Keypair::from_seed(&slash_keypair_seed_for_blocks);
                                        let sig = kp.sign(&tx.message.serialize());
                                        tx.signatures.push(sig);
                                        {
                                            let mut pool = mempool_for_slash_blocks.lock().await;
                                            if let Err(e) = pool.add_transaction(tx.clone(), 0, 0) {
                                                warn!("⚠️  Failed to add SlashValidator tx to mempool: {}", e);
                                            }
                                        }
                                        let target_id = tx.hash().0;
                                        let slash_msg = lichen_p2p::P2PMessage::new(
                                            lichen_p2p::MessageType::Transaction(tx),
                                            p2p_config_for_slash_blocks.listen_addr,
                                        );
                                        p2p_pm_for_slash_blocks
                                            .route_to_closest(
                                                &target_id,
                                                lichen_p2p::NON_CONSENSUS_FANOUT,
                                                slash_msg,
                                            )
                                            .await;
                                        info!(
                                            "📝 Submitted SlashValidator tx for DoubleBlock by {}",
                                            Pubkey(block.header.validator).to_base58()
                                        );
                                    }
                                }
                            }
                            drop(slasher);

                            // Reject the conflicting block
                            continue;
                        } else {
                            // FIX-FORK-2: Allow re-delivery for fork resolution.
                            // When a duplicate block arrives at or below our tip AND
                            // there are pending blocks from a longer fork that can't
                            // chain, let it through to fork choice. The previous
                            // attempt may have rejected it because we_are_behind was
                            // false at that time, but now with pending blocks queued
                            // the fork choice has better information.
                            let current = state_for_blocks.get_last_slot().unwrap_or(0);
                            let has_pending = sync_mgr.pending_count().await > 0;
                            if block_slot <= current && has_pending {
                                // Let through to fork choice for re-evaluation
                                info!(
                                    "🔄 Re-evaluating fork block {} (pending blocks exist)",
                                    block_slot
                                );
                            } else {
                                // Truly duplicate, skip
                                continue;
                            }
                        }
                    }
                    seen_blocks.insert(key, block_hash);

                    // Prune entries older than 1000 slots to bound memory
                    if block_slot > prune_below_slot + 2000 {
                        prune_below_slot = block_slot.saturating_sub(1000);
                        seen_blocks.retain(|&(slot, _), _| slot >= prune_below_slot);
                        // VOTE-AUTHORITY: Prune VoteAuthority's voted map alongside seen_blocks
                        vote_authority_for_rx.lock().await.prune(prune_below_slot);
                    }
                }

                sync_mgr.note_seen(block_slot).await;

                // STABILITY-FIX: Update last_block_time on block RECEIPT, not
                // just on successful apply. A node that is receiving blocks from
                // the network is alive — it's behind on sync, not deadlocked.
                // Without this, the watchdog kills nodes that are actively
                // receiving and queuing blocks but can't apply them yet because
                // intermediate blocks are still missing.
                *last_block_time_for_blocks.lock().await = std::time::Instant::now();
                if block_has_user_transactions {
                    *global_last_user_tx_activity_for_blocks.lock().await =
                        std::time::Instant::now();
                }

                // FIX-FORK-1: Record that this slot has a valid network block
                // at RECEIPT time. This prevents the production loop from
                // creating a conflicting block for a slot we've already seen
                // from the network. The entry is pruned after 200 slots.
                {
                    let mut rns = received_slots_for_rx.lock().await;
                    rns.insert(block_slot);
                    if block_slot > 200 {
                        rns.retain(|&s| s + 200 >= block_slot);
                    }
                }
                let current_slot = state_for_blocks.get_last_slot().unwrap_or(0);

                // Diagnostic: trace every block entering the receiver
                info!(
                    "📬 Block receiver: processing slot {} (tip={}, validator={})",
                    block_slot,
                    current_slot,
                    Pubkey(block.header.validator).to_base58()
                );

                // Handle genesis block specially (slot 0 when current is also 0)
                if block_slot == 0 && current_slot == 0 {
                    // M3 fix: Prevent overwriting an existing genesis block
                    if state_for_blocks
                        .get_block_by_slot(0)
                        .ok()
                        .flatten()
                        .is_some()
                    {
                        warn!("⚠️  Ignoring duplicate genesis block from network");
                        continue;
                    }
                    // Genesis block - store it and initialize full genesis state
                    if state_for_blocks.put_block(&block).is_ok() {
                        state_for_blocks.set_last_slot(0).ok();
                        *last_block_time_for_blocks.lock().await = std::time::Instant::now();

                        // ── C3 fix: Initialize genesis state from network block ──
                        // The local genesis path writes state directly; a joining
                        // validator must derive the same state from the genesis block
                        // transactions + genesis config.

                        // 1. Rent params from genesis config
                        state_for_blocks
                            .set_rent_params(
                                genesis_config_for_blocks
                                    .features
                                    .rent_rate_spores_per_kb_month,
                                genesis_config_for_blocks.features.rent_free_kb,
                            )
                            .ok();

                        // 2. Fee config from genesis config
                        let gc = &genesis_config_for_blocks;
                        let genesis_fee_config = FeeConfig {
                            base_fee: gc.features.base_fee_spores,
                            contract_deploy_fee: CONTRACT_DEPLOY_FEE,
                            contract_upgrade_fee: CONTRACT_UPGRADE_FEE,
                            nft_mint_fee: NFT_MINT_FEE,
                            nft_collection_fee: NFT_COLLECTION_FEE,
                            fee_burn_percent: gc.features.fee_burn_percentage,
                            fee_producer_percent: gc.features.fee_producer_percentage,
                            fee_voters_percent: gc.features.fee_voters_percentage,
                            fee_community_percent: gc.features.fee_community_percentage,
                            fee_treasury_percent: 100u64
                                .saturating_sub(gc.features.fee_burn_percentage)
                                .saturating_sub(gc.features.fee_producer_percentage)
                                .saturating_sub(gc.features.fee_voters_percentage)
                                .saturating_sub(gc.features.fee_community_percentage),
                        };
                        state_for_blocks
                            .set_fee_config_full(&genesis_fee_config)
                            .ok();

                        // 3. Extract genesis pubkey from mint tx
                        //    tx[0]: Mint — accounts = [GENESIS_MINT_PUBKEY, genesis_pubkey]
                        //    tx[1..]: Distribution transfers — accounts = [genesis_pubkey, recipient]
                        //    tx[1] is always the treasury (validator_rewards) for backward compat
                        let extracted_genesis_pubkey = block
                            .transactions
                            .first()
                            .and_then(|tx| tx.message.instructions.first())
                            .and_then(|ix| ix.accounts.get(1))
                            .copied();

                        if let Some(gpk) = extracted_genesis_pubkey {
                            // 4. Process all distribution transfers from genesis block
                            let total_supply_licn = 500_000_000u64;
                            let total_spores = Account::licn_to_spores(total_supply_licn);
                            let mut total_distributed_spores = 0u64;

                            for (i, tx) in block.transactions.iter().enumerate().skip(1) {
                                if let Some(ix) = tx.message.instructions.first() {
                                    if ix.data.first() == Some(&4) && ix.accounts.len() >= 2 {
                                        let recipient = ix.accounts[1];
                                        let amount_spores = if ix.data.len() >= 9 {
                                            u64::from_le_bytes(
                                                ix.data[1..9].try_into().unwrap_or([0u8; 8]),
                                            )
                                        } else {
                                            0
                                        };

                                        let mut acct = Account::new(0, SYSTEM_ACCOUNT_OWNER);
                                        acct.spores = amount_spores;
                                        acct.spendable = amount_spores;
                                        state_for_blocks.put_account(&recipient, &acct).ok();
                                        total_distributed_spores += amount_spores;

                                        // tx[1] = treasury (validator_rewards) — works for both old and new genesis
                                        if i == 1 {
                                            state_for_blocks.set_treasury_pubkey(&recipient).ok();
                                            info!(
                                                "  ✓ 📡 [sync] Treasury: {} ({} LICN)",
                                                recipient.to_base58(),
                                                amount_spores / 1_000_000_000
                                            );
                                        } else {
                                            info!(
                                                "  ✓ 📡 [sync] Distribution {}: {} ({} LICN)",
                                                i,
                                                recipient.to_base58(),
                                                amount_spores / 1_000_000_000
                                            );
                                        }
                                    }
                                }
                            }

                            // 5. Reconstruct genesis account (total supply minus all distributions)
                            let mut genesis_account = Account::new(total_supply_licn, gpk);
                            genesis_account.spores =
                                total_spores.saturating_sub(total_distributed_spores);
                            genesis_account.spendable = genesis_account
                                .spores
                                .saturating_sub(genesis_account.staked)
                                .saturating_sub(genesis_account.locked);
                            state_for_blocks.put_account(&gpk, &genesis_account).ok();
                            state_for_blocks.set_genesis_pubkey(&gpk).ok();
                            info!(
                                "  ✓ 📡 [sync] Genesis account: {} ({} LICN remaining)",
                                gpk.to_base58(),
                                genesis_account.spores / 1_000_000_000
                            );

                            // 6. Create initial accounts from genesis config
                            for account_info in &genesis_config_for_blocks.initial_accounts {
                                if let Ok(pubkey) = Pubkey::from_base58(&account_info.address) {
                                    let account = Account::new(account_info.balance_licn, pubkey);
                                    state_for_blocks.put_account(&pubkey, &account).ok();
                                }
                            }

                            // 6b. Reconstruct explicit slot-0 validator registrations.
                            if let Ok(Some(treasury_pubkey)) =
                                state_for_blocks.get_treasury_pubkey()
                            {
                                if let Ok(Some(mut treasury_account)) =
                                    state_for_blocks.get_account(&treasury_pubkey)
                                {
                                    let mut pool = state_for_blocks
                                        .get_stake_pool()
                                        .unwrap_or_else(|_| StakePool::new());
                                    for tx in block.transactions.iter().skip(1) {
                                        let Some(ix) = tx.message.instructions.first() else {
                                            continue;
                                        };
                                        if ix.data.first() != Some(&26) || ix.accounts.is_empty() {
                                            continue;
                                        }

                                        let validator_pubkey = ix.accounts[0];
                                        if treasury_account
                                            .deduct_spendable(BOOTSTRAP_GRANT_AMOUNT)
                                            .is_err()
                                        {
                                            warn!(
                                                "⚠️  [sync] Treasury could not fund explicit genesis validator {}",
                                                validator_pubkey.to_base58()
                                            );
                                            continue;
                                        }

                                        let mut account = state_for_blocks
                                            .get_account(&validator_pubkey)
                                            .ok()
                                            .flatten()
                                            .unwrap_or_else(|| Account::new(0, Pubkey([0x01; 32])));
                                        account.spores =
                                            account.spores.saturating_add(BOOTSTRAP_GRANT_AMOUNT);
                                        account.staked =
                                            account.staked.saturating_add(BOOTSTRAP_GRANT_AMOUNT);
                                        account.spendable = 0;
                                        state_for_blocks
                                            .put_account(&validator_pubkey, &account)
                                            .ok();

                                        if let Err(err) = pool.try_bootstrap_with_fingerprint(
                                            validator_pubkey,
                                            BOOTSTRAP_GRANT_AMOUNT,
                                            0,
                                            [0u8; 32],
                                        ) {
                                            warn!(
                                                "⚠️  [sync] Failed to reconstruct genesis validator {}: {}",
                                                validator_pubkey.to_base58(),
                                                err
                                            );
                                        }
                                    }

                                    state_for_blocks
                                        .put_account(&treasury_pubkey, &treasury_account)
                                        .ok();
                                    state_for_blocks.put_stake_pool(&pool).ok();

                                    {
                                        let mut live_pool = stake_pool_for_blocks.write().await;
                                        *live_pool = pool.clone();
                                    }

                                    {
                                        let mut live_vs = validator_set_for_blocks.write().await;
                                        for entry in pool.stake_entries() {
                                            let resolved_stake = entry.total_stake();
                                            if resolved_stake < min_validator_stake {
                                                continue;
                                            }

                                            if let Some(existing) =
                                                live_vs.get_validator_mut(&entry.validator)
                                            {
                                                existing.stake = resolved_stake;
                                                existing.pending_activation = false;
                                            } else {
                                                live_vs.add_validator(ValidatorInfo {
                                                    pubkey: entry.validator,
                                                    stake: resolved_stake,
                                                    reputation: 100,
                                                    blocks_proposed: 0,
                                                    votes_cast: 0,
                                                    correct_votes: 0,
                                                    last_active_slot: 0,
                                                    joined_slot: 0,
                                                    commission_rate: 500,
                                                    transactions_processed: 0,
                                                    pending_activation: false,
                                                });
                                            }
                                        }

                                        if let Err(err) =
                                            state_for_blocks.save_validator_set(&live_vs)
                                        {
                                            warn!(
                                                "⚠️  [sync] Failed to persist reconstructed validator set: {}",
                                                err
                                            );
                                        }
                                    }
                                }
                            }

                            // 7. Genesis transactions already stored + indexed
                            //    by put_block() above (CF_TRANSACTIONS + CF_TX_TO_SLOT
                            //    + CF_TX_BY_SLOT in one atomic WriteBatch).

                            // 8. Auto-deploy contracts
                            genesis_auto_deploy(&state_for_blocks, &gpk, "📡 [sync]");
                            genesis_initialize_contracts(
                                &state_for_blocks,
                                &gpk,
                                "📡 [sync]",
                                block.header.timestamp,
                            );
                            genesis_create_trading_pairs(&state_for_blocks, &gpk, "📡 [sync]");
                            genesis_seed_oracle(
                                &state_for_blocks,
                                &gpk,
                                "📡 [sync]",
                                block.header.timestamp,
                            );
                            genesis_seed_margin_prices(
                                &state_for_blocks,
                                &gpk,
                                block.header.timestamp,
                            );
                            genesis_seed_analytics_prices(
                                &state_for_blocks,
                                &gpk,
                                block.header.timestamp,
                            );

                            info!("✅ 📡 [sync] Applied genesis block (slot 0) from network — full state initialized");
                        } else {
                            warn!(
                                "⚠️  Genesis block has no mint tx — cannot extract genesis pubkey"
                            );
                            info!(
                                "✅ 📡 [sync] Applied genesis block (slot 0) from network (state incomplete)"
                            );
                        }

                        // Try to apply any pending blocks now that we have genesis
                        let genesis_hash = block.hash();
                        let pending = sync_mgr.try_apply_pending(0, genesis_hash).await;
                        for pending_block in pending {
                            let pending_slot = pending_block.header.slot;
                            // P1-1: Header-first sync — skip TX replay for blocks
                            // outside the full-execution window during catch-up.
                            if sync_mgr.should_full_validate(pending_slot).await {
                                replay_block_transactions(&processor_for_blocks, &pending_block);
                            }
                            run_analytics_bridge_from_state(
                                &state_for_blocks,
                                pending_block.header.slot,
                                genesis_config_for_blocks.consensus.slot_duration_ms.max(1),
                            );
                            run_sltp_triggers_from_state(&state_for_blocks);
                            reset_24h_stats_if_expired(
                                &state_for_blocks,
                                pending_block.header.timestamp,
                            );
                            if state_for_blocks
                                .put_block_atomic(&pending_block, None, None)
                                .is_ok()
                            {
                                *last_block_time_for_blocks.lock().await =
                                    std::time::Instant::now();
                                info!("✅ Applied pending block {}", pending_slot);
                                sync_mgr.record_progress(pending_slot).await;
                                if sync_mgr.is_caught_up(pending_slot).await {
                                    sync_mgr.transition_to_live().await;
                                }
                                apply_block_effects(
                                    &state_for_blocks,
                                    &validator_set_for_blocks,
                                    &stake_pool_for_blocks,
                                    &vote_agg_for_effects,
                                    &pending_block,
                                    false,
                                    min_validator_stake,
                                )
                                .await;
                                apply_oracle_from_block(&state_for_blocks, &pending_block);
                                maybe_create_checkpoint(
                                    &state_for_blocks,
                                    pending_slot,
                                    &data_dir_for_blocks,
                                    &sync_mgr,
                                )
                                .await;
                            }
                        }
                    }

                    // SYNC-FIX: Trigger sync IMMEDIATELY after genesis processing.
                    // After genesis, the joining node must fetch blocks 1..tip
                    // from peers. should_sync relies on highest_seen_slot, which
                    // may still be 0 if no blocks were processed yet (compact
                    // blocks are queued behind genesis in the channel). Use the
                    // network tip from should_sync if available; otherwise fall
                    // back to requesting the first batch unconditionally — the
                    // should_sync check on subsequent blocks will continue from
                    // there once highest_seen_slot is updated.
                    let post_genesis_slot = state_for_blocks.get_last_slot().unwrap_or(0);
                    let sync_range = sync_mgr.should_sync(post_genesis_slot).await;

                    // Determine the range to request.
                    let (start, end) = if let Some(range) = sync_range {
                        range
                    } else {
                        // Fallback: highest_seen_slot is 0 because no blocks
                        // made it through yet. Request a bootstrap batch of
                        // the first P2P_BLOCK_RANGE_LIMIT blocks and let the
                        // normal sync loop continue from there.
                        (1, sync::P2P_BLOCK_RANGE_LIMIT)
                    };

                    {
                        let gap = end.saturating_sub(post_genesis_slot);
                        if gap > sync::WARP_SYNC_THRESHOLD {
                            sync_mgr.set_sync_mode(sync::SyncMode::Warp).await;
                        } else if gap > sync::HEADER_SYNC_FULL_EXECUTION_WINDOW * 2 {
                            sync_mgr.set_sync_mode(sync::SyncMode::HeaderOnly).await;
                        } else {
                            sync_mgr.set_sync_mode(sync::SyncMode::Full).await;
                        }
                        info!("🔄 Post-genesis sync: blocks {} to {}", start, end);
                        sync_mgr.start_sync(start, end).await;

                        let mut peer_infos = peer_mgr_for_sync.get_peer_infos();
                        peer_infos.sort_by(|a, b| {
                            b.1.cmp(&a.1)
                                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
                        });
                        let all_peers: Vec<std::net::SocketAddr> = peer_infos
                            .into_iter()
                            .take(SYNC_REQUEST_FANOUT.max(1))
                            .map(|(addr, _)| addr)
                            .collect();

                        let mut chunk_start = start;
                        let mut chunk_idx: usize = 0;
                        while chunk_start <= end {
                            let chunk_end =
                                std::cmp::min(chunk_start + sync::P2P_BLOCK_RANGE_LIMIT - 1, end);

                            if all_peers.is_empty() {
                                let request_msg = P2PMessage::new(
                                    MessageType::BlockRangeRequest {
                                        start_slot: chunk_start,
                                        end_slot: chunk_end,
                                    },
                                    local_addr,
                                );
                                peer_mgr_for_sync.broadcast(request_msg).await;
                            } else {
                                let peer_addr = &all_peers[chunk_idx % all_peers.len()];
                                let request_msg = P2PMessage::new(
                                    MessageType::BlockRangeRequest {
                                        start_slot: chunk_start,
                                        end_slot: chunk_end,
                                    },
                                    local_addr,
                                );
                                if let Err(e) =
                                    peer_mgr_for_sync.send_to_peer(peer_addr, request_msg).await
                                {
                                    warn!(
                                        "⚠️  Failed post-genesis sync request {}-{} to {}: {}",
                                        chunk_start, chunk_end, peer_addr, e
                                    );
                                }
                            }
                            info!(
                                "📡 Sent post-genesis block range request: {} to {}",
                                chunk_start, chunk_end
                            );
                            chunk_start = chunk_end + 1;
                            chunk_idx += 1;
                        }

                        // Progress-based sync completion (same as main sync path)
                        let sync_mgr_complete = sync_mgr.clone();
                        let sync_start_slot = post_genesis_slot;
                        tokio::spawn(async move {
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            let progress = sync_mgr_complete.get_last_progress_slot().await;
                            if progress > sync_start_slot {
                                sync_mgr_complete.record_sync_success().await;
                            } else {
                                sync_mgr_complete.record_sync_failure().await;
                            }
                            sync_mgr_complete.complete_sync().await;
                        });
                    }
                } else if block_slot > current_slot {
                    // Check if this block extends our chain (parent matches our latest block)
                    // With slot gaps (when some leaders can't produce), we only
                    // require the parent_hash to match the tip — NOT block_slot - 1.
                    let can_chain = state_for_blocks
                        .get_block_by_slot(current_slot)
                        .ok()
                        .flatten()
                        .map(|tip| block.header.parent_hash == tip.hash())
                        .unwrap_or(false);

                    if can_chain {
                        // SYNC-FIX: Replicate genesis bootstrap for joining nodes.
                        // The genesis validator's account + pool entry are created
                        // by direct state write on the genesis node (not through a
                        // transaction). Without replication, joining nodes have
                        // divergent state_roots from block 1 onward. When we see
                        // the first non-genesis block, identify the genesis validator
                        // (block producer) and apply the same bootstrap: debit
                        // treasury → create validator account with staked funds →
                        // add to stake pool. This must happen BEFORE replay so that
                        // state matches the genesis node exactly.
                        if block_slot > 0 && block_slot <= 5 {
                            let producer = Pubkey(block.header.validator);
                            let pool_missing = state_for_blocks
                                .get_stake_pool()
                                .map(|p| p.get_stake(&producer).is_none())
                                .unwrap_or(true);
                            if pool_missing {
                                // Replicate genesis bootstrap
                                let treasury_pk =
                                    state_for_blocks.get_treasury_pubkey().ok().flatten();
                                if let Some(tpk) = treasury_pk {
                                    if let Ok(Some(mut treasury)) =
                                        state_for_blocks.get_account(&tpk)
                                    {
                                        if treasury.deduct_spendable(BOOTSTRAP_GRANT_AMOUNT).is_ok()
                                        {
                                            state_for_blocks.put_account(&tpk, &treasury).ok();
                                            let mut acct = state_for_blocks
                                                .get_account(&producer)
                                                .unwrap_or(None)
                                                .unwrap_or_else(|| {
                                                    Account::new(0, SYSTEM_ACCOUNT_OWNER)
                                                });
                                            acct.spores =
                                                acct.spores.saturating_add(BOOTSTRAP_GRANT_AMOUNT);
                                            acct.staked =
                                                acct.staked.saturating_add(BOOTSTRAP_GRANT_AMOUNT);
                                            state_for_blocks.put_account(&producer, &acct).ok();
                                            let mut pool = state_for_blocks
                                                .get_stake_pool()
                                                .unwrap_or_else(|_| StakePool::new());
                                            // Must use try_bootstrap_with_fingerprint (not upsert_stake)
                                            // to create byte-identical StakeInfo as the genesis node:
                                            // bootstrap_index=0, bootstrap_debt=amount, status=Bootstrapping.
                                            // upsert_stake creates bootstrap_index=u64::MAX, debt=0, FullyVested.
                                            if let Err(e) = pool.try_bootstrap_with_fingerprint(
                                                producer,
                                                BOOTSTRAP_GRANT_AMOUNT,
                                                0,
                                                [0u8; 32],
                                            ) {
                                                warn!(
                                                    "⚠️  Genesis bootstrap pool entry failed: {}",
                                                    e
                                                );
                                            }
                                            state_for_blocks.put_stake_pool(&pool).ok();
                                            {
                                                let mut mem_pool =
                                                    stake_pool_for_blocks.write().await;
                                                *mem_pool = pool;
                                            }
                                            info!(
                                                "🩹 Genesis bootstrap replicated: {} staked {} LICN",
                                                producer.to_base58(),
                                                BOOTSTRAP_GRANT_AMOUNT / 1_000_000_000
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        // Valid next block in chain - replay transactions then store
                        // P1-1: Skip TX replay in header-only sync for far-away blocks.
                        if sync_mgr.should_full_validate(block_slot).await {
                            replay_block_transactions(&processor_for_blocks, &block);
                        }
                        // SYNC-FIX: Apply block effects (rewards, staking) during sync
                        // so that the joining node's CF_ACCOUNTS state matches the
                        // genesis node's. Without this, block rewards accumulate only
                        // on the genesis node, causing state_root divergence when BFT
                        // starts. The reward guard (per-slot idempotency) prevents
                        // double-application if the block also goes through CommitBlock.
                        apply_block_effects(
                            &state_for_blocks,
                            &validator_set_for_blocks,
                            &stake_pool_for_blocks,
                            &vote_agg_for_effects,
                            &block,
                            false,
                            min_validator_stake,
                        )
                        .await;
                        apply_oracle_from_block(&state_for_blocks, &block);
                        // DIAG: Compare local state_root with block's stored state_root
                        // to detect the first sync slot where state diverges.
                        {
                            let local_root = state_for_blocks.compute_state_root();
                            let block_root = block.header.state_root;
                            if local_root != block_root && block_root != Hash::default() {
                                warn!(
                                    "⚠️  SYNC STATE MISMATCH at slot {}: local={} block={}",
                                    block_slot,
                                    hex::encode(&local_root.0[..8]),
                                    hex::encode(&block_root.0[..8]),
                                );
                            }
                        }
                        run_analytics_bridge_from_state(
                            &state_for_blocks,
                            block.header.slot,
                            genesis_config_for_blocks.consensus.slot_duration_ms.max(1),
                        );
                        run_sltp_triggers_from_state(&state_for_blocks);
                        reset_24h_stats_if_expired(&state_for_blocks, block.header.timestamp);
                        if state_for_blocks
                            .put_block_atomic(&block, None, None)
                            .is_ok()
                        {
                            *last_block_time_for_blocks.lock().await = std::time::Instant::now();
                            // FIX-FORK-1: Record ONLY after successful application
                            {
                                let mut rns = received_slots_for_rx.lock().await;
                                rns.insert(block_slot);
                                if block_slot > 200 {
                                    rns.retain(|&s| s + 200 >= block_slot);
                                }
                            }
                            info!("✅ Applied block {} from network", block_slot);
                            sync_mgr.record_progress(block_slot).await;

                            // Transition to LiveSync once caught up
                            if sync_mgr.is_caught_up(block_slot).await {
                                sync_mgr.transition_to_live().await;
                            }

                            // A5-02: Record this head in fork choice oracle with the
                            // proposer's stake weight so competing forks are compared
                            // by cumulative attestation weight.
                            {
                                let pool = stake_pool_for_blocks.read().await;
                                let proposer = Pubkey(block.header.validator);
                                let weight = pool
                                    .get_stake(&proposer)
                                    .map(|s| s.total_stake())
                                    .unwrap_or(1);
                                fork_choice.add_head(block_slot, block.hash(), weight);
                            }

                            // PERF-OPT 1: Notify production loop that tip advanced
                            // BEFORE casting vote or applying effects — lets the next
                            // leader start preparing immediately.
                            tip_notify_for_blocks.notify_waiters();

                            // PERF-OPT 2: Cast vote FIRST, then apply effects.
                            // Previously: apply_block_effects (heavy) → vote → broadcast
                            // Now:        vote → fire-and-forget broadcast → apply effects
                            // This cuts ~10-20ms off the critical path per block.

                            // VOTE-AUTHORITY: Atomically check-then-sign via VoteAuthority.
                            // This is the ONLY code path that can create a signed vote
                            // in the block receiver. VoteAuthority prevents all DoubleVote
                            // scenarios: P2P echo, fork re-evaluation, and view rotation.
                            let block_hash = block.hash();
                            let maybe_vote = vote_authority_for_rx
                                .lock()
                                .await
                                .try_vote(block_slot, block_hash);

                            if let Some(vote) = maybe_vote {
                                // Add our own vote (validated against validator set)
                                {
                                    let mut agg = vote_agg_for_blocks.write().await;
                                    let vs = validator_set_for_blocks.read().await;
                                    if agg.add_vote_validated(vote.clone(), &vs) {
                                        info!("🗳️  Cast vote for block {}", block_slot);

                                        // Check if block reached finality (2/3 supermajority - STAKE-WEIGHTED)
                                        let pool = stake_pool_for_blocks.read().await;
                                        if agg.has_supermajority(
                                            block_slot,
                                            &block_hash,
                                            &vs,
                                            &pool,
                                        ) {
                                            info!("🔒 Block {} FINALIZED with stake-weighted supermajority!", block_slot);
                                            // Update finality tracker + persist to StateStore
                                            if finality_for_blocks.mark_confirmed(block_slot) {
                                                let _ = state_for_blocks.set_last_confirmed_slot(
                                                    finality_for_blocks.confirmed_slot(),
                                                );
                                                let _ = state_for_blocks.set_last_finalized_slot(
                                                    finality_for_blocks.finalized_slot(),
                                                );
                                            }
                                        }
                                        drop(pool);
                                    }
                                    // Drop agg + vs before broadcast
                                }

                                // PERF-OPT 3: Fire-and-forget vote broadcast.
                                // P3-5: Route votes through validator mesh for lowest latency.
                                // Falls back to full broadcast if no validator peers are known.
                                {
                                    let vote_msg =
                                        P2PMessage::new(MessageType::Vote(vote), local_addr);
                                    let pm = peer_mgr_for_sync.clone();
                                    tokio::spawn(async move {
                                        pm.broadcast_to_validators(vote_msg).await;
                                    });
                                }
                            } else {
                                info!(
                                    "⚠️  VoteAuthority: slot {} already voted — skipping (prevents DoubleVote)",
                                    block_slot
                                );
                            }

                            // Now apply block effects (rewards, fees) — safe to run
                            // after vote since effects don't affect block validity.
                            apply_block_effects(
                                &state_for_blocks,
                                &validator_set_for_blocks,
                                &stake_pool_for_blocks,
                                &vote_agg_for_effects,
                                &block,
                                false,
                                min_validator_stake,
                            )
                            .await;
                            apply_oracle_from_block(&state_for_blocks, &block);
                            maybe_create_checkpoint(
                                &state_for_blocks,
                                block_slot,
                                &data_dir_for_blocks,
                                &sync_mgr,
                            )
                            .await;

                            // Try to apply any pending blocks (gap-aware).
                            // try_apply_pending now verifies parent_hash internally,
                            // returning only blocks that form a valid chain from the tip.
                            let tip_hash_for_pending = block.hash();
                            let pending = sync_mgr
                                .try_apply_pending(block_slot, tip_hash_for_pending)
                                .await;
                            for pending_block in pending {
                                let pending_slot = pending_block.header.slot;
                                if sync_mgr
                                    .should_full_validate(pending_block.header.slot)
                                    .await
                                {
                                    replay_block_transactions(
                                        &processor_for_blocks,
                                        &pending_block,
                                    );
                                }
                                run_analytics_bridge_from_state(
                                    &state_for_blocks,
                                    pending_block.header.slot,
                                    genesis_config_for_blocks.consensus.slot_duration_ms.max(1),
                                );
                                run_sltp_triggers_from_state(&state_for_blocks);
                                reset_24h_stats_if_expired(
                                    &state_for_blocks,
                                    pending_block.header.timestamp,
                                );
                                if state_for_blocks
                                    .put_block_atomic(&pending_block, None, None)
                                    .is_ok()
                                {
                                    *last_block_time_for_blocks.lock().await =
                                        std::time::Instant::now();
                                    info!("✅ Applied pending block {}", pending_slot);
                                    sync_mgr.record_progress(pending_slot).await;
                                    apply_block_effects(
                                        &state_for_blocks,
                                        &validator_set_for_blocks,
                                        &stake_pool_for_blocks,
                                        &vote_agg_for_effects,
                                        &pending_block,
                                        false,
                                        min_validator_stake,
                                    )
                                    .await;
                                    apply_oracle_from_block(&state_for_blocks, &pending_block);
                                    maybe_create_checkpoint(
                                        &state_for_blocks,
                                        pending_slot,
                                        &data_dir_for_blocks,
                                        &sync_mgr,
                                    )
                                    .await;
                                }
                            }
                        }
                    } else {
                        // Parent doesn't match current tip — store as pending
                        // and let sync fill in intermediate blocks.
                        if block_slot <= current_slot + 2 {
                            // Close-ahead block that doesn't chain — may indicate fork
                            warn!(
                                "⚠️  Block {} parent mismatch (expected parent of slot {})",
                                block_slot, current_slot
                            );

                            // FIX-FORK-2: Proactive fork adoption for close-ahead blocks.
                            // If this block is for slot tip+1 but its parent doesn't match
                            // our tip block, we're on a fork. The incoming block chains
                            // from a different version of our tip slot. Trigger a sync
                            // that includes the current_slot to get the alternative block,
                            // which will enter fork choice and replace our divergent tip.
                            if block_slot == current_slot + 1 {
                                info!(
                                    "🔄 Fork detected at slot {} — requesting alternative chain",
                                    current_slot
                                );
                            }
                        }
                        sync_mgr.add_pending_block(block).await;
                    }

                    // Check if we should start sync
                    if let Some((start, end)) = sync_mgr.should_sync(current_slot).await {
                        // P1-1 / P3-1: Auto-detect sync mode based on gap size.
                        // If enormously far behind (> warp threshold), use warp sync
                        // to download state snapshot instead of replaying blocks.
                        // If moderately far behind, use header-only. Otherwise, full.
                        let gap = end.saturating_sub(current_slot);
                        if gap > sync::WARP_SYNC_THRESHOLD {
                            sync_mgr.set_sync_mode(sync::SyncMode::Warp).await;
                        } else if gap > sync::HEADER_SYNC_FULL_EXECUTION_WINDOW * 2 {
                            sync_mgr.set_sync_mode(sync::SyncMode::HeaderOnly).await;
                        } else {
                            sync_mgr.set_sync_mode(sync::SyncMode::Full).await;
                        }
                        info!("🔄 Triggering sync: blocks {} to {}", start, end);

                        // Mark that we're starting sync
                        sync_mgr.start_sync(start, end).await;

                        // P3-1: If warp sync mode, request a state snapshot
                        // instead of downloading individual blocks.
                        let current_mode = sync_mgr.get_sync_mode().await;
                        if current_mode == sync::SyncMode::Warp {
                            info!(
                                "⚡ Warp sync: gap is {} blocks — requesting state snapshot",
                                gap
                            );
                            // Send CheckpointMetaRequest to all known peers
                            let peer_infos = peer_mgr_for_sync.get_peer_infos();
                            for (peer_addr, _) in peer_infos.iter().take(3) {
                                let meta_request =
                                    P2PMessage::new(MessageType::CheckpointMetaRequest, local_addr);
                                let _ = peer_mgr_for_sync
                                    .send_to_peer(peer_addr, meta_request)
                                    .await;
                            }
                            // The CheckpointMetaResponse handler will trigger
                            // StateSnapshotRequest downloads. After all chunks
                            // are received the state root is verified and the
                            // node fast-forwards, then switches to Full mode
                            // for the remaining tip blocks.
                            continue;
                        }

                        // Chunk the range into sub-batches that fit within the
                        // P2P layer's MAX_BLOCK_RANGE limit (AUDIT-FIX H1).
                        // Without this, any gap > 100 blocks causes a permanent
                        // sync deadlock because the responder rejects oversized
                        // range requests.
                        //
                        // P2-5: Round-robin chunk→peer assignment distributes
                        // pipeline stages across peers. Peer A serves chunk 0,
                        // peer B serves chunk 1, etc. This parallelizes
                        // download across all available peers rather than
                        // having every peer serve every chunk (which wastes
                        // bandwidth on duplicates during large syncs).
                        let mut peer_infos = peer_mgr_for_sync.get_peer_infos();
                        peer_infos.sort_by(|a, b| {
                            b.1.cmp(&a.1)
                                .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
                        });
                        let all_peers: Vec<std::net::SocketAddr> = peer_infos
                            .into_iter()
                            .take(SYNC_REQUEST_FANOUT.max(1))
                            .map(|(addr, _)| addr)
                            .collect();

                        let mut chunk_start = start;
                        let mut chunk_idx: usize = 0;
                        while chunk_start <= end {
                            let chunk_end =
                                std::cmp::min(chunk_start + sync::P2P_BLOCK_RANGE_LIMIT - 1, end);

                            if all_peers.is_empty() {
                                let request_msg = P2PMessage::new(
                                    MessageType::BlockRangeRequest {
                                        start_slot: chunk_start,
                                        end_slot: chunk_end,
                                    },
                                    local_addr,
                                );
                                peer_mgr_for_sync.broadcast(request_msg).await;
                            } else {
                                // P2-5: Round-robin — assign each chunk to a
                                // different peer to maximize parallelism.
                                let peer_addr = &all_peers[chunk_idx % all_peers.len()];
                                let request_msg = P2PMessage::new(
                                    MessageType::BlockRangeRequest {
                                        start_slot: chunk_start,
                                        end_slot: chunk_end,
                                    },
                                    local_addr,
                                );
                                if let Err(e) =
                                    peer_mgr_for_sync.send_to_peer(peer_addr, request_msg).await
                                {
                                    warn!(
                                        "⚠️  Failed sync request {}-{} to {}: {}",
                                        chunk_start, chunk_end, peer_addr, e
                                    );
                                    peer_mgr_for_sync.record_violation(peer_addr);
                                } else {
                                    peer_mgr_for_sync.record_success(peer_addr);
                                }
                            }
                            info!(
                                "📡 Sent block range request: {} to {} (chunk {}, peer {})",
                                chunk_start,
                                chunk_end,
                                chunk_end - chunk_start + 1,
                                if all_peers.is_empty() {
                                    "broadcast".to_string()
                                } else {
                                    all_peers[chunk_idx % all_peers.len()].to_string()
                                }
                            );
                            chunk_start = chunk_end + 1;
                            chunk_idx += 1;
                        }

                        // Mark slots as requested in sync manager
                        for slot in start..=end {
                            sync_mgr.mark_requested(slot).await;
                        }

                        // Progress-based sync completion.
                        // Record the slot when sync started.  After a delay, check
                        // if ANY progress was made (>= 1 block applied).  Only
                        // record failure if zero blocks were applied.
                        // InitialSync: 3s check (fast catch-up)
                        // LiveSync: 5s check (stable operation)
                        let sync_mgr_complete = sync_mgr.clone();
                        let state_for_sync_check = state_for_blocks.clone();
                        let sync_start_slot = current_slot;
                        let sync_end = end;
                        tokio::spawn(async move {
                            let phase = sync_mgr_complete.get_sync_phase().await;
                            let delay = if phase == sync::SyncPhase::InitialSync {
                                3
                            } else {
                                5
                            };
                            tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                            let current = state_for_sync_check.get_last_slot().unwrap_or(0);
                            let progress = sync_mgr_complete.get_last_progress_slot().await;
                            if progress > sync_start_slot {
                                // Made progress — reset backoff even if not at target
                                sync_mgr_complete.record_sync_success().await;
                                if current < sync_end {
                                    info!(
                                        "🔄 Sync progress: {} → {} (target {}), continuing",
                                        sync_start_slot, current, sync_end
                                    );
                                }
                            } else {
                                // Zero progress — something is wrong, backoff
                                info!(
                                    "🔄 Sync batch: no progress (stuck at {}, target {})",
                                    current, sync_end
                                );
                                sync_mgr_complete.record_sync_failure().await;
                            }
                            // Always complete to allow re-trigger
                            sync_mgr_complete.complete_sync().await;
                        });
                    }
                } else if block_slot <= current_slot {
                    // During InitialSync, skip fork choice entirely — we trust
                    // sequential blocks from the peer.  Fork choice only
                    // activates during LiveSync for competing blocks at the tip.
                    if sync_mgr.get_sync_phase().await == sync::SyncPhase::InitialSync {
                        continue;
                    }
                    // BUG #5 FIX: Never replace a block at a PAST slot.
                    // Blocks at slots < current_slot already have descendants
                    // (blocks slot+1..current_slot) that reference the existing
                    // block's hash as their parent_hash. Replacing the block
                    // would permanently break the parent-hash chain, making
                    // syncing validators unable to apply blocks past that slot.
                    // Fork choice is only safe at the CURRENT TIP.
                    if block_slot < current_slot {
                        continue;
                    }
                    if let Ok(Some(existing)) = state_for_blocks.get_block_by_slot(block_slot) {
                        if existing.hash() != block.hash() {
                            // G-8 fix: BFT finality takes precedence over fork choice.
                            // If the existing block has a valid commit certificate
                            // (2/3+ precommit signatures), it was formally committed
                            // by the BFT quorum and is FINAL. No fork choice rule
                            // should ever revert a committed block.
                            if !existing.commit_signatures.is_empty() {
                                let vs = validator_set_for_blocks.read().await;
                                let pool = stake_pool_for_blocks.read().await;
                                if existing.verify_commit(&vs, &pool).is_ok() {
                                    debug!(
                                        "🛡️  BFT FINALITY: Block {} has valid commit certificate — \
                                         rejecting fork choice replacement",
                                        block_slot
                                    );
                                    continue;
                                }
                            }

                            // A5-02: Fork choice — use cumulative stake weight from
                            // ForkChoice oracle + vote weight + network position.
                            // 1. Record both competing blocks in the oracle
                            // 2. Combine oracle weight + per-block vote weight
                            // 3. Also force adoption when behind or pending blocks exist
                            let highest_seen = sync_mgr.get_highest_seen().await;
                            let we_are_behind = highest_seen > current_slot;
                            let has_pending = sync_mgr.pending_count().await > 0;

                            // Record incoming block in fork choice oracle
                            {
                                let pool = stake_pool_for_blocks.read().await;
                                let proposer = Pubkey(block.header.validator);
                                let weight = pool
                                    .get_stake(&proposer)
                                    .map(|s| s.total_stake())
                                    .unwrap_or(1);
                                fork_choice.add_head(block_slot, block.hash(), weight);
                            }

                            // PERF-OPT 6: Single lock acquisition for both fork-choice weights.
                            let (existing_weight, incoming_weight) = {
                                let agg = vote_agg_for_blocks.read().await;
                                let vs = validator_set_for_blocks.read().await;
                                let pool = stake_pool_for_blocks.read().await;
                                let ew = block_vote_weight(
                                    block_slot,
                                    &existing.hash(),
                                    &agg,
                                    &vs,
                                    &pool,
                                );
                                let iw =
                                    block_vote_weight(block_slot, &block.hash(), &agg, &vs, &pool);
                                (ew, iw)
                            };

                            // A5-02: Also consult fork choice oracle — if it has
                            // accumulated more cumulative weight on the incoming
                            // block, prefer it even if per-block votes are equal.
                            let fc_existing = fork_choice.get_weight(&existing.hash());
                            let fc_incoming = fork_choice.get_weight(&block.hash());
                            let oracle_prefers_incoming = fc_incoming > fc_existing;

                            // STABILITY-FIX: Longest-chain adoption rule.
                            // When we're behind the network AND there are pending blocks
                            // that chain from the incoming block, adopt it. This is the
                            // Nakamoto consensus rule: the longest valid chain wins.
                            // The pending_chains_from_incoming check prevents malicious
                            // validators from forcing replacements — they'd need to
                            // provide actual blocks that chain from their fork.
                            let pending_chains_from_incoming =
                                sync_mgr.has_pending_child(&block.hash()).await;
                            let longest_chain_rule =
                                we_are_behind && has_pending && pending_chains_from_incoming;

                            // PHASE-3: Finality-bound fork choice — NEVER revert a
                            // finalized block.  This is how Ethereum, Cosmos and every
                            // production PoS chain works: once a block has a valid
                            // BFT supermajority commit it is
                            // irreversible.  Accepting a reorg past finality would
                            // break the safety guarantee for all downstream consumers
                            // (exchanges, bridges, wallets).
                            let current_finalized = finality_for_blocks.finalized_slot();
                            if block_slot <= current_finalized {
                                warn!(
                                    "🛡️  FINALITY GUARD: Rejecting reorg of slot {} — \
                                     block is at or before finalized slot {}",
                                    block_slot, current_finalized
                                );
                                continue;
                            }

                            // P9-VAL-07 / AUDIT-FIX E-3: For equal-length forks, require
                            // BOTH vote weight AND oracle to agree.
                            // For provably-longer chains, adopt unconditionally.
                            if (incoming_weight > existing_weight && oracle_prefers_incoming)
                                || longest_chain_rule
                            {
                                // Revert old block's financial effects before replacing
                                revert_block_effects(
                                    &state_for_blocks,
                                    &validator_set_for_blocks,
                                    &stake_pool_for_blocks,
                                    &existing,
                                )
                                .await;
                                // C7 fix: Also revert user transaction effects
                                revert_block_transactions(
                                    &state_for_blocks,
                                    &existing,
                                    &data_dir_for_blocks,
                                );
                                // Replace slot index with the higher-weight block
                                if sync_mgr.should_full_validate(block.header.slot).await {
                                    replay_block_transactions(&processor_for_blocks, &block);
                                }
                                run_analytics_bridge_from_state(
                                    &state_for_blocks,
                                    block.header.slot,
                                    genesis_config_for_blocks.consensus.slot_duration_ms.max(1),
                                );
                                run_sltp_triggers_from_state(&state_for_blocks);
                                reset_24h_stats_if_expired(
                                    &state_for_blocks,
                                    block.header.timestamp,
                                );
                                if state_for_blocks
                                    .put_block_atomic(&block, None, None)
                                    .is_ok()
                                {
                                    *last_block_time_for_blocks.lock().await =
                                        std::time::Instant::now();
                                    // FIX-FORK-1: Record after fork adoption
                                    {
                                        let mut rns = received_slots_for_rx.lock().await;
                                        rns.insert(block_slot);
                                        if block_slot > 200 {
                                            rns.retain(|&s| s + 200 >= block_slot);
                                        }
                                    }
                                    // A5-02: Update fork choice after successful replacement
                                    fork_choice.add_head(
                                        block_slot,
                                        block.hash(),
                                        incoming_weight.max(fc_incoming),
                                    );
                                    if we_are_behind || has_pending {
                                        info!(
                                            "🔗 Chain adoption: replaced block at slot {} (behind network by {} slots, {} pending)",
                                            block_slot, highest_seen.saturating_sub(current_slot),
                                            sync_mgr.pending_count().await
                                        );
                                    } else {
                                        info!(
                                            "⚖️  Replaced block at slot {} (weight {} -> {})",
                                            block_slot, existing_weight, incoming_weight
                                        );
                                    }
                                    apply_block_effects(
                                        &state_for_blocks,
                                        &validator_set_for_blocks,
                                        &stake_pool_for_blocks,
                                        &vote_agg_for_effects,
                                        &block,
                                        false,
                                        min_validator_stake,
                                    )
                                    .await;
                                    apply_oracle_from_block(&state_for_blocks, &block);
                                    maybe_create_checkpoint(
                                        &state_for_blocks,
                                        block_slot,
                                        &data_dir_for_blocks,
                                        &sync_mgr,
                                    )
                                    .await;

                                    // After replacing a block (fork adoption), try
                                    // applying pending blocks that now chain correctly.
                                    let fork_tip_hash = block.hash();
                                    let pending =
                                        sync_mgr.try_apply_pending(block_slot, fork_tip_hash).await;
                                    for pending_block in pending {
                                        let pending_slot = pending_block.header.slot;
                                        if sync_mgr
                                            .should_full_validate(pending_block.header.slot)
                                            .await
                                        {
                                            replay_block_transactions(
                                                &processor_for_blocks,
                                                &pending_block,
                                            );
                                        }
                                        run_analytics_bridge_from_state(
                                            &state_for_blocks,
                                            pending_block.header.slot,
                                            genesis_config_for_blocks
                                                .consensus
                                                .slot_duration_ms
                                                .max(1),
                                        );
                                        run_sltp_triggers_from_state(&state_for_blocks);
                                        reset_24h_stats_if_expired(
                                            &state_for_blocks,
                                            pending_block.header.timestamp,
                                        );
                                        if state_for_blocks
                                            .put_block_atomic(&pending_block, None, None)
                                            .is_ok()
                                        {
                                            *last_block_time_for_blocks.lock().await =
                                                std::time::Instant::now();
                                            info!(
                                                "✅ Applied pending block {} (after fork adoption)",
                                                pending_slot
                                            );
                                            sync_mgr.record_progress(pending_slot).await;
                                            if sync_mgr.is_caught_up(pending_slot).await {
                                                sync_mgr.transition_to_live().await;
                                            }
                                            apply_block_effects(
                                                &state_for_blocks,
                                                &validator_set_for_blocks,
                                                &stake_pool_for_blocks,
                                                &vote_agg_for_effects,
                                                &pending_block,
                                                false,
                                                min_validator_stake,
                                            )
                                            .await;
                                            apply_oracle_from_block(
                                                &state_for_blocks,
                                                &pending_block,
                                            );
                                            maybe_create_checkpoint(
                                                &state_for_blocks,
                                                pending_slot,
                                                &data_dir_for_blocks,
                                                &sync_mgr,
                                            )
                                            .await;
                                        }
                                    }
                                }
                            } else {
                                debug!("Fork choice kept existing block at slot {}", block_slot);
                            }
                        } else {
                            debug!("Block {} already processed", block_slot);
                        }
                    }
                } else {
                    debug!("Block {} is old (current: {})", block_slot, current_slot);
                }
            }
        });

        // Start incoming transaction handler
        let mempool_for_txs = mempool.clone();
        tokio::spawn(async move {
            info!("🔄 Transaction receiver started");
            while let Some(tx) = transaction_rx.recv().await {
                info!("📥 Received transaction from P2P");
                // AUDIT-FIX 1.6: Validate transaction before adding to mempool
                // 1. Verify all required signatures (first account of each instruction)
                if !validate_p2p_transaction_signatures(&tx) {
                    info!("❌ P2P transaction rejected: invalid or missing signature");
                    continue;
                }
                // 2. Validate transaction structure (size limits, instruction count)
                if let Err(e) = tx.validate_structure() {
                    info!("❌ P2P transaction rejected: {}", e);
                    continue;
                }
                // M-8 FIX: No reputation lookup needed — mempool uses fee-only ordering.
                // Do not reject based on local account balance here: peers can be
                // briefly behind in state sync, and strict pre-checks cause mempool
                // imbalance (one validator receives TXs, others drop them).
                let fee_config = FeeConfig::default_from_constants();
                let computed_fee = TxProcessor::compute_transaction_fee(&tx, &fee_config);
                let mut pool = mempool_for_txs.lock().await;
                if let Err(e) = pool.add_transaction(tx, computed_fee, 0) {
                    info!("Mempool: {}", e);
                }
            }
        });

        // Start vote handler for BFT consensus with slashing detection
        let vote_agg_for_handler = vote_aggregator.clone();
        let validator_set_for_votes = validator_set.clone();
        let stake_pool_for_votes = stake_pool.clone();
        let slashing_for_votes = slashing_tracker.clone();
        let validator_pubkey_for_slash_report = validator_pubkey;
        let peer_mgr_for_slash = p2p_peer_manager.clone();
        let local_addr_for_slash = p2p_config.listen_addr;
        let finality_for_votes = finality_tracker.clone();
        let state_for_votes = state.clone();
        // PHASE-3: Clones needed for consensus-based slashing (opcode 27)
        let mempool_for_slash_votes = mempool.clone();
        let slash_keypair_seed_for_votes = validator_keypair.to_seed();
        let p2p_config_for_slash_votes = p2p_config.clone();

        tokio::spawn(async move {
            info!("🔄 Vote receiver started");

            // Track votes per validator to detect double-voting
            let mut validator_votes: std::collections::HashMap<(lichen_core::Pubkey, u64), Vote> =
                std::collections::HashMap::new();

            while let Some(vote) = vote_rx.recv().await {
                // Prune old entries to prevent memory leak (keep last 100 slots)
                if validator_votes.len() > 500 {
                    let cutoff = vote.slot.saturating_sub(100);
                    validator_votes.retain(|&(_, slot), _| slot >= cutoff);
                }

                // Skip our own votes (we already counted them when we cast)
                if vote.validator == validator_pubkey_for_slash_report {
                    debug!("Skipping self-vote for block {}", vote.slot);
                    continue;
                }

                info!(
                    "📥 Received vote for block {} from {}",
                    vote.slot,
                    vote.validator.to_base58()
                );

                // Check for double-voting before adding
                let vote_key = (vote.validator, vote.slot);
                if let Some(existing_vote) = validator_votes.get(&vote_key) {
                    if existing_vote.block_hash != vote.block_hash {
                        // DOUBLE VOTE DETECTED!
                        warn!(
                            "⚔️  DOUBLE VOTE detected from {} at slot {}",
                            vote.validator.to_base58(),
                            vote.slot
                        );

                        let evidence = SlashingEvidence::new(
                            SlashingOffense::DoubleVote {
                                slot: vote.slot,
                                vote_1: existing_vote.clone(),
                                vote_2: vote.clone(),
                            },
                            vote.validator,
                            vote.slot,
                            validator_pubkey_for_slash_report,
                            vote.timestamp / 1000,
                        );

                        // Add to slashing tracker
                        let mut slasher = slashing_for_votes.lock().await;
                        if slasher.add_evidence(evidence.clone()) {
                            info!(
                                "⚔️  Slashing evidence recorded for {}",
                                vote.validator.to_base58()
                            );

                            // Broadcast evidence to network
                            if let Some(ref peer_mgr) = peer_mgr_for_slash {
                                let evidence_msg = P2PMessage::new(
                                    MessageType::SlashingEvidence(evidence.clone()),
                                    local_addr_for_slash,
                                );
                                peer_mgr.broadcast(evidence_msg).await;
                            }

                            // PHASE-3: Submit SlashValidator tx through consensus
                            // (opcode 27) so all nodes apply the same penalty
                            if let Ok(evidence_bytes) = bincode::serialize(&evidence) {
                                let mut ix_data = vec![27u8];
                                ix_data.extend_from_slice(&evidence_bytes);
                                let tip = state_for_votes.get_last_slot().unwrap_or(0);
                                if let Ok(Some(tip_block)) = state_for_votes.get_block_by_slot(tip)
                                {
                                    let ix = lichen_core::Instruction {
                                        program_id: lichen_core::processor::SYSTEM_PROGRAM_ID,
                                        accounts: vec![vote.validator],
                                        data: ix_data,
                                    };
                                    let msg = lichen_core::Message::new(vec![ix], tip_block.hash());
                                    let mut tx = Transaction::new(msg);
                                    let kp = Keypair::from_seed(&slash_keypair_seed_for_votes);
                                    let sig = kp.sign(&tx.message.serialize());
                                    tx.signatures.push(sig);
                                    {
                                        let mut pool = mempool_for_slash_votes.lock().await;
                                        if let Err(e) = pool.add_transaction(tx.clone(), 0, 0) {
                                            warn!("⚠️  Failed to add SlashValidator tx to mempool: {}", e);
                                        }
                                    }
                                    if let Some(ref peer_mgr) = peer_mgr_for_slash {
                                        let target_id = tx.hash().0;
                                        let slash_msg = lichen_p2p::P2PMessage::new(
                                            lichen_p2p::MessageType::Transaction(tx),
                                            p2p_config_for_slash_votes.listen_addr,
                                        );
                                        peer_mgr
                                            .route_to_closest(
                                                &target_id,
                                                lichen_p2p::NON_CONSENSUS_FANOUT,
                                                slash_msg,
                                            )
                                            .await;
                                    }
                                    info!(
                                        "📝 Submitted SlashValidator tx for DoubleVote by {}",
                                        vote.validator.to_base58()
                                    );
                                }
                            }
                        }
                        drop(slasher);
                        continue; // Don't add double vote
                    }
                } else {
                    // First vote from this validator at this slot
                    validator_votes.insert(vote_key, vote.clone());
                }

                let mut agg = vote_agg_for_handler.write().await;
                let mut vs = validator_set_for_votes.write().await;
                if agg.add_vote_validated(vote.clone(), &vs) {
                    // STABILITY-FIX: A validated vote proves the validator is online
                    // and actively processing blocks. Update last_active_slot so the
                    // downtime detector doesn't falsely flag voting validators that
                    // simply aren't the current block leader.
                    if let Some(val) = vs.get_validator_mut(&vote.validator) {
                        if vote.slot > val.last_active_slot {
                            val.last_active_slot = vote.slot;
                        }
                        val.votes_cast += 1;
                    }

                    // Vote added successfully, check if block reached finality
                    let pool = stake_pool_for_votes.read().await;
                    let vote_count = agg.vote_count(vote.slot, &vote.block_hash);

                    if agg.has_supermajority(vote.slot, &vote.block_hash, &vs, &pool) {
                        info!(
                            "🔒 Block {} FINALIZED! (stake-weighted votes: {}/{})",
                            vote.slot,
                            vote_count,
                            vs.validators().len()
                        );
                        // Update finality tracker + persist to StateStore
                        if finality_for_votes.mark_confirmed(vote.slot) {
                            let _ = state_for_votes
                                .set_last_confirmed_slot(finality_for_votes.confirmed_slot());
                            let _ = state_for_votes
                                .set_last_finalized_slot(finality_for_votes.finalized_slot());
                        }
                    } else {
                        info!(
                            "🗳️  Vote accepted for block {} ({}/{})",
                            vote.slot,
                            vote_count,
                            vs.validators().len()
                        );
                    }
                    drop(pool);
                    drop(vs);
                } else {
                    debug!(
                        "Vote rejected for block {} (duplicate or invalid)",
                        vote.slot
                    );
                }
                drop(agg);
            }
        });

        // Start validator announcement handler
        let state_for_validators = state.clone();
        let validator_set_for_announce = validator_set.clone();
        let stake_pool_for_announce = stake_pool.clone();
        let validator_pubkey_for_announce_handler = validator_pubkey;
        let sync_mgr_for_announce = sync_manager.clone();
        tokio::spawn(async move {
            info!("🔄 Validator announcement receiver started");
            // 1.5d: Per-minute announcement rate limiting
            let mut last_announce_times: std::collections::HashMap<
                lichen_core::account::Pubkey,
                std::time::Instant,
            > = std::collections::HashMap::new();
            while let Some(announcement) = validator_announce_rx.recv().await {
                // Skip our own announcements
                if announcement.pubkey == validator_pubkey_for_announce_handler {
                    continue;
                }

                // 1.5d: Rate limit — at most one announcement per pubkey per 60s
                let now = std::time::Instant::now();
                if let Some(last) = last_announce_times.get(&announcement.pubkey) {
                    if now.duration_since(*last) < std::time::Duration::from_secs(60) {
                        debug!(
                            "⚠️  Rate-limited announcement from {} (< 60s since last)",
                            announcement.pubkey.to_base58()
                        );
                        continue;
                    }
                }
                last_announce_times.insert(announcement.pubkey, now);

                info!(
                    "🦞 Received validator announcement: {}",
                    announcement.pubkey.to_base58()
                );

                let mut vs = validator_set_for_announce.write().await;
                let is_existing_validator = vs.get_validator(&announcement.pubkey).is_some();

                if !is_existing_validator {
                    if let Err(error) = validate_new_validator_version(&announcement.version) {
                        warn!(
                            "⚠️  Rejecting validator announcement from {} — {}",
                            announcement.pubkey.to_base58(),
                            error
                        );
                        drop(vs);
                        continue;
                    }
                }

                // Cap validator set size
                const MAX_VALIDATORS: usize = 1000;

                // Update highest seen slot from announcement so sync
                // manager knows the network tip even before any blocks are
                // processed by the block receiver.  Without this, a joining
                // node's highest_seen_slot stays 0 after genesis and
                // should_sync never fires.
                sync_mgr_for_announce
                    .note_seen_bounded(announcement.current_slot, 500)
                    .await;

                // Check if validator already exists
                if is_existing_validator {
                    // Update existing validator's activity
                    if let Some(val) = vs.get_validator_mut(&announcement.pubkey) {
                        val.last_active_slot = announcement.current_slot;
                    }

                    // Check for fingerprint migration (same pubkey, new machine)
                    if announcement.machine_fingerprint != [0u8; 32] {
                        let mut pool = stake_pool_for_announce.write().await;
                        if let Some(stake_info) = pool.get_stake(&announcement.pubkey) {
                            let current_fp = stake_info.machine_fingerprint;
                            if current_fp != [0u8; 32]
                                && current_fp != announcement.machine_fingerprint
                            {
                                info!(
                                    "🔄 Machine migration detected for {} — updating fingerprint",
                                    announcement.pubkey.to_base58()
                                );
                                match pool.migrate_fingerprint(
                                    &announcement.pubkey,
                                    announcement.machine_fingerprint,
                                    announcement.current_slot,
                                ) {
                                    Ok(()) => info!(
                                        "✅ Fingerprint migrated for {}",
                                        announcement.pubkey.to_base58()
                                    ),
                                    Err(e) => warn!(
                                        "⚠️  Fingerprint migration failed for {}: {}",
                                        announcement.pubkey.to_base58(),
                                        e
                                    ),
                                }
                            } else if current_fp == [0u8; 32] {
                                // Legacy validator — register fingerprint for the first time
                                match pool.register_fingerprint(
                                    &announcement.pubkey,
                                    announcement.machine_fingerprint,
                                ) {
                                    Ok(()) => info!(
                                        "🔒 Late fingerprint registered for {}",
                                        announcement.pubkey.to_base58()
                                    ),
                                    Err(e) => debug!(
                                        "Fingerprint registration skipped for {}: {}",
                                        announcement.pubkey.to_base58(),
                                        e
                                    ),
                                }
                            }
                        }
                        drop(pool);
                    }
                } else {
                    // ── PHANTOM VALIDATOR GUARD ──
                    // Reject new validators whose announced slot diverges too far
                    // from our chain tip. A legitimate validator on the same network
                    // will be within a few hundred slots. A node with stale/altered
                    // state (e.g. a local dev validator that wasn't flushed) will
                    // announce slot 0 or a completely different slot range, which
                    // would contaminate our validator set and break leader election.
                    let our_tip = state_for_validators.get_last_slot().unwrap_or(0);
                    let their_slot = announcement.current_slot;
                    let slot_drift = their_slot.abs_diff(our_tip);
                    // Allow generous 500-slot window for sync lag
                    const MAX_SLOT_DRIFT_FOR_NEW_VALIDATOR: u64 = 500;
                    if our_tip > 10 && slot_drift > MAX_SLOT_DRIFT_FOR_NEW_VALIDATOR {
                        warn!(
                            "⚠️  Rejecting new validator {} — slot drift too large (ours={}, theirs={}, drift={}). Likely stale or altered state.",
                            announcement.pubkey.to_base58(),
                            our_tip,
                            their_slot,
                            slot_drift,
                        );
                        drop(vs);
                        continue;
                    }

                    // Reject if at capacity
                    if vs.validators().len() >= MAX_VALIDATORS {
                        warn!(
                            "⚠️  Validator set full ({} validators) — rejecting {}",
                            MAX_VALIDATORS,
                            announcement.pubkey.to_base58()
                        );
                        drop(vs);
                        continue;
                    }

                    // Pre-check: reject if machine fingerprint is already registered
                    // to a different pubkey (prevents ghost creation from keypair changes)
                    if announcement.machine_fingerprint != [0u8; 32] {
                        let pool = stake_pool_for_announce.read().await;
                        if let Some(existing_pk) =
                            pool.fingerprint_owner(&announcement.machine_fingerprint)
                        {
                            if existing_pk != &announcement.pubkey {
                                warn!(
                                    "⚠️  Rejecting new validator {} — machine fingerprint already belongs to {}",
                                    announcement.pubkey.to_base58(),
                                    existing_pk.to_base58()
                                );
                                drop(pool);
                                drop(vs);
                                continue;
                            }
                        }
                        drop(pool);
                    }

                    // 1.5a: Defense-in-depth — re-verify announcement signature
                    //        New validator admissions require a version-bound signature.
                    if !verify_validator_announcement_signature(
                        &announcement.pubkey,
                        announcement.stake,
                        announcement.current_slot,
                        &announcement.version,
                        &announcement.signature,
                        &announcement.machine_fingerprint,
                        true,
                    ) {
                        warn!(
                            "⚠️  Rejecting announcement from {} — invalid version-bound signature",
                            announcement.pubkey.to_base58()
                        );
                        drop(vs);
                        continue;
                    }

                    // ── DISCOVERY-ONLY: Add to ValidatorSet for peer routing ──
                    // No bootstrap accounts, no treasury debits, no stake pool changes.
                    // Validator must submit a RegisterValidator transaction (opcode 26)
                    // through consensus to receive a bootstrap grant and enter the stake pool.
                    let on_chain_stake = state_for_validators
                        .get_account(&announcement.pubkey)
                        .unwrap_or(None)
                        .map(|a| a.staked)
                        .unwrap_or(0);
                    let local_tip = state_for_validators.get_last_slot().unwrap_or(0);
                    let local_stake = state_for_validators
                        .get_stake_pool()
                        .ok()
                        .and_then(|pool| {
                            pool.get_stake(&announcement.pubkey)
                                .map(|stake| stake.total_stake())
                        })
                        .unwrap_or(on_chain_stake);

                    // Height-based activation: new validators are added for
                    // P2P routing immediately but flagged pending_activation=true
                    // so they're excluded from consensus until the next height
                    // boundary after their on-chain stake is visible locally.
                    let current_slot = announcement.current_slot;
                    let current_epoch = lichen_core::slot_to_epoch(current_slot);
                    let pending = should_add_announced_validator_as_pending(
                        local_tip,
                        local_stake,
                        min_validator_stake,
                    );
                    let new_validator = ValidatorInfo {
                        pubkey: announcement.pubkey,
                        reputation: 100,
                        blocks_proposed: 0,
                        votes_cast: 0,
                        correct_votes: 0,
                        stake: on_chain_stake,
                        joined_slot: current_slot,
                        last_active_slot: current_slot,
                        commission_rate: 500,
                        transactions_processed: 0,
                        pending_activation: pending,
                    };
                    vs.add_validator(new_validator);

                    // Queue the pending change in state for observability and restart recovery
                    if pending {
                        let change = lichen_core::PendingValidatorChange {
                            pubkey: announcement.pubkey,
                            change_type: lichen_core::ValidatorChangeType::Add,
                            queued_at_slot: current_slot,
                            effective_epoch: current_epoch + 1,
                        };
                        if let Err(e) = state_for_validators.queue_pending_validator_change(&change)
                        {
                            warn!(
                                "⚠️  Failed to queue pending validator change for {}: {}",
                                announcement.pubkey.to_base58(),
                                e
                            );
                        }
                    }

                    if on_chain_stake == 0 {
                        info!(
                            "📋 New validator {} added for peer routing (pending activation at epoch {}, no on-chain stake yet)",
                            announcement.pubkey.to_base58(),
                            current_epoch + 1,
                        );
                    } else if pending {
                        info!(
                            "⏳ Validator {} queued for consensus activation at epoch {} ({} LICN staked)",
                            announcement.pubkey.to_base58(),
                            current_epoch + 1,
                            on_chain_stake / 1_000_000_000,
                        );
                    } else {
                        info!(
                            "✅ Validator {} added with on-chain stake {} LICN (genesis activation)",
                            announcement.pubkey.to_base58(),
                            on_chain_stake / 1_000_000_000
                        );
                    }
                }

                // Persist to state
                if let Err(e) = state_for_validators.save_validator_set(&vs) {
                    warn!("⚠️  Failed to save validator set: {}", e);
                } else {
                    let active = vs
                        .validators()
                        .iter()
                        .filter(|v| !v.pending_activation)
                        .count();
                    let pending = vs.pending_count();
                    info!(
                        "✅ Updated validator set ({} active, {} pending)",
                        active, pending
                    );
                }
                drop(vs);
            }
        });

        // Start block range request handler
        let state_for_block_requests = state.clone();
        let peer_mgr_for_responses = p2p_pm.clone();
        let local_addr_for_responses = p2p_config.listen_addr;
        tokio::spawn(async move {
            info!("🔄 Block range request handler started");
            const RATE_LIMIT_WINDOW_SECS: u64 = 10;
            const MAX_REQUESTS_PER_WINDOW: u64 = 30;
            let mut rate_limits: HashMap<std::net::SocketAddr, (u64, std::time::Instant)> =
                HashMap::new();
            let mut strikes: HashMap<std::net::SocketAddr, u32> = HashMap::new();
            let mut serve_tokens: HashMap<std::net::SocketAddr, TokenBucket> = HashMap::new();
            let mut last_prune = std::time::Instant::now();
            while let Some(request) = block_range_request_rx.recv().await {
                // M5 fix: Prune stale rate_limits and strikes entries every 60s
                if last_prune.elapsed().as_secs() >= 60 {
                    let cutoff = std::time::Instant::now() - std::time::Duration::from_secs(300);
                    rate_limits.retain(|_, (_, last_seen)| *last_seen > cutoff);
                    // Cap strikes map at 500 entries
                    if strikes.len() > 500 {
                        strikes.clear();
                    }
                    if serve_tokens.len() > 1000 {
                        serve_tokens.clear();
                    }
                    last_prune = std::time::Instant::now();
                }

                // Ignore accidental self-targeted requests (e.g. looped topology)
                // without penalizing local peer score.
                if request.requester == local_addr_for_responses {
                    debug!(
                        "Skipping self block range request {}-{}",
                        request.start_slot, request.end_slot
                    );
                    continue;
                }

                if !peer_mgr_for_responses
                    .get_peers()
                    .contains(&request.requester)
                {
                    warn!(
                        "⚠️  Ignoring block range request from unknown peer {}",
                        request.requester
                    );
                    continue;
                }

                if request.end_slot < request.start_slot {
                    warn!(
                        "⚠️  Invalid block range request {}-{} from {}",
                        request.start_slot, request.end_slot, request.requester
                    );
                    peer_mgr_for_responses.record_violation(&request.requester);
                    let count = strikes.entry(request.requester).or_insert(0);
                    *count = count.saturating_add(1);
                    if *count >= 3 {
                        warn!(
                            "⚠️  Banning peer {} — exceeded invalid request limit ({})",
                            request.requester, count
                        );
                        for _ in 0..5 {
                            peer_mgr_for_responses.record_violation(&request.requester);
                        }
                    }
                    continue;
                }

                let now = std::time::Instant::now();
                let entry = rate_limits.entry(request.requester).or_insert((0, now));
                if now.duration_since(entry.1).as_secs() >= RATE_LIMIT_WINDOW_SECS {
                    *entry = (0, now);
                }
                entry.0 = entry.0.saturating_add(1);
                if entry.0 > MAX_REQUESTS_PER_WINDOW {
                    debug!(
                        "Rate limit exceeded for {} ({} requests / {}s)",
                        request.requester, entry.0, RATE_LIMIT_WINDOW_SECS
                    );
                    continue;
                }

                let range_size = request.end_slot.saturating_sub(request.start_slot) + 1;

                let bucket = serve_tokens.entry(request.requester).or_insert_with(|| {
                    TokenBucket::new(
                        BLOCK_RANGE_SERVE_BURST_BLOCKS,
                        BLOCK_RANGE_SERVE_REFILL_BLOCKS_PER_SEC,
                    )
                });
                if !bucket.try_consume(range_size.max(1)) {
                    debug!(
                        "Rate limited block serve for {} ({} blocks requested)",
                        request.requester, range_size
                    );
                    continue;
                }

                // Rate limiting: prevent excessive requests
                if range_size > 2500 {
                    warn!(
                        "⚠️  Block range request too large: {} blocks from {}",
                        range_size, request.requester
                    );
                    peer_mgr_for_responses.record_violation(&request.requester);
                    let count = strikes.entry(request.requester).or_insert(0);
                    *count = count.saturating_add(1);
                    if *count >= 3 {
                        warn!(
                            "⚠️  Banning peer {} — exceeded invalid request limit ({})",
                            request.requester, count
                        );
                        for _ in 0..5 {
                            peer_mgr_for_responses.record_violation(&request.requester);
                        }
                    }
                    continue;
                }

                info!(
                    "📦 Processing block range request: {} to {} ({} blocks) from {}",
                    request.start_slot, request.end_slot, range_size, request.requester
                );

                // Load blocks from state (in chunks to avoid memory spike)
                let mut blocks = Vec::new();
                for slot in request.start_slot..=request.end_slot {
                    if let Ok(Some(block)) = state_for_block_requests.get_block_by_slot(slot) {
                        blocks.push(block);
                    }

                    // Limit response size to prevent memory issues
                    if blocks.len() >= 2000 {
                        warn!("⚠️  Truncating block response at 2000 blocks");
                        break;
                    }
                }

                if !blocks.is_empty() {
                    // P1-2: Adaptive batching — batch 50 blocks per message
                    // for large sync requests (>100 blocks), 1 per message
                    // for small/NAT-safe requests.
                    let batch_size = if range_size > 100 { 50 } else { 1 };
                    info!(
                        "📤 Sending {} blocks to {} (batch_size={})",
                        blocks.len(),
                        request.requester,
                        batch_size
                    );

                    for chunk in blocks.chunks(batch_size as usize) {
                        let response_msg = P2PMessage::new(
                            MessageType::BlockRangeResponse {
                                blocks: chunk.to_vec(),
                            },
                            local_addr_for_responses,
                        );
                        if let Err(e) = peer_mgr_for_responses
                            .send_to_peer(&request.requester, response_msg)
                            .await
                        {
                            warn!("Failed to send block response: {}", e);
                            break;
                        }
                    }
                    peer_mgr_for_responses.record_success(&request.requester);
                } else {
                    info!(
                        "⚠️  No blocks found for range {} to {}",
                        request.start_slot, request.end_slot
                    );
                }
            }
        });

        // Start status request handler
        let state_for_status = state.clone();
        let peer_mgr_for_status = p2p_pm.clone();
        let local_addr_for_status = p2p_config.listen_addr;
        tokio::spawn(async move {
            info!("🔄 Status request handler started");
            while let Some(request) = status_request_rx.recv().await {
                if !peer_mgr_for_status.get_peers().contains(&request.requester) {
                    warn!(
                        "⚠️  Ignoring status request from unknown peer {}",
                        request.requester
                    );
                    peer_mgr_for_status.record_violation(&request.requester);
                    continue;
                }
                let current_slot = state_for_status.get_last_slot().unwrap_or(0);
                let total_blocks = state_for_status.get_metrics().total_blocks;
                let response = P2PMessage::new(
                    MessageType::StatusResponse {
                        current_slot,
                        total_blocks,
                    },
                    local_addr_for_status,
                );
                if let Err(e) = peer_mgr_for_status
                    .send_to_peer(&request.requester, response)
                    .await
                {
                    warn!("⚠️  Failed to send status response: {}", e);
                    peer_mgr_for_status.record_violation(&request.requester);
                } else {
                    peer_mgr_for_status.record_success(&request.requester);
                }
            }
        });

        // Start status response handler
        let sync_mgr_for_status = sync_manager.clone();
        tokio::spawn(async move {
            while let Some(response) = status_response_rx.recv().await {
                // C5 fix: use bounded update to prevent malicious slot inflation
                // Cap at 500 slots ahead of current highest — enough for legitimate
                // sync but prevents u64::MAX attacks on fork choice.
                sync_mgr_for_status
                    .note_seen_bounded(response.current_slot, 500)
                    .await;
                debug!(
                    "📡 Peer {} reports slot {} ({} blocks)",
                    response.requester, response.current_slot, response.total_blocks
                );
            }
        });

        // Start consistency report handler
        let validator_set_for_consistency = validator_set.clone();
        let stake_pool_for_consistency = stake_pool.clone();
        let peer_mgr_for_consistency = p2p_pm.clone();
        let local_addr_for_consistency = p2p_config.listen_addr;
        tokio::spawn(async move {
            let mut last_request: HashMap<(std::net::SocketAddr, u8), std::time::Instant> =
                HashMap::new();
            while let Some(report) = consistency_report_rx.recv().await {
                let vs = validator_set_for_consistency.read().await;
                let pool = stake_pool_for_consistency.read().await;
                let local_vs_hash = hash_validator_set(&vs);
                let local_pool_hash = hash_stake_pool(&pool);
                drop(pool);
                drop(vs);

                if report.validator_set_hash != local_vs_hash {
                    warn!("⚠️  Validator set mismatch with {}", report.requester);
                    let key = (report.requester, 0u8);
                    let should_request = last_request
                        .get(&key)
                        .map(|instant| instant.elapsed().as_secs() >= 30)
                        .unwrap_or(true);
                    if should_request {
                        let request = P2PMessage::new(
                            MessageType::SnapshotRequest {
                                kind: SnapshotKind::ValidatorSet,
                            },
                            local_addr_for_consistency,
                        );
                        if let Err(e) = peer_mgr_for_consistency
                            .send_to_peer(&report.requester, request)
                            .await
                        {
                            warn!("⚠️  Failed to request validator set snapshot: {}", e);
                            peer_mgr_for_consistency.record_violation(&report.requester);
                        } else {
                            last_request.insert(key, std::time::Instant::now());
                        }
                    }
                }
                if report.stake_pool_hash != local_pool_hash {
                    warn!("⚠️  Stake pool mismatch with {}", report.requester);
                    let key = (report.requester, 1u8);
                    let should_request = last_request
                        .get(&key)
                        .map(|instant| instant.elapsed().as_secs() >= 30)
                        .unwrap_or(true);
                    if should_request {
                        let request = P2PMessage::new(
                            MessageType::SnapshotRequest {
                                kind: SnapshotKind::StakePool,
                            },
                            local_addr_for_consistency,
                        );
                        if let Err(e) = peer_mgr_for_consistency
                            .send_to_peer(&report.requester, request)
                            .await
                        {
                            warn!("⚠️  Failed to request stake pool snapshot: {}", e);
                            peer_mgr_for_consistency.record_violation(&report.requester);
                        } else {
                            last_request.insert(key, std::time::Instant::now());
                        }
                    }
                }
            }
        });

        // Start snapshot request handler
        let validator_set_for_snapshot = validator_set.clone();
        let stake_pool_for_snapshot = stake_pool.clone();
        let state_for_snapshot_serve = state.clone();
        let peer_mgr_for_snapshot = p2p_pm.clone();
        let local_addr_for_snapshot = p2p_config.listen_addr;
        let data_dir_for_snapshot = data_dir.clone();
        tokio::spawn(async move {
            info!("🔄 Snapshot request handler started");
            let mut snapshot_tokens: HashMap<std::net::SocketAddr, TokenBucket> = HashMap::new();
            #[allow(clippy::type_complexity)]
            let mut snapshot_export_cursors: std::collections::HashMap<
                (std::net::SocketAddr, String, u64),
                (u64, Option<Vec<u8>>, u64),
            > = std::collections::HashMap::new();
            // AUDIT-FIX M1: Track cursor last-access time for TTL eviction
            let mut cursor_last_access: std::collections::HashMap<
                (std::net::SocketAddr, String, u64),
                std::time::Instant,
            > = std::collections::HashMap::new();
            while let Some(request) = snapshot_request_rx.recv().await {
                // AUDIT-FIX M1: Evict cursors idle for >30 minutes
                {
                    let now = std::time::Instant::now();
                    cursor_last_access.retain(|k, last| {
                        if now.duration_since(*last).as_secs() > 1800 {
                            snapshot_export_cursors.remove(k);
                            false
                        } else {
                            true
                        }
                    });
                }
                if !peer_mgr_for_snapshot
                    .get_peers()
                    .contains(&request.requester)
                {
                    warn!(
                        "⚠️  Ignoring snapshot request from unknown peer {}",
                        request.requester
                    );
                    peer_mgr_for_snapshot.record_violation(&request.requester);
                    continue;
                }

                let snapshot_cost = if request.is_meta_request {
                    1
                } else if let Some((_, _, chunk_size)) = request.state_snapshot_params {
                    (chunk_size.min(MAX_SNAPSHOT_CHUNK_SIZE) / 256).max(1)
                } else {
                    2
                };
                let bucket = snapshot_tokens.entry(request.requester).or_insert_with(|| {
                    TokenBucket::new(
                        SNAPSHOT_SERVE_BURST_UNITS,
                        SNAPSHOT_SERVE_REFILL_UNITS_PER_SEC,
                    )
                });
                if !bucket.try_consume(snapshot_cost) {
                    debug!(
                        "Rate limited snapshot serve for {} (cost {})",
                        request.requester, snapshot_cost
                    );
                    continue;
                }

                // Handle CheckpointMetaRequest
                if request.is_meta_request {
                    let (
                        slot,
                        state_root,
                        total_accounts,
                        checkpoint_header,
                        commit_round,
                        commit_signatures,
                    ) = {
                        let vs = validator_set_for_snapshot.read().await;
                        let pool = stake_pool_for_snapshot.read().await;
                        match latest_verified_checkpoint(
                            &data_dir_for_snapshot,
                            &state_for_snapshot_serve,
                            &vs,
                            &pool,
                        ) {
                            Some((meta, _, block)) => (
                                meta.slot,
                                meta.state_root,
                                meta.total_accounts,
                                Some(block.header.clone()),
                                block.commit_round,
                                block.commit_signatures.clone(),
                            ),
                            None => (0, [0u8; 32], 0, None, 0, Vec::new()),
                        }
                    };
                    let msg = P2PMessage::new(
                        MessageType::CheckpointMetaResponse {
                            slot,
                            state_root,
                            total_accounts,
                            checkpoint_header,
                            commit_round,
                            commit_signatures,
                        },
                        local_addr_for_snapshot,
                    );
                    if let Err(e) = peer_mgr_for_snapshot
                        .send_to_peer(&request.requester, msg)
                        .await
                    {
                        warn!("⚠️  Failed to send checkpoint meta response: {}", e);
                    }
                    continue;
                }

                // Handle StateSnapshotRequest (chunked state transfer)
                if let Some((ref category, chunk_index, chunk_size)) = request.state_snapshot_params
                {
                    // Serve state only from a finalized checkpoint backed by a verified commit.
                    let checkpoint_store = {
                        let vs = validator_set_for_snapshot.read().await;
                        let pool = stake_pool_for_snapshot.read().await;
                        match latest_verified_checkpoint(
                            &data_dir_for_snapshot,
                            &state_for_snapshot_serve,
                            &vs,
                            &pool,
                        ) {
                            Some((meta, path, _block)) => {
                                match StateStore::open_checkpoint(&path) {
                                    Ok(store) => Some((store, meta)),
                                    Err(e) => {
                                        warn!("⚠️  Failed to open checkpoint for snapshot: {}", e);
                                        None
                                    }
                                }
                            }
                            None => None,
                        }
                    };

                    if let Some((store, meta)) = checkpoint_store {
                        // RPC-M06 FIX: Use cursor-paginated export so serving
                        // chunk N no longer rescans O(N*chunk_size) from start.
                        let chunk_sz = chunk_size.clamp(1, MAX_SNAPSHOT_CHUNK_SIZE);

                        let cache_key = (request.requester, category.clone(), chunk_sz);
                        if chunk_index == 0 {
                            snapshot_export_cursors.remove(&cache_key);
                        }

                        let total_entries = match category.as_str() {
                            "accounts" => meta.total_accounts,
                            "contract_storage" => {
                                store.count_contract_storage_entries().unwrap_or(0)
                            }
                            "programs" => store.get_program_count(),
                            _ => 0,
                        };
                        let total_chunks = total_entries.div_ceil(chunk_sz).max(1);

                        let entry = snapshot_export_cursors.entry(cache_key.clone()).or_insert((
                            0,
                            None,
                            total_chunks,
                        ));
                        // AUDIT-FIX M1: Track cursor access time
                        cursor_last_access.insert(cache_key.clone(), std::time::Instant::now());
                        entry.2 = total_chunks;

                        if chunk_index != entry.0 {
                            // Rebuild cursor position for out-of-order requests.
                            let mut replay_cursor: Option<Vec<u8>> = None;
                            let mut replay_index = 0u64;
                            while replay_index < chunk_index {
                                let replay_page = match category.as_str() {
                                    "accounts" => store
                                        .export_accounts_cursor(replay_cursor.as_deref(), chunk_sz),
                                    "contract_storage" => store.export_contract_storage_cursor(
                                        replay_cursor.as_deref(),
                                        chunk_sz,
                                    ),
                                    "programs" => store
                                        .export_programs_cursor(replay_cursor.as_deref(), chunk_sz),
                                    _ => Ok(lichen_core::state::KvPage {
                                        entries: Vec::new(),
                                        total: 0,
                                        next_cursor: None,
                                        has_more: false,
                                    }),
                                }
                                .unwrap_or_else(|_| lichen_core::state::KvPage {
                                    entries: Vec::new(),
                                    total: 0,
                                    next_cursor: None,
                                    has_more: false,
                                });

                                replay_cursor = replay_page.next_cursor;
                                replay_index = replay_index.saturating_add(1);
                                if !replay_page.has_more {
                                    break;
                                }
                            }
                            entry.0 = replay_index;
                            entry.1 = replay_cursor;
                        }

                        let page = match category.as_str() {
                            "accounts" => {
                                store.export_accounts_cursor(entry.1.as_deref(), chunk_sz)
                            }
                            "contract_storage" => {
                                store.export_contract_storage_cursor(entry.1.as_deref(), chunk_sz)
                            }
                            "programs" => {
                                store.export_programs_cursor(entry.1.as_deref(), chunk_sz)
                            }
                            _ => Ok(lichen_core::state::KvPage {
                                entries: Vec::new(),
                                total: 0,
                                next_cursor: None,
                                has_more: false,
                            }),
                        }
                        .unwrap_or_else(|_| lichen_core::state::KvPage {
                            entries: Vec::new(),
                            total: 0,
                            next_cursor: None,
                            has_more: false,
                        });

                        entry.0 = chunk_index.saturating_add(1);
                        entry.1 = page.next_cursor.clone();
                        if !page.has_more {
                            snapshot_export_cursors.remove(&cache_key);
                        }

                        let chunk = page.entries;

                        let entries_bytes = bincode::serialize(&chunk).unwrap_or_default();
                        let msg = P2PMessage::new(
                            MessageType::StateSnapshotResponse {
                                category: category.clone(),
                                chunk_index,
                                total_chunks: total_chunks.max(1),
                                snapshot_slot: meta.slot,
                                state_root: meta.state_root,
                                entries: entries_bytes,
                            },
                            local_addr_for_snapshot,
                        );
                        if let Err(e) = peer_mgr_for_snapshot
                            .send_to_peer(&request.requester, msg)
                            .await
                        {
                            warn!("⚠️  Failed to send state snapshot chunk: {}", e);
                        } else {
                            info!(
                                "📤 Sent {} snapshot chunk {}/{} to {}",
                                category,
                                chunk_index + 1,
                                total_chunks,
                                request.requester
                            );
                        }
                    }
                    continue;
                }

                // Handle regular ValidatorSet / StakePool snapshot requests
                let response = match request.kind {
                    SnapshotKind::ValidatorSet => {
                        let vs = validator_set_for_snapshot.read().await;
                        P2PMessage::new(
                            MessageType::SnapshotResponse {
                                kind: SnapshotKind::ValidatorSet,
                                validator_set: Some(vs.clone()),
                                stake_pool: None,
                            },
                            local_addr_for_snapshot,
                        )
                    }
                    SnapshotKind::StakePool => {
                        let pool = stake_pool_for_snapshot.read().await;
                        P2PMessage::new(
                            MessageType::SnapshotResponse {
                                kind: SnapshotKind::StakePool,
                                validator_set: None,
                                stake_pool: Some(pool.clone()),
                            },
                            local_addr_for_snapshot,
                        )
                    }
                    SnapshotKind::StateCheckpoint => {
                        // Generic StateCheckpoint request — respond with meta
                        let (
                            slot,
                            state_root,
                            total_accounts,
                            checkpoint_header,
                            commit_round,
                            commit_signatures,
                        ) = {
                            let vs = validator_set_for_snapshot.read().await;
                            let pool = stake_pool_for_snapshot.read().await;
                            match latest_verified_checkpoint(
                                &data_dir_for_snapshot,
                                &state_for_snapshot_serve,
                                &vs,
                                &pool,
                            ) {
                                Some((meta, _, block)) => (
                                    meta.slot,
                                    meta.state_root,
                                    meta.total_accounts,
                                    Some(block.header.clone()),
                                    block.commit_round,
                                    block.commit_signatures.clone(),
                                ),
                                None => (0, [0u8; 32], 0, None, 0, Vec::new()),
                            }
                        };
                        P2PMessage::new(
                            MessageType::CheckpointMetaResponse {
                                slot,
                                state_root,
                                total_accounts,
                                checkpoint_header,
                                commit_round,
                                commit_signatures,
                            },
                            local_addr_for_snapshot,
                        )
                    }
                };

                if let Err(e) = peer_mgr_for_snapshot
                    .send_to_peer(&request.requester, response)
                    .await
                {
                    warn!("⚠️  Failed to send snapshot response: {}", e);
                    peer_mgr_for_snapshot.record_violation(&request.requester);
                } else {
                    peer_mgr_for_snapshot.record_success(&request.requester);
                }
            }
        });

        // Start snapshot response handler
        let state_for_snapshot_apply = state.clone();
        let validator_set_for_snapshot_apply = validator_set.clone();
        let stake_pool_for_snapshot_apply = stake_pool.clone();
        let snapshot_sync_for_apply = snapshot_sync.clone();
        let data_dir_for_snapshot_apply = data_dir.clone();
        let peer_mgr_for_snapshot_apply = p2p_pm.clone();
        let local_addr_for_snap_apply = local_addr;
        let sync_mgr_for_snapshot = sync_manager.clone();
        tokio::spawn(async move {
            // Track state snapshot download progress per category
            let mut state_snap_progress: std::collections::HashMap<String, (u64, u64)> =
                std::collections::HashMap::new(); // category -> (received_chunks, total_chunks)
            let mut verified_checkpoint_anchors: std::collections::HashMap<
                std::net::SocketAddr,
                (u64, [u8; 32]),
            > = std::collections::HashMap::new();
            let mut active_snapshot_anchor: Option<(u64, [u8; 32])> = None;

            while let Some(response) = snapshot_response_rx.recv().await {
                // Handle CheckpointMetaResponse
                if let Some((
                    slot,
                    state_root,
                    total_accounts,
                    checkpoint_header,
                    commit_round,
                    commit_signatures,
                )) = response.checkpoint_meta
                {
                    if slot > 0 && total_accounts > 0 {
                        let anchor_verified = {
                            let vs = validator_set_for_snapshot_apply.read().await;
                            let pool = stake_pool_for_snapshot_apply.read().await;
                            verify_checkpoint_anchor(
                                slot,
                                state_root,
                                checkpoint_header.as_ref(),
                                commit_round,
                                &commit_signatures,
                                &vs,
                                &pool,
                            )
                        };
                        if let Err(err) = anchor_verified {
                            warn!(
                                "⚠️  Rejecting checkpoint metadata from {}: {}",
                                response.requester, err
                            );
                            peer_mgr_for_snapshot_apply.record_violation(&response.requester);
                            continue;
                        }
                        verified_checkpoint_anchors.insert(response.requester, (slot, state_root));
                        let support = checkpoint_anchor_support(
                            &verified_checkpoint_anchors,
                            slot,
                            state_root,
                        );
                        info!(
                            "📋 Peer {} has checkpoint at slot {} ({} accounts, {} corroboration{})",
                            response.requester,
                            slot,
                            total_accounts,
                            support,
                            if support == 1 { "" } else { "s" }
                        );
                        let local_slot = state_for_snapshot_apply.get_last_slot().unwrap_or(0);
                        if slot > local_slot + 100 {
                            if support < MIN_WARP_CHECKPOINT_ANCHOR_PEERS {
                                info!(
                                    "⏳ Awaiting corroborated checkpoint anchor before warp snapshot download (have {}, need {})",
                                    support,
                                    MIN_WARP_CHECKPOINT_ANCHOR_PEERS,
                                );
                                continue;
                            }

                            if let Some((active_slot, active_root)) = active_snapshot_anchor {
                                if active_slot != slot || active_root != state_root {
                                    info!(
                                        "⏳ Ignoring alternate checkpoint anchor from {} while snapshot sync is already pinned to slot {}",
                                        response.requester,
                                        active_slot,
                                    );
                                    continue;
                                }
                            }

                            if active_snapshot_anchor.is_some() {
                                continue;
                            }

                            active_snapshot_anchor = Some((slot, state_root));
                            // Peer is significantly ahead — request state snapshot
                            info!(
                                "🔄 Requesting state snapshot from {} after {}-peer checkpoint corroboration (local slot {}, peer slot {})",
                                response.requester,
                                support,
                                local_slot,
                                slot
                            );

                            // P3-1: Send StateSnapshotRequest for each category
                            let chunk_size = 1000u64;
                            for category in &["accounts", "contract_storage", "programs"] {
                                let snap_request = P2PMessage::new(
                                    MessageType::StateSnapshotRequest {
                                        category: category.to_string(),
                                        chunk_index: 0,
                                        chunk_size,
                                    },
                                    local_addr_for_snap_apply,
                                );
                                if let Err(e) = peer_mgr_for_snapshot_apply
                                    .send_to_peer(&response.requester, snap_request)
                                    .await
                                {
                                    warn!(
                                        "⚠️  Failed to send {} snapshot request: {}",
                                        category, e
                                    );
                                }
                            }
                        }
                    } else {
                        verified_checkpoint_anchors.remove(&response.requester);
                        warn!("📋 Peer {} has no checkpoint available", response.requester);
                        // Warp sync is impossible without a checkpoint.  Complete the
                        // current sync batch and switch to HeaderOnly so the next
                        // should_sync() call can re-trigger with block-range requests.
                        let current_mode = sync_mgr_for_snapshot.get_sync_mode().await;
                        let known_peers = peer_mgr_for_snapshot_apply.get_peer_infos().len();
                        if current_mode == crate::sync::SyncMode::Warp
                            && active_snapshot_anchor.is_none()
                            && verified_checkpoint_anchors.is_empty()
                            && known_peers <= 1
                        {
                            sync_mgr_for_snapshot
                                .set_sync_mode(crate::sync::SyncMode::HeaderOnly)
                                .await;
                            sync_mgr_for_snapshot.complete_sync().await;
                            sync_mgr_for_snapshot.record_sync_failure().await;
                        } else if current_mode == crate::sync::SyncMode::Warp {
                            info!(
                                "⏳ Waiting for corroborated checkpoint metadata from other peers before abandoning warp sync"
                            );
                        }
                    }
                    continue;
                }

                // Handle StateSnapshotResponse (chunked state data)
                if let Some((
                    ref category,
                    chunk_index,
                    total_chunks,
                    snapshot_slot,
                    state_root,
                    ref entries_bytes,
                )) = response.state_snapshot_data
                {
                    match verified_checkpoint_anchors.get(&response.requester) {
                        Some((expected_slot, expected_root))
                            if *expected_slot == snapshot_slot && *expected_root == state_root => {}
                        Some((expected_slot, expected_root)) => {
                            warn!(
                                "⚠️  Rejecting {} snapshot chunk from {}: anchor mismatch (expected slot {} root {}, got slot {} root {})",
                                category,
                                response.requester,
                                expected_slot,
                                hex::encode(&expected_root[..8]),
                                snapshot_slot,
                                hex::encode(&state_root[..8]),
                            );
                            peer_mgr_for_snapshot_apply.record_violation(&response.requester);
                            continue;
                        }
                        None => {
                            warn!(
                                "⚠️  Rejecting {} snapshot chunk from {} without a verified checkpoint anchor",
                                category, response.requester
                            );
                            peer_mgr_for_snapshot_apply.record_violation(&response.requester);
                            continue;
                        }
                    }
                    info!(
                        "📥 Received {} snapshot chunk {}/{} from {} (slot {})",
                        category,
                        chunk_index + 1,
                        total_chunks,
                        response.requester,
                        snapshot_slot
                    );

                    // Deserialize and import entries
                    match bincode::deserialize::<Vec<(Vec<u8>, Vec<u8>)>>(entries_bytes) {
                        Ok(entries) => {
                            let import_result = match category.as_str() {
                                "accounts" => state_for_snapshot_apply.import_accounts(&entries),
                                "contract_storage" => {
                                    state_for_snapshot_apply.import_contract_storage(&entries)
                                }
                                "programs" => state_for_snapshot_apply.import_programs(&entries),
                                _ => {
                                    warn!("⚠️  Unknown snapshot category: {}", category);
                                    Ok(0)
                                }
                            };
                            match import_result {
                                Ok(count) => {
                                    info!(
                                        "✅ Imported {} {} entries (chunk {}/{})",
                                        count,
                                        category,
                                        chunk_index + 1,
                                        total_chunks
                                    );
                                }
                                Err(e) => {
                                    warn!("⚠️  Failed to import {} entries: {}", category, e);
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "⚠️  Failed to deserialize {} snapshot chunk: {}",
                                category, e
                            );
                        }
                    }

                    // Track progress
                    let progress = state_snap_progress
                        .entry(category.clone())
                        .or_insert((0, total_chunks));
                    progress.0 = chunk_index + 1;
                    progress.1 = total_chunks;

                    // P3-1: Request the next chunk if there are more
                    if chunk_index + 1 < total_chunks {
                        let next_request = P2PMessage::new(
                            MessageType::StateSnapshotRequest {
                                category: category.clone(),
                                chunk_index: chunk_index + 1,
                                chunk_size: 1000,
                            },
                            local_addr_for_snap_apply,
                        );
                        if let Err(e) = peer_mgr_for_snapshot_apply
                            .send_to_peer(&response.requester, next_request)
                            .await
                        {
                            warn!(
                                "⚠️  Failed to request next {} snapshot chunk: {}",
                                category, e
                            );
                        }
                    }

                    // Check if all categories are complete
                    let accounts_done = state_snap_progress
                        .get("accounts")
                        .map(|(r, t)| r >= t)
                        .unwrap_or(false);
                    let storage_done = state_snap_progress
                        .get("contract_storage")
                        .map(|(r, t)| r >= t)
                        .unwrap_or(false);
                    let programs_done = state_snap_progress
                        .get("programs")
                        .map(|(r, t)| r >= t)
                        .unwrap_or(false);

                    if accounts_done && storage_done && programs_done {
                        info!("✅ State snapshot sync complete — all categories imported");

                        // P3-1: Verify the state root matches what the peer reported.
                        // Recompute our Merkle root from imported accounts and compare
                        // against the snapshot's advertised state_root.
                        let expected_root = state_root;
                        let computed_root =
                            state_for_snapshot_apply.compute_state_root_cold_start();
                        if computed_root.0 == expected_root {
                            info!(
                                "✅ State root verified: {} (matches snapshot)",
                                hex::encode(&computed_root.0[..8])
                            );
                        } else {
                            warn!(
                                "⚠️  State root MISMATCH! Computed {} vs expected {}. \
                                 Snapshot may be corrupted — falling back to header-only sync.",
                                hex::encode(&computed_root.0[..8]),
                                hex::encode(&expected_root[..8]),
                            );
                            // Don't set last_slot — let the node re-sync via blocks.
                            continue;
                        }

                        // Update last_slot to the checkpoint slot
                        if let Err(e) = state_for_snapshot_apply.set_last_slot(snapshot_slot) {
                            warn!(
                                "⚠️  Failed to set last_slot to snapshot slot {}: {}",
                                snapshot_slot, e
                            );
                        }
                        // Create a local checkpoint from the imported state
                        let checkpoint_path = format!(
                            "{}/checkpoints/slot-{}",
                            data_dir_for_snapshot_apply, snapshot_slot
                        );
                        match state_for_snapshot_apply
                            .create_checkpoint(&checkpoint_path, snapshot_slot)
                        {
                            Ok(meta) => info!(
                                "✅ Created local checkpoint at slot {} ({} accounts)",
                                meta.slot, meta.total_accounts
                            ),
                            Err(e) => warn!("⚠️  Failed to create local checkpoint: {}", e),
                        }
                        active_snapshot_anchor = None;
                        state_snap_progress.clear();
                        verified_checkpoint_anchors.clear();
                        verified_checkpoint_anchors.remove(&response.requester);
                    }

                    continue;
                }

                match response.kind {
                    SnapshotKind::ValidatorSet => {
                        if let Some(remote_set) = response.validator_set {
                            if remote_set.validators().is_empty() {
                                warn!(
                                    "⚠️  Ignoring empty validator set snapshot from {}",
                                    response.requester
                                );
                                continue;
                            }

                            let remote_hash = hash_validator_set(&remote_set);

                            let mut vs = validator_set_for_snapshot_apply.write().await;
                            let local_hash = hash_validator_set(&vs);

                            if remote_hash != local_hash {
                                // T2.9 fix: MERGE remote validators into local set
                                // instead of full replacement. This prevents a single
                                // malicious peer from removing legitimate validators.
                                // AUDIT-FIX 2.11: Only UPDATE existing validators from
                                // snapshot (never ADD new ones from a single peer).
                                // New validators must join via the announcement protocol
                                // which verifies signatures and on-chain stake.
                                let mut merged_count = 0u32;
                                for remote_val in remote_set.validators() {
                                    if let Some(local_val) =
                                        vs.get_validator_mut(&remote_val.pubkey)
                                    {
                                        // Keep local blocks_proposed authoritative from locally
                                        // validated canonical blocks. Importing max counters from
                                        // remote snapshots can permanently inflate one validator.
                                        if remote_val.last_active_slot > local_val.last_active_slot
                                        {
                                            local_val.last_active_slot =
                                                remote_val.last_active_slot;
                                            let local_pool_stake = {
                                                let pool =
                                                    stake_pool_for_snapshot_apply.read().await;
                                                load_local_stake_pool_amount(
                                                    &pool,
                                                    &remote_val.pubkey,
                                                )
                                            }
                                            .unwrap_or(0);
                                            local_val.stake = local_pool_stake;
                                        }
                                        merged_count += 1;
                                    } else {
                                        // Unknown validators from peer snapshots are only
                                        // eligible if the local node already has a canonical
                                        // stake-pool entry for them. A plain staked account is
                                        // not enough to confer validator eligibility.
                                        let local_pool_stake = {
                                            let pool = stake_pool_for_snapshot_apply.read().await;
                                            load_local_stake_pool_amount(&pool, &remote_val.pubkey)
                                        };

                                        if let Some(local_pool_stake) = local_pool_stake
                                            .filter(|stake| *stake >= min_validator_stake)
                                        {
                                            // PHANTOM GUARD: Also check slot plausibility
                                            let our_tip = state_for_snapshot_apply
                                                .get_last_slot()
                                                .unwrap_or(0);
                                            let their_slot = remote_val.last_active_slot;
                                            let drift = their_slot.abs_diff(our_tip);
                                            if our_tip > 10 && drift > 500 {
                                                warn!(
                                                    "⚠️  Snapshot: rejecting validator {} from {} — slot drift {} too large (ours={}, theirs={})",
                                                    remote_val.pubkey.to_base58(),
                                                    response.requester,
                                                    drift, our_tip, their_slot
                                                );
                                                continue;
                                            }
                                            // Do not trust remote pending_activation;
                                            // newly imported validators must be activated
                                            // locally at the next height boundary from the
                                            // locally frozen stake pool.
                                            let new_val = ValidatorInfo {
                                                pubkey: remote_val.pubkey,
                                                reputation: 100,
                                                blocks_proposed: remote_val.blocks_proposed,
                                                votes_cast: remote_val.votes_cast,
                                                correct_votes: remote_val.correct_votes,
                                                stake: local_pool_stake,
                                                joined_slot: remote_val.joined_slot,
                                                last_active_slot: remote_val.last_active_slot,
                                                commission_rate: 500,
                                                transactions_processed: 0,
                                                pending_activation: our_tip > 0,
                                            };
                                            vs.add_validator(new_val);
                                            merged_count += 1;
                                            info!(
                                                "✅ Snapshot: added locally verified validator {} from peer {} (stake-pool entry confirmed)",
                                                remote_val.pubkey.to_base58(),
                                                response.requester
                                            );
                                        } else {
                                            // No local stake-pool entry — reject (prevents
                                            // phantom validator admission from peer snapshots).
                                            warn!(
                                                "⚠️  Snapshot: rejecting unverified validator {} from peer {} (no local stake-pool entry)",
                                                hex::encode(remote_val.pubkey.0),
                                                response.requester
                                            );
                                        }
                                    }
                                }
                                let merged_set = vs.clone();
                                // Save while still holding the lock to prevent
                                // apply_block_effects from saving a newer version
                                // that we'd then overwrite with this stale clone.
                                if let Err(e) =
                                    state_for_snapshot_apply.save_validator_set(&merged_set)
                                {
                                    warn!("⚠️  Failed to persist merged validator set: {}", e);
                                } else {
                                    info!(
                                        "✅ Merged validator set snapshot from {} ({} entries merged)",
                                        response.requester,
                                        merged_count
                                    );
                                    // Only mark ready if the merged set is non-empty.
                                    // Before genesis processing, on-chain stake is zero
                                    // so the merge rejects all validators — the retry
                                    // task must keep retrying until genesis state exists.
                                    if !vs.validators().is_empty() {
                                        snapshot_sync_for_apply.lock().await.validator_set = true;
                                    } else {
                                        warn!("⚠️  Validator set merge produced empty set — snapshot not ready (genesis may not be applied yet)");
                                    }
                                }
                                drop(vs);
                            } else {
                                // Hashes match — local set is already correct (from block replay).
                                // Only mark ready if the local set is non-empty.
                                if !vs.validators().is_empty() {
                                    snapshot_sync_for_apply.lock().await.validator_set = true;
                                }
                                drop(vs);
                            }
                        }
                    }
                    SnapshotKind::StakePool => {
                        if let Some(remote_pool) = response.stake_pool {
                            if remote_pool.stake_entries().is_empty() {
                                warn!(
                                    "⚠️  Ignoring empty stake pool snapshot from {}",
                                    response.requester
                                );
                                continue;
                            }

                            // MERGE remote entries into local pool (full-fidelity)
                            let mut pool = stake_pool_for_snapshot_apply.write().await;
                            let local_hash = hash_stake_pool(&pool);
                            let mut merged_count = 0u32;
                            for entry in remote_pool.stake_entries() {
                                let entry_validator = entry.validator;
                                let entry_amount = entry.amount;
                                let existing = pool.get_stake(&entry_validator);
                                let should_upsert = match existing {
                                    None => true,
                                    Some(local_entry) => {
                                        entry_amount > local_entry.amount
                                            || entry.total_debt_repaid
                                                > local_entry.total_debt_repaid
                                            || (local_entry.bootstrap_index == u64::MAX
                                                && entry.bootstrap_index != u64::MAX)
                                    }
                                };
                                if should_upsert {
                                    // GUARD: Only upsert stake entries for validators
                                    // that exist in our local validator set AND have
                                    // confirmed on-chain stake. This prevents a joining
                                    // node (pre-registration) from contaminating the
                                    // producing node's pool and breaking solo BFT.
                                    let vs = validator_set_for_snapshot_apply.read().await;
                                    let is_known_validator =
                                        vs.get_validator(&entry_validator).is_some();
                                    drop(vs);
                                    if !is_known_validator {
                                        warn!(
                                            "⚠️  Snapshot: skipping stake entry for unknown validator {} from peer {} (not in local validator set)",
                                            entry.validator.to_base58(),
                                            response.requester
                                        );
                                        continue;
                                    }

                                    // Verify on-chain stake before accepting entry
                                    let Some(local_account_stake) = load_local_account_stake(
                                        &state_for_snapshot_apply,
                                        &entry_validator,
                                    )
                                    .filter(|stake| *stake >= min_validator_stake) else {
                                        debug!(
                                            "Snapshot: skipping stake entry for {} from {} (no on-chain stake)",
                                            entry.validator.to_base58(),
                                            response.requester
                                        );
                                        continue;
                                    };

                                    let mut sanitized_entry = entry.clone();
                                    sanitized_entry.amount = local_account_stake;
                                    if let Some(local_entry) = pool.get_stake(&entry_validator) {
                                        if local_entry.amount != local_account_stake {
                                            pool.upsert_stake(
                                                entry_validator,
                                                local_account_stake,
                                                0,
                                            );
                                        }
                                    }
                                    pool.upsert_stake_full(sanitized_entry);
                                    merged_count += 1;
                                    // NOTE: No bootstrap account creation here.
                                    // Validator accounts are created through consensus
                                    // via the RegisterValidator instruction (opcode 26).
                                    // Stake pool entries synced here reflect on-chain state
                                    // that was already processed through block consensus.
                                }
                            }
                            let merged_hash = hash_stake_pool(&pool);
                            if merged_hash != local_hash {
                                let merged_pool = pool.clone();
                                drop(pool);
                                if let Err(e) =
                                    state_for_snapshot_apply.put_stake_pool(&merged_pool)
                                {
                                    warn!("⚠️  Failed to persist merged stake pool: {}", e);
                                } else {
                                    info!(
                                        "✅ Merged {} stake entries from {} ({} -> {})",
                                        merged_count,
                                        response.requester,
                                        local_hash.to_hex(),
                                        merged_hash.to_hex()
                                    );
                                    // Only mark ready if the merged pool is non-empty.
                                    if !merged_pool.stake_entries().is_empty() {
                                        snapshot_sync_for_apply.lock().await.stake_pool = true;
                                        activate_pending_validators_for_height(
                                            &state_for_snapshot_apply,
                                            &validator_set_for_snapshot_apply,
                                            &merged_pool,
                                            state_for_snapshot_apply.get_last_slot().unwrap_or(0),
                                            min_validator_stake,
                                        )
                                        .await;
                                    } else {
                                        warn!("⚠️  Stake pool merge produced empty pool — snapshot not ready");
                                    }
                                }
                            } else {
                                // Hashes match — local pool is already correct (from block replay).
                                // Only mark ready if the local pool is non-empty.
                                let pool_non_empty = !pool.stake_entries().is_empty();
                                let snapshot_pool = pool.clone();
                                drop(pool);
                                if pool_non_empty {
                                    snapshot_sync_for_apply.lock().await.stake_pool = true;
                                    activate_pending_validators_for_height(
                                        &state_for_snapshot_apply,
                                        &validator_set_for_snapshot_apply,
                                        &snapshot_pool,
                                        state_for_snapshot_apply.get_last_slot().unwrap_or(0),
                                        min_validator_stake,
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                    SnapshotKind::StateCheckpoint => {
                        // Handled above via checkpoint_meta / state_snapshot_data fields
                        // This arm handles a generic StateCheckpoint response via SnapshotResponse
                        info!(
                            "📋 Received StateCheckpoint snapshot response from {}",
                            response.requester
                        );
                    }
                }
            }
        });
    }

    // RPC SERVER SETUP
    info!("🦞 Starting RPC server...");

    // Parse --rpc-port and --ws-port from CLI, or derive from P2P port
    // Port auto-derivation matches run-validator.sh exactly:
    //   Testnet V1 (p2p 7001): rpc=8899, ws=8900
    //   Testnet V2 (p2p 7002): rpc=8901, ws=8902
    //   Mainnet V1 (p2p 8001): rpc=9899, ws=9900
    //   Mainnet V2 (p2p 8002): rpc=9901, ws=9902
    // Formula: offset = p2p_port - base_p2p, rpc = base_rpc + 2*offset
    let rpc_port = get_flag_value(&args, &["--rpc-port"])
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| {
            let base_p2p = if p2p_port >= 8000 { 8001u16 } else { 7001u16 };
            let base_rpc = if p2p_port >= 8000 { 9899u16 } else { 8899u16 };
            let offset = p2p_port.saturating_sub(base_p2p);
            base_rpc.saturating_add(offset.saturating_mul(2))
        });

    let ws_port = get_flag_value(&args, &["--ws-port"])
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| {
            let base_p2p = if p2p_port >= 8000 { 8001u16 } else { 7001u16 };
            let base_ws = if p2p_port >= 8000 { 9900u16 } else { 8900u16 };
            let offset = p2p_port.saturating_sub(base_p2p);
            base_ws.saturating_add(offset.saturating_mul(2))
        });

    // Parse --admin-token from CLI or LICHEN_ADMIN_TOKEN env var
    let admin_token: Option<String> = get_flag_value(&args, &["--admin-token"])
        .map(|s| s.to_string())
        .or_else(|| env::var("LICHEN_ADMIN_TOKEN").ok())
        .filter(|t| !t.is_empty());
    if admin_token.is_some() {
        info!("🔒 Admin token configured for state-mutating RPC endpoints");
    } else {
        warn!(
            "⚠️  No admin_token configured — all admin RPC endpoints are disabled. \
               Set LICHEN_ADMIN_TOKEN env var or --admin-token flag for production."
        );
    }

    let state_for_rpc = state.clone();
    let state_for_ws = state.clone();
    let stake_pool_for_rpc = Some(stake_pool.clone());
    let chain_id_for_rpc = genesis_config.chain_id.clone();
    let network_id_for_rpc = genesis_config.chain_id.clone();

    // Create transaction submission channel for RPC -> mempool (bounded: backpressure returns HTTP 503)
    let (rpc_tx_sender, mut rpc_tx_receiver) = mpsc::channel::<Transaction>(50_000);

    // Forward RPC transactions to P2P network and mempool
    let mempool_for_rpc_txs = mempool.clone();
    let p2p_peer_manager_for_txs = p2p_peer_manager.clone();
    let p2p_config_for_txs = p2p_config.clone();
    tokio::spawn(async move {
        while let Some(tx) = rpc_tx_receiver.recv().await {
            info!("📨 RPC transaction received, adding to mempool");

            // P9-RPC-01: Defense-in-depth — reject sentinel blockhash for non-EVM TXs
            // before they even enter the mempool.  Only eth_sendRawTransaction may
            // submit TXs with the EVM sentinel; any other path is a bypass attempt.
            if tx.message.recent_blockhash == lichen_core::Hash([0xEE; 32]) {
                let is_evm = tx
                    .message
                    .instructions
                    .first()
                    .map(|ix| ix.program_id == EVM_PROGRAM_ID)
                    .unwrap_or(false);
                if !is_evm {
                    info!("❌ RPC transaction rejected: non-EVM TX with EVM sentinel blockhash");
                    continue;
                }
            }

            // Validate structure before adding to mempool
            if let Err(e) = tx.validate_structure() {
                info!("❌ RPC transaction rejected: {}", e);
                continue;
            }

            // M-8 FIX: No reputation lookup needed — mempool uses fee-only ordering.
            let reputation = 0u64;

            // Add to mempool
            {
                let fee_config = FeeConfig::default_from_constants();
                let computed_fee = TxProcessor::compute_transaction_fee(&tx, &fee_config);
                let mut pool = mempool_for_rpc_txs.lock().await;
                if let Err(e) = pool.add_transaction(tx.clone(), computed_fee, reputation) {
                    info!("Mempool add failed: {}", e);
                }
            }

            // Broadcast to P2P network
            if let Some(ref peer_mgr) = p2p_peer_manager_for_txs {
                let target_id = tx.hash().0;
                let msg = lichen_p2p::P2PMessage::new(
                    lichen_p2p::MessageType::Transaction(tx),
                    p2p_config_for_txs.listen_addr,
                );
                peer_mgr
                    .route_to_closest(&target_id, lichen_p2p::NON_CONSENSUS_FANOUT, msg)
                    .await;
                info!("📡 Broadcasted transaction to network");
            }
        }
    });

    let tx_sender_for_rpc = Some(rpc_tx_sender);
    let p2p_for_rpc: Option<Arc<dyn lichen_rpc::P2PNetworkTrait>> =
        p2p_peer_manager.as_ref().map(|peer_mgr| {
            struct PeerAdapter {
                peer_mgr: Arc<lichen_p2p::PeerManager>,
            }

            impl lichen_rpc::P2PNetworkTrait for PeerAdapter {
                fn peer_count(&self) -> usize {
                    self.peer_mgr.get_peers().len()
                }

                fn peer_addresses(&self) -> Vec<String> {
                    self.peer_mgr
                        .get_peers()
                        .into_iter()
                        .map(|addr| addr.to_string())
                        .collect()
                }
            }

            Arc::new(PeerAdapter {
                peer_mgr: peer_mgr.clone(),
            }) as Arc<dyn lichen_rpc::P2PNetworkTrait>
        });

    // Start WebSocket server FIRST so we can share its broadcasters with RPC
    let (ws_event_tx, ws_dex_broadcaster, ws_prediction_broadcaster, _ws_handle) =
        match lichen_rpc::start_ws_server(state_for_ws, ws_port, Some(finality_tracker.clone()))
            .await
        {
            Ok(result) => {
                info!("✅ WebSocket server starting on ws://0.0.0.0:{}", ws_port);
                result
            }
            Err(e) => {
                error!(
                    "Failed to start WebSocket server: {} — continuing without WebSocket",
                    e
                );
                // Create a dummy broadcast channel so the rest of the code can send events
                // without checking — receivers simply don't exist.
                let (dummy_tx, _) = tokio::sync::broadcast::channel::<lichen_rpc::ws::Event>(1);
                let dummy_broadcaster =
                    std::sync::Arc::new(lichen_rpc::dex_ws::DexEventBroadcaster::new(1));
                let dummy_pred =
                    std::sync::Arc::new(lichen_rpc::ws::PredictionEventBroadcaster::new(1));
                let dummy_handle = tokio::spawn(async {});
                (dummy_tx, dummy_broadcaster, dummy_pred, dummy_handle)
            }
        };

    // Start RPC server — share the WS broadcasters so REST emits reach WS subscribers
    let finality_for_rpc = Some(finality_tracker.clone());
    let dex_bc_for_rpc = ws_dex_broadcaster.clone();
    let pred_bc_for_rpc = ws_prediction_broadcaster.clone();
    tokio::spawn(async move {
        if let Err(e) = start_rpc_server(
            state_for_rpc,
            rpc_port,
            tx_sender_for_rpc,
            stake_pool_for_rpc,
            p2p_for_rpc,
            chain_id_for_rpc,
            network_id_for_rpc,
            min_validator_stake,
            admin_token,
            finality_for_rpc,
            Some(dex_bc_for_rpc),
            Some(pred_bc_for_rpc),
            treasury_keypair,
        )
        .await
        {
            error!("RPC server error: {}", e);
        }
    });
    info!("✅ RPC server starting on http://0.0.0.0:{}", rpc_port);

    // Start the oracle price feeder background task
    // Connects to Binance WebSocket (aggTrade) for real-time wSOL/wETH prices
    // and submits signed native oracle-attestation transactions.
    // Auto-reconnects with exponential backoff; falls back to REST API if WS is down.
    // Can be disabled via LICHEN_DISABLE_ORACLE=1 (e.g. if Binance is geo-blocked).
    // Create shared oracle prices — the feeder converts these observations
    // into native oracle-attestation transactions.
    let shared_oracle_prices = SharedOraclePrices::new();

    let oracle_disabled = std::env::var("LICHEN_DISABLE_ORACLE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if oracle_disabled {
        info!("🔮 Oracle price feeder disabled via LICHEN_DISABLE_ORACLE");
    } else {
        let state_for_oracle = state.clone();
        spawn_oracle_price_feeder(
            state_for_oracle,
            shared_oracle_prices.clone(),
            ws_dex_broadcaster.clone(),
            OracleFeedTxContext {
                mempool: mempool.clone(),
                p2p_peer_manager: p2p_peer_manager.clone(),
                p2p_config: p2p_config.clone(),
                validator_seed: validator_keypair.to_seed(),
                validator_pubkey,
            },
        );
    }

    info!("⚡ Starting consensus-based block production");
    info!("Validator: {}", validator_pubkey);
    info!(
        "Block time: {}ms",
        genesis_config.consensus.slot_duration_ms
    );
    info!(
        "Base fee: {} spores ({:.5} LICN)",
        BASE_FEE,
        BASE_FEE as f64 / 1_000_000_000.0
    );
    // AUDIT-FIX 3.13: Log actual fee config values, not hardcoded percentages
    let genesis_fee_info = format!(
        "Fee split: {}% burned, {}% producer, {}% voters, {}% treasury",
        genesis_config.features.fee_burn_percentage,
        genesis_config.features.fee_producer_percentage,
        genesis_config.features.fee_voters_percentage,
        100u64.saturating_sub(
            genesis_config.features.fee_burn_percentage
                + genesis_config.features.fee_producer_percentage
                + genesis_config.features.fee_voters_percentage,
        ),
    );
    info!("{}", genesis_fee_info);
    info!("Leader selection: stake + contribution weighted");

    if let Some(ref p2p_pm) = p2p_peer_manager {
        info!("🌐 Multi-validator mode: Broadcasting blocks to peers");

        // Broadcast validator announcement periodically for network discovery
        let peer_mgr_for_announce = p2p_pm.clone();
        let local_addr = p2p_config.listen_addr;
        let validator_pubkey_for_announce = validator_pubkey;
        let stake_pool_for_announce = stake_pool.clone();
        let state_for_announce = state.clone();
        let validator_seed_for_announce = validator_keypair.to_seed();
        let machine_fingerprint_for_announce = machine_fingerprint;
        tokio::spawn(async move {
            // Wait for initial peer connections
            time::sleep(Duration::from_secs(2)).await;

            // Announce periodically so new validators can discover us
            let mut interval = time::interval(Duration::from_secs(10));
            loop {
                let validator_stake = {
                    let pool = stake_pool_for_announce.read().await;
                    pool.get_stake(&validator_pubkey_for_announce)
                        .map(|s| s.total_stake())
                        .unwrap_or(0)
                };
                let current_slot = state_for_announce.get_last_slot().unwrap_or(0);

                // T2.3 fix: Sign announcement with validator keypair
                let announce_keypair = Keypair::from_seed(&validator_seed_for_announce);
                let sign_message = validator_announcement_signing_message(
                    &validator_pubkey_for_announce,
                    validator_stake,
                    current_slot,
                    &machine_fingerprint_for_announce,
                    Some(updater::VERSION),
                )
                .expect("validator version should always produce a valid announcement payload");
                let signature = announce_keypair.sign(&sign_message);

                let announce_msg = P2PMessage::new(
                    MessageType::ValidatorAnnounce {
                        pubkey: validator_pubkey_for_announce,
                        stake: validator_stake,
                        current_slot,
                        version: updater::VERSION.to_string(),
                        signature,
                        machine_fingerprint: machine_fingerprint_for_announce,
                    },
                    local_addr,
                );

                interval.tick().await;

                peer_mgr_for_announce.broadcast(announce_msg).await;
                info!(
                    "📣 Broadcasted signed validator announcement: {} (stake: {}, slot: {})",
                    validator_pubkey_for_announce.to_base58(),
                    validator_stake,
                    current_slot
                );
            }
        });

        // Proactive ping loop — send Ping to all peers every 5s for real-time liveness
        let peer_mgr_for_ping = p2p_pm.clone();
        let local_addr_for_ping = p2p_config.listen_addr;
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                let ping_msg = P2PMessage::new(MessageType::Ping, local_addr_for_ping);
                peer_mgr_for_ping.broadcast(ping_msg).await;
            }
        });

        // Broadcast consistency report periodically
        let peer_mgr_for_report = p2p_pm.clone();
        let local_addr_for_report = p2p_config.listen_addr;
        let validator_set_for_report = validator_set.clone();
        let stake_pool_for_report = stake_pool.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                let vs = validator_set_for_report.read().await;
                let pool = stake_pool_for_report.read().await;
                let vs_hash = hash_validator_set(&vs);
                let pool_hash = hash_stake_pool(&pool);
                drop(pool);
                drop(vs);

                let report = P2PMessage::new(
                    MessageType::ConsistencyReport {
                        validator_set_hash: vs_hash,
                        stake_pool_hash: pool_hash,
                    },
                    local_addr_for_report,
                );
                peer_mgr_for_report.broadcast(report).await;
            }
        });
    } else {
        info!("🔒 Single-validator mode: No P2P network");
    }

    // Periodic mempool cleanup (expired + stale blockhash eviction)
    let mempool_for_cleanup = mempool.clone();
    let state_for_mempool_cleanup = state.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let mut pool = mempool_for_cleanup.lock().await;
            pool.cleanup_expired();
            // Prune transactions referencing blockhashes older than MAX_TX_AGE_BLOCKS slots
            if let Ok(valid_hashes) =
                state_for_mempool_cleanup.get_recent_blockhashes(MAX_TX_AGE_BLOCKS)
            {
                let evicted = pool.prune_stale_blockhashes(&valid_hashes);
                if evicted > 0 {
                    warn!("🧹 Mempool pruned {} stale-blockhash transactions", evicted);
                }
            }
            info!("🧹 Mempool cleaned (size: {})", pool.size());
        }
    });

    // Periodic vote aggregator cleanup (keep last 100 slots)
    let vote_agg_for_cleanup = vote_aggregator.clone();
    let state_for_vote_cleanup = state.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let current_slot = state_for_vote_cleanup.get_last_slot().unwrap_or(0);
            let mut agg = vote_agg_for_cleanup.write().await;
            agg.prune_old_votes(current_slot, 100);
        }
    });

    // ── Stale validator cleanup ──
    // Two-tier cleanup:
    //   1. Never-active ghosts (0 blocks, 0 activity): removed after 500 slots
    //   2. Previously-active validators: removed after 50 epochs of inactivity
    {
        let vs_for_cleanup = validator_set.clone();
        let state_for_vs_cleanup = state.clone();
        let own_pubkey = validator_pubkey;
        tokio::spawn(async move {
            // Tier 2: Long-term stale threshold (50 epochs)
            const STALE_EPOCH_THRESHOLD: u64 = 50;
            let stale_slot_threshold = STALE_EPOCH_THRESHOLD * SLOTS_PER_EPOCH;
            // Tier 1: Never-active ghost threshold (500 slots ≈ 200 seconds)
            const GHOST_SLOT_THRESHOLD: u64 = 500;

            let mut interval = time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                let current_slot = state_for_vs_cleanup.get_last_slot().unwrap_or(0);

                // Don't prune during early bootstrap (first 500 slots)
                if current_slot < 500 {
                    continue;
                }

                let mut vs = vs_for_cleanup.write().await;
                let stale_cutoff = current_slot.saturating_sub(stale_slot_threshold);
                let ghost_cutoff = current_slot.saturating_sub(GHOST_SLOT_THRESHOLD);

                // Find stale validators (never remove ourselves)
                let stale: Vec<Pubkey> = vs
                    .validators()
                    .iter()
                    .filter(|v| {
                        if v.pubkey == own_pubkey {
                            return false;
                        }
                        // Tier 1: Never-active ghost — fast cleanup
                        if v.blocks_proposed == 0
                            && v.last_active_slot == 0
                            && v.joined_slot < ghost_cutoff
                        {
                            return true;
                        }
                        // Tier 2: Previously active but long-stale
                        v.last_active_slot < stale_cutoff
                            && v.blocks_proposed == 0
                            && v.joined_slot < stale_cutoff
                    })
                    .map(|v| v.pubkey)
                    .collect();

                for pubkey in &stale {
                    vs.remove_validator(pubkey);
                    info!(
                        "🧹 Removed stale validator {} (inactive since slot < {})",
                        pubkey.to_base58(),
                        stale_cutoff
                    );
                }

                if !stale.is_empty() {
                    if let Err(e) = state_for_vs_cleanup.save_validator_set(&vs) {
                        warn!("⚠️  Failed to save validator set after cleanup: {}", e);
                    }
                }
            }
        });
    }

    // ── P2-3: Periodic cold storage migration ──
    // Every 5 minutes, migrate blocks older than COLD_RETENTION_SLOTS to cold DB.
    if state.has_cold_storage() {
        let state_for_cold = state.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(300));
            loop {
                interval.tick().await;
                let current_slot = state_for_cold.get_last_slot().unwrap_or(0);
                let retain = lichen_core::state::COLD_RETENTION_SLOTS;
                if current_slot > retain {
                    let cutoff = current_slot - retain;
                    match state_for_cold.migrate_to_cold(cutoff) {
                        Ok(0) => {} // nothing to migrate
                        Ok(n) => {
                            info!(
                                "🗄️  Cold migration: moved {} blocks (cutoff slot {})",
                                n, cutoff
                            );
                        }
                        Err(e) => {
                            warn!("🗄️  Cold migration error: {}", e);
                        }
                    }
                }
            }
        });
    }

    // Periodic validator set + stake pool reconciliation from state
    let validator_set_for_reconcile = validator_set.clone();
    let stake_pool_for_reconcile = stake_pool.clone();
    let state_for_reconcile = state.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Ok(loaded_set) = state_for_reconcile.load_validator_set() {
                let mut vs = validator_set_for_reconcile.write().await;
                if hash_validator_set(&vs) != hash_validator_set(&loaded_set) {
                    *vs = loaded_set;
                    info!("🔄 Validator set reconciled from state");
                }
            }

            if let Ok(loaded_pool) = state_for_reconcile.get_stake_pool() {
                let mut pool = stake_pool_for_reconcile.write().await;
                if hash_stake_pool(&pool) != hash_stake_pool(&loaded_pool) {
                    // Full-fidelity merge from disk (includes bootstrap debt, vesting, etc.)
                    for entry in loaded_pool.stake_entries() {
                        let existing = pool.get_stake(&entry.validator);
                        let should_upsert = match existing {
                            None => true,
                            Some(local) => {
                                entry.amount > local.amount
                                    || entry.total_debt_repaid > local.total_debt_repaid
                                    || (local.bootstrap_index == u64::MAX
                                        && entry.bootstrap_index != u64::MAX)
                            }
                        };
                        if should_upsert {
                            pool.upsert_stake_full(entry);
                        }
                    }
                    info!("🔄 Stake pool reconciled from state");
                }
            }
        }
    });

    // Periodic reward stats reporting (every 120s)
    let stake_pool_for_rewards = stake_pool.clone();
    let validator_pubkey_for_rewards = validator_pubkey;

    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(120));
        loop {
            interval.tick().await;

            let pool = stake_pool_for_rewards.read().await;

            // Check accumulated rewards
            if let Some(stake_info) = pool.get_stake(&validator_pubkey_for_rewards) {
                let unclaimed = stake_info.rewards_earned;
                if unclaimed > 0 {
                    let vesting_progress = stake_info.vesting_progress();
                    let is_bootstrapping = !stake_info.is_fully_vested();

                    info!(
                        "💰 Accumulated rewards: {:.3} LICN (unclaimed)",
                        unclaimed as f64 / 1_000_000_000.0
                    );

                    if is_bootstrapping {
                        info!(
                            "🦞 Contributory Stake: {}% vested ({} blocks produced)",
                            vesting_progress, stake_info.blocks_produced
                        );
                    }
                }
            }

            // Report staking statistics
            let stats = pool.get_stats();
            info!(
                "📊 Staking Stats | Total: {:.2} LICN | Validators: {} | Unclaimed: {:.3} LICN",
                stats.total_staked as f64 / 1_000_000_000.0,
                stats.active_validators,
                stats.total_unclaimed_rewards as f64 / 1_000_000_000.0
            );

            drop(pool);
        }
    });

    // Periodic ban list cleanup
    if let Some(ref peer_mgr) = p2p_peer_manager {
        let peer_mgr_for_ban = peer_mgr.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(120));
            loop {
                interval.tick().await;
                peer_mgr_for_ban.prune_ban_list();
            }
        });
    }

    // Periodic downtime MONITORING (reputation impact only — NO slashing).
    //
    // DESIGN RATIONALE (matching Solana/Ethereum approach):
    // Real blockchains do NOT slash validators for downtime:
    //   - Solana: No slashing at all; offline validators simply miss rewards
    //   - Ethereum: Slashing ONLY for provable double-signing/attestation
    //   - Cosmos: Jailing (temporary removal) for downtime, NOT stake destruction
    //
    // Downtime is normal operational behavior (network issues, upgrades, restarts).
    // Slashing should be reserved for provably malicious Byzantine faults:
    // double-block production, double-voting, or invalid state transitions.
    //
    // This monitor:
    //   1. Tracks which validators are behind on last_active_slot
    //   2. Applies a small REPUTATION penalty (not stake) — reducing reward share
    //   3. Logs warnings for operational visibility
    //   4. Does NOT create SlashingEvidence, does NOT broadcast, does NOT slash
    let validator_set_for_downtime = validator_set.clone();
    let state_for_downtime = state.clone();
    let validator_pubkey_for_downtime = validator_pubkey;

    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(120));
        loop {
            interval.tick().await;
            let current_slot = state_for_downtime.get_last_slot().unwrap_or(0);

            let mut vs = validator_set_for_downtime.write().await;
            let num_validators = vs.validators().len() as u64;
            // A validator is "behind" if it hasn't been active for 500+ slots (~200s)
            let downtime_threshold = (200 * num_validators).max(1000);
            // Eviction threshold: 5000 slots (~33 min) of total inactivity
            let eviction_threshold: u64 = 5000;

            // Collect validators to evict (can't mutate + remove in same loop)
            let mut to_evict: Vec<(Pubkey, u64)> = Vec::new();

            for validator_info in vs.validators_mut() {
                if validator_info.pubkey == validator_pubkey_for_downtime {
                    continue; // Don't monitor ourselves
                }

                let missed_slots = current_slot.saturating_sub(validator_info.last_active_slot);
                let slots_since_join = current_slot.saturating_sub(validator_info.joined_slot);

                // Grace period for new validators (2000 slots ≈ 800s)
                if slots_since_join < 2000 {
                    continue;
                }

                // Evict validators that have been offline for 5000+ slots
                if missed_slots >= eviction_threshold {
                    to_evict.push((validator_info.pubkey, missed_slots));
                    continue;
                }

                if missed_slots >= downtime_threshold {
                    let rep_penalty = ((missed_slots / 500) as u64).min(5);
                    let old_rep = validator_info.reputation;
                    validator_info.reputation = validator_info
                        .reputation
                        .saturating_sub(rep_penalty)
                        .max(50);

                    if rep_penalty > 0 {
                        warn!(
                            "⏸️  Validator {} behind by {} slots — reputation {} → {} (no slashing)",
                            validator_info.pubkey.to_base58(),
                            missed_slots,
                            old_rep,
                            validator_info.reputation
                        );
                    }
                }
            }

            // Evict offline validators
            for (pubkey, missed) in &to_evict {
                warn!(
                    "🗑️  Evicting offline validator {} — inactive for {} slots",
                    pubkey.to_base58(),
                    missed
                );
                vs.remove_validator(pubkey);
            }

            let vs_snapshot = vs.clone();
            drop(vs);
            let _ = state_for_downtime.save_validator_set(&vs_snapshot);
        }
    });

    // P3-3: Compact block receiver — reconstruct full blocks from mempool
    {
        let mempool_for_compact = mempool.clone();
        let peer_mgr_for_compact = p2p_peer_manager.clone();
        let local_addr_for_compact = p2p_config.listen_addr;
        let block_tx_for_compact_task = block_tx_for_compact;
        tokio::spawn(async move {
            while let Some(msg) = compact_block_rx.recv().await {
                let cb = msg.compact_block;
                let sender = msg.sender;
                let slot = cb.header.slot;
                debug!(
                    "📦 Compact block slot {} from {} ({} short IDs)",
                    slot,
                    sender,
                    cb.short_ids.len()
                );

                // Attempt reconstruction from mempool
                let pool = mempool_for_compact.lock().await;
                let mut reconstructed_txs: Vec<Option<Transaction>> =
                    Vec::with_capacity(cb.short_ids.len());
                let mut missing_hashes: Vec<Hash> = Vec::new();

                // Build a lookup: short_id → (full_hash, Transaction)
                // Iterate mempool once to match short IDs
                let all_mempool_txs: Vec<(Hash, Transaction)> = pool
                    .all_transactions()
                    .into_iter()
                    .map(|tx| (tx.hash(), tx))
                    .collect();

                for short_id in &cb.short_ids {
                    let mut found = false;
                    for (hash, tx) in &all_mempool_txs {
                        if hash.0[..8] == *short_id {
                            reconstructed_txs.push(Some(tx.clone()));
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        reconstructed_txs.push(None);
                        // We don't know the full hash from just the short ID, so we
                        // need to request the full block for missing TXs.
                        // Use a sentinel hash with the short_id prefix for matching.
                        let mut sentinel = [0u8; 32];
                        sentinel[..8].copy_from_slice(short_id);
                        missing_hashes.push(Hash(sentinel));
                    }
                }
                drop(pool);

                if missing_hashes.is_empty() {
                    // Full reconstruction succeeded
                    // AUDIT-FIX C-8: Avoid unwrap() crash — gracefully skip
                    // block if any tx is unexpectedly None.
                    let transactions: Vec<Transaction> =
                        match reconstructed_txs.into_iter().collect::<Option<Vec<_>>>() {
                            Some(txs) => txs,
                            None => {
                                warn!(
                                "📦 Compact block slot {} reconstruction had unexpected None tx",
                                cb.header.slot
                            );
                                continue;
                            }
                        };

                    // AUDIT-FIX H1: Verify tx_root to guard against short-ID collision.
                    // Recompute tx_root from reconstructed transactions and compare
                    // against the header's tx_root to detect any collision-based mismatch.
                    let mut tx_hash_data = Vec::with_capacity(transactions.len() * 32);
                    for tx in &transactions {
                        tx_hash_data.extend_from_slice(&tx.hash().0);
                    }
                    let reconstructed_tx_root = if transactions.is_empty() {
                        Hash::default()
                    } else {
                        Hash::hash(&tx_hash_data)
                    };
                    if reconstructed_tx_root != cb.header.tx_root {
                        warn!(
                            "📦 Compact block slot {} tx_root mismatch after reconstruction — \
                             short-ID collision detected, requesting full block from {}",
                            slot, sender
                        );
                        // Fall through to request full block
                        if let Some(ref pm) = peer_mgr_for_compact {
                            let request = lichen_p2p::P2PMessage::new(
                                lichen_p2p::MessageType::BlockRequest { slot },
                                pm.local_addr(),
                            );
                            let pm2 = pm.clone();
                            tokio::spawn(async move {
                                if let Err(e) = pm2.send_to_peer(&sender, request).await {
                                    warn!(
                                        "P2P: Failed to request full block from {}: {}",
                                        sender, e
                                    );
                                }
                            });
                        }
                    } else {
                        let block = Block {
                            header: cb.header,
                            transactions,
                            tx_fees_paid: cb.tx_fees_paid,
                            oracle_prices: cb.oracle_prices,
                            commit_round: cb.commit_round,
                            commit_signatures: cb.commit_signatures,
                        };
                        info!(
                            "📦 Compact block slot {} fully reconstructed from mempool ({} txs)",
                            slot,
                            block.transactions.len()
                        );
                        if let Err(e) = block_tx_for_compact_task.try_send(block) {
                            warn!(
                                "P2P: Compact block channel full after reconstruction ({})",
                                e
                            );
                        }
                    }
                } else {
                    // Request missing transactions from the sender
                    info!(
                        "📦 Compact block slot {} missing {} txs, requesting from {}",
                        slot,
                        missing_hashes.len(),
                        sender
                    );
                    if let Some(ref pm) = peer_mgr_for_compact {
                        let request = lichen_p2p::P2PMessage::new(
                            lichen_p2p::MessageType::GetBlockTxs {
                                slot,
                                missing_hashes,
                            },
                            local_addr_for_compact,
                        );
                        let pm = pm.clone();
                        tokio::spawn(async move {
                            if let Err(e) = pm.send_to_peer(&sender, request).await {
                                warn!("P2P: Failed to request missing txs from {}: {}", sender, e);
                            }
                        });
                    }
                }
            }
        });
    }

    // P3-3: Handle GetBlockTxs requests — send back requested transactions
    {
        let state_for_get_txs = state.clone();
        let peer_mgr_for_get_txs = p2p_peer_manager.clone();
        let local_addr_for_get_txs = p2p_config.listen_addr;
        tokio::spawn(async move {
            while let Some(msg) = get_block_txs_rx.recv().await {
                let slot = msg.slot;
                let requester = msg.requester;
                // Try to look up the block from our state
                match state_for_get_txs.get_block_by_slot(slot) {
                    Ok(Some(block)) => {
                        // Send back all transactions from the block. The requester
                        // already knows which ones it needs; sending all is simpler
                        // and still efficient since the block is typically small.
                        let response = lichen_p2p::P2PMessage::new(
                            lichen_p2p::MessageType::BlockTxs {
                                slot,
                                transactions: block.transactions,
                            },
                            local_addr_for_get_txs,
                        );
                        if let Some(ref pm) = peer_mgr_for_get_txs {
                            let pm = pm.clone();
                            tokio::spawn(async move {
                                if let Err(e) = pm.send_to_peer(&requester, response).await {
                                    warn!(
                                        "P2P: Failed to send BlockTxs for slot {} to {}: {}",
                                        slot, requester, e
                                    );
                                }
                            });
                        }
                    }
                    _ => {
                        debug!("P2P: GetBlockTxs for slot {} — block not found", slot);
                    }
                }
            }
        });
    }

    // P3-4: Handle erasure shard requests — encode block and return requested shards
    {
        let state_for_erasure = state.clone();
        let peer_mgr_for_erasure = p2p_peer_manager.clone();
        let local_addr_for_erasure = p2p_config.listen_addr;
        tokio::spawn(async move {
            while let Some(msg) = erasure_shard_request_rx.recv().await {
                let slot = msg.slot;
                let requester = msg.requester;
                match state_for_erasure.get_block_by_slot(slot) {
                    Ok(Some(block)) => {
                        let serialized = match bincode::serialize(&block) {
                            Ok(s) => s,
                            Err(e) => {
                                warn!("P2P: Failed to serialize block {} for erasure: {}", slot, e);
                                continue;
                            }
                        };
                        match lichen_p2p::erasure::encode_shards(slot, &serialized) {
                            Ok(all_shards) => {
                                let requested: Vec<lichen_p2p::erasure::ErasureShard> = msg
                                    .shard_indices
                                    .iter()
                                    .filter_map(|&idx| all_shards.get(idx).cloned())
                                    .collect();
                                let response = lichen_p2p::P2PMessage::new(
                                    lichen_p2p::MessageType::ErasureShardResponse {
                                        slot,
                                        shards: requested,
                                    },
                                    local_addr_for_erasure,
                                );
                                if let Some(ref pm) = peer_mgr_for_erasure {
                                    let pm = pm.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = pm.send_to_peer(&requester, response).await
                                        {
                                            warn!("P2P: Failed to send erasure shards for slot {} to {}: {}", slot, requester, e);
                                        }
                                    });
                                }
                            }
                            Err(e) => {
                                warn!("P2P: Erasure encoding failed for slot {}: {}", slot, e);
                            }
                        }
                    }
                    _ => {
                        debug!(
                            "P2P: ErasureShardRequest for slot {} — block not found",
                            slot
                        );
                    }
                }
            }
        });
    }

    // P3-4: Handle erasure shard responses — collect and reconstruct blocks
    {
        use std::collections::HashMap;
        let block_tx_for_erasure = block_tx_for_erasure;
        tokio::spawn(async move {
            // Track received shards per slot
            let mut shard_buffers: HashMap<u64, Vec<Option<lichen_p2p::erasure::ErasureShard>>> =
                HashMap::new();
            while let Some(msg) = erasure_shard_response_rx.recv().await {
                let slot = msg.slot;
                let buffer = shard_buffers
                    .entry(slot)
                    .or_insert_with(|| vec![None; lichen_p2p::erasure::TOTAL_SHARDS]);

                for shard in msg.shards {
                    let idx = shard.index;
                    if idx < buffer.len() {
                        buffer[idx] = Some(shard);
                    }
                }

                let present = buffer.iter().filter(|s| s.is_some()).count();
                if present >= lichen_p2p::erasure::DATA_SHARDS {
                    match lichen_p2p::erasure::decode_shards(buffer) {
                        Ok(data) => {
                            match bincode::deserialize::<Block>(&data) {
                                Ok(block) => {
                                    info!(
                                        "📦 Erasure-reconstructed block slot {} ({} shards used)",
                                        slot, present
                                    );
                                    if let Err(e) = block_tx_for_erasure.try_send(block) {
                                        warn!("P2P: Block channel full after erasure reconstruction ({})", e);
                                    }
                                }
                                Err(e) => {
                                    warn!("P2P: Failed to deserialize erasure-reconstructed block {}: {}", slot, e);
                                }
                            }
                            shard_buffers.remove(&slot);
                        }
                        Err(e) => {
                            warn!("P2P: Erasure decode failed for slot {}: {}", slot, e);
                        }
                    }
                }

                // Prune old buffers to avoid memory leaks
                if shard_buffers.len() > 100 {
                    let min_slot = shard_buffers.keys().copied().min().unwrap_or(0);
                    shard_buffers.remove(&min_slot);
                }
            }
        });
    }

    // Process slashing evidence received from P2P peers
    {
        let slashing_for_evidence = slashing_tracker.clone();
        tokio::spawn(async move {
            while let Some(evidence) = slashing_evidence_rx.recv().await {
                info!(
                    "⚔️  Received slashing evidence from P2P: {:?} for validator {}",
                    evidence.offense,
                    evidence.validator.to_base58()
                );
                let mut slasher = slashing_for_evidence.lock().await;
                if slasher.add_evidence(evidence.clone()) {
                    info!(
                        "⚔️  Evidence recorded for {} — sweep will apply penalty",
                        evidence.validator.to_base58()
                    );
                    // AUDIT-FIX CRITICAL-1: Do NOT call should_slash()/slash() here.
                    // The P2P handler must only record evidence. The periodic sweep
                    // (every 100 slots) applies the correct tiered economic penalty.
                    // Previously, calling slash() here marked the validator as slashed
                    // without any economic penalty, and the sweep then skipped it
                    // because is_slashed() returned true — a complete penalty bypass.
                } else {
                    debug!(
                        "Duplicate or invalid evidence for {}",
                        evidence.validator.to_base58()
                    );
                }
            }
        });
    }

    // ── Internal health watchdog ──────────────────────────────────────
    // Monitors last_block_time.  If no block is produced or received for
    // watchdog_timeout seconds, the validator is likely deadlocked.
    // Exit with EXIT_CODE_RESTART so the supervisor can relaunch us.
    let watchdog_timeout_secs = get_flag_value(&args, &["--watchdog-timeout"])
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_WATCHDOG_TIMEOUT_SECS);

    // Shared flag: suppress watchdog during the joining sync phase.
    // Joining nodes may spend minutes syncing without committing blocks,
    // which is NOT a stall — the watchdog must not kill them.
    let joining_sync_active = Arc::new(std::sync::atomic::AtomicBool::new(is_joining_network));

    let last_block_time_for_watchdog = last_block_time.clone();
    let state_for_watchdog = state.clone();
    let sync_manager_for_watchdog = sync_manager.clone();
    let joining_sync_for_watchdog = joining_sync_active.clone();
    tokio::spawn(async move {
        // Give the validator time to start up and sync before monitoring.
        // Use 60s startup grace — newly joining nodes need time to discover
        // peers, request genesis, and begin syncing.
        time::sleep(Duration::from_secs(60)).await;
        let mut interval = time::interval(Duration::from_secs(5));
        let mut stale_checks: u32 = 0;
        let threshold = (watchdog_timeout_secs / 5).max(6) as u32; // 6 checks minimum (30s)
        let mut last_known_slot: u64 = 0;
        loop {
            interval.tick().await;
            let elapsed = last_block_time_for_watchdog.lock().await.elapsed();
            let current_slot = state_for_watchdog.get_last_slot().unwrap_or(0);

            // STABILITY-FIX: Don't count stale checks while the sync manager
            // has pending blocks or is actively syncing. The node is alive —
            // it's just behind, not deadlocked.
            let actively_receiving = sync_manager_for_watchdog.is_actively_receiving().await;

            // Suppress watchdog entirely while the joining sync phase is active.
            // Joining nodes wait for snapshots + chain sync before committing
            // any blocks — this is normal, not a stall.
            if joining_sync_for_watchdog.load(std::sync::atomic::Ordering::Relaxed) {
                stale_checks = 0;
                continue;
            }

            if elapsed > Duration::from_secs(watchdog_timeout_secs)
                && current_slot == last_known_slot
                && !actively_receiving
            {
                stale_checks += 1;
                warn!(
                    "🐺 Watchdog: no block activity for {:.0}s (stale {}/{})",
                    elapsed.as_secs_f64(),
                    stale_checks,
                    threshold
                );
                if stale_checks >= threshold {
                    error!(
                        "🐺 Watchdog: validator stalled for {}s — triggering restart (exit {})",
                        elapsed.as_secs(),
                        EXIT_CODE_RESTART
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    std::process::exit(EXIT_CODE_RESTART);
                }
            } else {
                if stale_checks > 0 {
                    if actively_receiving {
                        info!(
                            "🐺 Watchdog: sync active, resetting stale count (slot {}, {} pending)",
                            current_slot,
                            sync_manager_for_watchdog.pending_count().await
                        );
                    } else {
                        info!("🐺 Watchdog: activity resumed (slot {})", current_slot);
                    }
                }
                stale_checks = 0;
                last_known_slot = current_slot;
            }
        }
    });

    // ========================================================================
    // AUTO-SUBMIT RegisterValidator TRANSACTION (if needed)
    // ========================================================================
    // Restart-safe registration: checks both local state AND bootstrap peer's
    // RPC before submitting, and persists a marker file after successful
    // submission to prevent duplicates across process restarts.
    //
    // Like Ethereum (deposit contract → check before deposit),
    // Cosmos (MsgCreateValidator → query validator set first),
    // Solana (CreateVoteAccount → check account exists via RPC).
    //
    // Three-phase design:
    //   Phase 0 — CHECK: Query bootstrap peer's RPC for our account.
    //                     If already registered, skip entirely.
    //   Phase 1 — SUBMIT: Send tx to bootstrap peer (max 3 retries for failures).
    //                      Write marker file on success.
    //   Phase 2 — WAIT:   Poll local state until registration appears in synced
    //                      blocks. NEVER resubmit after a successful sendTransaction.
    if needs_on_chain_registration {
        let state_for_register = state.clone();
        let register_keypair_seed = validator_keypair.to_seed();
        let register_pubkey = validator_pubkey;
        let register_fingerprint = machine_fingerprint;
        let bootstrap_peer_strings = explicit_seed_peer_strings.clone();
        let marker_path = std::path::PathBuf::from(&data_dir).join("registration-submitted.marker");
        let sync_mgr_for_register = sync_manager.clone();
        tokio::spawn(async move {
            // ── Derive bootstrap peer's RPC URL from its P2P address ──
            let bootstrap_rpc_url = if let Some(peer_addr) = bootstrap_peer_strings.first() {
                let parts: Vec<&str> = peer_addr.rsplitn(2, ':').collect();
                let (host, peer_p2p) = if parts.len() == 2 {
                    let port = parts[0].parse::<u16>().unwrap_or(7001);
                    (parts[1].to_string(), port)
                } else {
                    (peer_addr.clone(), 7001u16)
                };
                let base_p2p = if peer_p2p >= 8000 { 8001u16 } else { 7001u16 };
                let base_rpc = if peer_p2p >= 8000 { 9899u16 } else { 8899u16 };
                let offset = peer_p2p.saturating_sub(base_p2p);
                let rpc_port = base_rpc.saturating_add(offset.saturating_mul(2));
                format!("http://{}:{}", host, rpc_port)
            } else {
                warn!("⚠️  No bootstrap peers — cannot submit RegisterValidator via RPC");
                return;
            };

            info!(
                "📡 Will submit RegisterValidator via bootstrap RPC: {}",
                bootstrap_rpc_url
            );

            let http_client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("Failed to build HTTP client");

            // ────────────────────────────────────────────────────────
            // WAIT FOR SYNC — a validator MUST be fully synced before
            // registering.  This is how every other blockchain works:
            // Ethereum syncs beacon chain → then deposits 32 ETH.
            // Cosmos syncs blocks → then sends MsgCreateValidator.
            // Solana syncs snapshots → then creates vote account.
            // We sync all blocks → then send RegisterValidator tx.
            // ────────────────────────────────────────────────────────

            info!("⏳ Waiting for chain sync to complete before registering...");
            let sync_wait_start = std::time::Instant::now();
            loop {
                let current_slot = state_for_register.get_last_slot().unwrap_or(0);
                let phase = sync_mgr_for_register.get_sync_phase().await;
                if phase == sync::SyncPhase::LiveSync && current_slot > 0 {
                    info!(
                        "✅ Chain sync complete (slot {}, LiveSync) — proceeding with registration",
                        current_slot
                    );
                    break;
                }
                // Also break if caught up (within 2 slots of network tip)
                if sync_mgr_for_register.is_caught_up(current_slot).await && current_slot > 0 {
                    info!(
                        "✅ Chain caught up (slot {}) — proceeding with registration",
                        current_slot
                    );
                    break;
                }
                // Fallback: query bootstrap peer's RPC for its slot.
                // This handles the case where external seed peers advertise
                // much higher slots (different network), polluting highest_seen_slot.
                // If we're within 5 slots of our bootstrap peer, we're synced enough.
                if current_slot > 0 && sync_wait_start.elapsed() > Duration::from_secs(30) {
                    if let Ok(resp) = http_client
                        .post(&bootstrap_rpc_url)
                        .json(&serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "getSlot",
                            "params": []
                        }))
                        .send()
                        .await
                    {
                        if let Ok(body) = resp.json::<serde_json::Value>().await {
                            if let Some(bootstrap_slot) = body["result"].as_u64() {
                                if current_slot + 5 >= bootstrap_slot {
                                    info!(
                                        "✅ Caught up with bootstrap peer (local={}, bootstrap={}) — proceeding with registration",
                                        current_slot, bootstrap_slot
                                    );
                                    break;
                                } else {
                                    info!(
                                        "⏳ Syncing to bootstrap peer: slot {} / {}",
                                        current_slot, bootstrap_slot
                                    );
                                }
                            }
                        }
                    }
                }
                // Log progress every 10s
                let highest = sync_mgr_for_register.get_highest_seen().await;
                if highest > 0 {
                    info!(
                        "⏳ Syncing before registration: slot {} / {} ({:?})",
                        current_slot, highest, phase
                    );
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }

            // Re-check local state — sync may have applied a block containing
            // our registration from a previous run
            if state_for_register
                .get_account(&register_pubkey)
                .unwrap_or(None)
                .map(|a| a.staked >= BOOTSTRAP_GRANT_AMOUNT)
                .unwrap_or(false)
            {
                info!(
                    "✅ Validator already registered on-chain after sync — no registration needed"
                );
                return;
            }

            let register_kp = Keypair::from_seed(&register_keypair_seed);

            // Helper: check local state for registration
            let is_registered = |st: &lichen_core::StateStore| -> bool {
                st.get_account(&register_pubkey)
                    .unwrap_or(None)
                    .map(|a| a.staked >= BOOTSTRAP_GRANT_AMOUNT)
                    .unwrap_or(false)
            };

            // Helper: check bootstrap peer's RPC for our account (network state)
            let check_remote_registration = |client: &reqwest::Client, url: &str| {
                let client = client.clone();
                let url = url.to_string();
                let pk_b58 = register_pubkey.to_base58();
                async move {
                    match client
                        .post(&url)
                        .json(&serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "getAccountInfo",
                            "params": [pk_b58]
                        }))
                        .send()
                        .await
                    {
                        Ok(resp) => match resp.json::<serde_json::Value>().await {
                            Ok(body) => {
                                if let Some(acct) = body["result"]["value"].as_object() {
                                    let staked =
                                        acct.get("staked").and_then(|v| v.as_u64()).unwrap_or(0);
                                    staked >= BOOTSTRAP_GRANT_AMOUNT
                                } else {
                                    false
                                }
                            }
                            Err(_) => false,
                        },
                        Err(_) => false,
                    }
                }
            };

            // ────────────────────────────────────────────────────────
            // PHASE 0: CHECK — is this validator already registered?
            // ────────────────────────────────────────────────────────
            // Check local state first (fastest)
            if is_registered(&state_for_register) {
                info!("✅ Validator already registered on-chain (local state)");
                return;
            }

            // Check marker file — a previous process already submitted
            if marker_path.exists() {
                info!("⏳ Registration marker found — previous process already submitted tx");
                info!("   Waiting for block sync to confirm registration...");
                for wait in 1..=300u32 {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if is_registered(&state_for_register) {
                        info!("✅ RegisterValidator confirmed — validator registered on-chain!");
                        return;
                    }
                    if wait % 30 == 0 {
                        info!(
                            "⏳ Still waiting for registration confirmation ({}s elapsed)",
                            wait * 2
                        );
                    }
                }
                warn!("⚠️  Registration not confirmed after 10 minutes — marker exists but tx may have been lost");
                // Remove stale marker to allow fresh submission
                let _ = std::fs::remove_file(&marker_path);
            }

            // Check bootstrap peer's RPC (catches restart after tx landed but before sync)
            if check_remote_registration(&http_client, &bootstrap_rpc_url).await {
                info!("✅ Validator already registered on bootstrap peer — waiting for sync");
                // Write marker so next restart doesn't re-check
                let _ = std::fs::write(&marker_path, "registered-remotely\n");
                for wait in 1..=300u32 {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if is_registered(&state_for_register) {
                        info!("✅ RegisterValidator confirmed — validator registered on-chain!");
                        return;
                    }
                    if wait % 30 == 0 {
                        info!(
                            "⏳ Still waiting for local sync to confirm registration ({}s elapsed)",
                            wait * 2
                        );
                    }
                }
                warn!("⚠️  Registration confirmed on peer but not synced locally after 10 minutes");
                return;
            }

            // ────────────────────────────────────────────────────────
            // PHASE 1: SUBMIT — send exactly one tx to bootstrap peer
            // ────────────────────────────────────────────────────────
            for attempt in 1..=3u32 {
                if is_registered(&state_for_register) {
                    info!("✅ Validator already registered on-chain");
                    return;
                }

                // Get blockhash from bootstrap peer
                let blockhash = match http_client
                    .post(&bootstrap_rpc_url)
                    .json(&serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "getRecentBlockhash",
                        "params": []
                    }))
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(body) => {
                            if let Some(hex) = body["result"]["blockhash"].as_str() {
                                match lichen_core::Hash::from_hex(hex) {
                                    Ok(h) => h,
                                    Err(e) => {
                                        warn!(
                                            "⚠️  Bad blockhash from RPC: {} — retrying ({}/3)",
                                            e, attempt
                                        );
                                        tokio::time::sleep(Duration::from_secs(5)).await;
                                        continue;
                                    }
                                }
                            } else {
                                let err = body["error"]["message"].as_str().unwrap_or("unknown");
                                info!("⏳ getRecentBlockhash: {} — retrying ({}/3)", err, attempt);
                                tokio::time::sleep(Duration::from_secs(5)).await;
                                continue;
                            }
                        }
                        Err(e) => {
                            info!("⏳ Bad RPC response: {} — retrying ({}/3)", e, attempt);
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            continue;
                        }
                    },
                    Err(e) => {
                        info!(
                            "⏳ Bootstrap RPC unreachable: {} — retrying ({}/3)",
                            e, attempt
                        );
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                };

                // Build RegisterValidator tx: opcode 26 + fingerprint
                let mut ix_data = vec![26u8];
                ix_data.extend_from_slice(&register_fingerprint);
                let ix = lichen_core::Instruction {
                    program_id: lichen_core::processor::SYSTEM_PROGRAM_ID,
                    accounts: vec![register_pubkey],
                    data: ix_data,
                };
                let msg = lichen_core::Message::new(vec![ix], blockhash);
                let mut tx = Transaction::new(msg);
                let sig = register_kp.sign(&tx.message.serialize());
                tx.signatures.push(sig);

                let tx_bytes = tx.to_wire();
                use base64::{engine::general_purpose, Engine as _};
                let tx_b64 = general_purpose::STANDARD.encode(&tx_bytes);

                info!(
                    "📝 Submitting RegisterValidator tx ({}/3, blockhash={})",
                    attempt,
                    blockhash.to_hex()
                );

                // Send to bootstrap peer
                let submitted = match http_client
                    .post(&bootstrap_rpc_url)
                    .json(&serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 2,
                        "method": "sendTransaction",
                        "params": [tx_b64]
                    }))
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(body) => {
                            if let Some(sig) = body["result"].as_str() {
                                info!("📡 RegisterValidator tx accepted — signature: {}", sig);
                                true
                            } else {
                                let err = body["error"]["message"].as_str().unwrap_or("unknown");
                                warn!(
                                    "⚠️  sendTransaction rejected: {} — retrying ({}/3)",
                                    err, attempt
                                );
                                false
                            }
                        }
                        Err(e) => {
                            warn!(
                                "⚠️  Bad sendTransaction response: {} — retrying ({}/3)",
                                e, attempt
                            );
                            false
                        }
                    },
                    Err(e) => {
                        warn!(
                            "⚠️  sendTransaction failed: {} — retrying ({}/3)",
                            e, attempt
                        );
                        false
                    }
                };

                if submitted {
                    // Write marker file IMMEDIATELY so restarts don't resubmit
                    let marker_content = format!(
                        "submitted\nattempt={}\ntimestamp={}\nbootstrap={}\n",
                        attempt,
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                        bootstrap_rpc_url
                    );
                    if let Err(e) = std::fs::write(&marker_path, marker_content) {
                        warn!("⚠️  Failed to write registration marker: {}", e);
                    }

                    // ──────────────────────────────────────────────────────
                    // PHASE 2: WAIT — tx is in the bootstrap peer's block.
                    // DO NOT resubmit. Just wait for P2P sync to deliver
                    // the block containing our tx to local state.
                    // ──────────────────────────────────────────────────────
                    info!("⏳ Tx accepted by bootstrap peer — waiting for block sync...");
                    for wait in 1..=300u32 {
                        // 300 × 2s = 10 minutes — more than enough for sync
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        if is_registered(&state_for_register) {
                            info!(
                                "✅ RegisterValidator confirmed — validator registered on-chain!"
                            );
                            return;
                        }
                        if wait % 30 == 0 {
                            info!(
                                "⏳ Still waiting for registration confirmation ({}s elapsed)",
                                wait * 2
                            );
                        }
                    }
                    // If we get here after 10 minutes, something is very wrong.
                    // But don't resubmit — the tx already landed on the bootstrap peer.
                    warn!("⚠️  Registration not confirmed locally after 10 minutes — check bootstrap peer");
                    return;
                }

                tokio::time::sleep(Duration::from_secs(5)).await;
            }

            // All 3 submission attempts failed (network errors / RPC rejections).
            // Check one last time in case registration happened via another path.
            if is_registered(&state_for_register) {
                info!("✅ Validator registered on-chain");
                return;
            }
            warn!("⚠️  RegisterValidator not submitted after 3 attempts — may need manual registration");
        });
    }

    // ═══════════════════════════════════════════════════════════════
    //  BFT CONSENSUS LOOP
    //
    //  Tendermint-style: Propose → Prevote → Precommit → Commit.
    //  The consensus engine is a pure state machine — it never touches
    //  I/O directly. The loop drives it by feeding incoming P2P messages
    //  and timeout events, then executing the resulting ConsensusActions.
    // ═══════════════════════════════════════════════════════════════

    // ── Pre-loop: Joining network sync ──
    // Wait until we have genesis, validators, and are caught up before
    // entering the consensus loop.
    if is_joining_network {
        info!("⏳ Joining network — waiting for genesis sync and validator discovery");
        let snapshot_sync_join = snapshot_sync.clone();
        let sync_manager_join = sync_manager.clone();
        let vs_join = validator_set.clone();
        let sp_join = stake_pool.clone();
        loop {
            let has_genesis = state.get_block_by_slot(0).unwrap_or(None).is_some();
            if !has_genesis {
                info!(
                    "⏳ Waiting for genesis sync from network (tip: {})",
                    state.get_last_slot().unwrap_or(0)
                );
                time::sleep(Duration::from_millis(500)).await;
                continue;
            }

            let snapshot_ready = {
                let mut ss = snapshot_sync_join.lock().await;
                // If snapshot exchange hasn't marked pool/validators ready yet,
                // check if block replay already populated them (apply_block_effects
                // during sync creates pool entries and validator set entries).
                if !ss.validator_set {
                    let vs = vs_join.read().await;
                    if !vs.validators().is_empty() {
                        ss.validator_set = true;
                    }
                }
                if !ss.stake_pool {
                    let pool = sp_join.read().await;
                    if !pool.stake_entries().is_empty() {
                        ss.stake_pool = true;
                    }
                }
                ss.is_ready()
            };
            if !snapshot_ready {
                info!("⏳ Waiting for validator/stake snapshots");
                time::sleep(Duration::from_millis(500)).await;
                continue;
            }

            let vs = vs_join.read().await;
            let validator_count = vs.validators().len();
            drop(vs);

            if validator_count == 0 {
                info!(
                    "⏳ Waiting for validator discovery (found {} validators)",
                    validator_count
                );
                time::sleep(Duration::from_millis(500)).await;
                continue;
            }

            // Wait for chain sync
            let current_slot = state.get_last_slot().unwrap_or(0);
            if !sync_manager_join.is_caught_up(current_slot).await {
                let network_slot = sync_manager_join.get_highest_seen().await;
                info!(
                    "⏳ Syncing to network (current: {}, network: {}, {} validators)",
                    current_slot, network_slot, validator_count
                );
                time::sleep(Duration::from_millis(200)).await;
                continue;
            }

            info!(
                "✅ Synced! Found {} validators, chain tip at slot {}",
                validator_count,
                state.get_last_slot().unwrap_or(0)
            );
            break;
        }

        // ── Wait for on-chain registration before entering BFT ──
        // Cosmos/Tendermint: full nodes sync blocks but DO NOT vote.
        // Only validators with voting power > 0 participate in BFT.
        // A joining node is a full node until RegisterValidator lands.
        // Block receiver continues applying blocks in the background.
        if needs_on_chain_registration {
            info!("⏳ Waiting for RegisterValidator to land on-chain before entering consensus...");
            info!("   (Block receiver continues syncing in the background)");
            let mut wait_count = 0u32;
            loop {
                let is_registered = state
                    .get_account(&validator_pubkey)
                    .unwrap_or(None)
                    .map(|a| a.staked >= BOOTSTRAP_GRANT_AMOUNT)
                    .unwrap_or(false);
                if is_registered {
                    info!("✅ RegisterValidator confirmed — validator has on-chain stake");
                    break;
                }
                wait_count += 1;
                if wait_count.is_multiple_of(15) {
                    info!(
                        "⏳ Still waiting for registration ({}s, tip={})",
                        wait_count * 2,
                        state.get_last_slot().unwrap_or(0)
                    );
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }

        info!(
            "✅ Entering BFT consensus from height {}",
            state.get_last_slot().unwrap_or(0) + 1
        );
        // Mark join as complete so restarts don't re-enter joining mode.
        if let Err(e) = state.put_metadata("join_complete", b"1") {
            warn!("⚠️  Failed to persist join_complete marker: {}", e);
        }
        // Clear the joining sync flag so the watchdog resumes monitoring.
        joining_sync_active.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    // ── Initialize BFT consensus engine ──
    let bft_keypair = Keypair::from_seed(validator_keypair.secret_key());
    let mut bft =
        ConsensusEngine::new_with_min_stake(bft_keypair, validator_pubkey, min_validator_stake);
    let mut last_dex_trade_count = state.get_program_storage_u64("DEX", b"dex_trade_count");

    // ── Initialize Consensus WAL (G-1/G-2 fix) ──
    let mut consensus_wal = wal::ConsensusWal::open(&data_dir);
    let wal_recovery = consensus_wal.recover();
    if let Some((lock_h, lock_r, lock_hash)) = wal_recovery.locked_state {
        info!(
            "📋 WAL: Recovered locked state: h={} r={} hash={}",
            lock_h,
            lock_r,
            hex::encode(&lock_hash.0[..4])
        );
    }
    if let Some(cp) = wal_recovery.last_checkpoint {
        info!("📋 WAL: Last checkpoint at height {}", cp);
    }
    // Track the last lock we persisted so we can detect new locks.
    let mut last_wal_lock: Option<(u32, Hash)> = None;

    // Current timeout handle (if any). When the engine requests a timeout,
    // we spawn a sleep and race it against incoming messages.
    let mut timeout_handle: Option<(RoundStep, u32, std::pin::Pin<Box<tokio::time::Sleep>>)> = None;

    // Helper: derive parent_hash from chain tip
    let get_parent_hash = |st: &StateStore| -> Hash {
        let tip = st.get_last_slot().unwrap_or(0);
        if tip > 0 {
            st.get_block_by_slot(tip)
                .ok()
                .flatten()
                .map(|b| b.hash())
                .unwrap_or_default()
        } else {
            st.get_block_by_slot(0)
                .ok()
                .flatten()
                .map(|b| b.hash())
                .unwrap_or_default()
        }
    };

    // Drain stale BFT messages that accumulated during sync.
    // Without this, the proposal channel stays full of old-height proposals
    // and new proposals from the leader get dropped → joining node misses
    // current rounds and proposes its own blocks (fork).
    {
        let mut drained = 0u64;
        while proposal_rx.try_recv().is_ok() {
            drained += 1;
        }
        while prevote_rx.try_recv().is_ok() {
            drained += 1;
        }
        while precommit_rx.try_recv().is_ok() {
            drained += 1;
        }
        if drained > 0 {
            info!(
                "🔄 Drained {} stale BFT messages before entering consensus",
                drained
            );
        }
    }

    // Start the first height
    let start_height = state.get_last_slot().unwrap_or(0) + 1;
    bft.start_height(start_height);
    consensus_wal.log_height_start(start_height);
    // Restore lock from WAL recovery (G-2 fix: lock persistence across crashes)
    if let Some((lock_h, lock_r, lock_hash)) = wal_recovery.locked_state {
        bft.restore_lock(lock_h, lock_r, lock_hash);
        last_wal_lock = Some((lock_r, lock_hash));
    }
    parent_hash = get_parent_hash(&state);

    let (mut height_vs, mut height_pool) = freeze_consensus_snapshot_for_height(
        &state,
        &validator_set,
        &stake_pool,
        start_height,
        min_validator_stake,
    )
    .await;

    // If we're the proposer for round 0, build a block
    {
        if bft.is_proposer(&height_vs, &height_pool, &parent_hash) {
            info!(
                "👑 BFT: We are proposer for height={} round=0",
                start_height
            );
            let mut mp = mempool.lock().await;
            let bft_ts = compute_proposed_timestamp(&state, &parent_hash, &height_vs, &height_pool);
            let (mut block, _processed_hashes) = block_producer::build_block(
                &state,
                &mut mp,
                &processor,
                start_height,
                parent_hash,
                &validator_pubkey,
                Vec::new(),
                bft_ts,
            );
            drop(mp);
            block.header.validators_hash = compute_validators_hash(&height_vs, &height_pool);
            block.sign(&validator_keypair);
            let action = bft.create_proposal(block, &height_vs, &height_pool);
            execute_consensus_actions(
                action,
                &bft,
                &state,
                &validator_set,
                &stake_pool,
                &vote_aggregator,
                &mempool,
                &processor,
                &finality_tracker,
                &p2p_peer_manager,
                &p2p_config,
                &ws_event_tx,
                &ws_dex_broadcaster,
                &shared_oracle_prices,
                &last_block_time_for_local,
                &mut last_dex_trade_count,
                &data_dir,
                &sync_manager,
                &mut parent_hash,
                slot_duration_ms,
                &validator_keypair,
                min_validator_stake,
            )
            .await;
            // Schedule timeout for the step we landed on after proposing
            match bft.step {
                RoundStep::Prevote => {
                    timeout_handle = Some((
                        RoundStep::Prevote,
                        bft.round,
                        Box::pin(tokio::time::sleep(bft.prevote_timeout())),
                    ));
                }
                RoundStep::Precommit => {
                    timeout_handle = Some((
                        RoundStep::Precommit,
                        bft.round,
                        Box::pin(tokio::time::sleep(bft.precommit_timeout())),
                    ));
                }
                _ => {}
            }
        } else {
            // Not proposer — schedule propose timeout
            timeout_handle = Some((
                RoundStep::Propose,
                bft.round,
                Box::pin(tokio::time::sleep(bft.initial_propose_timeout())),
            ));
        }
    }

    // ── Delayed proposal timer ──
    // When we are the proposer after a commit, we delay 800ms (empty mempool)
    // or 100ms (pending TXs) to reduce QUIC stream pressure on P2P.
    let mut propose_timer: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;

    // ── Height-frozen validator set snapshots ──
    // Tendermint-style deferred activation: snapshot the validator set and
    // stake pool at the START of each height. ALL consensus operations
    // during that height use this frozen snapshot. New validators only
    // enter the BFT quorum at the NEXT EPOCH BOUNDARY. Without this, a
    // concurrent P2P announcement adding a validator mid-height changes
    // the quorum denominator (e.g., 3→4 validators), making 2/3
    // unreachable and stalling the chain.
    //
    // ── Main BFT event loop ──
    loop {
        // Check if chain tip advanced (block received via sync/P2P outside of BFT)
        let tip_slot = state.get_last_slot().unwrap_or(0);
        if tip_slot >= bft.height {
            // Chain advanced past our current height — start new height
            let new_height = tip_slot + 1;
            // WAL: checkpoint the completed height + log new height
            consensus_wal.checkpoint(tip_slot);
            last_wal_lock = None;

            bft.start_height(new_height);
            consensus_wal.log_height_start(new_height);
            parent_hash = get_parent_hash(&state);

            // Re-snapshot for the new height.
            // Consensus uses a height-frozen validator set. Newly discovered
            // validators are admitted only after a committed height boundary.
            (height_vs, height_pool) = freeze_consensus_snapshot_for_height(
                &state,
                &validator_set,
                &stake_pool,
                new_height,
                min_validator_stake,
            )
            .await;

            // G-10 fix: Replay any buffered future messages for this height.
            // This is critical for fast catch-up — proposals and votes that
            // arrived while we were at a previous height are processed now.
            let replay_action = bft.drain_future_messages(&height_vs, &height_pool);
            execute_consensus_actions(
                replay_action,
                &bft,
                &state,
                &validator_set,
                &stake_pool,
                &vote_aggregator,
                &mempool,
                &processor,
                &finality_tracker,
                &p2p_peer_manager,
                &p2p_config,
                &ws_event_tx,
                &ws_dex_broadcaster,
                &shared_oracle_prices,
                &last_block_time_for_local,
                &mut last_dex_trade_count,
                &data_dir,
                &sync_manager,
                &mut parent_hash,
                slot_duration_ms,
                &validator_keypair,
                min_validator_stake,
            )
            .await;

            // If drain already committed, loop back immediately
            if bft.step == RoundStep::Commit {
                continue;
            }

            if bft.is_proposer(&height_vs, &height_pool, &parent_hash) {
                info!("👑 BFT: We are proposer for height={} round=0", new_height);
                let mut mp = mempool.lock().await;
                let bft_ts =
                    compute_proposed_timestamp(&state, &parent_hash, &height_vs, &height_pool);
                let (mut block, _) = block_producer::build_block(
                    &state,
                    &mut mp,
                    &processor,
                    new_height,
                    parent_hash,
                    &validator_pubkey,
                    Vec::new(),
                    bft_ts,
                );
                drop(mp);
                block.header.validators_hash = compute_validators_hash(&height_vs, &height_pool);
                block.sign(&validator_keypair);
                let action = bft.create_proposal(block, &height_vs, &height_pool);
                execute_consensus_actions(
                    action,
                    &bft,
                    &state,
                    &validator_set,
                    &stake_pool,
                    &vote_aggregator,
                    &mempool,
                    &processor,
                    &finality_tracker,
                    &p2p_peer_manager,
                    &p2p_config,
                    &ws_event_tx,
                    &ws_dex_broadcaster,
                    &shared_oracle_prices,
                    &last_block_time_for_local,
                    &mut last_dex_trade_count,
                    &data_dir,
                    &sync_manager,
                    &mut parent_hash,
                    slot_duration_ms,
                    &validator_keypair,
                    min_validator_stake,
                )
                .await;
                // Schedule timeout for post-proposal step
                match bft.step {
                    RoundStep::Prevote => {
                        timeout_handle = Some((
                            RoundStep::Prevote,
                            bft.round,
                            Box::pin(tokio::time::sleep(bft.prevote_timeout())),
                        ));
                    }
                    RoundStep::Precommit => {
                        timeout_handle = Some((
                            RoundStep::Precommit,
                            bft.round,
                            Box::pin(tokio::time::sleep(bft.precommit_timeout())),
                        ));
                    }
                    _ => {
                        timeout_handle = None;
                    }
                }
                // If the block committed instantly (solo BFT), loop back to
                // start the next height without waiting on tokio::select!
                if bft.step == RoundStep::Commit {
                    // Delay proposal: 800ms heartbeat for empty blocks,
                    // slot_duration for blocks with pending TXs.
                    let has_pending = { mempool.lock().await.size() > 0 };
                    let delay = if has_pending { slot_duration_ms } else { 800 };
                    time::sleep(Duration::from_millis(delay)).await;
                    continue;
                }
            } else {
                timeout_handle = Some((
                    RoundStep::Propose,
                    bft.round,
                    Box::pin(tokio::time::sleep(bft.initial_propose_timeout())),
                ));
            }
        }

        // G-4 fix: Freeze production when significantly behind.
        // The BFT engine handles 1-3 block gaps via tip_notify + future
        // message buffer. Only freeze when truly far behind (10+ blocks),
        // which indicates the node should let sync catch up rather than
        // participating in consensus with stale state.
        {
            sync_manager.decay_highest_seen(tip_slot, 10).await;
            let network_highest = sync_manager.get_highest_seen().await;
            if network_highest > tip_slot + 10 {
                // Far behind — sleep and let sync catch up
                time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        }

        tokio::select! {
            // ── Incoming proposal ──
            Some(proposal) = proposal_rx.recv() => {
                let action = bft.on_proposal(proposal, &height_vs, &height_pool);
                execute_consensus_actions(
                    action,
                    &bft,
                    &state,
                    &validator_set,
                    &stake_pool,
                    &vote_aggregator,
                    &mempool,
                    &processor,
                    &finality_tracker,
                    &p2p_peer_manager,
                    &p2p_config,
                    &ws_event_tx,
                    &ws_dex_broadcaster,
                    &shared_oracle_prices,
                    &last_block_time_for_local,
                    &mut last_dex_trade_count,
                    &data_dir,
                    &sync_manager,
                    &mut parent_hash,
                    slot_duration_ms,
                    &validator_keypair,
                    min_validator_stake,
                ).await;
            }

            // ── Incoming prevote ──
            Some(prevote) = prevote_rx.recv() => {
                let action = bft.on_prevote(prevote, &height_vs, &height_pool);
                execute_consensus_actions(
                    action,
                    &bft,
                    &state,
                    &validator_set,
                    &stake_pool,
                    &vote_aggregator,
                    &mempool,
                    &processor,
                    &finality_tracker,
                    &p2p_peer_manager,
                    &p2p_config,
                    &ws_event_tx,
                    &ws_dex_broadcaster,
                    &shared_oracle_prices,
                    &last_block_time_for_local,
                    &mut last_dex_trade_count,
                    &data_dir,
                    &sync_manager,
                    &mut parent_hash,
                    slot_duration_ms,
                    &validator_keypair,
                    min_validator_stake,
                ).await;
            }

            // ── Incoming precommit ──
            Some(precommit) = precommit_rx.recv() => {
                let action = bft.on_precommit(precommit, &height_vs, &height_pool);
                execute_consensus_actions(
                    action,
                    &bft,
                    &state,
                    &validator_set,
                    &stake_pool,
                    &vote_aggregator,
                    &mempool,
                    &processor,
                    &finality_tracker,
                    &p2p_peer_manager,
                    &p2p_config,
                    &ws_event_tx,
                    &ws_dex_broadcaster,
                    &shared_oracle_prices,
                    &last_block_time_for_local,
                    &mut last_dex_trade_count,
                    &data_dir,
                    &sync_manager,
                    &mut parent_hash,
                    slot_duration_ms,
                    &validator_keypair,
                    min_validator_stake,
                ).await;
            }

            // ── Tip-advance notification (block received via P2P/sync) ──
            _ = tip_notify_for_producer.notified() => {
                // Chain tip advanced — the top-of-loop check will handle height change
            }

            // ── Delayed proposal timer ──
            // Fires after commit delay (800ms empty / 100ms with TXs)
            () = async {
                if let Some(ref mut timer) = propose_timer {
                    timer.as_mut().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                propose_timer = None;
                // Verify we're still in the Propose step and still current
                if bft.step == RoundStep::Propose
                    && bft.is_proposer(&height_vs, &height_pool, &parent_hash)
                {
                        info!(
                            "👑 BFT: We are proposer for height={} round={}",
                            bft.height, bft.round
                        );
                        let mut mp = mempool.lock().await;
                        let bft_ts = compute_proposed_timestamp(&state, &parent_hash, &height_vs, &height_pool);
                        let (mut block, _) = block_producer::build_block(
                            &state,
                            &mut mp,
                            &processor,
                            bft.height,
                            parent_hash,
                            &validator_pubkey,
                            Vec::new(),
                            bft_ts,
                        );
                        drop(mp);
                        block.header.validators_hash = compute_validators_hash(&height_vs, &height_pool);
                        block.sign(&validator_keypair);
                        let proposal_action = bft.create_proposal(block, &height_vs, &height_pool);
                        execute_consensus_actions(
                            proposal_action,
                            &bft,
                            &state,
                            &validator_set,
                            &stake_pool,
                            &vote_aggregator,
                            &mempool,
                            &processor,
                            &finality_tracker,
                            &p2p_peer_manager,
                            &p2p_config,
                            &ws_event_tx,
                            &ws_dex_broadcaster,
                            &shared_oracle_prices,
                            &last_block_time_for_local,
                            &mut last_dex_trade_count,
                            &data_dir,
                            &sync_manager,
                            &mut parent_hash,
                            slot_duration_ms,
                            &validator_keypair,
                            min_validator_stake,
                        ).await;
                        // Schedule timeout for post-proposal step
                        timeout_handle = match bft.step {
                            RoundStep::Prevote => Some((
                                RoundStep::Prevote,
                                bft.round,
                                Box::pin(tokio::time::sleep(bft.prevote_timeout())),
                            )),
                            RoundStep::Precommit => Some((
                                RoundStep::Precommit,
                                bft.round,
                                Box::pin(tokio::time::sleep(bft.precommit_timeout())),
                            )),
                            _ => None,
                        };
                }
            }

            // ── Timeout ──
            () = async {
                if let Some((_, _, ref mut sleep)) = timeout_handle {
                    sleep.as_mut().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                if let Some((step, round, _)) = timeout_handle.take() {
                    // Height-frozen snapshots: use the same validator set for
                    // the entire height. No live reads during consensus.
                    let action = bft.on_timeout(step, round, &height_vs, &height_pool);
                    execute_consensus_actions(
                        action,
                        &bft,
                        &state,
                        &validator_set,
                        &stake_pool,
                        &vote_aggregator,
                        &mempool,
                        &processor,
                        &finality_tracker,
                        &p2p_peer_manager,
                        &p2p_config,
                        &ws_event_tx,
                        &ws_dex_broadcaster,
                        &shared_oracle_prices,
                        &last_block_time_for_local,
                        &mut last_dex_trade_count,
                        &data_dir,
                        &sync_manager,
                        &mut parent_hash,
                        slot_duration_ms,
                        &validator_keypair,
                        min_validator_stake,
                    ).await;

                    // After timeout handling, if we moved to a new round's Propose step
                    // and we're the proposer, build and propose a block.
                    // Uses the SAME height-frozen snapshot for consistent leader election.
                    if bft.step == RoundStep::Propose
                        && bft.is_proposer(&height_vs, &height_pool, &parent_hash)
                    {
                            info!(
                                "👑 BFT: We are proposer for height={} round={}",
                                bft.height, bft.round
                            );
                            let mut mp = mempool.lock().await;
                            let bft_ts = compute_proposed_timestamp(&state, &parent_hash, &height_vs, &height_pool);
                            let (mut block, _) = block_producer::build_block(
                                &state,
                                &mut mp,
                                &processor,
                                bft.height,
                                parent_hash,
                                &validator_pubkey,
                                Vec::new(),
                                bft_ts,
                            );
                            drop(mp);
                            block.header.validators_hash = compute_validators_hash(&height_vs, &height_pool);
                            block.sign(&validator_keypair);
                            let proposal_action = bft.create_proposal(block, &height_vs, &height_pool);
                            execute_consensus_actions(
                                proposal_action,
                                &bft,
                                &state,
                                &validator_set,
                                &stake_pool,
                                &vote_aggregator,
                                &mempool,
                                &processor,
                                &finality_tracker,
                                &p2p_peer_manager,
                                &p2p_config,
                                &ws_event_tx,
                                &ws_dex_broadcaster,
                                &shared_oracle_prices,
                                &last_block_time_for_local,
                                &mut last_dex_trade_count,
                                &data_dir,
                                &sync_manager,
                                &mut parent_hash,
                                slot_duration_ms,
                                &validator_keypair,
                                min_validator_stake,
                            ).await;
                    }
                }
            }
        }

        // ── WAL persistence (G-1/G-2 fix) ──
        // After each event, persist any new lock to the WAL so it survives crashes.
        if let Some((round, hash)) = bft.locked_state() {
            if last_wal_lock.as_ref() != Some(&(round, hash)) {
                consensus_wal.log_lock(bft.height, round, hash);
                last_wal_lock = Some((round, hash));
            }
        }

        // ── Update timeout handle from engine state ──
        // After processing any event, check if the engine wants a new timeout.
        // The engine sets step to indicate what it's waiting for.
        match bft.step {
            RoundStep::Propose => {
                if timeout_handle
                    .as_ref()
                    .map(|t| t.0 != RoundStep::Propose || t.1 != bft.round)
                    .unwrap_or(true)
                {
                    timeout_handle = Some((
                        RoundStep::Propose,
                        bft.round,
                        Box::pin(tokio::time::sleep(bft.initial_propose_timeout())),
                    ));
                }
            }
            RoundStep::Prevote => {
                // Always ensure a prevote timeout is running so we don't
                // deadlock waiting for votes that never arrive.
                if timeout_handle
                    .as_ref()
                    .map(|t| t.0 != RoundStep::Prevote || t.1 != bft.round)
                    .unwrap_or(true)
                {
                    timeout_handle = Some((
                        RoundStep::Prevote,
                        bft.round,
                        Box::pin(tokio::time::sleep(bft.prevote_timeout())),
                    ));
                }
            }
            RoundStep::Precommit => {
                // Always ensure a precommit timeout is running.
                if timeout_handle
                    .as_ref()
                    .map(|t| t.0 != RoundStep::Precommit || t.1 != bft.round)
                    .unwrap_or(true)
                {
                    timeout_handle = Some((
                        RoundStep::Precommit,
                        bft.round,
                        Box::pin(tokio::time::sleep(bft.precommit_timeout())),
                    ));
                }
            }
            RoundStep::Commit => {
                // Block committed — start new height
                let new_height = state.get_last_slot().unwrap_or(0) + 1;
                if new_height > bft.height {
                    // WAL: checkpoint + log new height (G-1/G-2 fix)
                    consensus_wal.checkpoint(new_height - 1);
                    last_wal_lock = None;

                    bft.start_height(new_height);
                    consensus_wal.log_height_start(new_height);
                    parent_hash = get_parent_hash(&state);

                    // Re-snapshot for the new height.
                    // Consensus uses a height-frozen validator set. Newly
                    // discovered validators are admitted only after a
                    // committed height boundary.
                    (height_vs, height_pool) = freeze_consensus_snapshot_for_height(
                        &state,
                        &validator_set,
                        &stake_pool,
                        new_height,
                        min_validator_stake,
                    )
                    .await;

                    // G-10 fix: Replay buffered future messages
                    let replay_action = bft.drain_future_messages(&height_vs, &height_pool);
                    execute_consensus_actions(
                        replay_action,
                        &bft,
                        &state,
                        &validator_set,
                        &stake_pool,
                        &vote_aggregator,
                        &mempool,
                        &processor,
                        &finality_tracker,
                        &p2p_peer_manager,
                        &p2p_config,
                        &ws_event_tx,
                        &ws_dex_broadcaster,
                        &shared_oracle_prices,
                        &last_block_time_for_local,
                        &mut last_dex_trade_count,
                        &data_dir,
                        &sync_manager,
                        &mut parent_hash,
                        slot_duration_ms,
                        &validator_keypair,
                        min_validator_stake,
                    )
                    .await;

                    if bft.is_proposer(&height_vs, &height_pool, &parent_hash) {
                        // Delay proposal to reduce QUIC stream pressure.
                        // 800ms for empty blocks (heartbeat), 100ms for blocks with TXs.
                        let has_pending_txs = {
                            let mp = mempool.lock().await;
                            mp.size() > 0
                        };
                        let delay_ms = if has_pending_txs { 100 } else { 800 };
                        propose_timer = Some(Box::pin(tokio::time::sleep(Duration::from_millis(
                            delay_ms,
                        ))));
                        // Also set a propose timeout as safety net
                        timeout_handle = Some((
                            RoundStep::Propose,
                            bft.round,
                            Box::pin(tokio::time::sleep(bft.initial_propose_timeout())),
                        ));
                    } else {
                        timeout_handle = Some((
                            RoundStep::Propose,
                            bft.round,
                            Box::pin(tokio::time::sleep(bft.initial_propose_timeout())),
                        ));
                    }
                }
            }
        }

        // Periodic slashing evidence housekeeping (every 100 heights)
        if bft.height.is_multiple_of(100) && bft.step == RoundStep::Propose && bft.round == 0 {
            let mut slasher = slashing_tracker.lock().await;
            slasher.cleanup_expired(bft.height);
            let rep_penalties: Vec<(Pubkey, u64)> = {
                let vs = validator_set.read().await;
                vs.validators()
                    .iter()
                    .filter_map(|vi| {
                        let has_fault = slasher
                            .get_evidence(&vi.pubkey)
                            .map(|ev| {
                                ev.iter().any(|e| {
                                    matches!(
                                        e.offense,
                                        SlashingOffense::DoubleBlock { .. }
                                            | SlashingOffense::DoubleVote { .. }
                                    )
                                })
                            })
                            .unwrap_or(false);
                        if has_fault {
                            let penalty = slasher.calculate_penalty(&vi.pubkey);
                            if penalty > 0 {
                                Some((vi.pubkey, penalty))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .collect()
            };
            if !rep_penalties.is_empty() {
                let mut vs_w = validator_set.write().await;
                for (pk, penalty) in &rep_penalties {
                    if let Some(val) = vs_w.get_validator_mut(pk) {
                        val.reputation = val.reputation.saturating_sub(*penalty).max(50);
                    }
                }
                let vs_snapshot = vs_w.clone();
                drop(vs_w);
                if let Err(e) = state.save_validator_set(&vs_snapshot) {
                    error!("Failed to persist validator set after rep penalty: {}", e);
                }
            }
            let slasher_snapshot = slasher.clone();
            drop(slasher);
            if let Err(e) = state.put_slashing_tracker(&slasher_snapshot) {
                error!("Failed to persist slashing tracker: {}", e);
            }
        }
    }
}

/// Execute one or more ConsensusActions returned by the BFT engine.
///
/// This is the bridge between the pure state machine (ConsensusEngine) and
/// the real world (P2P broadcast, state storage, mempool cleanup, etc.).
#[allow(clippy::too_many_arguments)]
async fn execute_consensus_actions(
    action: ConsensusAction,
    bft: &ConsensusEngine,
    state: &StateStore,
    validator_set: &Arc<RwLock<ValidatorSet>>,
    stake_pool: &Arc<RwLock<StakePool>>,
    vote_aggregator: &Arc<RwLock<VoteAggregator>>,
    mempool: &Arc<Mutex<Mempool>>,
    processor: &Arc<TxProcessor>,
    finality_tracker: &FinalityTracker,
    p2p_peer_manager: &Option<Arc<lichen_p2p::PeerManager>>,
    p2p_config: &P2PConfig,
    ws_event_tx: &tokio::sync::broadcast::Sender<lichen_rpc::ws::Event>,
    ws_dex_broadcaster: &Arc<lichen_rpc::dex_ws::DexEventBroadcaster>,
    shared_oracle_prices: &SharedOraclePrices,
    last_block_time: &Arc<Mutex<std::time::Instant>>,
    last_dex_trade_count: &mut u64,
    data_dir: &str,
    sync_manager: &Arc<SyncManager>,
    parent_hash: &mut Hash,
    slot_duration_ms: u64,
    validator_keypair: &Keypair,
    min_validator_stake: u64,
) {
    match action {
        ConsensusAction::None => {}

        ConsensusAction::ScheduleTimeout(_step, _duration) => {
            // Timeouts are handled by the main loop's timeout_handle.
            // The ScheduleTimeout action is informational — the loop uses
            // the engine's step + round to manage the tokio::time::Sleep.
        }

        ConsensusAction::BroadcastProposal(proposal) => {
            info!(
                "📡 BFT SEND: Proposal h={} r={} hash={}",
                proposal.height,
                proposal.round,
                hex::encode(&proposal.block.hash().0[..4])
            );
            if let Some(ref pm) = p2p_peer_manager {
                let peers_count = pm.get_peers().len();
                info!(
                    "📡 BFT SEND: Broadcasting proposal to {} peers",
                    peers_count
                );
                let msg = P2PMessage::new(MessageType::Proposal(proposal), p2p_config.listen_addr);
                let pm_c = pm.clone();
                tokio::spawn(async move {
                    pm_c.broadcast(msg).await;
                });
            } else {
                warn!("📡 BFT SEND: No P2P peer manager — proposal NOT sent!");
            }
        }

        ConsensusAction::BroadcastPrevote(prevote) => {
            info!(
                "📡 BFT SEND: Prevote h={} r={} block={}",
                prevote.height,
                prevote.round,
                prevote
                    .block_hash
                    .map(|h| hex::encode(&h.0[..4]))
                    .unwrap_or_else(|| "nil".to_string())
            );
            if let Some(ref pm) = p2p_peer_manager {
                let peers_count = pm.get_peers().len();
                info!("📡 BFT SEND: Broadcasting prevote to {} peers", peers_count);
                let msg = P2PMessage::new(MessageType::Prevote(prevote), p2p_config.listen_addr);
                let pm_c = pm.clone();
                tokio::spawn(async move {
                    pm_c.broadcast_to_validators(msg).await;
                });
            } else {
                warn!("📡 BFT SEND: No P2P peer manager — prevote NOT sent!");
            }
        }

        ConsensusAction::BroadcastPrecommit(precommit) => {
            info!(
                "📡 BFT SEND: Precommit h={} r={} block={}",
                precommit.height,
                precommit.round,
                precommit
                    .block_hash
                    .map(|h| hex::encode(&h.0[..4]))
                    .unwrap_or_else(|| "nil".to_string())
            );
            if let Some(ref pm) = p2p_peer_manager {
                let peers_count = pm.get_peers().len();
                info!(
                    "📡 BFT SEND: Broadcasting precommit to {} peers",
                    peers_count
                );
                let msg =
                    P2PMessage::new(MessageType::Precommit(precommit), p2p_config.listen_addr);
                let pm_c = pm.clone();
                tokio::spawn(async move {
                    pm_c.broadcast_to_validators(msg).await;
                });
            } else {
                warn!("📡 BFT SEND: No P2P peer manager — precommit NOT sent!");
            }
        }

        ConsensusAction::CommitBlock {
            height,
            round: _commit_round,
            block,
            block_hash,
        } => {
            // Non-proposer nodes must replay the block's transactions so
            // their on-chain state (accounts, stake pool, contracts) matches
            // the proposer.  The proposer already executed TXs during
            // build_block, so replay is skipped for our own blocks (the
            // duplicate-TX guard would reject them anyway).
            let our_pubkey = validator_keypair.pubkey();
            if block.header.validator != our_pubkey.0 {
                replay_block_transactions(processor, &block);
            }

            // Apply block effects (rewards, staking, oracle)
            apply_block_effects(
                state,
                validator_set,
                stake_pool,
                vote_aggregator,
                &block,
                false,
                min_validator_stake,
            )
            .await;
            apply_oracle_from_block(state, &block);

            // Store block AS-PROPOSED — do NOT recompute state_root or re-sign.
            // All validators must store identical blocks so that parent_hash
            // (derived from block.hash()) is the same on every node. If each
            // node recomputes state_root from local state and overwrites the
            // header, any micro-divergence in genesis initialization, contract
            // state, or reward rounding causes a different stored hash, which
            // makes leader election disagree and stalls the chain.
            //
            // Effects (rewards, staking) are still applied to local state above
            // so CF_ACCOUNTS stays as up-to-date as possible, but the block
            // header is authoritative and untouched.
            let final_hash = block.hash();
            info!(
                "🔐 BFT: Block {} stored hash={} state_root={} proposer={}",
                height,
                hex::encode(&final_hash.0[..8]),
                hex::encode(&block.header.state_root.0[..8]),
                Pubkey(block.header.validator).to_base58(),
            );

            let confirmed_slot = height;
            let finalized_slot = height;

            // Store block, tip, and commitment metadata atomically.
            if let Err(e) =
                state.put_block_atomic(&block, Some(confirmed_slot), Some(finalized_slot))
            {
                error!("Failed to store block at height {}: {e}", height);
            }

            // EVM tx inclusion tracking
            for tx in &block.transactions {
                if let Some(ix) = tx.message.instructions.first() {
                    if ix.program_id == EVM_PROGRAM_ID {
                        let evm_hash = evm_tx_hash(&ix.data).0;
                        if let Err(e) = state.mark_evm_tx_included(&evm_hash, height, &block_hash) {
                            warn!("⚠️  Failed to mark EVM tx included: {}", e);
                        }
                    }
                }
            }

            // Update timestamps
            *last_block_time.lock().await = std::time::Instant::now();

            // Broadcast block to network (compact + full fallback)
            if let Some(ref pm) = p2p_peer_manager {
                let target_id = block.hash().0;
                let compact = lichen_p2p::CompactBlock::from_block(&block);
                let compact_msg = P2PMessage::new(
                    MessageType::CompactBlockMsg(compact),
                    p2p_config.listen_addr,
                );
                let pm_c = pm.clone();
                tokio::spawn(async move {
                    pm_c.route_to_closest(
                        &target_id,
                        lichen_p2p::NON_CONSENSUS_FANOUT,
                        compact_msg,
                    )
                    .await;
                });
            }

            // Emit program and NFT WebSocket events
            emit_program_and_nft_events(state, ws_event_tx, &block);

            // Broadcast block event to WebSocket subscribers
            let _ = ws_event_tx.send(lichen_rpc::ws::Event::Block(block.clone()));
            let _ = ws_event_tx.send(lichen_rpc::ws::Event::Slot(height));

            // DEX events + analytics bridge + SL/TP triggers
            {
                let current_trade_count = state.get_program_storage_u64("DEX", b"dex_trade_count");
                if current_trade_count > *last_dex_trade_count {
                    let prev = *last_dex_trade_count;
                    *last_dex_trade_count = current_trade_count;
                    let state_c = state.clone();
                    let bc_c = ws_dex_broadcaster.clone();
                    let slot_c = height;
                    tokio::task::spawn_blocking(move || {
                        emit_dex_events(&state_c, &bc_c, prev, current_trade_count, slot_c);
                    });
                }
                run_analytics_bridge_from_state(state, height, slot_duration_ms);
                run_sltp_triggers_from_state(state);
            }

            // Rolling 24h window reset
            reset_24h_stats_if_expired(state, block.header.timestamp);

            // Finality tracking
            {
                let finality = finality_tracker.clone();
                let _ = finality.mark_confirmed(height);
            }

            // Remove included transactions from mempool
            {
                let tx_hashes: Vec<Hash> = block.transactions.iter().map(|tx| tx.hash()).collect();
                let mut pool = mempool.lock().await;
                pool.remove_transactions_bulk(&tx_hashes);
            }

            // Checkpoint
            maybe_create_checkpoint(state, height, data_dir, sync_manager).await;

            // Periodic stats pruning
            if height.is_multiple_of(1000) {
                match state.prune_slot_stats(height, 10_000) {
                    Ok(0) => {}
                    Ok(n) => info!("🧹 Pruned {} stale stats keys (retain last 10K slots)", n),
                    Err(e) => warn!("⚠️  Stats pruning failed at height {}: {}", height, e),
                }
                let sync_stats = sync_manager.stats().await;
                let checkpoint_slot = sync_manager.get_checkpoint().await;
                info!(
                    "📊 Sync stats [height {}]: pending={}, syncing={}, network_tip={}, checkpoint={}",
                    height,
                    sync_stats.pending_blocks,
                    sync_stats.is_syncing,
                    sync_stats.highest_seen,
                    checkpoint_slot,
                );
            }

            // Log
            let tx_count = block.transactions.len();
            if tx_count == 0 {
                info!(
                    "💓 COMMITTED {} | hash: {} | BFT round {} | liveness",
                    height,
                    hex::encode(&block_hash.0[..4]),
                    _commit_round,
                );
            } else {
                info!(
                    "📦 COMMITTED {} | hash: {} | txs: {} | BFT round {}",
                    height,
                    hex::encode(&block_hash.0[..4]),
                    tx_count,
                    _commit_round,
                );
                if let Ok(Some(val_account)) = state.get_account(&bft.validator_pubkey) {
                    info!(
                        "   💰 Validator balance: {} LICN",
                        val_account.balance_licn()
                    );
                }
            }

            *parent_hash = block_hash;
        }

        ConsensusAction::EquivocationDetected {
            height,
            round,
            validator,
            vote_type,
            hash_1,
            hash_2,
        } => {
            // G-9 evidence reactor: log BFT-level equivocation and record
            // it in the slashing tracker. The evidence is also broadcast
            // so other validators can verify and apply the same penalty.
            warn!(
                "⚔️  BFT EVIDENCE: Double-{} by {} at h={} r={} (hash1={} vs hash2={})",
                vote_type,
                validator.to_base58(),
                height,
                round,
                hash_1
                    .map(|h| hex::encode(&h.0[..4]))
                    .unwrap_or_else(|| "nil".into()),
                hash_2
                    .map(|h| hex::encode(&h.0[..4]))
                    .unwrap_or_else(|| "nil".into()),
            );
            // NOTE: Full evidence submission (SlashingEvidence + SlashValidator tx)
            // requires storing both conflicting signed messages. The SlashingTracker
            // already handles Vote-level and Block-level evidence. BFT-level
            // evidence is logged for monitoring; full provable evidence is a
            // Phase 2 enhancement once signed prevote/precommit are retained.
        }

        ConsensusAction::Multiple(actions) => {
            for sub_action in actions {
                // Box::pin to avoid recursive async issue
                Box::pin(execute_consensus_actions(
                    sub_action,
                    bft,
                    state,
                    validator_set,
                    stake_pool,
                    vote_aggregator,
                    mempool,
                    processor,
                    finality_tracker,
                    p2p_peer_manager,
                    p2p_config,
                    ws_event_tx,
                    ws_dex_broadcaster,
                    shared_oracle_prices,
                    last_block_time,
                    last_dex_trade_count,
                    data_dir,
                    sync_manager,
                    parent_hash,
                    slot_duration_ms,
                    validator_keypair,
                    min_validator_stake,
                ))
                .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lichen_core::{Instruction, Message, MIN_VALIDATOR_STAKE};

    // ── Helper builders ─────────────────────────────────────────────

    fn make_tx_with_opcode(program_id: Pubkey, opcode: u8) -> Transaction {
        Transaction {
            signatures: vec![[0u8; 64]],
            message: Message {
                instructions: vec![Instruction {
                    program_id,
                    accounts: vec![Pubkey([1u8; 32])],
                    data: vec![opcode],
                }],
                recent_blockhash: Hash([0u8; 32]),
                compute_budget: None,
                compute_unit_price: None,
            },
            tx_type: Default::default(),
        }
    }

    fn make_empty_tx() -> Transaction {
        Transaction {
            signatures: vec![],
            message: Message {
                instructions: vec![],
                recent_blockhash: Hash([0u8; 32]),
                compute_budget: None,
                compute_unit_price: None,
            },
            tx_type: Default::default(),
        }
    }

    fn make_block_with_txs(txs: Vec<Transaction>) -> Block {
        Block {
            header: lichen_core::BlockHeader {
                slot: 1,
                parent_hash: Hash([0u8; 32]),
                state_root: Hash([0u8; 32]),
                tx_root: Hash([0u8; 32]),
                timestamp: 0,
                validators_hash: Hash([0u8; 32]),
                validator: [0u8; 32],
                signature: [0u8; 64],
            },
            transactions: txs,
            tx_fees_paid: vec![],
            oracle_prices: vec![],
            commit_round: 0,
            commit_signatures: vec![],
        }
    }

    fn register_test_symbol(state: &StateStore, symbol: &str, program: Pubkey) {
        state
            .register_symbol(
                symbol,
                lichen_core::state::SymbolRegistryEntry {
                    symbol: symbol.to_string(),
                    program,
                    owner: Pubkey([9u8; 32]),
                    name: None,
                    template: None,
                    metadata: None,
                    decimals: None,
                },
            )
            .expect("register symbol");
    }

    #[test]
    fn latest_verified_checkpoint_requires_finalized_committed_block() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let state = StateStore::open(temp_dir.path()).expect("open state");

        let validator_kp = Keypair::generate();
        let validator_pk = validator_kp.pubkey();

        let mut validator_set = ValidatorSet::new();
        validator_set.add_validator(ValidatorInfo {
            pubkey: validator_pk,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake: 100_000_000_000_000,
            joined_slot: 0,
            last_active_slot: 0,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: false,
        });

        let mut stake_pool = StakePool::new();
        stake_pool
            .stake(validator_pk, 100_000_000_000_000, 0)
            .expect("stake validator");

        let committed_state_root = state.compute_state_root();
        let mut block = Block::new_with_timestamp(
            1,
            Hash::default(),
            committed_state_root,
            validator_pk.0,
            Vec::new(),
            1_000,
        );
        block.sign(&validator_kp);
        block.commit_round = 0;

        let block_hash = block.hash();
        let signable = Precommit::signable_bytes(1, 0, &Some(block_hash), 1_000);
        block.commit_signatures = vec![lichen_core::CommitSignature {
            validator: validator_pk.0,
            signature: validator_kp.sign(&signable),
            timestamp: 1_000,
        }];

        state.put_block(&block).expect("put block");

        let checkpoint_path = temp_dir.path().join("checkpoints/slot-1");
        std::fs::create_dir_all(
            checkpoint_path
                .parent()
                .expect("checkpoint parent directory exists"),
        )
        .expect("create checkpoints dir");
        state
            .create_checkpoint(checkpoint_path.to_str().expect("checkpoint path"), 1)
            .expect("create checkpoint");

        assert!(
            latest_verified_checkpoint(
                temp_dir.path().to_str().expect("data dir"),
                &state,
                &validator_set,
                &stake_pool,
            )
            .is_none(),
            "checkpoint should not be exposed before slot is finalized"
        );

        state
            .set_last_finalized_slot(1)
            .expect("set finalized slot");

        let (meta, _, _) = latest_verified_checkpoint(
            temp_dir.path().to_str().expect("data dir"),
            &state,
            &validator_set,
            &stake_pool,
        )
        .expect("checkpoint should be exposed once finalized and verified");

        assert_eq!(meta.slot, 1);
        assert_eq!(meta.state_root, block.header.state_root.0);
    }

    #[test]
    fn verify_checkpoint_anchor_requires_signed_committed_header() {
        let validator_kp = Keypair::generate();
        let validator_pk = validator_kp.pubkey();

        let mut validator_set = ValidatorSet::new();
        validator_set.add_validator(ValidatorInfo {
            pubkey: validator_pk,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake: 100_000_000_000_000,
            joined_slot: 0,
            last_active_slot: 0,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: false,
        });

        let mut stake_pool = StakePool::new();
        stake_pool
            .stake(validator_pk, 100_000_000_000_000, 0)
            .expect("stake validator");

        let mut block = Block::new_with_timestamp(
            1,
            Hash::default(),
            Hash::hash(b"checkpoint-anchor-root"),
            validator_pk.0,
            Vec::new(),
            1_000,
        );
        block.sign(&validator_kp);
        block.commit_round = 0;

        let block_hash = block.hash();
        let signable = Precommit::signable_bytes(1, 0, &Some(block_hash), 1_000);
        let commit_signatures = vec![lichen_core::CommitSignature {
            validator: validator_pk.0,
            signature: validator_kp.sign(&signable),
            timestamp: 1_000,
        }];

        assert!(verify_checkpoint_anchor(
            1,
            block.header.state_root.0,
            Some(&block.header),
            0,
            &commit_signatures,
            &validator_set,
            &stake_pool,
        )
        .is_ok());

        assert!(verify_checkpoint_anchor(
            1,
            block.header.state_root.0,
            None,
            0,
            &commit_signatures,
            &validator_set,
            &stake_pool,
        )
        .is_err());
    }

    #[test]
    fn verify_committed_block_authenticity_rejects_missing_commit_certificate() {
        let validator_kp = Keypair::generate();
        let validator_pk = validator_kp.pubkey();

        let mut validator_set = ValidatorSet::new();
        validator_set.add_validator(ValidatorInfo {
            pubkey: validator_pk,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake: 100_000_000_000_000,
            joined_slot: 0,
            last_active_slot: 0,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: false,
        });

        let mut stake_pool = StakePool::new();
        stake_pool
            .stake(validator_pk, 100_000_000_000_000, 0)
            .expect("stake validator");

        let mut block = Block::new_with_timestamp(
            1,
            Hash::default(),
            Hash::hash(b"missing-commit-cert"),
            validator_pk.0,
            Vec::new(),
            1_000,
        );
        block.sign(&validator_kp);

        let result = verify_committed_block_authenticity(&block, &validator_set, &stake_pool);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "block 1 has no commit certificate".to_string()
        );
    }

    #[test]
    fn verify_block_validators_hash_rejects_mismatch() {
        let validator_kp = Keypair::generate();
        let validator_pk = validator_kp.pubkey();

        let mut validator_set = ValidatorSet::new();
        validator_set.add_validator(ValidatorInfo {
            pubkey: validator_pk,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake: 100_000_000_000_000,
            joined_slot: 0,
            last_active_slot: 0,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: false,
        });

        let mut stake_pool = StakePool::new();
        stake_pool
            .stake(validator_pk, 100_000_000_000_000, 0)
            .expect("stake validator");

        let mut block = Block::new_with_timestamp(
            1,
            Hash::default(),
            Hash::hash(b"validators-hash-root"),
            validator_pk.0,
            Vec::new(),
            1_000,
        );
        block.header.validators_hash = Hash([9u8; 32]);

        let err =
            verify_block_validators_hash(&block, &validator_set, &stake_pool, MIN_VALIDATOR_STAKE)
                .expect_err("mismatched validators_hash must be rejected");
        assert!(err.contains("validators_hash mismatch"));
    }

    #[test]
    fn checkpoint_anchor_support_counts_matching_peers() {
        let root_a = [1u8; 32];
        let root_b = [2u8; 32];
        let anchors = HashMap::from([
            (
                "127.0.0.1:7001".parse::<SocketAddr>().expect("socket addr"),
                (42u64, root_a),
            ),
            (
                "127.0.0.1:7002".parse::<SocketAddr>().expect("socket addr"),
                (42u64, root_a),
            ),
            (
                "127.0.0.1:7003".parse::<SocketAddr>().expect("socket addr"),
                (42u64, root_b),
            ),
        ]);

        assert_eq!(checkpoint_anchor_support(&anchors, 42, root_a), 2);
        assert_eq!(checkpoint_anchor_support(&anchors, 42, root_b), 1);
        assert_eq!(checkpoint_anchor_support(&anchors, 43, root_a), 0);
    }

    #[test]
    fn verify_block_validators_hash_ignores_pending_discovery_validators() {
        let active_pk = Pubkey([1u8; 32]);
        let pending_pk = Pubkey([2u8; 32]);

        let mut validator_set = ValidatorSet::new();
        validator_set.add_validator(ValidatorInfo {
            pubkey: active_pk,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake: 100_000_000_000_000,
            joined_slot: 0,
            last_active_slot: 0,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: false,
        });
        validator_set.add_validator(ValidatorInfo {
            pubkey: pending_pk,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake: 0,
            joined_slot: 1,
            last_active_slot: 1,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: true,
        });

        let mut stake_pool = StakePool::new();
        stake_pool
            .stake(active_pk, 100_000_000_000_000, 0)
            .expect("stake active validator");

        let mut block = Block::new_with_timestamp(
            1,
            Hash::default(),
            Hash::hash(b"validators-hash-active-only-root"),
            active_pk.0,
            Vec::new(),
            1_000,
        );
        block.header.validators_hash =
            compute_validators_hash(&validator_set.consensus_set(), &stake_pool);

        verify_block_validators_hash(&block, &validator_set, &stake_pool, MIN_VALIDATOR_STAKE)
            .expect("pending discovery validators must not affect validators_hash verification");
    }

    #[test]
    fn verify_block_validators_hash_accepts_locally_verified_pending_validators() {
        let active_pk = Pubkey([1u8; 32]);
        let joining_pk = Pubkey([2u8; 32]);

        let mut validator_set = ValidatorSet::new();
        validator_set.add_validator(ValidatorInfo {
            pubkey: active_pk,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake: MIN_VALIDATOR_STAKE,
            joined_slot: 0,
            last_active_slot: 0,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: false,
        });
        validator_set.add_validator(ValidatorInfo {
            pubkey: joining_pk,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake: 0,
            joined_slot: 1,
            last_active_slot: 1,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: true,
        });

        let mut stake_pool = StakePool::new();
        stake_pool
            .stake(active_pk, MIN_VALIDATOR_STAKE, 0)
            .expect("stake active validator");
        stake_pool
            .stake(joining_pk, MIN_VALIDATOR_STAKE, 0)
            .expect("stake joining validator");

        let mut promoted_set = validator_set.clone();
        promoted_set
            .get_validator_mut(&joining_pk)
            .expect("joining validator present")
            .pending_activation = false;
        promoted_set
            .get_validator_mut(&joining_pk)
            .expect("joining validator present")
            .stake = MIN_VALIDATOR_STAKE;

        let mut block = Block::new_with_timestamp(
            1,
            Hash::default(),
            Hash::hash(b"validators-hash-promoted-root"),
            active_pk.0,
            Vec::new(),
            1_000,
        );
        block.header.validators_hash =
            compute_validators_hash(&promoted_set.consensus_set(), &stake_pool);

        verify_block_validators_hash(&block, &validator_set, &stake_pool, MIN_VALIDATOR_STAKE)
            .expect(
                "locally verified pending validators must satisfy validators_hash verification",
            );
    }

    #[test]
    fn local_validator_is_pending_only_during_fresh_join() {
        assert!(should_add_local_validator_as_pending(true, 0));
        assert!(should_add_local_validator_as_pending(false, 10));
        assert!(!should_add_local_validator_as_pending(false, 0));
    }

    #[test]
    fn unsynced_announcements_stay_pending_without_local_stake() {
        assert!(should_add_announced_validator_as_pending(
            0,
            0,
            MIN_VALIDATOR_STAKE
        ));
        assert!(!should_add_announced_validator_as_pending(
            0,
            MIN_VALIDATOR_STAKE,
            MIN_VALIDATOR_STAKE,
        ));
        assert!(should_add_announced_validator_as_pending(
            25,
            MIN_VALIDATOR_STAKE,
            MIN_VALIDATOR_STAKE,
        ));
    }

    // ── is_reward_or_debt_tx ────────────────────────────────────────

    #[test]
    fn reward_tx_opcode_2_is_reward() {
        let tx = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 2);
        assert!(is_reward_or_debt_tx(&tx));
    }

    #[test]
    fn debt_tx_opcode_3_is_reward() {
        let tx = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 3);
        assert!(is_reward_or_debt_tx(&tx));
    }

    #[test]
    fn transfer_opcode_0_is_not_reward() {
        let tx = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 0);
        assert!(!is_reward_or_debt_tx(&tx));
    }

    #[test]
    fn non_system_program_is_not_reward() {
        let tx = make_tx_with_opcode(Pubkey([99u8; 32]), 2);
        assert!(!is_reward_or_debt_tx(&tx));
    }

    #[test]
    fn apply_oracle_from_block_uses_consensus_prices_not_block_payload() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let state = StateStore::open(temp_dir.path()).expect("open state");

        register_test_symbol(&state, "ORACLE", Pubkey([1u8; 32]));
        register_test_symbol(&state, "ANALYTICS", Pubkey([2u8; 32]));
        register_test_symbol(&state, "DEX", Pubkey([3u8; 32]));

        let genesis_key = Keypair::generate();
        state
            .set_genesis_pubkey(&genesis_key.pubkey())
            .expect("put genesis pubkey");

        state
            .put_oracle_consensus_price("LICN", GENESIS_LICN_PRICE_8DEC, 8, 7, 3)
            .expect("seed LICN consensus price");
        state
            .put_oracle_consensus_price("wSOL", 8_250_000_000, 8, 7, 3)
            .expect("seed wSOL consensus price");
        state
            .put_oracle_consensus_price("wETH", 200_000_000_000, 8, 7, 3)
            .expect("seed wETH consensus price");
        state
            .put_oracle_consensus_price("wBNB", 61_000_000_000, 8, 7, 3)
            .expect("seed wBNB consensus price");

        let mut block = make_block_with_txs(vec![]);
        block.header.slot = 8;
        block.header.timestamp = 1_700_000_000;
        block.oracle_prices = vec![("wSOL".to_string(), 123), ("wETH".to_string(), 456)];

        apply_oracle_from_block(&state, &block);

        let oracle_program = state
            .get_symbol_registry("ORACLE")
            .expect("get ORACLE registry")
            .expect("ORACLE registry present")
            .program;
        let analytics_program = state
            .get_symbol_registry("ANALYTICS")
            .expect("get ANALYTICS registry")
            .expect("ANALYTICS registry present")
            .program;
        let dex_program = state
            .get_symbol_registry("DEX")
            .expect("get DEX registry")
            .expect("DEX registry present")
            .program;

        let oracle_wsol = state
            .get_contract_storage(&oracle_program, b"price_wSOL")
            .expect("read ORACLE storage")
            .expect("wSOL mirrored feed present");
        assert!(oracle_wsol.len() >= 8);
        assert_eq!(
            u64::from_le_bytes(oracle_wsol[0..8].try_into().expect("wSOL raw")),
            8_250_000_000
        );

        let dex_band = state
            .get_contract_storage(&dex_program, b"dex_band_2")
            .expect("read DEX band")
            .expect("DEX band present");
        assert!(dex_band.len() >= 8);
        assert_eq!(
            u64::from_le_bytes(dex_band[0..8].try_into().expect("dex band raw")),
            82_500_000_000
        );

        let analytics_price = state
            .get_contract_storage(&analytics_program, b"ana_lp_2")
            .expect("read analytics price")
            .expect("analytics price present");
        assert_eq!(
            u64::from_le_bytes(analytics_price[0..8].try_into().expect("analytics raw")),
            82_500_000_000
        );
    }

    #[test]
    fn build_oracle_attestation_tx_encodes_native_instruction() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let state = StateStore::open(temp_dir.path()).expect("open state");
        let validator_kp = Keypair::generate();
        let validator = validator_kp.pubkey();

        let mut stake_pool = StakePool::new();
        stake_pool
            .stake(validator, lichen_core::consensus::MIN_VALIDATOR_STAKE, 0)
            .expect("stake validator");
        state.put_stake_pool(&stake_pool).expect("put stake pool");

        let mut tip = Block::new_with_timestamp(
            1,
            Hash::default(),
            Hash::default(),
            validator.0,
            Vec::new(),
            1,
        );
        tip.sign(&validator_kp);
        state.put_block(&tip).expect("put tip block");
        state.set_last_slot(1).expect("set last slot");

        let tx = build_oracle_attestation_tx(
            &state,
            &validator_kp.to_seed(),
            validator,
            "wSOL",
            8_250_000_000,
            8,
        )
        .expect("build oracle tx");

        assert_eq!(tx.message.instructions.len(), 1);
        let ix = &tx.message.instructions[0];
        assert_eq!(ix.program_id, CORE_SYSTEM_PROGRAM_ID);
        assert_eq!(ix.accounts, vec![validator]);
        assert_eq!(ix.data[0], 30);
        assert_eq!(ix.data[1] as usize, 4);
        assert_eq!(&ix.data[2..6], b"wSOL");
        assert_eq!(
            u64::from_le_bytes(ix.data[6..14].try_into().expect("oracle price bytes")),
            8_250_000_000
        );
        assert_eq!(ix.data[14], 8);
        assert_eq!(tx.message.recent_blockhash, tip.hash());
        assert_eq!(tx.signatures.len(), 1);
    }

    #[test]
    fn empty_tx_is_not_reward() {
        let tx = make_empty_tx();
        assert!(!is_reward_or_debt_tx(&tx));
    }

    // ── block_has_user_transactions ─────────────────────────────────

    #[test]
    fn block_with_only_rewards_has_no_user_tx() {
        let reward = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 2);
        let block = make_block_with_txs(vec![reward]);
        assert!(!block_has_user_transactions(&block));
    }

    #[test]
    fn block_with_transfer_has_user_tx() {
        let transfer = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 0);
        let block = make_block_with_txs(vec![transfer]);
        assert!(block_has_user_transactions(&block));
    }

    #[test]
    fn empty_block_has_no_user_tx() {
        let block = make_block_with_txs(vec![]);
        assert!(!block_has_user_transactions(&block));
    }

    #[test]
    fn mixed_block_has_user_tx() {
        let reward = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 2);
        let transfer = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 0);
        let block = make_block_with_txs(vec![reward, transfer]);
        assert!(block_has_user_transactions(&block));
    }

    #[test]
    fn parse_validator_version_accepts_optional_v_prefix() {
        let parsed = parse_validator_version("v0.1.0").unwrap();
        assert_eq!(parsed, Version::parse("0.1.0").unwrap());
    }

    #[test]
    fn validate_new_validator_version_rejects_older_versions() {
        let error = validate_new_validator_version("0.0.9").unwrap_err();
        assert!(error.contains("below minimum supported"));
    }

    // ── parse_marketplace_args ──────────────────────────────────────

    #[test]
    fn parse_marketplace_args_empty() {
        let parsed = parse_marketplace_args(&[]);
        assert!(parsed.collection.is_none());
        assert!(parsed.price.is_none());
    }

    #[test]
    fn parse_marketplace_args_invalid_json() {
        let parsed = parse_marketplace_args(b"not json");
        assert!(parsed.collection.is_none());
    }

    #[test]
    fn parse_marketplace_args_price_and_token_id() {
        let json = r#"{"price": 1000, "token_id": 42}"#;
        let parsed = parse_marketplace_args(json.as_bytes());
        assert_eq!(parsed.price, Some(1000));
        assert_eq!(parsed.token_id, Some(42));
    }

    #[test]
    fn parse_marketplace_args_price_as_string() {
        let json = r#"{"price": "5000"}"#;
        let parsed = parse_marketplace_args(json.as_bytes());
        assert_eq!(parsed.price, Some(5000));
    }

    #[test]
    fn parse_marketplace_args_camel_case_keys() {
        let json = r#"{"nftContract": "11111111111111111111111111111111", "tokenId": 7}"#;
        let parsed = parse_marketplace_args(json.as_bytes());
        // nftContract is an alias for "collection"
        assert!(parsed.collection.is_some());
        assert_eq!(parsed.token_id, Some(7));
    }

    #[test]
    fn sync_observed_validator_info_does_not_infer_bootstrap_stake() {
        let producer = Keypair::generate().pubkey();

        let info = make_sync_observed_validator_info(producer, 42, 0, 3, false);

        assert_eq!(info.pubkey, producer);
        assert_eq!(info.stake, 0);
        assert_eq!(info.blocks_proposed, 1);
        assert_eq!(info.transactions_processed, 3);
        assert_eq!(info.last_active_slot, 42);
        assert!(!info.pending_activation);
    }

    #[test]
    fn newer_snapshot_activity_does_not_replace_local_stake() {
        let existing = ValidatorInfo {
            pubkey: Keypair::generate().pubkey(),
            stake: 123,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            joined_slot: 5,
            last_active_slot: 10,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: false,
        };

        let remote = ValidatorInfo {
            stake: BOOTSTRAP_GRANT_AMOUNT,
            last_active_slot: 25,
            ..existing.clone()
        };

        let mut merged = existing.clone();
        if remote.last_active_slot > merged.last_active_slot {
            merged.last_active_slot = remote.last_active_slot;
        }

        assert_eq!(merged.last_active_slot, 25);
        assert_eq!(merged.stake, existing.stake);
    }

    #[test]
    fn pending_validator_requires_height_pool_entry_for_activation() {
        // A validator with NO on-chain staked balance and NO pool entry
        // should remain pending with stake=0.
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let state = StateStore::open(temp_dir.path()).expect("open state");
        let validator_pubkey = Keypair::generate().pubkey();

        // Account exists but has zero staked balance
        let account = Account::new(0, SYSTEM_ACCOUNT_OWNER);
        state
            .put_account(&validator_pubkey, &account)
            .expect("persist validator account");

        let mut validator_set = ValidatorSet::new();
        validator_set.add_validator(ValidatorInfo {
            pubkey: validator_pubkey,
            stake: MIN_VALIDATOR_STAKE,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            joined_slot: 1,
            last_active_slot: 1,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: true,
        });

        let validator_set = Arc::new(RwLock::new(validator_set));
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(activate_pending_validators_for_height(
            &state,
            &validator_set,
            &StakePool::new(),
            2,
            MIN_VALIDATOR_STAKE,
        ));

        let reconciled = runtime.block_on(async {
            validator_set
                .read()
                .await
                .get_validator(&validator_pubkey)
                .cloned()
                .expect("validator exists")
        });

        assert!(reconciled.pending_activation);
        assert_eq!(reconciled.stake, 0);
    }

    #[test]
    fn pending_validator_waits_one_height_after_discovery() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let state = StateStore::open(temp_dir.path()).expect("open state");
        let validator_pubkey = Keypair::generate().pubkey();

        let mut account = Account::new(0, SYSTEM_ACCOUNT_OWNER);
        account.staked = MIN_VALIDATOR_STAKE;
        state
            .put_account(&validator_pubkey, &account)
            .expect("persist validator account");

        let mut validator_set = ValidatorSet::new();
        validator_set.add_validator(ValidatorInfo {
            pubkey: validator_pubkey,
            stake: 0,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            joined_slot: 1,
            last_active_slot: 1,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: true,
        });

        let validator_set = Arc::new(RwLock::new(validator_set));
        let mut height_pool = StakePool::new();
        height_pool
            .stake(validator_pubkey, MIN_VALIDATOR_STAKE, 0)
            .expect("stake validator in height pool");

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(activate_pending_validators_for_height(
            &state,
            &validator_set,
            &height_pool,
            2,
            MIN_VALIDATOR_STAKE,
        ));

        let reconciled = runtime.block_on(async {
            validator_set
                .read()
                .await
                .get_validator(&validator_pubkey)
                .cloned()
                .expect("validator exists")
        });

        assert!(reconciled.pending_activation);
        assert_eq!(reconciled.stake, MIN_VALIDATOR_STAKE);
    }

    #[test]
    fn pending_validator_activates_after_next_height_boundary() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let state = StateStore::open(temp_dir.path()).expect("open state");
        let validator_pubkey = Keypair::generate().pubkey();

        let mut account = Account::new(0, SYSTEM_ACCOUNT_OWNER);
        account.staked = MIN_VALIDATOR_STAKE;
        state
            .put_account(&validator_pubkey, &account)
            .expect("persist validator account");

        let mut validator_set = ValidatorSet::new();
        validator_set.add_validator(ValidatorInfo {
            pubkey: validator_pubkey,
            stake: 0,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            joined_slot: 1,
            last_active_slot: 1,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: true,
        });

        let validator_set = Arc::new(RwLock::new(validator_set));
        let mut height_pool = StakePool::new();
        height_pool
            .stake(validator_pubkey, MIN_VALIDATOR_STAKE, 0)
            .expect("stake validator in height pool");

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(activate_pending_validators_for_height(
            &state,
            &validator_set,
            &height_pool,
            3,
            MIN_VALIDATOR_STAKE,
        ));

        let reconciled = runtime.block_on(async {
            validator_set
                .read()
                .await
                .get_validator(&validator_pubkey)
                .cloned()
                .expect("validator exists")
        });

        assert!(!reconciled.pending_activation);
        assert_eq!(reconciled.stake, MIN_VALIDATOR_STAKE);
    }

    #[test]
    fn frozen_snapshot_can_change_round_zero_proposer_selection() {
        let local_kp = Keypair::generate();
        let pending_kp = Keypair::generate();
        let local_pubkey = local_kp.pubkey();
        let pending_pubkey = pending_kp.pubkey();

        let mut live_vs = ValidatorSet::new();
        live_vs.add_validator(ValidatorInfo {
            pubkey: local_pubkey,
            stake: MIN_VALIDATOR_STAKE,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            joined_slot: 1,
            last_active_slot: 1,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: false,
        });
        live_vs.add_validator(ValidatorInfo {
            pubkey: pending_pubkey,
            stake: 1_000_000_000_000_000,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            joined_slot: 1,
            last_active_slot: 1,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: true,
        });

        let frozen_vs = live_vs.consensus_set();
        assert_eq!(frozen_vs.validators().len(), 1);

        let mut pool = StakePool::new();
        pool.stake(local_pubkey, MIN_VALIDATOR_STAKE, 0)
            .expect("stake local validator");
        pool.stake(pending_pubkey, 1_000_000_000_000_000, 0)
            .expect("stake pending validator");

        let mut bft = crate::consensus::ConsensusEngine::new_with_min_stake(
            Keypair::from_seed(local_kp.secret_key()),
            local_pubkey,
            MIN_VALIDATOR_STAKE,
        );
        bft.start_height(2);

        let mut mismatch_found = false;
        for seed in 0..4096u64 {
            let parent_hash = Hash::hash(&seed.to_le_bytes());
            assert!(
                bft.is_proposer(&frozen_vs, &pool, &parent_hash),
                "single-validator frozen snapshot must always elect the local validator"
            );
            if !bft.is_proposer(&live_vs, &pool, &parent_hash) {
                mismatch_found = true;
                break;
            }
        }

        assert!(
            mismatch_found,
            "live and frozen validator views should be able to disagree on the round-0 proposer"
        );
    }

    // ── block_fee_at_index ──────────────────────────────────────────

    #[test]
    fn block_fee_at_index_prefers_local_exact_fee() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let state = StateStore::open(temp_dir.path()).expect("open state");
        let fee_config = FeeConfig::default_from_constants();
        let tx = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 0);
        let mut block = make_block_with_txs(vec![tx]);
        block.tx_fees_paid = vec![500];
        assert_eq!(
            block_fee_at_index(&state, &block, 0, &fee_config),
            TxProcessor::compute_transaction_fee(&block.transactions[0], &fee_config)
        );
    }

    #[test]
    fn block_fee_at_index_fallback_when_mismatched() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let state = StateStore::open(temp_dir.path()).expect("open state");
        let fee_config = FeeConfig::default_from_constants();
        let tx = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 0);
        let mut block = make_block_with_txs(vec![tx]);
        block.tx_fees_paid = vec![]; // length mismatch
        assert_eq!(
            block_fee_at_index(&state, &block, 0, &fee_config),
            TxProcessor::compute_transaction_fee(&block.transactions[0], &fee_config)
        );
    }

    #[test]
    fn block_fee_at_index_out_of_bounds() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let state = StateStore::open(temp_dir.path()).expect("open state");
        let fee_config = FeeConfig::default_from_constants();
        let tx = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 0);
        let mut block = make_block_with_txs(vec![tx]);
        block.tx_fees_paid = vec![100];
        assert_eq!(block_fee_at_index(&state, &block, 5, &fee_config), 0);
    }

    #[test]
    fn block_total_fees_paid_ignores_tampered_native_fee_vector() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let state = StateStore::open(temp_dir.path()).expect("open state");
        let fee_config = FeeConfig::default_from_constants();
        let tx = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 0);
        let mut block = make_block_with_txs(vec![tx]);
        block.tx_fees_paid = vec![1];

        assert_eq!(
            block_total_fees_paid(&state, &block, &fee_config),
            TxProcessor::compute_transaction_fee(&block.transactions[0], &fee_config)
        );
    }

    // ── resolve_peer_list ───────────────────────────────────────────

    #[test]
    fn resolve_peer_list_parses_ip() {
        let peers = vec!["127.0.0.1:8000".to_string()];
        let result = resolve_peer_list(&peers);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].port(), 8000);
    }

    #[test]
    fn resolve_peer_list_empty() {
        let result = resolve_peer_list(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn resolve_peer_list_ipv6() {
        let peers = vec!["[::1]:9000".to_string()];
        let result = resolve_peer_list(&peers);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn resolve_peer_list_invalid_skipped() {
        let peers = vec![
            "not-a-valid-address".to_string(),
            "127.0.0.1:8000".to_string(),
        ];
        let result = resolve_peer_list(&peers);
        // invalid hostname without port won't resolve
        assert!(!result.is_empty(), "should have at least the valid peer");
    }

    // ── constants sanity ────────────────────────────────────────────

    #[test]
    fn treasury_reserve_is_50m() {
        assert_eq!(TREASURY_RESERVE_LICN, 50_000_000);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn watchdog_timeout_reasonable() {
        assert!(DEFAULT_WATCHDOG_TIMEOUT_SECS >= 30);
        assert!(DEFAULT_WATCHDOG_TIMEOUT_SECS <= 600);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn sync_fanout_reasonable() {
        assert!(SYNC_REQUEST_FANOUT >= 1 && SYNC_REQUEST_FANOUT <= 10);
    }

    #[test]
    fn exit_code_restart_is_75() {
        assert_eq!(EXIT_CODE_RESTART, 75);
    }

    // ── existing P9 tests ───────────────────────────────────────────

    /// P9-VAL-01 test: Verify run_sltp_triggers_from_state uses a persistent
    /// cursor and only processes new trades. This ensures both block producers
    /// and receivers execute triggers with identical parameters.
    #[test]
    fn test_sltp_trigger_cursor_tracks_state() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let state = StateStore::open(temp_dir.path()).expect("open state");

        // Deploy a fake DEX program so cursor/trade_count keys resolve
        let dex_pk = Pubkey([42u8; 32]);
        state
            .register_symbol(
                "DEX",
                lichen_core::state::SymbolRegistryEntry {
                    symbol: "DEX".to_string(),
                    program: dex_pk,
                    owner: Pubkey([0u8; 32]),
                    name: None,
                    template: None,
                    metadata: None,
                    decimals: None,
                },
            )
            .unwrap();

        // Initially: trade_count=0, cursor=0 → no-op
        run_sltp_triggers_from_state(&state);
        let cursor_after_noop = state.get_program_storage_u64("DEX", b"dex_sltp_trigger_cursor");
        assert_eq!(cursor_after_noop, 0, "cursor should stay 0 when no trades");

        // Simulate new trades: set trade_count=5
        state
            .put_contract_storage(&dex_pk, b"dex_trade_count", &5u64.to_le_bytes())
            .unwrap();

        // Now run triggers — should update cursor to 5
        run_sltp_triggers_from_state(&state);
        let cursor_after_trades = state.get_program_storage_u64("DEX", b"dex_sltp_trigger_cursor");
        assert_eq!(cursor_after_trades, 5, "cursor should advance to 5");

        // Calling again with same trade_count → no-op (idempotent)
        run_sltp_triggers_from_state(&state);
        let cursor_idempotent = state.get_program_storage_u64("DEX", b"dex_sltp_trigger_cursor");
        assert_eq!(cursor_idempotent, 5, "cursor should stay 5 (idempotent)");

        // More trades: set trade_count=10
        state
            .put_contract_storage(&dex_pk, b"dex_trade_count", &10u64.to_le_bytes())
            .unwrap();
        run_sltp_triggers_from_state(&state);
        let cursor_final = state.get_program_storage_u64("DEX", b"dex_sltp_trigger_cursor");
        assert_eq!(cursor_final, 10, "cursor should advance to 10");
    }

    /// P9-VAL-02 + P9-VAL-03 test: Verify that margin SL/TP closure settles
    /// PnL through the insurance fund instead of creating money from nothing,
    /// and uses saturating_add for balance credit.
    #[test]
    fn test_margin_sltp_settles_via_insurance_fund() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let state = StateStore::open(temp_dir.path()).expect("open state");

        // Register DEXMARGIN program (trigger engine looks up "DEXMARGIN")
        let margin_pk = Pubkey([50u8; 32]);
        state
            .register_symbol(
                "DEXMARGIN",
                lichen_core::state::SymbolRegistryEntry {
                    symbol: "DEXMARGIN".to_string(),
                    program: margin_pk,
                    owner: Pubkey([0u8; 32]),
                    name: None,
                    template: None,
                    metadata: None,
                    decimals: None,
                },
            )
            .unwrap();

        // Register LICHENCOIN program
        let lichencoin_pk = Pubkey([51u8; 32]);
        state
            .register_symbol(
                "LICHENCOIN",
                lichen_core::state::SymbolRegistryEntry {
                    symbol: "LICHENCOIN".to_string(),
                    program: lichencoin_pk,
                    owner: Pubkey([0u8; 32]),
                    name: None,
                    template: None,
                    metadata: None,
                    decimals: None,
                },
            )
            .unwrap();

        // Register DEX program (needed for trigger engine)
        let dex_pk = Pubkey([42u8; 32]);
        state
            .register_symbol(
                "DEX",
                lichen_core::state::SymbolRegistryEntry {
                    symbol: "DEX".to_string(),
                    program: dex_pk,
                    owner: Pubkey([0u8; 32]),
                    name: None,
                    template: None,
                    metadata: None,
                    decimals: None,
                },
            )
            .unwrap();

        // Seed insurance fund with 1000 units
        state
            .put_contract_storage(&margin_pk, b"mrg_insurance", &1000u64.to_le_bytes())
            .unwrap();

        // Create a fake open long position (pid=1) that should be TP-triggered
        // Position format: trader[32] + pair_id[8]=1 + side[1]=0 + status[1]=0(open)
        //   + size[8] + margin[8] + entry_price[8] + ...
        //   + sl@106[8] + tp@114[8]
        let trader = [1u8; 32];
        let mut pos_data = vec![0u8; 122];
        pos_data[0..32].copy_from_slice(&trader);
        // pair_id = 1 at [40..48]
        pos_data[40..48].copy_from_slice(&1u64.to_le_bytes());
        // side=0 (long) at [48]
        pos_data[48] = 0;
        // status=0 (open) at [49]
        pos_data[49] = 0;
        // size=1_000_000_000 at [50..58]
        pos_data[50..58].copy_from_slice(&1_000_000_000u64.to_le_bytes());
        // margin=500 at [58..66]
        pos_data[58..66].copy_from_slice(&500u64.to_le_bytes());
        // entry_price=100 at [66..74]
        pos_data[66..74].copy_from_slice(&100u64.to_le_bytes());
        // sl_price=0 at [106..114] (no SL)
        // tp_price=150 at [114..122]
        pos_data[114..122].copy_from_slice(&150u64.to_le_bytes());

        // Trigger engine reads positions as "mrg_pos_{pid}" with count "mrg_pos_count"
        state
            .put_contract_storage(&margin_pk, b"mrg_pos_1", &pos_data)
            .unwrap();
        state
            .put_contract_storage(&margin_pk, b"mrg_pos_count", &1u64.to_le_bytes())
            .unwrap();

        // Set up a trade at price=200 (above TP=150, triggers TP)
        // dex_trade_1: pair_id=1, price=200
        let mut trade_data = vec![0u8; 32];
        trade_data[8..16].copy_from_slice(&1u64.to_le_bytes()); // pair_id
        trade_data[16..24].copy_from_slice(&200u64.to_le_bytes()); // price
        state
            .put_contract_storage(&dex_pk, b"dex_trade_1", &trade_data)
            .unwrap();
        state
            .put_contract_storage(&dex_pk, b"dex_trade_count", &1u64.to_le_bytes())
            .unwrap();

        // Run the trigger engine
        run_sltp_triggers_from_state(&state);

        // Verify: position should be closed (status=1)
        let closed_data = state
            .get_contract_storage(&margin_pk, b"mrg_pos_1")
            .unwrap()
            .unwrap();
        assert_eq!(closed_data[49], 1, "position should be closed");

        // PnL: (200 - 100) * 1B / 1B = 100 profit
        // return_amount = margin(500) + capped_profit(min(100, 1000)) = 600
        // insurance_fund should be debited by 100: 1000 - 100 = 900
        let insurance_after = state.get_program_storage_u64("DEXMARGIN", b"mrg_insurance");
        assert_eq!(
            insurance_after, 900,
            "insurance fund should be debited by profit"
        );

        // Verify PnL tracking
        let pnl_profit = state.get_program_storage_u64("DEXMARGIN", b"mrg_pnl_profit");
        assert_eq!(pnl_profit, 100, "cumulative profit should be tracked");

        // Verify user balance credited (with saturating_add, P9-VAL-03)
        let balance_key = format!("balance_{}", hex::encode(trader));
        let user_bal = state.get_program_storage_u64("LICHENCOIN", balance_key.as_bytes());
        assert_eq!(user_bal, 600, "user should receive margin + capped profit");
    }
}
