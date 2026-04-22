use anyhow::Result;

use crate::output_support::print_json;

pub(super) fn handle_host_functions(json_output: bool) -> Result<()> {
    let host_functions = serde_json::json!({
        "host_functions": [
            {"name": "storage_read", "signature": "(key_ptr: u32, key_len: u32) -> u32", "category": "storage", "description": "Read a value from contract storage. Returns value length or 0 if not found."},
            {"name": "storage_read_result", "signature": "(buf_ptr: u32, buf_len: u32) -> u32", "category": "storage", "description": "Copy the last storage_read result into a buffer."},
            {"name": "storage_write", "signature": "(key_ptr: u32, key_len: u32, val_ptr: u32, val_len: u32)", "category": "storage", "description": "Write a key-value pair to contract storage."},
            {"name": "storage_delete", "signature": "(key_ptr: u32, key_len: u32)", "category": "storage", "description": "Delete a key from contract storage."},
            {"name": "log", "signature": "(msg_ptr: u32, msg_len: u32)", "category": "logging", "description": "Write a log message (visible in transaction logs)."},
            {"name": "emit_event", "signature": "(name_ptr: u32, name_len: u32, data_ptr: u32, data_len: u32)", "category": "logging", "description": "Emit a named event with data payload."},
            {"name": "get_timestamp", "signature": "() -> u64", "category": "chain", "description": "Get the current block timestamp (Unix seconds)."},
            {"name": "get_caller", "signature": "(buf_ptr: u32) -> u32", "category": "chain", "description": "Get the caller's 32-byte public key."},
            {"name": "get_contract_address", "signature": "(buf_ptr: u32) -> u32", "category": "chain", "description": "Get this contract's own 32-byte address."},
            {"name": "get_value", "signature": "() -> u64", "category": "chain", "description": "Get the LICN value (spores) attached to this call."},
            {"name": "get_slot", "signature": "() -> u64", "category": "chain", "description": "Get the current block slot number."},
            {"name": "get_args_len", "signature": "() -> u32", "category": "arguments", "description": "Get the length of the call arguments in bytes."},
            {"name": "get_args", "signature": "(buf_ptr: u32, buf_len: u32) -> u32", "category": "arguments", "description": "Copy call arguments into a buffer."},
            {"name": "set_return_data", "signature": "(data_ptr: u32, data_len: u32)", "category": "arguments", "description": "Set the return data for this contract call."},
            {"name": "cross_contract_call", "signature": "(addr_ptr: u32, fn_ptr: u32, fn_len: u32, args_ptr: u32, args_len: u32, value: u64) -> i32", "category": "interop", "description": "Call another contract. Returns 0 on success, -1 on error."},
            {"name": "host_poseidon_hash", "signature": "(left_ptr: u32, right_ptr: u32, out_ptr: u32) -> u32", "category": "crypto", "description": "Compute Poseidon hash of two 32-byte field elements. ZK-friendly."}
        ],
        "sdk_crate": "lichen-contract-sdk",
        "compile_target": "wasm32-unknown-unknown",
        "build_command": "cargo build --target wasm32-unknown-unknown --release"
    });

    if json_output {
        print_json(&host_functions);
    } else {
        println!("🔧 WASM Host Functions (available in contracts)");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("SDK: lichen-contract-sdk | Target: wasm32-unknown-unknown");
        println!();
        println!("  Storage (4):");
        println!("    storage_read(key_ptr, key_len) -> u32");
        println!("    storage_read_result(buf_ptr, buf_len) -> u32");
        println!("    storage_write(key_ptr, key_len, val_ptr, val_len)");
        println!("    storage_delete(key_ptr, key_len)");
        println!();
        println!("  Logging (2):");
        println!("    log(msg_ptr, msg_len)");
        println!("    emit_event(name_ptr, name_len, data_ptr, data_len)");
        println!();
        println!("  Chain Introspection (5):");
        println!("    get_timestamp() -> u64");
        println!("    get_caller(buf_ptr) -> u32");
        println!("    get_contract_address(buf_ptr) -> u32");
        println!("    get_value() -> u64");
        println!("    get_slot() -> u64");
        println!();
        println!("  Arguments & Returns (3):");
        println!("    get_args_len() -> u32");
        println!("    get_args(buf_ptr, buf_len) -> u32");
        println!("    set_return_data(data_ptr, data_len)");
        println!();
        println!("  Cross-Contract (1):");
        println!("    cross_contract_call(addr, fn, fn_len, args, args_len, value) -> i32");
        println!();
        println!("  Cryptography (1):");
        println!("    host_poseidon_hash(left_ptr, right_ptr, out_ptr) -> u32");
    }

    Ok(())
}
