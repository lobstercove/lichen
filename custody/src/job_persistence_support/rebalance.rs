use super::*;

pub(crate) fn store_rebalance_job(db: &DB, job: &RebalanceJob) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_REBALANCE_JOBS)
        .ok_or_else(|| "missing rebalance_jobs cf".to_string())?;
    if let Ok(Some(old_bytes)) = db.get_cf(cf, job.job_id.as_bytes()) {
        if let Ok(old_job) = serde_json::from_slice::<RebalanceJob>(&old_bytes) {
            if let Err(error) =
                update_status_index(db, "rebalance", &old_job.status, &job.status, &job.job_id)
            {
                tracing::error!("Failed update_status_index: {error}");
            }
        }
    } else if let Err(error) = set_status_index(db, "rebalance", &job.status, &job.job_id) {
        tracing::error!("Failed set_status_index: {error}");
    }
    let bytes = serde_json::to_vec(job).map_err(|error| format!("encode: {}", error))?;
    db.put_cf(cf, job.job_id.as_bytes(), bytes)
        .map_err(|error| format!("db put: {}", error))
}

pub(crate) fn list_rebalance_jobs_by_status(
    db: &DB,
    status: &str,
) -> Result<Vec<RebalanceJob>, String> {
    let ids = list_ids_by_status_index(db, "rebalance", status)?;
    if !ids.is_empty() {
        let cf = db
            .cf_handle(CF_REBALANCE_JOBS)
            .ok_or_else(|| "missing rebalance_jobs cf".to_string())?;
        let mut results = Vec::new();
        for id in ids {
            if let Ok(Some(bytes)) = db.get_cf(cf, id.as_bytes()) {
                if let Ok(record) = serde_json::from_slice::<RebalanceJob>(&bytes) {
                    if record.status == status {
                        results.push(record);
                    }
                }
            }
        }
        return Ok(results);
    }

    let cf = db
        .cf_handle(CF_REBALANCE_JOBS)
        .ok_or_else(|| "missing rebalance_jobs cf".to_string())?;
    let mut results = Vec::new();
    let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for item in iter {
        let (_, value) = item.map_err(|error| format!("db iter: {}", error))?;
        let record: RebalanceJob =
            serde_json::from_slice(&value).map_err(|error| format!("decode: {}", error))?;
        if record.status == status {
            results.push(record);
        }
    }
    Ok(results)
}
