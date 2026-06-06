use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use clap::{Parser, Subcommand};
use lichen_core::{
    keypair_password_from_env, Hash, Instruction, Keypair, KeypairFile, Message, Pubkey,
    Transaction, SYSTEM_PROGRAM_ID,
};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};

const GOVERNANCE_ACTION_CONTRACT_CALL: u8 = 9;

#[derive(Parser)]
#[command(
    name = "protocol-governance-contract-call",
    about = "Submit protocol governance contract-call proposals and lifecycle actions"
)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8899")]
    rpc_url: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Propose {
        #[arg(long)]
        keypair: PathBuf,
        #[arg(long)]
        governance_authority: String,
        #[arg(long)]
        contract: String,
        #[arg(long)]
        function: String,
        #[arg(long, default_value = "")]
        args_hex: String,
        #[arg(long, default_value_t = 0)]
        value: u64,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        skip_preflight: bool,
        #[arg(long)]
        compute_budget: Option<u64>,
    },
    Approve {
        #[arg(long)]
        keypair: PathBuf,
        #[arg(long)]
        proposal_id: u64,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        skip_preflight: bool,
        #[arg(long)]
        compute_budget: Option<u64>,
    },
    Execute {
        #[arg(long)]
        keypair: PathBuf,
        #[arg(long)]
        proposal_id: u64,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        skip_preflight: bool,
        #[arg(long)]
        compute_budget: Option<u64>,
    },
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

struct Rpc {
    url: String,
    client: reqwest::Client,
}

impl Rpc {
    fn new(url: String) -> Self {
        Self {
            url,
            client: reqwest::Client::new(),
        }
    }

    async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let response = self
            .client
            .post(&self.url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            }))
            .send()
            .await
            .with_context(|| format!("failed to call RPC method {method}"))?;

        let body: RpcResponse = response
            .json()
            .await
            .with_context(|| format!("failed to decode RPC response for {method}"))?;

        if let Some(error) = body.error {
            bail!("RPC error {} from {method}: {}", error.code, error.message);
        }

        body.result
            .ok_or_else(|| anyhow!("RPC method {method} returned no result"))
    }

    async fn chain_id(&self) -> Result<String> {
        let result = self.call("getNetworkInfo", json!([])).await?;
        result
            .get("chain_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .context("getNetworkInfo result missing chain_id")
    }

    async fn recent_blockhash(&self) -> Result<Hash> {
        let result = self.call("getRecentBlockhash", json!([])).await?;
        let blockhash = if let Some(value) = result.as_str() {
            value
        } else {
            result
                .get("blockhash")
                .and_then(serde_json::Value::as_str)
                .context("getRecentBlockhash result missing blockhash")?
        };
        Hash::from_hex(blockhash).map_err(anyhow::Error::msg)
    }

    async fn simulate(&self, tx: &Transaction) -> Result<serde_json::Value> {
        let tx_base64 = base64::engine::general_purpose::STANDARD.encode(tx.to_wire());
        self.call("simulateTransaction", json!([tx_base64])).await
    }

    async fn send(&self, tx: &Transaction) -> Result<String> {
        let tx_base64 = base64::engine::general_purpose::STANDARD.encode(tx.to_wire());
        let result = self.call("sendTransaction", json!([tx_base64])).await?;
        result
            .as_str()
            .map(str::to_string)
            .context("sendTransaction result was not a signature string")
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let rpc = Rpc::new(cli.rpc_url);

    match cli.command {
        Command::Propose {
            keypair,
            governance_authority,
            contract,
            function,
            args_hex,
            value,
            dry_run,
            skip_preflight,
            compute_budget,
        } => {
            let signer = load_keypair(&keypair)?;
            let governance_authority = parse_pubkey(&governance_authority, "governance authority")?;
            let contract = parse_pubkey(&contract, "contract")?;
            let args = parse_hex(&args_hex).context("invalid --args-hex")?;
            let data = build_propose_contract_call_data(&function, &args, value)?;
            let instruction = Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![signer.pubkey(), governance_authority, contract],
                data,
            };
            submit_instruction(
                &rpc,
                &signer,
                instruction,
                dry_run,
                skip_preflight,
                compute_budget,
            )
            .await?;
        }
        Command::Approve {
            keypair,
            proposal_id,
            dry_run,
            skip_preflight,
            compute_budget,
        } => {
            let signer = load_keypair(&keypair)?;
            let instruction = Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![signer.pubkey()],
                data: build_proposal_id_data(35, proposal_id),
            };
            submit_instruction(
                &rpc,
                &signer,
                instruction,
                dry_run,
                skip_preflight,
                compute_budget,
            )
            .await?;
        }
        Command::Execute {
            keypair,
            proposal_id,
            dry_run,
            skip_preflight,
            compute_budget,
        } => {
            let signer = load_keypair(&keypair)?;
            let instruction = Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![signer.pubkey()],
                data: build_proposal_id_data(36, proposal_id),
            };
            submit_instruction(
                &rpc,
                &signer,
                instruction,
                dry_run,
                skip_preflight,
                compute_budget,
            )
            .await?;
        }
    }

    Ok(())
}

fn load_keypair(path: &Path) -> Result<Keypair> {
    let password = keypair_password_from_env();
    KeypairFile::load_with_password_policy(path, password.as_deref(), true)
        .and_then(|file| file.to_keypair())
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("failed to load keypair {}", path.display()))
}

fn parse_pubkey(value: &str, label: &str) -> Result<Pubkey> {
    Pubkey::from_base58(value).map_err(|error| anyhow!("invalid {label}: {error}"))
}

fn parse_hex(value: &str) -> Result<Vec<u8>> {
    let trimmed = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    hex::decode(trimmed).map_err(anyhow::Error::from)
}

fn build_propose_contract_call_data(function: &str, args: &[u8], value: u64) -> Result<Vec<u8>> {
    let function_bytes = function.as_bytes();
    if function_bytes.is_empty() {
        bail!("function name cannot be empty");
    }
    if function_bytes.len() > u16::MAX as usize {
        bail!("function name is too long");
    }
    if args.len() > u32::MAX as usize {
        bail!("args payload is too long");
    }

    let mut data = vec![34, GOVERNANCE_ACTION_CONTRACT_CALL];
    data.extend_from_slice(&value.to_le_bytes());
    data.extend_from_slice(&(function_bytes.len() as u16).to_le_bytes());
    data.extend_from_slice(function_bytes);
    data.extend_from_slice(&(args.len() as u32).to_le_bytes());
    data.extend_from_slice(args);
    Ok(data)
}

fn build_proposal_id_data(opcode: u8, proposal_id: u64) -> Vec<u8> {
    let mut data = vec![opcode];
    data.extend_from_slice(&proposal_id.to_le_bytes());
    data
}

async fn submit_instruction(
    rpc: &Rpc,
    signer: &Keypair,
    instruction: Instruction,
    dry_run: bool,
    skip_preflight: bool,
    compute_budget: Option<u64>,
) -> Result<()> {
    let chain_id = rpc.chain_id().await?;
    let message = Message {
        instructions: vec![instruction],
        recent_blockhash: rpc.recent_blockhash().await?,
        compute_budget,
        compute_unit_price: None,
    };
    let signature = signer.sign(&message.signing_bytes_for_chain_id(&chain_id));
    let tx = Transaction {
        signatures: vec![signature],
        message,
        tx_type: Default::default(),
    };

    if !skip_preflight {
        let simulation = rpc.simulate(&tx).await?;
        println!(
            "preflight={}",
            serde_json::to_string(&simulation).context("failed to render preflight result")?
        );
        if !simulation
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            let error = simulation
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("simulation returned success=false");
            bail!("preflight failed: {error}");
        }
    }

    if dry_run {
        println!("dry_run=true");
        return Ok(());
    }

    let signature = rpc.send(&tx).await?;
    println!("signature={signature}");
    Ok(())
}
