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
    paused: bool,
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
    let paused = read_bytes(&state, b"pm_paused").map(|d| d.first().copied().unwrap_or(0) != 0).unwrap_or(false);

    ApiResponse::ok(
        PlatformStatsJson {
            total_markets,
            open_markets,
            total_volume: total_volume as f64 / PRICE_SCALE as f64,
            total_collateral: total_collateral as f64 / PRICE_SCALE as f64,
            fees_collected: fees_collected as f64 / PRICE_SCALE as f64,
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

// ═══════════════════════════════════════════════════════════════════════════════
// PUBLIC: Build the Prediction Market API router
// ═══════════════════════════════════════════════════════════════════════════════

/// Build the /api/v1/prediction-market/* router.
pub(crate) fn build_prediction_router() -> Router<Arc<RpcState>> {
    Router::new()
        .route("/stats", get(get_stats))
        .route("/markets", get(get_markets))
        .route("/markets/:id", get(get_market))
        .route("/positions", get(get_positions))
        .route("/trade", post(post_trade))
        .route("/create", post(post_create))
}
