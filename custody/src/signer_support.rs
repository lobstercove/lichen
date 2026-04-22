use super::*;

#[derive(Debug, Serialize)]
pub(super) struct SignerRequest {
    pub(super) job_id: String,
    pub(super) chain: String,
    pub(super) asset: String,
    pub(super) from_address: String,
    pub(super) to_address: String,
    pub(super) amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tx_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) message_hex: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SignerResponse {
    pub(super) status: String,
    pub(super) signer_pubkey: String,
    pub(super) signature: String,
    pub(super) message_hash: String,
    pub(super) _message: String,
}

pub(super) fn promote_locally_signed_sweep_jobs(
    state: &CustodyState,
    sweep_mode: &str,
) -> Result<(), String> {
    let mut signing_jobs = list_sweep_jobs_by_status(&state.db, "signing")?;
    for job in signing_jobs.iter_mut() {
        if !job.signatures.is_empty() {
            job.signatures.clear();
        }
        job.status = "signed".to_string();
        store_sweep_job(&state.db, job)?;
        emit_custody_event(
            state,
            "sweep.signed",
            &job.job_id,
            Some(&job.deposit_id),
            None,
            Some(&json!({
                "mode": sweep_mode,
                "threshold_signing": false,
            })),
        );
    }
    Ok(())
}

#[allow(dead_code)]
async fn collect_evm_multisig_signatures(
    state: &CustodyState,
    job: &mut SweepJob,
    tx_hash: &[u8],
) -> Result<usize, String> {
    let request = SignerRequest {
        job_id: job.job_id.clone(),
        chain: job.chain.clone(),
        asset: job.asset.clone(),
        from_address: job.from_address.clone(),
        to_address: job.to_treasury.clone(),
        amount: job.amount.clone(),
        tx_hash: Some(hex::encode(tx_hash)),
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
                    let already_signed = job
                        .signatures
                        .iter()
                        .any(|signature| signature.signer_pubkey == payload.signer_pubkey);
                    if !already_signed {
                        job.signatures.push(SignerSignature {
                            kind: SignerSignatureKind::EvmEcdsa,
                            signer_pubkey: payload.signer_pubkey,
                            signature: payload.signature,
                            message_hash: payload.message_hash,
                            received_at: chrono::Utc::now().timestamp(),
                        });
                    }
                }
                _ => {}
            },
            Err(error) => {
                warn!("EVM signer request failed for signer {}: {}", idx, error);
            }
        }

        if job.signatures.len() >= state.config.signer_threshold {
            break;
        }
    }

    Ok(job.signatures.len())
}

#[allow(dead_code)]
async fn collect_signatures(state: &CustodyState, job: &mut SweepJob) -> Result<usize, String> {
    let request = SignerRequest {
        job_id: job.job_id.clone(),
        chain: job.chain.clone(),
        asset: job.asset.clone(),
        from_address: job.from_address.clone(),
        to_address: job.to_treasury.clone(),
        amount: job.amount.clone(),
        tx_hash: Some(job.tx_hash.clone()),
        message_hex: None,
    };

    for (idx, endpoint) in state.config.signer_endpoints.iter().enumerate() {
        let url = format!("{}/sign", endpoint.trim_end_matches('/'));
        let mut req = state.http.post(url).json(&request);
        let token = state
            .config
            .signer_auth_tokens
            .get(idx)
            .and_then(|token| token.as_ref())
            .or(state.config.signer_auth_token.as_ref());
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        let response = match req.send().await {
            Ok(response) => response,
            Err(error) => {
                warn!("signer request failed: {}", error);
                continue;
            }
        };
        let payload: SignerResponse = match response.json().await {
            Ok(payload) => payload,
            Err(error) => {
                warn!("signer response decode failed: {}", error);
                continue;
            }
        };

        if payload.status != "signed" {
            continue;
        }

        if job
            .signatures
            .iter()
            .any(|signature| signature.signer_pubkey == payload.signer_pubkey)
        {
            continue;
        }

        job.signatures.push(SignerSignature {
            kind: SignerSignatureKind::EvmEcdsa,
            signer_pubkey: payload.signer_pubkey,
            signature: payload.signature,
            message_hash: payload.message_hash,
            received_at: chrono::Utc::now().timestamp(),
        });

        if job.signatures.len() >= state.config.signer_threshold {
            break;
        }
    }

    Ok(job.signatures.len())
}
