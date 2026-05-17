use super::*;

pub(crate) fn build_credit_job(
    state: &CustodyState,
    sweep: &SweepJob,
) -> Result<Option<CreditJob>, String> {
    let amount_source =
        if sweep.chain.eq_ignore_ascii_case("solana") && sweep.asset.eq_ignore_ascii_case("sol") {
            sweep.credited_amount.as_ref().or(sweep.amount.as_ref())
        } else {
            sweep.amount.as_ref()
        };
    let raw_amount = match amount_source {
        Some(value) => value
            .parse::<u128>()
            .map_err(|_| "invalid amount".to_string())?,
        None => return Ok(None),
    };

    let deposit = fetch_deposit(&state.db, &sweep.deposit_id)?;
    let Some(deposit) = deposit else {
        return Ok(None);
    };

    if state.config.licn_rpc_url.is_none() || state.config.treasury_keypair_path.is_none() {
        tracing::warn!(
            "build_credit_job skipping: licn_rpc_url or treasury_keypair_path not configured"
        );
        return Ok(None);
    }

    if Pubkey::from_base58(&deposit.user_id).is_err() {
        return Ok(None);
    }

    let source_asset = deposit.asset.to_lowercase();
    let source_chain = deposit.chain.to_lowercase();
    let contract_addr = resolve_token_contract(&state.config, &source_chain, &source_asset);
    if contract_addr.is_none() {
        tracing::warn!(
            "no wrapped token contract configured for chain={} asset={}",
            source_chain,
            source_asset
        );
        return Ok(None);
    }

    let source_decimals: u32 = source_chain_decimals(&source_chain, &source_asset)?;
    let amount_spores: u64 = if source_decimals > 9 {
        let divisor = 10u128.pow(source_decimals - 9);
        if raw_amount % divisor != 0 {
            return Err(format!(
                "non-exact deposit decimal conversion rejected (raw={raw_amount}, div={divisor}, chain={source_chain}, asset={source_asset})"
            ));
        }
        u64::try_from(raw_amount / divisor).map_err(|_| {
            format!(
                "credit amount overflow after decimal conversion (raw={raw_amount}, div={divisor})"
            )
        })?
    } else if source_decimals < 9 {
        let multiplier = 10u128.pow(9 - source_decimals);
        u64::try_from(raw_amount.saturating_mul(multiplier)).map_err(|_| {
            format!(
                "credit amount overflow after decimal conversion (raw={raw_amount}, mul={multiplier})"
            )
        })?
    } else {
        u64::try_from(raw_amount)
            .map_err(|_| format!("credit amount overflow (raw={raw_amount})"))?
    };
    if amount_spores == 0 {
        tracing::warn!(
            "converted amount is 0 spores (raw={}, chain={}, asset={}, source_dec={}), skipping credit",
            raw_amount,
            source_chain,
            source_asset,
            source_decimals
        );
        return Ok(None);
    }

    Ok(Some(CreditJob {
        job_id: format!("credit:{}", sweep.job_id),
        deposit_id: sweep.deposit_id.clone(),
        to_address: deposit.user_id,
        amount_spores,
        source_asset,
        source_chain,
        status: "queued".to_string(),
        tx_signature: None,
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        created_at: chrono::Utc::now().timestamp(),
    }))
}
