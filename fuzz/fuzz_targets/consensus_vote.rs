#![no_main]
use libfuzzer_sys::fuzz_target;
use moltchain_core::consensus::{Vote, ForkChoice};

fuzz_target!(|data: &[u8]| {
    // Fuzz Vote deserialization — consensus-critical, must never panic.
    let _ = serde_json::from_slice::<Vote>(data);
    let _ = bincode::deserialize::<Vote>(data);

    // Fuzz ForkChoice deserialization
    let _ = serde_json::from_slice::<ForkChoice>(data);
    let _ = bincode::deserialize::<ForkChoice>(data);
});
