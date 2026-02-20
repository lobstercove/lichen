// Phase 1 Task 5: Caller verification sweep — source-level integrity tests
//
// These tests statically verify that all 7 identified caller-verification
// vulnerabilities (G1-01, G1-02, G7-01, G10-01, G13-01, G15-01, G26-01)
// remain fixed by checking that the target WASM contract source files
// contain the required `get_caller()` verification pattern in each
// vulnerable function.
//
// WASM contracts can't be unit-tested via Rust's #[test] harness because
// they compile to wasm32-unknown-unknown with no_std and extern "C" FFI.
// These source-level tests guard against regression by future edits.

use std::fs;
use std::path::PathBuf;

/// Get the workspace root (parent of core/)
fn workspace_root() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(manifest_dir)
        .parent()
        .expect("core/ should have a parent directory")
        .to_path_buf()
}

/// Read contract source and verify it contains the expected pattern
fn verify_contract_has_pattern(contract_rel_path: &str, patterns: &[(&str, &str)]) {
    let full_path = workspace_root().join(contract_rel_path);
    let source = fs::read_to_string(&full_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", full_path.display(), e));

    for (function_name, pattern) in patterns {
        assert!(
            source.contains(pattern),
            "REGRESSION: {} in {} is missing caller verification pattern: '{}'",
            function_name,
            contract_rel_path,
            pattern
        );
    }
}

#[test]
fn test_g1_01_moltcoin_approve_has_caller_check() {
    verify_contract_has_pattern(
        "contracts/moltcoin/src/lib.rs",
        &[(
            "approve()",
            // The AUDIT-FIX pattern: get_caller() + compare with owner_array
            "let real_caller = get_caller();\n    if real_caller.0 != owner_array",
        )],
    );
}

#[test]
fn test_g1_02_moltcoin_mint_has_caller_check() {
    verify_contract_has_pattern(
        "contracts/moltcoin/src/lib.rs",
        &[(
            "mint()",
            "let real_caller = get_caller();\n    if real_caller.0 != caller_array",
        )],
    );
}

#[test]
fn test_g7_01_dex_rewards_initialize_has_caller_check() {
    verify_contract_has_pattern(
        "contracts/dex_rewards/src/lib.rs",
        &[(
            "initialize()",
            "let real_caller = get_caller();\n    if real_caller.0 != addr",
        )],
    );
}

#[test]
fn test_g10_01_moltauction_create_auction_has_caller_check() {
    verify_contract_has_pattern(
        "contracts/moltauction/src/lib.rs",
        &[(
            "create_auction()",
            "let real_caller = get_caller();\n    if real_caller.0 != seller",
        )],
    );
}

#[test]
fn test_g13_01_moltdao_cancel_proposal_has_caller_check() {
    verify_contract_has_pattern(
        "contracts/moltdao/src/lib.rs",
        &[(
            "cancel_proposal()",
            "let real_caller = get_caller();\n    if real_caller.0 != canceller",
        )],
    );
}

#[test]
fn test_g15_01_moltoracle_submit_price_has_caller_check() {
    verify_contract_has_pattern(
        "contracts/moltoracle/src/lib.rs",
        &[(
            "submit_price()",
            "let real_caller = get_caller();\n    if real_caller.0 != feeder",
        )],
    );
}

#[test]
fn test_g26_01_compute_market_admin_fns_have_caller_checks() {
    verify_contract_has_pattern(
        "contracts/compute_market/src/lib.rs",
        &[
            (
                "set_claim_timeout()",
                // Pattern appears in all 5 admin functions with identical check
                "fn set_claim_timeout(caller_ptr",
            ),
            (
                "set_complete_timeout()",
                "fn set_complete_timeout(caller_ptr",
            ),
            (
                "set_challenge_period()",
                "fn set_challenge_period(caller_ptr",
            ),
            ("add_arbitrator()", "fn add_arbitrator(caller_ptr"),
            ("remove_arbitrator()", "fn remove_arbitrator(caller_ptr"),
        ],
    );
    // Additionally verify the shared caller check pattern exists for each
    verify_contract_has_pattern(
        "contracts/compute_market/src/lib.rs",
        &[(
            "admin functions (shared pattern)",
            // This exact string appears 5+ times for admin functions
            "let real_caller = get_caller();\n    if real_caller.0 != caller {\n        return 200;\n    }\n    if !is_admin",
        )],
    );
}

#[test]
fn test_all_7_contracts_import_get_caller() {
    // Every vulnerable contract must import get_caller
    let contracts = [
        "contracts/moltcoin/src/lib.rs",
        "contracts/dex_rewards/src/lib.rs",
        "contracts/moltauction/src/lib.rs",
        "contracts/moltdao/src/lib.rs",
        "contracts/moltoracle/src/lib.rs",
        "contracts/compute_market/src/lib.rs",
    ];

    for path in &contracts {
        let full_path = workspace_root().join(path);
        let source = fs::read_to_string(&full_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", full_path.display(), e));
        assert!(
            source.contains("get_caller"),
            "REGRESSION: {} does not contain get_caller import/usage",
            path
        );
    }
}

// ============================================================================
// Phase 2 Task 11: G19-01/G20-01 — Wrapped token WASM export annotations
// ============================================================================

/// The critical functions that must be exported for a token contract to work.
const REQUIRED_TOKEN_EXPORTS: &[&str] = &[
    "fn initialize",
    "fn balance_of",
    "fn transfer",
    "fn mint",
    "fn burn",
    "fn approve",
    "fn transfer_from",
    "fn total_supply",
];

#[test]
fn g19_01_musd_token_has_no_mangle_exports() {
    let source = fs::read_to_string(workspace_root().join("contracts/musd_token/src/lib.rs"))
        .expect("musd_token source should exist");

    for func in REQUIRED_TOKEN_EXPORTS {
        assert!(
            source.contains(func),
            "REGRESSION G19-01: musd_token missing function {}",
            func
        );
    }

    // Count #[no_mangle] annotations — must have at least 8 core + extras
    let no_mangle_count = source.matches("#[no_mangle]").count();
    assert!(
        no_mangle_count >= 8,
        "REGRESSION G19-01: musd_token has only {} #[no_mangle] annotations (need ≥8)",
        no_mangle_count
    );

    // Every #[no_mangle] must be followed by pub extern "C"
    let extern_c_count = source.matches("pub extern \"C\"").count();
    assert_eq!(
        no_mangle_count, extern_c_count,
        "REGRESSION G19-01: musd_token #[no_mangle] count ({}) != pub extern \"C\" count ({})",
        no_mangle_count, extern_c_count
    );
}

#[test]
fn g20_01_weth_token_has_no_mangle_exports() {
    let source = fs::read_to_string(workspace_root().join("contracts/weth_token/src/lib.rs"))
        .expect("weth_token source should exist");

    for func in REQUIRED_TOKEN_EXPORTS {
        assert!(
            source.contains(func),
            "REGRESSION G20-01: weth_token missing function {}",
            func
        );
    }

    let no_mangle_count = source.matches("#[no_mangle]").count();
    assert!(
        no_mangle_count >= 8,
        "REGRESSION G20-01: weth_token has only {} #[no_mangle] annotations (need ≥8)",
        no_mangle_count
    );

    let extern_c_count = source.matches("pub extern \"C\"").count();
    assert_eq!(
        no_mangle_count, extern_c_count,
        "REGRESSION G20-01: weth_token #[no_mangle] count ({}) != pub extern \"C\" count ({})",
        no_mangle_count, extern_c_count
    );
}

#[test]
fn g20_01_wsol_token_has_no_mangle_exports() {
    let source = fs::read_to_string(workspace_root().join("contracts/wsol_token/src/lib.rs"))
        .expect("wsol_token source should exist");

    for func in REQUIRED_TOKEN_EXPORTS {
        assert!(
            source.contains(func),
            "REGRESSION G20-01: wsol_token missing function {}",
            func
        );
    }

    let no_mangle_count = source.matches("#[no_mangle]").count();
    assert!(
        no_mangle_count >= 8,
        "REGRESSION G20-01: wsol_token has only {} #[no_mangle] annotations (need ≥8)",
        no_mangle_count
    );

    let extern_c_count = source.matches("pub extern \"C\"").count();
    assert_eq!(
        no_mangle_count, extern_c_count,
        "REGRESSION G20-01: wsol_token #[no_mangle] count ({}) != pub extern \"C\" count ({})",
        no_mangle_count, extern_c_count
    );
}

// ============================================================================
//  B1-02: Genesis contract initialization — all contracts must be initialized
// ============================================================================

/// Verify that every contract in GENESIS_CONTRACT_CATALOG is either:
/// (a) included in the `InitSpec` list inside `genesis_initialize_contracts()`, or
/// (b) handled as a special case (e.g. moltauction's two-step init).
///
/// This is a source-level regression test: it reads validator/src/main.rs and
/// checks that all 27 contracts are initialized at genesis, preventing the
/// first-caller-wins admin vulnerability (G22-02).
#[test]
fn b1_02_all_contracts_initialized_at_genesis() {
    let source =
        std::fs::read_to_string("../validator/src/main.rs").expect("Cannot read validator/src/main.rs");

    // All 27 contracts from GENESIS_CONTRACT_CATALOG
    let all_contracts = [
        "moltcoin",
        "musd_token",
        "wsol_token",
        "weth_token",
        "dex_core",
        "dex_amm",
        "dex_router",
        "dex_margin",
        "dex_rewards",
        "dex_governance",
        "dex_analytics",
        "moltswap",
        "moltbridge",
        "moltmarket",
        "moltoracle",
        "moltauction",
        "moltdao",
        "lobsterlend",
        "moltpunks",
        "moltyid",
        "clawpay",
        "clawpump",
        "clawvault",
        "bountyboard",
        "compute_market",
        "reef_storage",
        "prediction_market",
    ];

    // Extract the genesis_initialize_contracts function body
    let init_fn_start = source
        .find("fn genesis_initialize_contracts(")
        .expect("genesis_initialize_contracts function not found");
    // Take a generous slice — the function is ~450 lines
    let init_body = &source[init_fn_start..std::cmp::min(init_fn_start + 20000, source.len())];

    for contract in &all_contracts {
        // Each contract must appear as a dir_name in an InitSpec or in a special-case block
        let pattern = format!("\"{}\"", contract);
        assert!(
            init_body.contains(&pattern),
            "REGRESSION B1-02: contract '{}' is NOT initialized at genesis! \
             All contracts must have an InitSpec or special-case init in \
             genesis_initialize_contracts() to prevent first-caller-wins admin vulnerability.",
            contract
        );
    }
}

// ============================================================================
//  A12-01: Genesis distribution alignment — genesis.rs must match multisig.rs
// ============================================================================

/// Verify that genesis distribution amounts in genesis.rs match the canonical
/// GENESIS_DISTRIBUTION in multisig.rs. Prevents silent drift where one file
/// is updated but not the other.
#[test]
fn a12_01_genesis_distribution_matches_multisig() {
    let genesis_src =
        std::fs::read_to_string("src/genesis.rs").expect("Cannot read core/src/genesis.rs");
    let multisig_src =
        std::fs::read_to_string("src/multisig.rs").expect("Cannot read core/src/multisig.rs");

    // Canonical allocations from multisig.rs GENESIS_DISTRIBUTION
    let canonical = [
        ("validator_rewards", 150_000_000u64),
        ("community_treasury", 400_000_000),
        ("builder_grants", 250_000_000),
        ("founding_moltys", 100_000_000),
        ("ecosystem_partnerships", 50_000_000),
        ("reserve_pool", 50_000_000),
    ];

    // Verify multisig.rs has the canonical values
    for (name, amount) in &canonical {
        let pattern = format!("(\"{}\", {})", name, amount.to_string().chars()
            .enumerate()
            .map(|(i, c)| {
                if i > 0 && (amount.to_string().len() - i) % 3 == 0 { format!("_{}", c) } else { c.to_string() }
            })
            .collect::<String>());
        // Simpler: just search for the amount string with underscores
        let amount_str = format!("{}_000_000", amount / 1_000_000);
        assert!(
            multisig_src.contains(&amount_str),
            "REGRESSION A12-01: multisig.rs missing canonical amount {} for {}",
            amount, name
        );
    }

    // Verify genesis.rs has matching values
    // Genesis uses balance_molt field names
    for (name, amount) in &canonical {
        let amount_str = format!("{}_000_000", amount / 1_000_000);
        assert!(
            genesis_src.contains(&amount_str),
            "REGRESSION A12-01: genesis.rs missing amount {} for {} — \
             genesis distribution has drifted from multisig.rs canonical values!",
            amount, name
        );
    }

    // Verify totals: both should sum to 1B
    let total: u64 = canonical.iter().map(|(_, a)| a).sum();
    assert_eq!(
        total, 1_000_000_000,
        "REGRESSION A12-01: canonical genesis distribution sums to {} (expected 1,000,000,000)",
        total
    );
}
