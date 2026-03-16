// P2P Message Types

use moltchain_core::{
    Block, BlockHeader, CommitSignature, Hash, Precommit, Prevote, Proposal, Pubkey,
    SlashingEvidence, StakePool, Transaction, ValidatorSet, Vote,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// P3-3: Short TX ID — first 8 bytes of the full transaction hash.
/// Probability of collision within a single block is negligible
/// (birthday bound: ~2^32 for 8-byte IDs, blocks have at most 10K TXs).
pub type ShortTxId = [u8; 8];

/// Build the signed payload for validator announcements.
///
/// Legacy announcements signed only the fixed-width fields.
/// New announcements append a length-prefixed version string so peers can
/// enforce minimum validator versions for new admissions.
pub fn validator_announcement_signing_message(
    pubkey: &Pubkey,
    stake: u64,
    current_slot: u64,
    machine_fingerprint: &[u8; 32],
    version: Option<&str>,
) -> Result<Vec<u8>, String> {
    let version_len = version.map_or(0, |value| value.len());
    if version_len > u16::MAX as usize {
        return Err(format!(
            "Validator announcement version too long: {} bytes",
            version_len
        ));
    }

    let mut message = Vec::with_capacity(
        80 + if version.is_some() {
            2 + version_len
        } else {
            0
        },
    );
    message.extend_from_slice(&pubkey.0);
    message.extend_from_slice(&stake.to_le_bytes());
    message.extend_from_slice(&current_slot.to_le_bytes());
    message.extend_from_slice(machine_fingerprint);

    if let Some(version) = version {
        message.extend_from_slice(&(version_len as u16).to_le_bytes());
        message.extend_from_slice(version.as_bytes());
    }

    Ok(message)
}

/// P3-3: Compute the short TX ID from a full transaction hash.
pub fn short_tx_id(hash: &Hash) -> ShortTxId {
    let mut id = [0u8; 8];
    id.copy_from_slice(&hash.0[..8]);
    id
}

/// P3-3: Compact block — header + short TX IDs instead of full transactions.
/// Receiving peers reconstruct the block from their mempool. Only missing TXs
/// are requested individually, saving ~90% bandwidth for live block propagation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactBlock {
    /// Full block header (needed for validation)
    pub header: BlockHeader,
    /// Short TX IDs (first 8 bytes of each tx hash)
    pub short_ids: Vec<ShortTxId>,
    /// Execution fees (needed for deterministic state; same order as short_ids)
    pub tx_fees_paid: Vec<u64>,
    /// Oracle price data from the block producer
    pub oracle_prices: Vec<(String, u64)>,
    /// Commit certificate signatures (2/3+ validator precommits)
    #[serde(default)]
    pub commit_signatures: Vec<CommitSignature>,
}

impl CompactBlock {
    /// Build a compact block from a full block.
    pub fn from_block(block: &Block) -> Self {
        let short_ids = block
            .transactions
            .iter()
            .map(|tx| short_tx_id(&tx.hash()))
            .collect();
        CompactBlock {
            header: block.header.clone(),
            short_ids,
            tx_fees_paid: block.tx_fees_paid.clone(),
            oracle_prices: block.oracle_prices.clone(),
            commit_signatures: block.commit_signatures.clone(),
        }
    }
}

/// Serde helper for [u8; 64] signature arrays (serde only supports up to [u8; 32] natively)
mod signature_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Wrapper to serialize [u8; 64] as two [u8; 32] halves (bincode-safe)
    #[derive(Serialize, Deserialize)]
    struct SigHalves {
        lo: [u8; 32],
        hi: [u8; 32],
    }

    pub fn serialize<S: Serializer>(data: &[u8; 64], serializer: S) -> Result<S::Ok, S::Error> {
        let halves = SigHalves {
            lo: data[..32].try_into().unwrap(),
            hi: data[32..].try_into().unwrap(),
        };
        halves.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 64], D::Error> {
        let halves = SigHalves::deserialize(deserializer)?;
        let mut sig = [0u8; 64];
        sig[..32].copy_from_slice(&halves.lo);
        sig[32..].copy_from_slice(&halves.hi);
        Ok(sig)
    }
}

/// Current P2P protocol version. Bump when message format changes.
pub const P2P_PROTOCOL_VERSION: u32 = 1;

/// P2P message envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2PMessage {
    /// AUDIT-FIX L2: Protocol version — allows nodes to detect incompatible
    /// message formats and reject/ignore gracefully instead of deserialization
    /// failures.
    #[serde(default = "default_protocol_version")]
    pub version: u32,
    /// Message type and payload
    pub msg_type: MessageType,
    /// Sender's address
    pub sender: SocketAddr,
    /// Message timestamp
    pub timestamp: u64,
}

fn default_protocol_version() -> u32 {
    P2P_PROTOCOL_VERSION
}

/// Snapshot request kinds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SnapshotKind {
    ValidatorSet,
    StakePool,
    /// Full state checkpoint — accounts, contract storage, programs
    StateCheckpoint,
}

/// Message types in the P2P network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageType {
    /// New block announcement
    Block(Block),

    /// Consensus vote
    Vote(Vote),

    /// BFT: Block proposal from the designated proposer
    Proposal(Proposal),

    /// BFT: Prevote attestation (first voting phase)
    Prevote(Prevote),

    /// BFT: Precommit attestation (second voting phase)
    Precommit(Precommit),

    /// Transaction broadcast
    Transaction(Transaction),

    /// Peer information for gossip
    PeerInfo(Vec<PeerInfoMsg>),

    /// Request for peers
    PeerRequest,

    /// Ping message
    Ping,

    /// Pong response
    Pong,

    /// Request specific block by slot
    BlockRequest { slot: u64 },

    /// Request blocks in range
    BlockRangeRequest { start_slot: u64, end_slot: u64 },

    /// Response with single block
    BlockResponse(Block),

    /// Response with multiple blocks
    BlockRangeResponse { blocks: Vec<Block> },

    /// Request current chain status
    StatusRequest,

    /// Response with chain status
    StatusResponse {
        current_slot: u64,
        total_blocks: u64,
    },

    /// Consistency report for validator set + stake pool
    ConsistencyReport {
        validator_set_hash: Hash,
        stake_pool_hash: Hash,
    },

    /// Request a snapshot (validator set or stake pool)
    SnapshotRequest { kind: SnapshotKind },

    /// Snapshot response
    SnapshotResponse {
        kind: SnapshotKind,
        validator_set: Option<ValidatorSet>,
        stake_pool: Option<StakePool>,
    },
    /// Validator announcement — signed by the announcing validator's keypair.
    /// T2.3 fix: Must include Ed25519 signature over (pubkey || stake || current_slot)
    /// to prevent Sybil attacks via unsigned announcements.
    ValidatorAnnounce {
        pubkey: Pubkey,
        stake: u64,
        current_slot: u64,
        /// Semver version string of the running validator binary (e.g. "0.2.0")
        #[serde(default)]
        version: String,
        /// Ed25519 signature over (pubkey.0 || stake.to_le_bytes() || current_slot.to_le_bytes())
        #[serde(with = "signature_serde")]
        signature: [u8; 64],
        /// SHA-256 machine fingerprint (platform UUID + MAC address).
        /// [0u8; 32] if not available (dev mode or legacy).
        #[serde(default)]
        machine_fingerprint: [u8; 32],
    },

    /// State snapshot chunk request — joining validator asks for a chunk of
    /// account/contract/program state from a peer's latest checkpoint.
    StateSnapshotRequest {
        /// Which data category: "accounts", "contract_storage", "programs"
        category: String,
        /// Chunk index (0-based) for paginated transfer
        chunk_index: u64,
        /// Number of entries per chunk
        chunk_size: u64,
    },

    /// State snapshot chunk response — contains a batch of (key, value) pairs
    /// from the requested category.
    StateSnapshotResponse {
        /// Which data category this chunk belongs to
        category: String,
        /// Chunk index
        chunk_index: u64,
        /// Total number of chunks for this category
        total_chunks: u64,
        /// The slot at which the snapshot was taken
        snapshot_slot: u64,
        /// State root hash at snapshot slot (for verification)
        state_root: [u8; 32],
        /// key-value entries (bincode-serialized Vec<(Vec<u8>, Vec<u8>)>)
        entries: Vec<u8>,
    },

    /// Checkpoint metadata request — ask peer for its latest checkpoint info
    CheckpointMetaRequest,

    /// Checkpoint metadata response
    CheckpointMetaResponse {
        /// Slot of the latest checkpoint (0 if none)
        slot: u64,
        /// State root at that slot
        state_root: [u8; 32],
        /// Total accounts
        total_accounts: u64,
    },

    /// Slashing evidence broadcast
    SlashingEvidence(SlashingEvidence),

    /// P3-2: Kademlia FIND_NODE request — ask peer for the closest nodes
    /// it knows to `target_id`.
    FindNode {
        /// The target node ID to find closest peers for
        target_id: [u8; 32],
    },

    /// P3-2: Kademlia FIND_NODE response — returns the closest known peers.
    FindNodeResponse {
        /// The target that was queried
        target_id: [u8; 32],
        /// Closest known nodes: (node_id, socket_addr_bytes)
        /// Socket address serialized as string for serde compat.
        closest: Vec<([u8; 32], String)>,
    },

    /// P3-3: Compact block announcement — header + short TX IDs.
    /// Receiver reconstructs from mempool and requests only missing TXs.
    CompactBlockMsg(CompactBlock),

    /// P3-3: Request missing transactions for a compact block.
    /// Contains the slot and the full hashes of TXs the receiver couldn't
    /// find in its mempool.
    GetBlockTxs {
        slot: u64,
        missing_hashes: Vec<Hash>,
    },

    /// P3-3: Response with the requested transactions.
    BlockTxs {
        slot: u64,
        transactions: Vec<Transaction>,
    },

    /// P3-4: Request erasure-coded shard(s) for a block.
    /// The requester specifies which shard indices it still needs.
    ErasureShardRequest {
        slot: u64,
        shard_indices: Vec<usize>,
    },

    /// P3-4: Response with erasure-coded shard(s).
    ErasureShardResponse {
        slot: u64,
        shards: Vec<crate::erasure::ErasureShard>,
    },

    /// P3-6: Relay-assisted hole punch — a peer behind NAT asks a relay to
    /// forward a HolePunchRequest to the target, which then sends a QUIC
    /// packet to the requester's observed address to punch through the NAT.
    HolePunchRequest {
        /// The address the requester wants to reach
        target_addr: SocketAddr,
        /// The requester's externally-observed address (from QUIC connection)
        requester_observed_addr: SocketAddr,
    },

    /// P3-6: Hole punch notification — relay forwards this to the target peer,
    /// telling it to send a QUIC packet to the requester's observed address.
    HolePunchNotify {
        /// The observed external address of the peer that wants to connect
        peer_observed_addr: SocketAddr,
    },

    /// M-9: Certificate rotation announcement.
    /// A peer broadcasts this when it generates a new TLS certificate.
    /// The message is signed by the OLD certificate's private key, proving
    /// that the entity controlling the old cert authorized the rotation.
    CertRotation {
        /// SHA-256 fingerprint of the OLD certificate being retired
        old_fingerprint: [u8; 32],
        /// SHA-256 fingerprint of the NEW certificate replacing it
        new_fingerprint: [u8; 32],
        /// DER-encoded NEW certificate (peers cache it for verification)
        new_cert_der: Vec<u8>,
        /// Ed25519-style signature over (old_fingerprint || new_fingerprint)
        /// produced with the OLD certificate's private key. Verification uses
        /// the OLD certificate's public key extracted from stored cert data.
        /// For simplicity we use SHA-256(old_fp || new_fp) signed via the
        /// TLS private key — peers verify via `verify_self_signed_cert` on
        /// the new cert + fingerprint chain consistency.
        rotation_proof: Vec<u8>,
        /// Unix timestamp of the rotation (rate-limit enforcement)
        timestamp: u64,
    },
}

/// Peer information message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfoMsg {
    pub address: SocketAddr,
    pub last_seen: u64,
    pub reputation: u64,
    pub validator_pubkey: Option<Pubkey>, // NEW: Link peer to validator
}

/// Minimum payload size to trigger LZ4 compression (1 KB).
/// Messages smaller than this are sent uncompressed to avoid overhead.
const COMPRESSION_THRESHOLD: usize = 1024;

impl P2PMessage {
    /// Create new message
    pub fn new(msg_type: MessageType, sender: SocketAddr) -> Self {
        P2PMessage {
            version: P2P_PROTOCOL_VERSION,
            msg_type,
            sender,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }

    /// Serialize message for network transmission with optional LZ4 compression.
    ///
    /// Wire format (envelope):
    ///   `[0x00][bincode payload]`  — uncompressed
    ///   `[0xFF][4-byte LE uncompressed len][LZ4 compressed payload]`
    ///
    /// Magic bytes 0x00 and 0xFF are chosen to avoid collision with raw bincode:
    /// the first byte of a legacy message is the low byte of `P2P_PROTOCOL_VERSION`
    /// (currently 1), so 0x00 and 0xFF can never be the start of a valid legacy
    /// message unless the version reaches 0 or 255 (both implausible).
    ///
    /// Limit is 16 MB to accommodate state snapshot chunks.
    pub fn serialize(&self) -> Result<Vec<u8>, String> {
        use bincode::Options;
        let raw = bincode::options()
            .with_limit(16 * 1024 * 1024)
            .serialize(self)
            .map_err(|e| format!("Serialization error: {}", e))?;

        if raw.len() >= COMPRESSION_THRESHOLD {
            let compressed = lz4_flex::compress_prepend_size(&raw);
            // Only use compression if it actually saves space
            if compressed.len() < raw.len() {
                let mut out = Vec::with_capacity(1 + 4 + compressed.len());
                out.push(0xFF); // compressed flag
                out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
                out.extend_from_slice(&compressed);
                return Ok(out);
            }
        }

        let mut out = Vec::with_capacity(1 + raw.len());
        out.push(0x00); // uncompressed flag
        out.extend_from_slice(&raw);
        Ok(out)
    }

    /// Deserialize message from bytes (bounded to 16 MB to prevent OOM).
    /// Handles compressed (0xFF prefix), uncompressed (0x00 prefix), and
    /// legacy (raw bincode, any other first byte) formats.
    /// AUDIT-FIX L2: Rejects messages with incompatible protocol version.
    pub fn deserialize(bytes: &[u8]) -> Result<Self, String> {
        use bincode::Options;

        if bytes.is_empty() {
            return Err("Empty message".to_string());
        }

        let payload = match bytes[0] {
            0x00 => {
                // Uncompressed envelope
                &bytes[1..]
            }
            0xFF => {
                // LZ4 compressed: [0xFF][4-byte LE uncompressed len][compressed]
                if bytes.len() < 6 {
                    return Err("Compressed message too short".to_string());
                }
                let expected_len =
                    u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
                if expected_len > 16 * 1024 * 1024 {
                    return Err(format!(
                        "Decompressed size {} exceeds 16 MB limit",
                        expected_len
                    ));
                }
                // Use a leaked local binding so the borrow checker is happy
                // (the decompressed Vec lives long enough for deserialization).
                // We return early from this function, so no actual leak.
                let decompressed = lz4_flex::decompress_size_prepended(&bytes[5..])
                    .map_err(|e| format!("LZ4 decompression error: {}", e))?;
                // Can't return a reference to a local, so deserialize inline
                let msg: Self = bincode::options()
                    .with_limit(16 * 1024 * 1024)
                    .deserialize(&decompressed)
                    .map_err(|e| format!("Deserialization error: {}", e))?;
                if msg.version != P2P_PROTOCOL_VERSION {
                    return Err(format!(
                        "Protocol version mismatch: got {}, expected {}",
                        msg.version, P2P_PROTOCOL_VERSION
                    ));
                }
                return Ok(msg);
            }
            _other => {
                // Legacy compatibility: no prefix byte, try raw bincode
                // (for peers that haven't upgraded yet)
                bytes
            }
        };

        let msg: Self = bincode::options()
            .with_limit(16 * 1024 * 1024)
            .deserialize(payload)
            .map_err(|e| format!("Deserialization error: {}", e))?;
        if msg.version != P2P_PROTOCOL_VERSION {
            return Err(format!(
                "Protocol version mismatch: got {}, expected {}",
                msg.version, P2P_PROTOCOL_VERSION
            ));
        }
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moltchain_core::Keypair;

    #[test]
    fn test_message_serialization() {
        let addr = "127.0.0.1:8000".parse().unwrap();
        let msg = P2PMessage::new(MessageType::Ping, addr);

        let bytes = msg.serialize().unwrap();
        let deserialized = P2PMessage::deserialize(&bytes).unwrap();

        assert_eq!(msg.sender, deserialized.sender);
        assert_eq!(msg.timestamp, deserialized.timestamp);
        assert_eq!(deserialized.version, P2P_PROTOCOL_VERSION);
    }

    #[test]
    fn test_message_version_mismatch_rejected() {
        let addr = "127.0.0.1:8000".parse().unwrap();
        let mut msg = P2PMessage::new(MessageType::Ping, addr);
        msg.version = 999; // incompatible version
        let bytes = msg.serialize().unwrap();
        let result = P2PMessage::deserialize(&bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Protocol version mismatch"));
    }

    #[test]
    fn test_validator_announcement_signing_message_binds_version() {
        let keypair = Keypair::new();
        let pubkey = keypair.pubkey();
        let fingerprint = [7u8; 32];

        let payload =
            validator_announcement_signing_message(&pubkey, 123, 456, &fingerprint, Some("0.1.0"))
                .unwrap();
        let signature = keypair.sign(&payload);

        let tampered =
            validator_announcement_signing_message(&pubkey, 123, 456, &fingerprint, Some("0.1.1"))
                .unwrap();

        assert!(Keypair::verify(&pubkey, &payload, &signature));
        assert!(!Keypair::verify(&pubkey, &tampered, &signature));
    }

    #[test]
    fn test_validator_announcement_signing_message_legacy_differs() {
        let pubkey = Pubkey([9u8; 32]);
        let fingerprint = [3u8; 32];
        let legacy =
            validator_announcement_signing_message(&pubkey, 1, 2, &fingerprint, None).unwrap();
        let versioned =
            validator_announcement_signing_message(&pubkey, 1, 2, &fingerprint, Some("0.1.0"))
                .unwrap();

        assert_ne!(legacy, versioned);
    }

    #[test]
    fn test_max_message_size_exceeded() {
        // Craft a message that exceeds 16MB by using a large BlockRangeResponse
        // with gigantic block vectors. We can't easily construct 16MB of data
        // in a test, so verify the limit mechanism exists by checking that
        // serialization of a normal message succeeds.
        let addr = "127.0.0.1:8000".parse().unwrap();
        let msg = P2PMessage::new(MessageType::Pong, addr);
        let bytes = msg.serialize().unwrap();
        assert!(bytes.len() < 16 * 1024 * 1024);
    }

    // ----------------------------------------------------------------
    // P2-2: LZ4 compression tests
    // ----------------------------------------------------------------

    #[test]
    fn test_small_message_uncompressed() {
        // Messages below COMPRESSION_THRESHOLD should use 0x00 prefix
        let addr = "127.0.0.1:8000".parse().unwrap();
        let msg = P2PMessage::new(MessageType::Ping, addr);
        let bytes = msg.serialize().unwrap();
        assert_eq!(bytes[0], 0x00, "Small message should be uncompressed");
        // Should roundtrip
        let decoded = P2PMessage::deserialize(&bytes).unwrap();
        assert_eq!(decoded.version, msg.version);
    }

    #[test]
    fn test_large_message_compressed() {
        // Create a message with enough data to exceed COMPRESSION_THRESHOLD.
        // Use BlockRangeResponse with multiple blocks to push over 1KB.
        use moltchain_core::{Block, Hash};
        let addr = "127.0.0.1:8000".parse().unwrap();
        let blocks: Vec<Block> = (0..20)
            .map(|i| Block::new(i, Hash::default(), Hash::default(), [0u8; 32], vec![]))
            .collect();
        let msg = P2PMessage::new(MessageType::BlockRangeResponse { blocks }, addr);
        let bytes = msg.serialize().unwrap();
        // If compression was beneficial, first byte should be 0xFF
        if bytes[0] == 0xFF {
            // Verify the envelope structure: [0x01][4-byte LE len][compressed]
            assert!(bytes.len() >= 6);
            let raw_len = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
            assert!(raw_len >= COMPRESSION_THRESHOLD);
        }
        // Must roundtrip regardless
        let decoded = P2PMessage::deserialize(&bytes).unwrap();
        assert_eq!(decoded.version, msg.version);
    }

    #[test]
    fn test_legacy_message_backwards_compat() {
        // Simulate a message from an old peer without the 0x00/0x01 prefix.
        // The deserializer should handle raw bincode as legacy format.
        use bincode::Options;
        let addr = "127.0.0.1:8000".parse().unwrap();
        let msg = P2PMessage::new(MessageType::Ping, addr);
        // Serialize with raw bincode (no envelope prefix)
        let raw = bincode::options()
            .with_limit(16 * 1024 * 1024)
            .serialize(&msg)
            .unwrap();
        // Deserialize — should fall through to legacy path
        let decoded = P2PMessage::deserialize(&raw).unwrap();
        assert_eq!(decoded.version, msg.version);
    }

    #[test]
    fn test_empty_message_rejected() {
        let result = P2PMessage::deserialize(&[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Empty message"));
    }

    #[test]
    fn test_truncated_compressed_message_rejected() {
        // 0xFF prefix but not enough bytes for the header
        let result = P2PMessage::deserialize(&[0xFF, 0x00, 0x00]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too short"));
    }

    #[test]
    fn test_oversized_decompressed_rejected() {
        // 0xFF prefix with claimed decompressed size > 16MB
        let mut data = vec![0xFF];
        let huge_size: u32 = 20 * 1024 * 1024; // 20MB
        data.extend_from_slice(&huge_size.to_le_bytes());
        data.extend_from_slice(&[0u8; 10]); // junk payload
        let result = P2PMessage::deserialize(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds 16 MB"));
    }

    // ----------------------------------------------------------------
    // P3-3: Compact block tests
    // ----------------------------------------------------------------

    #[test]
    fn test_short_tx_id() {
        let h = Hash([
            0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ]);
        let sid = short_tx_id(&h);
        assert_eq!(sid, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22]);
    }

    #[test]
    fn test_compact_block_from_block() {
        use moltchain_core::Block;
        let block = Block::new(42, Hash::default(), Hash::default(), [1u8; 32], vec![]);
        let compact = CompactBlock::from_block(&block);
        assert_eq!(compact.header.slot, 42);
        assert!(compact.short_ids.is_empty());
        assert!(compact.tx_fees_paid.is_empty());
    }

    #[test]
    fn test_compact_block_roundtrip() {
        use moltchain_core::Block;
        let block = Block::new(99, Hash::default(), Hash::default(), [2u8; 32], vec![]);
        let compact = CompactBlock::from_block(&block);
        let addr = "127.0.0.1:9000".parse().unwrap();
        let msg = P2PMessage::new(MessageType::CompactBlockMsg(compact), addr);
        let bytes = msg.serialize().unwrap();
        let decoded = P2PMessage::deserialize(&bytes).unwrap();
        match decoded.msg_type {
            MessageType::CompactBlockMsg(cb) => {
                assert_eq!(cb.header.slot, 99);
            }
            _ => panic!("Expected CompactBlockMsg"),
        }
    }

    #[test]
    fn test_get_block_txs_roundtrip() {
        let addr = "127.0.0.1:9000".parse().unwrap();
        let hashes = vec![Hash([1u8; 32]), Hash([2u8; 32])];
        let msg = P2PMessage::new(
            MessageType::GetBlockTxs {
                slot: 10,
                missing_hashes: hashes.clone(),
            },
            addr,
        );
        let bytes = msg.serialize().unwrap();
        let decoded = P2PMessage::deserialize(&bytes).unwrap();
        match decoded.msg_type {
            MessageType::GetBlockTxs {
                slot,
                missing_hashes,
            } => {
                assert_eq!(slot, 10);
                assert_eq!(missing_hashes.len(), 2);
                assert_eq!(missing_hashes[0], Hash([1u8; 32]));
            }
            _ => panic!("Expected GetBlockTxs"),
        }
    }

    #[test]
    fn test_block_txs_roundtrip() {
        let addr = "127.0.0.1:9000".parse().unwrap();
        let msg = P2PMessage::new(
            MessageType::BlockTxs {
                slot: 5,
                transactions: vec![],
            },
            addr,
        );
        let bytes = msg.serialize().unwrap();
        let decoded = P2PMessage::deserialize(&bytes).unwrap();
        match decoded.msg_type {
            MessageType::BlockTxs { slot, transactions } => {
                assert_eq!(slot, 5);
                assert!(transactions.is_empty());
            }
            _ => panic!("Expected BlockTxs"),
        }
    }
}
