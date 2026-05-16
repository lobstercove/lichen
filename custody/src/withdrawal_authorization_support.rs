use super::*;

mod evm;
mod pq;
mod solana;

#[allow(unused_imports)]
pub(super) use self::evm::EvmSafeTransactionPlan;
#[cfg(test)]
pub(super) use self::evm::{build_evm_safe_exec_transaction_calldata, evm_function_selector};
pub(super) use self::evm::{
    build_evm_safe_transaction_plan, collect_threshold_evm_withdrawal_signatures,
    evm_executor_derivation_path, finalize_evm_safe_exec_plan, normalize_evm_signature,
};
pub(super) use self::pq::{collect_pq_withdrawal_approvals, valid_pq_withdrawal_approvers};
#[cfg(test)]
pub(super) use self::solana::build_threshold_solana_withdrawal_message;
pub(super) use self::solana::{
    build_solana_token_transfer_message, collect_threshold_solana_withdrawal_signatures,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WithdrawalSigningMode {
    PqApprovalQuorum,
    EvmThresholdSafe,
}

pub(super) fn determine_withdrawal_signing_mode(
    state: &CustodyState,
    job: &WithdrawalJob,
    outbound_asset: &str,
) -> Result<Option<WithdrawalSigningMode>, String> {
    validate_pq_signer_configuration(&state.config)?;

    if state.config.signer_endpoints.is_empty() || state.config.signer_threshold == 0 {
        return Ok(None);
    }

    match job.dest_chain.as_str() {
        "solana" | "sol" => {
            if state.config.signer_endpoints.len() > 1 {
                return Err(
                    "threshold Solana withdrawals are disabled until custody has a real threshold executor; PQ approval quorum plus local treasury signing is banned".to_string(),
                );
            }
            if outbound_asset != "sol" && !is_solana_stablecoin(outbound_asset) {
                return Err(format!(
                    "threshold Solana withdrawals currently support native SOL and SPL stablecoins, not {}",
                    outbound_asset
                ));
            }
            Ok(Some(WithdrawalSigningMode::PqApprovalQuorum))
        }
        chain if is_evm_chain(chain) => {
            if state.config.signer_threshold > 1 && state.config.signer_endpoints.len() > 1 {
                let route = evm_route_for_chain(&state.config, chain)
                    .ok_or_else(|| format!("unsupported destination chain: {}", chain))?;
                if route.multisig_address.is_none() {
                    return Err("EVM multisig address not configured for route".to_string());
                }
                Ok(Some(WithdrawalSigningMode::EvmThresholdSafe))
            } else {
                Ok(Some(WithdrawalSigningMode::PqApprovalQuorum))
            }
        }
        other => Err(format!("unsupported destination chain: {}", other)),
    }
}

pub(super) fn mark_withdrawal_failed(job: &mut WithdrawalJob, err: String) {
    job.attempts = job.attempts.saturating_add(1);
    job.last_error = Some(err);
    if job.attempts >= MAX_JOB_ATTEMPTS {
        job.status = "permanently_failed".to_string();
        job.next_attempt_at = None;
        tracing::error!(
            "AUDIT-FIX H2: withdrawal job {} exceeded {} attempts — moved to permanently_failed. \
             Manual intervention required.",
            job.job_id,
            MAX_JOB_ATTEMPTS
        );
    } else {
        job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
    }
}
