#!/bin/bash

set -e

NETWORK=${1:-testnet}
NETWORK=$(echo "$NETWORK" | tr '[:upper:]' '[:lower:]')

case $NETWORK in
  testnet)
    BASE_RPC=8899
    ;;
  mainnet)
    BASE_RPC=9899
    ;;
  *)
    echo "Usage: $0 [testnet|mainnet]"
    exit 1
    ;;
esac

RPC_PORTS=($BASE_RPC $((BASE_RPC - 1)) $((BASE_RPC - 2)))

json_rpc() {
  local url=$1
  curl -s -X POST "$url" \
    -H 'content-type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'
}

echo "🦞 Lichen local stack status ($NETWORK)"

echo "Validators:"
for port in "${RPC_PORTS[@]}"; do
  url="http://127.0.0.1:${port}"
  response=$(json_rpc "$url" || true)
  if echo "$response" | grep -q '"status":"ok"'; then
    echo "  ✓ $url"
  else
    echo "  ✗ $url"
  fi
done

echo "Custody:"
if curl -s "http://127.0.0.1:9105/health" | grep -q '"status":"ok"'; then
  echo "  ✓ http://127.0.0.1:9105"
else
  echo "  ✗ http://127.0.0.1:9105"
fi

if curl -s "http://127.0.0.1:9105/status" >/dev/null; then
  echo ""
  echo "Custody status:"
  curl -s "http://127.0.0.1:9105/status" | python3 -m json.tool
fi
