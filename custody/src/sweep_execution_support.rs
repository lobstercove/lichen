use super::*;

mod broadcast;
mod confirmation;

use self::broadcast::broadcast_sweep;
use self::confirmation::check_sweep_confirmation;

pub(super) async fn sweep_worker_loop(state: CustodyState) {
    loop {
        if let Err(err) = process_sweep_jobs(&state).await {
            tracing::warn!("sweep worker error: {}", err);
        }
        sleep(Duration::from_secs(state.config.poll_interval_secs)).await;
    }
}

pub(super) async fn process_sweep_jobs(state: &CustodyState) -> Result<(), String> {
    let local_sweep_error = local_sweep_policy_error(&state.config);
    let queued_jobs = list_sweep_jobs_by_status(&state.db, "queued")?;
    for mut job in queued_jobs {
        if let Some(err) = local_sweep_error.as_ref() {
            job.status = "permanently_failed".to_string();
            job.last_error = Some(err.clone());
            job.next_attempt_at = None;
            store_sweep_job(&state.db, &job)?;
            emit_custody_event(
                state,
                "sweep.failed",
                &job.job_id,
                Some(&job.deposit_id),
                None,
                Some(&json!({ "last_error": err, "mode": "blocked-local-sweep" })),
            );
            continue;
        }

        job.status = "signing".to_string();
        store_sweep_job(&state.db, &job)?;
        emit_custody_event(
            state,
            "sweep.signing",
            &job.job_id,
            Some(&job.deposit_id),
            None,
            None,
        );
    }

    if local_sweep_error.is_none()
        && !state.config.signer_endpoints.is_empty()
        && state.config.signer_threshold > 0
    {
        warn!(
            "external signer endpoints are configured, but deposit sweeps still broadcast with locally derived deposit keys; skipping placeholder sweep signature collection"
        );
        promote_locally_signed_sweep_jobs(state, "locally-derived-deposit-key")?;
    } else if local_sweep_error.is_none() {
        promote_locally_signed_sweep_jobs(state, "self-custody")?;
    }

    if let Some(err) = local_sweep_error.as_ref() {
        for status in ["signing", "signed"] {
            let jobs = list_sweep_jobs_by_status(&state.db, status)?;
            for mut job in jobs {
                job.status = "permanently_failed".to_string();
                job.last_error = Some(err.clone());
                job.next_attempt_at = None;
                store_sweep_job(&state.db, &job)?;
                emit_custody_event(
                    state,
                    "sweep.failed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    None,
                    Some(&json!({ "last_error": err, "mode": "blocked-local-sweep" })),
                );
            }
        }
    }

    let mut signed_jobs = list_sweep_jobs_by_status(&state.db, "signed")?;
    for job in signed_jobs.iter_mut() {
        if !is_ready_for_retry(job) {
            continue;
        }
        // AUDIT-FIX M4: Record intent before broadcast for crash idempotency
        if let Err(e) = record_tx_intent(&state.db, "sweep", &job.job_id, &job.chain) {
            tracing::error!("Failed record_tx_intent: {e}");
        }
        match broadcast_sweep(state, job).await {
            Ok(Some(tx_hash)) => {
                if let Err(e) = clear_tx_intent(&state.db, "sweep", &job.job_id) {
                    tracing::error!("Failed clear_tx_intent: {e}");
                }
                job.status = "sweep_submitted".to_string();
                job.sweep_tx_hash = Some(tx_hash);
                job.last_error = None;
                job.next_attempt_at = None;
                store_sweep_job(&state.db, job)?;
                emit_custody_event(
                    state,
                    "sweep.submitted",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.sweep_tx_hash.as_deref(),
                    None,
                );

                // AUDIT-FIX C2: Credit job (wrapped token mint) is now created AFTER
                // sweep confirmation, not here. Minting before sweep is confirmed risks
                // issuing wrapped tokens when the sweep tx reverts — a fund mismatch.
            }
            Ok(None) => {
                if let Err(e) = clear_tx_intent(&state.db, "sweep", &job.job_id) {
                    tracing::error!("Failed clear_tx_intent: {e}");
                }
                if job.chain == "solana" && !is_solana_stablecoin(&job.asset) {
                    job.status = "signed".to_string();
                    job.last_error = Some(
                        "insufficient native SOL to sweep after fees; awaiting additional funds"
                            .to_string(),
                    );
                    job.next_attempt_at = Some(chrono::Utc::now().timestamp() + 60);
                } else {
                    mark_sweep_failed(job, "broadcast returned empty".to_string());
                }
                store_sweep_job(&state.db, job)?;
                emit_custody_event(
                    state,
                    "sweep.failed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.sweep_tx_hash.as_deref(),
                    None,
                );
            }
            Err(err) => {
                if let Err(e) = clear_tx_intent(&state.db, "sweep", &job.job_id) {
                    tracing::error!("Failed clear_tx_intent: {e}");
                }
                warn!("sweep broadcast failed: {}", err);
                mark_sweep_failed(job, err);
                store_sweep_job(&state.db, job)?;
            }
        }
    }

    let mut submitted_jobs = list_sweep_jobs_by_status(&state.db, "sweep_submitted")?;
    for job in submitted_jobs.iter_mut() {
        if let Some(confirmed) = check_sweep_confirmation(state, job).await? {
            if confirmed {
                job.status = "sweep_confirmed".to_string();
                job.last_error = None;
                job.next_attempt_at = None;
                store_sweep_job(&state.db, job)?;

                // P0-FIX: Update the deposit record to "swept" so polling clients
                // see the status progression (issued -> confirmed -> swept -> credited)
                if let Err(e) = update_deposit_status(&state.db, &job.deposit_id, "swept") {
                    tracing::error!("Failed update_deposit_status: {e}");
                }
                if let Err(e) = update_status_index(
                    &state.db,
                    "deposits",
                    "sweep_queued",
                    "swept",
                    &job.deposit_id,
                ) {
                    tracing::error!("Failed update_status_index: {e}");
                }

                emit_custody_event(
                    state,
                    "sweep.confirmed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.sweep_tx_hash.as_deref(),
                    Some(&json!({ "chain": job.chain, "asset": job.asset, "amount": job.amount })),
                );

                // Track stablecoin reserves: when a sweep is confirmed, the treasury
                // now holds the deposited asset. Update the reserve ledger.
                let asset_lower = job.asset.to_lowercase();
                if asset_lower == "usdt" || asset_lower == "usdc" {
                    if let Some(ref amount_str) = job.amount {
                        if let Ok(amount) = amount_str.parse::<u64>() {
                            if let Err(e) = adjust_reserve_balance(
                                &state.db,
                                &job.chain,
                                &asset_lower,
                                amount,
                                true,
                            )
                            .await
                            {
                                tracing::warn!("reserve ledger update failed: {}", e);
                            }
                        }
                    }
                }

                // AUDIT-FIX C2: Create credit job (mint wrapped tokens) only AFTER
                // the sweep is confirmed on-chain. This ensures the treasury actually
                // received the funds before issuing wrapped tokens to the user.
                match build_credit_job(state, job)? {
                    Some(credit_job) => {
                        store_credit_job(&state.db, &credit_job)?;
                        emit_custody_event(
                            state,
                            "credit.queued",
                            &credit_job.job_id,
                            Some(&credit_job.deposit_id),
                            None,
                            Some(
                                &json!({ "amount_spores": credit_job.amount_spores, "to_address": credit_job.to_address }),
                            ),
                        );
                    }
                    None => {
                        // AUDIT-FIX R-H1: Log when credit job cannot be built
                        // after a confirmed sweep. This means the treasury received
                        // funds but the user won't get wrapped tokens automatically.
                        tracing::error!(
                            "🚨 CREDIT JOB NOT CREATED for sweep {} (deposit {}). \
                             Treasury received funds but no wrapped tokens will be minted. \
                             Manual operator intervention required to credit the user.",
                            job.job_id,
                            job.deposit_id
                        );
                        emit_custody_event(
                            state,
                            "credit.build_failed",
                            &job.job_id,
                            Some(&job.deposit_id),
                            None,
                            None,
                        );
                    }
                }
            } else {
                job.status = "failed".to_string();
                mark_sweep_failed(
                    job,
                    "sweep transaction reverted or failed on-chain".to_string(),
                );
                store_sweep_job(&state.db, job)?;
                emit_custody_event(
                    state,
                    "sweep.failed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.sweep_tx_hash.as_deref(),
                    Some(
                        &json!({ "last_error": job.last_error, "chain": job.chain, "asset": job.asset }),
                    ),
                );
            }
        }
    }

    Ok(())
}
