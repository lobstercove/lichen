// Gossip Protocol for Peer Discovery

use crate::message::{MessageType, P2PMessage, PeerInfoMsg};
use crate::peer::PeerManager;
use crate::peer_store::PeerStore;
use moltchain_core::Pubkey;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, info};

/// Manages peer discovery and gossip
pub struct GossipManager {
    /// Peer manager
    peer_manager: Arc<PeerManager>,

    /// Bootstrap seed peers
    seed_peers: Vec<SocketAddr>,

    /// Gossip interval (seconds)
    gossip_interval: u64,

    /// Peer cleanup timeout (seconds)
    cleanup_timeout: u64,

    /// Durable peer store
    peer_store: Option<Arc<PeerStore>>,

    /// This node's externally reachable address
    local_addr: SocketAddr,

    /// This node's validator pubkey (None if not a validator)
    validator_pubkey: Option<Pubkey>,
}

impl GossipManager {
    /// Create new gossip manager.
    /// T4.6 fix: `local_addr` is required — no longer defaults to 127.0.0.1:8000.
    pub fn new(
        peer_manager: Arc<PeerManager>,
        seed_peers: Vec<SocketAddr>,
        gossip_interval: u64,
        cleanup_timeout: u64,
        peer_store: Option<Arc<PeerStore>>,
        local_addr: SocketAddr,
    ) -> Self {
        GossipManager {
            peer_manager,
            seed_peers,
            gossip_interval,
            cleanup_timeout,
            peer_store,
            local_addr,
            validator_pubkey: None,
        }
    }

    /// Create with explicit local address and validator identity
    pub fn with_identity(
        peer_manager: Arc<PeerManager>,
        seed_peers: Vec<SocketAddr>,
        gossip_interval: u64,
        cleanup_timeout: u64,
        peer_store: Option<Arc<PeerStore>>,
        local_addr: SocketAddr,
        validator_pubkey: Option<Pubkey>,
    ) -> Self {
        GossipManager {
            peer_manager,
            seed_peers,
            gossip_interval,
            cleanup_timeout,
            peer_store,
            local_addr,
            validator_pubkey,
        }
    }

    /// Start gossip protocol
    pub async fn start(&self) {
        info!("🦞 P2P: Starting gossip protocol");

        // Connect to seed peers
        for seed_addr in &self.seed_peers {
            if let Err(e) = self.peer_manager.connect_peer(*seed_addr).await {
                info!("P2P: Failed to connect to seed peer {}: {}", seed_addr, e);
            }
        }

        // Start periodic gossip
        let peer_manager = self.peer_manager.clone();
        let gossip_interval = self.gossip_interval;
        let local_addr = self.local_addr;
        let validator_pubkey = self.validator_pubkey;
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(gossip_interval));
            loop {
                interval.tick().await;
                Self::do_gossip(&peer_manager, local_addr, validator_pubkey).await;
            }
        });

        // Start peer cleanup
        let peer_manager = self.peer_manager.clone();
        let cleanup_timeout = self.cleanup_timeout;
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                peer_manager.cleanup_stale_peers(cleanup_timeout);
            }
        });
    }

    /// Perform gossip round
    async fn do_gossip(
        peer_manager: &Arc<PeerManager>,
        local_addr: SocketAddr,
        validator_pubkey: Option<Pubkey>,
    ) {
        let peers = peer_manager.get_peers();

        if peers.is_empty() {
            return;
        }

        // Create peer info list (M12 fix: cap at 50 peers to bound message size)
        let peer_infos: Vec<PeerInfoMsg> = peers
            .iter()
            .take(50)
            .map(|addr| PeerInfoMsg {
                address: *addr,
                last_seen: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                reputation: 500,
                validator_pubkey,
            })
            .collect();

        // Broadcast peer info
        let message = P2PMessage::new(MessageType::PeerInfo(peer_infos), local_addr);
        peer_manager.broadcast(message).await;

        info!("🦞 P2P: Gossip round complete ({} peers)", peers.len());
    }

    /// Handle incoming peer info
    pub async fn handle_peer_info(&self, peer_infos: Vec<PeerInfoMsg>) {
        let local_peers = self.peer_manager.get_peers();

        // Stop discovering if already at max peer count
        if local_peers.len() >= PeerManager::MAX_PEERS {
            return;
        }

        for peer_info in peer_infos {
            if let Some(store) = &self.peer_store {
                store.record_peer(peer_info.address);
            }

            // Skip if already connected
            if local_peers.contains(&peer_info.address) {
                continue;
            }

            // Skip if trying to connect to ourselves
            let is_self = peer_info.address == self.local_addr
                || (peer_info.address.ip().is_loopback()
                    && peer_info.address.port() == self.local_addr.port());

            if is_self {
                continue; // Don't connect to ourselves
            }

            info!("🦞 P2P: Discovered new peer {}", peer_info.address);
            if let Err(e) = self.peer_manager.connect_peer(peer_info.address).await {
                debug!("P2P: Failed to connect to discovered peer: {}", e);
            }
        }
    }
}
