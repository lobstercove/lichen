use super::*;

fn list_pending_deposits(db: &DB, chain: &str) -> Result<Vec<DepositRequest>, String> {
    let mut results = Vec::new();
    for status in ["issued", "pending"] {
        let ids = list_ids_by_status_index(db, "deposits", status)?;
        for id in ids {
            if let Some(record) = fetch_deposit(db, &id)? {
                if record.chain == chain {
                    results.push(record);
                }
            }
        }
    }

    if results.is_empty() {
        let cf = db
            .cf_handle(CF_DEPOSITS)
            .ok_or_else(|| "missing deposits cf".to_string())?;
        let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (_, value) = item.map_err(|error| format!("db iter: {}", error))?;
            let record: DepositRequest =
                serde_json::from_slice(&value).map_err(|error| format!("decode: {}", error))?;
            if record.chain == chain && (record.status == "issued" || record.status == "pending") {
                results.push(record);
            }
        }
    }

    Ok(results)
}

pub(super) fn list_pending_deposits_for_chains(
    db: &DB,
    chains: &[&str],
) -> Result<Vec<DepositRequest>, String> {
    let mut results = Vec::new();
    for chain in chains {
        results.extend(list_pending_deposits(db, chain)?);
    }
    Ok(results)
}
