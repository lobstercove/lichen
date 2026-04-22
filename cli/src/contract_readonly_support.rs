use anyhow::{Context, Result};
use base64::Engine;

pub(super) fn parse_json_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
}

pub(super) fn decode_readonly_u64(result: &serde_json::Value, function: &str) -> Result<u64> {
    if matches!(
        result.get("success").and_then(|value| value.as_bool()),
        Some(false)
    ) {
        let message = result
            .get("error")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown contract error");
        anyhow::bail!("{} returned an error: {}", function, message);
    }

    let return_data = result
        .get("returnData")
        .or_else(|| result.get("return_data"))
        .context("Missing returnData in readonly contract response")?;

    if let Some(value) = parse_json_u64(return_data) {
        return Ok(value);
    }

    if let Some(bytes) = return_data.as_array() {
        if bytes.len() >= 8 && bytes.iter().all(|value| value.as_u64().is_some()) {
            let mut raw = [0u8; 8];
            for (index, value) in bytes.iter().take(8).enumerate() {
                raw[index] = value.as_u64().unwrap_or(0) as u8;
            }
            return Ok(u64::from_le_bytes(raw));
        }
    }

    if let Some(text) = return_data.as_str() {
        if let Ok(parsed) = text.parse::<u64>() {
            return Ok(parsed);
        }

        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(text) {
            if bytes.len() >= 8 {
                let mut raw = [0u8; 8];
                raw.copy_from_slice(&bytes[..8]);
                return Ok(u64::from_le_bytes(raw));
            }
        }
    }

    anyhow::bail!(
        "Could not decode {} return value from readonly contract response: {}",
        function,
        return_data
    )
}
