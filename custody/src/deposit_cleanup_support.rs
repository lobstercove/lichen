use super::*;

/// Background loop: prunes expired, unfunded deposit addresses.
/// Only deposits in "issued" status (never received funds) older than
/// `deposit_ttl_secs` are marked "expired" and their address index removed.
pub(super) async fn deposit_cleanup_loop(state: CustodyState) {
    loop {
        sleep(Duration::from_secs(600)).await;

        let ttl = state.config.deposit_ttl_secs;
        if ttl <= 0 {
            continue;
        }
        let cutoff = chrono::Utc::now().timestamp() - ttl;

        let issued_ids = match list_ids_by_status_index(&state.db, "deposits", "issued") {
            Ok(ids) => ids,
            Err(_) => continue,
        };

        let mut expired_ids = Vec::new();
        for id in &issued_ids {
            if let Ok(Some(record)) = fetch_deposit(&state.db, id) {
                if record.status == "issued" && record.created_at < cutoff {
                    expired_ids.push((id.clone(), record.address.clone()));
                }
            }
        }

        if expired_ids.is_empty() && issued_ids.is_empty() {
            if let Some(cf) = state.db.cf_handle(CF_DEPOSITS) {
                let iter = state.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
                for item in iter {
                    let (key, value) = match item {
                        Ok(kv) => kv,
                        Err(_) => continue,
                    };
                    let record: DepositRequest = match serde_json::from_slice(&value) {
                        Ok(record) => record,
                        Err(_) => continue,
                    };
                    if record.status == "issued" && record.created_at < cutoff {
                        expired_ids.push((
                            String::from_utf8_lossy(&key).to_string(),
                            record.address.clone(),
                        ));
                    }
                }
            }
        }

        let count = expired_ids.len();
        for (deposit_id, address) in &expired_ids {
            if let Some(cf) = state.db.cf_handle(CF_DEPOSITS) {
                if let Ok(Some(value)) = state.db.get_cf(&cf, deposit_id.as_bytes()) {
                    if let Ok(mut record) = serde_json::from_slice::<DepositRequest>(&value) {
                        let old_status = record.status.clone();
                        record.status = "expired".to_string();
                        if let Ok(json) = serde_json::to_vec(&record) {
                            if let Err(error) = state.db.put_cf(&cf, deposit_id.as_bytes(), &json) {
                                tracing::error!("Failed to write custody DB: {error}");
                            }
                            if let Err(error) = update_status_index(
                                &state.db,
                                "deposits",
                                &old_status,
                                "expired",
                                deposit_id,
                            ) {
                                tracing::error!("Failed update_status_index: {error}");
                            }
                        }
                    }
                }
            }

            if let Some(addr_cf) = state.db.cf_handle(CF_ADDRESS_INDEX) {
                if let Err(error) = state.db.delete_cf(&addr_cf, address.as_bytes()) {
                    tracing::error!("Failed to delete custody DB entry: {error}");
                }
            }

            if let Some(bal_cf) = state.db.cf_handle(CF_ADDRESS_BALANCES) {
                if let Err(error) = state.db.delete_cf(&bal_cf, address.as_bytes()) {
                    tracing::error!("Failed to delete custody DB entry: {error}");
                }
            }

            if let Some(tok_cf) = state.db.cf_handle(CF_TOKEN_BALANCES) {
                let prefix = format!("{}:", address);
                let iter = state.db.prefix_iterator_cf(&tok_cf, prefix.as_bytes());
                for (key, _) in iter.flatten() {
                    if key.starts_with(prefix.as_bytes()) {
                        if let Err(error) = state.db.delete_cf(&tok_cf, &key) {
                            tracing::error!("Failed to delete custody DB entry: {error}");
                        }
                    } else {
                        break;
                    }
                }
            }

            if let Some(evt_cf) = state.db.cf_handle(CF_DEPOSIT_EVENTS) {
                let dedup_prefix = format!("dedup:{}:", deposit_id);
                let iter = state
                    .db
                    .prefix_iterator_cf(&evt_cf, dedup_prefix.as_bytes());
                for (key, _) in iter.flatten() {
                    if key.starts_with(dedup_prefix.as_bytes()) {
                        if let Err(error) = state.db.delete_cf(&evt_cf, &key) {
                            tracing::error!("Failed to delete custody DB entry: {error}");
                        }
                    } else {
                        break;
                    }
                }

                let iter = state.db.iterator_cf(&evt_cf, rocksdb::IteratorMode::Start);
                for (key, value) in iter.flatten() {
                    if key.starts_with(b"dedup:") {
                        continue;
                    }
                    if let Ok(event) = serde_json::from_slice::<DepositEvent>(&value) {
                        if event.deposit_id == *deposit_id {
                            if let Err(error) = state.db.delete_cf(&evt_cf, &key) {
                                tracing::error!("Failed to delete custody DB entry: {error}");
                            }
                        }
                    }
                }
            }
        }

        if count > 0 {
            for (deposit_id, address) in &expired_ids {
                emit_custody_event(
                    &state,
                    "deposit.expired",
                    deposit_id,
                    Some(deposit_id),
                    None,
                    Some(&serde_json::json!({
                        "address": address,
                        "ttl_secs": ttl
                    })),
                );
            }
            info!(
                "deposit cleanup: expired {} unfunded deposits older than {}s",
                count, ttl
            );
        }
    }
}
