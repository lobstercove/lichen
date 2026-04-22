use super::evm::evm_executor_derivation_path;
use super::solana::resolve_solana_token_withdrawal_accounts;
use super::*;

#[derive(Debug, Deserialize)]
struct PqSignerResponse {
    status: String,
    #[serde(alias = "signature")]
    pq_signature: PqSignature,
}

#[derive(Debug, Serialize)]
struct WithdrawalApprovalMessage<'a> {
    domain: &'static str,
    version: u8,
    job_id: &'a str,
    user_id: &'a str,
    wrapped_asset: &'a str,
    outbound_asset: &'a str,
    outbound_amount: String,
    dest_chain: &'a str,
    dest_address: &'a str,
    preferred_stablecoin: &'a str,
    executor_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_contract: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_token_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination_token_account: Option<String>,
}

fn withdrawal_authorization_executor_address(
    state: &CustodyState,
    dest_chain: &str,
) -> Result<String, String> {
    match dest_chain {
        "solana" | "sol" => {
            derive_solana_address("custody/treasury/solana", &state.config.master_seed)
        }
        chain if is_evm_chain(chain) => derive_evm_address(
            evm_executor_derivation_path(chain),
            &state.config.master_seed,
        ),
        other => Err(format!("unsupported destination chain: {}", other)),
    }
}

fn build_withdrawal_approval_message(
    state: &CustodyState,
    job: &WithdrawalJob,
    outbound_asset: &str,
) -> Result<Vec<u8>, String> {
    let outbound_amount = if job.dest_chain == "solana" && outbound_asset == "sol" {
        if job.amount <= SOLANA_SWEEP_FEE_LAMPORTS {
            return Err("withdrawal amount too small to cover fees".to_string());
        }
        (job.amount - SOLANA_SWEEP_FEE_LAMPORTS).to_string()
    } else {
        spores_to_chain_amount(job.amount, &job.dest_chain, outbound_asset).to_string()
    };

    let mut token_contract = None;
    let mut source_token_account = None;
    let mut destination_token_account = None;

    if job.dest_chain == "solana" || job.dest_chain == "sol" {
        if is_solana_stablecoin(outbound_asset) {
            let (_, mint, from_token_account, to_token_account) =
                resolve_solana_token_withdrawal_accounts(
                    &state.config,
                    outbound_asset,
                    &job.dest_address,
                )?;
            token_contract = Some(mint);
            source_token_account = Some(from_token_account);
            destination_token_account = Some(to_token_account);
        }
    } else if matches!(outbound_asset, "usdt" | "usdc") {
        token_contract = Some(evm_contract_for_asset(&state.config, outbound_asset)?);
    }

    serde_json::to_vec(&WithdrawalApprovalMessage {
        domain: "lichen-custody-withdrawal-approval",
        version: 1,
        job_id: &job.job_id,
        user_id: &job.user_id,
        wrapped_asset: &job.asset,
        outbound_asset,
        outbound_amount,
        dest_chain: &job.dest_chain,
        dest_address: &job.dest_address,
        preferred_stablecoin: &job.preferred_stablecoin,
        executor_address: withdrawal_authorization_executor_address(state, &job.dest_chain)?,
        token_contract,
        source_token_account,
        destination_token_account,
    })
    .map_err(|error| format!("encode withdrawal approval message: {}", error))
}

pub(crate) fn valid_pq_withdrawal_approvers(
    state: &CustodyState,
    job: &WithdrawalJob,
    outbound_asset: &str,
) -> Result<BTreeSet<Pubkey>, String> {
    let message = build_withdrawal_approval_message(state, job, outbound_asset)?;
    let message_hex = hex::encode(&message);
    let allowed: BTreeSet<Pubkey> = state.config.signer_pq_addresses.iter().copied().collect();
    let mut approvers = BTreeSet::new();

    for signature in &job.signatures {
        if signature.kind != SignerSignatureKind::PqApproval
            || signature.message_hash != message_hex
        {
            continue;
        }

        let signer_address = match Pubkey::from_base58(&signature.signer_pubkey) {
            Ok(address) => address,
            Err(_) => continue,
        };
        if !allowed.contains(&signer_address) {
            continue;
        }

        let pq_signature = match signature.decode_pq_signature() {
            Ok(decoded) => decoded,
            Err(_) => continue,
        };
        if Keypair::verify(&signer_address, &message, &pq_signature) {
            approvers.insert(signer_address);
        }
    }

    Ok(approvers)
}

pub(crate) async fn collect_pq_withdrawal_approvals(
    state: &CustodyState,
    job: &mut WithdrawalJob,
    outbound_asset: &str,
    required_threshold: usize,
) -> Result<usize, String> {
    validate_pq_signer_configuration(&state.config)?;

    let message = build_withdrawal_approval_message(state, job, outbound_asset)?;
    let message_hex = hex::encode(&message);
    job.signatures.retain(|signature| {
        signature.kind == SignerSignatureKind::PqApproval && signature.message_hash == message_hex
    });

    let mut approved = valid_pq_withdrawal_approvers(state, job, outbound_asset)?;
    let request = SignerRequest {
        job_id: job.job_id.clone(),
        chain: job.dest_chain.clone(),
        asset: outbound_asset.to_string(),
        from_address: withdrawal_authorization_executor_address(state, &job.dest_chain)?,
        to_address: job.dest_address.clone(),
        amount: Some(job.amount.to_string()),
        tx_hash: None,
        message_hex: Some(message_hex.clone()),
    };

    for (idx, endpoint) in state.config.signer_endpoints.iter().enumerate() {
        let expected_address = state.config.signer_pq_addresses[idx];
        if approved.contains(&expected_address) {
            continue;
        }

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
            Ok(response) => match response.json::<PqSignerResponse>().await {
                Ok(payload) if payload.status == "signed" => {
                    if Keypair::verify(&expected_address, &message, &payload.pq_signature) {
                        job.signatures.push(SignerSignature::pq_approval(
                            &expected_address,
                            message_hex.clone(),
                            &payload.pq_signature,
                        )?);
                        approved.insert(expected_address);
                    } else {
                        warn!(
                            "PQ signer response failed verification for withdrawal {} from signer {}",
                            job.job_id,
                            expected_address.to_base58()
                        );
                    }
                }
                Ok(payload) => {
                    warn!(
                        "PQ signer request for withdrawal {} returned status={}",
                        job.job_id, payload.status
                    );
                }
                Err(error) => {
                    warn!(
                        "PQ signer response decode failed for withdrawal {}: {}",
                        job.job_id, error
                    );
                }
            },
            Err(error) => {
                warn!(
                    "PQ signer request failed for withdrawal {}: {}",
                    job.job_id, error
                );
            }
        }

        if required_threshold > 0 && approved.len() >= required_threshold {
            break;
        }
    }

    Ok(approved.len())
}
