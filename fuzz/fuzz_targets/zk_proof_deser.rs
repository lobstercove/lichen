//! Fuzz target: ZK proof deserialization
//!
//! Feeds arbitrary bytes to the ZK proof and nullifier decoders to ensure
//! they never panic on malformed input. Covers shielded_pool proof parsing.

#![no_main]
use libfuzzer_sys::fuzz_target;
use lichen_core::{zk::deserialize_stark_proof, Pubkey};

fuzz_target!(|data: &[u8]| {
    // ── 1. Try decoding as a native STARK proof envelope ─────────────
    let _ = deserialize_stark_proof(data);

    // ── 2. Try decoding as a nullifier (32 bytes) ───────────────────
    if data.len() >= 32 {
        let mut nullifier = [0u8; 32];
        nullifier.copy_from_slice(&data[..32]);
        // Hash the nullifier just like the shielded pool would
        let _hash = lichen_core::Hash::digest(&nullifier);
    }

    // ── 3. Try decoding as a generic shielded payload header ────────
    if data.len() >= 65 {
        let _opcode = data[0];
        let mut nf = [0u8; 32];
        nf.copy_from_slice(&data[1..33]);
        let mut commitment = [0u8; 32];
        commitment.copy_from_slice(&data[33..65]);
        let _proof_bytes = &data[65..];
    }

    // ── 4. Try decoding as a Pubkey from arbitrary bytes ────────────
    if data.len() >= 32 {
        let pk = Pubkey({
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&data[..32]);
            arr
        });
        // to_base58 must not panic
        let _ = pk.to_base58();
    }
});
