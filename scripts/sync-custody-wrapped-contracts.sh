#!/usr/bin/env bash
# Refresh custody's Lichen-side wrapped token contract pins from the live symbol
# registry. This is required after a fresh genesis because wrapped-token program
# IDs change even when source-chain route values stay the same.

set -euo pipefail

ENV_FILE="/etc/lichen/custody-env"
RPC_URL="${CUSTODY_LICHEN_RPC_URL:-}"

usage() {
  cat <<'EOF'
Usage: sudo bash scripts/sync-custody-wrapped-contracts.sh [--env-file PATH] [--rpc-url URL]

The script reads LUSD/WSOL/WETH/WBNB/WGAS/WNEO/WBTC from getSymbolRegistry and
updates only the matching CUSTODY_*_TOKEN_ADDR keys in the custody env file.
It does not remove source-chain route keys, custody DB state, chain state, DEX
pairs, DEX pools, DEX routes, or historical blocks.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --env-file)
      ENV_FILE="${2:?missing --env-file value}"
      shift 2
      ;;
    --rpc-url)
      RPC_URL="${2:?missing --rpc-url value}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ ! -f "$ENV_FILE" ]; then
  echo "FATAL: custody env file not found: $ENV_FILE" >&2
  exit 1
fi

if [ -z "${RPC_URL//[[:space:]]/}" ]; then
  RPC_URL="$(awk -F= '
    /^[[:space:]]*#/ { next }
    $1 == "CUSTODY_LICHEN_RPC_URL" {
      sub(/^[^=]*=/, "")
      print
    }
  ' "$ENV_FILE" | tail -n1)"
fi

if [ -z "${RPC_URL//[[:space:]]/}" ]; then
  echo "FATAL: missing RPC URL; pass --rpc-url or set CUSTODY_LICHEN_RPC_URL" >&2
  exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "FATAL: jq is required" >&2
  exit 1
fi
if ! command -v curl >/dev/null 2>&1; then
  echo "FATAL: curl is required" >&2
  exit 1
fi

fetch_program() {
  local symbol="$1"
  local response program error
  response="$(curl -fsS \
    -H 'Content-Type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSymbolRegistry\",\"params\":[\"$symbol\"]}" \
    "$RPC_URL")"
  error="$(printf '%s' "$response" | jq -r '.error.message // empty')"
  if [ -n "$error" ]; then
    echo "FATAL: getSymbolRegistry($symbol) failed: $error" >&2
    exit 1
  fi
  program="$(printf '%s' "$response" | jq -r '.result.program // empty')"
  if [ -z "$program" ] || [ "$program" = "null" ]; then
    echo "FATAL: getSymbolRegistry($symbol) did not return a program" >&2
    exit 1
  fi
  printf '%s' "$program"
}

tmp="$(mktemp)"
cleanup() {
  rm -f "$tmp" "$tmp.next"
}
trap cleanup EXIT

keys=(
  CUSTODY_LUSD_TOKEN_ADDR
  CUSTODY_WSOL_TOKEN_ADDR
  CUSTODY_WETH_TOKEN_ADDR
  CUSTODY_WBNB_TOKEN_ADDR
  CUSTODY_WGAS_TOKEN_ADDR
  CUSTODY_WNEO_TOKEN_ADDR
  CUSTODY_WBTC_TOKEN_ADDR
)

{
  printf 'CUSTODY_LUSD_TOKEN_ADDR=%s\n' "$(fetch_program LUSD)"
  printf 'CUSTODY_WSOL_TOKEN_ADDR=%s\n' "$(fetch_program WSOL)"
  printf 'CUSTODY_WETH_TOKEN_ADDR=%s\n' "$(fetch_program WETH)"
  printf 'CUSTODY_WBNB_TOKEN_ADDR=%s\n' "$(fetch_program WBNB)"
  printf 'CUSTODY_WGAS_TOKEN_ADDR=%s\n' "$(fetch_program WGAS)"
  printf 'CUSTODY_WNEO_TOKEN_ADDR=%s\n' "$(fetch_program WNEO)"
  printf 'CUSTODY_WBTC_TOKEN_ADDR=%s\n' "$(fetch_program WBTC)"
} > "$tmp"

awk -v keys="$(IFS=" "; echo "${keys[*]}")" '
  BEGIN {
    split(keys, entries, " ")
    for (idx in entries) remove[entries[idx]]=1
  }
  {
    line=$0
    if (line ~ /^[[:space:]]*#/) {
      print
      next
    }
    key=line
    sub(/=.*/, "", key)
    sub(/^[[:space:]]*/, "", key)
    if (remove[key]) next
    print
  }
' "$ENV_FILE" > "$tmp.next"

{
  cat "$tmp.next"
  printf '\n# Lichen wrapped contract pins refreshed by scripts/sync-custody-wrapped-contracts.sh\n'
  cat "$tmp"
} > "$tmp.next.2"
mv "$tmp.next.2" "$tmp.next"

mode="$(stat -c '%a' "$ENV_FILE" 2>/dev/null || echo 600)"
owner_group="$(stat -c '%u:%g' "$ENV_FILE" 2>/dev/null || true)"
cat "$tmp.next" > "$ENV_FILE"
chmod "$mode" "$ENV_FILE"
if [ -n "$owner_group" ]; then
  chown "$owner_group" "$ENV_FILE"
fi

echo "synced wrapped token contract pins in $ENV_FILE from $RPC_URL"
