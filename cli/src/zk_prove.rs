//! ZK Proof Generator CLI
//!
//! Generates Groth16/BN254 proofs for shield, unshield, and transfer
//! transactions.  Used by the E2E test suite (Python) to create valid
//! proofs that the validator can verify.
//!
//! Usage:
//!   zk-prove shield  --amount <shells> --pk-dir <path>
//!   zk-prove unshield --amount <shells> --pk-dir <path> --merkle-root <hex> --recipient <hex>
//!
//! Outputs a JSON object to stdout with all values needed to build the
//! on-chain transaction.

use moltchain_core::zk::{
    circuits::shield::ShieldCircuit,
    circuits::unshield::UnshieldCircuit,
    fr_to_bytes, poseidon_hash_fr,
    setup::load_verification_key,
    Prover, Verifier,
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

    // Generate random blinding / serial
    let blinding = Fr::rand(&mut OsRng);
    let serial = Fr::rand(&mut OsRng);

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

    // Accept --merkle-path-json (file with JSON array of 32 hex siblings)
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
        // Default: 32 zero siblings (the leaf is at index 0, all siblings are empty)
        vec!["00".repeat(32); 32]
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
        vec![false; 32]
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
        "Usage:\n  zk-prove shield  --amount <shells> --pk-dir <path>\n  zk-prove unshield --amount <shells> --pk-dir <path> --merkle-root <hex> --recipient <hex> --blinding <hex> --serial <hex>"
    );
    process::exit(1);
}
