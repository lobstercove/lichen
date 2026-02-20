// L2-01: Cross-SDK wire format compatibility tests
//
// Verifies that bincode-serialized transactions from the Rust SDK produce
// the same byte layout as the JS and Python SDK manual bincode encoders.
// Also tests JSON (serde_json) round-trip with hex-string signatures.

use moltchain_core::{Hash, Instruction, Message, Pubkey, Transaction};

// ─── Helper: build a reference transaction ───────────────────────────

fn make_test_transaction() -> Transaction {
    let sig = [0xABu8; 64];
    let program_id = Pubkey([1u8; 32]);
    let account1 = Pubkey([2u8; 32]);
    let account2 = Pubkey([3u8; 32]);
    let data = vec![10, 20, 30, 40];
    let blockhash = Hash::new([0xFFu8; 32]);

    let ix = Instruction {
        program_id,
        accounts: vec![account1, account2],
        data,
    };

    Transaction {
        signatures: vec![sig],
        message: Message::new(vec![ix], blockhash),
    }
}

// ─── Helper: manually build bincode bytes matching JS/Python layout ──
//
// Layout (matching sdk/js/src/bincode.ts and sdk/python/moltchain/bincode.py):
//   signatures: u64_le(count) + N * 64_raw_bytes
//   message.instructions: u64_le(count) + N * instruction
//   instruction: 32_bytes(program_id) + u64_le(accounts_count) + N*32 + u64_le(data_len) + data
//   message.recent_blockhash: 32_raw_bytes

fn encode_u64_le(v: u64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

fn build_expected_bincode(tx: &Transaction) -> Vec<u8> {
    let mut out = Vec::new();

    // Signatures: Vec<[u8; 64]> → u64 count + N * 64 raw bytes
    out.extend(encode_u64_le(tx.signatures.len() as u64));
    for sig in &tx.signatures {
        out.extend_from_slice(sig);
    }

    // Message.instructions: Vec<Instruction>
    out.extend(encode_u64_le(tx.message.instructions.len() as u64));
    for ix in &tx.message.instructions {
        // program_id: Pubkey([u8; 32]) — newtype, bincode writes inner array flat
        out.extend_from_slice(&ix.program_id.0);

        // accounts: Vec<Pubkey> → u64 count + N * 32 raw bytes
        out.extend(encode_u64_le(ix.accounts.len() as u64));
        for acct in &ix.accounts {
            out.extend_from_slice(&acct.0);
        }

        // data: Vec<u8> → u64 length + bytes
        out.extend(encode_u64_le(ix.data.len() as u64));
        out.extend_from_slice(&ix.data);
    }

    // recent_blockhash: Hash([u8; 32]) — newtype, bincode writes inner array flat
    out.extend_from_slice(&tx.message.recent_blockhash.0);

    out
}

// ═══════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════

#[test]
fn test_bincode_matches_sdk_layout() {
    // The Rust bincode::serialize output must match the expected byte layout
    // that JS and Python SDKs produce with their manual encoders.
    let tx = make_test_transaction();
    let rust_bincode = bincode::serialize(&tx).unwrap();
    let expected = build_expected_bincode(&tx);

    assert_eq!(
        rust_bincode,
        expected,
        "Rust bincode output does not match JS/Python SDK byte layout.\n\
         Rust bincode ({} bytes): {:?}\n\
         Expected     ({} bytes): {:?}",
        rust_bincode.len(),
        &rust_bincode[..rust_bincode.len().min(128)],
        expected.len(),
        &expected[..expected.len().min(128)],
    );
}

#[test]
fn test_bincode_round_trip() {
    let tx = make_test_transaction();
    let bytes = bincode::serialize(&tx).unwrap();
    let tx2: Transaction = bincode::deserialize(&bytes).unwrap();

    assert_eq!(tx.signatures.len(), tx2.signatures.len());
    assert_eq!(tx.signatures[0], tx2.signatures[0]);
    assert_eq!(
        tx.message.instructions.len(),
        tx2.message.instructions.len()
    );
    assert_eq!(
        tx.message.instructions[0].program_id,
        tx2.message.instructions[0].program_id
    );
    assert_eq!(
        tx.message.instructions[0].accounts,
        tx2.message.instructions[0].accounts
    );
    assert_eq!(
        tx.message.instructions[0].data,
        tx2.message.instructions[0].data
    );
    assert_eq!(tx.message.recent_blockhash, tx2.message.recent_blockhash);
}

#[test]
fn test_json_round_trip_with_hex_signatures() {
    let tx = make_test_transaction();
    let json_str = serde_json::to_string(&tx).unwrap();

    // Verify JSON uses hex strings for signatures
    let json_val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let sigs = json_val["signatures"].as_array().unwrap();
    assert_eq!(sigs.len(), 1);
    assert!(sigs[0].is_string(), "JSON signature should be a hex string");
    let sig_hex = sigs[0].as_str().unwrap();
    assert_eq!(
        sig_hex.len(),
        128,
        "Hex-encoded 64-byte signature should be 128 chars"
    );
    assert_eq!(sig_hex, "ab".repeat(64));

    // Deserialize back
    let tx2: Transaction = serde_json::from_str(&json_str).unwrap();
    assert_eq!(tx.signatures[0], tx2.signatures[0]);
    assert_eq!(tx.message.recent_blockhash, tx2.message.recent_blockhash);
}

#[test]
fn test_bincode_and_json_produce_different_bytes() {
    // Sanity check: bincode and JSON should produce different byte arrays
    let tx = make_test_transaction();
    let bincode_bytes = bincode::serialize(&tx).unwrap();
    let json_bytes = serde_json::to_vec(&tx).unwrap();

    assert_ne!(bincode_bytes, json_bytes);
}

#[test]
fn test_bincode_signature_encoding_is_raw_bytes() {
    let tx = make_test_transaction();
    let bytes = bincode::serialize(&tx).unwrap();

    // First 8 bytes: u64 LE count of signatures = 1
    let sig_count = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    assert_eq!(sig_count, 1);

    // Next 64 bytes: raw signature bytes (should be 0xAB repeated)
    let sig_bytes = &bytes[8..72];
    assert_eq!(sig_bytes, &[0xAB; 64]);
}

#[test]
fn test_message_serialize_for_signing_matches_bincode() {
    // The Message::serialize() method (used for signing) must produce the same
    // bytes as a standalone bincode::serialize(&message), so that signing bytes
    // are consistent regardless of the code path.
    let tx = make_test_transaction();
    let sign_bytes = tx.message.serialize();
    let bincode_bytes = bincode::serialize(&tx.message).unwrap();
    assert_eq!(sign_bytes, bincode_bytes);
}

#[test]
fn test_multiple_signatures() {
    let sig1 = [0x11u8; 64];
    let sig2 = [0x22u8; 64];
    let ix = Instruction {
        program_id: Pubkey([1u8; 32]),
        accounts: vec![],
        data: vec![],
    };
    let tx = Transaction {
        signatures: vec![sig1, sig2],
        message: Message::new(vec![ix], Hash::default()),
    };

    let bytes = bincode::serialize(&tx).unwrap();
    let expected = build_expected_bincode(&tx);
    assert_eq!(bytes, expected);

    // Round-trip
    let tx2: Transaction = bincode::deserialize(&bytes).unwrap();
    assert_eq!(tx2.signatures.len(), 2);
    assert_eq!(tx2.signatures[0], sig1);
    assert_eq!(tx2.signatures[1], sig2);
}

#[test]
fn test_empty_signatures() {
    let ix = Instruction {
        program_id: Pubkey([1u8; 32]),
        accounts: vec![],
        data: vec![],
    };
    let tx = Transaction {
        signatures: vec![],
        message: Message::new(vec![ix], Hash::default()),
    };

    let bytes = bincode::serialize(&tx).unwrap();
    let expected = build_expected_bincode(&tx);
    assert_eq!(bytes, expected);

    let tx2: Transaction = bincode::deserialize(&bytes).unwrap();
    assert_eq!(tx2.signatures.len(), 0);
}

#[test]
fn test_multiple_instructions() {
    let ix1 = Instruction {
        program_id: Pubkey([1u8; 32]),
        accounts: vec![Pubkey([10u8; 32])],
        data: vec![1, 2, 3],
    };
    let ix2 = Instruction {
        program_id: Pubkey([4u8; 32]),
        accounts: vec![Pubkey([5u8; 32]), Pubkey([6u8; 32]), Pubkey([7u8; 32])],
        data: vec![100, 200],
    };
    let tx = Transaction {
        signatures: vec![[0xCCu8; 64]],
        message: Message::new(vec![ix1, ix2], Hash::new([0xDDu8; 32])),
    };

    let bytes = bincode::serialize(&tx).unwrap();
    let expected = build_expected_bincode(&tx);
    assert_eq!(bytes, expected);

    let tx2: Transaction = bincode::deserialize(&bytes).unwrap();
    assert_eq!(tx2.message.instructions.len(), 2);
    assert_eq!(tx2.message.instructions[1].accounts.len(), 3);
}

#[test]
fn test_simulated_js_sdk_bytes_deserialize() {
    // Simulate what the JS SDK would produce: manually build bincode bytes
    // and verify Rust can deserialize them.
    let sig = [0x42u8; 64];
    let program_id = Pubkey([0xAAu8; 32]);
    let account = Pubkey([0xBBu8; 32]);
    let data = vec![1, 2, 3, 4, 5];
    let blockhash = Hash::new([0xCCu8; 32]);

    // Manually build what JS encodeTransaction would produce
    let mut js_bytes = Vec::new();
    // signatures: Vec<[u8; 64]>
    js_bytes.extend(encode_u64_le(1)); // 1 signature
    js_bytes.extend_from_slice(&sig);
    // instructions: Vec<Instruction>
    js_bytes.extend(encode_u64_le(1)); // 1 instruction
                                       // instruction.program_id
    js_bytes.extend_from_slice(&program_id.0);
    // instruction.accounts: Vec<Pubkey>
    js_bytes.extend(encode_u64_le(1)); // 1 account
    js_bytes.extend_from_slice(&account.0);
    // instruction.data: Vec<u8>
    js_bytes.extend(encode_u64_le(5)); // 5 bytes
    js_bytes.extend_from_slice(&data);
    // recent_blockhash
    js_bytes.extend_from_slice(&blockhash.0);

    // This must deserialize successfully
    let tx: Transaction =
        bincode::deserialize(&js_bytes).expect("Failed to deserialize JS SDK bincode bytes");

    assert_eq!(tx.signatures.len(), 1);
    assert_eq!(tx.signatures[0], sig);
    assert_eq!(tx.message.instructions.len(), 1);
    assert_eq!(tx.message.instructions[0].program_id, program_id);
    assert_eq!(tx.message.instructions[0].accounts, vec![account]);
    assert_eq!(tx.message.instructions[0].data, data);
    assert_eq!(tx.message.recent_blockhash, blockhash);
}

#[test]
fn test_json_backward_compat_hex_signatures() {
    // Verify we can still deserialize JSON with hex-string signatures
    // (used by browser wallets)
    // 64 bytes = 128 hex chars of "ab" repeated
    let sig_hex = "ab".repeat(64);
    let json = format!(
        r#"{{
        "signatures": ["{}"],
        "message": {{
            "instructions": [{{
                "program_id": [1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],
                "accounts": [],
                "data": []
            }}],
            "recent_blockhash": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
        }}
    }}"#,
        sig_hex
    );

    let tx: Transaction = serde_json::from_str(&json).unwrap();
    assert_eq!(tx.signatures.len(), 1);
    assert_eq!(tx.signatures[0], [0xABu8; 64]);
}
