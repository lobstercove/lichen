use super::*;

mod evm;
mod solana;

pub(super) async fn broadcast_outbound_withdrawal(
    state: &CustodyState,
    job: &WithdrawalJob,
) -> Result<String, String> {
    match job.dest_chain.as_str() {
        "solana" | "sol" => {
            let url = state
                .config
                .solana_rpc_url
                .as_ref()
                .ok_or_else(|| "missing solana RPC".to_string())?;
            let outbound_asset = match job.asset.to_lowercase().as_str() {
                "wsol" => "sol".to_string(),
                "musd" => job.preferred_stablecoin.clone(),
                _ => return Err(format!("unsupported solana withdrawal: {}", job.asset)),
            };

            if matches!(
                determine_withdrawal_signing_mode(state, job, &outbound_asset)?,
                Some(WithdrawalSigningMode::PqApprovalQuorum)
            ) {
                let approval_count =
                    valid_pq_withdrawal_approvers(state, job, &outbound_asset)?.len();
                if approval_count < state.config.signer_threshold {
                    return Err(format!(
                        "insufficient PQ withdrawal approvals: have {}, need {}",
                        approval_count, state.config.signer_threshold
                    ));
                }
            }

            solana::broadcast_self_custody_solana_withdrawal(state, url, job, &outbound_asset).await
        }
        "ethereum" | "eth" | "bsc" | "bnb" => {
            let url = rpc_url_for_chain(&state.config, &job.dest_chain)
                .ok_or_else(|| format!("missing RPC URL for chain {}", job.dest_chain))?;
            let outbound_asset = match job.asset.to_lowercase().as_str() {
                "weth" => "eth".to_string(),
                "wbnb" => "bnb".to_string(),
                "musd" => job.preferred_stablecoin.clone(),
                _ => return Err(format!("unsupported EVM withdrawal: {}", job.asset)),
            };

            match determine_withdrawal_signing_mode(state, job, &outbound_asset)? {
                Some(WithdrawalSigningMode::EvmThresholdSafe) => {
                    let signed_tx = assemble_signed_evm_tx(state, job, &outbound_asset).await?;
                    let tx_hex = format!("0x{}", hex::encode(&signed_tx));
                    let result =
                        evm_rpc_call(&state.http, &url, "eth_sendRawTransaction", json!([tx_hex]))
                            .await?;
                    result
                        .as_str()
                        .map(|value| value.to_string())
                        .ok_or_else(|| "no tx hash returned".to_string())
                }
                Some(WithdrawalSigningMode::PqApprovalQuorum) => {
                    let approval_count =
                        valid_pq_withdrawal_approvers(state, job, &outbound_asset)?.len();
                    if approval_count < state.config.signer_threshold {
                        return Err(format!(
                            "insufficient PQ withdrawal approvals: have {}, need {}",
                            approval_count, state.config.signer_threshold
                        ));
                    }
                    evm::broadcast_self_custody_evm_withdrawal(state, &url, job, &outbound_asset)
                        .await
                }
                None => {
                    evm::broadcast_self_custody_evm_withdrawal(state, &url, job, &outbound_asset)
                        .await
                }
            }
        }
        other => Err(format!("unsupported destination chain: {}", other)),
    }
}

pub(super) async fn assemble_signed_evm_tx(
    state: &CustodyState,
    job: &WithdrawalJob,
    asset: &str,
) -> Result<Vec<u8>, String> {
    evm::assemble_signed_evm_tx(state, job, asset).await
}
