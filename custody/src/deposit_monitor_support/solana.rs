use super::*;

pub(super) async fn process_solana_deposits(state: &CustodyState, url: &str) -> Result<(), String> {
    let deposits = list_pending_deposits_for_chains(&state.db, &["solana", "sol"])?;
    for deposit in deposits {
        if is_solana_stablecoin(&deposit.asset) {
            if let Err(error) = process_solana_token_deposit(state, url, &deposit).await {
                tracing::warn!(
                    "solana token deposit scan failed for deposit={} asset={} address={}: {}",
                    deposit.deposit_id,
                    deposit.asset,
                    deposit.address,
                    error
                );
            }
            continue;
        }
        let signatures =
            solana_get_signatures_for_address(&state.http, url, &deposit.address).await?;
        if signatures.is_empty() {
            continue;
        }

        for sig in &signatures {
            if deposit_event_already_processed(&state.db, &deposit.deposit_id, sig) {
                continue;
            }

            let status = solana_get_signature_status(&state.http, url, sig).await?;
            let confirmed = status.confirmation_status == Some("finalized".to_string())
                || status.confirmations.unwrap_or(0) >= state.config.solana_confirmations;

            if !confirmed {
                continue;
            }

            let mut sweep_job = None;
            if let Some(treasury) = state.config.treasury_solana_address.clone() {
                let balance = solana_get_balance(&state.http, url, &deposit.address).await?;
                let credited_amount = if balance > SOLANA_SWEEP_FEE_LAMPORTS {
                    Some((balance - SOLANA_SWEEP_FEE_LAMPORTS).to_string())
                } else {
                    None
                };
                sweep_job = Some(SweepJob {
                    job_id: Uuid::new_v4().to_string(),
                    deposit_id: deposit.deposit_id.clone(),
                    chain: deposit.chain.clone(),
                    asset: deposit.asset.clone(),
                    from_address: deposit.address.clone(),
                    to_treasury: treasury,
                    tx_hash: sig.clone(),
                    amount: Some(balance.to_string()),
                    credited_amount,
                    signatures: Vec::new(),
                    sweep_tx_hash: None,
                    attempts: 0,
                    last_error: None,
                    next_attempt_at: None,
                    status: "queued".to_string(),
                    created_at: chrono::Utc::now().timestamp(),
                });
            }

            let observation = DepositObservationWrite {
                event: DepositEvent {
                    event_id: Uuid::new_v4().to_string(),
                    deposit_id: deposit.deposit_id.clone(),
                    tx_hash: sig.clone(),
                    confirmations: status.confirmations.unwrap_or(0),
                    amount: None,
                    status: "confirmed".to_string(),
                    observed_at: chrono::Utc::now().timestamp(),
                },
                sweep_job,
                markers: Vec::new(),
            };

            if persist_deposit_observation(&state.db, &observation)? {
                emit_custody_event(
                    state,
                    "deposit.confirmed",
                    &deposit.deposit_id,
                    Some(&deposit.deposit_id),
                    Some(sig),
                    Some(&serde_json::json!({
                        "chain": deposit.chain,
                        "asset": deposit.asset,
                        "address": deposit.address,
                        "user_id": deposit.user_id
                    })),
                );
            }
            break;
        }
    }

    Ok(())
}

async fn process_solana_token_deposit(
    state: &CustodyState,
    url: &str,
    deposit: &DepositRequest,
) -> Result<(), String> {
    let balance = solana_get_token_balance(&state.http, url, &deposit.address).await?;
    let last_key = format!("spl:{}:{}", deposit.asset, deposit.address);

    if balance == 0 {
        if let Err(error) = set_last_balance_with_key(&state.db, &last_key, 0) {
            tracing::error!("Failed set_last_balance_with_key: {error}");
        }
        return Ok(());
    }

    let last_balance = get_last_balance_with_key(&state.db, &last_key)?;
    if last_balance >= balance {
        return Ok(());
    }

    let synthetic_tx_hash = format!("spl_balance:{}", balance);
    if deposit_event_already_processed(&state.db, &deposit.deposit_id, &synthetic_tx_hash) {
        return Ok(());
    }

    let mut sweep_job = None;
    if let Some(treasury) = state.config.solana_treasury_owner.clone() {
        let mint = solana_mint_for_asset(&state.config, &deposit.asset)?;
        let treasury_ata = derive_associated_token_address_from_str(&treasury, &mint)?;
        ensure_associated_token_account_for_str(state, &treasury, &mint, &treasury_ata).await?;

        sweep_job = Some(SweepJob {
            job_id: Uuid::new_v4().to_string(),
            deposit_id: deposit.deposit_id.clone(),
            chain: deposit.chain.clone(),
            asset: deposit.asset.clone(),
            from_address: deposit.address.clone(),
            to_treasury: treasury_ata,
            tx_hash: synthetic_tx_hash.clone(),
            amount: Some(balance.to_string()),
            credited_amount: None,
            signatures: Vec::new(),
            sweep_tx_hash: None,
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            status: "queued".to_string(),
            created_at: chrono::Utc::now().timestamp(),
        });
    }

    let observation = DepositObservationWrite {
        event: DepositEvent {
            event_id: Uuid::new_v4().to_string(),
            deposit_id: deposit.deposit_id.clone(),
            tx_hash: synthetic_tx_hash.clone(),
            confirmations: state.config.solana_confirmations,
            amount: Some(balance as u64),
            status: "confirmed".to_string(),
            observed_at: chrono::Utc::now().timestamp(),
        },
        sweep_job,
        markers: vec![DepositObservationMarker::TokenBalance {
            key: last_key,
            balance,
        }],
    };

    if persist_deposit_observation(&state.db, &observation)? {
        emit_custody_event(
            state,
            "deposit.confirmed",
            &deposit.deposit_id,
            Some(&deposit.deposit_id),
            Some(&synthetic_tx_hash),
            Some(&serde_json::json!({
                "chain": deposit.chain,
                "asset": deposit.asset,
                "address": deposit.address,
                "user_id": deposit.user_id,
                "amount": balance
            })),
        );
    }

    Ok(())
}
