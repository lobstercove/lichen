use serde_json::json;

use super::super::*;
use super::failure::reset_pending_burn_submission;

fn expected_burn_contract<'a>(state: &'a CustodyState, asset: &str) -> Option<&'a str> {
    match asset.to_lowercase().as_str() {
        "wsol" => state.config.wsol_contract_addr.as_deref(),
        "weth" => state.config.weth_contract_addr.as_deref(),
        "wbnb" => state.config.wbnb_contract_addr.as_deref(),
        "musd" => state.config.musd_contract_addr.as_deref(),
        _ => None,
    }
}

fn reset_invalid_burn_submission(state: &CustodyState, job: &mut WithdrawalJob, err: String) {
    if let Err(error) = reset_pending_burn_submission(&state.db, job, err) {
        tracing::error!("Failed reset_pending_burn_submission: {error}");
    }
}

fn mark_missing_burn_contract(state: &CustodyState, job: &mut WithdrawalJob) {
    tracing::error!(
        "🚨 BURN VERIFICATION FAILED for {}: no contract configured for asset {}. Cannot verify burn. Marking permanently_failed.",
        job.job_id,
        job.asset
    );
    job.status = "permanently_failed".to_string();
    job.last_error = Some(format!(
        "No contract address configured for asset '{}'",
        job.asset
    ));
    if let Err(error) = store_withdrawal_job(&state.db, job) {
        tracing::error!("Failed store_withdrawal_job: {error}");
    }
}

fn apply_velocity_hold(state: &CustodyState, job: &mut WithdrawalJob, burn_confirmed_at: i64) {
    if job.velocity_tier != WithdrawalVelocityTier::Standard && job.release_after.is_none() {
        let hold_until = burn_confirmed_at.saturating_add(velocity_delay_secs(
            &state.config.withdrawal_velocity_policy,
            job.velocity_tier,
        ));
        if hold_until > burn_confirmed_at {
            job.release_after = Some(hold_until);
            job.next_attempt_at = Some(hold_until);
            job.last_error = Some(format!(
                "withdrawal velocity hold active until {}",
                hold_until
            ));
        }
    }
}

pub(super) async fn confirm_pending_burn_job(
    state: &CustodyState,
    job: &mut WithdrawalJob,
) -> Result<(), String> {
    let Some(burn_sig) = job.burn_tx_signature.clone() else {
        return Ok(());
    };
    let Some(rpc_url) = state.config.licn_rpc_url.as_ref() else {
        return Ok(());
    };

    match licn_rpc_call(&state.http, rpc_url, "getTransaction", json!([burn_sig])).await {
        Ok(result) => {
            if result.is_null() {
                return Ok(());
            }

            let success = result.get("status").and_then(|value| value.as_str()) == Some("Success");
            if !success {
                return Ok(());
            }

            let Some(expected_contract) = expected_burn_contract(state, &job.asset) else {
                mark_missing_burn_contract(state, job);
                return Ok(());
            };

            let tx_contract = result.get("to").and_then(|value| value.as_str());
            if tx_contract != Some(expected_contract) {
                tracing::error!(
                    "🚨 BURN VERIFICATION FAILED for {}: expected contract {} but tx called {:?}. Possible attack!",
                    job.job_id,
                    expected_contract,
                    tx_contract
                );
                reset_invalid_burn_submission(
                    state,
                    job,
                    format!(
                        "Burn contract mismatch: expected {} got {:?}",
                        expected_contract, tx_contract
                    ),
                );
                return Ok(());
            }

            let tx_method = result
                .get("contract_function")
                .and_then(|value| value.as_str());
            if tx_method != Some("burn") {
                tracing::error!(
                    "🚨 BURN VERIFICATION FAILED for {}: expected method 'burn' but tx called {:?}. Possible attack!",
                    job.job_id,
                    tx_method
                );
                reset_invalid_burn_submission(
                    state,
                    job,
                    format!("Burn method mismatch: expected 'burn' got {:?}", tx_method),
                );
                return Ok(());
            }

            let tx_amount = result
                .get("token_amount_spores")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            if tx_amount != job.amount {
                let expected_amount = job.amount;
                tracing::error!(
                    "🚨 BURN VERIFICATION FAILED for {}: expected amount {} but tx burned {}. Amount mismatch!",
                    job.job_id,
                    expected_amount,
                    tx_amount
                );
                reset_invalid_burn_submission(
                    state,
                    job,
                    format!(
                        "Burn amount mismatch: expected {} got {}",
                        expected_amount, tx_amount
                    ),
                );
                return Ok(());
            }

            let tx_caller = result.get("from").and_then(|value| value.as_str());
            if tx_caller != Some(job.user_id.as_str()) {
                let expected_user_id = job.user_id.clone();
                tracing::error!(
                    "🚨 BURN VERIFICATION FAILED for {}: expected caller {} but tx caller was {:?}. Possible attack!",
                    job.job_id,
                    expected_user_id,
                    tx_caller
                );
                reset_invalid_burn_submission(
                    state,
                    job,
                    format!(
                        "Burn caller mismatch: expected {} got {:?}",
                        expected_user_id, tx_caller
                    ),
                );
                return Ok(());
            }

            let burn_confirmed_at = chrono::Utc::now().timestamp();
            job.status = "burned".to_string();
            job.burn_confirmed_at = Some(burn_confirmed_at);
            apply_velocity_hold(state, job, burn_confirmed_at);
            store_withdrawal_job(&state.db, job)?;
            emit_custody_event(
                state,
                "withdrawal.burn_confirmed",
                &job.job_id,
                None,
                job.burn_tx_signature.as_deref(),
                Some(&serde_json::json!({
                    "user_id": job.user_id,
                    "asset": job.asset,
                    "amount": job.amount
                })),
            );
            if let Some(release_after) = job.release_after {
                emit_custody_event(
                    state,
                    "security.withdrawal_velocity_hold",
                    &job.job_id,
                    None,
                    job.burn_tx_signature.as_deref(),
                    Some(&serde_json::json!({
                        "asset": job.asset,
                        "amount": job.amount,
                        "velocity_tier": job.velocity_tier.as_str(),
                        "release_after": release_after,
                        "required_signer_threshold": job.required_signer_threshold,
                        "required_operator_confirmations": job.required_operator_confirmations,
                    })),
                );
            }
            info!("withdrawal burn confirmed: {}", job.job_id);
        }
        Err(error) => {
            tracing::warn!("burn verification failed for {}: {}", job.job_id, error);
        }
    }

    Ok(())
}
