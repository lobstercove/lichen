use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WithdrawalOperatorConfirmation {
    pub(crate) operator_id: String,
    pub(crate) confirmed_at: i64,
    #[serde(default)]
    pub(crate) note: Option<String>,
}

pub(crate) fn operator_token_fingerprint(token: &str) -> String {
    use sha2::Digest;

    let digest = sha2::Sha256::digest(token.as_bytes());
    format!("operator-{}", hex::encode(&digest[..6]))
}

pub(crate) fn verify_operator_confirmation_auth(
    config: &CustodyConfig,
    headers: &axum::http::HeaderMap,
) -> Result<String, Json<ErrorResponse>> {
    let provided = headers
        .get("x-custody-operator-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");

    if provided.is_empty() {
        return Err(Json(ErrorResponse {
            code: "unauthorized",
            message: "Invalid or missing X-Custody-Operator-Token".to_string(),
        }));
    }

    use subtle::ConstantTimeEq;
    for token in &config
        .withdrawal_velocity_policy
        .operator_confirmation_tokens
    {
        let matches = bool::from(provided.as_bytes().ct_eq(token.as_bytes()));
        if matches {
            return Ok(operator_token_fingerprint(token));
        }
    }

    Err(Json(ErrorResponse {
        code: "unauthorized",
        message: "Invalid or missing X-Custody-Operator-Token".to_string(),
    }))
}

fn record_operator_confirmation(
    job: &mut WithdrawalJob,
    operator_id: &str,
    note: Option<String>,
) -> bool {
    if let Some(existing) = job
        .operator_confirmations
        .iter_mut()
        .find(|entry| entry.operator_id == operator_id)
    {
        if note.is_some() {
            existing.note = note;
        }
        return false;
    }

    job.operator_confirmations
        .push(WithdrawalOperatorConfirmation {
            operator_id: operator_id.to_string(),
            confirmed_at: chrono::Utc::now().timestamp(),
            note,
        });
    true
}

pub(crate) fn process_withdrawal_operator_confirmation(
    state: &CustodyState,
    job_id: &str,
    operator_id: &str,
    note: Option<String>,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    let mut job = fetch_withdrawal_job(&state.db, job_id)
        .map_err(|error| Json(ErrorResponse::db(&error)))?
        .ok_or_else(|| Json(ErrorResponse::invalid("withdrawal not found")))?;

    if matches!(
        job.status.as_str(),
        "confirmed" | "expired" | "failed" | "permanently_failed"
    ) {
        return Err(Json(ErrorResponse::invalid(&format!(
            "withdrawal {} is no longer confirmable (current: {})",
            job_id, job.status
        ))));
    }

    if job.required_operator_confirmations == 0 {
        return Err(Json(ErrorResponse::invalid(
            "withdrawal does not require operator confirmation",
        )));
    }

    let added = record_operator_confirmation(&mut job, operator_id, note.clone());
    store_withdrawal_job(&state.db, &job).map_err(|error| Json(ErrorResponse::db(&error)))?;

    if added {
        emit_custody_event(
            state,
            "withdrawal.operator_confirmed",
            &job.job_id,
            None,
            None,
            Some(&json!({
                "operator_id": operator_id,
                "required_operator_confirmations": job.required_operator_confirmations,
                "received_operator_confirmations": job.operator_confirmations.len(),
                "velocity_tier": job.velocity_tier.as_str(),
                "release_after": job.release_after,
                "note": note,
            })),
        );
    }

    Ok(Json(json!({
        "job_id": job.job_id,
        "status": job.status,
        "velocity_tier": job.velocity_tier.as_str(),
        "operator_confirmation_added": added,
        "required_operator_confirmations": job.required_operator_confirmations,
        "received_operator_confirmations": job.operator_confirmations.len(),
        "release_after": job.release_after,
    })))
}
