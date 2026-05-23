use super::*;

pub(crate) fn compute_webhook_signature(payload: &[u8], secret: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload);
    let result = mac.finalize().into_bytes();
    hex::encode(result)
}

fn webhook_host_from_url(raw_url: &str) -> Result<String, String> {
    let parsed =
        reqwest::Url::parse(raw_url).map_err(|error| format!("invalid webhook url: {}", error))?;
    parsed
        .host_str()
        .map(|host| host.to_ascii_lowercase())
        .ok_or_else(|| "webhook url must include a valid host".to_string())
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

pub(super) fn local_webhook_override_enabled() -> bool {
    env_truthy("LICHEN_LOCAL_DEV") && env_truthy("CUSTODY_ALLOW_LOCAL_WEBHOOKS")
}

fn classified_internal_webhook_host(host: &str) -> Option<&'static str> {
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    if host == "localhost" {
        return Some("loopback");
    }
    let ip: std::net::IpAddr = host.parse().ok()?;
    match ip {
        std::net::IpAddr::V4(addr) => {
            if addr.is_loopback() {
                Some("loopback")
            } else if addr.is_private() {
                Some("private")
            } else if addr.is_link_local() {
                Some("link-local")
            } else if addr.is_unspecified() {
                Some("unspecified")
            } else {
                None
            }
        }
        std::net::IpAddr::V6(addr) => {
            let segments = addr.segments();
            if addr.is_loopback() {
                Some("loopback")
            } else if (segments[0] & 0xfe00) == 0xfc00 {
                Some("private")
            } else if (segments[0] & 0xffc0) == 0xfe80 {
                Some("link-local")
            } else if addr.is_unspecified() {
                Some("unspecified")
            } else {
                None
            }
        }
    }
}

pub(super) fn is_local_webhook_destination(raw_url: &str) -> Result<bool, String> {
    Ok(classified_internal_webhook_host(&webhook_host_from_url(raw_url)?).is_some())
}

pub(crate) fn validate_webhook_destination(
    config: &CustodyConfig,
    raw_url: &str,
) -> Result<(), String> {
    validate_webhook_destination_with_mode(config, raw_url, local_webhook_override_enabled())
}

pub(crate) fn validate_webhook_destination_with_mode(
    config: &CustodyConfig,
    raw_url: &str,
    allow_internal_webhooks: bool,
) -> Result<(), String> {
    let host = webhook_host_from_url(raw_url)?;
    if let Some(classification) = classified_internal_webhook_host(&host) {
        if allow_internal_webhooks {
            return Ok(());
        }
        return Err(format!(
            "webhook host '{}' is {} or internal; set LICHEN_LOCAL_DEV=1 and CUSTODY_ALLOW_LOCAL_WEBHOOKS=1 only for local development",
            host, classification
        ));
    }
    if config.webhook_allowed_hosts.is_empty() {
        return Err(
            "non-local webhooks require CUSTODY_WEBHOOK_ALLOWED_HOSTS to be configured".to_string(),
        );
    }

    if config
        .webhook_allowed_hosts
        .iter()
        .any(|allowed| allowed == &host)
    {
        Ok(())
    } else {
        Err(format!(
            "webhook host '{}' is not in CUSTODY_WEBHOOK_ALLOWED_HOSTS",
            host
        ))
    }
}
