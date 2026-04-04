//! RPC client for Lichen

use crate::error::{Error, Result};
use crate::types::{Balance, Block, NetworkInfo};
use crate::{Hash, Instruction, Keypair, Pubkey, SYSTEM_PROGRAM_ID, CONTRACT_PROGRAM_ID, ContractInstruction, TransactionBuilder};
use reqwest;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Lichen RPC client
#[derive(Debug, Clone)]
pub struct Client {
    rpc_url: String,
    client: reqwest::Client,
    next_id: Arc<AtomicU64>,
}

impl Client {
    /// Create a new client with default settings
    pub fn new(rpc_url: impl Into<String>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            client: reqwest::Client::new(),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Create a client using the LICHEN_RPC_URL env var, falling back to localhost:8899.
    pub fn from_env() -> Self {
        let url = std::env::var("LICHEN_RPC_URL")
            .unwrap_or_else(|_| "http://localhost:8899".to_string());
        Self::new(url)
    }
    
    /// Create a client builder for custom configuration
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }
    
    /// Make an RPC call
    async fn rpc_call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        
        let response = self.client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await?
            .json::<Value>()
            .await?;
        
        if let Some(error) = response.get("error") {
            return Err(Error::RpcError(error.to_string()));
        }
        
        response.get("result")
            .cloned()
            .ok_or(Error::RpcError("No result in response".to_string()))
    }
    
    /// Get current slot
    pub async fn get_slot(&self) -> Result<u64> {
        let result = self.rpc_call("getSlot", json!([])).await?;
        result.as_u64()
            .ok_or(Error::ParseError("Invalid slot format".to_string()))
    }
    
    /// Get account balance
    pub async fn get_balance(&self, pubkey: &Pubkey) -> Result<Balance> {
        let result = self.rpc_call("getBalance", json!([pubkey.to_base58()])).await?;
        
        let spores = result["spores"]
            .as_u64()
            .ok_or(Error::ParseError("Invalid balance format".to_string()))?;
        
        Ok(Balance::from_spores(spores))
    }
    
    /// Get block by slot
    pub async fn get_block(&self, slot: u64) -> Result<Block> {
        let result = self.rpc_call("getBlock", json!([slot])).await?;
        serde_json::from_value(result)
            .map_err(|e| Error::ParseError(e.to_string()))
    }
    
    /// Get latest block
    pub async fn get_latest_block(&self) -> Result<Block> {
        let result = self.rpc_call("getLatestBlock", json!([])).await?;
        serde_json::from_value(result)
            .map_err(|e| Error::ParseError(e.to_string()))
    }
    
    /// Get network information
    pub async fn get_network_info(&self) -> Result<NetworkInfo> {
        let result = self.rpc_call("getNetworkInfo", json!([])).await?;
        serde_json::from_value(result)
            .map_err(|e| Error::ParseError(e.to_string()))
    }
    
    /// Get validators
    pub async fn get_validators(&self) -> Result<Vec<Value>> {
        let result = self.rpc_call("getValidators", json!([])).await?;
        // Handle both array format and object with "validators" field
        if let Some(arr) = result.as_array() {
            Ok(arr.clone())
        } else if let Some(validators) = result.get("validators").and_then(|v| v.as_array()) {
            Ok(validators.clone())
        } else {
            Err(Error::ParseError("Invalid validators format".to_string()))
        }
    }
    
    /// Send raw transaction (base64-encoded bincode)
    pub async fn send_raw_transaction(&self, tx_base64: &str) -> Result<String> {
        let result = self.rpc_call("sendTransaction", json!([tx_base64])).await?;
        result.as_str()
            .map(|s| s.to_string())
            .ok_or(Error::ParseError("Invalid transaction hash".to_string()))
    }
    
    /// Send transaction (serializes with wire envelope and encodes automatically)
    pub async fn send_transaction(&self, tx: &crate::types::Transaction) -> Result<String> {
        let tx_bytes = tx.to_wire();
        let tx_base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &tx_bytes);
        self.send_raw_transaction(&tx_base64).await
    }
    
    /// Get transaction by signature
    pub async fn get_transaction(&self, signature: &str) -> Result<Value> {
        self.rpc_call("getTransaction", json!([signature])).await
    }
    
    /// Get account info
    pub async fn get_account_info(&self, pubkey: &Pubkey) -> Result<Value> {
        self.rpc_call("getAccountInfo", json!([pubkey.to_base58()])).await
    }
    
    /// Get transaction history for an account
    pub async fn get_transaction_history(&self, pubkey: &Pubkey, limit: Option<u64>) -> Result<Value> {
        let limit = limit.unwrap_or(10);
        self.rpc_call("getTransactionHistory", json!([pubkey.to_base58(), limit])).await
    }
    
    /// Get recent blockhash (for transaction building)
    pub async fn get_recent_blockhash(&self) -> Result<String> {
        let result = self.rpc_call("getRecentBlockhash", json!([])).await?;
        // Handle both string format and object with "blockhash" field
        if let Some(hash_str) = result.as_str() {
            Ok(hash_str.to_string())
        } else if let Some(hash_str) = result.get("blockhash").and_then(|v| v.as_str()) {
            Ok(hash_str.to_string())
        } else {
            Err(Error::ParseError("Invalid blockhash format".to_string()))
        }
    }
    
    // ============================================================================
    // VALIDATOR OPERATIONS
    // ============================================================================
    
    /// Get detailed validator information
    pub async fn get_validator_info(&self, pubkey: &Pubkey) -> Result<Value> {
        self.rpc_call("getValidatorInfo", json!([pubkey.to_base58()])).await
    }
    
    /// Get validator performance metrics
    pub async fn get_validator_performance(&self, pubkey: &Pubkey) -> Result<Value> {
        self.rpc_call("getValidatorPerformance", json!([pubkey.to_base58()])).await
    }
    
    /// Get comprehensive chain status
    pub async fn get_chain_status(&self) -> Result<Value> {
        self.rpc_call("getChainStatus", json!([])).await
    }
    
    // ============================================================================
    // STAKING OPERATIONS
    // ============================================================================
    
    /// Create stake transaction
    pub async fn stake(&self, staker: &Keypair, validator: &Pubkey, amount: u64) -> Result<String> {
        let blockhash_str = self.get_recent_blockhash().await?;
        let blockhash = Hash::from_hex(&blockhash_str)
            .map_err(|e| Error::ParseError(e))?;

        let mut data = vec![9u8];
        data.extend_from_slice(&amount.to_le_bytes());

        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![staker.pubkey(), *validator],
            data,
        };

        let tx = TransactionBuilder::new()
            .add_instruction(instruction)
            .recent_blockhash(blockhash)
            .build_and_sign(staker)?;

        self.send_transaction(&tx).await
    }
    
    /// Create unstake transaction
    pub async fn unstake(&self, staker: &Keypair, validator: &Pubkey, amount: u64) -> Result<String> {
        let blockhash_str = self.get_recent_blockhash().await?;
        let blockhash = Hash::from_hex(&blockhash_str)
            .map_err(|e| Error::ParseError(e))?;

        let mut data = vec![10u8];
        data.extend_from_slice(&amount.to_le_bytes());

        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![staker.pubkey(), *validator],
            data,
        };

        let tx = TransactionBuilder::new()
            .add_instruction(instruction)
            .recent_blockhash(blockhash)
            .build_and_sign(staker)?;

        self.send_transaction(&tx).await
    }
    
    /// Get staking status for an account
    pub async fn get_staking_status(&self, pubkey: &Pubkey) -> Result<Value> {
        self.rpc_call("getStakingStatus", json!([pubkey.to_base58()])).await
    }
    
    /// Get staking rewards for an account
    pub async fn get_staking_rewards(&self, pubkey: &Pubkey) -> Result<Value> {
        self.rpc_call("getStakingRewards", json!([pubkey.to_base58()])).await
    }
    
    // ============================================================================
    // TRANSFER & CONTRACT OPERATIONS
    // ============================================================================

    /// Transfer native LICN (spores) from one account to another.
    pub async fn transfer(&self, from: &Keypair, to: &Pubkey, amount: u64) -> Result<String> {
        let blockhash_str = self.get_recent_blockhash().await?;
        let blockhash = Hash::from_hex(&blockhash_str)
            .map_err(|e| Error::ParseError(e))?;

        let mut data = vec![0u8]; // Transfer instruction type
        data.extend_from_slice(&amount.to_le_bytes());

        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![from.pubkey(), *to],
            data,
        };

        let tx = TransactionBuilder::new()
            .add_instruction(instruction)
            .recent_blockhash(blockhash)
            .build_and_sign(from)?;

        self.send_transaction(&tx).await
    }

    /// Deploy a WASM smart contract.
    ///
    /// # Arguments
    /// * `deployer` - Deployer keypair (signer, pays deploy fee)
    /// * `code` - WASM bytecode (must start with \0asm magic, max 512 KB)
    /// * `init_data` - Optional initialization data passed to contract init
    pub async fn deploy_contract(
        &self,
        deployer: &Keypair,
        code: Vec<u8>,
        init_data: Vec<u8>,
    ) -> Result<String> {
        if code.len() < 4 || &code[..4] != b"\0asm" {
            return Err(Error::BuildError("Invalid WASM bytecode: missing magic header (\\0asm)".into()));
        }
        if code.len() > 512 * 1024 {
            return Err(Error::BuildError("Contract code exceeds 512 KB limit".into()));
        }

        let blockhash_str = self.get_recent_blockhash().await?;
        let blockhash = Hash::from_hex(&blockhash_str)
            .map_err(|e| Error::ParseError(e))?;

        let contract_ix = ContractInstruction::Deploy { code, init_data };
        let data = serde_json::to_vec(&contract_ix)
            .map_err(|e| Error::SerializationError(e.to_string()))?;

        let instruction = Instruction {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: vec![deployer.pubkey()],
            data,
        };

        let tx = TransactionBuilder::new()
            .add_instruction(instruction)
            .recent_blockhash(blockhash)
            .build_and_sign(deployer)?;

        self.send_transaction(&tx).await
    }

    /// Call a function on a deployed WASM smart contract.
    ///
    /// # Arguments
    /// * `caller` - Caller keypair (signer)
    /// * `contract` - Contract account public key
    /// * `function` - Name of the contract function to invoke
    /// * `args` - Serialized function arguments
    /// * `value` - Native LICN to send with the call in spores
    pub async fn call_contract(
        &self,
        caller: &Keypair,
        contract: &Pubkey,
        function: &str,
        args: Vec<u8>,
        value: u64,
    ) -> Result<String> {
        let blockhash_str = self.get_recent_blockhash().await?;
        let blockhash = Hash::from_hex(&blockhash_str)
            .map_err(|e| Error::ParseError(e))?;

        let contract_ix = ContractInstruction::Call {
            function: function.to_string(),
            args,
            value,
        };
        let data = serde_json::to_vec(&contract_ix)
            .map_err(|e| Error::SerializationError(e.to_string()))?;

        let instruction = Instruction {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: vec![caller.pubkey(), *contract],
            data,
        };

        let tx = TransactionBuilder::new()
            .add_instruction(instruction)
            .recent_blockhash(blockhash)
            .build_and_sign(caller)?;

        self.send_transaction(&tx).await
    }

    /// Upgrade a deployed WASM smart contract (owner only).
    pub async fn upgrade_contract(
        &self,
        owner: &Keypair,
        contract: &Pubkey,
        code: Vec<u8>,
    ) -> Result<String> {
        if code.len() < 4 || &code[..4] != b"\0asm" {
            return Err(Error::BuildError("Invalid WASM bytecode: missing magic header (\\0asm)".into()));
        }
        if code.len() > 512 * 1024 {
            return Err(Error::BuildError("Contract code exceeds 512 KB limit".into()));
        }

        let blockhash_str = self.get_recent_blockhash().await?;
        let blockhash = Hash::from_hex(&blockhash_str)
            .map_err(|e| Error::ParseError(e))?;

        let contract_ix = ContractInstruction::Upgrade { code };
        let data = serde_json::to_vec(&contract_ix)
            .map_err(|e| Error::SerializationError(e.to_string()))?;

        let instruction = Instruction {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: vec![owner.pubkey(), *contract],
            data,
        };

        let tx = TransactionBuilder::new()
            .add_instruction(instruction)
            .recent_blockhash(blockhash)
            .build_and_sign(owner)?;

        self.send_transaction(&tx).await
    }
    
    // ============================================================================
    // NETWORK OPERATIONS
    // ============================================================================
    
    /// Get connected peers
    pub async fn get_peers(&self) -> Result<Value> {
        self.rpc_call("getPeers", json!([])).await
    }
    
    /// Get network metrics
    pub async fn get_metrics(&self) -> Result<Value> {
        self.rpc_call("getMetrics", json!([])).await
    }
    
    /// Get total burned tokens
    pub async fn get_total_burned(&self) -> Result<Value> {
        self.rpc_call("getTotalBurned", json!([])).await
    }
    
    // ============================================================================
    // CONTRACT/PROGRAM OPERATIONS
    // ============================================================================
    
    /// Get contract information
    pub async fn get_contract_info(&self, contract_id: &Pubkey) -> Result<Value> {
        self.rpc_call("getContractInfo", json!([contract_id.to_base58()])).await
    }
    
    /// Get contract execution logs
    pub async fn get_contract_logs(&self, contract_id: &Pubkey) -> Result<Value> {
        self.rpc_call("getContractLogs", json!([contract_id.to_base58()])).await
    }

    // ============================================================================
    // PROGRAM OPERATIONS (DRAFT)
    // ============================================================================

    pub async fn get_program(&self, program_id: &Pubkey) -> Result<Value> {
        self.rpc_call("getProgram", json!([program_id.to_base58()])).await
    }

    pub async fn get_program_stats(&self, program_id: &Pubkey) -> Result<Value> {
        self.rpc_call("getProgramStats", json!([program_id.to_base58()])).await
    }

    pub async fn get_programs(&self) -> Result<Value> {
        self.rpc_call("getPrograms", json!([])).await
    }

    pub async fn get_program_calls(&self, program_id: &Pubkey) -> Result<Value> {
        self.rpc_call("getProgramCalls", json!([program_id.to_base58()])).await
    }

    pub async fn get_program_storage(&self, program_id: &Pubkey) -> Result<Value> {
        self.rpc_call("getProgramStorage", json!([program_id.to_base58()])).await
    }

    // ============================================================================
    // NFT OPERATIONS (DRAFT)
    // ============================================================================

    pub async fn get_collection(&self, collection_id: &Pubkey) -> Result<Value> {
        self.rpc_call("getCollection", json!([collection_id.to_base58()])).await
    }

    pub async fn get_nft(&self, collection_id: &Pubkey, token_id: u64) -> Result<Value> {
        self.rpc_call("getNFT", json!([collection_id.to_base58(), token_id])).await
    }

    pub async fn get_nfts_by_owner(&self, owner: &Pubkey) -> Result<Value> {
        self.rpc_call("getNFTsByOwner", json!([owner.to_base58()])).await
    }

    pub async fn get_nfts_by_collection(&self, collection_id: &Pubkey) -> Result<Value> {
        self.rpc_call("getNFTsByCollection", json!([collection_id.to_base58()])).await
    }

    pub async fn get_nft_activity(&self, collection_id: &Pubkey, token_id: u64) -> Result<Value> {
        self.rpc_call("getNFTActivity", json!([collection_id.to_base58(), token_id])).await
    }
    
    /// Get all deployed contracts
    pub async fn get_all_contracts(&self) -> Result<Value> {
        self.rpc_call("getAllContracts", json!([])).await
    }
    
    /// Health check
    pub async fn health(&self) -> Result<bool> {
        let result = self.rpc_call("health", json!([])).await?;
        Ok(result.get("status").and_then(|v| v.as_str()) == Some("ok"))
    }
}

/// Builder for Client with custom configuration
#[derive(Default)]
pub struct ClientBuilder {
    rpc_url: Option<String>,
    timeout: Option<std::time::Duration>,
}

impl ClientBuilder {
    /// Set RPC URL
    pub fn rpc_url(mut self, url: impl Into<String>) -> Self {
        self.rpc_url = Some(url.into());
        self
    }
    
    /// Set request timeout
    pub fn timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
    
    /// Build the client
    pub fn build(self) -> Result<Client> {
        let rpc_url = self.rpc_url
            .ok_or(Error::ConfigError("RPC URL not set".to_string()))?;
        
        let mut client_builder = reqwest::Client::builder();
        
        if let Some(timeout) = self.timeout {
            client_builder = client_builder.timeout(timeout);
        }
        
        Ok(Client {
            rpc_url,
            client: client_builder.build()?,
            next_id: Arc::new(AtomicU64::new(1)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    // ── Client::new ─────────────────────────────────────────────────

    #[test]
    fn test_client_new() {
        let client = Client::new("http://localhost:8899");
        assert_eq!(client.rpc_url, "http://localhost:8899");
    }

    #[test]
    fn test_client_new_custom_url() {
        let client = Client::new("https://rpc.lichen.network:443");
        assert_eq!(client.rpc_url, "https://rpc.lichen.network:443");
    }

    #[test]
    fn test_client_new_id_starts_at_1() {
        let client = Client::new("http://localhost:8899");
        assert_eq!(client.next_id.load(Ordering::Relaxed), 1);
    }

    // ── Client::from_env ────────────────────────────────────────────

    #[test]
    fn test_client_from_env_defaults_to_localhost() {
        let _guard = env_lock().lock().unwrap();
        // Clear the env var to ensure fallback
        std::env::remove_var("LICHEN_RPC_URL");
        let client = Client::from_env();
        assert_eq!(client.rpc_url, "http://localhost:8899");
    }

    #[test]
    fn test_client_from_env_uses_var() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("LICHEN_RPC_URL", "http://custom:9999");
        let client = Client::from_env();
        assert_eq!(client.rpc_url, "http://custom:9999");
        std::env::remove_var("LICHEN_RPC_URL");
    }

    // ── ClientBuilder ───────────────────────────────────────────────

    #[test]
    fn test_client_builder() {
        let client = Client::builder()
            .rpc_url("http://localhost:8899")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("should build client");
        assert_eq!(client.rpc_url, "http://localhost:8899");
    }

    #[test]
    fn test_client_builder_no_url_fails() {
        let result = Client::builder().build();
        assert!(result.is_err(), "should fail without URL");
    }

    #[test]
    fn test_client_builder_no_timeout() {
        let client = Client::builder()
            .rpc_url("http://localhost:8899")
            .build()
            .expect("should build without timeout");
        assert_eq!(client.rpc_url, "http://localhost:8899");
    }

    #[test]
    fn test_client_builder_default() {
        let builder = ClientBuilder::default();
        assert!(builder.rpc_url.is_none());
        assert!(builder.timeout.is_none());
    }

    // ── Request ID counter ──────────────────────────────────────────

    #[test]
    fn test_client_id_increments() {
        let client = Client::new("http://localhost:8899");
        let v1 = client.next_id.fetch_add(1, Ordering::Relaxed);
        let v2 = client.next_id.fetch_add(1, Ordering::Relaxed);
        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
    }

    #[test]
    fn test_client_clone_shares_counter() {
        let client = Client::new("http://localhost:8899");
        client.next_id.fetch_add(1, Ordering::Relaxed);
        let clone = client.clone();
        let v = clone.next_id.fetch_add(1, Ordering::Relaxed);
        assert_eq!(v, 2); // Shared via Arc
    }

    #[test]
    fn test_client_clone_shares_url() {
        let client = Client::new("http://my-rpc:8899");
        let clone = client.clone();
        assert_eq!(clone.rpc_url, "http://my-rpc:8899");
    }
}
