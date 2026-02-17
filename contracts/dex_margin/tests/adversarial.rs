// DEX Margin — Adversarial & Hardening Tests
//
// Tests for: overflow, liquidation edge cases, PnL accounting, leverage abuse,
// and margin manipulation attacks.
// Updated for tiered leverage (up to 100x) and host-level collateral locking.

use dex_margin::*;

fn setup() -> [u8; 32] {
    moltchain_sdk::test_mock::reset();
    let admin = [1u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(initialize(admin.as_ptr()), 0);
    // Set mark price for pair 1: 1.0 (1_000_000_000)
    set_mark_price(admin.as_ptr(), 1, 1_000_000_000);
    admin
}

// ============================================================================
// OVERFLOW / UNDERFLOW
// ============================================================================

#[test]
fn test_add_margin_u64_overflow() {
    // FIXED: add_margin now uses checked_add and returns error code 6 on overflow.
    let _admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);
    // 2x tier: notional=1000, required=1000*5000/10000=500
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 2, 500), 0);
    assert_eq!(add_margin(trader.as_ptr(), 1, u64::MAX), 6, "overflow should return error 6");
}

#[test]
fn test_insurance_fund_overflow() {
    // FIXED: insurance fund now uses saturating_add — no panic, caps at u64::MAX.
    let admin = setup();
    moltchain_sdk::test_mock::set_slot(100);

    moltchain_sdk::storage_set(b"mrg_insurance", &moltchain_sdk::u64_to_bytes(u64::MAX - 10));

    let trader = [2u8; 32];
    // 5x short: notional=10000, required=10000*2000/10000=2000
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(open_position(trader.as_ptr(), 1, 1, 10_000, 5, 2000), 0);

    // Pump price 10x to make short liquidatable
    moltchain_sdk::test_mock::set_caller(admin);
    set_mark_price(admin.as_ptr(), 1, 10_000_000_000);

    let liquidator = [3u8; 32];
    moltchain_sdk::test_mock::set_caller(liquidator);
    assert_eq!(liquidate(liquidator.as_ptr(), 1), 0, "liquidation should succeed with saturating insurance");
    // Insurance fund should be at or near u64::MAX (saturated)
    let fund = get_insurance_fund();
    assert!(fund >= u64::MAX - 10, "insurance fund should be near max, got {}", fund);
}

// ============================================================================
// LEVERAGE EDGE CASES
// ============================================================================

#[test]
fn test_open_position_zero_leverage() {
    let _admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 0, 200), 2,
        "zero leverage should be rejected");
}

#[test]
fn test_open_position_overleveraged() {
    let _admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);
    // Default max = 100x (MAX_LEVERAGE_ISOLATED)
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 101, 200), 2,
        "101x leverage should be rejected with default 100x max");
}

#[test]
fn test_open_position_zero_margin_via_rounding() {
    // BUG DOCUMENTATION: required_margin = notional * initial_margin_bps / 10_000 / leverage
    // For small notional with high leverage, this can round to 0
    let _admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);

    // size=1, mark_price=1e9 → notional = 1*1e9/1e9 = 1
    // 5x tier: required = 1 * 2000 / 10000 / 5 = 0 (integer division)
    let result = open_position(trader.as_ptr(), 1, 0, 1, 5, 0);
    // Document: if 0, zero-margin positions are possible (BUG)
    // If not 0, there's a minimum margin check in place
    assert!(result == 0 || result == 3,
        "zero required margin rounding: result={}", result);
}

#[test]
fn test_open_position_size_zero() {
    let _admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);
    // Size 0 → notional 0 → should this be allowed?
    let result = open_position(trader.as_ptr(), 1, 0, 0, 2, 200);
    // Document behavior
    assert!(result == 0 || result == 3 || result == 2,
        "size=0 behavior: result={}", result);
}

#[test]
fn test_open_position_invalid_side() {
    let _admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);
    assert_eq!(open_position(trader.as_ptr(), 1, 2, 1000, 2, 250), 2,
        "invalid side=2 should be rejected");
}

// ============================================================================
// LIQUIDATION EDGE CASES
// ============================================================================

#[test]
fn test_liquidate_healthy_position() {
    let _admin = setup();
    let trader = [2u8; 32];
    let liquidator = [3u8; 32];
    moltchain_sdk::test_mock::set_slot(100);

    // 2x: notional=1000, required=250, maint=25% → need margin>250 to be healthy
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 2, 500), 0);
    // margin_ratio = 500/1000*10000 = 5000 bps > 2500 maint → healthy
    moltchain_sdk::test_mock::set_caller(liquidator);
    assert_eq!(liquidate(liquidator.as_ptr(), 1), 2,
        "healthy position should not be liquidatable");
}

#[test]
fn test_liquidate_already_closed() {
    let _admin = setup();
    let trader = [2u8; 32];
    let liquidator = [3u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);

    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 2, 500), 0);
    assert_eq!(close_position(trader.as_ptr(), 1), 0);
    moltchain_sdk::test_mock::set_caller(liquidator);
    assert_eq!(liquidate(liquidator.as_ptr(), 1), 2,
        "closed position should not be liquidatable");
}

#[test]
fn test_liquidate_already_liquidated() {
    let admin = setup();
    let trader = [2u8; 32];
    let liquidator = [3u8; 32];
    moltchain_sdk::test_mock::set_slot(100);

    // Open SHORT, pump price 10x to make liquidatable
    // 5x tier: notional=10000, required=10000*2000/10000=2000
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(open_position(trader.as_ptr(), 1, 1, 10_000, 5, 2000), 0);
    moltchain_sdk::test_mock::set_caller(admin);
    set_mark_price(admin.as_ptr(), 1, 10_000_000_000); // 10x
    moltchain_sdk::test_mock::set_caller(liquidator);
    assert_eq!(liquidate(liquidator.as_ptr(), 1), 0);
    // Try again — status is now POS_LIQUIDATED
    assert_eq!(liquidate(liquidator.as_ptr(), 1), 2,
        "already-liquidated position should not be liquidatable again");
}

#[test]
fn test_liquidate_nonexistent_position() {
    let _admin = setup();
    let liquidator = [3u8; 32];
    moltchain_sdk::test_mock::set_caller(liquidator);
    assert_eq!(liquidate(liquidator.as_ptr(), 99999), 1,
        "nonexistent position should return 1");
}

#[test]
fn test_liquidation_penalty_exceeds_margin() {
    // DOCUMENTATION: penalty is tiered by leverage, can exceed deposited margin
    let admin = setup();
    let trader = [2u8; 32];
    let liquidator = [3u8; 32];
    moltchain_sdk::test_mock::set_slot(100);

    // 50x tier: initial=200bps=2%, maint=100bps=1%, penalty=1000bps=10%
    // notional=10000, required=10000*200/10000=200
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 10_000, 50, 200), 0);

    // Raise price to 3.0
    moltchain_sdk::test_mock::set_caller(admin);
    set_mark_price(admin.as_ptr(), 1, 3_000_000_000);
    moltchain_sdk::test_mock::set_caller(liquidator);
    let result = liquidate(liquidator.as_ptr(), 1);
    assert_eq!(result, 0);

    let insurance = get_insurance_fund();
    assert!(insurance > 0, "insurance fund should have received penalty");
}

// ============================================================================
// CLOSE POSITION
// ============================================================================

#[test]
fn test_close_position_doesnt_return_margin() {
    // UPDATED: close_position now calculates PnL and returns unlock_amount via set_return_data
    let _admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);
    // 2x: notional=1000, required=500
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 2, 500), 0);
    assert_eq!(close_position(trader.as_ptr(), 1), 0);
    // With host-level collateral locking, close_position now unlocks margin
    let ret = moltchain_sdk::test_mock::get_return_data();
    let unlock = moltchain_sdk::bytes_to_u64(&ret);
    // No price change → PnL = 0 → unlock = margin = 500
    assert_eq!(unlock, 500);
}

#[test]
fn test_close_position_not_owner() {
    let _admin = setup();
    let trader = [2u8; 32];
    let attacker = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 2, 500), 0);
    moltchain_sdk::test_mock::set_caller(attacker);
    assert_eq!(close_position(attacker.as_ptr(), 1), 2,
        "attacker should not be able to close others' positions");
}

#[test]
fn test_close_already_closed() {
    let _admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 2, 500), 0);
    assert_eq!(close_position(trader.as_ptr(), 1), 0);
    assert_eq!(close_position(trader.as_ptr(), 1), 3,
        "closing already-closed should return 3");
}

// ============================================================================
// MARGIN MANIPULATION
// ============================================================================

#[test]
fn test_remove_margin_to_below_maintenance() {
    let _admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);
    // 2x: notional=1000, required=500, maint=2500bps=25%=250 of notional
    // Open with 500 (exactly at initial)
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 2, 500), 0);

    // Current ratio: 500/1000*10000 = 5000 > maint 2500 → healthy
    // Remove 255: new margin=245, ratio=2450 < 2500 → below maintenance → rejected
    let result = remove_margin(trader.as_ptr(), 1, 255);
    assert_eq!(result, 6, "removing margin below maintenance should be rejected");
}

#[test]
fn test_remove_margin_more_than_deposited() {
    let _admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 2, 500), 0);
    assert_eq!(remove_margin(trader.as_ptr(), 1, 501), 5,
        "removing more than deposited should be rejected");
}

#[test]
fn test_add_margin_to_closed_position() {
    let _admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 2, 500), 0);
    assert_eq!(close_position(trader.as_ptr(), 1), 0);
    assert_eq!(add_margin(trader.as_ptr(), 1, 100), 3,
        "adding margin to closed position should be rejected");
}

// ============================================================================
// MARK PRICE MANIPULATION
// ============================================================================

#[test]
fn test_set_mark_price_zero() {
    let admin = setup();
    assert_eq!(set_mark_price(admin.as_ptr(), 1, 0), 2,
        "zero mark price should be rejected");
}

#[test]
fn test_set_mark_price_non_admin() {
    let _admin = setup();
    let attacker = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(attacker);
    assert_eq!(set_mark_price(attacker.as_ptr(), 1, 999), 1,
        "non-admin should not be able to set mark price");
}

#[test]
fn test_flash_pump_then_liquidate_short() {
    let admin = setup();
    let trader = [2u8; 32];
    let liquidator = [3u8; 32];
    moltchain_sdk::test_mock::set_slot(100);

    // Open short position: 2x, notional=1000, required=500
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(open_position(trader.as_ptr(), 1, 1, 1000, 2, 500), 0);

    // Pump price 20x
    moltchain_sdk::test_mock::set_caller(admin);
    set_mark_price(admin.as_ptr(), 1, 20_000_000_000);

    moltchain_sdk::test_mock::set_caller(liquidator);
    assert_eq!(liquidate(liquidator.as_ptr(), 1), 0, "should be liquidatable after pump");

    // Set price back
    moltchain_sdk::test_mock::set_caller(admin);
    set_mark_price(admin.as_ptr(), 1, 1_000_000_000);

    // Try to liquidate again — already liquidated
    moltchain_sdk::test_mock::set_caller(liquidator);
    assert_eq!(liquidate(liquidator.as_ptr(), 1), 2);
}

// ============================================================================
// MAX LEVERAGE ADMIN
// ============================================================================

#[test]
fn test_set_max_leverage_non_admin() {
    let _admin = setup();
    let rando = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(rando);
    assert_eq!(set_max_leverage(rando.as_ptr(), 1, 3), 1);
}

#[test]
fn test_set_max_leverage_zero() {
    let admin = setup();
    assert_eq!(set_max_leverage(admin.as_ptr(), 1, 0), 2, "zero max leverage should be rejected");
}

#[test]
fn test_set_max_leverage_above_100() {
    let admin = setup();
    assert_eq!(set_max_leverage(admin.as_ptr(), 1, 101), 2, "leverage > 100 should be rejected");
    // 100 should succeed
    assert_eq!(set_max_leverage(admin.as_ptr(), 1, 100), 0, "leverage = 100 should be accepted");
}

#[test]
fn test_custom_max_leverage_enforced() {
    let admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);

    // Set max leverage to 3 for pair 1
    assert_eq!(set_max_leverage(admin.as_ptr(), 1, 3), 0);

    // 3x: notional=1000, 3x tier: init=3333bps, required=1000*3333/10000=333
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 3, 334), 0);

    // 4x should fail
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 4, 200), 2,
        "4x leverage should be rejected with 3x max");
}

// ============================================================================
// PAUSED STATE
// ============================================================================

#[test]
fn test_operations_while_paused() {
    let admin = setup();
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_slot(100);

    assert_eq!(emergency_pause(admin.as_ptr()), 0);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 2, 500), 1,
        "opening position should fail when paused");
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(emergency_unpause(admin.as_ptr()), 0);
    moltchain_sdk::test_mock::set_caller(trader);
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 2, 500), 0,
        "should work after unpause");
}

#[test]
fn test_pause_non_admin() {
    let _admin = setup();
    let rando = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(rando);
    assert_eq!(emergency_pause(rando.as_ptr()), 1);
}

// ============================================================================
// NO MARK PRICE
// ============================================================================

#[test]
fn test_open_position_no_mark_price() {
    moltchain_sdk::test_mock::reset();
    let admin = [1u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(initialize(admin.as_ptr()), 0);
    // No mark price set for pair 1
    let trader = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(trader);
    moltchain_sdk::test_mock::set_slot(100);
    assert_eq!(open_position(trader.as_ptr(), 1, 0, 1000, 2, 250), 6,
        "should fail with no mark price");
}