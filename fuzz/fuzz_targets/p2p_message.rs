//! Fuzz target: P2P message deserialization
//!
//! Feeds arbitrary bytes to the P2P message parser to ensure it never panics
//! on malformed network messages. Covers block propagation, vote gossip,
//! and sync protocol messages.

#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // ── 1. Try bincode deserialization as a P2P message envelope ─────
    // The P2P layer uses bincode for wire format. Corrupt payloads from
    // malicious peers must produce Err, never panic.
    let _ = bincode::deserialize::<moltchain_core::Block>(data);
    let _ = bincode::deserialize::<moltchain_core::Transaction>(data);
    let _ = bincode::deserialize::<moltchain_core::Vote>(data);
    let _ = bincode::deserialize::<moltchain_core::Message>(data);

    // ── 2. Try serde_json deserialization (JSON-RPC peer messages) ───
    let _ = serde_json::from_slice::<serde_json::Value>(data);

    // ── 3. Try decoding as a block header (fixed layout) ────────────
    // BlockHeader: slot(8) + parent_hash(32) + state_root(32) + timestamp(8) + validator(32) = 112
    if data.len() >= 112 {
        let _slot = u64::from_le_bytes(data[0..8].try_into().unwrap_or([0; 8]));
        let mut parent = [0u8; 32];
        parent.copy_from_slice(&data[8..40]);
        let mut state_root = [0u8; 32];
        state_root.copy_from_slice(&data[40..72]);
        let _ts = u64::from_le_bytes(data[72..80].try_into().unwrap_or([0; 8]));
        let mut validator = [0u8; 32];
        validator.copy_from_slice(&data[80..112]);
    }

    // ── 4. Hash::from_hex on arbitrary strings ──────────────────────
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = moltchain_core::Hash::from_hex(s);
    }

    // ── 5. Pubkey::from_base58 on arbitrary strings ─────────────────
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = moltchain_core::Pubkey::from_base58(s);
    }
});
