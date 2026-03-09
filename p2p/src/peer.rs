// Peer Connection Management

use crate::message::P2PMessage;
use crate::peer_ban::PeerBanList;
use crate::peer_store::PeerStore;
use dashmap::DashMap;
use moltchain_core::Pubkey;
use quinn::{Connection, Endpoint, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

fn runtime_moltchain_dir(runtime_home: Option<&Path>) -> PathBuf {
    runtime_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join(".moltchain")
}

/// Peer information
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub address: SocketAddr,
    pub connection: Option<Connection>,
    pub last_seen: u64,
    pub reputation: u64,
    pub is_validator: bool,
    pub score: i64,
    /// P3-2: Kademlia node ID (SHA-256 of public key). [0; 32] if unknown.
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

    /// AUDIT-FIX C1-01: Persistent node certificate chain for mutual TLS
    node_cert_chain: Vec<CertificateDer<'static>>,

    /// AUDIT-FIX C1-01: Raw node private key bytes for client cert auth
    node_key_bytes: Vec<u8>,

    /// Local node certificate fingerprint for self-connection detection
    local_fingerprint: [u8; 32],

    /// AUDIT-FIX C1-01: TOFU fingerprint store for certificate pinning
    fingerprint_store: Arc<PeerFingerprintStore>,

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

        // AUDIT-FIX C1-01: Load or generate persistent node identity
        // Replaces ephemeral per-startup certificate with persistent cert+key
        // stored at ~/.moltchain/node_cert.der + ~/.moltchain/node_key.der
        let runtime_dir = runtime_moltchain_dir(runtime_home.as_deref());
        let identity = NodeIdentity::load_or_generate(&runtime_dir)?;

        // Clone cert chain + key bytes for client connections (mutual TLS)
        let node_cert_chain = vec![identity.cert_der.clone()];
        let node_key_bytes = identity.key_bytes.clone();
        let local_fingerprint = NodeIdentity::compute_fingerprint(node_cert_chain[0].as_ref());

        // AUDIT-FIX C1-01: Server config with mutual TLS
        // Replaces .with_no_client_auth() — server now validates connecting peers'
        // certificates using MoltClientCertVerifier (self-signature verification).
        // client_auth_mandatory=false for backwards compatibility with un-upgraded nodes.
        let server_key = PrivateKeyDer::try_from(identity.key_bytes)
            .map_err(|e| format!("Failed to parse node key: {}", e))?;
        let mut server_crypto = rustls::ServerConfig::builder()
            .with_client_cert_verifier(Arc::new(MoltClientCertVerifier))
            .with_single_cert(vec![identity.cert_der], server_key)
            .map_err(|e| format!("Failed to create rustls config: {}", e))?;

        server_crypto.alpn_protocols = vec![b"molt".to_vec()];

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

        // Create QUIC endpoint
        let endpoint = Endpoint::server(server_config, local_addr)
            .map_err(|e| format!("Failed to create endpoint: {}", e))?;

        info!("🦞 P2P: QUIC endpoint listening on {}", local_addr);

        let ban_list_path = runtime_dir.join("peer-banlist.json");

        // AUDIT-FIX C1-01: TOFU fingerprint store for certificate pinning
        let fp_path = runtime_dir.join("peer_fingerprints.json");
        let fingerprint_store = Arc::new(PeerFingerprintStore::new(fp_path));

        Ok(PeerManager {
            peers: Arc::new(DashMap::new()),
            endpoint,
            local_addr,
            message_tx,
            peer_store,
            ban_list: Arc::new(Mutex::new(PeerBanList::new(ban_list_path))),
            node_cert_chain,
            node_key_bytes,
            local_fingerprint,
            fingerprint_store,
            // C2-01: 20K capacity ≈ 640KB — covers ~5 minutes of peak traffic
            seen_messages: Arc::new(Mutex::new(SeenMessageCache::new(20_000))),
            max_peers,
            reserved_peers,
            kademlia: Arc::new(Mutex::new(crate::kademlia::KademliaTable::new(
                local_fingerprint,
            ))),
        })
    }

    /// Maximum number of concurrent peer connections (configurable per role)
    pub const MAX_PEERS: usize = 50;

    /// Eclipse-attack resistance: max peers from the same /24 (IPv4) or /48 (IPv6) subnet.
    pub const MAX_PEERS_PER_SUBNET: usize = 2;

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
        if self.count_peers_in_subnet(&peer_addr.ip()) >= Self::MAX_PEERS_PER_SUBNET {
            return Err(format!(
                "Subnet limit reached ({}) for {}",
                Self::MAX_PEERS_PER_SUBNET,
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

        // AUDIT-FIX C1-01: Proper TLS certificate verification + mutual TLS
        // Replaces SkipServerVerification with MoltCertVerifier (validates self-signatures).
        // Client now presents its own certificate for mutual authentication.
        let client_key = PrivateKeyDer::try_from(self.node_key_bytes.clone())
            .map_err(|e| format!("Failed to parse node key: {}", e))?;
        let mut rustls_config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(MoltCertVerifier))
            .with_client_auth_cert(self.node_cert_chain.clone(), client_key)
            .map_err(|e| format!("Failed to create TLS client config: {}", e))?;

        // Configure ALPN
        rustls_config.alpn_protocols = vec![b"molt".to_vec()];

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

        // AUDIT-FIX C1-01: TOFU fingerprint check after connection
        // Extract peer certificate and verify fingerprint against known peers.
        // Rejects connections if a known peer's certificate fingerprint changes
        // (potential MITM attack or unauthorized identity change).
        if let Some(identity) = connection.peer_identity() {
            if let Some(certs) = identity.downcast_ref::<Vec<CertificateDer<'static>>>() {
                if let Some(cert) = certs.first() {
                    let fp = NodeIdentity::compute_fingerprint(cert.as_ref());
                    if fp == self.local_fingerprint {
                        warn!(
                            "P2P: Rejecting self-connection attempt to {} (same node identity)",
                            peer_addr
                        );
                        connection.close(quinn::VarInt::from_u32(1), b"self_connection");
                        return Err("Refusing self-connection (same node identity)".to_string());
                    }
                    match self.fingerprint_store.check_or_store(&peer_addr, &fp) {
                        Ok(true) => info!(
                            "P2P TOFU: New peer {} registered (fingerprint: {})",
                            peer_addr,
                            NodeIdentity::fingerprint_hex(&fp)
                        ),
                        Ok(false) => info!("P2P TOFU: Peer {} identity verified", peer_addr),
                        Err(e) => {
                            warn!("{}", e);
                            connection.close(quinn::VarInt::from_u32(1), b"fingerprint_mismatch");
                            return Err(e);
                        }
                    }
                    // P3-2: Register peer in Kademlia routing table using certificate fingerprint
                    self.update_kademlia(fp, peer_addr);
                }
            }
        }

        // Store peer info
        let mut peer_info = PeerInfo::new(peer_addr);
        peer_info.connection = Some(connection.clone());
        self.peers.insert(peer_addr, peer_info);
        if let Some(store) = &self.peer_store {
            store.record_peer(peer_addr);
        }

        info!("✅ P2P: Connected to peer {}", peer_addr);

        // Spawn task to handle incoming messages
        let peers = self.peers.clone();
        let message_tx = self.message_tx.clone();
        let seen_messages = self.seen_messages.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(
                connection,
                peer_addr,
                peers.clone(),
                message_tx,
                seen_messages,
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
        let connection = {
            let peer = self.peers.get(peer_addr).ok_or("Peer not found")?;
            peer.connection.clone()
        }; // guard dropped here

        if let Some(connection) = connection {
            let bytes = message.serialize()?;

            let mut send_stream = connection
                .open_uni()
                .await
                .map_err(|e| format!("Failed to open stream: {}", e))?;

            send_stream
                .write_all(&bytes)
                .await
                .map_err(|e| format!("Failed to send: {}", e))?;

            send_stream
                .finish()
                .map_err(|e| format!("Failed to finish stream: {}", e))?;

            // Bandwidth metering: track outbound bytes
            if let Some(mut peer) = self.peers.get_mut(peer_addr) {
                peer.add_bytes_sent(bytes.len() as u64);
            }

            Ok(())
        } else {
            Err("No active connection".to_string())
        }
    }

    /// Broadcast message to all peers except the specified sender (for relay/re-broadcasting).
    /// Uses concurrent sends like broadcast() but skips the sender to avoid echo.
    pub async fn broadcast_except(&self, message: &P2PMessage, except: &SocketAddr) {
        let peers: Vec<SocketAddr> = self
            .peers
            .iter()
            .map(|entry| *entry.key())
            .filter(|addr| addr != except)
            .collect();
        if peers.is_empty() {
            return;
        }

        let bytes = match message.serialize() {
            Ok(b) => std::sync::Arc::new(b),
            Err(e) => {
                warn!("P2P: broadcast_except serialize error: {}", e);
                return;
            }
        };

        let mut conn_tasks: Vec<(SocketAddr, Option<quinn::Connection>)> =
            Vec::with_capacity(peers.len());
        for addr in &peers {
            let conn = self.peers.get(addr).and_then(|p| p.connection.clone());
            conn_tasks.push((*addr, conn));
        }

        let mut handles = Vec::with_capacity(conn_tasks.len());
        for (peer_addr, connection) in conn_tasks {
            let bytes = bytes.clone();
            handles.push(tokio::spawn(async move {
                if let Some(conn) = connection {
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
    pub async fn broadcast(&self, message: P2PMessage) {
        let peers: Vec<SocketAddr> = self.peers.iter().map(|entry| *entry.key()).collect();
        if peers.is_empty() {
            return;
        }

        // Pre-serialize once (avoid N redundant serializations)
        let bytes = match message.serialize() {
            Ok(b) => std::sync::Arc::new(b),
            Err(e) => {
                warn!("P2P: broadcast serialize error: {}", e);
                return;
            }
        };

        // Extract connection handles upfront (drop DashMap guards before async)
        let mut conn_tasks: Vec<(SocketAddr, Option<quinn::Connection>)> =
            Vec::with_capacity(peers.len());
        for addr in &peers {
            let conn = self.peers.get(addr).and_then(|p| p.connection.clone());
            conn_tasks.push((*addr, conn));
        }

        // Spawn concurrent send tasks
        let mut handles = Vec::with_capacity(conn_tasks.len());
        for (peer_addr, connection) in conn_tasks {
            let bytes = bytes.clone();
            handles.push(tokio::spawn(async move {
                if let Some(conn) = connection {
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

    /// Get all peer addresses
    pub fn get_peers(&self) -> Vec<SocketAddr> {
        self.peers.iter().map(|entry| *entry.key()).collect()
    }

    /// P3-2: Route a message to the `count` closest peers by XOR distance
    /// to `target_id`. Falls back to all peers if the routing table is empty.
    pub async fn route_to_closest(&self, target_id: &[u8; 32], count: usize, message: P2PMessage) {
        let closest = {
            let table = self.kademlia.lock().unwrap();
            table.closest(target_id, count)
        };

        if closest.is_empty() {
            // Routing table empty — fall back to broadcast
            self.broadcast(message).await;
            return;
        }

        let bytes = match message.serialize() {
            Ok(b) => std::sync::Arc::new(b),
            Err(e) => {
                warn!("P2P: route serialize error: {}", e);
                return;
            }
        };

        let mut handles = Vec::with_capacity(closest.len());
        for entry in closest {
            let conn = self
                .peers
                .get(&entry.address)
                .and_then(|p| p.connection.clone());
            let bytes = bytes.clone();
            let addr = entry.address;
            handles.push(tokio::spawn(async move {
                if let Some(conn) = conn {
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
        let mut table = self.kademlia.lock().unwrap();
        table.insert(node_id, address);
    }

    /// P3-2: Get the number of entries in the Kademlia routing table.
    pub fn kademlia_size(&self) -> usize {
        let table = self.kademlia.lock().unwrap();
        table.len()
    }

    /// P3-2: Return the closest nodes to `target_id` as (node_id, address) pairs.
    pub fn kademlia_closest(&self, target_id: &[u8; 32], count: usize) -> Vec<([u8; 32], String)> {
        let table = self.kademlia.lock().unwrap();
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

        let bytes = match message.serialize() {
            Ok(b) => std::sync::Arc::new(b),
            Err(e) => {
                warn!("P2P: validator broadcast serialize error: {}", e);
                return;
            }
        };

        let mut handles = Vec::with_capacity(validator_addrs.len());
        for addr in validator_addrs {
            let conn = self.peers.get(&addr).and_then(|p| p.connection.clone());
            let bytes = bytes.clone();
            handles.push(tokio::spawn(async move {
                if let Some(conn) = conn {
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
    /// AUDIT-FIX M3: Gossip needs actual peer scores instead of hardcoded 500.
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
        let fingerprint_store = self.fingerprint_store.clone();
        let seen_messages = self.seen_messages.clone();
        let kademlia = self.kademlia.clone();
        let max_peers = self.max_peers;
        let local_addr = self.local_addr;
        let local_fingerprint = self.local_fingerprint;

        tokio::spawn(async move {
            while let Some(connecting) = endpoint.accept().await {
                let peers = peers.clone();
                let message_tx = message_tx.clone();
                let peer_store = peer_store.clone();
                let ban_list = ban_list.clone();
                let fingerprint_store = fingerprint_store.clone();
                let seen_messages = seen_messages.clone();
                let kademlia = kademlia.clone();

                tokio::spawn(async move {
                    match connecting.await {
                        Ok(connection) => {
                            let peer_addr = connection.remote_address();
                            let same_port = peer_addr.port() == local_addr.port();
                            let same_ip = peer_addr.ip() == local_addr.ip();
                            let loopback_pair =
                                peer_addr.ip().is_loopback() && local_addr.ip().is_loopback();
                            let unspecified_pair =
                                peer_addr.ip().is_unspecified() && local_addr.ip().is_unspecified();
                            if peer_addr == local_addr
                                || (same_port && (same_ip || loopback_pair || unspecified_pair))
                            {
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
                                if subnet_count >= PeerManager::MAX_PEERS_PER_SUBNET {
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

                            // AUDIT-FIX C1-01: TOFU fingerprint check for inbound connections
                            if let Some(identity) = connection.peer_identity() {
                                if let Some(certs) =
                                    identity.downcast_ref::<Vec<CertificateDer<'static>>>()
                                {
                                    if let Some(cert) = certs.first() {
                                        let fp = NodeIdentity::compute_fingerprint(cert.as_ref());
                                        if fp == local_fingerprint {
                                            warn!("P2P: Rejected inbound self-identity connection from {}", peer_addr);
                                            connection.close(
                                                quinn::VarInt::from_u32(1),
                                                b"self_connection",
                                            );
                                            return;
                                        }
                                        match fingerprint_store.check_or_store(&peer_addr, &fp) {
                                            Ok(true) => info!("P2P TOFU: New inbound peer {} registered (fingerprint: {})",
                                                peer_addr, NodeIdentity::fingerprint_hex(&fp)),
                                            Ok(false) => {},
                                            Err(e) => {
                                                warn!("{}", e);
                                                connection.close(quinn::VarInt::from_u32(1), b"fingerprint_mismatch");
                                                return;
                                            }
                                        }
                                        // P3-2: Register inbound peer in Kademlia routing table
                                        if fp != [0u8; 32] {
                                            let mut table = kademlia.lock().unwrap();
                                            table.insert(fp, peer_addr);
                                        }
                                    }
                                }
                            }

                            // Store peer
                            let mut peer_info = PeerInfo::new(peer_addr);
                            peer_info.connection = Some(connection.clone());
                            peers.insert(peer_addr, peer_info);
                            if let Some(store) = &peer_store {
                                store.record_peer(peer_addr);
                            }

                            // Handle connection
                            if let Err(e) = handle_connection(
                                connection,
                                peer_addr,
                                peers.clone(),
                                message_tx,
                                seen_messages,
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
) -> Result<(), String> {
    let mut deser_failures: u32 = 0;
    let mut deser_total: u32 = 0;
    const MAX_DESER_FAILURES: u32 = 10;
    // AUDIT-FIX H9: Track failure RATIO instead of consecutive-only.
    // Disconnect if >50% of messages in a window are failures.
    const DESER_WINDOW: u32 = 20;

    loop {
        let mut stream = connection
            .accept_uni()
            .await
            .map_err(|e| format!("Failed to accept stream: {}", e))?;

        let bytes = stream
            .read_to_end(16 * 1024 * 1024) // AUDIT-FIX H3: Align with P2PMessage serialize limit (16MB).
            // Previous 2MB limit silently rejected valid state snapshot chunks.
            .await
            .map_err(|e| format!("Failed to read: {}", e))?;

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

                // C2-01: Dedup — hash the raw message bytes and skip if already seen.
                // Only dedup gossip message types (Block, Vote, Transaction,
                // SlashingEvidence, ValidatorAnnounce). Request/response types
                // (Ping, Pong, BlockRequest, StatusRequest, etc.) are point-to-point
                // and must always be processed.
                let should_dedup = matches!(
                    message.msg_type,
                    crate::MessageType::Block(_)
                        | crate::MessageType::Vote(_)
                        | crate::MessageType::Transaction(_)
                        | crate::MessageType::SlashingEvidence(_)
                        | crate::MessageType::ValidatorAnnounce { .. }
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
// AUDIT-FIX C1-01: Proper TLS certificate validation infrastructure
// Replaces SkipServerVerification with cryptographic self-signature verification,
// persistent node identity, TOFU fingerprint pinning, and mutual TLS.
// ============================================================================

/// Persistent node identity — generates or loads a certificate + private key
/// from ~/.moltchain/node_cert.der and ~/.moltchain/node_key.der.
/// Provides stable cryptographic identity across node restarts.
struct NodeIdentity {
    cert_der: CertificateDer<'static>,
    key_bytes: Vec<u8>,
    #[allow(dead_code)]
    fingerprint: [u8; 32],
}

impl NodeIdentity {
    fn load_or_generate(moltchain_dir: &Path) -> Result<Self, String> {
        let cert_path = moltchain_dir.join("node_cert.der");
        let key_path = moltchain_dir.join("node_key.der");

        if cert_path.exists() && key_path.exists() {
            let cert_bytes = fs::read(&cert_path)
                .map_err(|e| format!("Failed to read {}: {}", cert_path.display(), e))?;
            let key_bytes = fs::read(&key_path)
                .map_err(|e| format!("Failed to read {}: {}", key_path.display(), e))?;

            let fingerprint = Self::compute_fingerprint(&cert_bytes);
            let cert_der = CertificateDer::from(cert_bytes);

            info!(
                "🔑 P2P: Loaded persistent node identity (fingerprint: {})",
                Self::fingerprint_hex(&fingerprint)
            );
            Ok(NodeIdentity {
                cert_der,
                key_bytes,
                fingerprint,
            })
        } else {
            fs::create_dir_all(moltchain_dir)
                .map_err(|e| format!("Failed to create {}: {}", moltchain_dir.display(), e))?;

            let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
                .map_err(|e| format!("Failed to generate certificate: {}", e))?;

            let cert_der = CertificateDer::from(cert.cert);
            let cert_bytes = cert_der.as_ref().to_vec();
            let key_bytes = cert.key_pair.serialize_der();

            // Save to disk with fsync for durability
            Self::write_file(&cert_path, &cert_bytes)?;
            Self::write_file(&key_path, &key_bytes)?;

            let fingerprint = Self::compute_fingerprint(&cert_bytes);

            info!(
                "🔑 P2P: Generated new persistent node identity (fingerprint: {})",
                Self::fingerprint_hex(&fingerprint)
            );
            Ok(NodeIdentity {
                cert_der,
                key_bytes,
                fingerprint,
            })
        }
    }

    fn compute_fingerprint(cert_der: &[u8]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(cert_der);
        hasher.finalize().into()
    }

    fn fingerprint_hex(fp: &[u8; 32]) -> String {
        fp.iter().map(|b| format!("{:02x}", b)).collect()
    }

    fn write_file(path: &Path, data: &[u8]) -> Result<(), String> {
        use std::io::Write;
        let mut file = fs::File::create(path)
            .map_err(|e| format!("Failed to create {}: {}", path.display(), e))?;
        file.write_all(data)
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync {}: {}", path.display(), e))?;
        // P10-P2P-01: Restrict key/cert file permissions to owner-only
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
        }
        Ok(())
    }
}

/// AUDIT-FIX C1-01: TOFU (Trust On First Use) peer certificate fingerprint store.
/// Tracks known peer certificate fingerprints to detect identity changes.
/// Persists to ~/.moltchain/peer_fingerprints.json for durability across restarts.
struct PeerFingerprintStore {
    /// Map from peer address string to hex-encoded SHA-256 certificate fingerprint
    fingerprints: Mutex<HashMap<String, String>>,
    path: PathBuf,
}

impl PeerFingerprintStore {
    fn new(path: PathBuf) -> Self {
        let fingerprints: HashMap<String, String> = match fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => HashMap::new(),
        };
        PeerFingerprintStore {
            fingerprints: Mutex::new(fingerprints),
            path,
        }
    }

    /// Check a peer's certificate fingerprint against the TOFU store.
    /// Returns Ok(true) for new peers, Ok(false) for known peers with matching fingerprint,
    /// and Err for known peers with changed fingerprints (potential MITM/impersonation).
    fn check_or_store(&self, addr: &SocketAddr, fingerprint: &[u8; 32]) -> Result<bool, String> {
        let hex_fp = NodeIdentity::fingerprint_hex(fingerprint);
        let addr_str = addr.to_string();
        let mut store = self.fingerprints.lock().unwrap_or_else(|e| e.into_inner());

        match store.get(&addr_str) {
            Some(known) if *known == hex_fp => Ok(false), // known, matches
            Some(known) => Err(format!(
                "TOFU VIOLATION: Peer {} certificate fingerprint changed! \
                     Known: {}..., Got: {}... — possible MITM or unauthorized identity change.",
                addr,
                &known[..16],
                &hex_fp[..16]
            )),
            None => {
                store.insert(addr_str, hex_fp);
                drop(store); // release lock before I/O
                self.save();
                Ok(true) // new peer registered
            }
        }
    }

    /// Remove a peer's stored fingerprint to allow reconnection after legitimate
    /// certificate rotation. Returns true if the peer had a stored fingerprint.
    #[allow(dead_code)]
    pub fn remove_fingerprint(&self, addr: &SocketAddr) -> bool {
        let addr_str = addr.to_string();
        let mut store = self.fingerprints.lock().unwrap_or_else(|e| e.into_inner());
        let removed = store.remove(&addr_str).is_some();
        drop(store);
        if removed {
            self.save();
        }
        removed
    }

    fn save(&self) {
        let store = self.fingerprints.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(json) = serde_json::to_string_pretty(&*store) {
            if let Some(parent) = self.path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(mut file) = fs::File::create(&self.path) {
                use std::io::Write;
                let _ = file.write_all(json.as_bytes());
                let _ = file.sync_all();
            }
        }
    }
}

/// AUDIT-FIX C1-01: Verify a certificate is properly self-signed and return its SHA-256 fingerprint.
/// Uses x509-parser for robust X.509 parsing and ring for cryptographic signature verification.
/// This replaces the old SkipServerVerification which only checked DER tag formatting.
fn verify_self_signed_cert(cert_der: &[u8]) -> Result<[u8; 32], String> {
    use x509_parser::prelude::*;

    if cert_der.is_empty() {
        return Err("Empty certificate".to_string());
    }

    // Parse the X.509 certificate structure
    let (_, cert) = X509Certificate::from_der(cert_der)
        .map_err(|e| format!("Invalid X.509 certificate: {}", e))?;

    // Verify the certificate is self-signed: the signature on the certificate
    // must validate against the certificate's own public key. This prevents
    // attackers from presenting arbitrary certificates they cannot prove ownership of.
    // (None = verify against the certificate's own public key, i.e., self-signature check)
    cert.verify_signature(None)
        .map_err(|e| format!("Certificate self-signature verification failed: {:?}", e))?;

    // Compute SHA-256 fingerprint of the full certificate DER
    let fingerprint: [u8; 32] = {
        let mut hasher = Sha256::new();
        hasher.update(cert_der);
        hasher.finalize().into()
    };

    Ok(fingerprint)
}

/// AUDIT-FIX C1-01: Proper TLS server certificate verifier replacing SkipServerVerification.
/// Validates that peer certificates are properly self-signed X.509 certificates using
/// x509-parser + ring for cryptographic verification, instead of blindly accepting any
/// DER-formatted data. Combined with TOFU fingerprint pinning (done after connection
/// establishment in connect_peer/start_accepting) for complete peer identity verification.
#[derive(Debug)]
struct MoltCertVerifier;

impl rustls::client::danger::ServerCertVerifier for MoltCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer,
        _intermediates: &[CertificateDer],
        _server_name: &rustls::pki_types::ServerName,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let cert_data = end_entity.as_ref();

        // AUDIT-FIX C1-01: Cryptographic self-signature verification
        // Replaces the old verify_server_cert which only checked DER tag (0x30)
        // and length encoding. Now performs full X.509 parsing and verifies the
        // certificate's self-signature using the certificate's own public key.
        match verify_self_signed_cert(cert_data) {
            Ok(fingerprint) => {
                info!(
                    "P2P TLS: Verified peer certificate (fingerprint: {})",
                    NodeIdentity::fingerprint_hex(&fingerprint)
                );
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
            Err(e) => {
                warn!("P2P TLS: Server certificate verification FAILED: {}", e);
                Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::BadEncoding,
                ))
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        // C4 fix: Actually verify the handshake signature using the cert's public key
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
        // C4 fix: Actually verify the handshake signature using the cert's public key
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
            rustls::SignatureScheme::ED25519,
        ]
    }
}

/// AUDIT-FIX C1-01: Server-side client certificate verifier for mutual TLS.
/// Validates that connecting peers present properly self-signed certificates.
/// client_auth_mandatory=false for backwards compatibility with un-upgraded nodes.
#[derive(Debug)]
struct MoltClientCertVerifier;

impl rustls::server::danger::ClientCertVerifier for MoltClientCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        // P9-NET-02: Enforce mutual TLS — all peers MUST present a valid
        // self-signed certificate.  Without this, unauthenticated peers can
        // connect and inject malicious blocks/votes.  All nodes now generate
        // a certificate at startup so no backwards-compat concern remains.
        true
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        // No CA root hints — self-signed certs in a permissionless network
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer,
        _intermediates: &[CertificateDer],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        let cert_data = end_entity.as_ref();

        match verify_self_signed_cert(cert_data) {
            Ok(fingerprint) => {
                info!(
                    "P2P TLS: Verified client certificate (fingerprint: {})",
                    NodeIdentity::fingerprint_hex(&fingerprint)
                );
                Ok(rustls::server::danger::ClientCertVerified::assertion())
            }
            Err(e) => {
                warn!("P2P TLS: Client certificate verification FAILED: {}", e);
                Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::BadEncoding,
                ))
            }
        }
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
            rustls::SignatureScheme::ED25519,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::client::danger::ServerCertVerifier;

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
    // AUDIT-FIX C1-01 Tests: TLS certificate validation
    // =========================================================================

    /// Test that a genuine self-signed certificate passes verification
    #[test]
    fn test_c1_01_verify_self_signed_valid() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();

        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("Failed to generate cert");
        let cert_der = CertificateDer::from(cert.cert);
        let result = verify_self_signed_cert(cert_der.as_ref());
        assert!(
            result.is_ok(),
            "Valid self-signed cert should pass: {:?}",
            result
        );

        // Fingerprint should be 32 bytes (SHA-256)
        let fp = result.unwrap();
        assert_eq!(fp.len(), 32);
        // Non-zero fingerprint
        assert!(fp.iter().any(|&b| b != 0));
    }

    /// Test that an empty certificate is rejected
    #[test]
    fn test_c1_01_verify_self_signed_empty() {
        let result = verify_self_signed_cert(&[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Empty certificate"));
    }

    /// Test that random garbage bytes are rejected
    #[test]
    fn test_c1_01_verify_self_signed_garbage() {
        let garbage = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];
        let result = verify_self_signed_cert(&garbage);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid X.509"));
    }

    /// Test that a valid cert with a flipped bit in the signature fails
    #[test]
    fn test_c1_01_verify_self_signed_modified() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();

        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("Failed to generate cert");
        let cert_der = CertificateDer::from(cert.cert);
        let mut modified = cert_der.as_ref().to_vec();
        // Flip bit in last byte (part of the signature)
        if let Some(last) = modified.last_mut() {
            *last ^= 0x01;
        }
        let result = verify_self_signed_cert(&modified);
        // Should fail because self-signature no longer matches
        assert!(result.is_err(), "Modified cert should fail verification");
    }

    /// Test that same cert data produces same fingerprint (deterministic)
    #[test]
    fn test_c1_01_fingerprint_deterministic() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();

        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("Failed to generate cert");
        let cert_der = CertificateDer::from(cert.cert);
        let fp1 = verify_self_signed_cert(cert_der.as_ref()).unwrap();
        let fp2 = verify_self_signed_cert(cert_der.as_ref()).unwrap();
        assert_eq!(fp1, fp2, "Same cert should produce same fingerprint");
    }

    /// Test that different certs produce different fingerprints
    #[test]
    fn test_c1_01_fingerprint_unique() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();

        let cert1 = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("Failed to generate cert 1");
        let cert2 = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("Failed to generate cert 2");
        let fp1 = verify_self_signed_cert(CertificateDer::from(cert1.cert).as_ref()).unwrap();
        let fp2 = verify_self_signed_cert(CertificateDer::from(cert2.cert).as_ref()).unwrap();
        assert_ne!(
            fp1, fp2,
            "Different certs should produce different fingerprints"
        );
    }

    /// Test TOFU fingerprint store: new peer is accepted
    #[test]
    fn test_c1_01_tofu_new_peer() {
        let path = std::env::temp_dir().join(format!(
            "moltchain_tofu_new_{}_{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = PeerFingerprintStore::new(path.clone());
        let addr: SocketAddr = "10.0.0.1:8000".parse().unwrap();
        let fp = [42u8; 32];

        let result = store.check_or_store(&addr, &fp);
        assert!(result.is_ok());
        assert!(result.unwrap(), "New peer should return true");

        let _ = fs::remove_file(&path);
    }

    /// Test TOFU fingerprint store: known peer with same fingerprint is accepted
    #[test]
    fn test_c1_01_tofu_known_peer_match() {
        let path = std::env::temp_dir().join(format!(
            "moltchain_tofu_match_{}_{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = PeerFingerprintStore::new(path.clone());
        let addr: SocketAddr = "10.0.0.1:8000".parse().unwrap();
        let fp = [42u8; 32];

        // First connection: register
        assert!(store.check_or_store(&addr, &fp).unwrap());
        // Second connection: verify match
        let result = store.check_or_store(&addr, &fp);
        assert!(result.is_ok());
        assert!(!result.unwrap(), "Known peer should return false (not new)");

        let _ = fs::remove_file(&path);
    }

    /// Test TOFU fingerprint store: known peer with changed fingerprint is rejected
    #[test]
    fn test_c1_01_tofu_fingerprint_changed() {
        let path = std::env::temp_dir().join(format!(
            "moltchain_tofu_changed_{}_{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = PeerFingerprintStore::new(path.clone());
        let addr: SocketAddr = "10.0.0.1:8000".parse().unwrap();
        let fp1 = [42u8; 32];
        let fp2 = [99u8; 32];

        // First connection: register with fp1
        assert!(store.check_or_store(&addr, &fp1).unwrap());
        // Second connection: different fingerprint → TOFU violation
        let result = store.check_or_store(&addr, &fp2);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("TOFU VIOLATION"));

        let _ = fs::remove_file(&path);
    }

    /// Test TOFU fingerprint store: persistence across reloads
    #[test]
    fn test_c1_01_tofu_persistence() {
        let path = std::env::temp_dir().join(format!(
            "moltchain_tofu_persist_{}_{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let addr: SocketAddr = "10.0.0.1:8000".parse().unwrap();
        let fp = [42u8; 32];

        // Register peer in first store instance
        {
            let store = PeerFingerprintStore::new(path.clone());
            assert!(store.check_or_store(&addr, &fp).unwrap());
        }
        // Reload from disk — peer should still be known
        {
            let store = PeerFingerprintStore::new(path.clone());
            let result = store.check_or_store(&addr, &fp);
            assert!(result.is_ok());
            assert!(!result.unwrap(), "Peer should be known after reload");
        }
        // Reload — changed fingerprint should still be rejected
        {
            let store = PeerFingerprintStore::new(path.clone());
            let fp2 = [99u8; 32];
            let result = store.check_or_store(&addr, &fp2);
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("TOFU VIOLATION"));
        }

        let _ = fs::remove_file(&path);
    }

    /// Test fingerprint hex encoding
    #[test]
    fn test_c1_01_fingerprint_hex_encoding() {
        let fp = [
            0x00, 0x01, 0x0a, 0xff, 0xab, 0xcd, 0xef, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        let hex = NodeIdentity::fingerprint_hex(&fp);
        assert_eq!(hex.len(), 64, "SHA-256 hex should be 64 chars");
        assert!(hex.starts_with("00010aff"));
    }

    /// Test NodeIdentity::compute_fingerprint is SHA-256
    #[test]
    fn test_c1_01_compute_fingerprint_sha256() {
        // SHA-256 of empty input is known
        let fp_empty = NodeIdentity::compute_fingerprint(&[]);
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        assert_eq!(fp_empty[0], 0xe3);
        assert_eq!(fp_empty[1], 0xb0);
        assert_eq!(fp_empty[2], 0xc4);

        // Different input → different fingerprint
        let fp_data = NodeIdentity::compute_fingerprint(&[1, 2, 3]);
        assert_ne!(fp_empty, fp_data);
    }

    /// Test MoltCertVerifier accepts valid self-signed certificates
    #[test]
    fn test_c1_01_molt_cert_verifier_accepts_valid() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();

        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("Failed to generate cert");
        let cert_der = CertificateDer::from(cert.cert);

        let verifier = MoltCertVerifier;
        let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let result = verifier.verify_server_cert(
            &cert_der,
            &[],
            &server_name,
            &[],
            rustls::pki_types::UnixTime::now(),
        );
        assert!(
            result.is_ok(),
            "Valid self-signed cert should be accepted by MoltCertVerifier"
        );
    }

    /// Test MoltCertVerifier rejects garbage data
    #[test]
    fn test_c1_01_molt_cert_verifier_rejects_garbage() {
        let garbage = CertificateDer::from(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let verifier = MoltCertVerifier;
        let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let result = verifier.verify_server_cert(
            &garbage,
            &[],
            &server_name,
            &[],
            rustls::pki_types::UnixTime::now(),
        );
        assert!(
            result.is_err(),
            "Garbage data should be rejected by MoltCertVerifier"
        );
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

    #[tokio::test]
    async fn test_subnet_limit_in_connect_peer() {
        let (tx, _rx) = mpsc::channel(100);
        let mgr = PeerManager::new("127.0.0.1:0".parse().unwrap(), tx, None, None, 50, vec![])
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
        let (tx, _rx) = mpsc::channel(100);
        let mgr = PeerManager::new("127.0.0.1:0".parse().unwrap(), tx, None, None, 50, vec![])
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
        let (tx, _rx) = mpsc::channel(100);
        let mgr = PeerManager::new("127.0.0.1:0".parse().unwrap(), tx, None, None, 50, vec![])
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
        let (tx, _rx) = mpsc::channel(100);
        let mgr = PeerManager::new("127.0.0.1:0".parse().unwrap(), tx, None, None, 50, vec![])
            .await
            .unwrap();

        let stats = mgr.bandwidth_stats(&"10.0.0.1:7001".parse().unwrap());
        assert!(stats.is_none());
    }

    #[test]
    fn test_recv_bandwidth_bps() {
        let mut peer = PeerInfo::new("10.0.0.1:7001".parse().unwrap());
        // tracking_since is ~now, so elapsed ≈ 1 (clamped min)
        peer.add_bytes_received(10_000);
        let bps = peer.recv_bandwidth_bps();
        assert!(bps >= 1.0, "bps should be positive, got {}", bps);
    }
}
