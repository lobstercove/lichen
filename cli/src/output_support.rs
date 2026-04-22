/// Convert LICN (f64) to spores (u64) with precise integer arithmetic.
/// Avoids floating-point precision loss for amounts near the f64 precision boundary
/// by splitting into whole and fractional parts and computing with integers.
pub(super) fn licn_to_spores(lichen: f64) -> u64 {
    if lichen <= 0.0 {
        return 0;
    }

    if lichen >= (u64::MAX / 1_000_000_000) as f64 {
        return u64::MAX;
    }

    let whole = lichen.trunc() as u64;
    let frac = ((lichen.fract() * 1_000_000_000.0).round()) as u64;
    whole.saturating_mul(1_000_000_000).saturating_add(frac)
}

pub(super) fn print_json(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
    );
}

pub(super) fn to_licn(spores: u64) -> f64 {
    spores as f64 / 1_000_000_000.0
}

pub(super) fn print_defi_stats(stats: &serde_json::Value) {
    if let Some(obj) = stats.as_object() {
        for (key, value) in obj {
            let label = key.replace('_', " ");
            if let Some(n) = value.as_u64() {
                if n > 1_000_000_000 {
                    println!("  {}: {:.4} LICN", label, to_licn(n));
                } else {
                    println!("  {}: {}", label, n);
                }
            } else if let Some(f) = value.as_f64() {
                println!("  {}: {:.4}", label, f);
            } else if let Some(s) = value.as_str() {
                println!("  {}: {}", label, s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::licn_to_spores;

    #[test]
    fn test_licn_to_spores_basic() {
        assert_eq!(licn_to_spores(1.0), 1_000_000_000);
        assert_eq!(licn_to_spores(0.5), 500_000_000);
        assert_eq!(licn_to_spores(100.0), 100_000_000_000);
    }

    #[test]
    fn test_licn_to_spores_zero_and_negative() {
        assert_eq!(licn_to_spores(0.0), 0);
        assert_eq!(licn_to_spores(-1.0), 0);
        assert_eq!(licn_to_spores(-0.001), 0);
    }

    #[test]
    fn test_licn_to_spores_fractional_precision() {
        assert_eq!(licn_to_spores(0.000000001), 1);
        assert_eq!(licn_to_spores(1.123456789), 1_123_456_789);
        assert_eq!(licn_to_spores(0.1), 100_000_000);
        assert_eq!(licn_to_spores(0.01), 10_000_000);
    }

    #[test]
    fn test_licn_to_spores_large_values() {
        assert_eq!(licn_to_spores(1_000_000.0), 1_000_000_000_000_000);
        assert_eq!(licn_to_spores(f64::MAX), u64::MAX);
    }

    #[test]
    fn test_licn_to_spores_saturating() {
        let huge = (u64::MAX / 1_000_000_000) as f64 + 1.0;
        assert_eq!(licn_to_spores(huge), u64::MAX);
    }
}
