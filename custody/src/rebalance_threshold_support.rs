use super::*;

pub(super) fn check_rebalance_thresholds(state: &CustodyState) -> Result<(), String> {
    let threshold = state.config.rebalance_threshold_bps;
    let target = state.config.rebalance_target_bps;

    for chain in &["solana", "ethereum", "bsc"] {
        let usdt = get_reserve_balance(&state.db, chain, "usdt").unwrap_or(0);
        let usdc = get_reserve_balance(&state.db, chain, "usdc").unwrap_or(0);
        let total = usdt.saturating_add(usdc);
        if total == 0 {
            continue;
        }

        let usdt_bps = (usdt as u128 * 10_000 / total as u128) as u64;

        if usdt_bps > threshold {
            let target_usdt = (total as u128 * target as u128 / 10_000) as u64;
            let swap_amount = usdt.saturating_sub(target_usdt);
            if swap_amount > 0 {
                create_threshold_rebalance(&state.db, chain, "usdt", "usdc", swap_amount)?;
            }
        } else if (10_000 - usdt_bps) > threshold {
            let target_usdc = (total as u128 * (10_000 - target) as u128 / 10_000) as u64;
            let swap_amount = usdc.saturating_sub(target_usdc);
            if swap_amount > 0 {
                create_threshold_rebalance(&state.db, chain, "usdc", "usdt", swap_amount)?;
            }
        }
    }

    Ok(())
}

fn create_threshold_rebalance(
    db: &DB,
    chain: &str,
    from: &str,
    to: &str,
    amount: u64,
) -> Result<(), String> {
    let existing = list_rebalance_jobs_by_status(db, "queued")?;
    for job in &existing {
        if job.chain == chain && job.from_asset == from && job.trigger == "threshold" {
            return Ok(());
        }
    }

    let job = RebalanceJob {
        job_id: Uuid::new_v4().to_string(),
        chain: chain.to_string(),
        from_asset: from.to_string(),
        to_asset: to.to_string(),
        amount,
        trigger: "threshold".to_string(),
        linked_withdrawal_job_id: None,
        swap_tx_hash: None,
        status: "queued".to_string(),
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        created_at: chrono::Utc::now().timestamp(),
    };

    info!(
        "auto-rebalance: {} {} → {} on {} (ratio threshold exceeded, job={})",
        amount, from, to, chain, job.job_id
    );

    store_rebalance_job(db, &job)
}
