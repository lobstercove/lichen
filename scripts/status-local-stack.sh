#!/bin/bash

set -euo pipefail

NETWORK=${1:-testnet}
NETWORK=$(echo "$NETWORK" | tr '[:upper:]' '[:lower:]')

case $NETWORK in
  testnet)
    BASE_RPC=8899
    CUSTODY_PORT=9105
    FAUCET_PORT=9100
    ;;
  mainnet)
    BASE_RPC=9899
    CUSTODY_PORT=9106
    FAUCET_PORT=""
    ;;
  *)
    echo "Usage: $0 [testnet|mainnet]"
    exit 1
    ;;
esac

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON_BIN="${PYTHON_BIN:-$ROOT_DIR/.venv/bin/python}"
if [[ ! -x "$PYTHON_BIN" ]]; then
  PYTHON_BIN="python3"
fi

RPC_PORTS=($BASE_RPC $((BASE_RPC + 2)) $((BASE_RPC + 4)))

json_rpc() {
  local url=$1
  curl -s -X POST "$url" \
    -H 'content-type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}'
}

is_healthy_response() {
  echo "$1" | $PYTHON_BIN -c "import json,sys; r=json.load(sys.stdin); result=r.get('result'); assert result is True or result in ['ok','healthy'] or (isinstance(result, dict) and result.get('status') in ['ok','healthy',True])" >/dev/null 2>&1
}

echo "🦞 Lichen local stack status ($NETWORK)"

echo "Validators:"
for port in "${RPC_PORTS[@]}"; do
  url="http://127.0.0.1:${port}"
  response=$(json_rpc "$url" || true)
  if is_healthy_response "$response"; then
    echo "  ✓ $url"
  else
    echo "  ✗ $url"
  fi
done

echo "Custody:"
if curl -s "http://127.0.0.1:${CUSTODY_PORT}/health" | grep -q '"status":"ok"'; then
  echo "  ✓ http://127.0.0.1:${CUSTODY_PORT}"
else
  echo "  ✗ http://127.0.0.1:${CUSTODY_PORT}"
fi

if [ -n "$FAUCET_PORT" ]; then
  echo "Faucet:"
  if curl -s "http://127.0.0.1:${FAUCET_PORT}/health" | grep -Eq 'OK|"status":"ok"'; then
    echo "  ✓ http://127.0.0.1:${FAUCET_PORT}"
  else
    echo "  ✗ http://127.0.0.1:${FAUCET_PORT}"
  fi
fi

if curl -s "http://127.0.0.1:${CUSTODY_PORT}/status" >/dev/null; then
  echo ""
  echo "Custody status:"
  curl -s "http://127.0.0.1:${CUSTODY_PORT}/status" | $PYTHON_BIN -m json.tool
fi
