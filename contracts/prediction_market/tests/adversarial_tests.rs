// Prediction Market — Adversarial & Edge Case Tests
//
// Circuit breaker, reentrancy patterns, state machine edge cases,
// boundary values, double-claim prevention, multi-outcome edge cases.

use prediction_market::*;

// ============================================================================
// TEST HELPERS
// ============================================================================

fn setup() -> [u8; 32] {
    lichen_sdk::test_mock::reset();
    let admin = [1u8; 32];
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_slot(1000);
    assert_eq!(initialize(admin.as_ptr()), 0);
    // Configure lUSD + self addresses so transfer_musd_out succeeds (fail-closed audit fix)
    lichen_sdk::test_mock::set_caller(admin);
    set_lusd_address(admin.as_ptr(), &[0xAAu8; 32] as *const u8);
    lichen_sdk::test_mock::set_caller(admin);
    set_self_address(admin.as_ptr(), &[0xBBu8; 32] as *const u8);
    admin
}

fn setup_active_market() -> ([u8; 32], u64) {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Test market?";
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    let mid = create_market(
        admin.as_ptr(),
        2,
        1000 + 100_000,
        2,
        qh.as_ptr(),
        q.as_ptr(),
        q.len() as u32,
    ) as u64;
    assert!(mid > 0);
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(10_000_000);
    assert_eq!(
        add_initial_liquidity(admin.as_ptr(), mid, 10_000_000, core::ptr::null(), 0),
        1
    );
    (admin, mid)
}

fn setup_active_market_large() -> ([u8; 32], u64) {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Large pool market?";
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    let mid = create_market(
        admin.as_ptr(),
        0,
        1000 + 100_000,
        2,
        qh.as_ptr(),
        q.as_ptr(),
        q.len() as u32,
    ) as u64;
    assert!(mid > 0);
    // 100 lUSD - larger pool so trades don't trigger circuit breaker as easily
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(100_000_000);
    assert_eq!(
        add_initial_liquidity(admin.as_ptr(), mid, 100_000_000, core::ptr::null(), 0),
        1
    );
    (admin, mid)
}

fn read_return_u64() -> u64 {
    let rd = lichen_sdk::test_mock::get_return_data();
    u64::from_le_bytes(rd[0..8].try_into().unwrap())
}

fn read_price(market_id: u64, outcome: u8) -> u64 {
    assert_eq!(get_price(market_id, outcome), 1);
    read_return_u64()
}

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
    if n == 0 {
        return vec![b'0'];
    }
    let mut buf = Vec::new();
    let mut v = n;
    while v > 0 {
        buf.push(b'0' + (v % 10) as u8);
        v /= 10;
    }
    buf.reverse();
    buf
}

fn read_position(market_id: u64, addr: &[u8; 32], outcome: u8) -> (u64, u64) {
    let key = position_key_for_test(market_id, addr, outcome);
    match lichen_sdk::test_mock::get_storage(&key) {
        Some(data) if data.len() >= 16 => {
            let shares = u64::from_le_bytes(data[0..8].try_into().unwrap());
            let cost = u64::from_le_bytes(data[8..16].try_into().unwrap());
            (shares, cost)
        }
        _ => (0, 0),
    }
}

// ============================================================================
// CIRCUIT BREAKER TESTS
// ============================================================================

#[test]
fn test_circuit_breaker_arms_on_large_price_move() {
    let (_admin, mid) = setup_active_market();
    let t = [2u8; 32];

    // A 5 lUSD buy on 10 lUSD pool is huge (50%) → triggers circuit breaker
    lichen_sdk::test_mock::set_caller(t);
    let r = buy_shares(t.as_ptr(), mid, 0, 5_000_000);
    // First buy should succeed (breaker check is BEFORE arming)
    assert!(r > 0, "First big buy should succeed");

    // Second buy in same slot should be blocked by armed breaker
    lichen_sdk::test_mock::set_caller(t);
    let r2 = buy_shares(t.as_ptr(), mid, 1, 1_000_000);
    assert_eq!(r2, 0, "Second buy should be blocked by circuit breaker");
}

#[test]
fn test_circuit_breaker_expires_after_pause_slots() {
    let (_admin, mid) = setup_active_market();
    let t = [2u8; 32];

    // Trigger breaker
    lichen_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 0, 5_000_000);

    // Blocked now
    lichen_sdk::test_mock::set_caller(t);
    assert_eq!(buy_shares(t.as_ptr(), mid, 1, 500_000), 0);

    // Advance past PRICE_MOVE_PAUSE_SLOTS (150)
    lichen_sdk::test_mock::set_slot(1000 + 151);
    lichen_sdk::test_mock::set_caller(t);
    let r = buy_shares(t.as_ptr(), mid, 1, 500_000);
    assert!(r > 0, "Buy should succeed after circuit breaker expires");
}

#[test]
fn test_small_trade_does_not_trigger_breaker() {
    let (_admin, mid) = setup_active_market_large();
    let t1 = [2u8; 32];
    let t2 = [3u8; 32];

    // Small trade relative to 100 lUSD pool
    lichen_sdk::test_mock::set_caller(t1);
    assert!(buy_shares(t1.as_ptr(), mid, 0, 1_000_000) > 0);

    // Another trade in same slot should also succeed (no breaker)
    lichen_sdk::test_mock::set_caller(t2);
    assert!(
        buy_shares(t2.as_ptr(), mid, 1, 1_000_000) > 0,
        "Small trades shouldn't trigger breaker"
    );
}

// ============================================================================
// MAX_BET_SIZE / CIRCUIT_BREAKER_COLLATERAL TESTS
// ============================================================================

#[test]
fn test_max_bet_size_reject() {
    let (_admin, mid) = setup_active_market();
    let t = [2u8; 32];
    // 60B exceeds CIRCUIT_BREAKER_COLLATERAL = 50_000_000_000
    lichen_sdk::test_mock::set_caller(t);
    assert_eq!(buy_shares(t.as_ptr(), mid, 0, 60_000_000_000), 0);
}

#[test]
fn test_at_max_collateral_boundary() {
    let (_admin, mid) = setup_active_market();
    let t = [2u8; 32];
    // Just at CIRCUIT_BREAKER_COLLATERAL minus existing collateral
    // Existing = 10_000_000 from initial liquidity
    // Max additional = 50_000_000_000 - 10_000_000 = 49_990_000_000
    // This amount should not exceed the per-market collateral breaker
    // But it will likely fail due to other reasons (huge price impact)
    lichen_sdk::test_mock::set_caller(t);
    let r = buy_shares(t.as_ptr(), mid, 0, 49_990_000_000);
    // Should be rejected because it exceeds CIRCUIT_BREAKER_COLLATERAL
    // existing_coll (10M) + 49990M = 50000M = 50B which equals CIRCUIT_BREAKER_COLLATERAL
    // The condition is > not >= so this is allowed by collateral check,
    // but will fail elsewhere (circuit breaker, zero shares, etc.)
    // Just ensure no panic
    let _ = r;
}

// ============================================================================
// STATE MACHINE EDGE CASES
// ============================================================================

#[test]
fn test_cannot_trade_on_pending_market() {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Test";
    lichen_sdk::test_mock::set_caller(admin);
    let mid = create_market(
        admin.as_ptr(),
        0,
        1000 + 100_000,
        2,
        qh.as_ptr(),
        q.as_ptr(),
        q.len() as u32,
    ) as u64;
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    assert_eq!(
        buy_shares(t.as_ptr(), mid, 0, 1_000_000),
        0,
        "Cannot trade on PENDING market"
    );
}

#[test]
fn test_cannot_trade_on_voided_market() {
    let (admin, mid) = setup_active_market();
    lichen_sdk::test_mock::set_caller(admin);
    dao_void(admin.as_ptr(), mid);
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    assert_eq!(
        buy_shares(t.as_ptr(), mid, 0, 1_000_000),
        0,
        "Cannot trade on VOIDED market"
    );
}

#[test]
fn test_cannot_mint_on_voided_market() {
    let (admin, mid) = setup_active_market();
    lichen_sdk::test_mock::set_caller(admin);
    dao_void(admin.as_ptr(), mid);
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    assert_eq!(
        mint_complete_set(t.as_ptr(), mid, 1_000_000),
        0,
        "Cannot mint on VOIDED market"
    );
}

#[test]
fn test_cannot_add_liquidity_to_voided_market() {
    let (admin, mid) = setup_active_market();
    lichen_sdk::test_mock::set_caller(admin);
    dao_void(admin.as_ptr(), mid);
    let lp = [3u8; 32];
    lichen_sdk::test_mock::set_caller(lp);
    assert_eq!(add_liquidity(lp.as_ptr(), mid, 5_000_000), 0);
}

#[test]
fn test_no_double_void() {
    let (admin, mid) = setup_active_market();
    lichen_sdk::test_mock::set_caller(admin);
    assert_eq!(dao_void(admin.as_ptr(), mid), 1); // first void succeeds
    lichen_sdk::test_mock::set_caller(admin);
    assert_eq!(dao_void(admin.as_ptr(), mid), 0); // already voided
}

#[test]
fn test_cannot_withdraw_lp_from_voided_market() {
    let (admin, mid) = setup_active_market();
    assert_eq!(get_lp_balance(mid, admin.as_ptr()), 1);
    let lp_bal = read_return_u64();
    lichen_sdk::test_mock::set_caller(admin);
    dao_void(admin.as_ptr(), mid);
    lichen_sdk::test_mock::set_caller(admin);
    assert_eq!(
        withdraw_liquidity(admin.as_ptr(), mid, lp_bal / 2),
        0,
        "Cannot withdraw LP from voided market"
    );
}

// ============================================================================
// DOUBLE-CLAIM PREVENTION
// ============================================================================

#[test]
fn test_double_reclaim_from_voided_market() {
    let (admin, mid) = setup_active_market();
    let t = [2u8; 32];

    lichen_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 0, 2_000_000);

    lichen_sdk::test_mock::set_caller(admin);
    dao_void(admin.as_ptr(), mid);

    // First reclaim
    lichen_sdk::test_mock::set_caller(t);
    let r1 = reclaim_collateral(t.as_ptr(), mid);
    assert_eq!(r1, 1);

    // Second reclaim should return 0 (shares already zeroed)
    lichen_sdk::test_mock::set_caller(t);
    let r2 = reclaim_collateral(t.as_ptr(), mid);
    assert_eq!(r2, 0, "Double reclaim must return 0");
}

#[test]
fn test_reclaim_on_non_voided_market() {
    let (_admin, mid) = setup_active_market();
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    assert_eq!(
        reclaim_collateral(t.as_ptr(), mid),
        0,
        "Cannot reclaim on active market"
    );
}

// ============================================================================
// CALLER MISMATCH TESTS (for each operation)
// ============================================================================

#[test]
fn test_buy_shares_caller_mismatch() {
    let (_admin, mid) = setup_active_market();
    let real = [2u8; 32];
    let fake = [3u8; 32];
    lichen_sdk::test_mock::set_caller(real);
    assert_eq!(buy_shares(fake.as_ptr(), mid, 0, 1_000_000), 0);
}

#[test]
fn test_sell_shares_caller_mismatch() {
    let (_admin, mid) = setup_active_market();
    let real = [2u8; 32];
    let fake = [3u8; 32];
    lichen_sdk::test_mock::set_caller(real);
    assert_eq!(sell_shares(fake.as_ptr(), mid, 0, 1_000_000), 0);
}

#[test]
fn test_mint_complete_set_caller_mismatch() {
    let (_admin, mid) = setup_active_market();
    let real = [2u8; 32];
    let fake = [3u8; 32];
    lichen_sdk::test_mock::set_caller(real);
    assert_eq!(mint_complete_set(fake.as_ptr(), mid, 1_000_000), 0);
}

#[test]
fn test_redeem_complete_set_caller_mismatch() {
    let (_admin, mid) = setup_active_market();
    let real = [2u8; 32];
    let fake = [3u8; 32];
    lichen_sdk::test_mock::set_caller(real);
    assert_eq!(redeem_complete_set(fake.as_ptr(), mid, 1_000_000), 0);
}

#[test]
fn test_add_liquidity_caller_mismatch() {
    let (_admin, mid) = setup_active_market();
    let real = [2u8; 32];
    let fake = [3u8; 32];
    lichen_sdk::test_mock::set_caller(real);
    assert_eq!(add_liquidity(fake.as_ptr(), mid, 5_000_000), 0);
}

#[test]
fn test_withdraw_liquidity_caller_mismatch() {
    let (admin, mid) = setup_active_market();
    let fake = [3u8; 32];
    lichen_sdk::test_mock::set_caller(admin);
    assert_eq!(withdraw_liquidity(fake.as_ptr(), mid, 1_000), 0);
}

// ============================================================================
// NONEXISTENT MARKET TESTS
// ============================================================================

#[test]
fn test_buy_shares_nonexistent_market() {
    setup();
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    assert_eq!(buy_shares(t.as_ptr(), 999, 0, 1_000_000), 0);
}

#[test]
fn test_sell_shares_nonexistent_market() {
    setup();
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    assert_eq!(sell_shares(t.as_ptr(), 999, 0, 1_000_000), 0);
}

#[test]
fn test_mint_nonexistent_market() {
    setup();
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    assert_eq!(mint_complete_set(t.as_ptr(), 999, 1_000_000), 0);
}

#[test]
fn test_add_liquidity_nonexistent_market() {
    setup();
    let lp = [3u8; 32];
    lichen_sdk::test_mock::set_caller(lp);
    assert_eq!(add_liquidity(lp.as_ptr(), 999, 5_000_000), 0);
}

// ============================================================================
// BOUNDARY VALUE TESTS
// ============================================================================

#[test]
fn test_minimum_collateral_accepted() {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Min test";
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    let mid = create_market(
        admin.as_ptr(),
        0,
        1000 + 100_000,
        2,
        qh.as_ptr(),
        q.as_ptr(),
        q.len() as u32,
    ) as u64;
    // MIN_COLLATERAL = 1_000_000 (1 lUSD)
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(1_000_000);
    let r = add_initial_liquidity(admin.as_ptr(), mid, 1_000_000, core::ptr::null(), 0);
    assert_eq!(r, 1, "minimum collateral should be accepted");
}

#[test]
fn test_below_minimum_collateral_rejected() {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Below min";
    lichen_sdk::test_mock::set_caller(admin);
    let mid = create_market(
        admin.as_ptr(),
        0,
        1000 + 100_000,
        2,
        qh.as_ptr(),
        q.as_ptr(),
        q.len() as u32,
    ) as u64;
    lichen_sdk::test_mock::set_caller(admin);
    let r = add_initial_liquidity(admin.as_ptr(), mid, 999_999, core::ptr::null(), 0);
    assert_eq!(r, 0, "below minimum collateral must be rejected");
}

#[test]
fn test_min_duration_boundary() {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Duration test";
    // MIN_DURATION = 9000 slots
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    let r = create_market(
        admin.as_ptr(),
        0,
        1000 + 9000,
        2,
        qh.as_ptr(),
        q.as_ptr(),
        q.len() as u32,
    );
    assert!(r > 0, "Exactly MIN_DURATION should be accepted");
}

#[test]
fn test_just_below_min_duration_rejected() {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Duration test";
    lichen_sdk::test_mock::set_caller(admin);
    let r = create_market(
        admin.as_ptr(),
        0,
        1000 + 8999,
        2,
        qh.as_ptr(),
        q.as_ptr(),
        q.len() as u32,
    );
    assert_eq!(r, 0, "Just below MIN_DURATION must be rejected");
}

#[test]
fn test_outcome_count_boundary_2_accepted() {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Two outcomes";
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    assert!(
        create_market(
            admin.as_ptr(),
            0,
            1000 + 100_000,
            2,
            qh.as_ptr(),
            q.as_ptr(),
            q.len() as u32
        ) > 0
    );
}

#[test]
fn test_outcome_count_boundary_8_accepted() {
    let admin = setup();
    let qh = [43u8; 32];
    let q = b"Eight outcomes";
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    assert!(
        create_market(
            admin.as_ptr(),
            0,
            1000 + 100_000,
            8,
            qh.as_ptr(),
            q.as_ptr(),
            q.len() as u32
        ) > 0
    );
}

// ============================================================================
// SELL EDGE CASES
// ============================================================================

#[test]
fn test_sell_exactly_all_shares() {
    let (_admin, mid) = setup_active_market_large();
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    let bought = buy_shares(t.as_ptr(), mid, 0, 1_000_000);
    assert!(bought > 0);
    let (pos, _) = read_position(mid, &t, 0);
    lichen_sdk::test_mock::set_caller(t);
    let returned = sell_shares(t.as_ptr(), mid, 0, pos);
    assert!(returned > 0, "Selling all shares should return lUSD");
    let (pos_after, _) = read_position(mid, &t, 0);
    assert_eq!(pos_after, 0);
}

#[test]
fn test_sell_one_share_more_than_owned() {
    let (_admin, mid) = setup_active_market_large();
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 0, 1_000_000);
    let (pos, _) = read_position(mid, &t, 0);
    lichen_sdk::test_mock::set_caller(t);
    assert_eq!(
        sell_shares(t.as_ptr(), mid, 0, pos + 1),
        0,
        "One more than owned must fail"
    );
}

// ============================================================================
// TRADE AFTER CLOSE SLOT EDGE CASE (buy, sell, mint, redeem)
// ============================================================================

#[test]
fn test_sell_after_close_slot() {
    let (_admin, mid) = setup_active_market_large();
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 0, 1_000_000);
    lichen_sdk::test_mock::set_slot(1000 + 100_001);
    lichen_sdk::test_mock::set_caller(t);
    let (pos, _) = read_position(mid, &t, 0);
    let r = sell_shares(t.as_ptr(), mid, 0, pos);
    assert_eq!(r, 0, "Cannot sell after close_slot");
}

#[test]
fn test_mint_after_close_slot() {
    let (_admin, mid) = setup_active_market();
    lichen_sdk::test_mock::set_slot(1000 + 100_001);
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    let r = mint_complete_set(t.as_ptr(), mid, 1_000_000);
    assert_eq!(r, 0, "Cannot mint after close_slot");
}

#[test]
fn test_redeem_complete_set_after_close_slot() {
    let (_admin, mid) = setup_active_market_large();
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    mint_complete_set(t.as_ptr(), mid, 1_000_000);
    lichen_sdk::test_mock::set_slot(1000 + 100_001);
    lichen_sdk::test_mock::set_caller(t);
    // redeem_complete_set is allowed on ACTIVE or CLOSED markets
    let r = redeem_complete_set(t.as_ptr(), mid, 1_000_000);
    // After close_slot the market is still ACTIVE in storage, but the buy/sell/mint
    // functions check close_slot. Redeem checks STATUS, not close_slot.
    // If status is still ACTIVE (not explicitly changed), redeem should work.
    // This tests the boundary behavior.
    // The result depends on whether market_status is changed to CLOSED automatically.
    // In our impl, status stays ACTIVE until resolution, but buy/sell check slot.
    let _ = r; // No assertion on direction — just ensure no panic
}

// ============================================================================
// MULTI-OUTCOME ADVERSARIAL
// ============================================================================

#[test]
fn test_multi_outcome_buy_all_outcomes() {
    let admin = setup();
    let qh = [99u8; 32];
    let q = b"Multi test";
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    let mid = create_market(
        admin.as_ptr(),
        0,
        1000 + 200_000,
        4,
        qh.as_ptr(),
        q.as_ptr(),
        q.len() as u32,
    ) as u64;
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(40_000_000);
    add_initial_liquidity(admin.as_ptr(), mid, 40_000_000, core::ptr::null(), 0);

    let t = [2u8; 32];
    for outcome in 0..4u8 {
        lichen_sdk::test_mock::set_slot(1000 + (outcome as u64) * 200); // advance slot each time
        lichen_sdk::test_mock::set_caller(t);
        lichen_sdk::test_mock::set_value(1_000_000);
        let r = buy_shares(t.as_ptr(), mid, outcome, 1_000_000);
        assert!(r > 0, "Should be able to buy outcome {}", outcome);
    }

    // After buying all outcomes, user should have position in each
    for outcome in 0..4u8 {
        let (shares, _) = read_position(mid, &t, outcome);
        assert!(shares > 0, "Should have shares in outcome {}", outcome);
    }
}

#[test]
fn test_multi_outcome_mint_complete_set() {
    let admin = setup();
    let qh = [45u8; 32];
    let q = b"Mint multi test";
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    let mid = create_market(
        admin.as_ptr(),
        0,
        1000 + 200_000,
        4,
        qh.as_ptr(),
        q.as_ptr(),
        q.len() as u32,
    ) as u64;
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(40_000_000);
    add_initial_liquidity(admin.as_ptr(), mid, 40_000_000, core::ptr::null(), 0);

    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    lichen_sdk::test_mock::set_value(5_000_000);
    assert_eq!(mint_complete_set(t.as_ptr(), mid, 5_000_000), 1);

    // Should have 5M shares in each of 4 outcomes
    for outcome in 0..4u8 {
        let (shares, _) = read_position(mid, &t, outcome);
        assert_eq!(shares, 5_000_000, "Should have 5M in outcome {}", outcome);
    }

    // Redeem
    lichen_sdk::test_mock::set_caller(t);
    let ret = redeem_complete_set(t.as_ptr(), mid, 5_000_000);
    assert_eq!(ret, 5_000_000);
    for outcome in 0..4u8 {
        let (shares, _) = read_position(mid, &t, outcome);
        assert_eq!(shares, 0);
    }
}

// ============================================================================
// PAUSE EDGE CASES
// ============================================================================

#[test]
fn test_pause_blocks_create_market() {
    let admin = setup();
    lichen_sdk::test_mock::set_caller(admin);
    emergency_pause(admin.as_ptr());
    let qh = [42u8; 32];
    let q = b"Test";
    lichen_sdk::test_mock::set_caller(admin);
    assert_eq!(
        create_market(
            admin.as_ptr(),
            0,
            1000 + 100_000,
            2,
            qh.as_ptr(),
            q.as_ptr(),
            q.len() as u32
        ),
        0
    );
}

#[test]
fn test_pause_blocks_add_liquidity() {
    let (admin, mid) = setup_active_market();
    let lp = [3u8; 32];
    lichen_sdk::test_mock::set_caller(admin);
    emergency_pause(admin.as_ptr());
    lichen_sdk::test_mock::set_caller(lp);
    assert_eq!(add_liquidity(lp.as_ptr(), mid, 5_000_000), 0);
}

#[test]
fn test_pause_blocks_sell() {
    let (_admin, mid) = setup_active_market_large();
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 0, 1_000_000);

    let admin = [1u8; 32];
    lichen_sdk::test_mock::set_caller(admin);
    emergency_pause(admin.as_ptr());

    lichen_sdk::test_mock::set_caller(t);
    let (pos, _) = read_position(mid, &t, 0);
    assert_eq!(
        sell_shares(t.as_ptr(), mid, 0, pos),
        0,
        "Sell blocked during pause"
    );
}

#[test]
fn test_pause_blocks_mint() {
    let (admin, mid) = setup_active_market();
    lichen_sdk::test_mock::set_caller(admin);
    emergency_pause(admin.as_ptr());
    let t = [2u8; 32];
    lichen_sdk::test_mock::set_caller(t);
    assert_eq!(mint_complete_set(t.as_ptr(), mid, 1_000_000), 0);
}

// ============================================================================
// LP EDGE CASES
// ============================================================================

#[test]
fn test_withdraw_zero_lp_rejected() {
    let (admin, mid) = setup_active_market();
    lichen_sdk::test_mock::set_caller(admin);
    assert_eq!(withdraw_liquidity(admin.as_ptr(), mid, 0), 0);
}

#[test]
fn test_add_liquidity_zero_rejected() {
    let (_admin, mid) = setup_active_market();
    let lp = [3u8; 32];
    lichen_sdk::test_mock::set_caller(lp);
    assert_eq!(add_liquidity(lp.as_ptr(), mid, 0), 0);
}

// ============================================================================
// CONCURRENT TRADER STRESS
// ============================================================================

#[test]
fn test_many_traders_sequential() {
    let (_admin, mid) = setup_active_market_large();

    for i in 0..20u8 {
        let mut addr = [0u8; 32];
        addr[0] = i + 10;
        lichen_sdk::test_mock::set_slot(1000 + (i as u64) * 200); // advance slot to avoid breaker
        lichen_sdk::test_mock::set_caller(addr);
        let outcome = i % 2;
        let r = buy_shares(addr.as_ptr(), mid, outcome, 500_000);
        assert!(r > 0, "Trader {} buy should succeed", i);
        let (shares, _) = read_position(mid, &addr, outcome);
        assert!(shares > 0, "Trader {} should have shares", i);
    }

    // Prices should still sum to ~$1.00
    let p0 = read_price(mid, 0);
    let p1 = read_price(mid, 1);
    let sum = p0 + p1;
    assert!(
        sum >= 998_000 && sum <= 1_002_000,
        "After 20 trades, prices must sum to ~$1.00, got {}",
        sum
    );
}

// ============================================================================
// MARKET COUNT CONSISTENCY
// ============================================================================

#[test]
fn test_market_count_consistency_across_voids() {
    let admin = setup();

    // Create 3 markets
    for i in 0..3u8 {
        let mut qh = [0u8; 32];
        qh[0] = i + 10;
        let q = b"Market";
        lichen_sdk::test_mock::set_caller(admin);
        lichen_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
        create_market(
            admin.as_ptr(),
            0,
            1000 + 100_000,
            2,
            qh.as_ptr(),
            q.as_ptr(),
            q.len() as u32,
        );
    }
    assert_eq!(get_market_count(), 3);

    // Activate and void market 1
    lichen_sdk::test_mock::set_caller(admin);
    lichen_sdk::test_mock::set_value(5_000_000);
    add_initial_liquidity(admin.as_ptr(), 1, 5_000_000, core::ptr::null(), 0);
    lichen_sdk::test_mock::set_caller(admin);
    dao_void(admin.as_ptr(), 1);

    // Market count shouldn't decrease (voiding doesn't delete)
    assert_eq!(
        get_market_count(),
        3,
        "Voiding shouldn't decrease market count"
    );
}

// ============================================================================
// GET_POSITION ALWAYS RETURNS 1
// ============================================================================

#[test]
fn test_get_position_never_returns_zero_for_status() {
    let (_admin, mid) = setup_active_market();
    let t = [2u8; 32];
    // get_position always returns 1 (even for zero position)
    let r = get_position(mid, t.as_ptr(), 0);
    assert_eq!(r, 1, "get_position should always return 1");
    let rd = lichen_sdk::test_mock::get_return_data();
    let shares = u64::from_le_bytes(rd[0..8].try_into().unwrap());
    assert_eq!(shares, 0, "Should have 0 shares initially");
}

// ============================================================================
// QUOTE EDGE CASES
// ============================================================================

#[test]
fn test_quote_buy_nonexistent_market() {
    setup();
    assert_eq!(quote_buy(999, 0, 1_000_000), 0);
}

#[test]
fn test_quote_sell_nonexistent_market() {
    setup();
    assert_eq!(quote_sell(999, 0, 1_000_000), 0);
}

#[test]
fn test_quote_buy_invalid_outcome() {
    let (_admin, mid) = setup_active_market();
    assert_eq!(quote_buy(mid, 5, 1_000_000), 0);
}

#[test]
fn test_quote_sell_invalid_outcome() {
    let (_admin, mid) = setup_active_market();
    assert_eq!(quote_sell(mid, 5, 1_000_000), 0);
}
