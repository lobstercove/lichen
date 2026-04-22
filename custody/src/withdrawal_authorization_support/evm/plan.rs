use serde_json::{json, Value};

use super::super::*;
use super::abi::{
    build_evm_safe_exec_transaction_calldata, build_evm_safe_get_transaction_hash_calldata,
    evm_function_selector,
};

#[derive(Debug, Clone)]
pub(crate) struct EvmSafeTransactionPlan {
    pub(crate) safe_address: String,
    pub(crate) nonce: u64,
    pub(crate) inner_to: String,
    pub(crate) inner_value: u128,
    pub(crate) inner_data: Vec<u8>,
    pub(crate) safe_tx_hash: [u8; 32],
    pub(crate) exec_calldata: Vec<u8>,
}

pub(crate) fn evm_executor_derivation_path(dest_chain: &str) -> &'static str {
    match dest_chain {
        "bsc" | "bnb" => "custody/treasury/bnb",
        _ => "custody/treasury/ethereum",
    }
}

async fn evm_call(
    client: &reqwest::Client,
    url: &str,
    to: &str,
    data: &[u8],
) -> Result<Value, String> {
    evm_rpc_call(
        client,
        url,
        "eth_call",
        json!([{
            "to": to,
            "data": format!("0x{}", hex::encode(data)),
        }, "latest"]),
    )
    .await
}

async fn evm_safe_get_nonce(
    client: &reqwest::Client,
    url: &str,
    safe_address: &str,
) -> Result<u64, String> {
    let selector = evm_function_selector("nonce()");
    let result = evm_call(client, url, safe_address, &selector).await?;
    let value = result.as_str().unwrap_or("0x0");
    parse_hex_u64(value)
}

fn build_evm_threshold_withdrawal_intent(
    state: &CustodyState,
    job: &WithdrawalJob,
    asset: &str,
) -> Result<(String, u128, Vec<u8>), String> {
    let is_erc20 = matches!(asset, "usdt" | "usdc");
    if is_erc20 {
        let contract_addr = evm_contract_for_asset(&state.config, asset)
            .map_err(|error| format!("resolve ERC-20 contract for withdrawal: {}", error))?;
        let chain_amount = spores_to_chain_amount(job.amount, &job.dest_chain, asset);
        let transfer_data = evm_encode_erc20_transfer(&job.dest_address, chain_amount)
            .map_err(|error| format!("encode ERC-20 transfer: {}", error))?;
        Ok((contract_addr, 0u128, transfer_data))
    } else {
        let chain_amount = spores_to_chain_amount(job.amount, &job.dest_chain, asset);
        Ok((job.dest_address.clone(), chain_amount, Vec::new()))
    }
}

pub(crate) async fn build_evm_safe_transaction_plan(
    state: &CustodyState,
    url: &str,
    job: &WithdrawalJob,
    asset: &str,
) -> Result<EvmSafeTransactionPlan, String> {
    let safe_address = state.config.evm_multisig_address.clone().ok_or_else(|| {
        "EVM multisig address not configured (set CUSTODY_EVM_MULTISIG_ADDRESS)".to_string()
    })?;
    let nonce = match job.safe_nonce {
        Some(nonce) => nonce,
        None => evm_safe_get_nonce(&state.http, url, &safe_address).await?,
    };
    let (inner_to, inner_value, inner_data) =
        build_evm_threshold_withdrawal_intent(state, job, asset)?;
    let hash_calldata =
        build_evm_safe_get_transaction_hash_calldata(&inner_to, inner_value, &inner_data, nonce)?;
    let hash_result = evm_call(&state.http, url, &safe_address, &hash_calldata).await?;
    let hash_hex = hash_result
        .as_str()
        .ok_or_else(|| "Safe getTransactionHash returned non-string result".to_string())?;
    let hash_bytes = hex::decode(hash_hex.trim_start_matches("0x"))
        .map_err(|error| format!("decode Safe tx hash: {}", error))?;
    if hash_bytes.len() != 32 {
        return Err(format!(
            "invalid Safe tx hash length: expected 32, got {}",
            hash_bytes.len()
        ));
    }

    let mut safe_tx_hash = [0u8; 32];
    safe_tx_hash.copy_from_slice(&hash_bytes);

    Ok(EvmSafeTransactionPlan {
        safe_address,
        nonce,
        inner_to,
        inner_value,
        inner_data,
        safe_tx_hash,
        exec_calldata: Vec::new(),
    })
}

pub(crate) fn finalize_evm_safe_exec_plan(
    mut plan: EvmSafeTransactionPlan,
    signatures: &[u8],
) -> Result<EvmSafeTransactionPlan, String> {
    plan.exec_calldata = build_evm_safe_exec_transaction_calldata(
        &plan.inner_to,
        plan.inner_value,
        &plan.inner_data,
        signatures,
    )?;
    Ok(plan)
}
