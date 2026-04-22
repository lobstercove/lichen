use anyhow::Result;

pub(super) fn format_token_amount(amount: u64, decimals: u8) -> String {
    if decimals == 0 {
        return amount.to_string();
    }

    let scale = 10u128.pow(decimals as u32);
    let amount = u128::from(amount);
    let whole = amount / scale;
    let fractional = amount % scale;
    if fractional == 0 {
        return whole.to_string();
    }

    let mut fractional_text = format!("{:0width$}", fractional, width = decimals as usize);
    while fractional_text.ends_with('0') {
        fractional_text.pop();
    }

    format!("{}.{}", whole, fractional_text)
}

pub(super) fn scale_whole_token_amount(amount: u64, decimals: u8) -> Result<u64> {
    let scale = 10u64
        .checked_pow(decimals as u32)
        .ok_or_else(|| anyhow::anyhow!("Token decimals {} are too large", decimals))?;
    amount.checked_mul(scale).ok_or_else(|| {
        anyhow::anyhow!(
            "Token amount overflow: {} whole units with {} decimals",
            amount,
            decimals
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_token_amount_trims_trailing_zeroes() {
        assert_eq!(format_token_amount(1_230_000_000, 9), "1.23");
        assert_eq!(format_token_amount(42, 0), "42");
    }

    #[test]
    fn test_scale_whole_token_amount_scales_by_decimals() {
        assert_eq!(scale_whole_token_amount(7, 9).unwrap(), 7_000_000_000);
    }
}
