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
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONTRACT_PROGRAM_ID: Pubkey = Pubkey([0xFF; 32]);
const STREAM_SIZE: usize = 105;
const MAX_STREAMS: u64 = 1_000_000;

#[derive(Parser)]
#[command(
    name = "sporepay-v3-migrate",
    about = "Capture, execute, and verify exact SporePay Accounting V3 migrations"
)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8899")]
    rpc_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture every immutable stream and independently derive all obligations.
    Manifest {
        #[arg(long)]
        contract: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Resume permissionless stream migration from the on-chain cursor.
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
    /// Verify the sealed stream manifest and finalized aggregate accounting.
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
        expected_stream_count: u64,
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
struct SporePayStats {
    stream_count: u64,
    #[serde(default)]
    escrow_liability: u64,
    #[serde(default)]
    unpaid_liability: u64,
    #[serde(default)]
    accounting_version: u64,
    #[serde(default)]
    migration_locked: bool,
    #[serde(default)]
    migration_expected_streams: u64,
    #[serde(default)]
    migration_cursor: u64,
    #[serde(default)]
    paused: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ManifestStream {
    stream_id: u64,
    sender: String,
    recipient: String,
    total_amount: u64,
    withdrawn: u64,
    start_slot: u64,
    end_slot: u64,
    cancelled: bool,
    created_slot: u64,
    cliff_slot: u64,
    unpaid_for_recipient: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestPayload {
    schema: u64,
    chain_id: String,
    source_slot: u64,
    contract: String,
    stream_count: u64,
    escrow_liability: u64,
    unpaid_liability: u64,
    streams: Vec<ManifestStream>,
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
    stream_id: u64,
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

    async fn stats(&self) -> Result<SporePayStats> {
        serde_json::from_value(self.call("getSporePayStats", json!([])).await?)
            .context("failed to decode getSporePayStats")
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
    let mut args =
        Vec::with_capacity(1 + layout.len() + values.iter().map(|v| v.len()).sum::<usize>());
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

fn decode_stream(stream_id: u64, data: &[u8]) -> Result<ManifestStream> {
    if data.len() < STREAM_SIZE {
        bail!("stream {stream_id} payload is shorter than {STREAM_SIZE} bytes");
    }
    let sender = Pubkey(data[0..32].try_into().unwrap()).to_base58();
    let recipient = Pubkey(data[32..64].try_into().unwrap()).to_base58();
    Ok(ManifestStream {
        stream_id,
        sender,
        recipient,
        total_amount: read_u64(data, 64, "total amount")?,
        withdrawn: read_u64(data, 72, "withdrawn amount")?,
        start_slot: read_u64(data, 80, "start slot")?,
        end_slot: read_u64(data, 88, "end slot")?,
        cancelled: data[96] == 1,
        created_slot: read_u64(data, 97, "created slot")?,
        cliff_slot: if data.len() >= 113 {
            read_u64(data, 105, "cliff slot")?
        } else {
            0
        },
        unpaid_for_recipient: 0,
    })
}

async fn read_address_index(
    rpc: &Rpc,
    contract: &str,
    function: &str,
    address: &str,
) -> Result<Vec<u64>> {
    let address = Pubkey::from_base58(address).map_err(anyhow::Error::msg)?;
    let mut cursor = 0u64;
    let mut ids = Vec::new();
    loop {
        let cursor_bytes = cursor.to_le_bytes();
        let limit = 64u64.to_le_bytes();
        let data = rpc
            .readonly(
                contract,
                function,
                layout_args(&[32, 8, 8], &[&address.0, &cursor_bytes, &limit]),
            )
            .await?;
        if data.len() < 24 {
            bail!("{function} returned a short page");
        }
        let total = read_u64(&data, 0, "index total")?;
        let next = read_u64(&data, 8, "index cursor")?;
        let returned = read_u64(&data, 16, "index returned count")?;
        let expected_len = 24usize
            .checked_add(
                usize::try_from(returned)?
                    .checked_mul(8)
                    .context("index page length overflow")?,
            )
            .context("index page length overflow")?;
        if data.len() != expected_len || next < cursor || next > total || next - cursor != returned
        {
            bail!("{function} returned an inconsistent page");
        }
        for index in 0..returned {
            ids.push(read_u64(
                &data,
                24 + usize::try_from(index)? * 8,
                "stream ID",
            )?);
        }
        if next == total {
            if ids.len() != usize::try_from(total)? {
                bail!("{function} total does not match returned IDs");
            }
            return Ok(ids);
        }
        if next == cursor {
            bail!("{function} pagination did not advance");
        }
        cursor = next;
    }
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
    if stats.stream_count > MAX_STREAMS {
        bail!(
            "stream count {} exceeds migration bound {MAX_STREAMS}",
            stats.stream_count
        );
    }
    if !stats.paused || !stats.migration_locked || stats.migration_cursor != 0 {
        bail!("capture requires a paused, locked migration at cursor zero");
    }
    if stats.migration_expected_streams != stats.stream_count {
        bail!("migration expected count does not match stream count");
    }

    let mut streams = Vec::with_capacity(stats.stream_count as usize);
    let mut liability = 0u64;
    let mut unpaid = 0u64;
    let mut visited_recipients = BTreeSet::new();
    for stream_id in 0..stats.stream_count {
        let id = stream_id.to_le_bytes();
        let data = rpc
            .readonly(contract, "get_stream_info", layout_args(&[8], &[&id]))
            .await?;
        let mut stream = decode_stream(stream_id, &data)?;
        let outstanding = stream
            .total_amount
            .checked_sub(stream.withdrawn)
            .with_context(|| format!("stream {stream_id} withdrawn exceeds total"))?;
        if stream.cancelled {
            if visited_recipients.insert(stream.recipient.clone()) {
                let recipient =
                    Pubkey::from_base58(&stream.recipient).map_err(anyhow::Error::msg)?;
                let data = rpc
                    .readonly(
                        contract,
                        "get_unpaid_payout",
                        layout_args(&[32], &[&recipient.0]),
                    )
                    .await?;
                stream.unpaid_for_recipient = read_u64(&data, 0, "unpaid payout")?;
                unpaid = unpaid
                    .checked_add(stream.unpaid_for_recipient)
                    .context("unpaid liability overflow")?;
                liability = liability
                    .checked_add(stream.unpaid_for_recipient)
                    .context("escrow liability overflow")?;
            }
        } else {
            liability = liability
                .checked_add(outstanding)
                .context("escrow liability overflow")?;
        }
        streams.push(stream);
    }
    let payload = ManifestPayload {
        schema: 1,
        chain_id: rpc.chain_id().await?,
        source_slot: rpc.slot().await?,
        contract: contract.to_string(),
        stream_count: stats.stream_count,
        escrow_liability: liability,
        unpaid_liability: unpaid,
        streams,
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
            println!("stream_count={}", manifest.payload.stream_count);
            println!("escrow_liability={}", manifest.payload.escrow_liability);
            println!("unpaid_liability={}", manifest.payload.unpaid_liability);
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
            if !stats.paused || !stats.migration_locked {
                bail!("SporePay is not paused and migration-locked");
            }
            if stats.migration_expected_streams != manifest.payload.stream_count
                || stats.stream_count != manifest.payload.stream_count
            {
                bail!("on-chain stream frontier differs from manifest");
            }

            let mut rows = if receipts.exists() {
                serde_json::from_slice::<Vec<MigrationReceipt>>(&std::fs::read(&receipts)?)?
            } else {
                Vec::new()
            };
            let start = stats.migration_cursor;
            for stream_id in start..manifest.payload.stream_count {
                let id = stream_id.to_le_bytes();
                let instruction = contract_instruction(
                    signer.pubkey(),
                    contract_key,
                    "migrate_accounting_v3_stream",
                    layout_args(&[8], &[&id]),
                )?;
                let transaction = build_transaction(&rpc, &signer, instruction).await?;
                rpc.simulate(&transaction).await?;
                if !execute {
                    println!("dry_run_next_stream={stream_id}");
                    println!("remaining={}", manifest.payload.stream_count - stream_id);
                    return Ok(());
                }
                let signature = rpc.send(&transaction).await?;
                rpc.wait_for_confirmation(&signature, confirmation_attempts)
                    .await?;
                let after = rpc.stats().await?;
                if after.migration_cursor != stream_id + 1 {
                    bail!("confirmed transaction did not advance migration cursor");
                }
                rows.push(MigrationReceipt {
                    stream_id,
                    signature,
                });
                std::fs::write(&receipts, serde_json::to_vec_pretty(&rows)?)?;
            }
            println!("migrated_streams={}", manifest.payload.stream_count - start);
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
            if stats.stream_count != manifest.payload.stream_count
                || stats.escrow_liability != manifest.payload.escrow_liability
                || stats.unpaid_liability != manifest.payload.unpaid_liability
                || stats.accounting_version != 3
                || stats.migration_locked
                || !stats.paused
            {
                bail!("final SporePay accounting state does not match the sealed manifest");
            }
            let mut expected_senders = BTreeMap::<String, Vec<u64>>::new();
            let mut expected_recipients = BTreeMap::<String, Vec<u64>>::new();
            for expected in &manifest.payload.streams {
                let id = expected.stream_id.to_le_bytes();
                let current = decode_stream(
                    expected.stream_id,
                    &rpc.readonly(&contract, "get_stream_info", layout_args(&[8], &[&id]))
                        .await?,
                )?;
                if current.sender != expected.sender
                    || current.recipient != expected.recipient
                    || current.total_amount != expected.total_amount
                    || current.withdrawn != expected.withdrawn
                    || current.start_slot != expected.start_slot
                    || current.end_slot != expected.end_slot
                    || current.cancelled != expected.cancelled
                    || current.created_slot != expected.created_slot
                    || current.cliff_slot != expected.cliff_slot
                {
                    bail!("stream {} changed during migration", expected.stream_id);
                }
                expected_senders
                    .entry(expected.sender.clone())
                    .or_default()
                    .push(expected.stream_id);
                expected_recipients
                    .entry(expected.recipient.clone())
                    .or_default()
                    .push(expected.stream_id);
            }
            for (address, expected) in expected_senders {
                let actual =
                    read_address_index(&rpc, &contract, "get_sender_stream_ids", &address).await?;
                if actual != expected {
                    bail!("sender stream index differs for {address}");
                }
            }
            for (address, expected) in expected_recipients {
                let actual =
                    read_address_index(&rpc, &contract, "get_recipient_stream_ids", &address)
                        .await?;
                if actual != expected {
                    bail!("recipient stream index differs for {address}");
                }
            }
            println!("verification=ok");
            println!("sha256={}", manifest.manifest_sha256);
        }
        Command::BeginArgs {
            authority,
            expected_stream_count,
        } => {
            let authority = Pubkey::from_base58(&authority).map_err(anyhow::Error::msg)?;
            let count = expected_stream_count.to_le_bytes();
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
            let liability = manifest.payload.escrow_liability.to_le_bytes();
            let unpaid = manifest.payload.unpaid_liability.to_le_bytes();
            governed_payload(
                "complete_accounting_v3_migration",
                layout_args(&[32, 8, 8], &[&authority.0, &liability, &unpaid]),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_decoder_rejects_underflow_and_preserves_layout() {
        let mut data = vec![0u8; STREAM_SIZE];
        data[0..32].copy_from_slice(&[1u8; 32]);
        data[32..64].copy_from_slice(&[2u8; 32]);
        data[64..72].copy_from_slice(&1000u64.to_le_bytes());
        data[72..80].copy_from_slice(&250u64.to_le_bytes());
        data[80..88].copy_from_slice(&10u64.to_le_bytes());
        data[88..96].copy_from_slice(&20u64.to_le_bytes());
        data[97..105].copy_from_slice(&9u64.to_le_bytes());
        let stream = decode_stream(7, &data).expect("decode stream");
        assert_eq!(stream.stream_id, 7);
        assert_eq!(stream.total_amount, 1000);
        assert_eq!(stream.withdrawn, 250);
        assert_eq!(stream.start_slot, 10);
        assert_eq!(stream.end_slot, 20);
        assert_eq!(stream.created_slot, 9);
        assert_eq!(stream.cliff_slot, 0);
        assert!(decode_stream(7, &data[..STREAM_SIZE - 1]).is_err());
    }

    #[test]
    fn governed_payload_layouts_are_unambiguous() {
        let authority = Pubkey([3u8; 32]);
        let count = 11u64.to_le_bytes();
        let begin = layout_args(&[32, 8], &[&authority.0, &count]);
        assert_eq!(&begin[..3], &[0xAB, 32, 8]);
        assert_eq!(&begin[3..35], &authority.0);
        assert_eq!(&begin[35..43], &11u64.to_le_bytes());
    }

    #[test]
    fn manifest_hash_changes_with_liability() {
        let payload = ManifestPayload {
            schema: 1,
            chain_id: "test".to_string(),
            source_slot: 1,
            contract: Pubkey([4u8; 32]).to_base58(),
            stream_count: 0,
            escrow_liability: 0,
            unpaid_liability: 0,
            streams: Vec::new(),
        };
        let first = manifest_hash(&payload).expect("hash");
        let mut changed = payload;
        changed.escrow_liability = 1;
        assert_ne!(first, manifest_hash(&changed).expect("changed hash"));
    }
}
