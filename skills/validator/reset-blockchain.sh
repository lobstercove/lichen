#!/bin/bash
# ============================================================================
# MoltChain - Full Blockchain Reset
# ============================================================================
#
# Stops ALL running services, removes ALL persistent state, and prepares
# for a clean genesis restart. Works regardless of port scheme or data path.
#
# Usage:
#   ./reset-blockchain.sh              # Reset everything
#   ./reset-blockchain.sh --restart    # Reset + restart local testnet
#   ./reset-blockchain.sh testnet      # Reset testnet state only
#   ./reset-blockchain.sh mainnet      # Reset mainnet state only
#
# All paths are resolved relative to the repo root (auto-detected).
# Works on any machine regardless of install location.
# ============================================================================

set -euo pipefail

# Colors
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'

# Resolve repo root (works from any CWD)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/../.."
REPO_ROOT="$(cd "$REPO_ROOT" && pwd)"

# Parse args
NETWORK="all"
RESTART=false

for arg in "$@"; do
    case "$arg" in
        --restart) RESTART=true ;;
        testnet|mainnet|all) NETWORK="$arg" ;;
    esac
done

if [ "$RESTART" = true ] && [ "$NETWORK" = "all" ]; then
    NETWORK="testnet"
fi

echo -e "${RED}=================================================${NC}"
echo -e "${RED}  MoltChain FULL RESET - All State Will Be Destroyed${NC}"
echo -e "${RED}=================================================${NC}"
echo ""
echo "  Repo root: $REPO_ROOT"
echo "  Network:   $NETWORK"
echo ""

# ════════════════════════════════════════════════════════════
# STEP 1: KILL ALL PROCESSES
# ════════════════════════════════════════════════════════════
echo -e "${YELLOW}[1/6] Killing all MoltChain processes...${NC}"

pkill -9 -f moltchain-validator 2>/dev/null || true
pkill -9 -f moltchain-faucet    2>/dev/null || true
pkill -9 -f moltchain-custody   2>/dev/null || true
sleep 1

# Retry if stubborn
if pgrep -f "moltchain-" >/dev/null 2>&1; then
    echo -e "  ${YELLOW}Retrying kill...${NC}"
    pkill -9 -f "moltchain-" 2>/dev/null || true
    sleep 2
fi

# Don't block on zombie PIDs
LEFTOVER=$(pgrep -f "moltchain-(validator|faucet|custody)" 2>/dev/null || true)
if [ -n "$LEFTOVER" ]; then
    echo -e "  ${YELLOW}Warning: PIDs still present: $LEFTOVER${NC}"
else
    echo -e "${GREEN}  All processes stopped${NC}"
fi

# ════════════════════════════════════════════════════════════
# STEP 2: FLUSH ROCKSDB STATE DIRECTORIES
# ════════════════════════════════════════════════════════════
echo -e "${YELLOW}[2/6] Removing blockchain state directories...${NC}"

cd "$REPO_ROOT"

if [ "$NETWORK" = "all" ]; then
    # Remove ALL state dirs regardless of naming convention
    # data/state-{port}, data/state-testnet-{port}, data/state-mainnet-{port}
    rm -rf data/state-* 2>/dev/null && echo "  removed data/state-*" || true

    # Skills subdir (legacy path)
    rm -rf skills/validator/data/state-* 2>/dev/null || true

    # Workspace root orphans (CWD confusion from ~/.openclaw)
    WORKSPACE_ROOT="$(dirname "$(dirname "$REPO_ROOT")")" 2>/dev/null || true
    if [ -d "$WORKSPACE_ROOT/data" ]; then
        rm -rf "$WORKSPACE_ROOT/data/state-"* 2>/dev/null && echo "  removed workspace root orphans" || true
    fi

    # Home directory state
    rm -rf ~/.moltchain/data-* ~/.moltchain/state-* 2>/dev/null || true

    # Custody state
    rm -rf data/custody* 2>/dev/null || true
    rm -rf /tmp/moltchain-custody* 2>/dev/null || true
else
    # Network-specific: remove both naming conventions
    # Convention 1: data/state-{network}-{port}
    rm -rf data/state-${NETWORK}-* 2>/dev/null && echo "  removed data/state-${NETWORK}-*" || true
    # Convention 2: data/state-{port} (used by run-validator.sh)
    if [ "$NETWORK" = "testnet" ]; then
        for port in 8000 8001 8002 8003 8004; do
            rm -rf "data/state-${port}" 2>/dev/null || true
        done
        echo "  removed data/state-800x (testnet ports)"
    elif [ "$NETWORK" = "mainnet" ]; then
        for port in 9000 9001 9002 9003 9004; do
            rm -rf "data/state-${port}" 2>/dev/null || true
        done
        echo "  removed data/state-900x (mainnet ports)"
    fi
    rm -rf skills/validator/data/state-${NETWORK}-* 2>/dev/null || true
    rm -rf data/custody-${NETWORK}* 2>/dev/null || true
fi

echo -e "${GREEN}  State directories flushed${NC}"

# ════════════════════════════════════════════════════════════
# STEP 3: FLUSH VALIDATOR KEYPAIRS
# ════════════════════════════════════════════════════════════
echo -e "${YELLOW}[3/6] Removing validator keypairs...${NC}"

if [ "$NETWORK" = "all" ]; then
    rm -rf ~/.moltchain/validators 2>/dev/null || true
    rm -f ~/.moltchain/validator-*.json 2>/dev/null || true
else
    # Only remove keypairs for the specific network ports
    if [ "$NETWORK" = "testnet" ]; then
        for port in 7001 7002 7003 8000 8001 8002; do
            rm -f "$HOME/.moltchain/validators/validator-${port}.json" 2>/dev/null || true
        done
    else
        for port in 8001 8002 8003 9000 9001 9002; do
            rm -f "$HOME/.moltchain/validators/validator-${port}.json" 2>/dev/null || true
        done
    fi
fi
echo -e "${GREEN}  Validator keypairs cleared (regenerate on start)${NC}"

# ════════════════════════════════════════════════════════════
# STEP 4: FLUSH SIGNER, PEER STORES, GENESIS, TEMP FILES
# ════════════════════════════════════════════════════════════
echo -e "${YELLOW}[4/6] Cleaning signer data, peer stores, genesis files...${NC}"

if [ "$NETWORK" = "all" ]; then
    # Signer keypairs
    rm -rf ~/.moltchain/signer-* ~/.moltchain/signers 2>/dev/null || true
    rm -rf "$REPO_ROOT"/data/signer-* 2>/dev/null || true

    # Peer stores (only check data dirs, not target/)
    find "$REPO_ROOT/data" -maxdepth 3 -name "known-peers.json" -delete 2>/dev/null || true
    rm -f ~/.moltchain/known-peers.json 2>/dev/null || true

    # Genesis files
    rm -f "$REPO_ROOT/genesis.json" 2>/dev/null || true
    find "$REPO_ROOT/data" -maxdepth 3 -name "genesis-wallet.json" -delete 2>/dev/null || true
    find "$REPO_ROOT/data" -maxdepth 3 -name "genesis-keys" -type d -exec rm -rf {} + 2>/dev/null || true
    rm -f ~/.moltchain/genesis-wallet.json 2>/dev/null || true
    rm -rf ~/.moltchain/genesis-keys 2>/dev/null || true

    # Faucet persisted state
    rm -f "$REPO_ROOT/airdrops.json" 2>/dev/null || true

    # Temp files and logs
    rm -rf /tmp/moltchain-* /tmp/validator-* /tmp/molt* 2>/dev/null || true
else
    # Network-specific
    for dir in "$REPO_ROOT"/data/state-${NETWORK}-*; do
        [ -d "$dir" ] || continue
        find "$dir" -name "known-peers.json" -delete 2>/dev/null || true
        find "$dir" -name "genesis-wallet.json" -delete 2>/dev/null || true
        find "$dir" -name "genesis-keys" -type d -exec rm -rf {} + 2>/dev/null || true
        find "$dir" -name "signer-keypair.json" -delete 2>/dev/null || true
    done
fi

echo -e "${GREEN}  All transient state cleaned${NC}"

# ════════════════════════════════════════════════════════════
# STEP 5: VERIFY CLEAN STATE
# ════════════════════════════════════════════════════════════
echo -e "${YELLOW}[5/6] Verifying clean state...${NC}"

DIRTY=0
if [ "$NETWORK" = "all" ]; then
    for dir in "$REPO_ROOT"/data/state-*; do
        if [ -d "$dir" ]; then
            echo -e "  ${RED}STILL EXISTS: $dir${NC}" && DIRTY=1
        fi
    done
fi

if [ $DIRTY -eq 0 ]; then
    echo -e "${GREEN}  All state verified clean${NC}"
else
    echo -e "${RED}  Warning: Some state may remain${NC}"
fi

# ════════════════════════════════════════════════════════════
# STEP 6: OPTIONAL RESTART
# ════════════════════════════════════════════════════════════
echo ""
echo -e "${GREEN}=================================================${NC}"
echo -e "${GREEN}  Reset complete. Ready for fresh genesis.${NC}"
echo -e "${GREEN}=================================================${NC}"
echo ""

if [ "$RESTART" = true ]; then
    echo -e "${YELLOW}Restarting ${NETWORK} local stack...${NC}"
    echo ""

    LAUNCHER="${SCRIPT_DIR}/run-validator.sh"

    if [ ! -x "$LAUNCHER" ]; then
        echo -e "${RED}Launcher not found: $LAUNCHER${NC}"
        exit 1
    fi

    echo "   Starting V1 (primary - creates genesis)..."
    nohup "$LAUNCHER" "$NETWORK" 1 > /tmp/moltchain-v1.log 2>&1 &
    V1_PID=$!
    echo "   V1 PID: $V1_PID"

    echo "   Waiting for V1 genesis (8s)..."
    sleep 8

    echo "   Starting V2 (secondary)..."
    nohup "$LAUNCHER" "$NETWORK" 2 > /tmp/moltchain-v2.log 2>&1 &
    echo "   V2 PID: $!"

    sleep 3

    echo "   Starting V3 (tertiary)..."
    nohup "$LAUNCHER" "$NETWORK" 3 > /tmp/moltchain-v3.log 2>&1 &
    echo "   V3 PID: $!"

    echo ""
    echo "   Waiting for sync (10s)..."
    sleep 10

    echo ""
    echo -e "${GREEN}Stack restarted. Check logs:${NC}"
    echo "   tail -f /tmp/moltchain-v1.log"
    echo "   tail -f /tmp/moltchain-v2.log"
    echo "   tail -f /tmp/moltchain-v3.log"
else
    echo "Next steps:"
    echo "   cd $REPO_ROOT"
    echo ""
    echo "   # Option A: Dev scripts (auto-ports)"
    echo "   ./skills/validator/run-validator.sh testnet 1   # V1 genesis"
    echo "   ./skills/validator/run-validator.sh testnet 2   # V2 sync"
    echo "   ./skills/validator/run-validator.sh testnet 3   # V3 sync"
    echo ""
    echo "   # Option B: Direct binary (explicit ports)"
    echo "   ./target/release/moltchain-validator --p2p-port 8000 --rpc-port 8899 --db-path \$PWD/data/state-8000"
    echo "   ./target/release/moltchain-validator --p2p-port 8001 --rpc-port 8901 --db-path \$PWD/data/state-8001 --bootstrap 127.0.0.1:8000"
    echo "   ./target/release/moltchain-validator --p2p-port 8002 --rpc-port 8903 --db-path \$PWD/data/state-8002 --bootstrap 127.0.0.1:8000"
    echo ""
    echo "   First validator creates genesis - start it first!"
fi
