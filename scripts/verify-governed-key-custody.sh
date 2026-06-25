#!/usr/bin/env bash
set -euo pipefail

RPC_URL="http://127.0.0.1:8899"
KEYS_DIR=""
LICHEN_BIN="${LICHEN_BIN:-lichen}"
REQUIRED_ROLES="genesis,validator_rewards,community_treasury,builder_grants,founding_symbionts,ecosystem_partnerships,reserve_pool"

usage() {
    cat <<'EOF'
Usage: scripts/verify-governed-key-custody.sh --keys-dir <path> [options]

Verifies that an offline/private key bundle contains loadable keypairs for the
live genesis governed signer set returned by getGenesisAccounts.

Options:
  --rpc-url <url>       RPC URL to query (default: http://127.0.0.1:8899)
  --keys-dir <path>     Directory containing encrypted genesis/governed keypair JSON files
  --roles <csv>         Required roles (default: genesis plus all distribution roles)
  --lichen-bin <path>   lichen CLI binary (default: $LICHEN_BIN or lichen)
  -h, --help            Show this help

Security:
  Set LICHEN_KEYPAIR_PASSWORD in the environment when key files are encrypted.
  This script prints public keys and file paths only. It never prints seeds.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --rpc-url)
            RPC_URL="${2:?--rpc-url requires a value}"
            shift 2
            ;;
        --keys-dir)
            KEYS_DIR="${2:?--keys-dir requires a value}"
            shift 2
            ;;
        --roles)
            REQUIRED_ROLES="${2:?--roles requires a comma-separated value}"
            shift 2
            ;;
        --lichen-bin)
            LICHEN_BIN="${2:?--lichen-bin requires a value}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [[ -z "$KEYS_DIR" ]]; then
    echo "ERROR: --keys-dir is required" >&2
    usage >&2
    exit 2
fi

if [[ ! -d "$KEYS_DIR" ]]; then
    echo "ERROR: keys directory does not exist: $KEYS_DIR" >&2
    exit 2
fi

if ! command -v "$LICHEN_BIN" >/dev/null 2>&1; then
    echo "ERROR: lichen CLI not found: $LICHEN_BIN" >&2
    exit 2
fi

rpc_payload='{"jsonrpc":"2.0","id":1,"method":"getGenesisAccounts","params":[]}'
rpc_response="$(curl -fsS \
    -H 'Content-Type: application/json' \
    -H 'User-Agent: lichen-governed-key-custody-verifier/1' \
    --data "$rpc_payload" \
    "$RPC_URL")"

rpc_file="$(mktemp)"
expected_file="$(mktemp)"
found_file="$(mktemp)"
errors_file="$(mktemp)"
trap 'rm -f "$rpc_file" "$expected_file" "$found_file" "$errors_file"' EXIT
printf '%s' "$rpc_response" >"$rpc_file"

REQUIRED_ROLES="$REQUIRED_ROLES" python3 - "$rpc_file" "$expected_file" <<'PY'
import json
import os
import sys

rpc_path = sys.argv[1]
out_path = sys.argv[2]
required = {item.strip() for item in os.environ["REQUIRED_ROLES"].split(",") if item.strip()}
with open(rpc_path, "r", encoding="utf-8") as handle:
    payload = json.load(handle)
if "error" in payload:
    raise SystemExit(f"RPC error: {payload['error']}")
accounts = payload.get("result", {}).get("accounts")
if not isinstance(accounts, list):
    raise SystemExit("RPC getGenesisAccounts response missing result.accounts")

seen = set()
with open(out_path, "w", encoding="utf-8") as out:
    for account in accounts:
        role = account.get("role")
        pubkey = account.get("pubkey")
        if role in required:
            if not pubkey:
                raise SystemExit(f"genesis account {role} has no pubkey")
            seen.add(role)
            out.write(f"{role}\t{pubkey}\n")

missing = sorted(required - seen)
if missing:
    raise SystemExit(f"RPC getGenesisAccounts missing required roles: {', '.join(missing)}")
PY

while IFS= read -r -d '' key_file; do
    if output="$(LICHEN_KEYPAIR_PASSWORD="${LICHEN_KEYPAIR_PASSWORD:-}" \
        "$LICHEN_BIN" identity show --keypair "$key_file" 2>&1)"; then
        pubkey="$(printf '%s\n' "$output" | awk '/Pubkey:/ {print $NF; exit}')"
        if [[ -n "$pubkey" ]]; then
            printf '%s\t%s\n' "$pubkey" "$key_file" >>"$found_file"
        else
            printf '%s\t%s\n' "$key_file" "identity show produced no Pubkey line" >>"$errors_file"
        fi
    else
        printf '%s\t%s\n' "$key_file" "$output" >>"$errors_file"
    fi
done < <(find "$KEYS_DIR" -type f -name '*.json' -print0 | sort -z)

missing=0
matched=0

echo "Governed key custody verification"
echo "  RPC:      $RPC_URL"
echo "  Keys dir: $KEYS_DIR"
echo ""

while IFS=$'\t' read -r role pubkey; do
    match="$(awk -F '\t' -v pk="$pubkey" '$1 == pk {print $2; exit}' "$found_file")"
    if [[ -n "$match" ]]; then
        printf '  OK      %-26s %s (%s)\n' "$role" "$pubkey" "$match"
        matched=$((matched + 1))
    else
        printf '  MISSING %-26s %s\n' "$role" "$pubkey"
        missing=$((missing + 1))
    fi
done <"$expected_file"

if [[ -s "$errors_file" ]]; then
    echo ""
    echo "Key files that could not be loaded:"
    while IFS=$'\t' read -r file reason; do
        printf '  %s: %s\n' "$file" "$reason"
    done <"$errors_file"
fi

echo ""
echo "Matched governed signer keys: $matched"

if [[ "$missing" -ne 0 ]]; then
    echo "ERROR: missing $missing live governed signer key(s)" >&2
    exit 1
fi

echo "Governed key custody verification passed."
