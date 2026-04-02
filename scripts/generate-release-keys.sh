#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Lichen Release Signing Key Generator
# ─────────────────────────────────────────────────────────────────────────────
# Generates a native PQ release signing keypair using the workspace crypto.
# The trusted compact address must be embedded in validator/src/updater.rs.
#
# Usage:
#   ./scripts/generate-release-keys.sh [output-dir]
#
# Output:
#   <output-dir>/release-signing-keypair.json  — SECRET key (keep offline!)
#   Prints the trusted signer address to embed in validator/src/updater.rs
# ─────────────────────────────────────────────────────────────────────────────

set -euo pipefail

OUTPUT_DIR="${1:-.}"
KEYPAIR_FILE="$OUTPUT_DIR/release-signing-keypair.json"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [ -f "$KEYPAIR_FILE" ]; then
    echo "⚠️  Keypair file already exists: $KEYPAIR_FILE"
    echo "    Delete it first if you want to regenerate."
    exit 1
fi

mkdir -p "$OUTPUT_DIR"

echo "🔑 Generating native PQ release signing keypair..."
echo ""

TEMP_DIR=$(mktemp -d)
trap 'rm -rf "$TEMP_DIR"' EXIT

cat > "$TEMP_DIR/Cargo.toml" <<TOML
[package]
name = "release-keygen"
version = "0.1.0"
edition = "2021"

[dependencies]
serde_json = "1.0"
lichen-core = { path = "$REPO_ROOT/core" }
TOML

mkdir -p "$TEMP_DIR/src"
cat > "$TEMP_DIR/src/main.rs" <<'RUST'
use lichen_core::Keypair;
use serde_json::json;
use std::{env, fs};

fn main() {
    let output_path = env::args().nth(1).unwrap_or_else(|| "release-signing-keypair.json".into());
    let keypair = Keypair::new();
    let public_key = keypair.public_key();

    let file = json!({
        "privateKey": keypair.to_seed(),
        "publicKey": public_key.bytes,
        "publicKeyBase58": keypair.pubkey().to_base58(),
    });

    fs::write(&output_path, serde_json::to_string_pretty(&file).expect("serialize key file"))
        .expect("write key file");

    eprintln!("✅ Keypair generated successfully!");
    eprintln!("");
    eprintln!("📁 Keypair file: {}", output_path);
    eprintln!("   ⚠️  KEEP THIS FILE SECRET AND OFFLINE!");
    eprintln!("");
    eprintln!("🔐 Trusted release signer address (embed in validator/src/updater.rs):");
    eprintln!("");
    println!("{}", keypair.pubkey().to_base58());
}
RUST

echo "🔨 Building key generator..."
cargo run --quiet --manifest-path "$TEMP_DIR/Cargo.toml" -- "$KEYPAIR_FILE"
echo ""
echo "Done! Store the keypair file in a secure offline location."
