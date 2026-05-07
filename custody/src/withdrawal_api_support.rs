use super::*;

/// POST /withdrawals — User requests to withdraw wrapped tokens
///
/// Flow:
///   1. User signs a withdrawal authorization bound to asset, amount, and destination
///   2. User POSTs the signed withdrawal request to create a pending withdrawal job
///   3. User burns the wrapped asset on Lichen and submits the burn tx signature separately
///   4. Custody verifies the burn on Lichen
///   5. For lUSD: checks stablecoin reserves, queues rebalance if needed
///   6. Custody uses threshold signatures to send native assets on the destination chain
pub(super) async fn create_withdrawal(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<WithdrawalRequest>,
) -> Json<Value> {
    let preflight = match prepare_create_withdrawal_request(&state, &headers, req) {
        Ok(preflight) => preflight,
        Err(response) => return response,
    };
    let req = preflight.req;
    let now_secs = preflight.now_secs;
    let replay_digest = preflight.replay_digest;
    let withdrawal_auth_expires_at = preflight.withdrawal_auth_expires_at;

    let asset_lower = req.asset.clone();
    let velocity_snapshot =
        match build_withdrawal_velocity_snapshot(&state.config, &asset_lower, req.amount) {
            Ok(snapshot) => snapshot,
            Err(error) => return Json(json!({ "error": error })),
        };

    if let Some(response) =
        handle_withdrawal_auth_replay(&state, now_secs, &replay_digest, &req, &velocity_snapshot)
            .await
    {
        return response;
    }

    if let Err(response) = validate_withdrawal_request_destination(&req, &asset_lower) {
        return response;
    }

    if let Err(error) = ensure_withdrawal_restrictions_allow(
        &state,
        &req.user_id,
        &asset_lower,
        req.amount,
        &req.dest_chain,
        &req.preferred_stablecoin,
    )
    .await
    {
        return Json(json!({ "error": error }));
    }

    if let Err(response) = enforce_withdrawal_rate_limits(&state, &req).await {
        return response;
    }

    let preferred = match resolve_withdrawal_preferred_stablecoin(&state.db, &req, &asset_lower) {
        Ok(preferred) => preferred,
        Err(response) => return response,
    };

    complete_withdrawal_request(
        &state,
        &req,
        preferred,
        &velocity_snapshot,
        &replay_digest,
        withdrawal_auth_expires_at,
    )
}

#[derive(Deserialize)]
pub(super) struct BurnSignaturePayload {
    pub(super) burn_tx_signature: String,
}

#[derive(Deserialize)]
pub(super) struct WithdrawalOperatorConfirmationPayload {
    #[serde(default)]
    pub(super) note: Option<String>,
}

/// AUDIT-FIX C4: Endpoint for clients to submit the Lichen burn tx signature.
///
/// PUT /withdrawals/:job_id/burn
///
/// After a user burns their wrapped tokens on Lichen, they submit the burn tx
/// signature here. The withdrawal worker then verifies it and progresses the job.
/// Without this endpoint, withdrawal jobs would hang at "pending_burn" forever.
pub(super) async fn submit_burn_signature(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(job_id): axum::extract::Path<String>,
    Json(payload): Json<BurnSignaturePayload>,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    if payload.burn_tx_signature.is_empty() {
        return Err(Json(ErrorResponse::invalid("burn_tx_signature required")));
    }

    submit_pending_burn_signature(&state, &job_id, payload.burn_tx_signature).await
}

pub(super) async fn confirm_withdrawal_operator(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(job_id): axum::extract::Path<String>,
    Json(payload): Json<WithdrawalOperatorConfirmationPayload>,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    let operator_id = verify_operator_confirmation_auth(&state.config, &headers)?;

    process_withdrawal_operator_confirmation(&state, &job_id, &operator_id, payload.note)
}
