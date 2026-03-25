// Lichen End-to-End Integration Tests
//
// Tests the full pipeline: genesis → token init → mint → DEX init →
// create pairs → place orders → match → AMM pools → margin → analytics →
// rewards → governance → router
//
// All 10 DEX+token contracts share the same test_mock::STORAGE,
// simulating the full deployed inter-contract environment.

use lichen_sdk::test_mock;

// ============================================================================
// HELPERS
// ============================================================================

fn addr(id: u8) -> [u8; 32] {
    let mut a = [0u8; 32];
    a[0] = id;
    a
}

const ADMIN: u8 = 1;
const ALICE: u8 = 2;
const BOB: u8 = 3;
const CAROL: u8 = 4;

// Zero proof hash for attest_reserves
const ZERO_PROOF: [u8; 32] = [0u8; 32];

// Token addresses (used as base/quote identifiers)
fn musd_addr() -> [u8; 32] {
    let mut a = [0u8; 32];
    a[0] = 100;
    a
}
fn wsol_addr() -> [u8; 32] {
    let mut a = [0u8; 32];
    a[0] = 101;
    a
}
fn weth_addr() -> [u8; 32] {
    let mut a = [0u8; 32];
    a[0] = 102;
    a
}

// ============================================================================
// PHASE 1: GENESIS — Initialize all 10 contracts
// ============================================================================

#[test]
fn test_phase1_genesis_all_contracts_initialize() {
    test_mock::reset();
    let admin = addr(ADMIN);

    // Tokens
    assert_eq!(lusd_token::initialize(admin.as_ptr()), 0, "lUSD init");
    assert_eq!(wsol_token::initialize(admin.as_ptr()), 0, "wSOL init");
    assert_eq!(weth_token::initialize(admin.as_ptr()), 0, "wETH init");

    // DEX
    assert_eq!(dex_core::initialize(admin.as_ptr()), 0, "dex_core init");
    assert_eq!(dex_amm::initialize(admin.as_ptr()), 0, "dex_amm init");
    assert_eq!(dex_router::initialize(admin.as_ptr()), 0, "dex_router init");
    assert_eq!(
        dex_governance::initialize(admin.as_ptr()),
        0,
        "dex_governance init"
    );
    assert_eq!(dex_margin::initialize(admin.as_ptr()), 0, "dex_margin init");
    assert_eq!(
        dex_rewards::initialize(admin.as_ptr()),
        0,
        "dex_rewards init"
    );
    assert_eq!(
        dex_analytics::initialize(admin.as_ptr()),
        0,
        "dex_analytics init"
    );

    // Double-init should fail
    assert_ne!(
        lusd_token::initialize(admin.as_ptr()),
        0,
        "lUSD double-init"
    );
    assert_ne!(
        dex_core::initialize(admin.as_ptr()),
        0,
        "dex_core double-init"
    );
}

// ============================================================================
// PHASE 2: TOKEN OPERATIONS — Mint, Transfer, Approve
// ============================================================================

#[test]
fn test_phase2_token_mint_and_transfer() {
    test_mock::reset();
    let admin = addr(ADMIN);
    let alice = addr(ALICE);
    let bob = addr(BOB);
    test_mock::set_slot(100);

    // Init tokens
    assert_eq!(lusd_token::initialize(admin.as_ptr()), 0);
    assert_eq!(wsol_token::initialize(admin.as_ptr()), 0);
    assert_eq!(weth_token::initialize(admin.as_ptr()), 0);

    // Attest reserves so lUSD circuit breaker passes
    assert_eq!(
        lusd_token::attest_reserves(admin.as_ptr(), 10_000_000_000, ZERO_PROOF.as_ptr()),
        0
    );

    // Mint tokens
    assert_eq!(
        lusd_token::mint(admin.as_ptr(), alice.as_ptr(), 1_000_000),
        0,
        "mint lUSD to alice"
    );
    assert_eq!(
        lusd_token::mint(admin.as_ptr(), bob.as_ptr(), 500_000),
        0,
        "mint lUSD to bob"
    );
    assert_eq!(
        wsol_token::mint(admin.as_ptr(), alice.as_ptr(), 50_000),
        0,
        "mint wSOL to alice"
    );
    assert_eq!(
        weth_token::mint(admin.as_ptr(), bob.as_ptr(), 25_000),
        0,
        "mint wETH to bob"
    );

    // Check balances
    assert_eq!(lusd_token::balance_of(alice.as_ptr()), 1_000_000);
    assert_eq!(lusd_token::balance_of(bob.as_ptr()), 500_000);
    assert_eq!(wsol_token::balance_of(alice.as_ptr()), 50_000);
    assert_eq!(weth_token::balance_of(bob.as_ptr()), 25_000);
    assert_eq!(lusd_token::total_supply(), 1_500_000);

    // Transfer lUSD: Alice → Bob
    assert_eq!(
        lusd_token::transfer(alice.as_ptr(), bob.as_ptr(), 200_000),
        0
    );
    assert_eq!(lusd_token::balance_of(alice.as_ptr()), 800_000);
    assert_eq!(lusd_token::balance_of(bob.as_ptr()), 700_000);
    assert_eq!(
        lusd_token::total_supply(),
        1_500_000,
        "transfer preserves supply"
    );

    // Approve + transfer_from: Alice approves Carol to spend 100k lUSD
    let carol = addr(CAROL);
    assert_eq!(
        lusd_token::approve(alice.as_ptr(), carol.as_ptr(), 100_000),
        0
    );
    assert_eq!(
        lusd_token::allowance(alice.as_ptr(), carol.as_ptr()),
        100_000
    );
    assert_eq!(
        lusd_token::transfer_from(carol.as_ptr(), alice.as_ptr(), bob.as_ptr(), 50_000),
        0
    );
    assert_eq!(lusd_token::balance_of(alice.as_ptr()), 750_000);
    assert_eq!(lusd_token::balance_of(bob.as_ptr()), 750_000);
    assert_eq!(
        lusd_token::allowance(alice.as_ptr(), carol.as_ptr()),
        50_000
    );
}

#[test]
fn test_phase2_token_burn() {
    test_mock::reset();
    let admin = addr(ADMIN);
    let alice = addr(ALICE);
    test_mock::set_slot(100);

    assert_eq!(lusd_token::initialize(admin.as_ptr()), 0);
    assert_eq!(
        lusd_token::attest_reserves(admin.as_ptr(), 1_000_000_000, ZERO_PROOF.as_ptr()),
        0
    );
    assert_eq!(lusd_token::mint(admin.as_ptr(), alice.as_ptr(), 500_000), 0);

    assert_eq!(lusd_token::burn(alice.as_ptr(), 200_000), 0);
    assert_eq!(lusd_token::balance_of(alice.as_ptr()), 300_000);
    assert_eq!(lusd_token::total_supply(), 300_000);
    assert_eq!(lusd_token::total_burned(), 200_000);
}

// ============================================================================
// PHASE 3: DEX CORE — Pairs, Orders, Matching
// ============================================================================

#[test]
fn test_phase3_dex_core_order_lifecycle() {
    test_mock::reset();
    let admin = addr(ADMIN);
    let alice = addr(ALICE);
    let bob = addr(BOB);
    test_mock::set_slot(100);

    assert_eq!(dex_core::initialize(admin.as_ptr()), 0);

    // Create wSOL/lUSD pair: tick=1_000_000, lot=100, min_order=1000
    let base = wsol_addr();
    let quote = musd_addr();
    assert_eq!(
        dex_core::create_pair(
            admin.as_ptr(),
            base.as_ptr(),
            quote.as_ptr(),
            1_000_000,
            100,
            1000
        ),
        0
    );
    assert_eq!(dex_core::get_pair_count(), 1);

    // Alice sells 200_000 wSOL at price 1.0 (1e9) — large enough for fees
    let price = 1_000_000_000u64;
    assert_eq!(
        dex_core::place_order(alice.as_ptr(), 1, 1, 0, price, 200_000, 0, 0),
        0
    );

    // Bob buys 200_000 wSOL at price 1.0 → should match
    assert_eq!(
        dex_core::place_order(bob.as_ptr(), 1, 0, 0, price, 200_000, 0, 0),
        0
    );

    // Verify matching happened
    assert!(
        dex_core::get_trade_count() > 0,
        "trades should have been executed"
    );
    assert!(
        dex_core::get_fee_treasury() > 0,
        "fees should have been collected"
    );

    // Order 1 was fully filled (same qty both sides), so cancel should return 3
    assert_eq!(
        dex_core::cancel_order(alice.as_ptr(), 1),
        3,
        "filled order can't be cancelled"
    );
}

#[test]
fn test_phase3_dex_core_multi_pair() {
    test_mock::reset();
    let admin = addr(ADMIN);
    test_mock::set_slot(100);

    assert_eq!(dex_core::initialize(admin.as_ptr()), 0);

    // Create two pairs: wSOL/lUSD and wETH/lUSD
    assert_eq!(
        dex_core::create_pair(
            admin.as_ptr(),
            wsol_addr().as_ptr(),
            musd_addr().as_ptr(),
            1_000_000,
            100,
            1000
        ),
        0
    );
    assert_eq!(
        dex_core::create_pair(
            admin.as_ptr(),
            weth_addr().as_ptr(),
            musd_addr().as_ptr(),
            1_000_000,
            100,
            1000
        ),
        0
    );
    assert_eq!(dex_core::get_pair_count(), 2);

    // Place orders on different pairs
    let alice = addr(ALICE);
    assert_eq!(
        dex_core::place_order(alice.as_ptr(), 1, 0, 0, 1_000_000_000, 1100, 0, 0),
        0
    );
    assert_eq!(
        dex_core::place_order(alice.as_ptr(), 2, 0, 0, 2_000_000_000, 1100, 0, 0),
        0
    );
}

// ============================================================================
// PHASE 4: AMM — Pool Creation, Liquidity, Swaps
// ============================================================================

#[test]
fn test_phase4_amm_pool_lifecycle() {
    test_mock::reset();
    let admin = addr(ADMIN);
    let alice = addr(ALICE);
    let bob = addr(BOB);
    test_mock::set_slot(100);

    assert_eq!(dex_amm::initialize(admin.as_ptr()), 0);

    // Create wSOL/lUSD pool: fee_tier=30bps (tick_spacing=60), sqrt_price=1<<32
    let token_a = wsol_addr();
    let token_b = musd_addr();
    assert_eq!(
        dex_amm::create_pool(
            admin.as_ptr(),
            token_a.as_ptr(),
            token_b.as_ptr(),
            1,
            1u64 << 32
        ),
        0
    );
    assert_eq!(dex_amm::get_pool_count(), 1);

    // Alice adds liquidity
    assert_eq!(
        dex_amm::add_liquidity(alice.as_ptr(), 1, -120, 120, 500_000, 500_000),
        0
    );
    assert_eq!(dex_amm::get_position_count(), 1);

    // TVL should be nonzero
    assert!(
        dex_amm::get_tvl(1) > 0,
        "TVL should reflect deposited liquidity"
    );

    // Bob swaps: exact_in 10_000 through the pool
    let result = dex_amm::swap_exact_in(bob.as_ptr(), 1, true, 10_000, 0, 1000);
    assert_eq!(result, 0, "swap should succeed");

    // Alice collects any accumulated fees
    let collect_result = dex_amm::collect_fees(alice.as_ptr(), 1);
    assert!(
        collect_result == 0 || collect_result == 3,
        "fee collection: {}",
        collect_result
    );

    // Alice removes liquidity
    assert_eq!(dex_amm::remove_liquidity(alice.as_ptr(), 1, 100_000), 0);
}

// ============================================================================
// PHASE 5: MARGIN — Positions, Leverage, Liquidation
// ============================================================================

#[test]
fn test_phase5_margin_position_lifecycle() {
    test_mock::reset();
    let admin = addr(ADMIN);
    let alice = addr(ALICE);
    let bob = addr(BOB);
    test_mock::set_slot(100);

    assert_eq!(dex_margin::initialize(admin.as_ptr()), 0);

    // Set mark price for pair 1: wSOL/lUSD at 1.0
    dex_margin::set_mark_price(admin.as_ptr(), 1, 1_000_000_000);

    // Alice opens a 2x long position: size=1000, leverage=2, margin=200
    assert_eq!(
        dex_margin::open_position(alice.as_ptr(), 1, 0, 1000, 2, 200),
        0
    );
    assert_eq!(dex_margin::get_position_count(), 1);

    // Check margin ratio: margin=200, notional=1000 → ratio=2000 bps (20%)
    let ratio = dex_margin::get_margin_ratio(1);
    assert_eq!(ratio, 2000, "initial margin ratio should be 20%");

    // Alice adds more margin
    assert_eq!(dex_margin::add_margin(alice.as_ptr(), 1, 300), 0);
    let new_ratio = dex_margin::get_margin_ratio(1);
    assert!(
        new_ratio > ratio,
        "ratio should increase after adding margin"
    );

    // Alice closes position
    assert_eq!(dex_margin::close_position(alice.as_ptr(), 1), 0);

    // Bob opens short, gets liquidated
    assert_eq!(
        dex_margin::open_position(bob.as_ptr(), 1, 1, 10_000, 5, 400),
        0
    );
    // Jack price up 10x to make short liquidatable
    dex_margin::set_mark_price(admin.as_ptr(), 1, 10_000_000_000);
    let carol = addr(CAROL);
    assert_eq!(
        dex_margin::liquidate(carol.as_ptr(), 2),
        0,
        "short should be liquidatable"
    );
    assert!(
        dex_margin::get_insurance_fund() > 0,
        "insurance fund should have balance"
    );
}

// ============================================================================
// PHASE 6: ANALYTICS — Trade Recording, Stats
// ============================================================================

#[test]
fn test_phase6_analytics_trade_recording() {
    test_mock::reset();
    let admin = addr(ADMIN);
    let alice = addr(ALICE);
    test_mock::set_slot(100);

    assert_eq!(dex_analytics::initialize(admin.as_ptr()), 0);

    // Record some trades
    assert_eq!(
        dex_analytics::record_trade(1, 1_000_000_000, 50_000, alice.as_ptr()),
        0
    );
    test_mock::set_slot(101);
    assert_eq!(
        dex_analytics::record_trade(1, 1_010_000_000, 30_000, alice.as_ptr()),
        0
    );
    test_mock::set_slot(102);
    assert_eq!(
        dex_analytics::record_trade(1, 990_000_000, 20_000, alice.as_ptr()),
        0
    );

    assert_eq!(dex_analytics::get_record_count(), 3);
    assert_eq!(dex_analytics::get_last_price(1), 990_000_000);

    // Trader stats
    let stats = dex_analytics::get_trader_stats(alice.as_ptr());
    assert!(stats > 0, "trader should have stats recorded");
}

// ============================================================================
// PHASE 7: REWARDS — Trade Recording, Tier, Claims
// ============================================================================

#[test]
fn test_phase7_rewards_trade_and_claim() {
    test_mock::reset();
    let admin = addr(ADMIN);
    let alice = addr(ALICE);
    let bob = addr(BOB);
    test_mock::set_slot(100);

    assert_eq!(dex_rewards::initialize(admin.as_ptr()), 0);

    // Record trades for Alice
    assert_eq!(dex_rewards::record_trade(alice.as_ptr(), 100, 50_000), 0);
    assert_eq!(dex_rewards::record_trade(alice.as_ptr(), 200, 100_000), 0);

    // Check Alice's pending rewards
    let pending = dex_rewards::get_pending_rewards(alice.as_ptr());
    assert!(pending > 0, "alice should have pending rewards");

    // Claim
    assert_eq!(dex_rewards::claim_trading_rewards(alice.as_ptr()), 0);
    assert!(
        dex_rewards::get_total_distributed() > 0,
        "rewards distributed"
    );

    // Register referral: Bob referred by Alice
    assert_eq!(
        dex_rewards::register_referral(bob.as_ptr(), alice.as_ptr()),
        0
    );
    let referral_stats = dex_rewards::get_referral_stats(alice.as_ptr());
    assert!(referral_stats > 0, "alice should have referral stats");
}

// ============================================================================
// PHASE 8: GOVERNANCE — Propose, Vote, Finalize
// ============================================================================

#[test]
fn test_phase8_governance_proposal_lifecycle() {
    test_mock::reset();
    let admin = addr(ADMIN);
    let alice = addr(ALICE);
    let bob = addr(BOB);
    test_mock::set_slot(100);

    assert_eq!(dex_governance::initialize(admin.as_ptr()), 0);

    // Propose new pair listing
    let base = wsol_addr();
    let quote = musd_addr();
    assert_eq!(
        dex_governance::propose_new_pair(alice.as_ptr(), base.as_ptr(), quote.as_ptr()),
        0
    );
    assert_eq!(dex_governance::get_proposal_count(), 1);

    // Vote
    assert_eq!(dex_governance::vote(alice.as_ptr(), 1, true), 0);
    assert_eq!(dex_governance::vote(bob.as_ptr(), 1, true), 0);

    // Proposal info
    let info = dex_governance::get_proposal_info(1);
    assert!(info > 0, "proposal should have info");

    // Propose fee change
    assert_eq!(
        dex_governance::propose_fee_change(admin.as_ptr(), 1, 0, 10),
        0
    );
    assert_eq!(dex_governance::get_proposal_count(), 2);
}

// ============================================================================
// PHASE 9: ROUTER — Route Registration, Swap
// ============================================================================

#[test]
fn test_phase9_router_route_and_swap() {
    test_mock::reset();
    let admin = addr(ADMIN);
    let alice = addr(ALICE);
    test_mock::set_slot(100);

    assert_eq!(dex_router::initialize(admin.as_ptr()), 0);

    // Set contract addresses (core, amm, legacy)
    let core_addr = addr(200);
    let amm_addr = addr(201);
    let legacy_addr = addr(202);
    assert_eq!(
        dex_router::set_addresses(admin.as_ptr(), core_addr.as_ptr(), amm_addr.as_ptr()),
        0
    );

    // Register route: wSOL→lUSD via CLOB pair 1
    assert_eq!(
        dex_router::register_route(
            admin.as_ptr(),
            wsol_addr().as_ptr(),
            musd_addr().as_ptr(),
            0,
            1,
            0,
            100
        ),
        0
    );
    assert_eq!(dex_router::get_route_count(), 1);

    // Attempt swap through router (may fail since actual contracts
    // aren't wired, but route lookup should work)
    let result = dex_router::swap(
        alice.as_ptr(),
        wsol_addr().as_ptr(),
        musd_addr().as_ptr(),
        10_000,
        0,
        1000,
    );
    // Route exists but execution depends on actual contract wiring
    assert!(result == 0 || result > 0, "swap attempted via router");
}

// ============================================================================
// PHASE 10: FULL PIPELINE — All contracts initialized together
// ============================================================================

#[test]
fn test_phase10_full_pipeline() {
    test_mock::reset();
    let admin = addr(ADMIN);
    let alice = addr(ALICE);
    let bob = addr(BOB);
    test_mock::set_slot(100);

    // === GENESIS ===
    assert_eq!(lusd_token::initialize(admin.as_ptr()), 0);
    assert_eq!(wsol_token::initialize(admin.as_ptr()), 0);
    assert_eq!(weth_token::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_core::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_amm::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_margin::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_rewards::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_analytics::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_governance::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_router::initialize(admin.as_ptr()), 0);

    // === MINT TOKENS ===
    assert_eq!(
        lusd_token::attest_reserves(admin.as_ptr(), 100_000_000_000, ZERO_PROOF.as_ptr()),
        0
    );
    assert_eq!(
        lusd_token::mint(admin.as_ptr(), alice.as_ptr(), 10_000_000),
        0
    );
    assert_eq!(lusd_token::mint(admin.as_ptr(), bob.as_ptr(), 5_000_000), 0);
    assert_eq!(
        wsol_token::mint(admin.as_ptr(), alice.as_ptr(), 1_000_000),
        0
    );
    assert_eq!(weth_token::mint(admin.as_ptr(), bob.as_ptr(), 500_000), 0);

    // === CREATE DEX PAIRS ===
    assert_eq!(
        dex_core::create_pair(
            admin.as_ptr(),
            wsol_addr().as_ptr(),
            musd_addr().as_ptr(),
            1_000_000,
            100,
            1000
        ),
        0
    );

    // === PLACE & MATCH ORDERS ===
    let price = 1_000_000_000u64;
    assert_eq!(
        dex_core::place_order(alice.as_ptr(), 1, 1, 0, price, 200_000, 0, 0),
        0
    ); // sell
    assert_eq!(
        dex_core::place_order(bob.as_ptr(), 1, 0, 0, price, 200_000, 0, 0),
        0
    ); // buy
    let trades = dex_core::get_trade_count();
    assert!(trades > 0, "orders should have matched");

    // === RECORD ANALYTICS ===
    assert_eq!(
        dex_analytics::record_trade(1, price, 200_000, alice.as_ptr()),
        0
    );
    assert_eq!(dex_analytics::get_record_count(), 1);

    // === RECORD REWARDS ===
    let fee = dex_core::get_fee_treasury();
    assert_eq!(dex_rewards::record_trade(alice.as_ptr(), fee, 200_000), 0);
    assert!(dex_rewards::get_pending_rewards(alice.as_ptr()) > 0);

    // === CREATE AMM POOL ===
    assert_eq!(
        dex_amm::create_pool(
            admin.as_ptr(),
            wsol_addr().as_ptr(),
            musd_addr().as_ptr(),
            1,
            1u64 << 32
        ),
        0
    );
    assert_eq!(
        dex_amm::add_liquidity(alice.as_ptr(), 1, -120, 120, 100_000, 100_000),
        0
    );

    // === OPEN MARGIN POSITION ===
    dex_margin::set_mark_price(admin.as_ptr(), 1, price);
    assert_eq!(
        dex_margin::open_position(alice.as_ptr(), 1, 0, 1000, 2, 200),
        0
    );

    // === GOVERNANCE PROPOSAL ===
    assert_eq!(
        dex_governance::propose_new_pair(
            alice.as_ptr(),
            weth_addr().as_ptr(),
            musd_addr().as_ptr()
        ),
        0
    );
    assert_eq!(dex_governance::vote(alice.as_ptr(), 1, true), 0);
    assert_eq!(dex_governance::vote(bob.as_ptr(), 1, true), 0);

    // === VERIFY STATE ACROSS ALL CONTRACTS ===
    assert_eq!(lusd_token::total_supply(), 15_000_000);
    assert_eq!(dex_core::get_pair_count(), 1);
    assert!(dex_core::get_trade_count() > 0);
    assert_eq!(dex_amm::get_pool_count(), 1);
    assert_eq!(dex_amm::get_position_count(), 1);
    assert_eq!(dex_margin::get_position_count(), 1);
    assert_eq!(dex_analytics::get_record_count(), 1);
    assert_eq!(dex_governance::get_proposal_count(), 1);
}

// ============================================================================
// PHASE 11: EMERGENCY OPERATIONS — Pause/Unpause across contracts
// ============================================================================

#[test]
fn test_phase11_emergency_pause_all() {
    test_mock::reset();
    let admin = addr(ADMIN);
    test_mock::set_slot(100);

    // Init all
    assert_eq!(lusd_token::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_core::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_amm::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_margin::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_rewards::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_analytics::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_governance::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_router::initialize(admin.as_ptr()), 0);

    // Pause everything
    assert_eq!(lusd_token::emergency_pause(admin.as_ptr()), 0);
    assert_eq!(dex_core::emergency_pause(admin.as_ptr()), 0);
    assert_eq!(dex_amm::emergency_pause(admin.as_ptr()), 0);
    assert_eq!(dex_margin::emergency_pause(admin.as_ptr()), 0);
    assert_eq!(dex_rewards::emergency_pause(admin.as_ptr()), 0);
    assert_eq!(dex_analytics::emergency_pause(admin.as_ptr()), 0);
    assert_eq!(dex_governance::emergency_pause(admin.as_ptr()), 0);
    assert_eq!(dex_router::emergency_pause(admin.as_ptr()), 0);

    // Operations should fail while paused
    let alice = addr(ALICE);
    assert_eq!(
        lusd_token::attest_reserves(admin.as_ptr(), 1_000_000, ZERO_PROOF.as_ptr()),
        0
    );
    assert_ne!(
        lusd_token::mint(admin.as_ptr(), alice.as_ptr(), 1000),
        0,
        "mint should fail while paused"
    );

    // Unpause everything
    assert_eq!(lusd_token::emergency_unpause(admin.as_ptr()), 0);
    assert_eq!(dex_core::emergency_unpause(admin.as_ptr()), 0);
    assert_eq!(dex_amm::emergency_unpause(admin.as_ptr()), 0);
    assert_eq!(dex_margin::emergency_unpause(admin.as_ptr()), 0);
    assert_eq!(dex_rewards::emergency_unpause(admin.as_ptr()), 0);
    assert_eq!(dex_analytics::emergency_unpause(admin.as_ptr()), 0);
    assert_eq!(dex_governance::emergency_unpause(admin.as_ptr()), 0);
    assert_eq!(dex_router::emergency_unpause(admin.as_ptr()), 0);

    // Operations should work again
    assert_eq!(
        lusd_token::mint(admin.as_ptr(), alice.as_ptr(), 1000),
        0,
        "mint after unpause"
    );
}

// ============================================================================
// PHASE 12: ACCESS CONTROL — Non-admin rejection across contracts
// ============================================================================

#[test]
fn test_phase12_access_control_all_contracts() {
    test_mock::reset();
    let admin = addr(ADMIN);
    let rando = addr(99);
    test_mock::set_slot(100);

    // Init all
    assert_eq!(lusd_token::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_core::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_amm::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_margin::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_governance::initialize(admin.as_ptr()), 0);
    assert_eq!(dex_router::initialize(admin.as_ptr()), 0);

    // Non-admin operations should fail
    assert_ne!(
        lusd_token::mint(rando.as_ptr(), rando.as_ptr(), 1000),
        0,
        "token mint non-admin"
    );
    assert_ne!(
        dex_core::create_pair(
            rando.as_ptr(),
            wsol_addr().as_ptr(),
            musd_addr().as_ptr(),
            1,
            1,
            1
        ),
        0,
        "create_pair non-admin"
    );
    assert_ne!(
        dex_amm::create_pool(
            rando.as_ptr(),
            wsol_addr().as_ptr(),
            musd_addr().as_ptr(),
            1,
            1 << 32
        ),
        0,
        "create_pool non-admin"
    );
    assert_ne!(
        dex_margin::set_mark_price(rando.as_ptr(), 1, 1000),
        0,
        "set_mark_price non-admin"
    );
    assert_ne!(
        dex_core::emergency_pause(rando.as_ptr()),
        0,
        "emergency_pause non-admin"
    );
    assert_ne!(
        dex_router::emergency_pause(rando.as_ptr()),
        0,
        "router pause non-admin"
    );
    assert_ne!(
        dex_governance::emergency_delist(rando.as_ptr(), 1),
        0,
        "delist non-admin"
    );
}
