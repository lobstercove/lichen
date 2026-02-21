#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════════
# MoltChain Mainnet Deploy Script (P9-INF-03)
# Wraps the deploy pipeline with mainnet-specific safety checks.
#
# Usage: ./mainnet-deploy.sh --rpc URL --network=mainnet [--admin PUBKEY]
# ═══════════════════════════════════════════════════════════════════════════════
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ── Safety: require explicit --network=mainnet ──
NETWORK=""
RPC_URL="${MOLTCHAIN_RPC_URL:-}"
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --network=mainnet) NETWORK="mainnet"; shift ;;
        --network=*) echo "❌ This script is for mainnet only. Got: $1"; exit 1 ;;
        --rpc) RPC_URL="$2"; shift 2 ;;
        --rpc=*) RPC_URL="${1#--rpc=}"; shift ;;
        *) EXTRA_ARGS+=("$1"); shift ;;
    esac
done

if [[ "$NETWORK" != "mainnet" ]]; then
    echo "❌ Must specify --network=mainnet explicitly"
    exit 1
fi

if [[ -z "$RPC_URL" ]]; then
    echo "❌ --rpc URL is required for mainnet deployment"
    exit 1
fi

if [[ -z "${MOLTCHAIN_ADMIN_PUBKEY:-}" ]]; then
    echo "❌ MOLTCHAIN_ADMIN_PUBKEY must be set for mainnet (no default deployer)"
    exit 1
fi

# ── Final confirmation ──
echo "═══════════════════════════════════════════════════════"
echo "⚠️  MAINNET DEPLOYMENT"
echo "═══════════════════════════════════════════════════════"
echo "  RPC:   $RPC_URL"
echo "  Admin: $MOLTCHAIN_ADMIN_PUBKEY"
echo "═══════════════════════════════════════════════════════"
echo ""
read -p "Type 'DEPLOY' to confirm mainnet deployment: " confirm
if [[ "$confirm" != "DEPLOY" ]]; then
    echo "Aborted."
    exit 1
fi

# ── Do NOT seed test pairs/pools on mainnet ──
exec "$SCRIPT_DIR/testnet-deploy.sh" \
    --rpc "$RPC_URL" \
    --admin "$MOLTCHAIN_ADMIN_PUBKEY" \
    "${EXTRA_ARGS[@]}"
