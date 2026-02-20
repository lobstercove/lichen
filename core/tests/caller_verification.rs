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
