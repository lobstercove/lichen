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
pub const MAX_DEPLOY_INSTRUCTION_DATA: usize = 4_194_304; // 4MB — WASM deploys via instruction type 17
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
            // Deploy contract (system program, instruction type 17) allows up to 4MB for WASM code
            let is_deploy = ix.program_id == crate::Pubkey([0u8; 32])
                && !ix.data.is_empty()
                && ix.data[0] == 17;
            let data_limit = if is_deploy {
                MAX_DEPLOY_INSTRUCTION_DATA
            } else {
                MAX_INSTRUCTION_DATA
            };
            if ix.data.len() > data_limit {
                return Err(format!(
                    "Instruction {} data too large: {} bytes (max {})",
                    i,
                    ix.data.len(),
                    data_limit
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

    // ── H16 tests: deploy instruction data limit exemption ──

    #[test]
    fn test_validate_structure_normal_instruction_10kb_limit() {
        let ix = Instruction {
            program_id: Pubkey([1u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data: vec![0u8; MAX_INSTRUCTION_DATA + 1],
        };
        let msg = Message::new(vec![ix], Hash::default());
        let tx = Transaction::new(msg);
        assert!(tx.validate_structure().is_err());
    }

    #[test]
    fn test_validate_structure_deploy_instruction_allows_large_data() {
        // System program (all zeros), instruction type 17 = DeployContract
        let mut data = vec![17u8]; // type byte
        data.extend_from_slice(&(100_000u32).to_le_bytes()); // code_length
        data.extend(vec![0u8; 100_000]); // fake WASM code (100KB > 10KB limit)

        let ix = Instruction {
            program_id: Pubkey([0u8; 32]), // system program
            accounts: vec![Pubkey([2u8; 32]), Pubkey([3u8; 32])],
            data,
        };
        let msg = Message::new(vec![ix], Hash::default());
        let tx = Transaction::new(msg);
        assert!(
            tx.validate_structure().is_ok(),
            "Deploy instruction should allow >10KB data"
        );
    }

    #[test]
    fn test_validate_structure_deploy_instruction_4mb_limit() {
        // Even deploy instructions have a 4MB cap
        let mut data = vec![17u8];
        data.extend(vec![0u8; MAX_DEPLOY_INSTRUCTION_DATA - 1]); // total = limit (type byte + payload)
        let ix = Instruction {
            program_id: Pubkey([0u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data,
        };
        let msg = Message::new(vec![ix], Hash::default());
        let tx = Transaction::new(msg);
        assert!(tx.validate_structure().is_ok());

        // Over limit
        let mut data2 = vec![17u8];
        data2.extend(vec![0u8; MAX_DEPLOY_INSTRUCTION_DATA + 1]);
        let ix2 = Instruction {
            program_id: Pubkey([0u8; 32]),
            accounts: vec![Pubkey([2u8; 32])],
            data: data2,
        };
        let msg2 = Message::new(vec![ix2], Hash::default());
        let tx2 = Transaction::new(msg2);
        assert!(
            tx2.validate_structure().is_err(),
            "Deploy instruction over 4MB should be rejected"
        );
    }
}
