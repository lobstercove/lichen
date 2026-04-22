use super::*;

pub(super) fn build_reserve_ledger_response(db: &DB) -> Json<Value> {
    let cf = match db.cf_handle(CF_RESERVE_LEDGER) {
        Some(cf) => cf,
        None => return Json(json!({"error": "reserve ledger not available"})),
    };
    let mut entries = Vec::new();
    let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for (_, value) in iter.flatten() {
        if let Ok(entry) = serde_json::from_slice::<ReserveLedgerEntry>(&value) {
            entries.push(json!({
                "chain": entry.chain,
                "asset": entry.asset,
                "amount": entry.amount,
                "last_updated": entry.last_updated,
            }));
        }
    }

    let mut by_chain: std::collections::HashMap<String, (u64, u64)> =
        std::collections::HashMap::new();
    for item in &entries {
        let chain = item["chain"].as_str().unwrap_or("?");
        let asset = item["asset"].as_str().unwrap_or("?");
        let amount = item["amount"].as_u64().unwrap_or(0);
        let entry = by_chain.entry(chain.to_string()).or_insert((0, 0));
        match asset {
            "usdt" => entry.0 = amount,
            "usdc" => entry.1 = amount,
            _ => {}
        }
    }

    let mut ratios = Vec::new();
    for (chain, (usdt, usdc)) in &by_chain {
        let total = usdt + usdc;
        let usdt_pct = if total > 0 {
            (*usdt as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        ratios.push(json!({
            "chain": chain,
            "usdt": usdt,
            "usdc": usdc,
            "total": total,
            "usdt_pct": format!("{:.1}%", usdt_pct),
            "usdc_pct": format!("{:.1}%", 100.0 - usdt_pct),
        }));
    }

    Json(json!({
        "reserves": entries,
        "chain_ratios": ratios,
    }))
}

pub(super) fn get_reserve_balance(db: &DB, chain: &str, asset: &str) -> Result<u64, String> {
    let cf = db
        .cf_handle(CF_RESERVE_LEDGER)
        .ok_or_else(|| "missing reserve_ledger cf".to_string())?;
    let key = format!("{}:{}", chain, asset);
    match db.get_cf(cf, key.as_bytes()) {
        Ok(Some(bytes)) => {
            let entry: ReserveLedgerEntry =
                serde_json::from_slice(&bytes).map_err(|error| format!("decode: {}", error))?;
            Ok(entry.amount)
        }
        Ok(None) => Ok(0),
        Err(error) => Err(format!("db get: {}", error)),
    }
}

pub(super) async fn adjust_reserve_balance(
    db: &DB,
    chain: &str,
    asset: &str,
    amount: u64,
    increment: bool,
) -> Result<(), String> {
    static RESERVE_LOCK: tokio::sync::OnceCell<tokio::sync::Mutex<()>> =
        tokio::sync::OnceCell::const_new();
    let mutex = RESERVE_LOCK
        .get_or_init(|| async { tokio::sync::Mutex::new(()) })
        .await;
    let _guard = mutex.lock().await;

    let cf = db
        .cf_handle(CF_RESERVE_LEDGER)
        .ok_or_else(|| "missing reserve_ledger cf".to_string())?;
    let key = format!("{}:{}", chain, asset);

    let current = match db.get_cf(cf, key.as_bytes()) {
        Ok(Some(bytes)) => {
            let entry: ReserveLedgerEntry =
                serde_json::from_slice(&bytes).map_err(|error| format!("decode: {}", error))?;
            entry.amount
        }
        Ok(None) => 0,
        Err(error) => return Err(format!("db get: {}", error)),
    };

    let new_amount = if increment {
        current.saturating_add(amount)
    } else {
        if amount > current {
            tracing::warn!(
                "reserve underflow: {}:{} has {} but trying to deduct {}",
                chain,
                asset,
                current,
                amount
            );
        }
        current.saturating_sub(amount)
    };

    let entry = ReserveLedgerEntry {
        chain: chain.to_string(),
        asset: asset.to_string(),
        amount: new_amount,
        last_updated: chrono::Utc::now().timestamp(),
    };
    let bytes = serde_json::to_vec(&entry).map_err(|error| format!("encode: {}", error))?;
    db.put_cf(cf, key.as_bytes(), bytes)
        .map_err(|error| format!("db put: {}", error))?;

    info!(
        "reserve ledger: {}:{} {} {} → {}",
        chain,
        asset,
        if increment { "+" } else { "-" },
        amount,
        new_amount
    );
    Ok(())
}
