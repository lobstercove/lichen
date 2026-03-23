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

echo "🛑 Stopping Lichen local stack ($NETWORK)"

pkill -f "lichen-validator" || true
pkill -f "lichen-custody" || true

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
