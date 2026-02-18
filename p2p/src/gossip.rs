// Gossip Protocol for Peer Discovery

use crate::message::{MessageType, P2PMessage, PeerInfoMsg};
use crate::peer::PeerManager;
use crate::peer_store::PeerStore;
use moltchain_core::Pubkey;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::interval;
use tracing::{debug, info, warn};

/// Minimum active peer count before aggressive reconnection to all known peers
const MIN_PEER_COUNT: usize = 2;

/// Maximum backoff interval (5 minutes)
const MAX_BACKOFF_SECS: u64 = 300;

/// Initial backoff interval (5 seconds)
const INITIAL_BACKOFF_SECS: u64 = 5;

/// Tracks reconnection attempts with exponential backoff so we don't
/// hammer unreachable peers on every gossip tick.
struct ReconnectTracker {
    /// Maps peer address → (next_attempt_unix_secs, current_backoff_secs)
    backoff: HashMap<SocketAddr, (u64, u64)>,
}

impl ReconnectTracker {
    fn new() -> Self {
        Self {
            backoff: HashMap::new(),
        }
    }

    /// Returns true if enough time has elapsed to retry this peer.
    fn should_attempt(&self, addr: &SocketAddr) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        match self.backoff.get(addr) {
            Some((next_attempt, _)) => now >= *next_attempt,
            None => true,
        }
    }

    /// Record a failed reconnection attempt — doubles the backoff (capped).
    fn record_failure(&mut self, addr: SocketAddr) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let current_backoff = self
            .backoff
            .get(&addr)
            .map(|(_, b)| *b)
            .unwrap_or(INITIAL_BACKOFF_SECS);
        let new_backoff = (current_backoff * 2).min(MAX_BACKOFF_SECS);
        self.backoff.insert(addr, (now + new_backoff, new_backoff));
    }

    /// Record a successful reconnection — reset backoff for this peer.
    fn record_success(&mut self, addr: SocketAddr) {
        self.backoff.remove(&addr);
    }
}

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

        // Start periodic gossip + reconnection
        let peer_manager = self.peer_manager.clone();
        let gossip_interval = self.gossip_interval;
        let local_addr = self.local_addr;
        let validator_pubkey = self.validator_pubkey;
        let seed_peers = self.seed_peers.clone();
        let peer_store = self.peer_store.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(gossip_interval));
            let mut reconnect_tracker = ReconnectTracker::new();
            loop {
                interval.tick().await;
                Self::do_gossip(&peer_manager, local_addr, validator_pubkey).await;

                // Reconnect to disconnected seed / known peers
                Self::reconnect_peers(
                    &peer_manager,
                    &seed_peers,
                    peer_store.as_ref(),
                    &mut reconnect_tracker,
                )
                .await;
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
                // AUDIT-FIX H17: Periodically prune expired entries from the
                // ban list to prevent unbounded memory growth from accumulating
                // banned peers that have served their timeout.
                peer_manager.prune_ban_list();
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
        // AUDIT-FIX M3: Use actual peer scores instead of hardcoded 500.
        // L4 note: last_seen is set to local clock. A malicious peer could relay
        // fabricated timestamps in incoming PeerInfoMsg to inflate reputation of
        // stale peers. Receivers should treat last_seen as untrusted advisory data
        // and not use it for critical decisions without independent verification.
        let peer_infos_raw = peer_manager.get_peer_infos();
        let peer_infos: Vec<PeerInfoMsg> = peer_infos_raw
            .iter()
            .take(50)
            .map(|(addr, score)| PeerInfoMsg {
                address: *addr,
                last_seen: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                // Map i64 score [-20..20] to u64 reputation [0..1000]:
                // score -20 → reputation 0, score 0 → reputation 500, score 20 → reputation 1000
                reputation: ((*score as i128 + 20) * 1000 / 40).clamp(0, 1000) as u64,
                validator_pubkey,
            })
            .collect();

        // Broadcast peer info
        let message = P2PMessage::new(MessageType::PeerInfo(peer_infos), local_addr);
        peer_manager.broadcast(message).await;

        info!("🦞 P2P: Gossip round complete ({} peers)", peers.len());
    }

    /// Attempt to reconnect to seed peers (and, if peer count is critically low,
    /// all known peers from the durable peer store).  Uses exponential backoff
    /// per-address so unreachable peers are not hammered every tick.
    async fn reconnect_peers(
        peer_manager: &Arc<PeerManager>,
        seed_peers: &[SocketAddr],
        peer_store: Option<&Arc<PeerStore>>,
        tracker: &mut ReconnectTracker,
    ) {
        let connected = peer_manager.get_peers();
        let connected_count = connected.len();

        // Build the set of candidate addresses to reconnect.
        // Always include seed peers; if below MIN_PEER_COUNT also include
        // all historically-known peers from the durable store.
        let mut candidates: Vec<SocketAddr> = seed_peers.to_vec();

        if connected_count < MIN_PEER_COUNT {
            if let Some(store) = peer_store {
                for addr in store.peers() {
                    if !candidates.contains(&addr) {
                        candidates.push(addr);
                    }
                }
            }
            debug!(
                "P2P reconnect: peer count {} < {}, trying all {} known peers",
                connected_count,
                MIN_PEER_COUNT,
                candidates.len()
            );
        }

        for addr in &candidates {
            // Already connected — make sure backoff is clear
            if connected.contains(addr) {
                tracker.record_success(*addr);
                continue;
            }

            // Don't reconnect to banned peers
            if peer_manager.is_banned(addr) {
                continue;
            }

            // Respect exponential backoff
            if !tracker.should_attempt(addr) {
                continue;
            }

            // Attempt reconnection
            match peer_manager.connect_peer(*addr).await {
                Ok(()) => {
                    info!("🦞 P2P: Reconnected to peer {}", addr);
                    tracker.record_success(*addr);
                }
                Err(e) => {
                    warn!("P2P: Failed to reconnect to peer {}: {}", addr, e);
                    tracker.record_failure(*addr);
                }
            }
        }
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
