use super::*;

pub(crate) enum DepositObservationMarker {
    AddressBalance { address: String, balance: u128 },
    TokenBalance { key: String, balance: u64 },
    Cursor { key: String, value: u64 },
}

pub(crate) struct DepositObservationWrite {
    pub(crate) event: DepositEvent,
    pub(crate) sweep_job: Option<SweepJob>,
    pub(crate) markers: Vec<DepositObservationMarker>,
}

pub(super) fn deposit_event_already_processed(db: &DB, deposit_id: &str, tx_hash: &str) -> bool {
    let cf = match db.cf_handle(CF_DEPOSIT_EVENTS) {
        Some(cf) => cf,
        None => return false,
    };
    let dedup_key = format!("dedup:{}:{}", deposit_id, tx_hash);
    matches!(db.get_cf(cf, dedup_key.as_bytes()), Ok(Some(_)))
}

pub(crate) fn persist_deposit_observation(
    db: &DB,
    observation: &DepositObservationWrite,
) -> Result<bool, String> {
    persist_deposit_observations(db, std::slice::from_ref(observation), &[])
        .map(|committed| !committed.is_empty())
}

pub(crate) fn persist_deposit_observations(
    db: &DB,
    observations: &[DepositObservationWrite],
    markers: &[DepositObservationMarker],
) -> Result<Vec<usize>, String> {
    let events_cf = db
        .cf_handle(CF_DEPOSIT_EVENTS)
        .ok_or_else(|| "missing deposit_events cf".to_string())?;
    let deposits_cf = db
        .cf_handle(CF_DEPOSITS)
        .ok_or_else(|| "missing deposits cf".to_string())?;
    let status_cf = db
        .cf_handle(CF_STATUS_INDEX)
        .ok_or_else(|| "missing status_index cf".to_string())?;
    let sweep_cf = db
        .cf_handle(CF_SWEEP_JOBS)
        .ok_or_else(|| "missing sweep_jobs cf".to_string())?;
    let address_balance_cf = db
        .cf_handle(CF_ADDRESS_BALANCES)
        .ok_or_else(|| "missing address_balances cf".to_string())?;
    let token_balance_cf = db
        .cf_handle(CF_TOKEN_BALANCES)
        .ok_or_else(|| "missing token_balances cf".to_string())?;
    let cursor_cf = db
        .cf_handle(CF_CURSORS)
        .ok_or_else(|| "missing cursors cf".to_string())?;

    let mut batch = WriteBatch::default();
    let mut committed = Vec::new();
    let mut observed_dedup_keys = BTreeSet::new();
    let mut has_writes = false;

    for (index, observation) in observations.iter().enumerate() {
        let dedup_key = deposit_event_dedup_key(&observation.event);
        if !observed_dedup_keys.insert(dedup_key.clone())
            || matches!(db.get_cf(events_cf, dedup_key.as_bytes()), Ok(Some(_)))
        {
            continue;
        }

        let mut deposit = fetch_deposit(db, &observation.event.deposit_id)
            .map_err(|error| format!("fetch deposit: {}", error))?
            .ok_or_else(|| "deposit not found".to_string())?;
        let old_status = deposit.status.clone();
        let final_status = if observation.sweep_job.is_some() {
            "sweep_queued"
        } else {
            "confirmed"
        };
        deposit.status = final_status.to_string();

        let event_bytes = serde_json::to_vec(&observation.event)
            .map_err(|error| format!("encode deposit event: {}", error))?;
        let deposit_bytes =
            serde_json::to_vec(&deposit).map_err(|error| format!("encode deposit: {}", error))?;

        batch.put_cf(
            events_cf,
            observation.event.event_id.as_bytes(),
            event_bytes,
        );
        batch.put_cf(events_cf, dedup_key.as_bytes(), b"1");
        batch.put_cf(deposits_cf, deposit.deposit_id.as_bytes(), deposit_bytes);
        if old_status != final_status {
            batch.delete_cf(
                status_cf,
                status_index_key("deposits", &old_status, &deposit.deposit_id).as_bytes(),
            );
        }
        batch.put_cf(
            status_cf,
            status_index_key("deposits", final_status, &deposit.deposit_id).as_bytes(),
            b"",
        );

        if let Some(job) = &observation.sweep_job {
            let job_bytes =
                serde_json::to_vec(job).map_err(|error| format!("encode sweep job: {}", error))?;
            batch.put_cf(sweep_cf, job.job_id.as_bytes(), job_bytes);
            batch.put_cf(
                status_cf,
                status_index_key("sweep", &job.status, &job.job_id).as_bytes(),
                b"",
            );
        }

        put_deposit_observation_markers(
            &mut batch,
            address_balance_cf,
            token_balance_cf,
            cursor_cf,
            &observation.markers,
        );
        committed.push(index);
        has_writes = true;
    }

    put_deposit_observation_markers(
        &mut batch,
        address_balance_cf,
        token_balance_cf,
        cursor_cf,
        markers,
    );
    has_writes |= !markers.is_empty();

    if has_writes {
        db.write(batch)
            .map_err(|error| format!("db write deposit observation: {}", error))?;
    }

    Ok(committed)
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

fn deposit_event_dedup_key(event: &DepositEvent) -> String {
    format!("dedup:{}:{}", event.deposit_id, event.tx_hash)
}

fn status_index_key(table: &str, status: &str, id: &str) -> String {
    format!("status:{}:{}:{}", table, status, id)
}

fn put_deposit_observation_markers(
    batch: &mut WriteBatch,
    address_balance_cf: &rocksdb::ColumnFamily,
    token_balance_cf: &rocksdb::ColumnFamily,
    cursor_cf: &rocksdb::ColumnFamily,
    markers: &[DepositObservationMarker],
) {
    for marker in markers {
        match marker {
            DepositObservationMarker::AddressBalance { address, balance } => {
                batch.put_cf(
                    address_balance_cf,
                    address.as_bytes(),
                    balance.to_le_bytes(),
                );
            }
            DepositObservationMarker::TokenBalance { key, balance } => {
                batch.put_cf(token_balance_cf, key.as_bytes(), balance.to_le_bytes());
            }
            DepositObservationMarker::Cursor { key, value } => {
                batch.put_cf(cursor_cf, key.as_bytes(), value.to_le_bytes());
            }
        }
    }
}
