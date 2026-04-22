use super::*;

pub(crate) async fn create_webhook(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<CreateWebhookRequest>,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    if payload.url.is_empty() {
        return Err(Json(ErrorResponse::invalid("url is required")));
    }
    if payload.secret.is_empty() {
        return Err(Json(ErrorResponse::invalid(
            "secret is required (used for HMAC-SHA256 signatures)",
        )));
    }
    let is_local_destination = super::validation::is_local_webhook_destination(&payload.url)
        .map_err(|error| Json(ErrorResponse::invalid(&error)))?;
    let uses_https = payload.url.starts_with("https://");
    let uses_loopback_http = is_local_destination && payload.url.starts_with("http://");
    if !uses_https && !uses_loopback_http {
        return Err(Json(ErrorResponse::invalid(
            "webhook url must use HTTPS (loopback HTTP allowed for local dev)",
        )));
    }
    if let Err(error) = super::validation::validate_webhook_destination(&state.config, &payload.url)
    {
        return Err(Json(ErrorResponse::invalid(&error)));
    }

    let webhook = WebhookRegistration {
        id: Uuid::new_v4().to_string(),
        url: payload.url,
        secret: payload.secret,
        event_filter: payload.event_filter,
        active: true,
        created_at: chrono::Utc::now().timestamp(),
        description: payload.description,
    };

    super::storage::store_webhook(&state.db, &webhook)
        .map_err(|error| Json(ErrorResponse::db(&error)))?;
    info!("webhook registered: {} → {}", webhook.id, webhook.url);

    Ok(Json(json!({
        "id": webhook.id,
        "url": webhook.url,
        "event_filter": webhook.event_filter,
        "active": webhook.active,
        "created_at": webhook.created_at,
    })))
}

pub(crate) async fn list_webhooks(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    let webhooks = super::storage::list_all_webhooks(&state.db)
        .map_err(|error| Json(ErrorResponse::db(&error)))?;
    let redacted: Vec<Value> = webhooks
        .iter()
        .map(|webhook| {
            json!({
                "id": webhook.id,
                "url": webhook.url,
                "event_filter": webhook.event_filter,
                "active": webhook.active,
                "created_at": webhook.created_at,
                "description": webhook.description,
            })
        })
        .collect();

    Ok(Json(json!({ "webhooks": redacted })))
}

pub(crate) async fn delete_webhook(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(webhook_id): axum::extract::Path<String>,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    super::storage::remove_webhook(&state.db, &webhook_id)
        .map_err(|error| Json(ErrorResponse::db(&error)))?;
    info!("webhook removed: {}", webhook_id);

    Ok(Json(json!({ "deleted": webhook_id })))
}
