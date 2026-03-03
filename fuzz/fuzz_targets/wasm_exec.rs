//! Fuzz target: WASM contract execution engine
//!
//! Feeds arbitrary bytecode to the WASM runtime to verify it never panics
//! on malformed modules. Tests the contract execution sandbox.

#![no_main]
use libfuzzer_sys::fuzz_target;
use moltchain_core::{ContractAccount, ContractContext, ContractInstruction, ContractRuntime, Pubkey};

fuzz_target!(|data: &[u8]| {
    // Minimum viable: need at least 33 bytes for program_id + 1 byte data
    if data.len() < 33 {
        return;
    }

    let program_id = Pubkey({
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&data[0..32]);
        arr
    });
    let call_data = &data[32..];

    // Create a minimal contract with the fuzzed bytecode as if it were WASM
    let contract = ContractAccount {
        owner: Pubkey([0u8; 32]),
        wasm_bytecode: data.to_vec(),
        storage: std::collections::HashMap::new(),
    };

    // Try to execute — must not panic regardless of input
    let instruction = ContractInstruction {
        program_id,
        data: call_data.to_vec(),
        accounts: vec![],
    };

    let ctx = ContractContext {
        caller: Pubkey([1u8; 32]),
        program_id,
        slot: 0,
        timestamp: 0,
    };

    // ContractRuntime::execute may return Err but must never panic
    let _ = ContractRuntime::execute(&contract, &instruction, &ctx);
});
