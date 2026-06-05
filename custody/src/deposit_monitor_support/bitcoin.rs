use super::*;

pub(super) async fn process_bitcoin_deposits(state: &CustodyState) -> Result<(), String> {
    let deposits = list_pending_deposits_for_chains(&state.db, &["bitcoin", "btc"])?;
    for deposit in deposits {
        if !deposit.asset.eq_ignore_ascii_case("btc") {
            continue;
        }
        validate_bitcoin_address_for_network(&deposit.address, &state.config.btc_network)?;
        let utxos = bitcoin_scan_confirmed_utxos(
            &state.http,
            &state.config,
            &deposit.address,
            state.config.btc_confirmations,
        )
        .await?;
        if utxos.is_empty() {
            continue;
        }
        let balance = utxos
            .iter()
            .try_fold(0u64, |acc, utxo| acc.checked_add(utxo.amount_sats))
            .ok_or_else(|| "bitcoin deposit amount overflow".to_string())?;
        if balance == 0 {
            continue;
        }

        let last_key = format!("btc:{}", deposit.address);
        let last_balance = get_last_balance_with_key(&state.db, &last_key)?;
        if last_balance >= balance {
            continue;
        }

        let tx_hash = format!(
            "btc_utxo_set:{}",
            utxos
                .iter()
                .map(|utxo| format!("{}:{}", utxo.txid, utxo.vout))
                .collect::<Vec<_>>()
                .join(",")
        );
        if deposit_event_already_processed(&state.db, &deposit.deposit_id, &tx_hash) {
            continue;
        }

        let sweep_job = state
            .config
            .treasury_btc_address
            .clone()
            .map(|treasury| SweepJob {
                job_id: Uuid::new_v4().to_string(),
                deposit_id: deposit.deposit_id.clone(),
                chain: deposit.chain.clone(),
                asset: deposit.asset.clone(),
                from_address: deposit.address.clone(),
                to_treasury: treasury,
                tx_hash: tx_hash.clone(),
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

        let confirmations = utxos
            .iter()
            .map(|utxo| utxo.confirmations)
            .min()
            .unwrap_or(0);
        let observation = DepositObservationWrite {
            event: DepositEvent {
                event_id: Uuid::new_v4().to_string(),
                deposit_id: deposit.deposit_id.clone(),
                tx_hash: tx_hash.clone(),
                confirmations,
                amount: Some(balance),
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
                Some(&tx_hash),
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
