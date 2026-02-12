//! Transaction building and signing

use crate::error::{Error, Result};
use crate::Keypair;
use moltchain_core::{Transaction as CoreTransaction, Message, Instruction, Hash};

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
        })
    }
}

impl Default for TransactionBuilder {
    fn default() -> Self {
        Self::new()
    }
}
