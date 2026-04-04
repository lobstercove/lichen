#!/bin/bash

set -e

NETWORK=${1:-testnet}
NETWORK=$(echo "$NETWORK" | tr '[:upper:]' '[:lower:]')

case $NETWORK in
  testnet|mainnet)
    ;;
  *)
    echo "Usage: $0 [testnet|mainnet]"
    exit 1
    ;;
esac

require_local_dev() {
  if [ "${LICHEN_LOCAL_DEV:-0}" != "1" ]; then
    echo "run-custody.sh is restricted to explicit local development." >&2
    echo "Export LICHEN_LOCAL_DEV=1 to continue, or run lichen-custody directly with explicit production configuration." >&2
    exit 1
  fi
}

generate_local_token() {
  if command -v python3 >/dev/null 2>&1; then
    python3 - <<'PY'
import secrets
print(secrets.token_hex(24))
PY
    return 0
  fi

  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex 24
    return 0
  fi

  echo "python3 or openssl is required to generate a local custody auth token" >&2
  return 1
}

require_local_dev

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/.."
cd "$REPO_ROOT" || exit 1
LOCAL_CUSTODY_TOKEN_FILE="$REPO_ROOT/tests/artifacts/local_cluster/custody-api-auth-token"

# In dev/insecure mode skip remote signers entirely (single-key local custody).
if [ "${CUSTODY_ALLOW_INSECURE_SEED:-0}" = "1" ]; then
  CUSTODY_SIGNER_ENDPOINTS=""
  export CUSTODY_SIGNER_ENDPOINTS
  export CUSTODY_SIGNER_THRESHOLD=1
else
  if [ -z "${CUSTODY_SIGNER_ENDPOINTS:-}" ]; then
    CUSTODY_SIGNER_ENDPOINTS="http://127.0.0.1:9201,http://127.0.0.1:9202,http://127.0.0.1:9203"
    export CUSTODY_SIGNER_ENDPOINTS
  fi
  if [ -z "${CUSTODY_SIGNER_THRESHOLD:-}" ]; then
    if [ "$NETWORK" = "mainnet" ]; then
      CUSTODY_SIGNER_THRESHOLD=3
    else
      CUSTODY_SIGNER_THRESHOLD=2
    fi
    export CUSTODY_SIGNER_THRESHOLD
  fi

  if [ -z "${CUSTODY_SIGNER_AUTH_TOKENS:-}" ] && [ -z "${CUSTODY_SIGNER_AUTH_TOKEN:-}" ]; then
    if [ -n "${LICHEN_SIGNER_AUTH_TOKEN:-}" ]; then
      export CUSTODY_SIGNER_AUTH_TOKEN="$LICHEN_SIGNER_AUTH_TOKEN"
    else
      echo "CUSTODY_SIGNER_AUTH_TOKEN or CUSTODY_SIGNER_AUTH_TOKENS must be set for multi-signer local custody." >&2
      echo "Export LICHEN_SIGNER_AUTH_TOKEN before starting validators and custody so both sides share the same signer auth secret." >&2
      exit 1
    fi
  fi
fi

if [ -z "${CUSTODY_DB_PATH:-}" ]; then
  export CUSTODY_DB_PATH="./data/custody-${NETWORK}"
fi

if [ -z "${CUSTODY_LISTEN_PORT:-}" ]; then
  if [ "$NETWORK" = "mainnet" ]; then
    export CUSTODY_LISTEN_PORT=9106
  else
    export CUSTODY_LISTEN_PORT=9105
  fi
fi

if [ -z "${CUSTODY_LICHEN_RPC_URL:-}" ]; then
  if [ "$NETWORK" = "mainnet" ]; then
    export CUSTODY_LICHEN_RPC_URL="http://127.0.0.1:9899"
  else
    export CUSTODY_LICHEN_RPC_URL="http://127.0.0.1:8899"
  fi
fi

if [ -z "${CUSTODY_TREASURY_KEYPAIR:-}" ]; then
  export CUSTODY_TREASURY_KEYPAIR="./data/state-${NETWORK}/genesis-keys/treasury-lichen-${NETWORK}-1.json"
fi

if [ -z "${CUSTODY_API_AUTH_TOKEN:-}" ]; then
  if [ -f "$LOCAL_CUSTODY_TOKEN_FILE" ]; then
    export CUSTODY_API_AUTH_TOKEN="$(cat "$LOCAL_CUSTODY_TOKEN_FILE")"
    echo "Loaded local CUSTODY_API_AUTH_TOKEN from $LOCAL_CUSTODY_TOKEN_FILE"
  else
    export CUSTODY_API_AUTH_TOKEN="$(generate_local_token)"
    echo "Generated an ephemeral local CUSTODY_API_AUTH_TOKEN for this run"
  fi
fi

echo "🦞 Starting Lichen Custody"
echo "=============================="
echo "Network: $NETWORK"
echo "DB: $CUSTODY_DB_PATH"
echo "Signers: $CUSTODY_SIGNER_ENDPOINTS"
echo "Threshold: $CUSTODY_SIGNER_THRESHOLD"
echo ""

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CUSTODY_BIN="${SCRIPT_DIR}/../target/release/lichen-custody"

if [ -x "$CUSTODY_BIN" ]; then
    exec "$CUSTODY_BIN"
else
    # Fallback: try cargo (requires ~/.cargo/env sourced)
    source "${LICHEN_REAL_HOME:-$HOME}/.cargo/env" 2>/dev/null || true
    cargo run --release --bin lichen-custody
fi
