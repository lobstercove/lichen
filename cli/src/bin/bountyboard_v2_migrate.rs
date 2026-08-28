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
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONTRACT_PROGRAM_ID: Pubkey = Pubkey([0xFF; 32]);
const BOUNTY_SIZE: usize = 91;
const MIGRATION_RECORD_SIZE: usize = 147;
const MAX_BOUNTIES: u64 = 1_000_000;
const BOUNTY_OPEN: u8 = 0;
const BOUNTY_COMPLETED: u8 = 1;
const BOUNTY_CANCELLED: u8 = 2;

#[derive(Parser)]
#[command(
    name = "bountyboard-v2-migrate",
    about = "Capture, execute, and verify exact BountyBoard Accounting V2 migrations"
)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8899")]
    rpc_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture the immutable bounty frontier, snapshots, fees, and custody.
    Manifest {
        #[arg(long)]
        contract: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Resume permissionless bounty migration from the exact on-chain cursor.
    Migrate {
        #[arg(long)]
        contract: String,
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
    /// Verify source rows, finalized accounting, and custody against a manifest.
    Verify {
        #[arg(long)]
        contract: String,
        #[arg(long)]
        manifest: PathBuf,
    },
    /// Print the governed begin_accounting_v2_migration payload.
    BeginArgs {
        #[arg(long)]
        authority: String,
        #[arg(long)]
        expected_bounty_count: u64,
    },
    /// Print the governed complete_accounting_v2_migration payload.
    CompleteArgs {
        #[arg(long)]
        authority: String,
        #[arg(long)]
        manifest: PathBuf,
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

#[derive(Clone, Debug, Default, Deserialize)]
struct BountyBoardStats {
    bounty_count: u64,
    #[serde(default)]
    payment_token: Option<String>,
    #[serde(default)]
    token_config_valid: bool,
    #[serde(default)]
    accounting_version: Option<u64>,
    #[serde(default)]
    migration_locked: Option<bool>,
    #[serde(default)]
    migration_expected_bounties: Option<u64>,
    #[serde(default)]
    migration_cursor: Option<u64>,
    #[serde(default)]
    migration_escrow: Option<u64>,
    #[serde(default)]
    escrow_liability: Option<u64>,
    #[serde(default)]
    platform_fees: Option<u64>,
    #[serde(default)]
    total_liability: Option<u64>,
    #[serde(default)]
    custody_balance: Option<u64>,
    #[serde(default)]
    accounting_ready: bool,
    #[serde(default)]
    solvent: bool,
    #[serde(default)]
    paused: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ManifestBounty {
    bounty_id: u64,
    bounty_record_base64: String,
    creator: String,
    reward: u64,
    status: u8,
    submission_count: u8,
    approved_idx: u8,
    source_token_snapshot_present: bool,
    source_token_snapshot: Option<String>,
    source_fee_snapshot_present: bool,
    source_fee_bps: Option<u64>,
    expected_token: String,
    expected_fee_bps: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestPayload {
    schema: u64,
    chain_id: String,
    source_slot: u64,
    contract: String,
    payment_token: String,
    bounty_count: u64,
    escrow_liability: u64,
    platform_fees: u64,
    total_liability: u64,
    custody_balance_at_source: u64,
    bounties: Vec<ManifestBounty>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrationManifest {
    manifest_sha256: String,
    #[serde(flatten)]
    payload: ManifestPayload,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrationReceipt {
    bounty_id: u64,
    signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptJournal {
    schema: u64,
    manifest_sha256: String,
    chain_id: String,
    contract: String,
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

    async fn slot(&self) -> Result<u64> {
        let value = self.call("getSlot", json!([])).await?;
        value
            .as_u64()
            .or_else(|| value.get("slot").and_then(serde_json::Value::as_u64))
            .context("getSlot missing slot")
    }

    async fn stats(&self) -> Result<BountyBoardStats> {
        serde_json::from_value(self.call("getBountyBoardStats", json!([])).await?)
            .context("failed to decode getBountyBoardStats")
    }

    async fn readonly(&self, contract: &str, function: &str, args: Vec<u8>) -> Result<Vec<u8>> {
        let args = base64::engine::general_purpose::STANDARD.encode(args);
        let result = self
            .call("callContract", json!([contract, function, args]))
            .await?;
        let code = result
            .get("returnCode")
            .or_else(|| result.get("return_code"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if code != 0 {
            bail!("{function} returned contract code {code}");
        }
        let encoded = result
            .get("returnData")
            .or_else(|| result.get("return_data"))
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("{function} returned no data"))?;
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .with_context(|| format!("{function} returned invalid base64"))
    }

    async fn recent_blockhash(&self) -> Result<Hash> {
        let value = self.call("getRecentBlockhash", json!([])).await?;
        let value = value
            .as_str()
            .or_else(|| value.get("blockhash").and_then(serde_json::Value::as_str))
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
            bail!(
                "preflight failed: {}",
                result
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("success=false")
            );
        }
        if result
            .get("returnCode")
            .or_else(|| result.get("return_code"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            != 0
        {
            bail!("preflight returned a nonzero contract code");
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
                if matches!(
                    value.get("status").and_then(serde_json::Value::as_str),
                    Some("confirmed" | "finalized" | "success")
                ) {
                    return Ok(());
                }
                if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
                    if !error.is_empty() {
                        bail!("transaction {signature} failed: {error}");
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        bail!("transaction {signature} was not confirmed after {attempts} attempts")
    }
}

fn layout_args(layout: &[u8], values: &[&[u8]]) -> Vec<u8> {
    let mut args = Vec::with_capacity(
        1 + layout.len() + values.iter().map(|value| value.len()).sum::<usize>(),
    );
    args.push(0xAB);
    args.extend_from_slice(layout);
    for value in values {
        args.extend_from_slice(value);
    }
    args
}

fn exact_u64(data: &[u8], offset: usize, field: &str) -> Result<u64> {
    let end = offset
        .checked_add(8)
        .with_context(|| format!("{field} offset overflow"))?;
    let bytes: [u8; 8] = data
        .get(offset..end)
        .with_context(|| format!("{field} is truncated"))?
        .try_into()
        .expect("exact u64 slice length");
    Ok(u64::from_le_bytes(bytes))
}

fn exact_flag(data: &[u8], offset: usize, field: &str) -> Result<bool> {
    match exact_u64(data, offset, field)? {
        0 => Ok(false),
        1 => Ok(true),
        value => bail!("{field} must be 0 or 1, got {value}"),
    }
}

fn decode_address(data: &[u8], field: &str) -> Result<String> {
    let bytes: [u8; 32] = data
        .try_into()
        .with_context(|| format!("{field} must be exactly 32 bytes"))?;
    Ok(Pubkey(bytes).to_base58())
}

fn decode_bounty(bounty_id: u64, data: &[u8], canonical_token: &str) -> Result<ManifestBounty> {
    if data.len() != MIGRATION_RECORD_SIZE {
        bail!("bounty {bounty_id} migration record must be exactly {MIGRATION_RECORD_SIZE} bytes");
    }
    let bounty = &data[..BOUNTY_SIZE];
    let creator = decode_address(&bounty[..32], "creator")?;
    let reward = exact_u64(bounty, 64, "reward")?;
    if reward == 0 {
        bail!("bounty {bounty_id} has a zero reward");
    }
    let status = bounty[80];
    let submission_count = bounty[81];
    let approved_idx = bounty[90];
    match status {
        BOUNTY_OPEN if approved_idx == u8::MAX => {}
        BOUNTY_COMPLETED if approved_idx < submission_count => {}
        BOUNTY_CANCELLED if approved_idx == u8::MAX => {}
        _ => bail!("bounty {bounty_id} has inconsistent status metadata"),
    }

    let token_present = exact_flag(data, 91, "token snapshot presence")?;
    let raw_token = decode_address(&data[99..131], "token snapshot")?;
    if token_present && raw_token != canonical_token {
        bail!("bounty {bounty_id} token snapshot differs from the canonical payment token");
    }
    if !token_present && data[99..131].iter().any(|byte| *byte != 0) {
        bail!("bounty {bounty_id} absent token snapshot has nonzero bytes");
    }
    let fee_present = exact_flag(data, 131, "fee snapshot presence")?;
    let raw_fee = exact_u64(data, 139, "fee snapshot")?;
    if raw_fee > 1_000 {
        bail!("bounty {bounty_id} fee snapshot exceeds the 10% cap");
    }
    if !fee_present && raw_fee != 0 {
        bail!("bounty {bounty_id} absent fee snapshot has a nonzero value");
    }

    Ok(ManifestBounty {
        bounty_id,
        bounty_record_base64: base64::engine::general_purpose::STANDARD.encode(bounty),
        creator,
        reward,
        status,
        submission_count,
        approved_idx,
        source_token_snapshot_present: token_present,
        source_token_snapshot: token_present.then_some(raw_token),
        source_fee_snapshot_present: fee_present,
        source_fee_bps: fee_present.then_some(raw_fee),
        expected_token: canonical_token.to_string(),
        expected_fee_bps: if fee_present { raw_fee } else { 0 },
    })
}

fn manifest_hash(payload: &ManifestPayload) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(payload)?)))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("failed to decode {}", path.display()))
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create {}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("output path has no valid file name")?;
    let temp_path = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp_path)
        .with_context(|| format!("failed to create {}", temp_path.display()))?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(&temp_path, path)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn read_manifest(path: &Path) -> Result<MigrationManifest> {
    let manifest: MigrationManifest = read_json(path)?;
    if manifest.payload.schema != 1 {
        bail!("unsupported manifest schema {}", manifest.payload.schema);
    }
    let expected = manifest_hash(&manifest.payload)?;
    if manifest.manifest_sha256 != expected {
        bail!("manifest checksum mismatch: expected {expected}");
    }
    if manifest.payload.bounties.len() != usize::try_from(manifest.payload.bounty_count)? {
        bail!("manifest bounty row count does not match its sealed frontier");
    }
    for (expected_id, bounty) in manifest.payload.bounties.iter().enumerate() {
        if bounty.bounty_id != u64::try_from(expected_id)? {
            bail!("manifest bounty rows are not contiguous and ascending");
        }
    }
    Ok(manifest)
}

fn load_keypair(path: &Path) -> Result<Keypair> {
    let password = keypair_password_from_env();
    KeypairFile::load_with_password_policy(path, password.as_deref(), true)
        .and_then(|file| file.to_keypair())
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("failed to load keypair {}", path.display()))
}

fn contract_instruction(
    signer: Pubkey,
    contract: Pubkey,
    function: &str,
    args: Vec<u8>,
) -> Result<Instruction> {
    let data = ContractInstruction::Call {
        function: function.to_string(),
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

async fn build_transaction(
    rpc: &Rpc,
    signer: &Keypair,
    instruction: Instruction,
) -> Result<Transaction> {
    let message = Message {
        instructions: vec![instruction],
        recent_blockhash: rpc.recent_blockhash().await?,
        compute_budget: Some(1_400_000),
        compute_unit_price: None,
    };
    let chain_id = rpc.chain_id().await?;
    Ok(Transaction {
        signatures: vec![signer.sign(&message.signing_bytes_for_chain_id(&chain_id))],
        message,
        tx_type: Default::default(),
    })
}

async fn capture_manifest(rpc: &Rpc, contract: &str) -> Result<MigrationManifest> {
    Pubkey::from_base58(contract).map_err(anyhow::Error::msg)?;
    let stats = rpc.stats().await?;
    if stats.bounty_count > MAX_BOUNTIES {
        bail!(
            "bounty count {} exceeds migration bound {MAX_BOUNTIES}",
            stats.bounty_count
        );
    }
    if !stats.paused || stats.migration_locked != Some(true) || stats.migration_cursor != Some(0) {
        bail!("capture requires a paused, locked migration at cursor zero");
    }
    if stats.migration_expected_bounties != Some(stats.bounty_count) {
        bail!("migration expected count does not match bounty count");
    }
    if !stats.token_config_valid {
        bail!("payment token configuration is missing or malformed");
    }
    let payment_token = stats
        .payment_token
        .clone()
        .context("BountyBoard stats omitted payment_token")?;
    Pubkey::from_base58(&payment_token).map_err(anyhow::Error::msg)?;

    let mut bounties = Vec::with_capacity(usize::try_from(stats.bounty_count)?);
    let mut escrow_liability = 0u64;
    for bounty_id in 0..stats.bounty_count {
        let id = bounty_id.to_le_bytes();
        let record = rpc
            .readonly(
                contract,
                "get_bounty_migration_record",
                layout_args(&[0x08], &[&id]),
            )
            .await?;
        let bounty = decode_bounty(bounty_id, &record, &payment_token)?;
        if bounty.status == BOUNTY_OPEN {
            escrow_liability = escrow_liability
                .checked_add(bounty.reward)
                .context("escrow liability overflow")?;
        }
        bounties.push(bounty);
    }

    let platform_fees = stats
        .platform_fees
        .context("platform fee ledger is malformed")?;
    let total_liability = escrow_liability
        .checked_add(platform_fees)
        .context("total liability overflow")?;
    let custody_balance = stats
        .custody_balance
        .context("custody balance is unavailable")?;
    if custody_balance < total_liability {
        bail!("custody is below independently reconstructed liabilities");
    }

    let payload = ManifestPayload {
        schema: 1,
        chain_id: rpc.chain_id().await?,
        source_slot: rpc.slot().await?,
        contract: contract.to_string(),
        payment_token,
        bounty_count: stats.bounty_count,
        escrow_liability,
        platform_fees,
        total_liability,
        custody_balance_at_source: custody_balance,
        bounties,
    };
    Ok(MigrationManifest {
        manifest_sha256: manifest_hash(&payload)?,
        payload,
    })
}

fn governed_payload(function: &str, args: Vec<u8>) {
    println!("function={function}");
    println!("args_hex={}", hex::encode(&args));
    println!(
        "args_base64={}",
        base64::engine::general_purpose::STANDARD.encode(args)
    );
}

fn load_receipt_journal(
    path: &Path,
    manifest: &MigrationManifest,
    cursor: u64,
) -> Result<ReceiptJournal> {
    let journal = if path.exists() {
        read_json::<ReceiptJournal>(path)?
    } else {
        ReceiptJournal {
            schema: 1,
            manifest_sha256: manifest.manifest_sha256.clone(),
            chain_id: manifest.payload.chain_id.clone(),
            contract: manifest.payload.contract.clone(),
            receipts: Vec::new(),
        }
    };
    if journal.schema != 1
        || journal.manifest_sha256 != manifest.manifest_sha256
        || journal.chain_id != manifest.payload.chain_id
        || journal.contract != manifest.payload.contract
    {
        bail!("receipt journal identity does not match the sealed manifest");
    }
    let mut previous = None;
    for receipt in &journal.receipts {
        if receipt.bounty_id >= cursor || previous.is_some_and(|id| receipt.bounty_id <= id) {
            bail!("receipt journal is not strictly ascending below the on-chain cursor");
        }
        previous = Some(receipt.bounty_id);
    }
    Ok(journal)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let rpc = Rpc::new(cli.rpc_url);
    match cli.command {
        Command::Manifest { contract, output } => {
            let manifest = capture_manifest(&rpc, &contract).await?;
            write_json_atomic(&output, &manifest)?;
            println!("manifest={}", output.display());
            println!("sha256={}", manifest.manifest_sha256);
            println!("bounty_count={}", manifest.payload.bounty_count);
            println!("escrow_liability={}", manifest.payload.escrow_liability);
            println!("platform_fees={}", manifest.payload.platform_fees);
            println!("total_liability={}", manifest.payload.total_liability);
        }
        Command::Migrate {
            contract,
            manifest,
            keypair,
            receipts,
            execute,
            confirmation_attempts,
        } => {
            let manifest = read_manifest(&manifest)?;
            if manifest.payload.contract != contract
                || manifest.payload.chain_id != rpc.chain_id().await?
            {
                bail!("manifest network or contract identity mismatch");
            }
            let contract_key = Pubkey::from_base58(&contract).map_err(anyhow::Error::msg)?;
            let signer = load_keypair(&keypair)?;
            let stats = rpc.stats().await?;
            if !stats.paused || stats.migration_locked != Some(true) {
                bail!("BountyBoard is not paused and migration-locked");
            }
            if stats.migration_expected_bounties != Some(manifest.payload.bounty_count)
                || stats.bounty_count != manifest.payload.bounty_count
                || stats.payment_token.as_deref() != Some(&manifest.payload.payment_token)
            {
                bail!("on-chain bounty frontier or payment token differs from manifest");
            }

            let start = stats
                .migration_cursor
                .context("migration cursor is malformed")?;
            if start > manifest.payload.bounty_count {
                bail!("migration cursor exceeds sealed bounty frontier");
            }
            let mut journal = load_receipt_journal(&receipts, &manifest, start)?;
            if start == manifest.payload.bounty_count {
                println!("migrated_bounties=0");
                println!("remaining=0");
                return Ok(());
            }
            for bounty_id in start..manifest.payload.bounty_count {
                let id = bounty_id.to_le_bytes();
                let instruction = contract_instruction(
                    signer.pubkey(),
                    contract_key,
                    "migrate_accounting_v2_bounty",
                    layout_args(&[0x08], &[&id]),
                )?;
                let transaction = build_transaction(&rpc, &signer, instruction).await?;
                rpc.simulate(&transaction).await?;
                if !execute {
                    println!("dry_run_next_bounty={bounty_id}");
                    println!("remaining={}", manifest.payload.bounty_count - bounty_id);
                    return Ok(());
                }
                let signature = rpc.send(&transaction).await?;
                rpc.wait_for_confirmation(&signature, confirmation_attempts)
                    .await?;
                let after = rpc.stats().await?;
                if after.migration_cursor != Some(bounty_id + 1)
                    || after.migration_escrow
                        != Some(
                            manifest.payload.bounties[..=usize::try_from(bounty_id)?]
                                .iter()
                                .filter(|bounty| bounty.status == BOUNTY_OPEN)
                                .try_fold(0u64, |sum, bounty| sum.checked_add(bounty.reward))
                                .context("manifest escrow prefix overflow")?,
                        )
                {
                    bail!("confirmed transaction did not produce the sealed migration prefix");
                }
                journal.receipts.push(MigrationReceipt {
                    bounty_id,
                    signature,
                });
                write_json_atomic(&receipts, &journal)?;
            }
            println!(
                "migrated_bounties={}",
                manifest.payload.bounty_count - start
            );
            println!("receipts={}", receipts.display());
        }
        Command::Verify { contract, manifest } => {
            let manifest = read_manifest(&manifest)?;
            if manifest.payload.contract != contract
                || manifest.payload.chain_id != rpc.chain_id().await?
            {
                bail!("manifest network or contract identity mismatch");
            }
            let stats = rpc.stats().await?;
            if stats.bounty_count != manifest.payload.bounty_count
                || stats.payment_token.as_deref() != Some(&manifest.payload.payment_token)
                || stats.escrow_liability != Some(manifest.payload.escrow_liability)
                || stats.platform_fees != Some(manifest.payload.platform_fees)
                || stats.total_liability != Some(manifest.payload.total_liability)
                || stats.accounting_version != Some(2)
                || stats.migration_locked != Some(false)
                || !stats.paused
                || !stats.accounting_ready
                || !stats.solvent
            {
                bail!("final BountyBoard accounting state does not match the sealed manifest");
            }
            if stats
                .custody_balance
                .is_none_or(|custody| custody < manifest.payload.total_liability)
            {
                bail!("final custody does not cover the sealed total liability");
            }

            for expected in &manifest.payload.bounties {
                let id = expected.bounty_id.to_le_bytes();
                let record = rpc
                    .readonly(
                        &contract,
                        "get_bounty_migration_record",
                        layout_args(&[0x08], &[&id]),
                    )
                    .await?;
                let actual =
                    decode_bounty(expected.bounty_id, &record, &manifest.payload.payment_token)?;
                if actual.bounty_record_base64 != expected.bounty_record_base64
                    || !actual.source_token_snapshot_present
                    || actual.source_token_snapshot.as_deref() != Some(&expected.expected_token)
                    || !actual.source_fee_snapshot_present
                    || actual.source_fee_bps != Some(expected.expected_fee_bps)
                {
                    bail!("bounty {} changed during migration", expected.bounty_id);
                }
            }
            println!("verification=ok");
            println!("sha256={}", manifest.manifest_sha256);
        }
        Command::BeginArgs {
            authority,
            expected_bounty_count,
        } => {
            let authority = Pubkey::from_base58(&authority).map_err(anyhow::Error::msg)?;
            let count = expected_bounty_count.to_le_bytes();
            governed_payload(
                "begin_accounting_v2_migration",
                layout_args(&[0x20, 0x08], &[&authority.0, &count]),
            );
        }
        Command::CompleteArgs {
            authority,
            manifest,
        } => {
            let authority = Pubkey::from_base58(&authority).map_err(anyhow::Error::msg)?;
            let manifest = read_manifest(&manifest)?;
            let escrow = manifest.payload.escrow_liability.to_le_bytes();
            let fees = manifest.payload.platform_fees.to_le_bytes();
            let total = manifest.payload.total_liability.to_le_bytes();
            governed_payload(
                "complete_accounting_v2_migration",
                layout_args(
                    &[0x20, 0x08, 0x08, 0x08],
                    &[&authority.0, &escrow, &fees, &total],
                ),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migration_record(status: u8, reward: u64) -> Vec<u8> {
        let mut record = vec![0u8; MIGRATION_RECORD_SIZE];
        record[..32].copy_from_slice(&[1u8; 32]);
        record[64..72].copy_from_slice(&reward.to_le_bytes());
        record[80] = status;
        record[90] = u8::MAX;
        record
    }

    #[test]
    fn bounty_decoder_is_exact_and_rejects_status_conflicts() {
        let token = Pubkey([0u8; 32]).to_base58();
        let record = migration_record(BOUNTY_OPEN, 100);
        let bounty = decode_bounty(7, &record, &token).expect("decode active bounty");
        assert_eq!(bounty.bounty_id, 7);
        assert_eq!(bounty.status, BOUNTY_OPEN);
        assert_eq!(bounty.reward, 100);
        assert!(decode_bounty(7, &record[..MIGRATION_RECORD_SIZE - 1], &token).is_err());

        let mut inconsistent = record;
        inconsistent[90] = 0;
        assert!(decode_bounty(7, &inconsistent, &token).is_err());
    }

    #[test]
    fn bounty_decoder_seals_existing_and_missing_snapshots() {
        let canonical = Pubkey([2u8; 32]).to_base58();
        let missing = migration_record(BOUNTY_OPEN, 100);
        let decoded = decode_bounty(0, &missing, &canonical).expect("missing snapshots");
        assert!(!decoded.source_token_snapshot_present);
        assert!(!decoded.source_fee_snapshot_present);
        assert_eq!(decoded.expected_token, canonical);
        assert_eq!(decoded.expected_fee_bps, 0);

        let mut present = migration_record(BOUNTY_OPEN, 100);
        present[91..99].copy_from_slice(&1u64.to_le_bytes());
        present[99..131].copy_from_slice(&[2u8; 32]);
        present[131..139].copy_from_slice(&1u64.to_le_bytes());
        present[139..147].copy_from_slice(&250u64.to_le_bytes());
        let decoded = decode_bounty(0, &present, &canonical).expect("existing snapshots");
        assert_eq!(
            decoded.source_token_snapshot.as_deref(),
            Some(canonical.as_str())
        );
        assert_eq!(decoded.source_fee_bps, Some(250));
        assert_eq!(decoded.expected_fee_bps, 250);
    }

    #[test]
    fn governed_payload_layouts_are_unambiguous() {
        let authority = Pubkey([3u8; 32]);
        let count = 11u64.to_le_bytes();
        let begin = layout_args(&[0x20, 0x08], &[&authority.0, &count]);
        assert_eq!(&begin[..3], &[0xAB, 0x20, 0x08]);
        assert_eq!(&begin[3..35], &authority.0);
        assert_eq!(&begin[35..43], &11u64.to_le_bytes());
    }

    #[test]
    fn manifest_hash_changes_with_each_liability_class() {
        let payload = ManifestPayload {
            schema: 1,
            chain_id: "test".to_string(),
            source_slot: 1,
            contract: Pubkey([4u8; 32]).to_base58(),
            payment_token: Pubkey([0u8; 32]).to_base58(),
            bounty_count: 0,
            escrow_liability: 0,
            platform_fees: 0,
            total_liability: 0,
            custody_balance_at_source: 0,
            bounties: Vec::new(),
        };
        let first = manifest_hash(&payload).expect("hash");
        let mut changed = payload;
        changed.platform_fees = 1;
        changed.total_liability = 1;
        assert_ne!(first, manifest_hash(&changed).expect("changed hash"));
    }
}
