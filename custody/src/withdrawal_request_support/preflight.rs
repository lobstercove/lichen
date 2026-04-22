use super::*;

pub(crate) struct CreateWithdrawalPreflight {
    pub(crate) req: WithdrawalRequest,
    pub(crate) now_secs: u64,
    pub(crate) replay_digest: String,
    pub(crate) withdrawal_auth_expires_at: u64,
}

fn normalize_withdrawal_request(
    mut req: WithdrawalRequest,
) -> Result<WithdrawalRequest, Json<ErrorResponse>> {
    req.user_id = req.user_id.trim().to_string();
    req.asset = req.asset.trim().to_lowercase();
    req.dest_chain = req.dest_chain.trim().to_lowercase();
    req.dest_address = req.dest_address.trim().to_string();
    req.preferred_stablecoin = req.preferred_stablecoin.trim().to_lowercase();
    if req.preferred_stablecoin.is_empty() || req.asset != "musd" {
        req.preferred_stablecoin = default_preferred_stablecoin();
    }

    if req.user_id.is_empty()
        || req.asset.is_empty()
        || req.dest_chain.is_empty()
        || req.dest_address.is_empty()
    {
        return Err(Json(ErrorResponse::invalid(
            "Missing user_id/asset/amount/dest_chain/dest_address",
        )));
    }

    Ok(req)
}

pub(crate) fn withdrawal_access_message(
    req: &WithdrawalRequest,
    issued_at: u64,
    expires_at: u64,
    nonce: &str,
) -> Vec<u8> {
    format!(
        "{}\nuser_id={}\nasset={}\namount={}\ndest_chain={}\ndest_address={}\npreferred_stablecoin={}\nissued_at={}\nexpires_at={}\nnonce={}\n",
        WITHDRAWAL_ACCESS_DOMAIN,
        req.user_id,
        req.asset,
        req.amount,
        req.dest_chain,
        req.dest_address,
        req.preferred_stablecoin,
        issued_at,
        expires_at,
        nonce,
    )
    .into_bytes()
}

fn parse_withdrawal_access_auth_value(
    value: &Value,
) -> Result<WithdrawalAccessAuth, Json<ErrorResponse>> {
    serde_json::from_value(value.clone()).map_err(|error| {
        Json(ErrorResponse::invalid(&format!(
            "Invalid withdrawal auth object: {}",
            error
        )))
    })
}

fn verify_withdrawal_access_auth_at(
    req: &WithdrawalRequest,
    auth: &WithdrawalAccessAuth,
    now: u64,
) -> Result<(), Json<ErrorResponse>> {
    if auth.expires_at <= auth.issued_at {
        return Err(Json(ErrorResponse::invalid(
            "withdrawal auth expires_at must be greater than issued_at",
        )));
    }

    if auth.expires_at - auth.issued_at > WITHDRAWAL_ACCESS_MAX_TTL_SECS {
        return Err(Json(ErrorResponse::invalid(&format!(
            "withdrawal auth exceeds max ttl of {} seconds",
            WITHDRAWAL_ACCESS_MAX_TTL_SECS
        ))));
    }

    if auth.issued_at > now.saturating_add(WITHDRAWAL_ACCESS_CLOCK_SKEW_SECS) {
        return Err(Json(ErrorResponse::invalid(
            "withdrawal auth issued_at is too far in the future",
        )));
    }

    if auth.expires_at < now {
        return Err(Json(ErrorResponse::invalid("withdrawal auth has expired")));
    }

    let nonce = auth.nonce.trim();
    if nonce.is_empty() {
        return Err(Json(ErrorResponse::invalid(
            "withdrawal auth nonce is required",
        )));
    }
    if nonce.len() > 128 || nonce.contains('\n') {
        return Err(Json(ErrorResponse::invalid(
            "withdrawal auth nonce must be <= 128 chars and contain no newlines",
        )));
    }

    let user_pubkey = Pubkey::from_base58(&req.user_id).map_err(|_| {
        Json(ErrorResponse::invalid(
            "user_id must be a valid Lichen base58 public key (32 bytes)",
        ))
    })?;
    let signature = parse_bridge_access_signature(&auth.signature)?;
    let message = withdrawal_access_message(req, auth.issued_at, auth.expires_at, nonce);
    if !Keypair::verify(&user_pubkey, &message, &signature) {
        return Err(Json(ErrorResponse::invalid(
            "Invalid withdrawal auth signature",
        )));
    }

    Ok(())
}

fn withdrawal_access_replay_digest(
    action: &str,
    req: &WithdrawalRequest,
    auth: &WithdrawalAccessAuth,
) -> Result<String, Json<ErrorResponse>> {
    use sha2::Digest;

    let signature = parse_bridge_access_signature(&auth.signature)?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(action.as_bytes());
    hasher.update([0]);
    hasher.update(req.user_id.as_bytes());
    hasher.update([0]);
    hasher.update(req.asset.as_bytes());
    hasher.update([0]);
    hasher.update(req.amount.to_be_bytes());
    hasher.update(req.dest_chain.as_bytes());
    hasher.update([0]);
    hasher.update(req.dest_address.as_bytes());
    hasher.update([0]);
    hasher.update(req.preferred_stablecoin.as_bytes());
    hasher.update([0]);
    hasher.update(auth.issued_at.to_be_bytes());
    hasher.update(auth.expires_at.to_be_bytes());
    hasher.update(auth.nonce.as_bytes());
    hasher.update([signature.scheme_version]);
    hasher.update([signature.public_key.scheme_version]);
    hasher.update(&signature.public_key.bytes);
    hasher.update(&signature.sig);
    Ok(hex::encode(hasher.finalize()))
}

pub(crate) fn build_create_withdrawal_response(
    job: &WithdrawalJob,
    velocity_snapshot: &WithdrawalVelocitySnapshot,
) -> Value {
    let stablecoin_info = if job.asset.eq_ignore_ascii_case("musd") {
        Some(job.preferred_stablecoin.clone())
    } else {
        None
    };
    let velocity_message = if velocity_snapshot.tier == WithdrawalVelocityTier::Standard {
        String::new()
    } else {
        format!(
            " Velocity tier={} applies after burn confirmation: delay={}s, signer_threshold={}, operator_confirmations={}",
            velocity_snapshot.tier.as_str(),
            velocity_snapshot.delay_secs,
            velocity_snapshot.required_signer_threshold,
            velocity_snapshot.required_operator_confirmations,
        )
    };
    let message = match job.status.as_str() {
        "pending_burn" => format!(
            "Burn {} {} on Lichen, then the outbound transfer to {} will be processed automatically.{}",
            job.amount, job.asset, job.dest_chain, velocity_message
        ),
        "burned" | "signing" | "broadcasting" => format!(
            "Withdrawal {} already exists and is currently {}. Custody will continue processing it automatically.",
            job.job_id, job.status
        ),
        "confirmed" => format!(
            "Withdrawal {} has already completed successfully.",
            job.job_id
        ),
        "expired" => format!(
            "Withdrawal {} expired before a confirmed burn was observed. Submit a new withdrawal request if you still want to continue.",
            job.job_id
        ),
        "failed" | "permanently_failed" => format!(
            "Withdrawal {} is currently {} and will not progress automatically.",
            job.job_id, job.status
        ),
        _ => format!(
            "Withdrawal {} already exists and is currently {}.",
            job.job_id, job.status
        ),
    };

    json!({
        "job_id": job.job_id.clone(),
        "status": job.status.clone(),
        "preferred_stablecoin": stablecoin_info,
        "velocity_tier": velocity_snapshot.tier.as_str(),
        "daily_cap": velocity_snapshot.daily_cap,
        "required_signer_threshold": job.required_signer_threshold,
        "required_operator_confirmations": job.required_operator_confirmations,
        "delay_seconds_after_burn": velocity_snapshot.delay_secs,
        "message": message,
    })
}

pub(crate) fn prepare_create_withdrawal_request(
    state: &CustodyState,
    headers: &axum::http::HeaderMap,
    req: WithdrawalRequest,
) -> Result<CreateWithdrawalPreflight, Json<Value>> {
    if let Err(err_resp) = verify_api_auth(&state.config, headers) {
        return Err(Json(json!({ "error": err_resp.0.message })));
    }

    if let Some(reason) = withdrawal_incident_block_reason(&state.config) {
        return Err(Json(json!({ "error": reason })));
    }

    let req = normalize_withdrawal_request(req)
        .map_err(|error| Json(json!({ "error": error.0.message })))?;
    let withdrawal_auth_value = req.auth.as_ref().ok_or_else(|| {
        Json(json!({
            "error": "Missing auth: expected wallet-signed withdrawal authorization"
        }))
    })?;
    let withdrawal_auth = parse_withdrawal_access_auth_value(withdrawal_auth_value)
        .map_err(|error| Json(json!({ "error": error.0.message })))?;
    let now_secs =
        current_unix_secs().map_err(|error| Json(json!({ "error": error.0.message })))?;
    verify_withdrawal_access_auth_at(&req, &withdrawal_auth, now_secs)
        .map_err(|error| Json(json!({ "error": error.0.message })))?;
    let replay_digest = withdrawal_access_replay_digest(
        BRIDGE_AUTH_REPLAY_ACTION_CREATE_WITHDRAWAL,
        &req,
        &withdrawal_auth,
    )
    .map_err(|error| Json(json!({ "error": error.0.message })))?;

    Ok(CreateWithdrawalPreflight {
        req,
        now_secs,
        replay_digest,
        withdrawal_auth_expires_at: withdrawal_auth.expires_at,
    })
}

pub(crate) async fn handle_withdrawal_auth_replay(
    state: &CustodyState,
    now_secs: u64,
    replay_digest: &str,
    req: &WithdrawalRequest,
    velocity_snapshot: &WithdrawalVelocitySnapshot,
) -> Option<Json<Value>> {
    let _replay_guard = state.bridge_auth_replay_lock.lock().await;
    if let Err(error) =
        prune_expired_bridge_auth_replays(&state.db, now_secs, BRIDGE_AUTH_REPLAY_PRUNE_BATCH)
    {
        return Some(Json(json!({ "error": format!("db error: {}", error) })));
    }

    match find_existing_withdrawal_auth_replay(
        &state.db,
        BRIDGE_AUTH_REPLAY_ACTION_CREATE_WITHDRAWAL,
        replay_digest,
        req,
        velocity_snapshot,
    ) {
        Ok(Some(existing)) => Some(Json(existing)),
        Ok(None) => None,
        Err(error) => Some(Json(json!({ "error": error.0.message }))),
    }
}
