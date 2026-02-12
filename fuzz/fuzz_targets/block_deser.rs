#![no_main]
use libfuzzer_sys::fuzz_target;
use moltchain_core::Block;

fuzz_target!(|data: &[u8]| {
    // Fuzz block deserialization — must never panic on arbitrary bytes.
    let _ = serde_json::from_slice::<Block>(data);
    let _ = bincode::deserialize::<Block>(data);

    // Try to verify an arbitrary block (should error, not panic)
    if let Ok(block) = serde_json::from_slice::<Block>(data) {
        let _ = block.hash();
    }
});
