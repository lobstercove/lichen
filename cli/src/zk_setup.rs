//! Standalone ZK trusted setup binary.
//!
//! Generates Groth16 proving/verification keys for all 3 shielded circuits
//! (shield, unshield, transfer) and writes them to `~/.moltchain/zk/`.
//!
//! This binary is intentionally minimal — no tokio, no RocksDB, no network —
//! so it gets maximum available memory for the Groth16 MSM/FFT computations.
//!
//! Usage:
//!   zk-setup                    # Generate keys to ~/.moltchain/zk/
//!   zk-setup --output /path     # Generate keys to custom directory
//!   zk-setup --force            # Regenerate even if keys exist

use moltchain_core::zk::setup;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut output_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".moltchain")
        .join("zk");
    let mut force = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--output" | "-o" => {
                i += 1;
                if i < args.len() {
                    output_dir = PathBuf::from(&args[i]);
                } else {
                    eprintln!("Error: --output requires a path argument");
                    std::process::exit(1);
                }
            }
            "--force" | "-f" => force = true,
            "--help" | "-h" => {
                println!("zk-setup — Generate Groth16 ZK proving/verification keys");
                println!();
                println!("Usage: zk-setup [OPTIONS]");
                println!();
                println!("Options:");
                println!("  -o, --output <DIR>   Output directory (default: ~/.moltchain/zk/)");
                println!("  -f, --force          Regenerate keys even if they already exist");
                println!("  -h, --help           Show this help");
                std::process::exit(0);
            }
            other => {
                eprintln!("Unknown argument: {}", other);
                std::process::exit(1);
            }
        }
        i += 1;
    }

    // Check if keys already exist
    let vk_shield = output_dir.join("vk_shield.bin");
    let vk_unshield = output_dir.join("vk_unshield.bin");
    let vk_transfer = output_dir.join("vk_transfer.bin");

    if !force && vk_shield.exists() && vk_unshield.exists() && vk_transfer.exists() {
        println!("✓ ZK keys already exist at {}", output_dir.display());
        println!("  Use --force to regenerate.");
        std::process::exit(0);
    }

    fs::create_dir_all(&output_dir).unwrap_or_else(|e| {
        eprintln!("Error creating {}: {}", output_dir.display(), e);
        std::process::exit(1);
    });

    println!("🔑 ZK Trusted Setup — Groth16/BN254");
    println!("   Output: {}", output_dir.display());
    println!("   Tree depth: {}", moltchain_core::zk::TREE_DEPTH);
    println!();

    // Generate each circuit sequentially to keep memory bounded.
    // Each circuit's PK is dropped before starting the next one.
    let circuits: Vec<(&str, fn() -> Result<setup::CeremonyOutput, String>)> = vec![
        ("shield", setup::setup_shield as fn() -> _),
        ("unshield", setup::setup_unshield as fn() -> _),
        ("transfer", setup::setup_transfer as fn() -> _),
    ];

    let total_start = Instant::now();

    for (name, setup_fn) in &circuits {
        print!("  ⚙️  {} circuit... ", name);
        let start = Instant::now();

        let output = match setup_fn() {
            Ok(o) => o,
            Err(e) => {
                eprintln!("\n❌ {} setup failed: {}", name, e);
                std::process::exit(1);
            }
        };

        let vk_path = output_dir.join(format!("vk_{}.bin", output.circuit_name));
        let pk_path = output_dir.join(format!("pk_{}.bin", output.circuit_name));

        let vk_len = output.verification_key_bytes.len();
        let pk_len = output.proving_key_bytes.len();

        fs::write(&vk_path, &output.verification_key_bytes).unwrap_or_else(|e| {
            eprintln!("\n❌ Failed writing {}: {}", vk_path.display(), e);
            std::process::exit(1);
        });
        fs::write(&pk_path, &output.proving_key_bytes).unwrap_or_else(|e| {
            eprintln!("\n❌ Failed writing {}: {}", pk_path.display(), e);
            std::process::exit(1);
        });

        println!(
            "✓ ({:.1}s) — VK {} bytes, PK {} bytes",
            start.elapsed().as_secs_f64(),
            vk_len,
            pk_len
        );
        // `output` dropped here — frees PK/VK memory before next circuit
    }

    println!();
    println!(
        "✅ ZK trusted setup complete in {:.1}s",
        total_start.elapsed().as_secs_f64()
    );
    println!("   Keys cached at: {}", output_dir.display());
    println!();
    println!("   Validators will load these keys automatically on startup.");
    println!("   Keys persist across blockchain resets.");
}
