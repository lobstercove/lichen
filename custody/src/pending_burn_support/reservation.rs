use super::*;

pub(super) fn burn_signature_index_key(burn_tx_signature: &str) -> String {
    format!("burn_sig:{}", burn_tx_signature)
}

pub(super) fn reserve_burn_signature(
    db: &DB,
    burn_tx_signature: &str,
    job_id: &str,
) -> Result<(), String> {
    let idx_cf = db
        .cf_handle(CF_INDEXES)
        .ok_or_else(|| "missing indexes cf".to_string())?;
    let key = burn_signature_index_key(burn_tx_signature);

    if let Some(existing) = db
        .get_cf(idx_cf, key.as_bytes())
        .map_err(|error| format!("db get: {}", error))?
    {
        let existing_job_id = String::from_utf8_lossy(&existing);
        if existing_job_id != job_id {
            return Err(format!(
                "burn_tx_signature already used by withdrawal {}",
                existing_job_id
            ));
        }
    }

    db.put_cf(idx_cf, key.as_bytes(), job_id.as_bytes())
        .map_err(|error| format!("db put: {}", error))
}

pub(super) fn release_burn_signature_reservation(
    db: &DB,
    burn_tx_signature: &str,
    job_id: &str,
) -> Result<(), String> {
    let idx_cf = db
        .cf_handle(CF_INDEXES)
        .ok_or_else(|| "missing indexes cf".to_string())?;
    let key = burn_signature_index_key(burn_tx_signature);

    if let Some(existing) = db
        .get_cf(idx_cf, key.as_bytes())
        .map_err(|error| format!("db get: {}", error))?
    {
        if existing.as_slice() == job_id.as_bytes() {
            db.delete_cf(idx_cf, key.as_bytes())
                .map_err(|error| format!("db delete: {}", error))?;
        }
    }

    Ok(())
}
