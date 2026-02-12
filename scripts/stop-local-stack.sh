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

echo "🛑 Stopping MoltChain local stack ($NETWORK)"

pkill -f "moltchain-validator" || true
pkill -f "moltchain-custody" || true

LOG_DIR="/tmp/moltchain-local-${NETWORK}"
if [ -d "$LOG_DIR" ]; then
  echo "Logs: $LOG_DIR"
fi

if pgrep -f "moltchain-validator" >/dev/null; then
  echo "⚠️  Some validators still running"
else
  echo "✅ Validators stopped"
fi

if pgrep -f "moltchain-custody" >/dev/null; then
  echo "⚠️  Custody still running"
else
  echo "✅ Custody stopped"
fi
