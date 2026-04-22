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

fn is_local_webhook_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

pub(super) fn is_local_webhook_destination(raw_url: &str) -> Result<bool, String> {
    Ok(is_local_webhook_host(&webhook_host_from_url(raw_url)?))
}

pub(crate) fn validate_webhook_destination(
    config: &CustodyConfig,
    raw_url: &str,
) -> Result<(), String> {
    let host = webhook_host_from_url(raw_url)?;
    if is_local_webhook_host(&host) {
        return Ok(());
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
