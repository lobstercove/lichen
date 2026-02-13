// MoltChain Smart Contract System
// WASM-based programmable contracts with proper host function implementations

use crate::{Hash, Pubkey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use wasmer::{
    imports, CompilerConfig, Function, FunctionEnv, FunctionEnvMut, Instance, Memory, Module, Store,
};
use wasmer_compiler_cranelift::Cranelift;
use wasmer_middlewares::metering::get_remaining_points;
use wasmer_middlewares::metering::MeteringPoints;
use wasmer_middlewares::Metering;

/// Maximum compute units per contract execution (T1.5)
const MAX_WASM_COMPUTE_UNITS: u64 = 1_400_000;
/// Maximum WASM memory pages (1 page = 64KB, 256 pages = 16MB) (T1.9)
const MAX_WASM_MEMORY_PAGES: u32 = 256;

// ============================================================================
// Contract ABI / IDL Schema
// ============================================================================

/// ABI type for function parameters and return values
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AbiType {
    U32,
    U64,
    I32,
    I64,
    // M12 fix: proper float types instead of mapping to U32/U64
    F32,
    F64,
    Bool,
    String,
    Bytes,
    /// 32-byte public key / address (passed as pointer to 32 bytes)
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
    pub params: Vec<AbiParam>,
    /// Return value (None = void)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<AbiReturn>,
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
    /// Value transferred (in shells)
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
/// Maximum storage key length (256 bytes)
const MAX_KEY_LEN: usize = 256;
/// Maximum storage value length (64 KB)
const MAX_VALUE_LEN: usize = 65_536;
/// Maximum return data from a contract call (64 KB)
const MAX_RETURN_DATA: usize = 65_536;
/// Maximum event data JSON size (8 KB)
const MAX_EVENT_DATA: usize = 8_192;
/// Default compute limit per contract call (1 million units)
pub const DEFAULT_COMPUTE_LIMIT: u64 = 1_000_000;
/// Compute cost for a storage read
const COMPUTE_STORAGE_READ: u64 = 100;
/// Compute cost for a storage write
const COMPUTE_STORAGE_WRITE: u64 = 200;
/// Compute cost for a storage delete
const COMPUTE_STORAGE_DELETE: u64 = 100;
/// Compute cost for emitting a log
const COMPUTE_LOG: u64 = 10;
/// Compute cost for emitting an event
const COMPUTE_EVENT: u64 = 50;
// AUDIT-FIX 2.1: Additional compute constants for previously uncharged host functions
const COMPUTE_GET_CALLER: u64 = 100;
const COMPUTE_GET_ARGS: u64 = 50;  // + per-byte cost
const COMPUTE_SET_RETURN_DATA: u64 = 50;  // + per-byte cost
const COMPUTE_READ_RESULT: u64 = 50;  // + per-byte cost
const COMPUTE_BYTE_COST: u64 = 1;

/// Contract runtime - executes WASM bytecode with compute metering
///
/// # Security Sandbox (T2.4)
///
/// The WASM runtime is sandboxed with the following security measures:
///
/// 1. **Compute Metering**: Every WASM instruction costs 1 compute unit.
///    Execution traps after `MAX_WASM_COMPUTE_UNITS` (1.4M) units, preventing
///    infinite loops and DoS via compute exhaustion.
///
/// 2. **Memory Limits**: WASM linear memory is capped at `MAX_WASM_MEMORY_PAGES`
///    (256 pages = 16MB). Contracts declaring or growing memory beyond this
///    limit are rejected at both deploy-time and post-execution.
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
///    - Cross-contract calls: stub (not yet re-entrant)
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

    /// Deploy contract — validate bytecode and enforce sandbox constraints (T2.4).
    ///
    /// Security checks performed:
    /// - Rejects WASI imports (no filesystem/network/syscall access)
    /// - Rejects imports from unauthorized modules (only `"env"` allowed)
    /// - Rejects memory declarations exceeding `MAX_WASM_MEMORY_PAGES` (16MB)
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
        // Load contract's existing storage and args into context
        let mut ctx = context;
        ctx.storage = contract.storage.clone();
        ctx.args = args.to_vec();
        let initial_compute = ctx.compute_remaining;

        let module = Module::new(&self.store, &contract.code)
            .map_err(|e| format!("Failed to compile contract: {}", e))?;

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
                "get_value" => Function::new_typed_with_env(&mut self.store, &env, host_get_value),
                "get_slot" => Function::new_typed_with_env(&mut self.store, &env, host_get_slot),
                // Args & return data
                "get_args_len" => Function::new_typed_with_env(&mut self.store, &env, host_get_args_len),
                "get_args" => Function::new_typed_with_env(&mut self.store, &env, host_get_args),
                "set_return_data" => Function::new_typed_with_env(&mut self.store, &env, host_set_return_data),
                // Cross-contract calls
                "cross_contract_call" => Function::new_typed_with_env(&mut self.store, &env, host_cross_contract_call),
            }
        };

        let instance = Instance::new(&mut self.store, &module, &imports)
            .map_err(|e| format!("Failed to instantiate contract: {}", e))?;

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
            env.as_mut(&mut self.store).memory = Some(memory.clone());
        }

        let func = instance
            .exports
            .get_function(function_name)
            .map_err(|e| format!("Function '{}' not found: {}", function_name, e))?;

        let exec_result = func.call(&mut self.store, &[]);

        // T1.5: Check remaining metering points after execution.
        // If exhausted, the execution already trapped, but we report it clearly.
        let metering_remaining = match get_remaining_points(&mut self.store, &instance) {
            MeteringPoints::Remaining(pts) => pts,
            MeteringPoints::Exhausted => 0,
        };
        let wasm_compute_used = MAX_WASM_COMPUTE_UNITS.saturating_sub(metering_remaining);

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
                });
            }
        }

        let final_ctx = env.as_ref(&self.store);
        // Total compute: host function costs + WASM instruction costs
        let host_compute_used = initial_compute.saturating_sub(final_ctx.compute_remaining);
        let compute_used = host_compute_used.saturating_add(wasm_compute_used);

        // AUDIT-FIX 2.3: Unified compute budget — total (WASM + host) must not exceed the limit.
        // Previously WASM got 1.4M and host got 1M independently (~2.4M effective).
        // Now the combined total is capped at DEFAULT_COMPUTE_LIMIT.
        if compute_used > DEFAULT_COMPUTE_LIMIT {
            return Ok(ContractResult {
                return_data: vec![],
                logs: final_ctx.logs.clone(),
                events: Vec::new(),
                storage_changes: HashMap::new(),
                success: false,
                error: Some(format!(
                    "Contract exceeded unified compute budget: {} > {} (WASM: {}, host: {})",
                    compute_used, DEFAULT_COMPUTE_LIMIT, wasm_compute_used, host_compute_used
                )),
                compute_used,
            });
        }

        match exec_result {
            Ok(_) => Ok(ContractResult {
                return_data: final_ctx.return_data.clone(),
                logs: final_ctx.logs.clone(),
                events: final_ctx.events.clone(),
                storage_changes: final_ctx.storage_changes.clone(),
                success: true,
                error: None,
                compute_used,
            }),
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
                })
            }
        }
    }
}

// ─── Host functions callable from WASM ───────────────────────────────────────

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
                let v = value.clone();
                ctx.last_read_value = v.clone();
                (Some(v), wl)
            }
            None => {
                ctx.last_read_value.clear();
                (None, 0)
            }
        }
    }; // mutable borrow dropped

    // Phase 3: Write value to WASM memory (immutable borrow)
    match found_value {
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
    }
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
    deduct_compute(ctx, COMPUTE_STORAGE_WRITE);
    // AUDIT-FIX 2.2: Enforce storage entry limit per contract
    const MAX_STORAGE_ENTRIES: usize = 10_000;
    if !ctx.storage.contains_key(&key) && ctx.storage.len() >= MAX_STORAGE_ENTRIES {
        return 0; // reject — storage full
    }
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
    if ctx.storage.remove(&key).is_some() {
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
    ctx.logs.push(msg);
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

/// Return the value (shells) transferred with the call.
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

/// Cross-contract call (basic implementation).
/// Reads target address (32 bytes), function name, args, and value.
/// NOTE: Full recursive CCC requires re-entrant execution which is deferred.
/// This implementation returns error status so contracts know it's not yet available
/// for re-entrant calls, but the FFI signature matches the SDK so contracts link correctly.
#[allow(clippy::too_many_arguments)]
fn host_cross_contract_call(
    _env: FunctionEnvMut<ContractContext>,
    _target_ptr: u32,
    _function_ptr: u32,
    _function_len: u32,
    _args_ptr: u32,
    _args_len: u32,
    _value: u64,
    _result_ptr: u32,
    _result_len: u32,
) -> u32 {
    // Return 0 = failure. Cross-contract calls require re-entrant execution
    // which is planned for Phase 2. The import exists so contracts compile.
    0
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
}
