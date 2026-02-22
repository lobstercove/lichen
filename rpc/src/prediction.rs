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
use std::sync::Arc;

use crate::RpcState;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const PREDICT_PROGRAM: &str = "PREDICT";
// F12.10 FIX: Prediction market uses MUSD_UNIT (1e6), not DEX PRICE_SCALE (1e9)
const PRICE_SCALE: u64 = 1_000_000;
const DEFAULT_CLOSE_SLOTS: u64 = 1_512_000; // ~7 days at 400ms/slot

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
fn read_bytes(state: &RpcState, key: &[u8]) -> Option<Vec<u8>> {
    state.state.get_program_storage(PREDICT_PROGRAM, key)
}

/// Read u64 from prediction_market storage via CF_CONTRACT_STORAGE (O(1) point-read).
fn read_u64_key(state: &RpcState, key: &[u8]) -> u64 {
    state.state.get_program_storage_u64(PREDICT_PROGRAM, key)
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

fn status_name(status: u8) -> &'static str {
    match status {
        0 => "pending",
        1 => "active",
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

/// Decode a 192-byte market record
fn decode_market(state: &RpcState, id: u64) -> Option<MarketJson> {
    let key = format!("pm_m_{}", id);
    let data = state
        .state
        .get_program_storage(PREDICT_PROGRAM, key.as_bytes())?;
    if data.len() < 192 {
        return None;
    }

    let market_id = u64_le(&data, 0);
    let creator = hex::encode(&data[8..40]);
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
    let question = state
        .state
        .get_program_storage(PREDICT_PROGRAM, q_key.as_bytes())
        .and_then(|d| String::from_utf8(d).ok())
        .unwrap_or_default();

    // F11.3 FIX: Read all outcome reserves first, then compute CPMM prices
    let mut outcome_reserves: Vec<u64> = Vec::new();
    let mut outcome_shares: Vec<u64> = Vec::new();
    let mut outcome_names: Vec<String> = Vec::new();
    for oi in 0..outcome_count {
        let o_key = format!("pm_o_{}_{}", id, oi);
        let on_key = format!("pm_on_{}_{}", id, oi);

        let name = state
            .state
            .get_program_storage(PREDICT_PROGRAM, on_key.as_bytes())
            .and_then(|d| String::from_utf8(d).ok())
            .unwrap_or_else(|| {
                if oi == 0 {
                    "Yes".to_string()
                } else {
                    "No".to_string()
                }
            });

        let (reserve, shares) = state
            .state
            .get_program_storage(PREDICT_PROGRAM, o_key.as_bytes())
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
    let unique_traders = state
        .state
        .get_program_storage(PREDICT_PROGRAM, trader_count_key.as_bytes())
        .map(|d| if d.len() >= 8 { u64_le(&d, 0) } else { 0 })
        .unwrap_or(0);

    Some(MarketJson {
        id: market_id,
        creator,
        question,
        category: category_name(category),
        status: status_name(status),
        outcome_count,
        winning_outcome,
        total_collateral: total_collateral as f64 / PRICE_SCALE as f64,
        total_volume: total_volume as f64 / PRICE_SCALE as f64,
        fees_collected: fees_collected as f64 / PRICE_SCALE as f64,
        created_slot,
        close_slot,
        resolve_slot,
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
        if let Some(mkt) = decode_market(&state, id) {
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
    }

    ApiResponse::ok(
        MarketsPage {
            markets: page,
            total,
            offset,
            limit,
        },
        slot,
    )
    .into_response()
}

/// GET /prediction-market/markets/:id — Single market detail
async fn get_market(State(state): State<Arc<RpcState>>, Path(id): Path<u64>) -> Response {
    let slot = current_slot(&state);

    match decode_market(&state, id) {
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

    // Get user's participation count
    let count_key = format!("pm_userc_{}", addr);
    let count = read_u64_key(&state, count_key.as_bytes());

    let mut positions = Vec::new();

    // Iterate user's markets
    for idx in 0..count {
        let um_key = format!("pm_user_{}_{}", addr, idx);
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
            let pos_key = format!("pm_p_{}_{}_{}", market_id, addr, oi);
            if let Some(pd) = read_bytes(&state, pos_key.as_bytes()) {
                if pd.len() >= 16 {
                    let shares = u64_le(&pd, 0);
                    let cost_basis = u64_le(&pd, 8);
                    if shares > 0 {
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
    if status != 1 {
        return api_err(&format!(
            "Market {} is not active (status={})",
            req.market_id,
            status_name(status)
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

    let close_slot = req.close_slot.unwrap_or(slot.saturating_add(DEFAULT_CLOSE_SLOTS));
    if close_slot <= slot {
        return api_err("close_slot must be greater than current slot");
    }

    let blockhash_hex = match latest_blockhash_hex(&state) {
        Ok(h) => h,
        Err(e) => return api_err(&e),
    };

    let symbol_entry = match state.state.get_symbol_registry(PREDICT_PROGRAM) {
        Ok(Some(entry)) => entry,
        Ok(None) => return api_err("prediction market symbol not found in registry"),
        Err(e) => return api_err(&format!("database error: {}", e)),
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
                    "accounts": [creator.to_base58(), symbol_entry.program.to_base58()],
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
        prediction_program: symbol_entry.program.to_base58(),
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
    let key = format!("pm_ts_{}", addr);
    let data = read_bytes(&state, key.as_bytes());
    match data {
        Some(d) if d.len() >= 24 => {
            let volume = u64_le(&d, 0);
            let trades = u64_le(&d, 8);
            let last_slot = u64_le(&d, 16);
            ApiResponse::ok(
                TraderStatsJson {
                    address: addr,
                    total_volume: volume as f64 / PRICE_SCALE as f64,
                    trade_count: trades,
                    last_trade_slot: last_slot,
                },
                slot,
            )
            .into_response()
        }
        _ => ApiResponse::ok(
            TraderStatsJson {
                address: addr,
                total_volume: 0.0,
                trade_count: 0,
                last_trade_slot: 0,
            },
            slot,
        )
        .into_response(),
    }
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
        // Only include active markets
        if status != 1 {
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
            status: status_name(status),
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
        .route("/stats", get(get_stats))
        .route("/markets", get(get_markets))
        .route("/markets/:id", get(get_market))
        .route("/markets/:id/price-history", get(get_price_history))
        .route("/markets/:id/analytics", get(get_market_analytics))
        .route("/positions", get(get_positions))
        .route("/traders/:addr/stats", get(get_trader_stats))
        .route("/leaderboard", get(get_leaderboard))
        .route("/trending", get(get_trending))
        .route("/trade", post(post_trade))
        .route("/create", post(post_create))
        .route("/create-template", post(post_create_template))
}
