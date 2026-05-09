use super::*;

impl TxProcessor {
    /// System instruction type 29: GovernanceParamChange
    ///
    /// Queues a consensus parameter change to take effect at the next epoch
    /// boundary. Only the governance authority may submit these instructions.
    pub(super) fn system_governance_param_change(&self, ix: &Instruction) -> Result<(), String> {
        if ix.data.len() < 10 {
            return Err(
                "GovernanceParamChange: data too short (need opcode + param_id + u64)".to_string(),
            );
        }

        let param_id = ix.data[1];
        let value = u64::from_le_bytes(
            ix.data[2..10]
                .try_into()
                .map_err(|_| "GovernanceParamChange: invalid value bytes".to_string())?,
        );

        if ix.accounts.is_empty() {
            return Err("GovernanceParamChange: requires governance authority account".to_string());
        }
        let signer = ix.accounts[0];

        let authority = self
            .state
            .get_governance_authority()?
            .ok_or("GovernanceParamChange: no governance authority configured")?;

        if self.state.get_governed_wallet_config(&authority)?.is_some() {
            return Err(
                "GovernanceParamChange: governed governance authority must use governance action proposal flow (use types 34-37)".to_string(),
            );
        }

        if signer != authority {
            return Err(
                "GovernanceParamChange: signer is not the governance authority".to_string(),
            );
        }

        self.validate_governance_param_change_value(param_id, value)?;
        self.b_queue_governance_param_change(param_id, value)?;

        Ok(())
    }

    /// System instruction type 30: Oracle multi-source price attestation.
    ///
    /// Validators submit price attestations for named assets. Once a 2/3+
    /// active-stake quorum attests within the staleness window, the processor
    /// stores the stake-weighted median as the consensus oracle price.
    pub(super) fn system_oracle_attestation(&self, ix: &Instruction) -> Result<(), String> {
        if ix.data.len() < 4 {
            return Err(
                "OracleAttestation: data too short (need opcode + asset_len + asset + price + decimals)"
                    .to_string(),
            );
        }
        let asset_len = ix.data[1] as usize;
        if !(ORACLE_ASSET_MIN_LEN..=ORACLE_ASSET_MAX_LEN).contains(&asset_len) {
            return Err(format!(
                "OracleAttestation: asset name length {} out of range {}..={}",
                asset_len, ORACLE_ASSET_MIN_LEN, ORACLE_ASSET_MAX_LEN
            ));
        }
        let expected_len = 2 + asset_len + 9;
        if ix.data.len() < expected_len {
            return Err(format!(
                "OracleAttestation: data too short (need {} bytes, got {})",
                expected_len,
                ix.data.len()
            ));
        }
        let asset = std::str::from_utf8(&ix.data[2..2 + asset_len])
            .map_err(|_| "OracleAttestation: asset name is not valid UTF-8".to_string())?;
        let price_offset = 2 + asset_len;
        let price = u64::from_le_bytes(
            ix.data[price_offset..price_offset + 8]
                .try_into()
                .map_err(|_| "OracleAttestation: invalid price bytes".to_string())?,
        );
        let decimals = ix.data[price_offset + 8];

        if price == 0 {
            return Err("OracleAttestation: price must be > 0".to_string());
        }
        if decimals > 18 {
            return Err("OracleAttestation: decimals must be 0..=18".to_string());
        }

        if ix.accounts.is_empty() {
            return Err("OracleAttestation: requires validator account".to_string());
        }
        let signer = ix.accounts[0];

        let pool = self.b_get_stake_pool()?;
        let stake_info = pool
            .get_stake(&signer)
            .ok_or_else(|| "OracleAttestation: signer has no stake".to_string())?;
        if !stake_info.is_active || !stake_info.meets_minimum() {
            return Err("OracleAttestation: signer is not an active validator".to_string());
        }
        let signer_stake = stake_info.total_stake();

        let current_slot = self.b_get_last_slot().unwrap_or(0);
        if self.is_speculative() {
            // Oracle attestation records and derived consensus prices are not
            // part of the state-root commitment. Canonical replay persists
            // them exactly once at commit; proposal execution only needs to
            // validate that the attestation transaction is includable.
            return Ok(());
        }

        self.state.put_oracle_attestation(
            asset,
            &signer,
            price,
            decimals,
            signer_stake,
            current_slot,
        )?;

        let attestations =
            self.state
                .get_oracle_attestations(asset, current_slot, ORACLE_STALENESS_SLOTS)?;

        let total_active_stake = pool.active_stake();
        if total_active_stake == 0 {
            return Ok(());
        }

        let attested_stake: u128 = attestations.iter().map(|a| a.stake as u128).sum();
        let threshold = (total_active_stake as u128) * 2 / 3;
        let active_validators = pool.active_validators().len();
        let min_attestors = if active_validators <= 1 { 1 } else { 2 };
        if attested_stake >= threshold && attestations.len() >= min_attestors {
            let consensus_price = compute_stake_weighted_median(&attestations);
            self.state.put_oracle_consensus_price(
                asset,
                consensus_price,
                decimals,
                current_slot,
                attestations.len() as u32,
            )?;
        }

        Ok(())
    }
}
