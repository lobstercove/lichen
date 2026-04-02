// L2-01: Cross-SDK wire format compatibility tests
//
// Verifies that bincode-serialized transactions from the Rust SDK produce
// the same byte layout as the JS and Python SDK manual bincode encoders.
// Also tests JSON (serde_json) round-trip with structured PQ signatures.

use lichen_core::{Hash, Instruction, Keypair, Message, PqSignature, Pubkey, Transaction};

fn test_signature(fill: u8, message: &[u8]) -> PqSignature {
    Keypair::from_seed(&[fill; 32]).sign(message)
}

// ─── Helper: build a reference transaction ───────────────────────────

fn make_test_transaction() -> Transaction {
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

    let message = Message::new(vec![ix], blockhash);

    Transaction {
        signatures: vec![test_signature(0xAB, &message.serialize())],
        message,
        tx_type: Default::default(),
    }
}

// ─── Helper: manually build bincode bytes matching JS/Python layout ──
//
// Layout (to be matched by updated SDK encoders):
//   signatures: u64_le(count) + N * PqSignature
//   PqSignature: u8(scheme) + PqPublicKey + bytes(sig)
//   PqPublicKey: u8(scheme) + bytes(pubkey)
//   message.instructions: u64_le(count) + N * instruction
//   instruction: 32_bytes(program_id) + u64_le(accounts_count) + N*32 + u64_le(data_len) + data
//   message.recent_blockhash: 32_raw_bytes

fn encode_u64_le(v: u64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

fn encode_pq_signature(signature: &PqSignature) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(signature.scheme_version);
    out.push(signature.public_key.scheme_version);
    out.extend(encode_u64_le(signature.public_key.bytes.len() as u64));
    out.extend_from_slice(&signature.public_key.bytes);
    out.extend(encode_u64_le(signature.sig.len() as u64));
    out.extend_from_slice(&signature.sig);
    out
}

fn build_expected_bincode(tx: &Transaction) -> Vec<u8> {
    let mut out = Vec::new();

    // Signatures: Vec<PqSignature>
    out.extend(encode_u64_le(tx.signatures.len() as u64));
    for sig in &tx.signatures {
        out.extend(encode_pq_signature(sig));
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
    // Native = 0, Evm = 1
    let variant = match tx.tx_type {
        lichen_core::TransactionType::Native => 0u32,
        lichen_core::TransactionType::Evm => 1u32,
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
fn test_json_round_trip_with_pq_signatures() {
    let tx = make_test_transaction();
    let json_str = serde_json::to_string(&tx).unwrap();

    // Verify JSON uses structured PQ signature objects with hex-encoded blobs.
    let json_val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let sigs = json_val["signatures"].as_array().unwrap();
    assert_eq!(sigs.len(), 1);
    let signature = sigs[0].as_object().unwrap();
    let public_key = signature["public_key"].as_object().unwrap();
    assert_eq!(signature["scheme_version"].as_u64(), Some(1));
    assert_eq!(public_key["scheme_version"].as_u64(), Some(1));
    assert_eq!(
        public_key["bytes"].as_str().unwrap().len(),
        lichen_core::account::ML_DSA_65_PUBLIC_KEY_BYTES * 2
    );
    assert_eq!(
        signature["sig"].as_str().unwrap().len(),
        lichen_core::account::ML_DSA_65_SIGNATURE_BYTES * 2
    );

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
fn test_bincode_signature_encoding_is_pq_structured_bytes() {
    let tx = make_test_transaction();
    let bytes = bincode::serialize(&tx).unwrap();

    // First 8 bytes: u64 LE count of signatures = 1
    let sig_count = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    assert_eq!(sig_count, 1);

    // Then the self-contained PQ signature layout.
    assert_eq!(bytes[8], tx.signatures[0].scheme_version);
    assert_eq!(bytes[9], tx.signatures[0].public_key.scheme_version);

    let public_key_len = u64::from_le_bytes(bytes[10..18].try_into().unwrap()) as usize;
    assert_eq!(public_key_len, tx.signatures[0].public_key.bytes.len());

    let public_key_start = 18;
    let public_key_end = public_key_start + public_key_len;
    assert_eq!(
        &bytes[public_key_start..public_key_end],
        tx.signatures[0].public_key.bytes.as_slice()
    );

    let sig_len = u64::from_le_bytes(
        bytes[public_key_end..public_key_end + 8]
            .try_into()
            .unwrap(),
    ) as usize;
    assert_eq!(sig_len, tx.signatures[0].sig.len());
    assert_eq!(
        &bytes[public_key_end + 8..public_key_end + 8 + sig_len],
        tx.signatures[0].sig.as_slice()
    );
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
    let ix = Instruction {
        program_id: Pubkey([1u8; 32]),
        accounts: vec![],
        data: vec![],
    };
    let message = Message::new(vec![ix], Hash::default());
    let sig1 = test_signature(0x11, &message.serialize());
    let sig2 = test_signature(0x22, &message.serialize());
    let tx = Transaction {
        signatures: vec![sig1.clone(), sig2.clone()],
        message,
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
    let message = Message::new(vec![ix1, ix2], Hash::new([0xDDu8; 32]));
    let tx = Transaction {
        signatures: vec![test_signature(0xCC, &message.serialize())],
        message,
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
    // Simulate what an updated SDK encoder would produce: manually build bincode bytes
    // and verify Rust can deserialize them.
    let program_id = Pubkey([0xAAu8; 32]);
    let account = Pubkey([0xBBu8; 32]);
    let data = vec![1, 2, 3, 4, 5];
    let blockhash = Hash::new([0xCCu8; 32]);
    let message = Message::new(
        vec![Instruction {
            program_id,
            accounts: vec![account],
            data: data.clone(),
        }],
        blockhash,
    );
    let signature = test_signature(0x42, &message.serialize());

    // Manually build what a PQ-aware SDK encodeTransaction would produce.
    let mut js_bytes = Vec::new();
    // signatures: Vec<PqSignature>
    js_bytes.extend(encode_u64_le(1)); // 1 signature
    js_bytes.extend(encode_pq_signature(&signature));
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
    assert_eq!(tx.signatures[0], signature);
    assert_eq!(tx.message.instructions.len(), 1);
    assert_eq!(tx.message.instructions[0].program_id, program_id);
    assert_eq!(tx.message.instructions[0].accounts, vec![account]);
    assert_eq!(tx.message.instructions[0].data, data);
    assert_eq!(tx.message.recent_blockhash, blockhash);
}

#[test]
fn test_json_manual_pq_signature_deserialize() {
    let tx = make_test_transaction();
    let signature = &tx.signatures[0];
    let mut json_val = serde_json::to_value(&tx).unwrap();
    json_val["signatures"] = serde_json::json!([{
        "scheme_version": signature.scheme_version,
        "public_key": {
            "scheme_version": signature.public_key.scheme_version,
            "bytes": hex::encode(&signature.public_key.bytes),
        },
        "sig": hex::encode(&signature.sig),
    }]);

    let tx2: Transaction = serde_json::from_value(json_val).unwrap();
    assert_eq!(tx2.signatures.len(), 1);
    assert_eq!(tx2.signatures[0], tx.signatures[0]);
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
    assert_eq!(&wire[0..2], &lichen_core::TX_WIRE_MAGIC);
    assert_eq!(wire[2], lichen_core::TX_WIRE_VERSION);
    assert_eq!(wire[3], 0); // Native = 0

    // Round-trip
    let tx2 = Transaction::from_wire(&wire, MAX_TEST_LIMIT).unwrap();
    assert_eq!(tx2.signatures, tx.signatures);
    assert_eq!(tx2.message.recent_blockhash, tx.message.recent_blockhash);
    assert_eq!(tx2.tx_type, lichen_core::TransactionType::Native);
}

#[test]
fn test_wire_envelope_round_trip_evm() {
    let ix = lichen_core::Instruction {
        program_id: Pubkey([0xEE; 32]),
        accounts: vec![Pubkey([2; 32])],
        data: vec![1, 2, 3],
    };
    let msg = lichen_core::Message::new(vec![ix], Hash::default());
    let tx = Transaction {
        signatures: vec![test_signature(0x11, &msg.serialize())],
        message: msg,
        tx_type: lichen_core::TransactionType::Evm,
    };
    let wire = tx.to_wire();
    assert_eq!(wire[3], 1); // Evm = 1

    let tx2 = Transaction::from_wire(&wire, MAX_TEST_LIMIT).unwrap();
    assert_eq!(tx2.tx_type, lichen_core::TransactionType::Evm);
}

#[test]
fn test_wire_envelope_accepts_raw_bincode() {
    // Raw bincode without the wire envelope is still accepted.
    let tx = make_test_transaction();
    let raw_bincode = bincode::serialize(&tx).unwrap();

    // First two bytes should NOT be the magic (they're the sig count u64 LE)
    assert_ne!(&raw_bincode[0..2], &lichen_core::TX_WIRE_MAGIC);

    let tx2 = Transaction::from_wire(&raw_bincode, MAX_TEST_LIMIT).unwrap();
    assert_eq!(tx2.signatures, tx.signatures);
    assert_eq!(tx2.message.recent_blockhash, tx.message.recent_blockhash);
}

#[test]
fn test_wire_envelope_accepts_json_payload() {
    // Browser wallet JSON payload.
    let tx = make_test_transaction();
    let json_bytes = serde_json::to_vec(&tx).unwrap();

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
    // Should fall through to the non-envelope decoders (which also fail)
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
    assert_eq!(tx2.tx_type, lichen_core::TransactionType::Evm);
}

#[test]
fn test_wire_envelope_size_matches() {
    let tx = make_test_transaction();
    let raw_bincode = bincode::serialize(&tx).unwrap();
    let wire = tx.to_wire();

    // Wire = 4 (header) + raw bincode
    assert_eq!(wire.len(), 4 + raw_bincode.len());
    assert_eq!(&wire[4..], &raw_bincode[..]);
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
    tx2.signatures = vec![test_signature(0xCD, &tx2.message.serialize())];

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
    tx2.signatures = vec![test_signature(0xCD, &tx2.message.serialize())];

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
    let blockhash = Hash::new([0xFFu8; 32]);
    let ix = Instruction {
        program_id: Pubkey([1u8; 32]),
        accounts: vec![Pubkey([2u8; 32])],
        data: vec![1],
    };
    let message = Message::new(vec![ix.clone()], blockhash);
    let sign_bytes = message.serialize();
    let sig_a = test_signature(0xAA, &sign_bytes);
    let sig_b = test_signature(0xBB, &sign_bytes);

    let tx1 = Transaction {
        signatures: vec![sig_a.clone(), sig_b.clone()],
        message: message.clone(),
        tx_type: Default::default(),
    };
    let tx2 = Transaction {
        signatures: vec![sig_b, sig_a],
        message,
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
