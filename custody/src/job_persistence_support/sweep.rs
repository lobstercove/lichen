use super::*;

pub(crate) fn list_sweep_jobs_by_status(db: &DB, status: &str) -> Result<Vec<SweepJob>, String> {
    let ids = list_ids_by_status_index(db, "sweep", status)?;
    if !ids.is_empty() {
        let cf = db
            .cf_handle(CF_SWEEP_JOBS)
            .ok_or_else(|| "missing sweep_jobs cf".to_string())?;
        let mut results = Vec::new();
        for id in ids {
            if let Ok(Some(bytes)) = db.get_cf(cf, id.as_bytes()) {
                if let Ok(record) = serde_json::from_slice::<SweepJob>(&bytes) {
                    if record.status == status {
                        results.push(record);
                    }
                }
            }
        }
        return Ok(results);
    }

    let cf = db
        .cf_handle(CF_SWEEP_JOBS)
        .ok_or_else(|| "missing sweep_jobs cf".to_string())?;
    let mut results = Vec::new();
    let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for item in iter {
        let (_, value) = item.map_err(|error| format!("db iter: {}", error))?;
        let record: SweepJob =
            serde_json::from_slice(&value).map_err(|error| format!("decode: {}", error))?;
        if record.status == status {
            results.push(record);
        }
    }
    Ok(results)
}

pub(crate) fn store_sweep_job(db: &DB, job: &SweepJob) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_SWEEP_JOBS)
        .ok_or_else(|| "missing sweep_jobs cf".to_string())?;
    if let Ok(Some(old_bytes)) = db.get_cf(cf, job.job_id.as_bytes()) {
        if let Ok(old_job) = serde_json::from_slice::<SweepJob>(&old_bytes) {
            if let Err(error) =
                update_status_index(db, "sweep", &old_job.status, &job.status, &job.job_id)
            {
                tracing::error!("Failed update_status_index: {error}");
            }
        }
    } else if let Err(error) = set_status_index(db, "sweep", &job.status, &job.job_id) {
        tracing::error!("Failed set_status_index: {error}");
    }
    let bytes = serde_json::to_vec(job).map_err(|error| format!("encode: {}", error))?;
    db.put_cf(cf, job.job_id.as_bytes(), bytes)
        .map_err(|error| format!("db put: {}", error))
}

pub(crate) fn enqueue_sweep_job(db: &DB, job: &SweepJob) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_SWEEP_JOBS)
        .ok_or_else(|| "missing sweep_jobs cf".to_string())?;
    let bytes = serde_json::to_vec(job).map_err(|error| format!("encode: {}", error))?;
    db.put_cf(cf, job.job_id.as_bytes(), bytes)
        .map_err(|error| format!("db put: {}", error))?;
    if let Err(error) = set_status_index(db, "sweep", &job.status, &job.job_id) {
        tracing::error!("Failed set_status_index: {error}");
    }
    Ok(())
}

pub(crate) fn count_sweep_jobs(db: &DB) -> Result<StatusCounts, String> {
    let mut counts = StatusCounts {
        total: 0,
        by_status: BTreeMap::new(),
    };
    for status in &[
        "queued",
        "signing",
        "signed",
        "sweep_submitted",
        "sweep_confirmed",
        "permanently_failed",
        "failed",
    ] {
        let ids = list_ids_by_status_index(db, "sweep", status)?;
        let count = ids.len();
        if count > 0 {
            counts.total += count;
            counts.by_status.insert(status.to_string(), count);
        }
    }
    if counts.total == 0 {
        let cf = db
            .cf_handle(CF_SWEEP_JOBS)
            .ok_or_else(|| "missing sweep_jobs cf".to_string())?;
        let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (_, value) = item.map_err(|error| format!("db iter: {}", error))?;
            let record: SweepJob =
                serde_json::from_slice(&value).map_err(|error| format!("decode: {}", error))?;
            counts.total += 1;
            *counts.by_status.entry(record.status).or_insert(0) += 1;
        }
    }
    Ok(counts)
}
