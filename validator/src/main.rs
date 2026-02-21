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
    evm_tx_hash, Account, Block, ContractAccount, ContractContext, ContractInstruction,
    ContractRuntime, FeeConfig, FinalityTracker, ForkChoice, GenesisConfig, GenesisWallet, Hash,
    Instruction, Keypair, MarketActivity, MarketActivityKind, Mempool, Message, NftActivity,
    NftActivityKind, ProgramCallActivity, Pubkey, SlashingEvidence, SlashingOffense, StakePool,
    StateStore, SymbolRegistryEntry, Transaction, TxProcessor, ValidatorInfo, ValidatorSet, Vote,
    VoteAggregator, BASE_FEE, BOOTSTRAP_GRANT_AMOUNT, CONTRACT_DEPLOY_FEE, CONTRACT_UPGRADE_FEE,
    EVM_PROGRAM_ID, MIN_VALIDATOR_STAKE, NFT_COLLECTION_FEE, NFT_MINT_FEE, SLOTS_PER_EPOCH,
    SYSTEM_PROGRAM_ID as CORE_SYSTEM_PROGRAM_ID,
};
use moltchain_p2p::{
    ConsistencyReportMsg, MessageType, P2PConfig, P2PMessage, P2PNetwork, SnapshotKind,
    SnapshotRequestMsg, SnapshotResponseMsg, StatusRequestMsg, StatusResponseMsg,
};
use moltchain_rpc::start_rpc_server;
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
const GENESIS_MINT_PUBKEY: Pubkey = Pubkey([0xFE; 32]);
/// AUDIT-FIX 3.12: Documented — this is 150M MOLT (15% of 1B supply) per whitepaper.
/// The `.min(1_000_000_000)` cap in the legacy path is a safety guard that's
/// redundant (150M < 1B). Kept for backward compat only — new deployments use
/// the GenesisAccounts distribution path.
const REWARD_POOL_MOLT: u64 = 150_000_000; // 15% of 1B supply (in MOLT, not shells)

/// Exit code used by the internal health watchdog to signal the supervisor
/// that the validator should be restarted (deadlock/stall detected).
const EXIT_CODE_RESTART: i32 = 75;

/// Default number of seconds with no block activity before the watchdog
/// triggers a restart.  Override with `--watchdog-timeout <secs>`.
/// Reduced from 120s to 30s for faster recovery from stalls.
const DEFAULT_WATCHDOG_TIMEOUT_SECS: u64 = 30;

/// Maximum number of automatic restarts before the supervisor gives up.
/// Override with `--max-restarts <n>`.
const DEFAULT_MAX_RESTARTS: u32 = 50;

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

#[derive(Debug, Deserialize)]
struct SeedsFile {
    testnet: Option<SeedNetwork>,
    mainnet: Option<SeedNetwork>,
    devnet: Option<SeedNetwork>,
}

#[derive(Debug, Deserialize)]
struct SeedNetwork {
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

fn load_seed_peers(chain_id: &str, seeds_path: &Path) -> Vec<String> {
    let contents = match fs::read_to_string(seeds_path) {
        Ok(data) => data,
        Err(_) => return Vec::new(),
    };

    let seeds: SeedsFile = match serde_json::from_str(&contents) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
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

    peers
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
    keys_dir: &Path,
    chain_id: &str,
) -> Option<PathBuf> {
    if let Some(wallet) = genesis_wallet {
        if let Some(path) = wallet.treasury_keypair_path.as_ref() {
            let candidate = PathBuf::from(path);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    let candidate = keys_dir.join(format!("treasury-{}.json", chain_id));
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

fn load_treasury_keypair(
    genesis_wallet: Option<&GenesisWallet>,
    keys_dir: &Path,
    chain_id: &str,
) -> Option<Keypair> {
    let path = resolve_treasury_keypair_path(genesis_wallet, keys_dir, chain_id)?;
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

                    let market_kind = match function.as_str() {
                        "list_nft" => Some(MarketActivityKind::Listing),
                        "buy_nft" => Some(MarketActivityKind::Sale),
                        "cancel_listing" => Some(MarketActivityKind::Cancel),
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
    let margin_pk = match state.get_symbol_registry("MARGIN") {
        Ok(Some(entry)) => entry.program,
        _ => return,
    };

    let pos_count = state.get_program_storage_u64("MARGIN", b"position_count");
    let mut sltp_closed: u64 = 0;

    for pid in 1..=pos_count {
        let pk = format!("margin_pos_{}", pid);
        let data = match state.get_program_storage("MARGIN", pk.as_bytes()) {
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
        let fresh_data = match state.get_program_storage("MARGIN", pk.as_bytes()) {
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
        // Stored as biased: actual_pnl + BIAS where BIAS = 1 << 62
        const BIAS: u64 = 1u64 << 62;
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
        let insurance_fund = state.get_program_storage_u64("MARGIN", b"mrg_insurance");

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
            let prev_profit = state.get_program_storage_u64("MARGIN", b"mrg_pnl_profit");
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
            let prev_loss = state.get_program_storage_u64("MARGIN", b"mrg_pnl_loss");
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
fn run_analytics_bridge_from_state(state: &StateStore, slot: u64) {
    let cursor = state.get_program_storage_u64("DEX", b"dex_analytics_bridge_cursor");
    let current = state.get_program_storage_u64("DEX", b"dex_trade_count");
    if current > cursor {
        bridge_dex_trades_to_analytics(state, cursor, current, slot);
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

fn bridge_dex_trades_to_analytics(state: &StateStore, from_trade: u64, to_trade: u64, slot: u64) {
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
    let slot_duration_ms = 400u64; // matches genesis config default
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
fn revert_block_effects(state: &StateStore, old_block: &Block) {
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
    let total_fee: u64 = old_block
        .transactions
        .iter()
        .map(|tx| TxProcessor::compute_transaction_fee(tx, &fee_config))
        .sum();

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

    for tx in old_block.transactions.iter().rev() {
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
                let fee = TxProcessor::compute_transaction_fee(tx, &fee_config);
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

    let stake_amount = {
        let pool = stake_pool.read().await;
        pool.get_stake(&producer)
            .map(|stake_info| stake_info.total_stake())
            .unwrap_or(0)
    };

    {
        let mut vs = validator_set.write().await;
        if let Some(val_info) = vs.get_validator_mut(&producer) {
            val_info.blocks_proposed += 1;
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
                    blocks_proposed: 1,
                    votes_cast: 0,
                    correct_votes: 0,
                    joined_slot: slot,
                    last_active_slot: slot,
                    commission_rate: 500,
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
    if !skip_rewards {
        let reward_already = match state.get_reward_distribution_hash(slot) {
            Ok(Some(_)) => true, // per-slot guard: any reward for this slot = skip
            Ok(None) => false,
            Err(e) => {
                warn!("⚠️  Failed to read reward distribution hash: {}", e);
                false
            }
        };

        if !reward_already {
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
                        treasury_account.shells =
                            treasury_account.shells.saturating_sub(debit_amount);
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
                        producer_account.add_spendable(credit_amount).unwrap_or_else(|e| {
                            warn!("\u{26a0}\u{fe0f}  Overflow crediting producer block reward: {}", e);
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
                                    // Fund reef_share from treasury
                                    if let Some(ref tpk) = treasury_pubkey {
                                        let mut t_acct =
                                            state.get_account(tpk).ok().flatten().unwrap_or_else(
                                                || Account::new(0, SYSTEM_ACCOUNT_OWNER),
                                            );
                                        if t_acct.shells >= reef_share {
                                            t_acct.shells =
                                                t_acct.shells.saturating_sub(reef_share);
                                            t_acct.spendable =
                                                t_acct.spendable.saturating_sub(reef_share);
                                            reef_pool.distribute_rewards(reef_share);
                                            // L4-01 fix: treasury debit + pool update in single atomic WriteBatch
                                            if let Err(e) = state.atomic_put_account_with_reefstake(
                                                tpk, &t_acct, &reef_pool,
                                            ) {
                                                warn!("⚠️  Failed to persist ReefStake distribution: {}", e);
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
    }

    let fee_config = state
        .get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());
    let total_fee: u64 = block
        .transactions
        .iter()
        .map(|tx| TxProcessor::compute_transaction_fee(tx, &fee_config))
        .sum();

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
    let mut voters_paid: u64 = 0;

    // NOTE: burn was already applied in charge_fee (processor.rs) during
    // transaction processing.  Do NOT call add_burned again here — that
    // caused a double-burn destroying twice the intended supply.

    // AUDIT-FIX 0.6: All fee distribution writes go through an atomic
    // WriteBatch. Nothing hits disk until commit_batch() succeeds, so a
    // crash mid-distribution cannot leave state half-credited.
    let mut batch = state.begin_batch();

    if producer_share > 0 {
        let mut producer_account = match state.get_account(&producer) {
            Ok(Some(account)) => account,
            _ => Account::new(0, SYSTEM_ACCOUNT_OWNER),
        };
        producer_account
            .add_spendable(producer_share)
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

    let treasury_share = total_fee.saturating_sub(burn + producer_share + voters_paid);

    // charge_fee credited treasury with (fee − burn) for each tx.
    // We only debit what we're distributing out: producer_share + voters_paid.
    // Treasury retains its own share (≈10%) automatically.
    treasury_account.shells = treasury_account
        .shells
        .saturating_sub(producer_share + voters_paid);
    treasury_account.spendable = treasury_account
        .spendable
        .saturating_sub(producer_share + voters_paid);
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
// Deploys all compiled WASM contracts from the contracts/ directory into
// the chain state immediately after genesis. This ensures the blockchain
// is fully operational from the first block — no external deploy scripts
// needed. Contract addresses are derived deterministically from
// SHA-256(deployer_pubkey + wasm_bytes).
// ========================================================================

/// Contract catalog: (directory_name, symbol, display_name, template)
const GENESIS_CONTRACT_CATALOG: &[(&str, &str, &str, &str)] = &[
    // Core token
    ("moltcoin", "MOLT", "MoltCoin", "token"),
    // Wrapped tokens
    ("musd_token", "MUSD", "Wrapped USD", "wrapped"),
    ("wsol_token", "WSOL", "Wrapped SOL", "wrapped"),
    ("weth_token", "WETH", "Wrapped ETH", "wrapped"),
    // DEX
    ("dex_core", "DEX", "MoltChain DEX Core", "dex"),
    ("dex_amm", "DEXAMM", "DEX AMM Engine", "dex"),
    ("dex_router", "DEXROUTER", "DEX Smart Router", "dex"),
    ("dex_margin", "DEXMARGIN", "DEX Margin Trading", "dex"),
    ("dex_rewards", "DEXREWARDS", "DEX Reward Distributor", "dex"),
    ("dex_governance", "DEXGOV", "DEX Governance", "dex"),
    ("dex_analytics", "ANALYTICS", "DEX Analytics", "dex"),
    // DeFi
    ("moltswap", "MOLTSWAP", "MoltSwap AMM", "defi"),
    ("moltbridge", "BRIDGE", "MoltBridge", "bridge"),
    ("moltmarket", "MARKET", "MoltMarket", "marketplace"),
    ("moltoracle", "ORACLE", "MoltOracle", "oracle"),
    ("moltauction", "AUCTION", "MoltAuction", "auction"),
    ("moltdao", "DAO", "MoltDAO Governance", "governance"),
    ("lobsterlend", "LEND", "LobsterLend", "lending"),
    // NFT / Identity
    ("moltpunks", "PUNKS", "MoltPunks NFT", "nft"),
    ("moltyid", "YID", "MoltyID Identity", "identity"),
    // Infrastructure
    ("clawpay", "CLAWPAY", "ClawPay Payments", "payments"),
    ("clawpump", "CLAWPUMP", "ClawPump Launchpad", "launchpad"),
    ("clawvault", "CLAWVAULT", "ClawVault", "vault"),
    ("bountyboard", "BOUNTY", "BountyBoard", "bounty"),
    ("compute_market", "COMPUTE", "Compute Market", "compute"),
    ("reef_storage", "REEF", "Reef Storage", "storage"),
    // Prediction Markets
    ("prediction_market", "PREDICT", "Prediction Markets", "defi"),
];

fn genesis_auto_deploy(state: &StateStore, deployer_pubkey: &Pubkey, label: &str) {
    info!("──────────────────────────────────────────────────────");
    info!("  {} Auto-deploying genesis contracts", label);
    info!("──────────────────────────────────────────────────────");

    let contracts_dir = PathBuf::from("contracts");
    if !contracts_dir.exists() {
        warn!("contracts/ directory not found — skipping auto-deploy");
        return;
    }

    let mut deployed: usize = 0;
    let mut failed: usize = 0;

    for &(dir_name, symbol, display_name, template) in GENESIS_CONTRACT_CATALOG {
        let wasm_path = contracts_dir
            .join(dir_name)
            .join(format!("{}.wasm", dir_name));
        if !wasm_path.exists() {
            warn!(
                "  SKIP {}: WASM not found at {}",
                symbol,
                wasm_path.display()
            );
            failed += 1;
            continue;
        }

        let wasm_bytes = match fs::read(&wasm_path) {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("  FAIL {}: Cannot read WASM: {}", symbol, e);
                failed += 1;
                continue;
            }
        };

        // Derive deterministic program address: SHA-256(deployer + name + wasm)
        // Including the name ensures identical WASMs (e.g. wrapped token stubs)
        // get unique addresses.
        let mut hasher = Sha256::new();
        hasher.update(deployer_pubkey.0);
        hasher.update(dir_name.as_bytes());
        hasher.update(&wasm_bytes);
        let hash_result = hasher.finalize();
        let mut addr_bytes = [0u8; 32];
        addr_bytes.copy_from_slice(&hash_result[..32]);
        let program_pubkey = Pubkey(addr_bytes);

        // Check if already deployed (idempotent)
        if let Ok(Some(_)) = state.get_account(&program_pubkey) {
            info!(
                "  SKIP {}: already deployed at {}",
                symbol,
                program_pubkey.to_base58()
            );
            continue;
        }

        // Create ContractAccount
        let contract = ContractAccount::new(wasm_bytes, *deployer_pubkey);

        // Create executable Account with contract data
        let mut account = Account::new(0, program_pubkey);
        match serde_json::to_vec(&contract) {
            Ok(data) => account.data = data,
            Err(e) => {
                error!("  FAIL {}: Serialize error: {}", symbol, e);
                failed += 1;
                continue;
            }
        }
        account.executable = true;

        // Store the account
        if let Err(e) = state.put_account(&program_pubkey, &account) {
            error!("  FAIL {}: put_account error: {}", symbol, e);
            failed += 1;
            continue;
        }

        // Index in CF_PROGRAMS (makes it visible to getAllContracts)
        if let Err(e) = state.index_program(&program_pubkey) {
            warn!("  WARN {}: index_program error: {}", symbol, e);
        }

        // Register in symbol registry with rich token metadata
        let mut meta = serde_json::json!({
            "genesis_deploy": true,
            "wasm_size": account.data.len(),
        });
        // Enrich token/wrapped contracts with MT-20 metadata
        match template {
            "token" => {
                // MOLT native token: 1B fixed supply, 9 decimals, NOT mintable (deflationary via 50% fee burn)
                meta["total_supply"] = serde_json::json!(1_000_000_000_u64 * 1_000_000_000_u64);
                meta["decimals"] = serde_json::json!(9);
                meta["mintable"] = serde_json::json!(false);
                meta["burnable"] = serde_json::json!(true);
                meta["is_native"] = serde_json::json!(true);
            }
            "wrapped" => {
                // Wrapped tokens start at 0 supply, 9 decimals
                meta["total_supply"] = serde_json::json!(0);
                meta["decimals"] = serde_json::json!(9);
                meta["mintable"] = serde_json::json!(true);
                meta["burnable"] = serde_json::json!(true);
            }
            _ => {}
        }
        let entry = SymbolRegistryEntry {
            symbol: symbol.to_string(),
            program: program_pubkey,
            owner: *deployer_pubkey,
            name: Some(display_name.to_string()),
            template: Some(template.to_string()),
            metadata: Some(meta),
        };
        if let Err(e) = state.register_symbol(symbol, entry) {
            warn!("  WARN {}: register_symbol error: {}", symbol, e);
        }

        info!(
            "  OK   {} ({}) -> {}",
            symbol,
            display_name,
            program_pubkey.to_base58()
        );
        deployed += 1;
    }

    info!("──────────────────────────────────────────────────────");
    info!(
        "  Genesis deploy complete: {} deployed, {} failed",
        deployed, failed
    );
    info!("──────────────────────────────────────────────────────");
}

// ========================================================================
//  GENESIS PHASE 2 — Initialize all 26 contracts by executing their
//  initialize() function via the WASM runtime.
// ========================================================================

/// Derive a contract's deterministic address from deployer + dir_name + wasm.
/// Must match the derivation in genesis_auto_deploy().
fn derive_contract_address(deployer_pubkey: &Pubkey, dir_name: &str) -> Option<Pubkey> {
    let contracts_dir = PathBuf::from("contracts");
    let wasm_path = contracts_dir
        .join(dir_name)
        .join(format!("{}.wasm", dir_name));
    let wasm_bytes = fs::read(&wasm_path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(deployer_pubkey.0);
    hasher.update(dir_name.as_bytes());
    hasher.update(&wasm_bytes);
    let hash_result = hasher.finalize();
    let mut addr_bytes = [0u8; 32];
    addr_bytes.copy_from_slice(&hash_result[..32]);
    Some(Pubkey(addr_bytes))
}

/// Execute a contract function via WASM runtime and apply storage changes.
/// Returns true on success.
/// Monotonic sequence counter for genesis activity indexing.
/// Each genesis call gets a unique sequence to avoid CF key collisions.
static GENESIS_ACTIVITY_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn genesis_exec_contract(
    state: &StateStore,
    program_pubkey: &Pubkey,
    deployer_pubkey: &Pubkey,
    function_name: &str,
    args: &[u8],
    label: &str,
) -> bool {
    let account = match state.get_account(program_pubkey) {
        Ok(Some(a)) => a,
        _ => {
            error!("  FAIL {}: account not found", label);
            return false;
        }
    };

    let mut contract: ContractAccount = match serde_json::from_slice(&account.data) {
        Ok(c) => c,
        Err(e) => {
            error!("  FAIL {}: deserialize ContractAccount: {}", label, e);
            return false;
        }
    };

    let ctx = ContractContext::with_args(
        *deployer_pubkey,
        *program_pubkey,
        0,
        0,
        contract.storage.clone(),
        args.to_vec(),
    );

    let mut runtime = ContractRuntime::new();
    match runtime.execute(&contract, function_name, args, ctx) {
        Ok(result) => {
            if !result.success {
                // Check for non-zero return code — indicates a real WASM error,
                // not just "already initialized". Return false so callers know.
                let rc = result.return_code.unwrap_or(1);
                if rc != 0 {
                    warn!(
                        "  FAIL {}: contract returned error code {} — {:?}",
                        label, rc, result.error
                    );
                    return false;
                }
                // return_code == 0 with success == false: treat as non-fatal
                // (e.g., "already initialized" idempotent calls)
                warn!(
                    "  WARN {}: contract returned !success with rc=0: {:?}",
                    label, result.error
                );
            }
            // Apply storage changes
            for (key, val_opt) in &result.storage_changes {
                match val_opt {
                    Some(val) => {
                        contract.set_storage(key.clone(), val.clone());
                        // Also write to CF_CONTRACT_STORAGE for fast-path RPC reads
                        if let Err(e) = state.put_contract_storage(program_pubkey, key, val) {
                            warn!("  WARN {}: put_contract_storage: {}", label, e);
                        }
                    }
                    None => {
                        contract.remove_storage(key);
                        let _ = state.delete_contract_storage(program_pubkey, key);
                    }
                }
            }
            // Re-serialize and store
            let mut updated_account = account;
            match serde_json::to_vec(&contract) {
                Ok(data) => updated_account.data = data,
                Err(e) => {
                    error!("  FAIL {}: re-serialize: {}", label, e);
                    return false;
                }
            }
            if let Err(e) = state.put_account(program_pubkey, &updated_account) {
                error!("  FAIL {}: put_account: {}", label, e);
                return false;
            }

            // ── Record genesis call in CF_PROGRAM_CALLS for explorer indexing ──
            let seq = GENESIS_ACTIVITY_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let activity = ProgramCallActivity {
                slot: 0,
                timestamp: 0,
                program: *program_pubkey,
                caller: *deployer_pubkey,
                function: function_name.to_string(),
                value: 0,
                tx_signature: Hash([0u8; 32]), // Genesis — no real tx
            };
            if let Err(e) = state.record_program_call(&activity, seq) {
                warn!("  WARN {}: failed to record genesis call: {}", label, e);
            }

            // ── Persist any events emitted during genesis WASM execution ──
            for event in &result.events {
                if let Err(e) = state.put_contract_event(program_pubkey, event) {
                    warn!("  WARN {}: failed to record genesis event: {}", label, e);
                }
            }

            true
        }
        Err(e) => {
            error!("  FAIL {}: WASM execution error: {}", label, e);
            false
        }
    }
}

fn genesis_initialize_contracts(state: &StateStore, deployer_pubkey: &Pubkey, label: &str) {
    info!("──────────────────────────────────────────────────────");
    info!("  {} Initializing all contracts", label);
    info!("──────────────────────────────────────────────────────");

    let admin = deployer_pubkey.0;
    let mut initialized: usize = 0;
    let mut skipped: usize = 0;

    // Build a lookup: dir_name -> Pubkey for cross-references
    let mut address_map: HashMap<String, Pubkey> = HashMap::new();
    for &(dir_name, _symbol, _display, _template) in GENESIS_CONTRACT_CATALOG {
        if let Some(addr) = derive_contract_address(deployer_pubkey, dir_name) {
            address_map.insert(dir_name.to_string(), addr);
        }
    }

    // ── Initialization in dependency order ──
    // Layer 0: Tokens (no dependencies)
    // Layer 1: Identity
    // Layer 2: DEX core (opcode dispatch)
    // Layer 3: DEX infrastructure (opcode dispatch)
    // Layer 4: DeFi protocols
    // Layer 5: Applications

    // Define initialization config for each contract:
    // (dir_name, function_name, args_builder)
    // For opcode-dispatch contracts: function="call", args=[0x00][admin 32B]
    // For named-export contracts: function="initialize" (or variant), args=[admin 32B]

    struct InitSpec {
        dir_name: &'static str,
        function: &'static str,
        /// Build arguments. We pass in admin pubkey and address map.
        args: Vec<u8>,
    }

    // Helper: build opcode-dispatch init args [0x00][admin 32B]
    fn opcode_init_args(admin: &[u8; 32]) -> Vec<u8> {
        let mut args = Vec::with_capacity(33);
        args.push(0x00); // opcode 0 = initialize
        args.extend_from_slice(admin);
        args
    }

    // Helper: build named-export init args = just [admin 32B]
    fn named_init_args(admin: &[u8; 32]) -> Vec<u8> {
        admin.to_vec()
    }

    // Resolve token contract addresses for moltswap and moltdao
    let molt_addr = address_map
        .get("moltcoin")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);
    let musd_addr = address_map
        .get("musd_token")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);

    // DAO: governance_token = MOLT address, treasury = deployer (initially),
    // min_proposal_threshold = 10,000 MOLT in shells (10_000 * 1e9)
    let dao_threshold: u64 = 10_000_000_000_000; // 10,000 MOLT
    let mut dao_args = Vec::with_capacity(72);
    dao_args.extend_from_slice(&molt_addr); // governance_token (32B)
    dao_args.extend_from_slice(&admin); // treasury (32B = deployer)
    dao_args.extend_from_slice(&dao_threshold.to_le_bytes()); // min_proposal_threshold (8B)

    // MoltSwap: token_a = MOLT, token_b = MUSD
    let mut moltswap_args = Vec::with_capacity(64);
    moltswap_args.extend_from_slice(&molt_addr);
    moltswap_args.extend_from_slice(&musd_addr);

    // MoltMarket: owner(32B) + fee_addr(32B) = deployer for both initially
    let mut moltmarket_args = Vec::with_capacity(64);
    moltmarket_args.extend_from_slice(&admin);
    moltmarket_args.extend_from_slice(&admin); // fee recipient = deployer initially

    // MoltAuction: initialize(marketplace_addr) + initialize_ma_admin(admin)
    // marketplace_addr = moltmarket address for integration
    let moltmarket_addr = address_map.get("moltmarket").map(|p| p.0).unwrap_or(admin);

    let specs: Vec<InitSpec> = vec![
        // ── Layer 0: Tokens ──
        InitSpec {
            dir_name: "moltcoin",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "musd_token",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "wsol_token",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "weth_token",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 1: Identity ──
        InitSpec {
            dir_name: "moltyid",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 2: DEX core (opcode dispatch) ──
        InitSpec {
            dir_name: "dex_core",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_amm",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_router",
            function: "call",
            args: opcode_init_args(&admin),
        },
        // ── Layer 3: DEX infrastructure (opcode dispatch) ──
        InitSpec {
            dir_name: "dex_margin",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_rewards",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_governance",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_analytics",
            function: "call",
            args: opcode_init_args(&admin),
        },
        // ── Layer 4: DeFi protocols ──
        InitSpec {
            dir_name: "moltswap",
            function: "initialize",
            args: moltswap_args,
        },
        InitSpec {
            dir_name: "moltbridge",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "moltoracle",
            function: "initialize_oracle",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "lobsterlend",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 4b: Governance ──
        InitSpec {
            dir_name: "moltdao",
            function: "initialize_dao",
            args: dao_args,
        },
        // ── Layer 5: Marketplaces ──
        InitSpec {
            dir_name: "moltmarket",
            function: "initialize",
            args: moltmarket_args,
        },
        InitSpec {
            dir_name: "moltpunks",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 5b: Infrastructure ──
        InitSpec {
            dir_name: "clawpay",
            function: "initialize_cp_admin",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "clawpump",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "clawvault",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "compute_market",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "reef_storage",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 5c: Prediction Markets ──
        InitSpec {
            dir_name: "prediction_market",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 5d: BountyBoard ──
        // bountyboard.initialize() sets identity_admin which is required by
        // verify_identity, update_reputation, and issue_credential.
        // Without this, first-caller-wins vulnerability (see G22-02).
        InitSpec {
            dir_name: "bountyboard",
            function: "initialize",
            args: named_init_args(&admin),
        },
    ];

    for spec in &specs {
        let pubkey = match address_map.get(spec.dir_name) {
            Some(pk) => *pk,
            None => {
                warn!(
                    "  SKIP {}: address not derived (WASM missing?)",
                    spec.dir_name
                );
                skipped += 1;
                continue;
            }
        };

        if genesis_exec_contract(
            state,
            &pubkey,
            deployer_pubkey,
            spec.function,
            &spec.args,
            spec.dir_name,
        ) {
            info!("  INIT {}", spec.dir_name);
            initialized += 1;
        } else {
            skipped += 1;
        }
    }

    // MoltAuction requires TWO init calls:
    // 1. initialize(marketplace_addr) — sets escrow address
    // 2. initialize_ma_admin(admin) — sets admin
    if let Some(auction_pk) = address_map.get("moltauction") {
        let mkt_args = moltmarket_addr.to_vec();
        if genesis_exec_contract(
            state,
            auction_pk,
            deployer_pubkey,
            "initialize",
            &mkt_args,
            "moltauction(escrow)",
        ) {
            if genesis_exec_contract(
                state,
                auction_pk,
                deployer_pubkey,
                "initialize_ma_admin",
                admin.as_ref(),
                "moltauction(admin)",
            ) {
                info!("  INIT moltauction (escrow + admin)");
                initialized += 1;
            } else {
                skipped += 1;
            }
        } else {
            skipped += 1;
        }
    }

    // ── Prediction Market: wire up cross-contract addresses ──
    // Set oracle, musd, moltyid, and dex_gov addresses via opcode dispatch.
    // Opcodes: 18=set_moltyid, 19=set_oracle, 20=set_musd, 21=set_dex_gov
    // Format: [opcode][admin 32B][address 32B] = 65 bytes
    if let Some(predict_pk) = address_map.get("prediction_market") {
        let oracle_addr = address_map.get("moltoracle").map(|p| p.0).unwrap_or(admin);
        let moltyid_addr = address_map.get("moltyid").map(|p| p.0).unwrap_or(admin);
        let dex_gov_addr = address_map
            .get("dex_governance")
            .map(|p| p.0)
            .unwrap_or(admin);

        // NOTE: MoltyID address IS set here. The processor's cross-contract
        // storage injection reads the caller's MoltyID reputation from
        // CF_CONTRACT_STORAGE and injects it into the contract's execution
        // context before WASM runs. The contract's load_u64("rep:{hex}")
        // call finds the injected value in ctx.storage.
        let configs: &[(u8, &[u8; 32], &str)] = &[
            (18, &moltyid_addr, "prediction_market(moltyid)"),
            (19, &oracle_addr, "prediction_market(oracle)"),
            (20, &musd_addr, "prediction_market(musd)"),
            (21, &dex_gov_addr, "prediction_market(dex_gov)"),
        ];

        for &(opcode, addr, label) in configs {
            let mut args = Vec::with_capacity(65);
            args.push(opcode);
            args.extend_from_slice(&admin);
            args.extend_from_slice(addr);
            if genesis_exec_contract(state, predict_pk, deployer_pubkey, "call", &args, label) {
                info!("  SET {}", label);
            } else {
                warn!("  WARN: Failed to set {}", label);
            }
        }
    }

    // ── DEX Governance: wire up MoltyID address for reputation verification ──
    // Opcode 14 = set_moltyid_address. Format: [14][admin 32B][moltyid_addr 32B]
    if let Some(dex_gov_pk) = address_map.get("dex_governance") {
        let moltyid_addr = address_map.get("moltyid").map(|p| p.0).unwrap_or(admin);
        let mut args = Vec::with_capacity(65);
        args.push(14u8);
        args.extend_from_slice(&admin);
        args.extend_from_slice(&moltyid_addr);
        if genesis_exec_contract(
            state,
            dex_gov_pk,
            deployer_pubkey,
            "call",
            &args,
            "dex_governance(moltyid)",
        ) {
            info!("  SET dex_governance(moltyid)");
        } else {
            warn!("  WARN: Failed to set dex_governance(moltyid)");
        }
    }

    // ── MoltyID: Bootstrap admin reputation ──
    // The admin (deployer) needs reputation >= 1000 to create prediction markets,
    // submit governance proposals, resolve markets, etc. The initial identity
    // registration gives only 100. Write directly to MoltyID's contract storage
    // so the admin has the required reputation from genesis.
    if let Some(moltyid_pk) = address_map.get("moltyid") {
        let admin_rep: u64 = 5000; // "Elite" tier — full access to all features
        let hex_chars: &[u8; 16] = b"0123456789abcdef";
        let mut rep_key = Vec::with_capacity(68);
        rep_key.extend_from_slice(b"rep:");
        for &b in admin.iter() {
            rep_key.push(hex_chars[(b >> 4) as usize]);
            rep_key.push(hex_chars[(b & 0x0f) as usize]);
        }
        if let Err(e) = state.put_contract_storage(moltyid_pk, &rep_key, &admin_rep.to_le_bytes()) {
            warn!("  WARN: Failed to set admin reputation in MoltyID: {}", e);
        } else {
            info!(
                "  SET admin MoltyID reputation = {} (Elite tier)",
                admin_rep
            );
        }
    }

    // ── MoltyID: Register reserved .molt names at genesis ──
    // Uses admin_register_reserved_name to bypass reserved-name checks.
    // Format: admin_register_reserved_name(admin_ptr, owner_ptr, name_ptr, name_len, agent_type)
    // Since this is a named export, args = [admin 32B][owner 32B][name bytes][name_len 4B LE][agent_type 1B]
    if let Some(moltyid_pk) = address_map.get("moltyid") {
        // Genesis .molt name registrations:
        // System wallets get their canonical names
        struct GenesisName {
            label: &'static str,
            owner_key: &'static str, // address_map key or "admin" for deployer
            agent_type: u8,          // 0=system
        }

        let genesis_names: &[GenesisName] = &[
            // ── System / Admin wallets ──
            GenesisName {
                label: "moltchain",
                owner_key: "admin",
                agent_type: 0,
            },
            GenesisName {
                label: "treasury",
                owner_key: "admin",
                agent_type: 0,
            },
            GenesisName {
                label: "validator",
                owner_key: "admin",
                agent_type: 0,
            },
            GenesisName {
                label: "system",
                owner_key: "admin",
                agent_type: 0,
            },
            GenesisName {
                label: "admin",
                owner_key: "admin",
                agent_type: 0,
            },
            // ── Core token ──
            GenesisName {
                label: "moltcoin",
                owner_key: "moltcoin",
                agent_type: 0,
            },
            // ── Wrapped tokens ──
            GenesisName {
                label: "musd",
                owner_key: "musd_token",
                agent_type: 0,
            },
            GenesisName {
                label: "wsol",
                owner_key: "wsol_token",
                agent_type: 0,
            },
            GenesisName {
                label: "weth",
                owner_key: "weth_token",
                agent_type: 0,
            },
            // ── DEX ──
            GenesisName {
                label: "dex",
                owner_key: "dex_core",
                agent_type: 0,
            },
            GenesisName {
                label: "amm",
                owner_key: "dex_amm",
                agent_type: 0,
            },
            GenesisName {
                label: "router",
                owner_key: "dex_router",
                agent_type: 0,
            },
            GenesisName {
                label: "margin",
                owner_key: "dex_margin",
                agent_type: 0,
            },
            GenesisName {
                label: "rewards",
                owner_key: "dex_rewards",
                agent_type: 0,
            },
            GenesisName {
                label: "governance",
                owner_key: "dex_governance",
                agent_type: 0,
            },
            GenesisName {
                label: "analytics",
                owner_key: "dex_analytics",
                agent_type: 0,
            },
            // ── DeFi protocols ──
            GenesisName {
                label: "moltswap",
                owner_key: "moltswap",
                agent_type: 0,
            },
            GenesisName {
                label: "bridge",
                owner_key: "moltbridge",
                agent_type: 0,
            },
            GenesisName {
                label: "oracle",
                owner_key: "moltoracle",
                agent_type: 0,
            },
            GenesisName {
                label: "dao",
                owner_key: "moltdao",
                agent_type: 0,
            },
            GenesisName {
                label: "lending",
                owner_key: "lobsterlend",
                agent_type: 0,
            },
            // ── Marketplaces ──
            GenesisName {
                label: "marketplace",
                owner_key: "moltmarket",
                agent_type: 0,
            },
            GenesisName {
                label: "auction",
                owner_key: "moltauction",
                agent_type: 0,
            },
            GenesisName {
                label: "moltpunks",
                owner_key: "moltpunks",
                agent_type: 0,
            },
            // ── Identity ──
            GenesisName {
                label: "moltyid",
                owner_key: "moltyid",
                agent_type: 0,
            },
            // ── Infrastructure ──
            GenesisName {
                label: "clawpay",
                owner_key: "clawpay",
                agent_type: 0,
            },
            GenesisName {
                label: "clawpump",
                owner_key: "clawpump",
                agent_type: 0,
            },
            GenesisName {
                label: "clawvault",
                owner_key: "clawvault",
                agent_type: 0,
            },
            GenesisName {
                label: "bountyboard",
                owner_key: "bountyboard",
                agent_type: 0,
            },
            GenesisName {
                label: "compute",
                owner_key: "compute_market",
                agent_type: 0,
            },
            GenesisName {
                label: "reefstake",
                owner_key: "reef_storage",
                agent_type: 0,
            },
            // ── Prediction Markets ──
            GenesisName {
                label: "predict",
                owner_key: "prediction_market",
                agent_type: 0,
            },
        ];

        for gn in genesis_names {
            let owner_addr = if gn.owner_key == "admin" {
                admin
            } else {
                address_map.get(gn.owner_key).map(|p| p.0).unwrap_or(admin)
            };

            // Build args: [admin 32B][owner 32B][name bytes...][name_len 4B LE][agent_type 1B]
            let name_bytes = gn.label.as_bytes();
            let name_len = name_bytes.len() as u32;
            let mut args = Vec::with_capacity(32 + 32 + name_bytes.len() + 4 + 1);
            args.extend_from_slice(&admin);
            args.extend_from_slice(&owner_addr);
            args.extend_from_slice(name_bytes);
            args.extend_from_slice(&name_len.to_le_bytes());
            args.push(gn.agent_type);

            if genesis_exec_contract(
                state,
                moltyid_pk,
                deployer_pubkey,
                "admin_register_reserved_name",
                &args,
                &format!("moltyid(name:{})", gn.label),
            ) {
                info!(
                    "  NAME {}.molt → {}",
                    gn.label,
                    if gn.owner_key == "admin" {
                        "deployer"
                    } else {
                        gn.owner_key
                    }
                );
            } else {
                warn!("  WARN: Failed to register {}.molt", gn.label);
            }
        }
    }

    info!("──────────────────────────────────────────────────────");
    info!(
        "  Genesis init complete: {} initialized, {} skipped",
        initialized, skipped
    );
    info!("──────────────────────────────────────────────────────");
}

// ========================================================================
//  GENESIS PHASE 3 — Create trading pairs and AMM pools at genesis.
//  Auto-lists MOLT/mUSD pair on dex_core and creates the corresponding
//  AMM pool on dex_amm.  WSOL/mUSD and WETH/mUSD are deferred until the
//  bridge & custody systems are live and tokens have real supply.
// ========================================================================

fn genesis_create_trading_pairs(state: &StateStore, deployer_pubkey: &Pubkey, label: &str) {
    info!("──────────────────────────────────────────────────────");
    info!("  {} Creating trading pairs & AMM pools", label);
    info!("──────────────────────────────────────────────────────");

    let admin = deployer_pubkey.0;

    // Resolve contract addresses
    let dex_core_pk = match derive_contract_address(deployer_pubkey, "dex_core") {
        Some(pk) => pk,
        None => {
            error!("  FAIL: Cannot derive dex_core address");
            return;
        }
    };
    let dex_amm_pk = match derive_contract_address(deployer_pubkey, "dex_amm") {
        Some(pk) => pk,
        None => {
            error!("  FAIL: Cannot derive dex_amm address");
            return;
        }
    };

    // Resolve token addresses
    let molt_addr = derive_contract_address(deployer_pubkey, "moltcoin")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);
    let musd_addr = derive_contract_address(deployer_pubkey, "musd_token")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);
    let wsol_addr = derive_contract_address(deployer_pubkey, "wsol_token")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);
    let weth_addr = derive_contract_address(deployer_pubkey, "weth_token")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);

    // Resolve dex_governance for allowed-quote setup
    let dex_gov_pk = derive_contract_address(deployer_pubkey, "dex_governance");

    // Genesis pair parameters (reasonable defaults for launch):
    // tick_size: 1 (minimum price increment in shells)
    // lot_size: 1_000_000 (minimum order lot = 0.001 tokens)
    // min_order: 1_000 (minimum order value in shells = MIN_ORDER_VALUE)
    let tick_size: u64 = 1;
    let lot_size: u64 = 1_000_000;
    let min_order: u64 = 1_000;

    // All genesis CLOB pairs: 3 mUSD-quoted + 2 MOLT-quoted = 5 pairs
    let pairs: [(&str, [u8; 32], [u8; 32]); 5] = [
        ("MOLT/mUSD", molt_addr, musd_addr),
        ("wSOL/mUSD", wsol_addr, musd_addr),
        ("wETH/mUSD", weth_addr, musd_addr),
        ("wSOL/MOLT", wsol_addr, molt_addr),
        ("wETH/MOLT", weth_addr, molt_addr),
    ];

    let mut created_pairs: usize = 0;
    let mut created_pools: usize = 0;
    let mut allowed_quotes_set: usize = 0;

    // ── Step 1: Set allowed quote tokens (mUSD + MOLT) on dex_core ──
    // opcode 21 = add_allowed_quote: [0x15][caller 32B][quote_addr 32B]
    for (sym, addr) in &[("mUSD", musd_addr), ("MOLT", molt_addr)] {
        let mut args = Vec::with_capacity(65);
        args.push(0x15); // opcode 21  = add_allowed_quote
        args.extend_from_slice(&admin);
        args.extend_from_slice(addr);

        if genesis_exec_contract(
            state,
            &dex_core_pk,
            deployer_pubkey,
            "call",
            &args,
            &format!("dex_core.add_allowed_quote({})", sym),
        ) {
            info!("  ALLOWED QUOTE {} (dex_core)", sym);
            allowed_quotes_set += 1;
        }
    }

    // ── Step 1b: Set allowed quote tokens on dex_governance too ──
    // opcode 15 = add_allowed_quote: [0x0F][caller 32B][quote_addr 32B]
    if let Some(ref gov_pk) = dex_gov_pk {
        for (sym, addr) in &[("mUSD", musd_addr), ("MOLT", molt_addr)] {
            let mut args = Vec::with_capacity(65);
            args.push(0x0F); // opcode 15 = add_allowed_quote
            args.extend_from_slice(&admin);
            args.extend_from_slice(addr);

            if genesis_exec_contract(
                state,
                gov_pk,
                deployer_pubkey,
                "call",
                &args,
                &format!("dex_governance.add_allowed_quote({})", sym),
            ) {
                info!("  ALLOWED QUOTE {} (dex_governance)", sym);
                allowed_quotes_set += 1;
            }
        }
    }

    // ── Step 2: Create CLOB trading pairs via dex_core opcode 1 (create_pair) ──
    // Args: [0x01][caller 32B][base 32B][quote 32B][tick_size 8B][lot_size 8B][min_order 8B]
    for (label, base, quote) in &pairs {
        let mut args = Vec::with_capacity(121);
        args.push(0x01); // opcode 1 = create_pair
        args.extend_from_slice(&admin); // caller
        args.extend_from_slice(base); // base_token
        args.extend_from_slice(quote); // quote_token
        args.extend_from_slice(&tick_size.to_le_bytes());
        args.extend_from_slice(&lot_size.to_le_bytes());
        args.extend_from_slice(&min_order.to_le_bytes());

        if genesis_exec_contract(
            state,
            &dex_core_pk,
            deployer_pubkey,
            "call",
            &args,
            &format!("dex_core.create_pair({})", label),
        ) {
            info!("  PAIR {}", label);
            created_pairs += 1;
        }
    }

    // ── Step 3: Create AMM pools via dex_amm opcode 1 (create_pool) ──
    // Args: [0x01][caller 32B][token_a 32B][token_b 32B][fee_tier 1B][initial_sqrt_price 8B]
    // fee_tier = 2 (30bps)
    // sqrt_price in Q32 fixed-point: value = (1 << 32) * sqrt(real_price)
    // Prices aligned with genesis oracle seeds: MOLT=$0.10, wSOL=$82, wETH=$1,979
    //   MOLT/mUSD  = $0.10         → sqrt_price =  1_358_187_913
    //   wSOL/mUSD  = $82           → sqrt_price = 38_892_583_020
    //   wETH/mUSD  = $1,979        → sqrt_price = 191_065_712_575
    //   wSOL/MOLT  = 820 MOLT      → sqrt_price = 122_989_146_433
    //   wETH/MOLT  = 19,790 MOLT   → sqrt_price = 604_202_834_500
    let fee_tier: u8 = 2; // FEE_TIER_30BPS

    let pool_configs: [(&str, [u8; 32], [u8; 32], u64); 5] = [
        ("MOLT/mUSD", molt_addr, musd_addr, 1_358_187_913), // $0.10
        ("wSOL/mUSD", wsol_addr, musd_addr, 38_892_583_020), // $82
        ("wETH/mUSD", weth_addr, musd_addr, 191_065_712_575), // $1,979
        ("wSOL/MOLT", wsol_addr, molt_addr, 122_989_146_433), // 820 MOLT
        ("wETH/MOLT", weth_addr, molt_addr, 604_202_834_500), // 19,790 MOLT
    ];

    for (label, token_a, token_b, sqrt_price) in &pool_configs {
        let mut args = Vec::with_capacity(106);
        args.push(0x01); // opcode 1 = create_pool
        args.extend_from_slice(&admin); // caller
        args.extend_from_slice(token_a); // token_a
        args.extend_from_slice(token_b); // token_b
        args.push(fee_tier);
        args.extend_from_slice(&sqrt_price.to_le_bytes());

        if genesis_exec_contract(
            state,
            &dex_amm_pk,
            deployer_pubkey,
            "call",
            &args,
            &format!("dex_amm.create_pool({})", label),
        ) {
            info!("  POOL {}", label);
            created_pools += 1;
        }
    }

    info!("──────────────────────────────────────────────────────");
    info!(
        "  Genesis DEX: {} pairs, {} pools, {} allowed quotes",
        created_pairs, created_pools, allowed_quotes_set
    );
    info!("──────────────────────────────────────────────────────");
}

// ========================================================================
//  GENESIS PHASE 4 — Seed Oracle Price Feeds
//  Authorizes the genesis admin as a MOLT price feeder on the moltoracle
//  contract, then submits the initial launch price ($0.10).
//  This ensures oracle-adjusted rewards work from the very first block.
// ========================================================================

fn genesis_seed_oracle(state: &StateStore, deployer_pubkey: &Pubkey, label: &str) {
    info!("──────────────────────────────────────────────────────");
    info!("  {} Seeding oracle price feeds", label);
    info!("──────────────────────────────────────────────────────");

    let admin = deployer_pubkey.0;

    // Resolve moltoracle contract address
    let oracle_pk = match derive_contract_address(deployer_pubkey, "moltoracle") {
        Some(pk) => pk,
        None => {
            warn!("  SKIP oracle seeding: moltoracle address not derived");
            return;
        }
    };

    // Step 1: Authorize genesis admin as MOLT price feeder
    // add_price_feeder(feeder_ptr: 32, asset_ptr: N, asset_len: u32) -> u32
    let asset = b"MOLT";
    let mut feeder_args = Vec::with_capacity(32 + asset.len() + 4);
    feeder_args.extend_from_slice(&admin); // feeder pubkey (32 bytes)
    feeder_args.extend_from_slice(asset); // asset name
    feeder_args.extend_from_slice(&(asset.len() as u32).to_le_bytes()); // asset_len

    if genesis_exec_contract(
        state,
        &oracle_pk,
        deployer_pubkey,
        "add_price_feeder",
        &feeder_args,
        "moltoracle.add_price_feeder(MOLT)",
    ) {
        info!("  FEEDER authorized: genesis admin → MOLT");
    } else {
        warn!("  SKIP feeder authorization failed");
        return;
    }

    // Step 2: Submit initial MOLT price ($0.10 with 8 decimals = 10_000_000)
    // submit_price(feeder_ptr: 32, asset_ptr: N, asset_len: u32, price: u64, decimals: u8) -> u32
    let launch_price: u64 = 10_000_000; // $0.10 with 8 decimals
    let decimals: u8 = 8;
    let mut price_args = Vec::with_capacity(32 + asset.len() + 4 + 8 + 1);
    price_args.extend_from_slice(&admin); // feeder pubkey
    price_args.extend_from_slice(asset); // asset name
    price_args.extend_from_slice(&(asset.len() as u32).to_le_bytes()); // asset_len
    price_args.extend_from_slice(&launch_price.to_le_bytes()); // price
    price_args.push(decimals); // decimals

    if genesis_exec_contract(
        state,
        &oracle_pk,
        deployer_pubkey,
        "submit_price",
        &price_args,
        "moltoracle.submit_price(MOLT=$0.10)",
    ) {
        info!("  PRICE submitted: MOLT = $0.10 (launch price)");
    } else {
        warn!("  SKIP initial price submission failed");
    }

    // ── Step 3: Seed external asset price feeds (wSOL, wETH) ──
    // These provide reference prices for oracle-priced DEX pairs.
    // Prices are approximate current market values; the background
    // WebSocket price feeder will update them to live prices immediately.
    let external_feeds: [(&[u8], u64, &str); 2] = [
        (b"wSOL", 8_200_000_000, "$82.00"),      // $82 at 8 decimals
        (b"wETH", 197_900_000_000, "$1,979.00"), // $1,979 at 8 decimals
    ];

    for (ext_asset, ext_price, display_price) in &external_feeds {
        // Authorize genesis admin as feeder for this asset
        let mut ext_feeder_args = Vec::with_capacity(32 + ext_asset.len() + 4);
        ext_feeder_args.extend_from_slice(&admin);
        ext_feeder_args.extend_from_slice(ext_asset);
        ext_feeder_args.extend_from_slice(&(ext_asset.len() as u32).to_le_bytes());

        let asset_name = core::str::from_utf8(ext_asset).unwrap_or("?");
        if genesis_exec_contract(
            state,
            &oracle_pk,
            deployer_pubkey,
            "add_price_feeder",
            &ext_feeder_args,
            &format!("moltoracle.add_price_feeder({})", asset_name),
        ) {
            info!("  FEEDER authorized: genesis admin → {}", asset_name);
        } else {
            warn!("  SKIP feeder auth for {} failed", asset_name);
            continue;
        }

        // Submit initial price
        let mut ext_price_args = Vec::with_capacity(32 + ext_asset.len() + 4 + 8 + 1);
        ext_price_args.extend_from_slice(&admin);
        ext_price_args.extend_from_slice(ext_asset);
        ext_price_args.extend_from_slice(&(ext_asset.len() as u32).to_le_bytes());
        ext_price_args.extend_from_slice(&ext_price.to_le_bytes());
        ext_price_args.push(decimals); // 8 decimals

        if genesis_exec_contract(
            state,
            &oracle_pk,
            deployer_pubkey,
            "submit_price",
            &ext_price_args,
            &format!("moltoracle.submit_price({}={})", asset_name, display_price),
        ) {
            info!(
                "  PRICE submitted: {} = {} (launch price)",
                asset_name, display_price
            );
        } else {
            warn!("  SKIP initial {} price submission failed", asset_name);
        }
    }

    // ── Step 4: Seed initial analytics prices for oracle-priced pairs ──
    // Write ana_lp_{pair_id} so the RPC /pairs endpoint shows prices from
    // the very first request, before the background price feeder starts.
    genesis_seed_analytics_prices(state, deployer_pubkey);

    info!("──────────────────────────────────────────────────────");
    info!("  Genesis oracle seeding complete (MOLT + wSOL + wETH)");
    info!("──────────────────────────────────────────────────────");
}

// ========================================================================
//  GENESIS PHASE 4b — Seed initial analytics prices for oracle-priced pairs
//  Writes ana_lp_{pair_id} and ana_24h_{pair_id} directly to dex_analytics
//  contract storage so that RPC /pairs and /tickers endpoints return valid
//  prices immediately, before any trades occur or the live feeder starts.
// ========================================================================

fn genesis_seed_analytics_prices(state: &StateStore, deployer_pubkey: &Pubkey) {
    let analytics_pk = match derive_contract_address(deployer_pubkey, "dex_analytics") {
        Some(pk) => pk,
        None => {
            warn!("  SKIP analytics price seeding: dex_analytics not derived");
            return;
        }
    };

    const PRICE_SCALE: u64 = 1_000_000_000;

    // Pair IDs match genesis_create_trading_pairs order:
    //   1=MOLT/mUSD, 2=wSOL/mUSD, 3=wETH/mUSD, 4=wSOL/MOLT, 5=wETH/MOLT
    let molt_usd: f64 = 0.10;
    let wsol_usd: f64 = 82.0;
    let weth_usd: f64 = 1979.0;

    let pair_prices: [(u64, f64); 5] = [
        (1, molt_usd),            // MOLT/mUSD = $0.10
        (2, wsol_usd),            // wSOL/mUSD = $82
        (3, weth_usd),            // wETH/mUSD = $1,979
        (4, wsol_usd / molt_usd), // wSOL/MOLT = 820
        (5, weth_usd / molt_usd), // wETH/MOLT = 19,790
    ];

    for (pair_id, price_f64) in &pair_prices {
        let price_scaled = (*price_f64 * PRICE_SCALE as f64) as u64;

        // Write last price: ana_lp_{pair_id}
        let lp_key = format!("ana_lp_{}", pair_id);
        let _ = state.put_contract_storage(
            &analytics_pk,
            lp_key.as_bytes(),
            &price_scaled.to_le_bytes(),
        );

        // Write 24h stats: ana_24h_{pair_id} (48 bytes)
        // Layout: volume(8) + high(8) + low(8) + open(8) + close(8) + trades(8)
        let mut stats = Vec::with_capacity(48);
        stats.extend_from_slice(&0u64.to_le_bytes()); // volume = 0
        stats.extend_from_slice(&price_scaled.to_le_bytes()); // high = price
        stats.extend_from_slice(&price_scaled.to_le_bytes()); // low = price (not u64::MAX for new pair)
        stats.extend_from_slice(&price_scaled.to_le_bytes()); // open = price
        stats.extend_from_slice(&price_scaled.to_le_bytes()); // close = price
        stats.extend_from_slice(&0u64.to_le_bytes()); // trades = 0
        let stats_key = format!("ana_24h_{}", pair_id);
        let _ = state.put_contract_storage(&analytics_pk, stats_key.as_bytes(), &stats);

        info!("  ANA seeded: pair {} → price {:.4}", pair_id, price_f64);
    }

    // Also write initial candles for each pair so TradingView has data
    // Use unix timestamp for the candle period start, matching the oracle feeder
    let genesis_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // All 9 intervals so every TF has a seed candle
    let all_intervals: [u64; 9] = [60, 300, 900, 3600, 14400, 86400, 259200, 604800, 31536000];
    for (pair_id, price_f64) in &pair_prices {
        let price_scaled = (*price_f64 * PRICE_SCALE as f64) as u64;

        // Candle layout: open(8)+high(8)+low(8)+close(8)+volume(8)+timestamp(8) = 48 bytes
        let mut candle = Vec::with_capacity(48);
        candle.extend_from_slice(&price_scaled.to_le_bytes()); // open
        candle.extend_from_slice(&price_scaled.to_le_bytes()); // high
        candle.extend_from_slice(&price_scaled.to_le_bytes()); // low
        candle.extend_from_slice(&price_scaled.to_le_bytes()); // close
        candle.extend_from_slice(&0u64.to_le_bytes()); // volume
                                                       // timestamp placeholder — overwritten per-interval below
        candle.extend_from_slice(&0u64.to_le_bytes());

        for interval in &all_intervals {
            let candle_start = (genesis_ts / interval) * interval;
            // Store period-start time so TradingView bars align to boundaries
            candle[40..48].copy_from_slice(&candle_start.to_le_bytes());
            let candle_key = format!("ana_c_{}_{}_{}", pair_id, interval, 0);
            let _ = state.put_contract_storage(&analytics_pk, candle_key.as_bytes(), &candle);
            // Set candle count to 1
            let count_key = format!("ana_cc_{}_{}", pair_id, interval);
            let _ = state.put_contract_storage(
                &analytics_pk,
                count_key.as_bytes(),
                &1u64.to_le_bytes(),
            );
            // Set current candle start to the timestamp-based period
            let cur_key = format!("ana_cur_{}_{}", pair_id, interval);
            let _ = state.put_contract_storage(
                &analytics_pk,
                cur_key.as_bytes(),
                &candle_start.to_le_bytes(),
            );
        }
    }
}

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

/// Binance WebSocket aggTrade stream URL for SOL and ETH
const BINANCE_WS_URL: &str = "wss://stream.binance.com:9443/ws/solusdt@aggTrade/ethusdt@aggTrade";

/// Binance REST fallback URL
const BINANCE_REST_URL: &str =
    "https://api.binance.com/api/v3/ticker/price?symbols=[%22SOLUSDT%22,%22ETHUSDT%22]";

/// REST ticker response
#[derive(Deserialize)]
struct BinanceTicker {
    symbol: String,
    price: String,
}

fn spawn_oracle_price_feeder(state: StateStore, deployer_pubkey: Pubkey) {
    tokio::spawn(async move {
        // Resolve contract pubkeys via symbol registry
        let oracle_pk = match state.get_symbol_registry("ORACLE") {
            Ok(Some(entry)) => entry.program,
            _ => {
                warn!("🔮 Oracle price feeder: ORACLE symbol not found, aborting");
                return;
            }
        };
        let analytics_pk = match state.get_symbol_registry("ANALYTICS") {
            Ok(Some(entry)) => entry.program,
            _ => {
                warn!("🔮 Oracle price feeder: ANALYTICS symbol not found, aborting");
                return;
            }
        };

        // Resolve DEX symbol for writing oracle price bands
        let dex_pk = match state.get_symbol_registry("DEX") {
            Ok(Some(entry)) => entry.program,
            _ => {
                warn!("🔮 Oracle price feeder: DEX symbol not found (price bands disabled)");
                // Use a sentinel — bands won't be written but feeder continues
                Pubkey([0u8; 32])
            }
        };

        const PRICE_SCALE: u64 = 1_000_000_000; // 1e9 for DEX price scaling
        const ORACLE_DECIMALS: u8 = 8;
        let feeder = deployer_pubkey.0; // genesis admin is the authorized feeder
        let molt_usd_default: f64 = 0.10;

        // Lock-free atomic price storage shared between WS reader and storage writer
        let wsol_micro = Arc::new(AtomicU64::new(0));
        let weth_micro = Arc::new(AtomicU64::new(0));
        let ws_healthy = Arc::new(AtomicBool::new(false));

        // Spawn WebSocket reader task
        {
            let ws_wsol = wsol_micro.clone();
            let ws_weth = weth_micro.clone();
            let ws_flag = ws_healthy.clone();
            tokio::spawn(async move {
                binance_ws_loop(ws_wsol, ws_weth, ws_flag).await;
            });
        }

        // REST fallback HTTP client (used only when WebSocket is unhealthy)
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        info!("🔮 Oracle price feeder started (WebSocket real-time + 1s storage writes)");

        let candle_intervals: [u64; 9] =
            [60, 300, 900, 3600, 14400, 86400, 259200, 604800, 31536000];

        // Track last-written prices to skip no-op writes
        let mut prev_wsol: u64 = 0;
        let mut prev_weth: u64 = 0;

        // Storage writer loop: 1-second tick
        let mut write_tick = time::interval(Duration::from_secs(1));

        loop {
            write_tick.tick().await;

            // Read current prices from atomics
            let mut cur_wsol = wsol_micro.load(Ordering::Relaxed);
            let mut cur_weth = weth_micro.load(Ordering::Relaxed);

            // REST fallback if WebSocket is not healthy or no prices yet
            if !ws_healthy.load(Ordering::Relaxed) || (cur_wsol == 0 && cur_weth == 0) {
                if let Ok(resp) = http.get(BINANCE_REST_URL).send().await {
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
                                _ => {}
                            }
                        }
                    }
                }
            }

            // Track whether prices actually changed — oracle feed storage writes
            // are skipped when unchanged, but candle writes ALWAYS proceed so
            // that new candle periods are created at correct time boundaries.
            let prices_changed = cur_wsol != prev_wsol || cur_weth != prev_weth;
            if prices_changed {
                prev_wsol = cur_wsol;
                prev_weth = cur_weth;
            }

            let wsol_usd = cur_wsol as f64 / MICRO_SCALE;
            let weth_usd = cur_weth as f64 / MICRO_SCALE;

            if wsol_usd <= 0.0 && weth_usd <= 0.0 {
                continue;
            }

            let now_ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let current_slot = state.get_last_slot().unwrap_or(0);

            // Read current MOLT price from oracle (or use default)
            let molt_usd = match state.get_contract_storage(&oracle_pk, b"price_MOLT") {
                Ok(Some(feed)) if feed.len() >= 8 => {
                    let raw = u64::from_le_bytes(feed[0..8].try_into().unwrap_or([0; 8]));
                    if raw > 0 {
                        raw as f64 / 100_000_000.0
                    } else {
                        molt_usd_default
                    }
                }
                _ => molt_usd_default,
            };

            // Write oracle prices for each external asset — only when changed
            if prices_changed {
                let oracle_feeds: [(&[u8], f64); 2] = [(b"wSOL", wsol_usd), (b"wETH", weth_usd)];

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
            }

            // Update dex_analytics for oracle-priced pairs
            // Pair 1=MOLT/mUSD, 2=wSOL/mUSD, 3=wETH/mUSD, 4=wSOL/MOLT, 5=wETH/MOLT
            let pair_prices: [(u64, f64); 5] = [
                (1, molt_usd), // MOLT/mUSD (fixed oracle price)
                (2, wsol_usd), // wSOL/mUSD
                (3, weth_usd), // wETH/mUSD
                (
                    4,
                    if molt_usd > 0.0 {
                        wsol_usd / molt_usd
                    } else {
                        0.0
                    },
                ), // wSOL/MOLT
                (
                    5,
                    if molt_usd > 0.0 {
                        weth_usd / molt_usd
                    } else {
                        0.0
                    },
                ), // wETH/MOLT
            ];

            // ── Phase C: Write oracle price bands to dex_core storage ──
            // dex_band_{pair_id}: 16 bytes = reference_price(8) + slot(8)
            // The dex_core contract reads this during place_order to enforce
            // ±5% (market) / ±10% (limit) price band protection.
            // Uses slot (not unix timestamp) because get_timestamp() in WASM
            // returns the block slot number.
            if prices_changed && dex_pk.0 != [0u8; 32] {
                for (pair_id, price_f64) in &pair_prices {
                    if *price_f64 <= 0.0 {
                        continue;
                    }
                    let price_scaled = (*price_f64 * PRICE_SCALE as f64) as u64;
                    let band_key = format!("dex_band_{}", pair_id);
                    let mut band_data = Vec::with_capacity(16);
                    band_data.extend_from_slice(&price_scaled.to_le_bytes());
                    band_data.extend_from_slice(&current_slot.to_le_bytes());
                    let _ = state.put_contract_storage(&dex_pk, band_key.as_bytes(), &band_data);
                }
            }

            for (pair_id, price_f64) in &pair_prices {
                if *price_f64 <= 0.0 {
                    continue;
                }
                let price_scaled = (*price_f64 * PRICE_SCALE as f64) as u64;

                // ── Phase B: Trade-driven fallback ──
                // If a real trade occurred within 60 seconds for this pair,
                // skip oracle analytics writes — the trade bridge owns the
                // displayed price and candles. Oracle still writes to
                // moltoracle storage (reference index) unconditionally.
                let ts_key = format!("ana_last_trade_ts_{}", pair_id);
                let last_trade_ts: u64 =
                    match state.get_contract_storage(&analytics_pk, ts_key.as_bytes()) {
                        Ok(Some(d)) if d.len() >= 8 => {
                            u64::from_le_bytes(d[0..8].try_into().unwrap_or([0; 8]))
                        }
                        _ => 0,
                    };
                let trade_active = last_trade_ts > 0 && now_ts.saturating_sub(last_trade_ts) < 60;

                if trade_active {
                    // Active market: trades drive analytics, skip oracle overwrite
                    continue;
                }

                // Update last price + 24h stats only when prices actually changed
                if prices_changed {
                    // Inactive market: oracle writes indicative price
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
                }

                // ALWAYS update candles — even when prices haven't changed,
                // so new candle periods are created at correct time boundaries.
                for &ci in &candle_intervals {
                    oracle_update_candle(
                        &state,
                        &analytics_pk,
                        *pair_id,
                        ci,
                        price_scaled,
                        current_slot,
                        now_ts,
                    );
                }
            }

            debug!(
                "🔮 Oracle prices updated: wSOL=${:.2} wETH=${:.2}",
                wsol_usd, weth_usd
            );
        }
    });
}

/// Binance WebSocket reader loop with auto-reconnect.
/// Connects to aggTrade streams, parses prices, stores in atomics.
/// On disconnect, retries with exponential backoff (1s → 30s max).
async fn binance_ws_loop(wsol: Arc<AtomicU64>, weth: Arc<AtomicU64>, healthy: Arc<AtomicBool>) {
    let mut backoff_secs: u64 = 1;

    loop {
        info!("🔮 Binance WebSocket connecting...");
        healthy.store(false, Ordering::Relaxed);

        match tokio_tungstenite::connect_async(BINANCE_WS_URL).await {
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
//  SUPERVISOR — wraps the validator in a restart loop.
//  When the internal watchdog detects a stall it exits with EXIT_CODE_RESTART;
//  the supervisor catches that and relaunches the process automatically.
//  Pass --no-watchdog to disable the supervisor entirely (e.g. when using
//  systemd Restart=on-failure which already handles restarts).
// ═══════════════════════════════════════════════════════════════════════

fn main() {
    let args: Vec<String> = env::args().collect();

    // If we're the child (worker) process, go straight to the async validator.
    if args.iter().any(|a| a == "--supervised") {
        return run_validator_sync();
    }

    // If the user opted out of the built-in supervisor, also run directly.
    if args.iter().any(|a| a == "--no-watchdog") {
        return run_validator_sync();
    }

    // Parse supervisor-specific flags
    let max_restarts = args
        .iter()
        .position(|a| a == "--max-restarts")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_RESTARTS);

    // ── Supervisor loop ─────────────────────────────────────────────
    // Re-exec ourselves with --supervised so the child enters run_validator()
    // directly.  On EXIT_CODE_RESTART → restart.  On 0 or SIGTERM → stop.
    let exe = env::current_exe().expect("Cannot determine own executable path");

    // Build child args: forward everything except supervisor-only flags,
    // then append --supervised.
    let child_args: Vec<String> = args[1..]
        .iter()
        .filter(|a| {
            !matches!(
                a.as_str(),
                "--no-watchdog" | "--max-restarts" | "--supervised"
            )
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
        let mut child = std::process::Command::new(&exe)
            .args(&child_args)
            .arg("--supervised")
            .stdin(std::process::Stdio::null())
            .spawn()
            .expect("Failed to spawn validator process");

        let status = child.wait().expect("Failed to wait on validator process");

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
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime");
    rt.block_on(run_validator());
}

/// The actual validator entrypoint — all existing logic lives here.
async fn run_validator() {
    // ── Logging ──
    // Parse data-dir early so we can place log files inside it.
    let pre_args: Vec<String> = env::args().collect();
    let pre_data_dir = pre_args
        .iter()
        .position(|arg| arg == "--db-path" || arg == "--db" || arg == "--data-dir")
        .and_then(|pos| pre_args.get(pos + 1))
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let port = pre_args
                .iter()
                .position(|arg| arg == "--p2p-port")
                .and_then(|pos| pre_args.get(pos + 1))
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(8000);
            format!("./data/state-{}", port)
        });
    let log_dir = PathBuf::from(&pre_data_dir).join("logs");
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
    let genesis_path = args
        .iter()
        .position(|arg| arg == "--genesis")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.to_string());

    // Parse --network flag (testnet | mainnet)
    let network_arg = args
        .iter()
        .position(|arg| arg == "--network")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.to_lowercase());

    // Parse --p2p-port flag properly
    let p2p_port = args
        .iter()
        .position(|arg| arg == "--p2p-port")
        .and_then(|pos| args.get(pos + 1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8000);

    // Parse --db-path / --db / --data-dir flag or use default based on port
    let data_dir = args
        .iter()
        .position(|arg| arg == "--db-path" || arg == "--db" || arg == "--data-dir")
        .and_then(|pos| args.get(pos + 1))
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

    // Open state database
    let state = match StateStore::open(&data_dir) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open state: {}", e);
            return;
        }
    };

    // Create transaction processor
    let processor = Arc::new(TxProcessor::new(state.clone()));

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
    let mut skip_next = false;
    for (i, arg) in args.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }

        match arg.as_str() {
            "--bootstrap" => {
                if let Some(value) = args.get(i + 1) {
                    seed_peer_strings.push(value.to_string());
                    explicit_seed_peer_strings.push(value.to_string());
                }
                skip_next = true;
            }
            "--bootstrap-peers" => {
                if let Some(value) = args.get(i + 1) {
                    for part in value.split(',') {
                        seed_peer_strings.push(part.to_string());
                        explicit_seed_peer_strings.push(part.to_string());
                    }
                }
                skip_next = true;
            }
            "--rpc-port"
            | "--ws-port"
            | "--p2p-port"
            | "--db-path"
            | "--genesis"
            | "--keypair"
            | "--import-key"
            | "--network"
            | "--admin-token"
            | "--watchdog-timeout"
            | "--max-restarts"
            | "--listen-addr"
            | "--auto-update"
            | "--update-check-interval"
            | "--update-channel" => {
                skip_next = true;
            }
            "--supervised" | "--no-watchdog" | "--no-auto-restart" | "--dev-mode" => {
                // Supervisor flags / boolean flags — skip without consuming next arg
                continue;
            }
            _ => {
                if i == 0 {
                    continue; // binary name
                }
                seed_peer_strings.push(arg.to_string());
                explicit_seed_peer_strings.push(arg.to_string());
            }
        }
    }

    // Parse --listen-addr flag for P2P bind address (default: 127.0.0.1 = local only)
    // For VPS / production use: --listen-addr 0.0.0.0
    let listen_host = args
        .iter()
        .position(|arg| arg == "--listen-addr")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());

    // ── Auto-Update Configuration ───────────────────────────────────────
    let auto_update_mode = args
        .iter()
        .position(|arg| arg == "--auto-update")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| updater::UpdateMode::parse_mode(s))
        .unwrap_or(updater::UpdateMode::Off);

    let update_check_interval = args
        .iter()
        .position(|arg| arg == "--update-check-interval")
        .and_then(|pos| args.get(pos + 1))
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(300);

    let update_channel = args
        .iter()
        .position(|arg| arg == "--update-channel")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "stable".to_string());

    let no_auto_restart = args.iter().any(|a| a == "--no-auto-restart");

    let update_config = updater::UpdateConfig {
        mode: auto_update_mode,
        check_interval_secs: update_check_interval,
        channel: update_channel,
        no_auto_restart,
        jitter_max_secs: 60,
    };

    // Spawn auto-updater background task
    info!("🔄 Validator version: v{}", updater::VERSION);
    let _updater_handle = updater::spawn_update_checker(update_config);

    let data_dir_path = Path::new(&data_dir);
    let peer_store_path = data_dir_path.join("known-peers.json");
    let listen_addr: SocketAddr = format!("{}:{}", listen_host, p2p_port)
        .parse()
        .expect("Invalid listen address (check --listen-addr)");

    let mut seed_peers = resolve_peer_list(&seed_peer_strings);
    let explicit_seed_peers = resolve_peer_list(&explicit_seed_peer_strings);
    let seeds_path = Path::new("seeds.json");
    let local_only = listen_addr.ip().is_loopback();
    let cached_peers = if explicit_seed_peers.is_empty() && !local_only {
        let seed_file_peers = load_seed_peers(&genesis_config.chain_id, seeds_path);
        seed_peers.extend(resolve_peer_list(&seed_file_peers));
        let cached = moltchain_p2p::PeerStore::load_from_path(&peer_store_path);
        seed_peers.extend(cached.iter().copied());
        cached
    } else {
        info!("🔒 Local bootstrap only: external seed peers disabled");
        Vec::new()
    };

    let mut seen = HashSet::new();
    seed_peers.retain(|addr| {
        if *addr == listen_addr {
            return false;
        }
        seen.insert(*addr)
    });

    let p2p_config = P2PConfig {
        listen_addr,
        seed_peers: seed_peers.clone(),
        gossip_interval: 10,
        cleanup_timeout: 300,
        peer_store_path: Some(peer_store_path.clone()),
        max_known_peers: 200,
    };

    let has_genesis_block = state.get_block_by_slot(0).unwrap_or(None).is_some();

    // Join network if we have seed peers and no local genesis yet
    let mut is_joining_network =
        (!explicit_seed_peers.is_empty() || !cached_peers.is_empty()) && !has_genesis_block;

    // ========================================================================
    // GENESIS STATE INITIALIZATION
    // ========================================================================

    // Genesis wallet path
    let genesis_wallet_path = data_dir_path.join("genesis-wallet.json");
    let genesis_keypairs_dir = data_dir_path.join("genesis-keys");
    std::fs::create_dir_all(&genesis_keypairs_dir).ok();

    // DYNAMIC GENESIS GENERATION
    // First validator starting after reset generates everything fresh
    let mut genesis_signer: Option<Keypair> = None;
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
    } else if !is_joining_network {
        info!("🔐 Generating FRESH genesis wallet (DYNAMIC GENERATION)");

        // Production-ready multi-sig for BOTH testnet and mainnet
        let is_mainnet = genesis_config.chain_id.contains("mainnet");
        let (signer_count, threshold_desc) = if is_mainnet {
            (5, "3/5 production multi-sig")
        } else {
            (3, "2/3 testnet multi-sig")
        };

        info!("  🔐 Creating {} setup...", threshold_desc);

        // Generate genesis wallet with multi-sig
        let (wallet, keypairs, distribution_keypairs) =
            GenesisWallet::generate(&genesis_config.chain_id, is_mainnet, signer_count)
                .expect("Failed to generate genesis wallet");

        genesis_signer = keypairs
            .first()
            .map(|keypair| Keypair::from_seed(&keypair.to_seed()));

        let pubkey = wallet.pubkey; // Extract before moving
        info!("  ✓ Generated genesis pubkey: {}", pubkey);

        if let Some(ref multisig) = wallet.multisig {
            info!("  ✓ Multi-sig configuration:");
            info!(
                "    - Threshold: {}/{} signatures",
                multisig.threshold,
                multisig.signers.len()
            );
            info!("    - Genesis treasury: {}", multisig.is_genesis);
            info!("    - Signers:");
            for (i, signer) in multisig.signers.iter().enumerate() {
                info!("      {}. {}", i + 1, signer.to_base58());
            }
        }

        // Log whitepaper distribution
        if let Some(ref dist) = wallet.distribution_wallets {
            info!(
                "  📊 Whitepaper genesis distribution ({} wallets):",
                dist.len()
            );
            for dw in dist {
                info!(
                    "    - {} ({}%): {} MOLT → {}",
                    dw.role,
                    dw.percentage,
                    dw.amount_molt,
                    dw.pubkey.to_base58()
                );
            }
        }

        // Save wallet info
        wallet
            .save(&genesis_wallet_path)
            .expect("Failed to save genesis wallet");
        info!("  ✓ Wallet info saved: {}", genesis_wallet_path.display());

        // Save all signer keypairs
        let keypair_paths = GenesisWallet::save_keypairs(
            &keypairs,
            &genesis_keypairs_dir,
            &genesis_config.chain_id,
        )
        .expect("Failed to save keypairs");

        // Save all distribution keypairs (one per whitepaper wallet)
        let dist_keypair_paths = GenesisWallet::save_distribution_keypairs(
            wallet.distribution_wallets.as_deref().unwrap_or(&[]),
            &distribution_keypairs,
            &genesis_keypairs_dir,
            &genesis_config.chain_id,
        )
        .expect("Failed to save distribution keypairs");

        // Save treasury keypair separately for backward compat (start-local-stack.sh)
        // Treasury = validator_rewards = first distribution keypair
        let treasury_keypair_path = GenesisWallet::save_treasury_keypair(
            &distribution_keypairs[0],
            &genesis_keypairs_dir,
            &genesis_config.chain_id,
        )
        .expect("Failed to save treasury keypair");

        info!("  ✓ Saved {} signer keypair(s):", keypair_paths.len());
        for path in &keypair_paths {
            info!("    - {}", path);
        }
        info!(
            "  ✓ Saved {} distribution keypair(s):",
            dist_keypair_paths.len()
        );
        for path in &dist_keypair_paths {
            info!("    - {}", path);
        }
        info!("  ✓ Treasury keypair: {}", treasury_keypair_path);

        info!("  ⚠️  KEEP THESE FILES SECURE - THEY CONTROL THE GENESIS TREASURY");

        (Some(wallet), Some(pubkey))
    } else {
        // Joining network - will sync genesis from peers
        info!("🔄 Joining existing network - genesis wallet will sync from peers");
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
                    // V2/V3 validators (e.g. p2p=8001 → 8903, actual RPC=8901).
                    let base_p2p = if peer_p2p >= 9000 { 9000u16 } else { 8000u16 };
                    let base_rpc = if peer_p2p >= 9000 { 9899u16 } else { 8899u16 };
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

    if !genesis_exists && !is_joining_network {
        let genesis_pubkey = genesis_pubkey.expect("Missing genesis pubkey for creation");
        let genesis_wallet = genesis_wallet
            .as_ref()
            .expect("Missing genesis wallet for creation");
        info!("📦 Creating genesis state from auto-generated wallet");

        if let Err(e) = state.set_rent_params(
            genesis_config.features.rent_rate_shells_per_kb_month,
            genesis_config.features.rent_free_kb,
        ) {
            warn!("⚠️  Failed to store rent params: {}", e);
        }

        // Persist fee configuration from genesis config into DB
        let genesis_fee_config = FeeConfig {
            base_fee: genesis_config.features.base_fee_shells,
            contract_deploy_fee: CONTRACT_DEPLOY_FEE,
            contract_upgrade_fee: CONTRACT_UPGRADE_FEE,
            nft_mint_fee: NFT_MINT_FEE,
            nft_collection_fee: NFT_COLLECTION_FEE,
            fee_burn_percent: genesis_config.features.fee_burn_percentage,
            fee_producer_percent: genesis_config.features.fee_producer_percentage,
            fee_voters_percent: genesis_config.features.fee_voters_percentage,
            fee_treasury_percent: 100u64
                .saturating_sub(genesis_config.features.fee_burn_percentage)
                .saturating_sub(genesis_config.features.fee_producer_percentage)
                .saturating_sub(genesis_config.features.fee_voters_percentage),
        };
        if let Err(e) = state.set_fee_config_full(&genesis_fee_config) {
            warn!("⚠️  Failed to store fee config: {}", e);
        } else {
            info!("  ✓ Fee config persisted: base={} shells, burn={}%, producer={}%, voters={}%, treasury={}%",
                genesis_fee_config.base_fee,
                genesis_fee_config.fee_burn_percent,
                genesis_fee_config.fee_producer_percent,
                genesis_fee_config.fee_voters_percent,
                genesis_fee_config.fee_treasury_percent,
            );
        }

        // Create genesis treasury account with full supply
        let total_supply_molt = 1_000_000_000u64;
        let mut genesis_account = Account::new(total_supply_molt, genesis_pubkey);

        // Store multi-sig configuration in account metadata (if enabled)
        if let Some(ref multisig) = genesis_wallet.multisig {
            // Mark as genesis treasury
            genesis_account.owner = genesis_pubkey; // Self-owned
            info!("  ✓ Flagged as genesis treasury with multi-sig");
            info!(
                "    Threshold: {}/{} signatures",
                multisig.threshold,
                multisig.signers.len()
            );
        }

        if let Err(e) = state.put_account(&genesis_pubkey, &genesis_account) {
            eprintln!("Failed to store genesis account: {e}");
        }
        if let Err(e) = state.set_genesis_pubkey(&genesis_pubkey) {
            eprintln!("Failed to set genesis pubkey: {e}");
        }
        info!("  ✓ Genesis mint: {} MOLT", total_supply_molt);
        info!("  ✓ Address: {}", genesis_pubkey.to_base58());

        // ════════════════════════════════════════════════════
        // WHITEPAPER GENESIS DISTRIBUTION (6 wallets, 1B total)
        // ════════════════════════════════════════════════════
        // Apply distribution directly to state — cannot use process_transaction()
        // here because no blocks exist yet and T1.3 rejects zero-blockhash txs.
        // Corresponding ledger entries are recorded in the genesis block below.
        if let Some(ref dist_wallets) = genesis_wallet.distribution_wallets {
            info!("📊 Creating whitepaper genesis distribution:");

            let mut src_acct = match state.get_account(&genesis_pubkey).ok().flatten() {
                Some(a) => a,
                None => {
                    error!("Genesis account missing after creation — cannot distribute");
                    Account::new(0, genesis_pubkey)
                }
            };

            for dw in dist_wallets {
                let amount_shells = Account::molt_to_shells(dw.amount_molt);

                // Create distribution account
                let mut acct = Account::new(0, SYSTEM_ACCOUNT_OWNER);
                acct.shells = amount_shells;
                acct.spendable = amount_shells;
                if let Err(e) = state.put_account(&dw.pubkey, &acct) {
                    error!("Failed to create {} account: {e}", dw.role);
                }

                // Debit genesis
                src_acct.shells = src_acct.shells.saturating_sub(amount_shells);
                src_acct.spendable = src_acct.spendable.saturating_sub(amount_shells);

                // Set treasury pubkey for the validator_rewards wallet
                if dw.role == "validator_rewards" {
                    if let Err(e) = state.set_treasury_pubkey(&dw.pubkey) {
                        error!("Failed to set treasury pubkey: {e}");
                    }
                    info!(
                        "  ✓ {} ({}%): {} MOLT → {} [TREASURY]",
                        dw.role,
                        dw.percentage,
                        dw.amount_molt,
                        dw.pubkey.to_base58()
                    );
                } else {
                    info!(
                        "  ✓ {} ({}%): {} MOLT → {}",
                        dw.role,
                        dw.percentage,
                        dw.amount_molt,
                        dw.pubkey.to_base58()
                    );
                }
            }

            if let Err(e) = state.put_account(&genesis_pubkey, &src_acct) {
                error!("Failed to update genesis account after distribution: {e}");
            }

            // Store genesis accounts in state DB for RPC/explorer lookups
            let ga_entries: Vec<(String, Pubkey, u64, u8)> = dist_wallets
                .iter()
                .map(|dw| (dw.role.clone(), dw.pubkey, dw.amount_molt, dw.percentage))
                .collect();
            if let Err(e) = state.set_genesis_accounts(&ga_entries) {
                error!("Failed to store genesis accounts in DB: {e}");
            } else {
                info!(
                    "  ✓ Stored {} genesis accounts in state DB",
                    ga_entries.len()
                );
            }

            info!("  ✓ Genesis distribution complete — 1B MOLT allocated per whitepaper");
        }
        // Legacy: single treasury (backward compat for old wallet files)
        else if let Some(treasury_pubkey) = genesis_wallet.treasury_pubkey {
            let reward_pool_molt = REWARD_POOL_MOLT.min(1_000_000_000);
            let treasury_account = Account::new(0, SYSTEM_ACCOUNT_OWNER);
            if let Err(e) = state.put_account(&treasury_pubkey, &treasury_account) {
                eprintln!("Failed to store treasury account: {e}");
            }
            if let Err(e) = state.set_treasury_pubkey(&treasury_pubkey) {
                eprintln!("Failed to set treasury pubkey: {e}");
            }
            info!(
                "  ✓ Treasury account created: {}",
                treasury_pubkey.to_base58()
            );
            info!("  ✓ Reward pool pending: {} MOLT", reward_pool_molt);

            let reward_shells = Account::molt_to_shells(reward_pool_molt);

            let mut src_acct = match state.get_account(&genesis_pubkey).ok().flatten() {
                Some(a) => a,
                None => {
                    error!("Genesis account missing after creation — cannot fund treasury");
                    Account::new(0, genesis_pubkey)
                }
            };
            src_acct.shells = src_acct.shells.saturating_sub(reward_shells);
            src_acct.spendable = src_acct.spendable.saturating_sub(reward_shells);
            if let Err(e) = state.put_account(&genesis_pubkey, &src_acct) {
                error!("Failed to update genesis account balance: {e}");
            }

            let mut trs_acct = state
                .get_account(&treasury_pubkey)
                .ok()
                .flatten()
                .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));
            trs_acct.shells = trs_acct.shells.saturating_add(reward_shells);
            trs_acct.spendable = trs_acct.spendable.saturating_add(reward_shells);
            if let Err(e) = state.put_account(&treasury_pubkey, &trs_acct) {
                error!("Failed to update treasury account balance: {e}");
            }

            info!("  ✓ Reward pool funded via genesis transfer tx");
        }

        // Create initial accounts from genesis config (if any)
        for account_info in &genesis_config.initial_accounts {
            let pubkey = match Pubkey::from_base58(&account_info.address) {
                Ok(pk) => pk,
                Err(e) => {
                    warn!(
                        "Skipping initial account with invalid address {}: {e}",
                        account_info.address
                    );
                    continue;
                }
            };
            let account = Account::new(account_info.balance_molt, pubkey);
            if let Err(e) = state.put_account(&pubkey, &account) {
                eprintln!("Failed to store initial account: {e}");
            }
            info!(
                "  ✓ Account {}: {} MOLT",
                &account_info.address[..20],
                account_info.balance_molt
            );
        }

        let mut genesis_txs = Vec::new();

        let mint_shells = Account::molt_to_shells(total_supply_molt);
        let mut mint_data = Vec::with_capacity(9);
        mint_data.push(5); // Genesis mint (synthetic, fee-free)
        mint_data.extend_from_slice(&mint_shells.to_le_bytes());

        let mint_instruction = Instruction {
            program_id: CORE_SYSTEM_PROGRAM_ID,
            accounts: vec![GENESIS_MINT_PUBKEY, genesis_pubkey],
            data: mint_data,
        };

        let mint_message = Message::new(vec![mint_instruction], Hash::default());
        let mut mint_tx = Transaction::new(mint_message);
        mint_tx.signatures.push([0u8; 64]);
        genesis_txs.push(mint_tx);

        // Record distribution transfers in genesis block
        // (validator_rewards FIRST for backward-compatible treasury extraction)
        if let Some(ref dist_wallets) = genesis_wallet.distribution_wallets {
            let signer = genesis_signer
                .as_ref()
                .expect("Missing genesis signer for distribution funding");

            for dw in dist_wallets {
                let mut data = Vec::with_capacity(9);
                data.push(4); // Genesis transfer (fee-free)
                data.extend_from_slice(&Account::molt_to_shells(dw.amount_molt).to_le_bytes());

                let instruction = Instruction {
                    program_id: CORE_SYSTEM_PROGRAM_ID,
                    accounts: vec![genesis_pubkey, dw.pubkey],
                    data,
                };

                let message = Message::new(vec![instruction], Hash::default());
                let mut tx = Transaction::new(message.clone());
                let signature = signer.sign(&message.serialize());
                tx.signatures.push(signature);
                genesis_txs.push(tx);
            }
        }
        // Legacy: single treasury transfer (backward compat)
        else if let Some(treasury_pubkey) = genesis_wallet.treasury_pubkey {
            let reward_pool_molt = REWARD_POOL_MOLT.min(1_000_000_000);
            let mut data = Vec::with_capacity(9);
            data.push(4); // Genesis transfer (fee-free)
            data.extend_from_slice(&Account::molt_to_shells(reward_pool_molt).to_le_bytes());

            let instruction = Instruction {
                program_id: CORE_SYSTEM_PROGRAM_ID,
                accounts: vec![genesis_pubkey, treasury_pubkey],
                data,
            };

            let message = Message::new(vec![instruction], Hash::default());
            let mut treasury_tx = Transaction::new(message.clone());
            let signer = genesis_signer
                .as_ref()
                .expect("Missing genesis signer for treasury funding");
            let signature = signer.sign(&message.serialize());
            treasury_tx.signatures.push(signature);
            genesis_txs.push(treasury_tx);
        }

        // Create genesis block
        let state_root = state.compute_state_root();
        let genesis_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let genesis_block = Block::genesis(state_root, genesis_timestamp, genesis_txs);
        if let Err(e) = state.put_block(&genesis_block) {
            error!("Failed to store genesis block: {e}");
        }
        if let Err(e) = state.set_last_slot(0) {
            error!("Failed to set initial slot: {e}");
        }
        info!("✓ Genesis block created and stored (slot 0)");
        info!("  Genesis hash: {}", genesis_block.hash());

        // Auto-deploy all compiled contracts from contracts/ directory
        genesis_auto_deploy(&state, &genesis_pubkey, "FIRST-BOOT:");
        genesis_initialize_contracts(&state, &genesis_pubkey, "FIRST-BOOT:");
        genesis_create_trading_pairs(&state, &genesis_pubkey, "FIRST-BOOT:");
        genesis_seed_oracle(&state, &genesis_pubkey, "FIRST-BOOT:");
    } else if genesis_exists {
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

    // Treasury keypair kept for governance/manual operations only.
    // Block rewards use protocol-level coinbase (no signing needed).
    let _treasury_keypair = load_treasury_keypair(
        genesis_wallet.as_ref(),
        &genesis_keypairs_dir,
        &genesis_config.chain_id,
    );

    // ========================================================================
    // VALIDATOR IDENTITY
    // ========================================================================

    // Parse --dev-mode flag (disables machine fingerprint, blocks mainnet)
    let dev_mode = args.iter().any(|arg| arg == "--dev-mode");
    if dev_mode {
        info!("🔧 Developer mode enabled — machine fingerprint disabled");
        if genesis_config.chain_id.contains("mainnet") {
            error!("❌ --dev-mode cannot be used on mainnet — aborting");
            std::process::exit(1);
        }
    }

    // Parse --import-key: copy an existing keypair file into the validator data directory,
    // then use it as the validator identity. This is for machine migration.
    if let Some(import_pos) = args.iter().position(|arg| arg == "--import-key") {
        if let Some(import_path) = args.get(import_pos + 1) {
            let source = Path::new(import_path);
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
            fs::copy(source, &dest).expect("Failed to copy keypair file for --import-key");
            // Set restrictive permissions
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).ok();
            }
            info!("✅ Keypair imported successfully — this validator will resume the imported identity");
        } else {
            error!("❌ --import-key requires a file path argument");
            std::process::exit(1);
        }
    }

    // Load validator keypair from file (production-ready)
    // Priority order:
    // 1. --keypair CLI argument
    // 2. MOLTCHAIN_VALIDATOR_KEYPAIR env var
    // 3. ~/.moltchain/validators/validator-{port}.json
    // 4. Generate new and save

    let keypair_path = args
        .iter()
        .position(|arg| arg == "--keypair")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.as_str());

    let validator_keypair = keypair_loader::load_or_generate_keypair(keypair_path, p2p_port)
        .expect("Failed to load or generate validator keypair");

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
    let (block_tx, mut block_rx) = mpsc::channel(500);
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

    // Create mempool
    let mempool = Arc::new(Mutex::new(Mempool::new(1000, 300))); // 1000 tx max, 300s expiration

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
            let sync_mgr_for_genesis_retry = sync_manager.clone();
            tokio::spawn(async move {
                let mut interval = time::interval(Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    if let Ok(Some(_)) = state_for_genesis_retry.get_block_by_slot(0) {
                        break;
                    }

                    // Skip if we already sent the request recently
                    if sync_mgr_for_genesis_retry.is_requested(0).await {
                        continue;
                    }

                    let request = P2PMessage::new(
                        MessageType::BlockRangeRequest {
                            start_slot: 0,
                            end_slot: 0,
                        },
                        local_addr_for_genesis_retry,
                    );
                    peer_mgr_for_genesis_retry.broadcast(request).await;
                    sync_mgr_for_genesis_retry.mark_requested(0).await;
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

    // Start incoming block handler with voting
    if let Some(ref p2p_pm) = p2p_peer_manager {
        let state_for_blocks = state.clone();
        let processor_for_blocks = processor.clone();
        let validator_pubkey_for_blocks = validator_pubkey;
        let validator_seed = validator_keypair.to_seed(); // Store seed to reconstruct keypair
        let sync_mgr = sync_manager.clone();
        let peer_mgr_for_sync = p2p_pm.clone();
        let vote_agg_for_blocks = vote_aggregator.clone();
        let validator_set_for_blocks = validator_set.clone();
        let stake_pool_for_blocks = stake_pool.clone();
        let vote_agg_for_effects = vote_aggregator.clone();
        let local_addr = p2p_config.listen_addr;
        let last_block_time_for_blocks = last_block_time_for_blocks.clone();
        let genesis_config_for_blocks = genesis_config.clone();
        // genesis_time_secs_for_blocks and slot_duration_ms_for_blocks removed:
        // Timestamp validation now uses wall-clock only, not slot-derived timestamps.
        let slashing_for_blocks = slashing_tracker.clone();
        let validator_pubkey_for_block_slash = validator_pubkey;
        let received_slots_for_rx = received_network_slots_for_blocks.clone();
        let tip_notify_for_blocks = tip_notify_for_blocks.clone();
        let data_dir_for_blocks = data_dir.clone();
        let finality_for_blocks = finality_tracker.clone();
        tokio::spawn(async move {
            info!("🔄 Block receiver started");
            // 1.7: Track (slot, validator) → block_hash to detect double-block equivocation
            let mut seen_blocks: HashMap<(u64, [u8; 32]), Hash> = HashMap::new();
            // A5-02: Fork choice oracle — tracks competing chain heads by
            // cumulative stake weight. Used to break ties when multiple valid
            // blocks exist for the same slot.
            let mut fork_choice = ForkChoice::new();
            // Periodically prune old entries (keep last 1000 slots)
            let mut prune_below_slot: u64 = 0;
            while let Some(block) = block_rx.recv().await {
                let block_slot = block.header.slot;

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
                    }
                }

                sync_mgr.note_seen(block_slot).await;

                // FIX-FORK-1: Record that this slot has a valid network block.
                // The production loop checks this set right before creating its
                // own block, preventing the TOCTOU fork where a validator
                // produces a conflicting block for a slot it already received.
                {
                    let mut rns = received_slots_for_rx.lock().await;
                    rns.insert(block_slot);
                    // Prune entries older than 200 slots to bound memory
                    if block_slot > 200 {
                        rns.retain(|&s| s + 200 >= block_slot);
                    }
                }
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
                            fee_treasury_percent: 100u64
                                .saturating_sub(gc.features.fee_burn_percentage)
                                .saturating_sub(gc.features.fee_producer_percentage)
                                .saturating_sub(gc.features.fee_voters_percentage),
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
                            replay_block_transactions(&processor_for_blocks, &pending_block);
                            run_analytics_bridge_from_state(
                                &state_for_blocks,
                                pending_block.header.slot,
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
                        replay_block_transactions(&processor_for_blocks, &block);
                        run_analytics_bridge_from_state(&state_for_blocks, block.header.slot);
                        run_sltp_triggers_from_state(&state_for_blocks);
                        reset_24h_stats_if_expired(&state_for_blocks, block.header.timestamp);
                        if state_for_blocks.put_block(&block).is_ok() {
                            state_for_blocks.set_last_slot(block_slot).ok();
                            *last_block_time_for_blocks.lock().await = std::time::Instant::now();
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

                            // Cast vote for this block (BFT consensus)
                            let block_hash = block.hash();
                            let mut vote_message = Vec::new();
                            vote_message.extend_from_slice(&block_slot.to_le_bytes());
                            vote_message.extend_from_slice(&block_hash.0);

                            // Reconstruct keypair from seed to sign vote
                            let keypair_for_vote = Keypair::from_seed(&validator_seed);
                            let signature = keypair_for_vote.sign(&vote_message);

                            let vote = Vote::new(
                                block_slot,
                                block_hash,
                                validator_pubkey_for_blocks,
                                signature,
                            );

                            // Add our own vote (validated against validator set)
                            {
                                let mut agg = vote_agg_for_blocks.write().await;
                                let vs = validator_set_for_blocks.read().await;
                                if agg.add_vote_validated(vote.clone(), &vs) {
                                    info!("🗳️  Cast vote for block {}", block_slot);

                                    // Check if block reached finality (2/3 supermajority - STAKE-WEIGHTED)
                                    let pool = stake_pool_for_blocks.read().await;
                                    if agg.has_supermajority(block_slot, &block_hash, &vs, &pool) {
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
                            // Don't await the broadcast — let QUIC sends happen
                            // concurrently while we proceed to apply_block_effects.
                            {
                                let vote_msg = P2PMessage::new(MessageType::Vote(vote), local_addr);
                                let pm = peer_mgr_for_sync.clone();
                                tokio::spawn(async move {
                                    pm.broadcast(vote_msg).await;
                                });
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
                                replay_block_transactions(&processor_for_blocks, &pending_block);
                                run_analytics_bridge_from_state(
                                    &state_for_blocks,
                                    pending_block.header.slot,
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
                        info!("🔄 Triggering sync: blocks {} to {}", start, end);

                        // Mark that we're starting sync
                        sync_mgr.start_sync(start, end).await;

                        // Send BlockRangeRequest to all peers
                        let request_msg = P2PMessage::new(
                            MessageType::BlockRangeRequest {
                                start_slot: start,
                                end_slot: end,
                            },
                            local_addr,
                        );
                        peer_mgr_for_sync.broadcast(request_msg).await;
                        info!("📡 Sent block range request: {} to {}", start, end);

                        // Mark slots as requested in sync manager
                        for slot in start..=end {
                            sync_mgr.mark_requested(slot).await;
                        }

                        // Complete sync flag after a delay (will re-trigger if still behind)
                        let sync_mgr_complete = sync_mgr.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                            sync_mgr_complete.complete_sync().await;
                        });
                    }
                } else if block_slot <= current_slot {
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

                            // P9-VAL-07: Only replace based on weight/stake evidence,
                            // not on sync state (we_are_behind) or pending queue (has_pending)
                            // to prevent malicious validators from forcing replacements.
                            if incoming_weight > existing_weight || oracle_prefers_incoming {
                                // Revert old block's financial effects before replacing
                                revert_block_effects(&state_for_blocks, &existing);
                                // C7 fix: Also revert user transaction effects
                                revert_block_transactions(
                                    &state_for_blocks,
                                    &existing,
                                    &data_dir_for_blocks,
                                );
                                // Replace slot index with the higher-weight block
                                replay_block_transactions(&processor_for_blocks, &block);
                                run_analytics_bridge_from_state(
                                    &state_for_blocks,
                                    block.header.slot,
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
                                        replay_block_transactions(
                                            &processor_for_blocks,
                                            &pending_block,
                                        );
                                        run_analytics_bridge_from_state(
                                            &state_for_blocks,
                                            pending_block.header.slot,
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
                // 1. Verify signature — fee payer signs the serialized message
                let sender_pubkey = tx
                    .message
                    .instructions
                    .first()
                    .and_then(|ix| ix.accounts.first())
                    .copied();
                let sig_valid = match (&sender_pubkey, tx.signatures.first()) {
                    (Some(sender), Some(sig)) => {
                        let msg_bytes = tx.message.serialize();
                        Keypair::verify(sender, &msg_bytes, sig)
                    }
                    _ => false,
                };
                if !sig_valid {
                    info!("❌ P2P transaction rejected: invalid or missing signature");
                    continue;
                }
                // 2. Verify sender exists with minimum balance for fee
                if let Some(sender) = &sender_pubkey {
                    match state_for_p2p_txs.get_account(sender) {
                        Ok(Some(acct)) if acct.spendable >= BASE_FEE => {}
                        _ => {
                            info!("❌ P2P transaction rejected: sender missing or insufficient balance");
                            continue;
                        }
                    }
                }
                // 3. Validate transaction structure (size limits, instruction count)
                if let Err(e) = tx.validate_structure() {
                    info!("❌ P2P transaction rejected: {}", e);
                    continue;
                }
                // AUDIT-FIX V5.3: Look up on-chain MoltyID reputation
                // so express-lane priority works for P2P-received transactions.
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
                let vs = validator_set_for_votes.read().await;
                if agg.add_vote_validated(vote.clone(), &vs) {
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

                // Cap validator set size
                const MAX_VALIDATORS: usize = 1000;

                // Check if validator already exists
                if vs.get_validator(&announcement.pubkey).is_some() {
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

                    // 1.5a: Defense-in-depth — re-verify announcement signature
                    //        Signature covers: pubkey + stake + slot + machine_fingerprint
                    {
                        let mut msg = Vec::with_capacity(80);
                        msg.extend_from_slice(&announcement.pubkey.0);
                        msg.extend_from_slice(&announcement.stake.to_le_bytes());
                        msg.extend_from_slice(&announcement.current_slot.to_le_bytes());
                        msg.extend_from_slice(&announcement.machine_fingerprint);
                        if !moltchain_core::account::Keypair::verify(
                            &announcement.pubkey,
                            &msg,
                            &announcement.signature,
                        ) {
                            warn!(
                                "⚠️  Rejecting announcement from {} — invalid signature at handler",
                                announcement.pubkey.to_base58()
                            );
                            drop(vs);
                            continue;
                        }
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
            let mut rate_limits: HashMap<std::net::SocketAddr, (u64, std::time::Instant)> =
                HashMap::new();
            let mut strikes: HashMap<std::net::SocketAddr, u32> = HashMap::new();
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
                    last_prune = std::time::Instant::now();
                }
                if !peer_mgr_for_responses
                    .get_peers()
                    .contains(&request.requester)
                {
                    warn!(
                        "⚠️  Ignoring block range request from unknown peer {}",
                        request.requester
                    );
                    peer_mgr_for_responses.record_violation(&request.requester);
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
                if now.duration_since(entry.1).as_secs() >= 10 {
                    *entry = (0, now);
                }
                entry.0 = entry.0.saturating_add(1);
                if entry.0 > 5 {
                    warn!("⚠️  Rate limit exceeded for {}", request.requester);
                    peer_mgr_for_responses.record_violation(&request.requester);
                    continue;
                }

                let range_size = request.end_slot.saturating_sub(request.start_slot) + 1;

                // Rate limiting: prevent excessive requests
                if range_size > 1000 {
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
                    if blocks.len() >= 500 {
                        warn!("⚠️  Truncating block response at 500 blocks");
                        break;
                    }
                }

                if !blocks.is_empty() {
                    info!(
                        "📤 Sending {} blocks to {}",
                        blocks.len(),
                        request.requester
                    );

                    // Send BlockRangeResponse
                    let response_msg = P2PMessage::new(
                        MessageType::BlockRangeResponse { blocks },
                        local_addr_for_responses,
                    );

                    // Send to requester specifically
                    peer_mgr_for_responses
                        .send_to_peer(&request.requester, response_msg)
                        .await
                        .unwrap_or_else(|e| warn!("Failed to send block response: {}", e));
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
            while let Some(request) = snapshot_request_rx.recv().await {
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
                        // P10-CORE-03 FIX: Use paginated export to avoid loading
                        // the entire column family into memory.
                        let chunk_sz = chunk_size.max(1) as u64;
                        let offset = (chunk_index as u64) * chunk_sz;

                        let page = match category.as_str() {
                            "accounts" => store.export_accounts_iter(offset, chunk_sz),
                            "contract_storage" => {
                                store.export_contract_storage_iter(offset, chunk_sz)
                            }
                            "programs" => store.export_programs_iter(offset, chunk_sz),
                            _ => Ok(moltchain_core::state::KvPage {
                                entries: Vec::new(),
                                total: 0,
                            }),
                        }
                        .unwrap_or_else(|_| {
                            moltchain_core::state::KvPage {
                                entries: Vec::new(),
                                total: 0,
                            }
                        });

                        let total_entries = page.total;
                        let total_chunks = total_entries.div_ceil(chunk_sz).max(1);
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
                            // Note: The actual state snapshot request would be sent via the
                            // peer manager, but we track it here for the sync flow.
                            // In practice, the join flow handles sending requests.
                        }
                    } else {
                        info!("📋 Peer {} has no checkpoint available", response.requester);
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
                                        // Update existing: prefer higher stats
                                        if remote_val.blocks_proposed > local_val.blocks_proposed {
                                            local_val.blocks_proposed = remote_val.blocks_proposed;
                                        }
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
    // Use safe arithmetic: offset = p2p_port % 1000 to avoid underflow/overflow
    // Port auto-derivation matches run-validator.sh exactly:
    //   V1 (p2p 8000): rpc=8899, ws=8900
    //   V2 (p2p 8001): rpc=8901, ws=8902
    //   V3 (p2p 8002): rpc=8903, ws=8904
    // Formula: offset = p2p_port - base_p2p, rpc = 8899 + 2*offset, ws = 8900 + 2*offset
    let rpc_port = args
        .iter()
        .position(|arg| arg == "--rpc-port")
        .and_then(|pos| args.get(pos + 1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| {
            let base_p2p = if p2p_port >= 9000 { 9000u16 } else { 8000u16 };
            let base_rpc = if p2p_port >= 9000 { 9899u16 } else { 8899u16 };
            let offset = p2p_port.saturating_sub(base_p2p);
            base_rpc.saturating_add(offset.saturating_mul(2))
        });

    let ws_port = args
        .iter()
        .position(|arg| arg == "--ws-port")
        .and_then(|pos| args.get(pos + 1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| {
            let base_p2p = if p2p_port >= 9000 { 9000u16 } else { 8000u16 };
            let base_ws = if p2p_port >= 9000 { 9900u16 } else { 8900u16 };
            let offset = p2p_port.saturating_sub(base_p2p);
            base_ws.saturating_add(offset.saturating_mul(2))
        });

    // Parse --admin-token from CLI or MOLTCHAIN_ADMIN_TOKEN env var
    let admin_token: Option<String> = args
        .iter()
        .position(|arg| arg == "--admin-token")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.to_string())
        .or_else(|| env::var("MOLTCHAIN_ADMIN_TOKEN").ok())
        .filter(|t| !t.is_empty());
    if admin_token.is_some() {
        info!("🔒 Admin token configured for state-mutating RPC endpoints");
    }

    let state_for_rpc = state.clone();
    let state_for_ws = state.clone();
    let stake_pool_for_rpc = Some(stake_pool.clone());
    let chain_id_for_rpc = genesis_config.chain_id.clone();
    let network_id_for_rpc = genesis_config.chain_id.clone();

    // Create transaction submission channel for RPC -> mempool (bounded: backpressure returns HTTP 503)
    let (rpc_tx_sender, mut rpc_tx_receiver) = mpsc::channel::<Transaction>(1_000);

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
    if let Ok(Some(gpk)) = state.get_genesis_pubkey() {
        let state_for_oracle = state.clone();
        spawn_oracle_price_feeder(state_for_oracle, gpk);
    } else {
        warn!("⚠️  Oracle price feeder: no genesis pubkey found, skipping");
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
            let mut interval = time::interval(Duration::from_secs(30));
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
                let mut sign_message = Vec::with_capacity(80);
                sign_message.extend_from_slice(&validator_pubkey_for_announce.0);
                sign_message.extend_from_slice(&validator_stake.to_le_bytes());
                sign_message.extend_from_slice(&current_slot.to_le_bytes());
                sign_message.extend_from_slice(&machine_fingerprint_for_announce);
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

    // Periodic mempool cleanup
    let mempool_for_cleanup = mempool.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let mut pool = mempool_for_cleanup.lock().await;
            pool.cleanup_expired();
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

    // Periodic downtime detection and slashing (check every 60s)
    let validator_set_for_downtime = validator_set.clone();
    let slashing_for_downtime = slashing_tracker.clone();
    let state_for_downtime = state.clone();
    let validator_pubkey_for_downtime = validator_pubkey;
    let peer_mgr_for_downtime_slash = p2p_peer_manager.clone();
    let local_addr_for_downtime = p2p_config.listen_addr;
    let genesis_time_for_downtime = genesis_time_secs;
    let slot_duration_for_downtime = slot_duration_ms;

    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let current_slot = state_for_downtime.get_last_slot().unwrap_or(0);

            // Check all validators for downtime (offline for 100+ slots)
            let vs = validator_set_for_downtime.read().await;

            // FIX: Detect chain-wide stall — if ALL validators have similar
            // missed_slots (within 200 of each other), the entire chain was
            // stalled. Do NOT slash for downtime during a full chain stall.
            // Only slash when individual validators go offline while the
            // chain is progressing.
            let all_missed: Vec<u64> = vs.validators().iter().map(|v| {
                current_slot.saturating_sub(v.last_active_slot)
            }).collect();
            let min_missed = all_missed.iter().copied().min().unwrap_or(0);
            let max_missed = all_missed.iter().copied().max().unwrap_or(0);
            // If every validator missed within 200 slots of each other, it's
            // a chain-wide stall, not individual downtime.
            let is_chain_stall = max_missed >= 100 && (max_missed.saturating_sub(min_missed)) < 200;
            if is_chain_stall {
                debug!("⏸️  Chain-wide stall detected (all validators missed {}-{} slots) — skipping downtime slashing",
                    min_missed, max_missed);
                drop(vs);
                continue;
            }

            for validator_info in vs.validators() {
                let missed_slots = current_slot.saturating_sub(validator_info.last_active_slot);

                // Grace period: skip newly-joined validators (200 slots ≈ 80s)
                let slots_since_join = current_slot.saturating_sub(validator_info.joined_slot);
                if slots_since_join < 200 {
                    continue;
                }

                // Slash if offline for 100+ slots (~40 seconds at 400ms/slot)
                if missed_slots >= 100 && validator_info.pubkey != validator_pubkey_for_downtime {
                    info!(
                        "⚔️  Validator {} offline for {} slots",
                        validator_info.pubkey.to_base58(),
                        missed_slots
                    );

                    let evidence = SlashingEvidence::new(
                        SlashingOffense::Downtime {
                            last_active_slot: validator_info.last_active_slot,
                            current_slot,
                            missed_slots,
                        },
                        validator_info.pubkey,
                        current_slot,
                        validator_pubkey_for_downtime,
                        moltchain_core::block::derive_slot_timestamp(
                            genesis_time_for_downtime,
                            current_slot,
                            slot_duration_for_downtime,
                        ),
                    );

                    let mut slasher = slashing_for_downtime.lock().await;
                    if slasher.add_evidence(evidence.clone()) {
                        info!(
                            "⚔️  Downtime evidence recorded for {}",
                            validator_info.pubkey.to_base58()
                        );

                        // Broadcast evidence
                        if let Some(ref peer_mgr) = peer_mgr_for_downtime_slash {
                            let evidence_msg = P2PMessage::new(
                                MessageType::SlashingEvidence(evidence),
                                local_addr_for_downtime,
                            );
                            peer_mgr.broadcast(evidence_msg).await;
                        }
                    }
                    drop(slasher);
                }
            }
            drop(vs);

            // Cleanup old evidence
            let mut slasher = slashing_for_downtime.lock().await;
            slasher.prune_old_evidence(current_slot, 1000);
        }
    });

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
    let watchdog_timeout_secs = args
        .iter()
        .position(|a| a == "--watchdog-timeout")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_WATCHDOG_TIMEOUT_SECS);

    let last_block_time_for_watchdog = last_block_time.clone();
    let state_for_watchdog = state.clone();
    tokio::spawn(async move {
        // Give the validator time to start up and sync before monitoring
        // Reduced startup grace from watchdog_timeout.max(60) to 30s for faster detection
        time::sleep(Duration::from_secs(30)).await;
        let mut interval = time::interval(Duration::from_secs(5)); // Check every 5s (was 15s)
        let mut stale_checks: u32 = 0;
        let threshold = (watchdog_timeout_secs / 5).max(3) as u32; // 3 checks minimum
        let mut last_known_slot: u64 = 0;
        loop {
            interval.tick().await;
            let elapsed = last_block_time_for_watchdog.lock().await.elapsed();
            let current_slot = state_for_watchdog.get_last_slot().unwrap_or(0);

            if elapsed > Duration::from_secs(watchdog_timeout_secs)
                && current_slot == last_known_slot
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
                    // AUDIT-FIX 2.12: Allow pending async I/O and Drop handlers
                    // a brief window to flush before hard exit. process::exit()
                    // skips destructors, so we give a small grace period.
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    std::process::exit(EXIT_CODE_RESTART);
                }
            } else {
                if stale_checks > 0 {
                    info!("🐺 Watchdog: activity resumed (slot {})", current_slot);
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

    // Adaptive heartbeat: Track last time we had activity (transaction block or heartbeat)
    let mut last_activity_time = std::time::Instant::now();
    let mut slot_start = std::time::Instant::now();
    let mut last_attempted_slot: u64 = 0;

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

        // Reset view timer when chain tip advances (new slot to fill)
        if slot != last_attempted_slot {
            slot_start = std::time::Instant::now();
            last_attempted_slot = slot;
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
        // FIX: Decay highest_seen toward our tip if no new blocks have arrived
        // from the network for 10 seconds. This prevents a permanent stall when
        // no peer can serve the missing blocks (e.g., all validators stalled).
        {
            sync_manager.decay_highest_seen(tip_slot, 10).await;
            let network_highest = sync_manager.get_highest_seen().await;
            if network_highest > tip_slot + 2 {
                continue;
            }
        }

        // Check if we already have a block for this slot (received from P2P)
        if let Ok(Some(_existing_block)) = state.get_block_by_slot(slot) {
            // Already have a block for this slot — tip will advance next iteration
            continue;
        }

        // Apply slashing penalties if any validators should be slashed.
        // PERF-FIX 5: Only run the slashing sweep every 100 slots (~40s) instead of
        // every slot tick (~400ms). This reduces lock contention on validator_set,
        // stake_pool, and slashing_tracker by ~99% while keeping offense-to-slash
        // latency well within acceptable bounds.
        if slot % 100 == 0 {
            let mut slasher = slashing_tracker.lock().await;
            // Lock ordering: validator_set before stake_pool (matches global convention
            // used by announcement handler, vote handlers, leader election, etc.)
            let mut vs = validator_set.write().await;
            let mut pool = stake_pool.write().await;

            // Cleanup expired suspensions and repayment boosts
            slasher.cleanup_expired(slot);

            for validator_info in vs.validators_mut() {
                // Grace period: don't slash validators that recently joined (200 slots ≈ 80s).
                // Prevents false-positive slashing during initial sync/handshake.
                let slots_since_join = slot.saturating_sub(validator_info.joined_slot);
                if slots_since_join < 200 {
                    continue;
                }

                // Skip if validator is temporarily suspended (Tier 2 penalty)
                if slasher.is_suspended(&validator_info.pubkey, slot) {
                    continue;
                }

                // Check for downtime evidence to apply tiered system
                let has_downtime = slasher.get_evidence(&validator_info.pubkey)
                    .map(|ev| ev.iter().any(|e| matches!(e.offense, SlashingOffense::Downtime { .. })))
                    .unwrap_or(false);

                let has_non_downtime = slasher.get_evidence(&validator_info.pubkey)
                    .map(|ev| ev.iter().any(|e| e.severity() >= 70 && !matches!(e.offense, SlashingOffense::Downtime { .. })))
                    .unwrap_or(false);

                // For downtime, record the offense to advance the tier
                // AUDIT-FIX HIGH-4: Only record a new offense when fresh evidence
                // has actually been added. Without this gate, the sweep would call
                // record_downtime_offense every 100 slots on the SAME evidence,
                // escalating Tier 1 → Tier 2 in just 40 seconds (1 sweep cycle).
                if has_downtime && !slasher.is_slashed(&validator_info.pubkey)
                    && slasher.has_new_downtime_evidence(&validator_info.pubkey)
                {
                    let tier = slasher.record_downtime_offense(&validator_info.pubkey, slot);

                    match tier {
                        1 => {
                            // Tier 1: Reputation penalty only (warning)
                            let reputation_penalty = slasher.calculate_penalty(&validator_info.pubkey);
                            let old_reputation = validator_info.reputation;
                            validator_info.reputation = validator_info
                                .reputation
                                .saturating_sub(reputation_penalty)
                                .max(50);
                            warn!(
                                "⚠️  DOWNTIME WARNING (Tier 1) {} | Rep: {} -> {} | No stake slashed",
                                validator_info.pubkey.to_base58(),
                                old_reputation,
                                validator_info.reputation
                            );
                        }
                        2 => {
                            // Tier 2: Small slash (0.5%) + suspension + penalty repayment boost
                            let slashed_amount = slasher.apply_economic_slashing_with_params(
                                &validator_info.pubkey,
                                &mut pool,
                                &genesis_config.consensus,
                                slot,
                            );

                            // Apply suspension
                            slasher.suspend_validator(&validator_info.pubkey, slot);

                            // Apply penalty repayment boost (90% to debt for ~1 day)
                            slasher.apply_penalty_repayment_boost(&validator_info.pubkey, slot);

                            // Set penalty boost on StakeInfo so claim_rewards auto-detects it
                            if let Some(stake_info) = pool.get_stake_mut(&validator_info.pubkey) {
                                stake_info.penalty_boost_until = slot + moltchain_core::consensus::PENALTY_REPAYMENT_BOOST_SLOTS;
                            }

                            let reputation_penalty = slasher.calculate_penalty(&validator_info.pubkey);
                            let old_reputation = validator_info.reputation;
                            validator_info.reputation = validator_info
                                .reputation
                                .saturating_sub(reputation_penalty)
                                .max(50);

                            if slashed_amount > 0 {
                                warn!(
                                    "⚔️  DOWNTIME SLASH (Tier 2) {} | Stake burned: {:.4} MOLT | Rep: {} -> {} | Suspended {} slots | Repayment boost active",
                                    validator_info.pubkey.to_base58(),
                                    slashed_amount as f64 / 1_000_000_000.0,
                                    old_reputation,
                                    validator_info.reputation,
                                    moltchain_core::consensus::DOWNTIME_SUSPENSION_SLOTS
                                );

                                // Persist slashing to on-chain account
                                if let Ok(Some(mut acct)) = state.get_account(&validator_info.pubkey) {
                                    let debit = slashed_amount.min(acct.staked);
                                    acct.staked = acct.staked.saturating_sub(debit);
                                    acct.shells = acct.shells.saturating_sub(debit);
                                    if let Err(e) = state.put_account(&validator_info.pubkey, &acct) {
                                        error!("Failed to persist slashed account: {}", e);
                                    }
                                }
                            }
                        }
                        _ => {
                            // Tier 3+: Full graduated slashing
                            let slashed_amount = slasher.apply_economic_slashing_with_params(
                                &validator_info.pubkey,
                                &mut pool,
                                &genesis_config.consensus,
                                slot,
                            );

                            let reputation_penalty = slasher.calculate_penalty(&validator_info.pubkey);
                            let old_reputation = validator_info.reputation;
                            validator_info.reputation = validator_info
                                .reputation
                                .saturating_sub(reputation_penalty)
                                .max(50);

                            if slashed_amount > 0 {
                                warn!(
                                    "⚔️💰 DOWNTIME SLASH (Tier 3) {} | Stake burned: {} MOLT | Rep: {} -> {}",
                                    validator_info.pubkey.to_base58(),
                                    slashed_amount / 1_000_000_000,
                                    old_reputation,
                                    validator_info.reputation
                                );

                                if let Ok(Some(mut acct)) = state.get_account(&validator_info.pubkey) {
                                    let debit = slashed_amount.min(acct.staked);
                                    acct.staked = acct.staked.saturating_sub(debit);
                                    acct.shells = acct.shells.saturating_sub(debit);
                                    if let Err(e) = state.put_account(&validator_info.pubkey, &acct) {
                                        error!("Failed to persist slashed account: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }

                // For non-downtime severe offenses, apply immediately (no tiering)
                if has_non_downtime && !slasher.is_slashed(&validator_info.pubkey) {
                    let slashed_amount = slasher.apply_economic_slashing_with_params(
                        &validator_info.pubkey,
                        &mut pool,
                        &genesis_config.consensus,
                        slot,
                    );

                    let reputation_penalty = slasher.calculate_penalty(&validator_info.pubkey);
                    let old_reputation = validator_info.reputation;
                    validator_info.reputation = validator_info
                        .reputation
                        .saturating_sub(reputation_penalty)
                        .max(50);

                    if slashed_amount > 0 {
                        warn!(
                            "⚔️💰 SLASHED {} | Stake burned: {} MOLT | Reputation: {} -> {}",
                            validator_info.pubkey.to_base58(),
                            slashed_amount / 1_000_000_000,
                            old_reputation,
                            validator_info.reputation
                        );

                        if let Ok(Some(mut acct)) = state.get_account(&validator_info.pubkey) {
                            let debit = slashed_amount.min(acct.staked);
                            acct.staked = acct.staked.saturating_sub(debit);
                            acct.shells = acct.shells.saturating_sub(debit);
                            if let Err(e) = state.put_account(&validator_info.pubkey, &acct) {
                                error!("Failed to persist slashed account: {}", e);
                            }
                        }
                    }
                }
            }

            // AUDIT-FIX CRITICAL-2: Clear slashed flag at end of sweep.
            // Without this, once a validator is marked slashed it is permanently
            // immune to all future slashing (is_slashed check at top of sweep skips
            // them). We clear the flag so the sweep can re-evaluate next cycle.
            // Permanently banned validators (collusion) are NOT cleared.
            {
                let all_slashed: Vec<_> = slasher.slashed_validators().collect();
                for pk in all_slashed {
                    if !slasher.is_permanently_banned(&pk) {
                        slasher.clear_slashed(&pk);
                    }
                }
            }

            // Clean up ghost validators (fully slashed, inactive for 10K+ slots)
            let removed = pool.remove_ghost_validators(slot, 10_000);
            if !removed.is_empty() {
                for pk in &removed {
                    vs.remove_validator(pk);
                    info!("🗑️  Removed ghost validator {}", pk.to_base58());
                }
            }

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
            let deadlock_timeout_ms = view_change_interval_ms * 20; // ~24 seconds
            if view >= 15 && slot_start.elapsed().as_millis() as u64 > deadlock_timeout_ms {
                info!(
                    "⚠️  Slot {} — all views exhausted with no block after {}ms, producing as deadlock breaker",
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
        let is_heartbeat_time = last_activity_time.elapsed() >= Duration::from_secs(5);

        // Peek at mempool to determine if this would be a heartbeat or tx block
        let has_pending = {
            let pool = mempool.lock().await;
            pool.size() > 0
        };

        if !has_pending {
            // No transactions — this will be a heartbeat block.
            // ALL heartbeats respect the 5-second timer, even primary leaders.
            // Only exception: deadlock breaker must produce to unstick a frozen chain.
            if !is_heartbeat_time && !is_deadlock_breaker {
                continue;
            }
        } else if !should_produce && !is_deadlock_breaker {
            // Has transactions but we were not selected as leader.
            // This shouldn't normally happen (leader check is above), but guard anyway.
            continue;
        }

        // Update parent_hash from actual latest block (in case chain was synced from P2P)
        if tip_slot > 0 {
            if let Ok(Some(latest_block)) = state.get_block_by_slot(tip_slot) {
                parent_hash = latest_block.hash();
            }
        } else {
            // We have genesis, use it as parent
            if let Ok(Some(genesis_block)) = state.get_block_by_slot(0) {
                parent_hash = genesis_block.hash();
            }
        }

        // Collect pending transactions from mempool
        let pending_transactions = {
            let mut pool = mempool.lock().await;
            pool.get_top_transactions(500) // PERF-FIX 8: 100 → 500 TXs per block for parallel throughput
        };

        // Process transactions in parallel where possible (FIX-2: rayon)
        // Non-conflicting TXs (disjoint account sets) run on separate threads.
        let processed_hashes: Vec<Hash> = pending_transactions.iter().map(|tx| tx.hash()).collect();
        let results =
            processor.process_transactions_parallel(&pending_transactions, &validator_pubkey);

        let mut transactions: Vec<Transaction> = Vec::new();
        for (tx, result) in pending_transactions.into_iter().zip(results.into_iter()) {
            if result.success {
                transactions.push(tx);
            } else {
                warn!(
                    "⚠️  Dropping transaction {}: {}",
                    tx.signature().to_hex(),
                    result.error.unwrap_or_else(|| "Unknown error".to_string())
                );
            }
        }

        let has_user_transactions = !transactions.is_empty();

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
            if already_received || already_stored {
                debug!(
                    "⏭️  Slot {} already has a network block, skipping production",
                    slot
                );
                continue;
            }
        }

        let is_heartbeat = !has_user_transactions;

        // Update activity tracking - reset timer after producing block
        last_activity_time = std::time::Instant::now();

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

        // Create block
        // Use wall-clock timestamp so explorer display and cross-validator
        // sync work correctly regardless of heartbeat cadence.  The receiving
        // side validates within a generous wall-clock window (see block rx).
        let state_root = state.compute_state_root();
        let wall_clock_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut block = Block::new_with_timestamp(
            slot,
            parent_hash,
            state_root,
            validator_pubkey.0,
            transactions.clone(),
            wall_clock_timestamp,
        );

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
        // Other validators need the block ASAP to advance their tip and produce
        // the next block. Everything else (self-vote, mempool cleanup, effects)
        // can happen after the broadcast.  Fire-and-forget via tokio::spawn so
        // we don't block on QUIC writes.
        if let Some(ref peer_mgr) = p2p_peer_manager {
            let block_msg = moltchain_p2p::P2PMessage::new(
                moltchain_p2p::MessageType::Block(block.clone()),
                p2p_config.listen_addr,
            );
            let pm_block = peer_mgr.clone();
            tokio::spawn(async move {
                pm_block.broadcast(block_msg).await;
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
            run_analytics_bridge_from_state(&state, slot);

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
        let vote = {
            let mut vote_message = Vec::new();
            vote_message.extend_from_slice(&slot.to_le_bytes());
            vote_message.extend_from_slice(&block_hash.0);
            let signature = validator_keypair.sign(&vote_message);

            let vote = Vote::new(slot, block_hash, validator_pubkey, signature);

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
            vote
        };

        // Remove included transactions from mempool (PERF: bulk removal, single heap rebuild)
        {
            let mut pool = mempool.lock().await;
            pool.remove_transactions_bulk(&processed_hashes);
        }

        // Broadcast self-vote to network (fire-and-forget)
        if let Some(ref peer_mgr) = p2p_peer_manager {
            let vote_msg = P2PMessage::new(MessageType::Vote(vote), p2p_config.listen_addr);
            let pm_vote = peer_mgr.clone();
            tokio::spawn(async move {
                pm_vote.broadcast(vote_msg).await;
            });
            info!("📡 Broadcasted block {} + vote to network", slot);
        }

        apply_block_effects(
            &state,
            &validator_set,
            &stake_pool,
            &vote_aggregator,
            &block,
            rewards_applied,
        )
        .await;
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

        // Register MARGIN program
        let margin_pk = Pubkey([50u8; 32]);
        state
            .register_symbol(
                "MARGIN",
                moltchain_core::state::SymbolRegistryEntry {
                    symbol: "MARGIN".to_string(),
                    program: margin_pk,
                    owner: Pubkey([0u8; 32]),
                    name: None,
                    template: None,
                    metadata: None,
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

        state
            .put_contract_storage(&margin_pk, b"margin_pos_1", &pos_data)
            .unwrap();
        state
            .put_contract_storage(&margin_pk, b"position_count", &1u64.to_le_bytes())
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
            .get_contract_storage(&margin_pk, b"margin_pos_1")
            .unwrap()
            .unwrap();
        assert_eq!(closed_data[49], 1, "position should be closed");

        // PnL: (200 - 100) * 1B / 1B = 100 profit
        // return_amount = margin(500) + capped_profit(min(100, 1000)) = 600
        // insurance_fund should be debited by 100: 1000 - 100 = 900
        let insurance_after = state.get_program_storage_u64("MARGIN", b"mrg_insurance");
        assert_eq!(
            insurance_after, 900,
            "insurance fund should be debited by profit"
        );

        // Verify PnL tracking
        let pnl_profit = state.get_program_storage_u64("MARGIN", b"mrg_pnl_profit");
        assert_eq!(pnl_profit, 100, "cumulative profit should be tracked");

        // Verify user balance credited (with saturating_add, P9-VAL-03)
        let balance_key = format!("balance_{}", hex::encode(trader));
        let user_bal = state.get_program_storage_u64("MOLTCOIN", balance_key.as_bytes());
        assert_eq!(user_bal, 600, "user should receive margin + capped profit");
    }
}
