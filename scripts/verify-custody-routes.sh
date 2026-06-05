#!/usr/bin/env bash
# Validate that a custody env has the source-chain route keys required by the
# operator. This script intentionally reports key names only; it never prints
# configured values.

set -euo pipefail

ENV_FILE="/etc/lichen/custody-env"
ROUTES="${CUSTODY_REQUIRED_ROUTES:-solana,ethereum,bnb,neox,bitcoin}"
REQUIRE_WRAPPED=0

usage() {
  cat <<'EOF'
Usage: bash scripts/verify-custody-routes.sh [--env-file PATH] [--routes LIST] [--require-wrapped]

LIST is a comma-separated route set such as solana,ethereum,bnb,neox,bitcoin.
Use --routes none when source-chain custody is intentionally disabled.
Use --require-wrapped after genesis/symbol-registry sync to require Lichen-side
wrapped-token contract pins as well as source-chain route values.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --env-file)
      ENV_FILE="${2:?missing --env-file value}"
      shift 2
      ;;
    --routes)
      ROUTES="${2:?missing --routes value}"
      shift 2
      ;;
    --require-wrapped)
      REQUIRE_WRAPPED=1
      shift
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

if [ -z "${ROUTES//[[:space:]]/}" ] || [ "$ROUTES" = "none" ]; then
  echo "custody route verification skipped: no required routes"
  exit 0
fi

if printf '%s' "$ROUTES" | grep -q '[^A-Za-z0-9_, -]'; then
  echo "FATAL: invalid route list: $ROUTES" >&2
  exit 1
fi

env_value() {
  local key="$1"
  awk -v key="$key" '
    /^[[:space:]]*#/ { next }
    $0 ~ "^[[:space:]]*" key "=" {
      sub("^[[:space:]]*" key "=", "")
      value=$0
    }
    END {
      if (value != "") print value
    }
  ' "$ENV_FILE"
}

is_placeholder() {
  local value="$1"
  [ -z "$value" ] && return 0
  case "$value" in
    REPLACE_WITH_*|*REPLACE_WITH_*|"<"*">"|"<"*)
      return 0
      ;;
  esac
  return 1
}

missing=()

require_key() {
  local route="$1"
  local key="$2"
  local value

  value="$(env_value "$key" || true)"
  if is_placeholder "$value"; then
    missing+=("${route}:${key}")
  fi
}

require_uint_key() {
  local route="$1"
  local key="$2"
  local value

  value="$(env_value "$key" || true)"
  if is_placeholder "$value"; then
    missing+=("${route}:${key}")
    return
  fi
  if ! printf '%s' "$value" | grep -Eq '^[0-9]+$'; then
    missing+=("${route}:${key} (must be an unsigned integer)")
  fi
}

require_one() {
  local route="$1"
  local label="$2"
  shift 2
  local key value

  for key in "$@"; do
    value="$(env_value "$key" || true)"
    if ! is_placeholder "$value"; then
      return 0
    fi
  done
  missing+=("${route}:${label} ($(IFS=/; echo "$*"))")
}

require_readable_path_key() {
  local route="$1"
  local key="$2"
  local value

  value="$(env_value "$key" || true)"
  if is_placeholder "$value"; then
    missing+=("${route}:${key}")
    return
  fi
  if [ ! -r "$value" ]; then
    missing+=("${route}:${key} (file not readable)")
  fi
}

require_wrapped_key() {
  if [ "$REQUIRE_WRAPPED" = "1" ]; then
    require_key "$@"
  fi
}

verify_solana() {
  require_key solana CUSTODY_SOLANA_RPC_URL
  require_key solana CUSTODY_SOLANA_USDC_MINT
  require_key solana CUSTODY_SOLANA_USDT_MINT
  require_readable_path_key solana CUSTODY_SOLANA_FEE_PAYER
  require_wrapped_key solana CUSTODY_LUSD_TOKEN_ADDR
  require_wrapped_key solana CUSTODY_WSOL_TOKEN_ADDR
}

verify_ethereum() {
  require_one ethereum ethereum_rpc CUSTODY_ETH_RPC_URL CUSTODY_EVM_RPC_URL
  require_uint_key ethereum CUSTODY_ETH_CHAIN_ID
  require_one ethereum ethereum_usdc CUSTODY_ETH_USDC_TOKEN_ADDR CUSTODY_ETH_USDC CUSTODY_EVM_USDC
  require_one ethereum ethereum_usdt CUSTODY_ETH_USDT_TOKEN_ADDR CUSTODY_ETH_USDT CUSTODY_EVM_USDT
  require_wrapped_key ethereum CUSTODY_LUSD_TOKEN_ADDR
  require_wrapped_key ethereum CUSTODY_WETH_TOKEN_ADDR
}

verify_bnb() {
  require_key bnb CUSTODY_BNB_RPC_URL
  require_uint_key bnb CUSTODY_BNB_CHAIN_ID
  require_one bnb bsc_usdc CUSTODY_BSC_USDC_TOKEN_ADDR CUSTODY_BNB_USDC_TOKEN_ADDR CUSTODY_BSC_USDC CUSTODY_BNB_USDC
  require_one bnb bsc_usdt CUSTODY_BSC_USDT_TOKEN_ADDR CUSTODY_BNB_USDT_TOKEN_ADDR CUSTODY_BSC_USDT CUSTODY_BNB_USDT
  require_wrapped_key bnb CUSTODY_LUSD_TOKEN_ADDR
  require_wrapped_key bnb CUSTODY_WBNB_TOKEN_ADDR
}

verify_neox() {
  require_key neox CUSTODY_NEOX_RPC_URL
  require_uint_key neox CUSTODY_NEOX_CHAIN_ID
  require_key neox CUSTODY_NEOX_NEO_TOKEN_ADDR
  require_wrapped_key neox CUSTODY_WGAS_TOKEN_ADDR
  require_wrapped_key neox CUSTODY_WNEO_TOKEN_ADDR
}

verify_bitcoin() {
  require_key bitcoin CUSTODY_BTC_RPC_URL
  require_key bitcoin CUSTODY_BTC_NETWORK
  require_uint_key bitcoin CUSTODY_BTC_CONFIRMATIONS
  require_uint_key bitcoin CUSTODY_BTC_FEE_RATE_SATS_VB
  require_key bitcoin CUSTODY_TREASURY_BTC
  require_wrapped_key bitcoin CUSTODY_WBTC_TOKEN_ADDR
}

normalized_routes="$(printf '%s' "$ROUTES" | tr '[:upper:]' '[:lower:]' | tr ',' ' ')"
for route in $normalized_routes; do
  case "$route" in
    sol|solana) verify_solana ;;
    eth|ethereum) verify_ethereum ;;
    bsc|bnb) verify_bnb ;;
    neo-x|neo_x|neox) verify_neox ;;
    btc|bitcoin) verify_bitcoin ;;
    none) ;;
    *)
      echo "FATAL: unsupported custody route in --routes: $route" >&2
      exit 1
      ;;
  esac
done

if [ "${#missing[@]}" -gt 0 ]; then
  echo "FATAL: missing custody route configuration in $ENV_FILE:" >&2
  for item in "${missing[@]}"; do
    echo "  - $item" >&2
  done
  exit 1
fi

echo "custody route verification passed for: $ROUTES"
