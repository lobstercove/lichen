#!/usr/bin/env bash
# ============================================================================
# MoltChain Validator — Stop Script
# ============================================================================
#
# Gracefully stops a running MoltChain validator started by moltchain-start.sh.
#
# Usage:
#   ./moltchain-stop.sh testnet           # Stop testnet validator
#   ./moltchain-stop.sh mainnet           # Stop mainnet validator
#   ./moltchain-stop.sh all               # Stop all validators
#
# ============================================================================

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

NETWORK=${1:-all}
NETWORK=$(echo "$NETWORK" | tr '[:upper:]' '[:lower:]')

case $NETWORK in
    all|testnet|mainnet) ;;
    *)
        echo "Usage: $0 [testnet|mainnet|all]"
        exit 1
        ;;
esac

echo -e "${CYAN}🦞 Stopping MoltChain Validator ($NETWORK)${NC}"

stop_network() {
    local net=$1
    local pid_file="/tmp/moltchain-${net}/pids.env"

    if [ -f "$pid_file" ]; then
        source "$pid_file"
        for pid_var in SUPERVISOR_PID VALIDATOR_PID DEPLOY_PID CUSTODY_PID; do
            pid=${!pid_var:-}
            if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
                echo -e "  Stopping $pid_var ($pid)..."
                kill "$pid" 2>/dev/null || true
            fi
        done
        rm -f "$pid_file"
        echo -e "  ${GREEN}✅ $net stopped${NC}"
    else
        # Fallback: kill by port
        local p2p_port
        case $net in
            testnet) p2p_port=7001 ;;
            mainnet) p2p_port=8001 ;;
        esac
        pkill -f "moltchain-validator.*--p2p-port ${p2p_port}" 2>/dev/null || true
        pkill -f "moltchain-custody.*${net}" 2>/dev/null || true
        echo -e "  ${GREEN}✅ $net stopped (by port match)${NC}"
    fi
}

if [ "$NETWORK" = "all" ]; then
    stop_network testnet
    stop_network mainnet
    # Kill any strays
    pkill -f "target/release/moltchain-validator" 2>/dev/null || true
    pkill -f "target/debug/moltchain-validator" 2>/dev/null || true
    pkill -f "moltchain-custody" 2>/dev/null || true
    pkill -f "moltchain-faucet" 2>/dev/null || true
else
    stop_network "$NETWORK"
fi

sleep 1

# Verify
if pgrep -f "moltchain-validator\|moltchain-faucet" >/dev/null 2>&1; then
    echo -e "${YELLOW}⚠  Some processes may still be running:${NC}"
    pgrep -la "moltchain-validator\|moltchain-faucet" 2>/dev/null || true
else
    echo -e "${GREEN}✅ All validators and services stopped.${NC}"
fi
