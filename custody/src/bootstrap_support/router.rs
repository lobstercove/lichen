use super::*;

pub(crate) fn build_custody_app(state: CustodyState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/deposits", post(create_deposit))
        .route("/deposits/:deposit_id", get(get_deposit))
        .route("/withdrawals", post(create_withdrawal))
        // AUDIT-FIX C4: Endpoint for clients to submit their Lichen burn tx signature.
        // Without this, withdrawal jobs stay in "pending_burn" forever because
        // burn_tx_signature starts as None and nothing ever populates it.
        .route("/withdrawals/:job_id/burn", put(submit_burn_signature))
        .route(
            "/withdrawals/:job_id/confirm",
            put(confirm_withdrawal_operator),
        )
        .route("/reserves", get(get_reserves))
        .route("/webhooks", post(create_webhook))
        .route("/webhooks", get(list_webhooks))
        .route("/webhooks/:webhook_id", delete(delete_webhook))
        .route("/ws/events", get(ws_events))
        .route("/events", get(list_events))
        .layer(build_cors_layer())
        .with_state(state)
}

pub(crate) fn custody_listen_addr() -> SocketAddr {
    let port = std::env::var("CUSTODY_LISTEN_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(9105);
    format!("0.0.0.0:{}", port)
        .parse()
        .expect("valid bind addr")
}

fn build_cors_layer() -> CorsLayer {
    let mut origins: Vec<http::HeaderValue> = vec![
        "https://lichen.network".parse().unwrap(),
        "https://wallet.lichen.network".parse().unwrap(),
        "https://explorer.lichen.network".parse().unwrap(),
        "https://dex.lichen.network".parse().unwrap(),
    ];
    if std::env::var("DEV_CORS").is_ok() {
        origins.extend([
            "http://localhost:3000".parse().unwrap(),
            "http://localhost:8080".parse().unwrap(),
        ]);
    }
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            http::Method::GET,
            http::Method::POST,
            http::Method::PUT,
            http::Method::DELETE,
            http::Method::OPTIONS,
        ])
        .allow_headers([
            http::header::CONTENT_TYPE,
            http::header::AUTHORIZATION,
            http::header::HeaderName::from_static("x-custody-operator-token"),
        ])
}
