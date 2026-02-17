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
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::RpcState;
use moltchain_core::contract::ContractAccount;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const PREDICT_PROGRAM: &str = "PREDICT";
const PRICE_SCALE: u64 = 1_000_000_000;

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

/// Load the entire prediction_market ContractAccount
fn load_predict_contract(state: &RpcState) -> Option<ContractAccount> {
    let entry = state.state.get_symbol_registry(PREDICT_PROGRAM).ok()??;
    let account = state.state.get_account(&entry.program).ok()??;
    serde_json::from_slice::<ContractAccount>(&account.data).ok()
}

/// Read raw bytes from prediction_market storage
fn read_bytes(state: &RpcState, key: &[u8]) -> Option<Vec<u8>> {
    let contract = load_predict_contract(state)?;
    contract.get_storage(key)
}

/// Read u64 from prediction_market storage
fn read_u64_key(state: &RpcState, key: &[u8]) -> u64 {
    read_bytes(state, key)
        .and_then(|d| if d.len() >= 8 { Some(u64::from_le_bytes(d[..8].try_into().ok()?)) } else { None })
        .unwrap_or(0)
}

fn current_slot(state: &RpcState) -> u64 {
    state.state.get_last_slot().unwrap_or(0)
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
struct CreateMarketRequest {
    question: String,
    category: String,
    #[serde(rename = "initialLiquidity")]
    initial_liquidity: u64,
    creator: String,
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
fn decode_market(contract: &ContractAccount, id: u64) -> Option<MarketJson> {
    let key = format!("pm_m_{}", id);
    let data = contract.get_storage(key.as_bytes())?;
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

    let winning_outcome = if winning_raw == 0xFF { None } else { Some(winning_raw) };

    // Read question text
    let q_key = format!("pm_q_{}", id);
    let question = contract
        .get_storage(q_key.as_bytes())
        .and_then(|d| String::from_utf8(d).ok())
        .unwrap_or_default();

    // Read outcomes
    let mut outcomes = Vec::new();
    for oi in 0..outcome_count {
        let o_key = format!("pm_o_{}_{}", id, oi);
        let on_key = format!("pm_on_{}_{}", id, oi);

        let name = contract
            .get_storage(on_key.as_bytes())
            .and_then(|d| String::from_utf8(d).ok())
            .unwrap_or_else(|| if oi == 0 { "Yes".to_string() } else { "No".to_string() });

        let (pool_yes, pool_no) = contract
            .get_storage(o_key.as_bytes())
            .map(|d| {
                if d.len() >= 16 {
                    let y = u64_le(&d, 0);
                    let n = u64_le(&d, 8);
                    (y, n)
                } else if d.len() >= 8 {
                    (u64_le(&d, 0), 0u64)
                } else {
                    (0u64, 0u64)
                }
            })
            .unwrap_or((0, 0));

        // CPMM price: price_yes = pool_no / (pool_yes + pool_no)
        let total_pool = pool_yes + pool_no;
        let price = if total_pool > 0 {
            pool_no as f64 / total_pool as f64
        } else {
            0.5
        };

        outcomes.push(OutcomeJson {
            index: oi,
            name,
            pool_yes: pool_yes as f64 / PRICE_SCALE as f64,
            pool_no: pool_no as f64 / PRICE_SCALE as f64,
            price,
        });
    }

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
    let paused = read_bytes(&state, b"pm_paused").map(|d| d.first().copied().unwrap_or(0) != 0).unwrap_or(false);

    ApiResponse::ok(
        PlatformStatsJson {
            total_markets,
            open_markets,
            total_volume: total_volume as f64 / PRICE_SCALE as f64,
            total_collateral: total_collateral as f64 / PRICE_SCALE as f64,
            fees_collected: fees_collected as f64 / PRICE_SCALE as f64,
            total_traders,
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
    let contract = match load_predict_contract(&state) {
        Some(c) => c,
        None => return api_err("Prediction market contract not found"),
    };

    let total_markets = contract
        .get_storage(b"pm_market_count")
        .and_then(|d| if d.len() >= 8 { Some(u64_le(&d, 0)) } else { None })
        .unwrap_or(0);

    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);

    let mut markets = Vec::new();
    for id in 1..=total_markets {
        if let Some(mkt) = decode_market(&contract, id) {
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
async fn get_market(
    State(state): State<Arc<RpcState>>,
    Path(id): Path<u64>,
) -> Response {
    let slot = current_slot(&state);
    let contract = match load_predict_contract(&state) {
        Some(c) => c,
        None => return api_err("Prediction market contract not found"),
    };

    match decode_market(&contract, id) {
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

    let contract = match load_predict_contract(&state) {
        Some(c) => c,
        None => return api_err("Prediction market contract not found"),
    };

    // Get user's participation count
    let count_key = format!("pm_userc_{}", addr);
    let count = contract
        .get_storage(count_key.as_bytes())
        .and_then(|d| if d.len() >= 8 { Some(u64_le(&d, 0)) } else { None })
        .unwrap_or(0);

    let mut positions = Vec::new();

    // Iterate user's markets
    for idx in 0..count {
        let um_key = format!("pm_user_{}_{}", addr, idx);
        let market_id = match contract.get_storage(um_key.as_bytes()) {
            Some(d) if d.len() >= 8 => u64_le(&d, 0),
            _ => continue,
        };

        // Get market record to know outcome_count
        let mkt_key = format!("pm_m_{}", market_id);
        let mkt_data = match contract.get_storage(mkt_key.as_bytes()) {
            Some(d) if d.len() >= 192 => d,
            _ => continue,
        };
        let outcome_count = mkt_data[65];

        // Check each outcome for positions
        for oi in 0..outcome_count {
            let pos_key = format!("pm_p_{}_{}_{}", market_id, addr, oi);
            if let Some(pd) = contract.get_storage(pos_key.as_bytes()) {
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

    let contract = match load_predict_contract(&state) {
        Some(c) => c,
        None => return api_err("Prediction market contract not found"),
    };

    // Read snapshot count
    let count_key = format!("pm_phc_{}", id);
    let count = contract
        .get_storage(count_key.as_bytes())
        .and_then(|d| if d.len() >= 8 { Some(u64_le(&d, 0)) } else { None })
        .unwrap_or(0);

    let offset = q.offset.unwrap_or(0) as u64;
    let start = if offset > 0 { offset.min(count) } else { count.saturating_sub(limit as u64) };
    let end = count.min(start + limit as u64);

    let mut snapshots = Vec::new();
    // Estimate timestamps: assume ~400ms per slot from genesis
    let slot_duration_ms: u64 = 400;
    let current_slot_val = slot;

    for i in start..end {
        let key = format!("pm_ph_{}_{}", id, i);
        if let Some(data) = contract.get_storage(key.as_bytes()) {
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
async fn post_trade(
    State(state): State<Arc<RpcState>>,
    Json(req): Json<TradeRequest>,
) -> Response {
    let slot = current_slot(&state);
    let contract = match load_predict_contract(&state) {
        Some(c) => c,
        None => return api_err("Prediction market contract not found"),
    };

    // Validate market exists and is active
    let mkt_key = format!("pm_m_{}", req.market_id);
    let mkt_data = match contract.get_storage(mkt_key.as_bytes()) {
        Some(d) if d.len() >= 192 => d,
        _ => return api_404(&format!("Market {} not found", req.market_id)),
    };

    let status = mkt_data[64];
    if status != 1 {
        return api_err(&format!("Market {} is not active (status={})", req.market_id, status_name(status)));
    }

    let outcome_count = mkt_data[65];
    if req.outcome >= outcome_count {
        return api_err(&format!("Invalid outcome {} (market has {} outcomes)", req.outcome, outcome_count));
    }

    // Read current pool for the outcome
    let o_key = format!("pm_o_{}_{}", req.market_id, req.outcome);
    let (pool_yes, pool_no) = contract
        .get_storage(o_key.as_bytes())
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
    let price = if total_pool > 0 { pool_no as f64 / total_pool as f64 } else { 0.5 };
    let fee_rate = 0.02; // 2% fee
    let net_amount = req.amount as f64 * (1.0 - fee_rate);
    let shares = if price > 0.0 && price < 1.0 { net_amount / price } else { net_amount };
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
/// In production this would submit a transaction. For now returns a preview.
async fn post_create(
    State(state): State<Arc<RpcState>>,
    Json(req): Json<CreateMarketRequest>,
) -> Response {
    let slot = current_slot(&state);

    if req.question.is_empty() || req.question.len() > 512 {
        return api_err("Question must be 1-512 characters");
    }

    let cat_id = match req.category.as_str() {
        "politics" => 0u8,
        "sports" => 1,
        "crypto" => 2,
        "science" => 3,
        "entertainment" => 4,
        "economics" => 5,
        "tech" => 6,
        _ => 7,
    };

    let market_count = read_u64_key(&state, b"pm_market_count");

    #[derive(Serialize)]
    struct CreatePreview {
        next_market_id: u64,
        question: String,
        category: &'static str,
        initial_liquidity: f64,
        creator: String,
        status: &'static str,
    }

    ApiResponse::ok(
        CreatePreview {
            next_market_id: market_count + 1,
            question: req.question,
            category: category_name(cat_id),
            initial_liquidity: req.initial_liquidity as f64 / PRICE_SCALE as f64,
            creator: req.creator,
            status: "preview",
        },
        slot,
    )
    .into_response()
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

    let contract = match load_predict_contract(&state) {
        Some(c) => c,
        None => return api_err("Prediction market contract not found"),
    };

    let scan_max = total_traders.min(500) as usize;
    let mut entries: Vec<(String, u64, u64)> = Vec::with_capacity(scan_max);

    for i in 0..scan_max as u64 {
        let lk = format!("pm_tl_{}", i);
        if let Some(addr_data) = contract.get_storage(lk.as_bytes()) {
            if addr_data.len() >= 32 {
                let addr_hex = hex::encode(&addr_data[..32]);
                let tk = format!("pm_ts_{}", addr_hex);
                if let Some(sd) = contract.get_storage(tk.as_bytes()) {
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
    let contract = match load_predict_contract(&state) {
        Some(c) => c,
        None => return api_err("Prediction market contract not found"),
    };

    let total_markets = contract
        .get_storage(b"pm_market_count")
        .and_then(|d| if d.len() >= 8 { Some(u64_le(&d, 0)) } else { None })
        .unwrap_or(0);

    let mut markets: Vec<TrendingMarketJson> = Vec::new();

    for id in 1..=total_markets {
        let mkt_key = format!("pm_m_{}", id);
        let mkt_data = match contract.get_storage(mkt_key.as_bytes()) {
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
        let question = contract
            .get_storage(q_key.as_bytes())
            .and_then(|d| String::from_utf8(d).ok())
            .unwrap_or_default();

        let vol24_key = format!("pm_mv24_{}", id);
        let vol24 = contract
            .get_storage(vol24_key.as_bytes())
            .and_then(|d| if d.len() >= 8 { Some(u64_le(&d, 0)) } else { None })
            .unwrap_or(0);

        let tc_key = format!("pm_mtc_{}", id);
        let traders = contract
            .get_storage(tc_key.as_bytes())
            .and_then(|d| if d.len() >= 8 { Some(u64_le(&d, 0)) } else { None })
            .unwrap_or(0);

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
    markets.sort_by(|a, b| b.volume_24h.partial_cmp(&a.volume_24h).unwrap_or(std::cmp::Ordering::Equal));
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
async fn get_market_analytics(
    State(state): State<Arc<RpcState>>,
    Path(id): Path<u64>,
) -> Response {
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
}
