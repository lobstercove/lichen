#![no_main]
use libfuzzer_sys::fuzz_target;
use lichen_core::Transaction;

fuzz_target!(|data: &[u8]| {
    // Try to deserialize arbitrary bytes as a Transaction.
    // This should never panic regardless of the input.
    let _ = serde_json::from_slice::<Transaction>(data);

    // Also try bincode deserialization
    let _ = bincode::deserialize::<Transaction>(data);
});
