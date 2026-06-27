// P2P Network Manager

use crate::gossip::GossipManager;
use crate::message::{
    validator_announcement_signing_message, CheckpointMetaAnchor, MessageType, P2PMessage,
    SnapshotKind,
};
use crate::peer::{PeerManager, NON_CONSENSUS_FANOUT};
use crate::peer_store::PeerStore;
use lichen_core::{
    codec::serialized_size_legacy_bincode, Block, PqSignature, Precommit, Prevote, Proposal,
    Pubkey, StakePool, Transaction, ValidatorSet, Vote, MAX_BLOCK_SIZE, MAX_TX_PER_BLOCK,
    STATE_SNAPSHOT_CATEGORIES, STATE_SNAPSHOT_SPECIAL_CATEGORIES,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};
use tracing::{debug, error, info, warn};

const MAX_BLOCK_RANGE_REQUEST_SPAN: u64 = 500;
const MAX_BLOCK_RANGE_RESPONSE_BLOCKS: usize = 500;
const MAX_COMPACT_BLOCK_TX_IDS: usize = MAX_TX_PER_BLOCK;
const MAX_GET_BLOCK_TXS_HASHES: usize = MAX_TX_PER_BLOCK;
const MAX_BLOCK_TXS_TRANSACTIONS: usize = MAX_TX_PER_BLOCK;
const MAX_STATE_SNAPSHOT_REQUEST_CHUNK_SIZE: u64 = 2000;
const MAX_STATE_SNAPSHOT_REQUEST_CHUNK_INDEX: u64 = 10_000_000;
const MAX_EXPENSIVE_REQUESTS_PER_WINDOW: u32 = 30;
const MAX_CONCURRENT_RELAY_FANOUT_TASKS: usize = 64;
const MAX_CONCURRENT_BFT_RELAY_FANOUT_TASKS: usize = 64;
const SYNC_BLOCK_QUEUE_SEND_TIMEOUT: Duration = Duration::from_secs(10);
const SNAPSHOT_REQUEST_QUEUE_SEND_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BftAdmission {
    validator: Pubkey,
    height: u64,
    signature_valid: bool,
}

fn proposal_bft_admission(proposal: &Proposal, chain_id: &str) -> BftAdmission {
    BftAdmission {
        validator: proposal.proposer,
        height: proposal.height,
        signature_valid: proposal.verify_signature_with_chain_id(chain_id),
    }
}

fn prevote_bft_admission(prevote: &Prevote, chain_id: &str) -> BftAdmission {
    BftAdmission {
        validator: prevote.validator,
        height: prevote.height,
        signature_valid: prevote.verify_signature_with_chain_id(chain_id),
    }
}

fn precommit_bft_admission(precommit: &Precommit, chain_id: &str) -> BftAdmission {
    BftAdmission {
        validator: precommit.validator,
        height: precommit.height,
        signature_valid: precommit.verify_signature_with_chain_id(chain_id),
    }
}

#[cfg(test)]
fn bft_admission_for_message(msg_type: &MessageType, chain_id: &str) -> Option<BftAdmission> {
    match msg_type {
        MessageType::Proposal(proposal) => Some(proposal_bft_admission(proposal, chain_id)),
        MessageType::Prevote(prevote) => Some(prevote_bft_admission(prevote, chain_id)),
        MessageType::Precommit(precommit) => Some(precommit_bft_admission(precommit, chain_id)),
        _ => None,
    }
}

fn is_allowed_state_snapshot_category(category: &str) -> bool {
    STATE_SNAPSHOT_CATEGORIES.contains(&category)
        || STATE_SNAPSHOT_SPECIAL_CATEGORIES.contains(&category)
}

fn is_rejected_find_node_response_addr(addr: &SocketAddr) -> bool {
    let ip = addr.ip();
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || matches!(ip, std::net::IpAddr::V4(v4) if v4.is_broadcast())
}

fn supplemental_kademlia_peer_infos(
    closest: Vec<([u8; 32], String)>,
    seen_addrs: &std::collections::HashSet<SocketAddr>,
    limit: usize,
) -> Vec<crate::message::PeerInfoMsg> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    closest
        .into_iter()
        .filter_map(|(_, addr_str)| addr_str.parse::<SocketAddr>().ok())
        .filter(|addr| !seen_addrs.contains(addr))
        .take(limit)
        .map(|address| crate::message::PeerInfoMsg {
            address,
            last_seen: now,
            reputation: 500,
            validator_pubkey: None,
        })
        .collect()
}

fn validate_block_for_p2p_admission(block: &Block) -> Result<(), String> {
    block
        .validate_structure()
        .map_err(|err| format!("invalid block structure: {}", err))
}

fn validate_transaction_for_p2p_admission(tx: &Transaction) -> Result<(), String> {
    tx.validate_structure()
        .map_err(|err| format!("invalid transaction structure: {}", err))
}

fn validate_compact_block_for_p2p_admission(
    compact_block: &crate::message::CompactBlock,
) -> Result<(), String> {
    if compact_block.short_ids.len() > MAX_COMPACT_BLOCK_TX_IDS {
        return Err(format!(
            "compact block has {} short tx ids (max {})",
            compact_block.short_ids.len(),
            MAX_COMPACT_BLOCK_TX_IDS
        ));
    }
    if compact_block.tx_fees_paid.len() > MAX_COMPACT_BLOCK_TX_IDS {
        return Err(format!(
            "compact block has {} fee entries (max {})",
            compact_block.tx_fees_paid.len(),
            MAX_COMPACT_BLOCK_TX_IDS
        ));
    }
    let serialized_size = serialized_size_legacy_bincode(compact_block, "compact block")
        .map_err(|err| format!("compact block size check failed: {}", err))?;
    if serialized_size > MAX_BLOCK_SIZE as u64 {
        return Err(format!(
            "compact block too large: {} bytes (max {})",
            serialized_size, MAX_BLOCK_SIZE
        ));
    }
    Ok(())
}

fn validate_get_block_txs_for_p2p_admission(
    missing_hashes: &[lichen_core::Hash],
) -> Result<(), String> {
    if missing_hashes.len() > MAX_GET_BLOCK_TXS_HASHES {
        return Err(format!(
            "GetBlockTxs has {} hashes (max {})",
            missing_hashes.len(),
            MAX_GET_BLOCK_TXS_HASHES
        ));
    }
    Ok(())
}

fn validate_block_txs_for_p2p_admission(transactions: &[Transaction]) -> Result<(), String> {
    if transactions.len() > MAX_BLOCK_TXS_TRANSACTIONS {
        return Err(format!(
            "BlockTxs has {} transactions (max {})",
            transactions.len(),
            MAX_BLOCK_TXS_TRANSACTIONS
        ));
    }
    for (idx, tx) in transactions.iter().enumerate() {
        validate_transaction_for_p2p_admission(tx)
            .map_err(|err| format!("BlockTxs transaction {} rejected: {}", idx, err))?;
    }
    Ok(())
}

fn validate_state_snapshot_request_for_p2p_admission(
    category: &str,
    checkpoint_slot: u64,
    checkpoint_state_root: &[u8; 32],
    snapshot_manifest_root: &[u8; 32],
    chunk_index: u64,
    chunk_size: u64,
) -> Result<(), String> {
    if !is_allowed_state_snapshot_category(category) {
        return Err(format!("unsupported state snapshot category: {}", category));
    }
    if checkpoint_slot == 0 {
        return Err("StateSnapshotRequest missing checkpoint slot anchor".to_string());
    }
    if checkpoint_state_root == &[0u8; 32] {
        return Err("StateSnapshotRequest missing checkpoint state-root anchor".to_string());
    }
    if snapshot_manifest_root == &[0u8; 32] {
        return Err("StateSnapshotRequest missing snapshot manifest-root anchor".to_string());
    }
    if chunk_size == 0 || chunk_size > MAX_STATE_SNAPSHOT_REQUEST_CHUNK_SIZE {
        return Err(format!(
            "StateSnapshotRequest chunk_size {} outside 1..={}",
            chunk_size, MAX_STATE_SNAPSHOT_REQUEST_CHUNK_SIZE
        ));
    }
    if chunk_index > MAX_STATE_SNAPSHOT_REQUEST_CHUNK_INDEX {
        return Err(format!(
            "StateSnapshotRequest chunk_index {} exceeds max {}",
            chunk_index, MAX_STATE_SNAPSHOT_REQUEST_CHUNK_INDEX
        ));
    }
    Ok(())
}

pub fn validate_message_for_p2p_admission(msg_type: &MessageType) -> Result<(), String> {
    match msg_type {
        MessageType::Block(block) | MessageType::BlockResponse(block) => {
            validate_block_for_p2p_admission(block)
        }
        MessageType::Proposal(proposal) => validate_block_for_p2p_admission(&proposal.block),
        MessageType::BlockRangeResponse { blocks } => {
            if blocks.len() > MAX_BLOCK_RANGE_RESPONSE_BLOCKS {
                return Err(format!(
                    "BlockRangeResponse has {} blocks (max {})",
                    blocks.len(),
                    MAX_BLOCK_RANGE_RESPONSE_BLOCKS
                ));
            }
            for (idx, block) in blocks.iter().enumerate() {
                validate_block_for_p2p_admission(block)
                    .map_err(|err| format!("BlockRangeResponse block {} rejected: {}", idx, err))?;
            }
            Ok(())
        }
        MessageType::Transaction(tx) => validate_transaction_for_p2p_admission(tx),
        MessageType::CompactBlockMsg(compact_block) => {
            validate_compact_block_for_p2p_admission(compact_block)
        }
        MessageType::GetBlockTxs { missing_hashes, .. } => {
            validate_get_block_txs_for_p2p_admission(missing_hashes)
        }
        MessageType::BlockTxs { transactions, .. } => {
            validate_block_txs_for_p2p_admission(transactions)
        }
        MessageType::StateSnapshotRequest {
            category,
            checkpoint_slot,
            checkpoint_state_root,
            snapshot_manifest_root,
            chunk_index,
            chunk_size,
        } => validate_state_snapshot_request_for_p2p_admission(
            category,
            *checkpoint_slot,
            checkpoint_state_root,
            snapshot_manifest_root,
            *chunk_index,
            *chunk_size,
        ),
        _ => Ok(()),
    }
}

fn expensive_request_label(msg_type: &MessageType) -> Option<&'static str> {
    match msg_type {
        MessageType::StatusRequest => Some("status request"),
        MessageType::SnapshotRequest { .. } => Some("snapshot request"),
        MessageType::CheckpointMetaRequest => Some("checkpoint meta request"),
        MessageType::FindNode { .. } => Some("FindNode request"),
        _ => None,
    }
}

/// Node role determines connection limits and relay behavior for a 500-validator network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeRole {
    /// Default: connects to 2-3 relays + some peers, max 20 connections
    #[default]
    Validator,
    /// High-bandwidth: accepts many connections, re-broadcasts gossip messages
    Relay,
    /// Address book: connects to many peers, shares peer lists
    Seed,
}

impl NodeRole {
    /// Default max peer connections for each role
    pub fn default_max_peers(&self) -> usize {
        match self {
            NodeRole::Validator => 20,
            NodeRole::Relay => 500,
            NodeRole::Seed => 1000,
        }
    }
}

impl std::fmt::Display for NodeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeRole::Validator => write!(f, "validator"),
            NodeRole::Relay => write!(f, "relay"),
            NodeRole::Seed => write!(f, "seed"),
        }
    }
}

impl std::str::FromStr for NodeRole {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "validator" => Ok(NodeRole::Validator),
            "relay" => Ok(NodeRole::Relay),
            "seed" => Ok(NodeRole::Seed),
            other => Err(format!(
                "Unknown node role '{}': expected 'validator', 'relay', or 'seed'",
                other
            )),
        }
    }
}

/// P2P network configuration
#[derive(Debug, Clone)]
pub struct P2PConfig {
    pub listen_addr: SocketAddr,
    pub seed_peers: Vec<SocketAddr>,
    pub gossip_interval: u64,
    pub cleanup_timeout: u64,
    pub runtime_home: Option<PathBuf>,
    pub peer_store_path: Option<PathBuf>,
    pub max_known_peers: usize,
    /// Node role determines connection limits and relay behavior
    pub role: NodeRole,
    /// Maximum peer connections (if None, auto-set by role)
    pub max_peers: Option<usize>,
    /// Reserved relay/seed peer addresses that are never evicted
    pub reserved_relay_peers: Vec<String>,
    /// P3-6: Externally-reachable address for NAT traversal (if known).
    /// If None, peers behind NAT will use relay-assisted hole punching.
    pub external_addr: Option<SocketAddr>,
    /// Chain id used to validate consensus activity signatures for peer liveness.
    pub consensus_chain_id: String,
    /// Whether this node participates in the consensus reactor by relaying and
    /// enqueueing BFT proposals/votes. Sync-only nodes keep P2P sync/RPC paths
    /// live but must not touch live BFT traffic.
    pub consensus_gossip_enabled: bool,
}

impl Default for P2PConfig {
    fn default() -> Self {
        P2PConfig {
            listen_addr: "127.0.0.1:7001".parse().unwrap(),
            seed_peers: Vec::new(),
            gossip_interval: 10,
            cleanup_timeout: 300,
            runtime_home: None,
            peer_store_path: None,
            max_known_peers: 200,
            role: NodeRole::Validator,
            max_peers: None,
            reserved_relay_peers: Vec::new(),
            external_addr: None,
            consensus_chain_id: String::new(),
            consensus_gossip_enabled: true,
        }
    }
}

impl P2PConfig {
    /// Effective max peers: explicit override or role-based default
    pub fn effective_max_peers(&self) -> usize {
        self.max_peers
            .unwrap_or_else(|| self.role.default_max_peers())
    }

    /// Address advertised in gossip and outgoing P2P messages.
    pub fn advertise_addr(&self) -> SocketAddr {
        self.external_addr.unwrap_or(self.listen_addr)
    }
}

/// T2.3 fix: Signed validator announcement (self-reported reputation removed)
#[derive(Debug, Clone)]
pub struct ValidatorAnnouncement {
    pub peer_addr: SocketAddr,
    pub pubkey: Pubkey,
    pub stake: u64,
    pub current_slot: u64,
    pub version: String,
    pub signature: PqSignature,
    /// SHA-256 machine fingerprint (platform UUID + MAC). [0u8;32] if not set.
    pub machine_fingerprint: [u8; 32],
}

/// Block range request from peer
#[derive(Debug, Clone)]
pub struct BlockRangeRequestMsg {
    pub start_slot: u64,
    pub end_slot: u64,
    pub requester: SocketAddr,
}

/// Status request from peer
#[derive(Debug, Clone)]
pub struct StatusRequestMsg {
    pub requester: SocketAddr,
}

/// Status response from peer
#[derive(Debug, Clone)]
pub struct StatusResponseMsg {
    pub requester: SocketAddr,
    pub current_slot: u64,
    pub total_blocks: u64,
}

/// Verified validator activity observed from signed consensus traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsensusActivityMsg {
    pub validator: Pubkey,
    pub slot: u64,
}

/// Consistency report from peer
#[derive(Debug, Clone)]
pub struct ConsistencyReportMsg {
    pub requester: SocketAddr,
    pub current_slot: u64,
    pub validator_set_hash: lichen_core::Hash,
    pub stake_pool_hash: lichen_core::Hash,
}

/// Exact checkpoint snapshot chunk request from peer.
#[derive(Debug, Clone)]
pub struct StateSnapshotRequestParams {
    pub category: String,
    pub checkpoint_slot: u64,
    pub checkpoint_state_root: [u8; 32],
    pub snapshot_manifest_root: [u8; 32],
    pub chunk_index: u64,
    pub chunk_size: u64,
}

/// Snapshot request from peer
#[derive(Debug, Clone)]
pub struct SnapshotRequestMsg {
    pub requester: SocketAddr,
    pub kind: SnapshotKind,
    /// For StateSnapshotRequest.
    pub state_snapshot_params: Option<StateSnapshotRequestParams>,
    /// True if this is a CheckpointMetaRequest
    pub is_meta_request: bool,
}

/// Snapshot response from peer
#[derive(Debug, Clone)]
pub struct SnapshotResponseMsg {
    pub requester: SocketAddr,
    pub kind: SnapshotKind,
    pub validator_set: Option<ValidatorSet>,
    pub stake_pool: Option<StakePool>,
    /// For StateSnapshotResponse: (category, chunk_index, total_chunks, snapshot_slot, state_root, entries)
    #[allow(clippy::type_complexity)]
    pub state_snapshot_data: Option<(String, u64, u64, u64, [u8; 32], Vec<u8>)>,
    /// For CheckpointMetaResponse: recent verified checkpoint anchors, newest first.
    pub checkpoint_meta: Option<Vec<CheckpointMetaAnchor>>,
}

/// P3-3: Compact block received from a peer
#[derive(Debug, Clone)]
pub struct CompactBlockMsg {
    pub compact_block: crate::message::CompactBlock,
    pub sender: SocketAddr,
}

/// P3-3: Request for missing transactions in a compact block
#[derive(Debug, Clone)]
pub struct GetBlockTxsMsg {
    pub slot: u64,
    pub missing_hashes: Vec<lichen_core::Hash>,
    pub requester: SocketAddr,
}

/// P3-4: Erasure shard request received from a peer
#[derive(Debug, Clone)]
pub struct ErasureShardRequestMsg {
    pub slot: u64,
    pub shard_indices: Vec<usize>,
    pub requester: SocketAddr,
}

/// P3-4: Erasure shard response received from a peer
#[derive(Debug, Clone)]
pub struct ErasureShardResponseMsg {
    pub slot: u64,
    pub shards: Vec<crate::erasure::ErasureShard>,
    pub sender: SocketAddr,
}

/// Main P2P network manager
pub struct P2PNetwork {
    /// Peer manager (public for broadcasting)
    pub peer_manager: Arc<PeerManager>,

    /// Gossip manager
    gossip_manager: Arc<GossipManager>,

    /// Local address
    local_addr: SocketAddr,

    /// Node role (determines relay behavior)
    role: NodeRole,

    /// Message receiver (bounded — T4.7)
    message_rx: mpsc::Receiver<(SocketAddr, P2PMessage)>,

    /// Outgoing block channel (live BFT blocks, compact-reconstructed)
    block_tx: mpsc::Sender<Block>,

    /// Outgoing sync block channel (BlockRangeResponse / BlockResponse)
    /// Separated from block_tx so sync-critical blocks are never dropped
    /// due to live traffic contention during InitialSync catch-up.
    sync_block_tx: mpsc::Sender<Block>,

    /// Outgoing vote channel
    vote_tx: mpsc::Sender<Vote>,

    /// Outgoing transaction channel
    transaction_tx: mpsc::Sender<Transaction>,

    /// Outgoing validator announcement channel
    validator_announce_tx: mpsc::Sender<ValidatorAnnouncement>,

    /// Outgoing block range request channel (for responding)
    block_range_request_tx: mpsc::Sender<BlockRangeRequestMsg>,

    /// Outgoing status request channel
    status_request_tx: mpsc::Sender<StatusRequestMsg>,

    /// Outgoing status response channel
    status_response_tx: mpsc::Sender<StatusResponseMsg>,

    /// Outgoing consistency report channel
    consistency_report_tx: mpsc::Sender<ConsistencyReportMsg>,

    /// Outgoing snapshot request channel
    snapshot_request_tx: mpsc::Sender<SnapshotRequestMsg>,

    /// Outgoing snapshot response channel
    snapshot_response_tx: mpsc::Sender<SnapshotResponseMsg>,

    /// Outgoing slashing evidence channel
    slashing_evidence_tx: mpsc::Sender<lichen_core::SlashingEvidence>,

    /// P3-3: Outgoing compact block channel
    compact_block_tx: mpsc::Sender<CompactBlockMsg>,

    /// P3-3: Outgoing get-block-txs request channel
    get_block_txs_tx: mpsc::Sender<GetBlockTxsMsg>,

    /// P3-4: Outgoing erasure shard request channel
    erasure_shard_request_tx: mpsc::Sender<ErasureShardRequestMsg>,

    /// P3-4: Outgoing erasure shard response channel
    erasure_shard_response_tx: mpsc::Sender<ErasureShardResponseMsg>,

    /// BFT: Outgoing proposal channel
    proposal_tx: mpsc::Sender<Proposal>,

    /// BFT: Outgoing prevote channel
    prevote_tx: mpsc::Sender<Prevote>,

    /// BFT: Outgoing precommit channel
    precommit_tx: mpsc::Sender<Precommit>,

    /// Verified validator activity from signed consensus traffic.
    consensus_activity_tx: mpsc::Sender<ConsensusActivityMsg>,

    /// Chain id used to validate consensus activity signatures for peer liveness.
    consensus_chain_id: String,

    /// False for sync-only nodes.
    consensus_gossip_enabled: bool,

    /// Bounds asynchronous non-consensus gossip relay fanout work so a flood of
    /// unique valid envelopes cannot create unbounded relay tasks.
    relay_task_semaphore: Arc<Semaphore>,

    /// Separate bounded capacity for BFT relay. Consensus messages must not be
    /// starved by non-consensus gossip saturation.
    bft_relay_task_semaphore: Arc<Semaphore>,

    /// AUDIT-FIX H11: Track last announcement slot per validator pubkey
    /// to reject stale/replayed validator announcements.
    last_announce_slot: std::sync::Mutex<std::collections::HashMap<[u8; 32], u64>>,
}

impl P2PNetwork {
    /// Create new P2P network
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        config: P2PConfig,
        block_tx: mpsc::Sender<Block>,
        sync_block_tx: mpsc::Sender<Block>,
        vote_tx: mpsc::Sender<Vote>,
        transaction_tx: mpsc::Sender<Transaction>,
        validator_announce_tx: mpsc::Sender<ValidatorAnnouncement>,
        block_range_request_tx: mpsc::Sender<BlockRangeRequestMsg>,
        status_request_tx: mpsc::Sender<StatusRequestMsg>,
        status_response_tx: mpsc::Sender<StatusResponseMsg>,
        consistency_report_tx: mpsc::Sender<ConsistencyReportMsg>,
        snapshot_request_tx: mpsc::Sender<SnapshotRequestMsg>,
        snapshot_response_tx: mpsc::Sender<SnapshotResponseMsg>,
        slashing_evidence_tx: mpsc::Sender<lichen_core::SlashingEvidence>,
        compact_block_tx: mpsc::Sender<CompactBlockMsg>,
        get_block_txs_tx: mpsc::Sender<GetBlockTxsMsg>,
        erasure_shard_request_tx: mpsc::Sender<ErasureShardRequestMsg>,
        erasure_shard_response_tx: mpsc::Sender<ErasureShardResponseMsg>,
        proposal_tx: mpsc::Sender<Proposal>,
        prevote_tx: mpsc::Sender<Prevote>,
        precommit_tx: mpsc::Sender<Precommit>,
        consensus_activity_tx: mpsc::Sender<ConsensusActivityMsg>,
    ) -> Result<Self, String> {
        let effective_max_peers = config.effective_max_peers();
        info!(
            "🦞 P2P: Initializing network on {} (role={}, max_peers={})",
            config.listen_addr, config.role, effective_max_peers
        );
        let advertise_addr = config.advertise_addr();
        if advertise_addr != config.listen_addr {
            info!(
                "🦞 P2P: Advertising external endpoint {} for bind address {}",
                advertise_addr, config.listen_addr
            );
        }

        // T4.7: Use bounded internal message channel to prevent memory exhaustion from peer floods.
        // Capacity 10K messages provides ~20MB buffer before backpressure kicks in.
        let (message_tx, message_rx) = mpsc::channel(10_000);

        let peer_store = config
            .peer_store_path
            .map(|path| Arc::new(PeerStore::new(path, config.max_known_peers)));

        // Resolve reserved relay peer addresses to SocketAddr for eviction protection
        let mut reserved_addrs: Vec<SocketAddr> = config
            .reserved_relay_peers
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();

        // Configured seed/bootstrap peers are implicitly reserved. Callers must
        // keep durable PeerStore entries separate so ordinary agent endpoints
        // are not promoted into strict bootstrap identity pins.
        for addr in &config.seed_peers {
            if !reserved_addrs.contains(addr) {
                reserved_addrs.push(*addr);
            }
        }

        // Create peer manager with configurable max_peers and reserved peers
        let peer_manager = Arc::new(
            PeerManager::new_with_external_addr(
                config.listen_addr,
                config.external_addr,
                message_tx,
                config.runtime_home.clone(),
                peer_store.clone(),
                effective_max_peers,
                reserved_addrs,
            )
            .await?,
        );

        // Start accepting connections
        peer_manager.start_accepting().await;

        // Create gossip manager (T4.6: pass explicit listen address)
        let gossip_manager = Arc::new(GossipManager::new(
            peer_manager.clone(),
            config.seed_peers,
            config.gossip_interval,
            config.cleanup_timeout,
            peer_store,
            advertise_addr,
        ));

        Ok(P2PNetwork {
            peer_manager,
            gossip_manager,
            local_addr: advertise_addr,
            role: config.role,
            message_rx,
            block_tx,
            sync_block_tx,
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
            compact_block_tx,
            get_block_txs_tx,
            erasure_shard_request_tx,
            erasure_shard_response_tx,
            proposal_tx,
            prevote_tx,
            precommit_tx,
            consensus_activity_tx,
            consensus_chain_id: config.consensus_chain_id,
            consensus_gossip_enabled: config.consensus_gossip_enabled,
            relay_task_semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_RELAY_FANOUT_TASKS)),
            bft_relay_task_semaphore: Arc::new(Semaphore::new(
                MAX_CONCURRENT_BFT_RELAY_FANOUT_TASKS,
            )),
            last_announce_slot: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Start the network
    pub async fn start(mut self) {
        info!("🦞 P2P: Network started on {}", self.local_addr);

        // Start gossip
        self.gossip_manager.start().await;
        info!("🦞 P2P: Gossip started, entering message loop");

        // Main message loop
        while let Some((peer_addr, message)) = self.message_rx.recv().await {
            if let Err(e) = self.handle_message(peer_addr, message).await {
                error!("P2P: Error handling message from {}: {}", peer_addr, e);
            }
        }
        error!("🦞 P2P: Message loop exited (message_rx closed)");
    }

    async fn enqueue_snapshot_request(
        &self,
        peer_addr: SocketAddr,
        label: &'static str,
        request: SnapshotRequestMsg,
    ) {
        match tokio::time::timeout(
            SNAPSHOT_REQUEST_QUEUE_SEND_TIMEOUT,
            self.snapshot_request_tx.send(request),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                warn!(
                    "P2P: Snapshot request channel closed while enqueueing {} from {} ({})",
                    label, peer_addr, e
                );
            }
            Err(_) => {
                warn!(
                    "P2P: Snapshot request channel backpressure timed out while enqueueing {} from {}",
                    label, peer_addr
                );
            }
        }
    }

    /// Handle incoming message
    async fn handle_message(
        &self,
        peer_addr: SocketAddr,
        message: P2PMessage,
    ) -> Result<(), String> {
        if let Err(err) = validate_message_for_p2p_admission(&message.msg_type) {
            warn!(
                "P2P: Rejecting malformed message from {} before gossip/queue admission: {}",
                peer_addr, err
            );
            self.peer_manager.record_violation(&peer_addr);
            return Ok(());
        }
        if let Some(label) = expensive_request_label(&message.msg_type) {
            if !self
                .peer_manager
                .check_expensive_rate_limit(&peer_addr, MAX_EXPENSIVE_REQUESTS_PER_WINDOW)
            {
                warn!("P2P: Rate-limiting {} from {}", label, peer_addr);
                self.peer_manager.record_violation(&peer_addr);
                return Ok(());
            }
        }

        // Relay/Seed nodes re-broadcast ALL gossip messages to all peers except sender.
        // Validator nodes additionally re-broadcast BFT consensus messages
        // (Proposal, Prevote, Precommit) — this matches CometBFT's consensus
        // reactor pattern where every node gossips all known votes to all peers.
        // The SeenMessageCache in handle_connection prevents infinite loops.
        let is_relay_or_seed = self.role == NodeRole::Relay || self.role == NodeRole::Seed;
        let is_bft_message = matches!(
            message.msg_type,
            MessageType::Proposal(_) | MessageType::Prevote(_) | MessageType::Precommit(_)
        );
        let is_gossip_message = matches!(
            message.msg_type,
            MessageType::Block(_)
                | MessageType::Vote(_)
                | MessageType::Proposal(_)
                | MessageType::Prevote(_)
                | MessageType::Precommit(_)
                | MessageType::Transaction(_)
                | MessageType::ValidatorAnnounce { .. }
                | MessageType::SlashingEvidence(_)
                | MessageType::CompactBlockMsg(_)
        );
        let should_relay_bft = is_bft_message && self.consensus_gossip_enabled;
        let relay_version = message.version;
        let relay_sender = message.sender;
        let relay_timestamp = message.timestamp;
        let should_relay_gossip = is_relay_or_seed && is_gossip_message && !is_bft_message;
        if should_relay_gossip {
            self.spawn_relay_except(message.clone(), peer_addr);
        }

        match message.msg_type {
            MessageType::Block(block) => {
                debug!(
                    "P2P: Received block slot {} from {}",
                    block.header.slot, peer_addr
                );
                // Non-blocking: if the validator is behind and the channel is
                // full, drop the block with a warning instead of blocking the
                // entire P2P message loop. The sync manager will request
                // missing blocks via BlockRangeRequest later.
                if let Err(e) = self.block_tx.try_send(block) {
                    warn!(
                        "P2P: Block channel full, dropping block from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::Vote(vote) => {
                debug!(
                    "P2P: Received vote for slot {} from {}",
                    vote.slot, peer_addr
                );
                if let Err(e) = self.vote_tx.try_send(vote) {
                    warn!(
                        "P2P: Vote channel full, dropping vote from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::Proposal(proposal) => {
                if !self.consensus_gossip_enabled {
                    debug!(
                        "P2P: Ignoring BFT proposal h={} r={} from {} in non-consensus mode",
                        proposal.height, proposal.round, peer_addr
                    );
                    return Ok(());
                }
                debug!(
                    "📥 BFT RECV: Proposal h={} r={} from peer {}",
                    proposal.height, proposal.round, peer_addr
                );
                let admission = proposal_bft_admission(&proposal, &self.consensus_chain_id);
                self.record_consensus_activity(
                    peer_addr,
                    admission.validator,
                    admission.height,
                    admission.signature_valid,
                );
                if !admission.signature_valid {
                    debug!(
                        "P2P: Dropping invalid BFT proposal h={} r={} from {}",
                        proposal.height, proposal.round, peer_addr
                    );
                    self.peer_manager.record_violation(&peer_addr);
                    return Ok(());
                }
                let relay_message = should_relay_bft.then(|| P2PMessage {
                    version: relay_version,
                    msg_type: MessageType::Proposal(proposal.clone()),
                    sender: relay_sender,
                    timestamp: relay_timestamp,
                });
                if let Err(e) = self.proposal_tx.try_send(proposal) {
                    warn!(
                        "P2P: Proposal channel full, dropping proposal from {} ({})",
                        peer_addr, e
                    );
                }
                if let Some(relay_message) = relay_message {
                    self.relay_bft_except(relay_message, peer_addr).await;
                }
            }

            MessageType::Prevote(prevote) => {
                if !self.consensus_gossip_enabled {
                    debug!(
                        "P2P: Ignoring BFT prevote h={} r={} from {} in non-consensus mode",
                        prevote.height, prevote.round, peer_addr
                    );
                    return Ok(());
                }
                debug!(
                    "📥 BFT RECV: Prevote h={} r={} from peer {}",
                    prevote.height, prevote.round, peer_addr
                );
                let admission = prevote_bft_admission(&prevote, &self.consensus_chain_id);
                self.record_consensus_activity(
                    peer_addr,
                    admission.validator,
                    admission.height,
                    admission.signature_valid,
                );
                if !admission.signature_valid {
                    debug!(
                        "P2P: Dropping invalid BFT prevote h={} r={} from {}",
                        prevote.height, prevote.round, peer_addr
                    );
                    self.peer_manager.record_violation(&peer_addr);
                    return Ok(());
                }
                let relay_message = should_relay_bft.then(|| P2PMessage {
                    version: relay_version,
                    msg_type: MessageType::Prevote(prevote.clone()),
                    sender: relay_sender,
                    timestamp: relay_timestamp,
                });
                if let Err(e) = self.prevote_tx.try_send(prevote) {
                    warn!(
                        "P2P: Prevote channel full, dropping prevote from {} ({})",
                        peer_addr, e
                    );
                }
                if let Some(relay_message) = relay_message {
                    self.relay_bft_except(relay_message, peer_addr).await;
                }
            }

            MessageType::Precommit(precommit) => {
                if !self.consensus_gossip_enabled {
                    debug!(
                        "P2P: Ignoring BFT precommit h={} r={} from {} in non-consensus mode",
                        precommit.height, precommit.round, peer_addr
                    );
                    return Ok(());
                }
                debug!(
                    "📥 BFT RECV: Precommit h={} r={} from peer {}",
                    precommit.height, precommit.round, peer_addr
                );
                let admission = precommit_bft_admission(&precommit, &self.consensus_chain_id);
                self.record_consensus_activity(
                    peer_addr,
                    admission.validator,
                    admission.height,
                    admission.signature_valid,
                );
                if !admission.signature_valid {
                    debug!(
                        "P2P: Dropping invalid BFT precommit h={} r={} from {}",
                        precommit.height, precommit.round, peer_addr
                    );
                    self.peer_manager.record_violation(&peer_addr);
                    return Ok(());
                }
                let relay_message = should_relay_bft.then(|| P2PMessage {
                    version: relay_version,
                    msg_type: MessageType::Precommit(precommit.clone()),
                    sender: relay_sender,
                    timestamp: relay_timestamp,
                });
                if let Err(e) = self.precommit_tx.try_send(precommit) {
                    warn!(
                        "P2P: Precommit channel full, dropping precommit from {} ({})",
                        peer_addr, e
                    );
                }
                if let Some(relay_message) = relay_message {
                    self.relay_bft_except(relay_message, peer_addr).await;
                }
            }

            MessageType::Transaction(tx) => {
                debug!("P2P: Received transaction from {}", peer_addr);
                if let Err(e) = self.transaction_tx.try_send(tx) {
                    warn!(
                        "P2P: Transaction channel full, dropping tx from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::PeerInfo(peer_infos) => {
                debug!(
                    "P2P: Received peer info from {} ({} peers)",
                    peer_addr,
                    peer_infos.len()
                );
                // PeerInfo gossip advertises candidate addresses only. Canonical
                // node identities come from authenticated handshakes and
                // FindNode responses, not from hashing third-party socket strings.
                let gm = self.gossip_manager.clone();
                tokio::spawn(async move {
                    gm.handle_peer_info(peer_infos).await;
                });
            }

            MessageType::PeerRequest => {
                debug!("P2P: Received peer request from {}", peer_addr);
                // AUDIT-FIX M3: Use actual peer scores, not hardcoded 500
                let peer_infos_raw = self.peer_manager.get_peer_infos();
                let mut seen_addrs: std::collections::HashSet<SocketAddr> =
                    std::collections::HashSet::new();
                let mut peer_infos: Vec<crate::message::PeerInfoMsg> = peer_infos_raw
                    .iter()
                    .take(40) // Leave room for DHT nodes
                    .map(|(addr, score)| {
                        seen_addrs.insert(*addr);
                        crate::message::PeerInfoMsg {
                            address: *addr,
                            last_seen: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs(),
                            reputation: ((*score as i128 + 20) * 1000 / 40).clamp(0, 1000) as u64,
                            validator_pubkey: None,
                        }
                    })
                    .collect();

                // Supplement with canonical Kademlia entries keyed by the
                // requester's authenticated node identity, not by hashing its
                // socket address.
                if let Some(target_id) = self.peer_manager.peer_node_id(&peer_addr) {
                    let closest = self.peer_manager.kademlia_closest(&target_id, 10);
                    peer_infos.extend(supplemental_kademlia_peer_infos(
                        closest,
                        &seen_addrs,
                        50usize.saturating_sub(peer_infos.len()),
                    ));
                }

                let response = P2PMessage::new(MessageType::PeerInfo(peer_infos), self.local_addr);
                let pm = self.peer_manager.clone();
                tokio::spawn(async move {
                    if let Err(e) = pm.send_to_peer(&peer_addr, response).await {
                        warn!("P2P: Failed to send peer info to {}: {}", peer_addr, e);
                    }
                });
            }

            MessageType::Ping => {
                debug!("P2P: Received ping from {}", peer_addr);
                let pong = P2PMessage::new(MessageType::Pong, self.local_addr);
                let pm = self.peer_manager.clone();
                tokio::spawn(async move {
                    if let Err(e) = pm.send_to_peer(&peer_addr, pong).await {
                        warn!("P2P: Failed to send pong to {}: {}", peer_addr, e);
                    }
                });
            }

            MessageType::Pong => {
                debug!("P2P: Received pong from {}", peer_addr);
                // Update peer liveness on pong response
                self.peer_manager.update_peer_last_seen(&peer_addr).await;
            }

            MessageType::BlockRequest { slot } => {
                debug!(
                    "P2P: Received block request for slot {} from {}",
                    slot, peer_addr
                );
                let request = BlockRangeRequestMsg {
                    start_slot: slot,
                    end_slot: slot,
                    requester: peer_addr,
                };
                if let Err(e) = self.block_range_request_tx.try_send(request) {
                    warn!(
                        "P2P: Block range request channel full, dropping request from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::BlockRangeRequest {
                start_slot,
                end_slot,
            } => {
                // AUDIT-FIX H1: Cap max block range to prevent DoS via unbounded requests.
                // A malicious peer could request start=0, end=u64::MAX causing OOM.
                let range = end_slot.saturating_sub(start_slot);
                if range > MAX_BLOCK_RANGE_REQUEST_SPAN {
                    warn!(
                        "P2P: Rejecting block range request {}-{} from {} — range {} exceeds max {}",
                        start_slot, end_slot, peer_addr, range, MAX_BLOCK_RANGE_REQUEST_SPAN
                    );
                    return Ok(());
                }
                if end_slot < start_slot {
                    warn!(
                        "P2P: Rejecting invalid block range {}-{} from {} — end < start",
                        start_slot, end_slot, peer_addr
                    );
                    return Ok(());
                }
                debug!(
                    "P2P: Received block range request {}-{} from {}",
                    start_slot, end_slot, peer_addr
                );
                // Forward to validator to load blocks from state
                let request = BlockRangeRequestMsg {
                    start_slot,
                    end_slot,
                    requester: peer_addr,
                };
                if let Err(e) = self.block_range_request_tx.try_send(request) {
                    warn!(
                        "P2P: Block range request channel full, dropping request from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::BlockResponse(block) => {
                let slot = block.header.slot;
                debug!(
                    "P2P: Received block response for slot {} from {}",
                    slot, peer_addr
                );
                match tokio::time::timeout(
                    SYNC_BLOCK_QUEUE_SEND_TIMEOUT,
                    self.sync_block_tx.send(block),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        warn!(
                            "P2P: Sync block receiver closed while enqueueing block response from {} slot {} ({})",
                            peer_addr, slot, e
                        );
                    }
                    Err(_) => {
                        warn!(
                            "P2P: Sync block channel backpressure timed out for block response from {} slot {}",
                            peer_addr, slot
                        );
                    }
                }
            }

            MessageType::BlockRangeResponse { blocks } => {
                // AUDIT-FIX M12: Cap response size to match request limit
                if blocks.len() > MAX_BLOCK_RANGE_RESPONSE_BLOCKS {
                    warn!(
                        "P2P: Rejecting oversized BlockRangeResponse from {} ({} blocks > 500)",
                        peer_addr,
                        blocks.len()
                    );
                    self.peer_manager.record_violation(&peer_addr);
                    return Ok(());
                }
                info!(
                    "📥 SYNC: Received {} blocks in range response from {} (slots: {}..{})",
                    blocks.len(),
                    peer_addr,
                    blocks.first().map(|b| b.header.slot).unwrap_or(0),
                    blocks.last().map(|b| b.header.slot).unwrap_or(0),
                );
                for block in blocks {
                    let slot = block.header.slot;
                    match tokio::time::timeout(
                        SYNC_BLOCK_QUEUE_SEND_TIMEOUT,
                        self.sync_block_tx.send(block),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            warn!(
                                "P2P: Sync block receiver closed during range response from {} slot {} ({})",
                                peer_addr, slot, e
                            );
                            break;
                        }
                        Err(_) => {
                            warn!(
                                "P2P: Sync block channel backpressure timed out during range response from {} slot {}",
                                peer_addr, slot
                            );
                            break;
                        }
                    }
                }
            }

            MessageType::StatusRequest => {
                debug!("P2P: Received status request from {}", peer_addr);
                let request = StatusRequestMsg {
                    requester: peer_addr,
                };
                if let Err(e) = self.status_request_tx.try_send(request) {
                    warn!(
                        "P2P: Status request channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::StatusResponse {
                current_slot,
                total_blocks,
            } => {
                debug!(
                    "P2P: Peer {} is at slot {} ({} blocks)",
                    peer_addr, current_slot, total_blocks
                );
                let response = StatusResponseMsg {
                    requester: peer_addr,
                    current_slot,
                    total_blocks,
                };
                if let Err(e) = self.status_response_tx.try_send(response) {
                    warn!(
                        "P2P: Status response channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::ConsistencyReport {
                current_slot,
                validator_set_hash,
                stake_pool_hash,
            } => {
                let report = ConsistencyReportMsg {
                    requester: peer_addr,
                    current_slot,
                    validator_set_hash,
                    stake_pool_hash,
                };
                if let Err(e) = self.consistency_report_tx.try_send(report) {
                    warn!(
                        "P2P: Consistency report channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::SnapshotRequest { kind } => {
                let request = SnapshotRequestMsg {
                    requester: peer_addr,
                    kind,
                    state_snapshot_params: None,
                    is_meta_request: false,
                };
                self.enqueue_snapshot_request(peer_addr, "snapshot request", request)
                    .await;
            }

            MessageType::SnapshotResponse {
                kind,
                validator_set,
                stake_pool,
            } => {
                let response = SnapshotResponseMsg {
                    requester: peer_addr,
                    kind,
                    validator_set,
                    stake_pool,
                    state_snapshot_data: None,
                    checkpoint_meta: None,
                };
                if let Err(e) = self.snapshot_response_tx.try_send(response) {
                    warn!(
                        "P2P: Snapshot response channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::StateSnapshotRequest {
                category,
                checkpoint_slot,
                checkpoint_state_root,
                snapshot_manifest_root,
                chunk_index,
                chunk_size,
            } => {
                let request = SnapshotRequestMsg {
                    requester: peer_addr,
                    kind: SnapshotKind::StateCheckpoint,
                    state_snapshot_params: Some(StateSnapshotRequestParams {
                        category,
                        checkpoint_slot,
                        checkpoint_state_root,
                        snapshot_manifest_root,
                        chunk_index,
                        chunk_size,
                    }),
                    is_meta_request: false,
                };
                self.enqueue_snapshot_request(peer_addr, "state snapshot request", request)
                    .await;
            }

            MessageType::StateSnapshotResponse {
                category,
                chunk_index,
                total_chunks,
                snapshot_slot,
                state_root,
                entries,
            } => {
                let response = SnapshotResponseMsg {
                    requester: peer_addr,
                    kind: SnapshotKind::StateCheckpoint,
                    validator_set: None,
                    stake_pool: None,
                    state_snapshot_data: Some((
                        category,
                        chunk_index,
                        total_chunks,
                        snapshot_slot,
                        state_root,
                        entries,
                    )),
                    checkpoint_meta: None,
                };
                if let Err(e) = self.snapshot_response_tx.try_send(response) {
                    warn!(
                        "P2P: State snapshot response channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::CheckpointMetaRequest => {
                let request = SnapshotRequestMsg {
                    requester: peer_addr,
                    kind: SnapshotKind::StateCheckpoint,
                    state_snapshot_params: None,
                    is_meta_request: true,
                };
                self.enqueue_snapshot_request(peer_addr, "checkpoint meta request", request)
                    .await;
            }

            MessageType::CheckpointMetaResponse {
                slot,
                state_root,
                total_accounts,
                checkpoint_header,
                commit_round,
                commit_signatures,
                snapshot_manifest,
                recent_checkpoints,
            } => {
                let mut checkpoint_anchors =
                    Vec::with_capacity(recent_checkpoints.len().saturating_add(1));
                if slot > 0 {
                    checkpoint_anchors.push(CheckpointMetaAnchor {
                        slot,
                        state_root,
                        total_accounts,
                        checkpoint_header,
                        commit_round,
                        commit_signatures,
                        snapshot_manifest,
                    });
                }
                checkpoint_anchors.extend(recent_checkpoints);
                let response = SnapshotResponseMsg {
                    requester: peer_addr,
                    kind: SnapshotKind::StateCheckpoint,
                    validator_set: None,
                    stake_pool: None,
                    state_snapshot_data: None,
                    checkpoint_meta: Some(checkpoint_anchors),
                };
                if let Err(e) = self.snapshot_response_tx.try_send(response) {
                    warn!(
                        "P2P: Checkpoint meta response channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::ValidatorAnnounce {
                pubkey,
                stake,
                current_slot,
                version,
                signature,
                machine_fingerprint,
            } => {
                let signature_valid = validator_announcement_signing_message(
                    &pubkey,
                    stake,
                    current_slot,
                    &machine_fingerprint,
                    Some(version.as_str()),
                )
                .ok()
                .map(|message| lichen_core::account::Keypair::verify(&pubkey, &message, &signature))
                .unwrap_or(false)
                    || validator_announcement_signing_message(
                        &pubkey,
                        stake,
                        current_slot,
                        &machine_fingerprint,
                        None,
                    )
                    .ok()
                    .map(|message| {
                        lichen_core::account::Keypair::verify(&pubkey, &message, &signature)
                    })
                    .unwrap_or(false);

                if !signature_valid {
                    warn!(
                        "⚠️  P2P: Rejecting validator announcement from {} — invalid signature",
                        pubkey.to_base58()
                    );
                    self.peer_manager.record_violation(&peer_addr);
                    return Ok(());
                }

                // AUDIT-FIX H11: Reject stale/replayed announcements.
                // Only accept if current_slot >= the last announcement slot from this validator.
                {
                    let mut slots = self
                        .last_announce_slot
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let last = slots.entry(pubkey.0).or_insert(0);
                    if current_slot < *last {
                        warn!(
                            "⚠️  P2P: Rejecting stale validator announcement from {} — slot {} < last {}",
                            pubkey.to_base58(), current_slot, *last
                        );
                        return Ok(());
                    }
                    *last = current_slot;
                }

                info!(
                    "🦞 P2P: Verified validator announcement from {}: {} (stake: {}, slot: {}, version: {})",
                    peer_addr,
                    pubkey.to_base58(),
                    stake,
                    current_slot,
                    if version.is_empty() { "unknown" } else { &version }
                );
                // Validator pubkeys remain metadata only until the validator binary
                // verifies local stake/validator-set membership. A self-signed P2P
                // announcement alone must not grant validator-route status.
                let announcement = ValidatorAnnouncement {
                    peer_addr,
                    pubkey,
                    stake,
                    current_slot,
                    version,
                    signature,
                    machine_fingerprint,
                };
                if let Err(e) = self.validator_announce_tx.try_send(announcement) {
                    warn!(
                        "P2P: Validator announce channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::SlashingEvidence(evidence) => {
                info!(
                    "🦞 P2P: Received slashing evidence for {} from {}",
                    evidence.validator.to_base58(),
                    peer_addr
                );
                if let Err(e) = self.slashing_evidence_tx.try_send(evidence) {
                    warn!(
                        "P2P: Slashing evidence channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::FindNode { target_id } => {
                debug!(
                    "P2P: Received FindNode from {} for target {:?}",
                    peer_addr,
                    &target_id[..4]
                );
                let closest = self.peer_manager.kademlia_closest(&target_id, 20);
                let response = P2PMessage::new(
                    MessageType::FindNodeResponse { target_id, closest },
                    self.local_addr,
                );
                let pm = self.peer_manager.clone();
                tokio::spawn(async move {
                    if let Err(e) = pm.send_to_peer(&peer_addr, response).await {
                        warn!(
                            "P2P: Failed to send FindNodeResponse to {}: {}",
                            peer_addr, e
                        );
                    }
                });
            }

            MessageType::FindNodeResponse {
                target_id: _,
                closest,
            } => {
                debug!(
                    "P2P: Received FindNodeResponse from {} ({} entries)",
                    peer_addr,
                    closest.len()
                );
                for (node_id, addr_str) in closest {
                    if let Ok(addr) = addr_str.parse::<SocketAddr>() {
                        if is_rejected_find_node_response_addr(&addr) {
                            warn!(
                                "P2P: Rejecting invalid address {} from FindNodeResponse by {}",
                                addr, peer_addr
                            );
                            continue;
                        }
                        self.peer_manager.update_kademlia(node_id, addr);
                    }
                }
            }

            MessageType::CompactBlockMsg(compact_block) => {
                debug!(
                    "P2P: Received compact block slot {} from {} ({} txs)",
                    compact_block.header.slot,
                    peer_addr,
                    compact_block.short_ids.len()
                );
                let msg = CompactBlockMsg {
                    compact_block,
                    sender: peer_addr,
                };
                if let Err(e) = self.compact_block_tx.try_send(msg) {
                    warn!(
                        "P2P: Compact block channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::GetBlockTxs {
                slot,
                missing_hashes,
            } => {
                debug!(
                    "P2P: Received GetBlockTxs for slot {} from {} ({} hashes)",
                    slot,
                    peer_addr,
                    missing_hashes.len()
                );
                let msg = GetBlockTxsMsg {
                    slot,
                    missing_hashes,
                    requester: peer_addr,
                };
                if let Err(e) = self.get_block_txs_tx.try_send(msg) {
                    warn!(
                        "P2P: GetBlockTxs channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::BlockTxs { slot, transactions } => {
                debug!(
                    "P2P: Received BlockTxs for slot {} from {} ({} txs)",
                    slot,
                    peer_addr,
                    transactions.len()
                );
                // Forward individual transactions to the normal tx channel so the
                // compact block reconstruction path in the validator can pick them up.
                for tx in transactions {
                    if let Err(e) = self.transaction_tx.try_send(tx) {
                        warn!("P2P: BlockTxs tx channel full, dropping ({})", e);
                        break;
                    }
                }
            }

            MessageType::ErasureShardRequest {
                slot,
                shard_indices,
            } => {
                // AUDIT-FIX M13: Cap shard indices to prevent amplification
                const MAX_SHARD_INDICES: usize = 10;
                if shard_indices.len() > MAX_SHARD_INDICES {
                    warn!(
                        "P2P: Rejecting ErasureShardRequest from {} — {} indices exceeds max {}",
                        peer_addr,
                        shard_indices.len(),
                        MAX_SHARD_INDICES
                    );
                    self.peer_manager.record_violation(&peer_addr);
                    return Ok(());
                }
                debug!(
                    "P2P: Received ErasureShardRequest for slot {} from {} ({} indices)",
                    slot,
                    peer_addr,
                    shard_indices.len()
                );
                let msg = ErasureShardRequestMsg {
                    slot,
                    shard_indices,
                    requester: peer_addr,
                };
                if let Err(e) = self.erasure_shard_request_tx.try_send(msg) {
                    warn!(
                        "P2P: ErasureShardRequest channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::ErasureShardResponse { slot, shards } => {
                debug!(
                    "P2P: Received ErasureShardResponse for slot {} from {} ({} shards)",
                    slot,
                    peer_addr,
                    shards.len()
                );
                let msg = ErasureShardResponseMsg {
                    slot,
                    shards,
                    sender: peer_addr,
                };
                if let Err(e) = self.erasure_shard_response_tx.try_send(msg) {
                    warn!(
                        "P2P: ErasureShardResponse channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            // P3-6: Relay-assisted hole punch request — only relay/seed nodes process this.
            // The relay forwards a HolePunchNotify to the target peer.
            MessageType::HolePunchRequest {
                target_addr,
                requester_observed_addr,
            } => {
                if self.role == NodeRole::Relay || self.role == NodeRole::Seed {
                    info!(
                        "P2P: Relaying hole punch from {} (observed: {}) to target {}",
                        peer_addr, requester_observed_addr, target_addr
                    );
                    let notify = P2PMessage::new(
                        MessageType::HolePunchNotify {
                            peer_observed_addr: requester_observed_addr,
                        },
                        self.local_addr,
                    );
                    let pm = self.peer_manager.clone();
                    tokio::spawn(async move {
                        if let Err(e) = pm.send_to_peer(&target_addr, notify).await {
                            warn!("P2P: Failed to relay hole punch to {}: {}", target_addr, e);
                        }
                    });
                } else {
                    debug!(
                        "P2P: Ignoring HolePunchRequest from {} (not a relay)",
                        peer_addr
                    );
                }
            }

            // P3-6: Hole punch notification — a relay is telling us to send a
            // packet to the given address to punch through their NAT.
            MessageType::HolePunchNotify { peer_observed_addr } => {
                info!(
                    "P2P: Received hole punch notify — attempting connection to {}",
                    peer_observed_addr
                );
                let pm = self.peer_manager.clone();
                tokio::spawn(async move {
                    if let Err(e) = pm.connect_peer(peer_observed_addr).await {
                        warn!(
                            "P2P: Hole punch connection to {} failed: {}",
                            peer_observed_addr, e
                        );
                    }
                });
            }
        }

        Ok(())
    }

    fn spawn_relay_except(&self, relay_message: P2PMessage, peer_addr: SocketAddr) {
        let peer_manager = self.peer_manager.clone();
        let relay_task_semaphore = self.relay_task_semaphore.clone();

        match relay_task_semaphore.try_acquire_owned() {
            Ok(permit) => {
                tokio::spawn(async move {
                    let _permit = permit;
                    peer_manager
                        .broadcast_except(&relay_message, &peer_addr)
                        .await;
                });
            }
            Err(_) => {
                debug!(
                    "P2P: Relay fanout saturated; skipping relay for message from {}",
                    peer_addr
                );
            }
        }
    }

    async fn relay_bft_except(&self, relay_message: P2PMessage, peer_addr: SocketAddr) {
        let permit = match self.bft_relay_task_semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(e) => {
                warn!(
                    "P2P: BFT relay semaphore closed; cannot relay consensus message from {} ({})",
                    peer_addr, e
                );
                return;
            }
        };
        let _permit = permit;
        self.peer_manager
            .broadcast_except(&relay_message, &peer_addr)
            .await;
    }

    fn record_consensus_activity(
        &self,
        peer_addr: SocketAddr,
        validator: Pubkey,
        slot: u64,
        signature_valid: bool,
    ) {
        if !signature_valid {
            return;
        }

        if let Err(e) = self
            .consensus_activity_tx
            .try_send(ConsensusActivityMsg { validator, slot })
        {
            warn!(
                "P2P: Consensus activity channel full, dropping validator activity from {} for {} at slot {} ({})",
                peer_addr,
                validator.to_base58(),
                slot,
                e
            );
        }
    }

    /// Disseminate a block through the non-consensus gossip path.
    pub async fn broadcast_block(&self, block: Block) {
        debug!("🦞 P2P: Broadcasting block slot {}", block.header.slot);
        let target_id = block.hash().0;
        let message = P2PMessage::new(MessageType::Block(block), self.local_addr);
        self.peer_manager
            .route_to_closest(&target_id, NON_CONSENSUS_FANOUT, message)
            .await;
    }

    /// Broadcast a vote to all peers
    pub async fn broadcast_vote(&self, vote: Vote) {
        debug!("🦞 P2P: Broadcasting vote for slot {}", vote.slot);
        let message = P2PMessage::new(MessageType::Vote(vote), self.local_addr);
        self.peer_manager.broadcast(message).await;
    }

    /// Disseminate a transaction through the non-consensus gossip path.
    pub async fn broadcast_transaction(&self, tx: Transaction) {
        debug!("🦞 P2P: Broadcasting transaction");
        let target_id = tx.hash().0;
        let message = P2PMessage::new(MessageType::Transaction(tx), self.local_addr);
        self.peer_manager
            .route_to_closest(&target_id, NON_CONSENSUS_FANOUT, message)
            .await;
    }

    /// Get connected peers
    pub fn get_peers(&self) -> Vec<SocketAddr> {
        self.peer_manager.get_peers()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lichen_core::account::{
        ML_DSA_65_PUBLIC_KEY_BYTES, ML_DSA_65_SIGNATURE_BYTES, PQ_SCHEME_ML_DSA_65,
    };
    use lichen_core::transaction::MAX_SIGNATURES_PER_TX;
    use lichen_core::{Hash, Keypair, Message, PqPublicKey};

    const TEST_CHAIN_ID: &str = "lichen-testnet-p2p-bft-admission";

    fn test_pq_signature(fill: u8) -> PqSignature {
        let public_key =
            PqPublicKey::new(PQ_SCHEME_ML_DSA_65, vec![fill; ML_DSA_65_PUBLIC_KEY_BYTES])
                .expect("test public key");
        PqSignature::new(
            PQ_SCHEME_ML_DSA_65,
            public_key,
            vec![fill; ML_DSA_65_SIGNATURE_BYTES],
        )
        .expect("test signature")
    }

    fn signed_test_proposal(
        signer: &Keypair,
        proposer: Pubkey,
        height: u64,
        chain_id: &str,
    ) -> Proposal {
        let block = Block::new(
            height,
            Hash::hash(b"p2p-bft-proposal-parent"),
            Hash::hash(b"p2p-bft-proposal-state"),
            proposer.0,
            Vec::new(),
        );
        let block_hash = block.hash();
        let signature = signer.sign(&Proposal::signing_bytes_static_for_chain_id(
            chain_id,
            height,
            0,
            &block_hash,
            -1,
        ));

        Proposal {
            height,
            round: 0,
            block,
            valid_round: -1,
            proposer,
            signature,
        }
    }

    fn signed_test_prevote(
        signer: &Keypair,
        validator: Pubkey,
        height: u64,
        chain_id: &str,
    ) -> Prevote {
        let block_hash = Some(Hash::hash(b"p2p-bft-prevote-block"));
        let signature = signer.sign(&Prevote::signing_bytes_for_chain_id(
            chain_id,
            height,
            0,
            &block_hash,
        ));

        Prevote {
            height,
            round: 0,
            block_hash,
            validator,
            signature,
        }
    }

    fn signed_test_precommit(
        signer: &Keypair,
        validator: Pubkey,
        height: u64,
        chain_id: &str,
    ) -> Precommit {
        let block_hash = Some(Hash::hash(b"p2p-bft-precommit-block"));
        let timestamp = 1_720_000_000;
        let signature = signer.sign(&Precommit::signing_bytes_for_chain_id(
            chain_id,
            height,
            0,
            &block_hash,
            timestamp,
        ));

        Precommit {
            height,
            round: 0,
            block_hash,
            validator,
            signature,
            timestamp,
        }
    }

    #[test]
    fn test_bft_admission_accepts_valid_chain_domain_signatures() {
        let validator = Keypair::generate();
        let validator_pubkey = validator.pubkey();

        let proposal = signed_test_proposal(&validator, validator_pubkey, 41, TEST_CHAIN_ID);
        let prevote = signed_test_prevote(&validator, validator_pubkey, 42, TEST_CHAIN_ID);
        let precommit = signed_test_precommit(&validator, validator_pubkey, 43, TEST_CHAIN_ID);

        assert_eq!(
            bft_admission_for_message(&MessageType::Proposal(proposal), TEST_CHAIN_ID,),
            Some(BftAdmission {
                validator: validator_pubkey,
                height: 41,
                signature_valid: true,
            })
        );
        assert_eq!(
            bft_admission_for_message(&MessageType::Prevote(prevote), TEST_CHAIN_ID),
            Some(BftAdmission {
                validator: validator_pubkey,
                height: 42,
                signature_valid: true,
            })
        );
        assert_eq!(
            bft_admission_for_message(&MessageType::Precommit(precommit), TEST_CHAIN_ID,),
            Some(BftAdmission {
                validator: validator_pubkey,
                height: 43,
                signature_valid: true,
            })
        );
    }

    #[test]
    fn test_bft_admission_rejects_claimed_validator_signature_mismatch() {
        let claimed_validator = Keypair::generate();
        let signer = Keypair::generate();
        let claimed_pubkey = claimed_validator.pubkey();

        let proposal = signed_test_proposal(&signer, claimed_pubkey, 51, TEST_CHAIN_ID);
        let prevote = signed_test_prevote(&signer, claimed_pubkey, 52, TEST_CHAIN_ID);
        let precommit = signed_test_precommit(&signer, claimed_pubkey, 53, TEST_CHAIN_ID);

        assert_eq!(
            bft_admission_for_message(&MessageType::Proposal(proposal), TEST_CHAIN_ID),
            Some(BftAdmission {
                validator: claimed_pubkey,
                height: 51,
                signature_valid: false,
            })
        );
        assert_eq!(
            bft_admission_for_message(&MessageType::Prevote(prevote), TEST_CHAIN_ID),
            Some(BftAdmission {
                validator: claimed_pubkey,
                height: 52,
                signature_valid: false,
            })
        );
        assert_eq!(
            bft_admission_for_message(&MessageType::Precommit(precommit), TEST_CHAIN_ID),
            Some(BftAdmission {
                validator: claimed_pubkey,
                height: 53,
                signature_valid: false,
            })
        );
    }

    #[test]
    fn test_bft_admission_ignores_non_bft_messages() {
        assert_eq!(
            bft_admission_for_message(&MessageType::Ping, TEST_CHAIN_ID),
            None
        );
    }

    #[test]
    fn test_node_role_default_is_validator() {
        assert_eq!(NodeRole::default(), NodeRole::Validator);
    }

    #[test]
    fn test_node_role_default_max_peers() {
        assert_eq!(NodeRole::Validator.default_max_peers(), 20);
        assert_eq!(NodeRole::Relay.default_max_peers(), 500);
        assert_eq!(NodeRole::Seed.default_max_peers(), 1000);
    }

    #[test]
    fn test_node_role_display() {
        assert_eq!(format!("{}", NodeRole::Validator), "validator");
        assert_eq!(format!("{}", NodeRole::Relay), "relay");
        assert_eq!(format!("{}", NodeRole::Seed), "seed");
    }

    #[test]
    fn test_expensive_request_classification_excludes_state_snapshot_chunks() {
        assert_eq!(
            expensive_request_label(&MessageType::CheckpointMetaRequest),
            Some("checkpoint meta request")
        );
        assert_eq!(
            expensive_request_label(&MessageType::StatusRequest),
            Some("status request")
        );
        assert_eq!(
            expensive_request_label(&MessageType::StateSnapshotRequest {
                category: "accounts".to_string(),
                checkpoint_slot: 1000,
                checkpoint_state_root: [7u8; 32],
                snapshot_manifest_root: [8u8; 32],
                chunk_index: 0,
                chunk_size: 2000,
            }),
            None
        );
        assert_eq!(expensive_request_label(&MessageType::Ping), None);
    }

    #[test]
    fn test_state_snapshot_request_admission_validation() {
        assert!(
            validate_message_for_p2p_admission(&MessageType::StateSnapshotRequest {
                category: "accounts".to_string(),
                checkpoint_slot: 1000,
                checkpoint_state_root: [7u8; 32],
                snapshot_manifest_root: [8u8; 32],
                chunk_index: 0,
                chunk_size: 2000,
            })
            .is_ok()
        );

        assert!(
            validate_message_for_p2p_admission(&MessageType::StateSnapshotRequest {
                category: "unknown".to_string(),
                checkpoint_slot: 1000,
                checkpoint_state_root: [7u8; 32],
                snapshot_manifest_root: [8u8; 32],
                chunk_index: 0,
                chunk_size: 2000,
            })
            .is_err()
        );

        assert!(
            validate_message_for_p2p_admission(&MessageType::StateSnapshotRequest {
                category: "accounts".to_string(),
                checkpoint_slot: 1000,
                checkpoint_state_root: [7u8; 32],
                snapshot_manifest_root: [8u8; 32],
                chunk_index: 0,
                chunk_size: 0,
            })
            .is_err()
        );

        assert!(
            validate_message_for_p2p_admission(&MessageType::StateSnapshotRequest {
                category: "accounts".to_string(),
                checkpoint_slot: 1000,
                checkpoint_state_root: [7u8; 32],
                snapshot_manifest_root: [8u8; 32],
                chunk_index: 0,
                chunk_size: 2001,
            })
            .is_err()
        );

        assert!(
            validate_message_for_p2p_admission(&MessageType::StateSnapshotRequest {
                category: "accounts".to_string(),
                checkpoint_slot: 0,
                checkpoint_state_root: [7u8; 32],
                snapshot_manifest_root: [8u8; 32],
                chunk_index: 0,
                chunk_size: 2000,
            })
            .is_err()
        );

        assert!(
            validate_message_for_p2p_admission(&MessageType::StateSnapshotRequest {
                category: "accounts".to_string(),
                checkpoint_slot: 1000,
                checkpoint_state_root: [0u8; 32],
                snapshot_manifest_root: [8u8; 32],
                chunk_index: 0,
                chunk_size: 2000,
            })
            .is_err()
        );

        assert!(
            validate_message_for_p2p_admission(&MessageType::StateSnapshotRequest {
                category: "accounts".to_string(),
                checkpoint_slot: 1000,
                checkpoint_state_root: [7u8; 32],
                snapshot_manifest_root: [0u8; 32],
                chunk_index: 0,
                chunk_size: 2000,
            })
            .is_err()
        );
    }

    #[test]
    fn test_node_role_from_str() {
        assert_eq!(
            "validator".parse::<NodeRole>().unwrap(),
            NodeRole::Validator
        );
        assert_eq!("relay".parse::<NodeRole>().unwrap(), NodeRole::Relay);
        assert_eq!("seed".parse::<NodeRole>().unwrap(), NodeRole::Seed);
        assert_eq!("RELAY".parse::<NodeRole>().unwrap(), NodeRole::Relay);
        assert_eq!("Seed".parse::<NodeRole>().unwrap(), NodeRole::Seed);
        assert!("unknown".parse::<NodeRole>().is_err());
    }

    #[test]
    fn test_node_role_roundtrip() {
        for role in [NodeRole::Validator, NodeRole::Relay, NodeRole::Seed] {
            let s = format!("{}", role);
            let parsed: NodeRole = s.parse().unwrap();
            assert_eq!(parsed, role);
        }
    }

    #[test]
    fn test_find_node_response_rejects_invalid_addresses() {
        for raw in [
            "127.0.0.1:7001",
            "0.0.0.0:7001",
            "224.0.0.1:7001",
            "255.255.255.255:7001",
        ] {
            let addr: SocketAddr = raw.parse().unwrap();
            assert!(
                is_rejected_find_node_response_addr(&addr),
                "{} should be rejected",
                raw
            );
        }

        let valid: SocketAddr = "198.51.100.8:7001".parse().unwrap();
        assert!(!is_rejected_find_node_response_addr(&valid));
    }

    #[test]
    fn test_p2p_config_effective_max_peers_default() {
        let config = P2PConfig::default();
        // Default role=Validator, max_peers=None → 20
        assert_eq!(config.effective_max_peers(), 20);
    }

    #[test]
    fn test_p2p_config_effective_max_peers_override() {
        let config = P2PConfig {
            max_peers: Some(100),
            ..Default::default()
        };
        assert_eq!(config.effective_max_peers(), 100);
    }

    #[test]
    fn test_p2p_config_advertise_addr_prefers_external_endpoint() {
        let external = "203.0.113.10:7001".parse().unwrap();
        let config = P2PConfig {
            listen_addr: "0.0.0.0:7001".parse().unwrap(),
            external_addr: Some(external),
            ..Default::default()
        };

        assert_eq!(config.advertise_addr(), external);
    }

    #[test]
    fn test_p2p_config_effective_max_peers_relay() {
        let config = P2PConfig {
            role: NodeRole::Relay,
            ..Default::default()
        };
        assert_eq!(config.effective_max_peers(), 500);
    }

    #[test]
    fn test_p2p_config_effective_max_peers_seed() {
        let config = P2PConfig {
            role: NodeRole::Seed,
            ..Default::default()
        };
        assert_eq!(config.effective_max_peers(), 1000);
    }

    #[test]
    fn test_p2p_config_reserved_peers_empty_by_default() {
        let config = P2PConfig::default();
        assert!(config.reserved_relay_peers.is_empty());
    }

    #[test]
    fn test_supplemental_kademlia_peer_infos_preserve_overlay_order() {
        let seen_addrs =
            std::collections::HashSet::from(["10.0.0.1:7001".parse::<SocketAddr>().unwrap()]);
        let closest = vec![
            ([1u8; 32], "10.0.0.1:7001".to_string()),
            ([2u8; 32], "10.0.0.2:7001".to_string()),
            ([3u8; 32], "10.0.0.3:7001".to_string()),
        ];

        let infos = supplemental_kademlia_peer_infos(closest, &seen_addrs, 2);

        assert_eq!(infos.len(), 2);
        assert_eq!(
            infos[0].address,
            "10.0.0.2:7001".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            infos[1].address,
            "10.0.0.3:7001".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn test_supplemental_kademlia_peer_infos_drop_invalid_addresses() {
        let closest = vec![
            ([1u8; 32], "not-a-socket".to_string()),
            ([2u8; 32], "10.0.0.4:7001".to_string()),
        ];

        let infos = supplemental_kademlia_peer_infos(closest, &std::collections::HashSet::new(), 4);

        assert_eq!(infos.len(), 1);
        assert_eq!(
            infos[0].address,
            "10.0.0.4:7001".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn test_p2p_admission_rejects_invalid_transaction_structure() {
        let mut tx = Transaction::new(Message::new(Vec::new(), Hash::default()));
        tx.signatures = (0..=MAX_SIGNATURES_PER_TX)
            .map(|idx| test_pq_signature(idx as u8))
            .collect();

        assert!(validate_transaction_for_p2p_admission(&tx).is_err());
        assert!(validate_message_for_p2p_admission(&MessageType::Transaction(tx)).is_err());
    }

    #[test]
    fn test_state_snapshot_request_admission_uses_core_category_surface() {
        for category in STATE_SNAPSHOT_CATEGORIES
            .iter()
            .chain(STATE_SNAPSHOT_SPECIAL_CATEGORIES.iter())
        {
            validate_state_snapshot_request_for_p2p_admission(
                category, 1000, &[7u8; 32], &[8u8; 32], 0, 1,
            )
            .unwrap_or_else(|err| panic!("{category} should be accepted: {err}"));
        }

        assert!(validate_state_snapshot_request_for_p2p_admission(
            "forgotten_cf",
            1000,
            &[7u8; 32],
            &[8u8; 32],
            0,
            1
        )
        .is_err());
    }

    #[test]
    fn test_p2p_admission_rejects_oversized_compact_block_vectors() {
        let block = Block::new(1, Hash::default(), Hash::default(), [0u8; 32], Vec::new());
        let mut compact = crate::message::CompactBlock::from_block(&block);
        compact.short_ids = vec![[0u8; 12]; MAX_COMPACT_BLOCK_TX_IDS + 1];

        assert!(validate_compact_block_for_p2p_admission(&compact).is_err());
        assert!(
            validate_message_for_p2p_admission(&MessageType::CompactBlockMsg(compact)).is_err()
        );
    }

    #[test]
    fn test_p2p_admission_rejects_oversized_get_block_txs() {
        let hashes = vec![Hash::default(); MAX_GET_BLOCK_TXS_HASHES + 1];

        assert!(validate_get_block_txs_for_p2p_admission(&hashes).is_err());
        assert!(
            validate_message_for_p2p_admission(&MessageType::GetBlockTxs {
                slot: 1,
                missing_hashes: hashes,
            })
            .is_err()
        );
    }

    #[test]
    fn test_p2p_admission_rejects_oversized_block_txs() {
        let tx = Transaction::new(Message::new(Vec::new(), Hash::default()));
        let transactions = vec![tx; MAX_BLOCK_TXS_TRANSACTIONS + 1];

        assert!(validate_block_txs_for_p2p_admission(&transactions).is_err());
        assert!(validate_message_for_p2p_admission(&MessageType::BlockTxs {
            slot: 1,
            transactions,
        })
        .is_err());
    }
}
