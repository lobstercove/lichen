use super::*;

pub(crate) fn find_existing_bridge_auth_replay(
    db: &DB,
    action: &str,
    digest: &str,
    requested_chain: &str,
    requested_asset: &str,
) -> Result<Option<CreateDepositResponse>, Json<ErrorResponse>> {
    let cf = db
        .cf_handle(CF_BRIDGE_AUTH_REPLAY)
        .ok_or_else(|| Json(ErrorResponse::db("missing bridge_auth_replay cf")))?;
    let lookup_key = super::keys::bridge_auth_replay_lookup_key(action, digest);
    let Some(bytes) = db
        .get_cf(cf, lookup_key.as_bytes())
        .map_err(|error| Json(ErrorResponse::db(&format!("db get: {}", error))))?
    else {
        return Ok(None);
    };

    let replay: BridgeAuthReplayRecord = serde_json::from_slice(&bytes).map_err(|error| {
        Json(ErrorResponse::db(&format!(
            "decode bridge auth replay: {}",
            error
        )))
    })?;
    if replay.chain != requested_chain || replay.asset != requested_asset {
        return Err(Json(ErrorResponse::invalid(
            "bridge auth already used for a different deposit request; sign a new bridge authorization",
        )));
    }

    if let Some(record) =
        fetch_deposit(db, &replay.deposit_id).map_err(|error| Json(ErrorResponse::db(&error)))?
    {
        return Ok(Some(CreateDepositResponse {
            deposit_id: record.deposit_id,
            address: record.address,
        }));
    }

    super::keys::delete_bridge_auth_replay_record(db, action, digest, &replay)
        .map_err(|error| Json(ErrorResponse::db(&error)))?;
    Ok(None)
}

pub(crate) fn prune_expired_bridge_auth_replays(
    db: &DB,
    now: u64,
    limit: usize,
) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_BRIDGE_AUTH_REPLAY)
        .ok_or_else(|| "missing bridge_auth_replay cf".to_string())?;
    let mut expired = Vec::new();

    for item in db.iterator_cf(cf, rocksdb::IteratorMode::Start) {
        let (key, _) = item.map_err(|error| format!("db iter: {}", error))?;
        let key_str = std::str::from_utf8(&key).map_err(|error| format!("utf8: {}", error))?;
        if !key_str.starts_with("0:") {
            break;
        }

        let mut parts = key_str[2..].splitn(3, ':');
        let expires_at = parts
            .next()
            .ok_or_else(|| "missing bridge auth replay expiry".to_string())?
            .parse::<u64>()
            .map_err(|error| format!("invalid bridge auth replay expiry: {}", error))?;
        if expires_at > now {
            break;
        }

        let action = parts
            .next()
            .ok_or_else(|| "missing bridge auth replay action".to_string())?
            .to_string();
        let digest = parts
            .next()
            .ok_or_else(|| "missing bridge auth replay digest".to_string())?
            .to_string();
        expired.push((
            key,
            super::keys::bridge_auth_replay_lookup_key(&action, &digest),
        ));
        if expired.len() >= limit {
            break;
        }
    }

    for (expiry_key, lookup_key) in expired {
        db.delete_cf(cf, &expiry_key)
            .map_err(|error| format!("db delete: {}", error))?;
        db.delete_cf(cf, lookup_key.as_bytes())
            .map_err(|error| format!("db delete: {}", error))?;
    }

    Ok(())
}

pub(crate) fn persist_new_deposit_with_bridge_auth_replay(
    db: &DB,
    record: &DepositRequest,
    action: &str,
    digest: &str,
    expires_at: u64,
) -> Result<(), String> {
    let deposits_cf = db
        .cf_handle(CF_DEPOSITS)
        .ok_or_else(|| "missing deposits cf".to_string())?;
    let address_cf = db
        .cf_handle(CF_ADDRESS_INDEX)
        .ok_or_else(|| "missing address_index cf".to_string())?;
    let status_cf = db
        .cf_handle(CF_STATUS_INDEX)
        .ok_or_else(|| "missing status_index cf".to_string())?;
    let replay_cf = db
        .cf_handle(CF_BRIDGE_AUTH_REPLAY)
        .ok_or_else(|| "missing bridge_auth_replay cf".to_string())?;

    let deposit_bytes =
        serde_json::to_vec(record).map_err(|error| format!("encode deposit: {}", error))?;
    let replay_bytes = serde_json::to_vec(&BridgeAuthReplayRecord {
        deposit_id: record.deposit_id.clone(),
        expires_at,
        chain: record.chain.clone(),
        asset: record.asset.clone(),
    })
    .map_err(|error| format!("encode bridge auth replay: {}", error))?;

    let mut batch = WriteBatch::default();
    batch.put_cf(deposits_cf, record.deposit_id.as_bytes(), deposit_bytes);
    batch.put_cf(
        address_cf,
        record.address.as_bytes(),
        record.deposit_id.as_bytes(),
    );
    batch.put_cf(
        status_cf,
        format!("status:deposits:issued:{}", record.deposit_id).as_bytes(),
        b"",
    );
    batch.put_cf(
        replay_cf,
        super::keys::bridge_auth_replay_lookup_key(action, digest).as_bytes(),
        replay_bytes,
    );
    batch.put_cf(
        replay_cf,
        super::keys::bridge_auth_replay_expiry_key(expires_at, action, digest).as_bytes(),
        b"",
    );
    db.write(batch)
        .map_err(|error| format!("db write: {}", error))
}
