use super::*;

mod processing;
mod reservation;
mod submission;

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn burn_signature_index_key(burn_tx_signature: &str) -> String {
    reservation::burn_signature_index_key(burn_tx_signature)
}

pub(super) async fn submit_pending_burn_signature(
    state: &CustodyState,
    job_id: &str,
    burn_tx_signature: String,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    submission::submit_pending_burn_signature(state, job_id, burn_tx_signature).await
}

pub(super) async fn process_pending_burn_withdrawals(state: &CustodyState) -> Result<(), String> {
    processing::process_pending_burn_withdrawals(state).await
}
