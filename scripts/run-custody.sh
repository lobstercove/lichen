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

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/.."
cd "$REPO_ROOT" || exit 1

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

if [ -z "${CUSTODY_DB_PATH:-}" ]; then
  export CUSTODY_DB_PATH="./data/custody-${NETWORK}"
fi

echo "🦞 Starting Lichen Custody"
echo "=============================="
echo "Network: $NETWORK"
echo "DB: $CUSTODY_DB_PATH"
echo "Signers: $CUSTODY_SIGNER_ENDPOINTS"
echo "Threshold: $CUSTODY_SIGNER_THRESHOLD"
echo ""

cargo run --release --bin lichen-custody
