use anyhow::Result;
use lichen_core::{Instruction, Keypair, Pubkey, SYSTEM_PROGRAM_ID};

use crate::client::{RpcClient, SymbolRegistration};
use crate::client_tx_support::submit_signed_instruction;

impl RpcClient {
    /// AUDIT-FIX I-1: Request airdrop from the faucet via requestAirdrop RPC
    pub async fn request_airdrop(&self, to: &Pubkey, amount_licn: f64) -> Result<String> {
        let amount_u64 = amount_licn.ceil() as u64;
        let params = serde_json::json!([to.to_base58(), amount_u64]);
        let result = self.call("requestAirdrop", params).await?;
        let sig = result
            .as_str()
            .or_else(|| result.get("signature").and_then(|value| value.as_str()))
            .unwrap_or("ok");
        Ok(sig.to_string())
    }

    /// Transfer spores from one account to another
    pub async fn transfer(&self, from: &Keypair, to: &Pubkey, spores: u64) -> Result<String> {
        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![from.pubkey(), *to],
            data: {
                let mut data = vec![0u8];
                data.extend_from_slice(&spores.to_le_bytes());
                data
            },
        };

        submit_signed_instruction(self, from, instruction).await
    }

    /// Register a deployed contract in the symbol registry (native instruction type 20)
    pub async fn register_symbol(
        &self,
        owner: &Keypair,
        contract_address: &Pubkey,
        registration: SymbolRegistration<'_>,
    ) -> Result<String> {
        let mut payload = serde_json::Map::new();
        payload.insert("symbol".to_string(), serde_json::json!(registration.symbol));
        if let Some(name) = registration.name {
            payload.insert("name".to_string(), serde_json::json!(name));
        }
        if let Some(template) = registration.template {
            payload.insert("template".to_string(), serde_json::json!(template));
        }
        if let Some(decimals) = registration.decimals {
            payload.insert("decimals".to_string(), serde_json::json!(decimals));
        }
        if let Some(metadata) = registration.metadata {
            payload.insert("metadata".to_string(), metadata);
        }
        let json_bytes = serde_json::to_vec(&payload)?;

        let mut data = vec![20u8];
        data.extend_from_slice(&json_bytes);

        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![owner.pubkey(), *contract_address],
            data,
        };

        submit_signed_instruction(self, owner, instruction).await
    }

    /// Stake LICN tokens
    pub async fn stake(&self, keypair: &Keypair, amount: u64) -> Result<String> {
        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![keypair.pubkey(), keypair.pubkey()],
            data: {
                let mut data = vec![9u8];
                data.extend_from_slice(&amount.to_le_bytes());
                data
            },
        };

        submit_signed_instruction(self, keypair, instruction).await
    }

    /// Unstake LICN tokens
    pub async fn unstake(&self, keypair: &Keypair, amount: u64) -> Result<String> {
        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![keypair.pubkey(), keypair.pubkey()],
            data: {
                let mut data = vec![10u8];
                data.extend_from_slice(&amount.to_le_bytes());
                data
            },
        };

        submit_signed_instruction(self, keypair, instruction).await
    }
}
