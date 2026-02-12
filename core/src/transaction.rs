// MoltChain Core - Transaction Model

use crate::account::Pubkey;
use crate::hash::Hash;
use serde::{Deserialize, Serialize};

/// Single instruction in a transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instruction {
    /// Program to invoke
    pub program_id: Pubkey,

    /// Accounts involved
    pub accounts: Vec<Pubkey>,

    /// Instruction data
    pub data: Vec<u8>,
}

/// Transaction message (before signing)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Instructions to execute
    pub instructions: Vec<Instruction>,

    /// Recent blockhash (for replay protection)
    pub recent_blockhash: Hash,
}

impl Message {
    pub fn new(instructions: Vec<Instruction>, recent_blockhash: Hash) -> Self {
        Message {
            instructions,
            recent_blockhash,
        }
    }

    /// Serialize for signing
    pub fn serialize(&self) -> Vec<u8> {
        bincode::serialize(self).expect("Message serialization failed — data corruption")
    }

    /// Hash for signing
    pub fn hash(&self) -> Hash {
        Hash::hash(&self.serialize())
    }
}

/// Signed transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Transaction signatures (as hex strings for serde compatibility)
    #[serde(
        serialize_with = "serialize_signatures",
        deserialize_with = "deserialize_signatures"
    )]
    pub signatures: Vec<[u8; 64]>,

    /// Transaction message
    pub message: Message,
}

// Helper functions for signature serialization
fn serialize_signatures<S>(sigs: &[[u8; 64]], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::Serialize;
    let hex_sigs: Vec<String> = sigs.iter().map(hex::encode).collect();
    hex_sigs.serialize(serializer)
}

fn deserialize_signatures<'de, D>(deserializer: D) -> Result<Vec<[u8; 64]>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let hex_sigs: Vec<String> = Vec::deserialize(deserializer)?;
    hex_sigs
        .iter()
        .map(|s| {
            let bytes = hex::decode(s).map_err(serde::de::Error::custom)?;
            if bytes.len() != 64 {
                return Err(serde::de::Error::custom("Invalid signature length"));
            }
            let mut sig = [0u8; 64];
            sig.copy_from_slice(&bytes);
            Ok(sig)
        })
        .collect()
}

/// Maximum instructions per transaction (T1.7)
pub const MAX_INSTRUCTIONS_PER_TX: usize = 64;
/// Maximum data bytes per instruction (T1.7)
pub const MAX_INSTRUCTION_DATA: usize = 10_240; // 10KB
/// Maximum accounts per instruction
pub const MAX_ACCOUNTS_PER_IX: usize = 64;

impl Transaction {
    pub fn new(message: Message) -> Self {
        Transaction {
            signatures: Vec::new(),
            message,
        }
    }

    /// Get transaction signature (first signature's identifier)
    pub fn signature(&self) -> Hash {
        self.hash()
    }

    /// Get transaction hash (includes signatures for unique deduplication).
    /// T3.4 fix: Hash now covers both message AND signatures,
    /// so two transactions with the same message but different signatures
    /// produce different hashes.
    pub fn hash(&self) -> Hash {
        let mut data = self.message.serialize();
        for sig in &self.signatures {
            data.extend_from_slice(sig);
        }
        Hash::hash(&data)
    }

    /// Validate transaction structure (size limits, T1.7)
    pub fn validate_structure(&self) -> Result<(), String> {
        if self.message.instructions.is_empty() {
            return Err("No instructions".to_string());
        }
        if self.message.instructions.len() > MAX_INSTRUCTIONS_PER_TX {
            return Err(format!(
                "Too many instructions: {} (max {})",
                self.message.instructions.len(),
                MAX_INSTRUCTIONS_PER_TX
            ));
        }
        for (i, ix) in self.message.instructions.iter().enumerate() {
            if ix.data.len() > MAX_INSTRUCTION_DATA {
                return Err(format!(
                    "Instruction {} data too large: {} bytes (max {})",
                    i,
                    ix.data.len(),
                    MAX_INSTRUCTION_DATA
                ));
            }
            if ix.accounts.len() > MAX_ACCOUNTS_PER_IX {
                return Err(format!(
                    "Instruction {} has too many accounts: {} (max {})",
                    i,
                    ix.accounts.len(),
                    MAX_ACCOUNTS_PER_IX
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_creation() {
        let program_id = Pubkey([1u8; 32]);
        let accounts = vec![Pubkey([2u8; 32]), Pubkey([3u8; 32])];

        let instruction = Instruction {
            program_id,
            accounts,
            data: vec![0, 1, 2, 3],
        };

        let message = Message::new(vec![instruction], Hash::hash(b"recent_block"));

        let tx = Transaction::new(message);

        println!("Transaction signature: {}", tx.signature());
        assert_eq!(tx.signatures.len(), 0); // Not signed yet
    }
}
