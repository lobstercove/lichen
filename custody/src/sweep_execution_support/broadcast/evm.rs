use super::*;

pub(super) async fn broadcast_evm_sweep(
    state: &CustodyState,
    url: &str,
    job: &SweepJob,
) -> Result<Option<String>, String> {
    if matches!(job.asset.as_str(), "usdc" | "usdt") {
        return broadcast_evm_token_sweep(state, url, job).await;
    }

    let amount = match job.amount.as_ref() {
        Some(value) => value
            .parse::<u128>()
            .map_err(|_| "invalid amount".to_string())?,
        None => return Ok(None),
    };

    let deposit = fetch_deposit(&state.db, &job.deposit_id)?;
    let Some(deposit) = deposit else {
        return Ok(None);
    };

    let from_address = deposit.address.clone();
    let to_address = job.to_treasury.clone();

    let nonce = evm_get_transaction_count(&state.http, url, &from_address).await?;
    let gas_price = evm_get_gas_price(&state.http, url).await?;
    let gas_limit = evm_estimate_gas(
        &state.http,
        url,
        &from_address,
        &to_address,
        amount,
        None,
        21_000,
    )
    .await;
    let fee = gas_price.saturating_mul(gas_limit);
    if amount <= fee {
        return Ok(None);
    }
    let value = amount - fee;

    let chain_id = evm_get_chain_id(&state.http, url).await?;
    let signing_key = derive_evm_signing_key(
        &deposit.derivation_path,
        deposit_seed_for_record(&state.config, &deposit),
    )?;
    let raw_tx = build_evm_signed_transaction(
        &signing_key,
        nonce,
        gas_price,
        gas_limit,
        &to_address,
        value,
        chain_id,
    )?;
    let tx_hex = format!("0x{}", hex::encode(raw_tx));

    let result = evm_rpc_call(&state.http, url, "eth_sendRawTransaction", json!([tx_hex])).await?;
    Ok(result.as_str().map(|value| value.to_string()))
}

async fn broadcast_evm_token_sweep(
    state: &CustodyState,
    url: &str,
    job: &SweepJob,
) -> Result<Option<String>, String> {
    let amount = match job.amount.as_ref() {
        Some(value) => value
            .parse::<u128>()
            .map_err(|_| "invalid amount".to_string())?,
        None => return Ok(None),
    };
    if amount == 0 {
        return Ok(None);
    }

    let deposit = fetch_deposit(&state.db, &job.deposit_id)?;
    let Some(deposit) = deposit else {
        return Ok(None);
    };

    let contract = evm_contract_for_asset(&state.config, &job.asset)?;
    let from_address = deposit.address.clone();
    let to_address = job.to_treasury.clone();

    let transfer_data = evm_encode_erc20_transfer(&to_address, amount)?;
    let gas_price = evm_get_gas_price(&state.http, url).await?;
    let gas_limit = evm_estimate_gas(
        &state.http,
        url,
        &from_address,
        &contract,
        0,
        Some(&transfer_data),
        100_000,
    )
    .await;
    let fee = gas_price.saturating_mul(gas_limit);
    let native_balance = evm_get_balance(&state.http, url, &from_address).await?;

    if native_balance < fee {
        let deficit = fee.saturating_sub(native_balance);
        let gas_grant = deficit.saturating_add(deficit / 5);

        info!(
            "M16 gas funding: deposit {} has {} wei, needs {} — granting {} wei from treasury",
            from_address, native_balance, fee, gas_grant
        );

        let fund_tx_hash =
            fund_evm_gas_for_sweep(state, url, &job.chain, &from_address, gas_grant).await?;
        info!(
            "M16 gas funding tx submitted: {} → {} ({})",
            fund_tx_hash, from_address, gas_grant
        );

        let mut confirmed = false;
        for attempt in 0..18 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            match check_evm_tx_confirmed(&state.http, url, &fund_tx_hash, 1).await {
                Ok(true) => {
                    confirmed = true;
                    break;
                }
                Ok(false) => {
                    if attempt % 6 == 5 {
                        tracing::debug!(
                            "M16 gas funding waiting for confirmation ({}/18)...",
                            attempt + 1
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!("M16 gas funding confirmation check error: {}", error);
                }
            }
        }
        if !confirmed {
            return Err(format!(
                "gas funding tx {} did not confirm within 90s",
                fund_tx_hash
            ));
        }

        let new_balance = evm_get_balance(&state.http, url, &from_address).await?;
        if new_balance < fee {
            return Err(format!(
                "gas funding confirmed but balance still insufficient: {} < {}",
                new_balance, fee
            ));
        }
    }

    let nonce = evm_get_transaction_count(&state.http, url, &from_address).await?;
    let chain_id = evm_get_chain_id(&state.http, url).await?;
    let signing_key = derive_evm_signing_key(
        &deposit.derivation_path,
        deposit_seed_for_record(&state.config, &deposit),
    )?;
    let raw_tx = build_evm_signed_transaction_with_data(
        &signing_key,
        nonce,
        gas_price,
        gas_limit,
        &contract,
        0,
        &transfer_data,
        chain_id,
    )?;
    let tx_hex = format!("0x{}", hex::encode(raw_tx));

    let result = evm_rpc_call(&state.http, url, "eth_sendRawTransaction", json!([tx_hex])).await?;
    Ok(result.as_str().map(|value| value.to_string()))
}

async fn fund_evm_gas_for_sweep(
    state: &CustodyState,
    url: &str,
    chain: &str,
    to_address: &str,
    amount_wei: u128,
) -> Result<String, String> {
    let treasury_chain = evm_treasury_derivation_path(chain)
        .ok_or_else(|| format!("unsupported EVM sweep chain: {}", chain))?;

    let treasury_addr = derive_evm_address(treasury_chain, &state.config.master_seed)?;

    let nonce = evm_get_transaction_count(&state.http, url, &treasury_addr).await?;
    let gas_price = evm_get_gas_price(&state.http, url).await?;
    let chain_id = evm_get_chain_id(&state.http, url).await?;
    let signing_key = derive_evm_signing_key(treasury_chain, &state.config.master_seed)?;

    let gas_limit = evm_estimate_gas(
        &state.http,
        url,
        &treasury_addr,
        to_address,
        amount_wei,
        None,
        21_000,
    )
    .await;
    let tx_fee = gas_price.saturating_mul(gas_limit);

    let treasury_balance = evm_get_balance(&state.http, url, &treasury_addr).await?;
    if treasury_balance < amount_wei.saturating_add(tx_fee) {
        return Err(format!(
            "treasury ETH balance too low for gas grant: has {} wei, needs {} + {} fee",
            treasury_balance, amount_wei, tx_fee
        ));
    }

    let raw_tx = build_evm_signed_transaction(
        &signing_key,
        nonce,
        gas_price,
        gas_limit,
        to_address,
        amount_wei,
        chain_id,
    )?;
    let tx_hex = format!("0x{}", hex::encode(raw_tx));
    let result = evm_rpc_call(&state.http, url, "eth_sendRawTransaction", json!([tx_hex])).await?;

    result
        .as_str()
        .map(|value| value.to_string())
        .ok_or_else(|| "no tx hash from gas funding".to_string())
}
