use super::*;
mod expiration;
mod failure;
mod verification;

pub(super) async fn process_pending_burn_withdrawals(state: &CustodyState) -> Result<(), String> {
    let pending = list_withdrawal_jobs_by_status(&state.db, "pending_burn")?;
    let now = chrono::Utc::now().timestamp();
    let pending_burn_ttl_secs = state.config.pending_burn_ttl_secs;
    for mut job in pending {
        if pending_burn_ttl_secs > 0 && job.created_at <= now.saturating_sub(pending_burn_ttl_secs)
        {
            expiration::expire_pending_burn_job(state, &mut job, pending_burn_ttl_secs, now)?;
            continue;
        }

        verification::confirm_pending_burn_job(state, &mut job).await?;
    }

    Ok(())
}
