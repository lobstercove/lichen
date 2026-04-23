// Lichen RPC Server
// JSON-RPC API for querying blockchain state
//
// ═══════════════════════════════════════════════════════════════════════════════
// MODULE LAYOUT (logical sections within this file)
// ═══════════════════════════════════════════════════════════════════════════════
//   mod ws;                        — WebSocket subscriptions (ws.rs)
//
//   SHARED STATE & TYPES           — RpcRequest, RpcResponse, RpcError, RpcState
//   ADMIN AUTH                     — verify_admin_auth, constant_time_eq
//   RATE LIMITING & MIDDLEWARE     — RateLimiter, rate_limit_middleware
//   UTILITY FUNCTIONS              — count_executable_accounts, parsing helpers
//   SOLANA SERIALIZATION HELPERS   — solana_context, solana_*_json, encoding
//   SERVER STARTUP & ROUTER        — start_rpc_server, build_rpc_router
//   RPC DISPATCH                   — handle_rpc, handle_solana_rpc, handle_evm_rpc
//   NATIVE LICN RPC METHODS        — getBalance, getAccount, getBlock, getSlot, …
//   CORE TRANSACTION METHODS       — getTransaction, getTransactionsByAddress, send
//   SOLANA-COMPATIBLE ENDPOINTS    — Solana JSON-RPC compatibility layer
//   NETWORK ENDPOINTS              — getPeers, getNetworkInfo, getClusterInfo
//   VALIDATOR ENDPOINTS            — getValidatorInfo, getValidatorPerformance
//   STAKING ENDPOINTS              — stake, unstake, getStakingStatus
//   ACCOUNT ENDPOINTS              — getAccountInfo, getTransactionHistory
//   CONTRACT ENDPOINTS             — getContractInfo, getContractLogs, ABI
//   PROGRAM ENDPOINTS              — getProgram, getProgramStats, getProgramCalls
//   NFT ENDPOINTS                  — getCollection, getNFT, getNFTsByOwner
//   MARKETPLACE ENDPOINTS          — getMarketListings, getMarketSales
//   ETHEREUM JSON-RPC LAYER        — eth_getBalance, eth_sendRawTransaction, …
//   MOSSSTAKE QUERY ENDPOINTS      — getStakingPosition, getMossStakePoolInfo
//   TOKEN ENDPOINTS                — getTokenBalance, getTokenHolders, getTokenTransfers
//   DEX REST API                    — /api/v1/* endpoints (dex.rs)
//   DEX WEBSOCKET FEEDS             — orderbook, trades, ticker, candles (dex_ws.rs)
// ═══════════════════════════════════════════════════════════════════════════════

pub mod dex;
pub mod dex_ws;
pub mod launchpad;
pub mod prediction;
pub mod shielded;
pub mod ws;

use alloy_primitives::{Address, Bytes, U256};
use axum::body::Bytes as AxumBytes;
use axum::http::{HeaderMap, HeaderValue, Method};
use axum::{
    extract::ConnectInfo,
    extract::State,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use chrono::{SecondsFormat, Utc};
use lichen_core::account::Keypair as TreasuryKeypair;
use lichen_core::contract::{ContractAccount, ContractEvent, ContractRuntime};
use lichen_core::keypair_file::KeypairFile;
use lichen_core::nft::{decode_collection_state, decode_token_state, NftActivityKind};
use lichen_core::{
    compute_units_for_tx, decode_evm_transaction, simulate_evm_call, spores_to_u256,
    FinalityTracker, Hash, Instruction, MarketActivityKind, Message, PqSignature, Pubkey,
    StakePool, StateStore, SymbolRegistryEntry, Transaction, TxProcessor, ValidatorSet,
    CONTRACT_PROGRAM_ID, EVM_PROGRAM_ID, MAX_CONTRACT_CODE, SYSTEM_PROGRAM_ID,
};
use lru::LruCache;

// RPC-H05: keep tx listing endpoints under ~600 DB reads/page by bounding page size.
// Each tx can consume up to ~4 reads in the worst common path:
// - 1 index entry read (CF_ACCOUNT_TXS or CF_TX_BY_SLOT)
// - 1 tx lookup (CF_TRANSACTIONS)
// - 2 reads for slot->block timestamp (CF_SLOTS + CF_BLOCKS)
const TX_LIST_MAX_DB_READS: usize = 600;
const TX_LIST_DB_READS_PER_TX_ESTIMATE: usize = 4;
const TX_LIST_MAX_LIMIT: usize = TX_LIST_MAX_DB_READS / TX_LIST_DB_READS_PER_TX_ESTIMATE;
const MARKET_LISTINGS_UNFILTERED_MAX_LIMIT: usize = 200;
const PROGRAM_LIST_CACHE_TTL_MS: u128 = 1000;
const PROGRAM_LIST_CACHE_MAX_ENTRIES: usize = 512;
const SERVICE_FLEET_CACHE_TTL_MS: u128 = 10_000;
const SIGNED_METADATA_MANIFEST_SCHEMA_VERSION: u64 = 1;
const LIVE_SIGNED_METADATA_SOURCE_RPC: &str = "live-rpc";
const SOLANA_SPL_TOKEN_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const SOLANA_TOKEN_ACCOUNT_SPACE: usize = 165;
const SOLANA_TOKEN_ACCOUNT_RENT_EXEMPT_LAMPORTS: u64 = 2_039_280;

/// P9-RPC-02: Maximum size for bincode transaction deserialization.
/// Prevents OOM/DoS from maliciously large payloads.  4 MiB matches
/// the contract-deploy datasize limit enforced by `validate_structure()`.
const MAX_TX_BINCODE_SIZE: u64 = 4 * 1024 * 1024;

/// Decode raw bytes into a Transaction using the wire-format envelope (M-6).
/// Supports V1 envelope, raw bincode, JSON (serde), and wallet JSON format.
pub(crate) fn decode_transaction_bytes(bytes: &[u8]) -> Result<Transaction, RpcError> {
    Transaction::from_wire(bytes, MAX_TX_BINCODE_SIZE)
        .or_else(|_| {
            // Fall back to wallet-specific JSON (array-of-numbers signatures, multi-key formats)
            parse_json_transaction(bytes).map_err(|e| e.message)
        })
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid transaction: {}", e),
        })
}
use dashmap::DashMap;
use lichen_core::consensus::{compute_block_reward, ValidatorInfo, GENESIS_SUPPLY_SPORES};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, RwLock};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{info, warn};

// Re-export WebSocket types
pub use ws::{start_ws_server, Event as WsEvent};

pub(crate) fn pq_signature_json(signature: &PqSignature) -> serde_json::Value {
    serde_json::to_value(signature).unwrap_or(serde_json::Value::Null)
}

pub(crate) fn pq_signature_option_json(signature: Option<&PqSignature>) -> serde_json::Value {
    signature
        .map(pq_signature_json)
        .unwrap_or(serde_json::Value::Null)
}

pub(crate) fn pq_signature_is_zero(signature: &PqSignature) -> bool {
    signature.sig.iter().all(|&byte| byte == 0)
}

#[derive(Debug, Clone)]
pub(crate) struct GovernanceEventRecord {
    pub proposal_id: u64,
    pub event_kind: String,
    pub action: String,
    pub authority: Pubkey,
    pub proposer: Pubkey,
    pub actor: Pubkey,
    pub approvals: u64,
    pub threshold: u8,
    pub execute_after_epoch: u64,
    pub executed: bool,
    pub cancelled: bool,
    pub metadata: String,
    pub target_contract: Option<Pubkey>,
    pub target_function: Option<String>,
    pub call_args_len: Option<u64>,
    pub call_value_spores: Option<u64>,
    pub slot: u64,
}

fn governance_event_kind(name: &str) -> Option<&'static str> {
    match name {
        "GovernanceProposalCreated" => Some("created"),
        "GovernanceProposalApproved" => Some("approved"),
        "GovernanceProposalExecuted" => Some("executed"),
        "GovernanceProposalCancelled" => Some("cancelled"),
        _ => None,
    }
}

pub(crate) fn parse_governance_event(event: &ContractEvent) -> Option<GovernanceEventRecord> {
    if event.program != SYSTEM_PROGRAM_ID {
        return None;
    }

    let event_kind = governance_event_kind(&event.name)?.to_string();
    let data = &event.data;

    Some(GovernanceEventRecord {
        proposal_id: data.get("proposal_id")?.parse().ok()?,
        event_kind,
        action: data.get("action")?.clone(),
        authority: Pubkey::from_base58(data.get("authority")?).ok()?,
        proposer: Pubkey::from_base58(data.get("proposer")?).ok()?,
        actor: Pubkey::from_base58(data.get("actor")?).ok()?,
        approvals: data.get("approvals")?.parse().ok()?,
        threshold: data.get("threshold")?.parse().ok()?,
        execute_after_epoch: data.get("execute_after_epoch")?.parse().ok()?,
        executed: data.get("executed")?.parse().ok()?,
        cancelled: data.get("cancelled")?.parse().ok()?,
        metadata: data.get("metadata").cloned().unwrap_or_default(),
        target_contract: data
            .get("target_contract")
            .and_then(|value| Pubkey::from_base58(value).ok()),
        target_function: data.get("target_function").cloned(),
        call_args_len: data
            .get("call_args_len")
            .and_then(|value| value.parse().ok()),
        call_value_spores: data
            .get("call_value_spores")
            .and_then(|value| value.parse().ok()),
        slot: event.slot,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct IncidentComponentStatus {
    status: String,
    #[serde(default)]
    message: String,
}

impl IncidentComponentStatus {
    fn new(status: &str, message: &str) -> Self {
        Self {
            status: status.to_string(),
            message: message.to_string(),
        }
    }

    fn normalize(&mut self, fallback: &Self) {
        if self.status.trim().is_empty() {
            self.status = fallback.status.clone();
        }
        if self.message.trim().is_empty() {
            self.message = fallback.message.clone();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct IncidentEnforcementTarget {
    #[serde(default)]
    id: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    pause_function: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct IncidentEnforcementRecord {
    #[serde(default)]
    mode: String,
    #[serde(default)]
    contract_targets: Vec<IncidentEnforcementTarget>,
}

impl IncidentEnforcementRecord {
    fn is_empty(&self) -> bool {
        self.mode.trim().is_empty() && self.contract_targets.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct IncidentStatusRecord {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    source: String,
    #[serde(default)]
    network: String,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    active_since: Option<String>,
    #[serde(default)]
    mode: String,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    banner_enabled: bool,
    #[serde(default)]
    headline: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    customer_message: String,
    #[serde(default)]
    status_page_url: Option<String>,
    #[serde(default)]
    actions: Vec<String>,
    #[serde(default, skip_serializing_if = "IncidentEnforcementRecord::is_empty")]
    enforcement: IncidentEnforcementRecord,
    #[serde(default)]
    components: BTreeMap<String, IncidentComponentStatus>,
}

impl IncidentStatusRecord {
    fn default_components() -> BTreeMap<String, IncidentComponentStatus> {
        BTreeMap::from([
            (
                "bridge".to_string(),
                IncidentComponentStatus::new(
                    "operational",
                    "Bridge deposits and mints are operating normally.",
                ),
            ),
            (
                "contracts".to_string(),
                IncidentComponentStatus::new(
                    "operational",
                    "No contract circuit breakers are active.",
                ),
            ),
            (
                "deposits".to_string(),
                IncidentComponentStatus::new(
                    "operational",
                    "Deposits and withdrawals are operating normally.",
                ),
            ),
            (
                "wallet".to_string(),
                IncidentComponentStatus::new(
                    "operational",
                    "Local wallet access remains available.",
                ),
            ),
        ])
    }

    fn normal(network_id: &str) -> Self {
        Self {
            schema_version: 1,
            source: "default".to_string(),
            network: network_id.to_string(),
            updated_at: None,
            active_since: None,
            mode: "normal".to_string(),
            severity: "info".to_string(),
            banner_enabled: false,
            headline: "All systems operational".to_string(),
            summary: "No incident response mode is active.".to_string(),
            customer_message: "Deposits, bridge access, and wallet usage are operating normally."
                .to_string(),
            status_page_url: None,
            actions: Vec::new(),
            enforcement: IncidentEnforcementRecord::default(),
            components: Self::default_components(),
        }
    }

    fn status_feed_error(network_id: &str) -> Self {
        let mut status = Self::normal(network_id);
        status.source = "error".to_string();
        status.mode = "status_feed_error".to_string();
        status.severity = "warning".to_string();
        status.banner_enabled = true;
        status.headline = "Public status feed unavailable".to_string();
        status.summary =
            "The incident-response manifest could not be loaded from the configured RPC source."
                .to_string();
        status.customer_message = "If you are waiting on an incident update, use official Lichen operator channels before making high-value deposit, bridge, or treasury decisions.".to_string();
        status.actions = vec![
            "Delay high-value bridge and deposit actions until the public status feed is restored."
                .to_string(),
        ];
        for component in status.components.values_mut() {
            component.status = "unknown".to_string();
            component.message =
                "Verify the current service state with operators before relying on this surface."
                    .to_string();
        }
        status
    }

    fn normalize(mut self, network_id: &str, source: &str) -> Self {
        let defaults = Self::normal(network_id);

        if self.schema_version == 0 {
            self.schema_version = defaults.schema_version;
        }
        if self.source.trim().is_empty() {
            self.source = source.to_string();
        }
        if self.network.trim().is_empty() {
            self.network = defaults.network;
        }
        if self.mode.trim().is_empty() {
            self.mode = defaults.mode;
        }
        if self.severity.trim().is_empty() {
            self.severity = defaults.severity;
        }
        if self.headline.trim().is_empty() {
            self.headline = defaults.headline;
        }
        if self.summary.trim().is_empty() {
            self.summary = defaults.summary;
        }
        if self.customer_message.trim().is_empty() {
            self.customer_message = defaults.customer_message;
        }

        self.updated_at = self
            .updated_at
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.active_since = self
            .active_since
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.status_page_url = self
            .status_page_url
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.actions = self
            .actions
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();

        for (name, fallback) in defaults.components {
            self.components
                .entry(name)
                .and_modify(|component| component.normalize(&fallback))
                .or_insert(fallback);
        }

        self
    }
}

fn load_incident_status_record(state: &RpcState) -> IncidentStatusRecord {
    let Some(path) = state.incident_status_path.as_ref() else {
        return IncidentStatusRecord::normal(&state.network_id);
    };

    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return IncidentStatusRecord::status_feed_error(&state.network_id),
    };

    match serde_json::from_str::<IncidentStatusRecord>(&raw) {
        Ok(status) => status.normalize(&state.network_id, "file"),
        Err(_) => IncidentStatusRecord::status_feed_error(&state.network_id),
    }
}

fn stable_json_stringify(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(boolean) => boolean.to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => {
            serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
        }
        serde_json::Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(stable_json_stringify).collect();
            format!("[{}]", parts.join(","))
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
            keys.sort_unstable();
            let parts: Vec<String> = keys
                .into_iter()
                .filter_map(|key| {
                    map.get(key).map(|entry| {
                        format!(
                            "{}:{}",
                            serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string()),
                            stable_json_stringify(entry)
                        )
                    })
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

fn signed_metadata_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn live_signed_metadata_source_rpc() -> &'static str {
    LIVE_SIGNED_METADATA_SOURCE_RPC
}

fn env_var_enabled(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1")
            | Some("true")
            | Some("TRUE")
            | Some("yes")
            | Some("YES")
            | Some("on")
            | Some("ON")
    )
}

fn live_signed_metadata_runtime_enabled() -> bool {
    env_var_enabled("LICHEN_ENABLE_LIVE_SIGNED_METADATA")
}

fn signed_metadata_keypair_path_from_env() -> Option<PathBuf> {
    let path = std::env::var("LICHEN_SIGNED_METADATA_KEYPAIR_FILE")
        .ok()
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())?;

    if live_signed_metadata_runtime_enabled() {
        return Some(path);
    }

    warn!(
        "ignoring LICHEN_SIGNED_METADATA_KEYPAIR_FILE for this RPC runtime; serve a persisted signed metadata manifest unless LICHEN_ENABLE_LIVE_SIGNED_METADATA=1"
    );
    None
}

const SIGNED_METADATA_REGISTRY_PAGE_SIZE: usize = 512;

fn load_live_signed_metadata_registry_entries(
    state: &RpcState,
) -> Result<Vec<SymbolRegistryEntry>, RpcError> {
    let mut entries = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut page = state
            .state
            .get_all_symbol_registry_paginated(
                SIGNED_METADATA_REGISTRY_PAGE_SIZE,
                cursor.as_deref(),
            )
            .map_err(|error| RpcError {
                code: -32000,
                message: format!("Database error: {}", error),
            })?;

        if page.is_empty() {
            break;
        }

        let has_more = page.len() == SIGNED_METADATA_REGISTRY_PAGE_SIZE;
        cursor = page.last().map(|entry| entry.symbol.clone());
        entries.append(&mut page);

        if !has_more {
            break;
        }
    }

    entries.sort_by(|left, right| left.symbol.cmp(&right.symbol));
    entries.dedup_by(|left, right| left.symbol == right.symbol);
    Ok(entries)
}

fn build_live_signed_metadata_snapshot(
    state: &RpcState,
) -> Result<(serde_json::Value, String), RpcError> {
    let entries = load_live_signed_metadata_registry_entries(state)?;

    let registry_entries: Vec<serde_json::Value> = entries
        .into_iter()
        .map(symbol_registry_entry_to_json)
        .collect();
    let snapshot = serde_json::json!({
        "schema_version": SIGNED_METADATA_MANIFEST_SCHEMA_VERSION,
        "network": state.network_id.clone(),
        "source_rpc": live_signed_metadata_source_rpc(),
        "symbol_registry": registry_entries,
    });
    let snapshot_key = stable_json_stringify(&snapshot);
    Ok((snapshot, snapshot_key))
}

fn build_signed_metadata_payload(
    snapshot: &serde_json::Value,
    generated_at: &str,
) -> Result<serde_json::Value, RpcError> {
    let mut payload = snapshot.as_object().cloned().ok_or_else(|| RpcError {
        code: -32000,
        message: "Signed metadata snapshot must be a JSON object".to_string(),
    })?;
    payload.insert(
        "generated_at".to_string(),
        serde_json::Value::String(generated_at.to_string()),
    );
    Ok(serde_json::Value::Object(payload))
}

fn signed_metadata_snapshot_key_from_payload(
    payload: &serde_json::Value,
) -> Result<String, RpcError> {
    let object = payload.as_object().ok_or_else(|| RpcError {
        code: -32000,
        message: "Signed metadata payload must be a JSON object".to_string(),
    })?;
    let snapshot = serde_json::json!({
        "schema_version": object
            .get("schema_version")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::from(SIGNED_METADATA_MANIFEST_SCHEMA_VERSION)),
        "network": object.get("network").cloned().unwrap_or(serde_json::Value::Null),
        "source_rpc": object
            .get("source_rpc")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "symbol_registry": object
            .get("symbol_registry")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
    });
    Ok(stable_json_stringify(&snapshot))
}

fn decode_signed_metadata_key_bytes(
    value: &serde_json::Value,
    field_name: &str,
    path: &Path,
) -> Result<Vec<u8>, String> {
    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_u64()
                    .filter(|byte| *byte <= u8::MAX as u64)
                    .map(|byte| byte as u8)
                    .ok_or_else(|| {
                        format!(
                            "{} field in signed metadata keypair {} must be a byte array",
                            field_name,
                            path.display()
                        )
                    })
            })
            .collect(),
        serde_json::Value::String(encoded) => {
            let normalized = encoded
                .strip_prefix("0x")
                .or_else(|| encoded.strip_prefix("0X"))
                .unwrap_or(encoded);
            hex::decode(normalized).map_err(|error| {
                format!(
                    "{} field in signed metadata keypair {} must be valid hex: {}",
                    field_name,
                    path.display(),
                    error
                )
            })
        }
        _ => Err(format!(
            "{} field in signed metadata keypair {} must be a byte array or hex string",
            field_name,
            path.display()
        )),
    }
}

fn load_seed_only_signed_metadata_keypair(path: &Path) -> Result<lichen_core::Keypair, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {}: {}", path.display(), error))?;
    let value = serde_json::from_str::<serde_json::Value>(&raw)
        .map_err(|error| format!("Failed to parse {}: {}", path.display(), error))?;
    let object = value.as_object().ok_or_else(|| {
        format!(
            "Signed metadata keypair {} must be a JSON object",
            path.display()
        )
    })?;

    let seed_value = object
        .get("privateKey")
        .or_else(|| object.get("seed"))
        .ok_or_else(|| {
            format!(
                "Signed metadata keypair {} is missing privateKey/seed",
                path.display()
            )
        })?;
    let seed_bytes = decode_signed_metadata_key_bytes(seed_value, "privateKey", path)?;
    if seed_bytes.len() != 32 {
        return Err(format!(
            "Signed metadata keypair {} has invalid private seed length {} (expected 32 bytes)",
            path.display(),
            seed_bytes.len()
        ));
    }

    let mut seed = [0u8; 32];
    seed.copy_from_slice(&seed_bytes);
    let keypair = lichen_core::Keypair::from_seed(&seed);
    seed.fill(0);

    if let Some(public_key_value) = object.get("publicKey") {
        let public_key_bytes =
            decode_signed_metadata_key_bytes(public_key_value, "publicKey", path)?;
        if keypair.public_key().bytes != public_key_bytes {
            return Err(format!(
                "Signed metadata keypair {} publicKey does not match the derived PQ verifying key",
                path.display()
            ));
        }
    }

    if let Some(expected_signer) = object
        .get("publicKeyBase58")
        .or_else(|| object.get("address_base58"))
        .and_then(serde_json::Value::as_str)
    {
        if keypair.pubkey().to_base58() != expected_signer {
            return Err(format!(
                "Signed metadata keypair {} publicKeyBase58 does not match the derived PQ address",
                path.display()
            ));
        }
    }

    Ok(keypair)
}

fn load_signed_metadata_keypair(path: &Path) -> Result<lichen_core::Keypair, RpcError> {
    KeypairFile::load(path)
        .and_then(|keypair_file| keypair_file.to_keypair())
        .or_else(|canonical_error| {
            load_seed_only_signed_metadata_keypair(path).map_err(|seed_error| {
                format!(
                    "canonical loader error: {}; seed-only fallback error: {}",
                    canonical_error, seed_error
                )
            })
        })
        .map_err(|error| RpcError {
            code: -32000,
            message: format!(
                "Failed to load signed metadata keypair from {}: {}",
                path.display(),
                error
            ),
        })
}

fn read_signed_metadata_manifest_file_value(
    state: &RpcState,
) -> Result<serde_json::Value, RpcError> {
    let path = state
        .signed_metadata_manifest_path
        .as_ref()
        .ok_or_else(|| RpcError {
            code: -32000,
            message: "Signed metadata manifest is not configured on this RPC node".to_string(),
        })?;

    let raw = fs::read_to_string(path).map_err(|error| RpcError {
        code: -32000,
        message: format!(
            "Failed to read signed metadata manifest from {}: {}",
            path.display(),
            error
        ),
    })?;

    let manifest = serde_json::from_str::<serde_json::Value>(&raw).map_err(|error| RpcError {
        code: -32000,
        message: format!(
            "Failed to parse signed metadata manifest from {}: {}",
            path.display(),
            error
        ),
    })?;

    if !manifest.is_object() {
        return Err(RpcError {
            code: -32000,
            message: format!(
                "Signed metadata manifest at {} must be a JSON object",
                path.display()
            ),
        });
    }

    if manifest.get("payload").is_none() || manifest.get("signature").is_none() {
        return Err(RpcError {
            code: -32000,
            message: format!(
                "Signed metadata manifest at {} must include payload and signature",
                path.display()
            ),
        });
    }

    Ok(manifest)
}

fn manifest_matches_live_snapshot(
    manifest: &serde_json::Value,
    snapshot_key: &str,
    signer: &str,
    keypair: &lichen_core::Keypair,
) -> bool {
    if manifest
        .get("manifest_type")
        .and_then(serde_json::Value::as_str)
        != Some("signed_metadata")
    {
        return false;
    }

    if manifest.get("signer").and_then(serde_json::Value::as_str) != Some(signer) {
        return false;
    }

    let Some(payload) = manifest.get("payload") else {
        return false;
    };
    let Ok(existing_snapshot_key) = signed_metadata_snapshot_key_from_payload(payload) else {
        return false;
    };
    if existing_snapshot_key != snapshot_key {
        return false;
    }

    let Some(signature_value) = manifest.get("signature") else {
        return false;
    };
    let Ok(signature) = serde_json::from_value::<PqSignature>(signature_value.clone()) else {
        return false;
    };

    let payload_bytes = stable_json_stringify(payload).into_bytes();
    lichen_core::Keypair::verify(&keypair.pubkey(), &payload_bytes, &signature)
}

fn generate_signed_metadata_manifest_value(
    snapshot: &serde_json::Value,
    keypair: &lichen_core::Keypair,
) -> Result<serde_json::Value, RpcError> {
    let generated_at = signed_metadata_timestamp();
    let payload = build_signed_metadata_payload(snapshot, &generated_at)?;
    let payload_bytes = stable_json_stringify(&payload).into_bytes();
    let signature = keypair.sign(&payload_bytes);

    Ok(serde_json::json!({
        "schema_version": SIGNED_METADATA_MANIFEST_SCHEMA_VERSION,
        "manifest_type": "signed_metadata",
        "signed_at": generated_at,
        "signer": keypair.pubkey().to_base58(),
        "payload": payload,
        "signature": signature,
    }))
}

fn persist_signed_metadata_manifest_best_effort(state: &RpcState, manifest: &serde_json::Value) {
    let Some(path) = state.signed_metadata_manifest_path.as_ref() else {
        return;
    };

    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(error) = fs::create_dir_all(parent) {
        warn!(
            "failed to create signed metadata manifest directory {}: {}",
            parent.display(),
            error
        );
        return;
    }

    let serialized = match serde_json::to_string_pretty(manifest) {
        Ok(mut json) => {
            json.push('\n');
            json
        }
        Err(error) => {
            warn!(
                "failed to serialize live signed metadata manifest: {}",
                error
            );
            return;
        }
    };

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("signed-metadata-manifest.json");
    let temp_path = path.with_file_name(format!(".{}.{}.tmp", file_name, std::process::id()));

    if let Err(error) = fs::write(&temp_path, serialized.as_bytes()) {
        warn!(
            "failed to write live signed metadata manifest temp file {}: {}",
            temp_path.display(),
            error
        );
        return;
    }

    if let Err(error) = fs::rename(&temp_path, path) {
        warn!(
            "failed to publish live signed metadata manifest to {}: {}",
            path.display(),
            error
        );
        let _ = fs::remove_file(&temp_path);
    }
}

async fn load_signed_metadata_manifest_value(
    state: &RpcState,
) -> Result<serde_json::Value, RpcError> {
    let Some(keypair_path) = state.signed_metadata_keypair_path.as_ref() else {
        return read_signed_metadata_manifest_file_value(state);
    };

    let (snapshot, snapshot_key) = build_live_signed_metadata_snapshot(state)?;

    {
        let guard = state.signed_metadata_manifest_cache.read().await;
        if let Some(cached) = guard.as_ref() {
            if cached.snapshot_key == snapshot_key {
                return Ok(cached.manifest.clone());
            }
        }
    }

    let keypair = match load_signed_metadata_keypair(keypair_path) {
        Ok(keypair) => keypair,
        Err(error) => {
            if state.signed_metadata_manifest_path.is_some() {
                warn!(
                    "{}; serving persisted signed metadata manifest instead",
                    error.message
                );
                return read_signed_metadata_manifest_file_value(state);
            }
            return Err(error);
        }
    };
    let signer = keypair.pubkey().to_base58();

    if let Some(path) = state.signed_metadata_manifest_path.as_ref() {
        if let Ok(raw) = fs::read_to_string(path) {
            if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&raw) {
                if manifest_matches_live_snapshot(&manifest, &snapshot_key, &signer, &keypair) {
                    let mut guard = state.signed_metadata_manifest_cache.write().await;
                    *guard = Some(SignedMetadataManifestCacheEntry {
                        snapshot_key,
                        manifest: manifest.clone(),
                    });
                    return Ok(manifest);
                }
            }
        }
    }

    let manifest = generate_signed_metadata_manifest_value(&snapshot, &keypair)?;
    persist_signed_metadata_manifest_best_effort(state, &manifest);

    let mut guard = state.signed_metadata_manifest_cache.write().await;
    *guard = Some(SignedMetadataManifestCacheEntry {
        snapshot_key,
        manifest: manifest.clone(),
    });

    Ok(manifest)
}

fn default_service_expected() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct ServiceFleetProbeConfig {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    body_contains_any: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct ServiceFleetServiceConfig {
    #[serde(default)]
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    service: String,
    #[serde(default = "default_service_expected")]
    expected: bool,
    #[serde(default)]
    intentionally_absent_message: String,
    #[serde(default)]
    probe: ServiceFleetProbeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct ServiceFleetHostConfig {
    #[serde(default)]
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    services: Vec<ServiceFleetServiceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct ServiceFleetConfigRecord {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    network: String,
    #[serde(default)]
    probe_timeout_ms: Option<u64>,
    #[serde(default)]
    hosts: Vec<ServiceFleetHostConfig>,
}

impl ServiceFleetConfigRecord {
    fn normalize(mut self, network_id: &str) -> Self {
        if self.schema_version == 0 {
            self.schema_version = 1;
        }
        if self.network.trim().is_empty() {
            self.network = network_id.to_string();
        }

        self.hosts = self
            .hosts
            .into_iter()
            .filter_map(|mut host| {
                host.id = host.id.trim().to_string();
                host.label = host.label.trim().to_string();
                if host.id.is_empty() {
                    return None;
                }
                if host.label.is_empty() {
                    host.label = host.id.clone();
                }

                host.services = host
                    .services
                    .into_iter()
                    .filter_map(|mut service| {
                        service.id = service.id.trim().to_string();
                        service.label = service.label.trim().to_string();
                        service.service = service.service.trim().to_string();
                        service.probe.kind = service.probe.kind.trim().to_string();
                        service.probe.url = service.probe.url.trim().to_string();
                        service.probe.method = service
                            .probe
                            .method
                            .map(|value| value.trim().to_string())
                            .filter(|value| !value.is_empty());
                        service.probe.body_contains_any = service
                            .probe
                            .body_contains_any
                            .into_iter()
                            .map(|value| value.trim().to_string())
                            .filter(|value| !value.is_empty())
                            .collect();
                        service.intentionally_absent_message =
                            service.intentionally_absent_message.trim().to_string();

                        if service.id.is_empty() {
                            return None;
                        }
                        if service.label.is_empty() {
                            service.label = service.id.clone();
                        }
                        if service.service.is_empty() {
                            service.service = service.id.clone();
                        }

                        Some(service)
                    })
                    .collect();

                Some(host)
            })
            .collect();

        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct ServiceFleetServiceStatusRecord {
    #[serde(default)]
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    service: String,
    #[serde(default)]
    host_id: String,
    #[serde(default)]
    host_label: String,
    #[serde(default)]
    expected: bool,
    #[serde(default)]
    intentionally_absent: bool,
    #[serde(default)]
    state: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    last_checked_at: Option<u64>,
    #[serde(default)]
    last_success_at: Option<u64>,
    #[serde(default)]
    latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct ServiceFleetHostStatusRecord {
    #[serde(default)]
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    healthy_services: u64,
    #[serde(default)]
    degraded_services: u64,
    #[serde(default)]
    intentionally_absent_services: u64,
    #[serde(default)]
    services: Vec<ServiceFleetServiceStatusRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct ServiceFleetSummaryRecord {
    #[serde(default)]
    host_count: u64,
    #[serde(default)]
    total_services: u64,
    #[serde(default)]
    healthy_services: u64,
    #[serde(default)]
    degraded_services: u64,
    #[serde(default)]
    intentionally_absent_services: u64,
    #[serde(default)]
    last_success_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ServiceFleetStatusRecord {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    source: String,
    #[serde(default)]
    network: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    updated_at: Option<u64>,
    #[serde(default)]
    probe_timeout_ms: Option<u64>,
    #[serde(default)]
    summary: ServiceFleetSummaryRecord,
    #[serde(default)]
    hosts: Vec<ServiceFleetHostStatusRecord>,
}

impl ServiceFleetStatusRecord {
    fn unconfigured(network_id: &str) -> Self {
        Self {
            schema_version: 1,
            source: "default".to_string(),
            network: network_id.to_string(),
            state: "unconfigured".to_string(),
            updated_at: Some(now_unix_ms()),
            probe_timeout_ms: None,
            summary: ServiceFleetSummaryRecord::default(),
            hosts: Vec::new(),
        }
    }

    fn probe_error(network_id: &str, message: &str) -> Self {
        let mut record = Self::unconfigured(network_id);
        record.source = "error".to_string();
        record.hosts = vec![ServiceFleetHostStatusRecord {
            id: "service-fleet".to_string(),
            label: "Service Fleet".to_string(),
            healthy_services: 0,
            degraded_services: 1,
            intentionally_absent_services: 0,
            services: vec![ServiceFleetServiceStatusRecord {
                id: "service-fleet".to_string(),
                label: "Service Fleet".to_string(),
                service: "service-fleet".to_string(),
                host_id: "service-fleet".to_string(),
                host_label: "Service Fleet".to_string(),
                expected: true,
                intentionally_absent: false,
                state: "degraded".to_string(),
                message: message.to_string(),
                kind: "internal".to_string(),
                url: String::new(),
                last_checked_at: record.updated_at,
                last_success_at: None,
                latency_ms: None,
            }],
        }];
        record.recompute_summary();
        record.state = "probe_error".to_string();
        record
    }

    fn recompute_summary(&mut self) {
        let mut summary = ServiceFleetSummaryRecord {
            host_count: self.hosts.len() as u64,
            total_services: 0,
            healthy_services: 0,
            degraded_services: 0,
            intentionally_absent_services: 0,
            last_success_at: None,
        };

        for host in &mut self.hosts {
            host.healthy_services = 0;
            host.degraded_services = 0;
            host.intentionally_absent_services = 0;

            for service in &host.services {
                summary.total_services += 1;
                if service.intentionally_absent {
                    summary.intentionally_absent_services += 1;
                    host.intentionally_absent_services += 1;
                } else if service.state == "healthy" {
                    summary.healthy_services += 1;
                    host.healthy_services += 1;
                } else {
                    summary.degraded_services += 1;
                    host.degraded_services += 1;
                }

                if let Some(last_success_at) = service.last_success_at {
                    summary.last_success_at = Some(
                        summary
                            .last_success_at
                            .map(|current| current.max(last_success_at))
                            .unwrap_or(last_success_at),
                    );
                }
            }
        }

        self.summary = summary;
        self.state = if self.summary.total_services == 0 {
            "unconfigured".to_string()
        } else if self.summary.degraded_services > 0 {
            "degraded".to_string()
        } else {
            "healthy".to_string()
        };
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn write_json_file_atomic<T: Serialize>(path: &PathBuf, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let temp_path = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    fs::write(&temp_path, bytes)?;
    fs::rename(temp_path, path)?;
    Ok(())
}

fn load_service_fleet_config_record(state: &RpcState) -> Result<ServiceFleetConfigRecord, String> {
    let Some(path) = state.service_fleet_config_path.as_ref() else {
        return Err("Service fleet config is not configured on this RPC node".to_string());
    };

    let raw = fs::read_to_string(path).map_err(|error| {
        format!(
            "Failed to read service fleet config from {}: {}",
            path.display(),
            error
        )
    })?;

    serde_json::from_str::<ServiceFleetConfigRecord>(&raw)
        .map(|record| record.normalize(&state.network_id))
        .map_err(|error| {
            format!(
                "Failed to parse service fleet config from {}: {}",
                path.display(),
                error
            )
        })
}

fn load_service_fleet_previous_status(
    path: Option<&PathBuf>,
) -> HashMap<String, ServiceFleetServiceStatusRecord> {
    let Some(path) = path else {
        return HashMap::new();
    };

    let Ok(raw) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(status) = serde_json::from_str::<ServiceFleetStatusRecord>(&raw) else {
        return HashMap::new();
    };

    status
        .hosts
        .into_iter()
        .flat_map(|host| {
            host.services.into_iter().map(move |service| {
                (
                    format!("{}:{}", host.id, service.id),
                    ServiceFleetServiceStatusRecord {
                        host_id: host.id.clone(),
                        host_label: host.label.clone(),
                        ..service
                    },
                )
            })
        })
        .collect()
}

async fn fetch_upstream_service_fleet_status(
    upstream_rpc_url: &str,
) -> Result<ServiceFleetStatusRecord, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(3_000))
        .build()
        .map_err(|error| {
            format!(
                "Failed to construct service-fleet upstream HTTP client: {}",
                error
            )
        })?;

    let response = client
        .post(upstream_rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getServiceFleetStatus",
            "params": []
        }))
        .send()
        .await
        .map_err(|error| {
            format!(
                "Failed to request upstream service fleet status from {}: {}",
                upstream_rpc_url, error
            )
        })?
        .error_for_status()
        .map_err(|error| {
            format!(
                "Upstream service fleet status request to {} returned an error: {}",
                upstream_rpc_url, error
            )
        })?;

    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| {
            format!(
                "Failed to decode upstream service fleet response from {}: {}",
                upstream_rpc_url, error
            )
        })?;

    if let Some(error) = payload.get("error") {
        return Err(format!(
            "Upstream service fleet RPC {} returned an error payload: {}",
            upstream_rpc_url, error
        ));
    }

    let mut status = serde_json::from_value::<ServiceFleetStatusRecord>(
        payload.get("result").cloned().ok_or_else(|| {
            format!(
                "Upstream service fleet RPC {} did not include a result field",
                upstream_rpc_url
            )
        })?,
    )
    .map_err(|error| {
        format!(
            "Failed to decode upstream service fleet status from {}: {}",
            upstream_rpc_url, error
        )
    })?;

    status.source = "upstream".to_string();
    Ok(status)
}

fn service_fleet_health_from_rpc_result(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(flag) => *flag,
        serde_json::Value::String(text) => {
            matches!(text.trim().to_ascii_lowercase().as_str(), "ok" | "healthy")
        }
        serde_json::Value::Object(map) => map
            .get("status")
            .and_then(|status| status.as_str())
            .map(|status| {
                matches!(
                    status.trim().to_ascii_lowercase().as_str(),
                    "ok" | "healthy"
                )
            })
            .unwrap_or(false),
        _ => false,
    }
}

async fn probe_service_fleet_service(
    client: &reqwest::Client,
    host: &ServiceFleetHostConfig,
    service: &ServiceFleetServiceConfig,
    previous: Option<&ServiceFleetServiceStatusRecord>,
) -> ServiceFleetServiceStatusRecord {
    let checked_at = now_unix_ms();
    let mut record = ServiceFleetServiceStatusRecord {
        id: service.id.clone(),
        label: service.label.clone(),
        service: service.service.clone(),
        host_id: host.id.clone(),
        host_label: host.label.clone(),
        expected: service.expected,
        intentionally_absent: !service.expected,
        state: if service.expected {
            "degraded".to_string()
        } else {
            "absent".to_string()
        },
        message: if service.expected {
            "Probe pending".to_string()
        } else if !service.intentionally_absent_message.is_empty() {
            service.intentionally_absent_message.clone()
        } else {
            "Not deployed on this host by design.".to_string()
        },
        kind: service.probe.kind.clone(),
        url: service.probe.url.clone(),
        last_checked_at: Some(checked_at),
        last_success_at: previous.and_then(|status| status.last_success_at),
        latency_ms: None,
    };

    if !service.expected {
        return record;
    }

    if service.probe.kind.is_empty() || service.probe.url.is_empty() {
        record.message = "Probe kind and URL must be configured for expected services.".to_string();
        return record;
    }

    let started_at = Instant::now();
    let result = if service.probe.kind.eq_ignore_ascii_case("jsonrpc") {
        client
            .post(&service.probe.url)
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": service.probe.method.as_deref().unwrap_or("getHealth"),
                "params": []
            }))
            .send()
            .await
            .and_then(|response| response.error_for_status())
    } else {
        client
            .get(&service.probe.url)
            .send()
            .await
            .and_then(|response| response.error_for_status())
    };

    match result {
        Ok(response) => {
            let latency_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
            record.latency_ms = Some(latency_ms);

            if service.probe.kind.eq_ignore_ascii_case("jsonrpc") {
                match response.json::<serde_json::Value>().await {
                    Ok(payload) => {
                        let healthy = payload
                            .get("result")
                            .map(service_fleet_health_from_rpc_result)
                            .unwrap_or(false);
                        if healthy {
                            record.state = "healthy".to_string();
                            record.message = "JSON-RPC health probe passed.".to_string();
                            record.last_success_at = Some(checked_at);
                        } else {
                            record.message =
                                "JSON-RPC health probe returned an unhealthy result.".to_string();
                        }
                    }
                    Err(error) => {
                        record.message =
                            format!("Failed to decode JSON-RPC probe response: {}", error);
                    }
                }
            } else {
                match response.text().await {
                    Ok(body) => {
                        let matched = service.probe.body_contains_any.is_empty()
                            || service
                                .probe
                                .body_contains_any
                                .iter()
                                .any(|fragment| body.contains(fragment));
                        if matched {
                            record.state = "healthy".to_string();
                            record.message = "HTTP health probe passed.".to_string();
                            record.last_success_at = Some(checked_at);
                        } else {
                            record.message =
                                "HTTP health probe response did not match the expected body."
                                    .to_string();
                        }
                    }
                    Err(error) => {
                        record.message =
                            format!("Failed to read HTTP probe response body: {}", error);
                    }
                }
            }
        }
        Err(error) => {
            record.message = format!("Probe request failed: {}", error);
        }
    }

    record
}

async fn refresh_service_fleet_status(state: &RpcState) -> ServiceFleetStatusRecord {
    if let Some(upstream_rpc_url) = state.service_fleet_upstream_rpc_url.as_deref() {
        match fetch_upstream_service_fleet_status(upstream_rpc_url).await {
            Ok(status) => {
                if let Some(path) = state.service_fleet_status_path.as_ref() {
                    if let Err(error) = write_json_file_atomic(path, &status) {
                        warn!(
                            "Failed to persist upstream service fleet status to {}: {}",
                            path.display(),
                            error
                        );
                    }
                }
                return status;
            }
            Err(error) => {
                warn!(
                    "Failed to fetch upstream service fleet status from {}: {}",
                    upstream_rpc_url, error
                );
            }
        }
    }

    let config = match load_service_fleet_config_record(state) {
        Ok(config) => config,
        Err(message) => return ServiceFleetStatusRecord::probe_error(&state.network_id, &message),
    };

    if config.hosts.is_empty() {
        return ServiceFleetStatusRecord::unconfigured(&config.network);
    }

    let timeout_ms = config.probe_timeout_ms.unwrap_or(3_000).max(250);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return ServiceFleetStatusRecord::probe_error(
                &config.network,
                &format!("Failed to construct service-fleet HTTP client: {}", error),
            )
        }
    };

    let previous_statuses =
        load_service_fleet_previous_status(state.service_fleet_status_path.as_ref());
    let mut hosts = Vec::with_capacity(config.hosts.len());

    for host in &config.hosts {
        let mut services = Vec::with_capacity(host.services.len());
        for service in &host.services {
            let key = format!("{}:{}", host.id, service.id);
            services.push(
                probe_service_fleet_service(&client, host, service, previous_statuses.get(&key))
                    .await,
            );
        }
        hosts.push(ServiceFleetHostStatusRecord {
            id: host.id.clone(),
            label: host.label.clone(),
            healthy_services: 0,
            degraded_services: 0,
            intentionally_absent_services: 0,
            services,
        });
    }

    let mut status = ServiceFleetStatusRecord {
        schema_version: 1,
        source: "probe".to_string(),
        network: config.network,
        state: "healthy".to_string(),
        updated_at: Some(now_unix_ms()),
        probe_timeout_ms: Some(timeout_ms),
        summary: ServiceFleetSummaryRecord::default(),
        hosts,
    };
    status.recompute_summary();

    if let Some(path) = state.service_fleet_status_path.as_ref() {
        if let Err(error) = write_json_file_atomic(path, &status) {
            warn!(
                "Failed to persist service fleet status to {}: {}",
                path.display(),
                error
            );
        }
    }

    status
}

fn incident_mode_matches(status: &IncidentStatusRecord, modes: &[&str]) -> bool {
    modes
        .iter()
        .any(|mode| status.mode.eq_ignore_ascii_case(mode))
}

fn incident_component_is_blocked(status: &IncidentStatusRecord, component: &str) -> bool {
    status
        .components
        .get(component)
        .map(|component_status| {
            matches!(
                component_status.status.trim().to_ascii_lowercase().as_str(),
                "paused" | "blocked" | "disabled" | "frozen"
            )
        })
        .unwrap_or(false)
}

fn bridge_deposit_incident_block_reason(status: &IncidentStatusRecord) -> Option<&'static str> {
    if incident_component_is_blocked(status, "bridge")
        || incident_mode_matches(status, &["bridge_pause"])
    {
        return Some("bridge deposits are temporarily paused while bridge risk is assessed");
    }
    if incident_component_is_blocked(status, "deposits")
        || incident_mode_matches(status, &["deposit_guard", "deposit_only_freeze"])
    {
        return Some("new deposits are temporarily paused while operators verify inbound activity");
    }
    None
}

fn parse_pq_signature_value(value: &serde_json::Value) -> Result<PqSignature, RpcError> {
    if value.is_object() {
        return serde_json::from_value(value.clone()).map_err(|error| RpcError {
            code: -32602,
            message: format!("Invalid PQ signature object: {}", error),
        });
    }

    if let Some(encoded) = value.as_str() {
        return serde_json::from_str(encoded).map_err(|error| RpcError {
            code: -32602,
            message: format!("Invalid PQ signature JSON string: {}", error),
        });
    }

    Err(RpcError {
        code: -32602,
        message: "Signature must be a PQ signature object or JSON string".to_string(),
    })
}

const BRIDGE_ACCESS_DOMAIN: &str = "LICHEN_BRIDGE_ACCESS_V1";
const BRIDGE_ACCESS_MAX_TTL_SECS: u64 = 24 * 60 * 60;
const BRIDGE_ACCESS_CLOCK_SKEW_SECS: u64 = 300;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BridgeAccessAuth {
    issued_at: u64,
    expires_at: u64,
    signature: serde_json::Value,
}

fn bridge_access_message(user_id: &str, issued_at: u64, expires_at: u64) -> Vec<u8> {
    format!(
        "{}\nuser_id={}\nissued_at={}\nexpires_at={}\n",
        BRIDGE_ACCESS_DOMAIN, user_id, issued_at, expires_at
    )
    .into_bytes()
}

fn current_unix_secs() -> Result<u64, RpcError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| RpcError {
            code: -32000,
            message: format!("System clock error: {}", error),
        })
}

fn parse_bridge_access_auth(value: &serde_json::Value) -> Result<BridgeAccessAuth, RpcError> {
    serde_json::from_value(value.clone()).map_err(|error| RpcError {
        code: -32602,
        message: format!("Invalid bridge auth object: {}", error),
    })
}

fn verify_bridge_access_auth(user_id: &str, auth: &BridgeAccessAuth) -> Result<(), RpcError> {
    verify_bridge_access_auth_at(user_id, auth, current_unix_secs()?)
}

fn verify_bridge_access_auth_at(
    user_id: &str,
    auth: &BridgeAccessAuth,
    now: u64,
) -> Result<(), RpcError> {
    if auth.expires_at <= auth.issued_at {
        return Err(RpcError {
            code: -32602,
            message: "bridge auth expires_at must be greater than issued_at".to_string(),
        });
    }

    if auth.expires_at - auth.issued_at > BRIDGE_ACCESS_MAX_TTL_SECS {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "bridge auth exceeds max ttl of {} seconds",
                BRIDGE_ACCESS_MAX_TTL_SECS
            ),
        });
    }

    if auth.issued_at > now.saturating_add(BRIDGE_ACCESS_CLOCK_SKEW_SECS) {
        return Err(RpcError {
            code: -32602,
            message: "bridge auth issued_at is too far in the future".to_string(),
        });
    }

    if auth.expires_at < now {
        return Err(RpcError {
            code: -32602,
            message: "bridge auth has expired".to_string(),
        });
    }

    let user_pubkey = Pubkey::from_base58(user_id).map_err(|_| RpcError {
        code: -32602,
        message: "user_id must be a valid Lichen base58 public key (32 bytes)".to_string(),
    })?;
    let signature = parse_pq_signature_value(&auth.signature)?;
    let message = bridge_access_message(user_id, auth.issued_at, auth.expires_at);
    if !lichen_core::account::Keypair::verify(&user_pubkey, &message, &signature) {
        return Err(RpcError {
            code: -32602,
            message: "Invalid bridge auth signature".to_string(),
        });
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// SHARED STATE & TYPES
// ═══════════════════════════════════════════════════════════════════════════════

/// JSON-RPC request
#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[serde(rename = "jsonrpc")]
    _jsonrpc: String,
    id: serde_json::Value,
    method: String,
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RpcTierProbe {
    #[serde(default)]
    id: Option<serde_json::Value>,
    method: String,
}

/// JSON-RPC response
#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: String,
    id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

/// JSON-RPC error
#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

fn method_not_found_error() -> RpcError {
    RpcError {
        code: -32601,
        message: "Method not found".to_string(),
    }
}

fn invalid_pubkey_format_error() -> RpcError {
    RpcError {
        code: -32602,
        message: "Invalid pubkey format".to_string(),
    }
}

fn invalid_address_filter_error() -> RpcError {
    RpcError {
        code: -32602,
        message: "Invalid address filter format".to_string(),
    }
}

fn symbol_not_found_error() -> RpcError {
    RpcError {
        code: -32001,
        message: "Symbol not found".to_string(),
    }
}

/// P2-4: Content-type negotiation for binary RPC responses.
///
/// Inspects the `Accept` header and serializes the RPC response accordingly:
///   - `application/msgpack`        → MessagePack (rmp-serde)
///   - `application/octet-stream`   → bincode
///   - anything else / absent       → JSON (default)
fn encode_rpc_response(headers: &HeaderMap, response: RpcResponse) -> Response {
    let accept = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json");

    if accept.contains("application/msgpack") {
        if let Ok(bytes) = rmp_serde::to_vec_named(&response) {
            return Response::builder()
                .status(200)
                .header("content-type", "application/msgpack")
                .body(axum::body::Body::from(bytes))
                .unwrap_or_else(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "msgpack response build failed",
                    )
                        .into_response()
                });
        }
    } else if accept.contains("application/octet-stream") {
        if let Ok(bytes) = bincode::serialize(&response) {
            return Response::builder()
                .status(200)
                .header("content-type", "application/octet-stream")
                .body(axum::body::Body::from(bytes))
                .unwrap_or_else(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "bincode response build failed",
                    )
                        .into_response()
                });
        }
    }

    // Default: JSON
    (StatusCode::OK, Json(response)).into_response()
}

fn sanitize_rpc_error_message(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    let storage_detail = lower.contains("database error")
        || lower.contains("rocksdb")
        || lower.contains("column family")
        || lower.contains(" cf ")
        || lower.contains("cf_");

    if storage_detail {
        return "Database error".to_string();
    }

    // Generic path redaction for non-storage errors
    let mut redacted = Vec::new();
    for token in message.split_whitespace() {
        let is_path = token.starts_with('/')
            || token.contains("/Users/")
            || token.contains("/home/")
            || token.starts_with("file://");
        if is_path {
            redacted.push("<redacted-path>");
        } else {
            redacted.push(token);
        }
    }
    redacted.join(" ")
}

fn sanitize_rpc_error(mut error: RpcError) -> RpcError {
    error.message = sanitize_rpc_error_message(&error.message);
    error
}

fn jsonrpc_error_response(
    status: StatusCode,
    id: serde_json::Value,
    code: i32,
    message: impl Into<String>,
) -> Response {
    (
        status,
        Json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message.into(),
            }
        })),
    )
        .into_response()
}

#[allow(clippy::result_large_err)]
fn parse_rpc_tier_probe(body: &[u8]) -> Result<RpcTierProbe, Response> {
    serde_json::from_slice(body).map_err(|_| {
        jsonrpc_error_response(
            StatusCode::BAD_REQUEST,
            serde_json::Value::Null,
            -32700,
            "Parse error",
        )
    })
}

#[allow(clippy::result_large_err)]
fn parse_rpc_request(body: &[u8], id: serde_json::Value) -> Result<RpcRequest, Response> {
    serde_json::from_slice(body)
        .map_err(|_| jsonrpc_error_response(StatusCode::BAD_REQUEST, id, -32600, "Invalid request"))
}

fn parse_get_block_slot_param(
    params: Option<&serde_json::Value>,
    include_options_hint: bool,
) -> Result<u64, RpcError> {
    let suffix = if include_options_hint {
        "[slot, options?]"
    } else {
        "[slot]"
    };
    let err_msg = format!(
        "Invalid params: expected {} where slot is a u64 block height (block hash is not supported)",
        suffix
    );

    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: err_msg.clone(),
    })?;

    params_array
        .first()
        .and_then(|v| v.as_u64())
        .ok_or(RpcError {
            code: -32602,
            message: err_msg,
        })
}

/// Shared RPC state
#[derive(Clone)]
struct RpcState {
    state: StateStore,
    /// Channel to send transactions to mempool
    tx_sender: Option<mpsc::Sender<Transaction>>,
    /// P2P network (optional, for peer count queries)
    p2p: Option<Arc<dyn P2PNetworkTrait>>,
    /// Stake pool (optional, for staking queries)
    stake_pool: Option<Arc<tokio::sync::RwLock<lichen_core::StakePool>>>,
    /// Live validator set from the embedded validator process, when available.
    /// This keeps activity-sensitive RPCs aligned with in-memory consensus state.
    live_validator_set: Option<Arc<RwLock<ValidatorSet>>>,
    chain_id: String,
    network_id: String,
    min_validator_stake: u64,
    version: String,
    evm_chain_id: u64,
    solana_tx_cache: Arc<RwLock<LruCache<Hash, SolanaTxRecord>>>,
    /// Admin token for state-mutating RPC endpoints (setFeeConfig, setRentParams, setContractAbi)
    /// Hot-rotatable: a background task re-reads LICHEN_ADMIN_TOKEN env var every 30s.
    admin_token: Arc<std::sync::RwLock<Option<String>>>,
    /// T2.6: Per-IP rate limiter
    rate_limiter: Arc<RateLimiter>,
    /// Lock-free finality tracker for commitment levels (processed/confirmed/finalized)
    finality: Option<FinalityTracker>,
    /// DEX real-time event broadcaster (WS push to subscribers)
    _dex_broadcaster: Arc<dex_ws::DexEventBroadcaster>,
    /// Prediction market real-time event broadcaster
    prediction_broadcaster: Arc<ws::PredictionEventBroadcaster>,
    /// Cached validators list — refreshed at most once per slot (~400ms).
    /// Avoids 6+ full CF_VALIDATORS scans per RPC cycle.
    validator_cache: Arc<RwLock<(Instant, Vec<ValidatorInfo>)>>,
    /// Cached metrics JSON — refreshed at most once per slot (~400ms).
    metrics_cache: Arc<RwLock<(Instant, Option<serde_json::Value>)>>,
    /// Cached responses for high-frequency list endpoints.
    program_list_response_cache: Arc<RwLock<LruCache<String, (Instant, serde_json::Value)>>>,
    /// AUDIT-FIX RPC-4: Per-address airdrop cooldown to prevent abuse.
    /// Bounded + async lock to avoid blocking runtime and unbounded growth.
    airdrop_cooldowns: Arc<RwLock<AirdropCooldowns>>,
    /// DEX orderbook cache — per-pair aggregated book levels, refreshed at most once per second.
    /// Eliminates O(total_orders) scan per request; cached result served in O(1).
    orderbook_cache: Arc<RwLock<HashMap<u64, (Instant, serde_json::Value)>>>,
    /// Custody service URL for bridge deposit proxy (e.g. http://localhost:9105)
    custody_url: Option<String>,
    /// Bearer token for custody API auth
    custody_auth_token: Option<String>,
    /// Optional incident-response manifest consumed by getIncidentStatus.
    incident_status_path: Option<PathBuf>,
    /// Optional signed metadata manifest file used as a fallback and best-effort persisted cache.
    signed_metadata_manifest_path: Option<PathBuf>,
    /// Optional release-signing keypair used to synthesize a live signed metadata manifest.
    /// Intended for local development or explicit opt-in runtimes only.
    signed_metadata_keypair_path: Option<PathBuf>,
    /// Cached live signed metadata manifest keyed by the current registry snapshot.
    signed_metadata_manifest_cache: Arc<RwLock<Option<SignedMetadataManifestCacheEntry>>>,
    /// Optional service-fleet probe config consumed by getServiceFleetStatus.
    service_fleet_config_path: Option<PathBuf>,
    /// Optional authoritative RPC endpoint used to proxy service fleet status.
    service_fleet_upstream_rpc_url: Option<String>,
    /// Optional cached service-fleet status persisted by getServiceFleetStatus.
    service_fleet_status_path: Option<PathBuf>,
    /// Cached service-fleet probe results to avoid re-probing on every request.
    service_fleet_status_cache: Arc<RwLock<(Instant, Option<ServiceFleetStatusRecord>)>>,
    /// Treasury keypair for signing consensus-based airdrop transactions.
    /// Loaded from the treasury keypair file at startup.
    treasury_keypair: Option<Arc<TreasuryKeypair>>,
}

#[derive(Debug, Clone)]
struct SignedMetadataManifestCacheEntry {
    snapshot_key: String,
    manifest: serde_json::Value,
}

const AIRDROP_COOLDOWN_SECS: u64 = 60;
const AIRDROP_COOLDOWN_STALE_SECS: u64 = 120;
const AIRDROP_COOLDOWN_MAX_ENTRIES: usize = 10_000;
/// Daily per-address airdrop limit in LICN (matches faucet-service).
const AIRDROP_DAILY_LIMIT_LICN: u64 = 150;
const AIRDROP_DAILY_WINDOW_SECS: u64 = 86_400;

#[derive(Default)]
struct AirdropCooldowns {
    by_address: HashMap<String, Instant>,
    /// Per-address daily airdrop ledger: (timestamp, amount_licn) entries.
    daily_ledger: HashMap<String, Vec<(Instant, u64)>>,
    order: VecDeque<String>,
}

impl AirdropCooldowns {
    fn prune_stale(&mut self, now: Instant) {
        while let Some(front) = self.order.front().cloned() {
            let stale = self
                .by_address
                .get(&front)
                .map(|ts| now.duration_since(*ts).as_secs() >= AIRDROP_COOLDOWN_STALE_SECS)
                .unwrap_or(true);
            if !stale {
                break;
            }
            self.order.pop_front();
            self.by_address.remove(&front);
        }
        // Prune daily ledger entries older than 24h
        self.daily_ledger.retain(|_, entries| {
            entries.retain(|(ts, _)| now.duration_since(*ts).as_secs() < AIRDROP_DAILY_WINDOW_SECS);
            !entries.is_empty()
        });
    }

    fn evict_overflow(&mut self) {
        while self.by_address.len() > AIRDROP_COOLDOWN_MAX_ENTRIES {
            if let Some(front) = self.order.pop_front() {
                self.by_address.remove(&front);
            } else {
                break;
            }
        }
    }

    /// Check daily limit for an address. Returns Err with message if exceeded.
    fn check_daily_limit(
        &self,
        address: &str,
        amount_licn: u64,
        now: Instant,
    ) -> Result<(), String> {
        let used: u64 = self
            .daily_ledger
            .get(address)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|(ts, _)| now.duration_since(*ts).as_secs() < AIRDROP_DAILY_WINDOW_SECS)
                    .map(|(_, amt)| *amt)
                    .sum()
            })
            .unwrap_or(0);
        if used.saturating_add(amount_licn) > AIRDROP_DAILY_LIMIT_LICN {
            return Err(format!(
                "Daily airdrop limit reached for this address ({}/{} LICN). Try again later.",
                used, AIRDROP_DAILY_LIMIT_LICN
            ));
        }
        Ok(())
    }

    /// Record an airdrop amount in the daily ledger.
    fn record_daily(&mut self, address: &str, amount_licn: u64, now: Instant) {
        self.daily_ledger
            .entry(address.to_string())
            .or_default()
            .push((now, amount_licn));
    }

    /// Returns Some(remaining_secs) if still cooling down, otherwise records access and returns None.
    fn check_and_record(&mut self, address: &str, now: Instant) -> Option<u64> {
        self.prune_stale(now);

        if let Some(last) = self.by_address.get(address) {
            let elapsed = now.duration_since(*last).as_secs();
            if elapsed < AIRDROP_COOLDOWN_SECS {
                return Some(AIRDROP_COOLDOWN_SECS - elapsed);
            }
        }

        let key = address.to_string();
        self.by_address.insert(key.clone(), now);
        self.order.push_back(key);
        self.evict_overflow();
        None
    }
}

/// H16 fix: Guard state-mutating RPC endpoints in multi-validator mode.
/// Direct state writes bypass consensus and cause divergence when >1 validator.
/// In multi-validator mode, callers must submit proper signed transactions
/// via `sendTransaction` instead.
pub(crate) async fn require_single_validator(
    state: &RpcState,
    endpoint: &str,
) -> Result<(), RpcError> {
    let validators = cached_validators(state).await.map_err(|e| RpcError {
        code: e.code,
        message: format!(
            "{} unavailable: failed to load validator set ({})",
            endpoint, e.message
        ),
    })?;
    if validators.len() > 1 {
        return Err(RpcError {
            code: -32003,
            message: format!(
                "{} is disabled in multi-validator mode ({} validators active). \
                 Submit a signed transaction via sendTransaction instead.",
                endpoint,
                validators.len()
            ),
        });
    }
    // AUDIT-FIX RPC-03: Log when single-validator direct-write endpoints are used.
    // These bypass consensus and should only be used in devnet/testing.
    warn!(
        "DEVNET-ONLY: {} called via direct state write (single-validator mode). \
         This bypasses consensus and must not be used in production.",
        endpoint
    );
    Ok(())
}

/// Cached validator list — avoids redundant CF_VALIDATORS full-scans within a
/// single slot (~400ms).  Six RPC handlers previously scanned the same CF
/// independently; this collapses them into at most one scan per slot.
const VALIDATOR_CACHE_TTL_MS: u128 = 400;

async fn cached_validators(state: &RpcState) -> Result<Vec<ValidatorInfo>, RpcError> {
    if let Some(ref live_validator_set) = state.live_validator_set {
        return Ok(live_validator_set.read().await.validators().to_vec());
    }

    // Fast path: read lock
    {
        let guard = state.validator_cache.read().await;
        if guard.0.elapsed().as_millis() < VALIDATOR_CACHE_TTL_MS && !guard.1.is_empty() {
            return Ok(guard.1.clone());
        }
    }
    // Slow path: write lock + refresh
    let mut guard = state.validator_cache.write().await;
    // Double check — another task may have refreshed while we waited for the write lock
    if guard.0.elapsed().as_millis() < VALIDATOR_CACHE_TTL_MS && !guard.1.is_empty() {
        return Ok(guard.1.clone());
    }
    let validators = state.state.get_all_validators().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;
    *guard = (Instant::now(), validators.clone());
    Ok(validators)
}

async fn load_validator_info(
    state: &RpcState,
    pubkey: &Pubkey,
) -> Result<Option<ValidatorInfo>, RpcError> {
    if let Some(ref live_validator_set) = state.live_validator_set {
        return Ok(live_validator_set
            .read()
            .await
            .get_validator(pubkey)
            .cloned());
    }

    state.state.get_validator(pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })
}

fn should_expose_public_validator(state: &RpcState, validator: &ValidatorInfo) -> bool {
    // Hide discovery-only validator records that never obtained stake or
    // produced any consensus-visible activity. These can appear when an
    // external peer broadcasts validator announcements solely for P2P routing.
    validator.blocks_proposed > 0
        || validator.votes_cast > 0
        || validator.stake > 0
        || state
            .state
            .get_account(&validator.pubkey)
            .ok()
            .flatten()
            .map(|account| account.staked > 0)
            .unwrap_or(false)
}

fn program_list_cache_key(method: &str, params: &Option<serde_json::Value>) -> String {
    let params_key = params
        .as_ref()
        .and_then(|v| serde_json::to_string(v).ok())
        .unwrap_or_else(|| "null".to_string());
    format!("{}:{}", method, params_key)
}

async fn get_cached_program_list_response(
    state: &RpcState,
    method: &str,
    params: &Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let key = program_list_cache_key(method, params);
    let mut guard = state.program_list_response_cache.write().await;
    match guard.get(&key).cloned() {
        Some((ts, cached)) if ts.elapsed().as_millis() < PROGRAM_LIST_CACHE_TTL_MS => Some(cached),
        Some(_) => {
            guard.pop(&key);
            None
        }
        None => None,
    }
}

fn prune_stale_program_list_entries(cache: &mut LruCache<String, (Instant, serde_json::Value)>) {
    loop {
        let stale_lru = cache
            .peek_lru()
            .map(|(_, (ts, _))| ts.elapsed().as_millis() >= PROGRAM_LIST_CACHE_TTL_MS * 2)
            .unwrap_or(false);
        if !stale_lru {
            break;
        }
        drop(cache.pop_lru());
    }
}

async fn put_cached_program_list_response(
    state: &RpcState,
    method: &str,
    params: &Option<serde_json::Value>,
    response: serde_json::Value,
) {
    let key = program_list_cache_key(method, params);
    let mut guard = state.program_list_response_cache.write().await;

    prune_stale_program_list_entries(&mut guard);
    guard.put(key, (Instant::now(), response));
}

/// AUDIT-FIX HIGH-03: Exact allowlist instead of substring matching.
/// Only the following network/chain IDs enable legacy admin RPCs:
const LEGACY_ADMIN_ALLOWED_IDS: &[&str] = &[
    "local",
    "dev",
    "localnet",
    "devnet",
    "lichen-test",
    "lichen-testnet-local",
    "lichen-testnet-1",
    "lichen-devnet-1",
    "lichen-local",
];

fn allow_legacy_admin_rpc(chain_id: &str, network_id: &str) -> bool {
    let chain = chain_id.to_ascii_lowercase();
    let network = network_id.to_ascii_lowercase();

    LEGACY_ADMIN_ALLOWED_IDS.iter().any(|id| network == *id)
        || LEGACY_ADMIN_ALLOWED_IDS.iter().any(|id| chain == *id)
}

fn require_legacy_admin_rpc_enabled(state: &RpcState, endpoint: &str) -> Result<(), RpcError> {
    if allow_legacy_admin_rpc(&state.chain_id, &state.network_id) {
        return Ok(());
    }

    Err(RpcError {
        code: -32003,
        message: format!(
            "{} is disabled outside local/dev environments. Use consensus transactions, governance, or deterministic deployment artifacts instead.",
            endpoint
        ),
    })
}

fn is_legacy_admin_method(method: &str) -> bool {
    matches!(
        method,
        "setFeeConfig" | "setRentParams" | "setContractAbi" | "deployContract" | "upgradeContract"
    )
}

fn require_legacy_admin_rpc_local_origin(
    method: &str,
    connect_info: Option<&ConnectInfo<SocketAddr>>,
) -> Result<(), RpcError> {
    let Some(connect_info) = connect_info else {
        return Ok(());
    };

    if connect_info.0.ip().is_loopback() {
        return Ok(());
    }

    Err(RpcError {
        code: -32003,
        message: format!(
            "{} is restricted to loopback clients on local/dev networks. Use localhost or an offline maintenance path instead.",
            method
        ),
    })
}

/// AUDIT-FIX HIGH-02: Strip any admin_token from JSON body params to prevent
/// clients from embedding secrets in loggable request payloads.
fn strip_admin_token_from_params(params: Option<serde_json::Value>) -> Option<serde_json::Value> {
    match params {
        Some(serde_json::Value::Object(mut obj)) => {
            obj.remove("admin_token");
            Some(serde_json::Value::Object(obj))
        }
        Some(serde_json::Value::Array(mut arr)) => {
            for value in &mut arr {
                if let Some(obj) = value.as_object_mut() {
                    obj.remove("admin_token");
                }
            }
            Some(serde_json::Value::Array(arr))
        }
        other => other,
    }
}

/// AUDIT-FIX HIGH-02: Admin auth uses Authorization header only.
/// The admin_token is never injected into or read from JSON body params.
fn verify_admin_auth(state: &RpcState, auth_header: Option<&str>) -> Result<(), RpcError> {
    let guard = state.admin_token.read().map_err(|_| RpcError {
        code: -32000,
        message: "Internal error: admin token lock poisoned".to_string(),
    })?;
    let required_token = guard.as_ref().ok_or_else(|| RpcError {
        code: -32003,
        message: "Admin endpoints disabled: no admin_token configured".to_string(),
    })?;

    let token = auth_header
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| t.trim());

    match token {
        Some(t) if constant_time_eq(t.as_bytes(), required_token.as_bytes()) => Ok(()),
        Some(_) => Err(RpcError {
            code: -32003,
            message: "Invalid admin token".to_string(),
        }),
        None => Err(RpcError {
            code: -32003,
            message: "Missing Authorization: Bearer <token> header".to_string(),
        }),
    }
}

fn log_privileged_rpc_mutation(
    method: &str,
    auth_scope: &str,
    actor: &str,
    resource_type: &str,
    resource_id: Option<&str>,
    details: serde_json::Value,
) {
    let details = serde_json::to_string(&details)
        .unwrap_or_else(|_| "{\"serialization_error\":true}".to_string());

    info!(
        target: "audit",
        event = "privileged_rpc_mutation",
        method,
        auth_scope,
        actor,
        resource_type,
        resource_id = resource_id.unwrap_or(""),
        details = %details,
        "Privileged RPC mutation executed"
    );
}

/// T8.4: Constant-time byte comparison to prevent timing side-channel attacks
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Debug, Clone)]
struct SolanaTxRecord {
    tx: Transaction,
    slot: u64,
    timestamp: u64,
    fee: u64,
}

/// Trait for P2P network to allow RPC queries
pub trait P2PNetworkTrait: Send + Sync {
    fn peer_count(&self) -> usize;
    fn peer_addresses(&self) -> Vec<String>;
}

// ═══════════════════════════════════════════════════════════════════════════════
// RATE LIMITING & MIDDLEWARE
// ═══════════════════════════════════════════════════════════════════════════════

/// P9-RPC-03: Method cost tier for tiered rate limiting.
/// Expensive methods (e.g., sendTransaction, simulateTransaction) get a lower
/// per-second cap than cheap read-only queries (e.g., getBalance, getSlot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MethodTier {
    /// Cheap read-only lookups (getBalance, getSlot, health)
    Cheap,
    /// Moderate reads that touch indexes or iterate (getTransactionsByAddress)
    Moderate,
    /// Expensive writes or simulations (sendTransaction, simulateTransaction)
    Expensive,
}

/// P9-RPC-03: Classify an RPC method name into a cost tier.
fn classify_method(method: &str) -> MethodTier {
    match method {
        // Writes / simulations
        "sendTransaction"
        | "simulateTransaction"
        | "estimateTransactionFee"
        | "deployContract"
        | "upgradeContract"
        | "stake"
        | "unstake"
        | "requestAirdrop"
        | "setFeeConfig"
        | "setRentParams"
        | "setContractAbi"
        | "callContract" => MethodTier::Expensive,

        // Moderate reads (iterate indexes, join data)
        "getTransactionsByAddress"
        | "getTransactionHistory"
        | "getRecentTransactions"
        | "getBlock"
        | "getBlockCommit"
        | "getAccountProof"
        | "getTokenHolders"
        | "getTokenTransfers"
        | "getContractEvents"
        | "getGovernanceEvents"
        | "getContractLogs"
        | "getNFTsByOwner"
        | "getNFTsByCollection"
        | "getNFTActivity"
        | "getMarketListings"
        | "getMarketSales"
        | "getProgramCalls"
        | "getProgramStorage"
        | "getPrograms"
        | "getAllContracts"
        | "getAllSymbolRegistry"
        | "getAllSymbols"
        | "getPredictionMarkets"
        | "getPredictionLeaderboard"
        | "batchReverseLichenNames"
        | "searchLichenNames"
        | "getServiceFleetStatus"
        | "getUnstakingQueue" => MethodTier::Moderate,

        // Everything else is a cheap point lookup
        _ => MethodTier::Cheap,
    }
}

fn classify_solana_method_tier(method: &str) -> MethodTier {
    match method {
        "sendTransaction" => MethodTier::Expensive,
        "getSignaturesForAddress" | "getSignatureStatuses" | "getBlock" => MethodTier::Moderate,
        _ => MethodTier::Cheap,
    }
}

/// T2.6: Per-IP rate limiter using DashMap for lock-free concurrent access.
/// P1-5: Upgraded from Mutex<HashMap> to DashMap to eliminate contention
/// under high request rates (10K+ req/s). DashMap uses shard-level locking
/// (16+ shards), so concurrent reads/writes don't serialize.
struct RateLimiter {
    requests: DashMap<IpAddr, (u64, Instant)>,
    max_per_second: u64,
    /// RPC-L02: Optional cross-process shared global limiter state file.
    shared_counter_file: Option<PathBuf>,
    last_prune: std::sync::Mutex<Instant>,
    /// P9-RPC-03: Per-tier per-IP counters.
    tier_requests: DashMap<(IpAddr, MethodTier), (u64, Instant)>,
    /// Per-second limits for each tier.
    tier_limits: [u64; 3], // [Cheap, Moderate, Expensive]
    /// Dedicated per-IP counters for REST /api/v1 tiered rate limiting.
    rest_tier_requests: DashMap<(IpAddr, MethodTier), (u64, Instant)>,
    /// REST per-second limits for each tier.
    rest_tier_limits: [u64; 3], // [Cheap, Moderate, Expensive]
}

impl RateLimiter {
    fn new(max_per_second: u64) -> Self {
        let shared_counter_file = std::env::var("RPC_RATE_LIMIT_SHARED_FILE")
            .ok()
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty());

        Self {
            requests: DashMap::new(),
            max_per_second,
            shared_counter_file,
            last_prune: std::sync::Mutex::new(Instant::now()),
            tier_requests: DashMap::new(),
            // P9-RPC-03: Default tier limits.
            // Cheap: 100% of global cap, Moderate: 40%, Expensive: 10%
            tier_limits: [
                max_per_second,                         // Cheap
                max_per_second * 2 / 5,                 // Moderate (40%)
                std::cmp::max(max_per_second / 10, 50), // Expensive (10%, min 50)
            ],
            rest_tier_requests: DashMap::new(),
            rest_tier_limits: [200, 100, 50],
        }
    }

    fn check_shared_global(&self, ip: IpAddr) -> Option<bool> {
        const LOCK_ATTEMPTS: usize = 20;
        const LOCK_RETRY_DELAY_MS: u64 = 3;

        let shared_file = self.shared_counter_file.as_ref()?;
        if let Some(parent) = shared_file.parent() {
            if fs::create_dir_all(parent).is_err() {
                return None;
            }
        }

        let lock_path = PathBuf::from(format!("{}.lock", shared_file.display()));
        let mut lock_file = None;
        for _ in 0..LOCK_ATTEMPTS {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(file) => {
                    lock_file = Some(file);
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    std::thread::sleep(Duration::from_millis(LOCK_RETRY_DELAY_MS));
                }
                Err(_) => return None,
            }
        }

        let result = if lock_file.is_some() {
            let mut counters: HashMap<String, (u64, u64)> =
                if let Ok(mut file) = OpenOptions::new().read(true).open(shared_file) {
                    let mut content = String::new();
                    if file.read_to_string(&mut content).is_ok() {
                        serde_json::from_str(&content).unwrap_or_default()
                    } else {
                        HashMap::new()
                    }
                } else {
                    HashMap::new()
                };

            let now_sec = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            counters.retain(|_, (_, ts_sec)| now_sec.saturating_sub(*ts_sec) <= 1);

            let key = ip.to_string();
            let entry = counters.entry(key).or_insert((0, now_sec));
            if entry.1 != now_sec {
                entry.0 = 0;
                entry.1 = now_sec;
            }

            entry.0 += 1;
            let allowed = entry.0 <= self.max_per_second;

            if let Ok(bytes) = serde_json::to_vec(&counters) {
                let tmp_path = PathBuf::from(format!("{}.tmp", shared_file.display()));
                if fs::write(&tmp_path, bytes).is_ok() {
                    if let Err(e) = fs::rename(&tmp_path, shared_file) {
                        tracing::warn!("rate limiter: failed to persist counters: {e}");
                    }
                } else {
                    if let Err(e) = fs::remove_file(&tmp_path) {
                        tracing::debug!("rate limiter: failed to clean up tmp file: {e}");
                    }
                }
            }

            Some(allowed)
        } else {
            None
        };

        drop(lock_file);
        if let Err(e) = fs::remove_file(&lock_path) {
            tracing::debug!("rate limiter: failed to remove lock file: {e}");
        }
        result
    }

    /// Check if a request from `ip` is within the global rate limit.
    fn classify_rest_tier(path: &str, method: &Method) -> Option<MethodTier> {
        if !path.starts_with("/api/v1") {
            return None;
        }

        let is_write =
            *method == Method::POST || *method == Method::PUT || *method == Method::DELETE;
        let expensive_paths = [
            "/orders",
            "/swap",
            "/liquidat",
            "/bridge",
            "/position",
            "/mint",
        ];
        let moderate_paths = [
            "/pairs",
            "/orderbook",
            "/trades",
            "/candles",
            "/history",
            "/analytics",
            "/market",
        ];

        if is_write || expensive_paths.iter().any(|needle| path.contains(needle)) {
            return Some(MethodTier::Expensive);
        }

        if moderate_paths.iter().any(|needle| path.contains(needle)) {
            return Some(MethodTier::Moderate);
        }

        Some(MethodTier::Cheap)
    }
    /// Returns `true` if allowed, `false` if rate-limited.
    fn check(&self, ip: IpAddr) -> bool {
        let now = Instant::now();

        // Prune stale entries every 30 seconds to prevent memory exhaustion
        {
            let mut last = self.last_prune.lock().unwrap_or_else(|e| e.into_inner());
            if now.duration_since(*last).as_secs() >= 30 {
                self.requests
                    .retain(|_, (_, ts)| now.duration_since(*ts).as_secs() < 60);
                self.tier_requests
                    .retain(|_, (_, ts)| now.duration_since(*ts).as_secs() < 60);
                self.rest_tier_requests
                    .retain(|_, (_, ts)| now.duration_since(*ts).as_secs() < 60);
                *last = now;
            }
        }

        let allowed_local = {
            let mut entry = self.requests.entry(ip).or_insert((0, now));
            if now.duration_since(entry.1).as_secs() >= 1 {
                entry.0 = 1;
                entry.1 = now;
                true
            } else {
                entry.0 += 1;
                entry.0 <= self.max_per_second
            }
        };

        if !allowed_local {
            return false;
        }

        if let Some(allowed_shared) = self.check_shared_global(ip) {
            return allowed_shared;
        }

        true
    }

    /// P9-RPC-03: Check if a request from `ip` for method `tier` is within
    /// the tier-specific rate limit.  Should be called AFTER `check()` passes.
    fn check_tier(&self, ip: IpAddr, tier: MethodTier) -> bool {
        let limit = self.tier_limits[tier as usize];
        let now = Instant::now();
        let mut entry = self.tier_requests.entry((ip, tier)).or_insert((0, now));
        if now.duration_since(entry.1).as_secs() >= 1 {
            entry.0 = 1;
            entry.1 = now;
            true
        } else {
            entry.0 += 1;
            entry.0 <= limit
        }
    }

    fn check_rest_tier(&self, ip: IpAddr, tier: MethodTier) -> bool {
        let limit = self.rest_tier_limits[tier as usize];
        let now = Instant::now();
        let mut entry = self
            .rest_tier_requests
            .entry((ip, tier))
            .or_insert((0, now));
        if now.duration_since(entry.1).as_secs() >= 1 {
            entry.0 = 1;
            entry.1 = now;
            true
        } else {
            entry.0 += 1;
            entry.0 <= limit
        }
    }
}

/// T2.6: Extract client IP from ConnectInfo extension.
/// Does NOT trust X-Forwarded-For to prevent rate limit bypass via header spoofing.
fn extract_client_ip(req: &axum::extract::Request) -> IpAddr {
    // Use ConnectInfo (available when using into_make_service_with_connect_info)
    if let Some(connect_info) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        return connect_info.0.ip();
    }
    // Fallback: use loopback (which will be rate-limited like any other IP)
    IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))
}

/// T2.6: Rate limiting middleware
async fn rate_limit_middleware(
    State(state): State<Arc<RpcState>>,
    req: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    // Allow CORS preflight through without rate limiting
    if req.method() == Method::OPTIONS {
        return next.run(req).await;
    }
    let ip = extract_client_ip(&req);
    if !state.rate_limiter.check(ip) {
        warn!("T2.6: Rate limit exceeded for IP {}", ip);
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32005, "message": "Rate limit exceeded"}
            })),
        )
            .into_response();
    }

    if let Some(tier) = RateLimiter::classify_rest_tier(req.uri().path(), req.method()) {
        if !state.rate_limiter.check_rest_tier(ip, tier) {
            let label = match tier {
                MethodTier::Expensive => "expensive",
                MethodTier::Moderate => "moderate",
                MethodTier::Cheap => "cheap",
            };
            warn!(
                "RPC-C01: /api/v1 {} endpoint rate limit exceeded for IP {} (path: {})",
                label,
                ip,
                req.uri().path()
            );
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32005,
                        "message": format!("Rate limit exceeded for /api/v1 {} endpoints", label)
                    }
                })),
            )
                .into_response();
        }
    }

    next.run(req).await
}

// ═══════════════════════════════════════════════════════════════════════════════
// UTILITY FUNCTIONS & HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Helper: Count executable accounts (contracts) using O(1) MetricsStore counter
fn count_executable_accounts(state: &StateStore) -> u64 {
    state.get_program_count()
}

fn parse_transfer_amount(ix: &Instruction) -> Option<u64> {
    if ix.program_id == CONTRACT_PROGRAM_ID {
        // Contract calls: parse JSON payload to extract "value" field
        if let Ok(json_str) = std::str::from_utf8(&ix.data) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                if let Some(call) = val.get("Call") {
                    if let Some(v) = call.get("value").and_then(|v| v.as_u64()) {
                        return Some(v);
                    }
                }
            }
        }
        return None;
    }
    if ix.program_id != SYSTEM_PROGRAM_ID {
        return None;
    }
    if ix.data.len() < 9 {
        return None;
    }
    // Parse amount from data[1..9] for instruction types that carry an amount:
    // 0=Transfer, 2=Reward, 3=GrantRepay, 4=GenesisTransfer, 5=GenesisMint,
    // 9=Stake, 10=Unstake, 13=MossStakeDeposit, 14=MossStakeUnstake,
    // 16=MossStakeTransfer, 19=FaucetAirdrop, 21=ProposeGovernedTransfer,
    // 22=ApproveGovernedTransfer, 23=Shield, 24=Unshield
    match ix.data[0] {
        0 | 2 | 3 | 4 | 5 | 9 | 10 | 13 | 14 | 16 | 19 | 21 | 22 | 23 | 24 => {
            let amount_bytes: [u8; 8] = ix.data[1..9].try_into().ok()?;
            Some(u64::from_le_bytes(amount_bytes))
        }
        _ => None,
    }
}

/// Extract the function name from a contract call instruction (for display purposes)
fn parse_contract_function(ix: &Instruction) -> Option<String> {
    if ix.program_id != CONTRACT_PROGRAM_ID {
        return None;
    }
    if let Ok(json_str) = std::str::from_utf8(&ix.data) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
            if let Some(call) = val.get("Call") {
                return call
                    .get("function")
                    .and_then(|f| f.as_str())
                    .map(|s| s.to_string());
            }
            if val.get("Deploy").is_some() {
                return Some("deploy".to_string());
            }
        }
    }
    None
}

fn parse_contract_call_args(ix: &Instruction) -> Option<Vec<u8>> {
    if ix.program_id != CONTRACT_PROGRAM_ID {
        return None;
    }
    let json_str = std::str::from_utf8(&ix.data).ok()?;
    let val: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let args = val.get("Call")?.get("args")?.as_array()?;
    let mut out = Vec::with_capacity(args.len());
    for item in args {
        let n = item.as_u64()?;
        if n > 255 {
            return None;
        }
        out.push(n as u8);
    }
    Some(out)
}

fn emit_prediction_events_from_tx(state: &RpcState, tx: &Transaction) {
    let predict_program = match state.state.get_symbol_registry("PREDICT") {
        Ok(Some(entry)) => entry.program,
        _ => return,
    };
    let slot_hint = state.state.get_last_slot().unwrap_or(0).saturating_add(1);

    for ix in &tx.message.instructions {
        if ix.program_id != CONTRACT_PROGRAM_ID
            || ix.accounts.len() < 2
            || ix.accounts[1] != predict_program
        {
            continue;
        }
        let Some(args) = parse_contract_call_args(ix) else {
            continue;
        };
        if args.is_empty() {
            continue;
        }

        match args[0] {
            // create_market
            1 if args.len() >= 43 => {
                let market_id = state
                    .state
                    .get_program_storage_u64("PREDICT", b"pm_market_count")
                    .saturating_add(1);
                state.prediction_broadcaster.emit_market_created(
                    market_id,
                    "New market",
                    slot_hint,
                );
            }
            // buy_shares / sell_shares
            4 | 5 if args.len() >= 50 => {
                let market_id = u64::from_le_bytes(args[33..41].try_into().unwrap_or([0u8; 8]));
                let outcome = args[41];
                let amount = u64::from_le_bytes(args[42..50].try_into().unwrap_or([0u8; 8]));
                let outcome_name = if outcome == 0 {
                    "yes"
                } else if outcome == 1 {
                    "no"
                } else {
                    "other"
                };
                state.prediction_broadcaster.emit_trade(
                    market_id,
                    outcome_name,
                    amount,
                    0.0,
                    slot_hint,
                );
            }
            // submit_resolution / finalize_resolution
            8 if args.len() >= 42 => {
                let market_id = u64::from_le_bytes(args[33..41].try_into().unwrap_or([0u8; 8]));
                let winning = args[41];
                let winner = if winning == 0 {
                    "yes"
                } else if winning == 1 {
                    "no"
                } else {
                    "other"
                };
                state
                    .prediction_broadcaster
                    .emit_market_resolved(market_id, winner, slot_hint);
            }
            10 if args.len() >= 41 => {
                let market_id = u64::from_le_bytes(args[33..41].try_into().unwrap_or([0u8; 8]));
                state.prediction_broadcaster.emit_market_resolved(
                    market_id,
                    "finalized",
                    slot_hint,
                );
            }
            _ => {}
        }
    }
}

/// Extract token metadata from a contract-call instruction.
/// Returns (symbol, token_amount_spores, decimals) if the target contract is a
/// token/wrapped contract and the function carries an amount argument.
/// Extracts token info from a contract call instruction.
/// Returns (symbol, amount, decimals, optional_recipient_base58).
fn extract_token_info(
    state: &StateStore,
    ix: &Instruction,
) -> Option<(String, u64, u64, Option<String>)> {
    if ix.program_id != CONTRACT_PROGRAM_ID {
        return None;
    }
    let contract_addr = ix.accounts.get(1)?;
    let entry = state.get_symbol_registry_by_program(contract_addr).ok()??;
    let template = entry.template.as_deref().or_else(|| {
        entry
            .metadata
            .as_ref()
            .and_then(|m| m.get("template"))
            .and_then(|v| v.as_str())
    })?;
    if template != "wrapped" && template != "token" {
        return None;
    }
    let decimals = entry
        .metadata
        .as_ref()
        .and_then(|m| m.get("decimals"))
        .and_then(|v| v.as_u64())
        .unwrap_or(9);
    let function = parse_contract_function(ix)?;
    let args = parse_contract_call_args(ix)?;
    let (amount, recipient) = match function.as_str() {
        "mint" if args.len() >= 72 => {
            // mint(caller: [u8;32], to: [u8;32], amount: u64)
            let amt = u64::from_le_bytes(args[64..72].try_into().ok()?);
            let to = Pubkey::new(<[u8; 32]>::try_from(&args[32..64]).ok()?);
            (amt, Some(to.to_base58()))
        }
        "burn" if args.len() >= 40 => {
            // burn(caller: [u8;32], amount: u64)
            let amt = u64::from_le_bytes(args[32..40].try_into().ok()?);
            (amt, None)
        }
        "transfer" if args.len() >= 72 => {
            // transfer(from: [u8;32], to: [u8;32], amount: u64)
            let amt = u64::from_le_bytes(args[64..72].try_into().ok()?);
            let to = Pubkey::new(<[u8; 32]>::try_from(&args[32..64]).ok()?);
            (amt, Some(to.to_base58()))
        }
        _ => return Some((entry.symbol.clone(), 0, decimals, None)),
    };
    Some((entry.symbol.clone(), amount, decimals, recipient))
}

fn instruction_type(ix: &Instruction) -> &'static str {
    if ix.program_id == SYSTEM_PROGRAM_ID {
        if ix.data.first() == Some(&0) {
            return "Transfer";
        }
        if ix.data.first() == Some(&1) {
            return "CreateAccount";
        }
        if ix.data.first() == Some(&2) {
            return "Reward";
        }
        if ix.data.first() == Some(&3) {
            return "GrantRepay";
        }
        if ix.data.first() == Some(&4) {
            return "GenesisTransfer";
        }
        if ix.data.first() == Some(&5) {
            return "GenesisMint";
        }
        if ix.data.first() == Some(&6) {
            return "CreateCollection";
        }
        if ix.data.first() == Some(&7) {
            return "MintNFT";
        }
        if ix.data.first() == Some(&8) {
            return "TransferNFT";
        }
        if ix.data.first() == Some(&9) {
            return "Stake";
        }
        if ix.data.first() == Some(&10) {
            return "Unstake";
        }
        if ix.data.first() == Some(&11) {
            return "ClaimUnstake";
        }
        if ix.data.first() == Some(&12) {
            return "RegisterEvmAddress";
        }
        if ix.data.first() == Some(&13) {
            return "MossStakeDeposit";
        }
        if ix.data.first() == Some(&14) {
            return "MossStakeUnstake";
        }
        if ix.data.first() == Some(&15) {
            return "MossStakeClaim";
        }
        if ix.data.first() == Some(&16) {
            return "MossStakeTransfer";
        }
        if ix.data.first() == Some(&17) {
            return "DeployContract";
        }
        if ix.data.first() == Some(&18) {
            return "SetContractABI";
        }
        if ix.data.first() == Some(&19) {
            return "FaucetAirdrop";
        }
        if ix.data.first() == Some(&20) {
            return "RegisterSymbol";
        }
        if ix.data.first() == Some(&23) {
            return "Shield";
        }
        if ix.data.first() == Some(&24) {
            return "Unshield";
        }
        if ix.data.first() == Some(&25) {
            return "ShieldedTransfer";
        }
        if ix.data.first() == Some(&26) {
            return "RegisterValidator";
        }
        if ix.data.first() == Some(&27) {
            return "SlashValidator";
        }
        if ix.data.first() == Some(&21) {
            return "ProposeGovernedTransfer";
        }
        if ix.data.first() == Some(&22) {
            return "ApproveGovernedTransfer";
        }
        if ix.data.first() == Some(&28) {
            return "DurableNonce";
        }
        if ix.data.first() == Some(&29) {
            return "GovernanceParamChange";
        }
        if ix.data.first() == Some(&30) {
            return "OracleAttestation";
        }
        if ix.data.first() == Some(&31) {
            return "DeregisterValidator";
        }
        return "System";
    }
    if ix.program_id == CONTRACT_PROGRAM_ID {
        if let Ok(json_str) = std::str::from_utf8(&ix.data) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                if val.get("Deploy").is_some() {
                    return "ContractDeploy";
                }
                if val.get("Call").is_some() {
                    return "ContractCall";
                }
                if val.get("Upgrade").is_some() {
                    return "ContractUpgrade";
                }
                if val.is_string() && val.as_str() == Some("Close") {
                    return "ContractClose";
                }
                if val.get("SetUpgradeTimelock").is_some() {
                    return "SetUpgradeTimelock";
                }
                if val.is_string() && val.as_str() == Some("ExecuteUpgrade") {
                    return "ExecuteUpgrade";
                }
                if val.is_string() && val.as_str() == Some("VetoUpgrade") {
                    return "VetoUpgrade";
                }
            }
        }
        return "Contract";
    }
    "Program"
}

fn evm_chain_id_from_chain_id(chain_id: &str) -> u64 {
    let hash = Hash::hash(chain_id.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash.0[..8]);
    let value = u64::from_le_bytes(bytes);
    if value == 0 {
        1
    } else {
        value
    }
}

fn hash_to_base58(hash: &Hash) -> String {
    bs58::encode(hash.0).into_string()
}

fn base58_to_hash(value: &str) -> Result<Hash, RpcError> {
    let bytes = bs58::decode(value).into_vec().map_err(|_| RpcError {
        code: -32602,
        message: "Invalid base58 signature".to_string(),
    })?;
    if bytes.len() != 32 {
        return Err(RpcError {
            code: -32602,
            message: format!("Invalid signature length: {}", bytes.len()),
        });
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&bytes);
    Ok(Hash(hash))
}

// ═══════════════════════════════════════════════════════════════════════════════
// SOLANA SERIALIZATION HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

fn solana_context(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;
    Ok(serde_json::json!({
        "slot": slot,
    }))
}

fn commitment_rank(commitment: &str) -> u8 {
    match commitment {
        "finalized" => 3,
        "confirmed" => 2,
        _ => 1,
    }
}

fn resolve_commitment_slot(state: &RpcState, commitment: &str) -> Result<u64, RpcError> {
    match commitment {
        "finalized" => Ok(if let Some(ref ft) = state.finality {
            ft.finalized_slot()
        } else {
            state.state.get_last_finalized_slot().unwrap_or(0)
        }),
        "confirmed" => Ok(if let Some(ref ft) = state.finality {
            ft.confirmed_slot()
        } else {
            state.state.get_last_confirmed_slot().unwrap_or(0)
        }),
        "processed" => state.state.get_last_slot().map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        }),
        other => Err(RpcError {
            code: -32602,
            message: format!("Unsupported commitment: {}", other),
        }),
    }
}

fn anchored_block_context(
    state: &RpcState,
    commitment: &str,
) -> Result<(u64, lichen_core::Block, serde_json::Value), RpcError> {
    let slot = resolve_commitment_slot(state, commitment)?;
    let block = state
        .state
        .get_block_by_slot(slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32001,
            message: format!(
                "No block available at {} commitment slot {}",
                commitment, slot
            ),
        })?;

    let block_hash = block.hash();
    let context = serde_json::json!({
        "slot": block.header.slot,
        "commitment": commitment,
        "block_hash": block_hash.to_hex(),
        "commit_round": block.commit_round,
        "parent_hash": block.header.parent_hash.to_hex(),
        "state_root": block.header.state_root.to_hex(),
        "tx_root": block.header.tx_root.to_hex(),
        "validators_hash": block.header.validators_hash.to_hex(),
        "timestamp": block.header.timestamp,
        "validator": Pubkey(block.header.validator).to_base58(),
        "block_signature": pq_signature_option_json(block.header.signature.as_ref()),
        "commit_signatures": block.commit_signatures.iter().map(|cs| {
            serde_json::json!({
                "validator": Pubkey(cs.validator).to_base58(),
                "signature": pq_signature_json(&cs.signature),
                "timestamp": cs.timestamp,
            })
        }).collect::<Vec<_>>(),
        "commit_validator_count": block.commit_signatures.len(),
    });

    Ok((slot, block, context))
}

/// Collect unique account keys from a transaction in Solana-compatible order.
fn collect_account_keys(tx: &Transaction) -> Vec<Pubkey> {
    let mut account_keys: Vec<Pubkey> = Vec::new();
    let mut seen: HashSet<Pubkey> = HashSet::new();

    let mut push_key = |key: Pubkey| {
        if seen.insert(key) {
            account_keys.push(key);
        }
    };

    if let Some(first_ix) = tx.message.instructions.first() {
        if let Some(first_acc) = first_ix.accounts.first() {
            push_key(*first_acc);
        }
    }

    for ix in &tx.message.instructions {
        for acc in &ix.accounts {
            push_key(*acc);
        }
    }

    for ix in &tx.message.instructions {
        push_key(ix.program_id);
    }

    account_keys
}

fn solana_message_json(tx: &Transaction) -> (Vec<String>, Vec<serde_json::Value>) {
    let account_keys = collect_account_keys(tx);

    let index_map: HashMap<Pubkey, usize> = account_keys
        .iter()
        .enumerate()
        .map(|(idx, key)| (*key, idx))
        .collect();

    let instructions = tx
        .message
        .instructions
        .iter()
        .map(|ix| {
            let program_id_index = *index_map.get(&ix.program_id).unwrap_or(&0);
            let account_indices: Vec<usize> = ix
                .accounts
                .iter()
                .filter_map(|acc| index_map.get(acc).copied())
                .collect();
            let data_base58 = bs58::encode(&ix.data).into_string();
            serde_json::json!({
                "programIdIndex": program_id_index,
                "accounts": account_indices,
                "data": data_base58,
            })
        })
        .collect();

    let account_keys = account_keys.iter().map(|key| key.to_base58()).collect();

    (account_keys, instructions)
}

/// Look up current balances for all account keys in a transaction.
/// Returns current state balances (post-execution view).
fn account_balances(state: &StateStore, tx: &Transaction) -> Vec<u64> {
    let keys = collect_account_keys(tx);
    keys.iter()
        .map(|pk| {
            state
                .get_account(pk)
                .ok()
                .flatten()
                .map(|a| a.spendable)
                .unwrap_or(0)
        })
        .collect()
}

fn solana_transaction_json(
    state: &StateStore,
    tx: &Transaction,
    slot: u64,
    timestamp: u64,
    fee: u64,
) -> serde_json::Value {
    let (account_keys, instructions) = solana_message_json(tx);
    let signature = hash_to_base58(&tx.signature());

    // F1: Populate balances from current state.
    // postBalances = current on-chain balances for each account key.
    // preBalances = postBalances adjusted by fee (payer index 0 gets fee added back).
    let post_balances = account_balances(state, tx);
    let pre_balances: Vec<u64> = post_balances
        .iter()
        .enumerate()
        .map(|(i, &bal)| if i == 0 { bal.saturating_add(fee) } else { bal })
        .collect();

    serde_json::json!({
        "slot": slot,
        "blockTime": timestamp,
        "meta": {
            "err": serde_json::Value::Null,
            "fee": fee,
            "preBalances": pre_balances,
            "postBalances": post_balances,
            "logMessages": [],
        },
        "transaction": {
            "signatures": [signature],
            "message": {
                "accountKeys": account_keys,
                "recentBlockhash": hash_to_base58(&tx.message.recent_blockhash),
                "instructions": instructions,
            }
        }
    })
}

fn solana_transaction_encoded_json(
    state: &StateStore,
    tx: &Transaction,
    slot: u64,
    timestamp: u64,
    fee: u64,
    encoding: &str,
) -> serde_json::Value {
    let encoded = encode_solana_transaction(tx, encoding);

    let post_balances = account_balances(state, tx);
    // AUDIT-FIX F-9: Reconstruct pre-balances from transaction effects.
    // The fee payer (account[0]) had fee added to their balance before deduction.
    // For transfer instructions (opcode 0), we also reconstruct the amount moved.
    let transfer_amount = tx
        .message
        .instructions
        .first()
        .and_then(|ix| {
            if ix.data.first() == Some(&0) && ix.data.len() >= 9 {
                Some(u64::from_le_bytes(
                    ix.data[1..9].try_into().unwrap_or([0; 8]),
                ))
            } else {
                None
            }
        })
        .unwrap_or(0);
    let pre_balances: Vec<u64> = post_balances
        .iter()
        .enumerate()
        .map(|(i, &bal)| {
            if i == 0 {
                // Fee payer: add back fee AND outgoing transfer
                bal.saturating_add(fee).saturating_add(transfer_amount)
            } else if i == 1 && transfer_amount > 0 {
                // Transfer recipient: subtract received amount
                bal.saturating_sub(transfer_amount)
            } else {
                bal
            }
        })
        .collect();

    serde_json::json!({
        "slot": slot,
        "blockTime": timestamp,
        "meta": {
            "err": serde_json::Value::Null,
            "fee": fee,
            "preBalances": pre_balances,
            "postBalances": post_balances,
            "logMessages": [],
        },
        "transaction": [encoded, encoding],
    })
}

fn solana_block_transaction_json(
    state: &StateStore,
    tx: &Transaction,
    fee: u64,
) -> serde_json::Value {
    let (account_keys, instructions) = solana_message_json(tx);
    let signature = hash_to_base58(&tx.signature());

    let post_balances = account_balances(state, tx);
    let pre_balances: Vec<u64> = post_balances
        .iter()
        .enumerate()
        .map(|(i, &bal)| if i == 0 { bal.saturating_add(fee) } else { bal })
        .collect();

    serde_json::json!({
        "meta": {
            "err": serde_json::Value::Null,
            "fee": fee,
            "preBalances": pre_balances,
            "postBalances": post_balances,
            "logMessages": [],
        },
        "transaction": {
            "signatures": [signature],
            "message": {
                "accountKeys": account_keys,
                "recentBlockhash": hash_to_base58(&tx.message.recent_blockhash),
                "instructions": instructions,
            }
        }
    })
}

fn solana_block_transaction_encoded_json(
    state: &StateStore,
    tx: &Transaction,
    fee: u64,
    encoding: &str,
) -> serde_json::Value {
    let encoded = encode_solana_transaction(tx, encoding);
    let signature = hash_to_base58(&tx.signature());

    let post_balances = account_balances(state, tx);
    let pre_balances: Vec<u64> = post_balances
        .iter()
        .enumerate()
        .map(|(i, &bal)| if i == 0 { bal.saturating_add(fee) } else { bal })
        .collect();

    serde_json::json!({
        "meta": {
            "err": serde_json::Value::Null,
            "fee": fee,
            "preBalances": pre_balances,
            "postBalances": post_balances,
            "logMessages": [],
        },
        "transaction": [encoded, encoding],
        "version": serde_json::Value::Null,
        "signatures": [signature],
    })
}

fn encode_solana_transaction(tx: &Transaction, encoding: &str) -> String {
    let bytes = bincode::serialize(tx).unwrap_or_default();
    match encoding {
        "base58" => bs58::encode(bytes).into_string(),
        _ => {
            use base64::{engine::general_purpose, Engine as _};
            general_purpose::STANDARD.encode(bytes)
        }
    }
}

fn validate_solana_encoding(encoding: &str) -> Result<(), RpcError> {
    match encoding {
        "json" | "base58" | "base64" => Ok(()),
        _ => Err(RpcError {
            code: -32602,
            message: format!("Unsupported encoding: {}", encoding),
        }),
    }
}

fn validate_solana_token_account_encoding(encoding: &str) -> Result<(), RpcError> {
    match encoding {
        "json" | "jsonParsed" | "base58" | "base64" => Ok(()),
        _ => Err(RpcError {
            code: -32602,
            message: format!("Unsupported token account encoding: {}", encoding),
        }),
    }
}

fn token_registry_decimals(registry: Option<&SymbolRegistryEntry>) -> u8 {
    registry
        .and_then(|entry| {
            entry.decimals.or_else(|| {
                entry
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("decimals"))
                    .and_then(|value| value.as_u64())
                    .and_then(|value| u8::try_from(value).ok())
            })
        })
        .unwrap_or(9)
}

fn token_ui_amount(balance: u64, decimals: u8) -> f64 {
    balance as f64 / 10_f64.powi(decimals as i32)
}

fn token_ui_amount_string(balance: u64, decimals: u8) -> String {
    if decimals == 0 {
        return balance.to_string();
    }

    let scale = 10u128.pow(decimals as u32);
    let whole = (balance as u128) / scale;
    let fraction = (balance as u128) % scale;
    if fraction == 0 {
        return whole.to_string();
    }

    let mut fraction_str = format!("{:0width$}", fraction, width = decimals as usize);
    while fraction_str.ends_with('0') {
        fraction_str.pop();
    }
    format!("{}.{}", whole, fraction_str)
}

#[derive(Clone)]
struct SolanaTokenAccountSnapshot {
    token_account: Pubkey,
    mint: Pubkey,
    owner: Pubkey,
    balance: u64,
    decimals: u8,
}

fn encode_solana_token_account_state(snapshot: &SolanaTokenAccountSnapshot) -> Vec<u8> {
    let mut data = Vec::with_capacity(SOLANA_TOKEN_ACCOUNT_SPACE);
    data.extend_from_slice(&snapshot.mint.0);
    data.extend_from_slice(&snapshot.owner.0);
    data.extend_from_slice(&snapshot.balance.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&[0u8; 32]);
    data.push(1u8);
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&[0u8; 32]);
    data
}

fn solana_token_account_data_json(
    snapshot: &SolanaTokenAccountSnapshot,
    encoding: &str,
) -> Result<serde_json::Value, RpcError> {
    validate_solana_token_account_encoding(encoding)?;

    if matches!(encoding, "json" | "jsonParsed") {
        return Ok(serde_json::json!({
            "program": "spl-token",
            "parsed": {
                "type": "account",
                "info": {
                    "mint": snapshot.mint.to_base58(),
                    "owner": snapshot.owner.to_base58(),
                    "tokenAmount": {
                        "amount": snapshot.balance.to_string(),
                        "decimals": snapshot.decimals,
                        "uiAmount": token_ui_amount(snapshot.balance, snapshot.decimals),
                        "uiAmountString": token_ui_amount_string(snapshot.balance, snapshot.decimals),
                    },
                    "state": "initialized",
                },
            },
            "space": SOLANA_TOKEN_ACCOUNT_SPACE,
        }));
    }

    let data = encode_solana_token_account_state(snapshot);
    let encoded = if encoding == "base58" {
        bs58::encode(data).into_string()
    } else {
        use base64::{engine::general_purpose, Engine as _};
        general_purpose::STANDARD.encode(data)
    };
    Ok(serde_json::json!([encoded, encoding]))
}

fn solana_token_account_response(
    snapshot: &SolanaTokenAccountSnapshot,
    encoding: &str,
) -> Result<serde_json::Value, RpcError> {
    Ok(serde_json::json!({
        "data": solana_token_account_data_json(snapshot, encoding)?,
        "executable": false,
        "lamports": SOLANA_TOKEN_ACCOUNT_RENT_EXEMPT_LAMPORTS,
        "owner": SOLANA_SPL_TOKEN_PROGRAM_ID,
        "rentEpoch": 0_u64,
        "space": SOLANA_TOKEN_ACCOUNT_SPACE,
    }))
}

fn load_solana_token_account_snapshot(
    state: &RpcState,
    token_account: &Pubkey,
) -> Result<Option<SolanaTokenAccountSnapshot>, RpcError> {
    let Some((mint, owner)) = state
        .state
        .get_solana_token_account_binding(token_account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
    else {
        return Ok(None);
    };

    let balance = state
        .state
        .get_token_balance(&mint, &owner)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;
    let registry = state
        .state
        .get_symbol_registry_by_program(&mint)
        .ok()
        .flatten();

    Ok(Some(SolanaTokenAccountSnapshot {
        token_account: *token_account,
        mint,
        owner,
        balance,
        decimals: token_registry_decimals(registry.as_ref()),
    }))
}

fn validate_solana_transaction_details(details: &str) -> Result<(), RpcError> {
    match details {
        "full" | "signatures" | "none" => Ok(()),
        _ => Err(RpcError {
            code: -32602,
            message: format!("Unsupported transactionDetails: {}", details),
        }),
    }
}

fn filter_signatures_for_address(
    indexed: Vec<(Hash, u64)>,
    before: Option<Hash>,
    until: Option<Hash>,
    limit: usize,
) -> Vec<(Hash, u64)> {
    let mut results: Vec<(Hash, u64)> = Vec::new();
    let mut started = before.is_none();

    for (hash, slot) in indexed {
        if !started {
            if let Some(before_hash) = before {
                if hash == before_hash {
                    started = true;
                }
            }
            continue;
        }

        if let Some(until_hash) = until {
            if hash == until_hash {
                break;
            }
        }

        results.push((hash, slot));
        if results.len() >= limit {
            break;
        }
    }

    results
}

fn tx_to_rpc_json(
    tx: &Transaction,
    slot: u64,
    timestamp: u64,
    fee_config: &lichen_core::FeeConfig,
    stored_cu: Option<u64>,
    store: &StateStore,
) -> serde_json::Value {
    let first_ix = tx.message.instructions.first();
    let (tx_type, from, to, amount, contract_fn) = if let Some(ix) = first_ix {
        let from = ix.accounts.first().map(|acc| acc.to_base58());
        let to = ix.accounts.get(1).map(|acc| acc.to_base58());
        let amount = parse_transfer_amount(ix);
        let contract_fn = parse_contract_function(ix);
        (instruction_type(ix), from, to, amount, contract_fn)
    } else {
        ("Unknown", None, None, None::<u64>, None)
    };

    let instructions: Vec<serde_json::Value> = tx
        .message
        .instructions
        .iter()
        .map(|ix| {
            let accounts: Vec<String> = ix.accounts.iter().map(|acc| acc.to_base58()).collect();
            serde_json::json!({
                "program_id": ix.program_id.to_base58(),
                "accounts": accounts,
                "data": ix.data,
            })
        })
        .collect();

    let signatures: Vec<serde_json::Value> = tx.signatures.iter().map(pq_signature_json).collect();
    let amount_licn = amount
        .map(|val| val as f64 / 1_000_000_000.0)
        .unwrap_or(0.0);

    let fee = TxProcessor::compute_transaction_fee(tx, fee_config);
    let base_fee = TxProcessor::compute_base_fee(tx, fee_config);
    let priority_fee = TxProcessor::compute_priority_fee(tx);
    let compute_units = stored_cu.unwrap_or_else(|| compute_units_for_tx(tx));
    let compute_budget = tx.message.effective_compute_budget();
    let compute_unit_price = tx.message.effective_compute_unit_price();

    let mut obj = serde_json::json!({
        "signature": tx.signature().to_hex(),
        "message_hash": tx.message_hash().to_hex(),
        "signatures": signatures,
        "slot": slot,
        "block_time": timestamp,
        // AUDIT-FIX GX-02: Status is "Success" because the block producer only includes
        // transactions where TxResult.success == true (see validator block production loop).
        // Failed transactions are dropped from the mempool before block creation.
        // If/when we add receipt storage, this should use the actual execution result.
        "status": "Success",
        "error": serde_json::Value::Null,
        "fee": fee,
        "fee_spores": fee,
        "fee_licn": fee as f64 / 1_000_000_000.0,
        "base_fee_spores": base_fee,
        "priority_fee_spores": priority_fee,
        "compute_units": compute_units,
        "compute_budget": compute_budget,
        "compute_unit_price": compute_unit_price,
        "type": tx_type,
        "from": from,
        "to": to,
        "amount": amount_licn,
        "amount_spores": amount.unwrap_or(0),
        "contract_function": contract_fn,
        "message": {
            "instructions": instructions,
            "recent_blockhash": tx.message.recent_blockhash.to_hex(),
        },
    });

    // Enrich contract-call transactions with token metadata
    if tx_type == "ContractCall" {
        if let Some(ix) = first_ix {
            if let Some((symbol, token_amt, decimals, token_to)) = extract_token_info(store, ix) {
                if let Some(m) = obj.as_object_mut() {
                    m.insert("token_symbol".to_string(), serde_json::json!(symbol));
                    m.insert(
                        "token_amount".to_string(),
                        serde_json::json!(token_amt as f64 / 10f64.powi(decimals as i32)),
                    );
                    m.insert(
                        "token_amount_spores".to_string(),
                        serde_json::json!(token_amt),
                    );
                    m.insert("token_decimals".to_string(), serde_json::json!(decimals));
                    if let Some(ref to_addr) = token_to {
                        m.insert("token_to".to_string(), serde_json::json!(to_addr));
                    }
                }
            }
        }
    }

    obj
}

// ═══════════════════════════════════════════════════════════════════════════════
// SERVER STARTUP & ROUTER
// ═══════════════════════════════════════════════════════════════════════════════

/// Start RPC server
#[allow(clippy::too_many_arguments)]
pub async fn start_rpc_server(
    state: StateStore,
    port: u16,
    tx_sender: Option<mpsc::Sender<Transaction>>,
    stake_pool: Option<Arc<RwLock<StakePool>>>,
    live_validator_set: Option<Arc<RwLock<ValidatorSet>>>,
    p2p: Option<Arc<dyn P2PNetworkTrait>>,
    chain_id: String,
    network_id: String,
    min_validator_stake: u64,
    admin_token: Option<String>,
    finality: Option<FinalityTracker>,
    dex_broadcaster: Option<Arc<dex_ws::DexEventBroadcaster>>,
    prediction_broadcaster: Option<Arc<ws::PredictionEventBroadcaster>>,
    treasury_keypair: Option<TreasuryKeypair>,
) -> Result<(), Box<dyn std::error::Error>> {
    let app = build_rpc_router_internal(
        state,
        tx_sender,
        stake_pool,
        live_validator_set,
        p2p,
        chain_id,
        network_id,
        min_validator_stake,
        admin_token,
        finality,
        dex_broadcaster,
        prediction_broadcaster,
        treasury_keypair,
    );

    let addr = format!("0.0.0.0:{}", port);
    info!("🌐 RPC server starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;

    // STABILITY-FIX: Limit concurrent connections to prevent RPC load from
    // starving block production. 8192 supports 5000+ concurrent traders
    // while still providing backpressure under extreme load.
    use tower::limit::ConcurrencyLimitLayer;
    let app = app.layer(ConcurrencyLimitLayer::new(8192));

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn build_rpc_router(
    state: StateStore,
    tx_sender: Option<mpsc::Sender<Transaction>>,
    stake_pool: Option<Arc<RwLock<StakePool>>>,
    p2p: Option<Arc<dyn P2PNetworkTrait>>,
    chain_id: String,
    network_id: String,
    admin_token: Option<String>,
    finality: Option<FinalityTracker>,
    dex_broadcaster: Option<Arc<dex_ws::DexEventBroadcaster>>,
    prediction_broadcaster: Option<Arc<ws::PredictionEventBroadcaster>>,
    treasury_keypair: Option<TreasuryKeypair>,
) -> Router {
    build_rpc_router_internal(
        state,
        tx_sender,
        stake_pool,
        None,
        p2p,
        chain_id,
        network_id,
        lichen_core::consensus::MIN_VALIDATOR_STAKE,
        admin_token,
        finality,
        dex_broadcaster,
        prediction_broadcaster,
        treasury_keypair,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_rpc_router_with_min_validator_stake(
    state: StateStore,
    tx_sender: Option<mpsc::Sender<Transaction>>,
    stake_pool: Option<Arc<RwLock<StakePool>>>,
    p2p: Option<Arc<dyn P2PNetworkTrait>>,
    chain_id: String,
    network_id: String,
    min_validator_stake: u64,
    admin_token: Option<String>,
    finality: Option<FinalityTracker>,
    dex_broadcaster: Option<Arc<dex_ws::DexEventBroadcaster>>,
    prediction_broadcaster: Option<Arc<ws::PredictionEventBroadcaster>>,
    treasury_keypair: Option<TreasuryKeypair>,
) -> Router {
    build_rpc_router_internal(
        state,
        tx_sender,
        stake_pool,
        None,
        p2p,
        chain_id,
        network_id,
        min_validator_stake,
        admin_token,
        finality,
        dex_broadcaster,
        prediction_broadcaster,
        treasury_keypair,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_rpc_router_internal(
    state: StateStore,
    tx_sender: Option<mpsc::Sender<Transaction>>,
    stake_pool: Option<Arc<RwLock<StakePool>>>,
    live_validator_set: Option<Arc<RwLock<ValidatorSet>>>,
    p2p: Option<Arc<dyn P2PNetworkTrait>>,
    chain_id: String,
    network_id: String,
    min_validator_stake: u64,
    admin_token: Option<String>,
    finality: Option<FinalityTracker>,
    dex_broadcaster: Option<Arc<dex_ws::DexEventBroadcaster>>,
    prediction_broadcaster: Option<Arc<ws::PredictionEventBroadcaster>>,
    treasury_keypair: Option<TreasuryKeypair>,
) -> Router {
    let evm_chain_id = evm_chain_id_from_chain_id(&chain_id);
    let legacy_admin_rpc_enabled = allow_legacy_admin_rpc(&chain_id, &network_id);
    let solana_tx_cache = Arc::new(RwLock::new(LruCache::new(
        NonZeroUsize::new(10_000).unwrap(),
    )));
    // Filter empty admin token to None
    let admin_token = admin_token.filter(|t| !t.is_empty());
    if legacy_admin_rpc_enabled && admin_token.is_some() {
        info!("\u{1f512} Legacy dev-only admin RPC token configured");
    } else if admin_token.is_some() {
        warn!(
            "Ignoring LICHEN_ADMIN_TOKEN on non-local/dev network {} — legacy admin RPCs are disabled",
            network_id
        );
    } else {
        info!(
            "\u{26a0}\u{fe0f}  Legacy admin RPCs disabled unless running in local/dev mode with an admin token"
        );
    }
    let admin_token = Arc::new(std::sync::RwLock::new(admin_token));

    // Spawn background task to hot-reload admin token from LICHEN_ADMIN_TOKEN env var
    {
        let token_ref = Arc::clone(&admin_token);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                if let Ok(new_val) = std::env::var("LICHEN_ADMIN_TOKEN") {
                    let new_token = if new_val.is_empty() {
                        None
                    } else {
                        Some(new_val)
                    };
                    if let Ok(mut guard) = token_ref.write() {
                        if *guard != new_token {
                            info!("Admin token rotated via LICHEN_ADMIN_TOKEN env var");
                            *guard = new_token;
                        }
                    }
                }
            }
        });
    }

    let rpc_state = RpcState {
        state,
        tx_sender,
        p2p,
        stake_pool,
        live_validator_set,
        chain_id,
        network_id,
        min_validator_stake,
        version: env!("CARGO_PKG_VERSION").to_string(),
        evm_chain_id,
        solana_tx_cache,
        admin_token,
        rate_limiter: Arc::new(RateLimiter::new(
            std::env::var("RPC_RATE_LIMIT")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(5_000),
        )),
        finality,
        _dex_broadcaster: dex_broadcaster
            .unwrap_or_else(|| Arc::new(dex_ws::DexEventBroadcaster::new(4096))),
        prediction_broadcaster: prediction_broadcaster
            .unwrap_or_else(|| Arc::new(ws::PredictionEventBroadcaster::new(1024))),
        validator_cache: Arc::new(RwLock::new((
            Instant::now() - std::time::Duration::from_secs(60),
            Vec::new(),
        ))),
        metrics_cache: Arc::new(RwLock::new((
            Instant::now() - std::time::Duration::from_secs(60),
            None,
        ))),
        program_list_response_cache: Arc::new(RwLock::new(LruCache::new(
            NonZeroUsize::new(PROGRAM_LIST_CACHE_MAX_ENTRIES).unwrap(),
        ))),
        airdrop_cooldowns: Arc::new(RwLock::new(AirdropCooldowns::default())),
        orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
        custody_url: std::env::var("CUSTODY_URL").ok().filter(|s| !s.is_empty()),
        custody_auth_token: std::env::var("CUSTODY_API_AUTH_TOKEN")
            .ok()
            .filter(|s| !s.is_empty()),
        incident_status_path: std::env::var("LICHEN_INCIDENT_STATUS_FILE")
            .ok()
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty()),
        signed_metadata_manifest_path: std::env::var("LICHEN_SIGNED_METADATA_MANIFEST_FILE")
            .ok()
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty()),
        signed_metadata_keypair_path: signed_metadata_keypair_path_from_env(),
        signed_metadata_manifest_cache: Arc::new(RwLock::new(None)),
        service_fleet_config_path: std::env::var("LICHEN_SERVICE_FLEET_CONFIG_FILE")
            .ok()
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty()),
        service_fleet_upstream_rpc_url: std::env::var("LICHEN_SERVICE_FLEET_UPSTREAM_RPC_URL")
            .ok()
            .filter(|url| !url.trim().is_empty()),
        service_fleet_status_path: std::env::var("LICHEN_SERVICE_FLEET_STATUS_FILE")
            .ok()
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty()),
        service_fleet_status_cache: Arc::new(RwLock::new((
            Instant::now() - std::time::Duration::from_secs(60),
            None,
        ))),
        treasury_keypair: treasury_keypair.map(Arc::new),
    };

    // D1-01: Configurable CORS origins via LICHEN_CORS_ORIGINS env var
    // (comma-separated).  Defaults to localhost-only + lichen.network subdomains.
    // Set to "*" for development-only wildcard (NOT recommended for production).
    let allowed_hosts: Vec<String> = std::env::var("LICHEN_CORS_ORIGINS")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_else(|| {
            vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "lichen.network".to_string(),
                "www.lichen.network".to_string(),
                "app.lichen.network".to_string(),
                "rpc.lichen.network".to_string(),
                "api.lichen.network".to_string(),
                "explorer.lichen.network".to_string(),
                "dex.lichen.network".to_string(),
                "faucet.lichen.network".to_string(),
                "wallet.lichen.network".to_string(),
                "marketplace.lichen.network".to_string(),
                "programs.lichen.network".to_string(),
                "developers.lichen.network".to_string(),
                "monitoring.lichen.network".to_string(),
                "testnet-rpc.lichen.network".to_string(),
            ]
        });

    // RPC-05: Refuse to start if mainnet with wildcard CORS — prevents
    // accidental open-CORS deployment in production.
    if rpc_state.network_id.contains("mainnet") && allowed_hosts.iter().any(|h| h == "*") {
        eprintln!(
            "FATAL: LICHEN_CORS_ORIGINS contains wildcard '*' on mainnet. \
             Set explicit origins or remove '*'. Aborting."
        );
        std::process::exit(1);
    }

    let allowed_hosts = Arc::new(allowed_hosts);

    // T2.7: Restrictive CORS — allow configured origins only
    // H14 fix: use exact host matching to prevent subdomain bypass
    let cors_hosts = allowed_hosts.clone();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
            let origin_str = origin.to_str().unwrap_or("");
            // Parse scheme://host:port — only allow exact matching hosts
            if let Some(rest) = origin_str
                .strip_prefix("http://")
                .or_else(|| origin_str.strip_prefix("https://"))
            {
                let host = rest.split('/').next().unwrap_or("");
                let host_only = host.split(':').next().unwrap_or("");
                cors_hosts.iter().any(|allowed| allowed == host_only)
            } else {
                false
            }
        }))
        .allow_methods([Method::POST, Method::GET, Method::OPTIONS])
        .allow_headers([axum::http::header::CONTENT_TYPE, axum::http::header::ACCEPT]);

    let state = Arc::new(rpc_state);

    Router::new()
        .route("/", post(handle_rpc))
        .route("/solana-compat", post(handle_solana_rpc))
        .route("/evm", post(handle_evm_rpc))
        // DEX REST API — /api/v1/*
        .nest("/api/v1", dex::build_dex_router())
        // Prediction Market REST API — /api/v1/prediction-market/*
        .nest(
            "/api/v1/prediction-market",
            prediction::build_prediction_router(),
        )
        // SporePump Launchpad REST API — /api/v1/launchpad/*
        .nest("/api/v1/launchpad", launchpad::build_launchpad_router())
        // Shielded Pool REST API — /api/v1/shielded/*
        .nest("/api/v1/shielded", shielded::build_shielded_router())
        .layer(cors)
        // P1-4: HTTP response compression (gzip + brotli)
        // Compresses JSON responses 5-10× for bandwidth savings.
        // Negligible CPU overhead; HTTP/2 negotiated automatically by Axum.
        .layer(CompressionLayer::new())
        // DDoS protection: limit request bodies to 5MB (must accommodate 4MB contract deploys + base64 overhead)
        .layer(axum::extract::DefaultBodyLimit::max(5 * 1024 * 1024))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
        .with_state(state)
}

// ═══════════════════════════════════════════════════════════════════════════════
// RPC DISPATCH HANDLERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Handle RPC request
async fn handle_rpc(
    State(state): State<Arc<RpcState>>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    body: AxumBytes,
) -> Response {
    let probe = match parse_rpc_tier_probe(body.as_ref()) {
        Ok(probe) => probe,
        Err(response) => return response,
    };
    let request_id = probe.id.clone().unwrap_or(serde_json::Value::Null);

    // P9-RPC-03: Tiered rate limiting — classify the method and enforce
    // a per-tier per-IP limit on top of the global rate limit.
    // RPC-M05: tier checks happen before full RpcRequest deserialization.
    let tier = classify_method(&probe.method);
    if tier != MethodTier::Cheap {
        let ip = connect_info
            .map(|ci| ci.0.ip())
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        if !state.rate_limiter.check_tier(ip, tier) {
            let label = match tier {
                MethodTier::Expensive => "expensive",
                MethodTier::Moderate => "moderate",
                MethodTier::Cheap => "cheap",
            };
            warn!(
                "P9-RPC-03: {} method rate limit exceeded for IP {}",
                label, ip
            );
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32005, "message": format!("Rate limit exceeded for {} methods", label)}
                })),
            )
                .into_response();
        }
    }

    let mut req = match parse_rpc_request(body.as_ref(), request_id.clone()) {
        Ok(req) => req,
        Err(response) => return response,
    };

    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    if is_legacy_admin_method(&req.method) {
        if let Err(error) =
            require_legacy_admin_rpc_local_origin(&req.method, connect_info.as_ref())
        {
            return jsonrpc_error_response(
                StatusCode::FORBIDDEN,
                request_id,
                error.code,
                error.message,
            );
        }
        req.params = strip_admin_token_from_params(req.params);
    }

    // Capture auth header as owned String for admin handlers
    let auth_header_owned: Option<String> = auth_header.map(String::from);

    // Route to appropriate handler
    let result = match req.method.as_str() {
        // Basic queries (canonical Lichen endpoints)
        "getBalance" => handle_get_balance(&state, req.params).await,
        "getAccount" => handle_get_account(&state, req.params).await,
        "getAccountAtSlot" => handle_get_account_at_slot(&state, req.params).await,
        "getBlock" => handle_get_block(&state, req.params).await,
        "getBlockCommit" => handle_get_block_commit(&state, req.params).await,
        "getAccountProof" => handle_get_account_proof(&state, req.params).await,
        "getLatestBlock" => handle_get_latest_block(&state).await,
        "getSlot" => handle_get_slot(&state, req.params).await,
        "getTransaction" => handle_get_transaction(&state, req.params).await,
        "getTransactionProof" => handle_get_transaction_proof(&state, req.params).await,
        "getTransactionsByAddress" | "getTransactionHistory" => {
            handle_get_transactions_by_address(&state, req.params).await
        }
        "getAccountTxCount" => handle_get_account_tx_count(&state, req.params).await,
        "getRecentTransactions" => handle_get_recent_transactions(&state, req.params).await,
        "getTokenAccounts" => handle_get_token_accounts(&state, req.params).await,
        "sendTransaction" => handle_send_transaction(&state, req.params).await,
        "confirmTransaction" => handle_confirm_transaction(&state, req.params).await,
        "simulateTransaction" => handle_simulate_transaction(&state, req.params).await,
        "callContract" => handle_call_contract(&state, req.params).await,
        "getTotalBurned" => handle_get_total_burned(&state).await,
        "getValidators" => handle_get_validators(&state).await,
        "getMetrics" => handle_get_metrics(&state).await,
        "getIncidentStatus" => handle_get_incident_status(&state).await,
        "getSignedMetadataManifest" => handle_get_signed_metadata_manifest(&state).await,
        "getServiceFleetStatus" => handle_get_service_fleet_status(&state).await,
        "getTreasuryInfo" => handle_get_treasury_info(&state).await,
        "getGenesisAccounts" => handle_get_genesis_accounts(&state).await,
        "getGovernedProposal" => handle_get_governed_proposal(&state, req.params).await,
        "getRecentBlockhash" => handle_get_recent_blockhash(&state).await,
        "health" | "getHealth" => {
            // GX-07: Check block staleness — return 503-equivalent if stalled
            let slot = state.state.get_last_slot().unwrap_or(0);
            let stale = if let Ok(Some(block)) = state.state.get_block_by_slot(slot) {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let age = now.saturating_sub(block.header.timestamp);
                age > 120 // stale if no block in 2 minutes
            } else {
                slot == 0 // stale if no blocks at all
            };
            if stale {
                Ok(serde_json::json!({"status": "behind", "slot": slot}))
            } else {
                Ok(serde_json::json!({"status": "ok", "slot": slot}))
            }
        }

        // Fee and rent config endpoints
        "getFeeConfig" => handle_get_fee_config(&state).await,
        "setFeeConfig" => {
            handle_set_fee_config(&state, req.params, auth_header_owned.as_deref()).await
        }
        "estimateTransactionFee" => handle_estimate_transaction_fee(&state, req.params).await,
        "getRentParams" => handle_get_rent_params(&state).await,
        "setRentParams" => {
            handle_set_rent_params(&state, req.params, auth_header_owned.as_deref()).await
        }

        // Network endpoints
        "getPeers" => handle_get_peers(&state).await,
        "getNetworkInfo" => handle_get_network_info(&state).await,
        "getClusterInfo" => handle_get_cluster_info(&state).await,

        // Validator endpoints
        "getValidatorInfo" => handle_get_validator_info(&state, req.params).await,
        "getValidatorPerformance" => handle_get_validator_performance(&state, req.params).await,
        "getChainStatus" => handle_get_chain_status(&state).await,

        // Staking endpoints
        "stake" => handle_stake(&state, req.params).await,
        "unstake" => handle_unstake(&state, req.params).await,
        "getStakingStatus" => handle_get_staking_status(&state, req.params).await,
        "getStakingRewards" => handle_get_staking_rewards(&state, req.params).await,

        // MossStake liquid staking query endpoints
        "getStakingPosition" => handle_get_staking_position(&state, req.params).await,
        "getMossStakePoolInfo" => handle_get_mossstake_pool_info(&state).await,
        "getUnstakingQueue" => handle_get_unstaking_queue(&state, req.params).await,

        // Price-based rewards
        "getRewardAdjustmentInfo" => handle_get_reward_adjustment_info(&state).await,

        // Account endpoints
        "getAccountInfo" => handle_get_account_info(&state, req.params).await,

        // Contract endpoints
        "getContractInfo" => handle_get_contract_info(&state, req.params).await,
        "getContractLogs" => handle_get_contract_logs(&state, req.params).await,
        "getContractAbi" => handle_get_contract_abi(&state, req.params).await,
        "setContractAbi" => {
            handle_set_contract_abi(&state, req.params, auth_header_owned.as_deref()).await
        }
        "getAllContracts" => handle_get_all_contracts(&state, req.params).await,
        "deployContract" => {
            handle_deploy_contract(&state, req.params, auth_header_owned.as_deref()).await
        }
        "upgradeContract" => {
            handle_upgrade_contract(&state, req.params, auth_header_owned.as_deref()).await
        }

        // Program endpoints (draft)
        "getProgram" => handle_get_program(&state, req.params).await,
        "getProgramStats" => handle_get_program_stats(&state, req.params).await,
        "getPrograms" => handle_get_programs(&state, req.params).await,
        "getProgramCalls" => handle_get_program_calls(&state, req.params).await,
        "getProgramStorage" => handle_get_program_storage(&state, req.params).await,

        // LichenID endpoints
        "getLichenIdIdentity" => handle_get_lichenid_identity(&state, req.params).await,
        "getLichenIdReputation" => handle_get_lichenid_reputation(&state, req.params).await,
        "getLichenIdSkills" => handle_get_lichenid_skills(&state, req.params).await,
        "getLichenIdVouches" => handle_get_lichenid_vouches(&state, req.params).await,
        "getLichenIdAchievements" => handle_get_lichenid_achievements(&state, req.params).await,
        "getLichenIdProfile" => handle_get_lichenid_profile(&state, req.params).await,
        "resolveLichenName" => handle_resolve_licn_name(&state, req.params).await,
        "reverseLichenName" => handle_reverse_licn_name(&state, req.params).await,
        "batchReverseLichenNames" => handle_batch_reverse_licn_names(&state, req.params).await,
        "searchLichenNames" => handle_search_licn_names(&state, req.params).await,
        "getLichenIdAgentDirectory" => {
            handle_get_lichenid_agent_directory(&state, req.params).await
        }
        "getLichenIdStats" => {
            let p = None;
            if let Some(cached) =
                get_cached_program_list_response(&state, "getLichenIdStats", &p).await
            {
                Ok(cached)
            } else {
                match handle_get_lichenid_stats(&state).await {
                    Ok(resp) => {
                        put_cached_program_list_response(
                            &state,
                            "getLichenIdStats",
                            &p,
                            resp.clone(),
                        )
                        .await;
                        Ok(resp)
                    }
                    Err(e) => Err(e),
                }
            }
        }
        "getNameAuction" => handle_get_name_auction(&state, req.params).await,

        // EVM address registry
        "getEvmRegistration" => handle_get_evm_registration(&state, req.params).await,
        "lookupEvmAddress" => handle_lookup_evm_address(&state, req.params).await,

        // Symbol registry
        "getSymbolRegistry" => handle_get_symbol_registry(&state, req.params).await,
        "getSymbolRegistryByProgram" => {
            handle_get_symbol_registry_by_program(&state, req.params).await
        }
        "getAllSymbolRegistry" | "getAllSymbols" => {
            handle_get_all_symbol_registry(&state, req.params).await
        }

        // NFT endpoints (draft)
        "getCollection" => handle_get_collection(&state, req.params).await,
        "getNFT" => handle_get_nft(&state, req.params).await,
        "getNFTsByOwner" => handle_get_nfts_by_owner(&state, req.params).await,
        "getNFTsByCollection" => handle_get_nfts_by_collection(&state, req.params).await,
        "getNFTActivity" => handle_get_nft_activity(&state, req.params).await,
        "getMarketListings" => handle_get_market_listings(&state, req.params).await,
        "getMarketSales" => handle_get_market_sales(&state, req.params).await,
        "getMarketOffers" => handle_get_market_offers(&state, req.params).await,
        "getMarketAuctions" => handle_get_market_auctions(&state, req.params).await,

        // Token endpoints
        "getTokenBalance" => handle_get_token_balance(&state, req.params).await,
        "getTokenHolders" => handle_get_token_holders(&state, req.params).await,
        "getTokenTransfers" => handle_get_token_transfers(&state, req.params).await,
        "getContractEvents" => handle_get_contract_events(&state, req.params).await,
        "getGovernanceEvents" => handle_get_governance_events(&state, req.params).await,

        // Testnet-only faucet airdrop
        "requestAirdrop" => handle_request_airdrop(&state, req.params).await,

        // Prediction Market endpoints
        "getPredictionMarketStats" => handle_get_prediction_stats(&state).await,
        "getPredictionMarkets" => {
            if let Some(cached) =
                get_cached_program_list_response(&state, "getPredictionMarkets", &req.params).await
            {
                Ok(cached)
            } else {
                match handle_get_prediction_markets(&state, req.params.clone()).await {
                    Ok(resp) => {
                        put_cached_program_list_response(
                            &state,
                            "getPredictionMarkets",
                            &req.params,
                            resp.clone(),
                        )
                        .await;
                        Ok(resp)
                    }
                    Err(e) => Err(e),
                }
            }
        }
        "getPredictionMarket" => handle_get_prediction_market(&state, req.params).await,
        "getPredictionPositions" => handle_get_prediction_positions(&state, req.params).await,
        "getPredictionTraderStats" => handle_get_prediction_trader_stats(&state, req.params).await,
        "getPredictionLeaderboard" => handle_get_prediction_leaderboard(&state, req.params).await,
        "getPredictionTrending" => handle_get_prediction_trending(&state).await,
        "getPredictionMarketAnalytics" => {
            handle_get_prediction_market_analytics(&state, req.params).await
        }

        // DEX & Platform Stats endpoints
        "getDexCoreStats" => handle_get_dex_core_stats(&state).await,
        "getDexAmmStats" => handle_get_dex_amm_stats(&state).await,
        "getDexMarginStats" => handle_get_dex_margin_stats(&state).await,
        "getDexRewardsStats" => handle_get_dex_rewards_stats(&state).await,
        "getDexRouterStats" => handle_get_dex_router_stats(&state).await,
        "getDexAnalyticsStats" => handle_get_dex_analytics_stats(&state).await,
        "getDexGovernanceStats" => handle_get_dex_governance_stats(&state).await,
        "getLichenSwapStats" => handle_get_lichenswap_stats(&state).await,
        "getThallLendStats" => handle_get_thalllend_stats(&state).await,
        "getSporePayStats" => handle_get_sporepay_stats(&state).await,
        "getSporePumpStats" => launchpad::handle_get_sporepump_stats(&state).await,
        "getBountyBoardStats" => handle_get_bountyboard_stats(&state).await,
        "getComputeMarketStats" => handle_get_compute_market_stats(&state).await,
        "getMossStorageStats" => handle_get_moss_storage_stats(&state).await,
        "getLichenMarketStats" => handle_get_lichenmarket_stats(&state).await,
        "getLichenAuctionStats" => handle_get_lichenauction_stats(&state).await,
        "getLichenPunksStats" => handle_get_lichenpunks_stats(&state).await,
        // Token contract stats
        "getLusdStats" | "getMusdStats" => handle_get_musd_stats(&state).await,
        "getWethStats" => handle_get_weth_stats(&state).await,
        "getWsolStats" => handle_get_wsol_stats(&state).await,
        "getWbnbStats" => handle_get_wbnb_stats(&state).await,
        // Platform contract stats — previously missing RPC wiring
        "getSporeVaultStats" => handle_get_sporevault_stats(&state).await,
        "getLichenBridgeStats" => handle_get_lichenbridge_stats(&state).await,
        "createBridgeDeposit" => handle_create_bridge_deposit(&state, req.params).await,
        "getBridgeDeposit" => handle_get_bridge_deposit(&state, req.params).await,
        "getLichenDaoStats" => handle_get_lichendao_stats(&state).await,
        "getLichenOracleStats" => handle_get_lichenoracle_stats(&state).await,

        // ── Wallet price-feed methods ───────────────────────────────
        "getDexPairs" => handle_get_dex_pairs(&state).await,
        "getOraclePrices" => handle_get_oracle_prices(&state).await,

        // ── Shielded Pool (ZK Privacy) ──────────────────────────────
        "getShieldedPoolState" => {
            shielded::handle_get_shielded_pool_state(&state, req.params).await
        }
        "getShieldedPoolStats" => {
            shielded::handle_get_shielded_pool_stats(&state, req.params).await
        }
        "getShieldedMerkleRoot" => {
            shielded::handle_get_shielded_merkle_root(&state, req.params).await
        }
        "getShieldedMerklePath" => {
            shielded::handle_get_shielded_merkle_path(&state, req.params).await
        }
        "isNullifierSpent" => shielded::handle_is_nullifier_spent(&state, req.params).await,
        "checkNullifier" => shielded::handle_is_nullifier_spent(&state, req.params).await,
        "getShieldedCommitments" => {
            shielded::handle_get_shielded_commitments(&state, req.params).await
        }
        "computeShieldCommitment" => {
            shielded::handle_compute_shield_commitment(&state, req.params).await
        }
        "generateShieldProof" => shielded::handle_generate_shield_proof(&state, req.params).await,
        "generateUnshieldProof" => {
            shielded::handle_generate_unshield_proof(&state, req.params).await
        }
        "generateTransferProof" => {
            shielded::handle_generate_transfer_proof(&state, req.params).await
        }

        _ => Err(method_not_found_error()),
    };

    let response = match result {
        Ok(result) => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(result),
            error: None,
        },
        Err(error) => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: None,
            error: Some(sanitize_rpc_error(error)),
        },
    };

    encode_rpc_response(&headers, response)
}

/// Handle Solana-compatible RPC request
async fn handle_solana_rpc(
    State(state): State<Arc<RpcState>>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    body: AxumBytes,
) -> Response {
    let probe = match parse_rpc_tier_probe(body.as_ref()) {
        Ok(probe) => probe,
        Err(response) => return response,
    };
    let request_id = probe.id.clone().unwrap_or(serde_json::Value::Null);

    // P9-RPC-03: Tiered rate limiting for Solana-compat methods
    // RPC-M05: tier checks happen before full RpcRequest deserialization.
    let tier = classify_solana_method_tier(&probe.method);
    if tier != MethodTier::Cheap {
        let ip = connect_info
            .map(|ci| ci.0.ip())
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        if !state.rate_limiter.check_tier(ip, tier) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32005, "message": "Rate limit exceeded"}
                })),
            )
                .into_response();
        }
    }

    let req = match parse_rpc_request(body.as_ref(), request_id) {
        Ok(req) => req,
        Err(response) => return response,
    };

    let result = match req.method.as_str() {
        "getLatestBlockhash" => handle_solana_get_latest_blockhash(&state).await,
        "getRecentBlockhash" => handle_solana_get_latest_blockhash(&state).await,
        "getBalance" => handle_solana_get_balance(&state, req.params).await,
        "getAccountInfo" => handle_solana_get_account_info(&state, req.params).await,
        "getBlock" => handle_solana_get_block(&state, req.params).await,
        "getBlockHeight" => handle_solana_get_block_height(&state).await,
        "getSignaturesForAddress" => {
            handle_solana_get_signatures_for_address(&state, req.params).await
        }
        "getSignatureStatuses" => handle_solana_get_signature_statuses(&state, req.params).await,
        "getSlot" => handle_solana_get_slot(&state).await,
        "getTransaction" => handle_solana_get_transaction(&state, req.params).await,
        "sendTransaction" => handle_solana_send_transaction(&state, req.params).await,
        "getTokenAccountsByOwner" => {
            handle_solana_get_token_accounts_by_owner(&state, req.params).await
        }
        "getTokenAccountBalance" => {
            handle_solana_get_token_account_balance(&state, req.params).await
        }
        "getHealth" => {
            let slot = state.state.get_last_slot().unwrap_or(0);
            let stale = if let Ok(Some(block)) = state.state.get_block_by_slot(slot) {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let age = now.saturating_sub(block.header.timestamp);
                age > 120
            } else {
                slot == 0
            };
            if stale {
                Ok(serde_json::json!({"status": "behind", "slot": slot}))
            } else {
                Ok(serde_json::json!({"status": "ok", "slot": slot}))
            }
        }
        "getVersion" => Ok(
            serde_json::json!({"solana-core": format!("lichen-{}", state.version), "feature-set": 0}),
        ),
        _ => Err(RpcError {
            code: -32601,
            message: format!(
                "Method '{}' is not supported in the Lichen Solana compatibility layer. \
                 Supported: getLatestBlockhash, getRecentBlockhash, getBalance, \
                 getAccountInfo, getBlock, getBlockHeight, getSignaturesForAddress, \
                 getSignatureStatuses, getSlot, getTransaction, sendTransaction, \
                 getTokenAccountsByOwner, getTokenAccountBalance, getHealth, getVersion",
                req.method
            ),
        }),
    };

    let response = match result {
        Ok(result) => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(result),
            error: None,
        },
        Err(error) => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: None,
            error: Some(sanitize_rpc_error(error)),
        },
    };

    encode_rpc_response(&headers, response)
}

/// Handle Ethereum-compatible RPC request
async fn handle_evm_rpc(
    State(state): State<Arc<RpcState>>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    body: AxumBytes,
) -> Response {
    let probe = match parse_rpc_tier_probe(body.as_ref()) {
        Ok(probe) => probe,
        Err(response) => return response,
    };
    let request_id = probe.id.clone().unwrap_or(serde_json::Value::Null);

    // P9-RPC-03: Tiered rate limiting for EVM-compat methods
    // RPC-M05: tier checks happen before full RpcRequest deserialization.
    let tier = match probe.method.as_str() {
        "eth_sendRawTransaction" | "eth_call" | "eth_estimateGas" => MethodTier::Expensive,
        "eth_getLogs" => MethodTier::Moderate,
        _ => MethodTier::Cheap,
    };
    if tier != MethodTier::Cheap {
        let ip = connect_info
            .map(|ci| ci.0.ip())
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        if !state.rate_limiter.check_tier(ip, tier) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32005, "message": "Rate limit exceeded"}
                })),
            )
                .into_response();
        }
    }

    let req = match parse_rpc_request(body.as_ref(), request_id) {
        Ok(req) => req,
        Err(response) => return response,
    };

    let result = match req.method.as_str() {
        "eth_getBalance" => handle_eth_get_balance(&state, req.params).await,
        "eth_sendRawTransaction" => handle_eth_send_raw_transaction(&state, req.params).await,
        "eth_call" => handle_eth_call(&state, req.params).await,
        "eth_chainId" => Ok(serde_json::json!(format!("0x{:x}", state.evm_chain_id))),
        "eth_blockNumber" => handle_eth_block_number(&state).await,
        "eth_getTransactionReceipt" => handle_eth_get_transaction_receipt(&state, req.params).await,
        "eth_getTransactionByHash" => handle_eth_get_transaction_by_hash(&state, req.params).await,
        "eth_accounts" => Ok(serde_json::json!([])), // No accounts (users use MetaMask)
        "net_version" => Ok(serde_json::json!("1297368660")), // "Lichen" as decimal
        "eth_gasPrice" => handle_eth_gas_price(&state).await,
        "eth_maxPriorityFeePerGas" => Ok(serde_json::json!("0x0")), // No priority fees in Lichen
        "eth_estimateGas" => handle_eth_estimate_gas(&state, req.params).await,
        "eth_getCode" => handle_eth_get_code(&state, req.params).await,
        "eth_getTransactionCount" => handle_eth_get_transaction_count(&state, req.params).await,
        "eth_getBlockByNumber" => handle_eth_get_block_by_number(&state, req.params).await,
        "eth_getBlockByHash" => handle_eth_get_block_by_hash(&state, req.params).await,
        "eth_getLogs" => handle_eth_get_logs(&state, req.params).await,
        "eth_getStorageAt" => handle_eth_get_storage_at(&state, req.params).await,
        "net_listening" => Ok(serde_json::json!(true)),
        "web3_clientVersion" => Ok(serde_json::json!(format!("Lichen/{}", state.version))),
        _ => Err(method_not_found_error()),
    };

    let response = match result {
        Ok(result) => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(result),
            error: None,
        },
        Err(error) => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: None,
            error: Some(sanitize_rpc_error(error)),
        },
    };

    encode_rpc_response(&headers, response)
}

// ═══════════════════════════════════════════════════════════════════════════════
// NATIVE LICN RPC METHODS
// ═══════════════════════════════════════════════════════════════════════════════

/// getEvmRegistration — check if a native address has an EVM address registered on-chain.
/// Params: [nativePubkey (base58)]
/// Returns: { "evmAddress": "0x..." } or null
async fn handle_get_evm_registration(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let native_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [nativePubkey]".to_string(),
        })?;

    let native_pubkey = Pubkey::from_base58(native_str).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid pubkey format".to_string(),
    })?;

    let evm = state
        .state
        .lookup_native_to_evm(&native_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: e,
        })?;

    match evm {
        Some(evm_bytes) => {
            let hex: String = evm_bytes.iter().map(|b| format!("{:02x}", b)).collect();
            Ok(serde_json::json!({ "evmAddress": format!("0x{}", hex) }))
        }
        None => Ok(serde_json::Value::Null),
    }
}

/// lookupEvmAddress — resolve an EVM address to native pubkey.
/// Params: [evmAddress (hex with or without 0x)]
/// Returns: { "nativePubkey": "..." } or null
async fn handle_lookup_evm_address(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let evm_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [evmAddress]".to_string(),
        })?;

    let evm_bytes = StateStore::parse_evm_address(evm_str).map_err(|e| RpcError {
        code: -32602,
        message: e,
    })?;

    let native = state
        .state
        .lookup_evm_address(&evm_bytes)
        .map_err(|e| RpcError {
            code: -32000,
            message: e,
        })?;

    match native {
        Some(pubkey) => Ok(serde_json::json!({ "nativePubkey": pubkey.to_base58() })),
        None => Ok(serde_json::Value::Null),
    }
}

async fn handle_get_symbol_registry(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let symbol = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [symbol]".to_string(),
        })?;

    let entry = state
        .state
        .get_symbol_registry(symbol)
        .map_err(|e| RpcError {
            code: -32000,
            message: e,
        })?;

    Ok(entry
        .map(symbol_registry_entry_to_json)
        .unwrap_or(serde_json::Value::Null))
}

async fn handle_get_symbol_registry_by_program(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let program = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [program_id]".to_string(),
        })?;

    let program = Pubkey::from_base58(program).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid pubkey format".to_string(),
    })?;

    let entry = state
        .state
        .get_symbol_registry_by_program(&program)
        .map_err(|e| RpcError {
            code: -32000,
            message: e,
        })?;

    Ok(entry
        .map(symbol_registry_entry_to_json)
        .unwrap_or(serde_json::Value::Null))
}

async fn handle_get_all_symbol_registry(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    if let Some(cached) =
        get_cached_program_list_response(state, "getAllSymbolRegistry", &params).await
    {
        return Ok(cached);
    }

    let mut limit = 500u64;
    let mut cursor: Option<String> = None;

    if let Some(val) = params.as_ref() {
        if let Some(arr) = val.as_array() {
            if let Some(first) = arr.first() {
                if let Some(v) = first.as_u64() {
                    limit = v;
                } else if let Some(s) = first.as_str() {
                    cursor = Some(s.to_string());
                } else if let Some(obj) = first.as_object() {
                    if let Some(v) = obj.get("limit").and_then(|v| v.as_u64()) {
                        limit = v;
                    }
                    cursor = obj
                        .get("cursor")
                        .and_then(|v| v.as_str())
                        .or_else(|| obj.get("after").and_then(|v| v.as_str()))
                        .or_else(|| obj.get("after_symbol").and_then(|v| v.as_str()))
                        .map(|s| s.to_string());
                }
            }
            if let Some(second) = arr.get(1) {
                if let Some(obj) = second.as_object() {
                    if let Some(v) = obj.get("limit").and_then(|v| v.as_u64()) {
                        limit = v;
                    }
                    if cursor.is_none() {
                        cursor = obj
                            .get("cursor")
                            .and_then(|v| v.as_str())
                            .or_else(|| obj.get("after").and_then(|v| v.as_str()))
                            .or_else(|| obj.get("after_symbol").and_then(|v| v.as_str()))
                            .map(|s| s.to_string());
                    }
                } else if cursor.is_none() {
                    if let Some(s) = second.as_str() {
                        cursor = Some(s.to_string());
                    }
                }
            }
        } else if let Some(obj) = val.as_object() {
            if let Some(v) = obj.get("limit").and_then(|v| v.as_u64()) {
                limit = v;
            }
            cursor = obj
                .get("cursor")
                .and_then(|v| v.as_str())
                .or_else(|| obj.get("after").and_then(|v| v.as_str()))
                .or_else(|| obj.get("after_symbol").and_then(|v| v.as_str()))
                .map(|s| s.to_string());
        } else if let Some(v) = val.as_u64() {
            limit = v;
        } else if let Some(s) = val.as_str() {
            cursor = Some(s.to_string());
        }
    }

    let limit = limit.clamp(1, 2000) as usize;
    let fetch_limit = limit.saturating_add(1);

    let mut entries = state
        .state
        .get_all_symbol_registry_paginated(fetch_limit, cursor.as_deref())
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let has_more = entries.len() > limit;
    if has_more {
        entries.truncate(limit);
    }

    let next_cursor = if has_more {
        entries.last().map(|entry| entry.symbol.clone())
    } else {
        None
    };

    let list: Vec<serde_json::Value> = entries
        .into_iter()
        .map(symbol_registry_entry_to_json)
        .collect();

    let response = serde_json::json!({
        "entries": list,
        "count": list.len(),
        "has_more": has_more,
        "next_cursor": next_cursor,
    });

    put_cached_program_list_response(state, "getAllSymbolRegistry", &params, response.clone())
        .await;

    Ok(response)
}

fn symbol_registry_entry_to_json(entry: SymbolRegistryEntry) -> serde_json::Value {
    serde_json::json!({
        "symbol": entry.symbol,
        "program": entry.program.to_base58(),
        "owner": entry.owner.to_base58(),
        "name": entry.name,
        "template": entry.template,
        "metadata": entry.metadata,
        "decimals": entry.decimals,
    })
}

/// Get balance
async fn handle_get_balance(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|_| invalid_pubkey_format_error())?;

    let account = state.state.get_account(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match account {
        Some(acc) => {
            // Convert helper for spores to LICN (with precision)
            let to_licn_str =
                |spores: u64| -> String { format!("{:.4}", spores as f64 / 1_000_000_000.0) };

            // Include MossStake liquid staking position
            let (moss_staked, moss_value) = state
                .state
                .get_mossstake_pool()
                .ok()
                .and_then(|pool| {
                    pool.positions.get(&pubkey).map(|p| {
                        (
                            p.licn_deposited,
                            pool.st_licn_token.st_licn_to_licn(p.st_licn_amount),
                        )
                    })
                })
                .unwrap_or((0, 0));

            Ok(serde_json::json!({
                // Total balance (backward compatible)
                "spores": acc.spores,
                "licn": to_licn_str(acc.spores),

                // Balance breakdown (NEW)
                "spendable": acc.spendable,
                "spendable_licn": to_licn_str(acc.spendable),

                "staked": acc.staked,
                "staked_licn": to_licn_str(acc.staked),

                "locked": acc.locked,
                "locked_licn": to_licn_str(acc.locked),

                // MossStake liquid staking (separate from native validator staking)
                "moss_staked": moss_staked,
                "moss_staked_licn": to_licn_str(moss_staked),
                "moss_value": moss_value,
                "moss_value_licn": to_licn_str(moss_value),
            }))
        }
        None => {
            // Account doesn't exist - return all zeros
            Ok(serde_json::json!({
                "spores": 0,
                "licn": "0.0000",
                "spendable": 0,
                "spendable_licn": "0.0000",
                "staked": 0,
                "staked_licn": "0.0000",
                "locked": 0,
                "locked_licn": "0.0000",
                "moss_staked": 0,
                "moss_staked_licn": "0.0000",
                "moss_value": 0,
                "moss_value_licn": "0.0000",
            }))
        }
    }
}

/// Get account
async fn handle_get_account(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|_| invalid_pubkey_format_error())?;

    let account = state.state.get_account(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match account {
        Some(acc) => {
            let to_licn_str =
                |spores: u64| -> String { format!("{:.4}", spores as f64 / 1_000_000_000.0) };

            Ok(serde_json::json!({
                "pubkey": pubkey.to_base58(),
                "evm_address": pubkey.to_evm(),
                "spores": acc.spores,
                "licn": to_licn_str(acc.spores),
                "spendable": acc.spendable,
                "spendable_licn": to_licn_str(acc.spendable),
                "staked": acc.staked,
                "staked_licn": to_licn_str(acc.staked),
                "locked": acc.locked,
                "locked_licn": to_licn_str(acc.locked),
                "owner": acc.owner.to_base58(),
                "executable": acc.executable,
                "data_len": acc.data.len(),
            }))
        }
        None => Err(RpcError {
            code: -32001,
            message: "Account not found".to_string(),
        }),
    }
}

/// Get historical account state at a specific slot (Task 3.9: Archive Mode)
async fn handle_get_account_at_slot(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [pubkey, slot]".to_string(),
    })?;

    let pubkey_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected pubkey as first argument".to_string(),
        })?;

    let target_slot = arr
        .get(1)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected slot number as second argument".to_string(),
        })?;

    let pubkey = lichen_core::account::Pubkey::from_base58(pubkey_str)
        .map_err(|_| invalid_pubkey_format_error())?;

    if !state.state.is_archive_mode() {
        return Err(RpcError {
            code: -32003,
            message: "Archive mode is not enabled on this node".to_string(),
        });
    }

    let account = state
        .state
        .get_account_at_slot(&pubkey, target_slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    match account {
        Some(acc) => {
            let to_licn_str =
                |spores: u64| -> String { format!("{:.4}", spores as f64 / 1_000_000_000.0) };

            Ok(serde_json::json!({
                "pubkey": pubkey.to_base58(),
                "slot": target_slot,
                "spores": acc.spores,
                "licn": to_licn_str(acc.spores),
                "spendable": acc.spendable,
                "spendable_licn": to_licn_str(acc.spendable),
                "staked": acc.staked,
                "staked_licn": to_licn_str(acc.staked),
                "locked": acc.locked,
                "locked_licn": to_licn_str(acc.locked),
                "owner": acc.owner.to_base58(),
                "executable": acc.executable,
                "data_len": acc.data.len(),
            }))
        }
        None => Err(RpcError {
            code: -32001,
            message: format!(
                "No snapshot found for account {} at or before slot {}",
                pubkey_str, target_slot
            ),
        }),
    }
}

/// Get block
async fn handle_get_block(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let slot = parse_get_block_slot_param(params.as_ref(), false)?;

    let block = state.state.get_block_by_slot(slot).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match block {
        Some(block) => {
            let fee_config = state
                .state
                .get_fee_config()
                .unwrap_or_else(|_| lichen_core::FeeConfig::default_from_constants());
            let block_hash = block.hash();
            let transactions: Vec<serde_json::Value> = block
                .transactions
                .iter()
                .map(|tx| {
                    let cu = state.state.get_tx_meta_cu(&tx.signature()).ok().flatten();
                    tx_to_rpc_json(
                        tx,
                        block.header.slot,
                        block.header.timestamp,
                        &fee_config,
                        cu,
                        &state.state,
                    )
                })
                .collect();

            // Protocol-level block reward — epoch-based inflation model
            // Actual rewards are distributed at epoch boundaries to ALL stakers
            // proportionally, NOT per-block to the producer. The per-slot rate
            // is included as a projection for APY calculations.
            let has_user_txs = block.transactions.iter().any(|tx| {
                tx.message
                    .instructions
                    .first()
                    .map(|ix| !matches!(ix.data.first(), Some(2) | Some(3)))
                    .unwrap_or(true)
            });
            let projected_per_slot =
                if block.header.slot == 0 || block.header.validator == [0u8; 32] {
                    0
                } else {
                    let total_supply = GENESIS_SUPPLY_SPORES
                        .saturating_add(state.state.get_total_minted().unwrap_or(0))
                        .saturating_sub(state.state.get_total_burned().unwrap_or(0));
                    compute_block_reward(block.header.slot, total_supply)
                };

            let current_epoch = lichen_core::consensus::slot_to_epoch(block.header.slot);

            Ok(serde_json::json!({
                "slot": block.header.slot,
                "hash": block_hash.to_hex(),
                "commit_round": block.commit_round,
                "parent_hash": block.header.parent_hash.to_hex(),
                "state_root": block.header.state_root.to_hex(),
                "tx_root": block.header.tx_root.to_hex(),
                "timestamp": block.header.timestamp,
                "validator": Pubkey(block.header.validator).to_base58(),
                "transaction_count": block.transactions.len(),
                "transactions": transactions,
                "block_reward": {
                    "amount": 0,
                    "amount_licn": 0.0,
                    "projected_per_slot": projected_per_slot,
                    "projected_per_slot_licn": projected_per_slot as f64 / 1_000_000_000.0,
                    "distribution": "epoch",
                    "epoch": current_epoch,
                    "type": if has_user_txs { "transaction" } else { "heartbeat" },
                    "recipient": Pubkey(block.header.validator).to_base58(),
                },
                "commit_signatures": block.commit_signatures.iter().map(|cs| {
                    serde_json::json!({
                        "validator": Pubkey(cs.validator).to_base58(),
                        "signature": pq_signature_json(&cs.signature),
                    })
                }).collect::<Vec<_>>(),
                "commit_validator_count": block.commit_signatures.len(),
            }))
        }
        None => Err(RpcError {
            code: -32001,
            message: "Block not found".to_string(),
        }),
    }
}

/// Get block commit certificate (commit signatures only)
async fn handle_get_block_commit(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let slot = parse_get_block_slot_param(params.as_ref(), false)?;

    let block = state.state.get_block_by_slot(slot).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match block {
        Some(block) => {
            let block_hash = block.hash();
            let sigs: Vec<serde_json::Value> = block
                .commit_signatures
                .iter()
                .map(|cs| {
                    serde_json::json!({
                        "validator": Pubkey(cs.validator).to_base58(),
                        "signature": pq_signature_json(&cs.signature),
                        "timestamp": cs.timestamp,
                    })
                })
                .collect();

            Ok(serde_json::json!({
                "slot": block.header.slot,
                "block_hash": block_hash.to_hex(),
                "commit_round": block.commit_round,
                "commit_signatures": sigs,
                "commit_validator_count": sigs.len(),
                "bft_timestamp": block.header.timestamp,
            }))
        }
        None => Err(RpcError {
            code: -32001,
            message: "Block not found".to_string(),
        }),
    }
}

/// Get a native anchored Merkle inclusion proof for an account.
///
/// Params: [pubkey_base58, {commitment?}] or { pubkey, commitment? }
/// Returns: {
///   pubkey,
///   account_data,
///   inclusion_proof: { leaf_hash, siblings, path },
///   anchor: { slot, commitment, block_hash, parent_hash, state_root, tx_root,
///             validators_hash, timestamp, validator, block_signature,
///             commit_signatures, commit_validator_count }
/// }
async fn handle_get_account_proof(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params_ref = params.as_ref();
    let pubkey_str = params
        .as_ref()
        .and_then(|p| p.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .or_else(|| {
            params
                .as_ref()
                .and_then(|p| p.as_object())
                .and_then(|o| o.get("pubkey"))
                .and_then(|v| v.as_str())
        })
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing pubkey parameter".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid pubkey".to_string(),
    })?;

    let proof = state
        .state
        .get_account_proof(&pubkey)
        .ok_or_else(|| RpcError {
            code: -32001,
            message: "Account not found or proof unavailable".to_string(),
        })?;

    let requested_commitment = params_ref
        .and_then(|p| p.as_array())
        .and_then(|arr| arr.get(1))
        .and_then(|v| v.get("commitment"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            params_ref
                .and_then(|p| p.as_object())
                .and_then(|o| o.get("commitment"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("finalized");

    let (anchor_slot, anchor_block, anchor_context) =
        anchored_block_context(state, requested_commitment)?;

    if anchor_block.header.state_root != proof.state_root {
        return Err(RpcError {
            code: -32001,
            message: format!(
                "Account proof is not anchored to the {} block at slot {}",
                requested_commitment, anchor_slot
            ),
        });
    }

    let siblings_hex: Vec<String> = proof.proof.siblings.iter().map(|h| h.to_hex()).collect();

    Ok(serde_json::json!({
        "pubkey": pubkey_str,
        "account_data": hex::encode(&proof.account_data),
        "inclusion_proof": {
            "leaf_hash": proof.proof.leaf_hash.to_hex(),
            "siblings": siblings_hex,
            "path": proof.proof.path,
        },
        "anchor": anchor_context,
    }))
}

/// Get current slot (supports optional commitment level)
async fn handle_get_slot(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let commitment = params
        .as_ref()
        .and_then(|p| p.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .or_else(|| {
            params
                .as_ref()
                .and_then(|p| p.as_object())
                .and_then(|o| o.get("commitment"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("processed");

    let slot = match commitment {
        "finalized" => {
            if let Some(ref ft) = state.finality {
                ft.finalized_slot()
            } else {
                state.state.get_last_finalized_slot().unwrap_or(0)
            }
        }
        "confirmed" => {
            if let Some(ref ft) = state.finality {
                ft.confirmed_slot()
            } else {
                state.state.get_last_confirmed_slot().unwrap_or(0)
            }
        }
        _ => state.state.get_last_slot().map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?,
    };

    Ok(serde_json::json!(slot))
}

/// Get recent blockhash for transaction building
async fn handle_get_recent_blockhash(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    if slot == 0 {
        return Err(RpcError {
            code: -32001,
            message: "No blocks yet".to_string(),
        });
    }

    // Get the latest block's hash
    let block = state.state.get_block_by_slot(slot).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match block {
        Some(block) => {
            let block_hash = block.hash();
            Ok(serde_json::json!({
                "blockhash": block_hash.to_hex(),
                "slot": slot,
            }))
        }
        None => Err(RpcError {
            code: -32001,
            message: "Latest block not found".to_string(),
        }),
    }
}

async fn handle_get_fee_config(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let config = state.state.get_fee_config().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!({
        "base_fee_spores": config.base_fee,
        "contract_deploy_fee_spores": config.contract_deploy_fee,
        "contract_upgrade_fee_spores": config.contract_upgrade_fee,
        "nft_mint_fee_spores": config.nft_mint_fee,
        "nft_collection_fee_spores": config.nft_collection_fee,
        "fee_burn_percent": config.fee_burn_percent,
        "fee_producer_percent": config.fee_producer_percent,
        "fee_voters_percent": config.fee_voters_percent,
        "fee_treasury_percent": config.fee_treasury_percent,
        "fee_community_percent": config.fee_community_percent,
    }))
}

/// AUDIT-FIX 3.24: fee_burn_percent = 0 is valid per governance design.
/// The sum constraint (burn + producer + voters + treasury == 100) ensures
/// fees still flow somewhere. A minimum burn floor is a governance decision,
/// not a technical invariant.
async fn handle_set_fee_config(
    state: &RpcState,
    params: Option<serde_json::Value>,
    auth_header: Option<&str>,
) -> Result<serde_json::Value, RpcError> {
    require_legacy_admin_rpc_enabled(state, "setFeeConfig")?;
    // L3-01: Block in multi-validator mode — direct state write bypasses consensus
    require_single_validator(state, "setFeeConfig").await?;
    verify_admin_auth(state, auth_header)?;

    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let obj = params.as_object().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected object".to_string(),
    })?;

    let mut config = state.state.get_fee_config().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    if let Some(value) = obj.get("base_fee_spores").and_then(|v| v.as_u64()) {
        config.base_fee = value;
    }
    if let Some(value) = obj
        .get("contract_deploy_fee_spores")
        .and_then(|v| v.as_u64())
    {
        config.contract_deploy_fee = value;
    }
    if let Some(value) = obj
        .get("contract_upgrade_fee_spores")
        .and_then(|v| v.as_u64())
    {
        config.contract_upgrade_fee = value;
    }
    if let Some(value) = obj.get("nft_mint_fee_spores").and_then(|v| v.as_u64()) {
        config.nft_mint_fee = value;
    }
    if let Some(value) = obj
        .get("nft_collection_fee_spores")
        .and_then(|v| v.as_u64())
    {
        config.nft_collection_fee = value;
    }
    if let Some(value) = obj.get("fee_burn_percent").and_then(|v| v.as_u64()) {
        if value <= 100 {
            config.fee_burn_percent = value;
        }
    }
    if let Some(value) = obj.get("fee_producer_percent").and_then(|v| v.as_u64()) {
        if value <= 100 {
            config.fee_producer_percent = value;
        }
    }
    if let Some(value) = obj.get("fee_voters_percent").and_then(|v| v.as_u64()) {
        if value <= 100 {
            config.fee_voters_percent = value;
        }
    }
    if let Some(value) = obj.get("fee_treasury_percent").and_then(|v| v.as_u64()) {
        if value <= 100 {
            config.fee_treasury_percent = value;
        }
    }
    if let Some(value) = obj.get("fee_community_percent").and_then(|v| v.as_u64()) {
        if value <= 100 {
            config.fee_community_percent = value;
        }
    }

    // Validate that fee distribution percentages sum to 100
    let pct_sum = config.fee_burn_percent
        + config.fee_producer_percent
        + config.fee_voters_percent
        + config.fee_treasury_percent
        + config.fee_community_percent;
    if pct_sum != 100 {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Fee percentages must sum to 100, got {} (burn={}, producer={}, voters={}, treasury={}, community={})",
                pct_sum, config.fee_burn_percent, config.fee_producer_percent,
                config.fee_voters_percent, config.fee_treasury_percent,
                config.fee_community_percent,
            ),
        });
    }

    state
        .state
        .set_fee_config_full(&config)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    log_privileged_rpc_mutation(
        "setFeeConfig",
        "legacy_admin",
        "admin_token",
        "fee_config",
        None,
        serde_json::json!({
            "base_fee_spores": config.base_fee,
            "contract_deploy_fee_spores": config.contract_deploy_fee,
            "contract_upgrade_fee_spores": config.contract_upgrade_fee,
            "nft_mint_fee_spores": config.nft_mint_fee,
            "nft_collection_fee_spores": config.nft_collection_fee,
            "fee_burn_percent": config.fee_burn_percent,
            "fee_producer_percent": config.fee_producer_percent,
            "fee_voters_percent": config.fee_voters_percent,
            "fee_treasury_percent": config.fee_treasury_percent,
            "fee_community_percent": config.fee_community_percent,
        }),
    );

    Ok(serde_json::json!({"status": "ok"}))
}

async fn handle_get_rent_params(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let (rate, free_kb) = state.state.get_rent_params().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!({
        "rent_rate_spores_per_kb_month": rate,
        "rent_free_kb": free_kb,
    }))
}

async fn handle_set_rent_params(
    state: &RpcState,
    params: Option<serde_json::Value>,
    auth_header: Option<&str>,
) -> Result<serde_json::Value, RpcError> {
    require_legacy_admin_rpc_enabled(state, "setRentParams")?;
    // L3-01: Block in multi-validator mode — direct state write bypasses consensus
    require_single_validator(state, "setRentParams").await?;
    verify_admin_auth(state, auth_header)?;

    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let obj = params.as_object().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected object".to_string(),
    })?;

    let (mut rate, mut free_kb) = state.state.get_rent_params().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    if let Some(value) = obj
        .get("rent_rate_spores_per_kb_month")
        .and_then(|v| v.as_u64())
    {
        rate = value;
    }
    if let Some(value) = obj.get("rent_free_kb").and_then(|v| v.as_u64()) {
        free_kb = value;
    }

    state
        .state
        .set_rent_params(rate, free_kb)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    log_privileged_rpc_mutation(
        "setRentParams",
        "legacy_admin",
        "admin_token",
        "rent_params",
        None,
        serde_json::json!({
            "rent_rate_spores_per_kb_month": rate,
            "rent_free_kb": free_kb,
        }),
    );

    Ok(serde_json::json!({"status": "ok"}))
}

// ═══════════════════════════════════════════════════════════════════════════════
// FEE ESTIMATION
// ═══════════════════════════════════════════════════════════════════════════════

/// Estimate the fee and compute units for a transaction without executing it.
/// Params: [transaction_base64]
async fn handle_estimate_transaction_fee(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let tx_base64 = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [transaction_base64]".to_string(),
        })?;

    use base64::{engine::general_purpose, Engine as _};
    let tx_bytes = general_purpose::STANDARD
        .decode(tx_base64)
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid base64: {}", e),
        })?;

    let tx: Transaction = decode_transaction_bytes(&tx_bytes)?;

    validate_incoming_transaction_limits(&tx)?;

    let fee_config = state
        .state
        .get_fee_config()
        .unwrap_or_else(|_| lichen_core::FeeConfig::default_from_constants());

    let base_fee = TxProcessor::compute_base_fee(&tx, &fee_config);
    let priority_fee = TxProcessor::compute_priority_fee(&tx);
    let total_fee = base_fee.saturating_add(priority_fee);
    let compute_units = compute_units_for_tx(&tx);
    let compute_budget = tx.message.effective_compute_budget();
    let compute_unit_price = tx.message.effective_compute_unit_price();

    Ok(serde_json::json!({
        "fee_spores": total_fee,
        "fee_licn": total_fee as f64 / 1_000_000_000.0,
        "base_fee_spores": base_fee,
        "priority_fee_spores": priority_fee,
        "compute_units": compute_units,
        "compute_budget": compute_budget,
        "compute_unit_price": compute_unit_price,
    }))
}

// ═══════════════════════════════════════════════════════════════════════════════
// CORE TRANSACTION METHODS
// ═══════════════════════════════════════════════════════════════════════════════

/// Get transaction
async fn handle_get_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let sig_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [signature]".to_string(),
        })?;

    let sig_hash = lichen_core::Hash::from_hex(sig_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid signature: {}", e),
    })?;

    let tx = state
        .state
        .get_transaction(&sig_hash)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    // T8.2: Use tx→slot reverse index for O(1) lookup, fall back to scan
    let (slot, timestamp, slot_indexed) = match state.state.get_tx_slot(&sig_hash) {
        Ok(Some(slot)) => {
            let ts = state
                .state
                .get_block_by_slot(slot)
                .ok()
                .flatten()
                .map(|b| b.header.timestamp)
                .unwrap_or(0);
            (slot, ts, true)
        }
        _ => {
            // Fallback: reverse scan (for txs indexed before the reverse index existed)
            // M20 fix: cap fallback scan to prevent DoS via non-existent tx lookups
            if let Ok(last_slot) = state.state.get_last_slot() {
                let mut found = None;
                let scan_limit = 1000u64; // max slots to scan backwards
                let start_slot = last_slot;
                let end_slot = start_slot.saturating_sub(scan_limit);
                for slot in (end_slot..=start_slot).rev() {
                    if let Ok(Some(block)) = state.state.get_block_by_slot(slot) {
                        if block
                            .transactions
                            .iter()
                            .any(|tx| tx.signature() == sig_hash)
                        {
                            found = Some((slot, block.header.timestamp));
                            break;
                        }
                    }
                }
                found
                    .map(|(slot, ts)| (slot, ts, true))
                    .unwrap_or((0, 0, false))
            } else {
                (0, 0, false)
            }
        }
    };

    let fee_config = state
        .state
        .get_fee_config()
        .unwrap_or_else(|_| lichen_core::FeeConfig::default_from_constants());

    if !slot_indexed {
        return Ok(serde_json::Value::Null);
    }

    match tx {
        Some(tx) => {
            let tx_meta = state.state.get_tx_meta_full(&tx.signature()).ok().flatten();
            let stored_cu = tx_meta.as_ref().map(|m| m.compute_units_used);
            let mut json =
                tx_to_rpc_json(&tx, slot, timestamp, &fee_config, stored_cu, &state.state);
            // Add commitment status to transaction response
            let (status, confirmations) = if slot_indexed {
                tx_commitment_status(state, slot)
            } else {
                ("processed", serde_json::json!(0))
            };
            if let Some(obj) = json.as_object_mut() {
                obj.insert("confirmation_status".to_string(), serde_json::json!(status));
                obj.insert("confirmations".to_string(), confirmations);
                // Include full contract execution metadata
                if let Some(ref meta) = tx_meta {
                    if let Some(rc) = meta.return_code {
                        obj.insert("return_code".to_string(), serde_json::json!(rc));
                    }
                    if !meta.return_data.is_empty() {
                        use base64::{engine::general_purpose, Engine as _};
                        obj.insert(
                            "return_data".to_string(),
                            serde_json::json!(general_purpose::STANDARD.encode(&meta.return_data)),
                        );
                    }
                    if !meta.logs.is_empty() {
                        obj.insert("contract_logs".to_string(), serde_json::json!(meta.logs));
                    }
                }
            }
            Ok(json)
        }
        None => {
            // Fallback: look inside the block itself (covers genesis txs
            // and any tx that wasn't individually stored)
            if let Ok(Some(block)) = state.state.get_block_by_slot(slot) {
                for block_tx in &block.transactions {
                    if block_tx.signature() == sig_hash {
                        let cu = state
                            .state
                            .get_tx_meta_cu(&block_tx.signature())
                            .ok()
                            .flatten();
                        return Ok(tx_to_rpc_json(
                            block_tx,
                            slot,
                            timestamp,
                            &fee_config,
                            cu,
                            &state.state,
                        ));
                    }
                }
            }
            Err(RpcError {
                code: -32001,
                message: "Transaction not found".to_string(),
            })
        }
    }
}

/// Get a Merkle inclusion proof for a transaction by its signature.
///
/// params: [signature_hex]
/// Returns: { slot, tx_index, tx_hash, root, proof: [{ hash, direction }] }
async fn handle_get_transaction_proof(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let sig_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [signature]".to_string(),
        })?;

    let sig_hash = lichen_core::Hash::from_hex(sig_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid signature: {}", e),
    })?;

    // Find the slot containing this transaction
    let slot = match state.state.get_tx_slot(&sig_hash) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Err(RpcError {
                code: -32001,
                message: "Transaction not found".to_string(),
            });
        }
        Err(e) => {
            return Err(RpcError {
                code: -32000,
                message: format!("Database error: {}", e),
            });
        }
    };

    // Get the full block to find the transaction index and build the proof
    let block = state
        .state
        .get_block_by_slot(slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32001,
            message: format!("Block at slot {} not found", slot),
        })?;

    // Find the transaction index in the block
    let tx_index = block
        .transactions
        .iter()
        .position(|tx| tx.signature() == sig_hash)
        .ok_or_else(|| RpcError {
            code: -32001,
            message: "Transaction not found in block".to_string(),
        })?;

    // Generate the Merkle proof
    let proof =
        lichen_core::merkle_tx_proof(&block.transactions, tx_index).ok_or_else(|| RpcError {
            code: -32000,
            message: "Failed to generate Merkle proof".to_string(),
        })?;

    let proof_json: Vec<serde_json::Value> = proof
        .iter()
        .map(|(hash, is_left)| {
            serde_json::json!({
                "hash": hash.to_hex(),
                "direction": if *is_left { "left" } else { "right" }
            })
        })
        .collect();

    Ok(serde_json::json!({
        "slot": slot,
        "tx_index": tx_index,
        "tx_hash": block.transactions[tx_index].hash().to_hex(),
        "root": block.header.tx_root.to_hex(),
        "proof": proof_json
    }))
}

/// Get transactions involving a specific address
/// Get transactions involving a specific address (cursor-paginated, newest first)
/// params: [address, { limit?, before_slot? }]
async fn handle_get_transactions_by_address(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [address, options]".to_string(),
    })?;

    let address_str = params_array
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [address, options]".to_string(),
        })?;

    let opts = params_array.get(1);
    let requested_limit = opts
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;
    let limit = requested_limit.min(TX_LIST_MAX_LIMIT);

    let before_slot = opts
        .and_then(|v| v.get("before_slot"))
        .and_then(|v| v.as_u64());

    let target = Pubkey::from_base58(address_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid address: {}", e),
    })?;

    let fee_config = state
        .state
        .get_fee_config()
        .unwrap_or_else(|_| lichen_core::FeeConfig::default_from_constants());

    // Use account->tx reverse index (CF_ACCOUNT_TXS) with cursor pagination.
    // Fetch one extra index row to compute has_more without extra scans.
    let fetch_limit = limit.saturating_add(1);
    let indexed = state
        .state
        .get_account_tx_signatures_paginated(&target, fetch_limit, before_slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;
    let has_more = indexed.len() > limit;
    let page_items = if has_more {
        &indexed[..limit]
    } else {
        indexed.as_slice()
    };

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut timestamps: HashMap<u64, u64> = HashMap::new();
    let mut last_slot: Option<u64> = None;

    for (hash, slot) in page_items {
        let tx = match state.state.get_transaction(hash) {
            Ok(Some(tx)) => tx,
            _ => {
                if let Ok(Some(block)) = state.state.get_block_by_slot(*slot) {
                    match block.transactions.iter().find(|t| t.signature() == *hash) {
                        Some(t) => t.clone(),
                        None => continue,
                    }
                } else {
                    continue;
                }
            }
        };

        let timestamp = if let Some(cached) = timestamps.get(slot) {
            *cached
        } else {
            let ts = state
                .state
                .get_block_by_slot(*slot)
                .ok()
                .and_then(|block| block.map(|b| b.header.timestamp))
                .unwrap_or(0);
            timestamps.insert(*slot, ts);
            ts
        };

        let first_ix = tx.message.instructions.first();
        let (tx_type, from, to, amount) = if let Some(ix) = first_ix {
            let from = ix
                .accounts
                .first()
                .map(|acc| acc.to_base58())
                .unwrap_or_default();
            let to = ix
                .accounts
                .get(1)
                .map(|acc| acc.to_base58())
                .unwrap_or_default();
            let amount = parse_transfer_amount(ix).unwrap_or(0);
            (instruction_type(ix), from, to, amount)
        } else {
            ("Unknown", String::new(), String::new(), 0)
        };

        let fee = TxProcessor::compute_transaction_fee(&tx, &fee_config);

        let mut entry = serde_json::json!({
            "hash": tx.signature().to_hex(),
            "signature": tx.signature().to_hex(),
            "slot": slot,
            "timestamp": timestamp,
            "from": from,
            "to": to,
            "type": tx_type,
            "amount": amount as f64 / 1_000_000_000.0,
            "amount_spores": amount,
            "fee": fee,
            "fee_spores": fee,
            "fee_licn": fee as f64 / 1_000_000_000.0,
            "success": true,
        });

        // Enrich contract-call transactions with token metadata
        if tx_type == "ContractCall" {
            if let Some(ix) = first_ix {
                if let Some(func) = parse_contract_function(ix) {
                    entry["contract_function"] = serde_json::json!(func);
                }
                if let Some((symbol, token_amt, decimals, token_to)) =
                    extract_token_info(&state.state, ix)
                {
                    entry["token_symbol"] = serde_json::json!(symbol);
                    entry["token_amount"] =
                        serde_json::json!(token_amt as f64 / 10f64.powi(decimals as i32));
                    entry["token_amount_spores"] = serde_json::json!(token_amt);
                    entry["token_decimals"] = serde_json::json!(decimals);
                    if let Some(ref to_addr) = token_to {
                        entry["token_to"] = serde_json::json!(to_addr);
                    }
                }
            }
        }

        results.push(entry);

        last_slot = Some(*slot);
    }

    // Return with pagination cursor
    Ok(serde_json::json!({
        "transactions": results,
        "has_more": has_more,
        "next_before_slot": if has_more { last_slot } else { None::<u64> },
    }))
}

/// Get recent transactions across all addresses (cursor-paginated via CF_TX_BY_SLOT)
/// params: [{ limit?, before_slot? }]  or  []
async fn handle_get_recent_transactions(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let opts = params
        .as_ref()
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first());

    let requested_limit = opts
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;
    let limit = requested_limit.min(TX_LIST_MAX_LIMIT);

    let before_slot = opts
        .and_then(|v| v.get("before_slot"))
        .and_then(|v| v.as_u64());

    let fee_config = state
        .state
        .get_fee_config()
        .unwrap_or_else(|_| lichen_core::FeeConfig::default_from_constants());

    // Use tx-by-slot reverse index (CF_TX_BY_SLOT), over-fetch by 1 for has_more.
    let fetch_limit = limit.saturating_add(1);
    let indexed = state
        .state
        .get_recent_txs(fetch_limit, before_slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;
    let has_more = indexed.len() > limit;
    let page_items = if has_more {
        &indexed[..limit]
    } else {
        indexed.as_slice()
    };

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut timestamps: HashMap<u64, u64> = HashMap::new();
    let mut last_slot: Option<u64> = None;

    for (hash, slot) in page_items {
        let tx = match state.state.get_transaction(hash) {
            Ok(Some(tx)) => tx,
            _ => continue,
        };

        let timestamp = if let Some(cached) = timestamps.get(slot) {
            *cached
        } else {
            let ts = state
                .state
                .get_block_by_slot(*slot)
                .ok()
                .and_then(|block| block.map(|b| b.header.timestamp))
                .unwrap_or(0);
            timestamps.insert(*slot, ts);
            ts
        };

        let first_ix = tx.message.instructions.first();
        let (tx_type, from, to, amount) = if let Some(ix) = first_ix {
            let from = ix
                .accounts
                .first()
                .map(|acc| acc.to_base58())
                .unwrap_or_default();
            let to = ix
                .accounts
                .get(1)
                .map(|acc| acc.to_base58())
                .unwrap_or_default();
            let amount = parse_transfer_amount(ix).unwrap_or(0);
            (instruction_type(ix), from, to, amount)
        } else {
            ("Unknown", String::new(), String::new(), 0)
        };

        let fee = TxProcessor::compute_transaction_fee(&tx, &fee_config);

        let mut entry = serde_json::json!({
            "hash": tx.signature().to_hex(),
            "signature": tx.signature().to_hex(),
            "slot": slot,
            "timestamp": timestamp,
            "from": from,
            "to": to,
            "type": tx_type,
            "amount": amount as f64 / 1_000_000_000.0,
            "amount_spores": amount,
            "fee": fee,
            "fee_spores": fee,
            "fee_licn": fee as f64 / 1_000_000_000.0,
            "success": true,
        });

        // Enrich contract-call transactions with token metadata
        if tx_type == "ContractCall" {
            if let Some(ix) = first_ix {
                if let Some(func) = parse_contract_function(ix) {
                    entry["contract_function"] = serde_json::json!(func);
                }
                if let Some((symbol, token_amt, decimals, token_to)) =
                    extract_token_info(&state.state, ix)
                {
                    entry["token_symbol"] = serde_json::json!(symbol);
                    entry["token_amount"] =
                        serde_json::json!(token_amt as f64 / 10f64.powi(decimals as i32));
                    entry["token_amount_spores"] = serde_json::json!(token_amt);
                    entry["token_decimals"] = serde_json::json!(decimals);
                    if let Some(ref to_addr) = token_to {
                        entry["token_to"] = serde_json::json!(to_addr);
                    }
                }
            }
        }

        results.push(entry);

        last_slot = Some(*slot);
    }

    Ok(serde_json::json!({
        "transactions": results,
        "has_more": has_more,
        "next_before_slot": if has_more { last_slot } else { None::<u64> },
    }))
}

/// Get all token accounts for a holder with balances and symbol info
/// params: [holder_address]
async fn handle_get_token_accounts(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let holder_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Expected [holder_address]".to_string(),
        })?;

    let holder = Pubkey::from_base58(holder_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid holder: {}", e),
    })?;

    let token_balances = state
        .state
        .get_holder_token_balances(&holder, 100)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mut accounts: Vec<serde_json::Value> = Vec::new();
    for (token_program, balance) in &token_balances {
        // Try to get symbol info from registry
        let registry = state
            .state
            .get_symbol_registry_by_program(token_program)
            .ok()
            .flatten();

        let symbol = registry
            .as_ref()
            .map(|r| r.symbol.clone())
            .unwrap_or_else(|| "Unknown".to_string());
        let name = registry
            .as_ref()
            .and_then(|r| r.name.clone())
            .unwrap_or_default();
        let decimals = registry
            .as_ref()
            .and_then(|r| r.metadata.as_ref())
            .and_then(|m| m.get("decimals"))
            .and_then(|d| d.as_u64())
            .unwrap_or(9);

        let ui_amount = *balance as f64 / 10_f64.powi(decimals as i32);

        accounts.push(serde_json::json!({
            "mint": token_program.to_base58(),
            "balance": balance,
            "ui_amount": ui_amount,
            "decimals": decimals,
            "symbol": symbol,
            "name": name,
        }));
    }

    Ok(serde_json::json!({
        "accounts": accounts,
        "count": accounts.len(),
    }))
}

async fn handle_get_account_tx_count(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let address_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [address]".to_string(),
        })?;

    let target = Pubkey::from_base58(address_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid address: {}", e),
    })?;

    let count = state
        .state
        .count_account_txs(&target)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!({
        "address": target.to_base58(),
        "count": count,
    }))
}

fn submit_transaction(state: &RpcState, tx: Transaction) -> Result<String, RpcError> {
    let signature_hash = tx.signature();

    if let Some(ref sender) = state.tx_sender {
        sender.try_send(tx).map_err(|e| RpcError {
            code: -32003,
            message: format!("Transaction queue full, try again later: {}", e),
        })?;
        info!(
            "📮 Transaction submitted to mempool: {}",
            signature_hash.to_hex()
        );
    } else {
        return Err(RpcError {
            code: -32000,
            message: "Node is not accepting transactions (no mempool configured)".to_string(),
        });
    }

    Ok(signature_hash.to_hex())
}

fn validate_incoming_transaction_limits(tx: &Transaction) -> Result<(), RpcError> {
    tx.validate_structure().map_err(|e| RpcError {
        code: -32003,
        message: format!("Invalid transaction structure: {}", e),
    })
}

fn transaction_has_contract_call_preflight(tx: &Transaction) -> bool {
    tx.message.instructions.iter().any(|ix| {
        ix.program_id == lichen_core::CONTRACT_PROGRAM_ID && !ix.data.starts_with(b"{\"Deploy\"")
    })
}

fn transaction_has_mandatory_shielded_preflight(tx: &Transaction) -> bool {
    tx.message.instructions.iter().any(|ix| {
        ix.program_id == lichen_core::SYSTEM_PROGRAM_ID
            && matches!(ix.data.first().copied(), Some(23..=25))
    })
}

pub(crate) async fn preflight_transaction_submission(
    state: &RpcState,
    tx: &Transaction,
    skip_preflight: bool,
) -> Result<(), RpcError> {
    validate_incoming_transaction_limits(tx)?;

    if tx.is_evm() {
        return Err(RpcError {
            code: -32003,
            message: "EVM transactions are not allowed via sendTransaction".to_string(),
        });
    }

    if tx.signatures.is_empty() {
        return Err(RpcError {
            code: -32003,
            message: "Transaction has no signatures".to_string(),
        });
    }

    for sig in &tx.signatures {
        if pq_signature_is_zero(sig) {
            return Err(RpcError {
                code: -32003,
                message: "Transaction contains an invalid zero signature".to_string(),
            });
        }
    }

    tx.verify_required_signatures().map_err(|error| RpcError {
        code: -32003,
        message: error,
    })?;

    {
        let budget = tx.message.effective_compute_budget();
        if budget > lichen_core::MAX_COMPUTE_BUDGET {
            return Err(RpcError {
                code: -32003,
                message: format!(
                    "Compute budget {} exceeds maximum {}",
                    budget,
                    lichen_core::MAX_COMPUTE_BUDGET
                ),
            });
        }
    }

    {
        let fee_payer = tx
            .message
            .instructions
            .first()
            .and_then(|ix| ix.accounts.first().cloned());
        if let Some(payer) = fee_payer {
            let fee_config = state
                .state
                .get_fee_config()
                .unwrap_or_else(|_| lichen_core::FeeConfig::default_from_constants());
            let expected_fee = TxProcessor::compute_transaction_fee(tx, &fee_config);

            if expected_fee > 0 {
                match state.state.get_account(&payer) {
                    Ok(Some(acct)) => {
                        if acct.spendable < expected_fee {
                            return Err(RpcError {
                                code: -32003,
                                message: format!(
                                    "Insufficient LICN balance for fees: need {} spores ({:.6} LICN), have {} spores ({:.6} LICN)",
                                    expected_fee,
                                    expected_fee as f64 / 1_000_000_000.0,
                                    acct.spendable,
                                    acct.spendable as f64 / 1_000_000_000.0
                                ),
                            });
                        }

                        if let Some(first_ix) = tx.message.instructions.first() {
                            if first_ix.program_id == lichen_core::SYSTEM_PROGRAM_ID {
                                if let Some(&kind) = first_ix.data.first() {
                                    if (kind == 0 || kind == 1) && first_ix.data.len() >= 9 {
                                        let transfer_amount = u64::from_le_bytes(
                                            first_ix.data[1..9].try_into().unwrap_or([0u8; 8]),
                                        );
                                        if acct.spendable
                                            < expected_fee.saturating_add(transfer_amount)
                                        {
                                            return Err(RpcError {
                                                code: -32003,
                                                message: format!(
                                                    "Insufficient LICN for transfer + fees: need {} spores (transfer) + {} spores (fee) = {} total, have {} spendable",
                                                    transfer_amount,
                                                    expected_fee,
                                                    transfer_amount.saturating_add(expected_fee),
                                                    acct.spendable
                                                ),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        return Err(RpcError {
                            code: -32003,
                            message: "Payer account does not exist on-chain. Fund it first."
                                .to_string(),
                        });
                    }
                    Err(_) => {}
                }
            }
        }
    }

    if let Some(first_ix) = tx.message.instructions.first() {
        if first_ix.program_id == lichen_core::SYSTEM_PROGRAM_ID {
            if let Some(&opcode) = first_ix.data.first() {
                if matches!(opcode, 27 | 30 | 31) {
                    let sender = tx.sender();
                    let is_active_validator = if let Some(ref pool) = state.stake_pool {
                        let pool_guard = pool.read().await;
                        pool_guard
                            .get_stake(&sender)
                            .map(|s| s.is_active && s.meets_minimum())
                            .unwrap_or(false)
                    } else {
                        false
                    };

                    if !is_active_validator {
                        let op_name = match opcode {
                            27 => "SlashValidator",
                            30 => "OracleAttestation",
                            31 => "DeregisterValidator",
                            _ => "System",
                        };
                        return Err(RpcError {
                            code: -32003,
                            message: format!(
                                "{} transactions can only be submitted by active validators",
                                op_name
                            ),
                        });
                    }
                }
            }
        }
    }

    let must_simulate = transaction_has_mandatory_shielded_preflight(tx);
    if must_simulate {
        let processor = TxProcessor::new(state.state.clone());
        processor
            .validate_shielded_preflight(tx)
            .map_err(|reason| RpcError {
                code: -32002,
                message: format!("Transaction simulation failed: {}", reason),
            })?;
    }

    if !skip_preflight && transaction_has_contract_call_preflight(tx) {
        let processor = TxProcessor::new(state.state.clone());
        let sim = processor.simulate_transaction(tx);
        if !sim.success {
            let reason = sim
                .error
                .unwrap_or_else(|| "Contract execution failed".to_string());
            return Err(RpcError {
                code: -32002,
                message: format!("Transaction simulation failed: {}", reason),
            });
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// COMMITMENT-LEVEL HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Helper: extract commitment string from params (supports both array and object forms)
fn parse_commitment(params: &Option<serde_json::Value>) -> &str {
    params
        .as_ref()
        .and_then(|p| {
            // Array form: ["sig", "confirmed"]  or  ["sig", {"commitment": "confirmed"}]
            p.as_array()
                .and_then(|arr| {
                    arr.get(1).and_then(|v| {
                        v.as_str().or_else(|| {
                            v.as_object()
                                .and_then(|o| o.get("commitment"))
                                .and_then(|c| c.as_str())
                        })
                    })
                })
                // Object form: { "signature": "...", "commitment": "confirmed" }
                .or_else(|| {
                    p.as_object()
                        .and_then(|o| o.get("commitment"))
                        .and_then(|v| v.as_str())
                })
        })
        .unwrap_or("processed")
}

/// Determine the commitment status of a transaction given its slot.
/// Returns (confirmation_status, confirmations_or_null).
fn tx_commitment_status(state: &RpcState, tx_slot: u64) -> (&'static str, serde_json::Value) {
    if let Some(ref ft) = state.finality {
        let status = ft.commitment_for_slot(tx_slot);
        let confirmations = match status {
            "finalized" => serde_json::Value::Null, // Solana returns null for finalized
            _ => {
                let confirmed = ft.confirmed_slot();
                if tx_slot <= confirmed {
                    serde_json::Value::Null
                } else {
                    // Approximate confirmations as processed_slot - tx_slot
                    let processed = state.state.get_last_slot().unwrap_or(0);
                    serde_json::json!(processed.saturating_sub(tx_slot))
                }
            }
        };
        (status, confirmations)
    } else {
        // No finality tracker — fall back to "processed" for all
        ("processed", serde_json::json!(0))
    }
}

/// confirmTransaction — check transaction confirmation status
///
/// Params: ["signature"] or ["signature", "commitment"] or ["signature", {"commitment": "..."}]
///
/// Returns:
/// ```json
/// {
///   "value": {
///     "confirmation_status": "processed"|"confirmed"|"finalized",
///     "slot": 42,
///     "confirmations": 5 | null,
///     "err": null
///   }
/// }
/// ```
///
/// If the transaction is not found, returns `{"value": null}`.
/// If a commitment level is specified, returns null unless the tx has reached
/// at least that commitment level.
async fn handle_confirm_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params_ref = &params;
    let sig_str = params_ref
        .as_ref()
        .and_then(|p| p.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .or_else(|| {
            params_ref
                .as_ref()
                .and_then(|p| p.as_object())
                .and_then(|o| o.get("signature"))
                .and_then(|v| v.as_str())
        })
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [signature] or {\"signature\": \"...\"}".to_string(),
        })?;

    let sig_hash = lichen_core::Hash::from_hex(sig_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid signature: {}", e),
    })?;

    let desired_commitment = parse_commitment(params_ref);

    // Look up which slot the tx was included in
    let tx_slot = match state.state.get_tx_slot(&sig_hash) {
        Ok(Some(slot)) => slot,
        Ok(None) => {
            // TX not yet included in any block
            return Ok(serde_json::json!({"value": null}));
        }
        Err(e) => {
            return Err(RpcError {
                code: -32000,
                message: format!("Database error: {}", e),
            });
        }
    };

    let (status, confirmations) = tx_commitment_status(state, tx_slot);

    // Check if the tx has reached the desired commitment level
    if commitment_rank(status) < commitment_rank(desired_commitment) {
        // TX exists but hasn't reached the desired commitment level
        return Ok(serde_json::json!({"value": null}));
    }

    Ok(serde_json::json!({
        "value": {
            "confirmation_status": status,
            "slot": tx_slot,
            "confirmations": confirmations,
            "err": null,
        }
    }))
}

fn decode_solana_transaction(
    payload: &str,
    encoding: Option<&str>,
) -> Result<Transaction, RpcError> {
    use base64::{engine::general_purpose, Engine as _};

    let tx_bytes = match encoding {
        Some("base58") => bs58::decode(payload).into_vec().map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid base58: {}", e),
        })?,
        Some("base64") | None => {
            let decoded = general_purpose::STANDARD.decode(payload);
            match decoded {
                Ok(bytes) => bytes,
                Err(_) => bs58::decode(payload).into_vec().map_err(|e| RpcError {
                    code: -32602,
                    message: format!("Invalid base58: {}", e),
                })?,
            }
        }
        Some(other) => {
            return Err(RpcError {
                code: -32602,
                message: format!("Unsupported encoding: {}", other),
            })
        }
    };

    decode_transaction_bytes(&tx_bytes)
}

/// Parse a JSON-format transaction from the wallet into a native Transaction.
/// Wallet sends: {signatures: [[byte,...]], message: {instructions: [...], blockhash: "hex"}}
fn parse_json_transaction(tx_bytes: &[u8]) -> Result<Transaction, RpcError> {
    let json_val: serde_json::Value = serde_json::from_slice(tx_bytes).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid JSON transaction: {}", e),
    })?;

    // Parse signatures: wallet sends array-of-arrays of numbers
    let sigs_raw = json_val
        .get("signatures")
        .and_then(|s| s.as_array())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing signatures array".into(),
        })?;
    let mut signatures = Vec::new();
    for sig_val in sigs_raw {
        signatures.push(parse_pq_signature_value(sig_val)?);
    }

    let msg_val = json_val.get("message").ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing message".into(),
    })?;

    // Blockhash — accept multiple naming conventions:
    // "blockhash" (wallet), "recent_blockhash" (Rust), "recentBlockhash" (SDK camelCase)
    let blockhash_str = msg_val
        .get("blockhash")
        .or_else(|| msg_val.get("recent_blockhash"))
        .or_else(|| msg_val.get("recentBlockhash"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing blockhash".into(),
        })?;
    let recent_blockhash = lichen_core::Hash::from_hex(blockhash_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid blockhash: {}", e),
    })?;

    // Instructions
    let ixs_raw = msg_val
        .get("instructions")
        .and_then(|i| i.as_array())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing instructions array".into(),
        })?;
    let mut instructions = Vec::new();
    for ix_val in ixs_raw {
        // Accept both snake_case "program_id" and camelCase "programId"
        let pid_val = ix_val.get("program_id").or_else(|| ix_val.get("programId"));
        let program_id = if let Some(arr) = pid_val.and_then(|p| p.as_array()) {
            let bytes: Vec<u8> = arr
                .iter()
                .filter_map(|b| b.as_u64().map(|n| n as u8))
                .collect();
            if bytes.len() != 32 {
                return Err(RpcError {
                    code: -32602,
                    message: format!("program_id must be 32 bytes, got {}", bytes.len()),
                });
            }
            let mut pk = [0u8; 32];
            pk.copy_from_slice(&bytes);
            Pubkey(pk)
        } else if let Some(s) = pid_val.and_then(|p| p.as_str()) {
            Pubkey::from_base58(s).map_err(|e| RpcError {
                code: -32602,
                message: format!("Invalid program_id: {}", e),
            })?
        } else {
            return Err(RpcError {
                code: -32602,
                message: "Invalid program_id format".into(),
            });
        };

        let accounts_raw = ix_val
            .get("accounts")
            .and_then(|a| a.as_array())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Missing accounts in instruction".into(),
            })?;
        let mut accounts = Vec::new();
        for acct in accounts_raw {
            if let Some(arr) = acct.as_array() {
                let bytes: Vec<u8> = arr
                    .iter()
                    .filter_map(|b| b.as_u64().map(|n| n as u8))
                    .collect();
                if bytes.len() != 32 {
                    return Err(RpcError {
                        code: -32602,
                        message: format!("Account pubkey must be 32 bytes, got {}", bytes.len()),
                    });
                }
                let mut pk = [0u8; 32];
                pk.copy_from_slice(&bytes);
                accounts.push(Pubkey(pk));
            } else if let Some(s) = acct.as_str() {
                accounts.push(Pubkey::from_base58(s).map_err(|e| RpcError {
                    code: -32602,
                    message: format!("Invalid account: {}", e),
                })?);
            }
        }

        let data: Vec<u8> = ix_val
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| b.as_u64().map(|n| n as u8))
                    .collect()
            })
            .unwrap_or_default();

        instructions.push(lichen_core::Instruction {
            program_id,
            accounts,
            data,
        });
    }

    // Parse optional compute_budget and compute_unit_price from message
    let compute_budget = msg_val
        .get("compute_budget")
        .or_else(|| msg_val.get("computeBudget"))
        .and_then(|v| v.as_u64());
    let compute_unit_price = msg_val
        .get("compute_unit_price")
        .or_else(|| msg_val.get("computeUnitPrice"))
        .and_then(|v| v.as_u64());

    Ok(Transaction {
        signatures,
        message: lichen_core::Message {
            instructions,
            recent_blockhash,
            compute_budget,
            compute_unit_price,
        },
        tx_type: lichen_core::TransactionType::Native,
    })
}

/// Send transaction
async fn handle_send_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    // Support optional second param: { skipPreflight: true }
    let skip_preflight = params
        .as_array()
        .and_then(|arr| arr.get(1))
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("skipPreflight"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let tx_base64 = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [transaction_base64]".to_string(),
        })?;

    // Decode base64 transaction
    use base64::{engine::general_purpose, Engine as _};
    let tx_bytes = general_purpose::STANDARD
        .decode(tx_base64)
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid base64: {}", e),
        })?;

    // M-6: Decode via wire-format envelope (supports V1 envelope, legacy bincode, JSON)
    let tx: Transaction = decode_transaction_bytes(&tx_bytes)?;

    preflight_transaction_submission(state, &tx, skip_preflight).await?;

    // RPC-04: Emit prediction WS events AFTER mempool acceptance (not before),
    // so clients never receive events for transactions that fail submission.
    // Clone the tx for event emission since submit_transaction consumes it.
    let tx_for_events = tx.clone();
    let signature = submit_transaction(state, tx)?;
    emit_prediction_events_from_tx(state, &tx_for_events);

    Ok(serde_json::json!(signature))
}

async fn handle_simulate_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let tx_base64 = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [transaction_base64]".to_string(),
        })?;

    // Decode base64 transaction
    use base64::{engine::general_purpose, Engine as _};
    let tx_bytes = general_purpose::STANDARD
        .decode(tx_base64)
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid base64: {}", e),
        })?;

    let tx: Transaction = decode_transaction_bytes(&tx_bytes)?;

    validate_incoming_transaction_limits(&tx)?;

    let processor = TxProcessor::new(state.state.clone());
    let result = processor.simulate_transaction(&tx);

    let return_data_b64 = result.return_data.as_ref().map(|data| {
        use base64::{engine::general_purpose, Engine as _};
        general_purpose::STANDARD.encode(data)
    });

    Ok(serde_json::json!({
        "success": result.success,
        "fee": result.fee,
        "logs": result.logs,
        "error": result.error,
        "computeUsed": result.compute_used,
        "computeBudget": tx.message.effective_compute_budget(),
        "computeUnitPrice": tx.message.effective_compute_unit_price(),
        "priorityFee": TxProcessor::compute_priority_fee(&tx),
        "returnData": return_data_b64,
        "returnCode": result.return_code,
        "stateChanges": result.state_changes,
    }))
}

// ============================================================================
// GX-01: callContract — read-only contract call (equivalent to eth_call)
// ============================================================================

fn execute_readonly_contract_call(
    state: &RpcState,
    contract_pubkey: Pubkey,
    contract: &ContractAccount,
    caller: Pubkey,
    function: &str,
    args: Vec<u8>,
) -> Result<lichen_core::contract::ContractResult, RpcError> {
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let live_storage = state
        .state
        .load_contract_storage_map(&contract_pubkey)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let context = lichen_core::contract::build_top_level_call_context(
        lichen_core::contract::ContractContext::with_args(
            caller,
            contract_pubkey,
            0,
            current_slot,
            live_storage,
            args,
        ),
        state.state.clone(),
        lichen_core::contract::DEFAULT_COMPUTE_LIMIT,
    );
    let exec_args = context.args.clone();

    let mut runtime = ContractRuntime::get_pooled();
    let exec_result = runtime.execute(contract, function, &exec_args, context);
    runtime.return_to_pool();

    exec_result.map_err(|error| RpcError {
        code: -32000,
        message: format!("Contract execution failed: {}", error),
    })
}

fn merged_contract_logs(result: &lichen_core::contract::ContractResult) -> Vec<String> {
    let mut logs = result.logs.clone();
    logs.extend(result.cross_call_logs.iter().cloned());
    logs
}

/// Execute a read-only contract call without requiring a signed transaction.
/// Params: { "contract": "<base58_address>", "function": "<fn_name>", "args": [<bytes>] }
/// or array form: ["<base58_address>", "<fn_name>", "<base64_args_optional>"]
async fn handle_call_contract(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    // Parse params: either object form or array form
    let (contract_str, function, args_b64, from_str) = if let Some(obj) = params.as_object() {
        let contract = obj
            .get("contract")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Missing 'contract' address".to_string(),
            })?;
        let function = obj
            .get("function")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Missing 'function' name".to_string(),
            })?;
        let args = obj.get("args").and_then(|v| v.as_str()).unwrap_or("");
        let from = obj
            .get("from")
            .or_else(|| obj.get("caller"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        (
            contract.to_string(),
            function.to_string(),
            args.to_string(),
            from,
        )
    } else if let Some(arr) = params.as_array() {
        let contract = arr
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Missing contract address (first param)".to_string(),
            })?;
        let function = arr
            .get(1)
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Missing function name (second param)".to_string(),
            })?;
        let args = arr.get(2).and_then(|v| v.as_str()).unwrap_or("");
        let from = arr.get(3).and_then(|v| v.as_str()).map(|s| s.to_string());
        (
            contract.to_string(),
            function.to_string(),
            args.to_string(),
            from,
        )
    } else {
        return Err(RpcError {
            code: -32602,
            message: "Invalid params: expected object or array".to_string(),
        });
    };

    // Decode contract address
    let contract_pubkey = Pubkey::from_base58(&contract_str).map_err(|_| RpcError {
        code: -32602,
        message: format!("Invalid contract address: {}", contract_str),
    })?;

    // Decode args (base64-encoded bytes, optional)
    let args: Vec<u8> = if args_b64.is_empty() {
        Vec::new()
    } else {
        use base64::{engine::general_purpose, Engine as _};
        general_purpose::STANDARD
            .decode(&args_b64)
            .map_err(|e| RpcError {
                code: -32602,
                message: format!("Invalid base64 args: {}", e),
            })?
    };

    // Load the contract account
    let account = state
        .state
        .get_account(&contract_pubkey)
        .map_err(|_| RpcError {
            code: -32000,
            message: "Failed to load contract account".to_string(),
        })?
        .ok_or_else(|| RpcError {
            code: -32000,
            message: format!("Contract not found: {}", contract_str),
        })?;

    if !account.executable {
        return Err(RpcError {
            code: -32000,
            message: format!("Account {} is not an executable contract", contract_str),
        });
    }

    let contract: ContractAccount =
        serde_json::from_slice(&account.data).map_err(|_| RpcError {
            code: -32000,
            message: "Failed to deserialize contract account".to_string(),
        })?;

    // Use provided caller address or zero address for read-only calls
    let caller = if let Some(ref from) = from_str {
        Pubkey::from_base58(from).map_err(|_| RpcError {
            code: -32602,
            message: format!("Invalid 'from' address: {}", from),
        })?
    } else {
        Pubkey::new([0u8; 32])
    };
    let result =
        execute_readonly_contract_call(state, contract_pubkey, &contract, caller, &function, args)?;
    let return_data_b64 = encode_readonly_return_data_b64(&result);
    Ok(serde_json::json!({
        "success": result.success,
        "returnData": return_data_b64,
        "returnCode": result.return_code,
        "logs": merged_contract_logs(&result),
        "error": result.error,
        "computeUsed": result.compute_used,
    }))
}

fn encode_readonly_return_data_b64(
    result: &lichen_core::contract::ContractResult,
) -> Option<String> {
    use base64::{engine::general_purpose, Engine as _};

    if !result.return_data.is_empty() {
        return Some(general_purpose::STANDARD.encode(&result.return_data));
    }

    result
        .return_code
        .map(|return_code| general_purpose::STANDARD.encode(return_code.to_le_bytes()))
}

fn decode_contract_result_u64(result: &lichen_core::contract::ContractResult) -> Option<u64> {
    if result.return_data.len() >= 8 {
        let mut raw = [0u8; 8];
        raw.copy_from_slice(&result.return_data[..8]);
        return Some(u64::from_le_bytes(raw));
    }

    result
        .return_code
        .and_then(|return_code| u64::try_from(return_code).ok())
}

// ============================================================================
// SOLANA-COMPATIBLE ENDPOINTS
// ============================================================================

async fn handle_solana_get_latest_blockhash(
    state: &RpcState,
) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let block_hash = state
        .state
        .get_block_by_slot(slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .map(|block| block.hash())
        .unwrap_or_default();

    Ok(serde_json::json!({
        "context": solana_context(state)?,
        "value": {
            "blockhash": hash_to_base58(&block_hash),
            "lastValidBlockHeight": slot,
        }
    }))
}

async fn handle_solana_get_slot(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    // Solana getSlot accepts optional [config] with commitment
    // For simplicity, return processed slot (chain tip) — matches Solana default
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!(slot))
}

async fn handle_solana_get_block_height(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!(slot))
}

async fn handle_solana_get_signature_statuses(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [signatures, options?]".to_string(),
    })?;

    let signatures = params_array
        .first()
        .and_then(|v| v.as_array())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [signatures, options?]".to_string(),
        })?;

    let last_slot = state.state.get_last_slot().unwrap_or(0);

    let mut values: Vec<serde_json::Value> = Vec::with_capacity(signatures.len());

    for sig_value in signatures {
        let sig_str = match sig_value.as_str() {
            Some(value) => value,
            None => {
                values.push(serde_json::Value::Null);
                continue;
            }
        };

        let sig_hash = match base58_to_hash(sig_str) {
            Ok(value) => value,
            Err(_) => {
                values.push(serde_json::Value::Null);
                continue;
            }
        };

        let mut found = false;
        let mut tx_slot = last_slot;
        if state.solana_tx_cache.read().await.contains(&sig_hash) {
            found = true;
            // Try to get actual slot
            if let Ok(Some(slot)) = state.state.get_tx_slot(&sig_hash) {
                tx_slot = slot;
            }
        } else if let Ok(Some(_)) = state.state.get_transaction(&sig_hash) {
            found = true;
            if let Ok(Some(slot)) = state.state.get_tx_slot(&sig_hash) {
                tx_slot = slot;
            }
        }

        if found {
            let (status, confirmations) = tx_commitment_status(state, tx_slot);
            values.push(serde_json::json!({
                "slot": tx_slot,
                "confirmations": confirmations,
                "err": serde_json::Value::Null,
                "confirmation_status": status,
            }));
        } else {
            values.push(serde_json::Value::Null);
        }
    }

    Ok(serde_json::json!({
        "context": solana_context(state)?,
        "value": values,
    }))
}

async fn handle_solana_get_signatures_for_address(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [address, options?]".to_string(),
    })?;

    let address_str = params_array
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [address, options?]".to_string(),
        })?;

    let options = params_array.get(1).and_then(|v| v.as_object());
    let limit = options
        .and_then(|opts| opts.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(100)
        .min(1000) as usize;

    let before = if let Some(value) = options
        .and_then(|opts| opts.get("before"))
        .and_then(|v| v.as_str())
    {
        Some(base58_to_hash(value)?)
    } else {
        None
    };

    let until = if let Some(value) = options
        .and_then(|opts| opts.get("until"))
        .and_then(|v| v.as_str())
    {
        Some(base58_to_hash(value)?)
    } else {
        None
    };

    let target = Pubkey::from_base58(address_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid address: {}", e),
    })?;

    let fetch_limit = limit.saturating_add(1).min(1000);
    let indexed = state
        .state
        .get_account_tx_signatures(&target, fetch_limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let filtered = filter_signatures_for_address(indexed, before, until, limit);

    let mut timestamps: HashMap<u64, u64> = HashMap::new();
    let mut results: Vec<serde_json::Value> = Vec::new();

    for (hash, slot) in filtered {
        let block_time = if let Some(cached) = timestamps.get(&slot) {
            *cached
        } else {
            let timestamp = state
                .state
                .get_block_by_slot(slot)
                .ok()
                .and_then(|block| block.map(|b| b.header.timestamp))
                .unwrap_or(0);
            timestamps.insert(slot, timestamp);
            timestamp
        };

        results.push(serde_json::json!({
            "signature": hash_to_base58(&hash),
            "slot": slot,
            "err": serde_json::Value::Null,
            "memo": serde_json::Value::Null,
            "blockTime": block_time,
            "confirmation_status": "finalized",
        }));
    }

    Ok(serde_json::json!(results))
}

async fn handle_solana_get_balance(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|_| invalid_pubkey_format_error())?;

    let balance = state.state.get_balance(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!({
        "context": solana_context(state)?,
        "value": balance,
    }))
}

async fn handle_solana_get_account_info(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [pubkey, options?]".to_string(),
    })?;

    let pubkey_str = params_array
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey, options?]".to_string(),
        })?;

    let encoding = params_array
        .get(1)
        .and_then(|v| v.get("encoding"))
        .and_then(|v| v.as_str())
        .unwrap_or("base64");

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|_| invalid_pubkey_format_error())?;

    let account = state.state.get_account(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let value = match account {
        Some(account) => {
            let data = match encoding {
                "base64" => {
                    use base64::{engine::general_purpose, Engine as _};
                    general_purpose::STANDARD.encode(&account.data)
                }
                "base58" => bs58::encode(&account.data).into_string(),
                other => {
                    return Err(RpcError {
                        code: -32602,
                        message: format!("Unsupported encoding: {}", other),
                    })
                }
            };

            serde_json::json!({
                "data": [data, encoding],
                "executable": account.executable,
                "lamports": account.spores,
                "owner": account.owner.to_base58(),
                "rentEpoch": account.rent_epoch,
                "space": account.data.len(),
            })
        }
        None => match load_solana_token_account_snapshot(state, &pubkey)? {
            Some(snapshot) => solana_token_account_response(&snapshot, encoding)?,
            None => serde_json::Value::Null,
        },
    };

    Ok(serde_json::json!({
        "context": solana_context(state)?,
        "value": value,
    }))
}

async fn handle_solana_get_block(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let slot = parse_get_block_slot_param(params.as_ref(), true)?;

    let params_array = params
        .as_ref()
        .and_then(|v| v.as_array())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [slot, options?] where slot is a u64 block height (block hash is not supported)".to_string(),
        })?;

    let options = params_array.get(1).and_then(|v| v.as_object());
    let transaction_details = options
        .and_then(|opts| opts.get("transactionDetails"))
        .and_then(|v| v.as_str())
        .unwrap_or("full");
    let encoding = options
        .and_then(|opts| opts.get("encoding"))
        .and_then(|v| v.as_str())
        .unwrap_or("json");

    validate_solana_transaction_details(transaction_details)?;
    validate_solana_encoding(encoding)?;

    let block = state.state.get_block_by_slot(slot).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let block = match block {
        Some(block) => block,
        None => return Ok(serde_json::Value::Null),
    };

    let transactions = match transaction_details {
        "none" => serde_json::Value::Null,
        "signatures" => serde_json::Value::Array(
            block
                .transactions
                .iter()
                .map(|tx| serde_json::Value::String(hash_to_base58(&tx.signature())))
                .collect(),
        ),
        _ => {
            let entries = block
                .transactions
                .iter()
                .map(|tx| {
                    if encoding == "base64" || encoding == "base58" {
                        solana_block_transaction_encoded_json(&state.state, tx, 0, encoding)
                    } else {
                        solana_block_transaction_json(&state.state, tx, 0)
                    }
                })
                .collect::<Vec<_>>();
            serde_json::Value::Array(entries)
        }
    };

    Ok(serde_json::json!({
        "blockHeight": block.header.slot,
        "blockTime": block.header.timestamp,
        "blockhash": hash_to_base58(&block.hash()),
        "previousBlockhash": hash_to_base58(&block.header.parent_hash),
        "parentSlot": block.header.slot.saturating_sub(1),
        "transactions": transactions,
    }))
}

async fn handle_solana_get_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [signature, options?]".to_string(),
    })?;

    let sig_str = params_array
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [signature, options?]".to_string(),
        })?;

    let encoding = params_array
        .get(1)
        .and_then(|v| v.get("encoding"))
        .and_then(|v| v.as_str())
        .unwrap_or("json");

    validate_solana_encoding(encoding)?;

    let sig_hash = base58_to_hash(sig_str)?;

    if let Some(record) = state.solana_tx_cache.read().await.peek(&sig_hash).cloned() {
        if encoding == "base64" || encoding == "base58" {
            return Ok(solana_transaction_encoded_json(
                &state.state,
                &record.tx,
                record.slot,
                record.timestamp,
                record.fee,
                encoding,
            ));
        }
        return Ok(solana_transaction_json(
            &state.state,
            &record.tx,
            record.slot,
            record.timestamp,
            record.fee,
        ));
    }

    let tx = state
        .state
        .get_transaction(&sig_hash)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    // T8.2: Use tx→slot reverse index for O(1) lookup, fall back to scan.
    // Only return the transaction once it is indexed in a block.
    let (slot, timestamp, slot_indexed) = match state.state.get_tx_slot(&sig_hash) {
        Ok(Some(slot)) => {
            let ts = state
                .state
                .get_block_by_slot(slot)
                .ok()
                .flatten()
                .map(|b| b.header.timestamp)
                .unwrap_or(0);
            (slot, ts, true)
        }
        _ => {
            // Fallback: reverse scan (for txs indexed before the reverse index existed)
            // AUDIT-FIX 0.13: Cap fallback scan to 1000 slots to prevent DoS
            if let Ok(last_slot) = state.state.get_last_slot() {
                let mut found = None;
                let scan_limit = 1000u64;
                let end_slot = last_slot.saturating_sub(scan_limit);
                for slot in (end_slot..=last_slot).rev() {
                    if let Ok(Some(block)) = state.state.get_block_by_slot(slot) {
                        if block
                            .transactions
                            .iter()
                            .any(|block_tx| block_tx.signature() == sig_hash)
                        {
                            found = Some((slot, block.header.timestamp));
                            break;
                        }
                    }
                }
                found
                    .map(|(slot, ts)| (slot, ts, true))
                    .unwrap_or((0, 0, false))
            } else {
                (0, 0, false)
            }
        }
    };

    if !slot_indexed {
        return Ok(serde_json::Value::Null);
    }

    let tx = match tx {
        Some(tx) => tx,
        None => {
            // Fallback: look inside the block itself
            if let Ok(Some(block)) = state.state.get_block_by_slot(slot) {
                match block
                    .transactions
                    .iter()
                    .find(|t| t.signature() == sig_hash)
                {
                    Some(t) => t.clone(),
                    None => return Ok(serde_json::Value::Null),
                }
            } else {
                return Ok(serde_json::Value::Null);
            }
        }
    };

    if encoding == "base64" || encoding == "base58" {
        Ok(solana_transaction_encoded_json(
            &state.state,
            &tx,
            slot,
            timestamp,
            0,
            encoding,
        ))
    } else {
        Ok(solana_transaction_json(
            &state.state,
            &tx,
            slot,
            timestamp,
            0,
        ))
    }
}

async fn handle_solana_send_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [transaction, options?]".to_string(),
    })?;

    let tx_payload = params_array
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [transaction, options?]".to_string(),
        })?;

    let encoding = params_array
        .get(1)
        .and_then(|v| v.get("encoding"))
        .and_then(|v| v.as_str());

    let tx = decode_solana_transaction(tx_payload, encoding)?;

    preflight_transaction_submission(state, &tx, false).await?;

    let signature_hash = tx.signature();
    let signature_base58 = hash_to_base58(&signature_hash);

    let slot = state.state.get_last_slot().unwrap_or(0);
    let timestamp = state
        .state
        .get_block_by_slot(slot)
        .ok()
        .flatten()
        .map(|block| block.header.timestamp)
        .unwrap_or(0);

    // H15 fix: submit first, cache only on success (was caching before submit)
    submit_transaction(state, tx.clone())?;

    state.solana_tx_cache.write().await.put(
        signature_hash,
        SolanaTxRecord {
            tx,
            slot,
            timestamp,
            fee: 0,
        },
    );

    Ok(serde_json::json!(signature_base58))
}

/// Solana-compat: getTokenAccountsByOwner
/// Returns all MT-20 token accounts for a given owner in Solana SPL format.
async fn handle_solana_get_token_accounts_by_owner(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Expected array params [owner, filter?, config?]".to_string(),
    })?;
    let owner_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing owner address".to_string(),
        })?;
    let holder = Pubkey::from_base58(owner_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid owner: {}", e),
    })?;

    let second = arr.get(1).filter(|value| !value.is_null());
    let third = arr.get(2).filter(|value| !value.is_null());

    let (filter, config) = match second {
        Some(value) => {
            let object = value.as_object().ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: filter/config must be an object".to_string(),
            })?;
            if object.contains_key("mint") || object.contains_key("programId") {
                let config = third
                    .map(|value| {
                        value.as_object().ok_or_else(|| RpcError {
                            code: -32602,
                            message: "Invalid params: config must be an object".to_string(),
                        })
                    })
                    .transpose()?;
                (Some(object), config)
            } else {
                (None, Some(object))
            }
        }
        None => (
            None,
            third
                .map(|value| {
                    value.as_object().ok_or_else(|| RpcError {
                        code: -32602,
                        message: "Invalid params: config must be an object".to_string(),
                    })
                })
                .transpose()?,
        ),
    };

    let mint_filter = filter
        .and_then(|object| object.get("mint"))
        .map(|value| {
            value.as_str().ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: mint filter must be a string".to_string(),
            })
        })
        .transpose()?
        .map(|value| {
            Pubkey::from_base58(value).map_err(|e| RpcError {
                code: -32602,
                message: format!("Invalid mint filter: {}", e),
            })
        })
        .transpose()?;

    let program_id_filter = filter
        .and_then(|object| object.get("programId"))
        .map(|value| {
            value.as_str().ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: programId filter must be a string".to_string(),
            })
        })
        .transpose()?;

    if mint_filter.is_some() && program_id_filter.is_some() {
        return Err(RpcError {
            code: -32602,
            message: "Invalid params: expected either mint or programId filter".to_string(),
        });
    }

    if let Some(program_id_filter) = program_id_filter {
        if program_id_filter != SOLANA_SPL_TOKEN_PROGRAM_ID {
            return Ok(serde_json::json!({
                "context": solana_context(state)?,
                "value": [],
            }));
        }
    }

    let encoding = config
        .and_then(|object| object.get("encoding"))
        .and_then(|value| value.as_str())
        .unwrap_or("jsonParsed");
    validate_solana_token_account_encoding(encoding)?;

    let mut token_accounts = state
        .state
        .get_solana_token_accounts_by_owner(&holder, 100)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    if token_accounts.is_empty() {
        let legacy_balances = state
            .state
            .get_holder_token_balances(&holder, 100)
            .map_err(|e| RpcError {
                code: -32000,
                message: format!("Database error: {}", e),
            })?;

        for (token_program, _) in legacy_balances {
            let token_account = state
                .state
                .ensure_solana_token_account_binding(&token_program, &holder)
                .map_err(|e| RpcError {
                    code: -32000,
                    message: format!("Database error: {}", e),
                })?;
            token_accounts.push((token_account, token_program));
        }
    }

    let ctx = solana_context(state)?;
    let mut accounts: Vec<serde_json::Value> = Vec::new();

    for (token_account, token_program) in &token_accounts {
        if mint_filter
            .as_ref()
            .is_some_and(|mint| mint != token_program)
        {
            continue;
        }

        let balance = state
            .state
            .get_token_balance(token_program, &holder)
            .map_err(|e| RpcError {
                code: -32000,
                message: format!("Database error: {}", e),
            })?;

        let registry = state
            .state
            .get_symbol_registry_by_program(token_program)
            .ok()
            .flatten();

        let snapshot = SolanaTokenAccountSnapshot {
            token_account: *token_account,
            mint: *token_program,
            owner: holder,
            balance,
            decimals: token_registry_decimals(registry.as_ref()),
        };

        accounts.push(serde_json::json!({
            "pubkey": snapshot.token_account.to_base58(),
            "account": solana_token_account_response(&snapshot, encoding)?,
        }));
    }

    Ok(serde_json::json!({
        "context": ctx,
        "value": accounts,
    }))
}

/// Solana-compat: getTokenAccountBalance
/// Returns the balance of an SPL-like token account in Solana format.
async fn handle_solana_get_token_account_balance(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Expected array params [token_account]".to_string(),
    })?;

    let token_account_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing token account address".to_string(),
        })?;

    let token_account = Pubkey::from_base58(token_account_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid token account: {}", e),
    })?;

    let snapshot =
        load_solana_token_account_snapshot(state, &token_account)?.ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid param: could not find token account".to_string(),
        })?;
    let ctx = solana_context(state)?;

    Ok(serde_json::json!({
        "context": ctx,
        "value": {
            "amount": snapshot.balance.to_string(),
            "decimals": snapshot.decimals,
            "uiAmount": token_ui_amount(snapshot.balance, snapshot.decimals),
            "uiAmountString": token_ui_amount_string(snapshot.balance, snapshot.decimals),
        },
    }))
}

/// Get latest block
async fn handle_get_latest_block(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let block = state.state.get_block_by_slot(slot).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match block {
        Some(block) => {
            let block_hash = block.hash();
            Ok(serde_json::json!({
                "slot": block.header.slot,
                "hash": block_hash.to_hex(),
                "parent_hash": block.header.parent_hash.to_hex(),
                "state_root": block.header.state_root.to_hex(),
                "tx_root": block.header.tx_root.to_hex(),
                "timestamp": block.header.timestamp,
                "validator": Pubkey(block.header.validator).to_base58(),
                "transaction_count": block.transactions.len(),
            }))
        }
        None => Err(RpcError {
            code: -32001,
            message: "Latest block not found".to_string(),
        }),
    }
}

/// Get total burned spores
async fn handle_get_total_burned(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let burned = state.state.get_total_burned().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!({
        "spores": burned,
        "licn": burned as f64 / 1_000_000_000.0,
    }))
}

/// Get all validators
async fn handle_get_validators(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let validators = cached_validators(state).await?;
    let observer_now_ms = now_unix_ms();

    // Pre-compute total reputation once (was O(n²) inside the map loop)
    let total_reputation: u64 = validators.iter().map(|val| val.reputation).sum();

    let validator_list: Vec<_> = validators
        .iter()
        .filter(|v| should_expose_public_validator(state, v))
        .map(|v| {
            // Get stake + bootstrap info from StakePool (authoritative source)
            let (pool_stake, bootstrap_debt, vesting_status, earned_amount, graduation_slot) =
                if let Some(ref pool_arc) = state.stake_pool {
                    if let Ok(pool) = pool_arc.try_read() {
                        if let Some(s) = pool.get_stake(&v.pubkey) {
                            (
                                s.amount,
                                s.bootstrap_debt,
                                format!("{:?}", s.status),
                                s.earned_amount,
                                s.graduation_slot,
                            )
                        } else {
                            (0, 0, "Unknown".to_string(), 0, None)
                        }
                    } else {
                        (0, 0, "Unknown".to_string(), 0, None)
                    }
                } else {
                    (0, 0, "Unknown".to_string(), 0, None)
                };
            // Fallback to account staked field if pool has nothing
            let actual_stake = if pool_stake > 0 {
                pool_stake
            } else {
                state
                    .state
                    .get_account(&v.pubkey)
                    .ok()
                    .flatten()
                    .map(|acc| acc.staked)
                    .unwrap_or(0)
            };

            let normalized_reputation = if total_reputation > 0 {
                v.reputation as f64 / total_reputation as f64
            } else {
                0.0
            };

            serde_json::json!({
                "pubkey": v.pubkey.to_base58(),
                "stake": actual_stake,  // Use actual account balance
                "reputation": v.reputation as f64,
                "_normalized_reputation": normalized_reputation,
                "_blocks_produced": v.blocks_proposed,
                "blocks_proposed": v.blocks_proposed,
                "transactions_processed": v.transactions_processed,
                "votes_cast": v.votes_cast,
                "correct_votes": v.correct_votes,
                "last_active_slot": v.last_active_slot,
                "last_vote_slot": v.last_active_slot,
                "last_observed_at_ms": v.last_observed_at_ms,
                "last_observed_block_at_ms": v.last_observed_block_at_ms,
                "last_observed_block_slot": v.last_observed_block_slot,
                "head_staleness_ms": if v.last_observed_block_at_ms > 0 {
                    observer_now_ms.saturating_sub(v.last_observed_block_at_ms)
                } else {
                    0
                },
                "bootstrap_debt": bootstrap_debt,
                "vesting_status": vesting_status,
                "earned_amount": earned_amount,
                "graduation_slot": graduation_slot,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "validators": validator_list,
        "count": validator_list.len(),
        "_count": validator_list.len(),
    }))
}

/// Handle getMetrics — with per-slot response caching to avoid
/// redundant full-scan computation on hot polling paths.
const METRICS_CACHE_TTL_MS: u128 = 400;

async fn handle_get_metrics(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    // Fast path: return cached response if still fresh
    {
        let guard = state.metrics_cache.read().await;
        if guard.0.elapsed().as_millis() < METRICS_CACHE_TTL_MS {
            if let Some(ref cached) = guard.1 {
                return Ok(cached.clone());
            }
        }
    }

    // Slow path: compute then cache
    let result = compute_metrics(state).await?;

    {
        let mut guard = state.metrics_cache.write().await;
        *guard = (Instant::now(), Some(result.clone()));
    }
    Ok(result)
}

/// Inner metrics computation (expensive — calls multiple DB reads)
async fn compute_metrics(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let metrics = state.state.get_metrics();
    let validators = cached_validators(state).await?;
    let total_staked: u64 = if let Some(ref pool_arc) = state.stake_pool {
        if let Ok(pool) = pool_arc.try_read() {
            pool.total_stake()
        } else {
            0
        }
    } else {
        validators
            .iter()
            .map(|v| {
                state
                    .state
                    .get_account(&v.pubkey)
                    .ok()
                    .flatten()
                    .map(|acc| acc.staked)
                    .unwrap_or(0)
            })
            .sum()
    };

    // Calculate average transactions per block
    let avg_txs_per_block = if metrics.total_blocks > 0 {
        metrics.total_transactions as f64 / metrics.total_blocks as f64
    } else {
        0.0
    };

    // Treasury balance (dynamically from state, no hardcoded address)
    let (treasury_balance, treasury_pubkey_b58) = match state.state.get_treasury_pubkey() {
        Ok(Some(tpk)) => {
            let bal = state
                .state
                .get_account(&tpk)
                .ok()
                .flatten()
                .map(|a| a.spores)
                .unwrap_or(0);
            (bal, Some(tpk.to_base58()))
        }
        _ => (0u64, None),
    };

    // Genesis wallet balance (from state)
    let (genesis_balance, genesis_pubkey_b58) = match state.state.get_genesis_pubkey() {
        Ok(Some(gpk)) => {
            let bal = state
                .state
                .get_account(&gpk)
                .ok()
                .flatten()
                .map(|a| a.spores)
                .unwrap_or(0);
            (bal, Some(gpk.to_base58()))
        }
        _ => (0u64, None),
    };

    // Circulating supply = total_supply - genesis_reserve - burned - staked
    // RPC-03: Subtract staked amounts for accurate freely-tradeable supply
    // Note: unstaking queue amounts are included in circulating as they will be released
    let circulating_supply = metrics
        .total_supply
        .saturating_sub(genesis_balance)
        .saturating_sub(metrics.total_burned)
        .saturating_sub(total_staked);

    // Distribution wallet balances
    let dist_wallets_json = {
        let ga = state.state.get_genesis_accounts().unwrap_or_default();
        let mut dw_map = serde_json::Map::new();
        for (role, pubkey, _amount_licn, _pct) in &ga {
            let bal = state
                .state
                .get_account(pubkey)
                .ok()
                .flatten()
                .map(|a| a.spores)
                .unwrap_or(0);
            dw_map.insert(format!("{}_balance", role), serde_json::json!(bal));
            dw_map.insert(
                format!("{}_pubkey", role),
                serde_json::json!(pubkey.to_base58()),
            );
        }
        serde_json::Value::Object(dw_map)
    };

    // Fee config for burn percentage display
    let fee_config = state
        .state
        .get_fee_config()
        .unwrap_or_else(|_| lichen_core::FeeConfig::default_from_constants());
    let slot_duration_ms = state.state.get_slot_duration_ms();
    let cadence_target_ms = metrics.cadence_target_ms.max(slot_duration_ms.max(1));
    let slot_pace_pct = if cadence_target_ms > 0 && metrics.observed_block_interval_ms > 0 {
        ((cadence_target_ms as f64 / metrics.observed_block_interval_ms as f64) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u64
    } else {
        0
    };

    // Projected supply: include theoretical inflation accrued since last epoch boundary.
    // Actual minting happens at epoch boundaries, but this projection gives live feedback.
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let current_epoch = lichen_core::consensus::slot_to_epoch(current_slot);
    let epoch_start = lichen_core::consensus::epoch_start_slot(current_epoch);
    let slots_into_epoch = current_slot.saturating_sub(epoch_start);
    let per_slot_reward = compute_block_reward(current_slot, metrics.total_supply);
    let projected_unminted = per_slot_reward as u128 * slots_into_epoch as u128;
    let projected_supply = metrics
        .total_supply
        .saturating_add(projected_unminted as u64);

    Ok(serde_json::json!({
        "tps": metrics.tps,
        "peak_tps": metrics.peak_tps,
        "total_transactions": metrics.total_transactions,
        "daily_transactions": metrics.daily_transactions,
        "total_blocks": metrics.total_blocks,
        "average_block_time": metrics.average_block_time,
        "avg_block_time_ms": metrics.average_block_time * 1000.0,
        "observed_block_interval_ms": metrics.observed_block_interval_ms,
        "cadence_target_ms": cadence_target_ms,
        "slot_pace_pct": slot_pace_pct,
        "head_staleness_ms": metrics.head_staleness_ms,
        "cadence_samples": metrics.cadence_samples,
        "last_observed_block_slot": metrics.last_observed_block_slot,
        "last_observed_block_at_ms": metrics.last_observed_block_at_ms,
        "cadence_source": "observer_wall_clock",
        "avg_txs_per_block": avg_txs_per_block,
        "total_accounts": metrics.total_accounts,
        "active_accounts": metrics.active_accounts,
        "total_supply": metrics.total_supply,
        "projected_supply": projected_supply,
        "circulating_supply": circulating_supply,
        "total_burned": metrics.total_burned,
        "total_minted": metrics.total_minted,
        "total_staked": total_staked,
        "treasury_balance": treasury_balance,
        "treasury_pubkey": treasury_pubkey_b58,
        "genesis_balance": genesis_balance,
        "genesis_pubkey": genesis_pubkey_b58,
        "total_contracts": count_executable_accounts(&state.state),
        "validator_count": validators.len(),
        "distribution_wallets": dist_wallets_json,
        "slot_duration_ms": slot_duration_ms,
        "fee_burn_percent": fee_config.fee_burn_percent,
        "current_epoch": current_epoch,
        "slots_into_epoch": slots_into_epoch,
        "inflation_rate_bps": lichen_core::consensus::inflation_rate_bps(current_slot),
    }))
}

// ============================================================================
// GENESIS ACCOUNTS ENDPOINT
// ============================================================================

/// Get all genesis distribution accounts with live balances
async fn handle_get_genesis_accounts(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let accounts = state.state.get_genesis_accounts().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let mut result = Vec::new();

    // Add genesis wallet itself
    // RPC-06: amount_licn = original allocation at genesis (constant 1B);
    // current balance reflects actual holdings after distribution.
    if let Ok(Some(gpk)) = state.state.get_genesis_pubkey() {
        let acc = state.state.get_account(&gpk).ok().flatten();
        let bal = acc.as_ref().map(|a| a.spores).unwrap_or(0);
        result.push(serde_json::json!({
            "role": "genesis",
            "pubkey": gpk.to_base58(),
            "amount_licn": 1_000_000_000u64,
            "percentage": 100,
            "balance": bal,
            "label": "Genesis Signer",
        }));
    }

    // Add all distribution wallets
    for (role, pubkey, amount_licn, percentage) in &accounts {
        let acc = state.state.get_account(pubkey).ok().flatten();
        let bal = acc.as_ref().map(|a| a.spores).unwrap_or(0);
        let label = match role.as_str() {
            "validator_rewards" => "Validator Treasury",
            "community_treasury" => "Community Treasury",
            "builder_grants" => "Builder Grants",
            "founding_symbionts" => "Founding Symbionts",
            "ecosystem_partnerships" => "Ecosystem Partnerships",
            "reserve_pool" => "Reserve Pool",
            _ => role.as_str(),
        };
        result.push(serde_json::json!({
            "role": role,
            "pubkey": pubkey.to_base58(),
            "amount_licn": amount_licn,
            "percentage": percentage,
            "balance": bal,
            "label": label,
        }));
    }

    Ok(serde_json::json!({ "accounts": result }))
}

/// Get a governed wallet proposal by ID.
///
/// Params: [proposal_id] (integer)
async fn handle_get_governed_proposal(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params: expected [proposal_id]".to_string(),
    })?;

    let proposal_id = params
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_u64())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: proposal_id must be a positive integer".to_string(),
        })?;

    let proposal = state
        .state
        .get_governed_proposal(proposal_id)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32001,
            message: format!("Proposal {} not found", proposal_id),
        })?;

    Ok(serde_json::json!({
        "id": proposal.id,
        "source": proposal.source.to_base58(),
        "recipient": proposal.recipient.to_base58(),
        "amount": proposal.amount,
        "amount_licn": proposal.amount / 1_000_000_000,
        "approvals": proposal.approvals.iter().map(|p| p.to_base58()).collect::<Vec<_>>(),
        "threshold": proposal.threshold,
        "executed": proposal.executed,
    }))
}

// ============================================================================
// TREASURY ENDPOINT
// ============================================================================

/// Get treasury info (dynamic -- reads pubkey from state, no hardcoded address)
async fn handle_get_treasury_info(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let (treasury_pubkey, treasury_balance, treasury_staked) =
        match state.state.get_treasury_pubkey() {
            Ok(Some(tpk)) => {
                let acc = state.state.get_account(&tpk).ok().flatten();
                let bal = acc.as_ref().map(|a| a.spores).unwrap_or(0);
                let stk = acc.as_ref().map(|a| a.staked).unwrap_or(0);
                (Some(tpk.to_base58()), bal, stk)
            }
            _ => (None, 0u64, 0u64),
        };

    let (genesis_pubkey, genesis_balance, genesis_staked) = match state.state.get_genesis_pubkey() {
        Ok(Some(gpk)) => {
            let acc = state.state.get_account(&gpk).ok().flatten();
            let bal = acc.as_ref().map(|a| a.spores).unwrap_or(0);
            let stk = acc.as_ref().map(|a| a.staked).unwrap_or(0);
            (Some(gpk.to_base58()), bal, stk)
        }
        _ => (None, 0u64, 0u64),
    };

    Ok(serde_json::json!({
        "treasury_pubkey": treasury_pubkey,
        "treasury_balance": treasury_balance,
        "treasury_staked": treasury_staked,
        "genesis_pubkey": genesis_pubkey,
        "genesis_balance": genesis_balance,
        "genesis_staked": genesis_staked,
    }))
}

// ============================================================================
// NETWORK ENDPOINTS
// ============================================================================

/// Get connected peers
async fn handle_get_peers(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let (peer_count, peers) = if let Some(ref p2p) = state.p2p {
        let addresses = p2p.peer_addresses();
        let list: Vec<serde_json::Value> = addresses
            .iter()
            .map(|addr| {
                serde_json::json!({
                    "peer_id": addr,
                    "address": addr,
                    "connected": true,
                })
            })
            .collect();
        (addresses.len(), list)
    } else {
        (0, Vec::new())
    };

    Ok(serde_json::json!({
        "peers": peers,
        "count": peer_count,
    }))
}

/// Get live cluster info: connected peers with their validator identity & slot
/// This is the PRODUCTION endpoint for monitoring — no hardcoded ports needed.
async fn handle_get_cluster_info(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let observer_now_ms = now_unix_ms();

    // Get connected P2P peers
    let connected_peers: Vec<String> = if let Some(ref p2p) = state.p2p {
        p2p.peer_addresses()
    } else {
        Vec::new()
    };

    // Get all known validators from cache (refreshed per-slot)
    let validators = cached_validators(state).await?;

    // Build per-validator node info for the canonical public validator set.
    let public_validators: Vec<&ValidatorInfo> = validators
        .iter()
        .filter(|validator| should_expose_public_validator(state, validator))
        .collect();

    let nodes: Vec<serde_json::Value> = public_validators
        .iter()
        .map(|v| {
            let pool_stake = if let Some(ref pool_arc) = state.stake_pool {
                if let Ok(pool) = pool_arc.try_read() {
                    pool.get_stake(&v.pubkey).map(|s| s.amount).unwrap_or(0)
                } else {
                    0
                }
            } else {
                0
            };
            let actual_stake = if pool_stake > 0 {
                pool_stake
            } else {
                state
                    .state
                    .get_account(&v.pubkey)
                    .ok()
                    .flatten()
                    .map(|acc| acc.staked)
                    .unwrap_or(0)
            };

            let is_active = validator_is_active(current_slot, v.last_active_slot);

            serde_json::json!({
                "pubkey": v.pubkey.to_base58(),
                "stake": actual_stake,
                "reputation": v.reputation as f64,
                "blocks_proposed": v.blocks_proposed,
                "transactions_processed": v.transactions_processed,
                "last_active_slot": v.last_active_slot,
                "joined_slot": v.joined_slot,
                "active": is_active,
                "last_observed_at_ms": v.last_observed_at_ms,
                "last_observed_block_at_ms": v.last_observed_block_at_ms,
                "last_observed_block_slot": v.last_observed_block_slot,
                "head_staleness_ms": if v.last_observed_block_at_ms > 0 {
                    observer_now_ms.saturating_sub(v.last_observed_block_at_ms)
                } else {
                    0
                },
            })
        })
        .collect();

    Ok(serde_json::json!({
        "observer_time_ms": observer_now_ms,
        "current_slot": current_slot,
        "cluster_nodes": nodes,
        "connected_peers": connected_peers,
        "validator_count": public_validators.len(),
        "peer_count": connected_peers.len(),
    }))
}

/// Get network information
async fn handle_get_network_info(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let validators = cached_validators(state).await?;

    let peer_count = if let Some(ref p2p) = state.p2p {
        p2p.peer_count()
    } else {
        0
    };

    Ok(serde_json::json!({
        "chain_id": state.chain_id,
        "network_id": state.network_id,
        "version": state.version,
        "current_slot": current_slot,
        "validator_count": validators.len(),
        "peer_count": peer_count,
    }))
}

const VALIDATOR_ACTIVE_WINDOW_SLOTS: u64 = 100;

fn validator_is_active(current_slot: u64, last_active_slot: u64) -> bool {
    last_active_slot == 0
        || current_slot.saturating_sub(last_active_slot) <= VALIDATOR_ACTIVE_WINDOW_SLOTS
}

// ============================================================================
// VALIDATOR ENDPOINTS
// ============================================================================

/// Get detailed validator information
async fn handle_get_validator_info(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|_| invalid_pubkey_format_error())?;

    let validator = load_validator_info(state, &pubkey)
        .await?
        .ok_or_else(|| RpcError {
            code: -32001,
            message: "Validator not found".to_string(),
        })?;

    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let is_active = validator_is_active(current_slot, validator.last_active_slot);

    Ok(serde_json::json!({
        "pubkey": validator.pubkey.to_base58(),
        "stake": validator.stake,
        "reputation": validator.reputation,
        "blocks_proposed": validator.blocks_proposed,
        "transactions_processed": validator.transactions_processed,
        "votes_cast": validator.votes_cast,
        "correct_votes": validator.correct_votes,
        "last_active_slot": validator.last_active_slot,
        "last_observed_at_ms": validator.last_observed_at_ms,
        "last_observed_block_at_ms": validator.last_observed_block_at_ms,
        "last_observed_block_slot": validator.last_observed_block_slot,
        "head_staleness_ms": if validator.last_observed_block_at_ms > 0 {
            now_unix_ms().saturating_sub(validator.last_observed_block_at_ms)
        } else {
            0
        },
        "joined_slot": validator.joined_slot,
        "commission_rate": validator.commission_rate,
        "is_active": is_active,
    }))
}

/// Get validator performance metrics
async fn handle_get_validator_performance(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|_| invalid_pubkey_format_error())?;

    let validator = load_validator_info(state, &pubkey)
        .await?
        .ok_or_else(|| RpcError {
            code: -32001,
            message: "Validator not found".to_string(),
        })?;

    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let vote_accuracy = if validator.votes_cast > 0 {
        (validator.correct_votes as f64 / validator.votes_cast as f64) * 100.0
    } else {
        0.0
    };

    // T2.15 fix: Calculate uptime from blocks_proposed (concrete metric)
    // instead of the misleading last_active_slot delta.
    // blocks_proposed / slots_since_joined gives the fraction of slots
    // where this validator actually produced a block.
    let slots_since_joined = current_slot.saturating_sub(validator.joined_slot);
    let uptime = if slots_since_joined > 0 {
        (validator.blocks_proposed as f64 / slots_since_joined as f64 * 100.0).min(100.0)
    } else {
        100.0 // Just joined, assume 100%
    };

    Ok(serde_json::json!({
        "pubkey": validator.pubkey.to_base58(),
        "blocks_proposed": validator.blocks_proposed,
        "transactions_processed": validator.transactions_processed,
        "votes_cast": validator.votes_cast,
        "correct_votes": validator.correct_votes,
        "vote_accuracy": vote_accuracy,
        "reputation": validator.reputation,
        "uptime": uptime,
    }))
}

/// Get comprehensive chain status
async fn handle_get_chain_status(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let validators = cached_validators(state).await?;

    let total_stake: u64 = if let Some(ref pool_arc) = state.stake_pool {
        if let Ok(pool) = pool_arc.try_read() {
            pool.total_stake()
        } else {
            validators.iter().map(|v| v.stake).sum()
        }
    } else {
        validators.iter().map(|v| v.stake).sum()
    };
    let metrics = state.state.get_metrics();

    // Calculate epoch from consensus constant (SLOTS_PER_EPOCH = 432,000)
    let epoch = lichen_core::consensus::slot_to_epoch(current_slot);
    // Block height is same as slot for now (1 block per slot)
    let block_height = current_slot;

    // Projected supply: include theoretical inflation accrued since last epoch
    let epoch_start = lichen_core::consensus::epoch_start_slot(epoch);
    let slots_into_epoch = current_slot.saturating_sub(epoch_start);
    let per_slot_reward = compute_block_reward(current_slot, metrics.total_supply);
    let projected_unminted = per_slot_reward as u128 * slots_into_epoch as u128;
    let projected_supply = metrics
        .total_supply
        .saturating_add(projected_unminted as u64);

    // Check chain health: stale if no block in 120 seconds
    let is_healthy = if let Ok(Some(block)) = state.state.get_block_by_slot(current_slot) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(block.header.timestamp) <= 120
    } else {
        false
    };

    Ok(serde_json::json!({
        "slot": current_slot,
        "_slot": current_slot,
        "epoch": epoch,
        "_epoch": epoch,
        "block_height": block_height,
        "_block_height": block_height,
        "current_slot": current_slot,
        "latest_block": block_height,
        "validator_count": validators.len(),
        "validators": validators.len(),
        "_validators": validators.len(),
        "total_stake": total_stake,
        "total_staked": total_stake,
        "tps": metrics.tps,
        "peak_tps": metrics.peak_tps,
        "total_transactions": metrics.total_transactions,
        "total_blocks": metrics.total_blocks,
        "average_block_time": metrics.average_block_time,
        "block_time_ms": metrics.average_block_time * 1000.0,
        "total_supply": metrics.total_supply,
        "projected_supply": projected_supply,
        "total_burned": metrics.total_burned,
        "total_minted": metrics.total_minted,
        "peer_count": if let Some(ref p2p) = state.p2p { p2p.peer_count() } else { 0 },
        "chain_id": state.chain_id,
        "network": state.network_id,
        "is_healthy": is_healthy,
        "inflation_rate_bps": lichen_core::consensus::inflation_rate_bps(current_slot),
    }))
}

// ============================================================================
// STAKING ENDPOINTS
// ============================================================================

/// Create stake transaction
async fn handle_stake(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected array".to_string(),
    })?;

    if arr.len() == 1 {
        let tx_base64 = arr
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: expected [transaction_base64]".to_string(),
            })?;

        use base64::{engine::general_purpose, Engine as _};
        let tx_bytes = general_purpose::STANDARD
            .decode(tx_base64)
            .map_err(|e| RpcError {
                code: -32602,
                message: format!("Invalid base64: {}", e),
            })?;

        let tx: Transaction = decode_transaction_bytes(&tx_bytes)?;

        let instruction = tx.message.instructions.first().ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid transaction: missing instructions".to_string(),
        })?;

        if instruction.program_id != SYSTEM_PROGRAM_ID || instruction.data.first() != Some(&9) {
            return Err(RpcError {
                code: -32602,
                message: "Invalid stake transaction: expected system opcode 9".to_string(),
            });
        }

        preflight_transaction_submission(state, &tx, false).await?;

        let signature = submit_transaction(state, tx)?;
        return Ok(serde_json::json!(signature));
    }

    Err(RpcError {
        code: -32602,
        message: "Unsupported params: submit signed transaction via sendTransaction or stake([tx_base64])".to_string(),
    })
}

/// Create unstake transaction
async fn handle_unstake(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected array".to_string(),
    })?;

    if arr.len() == 1 {
        let tx_base64 = arr
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: expected [transaction_base64]".to_string(),
            })?;

        use base64::{engine::general_purpose, Engine as _};
        let tx_bytes = general_purpose::STANDARD
            .decode(tx_base64)
            .map_err(|e| RpcError {
                code: -32602,
                message: format!("Invalid base64: {}", e),
            })?;

        let tx: Transaction = decode_transaction_bytes(&tx_bytes)?;

        let instruction = tx.message.instructions.first().ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid transaction: missing instructions".to_string(),
        })?;

        if instruction.program_id != SYSTEM_PROGRAM_ID || instruction.data.first() != Some(&10) {
            return Err(RpcError {
                code: -32602,
                message: "Invalid unstake transaction: expected system opcode 10".to_string(),
            });
        }

        preflight_transaction_submission(state, &tx, false).await?;

        let signature = submit_transaction(state, tx)?;
        return Ok(serde_json::json!(signature));
    }

    Err(RpcError {
        code: -32602,
        message: "Unsupported params: submit signed transaction via sendTransaction or unstake([tx_base64])".to_string(),
    })
}

/// Get staking status for an account
async fn handle_get_staking_status(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|_| invalid_pubkey_format_error())?;

    // Check if this is a validator
    let validator_info = load_validator_info(state, &pubkey).await?;

    if let Some(validator) = validator_info {
        let (
            live_stake,
            bootstrap_debt,
            bootstrap_index,
            earned_amount,
            total_debt_repaid,
            vesting_status,
            start_slot,
            graduation_slot,
        ) = if let Some(ref pool_arc) = state.stake_pool {
            if let Ok(pool) = pool_arc.try_read() {
                if let Some(s) = pool.get_stake(&pubkey) {
                    (
                        s.amount,
                        s.bootstrap_debt,
                        s.bootstrap_index,
                        s.earned_amount,
                        s.total_debt_repaid,
                        format!("{:?}", s.status),
                        s.start_slot,
                        s.graduation_slot,
                    )
                } else {
                    (
                        validator.stake,
                        0,
                        u64::MAX,
                        0,
                        0,
                        "Unknown".to_string(),
                        0,
                        None,
                    )
                }
            } else {
                (
                    validator.stake,
                    0,
                    u64::MAX,
                    0,
                    0,
                    "Unknown".to_string(),
                    0,
                    None,
                )
            }
        } else {
            (
                validator.stake,
                0,
                u64::MAX,
                0,
                0,
                "Unknown".to_string(),
                0,
                None,
            )
        };
        Ok(serde_json::json!({
            "is_validator": true,
            "total_staked": live_stake,
            "delegations": [],
            "status": "active",
            "bootstrap_debt": bootstrap_debt,
            "bootstrap_index": bootstrap_index,
            "earned_amount": earned_amount,
            "total_debt_repaid": total_debt_repaid,
            "vesting_status": vesting_status,
            "start_slot": start_slot,
            "graduation_slot": graduation_slot,
        }))
    } else {
        Ok(serde_json::json!({
            "is_validator": false,
            "total_staked": 0,
            "delegations": [],
            "status": "inactive",
        }))
    }
}

/// Get staking rewards for an account
async fn handle_get_staking_rewards(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|_| invalid_pubkey_format_error())?;

    // Get staking rewards from stake pool
    if let Some(ref pool) = state.stake_pool {
        let pool_guard = pool.read().await;
        if let Some(stake_info) = pool_guard.get_stake(&pubkey) {
            // total_claimed tracks all historically claimed rewards (liquid + debt)
            // rewards_earned is the currently pending (unclaimed) buffer
            let liquid_claimed = stake_info
                .total_claimed
                .saturating_sub(stake_info.total_debt_repaid);
            let total_claimed = stake_info.total_claimed;
            let total_earned = total_claimed + stake_info.rewards_earned;
            let pending = stake_info.rewards_earned;
            let claimed = liquid_claimed;

            // Epoch-based reward projection: compute this validator's estimated
            // share of the next epoch distribution based on current stake weight.
            let current_slot = state.state.get_last_slot().unwrap_or(0);
            let total_supply = GENESIS_SUPPLY_SPORES
                .saturating_add(state.state.get_total_minted().unwrap_or(0))
                .saturating_sub(state.state.get_total_burned().unwrap_or(0));

            let current_epoch = lichen_core::consensus::slot_to_epoch(current_slot);
            let epoch_start = lichen_core::consensus::epoch_start_slot(current_epoch);
            let slots_into_epoch = current_slot.saturating_sub(epoch_start);
            let total_pool_stake = pool_guard.total_stake().max(1);
            let validator_stake = stake_info.total_stake();
            let stake_share = validator_stake as f64 / total_pool_stake as f64;

            // Projected pending: this validator's proportional share of inflation
            // that has theoretically accrued since the current epoch started.
            let per_slot_reward = compute_block_reward(current_slot, total_supply);
            let epoch_accrued = per_slot_reward as u128 * slots_into_epoch as u128;
            let projected_pending = (epoch_accrued as f64 * stake_share) as u64;

            // Full epoch projection (what they'd earn at next boundary)
            let epoch_mint = lichen_core::consensus::compute_epoch_mint(epoch_start, total_supply);
            let projected_epoch_reward = (epoch_mint as f64 * stake_share) as u64;

            let current_reward = per_slot_reward;
            let base_rate_licn = current_reward as f64 / 1_000_000_000.0;
            let reward_rate = if stake_info.is_active {
                if stake_info.bootstrap_debt > 0 {
                    // During vesting: 50% goes to debt repayment, 50% liquid
                    format!("{:.4}", base_rate_licn / 2.0)
                } else {
                    format!("{:.4}", base_rate_licn)
                }
            } else {
                "0".to_string()
            };

            let vesting_progress = {
                let total = (stake_info.earned_amount as f64) + (stake_info.bootstrap_debt as f64);
                if total == 0.0 {
                    1.0
                } else {
                    stake_info.earned_amount as f64 / total
                }
            };

            return Ok(serde_json::json!({
                "total_rewards": total_earned,
                "pending_rewards": pending,
                "projected_pending": projected_pending,
                "projected_epoch_reward": projected_epoch_reward,
                "claimed_rewards": claimed,
                "liquid_claimed_rewards": liquid_claimed,
                "claimed_total_rewards": total_claimed,
                "reward_rate": reward_rate,
                "bootstrap_debt": stake_info.bootstrap_debt,
                "earned_amount": stake_info.earned_amount,
                "vesting_progress": vesting_progress,
                "blocks_produced": stake_info.blocks_produced,
                "total_debt_repaid": stake_info.total_debt_repaid,
            }));
        }
    }

    // No staking found
    Ok(serde_json::json!({
        "total_rewards": 0,
        "pending_rewards": 0,
        "claimed_rewards": 0,
        "liquid_claimed_rewards": 0,
        "claimed_total_rewards": 0,
        "reward_rate": 0.0,
    }))
}

// ============================================================================
// ACCOUNT ENDPOINTS
// ============================================================================

/// Get enhanced account information
async fn handle_get_account_info(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|_| invalid_pubkey_format_error())?;

    let account = state.state.get_account(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let balance = state.state.get_balance(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    // Check if it's a validator
    let is_validator = load_validator_info(state, &pubkey).await?.is_some();

    Ok(serde_json::json!({
        "pubkey": pubkey.to_base58(),
        "balance": balance,
        "licn": balance as f64 / 1_000_000_000.0,
        "exists": account.is_some(),
        "is_validator": is_validator,
        "is_executable": account.as_ref().map(|a| a.executable).unwrap_or(false),
    }))
}

// ============================================================================
// CONTRACT ENDPOINTS
// ============================================================================

/// Get contract information
async fn handle_get_contract_info(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let contract_id_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [contract_id]".to_string(),
        })?;

    let contract_id = Pubkey::from_base58(contract_id_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid contract ID: {}", e),
    })?;

    let account = state
        .state
        .get_account(&contract_id)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "Contract not found".to_string(),
    })?;

    // Try to parse ContractAccount to get rich metadata
    let (has_abi, abi_functions, code_hash, owner_b58, token_metadata) = if account.executable {
        if let Ok(ca) = serde_json::from_slice::<lichen_core::ContractAccount>(&account.data) {
            let func_count = ca.abi.as_ref().map(|a| a.functions.len()).unwrap_or(0);
            let abi_fn_names: Vec<String> = ca
                .abi
                .as_ref()
                .map(|a| a.functions.iter().map(|f| f.name.clone()).collect())
                .unwrap_or_default();

            // Extract MT-20 token metadata from contract storage + registry.
            //
            // All tokens use prefixed keys: {prefix}_supply (e.g. licn_supply, wbnb_supply).
            // Primary source: symbol registry entry (has name, symbol, decimals).
            // Supply value: read directly via {symbol_lowercase}_supply key.
            let mut tmeta = serde_json::Map::new();

            // Look up registry entry for this contract to get the prefix
            let reg_entry = state
                .state
                .get_symbol_registry_by_program(&contract_id)
                .ok()
                .flatten();
            if let Some(ref entry) = reg_entry {
                let prefix = entry.symbol.to_lowercase();
                let supply_key = format!("{}_supply", prefix);
                if let Ok(Some(v)) = state
                    .state
                    .get_contract_storage(&contract_id, supply_key.as_bytes())
                {
                    if v.len() == 8 {
                        let supply =
                            u64::from_le_bytes([v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]]);
                        tmeta.insert("total_supply".to_string(), serde_json::json!(supply));
                    }
                }
                if !tmeta.contains_key("total_supply") {
                    if let Some(v) = ca.storage.get(supply_key.as_bytes()) {
                        if v.len() == 8 {
                            let supply = u64::from_le_bytes([
                                v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7],
                            ]);
                            tmeta.insert("total_supply".to_string(), serde_json::json!(supply));
                        }
                    }
                }
                // Fallback: if supply not found in storage, check registry metadata
                if !tmeta.contains_key("total_supply") {
                    if let Some(ref meta) = entry.metadata {
                        if let Some(v) = meta.get("total_supply") {
                            // Accept both number and string representation
                            if let Some(n) = v.as_u64() {
                                tmeta.insert("total_supply".to_string(), serde_json::json!(n));
                            } else if let Some(s) = v.as_str() {
                                if let Ok(n) = s.parse::<u64>() {
                                    tmeta.insert("total_supply".to_string(), serde_json::json!(n));
                                }
                            }
                        }
                    }
                }
                if !tmeta.contains_key("total_supply")
                    && abi_fn_names.iter().any(|name| name == "total_supply")
                {
                    if let Ok(result) = execute_readonly_contract_call(
                        state,
                        contract_id,
                        &ca,
                        Pubkey::new([0u8; 32]),
                        "total_supply",
                        Vec::new(),
                    ) {
                        if result.success {
                            if let Some(supply) = decode_contract_result_u64(&result) {
                                tmeta.insert("total_supply".to_string(), serde_json::json!(supply));
                            }
                        }
                    }
                }
                // Preserve the full live registry profile metadata in token_metadata.
                if let Some(ref meta) = entry.metadata {
                    if let Some(obj) = meta.as_object() {
                        for (key, value) in obj {
                            tmeta.entry(key.clone()).or_insert_with(|| value.clone());
                        }
                    }
                }
                if let Some(decimals) = entry.decimals {
                    tmeta.insert("decimals".to_string(), serde_json::json!(decimals));
                } else if let Some(ref meta) = entry.metadata {
                    if let Some(v) = meta.get("decimals") {
                        tmeta.insert("decimals".to_string(), v.clone());
                    }
                }
                if let Some(ref name) = entry.name {
                    if !name.trim().is_empty() {
                        tmeta.insert("token_name".to_string(), serde_json::json!(name));
                        tmeta
                            .entry("name".to_string())
                            .or_insert_with(|| serde_json::json!(name));
                    }
                } else if let Some(ref meta) = entry.metadata {
                    if let Some(v) = meta.get("name") {
                        tmeta.insert("token_name".to_string(), v.clone());
                        tmeta.entry("name".to_string()).or_insert_with(|| v.clone());
                    }
                }
                tmeta.insert("token_symbol".to_string(), serde_json::json!(&entry.symbol));
            }

            // Detect mintable/burnable from ABI function names
            let has_mint = abi_fn_names.iter().any(|n| n == "mint");
            let has_burn = abi_fn_names.iter().any(|n| n == "burn");
            tmeta.insert("mintable".to_string(), serde_json::json!(has_mint));
            tmeta.insert("burnable".to_string(), serde_json::json!(has_burn));

            let token_meta = if tmeta.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(tmeta))
            };

            (
                ca.abi.is_some(),
                func_count,
                ca.code_hash.to_hex(),
                ca.owner.to_base58(),
                token_meta,
            )
        } else {
            (false, 0, String::new(), account.owner.to_base58(), None)
        }
    } else {
        (false, 0, String::new(), account.owner.to_base58(), None)
    };

    // Extract version + previous_code_hash when available
    let (contract_version, prev_code_hash) = if account.executable {
        if let Ok(ca) = serde_json::from_slice::<lichen_core::ContractAccount>(&account.data) {
            (ca.version, ca.previous_code_hash.map(|h| h.to_hex()))
        } else {
            (1u32, None)
        }
    } else {
        (1u32, None)
    };

    let mut result = serde_json::json!({
        "contract_id": contract_id.to_base58(),
        "owner": owner_b58,
        "code_size": account.data.len(),
        "is_executable": account.executable,
        "has_abi": has_abi,
        "abi_functions": abi_functions,
        "code_hash": code_hash,
        "deployed_at": 0,
        "version": contract_version,
    });
    if let Some(pch) = prev_code_hash {
        if let Some(obj) = result.as_object_mut() {
            obj.insert("previous_code_hash".to_string(), serde_json::json!(pch));
        }
    }
    if let Some(tm) = token_metadata {
        if let Some(obj) = result.as_object_mut() {
            obj.insert("token_metadata".to_string(), tm);
        }
    }

    // Enrich with registry metadata (is_native flag)
    if let Ok(Some(reg)) = state.state.get_symbol_registry_by_program(&contract_id) {
        if let Some(reg_meta) = &reg.metadata {
            if let Some(rm) = result.as_object_mut() {
                if reg_meta.get("is_native").and_then(|v| v.as_bool()) == Some(true) {
                    rm.insert("is_native".to_string(), serde_json::json!(true));
                }
            }
        }
    }

    Ok(result)
}

/// Get contract execution logs
async fn handle_get_contract_logs(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected array".to_string(),
    })?;

    let contract_id_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [contract_id, limit?]".to_string(),
        })?;

    let limit = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(100).min(1000) as usize; // AUDIT-FIX 2.16

    let before_slot = arr.get(2).and_then(|v| v.as_u64());

    let contract_id = Pubkey::from_base58(contract_id_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid contract ID: {}", e),
    })?;

    let events = state
        .state
        .get_contract_logs(&contract_id, limit, before_slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let logs: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "program": e.program.to_base58(),
                "name": e.name,
                "data": e.data,
                "slot": e.slot,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "logs": logs,
        "count": logs.len(),
    }))
}

/// Get contract ABI/IDL
async fn handle_get_contract_abi(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let contract_id_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [contract_id]".to_string(),
        })?;

    let contract_id = Pubkey::from_base58(contract_id_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid contract ID: {}", e),
    })?;

    let account = state
        .state
        .get_account(&contract_id)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "Contract not found".to_string(),
    })?;

    if !account.executable {
        return Err(RpcError {
            code: -32001,
            message: "Account is not a contract".to_string(),
        });
    }

    let contract: lichen_core::ContractAccount =
        serde_json::from_slice(&account.data).map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to decode contract: {}", e),
        })?;

    match contract.abi {
        Some(abi) => Ok(serde_json::to_value(&abi).unwrap_or_default()),
        None => Ok(serde_json::json!({
            "error": "No ABI available for this contract",
            "hint": "Deploy with an ABI in init_data, or use setContractAbi"
        })),
    }
}

/// Set/update contract ABI (admin-only — requires admin_token)
async fn handle_set_contract_abi(
    state: &RpcState,
    params: Option<serde_json::Value>,
    auth_header: Option<&str>,
) -> Result<serde_json::Value, RpcError> {
    require_legacy_admin_rpc_enabled(state, "setContractAbi")?;
    // H16 fix: reject in multi-validator mode (direct state write bypasses consensus)
    require_single_validator(state, "setContractAbi").await?;
    verify_admin_auth(state, auth_header)?;

    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [contract_id, abi_json]".to_string(),
    })?;

    let contract_id_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing contract_id".to_string(),
        })?;

    let abi_value = arr.get(1).ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing ABI JSON".to_string(),
    })?;

    let contract_id = Pubkey::from_base58(contract_id_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid contract ID: {}", e),
    })?;

    let abi: lichen_core::ContractAbi =
        serde_json::from_value(abi_value.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid ABI format: {}", e),
        })?;

    let mut account = state
        .state
        .get_account(&contract_id)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32001,
            message: "Contract not found".to_string(),
        })?;

    if !account.executable {
        return Err(RpcError {
            code: -32001,
            message: "Account is not a contract".to_string(),
        });
    }

    let mut contract: lichen_core::ContractAccount = serde_json::from_slice(&account.data)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to decode contract: {}", e),
        })?;

    contract.abi = Some(abi);
    account.data = serde_json::to_vec(&contract).map_err(|e| RpcError {
        code: -32000,
        message: format!("Failed to serialize contract: {}", e),
    })?;

    state
        .state
        .put_account(&contract_id, &account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    log_privileged_rpc_mutation(
        "setContractAbi",
        "legacy_admin",
        "admin_token",
        "contract_abi",
        Some(contract_id_str),
        serde_json::json!({
            "abi_functions": contract.abi.as_ref().map(|abi| abi.functions.len()).unwrap_or(0),
        }),
    );

    Ok(serde_json::json!({
        "success": true,
        "contract": contract_id.to_base58(),
        "abi_functions": contract.abi.as_ref().map(|a| a.functions.len()).unwrap_or(0),
    }))
}

/// Get all deployed contracts
fn parse_program_list_pagination(
    params: Option<serde_json::Value>,
    default_limit: u64,
    max_limit: u64,
) -> Result<(usize, Option<Pubkey>), RpcError> {
    let mut limit = default_limit;
    let mut cursor_str: Option<String> = None;

    if let Some(value) = params {
        if let Some(obj) = value.as_object() {
            if let Some(v) = obj.get("limit").and_then(|v| v.as_u64()) {
                limit = v;
            }
            cursor_str = obj
                .get("cursor")
                .and_then(|v| v.as_str())
                .or_else(|| obj.get("after").and_then(|v| v.as_str()))
                .or_else(|| obj.get("after_program").and_then(|v| v.as_str()))
                .map(|s| s.to_string());
        } else if let Some(arr) = value.as_array() {
            if let Some(first) = arr.first() {
                if let Some(v) = first.as_u64() {
                    limit = v;
                } else if let Some(obj) = first.as_object() {
                    if let Some(v) = obj.get("limit").and_then(|v| v.as_u64()) {
                        limit = v;
                    }
                    cursor_str = obj
                        .get("cursor")
                        .and_then(|v| v.as_str())
                        .or_else(|| obj.get("after").and_then(|v| v.as_str()))
                        .or_else(|| obj.get("after_program").and_then(|v| v.as_str()))
                        .map(|s| s.to_string());
                } else if let Some(s) = first.as_str() {
                    cursor_str = Some(s.to_string());
                }
            }
            if let Some(second) = arr.get(1) {
                if let Some(obj) = second.as_object() {
                    if let Some(v) = obj.get("limit").and_then(|v| v.as_u64()) {
                        limit = v;
                    }
                    if cursor_str.is_none() {
                        cursor_str = obj
                            .get("cursor")
                            .and_then(|v| v.as_str())
                            .or_else(|| obj.get("after").and_then(|v| v.as_str()))
                            .or_else(|| obj.get("after_program").and_then(|v| v.as_str()))
                            .map(|s| s.to_string());
                    }
                } else if cursor_str.is_none() {
                    if let Some(s) = second.as_str() {
                        cursor_str = Some(s.to_string());
                    }
                }
            }
        } else if let Some(v) = value.as_u64() {
            limit = v;
        } else if let Some(s) = value.as_str() {
            cursor_str = Some(s.to_string());
        }
    }

    let limit = limit.clamp(1, max_limit) as usize;
    let after = if let Some(cursor) = cursor_str {
        Some(Pubkey::from_base58(cursor.trim()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid cursor/after pubkey: {}", e),
        })?)
    } else {
        None
    };

    Ok((limit, after))
}

async fn handle_get_all_contracts(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    if let Some(cached) = get_cached_program_list_response(state, "getAllContracts", &params).await
    {
        return Ok(cached);
    }

    let (limit, after) = parse_program_list_pagination(params.clone(), 100, 1000)?;
    let fetch_limit = limit.saturating_add(1);

    let mut programs = state
        .state
        .get_all_programs_paginated(fetch_limit, after.as_ref())
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;
    let has_more = programs.len() > limit;
    if has_more {
        programs.truncate(limit);
    }

    let registry_entries = state
        .state
        .get_all_symbol_registry(5000)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let registry_by_program: HashMap<Pubkey, (String, Option<String>, String, Option<String>)> =
        registry_entries
            .into_iter()
            .map(|entry| {
                (
                    entry.program,
                    (
                        entry.symbol,
                        entry.name,
                        entry.owner.to_base58(),
                        entry.template,
                    ),
                )
            })
            .collect();

    let contracts: Vec<serde_json::Value> = programs
        .iter()
        .map(|(pk, metadata)| {
            let (symbol, name, owner, template) = registry_by_program
                .get(pk)
                .map(|(symbol, name, owner, template)| {
                    (
                        Some(symbol.clone()),
                        name.clone(),
                        Some(owner.clone()),
                        template.clone(),
                    )
                })
                .unwrap_or((None, None, None, None));
            serde_json::json!({
                "program_id": pk.to_base58(),
                "symbol": symbol,
                "name": name,
                "owner": owner,
                "template": template,
                "metadata": metadata,
            })
        })
        .collect();

    let next_cursor = if has_more {
        programs.last().map(|(pk, _)| pk.to_base58())
    } else {
        None
    };

    let response = serde_json::json!({
        "contracts": contracts,
        "count": contracts.len(),
        "has_more": has_more,
        "next_cursor": next_cursor,
    });

    put_cached_program_list_response(state, "getAllContracts", &params, response.clone()).await;

    Ok(response)
}

/// Deploy a contract via RPC (bypasses transaction instruction size limit).
///
/// Params: [deployer_base58, code_base64, init_data_json_or_null, signature_hex]
///
/// The deployer signs SHA-256(code_bytes) with their native PQ key.
/// Deploy fee (2.5 LICN) is charged from the deployer's account.
/// Contract address is derived as SHA-256(deployer_pubkey + code_bytes).
async fn handle_deploy_contract(
    state: &RpcState,
    params: Option<serde_json::Value>,
    auth_header: Option<&str>,
) -> Result<serde_json::Value, RpcError> {
    use base64::{engine::general_purpose, Engine as _};
    use lichen_core::account::Keypair as LichenKeypair;
    use sha2::{Digest, Sha256};

    require_legacy_admin_rpc_enabled(state, "deployContract")?;
    // H16 fix: reject in multi-validator mode (direct state write bypasses consensus)
    require_single_validator(state, "deployContract").await?;

    // Admin-gate: contract deployment requires admin authentication
    verify_admin_auth(state, auth_header)?;

    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Params must be an array: [deployer, code_base64, init_data, signature]"
            .to_string(),
    })?;

    if arr.len() < 4 {
        return Err(RpcError {
            code: -32602,
            message:
                "Expected [deployer_base58, code_base64, init_data_json_or_null, signature_hex]"
                    .to_string(),
        });
    }

    // Parse deployer pubkey
    let deployer_str = arr[0].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "deployer must be a base58 string".to_string(),
    })?;
    let deployer_pubkey = Pubkey::from_base58(deployer_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid deployer pubkey: {}", e),
    })?;

    // Parse WASM code (base64)
    let code_b64 = arr[1].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "code must be a base64 string".to_string(),
    })?;
    let code_bytes = general_purpose::STANDARD
        .decode(code_b64)
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid base64 code: {}", e),
        })?;

    if code_bytes.is_empty() {
        return Err(RpcError {
            code: -32602,
            message: "Code cannot be empty".to_string(),
        });
    }

    // Parse init_data (optional JSON)
    let init_data_bytes: Vec<u8> = if arr[2].is_null() {
        vec![]
    } else if let Some(s) = arr[2].as_str() {
        s.as_bytes().to_vec()
    } else {
        // If it's a JSON object, serialize it
        serde_json::to_vec(&arr[2]).unwrap_or_default()
    };

    let signature = parse_pq_signature_value(&arr[3])?;

    // Verify signature: deployer must sign SHA-256(code_bytes)
    let mut hasher = Sha256::new();
    hasher.update(&code_bytes);
    let code_hash = hasher.finalize();
    if !LichenKeypair::verify(&deployer_pubkey, &code_hash, &signature) {
        return Err(RpcError {
            code: -32003,
            message: "Invalid signature: deployer must sign SHA-256(code)".to_string(),
        });
    }

    // Derive program address: SHA-256(deployer + name + code)
    // Including the name/symbol ensures identical WASMs (e.g. wrapped token stubs)
    // get unique addresses — matches genesis derivation in validator/src/main.rs.
    let contract_name: Option<String> = if !init_data_bytes.is_empty() {
        serde_json::from_slice::<serde_json::Value>(&init_data_bytes)
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
    addr_hasher.update(deployer_pubkey.0);
    if let Some(ref name) = contract_name {
        addr_hasher.update(name.as_bytes());
    }
    addr_hasher.update(&code_bytes);
    let addr_hash = addr_hasher.finalize();
    let mut addr_bytes = [0u8; 32];
    addr_bytes.copy_from_slice(&addr_hash[..32]);
    let program_pubkey = Pubkey(addr_bytes);

    // Check if already deployed
    if let Ok(Some(_)) = state.state.get_account(&program_pubkey) {
        return Err(RpcError {
            code: -32000,
            message: format!("Contract already exists at {}", program_pubkey.to_base58()),
        });
    }

    // Charge deploy fee (2.5 LICN)
    let deploy_fee = lichen_core::CONTRACT_DEPLOY_FEE;
    let deployer_account = state
        .state
        .get_account(&deployer_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32000,
            message: "Deployer account not found".to_string(),
        })?;

    if deployer_account.spendable < deploy_fee {
        return Err(RpcError {
            code: -32000,
            message: format!(
                "Insufficient spendable balance: need {} spores ({:.1} LICN), have {} spendable ({:.1} LICN)",
                deploy_fee,
                deploy_fee as f64 / 1_000_000_000.0,
                deployer_account.spendable,
                deployer_account.spendable as f64 / 1_000_000_000.0,
            ),
        });
    }

    // AUDIT-FIX F-1: Use StateBatch for atomic all-or-nothing commit of
    // deployer debit + treasury credit + contract account creation.
    let mut batch = state.state.begin_batch();

    // Debit deployer using deduct_spendable to maintain spores == spendable + staked + locked
    let mut updated_deployer = deployer_account.clone();
    updated_deployer
        .deduct_spendable(deploy_fee)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to deduct deploy fee: {}", e),
        })?;
    batch
        .put_account(&deployer_pubkey, &updated_deployer)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to update deployer balance: {}", e),
        })?;

    // Credit deploy fee to treasury (not a vanishing deduction)
    let treasury_pubkey = state
        .state
        .get_treasury_pubkey()
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32000,
            message: "Treasury pubkey not set".to_string(),
        })?;
    let mut treasury_account = state
        .state
        .get_account(&treasury_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .unwrap_or_else(|| lichen_core::Account::new(0, treasury_pubkey));
    treasury_account
        .add_spendable(deploy_fee)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Treasury balance overflow: {}", e),
        })?;
    batch
        .put_account(&treasury_pubkey, &treasury_account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to credit treasury: {}", e),
        })?;

    // Create ContractAccount
    let contract = ContractAccount::new(code_bytes, deployer_pubkey);
    let mut account = lichen_core::Account::new(0, program_pubkey);
    account.data = serde_json::to_vec(&contract).map_err(|e| RpcError {
        code: -32000,
        message: format!("Failed to serialize contract: {}", e),
    })?;
    account.executable = true;

    // Store the contract account
    batch
        .put_account(&program_pubkey, &account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to store contract: {}", e),
        })?;

    // Commit all three writes atomically
    state.state.commit_batch(batch).map_err(|e| RpcError {
        code: -32000,
        message: format!("Atomic deploy commit failed: {}", e),
    })?;

    // Index in programs list
    if let Err(e) = state.state.index_program(&program_pubkey) {
        warn!("deployContract: index_program failed: {}", e);
    }

    // Process init_data for symbol registry
    if !init_data_bytes.is_empty() {
        if let Ok(raw) = std::str::from_utf8(&init_data_bytes) {
            if let Ok(registry_data) = serde_json::from_str::<serde_json::Value>(raw) {
                if let Some(symbol) = registry_data.get("symbol").and_then(|s| s.as_str()) {
                    let entry = SymbolRegistryEntry {
                        symbol: symbol.to_string(),
                        program: program_pubkey,
                        owner: deployer_pubkey,
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
                    if let Err(e) = state.state.register_symbol(symbol, entry) {
                        warn!("deployContract: register_symbol failed: {}", e);
                    }
                }
            }
        }
    }

    log_privileged_rpc_mutation(
        "deployContract",
        "legacy_admin",
        &deployer_pubkey.to_base58(),
        "contract",
        Some(&program_pubkey.to_base58()),
        serde_json::json!({
            "code_size": account.data.len(),
            "deploy_fee": deploy_fee,
            "contract_name": contract_name,
        }),
    );

    Ok(serde_json::json!({
        "program_id": program_pubkey.to_base58(),
        "deployer": deployer_pubkey.to_base58(),
        "code_size": account.data.len(),
        "deploy_fee": deploy_fee,
        "deploy_fee_licn": deploy_fee as f64 / 1_000_000_000.0,
    }))
}

/// Upgrade an existing smart contract (owner-only, charges upgrade fee).
/// Params: [owner_base58, contract_base58, code_base64, signature_hex]
async fn handle_upgrade_contract(
    state: &RpcState,
    params: Option<serde_json::Value>,
    auth_header: Option<&str>,
) -> Result<serde_json::Value, RpcError> {
    use base64::{engine::general_purpose, Engine as _};
    use lichen_core::account::Keypair as LichenKeypair;
    use sha2::{Digest, Sha256};

    require_legacy_admin_rpc_enabled(state, "upgradeContract")?;
    require_single_validator(state, "upgradeContract").await?;
    verify_admin_auth(state, auth_header)?;

    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Params must be an array: [owner, contract, code_base64, signature]".to_string(),
    })?;

    if arr.len() < 4 {
        return Err(RpcError {
            code: -32602,
            message: "Expected [owner_base58, contract_base58, code_base64, signature_hex]"
                .to_string(),
        });
    }

    // Parse owner pubkey
    let owner_str = arr[0].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "owner must be a base58 string".to_string(),
    })?;
    let owner_pubkey = Pubkey::from_base58(owner_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid owner pubkey: {}", e),
    })?;

    // Parse contract address
    let contract_str = arr[1].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "contract must be a base58 string".to_string(),
    })?;
    let contract_pubkey = Pubkey::from_base58(contract_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid contract pubkey: {}", e),
    })?;

    // Parse new WASM code (base64)
    let code_b64 = arr[2].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "code must be a base64 string".to_string(),
    })?;
    let code_bytes = general_purpose::STANDARD
        .decode(code_b64)
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid base64 code: {}", e),
        })?;

    if code_bytes.is_empty() {
        return Err(RpcError {
            code: -32602,
            message: "Code cannot be empty".to_string(),
        });
    }

    // P10-RPC-05: Reject oversized WASM code to prevent storage abuse
    if code_bytes.len() > MAX_CONTRACT_CODE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Contract code too large: {} bytes (max {} bytes / 512 KB)",
                code_bytes.len(),
                MAX_CONTRACT_CODE,
            ),
        });
    }

    let signature = parse_pq_signature_value(&arr[3])?;

    // Verify signature: owner must sign SHA-256(code_bytes)
    let mut hasher = Sha256::new();
    hasher.update(&code_bytes);
    let code_hash_bytes = hasher.finalize();
    if !LichenKeypair::verify(&owner_pubkey, &code_hash_bytes, &signature) {
        return Err(RpcError {
            code: -32003,
            message: "Invalid signature: owner must sign SHA-256(code)".to_string(),
        });
    }

    // Load existing contract
    let contract_account = state
        .state
        .get_account(&contract_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32000,
            message: format!("Contract not found at {}", contract_pubkey.to_base58()),
        })?;

    let mut contract: ContractAccount =
        serde_json::from_slice(&contract_account.data).map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to deserialize contract: {}", e),
        })?;

    // Verify caller is the contract owner
    if contract.owner != owner_pubkey {
        return Err(RpcError {
            code: -32003,
            message: "Only the contract owner can upgrade".to_string(),
        });
    }

    // Charge upgrade fee (10 LICN)
    let upgrade_fee = lichen_core::CONTRACT_UPGRADE_FEE;
    let owner_account = state
        .state
        .get_account(&owner_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32000,
            message: "Owner account not found".to_string(),
        })?;

    if owner_account.spendable < upgrade_fee {
        return Err(RpcError {
            code: -32000,
            message: format!(
                "Insufficient spendable balance: need {} spores ({:.1} LICN), have {} spendable ({:.1} LICN)",
                upgrade_fee,
                upgrade_fee as f64 / 1_000_000_000.0,
                owner_account.spendable,
                owner_account.spendable as f64 / 1_000_000_000.0,
            ),
        });
    }

    // AUDIT-FIX F-2: Use StateBatch for atomic all-or-nothing commit of
    // owner debit + treasury credit + contract upgrade.
    let mut batch = state.state.begin_batch();

    let mut updated_owner = owner_account.clone();
    updated_owner
        .deduct_spendable(upgrade_fee)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to deduct upgrade fee: {}", e),
        })?;
    batch
        .put_account(&owner_pubkey, &updated_owner)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to update owner balance: {}", e),
        })?;

    // Credit fee to treasury
    let treasury_pubkey = state
        .state
        .get_treasury_pubkey()
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32000,
            message: "Treasury pubkey not set".to_string(),
        })?;
    let mut treasury_account = state
        .state
        .get_account(&treasury_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .unwrap_or_else(|| lichen_core::Account::new(0, treasury_pubkey));
    treasury_account
        .add_spendable(upgrade_fee)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Treasury balance overflow: {}", e),
        })?;
    batch
        .put_account(&treasury_pubkey, &treasury_account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to credit treasury: {}", e),
        })?;

    // Perform the upgrade: bump version, store previous hash, replace code
    let old_version = contract.version;
    contract.previous_code_hash = Some(contract.code_hash);
    contract.version = contract.version.saturating_add(1);

    // Compute new code hash
    let mut code_hasher = Sha256::new();
    code_hasher.update(&code_bytes);
    let new_hash = code_hasher.finalize();
    let mut hash_bytes = [0u8; 32];
    hash_bytes.copy_from_slice(&new_hash[..32]);
    contract.code_hash = lichen_core::Hash(hash_bytes);
    contract.code = code_bytes;

    // Serialize back
    let mut updated_account = contract_account.clone();
    updated_account.data = serde_json::to_vec(&contract).map_err(|e| RpcError {
        code: -32000,
        message: format!("Failed to serialize upgraded contract: {}", e),
    })?;

    batch
        .put_account(&contract_pubkey, &updated_account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to store upgraded contract: {}", e),
        })?;

    // Commit all three writes atomically
    state.state.commit_batch(batch).map_err(|e| RpcError {
        code: -32000,
        message: format!("Atomic upgrade commit failed: {}", e),
    })?;

    log_privileged_rpc_mutation(
        "upgradeContract",
        "legacy_admin",
        &owner_pubkey.to_base58(),
        "contract",
        Some(&contract_pubkey.to_base58()),
        serde_json::json!({
            "previous_version": old_version,
            "version": contract.version,
            "code_size": updated_account.data.len(),
            "upgrade_fee": upgrade_fee,
        }),
    );

    Ok(serde_json::json!({
        "program_id": contract_pubkey.to_base58(),
        "owner": owner_pubkey.to_base58(),
        "version": contract.version,
        "previous_version": old_version,
        "code_size": updated_account.data.len(),
        "upgrade_fee": upgrade_fee,
        "upgrade_fee_licn": upgrade_fee as f64 / 1_000_000_000.0,
    }))
}

// ============================================================================
// PROGRAM ENDPOINTS
// ============================================================================

async fn handle_get_program(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let program_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [program_pubkey]".to_string(),
        })?;

    let program_pubkey = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program pubkey: {}", e),
    })?;

    let account = state
        .state
        .get_account(&program_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "Program not found".to_string(),
    })?;

    if !account.executable {
        return Err(RpcError {
            code: -32002,
            message: "Account is not executable".to_string(),
        });
    }

    let contract: ContractAccount =
        serde_json::from_slice(&account.data).map_err(|e| RpcError {
            code: -32002,
            message: format!("Invalid program data: {}", e),
        })?;

    let storage_stats = state
        .state
        .get_contract_storage_stats(&program_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!({
        "program": program_pubkey.to_base58(),
        "owner": contract.owner.to_base58(),
        "code_hash": contract.code_hash.to_hex(),
        "code_size": contract.code.len(),
        "storage_entries": storage_stats.entry_count,
        "storage_size": storage_stats.total_value_size,
        "executable": account.executable,
    }))
}

async fn handle_get_program_stats(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let program_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [program_pubkey]".to_string(),
        })?;

    let program_pubkey = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program pubkey: {}", e),
    })?;

    let account = state
        .state
        .get_account(&program_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "Program not found".to_string(),
    })?;

    let contract: ContractAccount =
        serde_json::from_slice(&account.data).map_err(|e| RpcError {
            code: -32002,
            message: format!("Invalid program data: {}", e),
        })?;

    let storage_stats = state
        .state
        .get_contract_storage_stats(&program_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let call_count = state
        .state
        .count_program_calls(&program_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!({
        "program": program_pubkey.to_base58(),
        "owner": contract.owner.to_base58(),
        "code_hash": contract.code_hash.to_hex(),
        "code_size": contract.code.len(),
        "storage_entries": storage_stats.entry_count,
        "call_count": call_count,
    }))
}

async fn handle_get_programs(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    if let Some(cached) = get_cached_program_list_response(state, "getPrograms", &params).await {
        return Ok(cached);
    }

    let (limit, after) = parse_program_list_pagination(params.clone(), 50, 500)?;
    let fetch_limit = limit.saturating_add(1);

    let mut programs = state
        .state
        .get_programs_paginated(fetch_limit, after.as_ref())
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;
    let has_more = programs.len() > limit;
    if has_more {
        programs.truncate(limit);
    }

    let list: Vec<String> = programs.iter().map(|p| p.to_base58()).collect();
    let next_cursor = if has_more {
        programs.last().map(|p| p.to_base58())
    } else {
        None
    };

    let response = serde_json::json!({
        "count": list.len(),
        "programs": list,
        "has_more": has_more,
        "next_cursor": next_cursor,
    });

    put_cached_program_list_response(state, "getPrograms", &params, response.clone()).await;

    Ok(response)
}

async fn handle_get_program_calls(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [program_pubkey, options?]".to_string(),
    })?;

    let program_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [program_pubkey, options?]".to_string(),
        })?;

    let limit = arr
        .get(1)
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;

    let before_slot = arr
        .get(1)
        .and_then(|v| v.get("before_slot"))
        .and_then(|v| v.as_u64());

    let program_pubkey = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program pubkey: {}", e),
    })?;

    let calls = state
        .state
        .get_program_calls(&program_pubkey, limit, before_slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let items: Vec<serde_json::Value> = calls
        .into_iter()
        .map(|call| {
            serde_json::json!({
                "slot": call.slot,
                "timestamp": call.timestamp,
                "program": call.program.to_base58(),
                "caller": call.caller.to_base58(),
                "function": call.function,
                "value": call.value,
                "tx_signature": call.tx_signature.to_hex(),
            })
        })
        .collect();

    Ok(serde_json::json!({
        "program": program_pubkey.to_base58(),
        "count": items.len(),
        "calls": items,
    }))
}

/// AUDIT-FIX 3.25: This endpoint intentionally exposes all contract storage
/// key-value pairs. On-chain state is public by design (similar to eth_getStorageAt).
/// Rate limiting via the global RateLimiter applies to this endpoint.
///
/// Uses CF_CONTRACT_STORAGE prefix iterator for O(limit) instead of loading
/// the entire ContractAccount and deserializing the full in-memory BTreeMap.
async fn handle_get_program_storage(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [program_pubkey, options?]".to_string(),
    })?;

    let program_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [program_pubkey, options?]".to_string(),
        })?;

    let limit = arr
        .get(1)
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;

    let after_key_hex = arr
        .get(1)
        .and_then(|v| v.get("after_key"))
        .and_then(|v| v.as_str());

    let program_pubkey = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program pubkey: {}", e),
    })?;

    // Decode hex cursor if provided
    let after_key = if let Some(hex_str) = after_key_hex {
        Some(hex::decode(hex_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid after_key hex: {}", e),
        })?)
    } else {
        None
    };

    // Use CF_CONTRACT_STORAGE prefix iterator — O(limit) instead of O(N) deserialization
    let raw_entries = state
        .state
        .get_contract_storage_entries(&program_pubkey, limit, after_key)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mut entries = Vec::new();
    for (key, value) in raw_entries {
        let key_hex = hex::encode(&key);
        // Try to decode key as UTF-8 for human-readable display
        let key_decoded = String::from_utf8(key.clone()).ok().filter(|s| {
            s.chars().all(|c| {
                c.is_ascii_graphic() || c == ' ' || c == ':' || c == '_' || c == '-' || c == '.'
            })
        });
        let value_hex = hex::encode(&value);
        let size = value.len();
        // Try to decode value as UTF-8 for preview; fallback to hex truncated
        let value_preview = String::from_utf8(value.clone())
            .ok()
            .filter(|s| !s.is_empty() && s.chars().take(16).all(|c| !c.is_control() || c == '\n'))
            .map(|s| {
                if s.len() > 128 {
                    format!("{}...", &s[..128])
                } else {
                    s
                }
            })
            .unwrap_or_else(|| {
                if value_hex.len() > 80 {
                    format!("0x{}...", &value_hex[..80])
                } else if value_hex.is_empty() {
                    "(empty)".to_string()
                } else {
                    format!("0x{}", value_hex)
                }
            });

        let mut entry = serde_json::json!({
            "key": key_hex.clone(),
            "key_hex": key_hex,
            "value": value_hex.clone(),
            "value_hex": value_hex,
            "value_preview": value_preview,
            "size": size,
        });
        if let Some(decoded) = key_decoded {
            entry["key_decoded"] = serde_json::Value::String(decoded);
        }
        entries.push(entry);
    }

    Ok(serde_json::json!({
        "program": program_pubkey.to_base58(),
        "count": entries.len(),
        "entries": entries,
    }))
}

// ============================================================================
// LICHENID ENDPOINTS
// ============================================================================

const LICHENID_SYMBOL: &str = "YID";
const LICHENID_IDENTITY_SIZE: usize = 127;

#[derive(Debug, Clone)]
struct LichenIdIdentityRecord {
    owner: Pubkey,
    agent_type: u8,
    name: String,
    reputation: u64,
    created_at: u64,
    updated_at: u64,
    skill_count: u8,
    vouch_count: u16,
    is_active: bool,
}

#[derive(Debug, Clone)]
struct LichenIdSkillRecord {
    name: String,
    proficiency: u8,
    timestamp: u64,
}

#[derive(Debug, Clone)]
struct LichenIdVouchRecord {
    voucher: Pubkey,
    timestamp: u64,
}

#[derive(Debug, Clone)]
struct LichenIdAchievementRecord {
    id: u8,
    timestamp: u64,
}

fn lichenid_agent_type_name(agent_type: u8) -> &'static str {
    match agent_type {
        0 => "System",
        1 => "Trading",
        2 => "Development",
        3 => "Analysis",
        4 => "Creative",
        5 => "Infrastructure",
        6 => "Governance",
        7 => "Oracle",
        8 => "Storage",
        9 => "General",
        _ => "Unknown",
    }
}

fn lichenid_trust_tier(score: u64) -> u8 {
    if score >= 10_000 {
        5
    } else if score >= 5_000 {
        4
    } else if score >= 1_000 {
        3
    } else if score >= 500 {
        2
    } else if score >= 100 {
        1
    } else {
        0
    }
}

fn lichenid_trust_tier_name(tier: u8) -> &'static str {
    match tier {
        1 => "Verified",
        2 => "Trusted",
        3 => "Established",
        4 => "Elite",
        5 => "Legendary",
        _ => "Newcomer",
    }
}

fn lichenid_achievement_name(achievement_id: u8) -> &'static str {
    match achievement_id {
        // Identity (1-12)
        1 => "First Transaction",
        2 => "Governance Voter",
        3 => "Program Builder",
        4 => "Trusted Agent",
        5 => "Veteran Agent",
        6 => "Legendary Agent",
        7 => "Well Endorsed",
        8 => "Bootstrap Graduation",
        9 => "Name Registrar",
        10 => "Skill Master",
        11 => "Social Butterfly",
        12 => "First Name",
        // DEX (13-21)
        13 => "First Trade",
        14 => "LP Provider",
        15 => "LP Withdrawal",
        16 => "DEX User",
        17 => "Multi-hop Trader",
        18 => "Margin Trader",
        19 => "Position Closer",
        20 => "Yield Farmer",
        21 => "Analytics Explorer",
        // Lending (31-38)
        31 => "First Lend",
        32 => "First Borrow",
        33 => "Loan Repaid",
        34 => "Liquidator",
        35 => "Withdrawal Expert",
        36 => "Stablecoin Minter",
        37 => "Stablecoin Redeemer",
        38 => "Stable Sender",
        // Staking (41-48)
        41 => "First Stake",
        42 => "Unstaked",
        43 => "MossStake Pioneer",
        44 => "Locked Staker",
        45 => "Diamond Hands",
        46 => "Whale Staker",
        47 => "Reward Harvester",
        48 => "stLICN Transferrer",
        // Bridge (51-56)
        51 => "Bridge Pioneer",
        52 => "Bridge Out",
        53 => "Bridge User",
        54 => "Wrapper",
        55 => "Unwrapper",
        56 => "Cross-chain Trader",
        // Shield/Privacy (57-60)
        57 => "Privacy Pioneer",
        58 => "Unshielded",
        59 => "Shadow Sender",
        60 => "ZK Privacy User",
        // NFT (63-70)
        63 => "Collection Creator",
        64 => "First Mint",
        65 => "NFT Trader",
        66 => "First Listing",
        67 => "First Purchase",
        68 => "Bidder",
        69 => "Deal Maker",
        70 => "Punk Collector",
        // Governance (71-73)
        71 => "Proposal Creator",
        72 => "First Vote",
        73 => "Delegator",
        // Oracle (81-82)
        81 => "Oracle Reporter",
        82 => "Oracle User",
        // Storage (86-88)
        86 => "File Uploader",
        87 => "Data Retriever",
        88 => "Storage User",
        // Marketplace/Auction (91-93)
        91 => "Auctioneer",
        92 => "Auction Bidder",
        93 => "Auction Winner",
        // Bounty (96-98)
        96 => "Bounty Poster",
        97 => "Bounty Hunter",
        98 => "Bounty Judge",
        // Prediction (101-104)
        101 => "Market Maker",
        102 => "First Prediction",
        103 => "Oracle Resolver",
        104 => "Prediction Winner",
        // General milestones (106-124)
        106 => "Big Spender",
        107 => "Whale Transfer",
        108 => "EVM Connected",
        109 => "Identity Created",
        110 => "Profile Customizer",
        111 => "Voucher",
        112 => "Agent Creator",
        113 => "Compute Provider",
        114 => "Compute Consumer",
        115 => "Payment Creator",
        116 => "First Payment",
        117 => "Subscription Creator",
        118 => "Token Launcher",
        119 => "Early Buyer",
        120 => "Token Seller",
        121 => "Vault Depositor",
        122 => "Vault Withdrawer",
        123 => "Token Contract User",
        124 => "Contract Interactor",
        _ => "Unknown Achievement",
    }
}

fn read_u64_le(input: &[u8], offset: usize) -> Option<u64> {
    if input.len() < offset + 8 {
        return None;
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&input[offset..offset + 8]);
    Some(u64::from_le_bytes(bytes))
}

fn extract_single_pubkey(
    params: &Option<serde_json::Value>,
    method: &str,
) -> Result<Pubkey, RpcError> {
    let params = params.as_ref().ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: format!("Invalid params for {}: expected [pubkey]", method),
        })?;
    Pubkey::from_base58(pubkey_str).map_err(|_| invalid_pubkey_format_error())
}

fn extract_single_string(
    params: &Option<serde_json::Value>,
    method: &str,
    label: &str,
) -> Result<String, RpcError> {
    let params = params.as_ref().ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: format!("Invalid params for {}: expected [{}]", method, label),
        })
}

fn lichenid_hex(pubkey: &Pubkey) -> String {
    hex::encode(pubkey.0)
}

fn lichenid_identity_key(pubkey: &Pubkey) -> Vec<u8> {
    format!("id:{}", lichenid_hex(pubkey)).into_bytes()
}

fn lichenid_reputation_key(pubkey: &Pubkey) -> Vec<u8> {
    format!("rep:{}", lichenid_hex(pubkey)).into_bytes()
}

fn lichenid_reverse_name_key(pubkey: &Pubkey) -> Vec<u8> {
    format!("name_rev:{}", lichenid_hex(pubkey)).into_bytes()
}

fn lichenid_skill_key(pubkey: &Pubkey, index: u8) -> Vec<u8> {
    format!("skill:{}:{}", lichenid_hex(pubkey), index).into_bytes()
}

fn lichenid_vouch_key(pubkey: &Pubkey, index: u16) -> Vec<u8> {
    format!("vouch:{}:{}", lichenid_hex(pubkey), index).into_bytes()
}

fn lichenid_vouch_given_key(pubkey: &Pubkey, index: u16) -> Vec<u8> {
    format!("vouch_given:{}:{}", lichenid_hex(pubkey), index).into_bytes()
}

fn lichenid_achievement_key(pubkey: &Pubkey, achievement_id: u8) -> Vec<u8> {
    format!("ach:{}:{:02}", lichenid_hex(pubkey), achievement_id).into_bytes()
}

fn lichenid_skill_hash(skill_name: &str) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (index, byte) in skill_name.as_bytes().iter().enumerate() {
        if index >= 8 {
            break;
        }
        out[index] = *byte;
    }
    out
}

fn lichenid_attestation_count_key(pubkey: &Pubkey, skill_name: &str) -> Vec<u8> {
    let skill_hash = lichenid_skill_hash(skill_name);
    format!(
        "attest_count_{}_{}",
        lichenid_hex(pubkey),
        hex::encode(skill_hash)
    )
    .into_bytes()
}

fn parse_lichenid_identity_record(input: &[u8]) -> Option<LichenIdIdentityRecord> {
    if input.len() < LICHENID_IDENTITY_SIZE {
        return None;
    }

    let mut owner_bytes = [0u8; 32];
    owner_bytes.copy_from_slice(&input[0..32]);
    let owner = Pubkey(owner_bytes);
    let agent_type = input[32];

    let name_len = (input[33] as usize) | ((input[34] as usize) << 8);
    if name_len > 64 || 35 + name_len > input.len() {
        return None;
    }
    let name = String::from_utf8_lossy(&input[35..35 + name_len]).to_string();

    Some(LichenIdIdentityRecord {
        owner,
        agent_type,
        name,
        reputation: read_u64_le(input, 99)?,
        created_at: read_u64_le(input, 107)?,
        updated_at: read_u64_le(input, 115)?,
        skill_count: input[123],
        vouch_count: (input[124] as u16) | ((input[125] as u16) << 8),
        is_active: input[126] == 1,
    })
}

fn parse_lichenid_skill_record(input: &[u8]) -> Option<LichenIdSkillRecord> {
    let name_len = *input.first()? as usize;
    if name_len == 0 || 1 + name_len + 1 + 8 > input.len() {
        return None;
    }
    let name = String::from_utf8_lossy(&input[1..1 + name_len]).to_string();
    let proficiency = input[1 + name_len];
    let timestamp = read_u64_le(input, 1 + name_len + 1)?;
    Some(LichenIdSkillRecord {
        name,
        proficiency,
        timestamp,
    })
}

fn parse_lichenid_vouch_record(input: &[u8]) -> Option<LichenIdVouchRecord> {
    if input.len() < 40 {
        return None;
    }
    let mut voucher = [0u8; 32];
    voucher.copy_from_slice(&input[0..32]);
    Some(LichenIdVouchRecord {
        voucher: Pubkey(voucher),
        timestamp: read_u64_le(input, 32)?,
    })
}

fn parse_lichenid_vouch_given_record(input: &[u8]) -> Option<(Pubkey, u64)> {
    if input.len() < 40 {
        return None;
    }
    let mut vouchee = [0u8; 32];
    vouchee.copy_from_slice(&input[0..32]);
    let timestamp = read_u64_le(input, 32)?;
    Some((Pubkey(vouchee), timestamp))
}

fn parse_lichenid_achievement_record(input: &[u8]) -> Option<LichenIdAchievementRecord> {
    if input.len() < 9 {
        return None;
    }
    Some(LichenIdAchievementRecord {
        id: input[0],
        timestamp: read_u64_le(input, 1)?,
    })
}

/// CF-based LichenID identity read — no full account deserialization.
fn get_lichenid_identity(state: &RpcState, pubkey: &Pubkey) -> Option<LichenIdIdentityRecord> {
    state
        .state
        .get_program_storage(LICHENID_SYMBOL, &lichenid_identity_key(pubkey))
        .and_then(|value| parse_lichenid_identity_record(&value))
}

fn get_lichenid_reputation(state: &RpcState, pubkey: &Pubkey) -> Option<u64> {
    state
        .state
        .get_program_storage(LICHENID_SYMBOL, &lichenid_reputation_key(pubkey))
        .and_then(|value| read_u64_le(&value, 0))
}

fn get_lichenid_name(state: &RpcState, pubkey: &Pubkey, current_slot: u64) -> Option<String> {
    let raw_name = state
        .state
        .get_program_storage(LICHENID_SYMBOL, &lichenid_reverse_name_key(pubkey))?;
    let label = String::from_utf8(raw_name).ok()?;
    let record = state
        .state
        .get_program_storage(LICHENID_SYMBOL, &format!("name:{}", label).into_bytes())?;
    if record.len() < 48 {
        return None;
    }
    let expiry_slot = read_u64_le(&record, 40)?;
    if current_slot >= expiry_slot {
        return None;
    }
    Some(format!("{}.lichen", label))
}

fn lichenid_cf_get(state: &RpcState, key: &[u8]) -> Option<Vec<u8>> {
    state.state.get_program_storage(LICHENID_SYMBOL, key)
}

async fn handle_get_lichenid_identity(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "getLichenIdIdentity")?;
    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let identity = match get_lichenid_identity(state, &pubkey) {
        Some(identity) => identity,
        None => return Ok(serde_json::Value::Null),
    };

    let score = get_lichenid_reputation(state, &pubkey).unwrap_or(identity.reputation);
    let tier = lichenid_trust_tier(score);
    let licn_name = get_lichenid_name(state, &pubkey, current_slot);

    Ok(serde_json::json!({
        "address": pubkey.to_base58(),
        "owner": identity.owner.to_base58(),
        "name": identity.name,
        "licn_name": licn_name,
        "agent_type": identity.agent_type,
        "agent_type_name": lichenid_agent_type_name(identity.agent_type),
        "reputation": score,
        "trust_tier": tier,
        "trust_tier_name": lichenid_trust_tier_name(tier),
        "created_at": identity.created_at,
        "updated_at": identity.updated_at,
        "skill_count": identity.skill_count,
        "vouch_count": identity.vouch_count,
        "is_active": identity.is_active,
    }))
}

async fn handle_get_lichenid_reputation(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "getLichenIdReputation")?;

    let score = get_lichenid_reputation(state, &pubkey)
        .or_else(|| get_lichenid_identity(state, &pubkey).map(|identity| identity.reputation))
        .unwrap_or(0);
    let tier = lichenid_trust_tier(score);

    Ok(serde_json::json!({
        "address": pubkey.to_base58(),
        "score": score,
        "tier": tier,
        "tier_name": lichenid_trust_tier_name(tier),
    }))
}

async fn handle_get_lichenid_skills(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "getLichenIdSkills")?;

    let identity = match get_lichenid_identity(state, &pubkey) {
        Some(identity) => identity,
        None => return Ok(serde_json::json!([])),
    };

    let mut skills = Vec::new();
    for index in 0..identity.skill_count {
        if let Some(raw) = lichenid_cf_get(state, &lichenid_skill_key(&pubkey, index)) {
            if let Some(skill) = parse_lichenid_skill_record(&raw) {
                let attestations =
                    lichenid_cf_get(state, &lichenid_attestation_count_key(&pubkey, &skill.name))
                        .and_then(|value| read_u64_le(&value, 0))
                        .unwrap_or(0);

                skills.push(serde_json::json!({
                    "index": index,
                    "name": skill.name,
                    "proficiency": skill.proficiency,
                    "attestation_count": attestations,
                    "timestamp": skill.timestamp,
                }));
            }
        }
    }

    Ok(serde_json::Value::Array(skills))
}

async fn handle_get_lichenid_vouches(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "getLichenIdVouches")?;
    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let identity = match get_lichenid_identity(state, &pubkey) {
        Some(identity) => identity,
        None => return Ok(serde_json::json!({"received": [], "given": []})),
    };

    let mut received = Vec::new();
    for index in 0..identity.vouch_count {
        if let Some(raw) = lichenid_cf_get(state, &lichenid_vouch_key(&pubkey, index)) {
            if let Some(vouch) = parse_lichenid_vouch_record(&raw) {
                received.push(serde_json::json!({
                    "voucher": vouch.voucher.to_base58(),
                    "voucher_name": get_lichenid_name(state, &vouch.voucher, current_slot),
                    "timestamp": vouch.timestamp,
                }));
            }
        }
    }

    let mut given = Vec::new();
    for index in 0..identity.vouch_count {
        if let Some(raw) = lichenid_cf_get(state, &lichenid_vouch_given_key(&pubkey, index)) {
            if let Some((vouchee, timestamp)) = parse_lichenid_vouch_given_record(&raw) {
                given.push(serde_json::json!({
                    "vouchee": vouchee.to_base58(),
                    "vouchee_name": get_lichenid_name(state, &vouchee, current_slot),
                    "timestamp": timestamp,
                }));
            }
        }
    }

    // Backward compatibility for pre-indexed historical data — scan CF entries.
    if given.is_empty() {
        let program = resolve_symbol_pubkey(state, LICHENID_SYMBOL)?;
        let entries = state
            .state
            .get_contract_storage_entries(&program, 10_000, None)
            .unwrap_or_default();
        for (key, value) in &entries {
            if !key.starts_with(b"id:") {
                continue;
            }
            let Some(vouchee_identity) = parse_lichenid_identity_record(value) else {
                continue;
            };
            for index in 0..vouchee_identity.vouch_count {
                if let Some(raw_vouch) =
                    lichenid_cf_get(state, &lichenid_vouch_key(&vouchee_identity.owner, index))
                {
                    if let Some(vouch) = parse_lichenid_vouch_record(&raw_vouch) {
                        if vouch.voucher == pubkey {
                            given.push(serde_json::json!({
                                "vouchee": vouchee_identity.owner.to_base58(),
                                "vouchee_name": get_lichenid_name(state, &vouchee_identity.owner, current_slot),
                                "timestamp": vouch.timestamp,
                            }));
                        }
                    }
                }
            }
        }
    }

    Ok(serde_json::json!({
        "received": received,
        "given": given,
    }))
}

async fn handle_get_lichenid_achievements(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "getLichenIdAchievements")?;

    if get_lichenid_identity(state, &pubkey).is_none() {
        return Ok(serde_json::json!([]));
    }

    let mut achievements = Vec::new();
    for achievement_id in 1u8..=128u8 {
        if let Some(raw) =
            lichenid_cf_get(state, &lichenid_achievement_key(&pubkey, achievement_id))
        {
            if let Some(achievement) = parse_lichenid_achievement_record(&raw) {
                achievements.push(serde_json::json!({
                    "id": achievement.id,
                    "name": lichenid_achievement_name(achievement.id),
                    "timestamp": achievement.timestamp,
                }));
            }
        }
    }

    Ok(serde_json::Value::Array(achievements))
}

async fn handle_get_lichenid_profile(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "getLichenIdProfile")?;
    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let identity = match get_lichenid_identity(state, &pubkey) {
        Some(identity) => identity,
        None => return Ok(serde_json::Value::Null),
    };

    let reputation = get_lichenid_reputation(state, &pubkey).unwrap_or(identity.reputation);
    let tier = lichenid_trust_tier(reputation);
    let licn_name = get_lichenid_name(state, &pubkey, current_slot);

    let mut skills = Vec::new();
    for index in 0..identity.skill_count {
        if let Some(raw) = lichenid_cf_get(state, &lichenid_skill_key(&pubkey, index)) {
            if let Some(skill) = parse_lichenid_skill_record(&raw) {
                let attestations =
                    lichenid_cf_get(state, &lichenid_attestation_count_key(&pubkey, &skill.name))
                        .and_then(|value| read_u64_le(&value, 0))
                        .unwrap_or(0);

                skills.push(serde_json::json!({
                    "index": index,
                    "name": skill.name,
                    "proficiency": skill.proficiency,
                    "attestation_count": attestations,
                    "timestamp": skill.timestamp,
                }));
            }
        }
    }

    let mut received_vouches = Vec::new();
    for index in 0..identity.vouch_count {
        if let Some(raw) = lichenid_cf_get(state, &lichenid_vouch_key(&pubkey, index)) {
            if let Some(vouch) = parse_lichenid_vouch_record(&raw) {
                received_vouches.push(serde_json::json!({
                    "voucher": vouch.voucher.to_base58(),
                    "voucher_name": get_lichenid_name(state, &vouch.voucher, current_slot),
                    "timestamp": vouch.timestamp,
                }));
            }
        }
    }

    let mut given_vouches = Vec::new();
    for index in 0..identity.vouch_count {
        if let Some(raw) = lichenid_cf_get(state, &lichenid_vouch_given_key(&pubkey, index)) {
            if let Some((vouchee, timestamp)) = parse_lichenid_vouch_given_record(&raw) {
                given_vouches.push(serde_json::json!({
                    "vouchee": vouchee.to_base58(),
                    "vouchee_name": get_lichenid_name(state, &vouchee, current_slot),
                    "timestamp": timestamp,
                }));
            }
        }
    }

    if given_vouches.is_empty() {
        let program = resolve_symbol_pubkey(state, LICHENID_SYMBOL)?;
        let entries = state
            .state
            .get_contract_storage_entries(&program, 10_000, None)
            .unwrap_or_default();
        for (key, value) in &entries {
            if !key.starts_with(b"id:") {
                continue;
            }
            let Some(vouchee_identity) = parse_lichenid_identity_record(value) else {
                continue;
            };
            for index in 0..vouchee_identity.vouch_count {
                if let Some(raw_vouch) =
                    lichenid_cf_get(state, &lichenid_vouch_key(&vouchee_identity.owner, index))
                {
                    if let Some(vouch) = parse_lichenid_vouch_record(&raw_vouch) {
                        if vouch.voucher == pubkey {
                            given_vouches.push(serde_json::json!({
                                "vouchee": vouchee_identity.owner.to_base58(),
                                "vouchee_name": get_lichenid_name(state, &vouchee_identity.owner, current_slot),
                                "timestamp": vouch.timestamp,
                            }));
                        }
                    }
                }
            }
        }
    }

    let mut achievements = Vec::new();
    for achievement_id in 1u8..=128u8 {
        if let Some(raw) =
            lichenid_cf_get(state, &lichenid_achievement_key(&pubkey, achievement_id))
        {
            if let Some(achievement) = parse_lichenid_achievement_record(&raw) {
                achievements.push(serde_json::json!({
                    "id": achievement.id,
                    "name": lichenid_achievement_name(achievement.id),
                    "timestamp": achievement.timestamp,
                }));
            }
        }
    }

    let endpoint = lichenid_cf_get(
        state,
        &format!("endpoint:{}", lichenid_hex(&pubkey)).into_bytes(),
    )
    .and_then(|raw| String::from_utf8(raw).ok());

    let metadata = lichenid_cf_get(
        state,
        &format!("metadata:{}", lichenid_hex(&pubkey)).into_bytes(),
    )
    .and_then(|raw| String::from_utf8(raw).ok())
    .map(|text| {
        serde_json::from_str::<serde_json::Value>(&text).unwrap_or(serde_json::json!(text))
    });

    let availability = lichenid_cf_get(
        state,
        &format!("availability:{}", lichenid_hex(&pubkey)).into_bytes(),
    )
    .and_then(|raw| raw.first().copied())
    .unwrap_or(0);

    let availability_name = match availability {
        1 => "available",
        2 => "busy",
        _ => "offline",
    };

    let rate = lichenid_cf_get(
        state,
        &format!("rate:{}", lichenid_hex(&pubkey)).into_bytes(),
    )
    .and_then(|raw| read_u64_le(&raw, 0))
    .unwrap_or(0);

    let mut contributions = serde_json::Map::new();
    let labels = [
        "successful_txs",
        "governance_votes",
        "programs_deployed",
        "uptime_hours",
        "peer_endorsements",
        "failed_txs",
        "slashing_events",
    ];
    for (index, label) in labels.iter().enumerate() {
        let key = format!("cont:{}:{}", lichenid_hex(&pubkey), index).into_bytes();
        let value = lichenid_cf_get(state, &key)
            .and_then(|raw| read_u64_le(&raw, 0))
            .unwrap_or(0);
        contributions.insert((*label).to_string(), serde_json::json!(value));
    }

    Ok(serde_json::json!({
        "identity": {
            "address": pubkey.to_base58(),
            "owner": identity.owner.to_base58(),
            "name": identity.name,
            "agent_type": identity.agent_type,
            "agent_type_name": lichenid_agent_type_name(identity.agent_type),
            "reputation": reputation,
            "created_at": identity.created_at,
            "updated_at": identity.updated_at,
            "skill_count": identity.skill_count,
            "vouch_count": identity.vouch_count,
            "is_active": identity.is_active,
        },
        "licn_name": licn_name,
        "reputation": {
            "score": reputation,
            "tier": tier,
            "tier_name": lichenid_trust_tier_name(tier),
        },
        "skills": skills,
        "vouches": {
            "received": received_vouches,
            "given": given_vouches,
        },
        "achievements": achievements,
        "agent": {
            "endpoint": endpoint,
            "metadata": metadata,
            "availability": availability,
            "availability_name": availability_name,
            "rate": rate,
        },
        "contributions": contributions,
    }))
}

fn normalize_licn_label(input: &str) -> String {
    input
        .trim()
        .to_ascii_lowercase()
        .trim_end_matches(".lichen")
        .to_string()
}

async fn handle_resolve_licn_name(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let raw_name = extract_single_string(&params, "resolveLichenName", "name")?;
    let label = normalize_licn_label(&raw_name);
    if label.is_empty() {
        return Ok(serde_json::Value::Null);
    }

    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let key = format!("name:{}", label).into_bytes();

    let Some(record) = lichenid_cf_get(state, &key) else {
        return Ok(serde_json::Value::Null);
    };
    if record.len() < 48 {
        return Ok(serde_json::Value::Null);
    }
    let expiry_slot = read_u64_le(&record, 40).unwrap_or(0);
    if current_slot >= expiry_slot {
        return Ok(serde_json::Value::Null);
    }

    let mut owner_bytes = [0u8; 32];
    owner_bytes.copy_from_slice(&record[0..32]);
    let owner = Pubkey(owner_bytes);
    Ok(serde_json::json!({
        "name": format!("{}.lichen", label),
        "owner": owner.to_base58(),
        "registered_slot": read_u64_le(&record, 32).unwrap_or(0),
        "expiry_slot": expiry_slot,
    }))
}

async fn handle_reverse_licn_name(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "reverseLichenName")?;
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    match get_lichenid_name(state, &pubkey, current_slot) {
        Some(name) => Ok(serde_json::json!({"name": name})),
        None => Ok(serde_json::Value::Null),
    }
}

async fn handle_batch_reverse_licn_names(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.as_ref().ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let addresses = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [pubkey1, pubkey2, ...]".to_string(),
    })?;

    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let mut output = serde_json::Map::new();
    for value in addresses.iter().take(500) {
        let Some(address_str) = value.as_str() else {
            continue;
        };
        let parsed = Pubkey::from_base58(address_str).ok();
        let name = parsed.and_then(|pubkey| get_lichenid_name(state, &pubkey, current_slot));
        output.insert(
            address_str.to_string(),
            name.map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
    }

    Ok(serde_json::Value::Object(output))
}

async fn handle_search_licn_names(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let prefix_raw = extract_single_string(&params, "searchLichenNames", "prefix")?;
    let prefix = normalize_licn_label(&prefix_raw);
    let program = resolve_symbol_pubkey(state, LICHENID_SYMBOL)?;
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let entries = state
        .state
        .get_contract_storage_entries(&program, 10_000, None)
        .unwrap_or_default();

    let mut names = Vec::new();
    for (key, value) in &entries {
        if !key.starts_with(b"name:") || key.starts_with(b"name_rev:") {
            continue;
        }
        let Ok(key_str) = std::str::from_utf8(key) else {
            continue;
        };
        let Some(label) = key_str.strip_prefix("name:") else {
            continue;
        };
        if !label.starts_with(&prefix) {
            continue;
        }
        if value.len() < 48 {
            continue;
        }
        let expiry_slot = read_u64_le(value, 40).unwrap_or(0);
        if current_slot >= expiry_slot {
            continue;
        }
        names.push(format!("{}.lichen", label));
        if names.len() >= 100 {
            break;
        }
    }

    names.sort_unstable();
    names.truncate(100);
    Ok(serde_json::json!(names))
}

async fn handle_get_lichenid_agent_directory(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let program = resolve_symbol_pubkey(state, LICHENID_SYMBOL)?;
    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let mut filter_type: Option<u8> = None;
    let mut filter_available: Option<bool> = None;
    let mut min_reputation: Option<u64> = None;
    let mut limit: usize = 50;
    let mut offset: usize = 0;

    if let Some(value) = params {
        let options_obj = if let Some(array) = value.as_array() {
            array.first().and_then(|entry| entry.as_object())
        } else {
            value.as_object()
        };

        if let Some(options) = options_obj {
            filter_type = options
                .get("type")
                .and_then(|v| v.as_u64())
                .map(|v| v as u8);
            filter_available = options.get("available").and_then(|v| v.as_bool());
            min_reputation = options.get("min_reputation").and_then(|v| v.as_u64());
            limit = options
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(50)
                .min(500) as usize;
            offset = options.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        }
    }

    let entries = state
        .state
        .get_contract_storage_entries(&program, 10_000, None)
        .unwrap_or_default();
    let mut agents = Vec::new();
    for (key, value) in &entries {
        if !key.starts_with(b"id:") {
            continue;
        }
        let Some(identity) = parse_lichenid_identity_record(value) else {
            continue;
        };
        if !identity.is_active {
            continue;
        }
        if let Some(required_type) = filter_type {
            if identity.agent_type != required_type {
                continue;
            }
        }

        let pubkey = identity.owner;
        let reputation = get_lichenid_reputation(state, &pubkey).unwrap_or(identity.reputation);
        if let Some(minimum) = min_reputation {
            if reputation < minimum {
                continue;
            }
        }

        let availability = lichenid_cf_get(
            state,
            &format!("availability:{}", lichenid_hex(&pubkey)).into_bytes(),
        )
        .and_then(|raw| raw.first().copied())
        .unwrap_or(0);
        let is_available = availability == 1;
        if let Some(required_available) = filter_available {
            if required_available != is_available {
                continue;
            }
        }

        let rate = lichenid_cf_get(
            state,
            &format!("rate:{}", lichenid_hex(&pubkey)).into_bytes(),
        )
        .and_then(|raw| read_u64_le(&raw, 0))
        .unwrap_or(0);

        let endpoint = lichenid_cf_get(
            state,
            &format!("endpoint:{}", lichenid_hex(&pubkey)).into_bytes(),
        )
        .and_then(|raw| String::from_utf8(raw).ok());

        let tier = lichenid_trust_tier(reputation);

        agents.push(serde_json::json!({
            "address": pubkey.to_base58(),
            "name": identity.name,
            "licn_name": get_lichenid_name(state, &pubkey, current_slot),
            "agent_type": identity.agent_type,
            "agent_type_name": lichenid_agent_type_name(identity.agent_type),
            "reputation": reputation,
            "trust_tier": tier,
            "trust_tier_name": lichenid_trust_tier_name(tier),
            "availability": availability,
            "available": is_available,
            "rate": rate,
            "endpoint": endpoint,
            "skill_count": identity.skill_count,
            "vouch_count": identity.vouch_count,
            "created_at": identity.created_at,
            "updated_at": identity.updated_at,
        }));

        if agents.len() >= 10_000 {
            break;
        }
    }

    agents.sort_by(|a, b| {
        let left = a.get("reputation").and_then(|v| v.as_u64()).unwrap_or(0);
        let right = b.get("reputation").and_then(|v| v.as_u64()).unwrap_or(0);
        right.cmp(&left)
    });

    let total = agents.len();
    let agents: Vec<serde_json::Value> = agents.into_iter().skip(offset).take(limit).collect();

    Ok(serde_json::json!({
        "agents": agents,
        "count": agents.len(),
        "total": total,
    }))
}

async fn handle_get_lichenid_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let program = resolve_symbol_pubkey(state, "YID")?;

    let total_identities = cf_stats_u64(state, "YID", b"mid_identity_count");
    let total_names = cf_stats_u64(state, "YID", b"licn_name_count");

    // PERF-NOTE (P10-VAL-07): Full CF storage scan for tier distribution.
    // Acceptable for current identity counts. Consider caching or contract-side
    // aggregate counters if identity count exceeds 100K.
    let entries = state
        .state
        .get_contract_storage_entries(&program, 100_000, None)
        .unwrap_or_default();

    let mut tier_distribution = [0u64; 6];
    let mut total_skills: u64 = 0;
    let mut total_vouches: u64 = 0;
    let mut total_attestations: u64 = 0;

    for (key, value) in &entries {
        if key.starts_with(b"id:") {
            let Some(identity) = parse_lichenid_identity_record(value) else {
                continue;
            };
            // Read reputation from CF
            let rep_key = lichenid_reputation_key(&identity.owner);
            let score = state
                .state
                .get_program_storage("YID", &rep_key)
                .and_then(|v| read_u64_le(&v, 0))
                .unwrap_or(identity.reputation);
            tier_distribution[lichenid_trust_tier(score) as usize] += 1;
            total_skills += identity.skill_count as u64;
            total_vouches += identity.vouch_count as u64;
        } else if key.starts_with(b"attest_count_") {
            total_attestations += read_u64_le(value, 0).unwrap_or(0);
        }
    }

    Ok(serde_json::json!({
        "total_identities": total_identities,
        "total_names": total_names,
        "total_skills": total_skills,
        "total_vouches": total_vouches,
        "total_attestations": total_attestations,
        "tier_distribution": {
            "newcomer": tier_distribution[0],
            "verified": tier_distribution[1],
            "trusted": tier_distribution[2],
            "established": tier_distribution[3],
            "elite": tier_distribution[4],
            "legendary": tier_distribution[5],
        },
    }))
}

/// getNameAuction — Query auction state for a .lichen name
async fn handle_get_name_auction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let name = params
        .as_ref()
        .and_then(|p| p.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Expected [name] parameter".to_string(),
        })?;

    // Normalize: strip .lichen suffix
    let normalized = name.to_lowercase().trim_end_matches(".lichen").to_string();

    // Build the auction key: "name_auc:" + name bytes
    let mut key = Vec::with_capacity(9 + normalized.len());
    key.extend_from_slice(b"name_auc:");
    key.extend_from_slice(normalized.as_bytes());

    let data = match lichenid_cf_get(state, &key) {
        Some(d) if d.len() >= 65 => d,
        _ => return Ok(serde_json::Value::Null),
    };

    let active = data[0] == 1;
    let start_slot = read_u64_le(&data, 1).unwrap_or(0);
    let end_slot = read_u64_le(&data, 9).unwrap_or(0);
    let reserve_bid = read_u64_le(&data, 17).unwrap_or(0);
    let highest_bid = read_u64_le(&data, 25).unwrap_or(0);

    let mut highest_bidder = [0u8; 32];
    highest_bidder.copy_from_slice(&data[33..65]);
    let bidder_pubkey = Pubkey(highest_bidder);
    let current_slot = state.state.get_last_slot().unwrap_or(0);

    Ok(serde_json::json!({
        "name": normalized,
        "active": active,
        "start_slot": start_slot,
        "end_slot": end_slot,
        "reserve_bid": reserve_bid,
        "highest_bid": highest_bid,
        "highest_bidder": bidder_pubkey.to_base58(),
        "current_slot": current_slot,
        "ended": current_slot >= end_slot,
    }))
}

// ============================================================================
// NFT ENDPOINTS
// ============================================================================

async fn handle_get_collection(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let collection_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [collection_pubkey]".to_string(),
        })?;

    let collection_pubkey = Pubkey::from_base58(collection_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid collection pubkey: {}", e),
    })?;

    let account = state
        .state
        .get_account(&collection_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "Collection not found".to_string(),
    })?;

    let collection = decode_collection_state(&account.data).map_err(|e| RpcError {
        code: -32002,
        message: format!("Invalid collection data: {}", e),
    })?;

    Ok(serde_json::json!({
        "collection": collection_pubkey.to_base58(),
        "name": collection.name,
        "symbol": collection.symbol,
        "creator": collection.creator.to_base58(),
        "royalty_bps": collection.royalty_bps,
        "max_supply": collection.max_supply,
        "minted": collection.minted,
        "public_mint": collection.public_mint,
        "mint_authority": collection.mint_authority.map(|p| p.to_base58()),
    }))
}

async fn handle_get_nft(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [collection_pubkey, token_id] or [token_pubkey]"
            .to_string(),
    })?;

    // Support two calling conventions:
    //   [collection_pubkey, token_id] -- derive token address from collection + token_id
    //   [token_pubkey]                -- direct token account lookup
    let token_pubkey = if arr.len() >= 2 {
        // [collection_pubkey, token_id] form
        let collection_str = arr[0].as_str().ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid collection pubkey".to_string(),
        })?;
        let token_id = arr[1].as_u64().ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid token_id".to_string(),
        })?;
        let collection_pubkey = Pubkey::from_base58(collection_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid collection pubkey: {}", e),
        })?;
        // Derive token address: SHA-256(collection_bytes + token_id_le_bytes)
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(collection_pubkey.0);
        hasher.update(token_id.to_le_bytes());
        let hash = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&hash[..32]);
        Pubkey(bytes)
    } else {
        // [token_pubkey] form (direct lookup)
        let token_str = arr[0].as_str().ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid token pubkey".to_string(),
        })?;
        Pubkey::from_base58(token_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid token pubkey: {}", e),
        })?
    };

    let account = state
        .state
        .get_account(&token_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "NFT not found".to_string(),
    })?;

    let token = decode_token_state(&account.data).map_err(|e| RpcError {
        code: -32002,
        message: format!("Invalid token data: {}", e),
    })?;

    Ok(serde_json::json!({
        "token": token_pubkey.to_base58(),
        "collection": token.collection.to_base58(),
        "token_id": token.token_id,
        "owner": token.owner.to_base58(),
        "metadata_uri": token.metadata_uri,
    }))
}

async fn handle_get_nfts_by_owner(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [owner_pubkey, options?]".to_string(),
    })?;

    let owner_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [owner_pubkey, options?]".to_string(),
        })?;

    let limit = arr
        .get(1)
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;

    let owner = Pubkey::from_base58(owner_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid owner pubkey: {}", e),
    })?;

    let token_pubkeys = state
        .state
        .get_nft_tokens_by_owner(&owner, limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account_map = state
        .state
        .get_accounts_batch(&token_pubkeys)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mut items = Vec::new();
    for token_pubkey in token_pubkeys {
        let Some(account) = account_map.get(&token_pubkey) else {
            continue;
        };

        let token = match decode_token_state(&account.data) {
            Ok(token) => token,
            Err(_) => continue,
        };

        items.push(serde_json::json!({
            "token": token_pubkey.to_base58(),
            "collection": token.collection.to_base58(),
            "token_id": token.token_id,
            "owner": token.owner.to_base58(),
            "metadata_uri": token.metadata_uri,
        }));
    }

    Ok(serde_json::json!({
        "owner": owner.to_base58(),
        "count": items.len(),
        "nfts": items,
    }))
}

async fn handle_get_nfts_by_collection(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [collection_pubkey, options?]".to_string(),
    })?;

    let collection_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [collection_pubkey, options?]".to_string(),
        })?;

    let limit = arr
        .get(1)
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;

    let collection = Pubkey::from_base58(collection_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid collection pubkey: {}", e),
    })?;

    let token_pubkeys = state
        .state
        .get_nft_tokens_by_collection(&collection, limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account_map = state
        .state
        .get_accounts_batch(&token_pubkeys)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mut items = Vec::new();
    for token_pubkey in token_pubkeys {
        let Some(account) = account_map.get(&token_pubkey) else {
            continue;
        };

        let token = match decode_token_state(&account.data) {
            Ok(token) => token,
            Err(_) => continue,
        };

        items.push(serde_json::json!({
            "token": token_pubkey.to_base58(),
            "collection": token.collection.to_base58(),
            "token_id": token.token_id,
            "owner": token.owner.to_base58(),
            "metadata_uri": token.metadata_uri,
        }));
    }

    Ok(serde_json::json!({
        "collection": collection.to_base58(),
        "count": items.len(),
        "nfts": items,
    }))
}

async fn handle_get_nft_activity(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [collection_pubkey, options?]".to_string(),
    })?;

    let collection_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [collection_pubkey, options?]".to_string(),
        })?;

    let limit = arr
        .get(1)
        .and_then(|v| {
            if v.is_object() {
                v.get("limit").and_then(|val| val.as_u64())
            } else {
                v.as_u64()
            }
        })
        .unwrap_or(50)
        .min(500) as usize;

    let collection = Pubkey::from_base58(collection_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid collection pubkey: {}", e),
    })?;

    let activities = state
        .state
        .get_nft_activity_by_collection(&collection, limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let items: Vec<serde_json::Value> = activities
        .into_iter()
        .map(|activity| {
            let kind = match activity.kind {
                NftActivityKind::Mint => "mint",
                NftActivityKind::Transfer => "transfer",
            };

            serde_json::json!({
                "slot": activity.slot,
                "timestamp": activity.timestamp,
                "kind": kind,
                "collection": activity.collection.to_base58(),
                "token": activity.token.to_base58(),
                "from": activity.from.map(|p| p.to_base58()),
                "to": activity.to.to_base58(),
                "tx_signature": activity.tx_signature.to_hex(),
            })
        })
        .collect();

    Ok(serde_json::json!({
        "collection": collection.to_base58(),
        "count": items.len(),
        "activity": items,
    }))
}

// ============================================================================
// MARKETPLACE ENDPOINTS
// ============================================================================

/// Extended marketplace filter parameters (v3)
struct MarketFilterParams {
    collection: Option<Pubkey>,
    limit: usize,
    price_min: Option<u64>,
    price_max: Option<u64>,
    seller: Option<Pubkey>,
    sort_by: Option<String>,
    category: Option<u8>,
    rarity: Option<u8>,
}

fn parse_market_params_extended(
    params: &Option<serde_json::Value>,
) -> Result<MarketFilterParams, RpcError> {
    let limit_default = 50usize;

    let Some(params) = params else {
        return Ok(MarketFilterParams {
            collection: None,
            limit: limit_default,
            price_min: None,
            price_max: None,
            seller: None,
            sort_by: None,
            category: None,
            rarity: None,
        });
    };

    // Object-form: { collection, limit, price_min, price_max, seller, sort_by, category, rarity }
    if let Some(obj) = params.as_object() {
        let collection = obj
            .get("collection")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(Pubkey::from_base58)
            .transpose()
            .map_err(|e| RpcError {
                code: -32602,
                message: format!("Invalid collection pubkey: {}", e),
            })?;

        let limit = obj
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(limit_default as u64)
            .min(500) as usize;

        let price_min = obj.get("price_min").and_then(|v| v.as_u64());
        let price_max = obj.get("price_max").and_then(|v| v.as_u64());
        let seller = obj
            .get("seller")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(Pubkey::from_base58)
            .transpose()
            .map_err(|e| RpcError {
                code: -32602,
                message: format!("Invalid seller pubkey: {}", e),
            })?;
        let sort_by = obj
            .get("sort_by")
            .and_then(|v| v.as_str())
            .map(String::from);
        let category = obj
            .get("category")
            .and_then(|v| v.as_u64())
            .map(|v| v as u8);
        let rarity = obj.get("rarity").and_then(|v| v.as_u64()).map(|v| v as u8);

        return Ok(MarketFilterParams {
            collection,
            limit,
            price_min,
            price_max,
            seller,
            sort_by,
            category,
            rarity,
        });
    }

    // Array-form (legacy): [collection_pubkey?, options?]
    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message:
            "Invalid params: expected [collection_pubkey?, options?] or {collection, limit, ...}"
                .to_string(),
    })?;

    let (collection, limit) = match arr.first() {
        Some(first) if first.is_object() => {
            let limit = first
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(limit_default as u64)
                .min(500) as usize;
            (None, limit)
        }
        _ => {
            let collection = arr
                .first()
                .and_then(|v| v.as_str())
                .map(Pubkey::from_base58)
                .transpose()
                .map_err(|e| RpcError {
                    code: -32602,
                    message: format!("Invalid collection pubkey: {}", e),
                })?;
            let limit = arr
                .get(1)
                .and_then(|v| {
                    if v.is_object() {
                        v.get("limit").and_then(|val| val.as_u64())
                    } else {
                        v.as_u64()
                    }
                })
                .unwrap_or(limit_default as u64)
                .min(500) as usize;
            (collection, limit)
        }
    };

    Ok(MarketFilterParams {
        collection,
        limit,
        price_min: None,
        price_max: None,
        seller: None,
        sort_by: None,
        category: None,
        rarity: None,
    })
}

/// Legacy wrapper for backward compat (sales endpoint)
fn parse_market_params(
    params: Option<serde_json::Value>,
) -> Result<(Option<Pubkey>, usize), RpcError> {
    let ext = parse_market_params_extended(&params)?;
    Ok((ext.collection, ext.limit))
}

fn market_activity_to_json(activity: &lichen_core::MarketActivity) -> serde_json::Value {
    let kind = match activity.kind {
        MarketActivityKind::Listing => "listing",
        MarketActivityKind::Sale => "sale",
        MarketActivityKind::Cancel => "cancel",
        MarketActivityKind::Offer => "offer",
        MarketActivityKind::OfferAccepted => "offer_accepted",
        MarketActivityKind::OfferCancelled => "offer_cancelled",
        MarketActivityKind::PriceUpdate => "price_update",
        MarketActivityKind::AuctionCreated => "auction_created",
        MarketActivityKind::AuctionBid => "auction_bid",
        MarketActivityKind::AuctionSettled => "auction_settled",
        MarketActivityKind::AuctionCancelled => "auction_cancelled",
        MarketActivityKind::CollectionOffer => "collection_offer",
        MarketActivityKind::CollectionOfferAccepted => "collection_offer_accepted",
        MarketActivityKind::Transfer => "transfer",
    };

    serde_json::json!({
        "slot": activity.slot,
        "timestamp": activity.timestamp,
        "kind": kind,
        "program": activity.program.to_base58(),
        "collection": activity.collection.as_ref().map(|p| p.to_base58()),
        "token": activity.token.as_ref().map(|p| p.to_base58()),
        "token_id": activity.token_id,
        "price": activity.price,
        "price_licn": activity.price.map(|val| val as f64 / 1_000_000_000.0),
        "seller": activity.seller.as_ref().map(|p| p.to_base58()),
        "buyer": activity.buyer.as_ref().map(|p| p.to_base58()),
        "function": activity.function.clone(),
        "tx_signature": activity.tx_signature.to_hex(),
    })
}

async fn handle_get_market_listings(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let filters = parse_market_params_extended(&params)?;

    let has_post_filters = filters.price_min.is_some()
        || filters.price_max.is_some()
        || filters.seller.is_some()
        || filters.category.is_some()
        || filters.rarity.is_some();

    // RPC-H09: cap unfiltered marketplace requests to a reasonable page size.
    let effective_limit = if filters.collection.is_none() && !has_post_filters {
        filters.limit.min(MARKET_LISTINGS_UNFILTERED_MAX_LIMIT)
    } else {
        filters.limit
    };

    // Fetch more than limit to allow post-filtering
    let fetch_limit = if has_post_filters {
        (effective_limit * 5).min(2000)
    } else {
        effective_limit
    };

    let activity = state
        .state
        .get_market_activity(
            filters.collection.as_ref(),
            Some(MarketActivityKind::Listing),
            fetch_limit,
        )
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    // Apply multi-criteria filters
    let mut filtered: Vec<&lichen_core::MarketActivity> = activity
        .iter()
        .filter(|a| {
            // Price range filter (in spores)
            if let Some(min) = filters.price_min {
                if a.price.unwrap_or(0) < min {
                    return false;
                }
            }
            if let Some(max) = filters.price_max {
                if a.price.unwrap_or(u64::MAX) > max {
                    return false;
                }
            }
            // Seller filter
            if let Some(ref seller) = filters.seller {
                if a.seller.as_ref() != Some(seller) {
                    return false;
                }
            }
            true
        })
        .collect();

    // Sort
    if let Some(ref sort_by) = filters.sort_by {
        match sort_by.as_str() {
            "price_asc" => filtered.sort_by_key(|a| a.price.unwrap_or(0)),
            "price_desc" => filtered.sort_by_key(|b| std::cmp::Reverse(b.price.unwrap_or(0))),
            "oldest" => filtered.sort_by_key(|a| a.timestamp),
            _ => {} // newest first (default from DB)
        }
    }

    // Apply limit after filtering
    filtered.truncate(effective_limit);

    let items: Vec<serde_json::Value> = filtered
        .iter()
        .map(|a| market_activity_to_json(a))
        .collect();

    Ok(serde_json::json!({
        "collection": filters.collection.map(|c| c.to_base58()),
        "count": items.len(),
        "listings": items,
        "filters": {
            "price_min": filters.price_min,
            "price_max": filters.price_max,
            "seller": filters.seller.map(|s| s.to_base58()),
            "sort_by": filters.sort_by,
            "category": filters.category,
            "rarity": filters.rarity,
        }
    }))
}

async fn handle_get_market_sales(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let (collection, limit) = parse_market_params(params)?;

    let activity = state
        .state
        .get_market_activity(collection.as_ref(), Some(MarketActivityKind::Sale), limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let items: Vec<serde_json::Value> = activity.iter().map(market_activity_to_json).collect();

    Ok(serde_json::json!({
        "collection": collection.map(|c| c.to_base58()),
        "count": items.len(),
        "sales": items,
    }))
}

/// Get marketplace offers (filtered by activity kind = Offer, OfferAccepted)
async fn handle_get_market_offers(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let (collection, limit) = parse_market_params(params.clone())?;

    let token_id_filter = params
        .as_ref()
        .and_then(|p| p.as_object())
        .and_then(|obj| obj.get("token_id"))
        .and_then(|v| v.as_u64());

    let token_filter = params
        .as_ref()
        .and_then(|p| p.as_object())
        .and_then(|obj| obj.get("token"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(Pubkey::from_base58)
        .transpose()
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid token pubkey: {}", e),
        })?;

    let include_collection_offers = params
        .as_ref()
        .and_then(|p| p.as_object())
        .and_then(|obj| obj.get("include_collection_offers"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let fetch_limit = if token_id_filter.is_some() || token_filter.is_some() {
        (limit.saturating_mul(10)).clamp(limit, 2000)
    } else {
        limit
    };

    let activity = state
        .state
        .get_market_activity(
            collection.as_ref(),
            Some(MarketActivityKind::Offer),
            fetch_limit,
        )
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mut all_activity = activity;
    if include_collection_offers {
        let collection_activity = state
            .state
            .get_market_activity(
                collection.as_ref(),
                Some(MarketActivityKind::CollectionOffer),
                fetch_limit,
            )
            .map_err(|e| RpcError {
                code: -32000,
                message: format!("Database error: {}", e),
            })?;
        all_activity.extend(collection_activity);
    }

    let mut filtered: Vec<&lichen_core::MarketActivity> = all_activity
        .iter()
        .filter(|a| {
            if !include_collection_offers && a.kind == MarketActivityKind::CollectionOffer {
                return false;
            }
            if a.kind == MarketActivityKind::CollectionOffer {
                return true;
            }
            if let Some(token_id) = token_id_filter {
                if a.token_id != Some(token_id) {
                    return false;
                }
            }
            if let Some(token) = token_filter.as_ref() {
                if a.token.as_ref() != Some(token) {
                    return false;
                }
            }
            true
        })
        .collect();

    filtered.sort_by_key(|b| std::cmp::Reverse(b.timestamp));

    filtered.truncate(limit);

    let items: Vec<serde_json::Value> = filtered
        .iter()
        .map(|a| market_activity_to_json(a))
        .collect();

    Ok(serde_json::json!({
        "collection": collection.map(|c| c.to_base58()),
        "count": items.len(),
        "offers": items,
    }))
}

/// Get marketplace auctions (filtered by activity kind = AuctionCreated)
async fn handle_get_market_auctions(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let (collection, limit) = parse_market_params(params)?;

    let activity = state
        .state
        .get_market_activity(
            collection.as_ref(),
            Some(MarketActivityKind::AuctionCreated),
            limit,
        )
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let items: Vec<serde_json::Value> = activity.iter().map(market_activity_to_json).collect();

    Ok(serde_json::json!({
        "collection": collection.map(|c| c.to_base58()),
        "count": items.len(),
        "auctions": items,
    }))
}

// ============================================================================
// ETHEREUM JSON-RPC COMPATIBILITY LAYER (MetaMask Support)
// ============================================================================

/// eth_getBalance - Get balance in wei format
async fn handle_eth_get_balance(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let evm_address_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [address, block]".to_string(),
        })?;

    // Parse EVM address
    let evm_address =
        lichen_core::StateStore::parse_evm_address(evm_address_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid EVM address: {}", e),
        })?;

    // Lookup native pubkey
    let native_pubkey = state
        .state
        .lookup_evm_address(&evm_address)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let balance = if let Some(pubkey) = native_pubkey {
        let account = state.state.get_account(&pubkey).map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;
        let spendable = account.map(|a| a.spendable).unwrap_or(0);
        spores_to_u256(spendable)
    } else if let Some(account) =
        state
            .state
            .get_evm_account(&evm_address)
            .map_err(|e| RpcError {
                code: -32000,
                message: format!("Database error: {}", e),
            })?
    {
        account.balance_u256()
    } else {
        spores_to_u256(0)
    };

    Ok(serde_json::json!(format!("0x{:x}", balance)))
}

/// eth_sendRawTransaction - Submit signed Ethereum transaction
async fn handle_eth_send_raw_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let tx_data = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [signedTxData]".to_string(),
        })?;

    let tx_hex = tx_data.strip_prefix("0x").unwrap_or(tx_data);
    let raw = hex::decode(tx_hex).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid tx hex: {}", e),
    })?;

    let evm_tx = decode_evm_transaction(&raw).map_err(|e| RpcError {
        code: -32602,
        message: e,
    })?;

    if let Some(chain_id) = evm_tx.chain_id {
        if chain_id != state.evm_chain_id {
            return Err(RpcError {
                code: -32602,
                message: format!("Invalid chainId: {}", chain_id),
            });
        }
    }

    let from_address: [u8; 20] = evm_tx.from.into();
    let mapped_pubkey = state
        .state
        .lookup_evm_address(&from_address)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mapped_pubkey = mapped_pubkey.ok_or_else(|| RpcError {
        code: -32602,
        message: "EVM address not registered".to_string(),
    })?;

    let instruction = Instruction {
        program_id: EVM_PROGRAM_ID,
        accounts: vec![mapped_pubkey],
        data: raw,
    };

    let message = lichen_core::Message {
        instructions: vec![instruction],
        // EVM transactions use the sentinel blockhash for backward compatibility.
        // The Transaction::new_evm() constructor sets tx_type = Evm which is the
        // primary detection path; the sentinel is kept as a legacy fallback.
        recent_blockhash: lichen_core::EVM_SENTINEL_BLOCKHASH,
        compute_budget: None,
        compute_unit_price: None,
    };

    let tx = Transaction::new_evm(message);

    submit_transaction(state, tx)?;

    let evm_hash: [u8; 32] = evm_tx.hash.into();
    Ok(serde_json::json!(format!("0x{}", hex::encode(evm_hash))))
}

/// eth_call - Execute call without sending transaction (read-only)
async fn handle_eth_call(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let call = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_object())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [callObject, block]".to_string(),
        })?;

    let to = call.get("to").and_then(|v| v.as_str());
    let from = call.get("from").and_then(|v| v.as_str());
    let data = call.get("data").and_then(|v| v.as_str()).unwrap_or("0x");
    let value = call.get("value").and_then(|v| v.as_str()).unwrap_or("0x0");
    let gas = call.get("gas").and_then(|v| v.as_str()).unwrap_or("0x0");

    let to_address = to
        .map(lichen_core::StateStore::parse_evm_address)
        .transpose()
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid to address: {}", e),
        })?
        .map(Address::from);

    let from_address = if let Some(from) = from {
        let parsed = lichen_core::StateStore::parse_evm_address(from).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid from address: {}", e),
        })?;
        Address::from(parsed)
    } else {
        Address::ZERO
    };

    let data_hex = data.strip_prefix("0x").unwrap_or(data);
    let data_bytes = hex::decode(data_hex).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid data hex: {}", e),
    })?;

    let value_hex = value.strip_prefix("0x").unwrap_or(value);
    let gas_hex = gas.strip_prefix("0x").unwrap_or(gas);
    let value_u256 = U256::from_str_radix(value_hex, 16).unwrap_or(U256::ZERO);
    let gas_limit = u64::from_str_radix(gas_hex, 16).unwrap_or(1_000_000);

    let output = simulate_evm_call(
        state.state.clone(),
        from_address,
        to_address,
        Bytes::from(data_bytes),
        value_u256,
        gas_limit,
        state.evm_chain_id,
    )
    .map_err(|e| RpcError {
        code: -32000,
        message: e,
    })?;

    Ok(serde_json::json!(format!("0x{}", hex::encode(output))))
}

/// eth_blockNumber - Get current block number (slot)
async fn handle_eth_block_number(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!(format!("0x{:x}", slot)))
}

/// eth_getTransactionReceipt - Get transaction receipt
async fn handle_eth_get_transaction_receipt(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let tx_hash_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [txHash]".to_string(),
        })?;

    // Parse transaction hash (strip 0x and convert to Hash)
    let tx_hash_str = tx_hash_str.strip_prefix("0x").unwrap_or(tx_hash_str);
    let hash = lichen_core::hash::Hash::from_hex(tx_hash_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid transaction hash: {}", e),
    })?;

    let receipt = state.state.get_evm_receipt(&hash.0).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    if let Some(receipt) = receipt {
        let status = if receipt.status { "0x1" } else { "0x0" };
        let block_number = receipt.block_slot.map(|slot| format!("0x{:x}", slot));
        let block_hash = receipt
            .block_hash
            .map(|hash| format!("0x{}", hex::encode(hash)));
        let contract_address = receipt
            .contract_address
            .map(|addr| format!("0x{}", hex::encode(addr)));

        // Task 3.4: Return structured EVM logs in receipt
        let receipt_logs: Vec<serde_json::Value> = receipt
            .structured_logs
            .iter()
            .enumerate()
            .map(|(i, log)| {
                let topics: Vec<serde_json::Value> = log
                    .topics
                    .iter()
                    .map(|t| serde_json::Value::String(format!("0x{}", hex::encode(t))))
                    .collect();
                serde_json::json!({
                    "address": format!("0x{}", hex::encode(log.address)),
                    "topics": topics,
                    "data": format!("0x{}", hex::encode(&log.data)),
                    "logIndex": format!("0x{:x}", i),
                    "blockNumber": block_number,
                    "blockHash": block_hash,
                    "transactionHash": format!("0x{}", hex::encode(receipt.evm_hash)),
                    "transactionIndex": "0x0",
                    "removed": false,
                })
            })
            .collect();

        return Ok(serde_json::json!({
            "transactionHash": format!("0x{}", hex::encode(receipt.evm_hash)),
            "status": status,
            "gasUsed": format!("0x{:x}", receipt.gas_used),
            "blockNumber": block_number,
            "blockHash": block_hash,
            "contractAddress": contract_address,
            "logs": receipt_logs,
            "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        }));
    }

    Ok(serde_json::json!(null))
}

/// eth_getTransactionByHash - Get transaction by hash
async fn handle_eth_get_transaction_by_hash(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let tx_hash_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [txHash]".to_string(),
        })?;

    // Parse transaction hash
    let tx_hash_str = tx_hash_str.strip_prefix("0x").unwrap_or(tx_hash_str);
    let hash = lichen_core::hash::Hash::from_hex(tx_hash_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid transaction hash: {}", e),
    })?;

    let record = state.state.get_evm_tx(&hash.0).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    if let Some(record) = record {
        let value = U256::from_be_bytes(record.value);
        let gas_price = U256::from_be_bytes(record.gas_price);
        let block_number = record.block_slot.map(|slot| format!("0x{:x}", slot));
        let block_hash = record
            .block_hash
            .map(|hash| format!("0x{}", hex::encode(hash)));
        return Ok(serde_json::json!({
            "hash": format!("0x{}", hex::encode(record.evm_hash)),
            "from": format!("0x{}", hex::encode(record.from)),
            "to": record.to.map(|addr| format!("0x{}", hex::encode(addr))),
            "nonce": format!("0x{:x}", record.nonce),
            "value": format!("0x{:x}", value),
            "gas": format!("0x{:x}", record.gas_limit),
            "gasPrice": format!("0x{:x}", gas_price),
            "input": format!("0x{}", hex::encode(record.data)),
            "blockNumber": block_number,
            "blockHash": block_hash,
        }));
    }

    Ok(serde_json::json!(null))
}

// ═══════════════════════════════════════════════════════════════════════════════
// EVM COMPATIBILITY HANDLERS (full implementations)
// ═══════════════════════════════════════════════════════════════════════════════

/// Parse an EVM block tag ("latest", "earliest", "pending", "safe", "finalized", or hex number)
/// into a slot number.
fn parse_evm_block_tag(tag: &str, state: &RpcState) -> Result<u64, RpcError> {
    match tag {
        "latest" | "pending" | "safe" | "finalized" => {
            state.state.get_last_slot().map_err(|e| RpcError {
                code: -32000,
                message: format!("Database error: {}", e),
            })
        }
        "earliest" => Ok(0),
        hex if hex.starts_with("0x") => u64::from_str_radix(hex.trim_start_matches("0x"), 16)
            .map_err(|_| RpcError {
                code: -32602,
                message: format!("Invalid block number: {}", hex),
            }),
        _ => Err(RpcError {
            code: -32602,
            message: format!("Invalid block tag: {}", tag),
        }),
    }
}

/// Format a real Block into EVM-compatible JSON block object.
fn format_evm_block(block: &lichen_core::Block, include_txs: bool) -> serde_json::Value {
    let slot = block.header.slot;
    // AUDIT-FIX P10-RPC-01: Use actual block hash, NOT state_root.
    // state_root is a Merkle root of account state — it is NOT the block identifier.
    // EVM tooling (MetaMask, Ethers.js) uses the "hash" field to track/index blocks.
    let block_hash = format!("0x{}", hex::encode(block.hash().0));
    let parent_hash = format!("0x{}", hex::encode(block.header.parent_hash.0));
    let timestamp = format!("0x{:x}", block.header.timestamp);
    let tx_root = format!("0x{}", hex::encode(block.header.tx_root.0));
    let validator = format!("0x{}", hex::encode(&block.header.validator[12..32]));

    let transactions: serde_json::Value = if include_txs {
        serde_json::json!(block
            .transactions
            .iter()
            .map(|tx| {
                let sig = tx.signature();
                let from_addr = tx
                    .message
                    .instructions
                    .first()
                    .and_then(|ix| ix.accounts.first())
                    .map(|acc| format!("0x{}", hex::encode(&acc.0[12..32])))
                    .unwrap_or_else(|| "0x0000000000000000000000000000000000000000".to_string());
                serde_json::json!({
                    "hash": format!("0x{}", hex::encode(sig.0)),
                    "blockNumber": format!("0x{:x}", slot),
                    "blockHash": &block_hash,
                    "from": from_addr,
                    "to": tx.message.instructions.first().map(|ix|
                        format!("0x{}", hex::encode(&ix.program_id.0[12..32]))
                    ),
                    "value": "0x0",
                    "gas": "0x5208",
                    "gasPrice": "0x0",
                    "input": "0x",
                    "nonce": "0x0",
                    "transactionIndex": "0x0",
                })
            })
            .collect::<Vec<_>>())
    } else {
        serde_json::json!(block
            .transactions
            .iter()
            .map(|tx| {
                let sig = tx.signature();
                serde_json::Value::String(format!("0x{}", hex::encode(sig.0)))
            })
            .collect::<Vec<_>>())
    };

    serde_json::json!({
        "number": format!("0x{:x}", slot),
        "hash": block_hash,
        "parentHash": parent_hash,
        "nonce": "0x0000000000000000",
        "sha3Uncles": "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
        "logsBloom": format!("0x{}", "00".repeat(256)),
        "transactionsRoot": tx_root,
        "stateRoot": format!("0x{}", hex::encode(block.header.state_root.0)),
        "receiptsRoot": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "miner": validator,
        "difficulty": "0x0",
        "totalDifficulty": "0x0",
        "extraData": "0x",
        "size": format!("0x{:x}", block.transactions.len() * 256 + 512),
        "gasLimit": "0x1c9c380",
        "gasUsed": format!("0x{:x}", block.transactions.len() * 21000),
        "timestamp": timestamp,
        "transactions": transactions,
        "uncles": [],
        "baseFeePerGas": "0x0",
        "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    })
}

/// eth_getCode — return contract bytecode at an address.
/// Checks EVM accounts first, then native contract accounts mapped via the EVM registry.
async fn handle_eth_get_code(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let addr_str = params
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [address, block?]".to_string(),
        })?;

    let evm_addr = lichen_core::StateStore::parse_evm_address(addr_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid EVM address: {}", e),
    })?;

    // 1. Check pure EVM account (has EVM bytecode stored directly)
    if let Ok(Some(acct)) = state.state.get_evm_account(&evm_addr) {
        if !acct.code.is_empty() {
            return Ok(serde_json::json!(format!("0x{}", hex::encode(&acct.code))));
        }
    }

    // 2. Check native contract mapped to this EVM address
    if let Ok(Some(pubkey)) = state.state.lookup_evm_address(&evm_addr) {
        if let Ok(Some(account)) = state.state.get_account(&pubkey) {
            if account.executable && !account.data.is_empty() {
                // Try to deserialize as ContractAccount to get WASM bytecode
                if let Ok(contract) = serde_json::from_slice::<ContractAccount>(&account.data) {
                    if !contract.code.is_empty() {
                        // Return WASM bytecode hexified — EVM tools see non-"0x" = contract
                        return Ok(serde_json::json!(format!(
                            "0x{}",
                            hex::encode(&contract.code)
                        )));
                    }
                }
                // Fallback: account.data is non-empty executable but not parsable
                // Return EIP-7702 designated invalid opcode sentinel
                return Ok(serde_json::json!("0xfe"));
            }
        }
    }

    // EOA or unknown address — no code
    Ok(serde_json::json!("0x"))
}

/// eth_getTransactionCount — return nonce for an address.
/// Checks EVM account nonce first, then counts native transactions.
async fn handle_eth_get_transaction_count(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let addr_str = params
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [address, block?]".to_string(),
        })?;

    let evm_addr = lichen_core::StateStore::parse_evm_address(addr_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid EVM address: {}", e),
    })?;

    // 1. Check EVM account nonce
    if let Ok(Some(acct)) = state.state.get_evm_account(&evm_addr) {
        return Ok(serde_json::json!(format!("0x{:x}", acct.nonce)));
    }

    // 2. Check native account transaction count
    if let Ok(Some(pubkey)) = state.state.lookup_evm_address(&evm_addr) {
        let count = state.state.count_account_txs(&pubkey).unwrap_or(0);
        return Ok(serde_json::json!(format!("0x{:x}", count)));
    }

    Ok(serde_json::json!("0x0"))
}

/// eth_estimateGas — estimate gas for a transaction.
/// Lichen uses flat fees, not gas-based metering.
/// Returns the actual fee (in spores) as the gas value with an implicit gasPrice of 1.
async fn handle_eth_estimate_gas(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let fee_config = state
        .state
        .get_fee_config()
        .unwrap_or_else(|_| lichen_core::FeeConfig::default_from_constants());

    let gas = if let Some(ref params) = params {
        if let Some(tx_obj) = params.as_array().and_then(|a| a.first()) {
            let to_field = tx_obj.get("to");
            let has_to = to_field
                .map(|v| !v.is_null() && v.as_str().is_some_and(|s| !s.is_empty()))
                .unwrap_or(false);
            let has_data = tx_obj
                .get("data")
                .or_else(|| tx_obj.get("input"))
                .and_then(|d| d.as_str())
                .is_some_and(|d| d.len() > 2); // "0x" alone = no data

            if !has_to && has_data {
                // Contract deployment
                fee_config.contract_deploy_fee
            } else {
                // Transfer or contract call — both cost base_fee
                fee_config.base_fee
            }
        } else {
            fee_config.base_fee
        }
    } else {
        fee_config.base_fee
    };

    Ok(serde_json::json!(format!("0x{:x}", gas)))
}

/// eth_gasPrice — return current gas price.
/// AUDIT-FIX A11-01: Lichen uses flat fees, so gasPrice = 1 (1 spore per gas unit).
/// Total cost = gasPrice(1) × estimateGas(actual_fee_in_spores) = actual_fee.
/// Previously this returned base_fee, causing MetaMask to display fee² (base_fee × base_fee).
async fn handle_eth_gas_price(_state: &RpcState) -> Result<serde_json::Value, RpcError> {
    // gasPrice = 1 spore per gas unit.
    // eth_estimateGas returns the actual fee in spores (= gas units consumed).
    // MetaMask/wallets compute: total = gasPrice × gasEstimate = 1 × fee = fee. ✓
    Ok(serde_json::json!("0x1"))
}

/// eth_getBlockByNumber — return full block data for a given block number or tag.
async fn handle_eth_get_block_by_number(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [blockTag, includeTxs?]".to_string(),
    })?;

    let block_tag = arr.first().and_then(|v| v.as_str()).unwrap_or("latest");
    let include_txs = arr.get(1).and_then(|v| v.as_bool()).unwrap_or(false);

    let slot = parse_evm_block_tag(block_tag, state)?;

    let block = state.state.get_block_by_slot(slot).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match block {
        Some(b) => Ok(format_evm_block(&b, include_txs)),
        None => Ok(serde_json::json!(null)),
    }
}

/// eth_getBlockByHash — return full block data for a given block hash.
async fn handle_eth_get_block_by_hash(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [blockHash, includeTxs?]".to_string(),
    })?;

    let hash_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing block hash".to_string(),
        })?;
    let include_txs = arr.get(1).and_then(|v| v.as_bool()).unwrap_or(false);

    let hash_hex = hash_str.strip_prefix("0x").unwrap_or(hash_str);
    let hash_bytes = hex::decode(hash_hex).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid block hash hex".to_string(),
    })?;

    if hash_bytes.len() != 32 {
        return Err(RpcError {
            code: -32602,
            message: "Block hash must be 32 bytes".to_string(),
        });
    }

    let mut hash_arr = [0u8; 32];
    hash_arr.copy_from_slice(&hash_bytes);

    let block = state
        .state
        .get_block(&Hash(hash_arr))
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    match block {
        Some(b) => Ok(format_evm_block(&b, include_txs)),
        None => Ok(serde_json::json!(null)),
    }
}

/// eth_getLogs — return contract event logs matching a filter.
/// Task 3.4: Reads from structured EVM log index first, then native contract events.
/// Supports EIP-1474 topics filtering: each position can be null (wildcard),
/// a single topic hash, or an array of topic hashes (OR matching).
async fn handle_eth_get_logs(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let filter = params
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(serde_json::json!({}));

    let latest_slot = state.state.get_last_slot().unwrap_or(0);

    let from_slot = filter
        .get("fromBlock")
        .and_then(|v| v.as_str())
        .map(|tag| parse_evm_block_tag(tag, state))
        .transpose()?
        .unwrap_or(latest_slot);

    let to_slot = filter
        .get("toBlock")
        .and_then(|v| v.as_str())
        .map(|tag| parse_evm_block_tag(tag, state))
        .transpose()?
        .unwrap_or(latest_slot);

    // Cap range to avoid unbounded scans (max 1000 blocks)
    let effective_from = if to_slot > 1000 && from_slot < to_slot.saturating_sub(1000) {
        to_slot.saturating_sub(1000)
    } else {
        from_slot
    };

    // Optional address filter (single address or array of addresses)
    let filter_addresses: Vec<[u8; 20]> = match filter.get("address") {
        Some(serde_json::Value::String(s)) => {
            vec![lichen_core::StateStore::parse_evm_address(s)
                .map_err(|_| invalid_address_filter_error())?]
        }
        Some(serde_json::Value::Array(arr)) => {
            let mut addrs = Vec::with_capacity(arr.len());
            for v in arr {
                if let Some(s) = v.as_str() {
                    addrs.push(
                        lichen_core::StateStore::parse_evm_address(s)
                            .map_err(|_| invalid_address_filter_error())?,
                    );
                }
            }
            addrs
        }
        _ => Vec::new(),
    };

    // Task 3.4: Parse EIP-1474 topics filter.
    // Each position can be: null (wildcard), "0x..." (single), or ["0x...", "0x..."] (OR).
    let filter_topics: Vec<Option<Vec<[u8; 32]>>> = filter
        .get("topics")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|t| match t {
                    serde_json::Value::Null => None,
                    serde_json::Value::String(s) => parse_topic_hash(s).map(|h| vec![h]),
                    serde_json::Value::Array(sub) => {
                        let hashes: Vec<[u8; 32]> = sub
                            .iter()
                            .filter_map(|v| v.as_str().and_then(parse_topic_hash))
                            .collect();
                        if hashes.is_empty() {
                            None
                        } else {
                            Some(hashes)
                        }
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();

    let mut logs = Vec::new();
    /// AUDIT-FIX F-13: Cap returned log count to prevent unbounded memory growth.
    const MAX_LOG_RESULTS: usize = 10_000;

    for slot in effective_from..=to_slot {
        // AUDIT-FIX F-5: Reset logIndex per block (EVM spec requires per-block indexing)
        let mut log_index: u64 = 0;

        // Cache block hash for this slot (used by both EVM logs and native events)
        let block_hash_str = state
            .state
            .get_block_by_slot(slot)
            .ok()
            .flatten()
            .map(|b| format!("0x{}", hex::encode(b.hash().0)))
            .unwrap_or_else(|| format!("0x{:064x}", slot));

        // ── Phase 1: Structured EVM logs (from actual EVM execution) ──
        if let Ok(evm_logs) = state.state.get_evm_logs_for_slot(slot) {
            for entry in &evm_logs {
                // Address filter
                if !filter_addresses.is_empty() && !filter_addresses.contains(&entry.log.address) {
                    continue;
                }

                // Topic filter (EIP-1474)
                if !lichen_core::topics_match(&entry.log.topics, &filter_topics) {
                    continue;
                }

                let topics_json: Vec<serde_json::Value> = entry
                    .log
                    .topics
                    .iter()
                    .map(|t| serde_json::Value::String(format!("0x{}", hex::encode(t))))
                    .collect();

                logs.push(serde_json::json!({
                    "address": format!("0x{}", hex::encode(entry.log.address)),
                    "topics": topics_json,
                    "data": format!("0x{}", hex::encode(&entry.log.data)),
                    "blockNumber": format!("0x{:x}", slot),
                    "blockHash": block_hash_str,
                    "transactionHash": format!("0x{}", hex::encode(entry.tx_hash)),
                    "transactionIndex": format!("0x{:x}", entry.tx_index),
                    "logIndex": format!("0x{:x}", log_index),
                    "removed": false,
                }));
                log_index += 1;

                if logs.len() >= MAX_LOG_RESULTS {
                    break;
                }
            }
        }

        if logs.len() >= MAX_LOG_RESULTS {
            break;
        }

        // ── Phase 2: Native Lichen contract events (backward compat) ──
        let events = state
            .state
            .get_events_by_slot(slot, 10_000)
            .unwrap_or_default();

        for event in &events {
            // Address filter: resolve native program to EVM address
            if !filter_addresses.is_empty() {
                let evm_addr =
                    if let Ok(Some(addr)) = state.state.lookup_native_to_evm(&event.program) {
                        addr
                    } else {
                        let mut addr = [0u8; 20];
                        addr.copy_from_slice(&event.program.0[12..32]);
                        addr
                    };
                if !filter_addresses.contains(&evm_addr) {
                    continue;
                }
            }

            // Build topics from event name + data keys
            let mut topics = Vec::new();
            // AUDIT-FIX A11-02: topic[0] = keccak256(event_name)
            let event_hash = {
                use sha3::{Digest, Keccak256};
                let mut hasher = Keccak256::new();
                hasher.update(event.name.as_bytes());
                let result = hasher.finalize();
                let mut h = [0u8; 32];
                h.copy_from_slice(&result);
                h
            };
            topics.push(event_hash);

            // Additional topics from indexed data fields
            for value in event.data.values() {
                if topics.len() >= 4 {
                    break;
                }
                let v_bytes = value.as_bytes();
                let mut padded = [0u8; 32];
                let start = 32usize.saturating_sub(v_bytes.len());
                let copy_len = v_bytes.len().min(32);
                padded[start..start + copy_len].copy_from_slice(&v_bytes[..copy_len]);
                topics.push(padded);
            }

            // Apply EIP-1474 topics filter
            if !lichen_core::topics_match(&topics, &filter_topics) {
                continue;
            }

            // AUDIT-FIX P10-RPC-03: ABI-encode data values
            let data_hex = {
                let mut data_bytes = Vec::new();
                for v in event.data.values() {
                    let v_bytes = v.as_bytes();
                    if v_bytes.len() < 32 {
                        let padding = 32 - v_bytes.len();
                        data_bytes.extend(std::iter::repeat_n(0u8, padding));
                    }
                    data_bytes.extend_from_slice(v_bytes);
                }
                format!("0x{}", hex::encode(&data_bytes))
            };

            let contract_addr =
                if let Ok(Some(evm_addr)) = state.state.lookup_native_to_evm(&event.program) {
                    format!("0x{}", hex::encode(evm_addr))
                } else {
                    format!("0x{}", hex::encode(&event.program.0[12..32]))
                };

            let topics_json: Vec<serde_json::Value> = topics
                .iter()
                .map(|t| serde_json::Value::String(format!("0x{}", hex::encode(t))))
                .collect();

            // AUDIT-FIX P10-RPC-02: Derive deterministic transactionHash
            let tx_hash = {
                use sha3::{Digest, Keccak256};
                let block_hash_hex = block_hash_str.strip_prefix("0x").unwrap_or(&block_hash_str);
                let bh_bytes = hex::decode(block_hash_hex).unwrap_or_default();
                let mut hasher = Keccak256::new();
                hasher.update(&bh_bytes);
                hasher.update(log_index.to_be_bytes());
                format!("0x{}", hex::encode(hasher.finalize()))
            };

            logs.push(serde_json::json!({
                "address": contract_addr,
                "topics": topics_json,
                "data": data_hex,
                "blockNumber": format!("0x{:x}", slot),
                "blockHash": block_hash_str,
                "transactionHash": tx_hash,
                "transactionIndex": "0x0",
                "logIndex": format!("0x{:x}", log_index),
                "removed": false,
            }));
            log_index += 1;

            if logs.len() >= MAX_LOG_RESULTS {
                break;
            }
        }
        if logs.len() >= MAX_LOG_RESULTS {
            break;
        }
    }

    Ok(serde_json::json!(logs))
}

/// Parse a hex topic hash string to [u8; 32]
fn parse_topic_hash(s: &str) -> Option<[u8; 32]> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(s).ok()?;
    if bytes.len() != 32 {
        return None;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Some(arr)
}

/// eth_getStorageAt — read a storage slot from an EVM contract.
async fn handle_eth_get_storage_at(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [address, slot, block?]".to_string(),
    })?;

    let addr_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing address".to_string(),
        })?;
    let slot_str = arr
        .get(1)
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing storage slot".to_string(),
        })?;

    let evm_addr = lichen_core::StateStore::parse_evm_address(addr_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid address: {}", e),
    })?;

    // Parse slot as a hex-encoded 32-byte value
    let slot_hex = slot_str.strip_prefix("0x").unwrap_or(slot_str);
    let slot_bytes_vec = hex::decode(slot_hex).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid storage slot hex".to_string(),
    })?;
    let mut slot_arr = [0u8; 32];
    // Right-align the slot bytes (big-endian)
    let start = 32usize.saturating_sub(slot_bytes_vec.len());
    slot_arr[start..].copy_from_slice(&slot_bytes_vec[..slot_bytes_vec.len().min(32)]);

    let value = state
        .state
        .get_evm_storage(&evm_addr, &slot_arr)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!(format!("0x{:064x}", value)))
}

/// Handle getStakingPosition: Get user's MossStake position
async fn handle_get_staking_position(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params: Vec<serde_json::Value> =
        params
            .and_then(|v| v.as_array().cloned())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: expected [user_pubkey]".to_string(),
            })?;

    if params.is_empty() {
        return Err(RpcError {
            code: -32602,
            message: "Invalid params: expected [user_pubkey]".to_string(),
        });
    }

    let user_pubkey = params[0].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid user_pubkey".to_string(),
    })?;

    let user = Pubkey::from_base58(user_pubkey).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid pubkey format".to_string(),
    })?;

    let pool = state.state.get_mossstake_pool().map_err(|e| RpcError {
        code: -32603,
        message: format!("Failed to get MossStake pool: {}", e),
    })?;

    if let Some(position) = pool.positions.get(&user) {
        let current_value = pool.st_licn_token.st_licn_to_licn(position.st_licn_amount);
        let tier = position.lock_tier as u8;
        let tier_name = position.lock_tier.display_name();
        let multiplier = position.lock_tier.reward_multiplier_bp() as f64 / 10_000.0;
        Ok(serde_json::json!({
            "owner": user_pubkey,
            "st_licn_amount": position.st_licn_amount,
            "licn_deposited": position.licn_deposited,
            "current_value_licn": current_value,
            "rewards_earned": position.rewards_earned,
            "deposited_at": position.deposited_at,
            "lock_tier": tier,
            "lock_tier_name": tier_name,
            "lock_until": position.lock_until,
            "reward_multiplier": multiplier
        }))
    } else {
        Ok(serde_json::json!({
            "owner": user_pubkey,
            "st_licn_amount": 0,
            "licn_deposited": 0,
            "current_value_licn": 0,
            "rewards_earned": 0,
            "deposited_at": 0,
            "lock_tier": 0,
            "lock_tier_name": "Flexible",
            "lock_until": 0,
            "reward_multiplier": 1.0
        }))
    }
}

/// Handle getMossStakePoolInfo: Get global MossStake pool info
async fn handle_get_mossstake_pool_info(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    use lichen_core::consensus::SLOTS_PER_YEAR;

    let pool = state.state.get_mossstake_pool().map_err(|e| RpcError {
        code: -32603,
        message: format!("Failed to get MossStake pool: {}", e),
    })?;

    // Derive active validators count and APY from the consensus StakePool
    let (active_validators, apy_percent) = if let Some(ref sp_arc) = state.stake_pool {
        let sp = sp_arc.read().await;
        let stats = sp.get_stats();
        let slots_per_day = SLOTS_PER_YEAR / 365;
        // Use inflation-curve block reward based on current slot
        let current_slot = state.state.get_last_slot().unwrap_or(0);
        let total_supply = GENESIS_SUPPLY_SPORES
            .saturating_add(state.state.get_total_minted().unwrap_or(0))
            .saturating_sub(state.state.get_total_burned().unwrap_or(0));
        let current_reward = compute_block_reward(current_slot, total_supply);
        let apy_bp = pool.calculate_apy_bp(slots_per_day, current_reward);
        (stats.active_validators, apy_bp as f64 / 100.0)
    } else {
        (0, 0.0)
    };

    Ok(serde_json::json!({
        "total_supply_st_licn": pool.st_licn_token.total_supply,
        "total_licn_staked": pool.st_licn_token.total_licn_staked,
        "exchange_rate": pool.st_licn_token.exchange_rate_display(),
        "total_validators": active_validators,
        "average_apy_percent": apy_percent,
        "total_stakers": pool.positions.len(),
        "tiers": [
            { "id": 0, "name": "Flexible", "lock_days": 0, "multiplier": 1.0, "apy_percent": apy_percent },
            { "id": 1, "name": "30-Day Lock", "lock_days": 30, "multiplier": 1.6, "apy_percent": apy_percent * 1.6 },
            { "id": 2, "name": "180-Day Lock", "lock_days": 180, "multiplier": 2.4, "apy_percent": apy_percent * 2.4 },
            { "id": 3, "name": "365-Day Lock", "lock_days": 365, "multiplier": 3.6, "apy_percent": apy_percent * 3.6 },
        ],
        "cooldown_days": 7
    }))
}

/// Handle getUnstakingQueue: Get user's pending unstake requests
async fn handle_get_unstaking_queue(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params: Vec<serde_json::Value> =
        params
            .and_then(|v| v.as_array().cloned())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: expected [user_pubkey]".to_string(),
            })?;

    if params.is_empty() {
        return Err(RpcError {
            code: -32602,
            message: "Invalid params: expected [user_pubkey]".to_string(),
        });
    }

    let user_pubkey = params[0].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid user_pubkey".to_string(),
    })?;

    let user = Pubkey::from_base58(user_pubkey).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid pubkey format".to_string(),
    })?;

    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let pool = state.state.get_mossstake_pool().map_err(|e| RpcError {
        code: -32603,
        message: format!("Failed to get MossStake pool: {}", e),
    })?;

    let requests = pool.get_unstake_requests(&user);
    let mut total_claimable = 0u64;
    let pending_requests: Vec<serde_json::Value> = requests
        .iter()
        .map(|request| {
            if request.claimable_at <= current_slot {
                total_claimable += request.licn_to_receive;
            }
            serde_json::json!({
                "st_licn_amount": request.st_licn_amount,
                "licn_to_receive": request.licn_to_receive,
                "requested_at": request.requested_at,
                "claimable_at": request.claimable_at,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "owner": user_pubkey,
        "pending_requests": pending_requests,
        "total_claimable": total_claimable
    }))
}

/// Handle getRewardAdjustmentInfo: Get current reward adjustment and staking economics
async fn handle_get_reward_adjustment_info(
    state: &RpcState,
) -> Result<serde_json::Value, RpcError> {
    use lichen_core::consensus::{
        compute_block_reward, compute_epoch_mint, inflation_rate_bps, BOOTSTRAP_GRANT_AMOUNT,
        GENESIS_SUPPLY_SPORES, INFLATION_DECAY_RATE_BPS, INITIAL_INFLATION_RATE_BPS,
        SLOTS_PER_EPOCH, SLOTS_PER_YEAR, TERMINAL_INFLATION_RATE_BPS,
    };

    let stake_pool_arc = state.stake_pool.as_ref().ok_or_else(|| RpcError {
        code: -32000,
        message: "Stake pool not available".to_string(),
    })?;
    let stake_pool = stake_pool_arc.read().await;
    let stats = stake_pool.get_stats();
    let active_count = stats.active_validators;
    let total_staked = stats.total_staked;

    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let total_minted = state.state.get_total_minted().unwrap_or(0);
    let total_burned = state.state.get_total_burned().unwrap_or(0);
    let fee_config = state
        .state
        .get_fee_config()
        .unwrap_or_else(|_| lichen_core::FeeConfig::default_from_constants());
    let total_supply = GENESIS_SUPPLY_SPORES
        .saturating_add(total_minted)
        .saturating_sub(total_burned);

    let current_inflation_bps = inflation_rate_bps(current_slot);
    let block_reward = compute_block_reward(current_slot, total_supply);
    let epoch_mint = compute_epoch_mint(current_slot, total_supply);
    let epochs_per_year = SLOTS_PER_YEAR / SLOTS_PER_EPOCH;

    // Price-adjusted reward (informational — epoch rewards use inflation curve directly)
    let reward_config = lichen_core::consensus::RewardConfig::new();
    let licn_price = lichen_core::consensus::licn_price_from_state(&state.state);
    let adjusted_reward = reward_config.get_adjusted_reward(current_slot, total_supply, licn_price);

    // Estimate APY: all inflation goes to stakers proportionally at epoch boundaries
    let annual_inflation = epoch_mint as f64 * epochs_per_year as f64;
    let apy = if total_staked > 0 {
        (annual_inflation / total_staked as f64) * 100.0
    } else {
        0.0
    };

    let inflation_year = current_slot / SLOTS_PER_YEAR;

    // Projected supply: include theoretical inflation accrued since last epoch
    let current_epoch = current_slot / SLOTS_PER_EPOCH;
    let epoch_start = current_epoch * SLOTS_PER_EPOCH;
    let slots_into_epoch = current_slot.saturating_sub(epoch_start);
    let projected_unminted = block_reward as u128 * slots_into_epoch as u128;
    let projected_supply = total_supply.saturating_add(projected_unminted as u64);

    // Load wallet pubkeys and balances for full transparency
    let wallet_info = |role: &str| -> serde_json::Value {
        let (pubkey_str, balance) = match state.state.get_wallet_pubkey(role) {
            Ok(Some(pk)) => {
                let bal = state
                    .state
                    .get_account(&pk)
                    .ok()
                    .flatten()
                    .map(|a| a.spores)
                    .unwrap_or(0);
                (pk.to_base58(), bal)
            }
            _ => ("unknown".to_string(), 0),
        };
        serde_json::json!({
            "pubkey": pubkey_str,
            "balance_spores": balance,
            "balance_licn": balance as f64 / 1_000_000_000.0,
        })
    };

    Ok(serde_json::json!({
        "supplyModel": "inflationary_with_burn",
        "genesisSupply": GENESIS_SUPPLY_SPORES,
        "totalMinted": total_minted,
        "totalBurned": total_burned,
        "totalSupply": total_supply,
        "projectedSupply": projected_supply,
        "inflationRateBps": current_inflation_bps,
        "inflationRatePercent": format!("{:.4}", current_inflation_bps as f64 / 100.0),
        "inflationYear": inflation_year,
        "initialInflationRateBps": INITIAL_INFLATION_RATE_BPS,
        "inflationDecayRateBps": INFLATION_DECAY_RATE_BPS,
        "terminalInflationRateBps": TERMINAL_INFLATION_RATE_BPS,
        "blockReward": block_reward,
        "adjustedBlockReward": adjusted_reward,
        "epochMint": epoch_mint,
        "slotsPerEpoch": SLOTS_PER_EPOCH,
        "epochsPerYear": epochs_per_year,
        "priceAdjustmentMultiplier": if licn_price > 0.0 {
            format!("{:.4}", (0.10 / licn_price).clamp(0.1, 10.0))
        } else {
            "1.0000".to_string()
        },
        "moldPrice": licn_price,
        "slotsPerYear": SLOTS_PER_YEAR,
        "currentSlot": current_slot,
        "minValidatorStake": state.min_validator_stake,
        "bootstrapGrantAmount": BOOTSTRAP_GRANT_AMOUNT,
        "totalStaked": total_staked,
        "totalSlashed": stats.total_slashed,
        "activeValidators": active_count,
        "unclaimedRewards": stats.total_unclaimed_rewards,
        "estimatedApy": format!("{:.2}", apy),
        "feeSplit": {
            "burn_pct": fee_config.fee_burn_percent,
            "producer_pct": fee_config.fee_producer_percent,
            "voters_pct": fee_config.fee_voters_percent,
            "treasury_pct": fee_config.fee_treasury_percent,
            "community_pct": fee_config.fee_community_percent,
        },
        "genesisDistribution": {
            "validator_rewards_pct": 10,
            "community_treasury_pct": 25,
            "builder_grants_pct": 35,
            "founding_symbionts_pct": 10,
            "ecosystem_partnerships_pct": 10,
            "reserve_pool_pct": 10,
        },
        "wallets": {
            "validator_rewards": wallet_info("validator_rewards"),
            "community_treasury": wallet_info("community_treasury"),
            "builder_grants": wallet_info("builder_grants"),
            "founding_symbionts": wallet_info("founding_symbionts"),
            "ecosystem_partnerships": wallet_info("ecosystem_partnerships"),
            "reserve_pool": wallet_info("reserve_pool"),
        },
        "note": "Epoch-based staker rewards: inflation minted at epoch boundaries and distributed to all stakers proportionally by stake weight. Block producers earn transaction fees per-block. 40% fee burn provides counter-pressure."
    }))
}

// ============================================================================
// TOKEN ENDPOINTS
// ============================================================================

/// Get token balance for a holder: params = [token_program, holder]
async fn handle_get_token_balance(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Expected array params".to_string(),
    })?;

    let token_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing token_program".to_string(),
        })?;
    let holder_str = arr
        .get(1)
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing holder".to_string(),
        })?;

    let token_program = Pubkey::from_base58(token_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid token_program: {}", e),
    })?;
    let holder = Pubkey::from_base58(holder_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid holder: {}", e),
    })?;

    let balance = state
        .state
        .get_token_balance(&token_program, &holder)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    // F19.2a: Include decimals and ui_amount from symbol registry
    let registry = state
        .state
        .get_symbol_registry_by_program(&token_program)
        .ok()
        .flatten();

    let decimals = registry
        .as_ref()
        .and_then(|r| r.metadata.as_ref())
        .and_then(|m| m.get("decimals"))
        .and_then(|d| d.as_u64())
        .unwrap_or(9);

    let symbol = registry
        .as_ref()
        .map(|r| r.symbol.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    let ui_amount = balance as f64 / 10_f64.powi(decimals as i32);

    Ok(serde_json::json!({
        "token_program": token_str,
        "holder": holder_str,
        "balance": balance,
        "decimals": decimals,
        "ui_amount": ui_amount,
        "symbol": symbol,
    }))
}

/// Get token holders: params = [token_program, limit?]
async fn handle_get_token_holders(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Expected array params".to_string(),
    })?;

    let token_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing token_program".to_string(),
        })?;
    let limit = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(100).min(1000) as usize; // AUDIT-FIX 2.16

    let after_holder = arr.get(2).and_then(|v| v.as_str());
    let after_holder_pubkey = if let Some(ah_str) = after_holder {
        Some(Pubkey::from_base58(ah_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid after_holder: {}", e),
        })?)
    } else {
        None
    };

    let token_program = Pubkey::from_base58(token_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid token_program: {}", e),
    })?;

    let holders = state
        .state
        .get_token_holders(&token_program, limit, after_holder_pubkey.as_ref())
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let holder_list: Vec<serde_json::Value> = holders
        .iter()
        .map(|(pk, bal)| {
            serde_json::json!({
                "holder": pk.to_base58(),
                "balance": bal,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "token_program": token_str,
        "holders": holder_list,
        "count": holder_list.len(),
    }))
}

/// Get token transfers: params = [token_program, limit?, before_slot?]
async fn handle_get_token_transfers(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Expected array params".to_string(),
    })?;

    let token_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing token_program".to_string(),
        })?;
    let limit = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(100).min(1000) as usize; // AUDIT-FIX 2.16

    let before_slot = arr.get(2).and_then(|v| v.as_u64());

    let token_program = Pubkey::from_base58(token_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid token_program: {}", e),
    })?;

    let transfers = state
        .state
        .get_token_transfers(&token_program, limit, before_slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let transfer_list: Vec<serde_json::Value> = transfers
        .iter()
        .map(|t| {
            serde_json::json!({
                "from": t.from,
                "to": t.to,
                "amount": t.amount,
                "slot": t.slot,
                "tx_hash": t.tx_hash,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "token_program": token_str,
        "transfers": transfer_list,
        "count": transfer_list.len(),
    }))
}

/// Get contract events: params = [program_id, limit?, before_slot?]
async fn handle_get_contract_events(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Expected array params".to_string(),
    })?;

    let program_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing program_id".to_string(),
        })?;
    let limit = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(100).min(1000) as usize; // AUDIT-FIX 2.16

    let before_slot = arr.get(2).and_then(|v| v.as_u64());

    let program = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program_id: {}", e),
    })?;

    let events = state
        .state
        .get_events_by_program(&program, limit, before_slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let event_list: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "program": e.program.to_base58(),
                "name": e.name,
                "data": e.data,
                "slot": e.slot,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "program": program_str,
        "events": event_list,
        "count": event_list.len(),
    }))
}

async fn handle_get_governance_events(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let (limit, before_slot) = match params {
        Some(params) => {
            let arr = params.as_array().ok_or_else(|| RpcError {
                code: -32602,
                message: "Expected array params".to_string(),
            })?;
            (
                arr.first()
                    .and_then(|v| v.as_u64())
                    .unwrap_or(100)
                    .min(1000) as usize,
                arr.get(1).and_then(|v| v.as_u64()),
            )
        }
        None => (100usize, None),
    };

    let fetch_limit = limit.saturating_mul(4).clamp(256, 1000);
    let events = state
        .state
        .get_events_by_program(&SYSTEM_PROGRAM_ID, fetch_limit, before_slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let event_list: Vec<serde_json::Value> = events
        .iter()
        .filter_map(parse_governance_event)
        .take(limit)
        .map(|event| {
            serde_json::json!({
                "proposal_id": event.proposal_id,
                "kind": event.event_kind,
                "action": event.action,
                "authority": event.authority.to_base58(),
                "proposer": event.proposer.to_base58(),
                "actor": event.actor.to_base58(),
                "approvals": event.approvals,
                "threshold": event.threshold,
                "execute_after_epoch": event.execute_after_epoch,
                "executed": event.executed,
                "cancelled": event.cancelled,
                "metadata": event.metadata,
                "target_contract": event.target_contract.map(|value| value.to_base58()),
                "target_function": event.target_function,
                "call_args_len": event.call_args_len,
                "call_value_spores": event.call_value_spores,
                "slot": event.slot,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "program": SYSTEM_PROGRAM_ID.to_base58(),
        "events": event_list,
        "count": event_list.len(),
    }))
}

async fn handle_get_incident_status(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let status = load_incident_status_record(state);
    serde_json::to_value(status).map_err(|error| RpcError {
        code: -32000,
        message: format!("Failed to serialize incident status: {}", error),
    })
}

async fn handle_get_signed_metadata_manifest(
    state: &RpcState,
) -> Result<serde_json::Value, RpcError> {
    load_signed_metadata_manifest_value(state).await
}

async fn handle_get_service_fleet_status(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    {
        let guard = state.service_fleet_status_cache.read().await;
        if guard.0.elapsed().as_millis() < SERVICE_FLEET_CACHE_TTL_MS {
            if let Some(cached) = guard.1.clone() {
                return serde_json::to_value(cached).map_err(|error| RpcError {
                    code: -32000,
                    message: format!("Failed to serialize cached service fleet status: {}", error),
                });
            }
        }
    }

    let status = refresh_service_fleet_status(state).await;

    {
        let mut guard = state.service_fleet_status_cache.write().await;
        *guard = (Instant::now(), Some(status.clone()));
    }

    serde_json::to_value(status).map_err(|error| RpcError {
        code: -32000,
        message: format!("Failed to serialize service fleet status: {}", error),
    })
}

/// Testnet-only airdrop: creates a signed consensus transaction (type 19)
/// that transfers LICN from treasury to a given address.
/// Usage: requestAirdrop [address, amount_in_licn]
/// The transaction goes through the mempool and consensus, ensuring all
/// validators apply the same state change.
async fn handle_request_airdrop(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    // Only allow on testnet / devnet (not mainnet)
    if state.network_id.contains("mainnet")
        || (!state.network_id.contains("testnet")
            && !state.network_id.contains("devnet")
            && !state.network_id.contains("local"))
    {
        return Err(RpcError {
            code: -32003,
            message: "Airdrop only available on testnet/devnet".to_string(),
        });
    }

    // Require treasury keypair to be configured
    let treasury_kp = state.treasury_keypair.as_ref().ok_or(RpcError {
        code: -32000,
        message: "Treasury keypair not configured — cannot sign airdrop transactions".to_string(),
    })?;

    let params = params.ok_or(RpcError {
        code: -32602,
        message: "Expected params: [address, amount_in_licn]".to_string(),
    })?;

    let arr = params.as_array().ok_or(RpcError {
        code: -32602,
        message: "Expected array params: [address, amount_in_licn]".to_string(),
    })?;

    if arr.len() < 2 {
        return Err(RpcError {
            code: -32602,
            message: "Expected params: [address, amount_in_licn]".to_string(),
        });
    }

    let address_str = arr[0].as_str().ok_or(RpcError {
        code: -32602,
        message: "address must be a string".to_string(),
    })?;

    let amount_licn = arr[1].as_u64().ok_or(RpcError {
        code: -32602,
        message: "amount must be an integer (LICN)".to_string(),
    })?;

    if amount_licn == 0 || amount_licn > 10 {
        return Err(RpcError {
            code: -32602,
            message: "Amount must be between 1 and 10 LICN".to_string(),
        });
    }

    let recipient = Pubkey::from_base58(address_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid address: {}", e),
    })?;

    // AUDIT-FIX RPC-4: Per-address airdrop rate limiting (1 per 60 seconds + 150 LICN/day)
    {
        let now = Instant::now();
        let mut cooldowns = state.airdrop_cooldowns.write().await;
        if let Some(remaining) = cooldowns.check_and_record(address_str, now) {
            return Err(RpcError {
                code: -32005,
                message: format!(
                    "Airdrop rate limit: 1 per {} seconds per address. Try again in {} seconds.",
                    AIRDROP_COOLDOWN_SECS, remaining
                ),
            });
        }
        if let Err(msg) = cooldowns.check_daily_limit(address_str, amount_licn, now) {
            return Err(RpcError {
                code: -32005,
                message: msg,
            });
        }
        cooldowns.record_daily(address_str, amount_licn, now);
    }

    let amount_spores = amount_licn * 1_000_000_000;

    // Verify treasury has sufficient balance (pre-check, actual debit happens in consensus)
    let treasury_pubkey = treasury_kp.pubkey();
    let treasury_account = state
        .state
        .get_account(&treasury_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to read treasury: {}", e),
        })?
        .ok_or(RpcError {
            code: -32000,
            message: "Treasury account not found".to_string(),
        })?;

    if treasury_account.spendable < amount_spores {
        return Err(RpcError {
            code: -32000,
            message: "Insufficient treasury balance for airdrop".to_string(),
        });
    }

    // Get recent blockhash for the transaction
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Failed to get latest slot: {}", e),
    })?;
    let block = state
        .state
        .get_block_by_slot(slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to get latest block: {}", e),
        })?
        .ok_or(RpcError {
            code: -32000,
            message: "Latest block not found".to_string(),
        })?;
    let recent_blockhash = block.hash();

    // Build instruction type 19 (FaucetAirdrop): [19 | amount_spores(8 LE)]
    let mut ix_data = vec![19u8];
    ix_data.extend_from_slice(&amount_spores.to_le_bytes());

    let ix = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![treasury_pubkey, recipient],
        data: ix_data,
    };

    // Build and sign the transaction
    let msg = Message::new(vec![ix], recent_blockhash);
    let mut tx = Transaction::new(msg);
    let sig = treasury_kp.sign(&tx.message.serialize());
    tx.signatures.push(sig);

    let tx_hash = tx.signature().to_hex();

    // Submit through mempool for consensus processing
    submit_transaction(state, tx)?;

    info!(
        "💧 Airdrop tx submitted: {} LICN from treasury to {} (tx: {})",
        amount_licn, address_str, tx_hash
    );

    Ok(serde_json::json!({
        "success": true,
        "signature": tx_hash,
        "amount": amount_licn,
        "recipient": address_str,
        "message": format!("{} LICN airdrop transaction submitted", amount_licn),
    }))
}

// ═══════════════════════════════════════════════════════════════════════════════
// PREDICTION MARKET JSON-RPC HANDLERS
// ═══════════════════════════════════════════════════════════════════════════════

const PREDICT_SYMBOL: &str = "PREDICT";
const PM_PRICE_SCALE: f64 = 1_000_000_000.0;

fn pm_u64(data: &[u8], off: usize) -> u64 {
    if data.len() < off + 8 {
        return 0;
    }
    u64::from_le_bytes(data[off..off + 8].try_into().unwrap_or([0; 8]))
}

/// getPredictionMarketStats — Platform stats
async fn handle_get_prediction_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "PREDICT")?;

    Ok(serde_json::json!({
        "total_markets": cf_stats_u64(state, "PREDICT", b"pm_market_count"),
        "open_markets": cf_stats_u64(state, "PREDICT", b"pm_open_markets"),
        "total_volume": cf_stats_u64(state, "PREDICT", b"pm_total_volume") as f64 / PM_PRICE_SCALE,
        "total_collateral": cf_stats_u64(state, "PREDICT", b"pm_total_collateral") as f64 / PM_PRICE_SCALE,
        "fees_collected": cf_stats_u64(state, "PREDICT", b"pm_fees_collected") as f64 / PM_PRICE_SCALE,
        "total_traders": cf_stats_u64(state, "PREDICT", b"pm_total_traders"),
        "paused": cf_stats_bool(state, "PREDICT", b"pm_paused"),
    }))
}

/// getPredictionMarkets — List markets with optional filter
async fn handle_get_prediction_markets(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let total = state
        .state
        .get_program_storage_u64(PREDICT_SYMBOL, b"pm_market_count");

    // Parse optional filter params
    let (cat_filter, status_filter, limit, offset) = match &params {
        Some(serde_json::Value::Object(obj)) => {
            let cat = obj
                .get("category")
                .and_then(|v| v.as_str())
                .map(String::from);
            let st = obj.get("status").and_then(|v| v.as_str()).map(String::from);
            let lim = obj.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            let off = obj.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            (cat, st, lim.min(200), off)
        }
        _ => (None, None, 50, 0),
    };

    let cat_map = |c: u8| -> &'static str {
        match c {
            0 => "politics",
            1 => "sports",
            2 => "crypto",
            3 => "science",
            4 => "entertainment",
            5 => "economics",
            6 => "tech",
            _ => "custom",
        }
    };
    let st_map = |s: u8| -> &'static str {
        match s {
            0 => "pending",
            1 => "active",
            2 => "closed",
            3 => "resolving",
            4 => "resolved",
            5 => "disputed",
            6 => "voided",
            _ => "unknown",
        }
    };

    // PERF-NOTE (P10-VAL-05): Linear scan over all markets. Acceptable at
    // current scale. For >100K markets, consider contract-side filtered
    // counters or an off-chain index to avoid O(n) per query.
    let mut markets = Vec::new();
    for id in 1..=total {
        let key = format!("pm_m_{}", id);
        let data = match state
            .state
            .get_program_storage(PREDICT_SYMBOL, key.as_bytes())
        {
            Some(d) if d.len() >= 192 => d,
            _ => continue,
        };
        let cat = cat_map(data[67]);
        let status = st_map(data[64]);

        if let Some(ref cf) = cat_filter {
            if cat != cf.as_str() {
                continue;
            }
        }
        if let Some(ref sf) = status_filter {
            if status != sf.as_str() {
                continue;
            }
        }

        let q_key = format!("pm_q_{}", id);
        let question = state
            .state
            .get_program_storage(PREDICT_SYMBOL, q_key.as_bytes())
            .and_then(|d| String::from_utf8(d).ok())
            .unwrap_or_default();

        markets.push(serde_json::json!({
            "id": pm_u64(&data, 0),
            "question": question,
            "category": cat,
            "status": status,
            "outcome_count": data[65],
            "total_collateral": pm_u64(&data, 68) as f64 / PM_PRICE_SCALE,
            "total_volume": pm_u64(&data, 76) as f64 / PM_PRICE_SCALE,
            "created_slot": pm_u64(&data, 40),
            "close_slot": pm_u64(&data, 48),
        }));
    }

    let result_total = markets.len();
    let page: Vec<_> = markets.into_iter().skip(offset).take(limit).collect();

    Ok(serde_json::json!({
        "markets": page,
        "total": result_total,
        "offset": offset,
        "limit": limit,
    }))
}

/// getPredictionMarket — Single market detail
async fn handle_get_prediction_market(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let market_id = match &params {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => {
            arr[0].as_u64().ok_or(RpcError {
                code: -32602,
                message: "market_id must be u64".into(),
            })?
        }
        Some(serde_json::Value::Object(obj)) => obj
            .get("market_id")
            .or(obj.get("id"))
            .and_then(|v| v.as_u64())
            .ok_or(RpcError {
                code: -32602,
                message: "market_id required".into(),
            })?,
        _ => {
            return Err(RpcError {
                code: -32602,
                message: "Expected params: [market_id] or {market_id}".into(),
            })
        }
    };

    let key = format!("pm_m_{}", market_id);
    let data = state
        .state
        .get_program_storage(PREDICT_SYMBOL, key.as_bytes())
        .ok_or(RpcError {
            code: -32001,
            message: format!("Market {} not found", market_id),
        })?;

    if data.len() < 192 {
        return Err(RpcError {
            code: -32002,
            message: "Invalid market record".into(),
        });
    }

    let cat_map = |c: u8| -> &'static str {
        match c {
            0 => "politics",
            1 => "sports",
            2 => "crypto",
            3 => "science",
            4 => "entertainment",
            5 => "economics",
            6 => "tech",
            _ => "custom",
        }
    };
    let st_map = |s: u8| -> &'static str {
        match s {
            0 => "pending",
            1 => "active",
            2 => "closed",
            3 => "resolving",
            4 => "resolved",
            5 => "disputed",
            6 => "voided",
            _ => "unknown",
        }
    };

    let q_key = format!("pm_q_{}", market_id);
    let question = state
        .state
        .get_program_storage(PREDICT_SYMBOL, q_key.as_bytes())
        .and_then(|d| String::from_utf8(d).ok())
        .unwrap_or_default();

    let outcome_count = data[65];
    let mut outcomes = Vec::new();
    for oi in 0..outcome_count {
        let o_key = format!("pm_o_{}_{}", market_id, oi);
        let on_key = format!("pm_on_{}_{}", market_id, oi);

        let name = state
            .state
            .get_program_storage(PREDICT_SYMBOL, on_key.as_bytes())
            .and_then(|d| String::from_utf8(d).ok())
            .unwrap_or_else(|| if oi == 0 { "Yes".into() } else { "No".into() });

        let (pool_y, pool_n) = state
            .state
            .get_program_storage(PREDICT_SYMBOL, o_key.as_bytes())
            .map(|d| {
                if d.len() >= 16 {
                    (pm_u64(&d, 0), pm_u64(&d, 8))
                } else {
                    (0, 0)
                }
            })
            .unwrap_or((0, 0));

        let total = pool_y + pool_n;
        let price = if total > 0 {
            pool_n as f64 / total as f64
        } else {
            0.5
        };

        outcomes.push(serde_json::json!({
            "index": oi,
            "name": name,
            "pool_yes": pool_y as f64 / PM_PRICE_SCALE,
            "pool_no": pool_n as f64 / PM_PRICE_SCALE,
            "price": price,
        }));
    }

    let winning = data[66];

    Ok(serde_json::json!({
        "id": pm_u64(&data, 0),
        "creator": hex::encode(&data[8..40]),
        "question": question,
        "category": cat_map(data[67]),
        "status": st_map(data[64]),
        "outcome_count": outcome_count,
        "winning_outcome": if winning == 0xFF { serde_json::Value::Null } else { serde_json::json!(winning) },
        "total_collateral": pm_u64(&data, 68) as f64 / PM_PRICE_SCALE,
        "total_volume": pm_u64(&data, 76) as f64 / PM_PRICE_SCALE,
        "fees_collected": pm_u64(&data, 164) as f64 / PM_PRICE_SCALE,
        "created_slot": pm_u64(&data, 40),
        "close_slot": pm_u64(&data, 48),
        "resolve_slot": pm_u64(&data, 56),
        "outcomes": outcomes,
    }))
}

/// getPredictionPositions [address] — User positions
async fn handle_get_prediction_positions(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let address = match &params {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => arr[0]
            .as_str()
            .ok_or(RpcError {
                code: -32602,
                message: "address must be string".into(),
            })?
            .to_string(),
        _ => {
            return Err(RpcError {
                code: -32602,
                message: "Expected params: [address]".into(),
            })
        }
    };

    let count_key = format!("pm_userc_{}", address);
    let count = state
        .state
        .get_program_storage_u64(PREDICT_SYMBOL, count_key.as_bytes());

    let mut positions = Vec::new();
    for idx in 0..count {
        let um_key = format!("pm_user_{}_{}", address, idx);
        let market_id = match state
            .state
            .get_program_storage(PREDICT_SYMBOL, um_key.as_bytes())
        {
            Some(d) if d.len() >= 8 => pm_u64(&d, 0),
            _ => continue,
        };

        let mkt_key = format!("pm_m_{}", market_id);
        let mkt_data = match state
            .state
            .get_program_storage(PREDICT_SYMBOL, mkt_key.as_bytes())
        {
            Some(d) if d.len() >= 192 => d,
            _ => continue,
        };
        let outcome_count = mkt_data[65];

        for oi in 0..outcome_count {
            let pos_key = format!("pm_p_{}_{}_{}", market_id, address, oi);
            if let Some(pd) = state
                .state
                .get_program_storage(PREDICT_SYMBOL, pos_key.as_bytes())
            {
                if pd.len() >= 16 {
                    let shares = pm_u64(&pd, 0);
                    let cost = pm_u64(&pd, 8);
                    if shares > 0 {
                        positions.push(serde_json::json!({
                            "market_id": market_id,
                            "outcome": oi,
                            "shares": shares as f64 / PM_PRICE_SCALE,
                            "cost_basis": cost as f64 / PM_PRICE_SCALE,
                        }));
                    }
                }
            }
        }
    }

    Ok(serde_json::json!(positions))
}

/// getPredictionTraderStats [address] — Per-trader analytics
async fn handle_get_prediction_trader_stats(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let address = match &params {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => arr[0]
            .as_str()
            .ok_or(RpcError {
                code: -32602,
                message: "address must be string".into(),
            })?
            .to_string(),
        _ => {
            return Err(RpcError {
                code: -32602,
                message: "Expected params: [address]".into(),
            })
        }
    };

    let key = format!("pm_ts_{}", address);
    match state
        .state
        .get_program_storage(PREDICT_SYMBOL, key.as_bytes())
    {
        Some(d) if d.len() >= 24 => Ok(serde_json::json!({
            "address": address,
            "total_volume": pm_u64(&d, 0) as f64 / PM_PRICE_SCALE,
            "trade_count": pm_u64(&d, 8),
            "last_trade_slot": pm_u64(&d, 16),
        })),
        _ => Ok(serde_json::json!({
            "address": address,
            "total_volume": 0.0,
            "trade_count": 0,
            "last_trade_slot": 0,
        })),
    }
}

/// getPredictionLeaderboard — Top traders by volume
async fn handle_get_prediction_leaderboard(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let limit = match &params {
        Some(serde_json::Value::Object(obj)) => {
            obj.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize
        }
        _ => 20,
    };
    let limit = limit.min(50);

    let total_traders = state
        .state
        .get_program_storage_u64(PREDICT_SYMBOL, b"pm_total_traders");
    let scan_max = (total_traders as usize).min(500);

    let mut entries: Vec<(String, u64, u64)> = Vec::with_capacity(scan_max);
    for i in 0..scan_max as u64 {
        let lk = format!("pm_tl_{}", i);
        if let Some(addr_data) = state
            .state
            .get_program_storage(PREDICT_SYMBOL, lk.as_bytes())
        {
            if addr_data.len() >= 32 {
                let addr_hex = hex::encode(&addr_data[..32]);
                let tk = format!("pm_ts_{}", addr_hex);
                if let Some(sd) = state
                    .state
                    .get_program_storage(PREDICT_SYMBOL, tk.as_bytes())
                {
                    if sd.len() >= 24 {
                        let vol = pm_u64(&sd, 0);
                        let trades = pm_u64(&sd, 8);
                        entries.push((addr_hex, vol, trades));
                    }
                }
            }
        }
    }

    entries.sort_by_key(|b| std::cmp::Reverse(b.1));
    entries.truncate(limit);

    let leaders: Vec<serde_json::Value> = entries
        .into_iter()
        .enumerate()
        .map(|(i, (addr, vol, trades))| {
            serde_json::json!({
                "rank": i + 1,
                "address": addr,
                "total_volume": vol as f64 / PM_PRICE_SCALE,
                "trade_count": trades,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "traders": leaders,
        "total_traders": total_traders,
    }))
}

/// getPredictionTrending — Active markets ranked by 24h volume
async fn handle_get_prediction_trending(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let total = state
        .state
        .get_program_storage_u64(PREDICT_SYMBOL, b"pm_market_count");

    let cat_map = |c: u8| -> &'static str {
        match c {
            0 => "politics",
            1 => "sports",
            2 => "crypto",
            3 => "science",
            4 => "entertainment",
            5 => "economics",
            6 => "tech",
            _ => "custom",
        }
    };

    let mut markets = Vec::new();
    for id in 1..=total {
        let key = format!("pm_m_{}", id);
        let data = match state
            .state
            .get_program_storage(PREDICT_SYMBOL, key.as_bytes())
        {
            Some(d) if d.len() >= 192 => d,
            _ => continue,
        };
        if data[64] != 1 {
            continue;
        } // only active

        let q_key = format!("pm_q_{}", id);
        let question = state
            .state
            .get_program_storage(PREDICT_SYMBOL, q_key.as_bytes())
            .and_then(|d| String::from_utf8(d).ok())
            .unwrap_or_default();

        let vol24_key = format!("pm_mv24_{}", id);
        let vol24 = state
            .state
            .get_program_storage_u64(PREDICT_SYMBOL, vol24_key.as_bytes());

        let tc_key = format!("pm_mtc_{}", id);
        let traders = state
            .state
            .get_program_storage_u64(PREDICT_SYMBOL, tc_key.as_bytes());

        markets.push((
            id,
            question,
            cat_map(data[67]).to_string(),
            vol24,
            traders,
            pm_u64(&data, 76) as f64 / PM_PRICE_SCALE,
        ));
    }

    markets.sort_by_key(|b| std::cmp::Reverse(b.3));
    markets.truncate(10);

    let items: Vec<serde_json::Value> = markets
        .into_iter()
        .map(|(id, q, cat, vol24, tc, tv)| {
            serde_json::json!({
                "id": id,
                "question": q,
                "category": cat,
                "volume_24h": vol24 as f64 / PM_PRICE_SCALE,
                "unique_traders": tc,
                "total_volume": tv,
            })
        })
        .collect();

    Ok(serde_json::json!(items))
}

/// getPredictionMarketAnalytics [market_id] — Per-market analytics
async fn handle_get_prediction_market_analytics(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let market_id = match &params {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => {
            arr[0].as_u64().ok_or(RpcError {
                code: -32602,
                message: "market_id must be u64".into(),
            })?
        }
        Some(serde_json::Value::Object(obj)) => obj
            .get("market_id")
            .or(obj.get("id"))
            .and_then(|v| v.as_u64())
            .ok_or(RpcError {
                code: -32602,
                message: "market_id required".into(),
            })?,
        _ => {
            return Err(RpcError {
                code: -32602,
                message: "Expected params: [market_id]".into(),
            })
        }
    };

    let tc_key = format!("pm_mtc_{}", market_id);
    let traders = state
        .state
        .get_program_storage_u64(PREDICT_SYMBOL, tc_key.as_bytes());
    let vol24_key = format!("pm_mv24_{}", market_id);
    let vol24 = state
        .state
        .get_program_storage_u64(PREDICT_SYMBOL, vol24_key.as_bytes());

    Ok(serde_json::json!({
        "market_id": market_id,
        "unique_traders": traders,
        "volume_24h": vol24 as f64 / PM_PRICE_SCALE,
    }))
}

// ═══════════════════════════════════════════════════════════════════════════════
// DEX & PLATFORM STATS JSON-RPC HANDLERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Read a u64 from CF_CONTRACT_STORAGE (fast path, always current).
/// This is the authoritative source — post-block hooks (SL/TP engine,
/// analytics bridge, candle resets) write to CF only, NOT to embedded storage.
fn cf_stats_u64(state: &RpcState, symbol: &str, key: &[u8]) -> u64 {
    state.state.get_program_storage_u64(symbol, key)
}

/// Read a bool flag from CF_CONTRACT_STORAGE.
fn cf_stats_bool(state: &RpcState, symbol: &str, key: &[u8]) -> bool {
    state
        .state
        .get_program_storage(symbol, key)
        .map(|d| d.first().copied().unwrap_or(0) != 0)
        .unwrap_or(false)
}

/// Resolve a symbol to its program Pubkey.
fn resolve_symbol_pubkey(state: &RpcState, symbol: &str) -> Result<Pubkey, RpcError> {
    state
        .state
        .get_symbol_registry(symbol)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("DB error: {}", e),
        })?
        .map(|e| e.program)
        .ok_or_else(symbol_not_found_error)
}

/// getDexCoreStats — DEX core exchange stats
async fn handle_get_dex_core_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DEX")?; // verify symbol exists
    Ok(serde_json::json!({
        "pair_count": cf_stats_u64(state, "DEX", b"dex_pair_count"),
        "order_count": cf_stats_u64(state, "DEX", b"dex_order_count"),
        "trade_count": cf_stats_u64(state, "DEX", b"dex_trade_count"),
        "total_volume": cf_stats_u64(state, "DEX", b"dex_total_volume"),
        "fee_treasury": cf_stats_u64(state, "DEX", b"dex_fee_treasury"),
        "paused": cf_stats_bool(state, "DEX", b"dex_paused"),
    }))
}

/// getDexAmmStats — AMM pool stats
async fn handle_get_dex_amm_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DEXAMM")?;
    Ok(serde_json::json!({
        "pool_count": cf_stats_u64(state, "DEXAMM", b"amm_pool_count"),
        "position_count": cf_stats_u64(state, "DEXAMM", b"amm_pos_count"),
        "swap_count": cf_stats_u64(state, "DEXAMM", b"amm_swap_count"),
        "total_volume": cf_stats_u64(state, "DEXAMM", b"amm_total_volume"),
        "total_fees": cf_stats_u64(state, "DEXAMM", b"amm_total_fees"),
        "paused": cf_stats_bool(state, "DEXAMM", b"amm_paused"),
    }))
}

/// getDexMarginStats — Margin trading stats
async fn handle_get_dex_margin_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DEXMARGIN")?;
    let per_pair_max = cf_stats_u64(state, "DEXMARGIN", b"mrg_maxl_0");
    let max_leverage = if per_pair_max > 0 { per_pair_max } else { 100 };
    Ok(serde_json::json!({
        "position_count": cf_stats_u64(state, "DEXMARGIN", b"mrg_pos_count"),
        "total_volume": cf_stats_u64(state, "DEXMARGIN", b"mrg_total_volume"),
        "liquidation_count": cf_stats_u64(state, "DEXMARGIN", b"mrg_liq_count"),
        "total_pnl_profit": cf_stats_u64(state, "DEXMARGIN", b"mrg_pnl_profit"),
        "total_pnl_loss": cf_stats_u64(state, "DEXMARGIN", b"mrg_pnl_loss"),
        "insurance_fund": cf_stats_u64(state, "DEXMARGIN", b"mrg_insurance"),
        "max_leverage": max_leverage,
        "paused": cf_stats_bool(state, "DEXMARGIN", b"mrg_paused"),
    }))
}

/// getDexRewardsStats — Rewards program stats
async fn handle_get_dex_rewards_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DEXREWARDS")?;
    Ok(serde_json::json!({
        "trade_count": cf_stats_u64(state, "DEXREWARDS", b"rew_trade_count"),
        "trader_count": cf_stats_u64(state, "DEXREWARDS", b"rew_trader_count"),
        "total_volume": cf_stats_u64(state, "DEXREWARDS", b"rew_total_volume"),
        "total_distributed": cf_stats_u64(state, "DEXREWARDS", b"rew_total_dist"),
        "epoch": cf_stats_u64(state, "DEXREWARDS", b"rew_epoch"),
        "paused": cf_stats_bool(state, "DEXREWARDS", b"rew_paused"),
    }))
}

/// getDexRouterStats — Router stats
async fn handle_get_dex_router_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DEXROUTER")?;
    Ok(serde_json::json!({
        "route_count": cf_stats_u64(state, "DEXROUTER", b"rtr_route_count"),
        "swap_count": cf_stats_u64(state, "DEXROUTER", b"rtr_swap_count"),
        "total_volume": cf_stats_u64(state, "DEXROUTER", b"rtr_total_volume"),
        "paused": cf_stats_bool(state, "DEXROUTER", b"rtr_paused"),
    }))
}

/// getDexAnalyticsStats — Analytics global stats
async fn handle_get_dex_analytics_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let program = resolve_symbol_pubkey(state, "ANALYTICS")?;

    // Aggregate candle counts and tracked pairs from CF_CONTRACT_STORAGE
    // Keys: "ana_cc_{pair_id}_{interval}" → u64 candle count
    // Keys: "ana_24h_{pair_id}" → 24h stats (presence means pair is tracked)
    let mut total_candles: u64 = 0;
    let mut tracked_pairs = std::collections::HashSet::new();

    // Iterate all storage entries for the ANALYTICS contract in CF
    let entries = state
        .state
        .get_contract_storage_entries(&program, 10_000, None)
        .unwrap_or_default();
    for (key, value) in &entries {
        if key.starts_with(b"ana_cc_") {
            if value.len() >= 8 {
                total_candles += u64::from_le_bytes([
                    value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
                ]);
            }
            if let Some(pair_part) = key.get(7..) {
                if let Some(end) = pair_part.iter().position(|&b| b == b'_') {
                    tracked_pairs.insert(pair_part[..end].to_vec());
                }
            }
        } else if key.starts_with(b"ana_24h_") {
            if let Some(pair_part) = key.get(8..) {
                tracked_pairs.insert(pair_part.to_vec());
            }
        }
    }

    Ok(serde_json::json!({
        "record_count": cf_stats_u64(state, "ANALYTICS", b"ana_rec_count"),
        "trader_count": cf_stats_u64(state, "ANALYTICS", b"ana_trader_count"),
        "total_volume": cf_stats_u64(state, "ANALYTICS", b"ana_total_volume"),
        "total_candles": total_candles,
        "tracked_pairs": tracked_pairs.len(),
        "paused": cf_stats_bool(state, "ANALYTICS", b"ana_paused"),
    }))
}

/// getDexGovernanceStats — Governance stats
/// AUDIT-NOTE F-14: This endpoint is already O(1). Each counter (proposal_count, total_votes,
/// voter_count) is stored as a dedicated key in contract storage and read via a single
/// `get_storage()` call. There is no proposal scanning or iteration over votes. The
/// `stats_u64()` helper performs a point-lookup, making this endpoint suitable for frequent
/// polling without performance concerns.
async fn handle_get_dex_governance_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DEXGOV")?;
    Ok(serde_json::json!({
        "proposal_count": cf_stats_u64(state, "DEXGOV", b"gov_prop_count"),
        "total_votes": cf_stats_u64(state, "DEXGOV", b"gov_total_votes"),
        "voter_count": cf_stats_u64(state, "DEXGOV", b"gov_voter_count"),
        "paused": cf_stats_bool(state, "DEXGOV", b"gov_paused"),
    }))
}

/// getLichenSwapStats — Legacy swap stats
async fn handle_get_lichenswap_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "LICHENSWAP")?;
    Ok(serde_json::json!({
        "swap_count": cf_stats_u64(state, "LICHENSWAP", b"ms_swap_count"),
        "volume_a": cf_stats_u64(state, "LICHENSWAP", b"ms_volume_a"),
        "volume_b": cf_stats_u64(state, "LICHENSWAP", b"ms_volume_b"),
        "paused": cf_stats_bool(state, "LICHENSWAP", b"ms_paused"),
    }))
}

/// getThallLendStats — Lending protocol stats
async fn handle_get_thalllend_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "LEND")?;
    Ok(serde_json::json!({
        "total_deposits": cf_stats_u64(state, "LEND", b"ll_total_deposits"),
        "total_borrows": cf_stats_u64(state, "LEND", b"ll_total_borrows"),
        "reserves": cf_stats_u64(state, "LEND", b"ll_reserves"),
        "deposit_count": cf_stats_u64(state, "LEND", b"ll_dep_count"),
        "borrow_count": cf_stats_u64(state, "LEND", b"ll_bor_count"),
        "liquidation_count": cf_stats_u64(state, "LEND", b"ll_liq_count"),
        "paused": cf_stats_bool(state, "LEND", b"ll_paused"),
    }))
}

/// getSporePayStats — Streaming payments stats
async fn handle_get_sporepay_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "SPOREPAY")?;
    Ok(serde_json::json!({
        "stream_count": cf_stats_u64(state, "SPOREPAY", b"stream_count"),
        "total_streamed": cf_stats_u64(state, "SPOREPAY", b"cp_total_streamed"),
        "total_withdrawn": cf_stats_u64(state, "SPOREPAY", b"cp_total_withdrawn"),
        "cancel_count": cf_stats_u64(state, "SPOREPAY", b"cp_cancel_count"),
        "paused": cf_stats_bool(state, "SPOREPAY", b"cp_paused"),
    }))
}

/// getBountyBoardStats — Bounty platform stats
async fn handle_get_bountyboard_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "BOUNTY")?;
    Ok(serde_json::json!({
        "bounty_count": cf_stats_u64(state, "BOUNTY", b"bounty_count"),
        "completed_count": cf_stats_u64(state, "BOUNTY", b"bb_completed_count"),
        "reward_volume": cf_stats_u64(state, "BOUNTY", b"bb_reward_volume"),
        "cancel_count": cf_stats_u64(state, "BOUNTY", b"bb_cancel_count"),
    }))
}

/// getComputeMarketStats — Compute marketplace stats
async fn handle_get_compute_market_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "COMPUTE")?;
    Ok(serde_json::json!({
        "job_count": cf_stats_u64(state, "COMPUTE", b"job_count"),
        "completed_count": cf_stats_u64(state, "COMPUTE", b"cm_completed_count"),
        "payment_volume": cf_stats_u64(state, "COMPUTE", b"cm_payment_volume"),
        "dispute_count": cf_stats_u64(state, "COMPUTE", b"cm_dispute_count"),
    }))
}

/// getMossStorageStats — Decentralized storage stats
async fn handle_get_moss_storage_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "MOSS")?;
    Ok(serde_json::json!({
        "data_count": cf_stats_u64(state, "MOSS", b"data_count"),
        "total_bytes": cf_stats_u64(state, "MOSS", b"moss_total_bytes"),
        "challenge_count": cf_stats_u64(state, "MOSS", b"moss_challenge_count"),
    }))
}

/// getLichenMarketStats — NFT marketplace stats
async fn handle_get_lichenmarket_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "MARKET")?;
    Ok(serde_json::json!({
        "listing_count": cf_stats_u64(state, "MARKET", b"mm_listing_count"),
        "sale_count": cf_stats_u64(state, "MARKET", b"mm_sale_count"),
        "sale_volume": cf_stats_u64(state, "MARKET", b"mm_sale_volume"),
        "paused": cf_stats_bool(state, "MARKET", b"mm_paused"),
    }))
}

/// getLichenAuctionStats — Auction stats
async fn handle_get_lichenauction_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "AUCTION")?;
    Ok(serde_json::json!({
        "auction_count": cf_stats_u64(state, "AUCTION", b"ma_auction_count"),
        "total_volume": cf_stats_u64(state, "AUCTION", b"ma_total_volume"),
        "total_sales": cf_stats_u64(state, "AUCTION", b"ma_total_sales"),
        "paused": cf_stats_bool(state, "AUCTION", b"ma_paused"),
    }))
}

/// getLichenPunksStats — NFT collection stats
async fn handle_get_lichenpunks_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "PUNKS")?;
    Ok(serde_json::json!({
        "total_minted": cf_stats_u64(state, "PUNKS", b"total_minted"),
        "transfer_count": cf_stats_u64(state, "PUNKS", b"mp_transfer_count"),
        "burn_count": cf_stats_u64(state, "PUNKS", b"mp_burn_count"),
        "paused": cf_stats_bool(state, "PUNKS", b"mp_paused"),
    }))
}

/// getLusdStats — lUSD stablecoin stats
async fn handle_get_musd_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "LUSD")?;
    Ok(serde_json::json!({
        "supply": cf_stats_u64(state, "LUSD", b"lusd_supply"),
        "total_minted": cf_stats_u64(state, "LUSD", b"lusd_minted"),
        "total_burned": cf_stats_u64(state, "LUSD", b"lusd_burned"),
        "mint_events": cf_stats_u64(state, "LUSD", b"lusd_mint_evt"),
        "burn_events": cf_stats_u64(state, "LUSD", b"lusd_burn_evt"),
        "transfer_count": cf_stats_u64(state, "LUSD", b"lusd_xfer_cnt"),
        "attestation_count": cf_stats_u64(state, "LUSD", b"lusd_att_count"),
        "reserve_attested": cf_stats_u64(state, "LUSD", b"lusd_reserve_att"),
        "paused": cf_stats_bool(state, "LUSD", b"lusd_paused"),
    }))
}

/// getWethStats — Wrapped ETH stats
async fn handle_get_weth_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "WETH")?;
    Ok(serde_json::json!({
        "supply": cf_stats_u64(state, "WETH", b"weth_supply"),
        "total_minted": cf_stats_u64(state, "WETH", b"weth_minted"),
        "total_burned": cf_stats_u64(state, "WETH", b"weth_burned"),
        "mint_events": cf_stats_u64(state, "WETH", b"weth_mint_evt"),
        "burn_events": cf_stats_u64(state, "WETH", b"weth_burn_evt"),
        "transfer_count": cf_stats_u64(state, "WETH", b"weth_xfer_cnt"),
        "attestation_count": cf_stats_u64(state, "WETH", b"weth_att_count"),
        "reserve_attested": cf_stats_u64(state, "WETH", b"weth_reserve_att"),
        "paused": cf_stats_bool(state, "WETH", b"weth_paused"),
    }))
}

/// getWsolStats — Wrapped SOL stats
async fn handle_get_wsol_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "WSOL")?;
    Ok(serde_json::json!({
        "supply": cf_stats_u64(state, "WSOL", b"wsol_supply"),
        "total_minted": cf_stats_u64(state, "WSOL", b"wsol_minted"),
        "total_burned": cf_stats_u64(state, "WSOL", b"wsol_burned"),
        "mint_events": cf_stats_u64(state, "WSOL", b"wsol_mint_evt"),
        "burn_events": cf_stats_u64(state, "WSOL", b"wsol_burn_evt"),
        "transfer_count": cf_stats_u64(state, "WSOL", b"wsol_xfer_cnt"),
        "attestation_count": cf_stats_u64(state, "WSOL", b"wsol_att_count"),
        "reserve_attested": cf_stats_u64(state, "WSOL", b"wsol_reserve_att"),
        "paused": cf_stats_bool(state, "WSOL", b"wsol_paused"),
    }))
}

/// getWbnbStats — Wrapped BNB stats
async fn handle_get_wbnb_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "WBNB")?;
    Ok(serde_json::json!({
        "supply": cf_stats_u64(state, "WBNB", b"wbnb_supply"),
        "total_minted": cf_stats_u64(state, "WBNB", b"wbnb_minted"),
        "total_burned": cf_stats_u64(state, "WBNB", b"wbnb_burned"),
        "mint_events": cf_stats_u64(state, "WBNB", b"wbnb_mint_evt"),
        "burn_events": cf_stats_u64(state, "WBNB", b"wbnb_burn_evt"),
        "transfer_count": cf_stats_u64(state, "WBNB", b"wbnb_xfer_cnt"),
        "attestation_count": cf_stats_u64(state, "WBNB", b"wbnb_att_count"),
        "reserve_attested": cf_stats_u64(state, "WBNB", b"wbnb_reserve_att"),
        "paused": cf_stats_bool(state, "WBNB", b"wbnb_paused"),
    }))
}

/// getSporeVaultStats — Yield vault stats
async fn handle_get_sporevault_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "SPOREVAULT")?;
    Ok(serde_json::json!({
        "total_assets": cf_stats_u64(state, "SPOREVAULT", b"cv_total_assets"),
        "total_shares": cf_stats_u64(state, "SPOREVAULT", b"cv_total_shares"),
        "strategy_count": cf_stats_u64(state, "SPOREVAULT", b"cv_strategy_count"),
        "total_earned": cf_stats_u64(state, "SPOREVAULT", b"cv_total_earned"),
        "fees_earned": cf_stats_u64(state, "SPOREVAULT", b"cv_fees_earned"),
        "protocol_fees": cf_stats_u64(state, "SPOREVAULT", b"cv_protocol_fees"),
        "paused": cf_stats_bool(state, "SPOREVAULT", b"cv_paused"),
    }))
}

/// getLichenBridgeStats — Cross-chain bridge stats
async fn handle_get_lichenbridge_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "BRIDGE")?;
    Ok(serde_json::json!({
        "nonce": cf_stats_u64(state, "BRIDGE", b"bridge_nonce"),
        "validator_count": cf_stats_u64(state, "BRIDGE", b"bridge_validator_count"),
        "required_confirms": cf_stats_u64(state, "BRIDGE", b"bridge_required_confirms"),
        "locked_amount": cf_stats_u64(state, "BRIDGE", b"bridge_locked_amount"),
        "request_timeout": cf_stats_u64(state, "BRIDGE", b"bridge_request_timeout"),
        "paused": cf_stats_bool(state, "BRIDGE", b"mb_paused"),
    }))
}

// ============================================================================
// BRIDGE DEPOSIT AUTH PROXY — Wallet-signed bridge access is verified here,
// forwarded to custody for re-verification, and wrapped in service Bearer auth.
// ============================================================================

/// createBridgeDeposit — Proxy to custody POST /deposits
/// Params: [{ user_id, chain, asset, auth: { issued_at, expires_at, signature } }]
async fn handle_create_bridge_deposit(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let payload = params
        .and_then(|v| {
            if v.is_array() {
                v.as_array().and_then(|a| a.first().cloned())
            } else if v.is_object() {
                Some(v)
            } else {
                None
            }
        })
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing params: expected [{ user_id, chain, asset }]".to_string(),
        })?;

    // Validate required fields
    let user_id = payload
        .get("user_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing user_id".to_string(),
        })?;
    let chain = payload
        .get("chain")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing chain".to_string(),
        })?;
    let asset = payload
        .get("asset")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing asset".to_string(),
        })?;
    let auth = payload.get("auth").ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing auth: expected wallet-signed bridge access".to_string(),
    })?;

    let valid_chains = ["solana", "ethereum", "bnb", "bsc"];
    let valid_assets = ["sol", "eth", "bnb", "usdc", "usdt"];
    if !valid_chains.contains(&chain) {
        return Err(RpcError {
            code: -32602,
            message: format!("Invalid chain: {}", chain),
        });
    }
    if !valid_assets.contains(&asset) {
        return Err(RpcError {
            code: -32602,
            message: format!("Invalid asset: {}", asset),
        });
    }

    // Validate user_id is a valid Lichen base58 public key (32 bytes)
    if bs58::decode(user_id)
        .into_vec()
        .map(|v| v.len())
        .unwrap_or(0)
        != 32
    {
        return Err(RpcError {
            code: -32602,
            message: "user_id must be a valid Lichen base58 public key (32 bytes)".to_string(),
        });
    }

    let bridge_auth = parse_bridge_access_auth(auth)?;
    verify_bridge_access_auth(user_id, &bridge_auth)?;

    let incident_status = load_incident_status_record(state);
    if let Some(reason) = bridge_deposit_incident_block_reason(&incident_status) {
        return Err(RpcError {
            code: -32000,
            message: reason.to_string(),
        });
    }

    let custody_url = state.custody_url.as_deref().ok_or_else(|| RpcError {
        code: -32000,
        message: "Bridge service not configured (CUSTODY_URL)".to_string(),
    })?;
    let auth_token = state
        .custody_auth_token
        .as_deref()
        .ok_or_else(|| RpcError {
            code: -32000,
            message: "Bridge service auth not configured".to_string(),
        })?;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/deposits", custody_url))
        .header("Authorization", format!("Bearer {}", auth_token))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "user_id": user_id,
            "chain": chain,
            "asset": asset,
            "auth": bridge_auth,
        }))
        .send()
        .await
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Bridge service unavailable: {}", e),
        })?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.map_err(|e| RpcError {
        code: -32000,
        message: format!("Bridge service invalid response: {}", e),
    })?;

    if !status.is_success() {
        let msg = body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Bridge request failed");
        return Err(RpcError {
            code: -32000,
            message: msg.to_string(),
        });
    }

    log_privileged_rpc_mutation(
        "createBridgeDeposit",
        "bridge_access",
        user_id,
        "bridge_deposit",
        body.get("deposit_id").and_then(|value| value.as_str()),
        serde_json::json!({
            "chain": chain,
            "asset": asset,
        }),
    );

    Ok(body)
}

/// getBridgeDeposit — Proxy to custody GET /deposits/:deposit_id
/// Params: [{ deposit_id, user_id, auth: { issued_at, expires_at, signature } }]
///    or [deposit_id, { user_id, auth: { ... } }]
fn parse_bridge_deposit_lookup_object(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<(String, String, BridgeAccessAuth), RpcError> {
    let deposit_id = object
        .get("deposit_id")
        .and_then(|value| value.as_str())
        .map(String::from)
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing deposit_id".to_string(),
        })?;
    let user_id = object
        .get("user_id")
        .and_then(|value| value.as_str())
        .map(String::from)
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing user_id".to_string(),
        })?;
    let auth = object.get("auth").ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing auth: expected wallet-signed bridge access".to_string(),
    })?;

    Ok((deposit_id, user_id, parse_bridge_access_auth(auth)?))
}

async fn handle_get_bridge_deposit(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let custody_url = state.custody_url.as_deref().ok_or_else(|| RpcError {
        code: -32000,
        message: "Bridge service not configured (CUSTODY_URL)".to_string(),
    })?;
    let auth_token = state
        .custody_auth_token
        .as_deref()
        .ok_or_else(|| RpcError {
            code: -32000,
            message: "Bridge service auth not configured".to_string(),
        })?;

    let payload = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params: expected [{ deposit_id, user_id, auth }]".to_string(),
    })?;
    let (deposit_id, user_id, bridge_auth) = if let Some(object) = payload.as_object() {
        parse_bridge_deposit_lookup_object(object)?
    } else if let Some(array) = payload.as_array() {
        if let Some(object) = array.first().and_then(|value| value.as_object()) {
            parse_bridge_deposit_lookup_object(object)?
        } else {
            let deposit_id = array
                .first()
                .and_then(|value| value.as_str())
                .map(String::from)
                .ok_or_else(|| RpcError {
                    code: -32602,
                    message: "Missing deposit_id".to_string(),
                })?;
            let auth_object = array
                .get(1)
                .and_then(|value| value.as_object())
                .ok_or_else(|| RpcError {
                    code: -32602,
                    message: "Missing auth params: expected [deposit_id, { user_id, auth }]"
                        .to_string(),
                })?;
            let user_id = auth_object
                .get("user_id")
                .and_then(|value| value.as_str())
                .map(String::from)
                .ok_or_else(|| RpcError {
                    code: -32602,
                    message: "Missing user_id".to_string(),
                })?;
            let auth = auth_object.get("auth").ok_or_else(|| RpcError {
                code: -32602,
                message: "Missing auth: expected wallet-signed bridge access".to_string(),
            })?;
            let bridge_auth = parse_bridge_access_auth(auth)?;
            (deposit_id, user_id, bridge_auth)
        }
    } else {
        return Err(RpcError {
            code: -32602,
            message: "Missing params: expected [{ deposit_id, user_id, auth }]".to_string(),
        });
    };

    // Basic ID validation — UUIDs are 36 chars (8-4-4-4-12 with hyphens)
    if deposit_id.len() != 36
        || !deposit_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-')
    {
        return Err(RpcError {
            code: -32602,
            message: "Invalid deposit_id format".to_string(),
        });
    }

    verify_bridge_access_auth(&user_id, &bridge_auth)?;
    let bridge_auth_json = serde_json::to_string(&bridge_auth).map_err(|e| RpcError {
        code: -32000,
        message: format!("Failed to encode bridge auth for custody lookup: {}", e),
    })?;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/deposits/{}", custody_url, deposit_id))
        .header("Authorization", format!("Bearer {}", auth_token))
        .query(&[
            ("user_id", user_id.as_str()),
            ("auth", bridge_auth_json.as_str()),
        ])
        .send()
        .await
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Bridge service unavailable: {}", e),
        })?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().await.map_err(|e| RpcError {
        code: -32000,
        message: format!("Bridge service invalid response: {}", e),
    })?;

    if !status.is_success() {
        let msg = body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Deposit lookup failed");
        return Err(RpcError {
            code: -32000,
            message: msg.to_string(),
        });
    }

    if body.get("user_id").and_then(|value| value.as_str()) != Some(user_id.as_str()) {
        return Err(RpcError {
            code: -32000,
            message: "Deposit not found for authenticated user".to_string(),
        });
    }

    Ok(body)
}

/// getLichenDaoStats — DAO governance stats
async fn handle_get_lichendao_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DAO")?;
    Ok(serde_json::json!({
        "proposal_count": cf_stats_u64(state, "DAO", b"proposal_count"),
        "min_proposal_threshold": cf_stats_u64(state, "DAO", b"min_proposal_threshold"),
        "paused": cf_stats_bool(state, "DAO", b"dao_paused"),
    }))
}

/// getLichenOracleStats — Oracle price feed stats
async fn handle_get_lichenoracle_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "ORACLE")?;
    Ok(serde_json::json!({
        "queries": cf_stats_u64(state, "ORACLE", b"stats_queries"),
        "feeds": cf_stats_u64(state, "ORACLE", b"stats_feeds"),
        "attestations": cf_stats_u64(state, "ORACLE", b"stats_attestations"),
        "paused": cf_stats_bool(state, "ORACLE", b"oracle_paused"),
    }))
}

/// getDexPairs — Returns trading pairs with last price for wallet price display.
/// Reads dex_core pair storage and enriches with oracle prices.
async fn handle_get_dex_pairs(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let pair_count = state
        .state
        .get_program_storage_u64("DEX", b"dex_pair_count");
    let limit = pair_count.min(100);
    let mut pairs = Vec::new();

    // Build symbol map from known token contracts
    let known_tokens: &[(&str, &str)] = &[
        ("LICN", "LICN"),
        ("LUSD", "lUSD"),
        ("WSOL", "wSOL"),
        ("WETH", "wETH"),
        ("WBNB", "wBNB"),
    ];
    let mut symbol_for_addr: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // Native LICN is the zero address sentinel [0;32] — map it explicitly
    // since there is no lichencoin contract in the symbol registry.
    let zero_pubkey = lichen_core::Pubkey([0u8; 32]);
    symbol_for_addr.insert(zero_pubkey.to_string(), "LICN".to_string());
    for &(sym, display) in known_tokens {
        if let Ok(Some(entry)) = state.state.get_symbol_registry(sym) {
            symbol_for_addr.insert(entry.program.to_string(), display.to_string());
        }
    }

    for i in 1..=limit {
        let key = format!("dex_pair_{}", i);
        if let Some(data) = state.state.get_program_storage("DEX", key.as_bytes()) {
            // 112-byte pair blob: base[0..32] | quote[32..64] | pair_id[64..72] | ...
            if data.len() >= 112 {
                let mut base_bytes = [0u8; 32];
                let mut quote_bytes = [0u8; 32];
                base_bytes.copy_from_slice(&data[0..32]);
                quote_bytes.copy_from_slice(&data[32..64]);
                let pair_id = u64::from_le_bytes(data[64..72].try_into().unwrap_or([0; 8]));
                let base_pk = lichen_core::Pubkey(base_bytes);
                let quote_pk = lichen_core::Pubkey(quote_bytes);
                let base_str = base_pk.to_string();
                let quote_str = quote_pk.to_string();
                let base = symbol_for_addr
                    .get(&base_str)
                    .cloned()
                    .unwrap_or_else(|| base_str[..8].to_string());
                let quote = symbol_for_addr
                    .get(&quote_str)
                    .cloned()
                    .unwrap_or_else(|| quote_str[..8].to_string());

                // Read last price from analytics
                let lp_key = format!("ana_lp_{}", pair_id);
                let lp_raw = state
                    .state
                    .get_program_storage_u64("ANALYTICS", lp_key.as_bytes());
                let price = if lp_raw > 0 {
                    lp_raw as f64 / 1_000_000_000.0
                } else {
                    let base_usd = lichen_core::consensus::consensus_oracle_price_from_state(
                        &state.state,
                        &base,
                    );
                    match quote.as_str() {
                        "LICN" => {
                            let licn_usd =
                                lichen_core::consensus::licn_price_from_state(&state.state);
                            if let Some(base_usd) = base_usd {
                                if licn_usd > 0.0 {
                                    base_usd / licn_usd
                                } else {
                                    0.0
                                }
                            } else {
                                0.0
                            }
                        }
                        _ => base_usd.unwrap_or(0.0),
                    }
                };

                pairs.push(serde_json::json!({
                    "pair_id": pair_id,
                    "base": base,
                    "quote": quote,
                    "price": price,
                }));
            }
        }
    }

    Ok(serde_json::json!(pairs))
}

/// getOraclePrices — Returns current oracle prices for all known assets.
async fn handle_get_oracle_prices(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let assets = ["LICN", "wSOL", "wETH", "wBNB", "lUSD"];
    let mut prices = serde_json::Map::new();
    prices.insert("source".to_string(), serde_json::json!("native_consensus"));
    for asset in &assets {
        let price = lichen_core::consensus::consensus_oracle_price_from_state(&state.state, asset)
            .unwrap_or(0.0);
        prices.insert(asset.to_string(), serde_json::json!(price));
    }
    Ok(serde_json::Value::Object(prices))
}

#[cfg(test)]
mod tests {
    use super::{
        bridge_access_message, classify_method, classify_solana_method_tier, constant_time_eq,
        decode_contract_result_u64, encode_readonly_return_data_b64, encode_rpc_response,
        filter_signatures_for_address, get_cached_program_list_response,
        handle_create_bridge_deposit, handle_get_all_symbol_registry, handle_get_bridge_deposit,
        handle_get_contract_info, handle_get_governance_events, handle_get_incident_status,
        handle_get_program, handle_get_program_stats, handle_get_service_fleet_status,
        handle_get_signed_metadata_manifest, handle_set_fee_config, handle_solana_get_account_info,
        handle_solana_get_token_account_balance, handle_solana_get_token_accounts_by_owner,
        live_signed_metadata_source_rpc, parse_bridge_access_auth, parse_get_block_slot_param,
        parse_governance_event, parse_rpc_request, parse_rpc_tier_probe, parse_topic_hash,
        pq_signature_json, put_cached_program_list_response, validate_incoming_transaction_limits,
        validate_solana_encoding, validate_solana_transaction_details, verify_admin_auth,
        verify_bridge_access_auth_at, AirdropCooldowns, MethodTier, RateLimiter, RpcError,
        RpcResponse, RpcState, AIRDROP_COOLDOWN_MAX_ENTRIES, AIRDROP_COOLDOWN_SECS,
        PROGRAM_LIST_CACHE_TTL_MS, SOLANA_SPL_TOKEN_PROGRAM_ID, SOLANA_TOKEN_ACCOUNT_SPACE,
    };
    use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
    use lichen_core::account::Keypair as LichenKeypair;
    use lichen_core::contract::{ContractAccount, ContractEvent, ContractResult};
    use lichen_core::keypair_file::KeypairFile;
    use lichen_core::{
        consensus::{ValidatorInfo, ValidatorSet},
        Hash, Pubkey, StateStore, SymbolRegistryEntry, SYSTEM_PROGRAM_ID,
    };
    use lru::LruCache;
    use std::collections::HashMap;
    use std::num::NonZeroUsize;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;
    use tokio::sync::{Mutex as TokioMutex, RwLock};

    fn make_test_rpc_state_with_program_cache_capacity(
        state: StateStore,
        program_cache_capacity: usize,
    ) -> RpcState {
        RpcState {
            state,
            tx_sender: None,
            p2p: None,
            stake_pool: None,
            live_validator_set: None,
            chain_id: "lichen-test".to_string(),
            network_id: "local-testnet".to_string(),
            min_validator_stake: 0,
            version: "test".to_string(),
            evm_chain_id: 31337,
            solana_tx_cache: Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(16).unwrap()))),
            admin_token: Arc::new(std::sync::RwLock::new(None)),
            rate_limiter: Arc::new(RateLimiter::new(1_000)),
            finality: None,
            _dex_broadcaster: Arc::new(super::dex_ws::DexEventBroadcaster::new(16)),
            prediction_broadcaster: Arc::new(super::ws::PredictionEventBroadcaster::new(16)),
            validator_cache: Arc::new(RwLock::new((Instant::now(), Vec::new()))),
            metrics_cache: Arc::new(RwLock::new((Instant::now(), None))),
            program_list_response_cache: Arc::new(RwLock::new(LruCache::new(
                NonZeroUsize::new(program_cache_capacity).unwrap(),
            ))),
            airdrop_cooldowns: Arc::new(RwLock::new(AirdropCooldowns::default())),
            orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
            custody_url: None,
            custody_auth_token: None,
            incident_status_path: None,
            signed_metadata_manifest_path: None,
            signed_metadata_keypair_path: None,
            signed_metadata_manifest_cache: Arc::new(RwLock::new(None)),
            service_fleet_config_path: None,
            service_fleet_upstream_rpc_url: None,
            service_fleet_status_path: None,
            service_fleet_status_cache: Arc::new(RwLock::new((
                Instant::now() - std::time::Duration::from_secs(60),
                None,
            ))),
            treasury_keypair: None,
        }
    }

    fn make_test_rpc_state(state: StateStore) -> RpcState {
        make_test_rpc_state_with_program_cache_capacity(state, 16)
    }

    #[tokio::test]
    async fn cached_validators_prefers_live_validator_set_activity() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();
        let pubkey = Pubkey([7u8; 32]);

        let persisted_validator = ValidatorInfo {
            pubkey,
            stake: 1,
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            joined_slot: 1,
            last_active_slot: 5,
            last_observed_at_ms: 0,
            last_observed_block_at_ms: 0,
            last_observed_block_slot: 0,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: false,
        };
        let mut persisted_set = ValidatorSet::new();
        persisted_set.add_validator(persisted_validator);
        state.save_validator_set(&persisted_set).unwrap();

        let live_validator = ValidatorInfo {
            last_active_slot: 42,
            votes_cast: 9,
            ..persisted_set.validators()[0].clone()
        };
        let mut live_set = ValidatorSet::new();
        live_set.add_validator(live_validator);

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.live_validator_set = Some(Arc::new(RwLock::new(live_set)));

        let validators = super::cached_validators(&rpc_state).await.unwrap();
        assert_eq!(validators.len(), 1);
        assert_eq!(validators[0].last_active_slot, 42);
        assert_eq!(validators[0].votes_cast, 9);
    }

    fn put_test_contract_account(
        state: &StateStore,
        program_pubkey: Pubkey,
        owner: Pubkey,
        snapshot_entries: &[(&[u8], &[u8])],
    ) {
        put_test_contract_account_with_code(
            state,
            program_pubkey,
            owner,
            vec![0x00, 0x61, 0x73, 0x6d],
            snapshot_entries,
        );
    }

    fn put_test_contract_account_with_code(
        state: &StateStore,
        program_pubkey: Pubkey,
        owner: Pubkey,
        code: Vec<u8>,
        snapshot_entries: &[(&[u8], &[u8])],
    ) {
        let mut contract = ContractAccount::new(code, owner);
        for (key, value) in snapshot_entries {
            contract.set_storage((*key).to_vec(), (*value).to_vec());
        }

        let mut account = lichen_core::Account::new(0, program_pubkey);
        account.data = serde_json::to_vec(&contract).unwrap();
        account.executable = true;
        state.put_account(&program_pubkey, &account).unwrap();
    }

    fn make_hash(value: u8) -> Hash {
        Hash([value; 32])
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

    fn make_contract_result(return_data: Vec<u8>, return_code: Option<i64>) -> ContractResult {
        ContractResult {
            return_data,
            logs: Vec::new(),
            events: Vec::new(),
            storage_changes: HashMap::new(),
            success: true,
            error: None,
            compute_used: 0,
            return_code,
            cross_call_changes: HashMap::new(),
            cross_call_events: Vec::new(),
            cross_call_logs: Vec::new(),
            ccc_value_deltas: HashMap::new(),
            native_account_ops: Vec::new(),
        }
    }

    #[tokio::test]
    async fn contract_info_reads_total_supply_from_canonical_storage() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();
        let rpc_state = make_test_rpc_state(state);

        let program = Pubkey([11u8; 32]);
        let owner = Pubkey([12u8; 32]);
        let embedded_supply = 10u64.to_le_bytes();
        let snapshot_entries = [(b"test_supply".as_slice(), embedded_supply.as_slice())];
        put_test_contract_account(&rpc_state.state, program, owner, &snapshot_entries);
        rpc_state
            .state
            .put_contract_storage(&program, b"test_supply", &42u64.to_le_bytes())
            .unwrap();
        rpc_state
            .state
            .register_symbol(
                "TEST",
                SymbolRegistryEntry {
                    symbol: String::new(),
                    program,
                    owner,
                    name: Some("Test Token".to_string()),
                    template: None,
                    metadata: None,
                    decimals: Some(9),
                },
            )
            .unwrap();

        let response =
            handle_get_contract_info(&rpc_state, Some(serde_json::json!([program.to_base58()])))
                .await
                .unwrap();

        assert_eq!(
            response["token_metadata"]["total_supply"],
            serde_json::json!(42u64)
        );
    }

    #[tokio::test]
    async fn program_endpoints_report_canonical_storage_stats() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();
        let rpc_state = make_test_rpc_state(state);

        let program = Pubkey([13u8; 32]);
        let owner = Pubkey([14u8; 32]);
        let snapshot_entries = [
            (b"ghost".as_slice(), b"stale".as_slice()),
            (b"shadow".as_slice(), b"value".as_slice()),
        ];
        put_test_contract_account(&rpc_state.state, program, owner, &snapshot_entries);
        rpc_state
            .state
            .put_contract_storage(&program, b"alpha", b"one")
            .unwrap();
        rpc_state
            .state
            .put_contract_storage(&program, b"beta", b"three")
            .unwrap();

        let program_response =
            handle_get_program(&rpc_state, Some(serde_json::json!([program.to_base58()])))
                .await
                .unwrap();
        assert_eq!(program_response["storage_entries"], serde_json::json!(2));
        assert_eq!(program_response["storage_size"], serde_json::json!(8));

        let stats_response =
            handle_get_program_stats(&rpc_state, Some(serde_json::json!([program.to_base58()])))
                .await
                .unwrap();
        assert_eq!(stats_response["storage_entries"], serde_json::json!(2));
    }

    #[tokio::test]
    async fn call_contract_readonly_uses_top_level_runtime_context() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();
        let rpc_state = make_test_rpc_state(state);
        use base64::Engine as _;

        let caller = Pubkey([21u8; 32]);
        let program = Pubkey([22u8; 32]);
        let owner = Pubkey([23u8; 32]);
        let lichenid_program = Pubkey([24u8; 32]);
        let rep_key = lichen_core::contract::lichenid_reputation_storage_key(&caller);
        let rep_data = 42u64.to_le_bytes().to_vec();

        put_test_contract_account_with_code(
            &rpc_state.state,
            program,
            owner,
            reputation_reader_contract_code(&rep_key),
            &[],
        );
        rpc_state
            .state
            .put_contract_storage(&program, b"pm_lichenid_addr", &lichenid_program.0)
            .unwrap();
        rpc_state
            .state
            .put_contract_storage(&lichenid_program, &rep_key, &rep_data)
            .unwrap();

        let response = super::handle_call_contract(
            &rpc_state,
            Some(serde_json::json!({
                "contract": program.to_base58(),
                "function": "read_reputation",
                "from": caller.to_base58(),
            })),
        )
        .await
        .unwrap();

        let return_data = response["returnData"]
            .as_str()
            .expect("return data should exist");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(return_data)
            .expect("return data should decode");

        assert_eq!(decoded, rep_data);
        assert_eq!(response["success"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn program_list_cache_overflow_evicts_only_lru_entry() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();
        let rpc_state = make_test_rpc_state_with_program_cache_capacity(state, 2);

        put_cached_program_list_response(
            &rpc_state,
            "getPrograms",
            &None,
            serde_json::json!({"key": "programs"}),
        )
        .await;
        put_cached_program_list_response(
            &rpc_state,
            "getAllContracts",
            &None,
            serde_json::json!({"key": "contracts"}),
        )
        .await;

        assert!(
            get_cached_program_list_response(&rpc_state, "getPrograms", &None)
                .await
                .is_some()
        );

        put_cached_program_list_response(
            &rpc_state,
            "getAllSymbolRegistry",
            &None,
            serde_json::json!({"key": "symbols"}),
        )
        .await;

        assert_eq!(
            get_cached_program_list_response(&rpc_state, "getPrograms", &None).await,
            Some(serde_json::json!({"key": "programs"}))
        );
        assert_eq!(
            get_cached_program_list_response(&rpc_state, "getAllContracts", &None).await,
            None
        );
        assert_eq!(
            get_cached_program_list_response(&rpc_state, "getAllSymbolRegistry", &None).await,
            Some(serde_json::json!({"key": "symbols"}))
        );
    }

    #[tokio::test]
    async fn program_list_cache_drops_expired_entries_on_lookup() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();
        let rpc_state = make_test_rpc_state_with_program_cache_capacity(state, 2);

        {
            let mut guard = rpc_state.program_list_response_cache.write().await;
            guard.put(
                "getPrograms:null".to_string(),
                (
                    Instant::now()
                        - Duration::from_millis((PROGRAM_LIST_CACHE_TTL_MS * 2 + 1) as u64),
                    serde_json::json!({"stale": true}),
                ),
            );
        }

        assert_eq!(
            get_cached_program_list_response(&rpc_state, "getPrograms", &None).await,
            None
        );
        assert_eq!(rpc_state.program_list_response_cache.read().await.len(), 0);
    }

    fn signed_bridge_deposit_payload(seed: u8, chain: &str, asset: &str) -> serde_json::Value {
        let keypair = LichenKeypair::from_seed(&[seed; 32]);
        let user_id = keypair.pubkey().to_base58();
        let issued_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_secs();
        let expires_at = issued_at + 600;
        let message = bridge_access_message(&user_id, issued_at, expires_at);

        serde_json::json!({
            "user_id": user_id,
            "chain": chain,
            "asset": asset,
            "auth": {
                "issued_at": issued_at,
                "expires_at": expires_at,
                "signature": pq_signature_json(&keypair.sign(&message)),
            }
        })
    }

    #[derive(Clone, Default)]
    struct MockCustodyState {
        requests: Arc<TokioMutex<Vec<serde_json::Value>>>,
        lookups: Arc<TokioMutex<Vec<serde_json::Value>>>,
    }

    async fn mock_custody_create_deposit(
        State(state): State<MockCustodyState>,
        Json(payload): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        state.requests.lock().await.push(payload);
        Json(serde_json::json!({
            "deposit_id": "11111111-1111-1111-1111-111111111111",
            "address": "mock-bridge-address"
        }))
    }

    async fn mock_custody_get_deposit(
        State(state): State<MockCustodyState>,
        axum::extract::Path(deposit_id): axum::extract::Path<String>,
        axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>,
    ) -> Json<serde_json::Value> {
        state.lookups.lock().await.push(serde_json::json!({
            "deposit_id": deposit_id,
            "query": query,
        }));

        let user_id = query.get("user_id").cloned().unwrap_or_default();
        Json(serde_json::json!({
            "deposit_id": "11111111-1111-1111-1111-111111111111",
            "user_id": user_id,
            "chain": "solana",
            "asset": "sol",
            "address": "mock-bridge-address",
            "derivation_path": "m/44'/501'/0'/0/0",
            "deposit_seed_source": "treasury_root",
            "created_at": 0,
            "status": "issued"
        }))
    }

    async fn mock_custody_create_deposit_rate_limited(
        State(state): State<MockCustodyState>,
        Json(payload): Json<serde_json::Value>,
    ) -> (axum::http::StatusCode, Json<serde_json::Value>) {
        state.requests.lock().await.push(payload);
        (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": "invalid_request",
                "message": "rate_limited: wait 10s between deposit requests"
            })),
        )
    }

    async fn mock_custody_create_deposit_replayed_auth(
        State(state): State<MockCustodyState>,
        Json(payload): Json<serde_json::Value>,
    ) -> (axum::http::StatusCode, Json<serde_json::Value>) {
        state.requests.lock().await.push(payload);
        (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": "invalid_request",
                "message": "bridge auth already used for a different deposit request; sign a new bridge authorization"
            })),
        )
    }

    async fn spawn_mock_server(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock listener");
        let addr = listener.local_addr().expect("mock listener addr");
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .expect("serve mock app");
        });
        format!("http://{}", addr)
    }

    #[derive(Clone, Default)]
    struct CapturedLogWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedLogWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn capture_logs_async<F>(future: F) -> String
    where
        F: std::future::Future<Output = ()>,
    {
        static LOG_CAPTURE_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> =
            std::sync::OnceLock::new();
        let _capture_guard = LOG_CAPTURE_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();

        let buffer = CapturedLogWriter::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        tracing::dispatcher::with_default(&dispatch, || runtime.block_on(future));

        let captured = buffer.0.lock().unwrap().clone();
        String::from_utf8(captured).unwrap()
    }

    #[test]
    fn test_validate_solana_encoding() {
        assert!(validate_solana_encoding("json").is_ok());
        assert!(validate_solana_encoding("base58").is_ok());
        assert!(validate_solana_encoding("base64").is_ok());
        assert!(validate_solana_encoding("binary").is_err());
    }

    #[test]
    fn test_validate_solana_transaction_details() {
        assert!(validate_solana_transaction_details("full").is_ok());
        assert!(validate_solana_transaction_details("signatures").is_ok());
        assert!(validate_solana_transaction_details("none").is_ok());
        assert!(validate_solana_transaction_details("accounts").is_err());
    }

    #[test]
    fn test_filter_signatures_for_address() {
        let h1 = make_hash(1);
        let h2 = make_hash(2);
        let h3 = make_hash(3);
        let h4 = make_hash(4);

        let indexed = vec![(h4, 4), (h3, 3), (h2, 2), (h1, 1)];

        let filtered = filter_signatures_for_address(indexed.clone(), Some(h3), None, 10);
        assert_eq!(filtered, vec![(h2, 2), (h1, 1)]);

        let filtered = filter_signatures_for_address(indexed.clone(), None, Some(h2), 10);
        assert_eq!(filtered, vec![(h4, 4), (h3, 3)]);

        let filtered = filter_signatures_for_address(indexed.clone(), Some(h3), Some(h1), 10);
        assert_eq!(filtered, vec![(h2, 2)]);

        let filtered = filter_signatures_for_address(indexed, None, None, 1);
        assert_eq!(filtered, vec![(h4, 4)]);
    }

    #[test]
    fn test_encode_readonly_return_data_b64_prefers_explicit_return_data() {
        let result = make_contract_result(vec![1, 2, 3, 4], Some(99));

        assert_eq!(
            encode_readonly_return_data_b64(&result),
            Some(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                [1u8, 2, 3, 4],
            ))
        );
    }

    #[test]
    fn test_encode_readonly_return_data_b64_falls_back_to_scalar_return_code() {
        let result = make_contract_result(Vec::new(), Some(1_000_000_000_000_000));

        assert_eq!(
            encode_readonly_return_data_b64(&result),
            Some(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                1_000_000_000_000_000i64.to_le_bytes(),
            ))
        );
    }

    #[test]
    fn test_decode_contract_result_u64_supports_return_data_and_scalar_return_code() {
        let from_return_data = make_contract_result(42u64.to_le_bytes().to_vec(), Some(7));
        let from_return_code = make_contract_result(Vec::new(), Some(84));

        assert_eq!(decode_contract_result_u64(&from_return_data), Some(42));
        assert_eq!(decode_contract_result_u64(&from_return_code), Some(84));
    }

    #[test]
    fn test_set_fee_config_emits_privileged_audit_log() {
        let logs = capture_logs_async(async {
            let dir = tempdir().unwrap();
            let state = StateStore::open(dir.path()).unwrap();
            let rpc_state = make_test_rpc_state(state);
            *rpc_state.admin_token.write().unwrap() = Some("supersecret".to_string());

            let response = handle_set_fee_config(
                &rpc_state,
                Some(serde_json::json!({
                    "base_fee_spores": 42,
                })),
                Some("Bearer supersecret"),
            )
            .await
            .expect("setFeeConfig should succeed in local admin mode");

            assert_eq!(response["status"], "ok");
        });

        assert!(logs.contains("Privileged RPC mutation executed"));
        assert!(logs.contains("setFeeConfig"));
        assert!(logs.contains("fee_config"));
        assert!(logs.contains("admin_token"));
    }

    #[test]
    fn test_m02_native_get_block_is_moderate() {
        assert_eq!(classify_method("getBlock"), MethodTier::Moderate);
    }

    #[test]
    fn test_m02_get_governance_events_is_moderate() {
        assert_eq!(classify_method("getGovernanceEvents"), MethodTier::Moderate);
    }

    #[test]
    fn test_m02_solana_block_and_signature_statuses_are_moderate() {
        assert_eq!(
            classify_solana_method_tier("getBlock"),
            MethodTier::Moderate
        );
        assert_eq!(
            classify_solana_method_tier("getSignatureStatuses"),
            MethodTier::Moderate
        );
    }

    #[test]
    fn test_hi13_constant_time_eq_matches_only_identical_tokens() {
        assert!(constant_time_eq(b"admin-token", b"admin-token"));
        assert!(!constant_time_eq(b"admin-token", b"admin-tokfn"));
        assert!(!constant_time_eq(b"admin-token", b"admin-token-extra"));
    }

    #[test]
    fn test_hi13_verify_admin_auth_uses_shared_header_path() {
        let dir = tempdir().unwrap();
        let state_store = StateStore::open(dir.path()).unwrap();
        let mut state = make_test_rpc_state(state_store);
        state.admin_token = Arc::new(std::sync::RwLock::new(Some("supersecret".to_string())));

        assert!(verify_admin_auth(&state, Some("Bearer supersecret")).is_ok());

        let invalid = verify_admin_auth(&state, Some("Bearer wrong-token")).unwrap_err();
        assert_eq!(invalid.code, -32003);
        assert_eq!(invalid.message, "Invalid admin token");

        let missing = verify_admin_auth(&state, Some("supersecret")).unwrap_err();
        assert_eq!(missing.code, -32003);
        assert_eq!(
            missing.message,
            "Missing Authorization: Bearer <token> header"
        );
    }

    #[test]
    fn test_hi14_verify_admin_auth_observes_live_token_rotation() {
        let dir = tempdir().unwrap();
        let state_store = StateStore::open(dir.path()).unwrap();
        let mut state = make_test_rpc_state(state_store);
        state.admin_token = Arc::new(std::sync::RwLock::new(Some("old-token".to_string())));

        assert!(verify_admin_auth(&state, Some("Bearer old-token")).is_ok());

        {
            let mut guard = state.admin_token.write().unwrap();
            *guard = Some("new-token".to_string());
        }

        let stale = verify_admin_auth(&state, Some("Bearer old-token")).unwrap_err();
        assert_eq!(stale.code, -32003);
        assert_eq!(stale.message, "Invalid admin token");
        assert!(verify_admin_auth(&state, Some("Bearer new-token")).is_ok());

        {
            let mut guard = state.admin_token.write().unwrap();
            *guard = None;
        }

        let disabled = verify_admin_auth(&state, Some("Bearer new-token")).unwrap_err();
        assert_eq!(disabled.code, -32003);
        assert_eq!(
            disabled.message,
            "Admin endpoints disabled: no admin_token configured"
        );
    }

    #[test]
    fn test_m04_airdrop_cooldown_enforced_and_expires() {
        let mut cooldowns = AirdropCooldowns::default();
        let now = Instant::now();

        assert_eq!(cooldowns.check_and_record("addr1", now), None);

        let retry_now = now + Duration::from_secs(1);
        let remaining = cooldowns.check_and_record("addr1", retry_now);
        assert_eq!(remaining, Some(AIRDROP_COOLDOWN_SECS - 1));

        let after_window = now + Duration::from_secs(AIRDROP_COOLDOWN_SECS + 1);
        assert_eq!(cooldowns.check_and_record("addr1", after_window), None);
    }

    #[test]
    fn test_m04_airdrop_cooldown_bounded_size() {
        let mut cooldowns = AirdropCooldowns::default();
        let base = Instant::now();

        for idx in 0..(AIRDROP_COOLDOWN_MAX_ENTRIES + 500) {
            let address = format!("addr{}", idx);
            let _ = cooldowns.check_and_record(&address, base + Duration::from_secs(idx as u64));
        }

        assert!(cooldowns.by_address.len() <= AIRDROP_COOLDOWN_MAX_ENTRIES);
    }

    #[test]
    fn test_m05_tier_probe_succeeds_before_full_request_deserialization() {
        let body = br#"{"id":7,"method":"sendTransaction"}"#;
        let probe = parse_rpc_tier_probe(body).expect("probe should parse method + id");

        assert_eq!(probe.method, "sendTransaction");
        assert_eq!(probe.id, Some(serde_json::json!(7)));

        let full = parse_rpc_request(body, serde_json::json!(7));
        assert!(
            full.is_err(),
            "full request parse should fail (missing required jsonrpc field)"
        );
    }

    #[test]
    fn test_m05_tier_probe_extracts_method_for_solana_request() {
        let body = br#"{"id":"abc","method":"getSignatureStatuses","params":[["sig"]]}"#;
        let probe = parse_rpc_tier_probe(body).expect("probe should parse method for tiering");

        assert_eq!(probe.method, "getSignatureStatuses");
        assert_eq!(probe.id, Some(serde_json::json!("abc")));
    }

    #[test]
    fn test_l01_rejects_too_many_instructions_on_incoming_transaction() {
        let instruction = lichen_core::Instruction {
            program_id: lichen_core::SYSTEM_PROGRAM_ID,
            accounts: Vec::new(),
            data: vec![0],
        };
        let mut seed = [0u8; 32];
        seed[0] = 1;
        let tx = lichen_core::Transaction {
            signatures: vec![lichen_core::Keypair::from_seed(&seed).sign(b"limits-1")],
            message: lichen_core::Message {
                instructions: vec![
                    instruction;
                    lichen_core::transaction::MAX_INSTRUCTIONS_PER_TX + 1
                ],
                recent_blockhash: lichen_core::Hash([7u8; 32]),
                compute_budget: None,
                compute_unit_price: None,
            },
            tx_type: Default::default(),
        };

        let err = validate_incoming_transaction_limits(&tx)
            .expect_err("must reject oversized instruction count");
        assert_eq!(err.code, -32003);
        assert!(err.message.contains("Too many instructions"));
    }

    #[test]
    fn test_l01_rejects_oversized_instruction_data_on_incoming_transaction() {
        let mut seed = [0u8; 32];
        seed[0] = 2;
        let tx = lichen_core::Transaction {
            signatures: vec![lichen_core::Keypair::from_seed(&seed).sign(b"limits-2")],
            message: lichen_core::Message {
                instructions: vec![lichen_core::Instruction {
                    program_id: lichen_core::SYSTEM_PROGRAM_ID,
                    accounts: Vec::new(),
                    data: vec![0u8; lichen_core::transaction::MAX_INSTRUCTION_DATA + 1],
                }],
                recent_blockhash: lichen_core::Hash([9u8; 32]),
                compute_budget: None,
                compute_unit_price: None,
            },
            tx_type: Default::default(),
        };

        let err = validate_incoming_transaction_limits(&tx)
            .expect_err("must reject oversized instruction data");
        assert_eq!(err.code, -32003);
        assert!(err.message.contains("data too large"));
    }

    #[test]
    fn test_l04_get_block_rejects_hash_like_string_param() {
        let hash_param = serde_json::json!(["0xdeadbeef"]);
        let err = parse_get_block_slot_param(Some(&hash_param), false)
            .expect_err("hash-like string must be rejected for getBlock slot param");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("block hash is not supported"));
    }

    #[test]
    fn test_l04_get_block_accepts_slot_u64_param() {
        let params = serde_json::json!([123u64]);
        let slot = parse_get_block_slot_param(Some(&params), false)
            .expect("u64 slot param should be accepted");
        assert_eq!(slot, 123u64);
    }

    #[test]
    fn test_bridge_access_auth_verifies_valid_signature() {
        let keypair = LichenKeypair::from_seed(&[7u8; 32]);
        let user_id = keypair.pubkey().to_base58();
        let issued_at = 1_700_000_000u64;
        let expires_at = issued_at + 600;
        let message = bridge_access_message(&user_id, issued_at, expires_at);
        let auth = parse_bridge_access_auth(&serde_json::json!({
            "issued_at": issued_at,
            "expires_at": expires_at,
            "signature": pq_signature_json(&keypair.sign(&message)),
        }))
        .expect("parse bridge auth");

        verify_bridge_access_auth_at(&user_id, &auth, issued_at + 60)
            .expect("valid signed bridge auth must verify");
    }

    #[test]
    fn test_bridge_access_auth_rejects_expired_signature() {
        let keypair = LichenKeypair::from_seed(&[8u8; 32]);
        let user_id = keypair.pubkey().to_base58();
        let issued_at = 1_700_000_000u64;
        let expires_at = issued_at + 600;
        let message = bridge_access_message(&user_id, issued_at, expires_at);
        let auth = parse_bridge_access_auth(&serde_json::json!({
            "issued_at": issued_at,
            "expires_at": expires_at,
            "signature": pq_signature_json(&keypair.sign(&message)),
        }))
        .expect("parse bridge auth");

        let err = verify_bridge_access_auth_at(&user_id, &auth, expires_at + 1)
            .expect_err("expired bridge auth must fail");
        assert!(err.message.contains("expired"));
    }

    #[test]
    fn test_bridge_access_auth_rejects_wrong_user() {
        let signer = LichenKeypair::from_seed(&[9u8; 32]);
        let other_user = LichenKeypair::from_seed(&[10u8; 32]).pubkey().to_base58();
        let signer_user = signer.pubkey().to_base58();
        let issued_at = 1_700_000_000u64;
        let expires_at = issued_at + 600;
        let message = bridge_access_message(&signer_user, issued_at, expires_at);
        let auth = parse_bridge_access_auth(&serde_json::json!({
            "issued_at": issued_at,
            "expires_at": expires_at,
            "signature": pq_signature_json(&signer.sign(&message)),
        }))
        .expect("parse bridge auth");

        let err = verify_bridge_access_auth_at(&other_user, &auth, issued_at + 60)
            .expect_err("bridge auth must be bound to the requesting user");
        assert!(err.message.contains("Invalid bridge auth signature"));
    }

    #[test]
    fn test_bridge_recipient_history_proxy_is_not_public() {
        let source = include_str!("lib.rs");
        assert!(
            !source.contains("\"getBridgeDepositsByRecipient\" =>"),
            "REGRESSION P0-4: getBridgeDepositsByRecipient must not remain publicly exposed via RPC"
        );
    }

    #[test]
    fn test_parse_governance_event_maps_system_event() {
        let mut data = HashMap::new();
        let target_contract = lichen_core::Pubkey([0x77u8; 32]);
        data.insert("proposal_id".to_string(), "7".to_string());
        data.insert("action".to_string(), "contract_call".to_string());
        data.insert(
            "authority".to_string(),
            lichen_core::Pubkey([0x11u8; 32]).to_base58(),
        );
        data.insert(
            "proposer".to_string(),
            lichen_core::Pubkey([0x22u8; 32]).to_base58(),
        );
        data.insert(
            "actor".to_string(),
            lichen_core::Pubkey([0x33u8; 32]).to_base58(),
        );
        data.insert("approvals".to_string(), "2".to_string());
        data.insert("threshold".to_string(), "2".to_string());
        data.insert("execute_after_epoch".to_string(), "9".to_string());
        data.insert("executed".to_string(), "true".to_string());
        data.insert("cancelled".to_string(), "false".to_string());
        data.insert(
            "metadata".to_string(),
            format!(
                "contract={} function=pause args_len=0 value_spores=0",
                target_contract.to_base58()
            ),
        );
        data.insert("target_contract".to_string(), target_contract.to_base58());
        data.insert("target_function".to_string(), "pause".to_string());
        data.insert("call_args_len".to_string(), "0".to_string());
        data.insert("call_value_spores".to_string(), "0".to_string());

        let event = ContractEvent {
            program: SYSTEM_PROGRAM_ID,
            name: "GovernanceProposalExecuted".to_string(),
            data,
            slot: 42,
        };

        let parsed = parse_governance_event(&event).expect("system governance event should parse");
        assert_eq!(parsed.proposal_id, 7);
        assert_eq!(parsed.event_kind, "executed");
        assert_eq!(parsed.action, "contract_call");
        assert_eq!(parsed.approvals, 2);
        assert!(parsed.executed);
        assert!(!parsed.cancelled);
        assert_eq!(parsed.target_contract, Some(target_contract));
        assert_eq!(parsed.target_function.as_deref(), Some("pause"));
        assert_eq!(parsed.call_args_len, Some(0));
        assert_eq!(parsed.call_value_spores, Some(0));
    }

    #[tokio::test]
    async fn test_get_governance_events_returns_structured_results() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let rpc_state = make_test_rpc_state(state.clone());

        let mut data = HashMap::new();
        let target_contract = lichen_core::Pubkey([0x77u8; 32]);
        data.insert("proposal_id".to_string(), "9".to_string());
        data.insert("action".to_string(), "contract_call".to_string());
        data.insert(
            "authority".to_string(),
            lichen_core::Pubkey([0x44u8; 32]).to_base58(),
        );
        data.insert(
            "proposer".to_string(),
            lichen_core::Pubkey([0x55u8; 32]).to_base58(),
        );
        data.insert(
            "actor".to_string(),
            lichen_core::Pubkey([0x66u8; 32]).to_base58(),
        );
        data.insert("approvals".to_string(), "3".to_string());
        data.insert("threshold".to_string(), "2".to_string());
        data.insert("execute_after_epoch".to_string(), "11".to_string());
        data.insert("executed".to_string(), "true".to_string());
        data.insert("cancelled".to_string(), "false".to_string());
        data.insert(
            "metadata".to_string(),
            format!(
                "contract={} function=mb_pause args_len=0 value_spores=0",
                target_contract.to_base58()
            ),
        );
        data.insert("target_contract".to_string(), target_contract.to_base58());
        data.insert("target_function".to_string(), "mb_pause".to_string());
        data.insert("call_args_len".to_string(), "0".to_string());
        data.insert("call_value_spores".to_string(), "0".to_string());

        state
            .put_contract_event(
                &SYSTEM_PROGRAM_ID,
                &ContractEvent {
                    program: SYSTEM_PROGRAM_ID,
                    name: "GovernanceProposalExecuted".to_string(),
                    data,
                    slot: 88,
                },
            )
            .unwrap();

        let result = handle_get_governance_events(&rpc_state, None)
            .await
            .expect("governance events RPC should succeed");

        assert_eq!(result["count"], 1);
        assert_eq!(result["events"][0]["proposal_id"], 9);
        assert_eq!(result["events"][0]["kind"], "executed");
        assert_eq!(result["events"][0]["action"], "contract_call");
        assert_eq!(result["events"][0]["approvals"], 3);
        assert_eq!(
            result["events"][0]["target_contract"],
            target_contract.to_base58()
        );
        assert_eq!(result["events"][0]["target_function"], "mb_pause");
        assert_eq!(result["events"][0]["call_args_len"], 0);
        assert_eq!(result["events"][0]["call_value_spores"], 0);
        assert_eq!(result["events"][0]["slot"], 88);
    }

    #[tokio::test]
    async fn test_get_incident_status_returns_default_operational_manifest() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let rpc_state = make_test_rpc_state(state);

        let result = handle_get_incident_status(&rpc_state)
            .await
            .expect("default incident status should serialize");

        assert_eq!(result["mode"], "normal");
        assert_eq!(result["severity"], "info");
        assert_eq!(result["banner_enabled"], false);
        assert_eq!(result["source"], "default");
        assert_eq!(result["components"]["wallet"]["status"], "operational");
    }

    #[tokio::test]
    async fn test_get_incident_status_reads_manifest_file() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let status_path = tmp.path().join("incident-status.json");
        std::fs::write(
            &status_path,
            serde_json::json!({
                "updated_at": "2026-04-03T12:00:00Z",
                "active_since": "2026-04-03T11:45:00Z",
                "mode": "deposit_only_freeze",
                "severity": "warning",
                "banner_enabled": true,
                "headline": "Deposits temporarily paused",
                "summary": "Inbound deposits are paused while withdrawals and local wallet access remain available.",
                "customer_message": "Please wait for an operator notice before sending new bridge or custody deposits.",
                "actions": ["Do not initiate new deposits until the banner clears."],
                "components": {
                    "deposits": {
                        "status": "paused",
                        "message": "New deposits are paused while withdrawals remain available."
                    },
                    "wallet": {
                        "status": "operational",
                        "message": "Local wallet access remains available."
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.incident_status_path = Some(status_path);

        let result = handle_get_incident_status(&rpc_state)
            .await
            .expect("file-backed incident status should serialize");

        assert_eq!(result["mode"], "deposit_only_freeze");
        assert_eq!(result["severity"], "warning");
        assert_eq!(result["banner_enabled"], true);
        assert_eq!(result["source"], "file");
        assert_eq!(result["components"]["deposits"]["status"], "paused");
        assert_eq!(result["components"]["bridge"]["status"], "operational");
        assert_eq!(
            result["customer_message"],
            "Please wait for an operator notice before sending new bridge or custody deposits."
        );
    }

    #[tokio::test]
    async fn test_get_incident_status_preserves_contract_enforcement_metadata() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let status_path = tmp.path().join("incident-status.json");
        std::fs::write(
            &status_path,
            serde_json::json!({
                "mode": "contract_circuit_breaker",
                "severity": "warning",
                "banner_enabled": true,
                "headline": "SporeSwap Router is in circuit-breaker mode",
                "summary": "SporeSwap Router is temporarily restricted while operators verify abnormal behavior.",
                "customer_message": "Only the affected contract flow is restricted.",
                "enforcement": {
                    "mode": "incident_guardian_allowlisted_pause",
                    "contract_targets": [
                        {
                            "id": "dexrouter",
                            "symbol": "DEXROUTER",
                            "display_name": "SporeSwap Router",
                            "pause_function": "emergency_pause"
                        }
                    ]
                },
                "components": {
                    "contracts": {
                        "status": "degraded",
                        "message": "SporeSwap Router is under an active contract-specific circuit breaker."
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.incident_status_path = Some(status_path);

        let result = handle_get_incident_status(&rpc_state)
            .await
            .expect("contract enforcement metadata should serialize");

        assert_eq!(
            result["enforcement"]["mode"],
            "incident_guardian_allowlisted_pause"
        );
        assert_eq!(
            result["enforcement"]["contract_targets"][0]["symbol"],
            "DEXROUTER"
        );
        assert_eq!(
            result["enforcement"]["contract_targets"][0]["pause_function"],
            "emergency_pause"
        );
    }

    #[tokio::test]
    async fn test_get_incident_status_warns_when_manifest_is_invalid() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let status_path = tmp.path().join("incident-status.json");
        std::fs::write(&status_path, "{ invalid json").unwrap();

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.incident_status_path = Some(status_path);

        let result = handle_get_incident_status(&rpc_state)
            .await
            .expect("invalid manifest should still yield a warning banner");

        assert_eq!(result["mode"], "status_feed_error");
        assert_eq!(result["severity"], "warning");
        assert_eq!(result["banner_enabled"], true);
        assert_eq!(result["source"], "error");
        assert_eq!(result["components"]["deposits"]["status"], "unknown");
    }

    #[tokio::test]
    async fn test_get_signed_metadata_manifest_requires_configured_file() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let rpc_state = make_test_rpc_state(state);

        let error = handle_get_signed_metadata_manifest(&rpc_state)
            .await
            .expect_err("missing metadata manifest should fail closed");

        assert_eq!(error.code, -32000);
        assert_eq!(
            error.message,
            "Signed metadata manifest is not configured on this RPC node"
        );
    }

    #[tokio::test]
    async fn test_get_signed_metadata_manifest_reads_manifest_file() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let manifest_path = tmp.path().join("signed-metadata-manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::json!({
                "schema_version": 1,
                "manifest_type": "signed_metadata",
                "payload": {
                    "schema_version": 1,
                    "network": "local-testnet",
                    "generated_at": "2026-04-03T12:00:00Z",
                    "symbol_registry": [
                        {
                            "symbol": "DEX",
                            "program": "11111111111111111111111111111112",
                            "owner": "11111111111111111111111111111111",
                            "name": "SporeSwap Core",
                            "template": "dex",
                            "metadata": {
                                "icon_class": "fa-chart-line"
                            },
                            "decimals": null
                        }
                    ]
                },
                "signature": {
                    "scheme_version": 1,
                    "public_key": {
                        "scheme_version": 1,
                        "bytes": "deadbeef"
                    },
                    "sig": "deadbeef"
                }
            })
            .to_string(),
        )
        .unwrap();

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.signed_metadata_manifest_path = Some(manifest_path);

        let result = handle_get_signed_metadata_manifest(&rpc_state)
            .await
            .expect("file-backed signed metadata manifest should serialize");

        assert_eq!(result["manifest_type"], "signed_metadata");
        assert_eq!(result["payload"]["network"], "local-testnet");
        assert_eq!(result["payload"]["symbol_registry"][0]["symbol"], "DEX");
        assert_eq!(result["signature"]["scheme_version"], 1);
    }

    #[tokio::test]
    async fn test_get_signed_metadata_manifest_generates_live_manifest_and_refreshes_file() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let manifest_path = tmp.path().join("signed-metadata-manifest.json");
        let keypair_path = tmp.path().join("release-signing-keypair.json");
        let signing_keypair = LichenKeypair::from_seed(&[42u8; 32]);

        std::fs::write(
            &keypair_path,
            serde_json::to_string_pretty(&KeypairFile::from_keypair(&signing_keypair)).unwrap(),
        )
        .unwrap();

        state
            .register_symbol(
                "DEX",
                SymbolRegistryEntry {
                    symbol: "DEX".to_string(),
                    program: Pubkey([2u8; 32]),
                    owner: Pubkey([3u8; 32]),
                    name: Some("SporeSwap Core".to_string()),
                    template: Some("dex".to_string()),
                    metadata: Some(serde_json::json!({ "icon_class": "fa-chart-line" })),
                    decimals: None,
                },
            )
            .unwrap();

        let mut rpc_state = make_test_rpc_state(state.clone());
        rpc_state.signed_metadata_manifest_path = Some(manifest_path.clone());
        rpc_state.signed_metadata_keypair_path = Some(keypair_path.clone());

        let first = handle_get_signed_metadata_manifest(&rpc_state)
            .await
            .expect("live manifest should be generated from state");

        assert_eq!(first["manifest_type"], "signed_metadata");
        assert_eq!(
            first["payload"]["symbol_registry"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            first["payload"]["source_rpc"],
            live_signed_metadata_source_rpc()
        );
        assert_eq!(first["signer"], signing_keypair.pubkey().to_base58());
        assert!(manifest_path.exists());

        state
            .register_symbol(
                "YID",
                SymbolRegistryEntry {
                    symbol: "YID".to_string(),
                    program: Pubkey([4u8; 32]),
                    owner: Pubkey([5u8; 32]),
                    name: Some("LichenID".to_string()),
                    template: Some("identity".to_string()),
                    metadata: Some(serde_json::json!({ "icon_class": "fa-id-card" })),
                    decimals: None,
                },
            )
            .unwrap();

        let second = handle_get_signed_metadata_manifest(&rpc_state)
            .await
            .expect("live manifest should refresh after registry changes");

        let symbols: Vec<String> = second["payload"]["symbol_registry"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry.get("symbol").and_then(serde_json::Value::as_str))
            .map(ToOwned::to_owned)
            .collect();
        assert_eq!(symbols, vec!["DEX".to_string(), "YID".to_string()]);

        let registry = handle_get_all_symbol_registry(&rpc_state, None)
            .await
            .expect("registry list should serialize");
        let registry_symbols: Vec<String> = registry["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry.get("symbol").and_then(serde_json::Value::as_str))
            .map(ToOwned::to_owned)
            .collect();
        assert_eq!(registry_symbols, symbols);

        let persisted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(
            persisted["payload"]["symbol_registry"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn test_get_signed_metadata_manifest_generates_live_manifest_without_file_path() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let keypair_path = tmp.path().join("release-signing-keypair.json");
        let signing_keypair = LichenKeypair::from_seed(&[7u8; 32]);

        std::fs::write(
            &keypair_path,
            serde_json::to_string_pretty(&KeypairFile::from_keypair(&signing_keypair)).unwrap(),
        )
        .unwrap();

        state
            .register_symbol(
                "LUSD",
                SymbolRegistryEntry {
                    symbol: "LUSD".to_string(),
                    program: Pubkey([9u8; 32]),
                    owner: Pubkey([10u8; 32]),
                    name: Some("Licn USD".to_string()),
                    template: Some("token".to_string()),
                    metadata: Some(
                        serde_json::json!({ "logo_url": "https://example.invalid/lusd.png" }),
                    ),
                    decimals: Some(9),
                },
            )
            .unwrap();

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.signed_metadata_keypair_path = Some(keypair_path);

        let manifest = handle_get_signed_metadata_manifest(&rpc_state)
            .await
            .expect("live manifest should be served without a configured file path");

        assert_eq!(manifest["payload"]["symbol_registry"][0]["symbol"], "LUSD");
        assert_eq!(manifest["signer"], signing_keypair.pubkey().to_base58());
    }

    #[tokio::test]
    async fn test_get_signed_metadata_manifest_accepts_seed_only_signing_key_file() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let manifest_path = tmp.path().join("signed-metadata-manifest.json");
        let keypair_path = tmp.path().join("release-signing-keypair.json");
        let signing_keypair = LichenKeypair::from_seed(&[11u8; 32]);

        std::fs::write(
            &keypair_path,
            serde_json::json!({
                "description": "ML-DSA-65 release signing seed for the Lichen auto-updater",
                "usage": "Sign SHA256SUMS files for GitHub releases. The public key is embedded in validator/src/updater.rs",
                "warning": "KEEP THIS FILE SECURE — it controls binary update integrity",
                "privateKey": signing_keypair.to_seed(),
            })
            .to_string(),
        )
        .unwrap();

        state
            .register_symbol(
                "LSEED",
                SymbolRegistryEntry {
                    symbol: "LSEED".to_string(),
                    program: Pubkey([21u8; 32]),
                    owner: Pubkey([22u8; 32]),
                    name: Some("Legacy Seed Token".to_string()),
                    template: Some("token".to_string()),
                    metadata: Some(serde_json::json!({ "icon_class": "fa-seedling" })),
                    decimals: Some(9),
                },
            )
            .unwrap();

        let mut rpc_state = make_test_rpc_state(state.clone());
        rpc_state.signed_metadata_manifest_path = Some(manifest_path.clone());
        rpc_state.signed_metadata_keypair_path = Some(keypair_path.clone());

        let first = handle_get_signed_metadata_manifest(&rpc_state)
            .await
            .expect("seed-only signing key should generate a live manifest");

        assert_eq!(first["signer"], signing_keypair.pubkey().to_base58());
        assert_eq!(first["payload"]["symbol_registry"][0]["symbol"], "LSEED");

        state
            .register_symbol(
                "LSEED2",
                SymbolRegistryEntry {
                    symbol: "LSEED2".to_string(),
                    program: Pubkey([23u8; 32]),
                    owner: Pubkey([24u8; 32]),
                    name: Some("Legacy Seed Token 2".to_string()),
                    template: Some("token".to_string()),
                    metadata: Some(serde_json::json!({ "icon_class": "fa-seedling" })),
                    decimals: Some(9),
                },
            )
            .unwrap();

        let second = handle_get_signed_metadata_manifest(&rpc_state)
            .await
            .expect("seed-only signing key should refresh the live manifest");

        let symbols: Vec<String> = second["payload"]["symbol_registry"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry.get("symbol").and_then(serde_json::Value::as_str))
            .map(ToOwned::to_owned)
            .collect();
        assert_eq!(symbols, vec!["LSEED".to_string(), "LSEED2".to_string()]);

        let persisted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        assert_eq!(
            persisted["payload"]["symbol_registry"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn test_get_signed_metadata_manifest_rejects_invalid_json() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let manifest_path = tmp.path().join("signed-metadata-manifest.json");
        std::fs::write(&manifest_path, "{ invalid json").unwrap();

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.signed_metadata_manifest_path = Some(manifest_path.clone());

        let error = handle_get_signed_metadata_manifest(&rpc_state)
            .await
            .expect_err("invalid signed metadata manifest should fail closed");

        assert_eq!(error.code, -32000);
        assert!(error
            .message
            .contains("Failed to parse signed metadata manifest"));
        assert!(error.message.contains(&manifest_path.display().to_string()));
    }

    #[tokio::test]
    async fn test_get_service_fleet_status_returns_probe_error_without_config() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let rpc_state = make_test_rpc_state(state);

        let result = handle_get_service_fleet_status(&rpc_state)
            .await
            .expect("service fleet status should serialize without config");

        assert_eq!(result["state"], "probe_error");
        assert_eq!(result["source"], "error");
        assert_eq!(result["summary"]["degraded_services"], 1);
    }

    #[tokio::test]
    async fn test_get_service_fleet_status_probes_services_and_tracks_absence() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();

        let healthy_url = spawn_mock_server(Router::new().route(
            "/health",
            axum::routing::get(|| async { Json(serde_json::json!({ "status": "ok" })) }),
        ))
        .await;
        let unhealthy_url = spawn_mock_server(Router::new().route(
            "/health",
            axum::routing::get(|| async {
                (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "status": "down" })),
                )
            }),
        ))
        .await;

        let config_path = tmp.path().join("service-fleet.json");
        let status_path = tmp.path().join("service-fleet-status.json");
        std::fs::write(
            &config_path,
            serde_json::json!({
                "schema_version": 1,
                "network": "local-testnet",
                "probe_timeout_ms": 1500,
                "hosts": [
                    {
                        "id": "local",
                        "label": "Local Host",
                        "services": [
                            {
                                "id": "custody",
                                "label": "Custody",
                                "service": "custody",
                                "probe": {
                                    "kind": "http",
                                    "url": format!("{}/health", healthy_url),
                                    "body_contains_any": ["\"status\": \"ok\"", "\"status\":\"ok\""]
                                }
                            },
                            {
                                "id": "faucet",
                                "label": "Faucet",
                                "service": "faucet",
                                "probe": {
                                    "kind": "http",
                                    "url": format!("{}/health", unhealthy_url),
                                    "body_contains_any": ["OK"]
                                }
                            },
                            {
                                "id": "custody-eu",
                                "label": "Custody EU",
                                "service": "custody",
                                "expected": false,
                                "intentionally_absent_message": "Custody is only deployed on the US footprint."
                            }
                        ]
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.service_fleet_config_path = Some(config_path);
        rpc_state.service_fleet_status_path = Some(status_path.clone());

        let result = handle_get_service_fleet_status(&rpc_state)
            .await
            .expect("service fleet status should serialize");

        assert_eq!(result["state"], "degraded");
        assert_eq!(result["summary"]["host_count"], 1);
        assert_eq!(result["summary"]["healthy_services"], 1);
        assert_eq!(result["summary"]["degraded_services"], 1);
        assert_eq!(result["summary"]["intentionally_absent_services"], 1);
        assert_eq!(result["hosts"][0]["services"][0]["state"], "healthy");
        assert_eq!(result["hosts"][0]["services"][1]["state"], "degraded");
        assert_eq!(result["hosts"][0]["services"][2]["state"], "absent");

        let persisted: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(status_path).expect("read persisted service fleet status"),
        )
        .expect("decode persisted service fleet status");
        assert_eq!(persisted["summary"]["healthy_services"], 1);
    }

    #[tokio::test]
    async fn test_get_service_fleet_status_preserves_last_success_timestamp() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let config_path = tmp.path().join("service-fleet.json");
        let status_path = tmp.path().join("service-fleet-status.json");

        std::fs::write(
            &config_path,
            serde_json::json!({
                "schema_version": 1,
                "network": "local-testnet",
                "hosts": [
                    {
                        "id": "local",
                        "label": "Local Host",
                        "services": [
                            {
                                "id": "custody",
                                "label": "Custody",
                                "service": "custody",
                                "probe": {
                                    "kind": "http",
                                    "url": "http://127.0.0.1:9/health",
                                    "body_contains_any": ["OK"]
                                }
                            }
                        ]
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            &status_path,
            serde_json::json!({
                "schema_version": 1,
                "source": "probe",
                "network": "local-testnet",
                "state": "healthy",
                "updated_at": 123456789,
                "summary": {
                    "host_count": 1,
                    "total_services": 1,
                    "healthy_services": 1,
                    "degraded_services": 0,
                    "intentionally_absent_services": 0,
                    "last_success_at": 123456789
                },
                "hosts": [
                    {
                        "id": "local",
                        "label": "Local Host",
                        "healthy_services": 1,
                        "degraded_services": 0,
                        "intentionally_absent_services": 0,
                        "services": [
                            {
                                "id": "custody",
                                "label": "Custody",
                                "service": "custody",
                                "host_id": "local",
                                "host_label": "Local Host",
                                "expected": true,
                                "intentionally_absent": false,
                                "state": "healthy",
                                "message": "HTTP health probe passed.",
                                "kind": "http",
                                "url": "http://127.0.0.1:9/health",
                                "last_checked_at": 123456789,
                                "last_success_at": 123456789,
                                "latency_ms": 5
                            }
                        ]
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.service_fleet_config_path = Some(config_path);
        rpc_state.service_fleet_status_path = Some(status_path);

        let result = handle_get_service_fleet_status(&rpc_state)
            .await
            .expect("service fleet status should serialize on failures");

        assert_eq!(result["state"], "degraded");
        assert_eq!(result["hosts"][0]["services"][0]["state"], "degraded");
        assert_eq!(
            result["hosts"][0]["services"][0]["last_success_at"],
            123456789
        );
    }

    #[tokio::test]
    async fn test_get_service_fleet_status_uses_upstream_rpc_when_configured() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();

        let upstream_url = spawn_mock_server(Router::new().route(
            "/",
            axum::routing::post(|| async {
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "schema_version": 1,
                        "source": "probe",
                        "network": "testnet",
                        "state": "healthy",
                        "updated_at": 123456789,
                        "probe_timeout_ms": 3000,
                        "summary": {
                            "host_count": 1,
                            "total_services": 1,
                            "healthy_services": 1,
                            "degraded_services": 0,
                            "intentionally_absent_services": 0,
                            "last_success_at": 123456789
                        },
                        "hosts": [
                            {
                                "id": "us",
                                "label": "US VPS",
                                "healthy_services": 1,
                                "degraded_services": 0,
                                "intentionally_absent_services": 0,
                                "services": [
                                    {
                                        "id": "custody",
                                        "label": "Custody",
                                        "service": "custody",
                                        "host_id": "us",
                                        "host_label": "US VPS",
                                        "expected": true,
                                        "intentionally_absent": false,
                                        "state": "healthy",
                                        "message": "HTTP health probe passed.",
                                        "kind": "http",
                                        "url": "http://127.0.0.1:9105/health",
                                        "last_checked_at": 123456789,
                                        "last_success_at": 123456789,
                                        "latency_ms": 5
                                    }
                                ]
                            }
                        ]
                    }
                }))
            }),
        ))
        .await;

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.service_fleet_upstream_rpc_url = Some(upstream_url);

        let result = handle_get_service_fleet_status(&rpc_state)
            .await
            .expect("service fleet status should serialize when upstream is configured");

        assert_eq!(result["source"], "upstream");
        assert_eq!(result["state"], "healthy");
        assert_eq!(result["summary"]["healthy_services"], 1);
        assert_eq!(result["hosts"][0]["services"][0]["state"], "healthy");
    }

    #[tokio::test]
    async fn test_create_bridge_deposit_blocked_when_deposits_are_paused() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let status_path = tmp.path().join("incident-status.json");
        std::fs::write(
            &status_path,
            serde_json::json!({
                "mode": "deposit_guard",
                "components": {
                    "deposits": {
                        "status": "paused"
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.custody_url = Some("http://127.0.0.1:9".to_string());
        rpc_state.custody_auth_token = Some("test-auth-token".to_string());
        rpc_state.incident_status_path = Some(status_path);

        let error = handle_create_bridge_deposit(
            &rpc_state,
            Some(serde_json::json!([signed_bridge_deposit_payload(
                17, "solana", "sol"
            )])),
        )
        .await
        .expect_err("bridge deposit creation must be blocked while deposits are paused");

        assert_eq!(error.code, -32000);
        assert_eq!(
            error.message,
            "new deposits are temporarily paused while operators verify inbound activity"
        );
    }

    #[tokio::test]
    async fn test_create_bridge_deposit_forwards_bridge_auth_to_custody() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();

        let custody_state = MockCustodyState::default();
        let custody_url = spawn_mock_server(
            Router::new()
                .route("/deposits", post(mock_custody_create_deposit))
                .with_state(custody_state.clone()),
        )
        .await;

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.custody_url = Some(custody_url);
        rpc_state.custody_auth_token = Some("test-auth-token".to_string());

        let payload = signed_bridge_deposit_payload(29, "solana", "sol");
        let expected_auth = payload
            .get("auth")
            .cloned()
            .expect("bridge auth payload should exist");

        let response = handle_create_bridge_deposit(&rpc_state, Some(serde_json::json!([payload])))
            .await
            .expect("bridge deposit creation should succeed");

        assert_eq!(
            response["deposit_id"],
            "11111111-1111-1111-1111-111111111111"
        );

        let requests = custody_state.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["auth"], expected_auth);
    }

    #[test]
    fn test_create_bridge_deposit_emits_privileged_audit_log() {
        let logs = capture_logs_async(async {
            let tmp = tempdir().unwrap();
            let state = StateStore::open(tmp.path()).unwrap();

            let custody_state = MockCustodyState::default();
            let custody_url = spawn_mock_server(
                Router::new()
                    .route("/deposits", post(mock_custody_create_deposit))
                    .with_state(custody_state.clone()),
            )
            .await;

            let mut rpc_state = make_test_rpc_state(state);
            rpc_state.custody_url = Some(custody_url);
            rpc_state.custody_auth_token = Some("test-auth-token".to_string());

            let payload = signed_bridge_deposit_payload(41, "solana", "sol");
            let user_id = payload["user_id"]
                .as_str()
                .expect("bridge payload user_id should exist")
                .to_string();

            let response =
                handle_create_bridge_deposit(&rpc_state, Some(serde_json::json!([payload])))
                    .await
                    .expect("bridge deposit creation should succeed");

            assert_eq!(
                response["deposit_id"],
                "11111111-1111-1111-1111-111111111111"
            );
            assert_eq!(response["address"], "mock-bridge-address");
            assert!(!user_id.is_empty());
        });

        assert!(
            logs.contains("Privileged RPC mutation executed"),
            "captured logs: {logs}"
        );
        assert!(
            logs.contains("createBridgeDeposit"),
            "captured logs: {logs}"
        );
        assert!(logs.contains("bridge_deposit"), "captured logs: {logs}");
        assert!(
            logs.contains("11111111-1111-1111-1111-111111111111"),
            "captured logs: {logs}"
        );
    }

    #[tokio::test]
    async fn test_create_bridge_deposit_surfaces_custody_http_errors() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();

        let custody_state = MockCustodyState::default();
        let custody_url = spawn_mock_server(
            Router::new()
                .route("/deposits", post(mock_custody_create_deposit_rate_limited))
                .with_state(custody_state.clone()),
        )
        .await;

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.custody_url = Some(custody_url);
        rpc_state.custody_auth_token = Some("test-auth-token".to_string());

        let error = handle_create_bridge_deposit(
            &rpc_state,
            Some(serde_json::json!([signed_bridge_deposit_payload(
                33, "solana", "sol"
            )])),
        )
        .await
        .expect_err("custody HTTP rate-limit errors must surface as RPC errors");

        assert_eq!(error.code, -32000);
        assert_eq!(
            error.message,
            "rate_limited: wait 10s between deposit requests"
        );

        let requests = custody_state.requests.lock().await;
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn test_create_bridge_deposit_surfaces_custody_replay_errors() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();

        let custody_state = MockCustodyState::default();
        let custody_url = spawn_mock_server(
            Router::new()
                .route("/deposits", post(mock_custody_create_deposit_replayed_auth))
                .with_state(custody_state.clone()),
        )
        .await;

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.custody_url = Some(custody_url);
        rpc_state.custody_auth_token = Some("test-auth-token".to_string());

        let error = handle_create_bridge_deposit(
            &rpc_state,
            Some(serde_json::json!([signed_bridge_deposit_payload(
                34, "solana", "sol"
            )])),
        )
        .await
        .expect_err("custody replay errors must surface as RPC errors");

        assert_eq!(error.code, -32000);
        assert_eq!(
            error.message,
            "bridge auth already used for a different deposit request; sign a new bridge authorization"
        );

        let requests = custody_state.requests.lock().await;
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn test_get_bridge_deposit_forwards_bridge_auth_to_custody() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();

        let custody_state = MockCustodyState::default();
        let custody_url = spawn_mock_server(
            Router::new()
                .route(
                    "/deposits/:deposit_id",
                    axum::routing::get(mock_custody_get_deposit),
                )
                .with_state(custody_state.clone()),
        )
        .await;

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.custody_url = Some(custody_url);
        rpc_state.custody_auth_token = Some("test-auth-token".to_string());

        let payload = signed_bridge_deposit_payload(31, "solana", "sol");
        let user_id = payload["user_id"]
            .as_str()
            .expect("bridge payload user_id should exist")
            .to_string();
        let auth = payload
            .get("auth")
            .cloned()
            .expect("bridge auth payload should exist");

        let response = handle_get_bridge_deposit(
            &rpc_state,
            Some(serde_json::json!({
                "deposit_id": "11111111-1111-1111-1111-111111111111",
                "user_id": user_id.clone(),
                "auth": auth.clone(),
            })),
        )
        .await
        .expect("bridge deposit lookup should succeed");

        assert_eq!(response["user_id"], user_id);

        let lookups = custody_state.lookups.lock().await;
        assert_eq!(lookups.len(), 1);
        assert_eq!(lookups[0]["query"]["user_id"], user_id);
        let forwarded_auth = lookups[0]["query"]["auth"]
            .as_str()
            .expect("forwarded auth query should be a JSON string");
        let parsed_auth: serde_json::Value =
            serde_json::from_str(forwarded_auth).expect("decode forwarded bridge auth");
        assert_eq!(parsed_auth, auth);
    }

    #[tokio::test]
    async fn test_get_bridge_deposit_accepts_single_object_array_params() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();

        let custody_state = MockCustodyState::default();
        let custody_url = spawn_mock_server(
            Router::new()
                .route(
                    "/deposits/:deposit_id",
                    axum::routing::get(mock_custody_get_deposit),
                )
                .with_state(custody_state.clone()),
        )
        .await;

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.custody_url = Some(custody_url);
        rpc_state.custody_auth_token = Some("test-auth-token".to_string());

        let payload = signed_bridge_deposit_payload(32, "solana", "sol");
        let user_id = payload["user_id"]
            .as_str()
            .expect("bridge payload user_id should exist")
            .to_string();
        let auth = payload
            .get("auth")
            .cloned()
            .expect("bridge auth payload should exist");

        let response = handle_get_bridge_deposit(
            &rpc_state,
            Some(serde_json::json!([{
                "deposit_id": "11111111-1111-1111-1111-111111111111",
                "user_id": user_id.clone(),
                "auth": auth.clone(),
            }])),
        )
        .await
        .expect("single-object array params should succeed");

        assert_eq!(response["user_id"], user_id);

        let lookups = custody_state.lookups.lock().await;
        assert_eq!(lookups.len(), 1);
        assert_eq!(lookups[0]["query"]["user_id"], user_id);
        let forwarded_auth = lookups[0]["query"]["auth"]
            .as_str()
            .expect("forwarded auth query should be a JSON string");
        let parsed_auth: serde_json::Value =
            serde_json::from_str(forwarded_auth).expect("decode forwarded bridge auth");
        assert_eq!(parsed_auth, auth);
    }

    #[tokio::test]
    async fn test_create_bridge_deposit_rpc_route_blocked_when_deposits_are_paused() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let status_path = tmp.path().join("incident-status.json");
        std::fs::write(
            &status_path,
            serde_json::json!({
                "mode": "deposit_guard",
                "components": {
                    "deposits": {
                        "status": "paused"
                    }
                }
            })
            .to_string(),
        )
        .unwrap();

        let custody_state = MockCustodyState::default();
        let custody_url = spawn_mock_server(
            Router::new()
                .route("/deposits", post(mock_custody_create_deposit))
                .with_state(custody_state.clone()),
        )
        .await;

        let mut rpc_state = make_test_rpc_state(state);
        rpc_state.custody_url = Some(custody_url);
        rpc_state.custody_auth_token = Some("test-auth-token".to_string());
        rpc_state.incident_status_path = Some(status_path);

        let rpc_url = spawn_mock_server(
            Router::new()
                .route("/", post(super::handle_rpc))
                .with_state(Arc::new(rpc_state)),
        )
        .await;

        let response: serde_json::Value = reqwest::Client::new()
            .post(format!("{}/", rpc_url))
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "createBridgeDeposit",
                "params": [signed_bridge_deposit_payload(23, "solana", "sol")]
            }))
            .send()
            .await
            .expect("send RPC request")
            .json()
            .await
            .expect("parse RPC response");

        assert_eq!(response["error"]["code"], -32000);
        assert_eq!(
            response["error"]["message"],
            "new deposits are temporarily paused while operators verify inbound activity"
        );
        assert!(custody_state.requests.lock().await.is_empty());
    }

    // ── AUDIT-FIX A11-01: eth_gasPrice must return 1, not base_fee ──

    #[test]
    fn test_a11_01_gas_price_returns_one() {
        // Verify the source code of handle_eth_gas_price returns "0x1"
        // (This is a source-level check since integration tests are complex)
        let source = include_str!("lib.rs");
        let fn_start = source
            .find("async fn handle_eth_gas_price")
            .expect("fn not found");
        let fn_body = &source[fn_start..fn_start + 600];

        // Must contain "0x1" as the return value
        assert!(
            fn_body.contains("\"0x1\""),
            "REGRESSION A11-01: eth_gasPrice must return \"0x1\" (1 spore per gas unit), \
             not base_fee. MetaMask computes total = gasPrice × estimateGas."
        );
        // Must NOT contain "fee_config.base_fee" in the function body
        assert!(
            !fn_body.contains("fee_config.base_fee"),
            "REGRESSION A11-01: eth_gasPrice must NOT return fee_config.base_fee"
        );
    }

    // ── AUDIT-FIX A11-02: eth_getLogs must use Keccak-256 for topic hashing ──

    #[test]
    fn test_a11_02_get_logs_uses_keccak256() {
        // Verify the topic hashing code uses sha3::Keccak256, not sha2::Sha256
        let source = include_str!("lib.rs");
        let fn_start = source
            .find("async fn handle_eth_get_logs")
            .expect("fn not found");
        // Task 3.4: Increased scan window from 5000 to 10000 to cover
        // both EVM log section and native event section of the expanded function.
        let fn_body = &source[fn_start..std::cmp::min(fn_start + 10000, source.len())];

        // Must use Keccak256
        assert!(
            fn_body.contains("Keccak256"),
            "REGRESSION A11-02: eth_getLogs topic hashing must use Keccak256, not SHA-256. \
             EVM tooling (Ethers.js, web3.py) uses keccak256 for event topic matching."
        );
        // Must NOT use sha2 or Sha256 for topic hashing
        assert!(
            !fn_body.contains("Sha256"),
            "REGRESSION A11-02: eth_getLogs must NOT use Sha256 for topic hashing"
        );
    }

    #[test]
    fn test_a11_02_keccak256_produces_correct_hash() {
        // Verify Keccak-256 of "Transfer(address,address,uint256)" matches known EVM value
        use sha3::{Digest, Keccak256};
        let mut hasher = Keccak256::new();
        hasher.update(b"Transfer(address,address,uint256)");
        let result = hasher.finalize();
        let hex_str = hex::encode(result);
        // Standard EVM Transfer topic: 0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef
        assert_eq!(
            hex_str,
            "ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
            "REGRESSION A11-02: Keccak-256 of ERC-20 Transfer event must match standard EVM topic hash"
        );
    }

    // ── P2-4: Binary RPC format tests ──

    #[test]
    fn test_encode_rpc_response_default_json() {
        let resp = RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            result: Some(serde_json::json!({"slot": 42})),
            error: None,
        };
        let headers = HeaderMap::new();
        let response = encode_rpc_response(&headers, resp);
        // Default should be JSON content type
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(ct.contains("json"), "default should be JSON, got: {}", ct);
    }

    #[test]
    fn test_encode_rpc_response_msgpack() {
        let resp = RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            result: Some(serde_json::json!({"balance": 1000})),
            error: None,
        };
        let mut headers = HeaderMap::new();
        headers.insert("accept", "application/msgpack".parse().unwrap());
        let response = encode_rpc_response(&headers, resp);
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(ct, "application/msgpack");
    }

    #[test]
    fn test_encode_rpc_response_bincode() {
        let resp = RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            result: Some(serde_json::json!("hello")),
            error: None,
        };
        let mut headers = HeaderMap::new();
        headers.insert("accept", "application/octet-stream".parse().unwrap());
        let response = encode_rpc_response(&headers, resp);
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(ct, "application/octet-stream");
    }

    #[test]
    fn test_encode_rpc_response_msgpack_roundtrip() {
        let resp = RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            result: Some(serde_json::json!({"slot": 42, "hash": "abc"})),
            error: None,
        };
        let bytes = rmp_serde::to_vec_named(&resp).unwrap();
        // Deserialize back to a map to verify the key fields round-trip.
        // rmp-serde omits None fields (skip_serializing_if) so we check
        // the fields we know are present.
        let decoded: std::collections::HashMap<String, serde_json::Value> =
            rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded["jsonrpc"], "2.0");
        assert_eq!(decoded["result"]["slot"], 42);
        assert_eq!(decoded["result"]["hash"], "abc");
    }

    #[test]
    fn test_encode_rpc_response_error_msgpack() {
        let resp = RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            result: None,
            error: Some(RpcError {
                code: -32601,
                message: "Method not found".to_string(),
            }),
        };
        let mut headers = HeaderMap::new();
        headers.insert("accept", "application/msgpack".parse().unwrap());
        let response = encode_rpc_response(&headers, resp);
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(ct, "application/msgpack");
    }

    // ── Task 3.4: parse_topic_hash tests ──

    #[test]
    fn test_parse_topic_hash_valid_with_prefix() {
        let hex = format!("0x{}", "ab".repeat(32));
        let result = parse_topic_hash(&hex);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), [0xAB; 32]);
    }

    #[test]
    fn test_parse_topic_hash_valid_without_prefix() {
        let hex = "cd".repeat(32);
        let result = parse_topic_hash(&hex);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), [0xCD; 32]);
    }

    #[test]
    fn test_parse_topic_hash_wrong_length() {
        // Too short (31 bytes)
        let hex = format!("0x{}", "ab".repeat(31));
        assert!(parse_topic_hash(&hex).is_none());

        // Too long (33 bytes)
        let hex = format!("0x{}", "ab".repeat(33));
        assert!(parse_topic_hash(&hex).is_none());
    }

    #[test]
    fn test_parse_topic_hash_invalid_hex() {
        let hex = format!("0x{}", "zz".repeat(32));
        assert!(parse_topic_hash(&hex).is_none());
    }

    #[test]
    fn test_parse_topic_hash_empty() {
        assert!(parse_topic_hash("").is_none());
        assert!(parse_topic_hash("0x").is_none());
    }

    #[test]
    fn test_parse_topic_hash_known_evm_transfer() {
        // Standard ERC-20 Transfer event topic: keccak256("Transfer(address,address,uint256)")
        let hex = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
        let result = parse_topic_hash(hex).expect("should parse transfer topic");
        assert_eq!(result[0], 0xdd);
        assert_eq!(result[1], 0xf2);
        assert_eq!(result[31], 0xef);
    }

    #[tokio::test]
    async fn test_solana_token_account_listing_and_balance_lookup_use_ata_pubkeys() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let rpc_state = make_test_rpc_state(state.clone());

        let owner = Pubkey([0x11u8; 32]);
        let mint = Pubkey([0x22u8; 32]);
        let amount = 1_250_000_000u64;

        state
            .register_symbol(
                "WSOL",
                SymbolRegistryEntry {
                    symbol: "WSOL".to_string(),
                    program: mint,
                    owner: Pubkey([0x33u8; 32]),
                    name: Some("Wrapped SOL".to_string()),
                    template: Some("token".to_string()),
                    metadata: None,
                    decimals: Some(9),
                },
            )
            .unwrap();
        state.update_token_balance(&mint, &owner, amount).unwrap();

        let expected_token_account =
            lichen_core::state::derive_solana_associated_token_address(&owner, &mint).unwrap();

        let result = handle_solana_get_token_accounts_by_owner(
            &rpc_state,
            Some(serde_json::json!([
                owner.to_base58(),
                { "programId": SOLANA_SPL_TOKEN_PROGRAM_ID },
                { "encoding": "jsonParsed" }
            ])),
        )
        .await
        .expect("token account listing should succeed");

        let values = result["value"]
            .as_array()
            .expect("token account list should be an array");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["pubkey"], expected_token_account.to_base58());
        assert_eq!(values[0]["account"]["owner"], SOLANA_SPL_TOKEN_PROGRAM_ID);
        assert_eq!(
            values[0]["account"]["data"]["parsed"]["info"]["mint"],
            mint.to_base58()
        );
        assert_eq!(
            values[0]["account"]["data"]["parsed"]["info"]["owner"],
            owner.to_base58()
        );
        assert_eq!(
            values[0]["account"]["data"]["parsed"]["info"]["tokenAmount"]["amount"],
            amount.to_string()
        );
        assert_eq!(
            values[0]["account"]["data"]["parsed"]["info"]["tokenAmount"]["uiAmountString"],
            "1.25"
        );

        let balance = handle_solana_get_token_account_balance(
            &rpc_state,
            Some(serde_json::json!([expected_token_account.to_base58()])),
        )
        .await
        .expect("token account balance lookup should succeed");

        assert_eq!(balance["value"]["amount"], amount.to_string());
        assert_eq!(balance["value"]["decimals"], 9);
        assert_eq!(balance["value"]["uiAmountString"], "1.25");
    }

    #[tokio::test]
    async fn test_solana_token_account_listing_preserves_zero_balance_accounts() {
        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let rpc_state = make_test_rpc_state(state.clone());

        let owner = Pubkey([0x66u8; 32]);
        let mint = Pubkey([0x77u8; 32]);

        state
            .register_symbol(
                "WBNB",
                SymbolRegistryEntry {
                    symbol: "WBNB".to_string(),
                    program: mint,
                    owner: Pubkey([0x88u8; 32]),
                    name: Some("Wrapped BNB".to_string()),
                    template: Some("token".to_string()),
                    metadata: None,
                    decimals: Some(9),
                },
            )
            .unwrap();
        state.update_token_balance(&mint, &owner, 9).unwrap();
        state.update_token_balance(&mint, &owner, 0).unwrap();

        let token_account =
            lichen_core::state::derive_solana_associated_token_address(&owner, &mint).unwrap();

        let result = handle_solana_get_token_accounts_by_owner(
            &rpc_state,
            Some(serde_json::json!([
                owner.to_base58(),
                { "programId": SOLANA_SPL_TOKEN_PROGRAM_ID },
                { "encoding": "jsonParsed" }
            ])),
        )
        .await
        .expect("zero-balance token account listing should succeed");

        let values = result["value"]
            .as_array()
            .expect("token account list should be an array");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["pubkey"], token_account.to_base58());
        assert_eq!(
            values[0]["account"]["data"]["parsed"]["info"]["tokenAmount"]["amount"],
            "0"
        );
        assert_eq!(
            values[0]["account"]["data"]["parsed"]["info"]["tokenAmount"]["uiAmountString"],
            "0"
        );

        let balance = handle_solana_get_token_account_balance(
            &rpc_state,
            Some(serde_json::json!([token_account.to_base58()])),
        )
        .await
        .expect("zero-balance token account lookup should succeed");

        assert_eq!(balance["value"]["amount"], "0");
        assert_eq!(balance["value"]["uiAmountString"], "0");
    }

    #[tokio::test]
    async fn test_solana_get_account_info_returns_synthetic_token_account_payload() {
        use base64::{engine::general_purpose, Engine as _};

        let tmp = tempdir().unwrap();
        let state = StateStore::open(tmp.path()).unwrap();
        let rpc_state = make_test_rpc_state(state.clone());

        let owner = Pubkey([0x44u8; 32]);
        let mint = Pubkey([0x55u8; 32]);
        let amount = 42u64;

        state.update_token_balance(&mint, &owner, amount).unwrap();
        let token_account =
            lichen_core::state::derive_solana_associated_token_address(&owner, &mint).unwrap();

        let result = handle_solana_get_account_info(
            &rpc_state,
            Some(serde_json::json!([
                token_account.to_base58(),
                { "encoding": "base64" }
            ])),
        )
        .await
        .expect("synthetic token account info should resolve");

        assert_eq!(result["value"]["owner"], SOLANA_SPL_TOKEN_PROGRAM_ID);
        assert_eq!(result["value"]["space"], SOLANA_TOKEN_ACCOUNT_SPACE);

        let encoded = result["value"]["data"][0]
            .as_str()
            .expect("base64 token account data should be present");
        let decoded = general_purpose::STANDARD
            .decode(encoded)
            .expect("decode synthetic token account");

        assert_eq!(decoded.len(), SOLANA_TOKEN_ACCOUNT_SPACE);
        assert_eq!(&decoded[..32], &mint.0);
        assert_eq!(&decoded[32..64], &owner.0);
        let encoded_amount = u64::from_le_bytes(decoded[64..72].try_into().unwrap());
        assert_eq!(encoded_amount, amount);
    }
}
