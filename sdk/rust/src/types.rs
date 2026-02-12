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
    
    /// Create from MOLT
    pub fn from_molt(molt: f64) -> Self {
        Self {
            shells: (molt * 1_000_000_000.0) as u64,
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
