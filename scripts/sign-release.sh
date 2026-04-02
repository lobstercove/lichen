#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Lichen Release Signer
# ─────────────────────────────────────────────────────────────────────────────
# Signs a SHA256SUMS file with the native PQ release signing key.
# The resulting .sig file is a JSON-encoded self-contained `PqSignature`.
#
# Usage:
#   ./scripts/sign-release.sh <SHA256SUMS> <keypair.json>
#
# Output:
#   SHA256SUMS.sig  — JSON-encoded `PqSignature`
#
# The keypair.json must be the canonical PQ key file produced by
# generate-release-keys.sh.
# ─────────────────────────────────────────────────────────────────────────────

set -euo pipefail

if [ $# -lt 2 ]; then
    echo "Usage: $0 <SHA256SUMS> <keypair.json>"
    echo ""
    echo "Signs SHA256SUMS with the release signing key."
    echo "Output: SHA256SUMS.sig (in the same directory as SHA256SUMS)"
    exit 1
fi

SUMS_FILE="$1"
KEYPAIR_FILE="$2"

SUMS_FILE="$(cd "$(dirname "$SUMS_FILE")" && pwd)/$(basename "$SUMS_FILE")"
KEYPAIR_FILE="$(cd "$(dirname "$KEYPAIR_FILE")" && pwd)/$(basename "$KEYPAIR_FILE")"

if [ ! -f "$SUMS_FILE" ]; then
    echo "❌ SHA256SUMS file not found: $SUMS_FILE"
    exit 1
fi

if [ ! -f "$KEYPAIR_FILE" ]; then
    echo "❌ Keypair file not found: $KEYPAIR_FILE"
    exit 1
fi

SUMS_DIR="$(dirname "$SUMS_FILE")"
SIG_FILE="$SUMS_DIR/SHA256SUMS.sig"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TEMP_DIR=$(mktemp -d)
trap 'rm -rf "$TEMP_DIR"' EXIT

cat > "$TEMP_DIR/Cargo.toml" <<TOML
[package]
name = "release-signer"
version = "0.1.0"
edition = "2021"

[dependencies]
hex = "0.4"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
lichen-core = { path = "$REPO_ROOT/core" }
TOML

mkdir -p "$TEMP_DIR/src"
cat > "$TEMP_DIR/src/main.rs" <<'RUST'
use lichen_core::Keypair;
use serde::Deserialize;
use std::{env, fs};

#[derive(Deserialize)]
struct KeypairFile {
    #[serde(rename = "privateKey")]
    private_key: Vec<u8>,
}

fn decode_seed(file: KeypairFile) -> [u8; 32] {
    file.private_key
        .as_slice()
        .try_into()
        .expect("privateKey must be 32 bytes")
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: signer <sums-file> <keypair-json>");
        std::process::exit(1);
    }

    let sums_content = fs::read(&args[1]).expect("Failed to read SHA256SUMS");
    let keypair_json = fs::read_to_string(&args[2]).expect("Failed to read keypair file");
    let keypair_file: KeypairFile = serde_json::from_str(&keypair_json).expect("Invalid keypair JSON");
    let seed = decode_seed(keypair_file);
    let keypair = Keypair::from_seed(&seed);
    let signature = keypair.sign(&sums_content);

    eprintln!("🔐 Signer address: {}", keypair.pubkey().to_base58());
    println!("{}", serde_json::to_string_pretty(&signature).expect("serialize signature"));
}
RUST

echo "🔨 Building signer..."
echo "🔏 Signing $SUMS_FILE..."
SIG_JSON=$(cargo run --quiet --manifest-path "$TEMP_DIR/Cargo.toml" -- "$SUMS_FILE" "$KEYPAIR_FILE")

printf '%s\n' "$SIG_JSON" > "$SIG_FILE"

echo ""
echo "✅ Signature written to: $SIG_FILE"
echo "   Format: JSON-encoded self-contained PQ signature"
echo ""
echo "Upload SHA256SUMS.sig to the GitHub Release alongside SHA256SUMS."
