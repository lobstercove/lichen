//! Common types used in the SDK

use serde::{Deserialize, Serialize};

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
    
    /// Create from LICN (handles negative, NaN, and overflow gracefully)
    /// J-3: Uses rounding instead of truncation to avoid systematic 1-spore loss
    pub fn from_licn(licn: f64) -> Self {
        if licn.is_nan() || licn < 0.0 {
            return Self { spores: 0 };
        }
        let spores = (licn * 1_000_000_000.0).round();
        Self {
            spores: spores.min(u64::MAX as f64) as u64,
        }
    }
    
    /// Get spores
    pub fn spores(&self) -> u64 {
        self.spores
    }
    
    /// Get LICN
    pub fn licn(&self) -> f64 {
        self.spores as f64 / 1_000_000_000.0
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
        let b = Balance::from_licn(2.5);
        assert_eq!(b.spores(), 2_500_000_000);
    }

    #[test]
    fn test_balance_from_licn_negative() {
        let b = Balance::from_licn(-1.0);
        assert_eq!(b.spores(), 0);
    }

    #[test]
    fn test_balance_from_licn_nan() {
        let b = Balance::from_licn(f64::NAN);
        assert_eq!(b.spores(), 0);
    }

    #[test]
    fn test_balance_from_licn_infinity() {
        let b = Balance::from_licn(f64::INFINITY);
        assert_eq!(b.spores(), u64::MAX);
    }

    #[test]
    fn test_balance_from_licn_zero() {
        let b = Balance::from_licn(0.0);
        assert_eq!(b.spores(), 0);
    }

    #[test]
    fn test_balance_neg_infinity() {
        let b = Balance::from_licn(f64::NEG_INFINITY);
        assert_eq!(b.spores(), 0);
    }

    #[test]
    fn test_balance_from_licn_tiny_fraction() {
        // 0.000000001 LICN = 1 spore (rounding)
        let b = Balance::from_licn(0.000_000_001);
        assert_eq!(b.spores(), 1);
    }

    #[test]
    fn test_balance_from_licn_sub_spore() {
        // 0.0000000001 LICN < 1 spore → rounds to 0
        let b = Balance::from_licn(0.000_000_000_1);
        assert_eq!(b.spores(), 0);
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
        let b = Balance::from_licn(1.0);
        assert_eq!(b.spores(), 1_000_000_000);
        assert!((b.licn() - 1.0).abs() < 1e-15);
    }
}
