use super::*;

pub(super) async fn execute_solana_rebalance_swap(
    state: &CustodyState,
    job: &RebalanceJob,
) -> Result<String, String> {
    let jupiter_url = state
        .config
        .jupiter_api_url
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_JUPITER_API_URL for Solana rebalance".to_string())?;
    let solana_url = state
        .config
        .solana_rpc_url
        .as_ref()
        .ok_or_else(|| "missing solana RPC for rebalance".to_string())?;
    let treasury_addr = state
        .config
        .treasury_solana_address
        .as_ref()
        .ok_or_else(|| "missing treasury solana address".to_string())?;

    let from_mint = match job.from_asset.as_str() {
        "usdt" => &state.config.solana_usdt_mint,
        "usdc" => &state.config.solana_usdc_mint,
        _ => return Err(format!("unsupported from_asset: {}", job.from_asset)),
    };
    let to_mint = match job.to_asset.as_str() {
        "usdt" => &state.config.solana_usdt_mint,
        "usdc" => &state.config.solana_usdc_mint,
        _ => return Err(format!("unsupported to_asset: {}", job.to_asset)),
    };

    let quote_url = format!(
        "{}/quote?inputMint={}&outputMint={}&amount={}&slippageBps={}",
        jupiter_url.trim_end_matches('/'),
        from_mint,
        to_mint,
        job.amount,
        state.config.rebalance_max_slippage_bps
    );
    let quote_resp = state
        .http
        .get(&quote_url)
        .send()
        .await
        .map_err(|error| format!("jupiter quote: {}", error))?;
    let quote: Value = quote_resp
        .json()
        .await
        .map_err(|error| format!("jupiter quote json: {}", error))?;

    let swap_url = format!("{}/swap", jupiter_url.trim_end_matches('/'));
    let swap_body = json!({
        "quoteResponse": quote,
        "userPublicKey": treasury_addr,
        "wrapAndUnwrapSol": false,
    });
    let swap_resp = state
        .http
        .post(&swap_url)
        .json(&swap_body)
        .send()
        .await
        .map_err(|error| format!("jupiter swap: {}", error))?;
    let swap_result: Value = swap_resp
        .json()
        .await
        .map_err(|error| format!("jupiter swap json: {}", error))?;

    let swap_tx_b64 = swap_result
        .get("swapTransaction")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "jupiter swap tx missing".to_string())?;

    let fee_payer_path = state
        .config
        .solana_fee_payer_keypair_path
        .as_ref()
        .ok_or_else(|| "missing fee payer for rebalance".to_string())?;
    let fee_payer = load_solana_keypair(fee_payer_path)?;

    let tx_bytes = base64::engine::general_purpose::STANDARD
        .decode(swap_tx_b64)
        .map_err(|error| format!("base64 decode jupiter tx: {}", error))?;

    if tx_bytes.is_empty() {
        return Err("empty jupiter transaction".to_string());
    }
    let (num_sigs, header_len) = decode_shortvec_u16(&tx_bytes)
        .ok_or_else(|| "invalid compact-u16 in jupiter tx".to_string())?;
    if num_sigs == 0 {
        return Err("jupiter tx has zero signatures".to_string());
    }
    let sigs_end = header_len + (num_sigs as usize) * 64;
    if sigs_end > tx_bytes.len() {
        return Err("jupiter tx too short for declared signatures".to_string());
    }
    let message_bytes = &tx_bytes[sigs_end..];
    let fee_payer_sig = fee_payer.sign(message_bytes);

    let mut signed_tx = tx_bytes.clone();
    signed_tx[header_len..header_len + 64].copy_from_slice(&fee_payer_sig);
    let signed_b64 = base64::engine::general_purpose::STANDARD.encode(&signed_tx);

    let params = json!([signed_b64, {"encoding": "base64", "skipPreflight": true}]);
    let result = solana_rpc_call(&state.http, solana_url, "sendTransaction", params).await?;
    result
        .as_str()
        .map(|value| value.to_string())
        .ok_or_else(|| "no tx hash from solana".to_string())
}
