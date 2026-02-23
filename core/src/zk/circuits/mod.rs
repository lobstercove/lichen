//! ZK Circuit Definitions (R1CS)
//!
//! Three circuits for the shielded pool:
//! - Shield: transparent -> shielded (deposit)
//! - Unshield: shielded -> transparent (withdraw)
//! - Transfer: shielded -> shielded (private send)

pub mod shield;
pub mod transfer;
pub mod unshield;
