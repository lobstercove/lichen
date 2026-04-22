use super::*;

mod evm;
mod solana;

use self::evm::broadcast_evm_sweep;
use self::solana::broadcast_solana_sweep;

pub(super) async fn broadcast_sweep(
    state: &CustodyState,
    job: &SweepJob,
) -> Result<Option<String>, String> {
    if let Some(err) = local_sweep_policy_error(&state.config) {
        return Err(format!(
            "{}; refusing to broadcast sweep {} on {}",
            err, job.job_id, job.chain
        ));
    }

    if job.chain == "sol" || job.chain == "solana" {
        let url = state
            .config
            .solana_rpc_url
            .as_ref()
            .ok_or_else(|| "missing CUSTODY_SOLANA_RPC_URL".to_string())?;
        return broadcast_solana_sweep(state, url, job).await;
    }

    if is_evm_chain(&job.chain) {
        let url = rpc_url_for_chain(&state.config, &job.chain)
            .ok_or_else(|| format!("missing RPC URL for chain {}", job.chain))?;
        return broadcast_evm_sweep(state, &url, job).await;
    }

    Ok(None)
}
