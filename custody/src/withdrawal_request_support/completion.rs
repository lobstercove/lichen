use super::preflight::build_create_withdrawal_response;
use super::*;

pub(crate) async fn enforce_withdrawal_rate_limits(
    state: &CustodyState,
    req: &WithdrawalRequest,
) -> Result<(), Json<Value>> {
    let mut rl = state.withdrawal_rate.lock().await;
    let now = std::time::Instant::now();

    if now.duration_since(rl.window_start) >= std::time::Duration::from_secs(60) {
        rl.window_start = now;
        rl.count_this_minute = 0;
        rl.count_warning_level = None;
    }
    if now.duration_since(rl.hour_start) >= std::time::Duration::from_secs(3600) {
        rl.hour_start = now;
        rl.value_this_hour = 0;
        rl.value_warning_level = None;
    }

    const MAX_WITHDRAWALS_PER_MIN: u64 = 20;
    const MAX_VALUE_PER_HOUR: u64 = 10_000_000_000_000_000;
    let projected_count_this_minute = rl.count_this_minute.saturating_add(1);
    let projected_value_this_hour = rl.value_this_hour.saturating_add(req.amount);
    let velocity_metrics = WithdrawalVelocityMetrics {
        count_this_minute: rl.count_this_minute,
        max_withdrawals_per_min: MAX_WITHDRAWALS_PER_MIN,
        value_this_hour: rl.value_this_hour,
        max_value_per_hour: MAX_VALUE_PER_HOUR,
    };

    if projected_count_this_minute > MAX_WITHDRAWALS_PER_MIN {
        tracing::warn!(
            "⚠️  Withdrawal rate limit exceeded: {} this minute",
            rl.count_this_minute
        );
        emit_withdrawal_spike_event(
            state,
            req,
            "count_per_minute",
            rl.count_this_minute,
            MAX_WITHDRAWALS_PER_MIN,
            rl.value_this_hour,
            MAX_VALUE_PER_HOUR,
        );
        return Err(Json(
            json!({ "error": "rate_limited: too many withdrawals, try again later" }),
        ));
    }

    if projected_value_this_hour > MAX_VALUE_PER_HOUR {
        tracing::warn!(
            "⚠️  Withdrawal value limit exceeded: {} this hour",
            rl.value_this_hour
        );
        emit_withdrawal_spike_event(
            state,
            req,
            "value_per_hour",
            rl.count_this_minute,
            MAX_WITHDRAWALS_PER_MIN,
            rl.value_this_hour,
            MAX_VALUE_PER_HOUR,
        );
        return Err(Json(json!({
            "error": "rate_limited: hourly withdrawal value limit reached"
        })));
    }

    if let Some(last) = rl.per_address.get(&req.dest_address) {
        if now.duration_since(*last) < std::time::Duration::from_secs(30) {
            return Err(Json(
                json!({ "error": "rate_limited: wait 30s between withdrawals" }),
            ));
        }
    }

    if let Some(level) = next_withdrawal_warning_level(
        projected_count_this_minute,
        MAX_WITHDRAWALS_PER_MIN,
        rl.count_warning_level,
    ) {
        emit_withdrawal_velocity_warning_event(
            state,
            req,
            "count_per_minute",
            level,
            velocity_metrics,
        );
        rl.count_warning_level = Some(level);
    }

    if let Some(level) = next_withdrawal_warning_level(
        projected_value_this_hour,
        MAX_VALUE_PER_HOUR,
        rl.value_warning_level,
    ) {
        emit_withdrawal_velocity_warning_event(
            state,
            req,
            "value_per_hour",
            level,
            velocity_metrics,
        );
        rl.value_warning_level = Some(level);
    }

    rl.count_this_minute = projected_count_this_minute;
    rl.value_this_hour = projected_value_this_hour;
    rl.per_address.insert(req.dest_address.clone(), now);
    if let Err(error) = persist_withdrawal_rate_state(&state.db, &rl) {
        return Err(Json(json!({ "error": format!("db error: {}", error) })));
    }

    Ok(())
}

pub(crate) fn validate_withdrawal_request_destination(
    req: &WithdrawalRequest,
    asset_lower: &str,
) -> Result<(), Json<Value>> {
    match req.dest_chain.as_str() {
        "solana" => {
            if bs58::decode(&req.dest_address)
                .into_vec()
                .map(|value| value.len())
                .unwrap_or(0)
                != 32
            {
                return Err(Json(json!({
                    "error": format!("invalid Solana destination address: {}", req.dest_address)
                })));
            }
        }
        "ethereum" | "eth" | "bsc" | "bnb" => {
            let trimmed = req.dest_address.trim_start_matches("0x");
            if trimmed.len() != 40 || hex::decode(trimmed).is_err() {
                return Err(Json(json!({
                    "error": format!("invalid EVM destination address: {}", req.dest_address)
                })));
            }
        }
        _ => {
            return Err(Json(json!({
                "error": format!("unsupported destination chain: {}", req.dest_chain)
            })));
        }
    }

    let dest_asset = match asset_lower {
        "musd" => "stablecoin",
        "wsol" => "sol",
        "weth" => "eth",
        "wbnb" => "bnb",
        _ => {
            return Err(Json(json!({
                "error": format!("unsupported withdrawal asset: {}", req.asset)
            })));
        }
    };

    let valid_chain = match dest_asset {
        "sol" => req.dest_chain == "solana",
        "eth" => req.dest_chain == "ethereum" || req.dest_chain == "eth",
        "bnb" => req.dest_chain == "bsc" || req.dest_chain == "bnb",
        "stablecoin" => {
            req.dest_chain == "solana"
                || req.dest_chain == "ethereum"
                || req.dest_chain == "eth"
                || req.dest_chain == "bsc"
                || req.dest_chain == "bnb"
        }
        _ => false,
    };
    if !valid_chain {
        return Err(Json(json!({
            "error": format!("cannot withdraw {} to {}", req.asset, req.dest_chain)
        })));
    }

    Ok(())
}

pub(crate) fn resolve_withdrawal_preferred_stablecoin(
    db: &DB,
    req: &WithdrawalRequest,
    asset_lower: &str,
) -> Result<String, Json<Value>> {
    if asset_lower != "musd" {
        return Ok("usdt".to_string());
    }

    let pref = req.preferred_stablecoin.clone();
    if pref != "usdt" && pref != "usdc" {
        return Err(Json(json!({
            "error": format!("preferred_stablecoin must be 'usdt' or 'usdc', got '{}'", pref)
        })));
    }

    let chain_amount = spores_to_chain_amount(req.amount, &req.dest_chain, &pref);
    let chain_amount_u64 = u64::try_from(chain_amount).unwrap_or(u64::MAX);

    let reserve = get_reserve_balance(db, &req.dest_chain, &pref).unwrap_or(0);
    let other = if pref == "usdt" { "usdc" } else { "usdt" };
    let other_reserve = get_reserve_balance(db, &req.dest_chain, other).unwrap_or(0);
    let total_on_chain = reserve.saturating_add(other_reserve);

    if chain_amount_u64 > total_on_chain {
        return Err(Json(json!({
            "error": format!(
                "insufficient total stablecoin reserves on {}: requested {} (chain units), available {} ({} {} + {} {})",
                req.dest_chain, chain_amount_u64, total_on_chain, reserve, pref, other_reserve, other
            )
        })));
    }

    if reserve < chain_amount_u64 {
        let deficit = chain_amount_u64 - reserve;
        let rebalance_job = RebalanceJob {
            job_id: Uuid::new_v4().to_string(),
            chain: req.dest_chain.clone(),
            from_asset: other.to_string(),
            to_asset: pref.clone(),
            amount: deficit,
            trigger: "withdrawal".to_string(),
            linked_withdrawal_job_id: None,
            swap_tx_hash: None,
            status: "queued".to_string(),
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            created_at: chrono::Utc::now().timestamp(),
        };

        info!(
            "reserve deficit: need {} more {} on {} — queuing rebalance from {} (job={})",
            deficit, pref, req.dest_chain, other, rebalance_job.job_id
        );

        if let Err(error) = store_rebalance_job(db, &rebalance_job) {
            return Err(Json(
                json!({"error": format!("failed to queue rebalance: {}", error)}),
            ));
        }
    }

    Ok(pref)
}

pub(crate) fn complete_withdrawal_request(
    state: &CustodyState,
    req: &WithdrawalRequest,
    preferred: String,
    velocity_snapshot: &WithdrawalVelocitySnapshot,
    replay_digest: &str,
    withdrawal_auth_expires_at: u64,
) -> Json<Value> {
    let job = WithdrawalJob {
        job_id: Uuid::new_v4().to_string(),
        user_id: req.user_id.clone(),
        asset: req.asset.clone(),
        amount: req.amount,
        dest_chain: req.dest_chain.clone(),
        dest_address: req.dest_address.clone(),
        preferred_stablecoin: preferred,
        burn_tx_signature: None,
        outbound_tx_hash: None,
        safe_nonce: None,
        signatures: Vec::new(),
        velocity_tier: velocity_snapshot.tier,
        required_signer_threshold: velocity_snapshot.required_signer_threshold,
        required_operator_confirmations: velocity_snapshot.required_operator_confirmations,
        release_after: None,
        burn_confirmed_at: None,
        operator_confirmations: Vec::new(),
        status: "pending_burn".to_string(),
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        created_at: chrono::Utc::now().timestamp(),
    };

    if let Err(error) = persist_new_withdrawal_with_auth_replay(
        &state.db,
        &job,
        BRIDGE_AUTH_REPLAY_ACTION_CREATE_WITHDRAWAL,
        replay_digest,
        withdrawal_auth_expires_at,
    ) {
        return Json(json!({
            "error": format!("failed to store withdrawal: {}", error)
        }));
    }

    emit_custody_event(
        state,
        "withdrawal.requested",
        &job.job_id,
        None,
        None,
        Some(&json!({
            "user_id": job.user_id,
            "asset": job.asset,
            "amount": job.amount,
            "dest_chain": job.dest_chain,
            "dest_address": job.dest_address,
        })),
    );

    info!(
        "withdrawal requested: {} {} → {} on {} (preferred_stablecoin={}, job={})",
        job.amount,
        job.asset,
        job.dest_address,
        job.dest_chain,
        job.preferred_stablecoin,
        job.job_id
    );

    Json(build_create_withdrawal_response(&job, velocity_snapshot))
}
