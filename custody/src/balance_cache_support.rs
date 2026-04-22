use super::*;

pub(super) fn get_last_balance(db: &DB, address: &str) -> Result<u128, String> {
    let cf = db
        .cf_handle(CF_ADDRESS_BALANCES)
        .ok_or_else(|| "missing address_balances cf".to_string())?;
    match db.get_cf(cf, address.as_bytes()) {
        Ok(Some(bytes)) => {
            let mut buf = [0u8; 16];
            buf.copy_from_slice(&bytes);
            Ok(u128::from_le_bytes(buf))
        }
        Ok(None) => Ok(0),
        Err(error) => Err(format!("db get: {}", error)),
    }
}

pub(super) fn set_last_balance(db: &DB, address: &str, balance: u128) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_ADDRESS_BALANCES)
        .ok_or_else(|| "missing address_balances cf".to_string())?;
    db.put_cf(cf, address.as_bytes(), balance.to_le_bytes())
        .map_err(|error| format!("db put: {}", error))
}

pub(super) fn get_last_balance_with_key(db: &DB, key: &str) -> Result<u64, String> {
    let cf = db
        .cf_handle(CF_TOKEN_BALANCES)
        .ok_or_else(|| "missing token_balances cf".to_string())?;
    match db.get_cf(cf, key.as_bytes()) {
        Ok(Some(bytes)) => {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes);
            Ok(u64::from_le_bytes(buf))
        }
        Ok(None) => Ok(0),
        Err(error) => Err(format!("db get: {}", error)),
    }
}

pub(super) fn set_last_balance_with_key(db: &DB, key: &str, balance: u64) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_TOKEN_BALANCES)
        .ok_or_else(|| "missing token_balances cf".to_string())?;
    db.put_cf(cf, key.as_bytes(), balance.to_le_bytes())
        .map_err(|error| format!("db put: {}", error))
}
