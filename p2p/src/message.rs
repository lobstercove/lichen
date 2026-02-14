// P2P Message Types

use moltchain_core::{
    Block, Hash, Pubkey, SlashingEvidence, StakePool, Transaction, ValidatorSet, Vote,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

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

/// P2P message envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2PMessage {
    /// Message type and payload
    pub msg_type: MessageType,
    /// Sender's address
    pub sender: SocketAddr,
    /// Message timestamp
    pub timestamp: u64,
}

/// Snapshot request kinds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SnapshotKind {
    ValidatorSet,
    StakePool,
}

/// Message types in the P2P network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageType {
    /// New block announcement
    Block(Block),

    /// Consensus vote
    Vote(Vote),

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

    /// Slashing evidence broadcast
    SlashingEvidence(SlashingEvidence),
}

/// Peer information message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfoMsg {
    pub address: SocketAddr,
    pub last_seen: u64,
    pub reputation: u64,
    pub validator_pubkey: Option<Pubkey>, // NEW: Link peer to validator
}

impl P2PMessage {
    /// Create new message
    pub fn new(msg_type: MessageType, sender: SocketAddr) -> Self {
        P2PMessage {
            msg_type,
            sender,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }

    /// Serialize message for network transmission
    pub fn serialize(&self) -> Result<Vec<u8>, String> {
        use bincode::Options;
        bincode::options()
            .with_limit(2 * 1024 * 1024)
            .serialize(self)
            .map_err(|e| format!("Serialization error: {}", e))
    }

    /// Deserialize message from bytes (bounded to 2MB to prevent OOM)
    pub fn deserialize(bytes: &[u8]) -> Result<Self, String> {
        use bincode::Options;
        bincode::options()
            .with_limit(2 * 1024 * 1024)
            .deserialize(bytes)
            .map_err(|e| format!("Deserialization error: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_serialization() {
        let addr = "127.0.0.1:8000".parse().unwrap();
        let msg = P2PMessage::new(MessageType::Ping, addr);

        let bytes = msg.serialize().unwrap();
        let deserialized = P2PMessage::deserialize(&bytes).unwrap();

        assert_eq!(msg.sender, deserialized.sender);
        assert_eq!(msg.timestamp, deserialized.timestamp);
    }
}
