use super::rebalance_output_support::{parse_evm_swap_output, parse_solana_swap_output};
use super::rebalance_threshold_support::check_rebalance_thresholds;
use super::*;

mod ethereum;
mod solana;

use self::ethereum::execute_ethereum_rebalance_swap;
use self::solana::execute_solana_rebalance_swap;

pub(super) async fn rebalance_worker_loop(state: CustodyState) {
    loop {
        if let Err(err) = process_rebalance_jobs(&state).await {
            tracing::warn!("rebalance worker error: {}", err);
        }

        if let Err(err) = check_rebalance_thresholds(&state) {
            tracing::warn!("rebalance threshold check error: {}", err);
        }

        tokio::time::sleep(std::time::Duration::from_secs(
            state.config.poll_interval_secs * 20,
        ))
        .await;
    }
}

pub(super) async fn process_rebalance_jobs(state: &CustodyState) -> Result<(), String> {
    let rebalance_policy_error = local_rebalance_policy_error(&state.config);
    let queued = list_rebalance_jobs_by_status(&state.db, "queued")?;
    for mut job in queued {
        if let Some(err) = rebalance_policy_error.as_ref() {
            job.attempts = job.attempts.saturating_add(1);
            job.status = "failed".to_string();
            job.last_error = Some(err.clone());
            job.next_attempt_at = None;
            store_rebalance_job(&state.db, &job)?;
            emit_custody_event(
                state,
                "rebalance.failed",
                &job.job_id,
                None,
                None,
                Some(&serde_json::json!({
                    "chain": job.chain,
                    "from_asset": job.from_asset,
                    "to_asset": job.to_asset,
                    "amount": job.amount,
                    "last_error": err,
                    "mode": "blocked-local-rebalance",
                })),
            );
            continue;
        }

        match execute_rebalance_swap(state, &job).await {
            Ok(tx_hash) => {
                job.swap_tx_hash = Some(tx_hash.clone());
                job.status = "submitted".to_string();
                job.last_error = None;
                store_rebalance_job(&state.db, &job)?;
                emit_custody_event(
                    state,
                    "rebalance.submitted",
                    &job.job_id,
                    None,
                    Some(&tx_hash),
                    Some(&serde_json::json!({
                        "chain": job.chain,
                        "from_asset": job.from_asset,
                        "to_asset": job.to_asset,
                        "amount": job.amount
                    })),
                );
                info!(
                    "rebalance swap submitted: {} {} → {} on {} (tx={})",
                    job.amount, job.from_asset, job.to_asset, job.chain, tx_hash
                );
            }
            Err(error) => {
                job.attempts = job.attempts.saturating_add(1);
                job.last_error = Some(error.clone());
                job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
                if job.attempts > 5 {
                    job.status = "failed".to_string();
                    tracing::error!(
                        "rebalance job {} failed permanently after {} attempts: {}",
                        job.job_id,
                        job.attempts,
                        error
                    );
                }
                store_rebalance_job(&state.db, &job)?;
            }
        }
    }

    let submitted = list_rebalance_jobs_by_status(&state.db, "submitted")?;
    for mut job in submitted {
        let confirmed = match job.chain.as_str() {
            "solana" => {
                if let (Some(url), Some(ref tx_hash)) =
                    (state.config.solana_rpc_url.as_ref(), &job.swap_tx_hash)
                {
                    solana_get_signature_confirmed(&state.http, url, tx_hash)
                        .await
                        .unwrap_or(None)
                        .unwrap_or(false)
                } else {
                    false
                }
            }
            "ethereum" => {
                if let (Some(url), Some(ref tx_hash)) =
                    (state.config.evm_rpc_url.as_ref(), &job.swap_tx_hash)
                {
                    check_evm_tx_confirmed(
                        &state.http,
                        url,
                        tx_hash,
                        state.config.evm_confirmations,
                    )
                    .await
                    .unwrap_or(false)
                } else {
                    false
                }
            }
            _ => false,
        };

        if confirmed {
            job.status = "confirmed".to_string();
            job.last_error = None;

            let actual_output = match job.chain.as_str() {
                "solana" => {
                    if let (Some(url), Some(ref tx_hash)) =
                        (state.config.solana_rpc_url.as_ref(), &job.swap_tx_hash)
                    {
                        let to_mint =
                            solana_mint_for_asset(&state.config, &job.to_asset).unwrap_or_default();
                        let treasury = state
                            .config
                            .treasury_solana_address
                            .as_deref()
                            .unwrap_or("");
                        parse_solana_swap_output(&state.http, url, tx_hash, treasury, &to_mint)
                            .await
                            .unwrap_or(None)
                    } else {
                        None
                    }
                }
                "ethereum" => {
                    if let (Some(url), Some(ref tx_hash)) =
                        (state.config.evm_rpc_url.as_ref(), &job.swap_tx_hash)
                    {
                        let to_contract = evm_contract_for_asset(&state.config, &job.to_asset)
                            .unwrap_or_default();
                        let treasury = state.config.treasury_evm_address.as_deref().unwrap_or("");
                        parse_evm_swap_output(&state.http, url, tx_hash, treasury, &to_contract)
                            .await
                            .unwrap_or(None)
                    } else {
                        None
                    }
                }
                _ => None,
            };

            let credit_amount = match actual_output {
                Some(output) => {
                    if job.amount > 0 {
                        let slippage_bps = (job.amount.saturating_sub(output) as u128 * 10_000
                            / job.amount as u128) as u64;
                        if slippage_bps > state.config.rebalance_max_slippage_bps {
                            tracing::error!(
                                "rebalance slippage {}bps exceeds max {}bps: input={} output={} (job={})",
                                slippage_bps,
                                state.config.rebalance_max_slippage_bps,
                                job.amount,
                                output,
                                job.job_id
                            );
                            job.status = "slippage_exceeded".to_string();
                            store_rebalance_job(&state.db, &job)?;
                            emit_custody_event(
                                state,
                                "rebalance.slippage_exceeded",
                                &job.job_id,
                                None,
                                job.swap_tx_hash.as_deref(),
                                Some(&serde_json::json!({
                                    "slippage_bps": slippage_bps,
                                    "max_slippage_bps": state.config.rebalance_max_slippage_bps,
                                    "input": job.amount,
                                    "output": output
                                })),
                            );
                            continue;
                        }
                    }
                    if output != job.amount {
                        info!(
                            "rebalance swap output differs from input: input={} output={} (job={})",
                            job.amount, output, job.job_id
                        );
                    }
                    output
                }
                None => {
                    tracing::warn!(
                        "could not parse swap output for job {}, marking unverified (NOT crediting assumed amount {})",
                        job.job_id,
                        job.amount
                    );
                    job.status = "unverified".to_string();
                    store_rebalance_job(&state.db, &job)?;
                    emit_custody_event(
                        state,
                        "rebalance.output_unverified",
                        &job.job_id,
                        None,
                        job.swap_tx_hash.as_deref(),
                        Some(&serde_json::json!({
                            "amount": job.amount,
                            "chain": job.chain
                        })),
                    );
                    continue;
                }
            };

            store_rebalance_job(&state.db, &job)?;

            adjust_reserve_balance(&state.db, &job.chain, &job.from_asset, job.amount, false)
                .await?;
            adjust_reserve_balance(&state.db, &job.chain, &job.to_asset, credit_amount, true)
                .await?;

            emit_custody_event(
                state,
                "rebalance.confirmed",
                &job.job_id,
                None,
                job.swap_tx_hash.as_deref(),
                Some(&serde_json::json!({
                    "chain": job.chain,
                    "from_asset": job.from_asset,
                    "to_asset": job.to_asset,
                    "amount": job.amount,
                    "credit_amount": credit_amount
                })),
            );
            info!(
                "rebalance confirmed: {} {} → {} on {} (job={})",
                job.amount, job.from_asset, job.to_asset, job.chain, job.job_id
            );
        }
    }

    Ok(())
}

pub(super) async fn execute_rebalance_swap(
    state: &CustodyState,
    job: &RebalanceJob,
) -> Result<String, String> {
    match job.chain.as_str() {
        "solana" => execute_solana_rebalance_swap(state, job).await,
        "ethereum" => execute_ethereum_rebalance_swap(state, job).await,
        other => Err(format!("unsupported rebalance chain: {}", other)),
    }
}
