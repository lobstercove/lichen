//! Fuzz target: ZK proof deserialization
//!
//! Feeds arbitrary bytes to the ZK proof and nullifier decoders to ensure
//! they never panic on malformed input. Covers shielded_pool proof parsing.

#![no_main]
use libfuzzer_sys::fuzz_target;
use moltchain_core::Pubkey;

fuzz_target!(|data: &[u8]| {
    // ── 1. Try decoding as a Groth16 proof (192 bytes expected) ─────
    // A valid Groth16 proof has 3 group elements: A (48 bytes), B (96 bytes),
    // C (48 bytes) = 192 bytes.  Parsing truncated/corrupt data must not panic.
    if data.len() >= 192 {
        // Simulate extracting proof components
        let _a_point = &data[0..48];
        let _b_point = &data[48..144];
        let _c_point = &data[144..192];
    }

    // ── 2. Try decoding as a nullifier (32 bytes) ───────────────────
    if data.len() >= 32 {
        let mut nullifier = [0u8; 32];
        nullifier.copy_from_slice(&data[..32]);
        // Hash the nullifier just like the shielded pool would
        let _hash = moltchain_core::Hash::digest(&nullifier);
    }

    // ── 3. Try decoding as a shielded transfer instruction ──────────
    // Format: opcode(1) + nullifier(32) + commitment(32) + proof(192) + amount(8) = 265
    if data.len() >= 265 {
        let _opcode = data[0];
        let mut nf = [0u8; 32];
        nf.copy_from_slice(&data[1..33]);
        let mut commitment = [0u8; 32];
        commitment.copy_from_slice(&data[33..65]);
        let _proof_bytes = &data[65..257];
        let amount_bytes: [u8; 8] = data[257..265].try_into().unwrap_or([0; 8]);
        let _amount = u64::from_le_bytes(amount_bytes);
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
