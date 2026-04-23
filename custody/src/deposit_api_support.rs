use super::*;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct CreateDepositRequest {
    pub(super) user_id: String,
    pub(super) chain: String,
    pub(super) asset: String,
    #[serde(default)]
    pub(super) auth: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct CreateDepositResponse {
    pub(super) deposit_id: String,
    pub(super) address: String,
}

/// AUDIT-FIX F8.6: Deposit creation now requires API auth.
/// Without auth, anyone can create deposit addresses which generates derivation paths
/// and (combined with a compromised master seed) could reconstruct private keys.
pub(super) async fn create_deposit(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<CreateDepositRequest>,
) -> Result<Json<CreateDepositResponse>, ErrorResponse> {
    verify_api_auth(&state.config, &headers)?;

    let chain = payload.chain.to_lowercase();
    let asset = payload.asset.to_lowercase();
    let user_id = payload.user_id.clone();
    if chain.is_empty() || asset.is_empty() || payload.user_id.is_empty() {
        return Err(ErrorResponse::invalid("Missing user_id/chain/asset"));
    }

    // Validate user_id is a valid Lichen base58 pubkey (32 bytes).
    // Reject early so build_credit_job never silently drops a credit.
    if Pubkey::from_base58(&user_id).is_err() {
        return Err(ErrorResponse::invalid(
            "user_id must be a valid Lichen base58 public key (32 bytes)",
        ));
    }

    let bridge_auth_value = payload.auth.as_ref().ok_or_else(|| {
        Json(ErrorResponse::invalid(
            "Missing auth: expected wallet-signed bridge access",
        ))
    })?;
    let bridge_auth = parse_bridge_access_auth_value(bridge_auth_value)?;
    let now = current_unix_secs()?;
    verify_bridge_access_auth_at(&user_id, &bridge_auth, now)?;
    let replay_digest = bridge_access_replay_digest(
        BRIDGE_AUTH_REPLAY_ACTION_CREATE_DEPOSIT,
        &user_id,
        &bridge_auth,
    )?;

    ensure_deposit_creation_allowed(&state.config).map_err(|e| Json(ErrorResponse::invalid(&e)))?;

    // AUDIT-FIX W-H4: Rate limit deposit creation (60/min global, 10s per-user cooldown)
    {
        let mut dr = state.deposit_rate.lock().await;
        let now = std::time::Instant::now();
        if now.duration_since(dr.window_start).as_secs() >= 60 {
            dr.window_start = now;
            dr.count_this_minute = 0;
        }
        dr.count_this_minute += 1;
        if dr.count_this_minute > 60 {
            tracing::warn!(
                "⚠️  Deposit rate limit exceeded: {} this minute",
                dr.count_this_minute
            );
            return Err(ErrorResponse::invalid(
                "rate_limited: too many deposit requests, try again later",
            ));
        }
        if let Some(last) = dr.per_user.get(&user_id) {
            if now.duration_since(*last).as_secs() < 10 {
                return Err(ErrorResponse::invalid(
                    "rate_limited: wait 10s between deposit requests",
                ));
            }
        }
        dr.per_user.insert(user_id.clone(), now);
        persist_deposit_rate_state(&state.db, &dr).map_err(|e| Json(ErrorResponse::db(&e)))?;
    }

    if (chain == "solana" || chain == "sol") && is_solana_stablecoin(&asset) {
        ensure_solana_config(&state.config).map_err(|e| Json(ErrorResponse::invalid(&e)))?;
    }

    let _replay_guard = state.bridge_auth_replay_lock.lock().await;
    prune_expired_bridge_auth_replays(&state.db, now, BRIDGE_AUTH_REPLAY_PRUNE_BATCH)
        .map_err(|e| Json(ErrorResponse::db(&e)))?;
    if let Some(existing) = find_existing_bridge_auth_replay(
        &state.db,
        BRIDGE_AUTH_REPLAY_ACTION_CREATE_DEPOSIT,
        &replay_digest,
        &chain,
        &asset,
    )? {
        return Ok(Json(existing));
    }

    let deposit_id = Uuid::new_v4().to_string();
    let _guard = state.next_index_lock.lock().await;
    let derivation_account = get_or_allocate_derivation_account(&state.db, &user_id)
        .map_err(|e| Json(ErrorResponse::db(&e)))?;
    let index = next_deposit_index(&state.db, &user_id, &chain, &asset)
        .map_err(|e| Json(ErrorResponse::db(&e)))?;

    let derivation_path = bip44_derivation_path(&chain, derivation_account, index)
        .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
    let deposit_seed_source = active_deposit_seed_source(&state.config).to_string();
    let deposit_seed = deposit_seed_for_source(&state.config, &deposit_seed_source);
    let address = if chain == "solana" || chain == "sol" {
        if is_solana_stablecoin(&asset) {
            let mint = solana_mint_for_asset(&state.config, &asset)
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
            let owner = derive_solana_owner_pubkey(&derivation_path, deposit_seed)
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
            let ata = derive_associated_token_address(&owner, &mint)
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
            ensure_associated_token_account(&state, &owner, &mint, &ata)
                .await
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
            ata
        } else {
            derive_deposit_address(&chain, &asset, &derivation_path, deposit_seed)
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?
        }
    } else {
        derive_deposit_address(&chain, &asset, &derivation_path, deposit_seed)
            .map_err(|e| Json(ErrorResponse::invalid(&e)))?
    };

    let record = DepositRequest {
        deposit_id: deposit_id.clone(),
        user_id: user_id.clone(),
        chain,
        asset,
        address: address.clone(),
        derivation_path,
        deposit_seed_source,
        created_at: chrono::Utc::now().timestamp(),
        status: "issued".to_string(),
    };

    persist_new_deposit_with_bridge_auth_replay(
        &state.db,
        &record,
        BRIDGE_AUTH_REPLAY_ACTION_CREATE_DEPOSIT,
        &replay_digest,
        bridge_auth.expires_at,
    )
    .map_err(|e| Json(ErrorResponse::db(&e)))?;

    emit_custody_event(
        &state,
        "deposit.created",
        &deposit_id,
        Some(&deposit_id),
        None,
        Some(&serde_json::json!({
            "user_id": record.user_id,
            "chain": record.chain,
            "asset": record.asset,
            "address": record.address
        })),
    );

    Ok(Json(CreateDepositResponse {
        deposit_id,
        address,
    }))
}

/// AUDIT-FIX F8.3 / P0-4: Deposit lookup requires both service Bearer auth and
/// user-signed bridge access auth so custody does not trust the proxy alone.
pub(super) async fn get_deposit(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(deposit_id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<BTreeMap<String, String>>,
) -> Result<Json<DepositRequest>, ErrorResponse> {
    verify_api_auth(&state.config, &headers)?;

    let user_id = query.get("user_id").ok_or_else(|| {
        Json(ErrorResponse::invalid(
            "Missing user_id: expected authenticated bridge lookup",
        ))
    })?;
    if Pubkey::from_base58(user_id).is_err() {
        return Err(ErrorResponse::invalid(
            "user_id must be a valid Lichen base58 public key (32 bytes)",
        ));
    }
    let bridge_auth_json = query.get("auth").ok_or_else(|| {
        Json(ErrorResponse::invalid(
            "Missing auth: expected wallet-signed bridge access",
        ))
    })?;
    let bridge_auth = parse_bridge_access_auth_json(bridge_auth_json)?;
    verify_bridge_access_auth(user_id, &bridge_auth)?;

    let record = fetch_deposit(&state.db, &deposit_id)
        .map_err(|e| Json(ErrorResponse::db(&e)))?
        .ok_or_else(|| Json(ErrorResponse::not_found("Deposit not found")))?;
    if record.user_id != *user_id {
        return Err(ErrorResponse::not_found(
            "Deposit not found for authenticated user",
        ));
    }
    Ok(Json(record))
}
