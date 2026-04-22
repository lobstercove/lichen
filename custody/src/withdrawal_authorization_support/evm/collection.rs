use super::super::*;
use super::plan::build_evm_safe_transaction_plan;

pub(crate) async fn collect_threshold_evm_withdrawal_signatures(
    state: &CustodyState,
    job: &mut WithdrawalJob,
    outbound_asset: &str,
    required_threshold: usize,
) -> Result<usize, String> {
    let url = rpc_url_for_chain(&state.config, &job.dest_chain)
        .ok_or_else(|| format!("missing RPC URL for chain {}", job.dest_chain))?;
    let plan = build_evm_safe_transaction_plan(state, &url, job, outbound_asset).await?;
    let safe_tx_hash_hex = hex::encode(plan.safe_tx_hash);
    job.safe_nonce = Some(plan.nonce);

    if job.signatures.iter().any(|signature| {
        signature.kind != SignerSignatureKind::EvmEcdsa
            || signature.message_hash != safe_tx_hash_hex
    }) {
        job.signatures.clear();
    }

    let request = SignerRequest {
        job_id: job.job_id.clone(),
        chain: job.dest_chain.clone(),
        asset: outbound_asset.to_string(),
        from_address: plan.safe_address,
        to_address: job.dest_address.clone(),
        amount: Some(job.amount.to_string()),
        tx_hash: Some(safe_tx_hash_hex.clone()),
        message_hex: None,
    };

    for (idx, endpoint) in state.config.signer_endpoints.iter().enumerate() {
        let url = format!("{}/sign", endpoint.trim_end_matches('/'));
        let mut req = state.http.post(&url).json(&request);
        let token = state
            .config
            .signer_auth_tokens
            .get(idx)
            .and_then(|token| token.as_ref())
            .or(state.config.signer_auth_token.as_ref());
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }

        match req.send().await {
            Ok(response) => match response.json::<SignerResponse>().await {
                Ok(payload) if payload.status == "signed" => {
                    let signer_addr = payload
                        .signer_pubkey
                        .trim_start_matches("0x")
                        .to_lowercase();
                    let already_signed = job.signatures.iter().any(|signature| {
                        signature
                            .signer_pubkey
                            .trim_start_matches("0x")
                            .eq_ignore_ascii_case(&signer_addr)
                    });
                    if !already_signed {
                        job.signatures.push(SignerSignature {
                            kind: SignerSignatureKind::EvmEcdsa,
                            signer_pubkey: signer_addr,
                            signature: payload.signature,
                            message_hash: safe_tx_hash_hex.clone(),
                            received_at: chrono::Utc::now().timestamp(),
                        });
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(
                        "EVM signer response decode failed for {}: {}",
                        job.job_id, error
                    );
                }
            },
            Err(error) => {
                warn!("EVM signer request failed for {}: {}", job.job_id, error);
            }
        }

        if required_threshold > 0 && job.signatures.len() >= required_threshold {
            break;
        }
    }

    Ok(job.signatures.len())
}
