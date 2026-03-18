#!/usr/bin/env bash
# ============================================================================
# MoltChain Validator — Production Start Script
# ============================================================================
#
# The official script for starting a MoltChain validator node. Handles both
# first-boot genesis creation AND bootstrapping from an existing network.
#
# Usage:
#   ./moltchain-start.sh testnet              # Start testnet validator
#   ./moltchain-start.sh mainnet              # Start mainnet validator
#   ./moltchain-start.sh testnet --bootstrap seed1.moltchain.network:7001
#   ./moltchain-start.sh testnet --no-deploy  # Skip first-boot deployment
#   ./moltchain-start.sh testnet --custody    # Also start custody service
#   ./moltchain-start.sh testnet --build      # Force rebuild before start
#   ./moltchain-start.sh testnet --foreground # Run validator in foreground
#
# Port assignments (canonical V1, matching run-validator.sh):
#   Testnet: RPC=8899  WS=8900  P2P=7001  Signer=9201
#   Mainnet: RPC=9899  WS=9900  P2P=8001  Signer=9201
#
# First-boot behavior:
#   If no existing blockchain state is found, the validator starts in genesis
#   mode: it creates the chain, treasury keys (500M MOLT supply), and auto-runs
#   first-boot-deploy.sh to deploy all 29 smart contracts (DEX, wrapped tokens,
#   core infrastructure), seed AMM pools, and fund the insurance reserve.
#
# Bootstrap behavior:
#   If --bootstrap is provided, the validator syncs genesis + state from the
#   specified peer and bootstraps into the network as a new validator with a
#   10,000 MOLT bootstrap grant.
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
NO_FAUCET=false
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
        --no-faucet)
            NO_FAUCET=true
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
            echo "  --no-faucet              Skip faucet service (testnet only)"
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

# ── Port assignments (canonical: testnet P2P=7001, mainnet P2P=8001) ──
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

DB_PATH="./data/state-${NETWORK}"
LOG_DIR="/tmp/moltchain-${NETWORK}"
TREASURY_KEYPAIR="${DB_PATH}/genesis-keys/treasury-${CHAIN_ID}.json"
BIN_PATH="./target/release/moltchain-validator"
SUPERVISOR_PATH="${REPO_ROOT}/scripts/validator-supervisor.sh"
VALIDATOR_HOME="${DB_PATH}/home"
REAL_HOME="$HOME"

mkdir -p "$LOG_DIR"

# ── Detect first boot vs bootstrap ──
# !! Must happen BEFORE mkdir "$VALIDATOR_HOME" — creating the home subdir would
#    make the DB_PATH non-empty and falsely trigger RESUME mode.
IS_GENESIS=false
HAS_CHAIN_STATE=false
if [ -d "$DB_PATH" ] && [ -f "$DB_PATH/CURRENT" ]; then
    HAS_CHAIN_STATE=true
fi

if [ -n "$BOOTSTRAP_PEERS" ]; then
    : # bootstrap mode — detected below in banner
elif ! $HAS_CHAIN_STATE; then
    IS_GENESIS=true
fi

# Now safe to create the home directory inside DB_PATH
mkdir -p "$VALIDATOR_HOME"

# Keep node identity/fingerprint state isolated per validator data directory.
export HOME="$VALIDATOR_HOME"

# Preserve the real home so the validator binary can store keypairs there
# (outside the state directory — survives `rm -rf data/state-*` flushes).
export MOLTCHAIN_REAL_HOME="$REAL_HOME"

# Preserve ZK verification key paths from the real home directory so the
# validator can find them even though HOME was overridden above.
export MOLTCHAIN_ZK_SHIELD_VK_PATH="${MOLTCHAIN_ZK_SHIELD_VK_PATH:-${REAL_HOME}/.moltchain/zk/vk_shield.bin}"
export MOLTCHAIN_ZK_UNSHIELD_VK_PATH="${MOLTCHAIN_ZK_UNSHIELD_VK_PATH:-${REAL_HOME}/.moltchain/zk/vk_unshield.bin}"
export MOLTCHAIN_ZK_TRANSFER_VK_PATH="${MOLTCHAIN_ZK_TRANSFER_VK_PATH:-${REAL_HOME}/.moltchain/zk/vk_transfer.bin}"

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

if [ -n "$BOOTSTRAP_PEERS" ]; then
    echo -e "  ${BOLD}Mode:${NC}       ${CYAN}BOOTSTRAP${NC} — syncing from $BOOTSTRAP_PEERS"
elif $IS_GENESIS; then
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
GENESIS_BIN="./target/release/moltchain-genesis"
if $FORCE_BUILD || [ ! -x "$BIN_PATH" ] || [ ! -x "$GENESIS_BIN" ]; then
    echo -e "${CYAN}[1/4]${NC} Building moltchain binaries..."
    cargo build --release --bin moltchain-validator --bin moltchain-genesis --bin moltchain-faucet 2>&1 | tail -5
    echo -e "  ${GREEN}✅ Build complete${NC}"
else
    echo -e "${CYAN}[1/4]${NC} Binaries found: $BIN_PATH, $GENESIS_BIN"
fi
echo ""

if [ ! -x "$SUPERVISOR_PATH" ]; then
    echo -e "${RED}Error: supervisor script missing or not executable: $SUPERVISOR_PATH${NC}"
    exit 1
fi

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

# Always bind on all interfaces so external peers can connect via QUIC.
VALIDATOR_CMD+=(--listen-addr 0.0.0.0)

# ── Start validator ──
if $IS_GENESIS; then
    echo -e "${CYAN}[2/4]${NC} Running ${GREEN}GENESIS${NC} creation..."
    echo -e "  🎯 Creating genesis block and treasury"
    echo -e "     Treasury keys will be saved to: ${DB_PATH}/genesis-keys/"

    # Fetch real-time prices from Binance for genesis pool pricing
    echo -e "  📈 Fetching real-time prices for genesis pools..."
    PRICE_JSON=$(curl -sf --max-time 10 \
        'https://api.binance.us/api/v3/ticker/price?symbols=["SOLUSDT","ETHUSDT","BNBUSDT"]' 2>/dev/null \
        || curl -sf --max-time 10 \
        'https://api.binance.com/api/v3/ticker/price?symbols=["SOLUSDT","ETHUSDT","BNBUSDT"]' 2>/dev/null \
        || echo '[]')
    if [ "$PRICE_JSON" != "[]" ] && command -v python3 &>/dev/null; then
        eval "$(python3 -c "
import json, sys
try:
    data = json.loads('''$PRICE_JSON''')
    m = {d['symbol']: float(d['price']) for d in data}
    print(f'export GENESIS_SOL_USD={m.get(\"SOLUSDT\", 145.0):.2f}')
    print(f'export GENESIS_ETH_USD={m.get(\"ETHUSDT\", 2600.0):.2f}')
    print(f'export GENESIS_BNB_USD={m.get(\"BNBUSDT\", 620.0):.2f}')
except: pass
" 2>/dev/null)"
        export GENESIS_MOLT_USD="${GENESIS_MOLT_USD:-0.10}"
        echo -e "     SOL=\$${GENESIS_SOL_USD:-?} ETH=\$${GENESIS_ETH_USD:-?} BNB=\$${GENESIS_BNB_USD:-?} MOLT=\$${GENESIS_MOLT_USD}"
    else
        echo -e "  ${YELLOW}⚠  Could not fetch prices, using defaults${NC}"
    fi

    # Run standalone genesis tool BEFORE starting the validator
    echo -e "  🔨 Running moltchain-genesis..."
    "$GENESIS_BIN" --network "$NETWORK" --db-path "$DB_PATH"
    GENESIS_EXIT=$?
    if [ $GENESIS_EXIT -ne 0 ]; then
        echo -e "  ${RED}❌ Genesis creation failed (exit code: $GENESIS_EXIT)${NC}"
        exit 1
    fi
    echo -e "  ${GREEN}✅ Genesis block created successfully${NC}"
    echo ""
    echo -e "  Starting validator on top of genesis state..."
else
    echo -e "${CYAN}[2/4]${NC} Starting validator..."
fi

if $FOREGROUND && ! $IS_GENESIS; then
    # Foreground mode — no first-boot or custody, just run
    echo -e ""
    echo -e "  📊 Adaptive Block Production (Tendermint BFT):"
    echo -e "     • No TXs: Heartbeat ~800ms (0.01 MOLT)"
    echo -e "     • With TXs: ~200ms blocks (0.02 MOLT)"
    echo -e ""
    echo -e "  Starting in foreground (Ctrl+C to stop)..."
    exec "$SUPERVISOR_PATH" "${NETWORK}-primary-p${P2P_PORT}" -- "${VALIDATOR_CMD[@]}"
fi

# Background mode — start validator, then deploy contracts
"$SUPERVISOR_PATH" "${NETWORK}-primary-p${P2P_PORT}" -- "${VALIDATOR_CMD[@]}" >"${LOG_DIR}/validator.log" 2>&1 &
VALIDATOR_PID=$!
SUPERVISOR_PID="$VALIDATOR_PID"
echo -e "  ${GREEN}✅ Validator supervisor started (PID: $SUPERVISOR_PID)${NC}"
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
    # Restore real HOME so Python/pip can find user-installed packages
    export CUSTODY_MOLT_RPC_URL="http://127.0.0.1:${RPC_PORT}"
    HOME="$REAL_HOME" "${REPO_ROOT}/scripts/first-boot-deploy.sh" \
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

# ── Faucet service (testnet only) ──
FAUCET_PID=""
FAUCET_BIN="${REPO_ROOT}/target/release/moltchain-faucet"
FAUCET_KEYPAIR_PATH="${DB_PATH}/genesis-keys/faucet-${CHAIN_ID}.json"
if [ "$NETWORK" = "testnet" ] && ! $NO_FAUCET && [ -x "$FAUCET_BIN" ] && [ -f "$FAUCET_KEYPAIR_PATH" ]; then
    FAUCET_PORT=9100
    if ! lsof -i ":$FAUCET_PORT" >/dev/null 2>&1; then
        echo -e "${CYAN}[faucet]${NC} Starting faucet service on port $FAUCET_PORT..."
        FAUCET_KEYPAIR="$FAUCET_KEYPAIR_PATH" \
        RPC_URL="http://127.0.0.1:${RPC_PORT}" \
        NETWORK="$NETWORK" \
        PORT="$FAUCET_PORT" \
        RUST_LOG=info \
            "$FAUCET_BIN" >"${LOG_DIR}/faucet.log" 2>&1 &
        FAUCET_PID=$!
        echo -e "  ${GREEN}✅ Faucet started (PID: $FAUCET_PID)${NC}"
        echo -e "     Log: ${LOG_DIR}/faucet.log"
    else
        echo -e "${CYAN}[faucet]${NC} ${YELLOW}Skipped${NC} — port $FAUCET_PORT already in use"
    fi
elif [ "$NETWORK" = "testnet" ] && ! $NO_FAUCET; then
    if [ ! -x "$FAUCET_BIN" ]; then
        echo -e "${CYAN}[faucet]${NC} ${YELLOW}Skipped${NC} — faucet binary not found (run: cargo build --release --bin moltchain-faucet)"
    elif [ ! -f "$FAUCET_KEYPAIR_PATH" ]; then
        echo -e "${CYAN}[faucet]${NC} ${YELLOW}Skipped${NC} — faucet keypair not found at $FAUCET_KEYPAIR_PATH"
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
echo -e "  ${BOLD}Supervisor PID:${NC} $SUPERVISOR_PID"
[ -n "${DEPLOY_PID:-}" ]  && echo -e "  ${BOLD}Deploy PID:${NC}     $DEPLOY_PID"
[ -n "${FAUCET_PID:-}" ]  && echo -e "  ${BOLD}Faucet PID:${NC}     $FAUCET_PID"
[ -n "${CUSTODY_PID:-}" ] && echo -e "  ${BOLD}Custody PID:${NC}    $CUSTODY_PID"
echo -e ""
echo -e "  ${BOLD}RPC endpoint:${NC}   http://localhost:$RPC_PORT"
echo -e "  ${BOLD}WebSocket:${NC}      ws://localhost:$WS_PORT"
echo -e "  ${BOLD}Health check:${NC}   curl -s http://localhost:$RPC_PORT -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\"}'"
echo -e ""
echo -e "  ${BOLD}Follow logs:${NC}"
echo -e "    tail -f ${LOG_DIR}/validator.log"
[ -n "${DEPLOY_PID:-}" ]  && echo -e "    tail -f ${LOG_DIR}/first-boot-deploy.log"
[ -n "${FAUCET_PID:-}" ]  && echo -e "    tail -f ${LOG_DIR}/faucet.log"
[ -n "${CUSTODY_PID:-}" ] && echo -e "    tail -f ${LOG_DIR}/custody.log"
echo -e ""
echo -e "  ${BOLD}Stop:${NC}"
echo -e "    kill $SUPERVISOR_PID"
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
SUPERVISOR_PID=$SUPERVISOR_PID
DEPLOY_PID=${DEPLOY_PID:-}
FAUCET_PID=${FAUCET_PID:-}
CUSTODY_PID=${CUSTODY_PID:-}
NETWORK=$NETWORK
RPC_PORT=$RPC_PORT
P2P_PORT=$P2P_PORT
EOF

echo -e "  ${GREEN}🦞 MoltChain is running!${NC}"
