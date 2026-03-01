// ═══════════════════════════════════════════════════════════════════════════════
// MoltChain RPC — ClawPump Launchpad REST API Module
// Implements /api/v1/launchpad/* endpoints for the bonding-curve token launcher
//
// Reads contract storage directly from StateStore using the ClawPump
// key layout (cp_*, cpt:*, bal:*, etc.).
// ═══════════════════════════════════════════════════════════════════════════════

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::RpcState;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const CLAWPUMP_PROGRAM: &str = "CLAWPUMP";
const SHELLS_PER_MOLT: f64 = 1_000_000_000.0;
const BASE_PRICE: u64 = 1_000;
const SLOPE: u64 = 1;
const SLOPE_SCALE: u64 = 1_000_000;
const CREATION_FEE_MOLT: f64 = 10.0;
const PLATFORM_FEE_PCT: u64 = 1;

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

fn read_bytes(state: &RpcState, key: &[u8]) -> Option<Vec<u8>> {
    state.state.get_program_storage(CLAWPUMP_PROGRAM, key)
}

fn read_u64_key(state: &RpcState, key: &[u8]) -> u64 {
    state.state.get_program_storage_u64(CLAWPUMP_PROGRAM, key)
}

fn current_slot(state: &RpcState) -> u64 {
    state.state.get_last_slot().unwrap_or(0)
}

fn u64_le(data: &[u8], offset: usize) -> u64 {
    if data.len() < offset + 8 {
        return 0;
    }
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap_or([0; 8]))
}

/// Compute bonding curve spot price at given supply
fn spot_price(supply: u64) -> f64 {
    let price_shells = BASE_PRICE as f64 + (supply as f64 * SLOPE as f64 / SLOPE_SCALE as f64);
    price_shells / SHELLS_PER_MOLT
}

/// Compute market cap: spot_price(supply) * supply / 1e9
fn market_cap(supply: u64) -> f64 {
    let price_shells = BASE_PRICE as u128 + (supply as u128 * SLOPE as u128 / SLOPE_SCALE as u128);
    (price_shells * supply as u128) as f64 / (SHELLS_PER_MOLT * SHELLS_PER_MOLT)
}

/// Graduation threshold in MOLT
const GRADUATION_MCAP_MOLT: f64 = 100_000.0;

// ─────────────────────────────────────────────────────────────────────────────
// JSON Types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct PlatformStatsJson {
    token_count: u64,
    fees_collected: f64,
    total_graduated: u64,
    graduation_threshold: f64,
    creation_fee: f64,
    platform_fee_pct: u64,
    current_slot: u64,
}

#[derive(Serialize)]
struct LaunchpadConfigJson {
    creation_fee: f64,
    graduation_threshold: f64,
    platform_fee_pct: u64,
    base_price_raw: u64,
    slope: u64,
    slope_scale: u64,
}

#[derive(Serialize)]
struct TokenJson {
    id: u64,
    creator: String,
    supply_sold: f64,
    molt_raised: f64,
    current_price: f64,
    market_cap: f64,
    graduated: bool,
    created_at: u64,
    graduation_pct: f64,
}

#[derive(Deserialize)]
struct TokenListQuery {
    sort: Option<String>,   // "newest", "raised", "graduation", "price"
    filter: Option<String>, // "active", "graduated", "all"
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Deserialize)]
struct TokenHoldersQuery {
    address: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Decode helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Decode a 65-byte token record from cpt:{hex_id} key
/// Layout: creator(32) + supply_sold(8) + molt_raised(8) + max_supply(8) + created_at(8) + graduated(1)
fn decode_token(state: &RpcState, id: u64) -> Option<TokenJson> {
    let key = format!("cpt:{:016x}", id);
    let data = read_bytes(state, key.as_bytes())?;
    if data.len() < 65 {
        return None;
    }

    let creator = hex::encode(&data[0..32]);
    let supply_sold = u64_le(&data, 32);
    let molt_raised = u64_le(&data, 40);
    // max_supply at offset 48 — we compute price from supply_sold
    let created_at = u64_le(&data, 56);
    let graduated = data[64] != 0;

    let price = spot_price(supply_sold);
    let mcap = market_cap(supply_sold);
    let grad_pct = (mcap / GRADUATION_MCAP_MOLT * 100.0).min(100.0);

    Some(TokenJson {
        id,
        creator,
        supply_sold: supply_sold as f64 / SHELLS_PER_MOLT,
        molt_raised: molt_raised as f64 / SHELLS_PER_MOLT,
        current_price: price,
        market_cap: mcap,
        graduated,
        created_at,
        graduation_pct: grad_pct,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// GET /stats — Platform-wide launchpad statistics
async fn get_stats(State(state): State<Arc<RpcState>>) -> impl IntoResponse {
    let slot = current_slot(&state);
    let token_count = read_u64_key(&state, b"cp_token_count");
    let fees_raw = read_u64_key(&state, b"cp_fees_collected");

    // Count graduated tokens
    // P10-VAL-10: Cap iteration to prevent unbounded scan when token_count is large.
    // At >10k tokens, consider maintaining a persistent graduated counter instead.
    let scan_limit = token_count.min(10_000);
    let mut graduated = 0u64;
    for id in 1..=scan_limit {
        let key = format!("cpt:{:016x}", id);
        if let Some(data) = read_bytes(&state, key.as_bytes()) {
            if data.len() >= 65 && data[64] != 0 {
                graduated += 1;
            }
        }
    }

    ApiResponse::ok(
        PlatformStatsJson {
            token_count,
            fees_collected: fees_raw as f64 / SHELLS_PER_MOLT,
            total_graduated: graduated,
            graduation_threshold: GRADUATION_MCAP_MOLT,
            creation_fee: CREATION_FEE_MOLT,
            platform_fee_pct: PLATFORM_FEE_PCT,
            current_slot: slot,
        },
        slot,
    )
}

/// GET /config — Launchpad protocol constants used by frontend bootstrap UI
async fn get_config(State(state): State<Arc<RpcState>>) -> impl IntoResponse {
    let slot = current_slot(&state);
    ApiResponse::ok(
        LaunchpadConfigJson {
            creation_fee: CREATION_FEE_MOLT,
            graduation_threshold: GRADUATION_MCAP_MOLT,
            platform_fee_pct: PLATFORM_FEE_PCT,
            base_price_raw: BASE_PRICE,
            slope: SLOPE,
            slope_scale: SLOPE_SCALE,
        },
        slot,
    )
}

/// GET /tokens — List all launched tokens
async fn get_tokens(
    State(state): State<Arc<RpcState>>,
    Query(q): Query<TokenListQuery>,
) -> impl IntoResponse {
    let slot = current_slot(&state);
    let token_count = read_u64_key(&state, b"cp_token_count");
    let filter = q.filter.as_deref().unwrap_or("all");
    let sort_by = q.sort.as_deref().unwrap_or("newest");
    let limit = q.limit.unwrap_or(50).min(200);
    let offset = q.offset.unwrap_or(0);

    let mut tokens: Vec<TokenJson> = Vec::new();

    for id in 1..=token_count {
        if let Some(t) = decode_token(&state, id) {
            let include = match filter {
                "active" => !t.graduated,
                "graduated" => t.graduated,
                _ => true,
            };
            if include {
                tokens.push(t);
            }
        }
    }

    // Sort
    match sort_by {
        "raised" => tokens.sort_by(|a, b| {
            b.molt_raised
                .partial_cmp(&a.molt_raised)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "graduation" => tokens.sort_by(|a, b| {
            b.graduation_pct
                .partial_cmp(&a.graduation_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "price" => tokens.sort_by(|a, b| {
            b.current_price
                .partial_cmp(&a.current_price)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "mcap" => tokens.sort_by(|a, b| {
            b.market_cap
                .partial_cmp(&a.market_cap)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        _ => tokens.sort_by(|a, b| b.id.cmp(&a.id)), // newest first
    }

    // Paginate
    let total = tokens.len();
    let tokens: Vec<TokenJson> = tokens.into_iter().skip(offset).take(limit).collect();

    #[derive(Serialize)]
    struct TokenListResponse {
        tokens: Vec<TokenJson>,
        total: usize,
        offset: usize,
        limit: usize,
    }

    ApiResponse::ok(
        TokenListResponse {
            tokens,
            total,
            offset,
            limit,
        },
        slot,
    )
}

/// GET /tokens/:id — Get single token info
async fn get_token(State(state): State<Arc<RpcState>>, Path(id): Path<u64>) -> Response {
    let slot = current_slot(&state);
    match decode_token(&state, id) {
        Some(t) => ApiResponse::ok(t, slot).into_response(),
        None => api_404(&format!("Token {} not found", id)),
    }
}

/// GET /tokens/:id/quote — Get buy quote (how many tokens for X MOLT)
async fn get_buy_quote(
    State(state): State<Arc<RpcState>>,
    Path(id): Path<u64>,
    Query(q): Query<QuoteQuery>,
) -> Response {
    let slot = current_slot(&state);
    let key = format!("cpt:{:016x}", id);
    let data = match read_bytes(&state, key.as_bytes()) {
        Some(d) if d.len() >= 65 => d,
        _ => return api_404(&format!("Token {} not found", id)),
    };

    if data[64] != 0 {
        return api_err("Token has graduated — trade on DEX");
    }

    let supply = u64_le(&data, 32);
    let molt_amount_f = q.amount.unwrap_or(1.0);
    let molt_shells = (molt_amount_f * SHELLS_PER_MOLT) as u128;

    // Deduct 1% platform fee
    let after_fee = molt_shells * 99 / 100;

    // Binary search for tokens received (matching contract logic)
    let tokens_out = compute_buy_tokens(supply, after_fee);
    let tokens_f = tokens_out as f64 / SHELLS_PER_MOLT;
    let price_after = spot_price(supply + tokens_out);
    let price_impact = if spot_price(supply) > 0.0 {
        (price_after - spot_price(supply)) / spot_price(supply) * 100.0
    } else {
        0.0
    };

    #[derive(Serialize)]
    struct QuoteResponse {
        tokens_received: f64,
        price_before: f64,
        price_after: f64,
        price_impact_pct: f64,
        platform_fee_pct: u64,
        molt_input: f64,
    }

    ApiResponse::ok(
        QuoteResponse {
            tokens_received: tokens_f,
            price_before: spot_price(supply),
            price_after,
            price_impact_pct: price_impact,
            platform_fee_pct: 1,
            molt_input: molt_amount_f,
        },
        slot,
    )
    .into_response()
}

#[derive(Deserialize)]
struct QuoteQuery {
    amount: Option<f64>, // MOLT amount (human-readable, e.g. 100.0)
}

/// Compute how many tokens you get for `after_fee_shells` shells at current supply
///
/// AUDIT-FIX F-8: Use u128 fixed-point arithmetic instead of f64 to avoid
/// precision loss above ~9M MOLT.
fn compute_buy_tokens(supply: u64, after_fee_shells: u128) -> u64 {
    // Quadratic: SLOPE/(2*SLOPE_SCALE) * a^2 + (BASE_PRICE + SLOPE*s/SLOPE_SCALE) * a - after_fee_shells = 0
    // Multiply everything by 2*SLOPE_SCALE to clear fractions:
    //   SLOPE * a^2 + 2*SLOPE_SCALE*(BASE_PRICE + SLOPE*s/SLOPE_SCALE) * a - 2*SLOPE_SCALE*after_fee_shells = 0
    //   SLOPE * a^2 + (2*SLOPE_SCALE*BASE_PRICE + 2*SLOPE*s) * a - 2*SLOPE_SCALE*after_fee_shells = 0
    //
    // Using quadratic formula: a = (-B + sqrt(B^2 + 4*A*C)) / (2*A)
    // where A = SLOPE, B = 2*SLOPE_SCALE*BASE_PRICE + 2*SLOPE*s, C = 2*SLOPE_SCALE*after_fee_shells
    let s = supply as u128;
    let a_coeff = SLOPE as u128;
    let b_coeff = 2u128 * SLOPE_SCALE as u128 * BASE_PRICE as u128 + 2u128 * SLOPE as u128 * s;
    let c_val = 2u128 * SLOPE_SCALE as u128 * after_fee_shells;

    // discriminant = B^2 + 4*A*C
    let discriminant = b_coeff.checked_mul(b_coeff).and_then(|b2| {
        let four_ac = 4u128.checked_mul(a_coeff)?.checked_mul(c_val)?;
        b2.checked_add(four_ac)
    });

    let discriminant = match discriminant {
        Some(d) => d,
        None => return 0, // overflow — amount too large
    };

    let sqrt_d = isqrt_u128(discriminant);

    // a = (-B + sqrt(discriminant)) / (2*A)
    // Since B > 0, we need sqrt(discriminant) > B for positive result
    if sqrt_d <= b_coeff {
        return 0;
    }
    let numerator = sqrt_d - b_coeff;
    let denominator = 2u128 * a_coeff;
    let tokens = numerator / denominator;

    if tokens > u64::MAX as u128 {
        u64::MAX
    } else {
        tokens as u64
    }
}

/// Integer square root for u128 using Newton's method
fn isqrt_u128(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// GET /tokens/:id/holders — Get user balance for a token
async fn get_holder_balance(
    State(state): State<Arc<RpcState>>,
    Path(id): Path<u64>,
    Query(q): Query<TokenHoldersQuery>,
) -> Response {
    let slot = current_slot(&state);
    let addr = match q.address {
        Some(ref a) if !a.is_empty() => a.clone(),
        _ => return api_err("address query parameter required"),
    };

    // Check token exists
    let key = format!("cpt:{:016x}", id);
    if read_bytes(&state, key.as_bytes()).is_none() {
        return api_404(&format!("Token {} not found", id));
    }

    let bal_key = format!("bal:{:016x}:{}", id, addr);
    let balance = read_u64_key(&state, bal_key.as_bytes());

    #[derive(Serialize)]
    struct HolderBalance {
        token_id: u64,
        address: String,
        balance: f64,
        balance_raw: u64,
    }

    ApiResponse::ok(
        HolderBalance {
            token_id: id,
            address: addr,
            balance: balance as f64 / SHELLS_PER_MOLT,
            balance_raw: balance,
        },
        slot,
    )
    .into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// PUBLIC: Build the Launchpad API router
// ═══════════════════════════════════════════════════════════════════════════════

/// Build the /api/v1/launchpad/* router.
pub(crate) fn build_launchpad_router() -> Router<Arc<RpcState>> {
    Router::new()
        .route("/config", get(get_config))
        .route("/stats", get(get_stats))
        .route("/tokens", get(get_tokens))
        .route("/tokens/:id", get(get_token))
        .route("/tokens/:id/quote", get(get_buy_quote))
        .route("/tokens/:id/holders", get(get_holder_balance))
}
