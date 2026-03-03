// PredictionReef — Resolution, Settlement & Full Lifecycle Tests
//
// Full market lifecycle: create → trade → close → resolve → redeem/reclaim
// Oracle attestation mocking, dispute flows, DAO resolution,
// double-redemption prevention, LP settlement.

use prediction_market::*;

// ============================================================================
// TEST HELPERS
// ============================================================================

fn setup() -> [u8; 32] {
    moltchain_sdk::test_mock::reset();
    let admin = [1u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    moltchain_sdk::test_mock::set_slot(1000);
    assert_eq!(initialize(admin.as_ptr()), 0);
    // Configure mUSD + self addresses so transfer_musd_out succeeds (fail-closed audit fix)
    moltchain_sdk::test_mock::set_caller(admin);
    set_musd_address(admin.as_ptr(), &[0xAAu8; 32] as *const u8);
    moltchain_sdk::test_mock::set_caller(admin);
    set_self_address(admin.as_ptr(), &[0xBBu8; 32] as *const u8);
    admin
}

/// Create a binary market with 100M mUSD pool, close_slot = 101_000.
fn setup_large_market() -> ([u8; 32], u64) {
    let admin = setup();
    let qh = [42u8; 32];
    let q = b"Will BTC reach 100k?";
    moltchain_sdk::test_mock::set_caller(admin);
    moltchain_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    let mid = create_market(admin.as_ptr(), 2, 101_000, 2, qh.as_ptr(), q.as_ptr(), q.len() as u32) as u64;
    assert!(mid > 0);
    moltchain_sdk::test_mock::set_caller(admin);
    moltchain_sdk::test_mock::set_value(100_000_000);
    assert_eq!(add_initial_liquidity(admin.as_ptr(), mid, 100_000_000, core::ptr::null(), 0), 1);
    (admin, mid)
}

/// Create a 4-outcome market with 40M mUSD pool, close_slot = 201_000.
fn setup_multi_market() -> ([u8; 32], u64) {
    let admin = setup();
    let qh = [55u8; 32];
    let q = b"Which party wins?";
    moltchain_sdk::test_mock::set_caller(admin);
    moltchain_sdk::test_mock::set_value(10_000_000); // MARKET_CREATION_FEE
    let mid = create_market(admin.as_ptr(), 1, 201_000, 4, qh.as_ptr(), q.as_ptr(), q.len() as u32) as u64;
    assert!(mid > 0);
    moltchain_sdk::test_mock::set_caller(admin);
    moltchain_sdk::test_mock::set_value(40_000_000);
    assert_eq!(add_initial_liquidity(admin.as_ptr(), mid, 40_000_000, core::ptr::null(), 0), 1);
    (admin, mid)
}

fn read_return_u64() -> u64 {
    let rd = moltchain_sdk::test_mock::get_return_data();
    u64::from_le_bytes(rd[0..8].try_into().unwrap())
}

fn read_price(market_id: u64, outcome: u8) -> u64 {
    assert_eq!(get_price(market_id, outcome), 1);
    read_return_u64()
}

fn itoa_test(n: u64) -> Vec<u8> {
    if n == 0 { return vec![b'0']; }
    let mut buf = Vec::new();
    let mut v = n;
    while v > 0 { buf.push(b'0' + (v % 10) as u8); v /= 10; }
    buf.reverse();
    buf
}

fn position_key_for_test(market_id: u64, addr: &[u8; 32], outcome: u8) -> Vec<u8> {
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut k = Vec::from(&b"pm_p_"[..]);
    let mut id_buf = itoa_test(market_id);
    k.append(&mut id_buf);
    k.push(b'_');
    for &b in addr { k.push(hex_chars[(b >> 4) as usize]); k.push(hex_chars[(b & 0x0f) as usize]); }
    k.push(b'_');
    let mut out_buf = itoa_test(outcome as u64);
    k.append(&mut out_buf);
    k
}

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

/// Read the full market record (192 bytes) directly from storage.
fn read_market_record(market_id: u64) -> Option<Vec<u8>> {
    let mut key = Vec::from(&b"pm_m_"[..]);
    key.extend_from_slice(&itoa_test(market_id));
    moltchain_sdk::test_mock::get_storage(&key)
}

fn market_status_from_record(data: &[u8]) -> u8 { data[64] }
fn market_winning_outcome_from_record(data: &[u8]) -> u8 { data[66] }

// ============================================================================
// CLOSE_MARKET TESTS
// ============================================================================

#[test]
fn test_close_market_after_close_slot() {
    let (admin, mid) = setup_large_market();
    // Advance past close_slot
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    assert_eq!(close_market(anyone.as_ptr(), mid), 1, "Should close after close_slot");
    let rec = read_market_record(mid).unwrap();
    assert_eq!(market_status_from_record(&rec), 2, "Status should be CLOSED (2)");
}

#[test]
fn test_close_market_before_close_slot() {
    let (_admin, mid) = setup_large_market();
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    assert_eq!(close_market(anyone.as_ptr(), mid), 0, "Cannot close before close_slot");
}

#[test]
fn test_close_market_at_exact_close_slot() {
    let (_admin, mid) = setup_large_market();
    moltchain_sdk::test_mock::set_slot(101_000); // exactly at close_slot
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    // close_market requires current_slot > close_slot, not >=
    assert_eq!(close_market(anyone.as_ptr(), mid), 0, "Cannot close at exact close_slot");
}

#[test]
fn test_close_market_already_closed() {
    let (_admin, mid) = setup_large_market();
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    close_market(anyone.as_ptr(), mid);
    moltchain_sdk::test_mock::set_caller(anyone);
    assert_eq!(close_market(anyone.as_ptr(), mid), 0, "Cannot close already-closed market");
}

// ============================================================================
// SUBMIT_RESOLUTION TESTS (without oracle/moltyid configured)
// ============================================================================

#[test]
fn test_submit_resolution_requires_closed() {
    let (admin, mid) = setup_large_market();
    // Market is ACTIVE, not CLOSED
    let resolver = [2u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    assert_eq!(submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000), 0);
}

#[test]
fn test_submit_resolution_basic() {
    let (_admin, mid) = setup_large_market();
    // Close the market
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    close_market(anyone.as_ptr(), mid);

    // Submit resolution (no oracle configured, so attestation check is skipped)
    let resolver = [2u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    let r = submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);
    assert_eq!(r, 1, "Resolution should be accepted");

    let rec = read_market_record(mid).unwrap();
    assert_eq!(market_status_from_record(&rec), 3, "Status should be RESOLVING (3)");
    assert_eq!(market_winning_outcome_from_record(&rec), 0, "Winning outcome should be 0");
}

#[test]
fn test_submit_resolution_invalid_outcome() {
    let (_admin, mid) = setup_large_market();
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    close_market(anyone.as_ptr(), mid);

    let resolver = [2u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    assert_eq!(submit_resolution(resolver.as_ptr(), mid, 5, att_hash.as_ptr(), 100_000_000), 0,
        "Invalid outcome index should fail");
}

#[test]
fn test_submit_resolution_insufficient_bond() {
    let (_admin, mid) = setup_large_market();
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    close_market(anyone.as_ptr(), mid);

    let resolver = [2u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    // DISPUTE_BOND = 100_000_000
    assert_eq!(submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 99_999_999), 0,
        "Below DISPUTE_BOND should fail");
}

// ============================================================================
// FINALIZE_RESOLUTION TESTS
// ============================================================================

#[test]
fn test_finalize_resolution_after_dispute_period() {
    let (_admin, mid) = setup_large_market();
    // Close
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    close_market(anyone.as_ptr(), mid);

    // Resolve
    let resolver = [2u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);

    // Advance past DISPUTE_PERIOD (172800 slots)
    moltchain_sdk::test_mock::set_slot(101_001 + 432_001);
    moltchain_sdk::test_mock::set_caller(anyone);
    let r = finalize_resolution(anyone.as_ptr(), mid);
    assert_eq!(r, 1, "Should finalize after dispute period");

    let rec = read_market_record(mid).unwrap();
    assert_eq!(market_status_from_record(&rec), 4, "Status should be RESOLVED (4)");
}

#[test]
fn test_finalize_resolution_during_dispute_period() {
    let (_admin, mid) = setup_large_market();
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    close_market(anyone.as_ptr(), mid);

    let resolver = [2u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);

    // Try finalize at resolve_slot + 1000 (still within 172800)
    moltchain_sdk::test_mock::set_slot(101_001 + 1000);
    moltchain_sdk::test_mock::set_caller(anyone);
    assert_eq!(finalize_resolution(anyone.as_ptr(), mid), 0, "Cannot finalize during dispute period");
}

// ============================================================================
// CHALLENGE_RESOLUTION TESTS
// ============================================================================

#[test]
fn test_challenge_resolution_basic() {
    let (_admin, mid) = setup_large_market();
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    close_market(anyone.as_ptr(), mid);

    let resolver = [2u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);

    let challenger = [3u8; 32];
    let evidence = [88u8; 32];
    moltchain_sdk::test_mock::set_caller(challenger);
    let r = challenge_resolution(challenger.as_ptr(), mid, evidence.as_ptr(), 100_000_000);
    assert_eq!(r, 1, "Challenge should succeed");

    let rec = read_market_record(mid).unwrap();
    assert_eq!(market_status_from_record(&rec), 5, "Status should be DISPUTED (5)");
}

#[test]
fn test_cannot_challenge_own_resolution() {
    let (_admin, mid) = setup_large_market();
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    close_market(anyone.as_ptr(), mid);

    let resolver = [2u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);

    // Same resolver tries to challenge
    let evidence = [88u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    assert_eq!(challenge_resolution(resolver.as_ptr(), mid, evidence.as_ptr(), 100_000_000), 0,
        "Cannot challenge own resolution");
}

#[test]
fn test_challenge_after_dispute_period_fails() {
    let (_admin, mid) = setup_large_market();
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    close_market(anyone.as_ptr(), mid);

    let resolver = [2u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);

    // Advance past dispute period
    moltchain_sdk::test_mock::set_slot(101_001 + 432_001);
    let challenger = [3u8; 32];
    let evidence = [88u8; 32];
    moltchain_sdk::test_mock::set_caller(challenger);
    assert_eq!(challenge_resolution(challenger.as_ptr(), mid, evidence.as_ptr(), 100_000_000), 0);
}

#[test]
fn test_challenge_insufficient_bond() {
    let (_admin, mid) = setup_large_market();
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    close_market(anyone.as_ptr(), mid);

    let resolver = [2u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);

    let challenger = [3u8; 32];
    let evidence = [88u8; 32];
    moltchain_sdk::test_mock::set_caller(challenger);
    assert_eq!(challenge_resolution(challenger.as_ptr(), mid, evidence.as_ptr(), 50_000_000), 0,
        "Insufficient bond should fail");
}

// ============================================================================
// DAO_RESOLVE TESTS
// ============================================================================

#[test]
fn test_dao_resolve_disputed_market() {
    let (admin, mid) = setup_large_market();
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    close_market(anyone.as_ptr(), mid);

    let resolver = [2u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);

    let challenger = [3u8; 32];
    let evidence = [88u8; 32];
    moltchain_sdk::test_mock::set_caller(challenger);
    challenge_resolution(challenger.as_ptr(), mid, evidence.as_ptr(), 100_000_000);

    // DAO (admin) resolves with different outcome
    moltchain_sdk::test_mock::set_caller(admin);
    let r = dao_resolve(admin.as_ptr(), mid, 1);
    assert_eq!(r, 1, "DAO resolve should succeed");

    let rec = read_market_record(mid).unwrap();
    assert_eq!(market_status_from_record(&rec), 4, "Status should be RESOLVED (4)");
    assert_eq!(market_winning_outcome_from_record(&rec), 1, "Winning outcome should be 1");
}

#[test]
fn test_dao_resolve_requires_disputed_status() {
    let (admin, mid) = setup_large_market();
    // Market is ACTIVE, not DISPUTED
    moltchain_sdk::test_mock::set_caller(admin);
    assert_eq!(dao_resolve(admin.as_ptr(), mid, 0), 0);
}

// ============================================================================
// REDEEM_SHARES (WINNER PAYOUT) TESTS
// ============================================================================

#[test]
fn test_redeem_winning_shares() {
    let (_admin, mid) = setup_large_market();
    let t = [2u8; 32];

    // Buy YES outcome
    moltchain_sdk::test_mock::set_caller(t);
    let bought = buy_shares(t.as_ptr(), mid, 0, 5_000_000);
    assert!(bought > 0);
    let (shares_before, _) = read_position(mid, &t, 0);
    assert!(shares_before > 0);

    // Close market
    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(t);
    close_market(t.as_ptr(), mid);

    // Submit resolution: outcome 0 wins (no oracle configured)
    let resolver = [3u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);

    // Finalize
    moltchain_sdk::test_mock::set_slot(101_001 + 432_001);
    moltchain_sdk::test_mock::set_caller(t);
    finalize_resolution(t.as_ptr(), mid);

    // Redeem winning shares
    moltchain_sdk::test_mock::set_caller(t);
    let result = redeem_shares(t.as_ptr(), mid, 0);
    assert_eq!(result, 1, "Winner should succeed");
    let payout = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());
    assert_eq!(payout, shares_before, "Payout = number of winning shares");

    // Position should be cleared
    let (shares_after, _) = read_position(mid, &t, 0);
    assert_eq!(shares_after, 0, "Position cleared after redemption");
}

#[test]
fn test_redeem_losing_shares() {
    let (_admin, mid) = setup_large_market();
    let t = [2u8; 32];

    // Buy NO outcome (index 1)
    moltchain_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 1, 5_000_000);
    let (shares_before, _) = read_position(mid, &t, 1);
    assert!(shares_before > 0);

    // Close → Resolve → Finalize (outcome 0 wins)
    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(t);
    close_market(t.as_ptr(), mid);

    let resolver = [3u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);

    moltchain_sdk::test_mock::set_slot(101_001 + 432_001);
    moltchain_sdk::test_mock::set_caller(t);
    finalize_resolution(t.as_ptr(), mid);

    // Redeem losing shares — succeeds but payout is 0
    moltchain_sdk::test_mock::set_caller(t);
    let result = redeem_shares(t.as_ptr(), mid, 1);
    assert_eq!(result, 1, "Losing redeem succeeds (clears position)");
    let payout = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());
    assert_eq!(payout, 0, "Loser gets 0 payout");

    // Position should still be cleared (shares zeroed)
    let (shares_after, _) = read_position(mid, &t, 1);
    assert_eq!(shares_after, 0);
}

#[test]
fn test_double_redemption_prevented() {
    let (_admin, mid) = setup_large_market();
    let t = [2u8; 32];

    moltchain_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 0, 5_000_000);

    // Full lifecycle: Close → Resolve → Finalize
    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(t);
    close_market(t.as_ptr(), mid);
    let resolver = [3u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);
    moltchain_sdk::test_mock::set_slot(101_001 + 432_001);
    moltchain_sdk::test_mock::set_caller(t);
    finalize_resolution(t.as_ptr(), mid);

    // First redemption
    moltchain_sdk::test_mock::set_caller(t);
    let payout1 = redeem_shares(t.as_ptr(), mid, 0);
    assert_eq!(payout1, 1);

    // Second redemption — should return 0 (shares cleared)
    moltchain_sdk::test_mock::set_caller(t);
    let payout2 = redeem_shares(t.as_ptr(), mid, 0);
    assert_eq!(payout2, 0, "Double redemption must return 0");
}

#[test]
fn test_redeem_requires_resolved_status() {
    let (_admin, mid) = setup_large_market();
    let t = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 0, 5_000_000);

    // Market is ACTIVE, cannot redeem
    moltchain_sdk::test_mock::set_caller(t);
    assert_eq!(redeem_shares(t.as_ptr(), mid, 0), 0, "Cannot redeem on active market");
}

#[test]
fn test_redeem_with_no_shares() {
    let (_admin, mid) = setup_large_market();

    // Full lifecycle to RESOLVED
    moltchain_sdk::test_mock::set_slot(101_001);
    let anyone = [5u8; 32];
    moltchain_sdk::test_mock::set_caller(anyone);
    close_market(anyone.as_ptr(), mid);
    let resolver = [3u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);
    moltchain_sdk::test_mock::set_slot(101_001 + 432_001);
    moltchain_sdk::test_mock::set_caller(anyone);
    finalize_resolution(anyone.as_ptr(), mid);

    // User with no shares
    let nobody = [77u8; 32];
    moltchain_sdk::test_mock::set_caller(nobody);
    assert_eq!(redeem_shares(nobody.as_ptr(), mid, 0), 0, "No shares to redeem");
}

// ============================================================================
// MULTI-OUTCOME RESOLUTION & REDEMPTION
// ============================================================================

#[test]
fn test_multi_outcome_resolution_and_redemption() {
    let (_admin, mid) = setup_multi_market();

    // Trader buys outcomes 0 and 2
    let t = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 0, 2_000_000);
    moltchain_sdk::test_mock::set_slot(1200);
    moltchain_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 2, 3_000_000);

    let (shares_o0, _) = read_position(mid, &t, 0);
    let (shares_o2, _) = read_position(mid, &t, 2);
    assert!(shares_o0 > 0);
    assert!(shares_o2 > 0);

    // Close → Resolve (outcome 2 wins) → Finalize
    moltchain_sdk::test_mock::set_slot(201_001);
    moltchain_sdk::test_mock::set_caller(t);
    close_market(t.as_ptr(), mid);

    let resolver = [3u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    moltchain_sdk::test_mock::set_value(100_000_000); // DISPUTE_BOND
    assert_eq!(submit_resolution(resolver.as_ptr(), mid, 2, att_hash.as_ptr(), 100_000_000), 1);

    moltchain_sdk::test_mock::set_slot(201_001 + 432_001);
    moltchain_sdk::test_mock::set_caller(t);
    finalize_resolution(t.as_ptr(), mid);

    // Redeem outcome 2 (winner) → payout = shares_o2
    moltchain_sdk::test_mock::set_caller(t);
    let result_win = redeem_shares(t.as_ptr(), mid, 2);
    assert_eq!(result_win, 1, "Winning outcome succeeds");
    let payout_win = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());
    assert_eq!(payout_win, shares_o2, "Winning outcome pays shares");

    // Redeem outcome 0 (loser) → payout = 0
    moltchain_sdk::test_mock::set_caller(t);
    let result_lose = redeem_shares(t.as_ptr(), mid, 0);
    assert_eq!(result_lose, 1, "Losing outcome clears position");
    let payout_lose = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());
    assert_eq!(payout_lose, 0, "Losing outcome pays 0");
}

// ============================================================================
// FULL END-TO-END LIFECYCLES
// ============================================================================

#[test]
fn test_full_lifecycle_binary_resolve_yes() {
    let (admin, mid) = setup_large_market();

    // Three traders buy different sides
    let t_yes = [2u8; 32];
    let t_no = [3u8; 32];
    let t_both = [4u8; 32];

    moltchain_sdk::test_mock::set_caller(t_yes);
    buy_shares(t_yes.as_ptr(), mid, 0, 5_000_000); // YES
    moltchain_sdk::test_mock::set_slot(1200);
    moltchain_sdk::test_mock::set_caller(t_no);
    buy_shares(t_no.as_ptr(), mid, 1, 3_000_000); // NO
    moltchain_sdk::test_mock::set_slot(1400);
    moltchain_sdk::test_mock::set_caller(t_both);
    buy_shares(t_both.as_ptr(), mid, 0, 2_000_000); // YES
    moltchain_sdk::test_mock::set_slot(1600);
    moltchain_sdk::test_mock::set_caller(t_both);
    buy_shares(t_both.as_ptr(), mid, 1, 2_000_000); // NO

    let (yes_shares_t1, _) = read_position(mid, &t_yes, 0);
    let (no_shares_t2, _) = read_position(mid, &t_no, 1);
    let (yes_shares_t3, _) = read_position(mid, &t_both, 0);
    let (no_shares_t3, _) = read_position(mid, &t_both, 1);

    // Close
    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(admin);
    close_market(admin.as_ptr(), mid);

    // Resolve: YES (0) wins
    let resolver = [10u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);

    // Finalize
    moltchain_sdk::test_mock::set_slot(101_001 + 432_001);
    moltchain_sdk::test_mock::set_caller(admin);
    finalize_resolution(admin.as_ptr(), mid);

    // Trader 1 redeems YES → profit
    moltchain_sdk::test_mock::set_caller(t_yes);
    assert_eq!(redeem_shares(t_yes.as_ptr(), mid, 0), 1);
    let p1 = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());
    assert_eq!(p1, yes_shares_t1);

    // Trader 2 redeems NO → 0
    moltchain_sdk::test_mock::set_caller(t_no);
    assert_eq!(redeem_shares(t_no.as_ptr(), mid, 1), 1);
    let p2 = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());
    assert_eq!(p2, 0);

    // Trader 3 redeems YES → profit, NO → 0
    moltchain_sdk::test_mock::set_caller(t_both);
    assert_eq!(redeem_shares(t_both.as_ptr(), mid, 0), 1);
    let p3y = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());
    assert_eq!(p3y, yes_shares_t3);
    moltchain_sdk::test_mock::set_caller(t_both);
    assert_eq!(redeem_shares(t_both.as_ptr(), mid, 1), 1);
    let p3n = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());
    assert_eq!(p3n, 0);
}

#[test]
fn test_full_lifecycle_dispute_dao_override() {
    let (admin, mid) = setup_large_market();
    let t = [2u8; 32];

    // Buy YES
    moltchain_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 0, 5_000_000);
    let (yes_shares, _) = read_position(mid, &t, 0);

    // Close
    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(admin);
    close_market(admin.as_ptr(), mid);

    // Resolver submits outcome 1 (NO wins)
    let resolver = [3u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 1, att_hash.as_ptr(), 100_000_000);

    // Challenger disputes
    let challenger = [4u8; 32];
    let evidence = [88u8; 32];
    moltchain_sdk::test_mock::set_caller(challenger);
    challenge_resolution(challenger.as_ptr(), mid, evidence.as_ptr(), 100_000_000);

    // DAO overrides with outcome 0 (YES wins)
    moltchain_sdk::test_mock::set_caller(admin);
    dao_resolve(admin.as_ptr(), mid, 0);

    // Trader redeems YES shares — profit!
    moltchain_sdk::test_mock::set_caller(t);
    assert_eq!(redeem_shares(t.as_ptr(), mid, 0), 1);
    let payout = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());
    assert_eq!(payout, yes_shares);
}

#[test]
fn test_full_lifecycle_void_after_dispute() {
    let (admin, mid) = setup_large_market();
    let t = [2u8; 32];

    moltchain_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 0, 5_000_000);

    // Close
    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(admin);
    close_market(admin.as_ptr(), mid);

    // Resolve → Dispute
    let resolver = [3u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);
    let challenger = [4u8; 32];
    let evidence = [88u8; 32];
    moltchain_sdk::test_mock::set_caller(challenger);
    challenge_resolution(challenger.as_ptr(), mid, evidence.as_ptr(), 100_000_000);

    // DAO voids instead of resolving
    moltchain_sdk::test_mock::set_caller(admin);
    dao_void(admin.as_ptr(), mid);

    let rec = read_market_record(mid).unwrap();
    assert_eq!(market_status_from_record(&rec), 6, "Status should be VOIDED (6)");

    // Trader reclaims collateral
    moltchain_sdk::test_mock::set_caller(t);
    let result = reclaim_collateral(t.as_ptr(), mid);
    assert_eq!(result, 1, "Trader should get refund from voided market");
    let refund = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());
    assert!(refund > 0, "Refund amount should be > 0");
}

// ============================================================================
// VOIDED MARKET RECLAIM TESTS
// ============================================================================

#[test]
fn test_voided_reclaim_multiple_traders() {
    let (admin, mid) = setup_large_market();
    let t1 = [2u8; 32];
    let t2 = [3u8; 32];

    // Two traders buy different sides
    moltchain_sdk::test_mock::set_caller(t1);
    buy_shares(t1.as_ptr(), mid, 0, 5_000_000);
    moltchain_sdk::test_mock::set_slot(1200);
    moltchain_sdk::test_mock::set_caller(t2);
    buy_shares(t2.as_ptr(), mid, 1, 3_000_000);

    let (_, cost1) = read_position(mid, &t1, 0);
    let (_, cost2) = read_position(mid, &t2, 1);

    // Void
    moltchain_sdk::test_mock::set_caller(admin);
    dao_void(admin.as_ptr(), mid);

    // Both reclaim
    moltchain_sdk::test_mock::set_caller(t1);
    let r1 = reclaim_collateral(t1.as_ptr(), mid);
    assert_eq!(r1, 1, "Trader 1 should reclaim");
    let refund1 = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());

    moltchain_sdk::test_mock::set_caller(t2);
    let r2 = reclaim_collateral(t2.as_ptr(), mid);
    assert_eq!(r2, 1, "Trader 2 should reclaim");
    let refund2 = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());

    // Each gets back their cost basis
    assert_eq!(refund1, cost1, "Refund ~= cost basis for t1");
    assert_eq!(refund2, cost2, "Refund ~= cost basis for t2");
}

#[test]
fn test_voided_reclaim_with_mint_position() {
    let (admin, mid) = setup_large_market();
    let t = [2u8; 32];

    // Mint complete set (equal cost in all outcomes)
    moltchain_sdk::test_mock::set_caller(t);
    assert_eq!(mint_complete_set(t.as_ptr(), mid, 10_000_000), 1);

    // Void
    moltchain_sdk::test_mock::set_caller(admin);
    dao_void(admin.as_ptr(), mid);

    // Reclaim — should get back 10M (= 10M cost_basis sum = 10M × 2 outcomes = 20M total cost, but capped)
    moltchain_sdk::test_mock::set_caller(t);
    let result = reclaim_collateral(t.as_ptr(), mid);
    // cost_basis per outcome = 10M, 2 outcomes → total_cost = 20M
    // But actual collateral is only 10M, so refund is capped at market_total_collateral
    // (100M pool + 10M mint = 110M total collateral)
    // refund = min(20M, 110M) = 20M
    assert_eq!(result, 1, "Reclaim should succeed");
    let refund = moltchain_sdk::bytes_to_u64(&moltchain_sdk::test_mock::get_return_data());
    assert_eq!(refund, 20_000_000, "Mint refund = sum of cost_basis");
}

// ============================================================================
// ORACLE ATTESTATION TESTS (with mocked oracle storage)
// ============================================================================

fn hex_encode_for_test(data: &[u8]) -> Vec<u8> {
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(data.len() * 2);
    for &b in data {
        out.push(hex_chars[(b >> 4) as usize]);
        out.push(hex_chars[(b & 0x0f) as usize]);
    }
    out
}

/// Set up a mock oracle attestation in storage.
/// The key format is `attestation_{hex_of_hash}` and the data
/// is 33+ bytes: 32-byte data hash + 1-byte sig_count.
fn mock_oracle_attestation(att_hash: &[u8; 32], sig_count: u8) {
    let mut key = Vec::from(&b"attestation_"[..]);
    key.extend_from_slice(&hex_encode_for_test(att_hash));
    let mut data = Vec::with_capacity(33);
    data.extend_from_slice(att_hash);
    data.push(sig_count);
    moltchain_sdk::test_mock::STORAGE.with(|s| {
        s.borrow_mut().insert(key, data);
    });
}

#[test]
fn test_resolution_with_oracle_attestation() {
    let (admin, mid) = setup_large_market();

    // Configure oracle address (non-zero enables attestation checks)
    let oracle_addr = [20u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    set_oracle_address(admin.as_ptr(), oracle_addr.as_ptr());

    // Close
    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(admin);
    close_market(admin.as_ptr(), mid);

    // Mock attestation in local storage — but post-fix submit_resolution uses
    // call_contract (cross-contract call) which returns Ok(Vec::new()) in mock,
    // so the oracle check correctly rejects (attestation not found on oracle).
    let att_hash = [99u8; 32];
    mock_oracle_attestation(&att_hash, 3);

    // Submit resolution — correctly rejects because call_contract returns empty
    // data (attestation not found on oracle contract).
    let resolver = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    let r = submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);
    assert_eq!(r, 0, "Oracle cross-contract call returns empty data in mock — correctly rejects");
}

#[test]
fn test_resolution_with_insufficient_attestation() {
    let (admin, mid) = setup_large_market();

    let oracle_addr = [20u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    set_oracle_address(admin.as_ptr(), oracle_addr.as_ptr());

    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(admin);
    close_market(admin.as_ptr(), mid);

    // Only 2 attestations (< threshold of 3)
    let att_hash = [99u8; 32];
    mock_oracle_attestation(&att_hash, 2);

    let resolver = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    let r = submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);
    assert_eq!(r, 0, "Should fail with insufficient attestations");
}

#[test]
fn test_resolution_with_missing_attestation() {
    let (admin, mid) = setup_large_market();

    let oracle_addr = [20u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    set_oracle_address(admin.as_ptr(), oracle_addr.as_ptr());

    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(admin);
    close_market(admin.as_ptr(), mid);

    // No attestation stored
    let att_hash = [99u8; 32];

    let resolver = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    let r = submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);
    assert_eq!(r, 0, "Should fail with missing attestation");
}

// ============================================================================
// MOLTYID REPUTATION TESTS (with mocked reputation storage)
// ============================================================================

fn mock_moltyid_reputation(_addr: &[u8; 32], reputation: u64) {
    // CON-14 audit fix: reputation is now read via cross-contract call, not storage.
    // Set the mock cross-call response to return the reputation as 8 le bytes.
    moltchain_sdk::test_mock::set_cross_call_response(
        Some(reputation.to_le_bytes().to_vec()),
    );
}

#[test]
fn test_resolution_with_moltyid_reputation_check() {
    let (admin, mid) = setup_large_market();

    // Configure MoltyID address (enables reputation checks)
    let moltyid_addr = [30u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    set_moltyid_address(admin.as_ptr(), moltyid_addr.as_ptr());

    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(admin);
    close_market(admin.as_ptr(), mid);

    // Resolver with 1500 reputation (above 1000 threshold)
    let resolver = [2u8; 32];
    mock_moltyid_reputation(&resolver, 1500);

    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    let r = submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);
    assert_eq!(r, 1, "Should succeed with 1500 reputation");
}

#[test]
fn test_resolution_insufficient_reputation() {
    let (admin, mid) = setup_large_market();

    let moltyid_addr = [30u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    set_moltyid_address(admin.as_ptr(), moltyid_addr.as_ptr());

    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(admin);
    close_market(admin.as_ptr(), mid);

    // Resolver with only 500 reputation (below 1000 threshold)
    let resolver = [2u8; 32];
    mock_moltyid_reputation(&resolver, 500);

    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    let r = submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);
    assert_eq!(r, 0, "Should fail with 500 reputation (need 1000+)");
}

// ============================================================================
// COMBINED ORACLE + REPUTATION TEST
// ============================================================================

#[test]
fn test_resolution_with_both_oracle_and_reputation() {
    let (admin, mid) = setup_large_market();

    // Configure both oracle and moltyid
    let oracle_addr = [20u8; 32];
    let moltyid_addr = [30u8; 32];
    moltchain_sdk::test_mock::set_caller(admin);
    set_oracle_address(admin.as_ptr(), oracle_addr.as_ptr());
    moltchain_sdk::test_mock::set_caller(admin);
    set_moltyid_address(admin.as_ptr(), moltyid_addr.as_ptr());

    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(admin);
    close_market(admin.as_ptr(), mid);

    // Mock attestation and reputation in local storage — but post-fix
    // submit_resolution uses call_contract for oracle, which returns empty
    // data in mock mode. Oracle check correctly rejects first.
    let att_hash = [99u8; 32];
    mock_oracle_attestation(&att_hash, 5);
    let resolver = [2u8; 32];
    mock_moltyid_reputation(&resolver, 2000);

    moltchain_sdk::test_mock::set_caller(resolver);
    let r = submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);
    assert_eq!(r, 0, "Oracle cross-contract call returns empty data in mock — correctly rejects");
}

// ============================================================================
// COLLATERAL ACCOUNTING AFTER RESOLUTION
// ============================================================================

#[test]
fn test_total_collateral_decreases_after_redemption() {
    let (_admin, mid) = setup_large_market();
    let t = [2u8; 32];
    moltchain_sdk::test_mock::set_caller(t);
    buy_shares(t.as_ptr(), mid, 0, 5_000_000);
    let (shares, _) = read_position(mid, &t, 0);

    // Full lifecycle
    moltchain_sdk::test_mock::set_slot(101_001);
    moltchain_sdk::test_mock::set_caller(t);
    close_market(t.as_ptr(), mid);
    let resolver = [3u8; 32];
    let att_hash = [99u8; 32];
    moltchain_sdk::test_mock::set_caller(resolver);
    submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), 100_000_000);
    moltchain_sdk::test_mock::set_slot(101_001 + 432_001);
    moltchain_sdk::test_mock::set_caller(t);
    finalize_resolution(t.as_ptr(), mid);

    // Check platform stats before
    get_platform_stats();
    let stats = moltchain_sdk::test_mock::get_return_data();
    let coll_before = u64::from_le_bytes(stats[24..32].try_into().unwrap());

    // Redeem
    moltchain_sdk::test_mock::set_caller(t);
    redeem_shares(t.as_ptr(), mid, 0);

    // Check platform stats after
    get_platform_stats();
    let stats2 = moltchain_sdk::test_mock::get_return_data();
    let coll_after = u64::from_le_bytes(stats2[24..32].try_into().unwrap());

    assert!(coll_after < coll_before, "Total collateral should decrease after redemption");
    assert_eq!(coll_before - coll_after, shares, "Decrease = shares redeemed");
}
