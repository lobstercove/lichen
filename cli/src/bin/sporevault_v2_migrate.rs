use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use clap::{Parser, Subcommand};
use lichen_core::Pubkey;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_STRATEGIES: u64 = 5;
const STRATEGY_LENDING: u64 = 1;

#[derive(Parser)]
#[command(
    name = "sporevault-v2-migrate",
    about = "Capture, bind, and verify exact SporeVault Accounting V2 migrations"
)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8899")]
    rpc_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture immutable source evidence after pause and contract upgrade.
    Manifest {
        #[arg(long)]
        contract: String,
        #[arg(long)]
        thalllend: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Print a source-bound retire_legacy_strategy payload for one manifest row.
    RetireArgs {
        #[arg(long)]
        authority: String,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        index: u64,
    },
    /// Verify live rows are migration-ready and print migrate_accounting_v2 payload.
    MigrateArgs {
        #[arg(long)]
        authority: String,
        #[arg(long)]
        manifest: PathBuf,
    },
    /// Verify finalized V2 accounting, configuration, strategy, and custody.
    Verify {
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
struct VaultStats {
    total_assets: u64,
    total_shares: u64,
    strategy_count: u64,
    protocol_fees: u64,
    #[serde(default)]
    idle_assets: u64,
    #[serde(default)]
    lending_assets: u64,
    #[serde(default)]
    accounting_version: u64,
    #[serde(default)]
    active_lending_strategies: u64,
    #[serde(default)]
    strategy_registry_valid: bool,
    #[serde(default)]
    native_licn: bool,
    #[serde(default)]
    thalllend_config_valid: bool,
    #[serde(default)]
    components_match_total: bool,
    #[serde(default)]
    share_state_consistent: bool,
    #[serde(default)]
    liquid_custody_covers_accounting: bool,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    operational: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct StrategyRow {
    index: u64,
    strategy_type: u64,
    allocation_percent: u64,
    deployed_amount: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestPayload {
    schema: u64,
    chain_id: String,
    source_slot: u64,
    contract: String,
    thalllend: String,
    legacy_total_assets: u64,
    total_shares: u64,
    protocol_fees: u64,
    native_custody: u64,
    expected_idle_assets: u64,
    expected_lending_assets: u64,
    expected_total_assets: u64,
    strategies: Vec<StrategyRow>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrationManifest {
    manifest_sha256: String,
    #[serde(flatten)]
    payload: ManifestPayload,
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

    async fn stats(&self) -> Result<VaultStats> {
        serde_json::from_value(self.call("getSporeVaultStats", json!([])).await?)
            .context("failed to decode getSporeVaultStats")
    }

    async fn native_balance(&self, account: &str) -> Result<u64> {
        self.call("getBalance", json!([account]))
            .await?
            .get("spores")
            .and_then(serde_json::Value::as_u64)
            .context("getBalance missing spores")
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
        .with_context(|| format!("{field} is missing"))?
        .try_into()
        .map(u64::from_le_bytes)
        .map_err(|_| anyhow!("invalid {field}"))
}

async fn read_strategy(rpc: &Rpc, contract: &str, index: u64) -> Result<StrategyRow> {
    let index_bytes = index.to_le_bytes();
    let data = rpc
        .readonly(
            contract,
            "get_strategy_info",
            layout_args(&[0x08], &[&index_bytes]),
        )
        .await?;
    if data.len() != 24 {
        bail!(
            "strategy {index} returned {} bytes, expected exactly 24",
            data.len()
        );
    }
    Ok(StrategyRow {
        index,
        strategy_type: read_u64(&data, 0, "strategy type")?,
        allocation_percent: read_u64(&data, 8, "strategy allocation")?,
        deployed_amount: read_u64(&data, 16, "strategy deployed amount")?,
    })
}

async fn read_lending_claim(rpc: &Rpc, thalllend: &str, vault: Pubkey) -> Result<u64> {
    let data = rpc
        .readonly(
            thalllend,
            "get_account_info",
            layout_args(&[0x20], &[&vault.0]),
        )
        .await?;
    if data.len() < 24 {
        bail!(
            "ThallLend get_account_info returned {} bytes, expected at least 24",
            data.len()
        );
    }
    read_u64(&data, 0, "ThallLend supplier claim")
}

fn manifest_hash(payload: &ManifestPayload) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(payload)?)))
}

fn validate_manifest_payload(payload: &ManifestPayload) -> Result<()> {
    if payload.schema != 1 {
        bail!("unsupported manifest schema {}", payload.schema);
    }
    if payload.chain_id.trim().is_empty() {
        bail!("manifest chain ID is empty");
    }
    Pubkey::from_base58(&payload.contract).map_err(anyhow::Error::msg)?;
    Pubkey::from_base58(&payload.thalllend).map_err(anyhow::Error::msg)?;
    if payload.strategies.len() > MAX_STRATEGIES as usize {
        bail!("manifest exceeds the {MAX_STRATEGIES}-strategy contract bound");
    }
    for (index, row) in payload.strategies.iter().enumerate() {
        if row.index != index as u64 {
            bail!("manifest strategy rows are not contiguous at index {index}");
        }
    }
    let idle = payload
        .native_custody
        .checked_sub(payload.protocol_fees)
        .context("manifest protocol fees exceed native custody")?;
    if idle != payload.expected_idle_assets {
        bail!("manifest idle assets do not match custody minus protocol fees");
    }
    let total = idle
        .checked_add(payload.expected_lending_assets)
        .context("manifest total assets overflow")?;
    if total != payload.expected_total_assets {
        bail!("manifest expected total assets do not match real components");
    }
    if (total == 0) != (payload.total_shares == 0) {
        bail!("manifest real assets and legacy shares are inconsistent");
    }
    let lending_rows = payload
        .strategies
        .iter()
        .filter(|row| row.strategy_type == STRATEGY_LENDING)
        .count();
    if lending_rows == 0 && payload.expected_lending_assets != 0 {
        bail!("manifest has a ThallLend claim without a lending strategy row");
    }
    Ok(())
}

fn read_manifest(path: &Path) -> Result<MigrationManifest> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let manifest: MigrationManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to decode {}", path.display()))?;
    validate_manifest_payload(&manifest.payload)?;
    let expected = manifest_hash(&manifest.payload)?;
    if manifest.manifest_sha256 != expected {
        bail!("manifest checksum mismatch: expected {expected}");
    }
    Ok(manifest)
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let parent = path
        .parent()
        .filter(|candidate| !candidate.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        bail!("output directory {} does not exist", parent.display());
    }
    if path.exists() {
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
            Err(error) => return Err(error.into()),
        }
    }
    let (temporary, mut file) = pending.context("could not allocate temporary output")?;
    let result = (|| -> Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::hard_link(&temporary, path)?;
        std::fs::remove_file(&temporary)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.with_context(|| format!("failed to atomically write {}", path.display()))
}

fn print_payload(function: &str, args: &[u8], manifest_sha256: &str) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "function": function,
            "args_base64": base64::engine::general_purpose::STANDARD.encode(args),
            "args_hex": hex::encode(args),
            "manifest_sha256": manifest_sha256,
        }))?
    );
    Ok(())
}

async fn capture_manifest(
    rpc: &Rpc,
    contract: String,
    thalllend: String,
    output: PathBuf,
) -> Result<()> {
    let vault = Pubkey::from_base58(&contract).map_err(anyhow::Error::msg)?;
    let thalllend_key = Pubkey::from_base58(&thalllend).map_err(anyhow::Error::msg)?;
    if vault.0.iter().all(|byte| *byte == 0) || thalllend_key.0.iter().all(|byte| *byte == 0) {
        bail!("contract addresses must be nonzero");
    }
    let stats = rpc.stats().await?;
    if !stats.paused {
        bail!("SporeVault must be paused before manifest capture");
    }
    if stats.accounting_version == 2 {
        bail!("SporeVault accounting v2 is already active");
    }
    if !stats.native_licn {
        bail!("this migration tool requires the canonical native LICN vault");
    }
    if stats.strategy_count > MAX_STRATEGIES {
        bail!("strategy count exceeds contract bound {MAX_STRATEGIES}");
    }
    let mut strategies = Vec::with_capacity(stats.strategy_count as usize);
    for index in 0..stats.strategy_count {
        strategies.push(read_strategy(rpc, &contract, index).await?);
    }
    let native_custody = rpc.native_balance(&contract).await?;
    let expected_idle_assets = native_custody
        .checked_sub(stats.protocol_fees)
        .context("protocol fees exceed real vault custody")?;
    let expected_lending_assets = read_lending_claim(rpc, &thalllend, vault).await?;
    let expected_total_assets = expected_idle_assets
        .checked_add(expected_lending_assets)
        .context("real vault asset total overflow")?;
    let payload = ManifestPayload {
        schema: 1,
        chain_id: rpc.chain_id().await?,
        source_slot: rpc.slot().await?,
        contract,
        thalllend,
        legacy_total_assets: stats.total_assets,
        total_shares: stats.total_shares,
        protocol_fees: stats.protocol_fees,
        native_custody,
        expected_idle_assets,
        expected_lending_assets,
        expected_total_assets,
        strategies,
    };
    validate_manifest_payload(&payload)?;
    let manifest = MigrationManifest {
        manifest_sha256: manifest_hash(&payload)?,
        payload,
    };
    write_json_atomic(&output, &manifest)?;
    println!(
        "sealed {} at slot {} with SHA-256 {}",
        output.display(),
        manifest.payload.source_slot,
        manifest.manifest_sha256
    );
    Ok(())
}

async fn retire_args(rpc: &Rpc, authority: String, path: PathBuf, index: u64) -> Result<()> {
    let manifest = read_manifest(&path)?;
    if rpc.chain_id().await? != manifest.payload.chain_id {
        bail!("live chain ID does not match manifest");
    }
    let row = manifest
        .payload
        .strategies
        .get(index as usize)
        .filter(|row| row.index == index)
        .context("strategy index is not present in manifest")?;
    let live = read_strategy(rpc, &manifest.payload.contract, index).await?;
    if &live != row {
        bail!("live strategy row {index} no longer matches sealed source data");
    }
    let authority = Pubkey::from_base58(&authority).map_err(anyhow::Error::msg)?;
    let index_bytes = index.to_le_bytes();
    let expected_type = u8::try_from(row.strategy_type).context("strategy type exceeds u8")?;
    let allocation = row.allocation_percent.to_le_bytes();
    let deployed = row.deployed_amount.to_le_bytes();
    let args = layout_args(
        &[0x20, 0x08, 0x01, 0x08, 0x08],
        &[
            &authority.0,
            &index_bytes,
            &[expected_type],
            &allocation,
            &deployed,
        ],
    );
    print_payload("retire_legacy_strategy", &args, &manifest.manifest_sha256)
}

async fn migrate_args(rpc: &Rpc, authority: String, path: PathBuf) -> Result<()> {
    let manifest = read_manifest(&path)?;
    if rpc.chain_id().await? != manifest.payload.chain_id {
        bail!("live chain ID does not match manifest");
    }
    let stats = rpc.stats().await?;
    if !stats.paused || stats.accounting_version == 2 {
        bail!("live vault must remain paused and pre-v2");
    }
    if stats.strategy_count != manifest.payload.strategies.len() as u64 {
        bail!("live strategy frontier differs from sealed manifest");
    }
    let mut lending_count = 0u64;
    for index in 0..stats.strategy_count {
        let row = read_strategy(rpc, &manifest.payload.contract, index).await?;
        if row.strategy_type == STRATEGY_LENDING {
            lending_count += 1;
            if row.allocation_percent > 100 {
                bail!("live lending allocation exceeds 100%");
            }
        } else if row.strategy_type != 0 || row.allocation_percent != 0 || row.deployed_amount != 0
        {
            bail!("live strategy {index} has not been retired exactly");
        }
    }
    if lending_count > 1 || (lending_count == 0 && manifest.payload.expected_lending_assets != 0) {
        bail!("live lending strategy state is not migration-ready");
    }
    if rpc.native_balance(&manifest.payload.contract).await? != manifest.payload.native_custody {
        bail!("live native custody changed after manifest capture");
    }
    let vault = Pubkey::from_base58(&manifest.payload.contract).map_err(anyhow::Error::msg)?;
    if read_lending_claim(rpc, &manifest.payload.thalllend, vault).await?
        != manifest.payload.expected_lending_assets
    {
        bail!("live ThallLend claim changed after manifest capture");
    }
    let authority = Pubkey::from_base58(&authority).map_err(anyhow::Error::msg)?;
    let idle = manifest.payload.expected_idle_assets.to_le_bytes();
    let lending = manifest.payload.expected_lending_assets.to_le_bytes();
    let args = layout_args(&[0x20, 0x08, 0x08], &[&authority.0, &idle, &lending]);
    print_payload("migrate_accounting_v2", &args, &manifest.manifest_sha256)
}

async fn verify(rpc: &Rpc, path: PathBuf) -> Result<()> {
    let manifest = read_manifest(&path)?;
    if rpc.chain_id().await? != manifest.payload.chain_id {
        bail!("live chain ID does not match manifest");
    }
    let stats = rpc.stats().await?;
    if stats.accounting_version != 2
        || stats.total_assets != manifest.payload.expected_total_assets
        || stats.total_shares != manifest.payload.total_shares
        || stats.idle_assets != manifest.payload.expected_idle_assets
        || stats.lending_assets != manifest.payload.expected_lending_assets
        || stats.protocol_fees != manifest.payload.protocol_fees
    {
        bail!("finalized vault accounting does not match sealed migration values");
    }
    if !stats.native_licn
        || !stats.thalllend_config_valid
        || !stats.components_match_total
        || !stats.share_state_consistent
        || !stats.liquid_custody_covers_accounting
        || !stats.strategy_registry_valid
        || stats.active_lending_strategies != 1
    {
        bail!("finalized vault configuration or custody proof is unhealthy");
    }
    if rpc.native_balance(&manifest.payload.contract).await? != manifest.payload.native_custody {
        bail!("final native custody differs from sealed migration custody");
    }
    if !stats.paused {
        bail!("vault was unpaused before independent migration verification");
    }
    println!(
        "verified SporeVault Accounting V2 at live slot {} against {}",
        rpc.slot().await?,
        manifest.manifest_sha256
    );
    if stats.operational {
        bail!("paused vault unexpectedly reports operational=true");
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let rpc = Rpc::new(cli.rpc_url);
    match cli.command {
        Command::Manifest {
            contract,
            thalllend,
            output,
        } => capture_manifest(&rpc, contract, thalllend, output).await,
        Command::RetireArgs {
            authority,
            manifest,
            index,
        } => retire_args(&rpc, authority, manifest, index).await,
        Command::MigrateArgs {
            authority,
            manifest,
        } => migrate_args(&rpc, authority, manifest).await,
        Command::Verify { manifest } => verify(&rpc, manifest).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> ManifestPayload {
        ManifestPayload {
            schema: 1,
            chain_id: "lichen-test".to_string(),
            source_slot: 100,
            contract: Pubkey([1u8; 32]).to_base58(),
            thalllend: Pubkey([2u8; 32]).to_base58(),
            legacy_total_assets: 999,
            total_shares: 900,
            protocol_fees: 100,
            native_custody: 700,
            expected_idle_assets: 600,
            expected_lending_assets: 300,
            expected_total_assets: 900,
            strategies: vec![StrategyRow {
                index: 0,
                strategy_type: 1,
                allocation_percent: 33,
                deployed_amount: 123,
            }],
        }
    }

    #[test]
    fn manifest_validation_recomputes_real_components() {
        let mut value = payload();
        validate_manifest_payload(&value).unwrap();
        value.expected_total_assets += 1;
        assert!(validate_manifest_payload(&value).is_err());
    }

    #[test]
    fn migration_layout_is_canonical() {
        let authority = Pubkey([3u8; 32]);
        let idle = 600u64.to_le_bytes();
        let lending = 300u64.to_le_bytes();
        let args = layout_args(&[0x20, 0x08, 0x08], &[&authority.0, &idle, &lending]);
        assert_eq!(&args[..4], &[0xAB, 0x20, 0x08, 0x08]);
        assert_eq!(read_u64(&args, 36, "idle").unwrap(), 600);
        assert_eq!(read_u64(&args, 44, "lending").unwrap(), 300);
    }
}
