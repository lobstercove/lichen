// ═══════════════════════════════════════════════════════════════════════════════
// MoltChain RPC — Prediction Market REST API Module
// Implements /api/v1/prediction-market/* endpoints for PredictionReef
//
// Reads contract storage directly from StateStore using the prediction_market
// key layout (pm_* keys).
// ═══════════════════════════════════════════════════════════════════════════════

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::RpcState;
use moltchain_core::{Instruction, Pubkey, CONTRACT_PROGRAM_ID};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const PREDICT_PROGRAM: &str = "PREDICT";
const DEFAULT_PREDICT_PROGRAM_B58: &str = "J8sMvYFXW4ZCHc488KJ1zmZq1sQMTWyWfr8qnzUwwEyD";
// F12.10 FIX: Prediction market uses MUSD_UNIT (1e6), not DEX PRICE_SCALE (1e9)
const PRICE_SCALE: u64 = 1_000_000;
const DEFAULT_CLOSE_SLOTS: u64 = 1_512_000; // ~7 days at 400ms/slot
const MIN_COLLATERAL: u64 = 1_000_000; // 1 mUSD (6 decimals)
const TRADING_FEE_BPS: u64 = 200; // 2%
const MIN_REPUTATION_CREATE: u64 = 500;
const MIN_REPUTATION_RESOLVE: u64 = 1_000;
const MARKET_CREATION_FEE: u64 = 10_000_000; // 10 mUSD
const DISPUTE_BOND: u64 = 100_000_000; // 100 mUSD
const DISPUTE_PERIOD_SLOTS: u64 = 172_800; // 48h at 400ms/slot

// ─────────────────────────────────────────────────────────────────────────────
// JSON Response Types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ApiResponse<T: Serialize> {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    slot: u64,
}

impl<T: Serialize> ApiResponse<T> {
    fn ok(data: T, slot: u64) -> Json<ApiResponse<T>> {
        Json(ApiResponse {
            success: true,
            data: Some(data),
            error: None,
            slot,
        })
    }
}

fn api_err(msg: &str) -> Response {
    let body = ApiResponse::<()> {
        success: false,
        data: None,
        error: Some(msg.to_string()),
        slot: 0,
    };
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

fn api_404(msg: &str) -> Response {
    let body = ApiResponse::<()> {
        success: false,
        data: None,
        error: Some(msg.to_string()),
        slot: 0,
    };
    (StatusCode::NOT_FOUND, Json(body)).into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// Storage Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Read raw bytes from prediction_market storage via CF_CONTRACT_STORAGE (O(1) point-read).
/// Avoids deserializing the entire ContractAccount + WASM bytecode.
fn resolve_predict_program(state: &RpcState) -> Option<Pubkey> {
    for symbol in [
        PREDICT_PROGRAM,
        "PREDICTION",
        "PREDICTION_MARKET",
        "PREDICTIONREEF",
    ] {
        if let Ok(Some(entry)) = state.state.get_symbol_registry(symbol) {
            if state
                .state
                .get_contract_storage(&entry.program, b"pm_market_count")
                .ok()
                .flatten()
                .is_some()
            {
                return Some(entry.program);
            }
        }
    }

    if let Ok(entries) = state.state.get_all_symbol_registry(512) {
        for entry in entries {
            if state
                .state
                .get_contract_storage(&entry.program, b"pm_market_count")
                .ok()
                .flatten()
                .is_some()
            {
                return Some(entry.program);
            }
        }
    }

    Pubkey::from_base58(DEFAULT_PREDICT_PROGRAM_B58).ok()
}

fn read_bytes(state: &RpcState, key: &[u8]) -> Option<Vec<u8>> {
    let program = resolve_predict_program(state)?;
    state.state.get_contract_storage(&program, key).ok().flatten()
}

/// Read u64 from prediction_market storage via CF_CONTRACT_STORAGE (O(1) point-read).
fn read_u64_key(state: &RpcState, key: &[u8]) -> u64 {
    let Some(program) = resolve_predict_program(state) else {
        return 0;
    };
    state.state.get_contract_storage_u64(&program, key)
}

fn current_slot(state: &RpcState) -> u64 {
    state.state.get_last_slot().unwrap_or(0)
}

fn latest_blockhash_hex(state: &RpcState) -> Result<String, String> {
    let slot = state
        .state
        .get_last_slot()
        .map_err(|e| format!("Database error: {}", e))?;
    if slot == 0 {
        return Err("No blocks yet".to_string());
    }

    let block = state
        .state
        .get_block_by_slot(slot)
        .map_err(|e| format!("Database error: {}", e))?
        .ok_or_else(|| "Latest block not found".to_string())?;
    Ok(block.hash().to_hex())
}

fn map_category(category: &str) -> Option<u8> {
    match category.trim().to_ascii_lowercase().as_str() {
        "politics" => Some(0),
        "sports" => Some(1),
        "crypto" => Some(2),
        "science" => Some(3),
        "entertainment" => Some(4),
        "economics" => Some(5),
        "tech" => Some(6),
        "custom" => Some(7),
        _ => None,
    }
}

fn build_create_market_args(
    creator: &moltchain_core::Pubkey,
    category: u8,
    close_slot: u64,
    outcome_count: u8,
    question: &str,
) -> Vec<u8> {
    let q_bytes = question.as_bytes();
    let mut args = Vec::with_capacity(79 + q_bytes.len());
    args.push(1); // opcode: create_market
    args.extend_from_slice(&creator.0);
    args.push(category);
    args.extend_from_slice(&close_slot.to_le_bytes());
    args.push(outcome_count);

    let mut hasher = Sha256::new();
    hasher.update(q_bytes);
    let digest = hasher.finalize();
    args.extend_from_slice(&digest[..32]);

    args.extend_from_slice(&(q_bytes.len() as u32).to_le_bytes());
    args.extend_from_slice(q_bytes);
    args
}

// ─────────────────────────────────────────────────────────────────────────────
// Market JSON Types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct MarketJson {
    id: u64,
    creator: String,
    question: String,
    category: &'static str,
    status: &'static str,
    outcome_count: u8,
    winning_outcome: Option<u8>,
    total_collateral: f64,
    total_volume: f64,
    fees_collected: f64,
    created_slot: u64,
    close_slot: u64,
    resolve_slot: u64,
    current_slot: u64,
    unique_traders: u64,
    outcomes: Vec<OutcomeJson>,
}

#[derive(Serialize)]
struct OutcomeJson {
    index: u8,
    name: String,
    pool_yes: f64,
    pool_no: f64,
    price: f64,
}

#[derive(Serialize)]
struct PlatformStatsJson {
    total_markets: u64,
    open_markets: u64,
    total_volume: f64,
    total_collateral: f64,
    fees_collected: f64,
    total_traders: u64,
    current_slot: u64,
    paused: bool,
}

#[derive(Serialize)]
struct PredictionConfigJson {
    min_reputation: u64,
    min_reputation_resolve: u64,
    min_collateral: f64,
    trading_fee_bps: u64,
    market_creation_fee: f64,
    dispute_bond: f64,
    dispute_period_slots: u64,
}

#[derive(Serialize)]
struct PriceSnapshotJson {
    slot: u64,
    price: f64,
    volume: f64,
    timestamp: u64,
}

#[derive(Serialize)]
struct PositionJson {
    market_id: u64,
    outcome: u8,
    shares: f64,
    cost_basis: f64,
}

#[derive(Deserialize)]
struct MarketListQuery {
    category: Option<String>,
    status: Option<String>,
    creator: Option<String>, // F12.5: Filter by creator address
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Deserialize)]
struct UserQuery {
    address: Option<String>,
}

#[derive(Deserialize)]
struct PriceHistoryQuery {
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Deserialize)]
struct PredictionTradesQuery {
    address: String,
    market_id: Option<u64>,
    limit: Option<usize>,
    before_slot: Option<u64>,
}

#[derive(Serialize)]
struct PredictionTradeJson {
    signature: String,
    slot: u64,
    timestamp: u64,
    market_id: u64,
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<u8>,
    amount: f64,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct TradeRequest {
    #[serde(rename = "marketId")]
    market_id: u64,
    outcome: u8,
    amount: u64,
    trader: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct CreateMarketRequest {
    question: String,
    category: String,
    #[serde(rename = "initialLiquidity")]
    #[serde(default)]
    initial_liquidity: u64,
    creator: String,
    /// Optional outcome names for multi-outcome markets (2-8). Omit for binary (Yes/No).
    #[serde(default)]
    outcomes: Vec<String>,
    /// Optional explicit outcome count (2-8). If omitted, derives from outcomes length.
    #[serde(rename = "outcomeCount")]
    #[serde(default)]
    outcome_count: Option<u8>,
    /// Optional market close slot. Defaults to current_slot + 7 days.
    #[serde(rename = "closeSlot")]
    #[serde(default)]
    close_slot: Option<u64>,
    /// FIX F13: Admin token required for market creation
    #[serde(default)]
    admin_token: Option<String>,
}

#[derive(Serialize)]
struct CreateMarketTemplateJson {
    rpc_method: &'static str,
    unsigned_transaction: serde_json::Value,
    unsigned_transaction_base64: String,
    prediction_program: String,
    next_market_id_hint: u64,
    close_slot: u64,
    outcome_count: u8,
    notes: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Decode helpers
// ─────────────────────────────────────────────────────────────────────────────

fn category_name(cat: u8) -> &'static str {
    match cat {
        0 => "politics",
        1 => "sports",
        2 => "crypto",
        3 => "science",
        4 => "entertainment",
        5 => "economics",
        6 => "tech",
        7 => "custom",
        _ => "unknown",
    }
}

fn status_name(status: u8, close_slot: u64, current_slot: u64) -> &'static str {
    match status {
        0 => "pending",
        1 => {
            if close_slot > 0 && current_slot >= close_slot {
                "closed"
            } else {
                "active"
            }
        }
        2 => "closed",
        3 => "resolving",
        4 => "resolved",
        5 => "disputed",
        6 => "voided",
        _ => "unknown",
    }
}

fn u64_le(data: &[u8], offset: usize) -> u64 {
    if data.len() < offset + 8 {
        return 0;
    }
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap_or([0; 8]))
}

fn parse_pubkey_flexible(input: &str) -> Option<Pubkey> {
    if let Ok(pk) = Pubkey::from_base58(input) {
        return Some(pk);
    }
    if input.len() == 64 && input.chars().all(|c| c.is_ascii_hexdigit()) {
        let bytes = hex::decode(input).ok()?;
        if bytes.len() != 32 {
            return None;
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        return Some(Pubkey(arr));
    }
    None
}

fn address_aliases(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let trimmed = input.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    if let Ok(pk) = Pubkey::from_base58(trimmed) {
        let hex_addr = hex::encode(pk.0);
        if !out.contains(&hex_addr) {
            out.push(hex_addr);
        }
    }
    if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        if let Ok(bytes) = hex::decode(trimmed) {
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                let b58 = Pubkey(arr).to_base58();
                if !out.contains(&b58) {
                    out.push(b58);
                }
            }
        }
    }
    out
}

fn parse_contract_call_args(ix: &Instruction) -> Option<Vec<u8>> {
    if ix.program_id != CONTRACT_PROGRAM_ID {
        return None;
    }
    let json_str = std::str::from_utf8(&ix.data).ok()?;
    let val: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let args = val.get("Call")?.get("args")?.as_array()?;
    let mut out = Vec::with_capacity(args.len());
    for n in args {
        let value = n.as_u64()?;
        if value > 255 {
            return None;
        }
        out.push(value as u8);
    }
    Some(out)
}

fn decode_prediction_trade(args: &[u8]) -> Option<(&'static str, u64, Option<u8>, f64)> {
    if args.is_empty() {
        return None;
    }
    let op = args[0];
    match op {
        // buy_shares [4][trader 32][market 8][outcome 1][amount 8]
        4 if args.len() >= 50 => {
            let market_id = u64_le(args, 33);
            let outcome = args[41];
            let amount = u64_le(args, 42) as f64 / PRICE_SCALE as f64;
            Some(("buy", market_id, Some(outcome), amount))
        }
        // sell_shares [5][trader 32][market 8][outcome 1][shares 8]
        5 if args.len() >= 50 => {
            let market_id = u64_le(args, 33);
            let outcome = args[41];
            let amount = u64_le(args, 42) as f64 / PRICE_SCALE as f64;
            Some(("sell", market_id, Some(outcome), amount))
        }
        // mint_complete_set [6][user 32][market 8][amount 8]
        6 if args.len() >= 49 => {
            let market_id = u64_le(args, 33);
            let amount = u64_le(args, 41) as f64 / PRICE_SCALE as f64;
            Some(("mint_complete_set", market_id, None, amount))
        }
        // redeem_complete_set [7][user 32][market 8][amount 8]
        7 if args.len() >= 49 => {
            let market_id = u64_le(args, 33);
            let amount = u64_le(args, 41) as f64 / PRICE_SCALE as f64;
            Some(("redeem_complete_set", market_id, None, amount))
        }
        _ => None,
    }
}

/// Decode a 192-byte market record
fn decode_market(state: &RpcState, id: u64, current_slot: u64) -> Option<MarketJson> {
    let key = format!("pm_m_{}", id);
    let data = read_bytes(state, key.as_bytes())?;
    if data.len() < 192 {
        return None;
    }

    let market_id = u64_le(&data, 0);
    let mut creator_bytes = [0u8; 32];
    creator_bytes.copy_from_slice(&data[8..40]);
    let creator = Pubkey(creator_bytes).to_base58();
    let created_slot = u64_le(&data, 40);
    let close_slot = u64_le(&data, 48);
    let resolve_slot = u64_le(&data, 56);
    let status = data[64];
    let outcome_count = data[65];
    let winning_raw = data[66];
    let category = data[67];
    let total_collateral = u64_le(&data, 68);
    let total_volume = u64_le(&data, 76);
    let fees_collected = u64_le(&data, 164);

    let winning_outcome = if winning_raw == 0xFF {
        None
    } else {
        Some(winning_raw)
    };

    // Read question text
    let q_key = format!("pm_q_{}", id);
    let question = read_bytes(state, q_key.as_bytes())
        .and_then(|d| String::from_utf8(d).ok())
        .unwrap_or_default();

    // F11.3 FIX: Read all outcome reserves first, then compute CPMM prices
    let mut outcome_reserves: Vec<u64> = Vec::new();
    let mut outcome_shares: Vec<u64> = Vec::new();
    let mut outcome_names: Vec<String> = Vec::new();
    for oi in 0..outcome_count {
        let o_key = format!("pm_o_{}_{}", id, oi);
        let on_key = format!("pm_on_{}_{}", id, oi);

        let name = read_bytes(state, on_key.as_bytes())
            .and_then(|d| String::from_utf8(d).ok())
            .unwrap_or_else(|| {
                if oi == 0 {
                    "Yes".to_string()
                } else {
                    "No".to_string()
                }
            });

        let (reserve, shares) = read_bytes(state, o_key.as_bytes())
            .map(|d| {
                if d.len() >= 16 {
                    (u64_le(&d, 0), u64_le(&d, 8))
                } else if d.len() >= 8 {
                    (u64_le(&d, 0), 0u64)
                } else {
                    (0u64, 0u64)
                }
            })
            .unwrap_or((0, 0));

        outcome_reserves.push(reserve);
        outcome_shares.push(shares);
        outcome_names.push(name);
    }

    // Compute CPMM prices using cross-outcome reserves
    let mut outcomes = Vec::new();
    for oi in 0..outcome_count as usize {
        let price = if outcome_reserves.len() == 2 {
            // Binary: price_i = reserve_other / (reserve_self + reserve_other)
            let self_r = outcome_reserves[oi] as f64;
            let other_r = outcome_reserves[1 - oi] as f64;
            let sum = self_r + other_r;
            if sum > 0.0 {
                other_r / sum
            } else {
                0.5
            }
        } else {
            // Multi-outcome: price_i = (1/r_i) / sum(1/r_j)
            let all_nonzero = outcome_reserves.iter().all(|&r| r > 0);
            if all_nonzero {
                let recip_sum: f64 = outcome_reserves.iter().map(|&r| 1.0 / r as f64).sum();
                let recip_i = 1.0 / outcome_reserves[oi] as f64;
                recip_i / recip_sum
            } else {
                1.0 / outcome_count as f64
            }
        };

        outcomes.push(OutcomeJson {
            index: oi as u8,
            name: outcome_names[oi].clone(),
            pool_yes: outcome_reserves[oi] as f64 / PRICE_SCALE as f64,
            pool_no: outcome_shares[oi] as f64 / PRICE_SCALE as f64,
            price,
        });
    }

    // F11.9 FIX: Include unique_traders to eliminate N+1 queries
    let trader_count_key = format!("pm_mtc_{}", id);
    let unique_traders = read_bytes(state, trader_count_key.as_bytes())
        .map(|d| if d.len() >= 8 { u64_le(&d, 0) } else { 0 })
        .unwrap_or(0);

    Some(MarketJson {
        id: market_id,
        creator,
        question,
        category: category_name(category),
        status: status_name(status, close_slot, current_slot),
        outcome_count,
        winning_outcome,
        total_collateral: total_collateral as f64 / PRICE_SCALE as f64,
        total_volume: total_volume as f64 / PRICE_SCALE as f64,
        fees_collected: fees_collected as f64 / PRICE_SCALE as f64,
        created_slot,
        close_slot,
        resolve_slot,
        current_slot,
        unique_traders,
        outcomes,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// GET /prediction-market/stats — Platform-wide stats
async fn get_stats(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    let total_markets = read_u64_key(&state, b"pm_market_count");
    let open_markets = read_u64_key(&state, b"pm_open_markets");
    let total_volume = read_u64_key(&state, b"pm_total_volume");
    let total_collateral = read_u64_key(&state, b"pm_total_collateral");
    let fees_collected = read_u64_key(&state, b"pm_fees_collected");
    let total_traders = read_u64_key(&state, b"pm_total_traders");
    let paused = read_bytes(&state, b"pm_paused")
        .map(|d| d.first().copied().unwrap_or(0) != 0)
        .unwrap_or(false);

    ApiResponse::ok(
        PlatformStatsJson {
            total_markets,
            open_markets,
            total_volume: total_volume as f64 / PRICE_SCALE as f64,
            total_collateral: total_collateral as f64 / PRICE_SCALE as f64,
            fees_collected: fees_collected as f64 / PRICE_SCALE as f64,
            total_traders,
            current_slot: slot,
            paused,
        },
        slot,
    )
    .into_response()
}

/// GET /prediction-market/config — protocol constants for frontend bootstrap UI.
async fn get_config(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    ApiResponse::ok(
        PredictionConfigJson {
            min_reputation: MIN_REPUTATION_CREATE,
            min_reputation_resolve: MIN_REPUTATION_RESOLVE,
            min_collateral: MIN_COLLATERAL as f64 / PRICE_SCALE as f64,
            trading_fee_bps: TRADING_FEE_BPS,
            market_creation_fee: MARKET_CREATION_FEE as f64 / PRICE_SCALE as f64,
            dispute_bond: DISPUTE_BOND as f64 / PRICE_SCALE as f64,
            dispute_period_slots: DISPUTE_PERIOD_SLOTS,
        },
        slot,
    )
    .into_response()
}

/// GET /prediction-market/markets — List markets (with optional category/status filter)
async fn get_markets(
    State(state): State<Arc<RpcState>>,
    Query(params): Query<MarketListQuery>,
) -> Response {
    let slot = current_slot(&state);
    let total_markets = read_u64_key(&state, b"pm_market_count");

    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);

    let mut markets = Vec::new();
    for id in 1..=total_markets {
        if let Some(mkt) = decode_market(&state, id, slot) {
            // category filter
            if let Some(ref cat) = params.category {
                if mkt.category != cat.as_str() {
                    continue;
                }
            }
            // status filter
            if let Some(ref st) = params.status {
                if mkt.status != st.as_str() {
                    continue;
                }
            }
            // F12.5 FIX: creator filter for "My Markets" tab
            if let Some(ref cr) = params.creator {
                if mkt.creator != cr.as_str() {
                    continue;
                }
            }
            markets.push(mkt);
        }
    }

    let total = markets.len();
    let page: Vec<_> = markets.into_iter().skip(offset).take(limit).collect();

    #[derive(Serialize)]
    struct MarketsPage {
        markets: Vec<MarketJson>,
        total: usize,
        offset: usize,
        limit: usize,
        current_slot: u64,
    }

    ApiResponse::ok(
        MarketsPage {
            markets: page,
            total,
            offset,
            limit,
            current_slot: slot,
        },
        slot,
    )
    .into_response()
}

/// GET /prediction-market/markets/:id — Single market detail
async fn get_market(State(state): State<Arc<RpcState>>, Path(id): Path<u64>) -> Response {
    let slot = current_slot(&state);

    match decode_market(&state, id, slot) {
        Some(mkt) => ApiResponse::ok(mkt, slot).into_response(),
        None => api_404(&format!("Market {} not found", id)),
    }
}

/// GET /prediction-market/positions?address=... — User's positions across markets
async fn get_positions(
    State(state): State<Arc<RpcState>>,
    Query(params): Query<UserQuery>,
) -> Response {
    let slot = current_slot(&state);
    let addr = match params.address {
        Some(a) => a,
        None => return api_err("address parameter required"),
    };

    let aliases = address_aliases(&addr);

    let mut positions = Vec::new();
    let mut seen = HashSet::new();

    for alias in aliases {
        // Get user's participation count
        let count_key = format!("pm_userc_{}", alias);
        let count = read_u64_key(&state, count_key.as_bytes());

        // Iterate user's markets
        for idx in 0..count {
            let um_key = format!("pm_user_{}_{}", alias, idx);
            let market_id = match read_bytes(&state, um_key.as_bytes()) {
                Some(d) if d.len() >= 8 => u64_le(&d, 0),
                _ => continue,
            };

            // Get market record to know outcome_count
            let mkt_key = format!("pm_m_{}", market_id);
            let mkt_data = match read_bytes(&state, mkt_key.as_bytes()) {
                Some(d) if d.len() >= 192 => d,
                _ => continue,
            };
            let outcome_count = mkt_data[65];

            // Check each outcome for positions
            for oi in 0..outcome_count {
                let pos_key = format!("pm_p_{}_{}_{}", market_id, alias, oi);
                if let Some(pd) = read_bytes(&state, pos_key.as_bytes()) {
                    if pd.len() >= 16 {
                        let shares = u64_le(&pd, 0);
                        let cost_basis = u64_le(&pd, 8);
                        if shares > 0 && seen.insert((market_id, oi)) {
                            positions.push(PositionJson {
                                market_id,
                                outcome: oi,
                                shares: shares as f64 / PRICE_SCALE as f64,
                                cost_basis: cost_basis as f64 / PRICE_SCALE as f64,
                            });
                        }
                    }
                }
            }
        }
    }

    ApiResponse::ok(positions, slot).into_response()
}

/// GET /prediction-market/markets/:id/price-history — Price history snapshots
async fn get_price_history(
    State(state): State<Arc<RpcState>>,
    Path(id): Path<u64>,
    Query(q): Query<PriceHistoryQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(200).min(500);
    let slot = current_slot(&state);

    // Read snapshot count
    let count_key = format!("pm_phc_{}", id);
    let count = read_u64_key(&state, count_key.as_bytes());

    let offset = q.offset.unwrap_or(0) as u64;
    let start = if offset > 0 {
        offset.min(count)
    } else {
        count.saturating_sub(limit as u64)
    };
    let end = count.min(start + limit as u64);

    let mut snapshots = Vec::new();
    // Estimate timestamps: assume ~400ms per slot from genesis
    let slot_duration_ms: u64 = 400;
    let current_slot_val = slot;

    for i in start..end {
        let key = format!("pm_ph_{}_{}", id, i);
        if let Some(data) = read_bytes(&state, key.as_bytes()) {
            if data.len() >= 24 {
                let snap_slot = u64_le(&data, 0);
                let price_raw = u64_le(&data, 8);
                let volume_raw = u64_le(&data, 16);
                // Price is in mUSD units (6 decimals) → normalize to 0.0–1.0
                let price = price_raw as f64 / 1_000_000.0;
                let volume = volume_raw as f64 / PRICE_SCALE as f64;
                // Approximate timestamp
                let slots_ago = current_slot_val.saturating_sub(snap_slot);
                let ts = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64)
                    .saturating_sub(slots_ago * slot_duration_ms)
                    / 1000;
                snapshots.push(PriceSnapshotJson {
                    slot: snap_slot,
                    price,
                    volume,
                    timestamp: ts,
                });
            }
        }
    }

    ApiResponse::ok(snapshots, slot).into_response()
}

/// POST /prediction-market/trade — Submit a trade (buy/sell outcome shares)
/// In production this would create a transaction. For now returns the trade preview.
async fn post_trade(State(state): State<Arc<RpcState>>, Json(req): Json<TradeRequest>) -> Response {
    let slot = current_slot(&state);

    // Validate market exists and is active
    let mkt_key = format!("pm_m_{}", req.market_id);
    let mkt_data = match read_bytes(&state, mkt_key.as_bytes()) {
        Some(d) if d.len() >= 192 => d,
        _ => return api_404(&format!("Market {} not found", req.market_id)),
    };

    let status = mkt_data[64];
    let close_slot = u64_le(&mkt_data, 48);
    let effective_status = status_name(status, close_slot, slot);
    if effective_status != "active" {
        return api_err(&format!(
            "Market {} is not active (status={})",
            req.market_id,
            effective_status
        ));
    }

    let outcome_count = mkt_data[65];
    if req.outcome >= outcome_count {
        return api_err(&format!(
            "Invalid outcome {} (market has {} outcomes)",
            req.outcome, outcome_count
        ));
    }

    // Read current pool for the outcome
    let o_key = format!("pm_o_{}_{}", req.market_id, req.outcome);
    let (pool_yes, pool_no) = read_bytes(&state, o_key.as_bytes())
        .map(|d| {
            if d.len() >= 16 {
                (u64_le(&d, 0), u64_le(&d, 8))
            } else {
                (0u64, 0u64)
            }
        })
        .unwrap_or((0, 0));

    // CPMM trade calculation
    let total_pool = pool_yes + pool_no;
    let price = if total_pool > 0 {
        pool_no as f64 / total_pool as f64
    } else {
        0.5
    };
    let fee_rate = 0.02; // 2% fee
    let net_amount = req.amount as f64 * (1.0 - fee_rate);
    let shares = if price > 0.0 && price < 1.0 {
        net_amount / price
    } else {
        net_amount
    };
    let fee = req.amount as f64 * fee_rate;

    #[derive(Serialize)]
    struct TradePreview {
        market_id: u64,
        outcome: u8,
        amount: f64,
        shares: f64,
        price: f64,
        fee: f64,
        // In production: tx_hash
        status: &'static str,
    }

    // FIX F12: Removed WS event emission from preview-only endpoint.
    // Trades must go through sendTransaction to emit real events.

    ApiResponse::ok(
        TradePreview {
            market_id: req.market_id,
            outcome: req.outcome,
            amount: req.amount as f64 / PRICE_SCALE as f64,
            shares: shares / PRICE_SCALE as f64,
            price,
            fee: fee / PRICE_SCALE as f64,
            status: "preview",
        },
        slot,
    )
    .into_response()
}

/// POST /prediction-market/create — Create a new market
///
/// SECURITY: direct state writes are intentionally disabled.
/// Market creation must go through `sendTransaction` so all writes execute
/// under consensus and deterministic contract logic.
async fn post_create(
    State(state): State<Arc<RpcState>>,
    Json(req): Json<CreateMarketRequest>,
) -> Response {
    let _ = state;
    let _ = req;
    api_err(
        "prediction-market/create is disabled for safety. Submit a signed transaction via sendTransaction to create markets under consensus.",
    )
}

/// POST /prediction-market/create-template — Build unsigned tx template for market creation.
/// Returns a wallet-signable transaction payload that must be submitted via sendTransaction.
async fn post_create_template(
    State(state): State<Arc<RpcState>>,
    Json(req): Json<CreateMarketRequest>,
) -> Response {
    let creator = match moltchain_core::Pubkey::from_base58(req.creator.trim()) {
        Ok(pk) => pk,
        Err(_) => return api_err("creator must be a valid base58 pubkey"),
    };

    if req.question.trim().is_empty() {
        return api_err("question is required");
    }

    let category = match map_category(&req.category) {
        Some(c) => c,
        None => {
            return api_err(
                "invalid category (expected one of: politics, sports, crypto, science, entertainment, economics, tech, custom)",
            )
        }
    };

    let derived_outcomes = if req.outcomes.len() >= 2 {
        req.outcomes.len() as u8
    } else {
        2
    };
    let outcome_count = req.outcome_count.unwrap_or(derived_outcomes);
    if !(2..=8).contains(&outcome_count) {
        return api_err("outcome_count must be between 2 and 8");
    }

    let slot = current_slot(&state);
    if slot == 0 {
        return api_err("chain is not ready: no slots available");
    }

    let close_slot = req
        .close_slot
        .unwrap_or(slot.saturating_add(DEFAULT_CLOSE_SLOTS));
    if close_slot <= slot {
        return api_err("close_slot must be greater than current slot");
    }

    let blockhash_hex = match latest_blockhash_hex(&state) {
        Ok(h) => h,
        Err(e) => return api_err(&e),
    };

    let prediction_program = match resolve_predict_program(&state) {
        Some(program) => program,
        None => return api_err("prediction market program unavailable"),
    };

    let args = build_create_market_args(
        &creator,
        category,
        close_slot,
        outcome_count,
        req.question.trim(),
    );

    let contract_call =
        match moltchain_core::ContractInstruction::call("call".to_string(), args, 0).serialize() {
            Ok(data) => data,
            Err(e) => return api_err(&format!("failed to build contract call: {}", e)),
        };

    let tx_json = serde_json::json!({
        "signatures": [],
        "message": {
            "instructions": [
                {
                    "program_id": moltchain_core::CONTRACT_PROGRAM_ID.to_base58(),
                    "accounts": [creator.to_base58(), prediction_program.to_base58()],
                    "data": contract_call,
                }
            ],
            "blockhash": blockhash_hex,
        }
    });

    let tx_json_bytes = match serde_json::to_vec(&tx_json) {
        Ok(bytes) => bytes,
        Err(e) => return api_err(&format!("failed to encode transaction JSON: {}", e)),
    };
    let tx_b64 = general_purpose::STANDARD.encode(tx_json_bytes);

    let template = CreateMarketTemplateJson {
        rpc_method: "sendTransaction",
        unsigned_transaction: tx_json,
        unsigned_transaction_base64: tx_b64,
        prediction_program: prediction_program.to_base58(),
        next_market_id_hint: read_u64_key(&state, b"pm_market_count").saturating_add(1),
        close_slot,
        outcome_count,
        notes: vec![
            "Sign with wallet, then submit via sendTransaction.".to_string(),
            "initialLiquidity is not auto-applied here; add liquidity in a follow-up transaction after market creation confirms.".to_string(),
            "next_market_id_hint is informational and can change under concurrent market creations.".to_string(),
        ],
    };

    ApiResponse::ok(template, slot).into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-trader stats & leaderboard
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct TraderStatsJson {
    address: String,
    total_volume: f64,
    trade_count: u64,
    last_trade_slot: u64,
}

/// GET /prediction-market/traders/:addr/stats — Per-trader analytics
async fn get_trader_stats(
    State(state): State<Arc<RpcState>>,
    Path(addr): Path<String>,
) -> Response {
    let slot = current_slot(&state);
    let aliases = address_aliases(&addr);
    let mut best_volume = 0u64;
    let mut best_trades = 0u64;
    let mut best_last_slot = 0u64;

    for alias in aliases {
        let key = format!("pm_ts_{}", alias);
        if let Some(d) = read_bytes(&state, key.as_bytes()) {
            if d.len() >= 24 {
                let volume = u64_le(&d, 0);
                let trades = u64_le(&d, 8);
                let last_slot = u64_le(&d, 16);
                if trades > best_trades || (trades == best_trades && last_slot > best_last_slot) {
                    best_volume = volume;
                    best_trades = trades;
                    best_last_slot = last_slot;
                }
            }
        }
    }

    ApiResponse::ok(
        TraderStatsJson {
            address: addr,
            total_volume: best_volume as f64 / PRICE_SCALE as f64,
            trade_count: best_trades,
            last_trade_slot: best_last_slot,
        },
        slot,
    )
    .into_response()
}

/// GET /prediction-market/trades?address=...&market_id=... — Per-fill prediction activity from indexed transactions
async fn get_trades(
    State(state): State<Arc<RpcState>>,
    Query(params): Query<PredictionTradesQuery>,
) -> Response {
    let slot = current_slot(&state);
    let limit = params.limit.unwrap_or(50).clamp(1, 200);

    let address = params.address.trim();
    if address.is_empty() {
        return api_err("address parameter required");
    }

    let pubkey = match parse_pubkey_flexible(address) {
        Some(pk) => pk,
        None => return api_err("address must be valid base58 or 64-char hex pubkey"),
    };

    let predict_program = match resolve_predict_program(&state) {
        Some(program) => program,
        None => return api_err("prediction market program unavailable"),
    };

    let scan_limit = (limit * 8).min(1000);
    let indexed = match state
        .state
        .get_account_tx_signatures_paginated(&pubkey, scan_limit, params.before_slot)
    {
        Ok(v) => v,
        Err(e) => return api_err(&format!("database error: {}", e)),
    };

    let mut out: Vec<PredictionTradeJson> = Vec::new();
    let mut slot_ts_cache: HashMap<u64, u64> = HashMap::new();

    for (sig, sig_slot) in indexed {
        let tx = match state.state.get_transaction(&sig) {
            Ok(Some(tx)) => tx,
            _ => {
                if let Ok(Some(block)) = state.state.get_block_by_slot(sig_slot) {
                    if let Some(found) = block.transactions.iter().find(|t| t.signature() == sig) {
                        found.clone()
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }
        };

        let timestamp = if let Some(ts) = slot_ts_cache.get(&sig_slot) {
            *ts
        } else {
            let ts = state
                .state
                .get_block_by_slot(sig_slot)
                .ok()
                .and_then(|b| b.map(|blk| blk.header.timestamp))
                .unwrap_or(0);
            slot_ts_cache.insert(sig_slot, ts);
            ts
        };

        for ix in &tx.message.instructions {
            if ix.program_id != CONTRACT_PROGRAM_ID || ix.accounts.len() < 2 || ix.accounts[1] != predict_program {
                continue;
            }
            let Some(args) = parse_contract_call_args(ix) else {
                continue;
            };
            let Some((action, market_id, outcome, amount)) = decode_prediction_trade(&args) else {
                continue;
            };
            if let Some(filter_mid) = params.market_id {
                if market_id != filter_mid {
                    continue;
                }
            }

            out.push(PredictionTradeJson {
                signature: bs58::encode(sig.0).into_string(),
                slot: sig_slot,
                timestamp,
                market_id,
                action,
                outcome,
                amount,
            });

            if out.len() >= limit {
                break;
            }
        }

        if out.len() >= limit {
            break;
        }
    }

    ApiResponse::ok(out, slot).into_response()
}

#[derive(Serialize)]
struct LeaderboardEntry {
    rank: usize,
    address: String,
    total_volume: f64,
    trade_count: u64,
}

#[derive(Deserialize)]
struct LeaderboardQuery {
    limit: Option<usize>,
}

/// GET /prediction-market/leaderboard — Top traders by volume
async fn get_leaderboard(
    State(state): State<Arc<RpcState>>,
    Query(params): Query<LeaderboardQuery>,
) -> Response {
    let slot = current_slot(&state);
    let limit = params.limit.unwrap_or(20).min(50);
    let total_traders = read_u64_key(&state, b"pm_total_traders");

    let scan_max = total_traders.min(500) as usize;
    let mut entries: Vec<(String, u64, u64)> = Vec::with_capacity(scan_max);

    for i in 0..scan_max as u64 {
        let lk = format!("pm_tl_{}", i);
        if let Some(addr_data) = read_bytes(&state, lk.as_bytes()) {
            if addr_data.len() >= 32 {
                let addr_hex = hex::encode(&addr_data[..32]);
                let tk = format!("pm_ts_{}", addr_hex);
                if let Some(sd) = read_bytes(&state, tk.as_bytes()) {
                    if sd.len() >= 24 {
                        let vol = u64_le(&sd, 0);
                        let trades = u64_le(&sd, 8);
                        entries.push((addr_hex, vol, trades));
                    }
                }
            }
        }
    }

    // Sort descending by volume
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries.truncate(limit);

    let leaderboard: Vec<LeaderboardEntry> = entries
        .into_iter()
        .enumerate()
        .map(|(i, (addr, vol, trades))| LeaderboardEntry {
            rank: i + 1,
            address: addr,
            total_volume: vol as f64 / PRICE_SCALE as f64,
            trade_count: trades,
        })
        .collect();

    #[derive(Serialize)]
    struct LeaderboardResponse {
        traders: Vec<LeaderboardEntry>,
        total_traders: u64,
    }

    ApiResponse::ok(
        LeaderboardResponse {
            traders: leaderboard,
            total_traders,
        },
        slot,
    )
    .into_response()
}

#[derive(Serialize)]
struct TrendingMarketJson {
    id: u64,
    question: String,
    category: &'static str,
    volume_24h: f64,
    unique_traders: u64,
    total_volume: f64,
    status: &'static str,
}

/// GET /prediction-market/trending — Markets ranked by 24h volume
async fn get_trending(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);

    let total_markets = read_u64_key(&state, b"pm_market_count");

    let mut markets: Vec<TrendingMarketJson> = Vec::new();

    for id in 1..=total_markets {
        let mkt_key = format!("pm_m_{}", id);
        let mkt_data = match read_bytes(&state, mkt_key.as_bytes()) {
            Some(d) if d.len() >= 192 => d,
            _ => continue,
        };

        let status = mkt_data[64];
        let close_slot = u64_le(&mkt_data, 48);
        let effective_status = status_name(status, close_slot, slot);
        // Only include active markets
        if effective_status != "active" {
            continue;
        }

        let category = mkt_data[67];
        let total_volume = u64_le(&mkt_data, 76);

        let q_key = format!("pm_q_{}", id);
        let question = read_bytes(&state, q_key.as_bytes())
            .and_then(|d| String::from_utf8(d).ok())
            .unwrap_or_default();

        let vol24_key = format!("pm_mv24_{}", id);
        let vol24 = read_u64_key(&state, vol24_key.as_bytes());

        let tc_key = format!("pm_mtc_{}", id);
        let traders = read_u64_key(&state, tc_key.as_bytes());

        markets.push(TrendingMarketJson {
            id,
            question,
            category: category_name(category),
            volume_24h: vol24 as f64 / PRICE_SCALE as f64,
            unique_traders: traders,
            total_volume: total_volume as f64 / PRICE_SCALE as f64,
            status: effective_status,
        });
    }

    // Sort by 24h volume descending
    markets.sort_by(|a, b| {
        b.volume_24h
            .partial_cmp(&a.volume_24h)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    markets.truncate(10);

    ApiResponse::ok(markets, slot).into_response()
}

#[derive(Serialize)]
struct MarketAnalyticsJson {
    market_id: u64,
    unique_traders: u64,
    volume_24h: f64,
}

/// GET /prediction-market/markets/:id/analytics — Per-market analytics
async fn get_market_analytics(State(state): State<Arc<RpcState>>, Path(id): Path<u64>) -> Response {
    let slot = current_slot(&state);
    let tc_key = format!("pm_mtc_{}", id);
    let traders = read_u64_key(&state, tc_key.as_bytes());
    let vol24_key = format!("pm_mv24_{}", id);
    let vol24 = read_u64_key(&state, vol24_key.as_bytes());

    ApiResponse::ok(
        MarketAnalyticsJson {
            market_id: id,
            unique_traders: traders,
            volume_24h: vol24 as f64 / PRICE_SCALE as f64,
        },
        slot,
    )
    .into_response()
}

// ═══════════════════════════════════════════════════════════════════════════════
// PUBLIC: Build the Prediction Market API router
// ═══════════════════════════════════════════════════════════════════════════════

/// Build the /api/v1/prediction-market/* router.
pub(crate) fn build_prediction_router() -> Router<Arc<RpcState>> {
    Router::new()
        .route("/config", get(get_config))
        .route("/stats", get(get_stats))
        .route("/markets", get(get_markets))
        .route("/markets/:id", get(get_market))
        .route("/markets/:id/price-history", get(get_price_history))
        .route("/markets/:id/analytics", get(get_market_analytics))
        .route("/positions", get(get_positions))
        .route("/trades", get(get_trades))
        .route("/traders/:addr/stats", get(get_trader_stats))
        .route("/leaderboard", get(get_leaderboard))
        .route("/trending", get(get_trending))
        .route("/trade", post(post_trade))
        .route("/create", post(post_create))
        .route("/create-template", post(post_create_template))
}
