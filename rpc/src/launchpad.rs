// ═══════════════════════════════════════════════════════════════════════════════
// Lichen RPC — SporePump Launchpad REST API Module
// Implements /api/v1/launchpad/* endpoints for the bonding-curve token launcher
//
// Reads contract storage directly from StateStore using the SporePump
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

use crate::{RpcError, RpcState};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

const SPOREPUMP_PROGRAM: &str = "SPOREPUMP";
const SPORES_PER_LICN: f64 = 1_000_000_000.0;
const BASE_PRICE: u64 = 1_000;
const SLOPE: u64 = 1;
const SLOPE_SCALE: u64 = 1_000_000;
const CREATION_FEE_LICN: f64 = 10.0;
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
    state.state.get_program_storage(SPOREPUMP_PROGRAM, key)
}

fn read_u64_key(state: &RpcState, key: &[u8]) -> u64 {
    state.state.get_program_storage_u64(SPOREPUMP_PROGRAM, key)
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
    let price_spores = BASE_PRICE as f64 + (supply as f64 * SLOPE as f64 / SLOPE_SCALE as f64);
    price_spores / SPORES_PER_LICN
}

/// Compute market cap: spot_price(supply) * supply / 1e9
fn market_cap(supply: u64) -> f64 {
    let price_spores = BASE_PRICE as u128 + (supply as u128 * SLOPE as u128 / SLOPE_SCALE as u128);
    (price_spores * supply as u128) as f64 / (SPORES_PER_LICN * SPORES_PER_LICN)
}

/// Graduation threshold in LICN
const GRADUATION_MCAP_LICN: f64 = 100_000.0;

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

fn collect_platform_stats(state: &RpcState) -> PlatformStatsJson {
    let slot = current_slot(state);
    let token_count = read_u64_key(state, b"cp_token_count");
    let fees_raw = read_u64_key(state, b"cp_fees_collected");

    // Count graduated tokens.
    // Cap the scan to avoid unbounded per-request work when token_count becomes very large.
    let scan_limit = token_count.min(10_000);
    let mut graduated = 0u64;
    for id in 1..=scan_limit {
        let key = format!("cpt:{:016x}", id);
        if let Some(data) = read_bytes(state, key.as_bytes()) {
            if data.len() >= 65 && data[64] != 0 {
                graduated += 1;
            }
        }
    }

    PlatformStatsJson {
        token_count,
        fees_collected: fees_raw as f64 / SPORES_PER_LICN,
        total_graduated: graduated,
        graduation_threshold: GRADUATION_MCAP_LICN,
        creation_fee: CREATION_FEE_LICN,
        platform_fee_pct: PLATFORM_FEE_PCT,
        current_slot: slot,
    }
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
    licn_raised: f64,
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
/// Layout: creator(32) + supply_sold(8) + licn_raised(8) + max_supply(8) + created_at(8) + graduated(1)
fn decode_token(state: &RpcState, id: u64) -> Option<TokenJson> {
    let key = format!("cpt:{:016x}", id);
    let data = read_bytes(state, key.as_bytes())?;
    if data.len() < 65 {
        return None;
    }

    let creator = hex::encode(&data[0..32]);
    let supply_sold = u64_le(&data, 32);
    let licn_raised = u64_le(&data, 40);
    // max_supply at offset 48 — we compute price from supply_sold
    let created_at = u64_le(&data, 56);
    let graduated = data[64] != 0;

    let price = spot_price(supply_sold);
    let mcap = market_cap(supply_sold);
    let grad_pct = (mcap / GRADUATION_MCAP_LICN * 100.0).min(100.0);

    Some(TokenJson {
        id,
        creator,
        supply_sold: supply_sold as f64 / SPORES_PER_LICN,
        licn_raised: licn_raised as f64 / SPORES_PER_LICN,
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
    ApiResponse::ok(collect_platform_stats(&state), slot)
}

pub(crate) async fn handle_get_sporepump_stats(
    state: &RpcState,
) -> Result<serde_json::Value, RpcError> {
    serde_json::to_value(collect_platform_stats(state)).map_err(|err| RpcError {
        code: -32603,
        message: format!("Failed to serialize SporePump stats: {err}"),
    })
}

/// GET /config — Launchpad protocol constants used by frontend bootstrap UI
async fn get_config(State(state): State<Arc<RpcState>>) -> impl IntoResponse {
    let slot = current_slot(&state);
    ApiResponse::ok(
        LaunchpadConfigJson {
            creation_fee: CREATION_FEE_LICN,
            graduation_threshold: GRADUATION_MCAP_LICN,
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
            b.licn_raised
                .partial_cmp(&a.licn_raised)
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
        _ => tokens.sort_by_key(|b| std::cmp::Reverse(b.id)), // newest first
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

/// GET /tokens/:id/quote — Get buy quote (how many tokens for X LICN)
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
    let licn_amount_f = q.amount.unwrap_or(1.0);
    let licn_spores = (licn_amount_f * SPORES_PER_LICN) as u128;

    // Deduct 1% platform fee
    let after_fee = licn_spores * 99 / 100;

    // Binary search for tokens received (matching contract logic)
    let tokens_out = match compute_buy_tokens(supply, after_fee) {
        Ok(t) => t,
        Err(e) => return api_err(e),
    };
    let tokens_f = tokens_out as f64 / SPORES_PER_LICN;
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
        licn_input: f64,
    }

    ApiResponse::ok(
        QuoteResponse {
            tokens_received: tokens_f,
            price_before: spot_price(supply),
            price_after,
            price_impact_pct: price_impact,
            platform_fee_pct: 1,
            licn_input: licn_amount_f,
        },
        slot,
    )
    .into_response()
}

#[derive(Deserialize)]
struct QuoteQuery {
    amount: Option<f64>, // LICN amount (human-readable, e.g. 100.0)
}

/// Compute how many tokens you get for `after_fee_spores` spores at current supply
///
/// AUDIT-FIX F-8: Use u128 fixed-point arithmetic instead of f64 to avoid
/// precision loss above ~9M LICN.
/// AUDIT-FIX C8: Return Result instead of silently capping on overflow.
fn compute_buy_tokens(supply: u64, after_fee_spores: u128) -> Result<u64, &'static str> {
    let s = supply as u128;
    let a_coeff = SLOPE as u128;
    let b_coeff = 2u128 * SLOPE_SCALE as u128 * BASE_PRICE as u128 + 2u128 * SLOPE as u128 * s;
    let c_val = 2u128 * SLOPE_SCALE as u128 * after_fee_spores;

    // discriminant = B^2 + 4*A*C
    let discriminant = b_coeff.checked_mul(b_coeff).and_then(|b2| {
        let four_ac = 4u128.checked_mul(a_coeff)?.checked_mul(c_val)?;
        b2.checked_add(four_ac)
    });

    let discriminant = match discriminant {
        Some(d) => d,
        None => return Err("Amount too large for bonding curve calculation"),
    };

    let sqrt_d = isqrt_u128(discriminant);

    if sqrt_d <= b_coeff {
        return Ok(0);
    }
    let numerator = sqrt_d - b_coeff;
    let denominator = 2u128 * a_coeff;
    let tokens = numerator / denominator;

    if tokens > u64::MAX as u128 {
        Err("Token amount exceeds maximum representable value")
    } else {
        Ok(tokens as u64)
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
            balance: balance as f64 / SPORES_PER_LICN,
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── Constants sanity ──

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn constants_sane() {
        assert!(BASE_PRICE > 0);
        assert!(SLOPE > 0);
        assert!(SLOPE_SCALE > 0);
        assert!(SPORES_PER_LICN > 0.0);
        assert!(CREATION_FEE_LICN > 0.0);
        assert!(GRADUATION_MCAP_LICN > 0.0);
    }

    // ── spot_price ──

    #[test]
    fn spot_price_at_zero_supply() {
        let p = spot_price(0);
        // At supply=0: price = BASE_PRICE / SPORES_PER_LICN
        let expected = BASE_PRICE as f64 / SPORES_PER_LICN;
        assert!(
            (p - expected).abs() < 1e-15,
            "spot_price(0) = {}, expected {}",
            p,
            expected
        );
    }

    #[test]
    fn spot_price_increases_with_supply() {
        let p0 = spot_price(0);
        let p1 = spot_price(1_000_000_000);
        let p2 = spot_price(10_000_000_000);
        assert!(p1 > p0, "Price should increase with supply");
        assert!(p2 > p1, "Price should increase with supply");
    }

    #[test]
    fn spot_price_monotonic() {
        let mut prev = spot_price(0);
        for supply in (1_000_000..=100_000_000).step_by(1_000_000) {
            let p = spot_price(supply);
            assert!(p >= prev, "spot_price must be monotonically non-decreasing");
            prev = p;
        }
    }

    // ── market_cap ──

    #[test]
    fn market_cap_zero_at_zero_supply() {
        assert_eq!(market_cap(0), 0.0);
    }

    #[test]
    fn market_cap_increases_with_supply() {
        let m0 = market_cap(0);
        let m1 = market_cap(1_000_000_000);
        let m2 = market_cap(10_000_000_000);
        assert!(m1 > m0);
        assert!(m2 > m1);
    }

    // ── isqrt_u128 ──

    #[test]
    fn isqrt_zero() {
        assert_eq!(isqrt_u128(0), 0);
    }

    #[test]
    fn isqrt_one() {
        assert_eq!(isqrt_u128(1), 1);
    }

    #[test]
    fn isqrt_perfect_squares() {
        assert_eq!(isqrt_u128(4), 2);
        assert_eq!(isqrt_u128(9), 3);
        assert_eq!(isqrt_u128(16), 4);
        assert_eq!(isqrt_u128(100), 10);
        assert_eq!(isqrt_u128(10000), 100);
        assert_eq!(isqrt_u128(1_000_000), 1000);
    }

    #[test]
    fn isqrt_non_perfect_squares() {
        // isqrt should floor
        assert_eq!(isqrt_u128(2), 1);
        assert_eq!(isqrt_u128(3), 1);
        assert_eq!(isqrt_u128(5), 2);
        assert_eq!(isqrt_u128(8), 2);
        assert_eq!(isqrt_u128(99), 9);
    }

    #[test]
    fn isqrt_large_values() {
        let n = 1u128 << 64; // 2^64
        let s = isqrt_u128(n);
        assert_eq!(s, 1u128 << 32); // 2^32
    }

    // ── compute_buy_tokens ──

    #[test]
    fn buy_tokens_zero_input_returns_zero() {
        assert_eq!(compute_buy_tokens(0, 0).unwrap(), 0);
    }

    #[test]
    fn buy_tokens_positive_input() {
        // With some spores, we should get tokens
        let tokens = compute_buy_tokens(0, 1_000_000_000).unwrap(); // 1 LICN worth
        assert!(tokens > 0, "Should receive >0 tokens for 1 LICN");
    }

    #[test]
    fn buy_tokens_more_input_more_output() {
        let t1 = compute_buy_tokens(0, 1_000_000_000).unwrap();
        let t2 = compute_buy_tokens(0, 10_000_000_000).unwrap();
        assert!(t2 > t1, "More LICN in should yield more tokens");
    }

    #[test]
    fn buy_tokens_higher_supply_fewer_tokens() {
        // At higher supply, same input yields fewer tokens (bonding curve)
        let t_low = compute_buy_tokens(0, 1_000_000_000).unwrap();
        let t_high = compute_buy_tokens(100_000_000_000, 1_000_000_000).unwrap();
        assert!(
            t_low > t_high,
            "Higher supply should yield fewer tokens per LICN"
        );
    }

    // ── u64_le helper ──

    #[test]
    fn u64_le_reads_correctly() {
        let val: u64 = 0x0102030405060708;
        let bytes = val.to_le_bytes();
        let mut data = vec![0u8; 16];
        data[4..12].copy_from_slice(&bytes);
        assert_eq!(u64_le(&data, 4), val);
    }

    #[test]
    fn u64_le_out_of_bounds_returns_zero() {
        let data = [0u8; 4]; // too short
        assert_eq!(u64_le(&data, 0), 0);
    }

    #[test]
    fn u64_le_at_end() {
        let val: u64 = 42;
        let data = val.to_le_bytes().to_vec();
        assert_eq!(u64_le(&data, 0), 42);
    }
}
