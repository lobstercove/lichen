use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use std::{
    net::SocketAddr,
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::error;

use super::models::FaucetResponse;

pub(super) fn error_json(status: StatusCode, message: &str) -> Response {
    Json(FaucetResponse {
        success: false,
        signature: None,
        amount: None,
        recipient: None,
        message: None,
        error: Some(message.to_string()),
    })
    .into_response()
    .with_status(status)
}

pub(super) fn error_response(status: StatusCode, message: &str) -> Response {
    error!("{}", message);
    error_json(status, message)
}

pub(super) fn extract_client_ip(
    headers: &HeaderMap,
    peer_addr: SocketAddr,
    trusted_proxies: &[String],
) -> String {
    let peer_ip = peer_addr.ip().to_string();
    if trusted_proxies.iter().any(|value| value == &peer_ip) {
        if let Some(forwarded) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return forwarded;
        }
        if let Some(real_ip) = headers
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return real_ip;
        }
    }
    peer_ip
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

trait ResponseExt {
    fn with_status(self, status: StatusCode) -> Response;
}

impl ResponseExt for Response {
    fn with_status(mut self, status: StatusCode) -> Response {
        *self.status_mut() = status;
        self
    }
}
