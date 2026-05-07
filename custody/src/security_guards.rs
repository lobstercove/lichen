use super::*;

mod incident;
mod policy;
mod restrictions;

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

pub(super) async fn ensure_deposit_restrictions_allow(
    state: &CustodyState,
    user_id: &str,
    chain: &str,
    asset: &str,
) -> Result<(), String> {
    restrictions::ensure_deposit_restrictions_allow(state, user_id, chain, asset).await
}

pub(super) async fn ensure_credit_restrictions_allow(
    state: &CustodyState,
    user_id: &str,
    chain: &str,
    asset: &str,
    amount_spores: u64,
) -> Result<(), String> {
    restrictions::ensure_credit_restrictions_allow(state, user_id, chain, asset, amount_spores)
        .await
}

pub(super) async fn ensure_withdrawal_restrictions_allow(
    state: &CustodyState,
    user_id: &str,
    asset: &str,
    amount_spores: u64,
    dest_chain: &str,
    preferred_stablecoin: &str,
) -> Result<(), String> {
    restrictions::ensure_withdrawal_restrictions_allow(
        state,
        user_id,
        asset,
        amount_spores,
        dest_chain,
        preferred_stablecoin,
    )
    .await
}
