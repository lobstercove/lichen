use hmac::{Hmac, Mac};
use k256::ecdsa::signature::hazmat::PrehashSigner;
use k256::ecdsa::SigningKey;
use ripemd::Ripemd160;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use super::*;

const BECH32_CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
const SIGHASH_ALL: u32 = 1;
const BTC_P2WPKH_INPUT_VBYTES: u64 = 68;
const BTC_P2WPKH_OUTPUT_VBYTES: u64 = 31;
const BTC_TX_OVERHEAD_VBYTES: u64 = 10;

#[derive(Clone, Debug)]
pub(crate) struct BitcoinUtxo {
    pub(crate) txid: String,
    pub(crate) vout: u32,
    pub(crate) amount_sats: u64,
    pub(crate) confirmations: u64,
}

pub(crate) fn is_bitcoin_chain(chain: &str) -> bool {
    matches!(
        chain.trim().to_ascii_lowercase().as_str(),
        "btc" | "bitcoin"
    )
}

pub(crate) fn normalize_bitcoin_network(value: &str) -> Result<&'static str, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "main" | "mainnet" | "bitcoin" => Ok("mainnet"),
        "test" | "testnet" | "signet" => Ok("testnet"),
        "regtest" | "local" => Ok("regtest"),
        other => Err(format!("unsupported bitcoin network: {other}")),
    }
}

fn bitcoin_hrp(network: &str) -> Result<&'static str, String> {
    match normalize_bitcoin_network(network)? {
        "mainnet" => Ok("bc"),
        "testnet" => Ok("tb"),
        "regtest" => Ok("bcrt"),
        _ => Err("unsupported bitcoin network".to_string()),
    }
}

pub(crate) fn derive_bitcoin_address(
    path: &str,
    master_seed: &str,
    network: &str,
) -> Result<String, String> {
    let signing_key = derive_bitcoin_signing_key(path, master_seed)?;
    let pubkey = compressed_public_key(&signing_key);
    let program = hash160(&pubkey);
    encode_segwit_address(bitcoin_hrp(network)?, 0, &program)
}

pub(crate) fn validate_bitcoin_address_for_network(
    address: &str,
    network: &str,
) -> Result<(), String> {
    let (hrp, version, program) = decode_segwit_address(address)?;
    let expected_hrp = bitcoin_hrp(network)?;
    if hrp != expected_hrp {
        return Err(format!(
            "bitcoin address network mismatch: expected {expected_hrp}1..., got {hrp}1..."
        ));
    }
    if version != 0 {
        return Err("only native SegWit v0 bitcoin addresses are supported".to_string());
    }
    if program.len() != 20 && program.len() != 32 {
        return Err("invalid SegWit v0 witness program length".to_string());
    }
    Ok(())
}

pub(crate) fn bitcoin_treasury_derivation_path() -> &'static str {
    "custody/treasury/bitcoin"
}

pub(crate) fn derive_bitcoin_treasury_address(config: &CustodyConfig) -> Result<String, String> {
    derive_bitcoin_address(
        bitcoin_treasury_derivation_path(),
        &config.master_seed,
        &config.btc_network,
    )
}

pub(crate) async fn bitcoin_scan_confirmed_utxos(
    client: &reqwest::Client,
    config: &CustodyConfig,
    address: &str,
    min_confirmations: u64,
) -> Result<Vec<BitcoinUtxo>, String> {
    validate_bitcoin_address_for_network(address, &config.btc_network)?;
    let tip = bitcoin_block_count(client, config).await?;
    let descriptor = format!("addr({address})");
    let result = bitcoin_rpc_call(
        client,
        config,
        "scantxoutset",
        json!(["start", [descriptor]]),
    )
    .await?;
    let unspents = result
        .get("unspents")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "bitcoin scantxoutset response missing unspents".to_string())?;

    let mut out = Vec::new();
    for entry in unspents {
        let txid = entry
            .get("txid")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "bitcoin UTXO missing txid".to_string())?
            .to_string();
        let vout = entry
            .get("vout")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| "bitcoin UTXO missing vout".to_string())?;
        let amount_sats = parse_btc_amount_sats(
            entry
                .get("amount")
                .ok_or_else(|| "bitcoin UTXO missing amount".to_string())?,
        )?;
        let height = entry
            .get("height")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let confirmations = if height == 0 || tip < height {
            0
        } else {
            tip.saturating_sub(height).saturating_add(1)
        };
        if confirmations < min_confirmations {
            continue;
        }
        out.push(BitcoinUtxo {
            txid,
            vout: u32::try_from(vout).map_err(|_| "bitcoin UTXO vout overflow".to_string())?,
            amount_sats,
            confirmations,
        });
    }
    Ok(out)
}

pub(crate) async fn bitcoin_send_raw_transaction(
    client: &reqwest::Client,
    config: &CustodyConfig,
    tx_hex: &str,
) -> Result<String, String> {
    bitcoin_rpc_call(client, config, "sendrawtransaction", json!([tx_hex]))
        .await?
        .as_str()
        .map(|value| value.to_string())
        .ok_or_else(|| "bitcoin sendrawtransaction returned no txid".to_string())
}

pub(crate) async fn bitcoin_tx_confirmed(
    client: &reqwest::Client,
    config: &CustodyConfig,
    txid: &str,
) -> Result<Option<bool>, String> {
    let result =
        match bitcoin_rpc_call(client, config, "getrawtransaction", json!([txid, true])).await {
            Ok(value) => value,
            Err(error) if error.contains("No such mempool or blockchain transaction") => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
    let confirmations = result
        .get("confirmations")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    if confirmations >= config.btc_confirmations.max(1) {
        Ok(Some(true))
    } else {
        Ok(None)
    }
}

pub(crate) fn build_bitcoin_sweep_tx_hex(
    derivation_path: &str,
    deposit_seed: &str,
    utxos: &[BitcoinUtxo],
    treasury_address: &str,
    network: &str,
    fee_rate_sats_vb: u64,
) -> Result<(String, u64), String> {
    if utxos.is_empty() {
        return Err("no bitcoin UTXOs to sweep".to_string());
    }
    validate_bitcoin_address_for_network(treasury_address, network)?;
    let total_in: u64 = utxos
        .iter()
        .try_fold(0u64, |acc, utxo| acc.checked_add(utxo.amount_sats))
        .ok_or_else(|| "bitcoin sweep input overflow".to_string())?;
    let vbytes = BTC_TX_OVERHEAD_VBYTES
        .saturating_add(BTC_P2WPKH_INPUT_VBYTES.saturating_mul(utxos.len() as u64))
        .saturating_add(BTC_P2WPKH_OUTPUT_VBYTES);
    let fee = fee_rate_sats_vb.max(1).saturating_mul(vbytes);
    if total_in <= fee {
        return Err(format!(
            "bitcoin sweep amount too small after fee: total={total_in} fee={fee}"
        ));
    }
    let output_value = total_in - fee;
    let treasury_script = script_pubkey_for_segwit_address(treasury_address, network)?;

    let signing_key = derive_bitcoin_signing_key(derivation_path, deposit_seed)?;
    let public_key = compressed_public_key(&signing_key);
    let pubkey_hash = hash160(&public_key);
    let script_code = p2pkh_script_code(&pubkey_hash);
    let hash_prevouts = hash_prevouts(utxos)?;
    let hash_sequence = hash_sequence(utxos.len());
    let output = TxOutput {
        value: output_value,
        script_pub_key: treasury_script,
    };
    let hash_outputs = double_sha256(&serialize_output_items(std::slice::from_ref(&output)));

    let mut signatures = Vec::with_capacity(utxos.len());
    for (index, utxo) in utxos.iter().enumerate() {
        let sighash = segwit_v0_sighash(
            utxos,
            index,
            &script_code,
            utxo.amount_sats,
            &hash_prevouts,
            &hash_sequence,
            &hash_outputs,
        )?;
        let signature: k256::ecdsa::Signature = signing_key
            .sign_prehash(&sighash)
            .map_err(|_| "bitcoin ECDSA signing failed".to_string())?;
        let signature = signature.normalize_s().unwrap_or(signature);
        let mut der = signature.to_der().as_bytes().to_vec();
        der.push(SIGHASH_ALL as u8);
        signatures.push(der);
    }

    let tx = serialize_witness_transaction(utxos, &[output], &signatures, &public_key)?;
    Ok((hex::encode(tx), output_value))
}

pub(crate) struct BitcoinPaymentRequest<'a> {
    pub(crate) derivation_path: &'a str,
    pub(crate) master_seed: &'a str,
    pub(crate) available_utxos: &'a [BitcoinUtxo],
    pub(crate) dest_address: &'a str,
    pub(crate) change_address: &'a str,
    pub(crate) amount_sats: u64,
    pub(crate) network: &'a str,
    pub(crate) fee_rate_sats_vb: u64,
}

pub(crate) fn build_bitcoin_payment_tx_hex(
    request: BitcoinPaymentRequest<'_>,
) -> Result<(String, u64, u64), String> {
    let BitcoinPaymentRequest {
        derivation_path,
        master_seed,
        available_utxos,
        dest_address,
        change_address,
        amount_sats,
        network,
        fee_rate_sats_vb,
    } = request;
    if amount_sats == 0 {
        return Err("bitcoin payment amount must be > 0".to_string());
    }
    validate_bitcoin_address_for_network(dest_address, network)?;
    validate_bitcoin_address_for_network(change_address, network)?;

    let mut selected = Vec::new();
    let mut total_in = 0u64;
    for utxo in available_utxos {
        selected.push(utxo.clone());
        total_in = total_in
            .checked_add(utxo.amount_sats)
            .ok_or_else(|| "bitcoin payment input overflow".to_string())?;
        let two_output_fee = estimated_p2wpkh_fee(selected.len(), 2, fee_rate_sats_vb);
        if total_in >= amount_sats.saturating_add(two_output_fee) {
            break;
        }
    }
    if selected.is_empty() {
        return Err("no bitcoin treasury UTXOs available".to_string());
    }

    let mut output_count = 2usize;
    let mut fee = estimated_p2wpkh_fee(selected.len(), output_count, fee_rate_sats_vb);
    if total_in < amount_sats.saturating_add(fee) {
        return Err(format!(
            "insufficient bitcoin treasury balance: available={total_in} amount={amount_sats} fee={fee}"
        ));
    }
    let mut change = total_in - amount_sats - fee;
    if change > 0 && change < 546 {
        output_count = 1;
        fee = estimated_p2wpkh_fee(selected.len(), output_count, fee_rate_sats_vb)
            .saturating_add(change);
        change = 0;
    }

    let mut outputs = vec![TxOutput {
        value: amount_sats,
        script_pub_key: script_pubkey_for_segwit_address(dest_address, network)?,
    }];
    if change > 0 {
        outputs.push(TxOutput {
            value: change,
            script_pub_key: script_pubkey_for_segwit_address(change_address, network)?,
        });
    }

    let tx_hex = build_signed_transaction_hex(derivation_path, master_seed, &selected, &outputs)?;
    Ok((tx_hex, amount_sats, fee))
}

fn estimated_p2wpkh_fee(inputs: usize, outputs: usize, fee_rate_sats_vb: u64) -> u64 {
    fee_rate_sats_vb.max(1).saturating_mul(
        BTC_TX_OVERHEAD_VBYTES
            .saturating_add(BTC_P2WPKH_INPUT_VBYTES.saturating_mul(inputs as u64))
            .saturating_add(BTC_P2WPKH_OUTPUT_VBYTES.saturating_mul(outputs as u64)),
    )
}

fn build_signed_transaction_hex(
    derivation_path: &str,
    seed: &str,
    utxos: &[BitcoinUtxo],
    outputs: &[TxOutput],
) -> Result<String, String> {
    let signing_key = derive_bitcoin_signing_key(derivation_path, seed)?;
    let public_key = compressed_public_key(&signing_key);
    let pubkey_hash = hash160(&public_key);
    let script_code = p2pkh_script_code(&pubkey_hash);
    let hash_prevouts = hash_prevouts(utxos)?;
    let hash_sequence = hash_sequence(utxos.len());
    let hash_outputs = double_sha256(&serialize_output_items(outputs));

    let mut signatures = Vec::with_capacity(utxos.len());
    for (index, utxo) in utxos.iter().enumerate() {
        let sighash = segwit_v0_sighash(
            utxos,
            index,
            &script_code,
            utxo.amount_sats,
            &hash_prevouts,
            &hash_sequence,
            &hash_outputs,
        )?;
        let signature: k256::ecdsa::Signature = signing_key
            .sign_prehash(&sighash)
            .map_err(|_| "bitcoin ECDSA signing failed".to_string())?;
        let signature = signature.normalize_s().unwrap_or(signature);
        let mut der = signature.to_der().as_bytes().to_vec();
        der.push(SIGHASH_ALL as u8);
        signatures.push(der);
    }

    let tx = serialize_witness_transaction(utxos, outputs, &signatures, &public_key)?;
    Ok(hex::encode(tx))
}

async fn bitcoin_block_count(
    client: &reqwest::Client,
    config: &CustodyConfig,
) -> Result<u64, String> {
    bitcoin_rpc_call(client, config, "getblockcount", json!([]))
        .await?
        .as_u64()
        .ok_or_else(|| "bitcoin getblockcount returned non-u64".to_string())
}

async fn bitcoin_rpc_call(
    client: &reqwest::Client,
    config: &CustodyConfig,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let url = config
        .btc_rpc_url
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_BTC_RPC_URL".to_string())?;
    let mut request = client.post(url).json(&json!({
        "jsonrpc": "1.0",
        "id": "lichen-custody",
        "method": method,
        "params": params,
    }));
    if let Some(user) = config
        .btc_rpc_user
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        request = request.basic_auth(user, config.btc_rpc_password.as_ref());
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("bitcoin RPC {method} transport: {error}"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("bitcoin RPC {method} decode: {error}"))?;
    if !status.is_success() {
        return Err(format!("bitcoin RPC {method} HTTP {status}: {body}"));
    }
    if !body.get("error").unwrap_or(&Value::Null).is_null() {
        return Err(format!("bitcoin RPC {method} error: {}", body["error"]));
    }
    Ok(body.get("result").cloned().unwrap_or(Value::Null))
}

fn derive_bitcoin_signing_key(path: &str, master_seed: &str) -> Result<SigningKey, String> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(master_seed.as_bytes()).map_err(|_| "HMAC key error")?;
    mac.update(path.as_bytes());
    let mut seed = mac.finalize().into_bytes();
    let result = SigningKey::from_bytes(&seed).map_err(|_| "invalid bitcoin seed".to_string());
    seed.as_mut_slice().zeroize();
    result
}

fn compressed_public_key(signing_key: &SigningKey) -> Vec<u8> {
    signing_key
        .verifying_key()
        .to_encoded_point(true)
        .as_bytes()
        .to_vec()
}

fn hash160(data: &[u8]) -> Vec<u8> {
    let sha = Sha256::digest(data);
    Ripemd160::digest(sha).to_vec()
}

fn double_sha256(data: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(data);
    let second = Sha256::digest(first);
    let mut out = [0u8; 32];
    out.copy_from_slice(&second);
    out
}

fn parse_btc_amount_sats(value: &Value) -> Result<u64, String> {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Number(_) => value.to_string(),
        _ => return Err("bitcoin amount is not a decimal".to_string()),
    };
    let mut parts = text.split('.');
    let whole = parts.next().unwrap_or("0");
    let frac = parts.next().unwrap_or("");
    if parts.next().is_some() || whole.starts_with('-') {
        return Err(format!("invalid bitcoin amount: {text}"));
    }
    let whole_sats = whole
        .parse::<u64>()
        .map_err(|_| format!("invalid bitcoin amount: {text}"))?
        .checked_mul(100_000_000)
        .ok_or_else(|| "bitcoin amount overflow".to_string())?;
    let mut frac_buf = frac.to_string();
    if frac_buf.len() > 8 {
        return Err(format!("bitcoin amount has >8 decimals: {text}"));
    }
    while frac_buf.len() < 8 {
        frac_buf.push('0');
    }
    let frac_sats = if frac_buf.is_empty() {
        0
    } else {
        frac_buf
            .parse::<u64>()
            .map_err(|_| format!("invalid bitcoin amount: {text}"))?
    };
    whole_sats
        .checked_add(frac_sats)
        .ok_or_else(|| "bitcoin amount overflow".to_string())
}

#[derive(Clone)]
struct TxOutput {
    value: u64,
    script_pub_key: Vec<u8>,
}

fn hash_prevouts(utxos: &[BitcoinUtxo]) -> Result<[u8; 32], String> {
    let mut data = Vec::with_capacity(36 * utxos.len());
    for utxo in utxos {
        data.extend_from_slice(&txid_little_endian(&utxo.txid)?);
        data.extend_from_slice(&utxo.vout.to_le_bytes());
    }
    Ok(double_sha256(&data))
}

fn hash_sequence(input_count: usize) -> [u8; 32] {
    let mut data = Vec::with_capacity(4 * input_count);
    for _ in 0..input_count {
        data.extend_from_slice(&0xffff_fffeu32.to_le_bytes());
    }
    double_sha256(&data)
}

fn segwit_v0_sighash(
    utxos: &[BitcoinUtxo],
    input_index: usize,
    script_code: &[u8],
    amount: u64,
    hash_prevouts: &[u8; 32],
    hash_sequence: &[u8; 32],
    hash_outputs: &[u8; 32],
) -> Result<[u8; 32], String> {
    let utxo = utxos
        .get(input_index)
        .ok_or_else(|| "bitcoin sighash input index out of range".to_string())?;
    let mut preimage = Vec::new();
    preimage.extend_from_slice(&2i32.to_le_bytes());
    preimage.extend_from_slice(hash_prevouts);
    preimage.extend_from_slice(hash_sequence);
    preimage.extend_from_slice(&txid_little_endian(&utxo.txid)?);
    preimage.extend_from_slice(&utxo.vout.to_le_bytes());
    push_varint(script_code.len() as u64, &mut preimage);
    preimage.extend_from_slice(script_code);
    preimage.extend_from_slice(&amount.to_le_bytes());
    preimage.extend_from_slice(&0xffff_fffeu32.to_le_bytes());
    preimage.extend_from_slice(hash_outputs);
    preimage.extend_from_slice(&0u32.to_le_bytes());
    preimage.extend_from_slice(&SIGHASH_ALL.to_le_bytes());
    Ok(double_sha256(&preimage))
}

fn serialize_witness_transaction(
    utxos: &[BitcoinUtxo],
    outputs: &[TxOutput],
    signatures: &[Vec<u8>],
    public_key: &[u8],
) -> Result<Vec<u8>, String> {
    if signatures.len() != utxos.len() {
        return Err("bitcoin witness signature count mismatch".to_string());
    }
    let mut out = Vec::new();
    out.extend_from_slice(&2i32.to_le_bytes());
    out.push(0x00);
    out.push(0x01);
    push_varint(utxos.len() as u64, &mut out);
    for utxo in utxos {
        out.extend_from_slice(&txid_little_endian(&utxo.txid)?);
        out.extend_from_slice(&utxo.vout.to_le_bytes());
        out.push(0x00);
        out.extend_from_slice(&0xffff_fffeu32.to_le_bytes());
    }
    out.extend_from_slice(&serialize_outputs(outputs));
    for signature in signatures {
        out.push(0x02);
        push_varint(signature.len() as u64, &mut out);
        out.extend_from_slice(signature);
        push_varint(public_key.len() as u64, &mut out);
        out.extend_from_slice(public_key);
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    Ok(out)
}

fn serialize_outputs(outputs: &[TxOutput]) -> Vec<u8> {
    let mut out = Vec::new();
    push_varint(outputs.len() as u64, &mut out);
    out.extend_from_slice(&serialize_output_items(outputs));
    out
}

fn serialize_output_items(outputs: &[TxOutput]) -> Vec<u8> {
    let mut out = Vec::new();
    for output in outputs {
        out.extend_from_slice(&output.value.to_le_bytes());
        push_varint(output.script_pub_key.len() as u64, &mut out);
        out.extend_from_slice(&output.script_pub_key);
    }
    out
}

fn txid_little_endian(txid: &str) -> Result<Vec<u8>, String> {
    let mut bytes = hex::decode(txid).map_err(|error| format!("invalid bitcoin txid: {error}"))?;
    if bytes.len() != 32 {
        return Err("invalid bitcoin txid length".to_string());
    }
    bytes.reverse();
    Ok(bytes)
}

fn p2pkh_script_code(pubkey_hash: &[u8]) -> Vec<u8> {
    let mut script = Vec::with_capacity(25);
    script.push(0x76);
    script.push(0xa9);
    script.push(0x14);
    script.extend_from_slice(pubkey_hash);
    script.push(0x88);
    script.push(0xac);
    script
}

fn script_pubkey_for_segwit_address(address: &str, network: &str) -> Result<Vec<u8>, String> {
    let (_hrp, version, program) = decode_segwit_address(address)?;
    validate_bitcoin_address_for_network(address, network)?;
    let mut script = Vec::with_capacity(program.len() + 2);
    script.push(version);
    script.push(program.len() as u8);
    script.extend_from_slice(&program);
    Ok(script)
}

fn push_varint(value: u64, out: &mut Vec<u8>) {
    if value < 0xfd {
        out.push(value as u8);
    } else if value <= 0xffff {
        out.push(0xfd);
        out.extend_from_slice(&(value as u16).to_le_bytes());
    } else if value <= 0xffff_ffff {
        out.push(0xfe);
        out.extend_from_slice(&(value as u32).to_le_bytes());
    } else {
        out.push(0xff);
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn encode_segwit_address(hrp: &str, version: u8, program: &[u8]) -> Result<String, String> {
    if version > 16 {
        return Err("invalid SegWit version".to_string());
    }
    let mut data = vec![version];
    data.extend(convert_bits(program, 8, 5, true)?);
    let checksum = bech32_checksum(hrp, &data);
    data.extend_from_slice(&checksum);
    let mut out = String::with_capacity(hrp.len() + 1 + data.len());
    out.push_str(hrp);
    out.push('1');
    for value in data {
        out.push(BECH32_CHARSET[value as usize] as char);
    }
    Ok(out)
}

fn decode_segwit_address(address: &str) -> Result<(String, u8, Vec<u8>), String> {
    let (hrp, data) = bech32_decode(address)?;
    if data.is_empty() {
        return Err("bitcoin address missing witness version".to_string());
    }
    let version = data[0];
    if version > 16 {
        return Err("invalid SegWit witness version".to_string());
    }
    let program = convert_bits(&data[1..], 5, 8, false)?;
    Ok((hrp, version, program))
}

fn bech32_decode(address: &str) -> Result<(String, Vec<u8>), String> {
    if address.len() < 8 || address.len() > 90 {
        return Err("invalid bech32 length".to_string());
    }
    let has_lower = address.bytes().any(|b| b.is_ascii_lowercase());
    let has_upper = address.bytes().any(|b| b.is_ascii_uppercase());
    if has_lower && has_upper {
        return Err("mixed-case bech32 address".to_string());
    }
    let normalized = address.to_ascii_lowercase();
    let split = normalized
        .rfind('1')
        .ok_or_else(|| "bech32 separator missing".to_string())?;
    if split == 0 || split + 7 > normalized.len() {
        return Err("invalid bech32 separator position".to_string());
    }
    let hrp = normalized[..split].to_string();
    let mut data = Vec::new();
    for b in normalized[split + 1..].bytes() {
        let value = BECH32_CHARSET
            .iter()
            .position(|candidate| *candidate == b)
            .ok_or_else(|| "invalid bech32 character".to_string())?;
        data.push(value as u8);
    }
    if !bech32_verify_checksum(&hrp, &data) {
        return Err("invalid bech32 checksum".to_string());
    }
    data.truncate(data.len().saturating_sub(6));
    Ok((hrp, data))
}

fn bech32_checksum(hrp: &str, data: &[u8]) -> [u8; 6] {
    let mut values = bech32_hrp_expand(hrp);
    values.extend_from_slice(data);
    values.extend_from_slice(&[0u8; 6]);
    let polymod = bech32_polymod(&values) ^ 1;
    let mut out = [0u8; 6];
    for (idx, slot) in out.iter_mut().enumerate() {
        *slot = ((polymod >> (5 * (5 - idx))) & 31) as u8;
    }
    out
}

fn bech32_verify_checksum(hrp: &str, data: &[u8]) -> bool {
    let mut values = bech32_hrp_expand(hrp);
    values.extend_from_slice(data);
    bech32_polymod(&values) == 1
}

fn bech32_hrp_expand(hrp: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(hrp.len() * 2 + 1);
    for b in hrp.bytes() {
        out.push(b >> 5);
    }
    out.push(0);
    for b in hrp.bytes() {
        out.push(b & 31);
    }
    out
}

fn bech32_polymod(values: &[u8]) -> u32 {
    let generators = [
        0x3b6a57b2u32,
        0x26508e6d,
        0x1ea119fa,
        0x3d4233dd,
        0x2a1462b3,
    ];
    let mut chk = 1u32;
    for value in values {
        let top = chk >> 25;
        chk = (chk & 0x1ff_ffff) << 5 ^ (*value as u32);
        for (idx, generator) in generators.iter().enumerate() {
            if ((top >> idx) & 1) == 1 {
                chk ^= generator;
            }
        }
    }
    chk
}

fn convert_bits(data: &[u8], from: u32, to: u32, pad: bool) -> Result<Vec<u8>, String> {
    let mut acc = 0u32;
    let mut bits = 0u32;
    let maxv = (1u32 << to) - 1;
    let max_acc = (1u32 << (from + to - 1)) - 1;
    let mut out = Vec::new();
    for value in data {
        let value = *value as u32;
        if value >> from != 0 {
            return Err("invalid bit conversion input".to_string());
        }
        acc = ((acc << from) | value) & max_acc;
        bits += from;
        while bits >= to {
            bits -= to;
            out.push(((acc >> bits) & maxv) as u8);
        }
    }
    if pad {
        if bits > 0 {
            out.push(((acc << (to - bits)) & maxv) as u8);
        }
    } else if bits >= from || ((acc << (to - bits)) & maxv) != 0 {
        return Err("invalid non-zero padding in bech32 data".to_string());
    }
    Ok(out)
}
