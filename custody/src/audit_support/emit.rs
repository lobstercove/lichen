use super::record::record_audit_event_ext;
use super::*;

/// Convenience: emit a custody event with full state context (DB + broadcast channel).
pub(in super::super) fn emit_custody_event(
    state: &CustodyState,
    event_type: &str,
    entity_id: &str,
    deposit_id: Option<&str>,
    tx_hash: Option<&str>,
    data: Option<&Value>,
) {
    if let Err(error) = record_audit_event_ext(
        &state.db,
        event_type,
        entity_id,
        deposit_id,
        tx_hash,
        data,
        Some(&state.event_tx),
    ) {
        tracing::warn!("audit event failed: {}", error);
    }
}
