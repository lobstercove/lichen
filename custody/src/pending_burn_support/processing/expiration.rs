use super::super::reservation::release_burn_signature_reservation;
use super::super::*;

pub(super) fn expire_pending_burn_job(
    state: &CustodyState,
    job: &mut WithdrawalJob,
    ttl_secs: i64,
    now: i64,
) -> Result<(), String> {
    let burn_tx_signature = job.burn_tx_signature.take();
    if let Some(existing) = burn_tx_signature.as_deref() {
        if let Err(error) = release_burn_signature_reservation(&state.db, existing, &job.job_id) {
            tracing::error!("Failed release_burn_signature_reservation: {error}");
        }
    }

    let age_secs = now.saturating_sub(job.created_at).max(0);
    let last_error = format!(
        "pending_burn expired after {} seconds without a confirmed burn",
        age_secs
    );
    job.status = "expired".to_string();
    job.last_error = Some(last_error.clone());
    job.next_attempt_at = None;
    store_withdrawal_job(&state.db, job)?;

    record_audit_event(
        &state.db,
        "withdrawal_pending_burn_expired",
        &job.job_id,
        None,
        burn_tx_signature.as_deref(),
    )
    .ok();
    emit_custody_event(
        state,
        "withdrawal.expired",
        &job.job_id,
        None,
        burn_tx_signature.as_deref(),
        Some(&serde_json::json!({
            "asset": job.asset,
            "amount": job.amount,
            "dest_chain": job.dest_chain,
            "ttl_secs": ttl_secs,
            "created_at": job.created_at,
            "expired_at": now,
            "last_error": last_error,
        })),
    );
    info!(
        "withdrawal pending_burn expired: {} (age={}s ttl={}s)",
        job.job_id, age_secs, ttl_secs
    );

    Ok(())
}
