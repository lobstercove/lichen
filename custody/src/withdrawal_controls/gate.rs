use super::*;

fn withdrawal_activity_timestamp(job: &WithdrawalJob) -> i64 {
    job.burn_confirmed_at.unwrap_or(job.created_at)
}

fn start_of_utc_day(timestamp: i64) -> i64 {
    const SECONDS_PER_DAY: i64 = 86_400;
    timestamp - timestamp.rem_euclid(SECONDS_PER_DAY)
}

pub(crate) fn next_utc_day_start(timestamp: i64) -> i64 {
    start_of_utc_day(timestamp) + 86_400
}

fn asset_daily_withdrawal_volume(db: &DB, asset: &str, now: i64) -> Result<u64, String> {
    let asset_key = asset.to_ascii_lowercase();
    let day_start = start_of_utc_day(now);
    let day_end = day_start + 86_400;
    let mut total = 0u64;

    for status in ["burned", "signing", "broadcasting", "confirmed"] {
        for job in list_withdrawal_jobs_by_status(db, status)? {
            if job.asset.to_ascii_lowercase() != asset_key {
                continue;
            }

            let activity_ts = withdrawal_activity_timestamp(&job);
            if activity_ts >= day_start && activity_ts < day_end {
                total = total.saturating_add(job.amount);
            }
        }
    }

    Ok(total)
}

pub(crate) fn effective_required_signer_threshold(
    job: &WithdrawalJob,
    config: &CustodyConfig,
) -> usize {
    if job.required_signer_threshold > 0 || job.velocity_tier != WithdrawalVelocityTier::Standard {
        job.required_signer_threshold
    } else {
        config.signer_threshold
    }
}

pub(crate) fn update_withdrawal_hold(
    job: &mut WithdrawalJob,
    reason: String,
    next_attempt_at: Option<i64>,
) -> bool {
    let changed = job.last_error.as_deref() != Some(reason.as_str())
        || job.next_attempt_at != next_attempt_at;
    job.last_error = Some(reason);
    job.next_attempt_at = next_attempt_at;
    changed
}

pub(crate) fn clear_withdrawal_hold(job: &mut WithdrawalJob) {
    job.last_error = None;
    job.next_attempt_at = None;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WithdrawalVelocityGate {
    Ready,
    AwaitingRelease {
        release_after: i64,
    },
    DailyCapHold {
        daily_cap: u64,
        current_volume: u64,
        retry_after: i64,
    },
    AwaitingOperatorConfirmation {
        required: usize,
        received: usize,
    },
}

pub(crate) fn evaluate_withdrawal_velocity_gate(
    state: &CustodyState,
    job: &WithdrawalJob,
    now: i64,
) -> Result<WithdrawalVelocityGate, String> {
    if let Some(release_after) = job.release_after {
        if now < release_after {
            return Ok(WithdrawalVelocityGate::AwaitingRelease { release_after });
        }
    }

    let daily_cap = super::policy::withdrawal_policy_amount(
        &state.config.withdrawal_velocity_policy.daily_caps,
        &job.asset.to_ascii_lowercase(),
    );
    if daily_cap > 0 {
        let current_volume = asset_daily_withdrawal_volume(&state.db, &job.asset, now)?;
        if current_volume > daily_cap {
            return Ok(WithdrawalVelocityGate::DailyCapHold {
                daily_cap,
                current_volume,
                retry_after: next_utc_day_start(now),
            });
        }
    }

    if job.required_operator_confirmations > job.operator_confirmations.len() {
        return Ok(WithdrawalVelocityGate::AwaitingOperatorConfirmation {
            required: job.required_operator_confirmations,
            received: job.operator_confirmations.len(),
        });
    }

    Ok(WithdrawalVelocityGate::Ready)
}
