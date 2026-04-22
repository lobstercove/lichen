use super::*;

pub(crate) fn record_tx_intent(
    db: &DB,
    tx_type: &str,
    job_id: &str,
    chain: &str,
) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_TX_INTENTS)
        .ok_or_else(|| "missing tx_intents cf".to_string())?;
    let key = format!("intent:{}:{}", tx_type, job_id);
    let payload = serde_json::json!({
        "tx_type": tx_type,
        "job_id": job_id,
        "chain": chain,
        "created_at": chrono::Utc::now().timestamp(),
    });
    let bytes = serde_json::to_vec(&payload).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, key.as_bytes(), bytes)
        .map_err(|e| format!("intent put: {}", e))
}

pub(crate) fn clear_tx_intent(db: &DB, tx_type: &str, job_id: &str) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_TX_INTENTS)
        .ok_or_else(|| "missing tx_intents cf".to_string())?;
    let key = format!("intent:{}:{}", tx_type, job_id);
    db.delete_cf(cf, key.as_bytes())
        .map_err(|e| format!("intent delete: {}", e))
}

pub(crate) fn recover_stale_intents(db: &DB) {
    let cf = match db.cf_handle(CF_TX_INTENTS) {
        Some(cf) => cf,
        None => return,
    };
    let iter = db.prefix_iterator_cf(cf, b"intent:");
    let mut count = 0u32;
    for item in iter {
        let (key, value) = match item {
            Ok(kv) => kv,
            Err(_) => continue,
        };
        let key_str = std::str::from_utf8(&key).unwrap_or("?");
        if !key_str.starts_with("intent:") {
            break;
        }
        let payload_str = std::str::from_utf8(&value).unwrap_or("{}");
        tracing::error!(
            "⚠️  STALE TX INTENT (possible crash during broadcast): key={} payload={}. \
             Manual reconciliation required — check chain state for this job.",
            key_str,
            payload_str
        );
        count += 1;
    }
    if count > 0 {
        tracing::error!(
            "🚨 Found {} stale TX intents from previous run. \
             These indicate broadcasts that may or may not have reached the chain. \
             Review each above and reconcile against on-chain state before proceeding.",
            count
        );
    }
}
