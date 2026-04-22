use crate::client::ContractInfo;
use crate::contract_readonly_support::parse_json_u64;

pub(super) fn build_token_registry_metadata(
    decimals: u8,
    description: Option<&str>,
    website: Option<&str>,
    logo_url: Option<&str>,
    twitter: Option<&str>,
    telegram: Option<&str>,
    discord: Option<&str>,
) -> Option<serde_json::Value> {
    let mut meta = serde_json::Map::new();
    meta.insert("decimals".to_string(), serde_json::json!(decimals));
    meta.insert("mintable".to_string(), serde_json::json!(true));
    meta.insert("burnable".to_string(), serde_json::json!(true));

    if let Some(value) = description.filter(|value| !value.trim().is_empty()) {
        meta.insert("description".to_string(), serde_json::json!(value.trim()));
    }
    if let Some(value) = website.filter(|value| !value.trim().is_empty()) {
        meta.insert("website".to_string(), serde_json::json!(value.trim()));
    }
    if let Some(value) = logo_url.filter(|value| !value.trim().is_empty()) {
        meta.insert("logo_url".to_string(), serde_json::json!(value.trim()));
    }
    if let Some(value) = twitter.filter(|value| !value.trim().is_empty()) {
        meta.insert("twitter".to_string(), serde_json::json!(value.trim()));
    }
    if let Some(value) = telegram.filter(|value| !value.trim().is_empty()) {
        meta.insert("telegram".to_string(), serde_json::json!(value.trim()));
    }
    if let Some(value) = discord.filter(|value| !value.trim().is_empty()) {
        meta.insert("discord".to_string(), serde_json::json!(value.trim()));
    }

    if meta.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(meta))
    }
}

fn parse_json_u8(value: &serde_json::Value) -> Option<u8> {
    parse_json_u64(value).and_then(|parsed| u8::try_from(parsed).ok())
}

pub(super) fn token_decimals(
    registry: Option<&serde_json::Value>,
    info: Option<&ContractInfo>,
) -> u8 {
    registry
        .and_then(|entry| entry.get("decimals"))
        .and_then(parse_json_u8)
        .or_else(|| {
            info.and_then(|contract| contract.token_metadata.as_ref())
                .and_then(|meta| meta.get("decimals").or_else(|| meta.get("token_decimals")))
                .and_then(parse_json_u8)
        })
        .unwrap_or(9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_token_registry_metadata_ignores_blank_optional_fields() {
        let metadata = build_token_registry_metadata(
            9,
            Some(" test token "),
            Some("   "),
            None,
            Some("https://x.example"),
            None,
            None,
        )
        .unwrap();

        assert_eq!(
            metadata.get("decimals").and_then(|value| value.as_u64()),
            Some(9)
        );
        assert_eq!(
            metadata.get("description").and_then(|value| value.as_str()),
            Some("test token")
        );
        assert!(metadata.get("website").is_none());
        assert_eq!(
            metadata.get("twitter").and_then(|value| value.as_str()),
            Some("https://x.example")
        );
    }
}
