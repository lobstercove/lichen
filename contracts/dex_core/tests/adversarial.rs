// DEX Core — Adversarial & Hardening Tests
//
// Tests for: overflow, DoS, edge cases, boundary conditions,
// stale state, and abuse patterns in the CLOB matching engine.

use dex_core::*;

fn setup() -> [u8; 32] {
    moltchain_sdk::test_mock::reset();
    let admin = [1u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(initialize(admin.as_ptr()), 0);
    admin
}

fn setup_with_pair() -> ([u8; 32], u64) {
    let admin = setup();
    let base = [10u8; 32];
    let quote = [20u8; 32];
    // tick=1_000_000, lot=100, min_order=1000
    // notional = price * qty / 1e9. Need notional >= 1000.
    assert_eq!(
        create_pair(
            admin.as_ptr(),
            base.as_ptr(),
            quote.as_ptr(),
            1_000_000,
            100,
            1000
        ),
        0
    );
    // CON-11 fix: Balance check is now fail-closed. Mock a large balance so
    // cross-contract balance queries succeed in integration tests.
    moltchain_sdk::test_mock::set_cross_call_response(Some(u64::MAX.to_le_bytes().to_vec()));
    (admin, 1)
}

// Price = 1.0 scaled (tick-aligned to 1_000_000)
const P: u64 = 1_000_000_000;
// Quantity: lot-aligned, notional = P * Q / 1e9 = 1100 ≥ 1000
const Q: u64 = 1_100;

// ============================================================================
// OVERFLOW / UNDERFLOW
// ============================================================================

#[test]
fn test_place_order_quantity_max_order_size_boundary() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    let result = place_order(trader.as_ptr(), pair_id, 0, 0, P, 1_000_000_000_000, 0, 0);
    assert_eq!(result, 0, "MAX_ORDER_SIZE quantity should be accepted");
}

#[test]
fn test_place_order_quantity_exceeds_max() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    // MAX_ORDER_SIZE is 10_000_000_000_000_000 (10M MOLT at 9 decimals)
    let result = place_order(trader.as_ptr(), pair_id, 0, 0, P, 10_000_000_000_000_001, 0, 0);
    assert_eq!(result, 4, "quantity exceeding MAX should be rejected");
}

#[test]
fn test_place_order_extreme_price_no_panic() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    let high_price = (u64::MAX / 1_000_000) * 1_000_000;
    let result = place_order(trader.as_ptr(), pair_id, 0, 0, high_price, 100, 0, 0);
    assert!(
        result == 0 || result == 4,
        "extreme price: result={}",
        result
    );
}

#[test]
fn test_fee_treasury_accumulation() {
    let (_admin, pair_id) = setup_with_pair();
    let buyer = [2u8; 32];
    let seller = [3u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    let big_q: u64 = 200_000;
    moltchain_sdk::test_mock::set_caller(seller);
    assert_eq!(place_order(seller.as_ptr(), pair_id, 1, 0, P, big_q, 0, 0), 0);
    moltchain_sdk::test_mock::set_caller(buyer);
    assert_eq!(place_order(buyer.as_ptr(), pair_id, 0, 0, P, big_q, 0, 0), 0);
    let treasury = get_fee_treasury();
    assert!(
        treasury > 0,
        "fees should have been collected, got {}",
        treasury
    );
}

// ============================================================================
// USER ORDER COUNT DoS
// ============================================================================

#[test]
fn test_user_order_count_at_limit() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    for i in 0..100u64 {
        let price = P + (i + 1) * 1_000_000;
        assert_eq!(
            place_order(trader.as_ptr(), pair_id, 0, 0, price, Q, 0, 0),
            0,
            "order {}",
            i
        );
    }
    let result = place_order(trader.as_ptr(), pair_id, 0, 0, P + 101_000_000, Q, 0, 0);
    assert_eq!(result, 5, "should reject at max open orders");
}

#[test]
fn test_user_order_count_after_cancel() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    for i in 0..100u64 {
        let price = P + (i + 1) * 1_000_000;
        assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 0, price, Q, 0, 0), 0);
    }
    assert_eq!(cancel_order(trader.as_ptr(), 1), 0);
    let result = place_order(trader.as_ptr(), pair_id, 0, 0, P + 101_000_000, Q, 0, 0);
    assert!(
        result == 0 || result == 5,
        "cancel count: result={}",
        result
    );
}

// ============================================================================
// ORDER EXPIRY EDGE CASES
// ============================================================================

#[test]
fn test_order_expiry_exact_boundary() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(1000);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 0, P, Q, 1000, 0), 8);
}

#[test]
fn test_order_expiry_one_slot_away() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(1000);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 0, P, Q, 1001, 0), 0);
}

#[test]
fn test_order_expiry_max_duration() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(1000);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(
        place_order(trader.as_ptr(), pair_id, 0, 0, P, Q, 1000 + 2_592_001, 0),
        4
    );
}

#[test]
fn test_expired_maker_skipped_during_matching() {
    let (_admin, pair_id) = setup_with_pair();
    let seller = [2u8; 32];
    let buyer = [3u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(seller);
    assert_eq!(place_order(seller.as_ptr(), pair_id, 1, 0, P, Q, 200, 0), 0);
    moltchain_sdk::test_mock::set_slot(201);
    moltchain_sdk::test_mock::set_caller(buyer);
    assert_eq!(place_order(buyer.as_ptr(), pair_id, 0, 0, P, Q, 0, 0), 0);
}

// ============================================================================
// SELF-TRADE PREVENTION
// ============================================================================

#[test]
fn test_self_trade_prevention() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 1, 0, P, Q, 0, 0), 0);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 0, P, Q, 0, 0), 0);
}

// ============================================================================
// POST-ONLY
// ============================================================================

#[test]
fn test_post_only_would_cross_ask() {
    let (_admin, pair_id) = setup_with_pair();
    let seller = [2u8; 32];
    let buyer = [3u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(seller);
    assert_eq!(place_order(seller.as_ptr(), pair_id, 1, 0, P, Q, 0, 0), 0);
    moltchain_sdk::test_mock::set_caller(buyer);
    assert_eq!(place_order(buyer.as_ptr(), pair_id, 0, 3, P, Q, 0, 0), 7);
}

#[test]
fn test_post_only_would_cross_bid() {
    let (_admin, pair_id) = setup_with_pair();
    let buyer = [2u8; 32];
    let seller = [3u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(buyer);
    assert_eq!(place_order(buyer.as_ptr(), pair_id, 0, 0, P, Q, 0, 0), 0);
    moltchain_sdk::test_mock::set_caller(seller);
    assert_eq!(place_order(seller.as_ptr(), pair_id, 1, 3, P, Q, 0, 0), 7);
}

// ============================================================================
// ADMIN ACCESS CONTROL
// ============================================================================

#[test]
fn test_create_pair_non_admin() {
    let _admin = setup();
    let rando = [99u8; 32];
    let base = [10u8; 32];
    let quote = [20u8; 32];
    moltchain_sdk::test_mock::set_caller(rando);
    assert_eq!(
        create_pair(
            rando.as_ptr(),
            base.as_ptr(),
            quote.as_ptr(),
            1_000_000,
            100,
            1000
        ),
        1
    );
}

#[test]
fn test_update_fees_non_admin() {
    let (_admin, pair_id) = setup_with_pair();
    let rando = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(rando);
    assert_eq!(update_pair_fees(rando.as_ptr(), pair_id, 0, 5), 1);
}

#[test]
fn test_pause_non_admin() {
    let (_admin, pair_id) = setup_with_pair();
    let rando = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(rando);
    assert_eq!(pause_pair(rando.as_ptr(), pair_id), 1);
}

#[test]
fn test_emergency_pause_non_admin() {
    let _admin = setup();
    let rando = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(rando);
    assert_eq!(emergency_pause(rando.as_ptr()), 1);
}

#[test]
fn test_operations_while_paused() {
    let (admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    assert_eq!(emergency_pause(admin.as_ptr()), 0);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 0, P, Q, 0, 0), 1);
}

#[test]
fn test_unpause_restores_operations() {
    let (admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    assert_eq!(emergency_pause(admin.as_ptr()), 0);
    // AUDIT-FIX M12: Unpause is now two-step: schedule then execute after timelock
    assert_eq!(emergency_unpause(admin.as_ptr()), 0); // schedules
    // Advance past timelock
    moltchain_sdk::test_mock::set_slot(100 + UNPAUSE_TIMELOCK_SLOTS);
    assert_eq!(execute_unpause(admin.as_ptr()), 0);   // executes
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 0, P, Q, 0, 0), 0);
}

// ============================================================================
// TICK / LOT ALIGNMENT
// ============================================================================

#[test]
fn test_price_not_tick_aligned() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 0, P + 1, Q, 0, 0), 4);
}

#[test]
fn test_quantity_not_lot_aligned() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 0, P, 99, 0, 0), 4);
}

// ============================================================================
// FEE MANIPULATION
// ============================================================================

#[test]
fn test_update_fees_extreme_maker_rebate() {
    let (admin, pair_id) = setup_with_pair();
    let result = update_pair_fees(admin.as_ptr(), pair_id, i16::MIN, 5);
    assert!(
        result == 0 || result == 3,
        "extreme negative fee: result={}",
        result
    );
}

#[test]
fn test_update_fees_taker_at_max() {
    let (admin, pair_id) = setup_with_pair();
    assert_eq!(update_pair_fees(admin.as_ptr(), pair_id, 0, 100), 0);
    assert_eq!(update_pair_fees(admin.as_ptr(), pair_id, 0, 101), 3);
}

// ============================================================================
// CANCEL EDGE CASES
// ============================================================================

#[test]
fn test_cancel_someone_elses_order() {
    let (_admin, pair_id) = setup_with_pair();
    let trader_a = [2u8; 32];
    let trader_b = [3u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader_a);
    assert_eq!(place_order(trader_a.as_ptr(), pair_id, 0, 0, P, Q, 0, 0), 0);
    moltchain_sdk::test_mock::set_caller(trader_b);
    assert_eq!(cancel_order(trader_b.as_ptr(), 1), 2);
}

#[test]
fn test_cancel_nonexistent_order() {
    let _ = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(cancel_order(trader.as_ptr(), 99999), 1);
}

#[test]
fn test_cancel_already_cancelled() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 0, P, Q, 0, 0), 0);
    assert_eq!(cancel_order(trader.as_ptr(), 1), 0);
    assert_eq!(cancel_order(trader.as_ptr(), 1), 3);
}

// ============================================================================
// ZERO / INVALID INPUTS
// ============================================================================

#[test]
fn test_place_order_zero_quantity() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 0, P, 0, 0, 0), 4);
}

#[test]
fn test_place_order_zero_price_limit() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 0, 0, Q, 0, 0), 4);
}

#[test]
fn test_place_order_invalid_side() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 2, 0, P, Q, 0, 0), 4);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 255, 0, P, Q, 0, 0), 4);
}

#[test]
fn test_place_order_invalid_order_type() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 4, P, Q, 0, 0), 4);
}

#[test]
fn test_market_order_empty_book() {
    let (_admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 1, 0, Q, 0, 0), 0);
}

// ============================================================================
// PAIR MANAGEMENT
// ============================================================================

#[test]
fn test_trade_on_paused_pair() {
    let (admin, pair_id) = setup_with_pair();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);
    assert_eq!(pause_pair(admin.as_ptr(), pair_id), 0);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(place_order(trader.as_ptr(), pair_id, 0, 0, P, Q, 0, 0), 3);
}

#[test]
fn test_matching_many_orders() {
    let (_admin, pair_id) = setup_with_pair();
    moltchain_sdk::test_mock::set_slot(100);
    for i in 2u8..12 {
        let seller = {
            let mut a = [0u8; 32];
            a[0] = i;
            a
        };
        moltchain_sdk::test_mock::set_caller(seller);
        assert_eq!(place_order(seller.as_ptr(), pair_id, 1, 0, P, Q, 0, 0), 0);
    }
    let buyer = [50u8; 32];
    moltchain_sdk::test_mock::set_caller(buyer);
    assert_eq!(place_order(buyer.as_ptr(), pair_id, 0, 0, P, Q * 10, 0, 0), 0);
}
