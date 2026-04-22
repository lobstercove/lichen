use super::record::record_audit_event_ext;
use super::*;

pub(in super::super) fn emit_withdrawal_spike_event(
    state: &CustodyState,
    req: &WithdrawalRequest,
    reason: &str,
    count_this_minute: u64,
    max_withdrawals_per_min: u64,
    value_this_hour: u64,
    max_value_per_hour: u64,
) {
    let entity_id = if req.user_id.trim().is_empty() {
        req.dest_address.as_str()
    } else {
        req.user_id.as_str()
    };
    let projected_value_this_hour = value_this_hour.saturating_add(req.amount);
    let data = json!({
        "reason": reason,
        "user_id": req.user_id,
        "dest_chain": req.dest_chain,
        "asset": req.asset,
        "requested_amount": req.amount,
        "dest_address": req.dest_address,
        "count_this_minute": count_this_minute,
        "projected_count_this_minute": count_this_minute.saturating_add(1),
        "max_withdrawals_per_min": max_withdrawals_per_min,
        "value_this_hour": value_this_hour,
        "projected_value_this_hour": projected_value_this_hour,
        "max_value_per_hour": max_value_per_hour,
    });

    if let Err(err) = record_audit_event_ext(
        &state.db,
        "security.withdrawal_spike",
        entity_id,
        None,
        None,
        Some(&data),
        Some(&state.event_tx),
    ) {
        warn!("failed to record withdrawal spike event: {}", err);
    }
}

pub(in super::super) fn next_withdrawal_warning_level(
    projected_value: u64,
    max_value: u64,
    last_emitted: Option<WithdrawalWarningLevel>,
) -> Option<WithdrawalWarningLevel> {
    if max_value == 0 {
        return None;
    }

    [
        WithdrawalWarningLevel::NearLimit,
        WithdrawalWarningLevel::ThreeQuartersUsed,
        WithdrawalWarningLevel::HalfUsed,
    ]
    .into_iter()
    .find(|level| {
        let threshold = u128::from(max_value) * u128::from(level.threshold_percent());
        let scaled = u128::from(projected_value) * 100;
        scaled >= threshold
            && match last_emitted {
                Some(prev) => *level > prev,
                None => true,
            }
    })
}

pub(in super::super) fn emit_withdrawal_velocity_warning_event(
    state: &CustodyState,
    req: &WithdrawalRequest,
    reason: &str,
    level: WithdrawalWarningLevel,
    metrics: WithdrawalVelocityMetrics,
) {
    let entity_id = if req.user_id.trim().is_empty() {
        req.dest_address.as_str()
    } else {
        req.user_id.as_str()
    };
    let projected_count_this_minute = metrics.count_this_minute.saturating_add(1);
    let projected_value_this_hour = metrics.value_this_hour.saturating_add(req.amount);
    let data = json!({
        "reason": reason,
        "alert_level": level.as_str(),
        "severity": level.severity(),
        "threshold_percent": level.threshold_percent(),
        "user_id": req.user_id,
        "dest_chain": req.dest_chain,
        "asset": req.asset,
        "requested_amount": req.amount,
        "dest_address": req.dest_address,
        "count_this_minute": metrics.count_this_minute,
        "projected_count_this_minute": projected_count_this_minute,
        "max_withdrawals_per_min": metrics.max_withdrawals_per_min,
        "value_this_hour": metrics.value_this_hour,
        "projected_value_this_hour": projected_value_this_hour,
        "max_value_per_hour": metrics.max_value_per_hour,
    });

    if let Err(err) = record_audit_event_ext(
        &state.db,
        "security.withdrawal_velocity_warning",
        entity_id,
        None,
        None,
        Some(&data),
        Some(&state.event_tx),
    ) {
        warn!(
            "failed to record withdrawal velocity warning event: {}",
            err
        );
    }
}
