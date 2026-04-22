use super::*;

mod persistence;
mod state;

pub(super) type DepositRateState = state::DepositRateState;
pub(super) type WithdrawalRateState = state::WithdrawalRateState;
pub(super) type WithdrawalVelocityMetrics = state::WithdrawalVelocityMetrics;
pub(super) type WithdrawalWarningLevel = state::WithdrawalWarningLevel;

pub(super) fn load_withdrawal_rate_state(db: &DB) -> Result<WithdrawalRateState, String> {
    persistence::load_withdrawal_rate_state(db)
}

pub(super) fn persist_withdrawal_rate_state(
    db: &DB,
    state: &WithdrawalRateState,
) -> Result<(), String> {
    persistence::persist_withdrawal_rate_state(db, state)
}

pub(super) fn load_deposit_rate_state(db: &DB) -> Result<DepositRateState, String> {
    persistence::load_deposit_rate_state(db)
}

pub(super) fn persist_deposit_rate_state(db: &DB, state: &DepositRateState) -> Result<(), String> {
    persistence::persist_deposit_rate_state(db, state)
}
