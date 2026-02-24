#!/usr/bin/env bash
# ============================================================================
# MoltChain Validator — Production Start Script
# ============================================================================
#
# The official script for starting a MoltChain validator node. Handles both
# first-boot genesis creation AND joining an existing network.
#
# Usage:
#   ./moltchain-start.sh testnet              # Start testnet validator
#   ./moltchain-start.sh mainnet              # Start mainnet validator
#   ./moltchain-start.sh testnet --bootstrap seed1.moltchain.io:7001
#   ./moltchain-start.sh testnet --no-deploy  # Skip first-boot deployment
#   ./moltchain-start.sh testnet --custody    # Also start custody service
#   ./moltchain-start.sh testnet --build      # Force rebuild before start
#   ./moltchain-start.sh testnet --foreground # Run validator in foreground
#
# Port assignments (one validator per network per machine):
#   Testnet: RPC=8899  WS=8900  P2P=7001  Signer=9201
#   Mainnet: RPC=9899  WS=9900  P2P=8001  Signer=9201
#
# First-boot behavior:
#   If no existing blockchain state is found, the validator starts in genesis
#   mode: it creates the chain, treasury keys (1B MOLT supply), and auto-runs
#   first-boot-deploy.sh to deploy all 26 smart contracts (DEX, wrapped tokens,
#   core infrastructure), seed AMM pools, and fund the insurance reserve.
#
# Joining behavior:
#   If --bootstrap is provided, the validator syncs genesis + state from the
#   specified peer and joins the network as a new validator with a 10,000 MOLT
#   bootstrap grant.
#
# ============================================================================

set -euo pipefail

# ── Colors ──
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ── Resolve repo root ──
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$SCRIPT_DIR"
cd "$REPO_ROOT" || exit 1

# ── Defaults ──
NETWORK=""
BOOTSTRAP_PEERS=""
NO_DEPLOY=false
START_CUSTODY=false
FORCE_BUILD=false
FOREGROUND=false

# ── Parse arguments ──
while [[ $# -gt 0 ]]; do
    case "$1" in
        testnet|mainnet)
            NETWORK="$1"
            ;;
        --bootstrap|--bootstrap-peers)
            shift
            BOOTSTRAP_PEERS="$1"
            ;;
        --bootstrap=*|--bootstrap-peers=*)
            BOOTSTRAP_PEERS="${1#*=}"
            ;;
        --no-deploy)
            NO_DEPLOY=true
            ;;
        --custody)
            START_CUSTODY=true
            ;;
        --build)
            FORCE_BUILD=true
            ;;
        --foreground|-f)
            FOREGROUND=true
            ;;
        --help|-h)
            echo "Usage: $0 <testnet|mainnet> [options]"
            echo ""
            echo "Options:"
            echo "  --bootstrap <host:port>  Bootstrap from existing validator"
            echo "  --no-deploy              Skip first-boot contract deployment"
            echo "  --custody                Also start the custody service"
            echo "  --build                  Force rebuild before starting"
            echo "  --foreground, -f         Run validator in foreground (default: background)"
            echo "  --help, -h               Show this help"
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown argument: $1${NC}"
            echo "Run: $0 --help"
            exit 1
            ;;
    esac
    shift
done

if [ -z "$NETWORK" ]; then
    echo -e "${RED}Error: Network required.${NC}"
    echo "Usage: $0 <testnet|mainnet> [options]"
    exit 1
fi

# ── Port assignments (matches root run-validator.sh V1) ──
case $NETWORK in
    testnet)
        RPC_PORT=8899
        WS_PORT=8900
        P2P_PORT=7001
        SIGNER_PORT=9201
        CHAIN_ID="moltchain-testnet-1"
        ;;
    mainnet)
        RPC_PORT=9899
        WS_PORT=9900
        P2P_PORT=8001
        SIGNER_PORT=9201
        CHAIN_ID="moltchain-mainnet-1"
        ;;
esac

DB_PATH="./data/state-${NETWORK}-${P2P_PORT}"
LOG_DIR="/tmp/moltchain-${NETWORK}"
TREASURY_KEYPAIR="${DB_PATH}/genesis-keys/treasury-${CHAIN_ID}.json"
BIN_PATH="./target/release/moltchain-validator"

mkdir -p "$LOG_DIR"

# ── Banner ──
echo -e "${CYAN}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║       🦞 MoltChain Validator — Production Start         ║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════╝${NC}"
echo -e ""
echo -e "  ${BOLD}Network:${NC}    $NETWORK"
echo -e "  ${BOLD}Chain ID:${NC}   $CHAIN_ID"
echo -e "  ${BOLD}RPC:${NC}        http://localhost:$RPC_PORT"
echo -e "  ${BOLD}WebSocket:${NC}  ws://localhost:$WS_PORT"
echo -e "  ${BOLD}P2P:${NC}        0.0.0.0:$P2P_PORT"
echo -e "  ${BOLD}Signer:${NC}     http://localhost:$SIGNER_PORT"
echo -e "  ${BOLD}DB:${NC}         $DB_PATH"
echo -e "  ${BOLD}Logs:${NC}       $LOG_DIR"
echo -e ""

# ── Detect first boot vs joining ──
IS_GENESIS=false
if [ -n "$BOOTSTRAP_PEERS" ]; then
    echo -e "  ${BOLD}Mode:${NC}       ${CYAN}JOINING${NC} — bootstrapping from $BOOTSTRAP_PEERS"
elif [ ! -d "$DB_PATH" ] || [ -z "$(ls -A "$DB_PATH" 2>/dev/null)" ]; then
    IS_GENESIS=true
    echo -e "  ${BOLD}Mode:${NC}       ${GREEN}GENESIS${NC} — first validator, creating new chain"
else
    echo -e "  ${BOLD}Mode:${NC}       ${GREEN}RESUME${NC} — existing chain state found"
fi
echo -e ""

# ── Check for already-running validator ──
if lsof -i ":$P2P_PORT" >/dev/null 2>&1; then
    echo -e "${RED}Error: Port $P2P_PORT already in use.${NC}"
    echo -e "A ${NETWORK} validator may already be running. Check with:"
    echo -e "  lsof -i :$P2P_PORT"
    exit 1
fi

# ── Build binary ──
if $FORCE_BUILD || [ ! -x "$BIN_PATH" ]; then
    echo -e "${CYAN}[1/4]${NC} Building moltchain-validator..."
    cargo build --release --bin moltchain-validator 2>&1 | tail -5
    echo -e "  ${GREEN}✅ Build complete${NC}"
else
    echo -e "${CYAN}[1/4]${NC} Binary found: $BIN_PATH"
fi
echo ""

# ── Set environment ──
export MOLTCHAIN_SIGNER_BIND="0.0.0.0:${SIGNER_PORT}"

# ── Build validator command ──
VALIDATOR_CMD=("$BIN_PATH"
    --network "$NETWORK"
    --rpc-port "$RPC_PORT"
    --ws-port "$WS_PORT"
    --p2p-port "$P2P_PORT"
    --db-path "$DB_PATH"
)

if [ -n "$BOOTSTRAP_PEERS" ]; then
    VALIDATOR_CMD+=(--bootstrap-peers "$BOOTSTRAP_PEERS")
fi

# ── Start validator ──
if $IS_GENESIS; then
    echo -e "${CYAN}[2/4]${NC} Starting ${GREEN}GENESIS${NC} validator..."
    echo -e "  🎯 This validator will create the chain and treasury"
    echo -e "     Treasury keys will be saved to: ${DB_PATH}/genesis-keys/"
else
    echo -e "${CYAN}[2/4]${NC} Starting validator..."
fi

if $FOREGROUND && ! $IS_GENESIS; then
    # Foreground mode — no first-boot or custody, just run
    echo -e ""
    echo -e "  📊 Adaptive Block Production:"
    echo -e "     • No TXs: Heartbeat every 5s (0.135 MOLT)"
    echo -e "     • With TXs: 400ms blocks (0.9 MOLT)"
    echo -e ""
    echo -e "  Starting in foreground (Ctrl+C to stop)..."
    exec "${VALIDATOR_CMD[@]}"
fi

# Background mode — start validator, then deploy contracts
"${VALIDATOR_CMD[@]}" >"${LOG_DIR}/validator.log" 2>&1 &
VALIDATOR_PID=$!
echo -e "  ${GREEN}✅ Validator started (PID: $VALIDATOR_PID)${NC}"
echo -e "     Log: ${LOG_DIR}/validator.log"
echo ""

# Verify validator is running after a moment
sleep 2
if ! kill -0 "$VALIDATOR_PID" 2>/dev/null; then
    echo -e "${RED}❌ Validator crashed on startup. Last 20 lines of log:${NC}"
    tail -20 "${LOG_DIR}/validator.log" 2>/dev/null || true
    exit 1
fi

# ── First-boot contract deployment ──
if $IS_GENESIS && ! $NO_DEPLOY; then
    echo -e "${CYAN}[3/4]${NC} Running first-boot contract deployment..."
    echo -e "  Waiting for validator to reach healthy state..."

    # Run first-boot-deploy in background, it has its own retry/wait logic
    export CUSTODY_MOLT_RPC_URL="http://127.0.0.1:${RPC_PORT}"
    "${REPO_ROOT}/scripts/first-boot-deploy.sh" \
        --rpc "http://127.0.0.1:${RPC_PORT}" \
        >"${LOG_DIR}/first-boot-deploy.log" 2>&1 &
    DEPLOY_PID=$!
    echo -e "  ${GREEN}✅ First-boot deployer started (PID: $DEPLOY_PID)${NC}"
    echo -e "     Log: ${LOG_DIR}/first-boot-deploy.log"
    echo -e "     Deploys: 26 contracts, DEX pairs, AMM pools, insurance fund"
else
    DEPLOY_PID=""
    if $NO_DEPLOY; then
        echo -e "${CYAN}[3/4]${NC} ${YELLOW}Skipped${NC} — first-boot deployment (--no-deploy)"
    elif ! $IS_GENESIS; then
        echo -e "${CYAN}[3/4]${NC} ${YELLOW}Skipped${NC} — not a genesis boot"
    fi
fi
echo ""

# ── Optional custody service ──
if $START_CUSTODY; then
    echo -e "${CYAN}[4/4]${NC} Starting custody service..."
    export CUSTODY_MOLT_RPC_URL="http://127.0.0.1:${RPC_PORT}"
    export CUSTODY_TREASURY_KEYPAIR="$TREASURY_KEYPAIR"
    "${REPO_ROOT}/scripts/run-custody.sh" "$NETWORK" \
        >"${LOG_DIR}/custody.log" 2>&1 &
    CUSTODY_PID=$!
    echo -e "  ${GREEN}✅ Custody started (PID: $CUSTODY_PID)${NC}"
    echo -e "     Log: ${LOG_DIR}/custody.log"
else
    CUSTODY_PID=""
    echo -e "${CYAN}[4/4]${NC} ${YELLOW}Skipped${NC} — custody service (use --custody to enable)"
fi

# ── Summary ──
echo -e ""
echo -e "${CYAN}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  MoltChain Validator Running                             ║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════╝${NC}"
echo -e ""
echo -e "  ${BOLD}Validator PID:${NC}  $VALIDATOR_PID"
[ -n "${DEPLOY_PID:-}" ]  && echo -e "  ${BOLD}Deploy PID:${NC}     $DEPLOY_PID"
[ -n "${CUSTODY_PID:-}" ] && echo -e "  ${BOLD}Custody PID:${NC}    $CUSTODY_PID"
echo -e ""
echo -e "  ${BOLD}RPC endpoint:${NC}   http://localhost:$RPC_PORT"
echo -e "  ${BOLD}WebSocket:${NC}      ws://localhost:$WS_PORT"
echo -e "  ${BOLD}Health check:${NC}   curl -s http://localhost:$RPC_PORT -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\"}'"
echo -e ""
echo -e "  ${BOLD}Follow logs:${NC}"
echo -e "    tail -f ${LOG_DIR}/validator.log"
[ -n "${DEPLOY_PID:-}" ]  && echo -e "    tail -f ${LOG_DIR}/first-boot-deploy.log"
[ -n "${CUSTODY_PID:-}" ] && echo -e "    tail -f ${LOG_DIR}/custody.log"
echo -e ""
echo -e "  ${BOLD}Stop:${NC}"
echo -e "    kill $VALIDATOR_PID"
[ -n "${CUSTODY_PID:-}" ] && echo -e "    kill $CUSTODY_PID"
echo -e ""

# ── Foreground wait (genesis mode) ──
if $FOREGROUND && $IS_GENESIS; then
    echo -e "  Waiting for first-boot deployment to finish, then switching to foreground..."
    if [ -n "${DEPLOY_PID:-}" ]; then
        wait "$DEPLOY_PID" 2>/dev/null || true
        echo -e "  ${GREEN}✅ First-boot deployment complete${NC}"
    fi
    echo -e "  ${BOLD}Switching to foreground (Ctrl+C to stop)...${NC}"
    echo -e ""
    # Bring validator to foreground by tailing its output
    wait "$VALIDATOR_PID"
fi

# ── Write PID file for stop script ──
cat > "${LOG_DIR}/pids.env" <<EOF
VALIDATOR_PID=$VALIDATOR_PID
DEPLOY_PID=${DEPLOY_PID:-}
CUSTODY_PID=${CUSTODY_PID:-}
NETWORK=$NETWORK
RPC_PORT=$RPC_PORT
P2P_PORT=$P2P_PORT
EOF

echo -e "  ${GREEN}🦞 MoltChain is running!${NC}"
