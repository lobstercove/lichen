use super::*;

mod builder;
mod lifecycle;
mod mint;
mod persistence;

pub(super) use builder::build_credit_job;
use lifecycle::{check_credit_confirmation, is_ready_for_credit_retry, mark_credit_failed};
use mint::submit_wrapped_credit;
pub(super) use persistence::{count_credit_jobs, list_credit_jobs_by_status, store_credit_job};

pub(super) async fn credit_worker_loop(state: CustodyState) {
    loop {
        if let Err(err) = process_credit_jobs(&state).await {
            tracing::warn!("credit worker error: {}", err);
        }
        sleep(Duration::from_secs(state.config.poll_interval_secs)).await;
    }
}

pub(super) async fn process_credit_jobs(state: &CustodyState) -> Result<(), String> {
    if state.config.licn_rpc_url.is_none() || state.config.treasury_keypair_path.is_none() {
        // AUDIT-FIX CUST-05: Warn instead of silently skipping (jobs accumulate in queued state)
        tracing::warn!(
            "credit worker skipping: licn_rpc_url or treasury_keypair_path not configured"
        );
        return Ok(());
    }

    let jobs = list_credit_jobs_by_status(&state.db, "queued")?;
    for mut job in jobs {
        if !is_ready_for_credit_retry(&job) {
            continue;
        }
        // AUDIT-FIX M4: Record intent before credit broadcast
        if let Err(e) = record_tx_intent(&state.db, "credit", &job.job_id, "lichen") {
            let error = format!("failed to record credit tx intent: {e}");
            tracing::error!("{error}");
            mark_credit_failed(&mut job, error);
            store_credit_job(&state.db, &job)?;
            continue;
        }
        match submit_wrapped_credit(state, &job).await {
            Ok(tx_signature) => {
                if let Err(e) = clear_tx_intent(&state.db, "credit", &job.job_id) {
                    tracing::error!("Failed clear_tx_intent: {e}");
                }
                job.status = "submitted".to_string();
                job.tx_signature = Some(tx_signature);
                job.last_error = None;
                job.next_attempt_at = None;
                store_credit_job(&state.db, &job)?;
                emit_custody_event(
                    state,
                    "credit.submitted",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.tx_signature.as_deref(),
                    None,
                );
            }
            Err(err) => {
                if let Err(e) = clear_tx_intent(&state.db, "credit", &job.job_id) {
                    tracing::error!("Failed clear_tx_intent: {e}");
                }
                tracing::warn!("credit mint failed for deposit={}: {}", job.deposit_id, err);
                mark_credit_failed(&mut job, err);
                store_credit_job(&state.db, &job)?;
            }
        }
    }

    let mut submitted = list_credit_jobs_by_status(&state.db, "submitted")?;
    for job in submitted.iter_mut() {
        let confirmation = match check_credit_confirmation(state, job).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "credit confirmation check failed for job={}: {}",
                    job.job_id,
                    e
                );
                continue;
            }
        };
        if let Some(confirmed) = confirmation {
            if confirmed {
                job.status = "confirmed".to_string();
                job.last_error = None;
                job.next_attempt_at = None;
                store_credit_job(&state.db, job)?;

                // P0-FIX: Update the deposit record to "credited" so polling clients
                // see the terminal state and can stop polling.
                if let Err(e) = update_deposit_status(&state.db, &job.deposit_id, "credited") {
                    tracing::error!("Failed update_deposit_status: {e}");
                }
                if let Err(e) =
                    update_status_index(&state.db, "deposits", "swept", "credited", &job.deposit_id)
                {
                    tracing::error!("Failed update_status_index: {e}");
                }

                emit_custody_event(
                    state,
                    "credit.confirmed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.tx_signature.as_deref(),
                    Some(
                        &json!({ "amount_spores": job.amount_spores, "to_address": job.to_address }),
                    ),
                );
            }
        }
    }
    Ok(())
}
