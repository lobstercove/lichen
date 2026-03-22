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
        tx_type: Default::default(),
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

    // compute_budget: Option<u64> — bincode encodes as 0x00 (None) or 0x01 + 8-byte LE (Some)
    match tx.message.compute_budget {
        None => out.push(0x00),
        Some(v) => {
            out.push(0x01);
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    // compute_unit_price: Option<u64> — same encoding as above
    match tx.message.compute_unit_price {
        None => out.push(0x00),
        Some(v) => {
            out.push(0x01);
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    // tx_type: enum variant index as u32 LE (bincode default)
    // Native = 0, Evm = 1, SolanaCompat = 2
    let variant = match tx.tx_type {
        moltchain_core::TransactionType::Native => 0u32,
        moltchain_core::TransactionType::Evm => 1u32,
        moltchain_core::TransactionType::SolanaCompat => 2u32,
    };
    out.extend_from_slice(&variant.to_le_bytes());

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
        tx_type: Default::default(),
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
        tx_type: Default::default(),
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
        tx_type: Default::default(),
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
    // compute_budget: Option<u64> = None (0x00)
    js_bytes.push(0x00);
    // compute_unit_price: Option<u64> = None (0x00)
    js_bytes.push(0x00);
    // tx_type: Native = variant 0 (u32 LE)
    js_bytes.extend_from_slice(&0u32.to_le_bytes());

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

// ═══════════════════════════════════════════════════════════════════
// M-6: Wire-format envelope tests
// ═══════════════════════════════════════════════════════════════════

const MAX_TEST_LIMIT: u64 = 4 * 1024 * 1024;

#[test]
fn test_wire_envelope_round_trip_native() {
    let tx = make_test_transaction();
    let wire = tx.to_wire();

    // Check envelope header
    assert_eq!(&wire[0..2], &moltchain_core::TX_WIRE_MAGIC);
    assert_eq!(wire[2], moltchain_core::TX_WIRE_VERSION);
    assert_eq!(wire[3], 0); // Native = 0

    // Round-trip
    let tx2 = Transaction::from_wire(&wire, MAX_TEST_LIMIT).unwrap();
    assert_eq!(tx2.signatures, tx.signatures);
    assert_eq!(tx2.message.recent_blockhash, tx.message.recent_blockhash);
    assert_eq!(tx2.tx_type, moltchain_core::TransactionType::Native);
}

#[test]
fn test_wire_envelope_round_trip_evm() {
    let ix = moltchain_core::Instruction {
        program_id: Pubkey([0xEE; 32]),
        accounts: vec![Pubkey([2; 32])],
        data: vec![1, 2, 3],
    };
    let msg = moltchain_core::Message::new(vec![ix], Hash::default());
    let tx = Transaction {
        signatures: vec![[0x11; 64]],
        message: msg,
        tx_type: moltchain_core::TransactionType::Evm,
    };
    let wire = tx.to_wire();
    assert_eq!(wire[3], 1); // Evm = 1

    let tx2 = Transaction::from_wire(&wire, MAX_TEST_LIMIT).unwrap();
    assert_eq!(tx2.tx_type, moltchain_core::TransactionType::Evm);
}

#[test]
fn test_wire_envelope_round_trip_solana_compat() {
    let ix = moltchain_core::Instruction {
        program_id: Pubkey([1; 32]),
        accounts: vec![],
        data: vec![],
    };
    let msg = moltchain_core::Message::new(vec![ix], Hash::default());
    let tx = Transaction {
        signatures: vec![[0xCC; 64]],
        message: msg,
        tx_type: moltchain_core::TransactionType::SolanaCompat,
    };
    let wire = tx.to_wire();
    assert_eq!(wire[3], 2); // SolanaCompat = 2

    let tx2 = Transaction::from_wire(&wire, MAX_TEST_LIMIT).unwrap();
    assert_eq!(tx2.tx_type, moltchain_core::TransactionType::SolanaCompat);
}

#[test]
fn test_wire_envelope_backward_compat_legacy_bincode() {
    // Legacy format: raw bincode without envelope header
    let tx = make_test_transaction();
    let legacy = bincode::serialize(&tx).unwrap();

    // First two bytes should NOT be the magic (they're the sig count u64 LE)
    assert_ne!(&legacy[0..2], &moltchain_core::TX_WIRE_MAGIC);

    // from_wire must still decode legacy bincode
    let tx2 = Transaction::from_wire(&legacy, MAX_TEST_LIMIT).unwrap();
    assert_eq!(tx2.signatures, tx.signatures);
    assert_eq!(tx2.message.recent_blockhash, tx.message.recent_blockhash);
}

#[test]
fn test_wire_envelope_backward_compat_json() {
    // Legacy JSON format: serde_json serialized Transaction
    let tx = make_test_transaction();
    let json_bytes = serde_json::to_vec(&tx).unwrap();

    // from_wire must decode JSON too
    let tx2 = Transaction::from_wire(&json_bytes, MAX_TEST_LIMIT).unwrap();
    assert_eq!(tx2.signatures, tx.signatures);
    assert_eq!(tx2.message.recent_blockhash, tx.message.recent_blockhash);
}

#[test]
fn test_wire_envelope_unsupported_version() {
    let tx = make_test_transaction();
    let mut wire = tx.to_wire();
    wire[2] = 99; // bad version

    let result = Transaction::from_wire(&wire, MAX_TEST_LIMIT);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unsupported wire version"));
}

#[test]
fn test_wire_envelope_unknown_type() {
    let tx = make_test_transaction();
    let mut wire = tx.to_wire();
    wire[3] = 255; // unknown type

    let result = Transaction::from_wire(&wire, MAX_TEST_LIMIT);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Unknown transaction type"));
}

#[test]
fn test_wire_envelope_corrupt_payload() {
    // Valid header but corrupt bincode payload
    let mut wire = vec![0x4D, 0x54, 1, 0]; // magic + version 1 + Native
    wire.extend_from_slice(&[0xFF; 32]); // garbage

    let result = Transaction::from_wire(&wire, MAX_TEST_LIMIT);
    assert!(result.is_err());
}

#[test]
fn test_wire_envelope_too_short() {
    // Less than 4-byte header but starts with magic
    let wire = vec![0x4D, 0x54, 1]; // only 3 bytes
    let result = Transaction::from_wire(&wire, MAX_TEST_LIMIT);
    // Should fall through to legacy path (which also fails)
    assert!(result.is_err());
}

#[test]
fn test_wire_envelope_type_overrides_payload() {
    // Envelope says Evm, but payload was serialized as Native.
    // Envelope type is authoritative.
    let tx = make_test_transaction(); // Native
    let payload = bincode::serialize(&tx).unwrap();
    let mut wire = vec![0x4D, 0x54, 1, 1]; // magic + v1 + Evm
    wire.extend_from_slice(&payload);

    let tx2 = Transaction::from_wire(&wire, MAX_TEST_LIMIT).unwrap();
    assert_eq!(tx2.tx_type, moltchain_core::TransactionType::Evm);
}

#[test]
fn test_wire_envelope_size_matches() {
    let tx = make_test_transaction();
    let legacy = bincode::serialize(&tx).unwrap();
    let wire = tx.to_wire();

    // Wire = 4 (header) + legacy bincode
    assert_eq!(wire.len(), 4 + legacy.len());
    assert_eq!(&wire[4..], &legacy[..]);
}

// ─── Task 4.1: Transaction Hash Determinism (H-7) ───────────────────

#[test]
fn test_hash_determinism_same_tx() {
    let tx = make_test_transaction();
    let h1 = tx.hash();
    let h2 = tx.hash();
    assert_eq!(h1, h2, "Same transaction must always produce the same hash");
}

#[test]
fn test_hash_determinism_cloned_tx() {
    let tx = make_test_transaction();
    let tx2 = tx.clone();
    assert_eq!(
        tx.hash(),
        tx2.hash(),
        "Cloned transaction must hash identically"
    );
}

#[test]
fn test_hash_determinism_reconstructed_tx() {
    // Build the same transaction from scratch twice
    let tx1 = make_test_transaction();
    let tx2 = make_test_transaction();
    assert_eq!(
        tx1.hash(),
        tx2.hash(),
        "Independently constructed identical transactions must hash identically"
    );
}

#[test]
fn test_hash_includes_signatures() {
    let tx1 = make_test_transaction();
    let mut tx2 = make_test_transaction();
    tx2.signatures = vec![[0xCDu8; 64]]; // different signature

    assert_ne!(
        tx1.hash(),
        tx2.hash(),
        "Transactions with different signatures must have different hashes"
    );
}

#[test]
fn test_message_hash_excludes_signatures() {
    let tx1 = make_test_transaction();
    let mut tx2 = make_test_transaction();
    tx2.signatures = vec![[0xCDu8; 64]]; // different signature

    assert_eq!(
        tx1.message_hash(),
        tx2.message_hash(),
        "message_hash must be signature-independent"
    );
}

#[test]
fn test_message_hash_differs_from_tx_hash() {
    let tx = make_test_transaction();
    assert_ne!(
        tx.hash(),
        tx.message_hash(),
        "tx hash (includes sigs) must differ from message hash (excludes sigs)"
    );
}

#[test]
fn test_hash_differs_with_different_message() {
    let tx1 = make_test_transaction();
    let mut tx2 = make_test_transaction();
    tx2.message.recent_blockhash = Hash::new([0x01u8; 32]);

    assert_ne!(tx1.hash(), tx2.hash());
    assert_ne!(tx1.message_hash(), tx2.message_hash());
}

#[test]
fn test_hash_signature_order_matters() {
    let sig_a = [0xAAu8; 64];
    let sig_b = [0xBBu8; 64];
    let blockhash = Hash::new([0xFFu8; 32]);
    let ix = Instruction {
        program_id: Pubkey([1u8; 32]),
        accounts: vec![Pubkey([2u8; 32])],
        data: vec![1],
    };

    let tx1 = Transaction {
        signatures: vec![sig_a, sig_b],
        message: Message::new(vec![ix.clone()], blockhash),
        tx_type: Default::default(),
    };
    let tx2 = Transaction {
        signatures: vec![sig_b, sig_a],
        message: Message::new(vec![ix], blockhash),
        tx_type: Default::default(),
    };

    assert_ne!(
        tx1.hash(),
        tx2.hash(),
        "Signature order must affect the transaction hash"
    );
    // But message hash is the same since message is identical
    assert_eq!(tx1.message_hash(), tx2.message_hash());
}
