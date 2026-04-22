use super::super::*;
use super::policy::multi_signer_local_sweep_mode;

#[derive(Debug, Clone, Deserialize, Default)]
struct IncidentComponentStatus {
    #[serde(default)]
    status: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct IncidentStatusRecord {
    #[serde(default)]
    mode: String,
    #[serde(default)]
    components: BTreeMap<String, IncidentComponentStatus>,
}

fn load_incident_status(config: &CustodyConfig) -> Option<IncidentStatusRecord> {
    let path = config
        .incident_status_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            warn!(
                "failed to read custody incident manifest {}: {}",
                path, error
            );
            return None;
        }
    };

    match serde_json::from_str::<IncidentStatusRecord>(&raw) {
        Ok(status) => Some(status),
        Err(error) => {
            warn!(
                "failed to parse custody incident manifest {}: {}",
                path, error
            );
            None
        }
    }
}

fn incident_mode_matches(status: &IncidentStatusRecord, modes: &[&str]) -> bool {
    modes
        .iter()
        .any(|mode| status.mode.eq_ignore_ascii_case(mode))
}

fn incident_component_is_blocked(status: &IncidentStatusRecord, component: &str) -> bool {
    status
        .components
        .get(component)
        .map(|component_status| {
            matches!(
                component_status.status.trim().to_ascii_lowercase().as_str(),
                "paused" | "blocked" | "disabled" | "frozen"
            )
        })
        .unwrap_or(false)
}

fn deposit_incident_block_reason(config: &CustodyConfig) -> Option<&'static str> {
    let status = load_incident_status(config)?;
    if incident_component_is_blocked(&status, "bridge")
        || incident_mode_matches(&status, &["bridge_pause"])
    {
        return Some("bridge deposits are temporarily paused while bridge risk is assessed");
    }
    if incident_component_is_blocked(&status, "deposits")
        || incident_mode_matches(&status, &["deposit_guard", "deposit_only_freeze"])
    {
        return Some("new deposits are temporarily paused while operators verify inbound activity");
    }
    None
}

pub(super) fn withdrawal_incident_block_reason(config: &CustodyConfig) -> Option<&'static str> {
    let status = load_incident_status(config)?;
    if incident_component_is_blocked(&status, "bridge")
        || incident_mode_matches(&status, &["bridge_pause"])
    {
        return Some("bridge redemptions are temporarily paused while bridge risk is assessed");
    }
    None
}

pub(super) fn ensure_deposit_creation_allowed(config: &CustodyConfig) -> Result<(), String> {
    if let Some(err) = local_sweep_policy_error(config) {
        return Err(err);
    }

    if let Some(err) = deposit_incident_block_reason(config) {
        return Err(err.to_string());
    }

    Ok(())
}

pub(super) fn local_sweep_policy_error(config: &CustodyConfig) -> Option<String> {
    if multi_signer_local_sweep_mode(config) {
        return Some(
            "multi-signer deposit creation is disabled because deposit sweeps still broadcast with locally derived deposit keys; this path remains hard-disabled until deposit sweeps have a real threshold architecture".to_string(),
        );
    }

    None
}

pub(super) fn local_rebalance_policy_error(config: &CustodyConfig) -> Option<String> {
    if multi_signer_local_sweep_mode(config) {
        return Some(
            "multi-signer reserve rebalance is disabled because rebalances still broadcast with locally derived treasury keys; this path remains hard-disabled until rebalances have a real threshold executor".to_string(),
        );
    }

    None
}
