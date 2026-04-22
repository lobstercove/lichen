use super::*;

pub(super) async fn process_evm_deposits_for_chains(
    state: &CustodyState,
    url: &str,
    chains: &[&str],
) -> Result<(), String> {
    let deposits = list_pending_deposits_for_chains(&state.db, chains)?;
    let block_number = evm_get_block_number(&state.http, url).await?;

    if let Err(error) = process_evm_erc20_deposits(state, url, &deposits, block_number).await {
        tracing::warn!("erc20 log scan failed (non-fatal): {}", error);
    }

    for deposit in deposits {
        let balance = evm_get_balance(&state.http, url, &deposit.address).await?;
        if balance == 0 {
            continue;
        }

        let last_balance = get_last_balance(&state.db, &deposit.address)?;
        if last_balance >= balance {
            continue;
        }

        set_last_balance(&state.db, &deposit.address, balance)?;

        let amount_u64 = u64::try_from(balance).ok();
        store_deposit_event(
            &state.db,
            &DepositEvent {
                event_id: Uuid::new_v4().to_string(),
                deposit_id: deposit.deposit_id.clone(),
                tx_hash: format!("balance:{}", balance),
                confirmations: state.config.evm_confirmations,
                amount: amount_u64,
                status: "confirmed".to_string(),
                observed_at: chrono::Utc::now().timestamp(),
            },
        )?;

        update_deposit_status(&state.db, &deposit.deposit_id, "confirmed")?;
        emit_custody_event(
            state,
            "deposit.confirmed",
            &deposit.deposit_id,
            Some(&deposit.deposit_id),
            None,
            Some(&serde_json::json!({
                "chain": deposit.chain,
                "asset": deposit.asset,
                "address": deposit.address,
                "user_id": deposit.user_id,
                "amount": balance
            })),
        );

        if let Some(treasury) = treasury_for_chain(&state.config, &deposit.chain) {
            enqueue_sweep_job(
                &state.db,
                &SweepJob {
                    job_id: Uuid::new_v4().to_string(),
                    deposit_id: deposit.deposit_id.clone(),
                    chain: deposit.chain.clone(),
                    asset: deposit.asset.clone(),
                    from_address: deposit.address.clone(),
                    to_treasury: treasury,
                    tx_hash: format!("balance:{}:block:{}", balance, block_number),
                    amount: Some(balance.to_string()),
                    credited_amount: None,
                    signatures: Vec::new(),
                    sweep_tx_hash: None,
                    attempts: 0,
                    last_error: None,
                    next_attempt_at: None,
                    status: "queued".to_string(),
                    created_at: chrono::Utc::now().timestamp(),
                },
            )?;
            update_deposit_status(&state.db, &deposit.deposit_id, "sweep_queued")?;
        }
    }

    Ok(())
}

pub(super) async fn process_evm_deposits(state: &CustodyState, url: &str) -> Result<(), String> {
    let deposits = list_pending_deposits_for_chains(&state.db, &["ethereum", "eth", "bsc", "bnb"])?;
    let block_number = evm_get_block_number(&state.http, url).await?;

    if let Err(error) = process_evm_erc20_deposits(state, url, &deposits, block_number).await {
        tracing::warn!("erc20 log scan failed (non-fatal): {}", error);
    }

    for deposit in deposits {
        let balance = evm_get_balance(&state.http, url, &deposit.address).await?;
        if balance == 0 {
            continue;
        }

        let last_balance = get_last_balance(&state.db, &deposit.address)?;
        if last_balance >= balance {
            continue;
        }

        set_last_balance(&state.db, &deposit.address, balance)?;

        let amount_u64 = u64::try_from(balance).ok();
        store_deposit_event(
            &state.db,
            &DepositEvent {
                event_id: Uuid::new_v4().to_string(),
                deposit_id: deposit.deposit_id.clone(),
                tx_hash: format!("balance:{}", balance),
                confirmations: state.config.evm_confirmations,
                amount: amount_u64,
                status: "confirmed".to_string(),
                observed_at: chrono::Utc::now().timestamp(),
            },
        )?;

        update_deposit_status(&state.db, &deposit.deposit_id, "confirmed")?;
        emit_custody_event(
            state,
            "deposit.confirmed",
            &deposit.deposit_id,
            Some(&deposit.deposit_id),
            None,
            Some(&serde_json::json!({
                "chain": deposit.chain,
                "asset": deposit.asset,
                "address": deposit.address,
                "user_id": deposit.user_id,
                "amount": balance
            })),
        );

        if let Some(treasury) = state.config.treasury_evm_address.clone() {
            enqueue_sweep_job(
                &state.db,
                &SweepJob {
                    job_id: Uuid::new_v4().to_string(),
                    deposit_id: deposit.deposit_id.clone(),
                    chain: deposit.chain.clone(),
                    asset: deposit.asset.clone(),
                    from_address: deposit.address.clone(),
                    to_treasury: treasury,
                    tx_hash: format!("balance:{}:block:{}", balance, block_number),
                    amount: Some(balance.to_string()),
                    credited_amount: None,
                    signatures: Vec::new(),
                    sweep_tx_hash: None,
                    attempts: 0,
                    last_error: None,
                    next_attempt_at: None,
                    status: "queued".to_string(),
                    created_at: chrono::Utc::now().timestamp(),
                },
            )?;
            update_deposit_status(&state.db, &deposit.deposit_id, "sweep_queued")?;
        }
    }

    Ok(())
}

async fn process_evm_erc20_deposits(
    state: &CustodyState,
    url: &str,
    deposits: &[DepositRequest],
    block_number: u64,
) -> Result<(), String> {
    let token_deposits: Vec<&DepositRequest> = deposits
        .iter()
        .filter(|deposit| matches!(deposit.asset.as_str(), "usdc" | "usdt"))
        .collect();
    if token_deposits.is_empty() {
        return Ok(());
    }

    let mut address_map = std::collections::HashMap::new();
    for deposit in token_deposits {
        address_map.insert(deposit.address.to_lowercase(), deposit);
    }

    for asset in ["usdc", "usdt"] {
        let contract = evm_contract_for_asset(&state.config, asset)?;
        let cursor_key = format!("evm_logs:{}", contract.to_lowercase());
        let from_block = get_last_u64_index(&state.db, &cursor_key)?
            .unwrap_or(block_number.saturating_sub(1000));
        let to_block = block_number.saturating_sub(state.config.evm_confirmations);
        if to_block < from_block {
            continue;
        }
        let from_block = if to_block - from_block > 10_000 {
            to_block - 10_000
        } else {
            from_block
        };

        let logs = evm_get_transfer_logs(&state.http, url, &contract, from_block, to_block).await?;
        for log in logs {
            if let Some((to, amount, tx_hash)) = decode_transfer_log(&log) {
                if let Some(deposit) = address_map.get(&to.to_lowercase()) {
                    store_deposit_event(
                        &state.db,
                        &DepositEvent {
                            event_id: Uuid::new_v4().to_string(),
                            deposit_id: deposit.deposit_id.clone(),
                            tx_hash: tx_hash.clone(),
                            confirmations: state.config.evm_confirmations,
                            amount: u64::try_from(amount).ok(),
                            status: "confirmed".to_string(),
                            observed_at: chrono::Utc::now().timestamp(),
                        },
                    )?;
                    update_deposit_status(&state.db, &deposit.deposit_id, "confirmed")?;
                    emit_custody_event(
                        state,
                        "deposit.confirmed",
                        &deposit.deposit_id,
                        Some(&deposit.deposit_id),
                        Some(&tx_hash),
                        Some(&serde_json::json!({
                            "chain": deposit.chain,
                            "asset": deposit.asset,
                            "address": deposit.address,
                            "user_id": deposit.user_id,
                            "amount": amount
                        })),
                    );

                    if let Some(treasury) = state.config.treasury_evm_address.clone() {
                        enqueue_sweep_job(
                            &state.db,
                            &SweepJob {
                                job_id: Uuid::new_v4().to_string(),
                                deposit_id: deposit.deposit_id.clone(),
                                chain: deposit.chain.clone(),
                                asset: deposit.asset.clone(),
                                from_address: deposit.address.clone(),
                                to_treasury: treasury,
                                tx_hash,
                                amount: Some(amount.to_string()),
                                credited_amount: None,
                                signatures: Vec::new(),
                                sweep_tx_hash: None,
                                attempts: 0,
                                last_error: None,
                                next_attempt_at: None,
                                status: "queued".to_string(),
                                created_at: chrono::Utc::now().timestamp(),
                            },
                        )?;
                        update_deposit_status(&state.db, &deposit.deposit_id, "sweep_queued")?;
                    }
                }
            }
        }

        set_last_u64_index(&state.db, &cursor_key, to_block.saturating_add(1))?;
    }

    Ok(())
}
