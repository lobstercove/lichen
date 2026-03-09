use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use moltchain_core::{ContractInstruction, Hash, Instruction, Keypair, Message, Pubkey, Transaction};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Deserialize)]
struct WalletFile {
    #[serde(rename = "privateKey")]
    private_key: String,
}

#[derive(Debug, Deserialize)]
struct ContractEntry {
    program_id: String,
    symbol: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AbiParam {
    #[allow(dead_code)]
    name: String,
    #[serde(rename = "type")]
    param_type: String,
}

#[derive(Debug, Deserialize)]
struct AbiFn {
    name: String,
    #[serde(default)]
    params: Vec<AbiParam>,
    #[serde(default)]
    readonly: bool,
}

#[derive(Debug, Serialize)]
struct FnResult {
    contract: String,
    program_id: String,
    function: String,
    readonly: bool,
    ok: bool,
    return_code: Option<i64>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct Report {
    rpc_url: String,
    caller: String,
    contracts_total: usize,
    functions_total: usize,
    simulate_ok: usize,
    simulate_fail: usize,
    top_errors: BTreeMap<String, usize>,
    results: Vec<FnResult>,
}

fn parse_cli_arg(flag: &str, default: &str) -> String {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == flag {
            return args[i + 1].clone();
        }
    }
    default.to_string()
}

fn default_wallet_path() -> String {
    std::env::var("HOME")
        .map(|home| format!("{}/.moltchain/wallets/agent.json", home))
        .unwrap_or_else(|_| ".moltchain/wallets/agent.json".to_string())
}

fn default_arg_for_type(t: &str, caller: &str) -> Value {
    let tl = t.to_ascii_lowercase();
    if tl.contains("vec<") || tl.ends_with("[]") {
        return json!([]);
    }
    if tl.contains("bool") {
        return json!(false);
    }
    if tl.contains("string") || tl.contains("str") {
        return json!("");
    }
    if tl.contains("pubkey") || tl.contains("address") {
        return json!(caller);
    }
    if tl.contains("u8")
        || tl.contains("u16")
        || tl.contains("u32")
        || tl.contains("u64")
        || tl.contains("u128")
        || tl.contains("i8")
        || tl.contains("i16")
        || tl.contains("i32")
        || tl.contains("i64")
        || tl.contains("i128")
        || tl.contains("int")
        || tl.contains("number")
    {
        return json!(0);
    }
    json!(0)
}

async fn rpc(client: &Client, rpc_url: &str, method: &str, params: Value) -> Result<Value> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });

    let resp: Value = client
        .post(rpc_url)
        .json(&payload)
        .send()
        .await
        .with_context(|| format!("rpc {} send failed", method))?
        .json()
        .await
        .with_context(|| format!("rpc {} json failed", method))?;

    if let Some(err) = resp.get("error") {
        return Err(anyhow::anyhow!(format!("{}", err)));
    }
    Ok(resp.get("result").cloned().unwrap_or(Value::Null))
}

fn build_contract_call_tx_b64(
    caller: &Keypair,
    contract_address: &Pubkey,
    function: &str,
    args_json_array: &[Value],
    recent_blockhash: Hash,
) -> Result<String> {
    let args_bytes = serde_json::to_vec(args_json_array)?;
    let contract_ix = ContractInstruction::Call {
        function: function.to_string(),
        args: args_bytes,
        value: 0,
    };

    let instruction = Instruction {
        program_id: Pubkey::new([0xFFu8; 32]),
        accounts: vec![caller.pubkey(), *contract_address],
        data: contract_ix
            .serialize()
            .map_err(|e| anyhow::anyhow!(format!("contract ix serialize: {}", e)))?,
    };

    let message = Message {
        instructions: vec![instruction],
        recent_blockhash,
    };

    let signature = caller.sign(&message.serialize());
    let tx = Transaction {
        signatures: vec![signature],
        message,
    };

    let tx_bytes = bincode::serialize(&tx)?;
    Ok(B64.encode(tx_bytes))
}

#[tokio::main]
async fn main() -> Result<()> {
    let rpc_url = parse_cli_arg("--rpc-url", "http://localhost:8899");
    let wallet_path = {
        let wallet_arg = parse_cli_arg("--wallet", "");
        if wallet_arg.is_empty() {
            default_wallet_path()
        } else {
            wallet_arg
        }
    };
    let max_functions: usize = parse_cli_arg("--max-functions", "0")
        .parse()
        .unwrap_or(0);

    let wallet_raw = fs::read_to_string(&wallet_path)
        .with_context(|| format!("read wallet file failed: {}", wallet_path))?;
    let wallet: WalletFile = serde_json::from_str(&wallet_raw)?;
    let seed_vec = hex::decode(wallet.private_key).context("wallet privateKey is not hex")?;
    if seed_vec.len() != 32 {
        return Err(anyhow::anyhow!("wallet privateKey must be 32 bytes"));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&seed_vec);
    let caller = Keypair::from_seed(&seed);
    let caller_addr = caller.pubkey().to_base58();

    let client = Client::new();
    let contracts_val = rpc(&client, &rpc_url, "getAllContracts", json!([])).await?;
    let contracts: Vec<ContractEntry> = serde_json::from_value(
        contracts_val
            .get("contracts")
            .cloned()
            .unwrap_or_else(|| Value::Array(vec![])),
    )?;

    let blockhash_val = rpc(&client, &rpc_url, "getRecentBlockhash", json!([])).await?;
    let blockhash_str = blockhash_val
        .get("blockhash")
        .and_then(Value::as_str)
        .or_else(|| blockhash_val.as_str())
        .context("getRecentBlockhash missing blockhash")?;
    let recent_blockhash = Hash::from_hex(blockhash_str)
        .map_err(|e| anyhow::anyhow!(format!("invalid blockhash: {}", e)))?;

    let mut results = Vec::new();
    let mut total_functions = 0usize;
    let mut ok_count = 0usize;
    let mut fail_count = 0usize;
    let mut top_errors: BTreeMap<String, usize> = BTreeMap::new();

    'contracts: for c in &contracts {
        let contract_label = c
            .symbol
            .clone()
            .or_else(|| c.name.clone())
            .unwrap_or_else(|| c.program_id.clone());
        let contract_pubkey = match Pubkey::from_base58(&c.program_id) {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("invalid program id: {}", e);
                *top_errors.entry(msg.clone()).or_insert(0) += 1;
                results.push(FnResult {
                    contract: contract_label,
                    program_id: c.program_id.clone(),
                    function: "<parse_program_id>".to_string(),
                    readonly: false,
                    ok: false,
                    return_code: None,
                    error: Some(msg),
                });
                continue;
            }
        };

        let abi_result = match rpc(&client, &rpc_url, "getContractAbi", json!([c.program_id])).await {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("abi rpc error: {}", e);
                *top_errors.entry(msg.clone()).or_insert(0) += 1;
                results.push(FnResult {
                    contract: contract_label,
                    program_id: c.program_id.clone(),
                    function: "<getContractAbi>".to_string(),
                    readonly: false,
                    ok: false,
                    return_code: None,
                    error: Some(msg),
                });
                continue;
            }
        };

        let funcs: Vec<AbiFn> = serde_json::from_value(
            abi_result
                .get("functions")
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![])),
        )
        .unwrap_or_default();

        for f in funcs {
            if max_functions > 0 && total_functions >= max_functions {
                break 'contracts;
            }

            total_functions += 1;
            let args_json: Vec<Value> = f
                .params
                .iter()
                .map(|p| default_arg_for_type(&p.param_type, &caller_addr))
                .collect();

            let tx_base64 = match build_contract_call_tx_b64(
                &caller,
                &contract_pubkey,
                &f.name,
                &args_json,
                recent_blockhash,
            ) {
                Ok(v) => v,
                Err(e) => {
                    let msg = format!("build tx error: {}", e);
                    *top_errors.entry(msg.clone()).or_insert(0) += 1;
                    fail_count += 1;
                    results.push(FnResult {
                        contract: contract_label.clone(),
                        program_id: c.program_id.clone(),
                        function: f.name.clone(),
                        readonly: f.readonly,
                        ok: false,
                        return_code: None,
                        error: Some(msg),
                    });
                    continue;
                }
            };

            let sim_res = rpc(&client, &rpc_url, "simulateTransaction", json!([tx_base64])).await;
            match sim_res {
                Ok(v) => {
                    let return_code = v.get("returnCode").and_then(Value::as_i64);
                    let ok = v
                        .get("success")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        && return_code.unwrap_or(1) == 0;
                    if ok {
                        ok_count += 1;
                    } else {
                        fail_count += 1;
                        let error_msg = v
                            .get("error")
                            .and_then(Value::as_str)
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("returnCode={:?}", return_code));
                        *top_errors.entry(error_msg.clone()).or_insert(0) += 1;
                    }
                    results.push(FnResult {
                        contract: contract_label.clone(),
                        program_id: c.program_id.clone(),
                        function: f.name.clone(),
                        readonly: f.readonly,
                        ok,
                        return_code,
                        error: if ok {
                            None
                        } else {
                            v.get("error").and_then(Value::as_str).map(|s| s.to_string())
                        },
                    });
                }
                Err(e) => {
                    let msg = format!("simulate rpc error: {}", e);
                    *top_errors.entry(msg.clone()).or_insert(0) += 1;
                    fail_count += 1;
                    results.push(FnResult {
                        contract: contract_label.clone(),
                        program_id: c.program_id.clone(),
                        function: f.name.clone(),
                        readonly: f.readonly,
                        ok: false,
                        return_code: None,
                        error: Some(msg),
                    });
                }
            }
        }
    }

    let report = Report {
        rpc_url: rpc_url.clone(),
        caller: caller_addr,
        contracts_total: contracts.len(),
        functions_total: total_functions,
        simulate_ok: ok_count,
        simulate_fail: fail_count,
        top_errors,
        results,
    };

    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let out_path = PathBuf::from(format!("artifacts/agent_tx_contract_suite_{}.json", ts));
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out_path, serde_json::to_vec_pretty(&report)?)?;

    println!("REPORT {}", out_path.display());
    println!("contracts_total {}", report.contracts_total);
    println!("functions_total {}", report.functions_total);
    println!("simulate_ok {}", report.simulate_ok);
    println!("simulate_fail {}", report.simulate_fail);
    for (k, v) in report.top_errors.iter().take(10) {
        println!("- {} x {}", v, k);
    }

    Ok(())
}
