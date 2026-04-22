use super::*;

pub(super) fn store_deposit(db: &DB, record: &DepositRequest) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_DEPOSITS)
        .ok_or_else(|| "missing deposits cf".to_string())?;
    let bytes = serde_json::to_vec(record).map_err(|error| format!("encode: {}", error))?;
    db.put_cf(cf, record.deposit_id.as_bytes(), bytes)
        .map_err(|error| format!("db put: {}", error))
}

pub(super) fn fetch_deposit(db: &DB, deposit_id: &str) -> Result<Option<DepositRequest>, String> {
    let cf = db
        .cf_handle(CF_DEPOSITS)
        .ok_or_else(|| "missing deposits cf".to_string())?;
    match db.get_cf(cf, deposit_id.as_bytes()) {
        Ok(Some(bytes)) => {
            let record =
                serde_json::from_slice(&bytes).map_err(|error| format!("decode: {}", error))?;
            Ok(Some(record))
        }
        Ok(None) => Ok(None),
        Err(error) => Err(format!("db get: {}", error)),
    }
}
