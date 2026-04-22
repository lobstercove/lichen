use super::*;

pub(super) fn store_webhook(db: &DB, webhook: &WebhookRegistration) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_WEBHOOKS)
        .ok_or_else(|| "missing webhooks cf".to_string())?;
    let bytes = serde_json::to_vec(webhook).map_err(|error| format!("encode: {}", error))?;
    db.put_cf(cf, webhook.id.as_bytes(), bytes)
        .map_err(|error| format!("db put: {}", error))
}

pub(super) fn list_all_webhooks(db: &DB) -> Result<Vec<WebhookRegistration>, String> {
    let cf = db
        .cf_handle(CF_WEBHOOKS)
        .ok_or_else(|| "missing webhooks cf".to_string())?;
    let mut webhooks = Vec::new();
    let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for item in iter {
        let (_, value) = item.map_err(|error| format!("db iter: {}", error))?;
        if let Ok(webhook) = serde_json::from_slice::<WebhookRegistration>(&value) {
            webhooks.push(webhook);
        }
    }
    Ok(webhooks)
}

pub(super) fn remove_webhook(db: &DB, webhook_id: &str) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_WEBHOOKS)
        .ok_or_else(|| "missing webhooks cf".to_string())?;
    db.delete_cf(cf, webhook_id.as_bytes())
        .map_err(|error| format!("db delete: {}", error))
}
