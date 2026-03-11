// MoltChain Validator with BFT Consensus + P2P Network + RPC Server
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

mod keypair_loader;
mod sync;
mod threshold_signer;
pub mod updater;

use futures_util::{SinkExt, StreamExt};
use moltchain_core::nft::decode_token_state;
use moltchain_core::{
    evm_tx_hash, Account, Block, ContractInstruction, FeeConfig, FinalityTracker, ForkChoice,
    GenesisConfig, GenesisWallet, Hash, Keypair, MarketActivity, MarketActivityKind, Mempool,
    NftActivity, NftActivityKind, ProgramCallActivity, Pubkey, SlashingEvidence, SlashingOffense,
    StakePool, StateStore, Transaction, TxProcessor, ValidatorInfo, ValidatorSet, Vote,
    VoteAggregator, VoteAuthority, BASE_FEE, BOOTSTRAP_GRANT_AMOUNT, CONTRACT_DEPLOY_FEE,
    CONTRACT_UPGRADE_FEE, EVM_PROGRAM_ID, MAX_TX_AGE_BLOCKS, MIN_VALIDATOR_STAKE,
    NFT_COLLECTION_FEE, NFT_MINT_FEE, SLOTS_PER_EPOCH, SYSTEM_PROGRAM_ID as CORE_SYSTEM_PROGRAM_ID,
};
use moltchain_genesis::{
    derive_contract_address, genesis_auto_deploy, genesis_create_trading_pairs,
    genesis_initialize_contracts, genesis_seed_analytics_prices, genesis_seed_margin_prices,
    genesis_seed_oracle,
};
use moltchain_p2p::{
    validator_announcement_signing_message, ConsistencyReportMsg, MessageType, NodeRole, P2PConfig,
    P2PMessage, P2PNetwork, SnapshotKind, SnapshotRequestMsg, SnapshotResponseMsg,
    StatusRequestMsg, StatusResponseMsg,
};
use moltchain_rpc::start_rpc_server;
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
use std::time::Duration;
use sync::SyncManager;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time;
use tokio_tungstenite::tungstenite;
use tracing::{debug, error, info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const SYSTEM_ACCOUNT_OWNER: Pubkey = Pubkey([0x01; 32]);
const LEGACY_CONTRACT_DEPLOY_FEE_SHELLS: u64 = 2_500_000_000;
/// Validator rewards pool: 100M MOLT (10% of 1B supply).
/// Reduced from 150M (15%) for sustainable treasury with 20% annual reward decay.
/// The `.min(1_000_000_000)` cap in the legacy path is a safety guard.
const REWARD_POOL_MOLT: u64 = 100_000_000; // 10% of 1B supply (in MOLT, not shells)

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
//  SHARED ORACLE PRICES — Thread-safe container for Binance price data
//
//  The background oracle price feeder (WebSocket + REST) updates these
//  atomics. The block production loop reads them to include in each block.
//  All validators then apply the same prices deterministically from the
//  block data, ensuring oracle state is consensus-propagated.
// =========================================================================

/// Thread-safe container for oracle prices fetched from external sources.
/// The background oracle feeder updates these atomics; block production reads them
/// and includes them in every produced block via `Block::oracle_prices`.
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

    /// Snapshot current prices into block-embeddable format.
    /// Returns vec of (asset_symbol, price_microcents) pairs with non-zero prices.
    fn snapshot(&self) -> Vec<(String, u64)> {
        let mut prices = Vec::with_capacity(3);
        let wsol = self.wsol_micro.load(Ordering::Relaxed);
        let weth = self.weth_micro.load(Ordering::Relaxed);
        let wbnb = self.wbnb_micro.load(Ordering::Relaxed);
        if wsol > 0 {
            prices.push(("wSOL".to_string(), wsol));
        }
        if weth > 0 {
            prices.push(("wETH".to_string(), weth));
        }
        if wbnb > 0 {
            prices.push(("wBNB".to_string(), wbnb));
        }
        prices
    }
}

/// Maximum duration (seconds) the freeze-production guard can stay active
/// before forcibly resuming production. Prevents the death spiral where
/// continuous incoming blocks keep highest_seen elevated and the node
/// never produces, eventually getting killed by the watchdog.
const MAX_FREEZE_DURATION_SECS: u64 = 30;

/// Heartbeat (empty block) production is allowed only after this duration
/// since the last observed user-transaction block anywhere on the node.
const HEARTBEAT_GLOBAL_IDLE_SECS: u64 = 5;

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
    // ZK keys are cached in a shared location (~/.moltchain/zk/) so they
    // survive blockchain resets.  Release tarballs ship pre-generated keys
    // in a `zk/` directory next to the binary — those are copied into the
    // shared cache on first run so the expensive Groth16 setup never needs
    // to happen on the operator's machine.
    //
    // Priority: env vars > ~/.moltchain/zk/ (shared cache) > bundled (next to exe) > auto-generate
    let shared_zk_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".moltchain")
        .join("zk");

    let shield_path = env::var("MOLTCHAIN_ZK_SHIELD_VK_PATH")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| shared_zk_dir.join("vk_shield.bin"));
    let unshield_path = env::var("MOLTCHAIN_ZK_UNSHIELD_VK_PATH")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| shared_zk_dir.join("vk_unshield.bin"));
    let transfer_path = env::var("MOLTCHAIN_ZK_TRANSFER_VK_PATH")
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
        ("moltchain-faucet", "moltchain-faucet"),
        ("moltchain-custody", "moltchain-custody"),
        ("molt-cli", "molt-cli"),
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
    let moltchain_dir = runtime_home.join(".moltchain");
    moltchain_dir.join("node_cert.der").exists() && moltchain_dir.join("node_key.der").exists()
}

fn resolve_validator_runtime_home(data_dir: &Path) -> PathBuf {
    if let Ok(explicit_home) = env::var("MOLTCHAIN_HOME") {
        let explicit_path = PathBuf::from(&explicit_home);
        if !explicit_path.as_os_str().is_empty() {
            info!(
                "🏠 Runtime home: {} (from MOLTCHAIN_HOME env)",
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
    use moltchain_core::network::{NetworkType, SeedsConfig};
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
    reputation: u64,
    stake: u64,
    joined_slot: u64,
    last_active_slot: u64,
}

fn hash_validator_set(set: &ValidatorSet) -> Hash {
    let entries: Vec<ValidatorHashEntry> = set
        .sorted_validators()
        .into_iter()
        .map(|validator| ValidatorHashEntry {
            pubkey: validator.pubkey,
            reputation: validator.reputation,
            stake: validator.stake,
            joined_slot: validator.joined_slot,
            last_active_slot: validator.last_active_slot,
        })
        .collect();

    let data = serde_json::to_vec(&entries).unwrap_or_default();
    Hash::hash(&data)
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
            } else if ix.program_id == moltchain_core::CONTRACT_PROGRAM_ID {
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
    dex_broadcaster: &moltchain_rpc::dex_ws::DexEventBroadcaster,
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
        let bid_levels: Vec<moltchain_rpc::dex_ws::PriceLevel> = {
            let mut v: Vec<_> = bids
                .into_iter()
                .map(|(p, (q, c))| moltchain_rpc::dex_ws::PriceLevel {
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
        let ask_levels: Vec<moltchain_rpc::dex_ws::PriceLevel> = {
            let mut v: Vec<_> = asks
                .into_iter()
                .map(|(p, (q, c))| moltchain_rpc::dex_ws::PriceLevel {
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
        let current_bal = state.get_program_storage_u64("MOLTCOIN", balance_key.as_bytes());
        let _ = state.put_contract_storage(
            &match state.get_symbol_registry("MOLTCOIN") {
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
    ws_event_tx: &tokio::sync::broadcast::Sender<moltchain_rpc::ws::Event>,
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
        let _ = ws_event_tx.send(moltchain_rpc::ws::Event::Transaction(tx.clone()));

        // Emit AccountChange events for all accounts touched by this tx
        let mut seen_accounts = std::collections::HashSet::new();
        for ix in &tx.message.instructions {
            for account_pubkey in &ix.accounts {
                if seen_accounts.insert(*account_pubkey) {
                    if let Ok(Some(acct)) = state.get_account(account_pubkey) {
                        let _ = ws_event_tx.send(moltchain_rpc::ws::Event::AccountChange {
                            pubkey: *account_pubkey,
                            balance: acct.shells,
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
                        let _ = ws_event_tx.send(moltchain_rpc::ws::Event::NftMint { collection });
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

                        let _ = ws_event_tx.send(moltchain_rpc::ws::Event::NftTransfer {
                            collection: token_state.collection,
                        });
                    }
                    _ => {}
                }
            } else if ix.program_id == moltchain_core::CONTRACT_PROGRAM_ID {
                if let Ok(contract_ix) = ContractInstruction::deserialize(&ix.data) {
                    match contract_ix {
                        ContractInstruction::Deploy { .. } => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(moltchain_rpc::ws::Event::ProgramUpdate {
                                    program: *program,
                                    kind: "deploy".to_string(),
                                });
                            }
                        }
                        ContractInstruction::Upgrade { .. } => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(moltchain_rpc::ws::Event::ProgramUpdate {
                                    program: *program,
                                    kind: "upgrade".to_string(),
                                });
                            }
                        }
                        ContractInstruction::Close => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(moltchain_rpc::ws::Event::ProgramUpdate {
                                    program: *program,
                                    kind: "close".to_string(),
                                });
                            }
                        }
                        ContractInstruction::Call { function, args, .. } => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(moltchain_rpc::ws::Event::ProgramCall {
                                    program: *program,
                                });

                                // Emit Log event for contract call
                                let _ = ws_event_tx.send(moltchain_rpc::ws::Event::Log {
                                    contract: *program,
                                    message: format!("call:{}", function),
                                });

                                // Emit contract events from DB if stored during processing
                                if let Ok(events) = state.get_contract_logs(program, 50, None) {
                                    for event in &events {
                                        if event.slot == block.header.slot {
                                            let _ =
                                                ws_event_tx.send(moltchain_rpc::ws::Event::Log {
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
                                        MarketActivityKind::Listing => ws_event_tx.send(
                                            moltchain_rpc::ws::Event::MarketListing { activity },
                                        ),
                                        MarketActivityKind::Sale => {
                                            ws_event_tx.send(moltchain_rpc::ws::Event::MarketSale {
                                                activity,
                                            })
                                        }
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
                                            .unwrap_or(moltchain_core::Pubkey([0; 32]));
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
                                            .unwrap_or("molt")
                                            .to_string();
                                        let _ = ws_event_tx.send(
                                            moltchain_rpc::ws::Event::BridgeLock {
                                                chain: dest_chain,
                                                asset,
                                                amount,
                                                sender,
                                                recipient,
                                            },
                                        );
                                    }
                                    "mint" | "bridge_mint" => {
                                        let recipient = ix
                                            .accounts
                                            .get(1)
                                            .copied()
                                            .unwrap_or(moltchain_core::Pubkey([0; 32]));
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
                                        let _ = ws_event_tx.send(
                                            moltchain_rpc::ws::Event::BridgeMint {
                                                chain: source_chain,
                                                asset,
                                                amount,
                                                recipient,
                                                tx_hash,
                                            },
                                        );
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

fn block_fee_at_index(block: &Block, tx_index: usize, fallback_fee: u64) -> u64 {
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

fn block_total_fees_paid(block: &Block, fee_config: &FeeConfig) -> u64 {
    if block.tx_fees_paid.len() == block.transactions.len() {
        block.tx_fees_paid.iter().copied().sum()
    } else {
        block
            .transactions
            .iter()
            .map(|tx| TxProcessor::compute_transaction_fee(tx, fee_config))
            .sum()
    }
}

// =========================================================================
//  CONSENSUS-PROPAGATED ORACLE — Deterministic oracle data from blocks
//
//  Oracle prices are embedded in each block by the leader validator and
//  applied identically by ALL validators during block processing. This
//  ensures every validator has identical oracle state, preventing any
//  divergence in DEX WASM execution (price band checks etc.).
// =========================================================================

/// Apply oracle price data from a block to the local state.
///
/// Called on ALL validators (both the block producer and receivers) after
/// `apply_block_effects`. The oracle prices come from `block.oracle_prices`
/// which the block producer populated from its `SharedOraclePrices` atomics.
///
/// This function writes:
/// 1. Oracle price feeds to the ORACLE contract storage (moltoracle)
/// 2. DEX price bands to the DEX contract storage (dex_core)
/// 3. Analytics indicative prices to ANALYTICS contract storage (dex_analytics)
///
/// Since all validators apply the SAME prices from the SAME block, the
/// resulting state is deterministic and identical across the network.
fn apply_oracle_from_block(state: &StateStore, block: &Block) {
    if block.oracle_prices.is_empty() || block.header.slot == 0 {
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

    const ORACLE_DECIMALS: u8 = 8;
    const PRICE_SCALE: u64 = 1_000_000_000; // 1e9 for DEX price scaling
    const MICRO_SCALE_DIV: f64 = 1_000_000.0;

    // Parse prices from block data
    let mut wsol_usd: f64 = 0.0;
    let mut weth_usd: f64 = 0.0;
    let mut wbnb_usd: f64 = 0.0;

    for (symbol, micro) in &block.oracle_prices {
        let price_usd = *micro as f64 / MICRO_SCALE_DIV;
        match symbol.as_str() {
            "wSOL" => wsol_usd = price_usd,
            "wETH" => weth_usd = price_usd,
            "wBNB" => wbnb_usd = price_usd,
            _ => {}
        }
    }

    if wsol_usd <= 0.0 && weth_usd <= 0.0 && wbnb_usd <= 0.0 {
        return;
    }

    // Read MOLT price from oracle (or use default 0.10)
    let molt_usd = match state.get_contract_storage(&oracle_pk, b"price_MOLT") {
        Ok(Some(feed)) if feed.len() >= 8 => {
            let raw = u64::from_le_bytes(feed[0..8].try_into().unwrap_or([0; 8]));
            if raw > 0 {
                raw as f64 / 100_000_000.0
            } else {
                0.10
            }
        }
        _ => 0.10,
    };

    // ── Phase A: Write oracle price feeds to ORACLE contract ──
    let oracle_feeds: [(&[u8], f64); 3] = [
        (b"wSOL", wsol_usd),
        (b"wETH", weth_usd),
        (b"wBNB", wbnb_usd),
    ];

    for (asset, price_usd) in &oracle_feeds {
        if *price_usd <= 0.0 {
            continue;
        }
        let price_raw = (*price_usd * 10f64.powi(ORACLE_DECIMALS as i32)) as u64;

        // Build 49-byte oracle feed: price(8)+timestamp(8)+decimals(1)+feeder(32)
        let mut feed = Vec::with_capacity(49);
        feed.extend_from_slice(&price_raw.to_le_bytes());
        feed.extend_from_slice(&now_ts.to_le_bytes());
        feed.push(ORACLE_DECIMALS);
        feed.extend_from_slice(&feeder);

        let price_key = format!("price_{}", core::str::from_utf8(asset).unwrap_or("?"));
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
        (1, molt_usd),
        (2, wsol_usd),
        (3, weth_usd),
        (
            4,
            if molt_usd > 0.0 {
                wsol_usd / molt_usd
            } else {
                0.0
            },
        ),
        (
            5,
            if molt_usd > 0.0 {
                weth_usd / molt_usd
            } else {
                0.0
            },
        ),
        (6, wbnb_usd),
        (
            7,
            if molt_usd > 0.0 {
                wbnb_usd / molt_usd
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
    // Oracle-driven candle writes happen HERE (deterministic, consensus-replicated).
    // Every validator processes the same block.oracle_prices and writes identical
    // candles, ensuring all validators have the exact same charting data.
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
    let results = processor.process_transactions_parallel(&block.transactions, &producer_pubkey);
    for (tx, result) in block.transactions.iter().zip(results.iter()) {
        if !result.success {
            warn!(
                "⚠️  Tx replay failed in slot {}: {} ({})",
                block.header.slot,
                tx.signature().to_hex(),
                result.error.as_deref().unwrap_or_default()
            );
        }
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
    let is_heartbeat = !block_has_user_transactions(old_block);

    let reward = {
        // Use oracle-adjusted rewards (same logic as apply_block_effects)
        let reward_config = moltchain_core::consensus::RewardConfig::new();
        let molt_price = moltchain_core::consensus::molt_price_from_state(state);
        if is_heartbeat {
            reward_config.get_adjusted_heartbeat_reward(molt_price)
        } else {
            reward_config.get_adjusted_transaction_reward(molt_price)
        }
    };

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

    // Phase 2a: Compute reward reversal
    let reward_debit = reward.min(producer_account.spendable);
    if reward_debit > 0 {
        producer_account.shells = producer_account.shells.saturating_sub(reward_debit);
        producer_account.spendable = producer_account.spendable.saturating_sub(reward_debit);
        treasury_account.shells = treasury_account.shells.saturating_add(reward_debit);
        treasury_account.spendable = treasury_account.spendable.saturating_add(reward_debit);
    }

    // Phase 2b: Compute fee reversal
    let fee_config = state
        .get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());
    let total_fee = block_total_fees_paid(old_block, &fee_config);

    if total_fee > 0 {
        let producer_share = total_fee * fee_config.fee_producer_percent / 100;
        if producer_share > 0 {
            let fee_debit = producer_share.min(producer_account.spendable);
            producer_account.shells = producer_account.shells.saturating_sub(fee_debit);
            producer_account.spendable = producer_account.spendable.saturating_sub(fee_debit);
            treasury_account.shells = treasury_account.shells.saturating_add(fee_debit);
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
    use moltchain_core::SYSTEM_PROGRAM_ID;

    if old_block.header.slot == 0 {
        return;
    }

    let fee_config = state
        .get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());

    // AUDIT-FIX C7: Collect accounts touched by non-revertible instructions
    // so we can restore them from checkpoint if needed.
    let mut non_revertible_accounts: Vec<moltchain_core::Pubkey> = Vec::new();

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
        let mut overlay: HashMap<moltchain_core::Pubkey, Account> = HashMap::new();

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
                        receiver.shells = receiver.shells.saturating_sub(debit);
                        receiver.spendable = receiver.spendable.saturating_sub(debit);

                        let sender = overlay.entry(from).or_insert_with(|| {
                            state
                                .get_account(&from)
                                .ok()
                                .flatten()
                                .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER))
                        });
                        sender.shells = sender.shells.saturating_add(debit);
                        sender.spendable = sender.spendable.saturating_add(debit);
                    }
                }
            }
        }

        // 2. Refund fee to fee payer
        if let Some(first_ix) = tx.message.instructions.first() {
            if let Some(&fee_payer) = first_ix.accounts.first() {
                let fallback_fee = TxProcessor::compute_transaction_fee(tx, &fee_config);
                let fee = block_fee_at_index(old_block, tx_index, fallback_fee);
                if fee > 0 {
                    let payer = overlay.entry(fee_payer).or_insert_with(|| {
                        state
                            .get_account(&fee_payer)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER))
                    });
                    payer.shells = payer.shells.saturating_add(fee);
                    payer.spendable = payer.spendable.saturating_add(fee);
                }
            }
        }

        // Flush all modified accounts atomically
        if !overlay.is_empty() {
            let batch_accounts: Vec<(&moltchain_core::Pubkey, &Account)> = overlay.iter().collect();
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
                    let mut restore_accounts: Vec<(moltchain_core::Pubkey, Account)> = Vec::new();
                    let mut skipped = 0usize;
                    for acct_key in &non_revertible_accounts {
                        match checkpoint_store.get_account(acct_key) {
                            Ok(Some(cp_account)) => {
                                restore_accounts.push((*acct_key, cp_account));
                            }
                            Ok(None) => {
                                // Account didn't exist at checkpoint time — zero it out
                                // (it was created by the reverted block's contract call)
                                let zeroed = moltchain_core::Account {
                                    shells: 0,
                                    spendable: 0,
                                    staked: 0,
                                    locked: 0,
                                    data: Vec::new(),
                                    owner: SYSTEM_ACCOUNT_OWNER,
                                    executable: false,
                                    rent_epoch: 0,
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
                        let batch_refs: Vec<(&moltchain_core::Pubkey, &Account)> =
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
) {
    if block.header.slot == 0 || block.header.validator == [0u8; 32] {
        return;
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
            // H13 fix: require minimum stake before accepting new validator
            if stake_amount < MIN_VALIDATOR_STAKE {
                warn!(
                    "⚠️  Ignoring unregistered block producer {} with insufficient stake ({} < {})",
                    producer.to_base58(),
                    stake_amount,
                    MIN_VALIDATOR_STAKE
                );
            } else {
                let new_validator = ValidatorInfo {
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
                        block.transactions.len() as u64
                    },
                };
                vs.add_validator(new_validator);
            }
        }

        // PERF-OPT 4: Clone under lock, persist AFTER dropping write guard.
        // This frees the RwLock while RocksDB I/O runs, unblocking all readers.
        let vs_snapshot = vs.clone();
        drop(vs);
        if let Err(e) = state.save_validator_set(&vs_snapshot) {
            warn!("⚠️  Failed to persist validator set update: {}", e);
        }
    }

    // ── Protocol-level block reward (coinbase) ──────────────────────────
    // This is a consensus rule, not a transaction. Every validator
    // deterministically applies the same reward when processing any block.
    // No treasury private key needed — the protocol itself authorizes it.
    let block_hash = block.hash();
    if !skip_rewards && !reward_already {
        // Read MOLT price from on-chain oracle; falls back to $0.10 if unavailable
        let reward_config = moltchain_core::consensus::RewardConfig::new();
        let molt_price = moltchain_core::consensus::molt_price_from_state(state);
        let reward_total = if is_heartbeat {
            reward_config.get_adjusted_heartbeat_reward(molt_price)
        } else {
            reward_config.get_adjusted_transaction_reward(molt_price)
        };

        // 1. Check treasury can afford the reward BEFORE updating StakePool
        let treasury_pubkey = state.get_treasury_pubkey().ok().flatten();
        let can_afford = if let Some(ref tpk) = treasury_pubkey {
            state
                .get_account(tpk)
                .ok()
                .flatten()
                .map(|a| a.shells >= reward_total)
                .unwrap_or(false)
        } else {
            false
        };

        if !can_afford {
            if let Some(ref tpk) = treasury_pubkey {
                let bal = state
                    .get_account(tpk)
                    .ok()
                    .flatten()
                    .map(|a| a.shells)
                    .unwrap_or(0);
                warn!(
                    "⚠️  Treasury balance {} < reward {}, skipping protocol reward",
                    bal, reward_total
                );
            }
        } else {
            // 2. Update StakePool (tracks rewards, vesting, bootstrap debt)
            let (liquid, debt_payment, reward) = {
                let mut pool = stake_pool.write().await;
                let is_active = pool
                    .get_stake(&producer)
                    .map(|info| info.is_active)
                    .unwrap_or(false);
                if !is_active {
                    (0u64, 0u64, 0u64)
                } else {
                    let reward = pool.distribute_block_reward(&producer, slot, is_heartbeat);
                    pool.record_block_produced(&producer);
                    let (liquid, debt_payment) = pool.claim_rewards(&producer, slot);
                    // PERF-OPT 4: Clone under lock, persist AFTER dropping write guard.
                    let pool_snapshot = pool.clone();
                    drop(pool);
                    if let Err(e) = state.put_stake_pool(&pool_snapshot) {
                        warn!("⚠️  Failed to persist stake pool reward update: {}", e);
                    }
                    (liquid, debt_payment, reward)
                }
            };

            // 3. Protocol-level balance transfer: treasury → producer
            if reward > 0 {
                if let Some(ref treasury_pubkey) = treasury_pubkey {
                    let mut treasury_account = state
                        .get_account(treasury_pubkey)
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));

                    // Debit treasury: only the liquid portion leaves treasury
                    // Debt repayment is internal bookkeeping (reclassifies existing stake)
                    // H12 fix: when liquid==0, no treasury debit or producer credit needed
                    let debit_amount = liquid;
                    treasury_account.shells = treasury_account.shells.saturating_sub(debit_amount);
                    treasury_account.spendable =
                        treasury_account.spendable.saturating_sub(debit_amount);

                    // Credit producer: only liquid portion to spendable
                    // During vesting: 50% liquid to spendable, 50% debt repayment (no new coins)
                    // Fully vested: 100% liquid
                    // H12 fix: when liquid==0, credit nothing (was falling through to reward_total)
                    let credit_amount = liquid;
                    let mut producer_account = state
                        .get_account(&producer)
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));
                    producer_account
                        .add_spendable(credit_amount)
                        .unwrap_or_else(|e| {
                            warn!(
                                "\u{26a0}\u{fe0f}  Overflow crediting producer block reward: {}",
                                e
                            );
                        });

                    // L4-01 fix: treasury debit + producer credit in single atomic WriteBatch
                    if let Err(e) = state.atomic_put_accounts(
                        &[
                            (treasury_pubkey, &treasury_account),
                            (&producer, &producer_account),
                        ],
                        0,
                    ) {
                        warn!(
                            "⚠️  Failed to persist block reward (treasury→producer): {}",
                            e
                        );
                    }
                }

                let reward_type = if is_heartbeat {
                    "heartbeat"
                } else {
                    "transaction"
                };
                info!(
                    "💰 Block reward: {:.3} MOLT ({}) | liquid {:.3}, debt {:.3}",
                    reward as f64 / 1_000_000_000.0,
                    reward_type,
                    liquid as f64 / 1_000_000_000.0,
                    debt_payment as f64 / 1_000_000_000.0,
                );

                // ── ReefStake liquid staking reward distribution ──
                // Allocate REEFSTAKE_BLOCK_SHARE_BPS (10%) of each block
                // reward to the ReefStake pool, funding stMOLT yield.
                let reef_share = (reward_total as u128
                    * moltchain_core::REEFSTAKE_BLOCK_SHARE_BPS as u128
                    / 10_000) as u64;
                if reef_share > 0 {
                    match state.get_reefstake_pool() {
                        Ok(mut reef_pool) => {
                            if reef_pool.st_molt_token.total_supply > 0 {
                                // AUDIT-FIX E-6: Re-read treasury from state to get the
                                // post-block-reward-debit balance. The re-read is safe because
                                // atomic_put_accounts above writes directly to RocksDB.
                                if let Some(ref tpk) = treasury_pubkey {
                                    let mut t_acct =
                                        state.get_account(tpk).ok().flatten().unwrap_or_else(
                                            || Account::new(0, SYSTEM_ACCOUNT_OWNER),
                                        );
                                    if t_acct.shells >= reef_share {
                                        t_acct.shells = t_acct.shells.saturating_sub(reef_share);
                                        t_acct.spendable =
                                            t_acct.spendable.saturating_sub(reef_share);
                                        reef_pool.distribute_rewards(reef_share);
                                        // L4-01 fix: treasury debit + pool update in single atomic WriteBatch
                                        if let Err(e) = state.atomic_put_account_with_reefstake(
                                            tpk, &t_acct, &reef_pool,
                                        ) {
                                            warn!(
                                                "⚠️  Failed to persist ReefStake distribution: {}",
                                                e
                                            );
                                        } else {
                                            debug!(
                                                    "🌊 ReefStake: distributed {:.6} MOLT to {} stakers",
                                                    reef_share as f64 / 1_000_000_000.0,
                                                    reef_pool.positions.len(),
                                                );
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("⚠️  Failed to load ReefStake pool: {}", e);
                        }
                    }
                }
            }
        }

        if let Err(e) = state.set_reward_distribution_hash(slot, &block_hash) {
            warn!(
                "⚠️  Failed to record reward distribution for slot {}: {}",
                slot, e
            );
        }
    }

    let fee_config = state
        .get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());
    let total_fee = block_total_fees_paid(block, &fee_config);

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

    if treasury_account.shells < total_fee {
        warn!(
            "⚠️  Treasury balance {} < total fees {}, skipping distribution",
            treasury_account.shells, total_fee
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
    treasury_account.shells = treasury_account
        .shells
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
            "🏦 Treasury fees retained: {:.6} MOLT",
            treasury_share as f64 / 1_000_000_000.0
        );
    }

    // ── Founding moltys periodic vesting unlock ──
    // Check if any locked founding moltys should be unlocked based on block timestamp.
    // Schedule: 6-month cliff, then 18-month linear vest to month 24.
    if let Ok(Some((cliff_end, vest_end, total_amount))) = state.get_founding_vesting_params() {
        let block_time = block.header.timestamp;
        if block_time >= cliff_end {
            if let Ok(Some(fm_pubkey)) = state.get_founding_moltys_pubkey() {
                if let Ok(Some(mut fm_acct)) = state.get_account(&fm_pubkey) {
                    let target_unlocked = moltchain_core::consensus::founding_vesting_unlocked(
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
                            warn!("⚠️  Failed to update founding moltys vesting: {}", e);
                        } else if new_unlock > 1_000_000_000 {
                            // Only log for significant unlocks (> 1 MOLT)
                            info!(
                                "🔓 Founding moltys vest: unlocked {:.2} MOLT (total {:.2}M / {:.2}M)",
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

// ========================================================================
// FIRST-BOOT CONTRACT AUTO-DEPLOY
// ========================================================================
//  BACKGROUND ORACLE PRICE FEEDER — Real-time Binance WebSocket price feed
//  with REST API fallback. Writes to moltoracle + dex_analytics storage.
//
//  Architecture:
//    1. WebSocket reader: connects to Binance aggTrade streams for SOL/ETH,
//       stores latest prices in lock-free AtomicU64 (microdollars).
//    2. Storage writer: 1-second tick reads atomics, writes to oracle +
//       analytics contract storage only when prices have changed.
//    3. REST fallback: if WebSocket is unhealthy (no message in 30s),
//       fetches prices from Binance REST API as backup.
//    4. Auto-reconnect: exponential backoff 1s → 2s → 4s → ... → 30s max.
// ========================================================================

/// Price stored as microdollars in AtomicU64 (price * 1_000_000).
/// This gives 6 decimal precision, far exceeding oracle's 8-decimal format.
const MICRO_SCALE: f64 = 1_000_000.0;

/// Default Binance WebSocket aggTrade stream URL for SOL, ETH, and BNB.
/// Override via MOLTCHAIN_ORACLE_WS_URL (e.g. for Binance US: wss://stream.binance.us:9443/ws/...)
const DEFAULT_BINANCE_WS_URL: &str =
    "wss://stream.binance.com:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade";

/// Default Binance REST fallback URL.
/// Override via MOLTCHAIN_ORACLE_REST_URL (e.g. for Binance US: https://api.binance.us/api/v3/...)
const DEFAULT_BINANCE_REST_URL: &str =
    "https://api.binance.com/api/v3/ticker/price?symbols=[%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22]";

/// REST ticker response
#[derive(Deserialize)]
struct BinanceTicker {
    symbol: String,
    price: String,
}

fn spawn_oracle_price_feeder(
    state: StateStore,
    shared_prices: SharedOraclePrices,
    dex_broadcaster: std::sync::Arc<moltchain_rpc::dex_ws::DexEventBroadcaster>,
) {
    tokio::spawn(async move {
        // Configurable Binance endpoints via env vars (for geo-blocked regions)
        let oracle_ws_url: String = std::env::var("MOLTCHAIN_ORACLE_WS_URL")
            .unwrap_or_else(|_| DEFAULT_BINANCE_WS_URL.to_string());
        let oracle_rest_url: String = std::env::var("MOLTCHAIN_ORACLE_REST_URL")
            .unwrap_or_else(|_| DEFAULT_BINANCE_REST_URL.to_string());
        info!("🔮 Oracle WS: {}", oracle_ws_url);
        info!("🔮 Oracle REST: {}", oracle_rest_url);

        // Use the shared atomics — block production reads these to include
        // oracle prices in each block for consensus propagation.
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

        info!("🔮 Oracle price feeder started (WebSocket real-time → SharedOraclePrices → block consensus)");

        let candle_intervals: [u64; 9] =
            [60, 300, 900, 3600, 14400, 86400, 259200, 604800, 31536000];

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
            // This feeder ONLY updates SharedOraclePrices atomics (for block production)
            // and reads consensus-written state to broadcast WS events.

            let wsol_usd = cur_wsol as f64 / MICRO_SCALE;
            let weth_usd = cur_weth as f64 / MICRO_SCALE;
            let wbnb_usd = cur_wbnb as f64 / MICRO_SCALE;

            if wsol_usd <= 0.0 && weth_usd <= 0.0 && wbnb_usd <= 0.0 {
                continue;
            }

            // WS broadcasts — read consensus state and emit to WebSocket clients
            let current_slot = state.get_last_slot().unwrap_or(0);

            let molt_usd: f64 = 0.10;
            let pair_prices: [(u64, f64); 7] = [
                (1, molt_usd),
                (2, wsol_usd),
                (3, weth_usd),
                (
                    4,
                    if molt_usd > 0.0 {
                        wsol_usd / molt_usd
                    } else {
                        0.0
                    },
                ),
                (
                    5,
                    if molt_usd > 0.0 {
                        weth_usd / molt_usd
                    } else {
                        0.0
                    },
                ),
                (6, wbnb_usd),
                (
                    7,
                    if molt_usd > 0.0 {
                        wbnb_usd / molt_usd
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
        "🐺 MoltChain Supervisor started (max restarts: {})",
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

    info!("🦞 MoltChain Validator starting...");

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

    let signer_bind = match env::var("MOLTCHAIN_SIGNER_BIND") {
        Ok(value) if value.eq_ignore_ascii_case("off") => None,
        Ok(value) => Some(value),
        Err(_) => {
            let offset = p2p_port % 1000;
            let derived_port = 9200u16.saturating_add(offset);
            Some(format!("0.0.0.0:{}", derived_port))
        }
    };

    if let Some(bind) = signer_bind {
        if let Ok(addr) = bind.parse::<SocketAddr>() {
            let signer_data_dir = data_dir_path.clone();
            tokio::spawn(async move {
                threshold_signer::start_signer_server(addr, &signer_data_dir).await;
            });
        } else {
            warn!("Invalid MOLTCHAIN_SIGNER_BIND value: {}", bind);
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
    let genesis_config = if let Some(ref genesis_file) = genesis_path {
        info!("📜 Loading genesis from: {}", genesis_file);
        match GenesisConfig::from_file(genesis_file) {
            Ok(config) => {
                info!("✓ Genesis loaded successfully");
                info!("  Chain ID: {}", config.chain_id);
                info!("  Total supply: {} MOLT", config.total_supply_molt());
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
        target_binary: "moltchain-validator".to_string(),
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
        PathBuf::from("/etc/moltchain/seeds.json"),
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
        let cached = moltchain_p2p::PeerStore::load_from_path(&peer_store_path);
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
        // P2P role: read from MOLTCHAIN_P2P_ROLE env var, default to Validator
        role: std::env::var("MOLTCHAIN_P2P_ROLE")
            .ok()
            .and_then(|s| s.parse::<NodeRole>().ok())
            .unwrap_or(NodeRole::Validator),
        // P2P max_peers: read from MOLTCHAIN_P2P_MAX_PEERS env var, or auto-set by role
        max_peers: std::env::var("MOLTCHAIN_P2P_MAX_PEERS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok()),
        // P2P reserved relay peers: read from MOLTCHAIN_P2P_RESERVED_PEERS env var (comma-separated)
        reserved_relay_peers: std::env::var("MOLTCHAIN_P2P_RESERVED_PEERS")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        // P3-6: External address for NAT traversal (from MOLTCHAIN_EXTERNAL_ADDR env var)
        external_addr: std::env::var("MOLTCHAIN_EXTERNAL_ADDR")
            .ok()
            .and_then(|s| s.parse::<std::net::SocketAddr>().ok()),
    };

    let has_genesis_block = state.get_block_by_slot(0).unwrap_or(None).is_some();

    // ────────────────────────────────────────────────────────────────
    // SINGLE GENESIS INVARIANT
    // ────────────────────────────────────────────────────────────────
    // Rule: If ANY seed peers are known (from --bootstrap-peers,
    // seeds.json, or cached peers), this node MUST join the existing
    // network and sync genesis from it. Genesis creation ONLY happens
    // when there are genuinely zero seeds — the very first validator
    // bootstrapping a brand-new network with no peers configured.
    //
    // This makes it impossible for a second node to accidentally
    // create its own genesis — as long as seeds.json or --bootstrap-peers
    // is present (which it always will be after the first validator
    // publishes its address), every subsequent node joins.
    // ────────────────────────────────────────────────────────────────
    let has_any_seed_peers =
        !explicit_seed_peers.is_empty() || !cached_peers.is_empty() || !seed_peers.is_empty();

    let mut is_joining_network = if has_genesis_block {
        // Already have genesis — not joining, just resuming
        false
    } else if has_any_seed_peers {
        // Seeds exist → this node MUST join the network, never create genesis
        info!("🔄 Seed peers found — will sync genesis from the existing network");
        info!(
            "   Sources: {} explicit, {} from seeds.json, {} cached",
            explicit_seed_peers.len(),
            seed_peers.len().saturating_sub(explicit_seed_peers.len()),
            cached_peers.len(),
        );
        true
    } else {
        // No seeds at all → this is the very first validator on a new network
        error!("❌ No genesis block found and no seed peers available.");
        error!("   Run moltchain-genesis first to create the genesis database,");
        error!("   or provide --bootstrap-peers / seeds.json to join an existing network.");
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
                        .map(|dw| (dw.role.clone(), dw.pubkey, dw.amount_molt, dw.percentage))
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
                                            let amt = acc["amount_molt"].as_u64().unwrap_or(0);
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
        // Use CLI command `molt admin reconcile-accounts` if needed
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

            // 4. Fund treasury from genesis account (transfer REWARD_POOL)
            let reward_shells = Account::molt_to_shells(REWARD_POOL_MOLT.min(1_000_000_000));
            if let Some(genesis_pk) = genesis_wallet.as_ref().map(|w| w.pubkey) {
                if let Ok(Some(mut genesis_acct)) = state.get_account(&genesis_pk) {
                    if genesis_acct.spendable >= reward_shells {
                        genesis_acct.shells = genesis_acct.shells.saturating_sub(reward_shells);
                        genesis_acct.spendable =
                            genesis_acct.spendable.saturating_sub(reward_shells);
                        treasury_account.shells = reward_shells;
                        treasury_account.spendable = reward_shells;
                        state.put_account(&genesis_pk, &genesis_acct).ok();
                        info!(
                            "  ✓ Funded treasury with {} MOLT from genesis",
                            REWARD_POOL_MOLT
                        );
                    } else {
                        warn!(
                            "  ⚠️  Genesis account has insufficient spendable balance ({} < {})",
                            genesis_acct.spendable, reward_shells
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
        // Some nodes persisted 2.5 MOLT instead of canonical 25 MOLT.
        // ================================================================
        {
            match state.get_fee_config() {
                Ok(mut cfg) if cfg.contract_deploy_fee == LEGACY_CONTRACT_DEPLOY_FEE_SHELLS => {
                    warn!(
                        "🔧 RECONCILE: correcting legacy contract deploy fee {} -> {} shells",
                        cfg.contract_deploy_fee, CONTRACT_DEPLOY_FEE
                    );
                    cfg.contract_deploy_fee = CONTRACT_DEPLOY_FEE;
                    if let Err(e) = state.set_fee_config_full(&cfg) {
                        error!("  ✗ Failed to reconcile contract deploy fee: {}", e);
                    } else {
                        info!(
                            "  ✓ Contract deploy fee reconciled to {} shells (25 MOLT)",
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

            // Check if analytics seed data is present (ana_lp_1 = MOLT/mUSD)
            let ana_lp_1_exists = state
                .get_program_storage("ANALYTICS", b"ana_lp_1")
                .is_some();

            if !ana_lp_1_exists {
                info!("🔄 RECONCILE: Analytics price seeds missing — writing initial prices");
                genesis_seed_analytics_prices(&state, &genesis_pk);
                info!("  ✓ Analytics prices seeded for pairs 1-5");
            }

            // Check if oracle price feeds are present (price_MOLT)
            let molt_price_exists = state.get_program_storage("ORACLE", b"price_MOLT").is_some();

            // Check if margin prices are present (mrg_mark_1 = MOLT/mUSD)
            let mrg_mark_1_exists = state.get_program_storage("MARGIN", b"mrg_mark_1").is_some();

            if !mrg_mark_1_exists {
                info!("🔄 RECONCILE: Margin prices missing — seeding mark/index prices");
                genesis_seed_margin_prices(&state, &genesis_pk);
                info!("  ✓ Margin prices seeded for pairs 1-5");
            }

            if !molt_price_exists {
                info!("🔄 RECONCILE: Oracle price feeds missing — seeding initial prices");
                // Write oracle prices directly to contract storage
                // (WASM calls may not work on existing DB, so use direct writes)
                if let Some(oracle_pk) = derive_contract_address(&genesis_pk, "moltoracle") {
                    const ORACLE_DECIMALS: u8 = 8;
                    let oracle_feeds: &[(&str, u64)] = &[
                        ("MOLT", 10_000_000),      // $0.10
                        ("wSOL", 8_200_000_000),   // $82
                        ("wETH", 197_900_000_000), // $1,979
                    ];
                    let now_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

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
                            &now_secs.to_le_bytes(),
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
    // 2. MOLTCHAIN_VALIDATOR_KEYPAIR env var
    // 3. ~/.moltchain/validators/validator-{port}.json
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
                    stake: Account::molt_to_shells(validator_info.stake_molt),
                    reputation: validator_info.reputation,
                    blocks_proposed: 0,
                    votes_cast: 0,
                    correct_votes: 0,
                    last_active_slot: 0,
                    joined_slot: 0,
                    commission_rate: 500,
                    transactions_processed: 0,
                };

                set.add_validator(validator);
            }
        }

        // Add this validator if not already in genesis set
        // ⚠️ CRITICAL: Prevent genesis wallet from becoming a validator
        if let Some(genesis_pubkey) = genesis_pubkey {
            if validator_pubkey != genesis_pubkey {
                if !genesis_config
                    .initial_validators
                    .iter()
                    .any(|v| v.pubkey == validator_pubkey.to_base58())
                {
                    info!("⚠️  This validator not in genesis set, adding dynamically");
                    set.add_validator(ValidatorInfo {
                        pubkey: validator_pubkey,
                        stake: BOOTSTRAP_GRANT_AMOUNT, // 100K MOLT stake — matches V2/V3 join grant
                        reputation: 100,
                        blocks_proposed: 0,
                        votes_cast: 0,
                        correct_votes: 0,
                        last_active_slot: 0,
                        joined_slot: 0,
                        commission_rate: 500,
                        transactions_processed: 0,
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
            info!("⚠️  This validator not in genesis set, adding dynamically");
            set.add_validator(ValidatorInfo {
                pubkey: validator_pubkey,
                stake: BOOTSTRAP_GRANT_AMOUNT, // 100K MOLT stake — matches V2/V3 join grant
                reputation: 100,
                blocks_proposed: 0,
                votes_cast: 0,
                correct_votes: 0,
                last_active_slot: 0,
                joined_slot: 0,
                commission_rate: 500,
                transactions_processed: 0,
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
                                        "💰 Returned {} MOLT bootstrap grant to treasury from ghost {}",
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

    // Check if this validator has an account, if not create with bootstrap grant
    let validator_account = state.get_account(&validator_pubkey).unwrap_or_else(|e| {
        eprintln!("Failed to read validator account: {e}");
        None
    });
    if validator_account.is_none() {
        // Check if we're still in the bootstrap phase (first 200 validators)
        let bootstrap_grants_issued = {
            let persisted_pool = state.get_stake_pool().unwrap_or_else(|_| StakePool::new());
            persisted_pool.bootstrap_grants_issued()
        };
        let is_bootstrap_eligible =
            bootstrap_grants_issued < moltchain_core::consensus::MAX_BOOTSTRAP_VALIDATORS;

        if is_bootstrap_eligible {
            // H13 fix: Bootstrap grant must come from treasury, not ex nihilo
            let bootstrap_molt = BOOTSTRAP_GRANT_AMOUNT / 1_000_000_000; // 100K MOLT
            let bootstrap_shells = BOOTSTRAP_GRANT_AMOUNT;
            let treasury_pk = state.get_treasury_pubkey().ok().flatten();
            let mut funded = false;

            if let Some(ref tpk) = treasury_pk {
                if let Ok(Some(mut treasury)) = state.get_account(tpk) {
                    if treasury.spendable >= bootstrap_shells {
                        treasury.deduct_spendable(bootstrap_shells).ok();
                        state.put_account(tpk, &treasury).ok();
                        funded = true;
                        info!(
                            "💰 Bootstrap grant #{}: {} MOLT deducted from treasury",
                            bootstrap_grants_issued + 1,
                            bootstrap_molt
                        );
                    } else {
                        warn!(
                            "⚠️  Treasury has insufficient funds for bootstrap grant ({} < {})",
                            treasury.spendable, bootstrap_shells
                        );
                    }
                }
            }

            if !funded {
                warn!("⚠️  No treasury available — bootstrap grant skipped. Validator needs manual funding.");
            }

            let bootstrap_account = if funded {
                Account {
                    shells: bootstrap_shells,
                    spendable: 0,
                    staked: bootstrap_shells,
                    locked: 0,
                    data: Vec::new(),
                    owner: SYSTEM_ACCOUNT_OWNER,
                    executable: false,
                    rent_epoch: 0,
                }
            } else {
                Account::new(0, SYSTEM_ACCOUNT_OWNER)
            };

            if let Err(e) = state.put_account(&validator_pubkey, &bootstrap_account) {
                eprintln!("Failed to create validator account: {e}");
            }
            info!(
                "✓ Bootstrap validator account created (grant #{}/{}): {} MOLT total",
                bootstrap_grants_issued + 1,
                moltchain_core::consensus::MAX_BOOTSTRAP_VALIDATORS,
                bootstrap_account.balance_molt()
            );
        } else {
            // Post-bootstrap phase: validator #201+ must bring their own stake
            info!(
                "📋 Bootstrap phase complete ({} grants issued). This validator must self-fund.",
                bootstrap_grants_issued
            );
            // Create empty account — validator needs external funding of BOOTSTRAP_GRANT_AMOUNT
            let empty_account = Account::new(0, SYSTEM_ACCOUNT_OWNER);
            if let Err(e) = state.put_account(&validator_pubkey, &empty_account) {
                eprintln!("Failed to create validator account: {e}");
            }
            info!(
                "✓ Validator account created (empty — requires {} MOLT deposit)",
                BOOTSTRAP_GRANT_AMOUNT / 1_000_000_000
            );
        }
    } else if let Some(account) = validator_account {
        info!(
            "✓ Validator account exists: {} MOLT",
            account.balance_molt()
        );
        info!(
            "   Spendable: {:.2} | Staked: {:.2} | Locked: {:.2}",
            account.spendable as f64 / 1_000_000_000.0,
            account.staked as f64 / 1_000_000_000.0,
            account.locked as f64 / 1_000_000_000.0
        );
    }

    // Initialize vote aggregator for BFT consensus
    let vote_aggregator = Arc::new(RwLock::new(VoteAggregator::new()));
    info!("🗳️  BFT voting system initialized");

    // Initialize finality tracker — lock-free commitment level tracking
    let initial_confirmed = state.get_last_confirmed_slot().unwrap_or(0);
    let initial_finalized = state.get_last_finalized_slot().unwrap_or(0);
    let finality_tracker = FinalityTracker::new(initial_confirmed, initial_finalized);
    info!(
        "🔒 Finality tracker initialized (confirmed={}, finalized={})",
        initial_confirmed, initial_finalized
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

    // Stake tokens for this validator (100,000 MOLT minimum)
    // Uses get_stake() to avoid accumulating on every restart
    {
        let mut pool = stake_pool.write().await;
        let current_slot = state.get_last_slot().unwrap_or(0);
        let existing = pool
            .get_stake(&validator_pubkey)
            .map(|s| s.amount)
            .unwrap_or(0);
        if existing >= BOOTSTRAP_GRANT_AMOUNT {
            info!("✅ Already staked: {} MOLT", existing / 1_000_000_000);

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
        } else {
            // New validator — atomic: validate fingerprint → allocate index → stake → register
            match pool.try_bootstrap_with_fingerprint(
                validator_pubkey,
                BOOTSTRAP_GRANT_AMOUNT,
                current_slot,
                machine_fingerprint,
            ) {
                Ok((bootstrap_index, _is_new)) => {
                    if bootstrap_index < moltchain_core::consensus::MAX_BOOTSTRAP_VALIDATORS {
                        info!(
                            "🦞 Bootstrap validator #{} — debt-based stake with graduation",
                            bootstrap_index + 1
                        );
                    } else {
                        info!("📋 Post-bootstrap validator — self-funded, no debt");
                    }
                    info!(
                        "💰 Staked {} MOLT (bootstrap grant)",
                        BOOTSTRAP_GRANT_AMOUNT / 1_000_000_000
                    );
                    if machine_fingerprint != [0u8; 32] {
                        info!("🔒 Machine fingerprint registered");
                    }
                }
                Err(e) => {
                    warn!("⚠️  Failed to stake: {}", e);
                }
            }

            info!("💰 Validator is now economically secured");
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
    let mut slot = if last_slot == 0 { 1 } else { last_slot + 1 };
    info!("Starting from slot {}", slot);

    // Get parent hash - if joining network and no genesis yet, use placeholder
    let mut parent_hash = if slot == 1 {
        if let Ok(Some(genesis)) = state.get_block_by_slot(0) {
            genesis.hash()
        } else {
            // No genesis yet (joining network) - will be set when genesis syncs
            Hash::default()
        }
    } else {
        state
            .get_block_by_slot(slot - 1)
            .ok()
            .flatten()
            .map(|b| b.hash())
            .unwrap_or_else(|| {
                warn!("⚠️  Could not load previous block at slot {}", slot - 1);
                Hash::default()
            })
    };

    let needs_genesis = is_joining_network; // Track if we need to request genesis

    // Create channels for P2P communication
    // M11: Bounded channels prevent memory exhaustion from slow consumers.
    // Capacity tiers: high-throughput (txs/votes) → larger, control msgs → smaller.
    // Block channel sized at 2000 to absorb sync bursts without backpressure
    // killing the P2P message loop (the old 500 was too small during catch-up).
    let (block_tx, mut block_rx) = mpsc::channel(10_000);
    let block_tx_for_compact = block_tx.clone(); // P3-3: sender for reconstructed compact blocks
    let block_tx_for_erasure = block_tx.clone(); // P3-4: sender for erasure-reconstructed blocks
    let (vote_tx, mut vote_rx) = mpsc::channel(2_000);
    let (transaction_tx, mut transaction_rx) = mpsc::channel(5_000);
    let (validator_announce_tx, mut validator_announce_rx) = mpsc::channel(100);
    let (block_range_request_tx, mut block_range_request_rx) = mpsc::channel(200);
    let (status_request_tx, mut status_request_rx) = mpsc::channel::<StatusRequestMsg>(100);
    let (status_response_tx, mut status_response_rx) = mpsc::channel::<StatusResponseMsg>(100);
    let (consistency_report_tx, mut consistency_report_rx) =
        mpsc::channel::<ConsistencyReportMsg>(50);
    let (snapshot_request_tx, mut snapshot_request_rx) = mpsc::channel::<SnapshotRequestMsg>(50);
    let (snapshot_response_tx, mut snapshot_response_rx) = mpsc::channel::<SnapshotResponseMsg>(50);
    let (slashing_evidence_tx, mut slashing_evidence_rx) =
        mpsc::channel::<moltchain_core::SlashingEvidence>(100);
    let (compact_block_tx, mut compact_block_rx) =
        mpsc::channel::<moltchain_p2p::CompactBlockMsg>(1_000);
    let (get_block_txs_tx, mut get_block_txs_rx) =
        mpsc::channel::<moltchain_p2p::GetBlockTxsMsg>(200);
    let (erasure_shard_request_tx, mut erasure_shard_request_rx) =
        mpsc::channel::<moltchain_p2p::ErasureShardRequestMsg>(200);
    let (erasure_shard_response_tx, mut erasure_shard_response_rx) =
        mpsc::channel::<moltchain_p2p::ErasureShardResponseMsg>(200);

    // Create mempool
    let mempool = Arc::new(Mutex::new(Mempool::new(50_000, 300))); // 50K tx max, 300s expiration — handles 5000 concurrent trader bursts

    // Start P2P network - need to extract peer manager before starting
    let (p2p_peer_manager, _p2p_handle) = match P2PNetwork::new(
        p2p_config.clone(),
        block_tx,
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
    let snapshot_sync = Arc::new(Mutex::new(SnapshotSync::new(is_joining_network)));

    // FIX-FORK-1: Shared set of slots where we received a valid block from the
    // network.  The block-receiver task inserts here; the production loop checks
    // before creating its own block, closing the TOCTOU race between the early
    // `get_block_by_slot` guard and the actual `Block::new` call.
    let received_network_slots: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
    let received_network_slots_for_blocks = received_network_slots.clone();
    let received_network_slots_for_producer = received_network_slots.clone();

    // Track last block time for leader timeout handling
    let last_block_time = Arc::new(Mutex::new(std::time::Instant::now()));
    let last_block_time_for_blocks = last_block_time.clone();
    let last_block_time_for_local = last_block_time.clone();
    let global_last_user_tx_activity = Arc::new(Mutex::new(std::time::Instant::now()));
    let global_last_user_tx_activity_for_blocks = global_last_user_tx_activity.clone();
    let global_last_user_tx_activity_for_producer = global_last_user_tx_activity.clone();

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
            while let Some(block) = block_rx.recv().await {
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
                if block_slot > 0 {
                    let vs = validator_set_for_blocks.read().await;
                    if vs.get_validator(&Pubkey(block.header.validator)).is_none() {
                        warn!(
                            "⚠️  Rejecting block {} — validator {} not in active set",
                            block_slot,
                            Pubkey(block.header.validator).to_base58()
                        );
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
                                    MessageType::SlashingEvidence(evidence),
                                    local_addr,
                                );
                                peer_mgr_for_sync.broadcast(evidence_msg).await;
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

                // FIX-FORK-1: Record that this slot has a valid network block.
                // MOVED: Insert into received_network_slots ONLY when the block
                // is actually applied (chaining onto the tip), not on receipt.
                // Previously, unchainable blocks from divergent peers poisoned
                // the set, permanently blocking production for those slots.
                // The insert now happens at each set_last_slot() call below.
                let current_slot = state_for_blocks.get_last_slot().unwrap_or(0);

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
                                    .rent_rate_shells_per_kb_month,
                                genesis_config_for_blocks.features.rent_free_kb,
                            )
                            .ok();

                        // 2. Fee config from genesis config
                        let gc = &genesis_config_for_blocks;
                        let genesis_fee_config = FeeConfig {
                            base_fee: gc.features.base_fee_shells,
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
                            let total_supply_molt = 1_000_000_000u64;
                            let total_shells = Account::molt_to_shells(total_supply_molt);
                            let mut total_distributed_shells = 0u64;

                            for (i, tx) in block.transactions.iter().enumerate().skip(1) {
                                if let Some(ix) = tx.message.instructions.first() {
                                    if ix.data.first() == Some(&4) && ix.accounts.len() >= 2 {
                                        let recipient = ix.accounts[1];
                                        let amount_shells = if ix.data.len() >= 9 {
                                            u64::from_le_bytes(
                                                ix.data[1..9].try_into().unwrap_or([0u8; 8]),
                                            )
                                        } else {
                                            0
                                        };

                                        let mut acct = Account::new(0, SYSTEM_ACCOUNT_OWNER);
                                        acct.shells = amount_shells;
                                        acct.spendable = amount_shells;
                                        state_for_blocks.put_account(&recipient, &acct).ok();
                                        total_distributed_shells += amount_shells;

                                        // tx[1] = treasury (validator_rewards) — works for both old and new genesis
                                        if i == 1 {
                                            state_for_blocks.set_treasury_pubkey(&recipient).ok();
                                            info!(
                                                "  ✓ 📡 [sync] Treasury: {} ({} MOLT)",
                                                recipient.to_base58(),
                                                amount_shells / 1_000_000_000
                                            );
                                        } else {
                                            info!(
                                                "  ✓ 📡 [sync] Distribution {}: {} ({} MOLT)",
                                                i,
                                                recipient.to_base58(),
                                                amount_shells / 1_000_000_000
                                            );
                                        }
                                    }
                                }
                            }

                            // 5. Reconstruct genesis account (total supply minus all distributions)
                            let mut genesis_account = Account::new(total_supply_molt, gpk);
                            genesis_account.shells =
                                total_shells.saturating_sub(total_distributed_shells);
                            genesis_account.spendable = genesis_account
                                .shells
                                .saturating_sub(genesis_account.staked)
                                .saturating_sub(genesis_account.locked);
                            state_for_blocks.put_account(&gpk, &genesis_account).ok();
                            state_for_blocks.set_genesis_pubkey(&gpk).ok();
                            info!(
                                "  ✓ 📡 [sync] Genesis account: {} ({} MOLT remaining)",
                                gpk.to_base58(),
                                genesis_account.shells / 1_000_000_000
                            );

                            // 6. Create initial accounts from genesis config
                            for account_info in &genesis_config_for_blocks.initial_accounts {
                                if let Ok(pubkey) = Pubkey::from_base58(&account_info.address) {
                                    let account = Account::new(account_info.balance_molt, pubkey);
                                    state_for_blocks.put_account(&pubkey, &account).ok();
                                }
                            }

                            // 7. Genesis transactions already stored + indexed
                            //    by put_block() above (CF_TRANSACTIONS + CF_TX_TO_SLOT
                            //    + CF_TX_BY_SLOT in one atomic WriteBatch).

                            // 8. Auto-deploy contracts
                            genesis_auto_deploy(&state_for_blocks, &gpk, "📡 [sync]");
                            genesis_initialize_contracts(&state_for_blocks, &gpk, "📡 [sync]");
                            genesis_create_trading_pairs(&state_for_blocks, &gpk, "📡 [sync]");
                            genesis_seed_oracle(&state_for_blocks, &gpk, "📡 [sync]");
                            genesis_seed_margin_prices(&state_for_blocks, &gpk);
                            genesis_seed_analytics_prices(&state_for_blocks, &gpk);

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
                        let pending = sync_mgr.try_apply_pending(0).await;
                        let mut chain_broken = false;
                        for pending_block in pending {
                            if chain_broken {
                                sync_mgr.add_pending_block(pending_block).await;
                                continue;
                            }
                            let pending_slot = pending_block.header.slot;
                            let tip = state_for_blocks.get_last_slot().unwrap_or(0);
                            let parent_ok = state_for_blocks
                                .get_block_by_slot(tip)
                                .ok()
                                .flatten()
                                .map(|tip_block| {
                                    pending_block.header.parent_hash == tip_block.hash()
                                })
                                .unwrap_or(false);
                            if !parent_ok {
                                chain_broken = true;
                                sync_mgr.add_pending_block(pending_block).await;
                                continue;
                            }
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
                            if state_for_blocks.put_block(&pending_block).is_ok() {
                                state_for_blocks.set_last_slot(pending_slot).ok();
                                *last_block_time_for_blocks.lock().await =
                                    std::time::Instant::now();
                                info!("✅ Applied pending block {}", pending_slot);
                                apply_block_effects(
                                    &state_for_blocks,
                                    &validator_set_for_blocks,
                                    &stake_pool_for_blocks,
                                    &vote_agg_for_effects,
                                    &pending_block,
                                    false,
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
                        // Valid next block in chain - replay transactions then store
                        // P1-1: Skip TX replay in header-only sync for far-away blocks.
                        if sync_mgr.should_full_validate(block_slot).await {
                            replay_block_transactions(&processor_for_blocks, &block);
                        }
                        run_analytics_bridge_from_state(
                            &state_for_blocks,
                            block.header.slot,
                            genesis_config_for_blocks.consensus.slot_duration_ms.max(1),
                        );
                        run_sltp_triggers_from_state(&state_for_blocks);
                        reset_24h_stats_if_expired(&state_for_blocks, block.header.timestamp);
                        if state_for_blocks.put_block(&block).is_ok() {
                            state_for_blocks.set_last_slot(block_slot).ok();
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
                            // After each applied block the chain tip advances,
                            // so check pending blocks against the UPDATED tip.
                            let pending = sync_mgr.try_apply_pending(block_slot).await;
                            let mut chain_broken = false;
                            for pending_block in pending {
                                if chain_broken {
                                    sync_mgr.add_pending_block(pending_block).await;
                                    continue;
                                }
                                let pending_slot = pending_block.header.slot;
                                let tip = state_for_blocks.get_last_slot().unwrap_or(0);
                                let parent_ok = state_for_blocks
                                    .get_block_by_slot(tip)
                                    .ok()
                                    .flatten()
                                    .map(|tip_block| {
                                        pending_block.header.parent_hash == tip_block.hash()
                                    })
                                    .unwrap_or(false);
                                if !parent_ok {
                                    chain_broken = true;
                                    sync_mgr.add_pending_block(pending_block).await;
                                    continue;
                                }
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
                                if state_for_blocks.put_block(&pending_block).is_ok() {
                                    state_for_blocks.set_last_slot(pending_slot).ok();
                                    *last_block_time_for_blocks.lock().await =
                                        std::time::Instant::now();
                                    info!("✅ Applied pending block {}", pending_slot);
                                    apply_block_effects(
                                        &state_for_blocks,
                                        &validator_set_for_blocks,
                                        &stake_pool_for_blocks,
                                        &vote_agg_for_effects,
                                        &pending_block,
                                        false,
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

                        // Complete sync flag after a delay.
                        // STABILITY-FIX: Wait 5s instead of 2s, then check if
                        // we actually received some blocks before completing.
                        // If still behind, re-trigger sync instead of silently
                        // marking sync complete.
                        let sync_mgr_complete = sync_mgr.clone();
                        let state_for_sync_check = state_for_blocks.clone();
                        let sync_end = end;
                        tokio::spawn(async move {
                            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                            let current = state_for_sync_check.get_last_slot().unwrap_or(0);
                            let highest = sync_mgr_complete.get_highest_seen().await;
                            if current < sync_end && highest > current + 2 {
                                info!(
                                    "🔄 Sync batch incomplete (at {}, target {}), will re-trigger",
                                    current, sync_end
                                );
                                // Record failure for exponential backoff
                                sync_mgr_complete.record_sync_failure().await;
                            } else {
                                // Successful sync progress — reset backoff
                                sync_mgr_complete.record_sync_success().await;
                            }
                            // Always complete to allow re-trigger
                            sync_mgr_complete.complete_sync().await;
                        });
                    }
                } else if block_slot <= current_slot {
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
                                if state_for_blocks.put_block(&block).is_ok() {
                                    state_for_blocks.set_last_slot(current_slot).ok();
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
                                    let pending = sync_mgr.try_apply_pending(block_slot).await;
                                    let mut chain_broken = false;
                                    for pending_block in pending {
                                        if chain_broken {
                                            sync_mgr.add_pending_block(pending_block).await;
                                            continue;
                                        }
                                        let pending_slot = pending_block.header.slot;
                                        let tip = state_for_blocks.get_last_slot().unwrap_or(0);
                                        let parent_ok = state_for_blocks
                                            .get_block_by_slot(tip)
                                            .ok()
                                            .flatten()
                                            .map(|tip_block| {
                                                pending_block.header.parent_hash == tip_block.hash()
                                            })
                                            .unwrap_or(false);
                                        if !parent_ok {
                                            chain_broken = true;
                                            sync_mgr.add_pending_block(pending_block).await;
                                            continue;
                                        }
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
                                        if state_for_blocks.put_block(&pending_block).is_ok() {
                                            state_for_blocks.set_last_slot(pending_slot).ok();
                                            *last_block_time_for_blocks.lock().await =
                                                std::time::Instant::now();
                                            info!(
                                                "✅ Applied pending block {} (after fork adoption)",
                                                pending_slot
                                            );
                                            apply_block_effects(
                                                &state_for_blocks,
                                                &validator_set_for_blocks,
                                                &stake_pool_for_blocks,
                                                &vote_agg_for_effects,
                                                &pending_block,
                                                false,
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
        let state_for_p2p_txs = state.clone();
        tokio::spawn(async move {
            info!("🔄 Transaction receiver started");
            while let Some(tx) = transaction_rx.recv().await {
                info!("📥 Received transaction from P2P");
                // AUDIT-FIX 1.6: Validate transaction before adding to mempool
                // 1. Verify all required signatures (first account of each instruction)
                let sender_pubkey = tx
                    .message
                    .instructions
                    .first()
                    .and_then(|ix| ix.accounts.first())
                    .copied();
                if !validate_p2p_transaction_signatures(&tx) {
                    info!("❌ P2P transaction rejected: invalid or missing signature");
                    continue;
                }
                // 2. Validate transaction structure (size limits, instruction count)
                if let Err(e) = tx.validate_structure() {
                    info!("❌ P2P transaction rejected: {}", e);
                    continue;
                }
                // AUDIT-FIX V5.3: Look up on-chain MoltyID reputation
                // so express-lane priority works for P2P-received transactions.
                // Do not reject based on local account balance here: peers can be
                // briefly behind in state sync, and strict pre-checks cause mempool
                // imbalance (one validator receives TXs, others drop them).
                let reputation = sender_pubkey
                    .as_ref()
                    .and_then(|pk| state_for_p2p_txs.get_reputation(pk).ok())
                    .unwrap_or(0);
                let mut pool = mempool_for_txs.lock().await;
                if let Err(e) = pool.add_transaction(tx, BASE_FEE, reputation) {
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

        tokio::spawn(async move {
            info!("🔄 Vote receiver started");

            // Track votes per validator to detect double-voting
            let mut validator_votes: std::collections::HashMap<
                (moltchain_core::Pubkey, u64),
                Vote,
            > = std::collections::HashMap::new();

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
                                    MessageType::SlashingEvidence(evidence),
                                    local_addr_for_slash,
                                );
                                peer_mgr.broadcast(evidence_msg).await;
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
        tokio::spawn(async move {
            info!("🔄 Validator announcement receiver started");
            // 1.5c+d: Rate limiting — per-epoch bootstrap cap and per-minute announcement limit
            let mut bootstrap_epoch: u64 = 0;
            let mut bootstrap_count: u64 = 0;
            let mut last_announce_times: std::collections::HashMap<
                moltchain_core::account::Pubkey,
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

                    // 1.5b: Check on-chain staked balance before granting bootstrap
                    let existing_account = state_for_validators
                        .get_account(&announcement.pubkey)
                        .unwrap_or(None);
                    let already_staked = existing_account.as_ref().map(|a| a.staked).unwrap_or(0);

                    // 1.5c: Per-epoch cap on bootstrap grants (max 10 per epoch)
                    const MAX_BOOTSTRAPS_PER_EPOCH: u64 = 10;
                    let current_epoch = announcement.current_slot / SLOTS_PER_EPOCH;
                    if current_epoch != bootstrap_epoch {
                        bootstrap_epoch = current_epoch;
                        bootstrap_count = 0;
                    }

                    let needs_bootstrap = already_staked < BOOTSTRAP_GRANT_AMOUNT;

                    if needs_bootstrap && bootstrap_count >= MAX_BOOTSTRAPS_PER_EPOCH {
                        warn!(
                            "⚠️  Bootstrap cap reached for epoch {} — rejecting {}",
                            current_epoch,
                            announcement.pubkey.to_base58()
                        );
                        drop(vs);
                        continue;
                    }

                    // Add new validator
                    let new_validator = ValidatorInfo {
                        pubkey: announcement.pubkey,
                        reputation: 100,
                        blocks_proposed: 0,
                        votes_cast: 0,
                        correct_votes: 0,
                        stake: if already_staked >= BOOTSTRAP_GRANT_AMOUNT {
                            already_staked
                        } else {
                            BOOTSTRAP_GRANT_AMOUNT
                        },
                        joined_slot: announcement.current_slot,
                        last_active_slot: announcement.current_slot,
                        commission_rate: 500,
                        transactions_processed: 0,
                    };
                    vs.add_validator(new_validator);

                    // Also stake in local pool so leader election can pick them
                    // AUDIT-FIX C4/H13: For self-funded validators, stake immediately.
                    // For bootstrap validators, defer stake pool entry until AFTER treasury
                    // debit succeeds — prevents inflating active stake with unfunded entries.
                    if !needs_bootstrap {
                        // Self-funded: stake immediately in local pool
                        let mut pool = stake_pool_for_announce.write().await;
                        if pool.get_stake(&announcement.pubkey).is_none() {
                            let fingerprint = announcement.machine_fingerprint;
                            match pool.try_bootstrap_with_fingerprint(
                                announcement.pubkey,
                                already_staked,
                                announcement.current_slot,
                                fingerprint,
                            ) {
                                Ok(_) => {
                                    info!(
                                        "💰 Self-funded validator staked in local pool ({} MOLT, no debt)",
                                        already_staked / 1_000_000_000
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        "⚠️  Failed to stake joining validator {}: {}",
                                        announcement.pubkey.to_base58(),
                                        e
                                    );
                                }
                            }
                        }
                    }

                    // Bootstrap account only if the validator doesn't already have sufficient stake
                    // AND we're still in the bootstrap phase (first 200 validators)
                    if needs_bootstrap {
                        // Check bootstrap cap from stake pool
                        let bootstrap_grants = {
                            let pool = stake_pool_for_announce.read().await;
                            pool.bootstrap_grants_issued()
                        };
                        let is_bootstrap_eligible =
                            bootstrap_grants < moltchain_core::consensus::MAX_BOOTSTRAP_VALIDATORS;

                        if !is_bootstrap_eligible {
                            info!(
                                "📋 Bootstrap phase complete ({} grants). Validator {} must self-fund.",
                                bootstrap_grants,
                                announcement.pubkey.to_base58()
                            );
                        } else {
                            // AUDIT-FIX C4/H13: Deduct from treasury FIRST, only stake if funded
                            let mut funded = false;
                            if let Ok(Some(tpk)) = state_for_validators.get_treasury_pubkey() {
                                if let Ok(Some(mut treasury)) =
                                    state_for_validators.get_account(&tpk)
                                {
                                    if treasury.spendable >= BOOTSTRAP_GRANT_AMOUNT {
                                        treasury.deduct_spendable(BOOTSTRAP_GRANT_AMOUNT).ok();
                                        if let Err(e) =
                                            state_for_validators.put_account(&tpk, &treasury)
                                        {
                                            warn!("⚠️  Failed to debit treasury for remote bootstrap: {}", e);
                                        } else {
                                            funded = true;
                                        }
                                    } else {
                                        warn!("⚠️  Treasury insufficient for remote validator bootstrap ({} < {})",
                                            treasury.spendable, BOOTSTRAP_GRANT_AMOUNT);
                                    }
                                }
                            }

                            if funded {
                                // Treasury debit succeeded — NOW credit stake pool
                                {
                                    let mut pool = stake_pool_for_announce.write().await;
                                    if pool.get_stake(&announcement.pubkey).is_none() {
                                        let fingerprint = announcement.machine_fingerprint;
                                        match pool.try_bootstrap_with_fingerprint(
                                            announcement.pubkey,
                                            BOOTSTRAP_GRANT_AMOUNT,
                                            announcement.current_slot,
                                            fingerprint,
                                        ) {
                                            Ok((bootstrap_index, _)) => {
                                                info!(
                                                    "💰 Bootstrap validator #{} staked in local pool ({} MOLT, with debt)",
                                                    bootstrap_index + 1,
                                                    BOOTSTRAP_GRANT_AMOUNT / 1_000_000_000
                                                );
                                            }
                                            Err(e) => {
                                                warn!(
                                                    "⚠️  Failed to stake bootstrap validator {}: {}",
                                                    announcement.pubkey.to_base58(),
                                                    e
                                                );
                                                // Reverse treasury debit since stake failed
                                                if let Ok(Some(tpk)) =
                                                    state_for_validators.get_treasury_pubkey()
                                                {
                                                    if let Ok(Some(mut treasury)) =
                                                        state_for_validators.get_account(&tpk)
                                                    {
                                                        treasury
                                                            .add_spendable(BOOTSTRAP_GRANT_AMOUNT)
                                                            .ok();
                                                        let _ = state_for_validators
                                                            .put_account(&tpk, &treasury);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                let mut bootstrap_account = Account {
                                    shells: BOOTSTRAP_GRANT_AMOUNT,
                                    spendable: 0,
                                    staked: BOOTSTRAP_GRANT_AMOUNT,
                                    locked: 0,
                                    data: Vec::new(),
                                    owner: SYSTEM_ACCOUNT_OWNER,
                                    executable: false,
                                    rent_epoch: 0,
                                };
                                // Preserve any existing spendable balance (from block rewards)
                                if let Some(existing) = &existing_account {
                                    bootstrap_account.shells += existing.spendable;
                                    bootstrap_account.spendable = existing.spendable;
                                }
                                if let Err(e) = state_for_validators
                                    .put_account(&announcement.pubkey, &bootstrap_account)
                                {
                                    warn!(
                                        "⚠️  Failed to create bootstrap account for {}: {}",
                                        announcement.pubkey, e
                                    );
                                } else {
                                    bootstrap_count += 1;
                                    info!(
                                        "💰 Created bootstrap account for validator {} ({} MOLT staked, treasury debited) [{}/{}]",
                                        announcement.pubkey.to_base58(),
                                        BOOTSTRAP_GRANT_AMOUNT / 1_000_000_000,
                                        bootstrap_count,
                                        MAX_BOOTSTRAPS_PER_EPOCH,
                                    );
                                }
                            } else {
                                warn!("⚠️  Skipping bootstrap account for {} — treasury unavailable or insufficient",
                                    announcement.pubkey.to_base58());
                            }
                        }
                    } else {
                        info!(
                            "✅ Validator {} already has sufficient on-chain stake ({}), skipping bootstrap",
                            announcement.pubkey.to_base58(),
                            already_staked
                        );
                    }
                }

                // Persist to state
                if let Err(e) = state_for_validators.save_validator_set(&vs) {
                    warn!("⚠️  Failed to save validator set: {}", e);
                } else {
                    let count = vs.validators().len();
                    info!("✅ Updated validator set (now {} validators)", count);
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
                    let (slot, state_root, total_accounts) =
                        match StateStore::latest_checkpoint(&data_dir_for_snapshot) {
                            Some((meta, _)) => (meta.slot, meta.state_root, meta.total_accounts),
                            None => (0, [0u8; 32], 0),
                        };
                    let msg = P2PMessage::new(
                        MessageType::CheckpointMetaResponse {
                            slot,
                            state_root,
                            total_accounts,
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
                    // Find latest checkpoint and serve from it
                    let checkpoint_store =
                        match StateStore::latest_checkpoint(&data_dir_for_snapshot) {
                            Some((meta, path)) => match StateStore::open_checkpoint(&path) {
                                Ok(store) => Some((store, meta)),
                                Err(e) => {
                                    warn!("⚠️  Failed to open checkpoint for snapshot: {}", e);
                                    None
                                }
                            },
                            None => {
                                // No checkpoint — serve from live state (less ideal but functional)
                                let meta = moltchain_core::CheckpointMeta {
                                    slot: state_for_snapshot_serve.get_last_slot().unwrap_or(0),
                                    state_root: state_for_snapshot_serve.compute_state_root().0,
                                    created_at: 0,
                                    total_accounts: state_for_snapshot_serve
                                        .count_accounts()
                                        .unwrap_or(0),
                                };
                                Some((state_for_snapshot_serve.clone(), meta))
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
                                    _ => Ok(moltchain_core::state::KvPage {
                                        entries: Vec::new(),
                                        total: 0,
                                        next_cursor: None,
                                        has_more: false,
                                    }),
                                }
                                .unwrap_or_else(|_| moltchain_core::state::KvPage {
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
                            _ => Ok(moltchain_core::state::KvPage {
                                entries: Vec::new(),
                                total: 0,
                                next_cursor: None,
                                has_more: false,
                            }),
                        }
                        .unwrap_or_else(|_| {
                            moltchain_core::state::KvPage {
                                entries: Vec::new(),
                                total: 0,
                                next_cursor: None,
                                has_more: false,
                            }
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
                        let (slot, state_root, total_accounts) =
                            match StateStore::latest_checkpoint(&data_dir_for_snapshot) {
                                Some((meta, _)) => {
                                    (meta.slot, meta.state_root, meta.total_accounts)
                                }
                                None => (0, [0u8; 32], 0),
                            };
                        P2PMessage::new(
                            MessageType::CheckpointMetaResponse {
                                slot,
                                state_root,
                                total_accounts,
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

            while let Some(response) = snapshot_response_rx.recv().await {
                // Handle CheckpointMetaResponse
                if let Some((slot, _state_root, total_accounts)) = response.checkpoint_meta {
                    if slot > 0 && total_accounts > 0 {
                        info!(
                            "📋 Peer {} has checkpoint at slot {} ({} accounts)",
                            response.requester, slot, total_accounts
                        );
                        let local_slot = state_for_snapshot_apply.get_last_slot().unwrap_or(0);
                        if slot > local_slot + 100 {
                            // Peer is significantly ahead — request state snapshot
                            info!(
                                "🔄 Requesting state snapshot from {} (local slot {}, peer slot {})",
                                response.requester, local_slot, slot
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
                        warn!(
                            "📋 Peer {} has no checkpoint available — falling back to block-range sync",
                            response.requester
                        );
                        // Warp sync is impossible without a checkpoint.  Complete the
                        // current sync batch and switch to HeaderOnly so the next
                        // should_sync() call can re-trigger with block-range requests.
                        let current_mode = sync_mgr_for_snapshot.get_sync_mode().await;
                        if current_mode == crate::sync::SyncMode::Warp {
                            sync_mgr_for_snapshot
                                .set_sync_mode(crate::sync::SyncMode::HeaderOnly)
                                .await;
                            sync_mgr_for_snapshot.complete_sync().await;
                            sync_mgr_for_snapshot.record_sync_failure().await;
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
                    _state_root,
                    ref entries_bytes,
                )) = response.state_snapshot_data
                {
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
                        let expected_root = _state_root;
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
                                            local_val.stake = remote_val.stake;
                                        }
                                        merged_count += 1;
                                    } else {
                                        // AUDIT-FIX 2.11 (revised): Verify unknown validators
                                        // from snapshot by checking on-chain staked balance.
                                        // Only add if they have sufficient on-chain stake,
                                        // which proves they went through the proper bootstrap
                                        // or self-funding process on another validator.
                                        let has_verified_stake = state_for_snapshot_apply
                                            .get_account(&remote_val.pubkey)
                                            .unwrap_or(None)
                                            .map(|a| a.staked >= MIN_VALIDATOR_STAKE)
                                            .unwrap_or(false);

                                        if has_verified_stake {
                                            // On-chain stake verified — safe to add
                                            let new_val = ValidatorInfo {
                                                pubkey: remote_val.pubkey,
                                                reputation: 100,
                                                blocks_proposed: remote_val.blocks_proposed,
                                                votes_cast: remote_val.votes_cast,
                                                correct_votes: remote_val.correct_votes,
                                                stake: remote_val.stake,
                                                joined_slot: remote_val.joined_slot,
                                                last_active_slot: remote_val.last_active_slot,
                                                commission_rate: 500,
                                                transactions_processed: 0,
                                            };
                                            vs.add_validator(new_val);
                                            merged_count += 1;
                                            info!(
                                                "✅ Snapshot: added verified validator {} from peer {} (on-chain stake confirmed)",
                                                remote_val.pubkey.to_base58(),
                                                response.requester
                                            );
                                        } else {
                                            // No on-chain stake — reject (prevents injection)
                                            warn!(
                                                "⚠️  Snapshot: rejecting unverified validator {} from peer {} (no on-chain stake)",
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
                                    snapshot_sync_for_apply.lock().await.validator_set = true;
                                }
                                drop(vs);
                            } else {
                                // Hashes match — local set is already correct (from block replay).
                                // Still mark snapshot as ready so the producer loop can proceed.
                                snapshot_sync_for_apply.lock().await.validator_set = true;
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
                                    pool.upsert_stake_full(entry.clone());
                                    merged_count += 1;

                                    // Create bootstrap account for this validator if it doesn't exist locally
                                    // This ensures V1 knows about V2/V3's staked accounts (and vice versa)
                                    let existing_account = state_for_snapshot_apply
                                        .get_account(&entry.validator)
                                        .unwrap_or(None);
                                    let needs_bootstrap = match &existing_account {
                                        None => true,
                                        Some(acct) => {
                                            acct.staked == 0 && entry.amount >= MIN_VALIDATOR_STAKE
                                        }
                                    };
                                    if needs_bootstrap {
                                        // Deduct from treasury — same as announce handler
                                        let mut funded = false;
                                        if let Ok(Some(tpk)) =
                                            state_for_snapshot_apply.get_treasury_pubkey()
                                        {
                                            if let Ok(Some(mut treasury)) =
                                                state_for_snapshot_apply.get_account(&tpk)
                                            {
                                                if treasury.spendable >= entry.amount {
                                                    treasury.deduct_spendable(entry.amount).ok();
                                                    if let Err(e) = state_for_snapshot_apply
                                                        .put_account(&tpk, &treasury)
                                                    {
                                                        warn!("⚠️  Failed to debit treasury for snapshot bootstrap: {}", e);
                                                    } else {
                                                        funded = true;
                                                    }
                                                }
                                            }
                                        }

                                        if funded {
                                            // Construct account directly with staked amount in shells
                                            // (avoids MOLT<->shells rounding issues)
                                            let mut bootstrap_account = Account {
                                                shells: entry.amount,
                                                spendable: 0,
                                                staked: entry.amount,
                                                locked: 0,
                                                data: Vec::new(),
                                                owner: SYSTEM_ACCOUNT_OWNER,
                                                executable: false,
                                                rent_epoch: 0,
                                            };
                                            // Preserve any existing spendable balance (from block rewards)
                                            if let Some(existing) = &existing_account {
                                                bootstrap_account.shells += existing.spendable;
                                                bootstrap_account.spendable = existing.spendable;
                                            }
                                            if let Err(e) = state_for_snapshot_apply
                                                .put_account(&entry.validator, &bootstrap_account)
                                            {
                                                warn!("⚠️  Failed to create bootstrap account for {}: {}", entry.validator, e);
                                            } else {
                                                info!(
                                                "💰 Created bootstrap account for validator {} ({:.4} MOLT staked, treasury debited)",
                                                entry.validator,
                                                entry.amount as f64 / 1_000_000_000.0
                                            );
                                            }
                                        } else {
                                            warn!("⚠️  Insufficient treasury to bootstrap validator {} from snapshot ({:.4} MOLT needed)",
                                                entry.validator, entry.amount as f64 / 1_000_000_000.0);
                                        }
                                    }
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
                                    snapshot_sync_for_apply.lock().await.stake_pool = true;
                                }
                            } else {
                                // Hashes match — local pool is already correct (from block replay).
                                drop(pool);
                                snapshot_sync_for_apply.lock().await.stake_pool = true;
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

    // Parse --admin-token from CLI or MOLTCHAIN_ADMIN_TOKEN env var
    let admin_token: Option<String> = get_flag_value(&args, &["--admin-token"])
        .map(|s| s.to_string())
        .or_else(|| env::var("MOLTCHAIN_ADMIN_TOKEN").ok())
        .filter(|t| !t.is_empty());
    if admin_token.is_some() {
        info!("🔒 Admin token configured for state-mutating RPC endpoints");
    } else {
        warn!(
            "⚠️  No admin_token configured — all admin RPC endpoints are disabled. \
               Set MOLTCHAIN_ADMIN_TOKEN env var or --admin-token flag for production."
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
    let state_for_rpc_lookup = state.clone(); // AUDIT-FIX V5.2: reputation lookup
    let p2p_peer_manager_for_txs = p2p_peer_manager.clone();
    let p2p_config_for_txs = p2p_config.clone();
    tokio::spawn(async move {
        while let Some(tx) = rpc_tx_receiver.recv().await {
            info!("📨 RPC transaction received, adding to mempool");

            // P9-RPC-01: Defense-in-depth — reject sentinel blockhash for non-EVM TXs
            // before they even enter the mempool.  Only eth_sendRawTransaction may
            // submit TXs with the EVM sentinel; any other path is a bypass attempt.
            if tx.message.recent_blockhash == moltchain_core::Hash([0xEE; 32]) {
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

            // AUDIT-FIX V5.2: Look up on-chain MoltyID reputation so
            // high-reputation agents get express-lane mempool priority.
            let reputation = tx
                .message
                .instructions
                .first()
                .and_then(|ix| ix.accounts.first())
                .and_then(|sender| state_for_rpc_lookup.get_reputation(sender).ok())
                .unwrap_or(0);

            // Add to mempool
            {
                let mut pool = mempool_for_rpc_txs.lock().await;
                if let Err(e) = pool.add_transaction(tx.clone(), BASE_FEE, reputation) {
                    info!("Mempool add failed: {}", e);
                }
            }

            // Broadcast to P2P network
            if let Some(ref peer_mgr) = p2p_peer_manager_for_txs {
                let msg = moltchain_p2p::P2PMessage::new(
                    moltchain_p2p::MessageType::Transaction(tx),
                    p2p_config_for_txs.listen_addr,
                );
                peer_mgr.broadcast(msg).await;
                info!("📡 Broadcasted transaction to network");
            }
        }
    });

    let tx_sender_for_rpc = Some(rpc_tx_sender);
    let p2p_for_rpc: Option<Arc<dyn moltchain_rpc::P2PNetworkTrait>> =
        p2p_peer_manager.as_ref().map(|peer_mgr| {
            struct PeerAdapter {
                peer_mgr: Arc<moltchain_p2p::PeerManager>,
            }

            impl moltchain_rpc::P2PNetworkTrait for PeerAdapter {
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
            }) as Arc<dyn moltchain_rpc::P2PNetworkTrait>
        });

    // Start WebSocket server FIRST so we can share its broadcasters with RPC
    let (ws_event_tx, ws_dex_broadcaster, ws_prediction_broadcaster, _ws_handle) =
        match moltchain_rpc::start_ws_server(state_for_ws, ws_port).await {
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
                let (dummy_tx, _) = tokio::sync::broadcast::channel::<moltchain_rpc::ws::Event>(1);
                let dummy_broadcaster =
                    std::sync::Arc::new(moltchain_rpc::dex_ws::DexEventBroadcaster::new(1));
                let dummy_pred =
                    std::sync::Arc::new(moltchain_rpc::ws::PredictionEventBroadcaster::new(1));
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
    // and writes to moltoracle + dex_analytics storage every 1s when prices change.
    // Auto-reconnects with exponential backoff; falls back to REST API if WS is down.
    // Can be disabled via MOLTCHAIN_DISABLE_ORACLE=1 (e.g. if Binance is geo-blocked).
    // Create shared oracle prices — block production reads these atomics
    // to embed oracle prices in each block for consensus propagation.
    let shared_oracle_prices = SharedOraclePrices::new();

    let oracle_disabled = std::env::var("MOLTCHAIN_DISABLE_ORACLE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if oracle_disabled {
        info!("🔮 Oracle price feeder disabled via MOLTCHAIN_DISABLE_ORACLE");
    } else {
        let state_for_oracle = state.clone();
        spawn_oracle_price_feeder(
            state_for_oracle,
            shared_oracle_prices.clone(),
            ws_dex_broadcaster.clone(),
        );
    }

    info!("⚡ Starting consensus-based block production");
    info!("Validator: {}", validator_pubkey);
    info!(
        "Block time: {}ms",
        genesis_config.consensus.slot_duration_ms
    );
    info!(
        "Base fee: {} shells ({:.5} MOLT)",
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
                        .unwrap_or(BOOTSTRAP_GRANT_AMOUNT)
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
                let retain = moltchain_core::state::COLD_RETENTION_SLOTS;
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
                        "💰 Accumulated rewards: {:.3} MOLT (unclaimed)",
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
                "📊 Staking Stats | Total: {:.2} MOLT | Validators: {} | Unclaimed: {:.3} MOLT",
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
                            let request = moltchain_p2p::P2PMessage::new(
                                moltchain_p2p::MessageType::BlockRequest { slot },
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
                        let request = moltchain_p2p::P2PMessage::new(
                            moltchain_p2p::MessageType::GetBlockTxs {
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
                        let response = moltchain_p2p::P2PMessage::new(
                            moltchain_p2p::MessageType::BlockTxs {
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
                        match moltchain_p2p::erasure::encode_shards(slot, &serialized) {
                            Ok(all_shards) => {
                                let requested: Vec<moltchain_p2p::erasure::ErasureShard> = msg
                                    .shard_indices
                                    .iter()
                                    .filter_map(|&idx| all_shards.get(idx).cloned())
                                    .collect();
                                let response = moltchain_p2p::P2PMessage::new(
                                    moltchain_p2p::MessageType::ErasureShardResponse {
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
            let mut shard_buffers: HashMap<u64, Vec<Option<moltchain_p2p::erasure::ErasureShard>>> =
                HashMap::new();
            while let Some(msg) = erasure_shard_response_rx.recv().await {
                let slot = msg.slot;
                let buffer = shard_buffers
                    .entry(slot)
                    .or_insert_with(|| vec![None; moltchain_p2p::erasure::TOTAL_SHARDS]);

                for shard in msg.shards {
                    let idx = shard.index;
                    if idx < buffer.len() {
                        buffer[idx] = Some(shard);
                    }
                }

                let present = buffer.iter().filter(|s| s.is_some()).count();
                if present >= moltchain_p2p::erasure::DATA_SHARDS {
                    match moltchain_p2p::erasure::decode_shards(buffer) {
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

    let last_block_time_for_watchdog = last_block_time.clone();
    let state_for_watchdog = state.clone();
    let sync_manager_for_watchdog = sync_manager.clone();
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

    // Track when we first discovered other validators (for stabilization wait)
    let mut first_announcement_time: Option<std::time::Instant> = None;
    let validator_set_stabilization = if !explicit_seed_peers.is_empty()
        && explicit_seed_peers
            .iter()
            .all(|addr| addr.ip().is_loopback())
    {
        Duration::from_secs(10)
    } else {
        Duration::from_secs(60)
    };

    // HEARTBEAT-FIX: Use the shared last_block_time (Arc<Mutex<Instant>>) instead
    // of a local-only timer. This ensures the heartbeat gate accounts for blocks
    // received from other validators via P2P, not just locally-produced blocks.
    // Without this, validators would produce heartbeats simultaneously because
    // each validator's local timer doesn't see network blocks.
    let mut slot_start = std::time::Instant::now();
    let mut last_attempted_slot: u64 = 0;

    // STABILITY-FIX: Track when the freeze-production guard first engaged.
    // If frozen for longer than MAX_FREEZE_DURATION_SECS, force-resume
    // production to break the death spiral.
    let mut freeze_started_at: Option<std::time::Instant> = None;

    // PERF-OPT 5: Leader election cache.
    // Cache the result of select_leader_weighted() to avoid acquiring RwLock
    // reads on validator_set + stake_pool every 2ms loop iteration (~500x/sec).
    // Invalidated when slot or view changes.
    let mut cached_leader: Option<(u64, u64, bool)> = None; // (slot, view, should_produce)

    // F6.2: Track DEX trade count for WS event emission
    let mut last_dex_trade_count = state.get_program_storage_u64("DEX", b"dex_trade_count");

    // SLOT TIMING FLOOR: Track when the last block was produced to enforce
    // minimum slot_duration_ms spacing between blocks. Without this, the 2ms
    // poll loop would produce blocks every ~3ms when the leader bypass is active.
    let mut last_block_produced_at: Option<std::time::Instant> = None;

    loop {
        // TIP-BASED SLOT: Always derive the next slot to produce from the chain tip.
        // This guarantees consecutive slot numbers — no gaps. Every validator agrees
        // on which slot comes next because they all read from the same chain.
        let tip_slot = state.get_last_slot().unwrap_or(0);
        slot = tip_slot + 1;

        // LEADER-SEED-FIX: Refresh parent_hash from chain tip BEFORE leader election.
        // Previously this was done AFTER leader election (after all `continue` guards),
        // meaning parent_hash could be stale for validators that weren't producing.
        // With a stale seed, validators disagree on leader selection → neither produces
        // → unnecessary view rotation delays and potential simultaneous production.
        // Now every validator uses the same parent_hash seed for the same tip.
        if tip_slot > 0 {
            if let Ok(Some(latest_block)) = state.get_block_by_slot(tip_slot) {
                parent_hash = latest_block.hash();
            }
        } else if let Ok(Some(genesis_block)) = state.get_block_by_slot(0) {
            parent_hash = genesis_block.hash();
        }

        // Reset view timer when chain tip advances (new slot to fill)
        if slot != last_attempted_slot {
            slot_start = std::time::Instant::now();
            last_attempted_slot = slot;
            // Invalidate leader cache when slot changes
            cached_leader = None;
        }

        // PERF-OPT 1: Event-driven wakeup instead of busy-poll.
        // Wait for either a tip-advance notification (from block receiver) or a
        // 2ms timeout.  This cuts average wakeup latency from 2.5ms to ~0ms
        // when blocks arrive, while still polling at 2ms for mempool changes
        // and heartbeat checks.
        tokio::select! {
            _ = tip_notify_for_producer.notified() => {},
            _ = time::sleep(Duration::from_millis(2)) => {},
        }

        // Broadcast slot event to WebSocket subscribers
        let _ = ws_event_tx.send(moltchain_rpc::ws::Event::Slot(slot));

        // Check if we need to wait for initial sync and validator discovery
        if is_joining_network {
            let has_genesis = state.get_block_by_slot(0).unwrap_or(None).is_some();
            if !has_genesis {
                // Still waiting for genesis sync — sleep 200ms instead of spinning at 2ms
                if slot_start.elapsed().as_secs() >= 5 {
                    info!(
                        "⏳ Waiting for genesis sync from network (tip: {})",
                        tip_slot
                    );
                    slot_start = std::time::Instant::now();
                    last_attempted_slot = slot;
                }
                time::sleep(Duration::from_millis(200)).await;
                continue;
            }

            let snapshot_ready = snapshot_sync.lock().await.is_ready();
            if !snapshot_ready {
                if slot_start.elapsed().as_secs() >= 5 {
                    info!(
                        "⏳ Waiting for validator/stake snapshots before producing (tip: {})",
                        tip_slot
                    );
                    slot_start = std::time::Instant::now();
                    last_attempted_slot = slot;
                }
                time::sleep(Duration::from_millis(200)).await;
                continue;
            } else {
                // Genesis synced! But wait for validator discovery AND full chain sync
                let vs = validator_set.read().await;
                let validator_count = vs.validators().len();
                drop(vs);

                if validator_count <= 1 {
                    // Still waiting for first validator announcement
                    if slot_start.elapsed().as_secs() >= 5 {
                        info!(
                            "⏳ Waiting for validator discovery (found {} validators)",
                            validator_count
                        );
                        slot_start = std::time::Instant::now();
                        last_attempted_slot = slot;
                    }
                    time::sleep(Duration::from_millis(200)).await;
                    continue;
                } else if first_announcement_time.is_none() {
                    // Just discovered validators! Start stabilization wait
                    first_announcement_time = Some(std::time::Instant::now());
                    info!(
                        "✅ Discovered {} validators. Waiting {}s for ValidatorSet stability...",
                        validator_count,
                        validator_set_stabilization.as_secs()
                    );
                    time::sleep(Duration::from_millis(200)).await;
                    continue;
                } else {
                    // Check if we've waited long enough for ValidatorSet to stabilize
                    let elapsed = first_announcement_time
                        .map(|t| t.elapsed())
                        .unwrap_or_default();
                    if elapsed < validator_set_stabilization {
                        if elapsed.as_secs().is_multiple_of(5)
                            && slot_start.elapsed().as_secs() >= 2
                        {
                            info!(
                                "⏳ ValidatorSet stabilizing... ({:.0}s / {}s, {} validators)",
                                elapsed.as_secs(),
                                validator_set_stabilization.as_secs(),
                                validator_count
                            );
                            slot_start = std::time::Instant::now();
                            last_attempted_slot = slot;
                        }
                        time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                }

                // ValidatorSet stable! Now wait until caught up with network
                let current_slot = state.get_last_slot().unwrap_or(0);
                if !sync_manager.is_caught_up(current_slot).await {
                    let network_slot = sync_manager.get_highest_seen().await;
                    // Only log every 5 seconds to avoid log spam during catch-up
                    if slot_start.elapsed().as_secs() >= 5 {
                        info!(
                            "⏳ Syncing to network (current: {}, network: {}, {} validators)",
                            current_slot, network_slot, validator_count
                        );
                        slot_start = std::time::Instant::now();
                        last_attempted_slot = slot;
                    }
                    // Sleep 100ms during catch-up instead of spinning at 2ms
                    // The P2P block receiver fills gaps independently
                    time::sleep(Duration::from_millis(100)).await;
                    continue;
                }

                // Fully synced!
                info!(
                    "✅ READY! Found {} validators, fully synced. Starting consensus from slot {}",
                    validator_count,
                    tip_slot + 1
                );
                is_joining_network = false; // Exit joining mode - we're caught up!
            }
        }

        // FREEZE PRODUCTION WHEN BEHIND: If the network is ahead of us,
        // don't produce blocks (including heartbeats) to avoid creating
        // a divergent chain. Let the block receiver + sync fill the gap.
        //
        // STABILITY-FIX: Bounded freeze guard. The original code could loop
        // forever because continuous incoming blocks keep highest_seen elevated
        // via note_seen(), preventing decay, and eventually the watchdog kills
        // the process. Now: if frozen for > MAX_FREEZE_DURATION_SECS, force-
        // decay and resume production to break the death spiral.
        {
            sync_manager.decay_highest_seen(tip_slot, 10).await;
            let network_highest = sync_manager.get_highest_seen().await;
            if network_highest > tip_slot + 10 {
                // Check bounded freeze timeout
                let now = std::time::Instant::now();
                let freeze_start = freeze_started_at.get_or_insert(now);
                let frozen_secs = freeze_start.elapsed().as_secs();
                if frozen_secs >= MAX_FREEZE_DURATION_SECS {
                    warn!(
                        "🔥 Freeze guard timeout after {}s (tip={}, network={}) — resuming production",
                        frozen_secs, tip_slot, network_highest
                    );
                    sync_manager.force_decay(tip_slot).await;
                    freeze_started_at = None;
                    // Fall through to production instead of continue
                } else {
                    continue;
                }
            } else {
                // Not frozen or caught up — reset freeze timer
                freeze_started_at = None;
            }
        }

        // Check if we already have a block for this slot (received from P2P)
        if let Ok(Some(_existing_block)) = state.get_block_by_slot(slot) {
            // Already have a block for this slot — tip will advance next iteration
            continue;
        }

        // ── BYZANTINE FAULT SLASHING SWEEP ──
        //
        // DESIGN (matching Solana/Ethereum):
        // Slash ONLY for provably malicious Byzantine faults:
        //   - DoubleBlock: same validator produced two blocks at the same slot
        //   - DoubleVote: same validator voted for different blocks at the same slot
        //   - InvalidStateTransition: provably wrong state root
        //   - Censorship / Collusion: detected by quorum analysis
        //
        // Downtime is NOT a slashable offense. Online validators that aren't
        // the current leader simply receive lower rewards (reputation-weighted).
        // This is how Solana, Ethereum 2.0, and other production chains work.
        //
        // Run every 100 slots (~40s) to reduce lock contention.
        if slot % 100 == 0 {
            let mut slasher = slashing_tracker.lock().await;
            let mut vs = validator_set.write().await;
            let mut pool = stake_pool.write().await;

            slasher.cleanup_expired(slot);

            let mut slash_debits: Vec<(moltchain_core::Pubkey, u64)> = Vec::new();

            for validator_info in vs.validators_mut() {
                // Only slash for BYZANTINE faults — never for downtime
                let has_byzantine_fault = slasher
                    .get_evidence(&validator_info.pubkey)
                    .map(|ev| {
                        ev.iter().any(|e| {
                            matches!(
                                e.offense,
                                SlashingOffense::DoubleBlock { .. }
                                    | SlashingOffense::DoubleVote { .. }
                                    | SlashingOffense::InvalidStateTransition { .. }
                                    | SlashingOffense::Censorship { .. }
                                    | SlashingOffense::Collusion { .. }
                            )
                        })
                    })
                    .unwrap_or(false);

                if has_byzantine_fault && !slasher.is_slashed(&validator_info.pubkey) {
                    let slashed_amount = slasher.apply_economic_slashing_with_params(
                        &validator_info.pubkey,
                        &mut pool,
                        &genesis_config.consensus,
                        slot,
                    );

                    // GRANT-PROTECT: Floor enforcement is now inside
                    // apply_economic_slashing_with_params — the returned amount
                    // already respects MIN_VALIDATOR_STAKE.  Use it directly.
                    let capped_slash = slashed_amount;

                    let reputation_penalty = slasher.calculate_penalty(&validator_info.pubkey);
                    let old_reputation = validator_info.reputation;
                    validator_info.reputation = validator_info
                        .reputation
                        .saturating_sub(reputation_penalty)
                        .max(50);

                    if capped_slash > 0 {
                        warn!(
                            "⚔️💰 BYZANTINE SLASH {} | Offense: {:?} | Stake burned: {:.4} MOLT | Rep: {} -> {}",
                            validator_info.pubkey.to_base58(),
                            slasher.get_evidence(&validator_info.pubkey)
                                .and_then(|ev| ev.iter().find(|e| !matches!(e.offense, SlashingOffense::Downtime { .. })))
                                .map(|e| format!("{:?}", e.offense))
                                .unwrap_or_default(),
                            capped_slash as f64 / 1_000_000_000.0,
                            old_reputation,
                            validator_info.reputation
                        );

                        if let Ok(Some(acct)) = state.get_account(&validator_info.pubkey) {
                            let debit = capped_slash.min(acct.staked);
                            if debit > 0 {
                                slash_debits.push((validator_info.pubkey, debit));
                            }
                        }
                    }
                }
            }

            // AUDIT-FIX E-9: Atomically persist all slashing debits in a single batch.
            // This ensures crash-consistency: either ALL account balance debits from this
            // sweep are persisted, or NONE are.
            if !slash_debits.is_empty() {
                let mut batch = state.begin_batch();
                for (pubkey, debit) in &slash_debits {
                    if let Ok(Some(mut acct)) = state.get_account(pubkey) {
                        acct.staked = acct.staked.saturating_sub(*debit);
                        acct.shells = acct.shells.saturating_sub(*debit);
                        if let Err(e) = batch.put_account(pubkey, &acct) {
                            error!(
                                "Failed to stage slashing debit for {}: {}",
                                pubkey.to_base58(),
                                e
                            );
                        }
                    }
                }
                if let Err(e) = state.commit_batch(batch) {
                    error!("Failed to atomically persist slashing debits: {}", e);
                } else {
                    info!(
                        "✅ Atomically persisted {} slashing debit(s) in sweep at slot {}",
                        slash_debits.len(),
                        slot
                    );
                }
            }

            // Clean up slashed flags for next sweep (keep permanent bans)
            {
                let all_slashed: Vec<_> = slasher.slashed_validators().collect();
                for pk in all_slashed {
                    if !slasher.is_permanently_banned(&pk) {
                        slasher.clear_slashed(&pk);
                    }
                }
            }

            // NOTE: We do NOT remove "ghost validators" — validators should
            // remain in the set even after slashing. They can re-stake to
            // get back above MIN_VALIDATOR_STAKE. Removal only happens for
            // permanently banned validators (collusion).

            // AUDIT-FIX 0.4: Persist stake pool and validator set after slashing
            // so that slashing effects survive node restarts.
            // PERF-OPT 4: Clone under lock, persist AFTER dropping write guards.
            let pool_snapshot = pool.clone();
            let vs_snapshot = vs.clone();
            let slasher_snapshot = slasher.clone();
            drop(vs);
            drop(pool);
            drop(slasher);
            if let Err(e) = state.put_stake_pool(&pool_snapshot) {
                error!("Failed to persist stake pool after slashing: {}", e);
            }
            if let Err(e) = state.save_validator_set(&vs_snapshot) {
                error!("Failed to persist validator set after slashing: {}", e);
            }
            // AUDIT-FIX M7: Persist slashing tracker evidence to disk
            if let Err(e) = state.put_slashing_tracker(&slasher_snapshot) {
                error!("Failed to persist slashing tracker: {}", e);
            }
        }

        // ── SLOT TIMING FLOOR ──
        // Enforce minimum slot_duration_ms (400ms) spacing between produced blocks.
        // This is a hard floor that prevents runaway block production regardless
        // of leader status or heartbeat gate logic. The 2ms poll loop is for
        // responsiveness, not for block production rate.
        if let Some(ref last_produced) = last_block_produced_at {
            if last_produced.elapsed() < Duration::from_millis(slot_duration_ms) {
                continue;
            }
        }

        // ── VIEW ROTATION: Wall-clock based for the CURRENT slot ──
        // Every view_change_interval (3 × slot_duration = 1200ms) without anyone
        // producing this slot, rotate the leader. slot_start resets when the
        // chain tip advances (a new slot to fill), so view starts fresh for
        // each consecutive slot number.
        let view_change_interval_ms = (slot_duration_ms * 3).max(1);
        let view = (slot_start.elapsed().as_millis() as u64 / view_change_interval_ms).min(15);

        // Stake-weighted leader election with deterministic fallback
        // PERF-OPT 5: Use cached result when slot+view haven't changed.
        let should_produce = if let Some((cs, cv, cp)) = cached_leader {
            if cs == slot && cv == view {
                cp
            } else {
                let vs = validator_set.read().await;
                let pool = stake_pool.read().await;
                let leader_slot = if view == 0 {
                    slot
                } else {
                    slot.saturating_mul(16).saturating_add(view)
                };
                // A5-01: Mix parent_hash for unpredictable leader selection
                let leader =
                    vs.select_leader_weighted_with_seed(leader_slot, &pool, &parent_hash.0);
                let sp = leader
                    .map(|pubkey| pubkey == validator_pubkey)
                    .unwrap_or(false);
                drop(pool);
                drop(vs);
                cached_leader = Some((slot, view, sp));
                sp
            }
        } else {
            let vs = validator_set.read().await;
            let pool = stake_pool.read().await;
            let leader_slot = if view == 0 {
                slot
            } else {
                slot.saturating_mul(16).saturating_add(view)
            };
            // A5-01: Mix parent_hash for unpredictable leader selection
            let leader = vs.select_leader_weighted_with_seed(leader_slot, &pool, &parent_hash.0);
            let sp = leader
                .map(|pubkey| pubkey == validator_pubkey)
                .unwrap_or(false);
            drop(pool);
            drop(vs);
            cached_leader = Some((slot, view, sp));
            sp
        };

        // Track whether we're producing as the deadlock breaker (immune to heartbeat gate)
        let mut is_deadlock_breaker = false;

        if !should_produce {
            // Not our turn — wait for the assigned leader to produce.
            // View rotation (wall-clock based above) will eventually make us
            // the leader if the current leader is offline. No slot advancement
            // needed since slot is derived from chain tip each iteration.
            //
            // FIX: Deadlock breaker — if we've exhausted all 16 views (view==15)
            // and still waited an additional full view interval without any block,
            // produce anyway. This prevents the network from permanently stalling
            // when the selected leaders (V2/V3) are still syncing and cannot produce.
            // The heartbeat mechanism ensures the chain stays alive.
            // STABILITY-FIX: Stagger the deadlock breaker per validator.
            // Without jitter, ALL validators fire simultaneously after the
            // same timeout, each producing its own block → immediate fork.
            // Add 0-4.5s jitter based on pubkey to let one win cleanly.
            let pubkey_jitter = (validator_pubkey.0[0] as u64 % 10) * 500;
            let deadlock_timeout_ms = view_change_interval_ms * 20 + pubkey_jitter;
            if view >= 15 && slot_start.elapsed().as_millis() as u64 > deadlock_timeout_ms {
                // AUDIT-FIX H3: Deterministic tiebreaker — only the validator
                // with the lowest pubkey (lexicographic) among active validators
                // should produce as deadlock breaker to prevent fork from dual production.
                let vs = validator_set.read().await;
                let is_lowest = vs.validators().iter().all(|v| validator_pubkey <= v.pubkey);
                drop(vs);
                if !is_lowest {
                    // Another active validator has a lower pubkey and should break the deadlock
                    continue;
                }
                info!(
                    "⚠️  Slot {} — all views exhausted with no block after {}ms, producing as deadlock breaker (lowest pubkey tiebreaker)",
                    slot,
                    slot_start.elapsed().as_millis()
                );
                is_deadlock_breaker = true;
                // Fall through to produce
            } else {
                continue;
            }
        }

        // ADAPTIVE HEARTBEAT: Early check BEFORE draining the mempool.
        // Heartbeats (empty blocks) are rate-limited to every 5 seconds for ALL
        // leaders including the primary. This prevents runaway empty block production.
        // Transaction blocks are produced immediately by the elected leader.
        // The SyncManager decay mechanism independently prevents chain stalls,
        // so the primary leader does NOT need to bypass the heartbeat gate.
        // HEARTBEAT-FIX: Check shared last_block_time to see when ANY block
        // (local or network-received) last occurred. This prevents simultaneous
        // heartbeats from multiple validators.
        let is_heartbeat_time =
            last_block_time_for_local.lock().await.elapsed() >= Duration::from_secs(5);
        let is_global_idle_for_heartbeat = global_last_user_tx_activity_for_producer
            .lock()
            .await
            .elapsed()
            >= Duration::from_secs(HEARTBEAT_GLOBAL_IDLE_SECS);

        // Peek at mempool to determine if this would be a heartbeat or tx block
        let has_pending = {
            let pool = mempool.lock().await;
            pool.size() > 0
        };

        if !has_pending {
            // No transactions — this will be a heartbeat block.
            // ALL heartbeats respect the 5-second timer, even primary leaders.
            // Only exception: deadlock breaker must produce to unstick a frozen chain.
            if (!is_heartbeat_time || !is_global_idle_for_heartbeat) && !is_deadlock_breaker {
                continue;
            }
        } else if !should_produce && !is_deadlock_breaker {
            // Has transactions but we were not selected as leader.
            // This shouldn't normally happen (leader check is above), but guard anyway.
            continue;
        }

        // parent_hash already refreshed at top of loop (LEADER-SEED-FIX)

        // Prune stale-blockhash transactions before draining mempool.
        // This avoids the death-spiral where a behind-validator collects
        // transactions with expired blockhashes, tries to include them,
        // drops them all, and produces only empty heartbeats.
        {
            let mut pool = mempool.lock().await;
            if let Ok(valid_hashes) = state.get_recent_blockhashes(MAX_TX_AGE_BLOCKS) {
                let evicted = pool.prune_stale_blockhashes(&valid_hashes);
                if evicted > 0 {
                    warn!(
                        "🧹 Pre-production prune: evicted {} stale-blockhash txs",
                        evicted
                    );
                }
            }
        }

        // Collect pending transactions from mempool
        let pending_transactions = {
            let mut pool = mempool.lock().await;
            pool.get_top_transactions(2000) // PERF: 500 → 2000 TXs per block for 5000-trader HF throughput
        };

        // Process transactions in parallel where possible (FIX-2: rayon)
        // Non-conflicting TXs (disjoint account sets) run on separate threads.
        let processed_hashes: Vec<Hash> = pending_transactions.iter().map(|tx| tx.hash()).collect();
        let results =
            processor.process_transactions_parallel(&pending_transactions, &validator_pubkey);

        let mut transactions: Vec<Transaction> = Vec::new();
        let mut tx_fees_paid: Vec<u64> = Vec::new();
        for (tx, result) in pending_transactions.into_iter().zip(results.into_iter()) {
            if result.success {
                transactions.push(tx);
                tx_fees_paid.push(result.fee_paid);
            } else {
                warn!(
                    "⚠️  Dropping transaction {}: {}",
                    tx.signature().to_hex(),
                    result.error.unwrap_or_else(|| "Unknown error".to_string())
                );
            }
        }

        let has_user_transactions = !transactions.is_empty();
        if has_user_transactions {
            *global_last_user_tx_activity_for_producer.lock().await = std::time::Instant::now();
        }

        // HEARTBEAT-RETROACTIVE: If the mempool looked non-empty (bypassed the
        // heartbeat gate) but all transactions failed processing, this block
        // would be a 0-tx heartbeat produced at the 400ms tx-rate instead of the
        // 5s heartbeat interval.  Re-apply the heartbeat gate here so spurious
        // empty blocks don't flood the chain during active tx submission.
        if !has_user_transactions
            && has_pending
            && !is_deadlock_breaker
            && (!is_heartbeat_time || !is_global_idle_for_heartbeat)
        {
            // Clean up the failed txs from mempool so they don't cause
            // the same bypass on the next iteration.
            if !processed_hashes.is_empty() {
                let mut pool = mempool.lock().await;
                pool.remove_transactions_bulk(&processed_hashes);
            }
            continue;
        }

        // ── FIX-FORK-1: Second guard right before block creation ──
        // Between the early `get_block_by_slot` check and here, the block
        // receiver task may have written a network block for this slot.
        // Re-check both RocksDB and the shared received_network_slots set.
        {
            let already_received = received_network_slots_for_producer
                .lock()
                .await
                .contains(&slot);
            let already_stored = state.get_block_by_slot(slot).ok().flatten().is_some();

            // STALL-FIX: If the slot is in received_network_slots but no actual
            // block exists in the DB, the entry is stale (e.g. from an unchainable
            // block on a divergent fork that was received but never applied).
            // Clear it and proceed with production to prevent permanent stalls.
            if already_received && !already_stored {
                let mut rns = received_network_slots_for_producer.lock().await;
                rns.remove(&slot);
                info!(
                    "🔧 Cleared stale received_network_slots entry for slot {} (no block in DB)",
                    slot
                );
                // Fall through to produce
            } else if already_received || already_stored {
                debug!(
                    "⏭️  Slot {} already has a network block, skipping production",
                    slot
                );

                // ANTI-SPIN-FIX: Remove drained transactions from mempool NOW.
                // Without this, the same stale/already-processed transactions stay
                // in the mempool and get re-drained every loop iteration, flooding
                // the log with "Dropping transaction" warnings.
                if !processed_hashes.is_empty() {
                    let mut pool = mempool.lock().await;
                    pool.remove_transactions_bulk(&processed_hashes);
                }

                // ANTI-SPIN-FIX: When the deadlock breaker can't produce because
                // a network block already occupies this slot, try to advance the
                // chain tip to incorporate stored network blocks. This breaks the
                // infinite spin loop where get_last_slot() stays behind while the
                // deadlock breaker keeps retrying the same slot.
                if is_deadlock_breaker {
                    // Scan forward from current tip to find consecutive stored blocks
                    // that we can adopt (advancing last_slot through the gap).
                    let mut advance_slot = tip_slot;
                    let max_scan = 200u64; // don't scan forever
                    for probe in 1..=max_scan {
                        let candidate = tip_slot + probe;
                        if state.get_block_by_slot(candidate).ok().flatten().is_some() {
                            advance_slot = candidate;
                        } else {
                            break;
                        }
                    }
                    if advance_slot > tip_slot {
                        if let Err(e) = state.set_last_slot(advance_slot) {
                            warn!("⚠️  Failed to advance last_slot to {}: {}", advance_slot, e);
                        } else {
                            info!(
                                "🔄 Deadlock breaker: advanced chain tip {} → {} (adopted network blocks)",
                                tip_slot, advance_slot
                            );
                        }
                    } else {
                        // Can't advance yet — sleep to prevent tight spin loop
                        // (without this, the loop spins at ~3ms per iteration,
                        //  generating 28,000+ log lines per minute)
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }

                continue;
            }
        }

        let is_heartbeat = !has_user_transactions;

        // HEARTBEAT-FIX: last_block_time_for_local is updated at L10012 after
        // block storage — no separate activity timer needed.

        if is_heartbeat {
            info!("💓 Slot {} - HEARTBEAT (proving liveness)", slot);
        } else {
            info!(
                "👑 Slot {} - I AM LEADER ({} transactions)",
                slot,
                transactions.len()
            );
        }

        // Block rewards are applied as protocol-level effects in
        // apply_block_effects (coinbase model), not as signed transactions.
        // This means no treasury private key is needed for block production.
        let rewards_applied = false;

        // Test transactions disabled - use wallet or CLI to send real transactions
        // (Previous test code was incorrectly signing transfers from genesis with validator key)

        // AUDIT-FIX E-7: Apply block effects BEFORE computing state_root so the
        // root in the block header reflects post-effect state (rewards, fees, etc.).
        // Step 1: Create a preliminary block (state_root = default placeholder).
        //         apply_block_effects only uses block.header.slot/validator/transactions.
        let wall_clock_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut block = Block::new_with_timestamp(
            slot,
            parent_hash,
            Hash::default(), // placeholder — will be set after effects
            validator_pubkey.0,
            transactions.clone(),
            wall_clock_timestamp,
        );
        block.tx_fees_paid = tx_fees_paid;

        // Embed current oracle prices in the block for consensus propagation.
        // All validators will apply these deterministically via apply_oracle_from_block().
        block.oracle_prices = shared_oracle_prices.snapshot();

        // Step 2: Apply block effects (rewards, stake updates, etc.)
        apply_block_effects(
            &state,
            &validator_set,
            &stake_pool,
            &vote_aggregator,
            &block,
            rewards_applied,
        )
        .await;
        apply_oracle_from_block(&state, &block);

        // Step 3: Now compute state_root AFTER effects are applied
        let state_root = state.compute_state_root();
        block.header.state_root = state_root;

        // Sign block so receiving validators can verify authenticity (T2.2)
        block.sign(&validator_keypair);

        let block_hash = block.hash();

        // Store block
        if let Err(e) = state.put_block(&block) {
            error!("Failed to store block at slot {}: {e}", slot);
        }
        if let Err(e) = state.set_last_slot(slot) {
            error!("Failed to update last slot to {}: {e}", slot);
        }
        for tx in &block.transactions {
            if let Some(ix) = tx.message.instructions.first() {
                if ix.program_id == EVM_PROGRAM_ID {
                    let evm_hash = evm_tx_hash(&ix.data).0;
                    if let Err(e) = state.mark_evm_tx_included(&evm_hash, slot, &block_hash) {
                        warn!("⚠️  Failed to mark EVM tx included: {}", e);
                    }
                }
            }
        }
        *last_block_time_for_local.lock().await = std::time::Instant::now();

        // PERF-OPT 5: Broadcast block to network IMMEDIATELY after storing.
        // P3-3: Send compact block (header + short TX IDs) instead of the full
        // block.  Receiving peers reconstruct from their mempool, saving ~90%
        // bandwidth.  Also send the full block as fallback for peers that don't
        // have all TXs in their mempool yet.
        if let Some(ref peer_mgr) = p2p_peer_manager {
            let compact = moltchain_p2p::CompactBlock::from_block(&block);
            let compact_msg = moltchain_p2p::P2PMessage::new(
                moltchain_p2p::MessageType::CompactBlockMsg(compact),
                p2p_config.listen_addr,
            );
            let pm_block = peer_mgr.clone();
            tokio::spawn(async move {
                pm_block.broadcast(compact_msg).await;
            });
        }

        if rewards_applied {
            if let Err(e) = state.set_reward_distribution_hash(slot, &block_hash) {
                warn!(
                    "⚠️  Failed to record reward distribution for slot {}: {}",
                    slot, e
                );
            }
        }

        emit_program_and_nft_events(&state, &ws_event_tx, &block);

        // PERF-OPT: Fire-and-forget DEX event emission + analytics bridge.
        // Read the trade counter once (cheap), update tracking counters
        // synchronously, then spawn the heavy I/O work (trade reads, WS
        // broadcasts, analytics writes) on the blocking thread pool so the
        // block production loop is not stalled.
        {
            // DEX-WS-FIX: Read trade count from shared state (not local-only).
            // This ensures DEX WS events are emitted for trades in blocks produced
            // by ANY validator (including blocks received via P2P and applied by
            // the block receiver), not just blocks this validator produced.
            let current_trade_count = state.get_program_storage_u64("DEX", b"dex_trade_count");

            // F6.2: Emit DEX WebSocket events for new trades/orders
            if current_trade_count > last_dex_trade_count {
                let prev = last_dex_trade_count;
                last_dex_trade_count = current_trade_count;
                let state_c = state.clone();
                let bc_c = ws_dex_broadcaster.clone();
                let slot_c = slot;
                tokio::task::spawn_blocking(move || {
                    emit_dex_events(&state_c, &bc_c, prev, current_trade_count, slot_c);
                });
            }

            // P9-VAL-04: Trade bridge uses state-persisted cursor (deterministic)
            run_analytics_bridge_from_state(&state, slot, slot_duration_ms);

            // SL/TP trigger engine: check dormant stop-limit orders and margin
            // position SL/TP levels when new trades occurred.
            // Uses state-persisted cursor so receivers execute identically (P9-VAL-01).
            run_sltp_triggers_from_state(&state);
        }

        // Rolling 24h window reset: check if any pair's 24h stats need to roll over
        // P9-VAL-05: Pass deterministic block timestamp
        reset_24h_stats_if_expired(&state, block.header.timestamp);

        // Broadcast block event to WebSocket subscribers
        let _ = ws_event_tx.send(moltchain_rpc::ws::Event::Block(block.clone()));

        // Cast vote for our own block (BFT consensus)
        // VOTE-AUTHORITY: Use the shared VoteAuthority to atomically check-then-sign.
        // If the block receiver already voted for this slot (e.g. P2P echo arrived
        // before producer finished), VoteAuthority returns None and we skip.
        let maybe_vote = vote_authority.lock().await.try_vote(slot, block_hash);

        if let Some(vote) = maybe_vote {
            let mut agg = vote_aggregator.write().await;
            let vs = validator_set.read().await;
            if agg.add_vote_validated(vote.clone(), &vs) {
                // Check if we reached finality immediately (solo validator case)
                let pool = stake_pool.read().await;
                if agg.has_supermajority(slot, &block_hash, &vs, &pool) {
                    info!("🔒 Block {} FINALIZED (stake-weighted self-vote)", slot);
                    // Update finality tracker + persist to StateStore
                    if finality_tracker.mark_confirmed(slot) {
                        let _ = state.set_last_confirmed_slot(finality_tracker.confirmed_slot());
                        let _ = state.set_last_finalized_slot(finality_tracker.finalized_slot());
                    }
                }
            }

            // P3-5: Route self-vote through validator mesh for lowest latency
            if let Some(ref peer_mgr) = p2p_peer_manager {
                let vote_msg = P2PMessage::new(MessageType::Vote(vote), p2p_config.listen_addr);
                let pm_vote = peer_mgr.clone();
                tokio::spawn(async move {
                    pm_vote.broadcast_to_validators(vote_msg).await;
                });
                info!("📡 Broadcasted block {} + vote to validator mesh", slot);
            }
        } else {
            info!(
                "⚠️  VoteAuthority: slot {} already voted — producer self-vote skipped",
                slot
            );
        }

        // Remove included transactions from mempool (PERF: bulk removal, single heap rebuild)
        {
            let mut pool = mempool.lock().await;
            pool.remove_transactions_bulk(&processed_hashes);
        }

        // AUDIT-FIX E-7: apply_block_effects already called before block creation (above)
        maybe_create_checkpoint(&state, slot, &data_dir, &sync_manager).await;

        // Periodic stats pruning — every 1000 slots, prune seq counters older than 10K slots
        if slot % 1000 == 0 {
            match state.prune_slot_stats(slot, 10_000) {
                Ok(0) => {} // nothing to prune
                Ok(n) => info!("🧹 Pruned {} stale stats keys (retain last 10K slots)", n),
                Err(e) => warn!("⚠️  Stats pruning failed at slot {}: {}", slot, e),
            }
        }

        // Periodic sync & checkpoint stats — every 1000 slots
        if slot % 1000 == 0 {
            let sync_stats = sync_manager.stats().await;
            let checkpoint_slot = sync_manager.get_checkpoint().await;
            info!(
                "📊 Sync stats [slot {}]: pending={}, syncing={}, network_tip={}, checkpoint={}",
                slot,
                sync_stats.pending_blocks,
                sync_stats.is_syncing,
                sync_stats.highest_seen,
                checkpoint_slot,
            );
            if let Some(progress) = sync_manager.get_sync_progress(slot).await {
                info!(
                    "📊 Sync progress: {}/{} slots (batch: {:?}, behind: {})",
                    progress.current_slot,
                    progress.target_slot,
                    progress.current_batch,
                    progress.blocks_behind,
                );
            }
        }

        let tx_count = transactions.len();
        let current_reputation = {
            let vs = validator_set.read().await;
            vs.get_validator(&validator_pubkey)
                .map(|v| v.reputation)
                .unwrap_or(0)
        };

        if is_heartbeat {
            info!(
                "💓 HEARTBEAT {} | hash: {} | parent: {} | reputation: {} | proving liveness",
                slot,
                block_hash.to_hex()[..8].to_string(),
                parent_hash.to_hex()[..8].to_string(),
                current_reputation,
            );
        } else {
            info!(
                "📦 BLOCK {} | hash: {} | txs: {} | parent: {} | reputation: {}",
                slot,
                block_hash.to_hex()[..8].to_string(),
                tx_count,
                parent_hash.to_hex()[..8].to_string(),
                current_reputation,
            );

            // Show validator balance for transaction blocks
            if let Ok(Some(val_account)) = state.get_account(&validator_pubkey) {
                info!(
                    "   💰 Validator balance: {} MOLT",
                    val_account.balance_molt()
                );
            }
        }

        parent_hash = block_hash;
        last_block_produced_at = Some(std::time::Instant::now());
        // (No slot increment — next iteration derives slot from chain tip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moltchain_core::{Instruction, Message};

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
            },
        }
    }

    fn make_empty_tx() -> Transaction {
        Transaction {
            signatures: vec![],
            message: Message {
                instructions: vec![],
                recent_blockhash: Hash([0u8; 32]),
            },
        }
    }

    fn make_block_with_txs(txs: Vec<Transaction>) -> Block {
        Block {
            header: moltchain_core::BlockHeader {
                slot: 1,
                parent_hash: Hash([0u8; 32]),
                state_root: Hash([0u8; 32]),
                tx_root: Hash([0u8; 32]),
                timestamp: 0,
                validator: [0u8; 32],
                signature: [0u8; 64],
            },
            transactions: txs,
            tx_fees_paid: vec![],
            oracle_prices: vec![],
        }
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

    // ── block_fee_at_index ──────────────────────────────────────────

    #[test]
    fn block_fee_at_index_uses_precomputed() {
        let tx = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 0);
        let mut block = make_block_with_txs(vec![tx]);
        block.tx_fees_paid = vec![500];
        assert_eq!(block_fee_at_index(&block, 0, 999), 500);
    }

    #[test]
    fn block_fee_at_index_fallback_when_mismatched() {
        let tx = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 0);
        let mut block = make_block_with_txs(vec![tx]);
        block.tx_fees_paid = vec![]; // length mismatch
        assert_eq!(block_fee_at_index(&block, 0, 999), 999);
    }

    #[test]
    fn block_fee_at_index_out_of_bounds() {
        let tx = make_tx_with_opcode(CORE_SYSTEM_PROGRAM_ID, 0);
        let mut block = make_block_with_txs(vec![tx]);
        block.tx_fees_paid = vec![100];
        // Index 5 out of bounds → fallback
        assert_eq!(block_fee_at_index(&block, 5, 999), 999);
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
    fn reward_pool_is_100m() {
        assert_eq!(REWARD_POOL_MOLT, 100_000_000);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn watchdog_timeout_reasonable() {
        assert!(DEFAULT_WATCHDOG_TIMEOUT_SECS >= 30);
        assert!(DEFAULT_WATCHDOG_TIMEOUT_SECS <= 600);
    }

    #[test]
    fn max_freeze_is_30s() {
        assert_eq!(MAX_FREEZE_DURATION_SECS, 30);
    }

    #[test]
    fn heartbeat_idle_is_5s() {
        assert_eq!(HEARTBEAT_GLOBAL_IDLE_SECS, 5);
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
                moltchain_core::state::SymbolRegistryEntry {
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
                moltchain_core::state::SymbolRegistryEntry {
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

        // Register MOLTCOIN program
        let moltcoin_pk = Pubkey([51u8; 32]);
        state
            .register_symbol(
                "MOLTCOIN",
                moltchain_core::state::SymbolRegistryEntry {
                    symbol: "MOLTCOIN".to_string(),
                    program: moltcoin_pk,
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
                moltchain_core::state::SymbolRegistryEntry {
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
        let user_bal = state.get_program_storage_u64("MOLTCOIN", balance_key.as_bytes());
        assert_eq!(user_bal, 600, "user should receive margin + capped profit");
    }
}
