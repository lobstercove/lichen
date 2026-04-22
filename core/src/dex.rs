use hex;

pub const PRICE_SCALE: u64 = 1_000_000_000;
pub const PNL_BIAS: u64 = 1u64 << 63;
pub const SLOT_DURATION_MS: u64 = 400;

pub const DEX_CORE_PROGRAM: &str = "DEX";
pub const DEX_AMM_PROGRAM: &str = "DEXAMM";
pub const DEX_MARGIN_PROGRAM: &str = "DEXMARGIN";
pub const DEX_ANALYTICS_PROGRAM: &str = "ANALYTICS";
pub const DEX_ROUTER_PROGRAM: &str = "DEXROUTER";
pub const DEX_REWARDS_PROGRAM: &str = "DEXREWARDS";
pub const DEX_GOVERNANCE_PROGRAM: &str = "DEXGOV";

pub const DEX_PAIR_COUNT_KEY: &str = "dex_pair_count";
pub const DEX_ORDER_COUNT_KEY: &str = "dex_order_count";
pub const DEX_TRADE_COUNT_KEY: &str = "dex_trade_count";
pub const AMM_POOL_COUNT_KEY: &str = "amm_pool_count";
pub const ROUTER_ROUTE_COUNT_KEY: &str = "rtr_route_count";
pub const MARGIN_POSITION_COUNT_KEY: &str = "mrg_pos_count";
pub const GOVERNANCE_PROPOSAL_COUNT_KEY: &str = "gov_prop_count";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexTradingPair {
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

#[derive(Debug, Clone, PartialEq)]
pub struct DexOrder {
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

#[derive(Debug, Clone, PartialEq)]
pub struct DexTrade {
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

#[derive(Debug, Clone, PartialEq)]
pub struct DexPool {
    pub pool_id: u64,
    pub token_a: String,
    pub token_b: String,
    pub sqrt_price: u64,
    pub price: f64,
    pub tick: i32,
    pub liquidity: u64,
    pub fee_tier: &'static str,
    pub protocol_fee: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexLpPosition {
    pub owner: String,
    pub pool_id: u64,
    pub lower_tick: i32,
    pub upper_tick: i32,
    pub liquidity: u64,
    pub fee_a_owed: u64,
    pub fee_b_owed: u64,
    pub created_slot: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DexMarginPosition {
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
    pub sl_price: u64,
    pub tp_price: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DexCandle {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: u64,
    pub slot: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DexStats24h {
    pub volume: u64,
    pub high: f64,
    pub low: f64,
    pub open: f64,
    pub close: f64,
    pub trade_count: u64,
    pub change: f64,
    pub change_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexRoute {
    pub route_id: u64,
    pub token_in: String,
    pub token_out: String,
    pub route_type: &'static str,
    pub pool_or_pair_id: u64,
    pub secondary_id: u64,
    pub split_percent: u8,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexProposal {
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

fn key_with_u64(prefix: &str, value: u64) -> String {
    format!("{prefix}{value}")
}

fn key_with_two_u64(prefix: &str, a: u64, b: u64) -> String {
    format!("{prefix}{a}_{b}")
}

fn key_with_three_u64(prefix: &str, a: u64, b: u64, c: u64) -> String {
    format!("{prefix}{a}_{b}_{c}")
}

fn key_with_string(prefix: &str, value: &str) -> String {
    format!("{prefix}{value}")
}

fn key_with_string_u64(prefix: &str, value: &str, idx: u64) -> String {
    format!("{prefix}{value}_{idx}")
}

pub fn pair_key(pair_id: u64) -> String {
    key_with_u64("dex_pair_", pair_id)
}

pub fn order_key(order_id: u64) -> String {
    key_with_u64("dex_order_", order_id)
}

pub fn trade_key(trade_id: u64) -> String {
    key_with_u64("dex_trade_", trade_id)
}

pub fn best_bid_key(pair_id: u64) -> String {
    key_with_u64("dex_best_bid_", pair_id)
}

pub fn best_ask_key(pair_id: u64) -> String {
    key_with_u64("dex_best_ask_", pair_id)
}

pub fn user_order_count_key(account_hex: &str) -> String {
    key_with_string("dex_uoc_", account_hex)
}

pub fn user_order_key(account_hex: &str, idx: u64) -> String {
    key_with_string_u64("dex_uo_", account_hex, idx)
}

pub fn analytics_last_price_key(pair_id: u64) -> String {
    key_with_u64("ana_lp_", pair_id)
}

pub fn analytics_last_trade_ts_key(pair_id: u64) -> String {
    key_with_u64("ana_last_trade_ts_", pair_id)
}

pub fn analytics_24h_key(pair_id: u64) -> String {
    key_with_u64("ana_24h_", pair_id)
}

pub fn analytics_candle_count_key(pair_id: u64, interval: u64) -> String {
    key_with_two_u64("ana_cc_", pair_id, interval)
}

pub fn analytics_candle_key(pair_id: u64, interval: u64, idx: u64) -> String {
    key_with_three_u64("ana_c_", pair_id, interval, idx)
}

pub fn analytics_leaderboard_key(rank: u64) -> String {
    key_with_u64("ana_lb_", rank)
}

pub fn analytics_trader_stats_key(account_hex: &str) -> String {
    key_with_string("ana_ts_", account_hex)
}

pub fn amm_pool_key(pool_id: u64) -> String {
    key_with_u64("amm_pool_", pool_id)
}

pub fn amm_owner_position_count_key(account_hex: &str) -> String {
    key_with_string("amm_opc_", account_hex)
}

pub fn amm_owner_position_key(account_hex: &str, idx: u64) -> String {
    key_with_string_u64("amm_op_", account_hex, idx)
}

pub fn amm_position_key(position_id: u64) -> String {
    key_with_u64("amm_pos_", position_id)
}

pub fn route_key(route_id: u64) -> String {
    key_with_u64("rtr_route_", route_id)
}

pub fn margin_user_position_count_key(account_hex: &str) -> String {
    key_with_string("mrg_upc_", account_hex)
}

pub fn margin_user_position_key(account_hex: &str, idx: u64) -> String {
    key_with_string_u64("mrg_up_", account_hex, idx)
}

pub fn margin_position_key(position_id: u64) -> String {
    key_with_u64("mrg_pos_", position_id)
}

pub fn margin_mark_key(pair_id: u64) -> String {
    key_with_u64("mrg_mark_", pair_id)
}

pub fn margin_enabled_key(pair_id: u64) -> String {
    key_with_u64("mrg_ena_", pair_id)
}

pub fn rewards_pending_key(account_hex: &str) -> String {
    key_with_string("rew_pend_", account_hex)
}

pub fn rewards_claimed_key(account_hex: &str) -> String {
    key_with_string("rew_claim_", account_hex)
}

pub fn rewards_volume_key(account_hex: &str) -> String {
    key_with_string("rew_vol_", account_hex)
}

pub fn rewards_referral_count_key(account_hex: &str) -> String {
    key_with_string("rew_refc_", account_hex)
}

pub fn rewards_referral_earnings_key(account_hex: &str) -> String {
    key_with_string("rew_refr_", account_hex)
}

pub fn governance_proposal_key(proposal_id: u64) -> String {
    key_with_u64("gov_prop_", proposal_id)
}

pub fn decode_pair(data: &[u8]) -> Option<DexTradingPair> {
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

    Some(DexTradingPair {
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

pub fn decode_order(data: &[u8]) -> Option<DexOrder> {
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

    Some(DexOrder {
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

pub fn decode_trade(data: &[u8]) -> Option<DexTrade> {
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

    Some(DexTrade {
        trade_id,
        pair_id,
        price: price_raw as f64 / PRICE_SCALE as f64,
        price_raw,
        quantity,
        taker,
        maker_order_id,
        slot,
        side: "buy",
        timestamp: 0,
    })
}

pub fn decode_pool(data: &[u8]) -> Option<DexPool> {
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
    let sqrt = sqrt_price as f64 / ((1u64 << 32) as f64);
    let price = sqrt * sqrt;

    Some(DexPool {
        pool_id,
        token_a,
        token_b,
        sqrt_price,
        price,
        tick,
        liquidity,
        fee_tier,
        protocol_fee,
    })
}

pub fn decode_lp_position(data: &[u8]) -> Option<DexLpPosition> {
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

    Some(DexLpPosition {
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

pub fn decode_margin_position(data: &[u8]) -> Option<DexMarginPosition> {
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
    let margin_type = if data.len() > 122 && data[122] == 1 {
        "cross"
    } else {
        "isolated"
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
    let sl_price = if data.len() >= 114 {
        u64::from_le_bytes(data[106..114].try_into().unwrap_or([0; 8]))
    } else {
        0
    };
    let tp_price = if data.len() >= 122 {
        u64::from_le_bytes(data[114..122].try_into().unwrap_or([0; 8]))
    } else {
        0
    };

    Some(DexMarginPosition {
        position_id,
        trader,
        pair_id,
        side,
        margin_type,
        status,
        size,
        margin,
        entry_price: entry_price_raw as f64 / PRICE_SCALE as f64,
        entry_price_raw,
        leverage,
        created_slot,
        realized_pnl,
        accumulated_funding,
        sl_price,
        tp_price,
    })
}

pub fn decode_candle(data: &[u8]) -> Option<DexCandle> {
    if data.len() < 48 {
        return None;
    }

    let open = u64::from_le_bytes(data[0..8].try_into().ok()?);
    let high = u64::from_le_bytes(data[8..16].try_into().ok()?);
    let low = u64::from_le_bytes(data[16..24].try_into().ok()?);
    let close = u64::from_le_bytes(data[24..32].try_into().ok()?);
    let volume = u64::from_le_bytes(data[32..40].try_into().ok()?);
    let slot = u64::from_le_bytes(data[40..48].try_into().ok()?);

    Some(DexCandle {
        open: open as f64 / PRICE_SCALE as f64,
        high: high as f64 / PRICE_SCALE as f64,
        low: low as f64 / PRICE_SCALE as f64,
        close: close as f64 / PRICE_SCALE as f64,
        volume,
        slot,
        timestamp: 0,
    })
}

pub fn decode_stats_24h(data: &[u8]) -> Option<DexStats24h> {
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

    Some(DexStats24h {
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

pub fn decode_route(data: &[u8]) -> Option<DexRoute> {
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
        _ => return None,
    };
    let pool_or_pair_id = u64::from_le_bytes(data[73..81].try_into().ok()?);
    let secondary_id = u64::from_le_bytes(data[81..89].try_into().ok()?);
    let split_percent = data[89];
    let enabled = data[90] == 1;

    Some(DexRoute {
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

pub fn decode_proposal(data: &[u8]) -> Option<DexProposal> {
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
    let base_token = if data[40] == 0 && data.len() >= 114 {
        Some(hex::encode(&data[82..114]))
    } else {
        None
    };
    let (new_maker_fee, new_taker_fee) = if data[40] == 1 && data.len() >= 118 {
        (
            Some(i16::from_le_bytes([data[114], data[115]])),
            Some(u16::from_le_bytes([data[116], data[117]])),
        )
    } else {
        (None, None)
    };

    Some(DexProposal {
        proposal_id,
        proposer,
        proposal_type,
        status,
        created_slot,
        end_slot,
        yes_votes,
        no_votes,
        pair_id,
        base_token,
        new_maker_fee,
        new_taker_fee,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pair_blob() -> Vec<u8> {
        let mut data = vec![0u8; 112];
        data[0..32].copy_from_slice(&[0x11; 32]);
        data[32..64].copy_from_slice(&[0x22; 32]);
        data[64..72].copy_from_slice(&7u64.to_le_bytes());
        data[72..80].copy_from_slice(&10u64.to_le_bytes());
        data[80..88].copy_from_slice(&100u64.to_le_bytes());
        data[88..96].copy_from_slice(&1_000u64.to_le_bytes());
        data[96] = 0;
        data[97..99].copy_from_slice(&(-2i16).to_le_bytes());
        data[99..101].copy_from_slice(&5u16.to_le_bytes());
        data[101..109].copy_from_slice(&123_456u64.to_le_bytes());
        data
    }

    #[test]
    fn key_builders_match_contract_layouts() {
        assert_eq!(pair_key(7), "dex_pair_7");
        assert_eq!(order_key(9), "dex_order_9");
        assert_eq!(trade_key(11), "dex_trade_11");
        assert_eq!(analytics_candle_count_key(3, 300), "ana_cc_3_300");
        assert_eq!(analytics_candle_key(3, 300, 4), "ana_c_3_300_4");
        assert_eq!(amm_owner_position_key("abcd", 2), "amm_op_abcd_2");
        assert_eq!(margin_user_position_key("abcd", 5), "mrg_up_abcd_5");
        assert_eq!(governance_proposal_key(12), "gov_prop_12");
    }

    #[test]
    fn decode_pair_roundtrip() {
        let pair = decode_pair(&make_pair_blob()).expect("pair should decode");

        assert_eq!(pair.pair_id, 7);
        assert_eq!(pair.base_token, "11".repeat(32));
        assert_eq!(pair.quote_token, "22".repeat(32));
        assert_eq!(pair.tick_size, 10);
        assert_eq!(pair.lot_size, 100);
        assert_eq!(pair.min_order, 1_000);
        assert_eq!(pair.status, "active");
        assert_eq!(pair.maker_fee_bps, -2);
        assert_eq!(pair.taker_fee_bps, 5);
        assert_eq!(pair.daily_volume, 123_456);
    }
}
