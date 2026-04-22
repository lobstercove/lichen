use super::*;

pub(super) async fn execute_ethereum_rebalance_swap(
    state: &CustodyState,
    job: &RebalanceJob,
) -> Result<String, String> {
    let router = state
        .config
        .uniswap_router
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_UNISWAP_ROUTER for Ethereum rebalance".to_string())?;
    let evm_url = state
        .config
        .evm_rpc_url
        .as_ref()
        .ok_or_else(|| "missing EVM RPC for rebalance".to_string())?;
    let treasury_addr = state
        .config
        .treasury_evm_address
        .as_ref()
        .ok_or_else(|| "missing treasury EVM address".to_string())?;

    let from_contract = match job.from_asset.as_str() {
        "usdt" => &state.config.evm_usdt_contract,
        "usdc" => &state.config.evm_usdc_contract,
        _ => return Err(format!("unsupported from_asset: {}", job.from_asset)),
    };
    let to_contract = match job.to_asset.as_str() {
        "usdt" => &state.config.evm_usdt_contract,
        "usdc" => &state.config.evm_usdc_contract,
        _ => return Err(format!("unsupported to_asset: {}", job.to_asset)),
    };

    let nonce = evm_get_transaction_count(&state.http, evm_url, treasury_addr).await?;
    let gas_price = evm_get_gas_price(&state.http, evm_url).await?;
    let chain_id = evm_get_chain_id(&state.http, evm_url).await?;

    let approve_data = evm_encode_erc20_approve(router, job.amount as u128)?;
    let signing_key = derive_evm_signing_key("custody-treasury-evm", &state.config.master_seed)?;
    let approve_tx = build_evm_signed_transaction_with_data(
        &signing_key,
        nonce,
        gas_price,
        100_000u128,
        from_contract,
        0,
        &approve_data,
        chain_id,
    )?;
    let approve_hex = format!("0x{}", hex::encode(&approve_tx));
    let approve_result = evm_rpc_call(
        &state.http,
        evm_url,
        "eth_sendRawTransaction",
        json!([approve_hex]),
    )
    .await?;

    let approve_tx_hash = approve_result
        .as_str()
        .ok_or_else(|| "no tx hash from approve".to_string())?;

    let mut confirmed = false;
    for _ in 0..36 {
        match check_evm_tx_confirmed(&state.http, evm_url, approve_tx_hash, 1).await {
            Ok(true) => {
                confirmed = true;
                break;
            }
            Ok(false) => {}
            Err(_) => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    }
    if !confirmed {
        return Err(format!(
            "ERC-20 approve tx {} not confirmed after 90s — aborting swap",
            approve_tx_hash
        ));
    }

    let swap_data = build_uniswap_exact_input_single(
        from_contract,
        to_contract,
        job.amount as u128,
        100,
        state.config.rebalance_max_slippage_bps,
        treasury_addr,
    )?;
    let swap_tx = build_evm_signed_transaction_with_data(
        &signing_key,
        nonce + 1,
        gas_price,
        300_000u128,
        router,
        0,
        &swap_data,
        chain_id,
    )?;
    let swap_hex = format!("0x{}", hex::encode(&swap_tx));
    let result = evm_rpc_call(
        &state.http,
        evm_url,
        "eth_sendRawTransaction",
        json!([swap_hex]),
    )
    .await?;
    result
        .as_str()
        .map(|value| value.to_string())
        .ok_or_else(|| "no tx hash from ethereum".to_string())
}

fn evm_encode_erc20_approve(spender: &str, amount: u128) -> Result<Vec<u8>, String> {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&hex::decode("095ea7b3").map_err(|_| "selector".to_string())?);

    let spender_bytes = parse_evm_address(spender)?;
    let mut padded_spender = vec![0u8; 12];
    padded_spender.extend_from_slice(&spender_bytes);
    data.extend_from_slice(&padded_spender);

    let mut padded_amount = vec![0u8; 16];
    padded_amount.extend_from_slice(&amount.to_be_bytes());
    data.extend_from_slice(&padded_amount);

    Ok(data)
}

fn build_uniswap_exact_input_single(
    token_in: &str,
    token_out: &str,
    amount_in: u128,
    fee: u32,
    max_slippage_bps: u64,
    recipient: &str,
) -> Result<Vec<u8>, String> {
    let mut data = Vec::with_capacity(228);
    data.extend_from_slice(&hex::decode("414bf389").map_err(|_| "selector".to_string())?);

    let token_in_bytes = parse_evm_address(token_in)?;
    let mut padded = vec![0u8; 12];
    padded.extend_from_slice(&token_in_bytes);
    data.extend_from_slice(&padded);

    let token_out_bytes = parse_evm_address(token_out)?;
    let mut padded = vec![0u8; 12];
    padded.extend_from_slice(&token_out_bytes);
    data.extend_from_slice(&padded);

    let mut fee_padded = vec![0u8; 28];
    fee_padded.extend_from_slice(&fee.to_be_bytes());
    data.extend_from_slice(&fee_padded);

    let recipient_bytes = parse_evm_address(recipient)?;
    let mut padded_recipient = vec![0u8; 12];
    padded_recipient.extend_from_slice(&recipient_bytes);
    data.extend_from_slice(&padded_recipient);

    let mut deadline = vec![0u8; 24];
    deadline.extend_from_slice(&u64::MAX.to_be_bytes());
    data.extend_from_slice(&deadline);

    let mut amount_padded = vec![0u8; 16];
    amount_padded.extend_from_slice(&amount_in.to_be_bytes());
    data.extend_from_slice(&amount_padded);

    let min_out = amount_in * (10_000u128 - max_slippage_bps as u128) / 10_000u128;
    let mut min_padded = vec![0u8; 16];
    min_padded.extend_from_slice(&min_out.to_be_bytes());
    data.extend_from_slice(&min_padded);

    data.extend_from_slice(&[0u8; 32]);

    Ok(data)
}
