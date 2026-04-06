//! Common types used in the SDK

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

const SPORES_PER_LICN: u64 = 1_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BalanceParseError {
    Empty,
    Negative,
    InvalidFormat,
    TooManyFractionalDigits,
    Overflow,
}

impl fmt::Display for BalanceParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "LICN amount cannot be empty"),
            Self::Negative => write!(f, "LICN amount cannot be negative"),
            Self::InvalidFormat => write!(f, "LICN amount must be a decimal string"),
            Self::TooManyFractionalDigits => {
                write!(f, "LICN amount supports at most 9 fractional digits")
            }
            Self::Overflow => write!(f, "LICN amount exceeds supported range"),
        }
    }
}

impl std::error::Error for BalanceParseError {}

/// Account balance
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Balance {
    spores: u64,
}

impl Balance {
    /// Create from spores
    pub fn from_spores(spores: u64) -> Self {
        Self { spores }
    }

    /// Create from a decimal LICN string without going through floating-point rounding.
    pub fn from_licn(licn: &str) -> Result<Self, BalanceParseError> {
        let licn = licn.trim();
        if licn.is_empty() {
            return Err(BalanceParseError::Empty);
        }

        if licn.starts_with('-') {
            return Err(BalanceParseError::Negative);
        }

        let licn = licn.strip_prefix('+').unwrap_or(licn);
        let mut parts = licn.split('.');
        let whole_part = parts.next().unwrap_or_default();
        let frac_part = parts.next();
        if parts.next().is_some() {
            return Err(BalanceParseError::InvalidFormat);
        }

        if whole_part.is_empty() && frac_part.unwrap_or_default().is_empty() {
            return Err(BalanceParseError::InvalidFormat);
        }

        if !whole_part.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(BalanceParseError::InvalidFormat);
        }

        let whole_spores = if whole_part.is_empty() {
            0
        } else {
            whole_part
                .parse::<u64>()
                .map_err(|_| BalanceParseError::Overflow)?
                .checked_mul(SPORES_PER_LICN)
                .ok_or(BalanceParseError::Overflow)?
        };

        let frac_spores = if let Some(frac_part) = frac_part {
            if !frac_part.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(BalanceParseError::InvalidFormat);
            }
            if frac_part.len() > 9 {
                return Err(BalanceParseError::TooManyFractionalDigits);
            }
            if frac_part.is_empty() {
                0
            } else {
                let frac_digits = frac_part
                    .parse::<u64>()
                    .map_err(|_| BalanceParseError::Overflow)?;
                frac_digits
                    .checked_mul(10_u64.pow((9 - frac_part.len()) as u32))
                    .ok_or(BalanceParseError::Overflow)?
            }
        } else {
            0
        };

        Ok(Self {
            spores: whole_spores
                .checked_add(frac_spores)
                .ok_or(BalanceParseError::Overflow)?,
        })
    }

    pub fn from_licn_parts(whole_licn: u64, fractional_spores: u32) -> Result<Self, BalanceParseError> {
        if fractional_spores >= SPORES_PER_LICN as u32 {
            return Err(BalanceParseError::TooManyFractionalDigits);
        }

        let whole_spores = whole_licn
            .checked_mul(SPORES_PER_LICN)
            .ok_or(BalanceParseError::Overflow)?;

        Ok(Self {
            spores: whole_spores
                .checked_add(fractional_spores as u64)
                .ok_or(BalanceParseError::Overflow)?,
        })
    }

    /// Get spores
    pub fn spores(&self) -> u64 {
        self.spores
    }

    /// Get LICN
    pub fn licn(&self) -> f64 {
        self.spores as f64 / SPORES_PER_LICN as f64
    }
}

impl FromStr for Balance {
    type Err = BalanceParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_licn(s)
    }
}

/// Block information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub hash: String,
    pub parent_hash: String,
    pub slot: u64,
    pub state_root: String,
    pub timestamp: u64,
    pub transaction_count: u64,
    pub validator: String,
}

/// Network information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub chain_id: String,
    pub current_slot: u64,
    pub network_id: String,
    pub peer_count: u64,
    pub validator_count: u64,
    pub version: String,
}

/// Re-export transaction from core
pub use lichen_core::Transaction;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_balance_roundtrip() {
        let b = Balance::from_spores(1_500_000_000);
        assert_eq!(b.spores(), 1_500_000_000);
        assert!((b.licn() - 1.5).abs() < 1e-9);
    }

    #[test]
    fn test_balance_from_licn_normal() {
        let b = Balance::from_licn("2.5").unwrap();
        assert_eq!(b.spores(), 2_500_000_000);
    }

    #[test]
    fn test_balance_from_licn_negative() {
        assert_eq!(
            Balance::from_licn("-1.0").unwrap_err(),
            BalanceParseError::Negative
        );
    }

    #[test]
    fn test_balance_from_licn_invalid_format() {
        assert_eq!(
            Balance::from_licn("NaN").unwrap_err(),
            BalanceParseError::InvalidFormat
        );
    }

    #[test]
    fn test_balance_from_licn_overflow() {
        assert_eq!(
            Balance::from_licn("18446744074").unwrap_err(),
            BalanceParseError::Overflow
        );
    }

    #[test]
    fn test_balance_from_licn_zero() {
        let b = Balance::from_licn("0").unwrap();
        assert_eq!(b.spores(), 0);
    }

    #[test]
    fn test_balance_empty_amount() {
        assert_eq!(
            Balance::from_licn("  ").unwrap_err(),
            BalanceParseError::Empty
        );
    }

    #[test]
    fn test_balance_from_licn_tiny_fraction() {
        let b = Balance::from_licn("0.000000001").unwrap();
        assert_eq!(b.spores(), 1);
    }

    #[test]
    fn test_balance_from_licn_sub_spore() {
        assert_eq!(
            Balance::from_licn("0.0000000001").unwrap_err(),
            BalanceParseError::TooManyFractionalDigits
        );
    }

    #[test]
    fn test_balance_from_licn_leading_decimal() {
        let b = Balance::from_licn(".5").unwrap();
        assert_eq!(b.spores(), 500_000_000);
    }

    #[test]
    fn test_balance_from_licn_trailing_decimal() {
        let b = Balance::from_licn("1.").unwrap();
        assert_eq!(b.spores(), 1_000_000_000);
    }

    #[test]
    fn test_balance_from_licn_parts() {
        let b = Balance::from_licn_parts(12, 345_000_000).unwrap();
        assert_eq!(b.spores(), 12_345_000_000);
    }

    #[test]
    fn test_balance_from_spores_max() {
        let b = Balance::from_spores(u64::MAX);
        assert_eq!(b.spores(), u64::MAX);
    }

    #[test]
    fn test_balance_eq() {
        let a = Balance::from_spores(100);
        let b = Balance::from_spores(100);
        assert_eq!(a, b);
    }

    #[test]
    fn test_balance_copy() {
        let a = Balance::from_spores(42);
        let b = a;
        assert_eq!(a.spores(), b.spores());
    }

    #[test]
    fn test_balance_debug() {
        let b = Balance::from_spores(0);
        let s = format!("{:?}", b);
        assert!(s.contains("Balance"));
    }

    #[test]
    fn test_balance_licn_precision() {
        // 1 LICN exactly
        let b = Balance::from_licn("1.0").unwrap();
        assert_eq!(b.spores(), 1_000_000_000);
        assert!((b.licn() - 1.0).abs() < 1e-15);
    }
}
