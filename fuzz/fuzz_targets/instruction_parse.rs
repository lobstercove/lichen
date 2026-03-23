#![no_main]
use libfuzzer_sys::fuzz_target;
use lichen_core::{ContractInstruction, Instruction, Pubkey};

fuzz_target!(|data: &[u8]| {
    // Fuzz ContractInstruction deserialization — should never panic.
    let _ = ContractInstruction::deserialize(data);

    // Also fuzz creating an Instruction with arbitrary data
    if data.len() >= 32 {
        let mut program_id = [0u8; 32];
        program_id.copy_from_slice(&data[..32]);
        let _ix = Instruction {
            program_id: Pubkey(program_id),
            accounts: vec![],
            data: data[32..].to_vec(),
        };
    }
});
