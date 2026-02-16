// ═══════════════════════════════════════════════════════════════════════════════
// MoltChain RPC — DEX REST API Module
// Implements /api/v1/* endpoints for MoltyDEX
//
// Reads contract storage directly from StateStore using the dex_core, dex_amm,
// dex_margin, dex_analytics, dex_router, dex_rewards, dex_governance key layouts.
// ═══════════════════════════════════════════════════════════════════════════════

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::RpcState;
use moltchain_core::contract::ContractAccount;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const PRICE_SCALE: u64 = 1_000_000_000;
const PNL_BIAS: u64 = 1u64 << 63;

// Contract storage key maps — must match genesis symbol registry (uppercase, alphanumeric only)
const DEX_CORE_PROGRAM: &str = "DEX";
const DEX_AMM_PROGRAM: &str = "DEXAMM";
const DEX_MARGIN_PROGRAM: &str = "DEXMARGIN";
const DEX_ANALYTICS_PROGRAM: &str = "ANALYTICS";
const DEX_ROUTER_PROGRAM: &str = "DEXROUTER";
const DEX_REWARDS_PROGRAM: &str = "DEXREWARDS";
const DEX_GOVERNANCE_PROGRAM: &str = "DEXGOV";

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

fn api_not_found(msg: &str) -> Response {
    let body = ApiResponse::<()> {
        success: false,
        data: None,
        error: Some(msg.to_string()),
        slot: 0,
    };
    (StatusCode::NOT_FOUND, Json(body)).into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// Data Structures (JSON representations of on-chain data)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct TradingPairJson {
    pub pair_id: u64,
    pub base_token: String,
    pub quote_token: String,
    pub tick_size: u64,
    pub lot_size: u64,
    pub min_order: u64,
    pub status: &'static str,
    pub maker_fee_bps: i16,
    pub taker_fee_bps: u16,
    pub daily_volume: u64,
}

#[derive(Serialize, Clone)]
pub struct OrderJson {
    pub order_id: u64,
    pub trader: String,
    pub pair_id: u64,
    pub side: &'static str,
    pub order_type: &'static str,
    pub price: f64,
    pub price_raw: u64,
    pub quantity: u64,
    pub filled: u64,
    pub status: &'static str,
    pub created_slot: u64,
    pub expiry_slot: u64,
}

#[derive(Serialize, Clone)]
pub struct TradeJson {
    pub trade_id: u64,
    pub pair_id: u64,
    pub price: f64,
    pub price_raw: u64,
    pub quantity: u64,
    pub taker: String,
    pub maker_order_id: u64,
    pub slot: u64,
}

#[derive(Serialize, Clone)]
pub struct OrderBookLevel {
    pub price: f64,
    pub quantity: u64,
    pub orders: u32,
}

#[derive(Serialize)]
pub struct OrderBookJson {
    pub pair_id: u64,
    pub bids: Vec<OrderBookLevel>,
    pub asks: Vec<OrderBookLevel>,
    pub slot: u64,
}

#[derive(Serialize, Clone)]
pub struct PoolJson {
    pub pool_id: u64,
    pub token_a: String,
    pub token_b: String,
    pub sqrt_price: u64,
    pub tick: i32,
    pub liquidity: u64,
    pub fee_tier: &'static str,
    pub protocol_fee: u8,
}

#[derive(Serialize, Clone)]
pub struct PositionJson {
    pub position_id: u64,
    pub owner: String,
    pub pool_id: u64,
    pub lower_tick: i32,
    pub upper_tick: i32,
    pub liquidity: u64,
    pub fee_a_owed: u64,
    pub fee_b_owed: u64,
    pub created_slot: u64,
}

#[derive(Serialize, Clone)]
pub struct MarginPositionJson {
    pub position_id: u64,
    pub trader: String,
    pub pair_id: u64,
    pub side: &'static str,
    pub status: &'static str,
    pub size: u64,
    pub margin: u64,
    pub entry_price: f64,
    pub entry_price_raw: u64,
    pub leverage: u64,
    pub created_slot: u64,
    pub realized_pnl: i64,
    pub accumulated_funding: u64,
}

#[derive(Serialize, Clone)]
pub struct MarginInfoJson {
    pub insurance_fund: u64,
    pub last_funding_slot: u64,
    pub maintenance_bps: u64,
    pub position_count: u64,
}

#[derive(Serialize, Clone)]
pub struct CandleJson {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: u64,
    pub slot: u64,
}

#[derive(Serialize, Clone)]
pub struct Stats24hJson {
    pub volume: u64,
    pub high: f64,
    pub low: f64,
    pub open: f64,
    pub close: f64,
    pub trade_count: u64,
    pub change: f64,
    pub change_percent: f64,
}

#[derive(Serialize)]
pub struct TickerJson {
    pub pair_id: u64,
    pub last_price: f64,
    pub bid: f64,
    pub ask: f64,
    pub volume_24h: u64,
    pub change_24h: f64,
}

#[derive(Serialize)]
pub struct LeaderboardEntryJson {
    pub rank: u32,
    pub address: String,
    pub total_volume: u64,
    pub trade_count: u64,
    pub total_pnl: i64,
}

#[derive(Serialize)]
pub struct RewardInfoJson {
    pub pending: u64,
    pub claimed: u64,
    pub total_volume: u64,
    pub referral_count: u64,
    pub referral_earnings: u64,
}

#[derive(Serialize, Clone)]
pub struct RouteJson {
    pub route_id: u64,
    pub token_in: String,
    pub token_out: String,
    pub route_type: &'static str,
    pub pool_or_pair_id: u64,
    pub secondary_id: u64,
    pub split_percent: u8,
    pub enabled: bool,
}

#[derive(Serialize, Clone)]
pub struct ProposalJson {
    pub proposal_id: u64,
    pub proposer: String,
    pub proposal_type: &'static str,
    pub status: &'static str,
    pub created_slot: u64,
    pub end_slot: u64,
    pub yes_votes: u64,
    pub no_votes: u64,
    pub pair_id: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Query Params
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DepthQuery {
    depth: Option<usize>,
}

#[derive(Deserialize)]
pub struct LimitQuery {
    limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct TraderQuery {
    trader: Option<String>,
    status: Option<String>,
    #[serde(rename = "pairId")]
    pair_id: Option<u64>,
}

#[derive(Deserialize)]
pub struct CandleQuery {
    interval: Option<u64>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct OwnerQuery {
    owner: Option<String>,
}

#[derive(Deserialize)]
pub struct StatusQuery {
    status: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// POST Body Types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PlaceOrderBody {
    pub pair: serde_json::Value, // string or number
    pub side: String,
    pub price: f64,
    pub quantity: u64,
    #[serde(default = "default_limit")]
    pub order_type: String,
    pub expiry: Option<u64>,
}

fn default_limit() -> String {
    "limit".into()
}

#[derive(Deserialize)]
pub struct SwapBody {
    #[serde(rename = "tokenIn")]
    pub token_in: String,
    #[serde(rename = "tokenOut")]
    pub token_out: String,
    #[serde(rename = "amountIn")]
    pub amount_in: u64,
    pub slippage: f64,
}

#[derive(Deserialize)]
pub struct OpenPositionBody {
    pub pair: serde_json::Value,
    pub side: String,
    pub margin: u64,
    pub leverage: u64,
}

#[derive(Deserialize)]
pub struct ClosePositionBody {
    #[serde(rename = "positionId")]
    pub position_id: u64,
}

#[derive(Deserialize)]
pub struct AddMarginBody {
    pub amount: u64,
}

#[derive(Deserialize)]
pub struct VoteBody {
    pub support: bool,
    pub amount: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Storage Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Read raw bytes from contract storage.
/// Resolves program name → Pubkey via symbol registry, loads the account,
/// deserializes as ContractAccount, then reads the given storage key.
fn read_bytes(state: &crate::RpcState, program: &str, key: &str) -> Option<Vec<u8>> {
    let entry = state.state.get_symbol_registry(program).ok()??;
    let account = state.state.get_account(&entry.program).ok()??;
    let contract: ContractAccount = serde_json::from_slice(&account.data).ok()?;
    contract.get_storage(key.as_bytes())
}

/// Read a u64 from contract storage
fn read_u64(state: &crate::RpcState, program: &str, key: &str) -> u64 {
    if let Some(data) = read_bytes(state, program, key) {
        if data.len() >= 8 {
            u64::from_le_bytes(data[..8].try_into().unwrap_or([0; 8]))
        } else {
            0
        }
    } else {
        0
    }
}

/// Read a 32-byte address and return as hex
#[allow(dead_code)]
fn read_addr_hex(state: &crate::RpcState, program: &str, key: &str) -> String {
    match read_bytes(state, program, key) {
        Some(data) if data.len() >= 32 => hex::encode(&data[..32]),
        _ => String::new(),
    }
}

/// Get current slot
fn current_slot(state: &crate::RpcState) -> u64 {
    state.state.get_last_slot().unwrap_or(0)
}

/// Decode a trading pair from 112-byte blob
fn decode_pair(data: &[u8]) -> Option<TradingPairJson> {
    if data.len() < 112 {
        return None;
    }

    let base_token = hex::encode(&data[0..32]);
    let quote_token = hex::encode(&data[32..64]);
    let pair_id = u64::from_le_bytes(data[64..72].try_into().ok()?);
    let tick_size = u64::from_le_bytes(data[72..80].try_into().ok()?);
    let lot_size = u64::from_le_bytes(data[80..88].try_into().ok()?);
    let min_order = u64::from_le_bytes(data[88..96].try_into().ok()?);
    let status = match data[96] {
        0 => "active",
        1 => "paused",
        _ => "delisted",
    };
    let maker_fee_bps = i16::from_le_bytes(data[97..99].try_into().ok()?);
    let taker_fee_bps = u16::from_le_bytes(data[99..101].try_into().ok()?);
    let daily_volume = u64::from_le_bytes(data[101..109].try_into().ok()?);

    Some(TradingPairJson {
        pair_id,
        base_token,
        quote_token,
        tick_size,
        lot_size,
        min_order,
        status,
        maker_fee_bps,
        taker_fee_bps,
        daily_volume,
    })
}

/// Decode an order from 128-byte blob
fn decode_order(data: &[u8]) -> Option<OrderJson> {
    if data.len() < 128 {
        return None;
    }

    let trader = hex::encode(&data[0..32]);
    let pair_id = u64::from_le_bytes(data[32..40].try_into().ok()?);
    let side = match data[40] {
        0 => "buy",
        _ => "sell",
    };
    let order_type = match data[41] {
        0 => "limit",
        1 => "market",
        2 => "stop-limit",
        _ => "post-only",
    };
    let price_raw = u64::from_le_bytes(data[42..50].try_into().ok()?);
    let quantity = u64::from_le_bytes(data[50..58].try_into().ok()?);
    let filled = u64::from_le_bytes(data[58..66].try_into().ok()?);
    let status = match data[66] {
        0 => "open",
        1 => "partial",
        2 => "filled",
        3 => "cancelled",
        _ => "expired",
    };
    let created_slot = u64::from_le_bytes(data[67..75].try_into().ok()?);
    let expiry_slot = u64::from_le_bytes(data[75..83].try_into().ok()?);
    let order_id = u64::from_le_bytes(data[83..91].try_into().ok()?);

    Some(OrderJson {
        order_id,
        trader,
        pair_id,
        side,
        order_type,
        price: price_raw as f64 / PRICE_SCALE as f64,
        price_raw,
        quantity,
        filled,
        status,
        created_slot,
        expiry_slot,
    })
}

/// Decode a trade from 80-byte blob
fn decode_trade(data: &[u8]) -> Option<TradeJson> {
    if data.len() < 80 {
        return None;
    }

    let trade_id = u64::from_le_bytes(data[0..8].try_into().ok()?);
    let pair_id = u64::from_le_bytes(data[8..16].try_into().ok()?);
    let price_raw = u64::from_le_bytes(data[16..24].try_into().ok()?);
    let quantity = u64::from_le_bytes(data[24..32].try_into().ok()?);
    let taker = hex::encode(&data[32..64]);
    let maker_order_id = u64::from_le_bytes(data[64..72].try_into().ok()?);
    let slot = u64::from_le_bytes(data[72..80].try_into().ok()?);

    Some(TradeJson {
        trade_id,
        pair_id,
        price: price_raw as f64 / PRICE_SCALE as f64,
        price_raw,
        quantity,
        taker,
        maker_order_id,
        slot,
    })
}

/// Decode a pool from 96-byte blob
fn decode_pool(data: &[u8]) -> Option<PoolJson> {
    if data.len() < 96 {
        return None;
    }

    let token_a = hex::encode(&data[0..32]);
    let token_b = hex::encode(&data[32..64]);
    let pool_id = u64::from_le_bytes(data[64..72].try_into().ok()?);
    let sqrt_price = u64::from_le_bytes(data[72..80].try_into().ok()?);
    let tick = i32::from_le_bytes(data[80..84].try_into().ok()?);
    let liquidity = u64::from_le_bytes(data[84..92].try_into().ok()?);
    let fee_tier = match data[92] {
        0 => "1bps",
        1 => "5bps",
        2 => "30bps",
        _ => "100bps",
    };
    let protocol_fee = data[93];

    Some(PoolJson {
        pool_id,
        token_a,
        token_b,
        sqrt_price,
        tick,
        liquidity,
        fee_tier,
        protocol_fee,
    })
}

/// Decode an LP position from 80-byte blob
fn decode_lp_position(data: &[u8], position_id: u64) -> Option<PositionJson> {
    if data.len() < 80 {
        return None;
    }

    let owner = hex::encode(&data[0..32]);
    let pool_id = u64::from_le_bytes(data[32..40].try_into().ok()?);
    let lower_tick = i32::from_le_bytes(data[40..44].try_into().ok()?);
    let upper_tick = i32::from_le_bytes(data[44..48].try_into().ok()?);
    let liquidity = u64::from_le_bytes(data[48..56].try_into().ok()?);
    let fee_a_owed = u64::from_le_bytes(data[56..64].try_into().ok()?);
    let fee_b_owed = u64::from_le_bytes(data[64..72].try_into().ok()?);
    let created_slot = u64::from_le_bytes(data[72..80].try_into().ok()?);

    Some(PositionJson {
        position_id,
        owner,
        pool_id,
        lower_tick,
        upper_tick,
        liquidity,
        fee_a_owed,
        fee_b_owed,
        created_slot,
    })
}

/// Decode a margin position from 112-byte blob
fn decode_margin_position(data: &[u8]) -> Option<MarginPositionJson> {
    if data.len() < 112 {
        return None;
    }

    let trader = hex::encode(&data[0..32]);
    let position_id = u64::from_le_bytes(data[32..40].try_into().ok()?);
    let pair_id = u64::from_le_bytes(data[40..48].try_into().ok()?);
    let side = match data[48] {
        0 => "long",
        _ => "short",
    };
    let status = match data[49] {
        0 => "open",
        1 => "closed",
        _ => "liquidated",
    };
    let size = u64::from_le_bytes(data[50..58].try_into().ok()?);
    let margin = u64::from_le_bytes(data[58..66].try_into().ok()?);
    let entry_price_raw = u64::from_le_bytes(data[66..74].try_into().ok()?);
    let leverage = u64::from_le_bytes(data[74..82].try_into().ok()?);
    let created_slot = u64::from_le_bytes(data[82..90].try_into().ok()?);
    let raw_pnl = u64::from_le_bytes(data[90..98].try_into().ok()?);
    let realized_pnl = raw_pnl as i64 - PNL_BIAS as i64;
    let accumulated_funding = u64::from_le_bytes(data[98..106].try_into().ok()?);

    Some(MarginPositionJson {
        position_id,
        trader,
        pair_id,
        side,
        status,
        size,
        margin,
        entry_price: entry_price_raw as f64 / PRICE_SCALE as f64,
        entry_price_raw,
        leverage,
        created_slot,
        realized_pnl,
        accumulated_funding,
    })
}

/// Decode a candle from 48-byte blob
fn decode_candle(data: &[u8]) -> Option<CandleJson> {
    if data.len() < 48 {
        return None;
    }

    let open = u64::from_le_bytes(data[0..8].try_into().ok()?);
    let high = u64::from_le_bytes(data[8..16].try_into().ok()?);
    let low = u64::from_le_bytes(data[16..24].try_into().ok()?);
    let close = u64::from_le_bytes(data[24..32].try_into().ok()?);
    let volume = u64::from_le_bytes(data[32..40].try_into().ok()?);
    let slot = u64::from_le_bytes(data[40..48].try_into().ok()?);

    Some(CandleJson {
        open: open as f64 / PRICE_SCALE as f64,
        high: high as f64 / PRICE_SCALE as f64,
        low: low as f64 / PRICE_SCALE as f64,
        close: close as f64 / PRICE_SCALE as f64,
        volume,
        slot,
    })
}

/// Decode 24h stats from 48-byte blob
fn decode_stats_24h(data: &[u8]) -> Option<Stats24hJson> {
    if data.len() < 48 {
        return None;
    }

    let volume = u64::from_le_bytes(data[0..8].try_into().ok()?);
    let high = u64::from_le_bytes(data[8..16].try_into().ok()?);
    let low = u64::from_le_bytes(data[16..24].try_into().ok()?);
    let open = u64::from_le_bytes(data[24..32].try_into().ok()?);
    let close = u64::from_le_bytes(data[32..40].try_into().ok()?);
    let trade_count = u64::from_le_bytes(data[40..48].try_into().ok()?);

    let open_f = open as f64 / PRICE_SCALE as f64;
    let close_f = close as f64 / PRICE_SCALE as f64;
    let change = close_f - open_f;
    let change_percent = if open_f > 0.0 {
        (change / open_f) * 100.0
    } else {
        0.0
    };

    Some(Stats24hJson {
        volume,
        high: high as f64 / PRICE_SCALE as f64,
        low: low as f64 / PRICE_SCALE as f64,
        open: open_f,
        close: close_f,
        trade_count,
        change,
        change_percent,
    })
}

/// Decode a route from 96-byte blob
fn decode_route(data: &[u8]) -> Option<RouteJson> {
    if data.len() < 96 {
        return None;
    }

    let token_in = hex::encode(&data[0..32]);
    let token_out = hex::encode(&data[32..64]);
    let route_id = u64::from_le_bytes(data[64..72].try_into().ok()?);
    let route_type = match data[72] {
        0 => "clob",
        1 => "amm",
        2 => "split",
        3 => "multi_hop",
        _ => "legacy",
    };
    let pool_or_pair_id = u64::from_le_bytes(data[73..81].try_into().ok()?);
    let secondary_id = u64::from_le_bytes(data[81..89].try_into().ok()?);
    let split_percent = data[89];
    let enabled = data[90] == 1;

    Some(RouteJson {
        route_id,
        token_in,
        token_out,
        route_type,
        pool_or_pair_id,
        secondary_id,
        split_percent,
        enabled,
    })
}

/// Decode a proposal from 120-byte blob
fn decode_proposal(data: &[u8]) -> Option<ProposalJson> {
    if data.len() < 120 {
        return None;
    }

    let proposer = hex::encode(&data[0..32]);
    let proposal_id = u64::from_le_bytes(data[32..40].try_into().ok()?);
    let proposal_type = match data[40] {
        0 => "new_pair",
        1 => "fee_change",
        2 => "delist",
        _ => "param_change",
    };
    let status = match data[41] {
        0 => "active",
        1 => "passed",
        2 => "rejected",
        3 => "executed",
        _ => "cancelled",
    };
    let created_slot = u64::from_le_bytes(data[42..50].try_into().ok()?);
    let end_slot = u64::from_le_bytes(data[50..58].try_into().ok()?);
    let yes_votes = u64::from_le_bytes(data[58..66].try_into().ok()?);
    let no_votes = u64::from_le_bytes(data[66..74].try_into().ok()?);
    let pair_id = u64::from_le_bytes(data[74..82].try_into().ok()?);

    Some(ProposalJson {
        proposal_id,
        proposer,
        proposal_type,
        status,
        created_slot,
        end_slot,
        yes_votes,
        no_votes,
        pair_id,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Route Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// GET /api/v1/pairs — All trading pairs
async fn get_pairs(State(state): State<Arc<RpcState>>) -> Response {
    let count = read_u64(&state, DEX_CORE_PROGRAM, "dex_pair_count");
    let slot = current_slot(&state);
    let mut pairs = Vec::new();

    for i in 1..=count {
        let key = format!("dex_pair_{}", i);
        if let Some(data) = read_bytes(&state, DEX_CORE_PROGRAM, &key) {
            if let Some(pair) = decode_pair(&data) {
                pairs.push(pair);
            }
        }
    }

    ApiResponse::ok(pairs, slot).into_response()
}

/// GET /api/v1/pairs/:id — Pair details
async fn get_pair(State(state): State<Arc<RpcState>>, Path(pair_id): Path<u64>) -> Response {
    let key = format!("dex_pair_{}", pair_id);
    let slot = current_slot(&state);

    match read_bytes(&state, DEX_CORE_PROGRAM, &key) {
        Some(data) => match decode_pair(&data) {
            Some(pair) => ApiResponse::ok(pair, slot).into_response(),
            None => api_err("invalid pair data"),
        },
        None => api_not_found(&format!("pair {} not found", pair_id)),
    }
}

/// GET /api/v1/pairs/:id/orderbook — L2 order book
async fn get_orderbook(
    State(state): State<Arc<RpcState>>,
    Path(pair_id): Path<u64>,
    Query(q): Query<DepthQuery>,
) -> Response {
    let depth = q.depth.unwrap_or(20).min(100);
    let slot = current_slot(&state);

    // Read best bid/ask and build aggregated levels
    let mut bids: HashMap<u64, u64> = HashMap::new();
    let mut asks: HashMap<u64, u64> = HashMap::new();

    // Scan orders for this pair (up to reasonable limit)
    let order_count = read_u64(&state, DEX_CORE_PROGRAM, "dex_order_count");
    let scan_limit = order_count.min(10_000);

    for i in 1..=scan_limit {
        let key = format!("dex_order_{}", i);
        if let Some(data) = read_bytes(&state, DEX_CORE_PROGRAM, &key) {
            if let Some(order) = decode_order(&data) {
                if order.pair_id != pair_id {
                    continue;
                }
                if order.status != "open" && order.status != "partial" {
                    continue;
                }
                let remaining = order.quantity - order.filled;
                if remaining == 0 {
                    continue;
                }

                let price_entry = if order.side == "buy" {
                    &mut bids
                } else {
                    &mut asks
                };
                *price_entry.entry(order.price_raw).or_insert(0) += remaining;
            }
        }
    }

    // Sort and limit to depth
    let mut bid_levels: Vec<OrderBookLevel> = bids
        .into_iter()
        .map(|(p, q)| OrderBookLevel {
            price: p as f64 / PRICE_SCALE as f64,
            quantity: q,
            orders: 1,
        })
        .collect();
    bid_levels.sort_by(|a, b| {
        b.price
            .partial_cmp(&a.price)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    bid_levels.truncate(depth);

    let mut ask_levels: Vec<OrderBookLevel> = asks
        .into_iter()
        .map(|(p, q)| OrderBookLevel {
            price: p as f64 / PRICE_SCALE as f64,
            quantity: q,
            orders: 1,
        })
        .collect();
    ask_levels.sort_by(|a, b| {
        a.price
            .partial_cmp(&b.price)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ask_levels.truncate(depth);

    ApiResponse::ok(
        OrderBookJson {
            pair_id,
            bids: bid_levels,
            asks: ask_levels,
            slot,
        },
        slot,
    )
    .into_response()
}

/// GET /api/v1/pairs/:id/trades — Recent trades
async fn get_trades(
    State(state): State<Arc<RpcState>>,
    Path(pair_id): Path<u64>,
    Query(q): Query<LimitQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(50).min(200);
    let slot = current_slot(&state);
    let trade_count = read_u64(&state, DEX_CORE_PROGRAM, "dex_trade_count");

    let mut trades = Vec::new();
    // Read from most recent
    let start = trade_count.saturating_sub(limit as u64);
    for i in (start..trade_count).rev() {
        let key = format!("dex_trade_{}", i);
        if let Some(data) = read_bytes(&state, DEX_CORE_PROGRAM, &key) {
            if let Some(trade) = decode_trade(&data) {
                if trade.pair_id == pair_id {
                    trades.push(trade);
                    if trades.len() >= limit {
                        break;
                    }
                }
            }
        }
    }

    ApiResponse::ok(trades, slot).into_response()
}

/// GET /api/v1/pairs/:id/candles — OHLCV candles
async fn get_candles(
    State(state): State<Arc<RpcState>>,
    Path(pair_id): Path<u64>,
    Query(q): Query<CandleQuery>,
) -> Response {
    let interval = q.interval.unwrap_or(3600);
    let limit = q.limit.unwrap_or(100).min(500);
    let slot = current_slot(&state);

    let count_key = format!("ana_cc_{}_{}", pair_id, interval);
    let candle_count = read_u64(&state, DEX_ANALYTICS_PROGRAM, &count_key);

    let mut candles = Vec::new();
    let start = candle_count.saturating_sub(limit as u64);

    for i in start..candle_count {
        let key = format!("ana_c_{}_{}_{}", pair_id, interval, i);
        if let Some(data) = read_bytes(&state, DEX_ANALYTICS_PROGRAM, &key) {
            if let Some(candle) = decode_candle(&data) {
                candles.push(candle);
            }
        }
    }

    ApiResponse::ok(candles, slot).into_response()
}

/// GET /api/v1/pairs/:id/stats — 24h rolling stats
async fn get_pair_stats(State(state): State<Arc<RpcState>>, Path(pair_id): Path<u64>) -> Response {
    let slot = current_slot(&state);
    let key = format!("ana_24h_{}", pair_id);

    match read_bytes(&state, DEX_ANALYTICS_PROGRAM, &key) {
        Some(data) => match decode_stats_24h(&data) {
            Some(stats) => ApiResponse::ok(stats, slot).into_response(),
            None => ApiResponse::ok(
                Stats24hJson {
                    volume: 0,
                    high: 0.0,
                    low: 0.0,
                    open: 0.0,
                    close: 0.0,
                    trade_count: 0,
                    change: 0.0,
                    change_percent: 0.0,
                },
                slot,
            )
            .into_response(),
        },
        None => ApiResponse::ok(
            Stats24hJson {
                volume: 0,
                high: 0.0,
                low: 0.0,
                open: 0.0,
                close: 0.0,
                trade_count: 0,
                change: 0.0,
                change_percent: 0.0,
            },
            slot,
        )
        .into_response(),
    }
}

/// GET /api/v1/pairs/:id/ticker — Ticker summary
async fn get_pair_ticker(State(state): State<Arc<RpcState>>, Path(pair_id): Path<u64>) -> Response {
    let slot = current_slot(&state);

    let last_price_key = format!("ana_lp_{}", pair_id);
    let last_price_raw = read_u64(&state, DEX_ANALYTICS_PROGRAM, &last_price_key);
    let last_price = last_price_raw as f64 / PRICE_SCALE as f64;

    let best_bid_raw = read_u64(
        &state,
        DEX_CORE_PROGRAM,
        &format!("dex_best_bid_{}", pair_id),
    );
    let best_ask_raw = read_u64(
        &state,
        DEX_CORE_PROGRAM,
        &format!("dex_best_ask_{}", pair_id),
    );

    let stats_key = format!("ana_24h_{}", pair_id);
    let volume_24h = match read_bytes(&state, DEX_ANALYTICS_PROGRAM, &stats_key) {
        Some(data) if data.len() >= 8 => {
            u64::from_le_bytes(data[0..8].try_into().unwrap_or([0; 8]))
        }
        _ => 0,
    };

    let open_24h = match read_bytes(&state, DEX_ANALYTICS_PROGRAM, &stats_key) {
        Some(data) if data.len() >= 32 => {
            let open = u64::from_le_bytes(data[24..32].try_into().unwrap_or([0; 8])) as f64
                / PRICE_SCALE as f64;
            if open > 0.0 {
                ((last_price - open) / open) * 100.0
            } else {
                0.0
            }
        }
        _ => 0.0,
    };

    ApiResponse::ok(
        TickerJson {
            pair_id,
            last_price,
            bid: best_bid_raw as f64 / PRICE_SCALE as f64,
            ask: best_ask_raw as f64 / PRICE_SCALE as f64,
            volume_24h,
            change_24h: open_24h,
        },
        slot,
    )
    .into_response()
}

/// GET /api/v1/tickers — All tickers
async fn get_all_tickers(State(state): State<Arc<RpcState>>) -> Response {
    let count = read_u64(&state, DEX_CORE_PROGRAM, "dex_pair_count");
    let slot = current_slot(&state);
    let mut tickers = Vec::new();

    for pair_id in 0..count {
        let last_price_raw = read_u64(
            &state,
            DEX_ANALYTICS_PROGRAM,
            &format!("ana_lp_{}", pair_id),
        );
        let best_bid_raw = read_u64(
            &state,
            DEX_CORE_PROGRAM,
            &format!("dex_best_bid_{}", pair_id),
        );
        let best_ask_raw = read_u64(
            &state,
            DEX_CORE_PROGRAM,
            &format!("dex_best_ask_{}", pair_id),
        );

        tickers.push(TickerJson {
            pair_id,
            last_price: last_price_raw as f64 / PRICE_SCALE as f64,
            bid: best_bid_raw as f64 / PRICE_SCALE as f64,
            ask: best_ask_raw as f64 / PRICE_SCALE as f64,
            volume_24h: 0,
            change_24h: 0.0,
        });
    }

    ApiResponse::ok(tickers, slot).into_response()
}

/// POST /api/v1/orders — Place order
async fn post_order(
    State(state): State<Arc<RpcState>>,
    Json(body): Json<PlaceOrderBody>,
) -> Response {
    let slot = current_slot(&state);

    // Validate
    if body.quantity == 0 {
        return api_err("quantity must be > 0");
    }
    if body.side != "buy" && body.side != "sell" {
        return api_err("side must be 'buy' or 'sell'");
    }

    // Build contract calldata: opcode 0x03 = place_order
    let pair_id = match &body.pair {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0),
        serde_json::Value::String(s) => {
            // TODO: resolve symbol → pair_id from a registry
            s.parse::<u64>().unwrap_or(0)
        }
        _ => 0,
    };

    // Return the order as it would be created (actual execution via sendTransaction)
    let price_raw = (body.price * PRICE_SCALE as f64) as u64;
    let order_count = read_u64(&state, DEX_CORE_PROGRAM, "dex_order_count");

    let order = OrderJson {
        order_id: order_count,
        trader: String::new(), // Filled from tx signer
        pair_id,
        side: if body.side == "buy" { "buy" } else { "sell" },
        order_type: match body.order_type.as_str() {
            "market" => "market",
            "stop-limit" => "stop-limit",
            "post-only" => "post-only",
            _ => "limit",
        },
        price: body.price,
        price_raw,
        quantity: body.quantity,
        filled: 0,
        status: "open",
        created_slot: slot,
        expiry_slot: body.expiry.unwrap_or(0),
    };

    (StatusCode::CREATED, ApiResponse::ok(order, slot)).into_response()
}

/// DELETE /api/v1/orders/:id — Cancel order
async fn delete_order(State(state): State<Arc<RpcState>>, Path(order_id): Path<u64>) -> Response {
    let slot = current_slot(&state);
    let key = format!("dex_order_{}", order_id);

    match read_bytes(&state, DEX_CORE_PROGRAM, &key) {
        Some(_data) => {
            // Actual cancellation would go through sendTransaction
            let body = serde_json::json!({ "cancelled": true, "order_id": order_id });
            ApiResponse::ok(body, slot).into_response()
        }
        None => api_not_found(&format!("order {} not found", order_id)),
    }
}

/// GET /api/v1/orders?trader=<addr> — List orders
async fn get_orders(State(state): State<Arc<RpcState>>, Query(q): Query<TraderQuery>) -> Response {
    let slot = current_slot(&state);

    let trader_hex = match &q.trader {
        Some(t) => t.to_lowercase(),
        None => return api_err("trader parameter required"),
    };

    // Read user's order count and order IDs
    let count_key = format!("dex_uoc_{}", trader_hex);
    let count = read_u64(&state, DEX_CORE_PROGRAM, &count_key);

    let mut orders = Vec::new();
    for i in 1..=count.min(200) {
        let idx_key = format!("dex_uo_{}_{}", trader_hex, i);
        let order_id = read_u64(&state, DEX_CORE_PROGRAM, &idx_key);
        let key = format!("dex_order_{}", order_id);
        if let Some(data) = read_bytes(&state, DEX_CORE_PROGRAM, &key) {
            if let Some(order) = decode_order(&data) {
                // Filter by status if requested
                if let Some(ref status) = q.status {
                    if status != "all" && order.status != status.as_str() {
                        continue;
                    }
                }
                // Filter by pair if requested
                if let Some(pid) = q.pair_id {
                    if order.pair_id != pid {
                        continue;
                    }
                }
                orders.push(order);
            }
        }
    }

    ApiResponse::ok(orders, slot).into_response()
}

/// GET /api/v1/orders/:id — Get specific order
async fn get_order(State(state): State<Arc<RpcState>>, Path(order_id): Path<u64>) -> Response {
    let slot = current_slot(&state);
    let key = format!("dex_order_{}", order_id);

    match read_bytes(&state, DEX_CORE_PROGRAM, &key) {
        Some(data) => match decode_order(&data) {
            Some(order) => ApiResponse::ok(order, slot).into_response(),
            None => api_err("invalid order data"),
        },
        None => api_not_found(&format!("order {} not found", order_id)),
    }
}

// ─── POOLS ──────────────────────────────────────────────────────────────────

/// GET /api/v1/pools — All AMM pools
async fn get_pools(State(state): State<Arc<RpcState>>) -> Response {
    let count = read_u64(&state, DEX_AMM_PROGRAM, "amm_pool_count");
    let slot = current_slot(&state);
    let mut pools = Vec::new();

    for i in 1..=count {
        let key = format!("amm_pool_{}", i);
        if let Some(data) = read_bytes(&state, DEX_AMM_PROGRAM, &key) {
            if let Some(pool) = decode_pool(&data) {
                pools.push(pool);
            }
        }
    }

    ApiResponse::ok(pools, slot).into_response()
}

/// GET /api/v1/pools/:id — Pool details
async fn get_pool(State(state): State<Arc<RpcState>>, Path(pool_id): Path<u64>) -> Response {
    let slot = current_slot(&state);
    let key = format!("amm_pool_{}", pool_id);

    match read_bytes(&state, DEX_AMM_PROGRAM, &key) {
        Some(data) => match decode_pool(&data) {
            Some(pool) => ApiResponse::ok(pool, slot).into_response(),
            None => api_err("invalid pool data"),
        },
        None => api_not_found(&format!("pool {} not found", pool_id)),
    }
}

/// GET /api/v1/pools/positions?owner=<addr> — LP positions
async fn get_lp_positions(
    State(state): State<Arc<RpcState>>,
    Query(q): Query<OwnerQuery>,
) -> Response {
    let slot = current_slot(&state);

    let owner_hex = match &q.owner {
        Some(o) => o.to_lowercase(),
        None => return api_err("owner parameter required"),
    };

    let count_key = format!("amm_opc_{}", owner_hex);
    let count = read_u64(&state, DEX_AMM_PROGRAM, &count_key);

    let mut positions = Vec::new();
    for i in 1..=count.min(100) {
        let idx_key = format!("amm_op_{}_{}", owner_hex, i);
        let pos_id = read_u64(&state, DEX_AMM_PROGRAM, &idx_key);
        let key = format!("amm_pos_{}", pos_id);
        if let Some(data) = read_bytes(&state, DEX_AMM_PROGRAM, &key) {
            if let Some(pos) = decode_lp_position(&data, pos_id) {
                positions.push(pos);
            }
        }
    }

    ApiResponse::ok(positions, slot).into_response()
}

// ─── AMM MATH (mirrors contracts/dex_amm compute_swap_output) ──────────────

/// Fee tiers in basis points — must match dex_amm FEE_VALUES
const AMM_FEE_BPS: [u64; 4] = [1, 5, 30, 100];

/// Compute swap output amount given input and pool state.
/// Replicates the exact Uniswap V3-style math from contracts/dex_amm.
fn compute_swap_output_rpc(
    amount_in: u64,
    liquidity: u64,
    sqrt_price: u64,
    fee_bps: u64,
    is_token_a: bool,
) -> (u64, u64) {
    if liquidity == 0 || amount_in == 0 {
        return (0, sqrt_price);
    }

    // Apply fee
    let fee = (amount_in as u128 * fee_bps as u128 / 10_000) as u64;
    let amount_after_fee = amount_in.saturating_sub(fee);

    if is_token_a {
        // Swapping A for B: price decreases
        // new_sqrt = L * sqrt_p / (L + amount * sqrt_p / 2^32)
        let numerator = liquidity as u128 * sqrt_price as u128;
        let denominator = liquidity as u128
            + (amount_after_fee as u128 * sqrt_price as u128 / (1u128 << 32));
        if denominator == 0 {
            return (0, sqrt_price);
        }
        let new_sqrt = (numerator / denominator) as u64;
        // amount_b_out = L * (sqrt_p - new_sqrt) / 2^32
        let delta_sqrt = sqrt_price as u128 - new_sqrt as u128;
        let amount_out = (liquidity as u128 * delta_sqrt / (1u128 << 32)) as u64;
        (amount_out, new_sqrt)
    } else {
        // Swapping B for A: price increases
        // new_sqrt = sqrt_p + amount * 2^32 / L
        let delta = amount_after_fee as u128 * (1u128 << 32) / liquidity as u128;
        let new_sqrt = (sqrt_price as u128 + delta) as u64;
        // amount_a_out = L * (new_sqrt - sqrt_p) / (sqrt_p * new_sqrt / 2^32)
        let delta_sqrt = new_sqrt as u128 - sqrt_price as u128;
        let denom = sqrt_price as u128 * new_sqrt as u128 / (1u128 << 32);
        let amount_out = if denom == 0 {
            0
        } else {
            (liquidity as u128 * delta_sqrt / denom) as u64
        };
        (amount_out, new_sqrt)
    }
}

/// Compute a swap quote through an AMM pool, reading pool state from storage.
/// Returns (amount_out, price_impact) or None if pool not found.
fn quote_amm_swap(
    state: &crate::RpcState,
    pool_id: u64,
    token_in: &str,
    amount_in: u64,
) -> Option<(u64, f64)> {
    let key = format!("amm_pool_{}", pool_id);
    let data = read_bytes(state, DEX_AMM_PROGRAM, &key)?;
    if data.len() < 96 {
        return None;
    }
    let token_a = hex::encode(&data[0..32]);
    let token_b = hex::encode(&data[32..64]);
    let sqrt_price = u64::from_le_bytes(data[72..80].try_into().ok()?);
    let liquidity = u64::from_le_bytes(data[84..92].try_into().ok()?);
    let fee_tier_idx = data[92] as usize;
    let fee_bps = if fee_tier_idx < AMM_FEE_BPS.len() {
        AMM_FEE_BPS[fee_tier_idx]
    } else {
        AMM_FEE_BPS[3] // default 100bps
    };

    let token_in_lower = token_in.to_lowercase();
    let is_token_a = token_in_lower == token_a;
    if !is_token_a && token_in_lower != token_b {
        return None; // token_in doesn't match either side of the pool
    }

    let (amount_out, new_sqrt) =
        compute_swap_output_rpc(amount_in, liquidity, sqrt_price, fee_bps, is_token_a);

    // Price impact = |1 - new_price / old_price| as percentage
    // sqrt_price is Q32.32, actual price = (sqrt_price / 2^32)^2
    // price_impact ≈ |1 - (new_sqrt / sqrt_price)^2| * 100
    let price_impact = if sqrt_price > 0 {
        let ratio = new_sqrt as f64 / sqrt_price as f64;
        ((1.0 - ratio * ratio).abs() * 100.0 * 100.0).round() / 100.0 // round to 2 decimals
    } else {
        0.0
    };

    Some((amount_out, price_impact))
}

// ─── ROUTER ─────────────────────────────────────────────────────────────────

/// POST /api/v1/router/swap — Smart-routed swap using real AMM pricing
async fn post_router_swap(
    State(state): State<Arc<RpcState>>,
    Json(body): Json<SwapBody>,
) -> Response {
    let slot = current_slot(&state);

    if body.amount_in == 0 {
        return api_err("amountIn must be > 0");
    }
    if body.slippage < 0.0 || body.slippage > 50.0 {
        return api_err("slippage must be 0-50%");
    }

    let token_in = body.token_in.to_lowercase();

    // Find the best route for this token pair
    let route_count = read_u64(&state, DEX_ROUTER_PROGRAM, "rtr_route_count");
    let mut best_route: Option<RouteJson> = None;
    let mut best_output: u64 = 0;
    let mut best_impact: f64 = 0.0;

    for i in 1..=route_count {
        let key = format!("rtr_route_{}", i);
        if let Some(data) = read_bytes(&state, DEX_ROUTER_PROGRAM, &key) {
            if let Some(route) = decode_route(&data) {
                if !route.enabled {
                    continue;
                }
                // Match token pair (both directions)
                let route_in = route.token_in.to_lowercase();
                let route_out = route.token_out.to_lowercase();
                let body_out = body.token_out.to_lowercase();
                if !((route_in == token_in && route_out == body_out)
                    || (route_out == token_in && route_in == body_out))
                {
                    continue;
                }

                // Quote through AMM pool if route type is AMM
                if route.route_type == "amm" {
                    if let Some((amount_out, impact)) =
                        quote_amm_swap(&state, route.pool_or_pair_id, &token_in, body.amount_in)
                    {
                        if amount_out > best_output {
                            best_output = amount_out;
                            best_impact = impact;
                            best_route = Some(route);
                        }
                    }
                } else {
                    // CLOB or other route types — fall back to 1:1 quote if no AMM available
                    if best_route.is_none() {
                        best_route = Some(route);
                    }
                }
            }
        }
    }

    // Fallback: if no explicit route found, scan all AMM pools for a matching pair
    if best_route.is_none() {
        let pool_count = read_u64(&state, DEX_AMM_PROGRAM, "amm_pool_count");
        for pid in 0..pool_count {
            let pk = format!("amm_pool_{}", pid);
            if let Some(data) = read_bytes(&state, DEX_AMM_PROGRAM, &pk) {
                if data.len() >= 96 {
                    let ta = hex::encode(&data[0..32]);
                    let tb = hex::encode(&data[32..64]);
                    let body_out = body.token_out.to_lowercase();
                    if (ta == token_in && tb == body_out)
                        || (tb == token_in && ta == body_out)
                    {
                        if let Some((amount_out, impact)) =
                            quote_amm_swap(&state, pid, &token_in, body.amount_in)
                        {
                            if amount_out > best_output {
                                best_output = amount_out;
                                best_impact = impact;
                                best_route = Some(RouteJson {
                                    route_id: pid,
                                    token_in: token_in.clone(),
                                    token_out: body_out.clone(),
                                    route_type: "amm",
                                    pool_or_pair_id: pid,
                                    secondary_id: 0,
                                    split_percent: 100,
                                    enabled: true,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    match best_route {
        Some(route) => {
            // Check slippage tolerance
            let min_out = (body.amount_in as f64 * (1.0 - body.slippage / 100.0)) as u64;
            if best_output > 0 && best_output < min_out {
                return api_err(&format!(
                    "slippage exceeded: output {} below minimum {}",
                    best_output, min_out
                ));
            }

            let result = serde_json::json!({
                "amountIn": body.amount_in,
                "amountOut": best_output,
                "routeType": route.route_type,
                "routeId": route.route_id,
                "poolId": route.pool_or_pair_id,
                "priceImpact": best_impact,
                "slot": slot,
            });
            ApiResponse::ok(result, slot).into_response()
        }
        None => api_err("no route found for this token pair"),
    }
}

/// POST /api/v1/router/quote — Swap quote (read-only, same AMM pricing)
async fn post_router_quote(
    State(state): State<Arc<RpcState>>,
    Json(body): Json<SwapBody>,
) -> Response {
    // Same logic as swap — both are read-only queries against on-chain state
    post_router_swap(State(state), Json(body)).await
}

/// GET /api/v1/routes — All routes
async fn get_routes(State(state): State<Arc<RpcState>>) -> Response {
    let count = read_u64(&state, DEX_ROUTER_PROGRAM, "rtr_route_count");
    let slot = current_slot(&state);
    let mut routes = Vec::new();

    for i in 1..=count {
        let key = format!("rtr_route_{}", i);
        if let Some(data) = read_bytes(&state, DEX_ROUTER_PROGRAM, &key) {
            if let Some(route) = decode_route(&data) {
                routes.push(route);
            }
        }
    }

    ApiResponse::ok(routes, slot).into_response()
}

// ─── MARGIN ─────────────────────────────────────────────────────────────────

/// POST /api/v1/margin/open — Open margin position
async fn post_margin_open(
    State(state): State<Arc<RpcState>>,
    Json(body): Json<OpenPositionBody>,
) -> Response {
    let slot = current_slot(&state);

    if body.leverage == 0 || body.leverage > 5 {
        return api_err("leverage must be 1-5");
    }
    if body.margin == 0 {
        return api_err("margin must be > 0");
    }

    let pair_id = match &body.pair {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0),
        serde_json::Value::String(s) => s.parse::<u64>().unwrap_or(0),
        _ => 0,
    };

    let pos_count = read_u64(&state, DEX_MARGIN_PROGRAM, "mrg_pos_count");

    let position = MarginPositionJson {
        position_id: pos_count,
        trader: String::new(),
        pair_id,
        side: if body.side == "short" {
            "short"
        } else {
            "long"
        },
        status: "open",
        size: body.margin * body.leverage,
        margin: body.margin,
        entry_price: 0.0, // Filled from mark price
        entry_price_raw: 0,
        leverage: body.leverage,
        created_slot: slot,
        realized_pnl: 0,
        accumulated_funding: 0,
    };

    (StatusCode::CREATED, ApiResponse::ok(position, slot)).into_response()
}

/// POST /api/v1/margin/close — Close margin position
async fn post_margin_close(
    State(state): State<Arc<RpcState>>,
    Json(body): Json<ClosePositionBody>,
) -> Response {
    let slot = current_slot(&state);
    let key = format!("mrg_pos_{}", body.position_id);

    match read_bytes(&state, DEX_MARGIN_PROGRAM, &key) {
        Some(data) => match decode_margin_position(&data) {
            Some(pos) => ApiResponse::ok(pos, slot).into_response(),
            None => api_err("invalid position data"),
        },
        None => api_not_found(&format!("position {} not found", body.position_id)),
    }
}

/// GET /api/v1/margin/positions?trader=<addr> — Margin positions
async fn get_margin_positions(
    State(state): State<Arc<RpcState>>,
    Query(q): Query<TraderQuery>,
) -> Response {
    let slot = current_slot(&state);

    let trader_hex = match &q.trader {
        Some(t) => t.to_lowercase(),
        None => return api_err("trader parameter required"),
    };

    let count_key = format!("mrg_upc_{}", trader_hex);
    let count = read_u64(&state, DEX_MARGIN_PROGRAM, &count_key);

    let mut positions = Vec::new();
    for i in 1..=count.min(100) {
        let idx_key = format!("mrg_up_{}_{}", trader_hex, i);
        let pos_id = read_u64(&state, DEX_MARGIN_PROGRAM, &idx_key);
        let key = format!("mrg_pos_{}", pos_id);
        if let Some(data) = read_bytes(&state, DEX_MARGIN_PROGRAM, &key) {
            if let Some(pos) = decode_margin_position(&data) {
                positions.push(pos);
            }
        }
    }

    ApiResponse::ok(positions, slot).into_response()
}

/// GET /api/v1/margin/positions/:id — Get specific position
async fn get_margin_position(
    State(state): State<Arc<RpcState>>,
    Path(position_id): Path<u64>,
) -> Response {
    let slot = current_slot(&state);
    let key = format!("mrg_pos_{}", position_id);

    match read_bytes(&state, DEX_MARGIN_PROGRAM, &key) {
        Some(data) => match decode_margin_position(&data) {
            Some(pos) => ApiResponse::ok(pos, slot).into_response(),
            None => api_err("invalid position data"),
        },
        None => api_not_found(&format!("position {} not found", position_id)),
    }
}

/// GET /api/v1/margin/info — Margin system info
async fn get_margin_info(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);

    let info = MarginInfoJson {
        insurance_fund: read_u64(&state, DEX_MARGIN_PROGRAM, "mrg_insurance"),
        last_funding_slot: read_u64(&state, DEX_MARGIN_PROGRAM, "mrg_last_fund"),
        maintenance_bps: read_u64(&state, DEX_MARGIN_PROGRAM, "mrg_maint_bps"),
        position_count: read_u64(&state, DEX_MARGIN_PROGRAM, "mrg_pos_count"),
    };

    ApiResponse::ok(info, slot).into_response()
}

// ─── ANALYTICS ──────────────────────────────────────────────────────────────

/// GET /api/v1/leaderboard — Top traders
async fn get_leaderboard(
    State(state): State<Arc<RpcState>>,
    Query(q): Query<LimitQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(20).min(100);
    let slot = current_slot(&state);
    let mut entries = Vec::new();

    for rank in 0..limit as u32 {
        let key = format!("ana_lb_{}", rank);
        if let Some(addr_data) = read_bytes(&state, DEX_ANALYTICS_PROGRAM, &key) {
            if addr_data.len() >= 32 {
                let addr_hex = hex::encode(&addr_data[..32]);
                let stats_key = format!("ana_ts_{}", addr_hex);
                let (volume, trade_count, pnl) =
                    match read_bytes(&state, DEX_ANALYTICS_PROGRAM, &stats_key) {
                        Some(data) if data.len() >= 32 => {
                            let vol = u64::from_le_bytes(data[0..8].try_into().unwrap_or([0; 8]));
                            let tc = u64::from_le_bytes(data[8..16].try_into().unwrap_or([0; 8]));
                            let raw_pnl =
                                u64::from_le_bytes(data[16..24].try_into().unwrap_or([0; 8]));
                            (vol, tc, raw_pnl as i64 - PNL_BIAS as i64)
                        }
                        _ => (0, 0, 0),
                    };

                entries.push(LeaderboardEntryJson {
                    rank: rank + 1,
                    address: addr_hex,
                    total_volume: volume,
                    trade_count,
                    total_pnl: pnl,
                });
            }
        }
    }

    ApiResponse::ok(entries, slot).into_response()
}

// ─── REWARDS ────────────────────────────────────────────────────────────────

/// GET /api/v1/rewards/:addr — Pending rewards
async fn get_rewards(State(state): State<Arc<RpcState>>, Path(addr): Path<String>) -> Response {
    let slot = current_slot(&state);
    let addr_hex = addr.to_lowercase();

    let info = RewardInfoJson {
        pending: read_u64(
            &state,
            DEX_REWARDS_PROGRAM,
            &format!("rew_pend_{}", addr_hex),
        ),
        claimed: read_u64(
            &state,
            DEX_REWARDS_PROGRAM,
            &format!("rew_claim_{}", addr_hex),
        ),
        total_volume: read_u64(
            &state,
            DEX_REWARDS_PROGRAM,
            &format!("rew_vol_{}", addr_hex),
        ),
        referral_count: read_u64(
            &state,
            DEX_REWARDS_PROGRAM,
            &format!("rew_refc_{}", addr_hex),
        ),
        referral_earnings: read_u64(
            &state,
            DEX_REWARDS_PROGRAM,
            &format!("rew_refr_{}", addr_hex),
        ),
    };

    ApiResponse::ok(info, slot).into_response()
}

// ─── GOVERNANCE ─────────────────────────────────────────────────────────────

/// GET /api/v1/governance/proposals — All proposals
async fn get_proposals(
    State(state): State<Arc<RpcState>>,
    Query(q): Query<StatusQuery>,
) -> Response {
    let count = read_u64(&state, DEX_GOVERNANCE_PROGRAM, "gov_prop_count");
    let slot = current_slot(&state);
    let mut proposals = Vec::new();

    for i in 1..=count {
        let key = format!("gov_prop_{}", i);
        if let Some(data) = read_bytes(&state, DEX_GOVERNANCE_PROGRAM, &key) {
            if let Some(p) = decode_proposal(&data) {
                if let Some(ref status) = q.status {
                    if p.status != status.as_str() {
                        continue;
                    }
                }
                proposals.push(p);
            }
        }
    }

    ApiResponse::ok(proposals, slot).into_response()
}

/// GET /api/v1/governance/proposals/:id — Proposal details
async fn get_proposal(
    State(state): State<Arc<RpcState>>,
    Path(proposal_id): Path<u64>,
) -> Response {
    let slot = current_slot(&state);
    let key = format!("gov_prop_{}", proposal_id);

    match read_bytes(&state, DEX_GOVERNANCE_PROGRAM, &key) {
        Some(data) => match decode_proposal(&data) {
            Some(p) => ApiResponse::ok(p, slot).into_response(),
            None => api_err("invalid proposal data"),
        },
        None => api_not_found(&format!("proposal {} not found", proposal_id)),
    }
}

// ─── TRADERS ────────────────────────────────────────────────────────────────

/// GET /api/v1/traders/:addr/stats — Trader stats
async fn get_trader_stats(
    State(state): State<Arc<RpcState>>,
    Path(addr): Path<String>,
) -> Response {
    let slot = current_slot(&state);
    let addr_hex = addr.to_lowercase();
    let key = format!("ana_ts_{}", addr_hex);

    let (volume, trade_count, pnl) = match read_bytes(&state, DEX_ANALYTICS_PROGRAM, &key) {
        Some(data) if data.len() >= 32 => {
            let vol = u64::from_le_bytes(data[0..8].try_into().unwrap_or([0; 8]));
            let tc = u64::from_le_bytes(data[8..16].try_into().unwrap_or([0; 8]));
            let raw_pnl = u64::from_le_bytes(data[16..24].try_into().unwrap_or([0; 8]));
            (vol, tc, raw_pnl as i64 - PNL_BIAS as i64)
        }
        _ => (0, 0, 0),
    };

    ApiResponse::ok(
        LeaderboardEntryJson {
            rank: 0,
            address: addr_hex,
            total_volume: volume,
            trade_count,
            total_pnl: pnl,
        },
        slot,
    )
    .into_response()
}

// ═══════════════════════════════════════════════════════════════════════════════
// PUBLIC: Build the DEX API router
// ═══════════════════════════════════════════════════════════════════════════════

/// Build the /api/v1 DEX REST router. Call from build_rpc_router() in lib.rs.
pub(crate) fn build_dex_router() -> Router<Arc<RpcState>> {
    Router::new()
        // Pairs
        .route("/pairs", get(get_pairs))
        .route("/pairs/:id", get(get_pair))
        .route("/pairs/:id/orderbook", get(get_orderbook))
        .route("/pairs/:id/trades", get(get_trades))
        .route("/pairs/:id/candles", get(get_candles))
        .route("/pairs/:id/stats", get(get_pair_stats))
        .route("/pairs/:id/ticker", get(get_pair_ticker))
        // Tickers
        .route("/tickers", get(get_all_tickers))
        // Orders
        .route("/orders", get(get_orders).post(post_order))
        .route("/orders/:id", get(get_order).delete(delete_order))
        // Router / Swaps
        .route("/router/swap", post(post_router_swap))
        .route("/router/quote", post(post_router_quote))
        .route("/routes", get(get_routes))
        // Pools
        .route("/pools", get(get_pools))
        .route("/pools/:id", get(get_pool))
        .route("/pools/positions", get(get_lp_positions))
        // Margin
        .route("/margin/open", post(post_margin_open))
        .route("/margin/close", post(post_margin_close))
        .route("/margin/positions", get(get_margin_positions))
        .route("/margin/positions/:id", get(get_margin_position))
        .route("/margin/info", get(get_margin_info))
        // Analytics
        .route("/leaderboard", get(get_leaderboard))
        .route("/traders/:addr/stats", get(get_trader_stats))
        // Rewards
        .route("/rewards/:addr", get(get_rewards))
        // Governance
        .route("/governance/proposals", get(get_proposals))
        .route("/governance/proposals/:id", get(get_proposal))
}
