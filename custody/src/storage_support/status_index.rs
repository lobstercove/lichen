use super::*;

pub(crate) fn set_status_index(
    db: &DB,
    table: &str,
    status: &str,
    job_id: &str,
) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_STATUS_INDEX)
        .ok_or_else(|| "missing status_index cf".to_string())?;
    let key = format!("status:{}:{}:{}", table, status, job_id);
    db.put_cf(cf, key.as_bytes(), b"")
        .map_err(|e| format!("status index put: {}", e))
}

fn remove_status_index(db: &DB, table: &str, status: &str, job_id: &str) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_STATUS_INDEX)
        .ok_or_else(|| "missing status_index cf".to_string())?;
    let key = format!("status:{}:{}:{}", table, status, job_id);
    db.delete_cf(cf, key.as_bytes())
        .map_err(|e| format!("status index delete: {}", e))
}

pub(crate) fn update_status_index(
    db: &DB,
    table: &str,
    old_status: &str,
    new_status: &str,
    job_id: &str,
) -> Result<(), String> {
    if old_status != new_status {
        if let Err(error) = remove_status_index(db, table, old_status, job_id) {
            tracing::error!("Failed remove_status_index: {error}");
        }
        set_status_index(db, table, new_status, job_id)?;
    }
    Ok(())
}

pub(crate) fn list_ids_by_status_index(
    db: &DB,
    table: &str,
    status: &str,
) -> Result<Vec<String>, String> {
    let cf = db
        .cf_handle(CF_STATUS_INDEX)
        .ok_or_else(|| "missing status_index cf".to_string())?;
    let prefix = format!("status:{}:{}:", table, status);
    let prefix_bytes = prefix.as_bytes();
    let mut ids = Vec::new();
    let iter = db.prefix_iterator_cf(cf, prefix_bytes);
    for item in iter {
        let (key, _) = item.map_err(|e| format!("db iter: {}", e))?;
        let key_str = std::str::from_utf8(&key).unwrap_or("");
        if !key_str.starts_with(&prefix) {
            break;
        }
        if let Some(job_id) = key_str.strip_prefix(&prefix) {
            ids.push(job_id.to_string());
        }
    }
    Ok(ids)
}
