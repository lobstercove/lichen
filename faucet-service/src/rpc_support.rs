use base64::{engine::general_purpose::STANDARD, Engine as _};
use lichen_core::{Hash, Instruction, Keypair, Message, Pubkey, Transaction, SYSTEM_PROGRAM_ID};
use serde_json::{json, Value};

use super::models::{FaucetState, TreasuryInfo};

pub(super) async fn fetch_treasury_info(state: &FaucetState) -> Result<TreasuryInfo, String> {
    let value = rpc_call(state, "getTreasuryInfo", json!([])).await?;
    serde_json::from_value(value).map_err(|err| format!("invalid treasury response: {}", err))
}

pub(super) async fn rpc_call(
    state: &FaucetState,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });

    let response = state
        .http
        .post(&state.config.rpc_url)
        .json(&payload)
        .send()
        .await
        .map_err(|err| format!("rpc request failed: {}", err))?;

    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|err| format!("invalid rpc response: {}", err))?;

    if !status.is_success() {
        return Err(format!("rpc http error {}", status));
    }

    if let Some(error) = body.get("error") {
        let message = error
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown rpc error");
        return Err(message.to_string());
    }

    body.get("result")
        .cloned()
        .ok_or_else(|| "rpc response missing result".to_string())
}

pub(super) async fn submit_faucet_transfer(
    state: &FaucetState,
    recipient: Pubkey,
    amount_spores: u64,
) -> Result<String, String> {
    let faucet_keypair = state.faucet_keypair.as_ref().ok_or_else(|| {
        "Faucet keypair not configured - cannot sign airdrop transactions".to_string()
    })?;
    let blockhash = fetch_recent_blockhash(state).await?;
    let chain_id = fetch_signing_chain_id(state).await?;
    let tx = build_faucet_transfer_transaction(
        faucet_keypair,
        recipient,
        amount_spores,
        blockhash,
        &chain_id,
    );
    let tx_base64 = STANDARD.encode(tx.to_wire());
    let result = rpc_call(state, "sendTransaction", json!([tx_base64])).await?;
    result
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            result
                .get("signature")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .ok_or_else(|| format!("invalid sendTransaction response: {result}"))
}

async fn fetch_recent_blockhash(state: &FaucetState) -> Result<Hash, String> {
    let result = rpc_call(state, "getRecentBlockhash", json!([])).await?;
    let hash = result
        .as_str()
        .or_else(|| result.get("blockhash").and_then(|value| value.as_str()))
        .ok_or_else(|| format!("invalid getRecentBlockhash response: {result}"))?;
    Hash::from_hex(hash).map_err(|err| format!("invalid recent blockhash: {err}"))
}

async fn fetch_signing_chain_id(state: &FaucetState) -> Result<String, String> {
    let result = rpc_call(state, "getNetworkInfo", json!([])).await?;
    result
        .get("chain_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("invalid getNetworkInfo response: {result}"))
}

pub(super) fn build_faucet_transfer_transaction(
    faucet_keypair: &Keypair,
    recipient: Pubkey,
    amount_spores: u64,
    recent_blockhash: Hash,
    chain_id: &str,
) -> Transaction {
    let mut data = vec![0u8];
    data.extend_from_slice(&amount_spores.to_le_bytes());
    let instruction = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![faucet_keypair.pubkey(), recipient],
        data,
    };
    let message = Message {
        instructions: vec![instruction],
        recent_blockhash,
        compute_budget: None,
        compute_unit_price: None,
    };
    let signature = faucet_keypair.sign(&message.signing_bytes_for_chain_id(chain_id));
    Transaction {
        signatures: vec![signature],
        message,
        tx_type: Default::default(),
    }
}
