//! ZK Proof Generator CLI
//!
//! Generates Groth16/BN254 proofs for shield, unshield, and transfer
//! transactions.  Used by the E2E test suite (Python) to create valid
//! proofs that the validator can verify.
//!
//! Usage:
//!   zk-prove shield   --amount <shells> --pk-dir <path>
//!   zk-prove unshield --amount <shells> --pk-dir <path> --merkle-root <hex> --recipient <hex>
//!                     --blinding <hex> --serial <hex> [--spending-key <hex>]
//!                     [--merkle-path-json <file>] [--path-bits-json <file>]
//!   zk-prove transfer --pk-dir <path> --transfer-json <file>
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

use moltchain_core::zk::{
    circuits::shield::ShieldCircuit,
    circuits::transfer::TransferCircuit,
    circuits::unshield::UnshieldCircuit,
    fr_to_bytes, poseidon_hash_fr,
    setup::load_verification_key,
    Prover, Verifier, TREE_DEPTH,
};

use ark_bn254::Fr;
use ark_ff::PrimeField;
use ark_std::rand::rngs::OsRng;
use ark_std::UniformRand;
use serde_json::json;
use std::{fs, path::PathBuf, process};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
    }
    let cmd = &args[1];

    // --pk-dir <path>  (directory containing pk_shield.bin etc.)
    let pk_dir = find_arg(&args, "--pk-dir").unwrap_or_else(|| {
        eprintln!("error: --pk-dir is required");
        process::exit(1);
    });

    match cmd.as_str() {
        "shield" => cmd_shield(&args, &pk_dir),
        "unshield" => cmd_unshield(&args, &pk_dir),
        "transfer" => cmd_transfer(&args, &pk_dir),
        _ => usage(),
    }
}

fn cmd_shield(args: &[String], pk_dir: &str) {
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

    let pk_bytes = fs::read(PathBuf::from(pk_dir).join("pk_shield.bin"))
        .unwrap_or_else(|e| {
            eprintln!("error: failed to read pk_shield.bin: {}", e);
            process::exit(1);
        });
    let vk_bytes = fs::read(PathBuf::from(pk_dir).join("vk_shield.bin"))
        .unwrap_or_else(|e| {
            eprintln!("error: failed to read vk_shield.bin: {}", e);
            process::exit(1);
        });

    let mut prover = Prover::new();
    prover.load_shield_key(&pk_bytes).unwrap_or_else(|e| {
        eprintln!("error: failed to parse pk_shield.bin: {}", e);
        process::exit(1);
    });
    let vk = load_verification_key(&vk_bytes).unwrap_or_else(|e| {
        eprintln!("error: failed to parse vk_shield.bin: {}", e);
        process::exit(1);
    });

    // Generate random blinding / serial / spending key
    let blinding = Fr::rand(&mut OsRng);
    let serial = Fr::rand(&mut OsRng);
    let spending_key = Fr::rand(&mut OsRng);

    let amount_fr = Fr::from(amount);
    let commitment_fr = poseidon_hash_fr(amount_fr, blinding);
    let commitment = fr_to_bytes(&commitment_fr);

    // Build circuit
    let circuit = ShieldCircuit::new(amount, amount, blinding, commitment_fr);

    // Prove
    let mut proof = prover.prove_shield(circuit).unwrap_or_else(|e| {
        eprintln!("error: proof generation failed: {}", e);
        process::exit(1);
    });
    proof.public_inputs = vec![fr_to_bytes(&amount_fr), commitment];

    // Verify locally
    let verifier = Verifier::from_vk_shield(vk);
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
        "blinding": hex::encode(fr_to_bytes(&blinding)),
        "serial": hex::encode(fr_to_bytes(&serial)),
        "spending_key": hex::encode(fr_to_bytes(&spending_key)),
    });
    println!("{}", serde_json::to_string(&out).unwrap());
}

fn cmd_unshield(args: &[String], pk_dir: &str) {
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
    let merkle_root_bytes: Vec<u8> = hex::decode(&merkle_root_hex).unwrap_or_else(|e| {
        eprintln!("error: invalid --merkle-root hex: {}", e);
        process::exit(1);
    });
    if merkle_root_bytes.len() != 32 {
        eprintln!("error: --merkle-root must be 32 bytes");
        process::exit(1);
    }

    let recipient_hex = find_arg(args, "--recipient").unwrap_or_else(|| {
        eprintln!("error: --recipient is required");
        process::exit(1);
    });
    let recipient_bytes: Vec<u8> = hex::decode(&recipient_hex).unwrap_or_else(|e| {
        eprintln!("error: invalid --recipient hex: {}", e);
        process::exit(1);
    });
    if recipient_bytes.len() != 32 {
        eprintln!("error: --recipient must be 32 bytes");
        process::exit(1);
    }

    // Read & parse a previously generated shield's blinding/serial.
    // Accept via --blinding and --serial flags (hex-encoded Fr).
    let blinding_hex = find_arg(args, "--blinding").unwrap_or_else(|| {
        eprintln!("error: --blinding is required (from shield output)");
        process::exit(1);
    });
    let blinding = Fr::from_le_bytes_mod_order(
        &hex::decode(&blinding_hex).unwrap_or_else(|e| {
            eprintln!("error: invalid --blinding hex: {}", e);
            process::exit(1);
        }),
    );

    let serial_hex = find_arg(args, "--serial").unwrap_or_else(|| {
        eprintln!("error: --serial is required (from shield output)");
        process::exit(1);
    });
    let serial = Fr::from_le_bytes_mod_order(
        &hex::decode(&serial_hex).unwrap_or_else(|e| {
            eprintln!("error: invalid --serial hex: {}", e);
            process::exit(1);
        }),
    );

    // Accept --spending-key (hex) or generate one.
    let spending_key = if let Some(sk_hex) = find_arg(args, "--spending-key") {
        Fr::from_le_bytes_mod_order(
            &hex::decode(&sk_hex).unwrap_or_else(|e| {
                eprintln!("error: invalid --spending-key hex: {}", e);
                process::exit(1);
            }),
        )
    } else {
        Fr::rand(&mut OsRng)
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

    let merkle_path: Vec<Fr> = merkle_path_hex
        .iter()
        .map(|h| {
            let bytes = hex::decode(h).unwrap();
            Fr::from_le_bytes_mod_order(&bytes)
        })
        .collect();

    let pk_bytes = fs::read(PathBuf::from(pk_dir).join("pk_unshield.bin")).unwrap_or_else(|e| {
        eprintln!("error: failed to read pk_unshield.bin: {}", e);
        process::exit(1);
    });
    let vk_bytes = fs::read(PathBuf::from(pk_dir).join("vk_unshield.bin")).unwrap_or_else(|e| {
        eprintln!("error: failed to read vk_unshield.bin: {}", e);
        process::exit(1);
    });
    let mut prover = Prover::new();
    prover.load_unshield_key(&pk_bytes).unwrap_or_else(|e| {
        eprintln!("error: failed to parse pk_unshield.bin: {}", e);
        process::exit(1);
    });
    let vk = load_verification_key(&vk_bytes).unwrap();

    let amount_fr = Fr::from(amount);
    let nullifier_fr = poseidon_hash_fr(serial, spending_key);
    let nullifier = fr_to_bytes(&nullifier_fr);

    let merkle_root_fr = Fr::from_le_bytes_mod_order(&merkle_root_bytes);

    let recipient_preimage = Fr::from_le_bytes_mod_order(&recipient_bytes);
    let recipient_hash_fr = poseidon_hash_fr(recipient_preimage, Fr::from(0u64));
    let recipient_hash = fr_to_bytes(&recipient_hash_fr);

    let circuit = UnshieldCircuit::new(
        merkle_root_fr,
        nullifier_fr,
        amount,
        recipient_hash_fr,
        amount,
        blinding,
        serial,
        spending_key,
        recipient_preimage,
        merkle_path,
        path_bits,
    );

    let mut proof = prover.prove_unshield(circuit).unwrap_or_else(|e| {
        eprintln!("error: proof generation failed: {}", e);
        process::exit(1);
    });
    proof.public_inputs = vec![
        fr_to_bytes(&merkle_root_fr),
        nullifier,
        fr_to_bytes(&amount_fr),
        recipient_hash,
    ];

    let verifier = Verifier::from_vk_unshield(vk);
    let ok = verifier.verify(&proof).unwrap();
    assert!(ok, "proof failed self-verification");

    let out = json!({
        "type": "unshield",
        "amount": amount,
        "nullifier": hex::encode(nullifier),
        "merkle_root": hex::encode(&merkle_root_bytes),
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
         zk-prove shield   --amount <shells> --pk-dir <path>\n  \
         zk-prove unshield --amount <shells> --pk-dir <path> --merkle-root <hex> \
                           --recipient <hex> --blinding <hex> --serial <hex>\n  \
         zk-prove transfer --pk-dir <path> --transfer-json <file>"
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

fn cmd_transfer(args: &[String], pk_dir: &str) {
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
        eprintln!("error: transfer requires exactly 2 inputs, got {}", witness.inputs.len());
        process::exit(1);
    }
    if witness.outputs.len() != 2 {
        eprintln!("error: transfer requires exactly 2 outputs, got {}", witness.outputs.len());
        process::exit(1);
    }

    // Parse merkle root
    let merkle_root_bytes = hex::decode(&witness.merkle_root).unwrap_or_else(|e| {
        eprintln!("error: invalid merkle_root hex: {}", e);
        process::exit(1);
    });
    if merkle_root_bytes.len() != 32 {
        eprintln!("error: merkle_root must be 32 bytes");
        process::exit(1);
    }
    let merkle_root_fr = Fr::from_le_bytes_mod_order(&merkle_root_bytes);

    // Parse inputs
    let mut input_values = [0u64; 2];
    let mut input_blindings_fr = [Fr::from(0u64); 2];
    let mut input_serials_fr = [Fr::from(0u64); 2];
    let mut spending_keys_fr = [Fr::from(0u64); 2];
    let mut input_merkle_paths: [Vec<Fr>; 2] = [vec![], vec![]];
    let mut input_path_bits: [Vec<bool>; 2] = [vec![], vec![]];
    let mut nullifiers_fr = [Fr::from(0u64); 2];

    for (i, inp) in witness.inputs.iter().enumerate() {
        input_values[i] = inp.amount;
        input_blindings_fr[i] = Fr::from_le_bytes_mod_order(
            &hex::decode(&inp.blinding).unwrap_or_else(|e| {
                eprintln!("error: input[{}].blinding invalid hex: {}", i, e);
                process::exit(1);
            }),
        );
        input_serials_fr[i] = Fr::from_le_bytes_mod_order(
            &hex::decode(&inp.serial).unwrap_or_else(|e| {
                eprintln!("error: input[{}].serial invalid hex: {}", i, e);
                process::exit(1);
            }),
        );
        spending_keys_fr[i] = Fr::from_le_bytes_mod_order(
            &hex::decode(&inp.spending_key).unwrap_or_else(|e| {
                eprintln!("error: input[{}].spending_key invalid hex: {}", i, e);
                process::exit(1);
            }),
        );
        if inp.merkle_path.len() != TREE_DEPTH {
            eprintln!(
                "error: input[{}].merkle_path has {} siblings, expected {}",
                i, inp.merkle_path.len(), TREE_DEPTH
            );
            process::exit(1);
        }
        input_merkle_paths[i] = inp
            .merkle_path
            .iter()
            .map(|h| {
                let bytes = hex::decode(h).unwrap_or_else(|e| {
                    eprintln!("error: input[{}].merkle_path sibling hex: {}", i, e);
                    process::exit(1);
                });
                Fr::from_le_bytes_mod_order(&bytes)
            })
            .collect();
        if inp.path_bits.len() != TREE_DEPTH {
            eprintln!(
                "error: input[{}].path_bits has {} bits, expected {}",
                i, inp.path_bits.len(), TREE_DEPTH
            );
            process::exit(1);
        }
        input_path_bits[i] = inp.path_bits.clone();

        // Compute nullifier = Poseidon(serial, spending_key)
        nullifiers_fr[i] = poseidon_hash_fr(input_serials_fr[i], spending_keys_fr[i]);
    }

    // Parse outputs (generate random blinding if not provided)
    let mut output_values = [0u64; 2];
    let mut output_blindings_fr = [Fr::from(0u64); 2];
    let mut output_serials_fr = [Fr::from(0u64); 2]; // new serial for each output note

    for (j, out) in witness.outputs.iter().enumerate() {
        output_values[j] = out.amount;
        output_blindings_fr[j] = if let Some(ref b_hex) = out.blinding {
            Fr::from_le_bytes_mod_order(
                &hex::decode(b_hex).unwrap_or_else(|e| {
                    eprintln!("error: output[{}].blinding invalid hex: {}", j, e);
                    process::exit(1);
                }),
            )
        } else {
            Fr::rand(&mut OsRng)
        };
        output_serials_fr[j] = Fr::rand(&mut OsRng);
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
    let mut output_commitments_fr = [Fr::from(0u64); 2];
    let mut output_commitments_bytes = [[0u8; 32]; 2];
    for j in 0..2 {
        let val_fr = Fr::from(output_values[j]);
        output_commitments_fr[j] = poseidon_hash_fr(val_fr, output_blindings_fr[j]);
        output_commitments_bytes[j] = fr_to_bytes(&output_commitments_fr[j]);
    }

    // Build circuit
    let circuit = TransferCircuit::new(
        merkle_root_fr,
        nullifiers_fr,
        output_commitments_fr,
        input_values,
        input_blindings_fr,
        input_serials_fr,
        spending_keys_fr,
        input_merkle_paths,
        input_path_bits,
        output_values,
        output_blindings_fr,
    );

    // Load proving + verification keys
    let pk_bytes = fs::read(PathBuf::from(pk_dir).join("pk_transfer.bin")).unwrap_or_else(|e| {
        eprintln!("error: failed to read pk_transfer.bin: {}", e);
        process::exit(1);
    });
    let vk_bytes = fs::read(PathBuf::from(pk_dir).join("vk_transfer.bin")).unwrap_or_else(|e| {
        eprintln!("error: failed to read vk_transfer.bin: {}", e);
        process::exit(1);
    });

    let mut prover = Prover::new();
    prover.load_transfer_key(&pk_bytes).unwrap_or_else(|e| {
        eprintln!("error: failed to parse pk_transfer.bin: {}", e);
        process::exit(1);
    });
    let vk = load_verification_key(&vk_bytes).unwrap_or_else(|e| {
        eprintln!("error: failed to parse vk_transfer.bin: {}", e);
        process::exit(1);
    });

    // Generate proof
    let mut proof = prover.prove_transfer(circuit).unwrap_or_else(|e| {
        eprintln!("error: proof generation failed: {}", e);
        process::exit(1);
    });

    // Set public inputs: [merkle_root, null_a, null_b, comm_c, comm_d]
    let nullifier_bytes = [
        fr_to_bytes(&nullifiers_fr[0]),
        fr_to_bytes(&nullifiers_fr[1]),
    ];
    proof.public_inputs = vec![
        fr_to_bytes(&merkle_root_fr),
        nullifier_bytes[0],
        nullifier_bytes[1],
        output_commitments_bytes[0],
        output_commitments_bytes[1],
    ];

    // Verify locally
    let verifier = Verifier::from_vk_transfer(vk);
    let ok = verifier.verify(&proof).unwrap_or_else(|e| {
        eprintln!("error: proof self-check failed: {}", e);
        process::exit(1);
    });
    assert!(ok, "transfer proof failed self-verification");

    // Output JSON
    let out = json!({
        "type": "transfer",
        "merkle_root": hex::encode(&merkle_root_bytes),
        "nullifier_a": hex::encode(nullifier_bytes[0]),
        "nullifier_b": hex::encode(nullifier_bytes[1]),
        "commitment_c": hex::encode(output_commitments_bytes[0]),
        "commitment_d": hex::encode(output_commitments_bytes[1]),
        "proof": hex::encode(&proof.proof_bytes),
        // Output note secrets (needed by recipient to spend later)
        "outputs": [
            {
                "amount": output_values[0],
                "blinding": hex::encode(fr_to_bytes(&output_blindings_fr[0])),
                "serial": hex::encode(fr_to_bytes(&output_serials_fr[0])),
                "commitment": hex::encode(output_commitments_bytes[0]),
            },
            {
                "amount": output_values[1],
                "blinding": hex::encode(fr_to_bytes(&output_blindings_fr[1])),
                "serial": hex::encode(fr_to_bytes(&output_serials_fr[1])),
                "commitment": hex::encode(output_commitments_bytes[1]),
            },
        ],
    });
    println!("{}", serde_json::to_string(&out).unwrap());
}
