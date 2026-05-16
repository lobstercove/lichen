//! Internal shielded witness-adapter circuits.
//!
//! Three circuits for the shielded pool:
//! - Shield: transparent -> shielded (deposit)
//! - Unshield: shielded -> transparent (withdraw)
//! - Transfer: shielded -> shielded (private send)
//! - Reserve/liability: public aggregate proof-service statement

pub mod reserve_liability;
pub mod shield;
pub mod transfer;
pub mod unshield;
pub mod utils;
