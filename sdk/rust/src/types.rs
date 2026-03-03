//! Common types used in the SDK

use serde::{Deserialize, Serialize};

/// Account balance
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Balance {
    shells: u64,
}

impl Balance {
    /// Create from shells
    pub fn from_shells(shells: u64) -> Self {
        Self { shells }
    }
    
    /// Create from MOLT (handles negative, NaN, and overflow gracefully)
    /// J-3: Uses rounding instead of truncation to avoid systematic 1-shell loss
    pub fn from_molt(molt: f64) -> Self {
        if molt.is_nan() || molt < 0.0 {
            return Self { shells: 0 };
        }
        let shells = (molt * 1_000_000_000.0).round();
        Self {
            shells: shells.min(u64::MAX as f64) as u64,
        }
    }
    
    /// Get shells
    pub fn shells(&self) -> u64 {
        self.shells
    }
    
    /// Get MOLT
    pub fn molt(&self) -> f64 {
        self.shells as f64 / 1_000_000_000.0
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
pub use moltchain_core::Transaction;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_balance_roundtrip() {
        let b = Balance::from_shells(1_500_000_000);
        assert_eq!(b.shells(), 1_500_000_000);
        assert!((b.molt() - 1.5).abs() < 1e-9);
    }

    #[test]
    fn test_balance_from_molt_normal() {
        let b = Balance::from_molt(2.5);
        assert_eq!(b.shells(), 2_500_000_000);
    }

    #[test]
    fn test_balance_from_molt_negative() {
        let b = Balance::from_molt(-1.0);
        assert_eq!(b.shells(), 0);
    }

    #[test]
    fn test_balance_from_molt_nan() {
        let b = Balance::from_molt(f64::NAN);
        assert_eq!(b.shells(), 0);
    }

    #[test]
    fn test_balance_from_molt_infinity() {
        let b = Balance::from_molt(f64::INFINITY);
        assert_eq!(b.shells(), u64::MAX);
    }

    #[test]
    fn test_balance_from_molt_zero() {
        let b = Balance::from_molt(0.0);
        assert_eq!(b.shells(), 0);
    }

    #[test]
    fn test_balance_neg_infinity() {
        let b = Balance::from_molt(f64::NEG_INFINITY);
        assert_eq!(b.shells(), 0);
    }

    #[test]
    fn test_balance_from_molt_tiny_fraction() {
        // 0.000000001 MOLT = 1 shell (rounding)
        let b = Balance::from_molt(0.000_000_001);
        assert_eq!(b.shells(), 1);
    }

    #[test]
    fn test_balance_from_molt_sub_shell() {
        // 0.0000000001 MOLT < 1 shell → rounds to 0
        let b = Balance::from_molt(0.000_000_000_1);
        assert_eq!(b.shells(), 0);
    }

    #[test]
    fn test_balance_from_shells_max() {
        let b = Balance::from_shells(u64::MAX);
        assert_eq!(b.shells(), u64::MAX);
    }

    #[test]
    fn test_balance_eq() {
        let a = Balance::from_shells(100);
        let b = Balance::from_shells(100);
        assert_eq!(a, b);
    }

    #[test]
    fn test_balance_copy() {
        let a = Balance::from_shells(42);
        let b = a;
        assert_eq!(a.shells(), b.shells());
    }

    #[test]
    fn test_balance_debug() {
        let b = Balance::from_shells(0);
        let s = format!("{:?}", b);
        assert!(s.contains("Balance"));
    }

    #[test]
    fn test_balance_molt_precision() {
        // 1 MOLT exactly
        let b = Balance::from_molt(1.0);
        assert_eq!(b.shells(), 1_000_000_000);
        assert!((b.molt() - 1.0).abs() < 1e-15);
    }
}
