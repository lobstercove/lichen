use super::withdrawal_signing_support::process_burned_withdrawals;
use super::*;

pub(super) async fn withdrawal_worker_loop(state: CustodyState) {
    loop {
        if let Err(err) = process_withdrawal_jobs(&state).await {
            tracing::warn!("withdrawal worker error: {}", err);
        }
        tokio::time::sleep(std::time::Duration::from_secs(
            state.config.poll_interval_secs,
        ))
        .await;
    }
}

pub(super) async fn process_withdrawal_jobs(state: &CustodyState) -> Result<(), String> {
    pending_burn_support::process_pending_burn_withdrawals(state).await?;

    process_burned_withdrawals(state).await?;

    process_signing_withdrawals(state).await?;
    process_broadcasting_withdrawals(state).await?;

    Ok(())
}

pub(super) async fn process_signing_withdrawals(state: &CustodyState) -> Result<(), String> {
    let signing = list_withdrawal_jobs_by_status(&state.db, "signing")?;
    for mut job in signing {
        if let Err(error) = record_tx_intent(&state.db, "withdrawal", &job.job_id, &job.dest_chain)
        {
            let error = format!("failed to record withdrawal tx intent: {error}");
            tracing::error!("{error}");
            mark_withdrawal_failed(&mut job, error);
            store_withdrawal_job(&state.db, &job)?;
            continue;
        }
        match broadcast_outbound_withdrawal(state, &job).await {
            Ok(tx_hash) => {
                if let Err(error) = clear_tx_intent(&state.db, "withdrawal", &job.job_id) {
                    tracing::error!("Failed clear_tx_intent: {error}");
                }
                job.outbound_tx_hash = Some(tx_hash.clone());
                job.status = "broadcasting".to_string();
                job.last_error = None;
                store_withdrawal_job(&state.db, &job)?;
                emit_custody_event(
                    state,
                    "withdrawal.broadcast",
                    &job.job_id,
                    None,
                    Some(&tx_hash),
                    Some(&serde_json::json!({
                        "dest_chain": job.dest_chain,
                        "dest_address": job.dest_address,
                        "asset": job.asset,
                        "amount": job.amount
                    })),
                );
                info!("withdrawal broadcast: {} → tx={}", job.job_id, tx_hash);
            }
            Err(error) => {
                if let Err(clear_error) = clear_tx_intent(&state.db, "withdrawal", &job.job_id) {
                    tracing::error!("Failed clear_tx_intent: {clear_error}");
                }
                job.attempts = job.attempts.saturating_add(1);
                job.last_error = Some(error.clone());
                if job.attempts >= MAX_JOB_ATTEMPTS {
                    job.status = "permanently_failed".to_string();
                    store_withdrawal_job(&state.db, &job)?;
                    tracing::error!(
                        "🚨 withdrawal {} permanently failed after {} attempts: {}",
                        job.job_id,
                        MAX_JOB_ATTEMPTS,
                        error
                    );
                    emit_custody_event(
                        state,
                        "withdrawal.permanently_failed",
                        &job.job_id,
                        None,
                        None,
                        Some(&serde_json::json!({
                            "attempts": job.attempts,
                            "last_error": error,
                            "asset": job.asset,
                            "amount": job.amount
                        })),
                    );
                } else {
                    job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
                    store_withdrawal_job(&state.db, &job)?;
                    tracing::warn!("withdrawal broadcast failed for {}: {}", job.job_id, error);
                }
            }
        }
    }

    Ok(())
}

pub(super) async fn process_broadcasting_withdrawals(state: &CustodyState) -> Result<(), String> {
    let broadcasting = list_withdrawal_jobs_by_status(&state.db, "broadcasting")?;
    for mut job in broadcasting {
        let confirmed = match job.dest_chain.as_str() {
            "solana" | "sol" => {
                if let (Some(url), Some(ref tx_hash)) =
                    (state.config.solana_rpc_url.as_ref(), &job.outbound_tx_hash)
                {
                    match check_solana_tx_confirmed(
                        &state.http,
                        url,
                        tx_hash,
                        state.config.solana_confirmations,
                    )
                    .await
                    {
                        Ok(confirmed) => confirmed,
                        Err(error) if terminal_confirmation_error(&error) => {
                            mark_withdrawal_confirmed_tx_failed(state, &mut job, error)?;
                            continue;
                        }
                        Err(error) => {
                            tracing::warn!(
                                "withdrawal confirmation check failed for {}: {}",
                                job.job_id,
                                error
                            );
                            false
                        }
                    }
                } else {
                    false
                }
            }
            chain if is_evm_chain(chain) => {
                if let (Some(url), Some(ref tx_hash)) = (
                    rpc_url_for_chain(&state.config, chain),
                    &job.outbound_tx_hash,
                ) {
                    match check_evm_tx_confirmed(
                        &state.http,
                        &url,
                        tx_hash,
                        evm_route_confirmations(&state.config, chain)
                            .unwrap_or(state.config.evm_confirmations),
                    )
                    .await
                    {
                        Ok(confirmed) => confirmed,
                        Err(error) if terminal_confirmation_error(&error) => {
                            mark_withdrawal_confirmed_tx_failed(state, &mut job, error)?;
                            continue;
                        }
                        Err(error) => {
                            tracing::warn!(
                                "withdrawal confirmation check failed for {}: {}",
                                job.job_id,
                                error
                            );
                            false
                        }
                    }
                } else {
                    false
                }
            }
            _ => false,
        };

        if confirmed {
            let asset_lower = job.asset.to_lowercase();
            if asset_lower == "musd" {
                let stablecoin = &job.preferred_stablecoin;
                let chain_debit = spores_to_chain_amount(job.amount, &job.dest_chain, stablecoin)?;
                let chain_debit_u64 = u64::try_from(chain_debit).unwrap_or(u64::MAX);
                if let Err(error) = adjust_reserve_balance_once(
                    &state.db,
                    &job.dest_chain,
                    stablecoin,
                    chain_debit_u64,
                    false,
                    &format!("withdrawal:{}", job.job_id),
                )
                .await
                {
                    tracing::warn!("reserve ledger decrement failed: {}", error);
                    continue;
                }
            }

            job.status = "confirmed".to_string();
            job.last_error = None;
            store_withdrawal_job(&state.db, &job)?;
            emit_custody_event(
                state,
                "withdrawal.confirmed",
                &job.job_id,
                None,
                job.outbound_tx_hash.as_deref(),
                Some(&serde_json::json!({
                    "dest_chain": job.dest_chain,
                    "dest_address": job.dest_address,
                    "asset": job.asset,
                    "amount": job.amount,
                    "user_id": job.user_id
                })),
            );

            info!(
                "withdrawal confirmed: {} (dest tx={})",
                job.job_id,
                job.outbound_tx_hash.as_deref().unwrap_or("?")
            );
        }
    }

    Ok(())
}

fn terminal_confirmation_error(error: &str) -> bool {
    error.starts_with("solana transaction failed:")
        || error.starts_with("evm transaction failed with status")
}

fn mark_withdrawal_confirmed_tx_failed(
    state: &CustodyState,
    job: &mut WithdrawalJob,
    error: String,
) -> Result<(), String> {
    job.status = "permanently_failed".to_string();
    job.last_error = Some(error.clone());
    job.next_attempt_at = None;
    store_withdrawal_job(&state.db, job)?;
    emit_custody_event(
        state,
        "withdrawal.permanently_failed",
        &job.job_id,
        None,
        job.outbound_tx_hash.as_deref(),
        Some(&serde_json::json!({
            "dest_chain": job.dest_chain,
            "dest_address": job.dest_address,
            "asset": job.asset,
            "amount": job.amount,
            "last_error": error,
        })),
    );
    Ok(())
}
