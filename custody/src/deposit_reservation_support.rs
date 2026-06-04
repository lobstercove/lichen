use super::*;

const ACTIVE_DEPOSIT_ROUTE_PREFIX: &str = "active_deposit_route:";

fn active_deposit_route_key(user_id: &str, chain: &str, asset: &str) -> String {
    format!(
        "{}{}:{}:{}",
        ACTIVE_DEPOSIT_ROUTE_PREFIX, user_id, chain, asset
    )
}

pub(crate) fn is_active_deposit_status(status: &str) -> bool {
    matches!(status, "issued" | "pending")
}

fn deposit_is_unexpired(record: &DepositRequest, now: i64, ttl_secs: i64) -> bool {
    ttl_secs <= 0 || record.created_at >= now.saturating_sub(ttl_secs)
}

fn reusable_active_deposit(
    record: &DepositRequest,
    user_id: &str,
    chain: &str,
    asset: &str,
    now: i64,
    ttl_secs: i64,
) -> bool {
    record.user_id == user_id
        && record.chain == chain
        && record.asset == asset
        && is_active_deposit_status(&record.status)
        && deposit_is_unexpired(record, now, ttl_secs)
}

pub(crate) fn put_active_deposit_route_index(
    batch: &mut WriteBatch,
    indexes_cf: &rocksdb::ColumnFamily,
    record: &DepositRequest,
) {
    if is_active_deposit_status(&record.status) {
        batch.put_cf(
            indexes_cf,
            active_deposit_route_key(&record.user_id, &record.chain, &record.asset).as_bytes(),
            record.deposit_id.as_bytes(),
        );
    }
}

pub(crate) fn delete_active_deposit_route_index_from_batch(
    batch: &mut WriteBatch,
    indexes_cf: &rocksdb::ColumnFamily,
    record: &DepositRequest,
) {
    batch.delete_cf(
        indexes_cf,
        active_deposit_route_key(&record.user_id, &record.chain, &record.asset).as_bytes(),
    );
}

pub(crate) fn clear_active_deposit_route_index(
    db: &DB,
    record: &DepositRequest,
) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_INDEXES)
        .ok_or_else(|| "missing indexes cf".to_string())?;
    db.delete_cf(
        cf,
        active_deposit_route_key(&record.user_id, &record.chain, &record.asset).as_bytes(),
    )
    .map_err(|error| format!("active deposit route delete: {}", error))
}

fn find_active_deposit_by_scan(
    db: &DB,
    user_id: &str,
    chain: &str,
    asset: &str,
    now: i64,
    ttl_secs: i64,
) -> Result<Option<DepositRequest>, String> {
    let mut best: Option<DepositRequest> = None;
    for status in ["issued", "pending"] {
        let ids = list_ids_by_status_index(db, "deposits", status)?;
        for id in ids {
            let Some(record) = fetch_deposit(db, &id)? else {
                continue;
            };
            if !reusable_active_deposit(&record, user_id, chain, asset, now, ttl_secs) {
                continue;
            }
            let replace = best
                .as_ref()
                .map(|current| record.created_at > current.created_at)
                .unwrap_or(true);
            if replace {
                best = Some(record);
            }
        }
    }
    Ok(best)
}

pub(crate) fn find_reusable_active_deposit(
    db: &DB,
    user_id: &str,
    chain: &str,
    asset: &str,
    now: i64,
    ttl_secs: i64,
) -> Result<Option<CreateDepositResponse>, String> {
    let cf = db
        .cf_handle(CF_INDEXES)
        .ok_or_else(|| "missing indexes cf".to_string())?;
    let route_key = active_deposit_route_key(user_id, chain, asset);

    if let Some(bytes) = db
        .get_cf(cf, route_key.as_bytes())
        .map_err(|error| format!("active deposit route get: {}", error))?
    {
        let deposit_id = String::from_utf8(bytes.to_vec())
            .map_err(|error| format!("active deposit route id utf8: {}", error))?;
        if let Some(record) = fetch_deposit(db, &deposit_id)? {
            if reusable_active_deposit(&record, user_id, chain, asset, now, ttl_secs) {
                return Ok(Some(CreateDepositResponse {
                    deposit_id: record.deposit_id,
                    address: record.address,
                }));
            }
        }
        db.delete_cf(cf, route_key.as_bytes())
            .map_err(|error| format!("stale active deposit route delete: {}", error))?;
    }

    let Some(record) = find_active_deposit_by_scan(db, user_id, chain, asset, now, ttl_secs)?
    else {
        return Ok(None);
    };
    db.put_cf(cf, route_key.as_bytes(), record.deposit_id.as_bytes())
        .map_err(|error| format!("active deposit route backfill: {}", error))?;
    Ok(Some(CreateDepositResponse {
        deposit_id: record.deposit_id,
        address: record.address,
    }))
}
