use super::*;

impl TxProcessor {
    pub(super) fn get_governed_governance_authority(
        &self,
    ) -> Result<Option<(Pubkey, crate::multisig::GovernedWalletConfig)>, String> {
        let Some(authority) = self.state.get_governance_authority()? else {
            return Ok(None);
        };
        let Some(config) = self.state.get_governed_wallet_config(&authority)? else {
            return Ok(None);
        };
        Ok(Some((authority, config)))
    }

    pub(super) fn get_governed_treasury_executor_authority(
        &self,
    ) -> Result<Option<(Pubkey, crate::multisig::GovernedWalletConfig)>, String> {
        if let Some(authority) = self.state.get_treasury_executor_authority()? {
            if let Some(config) = self.state.get_governed_wallet_config(&authority)? {
                return Ok(Some((authority, config)));
            }
        }

        let Some(governance_authority) = self.state.get_governance_authority()? else {
            return Ok(None);
        };
        let authority = crate::multisig::derive_treasury_executor_authority(&governance_authority);
        let Some(config) = self.state.get_governed_wallet_config(&authority)? else {
            return Ok(None);
        };
        Ok(Some((authority, config)))
    }

    pub(super) fn get_governed_incident_guardian_authority(
        &self,
    ) -> Result<Option<(Pubkey, crate::multisig::GovernedWalletConfig)>, String> {
        let Some(authority) = self.state.get_incident_guardian_authority()? else {
            return Ok(None);
        };
        let Some(config) = self.state.get_governed_wallet_config(&authority)? else {
            return Ok(None);
        };
        Ok(Some((authority, config)))
    }

    pub(super) fn get_governed_bridge_committee_admin_authority(
        &self,
    ) -> Result<Option<(Pubkey, crate::multisig::GovernedWalletConfig)>, String> {
        if let Some(authority) = self.state.get_bridge_committee_admin_authority()? {
            if let Some(config) = self.state.get_governed_wallet_config(&authority)? {
                return Ok(Some((authority, config)));
            }
        }

        let Some(governance_authority) = self.state.get_governance_authority()? else {
            return Ok(None);
        };
        let authority =
            crate::multisig::derive_bridge_committee_admin_authority(&governance_authority);
        let Some(config) = self.state.get_governed_wallet_config(&authority)? else {
            return Ok(None);
        };
        Ok(Some((authority, config)))
    }

    pub(super) fn get_governed_oracle_committee_admin_authority(
        &self,
    ) -> Result<Option<(Pubkey, crate::multisig::GovernedWalletConfig)>, String> {
        if let Some(authority) = self.state.get_oracle_committee_admin_authority()? {
            if let Some(config) = self.state.get_governed_wallet_config(&authority)? {
                return Ok(Some((authority, config)));
            }
        }

        let Some(governance_authority) = self.state.get_governance_authority()? else {
            return Ok(None);
        };
        let authority =
            crate::multisig::derive_oracle_committee_admin_authority(&governance_authority);
        let Some(config) = self.state.get_governed_wallet_config(&authority)? else {
            return Ok(None);
        };
        Ok(Some((authority, config)))
    }

    pub(super) fn get_governed_upgrade_proposer_authority(
        &self,
    ) -> Result<Option<(Pubkey, crate::multisig::GovernedWalletConfig)>, String> {
        if let Some(authority) = self.state.get_upgrade_proposer_authority()? {
            if let Some(config) = self.state.get_governed_wallet_config(&authority)? {
                return Ok(Some((authority, config)));
            }
        }

        let Some(governance_authority) = self.state.get_governance_authority()? else {
            return Ok(None);
        };
        let authority = crate::multisig::derive_upgrade_proposer_authority(&governance_authority);
        let Some(config) = self.state.get_governed_wallet_config(&authority)? else {
            return Ok(None);
        };
        Ok(Some((authority, config)))
    }

    pub(super) fn get_governed_upgrade_veto_guardian_authority(
        &self,
    ) -> Result<Option<(Pubkey, crate::multisig::GovernedWalletConfig)>, String> {
        if let Some(authority) = self.state.get_upgrade_veto_guardian_authority()? {
            if let Some(config) = self.state.get_governed_wallet_config(&authority)? {
                return Ok(Some((authority, config)));
            }
        }

        let Some(governance_authority) = self.state.get_governance_authority()? else {
            return Ok(None);
        };
        let authority =
            crate::multisig::derive_upgrade_veto_guardian_authority(&governance_authority);
        let Some(config) = self.state.get_governed_wallet_config(&authority)? else {
            return Ok(None);
        };
        Ok(Some((authority, config)))
    }

    pub(super) fn get_governance_proposal_approval_authority(
        &self,
        proposal: &GovernanceProposal,
    ) -> Result<(Pubkey, crate::multisig::GovernedWalletConfig), String> {
        let approval_authority = proposal.approval_authority();
        let Some(config) = self.state.get_governed_wallet_config(&approval_authority)? else {
            return Err(format!(
                "Governance proposal approval authority {} is not configured as a governed wallet",
                approval_authority.to_base58()
            ));
        };
        Ok((approval_authority, config))
    }

    pub(super) fn resolve_governance_proposal_authority(
        &self,
        requested_authority: &Pubkey,
        action: &GovernanceAction,
    ) -> Result<
        (
            Pubkey,
            Option<Pubkey>,
            crate::multisig::GovernedWalletConfig,
        ),
        String,
    > {
        let (governance_authority, governance_config) =
            self.get_governed_governance_authority()?.ok_or_else(|| {
                "Governance authority is not configured as a governed wallet".to_string()
            })?;

        if self.governance_action_requires_treasury_executor_policy(action)? {
            let (treasury_authority, treasury_config) = self
                .get_governed_treasury_executor_authority()?
                .ok_or_else(|| {
                    "Treasury executor authority is not configured as a governed wallet".to_string()
                })?;
            if *requested_authority != treasury_authority {
                return Err(
                    "Protocol fund movement governance actions must use the treasury executor approval authority"
                        .to_string(),
                );
            }
            return Ok((
                governance_authority,
                Some(treasury_authority),
                treasury_config,
            ));
        }

        if self.governance_action_requires_upgrade_proposer_policy(action) {
            let (upgrade_authority, upgrade_config) = self
                .get_governed_upgrade_proposer_authority()?
                .ok_or_else(|| {
                    "Upgrade proposer authority is not configured as a governed wallet".to_string()
                })?;
            if *requested_authority != upgrade_authority {
                return Err(
                    "Upgrade governance actions must use the upgrade proposer approval authority"
                        .to_string(),
                );
            }
            return Ok((
                governance_authority,
                Some(upgrade_authority),
                upgrade_config,
            ));
        }

        if self.governance_action_requires_upgrade_veto_guardian_policy(action) {
            let (veto_authority, veto_config) = self
                .get_governed_upgrade_veto_guardian_authority()?
                .ok_or_else(|| {
                    "Upgrade veto guardian authority is not configured as a governed wallet"
                        .to_string()
                })?;
            if *requested_authority != veto_authority {
                return Err(
                    "Upgrade veto governance actions must use the upgrade veto guardian approval authority"
                        .to_string(),
                );
            }
            return Ok((governance_authority, Some(veto_authority), veto_config));
        }

        if self.governance_action_requires_bridge_committee_admin_policy(action)? {
            let (bridge_authority, bridge_config) = self
                .get_governed_bridge_committee_admin_authority()?
                .ok_or_else(|| {
                    "Bridge committee admin authority is not configured as a governed wallet"
                        .to_string()
                })?;
            if *requested_authority != bridge_authority {
                return Err(
                    "Bridge governance actions must use the bridge committee admin approval authority"
                        .to_string(),
                );
            }
            return Ok((governance_authority, Some(bridge_authority), bridge_config));
        }

        if self.governance_action_requires_oracle_committee_admin_policy(action)? {
            let (oracle_authority, oracle_config) = self
                .get_governed_oracle_committee_admin_authority()?
                .ok_or_else(|| {
                    "Oracle committee admin authority is not configured as a governed wallet"
                        .to_string()
                })?;
            if *requested_authority != oracle_authority {
                return Err(
                    "Oracle governance actions must use the oracle committee admin approval authority"
                        .to_string(),
                );
            }
            return Ok((governance_authority, Some(oracle_authority), oracle_config));
        }

        if *requested_authority == governance_authority {
            return Ok((governance_authority, None, governance_config));
        }

        if self.governance_action_uses_immediate_risk_reduction_policy(action)? {
            if let Some((guardian_authority, guardian_config)) =
                self.get_governed_incident_guardian_authority()?
            {
                if *requested_authority == guardian_authority {
                    return Ok((
                        governance_authority,
                        Some(guardian_authority),
                        guardian_config,
                    ));
                }
            }
        }

        if let Some((guardian_authority, _)) = self.get_governed_incident_guardian_authority()? {
            if *requested_authority == guardian_authority {
                return Err(
                    "Incident guardian authority may only submit allowlisted immediate risk-reduction proposals"
                        .to_string(),
                );
            }
        }

        Err("Governance action authority account mismatch".to_string())
    }
}
