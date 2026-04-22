use super::*;

pub(super) fn is_ready_for_withdrawal_retry(job: &WithdrawalJob) -> bool {
    match job.next_attempt_at {
        Some(ts) => chrono::Utc::now().timestamp() >= ts,
        None => true,
    }
}
