use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use clap::{Parser, Subcommand};
use lichen_core::{
    keypair_password_from_env, ContractInstruction, Hash, Instruction, Keypair, KeypairFile,
    Message, Pubkey, Transaction,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONTRACT_PROGRAM_ID: Pubkey = Pubkey([0xFF; 32]);
const MIGRATION_TOKEN_SIZE: usize = 73;
const MAX_TOKENS: u64 = 1_000_000;

#[derive(Parser)]
#[command(
    name = "sporepump-v3-migrate",
    about = "Capture, execute, and verify exact SporePump Accounting V3 migrations"
)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8899")]
    rpc_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture every canonical token row and independently derive liabilities.
    Manifest {
        #[arg(long)]
        contract: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Resume permissionless token migration from the exact on-chain cursor.
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
    /// Verify the sealed rows, finalized aggregates, and custody coverage.
    Verify {
        #[arg(long)]
        contract: String,
        #[arg(long)]
        manifest: PathBuf,
    },
    /// Print the governed begin_accounting_v3_migration payload.
    BeginArgs {
        #[arg(long)]
        authority: String,
        #[arg(long)]
        expected_token_count: u64,
    },
    /// Print the governed complete_accounting_v3_migration payload.
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
struct SporePumpStats {
    token_count: u64,
    #[serde(default)]
    platform_fees_raw: u64,
    #[serde(default)]
    curve_reserve_raw: u64,
    #[serde(default)]
    creator_liability_raw: u64,
    #[serde(default)]
    cumulative_graduation_revenue_raw: u64,
    #[serde(default)]
    total_graduated: u64,
    #[serde(default)]
    accounting_version: u64,
    #[serde(default)]
    accounting_migration_locked: bool,
    #[serde(default)]
    accounting_migration_expected: u64,
    #[serde(default)]
    accounting_migration_cursor: u64,
    #[serde(default)]
    paused: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ManifestToken {
    token_id: u64,
    creator: String,
    supply_sold: u64,
    licn_raised: u64,
    max_supply: u64,
    created_slot: u64,
    lifecycle_state: u8,
    creator_royalty: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestPayload {
    schema: u64,
    chain_id: String,
    source_slot: u64,
    contract: String,
    token_count: u64,
    legacy_fees: u64,
    cumulative_graduation_revenue: u64,
    curve_reserve: u64,
    creator_liability: u64,
    platform_fees: u64,
    total_graduated: u64,
    total_obligations: u64,
    custody_balance: u64,
    tokens: Vec<ManifestToken>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrationManifest {
    manifest_sha256: String,
    #[serde(flatten)]
    payload: ManifestPayload,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct MigrationReceipt {
    token_id: u64,
    signature: String,
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

    async fn stats(&self) -> Result<SporePumpStats> {
        serde_json::from_value(self.call("getSporePumpStats", json!([])).await?)
            .context("failed to decode getSporePumpStats")
    }

    async fn readonly(&self, contract: &str, function: &str, args: Vec<u8>) -> Result<Vec<u8>> {
        let args = base64::engine::general_purpose::STANDARD.encode(args);
        let result = self
            .call("callContract", json!([contract, function, args]))
            .await?;
        let code = result
            .get("returnCode")
            .or_else(|| result.get("return_code"))
            .and_then(serde_json::Value::as_i64)
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
            .and_then(serde_json::Value::as_i64)
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

fn read_u64(data: &[u8], offset: usize, field: &str) -> Result<u64> {
    data.get(offset..offset + 8)
        .with_context(|| field.to_string())?
        .try_into()
        .map(u64::from_le_bytes)
        .map_err(|_| anyhow!("invalid {field}"))
}

fn decode_token(token_id: u64, data: &[u8]) -> Result<ManifestToken> {
    if data.len() != MIGRATION_TOKEN_SIZE {
        bail!(
            "token {token_id} migration payload has {} bytes, expected {MIGRATION_TOKEN_SIZE}",
            data.len()
        );
    }
    let lifecycle_state = data[64];
    if !matches!(lifecycle_state, 0 | 1 | 3) {
        bail!("token {token_id} has invalid migration lifecycle {lifecycle_state}");
    }
    let creator = Pubkey(data[0..32].try_into().unwrap());
    if creator.0.iter().all(|byte| *byte == 0) {
        bail!("token {token_id} has a zero creator");
    }
    let supply_sold = read_u64(data, 32, "supply sold")?;
    let max_supply = read_u64(data, 48, "max supply")?;
    if supply_sold > max_supply {
        bail!("token {token_id} supply exceeds max supply");
    }
    Ok(ManifestToken {
        token_id,
        creator: creator.to_base58(),
        supply_sold,
        licn_raised: read_u64(data, 40, "LICN raised")?,
        max_supply,
        created_slot: read_u64(data, 56, "created slot")?,
        lifecycle_state,
        creator_royalty: read_u64(data, 65, "creator royalty")?,
    })
}

fn manifest_hash(payload: &ManifestPayload) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(payload)?)))
}

fn validate_manifest_payload(payload: &ManifestPayload) -> Result<()> {
    if payload.chain_id.trim().is_empty() {
        bail!("manifest chain ID is empty");
    }
    let contract = Pubkey::from_base58(&payload.contract).map_err(anyhow::Error::msg)?;
    if contract.0.iter().all(|byte| *byte == 0) {
        bail!("manifest contract is the zero address");
    }
    if payload.token_count > MAX_TOKENS {
        bail!(
            "manifest token count {} exceeds migration bound {MAX_TOKENS}",
            payload.token_count
        );
    }
    let token_count =
        usize::try_from(payload.token_count).context("token count exceeds host limits")?;
    if payload.tokens.len() != token_count {
        bail!(
            "manifest has {} token rows for a frontier of {}",
            payload.tokens.len(),
            payload.token_count
        );
    }

    let mut curve_reserve = 0u64;
    let mut creator_liability = 0u64;
    let mut total_graduated = 0u64;
    for (index, token) in payload.tokens.iter().enumerate() {
        let expected_id = u64::try_from(index)
            .context("token index exceeds u64")?
            .checked_add(1)
            .context("token index overflow")?;
        if token.token_id != expected_id {
            bail!(
                "manifest row {} records token {}, expected contiguous token {expected_id}",
                index + 1,
                token.token_id
            );
        }
        let creator = Pubkey::from_base58(&token.creator).map_err(anyhow::Error::msg)?;
        if creator.0.iter().all(|byte| *byte == 0) {
            bail!("manifest token {expected_id} has a zero creator");
        }
        if token.supply_sold > token.max_supply {
            bail!("manifest token {expected_id} supply exceeds max supply");
        }
        if !matches!(token.lifecycle_state, 0 | 1 | 3) {
            bail!(
                "manifest token {expected_id} has invalid lifecycle {}",
                token.lifecycle_state
            );
        }
        if token.lifecycle_state == 3 {
            total_graduated = total_graduated
                .checked_add(1)
                .context("graduated count overflow")?;
        } else {
            curve_reserve = curve_reserve
                .checked_add(token.licn_raised)
                .context("curve reserve overflow")?;
        }
        creator_liability = creator_liability
            .checked_add(token.creator_royalty)
            .context("creator liability overflow")?;
    }

    let platform_fees = payload
        .legacy_fees
        .checked_sub(creator_liability)
        .and_then(|fees| fees.checked_add(payload.cumulative_graduation_revenue))
        .context("manifest legacy fees cannot fund creator reclassification")?;
    let total_obligations = curve_reserve
        .checked_add(creator_liability)
        .and_then(|value| value.checked_add(platform_fees))
        .context("manifest total obligation overflow")?;
    if payload.curve_reserve != curve_reserve
        || payload.creator_liability != creator_liability
        || payload.platform_fees != platform_fees
        || payload.total_graduated != total_graduated
        || payload.total_obligations != total_obligations
    {
        bail!("manifest aggregates do not match independently derived token liabilities");
    }
    if payload.custody_balance < total_obligations {
        bail!("manifest records insolvent SporePump custody");
    }
    Ok(())
}

fn read_manifest(path: &Path) -> Result<MigrationManifest> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let manifest: MigrationManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to decode {}", path.display()))?;
    if manifest.payload.schema != 1 {
        bail!("unsupported manifest schema {}", manifest.payload.schema);
    }
    let expected = manifest_hash(&manifest.payload)?;
    if manifest.manifest_sha256 != expected {
        bail!("manifest checksum mismatch: expected {expected}");
    }
    validate_manifest_payload(&manifest.payload)?;
    Ok(manifest)
}

/// Persist operator evidence without exposing a partially written JSON file.
/// Sealed manifests never replace an existing path; resumable receipts replace
/// the previous complete snapshot only after the new file is flushed.
fn write_json_atomic<T: Serialize>(path: &Path, value: &T, replace: bool) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let parent = path
        .parent()
        .filter(|candidate| !candidate.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        bail!("output directory {} does not exist", parent.display());
    }
    if !replace && path.exists() {
        bail!("refusing to replace sealed output {}", path.display());
    }
    let name = path
        .file_name()
        .context("output path has no file name")?
        .to_string_lossy();
    let mut pending = None;
    for attempt in 0..100u32 {
        let candidate = parent.join(format!(".{name}.tmp-{}-{attempt}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                pending = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to create temporary output beside {}",
                        path.display()
                    )
                });
            }
        }
    }
    let (temporary, mut file) = pending.context("could not allocate a unique temporary output")?;
    let result = (|| -> Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        if replace {
            std::fs::rename(&temporary, path)?;
        } else {
            // A hard link provides atomic no-clobber publication on the same
            // filesystem. The private temporary name is removed afterwards.
            std::fs::hard_link(&temporary, path)?;
            std::fs::remove_file(&temporary)?;
        }
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.with_context(|| format!("failed to atomically write {}", path.display()))
}

fn validate_receipts(
    receipts: &[MigrationReceipt],
    on_chain_cursor: u64,
    expected_token_count: u64,
) -> Result<()> {
    if on_chain_cursor > expected_token_count {
        bail!(
            "on-chain migration cursor {on_chain_cursor} exceeds token frontier {expected_token_count}"
        );
    }
    let cursor =
        usize::try_from(on_chain_cursor).context("migration cursor exceeds host limits")?;
    if receipts.len() != cursor {
        bail!(
            "receipt count {} does not match on-chain migration cursor {on_chain_cursor}",
            receipts.len()
        );
    }
    for (index, receipt) in receipts.iter().enumerate() {
        let expected_id = u64::try_from(index)
            .context("receipt index exceeds u64")?
            .checked_add(1)
            .context("receipt index overflow")?;
        if receipt.token_id != expected_id {
            bail!(
                "receipt {} records token {}, expected contiguous token {expected_id}",
                index + 1,
                receipt.token_id
            );
        }
        if receipt.signature.trim().is_empty() {
            bail!("receipt for token {expected_id} has an empty signature");
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

async fn custody_status(rpc: &Rpc, contract: &str) -> Result<(u64, u64, u64)> {
    let data = rpc
        .readonly(contract, "get_custody_status", Vec::new())
        .await?;
    if data.len() != 24 {
        bail!(
            "get_custody_status returned {} bytes, expected 24",
            data.len()
        );
    }
    Ok((
        read_u64(&data, 0, "custody balance")?,
        read_u64(&data, 8, "reported obligations")?,
        read_u64(&data, 16, "reported surplus")?,
    ))
}

async fn capture_manifest(rpc: &Rpc, contract: &str) -> Result<MigrationManifest> {
    Pubkey::from_base58(contract).map_err(anyhow::Error::msg)?;
    let stats = rpc.stats().await?;
    if stats.token_count > MAX_TOKENS {
        bail!(
            "token count {} exceeds migration bound {MAX_TOKENS}",
            stats.token_count
        );
    }
    if !stats.paused || !stats.accounting_migration_locked || stats.accounting_migration_cursor != 0
    {
        bail!("capture requires a paused, locked migration at cursor zero");
    }
    if stats.accounting_migration_expected != stats.token_count {
        bail!("migration expected count does not match token count");
    }

    let mut tokens = Vec::with_capacity(stats.token_count as usize);
    let mut curve_reserve = 0u64;
    let mut creator_liability = 0u64;
    let mut total_graduated = 0u64;
    for token_id in 1..=stats.token_count {
        let id = token_id.to_le_bytes();
        let token = decode_token(
            token_id,
            &rpc.readonly(
                contract,
                "get_accounting_migration_token",
                layout_args(&[8], &[&id]),
            )
            .await?,
        )?;
        if token.lifecycle_state == 3 {
            total_graduated = total_graduated
                .checked_add(1)
                .context("graduated count overflow")?;
        } else {
            curve_reserve = curve_reserve
                .checked_add(token.licn_raised)
                .context("curve reserve overflow")?;
        }
        creator_liability = creator_liability
            .checked_add(token.creator_royalty)
            .context("creator liability overflow")?;
        tokens.push(token);
    }
    let platform_fees = stats
        .platform_fees_raw
        .checked_sub(creator_liability)
        .and_then(|fees| fees.checked_add(stats.cumulative_graduation_revenue_raw))
        .context("legacy fees cannot fund creator reclassification plus graduation revenue")?;
    let total_obligations = curve_reserve
        .checked_add(creator_liability)
        .and_then(|value| value.checked_add(platform_fees))
        .context("total obligation overflow")?;
    let (custody_balance, _, _) = custody_status(rpc, contract).await?;
    if custody_balance < total_obligations {
        bail!(
            "SporePump custody is insolvent: balance {custody_balance}, obligations {total_obligations}"
        );
    }

    let payload = ManifestPayload {
        schema: 1,
        chain_id: rpc.chain_id().await?,
        source_slot: rpc.slot().await?,
        contract: contract.to_string(),
        token_count: stats.token_count,
        legacy_fees: stats.platform_fees_raw,
        cumulative_graduation_revenue: stats.cumulative_graduation_revenue_raw,
        curve_reserve,
        creator_liability,
        platform_fees,
        total_graduated,
        total_obligations,
        custody_balance,
        tokens,
    };
    validate_manifest_payload(&payload)?;
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let rpc = Rpc::new(cli.rpc_url);
    match cli.command {
        Command::Manifest { contract, output } => {
            let manifest = capture_manifest(&rpc, &contract).await?;
            write_json_atomic(&output, &manifest, false)?;
            println!("manifest={}", output.display());
            println!("sha256={}", manifest.manifest_sha256);
            println!("token_count={}", manifest.payload.token_count);
            println!("curve_reserve={}", manifest.payload.curve_reserve);
            println!("creator_liability={}", manifest.payload.creator_liability);
            println!("platform_fees={}", manifest.payload.platform_fees);
            println!("total_obligations={}", manifest.payload.total_obligations);
            println!("custody_balance={}", manifest.payload.custody_balance);
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
            if !stats.paused || !stats.accounting_migration_locked {
                bail!("SporePump is not paused and migration-locked");
            }
            if stats.accounting_migration_expected != manifest.payload.token_count
                || stats.token_count != manifest.payload.token_count
            {
                bail!("on-chain token frontier differs from manifest");
            }

            let mut rows = if receipts.exists() {
                serde_json::from_slice::<Vec<MigrationReceipt>>(&std::fs::read(&receipts)?)?
            } else {
                Vec::new()
            };
            let start = stats.accounting_migration_cursor;
            validate_receipts(&rows, start, manifest.payload.token_count)?;
            for token_id in start + 1..=manifest.payload.token_count {
                let id = token_id.to_le_bytes();
                let instruction = contract_instruction(
                    signer.pubkey(),
                    contract_key,
                    "migrate_accounting_v3_token",
                    layout_args(&[8], &[&id]),
                )?;
                let transaction = build_transaction(&rpc, &signer, instruction).await?;
                rpc.simulate(&transaction).await?;
                if !execute {
                    println!("dry_run_next_token={token_id}");
                    println!("remaining={}", manifest.payload.token_count - token_id + 1);
                    return Ok(());
                }
                let signature = rpc.send(&transaction).await?;
                rpc.wait_for_confirmation(&signature, confirmation_attempts)
                    .await?;
                let after = rpc.stats().await?;
                if after.accounting_migration_cursor != token_id {
                    bail!("confirmed transaction did not advance migration cursor");
                }
                rows.push(MigrationReceipt {
                    token_id,
                    signature,
                });
                write_json_atomic(&receipts, &rows, true)?;
            }
            println!("migrated_tokens={}", manifest.payload.token_count - start);
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
            if stats.token_count != manifest.payload.token_count
                || stats.curve_reserve_raw != manifest.payload.curve_reserve
                || stats.creator_liability_raw != manifest.payload.creator_liability
                || stats.platform_fees_raw != manifest.payload.platform_fees
                || stats.cumulative_graduation_revenue_raw
                    != manifest.payload.cumulative_graduation_revenue
                || stats.total_graduated != manifest.payload.total_graduated
                || stats.accounting_version != 3
                || stats.accounting_migration_locked
                || stats.accounting_migration_expected != manifest.payload.token_count
                || stats.accounting_migration_cursor != manifest.payload.token_count
                || !stats.paused
            {
                bail!("final SporePump accounting state does not match the sealed manifest");
            }
            for expected in &manifest.payload.tokens {
                let id = expected.token_id.to_le_bytes();
                let current = decode_token(
                    expected.token_id,
                    &rpc.readonly(
                        &contract,
                        "get_accounting_migration_token",
                        layout_args(&[8], &[&id]),
                    )
                    .await?,
                )?;
                if &current != expected {
                    bail!("token {} changed during migration", expected.token_id);
                }
            }
            let (balance, obligations, surplus) = custody_status(&rpc, &contract).await?;
            if balance < manifest.payload.total_obligations
                || obligations != manifest.payload.total_obligations
                || surplus != balance - obligations
            {
                bail!("final custody proof does not match the sealed manifest");
            }
            println!("verification=ok");
            println!("sha256={}", manifest.manifest_sha256);
        }
        Command::BeginArgs {
            authority,
            expected_token_count,
        } => {
            let authority = Pubkey::from_base58(&authority).map_err(anyhow::Error::msg)?;
            let count = expected_token_count.to_le_bytes();
            governed_payload(
                "begin_accounting_v3_migration",
                layout_args(&[32, 8], &[&authority.0, &count]),
            );
        }
        Command::CompleteArgs {
            authority,
            manifest,
        } => {
            let authority = Pubkey::from_base58(&authority).map_err(anyhow::Error::msg)?;
            let manifest = read_manifest(&manifest)?;
            println!("manifest_sha256={}", manifest.manifest_sha256);
            println!(
                "expected_obligations={}",
                manifest.payload.total_obligations
            );
            governed_payload(
                "complete_accounting_v3_migration",
                layout_args(&[32], &[&authority.0]),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_decoder_preserves_exact_manifest_layout() {
        let mut data = vec![0u8; MIGRATION_TOKEN_SIZE];
        data[0..32].copy_from_slice(&[1u8; 32]);
        data[32..40].copy_from_slice(&100u64.to_le_bytes());
        data[40..48].copy_from_slice(&200u64.to_le_bytes());
        data[48..56].copy_from_slice(&300u64.to_le_bytes());
        data[56..64].copy_from_slice(&400u64.to_le_bytes());
        data[64] = 1;
        data[65..73].copy_from_slice(&5u64.to_le_bytes());
        let token = decode_token(7, &data).expect("decode token");
        assert_eq!(token.token_id, 7);
        assert_eq!(token.supply_sold, 100);
        assert_eq!(token.licn_raised, 200);
        assert_eq!(token.max_supply, 300);
        assert_eq!(token.created_slot, 400);
        assert_eq!(token.lifecycle_state, 1);
        assert_eq!(token.creator_royalty, 5);
        assert!(decode_token(7, &data[..72]).is_err());
        data[64] = 2;
        assert!(decode_token(7, &data).is_err());
    }

    #[test]
    fn governed_payload_layouts_are_unambiguous() {
        let authority = Pubkey([3u8; 32]);
        let count = 11u64.to_le_bytes();
        let begin = layout_args(&[32, 8], &[&authority.0, &count]);
        assert_eq!(&begin[..3], &[0xAB, 32, 8]);
        assert_eq!(&begin[3..35], &authority.0);
        assert_eq!(&begin[35..43], &11u64.to_le_bytes());
        assert_eq!(layout_args(&[32], &[&authority.0]).len(), 34);
    }

    #[test]
    fn manifest_hash_changes_with_any_liability() {
        let payload = ManifestPayload {
            schema: 1,
            chain_id: "test".into(),
            source_slot: 1,
            contract: Pubkey([4u8; 32]).to_base58(),
            token_count: 0,
            legacy_fees: 0,
            cumulative_graduation_revenue: 0,
            curve_reserve: 0,
            creator_liability: 0,
            platform_fees: 0,
            total_graduated: 0,
            total_obligations: 0,
            custody_balance: 0,
            tokens: Vec::new(),
        };
        let first = manifest_hash(&payload).expect("hash");
        let mut changed = payload;
        changed.creator_liability = 1;
        assert_ne!(first, manifest_hash(&changed).expect("changed hash"));
    }

    #[test]
    fn receipts_must_exactly_match_the_contiguous_on_chain_cursor() {
        let receipts = vec![
            MigrationReceipt {
                token_id: 1,
                signature: "first".into(),
            },
            MigrationReceipt {
                token_id: 2,
                signature: "second".into(),
            },
        ];
        validate_receipts(&receipts, 2, 3).expect("valid receipts");
        assert!(validate_receipts(&receipts, 1, 3).is_err());
        assert!(validate_receipts(&receipts, 3, 3).is_err());

        let mut non_contiguous = receipts.clone();
        non_contiguous[1].token_id = 3;
        assert!(validate_receipts(&non_contiguous, 2, 3).is_err());

        let mut unsigned = receipts;
        unsigned[1].signature.clear();
        assert!(validate_receipts(&unsigned, 2, 3).is_err());
        assert!(validate_receipts(&[], 4, 3).is_err());
    }

    #[test]
    fn operator_evidence_is_atomic_and_sealed_manifests_are_no_clobber() {
        let directory = tempfile::tempdir().expect("temp directory");
        let output = directory.path().join("evidence.json");
        let first = vec![MigrationReceipt {
            token_id: 1,
            signature: "first".into(),
        }];
        let second = vec![
            first[0].clone(),
            MigrationReceipt {
                token_id: 2,
                signature: "second".into(),
            },
        ];

        write_json_atomic(&output, &first, false).expect("initial sealed write");
        assert!(write_json_atomic(&output, &second, false).is_err());
        assert_eq!(
            serde_json::from_slice::<Vec<MigrationReceipt>>(
                &std::fs::read(&output).expect("read first")
            )
            .expect("decode first"),
            first
        );

        write_json_atomic(&output, &second, true).expect("atomic receipt replacement");
        assert_eq!(
            serde_json::from_slice::<Vec<MigrationReceipt>>(
                &std::fs::read(&output).expect("read second")
            )
            .expect("decode second"),
            second
        );
        assert_eq!(
            std::fs::read_dir(directory.path())
                .expect("list directory")
                .count(),
            1
        );
    }

    #[test]
    fn manifest_semantics_rederive_every_aggregate() {
        let creator = Pubkey([5u8; 32]).to_base58();
        let mut payload = ManifestPayload {
            schema: 1,
            chain_id: "test".into(),
            source_slot: 1,
            contract: Pubkey([4u8; 32]).to_base58(),
            token_count: 2,
            legacy_fees: 30,
            cumulative_graduation_revenue: 7,
            curve_reserve: 20,
            creator_liability: 5,
            platform_fees: 32,
            total_graduated: 1,
            total_obligations: 57,
            custody_balance: 60,
            tokens: vec![
                ManifestToken {
                    token_id: 1,
                    creator: creator.clone(),
                    supply_sold: 1,
                    licn_raised: 20,
                    max_supply: 2,
                    created_slot: 1,
                    lifecycle_state: 0,
                    creator_royalty: 2,
                },
                ManifestToken {
                    token_id: 2,
                    creator,
                    supply_sold: 2,
                    licn_raised: 99,
                    max_supply: 2,
                    created_slot: 2,
                    lifecycle_state: 3,
                    creator_royalty: 3,
                },
            ],
        };
        validate_manifest_payload(&payload).expect("valid manifest");
        payload.total_obligations += 1;
        assert!(validate_manifest_payload(&payload).is_err());
        payload.total_obligations -= 1;
        payload.tokens[1].token_id = 3;
        assert!(validate_manifest_payload(&payload).is_err());
    }
}
