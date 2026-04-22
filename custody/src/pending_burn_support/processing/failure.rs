use rocksdb::DB;

use super::super::reservation::release_burn_signature_reservation;
use super::super::*;

pub(super) fn reset_pending_burn_submission(
    db: &DB,
    job: &mut WithdrawalJob,
    err: String,
) -> Result<(), String> {
    if let Some(existing) = job.burn_tx_signature.take() {
        if let Err(error) = release_burn_signature_reservation(db, &existing, &job.job_id) {
            tracing::error!("Failed release_burn_signature_reservation: {error}");
        }
    }

    job.attempts = job.attempts.saturating_add(1);
    job.last_error = Some(err);
    if job.attempts >= MAX_JOB_ATTEMPTS {
        job.status = "permanently_failed".to_string();
        job.next_attempt_at = None;
        tracing::error!(
            "withdrawal job {} exceeded {} invalid burn submissions — moved to permanently_failed",
            job.job_id,
            MAX_JOB_ATTEMPTS
        );
    } else {
        job.status = "pending_burn".to_string();
        job.next_attempt_at = None;
    }

    store_withdrawal_job(db, job)
}
