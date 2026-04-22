pub(super) fn parse_hex_u128(value: &str) -> Result<u128, String> {
    let trimmed = value.trim_start_matches("0x");
    u128::from_str_radix(trimmed, 16).map_err(|error| format!("parse hex: {}", error))
}

pub(super) fn parse_hex_u64(value: &str) -> Result<u64, String> {
    let trimmed = value.trim_start_matches("0x");
    u64::from_str_radix(trimmed, 16).map_err(|error| format!("parse hex: {}", error))
}
