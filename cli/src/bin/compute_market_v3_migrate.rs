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
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONTRACT_PROGRAM_ID: Pubkey = Pubkey([0xFF; 32]);
const JOB_SIZE: usize = 161;
const MAX_JOBS: u64 = 1_000_000;

#[derive(Parser)]
#[command(
    name = "compute-market-v3-migrate",
    about = "Capture, execute, and verify exact Compute Market Accounting V3 migrations"
)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8899")]
    rpc_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture every immutable job, deferred payout, fee, and custody obligation.
    Manifest {
        #[arg(long)]
        contract: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Resume permissionless job migration from the exact on-chain cursor.
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
    /// Verify the sealed source records and finalized aggregate accounting.
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
        expected_job_count: u64,
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
struct ComputeMarketStats {
    job_count: u64,
    #[serde(default)]
    payment_token: Option<String>,
    #[serde(default)]
    token_config_valid: bool,
    #[serde(default)]
    accounting_version: Option<u64>,
    #[serde(default)]
    migration_locked: Option<bool>,
    #[serde(default)]
    migration_expected_jobs: Option<u64>,
    #[serde(default)]
    migration_cursor: Option<u64>,
    #[serde(default)]
    escrow_liability: Option<u64>,
    #[serde(default)]
    unpaid_liability: Option<u64>,
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
struct ManifestJob {
    job_id: u64,
    record_base64: String,
    requester: String,
    provider: Option<String>,
    status: u8,
    escrow: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ManifestRecipient {
    recipient: String,
    unpaid: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestPayload {
    schema: u64,
    chain_id: String,
    source_slot: u64,
    contract: String,
    payment_token: String,
    job_count: u64,
    escrow_liability: u64,
    unpaid_liability: u64,
    platform_fees: u64,
    total_liability: u64,
    custody_balance_at_source: u64,
    jobs: Vec<ManifestJob>,
    recipients: Vec<ManifestRecipient>,
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
    job_id: u64,
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

    async fn stats(&self) -> Result<ComputeMarketStats> {
        serde_json::from_value(self.call("getComputeMarketStats", json!([])).await?)
            .context("failed to decode getComputeMarketStats")
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

fn read_u64(data: &[u8], field: &str) -> Result<u64> {
    if data.len() != 8 {
        bail!("{field} payload must be exactly 8 bytes");
    }
    Ok(u64::from_le_bytes(
        data.try_into().expect("exact u64 length checked"),
    ))
}

fn decode_address(data: &[u8], field: &str) -> Result<String> {
    let bytes: [u8; 32] = data
        .try_into()
        .with_context(|| format!("{field} must be exactly 32 bytes"))?;
    Ok(Pubkey(bytes).to_base58())
}

fn decode_job(job_id: u64, data: &[u8], escrow: u64) -> Result<ManifestJob> {
    if data.len() != JOB_SIZE {
        bail!("job {job_id} payload must be exactly {JOB_SIZE} bytes");
    }
    let status = data[80];
    if status > 6 {
        bail!("job {job_id} has unknown status {status}");
    }
    let active = status <= 3;
    if (active && escrow == 0) || (!active && escrow != 0) {
        bail!("job {job_id} status and escrow are inconsistent");
    }
    let requester = decode_address(&data[0..32], "requester")?;
    let provider_bytes: [u8; 32] = data[81..113].try_into().expect("fixed job layout");
    let provider = if provider_bytes.iter().all(|byte| *byte == 0) {
        None
    } else {
        Some(Pubkey(provider_bytes).to_base58())
    };
    Ok(ManifestJob {
        job_id,
        record_base64: base64::engine::general_purpose::STANDARD.encode(data),
        requester,
        provider,
        status,
        escrow,
    })
}

fn manifest_hash(payload: &ManifestPayload) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(payload)?)))
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
    if stats.job_count > MAX_JOBS {
        bail!(
            "job count {} exceeds migration bound {MAX_JOBS}",
            stats.job_count
        );
    }
    if !stats.paused || stats.migration_locked != Some(true) || stats.migration_cursor != Some(0) {
        bail!("capture requires a paused, locked migration at cursor zero");
    }
    if stats.migration_expected_jobs != Some(stats.job_count) {
        bail!("migration expected count does not match job count");
    }
    if !stats.token_config_valid {
        bail!("payment token configuration is missing or malformed");
    }
    let payment_token = stats
        .payment_token
        .clone()
        .context("Compute Market stats omitted payment_token")?;
    let token = Pubkey::from_base58(&payment_token).map_err(anyhow::Error::msg)?;

    let mut jobs = Vec::with_capacity(usize::try_from(stats.job_count)?);
    let mut recipients = BTreeMap::<String, u64>::new();
    let mut escrow_liability = 0u64;
    for job_id in 0..stats.job_count {
        let id = job_id.to_le_bytes();
        let record = rpc
            .readonly(contract, "get_job", layout_args(&[0x08], &[&id]))
            .await?;
        let escrow = read_u64(
            &rpc.readonly(contract, "get_escrow", layout_args(&[0x08], &[&id]))
                .await?,
            "escrow",
        )?;
        let job = decode_job(job_id, &record, escrow)?;
        escrow_liability = escrow_liability
            .checked_add(escrow)
            .context("escrow liability overflow")?;
        recipients.entry(job.requester.clone()).or_insert(0);
        if let Some(provider) = &job.provider {
            recipients.entry(provider.clone()).or_insert(0);
        }
        jobs.push(job);
    }

    let mut unpaid_liability = 0u64;
    for (recipient, unpaid) in &mut recipients {
        let recipient_key = Pubkey::from_base58(recipient).map_err(anyhow::Error::msg)?;
        *unpaid = read_u64(
            &rpc.readonly(
                contract,
                "get_unpaid_payout",
                layout_args(&[0x20, 0x20], &[&token.0, &recipient_key.0]),
            )
            .await?,
            "unpaid payout",
        )?;
        unpaid_liability = unpaid_liability
            .checked_add(*unpaid)
            .context("unpaid liability overflow")?;
    }

    let platform_fees = stats
        .platform_fees
        .context("platform fee ledger is malformed")?;
    let total_liability = escrow_liability
        .checked_add(unpaid_liability)
        .and_then(|value| value.checked_add(platform_fees))
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
        job_count: stats.job_count,
        escrow_liability,
        unpaid_liability,
        platform_fees,
        total_liability,
        custody_balance_at_source: custody_balance,
        jobs,
        recipients: recipients
            .into_iter()
            .map(|(recipient, unpaid)| ManifestRecipient { recipient, unpaid })
            .collect(),
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let rpc = Rpc::new(cli.rpc_url);
    match cli.command {
        Command::Manifest { contract, output } => {
            let manifest = capture_manifest(&rpc, &contract).await?;
            std::fs::write(&output, serde_json::to_vec_pretty(&manifest)?)
                .with_context(|| format!("failed to write {}", output.display()))?;
            println!("manifest={}", output.display());
            println!("sha256={}", manifest.manifest_sha256);
            println!("job_count={}", manifest.payload.job_count);
            println!("escrow_liability={}", manifest.payload.escrow_liability);
            println!("unpaid_liability={}", manifest.payload.unpaid_liability);
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
                bail!("Compute Market is not paused and migration-locked");
            }
            if stats.migration_expected_jobs != Some(manifest.payload.job_count)
                || stats.job_count != manifest.payload.job_count
                || stats.payment_token.as_deref() != Some(&manifest.payload.payment_token)
            {
                bail!("on-chain job frontier or payment token differs from manifest");
            }

            let mut rows = if receipts.exists() {
                serde_json::from_slice::<Vec<MigrationReceipt>>(&std::fs::read(&receipts)?)?
            } else {
                Vec::new()
            };
            let start = stats
                .migration_cursor
                .context("migration cursor is malformed")?;
            if start > manifest.payload.job_count {
                bail!("migration cursor exceeds sealed job frontier");
            }
            if start == manifest.payload.job_count {
                println!("migrated_jobs=0");
                println!("remaining=0");
                return Ok(());
            }
            for job_id in start..manifest.payload.job_count {
                let id = job_id.to_le_bytes();
                let instruction = contract_instruction(
                    signer.pubkey(),
                    contract_key,
                    "migrate_accounting_v3_job",
                    layout_args(&[0x08], &[&id]),
                )?;
                let transaction = build_transaction(&rpc, &signer, instruction).await?;
                rpc.simulate(&transaction).await?;
                if !execute {
                    println!("dry_run_next_job={job_id}");
                    println!("remaining={}", manifest.payload.job_count - job_id);
                    return Ok(());
                }
                let signature = rpc.send(&transaction).await?;
                rpc.wait_for_confirmation(&signature, confirmation_attempts)
                    .await?;
                let after = rpc.stats().await?;
                if after.migration_cursor != Some(job_id + 1) {
                    bail!("confirmed transaction did not advance migration cursor");
                }
                rows.push(MigrationReceipt { job_id, signature });
                std::fs::write(&receipts, serde_json::to_vec_pretty(&rows)?)?;
            }
            println!("migrated_jobs={}", manifest.payload.job_count - start);
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
            if stats.job_count != manifest.payload.job_count
                || stats.payment_token.as_deref() != Some(&manifest.payload.payment_token)
                || stats.escrow_liability != Some(manifest.payload.escrow_liability)
                || stats.unpaid_liability != Some(manifest.payload.unpaid_liability)
                || stats.platform_fees != Some(manifest.payload.platform_fees)
                || stats.total_liability != Some(manifest.payload.total_liability)
                || stats.accounting_version != Some(3)
                || stats.migration_locked != Some(false)
                || !stats.paused
                || !stats.accounting_ready
                || !stats.solvent
            {
                bail!("final Compute Market accounting state does not match the sealed manifest");
            }
            if stats
                .custody_balance
                .is_none_or(|custody| custody < manifest.payload.total_liability)
            {
                bail!("final custody does not cover the sealed total liability");
            }

            let token =
                Pubkey::from_base58(&manifest.payload.payment_token).map_err(anyhow::Error::msg)?;
            for expected in &manifest.payload.jobs {
                let id = expected.job_id.to_le_bytes();
                let record = rpc
                    .readonly(&contract, "get_job", layout_args(&[0x08], &[&id]))
                    .await?;
                let encoded = base64::engine::general_purpose::STANDARD.encode(&record);
                let escrow = read_u64(
                    &rpc.readonly(&contract, "get_escrow", layout_args(&[0x08], &[&id]))
                        .await?,
                    "escrow",
                )?;
                if encoded != expected.record_base64 || escrow != expected.escrow {
                    bail!("job {} changed during migration", expected.job_id);
                }
            }
            for expected in &manifest.payload.recipients {
                let recipient =
                    Pubkey::from_base58(&expected.recipient).map_err(anyhow::Error::msg)?;
                let unpaid = read_u64(
                    &rpc.readonly(
                        &contract,
                        "get_unpaid_payout",
                        layout_args(&[0x20, 0x20], &[&token.0, &recipient.0]),
                    )
                    .await?,
                    "unpaid payout",
                )?;
                if unpaid != expected.unpaid {
                    bail!("deferred payout changed for {}", expected.recipient);
                }
            }
            println!("verification=ok");
            println!("sha256={}", manifest.manifest_sha256);
        }
        Command::BeginArgs {
            authority,
            expected_job_count,
        } => {
            let authority = Pubkey::from_base58(&authority).map_err(anyhow::Error::msg)?;
            let count = expected_job_count.to_le_bytes();
            governed_payload(
                "begin_accounting_v3_migration",
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
            let unpaid = manifest.payload.unpaid_liability.to_le_bytes();
            let fees = manifest.payload.platform_fees.to_le_bytes();
            let total = manifest.payload.total_liability.to_le_bytes();
            governed_payload(
                "complete_accounting_v3_migration",
                layout_args(
                    &[0x20, 0x08, 0x08, 0x08, 0x08],
                    &[&authority.0, &escrow, &unpaid, &fees, &total],
                ),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_decoder_is_exact_and_rejects_status_escrow_conflicts() {
        let mut record = vec![0u8; JOB_SIZE];
        record[0..32].copy_from_slice(&[1u8; 32]);
        record[80] = 0;
        let job = decode_job(7, &record, 100).expect("decode active job");
        assert_eq!(job.job_id, 7);
        assert_eq!(job.status, 0);
        assert_eq!(job.escrow, 100);
        assert!(decode_job(7, &record, 0).is_err());
        assert!(decode_job(7, &record[..JOB_SIZE - 1], 100).is_err());
        record[80] = 6;
        assert!(decode_job(7, &record, 100).is_err());
        assert!(decode_job(7, &record, 0).is_ok());
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
            job_count: 0,
            escrow_liability: 0,
            unpaid_liability: 0,
            platform_fees: 0,
            total_liability: 0,
            custody_balance_at_source: 0,
            jobs: Vec::new(),
            recipients: Vec::new(),
        };
        let first = manifest_hash(&payload).expect("hash");
        let mut changed = payload;
        changed.platform_fees = 1;
        changed.total_liability = 1;
        assert_ne!(first, manifest_hash(&changed).expect("changed hash"));
    }
}
