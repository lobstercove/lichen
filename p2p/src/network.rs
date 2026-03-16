// P2P Network Manager

use crate::gossip::GossipManager;
use crate::kademlia::{KademliaTable, NodeId};
use crate::message::{
    validator_announcement_signing_message, MessageType, P2PMessage, SnapshotKind,
};
use crate::peer::PeerManager;
use crate::peer_store::PeerStore;
use moltchain_core::{
    Block, Precommit, Prevote, Proposal, Pubkey, StakePool, Transaction, ValidatorSet, Vote,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

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
        }
    }
}

impl P2PConfig {
    /// Effective max peers: explicit override or role-based default
    pub fn effective_max_peers(&self) -> usize {
        self.max_peers
            .unwrap_or_else(|| self.role.default_max_peers())
    }
}

/// T2.3 fix: Signed validator announcement (self-reported reputation removed)
#[derive(Debug, Clone)]
pub struct ValidatorAnnouncement {
    pub pubkey: Pubkey,
    pub stake: u64,
    pub current_slot: u64,
    pub version: String,
    pub signature: [u8; 64],
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

/// Consistency report from peer
#[derive(Debug, Clone)]
pub struct ConsistencyReportMsg {
    pub requester: SocketAddr,
    pub validator_set_hash: moltchain_core::Hash,
    pub stake_pool_hash: moltchain_core::Hash,
}

/// Snapshot request from peer
#[derive(Debug, Clone)]
pub struct SnapshotRequestMsg {
    pub requester: SocketAddr,
    pub kind: SnapshotKind,
    /// For StateSnapshotRequest: category, chunk_index, chunk_size
    pub state_snapshot_params: Option<(String, u64, u64)>,
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
    /// For CheckpointMetaResponse: (slot, state_root, total_accounts)
    pub checkpoint_meta: Option<(u64, [u8; 32], u64)>,
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
    pub missing_hashes: Vec<moltchain_core::Hash>,
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
    slashing_evidence_tx: mpsc::Sender<moltchain_core::SlashingEvidence>,

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

    /// AUDIT-FIX H11: Track last announcement slot per validator pubkey
    /// to reject stale/replayed validator announcements.
    last_announce_slot: std::sync::Mutex<std::collections::HashMap<[u8; 32], u64>>,

    /// Kademlia DHT routing table for structured peer discovery (H-8/H-9).
    dht: Arc<std::sync::Mutex<KademliaTable>>,
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
        slashing_evidence_tx: mpsc::Sender<moltchain_core::SlashingEvidence>,
        compact_block_tx: mpsc::Sender<CompactBlockMsg>,
        get_block_txs_tx: mpsc::Sender<GetBlockTxsMsg>,
        erasure_shard_request_tx: mpsc::Sender<ErasureShardRequestMsg>,
        erasure_shard_response_tx: mpsc::Sender<ErasureShardResponseMsg>,
        proposal_tx: mpsc::Sender<Proposal>,
        prevote_tx: mpsc::Sender<Prevote>,
        precommit_tx: mpsc::Sender<Precommit>,
    ) -> Result<Self, String> {
        let effective_max_peers = config.effective_max_peers();
        info!(
            "🦞 P2P: Initializing network on {} (role={}, max_peers={})",
            config.listen_addr, config.role, effective_max_peers
        );

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

        // Seed/bootstrap peers are implicitly reserved — their TOFU fingerprint
        // rotations are auto-accepted so freshly joining validators can always
        // reach the network even after seed nodes redeploy.
        for addr in &config.seed_peers {
            if !reserved_addrs.contains(addr) {
                reserved_addrs.push(*addr);
            }
        }

        // Create peer manager with configurable max_peers and reserved peers
        let peer_manager = Arc::new(
            PeerManager::new(
                config.listen_addr,
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
            config.listen_addr,
        ));

        // Create Kademlia DHT routing table for structured peer discovery (H-8/H-9).
        // Node ID derived from SHA-256 of the listen address for uniqueness.
        let local_node_id: NodeId = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(config.listen_addr.to_string().as_bytes());
            let hash = hasher.finalize();
            let mut id = [0u8; 32];
            id.copy_from_slice(&hash);
            id
        };
        let dht = Arc::new(std::sync::Mutex::new(KademliaTable::new(local_node_id)));

        Ok(P2PNetwork {
            peer_manager,
            gossip_manager,
            local_addr: config.listen_addr,
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
            last_announce_slot: std::sync::Mutex::new(std::collections::HashMap::new()),
            dht,
        })
    }

    /// Start the network
    pub async fn start(mut self) {
        info!("🦞 P2P: Network started on {}", self.local_addr);

        // Start gossip
        self.gossip_manager.start().await;

        // Main message loop
        while let Some((peer_addr, message)) = self.message_rx.recv().await {
            if let Err(e) = self.handle_message(peer_addr, message).await {
                error!("P2P: Error handling message from {}: {}", peer_addr, e);
            }
        }
    }

    /// Handle incoming message
    async fn handle_message(
        &self,
        peer_addr: SocketAddr,
        message: P2PMessage,
    ) -> Result<(), String> {
        // Relay/Seed nodes re-broadcast gossip messages to all peers except sender.
        // The SeenMessageCache in handle_connection already prevents loops.
        if (self.role == NodeRole::Relay || self.role == NodeRole::Seed)
            && matches!(
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
                    | MessageType::CertRotation { .. }
            )
        {
            self.peer_manager
                .broadcast_except(&message, &peer_addr)
                .await;
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
                debug!(
                    "P2P: Received BFT proposal height={} round={} from {}",
                    proposal.height, proposal.round, peer_addr
                );
                if let Err(e) = self.proposal_tx.try_send(proposal) {
                    warn!(
                        "P2P: Proposal channel full, dropping proposal from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::Prevote(prevote) => {
                debug!(
                    "P2P: Received BFT prevote height={} round={} from {}",
                    prevote.height, prevote.round, peer_addr
                );
                if let Err(e) = self.prevote_tx.try_send(prevote) {
                    warn!(
                        "P2P: Prevote channel full, dropping prevote from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::Precommit(precommit) => {
                debug!(
                    "P2P: Received BFT precommit height={} round={} from {}",
                    precommit.height, precommit.round, peer_addr
                );
                if let Err(e) = self.precommit_tx.try_send(precommit) {
                    warn!(
                        "P2P: Precommit channel full, dropping precommit from {} ({})",
                        peer_addr, e
                    );
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
                // Update DHT with received peer addresses
                {
                    use sha2::{Digest, Sha256};
                    if let Ok(mut table) = self.dht.lock() {
                        for pi in &peer_infos {
                            let mut hasher = Sha256::new();
                            hasher.update(pi.address.to_string().as_bytes());
                            let hash = hasher.finalize();
                            let mut node_id = [0u8; 32];
                            node_id.copy_from_slice(&hash);
                            table.insert(node_id, pi.address);
                        }
                    }
                }
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

                // Supplement with DHT nodes not already in peer manager
                if let Ok(table) = self.dht.lock() {
                    use sha2::{Digest, Sha256};
                    let mut hasher = Sha256::new();
                    hasher.update(peer_addr.to_string().as_bytes());
                    let hash = hasher.finalize();
                    let mut target_id = [0u8; 32];
                    target_id.copy_from_slice(&hash);
                    for entry in table.closest(&target_id, 10) {
                        if peer_infos.len() >= 50 {
                            break;
                        }
                        if !seen_addrs.contains(&entry.address) {
                            peer_infos.push(crate::message::PeerInfoMsg {
                                address: entry.address,
                                last_seen: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs(),
                                reputation: 500,
                                validator_pubkey: None,
                            });
                        }
                    }
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
                const MAX_BLOCK_RANGE: u64 = 500;
                let range = end_slot.saturating_sub(start_slot);
                if range > MAX_BLOCK_RANGE {
                    warn!(
                        "P2P: Rejecting block range request {}-{} from {} — range {} exceeds max {}",
                        start_slot, end_slot, peer_addr, range, MAX_BLOCK_RANGE
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
                debug!(
                    "P2P: Received block response for slot {} from {}",
                    block.header.slot, peer_addr
                );
                if let Err(e) = self.sync_block_tx.try_send(block) {
                    warn!(
                        "P2P: Sync block channel full, dropping block response from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::BlockRangeResponse { blocks } => {
                // AUDIT-FIX M12: Cap response size to match request limit
                if blocks.len() > 500 {
                    warn!(
                        "P2P: Rejecting oversized BlockRangeResponse from {} ({} blocks > 500)",
                        peer_addr,
                        blocks.len()
                    );
                    self.peer_manager.record_violation(&peer_addr);
                    return Ok(());
                }
                debug!(
                    "P2P: Received {} blocks in range response from {}",
                    blocks.len(),
                    peer_addr
                );
                for block in blocks {
                    if let Err(e) = self.sync_block_tx.try_send(block) {
                        warn!(
                            "P2P: Sync block channel full during range response from {} ({})",
                            peer_addr, e
                        );
                        break; // Stop sending remaining blocks — will be re-requested
                    }
                }
            }

            MessageType::StatusRequest => {
                // AUDIT-FIX C6: Rate-limit expensive requests (max 30/min)
                if !self.peer_manager.check_expensive_rate_limit(&peer_addr, 30) {
                    warn!("P2P: Rate-limiting status request from {}", peer_addr);
                    self.peer_manager.record_violation(&peer_addr);
                    return Ok(());
                }
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
                validator_set_hash,
                stake_pool_hash,
            } => {
                let report = ConsistencyReportMsg {
                    requester: peer_addr,
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
                // AUDIT-FIX C6: Rate-limit expensive requests (max 30/min)
                if !self.peer_manager.check_expensive_rate_limit(&peer_addr, 30) {
                    warn!("P2P: Rate-limiting snapshot request from {}", peer_addr);
                    self.peer_manager.record_violation(&peer_addr);
                    return Ok(());
                }
                let request = SnapshotRequestMsg {
                    requester: peer_addr,
                    kind,
                    state_snapshot_params: None,
                    is_meta_request: false,
                };
                if let Err(e) = self.snapshot_request_tx.try_send(request) {
                    warn!(
                        "P2P: Snapshot request channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
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
                chunk_index,
                chunk_size,
            } => {
                // AUDIT-FIX C6: Rate-limit expensive requests (max 30/min)
                if !self.peer_manager.check_expensive_rate_limit(&peer_addr, 30) {
                    warn!(
                        "P2P: Rate-limiting state snapshot request from {}",
                        peer_addr
                    );
                    self.peer_manager.record_violation(&peer_addr);
                    return Ok(());
                }
                let request = SnapshotRequestMsg {
                    requester: peer_addr,
                    kind: SnapshotKind::StateCheckpoint,
                    state_snapshot_params: Some((category, chunk_index, chunk_size)),
                    is_meta_request: false,
                };
                if let Err(e) = self.snapshot_request_tx.try_send(request) {
                    warn!(
                        "P2P: State snapshot request channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
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
                if let Err(e) = self.snapshot_request_tx.try_send(request) {
                    warn!(
                        "P2P: Checkpoint meta request channel full, dropping from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::CheckpointMetaResponse {
                slot,
                state_root,
                total_accounts,
            } => {
                let response = SnapshotResponseMsg {
                    requester: peer_addr,
                    kind: SnapshotKind::StateCheckpoint,
                    validator_set: None,
                    stake_pool: None,
                    state_snapshot_data: None,
                    checkpoint_meta: Some((slot, state_root, total_accounts)),
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
                .map(|message| {
                    moltchain_core::account::Keypair::verify(&pubkey, &message, &signature)
                })
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
                        moltchain_core::account::Keypair::verify(&pubkey, &message, &signature)
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
                // P3-5: Tag the peer as a validator in the peer manager
                self.peer_manager.mark_validator(&peer_addr, pubkey);

                // Update DHT with validator's peer address (use pubkey as node ID)
                if let Ok(mut table) = self.dht.lock() {
                    table.insert(pubkey.0, peer_addr);
                }
                let announcement = ValidatorAnnouncement {
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
                // AUDIT-FIX H12: Rate-limit FindNode (max 30/min)
                if !self.peer_manager.check_expensive_rate_limit(&peer_addr, 30) {
                    warn!("P2P: Rate-limiting FindNode request from {}", peer_addr);
                    self.peer_manager.record_violation(&peer_addr);
                    return Ok(());
                }
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
                        // AUDIT-FIX H13: Reject invalid/reserved IP addresses
                        let ip = addr.ip();
                        if ip.is_loopback()
                            || ip.is_unspecified()
                            || ip.is_multicast()
                            || matches!(ip, std::net::IpAddr::V4(v4) if v4.is_broadcast())
                        {
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

            // M-9: Certificate rotation — a peer announces it has generated a new
            // TLS certificate. Validate and update TOFU fingerprint store.
            MessageType::CertRotation {
                old_fingerprint,
                new_fingerprint,
                new_cert_der,
                rotation_proof: _,
                timestamp,
            } => {
                match self.peer_manager.handle_cert_rotation(
                    &peer_addr,
                    &old_fingerprint,
                    &new_fingerprint,
                    &new_cert_der,
                    timestamp,
                ) {
                    Ok(()) => {
                        info!(
                            "P2P: Certificate rotation accepted from {}",
                            peer_addr
                        );
                        // Re-gossip the rotation to other peers
                        let relay_msg = P2PMessage::new(
                            MessageType::CertRotation {
                                old_fingerprint,
                                new_fingerprint,
                                new_cert_der,
                                rotation_proof: vec![],
                                timestamp,
                            },
                            self.local_addr,
                        );
                        self.peer_manager.broadcast(relay_msg).await;
                    }
                    Err(e) => {
                        warn!("P2P: Certificate rotation rejected from {}: {}", peer_addr, e);
                    }
                }
            }
        }

        Ok(())
    }

    /// Broadcast a block to all peers
    pub async fn broadcast_block(&self, block: Block) {
        info!("🦞 P2P: Broadcasting block slot {}", block.header.slot);
        let message = P2PMessage::new(MessageType::Block(block), self.local_addr);
        self.peer_manager.broadcast(message).await;
    }

    /// Broadcast a vote to all peers
    pub async fn broadcast_vote(&self, vote: Vote) {
        info!("🦞 P2P: Broadcasting vote for slot {}", vote.slot);
        let message = P2PMessage::new(MessageType::Vote(vote), self.local_addr);
        self.peer_manager.broadcast(message).await;
    }

    /// Broadcast a transaction to all peers
    pub async fn broadcast_transaction(&self, tx: Transaction) {
        info!("🦞 P2P: Broadcasting transaction");
        let message = P2PMessage::new(MessageType::Transaction(tx), self.local_addr);
        self.peer_manager.broadcast(message).await;
    }

    /// Get connected peers
    pub fn get_peers(&self) -> Vec<SocketAddr> {
        self.peer_manager.get_peers()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
