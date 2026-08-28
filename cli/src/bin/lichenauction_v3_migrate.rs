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
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONTRACT_PROGRAM_ID: Pubkey = Pubkey([0xFF; 32]);
const AUCTION_SIZE: usize = 169;
const OFFER_SIZE: usize = 121;
const MAX_ROWS: usize = 1_000_000;
const UNPAID_PAYOUT_PREFIX: &[u8] = b"unpaid_payout:";
const PLATFORM_FEE_PREFIX: &[u8] = b"ma_platform_fee:";

#[derive(Parser)]
#[command(
    name = "lichenauction-v3-migrate",
    about = "Capture, execute, and verify fail-closed LichenAuction V3 migrations"
)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8899")]
    rpc_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture every frozen legacy auction/offer and seal its canonical terms.
    Manifest {
        #[arg(long)]
        contract: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Simulate every row, and submit only when --execute is present.
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
    /// Verify on-chain snapshots and the completed migration counters.
    Verify {
        #[arg(long)]
        contract: String,
        #[arg(long)]
        manifest: PathBuf,
    },
    /// Print the governed begin_v3_migration payload.
    BeginArgs {
        #[arg(long)]
        authority: String,
    },
    /// Print the governed seal_v3_migration_manifest payload.
    SealArgs {
        #[arg(long)]
        authority: String,
        #[arg(long)]
        manifest: PathBuf,
    },
    /// Print the governed complete_v3_migration payload.
    CompleteArgs {
        #[arg(long)]
        authority: String,
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum RowKind {
    Auction,
    Offer,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum LiabilityKind {
    PlatformFee,
    UnpaidPayout,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ManifestRow {
    kind: RowKind,
    offerer: Option<String>,
    nft_contract: String,
    token_id: u64,
    record_sha256: String,
    active: bool,
    highest_bid: u64,
    offer_amount: u64,
    payment_token: String,
    royalty_recipient: String,
    royalty_bps: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct LiabilityRow {
    kind: LiabilityKind,
    payment_token: String,
    recipient: Option<String>,
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
    storage_sha256: String,
    legacy_escrow: String,
    contract_escrow: String,
    auction_count: u64,
    offer_count: u64,
    active_bid_liability: u64,
    active_offer_liability: u64,
    unpaid_payout_liability: u64,
    platform_fee_liability: u64,
    rows: Vec<ManifestRow>,
    liabilities: Vec<LiabilityRow>,
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
    row_key: String,
    signature: String,
}

#[derive(Clone, Debug)]
struct MigrationStatus {
    version: u64,
    locked: bool,
    paused: bool,
    sealed: bool,
    expected_auctions: u64,
    migrated_auctions: u64,
    expected_offers: u64,
    migrated_offers: u64,
    manifest: [u8; 32],
    legacy_escrow: Pubkey,
    contract_escrow: Pubkey,
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
        let encoded = base64::engine::general_purpose::STANDARD.encode(args);
        let result = self
            .call("callContract", json!([contract, function, encoded]))
            .await?;
        let code = result
            .get("returnCode")
            .or_else(|| result.get("return_code"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        if code != 0 {
            bail!("{function} returned contract code {code}");
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
            if all.len() > MAX_ROWS.saturating_mul(8) {
                bail!("contract storage exceeds migration safety bound");
            }
        }
        all.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(all)
    }

    async fn recent_blockhash(&self) -> Result<Hash> {
        let value = self.call("getRecentBlockhash", json!([])).await?;
        let blockhash = value
            .as_str()
            .or_else(|| value.get("blockhash").and_then(serde_json::Value::as_str))
            .context("getRecentBlockhash missing blockhash")?;
        Hash::from_hex(blockhash).map_err(anyhow::Error::msg)
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
            .context("sendTransaction returned no signature")
    }

    async fn wait_for_confirmation(&self, signature: &str, attempts: usize) -> Result<()> {
        for _ in 0..attempts {
            let result = self
                .call("getSignatureStatuses", json!([[signature]]))
                .await?;
            let status = result
                .get("value")
                .and_then(|value| value.as_array())
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
    let bytes: [u8; 8] = data
        .get(offset..offset + 8)
        .with_context(|| format!("missing {field}"))?
        .try_into()
        .map_err(|_| anyhow!("invalid {field}"))?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_pubkey(data: &[u8], offset: usize, field: &str) -> Result<Pubkey> {
    let bytes: [u8; 32] = data
        .get(offset..offset + 32)
        .with_context(|| format!("missing {field}"))?
        .try_into()
        .map_err(|_| anyhow!("invalid {field}"))?;
    Ok(Pubkey(bytes))
}

fn parse_key_address(value: &str, field: &str) -> Result<Pubkey> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid {field} key component");
    }
    let bytes: [u8; 32] = hex::decode(value)?
        .try_into()
        .map_err(|_| anyhow!("invalid {field} length"))?;
    Ok(Pubkey(bytes))
}

fn parse_auction_key(key: &[u8]) -> Result<Option<(Pubkey, u64)>> {
    let Ok(text) = std::str::from_utf8(key) else {
        return Ok(None);
    };
    let Some(suffix) = text.strip_prefix("auction_") else {
        return Ok(None);
    };
    let mut parts = suffix.split('_');
    let Some(nft) = parts.next() else {
        return Ok(None);
    };
    let Some(token_id) = parts.next() else {
        return Ok(None);
    };
    if parts.next().is_some() || nft.len() != 64 || !nft.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(None);
    }
    Ok(Some((
        parse_key_address(nft, "auction NFT")?,
        token_id.parse().context("invalid auction token ID")?,
    )))
}

fn parse_offer_key(key: &[u8]) -> Result<Option<(Pubkey, Pubkey, u64)>> {
    let Ok(text) = std::str::from_utf8(key) else {
        return Ok(None);
    };
    let Some(suffix) = text.strip_prefix("offer_") else {
        return Ok(None);
    };
    let mut parts = suffix.split('_');
    let Some(offerer) = parts.next() else {
        return Ok(None);
    };
    let Some(nft) = parts.next() else {
        return Ok(None);
    };
    let Some(token_id) = parts.next() else {
        return Ok(None);
    };
    if parts.next().is_some()
        || offerer.len() != 64
        || nft.len() != 64
        || !offerer.bytes().all(|b| b.is_ascii_hexdigit())
        || !nft.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Ok(None);
    }
    Ok(Some((
        parse_key_address(offerer, "offerer")?,
        parse_key_address(nft, "offer NFT")?,
        token_id.parse().context("invalid offer token ID")?,
    )))
}

fn parse_unpaid_payout_key(key: &[u8]) -> Result<Option<(Pubkey, Pubkey)>> {
    let Some(suffix) = key.strip_prefix(UNPAID_PAYOUT_PREFIX) else {
        return Ok(None);
    };
    if suffix.len() != 65 || suffix[32] != b':' {
        bail!("malformed unpaid payout storage key");
    }
    let token = Pubkey(
        suffix[..32]
            .try_into()
            .map_err(|_| anyhow!("invalid unpaid payout token"))?,
    );
    let recipient = Pubkey(
        suffix[33..]
            .try_into()
            .map_err(|_| anyhow!("invalid unpaid payout recipient"))?,
    );
    if recipient == Pubkey([0u8; 32]) {
        bail!("unpaid payout recipient cannot be zero");
    }
    Ok(Some((token, recipient)))
}

fn parse_platform_fee_key(key: &[u8]) -> Result<Option<Pubkey>> {
    let Some(suffix) = key.strip_prefix(PLATFORM_FEE_PREFIX) else {
        return Ok(None);
    };
    let token: [u8; 32] = suffix
        .try_into()
        .map_err(|_| anyhow!("malformed platform fee storage key"))?;
    Ok(Some(Pubkey(token)))
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
            "probe_canonical_royalty",
            layout_args(&[32, 8], &[&nft.0, &token]),
        )
        .await?;
    if data.len() != 34 {
        bail!("canonical royalty probe returned {} bytes", data.len());
    }
    let recipient = read_pubkey(&data, 0, "royalty recipient")?;
    let bps = u16::from_le_bytes(data[32..34].try_into().unwrap());
    if bps > 1_000 || (bps > 0 && recipient == Pubkey([0u8; 32])) {
        bail!("canonical royalty terms are invalid");
    }
    Ok((recipient, bps))
}

async fn migration_status(rpc: &Rpc, contract: &str) -> Result<MigrationStatus> {
    let data = rpc
        .readonly(contract, "get_v3_migration_status", Vec::new())
        .await?;
    decode_migration_status(&data)
}

fn decode_migration_status(data: &[u8]) -> Result<MigrationStatus> {
    if data.len() != 139 {
        bail!(
            "migration status returned {} bytes, expected 139",
            data.len()
        );
    }
    Ok(MigrationStatus {
        version: read_u64(data, 0, "version")?,
        locked: data[8] == 1,
        paused: data[9] == 1,
        sealed: data[10] == 1,
        expected_auctions: read_u64(data, 11, "expected auctions")?,
        migrated_auctions: read_u64(data, 19, "migrated auctions")?,
        expected_offers: read_u64(data, 27, "expected offers")?,
        migrated_offers: read_u64(data, 35, "migrated offers")?,
        manifest: data[43..75].try_into().unwrap(),
        legacy_escrow: read_pubkey(data, 75, "legacy escrow")?,
        contract_escrow: read_pubkey(data, 107, "contract escrow")?,
    })
}

fn storage_hash(entries: &[(Vec<u8>, Vec<u8>)]) -> String {
    let mut hasher = Sha256::new();
    for (key, value) in entries {
        hasher.update((key.len() as u32).to_le_bytes());
        hasher.update(key);
        hasher.update((value.len() as u32).to_le_bytes());
        hasher.update(value);
    }
    hex::encode(hasher.finalize())
}

async fn capture_manifest(rpc: &Rpc, contract: &str) -> Result<MigrationManifest> {
    let contract_key = Pubkey::from_base58(contract).map_err(anyhow::Error::msg)?;
    let status = migration_status(rpc, contract).await?;
    if status.version != 2 || !status.locked || !status.paused || status.sealed {
        bail!("capture requires paused, locked, unsealed legacy V2 state");
    }
    if status.migrated_auctions != 0 || status.migrated_offers != 0 {
        bail!("capture requires pristine zero migration counters");
    }
    if status.contract_escrow != contract_key {
        bail!("contract-reported escrow identity does not match its program address");
    }
    let entries = rpc.storage(contract).await?;
    let mut rows = Vec::new();
    let mut liabilities = Vec::new();
    let mut active_bid_liability = 0u64;
    let mut active_offer_liability = 0u64;
    let mut unpaid_payout_liability = 0u64;
    let mut platform_fee_liability = 0u64;
    for (key, value) in &entries {
        if let Some((nft, token_id)) = parse_auction_key(key)? {
            if value.len() != AUCTION_SIZE
                || read_pubkey(value, 32, "auction NFT")? != nft
                || read_u64(value, 64, "auction token ID")? != token_id
                || value[168] > 1
            {
                bail!("malformed auction row {}:{token_id}", nft.to_base58());
            }
            let seller = read_pubkey(value, 0, "auction seller")?;
            let minimum = read_u64(value, 72, "auction minimum")?;
            let highest_bidder = read_pubkey(value, 128, "highest bidder")?;
            let highest_bid = read_u64(value, 160, "highest bid")?;
            if seller == Pubkey([0u8; 32])
                || minimum == 0
                || ((highest_bid == 0) != (highest_bidder == Pubkey([0u8; 32])))
                || (highest_bid > 0 && highest_bid < minimum)
            {
                bail!(
                    "auction row {}:{token_id} violates invariants",
                    nft.to_base58()
                );
            }
            if value[168] == 1 && highest_bid > 0 {
                active_bid_liability = active_bid_liability
                    .checked_add(highest_bid)
                    .context("active bid liability overflow")?;
            }
            let (recipient, bps) = canonical_royalty(rpc, contract, nft, token_id).await?;
            rows.push(ManifestRow {
                kind: RowKind::Auction,
                offerer: None,
                nft_contract: nft.to_base58(),
                token_id,
                record_sha256: hex::encode(Sha256::digest(value)),
                active: value[168] == 1,
                highest_bid,
                offer_amount: 0,
                payment_token: read_pubkey(value, 80, "auction payment token")?.to_base58(),
                royalty_recipient: recipient.to_base58(),
                royalty_bps: bps,
            });
            continue;
        }
        if let Some((offerer, nft, token_id)) = parse_offer_key(key)? {
            if value.len() != OFFER_SIZE
                || read_pubkey(value, 0, "offerer")? != offerer
                || read_pubkey(value, 32, "offer NFT")? != nft
                || read_u64(value, 64, "offer token ID")? != token_id
                || read_u64(value, 72, "offer amount")? == 0
                || value[120] > 1
            {
                bail!("malformed offer row {}:{token_id}", nft.to_base58());
            }
            let offer_amount = read_u64(value, 72, "offer amount")?;
            if value[120] == 1 {
                active_offer_liability = active_offer_liability
                    .checked_add(offer_amount)
                    .context("active offer liability overflow")?;
            }
            let (recipient, bps) = canonical_royalty(rpc, contract, nft, token_id).await?;
            rows.push(ManifestRow {
                kind: RowKind::Offer,
                offerer: Some(offerer.to_base58()),
                nft_contract: nft.to_base58(),
                token_id,
                record_sha256: hex::encode(Sha256::digest(value)),
                active: value[120] == 1,
                highest_bid: 0,
                offer_amount,
                payment_token: read_pubkey(value, 80, "offer payment token")?.to_base58(),
                royalty_recipient: recipient.to_base58(),
                royalty_bps: bps,
            });
            continue;
        }
        if let Some((token, recipient)) = parse_unpaid_payout_key(key)? {
            if value.len() != 8 {
                bail!("malformed unpaid payout ledger row");
            }
            let amount = read_u64(value, 0, "unpaid payout")?;
            unpaid_payout_liability = unpaid_payout_liability
                .checked_add(amount)
                .context("unpaid payout liability overflow")?;
            liabilities.push(LiabilityRow {
                kind: LiabilityKind::UnpaidPayout,
                payment_token: token.to_base58(),
                recipient: Some(recipient.to_base58()),
                amount,
                record_sha256: hex::encode(Sha256::digest(value)),
            });
            continue;
        }
        if let Some(token) = parse_platform_fee_key(key)? {
            if value.len() != 8 {
                bail!("malformed platform fee ledger row");
            }
            let amount = read_u64(value, 0, "platform fee")?;
            platform_fee_liability = platform_fee_liability
                .checked_add(amount)
                .context("platform fee liability overflow")?;
            liabilities.push(LiabilityRow {
                kind: LiabilityKind::PlatformFee,
                payment_token: token.to_base58(),
                recipient: None,
                amount,
                record_sha256: hex::encode(Sha256::digest(value)),
            });
        }
    }
    if rows.len() > MAX_ROWS {
        bail!("migration row count exceeds {MAX_ROWS}");
    }
    rows.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.nft_contract.cmp(&b.nft_contract))
            .then(a.token_id.cmp(&b.token_id))
            .then(a.offerer.cmp(&b.offerer))
    });
    liabilities.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(a.payment_token.cmp(&b.payment_token))
            .then(a.recipient.cmp(&b.recipient))
    });
    let auction_count = rows
        .iter()
        .filter(|row| row.kind == RowKind::Auction)
        .count() as u64;
    let offer_count = rows.iter().filter(|row| row.kind == RowKind::Offer).count() as u64;
    if (active_bid_liability > 0
        || active_offer_liability > 0
        || unpaid_payout_liability > 0
        || platform_fee_liability > 0)
        && status.legacy_escrow != status.contract_escrow
    {
        bail!(
            "legacy custody mismatch: {} active bids, {} active offers, {} unpaid payouts, and {} platform fees are attributed to {}, not contract escrow {}; explicit source-backed recovery is required",
            active_bid_liability,
            active_offer_liability,
            unpaid_payout_liability,
            platform_fee_liability,
            status.legacy_escrow.to_base58(),
            status.contract_escrow.to_base58()
        );
    }
    let payload = ManifestPayload {
        schema: 1,
        chain_id: rpc.chain_id().await?,
        source_slot: rpc.slot().await?,
        contract: contract.to_string(),
        storage_sha256: storage_hash(&entries),
        legacy_escrow: status.legacy_escrow.to_base58(),
        contract_escrow: status.contract_escrow.to_base58(),
        auction_count,
        offer_count,
        active_bid_liability,
        active_offer_liability,
        unpaid_payout_liability,
        platform_fee_liability,
        rows,
        liabilities,
    };
    let manifest_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(&payload)?));
    let manifest = MigrationManifest {
        manifest_sha256,
        payload,
    };
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_manifest(manifest: &MigrationManifest) -> Result<()> {
    if manifest.payload.schema != 1 {
        bail!("unsupported manifest schema {}", manifest.payload.schema);
    }
    if manifest.payload.chain_id.is_empty() {
        bail!("manifest chain ID is empty");
    }
    let contract = Pubkey::from_base58(&manifest.payload.contract).map_err(anyhow::Error::msg)?;
    let legacy_escrow =
        Pubkey::from_base58(&manifest.payload.legacy_escrow).map_err(anyhow::Error::msg)?;
    let contract_escrow =
        Pubkey::from_base58(&manifest.payload.contract_escrow).map_err(anyhow::Error::msg)?;
    if contract != contract_escrow {
        bail!("manifest contract escrow does not match its program address");
    }
    decode_sha256(&manifest.payload.storage_sha256, "storage hash")?;
    decode_sha256(&manifest.manifest_sha256, "manifest hash")?;
    let actual = hex::encode(Sha256::digest(serde_json::to_vec(&manifest.payload)?));
    if actual != manifest.manifest_sha256 {
        bail!(
            "manifest hash mismatch: expected {}, got {actual}",
            manifest.manifest_sha256
        );
    }
    let auctions = manifest
        .payload
        .rows
        .iter()
        .filter(|row| row.kind == RowKind::Auction)
        .count() as u64;
    let offers = manifest
        .payload
        .rows
        .iter()
        .filter(|row| row.kind == RowKind::Offer)
        .count() as u64;
    if auctions != manifest.payload.auction_count || offers != manifest.payload.offer_count {
        bail!("manifest row counts do not match payload counters");
    }
    if manifest.payload.rows.len() > MAX_ROWS {
        bail!("manifest row count exceeds {MAX_ROWS}");
    }
    let mut keys = BTreeSet::new();
    let mut active_bid_liability = 0u64;
    let mut active_offer_liability = 0u64;
    for row in &manifest.payload.rows {
        let key = row_key(row);
        if !keys.insert(key.clone()) {
            bail!("duplicate manifest row {key}");
        }
        decode_sha256(&row.record_sha256, "row record hash")?;
        Pubkey::from_base58(&row.nft_contract).map_err(anyhow::Error::msg)?;
        Pubkey::from_base58(&row.payment_token).map_err(anyhow::Error::msg)?;
        let recipient = Pubkey::from_base58(&row.royalty_recipient).map_err(anyhow::Error::msg)?;
        if row.royalty_bps > 1_000 || (row.royalty_bps > 0 && recipient == Pubkey([0u8; 32])) {
            bail!("invalid royalty terms for {key}");
        }
        if row.kind == RowKind::Offer {
            Pubkey::from_base58(
                row.offerer
                    .as_deref()
                    .context("offer row missing offerer")?,
            )
            .map_err(anyhow::Error::msg)?;
            if row.highest_bid != 0 || row.offer_amount == 0 {
                bail!("offer row {key} has invalid amount fields");
            }
            if row.active {
                active_offer_liability = active_offer_liability
                    .checked_add(row.offer_amount)
                    .context("active offer liability overflow")?;
            }
        } else if row.offerer.is_some() {
            bail!("auction row unexpectedly contains an offerer");
        } else {
            if row.offer_amount != 0 {
                bail!("auction row {key} has an offer amount");
            }
            if row.active {
                active_bid_liability = active_bid_liability
                    .checked_add(row.highest_bid)
                    .context("active bid liability overflow")?;
            }
        }
    }
    if active_bid_liability != manifest.payload.active_bid_liability
        || active_offer_liability != manifest.payload.active_offer_liability
    {
        bail!("manifest active escrow liabilities do not match row records");
    }

    if manifest.payload.liabilities.len() > MAX_ROWS {
        bail!("manifest liability row count exceeds {MAX_ROWS}");
    }
    let mut liability_keys = BTreeSet::new();
    let mut unpaid_payout_liability = 0u64;
    let mut platform_fee_liability = 0u64;
    for row in &manifest.payload.liabilities {
        decode_sha256(&row.record_sha256, "liability record hash")?;
        Pubkey::from_base58(&row.payment_token).map_err(anyhow::Error::msg)?;
        let key = format!(
            "{:?}:{}:{}",
            row.kind,
            row.payment_token,
            row.recipient.as_deref().unwrap_or("-")
        );
        if !liability_keys.insert(key.clone()) {
            bail!("duplicate manifest liability row {key}");
        }
        match row.kind {
            LiabilityKind::PlatformFee => {
                if row.recipient.is_some() {
                    bail!("platform fee row {key} unexpectedly has a recipient");
                }
                platform_fee_liability = platform_fee_liability
                    .checked_add(row.amount)
                    .context("platform fee liability overflow")?;
            }
            LiabilityKind::UnpaidPayout => {
                let recipient = Pubkey::from_base58(
                    row.recipient
                        .as_deref()
                        .context("unpaid payout row missing recipient")?,
                )
                .map_err(anyhow::Error::msg)?;
                if recipient == Pubkey([0u8; 32]) {
                    bail!("unpaid payout recipient cannot be zero");
                }
                unpaid_payout_liability = unpaid_payout_liability
                    .checked_add(row.amount)
                    .context("unpaid payout liability overflow")?;
            }
        }
    }
    if unpaid_payout_liability != manifest.payload.unpaid_payout_liability
        || platform_fee_liability != manifest.payload.platform_fee_liability
    {
        bail!("manifest durable liabilities do not match ledger rows");
    }
    if legacy_escrow != contract_escrow
        && (active_bid_liability > 0
            || active_offer_liability > 0
            || unpaid_payout_liability > 0
            || platform_fee_liability > 0)
    {
        bail!("manifest would strand custody liabilities in the legacy escrow");
    }
    Ok(())
}

fn decode_sha256(value: &str, field: &str) -> Result<[u8; 32]> {
    hex::decode(value)
        .with_context(|| format!("invalid {field} hex"))?
        .try_into()
        .map_err(|_| anyhow!("{field} must be 32 bytes"))
}

fn row_key(row: &ManifestRow) -> String {
    format!(
        "{:?}:{}:{}:{}",
        row.kind,
        row.offerer.as_deref().unwrap_or("-"),
        row.nft_contract,
        row.token_id
    )
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

fn migration_call(row: &ManifestRow, authority: Pubkey) -> Result<(&'static str, Vec<u8>)> {
    let nft = Pubkey::from_base58(&row.nft_contract).map_err(anyhow::Error::msg)?;
    let recipient = Pubkey::from_base58(&row.royalty_recipient).map_err(anyhow::Error::msg)?;
    let token = row.token_id.to_le_bytes();
    let bps = u64::from(row.royalty_bps).to_le_bytes();
    match row.kind {
        RowKind::Auction => Ok((
            "migrate_v3_auction",
            layout_args(
                &[32, 32, 8, 32, 8],
                &[&authority.0, &nft.0, &token, &recipient.0, &bps],
            ),
        )),
        RowKind::Offer => {
            let offerer = Pubkey::from_base58(
                row.offerer
                    .as_deref()
                    .context("offer row missing offerer")?,
            )
            .map_err(anyhow::Error::msg)?;
            Ok((
                "migrate_v3_offer",
                layout_args(
                    &[32, 32, 32, 8, 32, 8],
                    &[&authority.0, &offerer.0, &nft.0, &token, &recipient.0, &bps],
                ),
            ))
        }
    }
}

async fn verify_rows(rpc: &Rpc, manifest: &MigrationManifest) -> Result<()> {
    for row in &manifest.payload.rows {
        let nft = Pubkey::from_base58(&row.nft_contract).map_err(anyhow::Error::msg)?;
        let token = row.token_id.to_le_bytes();
        let data = match row.kind {
            RowKind::Auction => {
                rpc.readonly(
                    &manifest.payload.contract,
                    "get_auction_info",
                    layout_args(&[32, 8], &[&nft.0, &token]),
                )
                .await?
            }
            RowKind::Offer => {
                let offerer = Pubkey::from_base58(
                    row.offerer
                        .as_deref()
                        .context("offer row missing offerer")?,
                )
                .map_err(anyhow::Error::msg)?;
                rpc.readonly(
                    &manifest.payload.contract,
                    "get_offer_info",
                    layout_args(&[32, 32, 8], &[&offerer.0, &nft.0, &token]),
                )
                .await?
            }
        };
        let terms_offset = match row.kind {
            RowKind::Auction => AUCTION_SIZE + 24,
            RowKind::Offer => OFFER_SIZE + 8,
        };
        let record_size = match row.kind {
            RowKind::Auction => AUCTION_SIZE,
            RowKind::Offer => OFFER_SIZE,
        };
        let expected_len = terms_offset + 34;
        if data.len() != expected_len {
            bail!(
                "{} returned {} bytes, expected {expected_len}",
                row_key(row),
                data.len()
            );
        }
        if hex::encode(Sha256::digest(&data[..record_size])) != row.record_sha256 {
            bail!("{} legacy record changed during migration", row_key(row));
        }
        let recipient = read_pubkey(&data, terms_offset, "snapshotted royalty recipient")?;
        let bps = u16::from_le_bytes(
            data[terms_offset + 32..terms_offset + 34]
                .try_into()
                .unwrap(),
        );
        if recipient.to_base58() != row.royalty_recipient || bps != row.royalty_bps {
            bail!("{} royalty snapshot differs from manifest", row_key(row));
        }
        if read_u64(
            &data,
            match row.kind {
                RowKind::Auction => AUCTION_SIZE + 16,
                RowKind::Offer => OFFER_SIZE,
            },
            "fee snapshot",
        )? != 250
        {
            bail!("{} legacy fee snapshot is not 250 bps", row_key(row));
        }
    }
    Ok(())
}

async fn verify_liabilities(rpc: &Rpc, manifest: &MigrationManifest) -> Result<()> {
    for row in &manifest.payload.liabilities {
        let token = Pubkey::from_base58(&row.payment_token).map_err(anyhow::Error::msg)?;
        let data = match row.kind {
            LiabilityKind::PlatformFee => {
                rpc.readonly(
                    &manifest.payload.contract,
                    "get_platform_fees",
                    layout_args(&[32], &[&token.0]),
                )
                .await?
            }
            LiabilityKind::UnpaidPayout => {
                let recipient = Pubkey::from_base58(
                    row.recipient
                        .as_deref()
                        .context("unpaid payout row missing recipient")?,
                )
                .map_err(anyhow::Error::msg)?;
                rpc.readonly(
                    &manifest.payload.contract,
                    "get_unpaid_payout",
                    layout_args(&[32, 32], &[&token.0, &recipient.0]),
                )
                .await?
            }
        };
        if data.len() != 8
            || read_u64(&data, 0, "liability amount")? != row.amount
            || hex::encode(Sha256::digest(&data)) != row.record_sha256
        {
            bail!(
                "liability row {:?}:{} changed during migration",
                row.kind,
                row.payment_token
            );
        }
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
            println!("auctions={}", manifest.payload.auction_count);
            println!("offers={}", manifest.payload.offer_count);
            println!(
                "active_bid_liability={}",
                manifest.payload.active_bid_liability
            );
            println!(
                "active_offer_liability={}",
                manifest.payload.active_offer_liability
            );
            println!(
                "unpaid_payout_liability={}",
                manifest.payload.unpaid_payout_liability
            );
            println!(
                "platform_fee_liability={}",
                manifest.payload.platform_fee_liability
            );
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
            let status = migration_status(&rpc, &contract).await?;
            let expected_hash = decode_sha256(&manifest.manifest_sha256, "manifest hash")?;
            if !status.locked
                || !status.sealed
                || status.manifest != expected_hash
                || status.expected_auctions != manifest.payload.auction_count
                || status.expected_offers != manifest.payload.offer_count
            {
                bail!("on-chain migration seal differs from manifest");
            }
            let signer = load_keypair(&keypair)?;
            let contract_key = Pubkey::from_base58(&contract).map_err(anyhow::Error::msg)?;
            let mut rows: Vec<MigrationReceipt> = if receipts.exists() {
                serde_json::from_slice(&std::fs::read(&receipts)?)?
            } else {
                Vec::new()
            };
            let mut completed: BTreeSet<String> =
                rows.iter().map(|row| row.row_key.clone()).collect();
            for row in &manifest.payload.rows {
                let key = row_key(row);
                if completed.contains(&key) {
                    continue;
                }
                let (function, args) = migration_call(row, signer.pubkey())?;
                let instruction =
                    contract_instruction(signer.pubkey(), contract_key, function, args)?;
                let transaction = build_transaction(&rpc, &signer, instruction).await?;
                rpc.simulate(&transaction)
                    .await
                    .with_context(|| format!("{} migration simulation", key))?;
                if !execute {
                    println!("dry_run_row={key}");
                    continue;
                }
                let signature = rpc.send(&transaction).await?;
                rpc.wait_for_confirmation(&signature, confirmation_attempts)
                    .await?;
                completed.insert(key.clone());
                rows.push(MigrationReceipt {
                    row_key: key,
                    signature,
                });
                write_json_atomic(&receipts, &rows, true)?;
            }
            if execute {
                let after = migration_status(&rpc, &contract).await?;
                if after.migrated_auctions != manifest.payload.auction_count
                    || after.migrated_offers != manifest.payload.offer_count
                {
                    bail!("confirmed migration counters do not cover the manifest");
                }
                println!("migration_rows_confirmed={}", rows.len());
                println!("receipts={}", receipts.display());
            } else {
                println!("dry_run_complete=true");
            }
        }
        Command::Verify { contract, manifest } => {
            let manifest = read_manifest(&manifest)?;
            if manifest.payload.contract != contract
                || manifest.payload.chain_id != rpc.chain_id().await?
            {
                bail!("manifest network or contract identity mismatch");
            }
            let status = migration_status(&rpc, &contract).await?;
            let expected_hash = decode_sha256(&manifest.manifest_sha256, "manifest hash")?;
            if status.version != 3
                || status.locked
                || !status.sealed
                || status.manifest != expected_hash
                || status.expected_auctions != status.migrated_auctions
                || status.expected_offers != status.migrated_offers
                || status.migrated_auctions != manifest.payload.auction_count
                || status.migrated_offers != manifest.payload.offer_count
                || status.legacy_escrow != status.contract_escrow
            {
                bail!("completed migration status does not match the manifest");
            }
            verify_rows(&rpc, &manifest).await?;
            verify_liabilities(&rpc, &manifest).await?;
            println!("migration_verified=true");
            println!("sha256={}", manifest.manifest_sha256);
        }
        Command::BeginArgs { authority } => {
            let authority = Pubkey::from_base58(&authority).map_err(anyhow::Error::msg)?;
            governed_payload("begin_v3_migration", layout_args(&[32], &[&authority.0]));
        }
        Command::SealArgs {
            authority,
            manifest,
        } => {
            let authority = Pubkey::from_base58(&authority).map_err(anyhow::Error::msg)?;
            let manifest = read_manifest(&manifest)?;
            let hash = decode_sha256(&manifest.manifest_sha256, "manifest hash")?;
            let auctions = manifest.payload.auction_count.to_le_bytes();
            let offers = manifest.payload.offer_count.to_le_bytes();
            governed_payload(
                "seal_v3_migration_manifest",
                layout_args(&[32, 32, 8, 8], &[&authority.0, &hash, &auctions, &offers]),
            );
        }
        Command::CompleteArgs { authority } => {
            let authority = Pubkey::from_base58(&authority).map_err(anyhow::Error::msg)?;
            governed_payload("complete_v3_migration", layout_args(&[32], &[&authority.0]));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seal(payload: ManifestPayload) -> MigrationManifest {
        let manifest_sha256 = hex::encode(Sha256::digest(
            serde_json::to_vec(&payload).expect("serialize"),
        ));
        MigrationManifest {
            manifest_sha256,
            payload,
        }
    }

    fn valid_payload() -> ManifestPayload {
        let contract = Pubkey([4u8; 32]).to_base58();
        let payment_token = Pubkey([5u8; 32]).to_base58();
        let recipient = Pubkey([6u8; 32]).to_base58();
        ManifestPayload {
            schema: 1,
            chain_id: "lichen-test".into(),
            source_slot: 42,
            contract: contract.clone(),
            storage_sha256: hex::encode([7u8; 32]),
            legacy_escrow: contract.clone(),
            contract_escrow: contract,
            auction_count: 1,
            offer_count: 1,
            active_bid_liability: 50,
            active_offer_liability: 70,
            unpaid_payout_liability: 5,
            platform_fee_liability: 3,
            rows: vec![
                ManifestRow {
                    kind: RowKind::Auction,
                    offerer: None,
                    nft_contract: Pubkey([8u8; 32]).to_base58(),
                    token_id: 1,
                    record_sha256: hex::encode([9u8; 32]),
                    active: true,
                    highest_bid: 50,
                    offer_amount: 0,
                    payment_token: payment_token.clone(),
                    royalty_recipient: recipient.clone(),
                    royalty_bps: 500,
                },
                ManifestRow {
                    kind: RowKind::Offer,
                    offerer: Some(Pubkey([10u8; 32]).to_base58()),
                    nft_contract: Pubkey([8u8; 32]).to_base58(),
                    token_id: 2,
                    record_sha256: hex::encode([11u8; 32]),
                    active: true,
                    highest_bid: 0,
                    offer_amount: 70,
                    payment_token: payment_token.clone(),
                    royalty_recipient: recipient.clone(),
                    royalty_bps: 500,
                },
            ],
            liabilities: vec![
                LiabilityRow {
                    kind: LiabilityKind::PlatformFee,
                    payment_token: payment_token.clone(),
                    recipient: None,
                    amount: 3,
                    record_sha256: hex::encode([12u8; 32]),
                },
                LiabilityRow {
                    kind: LiabilityKind::UnpaidPayout,
                    payment_token,
                    recipient: Some(Pubkey([13u8; 32]).to_base58()),
                    amount: 5,
                    record_sha256: hex::encode([14u8; 32]),
                },
            ],
        }
    }

    #[test]
    fn rpc_storage_entry_uses_unambiguous_canonical_fields() {
        let entry: StorageEntry = serde_json::from_value(json!({
            "key": "aa",
            "key_hex": "bb",
            "value": "cc",
            "value_hex": "dd"
        }))
        .expect("canonical RPC entry");
        assert_eq!(entry.key_hex, "bb");
        assert_eq!(entry.value_hex, "dd");
        assert!(serde_json::from_value::<StorageEntry>(json!({
            "key": "aa",
            "value": "cc"
        }))
        .is_err());
    }

    #[test]
    fn liability_keys_are_binary_exact_and_fail_closed() {
        let token = [1u8; 32];
        let recipient = [2u8; 32];
        let mut unpaid = UNPAID_PAYOUT_PREFIX.to_vec();
        unpaid.extend_from_slice(&token);
        unpaid.push(b':');
        unpaid.extend_from_slice(&recipient);
        assert_eq!(
            parse_unpaid_payout_key(&unpaid).expect("parse"),
            Some((Pubkey(token), Pubkey(recipient)))
        );
        unpaid.pop();
        assert!(parse_unpaid_payout_key(&unpaid).is_err());

        let mut platform = PLATFORM_FEE_PREFIX.to_vec();
        platform.extend_from_slice(&token);
        assert_eq!(
            parse_platform_fee_key(&platform).expect("parse"),
            Some(Pubkey(token))
        );
        platform.push(0);
        assert!(parse_platform_fee_key(&platform).is_err());
    }

    #[test]
    fn status_decoder_requires_the_exact_v3_layout() {
        let mut data = vec![0u8; 139];
        data[..8].copy_from_slice(&2u64.to_le_bytes());
        data[8] = 1;
        data[9] = 1;
        data[10] = 1;
        data[11..19].copy_from_slice(&3u64.to_le_bytes());
        data[19..27].copy_from_slice(&1u64.to_le_bytes());
        data[27..35].copy_from_slice(&4u64.to_le_bytes());
        data[35..43].copy_from_slice(&2u64.to_le_bytes());
        data[43..75].copy_from_slice(&[5u8; 32]);
        data[75..107].copy_from_slice(&[6u8; 32]);
        data[107..139].copy_from_slice(&[7u8; 32]);
        let status = decode_migration_status(&data).expect("status");
        assert_eq!(status.version, 2);
        assert!(status.locked && status.paused && status.sealed);
        assert_eq!(status.expected_auctions, 3);
        assert_eq!(status.migrated_auctions, 1);
        assert_eq!(status.expected_offers, 4);
        assert_eq!(status.migrated_offers, 2);
        assert_eq!(status.manifest, [5u8; 32]);
        assert_eq!(status.legacy_escrow, Pubkey([6u8; 32]));
        assert_eq!(status.contract_escrow, Pubkey([7u8; 32]));
        assert!(decode_migration_status(&data[..138]).is_err());
    }

    #[test]
    fn manifest_rederives_all_custody_liabilities() {
        let manifest = seal(valid_payload());
        validate_manifest(&manifest).expect("valid manifest");

        let mut wrong_offer_total = valid_payload();
        wrong_offer_total.active_offer_liability += 1;
        assert!(validate_manifest(&seal(wrong_offer_total)).is_err());

        let mut wrong_ledger_total = valid_payload();
        wrong_ledger_total.platform_fee_liability += 1;
        assert!(validate_manifest(&seal(wrong_ledger_total)).is_err());

        let mut stranded = valid_payload();
        stranded.legacy_escrow = Pubkey([15u8; 32]).to_base58();
        assert!(validate_manifest(&seal(stranded)).is_err());
    }

    #[test]
    fn migration_payloads_bind_the_authenticated_admin() {
        let row = valid_payload().rows.remove(0);
        let authority = Pubkey([16u8; 32]);
        let (_, args) = migration_call(&row, authority).expect("migration call");
        assert_eq!(&args[..6], &[0xAB, 32, 32, 8, 32, 8]);
        assert_eq!(&args[6..38], &authority.0);
    }

    #[test]
    fn operator_evidence_is_atomic_and_manifests_are_no_clobber() {
        let directory = tempfile::tempdir().expect("temp directory");
        let output = directory.path().join("manifest.json");
        let first = seal(valid_payload());
        let mut changed = valid_payload();
        changed.source_slot += 1;
        let second = seal(changed);

        write_json_atomic(&output, &first, false).expect("initial write");
        assert!(write_json_atomic(&output, &second, false).is_err());
        assert_eq!(
            read_manifest(&output).expect("read").manifest_sha256,
            first.manifest_sha256
        );
        write_json_atomic(&output, &second, true).expect("replace receipts");
        assert_eq!(
            read_manifest(&output).expect("read").manifest_sha256,
            second.manifest_sha256
        );
        assert_eq!(
            std::fs::read_dir(directory.path()).expect("list").count(),
            1
        );
    }
}
