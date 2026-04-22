use super::*;

mod gate;
mod operator;
mod policy;

pub(super) use gate::{
    clear_withdrawal_hold, effective_required_signer_threshold, evaluate_withdrawal_velocity_gate,
    update_withdrawal_hold, WithdrawalVelocityGate,
};
pub(super) use operator::{
    process_withdrawal_operator_confirmation, verify_operator_confirmation_auth,
    WithdrawalOperatorConfirmation,
};
pub(super) use policy::{
    build_withdrawal_velocity_snapshot, load_withdrawal_velocity_policy, velocity_delay_secs,
    WithdrawalVelocityPolicy, WithdrawalVelocitySnapshot, WithdrawalVelocityTier,
};

#[cfg(test)]
pub(super) fn default_withdrawal_tx_caps() -> BTreeMap<String, u64> {
    policy::default_withdrawal_tx_caps()
}

#[cfg(test)]
pub(super) fn default_withdrawal_daily_caps() -> BTreeMap<String, u64> {
    policy::default_withdrawal_daily_caps()
}

#[cfg(test)]
pub(super) fn default_withdrawal_elevated_thresholds() -> BTreeMap<String, u64> {
    policy::default_withdrawal_elevated_thresholds()
}

#[cfg(test)]
pub(super) fn default_withdrawal_extraordinary_thresholds() -> BTreeMap<String, u64> {
    policy::default_withdrawal_extraordinary_thresholds()
}

#[cfg(test)]
pub(super) fn next_utc_day_start(timestamp: i64) -> i64 {
    gate::next_utc_day_start(timestamp)
}

#[cfg(test)]
pub(super) fn operator_token_fingerprint(token: &str) -> String {
    operator::operator_token_fingerprint(token)
}
