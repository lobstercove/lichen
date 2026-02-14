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
use tracing::{error, info, warn};

/// P2P network configuration
#[derive(Debug, Clone)]
pub struct P2PConfig {
    pub listen_addr: SocketAddr,
    pub seed_peers: Vec<SocketAddr>,
    pub gossip_interval: u64,
    pub cleanup_timeout: u64,
    pub peer_store_path: Option<PathBuf>,
    pub max_known_peers: usize,
}

impl Default for P2PConfig {
    fn default() -> Self {
        P2PConfig {
            listen_addr: "127.0.0.1:8000".parse().unwrap(),
            seed_peers: Vec::new(),
            gossip_interval: 10,
            cleanup_timeout: 300,
            peer_store_path: None,
            max_known_peers: 200,
        }
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
}

/// Snapshot response from peer
#[derive(Debug, Clone)]
pub struct SnapshotResponseMsg {
    pub requester: SocketAddr,
    pub kind: SnapshotKind,
    pub validator_set: Option<ValidatorSet>,
    pub stake_pool: Option<StakePool>,
}

/// Main P2P network manager
pub struct P2PNetwork {
    /// Peer manager (public for broadcasting)
    pub peer_manager: Arc<PeerManager>,

    /// Gossip manager
    gossip_manager: Arc<GossipManager>,

    /// Local address
    local_addr: SocketAddr,

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
        info!("🦞 P2P: Initializing network on {}", config.listen_addr);

        // T4.7: Use bounded internal message channel to prevent memory exhaustion from peer floods.
        // Capacity 10K messages provides ~20MB buffer before backpressure kicks in.
        let (message_tx, message_rx) = mpsc::channel(10_000);

        let peer_store = config
            .peer_store_path
            .map(|path| Arc::new(PeerStore::new(path, config.max_known_peers)));

        // Create peer manager
        let peer_manager =
            Arc::new(PeerManager::new(config.listen_addr, message_tx, peer_store.clone()).await?);

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
        match message.msg_type {
            MessageType::Block(block) => {
                info!(
                    "🦞 P2P: Received block slot {} from {}",
                    block.header.slot, peer_addr
                );
                self.block_tx
                    .send(block)
                    .await
                    .map_err(|_| "Failed to send block to validator".to_string())?;
            }

            MessageType::Vote(vote) => {
                info!(
                    "🦞 P2P: Received vote for slot {} from {}",
                    vote.slot, peer_addr
                );
                self.vote_tx
                    .send(vote)
                    .await
                    .map_err(|_| "Failed to send vote to validator".to_string())?;
            }

            MessageType::Transaction(tx) => {
                info!("🦞 P2P: Received transaction from {}", peer_addr);
                self.transaction_tx
                    .send(tx)
                    .await
                    .map_err(|_| "Failed to send transaction to validator".to_string())?;
            }

            MessageType::PeerInfo(peer_infos) => {
                info!(
                    "🦞 P2P: Received peer info from {} ({} peers)",
                    peer_addr,
                    peer_infos.len()
                );
                self.gossip_manager.handle_peer_info(peer_infos).await;
            }

            MessageType::PeerRequest => {
                info!("🦞 P2P: Received peer request from {}", peer_addr);
                // Send our peer list
                let peers = self.peer_manager.get_peers();
                let peer_infos = peers
                    .iter()
                    .map(|addr| crate::message::PeerInfoMsg {
                        address: *addr,
                        last_seen: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                        reputation: 500,
                        validator_pubkey: None, // Populated when validator identity is known
                    })
                    .collect();

                let response = P2PMessage::new(MessageType::PeerInfo(peer_infos), self.local_addr);
                self.peer_manager.send_to_peer(&peer_addr, response).await?;
            }

            MessageType::Ping => {
                info!("🦞 P2P: Received ping from {}", peer_addr);
                let pong = P2PMessage::new(MessageType::Pong, self.local_addr);
                self.peer_manager.send_to_peer(&peer_addr, pong).await?;
            }

            MessageType::Pong => {
                info!("🦞 P2P: Received pong from {}", peer_addr);
            }

            MessageType::BlockRequest { slot } => {
                info!(
                    "🦞 P2P: Received block request for slot {} from {}",
                    slot, peer_addr
                );
                let request = BlockRangeRequestMsg {
                    start_slot: slot,
                    end_slot: slot,
                    requester: peer_addr,
                };
                self.block_range_request_tx
                    .send(request)
                    .await
                    .map_err(|_| "Failed to forward block request".to_string())?;
            }

            MessageType::BlockRangeRequest {
                start_slot,
                end_slot,
            } => {
                info!(
                    "🦞 P2P: Received block range request {}-{} from {}",
                    start_slot, end_slot, peer_addr
                );
                // Forward to validator to load blocks from state
                let request = BlockRangeRequestMsg {
                    start_slot,
                    end_slot,
                    requester: peer_addr,
                };
                self.block_range_request_tx
                    .send(request)
                    .await
                    .map_err(|_| "Failed to send block range request".to_string())?;
            }

            MessageType::BlockResponse(block) => {
                info!(
                    "🦞 P2P: Received block response for slot {} from {}",
                    block.header.slot, peer_addr
                );
                self.block_tx
                    .send(block)
                    .await
                    .map_err(|_| "Failed to send block to validator".to_string())?;
            }

            MessageType::BlockRangeResponse { blocks } => {
                info!(
                    "🦞 P2P: Received {} blocks from {}",
                    blocks.len(),
                    peer_addr
                );
                for block in blocks {
                    self.block_tx
                        .send(block)
                        .await
                        .map_err(|_| "Failed to send block to validator".to_string())?;
                }
            }

            MessageType::StatusRequest => {
                info!("🦞 P2P: Received status request from {}", peer_addr);
                let request = StatusRequestMsg {
                    requester: peer_addr,
                };
                self.status_request_tx
                    .send(request)
                    .await
                    .map_err(|_| "Failed to forward status request".to_string())?;
            }

            MessageType::StatusResponse {
                current_slot,
                total_blocks,
            } => {
                info!(
                    "🦞 P2P: Peer {} is at slot {} ({} blocks)",
                    peer_addr, current_slot, total_blocks
                );
                let response = StatusResponseMsg {
                    requester: peer_addr,
                    current_slot,
                    total_blocks,
                };
                self.status_response_tx
                    .send(response)
                    .await
                    .map_err(|_| "Failed to forward status response".to_string())?;
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
                self.consistency_report_tx
                    .send(report)
                    .await
                    .map_err(|_| "Failed to forward consistency report".to_string())?;
            }

            MessageType::SnapshotRequest { kind } => {
                let request = SnapshotRequestMsg {
                    requester: peer_addr,
                    kind,
                };
                self.snapshot_request_tx
                    .send(request)
                    .await
                    .map_err(|_| "Failed to forward snapshot request".to_string())?;
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
                };
                self.snapshot_response_tx
                    .send(response)
                    .await
                    .map_err(|_| "Failed to forward snapshot response".to_string())?;
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
                self.validator_announce_tx
                    .send(announcement)
                    .await
                    .map_err(|_| "Failed to send validator announcement".to_string())?;
            }

            MessageType::SlashingEvidence(evidence) => {
                info!(
                    "🦞 P2P: Received slashing evidence for {} from {}",
                    evidence.validator.to_base58(),
                    peer_addr
                );
                self.slashing_evidence_tx
                    .send(evidence)
                    .await
                    .map_err(|_| "Failed to forward slashing evidence".to_string())?;
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
