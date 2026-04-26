use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use std::{
    net::{IpAddr, SocketAddr},
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
    let peer_ip = peer_addr.ip();
    if is_trusted_proxy(peer_ip, trusted_proxies) {
        if let Some(forwarded) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .and_then(parse_header_ip)
        {
            return forwarded.to_string();
        }
        if let Some(real_ip) = headers
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .and_then(parse_header_ip)
        {
            return real_ip.to_string();
        }
    }
    peer_ip.to_string()
}

fn is_trusted_proxy(peer_ip: IpAddr, trusted_proxies: &[String]) -> bool {
    trusted_proxies
        .iter()
        .filter_map(|value| value.parse::<IpAddr>().ok())
        .any(|trusted| trusted == peer_ip)
}

fn parse_header_ip(value: &str) -> Option<IpAddr> {
    value.trim().parse::<IpAddr>().ok()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(ip: &str) -> SocketAddr {
        SocketAddr::new(ip.parse().expect("test peer ip"), 443)
    }

    #[test]
    fn extract_client_ip_ignores_forwarded_headers_from_untrusted_peer() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "198.51.100.7".parse().unwrap());

        let actual = extract_client_ip(&headers, peer("203.0.113.1"), &["127.0.0.1".to_string()]);

        assert_eq!(actual, "203.0.113.1");
    }

    #[test]
    fn extract_client_ip_accepts_valid_forwarded_header_from_trusted_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            " 198.51.100.7, 198.51.100.8 ".parse().unwrap(),
        );

        let actual = extract_client_ip(&headers, peer("127.0.0.1"), &["127.0.0.1".to_string()]);

        assert_eq!(actual, "198.51.100.7");
    }

    #[test]
    fn extract_client_ip_rejects_malformed_forwarded_header_values() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "not-an-ip".parse().unwrap());

        let actual = extract_client_ip(&headers, peer("127.0.0.1"), &["127.0.0.1".to_string()]);

        assert_eq!(actual, "127.0.0.1");
    }

    #[test]
    fn extract_client_ip_falls_back_to_valid_real_ip_from_trusted_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        headers.insert("x-real-ip", "2001:db8::42".parse().unwrap());

        let actual = extract_client_ip(&headers, peer("::1"), &["::1".to_string()]);

        assert_eq!(actual, "2001:db8::42");
    }
}
