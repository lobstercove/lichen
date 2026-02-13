// MoltChain Validator with BFT Consensus + P2P Network + RPC Server
// Week 4: Multi-validator networking with QUIC transport + RPC integration
// Week 5: Block broadcasting, mempool, and multi-validator consensus

mod keypair_loader;
mod sync;
mod threshold_signer;
pub mod updater;

use moltchain_core::nft::decode_token_state;
use moltchain_core::{
    evm_tx_hash, Account, Block, ContractAccount, ContractContext, ContractInstruction,
    ContractRuntime, FeeConfig, GenesisConfig, GenesisWallet, Hash, Instruction, Keypair,
    MarketActivity, MarketActivityKind, Mempool, Message, NftActivity, NftActivityKind,
    ProgramCallActivity, Pubkey, SlashingEvidence, SlashingOffense, SlashingTracker, StakePool,
    StateStore, SymbolRegistryEntry, Transaction, TxProcessor, ValidatorInfo, ValidatorSet, Vote,
    VoteAggregator, BASE_FEE, CONTRACT_DEPLOY_FEE, CONTRACT_UPGRADE_FEE, EVM_PROGRAM_ID,
    HEARTBEAT_BLOCK_REWARD, MIN_VALIDATOR_STAKE, NFT_COLLECTION_FEE, NFT_MINT_FEE,
    SYSTEM_PROGRAM_ID as CORE_SYSTEM_PROGRAM_ID, TRANSACTION_BLOCK_REWARD,
};
use moltchain_p2p::{
    ConsistencyReportMsg, MessageType, P2PConfig, P2PMessage, P2PNetwork, SnapshotKind,
    SnapshotRequestMsg, SnapshotResponseMsg, StatusRequestMsg, StatusResponseMsg,
};
use moltchain_rpc::start_rpc_server;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use sync::SyncManager;
use tokio::sync::{mpsc, Mutex};
use tokio::time;
use tracing::{debug, error, info, warn};

const SYSTEM_ACCOUNT_OWNER: Pubkey = Pubkey([0x01; 32]);
const GENESIS_MINT_PUBKEY: Pubkey = Pubkey([0xFE; 32]);
const REWARD_POOL_MOLT: u64 = 150_000_000; // 15% of 1B supply

/// Exit code used by the internal health watchdog to signal the supervisor
/// that the validator should be restarted (deadlock/stall detected).
const EXIT_CODE_RESTART: i32 = 75;

/// Default number of seconds with no block activity before the watchdog
/// triggers a restart.  Override with `--watchdog-timeout <secs>`.
const DEFAULT_WATCHDOG_TIMEOUT_SECS: u64 = 120;

/// Maximum number of automatic restarts before the supervisor gives up.
/// Override with `--max-restarts <n>`.
const DEFAULT_MAX_RESTARTS: u32 = 50;

#[derive(Debug, Deserialize)]
struct SeedsFile {
    testnet: Option<SeedNetwork>,
    mainnet: Option<SeedNetwork>,
    devnet: Option<SeedNetwork>,
}

#[derive(Debug, Deserialize)]
struct SeedNetwork {
    #[allow(dead_code)]
    chain_id: String,
    #[serde(default)]
    bootstrap_peers: Vec<String>,
    #[serde(default)]
    seeds: Vec<SeedEntry>,
}

#[derive(Debug, Deserialize)]
struct SeedEntry {
    address: String,
}

fn resolve_peer_list(peers: &[String]) -> Vec<SocketAddr> {
    let mut resolved = Vec::new();
    for peer in peers {
        if let Ok(addr) = peer.parse::<SocketAddr>() {
            resolved.push(addr);
            continue;
        }
        if let Ok(addrs) = peer.to_socket_addrs() {
            resolved.extend(addrs);
        }
    }
    resolved
}

fn load_seed_peers(chain_id: &str, seeds_path: &Path) -> Vec<String> {
    let contents = match fs::read_to_string(seeds_path) {
        Ok(data) => data,
        Err(_) => return Vec::new(),
    };

    let seeds: SeedsFile = match serde_json::from_str(&contents) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };

    let network = if chain_id.contains("mainnet") {
        seeds.mainnet
    } else if chain_id.contains("testnet") {
        seeds.testnet
    } else if chain_id.contains("devnet") {
        seeds.devnet
    } else {
        None
    };

    let mut peers = Vec::new();
    if let Some(network) = network {
        peers.extend(network.bootstrap_peers);
        peers.extend(network.seeds.into_iter().map(|seed| seed.address));
    }

    peers
}

#[derive(Serialize)]
struct ValidatorHashEntry {
    pubkey: Pubkey,
    reputation: u64,
    stake: u64,
    joined_slot: u64,
    last_active_slot: u64,
}

fn hash_validator_set(set: &ValidatorSet) -> Hash {
    let entries: Vec<ValidatorHashEntry> = set
        .sorted_validators()
        .into_iter()
        .map(|validator| ValidatorHashEntry {
            pubkey: validator.pubkey,
            reputation: validator.reputation,
            stake: validator.stake,
            joined_slot: validator.joined_slot,
            last_active_slot: validator.last_active_slot,
        })
        .collect();

    let data = serde_json::to_vec(&entries).unwrap_or_default();
    Hash::hash(&data)
}

fn hash_stake_pool(pool: &StakePool) -> Hash {
    let entries = pool.stake_entries();
    let data = serde_json::to_vec(&entries).unwrap_or_default();
    Hash::hash(&data)
}

#[derive(Deserialize)]
struct TreasuryKeyFile {
    secret_key: String,
}

fn resolve_treasury_keypair_path(
    genesis_wallet: Option<&GenesisWallet>,
    keys_dir: &Path,
    chain_id: &str,
) -> Option<PathBuf> {
    if let Some(wallet) = genesis_wallet {
        if let Some(path) = wallet.treasury_keypair_path.as_ref() {
            let candidate = PathBuf::from(path);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    let candidate = keys_dir.join(format!("treasury-{}.json", chain_id));
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

fn load_treasury_keypair(
    genesis_wallet: Option<&GenesisWallet>,
    keys_dir: &Path,
    chain_id: &str,
) -> Option<Keypair> {
    let path = resolve_treasury_keypair_path(genesis_wallet, keys_dir, chain_id)?;
    let contents = match fs::read_to_string(&path) {
        Ok(data) => data,
        Err(e) => {
            warn!(
                "⚠️  Failed to read treasury keypair {}: {}",
                path.display(),
                e
            );
            return None;
        }
    };

    let parsed: TreasuryKeyFile = match serde_json::from_str(&contents) {
        Ok(file) => file,
        Err(e) => {
            warn!(
                "⚠️  Failed to parse treasury keypair {}: {}",
                path.display(),
                e
            );
            return None;
        }
    };

    let bytes = match hex::decode(parsed.secret_key) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!(
                "⚠️  Failed to decode treasury keypair {}: {}",
                path.display(),
                e
            );
            return None;
        }
    };

    if bytes.len() != 32 {
        warn!(
            "⚠️  Treasury keypair {} has invalid length {}",
            path.display(),
            bytes.len()
        );
        return None;
    }

    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes[..32]);
    info!("🔐 Loaded treasury keypair from {}", path.display());
    Some(Keypair::from_seed(&seed))
}

fn is_reward_or_debt_tx(tx: &Transaction) -> bool {
    let Some(ix) = tx.message.instructions.first() else {
        return false;
    };

    if ix.program_id != CORE_SYSTEM_PROGRAM_ID {
        return false;
    }

    matches!(ix.data.first(), Some(2) | Some(3))
}

fn block_has_user_transactions(block: &Block) -> bool {
    // With protocol-level rewards (coinbase model), blocks only contain user txs.
    // Keep the is_reward_or_debt_tx filter for backward-compat with legacy blocks.
    block
        .transactions
        .iter()
        .any(|tx| !is_reward_or_debt_tx(tx))
}

fn record_block_activity(state: &StateStore, block: &Block) {
    let mut activity_seq: u32 = 0;

    for tx in &block.transactions {
        let tx_signature = tx.signature();
        for ix in &tx.message.instructions {
            if ix.program_id == CORE_SYSTEM_PROGRAM_ID {
                match ix.data.first() {
                    Some(7) => {
                        if ix.accounts.len() < 4 {
                            continue;
                        }

                        let collection = ix.accounts[1];
                        let token = ix.accounts[2];
                        let owner = ix.accounts[3];

                        let activity = NftActivity {
                            slot: block.header.slot,
                            timestamp: block.header.timestamp,
                            kind: NftActivityKind::Mint,
                            collection,
                            token,
                            from: None,
                            to: owner,
                            tx_signature,
                        };

                        if let Err(e) = state.record_nft_activity(&activity, activity_seq) {
                            warn!("⚠️  Failed to record NFT mint activity: {}", e);
                        }

                        activity_seq = activity_seq.saturating_add(1);
                    }
                    Some(8) => {
                        if ix.accounts.len() < 3 {
                            continue;
                        }

                        let from = ix.accounts[0];
                        let token = ix.accounts[1];
                        let to = ix.accounts[2];

                        let token_account = match state.get_account(&token) {
                            Ok(Some(account)) => account,
                            _ => continue,
                        };

                        let token_state = match decode_token_state(&token_account.data) {
                            Ok(state) => state,
                            Err(_) => continue,
                        };

                        let activity = NftActivity {
                            slot: block.header.slot,
                            timestamp: block.header.timestamp,
                            kind: NftActivityKind::Transfer,
                            collection: token_state.collection,
                            token,
                            from: Some(from),
                            to,
                            tx_signature,
                        };

                        if let Err(e) = state.record_nft_activity(&activity, activity_seq) {
                            warn!("⚠️  Failed to record NFT transfer activity: {}", e);
                        }

                        activity_seq = activity_seq.saturating_add(1);
                    }
                    _ => {}
                }
            } else if ix.program_id == moltchain_core::CONTRACT_PROGRAM_ID {
                if let Ok(ContractInstruction::Call {
                    function,
                    args,
                    value,
                }) = ContractInstruction::deserialize(&ix.data)
                {
                    if ix.accounts.len() < 2 {
                        continue;
                    }

                    let caller = ix.accounts[0];
                    let program = ix.accounts[1];

                    let activity = ProgramCallActivity {
                        slot: block.header.slot,
                        timestamp: block.header.timestamp,
                        program,
                        caller,
                        function: function.clone(),
                        value,
                        tx_signature,
                    };

                    if let Err(e) = state.record_program_call(&activity, activity_seq) {
                        warn!("⚠️  Failed to record program call: {}", e);
                    }

                    let market_kind = match function.as_str() {
                        "list_nft" => Some(MarketActivityKind::Listing),
                        "buy_nft" => Some(MarketActivityKind::Sale),
                        "cancel_listing" => Some(MarketActivityKind::Cancel),
                        _ => None,
                    };

                    if let Some(kind) = market_kind {
                        let market_activity = build_market_activity(
                            kind,
                            function,
                            program,
                            caller,
                            &args,
                            block.header.slot,
                            block.header.timestamp,
                            tx_signature,
                        );

                        if let Err(e) = state.record_market_activity(&market_activity, activity_seq)
                        {
                            warn!("⚠️  Failed to record market activity: {}", e);
                        }

                        activity_seq = activity_seq.saturating_add(1);
                    } else {
                        activity_seq = activity_seq.saturating_add(1);
                    }
                }
            }
        }
    }
}

struct ParsedMarketArgs {
    collection: Option<Pubkey>,
    token: Option<Pubkey>,
    token_id: Option<u64>,
    price: Option<u64>,
    seller: Option<Pubkey>,
    buyer: Option<Pubkey>,
}

fn parse_marketplace_args(args: &[u8]) -> ParsedMarketArgs {
    let mut parsed = ParsedMarketArgs {
        collection: None,
        token: None,
        token_id: None,
        price: None,
        seller: None,
        buyer: None,
    };

    if args.is_empty() {
        return parsed;
    }

    let Ok(value) = serde_json::from_slice::<JsonValue>(args) else {
        return parsed;
    };

    let Some(obj) = value.as_object() else {
        return parsed;
    };

    let parse_pubkey = |val: &JsonValue| -> Option<Pubkey> {
        let s = val.as_str()?;
        Pubkey::from_base58(s).ok()
    };

    let parse_u64 = |val: &JsonValue| -> Option<u64> {
        if let Some(num) = val.as_u64() {
            return Some(num);
        }
        val.as_str().and_then(|s| s.parse::<u64>().ok())
    };

    if let Some(val) = obj
        .get("collection")
        .or_else(|| obj.get("nft_contract"))
        .or_else(|| obj.get("nftContract"))
    {
        parsed.collection = parse_pubkey(val);
    }

    if let Some(val) = obj.get("token") {
        parsed.token = parse_pubkey(val);
        if parsed.token.is_none() {
            parsed.token_id = parse_u64(val);
        }
    }

    if let Some(val) = obj.get("token_id").or_else(|| obj.get("tokenId")) {
        parsed.token_id = parse_u64(val);
    }

    if let Some(val) = obj.get("price") {
        parsed.price = parse_u64(val);
    }

    if let Some(val) = obj.get("seller") {
        parsed.seller = parse_pubkey(val);
    }

    if let Some(val) = obj.get("buyer") {
        parsed.buyer = parse_pubkey(val);
    }

    parsed
}

#[allow(clippy::too_many_arguments)]
fn build_market_activity(
    kind: MarketActivityKind,
    function: String,
    program: Pubkey,
    caller: Pubkey,
    args: &[u8],
    slot: u64,
    timestamp: u64,
    tx_signature: Hash,
) -> MarketActivity {
    let parsed = parse_marketplace_args(args);

    let (seller, buyer) = match kind {
        MarketActivityKind::Listing | MarketActivityKind::Cancel => {
            (parsed.seller.or(Some(caller)), parsed.buyer)
        }
        MarketActivityKind::Sale => (parsed.seller, parsed.buyer.or(Some(caller))),
    };

    MarketActivity {
        slot,
        timestamp,
        kind,
        program,
        collection: parsed.collection,
        token: parsed.token,
        token_id: parsed.token_id,
        price: parsed.price,
        seller,
        buyer,
        function,
        tx_signature,
    }
}

fn emit_program_and_nft_events(
    state: &StateStore,
    ws_event_tx: &tokio::sync::broadcast::Sender<moltchain_rpc::ws::Event>,
    block: &Block,
) {
    record_block_activity(state, block);

    for tx in &block.transactions {
        // Emit Transaction event for every tx in the block
        let _ = ws_event_tx.send(moltchain_rpc::ws::Event::Transaction(tx.clone()));

        // Emit AccountChange events for all accounts touched by this tx
        let mut seen_accounts = std::collections::HashSet::new();
        for ix in &tx.message.instructions {
            for account_pubkey in &ix.accounts {
                if seen_accounts.insert(*account_pubkey) {
                    if let Ok(Some(acct)) = state.get_account(account_pubkey) {
                        let _ = ws_event_tx.send(moltchain_rpc::ws::Event::AccountChange {
                            pubkey: *account_pubkey,
                            balance: acct.shells,
                        });
                    }
                }
            }

            if ix.program_id == CORE_SYSTEM_PROGRAM_ID {
                match ix.data.first() {
                    Some(7) => {
                        if ix.accounts.len() < 4 {
                            continue;
                        }

                        let collection = ix.accounts[1];
                        let _ = ws_event_tx.send(moltchain_rpc::ws::Event::NftMint { collection });
                    }
                    Some(8) => {
                        if ix.accounts.len() < 3 {
                            continue;
                        }

                        let token = ix.accounts[1];

                        let token_account = match state.get_account(&token) {
                            Ok(Some(account)) => account,
                            _ => continue,
                        };

                        let token_state = match decode_token_state(&token_account.data) {
                            Ok(state) => state,
                            Err(_) => continue,
                        };

                        let _ = ws_event_tx.send(moltchain_rpc::ws::Event::NftTransfer {
                            collection: token_state.collection,
                        });
                    }
                    _ => {}
                }
            } else if ix.program_id == moltchain_core::CONTRACT_PROGRAM_ID {
                if let Ok(contract_ix) = ContractInstruction::deserialize(&ix.data) {
                    match contract_ix {
                        ContractInstruction::Deploy { .. } => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(moltchain_rpc::ws::Event::ProgramUpdate {
                                    program: *program,
                                    kind: "deploy".to_string(),
                                });
                            }
                        }
                        ContractInstruction::Upgrade { .. } => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(moltchain_rpc::ws::Event::ProgramUpdate {
                                    program: *program,
                                    kind: "upgrade".to_string(),
                                });
                            }
                        }
                        ContractInstruction::Close => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(moltchain_rpc::ws::Event::ProgramUpdate {
                                    program: *program,
                                    kind: "close".to_string(),
                                });
                            }
                        }
                        ContractInstruction::Call { function, args, .. } => {
                            if let Some(program) = ix.accounts.get(1) {
                                let _ = ws_event_tx.send(moltchain_rpc::ws::Event::ProgramCall {
                                    program: *program,
                                });

                                // Emit Log event for contract call
                                let _ = ws_event_tx.send(moltchain_rpc::ws::Event::Log {
                                    contract: *program,
                                    message: format!("call:{}", function),
                                });

                                // Emit contract events from DB if stored during processing
                                if let Ok(events) = state.get_contract_logs(program, 50) {
                                    for event in &events {
                                        if event.slot == block.header.slot {
                                            let _ =
                                                ws_event_tx.send(moltchain_rpc::ws::Event::Log {
                                                    contract: event.program,
                                                    message: format!(
                                                        "event:{}:{}",
                                                        event.name,
                                                        serde_json::to_string(&event.data)
                                                            .unwrap_or_default()
                                                    ),
                                                });
                                        }
                                    }
                                }

                                let kind = match function.as_str() {
                                    "list_nft" => Some(MarketActivityKind::Listing),
                                    "buy_nft" => Some(MarketActivityKind::Sale),
                                    _ => None,
                                };

                                if let (Some(kind), Some(caller)) =
                                    (kind, ix.accounts.first().copied())
                                {
                                    let activity = build_market_activity(
                                        kind.clone(),
                                        function.clone(),
                                        *program,
                                        caller,
                                        &args,
                                        block.header.slot,
                                        block.header.timestamp,
                                        tx.signature(),
                                    );

                                    let _ = match kind {
                                        MarketActivityKind::Listing => ws_event_tx.send(
                                            moltchain_rpc::ws::Event::MarketListing { activity },
                                        ),
                                        MarketActivityKind::Sale => {
                                            ws_event_tx.send(moltchain_rpc::ws::Event::MarketSale {
                                                activity,
                                            })
                                        }
                                        MarketActivityKind::Cancel => Ok(0),
                                    };
                                }

                                // Emit bridge events for lock/mint calls
                                match function.as_str() {
                                    "lock" | "bridge_lock" => {
                                        let sender = ix
                                            .accounts
                                            .first()
                                            .map(|p| p.to_base58())
                                            .unwrap_or_default();
                                        let recipient = ix
                                            .accounts
                                            .get(2)
                                            .copied()
                                            .unwrap_or(moltchain_core::Pubkey([0; 32]));
                                        // Parse args from JSON bytes
                                        let parsed =
                                            serde_json::from_slice::<serde_json::Value>(&args)
                                                .unwrap_or_default();
                                        let amount = parsed
                                            .get("amount")
                                            .and_then(|v| {
                                                v.as_u64().or_else(|| {
                                                    v.as_str().and_then(|s| s.parse().ok())
                                                })
                                            })
                                            .unwrap_or(0);
                                        let dest_chain = parsed
                                            .get("dest_chain")
                                            .or_else(|| parsed.get("chain"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown")
                                            .to_string();
                                        let asset = parsed
                                            .get("asset")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("molt")
                                            .to_string();
                                        let _ = ws_event_tx.send(
                                            moltchain_rpc::ws::Event::BridgeLock {
                                                chain: dest_chain,
                                                asset,
                                                amount,
                                                sender,
                                                recipient,
                                            },
                                        );
                                    }
                                    "mint" | "bridge_mint" => {
                                        let recipient = ix
                                            .accounts
                                            .get(1)
                                            .copied()
                                            .unwrap_or(moltchain_core::Pubkey([0; 32]));
                                        // Parse args from JSON bytes
                                        let parsed =
                                            serde_json::from_slice::<serde_json::Value>(&args)
                                                .unwrap_or_default();
                                        let amount = parsed
                                            .get("amount")
                                            .and_then(|v| {
                                                v.as_u64().or_else(|| {
                                                    v.as_str().and_then(|s| s.parse().ok())
                                                })
                                            })
                                            .unwrap_or(0);
                                        let source_chain = parsed
                                            .get("source_chain")
                                            .or_else(|| parsed.get("chain"))
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown")
                                            .to_string();
                                        let asset = parsed
                                            .get("asset")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("musd")
                                            .to_string();
                                        let tx_hash = hex::encode(tx.signature().0);
                                        let _ = ws_event_tx.send(
                                            moltchain_rpc::ws::Event::BridgeMint {
                                                chain: source_chain,
                                                asset,
                                                amount,
                                                recipient,
                                                tx_hash,
                                            },
                                        );
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Default)]
struct SnapshotSync {
    validator_set: bool,
    stake_pool: bool,
}

impl SnapshotSync {
    fn new(is_joining_network: bool) -> Self {
        if is_joining_network {
            Self::default()
        } else {
            Self {
                validator_set: true,
                stake_pool: true,
            }
        }
    }

    fn is_ready(&self) -> bool {
        self.validator_set && self.stake_pool
    }
}

fn block_vote_weight(
    slot: u64,
    block_hash: &Hash,
    vote_agg: &VoteAggregator,
    validator_set: &ValidatorSet,
    stake_pool: &StakePool,
) -> u64 {
    if let Some(votes) = vote_agg.get_votes(slot, block_hash) {
        let total_stake = stake_pool.total_stake();
        if total_stake == 0 {
            return votes
                .iter()
                .filter_map(|vote| validator_set.get_validator(&vote.validator))
                .map(|v| v.voting_weight())
                .sum();
        }

        return votes
            .iter()
            .filter_map(|vote| stake_pool.get_stake(&vote.validator))
            .map(|stake_info| stake_info.total_stake())
            .sum();
    }

    0
}

/// Replay transactions from a received P2P block to update local state.
/// The producing validator already executed these transactions; receivers
/// must replay them so that fee charges and balance mutations are applied
/// identically, preventing state divergence across the network.
/// Genesis-block transactions (slot 0) are created with special
/// signatures and a zero blockhash, so they cannot pass the normal
/// validation pipeline — the genesis state was applied directly.
fn replay_block_transactions(processor: &TxProcessor, block: &Block) {
    if block.header.slot == 0 {
        return; // genesis txs use zero blockhash + dummy signatures
    }
    let producer_pubkey = Pubkey(block.header.validator);
    for tx in &block.transactions {
        let result = processor.process_transaction(tx, &producer_pubkey);
        if !result.success {
            warn!(
                "⚠️  Tx replay failed in slot {}: {} ({})",
                block.header.slot,
                tx.signature().to_hex(),
                result.error.unwrap_or_default()
            );
        }
    }
}

/// Reverse the financial effects of a replaced block during fork choice.
/// Attempts to debit the old producer's reward and credit treasury back.
/// Fee distribution reversal is approximate — voter shares remain (small
/// amounts relative to block reward). This prevents the worst case of the
/// wrong producer keeping an entire block reward.
fn revert_block_effects(state: &StateStore, old_block: &Block) {
    let old_producer = Pubkey(old_block.header.validator);
    let slot = old_block.header.slot;
    let is_heartbeat = !block_has_user_transactions(old_block);

    let reward = if is_heartbeat {
        HEARTBEAT_BLOCK_REWARD
    } else {
        TRANSACTION_BLOCK_REWARD
    };

    // Reverse block reward: debit old producer, credit treasury
    if let Ok(Some(treasury_pubkey)) = state.get_treasury_pubkey() {
        if let Ok(Some(mut producer_account)) = state.get_account(&old_producer) {
            let debit = reward.min(producer_account.spendable);
            if debit > 0 {
                producer_account.shells = producer_account.shells.saturating_sub(debit);
                producer_account.spendable = producer_account.spendable.saturating_sub(debit);
                if let Err(e) = state.put_account(&old_producer, &producer_account) {
                    warn!("revert_block_effects: failed to debit producer: {}", e);
                }
            }

            if let Ok(Some(mut treasury_account)) = state.get_account(&treasury_pubkey) {
                treasury_account.shells = treasury_account.shells.saturating_add(debit);
                treasury_account.spendable = treasury_account.spendable.saturating_add(debit);
                if let Err(e) = state.put_account(&treasury_pubkey, &treasury_account) {
                    warn!("revert_block_effects: failed to credit treasury: {}", e);
                }
            }
        }
    }

    // Reverse fee distribution: debit old producer's fee share, credit treasury.
    // Voter fee shares are NOT reversed (small amounts, no stored voter list).
    let fee_config = state
        .get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());
    let total_fee: u64 = old_block
        .transactions
        .iter()
        .map(|tx| TxProcessor::compute_transaction_fee(tx, &fee_config))
        .sum();

    if total_fee > 0 {
        let producer_share = total_fee * fee_config.fee_producer_percent / 100;
        if producer_share > 0 {
            if let Ok(Some(treasury_pubkey)) = state.get_treasury_pubkey() {
                if let Ok(Some(mut producer_account)) = state.get_account(&old_producer) {
                    let debit = producer_share.min(producer_account.spendable);
                    producer_account.shells = producer_account.shells.saturating_sub(debit);
                    producer_account.spendable = producer_account.spendable.saturating_sub(debit);
                    if let Err(e) = state.put_account(&old_producer, &producer_account) {
                        warn!("revert_block_effects: failed to debit producer fees: {}", e);
                    }

                    if let Ok(Some(mut treasury_account)) = state.get_account(&treasury_pubkey) {
                        treasury_account.shells = treasury_account.shells.saturating_add(debit);
                        treasury_account.spendable =
                            treasury_account.spendable.saturating_add(debit);
                        if let Err(e) = state.put_account(&treasury_pubkey, &treasury_account) {
                            warn!(
                                "revert_block_effects: failed to credit treasury fees: {}",
                                e
                            );
                        }
                    }
                }
            }
        }
    }

    // Clear distribution hashes so apply_block_effects can run for the new block
    if let Err(e) = state.clear_reward_distribution_hash(slot) {
        warn!(
            "revert_block_effects: failed to clear reward hash for slot {}: {}",
            slot, e
        );
    }
    if let Err(e) = state.clear_fee_distribution_hash(slot) {
        warn!(
            "revert_block_effects: failed to clear fee hash for slot {}: {}",
            slot, e
        );
    }

    info!(
        "⚖️  Reverted block effects for slot {} (old producer: {})",
        slot,
        old_producer.to_base58()
    );
}

/// C7 fix: Reverse user transaction effects of a replaced block during fork choice.
/// For each transaction: reverse transfer instructions, refund fees, remove tx record
/// so the new block's transactions can be properly replayed.
fn revert_block_transactions(state: &StateStore, old_block: &Block) {
    use moltchain_core::SYSTEM_PROGRAM_ID;

    if old_block.header.slot == 0 {
        return;
    }

    let fee_config = state
        .get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());

    for tx in old_block.transactions.iter().rev() {
        // AUDIT-FIX 0.5: Detect non-system-transfer instructions that can't be reverted
        let has_non_revertible = tx.message.instructions.iter().any(|ix| {
            if ix.program_id != SYSTEM_PROGRAM_ID {
                return true; // Contract call — can't revert
            }
            if ix.data.is_empty() {
                return false;
            }
            // Only types 0,2,3,4,5 (transfers) are revertible
            !matches!(ix.data[0], 0 | 2 | 3 | 4 | 5)
        });
        if has_non_revertible {
            error!(
                "⚠️ CRITICAL: Block {} contains non-revertible instructions (contract calls, \
                 NFT ops, staking, etc.). Fork switch may leave inconsistent state. \
                 Tx hash: {}",
                old_block.header.slot,
                tx.hash().to_hex()
            );
            // Still revert what we can (transfers + fees) — this is best-effort.
            // TODO: Implement full state snapshots for safe fork switches.
        }

        // 1. Reverse each system transfer instruction
        for ix in &tx.message.instructions {
            if ix.program_id == SYSTEM_PROGRAM_ID && !ix.data.is_empty() {
                let ix_type = ix.data[0];
                // Types 0,2,3,4,5 are all transfers
                if matches!(ix_type, 0 | 2 | 3 | 4 | 5)
                    && ix.accounts.len() >= 2
                    && ix.data.len() >= 9
                {
                    let from = ix.accounts[0]; // original sender
                    let to = ix.accounts[1]; // original receiver
                    let amount_bytes: [u8; 8] = match ix.data[1..9].try_into() {
                        Ok(b) => b,
                        Err(_) => continue,
                    };
                    let amount = u64::from_le_bytes(amount_bytes);

                    // Reverse: credit sender, debit receiver
                    if amount > 0 {
                        if let Ok(Some(mut receiver)) = state.get_account(&to) {
                            let debit = amount.min(receiver.spendable);
                            receiver.shells = receiver.shells.saturating_sub(debit);
                            receiver.spendable = receiver.spendable.saturating_sub(debit);
                            state.put_account(&to, &receiver).ok();

                            if let Ok(Some(mut sender)) = state.get_account(&from) {
                                sender.shells = sender.shells.saturating_add(debit);
                                sender.spendable = sender.spendable.saturating_add(debit);
                                state.put_account(&from, &sender).ok();
                            }
                        }
                    }
                }
            }
        }

        // 2. Refund fee to fee payer
        if let Some(first_ix) = tx.message.instructions.first() {
            if let Some(&fee_payer) = first_ix.accounts.first() {
                let fee = TxProcessor::compute_transaction_fee(tx, &fee_config);
                if fee > 0 {
                    if let Ok(Some(mut payer_account)) = state.get_account(&fee_payer) {
                        payer_account.shells = payer_account.shells.saturating_add(fee);
                        payer_account.spendable = payer_account.spendable.saturating_add(fee);
                        state.put_account(&fee_payer, &payer_account).ok();
                    }
                }
            }
        }

        // 3. Remove transaction record so new block's txs can be replayed
        let tx_hash = tx.hash();
        state.delete_transaction(&tx_hash).ok();
    }

    info!(
        "⚖️  Reverted {} user transactions for slot {}",
        old_block.transactions.len(),
        old_block.header.slot
    );
}

async fn apply_block_effects(
    state: &StateStore,
    validator_set: &Arc<Mutex<ValidatorSet>>,
    stake_pool: &Arc<Mutex<StakePool>>,
    vote_aggregator: &Arc<Mutex<VoteAggregator>>,
    block: &Block,
    skip_rewards: bool,
) {
    if block.header.slot == 0 || block.header.validator == [0u8; 32] {
        return;
    }

    let producer = Pubkey(block.header.validator);
    let slot = block.header.slot;
    let has_user_transactions = block_has_user_transactions(block);
    let is_heartbeat = !has_user_transactions;

    let stake_amount = {
        let pool = stake_pool.lock().await;
        pool.get_stake(&producer)
            .map(|stake_info| stake_info.total_stake())
            .unwrap_or(0)
    };

    {
        let mut vs = validator_set.lock().await;
        if let Some(val_info) = vs.get_validator_mut(&producer) {
            val_info.blocks_proposed += 1;
            val_info.last_active_slot = slot;
            val_info.update_reputation(true);
        } else {
            // H13 fix: require minimum stake before accepting new validator
            if stake_amount < MIN_VALIDATOR_STAKE {
                warn!(
                    "⚠️  Ignoring unregistered block producer {} with insufficient stake ({} < {})",
                    producer.to_base58(),
                    stake_amount,
                    MIN_VALIDATOR_STAKE
                );
            } else {
                let new_validator = ValidatorInfo {
                    pubkey: producer,
                    stake: stake_amount,
                    reputation: 100,
                    blocks_proposed: 1,
                    votes_cast: 0,
                    correct_votes: 0,
                    joined_slot: slot,
                    last_active_slot: slot,
                };
                vs.add_validator(new_validator);
            }
        }

        if let Err(e) = state.save_validator_set(&vs) {
            warn!("⚠️  Failed to persist validator set update: {}", e);
        }
    }

    // ── Protocol-level block reward (coinbase) ──────────────────────────
    // This is a consensus rule, not a transaction. Every validator
    // deterministically applies the same reward when processing any block.
    // No treasury private key needed — the protocol itself authorizes it.
    let block_hash = block.hash();
    if !skip_rewards {
        let reward_already = match state.get_reward_distribution_hash(slot) {
            Ok(Some(_)) => true, // per-slot guard: any reward for this slot = skip
            Ok(None) => false,
            Err(e) => {
                warn!("⚠️  Failed to read reward distribution hash: {}", e);
                false
            }
        };

        if !reward_already {
            let reward_total = if is_heartbeat {
                HEARTBEAT_BLOCK_REWARD
            } else {
                TRANSACTION_BLOCK_REWARD
            };

            // 1. Check treasury can afford the reward BEFORE updating StakePool
            let treasury_pubkey = state.get_treasury_pubkey().ok().flatten();
            let can_afford = if let Some(ref tpk) = treasury_pubkey {
                state
                    .get_account(tpk)
                    .ok()
                    .flatten()
                    .map(|a| a.shells >= reward_total)
                    .unwrap_or(false)
            } else {
                false
            };

            if !can_afford {
                if let Some(ref tpk) = treasury_pubkey {
                    let bal = state
                        .get_account(tpk)
                        .ok()
                        .flatten()
                        .map(|a| a.shells)
                        .unwrap_or(0);
                    warn!(
                        "⚠️  Treasury balance {} < reward {}, skipping protocol reward",
                        bal, reward_total
                    );
                }
            } else {
                // 2. Update StakePool (tracks rewards, vesting, bootstrap debt)
                let (liquid, debt_payment, reward) = {
                    let mut pool = stake_pool.lock().await;
                    let is_active = pool
                        .get_stake(&producer)
                        .map(|info| info.is_active)
                        .unwrap_or(false);
                    if !is_active {
                        (0u64, 0u64, 0u64)
                    } else {
                        let reward = pool.distribute_block_reward(&producer, slot, is_heartbeat);
                        pool.record_block_produced(&producer);
                        let (liquid, debt_payment) = pool.claim_rewards(&producer);
                        if let Err(e) = state.put_stake_pool(&pool) {
                            warn!("⚠️  Failed to persist stake pool reward update: {}", e);
                        }
                        (liquid, debt_payment, reward)
                    }
                };

                // 3. Protocol-level balance transfer: treasury → producer
                if reward > 0 {
                    if let Some(ref treasury_pubkey) = treasury_pubkey {
                        let mut treasury_account = state
                            .get_account(treasury_pubkey)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));

                        // Debit treasury: only the liquid portion leaves treasury
                        // Debt repayment is internal bookkeeping (reclassifies existing stake)
                        // H12 fix: when liquid==0, no treasury debit or producer credit needed
                        let debit_amount = liquid;
                        treasury_account.shells =
                            treasury_account.shells.saturating_sub(debit_amount);
                        treasury_account.spendable =
                            treasury_account.spendable.saturating_sub(debit_amount);
                        if let Err(e) = state.put_account(treasury_pubkey, &treasury_account) {
                            warn!("⚠️  Failed to debit treasury for block reward: {}", e);
                        }

                        // Credit producer: only liquid portion to spendable
                        // During vesting: 50% liquid to spendable, 50% debt repayment (no new coins)
                        // Fully vested: 100% liquid
                        // H12 fix: when liquid==0, credit nothing (was falling through to reward_total)
                        let credit_amount = liquid;
                        let mut producer_account = state
                            .get_account(&producer)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));
                        producer_account.add_spendable(credit_amount).unwrap_or_else(|e| {
                            warn!("\u{26a0}\u{fe0f}  Overflow crediting producer block reward: {}", e);
                        });
                        if let Err(e) = state.put_account(&producer, &producer_account) {
                            warn!("⚠️  Failed to credit producer block reward: {}", e);
                        }
                    }

                    let reward_type = if is_heartbeat {
                        "heartbeat"
                    } else {
                        "transaction"
                    };
                    info!(
                        "💰 Block reward: {:.3} MOLT ({}) | liquid {:.3}, debt {:.3}",
                        reward as f64 / 1_000_000_000.0,
                        reward_type,
                        liquid as f64 / 1_000_000_000.0,
                        debt_payment as f64 / 1_000_000_000.0,
                    );
                }
            }

            if let Err(e) = state.set_reward_distribution_hash(slot, &block_hash) {
                warn!(
                    "⚠️  Failed to record reward distribution for slot {}: {}",
                    slot, e
                );
            }
        }
    }

    let fee_config = state
        .get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());
    let total_fee: u64 = block
        .transactions
        .iter()
        .map(|tx| TxProcessor::compute_transaction_fee(tx, &fee_config))
        .sum();

    if total_fee == 0 {
        return;
    }

    if let Ok(Some(existing)) = state.get_fee_distribution_hash(slot) {
        if existing == block_hash {
            return;
        }
        warn!(
            "⚠️  Fee distribution already recorded for slot {} with different hash",
            slot
        );
        return;
    }

    let treasury_pubkey = match state.get_treasury_pubkey() {
        Ok(Some(pubkey)) => pubkey,
        _ => {
            warn!("⚠️  Treasury pubkey missing; skipping fee distribution");
            return;
        }
    };

    let mut treasury_account = match state.get_account(&treasury_pubkey) {
        Ok(Some(account)) => account,
        _ => Account::new(0, treasury_pubkey),
    };

    if treasury_account.shells < total_fee {
        warn!(
            "⚠️  Treasury balance {} < total fees {}, skipping distribution",
            treasury_account.shells, total_fee
        );
        return;
    }

    let burn = total_fee * fee_config.fee_burn_percent / 100;
    let producer_share = total_fee * fee_config.fee_producer_percent / 100;
    let voters_share = total_fee * fee_config.fee_voters_percent / 100;
    let mut voters_paid: u64 = 0;

    // NOTE: burn was already applied in charge_fee (processor.rs) during
    // transaction processing.  Do NOT call add_burned again here — that
    // caused a double-burn destroying twice the intended supply.

    // AUDIT-FIX 0.6: All fee distribution writes go through an atomic
    // WriteBatch. Nothing hits disk until commit_batch() succeeds, so a
    // crash mid-distribution cannot leave state half-credited.
    let mut batch = state.begin_batch();

    if producer_share > 0 {
        let mut producer_account = match state.get_account(&producer) {
            Ok(Some(account)) => account,
            _ => Account::new(0, SYSTEM_ACCOUNT_OWNER),
        };
        producer_account
            .add_spendable(producer_share)
            .unwrap_or_else(|e| {
                warn!("\u{26a0}\u{fe0f}  Overflow crediting producer fees: {}", e);
            });
        if let Err(e) = batch.put_account(&producer, &producer_account) {
            warn!(
                "⚠️  Failed to credit producer fees for {}: {}",
                producer.to_base58(),
                e
            );
        }
    }

    if voters_share > 0 {
        let voters = {
            let agg = vote_aggregator.lock().await;
            match agg.get_votes(slot, &block_hash) {
                Some(votes) => votes.clone(),
                None => Vec::new(),
            }
        };

        let mut voter_pubkeys: Vec<Pubkey> = voters
            .iter()
            .map(|vote| vote.validator)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        // Deterministic ordering is consensus-critical: the last voter
        // receives the integer-rounding remainder, so all validators
        // must iterate in the same order.
        voter_pubkeys.sort_by_key(|pk| pk.0);

        if !voter_pubkeys.is_empty() {
            let pool = stake_pool.lock().await;
            let total_voter_stake: u64 = voter_pubkeys
                .iter()
                .filter_map(|validator| pool.get_stake(validator))
                .map(|stake_info| stake_info.total_stake())
                .sum();

            let mut remaining = voters_share;
            for (idx, validator) in voter_pubkeys.iter().enumerate() {
                let share = if total_voter_stake > 0 {
                    let stake = pool
                        .get_stake(validator)
                        .map(|stake_info| stake_info.total_stake())
                        .unwrap_or(0);
                    (voters_share * stake / total_voter_stake).min(remaining)
                } else {
                    let remaining_validators = (voter_pubkeys.len() - idx) as u64;
                    (remaining / remaining_validators).min(remaining)
                };

                if share == 0 {
                    continue;
                }

                let mut voter_account = match batch.get_account(validator) {
                    Ok(Some(account)) => account,
                    _ => match state.get_account(validator) {
                        Ok(Some(account)) => account,
                        _ => Account::new(0, SYSTEM_ACCOUNT_OWNER),
                    },
                };
                voter_account.add_spendable(share).unwrap_or_else(|e| {
                    warn!("\u{26a0}\u{fe0f}  Overflow crediting voter fees: {}", e);
                });
                if let Err(e) = batch.put_account(validator, &voter_account) {
                    warn!(
                        "⚠️  Failed to credit voter fees for {}: {}",
                        validator.to_base58(),
                        e
                    );
                }
                remaining = remaining.saturating_sub(share);
                voters_paid = voters_paid.saturating_add(share);
            }
            drop(pool);
        }
    }

    let treasury_share = total_fee.saturating_sub(burn + producer_share + voters_paid);

    // charge_fee credited treasury with (fee − burn) for each tx.
    // We only debit what we're distributing out: producer_share + voters_paid.
    // Treasury retains its own share (≈10%) automatically.
    treasury_account.shells = treasury_account
        .shells
        .saturating_sub(producer_share + voters_paid);
    treasury_account.spendable = treasury_account
        .spendable
        .saturating_sub(producer_share + voters_paid);
    if let Err(e) = batch.put_account(&treasury_pubkey, &treasury_account) {
        warn!("⚠️  Failed to update treasury account: {}", e);
        return;
    }

    if let Err(e) = batch.set_fee_distribution_hash(slot, &block_hash) {
        warn!(
            "⚠️  Failed to record fee distribution hash in batch for slot {}: {}",
            slot, e
        );
        return;
    }

    // Commit all fee distribution writes atomically
    if let Err(e) = state.commit_batch(batch) {
        warn!(
            "⚠️  CRITICAL: Failed to commit fee distribution batch for slot {}: {}",
            slot, e
        );
        return;
    }

    if treasury_share > 0 {
        info!(
            "🏦 Treasury fees retained: {:.6} MOLT",
            treasury_share as f64 / 1_000_000_000.0
        );
    }

    // record_block_activity is called in emit_program_and_nft_events, not here
}

// ========================================================================
// FIRST-BOOT CONTRACT AUTO-DEPLOY
// ========================================================================
// Deploys all compiled WASM contracts from the contracts/ directory into
// the chain state immediately after genesis. This ensures the blockchain
// is fully operational from the first block — no external deploy scripts
// needed. Contract addresses are derived deterministically from
// SHA-256(deployer_pubkey + wasm_bytes).
// ========================================================================

/// Contract catalog: (directory_name, symbol, display_name, template)
const GENESIS_CONTRACT_CATALOG: &[(&str, &str, &str, &str)] = &[
    // Core token
    ("moltcoin", "MOLT", "MoltCoin", "token"),
    // Wrapped tokens
    ("musd_token", "MUSD", "Wrapped USD", "wrapped"),
    ("wsol_token", "WSOL", "Wrapped SOL", "wrapped"),
    ("weth_token", "WETH", "Wrapped ETH", "wrapped"),
    // DEX
    ("dex_core", "DEX", "MoltChain DEX Core", "dex"),
    ("dex_amm", "DEXAMM", "DEX AMM Engine", "dex"),
    ("dex_router", "DEXROUTER", "DEX Smart Router", "dex"),
    ("dex_margin", "DEXMARGIN", "DEX Margin Trading", "dex"),
    ("dex_rewards", "DEXREWARDS", "DEX Reward Distributor", "dex"),
    ("dex_governance", "DEXGOV", "DEX Governance", "dex"),
    ("dex_analytics", "ANALYTICS", "DEX Analytics", "dex"),
    // DeFi
    ("moltswap", "MOLTSWAP", "MoltSwap AMM", "defi"),
    ("moltbridge", "BRIDGE", "MoltBridge", "bridge"),
    ("moltmarket", "MARKET", "MoltMarket", "marketplace"),
    ("moltoracle", "ORACLE", "MoltOracle", "oracle"),
    ("moltauction", "AUCTION", "MoltAuction", "auction"),
    ("moltdao", "DAO", "MoltDAO Governance", "governance"),
    ("lobsterlend", "LEND", "LobsterLend", "lending"),
    // NFT / Identity
    ("moltpunks", "PUNKS", "MoltPunks NFT", "nft"),
    ("moltyid", "YID", "MoltyID Identity", "identity"),
    // Infrastructure
    ("clawpay", "CLAWPAY", "ClawPay Payments", "payments"),
    ("clawpump", "CLAWPUMP", "ClawPump Launchpad", "launchpad"),
    ("clawvault", "CLAWVAULT", "ClawVault", "vault"),
    ("bountyboard", "BOUNTY", "BountyBoard", "bounty"),
    ("compute_market", "COMPUTE", "Compute Market", "compute"),
    ("reef_storage", "REEF", "Reef Storage", "storage"),
];

fn genesis_auto_deploy(state: &StateStore, deployer_pubkey: &Pubkey) {
    info!("──────────────────────────────────────────────────────");
    info!("  FIRST-BOOT: Auto-deploying genesis contracts");
    info!("──────────────────────────────────────────────────────");

    let contracts_dir = PathBuf::from("contracts");
    if !contracts_dir.exists() {
        warn!("contracts/ directory not found — skipping auto-deploy");
        return;
    }

    let mut deployed: usize = 0;
    let mut failed: usize = 0;

    for &(dir_name, symbol, display_name, template) in GENESIS_CONTRACT_CATALOG {
        let wasm_path = contracts_dir
            .join(dir_name)
            .join(format!("{}.wasm", dir_name));
        if !wasm_path.exists() {
            warn!(
                "  SKIP {}: WASM not found at {}",
                symbol,
                wasm_path.display()
            );
            failed += 1;
            continue;
        }

        let wasm_bytes = match fs::read(&wasm_path) {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("  FAIL {}: Cannot read WASM: {}", symbol, e);
                failed += 1;
                continue;
            }
        };

        // Derive deterministic program address: SHA-256(deployer + name + wasm)
        // Including the name ensures identical WASMs (e.g. wrapped token stubs)
        // get unique addresses.
        let mut hasher = Sha256::new();
        hasher.update(deployer_pubkey.0);
        hasher.update(dir_name.as_bytes());
        hasher.update(&wasm_bytes);
        let hash_result = hasher.finalize();
        let mut addr_bytes = [0u8; 32];
        addr_bytes.copy_from_slice(&hash_result[..32]);
        let program_pubkey = Pubkey(addr_bytes);

        // Check if already deployed (idempotent)
        if let Ok(Some(_)) = state.get_account(&program_pubkey) {
            info!(
                "  SKIP {}: already deployed at {}",
                symbol,
                program_pubkey.to_base58()
            );
            continue;
        }

        // Create ContractAccount
        let contract = ContractAccount::new(wasm_bytes, *deployer_pubkey);

        // Create executable Account with contract data
        let mut account = Account::new(0, program_pubkey);
        match serde_json::to_vec(&contract) {
            Ok(data) => account.data = data,
            Err(e) => {
                error!("  FAIL {}: Serialize error: {}", symbol, e);
                failed += 1;
                continue;
            }
        }
        account.executable = true;

        // Store the account
        if let Err(e) = state.put_account(&program_pubkey, &account) {
            error!("  FAIL {}: put_account error: {}", symbol, e);
            failed += 1;
            continue;
        }

        // Index in CF_PROGRAMS (makes it visible to getAllContracts)
        if let Err(e) = state.index_program(&program_pubkey) {
            warn!("  WARN {}: index_program error: {}", symbol, e);
        }

        // Register in symbol registry with rich token metadata
        let mut meta = serde_json::json!({
            "genesis_deploy": true,
            "wasm_size": account.data.len(),
        });
        // Enrich token/wrapped contracts with MT-20 metadata
        match template {
            "token" => {
                // MOLT native token: 1B fixed supply, 9 decimals, NOT mintable (deflationary via 50% fee burn)
                meta["total_supply"] = serde_json::json!(1_000_000_000_u64 * 1_000_000_000_u64);
                meta["decimals"] = serde_json::json!(9);
                meta["mintable"] = serde_json::json!(false);
                meta["burnable"] = serde_json::json!(true);
                meta["is_native"] = serde_json::json!(true);
            }
            "wrapped" => {
                // Wrapped tokens start at 0 supply, 9 decimals
                meta["total_supply"] = serde_json::json!(0);
                meta["decimals"] = serde_json::json!(9);
                meta["mintable"] = serde_json::json!(true);
                meta["burnable"] = serde_json::json!(true);
            }
            _ => {}
        }
        let entry = SymbolRegistryEntry {
            symbol: symbol.to_string(),
            program: program_pubkey,
            owner: *deployer_pubkey,
            name: Some(display_name.to_string()),
            template: Some(template.to_string()),
            metadata: Some(meta),
        };
        if let Err(e) = state.register_symbol(symbol, entry) {
            warn!("  WARN {}: register_symbol error: {}", symbol, e);
        }

        info!(
            "  OK   {} ({}) -> {}",
            symbol,
            display_name,
            program_pubkey.to_base58()
        );
        deployed += 1;
    }

    info!("──────────────────────────────────────────────────────");
    info!(
        "  Genesis deploy complete: {} deployed, {} failed",
        deployed, failed
    );
    info!("──────────────────────────────────────────────────────");
}

// ========================================================================
//  GENESIS PHASE 2 — Initialize all 26 contracts by executing their
//  initialize() function via the WASM runtime.
// ========================================================================

/// Derive a contract's deterministic address from deployer + dir_name + wasm.
/// Must match the derivation in genesis_auto_deploy().
fn derive_contract_address(deployer_pubkey: &Pubkey, dir_name: &str) -> Option<Pubkey> {
    let contracts_dir = PathBuf::from("contracts");
    let wasm_path = contracts_dir
        .join(dir_name)
        .join(format!("{}.wasm", dir_name));
    let wasm_bytes = fs::read(&wasm_path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(deployer_pubkey.0);
    hasher.update(dir_name.as_bytes());
    hasher.update(&wasm_bytes);
    let hash_result = hasher.finalize();
    let mut addr_bytes = [0u8; 32];
    addr_bytes.copy_from_slice(&hash_result[..32]);
    Some(Pubkey(addr_bytes))
}

/// Execute a contract function via WASM runtime and apply storage changes.
/// Returns true on success.
fn genesis_exec_contract(
    state: &StateStore,
    program_pubkey: &Pubkey,
    deployer_pubkey: &Pubkey,
    function_name: &str,
    args: &[u8],
    label: &str,
) -> bool {
    let account = match state.get_account(program_pubkey) {
        Ok(Some(a)) => a,
        _ => {
            error!("  FAIL {}: account not found", label);
            return false;
        }
    };

    let mut contract: ContractAccount = match serde_json::from_slice(&account.data) {
        Ok(c) => c,
        Err(e) => {
            error!("  FAIL {}: deserialize ContractAccount: {}", label, e);
            return false;
        }
    };

    let ctx = ContractContext::with_args(
        *deployer_pubkey,
        *program_pubkey,
        0,
        0,
        contract.storage.clone(),
        args.to_vec(),
    );

    let mut runtime = ContractRuntime::new();
    match runtime.execute(&contract, function_name, args, ctx) {
        Ok(result) => {
            if !result.success {
                warn!(
                    "  WARN {}: contract returned error: {:?}",
                    label, result.error
                );
                // Some contracts return non-zero on "already initialized" — not fatal
            }
            // Apply storage changes
            for (key, val_opt) in &result.storage_changes {
                match val_opt {
                    Some(val) => contract.set_storage(key.clone(), val.clone()),
                    None => {
                        contract.remove_storage(key);
                    }
                }
            }
            // Re-serialize and store
            let mut updated_account = account;
            match serde_json::to_vec(&contract) {
                Ok(data) => updated_account.data = data,
                Err(e) => {
                    error!("  FAIL {}: re-serialize: {}", label, e);
                    return false;
                }
            }
            if let Err(e) = state.put_account(program_pubkey, &updated_account) {
                error!("  FAIL {}: put_account: {}", label, e);
                return false;
            }
            true
        }
        Err(e) => {
            error!("  FAIL {}: WASM execution error: {}", label, e);
            false
        }
    }
}

fn genesis_initialize_contracts(state: &StateStore, deployer_pubkey: &Pubkey) {
    info!("──────────────────────────────────────────────────────");
    info!("  GENESIS PHASE 2: Initializing all contracts");
    info!("──────────────────────────────────────────────────────");

    let admin = deployer_pubkey.0;
    let mut initialized: usize = 0;
    let mut skipped: usize = 0;

    // Build a lookup: dir_name -> Pubkey for cross-references
    let mut address_map: HashMap<String, Pubkey> = HashMap::new();
    for &(dir_name, _symbol, _display, _template) in GENESIS_CONTRACT_CATALOG {
        if let Some(addr) = derive_contract_address(deployer_pubkey, dir_name) {
            address_map.insert(dir_name.to_string(), addr);
        }
    }

    // ── Initialization in dependency order ──
    // Layer 0: Tokens (no dependencies)
    // Layer 1: Identity
    // Layer 2: DEX core (opcode dispatch)
    // Layer 3: DEX infrastructure (opcode dispatch)
    // Layer 4: DeFi protocols
    // Layer 5: Applications

    // Define initialization config for each contract:
    // (dir_name, function_name, args_builder)
    // For opcode-dispatch contracts: function="call", args=[0x00][admin 32B]
    // For named-export contracts: function="initialize" (or variant), args=[admin 32B]

    struct InitSpec {
        dir_name: &'static str,
        function: &'static str,
        /// Build arguments. We pass in admin pubkey and address map.
        args: Vec<u8>,
    }

    // Helper: build opcode-dispatch init args [0x00][admin 32B]
    fn opcode_init_args(admin: &[u8; 32]) -> Vec<u8> {
        let mut args = Vec::with_capacity(33);
        args.push(0x00); // opcode 0 = initialize
        args.extend_from_slice(admin);
        args
    }

    // Helper: build named-export init args = just [admin 32B]
    fn named_init_args(admin: &[u8; 32]) -> Vec<u8> {
        admin.to_vec()
    }

    // Resolve token contract addresses for moltswap and moltdao
    let molt_addr = address_map
        .get("moltcoin")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);
    let musd_addr = address_map
        .get("musd_token")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);

    // DAO: governance_token = MOLT address, treasury = deployer (initially),
    // min_proposal_threshold = 10,000 MOLT in shells (10_000 * 1e9)
    let dao_threshold: u64 = 10_000_000_000_000; // 10,000 MOLT
    let mut dao_args = Vec::with_capacity(72);
    dao_args.extend_from_slice(&molt_addr); // governance_token (32B)
    dao_args.extend_from_slice(&admin); // treasury (32B = deployer)
    dao_args.extend_from_slice(&dao_threshold.to_le_bytes()); // min_proposal_threshold (8B)

    // MoltSwap: token_a = MOLT, token_b = MUSD
    let mut moltswap_args = Vec::with_capacity(64);
    moltswap_args.extend_from_slice(&molt_addr);
    moltswap_args.extend_from_slice(&musd_addr);

    // MoltMarket: owner(32B) + fee_addr(32B) = deployer for both initially
    let mut moltmarket_args = Vec::with_capacity(64);
    moltmarket_args.extend_from_slice(&admin);
    moltmarket_args.extend_from_slice(&admin); // fee recipient = deployer initially

    // MoltAuction: initialize(marketplace_addr) + initialize_ma_admin(admin)
    // marketplace_addr = moltmarket address for integration
    let moltmarket_addr = address_map.get("moltmarket").map(|p| p.0).unwrap_or(admin);

    let specs: Vec<InitSpec> = vec![
        // ── Layer 0: Tokens ──
        InitSpec {
            dir_name: "moltcoin",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "musd_token",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "wsol_token",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "weth_token",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 1: Identity ──
        InitSpec {
            dir_name: "moltyid",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 2: DEX core (opcode dispatch) ──
        InitSpec {
            dir_name: "dex_core",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_amm",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_router",
            function: "call",
            args: opcode_init_args(&admin),
        },
        // ── Layer 3: DEX infrastructure (opcode dispatch) ──
        InitSpec {
            dir_name: "dex_margin",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_rewards",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_governance",
            function: "call",
            args: opcode_init_args(&admin),
        },
        InitSpec {
            dir_name: "dex_analytics",
            function: "call",
            args: opcode_init_args(&admin),
        },
        // ── Layer 4: DeFi protocols ──
        InitSpec {
            dir_name: "moltswap",
            function: "initialize",
            args: moltswap_args,
        },
        InitSpec {
            dir_name: "moltbridge",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "moltoracle",
            function: "initialize_oracle",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "lobsterlend",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 4b: Governance ──
        InitSpec {
            dir_name: "moltdao",
            function: "initialize_dao",
            args: dao_args,
        },
        // ── Layer 5: Marketplaces ──
        InitSpec {
            dir_name: "moltmarket",
            function: "initialize",
            args: moltmarket_args,
        },
        InitSpec {
            dir_name: "moltpunks",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // ── Layer 5b: Infrastructure ──
        InitSpec {
            dir_name: "clawpay",
            function: "initialize_cp_admin",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "clawpump",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "clawvault",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "compute_market",
            function: "initialize",
            args: named_init_args(&admin),
        },
        InitSpec {
            dir_name: "reef_storage",
            function: "initialize",
            args: named_init_args(&admin),
        },
        // bountyboard: no initialization needed (stateless bootstrap)
    ];

    for spec in &specs {
        let pubkey = match address_map.get(spec.dir_name) {
            Some(pk) => *pk,
            None => {
                warn!(
                    "  SKIP {}: address not derived (WASM missing?)",
                    spec.dir_name
                );
                skipped += 1;
                continue;
            }
        };

        if genesis_exec_contract(
            state,
            &pubkey,
            deployer_pubkey,
            spec.function,
            &spec.args,
            spec.dir_name,
        ) {
            info!("  INIT {}", spec.dir_name);
            initialized += 1;
        } else {
            skipped += 1;
        }
    }

    // MoltAuction requires TWO init calls:
    // 1. initialize(marketplace_addr) — sets escrow address
    // 2. initialize_ma_admin(admin) — sets admin
    if let Some(auction_pk) = address_map.get("moltauction") {
        let mkt_args = moltmarket_addr.to_vec();
        if genesis_exec_contract(
            state,
            auction_pk,
            deployer_pubkey,
            "initialize",
            &mkt_args,
            "moltauction(escrow)",
        ) {
            if genesis_exec_contract(
                state,
                auction_pk,
                deployer_pubkey,
                "initialize_ma_admin",
                admin.as_ref(),
                "moltauction(admin)",
            ) {
                info!("  INIT moltauction (escrow + admin)");
                initialized += 1;
            } else {
                skipped += 1;
            }
        } else {
            skipped += 1;
        }
    }

    info!("──────────────────────────────────────────────────────");
    info!(
        "  Genesis init complete: {} initialized, {} skipped",
        initialized, skipped
    );
    info!("──────────────────────────────────────────────────────");
}

// ========================================================================
//  GENESIS PHASE 3 — Create trading pairs and AMM pools at genesis.
//  Auto-lists MOLT/mUSD pair on dex_core and creates the corresponding
//  AMM pool on dex_amm.  WSOL/mUSD and WETH/mUSD are deferred until the
//  bridge & custody systems are live and tokens have real supply.
// ========================================================================

fn genesis_create_trading_pairs(state: &StateStore, deployer_pubkey: &Pubkey) {
    info!("──────────────────────────────────────────────────────");
    info!("  GENESIS PHASE 3: Creating trading pairs & AMM pools");
    info!("──────────────────────────────────────────────────────");

    let admin = deployer_pubkey.0;

    // Resolve contract addresses
    let dex_core_pk = match derive_contract_address(deployer_pubkey, "dex_core") {
        Some(pk) => pk,
        None => {
            error!("  FAIL: Cannot derive dex_core address");
            return;
        }
    };
    let dex_amm_pk = match derive_contract_address(deployer_pubkey, "dex_amm") {
        Some(pk) => pk,
        None => {
            error!("  FAIL: Cannot derive dex_amm address");
            return;
        }
    };

    // Resolve token addresses (only MOLT/mUSD at genesis)
    let molt_addr = derive_contract_address(deployer_pubkey, "moltcoin")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);
    let musd_addr = derive_contract_address(deployer_pubkey, "musd_token")
        .map(|p| p.0)
        .unwrap_or([0u8; 32]);

    // Genesis pair parameters (reasonable defaults for launch):
    // tick_size: 1 (minimum price increment in shells)
    // lot_size: 1_000_000 (minimum order lot = 0.001 tokens)
    // min_order: 1_000 (minimum order value in shells = MIN_ORDER_VALUE)
    let tick_size: u64 = 1;
    let lot_size: u64 = 1_000_000;
    let min_order: u64 = 1_000;

    // Only MOLT/mUSD at genesis. WSOL/mUSD and WETH/mUSD will be created
    // when the bridge + custody go live and wrapped tokens have real supply.
    let pairs: [(&str, [u8; 32], [u8; 32]); 1] = [
        ("MOLT/mUSD", molt_addr, musd_addr),
    ];

    let mut created_pairs: usize = 0;
    let mut created_pools: usize = 0;

    // Create CLOB trading pairs via dex_core opcode 1 (create_pair)
    // Args: [0x01][caller 32B][base 32B][quote 32B][tick_size 8B][lot_size 8B][min_order 8B]
    for (label, base, quote) in &pairs {
        let mut args = Vec::with_capacity(121);
        args.push(0x01); // opcode 1 = create_pair
        args.extend_from_slice(&admin); // caller
        args.extend_from_slice(base); // base_token
        args.extend_from_slice(quote); // quote_token
        args.extend_from_slice(&tick_size.to_le_bytes());
        args.extend_from_slice(&lot_size.to_le_bytes());
        args.extend_from_slice(&min_order.to_le_bytes());

        if genesis_exec_contract(
            state,
            &dex_core_pk,
            deployer_pubkey,
            "call",
            &args,
            &format!("dex_core.create_pair({})", label),
        ) {
            info!("  PAIR {}", label);
            created_pairs += 1;
        }
    }

    // Create AMM pools via dex_amm opcode 1 (create_pool)
    // Args: [0x01][caller 32B][token_a 32B][token_b 32B][fee_tier 1B][initial_sqrt_price 8B]
    // fee_tier = 2 (30bps), initial_sqrt_price = 1 << 32 (1.0 in Q32 fixed-point)
    let fee_tier: u8 = 2; // FEE_TIER_30BPS
    let initial_sqrt_price: u64 = 1u64 << 32; // 1.0 price

    for (label, base, quote) in &pairs {
        let mut args = Vec::with_capacity(106);
        args.push(0x01); // opcode 1 = create_pool
        args.extend_from_slice(&admin); // caller
        args.extend_from_slice(base); // token_a
        args.extend_from_slice(quote); // token_b
        args.push(fee_tier);
        args.extend_from_slice(&initial_sqrt_price.to_le_bytes());

        if genesis_exec_contract(
            state,
            &dex_amm_pk,
            deployer_pubkey,
            "call",
            &args,
            &format!("dex_amm.create_pool({})", label),
        ) {
            info!("  POOL {}", label);
            created_pools += 1;
        }
    }

    info!("──────────────────────────────────────────────────────");
    info!(
        "  Genesis pairs: {} pairs, {} pools created",
        created_pairs, created_pools
    );
    info!("──────────────────────────────────────────────────────");
}

// ═══════════════════════════════════════════════════════════════════════
//  SUPERVISOR — wraps the validator in a restart loop.
//  When the internal watchdog detects a stall it exits with EXIT_CODE_RESTART;
//  the supervisor catches that and relaunches the process automatically.
//  Pass --no-watchdog to disable the supervisor entirely (e.g. when using
//  systemd Restart=on-failure which already handles restarts).
// ═══════════════════════════════════════════════════════════════════════

fn main() {
    let args: Vec<String> = env::args().collect();

    // If we're the child (worker) process, go straight to the async validator.
    if args.iter().any(|a| a == "--supervised") {
        return run_validator_sync();
    }

    // If the user opted out of the built-in supervisor, also run directly.
    if args.iter().any(|a| a == "--no-watchdog") {
        return run_validator_sync();
    }

    // Parse supervisor-specific flags
    let max_restarts = args
        .iter()
        .position(|a| a == "--max-restarts")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_RESTARTS);

    // ── Supervisor loop ─────────────────────────────────────────────
    // Re-exec ourselves with --supervised so the child enters run_validator()
    // directly.  On EXIT_CODE_RESTART → restart.  On 0 or SIGTERM → stop.
    let exe = env::current_exe().expect("Cannot determine own executable path");

    // Build child args: forward everything except supervisor-only flags,
    // then append --supervised.
    let child_args: Vec<String> = args[1..]
        .iter()
        .filter(|a| {
            !matches!(
                a.as_str(),
                "--no-watchdog" | "--max-restarts" | "--supervised"
            )
        })
        .cloned()
        .collect();

    let mut restart_count: u32 = 0;
    let mut backoff_secs: u64 = 1;

    // Initialize minimal logging for supervisor messages
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!(
        "🐺 MoltChain Supervisor started (max restarts: {})",
        max_restarts
    );

    loop {
        info!(
            "🚀 Launching validator (attempt {}/{})",
            restart_count + 1,
            max_restarts
        );

        let child_start = std::time::Instant::now();
        let mut child = std::process::Command::new(&exe)
            .args(&child_args)
            .arg("--supervised")
            .stdin(std::process::Stdio::null())
            .spawn()
            .expect("Failed to spawn validator process");

        let status = child.wait().expect("Failed to wait on validator process");

        // L7 fix: reset backoff if child ran successfully for >3 minutes
        let runtime = child_start.elapsed();
        if runtime > Duration::from_secs(180) {
            backoff_secs = 1;
            restart_count = 0;
            info!(
                "🔄 Validator ran for {}s — resetting backoff",
                runtime.as_secs()
            );
        }

        match status.code() {
            Some(0) => {
                info!("✅ Validator exited cleanly (code 0) — shutting down supervisor");
                break;
            }
            Some(EXIT_CODE_RESTART) => {
                restart_count += 1;
                if restart_count >= max_restarts {
                    error!(
                        "❌ Validator requested restart but max restarts ({}) reached — giving up",
                        max_restarts
                    );
                    std::process::exit(1);
                }
                warn!(
                    "🔄 Validator stall detected (exit {}) — restarting in {}s (restart {}/{})",
                    EXIT_CODE_RESTART, backoff_secs, restart_count, max_restarts
                );
                std::thread::sleep(Duration::from_secs(backoff_secs));
                // Exponential backoff capped at 30s, reset after 3 successful minutes
                backoff_secs = (backoff_secs * 2).min(30);
            }
            Some(code) => {
                restart_count += 1;
                if restart_count >= max_restarts {
                    error!(
                        "❌ Validator crashed (exit {}) and max restarts ({}) reached — giving up",
                        code, max_restarts
                    );
                    std::process::exit(code);
                }
                warn!(
                    "💥 Validator crashed (exit {}) — restarting in {}s (restart {}/{})",
                    code, backoff_secs, restart_count, max_restarts
                );
                std::thread::sleep(Duration::from_secs(backoff_secs));
                backoff_secs = (backoff_secs * 2).min(30);
            }
            None => {
                // Killed by signal (SIGTERM, SIGKILL, etc.)
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(sig) = status.signal() {
                        if sig == 15 || sig == 2 {
                            // SIGTERM or SIGINT — graceful shutdown
                            info!(
                                "🛑 Validator terminated by signal {} — shutting down supervisor",
                                sig
                            );
                            break;
                        }
                        warn!("💥 Validator killed by signal {} — restarting", sig);
                    }
                }
                restart_count += 1;
                if restart_count >= max_restarts {
                    error!("❌ Max restarts reached after signal kill — giving up");
                    std::process::exit(1);
                }
                std::thread::sleep(Duration::from_secs(backoff_secs));
                backoff_secs = (backoff_secs * 2).min(30);
            }
        }
    }
}

/// Synchronous wrapper that sets up the tokio runtime and runs the validator.
fn run_validator_sync() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime");
    rt.block_on(run_validator());
}

/// The actual validator entrypoint — all existing logic lives here.
async fn run_validator() {
    // Initialize logging (only if not already initialized by supervisor)
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init();

    info!("🦞 MoltChain Validator starting...");

    // Parse CLI args for P2P configuration
    let args: Vec<String> = env::args().collect();

    // Parse --genesis flag
    let genesis_path = args
        .iter()
        .position(|arg| arg == "--genesis")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.to_string());

    // Parse --network flag (testnet | mainnet)
    let network_arg = args
        .iter()
        .position(|arg| arg == "--network")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.to_lowercase());

    // Parse --p2p-port flag properly
    let p2p_port = args
        .iter()
        .position(|arg| arg == "--p2p-port")
        .and_then(|pos| args.get(pos + 1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8000);

    // Parse --db-path / --db / --data-dir flag or use default based on port
    let data_dir = args
        .iter()
        .position(|arg| arg == "--db-path" || arg == "--db" || arg == "--data-dir")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("./data/state-{}", p2p_port));
    // Canonicalize to absolute path to prevent CWD-dependent state location
    let data_dir_path = std::fs::canonicalize(&data_dir).unwrap_or_else(|_| {
        // Directory doesn't exist yet — resolve parent + leaf
        let p = PathBuf::from(&data_dir);
        if p.is_absolute() {
            p
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(&p)
        }
    });
    let data_dir = data_dir_path.to_string_lossy().to_string();
    info!("📂 Data directory: {}", data_dir);

    let signer_bind = match env::var("MOLTCHAIN_SIGNER_BIND") {
        Ok(value) if value.eq_ignore_ascii_case("off") => None,
        Ok(value) => Some(value),
        Err(_) => {
            let offset = p2p_port % 1000;
            let derived_port = 9200u16.saturating_add(offset);
            Some(format!("0.0.0.0:{}", derived_port))
        }
    };

    if let Some(bind) = signer_bind {
        if let Ok(addr) = bind.parse::<SocketAddr>() {
            let signer_data_dir = data_dir_path.clone();
            tokio::spawn(async move {
                threshold_signer::start_signer_server(addr, &signer_data_dir).await;
            });
        } else {
            warn!("Invalid MOLTCHAIN_SIGNER_BIND value: {}", bind);
        }
    }

    // Open state database
    let state = match StateStore::open(&data_dir) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open state: {}", e);
            return;
        }
    };

    // Create transaction processor
    let processor = Arc::new(TxProcessor::new(state.clone()));

    // ========================================================================
    // GENESIS CONFIGURATION
    // ========================================================================

    // Load genesis configuration from file or use defaults
    let genesis_config = if let Some(ref genesis_file) = genesis_path {
        info!("📜 Loading genesis from: {}", genesis_file);
        match GenesisConfig::from_file(genesis_file) {
            Ok(config) => {
                info!("✓ Genesis loaded successfully");
                info!("  Chain ID: {}", config.chain_id);
                info!("  Total supply: {} MOLT", config.total_supply_molt());
                info!("  Initial validators: {}", config.initial_validators.len());
                config
            }
            Err(e) => {
                error!("Failed to load genesis: {}", e);
                return;
            }
        }
    } else {
        match network_arg.as_deref() {
            Some("mainnet") => {
                info!("⚠️  No genesis file specified, using default mainnet genesis");
                GenesisConfig::default_mainnet()
            }
            Some("testnet") | None => {
                info!("⚠️  No genesis file specified, using default testnet genesis");
                GenesisConfig::default_testnet()
            }
            Some(other) => {
                warn!(
                    "⚠️  Unknown network '{}', defaulting to testnet genesis",
                    other
                );
                GenesisConfig::default_testnet()
            }
        }
    };

    // P2P NETWORK SETUP - do this early to check if joining existing network
    info!("🦞 Initializing P2P network...");

    // Parse seed peers from CLI
    // Supports:
    //   --bootstrap <host:port>
    //   --bootstrap-peers <host:port,host:port>
    //   positional peers (legacy)
    let mut seed_peer_strings: Vec<String> = Vec::new();
    let mut explicit_seed_peer_strings: Vec<String> = Vec::new();
    let mut skip_next = false;
    for (i, arg) in args.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }

        match arg.as_str() {
            "--bootstrap" => {
                if let Some(value) = args.get(i + 1) {
                    seed_peer_strings.push(value.to_string());
                    explicit_seed_peer_strings.push(value.to_string());
                }
                skip_next = true;
            }
            "--bootstrap-peers" => {
                if let Some(value) = args.get(i + 1) {
                    for part in value.split(',') {
                        seed_peer_strings.push(part.to_string());
                        explicit_seed_peer_strings.push(part.to_string());
                    }
                }
                skip_next = true;
            }
            "--rpc-port"
            | "--ws-port"
            | "--p2p-port"
            | "--db-path"
            | "--genesis"
            | "--keypair"
            | "--network"
            | "--admin-token"
            | "--watchdog-timeout"
            | "--max-restarts"
            | "--listen-addr"
            | "--auto-update"
            | "--update-check-interval"
            | "--update-channel" => {
                skip_next = true;
            }
            "--supervised" | "--no-watchdog" | "--no-auto-restart" => {
                // Supervisor flags / boolean flags — skip without consuming next arg
                continue;
            }
            _ => {
                if i == 0 {
                    continue; // binary name
                }
                seed_peer_strings.push(arg.to_string());
                explicit_seed_peer_strings.push(arg.to_string());
            }
        }
    }

    // Parse --listen-addr flag for P2P bind address (default: 127.0.0.1 = local only)
    // For VPS / production use: --listen-addr 0.0.0.0
    let listen_host = args
        .iter()
        .position(|arg| arg == "--listen-addr")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());

    // ── Auto-Update Configuration ───────────────────────────────────────
    let auto_update_mode = args
        .iter()
        .position(|arg| arg == "--auto-update")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| updater::UpdateMode::parse_mode(s))
        .unwrap_or(updater::UpdateMode::Off);

    let update_check_interval = args
        .iter()
        .position(|arg| arg == "--update-check-interval")
        .and_then(|pos| args.get(pos + 1))
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(300);

    let update_channel = args
        .iter()
        .position(|arg| arg == "--update-channel")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "stable".to_string());

    let no_auto_restart = args.iter().any(|a| a == "--no-auto-restart");

    let update_config = updater::UpdateConfig {
        mode: auto_update_mode,
        check_interval_secs: update_check_interval,
        channel: update_channel,
        no_auto_restart,
        jitter_max_secs: 60,
    };

    // Spawn auto-updater background task
    info!("🔄 Validator version: v{}", updater::VERSION);
    let _updater_handle = updater::spawn_update_checker(update_config);

    let data_dir_path = Path::new(&data_dir);
    let peer_store_path = data_dir_path.join("known-peers.json");
    let listen_addr: SocketAddr = format!("{}:{}", listen_host, p2p_port)
        .parse()
        .expect("Invalid listen address (check --listen-addr)");

    let mut seed_peers = resolve_peer_list(&seed_peer_strings);
    let explicit_seed_peers = resolve_peer_list(&explicit_seed_peer_strings);
    let seeds_path = Path::new("seeds.json");
    let local_only = listen_addr.ip().is_loopback();
    let cached_peers = if explicit_seed_peers.is_empty() && !local_only {
        let seed_file_peers = load_seed_peers(&genesis_config.chain_id, seeds_path);
        seed_peers.extend(resolve_peer_list(&seed_file_peers));
        let cached = moltchain_p2p::PeerStore::load_from_path(&peer_store_path);
        seed_peers.extend(cached.iter().copied());
        cached
    } else {
        info!("🔒 Local bootstrap only: external seed peers disabled");
        Vec::new()
    };

    let mut seen = HashSet::new();
    seed_peers.retain(|addr| {
        if *addr == listen_addr {
            return false;
        }
        seen.insert(*addr)
    });

    let p2p_config = P2PConfig {
        listen_addr,
        seed_peers: seed_peers.clone(),
        gossip_interval: 10,
        cleanup_timeout: 300,
        peer_store_path: Some(peer_store_path.clone()),
        max_known_peers: 200,
    };

    let has_genesis_block = state.get_block_by_slot(0).unwrap_or(None).is_some();

    // Join network if we have seed peers and no local genesis yet
    let mut is_joining_network =
        (!explicit_seed_peers.is_empty() || !cached_peers.is_empty()) && !has_genesis_block;

    // ========================================================================
    // GENESIS STATE INITIALIZATION
    // ========================================================================

    // Genesis wallet path
    let genesis_wallet_path = data_dir_path.join("genesis-wallet.json");
    let genesis_keypairs_dir = data_dir_path.join("genesis-keys");
    std::fs::create_dir_all(&genesis_keypairs_dir).ok();

    // DYNAMIC GENESIS GENERATION
    // First validator starting after reset generates everything fresh
    let mut genesis_signer: Option<Keypair> = None;
    let (genesis_wallet, genesis_pubkey) = if has_genesis_block {
        if genesis_wallet_path.exists() {
            match GenesisWallet::load(&genesis_wallet_path) {
                Ok(wallet) => (Some(wallet.clone()), Some(wallet.pubkey)),
                Err(e) => {
                    warn!("⚠️  Failed to load genesis wallet: {}", e);
                    (None, None)
                }
            }
        } else {
            warn!("⚠️  Genesis wallet not found; genesis will not be regenerated");
            (None, None)
        }
    } else if !is_joining_network {
        info!("🔐 Generating FRESH genesis wallet (DYNAMIC GENERATION)");

        // Production-ready multi-sig for BOTH testnet and mainnet
        let is_mainnet = genesis_config.chain_id.contains("mainnet");
        let (signer_count, threshold_desc) = if is_mainnet {
            (5, "3/5 production multi-sig")
        } else {
            (3, "2/3 testnet multi-sig")
        };

        info!("  🔐 Creating {} setup...", threshold_desc);

        // Generate genesis wallet with multi-sig
        let (wallet, keypairs, distribution_keypairs) =
            GenesisWallet::generate(&genesis_config.chain_id, is_mainnet, signer_count)
                .expect("Failed to generate genesis wallet");

        genesis_signer = keypairs
            .first()
            .map(|keypair| Keypair::from_seed(&keypair.to_seed()));

        let pubkey = wallet.pubkey; // Extract before moving
        info!("  ✓ Generated genesis pubkey: {}", pubkey);

        if let Some(ref multisig) = wallet.multisig {
            info!("  ✓ Multi-sig configuration:");
            info!(
                "    - Threshold: {}/{} signatures",
                multisig.threshold,
                multisig.signers.len()
            );
            info!("    - Genesis treasury: {}", multisig.is_genesis);
            info!("    - Signers:");
            for (i, signer) in multisig.signers.iter().enumerate() {
                info!("      {}. {}", i + 1, signer.to_base58());
            }
        }

        // Log whitepaper distribution
        if let Some(ref dist) = wallet.distribution_wallets {
            info!(
                "  📊 Whitepaper genesis distribution ({} wallets):",
                dist.len()
            );
            for dw in dist {
                info!(
                    "    - {} ({}%): {} MOLT → {}",
                    dw.role,
                    dw.percentage,
                    dw.amount_molt,
                    dw.pubkey.to_base58()
                );
            }
        }

        // Save wallet info
        wallet
            .save(&genesis_wallet_path)
            .expect("Failed to save genesis wallet");
        info!("  ✓ Wallet info saved: {}", genesis_wallet_path.display());

        // Save all signer keypairs
        let keypair_paths = GenesisWallet::save_keypairs(
            &keypairs,
            &genesis_keypairs_dir,
            &genesis_config.chain_id,
        )
        .expect("Failed to save keypairs");

        // Save all distribution keypairs (one per whitepaper wallet)
        let dist_keypair_paths = GenesisWallet::save_distribution_keypairs(
            wallet.distribution_wallets.as_ref().unwrap(),
            &distribution_keypairs,
            &genesis_keypairs_dir,
            &genesis_config.chain_id,
        )
        .expect("Failed to save distribution keypairs");

        // Save treasury keypair separately for backward compat (start-local-stack.sh)
        // Treasury = validator_rewards = first distribution keypair
        let treasury_keypair_path = GenesisWallet::save_treasury_keypair(
            &distribution_keypairs[0],
            &genesis_keypairs_dir,
            &genesis_config.chain_id,
        )
        .expect("Failed to save treasury keypair");

        info!("  ✓ Saved {} signer keypair(s):", keypair_paths.len());
        for path in &keypair_paths {
            info!("    - {}", path);
        }
        info!(
            "  ✓ Saved {} distribution keypair(s):",
            dist_keypair_paths.len()
        );
        for path in &dist_keypair_paths {
            info!("    - {}", path);
        }
        info!("  ✓ Treasury keypair: {}", treasury_keypair_path);

        info!("  ⚠️  KEEP THESE FILES SECURE - THEY CONTROL THE GENESIS TREASURY");

        (Some(wallet), Some(pubkey))
    } else {
        // Joining network - will sync genesis from peers
        info!("🔄 Joining existing network - genesis wallet will sync from peers");
        (None, None)
    };

    let genesis_exists = has_genesis_block;

    // --- Migration: ensure genesis/treasury pubkeys are stored in DB ---
    // Older DBs may not have these keys set. Backfill from genesis-wallet.json.
    if genesis_exists {
        if let Some(ref gpk) = genesis_pubkey {
            if state.get_genesis_pubkey().ok().flatten().is_none() {
                if let Err(e) = state.set_genesis_pubkey(gpk) {
                    warn!("⚠️  Migration: failed to set genesis pubkey: {}", e);
                } else {
                    info!("  ✓ Migration: stored genesis pubkey in DB");
                }
            }
        }
        if let Some(ref gw) = genesis_wallet {
            if let Some(ref tpk) = gw.treasury_pubkey {
                if state.get_treasury_pubkey().ok().flatten().is_none() {
                    if let Err(e) = state.set_treasury_pubkey(tpk) {
                        warn!("⚠️  Migration: failed to set treasury pubkey: {}", e);
                    } else {
                        info!("  ✓ Migration: stored treasury pubkey in DB");
                    }
                }
            }
        }
        // Backfill genesis accounts from wallet if missing in DB
        if state
            .get_genesis_accounts()
            .map(|v| v.is_empty())
            .unwrap_or(true)
        {
            if let Some(ref gw) = genesis_wallet {
                if let Some(ref dist_wallets) = gw.distribution_wallets {
                    let ga_entries: Vec<(String, Pubkey, u64, u8)> = dist_wallets
                        .iter()
                        .map(|dw| (dw.role.clone(), dw.pubkey, dw.amount_molt, dw.percentage))
                        .collect();
                    if let Err(e) = state.set_genesis_accounts(&ga_entries) {
                        warn!("⚠️  Migration: failed to store genesis accounts: {}", e);
                    } else {
                        info!(
                            "  ✓ Migration: stored {} genesis accounts in DB",
                            ga_entries.len()
                        );
                    }
                }
            }
        }
    }

    // --- Fetch genesis accounts from bootstrap peer if still missing ---
    // This handles V2/V3 joining the network without genesis-wallet.json
    if state
        .get_genesis_accounts()
        .map(|v| v.is_empty())
        .unwrap_or(true)
        && !explicit_seed_peer_strings.is_empty()
    {
        info!("  🔄 Fetching genesis accounts from bootstrap peer...");
        for peer in &explicit_seed_peer_strings {
            // Derive RPC port from P2P port
            let parts: Vec<&str> = peer.split(':').collect();
            if let (Some(host), Some(p2p_port_str)) = (parts.first(), parts.get(1)) {
                if let Ok(peer_p2p) = p2p_port_str.parse::<u16>() {
                    let peer_rpc = if peer_p2p == 8000 {
                        8899
                    } else {
                        let offset = peer_p2p % 1000;
                        8900u16
                            .saturating_add(offset.saturating_mul(2))
                            .saturating_add(1)
                    };
                    let url = format!("http://{}:{}/", host, peer_rpc);
                    let body = serde_json::json!({
                        "jsonrpc": "2.0", "id": 1, "method": "getGenesisAccounts"
                    });
                    match reqwest::Client::new()
                        .post(&url)
                        .json(&body)
                        .timeout(std::time::Duration::from_secs(5))
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            if let Ok(json) = resp.json::<serde_json::Value>().await {
                                if let Some(accounts) = json["result"]["accounts"].as_array() {
                                    let mut ga_entries = Vec::new();
                                    for acc in accounts {
                                        let role = acc["role"].as_str().unwrap_or("").to_string();
                                        if role == "genesis" {
                                            continue; // Skip the genesis signer entry
                                        }
                                        let pk_str = acc["pubkey"].as_str().unwrap_or("");
                                        if let Ok(pk) = Pubkey::from_base58(pk_str) {
                                            let amt = acc["amount_molt"].as_u64().unwrap_or(0);
                                            let pct = acc["percentage"].as_u64().unwrap_or(0) as u8;
                                            ga_entries.push((role, pk, amt, pct));
                                        }
                                    }
                                    if !ga_entries.is_empty() {
                                        if let Err(e) = state.set_genesis_accounts(&ga_entries) {
                                            warn!(
                                                "⚠️  Failed to store fetched genesis accounts: {}",
                                                e
                                            );
                                        } else {
                                            info!(
                                                "  ✓ Fetched {} genesis accounts from {}",
                                                ga_entries.len(),
                                                peer
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "  ⚠️  Failed to fetch genesis accounts from {}: {}",
                                peer, e
                            );
                        }
                    }
                }
            }
        }
    }

    if !genesis_exists && !is_joining_network {
        let genesis_pubkey = genesis_pubkey.expect("Missing genesis pubkey for creation");
        let genesis_wallet = genesis_wallet
            .as_ref()
            .expect("Missing genesis wallet for creation");
        info!("📦 Creating genesis state from auto-generated wallet");

        if let Err(e) = state.set_rent_params(
            genesis_config.features.rent_rate_shells_per_kb_month,
            genesis_config.features.rent_free_kb,
        ) {
            warn!("⚠️  Failed to store rent params: {}", e);
        }

        // Persist fee configuration from genesis config into DB
        let genesis_fee_config = FeeConfig {
            base_fee: genesis_config.features.base_fee_shells,
            contract_deploy_fee: CONTRACT_DEPLOY_FEE,
            contract_upgrade_fee: CONTRACT_UPGRADE_FEE,
            nft_mint_fee: NFT_MINT_FEE,
            nft_collection_fee: NFT_COLLECTION_FEE,
            fee_burn_percent: genesis_config.features.fee_burn_percentage,
            fee_producer_percent: genesis_config.features.fee_producer_percentage,
            fee_voters_percent: genesis_config.features.fee_voters_percentage,
            fee_treasury_percent: 100u64
                .saturating_sub(genesis_config.features.fee_burn_percentage)
                .saturating_sub(genesis_config.features.fee_producer_percentage)
                .saturating_sub(genesis_config.features.fee_voters_percentage),
        };
        if let Err(e) = state.set_fee_config_full(&genesis_fee_config) {
            warn!("⚠️  Failed to store fee config: {}", e);
        } else {
            info!("  ✓ Fee config persisted: base={} shells, burn={}%, producer={}%, voters={}%, treasury={}%",
                genesis_fee_config.base_fee,
                genesis_fee_config.fee_burn_percent,
                genesis_fee_config.fee_producer_percent,
                genesis_fee_config.fee_voters_percent,
                genesis_fee_config.fee_treasury_percent,
            );
        }

        // Create genesis treasury account with full supply
        let total_supply_molt = 1_000_000_000u64;
        let mut genesis_account = Account::new(total_supply_molt, genesis_pubkey);

        // Store multi-sig configuration in account metadata (if enabled)
        if let Some(ref multisig) = genesis_wallet.multisig {
            // Mark as genesis treasury
            genesis_account.owner = genesis_pubkey; // Self-owned
            info!("  ✓ Flagged as genesis treasury with multi-sig");
            info!(
                "    Threshold: {}/{} signatures",
                multisig.threshold,
                multisig.signers.len()
            );
        }

        if let Err(e) = state.put_account(&genesis_pubkey, &genesis_account) {
            eprintln!("Failed to store genesis account: {e}");
        }
        if let Err(e) = state.set_genesis_pubkey(&genesis_pubkey) {
            eprintln!("Failed to set genesis pubkey: {e}");
        }
        info!("  ✓ Genesis mint: {} MOLT", total_supply_molt);
        info!("  ✓ Address: {}", genesis_pubkey.to_base58());

        // ════════════════════════════════════════════════════
        // WHITEPAPER GENESIS DISTRIBUTION (6 wallets, 1B total)
        // ════════════════════════════════════════════════════
        // Apply distribution directly to state — cannot use process_transaction()
        // here because no blocks exist yet and T1.3 rejects zero-blockhash txs.
        // Corresponding ledger entries are recorded in the genesis block below.
        if let Some(ref dist_wallets) = genesis_wallet.distribution_wallets {
            info!("📊 Creating whitepaper genesis distribution:");

            let mut src_acct = match state.get_account(&genesis_pubkey).ok().flatten() {
                Some(a) => a,
                None => {
                    error!("Genesis account missing after creation — cannot distribute");
                    Account::new(0, genesis_pubkey)
                }
            };

            for dw in dist_wallets {
                let amount_shells = Account::molt_to_shells(dw.amount_molt);

                // Create distribution account
                let mut acct = Account::new(0, SYSTEM_ACCOUNT_OWNER);
                acct.shells = amount_shells;
                acct.spendable = amount_shells;
                if let Err(e) = state.put_account(&dw.pubkey, &acct) {
                    error!("Failed to create {} account: {e}", dw.role);
                }

                // Debit genesis
                src_acct.shells = src_acct.shells.saturating_sub(amount_shells);
                src_acct.spendable = src_acct.spendable.saturating_sub(amount_shells);

                // Set treasury pubkey for the validator_rewards wallet
                if dw.role == "validator_rewards" {
                    if let Err(e) = state.set_treasury_pubkey(&dw.pubkey) {
                        error!("Failed to set treasury pubkey: {e}");
                    }
                    info!(
                        "  ✓ {} ({}%): {} MOLT → {} [TREASURY]",
                        dw.role,
                        dw.percentage,
                        dw.amount_molt,
                        dw.pubkey.to_base58()
                    );
                } else {
                    info!(
                        "  ✓ {} ({}%): {} MOLT → {}",
                        dw.role,
                        dw.percentage,
                        dw.amount_molt,
                        dw.pubkey.to_base58()
                    );
                }
            }

            if let Err(e) = state.put_account(&genesis_pubkey, &src_acct) {
                error!("Failed to update genesis account after distribution: {e}");
            }

            // Store genesis accounts in state DB for RPC/explorer lookups
            let ga_entries: Vec<(String, Pubkey, u64, u8)> = dist_wallets
                .iter()
                .map(|dw| (dw.role.clone(), dw.pubkey, dw.amount_molt, dw.percentage))
                .collect();
            if let Err(e) = state.set_genesis_accounts(&ga_entries) {
                error!("Failed to store genesis accounts in DB: {e}");
            } else {
                info!(
                    "  ✓ Stored {} genesis accounts in state DB",
                    ga_entries.len()
                );
            }

            info!("  ✓ Genesis distribution complete — 1B MOLT allocated per whitepaper");
        }
        // Legacy: single treasury (backward compat for old wallet files)
        else if let Some(treasury_pubkey) = genesis_wallet.treasury_pubkey {
            let reward_pool_molt = REWARD_POOL_MOLT.min(1_000_000_000);
            let treasury_account = Account::new(0, SYSTEM_ACCOUNT_OWNER);
            if let Err(e) = state.put_account(&treasury_pubkey, &treasury_account) {
                eprintln!("Failed to store treasury account: {e}");
            }
            if let Err(e) = state.set_treasury_pubkey(&treasury_pubkey) {
                eprintln!("Failed to set treasury pubkey: {e}");
            }
            info!(
                "  ✓ Treasury account created: {}",
                treasury_pubkey.to_base58()
            );
            info!("  ✓ Reward pool pending: {} MOLT", reward_pool_molt);

            let reward_shells = Account::molt_to_shells(reward_pool_molt);

            let mut src_acct = match state.get_account(&genesis_pubkey).ok().flatten() {
                Some(a) => a,
                None => {
                    error!("Genesis account missing after creation — cannot fund treasury");
                    Account::new(0, genesis_pubkey)
                }
            };
            src_acct.shells = src_acct.shells.saturating_sub(reward_shells);
            src_acct.spendable = src_acct.spendable.saturating_sub(reward_shells);
            if let Err(e) = state.put_account(&genesis_pubkey, &src_acct) {
                error!("Failed to update genesis account balance: {e}");
            }

            let mut trs_acct = state
                .get_account(&treasury_pubkey)
                .ok()
                .flatten()
                .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));
            trs_acct.shells = trs_acct.shells.saturating_add(reward_shells);
            trs_acct.spendable = trs_acct.spendable.saturating_add(reward_shells);
            if let Err(e) = state.put_account(&treasury_pubkey, &trs_acct) {
                error!("Failed to update treasury account balance: {e}");
            }

            info!("  ✓ Reward pool funded via genesis transfer tx");
        }

        // Create initial accounts from genesis config (if any)
        for account_info in &genesis_config.initial_accounts {
            let pubkey = match Pubkey::from_base58(&account_info.address) {
                Ok(pk) => pk,
                Err(e) => {
                    warn!(
                        "Skipping initial account with invalid address {}: {e}",
                        account_info.address
                    );
                    continue;
                }
            };
            let account = Account::new(account_info.balance_molt, pubkey);
            if let Err(e) = state.put_account(&pubkey, &account) {
                eprintln!("Failed to store initial account: {e}");
            }
            info!(
                "  ✓ Account {}: {} MOLT",
                &account_info.address[..20],
                account_info.balance_molt
            );
        }

        let mut genesis_txs = Vec::new();

        let mint_shells = Account::molt_to_shells(total_supply_molt);
        let mut mint_data = Vec::with_capacity(9);
        mint_data.push(5); // Genesis mint (synthetic, fee-free)
        mint_data.extend_from_slice(&mint_shells.to_le_bytes());

        let mint_instruction = Instruction {
            program_id: CORE_SYSTEM_PROGRAM_ID,
            accounts: vec![GENESIS_MINT_PUBKEY, genesis_pubkey],
            data: mint_data,
        };

        let mint_message = Message::new(vec![mint_instruction], Hash::default());
        let mut mint_tx = Transaction::new(mint_message);
        mint_tx.signatures.push([0u8; 64]);
        state.put_transaction(&mint_tx).ok();
        genesis_txs.push(mint_tx);

        // Record distribution transfers in genesis block
        // (validator_rewards FIRST for backward-compatible treasury extraction)
        if let Some(ref dist_wallets) = genesis_wallet.distribution_wallets {
            let signer = genesis_signer
                .as_ref()
                .expect("Missing genesis signer for distribution funding");

            for dw in dist_wallets {
                let mut data = Vec::with_capacity(9);
                data.push(4); // Genesis transfer (fee-free)
                data.extend_from_slice(&Account::molt_to_shells(dw.amount_molt).to_le_bytes());

                let instruction = Instruction {
                    program_id: CORE_SYSTEM_PROGRAM_ID,
                    accounts: vec![genesis_pubkey, dw.pubkey],
                    data,
                };

                let message = Message::new(vec![instruction], Hash::default());
                let mut tx = Transaction::new(message.clone());
                let signature = signer.sign(&message.serialize());
                tx.signatures.push(signature);
                state.put_transaction(&tx).ok();
                genesis_txs.push(tx);
            }
        }
        // Legacy: single treasury transfer (backward compat)
        else if let Some(treasury_pubkey) = genesis_wallet.treasury_pubkey {
            let reward_pool_molt = REWARD_POOL_MOLT.min(1_000_000_000);
            let mut data = Vec::with_capacity(9);
            data.push(4); // Genesis transfer (fee-free)
            data.extend_from_slice(&Account::molt_to_shells(reward_pool_molt).to_le_bytes());

            let instruction = Instruction {
                program_id: CORE_SYSTEM_PROGRAM_ID,
                accounts: vec![genesis_pubkey, treasury_pubkey],
                data,
            };

            let message = Message::new(vec![instruction], Hash::default());
            let mut treasury_tx = Transaction::new(message.clone());
            let signer = genesis_signer
                .as_ref()
                .expect("Missing genesis signer for treasury funding");
            let signature = signer.sign(&message.serialize());
            treasury_tx.signatures.push(signature);
            state.put_transaction(&treasury_tx).ok();
            genesis_txs.push(treasury_tx);
        }

        // Create genesis block
        let state_root = state.compute_state_root();
        let genesis_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let genesis_block = Block::genesis(state_root, genesis_timestamp, genesis_txs);
        if let Err(e) = state.put_block(&genesis_block) {
            error!("Failed to store genesis block: {e}");
        }
        if let Err(e) = state.set_last_slot(0) {
            error!("Failed to set initial slot: {e}");
        }
        info!("✓ Genesis block created and stored (slot 0)");
        info!("  Genesis hash: {}", genesis_block.hash());

        // Auto-deploy all compiled contracts from contracts/ directory
        genesis_auto_deploy(&state, &genesis_pubkey);
        genesis_initialize_contracts(&state, &genesis_pubkey);
        genesis_create_trading_pairs(&state, &genesis_pubkey);
    } else if genesis_exists {
        info!("✓ Genesis state already exists");
        let last_slot = state.get_last_slot().unwrap_or(0);
        info!("  Resuming from slot {}", last_slot);

        // Account reconciliation disabled on startup (too slow for large databases)
        // Use CLI command `molt admin reconcile-accounts` if needed
        let metrics = state.get_metrics();
        info!("  Total accounts (counter): {}", metrics.total_accounts);

        if let Some(wallet) = genesis_wallet.as_ref() {
            if let Some(treasury_pubkey) = wallet.treasury_pubkey {
                // Only set if not already stored (avoid overwriting canonical pubkey)
                if state.get_treasury_pubkey().ok().flatten().is_none() {
                    state.set_treasury_pubkey(&treasury_pubkey).ok();
                }
                if let Ok(None) = state.get_account(&treasury_pubkey) {
                    let treasury_account = Account::new(0, SYSTEM_ACCOUNT_OWNER);
                    state.put_account(&treasury_pubkey, &treasury_account).ok();
                }
            }
        }

        // ================================================================
        // MIGRATION: Auto-generate treasury keypair if missing
        // Handles genesis wallets created by older code versions that
        // did not generate a separate treasury keypair.
        // ================================================================
        let needs_treasury_migration = genesis_wallet
            .as_ref()
            .map(|w| w.treasury_pubkey.is_none())
            .unwrap_or(false);

        if needs_treasury_migration && state.get_treasury_pubkey().ok().flatten().is_none() {
            info!("🔄 MIGRATION: Genesis wallet missing treasury keypair — generating...");

            let treasury_keypair = Keypair::generate();
            let treasury_pubkey = treasury_keypair.pubkey();

            // 1. Save treasury keypair to disk
            match GenesisWallet::save_treasury_keypair(
                &treasury_keypair,
                &genesis_keypairs_dir,
                &genesis_config.chain_id,
            ) {
                Ok(path) => info!("  ✓ Treasury keypair saved: {}", path),
                Err(e) => error!("  ✗ Failed to save treasury keypair: {}", e),
            }

            // 2. Set treasury pubkey in state
            if let Err(e) = state.set_treasury_pubkey(&treasury_pubkey) {
                error!("  ✗ Failed to set treasury pubkey in state: {}", e);
            }

            // 3. Create treasury account
            let mut treasury_account = Account::new(0, SYSTEM_ACCOUNT_OWNER);

            // 4. Fund treasury from genesis account (transfer REWARD_POOL)
            let reward_shells = Account::molt_to_shells(REWARD_POOL_MOLT.min(1_000_000_000));
            if let Some(genesis_pk) = genesis_wallet.as_ref().map(|w| w.pubkey) {
                if let Ok(Some(mut genesis_acct)) = state.get_account(&genesis_pk) {
                    if genesis_acct.spendable >= reward_shells {
                        genesis_acct.shells = genesis_acct.shells.saturating_sub(reward_shells);
                        genesis_acct.spendable =
                            genesis_acct.spendable.saturating_sub(reward_shells);
                        treasury_account.shells = reward_shells;
                        treasury_account.spendable = reward_shells;
                        state.put_account(&genesis_pk, &genesis_acct).ok();
                        info!(
                            "  ✓ Funded treasury with {} MOLT from genesis",
                            REWARD_POOL_MOLT
                        );
                    } else {
                        warn!(
                            "  ⚠️  Genesis account has insufficient spendable balance ({} < {})",
                            genesis_acct.spendable, reward_shells
                        );
                    }
                }
            }

            state.put_account(&treasury_pubkey, &treasury_account).ok();
            info!("  ✓ Treasury account: {}", treasury_pubkey.to_base58());

            // 5. Update genesis wallet JSON with treasury info
            if let Some(mut wallet) = genesis_wallet.clone() {
                wallet.treasury_pubkey = Some(treasury_pubkey);
                wallet.treasury_keypair_path = Some(format!(
                    "genesis-keys/treasury-{}.json",
                    genesis_config.chain_id
                ));
                if let Err(e) = wallet.save(&genesis_wallet_path) {
                    error!("  ✗ Failed to update genesis wallet: {}", e);
                } else {
                    info!("  ✓ Updated genesis-wallet.json with treasury info");
                }
            }

            // 6. Persist fee config only if not already present
            {
                if state.get_fee_config().is_err() {
                    let fee_config = FeeConfig::default_from_constants();
                    if let Err(e) = state.set_fee_config_full(&fee_config) {
                        warn!("  ⚠️  Failed to persist fee config: {}", e);
                    } else {
                        info!("  ✓ Fee config persisted");
                    }
                }
            }

            info!("✅ Treasury migration complete");
        }
    }

    // Treasury keypair kept for governance/manual operations only.
    // Block rewards use protocol-level coinbase (no signing needed).
    let _treasury_keypair = load_treasury_keypair(
        genesis_wallet.as_ref(),
        &genesis_keypairs_dir,
        &genesis_config.chain_id,
    );

    // ========================================================================
    // VALIDATOR IDENTITY
    // ========================================================================

    // Load validator keypair from file (production-ready)
    // Priority order:
    // 1. --keypair CLI argument
    // 2. MOLTCHAIN_VALIDATOR_KEYPAIR env var
    // 3. ~/.moltchain/validators/validator-{port}.json
    // 4. Generate new and save

    let keypair_path = args
        .iter()
        .position(|arg| arg == "--keypair")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.as_str());

    let validator_keypair = keypair_loader::load_or_generate_keypair(keypair_path, p2p_port)
        .expect("Failed to load or generate validator keypair");

    let validator_pubkey = validator_keypair.pubkey();
    info!("🦞 Validator identity: {}", validator_pubkey.to_base58());
    info!("   Port: {}, Keypair loaded successfully", p2p_port);

    // ========================================================================
    // VALIDATOR SET INITIALIZATION
    // ========================================================================

    // Load or initialize validator set (shared across tasks)
    let validator_set = Arc::new(Mutex::new({
        let mut set = state
            .load_validator_set()
            .unwrap_or_else(|_| ValidatorSet::new());

        if set.validators().is_empty() {
            // Add genesis validators from configuration
            for validator_info in &genesis_config.initial_validators {
                let pubkey = match Pubkey::from_base58(&validator_info.pubkey) {
                    Ok(pk) => pk,
                    Err(e) => {
                        warn!(
                            "Skipping initial validator with invalid pubkey {}: {e}",
                            validator_info.pubkey
                        );
                        continue;
                    }
                };

                let validator = ValidatorInfo {
                    pubkey,
                    stake: Account::molt_to_shells(validator_info.stake_molt),
                    reputation: validator_info.reputation,
                    blocks_proposed: 0,
                    votes_cast: 0,
                    correct_votes: 0,
                    last_active_slot: 0,
                    joined_slot: 0,
                };

                set.add_validator(validator);
            }
        }

        // Add this validator if not already in genesis set
        // ⚠️ CRITICAL: Prevent genesis wallet from becoming a validator
        if let Some(genesis_pubkey) = genesis_pubkey {
            if validator_pubkey != genesis_pubkey {
                if !genesis_config
                    .initial_validators
                    .iter()
                    .any(|v| v.pubkey == validator_pubkey.to_base58())
                {
                    info!("⚠️  This validator not in genesis set, adding dynamically");
                    set.add_validator(ValidatorInfo {
                        pubkey: validator_pubkey,
                        stake: MIN_VALIDATOR_STAKE, // 100K MOLT stake — matches V2/V3 join grant
                        reputation: 100,
                        blocks_proposed: 0,
                        votes_cast: 0,
                        correct_votes: 0,
                        last_active_slot: 0,
                        joined_slot: 0,
                    });
                }
            } else {
                info!("🚫 Genesis wallet cannot be a validator");
            }
        } else if !genesis_config
            .initial_validators
            .iter()
            .any(|v| v.pubkey == validator_pubkey.to_base58())
        {
            info!("⚠️  This validator not in genesis set, adding dynamically");
            set.add_validator(ValidatorInfo {
                pubkey: validator_pubkey,
                stake: MIN_VALIDATOR_STAKE, // 100K MOLT stake — matches V2/V3 join grant
                reputation: 100,
                blocks_proposed: 0,
                votes_cast: 0,
                correct_votes: 0,
                last_active_slot: 0,
                joined_slot: 0,
            });
        }

        set
    }));

    // CRITICAL: Remove genesis wallet from validator set if it exists (cleanup for old bug)
    if let Some(genesis_pubkey) = genesis_pubkey {
        if let Ok(Some(_)) = state.get_validator(&genesis_pubkey) {
            info!("🧹 Cleaning up: Removing genesis wallet from validator set");
            if let Err(e) = state.delete_validator(&genesis_pubkey) {
                eprintln!("Failed to delete genesis validator: {e}");
            }
        }
    }

    // Save validator set to RocksDB on EVERY boot.
    // clear_all_validators() inside save_validator_set removes ghost entries from old
    // keypairs while preserving reputation/metrics for current validators via the
    // in-memory set that was loaded from DB above.
    if let Err(e) = state.save_validator_set(&*validator_set.lock().await) {
        eprintln!("Failed to save validator set: {e}");
    }

    info!(
        "✓ Validator set initialized with {} validators",
        validator_set.lock().await.validators().len()
    );

    // ============================================================================
    // VALIDATOR ACCOUNT CREATION / BOOTSTRAP GRANT
    // ============================================================================

    // Check if this validator has an account, if not create with bootstrap grant
    let validator_account = state.get_account(&validator_pubkey).unwrap_or_else(|e| {
        eprintln!("Failed to read validator account: {e}");
        None
    });
    if validator_account.is_none() {
        // H13 fix: Bootstrap grant must come from treasury, not ex nihilo
        let bootstrap_molt = MIN_VALIDATOR_STAKE / 1_000_000_000; // 100K MOLT — same as V2/V3 joining grant
        let bootstrap_shells = MIN_VALIDATOR_STAKE;
        let treasury_pk = state.get_treasury_pubkey().ok().flatten();
        let mut funded = false;

        if let Some(ref tpk) = treasury_pk {
            if let Ok(Some(mut treasury)) = state.get_account(tpk) {
                if treasury.spendable >= bootstrap_shells {
                    treasury.deduct_spendable(bootstrap_shells).ok();
                    state.put_account(tpk, &treasury).ok();
                    funded = true;
                    info!(
                        "💰 Bootstrap grant: {} MOLT deducted from treasury",
                        bootstrap_molt
                    );
                } else {
                    warn!(
                        "⚠️  Treasury has insufficient funds for bootstrap grant ({} < {})",
                        treasury.spendable, bootstrap_shells
                    );
                }
            }
        }

        if !funded {
            warn!("⚠️  No treasury available — bootstrap grant skipped. Validator needs manual funding.");
        }

        let bootstrap_account = if funded {
            // Set shells + staked directly — matches V2/V3 account creation pattern
            Account {
                shells: bootstrap_shells,
                spendable: 0,
                staked: bootstrap_shells,
                locked: 0,
                data: Vec::new(),
                owner: SYSTEM_ACCOUNT_OWNER,
                executable: false,
                rent_epoch: 0,
            }
        } else {
            // Create empty account — validator needs external funding
            Account::new(0, SYSTEM_ACCOUNT_OWNER)
        };

        if let Err(e) = state.put_account(&validator_pubkey, &bootstrap_account) {
            eprintln!("Failed to create validator account: {e}");
        }
        info!(
            "✓ Validator account created: {} MOLT total (0 spendable, 100K staked)",
            bootstrap_account.balance_molt()
        );
        info!(
            "   Spendable: {:.2} | Staked: {:.2} | Locked: {:.2}",
            bootstrap_account.spendable as f64 / 1_000_000_000.0,
            bootstrap_account.staked as f64 / 1_000_000_000.0,
            bootstrap_account.locked as f64 / 1_000_000_000.0
        );
    } else if let Some(account) = validator_account {
        info!(
            "✓ Validator account exists: {} MOLT",
            account.balance_molt()
        );
        info!(
            "   Spendable: {:.2} | Staked: {:.2} | Locked: {:.2}",
            account.spendable as f64 / 1_000_000_000.0,
            account.staked as f64 / 1_000_000_000.0,
            account.locked as f64 / 1_000_000_000.0
        );
    }

    // Initialize vote aggregator for BFT consensus
    let vote_aggregator = Arc::new(Mutex::new(VoteAggregator::new()));
    info!("🗳️  BFT voting system initialized");

    // Initialize slashing tracker
    let slashing_tracker = Arc::new(Mutex::new(SlashingTracker::new()));
    info!("⚔️  Slashing system initialized");

    // Initialize stake pool for economic security
    let stake_pool = Arc::new(Mutex::new(
        state.get_stake_pool().unwrap_or_else(|_| StakePool::new()),
    ));
    info!("💰 Stake pool initialized");

    // Periodically persist stake pool to disk
    let stake_pool_for_save = stake_pool.clone();
    let state_for_stake_save = state.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let pool = stake_pool_for_save.lock().await;
            if let Err(e) = state_for_stake_save.put_stake_pool(&pool) {
                warn!("⚠️  Failed to persist stake pool: {}", e);
            }
        }
    });

    // Stake tokens for this validator (10,000 MOLT minimum)
    // Uses get_stake() to avoid accumulating on every restart
    {
        let mut pool = stake_pool.lock().await;
        let current_slot = state.get_last_slot().unwrap_or(0);
        let existing = pool
            .get_stake(&validator_pubkey)
            .map(|s| s.amount)
            .unwrap_or(0);
        if existing >= MIN_VALIDATOR_STAKE {
            info!("✅ Already staked: {} MOLT", existing / 1_000_000_000);
        } else {
            pool.upsert_stake(validator_pubkey, MIN_VALIDATOR_STAKE, current_slot);
            info!(
                "💰 Staked {} MOLT (minimum required)",
                MIN_VALIDATOR_STAKE / 1_000_000_000
            );
            info!("💰 Validator is now economically secured");
        }
    };

    // Get starting slot (resume from last + 1)
    let last_slot = state.get_last_slot().unwrap_or(0);
    let mut slot = if last_slot == 0 { 1 } else { last_slot + 1 };
    info!("Starting from slot {}", slot);

    // Get parent hash - if joining network and no genesis yet, use placeholder
    let mut parent_hash = if slot == 1 {
        if let Ok(Some(genesis)) = state.get_block_by_slot(0) {
            genesis.hash()
        } else {
            // No genesis yet (joining network) - will be set when genesis syncs
            Hash::default()
        }
    } else {
        state
            .get_block_by_slot(slot - 1)
            .ok()
            .flatten()
            .map(|b| b.hash())
            .unwrap_or_else(|| {
                warn!("⚠️  Could not load previous block at slot {}", slot - 1);
                Hash::default()
            })
    };

    let needs_genesis = is_joining_network; // Track if we need to request genesis

    // Create channels for P2P communication
    // M11: Bounded channels prevent memory exhaustion from slow consumers.
    // Capacity tiers: high-throughput (txs/votes) → larger, control msgs → smaller.
    let (block_tx, mut block_rx) = mpsc::channel(500);
    let (vote_tx, mut vote_rx) = mpsc::channel(2_000);
    let (transaction_tx, mut transaction_rx) = mpsc::channel(5_000);
    let (validator_announce_tx, mut validator_announce_rx) = mpsc::channel(100);
    let (block_range_request_tx, mut block_range_request_rx) = mpsc::channel(200);
    let (status_request_tx, mut status_request_rx) = mpsc::channel::<StatusRequestMsg>(100);
    let (status_response_tx, mut status_response_rx) = mpsc::channel::<StatusResponseMsg>(100);
    let (consistency_report_tx, mut consistency_report_rx) =
        mpsc::channel::<ConsistencyReportMsg>(50);
    let (snapshot_request_tx, mut snapshot_request_rx) = mpsc::channel::<SnapshotRequestMsg>(50);
    let (snapshot_response_tx, mut snapshot_response_rx) = mpsc::channel::<SnapshotResponseMsg>(50);
    let (slashing_evidence_tx, mut slashing_evidence_rx) =
        mpsc::channel::<moltchain_core::SlashingEvidence>(100);

    // Create mempool
    let mempool = Arc::new(Mutex::new(Mempool::new(1000, 300))); // 1000 tx max, 300s expiration

    // Start P2P network - need to extract peer manager before starting
    let (p2p_peer_manager, _p2p_handle) = match P2PNetwork::new(
        p2p_config.clone(),
        block_tx,
        vote_tx,
        transaction_tx,
        validator_announce_tx,
        block_range_request_tx,
        status_request_tx,
        status_response_tx,
        consistency_report_tx,
        snapshot_request_tx,
        snapshot_response_tx,
        slashing_evidence_tx,
    )
    .await
    {
        Ok(network) => {
            info!("✅ P2P network initialized on port {}", p2p_port);

            // Get peer manager reference before network moves into spawn
            let peer_manager = network.peer_manager.clone();

            // Start accepting incoming connections
            peer_manager.start_accepting().await;
            info!("🔌 P2P: Started accepting incoming connections");

            // Start network message processing (consumes network)
            let handle = tokio::spawn(async move {
                network.start().await;
            });

            (Some(peer_manager), Some(handle))
        }
        Err(e) => {
            warn!("⚠️  P2P network failed to start: {}", e);
            warn!("⚠️  Running in single-validator mode");
            (None, None)
        }
    };

    // Create sync manager
    let sync_manager = Arc::new(SyncManager::new());
    let snapshot_sync = Arc::new(Mutex::new(SnapshotSync::new(is_joining_network)));

    // Track last block time for leader timeout handling
    let last_block_time = Arc::new(Mutex::new(std::time::Instant::now()));
    let last_block_time_for_blocks = last_block_time.clone();
    let last_block_time_for_local = last_block_time.clone();

    let slot_duration_ms = genesis_config.consensus.slot_duration_ms.max(1);
    let view_timeout = Duration::from_millis(slot_duration_ms * 3);

    // If joining network, immediately request genesis block (slot 0)
    if needs_genesis {
        if let Some(ref pm) = p2p_peer_manager {
            info!("📡 Requesting genesis block (slot 0) from network");
            let request_msg = P2PMessage::new(
                MessageType::BlockRangeRequest {
                    start_slot: 0,
                    end_slot: 0,
                },
                p2p_config.listen_addr,
            );
            pm.broadcast(request_msg).await;
            sync_manager.mark_requested(0).await;
        }
    }

    if needs_genesis {
        if let Some(ref pm) = p2p_peer_manager {
            let state_for_genesis_retry = state.clone();
            let peer_mgr_for_genesis_retry = pm.clone();
            let local_addr_for_genesis_retry = p2p_config.listen_addr;
            let sync_mgr_for_genesis_retry = sync_manager.clone();
            tokio::spawn(async move {
                let mut interval = time::interval(Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    if let Ok(Some(_)) = state_for_genesis_retry.get_block_by_slot(0) {
                        break;
                    }

                    let request = P2PMessage::new(
                        MessageType::BlockRangeRequest {
                            start_slot: 0,
                            end_slot: 0,
                        },
                        local_addr_for_genesis_retry,
                    );
                    peer_mgr_for_genesis_retry.broadcast(request).await;
                    sync_mgr_for_genesis_retry.mark_requested(0).await;
                }
            });
        }
    }

    if is_joining_network {
        if let Some(ref pm) = p2p_peer_manager {
            let peer_mgr_for_snapshot_retry = pm.clone();
            let local_addr_for_snapshot_retry = p2p_config.listen_addr;
            let snapshot_sync_for_retry = snapshot_sync.clone();
            tokio::spawn(async move {
                let mut interval = time::interval(Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    if snapshot_sync_for_retry.lock().await.is_ready() {
                        break;
                    }

                    let validator_request = P2PMessage::new(
                        MessageType::SnapshotRequest {
                            kind: SnapshotKind::ValidatorSet,
                        },
                        local_addr_for_snapshot_retry,
                    );
                    peer_mgr_for_snapshot_retry
                        .broadcast(validator_request)
                        .await;

                    let pool_request = P2PMessage::new(
                        MessageType::SnapshotRequest {
                            kind: SnapshotKind::StakePool,
                        },
                        local_addr_for_snapshot_retry,
                    );
                    peer_mgr_for_snapshot_retry.broadcast(pool_request).await;
                }
            });
        }
    }

    // Start incoming block handler with voting
    if let Some(ref p2p_pm) = p2p_peer_manager {
        let state_for_blocks = state.clone();
        let processor_for_blocks = processor.clone();
        let validator_pubkey_for_blocks = validator_pubkey;
        let validator_seed = validator_keypair.to_seed(); // Store seed to reconstruct keypair
        let sync_mgr = sync_manager.clone();
        let peer_mgr_for_sync = p2p_pm.clone();
        let vote_agg_for_blocks = vote_aggregator.clone();
        let validator_set_for_blocks = validator_set.clone();
        let stake_pool_for_blocks = stake_pool.clone();
        let vote_agg_for_effects = vote_aggregator.clone();
        let local_addr = p2p_config.listen_addr;
        let last_block_time_for_blocks = last_block_time_for_blocks.clone();
        let genesis_config_for_blocks = genesis_config.clone();
        tokio::spawn(async move {
            info!("🔄 Block receiver started");
            while let Some(block) = block_rx.recv().await {
                let block_slot = block.header.slot;

                // ── Block validation (T2.2) ──────────────────────────
                // Verify producer signature and structural limits BEFORE
                // accepting any block into local state.
                if !block.verify_signature() {
                    warn!(
                        "⚠️  Rejecting block {} — invalid signature from {}",
                        block_slot,
                        Pubkey(block.header.validator).to_base58()
                    );
                    continue;
                }
                if let Err(e) = block.validate_structure() {
                    warn!("⚠️  Rejecting block {} — {}", block_slot, e);
                    continue;
                }

                sync_mgr.note_seen(block_slot).await;
                let current_slot = state_for_blocks.get_last_slot().unwrap_or(0);

                // Handle genesis block specially (slot 0 when current is also 0)
                if block_slot == 0 && current_slot == 0 {
                    // M3 fix: Prevent overwriting an existing genesis block
                    if state_for_blocks
                        .get_block_by_slot(0)
                        .ok()
                        .flatten()
                        .is_some()
                    {
                        warn!("⚠️  Ignoring duplicate genesis block from network");
                        continue;
                    }
                    // Genesis block - store it and initialize full genesis state
                    if state_for_blocks.put_block(&block).is_ok() {
                        state_for_blocks.set_last_slot(0).ok();
                        *last_block_time_for_blocks.lock().await = std::time::Instant::now();

                        // ── C3 fix: Initialize genesis state from network block ──
                        // The local genesis path writes state directly; a joining
                        // validator must derive the same state from the genesis block
                        // transactions + genesis config.

                        // 1. Rent params from genesis config
                        state_for_blocks
                            .set_rent_params(
                                genesis_config_for_blocks
                                    .features
                                    .rent_rate_shells_per_kb_month,
                                genesis_config_for_blocks.features.rent_free_kb,
                            )
                            .ok();

                        // 2. Fee config from genesis config
                        let gc = &genesis_config_for_blocks;
                        let genesis_fee_config = FeeConfig {
                            base_fee: gc.features.base_fee_shells,
                            contract_deploy_fee: CONTRACT_DEPLOY_FEE,
                            contract_upgrade_fee: CONTRACT_UPGRADE_FEE,
                            nft_mint_fee: NFT_MINT_FEE,
                            nft_collection_fee: NFT_COLLECTION_FEE,
                            fee_burn_percent: gc.features.fee_burn_percentage,
                            fee_producer_percent: gc.features.fee_producer_percentage,
                            fee_voters_percent: gc.features.fee_voters_percentage,
                            fee_treasury_percent: 100u64
                                .saturating_sub(gc.features.fee_burn_percentage)
                                .saturating_sub(gc.features.fee_producer_percentage)
                                .saturating_sub(gc.features.fee_voters_percentage),
                        };
                        state_for_blocks
                            .set_fee_config_full(&genesis_fee_config)
                            .ok();

                        // 3. Extract genesis pubkey from mint tx
                        //    tx[0]: Mint — accounts = [GENESIS_MINT_PUBKEY, genesis_pubkey]
                        //    tx[1..]: Distribution transfers — accounts = [genesis_pubkey, recipient]
                        //    tx[1] is always the treasury (validator_rewards) for backward compat
                        let extracted_genesis_pubkey = block
                            .transactions
                            .first()
                            .and_then(|tx| tx.message.instructions.first())
                            .and_then(|ix| ix.accounts.get(1))
                            .copied();

                        if let Some(gpk) = extracted_genesis_pubkey {
                            // 4. Process all distribution transfers from genesis block
                            let total_supply_molt = 1_000_000_000u64;
                            let total_shells = Account::molt_to_shells(total_supply_molt);
                            let mut total_distributed_shells = 0u64;

                            for (i, tx) in block.transactions.iter().enumerate().skip(1) {
                                if let Some(ix) = tx.message.instructions.first() {
                                    if ix.data.first() == Some(&4) && ix.accounts.len() >= 2 {
                                        let recipient = ix.accounts[1];
                                        let amount_shells = if ix.data.len() >= 9 {
                                            u64::from_le_bytes(
                                                ix.data[1..9].try_into().unwrap_or([0u8; 8]),
                                            )
                                        } else {
                                            0
                                        };

                                        let mut acct = Account::new(0, SYSTEM_ACCOUNT_OWNER);
                                        acct.shells = amount_shells;
                                        acct.spendable = amount_shells;
                                        state_for_blocks.put_account(&recipient, &acct).ok();
                                        total_distributed_shells += amount_shells;

                                        // tx[1] = treasury (validator_rewards) — works for both old and new genesis
                                        if i == 1 {
                                            state_for_blocks.set_treasury_pubkey(&recipient).ok();
                                            info!(
                                                "  ✓ [network genesis] Treasury: {} ({} MOLT)",
                                                recipient.to_base58(),
                                                amount_shells / 1_000_000_000
                                            );
                                        } else {
                                            info!(
                                                "  ✓ [network genesis] Distribution {}: {} ({} MOLT)",
                                                i,
                                                recipient.to_base58(),
                                                amount_shells / 1_000_000_000
                                            );
                                        }
                                    }
                                }
                            }

                            // 5. Reconstruct genesis account (total supply minus all distributions)
                            let mut genesis_account = Account::new(total_supply_molt, gpk);
                            genesis_account.shells =
                                total_shells.saturating_sub(total_distributed_shells);
                            genesis_account.spendable = genesis_account
                                .shells
                                .saturating_sub(genesis_account.staked)
                                .saturating_sub(genesis_account.locked);
                            state_for_blocks.put_account(&gpk, &genesis_account).ok();
                            state_for_blocks.set_genesis_pubkey(&gpk).ok();
                            info!(
                                "  ✓ [network genesis] Genesis account: {} ({} MOLT remaining)",
                                gpk.to_base58(),
                                genesis_account.shells / 1_000_000_000
                            );

                            // 6. Create initial accounts from genesis config
                            for account_info in &genesis_config_for_blocks.initial_accounts {
                                if let Ok(pubkey) = Pubkey::from_base58(&account_info.address) {
                                    let account = Account::new(account_info.balance_molt, pubkey);
                                    state_for_blocks.put_account(&pubkey, &account).ok();
                                }
                            }

                            // 7. Store genesis transactions
                            for tx in &block.transactions {
                                state_for_blocks.put_transaction(tx).ok();
                            }

                            // 8. Auto-deploy contracts
                            genesis_auto_deploy(&state_for_blocks, &gpk);
                            genesis_initialize_contracts(&state_for_blocks, &gpk);
                            genesis_create_trading_pairs(&state_for_blocks, &gpk);

                            info!("✅ Applied genesis block (slot 0) from network — full state initialized");
                        } else {
                            warn!(
                                "⚠️  Genesis block has no mint tx — cannot extract genesis pubkey"
                            );
                            info!(
                                "✅ Applied genesis block (slot 0) from network (state incomplete)"
                            );
                        }

                        // Try to apply any pending blocks now that we have genesis
                        let pending = sync_mgr.try_apply_pending(0).await;
                        for pending_block in pending {
                            let pending_slot = pending_block.header.slot;
                            // Validate parent hash chain
                            let parent_ok = if pending_slot > 0 {
                                state_for_blocks
                                    .get_block_by_slot(pending_slot - 1)
                                    .ok()
                                    .flatten()
                                    .map(|parent| parent.hash() == pending_block.header.parent_hash)
                                    .unwrap_or(false)
                            } else {
                                true
                            };
                            if !parent_ok {
                                warn!("⚠️  Pending block {} parent hash mismatch after genesis, skipping", pending_slot);
                                continue;
                            }
                            replay_block_transactions(&processor_for_blocks, &pending_block);
                            if state_for_blocks.put_block(&pending_block).is_ok() {
                                state_for_blocks.set_last_slot(pending_slot).ok();
                                *last_block_time_for_blocks.lock().await =
                                    std::time::Instant::now();
                                info!("✅ Applied pending block {}", pending_slot);
                                apply_block_effects(
                                    &state_for_blocks,
                                    &validator_set_for_blocks,
                                    &stake_pool_for_blocks,
                                    &vote_agg_for_effects,
                                    &pending_block,
                                    false,
                                )
                                .await;
                            }
                        }
                    }
                } else if block_slot > current_slot {
                    // Check if this block extends our chain (parent matches our latest block)
                    if let Ok(Some(parent)) = state_for_blocks.get_block_by_slot(current_slot) {
                        if block.header.parent_hash == parent.hash() {
                            // Valid next block in chain - replay transactions then store
                            replay_block_transactions(&processor_for_blocks, &block);
                            if state_for_blocks.put_block(&block).is_ok() {
                                state_for_blocks.set_last_slot(block_slot).ok();
                                *last_block_time_for_blocks.lock().await =
                                    std::time::Instant::now();
                                info!("✅ Applied block {} from network", block_slot);
                                apply_block_effects(
                                    &state_for_blocks,
                                    &validator_set_for_blocks,
                                    &stake_pool_for_blocks,
                                    &vote_agg_for_effects,
                                    &block,
                                    false,
                                )
                                .await;

                                // Cast vote for this block (BFT consensus)
                                let block_hash = block.hash();
                                let mut vote_message = Vec::new();
                                vote_message.extend_from_slice(&block_slot.to_le_bytes());
                                vote_message.extend_from_slice(&block_hash.0);

                                // Reconstruct keypair from seed to sign vote
                                let keypair_for_vote = Keypair::from_seed(&validator_seed);
                                let signature = keypair_for_vote.sign(&vote_message);

                                let vote = Vote::new(
                                    block_slot,
                                    block_hash,
                                    validator_pubkey_for_blocks,
                                    signature,
                                );

                                // Add our own vote (validated against validator set)
                                let mut agg = vote_agg_for_blocks.lock().await;
                                let vs = validator_set_for_blocks.lock().await;
                                if agg.add_vote_validated(vote.clone(), &vs) {
                                    info!("🗳️  Cast vote for block {}", block_slot);

                                    // Check if block reached finality (2/3 supermajority - STAKE-WEIGHTED)
                                    let pool = stake_pool_for_blocks.lock().await;
                                    if agg.has_supermajority(block_slot, &block_hash, &vs, &pool) {
                                        info!("🔒 Block {} FINALIZED with stake-weighted supermajority!", block_slot);
                                    }
                                    drop(pool);
                                    drop(vs);

                                    // Broadcast vote to network
                                    let vote_msg =
                                        P2PMessage::new(MessageType::Vote(vote), local_addr);
                                    peer_mgr_for_sync.broadcast(vote_msg).await;
                                }
                                drop(agg);

                                // Try to apply any pending blocks
                                let pending = sync_mgr.try_apply_pending(block_slot).await;
                                for pending_block in pending {
                                    let pending_slot = pending_block.header.slot;
                                    // Validate parent hash before applying
                                    let parent_ok = if pending_slot > 0 {
                                        state_for_blocks
                                            .get_block_by_slot(pending_slot - 1)
                                            .ok()
                                            .flatten()
                                            .map(|parent| {
                                                parent.hash() == pending_block.header.parent_hash
                                            })
                                            .unwrap_or(false)
                                    } else {
                                        true
                                    };
                                    if !parent_ok {
                                        warn!("\u{26a0}\u{fe0f}  Pending block {} parent hash mismatch, skipping", pending_slot);
                                        continue;
                                    }
                                    replay_block_transactions(
                                        &processor_for_blocks,
                                        &pending_block,
                                    );
                                    if state_for_blocks.put_block(&pending_block).is_ok() {
                                        state_for_blocks.set_last_slot(pending_slot).ok();
                                        *last_block_time_for_blocks.lock().await =
                                            std::time::Instant::now();
                                        info!("✅ Applied pending block {}", pending_slot);
                                        apply_block_effects(
                                            &state_for_blocks,
                                            &validator_set_for_blocks,
                                            &stake_pool_for_blocks,
                                            &vote_agg_for_effects,
                                            &pending_block,
                                            false,
                                        )
                                        .await;
                                    }
                                }
                            }
                        } else {
                            // Parent doesn't match - might be on different fork or need intermediate blocks
                            warn!(
                                "⚠️  Block {} parent mismatch (expected parent of slot {})",
                                block_slot, current_slot
                            );
                            sync_mgr.add_pending_block(block).await;
                        }
                    } else {
                        // Can't find parent block
                        warn!(
                            "⚠️  Block {} is ahead (current: {}), storing as pending",
                            block_slot, current_slot
                        );
                        sync_mgr.add_pending_block(block).await;
                    }

                    // Check if we should start sync
                    if let Some((start, end)) = sync_mgr.should_sync(current_slot).await {
                        info!("🔄 Triggering sync: blocks {} to {}", start, end);

                        // Mark that we're starting sync
                        sync_mgr.start_sync(start, end).await;

                        // Send BlockRangeRequest to all peers
                        let request_msg = P2PMessage::new(
                            MessageType::BlockRangeRequest {
                                start_slot: start,
                                end_slot: end,
                            },
                            local_addr,
                        );
                        peer_mgr_for_sync.broadcast(request_msg).await;
                        info!("📡 Sent block range request: {} to {}", start, end);

                        // Mark slots as requested in sync manager
                        for slot in start..=end {
                            sync_mgr.mark_requested(slot).await;
                        }

                        // Complete sync flag after a delay (will re-trigger if still behind)
                        let sync_mgr_complete = sync_mgr.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                            sync_mgr_complete.complete_sync().await;
                        });
                    }
                } else if block_slot <= current_slot {
                    if let Ok(Some(existing)) = state_for_blocks.get_block_by_slot(block_slot) {
                        if existing.hash() != block.hash() {
                            // Fork choice: prefer block with higher vote weight,
                            // OR prefer incoming block when we're significantly behind
                            // the network (longest-chain-wins for fork resolution).
                            let highest_seen = sync_mgr.get_highest_seen().await;
                            let we_are_behind = highest_seen > current_slot + 5;

                            let existing_weight = {
                                let agg = vote_agg_for_blocks.lock().await;
                                let vs = validator_set_for_blocks.lock().await;
                                let pool = stake_pool_for_blocks.lock().await;
                                let weight = block_vote_weight(
                                    block_slot,
                                    &existing.hash(),
                                    &agg,
                                    &vs,
                                    &pool,
                                );
                                drop(pool);
                                drop(vs);
                                drop(agg);
                                weight
                            };

                            let incoming_weight = {
                                let agg = vote_agg_for_blocks.lock().await;
                                let vs = validator_set_for_blocks.lock().await;
                                let pool = stake_pool_for_blocks.lock().await;
                                let weight =
                                    block_vote_weight(block_slot, &block.hash(), &agg, &vs, &pool);
                                drop(pool);
                                drop(vs);
                                drop(agg);
                                weight
                            };

                            if incoming_weight > existing_weight || we_are_behind {
                                // Revert old block's financial effects before replacing
                                revert_block_effects(&state_for_blocks, &existing);
                                // C7 fix: Also revert user transaction effects
                                revert_block_transactions(&state_for_blocks, &existing);
                                // Replace slot index with the higher-weight block
                                replay_block_transactions(&processor_for_blocks, &block);
                                if state_for_blocks.put_block(&block).is_ok() {
                                    state_for_blocks.set_last_slot(current_slot).ok();
                                    *last_block_time_for_blocks.lock().await =
                                        std::time::Instant::now();
                                    if we_are_behind {
                                        info!(
                                            "🔗 Chain adoption: replaced block at slot {} (behind network by {} slots)",
                                            block_slot, highest_seen.saturating_sub(current_slot)
                                        );
                                    } else {
                                        info!(
                                            "⚖️  Replaced block at slot {} (weight {} -> {})",
                                            block_slot, existing_weight, incoming_weight
                                        );
                                    }
                                    apply_block_effects(
                                        &state_for_blocks,
                                        &validator_set_for_blocks,
                                        &stake_pool_for_blocks,
                                        &vote_agg_for_effects,
                                        &block,
                                        false,
                                    )
                                    .await;

                                    // After replacing a block (fork adoption), try
                                    // applying pending blocks that now chain correctly.
                                    let pending = sync_mgr.try_apply_pending(block_slot).await;
                                    for pending_block in pending {
                                        let pending_slot = pending_block.header.slot;
                                        let parent_ok = if pending_slot > 0 {
                                            state_for_blocks
                                                .get_block_by_slot(pending_slot - 1)
                                                .ok()
                                                .flatten()
                                                .map(|parent| {
                                                    parent.hash()
                                                        == pending_block.header.parent_hash
                                                })
                                                .unwrap_or(false)
                                        } else {
                                            true
                                        };
                                        if !parent_ok {
                                            warn!("⚠️  Pending block {} parent hash mismatch after fork adoption, skipping", pending_slot);
                                            continue;
                                        }
                                        replay_block_transactions(
                                            &processor_for_blocks,
                                            &pending_block,
                                        );
                                        if state_for_blocks.put_block(&pending_block).is_ok() {
                                            state_for_blocks.set_last_slot(pending_slot).ok();
                                            *last_block_time_for_blocks.lock().await =
                                                std::time::Instant::now();
                                            info!(
                                                "✅ Applied pending block {} (after fork adoption)",
                                                pending_slot
                                            );
                                            apply_block_effects(
                                                &state_for_blocks,
                                                &validator_set_for_blocks,
                                                &stake_pool_for_blocks,
                                                &vote_agg_for_effects,
                                                &pending_block,
                                                false,
                                            )
                                            .await;
                                        }
                                    }
                                }
                            } else {
                                debug!("Fork choice kept existing block at slot {}", block_slot);
                            }
                        } else {
                            debug!("Block {} already processed", block_slot);
                        }
                    }
                } else {
                    debug!("Block {} is old (current: {})", block_slot, current_slot);
                }
            }
        });

        // Start incoming transaction handler
        let mempool_for_txs = mempool.clone();
        tokio::spawn(async move {
            info!("🔄 Transaction receiver started");
            while let Some(tx) = transaction_rx.recv().await {
                info!("📥 Received transaction from P2P");
                // Add to mempool (fee calculation would be more sophisticated in production)
                let mut pool = mempool_for_txs.lock().await;
                // TODO: look up sender reputation from state for priority boost
                if let Err(e) = pool.add_transaction(tx, BASE_FEE, 0u64) {
                    info!("Mempool: {}", e);
                }
            }
        });

        // Start vote handler for BFT consensus with slashing detection
        let vote_agg_for_handler = vote_aggregator.clone();
        let validator_set_for_votes = validator_set.clone();
        let stake_pool_for_votes = stake_pool.clone();
        let slashing_for_votes = slashing_tracker.clone();
        let validator_pubkey_for_slash_report = validator_pubkey;
        let peer_mgr_for_slash = p2p_peer_manager.clone();
        let local_addr_for_slash = p2p_config.listen_addr;

        tokio::spawn(async move {
            info!("🔄 Vote receiver started");

            // Track votes per validator to detect double-voting
            let mut validator_votes: std::collections::HashMap<
                (moltchain_core::Pubkey, u64),
                Vote,
            > = std::collections::HashMap::new();

            while let Some(vote) = vote_rx.recv().await {
                // Prune old entries to prevent memory leak (keep last 100 slots)
                if validator_votes.len() > 500 {
                    let cutoff = vote.slot.saturating_sub(100);
                    validator_votes.retain(|&(_, slot), _| slot >= cutoff);
                }

                // Skip our own votes (we already counted them when we cast)
                if vote.validator == validator_pubkey_for_slash_report {
                    debug!("Skipping self-vote for block {}", vote.slot);
                    continue;
                }

                info!(
                    "📥 Received vote for block {} from {}",
                    vote.slot,
                    vote.validator.to_base58()
                );

                // Check for double-voting before adding
                let vote_key = (vote.validator, vote.slot);
                if let Some(existing_vote) = validator_votes.get(&vote_key) {
                    if existing_vote.block_hash != vote.block_hash {
                        // DOUBLE VOTE DETECTED!
                        warn!(
                            "⚔️  DOUBLE VOTE detected from {} at slot {}",
                            vote.validator.to_base58(),
                            vote.slot
                        );

                        let evidence = SlashingEvidence::new(
                            SlashingOffense::DoubleVote {
                                slot: vote.slot,
                                vote_1: existing_vote.clone(),
                                vote_2: vote.clone(),
                            },
                            vote.validator,
                            vote.slot,
                            validator_pubkey_for_slash_report,
                        );

                        // Add to slashing tracker
                        let mut slasher = slashing_for_votes.lock().await;
                        if slasher.add_evidence(evidence.clone()) {
                            info!(
                                "⚔️  Slashing evidence recorded for {}",
                                vote.validator.to_base58()
                            );

                            // Broadcast evidence to network
                            if let Some(ref peer_mgr) = peer_mgr_for_slash {
                                let evidence_msg = P2PMessage::new(
                                    MessageType::SlashingEvidence(evidence),
                                    local_addr_for_slash,
                                );
                                peer_mgr.broadcast(evidence_msg).await;
                            }
                        }
                        drop(slasher);
                        continue; // Don't add double vote
                    }
                } else {
                    // First vote from this validator at this slot
                    validator_votes.insert(vote_key, vote.clone());
                }

                let mut agg = vote_agg_for_handler.lock().await;
                let vs = validator_set_for_votes.lock().await;
                if agg.add_vote_validated(vote.clone(), &vs) {
                    // Vote added successfully, check if block reached finality
                    let pool = stake_pool_for_votes.lock().await;
                    let vote_count = agg.vote_count(vote.slot, &vote.block_hash);

                    if agg.has_supermajority(vote.slot, &vote.block_hash, &vs, &pool) {
                        info!(
                            "🔒 Block {} FINALIZED! (stake-weighted votes: {}/{})",
                            vote.slot,
                            vote_count,
                            vs.validators().len()
                        );
                    } else {
                        info!(
                            "🗳️  Vote accepted for block {} ({}/{})",
                            vote.slot,
                            vote_count,
                            vs.validators().len()
                        );
                    }
                    drop(pool);
                    drop(vs);
                } else {
                    debug!(
                        "Vote rejected for block {} (duplicate or invalid)",
                        vote.slot
                    );
                }
                drop(agg);
            }
        });

        // Start validator announcement handler
        let state_for_validators = state.clone();
        let validator_set_for_announce = validator_set.clone();
        let stake_pool_for_announce = stake_pool.clone();
        let validator_pubkey_for_announce_handler = validator_pubkey;
        tokio::spawn(async move {
            info!("🔄 Validator announcement receiver started");
            while let Some(announcement) = validator_announce_rx.recv().await {
                // Skip our own announcements
                if announcement.pubkey == validator_pubkey_for_announce_handler {
                    continue;
                }

                info!(
                    "🦞 Received validator announcement: {}",
                    announcement.pubkey.to_base58()
                );

                let mut vs = validator_set_for_announce.lock().await;

                // Cap validator set size
                const MAX_VALIDATORS: usize = 1000;

                // Check if validator already exists
                if vs.get_validator(&announcement.pubkey).is_some() {
                    // Update existing validator's activity
                    if let Some(val) = vs.get_validator_mut(&announcement.pubkey) {
                        val.last_active_slot = announcement.current_slot;
                    }
                } else {
                    // Reject if at capacity
                    if vs.validators().len() >= MAX_VALIDATORS {
                        warn!(
                            "⚠️  Validator set full ({} validators) — rejecting {}",
                            MAX_VALIDATORS,
                            announcement.pubkey.to_base58()
                        );
                        drop(vs);
                        continue;
                    }

                    // Add new validator (accept unconditionally, like committed version)
                    let new_validator = ValidatorInfo {
                        pubkey: announcement.pubkey,
                        reputation: 500,
                        blocks_proposed: 0,
                        votes_cast: 0,
                        correct_votes: 0,
                        stake: MIN_VALIDATOR_STAKE,
                        joined_slot: announcement.current_slot,
                        last_active_slot: announcement.current_slot,
                    };
                    vs.add_validator(new_validator);

                    // Also stake in local pool so leader election can pick them
                    {
                        let mut pool = stake_pool_for_announce.lock().await;
                        if pool.get_stake(&announcement.pubkey).is_none() {
                            if let Err(e) = pool.stake(
                                announcement.pubkey,
                                MIN_VALIDATOR_STAKE,
                                announcement.current_slot,
                            ) {
                                warn!(
                                    "⚠️  Failed to stake joining validator {}: {}",
                                    announcement.pubkey.to_base58(),
                                    e
                                );
                            }
                            info!(
                                "💰 Staked joining validator {} in local pool ({} MOLT)",
                                announcement.pubkey.to_base58(),
                                MIN_VALIDATOR_STAKE / 1_000_000_000
                            );
                        }
                    }

                    // Create bootstrap account for the joining validator if not present locally
                    // Must deduct from treasury — same as local bootstrap path (L2305-2312)
                    {
                        let existing_account = state_for_validators
                            .get_account(&announcement.pubkey)
                            .unwrap_or(None);
                        let needs_bootstrap = match &existing_account {
                            None => true,
                            Some(acct) => acct.staked == 0,
                        };
                        if needs_bootstrap {
                            // Deduct from treasury to avoid minting tokens ex nihilo
                            let mut funded = false;
                            if let Ok(Some(tpk)) = state_for_validators.get_treasury_pubkey() {
                                if let Ok(Some(mut treasury)) =
                                    state_for_validators.get_account(&tpk)
                                {
                                    if treasury.spendable >= MIN_VALIDATOR_STAKE {
                                        treasury.deduct_spendable(MIN_VALIDATOR_STAKE).ok();
                                        if let Err(e) =
                                            state_for_validators.put_account(&tpk, &treasury)
                                        {
                                            warn!("⚠️  Failed to debit treasury for remote bootstrap: {}", e);
                                        } else {
                                            funded = true;
                                        }
                                    } else {
                                        warn!("⚠️  Treasury insufficient for remote validator bootstrap ({} < {})",
                                            treasury.spendable, MIN_VALIDATOR_STAKE);
                                    }
                                }
                            }

                            if funded {
                                let mut bootstrap_account = Account {
                                    shells: MIN_VALIDATOR_STAKE,
                                    spendable: 0,
                                    staked: MIN_VALIDATOR_STAKE,
                                    locked: 0,
                                    data: Vec::new(),
                                    owner: SYSTEM_ACCOUNT_OWNER,
                                    executable: false,
                                    rent_epoch: 0,
                                };
                                // Preserve any existing spendable balance (from block rewards)
                                if let Some(existing) = &existing_account {
                                    bootstrap_account.shells += existing.spendable;
                                    bootstrap_account.spendable = existing.spendable;
                                }
                                if let Err(e) = state_for_validators
                                    .put_account(&announcement.pubkey, &bootstrap_account)
                                {
                                    warn!(
                                        "⚠️  Failed to create bootstrap account for {}: {}",
                                        announcement.pubkey, e
                                    );
                                } else {
                                    info!(
                                        "💰 Created bootstrap account for validator {} (10000 MOLT staked, treasury debited)",
                                        announcement.pubkey.to_base58()
                                    );
                                }
                            } else {
                                warn!("⚠️  Skipping bootstrap account for {} — treasury unavailable or insufficient",
                                    announcement.pubkey.to_base58());
                            }
                        }
                    }
                }

                // Persist to state
                if let Err(e) = state_for_validators.save_validator_set(&vs) {
                    warn!("⚠️  Failed to save validator set: {}", e);
                } else {
                    let count = vs.validators().len();
                    info!("✅ Updated validator set (now {} validators)", count);
                }
                drop(vs);
            }
        });

        // Start block range request handler
        let state_for_block_requests = state.clone();
        let peer_mgr_for_responses = p2p_pm.clone();
        let local_addr_for_responses = p2p_config.listen_addr;
        tokio::spawn(async move {
            info!("🔄 Block range request handler started");
            let mut rate_limits: HashMap<std::net::SocketAddr, (u64, std::time::Instant)> =
                HashMap::new();
            let mut strikes: HashMap<std::net::SocketAddr, u32> = HashMap::new();
            let mut last_prune = std::time::Instant::now();
            while let Some(request) = block_range_request_rx.recv().await {
                // M5 fix: Prune stale rate_limits and strikes entries every 60s
                if last_prune.elapsed().as_secs() >= 60 {
                    let cutoff = std::time::Instant::now() - std::time::Duration::from_secs(300);
                    rate_limits.retain(|_, (_, last_seen)| *last_seen > cutoff);
                    // Cap strikes map at 500 entries
                    if strikes.len() > 500 {
                        strikes.clear();
                    }
                    last_prune = std::time::Instant::now();
                }
                if !peer_mgr_for_responses
                    .get_peers()
                    .contains(&request.requester)
                {
                    warn!(
                        "⚠️  Ignoring block range request from unknown peer {}",
                        request.requester
                    );
                    peer_mgr_for_responses.record_violation(&request.requester);
                    continue;
                }

                if request.end_slot < request.start_slot {
                    warn!(
                        "⚠️  Invalid block range request {}-{} from {}",
                        request.start_slot, request.end_slot, request.requester
                    );
                    peer_mgr_for_responses.record_violation(&request.requester);
                    let count = strikes.entry(request.requester).or_insert(0);
                    *count = count.saturating_add(1);
                    if *count >= 3 {
                        warn!(
                            "⚠️  Banning peer {} — exceeded invalid request limit ({})",
                            request.requester, count
                        );
                        for _ in 0..5 {
                            peer_mgr_for_responses.record_violation(&request.requester);
                        }
                    }
                    continue;
                }

                let now = std::time::Instant::now();
                let entry = rate_limits.entry(request.requester).or_insert((0, now));
                if now.duration_since(entry.1).as_secs() >= 10 {
                    *entry = (0, now);
                }
                entry.0 = entry.0.saturating_add(1);
                if entry.0 > 5 {
                    warn!("⚠️  Rate limit exceeded for {}", request.requester);
                    peer_mgr_for_responses.record_violation(&request.requester);
                    continue;
                }

                let range_size = request.end_slot.saturating_sub(request.start_slot) + 1;

                // Rate limiting: prevent excessive requests
                if range_size > 1000 {
                    warn!(
                        "⚠️  Block range request too large: {} blocks from {}",
                        range_size, request.requester
                    );
                    peer_mgr_for_responses.record_violation(&request.requester);
                    let count = strikes.entry(request.requester).or_insert(0);
                    *count = count.saturating_add(1);
                    if *count >= 3 {
                        warn!(
                            "⚠️  Banning peer {} — exceeded invalid request limit ({})",
                            request.requester, count
                        );
                        for _ in 0..5 {
                            peer_mgr_for_responses.record_violation(&request.requester);
                        }
                    }
                    continue;
                }

                info!(
                    "📦 Processing block range request: {} to {} ({} blocks) from {}",
                    request.start_slot, request.end_slot, range_size, request.requester
                );

                // Load blocks from state (in chunks to avoid memory spike)
                let mut blocks = Vec::new();
                for slot in request.start_slot..=request.end_slot {
                    if let Ok(Some(block)) = state_for_block_requests.get_block_by_slot(slot) {
                        blocks.push(block);
                    }

                    // Limit response size to prevent memory issues
                    if blocks.len() >= 500 {
                        warn!("⚠️  Truncating block response at 500 blocks");
                        break;
                    }
                }

                if !blocks.is_empty() {
                    info!(
                        "📤 Sending {} blocks to {}",
                        blocks.len(),
                        request.requester
                    );

                    // Send BlockRangeResponse
                    let response_msg = P2PMessage::new(
                        MessageType::BlockRangeResponse { blocks },
                        local_addr_for_responses,
                    );

                    // Send to requester specifically
                    peer_mgr_for_responses
                        .send_to_peer(&request.requester, response_msg)
                        .await
                        .unwrap_or_else(|e| warn!("Failed to send block response: {}", e));
                    peer_mgr_for_responses.record_success(&request.requester);
                } else {
                    info!(
                        "⚠️  No blocks found for range {} to {}",
                        request.start_slot, request.end_slot
                    );
                }
            }
        });

        // Start status request handler
        let state_for_status = state.clone();
        let peer_mgr_for_status = p2p_pm.clone();
        let local_addr_for_status = p2p_config.listen_addr;
        tokio::spawn(async move {
            info!("🔄 Status request handler started");
            while let Some(request) = status_request_rx.recv().await {
                if !peer_mgr_for_status.get_peers().contains(&request.requester) {
                    warn!(
                        "⚠️  Ignoring status request from unknown peer {}",
                        request.requester
                    );
                    peer_mgr_for_status.record_violation(&request.requester);
                    continue;
                }
                let current_slot = state_for_status.get_last_slot().unwrap_or(0);
                let total_blocks = state_for_status.get_metrics().total_blocks;
                let response = P2PMessage::new(
                    MessageType::StatusResponse {
                        current_slot,
                        total_blocks,
                    },
                    local_addr_for_status,
                );
                if let Err(e) = peer_mgr_for_status
                    .send_to_peer(&request.requester, response)
                    .await
                {
                    warn!("⚠️  Failed to send status response: {}", e);
                    peer_mgr_for_status.record_violation(&request.requester);
                } else {
                    peer_mgr_for_status.record_success(&request.requester);
                }
            }
        });

        // Start status response handler
        let sync_mgr_for_status = sync_manager.clone();
        tokio::spawn(async move {
            while let Some(response) = status_response_rx.recv().await {
                // C5 fix: use bounded update to prevent malicious slot inflation
                // Cap at 500 slots ahead of current highest — enough for legitimate
                // sync but prevents u64::MAX attacks on fork choice.
                sync_mgr_for_status
                    .note_seen_bounded(response.current_slot, 500)
                    .await;
                debug!(
                    "📡 Peer {} reports slot {} ({} blocks)",
                    response.requester, response.current_slot, response.total_blocks
                );
            }
        });

        // Start consistency report handler
        let validator_set_for_consistency = validator_set.clone();
        let stake_pool_for_consistency = stake_pool.clone();
        let peer_mgr_for_consistency = p2p_pm.clone();
        let local_addr_for_consistency = p2p_config.listen_addr;
        tokio::spawn(async move {
            let mut last_request: HashMap<(std::net::SocketAddr, u8), std::time::Instant> =
                HashMap::new();
            while let Some(report) = consistency_report_rx.recv().await {
                let vs = validator_set_for_consistency.lock().await;
                let pool = stake_pool_for_consistency.lock().await;
                let local_vs_hash = hash_validator_set(&vs);
                let local_pool_hash = hash_stake_pool(&pool);
                drop(pool);
                drop(vs);

                if report.validator_set_hash != local_vs_hash {
                    warn!("⚠️  Validator set mismatch with {}", report.requester);
                    let key = (report.requester, 0u8);
                    let should_request = last_request
                        .get(&key)
                        .map(|instant| instant.elapsed().as_secs() >= 30)
                        .unwrap_or(true);
                    if should_request {
                        let request = P2PMessage::new(
                            MessageType::SnapshotRequest {
                                kind: SnapshotKind::ValidatorSet,
                            },
                            local_addr_for_consistency,
                        );
                        if let Err(e) = peer_mgr_for_consistency
                            .send_to_peer(&report.requester, request)
                            .await
                        {
                            warn!("⚠️  Failed to request validator set snapshot: {}", e);
                            peer_mgr_for_consistency.record_violation(&report.requester);
                        } else {
                            last_request.insert(key, std::time::Instant::now());
                        }
                    }
                }
                if report.stake_pool_hash != local_pool_hash {
                    warn!("⚠️  Stake pool mismatch with {}", report.requester);
                    let key = (report.requester, 1u8);
                    let should_request = last_request
                        .get(&key)
                        .map(|instant| instant.elapsed().as_secs() >= 30)
                        .unwrap_or(true);
                    if should_request {
                        let request = P2PMessage::new(
                            MessageType::SnapshotRequest {
                                kind: SnapshotKind::StakePool,
                            },
                            local_addr_for_consistency,
                        );
                        if let Err(e) = peer_mgr_for_consistency
                            .send_to_peer(&report.requester, request)
                            .await
                        {
                            warn!("⚠️  Failed to request stake pool snapshot: {}", e);
                            peer_mgr_for_consistency.record_violation(&report.requester);
                        } else {
                            last_request.insert(key, std::time::Instant::now());
                        }
                    }
                }
            }
        });

        // Start snapshot request handler
        let validator_set_for_snapshot = validator_set.clone();
        let stake_pool_for_snapshot = stake_pool.clone();
        let peer_mgr_for_snapshot = p2p_pm.clone();
        let local_addr_for_snapshot = p2p_config.listen_addr;
        tokio::spawn(async move {
            info!("🔄 Snapshot request handler started");
            while let Some(request) = snapshot_request_rx.recv().await {
                if !peer_mgr_for_snapshot
                    .get_peers()
                    .contains(&request.requester)
                {
                    warn!(
                        "⚠️  Ignoring snapshot request from unknown peer {}",
                        request.requester
                    );
                    peer_mgr_for_snapshot.record_violation(&request.requester);
                    continue;
                }

                let response = match request.kind {
                    SnapshotKind::ValidatorSet => {
                        let vs = validator_set_for_snapshot.lock().await;
                        P2PMessage::new(
                            MessageType::SnapshotResponse {
                                kind: SnapshotKind::ValidatorSet,
                                validator_set: Some(vs.clone()),
                                stake_pool: None,
                            },
                            local_addr_for_snapshot,
                        )
                    }
                    SnapshotKind::StakePool => {
                        let pool = stake_pool_for_snapshot.lock().await;
                        P2PMessage::new(
                            MessageType::SnapshotResponse {
                                kind: SnapshotKind::StakePool,
                                validator_set: None,
                                stake_pool: Some(pool.clone()),
                            },
                            local_addr_for_snapshot,
                        )
                    }
                };

                if let Err(e) = peer_mgr_for_snapshot
                    .send_to_peer(&request.requester, response)
                    .await
                {
                    warn!("⚠️  Failed to send snapshot response: {}", e);
                    peer_mgr_for_snapshot.record_violation(&request.requester);
                } else {
                    peer_mgr_for_snapshot.record_success(&request.requester);
                }
            }
        });

        // Start snapshot response handler
        let state_for_snapshot_apply = state.clone();
        let validator_set_for_snapshot_apply = validator_set.clone();
        let stake_pool_for_snapshot_apply = stake_pool.clone();
        let snapshot_sync_for_apply = snapshot_sync.clone();
        tokio::spawn(async move {
            while let Some(response) = snapshot_response_rx.recv().await {
                match response.kind {
                    SnapshotKind::ValidatorSet => {
                        if let Some(remote_set) = response.validator_set {
                            if remote_set.validators().is_empty() {
                                warn!(
                                    "⚠️  Ignoring empty validator set snapshot from {}",
                                    response.requester
                                );
                                continue;
                            }

                            let remote_hash = hash_validator_set(&remote_set);

                            let mut vs = validator_set_for_snapshot_apply.lock().await;
                            let local_hash = hash_validator_set(&vs);

                            if remote_hash != local_hash {
                                // T2.9 fix: MERGE remote validators into local set
                                // instead of full replacement. This prevents a single
                                // malicious peer from removing legitimate validators.
                                let mut merged_count = 0u32;
                                for remote_val in remote_set.validators() {
                                    if let Some(local_val) =
                                        vs.get_validator_mut(&remote_val.pubkey)
                                    {
                                        // Update existing: prefer higher stats
                                        if remote_val.blocks_proposed > local_val.blocks_proposed {
                                            local_val.blocks_proposed = remote_val.blocks_proposed;
                                        }
                                        if remote_val.last_active_slot > local_val.last_active_slot
                                        {
                                            local_val.last_active_slot =
                                                remote_val.last_active_slot;
                                            local_val.stake = remote_val.stake;
                                        }
                                        merged_count += 1;
                                    } else {
                                        // Add new validator from remote
                                        vs.add_validator(remote_val.clone());
                                        merged_count += 1;
                                    }
                                }
                                let merged_set = vs.clone();
                                // Save while still holding the lock to prevent
                                // apply_block_effects from saving a newer version
                                // that we'd then overwrite with this stale clone.
                                if let Err(e) =
                                    state_for_snapshot_apply.save_validator_set(&merged_set)
                                {
                                    warn!("⚠️  Failed to persist merged validator set: {}", e);
                                } else {
                                    info!(
                                        "✅ Merged validator set snapshot from {} ({} entries merged)",
                                        response.requester,
                                        merged_count
                                    );
                                    snapshot_sync_for_apply.lock().await.validator_set = true;
                                }
                                drop(vs);
                            }
                        }
                    }
                    SnapshotKind::StakePool => {
                        if let Some(remote_pool) = response.stake_pool {
                            if remote_pool.stake_entries().is_empty() {
                                warn!(
                                    "⚠️  Ignoring empty stake pool snapshot from {}",
                                    response.requester
                                );
                                continue;
                            }

                            // MERGE remote entries into local pool instead of replacing
                            let mut pool = stake_pool_for_snapshot_apply.lock().await;
                            let local_hash = hash_stake_pool(&pool);
                            let mut merged_count = 0u32;
                            for entry in remote_pool.stake_entries() {
                                let existing = pool.get_stake(&entry.validator);
                                let should_upsert = match existing {
                                    None => true,
                                    Some(local_entry) => entry.amount > local_entry.amount,
                                };
                                if should_upsert {
                                    pool.upsert_stake(
                                        entry.validator,
                                        entry.amount,
                                        entry.last_reward_slot,
                                    );
                                    merged_count += 1;

                                    // Create bootstrap account for this validator if it doesn't exist locally
                                    // This ensures V1 knows about V2/V3's staked accounts (and vice versa)
                                    let existing_account = state_for_snapshot_apply
                                        .get_account(&entry.validator)
                                        .unwrap_or(None);
                                    let needs_bootstrap = match &existing_account {
                                        None => true,
                                        Some(acct) => {
                                            acct.staked == 0 && entry.amount >= MIN_VALIDATOR_STAKE
                                        }
                                    };
                                    if needs_bootstrap {
                                        // Deduct from treasury — same as announce handler
                                        let mut funded = false;
                                        if let Ok(Some(tpk)) =
                                            state_for_snapshot_apply.get_treasury_pubkey()
                                        {
                                            if let Ok(Some(mut treasury)) =
                                                state_for_snapshot_apply.get_account(&tpk)
                                            {
                                                if treasury.spendable >= entry.amount {
                                                    treasury.deduct_spendable(entry.amount).ok();
                                                    if let Err(e) = state_for_snapshot_apply
                                                        .put_account(&tpk, &treasury)
                                                    {
                                                        warn!("⚠️  Failed to debit treasury for snapshot bootstrap: {}", e);
                                                    } else {
                                                        funded = true;
                                                    }
                                                }
                                            }
                                        }

                                        if funded {
                                            // Construct account directly with staked amount in shells
                                            // (avoids MOLT<->shells rounding issues)
                                            let mut bootstrap_account = Account {
                                                shells: entry.amount,
                                                spendable: 0,
                                                staked: entry.amount,
                                                locked: 0,
                                                data: Vec::new(),
                                                owner: SYSTEM_ACCOUNT_OWNER,
                                                executable: false,
                                                rent_epoch: 0,
                                            };
                                            // Preserve any existing spendable balance (from block rewards)
                                            if let Some(existing) = &existing_account {
                                                bootstrap_account.shells += existing.spendable;
                                                bootstrap_account.spendable = existing.spendable;
                                            }
                                            if let Err(e) = state_for_snapshot_apply
                                                .put_account(&entry.validator, &bootstrap_account)
                                            {
                                                warn!("⚠️  Failed to create bootstrap account for {}: {}", entry.validator, e);
                                            } else {
                                                info!(
                                                "💰 Created bootstrap account for validator {} ({:.4} MOLT staked, treasury debited)",
                                                entry.validator,
                                                entry.amount as f64 / 1_000_000_000.0
                                            );
                                            }
                                        } else {
                                            warn!("⚠️  Insufficient treasury to bootstrap validator {} from snapshot ({:.4} MOLT needed)",
                                                entry.validator, entry.amount as f64 / 1_000_000_000.0);
                                        }
                                    }
                                }
                            }
                            let merged_hash = hash_stake_pool(&pool);
                            if merged_hash != local_hash {
                                let merged_pool = pool.clone();
                                drop(pool);
                                if let Err(e) =
                                    state_for_snapshot_apply.put_stake_pool(&merged_pool)
                                {
                                    warn!("⚠️  Failed to persist merged stake pool: {}", e);
                                } else {
                                    info!(
                                        "✅ Merged {} stake entries from {} ({} -> {})",
                                        merged_count,
                                        response.requester,
                                        local_hash.to_hex(),
                                        merged_hash.to_hex()
                                    );
                                    snapshot_sync_for_apply.lock().await.stake_pool = true;
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    // RPC SERVER SETUP
    info!("🦞 Starting RPC server...");

    // Parse --rpc-port and --ws-port from CLI, or derive from P2P port
    // Use safe arithmetic: offset = p2p_port % 1000 to avoid underflow/overflow
    let rpc_port = args
        .iter()
        .position(|arg| arg == "--rpc-port")
        .and_then(|pos| args.get(pos + 1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| {
            if p2p_port == 8000 {
                8899
            } else {
                let offset = p2p_port % 1000;
                8900u16
                    .saturating_add(offset.saturating_mul(2))
                    .saturating_add(1)
            }
        });

    let ws_port = args
        .iter()
        .position(|arg| arg == "--ws-port")
        .and_then(|pos| args.get(pos + 1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| {
            if p2p_port == 8000 {
                8900
            } else {
                let offset = p2p_port % 1000;
                8900u16
                    .saturating_add(offset.saturating_mul(2))
                    .saturating_add(2)
            }
        });

    // Parse --admin-token from CLI or MOLTCHAIN_ADMIN_TOKEN env var
    let admin_token: Option<String> = args
        .iter()
        .position(|arg| arg == "--admin-token")
        .and_then(|pos| args.get(pos + 1))
        .map(|s| s.to_string())
        .or_else(|| env::var("MOLTCHAIN_ADMIN_TOKEN").ok())
        .filter(|t| !t.is_empty());
    if admin_token.is_some() {
        info!("🔒 Admin token configured for state-mutating RPC endpoints");
    }

    let state_for_rpc = state.clone();
    let state_for_ws = state.clone();
    let stake_pool_for_rpc = Some(stake_pool.clone());
    let chain_id_for_rpc = genesis_config.chain_id.clone();
    let network_id_for_rpc = genesis_config.chain_id.clone();

    // Create transaction submission channel for RPC -> mempool (bounded: backpressure returns HTTP 503)
    let (rpc_tx_sender, mut rpc_tx_receiver) = mpsc::channel::<Transaction>(1_000);

    // Forward RPC transactions to P2P network and mempool
    let mempool_for_rpc_txs = mempool.clone();
    let p2p_peer_manager_for_txs = p2p_peer_manager.clone();
    let p2p_config_for_txs = p2p_config.clone();
    tokio::spawn(async move {
        while let Some(tx) = rpc_tx_receiver.recv().await {
            info!("📨 RPC transaction received, adding to mempool");

            // Add to mempool
            {
                let mut pool = mempool_for_rpc_txs.lock().await;
                // TODO: look up sender reputation from state for priority boost
                if let Err(e) = pool.add_transaction(tx.clone(), BASE_FEE, 0u64) {
                    info!("Mempool add failed: {}", e);
                }
            }

            // Broadcast to P2P network
            if let Some(ref peer_mgr) = p2p_peer_manager_for_txs {
                let msg = moltchain_p2p::P2PMessage::new(
                    moltchain_p2p::MessageType::Transaction(tx),
                    p2p_config_for_txs.listen_addr,
                );
                peer_mgr.broadcast(msg).await;
                info!("📡 Broadcasted transaction to network");
            }
        }
    });

    let tx_sender_for_rpc = Some(rpc_tx_sender);
    let p2p_for_rpc: Option<Arc<dyn moltchain_rpc::P2PNetworkTrait>> =
        p2p_peer_manager.as_ref().map(|peer_mgr| {
            struct PeerAdapter {
                peer_mgr: Arc<moltchain_p2p::PeerManager>,
            }

            impl moltchain_rpc::P2PNetworkTrait for PeerAdapter {
                fn peer_count(&self) -> usize {
                    self.peer_mgr.get_peers().len()
                }

                fn peer_addresses(&self) -> Vec<String> {
                    self.peer_mgr
                        .get_peers()
                        .into_iter()
                        .map(|addr| addr.to_string())
                        .collect()
                }
            }

            Arc::new(PeerAdapter {
                peer_mgr: peer_mgr.clone(),
            }) as Arc<dyn moltchain_rpc::P2PNetworkTrait>
        });

    // Start RPC server
    tokio::spawn(async move {
        if let Err(e) = start_rpc_server(
            state_for_rpc,
            rpc_port,
            tx_sender_for_rpc,
            stake_pool_for_rpc,
            p2p_for_rpc,
            chain_id_for_rpc,
            network_id_for_rpc,
            admin_token,
        )
        .await
        {
            error!("RPC server error: {}", e);
        }
    });
    info!("✅ RPC server starting on http://0.0.0.0:{}", rpc_port);

    // Start WebSocket server and get event broadcaster
    let (ws_event_tx, _ws_handle) =
        match moltchain_rpc::start_ws_server(state_for_ws, ws_port).await {
            Ok(result) => {
                info!("✅ WebSocket server starting on ws://0.0.0.0:{}", ws_port);
                result
            }
            Err(e) => {
                error!(
                    "Failed to start WebSocket server: {} — continuing without WebSocket",
                    e
                );
                // Create a dummy broadcast channel so the rest of the code can send events
                // without checking — receivers simply don't exist.
                let (dummy_tx, _) = tokio::sync::broadcast::channel::<moltchain_rpc::ws::Event>(1);
                let dummy_handle = tokio::spawn(async {});
                (dummy_tx, dummy_handle)
            }
        };

    info!("⚡ Starting consensus-based block production");
    info!("Validator: {}", validator_pubkey);
    info!(
        "Block time: {}ms",
        genesis_config.consensus.slot_duration_ms
    );
    info!(
        "Base fee: {} shells ({:.5} MOLT)",
        BASE_FEE,
        BASE_FEE as f64 / 1_000_000_000.0
    );
    info!("Fee split: 50% burned, 30% producer, 10% voters, 10% treasury");
    info!("Leader selection: stake + contribution weighted");

    if let Some(ref p2p_pm) = p2p_peer_manager {
        info!("🌐 Multi-validator mode: Broadcasting blocks to peers");

        // Broadcast validator announcement periodically for network discovery
        let peer_mgr_for_announce = p2p_pm.clone();
        let local_addr = p2p_config.listen_addr;
        let validator_pubkey_for_announce = validator_pubkey;
        let stake_pool_for_announce = stake_pool.clone();
        let state_for_announce = state.clone();
        let validator_seed_for_announce = validator_keypair.to_seed();
        tokio::spawn(async move {
            // Wait for initial peer connections
            time::sleep(Duration::from_secs(2)).await;

            // Announce periodically so new validators can discover us
            let mut interval = time::interval(Duration::from_secs(30));
            loop {
                let validator_stake = {
                    let pool = stake_pool_for_announce.lock().await;
                    pool.get_stake(&validator_pubkey_for_announce)
                        .map(|s| s.total_stake())
                        .unwrap_or(MIN_VALIDATOR_STAKE)
                };
                let current_slot = state_for_announce.get_last_slot().unwrap_or(0);

                // T2.3 fix: Sign announcement with validator keypair
                let announce_keypair = Keypair::from_seed(&validator_seed_for_announce);
                let mut sign_message = Vec::with_capacity(48);
                sign_message.extend_from_slice(&validator_pubkey_for_announce.0);
                sign_message.extend_from_slice(&validator_stake.to_le_bytes());
                sign_message.extend_from_slice(&current_slot.to_le_bytes());
                let signature = announce_keypair.sign(&sign_message);

                let announce_msg = P2PMessage::new(
                    MessageType::ValidatorAnnounce {
                        pubkey: validator_pubkey_for_announce,
                        stake: validator_stake,
                        current_slot,
                        version: updater::VERSION.to_string(),
                        signature,
                    },
                    local_addr,
                );

                interval.tick().await;

                peer_mgr_for_announce.broadcast(announce_msg).await;
                info!(
                    "📣 Broadcasted signed validator announcement: {} (stake: {}, slot: {})",
                    validator_pubkey_for_announce.to_base58(),
                    validator_stake,
                    current_slot
                );
            }
        });

        // Broadcast consistency report periodically
        let peer_mgr_for_report = p2p_pm.clone();
        let local_addr_for_report = p2p_config.listen_addr;
        let validator_set_for_report = validator_set.clone();
        let stake_pool_for_report = stake_pool.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                let vs = validator_set_for_report.lock().await;
                let pool = stake_pool_for_report.lock().await;
                let vs_hash = hash_validator_set(&vs);
                let pool_hash = hash_stake_pool(&pool);
                drop(pool);
                drop(vs);

                let report = P2PMessage::new(
                    MessageType::ConsistencyReport {
                        validator_set_hash: vs_hash,
                        stake_pool_hash: pool_hash,
                    },
                    local_addr_for_report,
                );
                peer_mgr_for_report.broadcast(report).await;
            }
        });
    } else {
        info!("🔒 Single-validator mode: No P2P network");
    }

    // Periodic mempool cleanup
    let mempool_for_cleanup = mempool.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let mut pool = mempool_for_cleanup.lock().await;
            pool.cleanup_expired();
            info!("🧹 Mempool cleaned (size: {})", pool.size());
        }
    });

    // Periodic vote aggregator cleanup (keep last 100 slots)
    let vote_agg_for_cleanup = vote_aggregator.clone();
    let state_for_vote_cleanup = state.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let current_slot = state_for_vote_cleanup.get_last_slot().unwrap_or(0);
            let mut agg = vote_agg_for_cleanup.lock().await;
            agg.prune_old_votes(current_slot, 100);
        }
    });

    // Periodic validator set + stake pool reconciliation from state
    let validator_set_for_reconcile = validator_set.clone();
    let stake_pool_for_reconcile = stake_pool.clone();
    let state_for_reconcile = state.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Ok(loaded_set) = state_for_reconcile.load_validator_set() {
                let mut vs = validator_set_for_reconcile.lock().await;
                if hash_validator_set(&vs) != hash_validator_set(&loaded_set) {
                    *vs = loaded_set;
                    info!("🔄 Validator set reconciled from state");
                }
            }

            if let Ok(loaded_pool) = state_for_reconcile.get_stake_pool() {
                let mut pool = stake_pool_for_reconcile.lock().await;
                if hash_stake_pool(&pool) != hash_stake_pool(&loaded_pool) {
                    // Merge loaded entries into in-memory pool (don't replace)
                    for entry in loaded_pool.stake_entries() {
                        let existing = pool.get_stake(&entry.validator);
                        let should_upsert = match existing {
                            None => true,
                            Some(local) => entry.amount > local.amount,
                        };
                        if should_upsert {
                            pool.upsert_stake(
                                entry.validator,
                                entry.amount,
                                entry.last_reward_slot,
                            );
                        }
                    }
                    info!("🔄 Stake pool reconciled from state");
                }
            }
        }
    });

    // Periodic reward stats reporting (every 120s)
    let stake_pool_for_rewards = stake_pool.clone();
    let validator_pubkey_for_rewards = validator_pubkey;

    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(120));
        loop {
            interval.tick().await;

            let pool = stake_pool_for_rewards.lock().await;

            // Check accumulated rewards
            if let Some(stake_info) = pool.get_stake(&validator_pubkey_for_rewards) {
                let unclaimed = stake_info.rewards_earned;
                if unclaimed > 0 {
                    let vesting_progress = stake_info.vesting_progress();
                    let is_bootstrapping = !stake_info.is_fully_vested();

                    info!(
                        "💰 Accumulated rewards: {:.3} MOLT (unclaimed)",
                        unclaimed as f64 / 1_000_000_000.0
                    );

                    if is_bootstrapping {
                        info!(
                            "🦞 Contributory Stake: {}% vested ({} blocks produced)",
                            vesting_progress, stake_info.blocks_produced
                        );
                    }
                }
            }

            // Report staking statistics
            let stats = pool.get_stats();
            info!(
                "📊 Staking Stats | Total: {:.2} MOLT | Validators: {} | Unclaimed: {:.3} MOLT",
                stats.total_staked as f64 / 1_000_000_000.0,
                stats.active_validators,
                stats.total_unclaimed_rewards as f64 / 1_000_000_000.0
            );

            drop(pool);
        }
    });

    // Periodic ban list cleanup
    if let Some(ref peer_mgr) = p2p_peer_manager {
        let peer_mgr_for_ban = peer_mgr.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(120));
            loop {
                interval.tick().await;
                peer_mgr_for_ban.prune_ban_list();
            }
        });
    }

    // Periodic downtime detection and slashing (check every 60s)
    let validator_set_for_downtime = validator_set.clone();
    let slashing_for_downtime = slashing_tracker.clone();
    let state_for_downtime = state.clone();
    let validator_pubkey_for_downtime = validator_pubkey;
    let peer_mgr_for_downtime_slash = p2p_peer_manager.clone();
    let local_addr_for_downtime = p2p_config.listen_addr;

    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let current_slot = state_for_downtime.get_last_slot().unwrap_or(0);

            // Check all validators for downtime (offline for 100+ slots)
            let vs = validator_set_for_downtime.lock().await;
            for validator_info in vs.validators() {
                let missed_slots = current_slot.saturating_sub(validator_info.last_active_slot);

                // Slash if offline for 100+ slots (~40 seconds at 400ms/slot)
                if missed_slots >= 100 && validator_info.pubkey != validator_pubkey_for_downtime {
                    info!(
                        "⚔️  Validator {} offline for {} slots",
                        validator_info.pubkey.to_base58(),
                        missed_slots
                    );

                    let evidence = SlashingEvidence::new(
                        SlashingOffense::Downtime {
                            last_active_slot: validator_info.last_active_slot,
                            current_slot,
                            missed_slots,
                        },
                        validator_info.pubkey,
                        current_slot,
                        validator_pubkey_for_downtime,
                    );

                    let mut slasher = slashing_for_downtime.lock().await;
                    if slasher.add_evidence(evidence.clone()) {
                        info!(
                            "⚔️  Downtime evidence recorded for {}",
                            validator_info.pubkey.to_base58()
                        );

                        // Broadcast evidence
                        if let Some(ref peer_mgr) = peer_mgr_for_downtime_slash {
                            let evidence_msg = P2PMessage::new(
                                MessageType::SlashingEvidence(evidence),
                                local_addr_for_downtime,
                            );
                            peer_mgr.broadcast(evidence_msg).await;
                        }
                    }
                    drop(slasher);
                }
            }
            drop(vs);

            // Cleanup old evidence
            let mut slasher = slashing_for_downtime.lock().await;
            slasher.prune_old_evidence(current_slot, 1000);
        }
    });

    // Process slashing evidence received from P2P peers
    {
        let slashing_for_evidence = slashing_tracker.clone();
        tokio::spawn(async move {
            while let Some(evidence) = slashing_evidence_rx.recv().await {
                info!(
                    "⚔️  Received slashing evidence from P2P: {:?} for validator {}",
                    evidence.offense,
                    evidence.validator.to_base58()
                );
                let mut slasher = slashing_for_evidence.lock().await;
                if slasher.add_evidence(evidence.clone()) {
                    info!(
                        "⚔️  Evidence recorded for {}",
                        evidence.validator.to_base58()
                    );
                    if slasher.should_slash(&evidence.validator) {
                        slasher.slash(&evidence.validator);
                        info!(
                            "⚔️  Validator {} marked as slashed",
                            evidence.validator.to_base58()
                        );
                    }
                } else {
                    debug!(
                        "Duplicate or invalid evidence for {}",
                        evidence.validator.to_base58()
                    );
                }
            }
        });
    }

    // ── Internal health watchdog ──────────────────────────────────────
    // Monitors last_block_time.  If no block is produced or received for
    // watchdog_timeout seconds, the validator is likely deadlocked.
    // Exit with EXIT_CODE_RESTART so the supervisor can relaunch us.
    let watchdog_timeout_secs = args
        .iter()
        .position(|a| a == "--watchdog-timeout")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_WATCHDOG_TIMEOUT_SECS);

    let last_block_time_for_watchdog = last_block_time.clone();
    let state_for_watchdog = state.clone();
    tokio::spawn(async move {
        // Give the validator time to start up and sync before monitoring
        time::sleep(Duration::from_secs(watchdog_timeout_secs.max(60))).await;
        let mut interval = time::interval(Duration::from_secs(15));
        let mut stale_checks: u32 = 0;
        let threshold = (watchdog_timeout_secs / 15).max(4) as u32;
        let mut last_known_slot: u64 = 0;
        loop {
            interval.tick().await;
            let elapsed = last_block_time_for_watchdog.lock().await.elapsed();
            let current_slot = state_for_watchdog.get_last_slot().unwrap_or(0);

            if elapsed > Duration::from_secs(watchdog_timeout_secs)
                && current_slot == last_known_slot
            {
                stale_checks += 1;
                warn!(
                    "🐺 Watchdog: no block activity for {:.0}s (stale {}/{})",
                    elapsed.as_secs_f64(),
                    stale_checks,
                    threshold
                );
                if stale_checks >= threshold {
                    error!(
                        "🐺 Watchdog: validator stalled for {}s — triggering restart (exit {})",
                        elapsed.as_secs(),
                        EXIT_CODE_RESTART
                    );
                    std::process::exit(EXIT_CODE_RESTART);
                }
            } else {
                if stale_checks > 0 {
                    info!("🐺 Watchdog: activity resumed (slot {})", current_slot);
                }
                stale_checks = 0;
                last_known_slot = current_slot;
            }
        }
    });

    // Track when we first discovered other validators (for stabilization wait)
    let mut first_announcement_time: Option<std::time::Instant> = None;
    let validator_set_stabilization = if !explicit_seed_peers.is_empty()
        && explicit_seed_peers
            .iter()
            .all(|addr| addr.ip().is_loopback())
    {
        Duration::from_secs(10)
    } else {
        Duration::from_secs(60)
    };

    // Adaptive heartbeat: Track last time we had activity (transaction block or heartbeat)
    let mut last_activity_time = std::time::Instant::now();

    loop {
        time::sleep(Duration::from_millis(slot_duration_ms)).await;

        // Broadcast slot event to WebSocket subscribers
        let _ = ws_event_tx.send(moltchain_rpc::ws::Event::Slot(slot));

        // Check if we need to wait for initial sync and validator discovery
        if is_joining_network {
            let has_genesis = state.get_block_by_slot(0).unwrap_or(None).is_some();
            if !has_genesis {
                // Still waiting for genesis sync
                if slot % 50 == 0 {
                    info!("⏳ Waiting for genesis sync from network (slot {})", slot);
                }
                slot += 1;
                continue;
            }

            let snapshot_ready = snapshot_sync.lock().await.is_ready();
            if !snapshot_ready {
                if slot % 50 == 0 {
                    info!(
                        "⏳ Waiting for validator/stake snapshots before producing (slot {})",
                        slot
                    );
                }
                slot += 1;
                continue;
            } else {
                // Genesis synced! But wait for validator discovery AND full chain sync
                let vs = validator_set.lock().await;
                let validator_count = vs.validators().len();
                drop(vs);

                if validator_count <= 1 {
                    // Still waiting for first validator announcement
                    if slot % 50 == 0 {
                        info!(
                            "⏳ Waiting for validator discovery (found {} validators)",
                            validator_count
                        );
                    }
                    slot += 1;
                    continue;
                } else if first_announcement_time.is_none() {
                    // Just discovered validators! Start stabilization wait
                    first_announcement_time = Some(std::time::Instant::now());
                    info!(
                        "✅ Discovered {} validators. Waiting {}s for ValidatorSet stability...",
                        validator_count,
                        validator_set_stabilization.as_secs()
                    );
                    slot += 1;
                    continue;
                } else {
                    // Check if we've waited long enough for ValidatorSet to stabilize
                    let elapsed = first_announcement_time
                        .map(|t| t.elapsed())
                        .unwrap_or_default();
                    if elapsed < validator_set_stabilization {
                        if slot % 50 == 0 {
                            info!(
                                "⏳ ValidatorSet stabilizing... ({:.0}s / {}s, {} validators)",
                                elapsed.as_secs(),
                                validator_set_stabilization.as_secs(),
                                validator_count
                            );
                        }
                        slot += 1;
                        continue;
                    }
                }

                // ValidatorSet stable! Now wait until caught up with network
                let current_slot = state.get_last_slot().unwrap_or(0);
                if !sync_manager.is_caught_up(current_slot).await {
                    let network_slot = sync_manager.get_highest_seen().await;
                    if slot % 50 == 0 {
                        info!(
                            "⏳ Syncing to network (current: {}, network: {}, {} validators)",
                            current_slot, network_slot, validator_count
                        );
                    }
                    slot += 1;
                    continue;
                }

                // Fully synced! Reset slot to continue from where network is
                slot = current_slot + 1;
                info!(
                    "✅ READY! Found {} validators, fully synced. Starting consensus from slot {}",
                    validator_count, slot
                );
                is_joining_network = false; // Exit joining mode - we're caught up!
            }
        }

        // Check if we already have a block for this slot (received from P2P)
        if let Ok(Some(_existing_block)) = state.get_block_by_slot(slot) {
            // Already have a block for this slot, skip production
            slot += 1;
            continue;
        }

        // Apply slashing penalties if any validators should be slashed
        {
            let mut slasher = slashing_tracker.lock().await;
            // Lock ordering: validator_set before stake_pool (matches global convention
            // used by announcement handler, vote handlers, leader election, etc.)
            let mut vs = validator_set.lock().await;
            let mut pool = stake_pool.lock().await;

            for validator_info in vs.validators_mut() {
                if slasher.should_slash(&validator_info.pubkey)
                    && !slasher.is_slashed(&validator_info.pubkey)
                {
                    // Apply ECONOMIC slashing - slash actual stake
                    let slashed_amount =
                        slasher.apply_economic_slashing(&validator_info.pubkey, &mut pool);

                    // Also apply reputation penalty
                    let reputation_penalty = slasher.calculate_penalty(&validator_info.pubkey);
                    let old_reputation = validator_info.reputation;
                    validator_info.reputation =
                        validator_info.reputation.saturating_sub(reputation_penalty);

                    if slashed_amount > 0 {
                        warn!(
                            "⚔️💰 SLASHED {} | Stake burned: {} MOLT | Reputation: {} -> {}",
                            validator_info.pubkey.to_base58(),
                            slashed_amount / 1_000_000_000,
                            old_reputation,
                            validator_info.reputation
                        );

                        // AUDIT-FIX 0.4: Persist slashing to the validator's Account
                        // Debit staked amount from the on-chain account so restarts
                        // don't restore the slashed stake.
                        if let Ok(Some(mut acct)) = state.get_account(&validator_info.pubkey) {
                            let debit = slashed_amount.min(acct.staked);
                            acct.staked = acct.staked.saturating_sub(debit);
                            acct.shells = acct.shells.saturating_sub(debit);
                            if let Err(e) = state.put_account(&validator_info.pubkey, &acct) {
                                error!("Failed to persist slashed account: {}", e);
                            }
                        }
                    }
                }
            }

            // AUDIT-FIX 0.4: Persist stake pool and validator set after slashing
            // so that slashing effects survive node restarts.
            if let Err(e) = state.put_stake_pool(&pool) {
                error!("Failed to persist stake pool after slashing: {}", e);
            }
            if let Err(e) = state.save_validator_set(&vs) {
                error!("Failed to persist validator set after slashing: {}", e);
            }

            drop(vs);
            drop(pool);
        }

        // In-slot view-change: rotate leader if no blocks for too long
        let elapsed = last_block_time_for_local.lock().await.elapsed();
        let view_ms = view_timeout.as_millis().max(1);
        let view = (elapsed.as_millis() / view_ms) as u64;

        // Stake-weighted leader election with deterministic fallback
        let vs = validator_set.lock().await;
        let pool = stake_pool.lock().await;
        let leader_slot = slot.saturating_add(view);
        let leader = vs.select_leader_weighted(leader_slot, &pool);
        let should_produce = leader
            .map(|pubkey| pubkey == validator_pubkey)
            .unwrap_or(false);
        drop(pool);
        drop(vs);

        if !should_produce {
            // Not our turn, wait for view-change or leader block
            continue;
        }

        // Update parent_hash from actual latest block (in case chain was synced from P2P)
        // M7 fix: Ensure slot monotonicity — never produce behind chain head
        let current_slot_check = state.get_last_slot().unwrap_or(0);
        if slot <= current_slot_check {
            slot = current_slot_check + 1;
            warn!(
                "⚠️  Slot adjusted to {} (was behind chain head {})",
                slot, current_slot_check
            );
        }
        if current_slot_check > 0 {
            if let Ok(Some(latest_block)) = state.get_block_by_slot(current_slot_check) {
                parent_hash = latest_block.hash();
            }
        } else if current_slot_check == 0 {
            // We have genesis, use it as parent
            if let Ok(Some(genesis_block)) = state.get_block_by_slot(0) {
                parent_hash = genesis_block.hash();
            }
        }

        // Collect pending transactions from mempool
        let pending_transactions = {
            let mut pool = mempool.lock().await;
            pool.get_top_transactions(10) // Get up to 10 highest-priority transactions
        };

        // Process transactions before inclusion (fees, balances, persistence)
        let mut transactions: Vec<Transaction> = Vec::new();
        let mut processed_hashes: Vec<Hash> = Vec::new();
        for tx in pending_transactions {
            processed_hashes.push(tx.hash());
            let result = processor.process_transaction(&tx, &validator_pubkey);
            if result.success {
                transactions.push(tx);
            } else {
                warn!(
                    "⚠️  Dropping transaction {}: {}",
                    tx.signature().to_hex(),
                    result.error.unwrap_or_else(|| "Unknown error".to_string())
                );
            }
        }

        // ADAPTIVE HEARTBEAT: Skip block if mempool empty and not heartbeat time
        let has_user_transactions = !transactions.is_empty();
        let is_heartbeat_time = last_activity_time.elapsed() >= Duration::from_secs(5);

        if !has_user_transactions && !is_heartbeat_time {
            // Skip this block - no transactions and not time for heartbeat
            // NOTE: We do NOT increment slot here - slot only advances when blocks are produced
            continue;
        }

        let is_heartbeat = !has_user_transactions;

        // Update activity tracking - reset timer after producing block
        last_activity_time = std::time::Instant::now();

        if is_heartbeat {
            info!("💓 Slot {} - HEARTBEAT (proving liveness)", slot);
        } else {
            info!(
                "👑 Slot {} - I AM LEADER ({} transactions)",
                slot,
                transactions.len()
            );
        }

        // Block rewards are applied as protocol-level effects in
        // apply_block_effects (coinbase model), not as signed transactions.
        // This means no treasury private key is needed for block production.
        let rewards_applied = false;

        // Test transactions disabled - use wallet or CLI to send real transactions
        // (Previous test code was incorrectly signing transfers from genesis with validator key)

        // Create block
        let state_root = state.compute_state_root();
        let mut block = Block::new(
            slot,
            parent_hash,
            state_root,
            validator_pubkey.0,
            transactions.clone(),
        );

        // Sign block so receiving validators can verify authenticity (T2.2)
        block.sign(&validator_keypair);

        let block_hash = block.hash();

        // Store block
        if let Err(e) = state.put_block(&block) {
            error!("Failed to store block at slot {}: {e}", slot);
        }
        if let Err(e) = state.set_last_slot(slot) {
            error!("Failed to update last slot to {}: {e}", slot);
        }
        for tx in &block.transactions {
            if let Some(ix) = tx.message.instructions.first() {
                if ix.program_id == EVM_PROGRAM_ID {
                    let evm_hash = evm_tx_hash(&ix.data).0;
                    if let Err(e) = state.mark_evm_tx_included(&evm_hash, slot, &block_hash) {
                        warn!("⚠️  Failed to mark EVM tx included: {}", e);
                    }
                }
            }
        }
        *last_block_time_for_local.lock().await = std::time::Instant::now();

        if rewards_applied {
            if let Err(e) = state.set_reward_distribution_hash(slot, &block_hash) {
                warn!(
                    "⚠️  Failed to record reward distribution for slot {}: {}",
                    slot, e
                );
            }
        }

        emit_program_and_nft_events(&state, &ws_event_tx, &block);

        // Broadcast block event to WebSocket subscribers
        let _ = ws_event_tx.send(moltchain_rpc::ws::Event::Block(block.clone()));

        // Cast vote for our own block (BFT consensus)
        {
            let mut vote_message = Vec::new();
            vote_message.extend_from_slice(&slot.to_le_bytes());
            vote_message.extend_from_slice(&block_hash.0);
            let signature = validator_keypair.sign(&vote_message);

            let vote = Vote::new(slot, block_hash, validator_pubkey, signature);

            let mut agg = vote_aggregator.lock().await;
            let vs = validator_set.lock().await;
            if agg.add_vote_validated(vote.clone(), &vs) {
                // Check if we reached finality immediately (solo validator case)
                let pool = stake_pool.lock().await;
                if agg.has_supermajority(slot, &block_hash, &vs, &pool) {
                    info!("🔒 Block {} FINALIZED (stake-weighted self-vote)", slot);
                }
                drop(pool);
                drop(vs);

                // Broadcast vote to network
                if let Some(ref peer_mgr) = p2p_peer_manager {
                    let vote_msg = P2PMessage::new(MessageType::Vote(vote), p2p_config.listen_addr);
                    peer_mgr.broadcast(vote_msg).await;
                }
            }
        }

        // Remove included transactions from mempool
        {
            let mut pool = mempool.lock().await;
            for tx_hash in &processed_hashes {
                pool.remove_transaction(tx_hash);
            }
        }

        // Broadcast block to P2P network
        if let Some(ref peer_mgr) = p2p_peer_manager {
            let msg = moltchain_p2p::P2PMessage::new(
                moltchain_p2p::MessageType::Block(block.clone()),
                p2p_config.listen_addr,
            );
            peer_mgr.broadcast(msg).await;
            info!("📡 Broadcasted block {} to network", slot);
        }

        apply_block_effects(
            &state,
            &validator_set,
            &stake_pool,
            &vote_aggregator,
            &block,
            rewards_applied,
        )
        .await;

        // Periodic stats pruning — every 1000 slots, prune seq counters older than 10K slots
        if slot % 1000 == 0 {
            match state.prune_slot_stats(slot, 10_000) {
                Ok(0) => {} // nothing to prune
                Ok(n) => info!("🧹 Pruned {} stale stats keys (retain last 10K slots)", n),
                Err(e) => warn!("⚠️  Stats pruning failed at slot {}: {}", slot, e),
            }
        }

        let tx_count = transactions.len();
        let current_reputation = {
            let vs = validator_set.lock().await;
            vs.get_validator(&validator_pubkey)
                .map(|v| v.reputation)
                .unwrap_or(0)
        };

        if is_heartbeat {
            info!(
                "💓 HEARTBEAT {} | hash: {} | parent: {} | reputation: {} | proving liveness",
                slot,
                block_hash.to_hex()[..8].to_string(),
                parent_hash.to_hex()[..8].to_string(),
                current_reputation,
            );
        } else {
            info!(
                "📦 BLOCK {} | hash: {} | txs: {} | parent: {} | reputation: {}",
                slot,
                block_hash.to_hex()[..8].to_string(),
                tx_count,
                parent_hash.to_hex()[..8].to_string(),
                current_reputation,
            );

            // Show validator balance for transaction blocks
            if let Ok(Some(val_account)) = state.get_account(&validator_pubkey) {
                info!(
                    "   💰 Validator balance: {} MOLT",
                    val_account.balance_molt()
                );
            }
        }

        parent_hash = block_hash;
        slot += 1;
    }
}
