// P2P Network Manager

use crate::gossip::GossipManager;
use crate::message::{MessageType, P2PMessage, SnapshotKind};
use crate::peer::PeerManager;
use crate::peer_store::PeerStore;
use moltchain_core::{Block, Pubkey, StakePool, Transaction, ValidatorSet, Vote};
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
    pub peer_store_path: Option<PathBuf>,
    pub max_known_peers: usize,
    /// Node role determines connection limits and relay behavior
    pub role: NodeRole,
    /// Maximum peer connections (if None, auto-set by role)
    pub max_peers: Option<usize>,
    /// Reserved relay/seed peer addresses that are never evicted
    pub reserved_relay_peers: Vec<String>,
}

impl Default for P2PConfig {
    fn default() -> Self {
        P2PConfig {
            listen_addr: "127.0.0.1:7001".parse().unwrap(),
            seed_peers: Vec::new(),
            gossip_interval: 10,
            cleanup_timeout: 300,
            peer_store_path: None,
            max_known_peers: 200,
            role: NodeRole::Validator,
            max_peers: None,
            reserved_relay_peers: Vec::new(),
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

    /// Outgoing block channel
    block_tx: mpsc::Sender<Block>,

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
}

impl P2PNetwork {
    /// Create new P2P network
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        config: P2PConfig,
        block_tx: mpsc::Sender<Block>,
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
        let reserved_addrs: Vec<SocketAddr> = config
            .reserved_relay_peers
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect();

        // Create peer manager with configurable max_peers and reserved peers
        let peer_manager = Arc::new(
            PeerManager::new(
                config.listen_addr,
                message_tx,
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

        Ok(P2PNetwork {
            peer_manager,
            gossip_manager,
            local_addr: config.listen_addr,
            role: config.role,
            message_rx,
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
                    | MessageType::Transaction(_)
                    | MessageType::ValidatorAnnounce { .. }
                    | MessageType::SlashingEvidence(_)
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
                let gm = self.gossip_manager.clone();
                tokio::spawn(async move {
                    gm.handle_peer_info(peer_infos).await;
                });
            }

            MessageType::PeerRequest => {
                debug!("P2P: Received peer request from {}", peer_addr);
                // AUDIT-FIX M3: Use actual peer scores, not hardcoded 500
                let peer_infos_raw = self.peer_manager.get_peer_infos();
                let peer_infos = peer_infos_raw
                    .iter()
                    .take(50) // Cap response size
                    .map(|(addr, score)| crate::message::PeerInfoMsg {
                        address: *addr,
                        last_seen: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                        reputation: ((*score as i128 + 20) * 1000 / 40).clamp(0, 1000) as u64,
                        validator_pubkey: None, // Populated when validator identity is known
                    })
                    .collect();

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
                const MAX_BLOCK_RANGE: u64 = 100;
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
                if let Err(e) = self.block_tx.try_send(block) {
                    warn!(
                        "P2P: Block channel full, dropping block response from {} ({})",
                        peer_addr, e
                    );
                }
            }

            MessageType::BlockRangeResponse { blocks } => {
                debug!(
                    "P2P: Received {} blocks in range response from {}",
                    blocks.len(),
                    peer_addr
                );
                for block in blocks {
                    if let Err(e) = self.block_tx.try_send(block) {
                        warn!(
                            "P2P: Block channel full during range response from {} ({})",
                            peer_addr, e
                        );
                        break; // Stop sending remaining blocks — will be re-requested
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
                // T2.3 fix: Verify Ed25519 signature before forwarding
                // Message = pubkey(32) + stake(8) + slot(8) + fingerprint(32) = 80 bytes
                let mut message = Vec::with_capacity(80);
                message.extend_from_slice(&pubkey.0);
                message.extend_from_slice(&stake.to_le_bytes());
                message.extend_from_slice(&current_slot.to_le_bytes());
                message.extend_from_slice(&machine_fingerprint);

                if !moltchain_core::account::Keypair::verify(&pubkey, &message, &signature) {
                    warn!(
                        "⚠️  P2P: Rejecting validator announcement from {} — invalid signature",
                        pubkey.to_base58()
                    );
                    return Ok(());
                }

                info!(
                    "🦞 P2P: Verified validator announcement from {}: {} (stake: {}, slot: {}, version: {})",
                    peer_addr,
                    pubkey.to_base58(),
                    stake,
                    current_slot,
                    if version.is_empty() { "unknown" } else { &version }
                );
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
