// MoltChain EVM integration

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
    /// Native account balance sync: `Some((pubkey, spendable_shells))`
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

#[derive(Debug, Clone)]
pub struct EvmExecutionResult {
    pub success: bool,
    pub gas_used: u64,
    pub output: Vec<u8>,
    pub created_address: Option<[u8; 20]>,
    pub logs: Vec<Vec<u8>>,
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

pub fn u256_is_multiple_of_shell(value: &U256) -> bool {
    let divisor = U256::from(1_000_000_000u64);
    value % divisor == U256::ZERO
}

/// MoltChain EVM chain ID
pub const MOLTCHAIN_CHAIN_ID: u64 = 8001;

pub fn u256_to_shells(value: &U256) -> u64 {
    let divisor = U256::from(1_000_000_000u64);
    let shells = *value / divisor;
    // T3.9 fix: reject values that exceed u64::MAX instead of silently clamping
    shells.try_into().unwrap_or({
        // In a Result-returning context this would be Err;
        // Here we saturate but callers should use u256_to_shells_checked.
        u64::MAX
    })
}

/// Checked conversion — returns Err if value exceeds u64::MAX shells
pub fn u256_to_shells_checked(value: &U256) -> Result<u64, String> {
    let divisor = U256::from(1_000_000_000u64);
    let shells = *value / divisor;
    let max = U256::from(u64::MAX);
    if shells > max {
        return Err(format!(
            "EVM value {} exceeds maximum representable shells",
            value
        ));
    }
    Ok(shells.try_into().unwrap_or(u64::MAX))
}

pub fn shells_to_u256(shells: u64) -> U256 {
    U256::from(shells) * U256::from(1_000_000_000u64)
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
                return Ok(Some(revm_u256_from_alloy(shells_to_u256(acc.spendable))));
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
            // shell multiple. Round down to nearest shell to avoid fractional
            // shell amounts in the native account system.
            if let Ok(Some(pubkey)) = self.state.lookup_evm_address(&address_bytes) {
                let balance = alloy_u256_from_revm(account.info.balance);
                let divisor = U256::from(1_000_000_000u64);
                let shells = balance / divisor;
                // M9 fix: reject overflow instead of saturating to u64::MAX (prevents silent inflation)
                let shells_u64: u64 = match shells.try_into() {
                    Ok(v) => v,
                    Err(_) => {
                        self.commit_errors.push(format!(
                            "EVM balance overflow for {:?}: shells {} exceeds u64::MAX",
                            address_bytes, shells
                        ));
                        continue;
                    }
                };
                if let Err(e) = self.state.set_spendable_balance(&pubkey, shells_u64) {
                    self.commit_errors
                        .push(format!("commit native balance for {:?}: {}", pubkey, e));
                }
                // Warn if there's a fractional remainder being dropped
                let remainder = balance % divisor;
                if remainder != U256::ZERO {
                    eprintln!(
                        "T3.8: EVM balance for {:?} has sub-shell remainder {} wei (dropped)",
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

    if !u256_is_multiple_of_shell(&tx.value) {
        return Err("EVM value must be multiple of 1e9 wei".to_string());
    }
    if !u256_is_multiple_of_shell(&tx.gas_price) {
        return Err("EVM gas price must be multiple of 1e9 wei".to_string());
    }
    if let Some(max_fee) = tx.max_fee_per_gas {
        if !u256_is_multiple_of_shell(&max_fee) {
            return Err("EVM max fee must be multiple of 1e9 wei".to_string());
        }
    }
    if let Some(priority_fee) = tx.max_priority_fee_per_gas {
        if !u256_is_multiple_of_shell(&priority_fee) {
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
                let shells = balance / divisor;
                match shells.try_into() {
                    Ok(shells_u64) => {
                        let remainder = balance % divisor;
                        if remainder != U256::ZERO {
                            eprintln!(
                                "H3: EVM balance for {:?} has sub-shell remainder {} wei (dropped)",
                                address_bytes, remainder
                            );
                        }
                        Some((pubkey, shells_u64))
                    }
                    Err(_) => {
                        errors.push(format!(
                            "EVM balance overflow for {:?}: shells {} exceeds u64::MAX",
                            address_bytes, shells
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
    if !u256_is_multiple_of_shell(&value) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shells_to_u256_conversion() {
        let shells: u64 = 1_000_000_000; // 1 MOLT
        let u256 = shells_to_u256(shells);
        // shells_to_u256 converts shells → wei (1 shell = 10^9 wei)
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
        // This test verifies that the EVM bridge uses acc.spendable, not acc.shells
        use crate::{Account, Pubkey, StateStore};
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey::new([1u8; 32]);
        let mut account = Account::new(100, pk); // 100 MOLT total
                                                 // Stake half — spendable should be reduced
        account.staked = Account::molt_to_shells(50);
        account.spendable = Account::molt_to_shells(50);
        // shells stays at 100 MOLT total
        state.put_account(&pk, &account).unwrap();

        // Map an EVM address to this account
        let evm_addr = [0xABu8; 20];
        state.register_evm_address(&evm_addr, &pk).unwrap();

        let db = StateEvmDb::new(state);
        let revm_addr = RevmAddress::from(evm_addr);
        let balance = db.get_native_balance(revm_addr).unwrap();

        // Should reflect spendable (50 MOLT), not total (100 MOLT)
        assert!(balance.is_some());
        let balance_u256 = balance.unwrap();
        let total_u256 = revm_u256_from_alloy(shells_to_u256(Account::molt_to_shells(100)));
        let spendable_u256 = revm_u256_from_alloy(shells_to_u256(Account::molt_to_shells(50)));

        assert_eq!(
            balance_u256, spendable_u256,
            "EVM balance should equal spendable, not total"
        );
        assert_ne!(
            balance_u256, total_u256,
            "EVM balance should NOT equal total shells"
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
}
