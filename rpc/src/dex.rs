// ═══════════════════════════════════════════════════════════════════════════════
// Lichen RPC — DEX REST API Module
// Implements /api/v1/* endpoints for Lichen DEX
//
// Reads DEX state through the shared read-schema in lichen_core::dex and shapes
// the results into REST responses.
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
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use crate::RpcState;
use lichen_core::{account::Pubkey, dex as core_dex};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const PRICE_SCALE: u64 = core_dex::PRICE_SCALE;
const PNL_BIAS: u64 = core_dex::PNL_BIAS;
const SLOT_DURATION_MS: u64 = core_dex::SLOT_DURATION_MS;

const DEX_CORE_PROGRAM: &str = core_dex::DEX_CORE_PROGRAM;
const DEX_AMM_PROGRAM: &str = core_dex::DEX_AMM_PROGRAM;
const DEX_MARGIN_PROGRAM: &str = core_dex::DEX_MARGIN_PROGRAM;
const DEX_ANALYTICS_PROGRAM: &str = core_dex::DEX_ANALYTICS_PROGRAM;
const DEX_ROUTER_PROGRAM: &str = core_dex::DEX_ROUTER_PROGRAM;
const DEX_REWARDS_PROGRAM: &str = core_dex::DEX_REWARDS_PROGRAM;
const DEX_GOVERNANCE_PROGRAM: &str = core_dex::DEX_GOVERNANCE_PROGRAM;

const DEX_PAIR_COUNT_KEY: &str = core_dex::DEX_PAIR_COUNT_KEY;
const DEX_ORDER_COUNT_KEY: &str = core_dex::DEX_ORDER_COUNT_KEY;
const DEX_TRADE_COUNT_KEY: &str = core_dex::DEX_TRADE_COUNT_KEY;
const AMM_POOL_COUNT_KEY: &str = core_dex::AMM_POOL_COUNT_KEY;
const ROUTER_ROUTE_COUNT_KEY: &str = core_dex::ROUTER_ROUTE_COUNT_KEY;
const MARGIN_POSITION_COUNT_KEY: &str = core_dex::MARGIN_POSITION_COUNT_KEY;
const GOVERNANCE_PROPOSAL_COUNT_KEY: &str = core_dex::GOVERNANCE_PROPOSAL_COUNT_KEY;

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

    /// Return a pre-built JSON value wrapped in the standard API envelope.
    fn ok_raw(data: T, slot: u64) -> Json<ApiResponse<T>> {
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

fn api_method_not_allowed(msg: &str) -> Response {
    let body = ApiResponse::<()> {
        success: false,
        data: None,
        error: Some(msg.to_string()),
        slot: 0,
    };
    (StatusCode::METHOD_NOT_ALLOWED, Json(body)).into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// Data Structures (JSON representations of on-chain data)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TradingPairJson {
    pub pair_id: u64,
    pub base_token: String,
    pub quote_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_symbol: Option<String>,
    pub tick_size: u64,
    pub lot_size: u64,
    pub min_order: u64,
    pub status: &'static str,
    pub maker_fee_bps: i16,
    pub taker_fee_bps: u16,
    pub daily_volume: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_price: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_24h: Option<f64>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
pub struct TradeJson {
    pub trade_id: u64,
    pub pair_id: u64,
    pub price: f64,
    pub price_raw: u64,
    pub quantity: u64,
    pub taker: String,
    pub maker_order_id: u64,
    pub slot: u64,
    pub side: &'static str,
    pub timestamp: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OrderBookLevel {
    pub price: f64,
    pub quantity: u64,
    pub orders: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderBookJson {
    pub pair_id: u64,
    pub bids: Vec<OrderBookLevel>,
    pub asks: Vec<OrderBookLevel>,
    pub slot: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PoolJson {
    pub pool_id: u64,
    pub token_a: String,
    pub token_b: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_a_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_b_symbol: Option<String>,
    pub sqrt_price: u64,
    pub price: f64,
    pub tick: i32,
    pub liquidity: u64,
    pub fee_tier: &'static str,
    pub protocol_fee: u8,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
pub struct MarginPositionJson {
    pub position_id: u64,
    pub trader: String,
    pub pair_id: u64,
    pub side: &'static str,
    pub margin_type: &'static str,
    pub status: &'static str,
    pub size: u64,
    pub margin: u64,
    pub entry_price: f64,
    pub entry_price_raw: u64,
    pub leverage: u64,
    pub created_slot: u64,
    pub realized_pnl: i64,
    pub accumulated_funding: u64,
    pub mark_price: f64,
    pub sl_price: u64,
    pub tp_price: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MarginInfoJson {
    pub insurance_fund: u64,
    pub last_funding_slot: u64,
    pub maintenance_bps: u64,
    pub position_count: u64,
    pub max_leverage: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FundingRateJson {
    pub base_rate_bps: u64,
    pub interval_hours: u64,
    pub max_rate_bps: u64,
    pub tiers: Vec<FundingTierJson>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FundingTierJson {
    pub max_leverage: u64,
    pub multiplier_x10: u64,
    pub effective_rate_bps: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CandleJson {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: u64,
    pub slot: u64,
    pub timestamp: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TickerJson {
    pub pair_id: u64,
    pub last_price: f64,
    pub bid: f64,
    pub ask: f64,
    pub volume_24h: u64,
    pub change_24h: f64,
    pub high_24h: f64,
    pub low_24h: f64,
    pub trades_24h: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaderboardEntryJson {
    pub rank: u32,
    pub address: String,
    pub total_volume: u64,
    pub trade_count: u64,
    pub total_pnl: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardInfoJson {
    pub pending: u64,
    pub claimed: u64,
    pub total_volume: u64,
    pub referral_count: u64,
    pub referral_earnings: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
    pub base_token: Option<String>,
    pub new_maker_fee: Option<i16>,
    pub new_taker_fee: Option<u16>,
}

impl From<core_dex::DexTradingPair> for TradingPairJson {
    fn from(value: core_dex::DexTradingPair) -> Self {
        TradingPairJson {
            pair_id: value.pair_id,
            base_token: value.base_token,
            quote_token: value.quote_token,
            symbol: None,
            base_symbol: None,
            quote_symbol: None,
            tick_size: value.tick_size,
            lot_size: value.lot_size,
            min_order: value.min_order,
            status: value.status,
            maker_fee_bps: value.maker_fee_bps,
            taker_fee_bps: value.taker_fee_bps,
            daily_volume: value.daily_volume,
            last_price: None,
            change_24h: None,
        }
    }
}

impl From<core_dex::DexOrder> for OrderJson {
    fn from(value: core_dex::DexOrder) -> Self {
        OrderJson {
            order_id: value.order_id,
            trader: value.trader,
            pair_id: value.pair_id,
            side: value.side,
            order_type: value.order_type,
            price: value.price,
            price_raw: value.price_raw,
            quantity: value.quantity,
            filled: value.filled,
            status: value.status,
            created_slot: value.created_slot,
            expiry_slot: value.expiry_slot,
        }
    }
}

impl From<core_dex::DexTrade> for TradeJson {
    fn from(value: core_dex::DexTrade) -> Self {
        TradeJson {
            trade_id: value.trade_id,
            pair_id: value.pair_id,
            price: value.price,
            price_raw: value.price_raw,
            quantity: value.quantity,
            taker: value.taker,
            maker_order_id: value.maker_order_id,
            slot: value.slot,
            side: value.side,
            timestamp: value.timestamp,
        }
    }
}

impl From<core_dex::DexPool> for PoolJson {
    fn from(value: core_dex::DexPool) -> Self {
        PoolJson {
            pool_id: value.pool_id,
            token_a: value.token_a,
            token_b: value.token_b,
            token_a_symbol: None,
            token_b_symbol: None,
            sqrt_price: value.sqrt_price,
            price: value.price,
            tick: value.tick,
            liquidity: value.liquidity,
            fee_tier: value.fee_tier,
            protocol_fee: value.protocol_fee,
        }
    }
}

impl From<core_dex::DexCandle> for CandleJson {
    fn from(value: core_dex::DexCandle) -> Self {
        CandleJson {
            open: value.open,
            high: value.high,
            low: value.low,
            close: value.close,
            volume: value.volume,
            slot: value.slot,
            timestamp: value.timestamp,
        }
    }
}

impl From<core_dex::DexStats24h> for Stats24hJson {
    fn from(value: core_dex::DexStats24h) -> Self {
        Stats24hJson {
            volume: value.volume,
            high: value.high,
            low: value.low,
            open: value.open,
            close: value.close,
            trade_count: value.trade_count,
            change: value.change,
            change_percent: value.change_percent,
        }
    }
}

impl From<core_dex::DexRoute> for RouteJson {
    fn from(value: core_dex::DexRoute) -> Self {
        RouteJson {
            route_id: value.route_id,
            token_in: value.token_in,
            token_out: value.token_out,
            route_type: value.route_type,
            pool_or_pair_id: value.pool_or_pair_id,
            secondary_id: value.secondary_id,
            split_percent: value.split_percent,
            enabled: value.enabled,
        }
    }
}

impl From<core_dex::DexProposal> for ProposalJson {
    fn from(value: core_dex::DexProposal) -> Self {
        ProposalJson {
            proposal_id: value.proposal_id,
            proposer: value.proposer,
            proposal_type: value.proposal_type,
            status: value.status,
            created_slot: value.created_slot,
            end_slot: value.end_slot,
            yes_votes: value.yes_votes,
            no_votes: value.no_votes,
            pair_id: value.pair_id,
            base_token: value.base_token,
            new_maker_fee: value.new_maker_fee,
            new_taker_fee: value.new_taker_fee,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Query Params
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PairsQuery {
    limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct DepthQuery {
    depth: Option<usize>,
}

#[derive(Deserialize)]
pub struct LimitQuery {
    limit: Option<usize>,
    trader: Option<String>,
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
    from: Option<u64>,
    to: Option<u64>,
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
    #[serde(default)]
    pub amount: u64,
    #[serde(default)]
    pub voter: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateProposalBody {
    #[serde(rename = "type")]
    pub proposal_type: String,
    #[serde(default)]
    pub base_token: Option<String>,
    #[serde(default)]
    pub quote_token: Option<String>,
    #[serde(default)]
    pub pair: Option<String>,
    #[serde(default)]
    pub maker_fee: Option<i64>,
    #[serde(default)]
    pub taker_fee: Option<u64>,
    #[serde(default)]
    pub proposer: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Storage Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Read raw bytes from contract storage via CF_CONTRACT_STORAGE (O(1) point-read).
/// Avoids deserializing the entire ContractAccount + WASM bytecode.
fn read_bytes(state: &crate::RpcState, program: &str, key: &str) -> Option<Vec<u8>> {
    state.state.get_program_storage(program, key.as_bytes())
}

/// Read a u64 from contract storage via CF_CONTRACT_STORAGE (O(1) point-read).
fn read_u64(state: &crate::RpcState, program: &str, key: &str) -> u64 {
    state.state.get_program_storage_u64(program, key.as_bytes())
}

/// Get current slot
fn current_slot(state: &crate::RpcState) -> u64 {
    state.state.get_last_slot().unwrap_or(0)
}

fn normalize_account_lookup(addr: &str) -> String {
    let trimmed = addr.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if trimmed.len() == 64 && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return trimmed.to_lowercase();
    }

    Pubkey::from_base58(trimmed)
        .map(|pubkey| hex::encode(pubkey.0))
        .unwrap_or_else(|_| trimmed.to_lowercase())
}

fn pair_storage_key(pair_id: u64) -> String {
    core_dex::pair_key(pair_id)
}

fn order_storage_key(order_id: u64) -> String {
    core_dex::order_key(order_id)
}

fn trade_storage_key(trade_id: u64) -> String {
    core_dex::trade_key(trade_id)
}

fn best_bid_storage_key(pair_id: u64) -> String {
    core_dex::best_bid_key(pair_id)
}

fn best_ask_storage_key(pair_id: u64) -> String {
    core_dex::best_ask_key(pair_id)
}

fn user_order_count_storage_key(account_hex: &str) -> String {
    core_dex::user_order_count_key(account_hex)
}

fn user_order_storage_key(account_hex: &str, idx: u64) -> String {
    core_dex::user_order_key(account_hex, idx)
}

fn analytics_last_price_storage_key(pair_id: u64) -> String {
    core_dex::analytics_last_price_key(pair_id)
}

fn analytics_last_trade_ts_storage_key(pair_id: u64) -> String {
    core_dex::analytics_last_trade_ts_key(pair_id)
}

fn analytics_24h_storage_key(pair_id: u64) -> String {
    core_dex::analytics_24h_key(pair_id)
}

fn analytics_candle_count_storage_key(pair_id: u64, interval: u64) -> String {
    core_dex::analytics_candle_count_key(pair_id, interval)
}

fn analytics_candle_storage_key(pair_id: u64, interval: u64, idx: u64) -> String {
    core_dex::analytics_candle_key(pair_id, interval, idx)
}

fn analytics_leaderboard_storage_key(rank: u64) -> String {
    core_dex::analytics_leaderboard_key(rank)
}

fn analytics_trader_stats_storage_key(account_hex: &str) -> String {
    core_dex::analytics_trader_stats_key(account_hex)
}

fn amm_pool_storage_key(pool_id: u64) -> String {
    core_dex::amm_pool_key(pool_id)
}

fn amm_owner_position_count_storage_key(account_hex: &str) -> String {
    core_dex::amm_owner_position_count_key(account_hex)
}

fn amm_owner_position_storage_key(account_hex: &str, idx: u64) -> String {
    core_dex::amm_owner_position_key(account_hex, idx)
}

fn amm_position_storage_key(position_id: u64) -> String {
    core_dex::amm_position_key(position_id)
}

fn route_storage_key(route_id: u64) -> String {
    core_dex::route_key(route_id)
}

fn margin_user_position_count_storage_key(account_hex: &str) -> String {
    core_dex::margin_user_position_count_key(account_hex)
}

fn margin_user_position_storage_key(account_hex: &str, idx: u64) -> String {
    core_dex::margin_user_position_key(account_hex, idx)
}

fn margin_position_storage_key(position_id: u64) -> String {
    core_dex::margin_position_key(position_id)
}

fn margin_mark_storage_key(pair_id: u64) -> String {
    core_dex::margin_mark_key(pair_id)
}

fn margin_enabled_storage_key(pair_id: u64) -> String {
    core_dex::margin_enabled_key(pair_id)
}

fn rewards_pending_storage_key(account_hex: &str) -> String {
    core_dex::rewards_pending_key(account_hex)
}

fn rewards_claimed_storage_key(account_hex: &str) -> String {
    core_dex::rewards_claimed_key(account_hex)
}

fn rewards_volume_storage_key(account_hex: &str) -> String {
    core_dex::rewards_volume_key(account_hex)
}

fn rewards_referral_count_storage_key(account_hex: &str) -> String {
    core_dex::rewards_referral_count_key(account_hex)
}

fn rewards_referral_earnings_storage_key(account_hex: &str) -> String {
    core_dex::rewards_referral_earnings_key(account_hex)
}

fn governance_proposal_storage_key(proposal_id: u64) -> String {
    core_dex::governance_proposal_key(proposal_id)
}

/// Symbol map cache: (last_refresh, cached_map). Refreshes every 30 seconds.
static SYMBOL_MAP_CACHE: Mutex<Option<(Instant, HashMap<String, String>)>> = Mutex::new(None);
const SYMBOL_CACHE_TTL_SECS: u64 = 30;

/// Ticker cache: avoids 4+ DB reads per pair on every /tickers request.
/// TTL 2 seconds — fast enough for live trading, avoids O(pairs × 4) reads.
static TICKERS_CACHE: Mutex<Option<(Instant, Vec<TickerJson>, u64)>> = Mutex::new(None);
const TICKERS_CACHE_TTL_SECS: u64 = 2;

#[derive(Clone, Default)]
struct PairOrderIndex {
    order_ids: Vec<u64>,
    scanned_order_count: u64,
}

/// Pair -> known order IDs cache. Reduces repeated O(total_orders) scans in hot paths.
static PAIR_ORDER_INDEX_CACHE: LazyLock<Mutex<HashMap<u64, PairOrderIndex>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn get_pair_order_ids(state: &crate::RpcState, pair_id: u64) -> Vec<u64> {
    let latest_order_count = read_u64(state, DEX_CORE_PROGRAM, DEX_ORDER_COUNT_KEY);

    let (mut known_ids, mut scanned_order_count) = {
        let mut cache = PAIR_ORDER_INDEX_CACHE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let entry = cache.entry(pair_id).or_default();
        (entry.order_ids.clone(), entry.scanned_order_count)
    };

    if scanned_order_count == 0 {
        for order_id in 1..=latest_order_count {
            let key = order_storage_key(order_id);
            if let Some(data) = read_bytes(state, DEX_CORE_PROGRAM, &key) {
                if let Some(order) = decode_order(&data) {
                    if order.pair_id == pair_id {
                        known_ids.push(order_id);
                    }
                }
            }
        }
        scanned_order_count = latest_order_count;
    } else if latest_order_count > scanned_order_count {
        for order_id in (scanned_order_count + 1)..=latest_order_count {
            let key = order_storage_key(order_id);
            if let Some(data) = read_bytes(state, DEX_CORE_PROGRAM, &key) {
                if let Some(order) = decode_order(&data) {
                    if order.pair_id == pair_id {
                        known_ids.push(order_id);
                    }
                }
            }
        }
        scanned_order_count = latest_order_count;
    }

    {
        let mut cache = PAIR_ORDER_INDEX_CACHE
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cache.insert(
            pair_id,
            PairOrderIndex {
                order_ids: known_ids.clone(),
                scanned_order_count,
            },
        );
    }

    known_ids
}

/// Build a hex-address→display-symbol map for known token contracts.
/// Uses the symbol registry to resolve contract names to pubkey addresses,
/// then maps to human-readable token symbols.
/// Results are cached for 30 seconds to avoid redundant storage queries.
fn build_token_symbol_map(state: &crate::RpcState) -> HashMap<String, String> {
    let mut cache = SYMBOL_MAP_CACHE.lock().unwrap_or_else(|e| e.into_inner());

    if let Some((ref ts, ref map)) = *cache {
        if ts.elapsed().as_secs() < SYMBOL_CACHE_TTL_SECS {
            return map.clone();
        }
    }

    // Cache miss or expired — rebuild
    let known_tokens: &[(&str, &str)] = &[
        ("LICN", "LICN"),
        ("LUSD", "lUSD"),
        ("WSOL", "wSOL"),
        ("WETH", "wETH"),
        ("WBNB", "wBNB"),
        ("MOSS", "MOSS"),
        ("PUNKS", "PUNKS"),
        ("BOUNTY", "BOUNTY"),
        ("COMPUTE", "COMPUTE"),
    ];
    let mut map = HashMap::new();
    for (registry_symbol, display_symbol) in known_tokens {
        if let Ok(Some(entry)) = state.state.get_symbol_registry(registry_symbol) {
            map.insert(hex::encode(entry.program.0), display_symbol.to_string());
        }
    }

    // Native LICN coin uses zero address sentinel [0u8; 32] — always map it
    let zero_hex = "00".repeat(32);
    map.entry(zero_hex).or_insert_with(|| "LICN".to_string());

    *cache = Some((Instant::now(), map.clone()));
    map
}

/// Decode a trading pair from 112-byte blob
fn decode_pair(data: &[u8]) -> Option<TradingPairJson> {
    core_dex::decode_pair(data).map(Into::into)
}

/// Decode an order from 128-byte blob
fn decode_order(data: &[u8]) -> Option<OrderJson> {
    core_dex::decode_order(data).map(Into::into)
}

/// Decode a trade from 80-byte blob.
/// `side` defaults to "buy" — caller should infer from maker order (opposite side).
/// `timestamp` defaults to 0 — caller should compute from slot if possible.
fn decode_trade(data: &[u8]) -> Option<TradeJson> {
    core_dex::decode_trade(data).map(Into::into)
}

/// Decode a pool from 96-byte blob
fn decode_pool(data: &[u8]) -> Option<PoolJson> {
    core_dex::decode_pool(data).map(Into::into)
}

/// Decode an LP position from 80-byte blob
fn decode_lp_position(data: &[u8], position_id: u64) -> Option<PositionJson> {
    core_dex::decode_lp_position(data).map(|value| PositionJson {
        position_id,
        owner: value.owner,
        pool_id: value.pool_id,
        lower_tick: value.lower_tick,
        upper_tick: value.upper_tick,
        liquidity: value.liquidity,
        fee_a_owed: value.fee_a_owed,
        fee_b_owed: value.fee_b_owed,
        created_slot: value.created_slot,
    })
}

/// Decode a margin position from 112-byte (V1) / 128-byte (V2+) blob
fn decode_margin_position(data: &[u8]) -> Option<MarginPositionJson> {
    core_dex::decode_margin_position(data).map(|value| MarginPositionJson {
        position_id: value.position_id,
        trader: value.trader,
        pair_id: value.pair_id,
        side: value.side,
        margin_type: value.margin_type,
        status: value.status,
        size: value.size,
        margin: value.margin,
        entry_price: value.entry_price,
        entry_price_raw: value.entry_price_raw,
        leverage: value.leverage,
        created_slot: value.created_slot,
        realized_pnl: value.realized_pnl,
        accumulated_funding: value.accumulated_funding,
        mark_price: 0.0,
        sl_price: value.sl_price,
        tp_price: value.tp_price,
    })
}

/// Decode a candle from 48-byte blob
fn decode_candle(data: &[u8]) -> Option<CandleJson> {
    core_dex::decode_candle(data).map(Into::into)
}

/// Decode 24h stats from 48-byte blob
fn decode_stats_24h(data: &[u8]) -> Option<Stats24hJson> {
    core_dex::decode_stats_24h(data).map(Into::into)
}

/// Decode a route from 96-byte blob
fn decode_route(data: &[u8]) -> Option<RouteJson> {
    core_dex::decode_route(data).map(Into::into)
}

/// Decode a proposal from 120-byte blob
fn decode_proposal(data: &[u8]) -> Option<ProposalJson> {
    core_dex::decode_proposal(data).map(Into::into)
}

// ─────────────────────────────────────────────────────────────────────────────
// Route Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// GET /api/v1/pairs — All trading pairs (enriched with symbols + last price)
async fn get_pairs(State(state): State<Arc<RpcState>>, Query(q): Query<PairsQuery>) -> Response {
    let count = read_u64(&state, DEX_CORE_PROGRAM, DEX_PAIR_COUNT_KEY);
    let limit = q.limit.unwrap_or(100).min(500) as u64;
    let effective_count = count.min(limit);
    let slot = current_slot(&state);
    let mut pairs = Vec::new();

    // Build reverse address→symbol map using known token contracts
    let symbol_map = build_token_symbol_map(&state);

    for i in 1..=effective_count {
        let key = pair_storage_key(i);
        if let Some(data) = read_bytes(&state, DEX_CORE_PROGRAM, &key) {
            if let Some(mut pair) = decode_pair(&data) {
                // Resolve human-readable symbols
                let base_sym = symbol_map.get(&pair.base_token).cloned();
                let quote_sym = symbol_map.get(&pair.quote_token).cloned();
                if let (Some(ref b), Some(ref q)) = (&base_sym, &quote_sym) {
                    pair.symbol = Some(format!("{}/{}", b, q));
                }
                pair.base_symbol = base_sym;
                pair.quote_symbol = quote_sym;

                // Read last price from analytics
                let lp_key = analytics_last_price_storage_key(pair.pair_id);
                let lp_raw = read_u64(&state, DEX_ANALYTICS_PROGRAM, &lp_key);
                if lp_raw > 0 {
                    pair.last_price = Some(lp_raw as f64 / PRICE_SCALE as f64);
                }

                // If no real trades, prefer consensus oracle price over genesis seed
                let trade_ts_key = analytics_last_trade_ts_storage_key(pair.pair_id);
                let has_real_trades = read_u64(&state, DEX_ANALYTICS_PROGRAM, &trade_ts_key) > 0;
                if !has_real_trades {
                    if let Some(oracle_price) = oracle_price_for_pair(&state.state, pair.pair_id) {
                        if oracle_price > 0.0 {
                            pair.last_price = Some(oracle_price);
                        }
                    }
                }

                // Read 24h stats for change
                let stats_key = analytics_24h_storage_key(pair.pair_id);
                if let Some(stats_data) = read_bytes(&state, DEX_ANALYTICS_PROGRAM, &stats_key) {
                    if stats_data.len() >= 48 {
                        // F18.6: Contract layout: [16..24]=low, [24..32]=open (was reading low as open)
                        let open =
                            u64::from_le_bytes(stats_data[24..32].try_into().unwrap_or([0; 8]));
                        if open > 0 && lp_raw > 0 {
                            pair.change_24h =
                                Some(((lp_raw as f64 - open as f64) / open as f64) * 100.0);
                        }
                    }
                }

                pairs.push(pair);
            }
        }
    }

    ApiResponse::ok(pairs, slot).into_response()
}

/// GET /api/v1/pairs/:id — Pair details (enriched with symbols + last price)
async fn get_pair(State(state): State<Arc<RpcState>>, Path(pair_id): Path<u64>) -> Response {
    let key = pair_storage_key(pair_id);
    let slot = current_slot(&state);

    match read_bytes(&state, DEX_CORE_PROGRAM, &key) {
        Some(data) => match decode_pair(&data) {
            Some(mut pair) => {
                let symbol_map = build_token_symbol_map(&state);
                let base_sym = symbol_map.get(&pair.base_token).cloned();
                let quote_sym = symbol_map.get(&pair.quote_token).cloned();
                if let (Some(ref b), Some(ref q)) = (&base_sym, &quote_sym) {
                    pair.symbol = Some(format!("{}/{}", b, q));
                }
                pair.base_symbol = base_sym;
                pair.quote_symbol = quote_sym;
                let lp_raw = read_u64(
                    &state,
                    DEX_ANALYTICS_PROGRAM,
                    &analytics_last_price_storage_key(pair.pair_id),
                );
                if lp_raw > 0 {
                    pair.last_price = Some(lp_raw as f64 / PRICE_SCALE as f64);
                }
                ApiResponse::ok(pair, slot).into_response()
            }
            None => api_err("invalid pair data"),
        },
        None => api_not_found(&format!("pair {} not found", pair_id)),
    }
}

/// GET /api/v1/pairs/:id/orderbook — L2 order book
/// Uses per-pair cache + persistent pair-order index to avoid repeated O(total_orders) scans.
async fn get_orderbook(
    State(state): State<Arc<RpcState>>,
    Path(pair_id): Path<u64>,
    Query(q): Query<DepthQuery>,
) -> Response {
    let depth = q.depth.unwrap_or(20).min(100);
    let slot = current_slot(&state);

    // Check orderbook cache — if fresh (< 1 second old), return immediately
    {
        let cache = state.orderbook_cache.read().await;
        if let Some((cached_at, cached_json)) = cache.get(&pair_id) {
            if cached_at.elapsed() < std::time::Duration::from_secs(1) {
                // Re-apply depth limit from cached full book
                let mut result = cached_json.clone();
                if let Some(obj) = result.as_object_mut() {
                    if let Some(bids) = obj.get_mut("bids").and_then(|b| b.as_array_mut()) {
                        bids.truncate(depth);
                    }
                    if let Some(asks) = obj.get_mut("asks").and_then(|a| a.as_array_mut()) {
                        asks.truncate(depth);
                    }
                    obj.insert("slot".to_string(), serde_json::json!(slot));
                }
                return ApiResponse::<serde_json::Value>::ok_raw(result, slot).into_response();
            }
        }
    }

    // Cache miss or stale: rebuild using pair-specific order-id index
    let mut bids: HashMap<u64, (u64, u32)> = HashMap::new(); // price → (total_qty, order_count)
    let mut asks: HashMap<u64, (u64, u32)> = HashMap::new();

    let pair_order_ids = get_pair_order_ids(&state, pair_id);

    for order_id in pair_order_ids {
        let key = order_storage_key(order_id);
        if let Some(data) = read_bytes(&state, DEX_CORE_PROGRAM, &key) {
            if let Some(order) = decode_order(&data) {
                if order.status != "open" && order.status != "partial" {
                    continue;
                }
                let remaining = order.quantity - order.filled;
                if remaining == 0 {
                    continue;
                }

                let entry = if order.side == "buy" {
                    bids.entry(order.price_raw).or_insert((0, 0))
                } else {
                    asks.entry(order.price_raw).or_insert((0, 0))
                };
                entry.0 += remaining;
                entry.1 += 1;
            }
        }
    }

    // Sort bids descending by price
    let mut bid_levels: Vec<OrderBookLevel> = bids
        .into_iter()
        .map(|(p, (q, c))| OrderBookLevel {
            price: p as f64 / PRICE_SCALE as f64,
            quantity: q,
            orders: c,
        })
        .collect();
    bid_levels.sort_by(|a, b| {
        b.price
            .partial_cmp(&a.price)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Sort asks ascending by price
    let mut ask_levels: Vec<OrderBookLevel> = asks
        .into_iter()
        .map(|(p, (q, c))| OrderBookLevel {
            price: p as f64 / PRICE_SCALE as f64,
            quantity: q,
            orders: c,
        })
        .collect();
    ask_levels.sort_by(|a, b| {
        a.price
            .partial_cmp(&b.price)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Cache the full book (before truncation)
    let full_book_json = serde_json::json!({
        "pair_id": pair_id,
        "bids": bid_levels,
        "asks": ask_levels,
        "slot": slot,
    });
    {
        let mut cache = state.orderbook_cache.write().await;
        cache.insert(pair_id, (std::time::Instant::now(), full_book_json));
    }

    // Truncate to requested depth
    bid_levels.truncate(depth);
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
    // F4.4: Support optional trader filter using either hex or base58 addresses.
    let trader_filter = normalize_account_lookup(q.trader.as_deref().unwrap_or(""));
    let slot = current_slot(&state);
    let trade_count = read_u64(&state, DEX_CORE_PROGRAM, DEX_TRADE_COUNT_KEY);

    let mut trades = Vec::new();
    // Read from most recent — trade IDs are 1-indexed, trade_count is highest ID
    let start = if trade_count > limit as u64 {
        trade_count - limit as u64 + 1
    } else {
        1
    };
    // Genesis timestamp: use chain start time for slot→timestamp conversion
    // Slot duration: 400ms
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    for i in (start..=trade_count).rev() {
        let key = trade_storage_key(i);
        if let Some(data) = read_bytes(&state, DEX_CORE_PROGRAM, &key) {
            if let Some(mut trade) = decode_trade(&data) {
                if trade.pair_id == pair_id {
                    // F4.4: Filter by trader address if specified
                    if !trader_filter.is_empty() && trade.taker != trader_filter {
                        continue;
                    }
                    // F3.2: Infer taker side from maker order
                    // The maker's side is the opposite of the taker's side.
                    let maker_key = order_storage_key(trade.maker_order_id);
                    if let Some(maker_data) = read_bytes(&state, DEX_CORE_PROGRAM, &maker_key) {
                        if maker_data.len() > 40 {
                            // Byte 40 = side (0=buy, 1=sell); taker is opposite
                            trade.side = if maker_data[40] == 0 { "sell" } else { "buy" };
                        }
                    }
                    // F3.3: Approximate timestamp from slot delta
                    // timestamp_ms ≈ now - (current_slot - trade_slot) * SLOT_DURATION_MS
                    let slot_age_ms = slot.saturating_sub(trade.slot) * SLOT_DURATION_MS;
                    trade.timestamp = now_ms.saturating_sub(slot_age_ms);
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

    let count_key = analytics_candle_count_storage_key(pair_id, interval);
    let candle_count = read_u64(&state, DEX_ANALYTICS_PROGRAM, &count_key);

    // F5.1+F5.2: Compute timestamps from slot and use 1-based inclusive range
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let now_sec = now_ms / 1000;

    let mut candles = Vec::new();
    // Candle IDs are 0-based; candle_count is the number of stored candles.
    // Scale scan range for small intervals so 1m charts (~60s) cover the same
    // wall-clock duration as 5m charts (~300s).  Base: limit × 300s = 25h.
    let effective_limit = if interval > 0 && interval < 300 {
        ((limit as u64) * 300 / interval).min(candle_count)
    } else {
        (limit as u64).min(candle_count)
    };
    let start = candle_count.saturating_sub(effective_limit);

    for i in start..candle_count {
        let key = analytics_candle_storage_key(pair_id, interval, i);
        if let Some(data) = read_bytes(&state, DEX_ANALYTICS_PROGRAM, &key) {
            if let Some(mut candle) = decode_candle(&data) {
                // The slot field stores the unix timestamp directly (written by
                // oracle/bridge writers).  Ten-digit values are unix seconds;
                // values below that are legacy slot numbers where we estimate.
                if candle.slot >= 1_000_000_000 {
                    candle.timestamp = candle.slot;
                } else {
                    // Legacy: approximate timestamp from slot delta (0.4s/slot)
                    let slot_age_sec = slot.saturating_sub(candle.slot) * SLOT_DURATION_MS / 1000;
                    candle.timestamp = now_sec.saturating_sub(slot_age_sec);
                }
                // F5.2: Filter by from/to (seconds) if provided
                if let Some(from) = q.from {
                    if candle.timestamp < from {
                        continue;
                    }
                }
                if let Some(to) = q.to {
                    if candle.timestamp > to {
                        continue;
                    }
                }
                candles.push(candle);
            }
        }
    }

    ApiResponse::ok(candles, slot).into_response()
}

/// GET /api/v1/pairs/:id/stats — 24h rolling stats
async fn get_pair_stats(State(state): State<Arc<RpcState>>, Path(pair_id): Path<u64>) -> Response {
    let slot = current_slot(&state);
    let key = analytics_24h_storage_key(pair_id);

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

    let last_price_key = analytics_last_price_storage_key(pair_id);
    let last_price_raw = read_u64(&state, DEX_ANALYTICS_PROGRAM, &last_price_key);
    let mut last_price = last_price_raw as f64 / PRICE_SCALE as f64;

    // If no real trades have occurred, the analytics price is the genesis seed.
    // Prefer the live consensus oracle price so REST and WS sources agree.
    let trade_ts_key = analytics_last_trade_ts_storage_key(pair_id);
    let has_real_trades = read_u64(&state, DEX_ANALYTICS_PROGRAM, &trade_ts_key) > 0;
    if !has_real_trades {
        if let Some(oracle_price) = oracle_price_for_pair(&state.state, pair_id) {
            if oracle_price > 0.0 {
                last_price = oracle_price;
            }
        }
    }

    let best_bid_raw = read_u64(&state, DEX_CORE_PROGRAM, &best_bid_storage_key(pair_id));
    let best_ask_raw = read_u64(&state, DEX_CORE_PROGRAM, &best_ask_storage_key(pair_id));

    let stats_key = analytics_24h_storage_key(pair_id);
    let (volume_24h, change_24h, high_24h, low_24h, trades_24h) =
        match read_bytes(&state, DEX_ANALYTICS_PROGRAM, &stats_key) {
            Some(data) if data.len() >= 48 => {
                let vol = u64::from_le_bytes(data[0..8].try_into().unwrap_or([0; 8]));
                let high_raw = u64::from_le_bytes(data[8..16].try_into().unwrap_or([0; 8]));
                // F18.6: Contract layout: [16..24]=low, [24..32]=open (was swapped)
                let low_raw = u64::from_le_bytes(data[16..24].try_into().unwrap_or([0; 8]));
                let open_raw = u64::from_le_bytes(data[24..32].try_into().unwrap_or([0; 8]));
                let _close_raw = u64::from_le_bytes(data[32..40].try_into().unwrap_or([0; 8]));
                let tcount = u64::from_le_bytes(data[40..48].try_into().unwrap_or([0; 8]));
                let open_f = open_raw as f64 / PRICE_SCALE as f64;
                let change = if open_f > 0.0 {
                    ((last_price - open_f) / open_f) * 100.0
                } else {
                    0.0
                };
                (
                    vol,
                    change,
                    high_raw as f64 / PRICE_SCALE as f64,
                    low_raw as f64 / PRICE_SCALE as f64,
                    tcount,
                )
            }
            _ => (0, 0.0, 0.0, 0.0, 0),
        };

    // Clamp sentinel values: u64::MAX means "no bid/ask on book"
    let bid = if best_bid_raw == u64::MAX {
        0.0
    } else {
        best_bid_raw as f64 / PRICE_SCALE as f64
    };
    let ask = if best_ask_raw == u64::MAX || best_ask_raw == 0 {
        0.0
    } else {
        best_ask_raw as f64 / PRICE_SCALE as f64
    };

    ApiResponse::ok(
        TickerJson {
            pair_id,
            last_price,
            bid,
            ask,
            volume_24h,
            change_24h,
            high_24h,
            low_24h,
            trades_24h,
        },
        slot,
    )
    .into_response()
}

/// GET /api/v1/tickers — All tickers
async fn get_all_tickers(State(state): State<Arc<RpcState>>) -> Response {
    // PERF-OPT: Check ticker cache (2s TTL)
    {
        let cache = TICKERS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((ref ts, ref cached, cached_slot)) = *cache {
            if ts.elapsed().as_secs() < TICKERS_CACHE_TTL_SECS {
                return ApiResponse::ok(cached.clone(), cached_slot).into_response();
            }
        }
    }

    let count = read_u64(&state, DEX_CORE_PROGRAM, DEX_PAIR_COUNT_KEY);
    // AUDIT-FIX RPC-1: Cap pair iteration to prevent DoS on unbounded loops
    let effective_count = count.min(500);
    let slot = current_slot(&state);
    let mut tickers = Vec::new();

    for pair_id in 1..=effective_count {
        let last_price_raw = read_u64(
            &state,
            DEX_ANALYTICS_PROGRAM,
            &analytics_last_price_storage_key(pair_id),
        );
        let best_bid_raw = read_u64(&state, DEX_CORE_PROGRAM, &best_bid_storage_key(pair_id));
        let best_ask_raw = read_u64(&state, DEX_CORE_PROGRAM, &best_ask_storage_key(pair_id));

        let mut last_price = last_price_raw as f64 / PRICE_SCALE as f64;

        // If no real trades, prefer consensus oracle price over genesis seed
        let trade_ts_key = analytics_last_trade_ts_storage_key(pair_id);
        let has_real_trades = read_u64(&state, DEX_ANALYTICS_PROGRAM, &trade_ts_key) > 0;
        if !has_real_trades {
            if let Some(oracle_price) = oracle_price_for_pair(&state.state, pair_id) {
                if oracle_price > 0.0 {
                    last_price = oracle_price;
                }
            }
        }

        // Read 24h stats
        let stats_key = analytics_24h_storage_key(pair_id);
        let (volume_24h, change_24h, high_24h, low_24h, trades_24h) =
            match read_bytes(&state, DEX_ANALYTICS_PROGRAM, &stats_key) {
                Some(data) if data.len() >= 48 => {
                    let vol = u64::from_le_bytes(data[0..8].try_into().unwrap_or([0; 8]));
                    let high_raw = u64::from_le_bytes(data[8..16].try_into().unwrap_or([0; 8]));
                    // F18.6: Contract layout: [16..24]=low, [24..32]=open (was swapped)
                    let low_raw = u64::from_le_bytes(data[16..24].try_into().unwrap_or([0; 8]));
                    let open_raw = u64::from_le_bytes(data[24..32].try_into().unwrap_or([0; 8]));
                    let tcount = u64::from_le_bytes(data[40..48].try_into().unwrap_or([0; 8]));
                    let open_f = open_raw as f64 / PRICE_SCALE as f64;
                    let change = if open_f > 0.0 {
                        ((last_price - open_f) / open_f) * 100.0
                    } else {
                        0.0
                    };
                    (
                        vol,
                        change,
                        high_raw as f64 / PRICE_SCALE as f64,
                        low_raw as f64 / PRICE_SCALE as f64,
                        tcount,
                    )
                }
                _ => (0, 0.0, 0.0, 0.0, 0),
            };

        tickers.push(TickerJson {
            pair_id,
            last_price,
            bid: if best_bid_raw == u64::MAX {
                0.0
            } else {
                best_bid_raw as f64 / PRICE_SCALE as f64
            },
            ask: if best_ask_raw == u64::MAX || best_ask_raw == 0 {
                0.0
            } else {
                best_ask_raw as f64 / PRICE_SCALE as f64
            },
            volume_24h,
            change_24h,
            high_24h,
            low_24h,
            trades_24h,
        });
    }

    // Update cache
    {
        let mut cache = TICKERS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        *cache = Some((Instant::now(), tickers.clone(), slot));
    }

    ApiResponse::ok(tickers, slot).into_response()
}

/// POST /api/v1/orders — Place order (must use sendTransaction)
async fn post_order() -> Response {
    api_method_not_allowed("Orders must be submitted via sendTransaction RPC method")
}

/// DELETE /api/v1/orders/:id — Cancel order (must use sendTransaction)
async fn delete_order(Path(_order_id): Path<u64>) -> Response {
    api_method_not_allowed("Order cancellations must be submitted via sendTransaction RPC method")
}

/// GET /api/v1/orders?trader=<addr> — List orders
async fn get_orders(State(state): State<Arc<RpcState>>, Query(q): Query<TraderQuery>) -> Response {
    let slot = current_slot(&state);

    let trader_hex = match &q.trader {
        Some(t) => normalize_account_lookup(t),
        None => return api_err("trader parameter required"),
    };

    // Read user's order count and order IDs
    let count_key = user_order_count_storage_key(&trader_hex);
    let count = read_u64(&state, DEX_CORE_PROGRAM, &count_key);

    let mut orders = Vec::new();
    for i in 1..=count.min(200) {
        let idx_key = user_order_storage_key(&trader_hex, i);
        let order_id = read_u64(&state, DEX_CORE_PROGRAM, &idx_key);
        let key = order_storage_key(order_id);
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
    let key = order_storage_key(order_id);

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
    let count = read_u64(&state, DEX_AMM_PROGRAM, AMM_POOL_COUNT_KEY);
    let slot = current_slot(&state);
    let mut pools = Vec::new();
    let symbol_map = build_token_symbol_map(&state);

    for i in 1..=count.min(500) {
        let key = amm_pool_storage_key(i);
        if let Some(data) = read_bytes(&state, DEX_AMM_PROGRAM, &key) {
            if let Some(mut pool) = decode_pool(&data) {
                pool.token_a_symbol = symbol_map.get(&pool.token_a).cloned();
                pool.token_b_symbol = symbol_map.get(&pool.token_b).cloned();
                pools.push(pool);
            }
        }
    }

    ApiResponse::ok(pools, slot).into_response()
}

/// GET /api/v1/pools/:id — Pool details
async fn get_pool(State(state): State<Arc<RpcState>>, Path(pool_id): Path<u64>) -> Response {
    let slot = current_slot(&state);
    let key = amm_pool_storage_key(pool_id);

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
        Some(o) => normalize_account_lookup(o),
        None => return api_err("owner parameter required"),
    };

    let count_key = amm_owner_position_count_storage_key(&owner_hex);
    let count = read_u64(&state, DEX_AMM_PROGRAM, &count_key);

    let mut positions = Vec::new();
    for i in 1..=count.min(100) {
        let idx_key = amm_owner_position_storage_key(&owner_hex, i);
        let pos_id = read_u64(&state, DEX_AMM_PROGRAM, &idx_key);
        let key = amm_position_storage_key(pos_id);
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
        let denominator =
            liquidity as u128 + (amount_after_fee as u128 * sqrt_price as u128 / (1u128 << 32));
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
        let amount_out = (liquidity as u128 * delta_sqrt)
            .checked_div(denom)
            .unwrap_or(0) as u64;
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
    let key = amm_pool_storage_key(pool_id);
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

/// Compute a swap quote through CLOB order book by matching against resting orders.
/// For a "buy" swap (token_in is quote, token_out is base): walk asks ascending.
/// For a "sell" swap (token_in is base, token_out is quote): walk bids descending.
/// Returns (amount_out, price_impact_pct) or None if the pair has no liquidity.
fn quote_clob_swap(
    state: &crate::RpcState,
    pair_id: u64,
    token_in: &str,
    amount_in: u64,
) -> Option<(u64, f64)> {
    // Load the pair to determine base/quote tokens
    let pair_key = pair_storage_key(pair_id);
    let pair_data = read_bytes(state, DEX_CORE_PROGRAM, &pair_key)?;
    let pair = decode_pair(&pair_data)?;

    let token_in_lower = token_in.to_lowercase();
    let is_buying_base = token_in_lower != pair.base_token.to_lowercase();

    // Collect open orders on the opposing side, sorted by best price
    // (price_raw, remaining_qty) — sorted by best price
    let mut opposing_orders: Vec<(u64, u64)> = Vec::new();

    let pair_order_ids = get_pair_order_ids(state, pair_id);
    for order_id in pair_order_ids {
        let key = order_storage_key(order_id);
        if let Some(data) = read_bytes(state, DEX_CORE_PROGRAM, &key) {
            if let Some(order) = decode_order(&data) {
                if order.status != "open" && order.status != "partial" {
                    continue;
                }
                let remaining = order.quantity.saturating_sub(order.filled);
                if remaining == 0 {
                    continue;
                }
                // For buying base, we want sells (asks); for selling base, we want buys (bids)
                let wanted_side = if is_buying_base { "sell" } else { "buy" };
                if order.side != wanted_side {
                    continue;
                }
                opposing_orders.push((order.price_raw, remaining));
            }
        }
    }

    if opposing_orders.is_empty() {
        return None;
    }

    // Sort: for buying base → asks ascending (cheapest first)
    //       for selling base → bids descending (highest first)
    if is_buying_base {
        opposing_orders.sort_by_key(|&(price, _)| price);
    } else {
        opposing_orders.sort_by_key(|&(price, _)| std::cmp::Reverse(price));
    }

    let best_price = opposing_orders[0].0;

    // Walk the order book matching amount_in against resting orders
    let mut remaining_in = amount_in;
    let mut total_out: u64 = 0;
    let mut last_fill_price: u64 = 0;

    for (price_raw, qty_available) in &opposing_orders {
        if remaining_in == 0 {
            break;
        }

        if is_buying_base {
            // Buying base with quote: at this price, each base unit costs price_raw (scaled)
            let can_buy = if *price_raw > 0 {
                (remaining_in as u128 * PRICE_SCALE as u128 / *price_raw as u128) as u64
            } else {
                continue;
            };
            // AUDIT-FIX F-6: Skip if integer truncation produces zero output
            if can_buy == 0 {
                continue;
            }
            let fill_qty = can_buy.min(*qty_available);
            let fill_cost = (fill_qty as u128 * *price_raw as u128 / PRICE_SCALE as u128) as u64;

            total_out += fill_qty;
            remaining_in = remaining_in.saturating_sub(fill_cost.max(1));
        } else {
            // Selling base for quote: each base unit earns price_raw (scaled)
            let fill_qty = remaining_in.min(*qty_available);
            let fill_proceeds =
                (fill_qty as u128 * *price_raw as u128 / PRICE_SCALE as u128) as u64;

            // AUDIT-FIX F-11: Skip if integer truncation produces zero proceeds
            if fill_proceeds == 0 {
                continue;
            }
            total_out += fill_proceeds;
            remaining_in = remaining_in.saturating_sub(fill_qty);
        }

        last_fill_price = *price_raw;
    }

    // AUDIT-FIX F-6/F-11: If total_out is zero after all fills, the trade amount
    // is too small for any order book level. Return None.
    if total_out == 0 {
        return None;
    }

    // Price impact = |1 - last_fill_price / best_price| * 100
    let price_impact = if best_price > 0 && last_fill_price > 0 {
        let ratio = last_fill_price as f64 / best_price as f64;
        ((1.0 - ratio).abs() * 100.0 * 100.0).round() / 100.0
    } else {
        0.0
    };

    Some((total_out, price_impact))
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
    let route_count = read_u64(&state, DEX_ROUTER_PROGRAM, ROUTER_ROUTE_COUNT_KEY);
    let mut best_route: Option<RouteJson> = None;
    let mut best_output: u64 = 0;
    let mut best_impact: f64 = 0.0;

    for i in 1..=route_count {
        let key = route_storage_key(i);
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
                } else if route.route_type == "split" {
                    // F9.4a: Quote both CLOB and AMM legs proportionally
                    let clob_pct = route.split_percent as u64;
                    let _amm_pct = 100u64.saturating_sub(clob_pct);
                    let clob_amount = body.amount_in * clob_pct / 100;
                    let amm_amount = body.amount_in.saturating_sub(clob_amount);
                    let mut total_out = 0u64;
                    let mut total_impact = 0.0f64;
                    let mut legs = 0u32;
                    if clob_amount > 0 {
                        if let Some((out, imp)) =
                            quote_clob_swap(&state, route.pool_or_pair_id, &token_in, clob_amount)
                        {
                            total_out += out;
                            total_impact += imp;
                            legs += 1;
                        }
                    }
                    if amm_amount > 0 {
                        if let Some((out, imp)) =
                            quote_amm_swap(&state, route.secondary_id, &token_in, amm_amount)
                        {
                            total_out += out;
                            total_impact += imp;
                            legs += 1;
                        }
                    }
                    if total_out > best_output {
                        best_output = total_out;
                        best_impact = if legs > 0 {
                            total_impact / legs as f64
                        } else {
                            0.0
                        };
                        best_route = Some(route);
                    }
                } else {
                    // CLOB route: quote against resting limit orders on the order book
                    if let Some((amount_out, impact)) =
                        quote_clob_swap(&state, route.pool_or_pair_id, &token_in, body.amount_in)
                    {
                        if amount_out > best_output {
                            best_output = amount_out;
                            best_impact = impact;
                            best_route = Some(route);
                        }
                    } else if best_route.is_none() {
                        // No CLOB liquidity, but record the route for fallback error messaging
                        best_route = Some(route);
                    }
                }
            }
        }
    }

    // Fallback: if no explicit route found, scan all AMM pools for a matching pair
    if best_route.is_none() {
        let pool_count = read_u64(&state, DEX_AMM_PROGRAM, AMM_POOL_COUNT_KEY);
        // Hard cap fallback scan to keep quote latency bounded when pool_count is very large.
        // Route registry remains the primary path; this is best-effort compatibility fallback.
        let scan_limit = pool_count.min(10_000);
        for pid in 0..scan_limit {
            let pk = amm_pool_storage_key(pid);
            if let Some(data) = read_bytes(&state, DEX_AMM_PROGRAM, &pk) {
                if data.len() >= 96 {
                    let ta = hex::encode(&data[0..32]);
                    let tb = hex::encode(&data[32..64]);
                    let body_out = body.token_out.to_lowercase();
                    if (ta == token_in && tb == body_out) || (tb == token_in && ta == body_out) {
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
            // F9.4b: Compute minAmountOut for the response (slippage check is informational only for quotes)
            let min_out = (best_output as f64 * (1.0 - body.slippage / 100.0)) as u64;

            // F9.12b: Determine fee rate based on route type
            let fee_bps: u64 = if route.route_type == "amm" {
                // Read pool fee tier
                let pk = amm_pool_storage_key(route.pool_or_pair_id);
                if let Some(data) = read_bytes(&state, DEX_AMM_PROGRAM, &pk) {
                    if data.len() >= 93 {
                        let idx = data[92] as usize;
                        if idx < AMM_FEE_BPS.len() {
                            AMM_FEE_BPS[idx]
                        } else {
                            30
                        }
                    } else {
                        30
                    }
                } else {
                    30
                }
            } else if route.route_type == "split" {
                // Blended fee: weighted average of CLOB taker (5bps) and AMM fee
                let clob_pct = route.split_percent as u64;
                let amm_pct = 100u64.saturating_sub(clob_pct);
                let amm_fee = {
                    let pk = amm_pool_storage_key(route.secondary_id);
                    if let Some(data) = read_bytes(&state, DEX_AMM_PROGRAM, &pk) {
                        if data.len() >= 93 {
                            let idx = data[92] as usize;
                            if idx < AMM_FEE_BPS.len() {
                                AMM_FEE_BPS[idx]
                            } else {
                                30
                            }
                        } else {
                            30
                        }
                    } else {
                        30
                    }
                };
                (5 * clob_pct + amm_fee * amm_pct) / 100
            } else {
                5 // CLOB taker fee default
            };
            let estimated_fee = best_output * fee_bps / 10000;

            let result = serde_json::json!({
                "amountIn": body.amount_in,
                "amountOut": best_output,
                "minAmountOut": min_out,
                "routeType": route.route_type,
                "routeId": route.route_id,
                "poolId": route.pool_or_pair_id,
                "priceImpact": best_impact,
                "feeRate": fee_bps,
                "estimatedFee": estimated_fee,
                "splitPercent": route.split_percent,
                "slot": slot,
            });
            // FIX F14: Removed WS event emissions from read-only quote endpoint.
            // Real trade events are emitted by the dex_router WASM contract during sendTransaction execution.

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
    let count = read_u64(&state, DEX_ROUTER_PROGRAM, ROUTER_ROUTE_COUNT_KEY);
    let slot = current_slot(&state);
    let mut routes = Vec::new();

    for i in 1..=count.min(500) {
        let key = route_storage_key(i);
        if let Some(data) = read_bytes(&state, DEX_ROUTER_PROGRAM, &key) {
            if let Some(route) = decode_route(&data) {
                routes.push(route);
            }
        }
    }

    ApiResponse::ok(routes, slot).into_response()
}

// ─── MARGIN ─────────────────────────────────────────────────────────────────

/// POST /api/v1/margin/open — Open margin position (must use sendTransaction)
async fn post_margin_open() -> Response {
    api_method_not_allowed("Margin positions must be opened via sendTransaction RPC method")
}

/// POST /api/v1/margin/close — Close margin position (must use sendTransaction)
async fn post_margin_close() -> Response {
    api_method_not_allowed("Margin positions must be closed via sendTransaction RPC method")
}

/// GET /api/v1/margin/positions?trader=<addr> — Margin positions
async fn get_margin_positions(
    State(state): State<Arc<RpcState>>,
    Query(q): Query<TraderQuery>,
) -> Response {
    let slot = current_slot(&state);

    let trader_hex = match &q.trader {
        Some(t) => normalize_account_lookup(t),
        None => return api_err("trader parameter required"),
    };

    let count_key = margin_user_position_count_storage_key(&trader_hex);
    let count = read_u64(&state, DEX_MARGIN_PROGRAM, &count_key);

    let mut positions = Vec::new();
    for i in 1..=count.min(100) {
        let idx_key = margin_user_position_storage_key(&trader_hex, i);
        let pos_id = read_u64(&state, DEX_MARGIN_PROGRAM, &idx_key);
        let key = margin_position_storage_key(pos_id);
        if let Some(data) = read_bytes(&state, DEX_MARGIN_PROGRAM, &key) {
            if let Some(mut pos) = decode_margin_position(&data) {
                // F24.3 FIX: Populate mark_price from pair's current mark price
                let mark_key = margin_mark_storage_key(pos.pair_id);
                let mark_raw = read_u64(&state, DEX_MARGIN_PROGRAM, &mark_key);
                if mark_raw > 0 {
                    pos.mark_price = mark_raw as f64 / PRICE_SCALE as f64;
                } else {
                    // Fallback: use analytics last price for the pair
                    let lp_key = analytics_last_price_storage_key(pos.pair_id);
                    let close_raw = read_u64(&state, DEX_ANALYTICS_PROGRAM, &lp_key);
                    if close_raw > 0 {
                        pos.mark_price = close_raw as f64 / PRICE_SCALE as f64;
                    }
                }
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
    let key = margin_position_storage_key(position_id);

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
        position_count: read_u64(&state, DEX_MARGIN_PROGRAM, MARGIN_POSITION_COUNT_KEY),
        max_leverage: {
            let v = read_u64(&state, DEX_MARGIN_PROGRAM, "mrg_max_lev");
            if v > 0 {
                v
            } else {
                100
            } // default 100x
        },
    };

    ApiResponse::ok(info, slot).into_response()
}

/// GET /api/v1/margin/enabled-pairs — List pair IDs that have margin trading enabled
async fn get_margin_enabled_pairs(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    let pair_count = read_u64(&state, DEX_CORE_PROGRAM, DEX_PAIR_COUNT_KEY);
    let mut enabled: Vec<u64> = Vec::new();
    for i in 1..=pair_count.min(500) {
        let key = margin_enabled_storage_key(i);
        if read_u64(&state, DEX_MARGIN_PROGRAM, &key) == 1 {
            enabled.push(i);
        }
    }
    ApiResponse::ok(serde_json::json!({ "enabledPairIds": enabled }), slot).into_response()
}

/// GET /api/v1/margin/funding-rate — Returns funding rate constants per tier
async fn get_margin_funding_rate(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);

    // Base rate: 1 bps = 0.01% per 8h interval (from contract constant MAX_FUNDING_RATE_BPS=100 / 100)
    let base_rate_bps: u64 = 1;
    let interval_hours: u64 = 8; // FUNDING_INTERVAL_SLOTS = 28_800 ≈ 8h
    let max_rate_bps: u64 = 100; // 1% max per interval

    // Tier table mirrors contract's get_tier_params funding_rate_mult_x10
    let tiers = vec![
        FundingTierJson {
            max_leverage: 2,
            multiplier_x10: 10,
            effective_rate_bps: base_rate_bps as f64 * 10.0 / 10.0,
        },
        FundingTierJson {
            max_leverage: 3,
            multiplier_x10: 10,
            effective_rate_bps: base_rate_bps as f64 * 10.0 / 10.0,
        },
        FundingTierJson {
            max_leverage: 5,
            multiplier_x10: 15,
            effective_rate_bps: base_rate_bps as f64 * 15.0 / 10.0,
        },
        FundingTierJson {
            max_leverage: 10,
            multiplier_x10: 20,
            effective_rate_bps: base_rate_bps as f64 * 20.0 / 10.0,
        },
        FundingTierJson {
            max_leverage: 25,
            multiplier_x10: 30,
            effective_rate_bps: base_rate_bps as f64 * 30.0 / 10.0,
        },
        FundingTierJson {
            max_leverage: 50,
            multiplier_x10: 50,
            effective_rate_bps: base_rate_bps as f64 * 50.0 / 10.0,
        },
        FundingTierJson {
            max_leverage: 100,
            multiplier_x10: 100,
            effective_rate_bps: base_rate_bps as f64 * 100.0 / 10.0,
        },
    ];

    let info = FundingRateJson {
        base_rate_bps,
        interval_hours,
        max_rate_bps,
        tiers,
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
        let key = analytics_leaderboard_storage_key(rank as u64);
        if let Some(addr_data) = read_bytes(&state, DEX_ANALYTICS_PROGRAM, &key) {
            if addr_data.len() >= 32 {
                let addr_hex = hex::encode(&addr_data[..32]);
                let stats_key = analytics_trader_stats_storage_key(&addr_hex);
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
    let addr_hex = normalize_account_lookup(&addr);

    let info = RewardInfoJson {
        pending: read_u64(
            &state,
            DEX_REWARDS_PROGRAM,
            &rewards_pending_storage_key(&addr_hex),
        ),
        claimed: read_u64(
            &state,
            DEX_REWARDS_PROGRAM,
            &rewards_claimed_storage_key(&addr_hex),
        ),
        total_volume: read_u64(
            &state,
            DEX_REWARDS_PROGRAM,
            &rewards_volume_storage_key(&addr_hex),
        ),
        referral_count: read_u64(
            &state,
            DEX_REWARDS_PROGRAM,
            &rewards_referral_count_storage_key(&addr_hex),
        ),
        referral_earnings: read_u64(
            &state,
            DEX_REWARDS_PROGRAM,
            &rewards_referral_earnings_storage_key(&addr_hex),
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
    let count = read_u64(
        &state,
        DEX_GOVERNANCE_PROGRAM,
        GOVERNANCE_PROPOSAL_COUNT_KEY,
    );
    let slot = current_slot(&state);
    let mut proposals = Vec::new();

    for i in 1..=count.min(500) {
        let key = governance_proposal_storage_key(i);
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
    let key = governance_proposal_storage_key(proposal_id);

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
    let addr_hex = normalize_account_lookup(&addr);
    let key = analytics_trader_stats_storage_key(&addr_hex);

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
            address: addr,
            total_volume: volume,
            trade_count,
            total_pnl: pnl,
        },
        slot,
    )
    .into_response()
}

/// AUDIT-FIX RPC-2: GET /api/v1/traders/:addr/trades — User trade history across all pairs
async fn get_trader_trades(
    State(state): State<Arc<RpcState>>,
    Path(addr): Path<String>,
    Query(q): Query<LimitQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(50).min(200);
    let slot = current_slot(&state);
    let addr_hex = normalize_account_lookup(&addr);
    let trade_count = read_u64(&state, DEX_CORE_PROGRAM, DEX_TRADE_COUNT_KEY);

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut trades = Vec::new();
    // Iterate recent trades and filter by taker address
    let start = if trade_count > 1000 {
        trade_count - 999
    } else {
        1
    };
    for i in (start..=trade_count).rev() {
        let key = trade_storage_key(i);
        if let Some(data) = read_bytes(&state, DEX_CORE_PROGRAM, &key) {
            if let Some(mut trade) = decode_trade(&data) {
                if trade.taker == addr_hex {
                    let maker_key = order_storage_key(trade.maker_order_id);
                    if let Some(maker_data) = read_bytes(&state, DEX_CORE_PROGRAM, &maker_key) {
                        if maker_data.len() > 40 {
                            trade.side = if maker_data[40] == 0 { "sell" } else { "buy" };
                        }
                    }
                    let slot_age_ms = slot.saturating_sub(trade.slot) * SLOT_DURATION_MS;
                    trade.timestamp = now_ms.saturating_sub(slot_age_ms);
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

// ═══════════════════════════════════════════════════════════════════════════════
// GOVERNANCE: POST handlers for proposals and votes
// ═══════════════════════════════════════════════════════════════════════════════

/// POST /api/v1/governance/proposals — Create a new proposal (must use sendTransaction)
/// FIX F16: Changed from misleading 200 response to 405 — proposals must be created on-chain.
async fn post_create_proposal() -> Response {
    api_method_not_allowed("Governance proposals must be created via sendTransaction RPC method")
}

/// POST /api/v1/governance/proposals/:id/vote — Vote on a proposal (must use sendTransaction)
async fn post_vote(Path(_proposal_id): Path<u64>) -> Response {
    api_method_not_allowed("Votes must be submitted via sendTransaction RPC method")
}

// ═══════════════════════════════════════════════════════════════════════════════
// ORACLE: Price feed endpoints
// ═══════════════════════════════════════════════════════════════════════════════

/// GET /api/v1/oracle/prices — All oracle price feeds
async fn get_oracle_prices(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    let assets = ["LICN", "wSOL", "wETH", "wBNB"];
    let mut feeds = Vec::new();

    for asset in &assets {
        if let Ok(Some(price)) = state.state.get_oracle_consensus_price(asset) {
            let stale = slot.saturating_sub(price.slot) > lichen_core::ORACLE_STALENESS_SLOTS;
            let timestamp = state
                .state
                .get_block_by_slot(price.slot)
                .ok()
                .flatten()
                .map(|block| block.header.timestamp)
                .unwrap_or(0);

            feeds.push(serde_json::json!({
                "asset": asset,
                "price": price.price as f64 / 10f64.powi(price.decimals as i32),
                "priceRaw": price.price,
                "decimals": price.decimals,
                "slot": price.slot,
                "timestamp": timestamp,
                "stale": stale,
                "source": "native_consensus"
            }));
        }
    }

    ApiResponse::ok(
        serde_json::json!({
            "oracleActive": true,
            "feeds": feeds,
        }),
        slot,
    )
    .into_response()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Oracle price helper — maps pair IDs to consensus oracle prices
// ═══════════════════════════════════════════════════════════════════════════════

/// Get the live consensus oracle price for a DEX pair.
/// Returns the USD-denominated price (or LICN-denominated for cross pairs)
/// from the validator-attested oracle, or None if unavailable/stale.
fn oracle_price_for_pair(state: &lichen_core::StateStore, pair_id: u64) -> Option<f64> {
    use lichen_core::consensus::{consensus_oracle_price_from_state, licn_price_from_state};
    let licn_usd = licn_price_from_state(state);
    match pair_id {
        1 => Some(licn_usd),
        2 => consensus_oracle_price_from_state(state, "wSOL"),
        3 => consensus_oracle_price_from_state(state, "wETH"),
        4 => consensus_oracle_price_from_state(state, "wSOL").map(|wsol| {
            if licn_usd > 0.0 {
                wsol / licn_usd
            } else {
                0.0
            }
        }),
        5 => consensus_oracle_price_from_state(state, "wETH").map(|weth| {
            if licn_usd > 0.0 {
                weth / licn_usd
            } else {
                0.0
            }
        }),
        6 => consensus_oracle_price_from_state(state, "wBNB"),
        7 => consensus_oracle_price_from_state(state, "wBNB").map(|wbnb| {
            if licn_usd > 0.0 {
                wbnb / licn_usd
            } else {
                0.0
            }
        }),
        _ => None,
    }
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
        .route("/margin/enabled-pairs", get(get_margin_enabled_pairs))
        .route("/margin/funding-rate", get(get_margin_funding_rate))
        // Analytics
        .route("/leaderboard", get(get_leaderboard))
        .route("/traders/:addr/stats", get(get_trader_stats))
        .route("/traders/:addr/trades", get(get_trader_trades))
        // Rewards
        .route("/rewards/:addr", get(get_rewards))
        // Governance
        .route(
            "/governance/proposals",
            get(get_proposals).post(post_create_proposal),
        )
        .route("/governance/proposals/:id", get(get_proposal))
        .route("/governance/proposals/:id/vote", post(post_vote))
        // Platform Stats
        .route("/stats/core", get(get_core_stats))
        .route("/stats/amm", get(get_amm_stats))
        .route("/stats/margin", get(get_margin_stats_rest))
        .route("/stats/router", get(get_router_stats))
        .route("/stats/rewards", get(get_rewards_stats))
        .route("/stats/analytics", get(get_analytics_stats))
        .route("/stats/governance", get(get_governance_stats))
        .route("/stats/lichenswap", get(get_lichenswap_stats))
        // Oracle
        .route("/oracle/prices", get(get_oracle_prices))
}

// ═══════════════════════════════════════════════════════════════════════════════
// PLATFORM STATS REST HANDLERS
// ═══════════════════════════════════════════════════════════════════════════════

async fn get_core_stats(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    ApiResponse::ok(
        serde_json::json!({
            "pairCount": read_u64(&state, DEX_CORE_PROGRAM, DEX_PAIR_COUNT_KEY),
            "orderCount": read_u64(&state, DEX_CORE_PROGRAM, DEX_ORDER_COUNT_KEY),
            "tradeCount": read_u64(&state, DEX_CORE_PROGRAM, DEX_TRADE_COUNT_KEY),
            "totalVolume": read_u64(&state, DEX_CORE_PROGRAM, "dex_total_volume"),
            "feeTreasury": read_u64(&state, DEX_CORE_PROGRAM, "dex_fee_treasury"),
        }),
        slot,
    )
    .into_response()
}

async fn get_amm_stats(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    ApiResponse::ok(
        serde_json::json!({
            "poolCount": read_u64(&state, DEX_AMM_PROGRAM, AMM_POOL_COUNT_KEY),
            "positionCount": read_u64(&state, DEX_AMM_PROGRAM, "amm_pos_count"),
            "swapCount": read_u64(&state, DEX_AMM_PROGRAM, "amm_swap_count"),
            "totalVolume": read_u64(&state, DEX_AMM_PROGRAM, "amm_total_volume"),
            "totalFees": read_u64(&state, DEX_AMM_PROGRAM, "amm_total_fees"),
        }),
        slot,
    )
    .into_response()
}

async fn get_margin_stats_rest(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    ApiResponse::ok(
        serde_json::json!({
            "positionCount": read_u64(&state, DEX_MARGIN_PROGRAM, MARGIN_POSITION_COUNT_KEY),
            "totalVolume": read_u64(&state, DEX_MARGIN_PROGRAM, "mrg_total_volume"),
            "liquidationCount": read_u64(&state, DEX_MARGIN_PROGRAM, "mrg_liq_count"),
            "insuranceFund": read_u64(&state, DEX_MARGIN_PROGRAM, "mrg_insurance"),
        }),
        slot,
    )
    .into_response()
}

async fn get_router_stats(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    ApiResponse::ok(
        serde_json::json!({
            "routeCount": read_u64(&state, DEX_ROUTER_PROGRAM, ROUTER_ROUTE_COUNT_KEY),
            "swapCount": read_u64(&state, DEX_ROUTER_PROGRAM, "rtr_swap_count"),
            "totalVolume": read_u64(&state, DEX_ROUTER_PROGRAM, "rtr_total_volume"),
        }),
        slot,
    )
    .into_response()
}

async fn get_rewards_stats(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    ApiResponse::ok(
        serde_json::json!({
            "tradeCount": read_u64(&state, DEX_REWARDS_PROGRAM, "rew_trade_count"),
            "traderCount": read_u64(&state, DEX_REWARDS_PROGRAM, "rew_trader_count"),
            "totalVolume": read_u64(&state, DEX_REWARDS_PROGRAM, "rew_total_volume"),
            "totalDistributed": read_u64(&state, DEX_REWARDS_PROGRAM, "rew_total_dist"),
        }),
        slot,
    )
    .into_response()
}

async fn get_analytics_stats(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    ApiResponse::ok(
        serde_json::json!({
            "recordCount": read_u64(&state, DEX_ANALYTICS_PROGRAM, "ana_rec_count"),
            "traderCount": read_u64(&state, DEX_ANALYTICS_PROGRAM, "ana_trader_count"),
            "totalVolume": read_u64(&state, DEX_ANALYTICS_PROGRAM, "ana_total_volume"),
        }),
        slot,
    )
    .into_response()
}

async fn get_governance_stats(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    // F14.4: Count active proposals
    let count = read_u64(
        &state,
        DEX_GOVERNANCE_PROGRAM,
        GOVERNANCE_PROPOSAL_COUNT_KEY,
    );
    let mut active = 0u64;
    for i in 1..=count.min(500) {
        let key = governance_proposal_storage_key(i);
        if let Some(data) = read_bytes(&state, DEX_GOVERNANCE_PROGRAM, &key) {
            if data.len() > 41 && data[41] == 0 {
                // status byte at offset 41, 0 = active
                active += 1;
            }
        }
    }
    // F14.8: Use camelCase keys
    ApiResponse::ok(
        serde_json::json!({
            "proposalCount": count,
            "activeProposals": active,
            "totalVotes": read_u64(&state, DEX_GOVERNANCE_PROGRAM, "gov_total_votes"),
            "voterCount": read_u64(&state, DEX_GOVERNANCE_PROGRAM, "gov_voter_count"),
            "minQuorum": 3,
            "min_quorum": 3,
        }),
        slot,
    )
    .into_response()
}

async fn get_lichenswap_stats(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    ApiResponse::ok(
        serde_json::json!({
            "swapCount": read_u64(&state, "LICHENSWAP", "ms_swap_count"),
            "volumeA": read_u64(&state, "LICHENSWAP", "ms_volume_a"),
            "volumeB": read_u64(&state, "LICHENSWAP", "ms_volume_b"),
        }),
        slot,
    )
    .into_response()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Unit Tests — decode_* helpers, compute_swap_output_rpc, constants
// ═══════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────

    /// Build a minimal 112-byte trading-pair blob for decode_pair.
    #[allow(clippy::too_many_arguments)]
    fn make_pair_blob(
        pair_id: u64,
        tick: u64,
        lot: u64,
        min_order: u64,
        status: u8,
        maker_bps: i16,
        taker_bps: u16,
        vol: u64,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; 112];
        buf[0..32].copy_from_slice(&[0xAA; 32]); // base_token
        buf[32..64].copy_from_slice(&[0xBB; 32]); // quote_token
        buf[64..72].copy_from_slice(&pair_id.to_le_bytes());
        buf[72..80].copy_from_slice(&tick.to_le_bytes());
        buf[80..88].copy_from_slice(&lot.to_le_bytes());
        buf[88..96].copy_from_slice(&min_order.to_le_bytes());
        buf[96] = status;
        buf[97..99].copy_from_slice(&maker_bps.to_le_bytes());
        buf[99..101].copy_from_slice(&taker_bps.to_le_bytes());
        buf[101..109].copy_from_slice(&vol.to_le_bytes());
        buf
    }

    /// Build a minimal 128-byte order blob for decode_order.
    #[allow(clippy::too_many_arguments)]
    fn make_order_blob(
        trader: [u8; 32],
        pair_id: u64,
        side: u8,
        otype: u8,
        price: u64,
        qty: u64,
        filled: u64,
        status: u8,
        created: u64,
        expiry: u64,
        order_id: u64,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; 128];
        buf[0..32].copy_from_slice(&trader);
        buf[32..40].copy_from_slice(&pair_id.to_le_bytes());
        buf[40] = side;
        buf[41] = otype;
        buf[42..50].copy_from_slice(&price.to_le_bytes());
        buf[50..58].copy_from_slice(&qty.to_le_bytes());
        buf[58..66].copy_from_slice(&filled.to_le_bytes());
        buf[66] = status;
        buf[67..75].copy_from_slice(&created.to_le_bytes());
        buf[75..83].copy_from_slice(&expiry.to_le_bytes());
        buf[83..91].copy_from_slice(&order_id.to_le_bytes());
        buf
    }

    /// Build a minimal 80-byte trade blob.
    fn make_trade_blob(
        trade_id: u64,
        pair_id: u64,
        price: u64,
        qty: u64,
        taker: [u8; 32],
        maker_order_id: u64,
        slot: u64,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; 80];
        buf[0..8].copy_from_slice(&trade_id.to_le_bytes());
        buf[8..16].copy_from_slice(&pair_id.to_le_bytes());
        buf[16..24].copy_from_slice(&price.to_le_bytes());
        buf[24..32].copy_from_slice(&qty.to_le_bytes());
        buf[32..64].copy_from_slice(&taker);
        buf[64..72].copy_from_slice(&maker_order_id.to_le_bytes());
        buf[72..80].copy_from_slice(&slot.to_le_bytes());
        buf
    }

    /// Build a minimal 96-byte pool blob.
    fn make_pool_blob(
        pool_id: u64,
        sqrt_price: u64,
        tick: i32,
        liquidity: u64,
        fee_tier: u8,
        protocol_fee: u8,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; 96];
        buf[0..32].copy_from_slice(&[0xCC; 32]); // token_a
        buf[32..64].copy_from_slice(&[0xDD; 32]); // token_b
        buf[64..72].copy_from_slice(&pool_id.to_le_bytes());
        buf[72..80].copy_from_slice(&sqrt_price.to_le_bytes());
        buf[80..84].copy_from_slice(&tick.to_le_bytes());
        buf[84..92].copy_from_slice(&liquidity.to_le_bytes());
        buf[92] = fee_tier;
        buf[93] = protocol_fee;
        buf
    }

    /// Build a minimal 48-byte candle blob.
    fn make_candle_blob(o: u64, h: u64, l: u64, c: u64, vol: u64, slot: u64) -> Vec<u8> {
        let mut buf = vec![0u8; 48];
        buf[0..8].copy_from_slice(&o.to_le_bytes());
        buf[8..16].copy_from_slice(&h.to_le_bytes());
        buf[16..24].copy_from_slice(&l.to_le_bytes());
        buf[24..32].copy_from_slice(&c.to_le_bytes());
        buf[32..40].copy_from_slice(&vol.to_le_bytes());
        buf[40..48].copy_from_slice(&slot.to_le_bytes());
        buf
    }

    /// Build a minimal 48-byte 24h stats blob.
    fn make_stats_blob(vol: u64, h: u64, l: u64, o: u64, c: u64, trades: u64) -> Vec<u8> {
        let mut buf = vec![0u8; 48];
        buf[0..8].copy_from_slice(&vol.to_le_bytes());
        buf[8..16].copy_from_slice(&h.to_le_bytes());
        buf[16..24].copy_from_slice(&l.to_le_bytes());
        buf[24..32].copy_from_slice(&o.to_le_bytes());
        buf[32..40].copy_from_slice(&c.to_le_bytes());
        buf[40..48].copy_from_slice(&trades.to_le_bytes());
        buf
    }

    /// Build a minimal 96-byte route blob.
    fn make_route_blob(
        route_id: u64,
        rtype: u8,
        pool_id: u64,
        secondary: u64,
        split: u8,
        enabled: bool,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; 96];
        buf[0..32].copy_from_slice(&[0x11; 32]); // token_in
        buf[32..64].copy_from_slice(&[0x22; 32]); // token_out
        buf[64..72].copy_from_slice(&route_id.to_le_bytes());
        buf[72] = rtype;
        buf[73..81].copy_from_slice(&pool_id.to_le_bytes());
        buf[81..89].copy_from_slice(&secondary.to_le_bytes());
        buf[89] = split;
        buf[90] = if enabled { 1 } else { 0 };
        buf
    }

    // ── decode_pair ─────────────────────────────────────────────────────

    #[test]
    fn decode_pair_too_short() {
        assert!(decode_pair(&[0u8; 111]).is_none());
    }

    #[test]
    fn normalize_account_lookup_keeps_hex() {
        let input = "AABBCCDDEEFF00112233445566778899AABBCCDDEEFF00112233445566778899";
        assert_eq!(
            normalize_account_lookup(input),
            "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899"
        );
    }

    #[test]
    fn normalize_account_lookup_decodes_base58() {
        let pubkey = Pubkey::new([0x11; 32]);
        assert_eq!(
            normalize_account_lookup(&pubkey.to_base58()),
            hex::encode([0x11; 32])
        );
    }

    #[test]
    fn decode_pair_roundtrip() {
        let blob = make_pair_blob(42, 100, 10, 5, 0, -5, 30, 999_000);
        let p = decode_pair(&blob).unwrap();
        assert_eq!(p.pair_id, 42);
        assert_eq!(p.tick_size, 100);
        assert_eq!(p.lot_size, 10);
        assert_eq!(p.min_order, 5);
        assert_eq!(p.status, "active");
        assert_eq!(p.maker_fee_bps, -5);
        assert_eq!(p.taker_fee_bps, 30);
        assert_eq!(p.daily_volume, 999_000);
        assert_eq!(p.base_token, hex::encode([0xAA; 32]));
    }

    #[test]
    fn decode_pair_status_paused() {
        let blob = make_pair_blob(1, 0, 0, 0, 1, 0, 0, 0);
        assert_eq!(decode_pair(&blob).unwrap().status, "paused");
    }

    #[test]
    fn decode_pair_status_delisted() {
        let blob = make_pair_blob(1, 0, 0, 0, 99, 0, 0, 0);
        assert_eq!(decode_pair(&blob).unwrap().status, "delisted");
    }

    // ── decode_order ────────────────────────────────────────────────────

    #[test]
    fn decode_order_too_short() {
        assert!(decode_order(&[0u8; 127]).is_none());
    }

    #[test]
    fn decode_order_roundtrip() {
        let trader = [0x01; 32];
        let blob = make_order_blob(trader, 7, 0, 0, PRICE_SCALE * 100, 50, 10, 1, 500, 1000, 99);
        let o = decode_order(&blob).unwrap();
        assert_eq!(o.order_id, 99);
        assert_eq!(o.pair_id, 7);
        assert_eq!(o.side, "buy");
        assert_eq!(o.order_type, "limit");
        assert_eq!(o.price, 100.0);
        assert_eq!(o.quantity, 50);
        assert_eq!(o.filled, 10);
        assert_eq!(o.status, "partial");
        assert_eq!(o.created_slot, 500);
        assert_eq!(o.expiry_slot, 1000);
    }

    #[test]
    fn decode_order_sell_market() {
        let blob = make_order_blob([0; 32], 1, 1, 1, 0, 0, 0, 0, 0, 0, 1);
        let o = decode_order(&blob).unwrap();
        assert_eq!(o.side, "sell");
        assert_eq!(o.order_type, "market");
    }

    #[test]
    fn decode_order_all_statuses() {
        for (byte, expected) in [
            (0u8, "open"),
            (1, "partial"),
            (2, "filled"),
            (3, "cancelled"),
            (4, "expired"),
        ] {
            let blob = make_order_blob([0; 32], 1, 0, 0, 0, 0, 0, byte, 0, 0, 1);
            assert_eq!(decode_order(&blob).unwrap().status, expected);
        }
    }

    #[test]
    fn decode_order_all_types() {
        for (byte, expected) in [
            (0u8, "limit"),
            (1, "market"),
            (2, "stop-limit"),
            (3, "post-only"),
        ] {
            let blob = make_order_blob([0; 32], 1, 0, byte, 0, 0, 0, 0, 0, 0, 1);
            assert_eq!(decode_order(&blob).unwrap().order_type, expected);
        }
    }

    // ── decode_trade ────────────────────────────────────────────────────

    #[test]
    fn decode_trade_too_short() {
        assert!(decode_trade(&[0u8; 79]).is_none());
    }

    #[test]
    fn decode_trade_roundtrip() {
        let taker = [0xFF; 32];
        let price_raw = 50 * PRICE_SCALE;
        let blob = make_trade_blob(1, 2, price_raw, 100, taker, 77, 12345);
        let t = decode_trade(&blob).unwrap();
        assert_eq!(t.trade_id, 1);
        assert_eq!(t.pair_id, 2);
        assert_eq!(t.price, 50.0);
        assert_eq!(t.quantity, 100);
        assert_eq!(t.maker_order_id, 77);
        assert_eq!(t.slot, 12345);
        assert_eq!(t.taker, hex::encode([0xFF; 32]));
    }

    // ── decode_pool ─────────────────────────────────────────────────────

    #[test]
    fn decode_pool_too_short() {
        assert!(decode_pool(&[0u8; 95]).is_none());
    }

    #[test]
    fn decode_pool_roundtrip() {
        let sqrt_price: u64 = 1u64 << 32; // sqrt_price = 1.0 → price = 1.0
        let blob = make_pool_blob(5, sqrt_price, -100, 500_000, 2, 10);
        let p = decode_pool(&blob).unwrap();
        assert_eq!(p.pool_id, 5);
        assert!((p.price - 1.0).abs() < 1e-6);
        assert_eq!(p.tick, -100);
        assert_eq!(p.liquidity, 500_000);
        assert_eq!(p.fee_tier, "30bps");
        assert_eq!(p.protocol_fee, 10);
    }

    #[test]
    fn decode_pool_fee_tiers() {
        for (byte, expected) in [(0u8, "1bps"), (1, "5bps"), (2, "30bps"), (3, "100bps")] {
            let blob = make_pool_blob(1, 0, 0, 0, byte, 0);
            assert_eq!(decode_pool(&blob).unwrap().fee_tier, expected);
        }
    }

    // ── decode_lp_position ──────────────────────────────────────────────

    #[test]
    fn decode_lp_position_too_short() {
        assert!(decode_lp_position(&[0u8; 79], 1).is_none());
    }

    #[test]
    fn decode_lp_position_roundtrip() {
        let mut buf = vec![0u8; 80];
        buf[0..32].copy_from_slice(&[0xAB; 32]);
        buf[32..40].copy_from_slice(&3u64.to_le_bytes());
        buf[40..44].copy_from_slice(&(-200i32).to_le_bytes());
        buf[44..48].copy_from_slice(&200i32.to_le_bytes());
        buf[48..56].copy_from_slice(&1_000_000u64.to_le_bytes());
        buf[56..64].copy_from_slice(&50u64.to_le_bytes());
        buf[64..72].copy_from_slice(&75u64.to_le_bytes());
        buf[72..80].copy_from_slice(&999u64.to_le_bytes());

        let pos = decode_lp_position(&buf, 42).unwrap();
        assert_eq!(pos.position_id, 42);
        assert_eq!(pos.pool_id, 3);
        assert_eq!(pos.lower_tick, -200);
        assert_eq!(pos.upper_tick, 200);
        assert_eq!(pos.liquidity, 1_000_000);
        assert_eq!(pos.fee_a_owed, 50);
        assert_eq!(pos.fee_b_owed, 75);
        assert_eq!(pos.created_slot, 999);
    }

    // ── decode_margin_position ──────────────────────────────────────────

    #[test]
    fn decode_margin_position_too_short() {
        assert!(decode_margin_position(&[0u8; 111]).is_none());
    }

    #[test]
    fn decode_margin_position_v1() {
        let mut buf = vec![0u8; 112];
        buf[0..32].copy_from_slice(&[0x33; 32]);
        buf[32..40].copy_from_slice(&7u64.to_le_bytes()); // position_id
        buf[40..48].copy_from_slice(&2u64.to_le_bytes()); // pair_id
        buf[48] = 0; // side = long
        buf[49] = 0; // status = open
        buf[50..58].copy_from_slice(&100u64.to_le_bytes()); // size
        buf[58..66].copy_from_slice(&10u64.to_le_bytes()); // margin
        let entry = 50 * PRICE_SCALE;
        buf[66..74].copy_from_slice(&entry.to_le_bytes());
        buf[74..82].copy_from_slice(&5u64.to_le_bytes()); // leverage
        buf[82..90].copy_from_slice(&1000u64.to_le_bytes()); // created_slot
                                                             // PNL at zero => bias
        buf[90..98].copy_from_slice(&PNL_BIAS.to_le_bytes());
        buf[98..106].copy_from_slice(&0u64.to_le_bytes());

        let m = decode_margin_position(&buf).unwrap();
        assert_eq!(m.position_id, 7);
        assert_eq!(m.pair_id, 2);
        assert_eq!(m.side, "long");
        assert_eq!(m.status, "open");
        assert_eq!(m.entry_price, 50.0);
        assert_eq!(m.leverage, 5);
        assert_eq!(m.realized_pnl, 0);
        assert_eq!(m.margin_type, "isolated"); // V1 = isolated
        assert_eq!(m.sl_price, 0);
        assert_eq!(m.tp_price, 0);
    }

    #[test]
    fn decode_margin_position_v2_cross() {
        let mut buf = vec![0u8; 128];
        buf[0..32].copy_from_slice(&[0x44; 32]);
        buf[32..40].copy_from_slice(&1u64.to_le_bytes());
        buf[40..48].copy_from_slice(&1u64.to_le_bytes());
        buf[48] = 1; // short
        buf[49] = 1; // closed
        buf[50..58].copy_from_slice(&200u64.to_le_bytes());
        buf[58..66].copy_from_slice(&20u64.to_le_bytes());
        let entry = 75 * PRICE_SCALE;
        buf[66..74].copy_from_slice(&entry.to_le_bytes());
        buf[74..82].copy_from_slice(&10u64.to_le_bytes());
        buf[82..90].copy_from_slice(&500u64.to_le_bytes());
        // PNL: +1000 => bias + 1000
        let pnl_raw = PNL_BIAS + 1000;
        buf[90..98].copy_from_slice(&pnl_raw.to_le_bytes());
        buf[98..106].copy_from_slice(&0u64.to_le_bytes());
        // SL/TP
        let sl = 80 * PRICE_SCALE;
        let tp = 60 * PRICE_SCALE;
        buf[106..114].copy_from_slice(&sl.to_le_bytes());
        buf[114..122].copy_from_slice(&tp.to_le_bytes());
        buf[122] = 1; // cross
        buf[123..128].fill(0);

        let m = decode_margin_position(&buf).unwrap();
        assert_eq!(m.side, "short");
        assert_eq!(m.status, "closed");
        assert_eq!(m.margin_type, "cross");
        assert_eq!(m.realized_pnl, 1000);
        assert_eq!(m.sl_price, sl);
        assert_eq!(m.tp_price, tp);
    }

    #[test]
    fn decode_margin_position_liquidated() {
        let mut buf = vec![0u8; 112];
        buf[0..32].fill(0);
        buf[32..40].copy_from_slice(&1u64.to_le_bytes());
        buf[40..48].copy_from_slice(&1u64.to_le_bytes());
        buf[48] = 0;
        buf[49] = 2; // liquidated
        buf[50..106].fill(0);
        buf[90..98].copy_from_slice(&PNL_BIAS.to_le_bytes());
        let m = decode_margin_position(&buf).unwrap();
        assert_eq!(m.status, "liquidated");
    }

    // ── decode_candle ───────────────────────────────────────────────────

    #[test]
    fn decode_candle_too_short() {
        assert!(decode_candle(&[0u8; 47]).is_none());
    }

    #[test]
    fn decode_candle_roundtrip() {
        let scale = PRICE_SCALE;
        let blob = make_candle_blob(100 * scale, 110 * scale, 90 * scale, 105 * scale, 5000, 42);
        let c = decode_candle(&blob).unwrap();
        assert!((c.open - 100.0).abs() < 1e-6);
        assert!((c.high - 110.0).abs() < 1e-6);
        assert!((c.low - 90.0).abs() < 1e-6);
        assert!((c.close - 105.0).abs() < 1e-6);
        assert_eq!(c.volume, 5000);
        assert_eq!(c.slot, 42);
    }

    // ── decode_stats_24h ────────────────────────────────────────────────

    #[test]
    fn decode_stats_24h_too_short() {
        assert!(decode_stats_24h(&[0u8; 47]).is_none());
    }

    #[test]
    fn decode_stats_24h_positive_change() {
        let scale = PRICE_SCALE;
        let blob = make_stats_blob(
            10_000,
            120 * scale,
            80 * scale,
            100 * scale,
            110 * scale,
            200,
        );
        let s = decode_stats_24h(&blob).unwrap();
        assert_eq!(s.volume, 10_000);
        assert!((s.open - 100.0).abs() < 1e-6);
        assert!((s.close - 110.0).abs() < 1e-6);
        assert!((s.change - 10.0).abs() < 1e-6);
        assert!((s.change_percent - 10.0).abs() < 1e-6);
        assert_eq!(s.trade_count, 200);
    }

    #[test]
    fn decode_stats_24h_zero_open() {
        let blob = make_stats_blob(0, 0, 0, 0, 0, 0);
        let s = decode_stats_24h(&blob).unwrap();
        assert_eq!(s.change_percent, 0.0); // no div by zero
    }

    // ── decode_route ────────────────────────────────────────────────────

    #[test]
    fn decode_route_too_short() {
        assert!(decode_route(&[0u8; 95]).is_none());
    }

    #[test]
    fn decode_route_roundtrip() {
        let blob = make_route_blob(10, 1, 5, 0, 50, true);
        let r = decode_route(&blob).unwrap();
        assert_eq!(r.route_id, 10);
        assert_eq!(r.route_type, "amm");
        assert_eq!(r.pool_or_pair_id, 5);
        assert_eq!(r.split_percent, 50);
        assert!(r.enabled);
    }

    #[test]
    fn decode_route_all_types() {
        for (byte, expected) in [(0u8, "clob"), (1, "amm"), (2, "split"), (3, "multi_hop")] {
            let blob = make_route_blob(1, byte, 0, 0, 0, false);
            assert_eq!(decode_route(&blob).unwrap().route_type, expected);
        }

        let invalid = make_route_blob(1, 4, 0, 0, 0, false);
        assert!(decode_route(&invalid).is_none());
    }

    // ── decode_proposal ─────────────────────────────────────────────────

    #[test]
    fn decode_proposal_too_short() {
        assert!(decode_proposal(&[0u8; 119]).is_none());
    }

    #[test]
    fn decode_proposal_active_new_pair() {
        let mut buf = vec![0u8; 120];
        buf[0..32].copy_from_slice(&[0x55; 32]); // proposer
        buf[32..40].copy_from_slice(&1u64.to_le_bytes());
        buf[40] = 0; // new_pair
        buf[41] = 0; // active
        buf[42..50].copy_from_slice(&100u64.to_le_bytes());
        buf[50..58].copy_from_slice(&200u64.to_le_bytes());
        buf[58..66].copy_from_slice(&50u64.to_le_bytes());
        buf[66..74].copy_from_slice(&10u64.to_le_bytes());
        buf[74..82].copy_from_slice(&3u64.to_le_bytes());

        let p = decode_proposal(&buf).unwrap();
        assert_eq!(p.proposal_id, 1);
        assert_eq!(p.proposal_type, "new_pair");
        assert_eq!(p.status, "active");
        assert_eq!(p.yes_votes, 50);
        assert_eq!(p.no_votes, 10);
        assert!(p.base_token.is_some()); // 120 >= 114, so new_pair includes base_token
    }

    #[test]
    fn decode_proposal_all_statuses() {
        for (byte, expected) in [
            (0u8, "active"),
            (1, "passed"),
            (2, "rejected"),
            (3, "executed"),
            (4, "cancelled"),
        ] {
            let mut buf = vec![0u8; 120];
            buf[41] = byte;
            assert_eq!(decode_proposal(&buf).unwrap().status, expected);
        }
    }

    #[test]
    fn decode_proposal_all_types() {
        for (byte, expected) in [
            (0u8, "new_pair"),
            (1, "fee_change"),
            (2, "delist"),
            (3, "param_change"),
        ] {
            let mut buf = vec![0u8; 120];
            buf[40] = byte;
            assert_eq!(decode_proposal(&buf).unwrap().proposal_type, expected);
        }
    }

    // ── compute_swap_output_rpc ─────────────────────────────────────────

    #[test]
    fn swap_zero_liquidity_returns_zero() {
        let (out, new_sqrt) = compute_swap_output_rpc(1000, 0, 1u64 << 32, 30, true);
        assert_eq!(out, 0);
        assert_eq!(new_sqrt, 1u64 << 32);
    }

    #[test]
    fn swap_zero_amount_returns_zero() {
        let (out, new_sqrt) = compute_swap_output_rpc(0, 1_000_000, 1u64 << 32, 30, true);
        assert_eq!(out, 0);
        assert_eq!(new_sqrt, 1u64 << 32);
    }

    #[test]
    fn swap_a_for_b_produces_output() {
        let sqrt_price = 1u64 << 32;
        let (out, new_sqrt) = compute_swap_output_rpc(10_000, 1_000_000, sqrt_price, 30, true);
        assert!(out > 0, "should produce output tokens");
        assert!(new_sqrt < sqrt_price, "A→B should lower sqrt price");
    }

    #[test]
    fn swap_b_for_a_produces_output() {
        let sqrt_price = 1u64 << 32;
        let (out, new_sqrt) = compute_swap_output_rpc(10_000, 1_000_000, sqrt_price, 30, false);
        assert!(out > 0, "should produce output tokens");
        assert!(new_sqrt > sqrt_price, "B→A should raise sqrt price");
    }

    #[test]
    fn swap_more_input_more_output() {
        let sqrt_price = 1u64 << 32;
        let liq = 1_000_000u64;
        let (out_small, _) = compute_swap_output_rpc(1_000, liq, sqrt_price, 30, true);
        let (out_large, _) = compute_swap_output_rpc(10_000, liq, sqrt_price, 30, true);
        assert!(
            out_large > out_small,
            "larger input should yield more output"
        );
    }

    #[test]
    fn swap_higher_fee_less_output() {
        let sqrt_price = 1u64 << 32;
        let liq = 1_000_000u64;
        let (out_low_fee, _) = compute_swap_output_rpc(10_000, liq, sqrt_price, 10, true);
        let (out_high_fee, _) = compute_swap_output_rpc(10_000, liq, sqrt_price, 500, true);
        assert!(
            out_low_fee > out_high_fee,
            "lower fee should yield more output"
        );
    }

    // ── constants sanity ────────────────────────────────────────────────

    #[test]
    fn price_scale_is_1e9() {
        assert_eq!(PRICE_SCALE, 1_000_000_000);
    }

    #[test]
    fn pnl_bias_is_2_63() {
        assert_eq!(PNL_BIAS, 1u64 << 63);
    }

    #[test]
    fn slot_duration_ms_is_400() {
        assert_eq!(SLOT_DURATION_MS, 400);
    }

    #[test]
    fn program_constants_non_empty() {
        assert!(!DEX_CORE_PROGRAM.is_empty());
        assert!(!DEX_AMM_PROGRAM.is_empty());
        assert!(!DEX_MARGIN_PROGRAM.is_empty());
        assert!(!DEX_ANALYTICS_PROGRAM.is_empty());
        assert!(!DEX_ROUTER_PROGRAM.is_empty());
        assert!(!DEX_REWARDS_PROGRAM.is_empty());
        assert!(!DEX_GOVERNANCE_PROGRAM.is_empty());
    }

    // ── api_err / api_not_found helpers ─────────────────────────────────

    #[test]
    fn default_limit_is_limit() {
        assert_eq!(default_limit(), "limit");
    }
}
