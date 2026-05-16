//! Reserve/liability proof statement for Neo proof-service attestations.
//!
//! This circuit adapter is intentionally transparent in v1: aggregate reserve
//! and liability totals are public inputs, while source records stay outside
//! the proof behind the witness commitment.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveLiabilityCircuit {
    pub domain_hash: [u8; 32],
    pub statement_hash: [u8; 32],
    pub witness_commitment: [u8; 32],
    pub reserve_amount: u64,
    pub liability_amount: u64,
    pub epoch: u64,
    pub verifier_version: u64,
}

impl ReserveLiabilityCircuit {
    pub fn new(
        domain_hash: [u8; 32],
        statement_hash: [u8; 32],
        witness_commitment: [u8; 32],
        reserve_amount: u64,
        liability_amount: u64,
        epoch: u64,
        verifier_version: u64,
    ) -> Self {
        Self {
            domain_hash,
            statement_hash,
            witness_commitment,
            reserve_amount,
            liability_amount,
            epoch,
            verifier_version,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.domain_hash == [0u8; 32] {
            return Err("reserve/liability domain hash must be non-zero".to_string());
        }
        if self.statement_hash == [0u8; 32] {
            return Err("reserve/liability statement hash must be non-zero".to_string());
        }
        if self.witness_commitment == [0u8; 32] {
            return Err("reserve/liability witness commitment must be non-zero".to_string());
        }
        if self.verifier_version == 0 {
            return Err("reserve/liability verifier version must be positive".to_string());
        }
        if self.reserve_amount < self.liability_amount {
            return Err("reserve/liability statement is undercollateralized".to_string());
        }
        Ok(())
    }
}
