use super::retry::is_ready_for_withdrawal_retry;
use super::*;

pub(in super::super) async fn process_burned_withdrawals(
    state: &CustodyState,
) -> Result<(), String> {
    let burned = list_withdrawal_jobs_by_status(&state.db, "burned")?;
    for mut job in burned {
        if !is_ready_for_withdrawal_retry(&job) {
            continue;
        }

        let now = chrono::Utc::now().timestamp();
        let asset_lower = job.asset.to_lowercase();
        if let Err(error) = ensure_withdrawal_restrictions_allow(
            state,
            &job.user_id,
            &asset_lower,
            job.amount,
            &job.dest_chain,
            &job.preferred_stablecoin,
        )
        .await
        {
            let retry_after = now.saturating_add(state.config.poll_interval_secs as i64);
            if update_withdrawal_hold(&mut job, error.clone(), Some(retry_after)) {
                store_withdrawal_job(&state.db, &job)?;
                emit_custody_event(
                    state,
                    "security.withdrawal_restriction_hold",
                    &job.job_id,
                    None,
                    job.burn_tx_signature.as_deref(),
                    Some(&serde_json::json!({
                        "asset": job.asset,
                        "amount": job.amount,
                        "dest_chain": job.dest_chain,
                        "reason": error,
                        "retry_after": retry_after,
                    })),
                );
            }
            continue;
        }

        match evaluate_withdrawal_velocity_gate(state, &job, now)? {
            WithdrawalVelocityGate::Ready => clear_withdrawal_hold(&mut job),
            WithdrawalVelocityGate::AwaitingRelease { release_after } => {
                let reason = format!("withdrawal velocity hold active until {}", release_after);
                if update_withdrawal_hold(&mut job, reason, Some(release_after)) {
                    store_withdrawal_job(&state.db, &job)?;
                }
                continue;
            }
            WithdrawalVelocityGate::DailyCapHold {
                daily_cap,
                current_volume,
                retry_after,
            } => {
                let reason = format!(
                    "daily withdrawal cap hold: asset={} volume={} cap={} retry_after={}",
                    job.asset, current_volume, daily_cap, retry_after
                );
                if update_withdrawal_hold(&mut job, reason, Some(retry_after)) {
                    store_withdrawal_job(&state.db, &job)?;
                    emit_custody_event(
                        state,
                        "security.withdrawal_daily_cap_hold",
                        &job.job_id,
                        None,
                        job.burn_tx_signature.as_deref(),
                        Some(&serde_json::json!({
                            "asset": job.asset,
                            "amount": job.amount,
                            "current_volume": current_volume,
                            "daily_cap": daily_cap,
                            "retry_after": retry_after,
                        })),
                    );
                }
                continue;
            }
            WithdrawalVelocityGate::AwaitingOperatorConfirmation { required, received } => {
                let reason = format!(
                    "awaiting operator confirmation: {}/{} confirmations",
                    received, required
                );
                if update_withdrawal_hold(&mut job, reason, None) {
                    store_withdrawal_job(&state.db, &job)?;
                    emit_custody_event(
                        state,
                        "security.withdrawal_operator_confirmation_required",
                        &job.job_id,
                        None,
                        job.burn_tx_signature.as_deref(),
                        Some(&serde_json::json!({
                            "asset": job.asset,
                            "amount": job.amount,
                            "velocity_tier": job.velocity_tier.as_str(),
                            "required_operator_confirmations": required,
                            "received_operator_confirmations": received,
                        })),
                    );
                }
                continue;
            }
        }

        let outbound_asset = match asset_lower.as_str() {
            "musd" => job.preferred_stablecoin.clone(),
            "wsol" => "sol".to_string(),
            "weth" => "eth".to_string(),
            "wbnb" => "bnb".to_string(),
            "wgas" => "gas".to_string(),
            _ => continue,
        };
        let required_signer_threshold = effective_required_signer_threshold(&job, &state.config);

        let signing_mode = match determine_withdrawal_signing_mode(state, &job, &outbound_asset) {
            Ok(mode) => mode,
            Err(error) => {
                job.status = "permanently_failed".to_string();
                job.last_error = Some(error.clone());
                job.next_attempt_at = None;
                store_withdrawal_job(&state.db, &job)?;
                emit_custody_event(
                    state,
                    "withdrawal.permanently_failed",
                    &job.job_id,
                    None,
                    None,
                    Some(&serde_json::json!({
                        "asset": job.asset,
                        "amount": job.amount,
                        "dest_chain": job.dest_chain,
                        "last_error": error
                    })),
                );
                continue;
            }
        };

        if signing_mode.is_none() {
            job.status = "signing".to_string();
            store_withdrawal_job(&state.db, &job)?;
            emit_custody_event(
                state,
                "withdrawal.self_signed",
                &job.job_id,
                None,
                None,
                Some(&serde_json::json!({
                    "mode": "self-custody",
                    "asset": job.asset,
                    "amount": job.amount
                })),
            );
            info!(
                "withdrawal self-signed (no external signers): {}",
                job.job_id
            );
            continue;
        }

        let sig_count = match signing_mode.unwrap() {
            WithdrawalSigningMode::PqApprovalQuorum => {
                if job.dest_chain == "solana" || job.dest_chain == "sol" {
                    collect_threshold_solana_withdrawal_signatures(
                        state,
                        &mut job,
                        &outbound_asset,
                        required_signer_threshold,
                    )
                    .await
                } else {
                    collect_pq_withdrawal_approvals(
                        state,
                        &mut job,
                        &outbound_asset,
                        required_signer_threshold,
                    )
                    .await
                }
            }
            WithdrawalSigningMode::EvmThresholdSafe => {
                collect_threshold_evm_withdrawal_signatures(
                    state,
                    &mut job,
                    &outbound_asset,
                    required_signer_threshold,
                )
                .await
            }
        };

        let sig_count = match sig_count {
            Ok(count) => count,
            Err(error) => {
                mark_withdrawal_failed(&mut job, error);
                store_withdrawal_job(&state.db, &job)?;
                continue;
            }
        };

        if sig_count >= required_signer_threshold && required_signer_threshold > 0 {
            job.status = "signing".to_string();
            job.last_error = None;
            job.next_attempt_at = None;
            store_withdrawal_job(&state.db, &job)?;
            emit_custody_event(
                state,
                "withdrawal.signatures_collected",
                &job.job_id,
                None,
                None,
                Some(&serde_json::json!({
                    "sig_count": sig_count,
                    "threshold": required_signer_threshold
                })),
            );
            info!(
                "withdrawal threshold met: {} ({}/{} signatures)",
                job.job_id, sig_count, required_signer_threshold
            );
        } else {
            store_withdrawal_job(&state.db, &job)?;
        }
    }

    Ok(())
}
