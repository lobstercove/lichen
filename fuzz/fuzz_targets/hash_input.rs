#![no_main]
use libfuzzer_sys::fuzz_target;
use moltchain_core::Hash;

fuzz_target!(|data: &[u8]| {
    // Fuzz hash computation — must never panic on any input size.
    let h = Hash::hash(data);

    // Hash should be deterministic
    let h2 = Hash::hash(data);
    assert_eq!(h, h2, "Hash must be deterministic");

    // Empty vs non-empty should differ (unless data is empty)
    if !data.is_empty() {
        let empty_hash = Hash::hash(&[]);
        assert_ne!(h, empty_hash, "Non-empty input should not hash to empty hash");
    }
});
