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

pub(super) fn verify_bridge_access_auth(
    user_id: &str,
    auth: &BridgeAccessAuth,
) -> Result<(), Json<ErrorResponse>> {
    verify_bridge_access_auth_at(user_id, auth, current_unix_secs()?)
}

pub(super) fn verify_bridge_access_auth_at(
    user_id: &str,
    auth: &BridgeAccessAuth,
    now: u64,
) -> Result<(), Json<ErrorResponse>> {
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

    let user_pubkey = Pubkey::from_base58(user_id).map_err(|_| {
        Json(ErrorResponse::invalid(
            "user_id must be a valid Lichen base58 public key (32 bytes)",
        ))
    })?;
    let signature = parse_bridge_access_signature(&auth.signature)?;
    let message = bridge_access_message(user_id, auth.issued_at, auth.expires_at);
    if !Keypair::verify(&user_pubkey, &message, &signature) {
        return Err(Json(ErrorResponse::invalid(
            "Invalid bridge auth signature",
        )));
    }

    Ok(())
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
    hasher.update(auth.issued_at.to_be_bytes());
    hasher.update(auth.expires_at.to_be_bytes());
    hasher.update([signature.scheme_version]);
    hasher.update([signature.public_key.scheme_version]);
    hasher.update(&signature.public_key.bytes);
    hasher.update(&signature.sig);
    Ok(hex::encode(hasher.finalize()))
}
