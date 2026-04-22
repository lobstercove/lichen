use super::*;

mod incident;
mod policy;

pub(super) fn default_signer_threshold(endpoint_count: usize) -> usize {
    policy::default_signer_threshold(endpoint_count)
}

pub(super) fn ensure_deposit_creation_allowed(config: &CustodyConfig) -> Result<(), String> {
    incident::ensure_deposit_creation_allowed(config)
}

pub(super) fn local_rebalance_policy_error(config: &CustodyConfig) -> Option<String> {
    incident::local_rebalance_policy_error(config)
}

pub(super) fn local_sweep_policy_error(config: &CustodyConfig) -> Option<String> {
    incident::local_sweep_policy_error(config)
}

pub(super) fn validate_custody_security_configuration(
    config: &CustodyConfig,
) -> Result<(), String> {
    policy::validate_custody_security_configuration(config)
}

pub(super) fn validate_custody_security_configuration_with_mode(
    config: &CustodyConfig,
    allow_insecure_seed_mode: bool,
) -> Result<(), String> {
    policy::validate_custody_security_configuration_with_mode(config, allow_insecure_seed_mode)
}

pub(super) fn validate_pq_signer_configuration(config: &CustodyConfig) -> Result<(), String> {
    policy::validate_pq_signer_configuration(config)
}

pub(super) fn withdrawal_incident_block_reason(config: &CustodyConfig) -> Option<&'static str> {
    incident::withdrawal_incident_block_reason(config)
}
