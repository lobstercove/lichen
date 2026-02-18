#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════════
# Seed Insurance Fund
# Transfers MOLT tokens to the dex_margin insurance fund
#
# Usage: ./seed-insurance-fund.sh [--amount 100000] [--rpc URL]
# ═══════════════════════════════════════════════════════════════════════════════
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
RPC_URL="${MOLTCHAIN_RPC_URL:-http://localhost:8899}"
AMOUNT=100000  # 100K MOLT default

while [[ $# -gt 0 ]]; do
    case "$1" in
        --amount) AMOUNT="$2"; shift 2 ;;
        --rpc) RPC_URL="$2"; shift 2 ;;
        --help) echo "Usage: $0 [--amount N] [--rpc URL]"; exit 0 ;;
        *) echo "Unknown: $1"; exit 1 ;;
    esac
done

echo "═══════════════════════════════════════════════"
echo "  Insurance Fund Seeding"
echo "  Amount: ${AMOUNT} MOLT"
echo "  RPC:    ${RPC_URL}"
echo "═══════════════════════════════════════════════"

# Check margin contract insurance fund balance
MARGIN_INFO=$(curl -s "$RPC_URL/api/v1/margin/info" 2>/dev/null || echo '{}')
CURRENT=$(echo "$MARGIN_INFO" | python3 -c "
import json,sys
try:
    d = json.load(sys.stdin)
    fund = d.get('data', {}).get('insurance_fund', 0)
    print(fund)
except:
    print(0)
" 2>/dev/null || echo "0")

echo "Current insurance fund: $CURRENT"
echo "Target:                 $AMOUNT"

if [[ "$CURRENT" -ge "$AMOUNT" ]]; then
    echo "✅ Insurance fund already meets minimum. No action needed."
    exit 0
fi

NEEDED=$((AMOUNT - CURRENT))
echo "Need to add:            $NEEDED MOLT"

# Send transaction to add to insurance fund
# This calls dex_margin::add_to_insurance(amount)
# Opcode 0x06 = add_to_insurance
echo ""
echo "To seed the insurance fund, run:"
echo "  moltchain tx send --to dex_margin --data 0x06$(printf '%016x' $NEEDED) --rpc $RPC_URL"
echo ""
echo "Or via the admin dashboard, call dex_margin::add_to_insurance($NEEDED)"
