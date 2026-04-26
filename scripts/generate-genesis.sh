#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
GENESIS_BIN="${LICHEN_GENESIS_BIN:-$ROOT_DIR/target/release/lichen-genesis}"

usage() {
    cat <<'EOF'
Lichen genesis wrapper for the canonical PQ launch path.

Wallet artifact preparation:
  ./scripts/generate-genesis.sh --network testnet --prepare-wallet --output-dir ./artifacts/testnet

Genesis DB creation with an explicit validator address:
  ./scripts/generate-genesis.sh \
    --network testnet \
    --db-path ./data/state-7001 \
    --wallet-file ./artifacts/testnet/genesis-wallet.json \
    --validator-keypair ./data/state-7001/validator-keypair.json

Options:
  --network <testnet|mainnet>      Required network name
  --prepare-wallet                 Generate wallet artifacts only
  --output-dir <path>              Required with --prepare-wallet
  --db-path <path>                 Required for genesis DB creation
  --wallet-file <path>             Required for genesis DB creation
  --initial-validator <base58>     Repeatable explicit validator address
  --validator-keypair <path>       Derive the validator address from a canonical keypair file
  --config <path>                  Optional genesis config override passed through to lichen-genesis
  --genesis-prices-file <path>     Optional audited price snapshot for genesis market seeds
  --help                           Show this message

Removed legacy options:
  --output, --validators, --treasury, and --chain-id are intentionally rejected.
  The canonical PQ launch path stores the genesis block in a DB path, not in a handcrafted genesis.json file.
EOF
}

print_error() {
    echo "[generate-genesis] ERROR: $*" >&2
}

require_value() {
    local flag="$1"
    local value="${2:-}"
    if [[ -z "$value" ]]; then
        print_error "Missing value for $flag"
        exit 2
    fi
}

derive_validator_address() {
    local keypair_path="$1"
    python3 - "$keypair_path" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, 'r', encoding='utf-8') as handle:
    data = json.load(handle)

for key in ('publicKeyBase58', 'address', 'pubkey'):
    value = data.get(key)
    if isinstance(value, str) and value.strip():
        print(value.strip())
        raise SystemExit(0)

raise SystemExit('validator keypair file is missing publicKeyBase58/address/pubkey')
PY
}

NETWORK=""
PREPARE_WALLET=0
OUTPUT_DIR=""
DB_PATH=""
WALLET_FILE=""
VALIDATOR_KEYPAIR=""
CONFIG_PATH=""
GENESIS_PRICES_FILE=""
INITIAL_VALIDATORS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --network)
            require_value "$1" "${2:-}"
            NETWORK="$2"
            shift 2
            ;;
        --prepare-wallet)
            PREPARE_WALLET=1
            shift
            ;;
        --output-dir)
            require_value "$1" "${2:-}"
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --db-path)
            require_value "$1" "${2:-}"
            DB_PATH="$2"
            shift 2
            ;;
        --wallet-file)
            require_value "$1" "${2:-}"
            WALLET_FILE="$2"
            shift 2
            ;;
        --initial-validator)
            require_value "$1" "${2:-}"
            INITIAL_VALIDATORS+=("$2")
            shift 2
            ;;
        --validator-keypair)
            require_value "$1" "${2:-}"
            VALIDATOR_KEYPAIR="$2"
            shift 2
            ;;
        --config)
            require_value "$1" "${2:-}"
            CONFIG_PATH="$2"
            shift 2
            ;;
        --genesis-prices-file)
            require_value "$1" "${2:-}"
            GENESIS_PRICES_FILE="$2"
            shift 2
            ;;
        --output|--validators|--treasury|--chain-id)
            print_error "$1 is a removed legacy option. Use --prepare-wallet/--output-dir or --db-path/--wallet-file instead."
            exit 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            print_error "Unknown option: $1"
            usage
            exit 2
            ;;
    esac
done

if [[ "$NETWORK" != "testnet" && "$NETWORK" != "mainnet" ]]; then
    print_error "--network must be testnet or mainnet"
    exit 2
fi

if [[ ! -x "$GENESIS_BIN" ]]; then
    echo "[generate-genesis] Building lichen-genesis..."
    cargo build --release --bin lichen-genesis >/dev/null
fi

COMMAND=("$GENESIS_BIN" --network "$NETWORK")

if [[ -n "$CONFIG_PATH" ]]; then
    COMMAND+=(--config "$CONFIG_PATH")
fi
if [[ -n "$GENESIS_PRICES_FILE" ]]; then
    COMMAND+=(--genesis-prices-file "$GENESIS_PRICES_FILE")
fi

if (( PREPARE_WALLET )); then
    if [[ -z "$OUTPUT_DIR" ]]; then
        print_error "--prepare-wallet requires --output-dir <path>"
        exit 2
    fi
    COMMAND+=(--prepare-wallet --output-dir "$OUTPUT_DIR")
else
    if [[ -n "$VALIDATOR_KEYPAIR" ]]; then
        INITIAL_VALIDATORS+=("$(derive_validator_address "$VALIDATOR_KEYPAIR")")
    fi

    if [[ -z "$DB_PATH" || -z "$WALLET_FILE" ]]; then
        print_error "Genesis DB creation requires --db-path <path> and --wallet-file <path>"
        exit 2
    fi
    if [[ ${#INITIAL_VALIDATORS[@]} -eq 0 && -z "$CONFIG_PATH" ]]; then
        print_error "Pass at least one --initial-validator <base58> or --validator-keypair <path>"
        exit 2
    fi

    COMMAND+=(--db-path "$DB_PATH" --wallet-file "$WALLET_FILE")
    for validator in "${INITIAL_VALIDATORS[@]}"; do
        COMMAND+=(--initial-validator "$validator")
    done
fi

echo "[generate-genesis] Executing: ${COMMAND[*]}"
exec "${COMMAND[@]}"
