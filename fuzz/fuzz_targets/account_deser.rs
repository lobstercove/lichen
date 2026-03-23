#![no_main]
use libfuzzer_sys::fuzz_target;
use lichen_core::{Account, Pubkey};

fuzz_target!(|data: &[u8]| {
    // Fuzz account deserialization — must never panic.
    let _ = serde_json::from_slice::<Account>(data);
    let _ = bincode::deserialize::<Account>(data);

    // Fuzz Pubkey construction from arbitrary bytes
    if data.len() >= 32 {
        let mut key = [0u8; 32];
        key.copy_from_slice(&data[..32]);
        let pk = Pubkey(key);
        let _ = format!("{:?}", pk);
    }
});
