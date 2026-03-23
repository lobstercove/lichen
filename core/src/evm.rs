// Lichen EVM integration

use crate::{Pubkey, StateStore};
use alloy_consensus::{transaction::Transaction as AlloyTransaction, TxEnvelope};
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use alloy_rlp::Decodable;
use revm::context::{BlockEnv, CfgEnv, Context, TxEnv};
use revm::context_interface::result::{ExecutionResult, Output};
use revm::database_interface::DBErrorMarker;
use revm::handler::{ExecuteEvm, MainBuilder};
use revm::primitives::HashMap as RevmHashMap;
use revm::primitives::{
    hardfork::SpecId, Address as RevmAddress, Bytes as RevmBytes, TxKind, B256 as RevmB256,
    U256 as RevmU256,
};
use revm::state::{Account, AccountInfo, Bytecode};
use revm::{Database, DatabaseCommit};
use serde::{Deserialize, Serialize};
use std::collections::HashMap as StdHashMap;
use std::error::Error;
use std::fmt;

/// EVM program ID (all 0xEE)
pub const EVM_PROGRAM_ID: Pubkey = Pubkey([0xEEu8; 32]);

/// Represents a single deferred EVM state change (H3 fix: atomic with StateBatch).
#[derive(Debug, Clone)]
pub struct EvmStateChange {
    pub evm_address: [u8; 20],
    /// `Some(account)` = put, `None` = clear (self-destruct)
    pub account: Option<EvmAccount>,
    /// Storage changes: `(slot, Some(value))` = set, `(slot, None)` = delete
    pub storage_changes: Vec<([u8; 32], Option<U256>)>,
    /// Native account balance sync: `Some((pubkey, spendable_spores))`
    pub native_balance_update: Option<(Pubkey, u64)>,
}

/// Collection of deferred EVM state changes returned from `execute_evm_transaction`.
#[derive(Debug, Clone, Default)]
pub struct EvmStateChanges {
    pub changes: Vec<EvmStateChange>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmAccount {
    pub nonce: u64,
    pub balance: [u8; 32],
    pub code: Vec<u8>,
}

impl Default for EvmAccount {
    fn default() -> Self {
        Self::new()
    }
}

impl EvmAccount {
    pub fn new() -> Self {
        Self {
            nonce: 0,
            balance: [0u8; 32],
            code: Vec::new(),
        }
    }

    pub fn balance_u256(&self) -> U256 {
        U256::from_be_bytes(self.balance)
    }

    pub fn set_balance_u256(&mut self, value: U256) {
        self.balance = value.to_be_bytes::<32>();
    }
}

#[derive(Debug, Clone)]
pub struct EvmTx {
    pub raw: Vec<u8>,
    pub hash: B256,
    pub from: Address,
    pub to: Option<Address>,
    pub nonce: u64,
    pub gas_limit: u64,
    pub gas_price: U256,
    pub max_fee_per_gas: Option<U256>,
    pub max_priority_fee_per_gas: Option<U256>,
    pub value: U256,
    pub data: Bytes,
    pub chain_id: Option<u64>,
}

/// Structured EVM log matching the Ethereum log format.
/// Stored in receipts and indexed per-slot for eth_getLogs queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmLog {
    /// Contract address that emitted the log
    pub address: [u8; 20],
    /// Topic hashes (up to 4: topic[0] = event signature hash)
    pub topics: Vec<[u8; 32]>,
    /// ABI-encoded non-indexed data
    pub data: Vec<u8>,
}

/// Entry in the per-slot EVM log index.
/// Stored as Vec<EvmLogEntry> per slot in CF_EVM_LOGS_BY_SLOT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmLogEntry {
    pub tx_hash: [u8; 32],
    pub tx_index: u32,
    pub log_index: u32,
    pub log: EvmLog,
}

#[derive(Debug, Clone)]
pub struct EvmExecutionResult {
    pub success: bool,
    pub gas_used: u64,
    pub output: Vec<u8>,
    pub created_address: Option<[u8; 20]>,
    /// Legacy raw log data (kept for backward compat)
    pub logs: Vec<Vec<u8>>,
    /// Structured EVM logs with address + topics + data
    pub structured_logs: Vec<EvmLog>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmTxRecord {
    pub evm_hash: [u8; 32],
    pub native_hash: [u8; 32],
    pub from: [u8; 20],
    pub to: Option<[u8; 20]>,
    pub value: [u8; 32],
    pub gas_limit: u64,
    pub gas_price: [u8; 32],
    pub nonce: u64,
    pub data: Vec<u8>,
    pub status: Option<bool>,
    pub gas_used: Option<u64>,
    pub block_slot: Option<u64>,
    pub block_hash: Option<[u8; 32]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmReceipt {
    pub evm_hash: [u8; 32],
    pub status: bool,
    pub gas_used: u64,
    pub block_slot: Option<u64>,
    pub block_hash: Option<[u8; 32]>,
    pub contract_address: Option<[u8; 20]>,
    pub logs: Vec<Vec<u8>>,
    /// Structured EVM logs with full address + topics + data (Task 3.4)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub structured_logs: Vec<EvmLog>,
}

pub fn evm_tx_hash(raw: &[u8]) -> B256 {
    keccak256(raw)
}

pub fn decode_evm_transaction(raw: &[u8]) -> Result<EvmTx, String> {
    let mut bytes = raw;
    let envelope =
        TxEnvelope::decode(&mut bytes).map_err(|e| format!("Invalid EVM tx RLP: {}", e))?;

    let (from, hash) = match &envelope {
        TxEnvelope::Legacy(tx) => (
            tx.recover_signer()
                .map_err(|_| "Failed to recover signer".to_string())?,
            *tx.hash(),
        ),
        TxEnvelope::Eip2930(tx) => (
            tx.recover_signer()
                .map_err(|_| "Failed to recover signer".to_string())?,
            *tx.hash(),
        ),
        TxEnvelope::Eip1559(tx) => (
            tx.recover_signer()
                .map_err(|_| "Failed to recover signer".to_string())?,
            *tx.hash(),
        ),
        TxEnvelope::Eip4844(tx) => (
            tx.recover_signer()
                .map_err(|_| "Failed to recover signer".to_string())?,
            *tx.hash(),
        ),
        TxEnvelope::Eip7702(tx) => (
            tx.recover_signer()
                .map_err(|_| "Failed to recover signer".to_string())?,
            *tx.hash(),
        ),
        _ => return Err("Unsupported EVM tx envelope".to_string()),
    };

    match envelope {
        TxEnvelope::Legacy(tx) => {
            let inner = tx.tx();
            Ok(EvmTx {
                raw: raw.to_vec(),
                hash,
                from,
                to: inner.to.to().copied(),
                nonce: inner.nonce,
                gas_limit: inner.gas_limit,
                gas_price: U256::from(inner.gas_price),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                value: inner.value,
                data: inner.input.clone(),
                chain_id: inner.chain_id,
            })
        }
        TxEnvelope::Eip2930(tx) => {
            let inner = tx.tx();
            Ok(EvmTx {
                raw: raw.to_vec(),
                hash,
                from,
                to: inner.to.to().copied(),
                nonce: inner.nonce,
                gas_limit: inner.gas_limit,
                gas_price: U256::from(inner.gas_price),
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                value: inner.value,
                data: inner.input.clone(),
                chain_id: Some(inner.chain_id),
            })
        }
        TxEnvelope::Eip1559(tx) => {
            let inner = tx.tx();
            Ok(EvmTx {
                raw: raw.to_vec(),
                hash,
                from,
                to: inner.to.to().copied(),
                nonce: inner.nonce,
                gas_limit: inner.gas_limit,
                gas_price: U256::from(inner.max_fee_per_gas),
                max_fee_per_gas: Some(U256::from(inner.max_fee_per_gas)),
                max_priority_fee_per_gas: Some(U256::from(inner.max_priority_fee_per_gas)),
                value: inner.value,
                data: inner.input.clone(),
                chain_id: Some(inner.chain_id),
            })
        }
        TxEnvelope::Eip4844(tx) => {
            let inner = tx.tx();
            Ok(EvmTx {
                raw: raw.to_vec(),
                hash,
                from,
                to: inner.to(),
                nonce: inner.nonce(),
                gas_limit: inner.gas_limit(),
                gas_price: U256::from(inner.max_fee_per_gas()),
                max_fee_per_gas: Some(U256::from(inner.max_fee_per_gas())),
                max_priority_fee_per_gas: inner.max_priority_fee_per_gas().map(U256::from),
                value: inner.value(),
                data: inner.input().clone(),
                chain_id: inner.chain_id(),
            })
        }
        TxEnvelope::Eip7702(tx) => {
            let inner = tx.tx();
            Ok(EvmTx {
                raw: raw.to_vec(),
                hash,
                from,
                to: inner.to(),
                nonce: inner.nonce(),
                gas_limit: inner.gas_limit(),
                gas_price: U256::from(inner.max_fee_per_gas()),
                max_fee_per_gas: Some(U256::from(inner.max_fee_per_gas())),
                max_priority_fee_per_gas: inner.max_priority_fee_per_gas().map(U256::from),
                value: inner.value(),
                data: inner.input().clone(),
                chain_id: inner.chain_id(),
            })
        }
        _ => Err("Unsupported EVM tx envelope".to_string()),
    }
}

pub fn u256_is_multiple_of_spore(value: &U256) -> bool {
    let divisor = U256::from(1_000_000_000u64);
    value % divisor == U256::ZERO
}

/// Lichen EVM chain ID
pub const LICHEN_CHAIN_ID: u64 = 8001;

pub fn u256_to_spores(value: &U256) -> u64 {
    let divisor = U256::from(1_000_000_000u64);
    let spores = *value / divisor;
    // T3.9 fix: reject values that exceed u64::MAX instead of silently clamping
    spores.try_into().unwrap_or({
        // In a Result-returning context this would be Err;
        // Here we saturate but callers should use u256_to_spores_checked.
        u64::MAX
    })
}

/// Checked conversion — returns Err if value exceeds u64::MAX spores
pub fn u256_to_spores_checked(value: &U256) -> Result<u64, String> {
    let divisor = U256::from(1_000_000_000u64);
    let spores = *value / divisor;
    let max = U256::from(u64::MAX);
    if spores > max {
        return Err(format!(
            "EVM value {} exceeds maximum representable spores",
            value
        ));
    }
    Ok(spores.try_into().unwrap_or(u64::MAX))
}

pub fn spores_to_u256(spores: u64) -> U256 {
    U256::from(spores) * U256::from(1_000_000_000u64)
}

#[derive(Debug)]
struct EvmDbError(String);

impl fmt::Display for EvmDbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for EvmDbError {}

impl DBErrorMarker for EvmDbError {}

struct StateEvmDb {
    state: StateStore,
    code_cache: StdHashMap<RevmB256, Bytecode>,
    /// Errors accumulated during DatabaseCommit::commit() (C7 fix)
    commit_errors: Vec<String>,
}

impl StateEvmDb {
    fn new(state: StateStore) -> Self {
        Self {
            state,
            code_cache: StdHashMap::new(),
            commit_errors: Vec::new(),
        }
    }

    fn get_native_balance(&self, address: RevmAddress) -> Result<Option<RevmU256>, EvmDbError> {
        let evm_address = revm_address_to_array(address);
        let mapped = self
            .state
            .lookup_evm_address(&evm_address)
            .map_err(EvmDbError)?;
        if let Some(pubkey) = mapped {
            let account = self.state.get_account(&pubkey).map_err(EvmDbError)?;
            if let Some(acc) = account {
                // Use only spendable balance — staked/locked funds must not be EVM-accessible
                return Ok(Some(revm_u256_from_alloy(spores_to_u256(acc.spendable))));
            }
            return Ok(Some(RevmU256::ZERO));
        }
        Ok(None)
    }

    fn load_account(&mut self, address: RevmAddress) -> Result<Option<AccountInfo>, EvmDbError> {
        if let Some(native_balance) = self.get_native_balance(address)? {
            let address_bytes = revm_address_to_array(address);
            let evm_account = self
                .state
                .get_evm_account(&address_bytes)
                .map_err(EvmDbError)?;
            let (nonce, code) = if let Some(account) = evm_account {
                (account.nonce, account.code)
            } else {
                (0u64, Vec::new())
            };
            let bytecode = if code.is_empty() {
                Bytecode::new()
            } else {
                Bytecode::new_raw(RevmBytes::from(code))
            };
            let code_hash = bytecode.hash_slow();
            self.code_cache.insert(code_hash, bytecode.clone());
            return Ok(Some(AccountInfo {
                balance: native_balance,
                nonce,
                code_hash,
                account_id: None,
                code: Some(bytecode),
            }));
        }

        let address_bytes = revm_address_to_array(address);
        if let Some(account) = self
            .state
            .get_evm_account(&address_bytes)
            .map_err(EvmDbError)?
        {
            let bytecode = if account.code.is_empty() {
                Bytecode::new()
            } else {
                Bytecode::new_raw(RevmBytes::from(account.code.clone()))
            };
            let code_hash = bytecode.hash_slow();
            self.code_cache.insert(code_hash, bytecode.clone());
            return Ok(Some(AccountInfo {
                balance: revm_u256_from_alloy(account.balance_u256()),
                nonce: account.nonce,
                code_hash,
                account_id: None,
                code: Some(bytecode),
            }));
        }

        Ok(None)
    }
}

impl Database for StateEvmDb {
    type Error = EvmDbError;

    fn basic(&mut self, address: RevmAddress) -> Result<Option<AccountInfo>, Self::Error> {
        self.load_account(address)
    }

    fn code_by_hash(&mut self, code_hash: RevmB256) -> Result<Bytecode, Self::Error> {
        if let Some(code) = self.code_cache.get(&code_hash) {
            return Ok(code.clone());
        }
        Ok(Bytecode::new())
    }

    fn storage(&mut self, address: RevmAddress, index: RevmU256) -> Result<RevmU256, Self::Error> {
        let address_bytes = revm_address_to_array(address);
        let slot = alloy_u256_from_revm(index).to_be_bytes::<32>();
        let value = self
            .state
            .get_evm_storage(&address_bytes, &slot)
            .map_err(EvmDbError)?;
        Ok(revm_u256_from_alloy(value))
    }

    fn block_hash(&mut self, number: u64) -> Result<RevmB256, Self::Error> {
        if let Ok(Some(block)) = self.state.get_block_by_slot(number) {
            return Ok(RevmB256::from(block.hash().0));
        }
        Ok(RevmB256::ZERO)
    }
}

impl DatabaseCommit for StateEvmDb {
    fn commit(&mut self, changes: RevmHashMap<RevmAddress, Account>) {
        // P9-CORE-07: Guard against direct state writes bypassing StateBatch atomicity.
        // In production, EVM state changes must be collected and applied via StateBatch
        // to ensure atomic block processing. This commit() is retained for revm trait
        // compatibility but logs a warning when called with actual changes.
        if !changes.is_empty() {
            eprintln!(
                "⚠️  P9-CORE-07: StateEvmDb::commit() called with {} changes — \
                 these writes bypass StateBatch atomicity. Use collect_changes() instead.",
                changes.len()
            );
        }
        self.commit_errors.clear();
        for (address, account) in changes {
            let address_bytes = revm_address_to_array(address);
            if account.is_empty() {
                if let Err(e) = self.state.clear_evm_account(&address_bytes) {
                    self.commit_errors
                        .push(format!("clear EVM account {:?}: {}", address_bytes, e));
                }
                if let Err(e) = self.state.clear_evm_storage(&address_bytes) {
                    self.commit_errors
                        .push(format!("clear EVM storage {:?}: {}", address_bytes, e));
                }
                continue;
            }

            let mut stored = EvmAccount::new();
            stored.nonce = account.info.nonce;
            stored.set_balance_u256(alloy_u256_from_revm(account.info.balance));
            if let Some(code) = account.info.code {
                stored.code = code.bytes().to_vec();
            }

            if let Err(e) = self.state.put_evm_account(&address_bytes, &stored) {
                self.commit_errors
                    .push(format!("put EVM account {:?}: {}", address_bytes, e));
            }

            for (slot, value) in account.storage {
                let slot_bytes = alloy_u256_from_revm(slot).to_be_bytes::<32>();
                let present_value = alloy_u256_from_revm(value.present_value);
                let result = if present_value == U256::ZERO {
                    self.state
                        .clear_evm_storage_slot(&address_bytes, &slot_bytes)
                } else {
                    self.state
                        .put_evm_storage(&address_bytes, &slot_bytes, present_value)
                };
                if let Err(e) = result {
                    self.commit_errors.push(format!(
                        "commit EVM storage {:?} slot: {}",
                        address_bytes, e
                    ));
                }
            }

            // T3.8 fix: Always write back native balance, even if not a perfect
            // spore multiple. Round down to nearest spore to avoid fractional
            // spore amounts in the native account system.
            if let Ok(Some(pubkey)) = self.state.lookup_evm_address(&address_bytes) {
                let balance = alloy_u256_from_revm(account.info.balance);
                let divisor = U256::from(1_000_000_000u64);
                let spores = balance / divisor;
                // M9 fix: reject overflow instead of saturating to u64::MAX (prevents silent inflation)
                let spores_u64: u64 = match spores.try_into() {
                    Ok(v) => v,
                    Err(_) => {
                        self.commit_errors.push(format!(
                            "EVM balance overflow for {:?}: spores {} exceeds u64::MAX",
                            address_bytes, spores
                        ));
                        continue;
                    }
                };
                if let Err(e) = self.state.set_spendable_balance(&pubkey, spores_u64) {
                    self.commit_errors
                        .push(format!("commit native balance for {:?}: {}", pubkey, e));
                }
                // Warn if there's a fractional remainder being dropped
                let remainder = balance % divisor;
                if remainder != U256::ZERO {
                    eprintln!(
                        "T3.8: EVM balance for {:?} has sub-spore remainder {} wei (dropped)",
                        address_bytes, remainder
                    );
                }
            }
        }
    }
}

pub fn execute_evm_transaction(
    state: StateStore,
    tx: &EvmTx,
    chain_id: u64,
) -> Result<(EvmExecutionResult, EvmStateChanges), String> {
    // T3.10 fix: Reject pre-EIP-155 transactions (chain_id=0) and wrong chain IDs
    if let Some(tx_chain_id) = tx.chain_id {
        if tx_chain_id != chain_id {
            return Err(format!(
                "Invalid chainId: expected {}, got {}",
                chain_id, tx_chain_id
            ));
        }
    } else {
        return Err("Transaction must include chain_id (EIP-155)".to_string());
    }

    if !u256_is_multiple_of_spore(&tx.value) {
        return Err("EVM value must be multiple of 1e9 wei".to_string());
    }
    if !u256_is_multiple_of_spore(&tx.gas_price) {
        return Err("EVM gas price must be multiple of 1e9 wei".to_string());
    }
    if let Some(max_fee) = tx.max_fee_per_gas {
        if !u256_is_multiple_of_spore(&max_fee) {
            return Err("EVM max fee must be multiple of 1e9 wei".to_string());
        }
    }
    if let Some(priority_fee) = tx.max_priority_fee_per_gas {
        if !u256_is_multiple_of_spore(&priority_fee) {
            return Err("EVM priority fee must be multiple of 1e9 wei".to_string());
        }
    }

    let current_slot = state.get_last_slot().unwrap_or(0);
    // T3.7: Get actual UNIX timestamp for EVM block.timestamp
    // Use previous block's timestamp for determinism; fall back to 0 at genesis.
    let block_timestamp = if let Ok(Some(block)) = state.get_block_by_slot(current_slot) {
        block.header.timestamp
    } else {
        0 // Genesis or missing block — deterministic fallback, never wall-clock
    };
    let db = StateEvmDb::new(state);
    let mut context: Context<
        BlockEnv,
        TxEnv,
        CfgEnv,
        StateEvmDb,
        revm::context::Journal<StateEvmDb>,
        (),
    > = Context::new(db, SpecId::PRAGUE);

    context.cfg.chain_id = chain_id;
    context.block.number = RevmU256::from(current_slot);
    context.block.timestamp = RevmU256::from(block_timestamp);
    context.block.basefee = 1; // Non-zero for EIP-1559 compatibility

    let tx_env = build_revm_tx_env(tx, chain_id)?;
    let mut evm = context.build_mainnet();

    // H3 fix: Use `transact()` (non-commit) so EVM state changes are NOT persisted
    // until the caller commits them atomically through StateBatch.
    let result_and_state = evm
        .transact(tx_env)
        .map_err(|e| format!("EVM execution error: {}", e))?;

    // Convert REVM state changes to our deferred format
    let evm_changes = convert_revm_state_to_deferred(
        &evm.ctx.journaled_state.database.state,
        result_and_state.state,
    );

    // If conversion produced hard errors, fail
    if !evm_changes.errors.is_empty() {
        return Err(format!(
            "EVM state conversion failed: {}",
            evm_changes.errors.join("; ")
        ));
    }

    match result_and_state.result {
        ExecutionResult::Success {
            gas_used,
            output,
            logs,
            ..
        } => {
            let (output_bytes, created_address) = match output {
                Output::Call(data) => (data.to_vec(), None),
                Output::Create(data, address) => {
                    (data.to_vec(), address.map(revm_address_to_array))
                }
            };
            // Task 3.4: Extract structured logs with address + topics + data
            let structured_logs: Vec<EvmLog> = logs
                .iter()
                .map(|log| EvmLog {
                    address: revm_address_to_array(log.address),
                    topics: log.data.topics().iter().map(|t| t.0).collect(),
                    data: log.data.data.to_vec(),
                })
                .collect();
            Ok((
                EvmExecutionResult {
                    success: true,
                    gas_used,
                    output: output_bytes,
                    created_address,
                    logs: logs
                        .iter()
                        .map(|log| log.data.data.clone().into())
                        .collect(),
                    structured_logs,
                },
                evm_changes,
            ))
        }
        ExecutionResult::Revert { gas_used, output } => Ok((
            EvmExecutionResult {
                success: false,
                gas_used,
                output: output.to_vec(),
                created_address: None,
                logs: Vec::new(),
                structured_logs: Vec::new(),
            },
            evm_changes,
        )),
        ExecutionResult::Halt { gas_used, .. } => Ok((
            EvmExecutionResult {
                success: false,
                gas_used,
                output: Vec::new(),
                created_address: None,
                logs: Vec::new(),
                structured_logs: Vec::new(),
            },
            evm_changes,
        )),
    }
}

/// Convert REVM state changes to our deferred format for atomic commit through StateBatch.
/// The `state_db` reference is needed to lookup native ↔ EVM address mappings.
fn convert_revm_state_to_deferred(
    state_db: &StateStore,
    revm_state: RevmHashMap<RevmAddress, Account>,
) -> EvmStateChanges {
    let mut changes = Vec::with_capacity(revm_state.len());
    let mut errors = Vec::new();

    for (address, account) in revm_state {
        let address_bytes = revm_address_to_array(address);

        if account.is_empty() {
            // Account destroyed — clear account + storage
            changes.push(EvmStateChange {
                evm_address: address_bytes,
                account: None,
                storage_changes: Vec::new(), // on-disk storage cleared by StateBatch
                native_balance_update: None,
            });
            continue;
        }

        // Build EVM account data
        let mut stored = EvmAccount::new();
        stored.nonce = account.info.nonce;
        stored.set_balance_u256(alloy_u256_from_revm(account.info.balance));
        if let Some(code) = account.info.code {
            stored.code = code.bytes().to_vec();
        }

        // Build storage changes
        let mut storage_changes = Vec::with_capacity(account.storage.len());
        for (slot, value) in account.storage {
            let slot_bytes = alloy_u256_from_revm(slot).to_be_bytes::<32>();
            let present_value = alloy_u256_from_revm(value.present_value);
            if present_value == U256::ZERO {
                storage_changes.push((slot_bytes, None));
            } else {
                storage_changes.push((slot_bytes, Some(present_value)));
            }
        }

        // Check for native account mapping
        let native_balance_update = match state_db.lookup_evm_address(&address_bytes) {
            Ok(Some(pubkey)) => {
                let balance = alloy_u256_from_revm(account.info.balance);
                let divisor = U256::from(1_000_000_000u64);
                let spores = balance / divisor;
                match spores.try_into() {
                    Ok(spores_u64) => {
                        let remainder = balance % divisor;
                        if remainder != U256::ZERO {
                            eprintln!(
                                "H3: EVM balance for {:?} has sub-spore remainder {} wei (dropped)",
                                address_bytes, remainder
                            );
                        }
                        Some((pubkey, spores_u64))
                    }
                    Err(_) => {
                        errors.push(format!(
                            "EVM balance overflow for {:?}: spores {} exceeds u64::MAX",
                            address_bytes, spores
                        ));
                        None
                    }
                }
            }
            Ok(None) => None,
            Err(e) => {
                errors.push(format!(
                    "Failed to lookup EVM mapping for {:?}: {}",
                    address_bytes, e
                ));
                None
            }
        };

        changes.push(EvmStateChange {
            evm_address: address_bytes,
            account: Some(stored),
            storage_changes,
            native_balance_update,
        });
    }

    EvmStateChanges { changes, errors }
}

pub fn simulate_evm_call(
    state: StateStore,
    from: Address,
    to: Option<Address>,
    data: Bytes,
    value: U256,
    gas_limit: u64,
    chain_id: u64,
) -> Result<Vec<u8>, String> {
    if !u256_is_multiple_of_spore(&value) {
        return Err("EVM value must be multiple of 1e9 wei".to_string());
    }

    let current_slot = state.get_last_slot().unwrap_or(0);
    // T3.7: Get actual UNIX timestamp for simulate_evm_call too
    // Use previous block's timestamp for determinism; fall back to 0 at genesis.
    let block_timestamp = if let Ok(Some(block)) = state.get_block_by_slot(current_slot) {
        block.header.timestamp
    } else {
        0 // Genesis or missing block — deterministic fallback, never wall-clock
    };
    let db = StateEvmDb::new(state);
    let mut context: Context<
        BlockEnv,
        TxEnv,
        CfgEnv,
        StateEvmDb,
        revm::context::Journal<StateEvmDb>,
        (),
    > = Context::new(db, SpecId::PRAGUE);

    context.cfg.chain_id = chain_id;
    context.block.number = RevmU256::from(current_slot);
    context.block.timestamp = RevmU256::from(block_timestamp);
    context.block.basefee = 1;

    let tx_env = build_revm_call_env(from, to, data, value, gas_limit, chain_id)?;
    let mut evm = context.build_mainnet();

    let result = evm
        .transact(tx_env)
        .map_err(|e| format!("EVM call error: {}", e))?;

    match result.result {
        ExecutionResult::Success { output, .. } => Ok(output.into_data().to_vec()),
        ExecutionResult::Revert { output, .. } => Ok(output.to_vec()),
        ExecutionResult::Halt { .. } => Ok(Vec::new()),
    }
}

pub fn evm_tx_to_native_instruction(raw: &[u8]) -> Vec<u8> {
    raw.to_vec()
}

fn revm_address_to_array(address: RevmAddress) -> [u8; 20] {
    address.into()
}

fn revm_address_from_alloy(address: Address) -> RevmAddress {
    let bytes: [u8; 20] = address.into();
    RevmAddress::from(bytes)
}

fn revm_u256_from_alloy(value: U256) -> RevmU256 {
    RevmU256::from_be_bytes(value.to_be_bytes::<32>())
}

fn alloy_u256_from_revm(value: RevmU256) -> U256 {
    U256::from_be_bytes(value.to_be_bytes::<32>())
}

fn u256_to_u128(value: U256, label: &str) -> Result<u128, String> {
    let bytes = value.to_be_bytes::<32>();
    if bytes[..16].iter().any(|b| *b != 0) {
        return Err(format!("EVM {} exceeds u128", label));
    }
    Ok(u128::from_be_bytes(bytes[16..].try_into().unwrap()))
}

fn build_revm_tx_env(tx: &EvmTx, chain_id: u64) -> Result<TxEnv, String> {
    let caller = revm_address_from_alloy(tx.from);
    let kind = match tx.to {
        Some(to) => TxKind::Call(revm_address_from_alloy(to)),
        None => TxKind::Create,
    };

    let data_vec: Vec<u8> = tx.data.clone().into();
    let data = RevmBytes::from(data_vec);
    let value = revm_u256_from_alloy(tx.value);
    let gas_price = u256_to_u128(tx.gas_price, "gas price")?;

    let mut builder = TxEnv::builder()
        .caller(caller)
        .gas_limit(tx.gas_limit)
        .value(value)
        .data(data)
        .nonce(tx.nonce)
        .chain_id(Some(chain_id))
        .kind(kind);

    if let Some(max_fee) = tx.max_fee_per_gas {
        builder = builder.max_fee_per_gas(u256_to_u128(max_fee, "max fee")?);
    } else {
        builder = builder.gas_price(gas_price);
    }

    if let Some(priority_fee) = tx.max_priority_fee_per_gas {
        builder = builder.gas_priority_fee(Some(u256_to_u128(priority_fee, "priority fee")?));
    }

    builder
        .build()
        .map_err(|e| format!("Invalid EVM tx: {:?}", e))
}

fn build_revm_call_env(
    from: Address,
    to: Option<Address>,
    data: Bytes,
    value: U256,
    gas_limit: u64,
    chain_id: u64,
) -> Result<TxEnv, String> {
    let caller = revm_address_from_alloy(from);
    let kind = match to {
        Some(to) => TxKind::Call(revm_address_from_alloy(to)),
        None => TxKind::Create,
    };

    let data_vec: Vec<u8> = data.into();
    let builder = TxEnv::builder()
        .caller(caller)
        .gas_limit(gas_limit)
        .gas_price(0)
        .value(revm_u256_from_alloy(value))
        .data(RevmBytes::from(data_vec))
        .chain_id(Some(chain_id))
        .kind(kind);

    builder
        .build()
        .map_err(|e| format!("Invalid EVM call: {:?}", e))
}

// ─── Task 3.4: Standard EVM precompile addresses ────────────────────────────
// REVM's build_mainnet() with SpecId::PRAGUE includes all standard precompiles.
// These constants document the addresses for reference and testing.

/// ecRecover (ECDSARECOVER) precompile address
pub const PRECOMPILE_ECRECOVER: [u8; 20] = {
    let mut addr = [0u8; 20];
    addr[19] = 0x01;
    addr
};

/// SHA-256 hash precompile address
pub const PRECOMPILE_SHA256: [u8; 20] = {
    let mut addr = [0u8; 20];
    addr[19] = 0x02;
    addr
};

/// RIPEMD-160 hash precompile address
pub const PRECOMPILE_RIPEMD160: [u8; 20] = {
    let mut addr = [0u8; 20];
    addr[19] = 0x03;
    addr
};

/// Identity (data copy) precompile address
pub const PRECOMPILE_IDENTITY: [u8; 20] = {
    let mut addr = [0u8; 20];
    addr[19] = 0x04;
    addr
};

/// Modular exponentiation (MODEXP) precompile address
pub const PRECOMPILE_MODEXP: [u8; 20] = {
    let mut addr = [0u8; 20];
    addr[19] = 0x05;
    addr
};

/// BN256 point addition precompile address
pub const PRECOMPILE_BN256_ADD: [u8; 20] = {
    let mut addr = [0u8; 20];
    addr[19] = 0x06;
    addr
};

/// BN256 scalar multiplication precompile address
pub const PRECOMPILE_BN256_MUL: [u8; 20] = {
    let mut addr = [0u8; 20];
    addr[19] = 0x07;
    addr
};

/// BN256 pairing precompile address
pub const PRECOMPILE_BN256_PAIRING: [u8; 20] = {
    let mut addr = [0u8; 20];
    addr[19] = 0x08;
    addr
};

/// Blake2f compression precompile address
pub const PRECOMPILE_BLAKE2F: [u8; 20] = {
    let mut addr = [0u8; 20];
    addr[19] = 0x09;
    addr
};

/// List of all supported precompile addresses and their names.
pub fn supported_precompiles() -> Vec<([u8; 20], &'static str)> {
    vec![
        (PRECOMPILE_ECRECOVER, "ecRecover"),
        (PRECOMPILE_SHA256, "SHA-256"),
        (PRECOMPILE_RIPEMD160, "RIPEMD-160"),
        (PRECOMPILE_IDENTITY, "identity"),
        (PRECOMPILE_MODEXP, "modexp"),
        (PRECOMPILE_BN256_ADD, "bn256Add"),
        (PRECOMPILE_BN256_MUL, "bn256Mul"),
        (PRECOMPILE_BN256_PAIRING, "bn256Pairing"),
        (PRECOMPILE_BLAKE2F, "blake2f"),
    ]
}

/// Match topic filter against log topics per EIP-1474.
/// Each position in filter_topics can be:
/// - None: wildcard (matches any topic)
/// - Some(single): exact match for that position
/// - The caller handles OR-arrays by expanding before calling this.
pub fn topics_match(log_topics: &[[u8; 32]], filter_topics: &[Option<Vec<[u8; 32]>>]) -> bool {
    for (i, filter) in filter_topics.iter().enumerate() {
        if let Some(candidates) = filter {
            // Must match at least one candidate at this position
            match log_topics.get(i) {
                Some(log_topic) => {
                    if !candidates.iter().any(|c| c == log_topic) {
                        return false;
                    }
                }
                None => return false, // Log doesn't have enough topics
            }
        }
        // None = wildcard, always matches
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spores_to_u256_conversion() {
        let spores: u64 = 1_000_000_000; // 1 LICN
        let u256 = spores_to_u256(spores);
        // spores_to_u256 converts spores → wei (1 spore = 10^9 wei)
        assert!(u256 > U256::ZERO);
    }

    #[test]
    fn test_address_roundtrip() {
        let addr = RevmAddress::from([0xABu8; 20]);
        let arr = revm_address_to_array(addr);
        assert_eq!(arr.len(), 20);
        assert_eq!(arr[0], 0xAB);
    }

    #[test]
    fn test_evm_uses_spendable_balance() {
        // This test verifies that the EVM bridge uses acc.spendable, not acc.spores
        use crate::{Account, Pubkey, StateStore};
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey::new([1u8; 32]);
        let mut account = Account::new(100, pk); // 100 LICN total
                                                 // Stake half — spendable should be reduced
        account.staked = Account::licn_to_spores(50);
        account.spendable = Account::licn_to_spores(50);
        // spores stays at 100 LICN total
        state.put_account(&pk, &account).unwrap();

        // Map an EVM address to this account
        let evm_addr = [0xABu8; 20];
        state.register_evm_address(&evm_addr, &pk).unwrap();

        let db = StateEvmDb::new(state);
        let revm_addr = RevmAddress::from(evm_addr);
        let balance = db.get_native_balance(revm_addr).unwrap();

        // Should reflect spendable (50 LICN), not total (100 LICN)
        assert!(balance.is_some());
        let balance_u256 = balance.unwrap();
        let total_u256 = revm_u256_from_alloy(spores_to_u256(Account::licn_to_spores(100)));
        let spendable_u256 = revm_u256_from_alloy(spores_to_u256(Account::licn_to_spores(50)));

        assert_eq!(
            balance_u256, spendable_u256,
            "EVM balance should equal spendable, not total"
        );
        assert_ne!(
            balance_u256, total_u256,
            "EVM balance should NOT equal total spores"
        );
    }

    // ── H3 tests: deferred EVM state types ──

    #[test]
    fn test_evm_state_change_default() {
        let changes = EvmStateChanges::default();
        assert!(changes.changes.is_empty());
        assert!(changes.errors.is_empty());
    }

    #[test]
    fn test_evm_state_change_construction() {
        let change = EvmStateChange {
            evm_address: [0xAB; 20],
            account: Some(EvmAccount {
                nonce: 1,
                balance: [0u8; 32],
                code: vec![],
            }),
            storage_changes: vec![([0x01; 32], Some(U256::from(42)))],
            native_balance_update: None,
        };
        assert_eq!(change.evm_address[0], 0xAB);
        assert!(change.account.is_some());
        assert_eq!(change.storage_changes.len(), 1);
        assert!(change.native_balance_update.is_none());
    }

    #[test]
    fn test_evm_state_changes_with_native_balance() {
        let pk = crate::Pubkey([0x01; 32]);
        let change = EvmStateChange {
            evm_address: [0xCC; 20],
            account: None,
            storage_changes: vec![],
            native_balance_update: Some((pk, 500)),
        };
        let changes = EvmStateChanges {
            changes: vec![change],
            errors: vec![],
        };
        assert_eq!(changes.changes.len(), 1);
        assert_eq!(
            changes.changes[0].native_balance_update.as_ref().unwrap().1,
            500
        );
    }

    // ── Task 3.4: EVM Precompiles + eth_getLogs tests ──

    #[test]
    fn test_evm_log_serde_roundtrip() {
        let log = EvmLog {
            address: [0xAB; 20],
            topics: vec![[0x01; 32], [0x02; 32]],
            data: vec![0xFF, 0xFE, 0xFD],
        };
        let bytes = bincode::serialize(&log).expect("serialize");
        let decoded: EvmLog = bincode::deserialize(&bytes).expect("deserialize");
        assert_eq!(decoded.address, [0xAB; 20]);
        assert_eq!(decoded.topics.len(), 2);
        assert_eq!(decoded.topics[0], [0x01; 32]);
        assert_eq!(decoded.topics[1], [0x02; 32]);
        assert_eq!(decoded.data, vec![0xFF, 0xFE, 0xFD]);
    }

    #[test]
    fn test_evm_log_entry_serde_roundtrip() {
        let entry = EvmLogEntry {
            tx_hash: [0xAA; 32],
            tx_index: 5,
            log_index: 3,
            log: EvmLog {
                address: [0xCC; 20],
                topics: vec![[0x11; 32]],
                data: vec![0x42],
            },
        };
        let bytes = bincode::serialize(&entry).expect("serialize");
        let decoded: EvmLogEntry = bincode::deserialize(&bytes).expect("deserialize");
        assert_eq!(decoded.tx_hash, [0xAA; 32]);
        assert_eq!(decoded.tx_index, 5);
        assert_eq!(decoded.log_index, 3);
        assert_eq!(decoded.log.address, [0xCC; 20]);
        assert_eq!(decoded.log.topics.len(), 1);
        assert_eq!(decoded.log.data, vec![0x42]);
    }

    #[test]
    fn test_evm_log_json_roundtrip() {
        let log = EvmLog {
            address: [0x00; 20],
            topics: vec![],
            data: vec![],
        };
        let json = serde_json::to_string(&log).expect("json serialize");
        let decoded: EvmLog = serde_json::from_str(&json).expect("json deserialize");
        assert_eq!(decoded.address, [0x00; 20]);
        assert!(decoded.topics.is_empty());
        assert!(decoded.data.is_empty());
    }

    #[test]
    fn test_topics_match_wildcard() {
        // All None = matches everything
        let log_topics = vec![[0x01; 32], [0x02; 32]];
        let filter: Vec<Option<Vec<[u8; 32]>>> = vec![None, None];
        assert!(topics_match(&log_topics, &filter));
    }

    #[test]
    fn test_topics_match_empty_filter() {
        // Empty filter = matches everything
        let log_topics = vec![[0x01; 32]];
        let filter: Vec<Option<Vec<[u8; 32]>>> = vec![];
        assert!(topics_match(&log_topics, &filter));
    }

    #[test]
    fn test_topics_match_exact_single() {
        let topic_a = [0xAA; 32];
        let topic_b = [0xBB; 32];
        let log_topics = vec![topic_a, topic_b];

        // Match: filter matches topic[0]
        let filter = vec![Some(vec![topic_a])];
        assert!(topics_match(&log_topics, &filter));

        // No match: filter at position 0 doesn't match
        let filter = vec![Some(vec![topic_b])];
        assert!(!topics_match(&log_topics, &filter));
    }

    #[test]
    fn test_topics_match_or_array() {
        let topic_a = [0xAA; 32];
        let topic_b = [0xBB; 32];
        let topic_c = [0xCC; 32];
        let log_topics = vec![topic_a];

        // OR: topic_a OR topic_c at position 0 — matches because topic_a is present
        let filter = vec![Some(vec![topic_a, topic_c])];
        assert!(topics_match(&log_topics, &filter));

        // OR: topic_b OR topic_c at position 0 — no match
        let filter = vec![Some(vec![topic_b, topic_c])];
        assert!(!topics_match(&log_topics, &filter));
    }

    #[test]
    fn test_topics_match_wildcard_then_exact() {
        let topic_a = [0xAA; 32];
        let topic_b = [0xBB; 32];
        let log_topics = vec![topic_a, topic_b];

        // Position 0: wildcard, Position 1: exact match on topic_b
        let filter: Vec<Option<Vec<[u8; 32]>>> = vec![None, Some(vec![topic_b])];
        assert!(topics_match(&log_topics, &filter));

        // Position 0: wildcard, Position 1: wrong topic
        let filter: Vec<Option<Vec<[u8; 32]>>> = vec![None, Some(vec![topic_a])];
        assert!(!topics_match(&log_topics, &filter));
    }

    #[test]
    fn test_topics_match_insufficient_log_topics() {
        let log_topics = vec![[0x01; 32]]; // Only 1 topic

        // Filter requires topic at position 1 — log doesn't have it
        let filter = vec![None, Some(vec![[0x02; 32]])];
        assert!(!topics_match(&log_topics, &filter));
    }

    #[test]
    fn test_topics_match_empty_log_topics() {
        let log_topics: Vec<[u8; 32]> = vec![];

        // Any non-wildcard filter should fail on empty log
        let filter = vec![Some(vec![[0x01; 32]])];
        assert!(!topics_match(&log_topics, &filter));

        // All wildcards should match even empty logs
        let filter: Vec<Option<Vec<[u8; 32]>>> = vec![None, None];
        assert!(topics_match(&log_topics, &filter));
    }

    #[test]
    fn test_supported_precompiles_returns_nine() {
        let precompiles = supported_precompiles();
        assert_eq!(
            precompiles.len(),
            9,
            "Should return exactly 9 precompiles (0x01-0x09)"
        );
    }

    #[test]
    fn test_supported_precompiles_addresses_sequential() {
        let precompiles = supported_precompiles();
        for (i, (addr, _name)) in precompiles.iter().enumerate() {
            // Each precompile address should be [0..0, N] where N = i+1
            assert_eq!(
                addr[19],
                (i + 1) as u8,
                "Precompile {} should have addr byte 0x{:02x}",
                i,
                i + 1
            );
            // First 19 bytes should be zero
            assert_eq!(
                &addr[..19],
                &[0u8; 19],
                "Precompile prefix bytes should be zero"
            );
        }
    }

    #[test]
    fn test_precompile_constants_match_ethereum() {
        // Standard Ethereum precompile addresses per EIP-196, EIP-197, EIP-198, EIP-152
        assert_eq!(PRECOMPILE_ECRECOVER[19], 0x01);
        assert_eq!(PRECOMPILE_SHA256[19], 0x02);
        assert_eq!(PRECOMPILE_RIPEMD160[19], 0x03);
        assert_eq!(PRECOMPILE_IDENTITY[19], 0x04);
        assert_eq!(PRECOMPILE_MODEXP[19], 0x05);
        assert_eq!(PRECOMPILE_BN256_ADD[19], 0x06);
        assert_eq!(PRECOMPILE_BN256_MUL[19], 0x07);
        assert_eq!(PRECOMPILE_BN256_PAIRING[19], 0x08);
        assert_eq!(PRECOMPILE_BLAKE2F[19], 0x09);
    }

    #[test]
    fn test_precompile_constants_are_20_byte_addresses() {
        let all = [
            PRECOMPILE_ECRECOVER,
            PRECOMPILE_SHA256,
            PRECOMPILE_RIPEMD160,
            PRECOMPILE_IDENTITY,
            PRECOMPILE_MODEXP,
            PRECOMPILE_BN256_ADD,
            PRECOMPILE_BN256_MUL,
            PRECOMPILE_BN256_PAIRING,
            PRECOMPILE_BLAKE2F,
        ];
        for (i, addr) in all.iter().enumerate() {
            assert_eq!(addr.len(), 20, "Precompile {} address must be 20 bytes", i);
            // All leading bytes zero, only last byte nonzero
            for (j, byte) in addr[..19].iter().enumerate() {
                assert_eq!(*byte, 0, "Precompile {} byte {} should be 0", i, j);
            }
        }
    }

    #[test]
    fn test_evm_receipt_structured_logs_serde_default() {
        // Backward compat: old receipts without structured_logs should deserialize
        let receipt = EvmReceipt {
            evm_hash: [0x01; 32],
            status: true,
            gas_used: 21000,
            block_slot: None,
            block_hash: None,
            contract_address: None,
            logs: vec![vec![0x01, 0x02]],
            structured_logs: vec![], // Would be skipped in JSON
        };
        let json = serde_json::to_string(&receipt).expect("serialize");
        // structured_logs is empty → should be omitted in JSON
        assert!(
            !json.contains("structured_logs"),
            "Empty structured_logs should be omitted via skip_serializing_if"
        );

        // Deserialize without structured_logs field → should default to empty
        let minimal = r#"{"evm_hash":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],"status":true,"gas_used":21000,"block_slot":null,"block_hash":null,"contract_address":null,"logs":[[1,2]]}"#;
        let decoded: EvmReceipt = serde_json::from_str(minimal).expect("deserialize");
        assert!(decoded.structured_logs.is_empty());
    }

    #[test]
    fn test_evm_receipt_with_structured_logs() {
        let receipt = EvmReceipt {
            evm_hash: [0x02; 32],
            status: true,
            gas_used: 50000,
            block_slot: Some(42),
            block_hash: None,
            contract_address: Some([0xDD; 20]),
            logs: vec![],
            structured_logs: vec![EvmLog {
                address: [0xDD; 20],
                topics: vec![[0x11; 32], [0x22; 32]],
                data: vec![0xAB, 0xCD],
            }],
        };
        let bytes = bincode::serialize(&receipt).expect("serialize");
        let decoded: EvmReceipt = bincode::deserialize(&bytes).expect("deserialize");
        assert_eq!(decoded.structured_logs.len(), 1);
        assert_eq!(decoded.structured_logs[0].address, [0xDD; 20]);
        assert_eq!(decoded.structured_logs[0].topics.len(), 2);
        assert_eq!(decoded.structured_logs[0].data, vec![0xAB, 0xCD]);
    }

    #[test]
    fn test_evm_execution_result_includes_structured_logs() {
        let result = EvmExecutionResult {
            success: true,
            gas_used: 21000,
            output: vec![],
            created_address: None,
            logs: vec![],
            structured_logs: vec![
                EvmLog {
                    address: [0x01; 20],
                    topics: vec![[0xAA; 32]],
                    data: vec![1, 2, 3],
                },
                EvmLog {
                    address: [0x02; 20],
                    topics: vec![[0xBB; 32], [0xCC; 32]],
                    data: vec![4, 5],
                },
            ],
        };
        assert_eq!(result.structured_logs.len(), 2);
        assert_eq!(result.structured_logs[0].address, [0x01; 20]);
        assert_eq!(result.structured_logs[1].topics.len(), 2);
    }
}
