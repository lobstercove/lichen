#!/bin/bash

set -euo pipefail

# Restore a sane tool PATH when the caller shell exported a stripped environment.
BOOTSTRAP_PATH="/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
if [ -n "${HOME:-}" ] && [ -d "${HOME}/.cargo/bin" ]; then
  BOOTSTRAP_PATH="${HOME}/.cargo/bin:${BOOTSTRAP_PATH}"
fi
if [ -n "${HOME:-}" ] && [ -d "${HOME}/.local/bin" ]; then
  BOOTSTRAP_PATH="${HOME}/.local/bin:${BOOTSTRAP_PATH}"
fi
PATH="${BOOTSTRAP_PATH}:${PATH:-}"
export PATH

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

echo "🛑 Stopping Lichen local stack ($NETWORK)"

pkill -f "validator-supervisor.sh" || true
pkill -f "run-validator.sh ${NETWORK}" || true
pkill -f "lichen-validator" || true
pkill -f "lichen-custody" || true
pkill -f "lichen-faucet" || true
pkill -f "local-solana-rpc-mock.py" || true
pkill -f "local-evm-rpc-mock.py" || true
pkill -f "first-boot-deploy.sh" || true

LOG_DIR="/tmp/lichen-local-${NETWORK}"
if [ -d "$LOG_DIR" ]; then
  echo "Logs: $LOG_DIR"
fi

if pgrep -f "lichen-validator" >/dev/null; then
  echo "⚠️  Some validators still running"
else
  echo "✅ Validators stopped"
fi

if pgrep -f "lichen-custody" >/dev/null; then
  echo "⚠️  Custody still running"
else
  echo "✅ Custody stopped"
fi

if pgrep -f "lichen-faucet" >/dev/null; then
  echo "⚠️  Faucet still running"
else
  echo "✅ Faucet stopped"
fi

if pgrep -f "local-evm-rpc-mock.py" >/dev/null; then
  echo "⚠️  Local EVM RPC still running"
else
  echo "✅ Local EVM RPC stopped"
fi

if pgrep -f "local-solana-rpc-mock.py" >/dev/null; then
  echo "⚠️  Local Solana RPC still running"
else
  echo "✅ Local Solana RPC stopped"
fi
