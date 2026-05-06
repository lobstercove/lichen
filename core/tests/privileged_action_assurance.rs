use lichen_core::multisig::{GovernedTransferVelocityPolicy, GovernedTransferVelocityTier};
use lichen_core::{GovernanceAction, GovernanceProposal, Pubkey};
use proptest::prelude::*;
use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(manifest_dir)
        .parent()
        .expect("core/ should have a parent directory")
        .to_path_buf()
}

fn read_workspace_file(relative_path: &str) -> String {
    let full_path = workspace_root().join(relative_path);
    fs::read_to_string(&full_path)
        .unwrap_or_else(|error| panic!("Failed to read {}: {}", full_path.display(), error))
}

#[test]
fn privileged_processor_regressions_remain_in_suite() {
    let source = read_workspace_file("core/src/processor.rs");

    for test_name in [
        "test_governance_param_change_via_governed_authority_proposal_flow",
        "test_upgrade_timelock_set_and_stage",
        "test_veto_upgrade_rejects_general_governance_authority_when_split_is_configured",
        "test_allowlisted_emergency_pause_contract_call_stays_timelocked_on_governance_authority",
        "test_bridge_committee_admin_contract_call_rejects_governance_authority_direct_path",
        "test_governed_transfer_velocity_policy_snapshots_escalation",
        "test_restriction_governance_actions_do_not_match_legacy_split_role_policies",
        "test_restriction_governance_split_roles_route_scoped_creates",
        "test_governed_wallet_direct_transfer_still_requires_proposal_when_restricted",
        "test_governed_transfer_source_restriction_blocks_execution_without_losing_proposal",
        "test_governed_transfer_recipient_restriction_blocks_execution_without_losing_proposal",
        "test_nonce_withdraw_authority_restriction_blocks_value_exit",
        "test_nonce_withdraw_authority_native_frozen_amount_blocks_value_exit",
        "test_nonce_withdraw_nonce_account_restriction_blocks_value_exit",
        "test_nonce_withdraw_recipient_restriction_blocks_without_closing_nonce",
        "test_stake_rejects_outgoing_restricted_staker_without_pool_mutation",
        "test_stake_rejects_native_frozen_amount_without_pool_mutation",
        "test_staking_protocol_pause_rejects_stake_without_pool_mutation",
        "test_request_unstake_rejects_outgoing_restricted_staker_without_request",
        "test_claim_unstake_rejects_incoming_restricted_staker_without_unlocking",
        "test_mossstake_deposit_rejects_outgoing_restricted_depositor",
        "test_mossstake_deposit_rejects_native_frozen_amount",
        "test_mossstake_protocol_pause_rejects_deposit_without_position",
        "test_mossstake_unstake_rejects_outgoing_restricted_position_owner",
        "test_mossstake_claim_rejects_incoming_restricted_user_without_dropping_request",
        "test_mossstake_transfer_rejects_outgoing_restricted_sender",
        "test_mossstake_transfer_rejects_incoming_restricted_recipient",
        "test_register_validator_rejects_treasury_outgoing_restriction_without_grant",
        "test_register_validator_rejects_treasury_native_frozen_amount_without_grant",
        "test_register_validator_rejects_incoming_restricted_validator_without_grant",
        "test_register_validator_protocol_pause_rejects_without_grant",
        "test_deregister_validator_protocol_pause_rejects_without_deactivation",
        "test_shield_rejects_outgoing_restricted_sender_without_pool_mutation",
        "test_shield_rejects_native_frozen_amount_without_pool_mutation",
        "test_shield_protocol_pause_rejects_deposit_without_pool_mutation",
        "test_unshield_rejects_incoming_restricted_recipient_without_spending_nullifier",
        "test_unshield_rejects_native_incoming_restricted_recipient_without_spending_nullifier",
        "test_shielded_transfer_protocol_pause_rejects_before_nullifier_mutation",
        "test_fee_debit_bypasses_account_and_native_asset_restrictions",
        "test_restricted_transfer_failure_keeps_only_fee_debit",
        "test_restricted_governance_authority_can_pay_fee_for_lift_remediation",
        "test_failed_premium_fee_refund_bypasses_incoming_restriction",
        "test_contract_lifecycle_active_allows_state_changing_wasm_execution",
        "test_contract_lifecycle_suspended_rejects_state_changing_wasm_before_execution",
        "test_contract_lifecycle_quarantined_rejects_wasm_before_execution",
        "test_contract_lifecycle_terminated_rejects_wasm_before_execution",
        "test_contract_lifecycle_simulation_rejects_blocked_contract_before_execution",
        "restriction_governance_contract_suspend_and_lift_drive_lifecycle_without_owner_spoofing",
        "restriction_governance_contract_temporary_restriction_expires_and_resumes_on_next_call",
        "restriction_governance_contract_termination_is_permanent_and_preserves_state",
        "test_contract_close_owner_semantics_preserved_for_non_active_lifecycle_statuses",
        "test_contract_close_non_owner_still_rejected_for_non_active_lifecycle_contract",
        "test_contract_close_requires_governance_proposal_when_owner_is_governed",
    ] {
        assert!(
            source.contains(&format!("fn {}", test_name)),
            "REGRESSION: core/src/processor.rs must keep privileged-action regression {}",
            test_name
        );
    }
}

#[test]
fn caller_verification_regressions_remain_in_suite() {
    let source = read_workspace_file("core/tests/caller_verification.rs");

    for test_name in [
        "test_g7_01_dex_rewards_initialize_has_caller_check",
        "test_g10_01_lichenauction_create_auction_has_caller_check",
        "test_g13_01_lichendao_cancel_proposal_has_caller_check",
        "test_g15_01_lichenoracle_submit_price_has_caller_check",
        "test_g26_01_compute_market_admin_fns_have_caller_checks",
        "b1_03_genesis_initialization_uses_governance_authority",
        "b1_05_genesis_oracle_seeding_uses_governance_authority",
    ] {
        assert!(
            source.contains(&format!("fn {}", test_name)),
            "REGRESSION: core/tests/caller_verification.rs must keep {}",
            test_name
        );
    }
}

#[test]
fn contract_proxy_bypass_regressions_remain_in_suite() {
    let source = read_workspace_file("core/tests/cross_contract_call.rs");

    for test_name in [
        "test_cross_contract_call_rejects_suspended_target_before_callee_mutation",
        "test_cross_contract_call_derives_target_lifecycle_from_active_restriction",
        "test_scam_contract_proxy_forwarder_cannot_bypass_target_lifecycle_restrictions",
        "test_scam_proxy_contract_restriction_blocks_forwarded_target_mutation",
    ] {
        assert!(
            source.contains(&format!("fn {}", test_name)),
            "REGRESSION: core/tests/cross_contract_call.rs must keep contract proxy bypass regression {}",
            test_name
        );
    }
}

proptest! {
    #[test]
    fn governance_proposal_approval_authority_prefers_split_authority(
        authority in any::<[u8; 32]>(),
        proposer in any::<[u8; 32]>(),
        split_authority in proptest::option::of(any::<[u8; 32]>()),
    ) {
        let authority = Pubkey(authority);
        let proposer = Pubkey(proposer);
        let split_authority = split_authority.map(Pubkey);
        let expected = split_authority.unwrap_or(authority);

        let proposal = GovernanceProposal {
            id: 7,
            authority,
            approval_authority: split_authority,
            proposer,
            action: GovernanceAction::ParamChange {
                param_id: 1,
                value: 42,
            },
            action_label: "governance_param_change".to_string(),
            metadata: String::new(),
            approvals: Vec::new(),
            threshold: 1,
            execute_after_epoch: 0,
            velocity_tier: GovernedTransferVelocityTier::Standard,
            daily_cap_spores: 0,
            executed: false,
            cancelled: false,
        };

        prop_assert_eq!(proposal.approval_authority(), expected);
    }

    #[test]
    fn governance_proposal_readiness_requires_quorum_epoch_and_live_status(
        authority in any::<[u8; 32]>(),
        proposer in any::<[u8; 32]>(),
        approval_count in 0usize..6,
        threshold in 1u8..6,
        current_epoch in 0u64..20,
        execute_after_epoch in 0u64..20,
        executed in any::<bool>(),
        cancelled in any::<bool>(),
    ) {
        let approvals = (0..approval_count)
            .map(|index| Pubkey([index as u8; 32]))
            .collect();
        let proposal = GovernanceProposal {
            id: 11,
            authority: Pubkey(authority),
            approval_authority: None,
            proposer: Pubkey(proposer),
            action: GovernanceAction::ExecuteContractUpgrade {
                contract: Pubkey([9u8; 32]),
            },
            action_label: "execute_contract_upgrade".to_string(),
            metadata: String::new(),
            approvals,
            threshold,
            execute_after_epoch,
            velocity_tier: GovernedTransferVelocityTier::Standard,
            daily_cap_spores: 0,
            executed,
            cancelled,
        };

        let expected = !executed
            && !cancelled
            && approval_count >= threshold as usize
            && current_epoch >= execute_after_epoch;

        prop_assert_eq!(proposal.is_ready(current_epoch), expected);
    }

    #[test]
    fn governed_transfer_velocity_policy_never_downgrades_threshold_or_timelock(
        per_transfer_cap_spores in 1u64..1_000_000,
        daily_cap_spores in 1u64..1_000_000,
        elevated_threshold_spores in 1u64..1_000_000,
        extraordinary_threshold_spores in 1u64..1_000_000,
        elevated_additional_timelock_epochs in 0u32..8,
        extraordinary_additional_timelock_epochs in 0u32..8,
        base_threshold in 0u8..16,
        signer_count in 0usize..16,
    ) {
        let policy = GovernedTransferVelocityPolicy::new(
            per_transfer_cap_spores,
            daily_cap_spores,
            elevated_threshold_spores,
            extraordinary_threshold_spores,
            elevated_additional_timelock_epochs,
            extraordinary_additional_timelock_epochs.max(elevated_additional_timelock_epochs),
        );

        let standard_threshold = policy.required_threshold(
            base_threshold,
            signer_count,
            GovernedTransferVelocityTier::Standard,
        );
        let elevated_threshold = policy.required_threshold(
            base_threshold,
            signer_count,
            GovernedTransferVelocityTier::Elevated,
        );
        let extraordinary_threshold = policy.required_threshold(
            base_threshold,
            signer_count,
            GovernedTransferVelocityTier::Extraordinary,
        );

        let max_threshold = u8::try_from(signer_count).unwrap_or(u8::MAX);
        let capped_base_threshold = base_threshold.min(max_threshold);

        prop_assert_eq!(standard_threshold, capped_base_threshold);
        prop_assert!(standard_threshold <= elevated_threshold);
        prop_assert!(elevated_threshold <= extraordinary_threshold);
        prop_assert_eq!(extraordinary_threshold, max_threshold);
        prop_assert_eq!(
            policy.additional_timelock_epochs(GovernedTransferVelocityTier::Standard),
            0
        );
        prop_assert!(
            policy.additional_timelock_epochs(GovernedTransferVelocityTier::Elevated)
                <= policy.additional_timelock_epochs(GovernedTransferVelocityTier::Extraordinary)
        );
    }
}
