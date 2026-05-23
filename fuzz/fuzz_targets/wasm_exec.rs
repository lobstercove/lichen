#![no_main]
use libfuzzer_sys::fuzz_target;
use lichen_core::{ContractAccount, ContractContext, ContractInstruction, ContractRuntime, Pubkey};

fuzz_target!(|data: &[u8]| {
    if data.len() < 33 {
        return;
    }

    let program_id = Pubkey({
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&data[0..32]);
        arr
    });
    let call_data = &data[32..];

    let contract = ContractAccount::new(data.to_vec(), Pubkey([0u8; 32]));
    let mut runtime = ContractRuntime::new();

    let _ = ContractInstruction::deserialize(call_data);
    let _ = ContractInstruction::call("fuzz".to_string(), call_data.to_vec(), 0).serialize();
    let _ = runtime.deploy(data);

    let ctx = ContractContext::new(Pubkey([1u8; 32]), program_id, 0, 0);
    let function_name = std::str::from_utf8(call_data)
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or("fuzz");

    let _ = runtime.execute(&contract, function_name, call_data, ctx);
});
