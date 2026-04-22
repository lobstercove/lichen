use super::super::*;

fn insecure_seed_mode_enabled() -> bool {
    std::env::var("CUSTODY_ALLOW_INSECURE_SEED").unwrap_or_default() == "1"
}

fn strict_majority_threshold(endpoint_count: usize) -> usize {
    if endpoint_count == 0 {
        0
    } else {
        (endpoint_count / 2) + 1
    }
}

pub(super) fn default_signer_threshold(endpoint_count: usize) -> usize {
    strict_majority_threshold(endpoint_count)
}

fn validate_multi_signer_policy(config: &CustodyConfig) -> Result<(), String> {
    let endpoint_count = config.signer_endpoints.len();
    if config.signer_threshold > endpoint_count {
        return Err(format!(
            "signer_threshold={} exceeds configured signer count={}. Threshold must be <= number of signer endpoints.",
            config.signer_threshold, endpoint_count
        ));
    }

    if endpoint_count > 1 {
        let required_threshold = strict_majority_threshold(endpoint_count);
        if config.signer_threshold < required_threshold {
            return Err(format!(
                "multi-signer custody requires a strict-majority threshold; signer_threshold={} but {} signer endpoint(s) require at least {}",
                config.signer_threshold, endpoint_count, required_threshold
            ));
        }
    }

    Ok(())
}

pub(super) fn validate_custody_security_configuration(
    config: &CustodyConfig,
) -> Result<(), String> {
    super::validate_custody_security_configuration_with_mode(config, insecure_seed_mode_enabled())
}

pub(super) fn validate_custody_security_configuration_with_mode(
    config: &CustodyConfig,
    allow_insecure_seed_mode: bool,
) -> Result<(), String> {
    if !allow_insecure_seed_mode && config.deposit_master_seed == config.master_seed {
        return Err(
            "CUSTODY_DEPOSIT_MASTER_SEED must be set and must differ from CUSTODY_MASTER_SEED outside explicit dev mode (CUSTODY_ALLOW_INSECURE_SEED=1)".to_string(),
        );
    }

    validate_multi_signer_policy(config)
}

pub(super) fn validate_pq_signer_configuration(config: &CustodyConfig) -> Result<(), String> {
    validate_multi_signer_policy(config)?;

    if config.signer_endpoints.is_empty() || config.signer_threshold == 0 {
        return Ok(());
    }

    if config.signer_pq_addresses.len() != config.signer_endpoints.len() {
        return Err(format!(
            "configured {} signer endpoint(s) but {} PQ signer address(es); set CUSTODY_SIGNER_PQ_ADDRESSES to match signer endpoints one-for-one",
            config.signer_endpoints.len(),
            config.signer_pq_addresses.len()
        ));
    }

    if config.signer_threshold > config.signer_pq_addresses.len() {
        return Err(format!(
            "signer_threshold={} exceeds configured PQ signer count={}",
            config.signer_threshold,
            config.signer_pq_addresses.len()
        ));
    }

    Ok(())
}

pub(super) fn multi_signer_local_sweep_mode(config: &CustodyConfig) -> bool {
    config.signer_endpoints.len() > 1
}
