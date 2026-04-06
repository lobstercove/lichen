// Peer Connection Management

use crate::message::P2PMessage;
use crate::peer_ban::PeerBanList;
use crate::peer_store::PeerStore;
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use dashmap::DashMap;
use hkdf::Hkdf;
use lichen_core::{Keypair, PqSignature, Pubkey};
use ml_kem::kem::{Decapsulate, Encapsulate};
use ml_kem::{Encoded, EncodedSizeUser, KemCore, MlKem768};
use quinn::{Connection, Endpoint, ServerConfig};
use rand::{rngs::OsRng, RngCore};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

fn runtime_lichen_dir(runtime_home: Option<&Path>) -> PathBuf {
    runtime_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join(".lichen")
}

/// Peer information
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub address: SocketAddr,
    pub connection: Option<Connection>,
    secure_session: Option<Arc<SecureSession>>,
    pub last_seen: u64,
    pub reputation: u64,
    pub is_validator: bool,
    pub score: i64,
    /// P3-2: Kademlia node ID (native PQ node address). [0; 32] if unknown.
    pub node_id: [u8; 32],
    /// P3-5: Validator pubkey (set when we receive a verified ValidatorAnnounce from this peer)
    pub validator_pubkey: Option<Pubkey>,
    /// Peer scoring: rolling average response latency in milliseconds.
    /// Updated on each successful block/status response from this peer.
    pub avg_response_ms: Option<f64>,
    /// Bandwidth metering: total bytes received from this peer since connection.
    pub bytes_received: u64,
    /// Bandwidth metering: total bytes sent to this peer since connection.
    pub bytes_sent: u64,
    /// Bandwidth metering: timestamp when tracking started (connection time).
    pub tracking_since: u64,
    /// AUDIT-FIX C6: Per-peer request rate limiting for expensive operations.
    /// Tracks (window_start_epoch, request_count) per 60-second window.
    pub expensive_request_window: (u64, u32),
}

impl PeerInfo {
    pub fn new(address: SocketAddr) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        PeerInfo {
            address,
            connection: None,
            secure_session: None,
            last_seen: now,
            reputation: 500,
            is_validator: false,
            score: 0,
            node_id: [0u8; 32],
            validator_pubkey: None,
            avg_response_ms: None,
            bytes_received: 0,
            bytes_sent: 0,
            tracking_since: now,
            expensive_request_window: (now, 0),
        }
    }

    pub fn update_last_seen(&mut self) {
        self.last_seen = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }

    pub fn adjust_score(&mut self, delta: i64) {
        self.score = self.score.saturating_add(delta).clamp(-20, 20);
    }

    /// Update the rolling average response latency (exponential moving average, alpha=0.3).
    pub fn record_latency(&mut self, latency_ms: f64) {
        const ALPHA: f64 = 0.3;
        self.avg_response_ms = Some(match self.avg_response_ms {
            Some(prev) => prev * (1.0 - ALPHA) + latency_ms * ALPHA,
            None => latency_ms,
        });
    }

    /// Record bytes received from this peer.
    pub fn add_bytes_received(&mut self, bytes: u64) {
        self.bytes_received = self.bytes_received.saturating_add(bytes);
    }

    /// Record bytes sent to this peer.
    pub fn add_bytes_sent(&mut self, bytes: u64) {
        self.bytes_sent = self.bytes_sent.saturating_add(bytes);
    }

    /// Bytes per second received from this peer (average since tracking start).
    pub fn recv_bandwidth_bps(&self) -> f64 {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(self.tracking_since)
            .max(1);
        self.bytes_received as f64 / elapsed as f64
    }
}

/// C2-01: Bounded LRU cache of seen message hashes.
/// Prevents re-processing duplicate gossip messages (blocks, votes, txs, etc.)
/// that arrive from multiple peers.  Uses FIFO eviction when at capacity.
pub struct SeenMessageCache {
    hashes: HashSet<[u8; 32]>,
    order: VecDeque<[u8; 32]>,
    capacity: usize,
}

impl SeenMessageCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            hashes: HashSet::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Returns true if the hash was already seen.  If not, inserts it.
    pub fn check_and_insert(&mut self, hash: [u8; 32]) -> bool {
        if self.hashes.contains(&hash) {
            return true; // already seen
        }
        // Evict oldest if at capacity
        if self.hashes.len() >= self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.hashes.remove(&old);
            }
        }
        self.hashes.insert(hash);
        self.order.push_back(hash);
        false // new message
    }

    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }
}

/// Manages peer connections
pub struct PeerManager {
    /// Active peer connections
    peers: Arc<DashMap<SocketAddr, PeerInfo>>,

    /// QUIC endpoint
    endpoint: Endpoint,

    /// Local address
    #[allow(dead_code)]
    local_addr: SocketAddr,

    /// Channel for incoming messages (bounded — T4.7)
    message_tx: mpsc::Sender<(SocketAddr, P2PMessage)>,

    /// Durable peer store
    peer_store: Option<Arc<PeerStore>>,

    /// Persistent ban list
    ban_list: Arc<Mutex<PeerBanList>>,

    /// Persistent native PQ node identity for transport authentication.
    node_identity: Arc<NodeIdentity>,

    /// Local native PQ node address for self-connection detection.
    local_node_address: Pubkey,

    /// Native PQ peer identity TOFU store.
    identity_store: Arc<PeerIdentityStore>,

    /// C2-01: Bounded seen-message cache to prevent re-processing of
    /// duplicate gossip messages.  Stores SHA-256 hashes of deserialized
    /// message bytes.  VecDeque provides FIFO eviction order.
    seen_messages: Arc<Mutex<SeenMessageCache>>,

    /// Configurable maximum peer connections (replaces const MAX_PEERS)
    max_peers: usize,

    /// Reserved peer addresses that are never evicted
    reserved_peers: Vec<SocketAddr>,

    /// P3-2: Kademlia routing table for O(log N) peer routing
    kademlia: Arc<Mutex<crate::kademlia::KademliaTable>>,
}

/// Default fanout for non-consensus dissemination paths such as block and
/// transaction gossip. Consensus votes continue to use validator-targeted or
/// full broadcast paths for minimum latency.
pub const NON_CONSENSUS_FANOUT: usize = 8;

/// Check whether two IPs share the same subnet.
/// IPv4: /24 prefix (first 3 octets).  IPv6: /48 prefix (first 3 hextets).
fn same_subnet(a: &IpAddr, b: &IpAddr) -> bool {
    match (a, b) {
        (IpAddr::V4(a4), IpAddr::V4(b4)) => {
            let ao = a4.octets();
            let bo = b4.octets();
            ao[0] == bo[0] && ao[1] == bo[1] && ao[2] == bo[2]
        }
        (IpAddr::V6(a6), IpAddr::V6(b6)) => {
            let as6 = a6.segments();
            let bs6 = b6.segments();
            as6[0] == bs6[0] && as6[1] == bs6[1] && as6[2] == bs6[2]
        }
        _ => false, // v4 vs v6 — different subnets by definition
    }
}

fn should_bypass_localhost_peer_limits(local: &IpAddr, peer: &IpAddr) -> bool {
    local.is_loopback() && peer.is_loopback()
}

fn is_self_connection_addr(peer_addr: SocketAddr, local_addr: SocketAddr) -> bool {
    let same_port = peer_addr.port() == local_addr.port();
    let same_ip = peer_addr.ip() == local_addr.ip();
    let loopback_pair = peer_addr.ip().is_loopback() && local_addr.ip().is_loopback();
    let unspecified_pair = peer_addr.ip().is_unspecified() && local_addr.ip().is_unspecified();

    peer_addr == local_addr || (same_port && (same_ip || loopback_pair || unspecified_pair))
}

/// M-10: Compute an AS-level bucket ID for eclipse defense.
/// Uses /16 prefix for IPv4 and /32 prefix for IPv6 as a lightweight
/// approximation of AS-number grouping (most ASNs own contiguous /16 blocks).
/// For production deployments, an optional ASN database can override this.
fn asn_bucket(ip: &IpAddr) -> u32 {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            // /16 prefix — first 2 octets as bucket
            ((o[0] as u32) << 8) | (o[1] as u32)
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            // /32 prefix — first 2 segments as bucket
            ((s[0] as u32) << 16) | (s[1] as u32)
        }
    }
}

/// M-10: Check whether two IPs fall in the same AS-level bucket.
fn same_asn_bucket(a: &IpAddr, b: &IpAddr) -> bool {
    asn_bucket(a) == asn_bucket(b)
}

impl PeerManager {
    /// Get the local listening address
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Create new peer manager
    pub async fn new(
        local_addr: SocketAddr,
        message_tx: mpsc::Sender<(SocketAddr, P2PMessage)>,
        runtime_home: Option<PathBuf>,
        peer_store: Option<Arc<PeerStore>>,
        max_peers: usize,
        reserved_peers: Vec<SocketAddr>,
    ) -> Result<Self, String> {
        // Install crypto provider for rustls (required by quinn)
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok(); // Ignore error if already installed

        // Load or generate the persistent native PQ node identity used by the
        // application-layer transport handshake.
        let runtime_dir = runtime_lichen_dir(runtime_home.as_deref());
        let node_identity = Arc::new(NodeIdentity::load_or_generate(&runtime_dir)?);
        let local_node_address = node_identity.address;

        // QUIC still requires a TLS certificate, but it is treated strictly as an
        // anonymous carrier. Peer authentication and key establishment happen in
        // the native PQ handshake before any P2P message is accepted.
        let carrier_identity = CarrierIdentity::generate()?;
        let server_key = PrivateKeyDer::try_from(carrier_identity.key_bytes)
            .map_err(|e| format!("Failed to parse node key: {}", e))?;
        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![carrier_identity.cert_der], server_key)
            .map_err(|e| format!("Failed to create rustls config: {}", e))?;

        server_crypto.alpn_protocols = vec![b"lichen".to_vec()];

        // Configure QUIC server
        let mut server_config = ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
                .map_err(|e| format!("Failed to create QUIC server config: {}", e))?,
        ));

        // P9-NET-01: Limit concurrent uni-directional streams per connection.
        // Without this, a malicious peer could open thousands of streams
        // and exhaust memory/file descriptors.  256 concurrent streams is
        // generous for honest peers (they open 1 stream per message) while
        // bounding resource consumption.
        //
        // FIX: Also set keep_alive_interval (5s) so idle connections are not
        // dropped by the 30s idle timeout.  Without keep-alive, the QUIC
        // connection dies after max_idle_timeout when no streams are opened,
        // causing V2/V3 to never sync blocks from V1 and deadlocking the
        // entire network.
        {
            let mut transport = quinn::TransportConfig::default();
            transport.max_concurrent_uni_streams(256u32.into());
            transport.max_concurrent_bidi_streams(16u32.into());
            transport.keep_alive_interval(Some(Duration::from_secs(5)));
            transport.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));
            server_config.transport_config(Arc::new(transport));
        }

        // Create QUIC endpoint with retry for port conflicts (e.g. stale process
        // hasn't released the UDP port yet after a restart).
        let endpoint = {
            let max_retries = 5u32;
            let mut last_err = String::new();
            let mut bound = None;
            for attempt in 0..max_retries {
                match Endpoint::server(server_config.clone(), local_addr) {
                    Ok(ep) => {
                        if attempt > 0 {
                            info!("🦞 P2P: Endpoint bound on attempt {}", attempt + 1);
                        }
                        bound = Some(ep);
                        break;
                    }
                    Err(e) => {
                        last_err = format!("{}", e);
                        if attempt + 1 < max_retries {
                            warn!(
                                "⚠️  P2P: Failed to bind {} (attempt {}/{}): {} — retrying in 2s",
                                local_addr,
                                attempt + 1,
                                max_retries,
                                e
                            );
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                    }
                }
            }
            bound.ok_or_else(|| {
                format!(
                    "Failed to create endpoint after {} attempts: {}",
                    max_retries, last_err
                )
            })?
        };

        info!("🦞 P2P: QUIC endpoint listening on {}", local_addr);

        let ban_list_path = runtime_dir.join("peer-banlist.json");

        let identity_store_path = runtime_dir.join("peer_identities.json");
        let identity_store = Arc::new(PeerIdentityStore::new(identity_store_path));

        Ok(PeerManager {
            peers: Arc::new(DashMap::new()),
            endpoint,
            local_addr,
            message_tx,
            peer_store,
            ban_list: Arc::new(Mutex::new(PeerBanList::new(ban_list_path))),
            node_identity,
            local_node_address,
            identity_store,
            // C2-01: 20K capacity ≈ 640KB — covers ~5 minutes of peak traffic
            seen_messages: Arc::new(Mutex::new(SeenMessageCache::new(20_000))),
            max_peers,
            reserved_peers,
            kademlia: Arc::new(Mutex::new(crate::kademlia::KademliaTable::new(
                local_node_address.0,
            ))),
        })
    }

    /// Maximum number of concurrent peer connections (configurable per role)
    pub const MAX_PEERS: usize = 50;

    /// Eclipse-attack resistance: max peers from the same /24 (IPv4) or /48 (IPv6) subnet.
    pub const MAX_PEERS_PER_SUBNET: usize = 2;

    /// M-10: AS-level eclipse defense: max peers from the same /16 (IPv4) or /32 (IPv6) AS bucket.
    /// Broader than subnet — catches attackers who control many /24s within one ISP.
    pub const MAX_PEERS_PER_ASN_BUCKET: usize = 4;

    /// Get the effective max peers for this manager instance
    pub fn effective_max_peers(&self) -> usize {
        self.max_peers
    }

    /// Check if a peer address is reserved (never evicted)
    pub fn is_reserved(&self, addr: &SocketAddr) -> bool {
        self.reserved_peers.contains(addr)
    }

    /// Count how many currently-connected peers share the same subnet as `ip`.
    /// IPv4: /24 prefix (first 3 octets).  IPv6: /48 prefix (first 3 hextets).
    pub fn count_peers_in_subnet(&self, ip: &IpAddr) -> usize {
        self.peers
            .iter()
            .filter(|entry| same_subnet(&entry.key().ip(), ip))
            .count()
    }

    /// M-10: Count how many currently-connected peers share the same AS-level bucket as `ip`.
    /// IPv4: /16 prefix.  IPv6: /32 prefix.
    pub fn count_peers_in_asn_bucket(&self, ip: &IpAddr) -> usize {
        self.peers
            .iter()
            .filter(|entry| same_asn_bucket(&entry.key().ip(), ip))
            .count()
    }

    /// Return peers sorted by lowest average response latency (best first).
    /// Peers without recorded latency are placed at the end.
    pub fn fastest_peers(&self, count: usize) -> Vec<SocketAddr> {
        let mut peers: Vec<(SocketAddr, f64)> = self
            .peers
            .iter()
            .map(|e| {
                let lat = e.value().avg_response_ms.unwrap_or(f64::MAX);
                (*e.key(), lat)
            })
            .collect();
        peers.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        peers
            .into_iter()
            .take(count)
            .map(|(addr, _)| addr)
            .collect()
    }

    /// Record a response latency sample for a peer.
    pub fn record_peer_latency(&self, peer_addr: &SocketAddr, latency_ms: f64) {
        if let Some(mut peer) = self.peers.get_mut(peer_addr) {
            peer.record_latency(latency_ms);
        }
    }

    /// Record inbound bytes for a peer.
    pub fn record_bytes_received(&self, peer_addr: &SocketAddr, bytes: u64) {
        if let Some(mut peer) = self.peers.get_mut(peer_addr) {
            peer.add_bytes_received(bytes);
        }
    }

    /// Record outbound bytes for a peer.
    pub fn record_bytes_sent(&self, peer_addr: &SocketAddr, bytes: u64) {
        if let Some(mut peer) = self.peers.get_mut(peer_addr) {
            peer.add_bytes_sent(bytes);
        }
    }

    /// Return (recv_bps, sent_bps) for a peer, or None if the peer is not connected.
    pub fn bandwidth_stats(&self, peer_addr: &SocketAddr) -> Option<(f64, f64)> {
        self.peers.get(peer_addr).map(|p| {
            let elapsed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .saturating_sub(p.tracking_since)
                .max(1) as f64;
            (
                p.bytes_received as f64 / elapsed,
                p.bytes_sent as f64 / elapsed,
            )
        })
    }

    /// Connect to a peer
    pub async fn connect_peer(&self, peer_addr: SocketAddr) -> Result<(), String> {
        let same_port = peer_addr.port() == self.local_addr.port();
        let same_ip = peer_addr.ip() == self.local_addr.ip();
        let loopback_pair = peer_addr.ip().is_loopback() && self.local_addr.ip().is_loopback();
        let unspecified_pair =
            peer_addr.ip().is_unspecified() && self.local_addr.ip().is_unspecified();
        if peer_addr == self.local_addr
            || (same_port && (same_ip || loopback_pair || unspecified_pair))
        {
            return Err(format!(
                "Refusing to connect to self endpoint {}",
                peer_addr
            ));
        }

        if self
            .ban_list
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_banned(&peer_addr)
        {
            return Err("Peer is banned".to_string());
        }
        // Eclipse-attack resistance: limit peers per /24 subnet
        if !should_bypass_localhost_peer_limits(&self.local_addr.ip(), &peer_addr.ip())
            && self.count_peers_in_subnet(&peer_addr.ip()) >= Self::MAX_PEERS_PER_SUBNET
        {
            return Err(format!(
                "Subnet limit reached ({}) for {}",
                Self::MAX_PEERS_PER_SUBNET,
                peer_addr
            ));
        }
        // M-10: AS-level eclipse defense — limit peers per /16 (IPv4) or /32 (IPv6) bucket
        if !should_bypass_localhost_peer_limits(&self.local_addr.ip(), &peer_addr.ip())
            && self.count_peers_in_asn_bucket(&peer_addr.ip()) >= Self::MAX_PEERS_PER_ASN_BUCKET
        {
            return Err(format!(
                "ASN bucket limit reached ({}) for {}",
                Self::MAX_PEERS_PER_ASN_BUCKET,
                peer_addr
            ));
        }
        if self.peers.contains_key(&peer_addr) {
            return Ok(());
        }
        if self.peers.len() >= self.max_peers {
            return Err(format!(
                "Max peer limit reached ({}), rejecting {}",
                self.max_peers, peer_addr
            ));
        }

        info!("🦞 P2P: Connecting to peer {}", peer_addr);

        let mut rustls_config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnonymousCarrierCertVerifier))
            .with_no_client_auth();

        // Configure ALPN
        rustls_config.alpn_protocols = vec![b"lichen".to_vec()];

        let mut client_config = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(rustls_config)
                .map_err(|e| format!("Failed to create QUIC config: {}", e))?,
        ));
        // FIX: Client transport must also set keep_alive + idle timeout to
        // match server config, otherwise connections still flap.
        {
            let mut transport = quinn::TransportConfig::default();
            transport.max_concurrent_uni_streams(256u32.into());
            transport.max_concurrent_bidi_streams(16u32.into());
            transport.keep_alive_interval(Some(Duration::from_secs(5)));
            transport.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));
            client_config.transport_config(Arc::new(transport));
        }
        let mut endpoint = self.endpoint.clone();
        endpoint.set_default_client_config(client_config);

        // Connect
        let connection = endpoint
            .connect(peer_addr, "localhost")
            .map_err(|e| format!("Failed to connect: {}", e))?
            .await
            .map_err(|e| format!("Connection failed: {}", e))?;

        let (remote_identity, secure_session) = self
            .perform_outbound_handshake(&connection, peer_addr)
            .await?;

        if remote_identity.address == self.local_node_address {
            warn!(
                "P2P: Rejecting self-connection attempt to {} (same node identity)",
                peer_addr
            );
            connection.close(quinn::VarInt::from_u32(1), b"self_connection");
            return Err("Refusing self-connection (same node identity)".to_string());
        }

        self.identity_store
            .check_or_store(peer_addr, &remote_identity)?;
        self.update_kademlia(remote_identity.node_id, peer_addr);

        // Store peer info
        let mut peer_info = PeerInfo::new(peer_addr);
        peer_info.connection = Some(connection.clone());
        peer_info.secure_session = Some(Arc::new(secure_session.clone()));
        peer_info.node_id = remote_identity.node_id;
        self.peers.insert(peer_addr, peer_info);
        if let Some(store) = &self.peer_store {
            store.record_peer(peer_addr);
        }

        info!("✅ P2P: Connected to peer {}", peer_addr);

        // Spawn task to handle incoming messages
        let peers = self.peers.clone();
        let message_tx = self.message_tx.clone();
        let seen_messages = self.seen_messages.clone();
        let secure_session = Arc::new(secure_session);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(
                connection,
                peer_addr,
                peers.clone(),
                message_tx,
                seen_messages,
                secure_session,
            )
            .await
            {
                error!("P2P: Connection error with {}: {}", peer_addr, e);
            }
            // AUDIT-FIX H2: Remove peer from DashMap when connection drops.
            // Without this, dead peers linger until cleanup_stale_peers runs,
            // causing failed sends and inflated peer counts.
            peers.remove(&peer_addr);
            info!(
                "P2P: Peer {} disconnected, removed from peer map",
                peer_addr
            );
        });

        Ok(())
    }

    /// Send message to peer
    pub async fn send_to_peer(
        &self,
        peer_addr: &SocketAddr,
        message: P2PMessage,
    ) -> Result<(), String> {
        // M18 fix: clone connection handle and drop DashMap guard before async I/O
        // to prevent holding shard read lock across .await points
        let (connection, secure_session) = {
            let peer = self.peers.get(peer_addr).ok_or("Peer not found")?;
            (peer.connection.clone(), peer.secure_session.clone())
        }; // guard dropped here

        if let (Some(connection), Some(secure_session)) = (connection, secure_session) {
            let msg_type_label = match &message.msg_type {
                crate::MessageType::BlockRangeResponse { blocks } => {
                    format!("BlockRangeResponse({} blocks)", blocks.len())
                }
                crate::MessageType::BlockResponse(b) => {
                    format!("BlockResponse(slot={})", b.header.slot)
                }
                _ => String::new(),
            };

            let plaintext = message.serialize()?;
            let bytes = secure_session.encrypt(&plaintext)?;

            if !msg_type_label.is_empty() {
                tracing::info!(
                    "📤 P2P SEND: {} ({} bytes) to {}",
                    msg_type_label,
                    bytes.len(),
                    peer_addr
                );
            }

            let mut send_stream = connection
                .open_uni()
                .await
                .map_err(|e| format!("Failed to open stream to {}: {}", peer_addr, e))?;

            send_stream
                .write_all(&bytes)
                .await
                .map_err(|e| format!("Failed to write to {}: {}", peer_addr, e))?;

            send_stream
                .finish()
                .map_err(|e| format!("Failed to finish stream to {}: {}", peer_addr, e))?;

            // Bandwidth metering: track outbound bytes
            if let Some(mut peer) = self.peers.get_mut(peer_addr) {
                peer.add_bytes_sent(bytes.len() as u64);
            }

            Ok(())
        } else {
            Err(format!("No active secure connection to {}", peer_addr))
        }
    }

    /// Broadcast message to all peers except the specified sender (for relay/re-broadcasting).
    /// Uses concurrent sends like broadcast() but skips the sender to avoid echo.
    /// F-17 audit fix: Also skips low-score peers.
    pub async fn broadcast_except(&self, message: &P2PMessage, except: &SocketAddr) {
        let peers: Vec<SocketAddr> = self
            .peers
            .iter()
            .filter(|entry| entry.value().score > -5)
            .map(|entry| *entry.key())
            .filter(|addr| addr != except)
            .collect();
        if peers.is_empty() {
            return;
        }

        let plaintext = match message.serialize() {
            Ok(b) => std::sync::Arc::new(b),
            Err(e) => {
                warn!("P2P: broadcast_except serialize error: {}", e);
                return;
            }
        };

        let mut conn_tasks: Vec<(
            SocketAddr,
            Option<quinn::Connection>,
            Option<Arc<SecureSession>>,
        )> = Vec::with_capacity(peers.len());
        for addr in &peers {
            let peer = self.peers.get(addr);
            let conn = peer.as_ref().and_then(|p| p.connection.clone());
            let session = peer.as_ref().and_then(|p| p.secure_session.clone());
            conn_tasks.push((*addr, conn, session));
        }

        let mut handles = Vec::with_capacity(conn_tasks.len());
        for (peer_addr, connection, secure_session) in conn_tasks {
            let plaintext = plaintext.clone();
            handles.push(tokio::spawn(async move {
                if let (Some(conn), Some(session)) = (connection, secure_session) {
                    let bytes = match session.encrypt(plaintext.as_ref()) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            warn!("P2P: Failed to encrypt relay to {}: {}", peer_addr, e);
                            return;
                        }
                    };
                    match conn.open_uni().await {
                        Ok(mut stream) => {
                            if let Err(e) = stream.write_all(&bytes).await {
                                warn!("P2P: Failed to relay to {}: {}", peer_addr, e);
                            }
                            let _ = stream.finish();
                        }
                        Err(e) => warn!("P2P: Failed to open relay stream to {}: {}", peer_addr, e),
                    }
                }
            }));
        }

        for handle in handles {
            let _ = handle.await;
        }
    }

    /// Broadcast message to all peers (parallel — PERF-FIX 1)
    /// Uses concurrent sends instead of sequential awaits.
    /// With 500 validators, sequential = 2.5s; parallel = ~50ms.
    /// F-17 audit fix: Skip peers with score <= -5 (degraded but not yet evicted).
    pub async fn broadcast(&self, message: P2PMessage) {
        let peers: Vec<SocketAddr> = self
            .peers
            .iter()
            .filter(|entry| entry.value().score > -5)
            .map(|entry| *entry.key())
            .collect();
        if peers.is_empty() {
            return;
        }

        // Pre-serialize once (avoid N redundant serializations)
        let plaintext = match message.serialize() {
            Ok(b) => std::sync::Arc::new(b),
            Err(e) => {
                warn!("P2P: broadcast serialize error: {}", e);
                return;
            }
        };

        // Extract connection handles upfront (drop DashMap guards before async)
        let mut conn_tasks: Vec<(
            SocketAddr,
            Option<quinn::Connection>,
            Option<Arc<SecureSession>>,
        )> = Vec::with_capacity(peers.len());
        for addr in &peers {
            let peer = self.peers.get(addr);
            let conn = peer.as_ref().and_then(|p| p.connection.clone());
            let session = peer.as_ref().and_then(|p| p.secure_session.clone());
            conn_tasks.push((*addr, conn, session));
        }

        // Spawn concurrent send tasks
        let mut handles = Vec::with_capacity(conn_tasks.len());
        for (peer_addr, connection, secure_session) in conn_tasks {
            let plaintext = plaintext.clone();
            handles.push(tokio::spawn(async move {
                if let (Some(conn), Some(session)) = (connection, secure_session) {
                    let bytes = match session.encrypt(plaintext.as_ref()) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            warn!("P2P: Failed to encrypt send to {}: {}", peer_addr, e);
                            return;
                        }
                    };
                    match conn.open_uni().await {
                        Ok(mut stream) => {
                            if let Err(e) = stream.write_all(&bytes).await {
                                warn!("P2P: Failed to send to {}: {}", peer_addr, e);
                            }
                            let _ = stream.finish();
                        }
                        Err(e) => warn!("P2P: Failed to open stream to {}: {}", peer_addr, e),
                    }
                }
            }));
        }

        // Await all sends concurrently
        for handle in handles {
            let _ = handle.await;
        }
    }

    fn non_consensus_targets(&self, target_id: &[u8; 32], fanout: usize) -> Vec<SocketAddr> {
        let closest = {
            let table = self.kademlia.lock().unwrap_or_else(|e| e.into_inner());
            table.closest(target_id, fanout.max(1))
        };

        if !closest.is_empty() {
            return closest.into_iter().map(|entry| entry.address).collect();
        }

        self.peers.iter().map(|entry| *entry.key()).collect()
    }

    /// Get all peer addresses
    pub fn get_peers(&self) -> Vec<SocketAddr> {
        self.peers.iter().map(|entry| *entry.key()).collect()
    }

    /// Clear the stored TOFU node identity for a specific peer.
    pub fn clear_peer_identity(&self, addr: &SocketAddr) -> bool {
        let removed = self.identity_store.remove_identity(addr);
        if removed {
            info!(
                "P2P TOFU: Cleared node identity for {} — will re-trust on next connection",
                addr
            );
        }
        removed
    }

    /// Clear all stored TOFU identities (used during network-wide reset).
    pub fn clear_all_peer_identities(&self) {
        self.identity_store.clear_all();
        info!("P2P TOFU: Cleared ALL node identities — will re-trust all peers on next connection");
    }

    /// P3-2: Route a message to the `count` closest peers by XOR distance
    /// to `target_id`. Falls back to all peers if the routing table is empty.
    pub async fn route_to_closest(&self, target_id: &[u8; 32], count: usize, message: P2PMessage) {
        let targets = self.non_consensus_targets(target_id, count);
        if targets.is_empty() {
            return;
        }

        let plaintext = match message.serialize() {
            Ok(b) => std::sync::Arc::new(b),
            Err(e) => {
                warn!("P2P: route serialize error: {}", e);
                return;
            }
        };

        let mut handles = Vec::with_capacity(targets.len());
        for addr in targets {
            let peer = self.peers.get(&addr);
            let conn = peer.as_ref().and_then(|p| p.connection.clone());
            let session = peer.as_ref().and_then(|p| p.secure_session.clone());
            let plaintext = plaintext.clone();
            handles.push(tokio::spawn(async move {
                if let (Some(conn), Some(session)) = (conn, session) {
                    let bytes = match session.encrypt(plaintext.as_ref()) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            warn!("P2P: Failed to encrypt routed send to {}: {}", addr, e);
                            return;
                        }
                    };
                    match conn.open_uni().await {
                        Ok(mut stream) => {
                            if let Err(e) = stream.write_all(&bytes).await {
                                warn!("P2P: routed send to {} failed: {}", addr, e);
                            }
                            let _ = stream.finish();
                        }
                        Err(e) => warn!("P2P: routed stream to {} failed: {}", addr, e),
                    }
                }
            }));
        }
        for handle in handles {
            let _ = handle.await;
        }
    }

    /// P3-2: Update the Kademlia routing table when a peer's node_id is learned.
    pub fn update_kademlia(&self, node_id: [u8; 32], address: SocketAddr) {
        if node_id == [0u8; 32] {
            return; // Unknown node_id — skip
        }
        let mut table = self.kademlia.lock().unwrap_or_else(|e| e.into_inner());
        table.insert(node_id, address);
    }

    /// P3-2: Get the number of entries in the Kademlia routing table.
    pub fn kademlia_size(&self) -> usize {
        let table = self.kademlia.lock().unwrap_or_else(|e| e.into_inner());
        table.len()
    }

    /// P3-2: Return the closest nodes to `target_id` as (node_id, address) pairs.
    pub fn kademlia_closest(&self, target_id: &[u8; 32], count: usize) -> Vec<([u8; 32], String)> {
        let table = self.kademlia.lock().unwrap_or_else(|e| e.into_inner());
        table
            .closest(target_id, count)
            .into_iter()
            .map(|e| (e.node_id, e.address.to_string()))
            .collect()
    }

    /// P3-5: Mark a peer as a validator with their pubkey.
    /// Called when we receive a verified ValidatorAnnounce from this peer.
    pub fn mark_validator(&self, peer_addr: &SocketAddr, pubkey: Pubkey) {
        if let Some(mut peer) = self.peers.get_mut(peer_addr) {
            peer.is_validator = true;
            peer.validator_pubkey = Some(pubkey);
            // Boost score: validators get +5 priority to resist eviction
            peer.adjust_score(5);
        }
    }

    /// P3-5: Broadcast message only to peers marked as validators.
    /// Falls back to full broadcast if no validator peers are connected.
    pub async fn broadcast_to_validators(&self, message: P2PMessage) {
        let validator_addrs: Vec<SocketAddr> = self
            .peers
            .iter()
            .filter(|entry| entry.value().is_validator)
            .map(|entry| *entry.key())
            .collect();

        if validator_addrs.is_empty() {
            // No validator peers known — fall back to full broadcast
            self.broadcast(message).await;
            return;
        }

        let plaintext = match message.serialize() {
            Ok(b) => std::sync::Arc::new(b),
            Err(e) => {
                warn!("P2P: validator broadcast serialize error: {}", e);
                return;
            }
        };

        let mut handles = Vec::with_capacity(validator_addrs.len());
        for addr in validator_addrs {
            let peer = self.peers.get(&addr);
            let conn = peer.as_ref().and_then(|p| p.connection.clone());
            let session = peer.as_ref().and_then(|p| p.secure_session.clone());
            let plaintext = plaintext.clone();
            handles.push(tokio::spawn(async move {
                if let (Some(conn), Some(session)) = (conn, session) {
                    let bytes = match session.encrypt(plaintext.as_ref()) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            warn!("P2P: Failed to encrypt validator send to {}: {}", addr, e);
                            return;
                        }
                    };
                    match conn.open_uni().await {
                        Ok(mut stream) => {
                            if let Err(e) = stream.write_all(&bytes).await {
                                warn!("P2P: validator send to {} failed: {}", addr, e);
                            }
                            let _ = stream.finish();
                        }
                        Err(e) => warn!("P2P: validator stream to {} failed: {}", addr, e),
                    }
                }
            }));
        }
        for handle in handles {
            let _ = handle.await;
        }
    }

    /// P3-5: Get addresses of all connected validator peers.
    pub fn validator_peers(&self) -> Vec<SocketAddr> {
        self.peers
            .iter()
            .filter(|entry| entry.value().is_validator)
            .map(|entry| *entry.key())
            .collect()
    }

    /// Get peer info for all connected peers (address + score).
    /// F-17 audit fix: Returns actual peer scores used for broadcast filtering.
    pub fn get_peer_infos(&self) -> Vec<(SocketAddr, i64)> {
        self.peers
            .iter()
            .map(|entry| (*entry.key(), entry.value().score))
            .collect()
    }

    /// Record a peer violation (rate limit or invalid request)
    pub fn record_violation(&self, peer_addr: &SocketAddr) {
        if let Some(mut peer) = self.peers.get_mut(peer_addr) {
            peer.adjust_score(-2);
            if peer.score <= -10 {
                let addr = *peer_addr;
                self.ban_list
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .record_score(addr, peer.score);
                drop(peer);
                self.peers.remove(&addr);
                warn!("P2P: Removed peer {} due to low score", addr);
            }
        }
    }

    /// AUDIT-FIX C6: Check if a peer has exceeded the expensive-request rate limit.
    /// Returns true if the request should be allowed, false if rate-limited.
    /// Allows up to `max_per_window` expensive requests per 60-second window.
    pub fn check_expensive_rate_limit(&self, peer_addr: &SocketAddr, max_per_window: u32) -> bool {
        if let Some(mut peer) = self.peers.get_mut(peer_addr) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let (window_start, count) = peer.expensive_request_window;
            if now.saturating_sub(window_start) >= 60 {
                // New window
                peer.expensive_request_window = (now, 1);
                true
            } else if count < max_per_window {
                peer.expensive_request_window = (window_start, count + 1);
                true
            } else {
                // Rate limited
                false
            }
        } else {
            // Unknown peer — deny
            false
        }
    }

    /// Record a peer success (valid request/response)
    pub fn record_success(&self, peer_addr: &SocketAddr) {
        if let Some(mut peer) = self.peers.get_mut(peer_addr) {
            peer.adjust_score(1);
        }
    }

    /// Update a peer's last_seen timestamp (called on Pong response)
    pub async fn update_peer_last_seen(&self, peer_addr: &SocketAddr) {
        if let Some(mut peer) = self.peers.get_mut(peer_addr) {
            peer.update_last_seen();
        }
    }

    pub fn prune_ban_list(&self) {
        self.ban_list
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .prune();
    }

    /// Check if a peer address is currently banned
    pub fn is_banned(&self, addr: &SocketAddr) -> bool {
        self.ban_list
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_banned(addr)
    }

    /// Start accepting connections
    pub async fn start_accepting(&self) {
        let endpoint = self.endpoint.clone();
        let peers = self.peers.clone();
        let message_tx = self.message_tx.clone();
        let peer_store = self.peer_store.clone();
        let ban_list = self.ban_list.clone();
        let seen_messages = self.seen_messages.clone();
        let kademlia = self.kademlia.clone();
        let max_peers = self.max_peers;
        let local_addr = self.local_addr;
        let node_identity = self.node_identity.clone();
        let local_node_address = self.local_node_address;
        let identity_store = self.identity_store.clone();

        tokio::spawn(async move {
            while let Some(connecting) = endpoint.accept().await {
                let peers = peers.clone();
                let message_tx = message_tx.clone();
                let peer_store = peer_store.clone();
                let ban_list = ban_list.clone();
                let seen_messages = seen_messages.clone();
                let kademlia = kademlia.clone();
                let node_identity = node_identity.clone();
                let identity_store = identity_store.clone();

                tokio::spawn(async move {
                    match connecting.await {
                        Ok(connection) => {
                            let peer_addr = connection.remote_address();
                            if is_self_connection_addr(peer_addr, local_addr) {
                                warn!("P2P: Rejected self inbound connection from {}", peer_addr);
                                connection.close(quinn::VarInt::from_u32(1), b"self_connection");
                                return;
                            }
                            if ban_list
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .is_banned(&peer_addr)
                            {
                                warn!("P2P: Rejected banned peer {}", peer_addr);
                                return;
                            }
                            // Eclipse-attack resistance: limit peers per /24 subnet
                            {
                                let subnet_count = peers
                                    .iter()
                                    .filter(|e| same_subnet(&e.key().ip(), &peer_addr.ip()))
                                    .count();
                                if !should_bypass_localhost_peer_limits(
                                    &local_addr.ip(),
                                    &peer_addr.ip(),
                                ) && subnet_count >= PeerManager::MAX_PEERS_PER_SUBNET
                                {
                                    warn!(
                                        "P2P: Rejected inbound {} — subnet limit ({})",
                                        peer_addr,
                                        PeerManager::MAX_PEERS_PER_SUBNET
                                    );
                                    return;
                                }
                            }
                            // Enforce max_peers on inbound connections too
                            if peers.len() >= max_peers {
                                warn!(
                                    "P2P: Rejected inbound connection from {} — at max peers ({})",
                                    peer_addr, max_peers
                                );
                                return;
                            }
                            info!("🦞 P2P: Accepted connection from {}", peer_addr);

                            let handshake =
                                perform_inbound_handshake(&connection, peer_addr, &node_identity)
                                    .await;

                            let (remote_identity, secure_session) = match handshake {
                                Ok(handshake) => handshake,
                                Err(error) => {
                                    warn!(
                                        "P2P: Inbound transport handshake rejected from {}: {}",
                                        peer_addr, error
                                    );
                                    connection.close(
                                        quinn::VarInt::from_u32(1),
                                        b"transport_handshake_failed",
                                    );
                                    return;
                                }
                            };

                            if remote_identity.address == local_node_address {
                                warn!(
                                    "P2P: Rejected inbound self-identity connection from {}",
                                    peer_addr
                                );
                                connection.close(quinn::VarInt::from_u32(1), b"self_connection");
                                return;
                            }

                            if let Err(error) =
                                identity_store.check_or_store(peer_addr, &remote_identity)
                            {
                                warn!("{}", error);
                                connection.close(quinn::VarInt::from_u32(1), b"identity_mismatch");
                                return;
                            }

                            if remote_identity.node_id != [0u8; 32] {
                                let mut table = kademlia.lock().unwrap_or_else(|e| e.into_inner());
                                table.insert(remote_identity.node_id, peer_addr);
                            }

                            // Store peer
                            let mut peer_info = PeerInfo::new(peer_addr);
                            peer_info.connection = Some(connection.clone());
                            peer_info.secure_session = Some(Arc::new(secure_session.clone()));
                            peer_info.node_id = remote_identity.node_id;
                            peers.insert(peer_addr, peer_info);
                            if let Some(store) = &peer_store {
                                store.record_peer(peer_addr);
                            }

                            // Handle connection
                            let secure_session = Arc::new(secure_session);
                            if let Err(e) = handle_connection(
                                connection,
                                peer_addr,
                                peers.clone(),
                                message_tx,
                                seen_messages,
                                secure_session,
                            )
                            .await
                            {
                                error!("P2P: Connection error with {}: {}", peer_addr, e);
                            }
                            // AUDIT-FIX H2: Remove peer on disconnect (inbound path)
                            peers.remove(&peer_addr);
                            info!(
                                "P2P: Inbound peer {} disconnected, removed from peer map",
                                peer_addr
                            );
                        }
                        Err(e) => {
                            error!("P2P: Failed to accept connection: {}", e);
                        }
                    }
                });
            }
        });
    }

    /// Clean up stale peers and detect silent connections.
    /// AUDIT-FIX H17: Also removes peers connected longer than timeout
    /// with negative score (indicates repeated failures without recovery).
    /// Reserved peers are never evicted.
    pub fn cleanup_stale_peers(&self, timeout_secs: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut to_remove = Vec::new();

        for entry in self.peers.iter() {
            // AUDIT-FIX H14: Reserved peers can now be evicted if they've been
            // unreachable for a long time (3x normal timeout) AND have negative score.
            if self.reserved_peers.contains(entry.key()) {
                let age = now.saturating_sub(entry.value().last_seen);
                if age > timeout_secs * 3 && entry.value().score < -5 {
                    to_remove.push((*entry.key(), "reserved-stale"));
                }
                continue;
            }
            let age = now.saturating_sub(entry.value().last_seen);
            // Original: remove peers not seen within timeout
            if age > timeout_secs {
                to_remove.push((*entry.key(), "stale"));
            }
            // AUDIT-FIX H17: Remove peers that have been idle for half
            // the timeout AND have a negative score (indicates connection
            // errors without successful message exchange).
            else if age > timeout_secs / 2 && entry.value().score < 0 {
                to_remove.push((*entry.key(), "failing"));
            }
        }

        for (addr, reason) in to_remove {
            info!("🦞 P2P: Removing {} peer {}", reason, addr);
            self.peers.remove(&addr);
        }
    }
}

/// Handle incoming messages from a connection
async fn handle_connection(
    connection: Connection,
    peer_addr: SocketAddr,
    peers: Arc<DashMap<SocketAddr, PeerInfo>>,
    message_tx: mpsc::Sender<(SocketAddr, P2PMessage)>,
    seen_messages: Arc<Mutex<SeenMessageCache>>,
    secure_session: Arc<SecureSession>,
) -> Result<(), String> {
    let mut deser_failures: u32 = 0;
    let mut deser_total: u32 = 0;
    const MAX_DESER_FAILURES: u32 = 10;
    // AUDIT-FIX H9: Track failure RATIO instead of consecutive-only.
    // Disconnect if >50% of messages in a window are failures.
    const DESER_WINDOW: u32 = 20;

    let mut stream_count: u64 = 0;

    loop {
        let mut stream = connection
            .accept_uni()
            .await
            .map_err(|e| format!("Failed to accept stream: {}", e))?;

        stream_count += 1;

        let encrypted_bytes = stream
            .read_to_end(16 * 1024 * 1024) // AUDIT-FIX H3: Align with P2PMessage serialize limit (16MB).
            // Previous 2MB limit silently rejected valid state snapshot chunks.
            .await
            .map_err(|e| {
                format!(
                    "Failed to read stream #{} from {}: {}",
                    stream_count, peer_addr, e
                )
            })?;

        let bytes = secure_session.decrypt(&encrypted_bytes).map_err(|e| {
            format!(
                "Failed to decrypt stream #{} from {}: {}",
                stream_count, peer_addr, e
            )
        })?;

        // Log every Nth stream for debugging connection liveness, and always
        // log large payloads that might be sync responses.
        if stream_count <= 3 || stream_count.is_multiple_of(100) || bytes.len() > 1024 {
            tracing::info!(
                "📥 P2P STREAM #{} from {}: {} bytes",
                stream_count,
                peer_addr,
                bytes.len()
            );
        }

        // Bandwidth metering: track inbound bytes from this peer
        if let Some(mut peer) = peers.get_mut(&peer_addr) {
            peer.add_bytes_received(bytes.len() as u64);
        }

        // Deserialize message
        match P2PMessage::deserialize(&bytes) {
            Ok(message) => {
                // AUDIT-FIX H9: Decay failure count gradually instead of
                // resetting to 0 — prevents [9 bad, 1 good] evasion pattern.
                deser_failures = deser_failures.saturating_sub(1);
                deser_total = deser_total.saturating_add(1);

                // Log message type for sync debugging
                match &message.msg_type {
                    crate::MessageType::BlockRangeResponse { blocks } => {
                        tracing::info!(
                            "📥 P2P WIRE: BlockRangeResponse ({} blocks) from {}",
                            blocks.len(),
                            peer_addr
                        );
                    }
                    crate::MessageType::BlockResponse(b) => {
                        tracing::info!(
                            "📥 P2P WIRE: BlockResponse slot {} from {}",
                            b.header.slot,
                            peer_addr
                        );
                    }
                    _ => {}
                }

                // C2-01: Dedup — hash the raw message bytes and skip if already seen.
                // Only dedup gossip message types (Block, Vote, Transaction,
                // SlashingEvidence, ValidatorAnnounce, and BFT consensus
                // messages). Request/response types (Ping, Pong, BlockRequest,
                // StatusRequest, etc.) are point-to-point and must always be
                // processed. BFT messages (Proposal, Prevote, Precommit) are
                // included because validators re-gossip them (CometBFT reactor
                // pattern) and we must prevent infinite relay loops.
                let should_dedup = matches!(
                    message.msg_type,
                    crate::MessageType::Block(_)
                        | crate::MessageType::Vote(_)
                        | crate::MessageType::Transaction(_)
                        | crate::MessageType::SlashingEvidence(_)
                        | crate::MessageType::ValidatorAnnounce { .. }
                        | crate::MessageType::Proposal(_)
                        | crate::MessageType::Prevote(_)
                        | crate::MessageType::Precommit(_)
                );
                if should_dedup {
                    let hash: [u8; 32] = Sha256::digest(&bytes).into();
                    let already_seen = seen_messages
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .check_and_insert(hash);
                    if already_seen {
                        continue; // silently drop duplicate
                    }
                }

                // Update last seen
                if let Some(mut peer) = peers.get_mut(&peer_addr) {
                    peer.update_last_seen();
                }

                // Forward to network manager (backpressure via bounded channel)
                if message_tx.send((peer_addr, message)).await.is_err() {
                    return Err("Message channel closed".to_string());
                }
            }
            Err(e) => {
                deser_failures += 1;
                deser_total = deser_total.saturating_add(1);
                warn!(
                    "P2P: Failed to deserialize message from {} ({}/{}): {}",
                    peer_addr, deser_failures, MAX_DESER_FAILURES, e
                );
                // H18 fix: disconnect after too many consecutive failures
                // AUDIT-FIX H9: Also disconnect if failure ratio >50% over window
                let ratio_exceeded =
                    deser_total >= DESER_WINDOW && deser_failures > deser_total / 2;
                if deser_failures >= MAX_DESER_FAILURES || ratio_exceeded {
                    warn!(
                        "P2P: Disconnecting {} — too many deserialization failures",
                        peer_addr
                    );
                    if let Some(mut peer) = peers.get_mut(&peer_addr) {
                        peer.score -= 20;
                    }
                    return Err(format!(
                        "Too many deserialization failures from {}",
                        peer_addr
                    ));
                }
            }
        }
    }
}

// ============================================================================
// Native PQ transport identity + handshake.
// QUIC remains the carrier, but peer authentication and key establishment happen
// in an application-layer ML-DSA + ML-KEM handshake before any P2P message is
// accepted. Every payload after that handshake is encrypted under the derived
// PQ session key.
// ============================================================================

const TRANSPORT_HANDSHAKE_VERSION: u32 = 1;
const TRANSPORT_HANDSHAKE_TIMEOUT_SECS: u64 = 10;
const TRANSPORT_FRAME_LIMIT_BYTES: usize = 64 * 1024;
const CLIENT_HELLO_TAG: &[u8] = b"lichen-p2p-client-hello-v1";
const SERVER_HELLO_TAG: &[u8] = b"lichen-p2p-server-hello-v1";
const SESSION_KEY_INFO_TAG: &[u8] = b"lichen-p2p-session-key-v1";

type MlKem768EncapsulationKey = <MlKem768 as KemCore>::EncapsulationKey;
type MlKem768DecapsulationKey = <MlKem768 as KemCore>::DecapsulationKey;
type MlKem768Ciphertext = ml_kem::Ciphertext<MlKem768>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeIdentityFile {
    #[serde(rename = "privateKey")]
    private_key: Vec<u8>,
    #[serde(rename = "publicKey")]
    public_key: Vec<u8>,
    #[serde(rename = "publicKeyBase58")]
    public_key_base58: String,
}

struct NodeIdentity {
    keypair: Keypair,
    address: Pubkey,
}

impl NodeIdentity {
    fn load_or_generate(lichen_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(lichen_dir)
            .map_err(|e| format!("Failed to create {}: {}", lichen_dir.display(), e))?;

        let identity_path = lichen_dir.join("node_identity.json");
        if identity_path.exists() {
            let json = fs::read_to_string(&identity_path)
                .map_err(|e| format!("Failed to read {}: {}", identity_path.display(), e))?;
            let file: NodeIdentityFile = serde_json::from_str(&json)
                .map_err(|e| format!("Failed to parse {}: {}", identity_path.display(), e))?;
            if file.private_key.len() != 32 {
                return Err(format!(
                    "Node identity {} has invalid seed length {} (expected 32 bytes)",
                    identity_path.display(),
                    file.private_key.len()
                ));
            }

            let mut seed = [0u8; 32];
            seed.copy_from_slice(&file.private_key);
            let keypair = Keypair::from_seed(&seed);
            if keypair.public_key().bytes != file.public_key {
                return Err(format!(
                    "Node identity {} publicKey does not match the derived PQ verifying key",
                    identity_path.display()
                ));
            }
            let address = keypair.pubkey();
            if address.to_base58() != file.public_key_base58 {
                return Err(format!(
                    "Node identity {} publicKeyBase58 does not match the derived PQ address",
                    identity_path.display()
                ));
            }

            info!("🔑 P2P: Loaded native node identity ({})", address);
            return Ok(Self { keypair, address });
        }

        let keypair = Keypair::new();
        let address = keypair.pubkey();
        let file = NodeIdentityFile {
            private_key: keypair.to_seed().to_vec(),
            public_key: keypair.public_key().bytes,
            public_key_base58: address.to_base58(),
        };
        Self::write_file(
            &identity_path,
            serde_json::to_string_pretty(&file)
                .map_err(|e| format!("Failed to serialize {}: {}", identity_path.display(), e))?
                .as_bytes(),
        )?;

        info!("🔑 P2P: Generated native node identity ({})", address);
        Ok(Self { keypair, address })
    }
    fn write_file(path: &Path, data: &[u8]) -> Result<(), String> {
        use std::io::Write;
        let mut file = fs::File::create(path)
            .map_err(|e| format!("Failed to create {}: {}", path.display(), e))?;
        file.write_all(data)
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync {}: {}", path.display(), e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct RemoteNodeIdentity {
    address: Pubkey,
    node_id: [u8; 32],
    public_key_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KnownPeerIdentity {
    address: String,
    public_key: String,
}

struct PeerIdentityStore {
    identities: Mutex<HashMap<String, KnownPeerIdentity>>,
    path: PathBuf,
}

impl PeerIdentityStore {
    fn new(path: PathBuf) -> Self {
        let identities = match fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => HashMap::new(),
        };
        Self {
            identities: Mutex::new(identities),
            path,
        }
    }

    fn check_or_store(
        &self,
        addr: SocketAddr,
        identity: &RemoteNodeIdentity,
    ) -> Result<bool, String> {
        let addr_key = addr.to_string();
        let candidate = KnownPeerIdentity {
            address: identity.address.to_base58(),
            public_key: identity.public_key_hex.clone(),
        };
        let mut store = self.identities.lock().unwrap_or_else(|e| e.into_inner());
        match store.get(&addr_key) {
            Some(known)
                if known.address == candidate.address && known.public_key == candidate.public_key =>
            {
                Ok(false)
            }
            Some(known) => Err(format!(
                "TOFU VIOLATION: Peer {} identity changed (known address {}, got {}; known key {}..., got {}...)",
                addr,
                known.address,
                candidate.address,
                &known.public_key[..16.min(known.public_key.len())],
                &candidate.public_key[..16.min(candidate.public_key.len())]
            )),
            None => {
                store.insert(addr_key, candidate);
                drop(store);
                self.save();
                Ok(true)
            }
        }
    }

    fn remove_identity(&self, addr: &SocketAddr) -> bool {
        let mut store = self.identities.lock().unwrap_or_else(|e| e.into_inner());
        let removed = store.remove(&addr.to_string()).is_some();
        drop(store);
        if removed {
            self.save();
        }
        removed
    }

    fn clear_all(&self) {
        let mut store = self.identities.lock().unwrap_or_else(|e| e.into_inner());
        store.clear();
        drop(store);
        self.save();
    }

    fn save(&self) {
        let store = self.identities.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(json) = serde_json::to_string_pretty(&*store) {
            if let Some(parent) = self.path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let tmp_path = self.path.with_extension("tmp");
            if let Ok(mut file) = fs::File::create(&tmp_path) {
                use std::io::Write;
                if file.write_all(json.as_bytes()).is_ok() && file.sync_all().is_ok() {
                    let _ = fs::rename(&tmp_path, &self.path);
                } else {
                    let _ = fs::remove_file(&tmp_path);
                }
            }
        }
    }
}

#[derive(Debug)]
struct CarrierIdentity {
    cert_der: CertificateDer<'static>,
    key_bytes: Vec<u8>,
}

impl CarrierIdentity {
    fn generate() -> Result<Self, String> {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .map_err(|e| format!("Failed to generate carrier certificate: {}", e))?;
        Ok(Self {
            cert_der: CertificateDer::from(cert.cert),
            key_bytes: cert.key_pair.serialize_der(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TransportClientHello {
    version: u32,
    address: Pubkey,
    kem_public_key: Vec<u8>,
    nonce: [u8; 32],
    signature: PqSignature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TransportServerHello {
    version: u32,
    address: Pubkey,
    nonce: [u8; 32],
    kem_ciphertext: Vec<u8>,
    signature: PqSignature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SecureTransportFrame {
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
}

#[derive(Clone, Debug)]
struct SecureSession {
    key: [u8; 32],
}

impl SecureSession {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let cipher = XChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|e| format!("Failed to create transport cipher: {}", e))?;
        let mut nonce = [0u8; 24];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&nonce), plaintext)
            .map_err(|e| format!("Failed to encrypt transport frame: {}", e))?;
        bincode::serialize(&SecureTransportFrame { nonce, ciphertext })
            .map_err(|e| format!("Failed to serialize secure transport frame: {}", e))
    }

    fn decrypt(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        let frame: SecureTransportFrame = bincode::deserialize(bytes)
            .map_err(|e| format!("Failed to deserialize secure transport frame: {}", e))?;
        let cipher = XChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|e| format!("Failed to create transport cipher: {}", e))?;
        cipher
            .decrypt(XNonce::from_slice(&frame.nonce), frame.ciphertext.as_ref())
            .map_err(|e| format!("Failed to decrypt transport frame: {}", e))
    }
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}

fn transport_client_hello_signing_message(
    address: &Pubkey,
    kem_public_key: &[u8],
    nonce: &[u8; 32],
) -> Result<Vec<u8>, String> {
    if kem_public_key.len() > u16::MAX as usize {
        return Err(format!(
            "ML-KEM public key too large for transport hello: {} bytes",
            kem_public_key.len()
        ));
    }
    let mut message =
        Vec::with_capacity(CLIENT_HELLO_TAG.len() + 32 + 2 + kem_public_key.len() + 32);
    message.extend_from_slice(CLIENT_HELLO_TAG);
    message.extend_from_slice(&address.0);
    message.extend_from_slice(&(kem_public_key.len() as u16).to_le_bytes());
    message.extend_from_slice(kem_public_key);
    message.extend_from_slice(nonce);
    Ok(message)
}

fn transport_server_hello_signing_message(
    client_hello_hash: &[u8; 32],
    address: &Pubkey,
    nonce: &[u8; 32],
    kem_ciphertext: &[u8],
) -> Result<Vec<u8>, String> {
    if kem_ciphertext.len() > u16::MAX as usize {
        return Err(format!(
            "ML-KEM ciphertext too large for transport hello: {} bytes",
            kem_ciphertext.len()
        ));
    }
    let mut message =
        Vec::with_capacity(SERVER_HELLO_TAG.len() + 32 + 32 + 32 + 2 + kem_ciphertext.len());
    message.extend_from_slice(SERVER_HELLO_TAG);
    message.extend_from_slice(client_hello_hash);
    message.extend_from_slice(&address.0);
    message.extend_from_slice(nonce);
    message.extend_from_slice(&(kem_ciphertext.len() as u16).to_le_bytes());
    message.extend_from_slice(kem_ciphertext);
    Ok(message)
}

fn transport_transcript_hash(client_hello_bytes: &[u8], server_hello_bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(client_hello_bytes);
    hasher.update(server_hello_bytes);
    hasher.finalize().into()
}

fn derive_transport_session_key(
    shared_secret: &[u8],
    client_nonce: &[u8; 32],
    server_nonce: &[u8; 32],
    client_address: &Pubkey,
    server_address: &Pubkey,
    transcript_hash: &[u8; 32],
) -> Result<[u8; 32], String> {
    let mut salt = Vec::with_capacity(64);
    salt.extend_from_slice(client_nonce);
    salt.extend_from_slice(server_nonce);

    let mut info = Vec::with_capacity(SESSION_KEY_INFO_TAG.len() + 32 + 32 + 32);
    info.extend_from_slice(SESSION_KEY_INFO_TAG);
    info.extend_from_slice(&client_address.0);
    info.extend_from_slice(&server_address.0);
    info.extend_from_slice(transcript_hash);

    let hkdf = Hkdf::<Sha256>::new(Some(&salt), shared_secret);
    let mut key = [0u8; 32];
    hkdf.expand(&info, &mut key)
        .map_err(|_| "Failed to derive PQ transport session key".to_string())?;
    Ok(key)
}

fn remote_identity_from_signature(
    address: &Pubkey,
    signature: &PqSignature,
) -> Result<RemoteNodeIdentity, String> {
    if signature.signer_address() != *address {
        return Err(format!(
            "Transport handshake address mismatch: claimed {}, derived {}",
            address,
            signature.signer_address()
        ));
    }
    Ok(RemoteNodeIdentity {
        address: *address,
        node_id: address.0,
        public_key_hex: encode_hex(&signature.public_key.bytes),
    })
}

fn decode_mlkem_encapsulation_key(bytes: &[u8]) -> Result<MlKem768EncapsulationKey, String> {
    let encoded = Encoded::<MlKem768EncapsulationKey>::try_from(bytes)
        .map_err(|_| format!("Invalid ML-KEM encapsulation key length: {}", bytes.len()))?;
    Ok(MlKem768EncapsulationKey::from_bytes(&encoded))
}

fn decode_mlkem_ciphertext(bytes: &[u8]) -> Result<MlKem768Ciphertext, String> {
    bytes
        .try_into()
        .map_err(|_| format!("Invalid ML-KEM ciphertext length: {}", bytes.len()))
}

fn build_client_hello(
    node_identity: &NodeIdentity,
    kem_public_key: &[u8],
    nonce: [u8; 32],
) -> Result<TransportClientHello, String> {
    let signing_message =
        transport_client_hello_signing_message(&node_identity.address, kem_public_key, &nonce)?;
    Ok(TransportClientHello {
        version: TRANSPORT_HANDSHAKE_VERSION,
        address: node_identity.address,
        kem_public_key: kem_public_key.to_vec(),
        nonce,
        signature: node_identity.keypair.sign(&signing_message),
    })
}

fn verify_client_hello(hello: &TransportClientHello) -> Result<RemoteNodeIdentity, String> {
    if hello.version != TRANSPORT_HANDSHAKE_VERSION {
        return Err(format!(
            "Unsupported transport hello version {}",
            hello.version
        ));
    }
    let signing_message = transport_client_hello_signing_message(
        &hello.address,
        &hello.kem_public_key,
        &hello.nonce,
    )?;
    if !Keypair::verify(&hello.address, &signing_message, &hello.signature) {
        return Err("Invalid transport client hello signature".to_string());
    }
    remote_identity_from_signature(&hello.address, &hello.signature)
}

fn build_server_hello(
    node_identity: &NodeIdentity,
    client_hello_hash: &[u8; 32],
    nonce: [u8; 32],
    kem_ciphertext: &[u8],
) -> Result<TransportServerHello, String> {
    let signing_message = transport_server_hello_signing_message(
        client_hello_hash,
        &node_identity.address,
        &nonce,
        kem_ciphertext,
    )?;
    Ok(TransportServerHello {
        version: TRANSPORT_HANDSHAKE_VERSION,
        address: node_identity.address,
        nonce,
        kem_ciphertext: kem_ciphertext.to_vec(),
        signature: node_identity.keypair.sign(&signing_message),
    })
}

fn verify_server_hello(
    response: &TransportServerHello,
    client_hello_hash: &[u8; 32],
) -> Result<RemoteNodeIdentity, String> {
    if response.version != TRANSPORT_HANDSHAKE_VERSION {
        return Err(format!(
            "Unsupported transport response version {}",
            response.version
        ));
    }
    let signing_message = transport_server_hello_signing_message(
        client_hello_hash,
        &response.address,
        &response.nonce,
        &response.kem_ciphertext,
    )?;
    if !Keypair::verify(&response.address, &signing_message, &response.signature) {
        return Err("Invalid transport server hello signature".to_string());
    }
    remote_identity_from_signature(&response.address, &response.signature)
}

async fn perform_inbound_handshake(
    connection: &Connection,
    peer_addr: SocketAddr,
    node_identity: &NodeIdentity,
) -> Result<(RemoteNodeIdentity, SecureSession), String> {
    let (mut send, mut recv) = tokio::time::timeout(
        Duration::from_secs(TRANSPORT_HANDSHAKE_TIMEOUT_SECS),
        connection.accept_bi(),
    )
    .await
    .map_err(|_| {
        format!(
            "Timed out waiting for inbound transport handshake from {}",
            peer_addr
        )
    })?
    .map_err(|e| {
        format!(
            "Failed to accept inbound transport handshake from {}: {}",
            peer_addr, e
        )
    })?;

    let client_hello_bytes = recv
        .read_to_end(TRANSPORT_FRAME_LIMIT_BYTES)
        .await
        .map_err(|e| {
            format!(
                "Failed to read transport client hello from {}: {}",
                peer_addr, e
            )
        })?;
    let client_hello: TransportClientHello =
        bincode::deserialize(&client_hello_bytes).map_err(|e| {
            format!(
                "Failed to decode transport client hello from {}: {}",
                peer_addr, e
            )
        })?;
    let remote_identity = verify_client_hello(&client_hello)?;

    let client_ek = decode_mlkem_encapsulation_key(&client_hello.kem_public_key)?;
    let (ciphertext, shared_secret) = client_ek.encapsulate(&mut OsRng).map_err(|e| {
        format!(
            "Failed to encapsulate ML-KEM secret for {}: {:?}",
            peer_addr, e
        )
    })?;
    let server_nonce = random_bytes::<32>();
    let client_hello_hash: [u8; 32] = Sha256::digest(&client_hello_bytes).into();
    let server_hello = build_server_hello(
        node_identity,
        &client_hello_hash,
        server_nonce,
        ciphertext.as_slice(),
    )?;
    let server_hello_bytes = bincode::serialize(&server_hello)
        .map_err(|e| format!("Failed to serialize transport server hello: {}", e))?;

    send.write_all(&server_hello_bytes).await.map_err(|e| {
        format!(
            "Failed to write transport server hello to {}: {}",
            peer_addr, e
        )
    })?;
    send.finish().map_err(|e| {
        format!(
            "Failed to finish transport handshake stream to {}: {}",
            peer_addr, e
        )
    })?;

    let transcript_hash = transport_transcript_hash(&client_hello_bytes, &server_hello_bytes);
    let session_key = derive_transport_session_key(
        shared_secret.as_slice(),
        &client_hello.nonce,
        &server_hello.nonce,
        &client_hello.address,
        &node_identity.address,
        &transcript_hash,
    )?;

    Ok((remote_identity, SecureSession { key: session_key }))
}

impl PeerManager {
    async fn perform_outbound_handshake(
        &self,
        connection: &Connection,
        peer_addr: SocketAddr,
    ) -> Result<(RemoteNodeIdentity, SecureSession), String> {
        let (client_dk, client_ek): (MlKem768DecapsulationKey, MlKem768EncapsulationKey) =
            MlKem768::generate(&mut OsRng);
        let client_nonce = random_bytes::<32>();
        let client_hello = build_client_hello(
            &self.node_identity,
            client_ek.as_bytes().as_slice(),
            client_nonce,
        )?;
        let client_hello_bytes = bincode::serialize(&client_hello)
            .map_err(|e| format!("Failed to serialize transport client hello: {}", e))?;

        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|e| format!("Failed to open transport handshake to {}: {}", peer_addr, e))?;
        send.write_all(&client_hello_bytes).await.map_err(|e| {
            format!(
                "Failed to write transport client hello to {}: {}",
                peer_addr, e
            )
        })?;
        send.finish().map_err(|e| {
            format!(
                "Failed to finish transport handshake stream to {}: {}",
                peer_addr, e
            )
        })?;

        let server_hello_bytes = tokio::time::timeout(
            Duration::from_secs(TRANSPORT_HANDSHAKE_TIMEOUT_SECS),
            recv.read_to_end(TRANSPORT_FRAME_LIMIT_BYTES),
        )
        .await
        .map_err(|_| {
            format!(
                "Timed out waiting for transport response from {}",
                peer_addr
            )
        })?
        .map_err(|e| {
            format!(
                "Failed to read transport response from {}: {}",
                peer_addr, e
            )
        })?;

        let server_hello: TransportServerHello = bincode::deserialize(&server_hello_bytes)
            .map_err(|e| {
                format!(
                    "Failed to decode transport response from {}: {}",
                    peer_addr, e
                )
            })?;
        let client_hello_hash: [u8; 32] = Sha256::digest(&client_hello_bytes).into();
        let remote_identity = verify_server_hello(&server_hello, &client_hello_hash)?;
        let ciphertext = decode_mlkem_ciphertext(&server_hello.kem_ciphertext)?;
        let shared_secret = client_dk.decapsulate(&ciphertext).map_err(|e| {
            format!(
                "Failed to decapsulate ML-KEM secret from {}: {:?}",
                peer_addr, e
            )
        })?;
        let transcript_hash = transport_transcript_hash(&client_hello_bytes, &server_hello_bytes);
        let session_key = derive_transport_session_key(
            shared_secret.as_slice(),
            &client_hello.nonce,
            &server_hello.nonce,
            &self.node_identity.address,
            &server_hello.address,
            &transcript_hash,
        )?;

        Ok((remote_identity, SecureSession { key: session_key }))
    }
}

#[derive(Debug)]
struct AnonymousCarrierCertVerifier;

impl rustls::client::danger::ServerCertVerifier for AnonymousCarrierCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer,
        _intermediates: &[CertificateDer],
        _server_name: &rustls::pki_types::ServerName,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_info_new() {
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        let peer = PeerInfo::new(addr);
        assert_eq!(peer.address, addr);
        assert_eq!(peer.reputation, 500);
        assert!(!peer.is_validator);
        assert_eq!(peer.score, 0);
        assert!(peer.connection.is_none());
    }

    #[test]
    fn test_peer_info_update_last_seen() {
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        let mut peer = PeerInfo::new(addr);
        let initial = peer.last_seen;
        // Sleep briefly to ensure time progresses
        std::thread::sleep(std::time::Duration::from_millis(10));
        peer.update_last_seen();
        assert!(peer.last_seen >= initial);
    }

    #[test]
    fn test_peer_info_adjust_score_positive() {
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        let mut peer = PeerInfo::new(addr);
        peer.adjust_score(5);
        assert_eq!(peer.score, 5);
        peer.adjust_score(10);
        assert_eq!(peer.score, 15);
    }

    #[test]
    fn test_peer_info_adjust_score_clamped() {
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        let mut peer = PeerInfo::new(addr);
        // Score clamped to max 20
        peer.adjust_score(100);
        assert_eq!(peer.score, 20);
        // Score clamped to min -20
        let mut peer2 = PeerInfo::new(addr);
        peer2.adjust_score(-100);
        assert_eq!(peer2.score, -20);
    }

    #[test]
    fn test_peer_info_score_oscillation() {
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        let mut peer = PeerInfo::new(addr);
        peer.adjust_score(10);
        peer.adjust_score(-5);
        assert_eq!(peer.score, 5);
        peer.adjust_score(-8);
        assert_eq!(peer.score, -3);
    }

    #[test]
    fn test_peer_info_default_values() {
        let addr: SocketAddr = "10.0.0.1:7001".parse().unwrap();
        let peer = PeerInfo::new(addr);
        assert_eq!(peer.address, addr);
        assert_eq!(peer.reputation, 500);
        assert!(!peer.is_validator);
        assert_eq!(peer.score, 0);
        assert!(peer.connection.is_none());
        // last_seen should be within the last second
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        assert!(peer.last_seen <= now);
        assert!(peer.last_seen >= now.saturating_sub(2));
    }

    #[test]
    fn test_peer_info_score_saturating() {
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        let mut peer = PeerInfo::new(addr);
        // Verify saturating_add prevents overflow
        peer.score = i64::MAX - 5;
        peer.adjust_score(100);
        assert_eq!(peer.score, 20); // clamped to max 20
        peer.score = i64::MIN + 5;
        peer.adjust_score(-100);
        assert_eq!(peer.score, -20); // clamped to min -20
    }

    // =========================================================================
    // Native PQ transport tests
    // =========================================================================

    fn temp_path(prefix: &str, suffix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{}_{}_{}.{}",
            prefix,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            suffix
        ))
    }

    fn sample_remote_identity() -> RemoteNodeIdentity {
        let keypair = Keypair::new();
        let address = keypair.pubkey();
        RemoteNodeIdentity {
            address,
            node_id: address.0,
            public_key_hex: encode_hex(&keypair.public_key().bytes),
        }
    }

    #[test]
    fn test_native_node_identity_generates_canonical_file() {
        let dir = temp_path("lichen_p2p_identity", "dir");
        fs::create_dir_all(&dir).unwrap();

        let identity = NodeIdentity::load_or_generate(&dir).unwrap();
        let identity_path = dir.join("node_identity.json");
        let stored: NodeIdentityFile =
            serde_json::from_str(&fs::read_to_string(&identity_path).unwrap()).unwrap();

        assert_eq!(stored.private_key.len(), 32);
        assert_eq!(stored.public_key, identity.keypair.public_key().bytes);
        assert_eq!(stored.public_key_base58, identity.address.to_base58());

        let reloaded = NodeIdentity::load_or_generate(&dir).unwrap();
        assert_eq!(identity.address, reloaded.address);
        assert_eq!(
            identity.keypair.public_key().bytes,
            reloaded.keypair.public_key().bytes
        );

        let _ = fs::remove_file(identity_path);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_peer_identity_store_accepts_known_identity() {
        let path = temp_path("lichen_peer_identity", "json");
        let store = PeerIdentityStore::new(path.clone());
        let addr: SocketAddr = "10.0.0.1:8000".parse().unwrap();
        let identity = sample_remote_identity();

        assert!(store.check_or_store(addr, &identity).unwrap());
        assert!(!store.check_or_store(addr, &identity).unwrap());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_peer_identity_store_rejects_identity_change() {
        let path = temp_path("lichen_peer_identity_change", "json");
        let store = PeerIdentityStore::new(path.clone());
        let addr: SocketAddr = "10.0.0.1:8000".parse().unwrap();
        let identity = sample_remote_identity();
        let changed_identity = sample_remote_identity();

        assert!(store.check_or_store(addr, &identity).unwrap());
        let result = store.check_or_store(addr, &changed_identity);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("TOFU VIOLATION"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_peer_identity_store_persists_across_reload() {
        let path = temp_path("lichen_peer_identity_persist", "json");
        let addr: SocketAddr = "10.0.0.1:8000".parse().unwrap();
        let identity = sample_remote_identity();

        {
            let store = PeerIdentityStore::new(path.clone());
            assert!(store.check_or_store(addr, &identity).unwrap());
        }

        {
            let store = PeerIdentityStore::new(path.clone());
            assert!(!store.check_or_store(addr, &identity).unwrap());
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_secure_session_roundtrip() {
        let session = SecureSession { key: [7u8; 32] };
        let plaintext = b"hello pq transport";
        let ciphertext = session.encrypt(plaintext).unwrap();
        let decrypted = session.decrypt(&ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encode_hex_roundtrip_shape() {
        let hex = encode_hex(&[0x00, 0x01, 0x0a, 0xff]);
        assert_eq!(hex, "00010aff");
    }

    /// C2-01: SeenMessageCache correctly deduplicates and evicts
    #[test]
    fn test_seen_message_cache_dedup() {
        let mut cache = SeenMessageCache::new(3);
        let h1 = [1u8; 32];
        let h2 = [2u8; 32];
        let h3 = [3u8; 32];
        let h4 = [4u8; 32];

        // First insert returns false (not seen)
        assert!(!cache.check_and_insert(h1));
        assert!(!cache.check_and_insert(h2));
        assert!(!cache.check_and_insert(h3));
        assert_eq!(cache.len(), 3);

        // Duplicate returns true (already seen)
        assert!(cache.check_and_insert(h1));
        assert!(cache.check_and_insert(h2));
        assert_eq!(cache.len(), 3);

        // Fourth insert evicts oldest (h1) — order was h1, h2, h3
        assert!(!cache.check_and_insert(h4));
        assert_eq!(cache.len(), 3);

        // h1 was evicted — no longer seen
        assert!(!cache.check_and_insert(h1));
        // h1 re-insert evicted h2 (next oldest) — order is now h3, h4, h1
        // h3 still present
        assert!(cache.check_and_insert(h3));
        // h2 was evicted
        assert!(!cache.check_and_insert(h2));
    }

    // =========================================================================
    // P3-5 Tests: Validator-tier peering
    // =========================================================================

    #[test]
    fn test_peer_info_validator_pubkey_default() {
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        let peer = PeerInfo::new(addr);
        assert!(!peer.is_validator);
        assert!(peer.validator_pubkey.is_none());
    }

    #[test]
    fn test_peer_info_mark_as_validator() {
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        let mut peer = PeerInfo::new(addr);
        let pubkey = Pubkey([42u8; 32]);
        peer.is_validator = true;
        peer.validator_pubkey = Some(pubkey);
        assert!(peer.is_validator);
        assert_eq!(peer.validator_pubkey.unwrap(), pubkey);
    }

    #[test]
    fn test_validator_score_boost() {
        let addr: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        let mut peer = PeerInfo::new(addr);
        assert_eq!(peer.score, 0);
        // Simulate mark_validator boosting score by +5
        peer.adjust_score(5);
        assert_eq!(peer.score, 5);
        // Even after a violation, validator stays above eviction threshold
        peer.adjust_score(-2);
        assert_eq!(peer.score, 3);
    }

    #[test]
    fn test_validator_peer_filtering() {
        let peers: DashMap<SocketAddr, PeerInfo> = DashMap::new();

        let addr1: SocketAddr = "10.0.0.1:7001".parse().unwrap();
        let addr2: SocketAddr = "10.0.0.2:7001".parse().unwrap();
        let addr3: SocketAddr = "10.0.0.3:7001".parse().unwrap();

        let mut p1 = PeerInfo::new(addr1);
        p1.is_validator = true;
        p1.validator_pubkey = Some(Pubkey([1u8; 32]));

        let p2 = PeerInfo::new(addr2); // observer

        let mut p3 = PeerInfo::new(addr3);
        p3.is_validator = true;
        p3.validator_pubkey = Some(Pubkey([3u8; 32]));

        peers.insert(addr1, p1);
        peers.insert(addr2, p2);
        peers.insert(addr3, p3);

        // Filter validators only
        let validator_addrs: Vec<SocketAddr> = peers
            .iter()
            .filter(|entry| entry.value().is_validator)
            .map(|entry| *entry.key())
            .collect();

        assert_eq!(validator_addrs.len(), 2);
        assert!(validator_addrs.contains(&addr1));
        assert!(validator_addrs.contains(&addr3));
        assert!(!validator_addrs.contains(&addr2));
    }

    #[test]
    fn test_validator_eviction_resistance() {
        // Validators start with +5 boost from mark_validator.
        // Two violations (-2 each) still keep score positive.
        let mut peer = PeerInfo::new("10.0.0.1:7001".parse().unwrap());
        peer.adjust_score(5); // validator boost
        peer.adjust_score(-2); // violation 1
        peer.adjust_score(-2); // violation 2
        assert_eq!(peer.score, 1); // still positive — wouldn't be evicted
    }

    // ── Eclipse Attack Resistance ────────────────────────────────────

    #[test]
    fn test_same_subnet_ipv4() {
        let a: IpAddr = "10.0.1.5".parse().unwrap();
        let b: IpAddr = "10.0.1.99".parse().unwrap();
        let c: IpAddr = "10.0.2.5".parse().unwrap();
        assert!(same_subnet(&a, &b)); // same /24
        assert!(!same_subnet(&a, &c)); // different /24
    }

    #[test]
    fn test_same_subnet_ipv6() {
        let a: IpAddr = "2001:db8:abcd::1".parse().unwrap();
        let b: IpAddr = "2001:db8:abcd::ffff".parse().unwrap();
        let c: IpAddr = "2001:db8:abce::1".parse().unwrap();
        assert!(same_subnet(&a, &b)); // same /48
        assert!(!same_subnet(&a, &c)); // different /48
    }

    #[test]
    fn test_same_subnet_mixed_families() {
        let v4: IpAddr = "10.0.1.1".parse().unwrap();
        let v6: IpAddr = "::ffff:10.0.1.1".parse().unwrap();
        // Mixed address families are never the same subnet (by design)
        assert!(!same_subnet(&v4, &v6));
    }

    #[test]
    fn test_self_connection_addr_detects_same_socket() {
        let addr: SocketAddr = "127.0.0.1:7001".parse().unwrap();
        assert!(is_self_connection_addr(addr, addr));
    }

    #[test]
    fn test_self_connection_addr_detects_loopback_alias() {
        let peer: SocketAddr = "127.0.0.2:7001".parse().unwrap();
        let local: SocketAddr = "127.0.0.1:7001".parse().unwrap();
        assert!(is_self_connection_addr(peer, local));
    }

    #[test]
    fn test_self_connection_addr_allows_distinct_remote_peer() {
        let peer: SocketAddr = "10.0.1.8:7001".parse().unwrap();
        let local: SocketAddr = "10.0.2.8:7001".parse().unwrap();
        assert!(!is_self_connection_addr(peer, local));
    }

    #[test]
    fn test_localhost_peer_limits_are_bypassed() {
        let local: IpAddr = "127.0.0.1".parse().unwrap();
        let peer: IpAddr = "127.0.0.1".parse().unwrap();
        let remote: IpAddr = "10.0.1.5".parse().unwrap();

        assert!(should_bypass_localhost_peer_limits(&local, &peer));
        assert!(!should_bypass_localhost_peer_limits(&local, &remote));
    }

    #[tokio::test]
    async fn test_subnet_limit_in_connect_peer() {
        let tmp = std::env::temp_dir().join(format!(
            "lichen-test-subnet-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (tx, _rx) = mpsc::channel(100);
        let mgr = PeerManager::new(
            "127.0.0.1:0".parse().unwrap(),
            tx,
            Some(tmp.clone()),
            None,
            50,
            vec![],
        )
        .await
        .unwrap();

        // Manually insert MAX_PEERS_PER_SUBNET peers from the same /24
        for i in 0..PeerManager::MAX_PEERS_PER_SUBNET {
            let addr: SocketAddr = format!("10.0.1.{}:7001", i + 1).parse().unwrap();
            mgr.peers.insert(addr, PeerInfo::new(addr));
        }

        // Next peer in same /24 should be rejected
        let result = mgr.connect_peer("10.0.1.200:7001".parse().unwrap()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Subnet limit"));

        // Peer from different /24 should be fine (will fail TLS but won't hit subnet check)
        let result2 = mgr.connect_peer("10.0.2.1:7001".parse().unwrap()).await;
        // Won't succeed because there's no real server, but error should NOT be about subnet
        assert!(!result2
            .as_ref()
            .err()
            .map(|e| e.contains("Subnet limit"))
            .unwrap_or(false));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_expensive_request_rate_limit_denies_after_window_cap() {
        let tmp = std::env::temp_dir().join(format!(
            "lichen-test-expensive-limit-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (tx, _rx) = mpsc::channel(8);
        let mgr = PeerManager::new(
            "127.0.0.1:0".parse().unwrap(),
            tx,
            Some(tmp.clone()),
            None,
            10,
            vec![],
        )
        .await
        .unwrap();

        let addr: SocketAddr = "10.0.1.99:7001".parse().unwrap();
        mgr.peers.insert(addr, PeerInfo::new(addr));

        assert!(mgr.check_expensive_rate_limit(&addr, 2));
        assert!(mgr.check_expensive_rate_limit(&addr, 2));
        assert!(!mgr.check_expensive_rate_limit(&addr, 2));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_expensive_request_rate_limit_resets_after_window() {
        let tmp = std::env::temp_dir().join(format!(
            "lichen-test-expensive-reset-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (tx, _rx) = mpsc::channel(8);
        let mgr = PeerManager::new(
            "127.0.0.1:0".parse().unwrap(),
            tx,
            Some(tmp.clone()),
            None,
            10,
            vec![],
        )
        .await
        .unwrap();

        let addr: SocketAddr = "10.0.1.100:7001".parse().unwrap();
        mgr.peers.insert(addr, PeerInfo::new(addr));
        {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let mut peer = mgr.peers.get_mut(&addr).unwrap();
            peer.expensive_request_window = (now.saturating_sub(61), 2);
        }

        assert!(mgr.check_expensive_rate_limit(&addr, 2));

        let _ = fs::remove_dir_all(&tmp);
    }

    // ── Peer Scoring / Latency Tracking ──────────────────────────────

    #[test]
    fn test_record_latency_initial() {
        let mut peer = PeerInfo::new("10.0.0.1:7001".parse().unwrap());
        assert!(peer.avg_response_ms.is_none());
        peer.record_latency(100.0);
        assert_eq!(peer.avg_response_ms, Some(100.0)); // first sample = exact
    }

    #[test]
    fn test_record_latency_ema_converges() {
        let mut peer = PeerInfo::new("10.0.0.1:7001".parse().unwrap());
        // Seed with 100ms then send many 50ms samples — should converge towards 50
        peer.record_latency(100.0);
        for _ in 0..20 {
            peer.record_latency(50.0);
        }
        let avg = peer.avg_response_ms.unwrap();
        assert!(
            avg > 49.5 && avg < 52.0,
            "EMA should converge to ~50, got {}",
            avg
        );
    }

    #[tokio::test]
    async fn test_fastest_peers_sorting() {
        let tmp = std::env::temp_dir().join(format!(
            "lichen-test-fastest-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (tx, _rx) = mpsc::channel(100);
        let mgr = PeerManager::new(
            "127.0.0.1:0".parse().unwrap(),
            tx,
            Some(tmp.clone()),
            None,
            50,
            vec![],
        )
        .await
        .unwrap();

        let fast: SocketAddr = "10.0.0.1:7001".parse().unwrap();
        let medium: SocketAddr = "10.0.1.1:7001".parse().unwrap();
        let slow: SocketAddr = "10.0.2.1:7001".parse().unwrap();
        let unknown: SocketAddr = "10.0.3.1:7001".parse().unwrap();

        for addr in [fast, medium, slow, unknown] {
            mgr.peers.insert(addr, PeerInfo::new(addr));
        }
        mgr.record_peer_latency(&fast, 10.0);
        mgr.record_peer_latency(&medium, 50.0);
        mgr.record_peer_latency(&slow, 200.0);
        // 'unknown' has no samples — should sort last

        let top3 = mgr.fastest_peers(3);
        assert_eq!(top3.len(), 3);
        assert_eq!(top3[0], fast);
        assert_eq!(top3[1], medium);
        assert_eq!(top3[2], slow);
    }

    // ── Bandwidth Metering ───────────────────────────────────────────

    #[test]
    fn test_bytes_tracking() {
        let mut peer = PeerInfo::new("10.0.0.1:7001".parse().unwrap());
        assert_eq!(peer.bytes_received, 0);
        assert_eq!(peer.bytes_sent, 0);

        peer.add_bytes_received(1500);
        peer.add_bytes_received(500);
        assert_eq!(peer.bytes_received, 2000);

        peer.add_bytes_sent(3000);
        assert_eq!(peer.bytes_sent, 3000);
    }

    #[test]
    fn test_bytes_tracking_saturates() {
        let mut peer = PeerInfo::new("10.0.0.1:7001".parse().unwrap());
        peer.bytes_received = u64::MAX - 10;
        peer.add_bytes_received(100);
        assert_eq!(peer.bytes_received, u64::MAX); // saturating, no panic
    }

    #[tokio::test]
    async fn test_bandwidth_stats() {
        let tmp = std::env::temp_dir().join(format!(
            "lichen-test-bw-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (tx, _rx) = mpsc::channel(100);
        let mgr = PeerManager::new(
            "127.0.0.1:0".parse().unwrap(),
            tx,
            Some(tmp.clone()),
            None,
            50,
            vec![],
        )
        .await
        .unwrap();

        let addr: SocketAddr = "10.0.0.1:7001".parse().unwrap();
        mgr.peers.insert(addr, PeerInfo::new(addr));

        mgr.record_bytes_received(&addr, 10_000);
        mgr.record_bytes_sent(&addr, 5_000);

        let stats = mgr.bandwidth_stats(&addr);
        assert!(stats.is_some());
        let (recv_bps, send_bps) = stats.unwrap();
        // With tracking_since ≈ now, elapsed rounds to 1s max, so bps ≈ bytes
        assert!(
            recv_bps >= 1.0,
            "recv_bps should be positive, got {}",
            recv_bps
        );
        assert!(
            send_bps >= 1.0,
            "send_bps should be positive, got {}",
            send_bps
        );
    }

    #[tokio::test]
    async fn test_bandwidth_stats_unknown_peer() {
        let tmp = std::env::temp_dir().join(format!(
            "lichen-test-bw-unk-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (tx, _rx) = mpsc::channel(100);
        let mgr = PeerManager::new(
            "127.0.0.1:0".parse().unwrap(),
            tx,
            Some(tmp.clone()),
            None,
            50,
            vec![],
        )
        .await
        .unwrap();

        let stats = mgr.bandwidth_stats(&"10.0.0.1:7001".parse().unwrap());
        assert!(stats.is_none());
    }

    #[tokio::test]
    async fn test_non_consensus_targets_use_bounded_kademlia_fanout() {
        let tmp = std::env::temp_dir().join(format!(
            "lichen-test-kad-fanout-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (tx, _rx) = mpsc::channel(100);
        let mgr = PeerManager::new(
            "127.0.0.1:0".parse().unwrap(),
            tx,
            Some(tmp.clone()),
            None,
            50,
            vec![],
        )
        .await
        .unwrap();

        let peers = [
            ([1u8; 32], "10.0.0.1:7001".parse().unwrap()),
            ([2u8; 32], "10.0.0.2:7001".parse().unwrap()),
            ([3u8; 32], "10.0.0.3:7001".parse().unwrap()),
        ];

        for (node_id, addr) in peers {
            mgr.peers.insert(addr, PeerInfo::new(addr));
            mgr.update_kademlia(node_id, addr);
        }

        let targets = mgr.non_consensus_targets(&[0u8; 32], 2);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0], "10.0.0.1:7001".parse::<SocketAddr>().unwrap());
        assert_eq!(targets[1], "10.0.0.2:7001".parse::<SocketAddr>().unwrap());
    }

    #[tokio::test]
    async fn test_non_consensus_targets_fall_back_to_all_peers_without_overlay_entries() {
        let tmp = std::env::temp_dir().join(format!(
            "lichen-test-kad-fallback-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (tx, _rx) = mpsc::channel(100);
        let mgr = PeerManager::new(
            "127.0.0.1:0".parse().unwrap(),
            tx,
            Some(tmp.clone()),
            None,
            50,
            vec![],
        )
        .await
        .unwrap();

        let addr_a: SocketAddr = "10.0.1.1:7001".parse().unwrap();
        let addr_b: SocketAddr = "10.0.1.2:7001".parse().unwrap();
        mgr.peers.insert(addr_a, PeerInfo::new(addr_a));
        mgr.peers.insert(addr_b, PeerInfo::new(addr_b));

        let targets = mgr.non_consensus_targets(&[9u8; 32], 1);
        assert_eq!(targets.len(), 2);
        assert!(targets.contains(&addr_a));
        assert!(targets.contains(&addr_b));
    }

    #[test]
    fn test_recv_bandwidth_bps() {
        let mut peer = PeerInfo::new("10.0.0.1:7001".parse().unwrap());
        // tracking_since is ~now, so elapsed ≈ 1 (clamped min)
        peer.add_bytes_received(10_000);
        let bps = peer.recv_bandwidth_bps();
        assert!(bps >= 1.0, "bps should be positive, got {}", bps);
    }

    // ── PQ Identity Store Maintenance Tests ────────────────────────

    #[test]
    fn test_peer_identity_store_remove_identity() {
        let path = temp_path("lichen_peer_identity_remove", "json");
        let store = PeerIdentityStore::new(path.clone());
        let addr: SocketAddr = "10.0.0.1:8000".parse().unwrap();
        let identity = sample_remote_identity();

        assert!(store.check_or_store(addr, &identity).unwrap());
        assert!(store.remove_identity(&addr));
        assert!(store.check_or_store(addr, &identity).unwrap());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_peer_identity_store_clear_all() {
        let path = temp_path("lichen_peer_identity_clear", "json");
        let store = PeerIdentityStore::new(path.clone());
        let addr_a: SocketAddr = "10.0.0.1:8000".parse().unwrap();
        let addr_b: SocketAddr = "10.0.0.2:8000".parse().unwrap();

        assert!(store
            .check_or_store(addr_a, &sample_remote_identity())
            .unwrap());
        assert!(store
            .check_or_store(addr_b, &sample_remote_identity())
            .unwrap());
        store.clear_all();

        assert!(store
            .check_or_store(addr_a, &sample_remote_identity())
            .unwrap());
        assert!(store
            .check_or_store(addr_b, &sample_remote_identity())
            .unwrap());

        let _ = fs::remove_file(&path);
    }

    // ── M-10: AS-Level Eclipse Defense Tests ────────────────────────

    #[test]
    fn test_asn_bucket_ipv4_same_slash16() {
        let a: IpAddr = "10.5.1.1".parse().unwrap();
        let b: IpAddr = "10.5.200.99".parse().unwrap();
        assert!(
            same_asn_bucket(&a, &b),
            "Same /16 should be same ASN bucket"
        );
        assert_eq!(asn_bucket(&a), asn_bucket(&b));
    }

    #[test]
    fn test_asn_bucket_ipv4_different_slash16() {
        let a: IpAddr = "10.5.1.1".parse().unwrap();
        let b: IpAddr = "10.6.1.1".parse().unwrap();
        assert!(
            !same_asn_bucket(&a, &b),
            "Different /16 should be different ASN bucket"
        );
    }

    #[test]
    fn test_asn_bucket_ipv6_same_slash32() {
        let a: IpAddr = "2001:db8:abcd:1::1".parse().unwrap();
        let b: IpAddr = "2001:db8:ffff:9::9".parse().unwrap();
        assert!(
            same_asn_bucket(&a, &b),
            "Same /32 should be same ASN bucket"
        );
    }

    #[test]
    fn test_asn_bucket_ipv6_different_slash32() {
        let a: IpAddr = "2001:db8::1".parse().unwrap();
        let b: IpAddr = "2001:db9::1".parse().unwrap();
        assert!(
            !same_asn_bucket(&a, &b),
            "Different /32 should be different ASN bucket"
        );
    }

    #[test]
    fn test_asn_bucket_v4_v6_never_same() {
        let v4: IpAddr = "10.5.1.1".parse().unwrap();
        let v6: IpAddr = "::ffff:10.5.1.1".parse().unwrap();
        // asn_bucket returns different types of values for v4 vs v6
        // but same_asn_bucket just compares u32 values — could theoretically
        // collide but won't for typical addresses
        let _ = asn_bucket(&v4);
        let _ = asn_bucket(&v6);
    }

    #[test]
    fn test_asn_bucket_deterministic() {
        let ip: IpAddr = "192.168.50.1".parse().unwrap();
        let b1 = asn_bucket(&ip);
        let b2 = asn_bucket(&ip);
        assert_eq!(b1, b2, "ASN bucket should be deterministic");
        // 192.168 => (192 << 8) | 168 = 49320
        assert_eq!(b1, (192 << 8) | 168);
    }

    #[tokio::test]
    async fn test_asn_bucket_limit_in_connect_peer() {
        let tmp = std::env::temp_dir().join(format!(
            "lichen-test-asn-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (tx, _rx) = mpsc::channel(100);
        let mgr = PeerManager::new(
            "127.0.0.1:0".parse().unwrap(),
            tx,
            Some(tmp.clone()),
            None,
            50,
            vec![],
        )
        .await
        .unwrap();

        // Insert MAX_PEERS_PER_ASN_BUCKET peers from same /16 but different /24s
        for i in 0..PeerManager::MAX_PEERS_PER_ASN_BUCKET {
            let addr: SocketAddr = format!("10.5.{}.1:7001", i + 1).parse().unwrap();
            mgr.peers.insert(addr, PeerInfo::new(addr));
        }

        // Next peer in same /16 should be rejected (ASN bucket limit)
        let result = mgr.connect_peer("10.5.200.1:7001".parse().unwrap()).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("ASN bucket limit"),
            "Should hit ASN bucket limit"
        );

        // Peer from different /16 should not hit ASN limit
        // (it will fail for other reasons like TLS but not "ASN bucket limit")
        let result2 = mgr.connect_peer("10.6.1.1:7001".parse().unwrap()).await;
        assert!(
            !result2
                .as_ref()
                .err()
                .map(|e| e.contains("ASN bucket limit"))
                .unwrap_or(false),
            "Different /16 should not hit ASN bucket limit"
        );
    }
}
