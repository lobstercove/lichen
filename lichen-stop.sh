#!/usr/bin/env bash
# ============================================================================
# Lichen Validator — Stop Script
# ============================================================================
#
# Gracefully stops a running Lichen validator started by lichen-start.sh.
#
# Usage:
#   ./lichen-stop.sh testnet           # Stop testnet validator
#   ./lichen-stop.sh mainnet           # Stop mainnet validator
#   ./lichen-stop.sh all               # Stop all validators
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

echo -e "${CYAN}🦞 Stopping Lichen Validator ($NETWORK)${NC}"

stop_network() {
    local net=$1
    local pid_file="/tmp/lichen-${net}/pids.env"

    if [ -f "$pid_file" ]; then
        source "$pid_file"
        for pid_var in SUPERVISOR_PID VALIDATOR_PID DEPLOY_PID FAUCET_PID CUSTODY_PID; do
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
        pkill -f "lichen-validator.*--p2p-port ${p2p_port}" 2>/dev/null || true
        pkill -f "lichen-faucet" 2>/dev/null || true
        pkill -f "lichen-custody.*${net}" 2>/dev/null || true
        echo -e "  ${GREEN}✅ $net stopped (by port match)${NC}"
    fi
}

if [ "$NETWORK" = "all" ]; then
    stop_network testnet
    stop_network mainnet
    # Kill any strays
    pkill -f "target/release/lichen-validator" 2>/dev/null || true
    pkill -f "target/debug/lichen-validator" 2>/dev/null || true
    pkill -f "lichen-custody" 2>/dev/null || true
    pkill -f "lichen-faucet" 2>/dev/null || true
else
    stop_network "$NETWORK"
fi

sleep 1

# Verify
if pgrep -f "lichen-validator\|lichen-faucet" >/dev/null 2>&1; then
    echo -e "${YELLOW}⚠  Some processes may still be running:${NC}"
    pgrep -la "lichen-validator\|lichen-faucet" 2>/dev/null || true
else
    echo -e "${GREEN}✅ All validators and services stopped.${NC}"
fi
