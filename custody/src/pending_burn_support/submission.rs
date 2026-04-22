use super::reservation::{release_burn_signature_reservation, reserve_burn_signature};
use super::*;

pub(super) async fn submit_pending_burn_signature(
    state: &CustodyState,
    job_id: &str,
    burn_tx_signature: String,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    // Serialize burn signature submission per job_id to avoid concurrent overwrites.
    static BURN_LOCKS: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

    let lock = {
        let mut locks = BURN_LOCKS.lock().unwrap_or_else(|error| error.into_inner());
        if locks.len() > 10_000 {
            locks.retain(|_, value| std::sync::Arc::strong_count(value) > 1);
        }
        locks
            .entry(job_id.to_string())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let _guard = lock.lock().await;

    let mut job = fetch_withdrawal_job(&state.db, job_id)
        .map_err(|error| Json(ErrorResponse::db(&error)))?
        .ok_or_else(|| Json(ErrorResponse::invalid("withdrawal not found")))?;

    if job.status != "pending_burn" {
        return Err(Json(ErrorResponse::invalid(&format!(
            "withdrawal {} is not in pending_burn state (current: {})",
            job_id, job.status
        ))));
    }

    if job.burn_tx_signature.as_deref() == Some(burn_tx_signature.as_str()) {
        return Ok(Json(json!({
            "job_id": job.job_id,
            "status": job.status,
            "burn_tx_signature": burn_tx_signature,
            "message": "burn_tx_signature already recorded"
        })));
    }

    reserve_burn_signature(&state.db, &burn_tx_signature, job_id)
        .map_err(|error| Json(ErrorResponse::invalid(&error)))?;

    if let Some(existing) = job.burn_tx_signature.replace(burn_tx_signature.clone()) {
        if let Err(error) = release_burn_signature_reservation(&state.db, &existing, job_id) {
            tracing::error!("Failed release_burn_signature_reservation: {error}");
        }
    }

    job.last_error = None;
    job.next_attempt_at = None;
    store_withdrawal_job(&state.db, &job).map_err(|error| Json(ErrorResponse::db(&error)))?;

    record_audit_event(
        &state.db,
        "withdrawal_burn_submitted",
        &job.job_id,
        None,
        Some(&burn_tx_signature),
    )
    .ok();
    emit_custody_event(
        state,
        "withdrawal.burn_submitted",
        &job.job_id,
        None,
        Some(&burn_tx_signature),
        None,
    );

    info!(
        "burn signature submitted for withdrawal {}: {}",
        job_id, burn_tx_signature
    );

    Ok(Json(json!({
        "job_id": job_id,
        "status": "pending_burn",
        "burn_tx_signature": burn_tx_signature,
        "message": "Burn signature recorded. Verification will proceed automatically.",
    })))
}
