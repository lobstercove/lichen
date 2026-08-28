use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use clap::{Parser, Subcommand};
use lichen_core::{
    keypair_password_from_env, ContractInstruction, Hash, Instruction, Keypair, KeypairFile,
    Message, Pubkey, Transaction,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const PRICE_SCALE: u128 = 1_000_000_000;
const MAX_POSITIONS: u64 = 10_000;
const CONTRACT_PROGRAM_ID: Pubkey = Pubkey([0xFF; 32]);

#[derive(Parser)]
#[command(
    name = "dex-margin-v2-migrate",
    about = "Create, execute, and verify fail-closed DEX Margin V2 migrations"
)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8899")]
    rpc_url: String,

    #[arg(long, default_value = "http://127.0.0.1:8899/api/v1")]
    rest_url: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture the exact paused, migration-locked legacy position manifest.
    Manifest {
        #[arg(long)]
        margin_contract: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Simulate every migration transaction; submit only with --execute.
    Migrate {
        #[arg(long)]
        margin_contract: String,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        keypair: PathBuf,
        #[arg(long)]
        receipts: PathBuf,
        #[arg(long)]
        execute: bool,
        #[arg(long, default_value_t = 20)]
        confirmation_attempts: usize,
    },
    /// Verify every manifest position and all aggregate migration invariants.
    Verify {
        #[arg(long)]
        manifest: PathBuf,
    },
    /// Print the governed call payload that begins the frozen migration phase.
    BeginArgs {
        #[arg(long)]
        governance_authority: String,
    },
    /// Print the single governed payload that atomically activates both V2 engines.
    ActivateArgs {
        #[arg(long)]
        governance_authority: String,
        #[arg(long)]
        manifest: PathBuf,
    },
    /// Print the governed payload that completes migration and atomically reopens margin.
    CompleteArgs {
        #[arg(long)]
        governance_authority: String,
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

#[derive(Deserialize)]
struct RestEnvelope<T> {
    success: bool,
    data: Option<T>,
    error: Option<String>,
    slot: u64,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarginInfo {
    paused: bool,
    migration_locked: bool,
    position_count: u64,
    total_open_interest: u64,
    total_collateral_escrowed: u64,
    funding_v2_enabled: bool,
    funding_migrated_open_count: u64,
    funding_migration_finalized: bool,
    cross_v2_enabled: bool,
    cross_migrated_open_count: u64,
    cross_migration_finalized: bool,
    cross_total_collateral: u64,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarginPosition {
    position_id: u64,
    trader: String,
    pair_id: u64,
    side: String,
    margin_type: String,
    status: String,
    size: u64,
    margin: u64,
    entry_price_raw: u64,
    leverage: u64,
    funding_v2_migrated: bool,
    cross_v2_migrated: bool,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ManifestPosition {
    position_id: u64,
    trader: String,
    pair_id: u64,
    side: String,
    margin_type: String,
    size: u64,
    margin: u64,
    entry_price_raw: u64,
    leverage: u64,
    entry_notional: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestPayload {
    schema: u64,
    chain_id: String,
    source_slot: u64,
    margin_contract: String,
    position_count: u64,
    open_position_count: u64,
    open_cross_count: u64,
    total_open_interest: u64,
    total_collateral_escrowed: u64,
    total_cross_collateral: u64,
    positions: Vec<ManifestPosition>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrationManifest {
    manifest_sha256: String,
    #[serde(flatten)]
    payload: ManifestPayload,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MigrationReceipt {
    position_id: u64,
    opcodes: Vec<u8>,
    simulated: bool,
    submitted: bool,
    signature: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptFile {
    schema: u64,
    chain_id: String,
    manifest_sha256: String,
    operator: String,
    execute: bool,
    receipts: Vec<MigrationReceipt>,
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
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .with_context(|| format!("failed to call {method}"))?;
        let status = response.status();
        let body: RpcResponse = response
            .json()
            .await
            .with_context(|| format!("failed to decode {method} response (HTTP {status})"))?;
        if let Some(error) = body.error {
            bail!("RPC error {} from {method}: {}", error.code, error.message);
        }
        body.result
            .ok_or_else(|| anyhow!("RPC method {method} returned no result"))
    }

    async fn chain_id(&self) -> Result<String> {
        self.call("getNetworkInfo", json!([]))
            .await?
            .get("chain_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .context("getNetworkInfo missing chain_id")
    }

    async fn recent_blockhash(&self) -> Result<Hash> {
        let result = self.call("getRecentBlockhash", json!([])).await?;
        let value = result
            .as_str()
            .or_else(|| result.get("blockhash").and_then(serde_json::Value::as_str))
            .context("getRecentBlockhash missing blockhash")?;
        Hash::from_hex(value).map_err(anyhow::Error::msg)
    }

    async fn simulate(&self, transaction: &Transaction) -> Result<()> {
        let wire = base64::engine::general_purpose::STANDARD.encode(transaction.to_wire());
        let result = self.call("simulateTransaction", json!([wire])).await?;
        if !result
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            let error = result
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("simulation returned success=false");
            bail!("preflight failed: {error}");
        }
        if let Some(code) = result
            .get("returnCode")
            .or_else(|| result.get("return_code"))
            .and_then(serde_json::Value::as_u64)
        {
            if code != 0 {
                bail!("preflight returned contract code {code}");
            }
        }
        Ok(())
    }

    async fn send(&self, transaction: &Transaction) -> Result<String> {
        let wire = base64::engine::general_purpose::STANDARD.encode(transaction.to_wire());
        self.call("sendTransaction", json!([wire]))
            .await?
            .as_str()
            .map(str::to_owned)
            .context("sendTransaction did not return a signature")
    }

    async fn wait_for_confirmation(&self, signature: &str, attempts: usize) -> Result<()> {
        for _ in 0..attempts {
            if let Ok(value) = self.call("getTransaction", json!([signature])).await {
                if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
                    if !error.is_empty() {
                        bail!("transaction {signature} failed: {error}");
                    }
                }
                if matches!(
                    value.get("status").and_then(serde_json::Value::as_str),
                    Some("confirmed" | "finalized" | "success")
                ) {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        bail!("transaction {signature} was not confirmed after {attempts} attempts")
    }
}

async fn get_rest<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<RestEnvelope<T>> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to GET {url}"))?;
    let status = response.status();
    let envelope: RestEnvelope<T> = response
        .json()
        .await
        .with_context(|| format!("failed to decode {url} (HTTP {status})"))?;
    if !status.is_success() || !envelope.success {
        bail!(
            "GET {url} failed: {}",
            envelope.error.unwrap_or_else(|| status.to_string())
        );
    }
    Ok(envelope)
}

fn manifest_hash(payload: &ManifestPayload) -> Result<String> {
    let encoded = serde_json::to_vec(payload).context("failed to encode manifest payload")?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

fn read_manifest(path: &Path) -> Result<MigrationManifest> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read manifest {}", path.display()))?;
    let manifest: MigrationManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to decode manifest {}", path.display()))?;
    if manifest.payload.schema != 1 {
        bail!("unsupported manifest schema {}", manifest.payload.schema);
    }
    let expected = manifest_hash(&manifest.payload)?;
    if manifest.manifest_sha256 != expected {
        bail!("manifest checksum mismatch: expected {expected}");
    }
    Ok(manifest)
}

fn checked_notional(position: &MarginPosition) -> Result<u64> {
    let value = (position.size as u128)
        .checked_mul(position.entry_price_raw as u128)
        .context("position notional multiplication overflow")?
        / PRICE_SCALE;
    u64::try_from(value).context("position notional exceeds u64")
}

fn validate_manifest_positions(
    info: &MarginInfo,
    positions: &[MarginPosition],
) -> Result<(Vec<ManifestPosition>, u64, u64)> {
    let mut output = Vec::new();
    let mut open_interest = 0u64;
    let mut collateral = 0u64;
    let mut cross_per_trader = HashMap::<&str, u64>::new();
    let mut side_sizes = HashMap::<(u64, &str), u64>::new();
    let mut cross_balances = HashMap::<&str, u64>::new();

    for position in positions
        .iter()
        .filter(|position| position.status == "open")
    {
        if position.position_id == 0 || position.size == 0 || position.entry_price_raw == 0 {
            bail!(
                "open position {} has invalid zero fields",
                position.position_id
            );
        }
        if position.margin_type != "isolated" && position.margin_type != "cross" {
            bail!("position {} has invalid margin mode", position.position_id);
        }
        if position.funding_v2_migrated || position.cross_v2_migrated {
            bail!(
                "position {} was already partially migrated",
                position.position_id
            );
        }
        let entry_notional = checked_notional(position)?;
        open_interest = open_interest
            .checked_add(entry_notional)
            .context("open-interest sum overflow")?;
        collateral = collateral
            .checked_add(position.margin)
            .context("collateral sum overflow")?;
        if position.margin_type == "cross" {
            let count = cross_per_trader.entry(&position.trader).or_default();
            *count += 1;
            if *count > 32 {
                bail!(
                    "trader {} exceeds the 32-position Cross V2 bound",
                    position.trader
                );
            }
            let balance = cross_balances.entry(&position.trader).or_default();
            *balance = balance
                .checked_add(position.margin)
                .context("per-trader cross collateral overflow")?;
        }
        let side_size = side_sizes
            .entry((position.pair_id, &position.side))
            .or_default();
        *side_size = side_size
            .checked_add(position.size)
            .context("per-pair side-size overflow")?;
        output.push(ManifestPosition {
            position_id: position.position_id,
            trader: position.trader.clone(),
            pair_id: position.pair_id,
            side: position.side.clone(),
            margin_type: position.margin_type.clone(),
            size: position.size,
            margin: position.margin,
            entry_price_raw: position.entry_price_raw,
            leverage: position.leverage,
            entry_notional,
        });
    }

    output.sort_by_key(|position| position.position_id);
    if open_interest != info.total_open_interest {
        bail!(
            "open-interest mismatch: manifest {open_interest}, on-chain {}",
            info.total_open_interest
        );
    }
    if collateral != info.total_collateral_escrowed {
        bail!(
            "collateral mismatch: manifest {collateral}, on-chain {}",
            info.total_collateral_escrowed
        );
    }
    Ok((output, open_interest, collateral))
}

async fn load_info_and_positions(
    rest_url: &str,
) -> Result<(RestEnvelope<MarginInfo>, Vec<MarginPosition>)> {
    let client = reqwest::Client::new();
    let base = rest_url.trim_end_matches('/');
    let info = get_rest::<MarginInfo>(&client, &format!("{base}/margin/info")).await?;
    let info_data = info.data.as_ref().context("margin info missing data")?;
    if info_data.position_count > MAX_POSITIONS {
        bail!(
            "position count {} exceeds contract bound {MAX_POSITIONS}",
            info_data.position_count
        );
    }
    let mut positions = Vec::with_capacity(info_data.position_count as usize);
    for position_id in 1..=info_data.position_count {
        let envelope =
            get_rest::<MarginPosition>(&client, &format!("{base}/margin/positions/{position_id}"))
                .await?;
        let position = envelope
            .data
            .context("margin position response missing data")?;
        if position.position_id != position_id {
            bail!(
                "position endpoint mismatch: requested {position_id}, received {}",
                position.position_id
            );
        }
        positions.push(position);
    }
    Ok((info, positions))
}

fn load_keypair(path: &Path) -> Result<Keypair> {
    let password = keypair_password_from_env();
    KeypairFile::load_with_password_policy(path, password.as_deref(), true)
        .and_then(|file| file.to_keypair())
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("failed to load keypair {}", path.display()))
}

fn contract_instruction(signer: Pubkey, contract: Pubkey, args: Vec<u8>) -> Result<Instruction> {
    let data = ContractInstruction::Call {
        function: "call".to_string(),
        args,
        value: 0,
    }
    .serialize()
    .map_err(|error| anyhow!("failed to serialize contract instruction: {error}"))?;
    Ok(Instruction {
        program_id: CONTRACT_PROGRAM_ID,
        accounts: vec![signer, contract],
        data,
    })
}

fn migration_args(opcode: u8, caller: &Pubkey, position_id: u64) -> Vec<u8> {
    let mut args = Vec::with_capacity(41);
    args.push(opcode);
    args.extend_from_slice(&caller.0);
    args.extend_from_slice(&position_id.to_le_bytes());
    args
}

async fn build_transaction(
    rpc: &Rpc,
    signer: &Keypair,
    instructions: Vec<Instruction>,
) -> Result<Transaction> {
    let message = Message {
        instructions,
        recent_blockhash: rpc.recent_blockhash().await?,
        compute_budget: Some(1_400_000),
        compute_unit_price: None,
    };
    let chain_id = rpc.chain_id().await?;
    let signature = signer.sign(&message.signing_bytes_for_chain_id(&chain_id));
    Ok(Transaction {
        signatures: vec![signature],
        message,
        tx_type: Default::default(),
    })
}

fn governed_args(opcode: u8, authority: &Pubkey, counts: Option<(u64, u64)>) -> String {
    let mut args = Vec::with_capacity(if counts.is_some() { 49 } else { 33 });
    args.push(opcode);
    args.extend_from_slice(&authority.0);
    if let Some((open, cross)) = counts {
        args.extend_from_slice(&open.to_le_bytes());
        args.extend_from_slice(&cross.to_le_bytes());
    }
    hex::encode(args)
}

fn verify_identity(manifest: &MigrationManifest, chain_id: &str, contract: &str) -> Result<()> {
    if manifest.payload.chain_id != chain_id {
        bail!(
            "chain ID mismatch: manifest {}, RPC {chain_id}",
            manifest.payload.chain_id
        );
    }
    if manifest.payload.margin_contract != contract {
        bail!(
            "contract mismatch: manifest {}, requested {contract}",
            manifest.payload.margin_contract
        );
    }
    Ok(())
}

async fn verify_migrated_state(rest_url: &str, manifest: &MigrationManifest) -> Result<()> {
    let (info_envelope, positions) = load_info_and_positions(rest_url).await?;
    let info = info_envelope.data.context("margin info missing data")?;
    if !info.paused {
        bail!("margin must remain paused through post-activation verification");
    }
    let pre_activation = !info.funding_v2_enabled && !info.cross_v2_enabled;
    let post_activation = info.funding_v2_enabled && info.cross_v2_enabled;
    if !pre_activation && !post_activation {
        bail!("Funding V2 and Cross V2 are partially active");
    }
    if !info.migration_locked {
        bail!("migration lock must remain active through post-activation verification");
    }
    if info.position_count != manifest.payload.position_count
        || info.total_open_interest != manifest.payload.total_open_interest
        || info.total_collateral_escrowed != manifest.payload.total_collateral_escrowed
    {
        bail!("aggregate margin state changed after manifest capture");
    }
    if info.funding_migrated_open_count != manifest.payload.open_position_count
        || info.cross_migrated_open_count != manifest.payload.open_cross_count
    {
        bail!("on-chain migrated counts do not match the signed manifest payload");
    }
    if pre_activation && (info.funding_migration_finalized || info.cross_migration_finalized) {
        bail!("migration was finalized before atomic activation");
    }
    if post_activation && (!info.funding_migration_finalized || !info.cross_migration_finalized) {
        bail!("active V2 engines do not have both sealed migration manifests");
    }
    if info.cross_total_collateral != manifest.payload.total_cross_collateral {
        bail!("Cross V2 collateral does not match legacy cross-position collateral");
    }

    let current: HashMap<u64, &MarginPosition> = positions
        .iter()
        .map(|position| (position.position_id, position))
        .collect();
    for expected in &manifest.payload.positions {
        let position = current
            .get(&expected.position_id)
            .with_context(|| format!("position {} disappeared", expected.position_id))?;
        if position.status != "open"
            || position.trader != expected.trader
            || position.pair_id != expected.pair_id
            || position.side != expected.side
            || position.margin_type != expected.margin_type
            || position.size != expected.size
            || position.entry_price_raw != expected.entry_price_raw
            || position.leverage != expected.leverage
            || !position.funding_v2_migrated
            || (expected.margin_type == "cross" && !position.cross_v2_migrated)
            || (expected.margin_type == "cross" && position.margin != 0)
            || (expected.margin_type == "isolated" && position.margin != expected.margin)
        {
            bail!(
                "position {} failed exact migration parity",
                expected.position_id
            );
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let rpc = Rpc::new(cli.rpc_url);

    match cli.command {
        Command::Manifest {
            margin_contract,
            output,
        } => {
            Pubkey::from_base58(&margin_contract).map_err(anyhow::Error::msg)?;
            let chain_id = rpc.chain_id().await?;
            let (info_envelope, positions) = load_info_and_positions(&cli.rest_url).await?;
            let info = info_envelope.data.context("margin info missing data")?;
            if !info.paused || !info.migration_locked {
                bail!("run governed opcode 51 before capturing the manifest");
            }
            if info.funding_v2_enabled || info.cross_v2_enabled {
                bail!("cannot manifest an already active V2 margin engine");
            }
            if info.funding_migrated_open_count != 0
                || info.cross_migrated_open_count != 0
                || info.funding_migration_finalized
                || info.cross_migration_finalized
                || info.cross_total_collateral != 0
            {
                bail!("migration state is not pristine; refusing to create an ambiguous manifest");
            }
            let (manifest_positions, open_interest, collateral) =
                validate_manifest_positions(&info, &positions)?;
            let open_cross_count = manifest_positions
                .iter()
                .filter(|position| position.margin_type == "cross")
                .count() as u64;
            let total_cross_collateral = manifest_positions
                .iter()
                .filter(|position| position.margin_type == "cross")
                .try_fold(0u64, |sum, position| sum.checked_add(position.margin))
                .context("cross collateral sum overflow")?;
            let payload = ManifestPayload {
                schema: 1,
                chain_id,
                source_slot: info_envelope.slot,
                margin_contract,
                position_count: info.position_count,
                open_position_count: manifest_positions.len() as u64,
                open_cross_count,
                total_open_interest: open_interest,
                total_collateral_escrowed: collateral,
                total_cross_collateral,
                positions: manifest_positions,
            };
            let manifest = MigrationManifest {
                manifest_sha256: manifest_hash(&payload)?,
                payload,
            };
            let encoded = serde_json::to_vec_pretty(&manifest)?;
            std::fs::write(&output, encoded)
                .with_context(|| format!("failed to write {}", output.display()))?;
            println!("manifest={}", output.display());
            println!("sha256={}", manifest.manifest_sha256);
            println!("open_positions={}", manifest.payload.open_position_count);
            println!("open_cross={}", manifest.payload.open_cross_count);
        }
        Command::Migrate {
            margin_contract,
            manifest,
            keypair,
            receipts,
            execute,
            confirmation_attempts,
        } => {
            let manifest = read_manifest(&manifest)?;
            let contract = Pubkey::from_base58(&margin_contract).map_err(anyhow::Error::msg)?;
            let chain_id = rpc.chain_id().await?;
            verify_identity(&manifest, &chain_id, &margin_contract)?;
            let signer = load_keypair(&keypair)?;
            let operator = signer.pubkey();
            let (info_envelope, _) = load_info_and_positions(&cli.rest_url).await?;
            let info = info_envelope.data.context("margin info missing data")?;
            if !info.paused || !info.migration_locked {
                bail!("margin migration lock is not active");
            }
            if info.funding_v2_enabled || info.cross_v2_enabled {
                bail!("V2 activation already occurred");
            }

            let mut receipt_rows = Vec::with_capacity(manifest.payload.positions.len());
            let rest_client = reqwest::Client::new();
            let rest_base = cli.rest_url.trim_end_matches('/');
            for position in &manifest.payload.positions {
                let current = get_rest::<MarginPosition>(
                    &rest_client,
                    &format!("{rest_base}/margin/positions/{}", position.position_id),
                )
                .await?
                .data
                .context("margin position response missing data")?;
                if current.status != "open"
                    || current.trader != position.trader
                    || current.pair_id != position.pair_id
                    || current.side != position.side
                    || current.margin_type != position.margin_type
                    || current.size != position.size
                    || current.entry_price_raw != position.entry_price_raw
                    || current.leverage != position.leverage
                {
                    bail!(
                        "position {} changed after manifest capture",
                        position.position_id
                    );
                }
                if current.cross_v2_migrated && position.margin_type != "cross" {
                    bail!(
                        "isolated position {} has a cross migration marker",
                        position.position_id
                    );
                }

                let mut opcodes = Vec::with_capacity(2);
                let mut instructions = Vec::with_capacity(2);
                if !current.funding_v2_migrated {
                    opcodes.push(41);
                    instructions.push(contract_instruction(
                        operator,
                        contract,
                        migration_args(41, &operator, position.position_id),
                    )?);
                }
                if position.margin_type == "cross" && !current.cross_v2_migrated {
                    opcodes.push(44);
                    instructions.push(contract_instruction(
                        operator,
                        contract,
                        migration_args(44, &operator, position.position_id),
                    )?);
                }
                if instructions.is_empty() {
                    receipt_rows.push(MigrationReceipt {
                        position_id: position.position_id,
                        opcodes,
                        simulated: false,
                        submitted: false,
                        signature: None,
                    });
                    continue;
                }
                let transaction = build_transaction(&rpc, &signer, instructions).await?;
                rpc.simulate(&transaction).await.with_context(|| {
                    format!("position {} migration simulation", position.position_id)
                })?;
                let signature = if execute {
                    let signature = rpc.send(&transaction).await?;
                    rpc.wait_for_confirmation(&signature, confirmation_attempts)
                        .await?;
                    Some(signature)
                } else {
                    None
                };
                receipt_rows.push(MigrationReceipt {
                    position_id: position.position_id,
                    opcodes,
                    simulated: true,
                    submitted: execute,
                    signature,
                });
            }

            let receipt_file = ReceiptFile {
                schema: 1,
                chain_id,
                manifest_sha256: manifest.manifest_sha256.clone(),
                operator: operator.to_base58(),
                execute,
                receipts: receipt_rows,
            };
            std::fs::write(&receipts, serde_json::to_vec_pretty(&receipt_file)?)
                .with_context(|| format!("failed to write {}", receipts.display()))?;
            if execute {
                verify_migrated_state(&cli.rest_url, &manifest).await?;
            }
            println!("receipts={}", receipts.display());
            println!("execute={execute}");
        }
        Command::Verify { manifest } => {
            let manifest = read_manifest(&manifest)?;
            let chain_id = rpc.chain_id().await?;
            verify_identity(&manifest, &chain_id, &manifest.payload.margin_contract)?;
            verify_migrated_state(&cli.rest_url, &manifest).await?;
            println!("migration_verified=true");
            println!("manifest_sha256={}", manifest.manifest_sha256);
        }
        Command::BeginArgs {
            governance_authority,
        } => {
            let authority =
                Pubkey::from_base58(&governance_authority).map_err(anyhow::Error::msg)?;
            println!("args_hex={}", governed_args(51, &authority, None));
        }
        Command::ActivateArgs {
            governance_authority,
            manifest,
        } => {
            let authority =
                Pubkey::from_base58(&governance_authority).map_err(anyhow::Error::msg)?;
            let manifest = read_manifest(&manifest)?;
            println!(
                "args_hex={}",
                governed_args(
                    50,
                    &authority,
                    Some((
                        manifest.payload.open_position_count,
                        manifest.payload.open_cross_count,
                    )),
                )
            );
            println!("manifest_sha256={}", manifest.manifest_sha256);
        }
        Command::CompleteArgs {
            governance_authority,
        } => {
            let authority =
                Pubkey::from_base58(&governance_authority).map_err(anyhow::Error::msg)?;
            println!("args_hex={}", governed_args(52, &authority, None));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn governed_activation_args_are_exact() {
        let authority = Pubkey([7; 32]);
        let encoded = hex::decode(governed_args(50, &authority, Some((9, 3)))).unwrap();
        assert_eq!(encoded.len(), 49);
        assert_eq!(encoded[0], 50);
        assert_eq!(&encoded[1..33], &[7; 32]);
        assert_eq!(u64::from_le_bytes(encoded[33..41].try_into().unwrap()), 9);
        assert_eq!(u64::from_le_bytes(encoded[41..49].try_into().unwrap()), 3);
    }

    #[test]
    fn manifest_checksum_detects_payload_changes() {
        let payload = ManifestPayload {
            schema: 1,
            chain_id: "test".into(),
            source_slot: 1,
            margin_contract: "contract".into(),
            position_count: 0,
            open_position_count: 0,
            open_cross_count: 0,
            total_open_interest: 0,
            total_collateral_escrowed: 0,
            total_cross_collateral: 0,
            positions: vec![],
        };
        let first = manifest_hash(&payload).unwrap();
        let mut changed = payload.clone();
        changed.source_slot = 2;
        assert_ne!(first, manifest_hash(&changed).unwrap());
    }
}
