// RPC client for communicating with MoltChain validator

use anyhow::{Context, Result};
use moltchain_core::{Hash, Instruction, Keypair, Message, Pubkey, Transaction, SYSTEM_PROGRAM_ID};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone)]
pub struct RpcClient {
    url: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct RpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct RpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: u64,
    result: Option<serde_json::Value>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: i32,
    message: String,
}

#[derive(Deserialize)]
pub struct BlockInfo {
    pub slot: u64,
    pub hash: String,
    pub parent_hash: String,
    pub state_root: String,
    pub validator: String,
    pub timestamp: u64,
    pub transaction_count: usize,
}

#[derive(Deserialize)]
pub struct BurnedInfo {
    pub shells: u64,
    #[allow(dead_code)]
    pub molt: u64,
}

#[derive(Deserialize)]
pub struct ValidatorsInfo {
    pub validators: Vec<ValidatorInfo>,
    pub _count: usize,
}

#[derive(Deserialize)]
pub struct ValidatorInfo {
    pub pubkey: String,
    pub stake: u64,
    pub reputation: f64,
    pub _normalized_reputation: f64,
    pub _blocks_produced: u64,
    #[allow(dead_code)]
    pub last_vote_slot: u64,
}

#[derive(Deserialize)]
pub struct ChainStatus {
    pub _slot: u64,
    pub _epoch: u64,
    pub _block_height: u64,
    pub _validators: usize,
    pub tps: f64,
    pub total_staked: u64,
    pub block_time_ms: f64,
    pub validator_count: usize,
    pub peer_count: usize,
    pub total_transactions: u64,
    pub total_blocks: u64,
    pub total_supply: u64,
    pub total_burned: u64,
    pub current_slot: u64,
    pub latest_block: u64,
    pub chain_id: String,
    pub network: String,
}

#[derive(Deserialize)]
pub struct Metrics {
    pub tps: f64,
    pub total_blocks: u64,
    pub total_transactions: u64,
    pub total_supply: u64,
    pub circulating_supply: u64,
    pub total_burned: u64,
    pub total_staked: u64,
    pub avg_block_time_ms: f64,
    pub avg_txs_per_block: f64,
    pub total_accounts: u64,
    pub total_contracts: u64,
}

#[derive(Deserialize)]
pub struct PeerInfo {
    pub peer_id: String,
    pub address: String,
    pub connected: bool,
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct NetworkInfo {
    pub network_id: String,
    #[serde(default)]
    pub chain_id: String, // Changed to String to match RPC response
    pub current_slot: u64,
    pub validator_count: usize,
    pub peer_count: usize,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub tps: f64, // Not always present, default to 0
}

#[derive(Deserialize)]
pub struct ValidatorInfoDetailed {
    pub pubkey: String,
    pub stake: u64,
    pub reputation: f64,
    pub blocks_produced: u64,
    pub is_active: bool,
}

#[derive(Deserialize)]
pub struct ValidatorPerformance {
    pub _pubkey: String,
    pub blocks_produced: u64,
    pub blocks_expected: u64,
    pub uptime_percent: f64,
    pub avg_block_time_ms: f64,
}

#[derive(Deserialize)]
pub struct BalanceInfo {
    pub shells: u64,
    pub spendable: u64,
    pub staked: u64,
    pub locked: u64,
}

#[derive(Deserialize)]
pub struct StakingStatus {
    pub address: String,
    pub staked: u64,
    pub is_validator: bool,
}

#[derive(Deserialize)]
pub struct StakingRewards {
    pub address: String,
    pub total_rewards: u64,
    pub pending_rewards: u64,
}

#[derive(Deserialize)]
pub struct AccountInfo {
    pub pubkey: String,
    pub balance: u64,
    pub molt: u64,
    pub exists: bool,
    pub is_executable: bool,
    pub is_validator: bool,
}

#[derive(Deserialize)]
pub struct TransactionInfo {
    pub signature: String,
    pub slot: u64,
    pub from: String,
    pub to: String,
    pub amount: u64,
}

#[derive(Deserialize)]
pub struct ContractInfo {
    pub address: String,
    pub deployer: String,
    pub deployed_at: u64,
    pub code_size: usize,
}

#[derive(Deserialize)]
pub struct ContractLog {
    pub slot: u64,
    pub message: String,
}

#[derive(Deserialize)]
pub struct ContractSummary {
    pub address: String,
    pub deployer: String,
}

impl RpcClient {
    pub fn new(url: &str) -> Self {
        RpcClient {
            url: url.to_string(),
            client: reqwest::Client::new(),
        }
    }

    async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: method.to_string(),
            params,
        };

        let response = self
            .client
            .post(&self.url)
            .json(&request)
            .send()
            .await
            .context("Failed to send RPC request")?;

        let rpc_response: RpcResponse = response
            .json()
            .await
            .context("Failed to parse RPC response")?;

        if let Some(error) = rpc_response.error {
            anyhow::bail!("RPC error {}: {}", error.code, error.message);
        }

        rpc_response
            .result
            .context("Missing result in RPC response")
    }

    /// Get account balance breakdown
    pub async fn get_balance(&self, pubkey: &Pubkey) -> Result<BalanceInfo> {
        let params = json!([pubkey.to_base58()]);
        let result = self.call("getBalance", params).await?;

        let shells = result.get("shells").and_then(|v| v.as_u64()).unwrap_or(0);
        let spendable = result
            .get("spendable")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let staked = result.get("staked").and_then(|v| v.as_u64()).unwrap_or(0);
        let locked = result.get("locked").and_then(|v| v.as_u64()).unwrap_or(0);

        Ok(BalanceInfo {
            shells,
            spendable,
            staked,
            locked,
        })
    }

    /// Get block by slot
    pub async fn get_block(&self, slot: u64) -> Result<BlockInfo> {
        let params = json!([slot]);
        let result = self.call("getBlock", params).await?;

        let block: BlockInfo =
            serde_json::from_value(result).context("Failed to parse block info")?;

        Ok(block)
    }

    /// Get current slot
    pub async fn get_slot(&self) -> Result<u64> {
        let params = json!([]);
        let result = self.call("getSlot", params).await?;

        result.as_u64().context("Invalid slot response")
    }

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

        Hash::from_hex(hash_str).map_err(|e| anyhow::anyhow!(e))
    }

    /// AUDIT-FIX I-1: Request airdrop from the faucet via requestAirdrop RPC
    pub async fn request_airdrop(&self, to: &Pubkey, amount_molt: f64) -> Result<String> {
        // The RPC accepts whole MOLT amounts as u64
        let amount_u64 = amount_molt.ceil() as u64;
        let params = json!([to.to_base58(), amount_u64]);
        let result = self.call("requestAirdrop", params).await?;
        let sig = result
            .as_str()
            .or_else(|| result.get("signature").and_then(|v| v.as_str()))
            .unwrap_or("ok");
        Ok(sig.to_string())
    }

    /// Transfer shells from one account to another
    pub async fn transfer(&self, from: &Keypair, to: &Pubkey, shells: u64) -> Result<String> {
        let recent_blockhash = self.get_recent_blockhash().await?;

        // Create transfer instruction
        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![from.pubkey(), *to],
            data: {
                let mut data = vec![0u8]; // Opcode 0 = transfer
                data.extend_from_slice(&shells.to_le_bytes());
                data
            },
        };

        // Create message
        let message = Message {
            instructions: vec![instruction],
            recent_blockhash,
        };

        // Sign transaction
        let signature = from.sign(&message.serialize());

        // Create transaction
        let transaction = Transaction {
            signatures: vec![signature],
            message,
        };

        // Serialize transaction
        let tx_bytes =
            bincode::serialize(&transaction).context("Failed to serialize transaction")?;
        let tx_base64 = base64_encode(&tx_bytes);

        // Send transaction via RPC
        let params = json!([tx_base64]);
        let result = self.call("sendTransaction", params).await?;

        let signature_hex = result
            .as_str()
            .context("Invalid transaction response")?
            .to_string();

        Ok(signature_hex)
    }

    /// Deploy a smart contract
    pub async fn deploy_contract(
        &self,
        deployer: &Keypair,
        wasm_code: Vec<u8>,
        contract_address: &Pubkey,
        init_data: Vec<u8>,
    ) -> Result<String> {
        use moltchain_core::ContractInstruction;

        let recent_blockhash = self.get_recent_blockhash().await?;

        // Create deploy instruction
        let contract_ix = ContractInstruction::Deploy {
            code: wasm_code,
            init_data,
        };

        let instruction = Instruction {
            program_id: Pubkey::new([0xFFu8; 32]), // Contract program
            accounts: vec![deployer.pubkey(), *contract_address],
            data: contract_ix
                .serialize()
                .map_err(|e| anyhow::anyhow!("Serialization error: {}", e))?,
        };

        let message = Message {
            instructions: vec![instruction],
            recent_blockhash,
        };

        let signature = deployer.sign(&message.serialize());

        let transaction = Transaction {
            signatures: vec![signature],
            message,
        };

        let tx_bytes = bincode::serialize(&transaction)?;
        let tx_base64 = base64_encode(&tx_bytes);

        let params = json!([tx_base64]);
        let result = self.call("sendTransaction", params).await?;

        let signature_hex = result
            .as_str()
            .context("Invalid transaction response")?
            .to_string();

        Ok(signature_hex)
    }

    /// Upgrade a deployed smart contract (owner only)
    pub async fn upgrade_contract(
        &self,
        owner: &Keypair,
        wasm_code: Vec<u8>,
        contract_address: &Pubkey,
    ) -> Result<String> {
        use moltchain_core::ContractInstruction;

        let recent_blockhash = self.get_recent_blockhash().await?;

        let contract_ix = ContractInstruction::Upgrade { code: wasm_code };

        let instruction = Instruction {
            program_id: Pubkey::new([0xFFu8; 32]), // Contract program
            accounts: vec![owner.pubkey(), *contract_address],
            data: contract_ix
                .serialize()
                .map_err(|e| anyhow::anyhow!("Serialization error: {}", e))?,
        };

        let message = Message {
            instructions: vec![instruction],
            recent_blockhash,
        };

        let signature = owner.sign(&message.serialize());

        let transaction = Transaction {
            signatures: vec![signature],
            message,
        };

        let tx_bytes = bincode::serialize(&transaction)?;
        let tx_base64 = base64_encode(&tx_bytes);

        let params = json!([tx_base64]);
        let result = self.call("sendTransaction", params).await?;

        let signature_hex = result
            .as_str()
            .context("Invalid transaction response")?
            .to_string();

        Ok(signature_hex)
    }

    /// Register a deployed contract in the symbol registry (native instruction type 20)
    pub async fn register_symbol(
        &self,
        owner: &Keypair,
        contract_address: &Pubkey,
        symbol: &str,
        name: Option<&str>,
        template: Option<&str>,
        decimals: Option<u8>,
    ) -> Result<String> {
        let recent_blockhash = self.get_recent_blockhash().await?;

        // Build JSON payload for native type 20 instruction
        let mut payload = serde_json::Map::new();
        payload.insert("symbol".to_string(), serde_json::json!(symbol));
        if let Some(n) = name {
            payload.insert("name".to_string(), serde_json::json!(n));
        }
        if let Some(t) = template {
            payload.insert("template".to_string(), serde_json::json!(t));
        }
        if let Some(d) = decimals {
            payload.insert("decimals".to_string(), serde_json::json!(d));
        }
        let json_bytes = serde_json::to_vec(&payload)?;

        // Native type 20 = register_symbol: [0x14, json...]
        let mut data = vec![20u8];
        data.extend_from_slice(&json_bytes);

        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![owner.pubkey(), *contract_address],
            data,
        };

        let message = Message {
            instructions: vec![instruction],
            recent_blockhash,
        };

        let signature = owner.sign(&message.serialize());

        let transaction = Transaction {
            signatures: vec![signature],
            message,
        };

        let tx_bytes = bincode::serialize(&transaction)?;
        let tx_base64 = base64_encode(&tx_bytes);

        let params = json!([tx_base64]);
        let result = self.call("sendTransaction", params).await?;

        let signature_hex = result
            .as_str()
            .context("Invalid transaction response")?
            .to_string();

        Ok(signature_hex)
    }

    /// Call a smart contract function
    pub async fn call_contract(
        &self,
        caller: &Keypair,
        contract_address: &Pubkey,
        function: String,
        args: Vec<u8>,
        value: u64,
    ) -> Result<String> {
        use moltchain_core::ContractInstruction;

        let recent_blockhash = self.get_recent_blockhash().await?;

        let contract_ix = ContractInstruction::Call {
            function,
            args,
            value,
        };

        let instruction = Instruction {
            program_id: Pubkey::new([0xFFu8; 32]), // Contract program
            accounts: vec![caller.pubkey(), *contract_address],
            data: contract_ix
                .serialize()
                .map_err(|e| anyhow::anyhow!("Serialization error: {}", e))?,
        };

        let message = Message {
            instructions: vec![instruction],
            recent_blockhash,
        };

        let signature = caller.sign(&message.serialize());

        let transaction = Transaction {
            signatures: vec![signature],
            message,
        };

        let tx_bytes = bincode::serialize(&transaction)?;
        let tx_base64 = base64_encode(&tx_bytes);

        let params = json!([tx_base64]);
        let result = self.call("sendTransaction", params).await?;

        let signature_hex = result
            .as_str()
            .context("Invalid transaction response")?
            .to_string();

        Ok(signature_hex)
    }

    /// Get latest block
    pub async fn get_latest_block(&self) -> Result<BlockInfo> {
        let params = json!([]);
        let result = self.call("getLatestBlock", params).await?;

        let block: BlockInfo =
            serde_json::from_value(result).context("Failed to parse block info")?;

        Ok(block)
    }

    /// Get total burned MOLT
    pub async fn get_total_burned(&self) -> Result<BurnedInfo> {
        let params = json!([]);
        let result = self.call("getTotalBurned", params).await?;

        let burned: BurnedInfo =
            serde_json::from_value(result).context("Failed to parse burned info")?;

        Ok(burned)
    }

    /// Get all validators
    pub async fn get_validators(&self) -> Result<ValidatorsInfo> {
        let params = json!([]);
        let result = self.call("getValidators", params).await?;

        let validators: ValidatorsInfo =
            serde_json::from_value(result).context("Failed to parse validators info")?;

        Ok(validators)
    }

    /// Get comprehensive chain status
    pub async fn get_chain_status(&self) -> Result<ChainStatus> {
        let params = json!([]);
        let result = self.call("getChainStatus", params).await?;

        let status: ChainStatus =
            serde_json::from_value(result).context("Failed to parse chain status")?;

        Ok(status)
    }

    /// Get performance metrics
    pub async fn get_metrics(&self) -> Result<Metrics> {
        let params = json!([]);
        let result = self.call("getMetrics", params).await?;

        let metrics: Metrics = serde_json::from_value(result).context("Failed to parse metrics")?;

        Ok(metrics)
    }

    /// Get connected peers
    pub async fn get_peers(&self) -> Result<Vec<PeerInfo>> {
        let params = json!([]);
        let result = self.call("getPeers", params).await?;
        let peers_value = if let Some(arr) = result.as_array() {
            serde_json::Value::Array(arr.clone())
        } else {
            result
                .get("peers")
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Array(vec![]))
        };
        let peers: Vec<PeerInfo> =
            serde_json::from_value(peers_value).context("Failed to parse peers info")?;

        Ok(peers)
    }

    /// Get network information
    pub async fn get_network_info(&self) -> Result<NetworkInfo> {
        let params = json!([]);
        let result = self.call("getNetworkInfo", params).await?;

        let info: NetworkInfo =
            serde_json::from_value(result).context("Failed to parse network info")?;

        Ok(info)
    }

    /// Get detailed validator information
    pub async fn get_validator_info(&self, pubkey: &str) -> Result<ValidatorInfoDetailed> {
        let params = json!([pubkey]);
        let result = self.call("getValidatorInfo", params).await?;

        let info: ValidatorInfoDetailed =
            serde_json::from_value(result).context("Failed to parse validator info")?;

        Ok(info)
    }

    /// Get validator performance metrics
    pub async fn get_validator_performance(&self, pubkey: &str) -> Result<ValidatorPerformance> {
        let params = json!([pubkey]);
        let result = self.call("getValidatorPerformance", params).await?;

        let perf: ValidatorPerformance =
            serde_json::from_value(result).context("Failed to parse validator performance")?;

        Ok(perf)
    }

    /// Stake MOLT tokens
    pub async fn stake(&self, keypair: &Keypair, amount: u64) -> Result<String> {
        let recent_blockhash = self.get_recent_blockhash().await?;

        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![keypair.pubkey(), keypair.pubkey()],
            data: {
                let mut data = vec![9u8]; // Opcode 9 = stake
                data.extend_from_slice(&amount.to_le_bytes());
                data
            },
        };

        let message = Message {
            instructions: vec![instruction],
            recent_blockhash,
        };

        let signature = keypair.sign(&message.serialize());

        let transaction = Transaction {
            signatures: vec![signature],
            message,
        };

        let tx_bytes =
            bincode::serialize(&transaction).context("Failed to serialize transaction")?;
        let tx_base64 = base64_encode(&tx_bytes);

        let params = json!([tx_base64]);
        let result = self.call("sendTransaction", params).await?;

        let signature_hex = result
            .as_str()
            .context("Invalid transaction response")?
            .to_string();

        Ok(signature_hex)
    }

    /// Unstake MOLT tokens
    pub async fn unstake(&self, keypair: &Keypair, amount: u64) -> Result<String> {
        let recent_blockhash = self.get_recent_blockhash().await?;

        let instruction = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![keypair.pubkey(), keypair.pubkey()],
            data: {
                let mut data = vec![10u8]; // Opcode 10 = unstake request
                data.extend_from_slice(&amount.to_le_bytes());
                data
            },
        };

        let message = Message {
            instructions: vec![instruction],
            recent_blockhash,
        };

        let signature = keypair.sign(&message.serialize());

        let transaction = Transaction {
            signatures: vec![signature],
            message,
        };

        let tx_bytes =
            bincode::serialize(&transaction).context("Failed to serialize transaction")?;
        let tx_base64 = base64_encode(&tx_bytes);

        let params = json!([tx_base64]);
        let result = self.call("sendTransaction", params).await?;

        let signature_hex = result
            .as_str()
            .context("Invalid transaction response")?
            .to_string();

        Ok(signature_hex)
    }

    /// Get staking status
    pub async fn get_staking_status(&self, address: &str) -> Result<StakingStatus> {
        let params = json!([address]);
        let result = self.call("getStakingStatus", params).await?;

        let status: StakingStatus =
            serde_json::from_value(result).context("Failed to parse staking status")?;

        Ok(status)
    }

    /// Get staking rewards
    pub async fn get_staking_rewards(&self, address: &str) -> Result<StakingRewards> {
        let params = json!([address]);
        let result = self.call("getStakingRewards", params).await?;

        let rewards: StakingRewards =
            serde_json::from_value(result).context("Failed to parse staking rewards")?;

        Ok(rewards)
    }

    /// Get account information
    pub async fn get_account_info(&self, address: &str) -> Result<AccountInfo> {
        let params = json!([address]);
        let result = self.call("getAccountInfo", params).await?;

        let info: AccountInfo =
            serde_json::from_value(result).context("Failed to parse account info")?;

        Ok(info)
    }

    /// Get transaction history
    pub async fn get_transaction_history(
        &self,
        address: &str,
        limit: usize,
    ) -> Result<Vec<TransactionInfo>> {
        let params = json!([address, limit]);
        let result = self.call("getTransactionHistory", params).await?;

        let history: Vec<TransactionInfo> =
            serde_json::from_value(result).context("Failed to parse transaction history")?;

        Ok(history)
    }

    /// Get contract information
    pub async fn get_contract_info(&self, address: &str) -> Result<ContractInfo> {
        let params = json!([address]);
        let result = self.call("getContractInfo", params).await?;

        let info: ContractInfo =
            serde_json::from_value(result).context("Failed to parse contract info")?;

        Ok(info)
    }

    /// Get contract logs
    pub async fn get_contract_logs(&self, address: &str, limit: usize) -> Result<Vec<ContractLog>> {
        let params = json!([address, limit]);
        let result = self.call("getContractLogs", params).await?;

        let logs: Vec<ContractLog> =
            serde_json::from_value(result).context("Failed to parse contract logs")?;

        Ok(logs)
    }

    /// Get all deployed contracts
    pub async fn get_all_contracts(&self) -> Result<Vec<ContractSummary>> {
        let params = json!([]);
        let result = self.call("getAllContracts", params).await?;

        let contracts: Vec<ContractSummary> =
            serde_json::from_value(result).context("Failed to parse contracts list")?;

        Ok(contracts)
    }

    /// Resolve a symbol (e.g., "DAO", "MOLT", "DEX") to its on-chain contract address
    /// via the symbol registry. Returns None if the symbol is not registered.
    pub async fn resolve_symbol(&self, symbol: &str) -> Result<Option<Pubkey>> {
        let params = json!([symbol]);
        let result = self.call("getSymbolRegistry", params).await;
        match result {
            Ok(val) => {
                if let Some(program) = val.get("program").and_then(|v| v.as_str()) {
                    let bytes = hex::decode(program)
                        .map_err(|e| anyhow::anyhow!("Invalid hex in symbol registry: {}", e))?;
                    if bytes.len() == 32 {
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&bytes);
                        Ok(Some(Pubkey::new(arr)))
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
            Err(_) => Ok(None),
        }
    }
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}
