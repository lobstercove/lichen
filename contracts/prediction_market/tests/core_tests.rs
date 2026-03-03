// PredictionReef — Core Unit Tests (Phase A)
//
// Tests for: initialization, market creation, AMM math, binary/multi-outcome pricing,
// buy/sell operations, mint/redeem complete sets, market lifecycle state machine,
// LP operations, admin functions, query functions.
//
// All tests only use public extern "C" functions + test_mock for inspection.

use prediction_market::*;

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Reset everything and initialize with admin = [1u8; 32].
fn setup() -> [u8; 32] {
    moltchain_sdk::test_mock::reset();
    let admin = [1u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    moltchain_sdk::test_mock::set_slot(1000);
    assert_eq!(initialize(admin.as_ptr()), 0, "initialize failed");
    // Configure mUSD + self addresses so transfer_musd_out succeeds (fail-closed audit fix)
    moltchain_sdk::test_mock::set_caller(admin);
    set_musd_address(admin.as_ptr(), &[0xAAu8; 32] as *const u8);
    moltchain_sdk::test_mock::set_caller(admin);
    set_self_address(admin.as_ptr(), &[0xBBu8; 32] as *const u8);
    admin
}

/// Create a default binary market after init. Returns (admin, market_id).
fn setup_with_market() -> ([u8; 32], u64) {
    let admin = setup();
    let question = b"Will BTC reach $100K by March 2026?";
    let question_hash = [42u8; 32];
    let close_slot: u64 = 1000 + 100_000;

    moltchain_sdk::test_mock::set_caller(admin);
    moltchain_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    let market_id = create_market(
        admin.as_ptr(),
        2,  // CRYPTO category
        close_slot,
        2,  // binary (YES/NO)
        question_hash.as_ptr(),
        question.as_ptr(),
        question.len() as u32,
    );
    assert!(market_id > 0, "create_market should succeed, got {}", market_id);
    (admin, market_id as u64)
}

/// Create a binary market and add initial liquidity (10 mUSD, equal odds).
fn setup_active_market() -> ([u8; 32], u64) {
    let (admin, market_id) = setup_with_market();
    let amount: u64 = 10_000_000; // 10 mUSD

    moltchain_sdk::test_mock::set_caller(admin);
    moltchain_sdk::test_mock::set_value(amount);
    let result = add_initial_liquidity(
        admin.as_ptr(),
        market_id,
        amount,
        core::ptr::null(),
        0, // no custom odds → equal
    );
    assert_eq!(result, 1, "add_initial_liquidity should succeed");
    (admin, market_id)
}

/// Create a 4-outcome market with initial liquidity.
fn setup_multi_outcome_market() -> ([u8; 32], u64) {
    let admin = setup();
    let question = b"Who wins the 2026 World Cup?";
    let question_hash = [99u8; 32];
    let close_slot: u64 = 1000 + 200_000;

    moltchain_sdk::test_mock::set_caller(admin);
    moltchain_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    let market_id = create_market(
        admin.as_ptr(),
        1,  // SPORTS
        close_slot,
        4,  // 4 outcomes
        question_hash.as_ptr(),
        question.as_ptr(),
        question.len() as u32,
    );
    assert!(market_id > 0);
    let mid = market_id as u64;

    let amount: u64 = 40_000_000; // 40 mUSD
    moltchain_sdk::test_mock::set_caller(admin);
    moltchain_sdk::test_mock::set_value(amount);
    let result = add_initial_liquidity(admin.as_ptr(), mid, amount, core::ptr::null(), 0);
    assert_eq!(result, 1);
    (admin, mid)
}

/// Read u64 from return_data (first 8 bytes LE).
fn read_return_u64() -> u64 {
    let rd = moltchain_sdk::test_mock::get_return_data();
    assert!(rd.len() >= 8, "return_data too short: {} bytes", rd.len());
    u64::from_le_bytes(rd[0..8].try_into().unwrap())
}

/// Build a position storage key for direct test_mock::get_storage reads.
fn position_key_for_test(market_id: u64, addr: &[u8; 32], outcome: u8) -> Vec<u8> {
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut k = Vec::from(&b"pm_p_"[..]);
    let mut id_buf = itoa_test(market_id);
    k.append(&mut id_buf);
    k.push(b'_');
    for &b in addr {
        k.push(hex_chars[(b >> 4) as usize]);
        k.push(hex_chars[(b & 0x0f) as usize]);
    }
    k.push(b'_');
    let mut out_buf = itoa_test(outcome as u64);
    k.append(&mut out_buf);
    k
}

fn itoa_test(n: u64) -> Vec<u8> {
    if n == 0 { return vec![b'0']; }
    let mut buf = Vec::new();
    let mut v = n;
    while v > 0 { buf.push(b'0' + (v % 10) as u8); v /= 10; }
    buf.reverse();
    buf
}

/// Read position from storage directly.
fn read_position(market_id: u64, addr: &[u8; 32], outcome: u8) -> (u64, u64) {
    let key = position_key_for_test(market_id, addr, outcome);
    match moltchain_sdk::test_mock::get_storage(&key) {
        Some(data) if data.len() >= 16 => {
            let shares = u64::from_le_bytes(data[0..8].try_into().unwrap());
            let cost = u64::from_le_bytes(data[8..16].try_into().unwrap());
            (shares, cost)
        }
        _ => (0, 0),
    }
}

/// Read price for a given outcome (calls get_price, parses return_data).
fn read_price(market_id: u64, outcome: u8) -> u64 {
    let r = get_price(market_id, outcome);
    assert_eq!(r, 1, "get_price failed for market {} outcome {}", market_id, outcome);
    read_return_u64()
}

// ============================================================================
// INITIALIZATION TESTS
// ============================================================================

#[test]
fn test_initialize_basic() {
    moltchain_sdk::test_mock::reset();
    let admin = [1u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    let result = initialize(admin.as_ptr());
    assert_eq!(result, 0, "Initialize should return 0 (success)");
    assert_eq!(get_market_count(), 0);
}

#[test]
fn test_initialize_rejects_reinit() {
    let _admin = setup();
    let other = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(other);
    let result = initialize(other.as_ptr());
    assert_eq!(result, 1, "Re-init should be rejected");
}

#[test]
fn test_initialize_sets_counters_to_zero() {
    let _admin = setup();
    assert_eq!(get_market_count(), 0);
    let r = get_platform_stats();
    assert_eq!(r, 1);
    let rd = moltchain_sdk::test_mock::get_return_data();
    assert_eq!(rd.len(), 40);
    let mc = u64::from_le_bytes(rd[0..8].try_into().unwrap());
    assert_eq!(mc, 0);
}

// ============================================================================
// MARKET CREATION TESTS
// ============================================================================

#[test]
fn test_create_market_basic() {
    let (_admin, market_id) = setup_with_market();
    assert_eq!(market_id, 1);
    assert_eq!(get_market_count(), 1);
}

#[test]
fn test_create_market_multiple() {
    let admin = setup();
    for i in 0..5u8 {
        let mut qh = [0u8; 32];
        qh[0] = i + 10;
        let q = b"Test question";
        moltchain_sdk::test_mock::set_caller(admin);
        moltchain_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
        let mid = create_market(
            admin.as_ptr(), 0, 1000 + 100_000, 2,
            qh.as_ptr(), q.as_ptr(), q.len() as u32,
        );
        assert_eq!(mid, (i + 1) as u32, "Market {} should be created", i + 1);
    }
    assert_eq!(get_market_count(), 5);
}

#[test]
fn test_create_market_rejects_caller_mismatch() {
    let admin = setup();
    let faker = [99u8; 32];
    let qh = [42u8; 32];
    let q = b"Test";
    moltchain_sdk::test_mock::set_caller(admin);
    let r = create_market(faker.as_ptr(), 0, 1000 + 100_000, 2, qh.as_ptr(), q.as_ptr(), q.len() as u32);
    assert_eq!(r, 0, "Should reject caller/creator mismatch");
}

#[test]
fn test_create_market_rejects_outcome_count_1() {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Test";
    moltchain_sdk::test_mock::set_caller(admin);
    let r = create_market(admin.as_ptr(), 0, 1000 + 100_000, 1, qh.as_ptr(), q.as_ptr(), q.len() as u32);
    assert_eq!(r, 0, "outcome_count=1 must be rejected");
}

#[test]
fn test_create_market_rejects_outcome_count_9() {
    let admin = setup();
    let qh = [43u8; 32];
    let q = b"Test";
    moltchain_sdk::test_mock::set_caller(admin);
    let r = create_market(admin.as_ptr(), 0, 1000 + 100_000, 9, qh.as_ptr(), q.as_ptr(), q.len() as u32);
    assert_eq!(r, 0, "outcome_count=9 must be rejected");
}

#[test]
fn test_create_market_rejects_invalid_category() {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Test";
    moltchain_sdk::test_mock::set_caller(admin);
    let r = create_market(admin.as_ptr(), 8, 1000 + 100_000, 2, qh.as_ptr(), q.as_ptr(), q.len() as u32);
    assert_eq!(r, 0, "category=8 must be rejected");
}

#[test]
fn test_create_market_rejects_past_close_slot() {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Test";
    moltchain_sdk::test_mock::set_caller(admin);
    let r = create_market(admin.as_ptr(), 0, 500, 2, qh.as_ptr(), q.as_ptr(), q.len() as u32);
    assert_eq!(r, 0, "Past close_slot must be rejected");
}

#[test]
fn test_create_market_rejects_too_short_duration() {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Test";
    moltchain_sdk::test_mock::set_caller(admin);
    let r = create_market(admin.as_ptr(), 0, 1000 + 100, 2, qh.as_ptr(), q.as_ptr(), q.len() as u32);
    assert_eq!(r, 0, "Too-short duration must be rejected");
}

#[test]
fn test_create_market_rejects_too_long_duration() {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Test";
    moltchain_sdk::test_mock::set_caller(admin);
    let r = create_market(admin.as_ptr(), 0, 1000 + 70_000_000, 2, qh.as_ptr(), q.as_ptr(), q.len() as u32);
    assert_eq!(r, 0, "Too-long duration must be rejected");
}

#[test]
fn test_create_market_rejects_empty_question() {
    let admin = setup();
    let qh = [42u8; 32];
    let q: &[u8] = b"";
    moltchain_sdk::test_mock::set_caller(admin);
    let r = create_market(admin.as_ptr(), 0, 1000 + 100_000, 2, qh.as_ptr(), q.as_ptr(), 0);
    assert_eq!(r, 0, "Empty question must be rejected");
}

#[test]
fn test_create_market_rejects_duplicate_question_hash() {
    let (_admin, _) = setup_with_market();
    let admin = [1u8; 32];
    let qh = [42u8; 32]; // same hash as first market
    let q = b"Duplicate question";
    moltchain_sdk::test_mock::set_caller(admin);
    let r = create_market(admin.as_ptr(), 0, 1000 + 100_000, 2, qh.as_ptr(), q.as_ptr(), q.len() as u32);
    assert_eq!(r, 0, "Duplicate question hash must be rejected");
}

#[test]
fn test_create_market_all_valid_categories() {
    let admin = setup();
    for cat in 0..=7u8 {
        let mut qh = [0u8; 32];
        qh[0] = cat + 200;
        let q = b"Category test";
        moltchain_sdk::test_mock::set_caller(admin);
        moltchain_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
        let r = create_market(admin.as_ptr(), cat, 1000 + 100_000, 2, qh.as_ptr(), q.as_ptr(), q.len() as u32);
        assert!(r > 0, "Category {} should be accepted", cat);
    }
}

#[test]
fn test_create_multi_outcome_market() {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Multi outcome test";
    moltchain_sdk::test_mock::set_caller(admin);
    moltchain_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    let mid = create_market(admin.as_ptr(), 0, 1000 + 100_000, 5, qh.as_ptr(), q.as_ptr(), q.len() as u32);
    assert!(mid > 0);
    assert_eq!(get_market(mid as u64), 1);
    let rd = moltchain_sdk::test_mock::get_return_data();
    assert!(rd.len() >= 192);
    assert_eq!(rd[65], 5); // outcome_count at byte 65
}

// ============================================================================
// INITIAL LIQUIDITY TESTS
// ============================================================================

#[test]
fn test_add_initial_liquidity_basic() {
    let (admin, market_id) = setup_with_market();
    let amount: u64 = 5_000_000;
    moltchain_sdk::test_mock::set_caller(admin);
    let r = add_initial_liquidity(admin.as_ptr(), market_id, amount, core::ptr::null(), 0);
    assert_eq!(r, 1, "Should succeed");
    assert_eq!(get_market(market_id), 1);
    let rd = moltchain_sdk::test_mock::get_return_data();
    assert_eq!(rd[64], 1); // STATUS_ACTIVE
}

#[test]
fn test_initial_liquidity_equal_odds_prices() {
    let (_admin, market_id) = setup_active_market();
    let p0 = read_price(market_id, 0);
    let p1 = read_price(market_id, 1);
    assert_eq!(p0, 500_000, "YES price should be $0.50");
    assert_eq!(p1, 500_000, "NO price should be $0.50");
    assert_eq!(p0 + p1, 1_000_000, "Prices should sum to $1.00");
}

#[test]
fn test_initial_liquidity_custom_odds() {
    let (admin, market_id) = setup_with_market();
    let amount: u64 = 10_000_000;
    let mut odds = [0u8; 4];
    odds[0..2].copy_from_slice(&7000u16.to_le_bytes());
    odds[2..4].copy_from_slice(&3000u16.to_le_bytes());
    moltchain_sdk::test_mock::set_caller(admin);
    let r = add_initial_liquidity(admin.as_ptr(), market_id, amount, odds.as_ptr(), 4);
    assert_eq!(r, 1);
    let price_yes = read_price(market_id, 0);
    assert!(price_yes > 600_000 && price_yes < 800_000,
        "YES price should be ~$0.70, got {}", price_yes);
}

#[test]
fn test_initial_liquidity_rejects_non_creator() {
    let (_admin, market_id) = setup_with_market();
    let other = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(other);
    let r = add_initial_liquidity(other.as_ptr(), market_id, 5_000_000, core::ptr::null(), 0);
    assert_eq!(r, 0, "Non-creator must be rejected");
}

#[test]
fn test_initial_liquidity_rejects_below_minimum() {
    let (admin, market_id) = setup_with_market();
    moltchain_sdk::test_mock::set_caller(admin);
    let r = add_initial_liquidity(admin.as_ptr(), market_id, 500, core::ptr::null(), 0);
    assert_eq!(r, 0, "Below minimum must be rejected");
}

#[test]
fn test_initial_liquidity_rejects_double_activation() {
    let (admin, market_id) = setup_active_market();
    moltchain_sdk::test_mock::set_caller(admin);
    let r = add_initial_liquidity(admin.as_ptr(), market_id, 5_000_000, core::ptr::null(), 0);
    assert_eq!(r, 0, "Cannot add initial liquidity to already ACTIVE market");
}

#[test]
fn test_initial_liquidity_rejects_bad_odds_sum() {
    let (admin, market_id) = setup_with_market();
    let mut odds = [0u8; 4];
    odds[0..2].copy_from_slice(&6000u16.to_le_bytes());
    odds[2..4].copy_from_slice(&5000u16.to_le_bytes());
    moltchain_sdk::test_mock::set_caller(admin);
    let r = add_initial_liquidity(admin.as_ptr(), market_id, 10_000_000, odds.as_ptr(), 4);
    assert_eq!(r, 0, "Odds not summing to 10000 must be rejected");
}

// ============================================================================
// AMM MATH TESTS
// ============================================================================

#[test]
fn test_binary_cpmm_initial_prices_50_50() {
    let (_admin, market_id) = setup_active_market();
    let p0 = read_price(market_id, 0);
    assert_eq!(p0, 500_000, "Equal reserves → 50%");
}

#[test]
fn test_prices_always_sum_to_one_binary() {
    let (_admin, market_id) = setup_active_market();
    let p0 = read_price(market_id, 0);
    let p1 = read_price(market_id, 1);
    let sum = p0 + p1;
    assert!(sum >= 999_999 && sum <= 1_000_001,
        "Prices must sum to ~1.00, got {}", sum);
}

#[test]
fn test_buy_yes_increases_yes_price() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    let price_before = read_price(market_id, 0);
    moltchain_sdk::test_mock::set_caller(trader);
    let r = buy_shares(trader.as_ptr(), market_id, 0, 1_000_000);
    assert!(r > 0, "buy_shares failed");
    let price_after = read_price(market_id, 0);
    assert!(price_after > price_before,
        "Buying YES should increase YES price: {} → {}", price_before, price_after);
}

#[test]
fn test_buy_no_decreases_yes_price() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    let price_before = read_price(market_id, 0);
    moltchain_sdk::test_mock::set_caller(trader);
    let r = buy_shares(trader.as_ptr(), market_id, 1, 1_000_000);
    assert!(r > 0);
    let price_after = read_price(market_id, 0);
    assert!(price_after < price_before,
        "Buying NO should decrease YES price: {} → {}", price_before, price_after);
}

#[test]
fn test_prices_sum_after_trade() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    buy_shares(trader.as_ptr(), market_id, 0, 3_000_000);
    let p0 = read_price(market_id, 0);
    let p1 = read_price(market_id, 1);
    let sum = p0 + p1;
    assert!(sum >= 999_000 && sum <= 1_001_000,
        "Post-trade prices must sum to ~$1.00, got {} (p0={}, p1={})", sum, p0, p1);
}

#[test]
fn test_sell_reverses_buy_position() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    let shares_bought = buy_shares(trader.as_ptr(), market_id, 0, 2_000_000);
    assert!(shares_bought > 0);
    let (pos_shares, _) = read_position(market_id, &trader, 0);
    assert!(pos_shares > 0);
    moltchain_sdk::test_mock::set_caller(trader);
    let musd_back = sell_shares(trader.as_ptr(), market_id, 0, pos_shares);
    assert!(musd_back > 0);
    let (pos_after, _) = read_position(market_id, &trader, 0);
    assert_eq!(pos_after, 0, "All shares should be sold");
}

#[test]
fn test_mint_complete_set_no_price_impact() {
    let (_admin, market_id) = setup_active_market();
    let user = [2u8; 32];
    let price_before = read_price(market_id, 0);
    moltchain_sdk::test_mock::set_caller(user);
    let r = mint_complete_set(user.as_ptr(), market_id, 5_000_000);
    assert_eq!(r, 1);
    let price_after = read_price(market_id, 0);
    assert_eq!(price_before, price_after, "Minting complete set must not change price");
}

#[test]
fn test_redeem_complete_set_returns_collateral() {
    let (_admin, market_id) = setup_active_market();
    let user = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(user);
    assert_eq!(mint_complete_set(user.as_ptr(), market_id, 3_000_000), 1);
    for outcome in 0..2u8 {
        let (shares, _) = read_position(market_id, &user, outcome);
        assert_eq!(shares, 3_000_000, "Should have 3M shares of outcome {}", outcome);
    }
    moltchain_sdk::test_mock::set_caller(user);
    let returned = redeem_complete_set(user.as_ptr(), market_id, 3_000_000);
    assert_eq!(returned, 3_000_000, "Should return full collateral");
    for outcome in 0..2u8 {
        let (shares, _) = read_position(market_id, &user, outcome);
        assert_eq!(shares, 0);
    }
}

#[test]
fn test_quote_buy_matches_actual_buy() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    assert_eq!(quote_buy(market_id, 0, 2_000_000), 1);
    let quoted = read_return_u64();
    moltchain_sdk::test_mock::set_caller(trader);
    buy_shares(trader.as_ptr(), market_id, 0, 2_000_000);
    let (actual, _) = read_position(market_id, &trader, 0);
    assert_eq!(quoted, actual, "Quote and actual must match: q={} a={}", quoted, actual);
}

#[test]
fn test_zero_amount_buy_rejected() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(buy_shares(trader.as_ptr(), market_id, 0, 0), 0);
}

#[test]
fn test_sell_without_shares_rejected() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(sell_shares(trader.as_ptr(), market_id, 0, 1_000_000), 0);
}

#[test]
fn test_invalid_outcome_index_rejected() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(buy_shares(trader.as_ptr(), market_id, 2, 1_000_000), 0);
}

// ============================================================================
// MARKET LIFECYCLE TESTS
// ============================================================================

#[test]
fn test_market_status_active_after_liquidity() {
    let (_, market_id) = setup_active_market();
    assert_eq!(get_market(market_id), 1);
    let rd = moltchain_sdk::test_mock::get_return_data();
    assert_eq!(rd[64], 1); // STATUS_ACTIVE
}

#[test]
fn test_trading_rejected_after_close_slot() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(buy_shares(trader.as_ptr(), market_id, 0, 1_000_000), 0);
}

#[test]
fn test_emergency_pause_stops_trading() {
    let (admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(emergency_pause(admin.as_ptr()), 1);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(buy_shares(trader.as_ptr(), market_id, 0, 1_000_000), 0);
}

#[test]
fn test_emergency_unpause_resumes_trading() {
    let (admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    emergency_pause(admin.as_ptr());
    moltchain_sdk::test_mock::set_caller(admin);
    emergency_unpause(admin.as_ptr());
    moltchain_sdk::test_mock::set_caller(trader);
    assert!(buy_shares(trader.as_ptr(), market_id, 0, 1_000_000) > 0);
}

#[test]
fn test_only_admin_can_pause() {
    let _admin = setup();
    let non_admin = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(non_admin);
    assert_eq!(emergency_pause(non_admin.as_ptr()), 0);
}

#[test]
fn test_only_admin_can_unpause() {
    let admin = setup();
    moltchain_sdk::test_mock::set_caller(admin);
    emergency_pause(admin.as_ptr());
    let non_admin = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(non_admin);
    assert_eq!(emergency_unpause(non_admin.as_ptr()), 0);
}

// ============================================================================
// LIQUIDITY PROVIDER TESTS
// ============================================================================

#[test]
fn test_add_liquidity_to_active_market() {
    let (_admin, market_id) = setup_active_market();
    let lp = [3u8; 32];
    moltchain_sdk::test_mock::set_caller(lp);
    let r = add_liquidity(lp.as_ptr(), market_id, 5_000_000);
    assert!(r > 0, "add_liquidity should return LP shares");
    assert_eq!(get_lp_balance(market_id, lp.as_ptr()), 1);
    let lp_bal = read_return_u64();
    assert!(lp_bal > 0);
}

#[test]
fn test_add_liquidity_rejects_non_active() {
    let (_admin, market_id) = setup_with_market();
    let lp = [3u8; 32];
    moltchain_sdk::test_mock::set_caller(lp);
    assert_eq!(add_liquidity(lp.as_ptr(), market_id, 5_000_000), 0);
}

#[test]
fn test_withdraw_liquidity_basic() {
    let (admin, market_id) = setup_active_market();
    assert_eq!(get_lp_balance(market_id, admin.as_ptr()), 1);
    let lp_bal = read_return_u64();
    assert!(lp_bal > 0);
    let half = lp_bal / 2;
    moltchain_sdk::test_mock::set_caller(admin);
    let r = withdraw_liquidity(admin.as_ptr(), market_id, half);
    assert!(r > 0);
    assert_eq!(get_lp_balance(market_id, admin.as_ptr()), 1);
    let new_bal = read_return_u64();
    assert_eq!(new_bal, lp_bal - half);
}

#[test]
fn test_withdraw_liquidity_rejects_excess() {
    let (admin, market_id) = setup_active_market();
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(withdraw_liquidity(admin.as_ptr(), market_id, u64::MAX), 0);
}

// ============================================================================
// RESOLUTION TESTS
// ============================================================================

#[test]
fn test_submit_resolution_rejects_active_market() {
    let (_admin, market_id) = setup_active_market();
    let resolver = [5u8; 32];
    let att_hash = [77u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    assert_eq!(submit_resolution(resolver.as_ptr(), market_id, 0, att_hash.as_ptr(), 100_000_000), 0);
}

#[test]
fn test_dao_resolve_rejects_non_disputed() {
    let (_admin, market_id) = setup_active_market();
    let admin = [1u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(dao_resolve(admin.as_ptr(), market_id, 0), 0);
}

#[test]
fn test_dao_resolve_rejects_non_admin() {
    let (_admin, market_id) = setup_active_market();
    let non_admin = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(non_admin);
    assert_eq!(dao_resolve(non_admin.as_ptr(), market_id, 0), 0);
}

// ============================================================================
// VOID AND RECLAIM TESTS
// ============================================================================

#[test]
fn test_dao_void_active_market() {
    let (admin, market_id) = setup_active_market();
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(dao_void(admin.as_ptr(), market_id), 1);
    assert_eq!(get_market(market_id), 1);
    let rd = moltchain_sdk::test_mock::get_return_data();
    assert_eq!(rd[64], 6); // STATUS_VOIDED
}

#[test]
fn test_voided_market_refund() {
    let (admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    buy_shares(trader.as_ptr(), market_id, 0, 2_000_000);
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(dao_void(admin.as_ptr(), market_id), 1);
    moltchain_sdk::test_mock::set_caller(trader);
    let refund = reclaim_collateral(trader.as_ptr(), market_id);
    assert_eq!(refund, 1, "Should get refund from voided market");
}

#[test]
fn test_cannot_void_pending_market() {
    let (admin, market_id) = setup_with_market();
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(dao_void(admin.as_ptr(), market_id), 0);
}

#[test]
fn test_cannot_void_non_admin() {
    let (_admin, market_id) = setup_active_market();
    let non_admin = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(non_admin);
    assert_eq!(dao_void(non_admin.as_ptr(), market_id), 0);
}

// ============================================================================
// ADMIN / CONFIG TESTS
// ============================================================================

#[test]
fn test_set_moltyid_address() {
    let admin = setup();
    let addr = [42u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(set_moltyid_address(admin.as_ptr(), addr.as_ptr()), 1);
}

#[test]
fn test_set_oracle_address() {
    let admin = setup();
    let addr = [43u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(set_oracle_address(admin.as_ptr(), addr.as_ptr()), 1);
}

#[test]
fn test_set_addresses_admin_only() {
    let _admin = setup();
    let non_admin = [99u8; 32];
    let addr = [50u8; 32];
    moltchain_sdk::test_mock::set_caller(non_admin);
    assert_eq!(set_moltyid_address(non_admin.as_ptr(), addr.as_ptr()), 0);
    assert_eq!(set_oracle_address(non_admin.as_ptr(), addr.as_ptr()), 0);
    assert_eq!(set_musd_address(non_admin.as_ptr(), addr.as_ptr()), 0);
    assert_eq!(set_dex_gov_address(non_admin.as_ptr(), addr.as_ptr()), 0);
}

// ============================================================================
// QUERY TESTS
// ============================================================================

#[test]
fn test_get_market_nonexistent() {
    setup();
    assert_eq!(get_market(999), 0);
}

#[test]
fn test_get_outcome_pool_nonexistent() {
    setup();
    assert_eq!(get_outcome_pool(999, 0), 0);
}

#[test]
fn test_get_pool_reserves_binary() {
    let (_, market_id) = setup_active_market();
    assert_eq!(get_pool_reserves(market_id), 1);
    let rd = moltchain_sdk::test_mock::get_return_data();
    assert_eq!(rd.len(), 16);
    let r0 = u64::from_le_bytes(rd[0..8].try_into().unwrap());
    let r1 = u64::from_le_bytes(rd[8..16].try_into().unwrap());
    assert!(r0 > 0 && r1 > 0);
    assert_eq!(r0, r1, "Equal odds → equal reserves");
}

#[test]
fn test_get_platform_stats_initial() {
    let _admin = setup();
    assert_eq!(get_platform_stats(), 1);
    let rd = moltchain_sdk::test_mock::get_return_data();
    assert_eq!(rd.len(), 40);
    let mc = u64::from_le_bytes(rd[0..8].try_into().unwrap());
    assert_eq!(mc, 0);
}

#[test]
fn test_get_user_markets_tracking() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    assert_eq!(get_user_markets(trader.as_ptr()), 0);
    moltchain_sdk::test_mock::set_caller(trader);
    buy_shares(trader.as_ptr(), market_id, 0, 1_000_000);
    assert_eq!(get_user_markets(trader.as_ptr()), 1);
    moltchain_sdk::test_mock::set_caller(trader);
    buy_shares(trader.as_ptr(), market_id, 0, 1_000_000);
    assert_eq!(get_user_markets(trader.as_ptr()), 1, "No duplicate tracking");
}

#[test]
fn test_fee_treasury_accumulates() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    assert_eq!(get_fee_treasury(), 0);
    moltchain_sdk::test_mock::set_caller(trader);
    buy_shares(trader.as_ptr(), market_id, 0, 5_000_000);
    assert!(get_fee_treasury() > 0, "Fees should accumulate");
}

// ============================================================================
// MULTIPLE TRADERS
// ============================================================================

#[test]
fn test_multiple_traders_same_market() {
    let (_admin, market_id) = setup_active_market();
    let t1 = [2u8; 32];
    let t2 = [3u8; 32];
    let t3 = [4u8; 32];

    moltchain_sdk::test_mock::set_caller(t1);
    assert!(buy_shares(t1.as_ptr(), market_id, 0, 2_000_000) > 0);
    moltchain_sdk::test_mock::set_caller(t2);
    assert!(buy_shares(t2.as_ptr(), market_id, 1, 1_000_000) > 0);
    moltchain_sdk::test_mock::set_caller(t3);
    assert!(buy_shares(t3.as_ptr(), market_id, 0, 3_000_000) > 0);

    let p0 = read_price(market_id, 0);
    let p1 = read_price(market_id, 1);
    let sum = p0 + p1;
    assert!(sum >= 999_000 && sum <= 1_001_000,
        "Prices must sum to ~$1.00: {} (p0={}, p1={})", sum, p0, p1);

    let (s1, _) = read_position(market_id, &t1, 0);
    let (s2, _) = read_position(market_id, &t2, 1);
    let (s3, _) = read_position(market_id, &t3, 0);
    assert!(s1 > 0 && s2 > 0 && s3 > 0);
}

// ============================================================================
// COMPLETE-SET EDGE CASES
// ============================================================================

#[test]
fn test_redeem_complete_set_rejects_insufficient() {
    let (_admin, market_id) = setup_active_market();
    let user = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(user);
    mint_complete_set(user.as_ptr(), market_id, 2_000_000);
    moltchain_sdk::test_mock::set_caller(user);
    assert_eq!(redeem_complete_set(user.as_ptr(), market_id, 3_000_000), 0);
}

#[test]
fn test_mint_zero_amount_rejected() {
    let (_admin, market_id) = setup_active_market();
    let user = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(user);
    assert_eq!(mint_complete_set(user.as_ptr(), market_id, 0), 0);
}

// ============================================================================
// OVERFLOW / CIRCUIT BREAKER TESTS
// ============================================================================

#[test]
fn test_large_buy_hits_circuit_breaker() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(buy_shares(trader.as_ptr(), market_id, 0, 60_000_000_000), 0);
}

#[test]
fn test_sell_more_than_owned_rejected() {
    let (_admin, market_id) = setup_active_market();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    buy_shares(trader.as_ptr(), market_id, 0, 1_000_000);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(sell_shares(trader.as_ptr(), market_id, 0, u64::MAX), 0);
}

// ============================================================================
// MULTI-OUTCOME TESTS
// ============================================================================

#[test]
fn test_multi_outcome_prices_sum_to_one() {
    let (_admin, market_id) = setup_multi_outcome_market();
    let mut total: u64 = 0;
    for i in 0..4u8 { total += read_price(market_id, i); }
    assert!(total >= 999_000 && total <= 1_001_000,
        "4-outcome prices must sum to ~$1.00, got {}", total);
}

#[test]
fn test_multi_outcome_initial_equal() {
    let (_admin, market_id) = setup_multi_outcome_market();
    let p0 = read_price(market_id, 0);
    assert!(p0 > 200_000 && p0 < 300_000,
        "Each of 4 outcomes should be ~$0.25, got {}", p0);
}

#[test]
fn test_multi_outcome_buy_increases_price() {
    let (_admin, market_id) = setup_multi_outcome_market();
    let trader = [2u8; 32];
    let p_before = read_price(market_id, 0);
    moltchain_sdk::test_mock::set_caller(trader);
    assert!(buy_shares(trader.as_ptr(), market_id, 0, 2_000_000) > 0);
    let p_after = read_price(market_id, 0);
    assert!(p_after > p_before);
}

#[test]
fn test_multi_outcome_prices_sum_after_trade() {
    let (_admin, market_id) = setup_multi_outcome_market();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    buy_shares(trader.as_ptr(), market_id, 2, 5_000_000);
    let mut total: u64 = 0;
    for i in 0..4u8 { total += read_price(market_id, i); }
    assert!(total >= 998_000 && total <= 1_002_000,
        "Multi-outcome prices must sum to ~$1.00 after trade, got {}", total);
}

// ============================================================================
// MAX OUTCOMES BOUNDARY
// ============================================================================

#[test]
fn test_max_outcomes_8_accepted() {
    let admin = setup();
    let qh = [88u8; 32];
    let q = b"8 outcome market";
    moltchain_sdk::test_mock::set_caller(admin);
    moltchain_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    assert!(create_market(admin.as_ptr(), 0, 1000 + 100_000, 8, qh.as_ptr(), q.as_ptr(), q.len() as u32) > 0);
}

#[test]
fn test_outcome_9_rejected() {
    let admin = setup();
    let qh = [89u8; 32];
    let q = b"9 outcome market";
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(create_market(admin.as_ptr(), 0, 1000 + 100_000, 9, qh.as_ptr(), q.as_ptr(), q.len() as u32), 0);
}

// ============================================================================
// INTEGRATION: FULL LIFECYCLE TO VOID
// ============================================================================

#[test]
fn test_full_binary_lifecycle_to_void() {
    let (admin, market_id) = setup_active_market();
    let t1 = [2u8; 32];
    let t2 = [3u8; 32];

    moltchain_sdk::test_mock::set_caller(t1);
    assert!(buy_shares(t1.as_ptr(), market_id, 0, 3_000_000) > 0);
    moltchain_sdk::test_mock::set_caller(t2);
    assert!(buy_shares(t2.as_ptr(), market_id, 1, 2_000_000) > 0);

    let (s1, _) = read_position(market_id, &t1, 0);
    let (s2, _) = read_position(market_id, &t2, 1);
    assert!(s1 > 0 && s2 > 0);

    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(dao_void(admin.as_ptr(), market_id), 1);

    moltchain_sdk::test_mock::set_caller(t1);
    assert_eq!(reclaim_collateral(t1.as_ptr(), market_id), 1);
    moltchain_sdk::test_mock::set_caller(t2);
    assert_eq!(reclaim_collateral(t2.as_ptr(), market_id), 1);
}

#[test]
fn test_mint_then_sell_one_side() {
    let (_admin, market_id) = setup_active_market();
    let user = [2u8; 32];

    moltchain_sdk::test_mock::set_caller(user);
    mint_complete_set(user.as_ptr(), market_id, 5_000_000);

    let (yes_shares, _) = read_position(market_id, &user, 0);
    moltchain_sdk::test_mock::set_caller(user);
    let musd_back = sell_shares(user.as_ptr(), market_id, 0, yes_shares);
    assert!(musd_back > 0);

    let (no_shares, _) = read_position(market_id, &user, 1);
    assert_eq!(no_shares, 5_000_000);
    let (yes_after, _) = read_position(market_id, &user, 0);
    assert_eq!(yes_after, 0);
}

#[test]
fn test_multiple_lps_and_withdrawal() {
    let (admin, market_id) = setup_active_market();
    let lp2 = [3u8; 32];

    moltchain_sdk::test_mock::set_caller(lp2);
    let lp_shares = add_liquidity(lp2.as_ptr(), market_id, 5_000_000);
    assert!(lp_shares > 0);

    assert_eq!(get_lp_balance(market_id, admin.as_ptr()), 1);
    let admin_lp = read_return_u64();
    assert_eq!(get_lp_balance(market_id, lp2.as_ptr()), 1);
    let lp2_bal = read_return_u64();
    assert!(admin_lp > 0 && lp2_bal > 0);

    let half = lp2_bal / 2;
    moltchain_sdk::test_mock::set_caller(lp2);
    assert!(withdraw_liquidity(lp2.as_ptr(), market_id, half) > 0);

    assert_eq!(get_lp_balance(market_id, lp2.as_ptr()), 1);
    let lp2_after = read_return_u64();
    assert_eq!(lp2_after, lp2_bal - half);
}
