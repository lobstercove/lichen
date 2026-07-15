use base64::{engine::general_purpose, Engine as _};
use lichen_core::{
    ContractInstruction, Hash, Instruction, Keypair, KeypairFile, Message, Pubkey, Transaction,
};
use serde::Deserialize;
use serde_json::json;
use std::env;
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "Usage: cargo run -p lichen-rpc --bin wrapped_burn -- \
  --rpc-url URL --contract ADDRESS --amount SPORES \
  [--seed-byte N | --keypair PATH]"
    );
    std::process::exit(2);
}

fn next_arg(args: &[String], index: &mut usize, flag: &str) -> String {
    *index += 1;
    if *index >= args.len() {
        eprintln!("missing value for {}", flag);
        usage();
    }
    args[*index].clone()
}

#[derive(Deserialize)]
struct RpcResponse {
    result: Option<serde_json::Value>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

async fn rpc_call(
    client: &reqwest::Client,
    rpc_url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let response = client
        .post(rpc_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        }))
        .send()
        .await
        .map_err(|error| format!("send RPC request: {}", error))?;
    let response: RpcResponse = response
        .json()
        .await
        .map_err(|error| format!("decode RPC response: {}", error))?;
    if let Some(error) = response.error {
        return Err(format!("RPC error {}: {}", error.code, error.message));
    }
    response
        .result
        .ok_or_else(|| "missing RPC result".to_string())
}

async fn recent_blockhash(client: &reqwest::Client, rpc_url: &str) -> Result<Hash, String> {
    let result = rpc_call(client, rpc_url, "getRecentBlockhash", json!([])).await?;
    let hash = result
        .as_str()
        .or_else(|| result.get("blockhash").and_then(|value| value.as_str()))
        .ok_or_else(|| "invalid getRecentBlockhash response".to_string())?;
    Hash::from_hex(hash)
}

async fn chain_id(client: &reqwest::Client, rpc_url: &str) -> Result<String, String> {
    let result = rpc_call(client, rpc_url, "getNetworkInfo", json!([])).await?;
    result
        .get("chain_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "getNetworkInfo response missing chain_id".to_string())
}

fn load_signer(seed_byte: Option<u8>, keypair_path: Option<PathBuf>) -> Result<Keypair, String> {
    match (seed_byte, keypair_path) {
        (Some(seed_byte), None) => Ok(Keypair::from_seed(&[seed_byte; 32])),
        (None, Some(path)) => KeypairFile::load(&path)?.to_keypair(),
        _ => Err("provide exactly one of --seed-byte or --keypair".to_string()),
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        usage();
    }

    let mut rpc_url = None;
    let mut contract = None;
    let mut amount = None;
    let mut seed_byte = None;
    let mut keypair_path = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--rpc-url" => rpc_url = Some(next_arg(&args, &mut index, "--rpc-url")),
            "--contract" => contract = Some(next_arg(&args, &mut index, "--contract")),
            "--amount" => {
                amount = Some(
                    next_arg(&args, &mut index, "--amount")
                        .parse::<u64>()
                        .unwrap_or_else(|_| {
                            eprintln!("--amount must be an unsigned integer in base units");
                            usage();
                        }),
                );
            }
            "--seed-byte" => {
                seed_byte = Some(
                    next_arg(&args, &mut index, "--seed-byte")
                        .parse::<u8>()
                        .unwrap_or_else(|_| {
                            eprintln!("--seed-byte must fit in u8");
                            usage();
                        }),
                );
            }
            "--keypair" => {
                keypair_path = Some(PathBuf::from(next_arg(&args, &mut index, "--keypair")))
            }
            unknown => {
                eprintln!("unknown argument: {}", unknown);
                usage();
            }
        }
        index += 1;
    }

    let rpc_url = rpc_url.unwrap_or_else(|| usage());
    let contract = Pubkey::from_base58(&contract.unwrap_or_else(|| usage()))
        .unwrap_or_else(|error| panic!("invalid --contract: {}", error));
    let amount = amount.unwrap_or_else(|| usage());
    let signer = load_signer(seed_byte, keypair_path).unwrap_or_else(|error| {
        eprintln!("{}", error);
        std::process::exit(2);
    });
    let user = signer.pubkey();

    let mut burn_args = Vec::with_capacity(40);
    burn_args.extend_from_slice(&user.0);
    burn_args.extend_from_slice(&amount.to_le_bytes());

    let contract_ix = ContractInstruction::Call {
        function: "burn".to_string(),
        args: burn_args,
        value: 0,
    };
    let instruction = Instruction {
        program_id: Pubkey::new([0xFFu8; 32]),
        accounts: vec![user, contract],
        data: contract_ix
            .serialize()
            .unwrap_or_else(|error| panic!("serialize contract instruction: {}", error)),
    };

    let client = reqwest::Client::new();
    let message = Message {
        instructions: vec![instruction],
        recent_blockhash: recent_blockhash(&client, &rpc_url)
            .await
            .unwrap_or_else(|error| panic!("get recent blockhash: {}", error)),
        compute_budget: None,
        compute_unit_price: None,
    };
    let signing_chain_id = chain_id(&client, &rpc_url)
        .await
        .unwrap_or_else(|error| panic!("get chain id: {}", error));
    let signature = signer.sign(&message.signing_bytes_for_chain_id(&signing_chain_id));
    let tx = Transaction {
        signatures: vec![signature],
        message,
        tx_type: Default::default(),
    };
    let tx_base64 = general_purpose::STANDARD.encode(tx.to_wire());
    let result = rpc_call(&client, &rpc_url, "sendTransaction", json!([tx_base64]))
        .await
        .unwrap_or_else(|error| panic!("send transaction: {}", error));
    let tx_signature = result
        .as_str()
        .unwrap_or_else(|| panic!("sendTransaction returned non-string result: {}", result));

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "signature": tx_signature,
            "user_id": user.to_base58(),
            "contract": contract.to_base58(),
            "function": "burn",
            "amount": amount,
        }))
        .expect("encode output")
    );
}
