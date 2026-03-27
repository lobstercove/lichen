// Lichen Smart Contract System
// WASM-based programmable contracts with proper host function implementations

use crate::{Account, Hash, Pubkey, StateStore};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Type alias for cross-contract call pending storage changes.
type CccChanges = HashMap<Pubkey, HashMap<Vec<u8>, Option<Vec<u8>>>>;

use wasmer::{
    imports, CompilerConfig, Function, FunctionEnv, FunctionEnvMut, Instance, Memory, Module,
    Store, Type, Value,
};
use wasmer_compiler_cranelift::Cranelift;
use wasmer_middlewares::metering::MeteringPoints;
use wasmer_middlewares::metering::{get_remaining_points, set_remaining_points};
use wasmer_middlewares::Metering;

/// PERF-FIX 2 + P9-CORE-04: Global WASM compiled-module cache with LRU eviction.
/// Stores Cranelift-compiled module bytes keyed by SHA-256 of WASM bytecode.
/// Eliminates redundant 5-50ms Cranelift compilations on every contract call.
/// LRU eviction prevents unbounded memory growth on long-running validators.
const MODULE_CACHE_MAX_ENTRIES: usize = 1024;
static MODULE_CACHE: std::sync::LazyLock<Mutex<lru::LruCache<[u8; 32], Vec<u8>>>> =
    std::sync::LazyLock::new(|| {
        Mutex::new(lru::LruCache::new(
            std::num::NonZeroUsize::new(MODULE_CACHE_MAX_ENTRIES).unwrap(),
        ))
    });

// PERF-FIX 7: Thread-local ContractRuntime pool.
// Avoids creating a new Cranelift compiler + Wasmer Store on every contract call.
// Each rayon thread reuses its own runtime instance, eliminating ~1-5ms overhead per TX.
thread_local! {
    static RUNTIME_POOL: std::cell::RefCell<Option<ContractRuntime>> = const { std::cell::RefCell::new(None) };
}

/// Maximum compute units per contract execution (T1.5)
/// Contracts with 64KB stack buffers (storage_get) + complex init can easily
/// use 2-3M instructions. 10M provides ample headroom for legitimate contracts
/// while still preventing infinite loops.
const MAX_WASM_COMPUTE_UNITS: u64 = 10_000_000;

/// Conversion ratio: how many raw WASM instructions equal 1 CU.
/// WASM instructions are much finer-grained than Solana-model CUs.
/// With DIVISOR=50:  200K CU budget ≈ 10M WASM instructions (the hard limit).
/// A simple contract (~500K WASM instructions) costs ~10K CU.
pub const WASM_CU_DIVISOR: u64 = 50;

/// Maximum WASM memory pages (1 page = 64KB, 1024 pages = 64MB) (T1.9, Task 3.5)
pub const MAX_WASM_MEMORY_PAGES: u32 = 1024;
/// Default minimum WASM memory pages (16 pages = 1MB) (Task 3.5)
/// Contracts with less memory will be grown to this minimum after instantiation.
pub const DEFAULT_WASM_MEMORY_PAGES: u32 = 16;

// ============================================================================
// Contract ABI / IDL Schema
// ============================================================================

/// ABI type for function parameters and return values
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AbiType {
    U8,
    U16,
    U32,
    U64,
    I16,
    I32,
    I64,
    // M12 fix: proper float types instead of mapping to U32/U64
    F32,
    F64,
    Bool,
    String,
    Bytes,
    /// 32-byte public key / address (passed as pointer to 32 bytes)
    #[serde(alias = "Pubkey")]
    Pubkey,
    /// Arbitrary-length byte array with an explicit length param
    #[serde(rename = "bytes_with_len")]
    BytesWithLen,
}

/// Single function parameter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbiParam {
    /// Parameter name
    pub name: String,
    /// Parameter type
    #[serde(rename = "type")]
    pub param_type: AbiType,
    #[serde(default)]
    pub optional: bool,
    /// Human-readable description (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Function return descriptor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbiReturn {
    #[serde(rename = "type")]
    pub return_type: AbiType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Describes a single callable contract function
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbiFunction {
    /// Function name (matches WASM export name exactly)
    pub name: String,
    /// Human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Parameters
    #[serde(default)]
    pub params: Vec<AbiParam>,
    /// Return value (None = void)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<AbiReturn>,
    /// Opcode selector for contracts that expose a single `call()` export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opcode: Option<u8>,
    /// Whether this function only reads state (no writes)
    #[serde(default)]
    pub readonly: bool,
}

/// Event field descriptor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbiEventField {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: AbiType,
    /// Indexed fields can be used for filtering
    #[serde(default)]
    pub indexed: bool,
}

/// Describes a structured event emitted by a contract
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbiEvent {
    /// Event name
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Event fields
    pub fields: Vec<AbiEventField>,
}

/// Custom error descriptor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbiError {
    /// Error code
    pub code: u32,
    /// Error name
    pub name: String,
    /// Human-readable message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Full contract ABI (Application Binary Interface)
/// Machine-readable specification of a contract's public interface
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractAbi {
    /// ABI schema version
    pub version: String,
    /// Contract name
    #[serde(alias = "contract")]
    pub name: String,
    /// Contract template/standard (e.g., "mt20", "mt721", "custom")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Exported callable functions
    pub functions: Vec<AbiFunction>,
    /// Events the contract can emit
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<AbiEvent>,
    /// Known error codes
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<AbiError>,
}

impl ContractAbi {
    /// Extract a minimal ABI from WASM bytecode by inspecting exports.
    /// This gives function names and WASM-level parameter types but no
    /// semantic information (names, descriptions, high-level types).
    pub fn from_wasm(code: &[u8]) -> Option<Self> {
        let store = Store::new(Cranelift::default());
        let module = Module::new(&store, code).ok()?;

        let functions: Vec<AbiFunction> = module
            .exports()
            .filter_map(|export| {
                if let wasmer::ExternType::Function(ft) = export.ty() {
                    let name = export.name().to_string();
                    // Skip WASM internal exports
                    if name.starts_with("__") || name == "memory" {
                        return None;
                    }
                    let params: Vec<AbiParam> = ft
                        .params()
                        .iter()
                        .enumerate()
                        .map(|(i, vt)| AbiParam {
                            name: format!("arg{}", i),
                            param_type: wasm_valtype_to_abi(vt),
                            optional: false,
                            description: None,
                        })
                        .collect();
                    let returns = ft.results().first().map(|vt| AbiReturn {
                        return_type: wasm_valtype_to_abi(vt),
                        description: None,
                    });
                    Some(AbiFunction {
                        name,
                        description: None,
                        params,
                        returns,
                        opcode: None,
                        readonly: false,
                    })
                } else {
                    None
                }
            })
            .collect();

        if functions.is_empty() {
            return None;
        }

        Some(Self {
            version: "1.0".to_string(),
            name: "unknown".to_string(),
            template: None,
            description: None,
            functions,
            events: Vec::new(),
            errors: Vec::new(),
        })
    }
}

fn build_opcode_dispatch_args(opcode: u8, args: &[u8]) -> Vec<u8> {
    let mut dispatch_args = Vec::with_capacity(args.len() + 1);
    dispatch_args.push(opcode);
    dispatch_args.extend_from_slice(args);
    dispatch_args
}

fn find_abi_function<'a>(
    contract: &'a ContractAccount,
    function_name: &str,
) -> Option<&'a AbiFunction> {
    contract.abi.as_ref().and_then(|abi| {
        abi.functions
            .iter()
            .find(|function| function.name == function_name)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeAccountOp {
    Lock {
        account: Pubkey,
        amount: u64,
    },
    Unlock {
        account: Pubkey,
        amount: u64,
    },
    DeductLocked {
        account: Pubkey,
        amount: u64,
    },
    /// Transfer native LICN from one account to another.
    /// Used by contracts (e.g. DEX) to release native LICN to users.
    Transfer {
        from: Pubkey,
        to: Pubkey,
        amount: u64,
    },
}

impl NativeAccountOp {
    pub(crate) fn account(&self) -> Pubkey {
        match self {
            Self::Lock { account, .. }
            | Self::Unlock { account, .. }
            | Self::DeductLocked { account, .. } => *account,
            Self::Transfer { from, .. } => *from,
        }
    }

    /// For Transfer ops, returns the recipient account key.
    pub(crate) fn transfer_to(&self) -> Option<Pubkey> {
        match self {
            Self::Transfer { to, .. } => Some(*to),
            _ => None,
        }
    }

    pub(crate) fn apply(&self, account: &mut Account) -> Result<(), String> {
        match self {
            Self::Lock { amount, .. } => account.lock(*amount),
            Self::Unlock { amount, .. } => account.unlock(*amount),
            Self::DeductLocked { amount, .. } => account.deduct_locked(*amount),
            Self::Transfer { amount, .. } => {
                // Debit the sender — apply() is called on the `from` account
                if account.spendable < *amount {
                    return Err(format!(
                        "Insufficient spendable balance for native transfer: have {}, need {}",
                        account.spendable, amount
                    ));
                }
                account.spores = account.spores.saturating_sub(*amount);
                account.spendable = account.spendable.saturating_sub(*amount);
                Ok(())
            }
        }
    }
}

/// Map WASM ValType to our ABI type system
fn wasm_valtype_to_abi(vt: &wasmer::Type) -> AbiType {
    match vt {
        wasmer::Type::I32 => AbiType::I32,
        wasmer::Type::I64 => AbiType::I64,
        wasmer::Type::F32 => AbiType::F32,
        wasmer::Type::F64 => AbiType::F64,
        _ => AbiType::I32,
    }
}

// ============================================================================
// Contract Account
// ============================================================================

/// Contract account storing bytecode and state
/// AUDIT-FIX 3.5: NOTE — `code` (Vec<u8>) is serialized as a JSON integer array
/// by serde_json, causing ~3-4x storage bloat vs base64 or raw bytes. A migration
/// to base64 encoding (serde_bytes + base64 serializer) is recommended for a future
/// release but requires a storage migration for existing deployed contracts.
/// AUDIT-FIX 3.6: NOTE — WASM modules are compiled from raw bytecode on every
/// `execute()` call. A compiled module cache (keyed by code_hash) would eliminate
/// redundant Cranelift compilations. Deferred to a future optimization pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractAccount {
    /// WASM bytecode
    pub code: Vec<u8>,
    /// Contract state storage (key-value)
    /// Keys are byte arrays from WASM but must serialize as strings for JSON.
    /// We try UTF-8 first (most keys are valid UTF-8 like "admin", "pair:X_Y"),
    /// falling back to hex encoding with a "0x" prefix for binary keys.
    #[serde(
        serialize_with = "serialize_byte_map",
        deserialize_with = "deserialize_byte_map"
    )]
    pub storage: HashMap<Vec<u8>, Vec<u8>>,
    /// Owner who deployed the contract
    pub owner: Pubkey,
    /// Code hash for verification
    pub code_hash: Hash,
    /// Machine-readable ABI (optional, set at deploy or updated later)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abi: Option<ContractAbi>,
    /// Contract version — starts at 1, incremented on each upgrade
    #[serde(default = "default_version")]
    pub version: u32,
    /// Code hash of the previous version (for rollback reference)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_code_hash: Option<Hash>,
    /// Optional upgrade timelock: number of epochs that must elapse between
    /// submitting an upgrade and executing it. `None` = instant upgrades (legacy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade_timelock_epochs: Option<u32>,
    /// Staged upgrade awaiting timelock expiry. Set when an upgrade is submitted
    /// on a timelocked contract; cleared on execute or veto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_upgrade: Option<PendingUpgrade>,
}

/// A staged contract upgrade waiting for the timelock to expire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingUpgrade {
    /// New WASM bytecode
    pub code: Vec<u8>,
    /// SHA-256 hash of the new code (pre-validated at submission time)
    pub code_hash: Hash,
    /// Epoch when the upgrade was submitted
    pub submitted_epoch: u64,
    /// Epoch at which the upgrade becomes executable
    pub execute_after_epoch: u64,
}

fn default_version() -> u32 {
    1
}

/// Serialize HashMap<Vec<u8>, Vec<u8>> as a JSON object with string keys.
/// Keys that are valid UTF-8 are stored as-is; binary keys get hex-encoded with "0x" prefix.
fn serialize_byte_map<S>(map: &HashMap<Vec<u8>, Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    let mut ser_map = serializer.serialize_map(Some(map.len()))?;
    for (key, value) in map {
        let key_str = match std::str::from_utf8(key) {
            Ok(s) if !s.starts_with("0x") => s.to_string(),
            _ => format!("0x{}", hex::encode(key)),
        };
        ser_map.serialize_entry(&key_str, value)?;
    }
    ser_map.end()
}

/// Deserialize a JSON object with string keys back into HashMap<Vec<u8>, Vec<u8>>.
/// Keys prefixed with "0x" are hex-decoded; all others are treated as raw UTF-8 bytes.
fn deserialize_byte_map<'de, D>(deserializer: D) -> Result<HashMap<Vec<u8>, Vec<u8>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let str_map: HashMap<String, Vec<u8>> = HashMap::deserialize(deserializer)?;
    let mut result = HashMap::with_capacity(str_map.len());
    for (key_str, value) in str_map {
        let key_bytes = if let Some(hex_part) = key_str.strip_prefix("0x") {
            hex::decode(hex_part).map_err(serde::de::Error::custom)?
        } else {
            key_str.into_bytes()
        };
        result.insert(key_bytes, value);
    }
    Ok(result)
}

impl ContractAccount {
    /// Create new contract account
    pub fn new(code: Vec<u8>, owner: Pubkey) -> Self {
        let code_hash = Hash::hash(&code);
        // Try to auto-extract ABI from WASM exports
        let abi = ContractAbi::from_wasm(&code);
        Self {
            code,
            storage: HashMap::new(),
            owner,
            code_hash,
            abi,
            version: 1,
            previous_code_hash: None,
            upgrade_timelock_epochs: None,
            pending_upgrade: None,
        }
    }

    /// Get value from contract storage
    pub fn get_storage(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.storage.get(key).cloned()
    }

    /// Set value in contract storage
    pub fn set_storage(&mut self, key: Vec<u8>, value: Vec<u8>) {
        self.storage.insert(key, value);
    }

    /// Remove value from contract storage
    pub fn remove_storage(&mut self, key: &[u8]) -> Option<Vec<u8>> {
        self.storage.remove(key)
    }

    /// Get contract size in bytes
    pub fn size(&self) -> usize {
        self.code.len()
            + self
                .storage
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum::<usize>()
    }
}

/// Structured event emitted by a contract (indexed by the chain)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractEvent {
    /// Emitting contract address
    pub program: Pubkey,
    /// Event name / topic (e.g. "Transfer", "Mint", "Approve")
    pub name: String,
    /// Structured fields as key-value pairs (JSON-serialized in the contract)
    pub data: HashMap<String, String>,
    /// Slot in which the event was emitted
    pub slot: u64,
}

/// Contract execution context — shared with WASM host functions
#[derive(Clone)]
pub struct ContractContext {
    /// Caller address
    pub caller: Pubkey,
    /// Contract address
    pub contract: Pubkey,
    /// Value transferred (in spores)
    pub value: u64,
    /// Block slot (used for deterministic timestamp)
    pub slot: u64,
    /// Live storage state (initially loaded from ContractAccount, mutated by host fns)
    pub storage: HashMap<Vec<u8>, Vec<u8>>,
    /// Logs emitted by contract (free-form text)
    pub logs: Vec<String>,
    /// Structured events emitted by contract
    pub events: Vec<ContractEvent>,
    /// Tracked storage changes: key → Some(value) for writes, None for deletes
    pub storage_changes: HashMap<Vec<u8>, Option<Vec<u8>>>,
    /// Last value read by storage_read (retrieved via storage_read_result)
    pub last_read_value: Vec<u8>,
    /// WASM linear memory handle (set after instantiation)
    pub memory: Option<Memory>,
    /// Function arguments passed by the caller
    pub args: Vec<u8>,
    /// Return data set by the contract
    pub return_data: Vec<u8>,
    /// Remaining compute units (fuel). 0 = exhausted.
    pub compute_remaining: u64,
    /// Transaction-level compute limit for this execution.
    /// Defaults to `DEFAULT_COMPUTE_LIMIT` (10M). When called from the
    /// transaction processor, set to the TX's remaining CU budget so that
    /// WASM fuel metering respects the user-declared budget.
    pub compute_limit: u64,
    /// Cross-contract storage entries injected by the processor.
    /// Merged into `storage` at execution time so contracts can read other
    /// contracts' data (e.g., LichenID reputation) via normal `storage_read`.
    /// NOT tracked in `storage_changes`, NOT persisted to the contract's DB.
    pub cross_contract_storage: HashMap<Vec<u8>, Vec<u8>>,
    /// Shared reference to the state store for cross-contract calls.
    /// Only present when running within the processor (not in standalone tests).
    pub state_store: Option<StateStore>,
    /// Current call depth (0 = top-level, incremented for each nested CCC).
    /// Prevents infinite recursion — capped at MAX_CROSS_CALL_DEPTH.
    pub call_depth: u32,
    /// Accumulated storage changes from cross-contract calls, keyed by contract
    /// address. Shared via Arc<Mutex<>> so nested calls all contribute to the
    /// same collection. Applied atomically by the processor after execution.
    pub pending_ccc_changes: Arc<Mutex<CccChanges>>,
    /// Events collected from cross-contract sub-calls.
    pub pending_ccc_events: Arc<Mutex<Vec<ContractEvent>>>,
    /// Logs collected from cross-contract sub-calls.
    pub pending_ccc_logs: Arc<Mutex<Vec<String>>>,
    /// AUDIT-FIX C-2: Accumulated value transfer deltas from cross-contract calls.
    /// Positive = credit, negative = debit. Applied atomically through the
    /// StateBatch by the processor after execution, preventing the split-brain
    /// between direct state_store writes and the batch overlay.
    pub pending_ccc_value_deltas: Arc<Mutex<HashMap<Pubkey, i64>>>,
    /// Native account balance operations requested through the zero-address
    /// host call surface. These are applied atomically by the processor.
    pub pending_native_account_ops: Vec<NativeAccountOp>,
    /// Projected account state after applying `pending_native_account_ops`.
    pub pending_native_account_state: HashMap<Pubkey, Account>,
    /// Current total storage bytes (keys + values). Tracked live during execution
    /// for protocol-level enforcement of MAX_TOTAL_STORAGE_BYTES (Task 4.3 M-4).
    pub storage_bytes_used: usize,
}

impl ContractContext {
    pub fn new(caller: Pubkey, contract: Pubkey, value: u64, slot: u64) -> Self {
        Self {
            caller,
            contract,
            value,
            slot,
            storage: HashMap::new(),
            logs: Vec::new(),
            events: Vec::new(),
            storage_changes: HashMap::new(),
            last_read_value: Vec::new(),
            memory: None,
            args: Vec::new(),
            return_data: Vec::new(),
            compute_remaining: DEFAULT_COMPUTE_LIMIT,
            compute_limit: DEFAULT_COMPUTE_LIMIT,
            cross_contract_storage: HashMap::new(),
            state_store: None,
            call_depth: 0,
            pending_ccc_changes: Arc::new(Mutex::new(HashMap::new())),
            pending_ccc_events: Arc::new(Mutex::new(Vec::new())),
            pending_ccc_logs: Arc::new(Mutex::new(Vec::new())),
            pending_ccc_value_deltas: Arc::new(Mutex::new(HashMap::new())),
            pending_native_account_ops: Vec::new(),
            pending_native_account_state: HashMap::new(),
            storage_bytes_used: 0,
        }
    }

    /// Create context pre-loaded with contract's existing storage
    pub fn with_storage(
        caller: Pubkey,
        contract: Pubkey,
        value: u64,
        slot: u64,
        storage: HashMap<Vec<u8>, Vec<u8>>,
    ) -> Self {
        let storage_bytes_used = storage.iter().map(|(k, v)| k.len() + v.len()).sum();
        Self {
            caller,
            contract,
            value,
            slot,
            storage,
            logs: Vec::new(),
            events: Vec::new(),
            storage_changes: HashMap::new(),
            last_read_value: Vec::new(),
            memory: None,
            args: Vec::new(),
            return_data: Vec::new(),
            compute_remaining: DEFAULT_COMPUTE_LIMIT,
            compute_limit: DEFAULT_COMPUTE_LIMIT,
            cross_contract_storage: HashMap::new(),
            state_store: None,
            call_depth: 0,
            pending_ccc_changes: Arc::new(Mutex::new(HashMap::new())),
            pending_ccc_events: Arc::new(Mutex::new(Vec::new())),
            pending_ccc_logs: Arc::new(Mutex::new(Vec::new())),
            pending_ccc_value_deltas: Arc::new(Mutex::new(HashMap::new())),
            pending_native_account_ops: Vec::new(),
            pending_native_account_state: HashMap::new(),
            storage_bytes_used,
        }
    }

    /// Create context with args and storage
    pub fn with_args(
        caller: Pubkey,
        contract: Pubkey,
        value: u64,
        slot: u64,
        storage: HashMap<Vec<u8>, Vec<u8>>,
        args: Vec<u8>,
    ) -> Self {
        let storage_bytes_used = storage.iter().map(|(k, v)| k.len() + v.len()).sum();
        Self {
            caller,
            contract,
            value,
            slot,
            storage,
            logs: Vec::new(),
            events: Vec::new(),
            storage_changes: HashMap::new(),
            last_read_value: Vec::new(),
            memory: None,
            args,
            return_data: Vec::new(),
            compute_remaining: DEFAULT_COMPUTE_LIMIT,
            compute_limit: DEFAULT_COMPUTE_LIMIT,
            cross_contract_storage: HashMap::new(),
            state_store: None,
            call_depth: 0,
            pending_ccc_changes: Arc::new(Mutex::new(HashMap::new())),
            pending_ccc_events: Arc::new(Mutex::new(Vec::new())),
            pending_ccc_logs: Arc::new(Mutex::new(Vec::new())),
            pending_ccc_value_deltas: Arc::new(Mutex::new(HashMap::new())),
            pending_native_account_ops: Vec::new(),
            pending_native_account_state: HashMap::new(),
            storage_bytes_used,
        }
    }
}

/// Contract execution result
#[derive(Debug, Clone)]
pub struct ContractResult {
    /// Return data from contract
    pub return_data: Vec<u8>,
    /// Logs emitted (free-form text)
    pub logs: Vec<String>,
    /// Structured events emitted
    pub events: Vec<ContractEvent>,
    /// Storage changes: key → Some(new_value) for writes, None for deletes
    pub storage_changes: HashMap<Vec<u8>, Option<Vec<u8>>>,
    /// Success or error message
    pub success: bool,
    pub error: Option<String>,
    /// Compute units consumed
    pub compute_used: u64,
    /// WASM function return code (first I32/I64 return value), if any.
    /// Informational — contracts use inconsistent conventions:
    /// some return 0=success, others return 1=success. Callers can
    /// inspect this to implement contract-specific error handling.
    /// Widened to i64 so that u64-returning functions (balance_of,
    /// total_supply) are captured without silent truncation.
    pub return_code: Option<i64>,
    /// Accumulated storage changes from cross-contract sub-calls, keyed by
    /// target contract address. Applied by the processor alongside the
    /// top-level contract's own storage_changes.
    pub cross_call_changes: HashMap<Pubkey, HashMap<Vec<u8>, Option<Vec<u8>>>>,
    /// Events emitted by cross-contract sub-calls.
    pub cross_call_events: Vec<ContractEvent>,
    /// Logs emitted by cross-contract sub-calls.
    pub cross_call_logs: Vec<String>,
    /// AUDIT-FIX C-2: Accumulated value transfer deltas from cross-contract calls.
    /// Applied atomically through the StateBatch by the processor.
    pub ccc_value_deltas: HashMap<Pubkey, i64>,
    /// Native account balance operations produced during execution.
    pub native_account_ops: Vec<NativeAccountOp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramCallActivity {
    pub slot: u64,
    pub timestamp: u64,
    pub program: Pubkey,
    pub caller: Pubkey,
    pub function: String,
    pub value: u64,
    pub tx_signature: Hash,
}

pub fn encode_program_call_activity(activity: &ProgramCallActivity) -> Result<Vec<u8>, String> {
    bincode::serialize(activity).map_err(|e| format!("Failed to encode program call: {}", e))
}

pub fn decode_program_call_activity(data: &[u8]) -> Result<ProgramCallActivity, String> {
    bincode::deserialize(data).map_err(|e| format!("Failed to decode program call: {}", e))
}

/// Maximum log message length (16 KB)
const MAX_LOG_LEN: usize = 16_384;
/// P9-CORE-06: Maximum number of log entries per contract execution
const MAX_LOG_ENTRIES: usize = 1024;
/// Maximum storage key length (256 bytes)
const MAX_KEY_LEN: usize = 256;
/// Maximum storage value length (64 KB)
const MAX_VALUE_LEN: usize = 65_536;
/// Maximum return data from a contract call (64 KB)
const MAX_RETURN_DATA: usize = 65_536;
/// Maximum event data JSON size (8 KB)
const MAX_EVENT_DATA: usize = 8_192;
/// Default compute limit per contract call (10 million units)
pub const DEFAULT_COMPUTE_LIMIT: u64 = 10_000_000;
/// Compute cost for a storage read
const COMPUTE_STORAGE_READ: u64 = 100;
/// Compute cost for a storage write (base, plus per-byte cost)
const COMPUTE_STORAGE_WRITE: u64 = 200;
/// Per-byte compute cost for storage writes (Task 4.3 M-4 protocol enforcement)
const COMPUTE_STORAGE_WRITE_PER_BYTE: u64 = 1;
/// Maximum total storage bytes per contract (10 MB). Enforced at the host
/// function level to prevent unlimited state growth (Task 4.3 M-4).
const MAX_TOTAL_STORAGE_BYTES: usize = 10 * 1024 * 1024;
/// Compute cost for a storage delete
const COMPUTE_STORAGE_DELETE: u64 = 100;
/// Compute cost for emitting a log
const COMPUTE_LOG: u64 = 10;
/// Compute cost for emitting an event
const COMPUTE_EVENT: u64 = 50;
// AUDIT-FIX 2.1: Additional compute constants for previously uncharged host functions
const COMPUTE_GET_CALLER: u64 = 100;
const COMPUTE_GET_ARGS: u64 = 50; // + per-byte cost
const COMPUTE_SET_RETURN_DATA: u64 = 50; // + per-byte cost
const COMPUTE_READ_RESULT: u64 = 50; // + per-byte cost
const COMPUTE_BYTE_COST: u64 = 1;
/// Compute cost for initiating a cross-contract call (base cost before callee's compute)
const COMPUTE_CROSS_CALL: u64 = 5_000;
/// Compute cost for Poseidon hash (SNARK-friendly, more expensive than plain hash)
const COMPUTE_POSEIDON_HASH: u64 = 2_000;
/// Maximum cross-contract call depth (prevents infinite recursion)
const MAX_CROSS_CALL_DEPTH: u32 = 8;
/// Maximum function name length for cross-contract calls
const MAX_CCC_FUNCTION_LEN: u32 = 256;
/// Maximum args length for cross-contract calls (64 KB)
const MAX_CCC_ARGS_LEN: u32 = 65_536;

/// Contract runtime - executes WASM bytecode with compute metering
///
/// # Security Sandbox (T2.4)
///
/// The WASM runtime is sandboxed with the following security measures:
///
/// 1. **Compute Metering**: Every WASM instruction costs 1 compute unit.
///    Execution traps after `MAX_WASM_COMPUTE_UNITS` (10M) units, preventing
///    infinite loops and DoS via compute exhaustion.
///
/// 2. **Memory Limits**: WASM linear memory is capped at `MAX_WASM_MEMORY_PAGES`
///    (1024 pages = 64MB). Contracts declaring or growing memory beyond this
///    limit are rejected at both deploy-time and post-execution. Contracts with
///    less than `DEFAULT_WASM_MEMORY_PAGES` (16 pages = 1MB) are grown to the
///    minimum after instantiation.
///
/// 3. **No WASI**: The runtime does NOT enable WASI. Contracts have zero access
///    to the host filesystem, network, environment variables, or system calls.
///    WASI imports are explicitly rejected at deploy time.
///
/// 4. **Explicit Imports Only**: Contracts may only import from the `"env"` module.
///    All host functions are explicitly defined and audited:
///    - Storage: read, write, delete (scoped to contract's own storage)
///    - Logging: log messages and structured events
///    - Chain introspection: timestamp, caller, value, slot (read-only)
///    - Args/returns: get_args, set_return_data
///    - Cross-contract calls: synchronous dispatch via call_contract (non-reentrant)
///
/// 5. **Deploy-time Validation**: Bytecode is validated at deploy to reject
///    modules with excessive memory declarations, unauthorized import modules,
///    or WASI capabilities.
pub struct ContractRuntime {
    store: Store,
}

impl Default for ContractRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl ContractRuntime {
    /// Create new contract runtime with WASM compute metering (T1.5).
    /// Every WASM instruction costs 1 compute unit.
    /// Execution traps when compute budget is exhausted — prevents infinite loops.
    pub fn new() -> Self {
        let metering = std::sync::Arc::new(Metering::new(MAX_WASM_COMPUTE_UNITS, |_| 1));
        let mut compiler = Cranelift::default();
        compiler.push_middleware(metering);
        let store = Store::new(compiler);
        Self { store }
    }

    /// PERF-FIX 7: Get a runtime from the thread-local pool, or create one if empty.
    /// This avoids constructing a new Cranelift compiler + Store on every contract call,
    /// saving ~1-5ms per invocation. The runtime is returned to the pool after use.
    pub fn get_pooled() -> Self {
        RUNTIME_POOL.with(|cell| cell.borrow_mut().take().unwrap_or_else(Self::new))
    }

    /// PERF-FIX 7: Return a runtime to the thread-local pool for reuse.
    pub fn return_to_pool(self) {
        RUNTIME_POOL.with(|cell| {
            *cell.borrow_mut() = Some(self);
        });
    }

    /// Deploy contract — validate bytecode and enforce sandbox constraints (T2.4).
    ///
    /// Security checks performed:
    /// - Rejects WASI imports (no filesystem/network/syscall access)
    /// - Rejects imports from unauthorized modules (only `"env"` allowed)
    /// - Rejects memory declarations exceeding `MAX_WASM_MEMORY_PAGES` (64MB)
    pub fn deploy(&mut self, bytecode: &[u8]) -> Result<Hash, String> {
        let module = Module::new(&self.store, bytecode)
            .map_err(|e| format!("Invalid WASM bytecode: {}", e))?;

        // T2.4: Validate imports — only "env" module allowed, no WASI
        for import in module.imports() {
            let module_name = import.module();
            if module_name == "wasi_snapshot_preview1" || module_name == "wasi_unstable" {
                return Err(
                    "WASI imports are forbidden — contracts cannot access host filesystem or network"
                        .to_string(),
                );
            }
            if module_name != "env" {
                return Err(format!(
                    "Unauthorized import module '{}'. Only 'env' imports are allowed.",
                    module_name
                ));
            }
        }

        // T2.4: Validate exported memory declarations don't exceed sandbox limits
        for export in module.exports() {
            if let wasmer::ExternType::Memory(mem_type) = export.ty() {
                if mem_type.minimum.0 > MAX_WASM_MEMORY_PAGES {
                    return Err(format!(
                        "Contract initial memory ({} pages) exceeds limit ({} pages = {}MB)",
                        mem_type.minimum.0,
                        MAX_WASM_MEMORY_PAGES,
                        MAX_WASM_MEMORY_PAGES as u64 * 64 / 1024
                    ));
                }
                if let Some(max_pages) = mem_type.maximum {
                    if max_pages.0 > MAX_WASM_MEMORY_PAGES {
                        return Err(format!(
                            "Contract max memory ({} pages) exceeds limit ({} pages = {}MB)",
                            max_pages.0,
                            MAX_WASM_MEMORY_PAGES,
                            MAX_WASM_MEMORY_PAGES as u64 * 64 / 1024
                        ));
                    }
                }
            }
        }

        Ok(Hash::hash(bytecode))
    }

    /// Execute contract function
    pub fn execute(
        &mut self,
        contract: &ContractAccount,
        function_name: &str,
        args: &[u8],
        context: ContractContext,
    ) -> Result<ContractResult, String> {
        let ctx = Self::prepare_execution_context(context, args);
        let initial_compute = ctx.compute_remaining;
        // Capture the TX-level compute limit. When called from the processor,
        // this is the TX's remaining budget. For standalone/test calls it
        // defaults to DEFAULT_COMPUTE_LIMIT (10M).
        let compute_limit = ctx.compute_limit;

        // PERF-FIX 2 + P9-CORE-04: Compiled-module cache with LRU eviction.
        // Cranelift compilation takes 5-50ms per module. With 28 contracts and
        // thousands of calls, this eliminates >99% of redundant compilations.
        // LRU cap at MODULE_CACHE_MAX_ENTRIES prevents unbounded memory growth.
        let code_hash = Hash::hash(&contract.code);
        let module = {
            let mut cache = MODULE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cached_bytes) = cache.get(&code_hash.0) {
                // Hot path: deserialize pre-compiled module (~0.5ms)
                // SAFETY: We serialized these bytes ourselves from a valid Module.
                // The Store uses the same Cranelift + metering config every time.
                unsafe { Module::deserialize(&self.store, cached_bytes) }
                    .map_err(|e| format!("Failed to deserialize cached module: {}", e))?
            } else {
                drop(cache);
                // Cold path: compile from bytecode + cache for next time
                let m = Module::new(&self.store, &contract.code)
                    .map_err(|e| format!("Failed to compile contract: {}", e))?;
                if let Ok(serialized) = m.serialize() {
                    let mut cache_w = MODULE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
                    // put() returns the evicted entry (if any) when cache is full
                    cache_w.put(code_hash.0, serialized.to_vec());
                }
                m
            }
        };

        let env = FunctionEnv::new(&mut self.store, ctx);

        let imports = imports! {
            "env" => {
                // Storage (4-param read matches SDK FFI, 2-param kept for backward compat)
                "storage_read" => Function::new_typed_with_env(&mut self.store, &env, host_storage_read),
                "storage_read_result" => Function::new_typed_with_env(&mut self.store, &env, host_storage_read_result),
                "storage_write" => Function::new_typed_with_env(&mut self.store, &env, host_storage_write),
                "storage_delete" => Function::new_typed_with_env(&mut self.store, &env, host_storage_delete),
                // Logging & events
                "log" => Function::new_typed_with_env(&mut self.store, &env, host_log_msg),
                "emit_event" => Function::new_typed_with_env(&mut self.store, &env, host_emit_event),
                // Chain introspection
                "get_timestamp" => Function::new_typed_with_env(&mut self.store, &env, host_get_timestamp),
                "get_caller" => Function::new_typed_with_env(&mut self.store, &env, host_get_caller),
                "get_contract_address" => Function::new_typed_with_env(&mut self.store, &env, host_get_contract_address),
                "get_value" => Function::new_typed_with_env(&mut self.store, &env, host_get_value),
                "get_slot" => Function::new_typed_with_env(&mut self.store, &env, host_get_slot),
                // Args & return data
                "get_args_len" => Function::new_typed_with_env(&mut self.store, &env, host_get_args_len),
                "get_args" => Function::new_typed_with_env(&mut self.store, &env, host_get_args),
                "set_return_data" => Function::new_typed_with_env(&mut self.store, &env, host_set_return_data),
                // Cross-contract calls
                "cross_contract_call" => Function::new_typed_with_env(&mut self.store, &env, host_cross_contract_call),
                // Cryptographic functions
                "host_poseidon_hash" => Function::new_typed_with_env(&mut self.store, &env, host_poseidon_hash),
            }
        };

        let instance = Instance::new(&mut self.store, &module, &imports)
            .map_err(|e| format!("Failed to instantiate contract: {}", e))?;

        // Set WASM fuel to the hard runtime ceiling.  The actual CU charge
        // is post-computed by dividing raw WASM instructions by WASM_CU_DIVISOR,
        // then checked against the TX-level compute_limit (Solana model).
        let wasm_fuel_limit = MAX_WASM_COMPUTE_UNITS;
        set_remaining_points(&mut self.store, &instance, wasm_fuel_limit);

        // Bind WASM linear memory to context for host function access
        if let Ok(memory) = instance.exports.get_memory("memory") {
            // T1.9: Enforce memory limit — reject contracts that declare too much memory
            let current_pages = memory.view(&self.store).size().0;
            if current_pages > MAX_WASM_MEMORY_PAGES {
                return Err(format!(
                    "Contract memory exceeds limit: {} pages > {} max",
                    current_pages, MAX_WASM_MEMORY_PAGES
                ));
            }
            // Task 3.5: Ensure minimum memory — grow to DEFAULT_WASM_MEMORY_PAGES
            // if the contract declares less. This guarantees 1MB working memory
            // for all contracts regardless of their declared initial pages.
            if current_pages < DEFAULT_WASM_MEMORY_PAGES {
                let grow_by = DEFAULT_WASM_MEMORY_PAGES - current_pages;
                let _ = memory.grow(&mut self.store, grow_by);
            }
            env.as_mut(&mut self.store).memory = Some(memory.clone());
        }

        let (func, effective_args) = match instance.exports.get_function(function_name) {
            Ok(function) => (function, args.to_vec()),
            Err(named_error) => {
                let abi_function = find_abi_function(contract, function_name).ok_or_else(|| {
                    format!("Function '{}' not found: {}", function_name, named_error)
                })?;
                let opcode = abi_function.opcode.ok_or_else(|| {
                    format!(
                        "Function '{}' is not exported and has no opcode selector in ABI",
                        function_name
                    )
                })?;
                let fallback = instance
                    .exports
                    .get_function("call")
                    .map_err(|call_error| {
                        format!(
                            "Function '{}' not found: {}. ABI selector fallback also failed: {}",
                            function_name, named_error, call_error
                        )
                    })?;
                (fallback, build_opcode_dispatch_args(opcode, args))
            }
        };

        // Build WASM-level call arguments by introspecting the function's type
        // signature. Contracts use two ABIs:
        //   (a) Named-export ABI: fn initialize(ptr: *const u8) — I32 params are
        //       pointers into linear memory (32-byte pubkeys); I64 params are raw
        //       u64 values (amounts, thresholds).
        //   (b) Opcode ABI: fn call() — zero WASM params; args read via get_args()
        //       host import.
        // This block handles both transparently.
        let func_type = func.ty(&self.store);
        let params: Vec<Type> = func_type.params().to_vec();
        let call_args: Vec<Value> = if params.is_empty() || effective_args.is_empty() {
            vec![]
        } else {
            // Grow WASM memory by 1 page (64KB) to get a safe buffer area for
            // writing the function arguments. This avoids corrupting the module's
            // stack/heap/data sections.
            let memory = instance
                .exports
                .get_memory("memory")
                .map_err(|e| format!("Contract has no memory export: {}", e))?;
            let old_pages = memory
                .grow(&mut self.store, 1)
                .map_err(|e| format!("Failed to grow WASM memory for args: {}", e))?;
            let args_base: u32 = old_pages.0 * 65536; // byte offset of the new page

            // ── ABI-aware JSON arg encoding ─────────────────────────────
            // When the CLI sends JSON-encoded args (e.g. ["addr", 1, "name", 21]),
            // auto-encode them to binary with a layout descriptor so the WASM
            // function receives correctly laid-out memory (base58 → 32 bytes,
            // strings → pointer data, integers → raw bytes).
            let encoded_args = if !effective_args.is_empty()
                && effective_args[0] == b'['
                && !params.is_empty()
                && effective_args[0] != 0xAB
                && std::str::from_utf8(&effective_args).is_ok()
            {
                if let Ok(json_vals) =
                    serde_json::from_slice::<Vec<serde_json::Value>>(&effective_args)
                {
                    encode_json_args_to_binary(&json_vals, &params)
                        .unwrap_or_else(|_| effective_args.clone())
                } else {
                    effective_args.clone()
                }
            } else {
                effective_args.clone()
            };
            let args = &encoded_args;

            let view = memory.view(&self.store);
            view.write(args_base as u64, args)
                .map_err(|e| format!("Failed to write args to WASM memory: {}", e))?;

            // ABI convention for named-export functions:
            //
            // DEFAULT MODE (backward-compatible):
            //   I32 → pointer to a 32-byte address/pubkey (advance 32 bytes)
            //   I64 → raw u64 value (advance 8 bytes, little-endian)
            //
            // LAYOUT DESCRIPTOR MODE (for mixed pointer/integer I32 params):
            //   If args[0] == 0xAB, bytes 1..1+N are a layout descriptor where
            //   N = number of params. Each byte specifies the data size:
            //     32 (0x20) = pointer — advance 32 bytes, pass memory pointer
            //      4 (0x04) = u32 integer — read 4 LE bytes, pass raw i32
            //      1 (0x01) = u8/bool — read 1 byte, pass raw i32
            //      2 (0x02) = u16/i16 — read 2 LE bytes, pass raw i32
            //      8 (0x08) = u64 via I32 — read 8 LE bytes (unusual, for compatibility)
            //   The actual arg data starts at offset 1 + N.
            //
            // This allows callers to correctly encode args for functions with
            // mixed pointer and plain-integer I32 parameters (e.g. lichendao's
            // create_proposal which takes both *const u8 and u32 lengths).
            let has_layout = !args.is_empty() && args[0] == 0xAB && args.len() > params.len();
            let layout: Vec<u8> = if has_layout {
                args[1..1 + params.len()].to_vec()
            } else {
                Vec::new()
            };
            let data_start: u32 = if has_layout {
                (1 + params.len()) as u32
            } else {
                0
            };

            // Re-write only the data portion into WASM memory if using layout mode
            if has_layout {
                let data_slice = &args[data_start as usize..];
                let view2 = memory.view(&self.store);
                view2
                    .write(args_base as u64, data_slice)
                    .map_err(|e| format!("Failed to write args data to WASM memory: {}", e))?;
            }

            let mut wasm_args = Vec::with_capacity(params.len());
            let mut byte_offset: u32 = 0;
            for (idx, param) in params.iter().enumerate() {
                if has_layout {
                    // Layout descriptor mode: stride determined by descriptor byte
                    let stride = layout.get(idx).copied().unwrap_or(32) as u32;
                    match param {
                        Type::I32 => {
                            if stride >= 32 {
                                // Pointer — pass memory address
                                wasm_args.push(Value::I32((args_base + byte_offset) as i32));
                                byte_offset += stride;
                            } else {
                                // Plain integer — read raw bytes from args data
                                let data = &args[data_start as usize..];
                                let off = byte_offset as usize;
                                let val: i32 = match stride {
                                    4 => {
                                        if off + 4 <= data.len() {
                                            i32::from_le_bytes([
                                                data[off],
                                                data[off + 1],
                                                data[off + 2],
                                                data[off + 3],
                                            ])
                                        } else {
                                            0
                                        }
                                    }
                                    2 => {
                                        if off + 2 <= data.len() {
                                            i16::from_le_bytes([data[off], data[off + 1]]) as i32
                                        } else {
                                            0
                                        }
                                    }
                                    1 => {
                                        if off < data.len() {
                                            data[off] as i32
                                        } else {
                                            0
                                        }
                                    }
                                    _ => 0,
                                };
                                wasm_args.push(Value::I32(val));
                                byte_offset += stride;
                            }
                        }
                        Type::I64 => {
                            let data = &args[data_start as usize..];
                            let start = byte_offset as usize;
                            let end = (start + 8).min(data.len());
                            let val = if end <= data.len() && end > start {
                                let mut buf = [0u8; 8];
                                buf[..end - start].copy_from_slice(&data[start..end]);
                                u64::from_le_bytes(buf)
                            } else {
                                0
                            };
                            wasm_args.push(Value::I64(val as i64));
                            byte_offset += 8;
                        }
                        _ => {
                            wasm_args.push(Value::I32(0));
                        }
                    }
                } else {
                    // Default mode: I32 = 32-byte pointer, I64 = 8-byte value
                    match param {
                        Type::I32 => {
                            wasm_args.push(Value::I32((args_base + byte_offset) as i32));
                            byte_offset += 32;
                        }
                        Type::I64 => {
                            let start = byte_offset as usize;
                            let end = (start + 8).min(args.len());
                            let val = if end <= args.len() && end > start {
                                let mut buf = [0u8; 8];
                                buf[..end - start].copy_from_slice(&args[start..end]);
                                u64::from_le_bytes(buf)
                            } else {
                                0
                            };
                            wasm_args.push(Value::I64(val as i64));
                            byte_offset += 8;
                        }
                        _ => {
                            wasm_args.push(Value::I32(0)); // fallback
                        }
                    }
                }
            }
            wasm_args
        };

        let exec_result = func.call(&mut self.store, &call_args);

        // T1.5: Check remaining metering points after execution.
        // If exhausted, the execution already trapped, but we report it clearly.
        let metering_remaining = match get_remaining_points(&mut self.store, &instance) {
            MeteringPoints::Remaining(pts) => pts,
            MeteringPoints::Exhausted => 0,
        };
        // Convert raw WASM instructions to CU using the divisor.
        // With WASM_CU_DIVISOR=50: 10M instructions → 200K CU.
        let raw_wasm_instructions = wasm_fuel_limit.saturating_sub(metering_remaining);
        let wasm_compute_used = raw_wasm_instructions / WASM_CU_DIVISOR;

        // T2.4: Post-execution memory growth check — enforce sandbox limits.
        // Catches contracts that call memory.grow() during execution.
        if let Ok(memory) = instance.exports.get_memory("memory") {
            let final_pages = memory.view(&self.store).size().0;
            if final_pages > MAX_WASM_MEMORY_PAGES {
                let ctx = env.as_ref(&self.store);
                let host_cost = initial_compute.saturating_sub(ctx.compute_remaining);
                return Ok(ContractResult {
                    return_data: vec![],
                    logs: ctx.logs.clone(),
                    events: Vec::new(),
                    storage_changes: HashMap::new(),
                    success: false,
                    error: Some(format!(
                        "Contract exceeded memory limit during execution: {} pages > {} max",
                        final_pages, MAX_WASM_MEMORY_PAGES
                    )),
                    compute_used: host_cost.saturating_add(wasm_compute_used),
                    return_code: None,
                    cross_call_changes: HashMap::new(),
                    cross_call_events: Vec::new(),
                    cross_call_logs: Vec::new(),
                    ccc_value_deltas: HashMap::new(),
                    native_account_ops: Vec::new(),
                });
            }
        }

        let final_ctx = env.as_ref(&self.store);
        // Total compute: host function costs + WASM instruction costs
        let host_compute_used = initial_compute.saturating_sub(final_ctx.compute_remaining);
        let compute_used = host_compute_used.saturating_add(wasm_compute_used);

        // AUDIT-FIX 2.3: Unified compute budget — total (WASM + host) must not exceed the limit.
        // The limit is now driven by the transaction-level budget (compute_limit)
        // rather than the hard DEFAULT_COMPUTE_LIMIT, aligning WASM metering with
        // the user-declared CU budget (Solana model).
        if compute_used > compute_limit {
            return Ok(ContractResult {
                return_data: vec![],
                logs: final_ctx.logs.clone(),
                events: Vec::new(),
                storage_changes: HashMap::new(),
                success: false,
                error: Some(format!(
                    "Contract exceeded compute budget: {} > {} (WASM: {}, host: {})",
                    compute_used, compute_limit, wasm_compute_used, host_compute_used
                )),
                compute_used,
                return_code: None,
                cross_call_changes: HashMap::new(),
                cross_call_events: Vec::new(),
                cross_call_logs: Vec::new(),
                ccc_value_deltas: HashMap::new(),
                native_account_ops: Vec::new(),
            });
        }

        match exec_result {
            Ok(values) => {
                // Extract the WASM function's return code for informational
                // purposes.  Contracts use inconsistent conventions — some
                // return 0=success (lusd_token, lichenid), others return
                // 1=success (lichenoracle queries, lichenpunks), and some return
                // meaningful i64 values (swap outputs, balances).  We record
                // the code but do NOT use it to override success/failure:
                // the JSON arg encoding fix ensures args arrive correctly,
                // and a WASM trap is the only true execution failure.
                let ret_code = values.first().and_then(|v| match v {
                    Value::I32(n) => Some(*n as i64),
                    Value::I64(n) => Some(*n),
                    _ => None,
                });

                // Extract accumulated cross-contract call state
                let ccc_changes = final_ctx
                    .pending_ccc_changes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                let ccc_events = final_ctx
                    .pending_ccc_events
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                let ccc_logs = final_ctx
                    .pending_ccc_logs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();

                Ok(ContractResult {
                    return_data: final_ctx.return_data.clone(),
                    logs: final_ctx.logs.clone(),
                    events: final_ctx.events.clone(),
                    storage_changes: final_ctx.storage_changes.clone(),
                    success: true,
                    error: None,
                    compute_used,
                    return_code: ret_code,
                    cross_call_changes: ccc_changes,
                    cross_call_events: ccc_events,
                    cross_call_logs: ccc_logs,
                    ccc_value_deltas: final_ctx
                        .pending_ccc_value_deltas
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone(),
                    native_account_ops: final_ctx.pending_native_account_ops.clone(),
                })
            }
            Err(e) => {
                let error_msg = if metering_remaining == 0 {
                    "Contract execution exceeded compute budget (out of gas)".to_string()
                } else {
                    format!("Contract trap: {}", e)
                };
                Ok(ContractResult {
                    return_data: vec![],
                    logs: final_ctx.logs.clone(),
                    events: Vec::new(),              // discard events on failure
                    storage_changes: HashMap::new(), // discard changes on failure
                    success: false,
                    error: Some(error_msg),
                    compute_used,
                    return_code: None,
                    cross_call_changes: HashMap::new(),
                    cross_call_events: Vec::new(),
                    cross_call_logs: Vec::new(),
                    ccc_value_deltas: HashMap::new(),
                    native_account_ops: Vec::new(),
                })
            }
        }
    }

    fn prepare_execution_context(mut ctx: ContractContext, args: &[u8]) -> ContractContext {
        for (k, v) in std::mem::take(&mut ctx.cross_contract_storage) {
            ctx.storage.entry(k).or_insert(v);
        }
        ctx.args = args.to_vec();
        ctx
    }
}

// ─── Host functions callable from WASM ───────────────────────────────────────

/// Poseidon hash host function: hash two 32-byte field elements and write
/// the 32-byte result into WASM memory.
///
/// Signature: host_poseidon_hash(left_ptr: u32, right_ptr: u32, out_ptr: u32) -> u32
///   left_ptr  — pointer to 32 bytes (Fr element, little-endian)
///   right_ptr — pointer to 32 bytes (Fr element, little-endian)
///   out_ptr   — pointer to 32-byte output buffer
///   Returns 0 on success, 1 on error
fn host_poseidon_hash(
    mut env: FunctionEnvMut<ContractContext>,
    left_ptr: u32,
    right_ptr: u32,
    out_ptr: u32,
) -> u32 {
    use ark_bn254::Fr;
    use ark_ff::PrimeField;

    {
        let ctx = env.data_mut();
        if !deduct_compute(ctx, COMPUTE_POSEIDON_HASH) {
            return 1;
        }
    }
    let ctx = env.data();
    let memory = match &ctx.memory {
        Some(m) => m.clone(),
        None => return 1,
    };
    let view = memory.view(&env);

    // Read left and right field elements
    let mut left_bytes = [0u8; 32];
    let mut right_bytes = [0u8; 32];
    if view.read(left_ptr as u64, &mut left_bytes).is_err() {
        return 1;
    }
    if view.read(right_ptr as u64, &mut right_bytes).is_err() {
        return 1;
    }

    // Convert to Fr (little-endian, reduced mod field order)
    let left_fr = Fr::from_le_bytes_mod_order(&left_bytes);
    let right_fr = Fr::from_le_bytes_mod_order(&right_bytes);

    // Compute Poseidon hash
    let result_fr = crate::zk::poseidon_hash_fr(left_fr, right_fr);
    let result_bytes = crate::zk::fr_to_bytes(&result_fr);

    // Write result to output buffer
    if view.write(out_ptr as u64, &result_bytes).is_err() {
        return 1;
    }

    0
}

/// Helper: deduct compute units. Returns false if budget exhausted.
fn deduct_compute(ctx: &mut ContractContext, cost: u64) -> bool {
    if ctx.compute_remaining < cost {
        ctx.compute_remaining = 0;
        false
    } else {
        ctx.compute_remaining -= cost;
        true
    }
}

/// Read from contract storage.
/// Supports TWO calling conventions:
/// - 4-param (SDK-compatible): storage_read(key_ptr, key_len, val_ptr, val_len) -> bytes_written | 0
///   Reads key, looks up value, writes value directly into val_ptr buffer.
/// - 2-param (legacy): storage_read(key_ptr, key_len) -> value_len | 0xFFFFFFFF
///   Stores result internally for retrieval via storage_read_result.
///
/// We implement the 4-param version since the SDK uses it. It reads key, looks up,
/// and writes the value into the output buffer in a single call.
fn host_storage_read(
    mut env: FunctionEnvMut<ContractContext>,
    key_ptr: u32,
    key_len: u32,
    val_ptr: u32,
    val_len: u32,
) -> u32 {
    let key_len_usize = key_len as usize;
    if key_len_usize > MAX_KEY_LEN {
        return 0;
    }

    // Phase 1: Read key from WASM memory (immutable borrow)
    let key = {
        let ctx = env.data();
        if ctx.compute_remaining < COMPUTE_STORAGE_READ {
            return 0;
        }
        let memory = match &ctx.memory {
            Some(m) => m.clone(),
            None => return 0,
        };
        let view = memory.view(&env);
        let mut buf = vec![0u8; key_len_usize];
        if view.read(key_ptr as u64, &mut buf).is_err() {
            return 0;
        }
        buf
    };

    // Phase 2: Lookup value and clone it (mutable borrow for compute + cache)
    let (found_value, write_len) = {
        let ctx = env.data_mut();
        deduct_compute(ctx, COMPUTE_STORAGE_READ);
        match ctx.storage.get(&key) {
            Some(value) => {
                let wl = value.len().min(val_len as usize);
                // PERF-FIX 4: Eliminate double-clone. clone_from reuses the
                // existing last_read_value buffer when it has sufficient capacity,
                // saving one allocation on repeated reads of similar-sized values.
                let v = value.clone();
                ctx.last_read_value.clone_from(&v);
                (Some(v), wl)
            }
            None => {
                ctx.last_read_value.clear();
                (None, 0)
            }
        }
    }; // mutable borrow dropped

    // Phase 3: Write value to WASM memory (immutable borrow)
    let ret = match found_value {
        Some(value) => {
            if write_len > 0 {
                let memory = match env.data().memory.clone() {
                    Some(m) => m,
                    None => return 0,
                };
                let view = memory.view(&env);
                if view.write(val_ptr as u64, &value[..write_len]).is_err() {
                    return 0;
                }
            }
            write_len as u32
        }
        None => 0,
    };
    ret
}

/// Copy last `storage_read` result into WASM memory at `[out_ptr..out_ptr+out_len]`.
/// Backward-compat for 2-phase read pattern.
/// Returns: number of bytes actually written (min of value length and out_len).
fn host_storage_read_result(
    mut env: FunctionEnvMut<ContractContext>,
    out_ptr: u32,
    out_len: u32,
) -> u32 {
    // AUDIT-FIX 2.1: Charge compute for read_result
    {
        let ctx = env.data_mut();
        let cost = COMPUTE_READ_RESULT + (out_len as u64) * COMPUTE_BYTE_COST;
        if !deduct_compute(ctx, cost) {
            return 0;
        }
    }
    let ctx = env.data();
    let value = ctx.last_read_value.clone();
    let memory = match &ctx.memory {
        Some(m) => m.clone(),
        None => return 0,
    };
    let view = memory.view(&env);

    let write_len = value.len().min(out_len as usize);
    if write_len == 0 {
        return 0;
    }
    if view.write(out_ptr as u64, &value[..write_len]).is_err() {
        return 0;
    }
    write_len as u32
}

/// Write to contract storage.
/// Reads key at `[key_ptr..key_ptr+key_len]` and value at `[val_ptr..val_ptr+val_len]`.
/// Returns: 1 on success, 0 on error.
fn host_storage_write(
    mut env: FunctionEnvMut<ContractContext>,
    key_ptr: u32,
    key_len: u32,
    val_ptr: u32,
    val_len: u32,
) -> u32 {
    let key_len_usize = key_len as usize;
    let val_len_usize = val_len as usize;
    if key_len_usize > MAX_KEY_LEN || val_len_usize > MAX_VALUE_LEN {
        return 0;
    }

    // Read key and value from WASM memory
    let (key, val) = {
        let ctx = env.data();
        if ctx.compute_remaining < COMPUTE_STORAGE_WRITE {
            return 0;
        }
        let memory = match &ctx.memory {
            Some(m) => m.clone(),
            None => return 0,
        };
        let view = memory.view(&env);
        let mut key_buf = vec![0u8; key_len_usize];
        let mut val_buf = vec![0u8; val_len_usize];
        if view.read(key_ptr as u64, &mut key_buf).is_err() {
            return 0;
        }
        if view.read(val_ptr as u64, &mut val_buf).is_err() {
            return 0;
        }
        (key_buf, val_buf)
    };

    // Update live storage and track the change
    let ctx = env.data_mut();
    // Task 4.3 (M-4): per-byte compute cost for storage writes
    let write_cost = COMPUTE_STORAGE_WRITE + (val.len() as u64) * COMPUTE_STORAGE_WRITE_PER_BYTE;
    deduct_compute(ctx, write_cost);
    // AUDIT-FIX 2.2 + H18: Enforce storage entry limit per contract.
    // Increased from 10K to 100K to support contracts with many entries
    // (e.g., DAO governance proposals, NFT collections).
    const MAX_STORAGE_ENTRIES: usize = 100_000;
    if !ctx.storage.contains_key(&key) && ctx.storage.len() >= MAX_STORAGE_ENTRIES {
        tracing::warn!(
            "Contract {} hit storage limit ({} entries) — rejecting write for key {:?}",
            ctx.contract.to_base58(),
            MAX_STORAGE_ENTRIES,
            &key[..key.len().min(16)]
        );
        return 0; // reject — storage full
    }
    // Task 4.3 (M-4): Enforce total storage bytes limit at protocol level.
    // Compute the delta from this write (new key+val adds bytes; overwrite adjusts by diff).
    let new_bytes = key.len() + val.len();
    let old_bytes = ctx
        .storage
        .get(&key)
        .map_or(0, |old_val| key.len() + old_val.len());
    let projected = ctx.storage_bytes_used + new_bytes - old_bytes;
    if projected > MAX_TOTAL_STORAGE_BYTES {
        tracing::warn!(
            "Contract {} hit storage byte limit ({} > {} bytes) — rejecting write",
            ctx.contract.to_base58(),
            projected,
            MAX_TOTAL_STORAGE_BYTES,
        );
        return 0; // reject — total bytes exceeded
    }
    ctx.storage_bytes_used = projected;
    ctx.storage.insert(key.clone(), val.clone());
    ctx.storage_changes.insert(key, Some(val));
    1
}

/// Delete a key from contract storage.
/// Returns: 1 on success-deleted, 0 if key not found or error.
fn host_storage_delete(
    mut env: FunctionEnvMut<ContractContext>,
    key_ptr: u32,
    key_len: u32,
) -> u32 {
    let key_len_usize = key_len as usize;
    if key_len_usize > MAX_KEY_LEN {
        return 0;
    }

    let key = {
        let ctx = env.data();
        if ctx.compute_remaining < COMPUTE_STORAGE_DELETE {
            return 0;
        }
        let memory = match &ctx.memory {
            Some(m) => m.clone(),
            None => return 0,
        };
        let view = memory.view(&env);
        let mut buf = vec![0u8; key_len_usize];
        if view.read(key_ptr as u64, &mut buf).is_err() {
            return 0;
        }
        buf
    };

    let ctx = env.data_mut();
    deduct_compute(ctx, COMPUTE_STORAGE_DELETE);
    if let Some(old_val) = ctx.storage.remove(&key) {
        // Task 4.3 (M-4): reclaim bytes on delete
        let freed = key.len() + old_val.len();
        ctx.storage_bytes_used = ctx.storage_bytes_used.saturating_sub(freed);
        ctx.storage_changes.insert(key, None);
        1
    } else {
        0
    }
}

/// Log a message from the contract.
/// Reads UTF-8 string at `[msg_ptr..msg_ptr+msg_len]` from WASM memory.
fn host_log_msg(mut env: FunctionEnvMut<ContractContext>, msg_ptr: u32, msg_len: u32) {
    let msg_len_usize = msg_len as usize;
    if msg_len_usize > MAX_LOG_LEN {
        return;
    }

    let msg = {
        let ctx = env.data();
        if ctx.compute_remaining < COMPUTE_LOG {
            return;
        }
        let memory = match &ctx.memory {
            Some(m) => m.clone(),
            None => return,
        };
        let view = memory.view(&env);
        let mut buf = vec![0u8; msg_len_usize];
        if view.read(msg_ptr as u64, &mut buf).is_err() {
            return;
        }
        String::from_utf8_lossy(&buf).into_owned()
    };

    let ctx = env.data_mut();
    deduct_compute(ctx, COMPUTE_LOG);
    // P9-CORE-06: Cap log entries to prevent unbounded heap growth
    if ctx.logs.len() < MAX_LOG_ENTRIES {
        ctx.logs.push(msg);
    }
}

/// Emit a structured event.
/// Reads JSON-serialized event at `[data_ptr..data_ptr+data_len]` from WASM memory.
/// Expected format: `{"name":"Transfer","from":"...","to":"...","amount":"..."}`
/// The `name` field is extracted as the event topic; remaining fields become data.
fn host_emit_event(mut env: FunctionEnvMut<ContractContext>, data_ptr: u32, data_len: u32) -> u32 {
    let data_len_usize = data_len as usize;
    if data_len_usize > MAX_EVENT_DATA {
        return 1;
    }

    let json_str = {
        let ctx = env.data();
        if ctx.compute_remaining < COMPUTE_EVENT {
            return 1;
        }
        let memory = match &ctx.memory {
            Some(m) => m.clone(),
            None => return 1,
        };
        let view = memory.view(&env);
        let mut buf = vec![0u8; data_len_usize];
        if view.read(data_ptr as u64, &mut buf).is_err() {
            return 1;
        }
        match String::from_utf8(buf) {
            Ok(s) => s,
            Err(_) => return 1,
        }
    };

    // Parse as JSON object
    let parsed: HashMap<String, String> = match serde_json::from_str(&json_str) {
        Ok(m) => m,
        Err(_) => return 1,
    };

    let ctx = env.data_mut();
    deduct_compute(ctx, COMPUTE_EVENT);

    let name = parsed
        .get("name")
        .cloned()
        .unwrap_or_else(|| "Unknown".to_string());
    let mut data = parsed;
    data.remove("name");

    let event = ContractEvent {
        program: ctx.contract,
        name,
        data,
        slot: ctx.slot,
    };
    ctx.events.push(event);
    0
}

/// Deterministic timestamp: returns the block slot number.
/// Contracts must NOT use wall-clock time for determinism.
fn host_get_timestamp(env: FunctionEnvMut<ContractContext>) -> u64 {
    env.data().slot
}

/// Write the 32-byte caller pubkey into WASM memory at `out_ptr`.
fn host_get_caller(mut env: FunctionEnvMut<ContractContext>, out_ptr: u32) -> u32 {
    // AUDIT-FIX 2.1: Charge compute for get_caller
    {
        let ctx = env.data_mut();
        if !deduct_compute(ctx, COMPUTE_GET_CALLER) {
            return 1;
        }
    }
    let ctx = env.data();
    let caller_bytes = ctx.caller.0;
    let memory = match &ctx.memory {
        Some(m) => m.clone(),
        None => return 1,
    };
    let view = memory.view(&env);
    if view.write(out_ptr as u64, &caller_bytes).is_err() {
        return 1;
    }
    0
}

/// Write the 32-byte contract (self) address into WASM memory at `out_ptr`.
/// This lets a contract discover its own on-chain address, which is required
/// for the self-custody pattern: the contract holds tokens at its own address
/// and uses get_contract_address() as the `from` field in cross-contract
/// token transfers so that `caller == from` is always satisfied.
fn host_get_contract_address(mut env: FunctionEnvMut<ContractContext>, out_ptr: u32) -> u32 {
    {
        let ctx = env.data_mut();
        if !deduct_compute(ctx, COMPUTE_GET_CALLER) {
            return 1;
        }
    }
    let ctx = env.data();
    let contract_bytes = ctx.contract.0;
    let memory = match &ctx.memory {
        Some(m) => m.clone(),
        None => return 1,
    };
    let view = memory.view(&env);
    if view.write(out_ptr as u64, &contract_bytes).is_err() {
        return 1;
    }
    0
}

/// Return the value (spores) transferred with the call.
fn host_get_value(env: FunctionEnvMut<ContractContext>) -> u64 {
    env.data().value
}

/// Return the current block slot.
fn host_get_slot(env: FunctionEnvMut<ContractContext>) -> u64 {
    env.data().slot
}

/// Return the length of the args passed to this contract call.
fn host_get_args_len(env: FunctionEnvMut<ContractContext>) -> u32 {
    env.data().args.len() as u32
}

/// Copy function args into WASM memory at `[out_ptr..out_ptr+out_len]`.
/// Returns: number of bytes written.
fn host_get_args(mut env: FunctionEnvMut<ContractContext>, out_ptr: u32, out_len: u32) -> u32 {
    // AUDIT-FIX 2.1: Charge compute for get_args
    {
        let ctx = env.data_mut();
        let cost = COMPUTE_GET_ARGS + (out_len as u64) * COMPUTE_BYTE_COST;
        if !deduct_compute(ctx, cost) {
            return 0;
        }
    }
    let ctx = env.data();
    let args = ctx.args.clone();
    let memory = match &ctx.memory {
        Some(m) => m.clone(),
        None => return 0,
    };
    let view = memory.view(&env);
    let write_len = args.len().min(out_len as usize);
    if write_len == 0 {
        return 0;
    }
    if view.write(out_ptr as u64, &args[..write_len]).is_err() {
        return 0;
    }
    write_len as u32
}

/// Set return data from the contract.
/// Reads `[data_ptr..data_ptr+data_len]` from WASM memory and stores it
/// as the return value of this execution.
fn host_set_return_data(
    mut env: FunctionEnvMut<ContractContext>,
    data_ptr: u32,
    data_len: u32,
) -> u32 {
    let data_len_usize = data_len as usize;
    if data_len_usize > MAX_RETURN_DATA {
        return 1;
    }
    // AUDIT-FIX 2.1: Charge compute for set_return_data
    {
        let ctx = env.data_mut();
        let cost = COMPUTE_SET_RETURN_DATA + (data_len as u64) * COMPUTE_BYTE_COST;
        if !deduct_compute(ctx, cost) {
            return 1;
        }
    }

    let data = {
        let ctx = env.data();
        let memory = match &ctx.memory {
            Some(m) => m.clone(),
            None => return 1,
        };
        let view = memory.view(&env);
        let mut buf = vec![0u8; data_len_usize];
        if view.read(data_ptr as u64, &mut buf).is_err() {
            return 1;
        }
        buf
    };

    let ctx = env.data_mut();
    ctx.return_data = data;
    0
}

/// Cross-contract call — full re-entrant implementation.
///
/// Reads target address (32 bytes), function name, args, and value from the
/// caller's WASM memory, loads the target contract from the state store,
/// creates a nested execution context, and executes the target function in a
/// fresh ContractRuntime.
///
/// Returns the number of bytes written to result_ptr on success, or 0 on
/// failure. The SDK treats >0 as success.
///
/// Storage changes from the callee are accumulated in `pending_ccc_changes`
/// (shared between all nesting levels via Arc<Mutex<>>). The processor applies
/// them atomically after the top-level execution completes.
///
/// Re-entrancy is bounded by `MAX_CROSS_CALL_DEPTH` (8 levels).
#[allow(clippy::too_many_arguments)]
fn host_cross_contract_call(
    mut env: FunctionEnvMut<ContractContext>,
    target_ptr: u32,
    function_ptr: u32,
    function_len: u32,
    args_ptr: u32,
    args_len: u32,
    value: u64,
    result_ptr: u32,
    result_len: u32,
) -> u32 {
    // ── Validate lengths ─────────────────────────────────────────────
    if function_len > MAX_CCC_FUNCTION_LEN || args_len > MAX_CCC_ARGS_LEN {
        return 0;
    }

    // ── Read parameters from caller's WASM linear memory ─────────────
    let memory = match env.data().memory.clone() {
        Some(m) => m,
        None => return 0,
    };

    let (target, function_name, args_buf) = {
        let view = memory.view(&env);

        // Target address (32 bytes)
        let mut target_bytes = [0u8; 32];
        if view.read(target_ptr as u64, &mut target_bytes).is_err() {
            return 0;
        }

        // Function name (UTF-8 string)
        let mut func_buf = vec![0u8; function_len as usize];
        if view.read(function_ptr as u64, &mut func_buf).is_err() {
            return 0;
        }
        let function_name = match String::from_utf8(func_buf) {
            Ok(s) => s,
            Err(_) => return 0,
        };

        // Args
        let mut args_buf = vec![0u8; args_len as usize];
        if args_len > 0 && view.read(args_ptr as u64, &mut args_buf).is_err() {
            return 0;
        }

        (Pubkey(target_bytes), function_name, args_buf)
    };

    // ── Extract shared state from context (before mutable borrow) ────
    let state_store = match env.data().state_store.clone() {
        Some(s) => s,
        None => {
            // No state store — running in test mode or standalone.
            // Return 0 so contracts get an Err from call_contract.
            return 0;
        }
    };
    let call_depth = env.data().call_depth;
    if call_depth >= MAX_CROSS_CALL_DEPTH {
        return 0; // Recursion depth exceeded
    }

    let caller_contract = env.data().contract;
    let current_slot = env.data().slot;
    let pending_changes = env.data().pending_ccc_changes.clone();
    let pending_events = env.data().pending_ccc_events.clone();
    let pending_logs = env.data().pending_ccc_logs.clone();
    let pending_value_deltas = env.data().pending_ccc_value_deltas.clone();

    // ── Deduct base compute cost ─────────────────────────────────────
    {
        let ctx = env.data_mut();
        if !deduct_compute(ctx, COMPUTE_CROSS_CALL) {
            return 0;
        }
    }

    if target == Pubkey([0u8; 32]) {
        return handle_native_account_call(env, &function_name, &args_buf, result_ptr, result_len);
    }

    // ── Load target contract from state ──────────────────────────────
    let target_account = match state_store.get_account(&target) {
        Ok(Some(a)) if a.executable => a,
        _ => return 0, // Target not found or not a contract
    };
    let target_contract: ContractAccount = match serde_json::from_slice(&target_account.data) {
        Ok(c) => c,
        Err(_) => return 0,
    };

    // ── Build callee storage: base + pending overlay ─────────────────
    // If prior cross-contract calls in this transaction already modified the
    // target contract's storage, merge those pending changes so the callee
    // sees a consistent view.
    let mut callee_storage: HashMap<Vec<u8>, Vec<u8>> =
        match state_store.load_contract_storage_map(&target) {
            Ok(entries) => entries.into_iter().collect(),
            Err(_) => return 0,
        };
    {
        let changes = pending_changes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(target_changes) = changes.get(&target) {
            for (k, v_opt) in target_changes {
                match v_opt {
                    Some(v) => {
                        callee_storage.insert(k.clone(), v.clone());
                    }
                    None => {
                        callee_storage.remove(k);
                    }
                }
            }
        }
    }

    // ── Cap callee compute at caller's remaining budget ──────────────
    let caller_remaining = env.data().compute_remaining;

    // ── Build callee context ─────────────────────────────────────────
    let callee_storage_bytes: usize = callee_storage.iter().map(|(k, v)| k.len() + v.len()).sum();
    let callee_ctx = ContractContext {
        caller: caller_contract, // The calling contract is the caller
        contract: target,
        value,
        slot: current_slot,
        storage: callee_storage,
        logs: Vec::new(),
        events: Vec::new(),
        storage_changes: HashMap::new(),
        last_read_value: Vec::new(),
        memory: None, // Set during execute()
        args: args_buf.clone(),
        return_data: Vec::new(),
        compute_remaining: caller_remaining,
        compute_limit: caller_remaining,
        cross_contract_storage: HashMap::new(),
        state_store: Some(state_store.clone()),
        call_depth: call_depth + 1,
        pending_ccc_changes: pending_changes.clone(),
        pending_ccc_events: pending_events.clone(),
        pending_ccc_logs: pending_logs.clone(),
        // AUDIT-FIX C-2: Fresh delta map so nested CCC deltas can be
        // rolled back if this callee fails.
        pending_ccc_value_deltas: Arc::new(Mutex::new(HashMap::new())),
        pending_native_account_ops: Vec::new(),
        pending_native_account_state: HashMap::new(),
        storage_bytes_used: callee_storage_bytes,
    };

    // ── AUDIT-FIX C-2: Track value as deltas instead of direct DB writes ──
    // Direct state_store.put_account writes bypass the StateBatch overlay,
    // causing balance inflation when the batch commits and overwrites the
    // CCC-modified values.  Track all value movements as deltas that the
    // processor applies atomically through the batch after execution.
    if value > 0 {
        let base_balance = match state_store.get_account(&caller_contract) {
            Ok(Some(a)) => a.spendable,
            _ => 0u64,
        };
        let current_delta = {
            let deltas = pending_value_deltas
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *deltas.get(&caller_contract).unwrap_or(&0)
        };
        let effective = (base_balance as i128) + (current_delta as i128);
        if effective < (value as i128) {
            let ctx = env.data_mut();
            ctx.logs.push(format!(
                "[CCC] Call to {}::{} rejected: caller {} has insufficient balance for value {}",
                crate::Pubkey(target.0),
                function_name,
                crate::Pubkey(caller_contract.0),
                value
            ));
            return 0;
        }
        // Record escrow delta (deduct from caller)
        {
            let mut deltas = pending_value_deltas
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *deltas.entry(caller_contract).or_default() -= value as i64;
        }
    }

    // ── Execute callee in a fresh runtime ────────────────────────────
    let mut runtime = ContractRuntime::get_pooled();
    let result = match runtime.execute(&target_contract, &function_name, &args_buf, callee_ctx) {
        Ok(r) => r,
        Err(e) => {
            runtime.return_to_pool();
            // AUDIT-FIX C-2: Refund escrowed value via delta on execute error
            if value > 0 {
                let mut deltas = pending_value_deltas
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                *deltas.entry(caller_contract).or_default() += value as i64;
            }
            // Log the error for diagnostics
            let ctx = env.data_mut();
            ctx.logs.push(format!(
                "[CCC] Call to {}::{} failed: {}",
                crate::Pubkey(target.0),
                function_name,
                e
            ));
            return 0;
        }
    };
    runtime.return_to_pool();

    // ── AUDIT-FIX D-1: Always deduct callee compute, even on failure ─
    {
        let ctx = env.data_mut();
        ctx.compute_remaining = ctx.compute_remaining.saturating_sub(result.compute_used);
    }

    if !result.success {
        // Callee failed — return 0, don't apply any changes
        // AUDIT-FIX C-2: Refund escrowed value via delta on callee failure
        if value > 0 {
            let mut deltas = pending_value_deltas
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *deltas.entry(caller_contract).or_default() += value as i64;
        }
        let ctx = env.data_mut();
        if let Some(ref err) = result.error {
            ctx.logs.push(format!(
                "[CCC] {}::{} returned error: {}",
                crate::Pubkey(target.0),
                function_name,
                err
            ));
        }
        return 0;
    }

    // ── AUDIT-FIX C-2: Credit value to callee via delta on success ──────
    if value > 0 {
        let mut deltas = pending_value_deltas
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *deltas.entry(target).or_default() += value as i64;
    }

    // ── Merge callee's direct storage changes into pending ───────────
    if !result.storage_changes.is_empty() {
        let mut changes = pending_changes.lock().unwrap_or_else(|e| e.into_inner());
        let entry = changes.entry(target).or_default();
        for (k, v) in &result.storage_changes {
            entry.insert(k.clone(), v.clone());
        }
    }

    // ── Merge callee's nested cross-call changes (from deeper levels) ─
    if !result.cross_call_changes.is_empty() {
        let mut changes = pending_changes.lock().unwrap_or_else(|e| e.into_inner());
        for (addr, addr_changes) in &result.cross_call_changes {
            let entry = changes.entry(*addr).or_default();
            for (k, v) in addr_changes {
                entry.insert(k.clone(), v.clone());
            }
        }
    }

    // ── Collect events and logs from callee ──────────────────────────
    if !result.events.is_empty() || !result.cross_call_events.is_empty() {
        let mut events = pending_events.lock().unwrap_or_else(|e| e.into_inner());
        events.extend(result.events);
        events.extend(result.cross_call_events);
    }
    if !result.logs.is_empty() || !result.cross_call_logs.is_empty() {
        let mut logs = pending_logs.lock().unwrap_or_else(|e| e.into_inner());
        logs.extend(result.logs);
        logs.extend(result.cross_call_logs);
    }

    // ── AUDIT-FIX C-2: Merge callee's CCC value deltas on success ───
    if !result.ccc_value_deltas.is_empty() {
        let mut deltas = pending_value_deltas
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for (addr, delta) in &result.ccc_value_deltas {
            *deltas.entry(*addr).or_default() += delta;
        }
    }

    if !result.native_account_ops.is_empty() {
        let ctx = env.data_mut();
        for op in &result.native_account_ops {
            if queue_native_account_op(ctx, &state_store, op.clone()).is_err() {
                return 0;
            }
        }
    }

    // ── Determine result data to write back to caller ────────────────
    // Priority: explicit return_data > return_code encoding > success byte
    let effective_result: Vec<u8> = if !result.return_data.is_empty() {
        result.return_data
    } else if let Some(rc) = result.return_code {
        // Encode the WASM return code as a 4-byte LE value.
        // For token `transfer()` returning 1, this gives [1, 0, 0, 0].
        // The SDK's `call_token_transfer` checks result[0] == 1, which matches.
        (rc as u32).to_le_bytes().to_vec()
    } else {
        // No return data and no return code — just signal success.
        vec![1u8]
    };

    // ── Write result data into caller's buffer ───────────────────────
    let write_len = effective_result.len().min(result_len as usize);
    if write_len > 0 {
        let memory = match env.data().memory.clone() {
            Some(m) => m,
            None => return 0,
        };
        let view = memory.view(&env);
        if view
            .write(result_ptr as u64, &effective_result[..write_len])
            .is_err()
        {
            return 0;
        }
    }

    // Return bytes written (>0 = success per SDK convention).
    // If write_len is 0 but call succeeded, return 1 as a success signal.
    if write_len == 0 {
        1
    } else {
        write_len as u32
    }
}

fn queue_native_account_op(
    ctx: &mut ContractContext,
    state_store: &StateStore,
    op: NativeAccountOp,
) -> Result<(), String> {
    let account_key = op.account();
    let mut account = if let Some(account) = ctx.pending_native_account_state.get(&account_key) {
        account.clone()
    } else {
        state_store
            .get_account(&account_key)?
            .ok_or_else(|| format!("Native account {} not found", account_key))?
    };
    op.apply(&mut account)?;
    ctx.pending_native_account_state
        .insert(account_key, account);

    // For Transfer ops, also credit the recipient account
    if let Some(to_key) = op.transfer_to() {
        if let NativeAccountOp::Transfer { amount, .. } = &op {
            let mut to_account = if let Some(acc) = ctx.pending_native_account_state.get(&to_key) {
                acc.clone()
            } else {
                state_store
                    .get_account(&to_key)?
                    .unwrap_or_else(|| Account::new(0, to_key))
            };
            to_account.spores = to_account.spores.saturating_add(*amount);
            to_account.spendable = to_account.spendable.saturating_add(*amount);
            ctx.pending_native_account_state.insert(to_key, to_account);
        }
    }

    ctx.pending_native_account_ops.push(op);
    Ok(())
}

fn handle_native_account_call(
    mut env: FunctionEnvMut<ContractContext>,
    function_name: &str,
    args: &[u8],
    result_ptr: u32,
    result_len: u32,
) -> u32 {
    let state_store = match env.data().state_store.clone() {
        Some(store) => store,
        None => return 0,
    };

    // balance_of only needs 32 bytes (address), not 40
    if function_name == "balance_of" {
        if args.len() < 32 {
            return 0;
        }
        let mut account_bytes = [0u8; 32];
        account_bytes.copy_from_slice(&args[..32]);
        let pubkey = Pubkey(account_bytes);
        let balance = state_store
            .get_account(&pubkey)
            .ok()
            .flatten()
            .map(|a| a.spores)
            .unwrap_or(0);
        // Write balance as u64 LE bytes to result buffer
        let balance_bytes = balance.to_le_bytes();
        let write_len = (8usize).min(result_len as usize);
        if write_len > 0 {
            let memory = match env.data().memory.clone() {
                Some(memory) => memory,
                None => return 0,
            };
            let view = memory.view(&env);
            if view
                .write(result_ptr as u64, &balance_bytes[..write_len])
                .is_err()
            {
                return 0;
            }
        }
        return 8; // return 8 bytes written
    }

    if args.len() < 40 {
        return 0;
    }

    let mut account_bytes = [0u8; 32];
    account_bytes.copy_from_slice(&args[..32]);
    let amount = u64::from_le_bytes(match args[32..40].try_into() {
        Ok(bytes) => bytes,
        Err(_) => return 0,
    });

    let op = match function_name {
        "lock" => NativeAccountOp::Lock {
            account: Pubkey(account_bytes),
            amount,
        },
        "unlock" => NativeAccountOp::Unlock {
            account: Pubkey(account_bytes),
            amount,
        },
        "deduct" => NativeAccountOp::DeductLocked {
            account: Pubkey(account_bytes),
            amount,
        },
        "transfer" => {
            // Transfer native LICN from the calling contract to a user account.
            // args layout: [to_address(32)][amount(8)]
            // The `from` is always the calling contract (env.data().contract).
            let from_contract = env.data().contract;
            NativeAccountOp::Transfer {
                from: from_contract,
                to: Pubkey(account_bytes),
                amount,
            }
        }
        _ => return 0,
    };

    {
        let ctx = env.data_mut();
        if queue_native_account_op(ctx, &state_store, op).is_err() {
            return 0;
        }
    }

    let write_len = (1usize).min(result_len as usize);
    if write_len > 0 {
        let memory = match env.data().memory.clone() {
            Some(memory) => memory,
            None => return 0,
        };
        let view = memory.view(&env);
        if view.write(result_ptr as u64, &[1u8]).is_err() {
            return 0;
        }
        1
    } else {
        1
    }
}

// ============================================================================
// ABI-AWARE JSON ARG ENCODING
// ============================================================================
//
// When the CLI or an agent sends contract call args as a JSON array (e.g.
// ["8nRM2Fk...", 1, "my-name", 21]), this encoder converts them to binary
// with a 0xAB layout descriptor so the WASM runtime can correctly map:
//   - Base58 string → 32-byte pubkey pointer (stride 32)
//   - Plain string  → UTF-8 byte pointer (stride = byte length)
//   - Integer       → raw bytes (stride 1, 2, or 4 depending on magnitude)
//   - I64 param     → 8-byte LE value (stride 8)
//
// This makes generic contract calls "just work" without clients needing to
// manually construct layout descriptors.

fn encode_json_args_to_binary(
    json_vals: &[serde_json::Value],
    wasm_params: &[wasmer::Type],
) -> Result<Vec<u8>, String> {
    if json_vals.len() != wasm_params.len() {
        return Err("JSON arg count does not match WASM param count".into());
    }

    // First pass: encode each JSON value to bytes and determine stride
    let mut parts: Vec<(u8, Vec<u8>)> = Vec::with_capacity(json_vals.len()); // (stride, data)

    for (val, param_type) in json_vals.iter().zip(wasm_params.iter()) {
        match param_type {
            wasmer::Type::I32 => {
                match val {
                    serde_json::Value::String(s) => {
                        // Try base58 decode (32-byte pubkey)
                        if let Ok(pk) = crate::Pubkey::from_base58(s) {
                            parts.push((32, pk.0.to_vec()));
                        } else {
                            // Plain UTF-8 string (passed as pointer).
                            // Pad to next 32-byte boundary.  Cap at stride
                            // 224 (u8 max is 255; 224 = 7×32 covers strings
                            // up to 224 bytes).  Longer strings are truncated
                            // with a log warning — callers should use binary
                            // layout descriptors for very large payloads.
                            let bytes = s.as_bytes();
                            let usable = bytes.len().min(224);
                            let padded_len = usable.div_ceil(32) * 32;
                            let mut padded = bytes[..usable].to_vec();
                            padded.resize(padded_len.max(32), 0);
                            parts.push((padded.len() as u8, padded));
                        }
                    }
                    serde_json::Value::Number(n) => {
                        if let Some(v) = n.as_u64() {
                            if v <= 0xFF {
                                parts.push((1, vec![v as u8]));
                            } else if v <= 0xFFFF {
                                parts.push((2, (v as u16).to_le_bytes().to_vec()));
                            } else if v <= 0xFFFF_FFFF {
                                // AUDIT-FIX D-3: Only use 4 bytes when value fits in u32
                                parts.push((4, (v as u32).to_le_bytes().to_vec()));
                            } else {
                                // AUDIT-FIX D-3: Use 8 bytes for values > u32::MAX
                                parts.push((8, v.to_le_bytes().to_vec()));
                            }
                        } else if let Some(v) = n.as_i64() {
                            if v >= i32::MIN as i64 && v <= i32::MAX as i64 {
                                parts.push((4, (v as i32).to_le_bytes().to_vec()));
                            } else {
                                // AUDIT-FIX D-3: Use 8 bytes for values outside i32 range
                                parts.push((8, v.to_le_bytes().to_vec()));
                            }
                        } else {
                            parts.push((4, 0u32.to_le_bytes().to_vec()));
                        }
                    }
                    serde_json::Value::Bool(b) => {
                        parts.push((1, vec![*b as u8]));
                    }
                    serde_json::Value::Array(arr) => {
                        // Byte array: [1, 2, 3, ...] → raw bytes as pointer
                        let bytes: Vec<u8> = arr
                            .iter()
                            .filter_map(|v| v.as_u64().map(|n| n as u8))
                            .collect();
                        let usable = bytes.len().min(224);
                        let padded_len = usable.div_ceil(32) * 32;
                        let mut padded = bytes[..usable].to_vec();
                        padded.resize(padded_len.max(32), 0);
                        parts.push((padded.len() as u8, padded));
                    }
                    _ => {
                        parts.push((4, 0u32.to_le_bytes().to_vec()));
                    }
                }
            }
            wasmer::Type::I64 => {
                let v = val
                    .as_u64()
                    .or_else(|| val.as_i64().map(|i| i as u64))
                    .unwrap_or(0);
                parts.push((8, v.to_le_bytes().to_vec()));
            }
            wasmer::Type::F32 => {
                let v = val.as_f64().unwrap_or(0.0) as f32;
                parts.push((4, v.to_le_bytes().to_vec()));
            }
            wasmer::Type::F64 => {
                let v = val.as_f64().unwrap_or(0.0);
                parts.push((8, v.to_le_bytes().to_vec()));
            }
            _ => {
                parts.push((4, 0u32.to_le_bytes().to_vec()));
            }
        }
    }

    // Build layout descriptor blob: 0xAB + [stride per param] + [data...]
    let n = parts.len();
    let data_len: usize = parts.iter().map(|(_, d)| d.len()).sum();
    let mut buf = Vec::with_capacity(1 + n + data_len);
    buf.push(0xAB); // layout descriptor marker
    for (stride, _) in &parts {
        buf.push(*stride);
    }
    for (_, data) in &parts {
        buf.extend_from_slice(data);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contract_account() {
        let owner = Pubkey::new([1u8; 32]);
        let code = vec![0x00, 0x61, 0x73, 0x6d]; // WASM magic number
        let contract = ContractAccount::new(code.clone(), owner);

        assert_eq!(contract.code, code);
        assert_eq!(contract.owner, owner);
        assert_eq!(contract.storage.len(), 0);
    }

    #[test]
    fn test_contract_context() {
        let caller = Pubkey::new([1u8; 32]);
        let contract = Pubkey::new([2u8; 32]);
        let ctx = ContractContext::new(caller, contract, 1000, 100);

        assert_eq!(ctx.value, 1000);
        assert_eq!(ctx.slot, 100);
        assert!(ctx.storage.is_empty());
        assert!(ctx.storage_changes.is_empty());
        assert!(ctx.args.is_empty());
        assert!(ctx.return_data.is_empty());
        assert!(ctx.events.is_empty());
        assert_eq!(ctx.compute_remaining, DEFAULT_COMPUTE_LIMIT);
    }

    #[test]
    fn test_contract_context_with_storage() {
        let caller = Pubkey::new([1u8; 32]);
        let contract = Pubkey::new([2u8; 32]);
        let mut store = HashMap::new();
        store.insert(b"key1".to_vec(), b"val1".to_vec());

        let ctx = ContractContext::with_storage(caller, contract, 0, 50, store.clone());
        assert_eq!(ctx.storage.len(), 1);
        assert_eq!(ctx.storage.get(b"key1".as_slice()), Some(&b"val1".to_vec()));
        assert_eq!(ctx.compute_remaining, DEFAULT_COMPUTE_LIMIT);
    }

    #[test]
    fn test_contract_context_with_args() {
        let caller = Pubkey::new([1u8; 32]);
        let contract = Pubkey::new([2u8; 32]);
        let args = vec![1, 2, 3, 4];
        let ctx =
            ContractContext::with_args(caller, contract, 500, 42, HashMap::new(), args.clone());
        assert_eq!(ctx.args, args);
        assert_eq!(ctx.value, 500);
        assert_eq!(ctx.slot, 42);
    }

    #[test]
    fn test_contract_event() {
        let program = Pubkey::new([3u8; 32]);
        let mut data = HashMap::new();
        data.insert("from".to_string(), "alice".to_string());
        data.insert("to".to_string(), "bob".to_string());
        data.insert("amount".to_string(), "1000".to_string());

        let event = ContractEvent {
            program,
            name: "Transfer".to_string(),
            data: data.clone(),
            slot: 100,
        };

        assert_eq!(event.name, "Transfer");
        assert_eq!(event.data.len(), 3);
        assert_eq!(event.slot, 100);
    }

    #[test]
    fn test_contract_result_fields() {
        let result = ContractResult {
            return_data: vec![42],
            logs: vec!["hello".to_string()],
            events: vec![ContractEvent {
                program: Pubkey::new([1u8; 32]),
                name: "Test".to_string(),
                data: HashMap::new(),
                slot: 1,
            }],
            storage_changes: HashMap::new(),
            success: true,
            error: None,
            compute_used: 500,
            return_code: None,
            cross_call_changes: HashMap::new(),
            cross_call_events: Vec::new(),
            cross_call_logs: Vec::new(),
            ccc_value_deltas: HashMap::new(),
            native_account_ops: Vec::new(),
        };

        assert!(result.success);
        assert_eq!(result.return_data, vec![42]);
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.compute_used, 500);
    }

    #[test]
    fn test_deduct_compute() {
        let caller = Pubkey::new([1u8; 32]);
        let contract = Pubkey::new([2u8; 32]);
        let mut ctx = ContractContext::new(caller, contract, 0, 0);
        ctx.compute_remaining = 500;

        assert!(deduct_compute(&mut ctx, 200));
        assert_eq!(ctx.compute_remaining, 300);

        assert!(deduct_compute(&mut ctx, 300));
        assert_eq!(ctx.compute_remaining, 0);

        assert!(!deduct_compute(&mut ctx, 1));
        assert_eq!(ctx.compute_remaining, 0);
    }

    #[test]
    fn test_contract_account_storage() {
        let owner = Pubkey::new([1u8; 32]);
        let mut contract = ContractAccount::new(vec![0x00], owner);

        contract.set_storage(b"hello".to_vec(), b"world".to_vec());
        assert_eq!(contract.get_storage(b"hello"), Some(b"world".to_vec()));

        let removed = contract.remove_storage(b"hello");
        assert_eq!(removed, Some(b"world".to_vec()));
        assert_eq!(contract.get_storage(b"hello"), None);
    }

    // ── JSON arg encoder tests ──────────────────────────────────────

    #[test]
    fn test_encode_json_pubkey_and_integers() {
        // Simulates: register_identity(owner_ptr: I32, agent_type: I32, name_ptr: I32, name_len: I32)
        // JSON:      ["11111111111111111111111111111111", 1, "agent-demo", 10]
        let json: Vec<serde_json::Value> =
            serde_json::from_str(r#"["11111111111111111111111111111111", 1, "agent-demo", 10]"#)
                .unwrap();
        let params = vec![
            wasmer::Type::I32,
            wasmer::Type::I32,
            wasmer::Type::I32,
            wasmer::Type::I32,
        ];
        let buf = encode_json_args_to_binary(&json, &params).unwrap();

        // Layout: 0xAB [32, 1, 32, 1] [32B pubkey] [1B: 1] [32B: "agent-demo\0..."] [1B: 10]
        assert_eq!(buf[0], 0xAB);
        assert_eq!(buf[1], 32); // pubkey stride
        assert_eq!(buf[2], 1); // agent_type stride
        assert_eq!(buf[3], 32); // name string stride
        assert_eq!(buf[4], 1); // name_len stride
                               // Data starts at offset 5
                               // 32-byte pubkey (all zeros for "1111...1")
        assert_eq!(&buf[5..37], &[0u8; 32]);
        // agent_type = 1
        assert_eq!(buf[37], 1);
        // name string starts at offset 38, "agent-demo" = 10 bytes + 22 padding
        assert_eq!(&buf[38..48], b"agent-demo");
        assert_eq!(&buf[48..70], &[0u8; 22]); // padding
                                              // name_len = 10
        assert_eq!(buf[70], 10);
        assert_eq!(buf.len(), 71);
    }

    #[test]
    fn test_encode_json_i64_param() {
        // Simulates: transfer(from: I32, to: I32, amount: I64)
        let json: Vec<serde_json::Value> = serde_json::from_str(
            r#"["11111111111111111111111111111111", "11111111111111111111111111111111", 1000000]"#,
        )
        .unwrap();
        let params = vec![wasmer::Type::I32, wasmer::Type::I32, wasmer::Type::I64];
        let buf = encode_json_args_to_binary(&json, &params).unwrap();

        assert_eq!(buf[0], 0xAB);
        assert_eq!(buf[1], 32); // from pubkey
        assert_eq!(buf[2], 32); // to pubkey
        assert_eq!(buf[3], 8); // amount i64
                               // Data: 32 + 32 + 8 = 72 bytes, total = 1 + 3 + 72 = 76
        assert_eq!(buf.len(), 76);
        // amount at offset 4+32+32 = 68
        let amount = u64::from_le_bytes(buf[68..76].try_into().unwrap());
        assert_eq!(amount, 1000000);
    }

    #[test]
    fn test_encode_json_count_mismatch() {
        let json: Vec<serde_json::Value> = serde_json::from_str(r#"[1, 2]"#).unwrap();
        let params = vec![wasmer::Type::I32];
        assert!(encode_json_args_to_binary(&json, &params).is_err());
    }

    #[test]
    fn test_encode_json_u16_u32_numbers() {
        let json: Vec<serde_json::Value> = serde_json::from_str(r#"[300, 70000]"#).unwrap();
        let params = vec![wasmer::Type::I32, wasmer::Type::I32];
        let buf = encode_json_args_to_binary(&json, &params).unwrap();

        assert_eq!(buf[0], 0xAB);
        assert_eq!(buf[1], 2); // 300 fits in u16
        assert_eq!(buf[2], 4); // 70000 needs u32
                               // Data: 2 + 4 = 6 bytes
        let v16 = u16::from_le_bytes([buf[3], buf[4]]);
        assert_eq!(v16, 300);
        let v32 = u32::from_le_bytes([buf[5], buf[6], buf[7], buf[8]]);
        assert_eq!(v32, 70000);
    }

    #[test]
    fn test_encode_json_bool_param() {
        let json: Vec<serde_json::Value> = serde_json::from_str(r#"[true, false]"#).unwrap();
        let params = vec![wasmer::Type::I32, wasmer::Type::I32];
        let buf = encode_json_args_to_binary(&json, &params).unwrap();

        assert_eq!(buf[0], 0xAB);
        assert_eq!(buf[1], 1);
        assert_eq!(buf[2], 1);
        assert_eq!(buf[3], 1); // true
        assert_eq!(buf[4], 0); // false
    }

    #[test]
    fn test_encode_json_long_string_capped() {
        // String > 224 bytes should be capped at 224 (stride 224)
        let long_str = "x".repeat(250);
        let json = vec![serde_json::Value::String(long_str)];
        let params = vec![wasmer::Type::I32];
        let buf = encode_json_args_to_binary(&json, &params).unwrap();

        assert_eq!(buf[0], 0xAB);
        assert_eq!(buf[1], 224); // capped stride
                                 // Data: 224 bytes (truncated from 250, padded to 224)
        assert_eq!(buf.len(), 1 + 1 + 224);
        // First bytes should be 'x'
        assert_eq!(buf[2], b'x');
        assert_eq!(buf[225], b'x');
    }

    #[test]
    fn test_encode_json_return_code_field() {
        let result = ContractResult {
            return_data: vec![],
            logs: vec![],
            events: vec![],
            storage_changes: HashMap::new(),
            success: true,
            error: None,
            compute_used: 100,
            return_code: Some(1),
            cross_call_changes: HashMap::new(),
            cross_call_events: Vec::new(),
            cross_call_logs: Vec::new(),
            ccc_value_deltas: HashMap::new(),
            native_account_ops: Vec::new(),
        };
        assert!(result.success);
        assert_eq!(result.return_code, Some(1));
    }

    #[test]
    fn test_contract_abi_parses_repo_json_shape() {
        let abi: ContractAbi = serde_json::from_str(
            r#"{
                "contract": "dex_core",
                "version": "1.0",
                "functions": [
                    {
                        "name": "update_pair_fees",
                        "opcode": 7,
                        "params": [
                            {"name": "caller", "type": "Pubkey"},
                            {"name": "pair_id", "type": "u64"},
                            {"name": "maker_fee_bps", "type": "i16"},
                            {"name": "taker_fee_bps", "type": "u16"},
                            {"name": "enabled", "type": "bool", "optional": true}
                        ]
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(abi.name, "dex_core");
        assert_eq!(abi.functions[0].opcode, Some(7));
        assert_eq!(abi.functions[0].params[0].param_type, AbiType::Pubkey);
        assert_eq!(abi.functions[0].params[2].param_type, AbiType::I16);
        assert_eq!(abi.functions[0].params[3].param_type, AbiType::U16);
        assert_eq!(abi.functions[0].params[4].param_type, AbiType::Bool);
        assert!(abi.functions[0].params[4].optional);
    }

    #[test]
    fn test_build_opcode_dispatch_args_prefixes_selector() {
        let args = vec![1u8, 2, 3, 4];
        assert_eq!(build_opcode_dispatch_args(9, &args), vec![9u8, 1, 2, 3, 4]);
    }

    /// P9-CORE-04: Verify MODULE_CACHE uses LRU with bounded capacity
    #[test]
    fn test_module_cache_lru_bounded() {
        let cap = MODULE_CACHE_MAX_ENTRIES;
        assert!(cap > 0, "MODULE_CACHE_MAX_ENTRIES must be positive");
        // Verify the cache is an LruCache with the expected cap
        let cache = MODULE_CACHE.lock().unwrap();
        assert_eq!(
            cache.cap().get(),
            cap,
            "cache capacity should match constant"
        );
    }

    // ── Task 3.5: WASM Memory Limits tests ──

    #[test]
    fn test_wasm_memory_constants() {
        assert_eq!(
            MAX_WASM_MEMORY_PAGES, 1024,
            "Max should be 1024 pages (64MB)"
        );
        assert_eq!(
            DEFAULT_WASM_MEMORY_PAGES, 16,
            "Default should be 16 pages (1MB)"
        );
        const { assert!(DEFAULT_WASM_MEMORY_PAGES < MAX_WASM_MEMORY_PAGES) };
    }

    #[test]
    fn test_wasm_memory_limit_sizes() {
        // Verify the sizes are correct
        let max_bytes = MAX_WASM_MEMORY_PAGES as u64 * 65536;
        assert_eq!(max_bytes, 64 * 1024 * 1024, "Max should be 64MB");
        let default_bytes = DEFAULT_WASM_MEMORY_PAGES as u64 * 65536;
        assert_eq!(default_bytes, 1024 * 1024, "Default should be 1MB");
    }

    /// Build a minimal valid WASM module with a memory section (exported).
    /// `min_pages` = initial memory, `max_pages` = optional max memory.
    fn wasm_with_memory(min_pages: u32, max_pages: Option<u32>) -> Vec<u8> {
        let mut wasm = vec![
            0x00, 0x61, 0x73, 0x6D, // magic
            0x01, 0x00, 0x00, 0x00, // version 1
        ];

        // Memory section (id=5)
        let mut mem_data = vec![0x01]; // 1 memory
        if let Some(max) = max_pages {
            // Flags 0x01 = has maximum
            mem_data.push(0x01);
            // LEB128-encode min_pages
            leb128_encode(&mut mem_data, min_pages);
            // LEB128-encode max_pages
            leb128_encode(&mut mem_data, max);
        } else {
            // Flags 0x00 = no maximum
            mem_data.push(0x00);
            leb128_encode(&mut mem_data, min_pages);
        }
        wasm.push(0x05); // section id (memory)
        leb128_encode(&mut wasm, mem_data.len() as u32);
        wasm.extend_from_slice(&mem_data);

        // Export section (id=7) — export memory as "memory"
        let name = b"memory";
        let mut export_data = vec![0x01]; // 1 export
        leb128_encode(&mut export_data, name.len() as u32);
        export_data.extend_from_slice(name);
        export_data.push(0x02); // export kind = memory
        export_data.push(0x00); // memory index 0
        wasm.push(0x07); // section id (export)
        leb128_encode(&mut wasm, export_data.len() as u32);
        wasm.extend_from_slice(&export_data);

        wasm
    }

    /// LEB128 unsigned encoding for WASM integers.
    fn leb128_encode(buf: &mut Vec<u8>, mut value: u32) {
        loop {
            let byte = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                buf.push(byte);
                break;
            } else {
                buf.push(byte | 0x80);
            }
        }
    }

    #[test]
    fn test_deploy_rejects_memory_exceeding_max() {
        let mut rt = ContractRuntime::new();
        // Create a WASM module with 1025 initial pages (exceeds 1024 max)
        let wasm = wasm_with_memory(1025, None);
        let result = rt.deploy(&wasm);
        assert!(
            result.is_err(),
            "Deploy should reject >1024 pages initial memory"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("exceeds limit"),
            "Error should mention limit: {}",
            err
        );
    }

    #[test]
    fn test_deploy_rejects_max_memory_exceeding_limit() {
        let mut rt = ContractRuntime::new();
        // Create a WASM module with 1 initial page but 2000 max pages
        let wasm = wasm_with_memory(1, Some(2000));
        let result = rt.deploy(&wasm);
        assert!(result.is_err(), "Deploy should reject max_pages > 1024");
        let err = result.unwrap_err();
        assert!(
            err.contains("exceeds limit"),
            "Error should mention limit: {}",
            err
        );
    }

    #[test]
    fn test_deploy_accepts_memory_at_max() {
        let mut rt = ContractRuntime::new();
        // Create a WASM module with exactly 1024 initial pages and 1024 max
        let wasm = wasm_with_memory(1024, Some(1024));
        let result = rt.deploy(&wasm);
        assert!(
            result.is_ok(),
            "Deploy should accept exactly 1024 pages: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_deploy_accepts_small_memory() {
        let mut rt = ContractRuntime::new();
        // Create a WASM module with 1 page (64KB)
        let wasm = wasm_with_memory(1, None);
        let result = rt.deploy(&wasm);
        assert!(
            result.is_ok(),
            "Deploy should accept 1-page memory: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_deploy_accepts_default_memory() {
        let mut rt = ContractRuntime::new();
        // Create a WASM module with 16 pages (1MB = default)
        let wasm = wasm_with_memory(DEFAULT_WASM_MEMORY_PAGES, Some(MAX_WASM_MEMORY_PAGES));
        let result = rt.deploy(&wasm);
        assert!(
            result.is_ok(),
            "Deploy should accept default memory: {:?}",
            result.err()
        );
    }

    // ── Task 4.3 (M-4): Contract Storage Protocol Enforcement ───────

    #[test]
    fn test_storage_bytes_tracking_new_context() {
        let ctx = ContractContext::new(Pubkey([1u8; 32]), Pubkey([2u8; 32]), 0, 0);
        assert_eq!(ctx.storage_bytes_used, 0);
    }

    #[test]
    fn test_storage_bytes_tracking_with_storage() {
        let mut storage = HashMap::new();
        storage.insert(b"key1".to_vec(), b"value1".to_vec()); // 4 + 6 = 10
        storage.insert(b"key2".to_vec(), b"val".to_vec()); // 4 + 3 = 7
        let ctx =
            ContractContext::with_storage(Pubkey([1u8; 32]), Pubkey([2u8; 32]), 0, 0, storage);
        assert_eq!(ctx.storage_bytes_used, 17);
    }

    #[test]
    fn test_storage_bytes_tracking_with_args() {
        let mut storage = HashMap::new();
        storage.insert(b"k".to_vec(), b"v".to_vec()); // 1 + 1 = 2
        let ctx = ContractContext::with_args(
            Pubkey([1u8; 32]),
            Pubkey([2u8; 32]),
            0,
            0,
            storage,
            vec![1, 2, 3],
        );
        assert_eq!(ctx.storage_bytes_used, 2);
    }

    #[test]
    fn test_prepare_execution_context_preserves_live_storage() {
        let mut context = ContractContext::with_args(
            Pubkey::new([1u8; 32]),
            Pubkey::new([2u8; 32]),
            0,
            1,
            HashMap::from([(b"key".to_vec(), b"cf-value".to_vec())]),
            vec![1, 2, 3],
        );
        context
            .cross_contract_storage
            .insert(b"overlay-only".to_vec(), b"ccc".to_vec());
        context
            .cross_contract_storage
            .insert(b"key".to_vec(), b"embedded-should-not-win".to_vec());

        let prepared = ContractRuntime::prepare_execution_context(context, &[9, 8]);

        assert_eq!(
            prepared.storage.get(b"key" as &[u8]),
            Some(&b"cf-value".to_vec())
        );
        assert_eq!(
            prepared.storage.get(b"overlay-only" as &[u8]),
            Some(&b"ccc".to_vec())
        );
        assert_eq!(prepared.args, vec![9, 8]);
    }

    #[test]
    fn test_per_byte_storage_write_cost() {
        // COMPUTE_STORAGE_WRITE (200) + val_len (100) * COMPUTE_STORAGE_WRITE_PER_BYTE (1) = 300
        let expected = COMPUTE_STORAGE_WRITE + 100 * COMPUTE_STORAGE_WRITE_PER_BYTE;
        assert_eq!(expected, 300);
    }

    #[test]
    fn test_max_total_storage_bytes_constant() {
        assert_eq!(MAX_TOTAL_STORAGE_BYTES, 10 * 1024 * 1024); // 10 MB
    }
}
