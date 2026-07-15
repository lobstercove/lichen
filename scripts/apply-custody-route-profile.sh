#!/usr/bin/env bash
# Merge an operator-maintained custody route profile into a generated custody env.
# The target env keeps its generated auth tokens, seed paths, wrapped contract
# pins, and treasury keypair path; only approved route keys are replaced.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROFILE_FILE=""
TARGET_ENV="/etc/lichen/custody-env"
ROUTES="${CUSTODY_REQUIRED_ROUTES:-solana,ethereum,bnb,neox,bitcoin}"

usage() {
  cat <<'EOF'
Usage: sudo bash scripts/apply-custody-route-profile.sh --profile PATH [--target PATH] [--routes LIST]

The profile is an env-format file containing only custody route keys. Filled
profiles are operator secrets/config and must not be committed.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --profile)
      PROFILE_FILE="${2:?missing --profile value}"
      shift 2
      ;;
    --target)
      TARGET_ENV="${2:?missing --target value}"
      shift 2
      ;;
    --routes)
      ROUTES="${2:?missing --routes value}"
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

if [ -z "$PROFILE_FILE" ]; then
  echo "FATAL: --profile is required" >&2
  usage >&2
  exit 2
fi
if [ ! -f "$PROFILE_FILE" ]; then
  echo "FATAL: profile not found: $PROFILE_FILE" >&2
  exit 1
fi
if [ ! -f "$TARGET_ENV" ]; then
  echo "FATAL: target env not found: $TARGET_ENV" >&2
  exit 1
fi

allowed_keys=(
  CUSTODY_SOLANA_RPC_URL
  CUSTODY_ETH_RPC_URL
  CUSTODY_ETH_CHAIN_ID
  CUSTODY_BNB_RPC_URL
  CUSTODY_BNB_CHAIN_ID
  CUSTODY_NEOX_RPC_URL
  CUSTODY_NEOX_CHAIN_ID
  CUSTODY_BTC_RPC_URL
  CUSTODY_BTC_RPC_USER
  CUSTODY_BTC_RPC_PASSWORD
  CUSTODY_BTC_NETWORK
  CUSTODY_BTC_CONFIRMATIONS
  CUSTODY_BTC_FEE_RATE_SATS_VB
  CUSTODY_SOLANA_CONFIRMATIONS
  CUSTODY_EVM_CONFIRMATIONS
  CUSTODY_NEOX_CONFIRMATIONS
  CUSTODY_TREASURY_SOLANA
  CUSTODY_TREASURY_ETH
  CUSTODY_TREASURY_BNB
  CUSTODY_TREASURY_NEOX
  CUSTODY_TREASURY_BTC
  CUSTODY_ETH_MULTISIG_ADDRESS
  CUSTODY_BNB_MULTISIG_ADDRESS
  CUSTODY_NEOX_MULTISIG_ADDRESS
  CUSTODY_SOLANA_FEE_PAYER
  CUSTODY_SOLANA_TREASURY_OWNER
  CUSTODY_SOLANA_USDC_MINT
  CUSTODY_SOLANA_USDT_MINT
  CUSTODY_ETH_USDC_TOKEN_ADDR
  CUSTODY_ETH_USDT_TOKEN_ADDR
  CUSTODY_BSC_USDC_TOKEN_ADDR
  CUSTODY_BSC_USDT_TOKEN_ADDR
  CUSTODY_NEOX_NEO_TOKEN_ADDR
)

is_allowed_key() {
  local candidate="$1"
  local key
  for key in "${allowed_keys[@]}"; do
    [ "$candidate" = "$key" ] && return 0
  done
  return 1
}

profile_tmp="$(mktemp)"
target_tmp="$(mktemp)"
cleanup() {
  rm -f "$profile_tmp" "$target_tmp"
}
trap cleanup EXIT

declare -a update_keys=()

while IFS= read -r line || [ -n "$line" ]; do
  line="${line%$'\r'}"
  case "$line" in
    ""|\#*) continue ;;
  esac
  if ! printf '%s\n' "$line" | grep -Eq '^[A-Z0-9_]+='; then
    echo "FATAL: invalid profile line; expected KEY=value" >&2
    exit 1
  fi
  key="${line%%=*}"
  if ! is_allowed_key "$key"; then
    echo "FATAL: unsupported custody route profile key: $key" >&2
    exit 1
  fi
  update_keys+=("$key")
  printf '%s\n' "$line" >> "$profile_tmp"
done < "$PROFILE_FILE"

if [ "${#update_keys[@]}" -eq 0 ]; then
  echo "FATAL: profile contains no route keys" >&2
  exit 1
fi

awk -v keys="$(IFS=" "; echo "${update_keys[*]}")" '
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
' "$TARGET_ENV" > "$target_tmp"

{
  cat "$target_tmp"
  printf '\n# Source-chain custody route profile applied by scripts/apply-custody-route-profile.sh\n'
  cat "$profile_tmp"
} > "$target_tmp.next"
mv "$target_tmp.next" "$target_tmp"

mode="$(stat -c '%a' "$TARGET_ENV" 2>/dev/null || echo 600)"
owner_group="$(stat -c '%u:%g' "$TARGET_ENV" 2>/dev/null || true)"
cat "$target_tmp" > "$TARGET_ENV"
chmod "$mode" "$TARGET_ENV"
if [ -n "$owner_group" ]; then
  chown "$owner_group" "$TARGET_ENV"
fi

bash "$SCRIPT_DIR/verify-custody-routes.sh" --env-file "$TARGET_ENV" --routes "$ROUTES"
echo "applied custody route profile to $TARGET_ENV"
