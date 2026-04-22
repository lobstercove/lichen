use super::*;
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
struct IncidentGuardianPauseTarget {
    symbol: String,
    pause_function: String,
}

static INCIDENT_GUARDIAN_PAUSE_TARGETS: OnceLock<Vec<IncidentGuardianPauseTarget>> =
    OnceLock::new();

fn incident_guardian_pause_targets() -> &'static [IncidentGuardianPauseTarget] {
    INCIDENT_GUARDIAN_PAUSE_TARGETS
        .get_or_init(|| {
            serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/resources/incident-guardian-pause-allowlist.json"
            )))
            .expect("incident guardian pause allowlist must be valid JSON")
        })
        .as_slice()
}

fn is_allowlisted_incident_guardian_pause(symbol: &str, function: &str) -> bool {
    incident_guardian_pause_targets()
        .iter()
        .any(|target| target.symbol == symbol && target.pause_function == function)
}

fn is_bridge_committee_admin_contract_call(symbol: &str, function: &str) -> bool {
    (symbol.eq_ignore_ascii_case("BRIDGE") || symbol.eq_ignore_ascii_case("LICHENBRIDGE"))
        && matches!(
            function,
            "add_bridge_validator"
                | "remove_bridge_validator"
                | "set_required_confirmations"
                | "set_request_timeout"
        )
}

fn is_oracle_committee_admin_contract_call(symbol: &str, function: &str, args: &[u8]) -> bool {
    if ((symbol.eq_ignore_ascii_case("ORACLE") || symbol.eq_ignore_ascii_case("LICHENORACLE"))
        && matches!(function, "add_price_feeder" | "set_authorized_attester"))
        || (matches!(symbol, "LUSD" | "WSOL" | "WETH" | "WBNB") && function == "set_attester")
    {
        return true;
    }

    if symbol.eq_ignore_ascii_case("DEXMARGIN") || symbol.eq_ignore_ascii_case("MARGIN") {
        return matches!(function, "set_mark_price" | "set_index_price")
            || (function == "call" && matches!(args.first().copied(), Some(1) | Some(31)));
    }

    false
}

fn is_immediate_oracle_committee_contract_call(symbol: &str, function: &str, args: &[u8]) -> bool {
    (symbol.eq_ignore_ascii_case("DEXMARGIN") || symbol.eq_ignore_ascii_case("MARGIN"))
        && (matches!(function, "set_mark_price" | "set_index_price")
            || (function == "call" && matches!(args.first().copied(), Some(1) | Some(31))))
}

fn is_treasury_executor_contract_call(symbol: &str, function: &str, args: &[u8]) -> bool {
    if symbol.eq_ignore_ascii_case("DEXMARGIN") {
        return function == "call" && args.first() == Some(&9u8);
    }

    if symbol.eq_ignore_ascii_case("DEXAMM") || symbol.eq_ignore_ascii_case("AMM") {
        return function == "call" && args.first() == Some(&21u8);
    }

    ((symbol.eq_ignore_ascii_case("LEND") || symbol.eq_ignore_ascii_case("THALLLEND"))
        && function == "withdraw_reserves")
        || ((symbol.eq_ignore_ascii_case("SPOREVAULT") || symbol.eq_ignore_ascii_case("VAULT"))
            && function == "withdraw_protocol_fees")
        || (symbol.eq_ignore_ascii_case("SPOREPUMP") && function == "withdraw_fees")
        || (matches!(symbol, "LUSD" | "WSOL" | "WETH" | "WBNB") && function == "set_minter")
}

impl TxProcessor {
    pub(super) fn governance_action_uses_immediate_risk_reduction_policy(
        &self,
        action: &GovernanceAction,
    ) -> Result<bool, String> {
        let GovernanceAction::ContractCall {
            contract,
            function,
            args,
            ..
        } = action
        else {
            return Ok(false);
        };

        let Some(entry) = self.state.get_symbol_registry_by_program(contract)? else {
            return Ok(false);
        };

        Ok(
            is_allowlisted_incident_guardian_pause(entry.symbol.as_str(), function.as_str())
                || is_immediate_oracle_committee_contract_call(
                    entry.symbol.as_str(),
                    function.as_str(),
                    args.as_slice(),
                ),
        )
    }

    pub(super) fn governance_action_requires_treasury_executor_policy(
        &self,
        action: &GovernanceAction,
    ) -> Result<bool, String> {
        if matches!(action, GovernanceAction::TreasuryTransfer { .. }) {
            return Ok(true);
        }

        let GovernanceAction::ContractCall {
            contract,
            function,
            args,
            ..
        } = action
        else {
            return Ok(false);
        };

        let Some(entry) = self.state.get_symbol_registry_by_program(contract)? else {
            return Ok(false);
        };

        Ok(is_treasury_executor_contract_call(
            entry.symbol.as_str(),
            function.as_str(),
            args.as_slice(),
        ))
    }

    pub(super) fn governance_action_requires_upgrade_proposer_policy(
        &self,
        action: &GovernanceAction,
    ) -> bool {
        matches!(
            action,
            GovernanceAction::ContractUpgrade { .. }
                | GovernanceAction::SetContractUpgradeTimelock { .. }
                | GovernanceAction::ExecuteContractUpgrade { .. }
        )
    }

    pub(super) fn governance_action_requires_upgrade_veto_guardian_policy(
        &self,
        action: &GovernanceAction,
    ) -> bool {
        matches!(action, GovernanceAction::VetoContractUpgrade { .. })
    }

    pub(super) fn governance_action_requires_bridge_committee_admin_policy(
        &self,
        action: &GovernanceAction,
    ) -> Result<bool, String> {
        let GovernanceAction::ContractCall {
            contract, function, ..
        } = action
        else {
            return Ok(false);
        };

        let Some(entry) = self.state.get_symbol_registry_by_program(contract)? else {
            return Ok(false);
        };

        Ok(is_bridge_committee_admin_contract_call(
            entry.symbol.as_str(),
            function.as_str(),
        ))
    }

    pub(super) fn governance_action_requires_oracle_committee_admin_policy(
        &self,
        action: &GovernanceAction,
    ) -> Result<bool, String> {
        let GovernanceAction::ContractCall {
            contract,
            function,
            args,
            ..
        } = action
        else {
            return Ok(false);
        };

        let Some(entry) = self.state.get_symbol_registry_by_program(contract)? else {
            return Ok(false);
        };

        Ok(is_oracle_committee_admin_contract_call(
            entry.symbol.as_str(),
            function.as_str(),
            args.as_slice(),
        ))
    }
}
