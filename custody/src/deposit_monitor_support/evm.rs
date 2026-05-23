use super::*;

pub(super) async fn process_evm_deposits_for_chains(
    state: &CustodyState,
    url: &str,
    chains: &[&str],
) -> Result<(), String> {
    let deposits = list_pending_deposits_for_chains(&state.db, chains)?;
    let block_number = evm_get_block_number(&state.http, url).await?;

    if let Err(error) =
        process_evm_erc20_deposits(state, url, chains, &deposits, block_number).await
    {
        tracing::warn!("erc20 log scan failed (non-fatal): {}", error);
    }

    for deposit in deposits {
        if is_evm_token_asset(&deposit.chain, &deposit.asset) {
            continue;
        }
        let balance = evm_get_balance(&state.http, url, &deposit.address).await?;
        if balance == 0 {
            continue;
        }

        let last_balance = get_last_balance(&state.db, &deposit.address)?;
        if last_balance >= balance {
            continue;
        }

        let amount_u64 = u64::try_from(balance).ok();
        let sweep_job =
            treasury_for_chain(&state.config, &deposit.chain).map(|treasury| SweepJob {
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
            });
        let observation = DepositObservationWrite {
            event: DepositEvent {
                event_id: Uuid::new_v4().to_string(),
                deposit_id: deposit.deposit_id.clone(),
                tx_hash: format!("balance:{}", balance),
                confirmations: state.config.evm_confirmations,
                amount: amount_u64,
                status: "confirmed".to_string(),
                observed_at: chrono::Utc::now().timestamp(),
            },
            sweep_job,
            markers: vec![DepositObservationMarker::AddressBalance {
                address: deposit.address.clone(),
                balance,
            }],
        };

        if persist_deposit_observation(&state.db, &observation)? {
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
        }
    }

    Ok(())
}

pub(super) async fn process_evm_deposits(state: &CustodyState, url: &str) -> Result<(), String> {
    let deposits = list_pending_deposits_for_chains(&state.db, &["ethereum", "eth", "bsc", "bnb"])?;
    let block_number = evm_get_block_number(&state.http, url).await?;

    if let Err(error) = process_evm_erc20_deposits(
        state,
        url,
        &["ethereum", "eth", "bsc", "bnb"],
        &deposits,
        block_number,
    )
    .await
    {
        tracing::warn!("erc20 log scan failed (non-fatal): {}", error);
    }

    for deposit in deposits {
        if is_evm_token_asset(&deposit.chain, &deposit.asset) {
            continue;
        }
        let balance = evm_get_balance(&state.http, url, &deposit.address).await?;
        if balance == 0 {
            continue;
        }

        let last_balance = get_last_balance(&state.db, &deposit.address)?;
        if last_balance >= balance {
            continue;
        }

        let amount_u64 = u64::try_from(balance).ok();
        let sweep_job = state
            .config
            .treasury_evm_address
            .clone()
            .map(|treasury| SweepJob {
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
            });
        let observation = DepositObservationWrite {
            event: DepositEvent {
                event_id: Uuid::new_v4().to_string(),
                deposit_id: deposit.deposit_id.clone(),
                tx_hash: format!("balance:{}", balance),
                confirmations: state.config.evm_confirmations,
                amount: amount_u64,
                status: "confirmed".to_string(),
                observed_at: chrono::Utc::now().timestamp(),
            },
            sweep_job,
            markers: vec![DepositObservationMarker::AddressBalance {
                address: deposit.address.clone(),
                balance,
            }],
        };

        if persist_deposit_observation(&state.db, &observation)? {
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
        }
    }

    Ok(())
}

async fn process_evm_erc20_deposits(
    state: &CustodyState,
    url: &str,
    chains: &[&str],
    deposits: &[DepositRequest],
    block_number: u64,
) -> Result<(), String> {
    let token_deposits: Vec<&DepositRequest> = deposits
        .iter()
        .filter(|deposit| is_evm_token_asset(&deposit.chain, &deposit.asset))
        .collect();
    if token_deposits.is_empty() {
        return Ok(());
    }

    let mut address_map: std::collections::HashMap<String, Vec<&DepositRequest>> =
        std::collections::HashMap::new();
    let mut token_scans: std::collections::BTreeMap<(String, String), String> =
        std::collections::BTreeMap::new();
    for deposit in token_deposits {
        let contract = evm_token_contract_for_asset(&state.config, &deposit.chain, &deposit.asset)?;
        address_map
            .entry(deposit.address.to_lowercase())
            .or_default()
            .push(deposit);
        token_scans.insert(
            (deposit.asset.to_lowercase(), contract.to_lowercase()),
            contract,
        );
    }

    let cursor_scope = chains
        .iter()
        .filter_map(|chain| canonical_evm_chain(chain))
        .next()
        .unwrap_or("legacy");

    for ((asset, contract_lower), contract) in token_scans {
        let cursor_key = format!("evm_logs:{}:{}", cursor_scope, contract.to_lowercase());
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

        struct EvmTokenDepositEmission {
            deposit_id: String,
            tx_hash: String,
            chain: String,
            asset: String,
            address: String,
            user_id: String,
            amount: u128,
        }

        let logs = evm_get_transfer_logs(&state.http, url, &contract, from_block, to_block).await?;
        let mut observations = Vec::new();
        let mut emissions = Vec::new();
        for log in logs {
            if let Some((to, amount, tx_hash)) = decode_transfer_log(&log) {
                if let Some(deposits) = address_map.get(&to.to_lowercase()) {
                    for deposit in deposits.iter().copied().filter(|deposit| {
                        deposit.asset.eq_ignore_ascii_case(&asset)
                            && evm_token_contract_for_asset(
                                &state.config,
                                &deposit.chain,
                                &deposit.asset,
                            )
                            .map(|deposit_contract| {
                                deposit_contract.eq_ignore_ascii_case(&contract_lower)
                            })
                            .unwrap_or(false)
                    }) {
                        if deposit_event_already_processed(&state.db, &deposit.deposit_id, &tx_hash)
                        {
                            continue;
                        }

                        let sweep_job =
                            treasury_for_chain(&state.config, &deposit.chain).map(|treasury| {
                                SweepJob {
                                    job_id: Uuid::new_v4().to_string(),
                                    deposit_id: deposit.deposit_id.clone(),
                                    chain: deposit.chain.clone(),
                                    asset: deposit.asset.clone(),
                                    from_address: deposit.address.clone(),
                                    to_treasury: treasury,
                                    tx_hash: tx_hash.clone(),
                                    amount: Some(amount.to_string()),
                                    credited_amount: None,
                                    signatures: Vec::new(),
                                    sweep_tx_hash: None,
                                    attempts: 0,
                                    last_error: None,
                                    next_attempt_at: None,
                                    status: "queued".to_string(),
                                    created_at: chrono::Utc::now().timestamp(),
                                }
                            });
                        observations.push(DepositObservationWrite {
                            event: DepositEvent {
                                event_id: Uuid::new_v4().to_string(),
                                deposit_id: deposit.deposit_id.clone(),
                                tx_hash: tx_hash.clone(),
                                confirmations: state.config.evm_confirmations,
                                amount: u64::try_from(amount).ok(),
                                status: "confirmed".to_string(),
                                observed_at: chrono::Utc::now().timestamp(),
                            },
                            sweep_job,
                            markers: Vec::new(),
                        });
                        emissions.push(EvmTokenDepositEmission {
                            deposit_id: deposit.deposit_id.clone(),
                            tx_hash: tx_hash.clone(),
                            chain: deposit.chain.clone(),
                            asset: deposit.asset.clone(),
                            address: deposit.address.clone(),
                            user_id: deposit.user_id.clone(),
                            amount,
                        });
                    }
                }
            }
        }

        let committed = persist_deposit_observations(
            &state.db,
            &observations,
            &[DepositObservationMarker::Cursor {
                key: cursor_key,
                value: to_block.saturating_add(1),
            }],
        )?;
        for index in committed {
            if let Some(emission) = emissions.get(index) {
                emit_custody_event(
                    state,
                    "deposit.confirmed",
                    &emission.deposit_id,
                    Some(&emission.deposit_id),
                    Some(&emission.tx_hash),
                    Some(&serde_json::json!({
                        "chain": emission.chain,
                        "asset": emission.asset,
                        "address": emission.address,
                        "user_id": emission.user_id,
                        "amount": emission.amount
                    })),
                );
            }
        }
    }

    Ok(())
}
