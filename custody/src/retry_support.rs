use super::*;

/// Cap failed job retries so operators get a hard stop instead of infinite replays.
pub(super) const MAX_JOB_ATTEMPTS: u32 = 10;

pub(super) fn mark_sweep_failed(job: &mut SweepJob, err: String) {
    job.attempts = job.attempts.saturating_add(1);
    job.last_error = Some(err);
    if job.attempts >= MAX_JOB_ATTEMPTS {
        job.status = "permanently_failed".to_string();
        job.next_attempt_at = None;
        tracing::error!(
            "AUDIT-FIX H2: sweep job {} exceeded {} attempts — moved to permanently_failed. \
             Manual intervention required.",
            job.job_id,
            MAX_JOB_ATTEMPTS
        );
    } else {
        job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
    }
}

pub(super) fn next_retry_timestamp(attempts: u32) -> i64 {
    let delay = 30i64.saturating_mul(2i64.saturating_pow(attempts.min(5)));
    chrono::Utc::now().timestamp() + delay
}

pub(super) fn is_ready_for_retry(job: &SweepJob) -> bool {
    match job.next_attempt_at {
        Some(ts) => chrono::Utc::now().timestamp() >= ts,
        None => true,
    }
}
