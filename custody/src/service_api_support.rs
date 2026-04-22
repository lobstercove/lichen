use super::*;

pub(super) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub(super) async fn status(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    let sweep_counts =
        count_sweep_jobs(&state.db).map_err(|error| Json(ErrorResponse::db(&error)))?;
    let credit_counts =
        count_credit_jobs(&state.db).map_err(|error| Json(ErrorResponse::db(&error)))?;
    let withdrawal_counts =
        count_withdrawal_jobs(&state.db).map_err(|error| Json(ErrorResponse::db(&error)))?;

    Ok(Json(json!({
        "signers": {
            "configured": state.config.signer_endpoints.len(),
            "threshold": state.config.signer_threshold,
        },
        "sweeps": sweep_counts,
        "credits": credit_counts,
        "withdrawals": withdrawal_counts,
    })))
}

pub(super) async fn get_reserves(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    Ok(build_reserve_ledger_response(&state.db))
}
