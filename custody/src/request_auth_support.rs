use super::*;

pub(super) fn verify_api_auth(
    config: &CustodyConfig,
    headers: &axum::http::HeaderMap,
) -> Result<(), Json<ErrorResponse>> {
    let expected = config.api_auth_token.as_deref().unwrap_or("");
    let provided = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");

    if expected.is_empty() {
        return Err(Json(ErrorResponse {
            code: "unauthorized",
            message: "Invalid or missing Bearer token".to_string(),
        }));
    }

    use subtle::ConstantTimeEq;
    let matches: bool = provided.as_bytes().ct_eq(expected.as_bytes()).into();
    if !matches {
        return Err(Json(ErrorResponse {
            code: "unauthorized",
            message: "Invalid or missing Bearer token".to_string(),
        }));
    }
    Ok(())
}

pub(super) fn bridge_access_message(user_id: &str, issued_at: u64, expires_at: u64) -> Vec<u8> {
    format!(
        "{}\nuser_id={}\nissued_at={}\nexpires_at={}\n",
        BRIDGE_ACCESS_DOMAIN, user_id, issued_at, expires_at
    )
    .into_bytes()
}

pub(super) fn bridge_access_message_v2_create(
    user_id: &str,
    chain: &str,
    asset: &str,
    issued_at: u64,
    expires_at: u64,
    nonce: &str,
) -> Vec<u8> {
    let chain = chain.trim().to_lowercase();
    let asset = asset.trim().to_lowercase();
    format!(
        "{}\naction={}\nuser_id={}\nchain={}\nasset={}\nroute={}:{}\nissued_at={}\nexpires_at={}\nnonce={}\n",
        BRIDGE_ACCESS_DOMAIN_V2,
        BRIDGE_AUTH_REPLAY_ACTION_CREATE_DEPOSIT,
        user_id,
        chain,
        asset,
        chain,
        asset,
        issued_at,
        expires_at,
        nonce
    )
    .into_bytes()
}

pub(super) fn bridge_access_message_v2_lookup(
    user_id: &str,
    deposit_id: &str,
    issued_at: u64,
    expires_at: u64,
    nonce: &str,
) -> Vec<u8> {
    format!(
        "{}\naction={}\nuser_id={}\ndeposit_id={}\nissued_at={}\nexpires_at={}\nnonce={}\n",
        BRIDGE_ACCESS_DOMAIN_V2,
        BRIDGE_AUTH_ACTION_GET_DEPOSIT,
        user_id,
        deposit_id,
        issued_at,
        expires_at,
        nonce
    )
    .into_bytes()
}

pub(super) fn current_unix_secs() -> Result<u64, Json<ErrorResponse>> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| {
            Json(ErrorResponse::invalid(&format!(
                "System clock error: {}",
                error
            )))
        })
}

pub(super) fn parse_bridge_access_signature(
    value: &Value,
) -> Result<PqSignature, Json<ErrorResponse>> {
    if value.is_object() {
        return serde_json::from_value(value.clone()).map_err(|error| {
            Json(ErrorResponse::invalid(&format!(
                "Invalid PQ signature object: {}",
                error
            )))
        });
    }

    if let Some(encoded) = value.as_str() {
        return serde_json::from_str(encoded).map_err(|error| {
            Json(ErrorResponse::invalid(&format!(
                "Invalid PQ signature JSON string: {}",
                error
            )))
        });
    }

    Err(Json(ErrorResponse::invalid(
        "Signature must be a PQ signature object or JSON string",
    )))
}

pub(super) fn parse_bridge_access_auth_value(
    value: &Value,
) -> Result<BridgeAccessAuth, Json<ErrorResponse>> {
    serde_json::from_value(value.clone()).map_err(|error| {
        Json(ErrorResponse::invalid(&format!(
            "Invalid bridge auth object: {}",
            error
        )))
    })
}

pub(super) fn parse_bridge_access_auth_json(
    value: &str,
) -> Result<BridgeAccessAuth, Json<ErrorResponse>> {
    serde_json::from_str(value).map_err(|error| {
        Json(ErrorResponse::invalid(&format!(
            "Invalid bridge auth object: {}",
            error
        )))
    })
}

pub(super) fn verify_bridge_access_auth_for_create_at(
    user_id: &str,
    chain: &str,
    asset: &str,
    auth: &BridgeAccessAuth,
    now: u64,
) -> Result<(), Json<ErrorResponse>> {
    if !bridge_auth_is_v2(auth) {
        return verify_bridge_access_auth_v1_at(user_id, auth, now);
    }

    let chain = chain.trim().to_lowercase();
    let asset = asset.trim().to_lowercase();
    let action = bridge_auth_v2_action(auth)?;
    if action != BRIDGE_AUTH_REPLAY_ACTION_CREATE_DEPOSIT {
        return Err(Json(ErrorResponse::invalid(
            "bridge auth action does not match createBridgeDeposit",
        )));
    }
    let auth_user_id = bridge_auth_v2_field(auth.user_id.as_deref(), "user_id")?;
    if auth_user_id != user_id {
        return Err(Json(ErrorResponse::invalid(
            "bridge auth user_id does not match request",
        )));
    }
    let auth_chain = bridge_auth_v2_field(auth.chain.as_deref(), "chain")?.to_lowercase();
    let auth_asset = bridge_auth_v2_field(auth.asset.as_deref(), "asset")?.to_lowercase();
    let expected_route = format!("{}:{}", chain, asset);
    let auth_route = bridge_auth_v2_field(auth.route.as_deref(), "route")?.to_lowercase();
    if auth_chain != chain || auth_asset != asset || auth_route != expected_route {
        return Err(Json(ErrorResponse::invalid(
            "bridge auth route does not match request",
        )));
    }

    verify_bridge_access_auth_v2_create_at(user_id, &chain, &asset, auth, now)
}

pub(super) fn verify_bridge_access_auth_for_lookup_at(
    user_id: &str,
    deposit_id: &str,
    auth: &BridgeAccessAuth,
    now: u64,
) -> Result<(), Json<ErrorResponse>> {
    if !bridge_auth_is_v2(auth) {
        return verify_bridge_access_auth_v1_at(user_id, auth, now);
    }

    match bridge_auth_v2_action(auth)? {
        BRIDGE_AUTH_REPLAY_ACTION_CREATE_DEPOSIT => {
            verify_bridge_access_auth_v2_self_contained_at(user_id, auth, now)
        }
        BRIDGE_AUTH_ACTION_GET_DEPOSIT => {
            let auth_user_id = bridge_auth_v2_field(auth.user_id.as_deref(), "user_id")?;
            if auth_user_id != user_id {
                return Err(Json(ErrorResponse::invalid(
                    "bridge auth user_id does not match request",
                )));
            }
            let auth_deposit_id = bridge_auth_v2_field(auth.deposit_id.as_deref(), "deposit_id")?;
            if auth_deposit_id != deposit_id {
                return Err(Json(ErrorResponse::invalid(
                    "bridge auth deposit_id does not match request",
                )));
            }
            verify_bridge_access_auth_v2_lookup_at(user_id, deposit_id, auth, now)
        }
        _ => Err(Json(ErrorResponse::invalid(
            "unsupported bridge auth action",
        ))),
    }
}

fn verify_bridge_access_auth_v1_at(
    user_id: &str,
    auth: &BridgeAccessAuth,
    now: u64,
) -> Result<(), Json<ErrorResponse>> {
    validate_bridge_auth_time(auth, now)?;

    let message = bridge_access_message(user_id, auth.issued_at, auth.expires_at);
    verify_bridge_access_signature(user_id, auth, &message)
}

fn validate_bridge_auth_time(auth: &BridgeAccessAuth, now: u64) -> Result<(), Json<ErrorResponse>> {
    if auth.expires_at <= auth.issued_at {
        return Err(Json(ErrorResponse::invalid(
            "bridge auth expires_at must be greater than issued_at",
        )));
    }

    if auth.expires_at - auth.issued_at > BRIDGE_ACCESS_MAX_TTL_SECS {
        return Err(Json(ErrorResponse::invalid(&format!(
            "bridge auth exceeds max ttl of {} seconds",
            BRIDGE_ACCESS_MAX_TTL_SECS
        ))));
    }

    if auth.issued_at > now.saturating_add(BRIDGE_ACCESS_CLOCK_SKEW_SECS) {
        return Err(Json(ErrorResponse::invalid(
            "bridge auth issued_at is too far in the future",
        )));
    }

    if auth.expires_at < now {
        return Err(Json(ErrorResponse::invalid("bridge auth has expired")));
    }

    Ok(())
}

fn verify_bridge_access_signature(
    user_id: &str,
    auth: &BridgeAccessAuth,
    message: &[u8],
) -> Result<(), Json<ErrorResponse>> {
    let user_pubkey = Pubkey::from_base58(user_id).map_err(|_| {
        Json(ErrorResponse::invalid(
            "user_id must be a valid Lichen base58 public key (32 bytes)",
        ))
    })?;
    let signature = parse_bridge_access_signature(&auth.signature)?;
    if !Keypair::verify(&user_pubkey, message, &signature) {
        return Err(Json(ErrorResponse::invalid(
            "Invalid bridge auth signature",
        )));
    }

    Ok(())
}

fn bridge_auth_is_v2(auth: &BridgeAccessAuth) -> bool {
    auth.version == Some(2) || auth.domain.as_deref() == Some(BRIDGE_ACCESS_DOMAIN_V2)
}

fn bridge_auth_v2_action(auth: &BridgeAccessAuth) -> Result<&str, Json<ErrorResponse>> {
    validate_bridge_auth_v2_header(auth)?;
    bridge_auth_v2_field(auth.action.as_deref(), "action")
}

fn validate_bridge_auth_v2_header(auth: &BridgeAccessAuth) -> Result<(), Json<ErrorResponse>> {
    if auth.version != Some(2) {
        return Err(Json(ErrorResponse::invalid(
            "bridge auth version must be 2",
        )));
    }
    if auth.domain.as_deref() != Some(BRIDGE_ACCESS_DOMAIN_V2) {
        return Err(Json(ErrorResponse::invalid(
            "bridge auth domain must be LICHEN_BRIDGE_ACCESS_V2",
        )));
    }
    Ok(())
}

fn bridge_auth_v2_field<'a>(
    value: Option<&'a str>,
    field: &str,
) -> Result<&'a str, Json<ErrorResponse>> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            Json(ErrorResponse::invalid(&format!(
                "bridge auth {} is required",
                field
            )))
        })?;
    if value.contains('\n') || value.len() > 160 {
        return Err(Json(ErrorResponse::invalid(&format!(
            "bridge auth {} must be <= 160 chars and contain no newlines",
            field
        ))));
    }
    Ok(value)
}

fn bridge_auth_v2_nonce(auth: &BridgeAccessAuth) -> Result<&str, Json<ErrorResponse>> {
    let nonce = bridge_auth_v2_field(auth.nonce.as_deref(), "nonce")?;
    if nonce.len() > 128 {
        return Err(Json(ErrorResponse::invalid(
            "bridge auth nonce must be <= 128 chars and contain no newlines",
        )));
    }
    Ok(nonce)
}

fn verify_bridge_access_auth_v2_create_at(
    user_id: &str,
    chain: &str,
    asset: &str,
    auth: &BridgeAccessAuth,
    now: u64,
) -> Result<(), Json<ErrorResponse>> {
    validate_bridge_auth_time(auth, now)?;
    validate_bridge_auth_v2_header(auth)?;
    let nonce = bridge_auth_v2_nonce(auth)?;
    let message = bridge_access_message_v2_create(
        user_id,
        chain,
        asset,
        auth.issued_at,
        auth.expires_at,
        nonce,
    );
    verify_bridge_access_signature(user_id, auth, &message)
}

fn verify_bridge_access_auth_v2_lookup_at(
    user_id: &str,
    deposit_id: &str,
    auth: &BridgeAccessAuth,
    now: u64,
) -> Result<(), Json<ErrorResponse>> {
    validate_bridge_auth_time(auth, now)?;
    validate_bridge_auth_v2_header(auth)?;
    let nonce = bridge_auth_v2_nonce(auth)?;
    let message = bridge_access_message_v2_lookup(
        user_id,
        deposit_id,
        auth.issued_at,
        auth.expires_at,
        nonce,
    );
    verify_bridge_access_signature(user_id, auth, &message)
}

fn verify_bridge_access_auth_v2_self_contained_at(
    user_id: &str,
    auth: &BridgeAccessAuth,
    now: u64,
) -> Result<(), Json<ErrorResponse>> {
    match bridge_auth_v2_action(auth)? {
        BRIDGE_AUTH_REPLAY_ACTION_CREATE_DEPOSIT => {
            let auth_user_id = bridge_auth_v2_field(auth.user_id.as_deref(), "user_id")?;
            if auth_user_id != user_id {
                return Err(Json(ErrorResponse::invalid(
                    "bridge auth user_id does not match request",
                )));
            }
            let chain = bridge_auth_v2_field(auth.chain.as_deref(), "chain")?.to_lowercase();
            let asset = bridge_auth_v2_field(auth.asset.as_deref(), "asset")?.to_lowercase();
            let route = bridge_auth_v2_field(auth.route.as_deref(), "route")?.to_lowercase();
            if route != format!("{}:{}", chain, asset) {
                return Err(Json(ErrorResponse::invalid(
                    "bridge auth route is inconsistent",
                )));
            }
            verify_bridge_access_auth_v2_create_at(user_id, &chain, &asset, auth, now)
        }
        BRIDGE_AUTH_ACTION_GET_DEPOSIT => {
            let auth_user_id = bridge_auth_v2_field(auth.user_id.as_deref(), "user_id")?;
            if auth_user_id != user_id {
                return Err(Json(ErrorResponse::invalid(
                    "bridge auth user_id does not match request",
                )));
            }
            let deposit_id = bridge_auth_v2_field(auth.deposit_id.as_deref(), "deposit_id")?;
            verify_bridge_access_auth_v2_lookup_at(user_id, deposit_id, auth, now)
        }
        _ => Err(Json(ErrorResponse::invalid(
            "unsupported bridge auth action",
        ))),
    }
}

pub(super) fn bridge_access_replay_digest(
    action: &str,
    user_id: &str,
    auth: &BridgeAccessAuth,
) -> Result<String, Json<ErrorResponse>> {
    use sha2::Digest;

    let signature = parse_bridge_access_signature(&auth.signature)?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(action.as_bytes());
    hasher.update([0]);
    hasher.update(user_id.as_bytes());
    hasher.update([0]);
    if bridge_auth_is_v2(auth) {
        let signed_message = bridge_auth_v2_signed_message(user_id, auth)?;
        hasher.update(BRIDGE_ACCESS_DOMAIN_V2.as_bytes());
        hasher.update([0]);
        hasher.update(signed_message);
        hasher.update([0]);
    }
    hasher.update(auth.issued_at.to_be_bytes());
    hasher.update(auth.expires_at.to_be_bytes());
    hasher.update([signature.scheme_version]);
    hasher.update([signature.public_key.scheme_version]);
    hasher.update(&signature.public_key.bytes);
    hasher.update(&signature.sig);
    Ok(hex::encode(hasher.finalize()))
}

fn bridge_auth_v2_signed_message(
    user_id: &str,
    auth: &BridgeAccessAuth,
) -> Result<Vec<u8>, Json<ErrorResponse>> {
    match bridge_auth_v2_action(auth)? {
        BRIDGE_AUTH_REPLAY_ACTION_CREATE_DEPOSIT => {
            let auth_user_id = bridge_auth_v2_field(auth.user_id.as_deref(), "user_id")?;
            if auth_user_id != user_id {
                return Err(Json(ErrorResponse::invalid(
                    "bridge auth user_id does not match request",
                )));
            }
            let chain = bridge_auth_v2_field(auth.chain.as_deref(), "chain")?.to_lowercase();
            let asset = bridge_auth_v2_field(auth.asset.as_deref(), "asset")?.to_lowercase();
            let route = bridge_auth_v2_field(auth.route.as_deref(), "route")?.to_lowercase();
            if route != format!("{}:{}", chain, asset) {
                return Err(Json(ErrorResponse::invalid(
                    "bridge auth route is inconsistent",
                )));
            }
            Ok(bridge_access_message_v2_create(
                user_id,
                &chain,
                &asset,
                auth.issued_at,
                auth.expires_at,
                bridge_auth_v2_nonce(auth)?,
            ))
        }
        BRIDGE_AUTH_ACTION_GET_DEPOSIT => {
            let auth_user_id = bridge_auth_v2_field(auth.user_id.as_deref(), "user_id")?;
            if auth_user_id != user_id {
                return Err(Json(ErrorResponse::invalid(
                    "bridge auth user_id does not match request",
                )));
            }
            let deposit_id = bridge_auth_v2_field(auth.deposit_id.as_deref(), "deposit_id")?;
            Ok(bridge_access_message_v2_lookup(
                user_id,
                deposit_id,
                auth.issued_at,
                auth.expires_at,
                bridge_auth_v2_nonce(auth)?,
            ))
        }
        _ => Err(Json(ErrorResponse::invalid(
            "unsupported bridge auth action",
        ))),
    }
}
