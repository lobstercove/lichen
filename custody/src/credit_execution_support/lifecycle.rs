use super::*;

pub(super) async fn check_credit_confirmation(
    state: &CustodyState,
    job: &CreditJob,
) -> Result<Option<bool>, String> {
    let Some(signature) = job.tx_signature.as_ref() else {
        return Ok(None);
    };
    let Some(rpc_url) = state.config.licn_rpc_url.as_ref() else {
        return Ok(None);
    };
    let result =
        match licn_rpc_call(&state.http, rpc_url, "getTransaction", json!([signature])).await {
            Ok(v) => v,
            Err(e) if e.contains("not found") || e.contains("not exist") => return Ok(None),
            Err(e) => return Err(e),
        };
    if result.is_null() {
        return Ok(None);
    }
    let success = result.get("status").and_then(|v| v.as_str()) == Some("Success");
    Ok(Some(success))
}

pub(super) fn mark_credit_failed(job: &mut CreditJob, err: String) {
    job.attempts = job.attempts.saturating_add(1);
    job.last_error = Some(err);
    if job.attempts >= MAX_JOB_ATTEMPTS {
        job.status = "permanently_failed".to_string();
        job.next_attempt_at = None;
        tracing::error!(
            "AUDIT-FIX H2: credit job {} exceeded {} attempts — moved to permanently_failed. \
             Manual intervention required.",
            job.job_id,
            MAX_JOB_ATTEMPTS
        );
    } else {
        job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
    }
}

pub(super) fn is_ready_for_credit_retry(job: &CreditJob) -> bool {
    match job.next_attempt_at {
        Some(ts) => chrono::Utc::now().timestamp() >= ts,
        None => true,
    }
}
