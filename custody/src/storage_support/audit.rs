use super::*;

pub(crate) fn backfill_audit_event_indexes(db: &DB) -> Result<(), String> {
    let events_cf = db
        .cf_handle(CF_AUDIT_EVENTS)
        .ok_or_else(|| "missing audit_events cf".to_string())?;
    let time_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_TIME)
        .ok_or_else(|| "missing audit_events_by_time cf".to_string())?;
    let type_time_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_TYPE_TIME)
        .ok_or_else(|| "missing audit_events_by_type_time cf".to_string())?;
    let entity_time_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_ENTITY_TIME)
        .ok_or_else(|| "missing audit_events_by_entity_time cf".to_string())?;
    let tx_time_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_TX_TIME)
        .ok_or_else(|| "missing audit_events_by_tx_time cf".to_string())?;

    let mut scanned = 0usize;
    let mut inserted = 0usize;

    for item in db.iterator_cf(events_cf, rocksdb::IteratorMode::Start) {
        let (key, value) = item.map_err(|e| format!("db iter: {}", e))?;
        let event: Value = match serde_json::from_slice(&value) {
            Ok(value) => value,
            Err(_) => continue,
        };
        scanned += 1;

        let key_id = std::str::from_utf8(&key).unwrap_or("").to_string();
        let event_id = event
            .get("event_id")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .unwrap_or(&key_id)
            .to_string();
        let event_type = event
            .get("event_type")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("unknown")
            .to_string();
        let entity_id = event
            .get("entity_id")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("unknown")
            .to_string();
        let tx_hash = event
            .get("tx_hash")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string());
        let ts_ms = event
            .get("timestamp_ms")
            .and_then(|v| v.as_i64())
            .or_else(|| {
                event
                    .get("timestamp")
                    .and_then(|v| v.as_i64())
                    .map(|seconds| seconds.saturating_mul(1000))
            })
            .unwrap_or(0)
            .max(0);

        let time_key = format!("{:020}:{}", ts_ms, event_id);
        if matches!(db.get_cf(time_cf, time_key.as_bytes()), Ok(None)) {
            db.put_cf(time_cf, time_key.as_bytes(), event_id.as_bytes())
                .map_err(|e| format!("time index put: {}", e))?;
            inserted += 1;
        }

        let type_key = format!("type:{}:{:020}:{}", event_type, ts_ms, event_id);
        if matches!(db.get_cf(type_time_cf, type_key.as_bytes()), Ok(None)) {
            db.put_cf(type_time_cf, type_key.as_bytes(), event_id.as_bytes())
                .map_err(|e| format!("type index put: {}", e))?;
            inserted += 1;
        }

        let entity_key = format!("entity:{}:{:020}:{}", entity_id, ts_ms, event_id);
        if matches!(db.get_cf(entity_time_cf, entity_key.as_bytes()), Ok(None)) {
            db.put_cf(entity_time_cf, entity_key.as_bytes(), event_id.as_bytes())
                .map_err(|e| format!("entity index put: {}", e))?;
            inserted += 1;
        }

        if let Some(tx_hash) = tx_hash {
            let tx_key = format!("tx:{}:{:020}:{}", tx_hash, ts_ms, event_id);
            if matches!(db.get_cf(tx_time_cf, tx_key.as_bytes()), Ok(None)) {
                db.put_cf(tx_time_cf, tx_key.as_bytes(), event_id.as_bytes())
                    .map_err(|e| format!("tx index put: {}", e))?;
                inserted += 1;
            }
        }
    }

    if scanned > 0 {
        tracing::info!(
            "audit event index backfill complete: scanned={}, inserted={}",
            scanned,
            inserted
        );
    }

    Ok(())
}
