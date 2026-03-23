#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RPC_URL="http://localhost:8899"

KEYPAIR_PATH="${GENESIS_KEYPAIR_PATH:-}"
if [[ -z "${KEYPAIR_PATH}" ]]; then
  KEYPAIR_PATH=$(find "$REPO_ROOT" "$HOME" -path "*/genesis-keys/genesis-primary-cli.json" -type f 2>/dev/null | sort | tail -n 1)
fi
if [[ -z "${KEYPAIR_PATH}" ]]; then
  KEYPAIR_PATH=$(find "$REPO_ROOT" "$HOME" -path "*/genesis-keys/genesis-primary-*.json" -type f 2>/dev/null | sort | tail -n 1)
fi
if [[ -z "${KEYPAIR_PATH}" ]]; then
  echo "ERROR: Genesis keypair not found." >&2
  echo "Set GENESIS_KEYPAIR_PATH to override auto-discovery." >&2
  exit 1
fi

CLI_BIN="$REPO_ROOT/target/release/licn"
if [[ ! -x "$CLI_BIN" ]]; then
  echo "ERROR: CLI binary not found at $CLI_BIN" >&2
  exit 1
fi

echo "Using genesis keypair: $KEYPAIR_PATH"

airdrop_once() {
  local validators
  validators=$(curl -s -X POST "$RPC_URL" \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getValidators","params":[]}' \
    | jq -r '.result.validators[].pubkey')

  if [[ -z "${validators}" ]]; then
    echo "WARN: No validators returned from RPC." >&2
    return 0
  fi

  while read -r pubkey; do
    if [[ -z "$pubkey" ]]; then
      continue
    fi
    echo "Sending 1 LICN -> $pubkey"
    "$CLI_BIN" --rpc-url "$RPC_URL" transfer "$pubkey" 1 --keypair "$KEYPAIR_PATH"
  done <<< "$validators"
}

while true; do
  airdrop_once
  sleep 60
done
