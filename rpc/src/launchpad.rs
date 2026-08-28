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
use lichen_core::Pubkey;
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
const TOKEN_UNIT: u128 = 1_000_000_000;
const CREATION_FEE_LICN: f64 = 10.0;
const PLATFORM_FEE_PCT: u64 = 1;
const PLATFORM_FEE_BPS: u64 = PLATFORM_FEE_PCT * 100;
const DEFAULT_CREATOR_ROYALTY_BPS: u64 = 50;
const BPS_SCALE: u64 = 10_000;
const DEFAULT_BUY_COOLDOWN_SLOTS: u64 = 5;
const DEFAULT_SELL_COOLDOWN_SLOTS: u64 = 13;
const DEFAULT_MAX_BUY_AMOUNT: u64 = 100_000_000_000_000;
const MAX_FILTERED_SORT_SCAN: u64 = 10_000;
const MAX_TOKEN_NAME_LEN: usize = 64;
const MAX_TOKEN_SYMBOL_LEN: usize = 12;

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

fn api_internal(msg: &str, slot: u64) -> Response {
    let body = ApiResponse::<()> {
        success: false,
        data: None,
        error: Some(msg.to_string()),
        slot,
    };
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
}

fn api_unprocessable(msg: &str, slot: u64) -> Response {
    let body = ApiResponse::<()> {
        success: false,
        data: None,
        error: Some(msg.to_string()),
        slot,
    };
    (StatusCode::UNPROCESSABLE_ENTITY, Json(body)).into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// Storage Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn read_bytes(state: &RpcState, key: &[u8]) -> Option<Vec<u8>> {
    state.state.get_program_storage(SPOREPUMP_PROGRAM, key)
}

fn read_exact_u64(state: &RpcState, key: &[u8]) -> Result<Option<u64>, String> {
    match read_bytes(state, key) {
        Some(value) if value.len() == 8 => Ok(Some(u64_le(&value, 0))),
        Some(value) => Err(format!(
            "malformed SporePump key {}: expected 8 bytes, got {}",
            String::from_utf8_lossy(key),
            value.len()
        )),
        None => Ok(None),
    }
}

fn read_exact_u64_or_zero(state: &RpcState, key: &[u8]) -> Result<u64, String> {
    Ok(read_exact_u64(state, key)?.unwrap_or(0))
}

fn read_config_u64(state: &RpcState, key: &[u8], default: u64) -> Result<u64, String> {
    Ok(read_exact_u64(state, key)?.unwrap_or(default))
}

fn read_exact_bool(state: &RpcState, key: &[u8], default: bool) -> Result<bool, String> {
    match read_bytes(state, key) {
        Some(value) if value == [0] => Ok(false),
        Some(value) if value == [1] => Ok(true),
        Some(value) => Err(format!(
            "malformed SporePump key {}: expected one boolean byte, got {}",
            String::from_utf8_lossy(key),
            value.len()
        )),
        None => Ok(default),
    }
}

fn accounting_ready(state: &RpcState) -> Result<bool, String> {
    let version = read_exact_u64_or_zero(state, b"cp_account_version")?;
    let locked = read_exact_bool(state, b"cp_account_migration_lock", false)?;
    Ok(version == 3 && !locked)
}

fn token_frozen(state: &RpcState, id: u64) -> Result<bool, String> {
    read_exact_bool(state, graduation_key("cpf:", id).as_bytes(), false)
}

fn read_token_metadata(state: &RpcState, id: u64) -> Result<(String, String), String> {
    let name_key = graduation_key("cpn:", id);
    let name_bytes = read_bytes(state, name_key.as_bytes())
        .ok_or_else(|| format!("missing canonical SporePump metadata key {name_key}"))?;
    if name_bytes.is_empty()
        || name_bytes.len() > MAX_TOKEN_NAME_LEN
        || name_bytes.first().is_some_and(u8::is_ascii_whitespace)
        || name_bytes.last().is_some_and(u8::is_ascii_whitespace)
    {
        return Err(format!("malformed SporePump token {id} name"));
    }
    let name = String::from_utf8(name_bytes)
        .map_err(|_| format!("malformed SporePump token {id} name encoding"))?;
    if name.chars().any(char::is_control) {
        return Err(format!("malformed SporePump token {id} name"));
    }

    let symbol_key = graduation_key("cpsy:", id);
    let symbol_bytes = read_bytes(state, symbol_key.as_bytes())
        .ok_or_else(|| format!("missing canonical SporePump metadata key {symbol_key}"))?;
    if symbol_bytes.len() < 2
        || symbol_bytes.len() > MAX_TOKEN_SYMBOL_LEN
        || !symbol_bytes.first().is_some_and(u8::is_ascii_alphabetic)
        || !symbol_bytes.iter().all(u8::is_ascii_alphanumeric)
        || symbol_bytes.iter().any(u8::is_ascii_lowercase)
    {
        return Err(format!("malformed SporePump token {id} symbol"));
    }
    let symbol = String::from_utf8(symbol_bytes)
        .map_err(|_| format!("malformed SporePump token {id} symbol encoding"))?;
    let index_key = format!("cpsym:{symbol}");
    if read_exact_u64(state, index_key.as_bytes())? != Some(id) {
        return Err(format!(
            "inconsistent SporePump token {id}: symbol index does not reference this token"
        ));
    }
    Ok((name, symbol))
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

fn graduation_key(prefix: &str, id: u64) -> String {
    format!("{}{:016x}", prefix, id)
}

fn graduation_state_name(state: u8) -> &'static str {
    match state {
        1 => "eligible",
        2 => "migrating",
        3 => "graduated",
        _ => "active",
    }
}

fn read_graduation_state(state: &RpcState, id: u64, graduated_flag: u8) -> Result<u8, String> {
    if graduated_flag > 1 {
        return Err(format!(
            "malformed SporePump token {id}: graduated flag is not boolean"
        ));
    }
    let state_key = graduation_key("cpgs:", id);
    let stored_state = match read_bytes(state, state_key.as_bytes()) {
        Some(value) if value.len() == 1 && value[0] <= 3 => value[0],
        Some(value) => {
            return Err(format!(
                "malformed SporePump key {state_key}: expected one lifecycle byte, got {} bytes",
                value.len()
            ));
        }
        None => 0,
    };
    if (graduated_flag == 1) != (stored_state == 3) {
        return Err(format!(
            "inconsistent SporePump token {id}: graduated flag and lifecycle disagree"
        ));
    }
    Ok(stored_state)
}

fn optional_id(state: &RpcState, prefix: &str, id: u64) -> Result<Option<u64>, String> {
    let key = graduation_key(prefix, id);
    match read_exact_u64(state, key.as_bytes())? {
        Some(0) => Err(format!("malformed SporePump key {key}: identifier is zero")),
        value => Ok(value),
    }
}

fn optional_address(state: &RpcState, prefix: &str, id: u64) -> Result<Option<String>, String> {
    let key = graduation_key(prefix, id);
    match read_bytes(state, key.as_bytes()) {
        Some(data) if data.len() == 32 && data.iter().any(|byte| *byte != 0) => {
            Ok(Some(hex::encode(data)))
        }
        Some(data) if data.len() == 32 => {
            Err(format!("malformed SporePump key {key}: address is zero"))
        }
        Some(data) => Err(format!(
            "malformed SporePump key {key}: expected 32 bytes, got {}",
            data.len()
        )),
        None => Ok(None),
    }
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
    token_count_exact: String,
    fees_collected: f64,
    platform_fees_raw: u64,
    platform_fees_raw_exact: String,
    total_raised: f64,
    curve_reserve_raw: u64,
    curve_reserve_raw_exact: String,
    creator_liability: f64,
    creator_liability_raw: u64,
    creator_liability_raw_exact: String,
    cumulative_graduation_revenue: f64,
    cumulative_graduation_revenue_raw: u64,
    cumulative_graduation_revenue_raw_exact: String,
    total_graduated: u64,
    total_graduated_exact: String,
    accounting_version: u64,
    accounting_ready: bool,
    paused: bool,
    accounting_migration_locked: bool,
    accounting_migration_expected: u64,
    accounting_migration_expected_exact: String,
    accounting_migration_cursor: u64,
    accounting_migration_cursor_exact: String,
    graduation_threshold: f64,
    creation_fee: f64,
    platform_fee_pct: u64,
    creator_royalty_bps: u64,
    current_slot: u64,
    current_slot_exact: String,
}

fn collect_platform_stats(state: &RpcState) -> Result<PlatformStatsJson, String> {
    let slot = current_slot(state);
    let token_count = read_exact_u64_or_zero(state, b"cp_token_count")?;
    let fees_raw = read_exact_u64_or_zero(state, b"cp_fees_collected")?;
    let curve_reserve_raw = read_exact_u64_or_zero(state, b"cp_curve_reserve")?;
    let creator_liability_raw = read_exact_u64_or_zero(state, b"cp_creator_liability")?;
    let graduation_revenue_raw = read_exact_u64_or_zero(state, b"cp_graduation_revenue")?;
    let total_graduated = read_exact_u64_or_zero(state, b"cp_total_graduated")?;
    let accounting_version = read_exact_u64_or_zero(state, b"cp_account_version")?;
    let accounting_migration_expected =
        read_exact_u64_or_zero(state, b"cp_account_migration_expected")?;
    let accounting_migration_cursor =
        read_exact_u64_or_zero(state, b"cp_account_migration_cursor")?;
    let creator_royalty_bps =
        read_config_u64(state, b"cp_creator_royalty", DEFAULT_CREATOR_ROYALTY_BPS)?;
    if creator_royalty_bps > 1_000 {
        return Err("SporePump creator royalty exceeds the contract maximum".to_string());
    }
    let accounting_migration_locked = read_exact_bool(state, b"cp_account_migration_lock", false)?;
    let paused = read_exact_bool(state, b"cp_paused", false)?;

    Ok(PlatformStatsJson {
        token_count,
        token_count_exact: token_count.to_string(),
        fees_collected: fees_raw as f64 / SPORES_PER_LICN,
        platform_fees_raw: fees_raw,
        platform_fees_raw_exact: fees_raw.to_string(),
        total_raised: curve_reserve_raw as f64 / SPORES_PER_LICN,
        curve_reserve_raw,
        curve_reserve_raw_exact: curve_reserve_raw.to_string(),
        creator_liability: creator_liability_raw as f64 / SPORES_PER_LICN,
        creator_liability_raw,
        creator_liability_raw_exact: creator_liability_raw.to_string(),
        cumulative_graduation_revenue: graduation_revenue_raw as f64 / SPORES_PER_LICN,
        cumulative_graduation_revenue_raw: graduation_revenue_raw,
        cumulative_graduation_revenue_raw_exact: graduation_revenue_raw.to_string(),
        total_graduated,
        total_graduated_exact: total_graduated.to_string(),
        accounting_version,
        accounting_ready: accounting_version == 3 && !accounting_migration_locked,
        paused,
        accounting_migration_locked,
        accounting_migration_expected,
        accounting_migration_expected_exact: accounting_migration_expected.to_string(),
        accounting_migration_cursor,
        accounting_migration_cursor_exact: accounting_migration_cursor.to_string(),
        graduation_threshold: GRADUATION_MCAP_LICN,
        creation_fee: CREATION_FEE_LICN,
        platform_fee_pct: PLATFORM_FEE_PCT,
        creator_royalty_bps,
        current_slot: slot,
        current_slot_exact: slot.to_string(),
    })
}

#[derive(Serialize)]
struct LaunchpadConfigJson {
    creation_fee: f64,
    graduation_threshold: f64,
    platform_fee_pct: u64,
    creator_royalty_bps: u64,
    buy_cooldown_slots: u64,
    sell_cooldown_slots: u64,
    max_buy_raw: u64,
    max_buy_raw_exact: String,
    base_price_raw: u64,
    slope: u64,
    slope_scale: u64,
}

#[derive(Serialize)]
struct TokenJson {
    id: u64,
    id_exact: String,
    name: String,
    symbol: String,
    creator: String,
    creator_royalty_raw: u64,
    creator_royalty_raw_exact: String,
    supply_sold_raw: u64,
    supply_sold_raw_exact: String,
    supply_sold: f64,
    licn_raised_raw: u64,
    licn_raised_raw_exact: String,
    licn_raised: f64,
    max_supply_raw: u64,
    max_supply_raw_exact: String,
    current_price: f64,
    market_cap: f64,
    graduated: bool,
    frozen: bool,
    graduation_state: &'static str,
    graduation_state_code: u8,
    eligibility_slot: u64,
    eligibility_slot_exact: String,
    migration_boundary_slot: u64,
    migration_boundary_slot_exact: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    migrated_token_program: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pair_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pair_id_exact: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pool_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pool_id_exact: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route_id_exact: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reverse_route_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reverse_route_id_exact: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position_id_exact: Option<String>,
    quote_symbol: &'static str,
    licn_liquidity_raw: u64,
    licn_liquidity_raw_exact: String,
    token_liquidity_raw: u64,
    token_liquidity_raw_exact: String,
    protocol_token_inventory_raw: u64,
    protocol_token_inventory_raw_exact: String,
    created_at: u64,
    created_at_exact: String,
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
fn decode_token(state: &RpcState, id: u64) -> Result<Option<TokenJson>, String> {
    let key = format!("cpt:{:016x}", id);
    let Some(data) = read_bytes(state, key.as_bytes()) else {
        return Ok(None);
    };
    if data.len() != 65 {
        return Err(format!(
            "malformed SporePump key {key}: expected 65 bytes, got {}",
            data.len()
        ));
    }
    if data[0..32].iter().all(|byte| *byte == 0) {
        return Err(format!("malformed SporePump token {id}: creator is zero"));
    }

    let creator = hex::encode(&data[0..32]);
    let supply_sold = u64_le(&data, 32);
    let licn_raised = u64_le(&data, 40);
    let max_supply = u64_le(&data, 48);
    if supply_sold > max_supply {
        return Err(format!(
            "malformed SporePump token {id}: supply exceeds max supply"
        ));
    }
    let created_at = u64_le(&data, 56);
    let graduation_state = read_graduation_state(state, id, data[64])?;
    let graduated = graduation_state == 3;
    let frozen = token_frozen(state, id)?;
    let liquidity_key = graduation_key("cpgl:", id);
    let liquidity = match read_bytes(state, liquidity_key.as_bytes()) {
        Some(value) if value.len() == 16 => value,
        Some(value) => {
            return Err(format!(
                "malformed SporePump key {liquidity_key}: expected 16 bytes, got {}",
                value.len()
            ));
        }
        None => vec![0u8; 16],
    };

    let price = spot_price(supply_sold);
    let mcap = market_cap(supply_sold);
    let grad_pct = (mcap / GRADUATION_MCAP_LICN * 100.0).min(100.0);
    let (name, symbol) = read_token_metadata(state, id)?;

    let creator_royalty_raw =
        read_exact_u64_or_zero(state, format!("cry:{:016x}:{}", id, creator).as_bytes())?;
    let eligibility_slot = read_exact_u64_or_zero(state, graduation_key("cpge:", id).as_bytes())?;
    let migration_boundary_slot =
        read_exact_u64_or_zero(state, graduation_key("cpgb:", id).as_bytes())?;
    let pair_id = optional_id(state, "cpgp:", id)?;
    let pool_id = optional_id(state, "cpga:", id)?;
    let route_id = optional_id(state, "cpgr:", id)?;
    let reverse_route_id = optional_id(state, "cpgr2:", id)?;
    let position_id = optional_id(state, "cpgpos:", id)?;
    let licn_liquidity_raw = u64_le(&liquidity, 0);
    let token_liquidity_raw = u64_le(&liquidity, 8);
    let protocol_token_inventory_raw =
        read_exact_u64_or_zero(state, graduation_key("cpgx:", id).as_bytes())?;

    Ok(Some(TokenJson {
        id,
        id_exact: id.to_string(),
        name,
        symbol,
        creator_royalty_raw,
        creator_royalty_raw_exact: creator_royalty_raw.to_string(),
        creator,
        supply_sold_raw: supply_sold,
        supply_sold_raw_exact: supply_sold.to_string(),
        supply_sold: supply_sold as f64 / SPORES_PER_LICN,
        licn_raised_raw: licn_raised,
        licn_raised_raw_exact: licn_raised.to_string(),
        licn_raised: licn_raised as f64 / SPORES_PER_LICN,
        max_supply_raw: max_supply,
        max_supply_raw_exact: max_supply.to_string(),
        current_price: price,
        market_cap: mcap,
        graduated,
        frozen,
        graduation_state: graduation_state_name(graduation_state),
        graduation_state_code: graduation_state,
        eligibility_slot,
        eligibility_slot_exact: eligibility_slot.to_string(),
        migration_boundary_slot,
        migration_boundary_slot_exact: migration_boundary_slot.to_string(),
        migrated_token_program: optional_address(state, "cpgt:", id)?,
        pair_id,
        pair_id_exact: pair_id.map(|value| value.to_string()),
        pool_id,
        pool_id_exact: pool_id.map(|value| value.to_string()),
        route_id,
        route_id_exact: route_id.map(|value| value.to_string()),
        reverse_route_id,
        reverse_route_id_exact: reverse_route_id.map(|value| value.to_string()),
        position_id,
        position_id_exact: position_id.map(|value| value.to_string()),
        quote_symbol: "LICN",
        licn_liquidity_raw,
        licn_liquidity_raw_exact: licn_liquidity_raw.to_string(),
        token_liquidity_raw,
        token_liquidity_raw_exact: token_liquidity_raw.to_string(),
        protocol_token_inventory_raw,
        protocol_token_inventory_raw_exact: protocol_token_inventory_raw.to_string(),
        created_at,
        created_at_exact: created_at.to_string(),
        graduation_pct: grad_pct,
    }))
}

fn account_key_component(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.len() == 64 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(trimmed.to_ascii_lowercase());
    }
    Pubkey::from_base58(trimmed)
        .ok()
        .map(|pubkey| hex::encode(pubkey.0))
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// GET /stats — Platform-wide launchpad statistics
async fn get_stats(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    match collect_platform_stats(&state) {
        Ok(stats) => ApiResponse::ok(stats, slot).into_response(),
        Err(error) => api_internal(&error, slot),
    }
}

pub(crate) async fn handle_get_sporepump_stats(
    state: &RpcState,
) -> Result<serde_json::Value, RpcError> {
    let stats = collect_platform_stats(state).map_err(|err| RpcError {
        code: -32603,
        message: err,
    })?;
    serde_json::to_value(stats).map_err(|err| RpcError {
        code: -32603,
        message: format!("Failed to serialize SporePump stats: {err}"),
    })
}

/// GET /config — Launchpad protocol constants used by frontend bootstrap UI
async fn get_config(State(state): State<Arc<RpcState>>) -> Response {
    let slot = current_slot(&state);
    let creator_royalty_bps =
        match read_config_u64(&state, b"cp_creator_royalty", DEFAULT_CREATOR_ROYALTY_BPS) {
            Ok(value) if value <= 1_000 => value,
            Ok(_) => return api_internal("SporePump creator royalty is out of range", slot),
            Err(error) => return api_internal(&error, slot),
        };
    let buy_cooldown_slots =
        match read_config_u64(&state, b"cp_buy_cooldown", DEFAULT_BUY_COOLDOWN_SLOTS) {
            Ok(value) => value,
            Err(error) => return api_internal(&error, slot),
        };
    let sell_cooldown_slots =
        match read_config_u64(&state, b"cp_sell_cooldown", DEFAULT_SELL_COOLDOWN_SLOTS) {
            Ok(value) => value,
            Err(error) => return api_internal(&error, slot),
        };
    let max_buy_raw = match read_config_u64(&state, b"cp_max_buy", DEFAULT_MAX_BUY_AMOUNT) {
        Ok(value) if value > 0 => value,
        Ok(_) => return api_internal("SporePump max-buy configuration is zero", slot),
        Err(error) => return api_internal(&error, slot),
    };
    ApiResponse::ok(
        LaunchpadConfigJson {
            creation_fee: CREATION_FEE_LICN,
            graduation_threshold: GRADUATION_MCAP_LICN,
            platform_fee_pct: PLATFORM_FEE_PCT,
            creator_royalty_bps,
            buy_cooldown_slots,
            sell_cooldown_slots,
            max_buy_raw,
            max_buy_raw_exact: max_buy_raw.to_string(),
            base_price_raw: BASE_PRICE,
            slope: SLOPE,
            slope_scale: SLOPE_SCALE,
        },
        slot,
    )
    .into_response()
}

/// GET /tokens — List all launched tokens
async fn get_tokens(
    State(state): State<Arc<RpcState>>,
    Query(q): Query<TokenListQuery>,
) -> Response {
    let slot = current_slot(&state);
    let token_count = match read_exact_u64_or_zero(&state, b"cp_token_count") {
        Ok(value) => value,
        Err(error) => return api_internal(&error, slot),
    };
    let filter = q.filter.as_deref().unwrap_or("all");
    let sort_by = q.sort.as_deref().unwrap_or("newest");
    let limit = q.limit.unwrap_or(50).min(200);
    let offset = q.offset.unwrap_or(0);
    if !matches!(filter, "active" | "graduated" | "all") {
        return api_err("filter must be active, graduated, or all");
    }
    if !matches!(
        sort_by,
        "newest" | "raised" | "graduation" | "price" | "mcap"
    ) {
        return api_err("sort must be newest, raised, graduation, price, or mcap");
    }
    let offset_u64 = match u64::try_from(offset) {
        Ok(value) => value,
        Err(_) => return api_err("offset is outside the supported u64 range"),
    };

    #[derive(Serialize)]
    struct TokenListResponse {
        tokens: Vec<TokenJson>,
        total: u64,
        total_exact: String,
        offset: usize,
        limit: usize,
        scan_cap: u64,
    }

    // The common newest/all path is O(page size), irrespective of launch count.
    // IDs are canonical and contiguous, so no global scan or sort is needed.
    if filter == "all" && sort_by == "newest" {
        let mut tokens = Vec::with_capacity(limit);
        if offset_u64 < token_count {
            let first_id = token_count - offset_u64;
            let take = u64::try_from(limit).unwrap_or(u64::MAX).min(first_id);
            for delta in 0..take {
                let id = first_id - delta;
                match decode_token(&state, id) {
                    Ok(Some(token)) => tokens.push(token),
                    Ok(None) => {
                        return api_internal(
                            &format!("SporePump token counter references missing token {id}"),
                            slot,
                        );
                    }
                    Err(error) => return api_internal(&error, slot),
                }
            }
        }
        return ApiResponse::ok(
            TokenListResponse {
                tokens,
                total: token_count,
                total_exact: token_count.to_string(),
                offset,
                limit,
                scan_cap: MAX_FILTERED_SORT_SCAN,
            },
            slot,
        )
        .into_response();
    }

    // Filtered and ranked views require inspecting every mutable token record.
    // Fail explicitly above a fixed cap instead of allowing one request to
    // monopolize the RPC process or returning a misleading partial ranking.
    if token_count > MAX_FILTERED_SORT_SCAN {
        return api_unprocessable(
            "filtered or ranked launchpad queries exceed the direct-state scan cap; use newest/all pagination (ranked views require a dedicated indexer above this cap)",
            slot,
        );
    }

    let mut tokens: Vec<TokenJson> = Vec::with_capacity(token_count as usize);
    for id in 1..=token_count {
        match decode_token(&state, id) {
            Ok(Some(t)) => {
                let include = match filter {
                    "active" => !t.graduated,
                    "graduated" => t.graduated,
                    _ => true,
                };
                if include {
                    tokens.push(t);
                }
            }
            Ok(None) => {
                return api_internal(
                    &format!("SporePump token counter references missing token {id}"),
                    slot,
                );
            }
            Err(error) => return api_internal(&error, slot),
        }
    }

    // Sort
    match sort_by {
        "raised" => tokens.sort_by_key(|token| std::cmp::Reverse(token.licn_raised_raw)),
        "graduation" => tokens.sort_by(|a, b| {
            b.graduation_pct
                .partial_cmp(&a.graduation_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "price" => tokens.sort_by_key(|token| std::cmp::Reverse(token.supply_sold_raw)),
        "mcap" => tokens.sort_by_key(|token| std::cmp::Reverse(token.supply_sold_raw)),
        _ => tokens.sort_by_key(|b| std::cmp::Reverse(b.id)), // newest first
    }

    // Paginate
    let total = tokens.len() as u64;
    let tokens: Vec<TokenJson> = tokens.into_iter().skip(offset).take(limit).collect();

    ApiResponse::ok(
        TokenListResponse {
            tokens,
            total,
            total_exact: total.to_string(),
            offset,
            limit,
            scan_cap: MAX_FILTERED_SORT_SCAN,
        },
        slot,
    )
    .into_response()
}

/// GET /tokens/:id — Get single token info
async fn get_token(State(state): State<Arc<RpcState>>, Path(id): Path<u64>) -> Response {
    let slot = current_slot(&state);
    match decode_token(&state, id) {
        Ok(Some(t)) => ApiResponse::ok(t, slot).into_response(),
        Ok(None) => api_404(&format!("Token {} not found", id)),
        Err(error) => api_internal(&error, slot),
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
        Some(d) if d.len() == 65 => d,
        Some(data) => {
            return api_internal(
                &format!(
                    "malformed SporePump key {key}: expected 65 bytes, got {}",
                    data.len()
                ),
                slot,
            );
        }
        None => return api_404(&format!("Token {} not found", id)),
    };
    if data[0..32].iter().all(|byte| *byte == 0) || u64_le(&data, 32) > u64_le(&data, 48) {
        return api_internal(
            "SporePump token row violates canonical supply or creator invariants",
            slot,
        );
    }

    match accounting_ready(&state) {
        Ok(true) => {}
        Ok(false) => {
            return api_unprocessable(
                "SporePump Accounting V3 is not active; buys are unavailable",
                slot,
            );
        }
        Err(error) => return api_internal(&error, slot),
    }
    match read_exact_bool(&state, b"cp_paused", false) {
        Ok(true) => return api_unprocessable("SporePump is paused; buys are unavailable", slot),
        Ok(false) => {}
        Err(error) => return api_internal(&error, slot),
    }
    match token_frozen(&state, id) {
        Ok(true) => return api_unprocessable("Token is frozen; trades are unavailable", slot),
        Ok(false) => {}
        Err(error) => return api_internal(&error, slot),
    }

    let graduation_state = match read_graduation_state(&state, id, data[64]) {
        Ok(value) => value,
        Err(error) => return api_internal(&error, slot),
    };
    if graduation_state != 0 {
        return api_err("Bonding-curve buys are closed for graduation");
    }

    let supply = u64_le(&data, 32);
    let licn_spores = match parse_quote_amount(q.amount_raw.as_deref(), q.amount, 1.0, "LICN") {
        Ok(value) => value,
        Err(error) => return api_err(&error),
    };
    let max_buy = match read_config_u64(&state, b"cp_max_buy", DEFAULT_MAX_BUY_AMOUNT) {
        Ok(value) if value > 0 => value,
        Ok(_) => return api_internal("SporePump max-buy configuration is zero", slot),
        Err(error) => return api_internal(&error, slot),
    };
    if licn_spores > max_buy {
        return api_err("amount exceeds the configured maximum buy");
    }
    let creator_royalty_bps =
        match read_config_u64(&state, b"cp_creator_royalty", DEFAULT_CREATOR_ROYALTY_BPS) {
            Ok(value) if value <= 1_000 => value,
            Ok(_) => return api_internal("SporePump creator royalty is out of range", slot),
            Err(error) => return api_internal(&error, slot),
        };

    // Binary search for tokens received (matching contract logic)
    let max_supply = u64_le(&data, 48);
    let max_available = max_supply.saturating_sub(supply);
    let quote = match compute_buy_quote(supply, licn_spores, max_available, creator_royalty_bps) {
        Ok(quote) => quote,
        Err(e) => return api_err(e),
    };
    let tokens_out = quote.tokens_out;
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
        creator_royalty_bps: u64,
        licn_input: f64,
        licn_input_raw: String,
        curve_cost_raw: String,
        platform_fee_raw: String,
        creator_royalty_raw: String,
        charged_raw: String,
        refund_raw: String,
        tokens_received_raw: String,
    }

    ApiResponse::ok(
        QuoteResponse {
            tokens_received: tokens_f,
            price_before: spot_price(supply),
            price_after,
            price_impact_pct: price_impact,
            platform_fee_pct: PLATFORM_FEE_PCT,
            creator_royalty_bps,
            licn_input: licn_spores as f64 / SPORES_PER_LICN,
            licn_input_raw: licn_spores.to_string(),
            curve_cost_raw: quote.curve_cost.to_string(),
            platform_fee_raw: quote.platform_fee.to_string(),
            creator_royalty_raw: quote.creator_fee.to_string(),
            charged_raw: quote.charged.to_string(),
            refund_raw: (licn_spores - quote.charged).to_string(),
            tokens_received_raw: tokens_out.to_string(),
        },
        slot,
    )
    .into_response()
}

#[derive(Deserialize)]
struct QuoteQuery {
    amount: Option<f64>, // LICN amount (human-readable, e.g. 100.0)
    amount_raw: Option<String>,
}

fn parse_quote_amount(
    amount_raw: Option<&str>,
    amount: Option<f64>,
    default_amount: f64,
    unit_name: &str,
) -> Result<u64, String> {
    if amount_raw.is_some() && amount.is_some() {
        return Err("provide amount_raw or amount, not both".to_string());
    }
    if let Some(raw) = amount_raw {
        return raw
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| "amount_raw must be a positive u64 decimal string".to_string());
    }
    let amount = amount.unwrap_or(default_amount);
    let raw = amount * SPORES_PER_LICN;
    if !amount.is_finite() || amount <= 0.0 || raw > u64::MAX as f64 {
        return Err(format!(
            "amount must be a positive finite {unit_name} value within u64 range"
        ));
    }
    let rounded = raw.round();
    if rounded < 1.0 {
        return Err("amount rounds below one raw unit".to_string());
    }
    Ok(rounded as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExactBuyQuote {
    tokens_out: u64,
    curve_cost: u64,
    platform_fee: u64,
    creator_fee: u64,
    charged: u64,
}

fn launchpad_buy_charge(curve_cost: u64, creator_bps: u64) -> Option<(u64, u64, u64)> {
    let total_bps = PLATFORM_FEE_BPS.checked_add(creator_bps)?;
    if total_bps >= BPS_SCALE {
        return None;
    }
    if total_bps == 0 {
        return Some((curve_cost, 0, 0));
    }
    let total_fee = (curve_cost as u128)
        .checked_mul(total_bps as u128)?
        .div_ceil((BPS_SCALE - total_bps) as u128);
    let total_fee = u64::try_from(total_fee).ok()?;
    let creator_fee =
        u64::try_from((total_fee as u128).checked_mul(creator_bps as u128)? / total_bps as u128)
            .ok()?;
    let platform_fee = total_fee.checked_sub(creator_fee)?;
    let charged = curve_cost.checked_add(total_fee)?;
    Some((charged, platform_fee, creator_fee))
}

/// Compute the exact contract quote for a gross LICN input.
///
/// Mirror the contract's fixed-point integral and bounded binary search exactly.
/// Keeping this deliberately mechanical prevents public quotes from drifting from
/// the amount the WASM contract credits.
fn compute_buy_quote(
    supply: u64,
    licn_input: u64,
    max_available: u64,
    creator_bps: u64,
) -> Result<ExactBuyQuote, &'static str> {
    if licn_input == 0 || max_available == 0 {
        return Err("Amount is too small or no curve supply remains");
    }

    let mut lo = 0u64;
    let mut hi = max_available;
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        let cost = launchpad_buy_cost(supply, mid);
        let affordable = launchpad_buy_charge(cost, creator_bps)
            .is_some_and(|(charged, _, _)| cost > 0 && charged <= licn_input);
        if affordable {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 {
        return Err("Amount is too small to buy any token units");
    }
    let curve_cost = launchpad_buy_cost(supply, lo);
    let Some((charged, platform_fee, creator_fee)) = launchpad_buy_charge(curve_cost, creator_bps)
    else {
        return Err("Fee arithmetic overflow");
    };
    if curve_cost == 0 || charged > licn_input {
        return Err("Quote arithmetic is inconsistent");
    }
    Ok(ExactBuyQuote {
        tokens_out: lo,
        curve_cost,
        platform_fee,
        creator_fee,
        charged,
    })
}

fn launchpad_buy_cost(supply: u64, amount: u64) -> u64 {
    let s = supply as u128;
    let a = amount as u128;
    let linear = (BASE_PRICE as u128).saturating_mul(a);
    let quadratic = (SLOPE as u128)
        .saturating_mul(a)
        .saturating_mul(s.saturating_mul(2).saturating_add(a))
        / (2 * SLOPE_SCALE as u128);
    let raw = linear.saturating_add(quadratic) / TOKEN_UNIT;
    raw.min(u64::MAX as u128) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExactSellQuote {
    tokens_in: u64,
    curve_refund: u64,
    platform_fee: u64,
    creator_fee: u64,
    net_refund: u64,
}

fn launchpad_sell_refund(supply: u64, amount: u64) -> u64 {
    if amount > supply {
        return 0;
    }
    let s = supply as u128;
    let a = amount as u128;
    let linear = (BASE_PRICE as u128).saturating_mul(a);
    let quadratic = (SLOPE as u128)
        .saturating_mul(a)
        .saturating_mul(s.saturating_mul(2).saturating_sub(a))
        / (2 * SLOPE_SCALE as u128);
    let raw = linear.saturating_add(quadratic) / TOKEN_UNIT;
    raw.min(u64::MAX as u128) as u64
}

fn compute_sell_quote(
    supply: u64,
    raised: u64,
    token_amount: u64,
    creator_bps: u64,
) -> Result<ExactSellQuote, &'static str> {
    if token_amount == 0 || token_amount > supply {
        return Err("token amount is zero or exceeds curve supply");
    }
    if creator_bps > 1_000 {
        return Err("creator royalty is out of range");
    }
    let curve_refund = launchpad_sell_refund(supply, token_amount);
    if curve_refund == 0 || curve_refund > raised {
        return Err("token amount is too small or curve reserve is inconsistent");
    }
    let platform_fee = u64::try_from(
        (curve_refund as u128)
            .checked_mul(PLATFORM_FEE_BPS as u128)
            .ok_or("platform fee overflow")?
            / BPS_SCALE as u128,
    )
    .map_err(|_| "platform fee overflow")?;
    let creator_fee = u64::try_from(
        (curve_refund as u128)
            .checked_mul(creator_bps as u128)
            .ok_or("creator fee overflow")?
            / BPS_SCALE as u128,
    )
    .map_err(|_| "creator fee overflow")?;
    let net_refund = curve_refund
        .checked_sub(platform_fee)
        .and_then(|value| value.checked_sub(creator_fee))
        .ok_or("sell fees exceed curve refund")?;
    Ok(ExactSellQuote {
        tokens_in: token_amount,
        curve_refund,
        platform_fee,
        creator_fee,
        net_refund,
    })
}

/// GET /tokens/:id/sell-quote — Get exact net LICN for a curve sale.
async fn get_sell_quote(
    State(state): State<Arc<RpcState>>,
    Path(id): Path<u64>,
    Query(q): Query<QuoteQuery>,
) -> Response {
    let slot = current_slot(&state);
    let key = format!("cpt:{:016x}", id);
    let data = match read_bytes(&state, key.as_bytes()) {
        Some(data) if data.len() == 65 => data,
        Some(data) => {
            return api_internal(
                &format!(
                    "malformed SporePump key {key}: expected 65 bytes, got {}",
                    data.len()
                ),
                slot,
            );
        }
        None => return api_404(&format!("Token {} not found", id)),
    };
    if data[0..32].iter().all(|byte| *byte == 0) || u64_le(&data, 32) > u64_le(&data, 48) {
        return api_internal(
            "SporePump token row violates canonical supply or creator invariants",
            slot,
        );
    }
    match accounting_ready(&state) {
        Ok(true) => {}
        Ok(false) => {
            return api_unprocessable(
                "SporePump Accounting V3 is not active; sells are unavailable",
                slot,
            );
        }
        Err(error) => return api_internal(&error, slot),
    }
    match token_frozen(&state, id) {
        Ok(true) => return api_unprocessable("Token is frozen; trades are unavailable", slot),
        Ok(false) => {}
        Err(error) => return api_internal(&error, slot),
    }
    let graduation_state = match read_graduation_state(&state, id, data[64]) {
        Ok(value) => value,
        Err(error) => return api_internal(&error, slot),
    };
    if graduation_state >= 2 {
        return api_err("Bonding-curve sells are closed during or after graduation");
    }
    let token_amount = match parse_quote_amount(q.amount_raw.as_deref(), q.amount, 1.0, "token") {
        Ok(value) => value,
        Err(error) => return api_err(&error),
    };
    let supply = u64_le(&data, 32);
    let raised = u64_le(&data, 40);
    let creator_royalty_bps =
        match read_config_u64(&state, b"cp_creator_royalty", DEFAULT_CREATOR_ROYALTY_BPS) {
            Ok(value) if value <= 1_000 => value,
            Ok(_) => return api_internal("SporePump creator royalty is out of range", slot),
            Err(error) => return api_internal(&error, slot),
        };
    let quote = match compute_sell_quote(supply, raised, token_amount, creator_royalty_bps) {
        Ok(value) => value,
        Err(error) => return api_err(error),
    };
    let price_before = spot_price(supply);
    let price_after = spot_price(supply - token_amount);
    let price_impact_pct = if price_before > 0.0 {
        (price_before - price_after) / price_before * 100.0
    } else {
        0.0
    };

    #[derive(Serialize)]
    struct SellQuoteResponse {
        tokens_input: f64,
        licn_received: f64,
        price_before: f64,
        price_after: f64,
        price_impact_pct: f64,
        platform_fee_pct: u64,
        creator_royalty_bps: u64,
        tokens_input_raw: String,
        curve_refund_raw: String,
        platform_fee_raw: String,
        creator_royalty_raw: String,
        licn_received_raw: String,
        minimum_licn_out_raw: String,
    }

    ApiResponse::ok(
        SellQuoteResponse {
            tokens_input: quote.tokens_in as f64 / SPORES_PER_LICN,
            licn_received: quote.net_refund as f64 / SPORES_PER_LICN,
            price_before,
            price_after,
            price_impact_pct,
            platform_fee_pct: PLATFORM_FEE_PCT,
            creator_royalty_bps,
            tokens_input_raw: quote.tokens_in.to_string(),
            curve_refund_raw: quote.curve_refund.to_string(),
            platform_fee_raw: quote.platform_fee.to_string(),
            creator_royalty_raw: quote.creator_fee.to_string(),
            licn_received_raw: quote.net_refund.to_string(),
            minimum_licn_out_raw: quote.net_refund.to_string(),
        },
        slot,
    )
    .into_response()
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
    let token_data = match read_bytes(&state, key.as_bytes()) {
        Some(data) if data.len() == 65 => data,
        Some(data) => {
            return api_internal(
                &format!(
                    "malformed SporePump key {key}: expected 65 bytes, got {}",
                    data.len()
                ),
                slot,
            );
        }
        None => return api_404(&format!("Token {} not found", id)),
    };
    if token_data[0..32].iter().all(|byte| *byte == 0)
        || u64_le(&token_data, 32) > u64_le(&token_data, 48)
    {
        return api_internal(
            "SporePump token row violates canonical supply or creator invariants",
            slot,
        );
    }

    let account_hex = match account_key_component(&addr) {
        Some(value) => value,
        None => return api_err("invalid address query parameter"),
    };
    let bal_key = format!("bal:{:016x}:{}", id, account_hex);
    let balance = match read_exact_u64_or_zero(&state, bal_key.as_bytes()) {
        Ok(value) => value,
        Err(error) => return api_internal(&error, slot),
    };

    #[derive(Serialize)]
    struct HolderBalance {
        token_id: u64,
        address: String,
        balance: f64,
        balance_raw: u64,
        balance_raw_exact: String,
        claimable_raw: u64,
        claimable_raw_exact: String,
        claimed: bool,
    }

    let claim_key = format!("cpgc:{:016x}:{}", id, account_hex);
    let claimed = match read_bytes(&state, claim_key.as_bytes()) {
        Some(value) if value == [1] => true,
        Some(value) => {
            return api_internal(
                &format!(
                    "malformed SporePump claim key {claim_key}: expected one committed byte, got {}",
                    value.len()
                ),
                slot,
            );
        }
        None => false,
    };
    let graduation_state = match read_graduation_state(&state, id, token_data[64]) {
        Ok(value) => value,
        Err(error) => return api_internal(&error, slot),
    };
    let claimable_raw = if graduation_state == 3 && !claimed {
        balance
    } else {
        0
    };

    ApiResponse::ok(
        HolderBalance {
            token_id: id,
            address: addr,
            balance: balance as f64 / SPORES_PER_LICN,
            balance_raw: balance,
            balance_raw_exact: balance.to_string(),
            claimable_raw,
            claimable_raw_exact: claimable_raw.to_string(),
            claimed,
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
        .route("/tokens/:id/sell-quote", get(get_sell_quote))
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

    #[test]
    fn graduation_state_names_cover_the_complete_lifecycle() {
        assert_eq!(graduation_state_name(0), "active");
        assert_eq!(graduation_state_name(1), "eligible");
        assert_eq!(graduation_state_name(2), "migrating");
        assert_eq!(graduation_state_name(3), "graduated");
        assert_eq!(graduation_state_name(u8::MAX), "active");
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

    // ── compute_buy_tokens ──

    #[test]
    fn buy_tokens_zero_input_returns_zero() {
        assert!(compute_buy_quote(0, 0, u64::MAX, DEFAULT_CREATOR_ROYALTY_BPS).is_err());
    }

    #[test]
    fn buy_tokens_positive_input() {
        // With some spores, we should get tokens
        let tokens = compute_buy_quote(0, 1_000_000_000, u64::MAX, 50)
            .unwrap()
            .tokens_out; // 1 LICN worth
        assert!(tokens > 0, "Should receive >0 tokens for 1 LICN");
    }

    #[test]
    fn buy_tokens_more_input_more_output() {
        let t1 = compute_buy_quote(0, 1_000_000_000, u64::MAX, 50)
            .unwrap()
            .tokens_out;
        let t2 = compute_buy_quote(0, 10_000_000_000, u64::MAX, 50)
            .unwrap()
            .tokens_out;
        assert!(t2 > t1, "More LICN in should yield more tokens");
    }

    #[test]
    fn buy_tokens_higher_supply_fewer_tokens() {
        // At higher supply, same input yields fewer tokens (bonding curve)
        let t_low = compute_buy_quote(0, 1_000_000_000, u64::MAX, 50)
            .unwrap()
            .tokens_out;
        let t_high = compute_buy_quote(100_000_000_000, 1_000_000_000, u64::MAX, 50)
            .unwrap()
            .tokens_out;
        assert!(
            t_low > t_high,
            "Higher supply should yield fewer tokens per LICN"
        );
    }

    #[test]
    fn buy_quote_is_maximal_under_contract_cost_function() {
        let input = 5_000_000_000u64;
        let quote = compute_buy_quote(0, input, 1_000_000_000_000_000_000, 50).unwrap();
        let tokens = quote.tokens_out;
        assert!(
            tokens > 1_000_000_000_000,
            "5 LICN must not be capped at 1,000 tokens"
        );
        assert!(quote.charged <= input);
        assert!(
            launchpad_buy_charge(launchpad_buy_cost(0, tokens + 1), 50)
                .unwrap()
                .0
                > input
        );
    }

    #[test]
    fn sell_integral_exactly_reverses_the_curve_cost() {
        for (initial_supply, amount) in [
            (0, 1_000_000_000),
            (9_000_000_000, 4_000_000_000),
            (1_000_000_000_000, 333_333_333_333),
        ] {
            let final_supply = initial_supply + amount;
            assert_eq!(
                launchpad_sell_refund(final_supply, amount),
                launchpad_buy_cost(initial_supply, amount)
            );
        }
    }

    #[test]
    fn sell_quote_funds_platform_and_creator_without_touching_curve_principal() {
        let supply = 1_000_000_000_000u64;
        let amount = supply / 3;
        let raised = launchpad_buy_cost(0, supply);
        let quote = compute_sell_quote(supply, raised, amount, 50).unwrap();
        assert_eq!(quote.curve_refund, launchpad_sell_refund(supply, amount));
        assert_eq!(
            quote.net_refund + quote.platform_fee + quote.creator_fee,
            quote.curve_refund
        );
        assert_eq!(quote.platform_fee, quote.curve_refund / 100);
        assert_eq!(quote.creator_fee, quote.curve_refund * 50 / BPS_SCALE);
    }

    #[test]
    fn sell_quote_rejects_supply_and_reserve_inconsistency() {
        assert!(compute_sell_quote(100, u64::MAX, 101, 50).is_err());
        assert!(compute_sell_quote(1_000_000_000, 0, 1_000_000_000, 50).is_err());
        assert!(compute_sell_quote(1_000_000_000, u64::MAX, 1_000_000_000, 1_001).is_err());
    }

    #[test]
    fn quote_amount_parser_prioritizes_exact_units_and_rejects_ambiguity() {
        assert_eq!(parse_quote_amount(Some("42"), None, 1.0, "token"), Ok(42));
        assert_eq!(
            parse_quote_amount(None, Some(1.25), 1.0, "LICN"),
            Ok(1_250_000_000)
        );
        assert!(parse_quote_amount(Some("42"), Some(1.0), 1.0, "LICN").is_err());
        assert!(parse_quote_amount(Some("0"), None, 1.0, "LICN").is_err());
        assert!(parse_quote_amount(None, Some(f64::NAN), 1.0, "LICN").is_err());
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

    #[test]
    fn account_key_component_accepts_base58_pubkey() {
        let pubkey = Pubkey([0x11; 32]);
        let encoded = pubkey.to_base58();
        assert_eq!(account_key_component(&encoded), Some("11".repeat(32)));
    }

    #[test]
    fn account_key_component_accepts_hex_and_normalizes_case() {
        assert_eq!(
            account_key_component(&"AB".repeat(32)),
            Some("ab".repeat(32))
        );
    }

    #[test]
    fn account_key_component_rejects_invalid_input() {
        assert_eq!(account_key_component("not a wallet address"), None);
    }
}
