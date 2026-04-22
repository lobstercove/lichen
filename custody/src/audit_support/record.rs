use super::*;

pub(in super::super) fn record_audit_event(
    db: &DB,
    event_type: &str,
    entity_id: &str,
    deposit_id: Option<&str>,
    tx_hash: Option<&str>,
) -> Result<(), String> {
    record_audit_event_ext(db, event_type, entity_id, deposit_id, tx_hash, None, None)
}

/// Extended audit event recorder — also emits to webhook/WS broadcast channel.
/// Call this variant from code paths that have access to `CustodyState`.
pub(in super::super) fn record_audit_event_ext(
    db: &DB,
    event_type: &str,
    entity_id: &str,
    deposit_id: Option<&str>,
    tx_hash: Option<&str>,
    data: Option<&Value>,
    event_tx: Option<&broadcast::Sender<CustodyWebhookEvent>>,
) -> Result<(), String> {
    let event_id = Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().timestamp();
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    let cf = db
        .cf_handle(CF_AUDIT_EVENTS)
        .ok_or_else(|| "missing audit_events cf".to_string())?;
    let index_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_TIME)
        .ok_or_else(|| "missing audit_events_by_time cf".to_string())?;
    let type_index_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_TYPE_TIME)
        .ok_or_else(|| "missing audit_events_by_type_time cf".to_string())?;
    let entity_index_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_ENTITY_TIME)
        .ok_or_else(|| "missing audit_events_by_entity_time cf".to_string())?;
    let tx_index_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_TX_TIME)
        .ok_or_else(|| "missing audit_events_by_tx_time cf".to_string())?;
    let payload = serde_json::json!({
        "event_id": &event_id,
        "event_type": event_type,
        "entity_id": entity_id,
        "deposit_id": deposit_id,
        "tx_hash": tx_hash,
        "data": data,
        "timestamp": timestamp,
        "timestamp_ms": timestamp_ms,
    });
    let bytes = serde_json::to_vec(&payload).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, event_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))?;

    // Scale-safe read index for event history pagination.
    // Key format preserves chronological ordering in RocksDB iteration.
    let index_key = format!("{:020}:{}", timestamp_ms.max(0), event_id);
    db.put_cf(index_cf, index_key.as_bytes(), event_id.as_bytes())
        .map_err(|e| format!("db put index: {}", e))?;
    let type_index_key = format!(
        "type:{}:{:020}:{}",
        event_type,
        timestamp_ms.max(0),
        event_id
    );
    db.put_cf(
        type_index_cf,
        type_index_key.as_bytes(),
        event_id.as_bytes(),
    )
    .map_err(|e| format!("db put type index: {}", e))?;
    let entity = if entity_id.is_empty() {
        "unknown"
    } else {
        entity_id
    };
    let entity_index_key = format!("entity:{}:{:020}:{}", entity, timestamp_ms.max(0), event_id);
    db.put_cf(
        entity_index_cf,
        entity_index_key.as_bytes(),
        event_id.as_bytes(),
    )
    .map_err(|e| format!("db put entity index: {}", e))?;
    if let Some(hash) = tx_hash.filter(|h| !h.is_empty()) {
        let tx_index_key = format!("tx:{}:{:020}:{}", hash, timestamp_ms.max(0), event_id);
        db.put_cf(tx_index_cf, tx_index_key.as_bytes(), event_id.as_bytes())
            .map_err(|e| format!("db put tx index: {}", e))?;
    }

    // Emit to broadcast channel for webhooks + WebSocket subscribers
    if let Some(tx) = event_tx {
        let event = CustodyWebhookEvent {
            event_id,
            event_type: event_type.to_string(),
            entity_id: entity_id.to_string(),
            deposit_id: deposit_id.map(|s| s.to_string()),
            tx_hash: tx_hash.map(|s| s.to_string()),
            data: data.cloned(),
            timestamp,
        };
        // Best-effort: if no receivers are listening, that's fine
        drop(tx.send(event));
    }

    Ok(())
}
