use super::*;

pub(super) fn store_deposit_event(db: &DB, event: &DepositEvent) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_DEPOSIT_EVENTS)
        .ok_or_else(|| "missing deposit_events cf".to_string())?;
    let bytes = serde_json::to_vec(event).map_err(|error| format!("encode: {}", error))?;
    db.put_cf(cf, event.event_id.as_bytes(), bytes)
        .map_err(|error| format!("db put: {}", error))?;
    let dedup_key = format!("dedup:{}:{}", event.deposit_id, event.tx_hash);
    db.put_cf(cf, dedup_key.as_bytes(), b"1")
        .map_err(|error| format!("dedup marker: {}", error))?;
    Ok(())
}

pub(super) fn deposit_event_already_processed(db: &DB, deposit_id: &str, tx_hash: &str) -> bool {
    let cf = match db.cf_handle(CF_DEPOSIT_EVENTS) {
        Some(cf) => cf,
        None => return false,
    };
    let dedup_key = format!("dedup:{}:{}", deposit_id, tx_hash);
    matches!(db.get_cf(cf, dedup_key.as_bytes()), Ok(Some(_)))
}

pub(super) fn update_deposit_status(db: &DB, deposit_id: &str, status: &str) -> Result<(), String> {
    let mut record = fetch_deposit(db, deposit_id)
        .map_err(|error| format!("fetch deposit: {}", error))?
        .ok_or_else(|| "deposit not found".to_string())?;
    let old_status = record.status.clone();
    record.status = status.to_string();
    store_deposit(db, &record)?;
    if let Err(error) = update_status_index(db, "deposits", &old_status, status, deposit_id) {
        tracing::error!("Failed update_status_index: {error}");
    }
    Ok(())
}
