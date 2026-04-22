use super::*;

pub(super) async fn check_sweep_confirmation(
    state: &CustodyState,
    job: &SweepJob,
) -> Result<Option<bool>, String> {
    let Some(tx_hash) = job.sweep_tx_hash.as_ref() else {
        return Ok(None);
    };

    if job.chain == "sol" || job.chain == "solana" {
        let url = state
            .config
            .solana_rpc_url
            .as_ref()
            .ok_or_else(|| "missing CUSTODY_SOLANA_RPC_URL".to_string())?;
        return solana_get_signature_confirmed(&state.http, url, tx_hash).await;
    }

    if is_evm_chain(&job.chain) {
        let url = rpc_url_for_chain(&state.config, &job.chain)
            .ok_or_else(|| format!("missing RPC URL for chain {}", job.chain))?;
        if let Some(receipt) = evm_get_transaction_receipt(&state.http, &url, tx_hash).await? {
            let status = receipt
                .get("status")
                .and_then(|value| value.as_str())
                .unwrap_or("0x0");
            return Ok(Some(status == "0x1"));
        }
        return Ok(None);
    }

    Ok(None)
}
