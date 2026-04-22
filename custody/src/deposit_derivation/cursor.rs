use super::super::*;

pub(super) const CURSOR_NEXT_DERIVATION_ACCOUNT: &str = "next_derivation_account";

pub(super) fn next_deposit_index(
    db: &DB,
    user_id: &str,
    chain: &str,
    asset: &str,
) -> Result<u64, String> {
    let cf = db
        .cf_handle(CF_INDEXES)
        .ok_or_else(|| "missing indexes cf".to_string())?;
    let key = format!("{}/{}/{}", user_id, chain, asset);
    let current = match db.get_cf(cf, key.as_bytes()) {
        Ok(Some(bytes)) => {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes);
            u64::from_le_bytes(buf)
        }
        Ok(None) => 0,
        Err(e) => return Err(format!("db get: {}", e)),
    };

    let next = current + 1;
    db.put_cf(cf, key.as_bytes(), next.to_le_bytes())
        .map_err(|e| format!("db put: {}", e))?;
    Ok(next)
}

pub(super) fn get_last_u64_index(db: &DB, key: &str) -> Result<Option<u64>, String> {
    let cf = db
        .cf_handle(CF_CURSORS)
        .ok_or_else(|| "missing cursors cf".to_string())?;
    match db.get_cf(cf, key.as_bytes()) {
        Ok(Some(bytes)) => {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes);
            Ok(Some(u64::from_le_bytes(buf)))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(format!("db get: {}", e)),
    }
}

pub(super) fn set_last_u64_index(db: &DB, key: &str, value: u64) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_CURSORS)
        .ok_or_else(|| "missing cursors cf".to_string())?;
    db.put_cf(cf, key.as_bytes(), value.to_le_bytes())
        .map_err(|e| format!("db put: {}", e))
}
