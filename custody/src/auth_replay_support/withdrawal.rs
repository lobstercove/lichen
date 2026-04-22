use super::*;

pub(crate) fn find_existing_withdrawal_auth_replay(
    db: &DB,
    action: &str,
    digest: &str,
    req: &WithdrawalRequest,
    velocity_snapshot: &WithdrawalVelocitySnapshot,
) -> Result<Option<Value>, Json<ErrorResponse>> {
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

    let replay: WithdrawalAuthReplayRecord = serde_json::from_slice(&bytes).map_err(|error| {
        Json(ErrorResponse::db(&format!(
            "decode withdrawal auth replay: {}",
            error
        )))
    })?;
    if replay.user_id != req.user_id
        || replay.asset != req.asset
        || replay.amount != req.amount
        || replay.dest_chain != req.dest_chain
        || replay.dest_address != req.dest_address
        || replay.preferred_stablecoin != req.preferred_stablecoin
    {
        return Err(Json(ErrorResponse::invalid(
            "withdrawal auth already used for a different withdrawal request; sign a new withdrawal authorization",
        )));
    }

    if let Some(job) =
        fetch_withdrawal_job(db, &replay.job_id).map_err(|error| Json(ErrorResponse::db(&error)))?
    {
        if job.status == "expired" {
            super::keys::delete_auth_replay_record_by_expiry(db, action, digest, replay.expires_at)
                .map_err(|error| Json(ErrorResponse::db(&error)))?;
            return Ok(None);
        }
        return Ok(Some(build_create_withdrawal_response(
            &job,
            velocity_snapshot,
        )));
    }

    super::keys::delete_auth_replay_record_by_expiry(db, action, digest, replay.expires_at)
        .map_err(|error| Json(ErrorResponse::db(&error)))?;
    Ok(None)
}

pub(crate) fn persist_new_withdrawal_with_auth_replay(
    db: &DB,
    job: &WithdrawalJob,
    action: &str,
    digest: &str,
    expires_at: u64,
) -> Result<(), String> {
    let withdrawals_cf = db
        .cf_handle(CF_WITHDRAWAL_JOBS)
        .ok_or_else(|| "missing withdrawal_jobs cf".to_string())?;
    let status_cf = db
        .cf_handle(CF_STATUS_INDEX)
        .ok_or_else(|| "missing status_index cf".to_string())?;
    let replay_cf = db
        .cf_handle(CF_BRIDGE_AUTH_REPLAY)
        .ok_or_else(|| "missing bridge_auth_replay cf".to_string())?;

    let withdrawal_bytes =
        serde_json::to_vec(job).map_err(|error| format!("encode withdrawal: {}", error))?;
    let replay_bytes = serde_json::to_vec(&WithdrawalAuthReplayRecord {
        job_id: job.job_id.clone(),
        expires_at,
        user_id: job.user_id.clone(),
        asset: job.asset.clone(),
        amount: job.amount,
        dest_chain: job.dest_chain.clone(),
        dest_address: job.dest_address.clone(),
        preferred_stablecoin: job.preferred_stablecoin.clone(),
    })
    .map_err(|error| format!("encode withdrawal auth replay: {}", error))?;

    let mut batch = WriteBatch::default();
    batch.put_cf(withdrawals_cf, job.job_id.as_bytes(), withdrawal_bytes);
    batch.put_cf(
        status_cf,
        format!("status:withdrawal:{}:{}", job.status, job.job_id).as_bytes(),
        b"",
    );
    batch.put_cf(
        replay_cf,
        super::keys::bridge_auth_replay_lookup_key(action, digest).as_bytes(),
        replay_bytes.clone(),
    );
    batch.put_cf(
        replay_cf,
        super::keys::bridge_auth_replay_expiry_key(expires_at, action, digest).as_bytes(),
        replay_bytes,
    );

    db.write(batch)
        .map_err(|error| format!("db write: {}", error))
}
