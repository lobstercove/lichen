//! ZK Proof Generator CLI
//!
//! Generates native Plonky3 STARK proofs for shield, unshield, and transfer
//! transactions.  Used by the E2E test suite (Python) to create valid
//! proofs that the validator can verify.
//!
//! Usage:
//!   zk-prove shield   --amount <spores>
//!   zk-prove unshield --amount <spores> --merkle-root <hex> --recipient <hex>
//!                     --blinding <hex> --serial <hex> [--spending-key <hex>]
//!                     [--merkle-path-json <file>] [--path-bits-json <file>]
//!   zk-prove transfer --transfer-json <file>
//!
//! The transfer subcommand reads a JSON file with the full witness:
//!   {
//!     "merkle_root": "<hex>",
//!     "inputs": [
//!       { "amount": <u64>, "blinding": "<hex>", "serial": "<hex>",
//!         "spending_key": "<hex>", "merkle_path": ["<hex>",...],
//!         "path_bits": [bool,...] },
//!       { ... }
//!     ],
//!     "outputs": [
//!       { "amount": <u64> },
//!       { "amount": <u64> }
//!     ]
//!   }
//!
//! Outputs a JSON object to stdout with all values needed to build the
//! on-chain transaction.

use lichen_core::zk::{
    circuits::shield::ShieldCircuit, circuits::transfer::TransferCircuit,
    circuits::unshield::UnshieldCircuit, commitment_hash, nullifier_hash, random_scalar_bytes,
    recipient_hash, recipient_preimage_from_bytes, Prover, Verifier, TREE_DEPTH,
};

use serde_json::json;
use std::{fs, process};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
    }
    let cmd = &args[1];

    match cmd.as_str() {
        "shield" => cmd_shield(&args),
        "unshield" => cmd_unshield(&args),
        "transfer" => cmd_transfer(&args),
        _ => usage(),
    }
}

fn parse_hex_32_or_exit(value: &str, label: &str) -> [u8; 32] {
    let bytes = hex::decode(value).unwrap_or_else(|e| {
        eprintln!("error: invalid {} hex: {}", label, e);
        process::exit(1);
    });
    if bytes.len() != 32 {
        eprintln!("error: {} must be 32 bytes", label);
        process::exit(1);
    }

    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

fn cmd_shield(args: &[String]) {
    let amount: u64 = find_arg(args, "--amount")
        .unwrap_or_else(|| {
            eprintln!("error: --amount is required");
            process::exit(1);
        })
        .parse()
        .unwrap_or_else(|_| {
            eprintln!("error: --amount must be a u64");
            process::exit(1);
        });

    let prover = Prover::new();

    // Generate random blinding / serial / spending key
    let blinding = random_scalar_bytes();
    let serial = random_scalar_bytes();
    let spending_key = random_scalar_bytes();

    let commitment = commitment_hash(amount, &blinding);

    // Build circuit
    let circuit = ShieldCircuit::new_bytes(amount, amount, blinding, commitment);

    // Prove
    let proof = prover.prove_shield(circuit).unwrap_or_else(|e| {
        eprintln!("error: proof generation failed: {}", e);
        process::exit(1);
    });

    // Verify locally
    let verifier = Verifier::new();
    let ok = verifier.verify(&proof).unwrap_or_else(|e| {
        eprintln!("error: proof self-check failed: {}", e);
        process::exit(1);
    });
    assert!(ok, "proof failed self-verification");

    // Output JSON
    let out = json!({
        "type": "shield",
        "amount": amount,
        "commitment": hex::encode(commitment),
        "proof": hex::encode(&proof.proof_bytes),
        "blinding": hex::encode(blinding),
        "serial": hex::encode(serial),
        "spending_key": hex::encode(spending_key),
    });
    println!("{}", serde_json::to_string(&out).unwrap());
}

fn cmd_unshield(args: &[String]) {
    let amount: u64 = find_arg(args, "--amount")
        .unwrap_or_else(|| {
            eprintln!("error: --amount is required");
            process::exit(1);
        })
        .parse()
        .unwrap_or_else(|_| {
            eprintln!("error: --amount must be a u64");
            process::exit(1);
        });

    let merkle_root_hex = find_arg(args, "--merkle-root").unwrap_or_else(|| {
        eprintln!("error: --merkle-root is required");
        process::exit(1);
    });
    let merkle_root_bytes = parse_hex_32_or_exit(&merkle_root_hex, "--merkle-root");

    let recipient_hex = find_arg(args, "--recipient").unwrap_or_else(|| {
        eprintln!("error: --recipient is required");
        process::exit(1);
    });
    let recipient_bytes = parse_hex_32_or_exit(&recipient_hex, "--recipient");

    // Read & parse a previously generated shield's blinding/serial.
    // Accept via --blinding and --serial flags (hex-encoded canonical bytes).
    let blinding_hex = find_arg(args, "--blinding").unwrap_or_else(|| {
        eprintln!("error: --blinding is required (from shield output)");
        process::exit(1);
    });
    let blinding = parse_hex_32_or_exit(&blinding_hex, "--blinding");

    let serial_hex = find_arg(args, "--serial").unwrap_or_else(|| {
        eprintln!("error: --serial is required (from shield output)");
        process::exit(1);
    });
    let serial = parse_hex_32_or_exit(&serial_hex, "--serial");

    // Accept --spending-key (hex) or generate one.
    let spending_key = if let Some(sk_hex) = find_arg(args, "--spending-key") {
        parse_hex_32_or_exit(&sk_hex, "--spending-key")
    } else {
        random_scalar_bytes()
    };

    // Accept --merkle-path-json (file with JSON array of TREE_DEPTH hex siblings)
    // and --path-bits-json (file with JSON array of booleans).
    // For a single-leaf tree (index 0 after one shield), both are all-zeros / all-false.
    let merkle_path_hex: Vec<String> = if let Some(mp_file) = find_arg(args, "--merkle-path-json") {
        let data = fs::read_to_string(&mp_file).unwrap_or_else(|e| {
            eprintln!("error: failed to read {}: {}", mp_file, e);
            process::exit(1);
        });
        serde_json::from_str(&data).unwrap_or_else(|e| {
            eprintln!("error: invalid JSON in {}: {}", mp_file, e);
            process::exit(1);
        })
    } else {
        // Default: TREE_DEPTH zero siblings (leaf at index 0, all siblings are empty)
        vec!["00".repeat(32); TREE_DEPTH]
    };
    let path_bits: Vec<bool> = if let Some(pb_file) = find_arg(args, "--path-bits-json") {
        let data = fs::read_to_string(&pb_file).unwrap_or_else(|e| {
            eprintln!("error: failed to read {}: {}", pb_file, e);
            process::exit(1);
        });
        serde_json::from_str(&data).unwrap_or_else(|e| {
            eprintln!("error: invalid JSON in {}: {}", pb_file, e);
            process::exit(1);
        })
    } else {
        vec![false; TREE_DEPTH]
    };

    let merkle_path: Vec<[u8; 32]> = merkle_path_hex
        .iter()
        .map(|h| parse_hex_32_or_exit(h, "merkle path sibling"))
        .collect();

    let prover = Prover::new();

    let nullifier = nullifier_hash(&serial, &spending_key);
    let recipient_preimage = recipient_preimage_from_bytes(recipient_bytes);
    let recipient_hash = recipient_hash(&recipient_preimage);

    let circuit = UnshieldCircuit::new_bytes(
        merkle_root_bytes,
        nullifier,
        amount,
        recipient_hash,
        amount,
        blinding,
        serial,
        spending_key,
        recipient_preimage,
        merkle_path,
        path_bits,
    );

    let proof = prover.prove_unshield(circuit).unwrap_or_else(|e| {
        eprintln!("error: proof generation failed: {}", e);
        process::exit(1);
    });

    let verifier = Verifier::new();
    let ok = verifier.verify(&proof).unwrap();
    assert!(ok, "proof failed self-verification");

    let out = json!({
        "type": "unshield",
        "amount": amount,
        "nullifier": hex::encode(nullifier),
        "merkle_root": hex::encode(merkle_root_bytes),
        "recipient_hash": hex::encode(recipient_hash),
        "proof": hex::encode(&proof.proof_bytes),
    });
    println!("{}", serde_json::to_string(&out).unwrap());
}

fn find_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn usage() -> ! {
    eprintln!(
        "Usage:\n  \
         zk-prove shield   --amount <spores>\n  \
         zk-prove unshield --amount <spores> --merkle-root <hex> \
                           --recipient <hex> --blinding <hex> --serial <hex>\n  \
         zk-prove transfer --transfer-json <file>"
    );
    process::exit(1);
}

// ─────────────────────────────────────────── Transfer ──────────────────────────

/// JSON schema for the transfer witness file.
#[derive(serde::Deserialize)]
struct TransferWitness {
    merkle_root: String,
    inputs: Vec<TransferInput>,
    outputs: Vec<TransferOutput>,
}

#[derive(serde::Deserialize)]
struct TransferInput {
    amount: u64,
    blinding: String,
    serial: String,
    spending_key: String,
    merkle_path: Vec<String>,
    path_bits: Vec<bool>,
}

#[derive(serde::Deserialize)]
struct TransferOutput {
    amount: u64,
    #[serde(default)]
    blinding: Option<String>,
}

fn cmd_transfer(args: &[String]) {
    let witness_file = find_arg(args, "--transfer-json").unwrap_or_else(|| {
        eprintln!("error: --transfer-json is required");
        process::exit(1);
    });
    let witness_data = fs::read_to_string(&witness_file).unwrap_or_else(|e| {
        eprintln!("error: failed to read {}: {}", witness_file, e);
        process::exit(1);
    });
    let witness: TransferWitness = serde_json::from_str(&witness_data).unwrap_or_else(|e| {
        eprintln!("error: invalid JSON in {}: {}", witness_file, e);
        process::exit(1);
    });

    if witness.inputs.len() != 2 {
        eprintln!(
            "error: transfer requires exactly 2 inputs, got {}",
            witness.inputs.len()
        );
        process::exit(1);
    }
    if witness.outputs.len() != 2 {
        eprintln!(
            "error: transfer requires exactly 2 outputs, got {}",
            witness.outputs.len()
        );
        process::exit(1);
    }

    // Parse merkle root
    let merkle_root_bytes = parse_hex_32_or_exit(&witness.merkle_root, "merkle_root");

    // Parse inputs
    let mut input_values = [0u64; 2];
    let mut input_blindings = [[0u8; 32]; 2];
    let mut input_serials = [[0u8; 32]; 2];
    let mut spending_keys = [[0u8; 32]; 2];
    let mut input_merkle_paths: [Vec<[u8; 32]>; 2] = [vec![], vec![]];
    let mut input_path_bits: [Vec<bool>; 2] = [vec![], vec![]];
    let mut nullifiers = [[0u8; 32]; 2];

    for (i, inp) in witness.inputs.iter().enumerate() {
        input_values[i] = inp.amount;
        input_blindings[i] = parse_hex_32_or_exit(&inp.blinding, &format!("input[{}].blinding", i));
        input_serials[i] = parse_hex_32_or_exit(&inp.serial, &format!("input[{}].serial", i));
        spending_keys[i] =
            parse_hex_32_or_exit(&inp.spending_key, &format!("input[{}].spending_key", i));
        if inp.merkle_path.len() != TREE_DEPTH {
            eprintln!(
                "error: input[{}].merkle_path has {} siblings, expected {}",
                i,
                inp.merkle_path.len(),
                TREE_DEPTH
            );
            process::exit(1);
        }
        input_merkle_paths[i] = inp
            .merkle_path
            .iter()
            .map(|h| parse_hex_32_or_exit(h, &format!("input[{}].merkle_path sibling", i)))
            .collect();
        if inp.path_bits.len() != TREE_DEPTH {
            eprintln!(
                "error: input[{}].path_bits has {} bits, expected {}",
                i,
                inp.path_bits.len(),
                TREE_DEPTH
            );
            process::exit(1);
        }
        input_path_bits[i] = inp.path_bits.clone();

        nullifiers[i] = nullifier_hash(&input_serials[i], &spending_keys[i]);
    }

    // Parse outputs (generate random blinding if not provided)
    let mut output_values = [0u64; 2];
    let mut output_blindings = [[0u8; 32]; 2];
    let mut output_serials = [[0u8; 32]; 2]; // new serial for each output note

    for (j, out) in witness.outputs.iter().enumerate() {
        output_values[j] = out.amount;
        output_blindings[j] = if let Some(ref b_hex) = out.blinding {
            parse_hex_32_or_exit(b_hex, &format!("output[{}].blinding", j))
        } else {
            random_scalar_bytes()
        };
        output_serials[j] = random_scalar_bytes();
    }

    // Value conservation check (client-side, circuit enforces this too)
    let total_in: u64 = input_values.iter().sum();
    let total_out: u64 = output_values.iter().sum();
    if total_in != total_out {
        eprintln!(
            "error: value not conserved: sum(inputs)={} != sum(outputs)={}",
            total_in, total_out
        );
        process::exit(1);
    }

    // Compute output commitments
    let mut output_commitments_bytes = [[0u8; 32]; 2];
    for j in 0..2 {
        output_commitments_bytes[j] = commitment_hash(output_values[j], &output_blindings[j]);
    }

    // Build circuit
    let circuit = TransferCircuit::new_bytes(
        merkle_root_bytes,
        nullifiers,
        output_commitments_bytes,
        input_values,
        input_blindings,
        input_serials,
        spending_keys,
        input_merkle_paths,
        input_path_bits,
        output_values,
        output_blindings,
    );

    let prover = Prover::new();

    // Generate proof
    let proof = prover.prove_transfer(circuit).unwrap_or_else(|e| {
        eprintln!("error: proof generation failed: {}", e);
        process::exit(1);
    });

    // Verify locally
    let verifier = Verifier::new();
    let ok = verifier.verify(&proof).unwrap_or_else(|e| {
        eprintln!("error: proof self-check failed: {}", e);
        process::exit(1);
    });
    assert!(ok, "transfer proof failed self-verification");

    // Output JSON
    let out = json!({
        "type": "transfer",
        "merkle_root": hex::encode(merkle_root_bytes),
        "nullifier_a": hex::encode(nullifiers[0]),
        "nullifier_b": hex::encode(nullifiers[1]),
        "commitment_c": hex::encode(output_commitments_bytes[0]),
        "commitment_d": hex::encode(output_commitments_bytes[1]),
        "proof": hex::encode(&proof.proof_bytes),
        // Output note secrets (needed by recipient to spend later)
        "outputs": [
            {
                "amount": output_values[0],
                "blinding": hex::encode(output_blindings[0]),
                "serial": hex::encode(output_serials[0]),
                "commitment": hex::encode(output_commitments_bytes[0]),
            },
            {
                "amount": output_values[1],
                "blinding": hex::encode(output_blindings[1]),
                "serial": hex::encode(output_serials[1]),
                "commitment": hex::encode(output_commitments_bytes[1]),
            },
        ],
    });
    println!("{}", serde_json::to_string(&out).unwrap());
}
