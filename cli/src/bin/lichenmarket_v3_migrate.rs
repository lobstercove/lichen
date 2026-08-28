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
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONTRACT_PROGRAM_ID: Pubkey = Pubkey([0xFF; 32]);
const ZERO_PUBKEY: Pubkey = Pubkey([0u8; 32]);
const LISTING_SIZE: usize = 147;
const OFFER_SIZE: usize = 73;
const OFFER_EXPIRY_SIZE: usize = 81;
const AUCTION_SIZE: usize = 211;
const COLLECTION_OFFER_SIZE: usize = 113;
const DEFAULT_FEE_BPS: u64 = 250;
const MAX_FEE_BPS: u64 = 1_000;
const MAX_ROWS: usize = 1_000_000;

#[derive(Parser)]
#[command(
    name = "lichenmarket-v3-migrate",
    about = "Capture, execute, and verify source-bound LichenMarket V3 migrations"
)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8899")]
    rpc_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture frozen storage plus genesis-to-source transaction replay.
    Manifest {
        #[arg(long)]
        contract: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Migrate global/token metrics and active listing/offer settlement terms.
    MigrateAdmin {
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
    /// Move active auction bids and unpaid payouts from the configured treasury.
    MigrateTreasury {
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
    /// Move every active offer owned by one signer into exact payment custody.
    MigrateOffers {
        #[arg(long)]
        contract: String,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        keypair: PathBuf,
        #[arg(long)]
        receipts: PathBuf,
        /// Explicitly attach each native offer amount. Normally historical value
        /// is already held by the contract and this must remain false.
        #[arg(long)]
        supply_native: bool,
        #[arg(long)]
        execute: bool,
        #[arg(long, default_value_t = 20)]
        confirmation_attempts: usize,
    },
    /// Move every active auction NFT owned by one seller into contract custody.
    MigrateAuctions {
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
    /// Verify the completed migration against the source-bound manifest.
    Verify {
        #[arg(long)]
        contract: String,
        #[arg(long)]
        manifest: PathBuf,
    },
    /// Print the governed begin_metrics_v3_migration payload.
    BeginArgs {
        #[arg(long)]
        authority: String,
    },
    /// Print the governed seal_metrics_v3_manifest payload.
    SealArgs {
        #[arg(long)]
        authority: String,
        #[arg(long)]
        manifest: PathBuf,
    },
    /// Print the governed complete_metrics_v3_migration payload.
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

#[derive(Clone, Debug, Deserialize)]
struct StorageEntry {
    key_hex: String,
    value_hex: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ProgramCallRow {
    slot: u64,
    sequence: u64,
    caller: String,
    function: String,
    value: u64,
    tx_signature: String,
}

#[derive(Clone, Debug)]
struct HistoricalCall {
    row: ProgramCallRow,
    args: Vec<u8>,
    tx_success: bool,
    return_code: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct TokenMetricRow {
    payment_token: String,
    sale_count: u64,
    sale_volume: u64,
    realized_fees: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ListingRow {
    seller: String,
    nft_contract: String,
    token_id: u64,
    price: u64,
    payment_token: String,
    royalty_recipient: String,
    royalty_bps: u16,
    record_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct OfferRow {
    offerer: String,
    nft_contract: String,
    token_id: u64,
    price: u64,
    payment_token: String,
    expiry_slot: Option<u64>,
    royalty_recipient: String,
    royalty_bps: u16,
    record_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct AuctionRow {
    seller: String,
    nft_contract: String,
    token_id: u64,
    start_price: u64,
    reserve_price: u64,
    highest_bid: u64,
    highest_bidder: String,
    payment_token: String,
    royalty_recipient: String,
    royalty_bps: u16,
    record_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CollectionOfferRow {
    offerer: String,
    collection: String,
    price: u64,
    payment_token: String,
    expiry_slot: u64,
    royalty_recipient: String,
    royalty_bps: u16,
    record_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PayoutRow {
    payment_token: String,
    recipient: String,
    amount: u64,
    record_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestPayload {
    schema: u64,
    chain_id: String,
    source_slot: u64,
    contract: String,
    admin: String,
    treasury: String,
    current_fee_bps: u64,
    storage_sha256: String,
    history_sha256: String,
    program_call_count: u64,
    legacy_sale_count: u64,
    legacy_mixed_sale_volume: u64,
    native_sale_volume: u64,
    expected_custody_rows: u64,
    expected_native_custody: u64,
    token_metrics: Vec<TokenMetricRow>,
    listings: Vec<ListingRow>,
    offers: Vec<OfferRow>,
    auctions: Vec<AuctionRow>,
    collection_offers: Vec<CollectionOfferRow>,
    unpaid_payouts: Vec<PayoutRow>,
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
    action_key: String,
    signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MigrationStatus {
    version: u64,
    locked: bool,
    paused: bool,
    sealed: bool,
    expected_token_rows: u64,
    migrated_token_rows: u64,
    expected_sales: u64,
    migrated_sales: u64,
    native_sale_volume: u64,
    expected_custody_rows: u64,
    migrated_custody_rows: u64,
    expected_native_custody: u64,
    reserved_native_custody: u64,
    manifest: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct NftKey(Pubkey, u64);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct OfferKey(Pubkey, u64, Pubkey);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CollectionOfferKey(Pubkey, Pubkey);

#[derive(Clone, Debug, PartialEq, Eq)]
struct ListingState {
    seller: Pubkey,
    price: u64,
    payment_token: Pubkey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OfferState {
    price: u64,
    payment_token: Pubkey,
    expiry_slot: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AuctionState {
    seller: Pubkey,
    start_price: u64,
    reserve_price: u64,
    highest_bid: u64,
    highest_bidder: Pubkey,
    payment_token: Pubkey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CollectionOfferState {
    price: u64,
    payment_token: Pubkey,
    expiry_slot: u64,
}

#[derive(Default)]
struct ReplayState {
    fee_bps: u64,
    listings: BTreeMap<NftKey, ListingState>,
    offers: BTreeMap<OfferKey, OfferState>,
    auctions: BTreeMap<NftKey, AuctionState>,
    collection_offers: BTreeMap<CollectionOfferKey, CollectionOfferState>,
    token_metrics: BTreeMap<Pubkey, (u64, u64, u64)>,
    uncertain_offers: BTreeSet<OfferKey>,
    uncertain_auctions: BTreeSet<NftKey>,
    uncertain_collection_offers: BTreeSet<CollectionOfferKey>,
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

    async fn readonly(&self, contract: &str, function: &str, args: Vec<u8>) -> Result<Vec<u8>> {
        self.readonly_expected(contract, function, args, 0).await
    }

    async fn readonly_expected(
        &self,
        contract: &str,
        function: &str,
        args: Vec<u8>,
        expected_code: i64,
    ) -> Result<Vec<u8>> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(args);
        let result = self
            .call("callContract", json!([contract, function, encoded]))
            .await?;
        let code = result
            .get("returnCode")
            .or_else(|| result.get("return_code"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        if code != expected_code {
            bail!("{function} returned contract code {code}, expected {expected_code}");
        }
        let data = result
            .get("returnData")
            .or_else(|| result.get("return_data"))
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("{function} returned no data"))?;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .with_context(|| format!("{function} returned invalid base64"))
    }

    async fn storage(&self, contract: &str) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut all = Vec::new();
        let mut after_key: Option<String> = None;
        loop {
            let options = match after_key.as_deref() {
                Some(after) => json!({"limit":500,"after_key":after}),
                None => json!({"limit":500}),
            };
            let value = self
                .call("getProgramStorage", json!([contract, options]))
                .await?;
            let entries: Vec<StorageEntry> = serde_json::from_value(
                value
                    .get("entries")
                    .cloned()
                    .context("getProgramStorage missing entries")?,
            )?;
            if entries.is_empty() {
                break;
            }
            for entry in &entries {
                all.push((hex::decode(&entry.key_hex)?, hex::decode(&entry.value_hex)?));
            }
            if entries.len() < 500 {
                break;
            }
            let next = entries.last().unwrap().key_hex.clone();
            if after_key.as_ref() == Some(&next) {
                bail!("getProgramStorage pagination did not advance");
            }
            after_key = Some(next);
            if all.len() > MAX_ROWS.saturating_mul(16) {
                bail!("contract storage exceeds migration safety bound");
            }
        }
        all.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(all)
    }

    async fn program_calls(&self, contract: &str, source_slot: u64) -> Result<Vec<ProgramCallRow>> {
        let upper = source_slot
            .checked_add(1)
            .context("source slot cannot be incremented")?;
        let mut cursor = format!("v1:{upper:016x}:0000000000000000");
        let mut calls = Vec::new();
        loop {
            let value = self
                .call(
                    "getProgramCalls",
                    json!([contract, {"limit":500,"before_cursor":cursor}]),
                )
                .await?;
            let page: Vec<ProgramCallRow> = serde_json::from_value(
                value
                    .get("calls")
                    .cloned()
                    .context("getProgramCalls missing calls")?,
            )?;
            if page.iter().any(|row| row.slot > source_slot) {
                bail!("getProgramCalls returned a row newer than the source slot");
            }
            calls.extend(page);
            if calls.len() > MAX_ROWS {
                bail!("program call history exceeds migration safety bound");
            }
            let has_more = value
                .get("has_more")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if !has_more {
                break;
            }
            let next = value
                .get("next_cursor")
                .and_then(serde_json::Value::as_str)
                .context("getProgramCalls has_more without next_cursor")?
                .to_owned();
            if next == cursor {
                bail!("getProgramCalls pagination did not advance");
            }
            cursor = next;
        }
        calls.sort_by_key(|row| (row.slot, row.sequence));
        let mut positions = BTreeSet::new();
        let mut signatures = BTreeSet::new();
        for row in &calls {
            if !positions.insert((row.slot, row.sequence)) {
                bail!(
                    "duplicate program-call position {}:{}",
                    row.slot,
                    row.sequence
                );
            }
            if !signatures.insert(row.tx_signature.clone()) {
                bail!(
                    "transaction {} produced multiple target calls; return-code attribution is ambiguous",
                    row.tx_signature
                );
            }
        }
        Ok(calls)
    }

    async fn historical_call(
        &self,
        contract: Pubkey,
        row: ProgramCallRow,
    ) -> Result<HistoricalCall> {
        let value = self
            .call("getTransaction", json!([row.tx_signature]))
            .await?;
        if value.is_null() {
            bail!("transaction {} is unavailable", row.tx_signature);
        }
        if value.get("slot").and_then(serde_json::Value::as_u64) != Some(row.slot) {
            bail!(
                "transaction {} slot differs from its call index",
                row.tx_signature
            );
        }
        let tx_success = value
            .get("success")
            .and_then(serde_json::Value::as_bool)
            .context("getTransaction missing success")?;
        let return_code = value
            .get("return_code")
            .and_then(serde_json::Value::as_u64)
            .map(u32::try_from)
            .transpose()
            .context("transaction return code exceeds u32")?;
        let instructions = value
            .get("message")
            .and_then(|message| message.get("instructions"))
            .and_then(serde_json::Value::as_array)
            .context("getTransaction missing message.instructions")?;
        let mut target = None;
        for instruction in instructions {
            let program = instruction
                .get("program_id")
                .and_then(serde_json::Value::as_str)
                .context("transaction instruction missing program_id")?;
            if program != CONTRACT_PROGRAM_ID.to_base58() {
                continue;
            }
            let accounts = instruction
                .get("accounts")
                .and_then(serde_json::Value::as_array)
                .context("contract instruction missing accounts")?;
            if accounts.get(1).and_then(serde_json::Value::as_str)
                != Some(contract.to_base58().as_str())
            {
                continue;
            }
            let bytes: Vec<u8> = serde_json::from_value(
                instruction
                    .get("data")
                    .cloned()
                    .context("contract instruction missing data")?,
            )?;
            let decoded = ContractInstruction::deserialize(&bytes)
                .map_err(|error| anyhow!("invalid contract instruction: {error}"))?;
            let (function, args, call_value) = match decoded {
                ContractInstruction::Call {
                    function,
                    args,
                    value,
                } => (function, args, value),
                _ => continue,
            };
            let caller = accounts
                .first()
                .and_then(serde_json::Value::as_str)
                .context("contract call missing caller account")?;
            if function != row.function || caller != row.caller || call_value != row.value {
                bail!(
                    "transaction {} differs from its program-call index",
                    row.tx_signature
                );
            }
            if target.replace(args).is_some() {
                bail!(
                    "transaction {} contains multiple target contract calls",
                    row.tx_signature
                );
            }
        }
        let args = target.with_context(|| {
            format!(
                "transaction {} does not contain its indexed target call",
                row.tx_signature
            )
        })?;
        Ok(HistoricalCall {
            row,
            args,
            tx_success,
            return_code,
        })
    }

    async fn recent_blockhash(&self) -> Result<Hash> {
        let value = self.call("getRecentBlockhash", json!([])).await?;
        let blockhash = value
            .as_str()
            .or_else(|| value.get("blockhash").and_then(serde_json::Value::as_str))
            .context("getRecentBlockhash missing blockhash")?;
        Hash::from_hex(blockhash).map_err(anyhow::Error::msg)
    }

    async fn simulate(&self, transaction: &Transaction, expected_code: u32) -> Result<()> {
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
        let code = result
            .get("returnCode")
            .or_else(|| result.get("return_code"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if code != u64::from(expected_code) {
            bail!("preflight returned contract code {code}, expected {expected_code}");
        }
        Ok(())
    }

    async fn send(&self, transaction: &Transaction) -> Result<String> {
        let wire = base64::engine::general_purpose::STANDARD.encode(transaction.to_wire());
        self.call("sendTransaction", json!([wire]))
            .await?
            .as_str()
            .map(str::to_owned)
            .context("sendTransaction returned no signature")
    }

    async fn wait_for_confirmation(&self, signature: &str, attempts: usize) -> Result<()> {
        for _ in 0..attempts {
            let result = self
                .call("getSignatureStatuses", json!([[signature]]))
                .await?;
            let status = result
                .get("value")
                .and_then(serde_json::Value::as_array)
                .and_then(|values| values.first())
                .filter(|value| !value.is_null());
            if let Some(status) = status {
                if let Some(error) = status.get("err").filter(|value| !value.is_null()) {
                    bail!("transaction {signature} failed: {error}");
                }
                if status
                    .get("confirmationStatus")
                    .or_else(|| status.get("confirmation_status"))
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| matches!(value, "confirmed" | "finalized"))
                {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        bail!("transaction {signature} was not confirmed after {attempts} attempts")
    }

    async fn verify_action_receipt(
        &self,
        signature: &str,
        signer: Pubkey,
        contract: Pubkey,
        action: &MigrationAction,
    ) -> Result<()> {
        let value = self.call("getTransaction", json!([signature])).await?;
        if value.is_null()
            || value.get("success").and_then(serde_json::Value::as_bool) != Some(true)
            || value.get("return_code").and_then(serde_json::Value::as_u64)
                != Some(u64::from(action.expected_code))
            || !value
                .get("confirmation_status")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|status| matches!(status, "confirmed" | "finalized"))
        {
            bail!("receipt {signature} is not a confirmed successful action");
        }
        let instructions = value
            .get("message")
            .and_then(|message| message.get("instructions"))
            .and_then(serde_json::Value::as_array)
            .context("receipt transaction is missing instructions")?;
        let mut matches = 0usize;
        for instruction in instructions {
            if instruction
                .get("program_id")
                .and_then(serde_json::Value::as_str)
                != Some(CONTRACT_PROGRAM_ID.to_base58().as_str())
            {
                continue;
            }
            let accounts = instruction
                .get("accounts")
                .and_then(serde_json::Value::as_array)
                .context("receipt contract instruction is missing accounts")?;
            if accounts.first().and_then(serde_json::Value::as_str)
                != Some(signer.to_base58().as_str())
                || accounts.get(1).and_then(serde_json::Value::as_str)
                    != Some(contract.to_base58().as_str())
            {
                continue;
            }
            let bytes: Vec<u8> = serde_json::from_value(
                instruction
                    .get("data")
                    .cloned()
                    .context("receipt contract instruction is missing data")?,
            )?;
            match ContractInstruction::deserialize(&bytes)
                .map_err(|error| anyhow!("invalid receipt instruction: {error}"))?
            {
                ContractInstruction::Call {
                    function,
                    args,
                    value,
                } if function == action.function
                    && args == action.args
                    && value == action.value =>
                {
                    matches += 1;
                }
                _ => {}
            }
        }
        if matches != 1 {
            bail!("receipt {signature} does not bind exactly one expected action");
        }
        Ok(())
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

fn decoded_fields<'a>(function: &str, args: &'a [u8], widths: &[u8]) -> Result<Vec<&'a [u8]>> {
    let payload_len = widths.iter().try_fold(0usize, |total, width| {
        total.checked_add(usize::from(*width))
    });
    let payload_len = payload_len.context("argument layout length overflow")?;
    let payload = if args.first() == Some(&0xAB)
        && args.get(1..1 + widths.len()) == Some(widths)
        && args.len() == 1 + widths.len() + payload_len
    {
        &args[1 + widths.len()..]
    } else if args.len() == payload_len {
        // Pre-layout-descriptor clients encoded the same fixed-width fields as
        // a raw concatenation. Exact total length keeps this fallback unambiguous.
        args
    } else {
        bail!(
            "{function} has noncanonical arguments: {} bytes for widths {:?}",
            args.len(),
            widths
        );
    };
    let mut fields = Vec::with_capacity(widths.len());
    let mut cursor = 0usize;
    for width in widths {
        let end = cursor
            .checked_add(usize::from(*width))
            .context("argument field offset overflow")?;
        fields.push(&payload[cursor..end]);
        cursor = end;
    }
    Ok(fields)
}

fn field_pubkey(field: &[u8], name: &str) -> Result<Pubkey> {
    Ok(Pubkey(
        field
            .try_into()
            .map_err(|_| anyhow!("{name} must be 32 bytes"))?,
    ))
}

fn field_u64(field: &[u8], name: &str) -> Result<u64> {
    Ok(u64::from_le_bytes(
        field
            .try_into()
            .map_err(|_| anyhow!("{name} must be 8 bytes"))?,
    ))
}

fn read_u64(data: &[u8], offset: usize, field: &str) -> Result<u64> {
    field_u64(
        data.get(offset..offset + 8)
            .with_context(|| format!("missing {field}"))?,
        field,
    )
}

fn read_pubkey(data: &[u8], offset: usize, field: &str) -> Result<Pubkey> {
    field_pubkey(
        data.get(offset..offset + 32)
            .with_context(|| format!("missing {field}"))?,
        field,
    )
}

fn add_sale(state: &mut ReplayState, token: Pubkey, price: u64) -> Result<()> {
    let fee = u64::try_from(u128::from(price) * u128::from(state.fee_bps) / 10_000)
        .context("realized fee exceeds u64")?;
    let metrics = state.token_metrics.entry(token).or_insert((0, 0, 0));
    metrics.0 = metrics.0.checked_add(1).context("sale count overflow")?;
    metrics.1 = metrics
        .1
        .checked_add(price)
        .context("sale volume overflow")?;
    metrics.2 = metrics.2.checked_add(fee).context("sale fee overflow")?;
    Ok(())
}

fn successful_code(call: &HistoricalCall) -> Result<Option<u32>> {
    if !call.tx_success {
        return Ok(None);
    }
    call.return_code
        .map(Some)
        .with_context(|| format!("successful {} call has no return code", call.row.function))
}

fn replay_history(calls: &[HistoricalCall]) -> Result<ReplayState> {
    let mut state = ReplayState {
        fee_bps: DEFAULT_FEE_BPS,
        ..ReplayState::default()
    };
    for call in calls {
        let function = call.row.function.as_str();
        let tracked = matches!(
            function,
            "set_marketplace_fee"
                | "list_nft"
                | "list_nft_with_royalty"
                | "buy_nft"
                | "cancel_listing"
                | "update_listing_price"
                | "make_offer"
                | "make_offer_with_expiry"
                | "cancel_offer"
                | "accept_offer"
                | "create_auction"
                | "place_bid"
                | "settle_auction"
                | "cancel_auction"
                | "make_collection_offer"
                | "accept_collection_offer"
                | "cancel_collection_offer"
        );
        if !tracked {
            continue;
        }
        let Some(code) = successful_code(call)? else {
            continue;
        };
        let success = code == 1 || (function == "settle_auction" && code == 2);
        if !success {
            // Three legacy failure paths could commit a refund/deactivation
            // before returning zero. Keep the exact pre-call record and mark
            // only that key as potentially inactive; final storage resolves
            // the branch without inventing a sale or fee.
            if code == 0 {
                match function {
                    "accept_offer" => {
                        let fields = decoded_fields(function, &call.args, &[32, 32, 8, 32])?;
                        state.uncertain_offers.insert(OfferKey(
                            field_pubkey(fields[1], "failed offer NFT")?,
                            field_u64(fields[2], "failed offer token ID")?,
                            field_pubkey(fields[3], "failed offerer")?,
                        ));
                    }
                    "settle_auction" => {
                        let fields = decoded_fields(function, &call.args, &[32, 32, 8])?;
                        state.uncertain_auctions.insert(NftKey(
                            field_pubkey(fields[1], "failed auction NFT")?,
                            field_u64(fields[2], "failed auction token ID")?,
                        ));
                    }
                    "accept_collection_offer" => {
                        let fields = decoded_fields(function, &call.args, &[32, 32, 8, 32])?;
                        state.uncertain_collection_offers.insert(CollectionOfferKey(
                            field_pubkey(fields[1], "failed collection")?,
                            field_pubkey(fields[3], "failed collection offerer")?,
                        ));
                    }
                    _ => {}
                }
            }
            continue;
        }
        match function {
            "set_marketplace_fee" => {
                let fields = decoded_fields(function, &call.args, &[32, 8])?;
                let fee = field_u64(fields[1], "marketplace fee")?;
                if fee > MAX_FEE_BPS {
                    bail!("successful marketplace fee exceeds protocol maximum");
                }
                state.fee_bps = fee;
            }
            "list_nft" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8, 8, 32])?;
                let key = NftKey(
                    field_pubkey(fields[1], "listing NFT")?,
                    field_u64(fields[2], "listing token ID")?,
                );
                state.listings.insert(
                    key,
                    ListingState {
                        seller: field_pubkey(fields[0], "listing seller")?,
                        price: field_u64(fields[3], "listing price")?,
                        payment_token: field_pubkey(fields[4], "listing payment token")?,
                    },
                );
            }
            "list_nft_with_royalty" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8, 8, 32, 32, 4])?;
                let key = NftKey(
                    field_pubkey(fields[1], "listing NFT")?,
                    field_u64(fields[2], "listing token ID")?,
                );
                state.listings.insert(
                    key,
                    ListingState {
                        seller: field_pubkey(fields[0], "listing seller")?,
                        price: field_u64(fields[3], "listing price")?,
                        payment_token: field_pubkey(fields[4], "listing payment token")?,
                    },
                );
            }
            "buy_nft" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8])?;
                let key = NftKey(
                    field_pubkey(fields[1], "purchase NFT")?,
                    field_u64(fields[2], "purchase token ID")?,
                );
                let listing = state.listings.remove(&key).with_context(|| {
                    format!(
                        "successful buy at {}:{} has no replayed active listing",
                        call.row.slot, call.row.sequence
                    )
                })?;
                add_sale(&mut state, listing.payment_token, listing.price)?;
            }
            "cancel_listing" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8])?;
                let key = NftKey(
                    field_pubkey(fields[1], "cancelled listing NFT")?,
                    field_u64(fields[2], "cancelled listing token ID")?,
                );
                // Legacy cancellation returned success for an already inactive
                // record as well, so absence from the active replay is valid.
                state.listings.remove(&key);
            }
            "update_listing_price" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8, 8])?;
                let key = NftKey(
                    field_pubkey(fields[1], "updated listing NFT")?,
                    field_u64(fields[2], "updated listing token ID")?,
                );
                let listing = state
                    .listings
                    .get_mut(&key)
                    .context("successful price update has no replayed active listing")?;
                listing.price = field_u64(fields[3], "updated listing price")?;
            }
            "make_offer" | "make_offer_with_expiry" => {
                let widths: &[u8] = if function == "make_offer" {
                    &[32, 32, 8, 8, 32]
                } else {
                    &[32, 32, 8, 8, 32, 8]
                };
                let fields = decoded_fields(function, &call.args, widths)?;
                let offerer = field_pubkey(fields[0], "offerer")?;
                let key = OfferKey(
                    field_pubkey(fields[1], "offer NFT")?,
                    field_u64(fields[2], "offer token ID")?,
                    offerer,
                );
                state.offers.insert(
                    key.clone(),
                    OfferState {
                        price: field_u64(fields[3], "offer price")?,
                        payment_token: field_pubkey(fields[4], "offer payment token")?,
                        expiry_slot: (function == "make_offer_with_expiry")
                            .then(|| field_u64(fields[5], "offer expiry"))
                            .transpose()?,
                    },
                );
                state.uncertain_offers.remove(&key);
            }
            "cancel_offer" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8])?;
                let key = OfferKey(
                    field_pubkey(fields[1], "cancelled offer NFT")?,
                    field_u64(fields[2], "cancelled offer token ID")?,
                    field_pubkey(fields[0], "cancelled offerer")?,
                );
                state.offers.remove(&key);
                state.uncertain_offers.remove(&key);
            }
            "accept_offer" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8, 32])?;
                let key = OfferKey(
                    field_pubkey(fields[1], "accepted offer NFT")?,
                    field_u64(fields[2], "accepted offer token ID")?,
                    field_pubkey(fields[3], "accepted offerer")?,
                );
                let offer = state
                    .offers
                    .remove(&key)
                    .context("successful offer acceptance has no replayed active offer")?;
                state.uncertain_offers.remove(&key);
                add_sale(&mut state, offer.payment_token, offer.price)?;
            }
            "create_auction" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8, 8, 8, 8, 32])?;
                let key = NftKey(
                    field_pubkey(fields[1], "auction NFT")?,
                    field_u64(fields[2], "auction token ID")?,
                );
                if state.auctions.contains_key(&key) {
                    bail!("successful auction creation replaced a replayed active auction");
                }
                state.auctions.insert(
                    key.clone(),
                    AuctionState {
                        seller: field_pubkey(fields[0], "auction seller")?,
                        start_price: field_u64(fields[3], "auction start price")?,
                        reserve_price: field_u64(fields[4], "auction reserve price")?,
                        highest_bid: 0,
                        highest_bidder: ZERO_PUBKEY,
                        payment_token: field_pubkey(fields[6], "auction payment token")?,
                    },
                );
                state.uncertain_auctions.remove(&key);
            }
            "place_bid" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8, 8])?;
                let key = NftKey(
                    field_pubkey(fields[1], "bid auction NFT")?,
                    field_u64(fields[2], "bid auction token ID")?,
                );
                let auction = state
                    .auctions
                    .get_mut(&key)
                    .context("successful bid has no replayed active auction")?;
                auction.highest_bidder = field_pubkey(fields[0], "bidder")?;
                auction.highest_bid = field_u64(fields[3], "bid amount")?;
                state.uncertain_auctions.remove(&key);
            }
            "settle_auction" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8])?;
                let key = NftKey(
                    field_pubkey(fields[1], "settled auction NFT")?,
                    field_u64(fields[2], "settled auction token ID")?,
                );
                let auction = state
                    .auctions
                    .remove(&key)
                    .context("successful auction settlement has no replayed active auction")?;
                state.uncertain_auctions.remove(&key);
                if code == 1 {
                    if auction.highest_bid == 0 {
                        bail!("auction sale has a zero replayed highest bid");
                    }
                    add_sale(&mut state, auction.payment_token, auction.highest_bid)?;
                }
            }
            "cancel_auction" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8])?;
                let key = NftKey(
                    field_pubkey(fields[1], "cancelled auction NFT")?,
                    field_u64(fields[2], "cancelled auction token ID")?,
                );
                if state.auctions.remove(&key).is_none() {
                    bail!("successful auction cancellation has no replayed active auction");
                }
                state.uncertain_auctions.remove(&key);
            }
            "make_collection_offer" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8, 32, 8])?;
                let offerer = field_pubkey(fields[0], "collection offerer")?;
                let collection = field_pubkey(fields[1], "collection")?;
                let key = CollectionOfferKey(collection, offerer);
                state.collection_offers.insert(
                    key.clone(),
                    CollectionOfferState {
                        price: field_u64(fields[2], "collection offer price")?,
                        payment_token: field_pubkey(fields[3], "collection payment token")?,
                        expiry_slot: field_u64(fields[4], "collection offer expiry")?,
                    },
                );
                state.uncertain_collection_offers.remove(&key);
            }
            "accept_collection_offer" => {
                let fields = decoded_fields(function, &call.args, &[32, 32, 8, 32])?;
                let key = CollectionOfferKey(
                    field_pubkey(fields[1], "accepted collection")?,
                    field_pubkey(fields[3], "accepted collection offerer")?,
                );
                let offer = state.collection_offers.remove(&key).context(
                    "successful collection-offer acceptance has no replayed active offer",
                )?;
                state.uncertain_collection_offers.remove(&key);
                add_sale(&mut state, offer.payment_token, offer.price)?;
            }
            "cancel_collection_offer" => {
                let fields = decoded_fields(function, &call.args, &[32, 32])?;
                let key = CollectionOfferKey(
                    field_pubkey(fields[1], "cancelled collection")?,
                    field_pubkey(fields[0], "cancelled collection offerer")?,
                );
                if state.collection_offers.remove(&key).is_none() {
                    bail!("successful collection-offer cancellation has no replayed active offer");
                }
                state.uncertain_collection_offers.remove(&key);
            }
            _ => unreachable!(),
        }
    }
    Ok(state)
}

fn history_hash(calls: &[HistoricalCall]) -> String {
    let mut hasher = Sha256::new();
    for call in calls {
        hasher.update(call.row.slot.to_le_bytes());
        hasher.update(call.row.sequence.to_le_bytes());
        hash_len_bytes(&mut hasher, call.row.caller.as_bytes());
        hash_len_bytes(&mut hasher, call.row.function.as_bytes());
        hasher.update(call.row.value.to_le_bytes());
        hash_len_bytes(&mut hasher, call.row.tx_signature.as_bytes());
        hasher.update([u8::from(call.tx_success)]);
        match call.return_code {
            Some(code) => {
                hasher.update([1]);
                hasher.update(code.to_le_bytes());
            }
            None => hasher.update([0]),
        }
        hasher.update(Sha256::digest(&call.args));
    }
    hex::encode(hasher.finalize())
}

fn hash_len_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn storage_hash(entries: &[(Vec<u8>, Vec<u8>)]) -> String {
    let mut hasher = Sha256::new();
    for (key, value) in entries {
        hash_len_bytes(&mut hasher, key);
        hash_len_bytes(&mut hasher, value);
    }
    hex::encode(hasher.finalize())
}

fn exact_storage_u64(entries: &[(Vec<u8>, Vec<u8>)], key: &[u8]) -> Result<u64> {
    match entries
        .binary_search_by(|(candidate, _)| candidate.as_slice().cmp(key))
        .ok()
        .map(|index| entries[index].1.as_slice())
    {
        Some(value) if value.len() == 8 => read_u64(value, 0, "storage counter"),
        Some(_) => bail!("storage key {} is not an exact u64", hex::encode(key)),
        None => Ok(0),
    }
}

fn parse_listing_key(key: &[u8]) -> Result<Option<NftKey>> {
    let Some(suffix) = key.strip_prefix(b"listing:") else {
        return Ok(None);
    };
    if suffix.len() != 41 || suffix[32] != b':' {
        bail!("malformed listing storage key {}", hex::encode(key));
    }
    Ok(Some(NftKey(
        field_pubkey(&suffix[..32], "listing key NFT")?,
        field_u64(&suffix[33..], "listing key token ID")?,
    )))
}

fn parse_offer_key(key: &[u8]) -> Result<Option<OfferKey>> {
    let Some(suffix) = key.strip_prefix(b"offer:") else {
        return Ok(None);
    };
    if suffix.len() != 74 || suffix[32] != b':' || suffix[41] != b':' {
        bail!("malformed offer storage key {}", hex::encode(key));
    }
    Ok(Some(OfferKey(
        field_pubkey(&suffix[..32], "offer key NFT")?,
        field_u64(&suffix[33..41], "offer key token ID")?,
        field_pubkey(&suffix[42..], "offer key offerer")?,
    )))
}

fn parse_auction_key(key: &[u8]) -> Result<Option<NftKey>> {
    let Some(suffix) = key.strip_prefix(b"auction:") else {
        return Ok(None);
    };
    if suffix.len() != 41 || suffix[32] != b':' {
        bail!("malformed auction storage key {}", hex::encode(key));
    }
    Ok(Some(NftKey(
        field_pubkey(&suffix[..32], "auction key NFT")?,
        field_u64(&suffix[33..], "auction key token ID")?,
    )))
}

fn parse_collection_offer_key(key: &[u8]) -> Result<Option<CollectionOfferKey>> {
    let Some(suffix) = key.strip_prefix(b"col_offer:") else {
        return Ok(None);
    };
    if suffix.len() != 65 || suffix[32] != b':' {
        bail!(
            "malformed collection-offer storage key {}",
            hex::encode(key)
        );
    }
    Ok(Some(CollectionOfferKey(
        field_pubkey(&suffix[..32], "collection-offer key collection")?,
        field_pubkey(&suffix[33..], "collection-offer key offerer")?,
    )))
}

fn parse_unpaid_payout_key(key: &[u8]) -> Result<Option<(Pubkey, Pubkey)>> {
    let Some(suffix) = key.strip_prefix(b"unpaid_payout:") else {
        return Ok(None);
    };
    if suffix.len() != 65 || suffix[32] != b':' {
        bail!("malformed unpaid-payout storage key {}", hex::encode(key));
    }
    let token = field_pubkey(&suffix[..32], "unpaid-payout token")?;
    let recipient = field_pubkey(&suffix[33..], "unpaid-payout recipient")?;
    if recipient == ZERO_PUBKEY {
        bail!("unpaid-payout recipient cannot be zero");
    }
    Ok(Some((token, recipient)))
}

fn is_preexisting_v3_row_key(key: &[u8]) -> bool {
    [
        b"mm_listing_fee:".as_slice(),
        b"mm_listing_slot:".as_slice(),
        b"mm_offer_fee:".as_slice(),
        b"mm_offer_custody:".as_slice(),
        b"mm_offer_royalty:".as_slice(),
        b"mm_offer_indexed:".as_slice(),
        b"mm_active_offer_count:".as_slice(),
        b"mm_auction_fee:".as_slice(),
        b"mm_auction_escrowed:".as_slice(),
        b"mm_auction_extensions:".as_slice(),
        b"mm_auction_bid_custody:".as_slice(),
        b"mm_collection_offer_fee:".as_slice(),
        b"mm_collection_offer_custody:".as_slice(),
        b"mm_collection_offer_royalty:".as_slice(),
        b"mm_unpaid_custody:".as_slice(),
        b"mm_token_sale_count:".as_slice(),
        b"mm_token_sale_volume:".as_slice(),
        b"mm_token_sale_fees:".as_slice(),
        b"mm_metrics_mig_token:".as_slice(),
        b"mm_platform_fee:".as_slice(),
    ]
    .iter()
    .any(|prefix| key.starts_with(prefix))
        || key == b"mm_native_sale_volume"
}

async fn canonical_royalty(
    rpc: &Rpc,
    contract: &str,
    nft: Pubkey,
    token_id: u64,
) -> Result<(Pubkey, u16)> {
    let token = token_id.to_le_bytes();
    let data = rpc
        .readonly(
            contract,
            "get_canonical_royalty",
            layout_args(&[32, 8], &[&nft.0, &token]),
        )
        .await?;
    if data.len() != 40 {
        bail!(
            "canonical royalty returned {} bytes, expected 40",
            data.len()
        );
    }
    let recipient = read_pubkey(&data, 0, "canonical royalty recipient")?;
    let bps64 = read_u64(&data, 32, "canonical royalty bps")?;
    let bps = u16::try_from(bps64).context("canonical royalty bps exceeds u16")?;
    if bps > 1_000 || (bps > 0 && recipient == ZERO_PUBKEY) {
        bail!("canonical royalty terms are invalid");
    }
    Ok((recipient, bps))
}

async fn marketplace_config(rpc: &Rpc, contract: &str) -> Result<(Pubkey, Pubkey, u64, bool)> {
    let data = rpc
        .readonly(contract, "get_marketplace_config", Vec::new())
        .await?;
    if data.len() != 105 {
        bail!(
            "marketplace config returned {} bytes, expected 105",
            data.len()
        );
    }
    let admin = read_pubkey(&data, 0, "marketplace admin")?;
    let treasury = read_pubkey(&data, 64, "marketplace treasury")?;
    let fee = read_u64(&data, 96, "marketplace fee")?;
    if admin == ZERO_PUBKEY || treasury == ZERO_PUBKEY || fee > MAX_FEE_BPS || data[104] > 1 {
        bail!("marketplace config violates migration invariants");
    }
    Ok((admin, treasury, fee, data[104] == 1))
}

async fn migration_status(rpc: &Rpc, contract: &str) -> Result<MigrationStatus> {
    let data = rpc
        .readonly(contract, "get_metrics_v3_migration_status", Vec::new())
        .await?;
    decode_migration_status(&data)
}

fn decode_migration_status(data: &[u8]) -> Result<MigrationStatus> {
    if data.len() != 115 {
        bail!(
            "migration status returned {} bytes, expected 115",
            data.len()
        );
    }
    if data[8] > 1 || data[9] > 1 || data[10] > 1 {
        bail!("migration status contains a malformed boolean");
    }
    Ok(MigrationStatus {
        version: read_u64(data, 0, "migration version")?,
        locked: data[8] == 1,
        paused: data[9] == 1,
        sealed: data[10] == 1,
        expected_token_rows: read_u64(data, 11, "expected token rows")?,
        migrated_token_rows: read_u64(data, 19, "migrated token rows")?,
        expected_sales: read_u64(data, 27, "expected sales")?,
        migrated_sales: read_u64(data, 35, "migrated sales")?,
        native_sale_volume: read_u64(data, 43, "native sale volume")?,
        expected_custody_rows: read_u64(data, 51, "expected custody rows")?,
        migrated_custody_rows: read_u64(data, 59, "migrated custody rows")?,
        expected_native_custody: read_u64(data, 67, "expected native custody")?,
        reserved_native_custody: read_u64(data, 75, "reserved native custody")?,
        manifest: data[83..115].try_into().unwrap(),
    })
}

fn pristine_frozen_status(status: &MigrationStatus) -> bool {
    status.version == 0
        && status.locked
        && status.paused
        && !status.sealed
        && status.expected_token_rows == 0
        && status.migrated_token_rows == 0
        && status.expected_sales == 0
        && status.migrated_sales == 0
        && status.native_sale_volume == 0
        && status.expected_custody_rows == 0
        && status.migrated_custody_rows == 0
        && status.expected_native_custody == 0
        && status.reserved_native_custody == 0
        && status.manifest == [0u8; 32]
}

async fn capture_manifest(rpc: &Rpc, contract: &str) -> Result<MigrationManifest> {
    let contract_key = Pubkey::from_base58(contract).map_err(anyhow::Error::msg)?;
    let before = migration_status(rpc, contract).await?;
    if !pristine_frozen_status(&before) {
        bail!("capture requires pristine, paused, locked, unsealed legacy metrics state");
    }
    let first_storage = rpc.storage(contract).await?;
    let source_slot = rpc.slot().await?;
    let second_storage = rpc.storage(contract).await?;
    if first_storage != second_storage {
        bail!("contract storage changed across the source snapshot");
    }
    let (admin, treasury, current_fee_bps, paused) = marketplace_config(rpc, contract).await?;
    if !paused {
        bail!("marketplace config is not paused during capture");
    }

    let indexed = rpc.program_calls(contract, source_slot).await?;
    let mut calls = Vec::with_capacity(indexed.len());
    for row in indexed {
        calls.push(rpc.historical_call(contract_key, row).await?);
    }
    let replay = replay_history(&calls)?;
    if replay.fee_bps != current_fee_bps {
        bail!(
            "replayed fee {} differs from stored fee {}",
            replay.fee_bps,
            current_fee_bps
        );
    }

    let legacy_sale_count = exact_storage_u64(&first_storage, b"mm_sale_count")?;
    let legacy_mixed_sale_volume = exact_storage_u64(&first_storage, b"mm_sale_volume")?;
    let replay_sale_count = replay.token_metrics.values().try_fold(0u64, |total, row| {
        total
            .checked_add(row.0)
            .context("replayed sale count overflow")
    })?;
    let replay_mixed_volume = replay.token_metrics.values().try_fold(0u64, |total, row| {
        total
            .checked_add(row.1)
            .context("replayed mixed volume overflow")
    })?;
    if replay_sale_count != legacy_sale_count || replay_mixed_volume != legacy_mixed_sale_volume {
        bail!(
            "history replay does not cover legacy counters: replayed {replay_sale_count}/{replay_mixed_volume}, stored {legacy_sale_count}/{legacy_mixed_sale_volume}"
        );
    }

    let mut listings = Vec::new();
    let mut offers = Vec::new();
    let mut auctions = Vec::new();
    let mut collection_offers = Vec::new();
    let mut unpaid_payouts = Vec::new();
    let mut inventory_listings = BTreeMap::new();
    let mut inventory_offers = BTreeMap::new();
    let mut inventory_auctions = BTreeMap::new();
    let mut inventory_collection_offers = BTreeMap::new();
    let mut active_offers_by_wallet = BTreeMap::<Pubkey, u64>::new();
    let mut stored_offer_counts = BTreeMap::<Pubkey, u64>::new();
    let mut expected_custody_rows = 0u64;
    let mut expected_native_custody = 0u64;

    for (key, value) in &first_storage {
        if is_preexisting_v3_row_key(key) {
            bail!(
                "pristine capture found pre-existing V3 row {}",
                hex::encode(key)
            );
        }
        if let Some(suffix) = key.strip_prefix(b"offerer_count:") {
            let offerer = field_pubkey(suffix, "offer-count wallet")?;
            if value.len() != 8
                || stored_offer_counts
                    .insert(offerer, read_u64(value, 0, "offer count")?)
                    .is_some()
            {
                bail!("malformed or duplicate offer-count row");
            }
            continue;
        }
        if let Some(nft_key @ NftKey(nft, token_id)) = parse_listing_key(key)? {
            if value.len() != LISTING_SIZE
                || read_pubkey(value, 32, "listing NFT")? != nft
                || read_u64(value, 64, "listing token ID")? != token_id
                || read_u64(value, 72, "listing price")? == 0
                || value[144] > 1
            {
                bail!("malformed listing {}:{token_id}", nft.to_base58());
            }
            if value[144] == 1 {
                let seller = read_pubkey(value, 0, "listing seller")?;
                let price = read_u64(value, 72, "listing price")?;
                let payment_token = read_pubkey(value, 80, "listing payment token")?;
                if seller == ZERO_PUBKEY {
                    bail!("active listing seller cannot be zero");
                }
                let (recipient, bps) = canonical_royalty(rpc, contract, nft, token_id).await?;
                inventory_listings.insert(
                    nft_key,
                    ListingState {
                        seller,
                        price,
                        payment_token,
                    },
                );
                listings.push(ListingRow {
                    seller: seller.to_base58(),
                    nft_contract: nft.to_base58(),
                    token_id,
                    price,
                    payment_token: payment_token.to_base58(),
                    royalty_recipient: recipient.to_base58(),
                    royalty_bps: bps,
                    record_sha256: hex::encode(Sha256::digest(value)),
                });
            }
            continue;
        }
        if let Some(offer_key @ OfferKey(nft, token_id, offerer)) = parse_offer_key(key)? {
            if !matches!(value.len(), OFFER_SIZE | OFFER_EXPIRY_SIZE)
                || read_pubkey(value, 0, "offerer")? != offerer
                || read_u64(value, 32, "offer price")? == 0
                || value[72] > 1
            {
                bail!("malformed offer {}:{token_id}", nft.to_base58());
            }
            if value[72] == 1 {
                let price = read_u64(value, 32, "offer price")?;
                let payment_token = read_pubkey(value, 40, "offer payment token")?;
                let expiry_slot = (value.len() == OFFER_EXPIRY_SIZE)
                    .then(|| read_u64(value, 73, "offer expiry"))
                    .transpose()?;
                let (recipient, bps) = canonical_royalty(rpc, contract, nft, token_id).await?;
                inventory_offers.insert(
                    offer_key,
                    OfferState {
                        price,
                        payment_token,
                        expiry_slot,
                    },
                );
                let current = active_offers_by_wallet.entry(offerer).or_insert(0);
                *current = current
                    .checked_add(1)
                    .context("active offer count overflow")?;
                expected_custody_rows = expected_custody_rows
                    .checked_add(1)
                    .context("custody row count overflow")?;
                if payment_token == ZERO_PUBKEY {
                    expected_native_custody = expected_native_custody
                        .checked_add(price)
                        .context("native custody overflow")?;
                }
                offers.push(OfferRow {
                    offerer: offerer.to_base58(),
                    nft_contract: nft.to_base58(),
                    token_id,
                    price,
                    payment_token: payment_token.to_base58(),
                    expiry_slot,
                    royalty_recipient: recipient.to_base58(),
                    royalty_bps: bps,
                    record_sha256: hex::encode(Sha256::digest(value)),
                });
            }
            continue;
        }
        if let Some(nft_key @ NftKey(nft, token_id)) = parse_auction_key(key)? {
            if value.len() != AUCTION_SIZE
                || read_pubkey(value, 32, "auction NFT")? != nft
                || read_u64(value, 64, "auction token ID")? != token_id
                || read_u64(value, 72, "auction start price")? == 0
                || value[144] > 2
            {
                bail!("malformed auction {}:{token_id}", nft.to_base58());
            }
            let highest_bid = read_u64(value, 88, "auction highest bid")?;
            let highest_bidder = read_pubkey(value, 96, "auction highest bidder")?;
            if (highest_bid == 0) != (highest_bidder == ZERO_PUBKEY) {
                bail!("auction bid and bidder invariants disagree");
            }
            if value[144] == 1 {
                let seller = read_pubkey(value, 0, "auction seller")?;
                let start_price = read_u64(value, 72, "auction start price")?;
                let reserve_price = read_u64(value, 80, "auction reserve price")?;
                let payment_token = read_pubkey(value, 145, "auction payment token")?;
                let (recipient, bps) = canonical_royalty(rpc, contract, nft, token_id).await?;
                inventory_auctions.insert(
                    nft_key,
                    AuctionState {
                        seller,
                        start_price,
                        reserve_price,
                        highest_bid,
                        highest_bidder,
                        payment_token,
                    },
                );
                if highest_bid > 0 {
                    expected_custody_rows = expected_custody_rows
                        .checked_add(1)
                        .context("custody row count overflow")?;
                    if payment_token == ZERO_PUBKEY {
                        expected_native_custody = expected_native_custody
                            .checked_add(highest_bid)
                            .context("native custody overflow")?;
                    }
                }
                auctions.push(AuctionRow {
                    seller: seller.to_base58(),
                    nft_contract: nft.to_base58(),
                    token_id,
                    start_price,
                    reserve_price,
                    highest_bid,
                    highest_bidder: highest_bidder.to_base58(),
                    payment_token: payment_token.to_base58(),
                    royalty_recipient: recipient.to_base58(),
                    royalty_bps: bps,
                    record_sha256: hex::encode(Sha256::digest(value)),
                });
            }
            continue;
        }
        if let Some(collection_key @ CollectionOfferKey(collection, offerer)) =
            parse_collection_offer_key(key)?
        {
            if value.len() != COLLECTION_OFFER_SIZE
                || read_pubkey(value, 0, "collection offerer")? != offerer
                || read_pubkey(value, 32, "collection")? != collection
                || read_u64(value, 64, "collection offer price")? == 0
                || value[104] > 1
            {
                bail!("malformed collection offer");
            }
            if value[104] == 1 {
                let price = read_u64(value, 64, "collection offer price")?;
                let payment_token = read_pubkey(value, 72, "collection payment token")?;
                let expiry_slot = read_u64(value, 105, "collection offer expiry")?;
                let (recipient, bps) = canonical_royalty(rpc, contract, collection, 0).await?;
                inventory_collection_offers.insert(
                    collection_key,
                    CollectionOfferState {
                        price,
                        payment_token,
                        expiry_slot,
                    },
                );
                expected_custody_rows = expected_custody_rows
                    .checked_add(1)
                    .context("custody row count overflow")?;
                if payment_token == ZERO_PUBKEY {
                    expected_native_custody = expected_native_custody
                        .checked_add(price)
                        .context("native custody overflow")?;
                }
                collection_offers.push(CollectionOfferRow {
                    offerer: offerer.to_base58(),
                    collection: collection.to_base58(),
                    price,
                    payment_token: payment_token.to_base58(),
                    expiry_slot,
                    royalty_recipient: recipient.to_base58(),
                    royalty_bps: bps,
                    record_sha256: hex::encode(Sha256::digest(value)),
                });
            }
            continue;
        }
        if let Some((payment_token, recipient)) = parse_unpaid_payout_key(key)? {
            if value.len() != 8 {
                bail!("malformed unpaid-payout value");
            }
            let amount = read_u64(value, 0, "unpaid payout")?;
            if amount > 0 {
                expected_custody_rows = expected_custody_rows
                    .checked_add(1)
                    .context("custody row count overflow")?;
                if payment_token == ZERO_PUBKEY {
                    expected_native_custody = expected_native_custody
                        .checked_add(amount)
                        .context("native custody overflow")?;
                }
                unpaid_payouts.push(PayoutRow {
                    payment_token: payment_token.to_base58(),
                    recipient: recipient.to_base58(),
                    amount,
                    record_sha256: hex::encode(Sha256::digest(value)),
                });
            }
        }
    }

    if inventory_listings != replay.listings {
        bail!("active listing inventory differs from transaction-history replay");
    }
    if inventory_offers
        .iter()
        .any(|(key, value)| replay.offers.get(key) != Some(value))
        || replay.offers.keys().any(|key| {
            !inventory_offers.contains_key(key) && !replay.uncertain_offers.contains(key)
        })
    {
        bail!("active offer inventory differs from transaction-history replay");
    }
    if inventory_auctions
        .iter()
        .any(|(key, value)| replay.auctions.get(key) != Some(value))
        || replay.auctions.keys().any(|key| {
            !inventory_auctions.contains_key(key) && !replay.uncertain_auctions.contains(key)
        })
    {
        bail!("active auction inventory differs from transaction-history replay");
    }
    if inventory_collection_offers
        .iter()
        .any(|(key, value)| replay.collection_offers.get(key) != Some(value))
        || replay.collection_offers.keys().any(|key| {
            !inventory_collection_offers.contains_key(key)
                && !replay.uncertain_collection_offers.contains(key)
        })
    {
        bail!("active collection-offer inventory differs from transaction-history replay");
    }
    let offer_count_wallets: BTreeSet<Pubkey> = active_offers_by_wallet
        .keys()
        .chain(stored_offer_counts.keys())
        .copied()
        .collect();
    for offerer in offer_count_wallets {
        let active = active_offers_by_wallet.get(&offerer).copied().unwrap_or(0);
        let stored = stored_offer_counts.get(&offerer).copied().unwrap_or(0);
        if active > 64 || stored != active {
            bail!("active offer count differs for {}", offerer.to_base58());
        }
    }

    let mut token_metrics: Vec<TokenMetricRow> = replay
        .token_metrics
        .iter()
        .map(
            |(token, (sale_count, sale_volume, realized_fees))| TokenMetricRow {
                payment_token: token.to_base58(),
                sale_count: *sale_count,
                sale_volume: *sale_volume,
                realized_fees: *realized_fees,
            },
        )
        .collect();
    token_metrics.sort_by(|a, b| a.payment_token.cmp(&b.payment_token));
    listings.sort_by(|a, b| {
        a.nft_contract
            .cmp(&b.nft_contract)
            .then(a.token_id.cmp(&b.token_id))
    });
    offers.sort_by(|a, b| {
        a.nft_contract
            .cmp(&b.nft_contract)
            .then(a.token_id.cmp(&b.token_id))
            .then(a.offerer.cmp(&b.offerer))
    });
    auctions.sort_by(|a, b| {
        a.nft_contract
            .cmp(&b.nft_contract)
            .then(a.token_id.cmp(&b.token_id))
    });
    collection_offers.sort_by(|a, b| {
        a.collection
            .cmp(&b.collection)
            .then(a.offerer.cmp(&b.offerer))
    });
    unpaid_payouts.sort_by(|a, b| {
        a.payment_token
            .cmp(&b.payment_token)
            .then(a.recipient.cmp(&b.recipient))
    });
    let native_sale_volume = replay
        .token_metrics
        .get(&ZERO_PUBKEY)
        .map(|metrics| metrics.1)
        .unwrap_or(0);
    let after = migration_status(rpc, contract).await?;
    if after != before || !pristine_frozen_status(&after) {
        bail!("migration status changed during source capture");
    }
    let payload = ManifestPayload {
        schema: 1,
        chain_id: rpc.chain_id().await?,
        source_slot,
        contract: contract.to_string(),
        admin: admin.to_base58(),
        treasury: treasury.to_base58(),
        current_fee_bps,
        storage_sha256: storage_hash(&first_storage),
        history_sha256: history_hash(&calls),
        program_call_count: calls.len() as u64,
        legacy_sale_count,
        legacy_mixed_sale_volume,
        native_sale_volume,
        expected_custody_rows,
        expected_native_custody,
        token_metrics,
        listings,
        offers,
        auctions,
        collection_offers,
        unpaid_payouts,
    };
    let manifest_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(&payload)?));
    let manifest = MigrationManifest {
        manifest_sha256,
        payload,
    };
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn decode_sha256(value: &str, field: &str) -> Result<[u8; 32]> {
    hex::decode(value)
        .with_context(|| format!("invalid {field} hex"))?
        .try_into()
        .map_err(|_| anyhow!("{field} must be 32 bytes"))
}

fn parsed_pubkey(value: &str, field: &str) -> Result<Pubkey> {
    Pubkey::from_base58(value)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("invalid {field}"))
}

fn validate_royalty(recipient: &str, bps: u16, key: &str) -> Result<()> {
    let recipient = parsed_pubkey(recipient, "royalty recipient")?;
    if bps > 1_000 || (bps > 0 && recipient == ZERO_PUBKEY) {
        bail!("invalid royalty terms for {key}");
    }
    Ok(())
}

fn validate_manifest(manifest: &MigrationManifest) -> Result<()> {
    let payload = &manifest.payload;
    if payload.schema != 1 {
        bail!("unsupported manifest schema {}", payload.schema);
    }
    if payload.chain_id.is_empty() || payload.source_slot == u64::MAX {
        bail!("manifest network identity is invalid");
    }
    let contract = parsed_pubkey(&payload.contract, "contract")?;
    let admin = parsed_pubkey(&payload.admin, "admin")?;
    let treasury = parsed_pubkey(&payload.treasury, "treasury")?;
    if contract == ZERO_PUBKEY || admin == ZERO_PUBKEY || treasury == ZERO_PUBKEY {
        bail!("manifest contains a zero authority or contract");
    }
    if payload.current_fee_bps > MAX_FEE_BPS {
        bail!("manifest fee exceeds protocol maximum");
    }
    decode_sha256(&payload.storage_sha256, "storage hash")?;
    decode_sha256(&payload.history_sha256, "history hash")?;
    decode_sha256(&manifest.manifest_sha256, "manifest hash")?;
    let actual_hash = hex::encode(Sha256::digest(serde_json::to_vec(payload)?));
    if actual_hash != manifest.manifest_sha256 {
        bail!(
            "manifest hash mismatch: expected {}, got {actual_hash}",
            manifest.manifest_sha256
        );
    }
    if payload.program_call_count as usize > MAX_ROWS
        || payload.token_metrics.len() > MAX_ROWS
        || payload.listings.len() > MAX_ROWS
        || payload.offers.len() > MAX_ROWS
        || payload.auctions.len() > MAX_ROWS
        || payload.collection_offers.len() > MAX_ROWS
        || payload.unpaid_payouts.len() > MAX_ROWS
    {
        bail!("manifest exceeds migration safety bounds");
    }

    let mut token_keys = BTreeSet::new();
    let mut sale_count = 0u64;
    let mut mixed_volume = 0u64;
    let mut native_volume = 0u64;
    for row in &payload.token_metrics {
        let token = parsed_pubkey(&row.payment_token, "payment token")?;
        if !token_keys.insert(token) || row.sale_count == 0 || row.sale_volume == 0 {
            bail!("duplicate or empty token metric row {}", row.payment_token);
        }
        if row.realized_fees > row.sale_volume {
            bail!("token realized fees exceed volume");
        }
        sale_count = sale_count
            .checked_add(row.sale_count)
            .context("manifest sale count overflow")?;
        mixed_volume = mixed_volume
            .checked_add(row.sale_volume)
            .context("manifest sale volume overflow")?;
        if token == ZERO_PUBKEY {
            native_volume = row.sale_volume;
        }
    }
    if sale_count != payload.legacy_sale_count
        || mixed_volume != payload.legacy_mixed_sale_volume
        || native_volume != payload.native_sale_volume
        || (sale_count == 0) != payload.token_metrics.is_empty()
    {
        bail!("manifest token metrics do not match legacy aggregate evidence");
    }

    let mut listing_keys = BTreeSet::new();
    for row in &payload.listings {
        let seller = parsed_pubkey(&row.seller, "listing seller")?;
        let nft = parsed_pubkey(&row.nft_contract, "listing NFT")?;
        parsed_pubkey(&row.payment_token, "listing payment token")?;
        let key = format!("listing:{}:{}", row.nft_contract, row.token_id);
        if seller == ZERO_PUBKEY
            || nft == ZERO_PUBKEY
            || row.price == 0
            || !listing_keys.insert(key.clone())
        {
            bail!("invalid or duplicate {key}");
        }
        validate_royalty(&row.royalty_recipient, row.royalty_bps, &key)?;
        decode_sha256(&row.record_sha256, "listing record hash")?;
    }

    let mut offer_keys = BTreeSet::new();
    let mut custody_rows = 0u64;
    let mut native_custody = 0u64;
    for row in &payload.offers {
        let offerer = parsed_pubkey(&row.offerer, "offerer")?;
        let nft = parsed_pubkey(&row.nft_contract, "offer NFT")?;
        let payment = parsed_pubkey(&row.payment_token, "offer payment token")?;
        let key = format!(
            "offer:{}:{}:{}",
            row.nft_contract, row.token_id, row.offerer
        );
        if offerer == ZERO_PUBKEY
            || nft == ZERO_PUBKEY
            || row.price == 0
            || !offer_keys.insert(key.clone())
        {
            bail!("invalid or duplicate {key}");
        }
        validate_royalty(&row.royalty_recipient, row.royalty_bps, &key)?;
        decode_sha256(&row.record_sha256, "offer record hash")?;
        custody_rows = custody_rows
            .checked_add(1)
            .context("custody rows overflow")?;
        if payment == ZERO_PUBKEY {
            native_custody = native_custody
                .checked_add(row.price)
                .context("native custody overflow")?;
        }
    }

    let mut auction_keys = BTreeSet::new();
    for row in &payload.auctions {
        let seller = parsed_pubkey(&row.seller, "auction seller")?;
        let nft = parsed_pubkey(&row.nft_contract, "auction NFT")?;
        let bidder = parsed_pubkey(&row.highest_bidder, "auction bidder")?;
        let payment = parsed_pubkey(&row.payment_token, "auction payment token")?;
        let key = format!("auction:{}:{}", row.nft_contract, row.token_id);
        if seller == ZERO_PUBKEY
            || nft == ZERO_PUBKEY
            || row.start_price == 0
            || (row.highest_bid == 0) != (bidder == ZERO_PUBKEY)
            || !auction_keys.insert(key.clone())
        {
            bail!("invalid or duplicate {key}");
        }
        validate_royalty(&row.royalty_recipient, row.royalty_bps, &key)?;
        decode_sha256(&row.record_sha256, "auction record hash")?;
        if row.highest_bid > 0 {
            custody_rows = custody_rows
                .checked_add(1)
                .context("custody rows overflow")?;
            if payment == ZERO_PUBKEY {
                native_custody = native_custody
                    .checked_add(row.highest_bid)
                    .context("native custody overflow")?;
            }
        }
    }

    let mut collection_keys = BTreeSet::new();
    for row in &payload.collection_offers {
        let offerer = parsed_pubkey(&row.offerer, "collection offerer")?;
        let collection = parsed_pubkey(&row.collection, "collection")?;
        let payment = parsed_pubkey(&row.payment_token, "collection payment token")?;
        let key = format!("collection_offer:{}:{}", row.collection, row.offerer);
        if offerer == ZERO_PUBKEY
            || collection == ZERO_PUBKEY
            || row.price == 0
            || !collection_keys.insert(key.clone())
        {
            bail!("invalid or duplicate {key}");
        }
        validate_royalty(&row.royalty_recipient, row.royalty_bps, &key)?;
        decode_sha256(&row.record_sha256, "collection-offer record hash")?;
        custody_rows = custody_rows
            .checked_add(1)
            .context("custody rows overflow")?;
        if payment == ZERO_PUBKEY {
            native_custody = native_custody
                .checked_add(row.price)
                .context("native custody overflow")?;
        }
    }

    let mut payout_keys = BTreeSet::new();
    for row in &payload.unpaid_payouts {
        let token = parsed_pubkey(&row.payment_token, "payout token")?;
        let recipient = parsed_pubkey(&row.recipient, "payout recipient")?;
        let key = format!("payout:{}:{}", row.payment_token, row.recipient);
        if recipient == ZERO_PUBKEY || row.amount == 0 || !payout_keys.insert(key.clone()) {
            bail!("invalid or duplicate {key}");
        }
        decode_sha256(&row.record_sha256, "payout record hash")?;
        custody_rows = custody_rows
            .checked_add(1)
            .context("custody rows overflow")?;
        if token == ZERO_PUBKEY {
            native_custody = native_custody
                .checked_add(row.amount)
                .context("native custody overflow")?;
        }
    }
    if custody_rows != payload.expected_custody_rows
        || native_custody != payload.expected_native_custody
        || (custody_rows == 0 && native_custody != 0)
    {
        bail!("manifest custody totals do not match exact liability rows");
    }
    Ok(())
}

fn read_manifest(path: &Path) -> Result<MigrationManifest> {
    let manifest: MigrationManifest = serde_json::from_slice(&std::fs::read(path)?)?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T, overwrite: bool) -> Result<()> {
    if path.exists() && !overwrite {
        bail!("refusing to overwrite {}", path.display());
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("migration"),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .with_context(|| format!("failed to create {}", temp.display()))?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(&temp, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn load_keypair(path: &Path) -> Result<Keypair> {
    let password = keypair_password_from_env();
    KeypairFile::load_with_password_policy(path, password.as_deref(), true)
        .and_then(|file| file.to_keypair())
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("failed to load keypair {}", path.display()))
}

async fn verify_manifest_identity(
    rpc: &Rpc,
    contract: &str,
    manifest: &MigrationManifest,
) -> Result<()> {
    if manifest.payload.contract != contract || manifest.payload.chain_id != rpc.chain_id().await? {
        bail!("manifest network or contract identity mismatch");
    }
    Ok(())
}

struct MigrationAction {
    key: String,
    function: &'static str,
    args: Vec<u8>,
    value: u64,
    expected_code: u32,
}

fn contract_instruction(
    signer: Pubkey,
    contract: Pubkey,
    function: &str,
    args: Vec<u8>,
    value: u64,
) -> Result<Instruction> {
    let data = ContractInstruction::Call {
        function: function.to_string(),
        args,
        value,
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

fn sealed_status_matches(status: &MigrationStatus, manifest: &MigrationManifest) -> Result<()> {
    let expected_hash = decode_sha256(&manifest.manifest_sha256, "manifest hash")?;
    if status.version != 0
        || !status.locked
        || !status.paused
        || !status.sealed
        || status.manifest != expected_hash
        || status.expected_token_rows != manifest.payload.token_metrics.len() as u64
        || status.expected_sales != manifest.payload.legacy_sale_count
        || status.native_sale_volume != manifest.payload.native_sale_volume
        || status.expected_custody_rows != manifest.payload.expected_custody_rows
        || status.expected_native_custody != manifest.payload.expected_native_custody
    {
        bail!("on-chain migration seal differs from the manifest");
    }
    Ok(())
}

fn load_receipts(path: &Path, allowed: &BTreeSet<String>) -> Result<Vec<MigrationReceipt>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let receipts: Vec<MigrationReceipt> = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut seen = BTreeSet::new();
    for receipt in &receipts {
        if !allowed.contains(&receipt.action_key) {
            bail!("receipt contains unknown action {}", receipt.action_key);
        }
        if receipt.signature.is_empty() || !seen.insert(receipt.action_key.clone()) {
            bail!("receipt contains an empty signature or duplicate action");
        }
    }
    Ok(receipts)
}

async fn run_actions(
    rpc: &Rpc,
    contract: Pubkey,
    signer: &Keypair,
    actions: Vec<MigrationAction>,
    receipts_path: &Path,
    execute: bool,
    confirmation_attempts: usize,
) -> Result<usize> {
    let allowed: BTreeSet<String> = actions.iter().map(|action| action.key.clone()).collect();
    let mut receipts = load_receipts(receipts_path, &allowed)?;
    let mut completed: BTreeMap<String, String> = receipts
        .iter()
        .map(|receipt| (receipt.action_key.clone(), receipt.signature.clone()))
        .collect();
    for action in actions {
        if let Some(signature) = completed.get(&action.key) {
            rpc.verify_action_receipt(signature, signer.pubkey(), contract, &action)
                .await
                .with_context(|| format!("invalid receipt for {}", action.key))?;
            println!("verified_receipt={}", action.key);
            continue;
        }
        let instruction = contract_instruction(
            signer.pubkey(),
            contract,
            action.function,
            action.args,
            action.value,
        )?;
        let transaction = build_transaction(rpc, signer, instruction).await?;
        rpc.simulate(&transaction, action.expected_code)
            .await
            .with_context(|| format!("{} migration simulation", action.key))?;
        if !execute {
            println!("dry_run_action={}", action.key);
            continue;
        }
        let signature = rpc.send(&transaction).await?;
        rpc.wait_for_confirmation(&signature, confirmation_attempts)
            .await?;
        completed.insert(action.key.clone(), signature.clone());
        receipts.push(MigrationReceipt {
            action_key: action.key,
            signature,
        });
        write_json_atomic(receipts_path, &receipts, true)?;
    }
    if execute {
        println!("migration_actions_confirmed={}", receipts.len());
        println!("receipts={}", receipts_path.display());
    } else {
        println!("dry_run_complete=true");
    }
    Ok(receipts.len())
}

fn admin_actions(manifest: &MigrationManifest, authority: Pubkey) -> Result<Vec<MigrationAction>> {
    let fee = manifest.payload.current_fee_bps.to_le_bytes();
    let mut actions = vec![MigrationAction {
        key: "metrics:global".to_string(),
        function: "migrate_metrics_v3_global",
        args: layout_args(&[32], &[&authority.0]),
        value: 0,
        expected_code: 0,
    }];
    for row in &manifest.payload.token_metrics {
        let token = parsed_pubkey(&row.payment_token, "metric token")?;
        let count = row.sale_count.to_le_bytes();
        let volume = row.sale_volume.to_le_bytes();
        let fees = row.realized_fees.to_le_bytes();
        actions.push(MigrationAction {
            key: format!("metrics:token:{}", row.payment_token),
            function: "migrate_metrics_v3_token",
            args: layout_args(
                &[32, 32, 8, 8, 8],
                &[&authority.0, &token.0, &count, &volume, &fees],
            ),
            value: 0,
            expected_code: 0,
        });
    }
    for row in &manifest.payload.listings {
        let nft = parsed_pubkey(&row.nft_contract, "listing NFT")?;
        let recipient = parsed_pubkey(&row.royalty_recipient, "listing royalty recipient")?;
        let token_id = row.token_id.to_le_bytes();
        let bps = u64::from(row.royalty_bps).to_le_bytes();
        actions.push(MigrationAction {
            key: format!("terms:listing:{}:{}", row.nft_contract, row.token_id),
            function: "migrate_v3_listing",
            args: layout_args(
                &[32, 32, 8, 8, 32, 8],
                &[&authority.0, &nft.0, &token_id, &fee, &recipient.0, &bps],
            ),
            value: 0,
            expected_code: 0,
        });
    }
    for row in &manifest.payload.offers {
        let nft = parsed_pubkey(&row.nft_contract, "offer NFT")?;
        let offerer = parsed_pubkey(&row.offerer, "offerer")?;
        let recipient = parsed_pubkey(&row.royalty_recipient, "offer royalty recipient")?;
        let token_id = row.token_id.to_le_bytes();
        let bps = u64::from(row.royalty_bps).to_le_bytes();
        actions.push(MigrationAction {
            key: format!(
                "terms:offer:{}:{}:{}",
                row.nft_contract, row.token_id, row.offerer
            ),
            function: "migrate_v3_offer",
            args: layout_args(
                &[32, 32, 8, 32, 8, 32, 8],
                &[
                    &authority.0,
                    &nft.0,
                    &token_id,
                    &offerer.0,
                    &fee,
                    &recipient.0,
                    &bps,
                ],
            ),
            value: 0,
            expected_code: 0,
        });
    }
    for row in &manifest.payload.collection_offers {
        let collection = parsed_pubkey(&row.collection, "collection")?;
        let offerer = parsed_pubkey(&row.offerer, "collection offerer")?;
        let recipient =
            parsed_pubkey(&row.royalty_recipient, "collection-offer royalty recipient")?;
        let bps = u64::from(row.royalty_bps).to_le_bytes();
        actions.push(MigrationAction {
            key: format!("terms:collection_offer:{}:{}", row.collection, row.offerer),
            function: "migrate_v3_collection_offer",
            args: layout_args(
                &[32, 32, 32, 8, 32, 8],
                &[
                    &authority.0,
                    &collection.0,
                    &offerer.0,
                    &fee,
                    &recipient.0,
                    &bps,
                ],
            ),
            value: 0,
            expected_code: 0,
        });
    }
    Ok(actions)
}

fn treasury_actions(
    manifest: &MigrationManifest,
    treasury: Pubkey,
) -> Result<Vec<MigrationAction>> {
    let mut actions = Vec::new();
    for row in &manifest.payload.auctions {
        if row.highest_bid == 0 {
            continue;
        }
        let nft = parsed_pubkey(&row.nft_contract, "auction NFT")?;
        let token_id = row.token_id.to_le_bytes();
        actions.push(MigrationAction {
            key: format!("custody:auction_bid:{}:{}", row.nft_contract, row.token_id),
            function: "migrate_v3_auction_bid_custody",
            args: layout_args(&[32, 32, 8], &[&treasury.0, &nft.0, &token_id]),
            value: 0,
            expected_code: 1,
        });
    }
    for row in &manifest.payload.unpaid_payouts {
        let token = parsed_pubkey(&row.payment_token, "payout token")?;
        let recipient = parsed_pubkey(&row.recipient, "payout recipient")?;
        actions.push(MigrationAction {
            key: format!("custody:payout:{}:{}", row.payment_token, row.recipient),
            function: "migrate_v3_unpaid_payout_custody",
            args: layout_args(&[32, 32, 32], &[&treasury.0, &token.0, &recipient.0]),
            value: 0,
            expected_code: 1,
        });
    }
    Ok(actions)
}

fn offer_actions(
    manifest: &MigrationManifest,
    offerer: Pubkey,
    supply_native: bool,
) -> Result<Vec<MigrationAction>> {
    let mut actions = Vec::new();
    for row in manifest
        .payload
        .offers
        .iter()
        .filter(|row| row.offerer == offerer.to_base58())
    {
        let nft = parsed_pubkey(&row.nft_contract, "offer NFT")?;
        let token_id = row.token_id.to_le_bytes();
        let payment = parsed_pubkey(&row.payment_token, "offer payment token")?;
        actions.push(MigrationAction {
            key: format!(
                "custody:offer:{}:{}:{}",
                row.nft_contract, row.token_id, row.offerer
            ),
            function: "migrate_v3_offer_custody",
            args: layout_args(&[32, 32, 8], &[&offerer.0, &nft.0, &token_id]),
            value: if supply_native && payment == ZERO_PUBKEY {
                row.price
            } else {
                0
            },
            expected_code: 1,
        });
    }
    for row in manifest
        .payload
        .collection_offers
        .iter()
        .filter(|row| row.offerer == offerer.to_base58())
    {
        let collection = parsed_pubkey(&row.collection, "collection")?;
        let payment = parsed_pubkey(&row.payment_token, "collection payment token")?;
        actions.push(MigrationAction {
            key: format!(
                "custody:collection_offer:{}:{}",
                row.collection, row.offerer
            ),
            function: "migrate_v3_collection_offer_custody",
            args: layout_args(&[32, 32], &[&offerer.0, &collection.0]),
            value: if supply_native && payment == ZERO_PUBKEY {
                row.price
            } else {
                0
            },
            expected_code: 1,
        });
    }
    Ok(actions)
}

fn auction_actions(manifest: &MigrationManifest, seller: Pubkey) -> Result<Vec<MigrationAction>> {
    manifest
        .payload
        .auctions
        .iter()
        .filter(|row| row.seller == seller.to_base58())
        .map(|row| {
            let nft = parsed_pubkey(&row.nft_contract, "auction NFT")?;
            let token_id = row.token_id.to_le_bytes();
            Ok(MigrationAction {
                key: format!("custody:auction_nft:{}:{}", row.nft_contract, row.token_id),
                function: "migrate_auction_escrow",
                args: layout_args(&[32, 32, 8], &[&seller.0, &nft.0, &token_id]),
                value: 0,
                expected_code: 1,
            })
        })
        .collect()
}

fn verify_listing_terms(data: &[u8], row: &ListingRow, fee: u64) -> Result<()> {
    if data.len() != LISTING_SIZE + 8
        || read_pubkey(data, 0, "listing seller")?.to_base58() != row.seller
        || read_pubkey(data, 32, "listing NFT")?.to_base58() != row.nft_contract
        || read_u64(data, 64, "listing token ID")? != row.token_id
        || read_u64(data, 72, "listing price")? != row.price
        || read_pubkey(data, 80, "listing payment token")?.to_base58() != row.payment_token
        || read_pubkey(data, 112, "listing royalty recipient")?.to_base58() != row.royalty_recipient
        || data[144] != 1
        || u16::from_le_bytes(data[145..147].try_into().unwrap()) != row.royalty_bps
        || read_u64(data, LISTING_SIZE, "listing fee")? != fee
    {
        bail!("listing terms differ from manifest");
    }
    Ok(())
}

fn verify_offer_terms(data: &[u8], row: &OfferRow, fee: u64) -> Result<()> {
    let record_len = if row.expiry_slot.is_some() {
        OFFER_EXPIRY_SIZE
    } else {
        OFFER_SIZE
    };
    if data.len() != record_len + 48
        || hex::encode(Sha256::digest(&data[..record_len])) != row.record_sha256
        || read_u64(data, record_len, "offer fee")? != fee
        || read_pubkey(data, record_len + 8, "offer royalty recipient")?.to_base58()
            != row.royalty_recipient
        || read_u64(data, record_len + 40, "offer royalty bps")? != u64::from(row.royalty_bps)
    {
        bail!("offer terms differ from manifest");
    }
    Ok(())
}

fn verify_auction_terms(data: &[u8], row: &AuctionRow, fee: u64) -> Result<()> {
    if data.len() != AUCTION_SIZE + 8
        || read_pubkey(data, 0, "auction seller")?.to_base58() != row.seller
        || read_pubkey(data, 32, "auction NFT")?.to_base58() != row.nft_contract
        || read_u64(data, 64, "auction token ID")? != row.token_id
        || read_u64(data, 72, "auction start price")? != row.start_price
        || read_u64(data, 80, "auction reserve price")? != row.reserve_price
        || read_u64(data, 88, "auction highest bid")? != row.highest_bid
        || read_pubkey(data, 96, "auction highest bidder")?.to_base58() != row.highest_bidder
        || data[144] != 1
        || read_pubkey(data, 145, "auction payment token")?.to_base58() != row.payment_token
        || read_pubkey(data, 177, "auction royalty recipient")?.to_base58() != row.royalty_recipient
        || u16::from_le_bytes(data[209..211].try_into().unwrap()) != row.royalty_bps
        || read_u64(data, AUCTION_SIZE, "auction fee")? != fee
    {
        bail!("auction terms differ from manifest");
    }
    Ok(())
}

fn verify_collection_offer_terms(data: &[u8], row: &CollectionOfferRow, fee: u64) -> Result<()> {
    if data.len() != COLLECTION_OFFER_SIZE + 48
        || hex::encode(Sha256::digest(&data[..COLLECTION_OFFER_SIZE])) != row.record_sha256
        || read_u64(data, COLLECTION_OFFER_SIZE, "collection-offer fee")? != fee
        || read_pubkey(
            data,
            COLLECTION_OFFER_SIZE + 8,
            "collection-offer royalty recipient",
        )?
        .to_base58()
            != row.royalty_recipient
        || read_u64(
            data,
            COLLECTION_OFFER_SIZE + 40,
            "collection-offer royalty bps",
        )? != u64::from(row.royalty_bps)
    {
        bail!("collection-offer terms differ from manifest");
    }
    Ok(())
}

async fn verify_completed_migration(
    rpc: &Rpc,
    contract: &str,
    manifest: &MigrationManifest,
) -> Result<()> {
    verify_manifest_identity(rpc, contract, manifest).await?;
    let status = migration_status(rpc, contract).await?;
    let expected_hash = decode_sha256(&manifest.manifest_sha256, "manifest hash")?;
    if status.version != 3
        || status.locked
        || !status.paused
        || !status.sealed
        || status.manifest != expected_hash
        || status.expected_token_rows != status.migrated_token_rows
        || status.expected_token_rows != manifest.payload.token_metrics.len() as u64
        || status.expected_sales != status.migrated_sales
        || status.expected_sales != manifest.payload.legacy_sale_count
        || status.native_sale_volume != manifest.payload.native_sale_volume
        || status.expected_custody_rows != status.migrated_custody_rows
        || status.expected_custody_rows != manifest.payload.expected_custody_rows
        || status.expected_native_custody != status.reserved_native_custody
        || status.expected_native_custody != manifest.payload.expected_native_custody
    {
        bail!("completed migration status differs from the manifest");
    }
    let (admin, treasury, fee, paused) = marketplace_config(rpc, contract).await?;
    if admin.to_base58() != manifest.payload.admin
        || treasury.to_base58() != manifest.payload.treasury
        || fee != manifest.payload.current_fee_bps
        || !paused
    {
        bail!("marketplace configuration changed during migration");
    }
    let stats = rpc
        .readonly(contract, "get_marketplace_stats", Vec::new())
        .await?;
    if stats.len() != 32
        || read_u64(&stats, 8, "marketplace fee")? != fee
        || read_u64(&stats, 16, "marketplace sales")? != manifest.payload.legacy_sale_count
        || read_u64(&stats, 24, "marketplace native volume")? != manifest.payload.native_sale_volume
    {
        bail!("global marketplace metrics differ from the manifest");
    }
    for row in &manifest.payload.token_metrics {
        let token = parsed_pubkey(&row.payment_token, "metric token")?;
        let data = rpc
            .readonly(
                contract,
                "get_marketplace_token_stats",
                layout_args(&[32], &[&token.0]),
            )
            .await?;
        let expected_withdrawable = if token == ZERO_PUBKEY {
            row.realized_fees
        } else {
            0
        };
        if data.len() != 32
            || read_u64(&data, 0, "token sale count")? != row.sale_count
            || read_u64(&data, 8, "token sale volume")? != row.sale_volume
            || read_u64(&data, 16, "token realized fees")? != row.realized_fees
            || read_u64(&data, 24, "token withdrawable fees")? != expected_withdrawable
        {
            bail!("token metrics differ for {}", row.payment_token);
        }
    }
    for row in &manifest.payload.listings {
        let nft = parsed_pubkey(&row.nft_contract, "listing NFT")?;
        let token_id = row.token_id.to_le_bytes();
        let data = rpc
            .readonly(
                contract,
                "get_listing_terms",
                layout_args(&[32, 8], &[&nft.0, &token_id]),
            )
            .await?;
        verify_listing_terms(&data, row, fee)?;
    }
    for row in &manifest.payload.offers {
        let nft = parsed_pubkey(&row.nft_contract, "offer NFT")?;
        let offerer = parsed_pubkey(&row.offerer, "offerer")?;
        let token_id = row.token_id.to_le_bytes();
        let data = rpc
            .readonly(
                contract,
                "get_offer",
                layout_args(&[32, 8, 32], &[&nft.0, &token_id, &offerer.0]),
            )
            .await?;
        verify_offer_terms(&data, row, fee)?;
        let custody = rpc
            .readonly(
                contract,
                "get_offer_custody",
                layout_args(&[32, 8, 32], &[&nft.0, &token_id, &offerer.0]),
            )
            .await?;
        if custody != [1u8] {
            bail!("offer custody is not ready");
        }
    }
    for row in &manifest.payload.auctions {
        let nft = parsed_pubkey(&row.nft_contract, "auction NFT")?;
        let token_id = row.token_id.to_le_bytes();
        let data = rpc
            .readonly(
                contract,
                "get_auction_terms",
                layout_args(&[32, 8], &[&nft.0, &token_id]),
            )
            .await?;
        verify_auction_terms(&data, row, fee)?;
        let custody = rpc
            .readonly(
                contract,
                "get_auction_custody",
                layout_args(&[32, 8], &[&nft.0, &token_id]),
            )
            .await?;
        if custody != [1u8] {
            bail!("auction NFT custody is not ready");
        }
    }
    for row in &manifest.payload.collection_offers {
        let collection = parsed_pubkey(&row.collection, "collection")?;
        let offerer = parsed_pubkey(&row.offerer, "collection offerer")?;
        let data = rpc
            .readonly(
                contract,
                "get_collection_offer",
                layout_args(&[32, 32], &[&collection.0, &offerer.0]),
            )
            .await?;
        verify_collection_offer_terms(&data, row, fee)?;
        let custody = rpc
            .readonly(
                contract,
                "get_collection_offer_custody",
                layout_args(&[32, 32], &[&collection.0, &offerer.0]),
            )
            .await?;
        if custody != [1u8] {
            bail!("collection-offer custody is not ready");
        }
    }
    for row in &manifest.payload.unpaid_payouts {
        let token = parsed_pubkey(&row.payment_token, "payout token")?;
        let recipient = parsed_pubkey(&row.recipient, "payout recipient")?;
        let data = rpc
            .readonly_expected(
                contract,
                "get_unpaid_payout",
                layout_args(&[32, 32], &[&token.0, &recipient.0]),
                1,
            )
            .await?;
        if data.len() != 8
            || read_u64(&data, 0, "unpaid payout")? != row.amount
            || hex::encode(Sha256::digest(&data)) != row.record_sha256
        {
            bail!("unpaid payout differs from the manifest");
        }
    }
    let storage = rpc.storage(contract).await?;
    if exact_storage_u64(&storage, b"mm_sale_volume")? != manifest.payload.legacy_mixed_sale_volume
    {
        bail!("immutable legacy mixed sale volume changed during migration");
    }
    Ok(())
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
            println!("source_slot={}", manifest.payload.source_slot);
            println!("program_calls={}", manifest.payload.program_call_count);
            println!("sales={}", manifest.payload.legacy_sale_count);
            println!("token_rows={}", manifest.payload.token_metrics.len());
            println!("active_listings={}", manifest.payload.listings.len());
            println!("active_offers={}", manifest.payload.offers.len());
            println!("active_auctions={}", manifest.payload.auctions.len());
            println!(
                "active_collection_offers={}",
                manifest.payload.collection_offers.len()
            );
            println!("custody_rows={}", manifest.payload.expected_custody_rows);
            println!(
                "native_custody={}",
                manifest.payload.expected_native_custody
            );
        }
        Command::MigrateAdmin {
            contract,
            manifest,
            keypair,
            receipts,
            execute,
            confirmation_attempts,
        } => {
            let manifest = read_manifest(&manifest)?;
            verify_manifest_identity(&rpc, &contract, &manifest).await?;
            sealed_status_matches(&migration_status(&rpc, &contract).await?, &manifest)?;
            let signer = load_keypair(&keypair)?;
            if signer.pubkey().to_base58() != manifest.payload.admin {
                bail!("signer is not the manifest-bound marketplace admin");
            }
            let actions = admin_actions(&manifest, signer.pubkey())?;
            let contract_key = parsed_pubkey(&contract, "contract")?;
            run_actions(
                &rpc,
                contract_key,
                &signer,
                actions,
                &receipts,
                execute,
                confirmation_attempts,
            )
            .await?;
            if execute {
                let after = migration_status(&rpc, &contract).await?;
                if after.migrated_token_rows != manifest.payload.token_metrics.len() as u64
                    || after.migrated_sales != manifest.payload.legacy_sale_count
                {
                    bail!("confirmed admin migration does not cover exact token metrics");
                }
            }
        }
        Command::MigrateTreasury {
            contract,
            manifest,
            keypair,
            receipts,
            execute,
            confirmation_attempts,
        } => {
            let manifest = read_manifest(&manifest)?;
            verify_manifest_identity(&rpc, &contract, &manifest).await?;
            sealed_status_matches(&migration_status(&rpc, &contract).await?, &manifest)?;
            let signer = load_keypair(&keypair)?;
            if signer.pubkey().to_base58() != manifest.payload.treasury {
                bail!("signer is not the manifest-bound marketplace treasury");
            }
            let actions = treasury_actions(&manifest, signer.pubkey())?;
            let contract_key = parsed_pubkey(&contract, "contract")?;
            run_actions(
                &rpc,
                contract_key,
                &signer,
                actions,
                &receipts,
                execute,
                confirmation_attempts,
            )
            .await?;
        }
        Command::MigrateOffers {
            contract,
            manifest,
            keypair,
            receipts,
            supply_native,
            execute,
            confirmation_attempts,
        } => {
            let manifest = read_manifest(&manifest)?;
            verify_manifest_identity(&rpc, &contract, &manifest).await?;
            sealed_status_matches(&migration_status(&rpc, &contract).await?, &manifest)?;
            let signer = load_keypair(&keypair)?;
            let actions = offer_actions(&manifest, signer.pubkey(), supply_native)?;
            if actions.is_empty() {
                bail!("signer owns no active offer rows in the manifest");
            }
            let contract_key = parsed_pubkey(&contract, "contract")?;
            run_actions(
                &rpc,
                contract_key,
                &signer,
                actions,
                &receipts,
                execute,
                confirmation_attempts,
            )
            .await?;
        }
        Command::MigrateAuctions {
            contract,
            manifest,
            keypair,
            receipts,
            execute,
            confirmation_attempts,
        } => {
            let manifest = read_manifest(&manifest)?;
            verify_manifest_identity(&rpc, &contract, &manifest).await?;
            sealed_status_matches(&migration_status(&rpc, &contract).await?, &manifest)?;
            let signer = load_keypair(&keypair)?;
            let actions = auction_actions(&manifest, signer.pubkey())?;
            if actions.is_empty() {
                bail!("signer owns no active auction rows in the manifest");
            }
            let contract_key = parsed_pubkey(&contract, "contract")?;
            run_actions(
                &rpc,
                contract_key,
                &signer,
                actions,
                &receipts,
                execute,
                confirmation_attempts,
            )
            .await?;
        }
        Command::Verify { contract, manifest } => {
            let manifest = read_manifest(&manifest)?;
            verify_completed_migration(&rpc, &contract, &manifest).await?;
            println!("migration_verified=true");
            println!("sha256={}", manifest.manifest_sha256);
        }
        Command::BeginArgs { authority } => {
            let authority = parsed_pubkey(&authority, "authority")?;
            governed_payload(
                "begin_metrics_v3_migration",
                layout_args(&[32], &[&authority.0]),
            );
        }
        Command::SealArgs {
            authority,
            manifest,
        } => {
            let authority = parsed_pubkey(&authority, "authority")?;
            let manifest = read_manifest(&manifest)?;
            if authority.to_base58() != manifest.payload.admin {
                bail!("authority differs from manifest-bound admin");
            }
            let hash = decode_sha256(&manifest.manifest_sha256, "manifest hash")?;
            let token_rows = (manifest.payload.token_metrics.len() as u64).to_le_bytes();
            let sales = manifest.payload.legacy_sale_count.to_le_bytes();
            let native_volume = manifest.payload.native_sale_volume.to_le_bytes();
            let custody_rows = manifest.payload.expected_custody_rows.to_le_bytes();
            let native_custody = manifest.payload.expected_native_custody.to_le_bytes();
            governed_payload(
                "seal_metrics_v3_manifest",
                layout_args(
                    &[32, 32, 8, 8, 8, 8, 8],
                    &[
                        &authority.0,
                        &hash,
                        &token_rows,
                        &sales,
                        &native_volume,
                        &custody_rows,
                        &native_custody,
                    ],
                ),
            );
        }
        Command::CompleteArgs {
            authority,
            manifest,
        } => {
            let authority = parsed_pubkey(&authority, "authority")?;
            let manifest = read_manifest(&manifest)?;
            if authority.to_base58() != manifest.payload.admin {
                bail!("authority differs from manifest-bound admin");
            }
            governed_payload(
                "complete_metrics_v3_migration",
                layout_args(&[32], &[&authority.0]),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(function: &str, args: Vec<u8>, code: u32, sequence: u64) -> HistoricalCall {
        HistoricalCall {
            row: ProgramCallRow {
                slot: 100 + sequence,
                sequence,
                caller: Pubkey([9u8; 32]).to_base58(),
                function: function.to_string(),
                value: 0,
                tx_signature: hex::encode(Sha256::digest(sequence.to_le_bytes())),
            },
            args,
            tx_success: true,
            return_code: Some(code),
        }
    }

    fn minimal_payload() -> ManifestPayload {
        ManifestPayload {
            schema: 1,
            chain_id: "lichen-test".to_string(),
            source_slot: 99,
            contract: Pubkey([1u8; 32]).to_base58(),
            admin: Pubkey([2u8; 32]).to_base58(),
            treasury: Pubkey([3u8; 32]).to_base58(),
            current_fee_bps: 250,
            storage_sha256: hex::encode([4u8; 32]),
            history_sha256: hex::encode([5u8; 32]),
            program_call_count: 0,
            legacy_sale_count: 0,
            legacy_mixed_sale_volume: 0,
            native_sale_volume: 0,
            expected_custody_rows: 0,
            expected_native_custody: 0,
            token_metrics: Vec::new(),
            listings: Vec::new(),
            offers: Vec::new(),
            auctions: Vec::new(),
            collection_offers: Vec::new(),
            unpaid_payouts: Vec::new(),
        }
    }

    fn seal(payload: ManifestPayload) -> MigrationManifest {
        MigrationManifest {
            manifest_sha256: hex::encode(Sha256::digest(
                serde_json::to_vec(&payload).expect("serialize payload"),
            )),
            payload,
        }
    }

    #[test]
    fn argument_decoder_accepts_only_exact_descriptor_or_raw_layout() {
        let address = [7u8; 32];
        let number = 42u64.to_le_bytes();
        let canonical = layout_args(&[32, 8], &[&address, &number]);
        let fields = decoded_fields("sample", &canonical, &[32, 8]).expect("canonical");
        assert_eq!(fields[0], address);
        assert_eq!(fields[1], number);

        let mut raw = address.to_vec();
        raw.extend_from_slice(&number);
        let fields = decoded_fields("sample", &raw, &[32, 8]).expect("raw");
        assert_eq!(fields[0], address);
        assert_eq!(fields[1], number);

        assert!(decoded_fields("sample", &canonical[..canonical.len() - 1], &[32, 8]).is_err());
        let mut wrong_descriptor = canonical;
        wrong_descriptor[2] = 4;
        assert!(decoded_fields("sample", &wrong_descriptor, &[32, 8]).is_err());
    }

    #[test]
    fn replay_derives_dynamic_fee_and_token_specific_metrics() {
        let seller = Pubkey([10u8; 32]);
        let buyer = Pubkey([11u8; 32]);
        let nft = Pubkey([12u8; 32]);
        let offer_nft = Pubkey([13u8; 32]);
        let offerer = Pubkey([14u8; 32]);
        let admin = Pubkey([15u8; 32]);
        let quote = Pubkey([16u8; 32]);
        let token_id = 7u64.to_le_bytes();
        let offer_token_id = 8u64.to_le_bytes();
        let fee = 300u64.to_le_bytes();
        let native_price = 1_000u64.to_le_bytes();
        let quote_price = 2_000u64.to_le_bytes();

        let calls = vec![
            call(
                "set_marketplace_fee",
                layout_args(&[32, 8], &[&admin.0, &fee]),
                1,
                0,
            ),
            call(
                "list_nft",
                layout_args(
                    &[32, 32, 8, 8, 32],
                    &[&seller.0, &nft.0, &token_id, &native_price, &ZERO_PUBKEY.0],
                ),
                1,
                1,
            ),
            call(
                "buy_nft",
                layout_args(&[32, 32, 8], &[&buyer.0, &nft.0, &token_id]),
                1,
                2,
            ),
            call(
                "make_offer",
                layout_args(
                    &[32, 32, 8, 8, 32],
                    &[
                        &offerer.0,
                        &offer_nft.0,
                        &offer_token_id,
                        &quote_price,
                        &quote.0,
                    ],
                ),
                1,
                3,
            ),
            call(
                "accept_offer",
                layout_args(
                    &[32, 32, 8, 32],
                    &[&seller.0, &offer_nft.0, &offer_token_id, &offerer.0],
                ),
                1,
                4,
            ),
        ];
        let replay = replay_history(&calls).expect("replay");
        assert_eq!(replay.fee_bps, 300);
        assert_eq!(
            replay.token_metrics.get(&ZERO_PUBKEY),
            Some(&(1, 1_000, 30))
        );
        assert_eq!(replay.token_metrics.get(&quote), Some(&(1, 2_000, 60)));
        assert!(replay.listings.is_empty());
        assert!(replay.offers.is_empty());
    }

    #[test]
    fn legacy_zero_code_deactivation_is_marked_uncertain_not_counted_as_sale() {
        let seller = Pubkey([20u8; 32]);
        let nft = Pubkey([21u8; 32]);
        let offerer = Pubkey([22u8; 32]);
        let token_id = 9u64.to_le_bytes();
        let price = 5_000u64.to_le_bytes();
        let calls = vec![
            call(
                "make_offer",
                layout_args(
                    &[32, 32, 8, 8, 32],
                    &[&offerer.0, &nft.0, &token_id, &price, &ZERO_PUBKEY.0],
                ),
                1,
                0,
            ),
            call(
                "accept_offer",
                layout_args(
                    &[32, 32, 8, 32],
                    &[&seller.0, &nft.0, &token_id, &offerer.0],
                ),
                0,
                1,
            ),
        ];
        let replay = replay_history(&calls).expect("replay");
        let key = OfferKey(nft, 9, offerer);
        assert!(replay.offers.contains_key(&key));
        assert!(replay.uncertain_offers.contains(&key));
        assert!(replay.token_metrics.is_empty());
    }

    #[test]
    fn migration_status_decoder_is_exact() {
        let mut data = vec![0u8; 115];
        data[..8].copy_from_slice(&3u64.to_le_bytes());
        data[9] = 1;
        data[10] = 1;
        data[11..19].copy_from_slice(&2u64.to_le_bytes());
        data[19..27].copy_from_slice(&2u64.to_le_bytes());
        data[27..35].copy_from_slice(&4u64.to_le_bytes());
        data[35..43].copy_from_slice(&4u64.to_le_bytes());
        data[43..51].copy_from_slice(&500u64.to_le_bytes());
        data[51..59].copy_from_slice(&3u64.to_le_bytes());
        data[59..67].copy_from_slice(&3u64.to_le_bytes());
        data[67..75].copy_from_slice(&700u64.to_le_bytes());
        data[75..83].copy_from_slice(&700u64.to_le_bytes());
        data[83..].copy_from_slice(&[8u8; 32]);
        let status = decode_migration_status(&data).expect("status");
        assert_eq!(status.version, 3);
        assert!(!status.locked);
        assert!(status.paused && status.sealed);
        assert_eq!(status.expected_custody_rows, 3);
        assert_eq!(status.reserved_native_custody, 700);
        assert!(decode_migration_status(&data[..114]).is_err());
    }

    #[test]
    fn manifest_hash_and_custody_totals_are_fail_closed() {
        let payload = minimal_payload();
        let valid = seal(payload.clone());
        validate_manifest(&valid).expect("valid empty manifest");

        let mut bad_totals = payload.clone();
        bad_totals.expected_custody_rows = 1;
        assert!(validate_manifest(&seal(bad_totals)).is_err());

        let mut bad_hash = seal(payload);
        bad_hash.manifest_sha256 = hex::encode([0u8; 32]);
        assert!(validate_manifest(&bad_hash).is_err());
    }
}
