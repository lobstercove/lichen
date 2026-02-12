// Peer Connection Management

use crate::message::P2PMessage;
use crate::peer_ban::PeerBanList;
use crate::peer_store::PeerStore;
use dashmap::DashMap;
use quinn::{Connection, Endpoint, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Peer information
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub address: SocketAddr,
    pub connection: Option<Connection>,
    pub last_seen: u64,
    pub reputation: u64,
    pub is_validator: bool,
    pub score: i64,
}

impl PeerInfo {
    pub fn new(address: SocketAddr) -> Self {
        PeerInfo {
            address,
            connection: None,
            last_seen: SystemTime::now()
                .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
                .as_secs(),
            reputation: 500,
            is_validator: false,
            score: 0,
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
}

impl PeerManager {
    /// Create new peer manager
    pub async fn new(
        local_addr: SocketAddr,
        message_tx: mpsc::Sender<(SocketAddr, P2PMessage)>,
        peer_store: Option<Arc<PeerStore>>,
    ) -> Result<Self, String> {
        // Install crypto provider for rustls (required by quinn)
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok(); // Ignore error if already installed

        // Generate self-signed certificate for QUIC
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .map_err(|e| format!("Failed to generate certificate: {}", e))?;

        let cert_der = CertificateDer::from(cert.cert);
        let key_der = PrivateKeyDer::try_from(cert.key_pair.serialize_der())
            .map_err(|e| format!("Failed to serialize key: {}", e))?;

        // Configure rustls with ALPN
        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .map_err(|e| format!("Failed to create rustls config: {}", e))?;

        server_crypto.alpn_protocols = vec![b"molt".to_vec()];

        // Configure QUIC server
        let server_config = ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
                .map_err(|e| format!("Failed to create QUIC server config: {}", e))?,
        ));

        // Create QUIC endpoint
        let endpoint = Endpoint::server(server_config, local_addr)
            .map_err(|e| format!("Failed to create endpoint: {}", e))?;

        info!("🦞 P2P: QUIC endpoint listening on {}", local_addr);

        let ban_list_path = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".moltchain/peer-banlist.json");

        Ok(PeerManager {
            peers: Arc::new(DashMap::new()),
            endpoint,
            local_addr,
            message_tx,
            peer_store,
            ban_list: Arc::new(Mutex::new(PeerBanList::new(ban_list_path))),
        })
    }

    /// Maximum number of concurrent peer connections
    pub const MAX_PEERS: usize = 50;

    /// Connect to a peer
    pub async fn connect_peer(&self, peer_addr: SocketAddr) -> Result<(), String> {
        if self.ban_list.lock().unwrap_or_else(|e| e.into_inner()).is_banned(&peer_addr) {
            return Err("Peer is banned".to_string());
        }
        if self.peers.contains_key(&peer_addr) {
            return Ok(());
        }
        if self.peers.len() >= Self::MAX_PEERS {
            return Err(format!(
                "Max peer limit reached ({}), rejecting {}",
                Self::MAX_PEERS,
                peer_addr
            ));
        }

        info!("🦞 P2P: Connecting to peer {}", peer_addr);

        // Skip TLS verification for local development
        let mut rustls_config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
            .with_no_client_auth();

        // Configure ALPN
        rustls_config.alpn_protocols = vec![b"molt".to_vec()];

        let client_config = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(rustls_config)
                .map_err(|e| format!("Failed to create QUIC config: {}", e))?,
        ));
        let mut endpoint = self.endpoint.clone();
        endpoint.set_default_client_config(client_config);

        // Connect
        let connection = endpoint
            .connect(peer_addr, "localhost")
            .map_err(|e| format!("Failed to connect: {}", e))?
            .await
            .map_err(|e| format!("Connection failed: {}", e))?;

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
        tokio::spawn(async move {
            if let Err(e) = handle_connection(connection, peer_addr, peers, message_tx).await {
                error!("P2P: Connection error with {}: {}", peer_addr, e);
            }
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

            Ok(())
        } else {
            Err("No active connection".to_string())
        }
    }

    /// Broadcast message to all peers
    pub async fn broadcast(&self, message: P2PMessage) {
        for entry in self.peers.iter() {
            let peer_addr = *entry.key();
            if let Err(e) = self.send_to_peer(&peer_addr, message.clone()).await {
                warn!("P2P: Failed to send to {}: {}", peer_addr, e);
            }
        }
    }

    /// Get all peer addresses
    pub fn get_peers(&self) -> Vec<SocketAddr> {
        self.peers.iter().map(|entry| *entry.key()).collect()
    }

    /// Record a peer violation (rate limit or invalid request)
    pub fn record_violation(&self, peer_addr: &SocketAddr) {
        if let Some(mut peer) = self.peers.get_mut(peer_addr) {
            peer.adjust_score(-2);
            if peer.score <= -10 {
                let addr = *peer_addr;
                self.ban_list.lock().unwrap_or_else(|e| e.into_inner()).record_score(addr, peer.score);
                drop(peer);
                self.peers.remove(&addr);
                warn!("P2P: Removed peer {} due to low score", addr);
            }
        }
    }

    /// Record a peer success (valid request/response)
    pub fn record_success(&self, peer_addr: &SocketAddr) {
        if let Some(mut peer) = self.peers.get_mut(peer_addr) {
            peer.adjust_score(1);
        }
    }

    pub fn prune_ban_list(&self) {
        self.ban_list.lock().unwrap_or_else(|e| e.into_inner()).prune();
    }

    /// Start accepting connections
    pub async fn start_accepting(&self) {
        let endpoint = self.endpoint.clone();
        let peers = self.peers.clone();
        let message_tx = self.message_tx.clone();
        let peer_store = self.peer_store.clone();
        let ban_list = self.ban_list.clone();

        tokio::spawn(async move {
            while let Some(connecting) = endpoint.accept().await {
                let peers = peers.clone();
                let message_tx = message_tx.clone();
                let peer_store = peer_store.clone();
                let ban_list = ban_list.clone();

                tokio::spawn(async move {
                    match connecting.await {
                        Ok(connection) => {
                            let peer_addr = connection.remote_address();
                            if ban_list.lock().unwrap_or_else(|e| e.into_inner()).is_banned(&peer_addr) {
                                warn!("P2P: Rejected banned peer {}", peer_addr);
                                return;
                            }
                            // Enforce MAX_PEERS on inbound connections too
                            if peers.len() >= PeerManager::MAX_PEERS {
                                warn!("P2P: Rejected inbound connection from {} — at max peers ({})", peer_addr, PeerManager::MAX_PEERS);
                                return;
                            }
                            info!("🦞 P2P: Accepted connection from {}", peer_addr);

                            // Store peer
                            let mut peer_info = PeerInfo::new(peer_addr);
                            peer_info.connection = Some(connection.clone());
                            peers.insert(peer_addr, peer_info);
                            if let Some(store) = &peer_store {
                                store.record_peer(peer_addr);
                            }

                            // Handle connection
                            if let Err(e) =
                                handle_connection(connection, peer_addr, peers, message_tx).await
                            {
                                error!("P2P: Connection error with {}: {}", peer_addr, e);
                            }
                        }
                        Err(e) => {
                            error!("P2P: Failed to accept connection: {}", e);
                        }
                    }
                });
            }
        });
    }

    /// Clean up stale peers
    pub fn cleanup_stale_peers(&self, timeout_secs: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
            .as_secs();
        let mut to_remove = Vec::new();

        for entry in self.peers.iter() {
            if now - entry.value().last_seen > timeout_secs {
                to_remove.push(*entry.key());
            }
        }

        for addr in to_remove {
            info!("🦞 P2P: Removing stale peer {}", addr);
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
) -> Result<(), String> {
    let mut deser_failures: u32 = 0;
    const MAX_DESER_FAILURES: u32 = 10;

    loop {
        let mut stream = connection
            .accept_uni()
            .await
            .map_err(|e| format!("Failed to accept stream: {}", e))?;

        let bytes = stream
            .read_to_end(2 * 1024 * 1024) // T4.8: 2MB max (enough for max block)
            .await
            .map_err(|e| format!("Failed to read: {}", e))?;

        // Deserialize message
        match P2PMessage::deserialize(&bytes) {
            Ok(message) => {
                deser_failures = 0; // reset on success
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
                warn!(
                    "P2P: Failed to deserialize message from {} ({}/{}): {}",
                    peer_addr, deser_failures, MAX_DESER_FAILURES, e
                );
                // H18 fix: disconnect after too many consecutive failures
                if deser_failures >= MAX_DESER_FAILURES {
                    warn!("P2P: Disconnecting {} — too many deserialization failures", peer_addr);
                    if let Some(mut peer) = peers.get_mut(&peer_addr) {
                        peer.score -= 20;
                    }
                    return Err(format!("Too many deserialization failures from {}", peer_addr));
                }
            }
        }
    }
}

/// Skip TLS server verification (for local development)
#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer,
        _intermediates: &[CertificateDer],
        _server_name: &rustls::pki_types::ServerName,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // T2.1 fix: Validate certificate is well-formed DER/X.509 instead of
        // blindly accepting anything. This prevents trivial MITM with garbage
        // data while still allowing self-signed certs (permissionless network).
        let cert_data = end_entity.as_ref();
        if cert_data.is_empty() {
            warn!("P2P: Rejecting empty certificate from peer");
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ));
        }
        // X.509 certificates must start with ASN.1 SEQUENCE tag (0x30)
        if cert_data[0] != 0x30 {
            warn!(
                "P2P: Rejecting certificate with invalid DER tag: 0x{:02x}",
                cert_data[0]
            );
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ));
        }
        // Minimum viable DER: tag + length + content needs at least 4 bytes
        if cert_data.len() < 4 {
            warn!(
                "P2P: Rejecting certificate too short ({} bytes)",
                cert_data.len()
            );
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ));
        }
        // Validate DER length encoding is consistent with actual data length
        let len_byte = cert_data[1];
        let (claimed_len, header_size) = if len_byte < 0x80 {
            // Short form: length is directly in the byte
            (len_byte as usize, 2usize)
        } else if len_byte == 0x81 && cert_data.len() > 2 {
            (cert_data[2] as usize, 3usize)
        } else if len_byte == 0x82 && cert_data.len() > 3 {
            (
                ((cert_data[2] as usize) << 8) | cert_data[3] as usize,
                4usize,
            )
        } else {
            // For very large certs (0x83/0x84) or truncated length, just accept
            // since parsing the content is beyond minimal validation
            (0, 0)
        };
        if header_size > 0 && header_size + claimed_len != cert_data.len() {
            warn!(
                "P2P: Certificate DER length mismatch: header {} + claimed {} != actual {}",
                header_size,
                claimed_len,
                cert_data.len()
            );
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ));
        }
        // Accept self-signed certificates (permissionless network)
        Ok(rustls::client::danger::ServerCertVerified::assertion())
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
}
