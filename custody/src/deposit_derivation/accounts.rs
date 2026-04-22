use super::super::*;
use super::cursor::{get_last_u64_index, set_last_u64_index, CURSOR_NEXT_DERIVATION_ACCOUNT};
use super::path::MAX_BIP44_ACCOUNT_INDEX;

const INDEX_KEY_DERIVATION_ACCOUNT_PREFIX: &str = "derivation_account:";

fn derivation_account_index_key(user_id: &str) -> String {
    format!("{}{}", INDEX_KEY_DERIVATION_ACCOUNT_PREFIX, user_id)
}

fn load_derivation_account(db: &DB, user_id: &str) -> Result<Option<u32>, String> {
    let cf = db
        .cf_handle(CF_INDEXES)
        .ok_or_else(|| "missing indexes cf".to_string())?;
    let key = derivation_account_index_key(user_id);
    match db.get_cf(cf, key.as_bytes()) {
        Ok(Some(bytes)) => {
            if bytes.len() != 4 {
                return Err(format!(
                    "invalid derivation account entry for user {}",
                    user_id
                ));
            }
            let mut buf = [0u8; 4];
            buf.copy_from_slice(&bytes);
            Ok(Some(u32::from_le_bytes(buf)))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(format!("db get: {}", e)),
    }
}

fn store_derivation_account(db: &DB, user_id: &str, account: u32) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_INDEXES)
        .ok_or_else(|| "missing indexes cf".to_string())?;
    let key = derivation_account_index_key(user_id);
    db.put_cf(cf, key.as_bytes(), account.to_le_bytes())
        .map_err(|e| format!("db put: {}", e))
}

fn parse_bip44_account_index(path: &str) -> Result<u32, String> {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 6 || parts[0] != "m" {
        return Err(format!("invalid BIP-44 derivation path: {}", path));
    }
    let account = parts[3]
        .strip_suffix('\'')
        .ok_or_else(|| format!("invalid BIP-44 account segment: {}", path))?
        .parse::<u32>()
        .map_err(|e| format!("parse derivation account: {}", e))?;
    if account > MAX_BIP44_ACCOUNT_INDEX {
        return Err(format!(
            "BIP-44 account {} exceeds the 31-bit hardened range",
            account
        ));
    }
    Ok(account)
}

fn max_legacy_derivation_account(db: &DB) -> Result<Option<u32>, String> {
    let cf = db
        .cf_handle(CF_DEPOSITS)
        .ok_or_else(|| "missing deposits cf".to_string())?;
    let mut max_account: Option<u32> = None;
    for item in db.iterator_cf(cf, rocksdb::IteratorMode::Start) {
        let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
        let record: DepositRequest =
            serde_json::from_slice(&value).map_err(|e| format!("decode deposit: {}", e))?;
        let account = parse_bip44_account_index(&record.derivation_path)?;
        max_account = Some(max_account.map_or(account, |current| current.max(account)));
    }
    Ok(max_account)
}

fn find_legacy_user_derivation_account(db: &DB, user_id: &str) -> Result<Option<u32>, String> {
    let cf = db
        .cf_handle(CF_DEPOSITS)
        .ok_or_else(|| "missing deposits cf".to_string())?;
    for item in db.iterator_cf(cf, rocksdb::IteratorMode::Start) {
        let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
        let record: DepositRequest =
            serde_json::from_slice(&value).map_err(|e| format!("decode deposit: {}", e))?;
        if record.user_id == user_id {
            return parse_bip44_account_index(&record.derivation_path).map(Some);
        }
    }
    Ok(None)
}

fn initialize_next_derivation_account_cursor(db: &DB) -> Result<u64, String> {
    if let Some(next_account) = get_last_u64_index(db, CURSOR_NEXT_DERIVATION_ACCOUNT)? {
        return Ok(next_account);
    }

    let next_account = max_legacy_derivation_account(db)?
        .map(|account| u64::from(account).saturating_add(1))
        .unwrap_or(0);
    set_last_u64_index(db, CURSOR_NEXT_DERIVATION_ACCOUNT, next_account)?;
    Ok(next_account)
}

pub(super) fn get_or_allocate_derivation_account(db: &DB, user_id: &str) -> Result<u32, String> {
    if let Some(account) = load_derivation_account(db, user_id)? {
        return Ok(account);
    }

    let mut next_account = initialize_next_derivation_account_cursor(db)?;
    if let Some(account) = find_legacy_user_derivation_account(db, user_id)? {
        next_account = next_account.max(u64::from(account).saturating_add(1));
        store_derivation_account(db, user_id, account)?;
        set_last_u64_index(db, CURSOR_NEXT_DERIVATION_ACCOUNT, next_account)?;
        return Ok(account);
    }

    if next_account > u64::from(MAX_BIP44_ACCOUNT_INDEX) {
        return Err("custody derivation account space exhausted".to_string());
    }

    let account = u32::try_from(next_account)
        .map_err(|_| "custody derivation account space exhausted".to_string())?;
    store_derivation_account(db, user_id, account)?;
    set_last_u64_index(
        db,
        CURSOR_NEXT_DERIVATION_ACCOUNT,
        next_account.saturating_add(1),
    )?;
    Ok(account)
}
