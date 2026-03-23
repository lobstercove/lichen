//! Transaction building and signing

use crate::error::{Error, Result};
use crate::Keypair;
use lichen_core::{Transaction as CoreTransaction, Message, Instruction, Hash};

/// Transaction builder
pub struct TransactionBuilder {
    instructions: Vec<Instruction>,
    recent_blockhash: Option<Hash>,
}

impl TransactionBuilder {
    /// Create a new transaction builder
    pub fn new() -> Self {
        Self {
            instructions: Vec::new(),
            recent_blockhash: None,
        }
    }
    
    /// Add an instruction
    pub fn add_instruction(mut self, instruction: Instruction) -> Self {
        self.instructions.push(instruction);
        self
    }
    
    /// Set recent blockhash
    pub fn recent_blockhash(mut self, blockhash: Hash) -> Self {
        self.recent_blockhash = Some(blockhash);
        self
    }
    
    /// Build and sign the transaction
    pub fn build_and_sign(self, keypair: &Keypair) -> Result<CoreTransaction> {
        let blockhash = self.recent_blockhash
            .ok_or(Error::BuildError("Recent blockhash not set".to_string()))?;
        
        if self.instructions.is_empty() {
            return Err(Error::BuildError("No instructions added".to_string()));
        }
        
        // Create message
        let message = Message::new(self.instructions, blockhash);
        
        // Sign message
        let message_bytes = message.serialize();
        let signature = keypair.sign(&message_bytes);
        
        // Create transaction
        Ok(CoreTransaction {
            signatures: vec![signature],
            message,
                    tx_type: Default::default(),
})
    }
}

impl Default for TransactionBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Keypair;
    use lichen_core::{Instruction, SYSTEM_PROGRAM_ID};

    fn dummy_instruction() -> Instruction {
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![],
            data: vec![0u8],
        }
    }

    fn zero_blockhash() -> Hash {
        Hash([0u8; 32])
    }

    #[test]
    fn builder_new_has_no_instructions() {
        let builder = TransactionBuilder::new();
        assert!(builder.instructions.is_empty());
        assert!(builder.recent_blockhash.is_none());
    }

    #[test]
    fn builder_default_same_as_new() {
        let builder = TransactionBuilder::default();
        assert!(builder.instructions.is_empty());
    }

    #[test]
    fn add_instruction_appends() {
        let builder = TransactionBuilder::new()
            .add_instruction(dummy_instruction())
            .add_instruction(dummy_instruction());
        assert_eq!(builder.instructions.len(), 2);
    }

    #[test]
    fn recent_blockhash_sets_hash() {
        let h = Hash([42u8; 32]);
        let builder = TransactionBuilder::new().recent_blockhash(h);
        assert_eq!(builder.recent_blockhash, Some(h));
    }

    #[test]
    fn build_and_sign_no_blockhash_fails() {
        let kp = Keypair::new();
        let result = TransactionBuilder::new()
            .add_instruction(dummy_instruction())
            .build_and_sign(&kp);
        assert!(result.is_err());
    }

    #[test]
    fn build_and_sign_no_instructions_fails() {
        let kp = Keypair::new();
        let result = TransactionBuilder::new()
            .recent_blockhash(zero_blockhash())
            .build_and_sign(&kp);
        assert!(result.is_err());
    }

    #[test]
    fn build_and_sign_success() {
        let kp = Keypair::new();
        let tx = TransactionBuilder::new()
            .add_instruction(dummy_instruction())
            .recent_blockhash(zero_blockhash())
            .build_and_sign(&kp)
            .expect("should build");
        assert_eq!(tx.signatures.len(), 1);
        assert_eq!(tx.signatures[0].len(), 64);
    }

    #[test]
    fn build_and_sign_deterministic() {
        let kp = Keypair::from_seed(&[7u8; 32]);
        let tx1 = TransactionBuilder::new()
            .add_instruction(dummy_instruction())
            .recent_blockhash(zero_blockhash())
            .build_and_sign(&kp)
            .unwrap();
        let tx2 = TransactionBuilder::new()
            .add_instruction(dummy_instruction())
            .recent_blockhash(zero_blockhash())
            .build_and_sign(&kp)
            .unwrap();
        assert_eq!(tx1.signatures, tx2.signatures);
    }

    #[test]
    fn multiple_instructions_preserved() {
        let kp = Keypair::new();
        let ix1 = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![],
            data: vec![1],
        };
        let ix2 = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![],
            data: vec![2],
        };
        let tx = TransactionBuilder::new()
            .add_instruction(ix1)
            .add_instruction(ix2)
            .recent_blockhash(zero_blockhash())
            .build_and_sign(&kp)
            .unwrap();
        assert_eq!(tx.message.instructions.len(), 2);
    }
}
