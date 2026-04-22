use anyhow::{anyhow, Context, Result};
use lichen_core::{ContractInstruction, Hash, Instruction, Keypair, Message, Transaction};
use serde_json::json;

use crate::client::RpcClient;

impl RpcClient {
    /// Get recent blockhash for transaction building
    pub async fn get_recent_blockhash(&self) -> Result<Hash> {
        let params = json!([]);
        let result = self.call("getRecentBlockhash", params).await?;

        let hash_str = if let Some(hash) = result.as_str() {
            hash
        } else {
            result
                .get("blockhash")
                .and_then(|value| value.as_str())
                .context("Invalid blockhash response")?
        };

        Hash::from_hex(hash_str).map_err(|error| anyhow::anyhow!(error))
    }
}

pub(crate) fn serialize_contract_instruction(instruction: ContractInstruction) -> Result<Vec<u8>> {
    instruction
        .serialize()
        .map_err(|error| anyhow!("Serialization error: {}", error))
}

pub(crate) async fn submit_signed_instruction(
    client: &RpcClient,
    signer: &Keypair,
    instruction: Instruction,
) -> Result<String> {
    let message = Message {
        instructions: vec![instruction],
        recent_blockhash: client.get_recent_blockhash().await?,
        compute_budget: None,
        compute_unit_price: None,
    };

    let signature = signer.sign(&message.serialize());

    let transaction = Transaction {
        signatures: vec![signature],
        message,
        tx_type: Default::default(),
    };

    client.submit_wire_transaction(transaction.to_wire()).await
}
