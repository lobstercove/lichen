use super::super::*;
use super::state::{
    DepositRateState, DepositRateStateSnapshot, WithdrawalRateState, WithdrawalRateStateSnapshot,
};

const CURSOR_WITHDRAWAL_RATE_STATE: &str = "withdrawal_rate_state";
const CURSOR_DEPOSIT_RATE_STATE: &str = "deposit_rate_state";

fn current_unix_secs_lossy() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(in crate::rate_limits) fn instant_to_unix_secs(
    instant: std::time::Instant,
    reference_now: std::time::Instant,
    reference_secs: u64,
) -> u64 {
    reference_secs.saturating_sub(reference_now.duration_since(instant).as_secs())
}

pub(in crate::rate_limits) fn unix_secs_to_instant(
    saved_secs: u64,
    reference_now: std::time::Instant,
    reference_secs: u64,
) -> std::time::Instant {
    reference_now
        .checked_sub(std::time::Duration::from_secs(
            reference_secs.saturating_sub(saved_secs),
        ))
        .unwrap_or(reference_now)
}

fn load_cursor_snapshot<T: serde::de::DeserializeOwned>(
    db: &DB,
    key: &str,
) -> Result<Option<T>, String> {
    let cf = db
        .cf_handle(CF_CURSORS)
        .ok_or_else(|| "missing cursors cf".to_string())?;
    match db.get_cf(cf, key.as_bytes()) {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("decode {}: {}", key, e)),
        Ok(None) => Ok(None),
        Err(e) => Err(format!("db get {}: {}", key, e)),
    }
}

fn save_cursor_snapshot<T: Serialize>(db: &DB, key: &str, value: &T) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_CURSORS)
        .ok_or_else(|| "missing cursors cf".to_string())?;
    let bytes = serde_json::to_vec(value).map_err(|e| format!("encode {}: {}", key, e))?;
    db.put_cf(cf, key.as_bytes(), bytes)
        .map_err(|e| format!("db put {}: {}", key, e))
}

pub(crate) fn load_withdrawal_rate_state(db: &DB) -> Result<WithdrawalRateState, String> {
    let reference_now = std::time::Instant::now();
    let reference_secs = current_unix_secs_lossy();
    match load_cursor_snapshot::<WithdrawalRateStateSnapshot>(db, CURSOR_WITHDRAWAL_RATE_STATE)? {
        Some(snapshot) => Ok(WithdrawalRateState::from_snapshot(
            snapshot,
            reference_now,
            reference_secs,
        )),
        None => Ok(WithdrawalRateState::new()),
    }
}

pub(crate) fn persist_withdrawal_rate_state(
    db: &DB,
    state: &WithdrawalRateState,
) -> Result<(), String> {
    let reference_now = std::time::Instant::now();
    let reference_secs = current_unix_secs_lossy();
    save_cursor_snapshot(
        db,
        CURSOR_WITHDRAWAL_RATE_STATE,
        &state.snapshot(reference_now, reference_secs),
    )
}

pub(crate) fn load_deposit_rate_state(db: &DB) -> Result<DepositRateState, String> {
    let reference_now = std::time::Instant::now();
    let reference_secs = current_unix_secs_lossy();
    match load_cursor_snapshot::<DepositRateStateSnapshot>(db, CURSOR_DEPOSIT_RATE_STATE)? {
        Some(snapshot) => Ok(DepositRateState::from_snapshot(
            snapshot,
            reference_now,
            reference_secs,
        )),
        None => Ok(DepositRateState::new()),
    }
}

pub(crate) fn persist_deposit_rate_state(db: &DB, state: &DepositRateState) -> Result<(), String> {
    let reference_now = std::time::Instant::now();
    let reference_secs = current_unix_secs_lossy();
    save_cursor_snapshot(
        db,
        CURSOR_DEPOSIT_RATE_STATE,
        &state.snapshot(reference_now, reference_secs),
    )
}
