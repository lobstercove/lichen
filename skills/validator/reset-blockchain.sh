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
#   ./reset-blockchain.sh --restart    # Reset + restart local testnet (dev mode)
#   ./reset-blockchain.sh testnet      # Reset testnet state only
#   ./reset-blockchain.sh mainnet      # Reset mainnet state only
#   ./reset-blockchain.sh --no-keys    # Reset state but keep keypairs
#
# Flags:
#   --restart     Reset + relaunch 3 testnet validators in dev mode
#   --no-keys     Preserve validator keypairs (resume same identities)
#   --dev-mode    Passed through to validators on --restart (default: on)
#
# All paths are resolved relative to the repo root (auto-detected).
# Works on any machine regardless of install location.
# ============================================================================

set -euo pipefail

# Colors
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'

# Resolve repo root (works from any CWD)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/../.."
REPO_ROOT="$(cd "$REPO_ROOT" && pwd)"

# Parse args
NETWORK="all"
RESTART=false
KEEP_KEYS=false
EXTRA_FLAGS=""

for arg in "$@"; do
    case "$arg" in
        --restart)  RESTART=true ;;
        --no-keys)  KEEP_KEYS=true ;;
        --dev-mode) EXTRA_FLAGS="$EXTRA_FLAGS --dev-mode" ;;
        --zk-reset) EXTRA_FLAGS="$EXTRA_FLAGS --zk-reset" ;;
        testnet|mainnet|all) NETWORK="$arg" ;;
    esac
done

if [ "$RESTART" = true ] && [ "$NETWORK" = "all" ]; then
    NETWORK="testnet"
fi

# --restart always implies --dev-mode for local testing (multiple validators
# on one machine require dev-mode to bypass machine fingerprint checks)
if [ "$RESTART" = true ]; then
    if [[ "$EXTRA_FLAGS" != *"--dev-mode"* ]]; then
        EXTRA_FLAGS="$EXTRA_FLAGS --dev-mode"
    fi
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
echo -e "${YELLOW}[1/7] Killing all MoltChain processes...${NC}"

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
echo -e "${YELLOW}[2/7] Removing blockchain state directories...${NC}"

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

    # NOTE: ~/.moltchain/zk/ is intentionally NOT wiped.
    # ZK verification/proving keys are deterministic Groth16 parameters — they
    # contain no blockchain state.  Regenerating them takes ~10s (standalone
    # zk-setup binary) but gains nothing security-wise.  Add --zk-reset to
    # explicitly force regeneration (useful after changing circuit parameters).
    if [[ "${EXTRA_FLAGS:-}" == *"--zk-reset"* ]]; then
        rm -rf ~/.moltchain/zk 2>/dev/null && echo "  removed ZK key cache (--zk-reset)"
    fi

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
# STEP 3: FLUSH VALIDATOR KEYPAIRS (unless --no-keys)
# ════════════════════════════════════════════════════════════
if [ "$KEEP_KEYS" = true ]; then
    echo -e "${CYAN}[3/7] Keeping validator keypairs (--no-keys)${NC}"
else
    echo -e "${YELLOW}[3/7] Removing validator keypairs...${NC}"

    if [ "$NETWORK" = "all" ]; then
        rm -rf ~/.moltchain/validators 2>/dev/null || true
        rm -f ~/.moltchain/validator-*.json 2>/dev/null || true

        # Keypairs copied via --import-key into data dirs
        find "$REPO_ROOT/data" -maxdepth 3 -name "validator-keypair.json" -delete 2>/dev/null || true
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
fi

# ════════════════════════════════════════════════════════════
# STEP 4: FLUSH SIGNER, PEER STORES, GENESIS, TEMP FILES
# ════════════════════════════════════════════════════════════
echo -e "${YELLOW}[4/7] Cleaning signer data, peer stores, genesis files...${NC}"

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
# STEP 5: FLUSH MACHINE FINGERPRINT & MIGRATION ARTIFACTS
# ════════════════════════════════════════════════════════════
echo -e "${YELLOW}[5/7] Cleaning machine fingerprint and migration artifacts...${NC}"

if [ "$NETWORK" = "all" ]; then
    # Imported/migrated keypair temp files in /tmp
    rm -f /tmp/v*-migrated-keypair.json 2>/dev/null || true
    rm -f /tmp/*-keypair-backup-*.json 2>/dev/null || true

    # Migration metadata that may be left over in data dirs
    find "$REPO_ROOT/data" -maxdepth 3 -name "migration-*.json" -delete 2>/dev/null || true
    find "$REPO_ROOT/data" -maxdepth 3 -name "fingerprint-*.dat" -delete 2>/dev/null || true

    # Any stale validator PID files
    rm -f /tmp/moltchain-validator-*.pid 2>/dev/null || true

    # Dev signer temp files
    rm -f /tmp/signer-*.json 2>/dev/null || true
else
    # Network-specific: clean migration artifacts from matching data dirs
    for dir in "$REPO_ROOT"/data/state-${NETWORK}-*; do
        [ -d "$dir" ] || continue
        find "$dir" -name "migration-*.json" -delete 2>/dev/null || true
        find "$dir" -name "fingerprint-*.dat" -delete 2>/dev/null || true
    done
fi

echo -e "${GREEN}  Fingerprint and migration artifacts cleaned${NC}"

# ════════════════════════════════════════════════════════════
# STEP 5b: FLUSH LOGS & DEPLOY MANIFEST (dev convenience)
# ════════════════════════════════════════════════════════════
echo -e "${YELLOW}[5b/7] Cleaning logs and deploy manifest...${NC}"

if [ "$NETWORK" = "all" ] || [ "$NETWORK" = "testnet" ]; then
    # Flush validator logs so new runs get clean log files
    rm -f "$REPO_ROOT"/logs/v*.log 2>/dev/null && echo "  removed logs/v*.log" || true

    # Flush deploy manifest (contract addresses change on each genesis)
    rm -f "$REPO_ROOT/deploy-manifest.json" 2>/dev/null && echo "  removed deploy-manifest.json" || true

    # Flush faucet keypair (regenerated on genesis)
    rm -f "$REPO_ROOT/faucet-keypair.json" 2>/dev/null || true

    # Flush test artifacts that reference old contract addresses
    rm -f "$REPO_ROOT"/artifacts/*.json 2>/dev/null && echo "  removed artifacts/*.json" || true
fi

echo -e "${GREEN}  Dev artifacts cleaned${NC}"

# ════════════════════════════════════════════════════════════
# STEP 6: VERIFY CLEAN STATE
# ════════════════════════════════════════════════════════════
echo -e "${YELLOW}[6/7] Verifying clean state...${NC}"

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
# STEP 7: OPTIONAL RESTART
# ════════════════════════════════════════════════════════════
echo ""
echo -e "${GREEN}=================================================${NC}"
echo -e "${GREEN}  Reset complete. Ready for fresh genesis.${NC}"
echo -e "${GREEN}=================================================${NC}"
echo ""

if [ "$RESTART" = true ]; then
    echo -e "${YELLOW}Restarting ${NETWORK} local stack (dev mode)...${NC}"
    echo ""

    LAUNCHER="${SCRIPT_DIR}/run-validator.sh"

    if [ ! -x "$LAUNCHER" ]; then
        echo -e "${RED}Launcher not found: $LAUNCHER${NC}"
        exit 1
    fi

    # ── Generate ZK keys (if not cached) ──
    # The Groth16 trusted setup is memory-intensive, so it runs as a
    # standalone binary before validators start.  Keys are cached at
    # ~/.moltchain/zk/ and persist across resets.
    ZK_SETUP_BIN="${REPO_ROOT}/target/release/zk-setup"
    if [ -x "$ZK_SETUP_BIN" ]; then
        echo "   Ensuring ZK proving/verification keys exist..."
        "$ZK_SETUP_BIN" 2>&1 | sed 's/^/   /'
    else
        echo -e "   ${YELLOW}⚠  zk-setup binary not found — build with: cargo build --release --bin zk-setup${NC}"
        echo -e "   ${YELLOW}   Shielded transactions will be unavailable until keys are generated.${NC}"
    fi
    echo ""

    echo "   Starting V1 (primary - creates genesis)..."
    nohup "$LAUNCHER" "$NETWORK" 1 $EXTRA_FLAGS > /tmp/moltchain-v1.log 2>&1 &
    V1_PID=$!
    echo "   V1 PID: $V1_PID"

    echo "   Waiting for V1 genesis (25s)..."
    sleep 25

    # Auto-copy genesis keypair to keypairs/deployer.json for E2E tests.
    # Retry up to 3 times in case genesis initialization is still in progress.
    GENESIS_KEY=""
    for _attempt in 1 2 3; do
        GENESIS_KEY=$(find "$REPO_ROOT/data/state-8000/genesis-keys" -name "genesis-primary-*.json" -type f 2>/dev/null | head -1)
        [ -n "$GENESIS_KEY" ] && break
        echo "   ⏳ Genesis keys not ready yet, waiting 5s more..."
        sleep 5
    done
    if [ -n "$GENESIS_KEY" ]; then
        mkdir -p "$REPO_ROOT/keypairs"
        cp "$GENESIS_KEY" "$REPO_ROOT/keypairs/deployer.json"
        DEPLOYER_PUBKEY=$(python3 -c "import json; d=json.load(open('$GENESIS_KEY')); print(d.get('pubkey','?'))" 2>/dev/null || echo '?')
        echo "   ✓ Copied genesis keypair to keypairs/deployer.json (pubkey=$DEPLOYER_PUBKEY)"
    else
        echo -e "   ${RED}⚠  Could not find genesis-primary-*.json after 40s — deployer.json NOT updated${NC}"
        echo -e "   ${YELLOW}   E2E tests may fail. Check: data/state-8000/genesis-keys/${NC}"
    fi

    echo "   Starting V2 (secondary)..."
    nohup "$LAUNCHER" "$NETWORK" 2 $EXTRA_FLAGS > /tmp/moltchain-v2.log 2>&1 &
    echo "   V2 PID: $!"

    echo "   Waiting for V2 sync (20s)..."
    sleep 20

    echo "   Starting V3 (tertiary)..."
    nohup "$LAUNCHER" "$NETWORK" 3 $EXTRA_FLAGS > /tmp/moltchain-v3.log 2>&1 &
    echo "   V3 PID: $!"

    echo ""
    echo "   Waiting for final sync (10s)..."
    sleep 10

    # Auto-fund deployer from treasury for E2E tests
    FUND_SCRIPT="${REPO_ROOT}/scripts/fund-deployer.py"
    if [ -f "$FUND_SCRIPT" ] && command -v python3 &>/dev/null; then
        echo "   Funding deployer from treasury..."
        cd "$REPO_ROOT"
        if [ -d ".venv" ]; then
            source .venv/bin/activate 2>/dev/null
        fi
        python3 "$FUND_SCRIPT" 2>&1 | sed 's/^/   /'
    fi

    # ── Start faucet service ──
    # The faucet sends signed transfer transactions (works in multi-validator mode).
    # Its keypair was auto-generated and funded at genesis.
    FAUCET_BIN="${REPO_ROOT}/target/release/moltchain-faucet"
    if [ -x "$FAUCET_BIN" ]; then
        # Copy faucet keypair from genesis-keys to repo root
        FAUCET_KEY=$(find "$REPO_ROOT/data/state-8000/genesis-keys" -name "faucet-*.json" -type f 2>/dev/null | head -1)
        if [ -n "$FAUCET_KEY" ]; then
            cp "$FAUCET_KEY" "$REPO_ROOT/faucet-keypair.json"
            echo "   ✓ Copied faucet keypair to faucet-keypair.json"
        fi
        echo "   Starting faucet service on port 9100..."
        cd "$REPO_ROOT"
        FAUCET_KEYPAIR="$REPO_ROOT/faucet-keypair.json" \
        RPC_URL="http://127.0.0.1:8899" \
        NETWORK="$NETWORK" \
        nohup "$FAUCET_BIN" > /tmp/moltchain-faucet.log 2>&1 &
        FAUCET_PID=$!
        echo "   ✓ Faucet PID: $FAUCET_PID"
    else
        echo "   ⚠  Faucet binary not found — build with: cargo build --release --bin moltchain-faucet"
    fi

    echo ""
    echo -e "${GREEN}Stack restarted (dev mode — fingerprint = SHA-256(pubkey)). Check logs:${NC}"
    echo "   tail -f /tmp/moltchain-v1.log"
    echo "   tail -f /tmp/moltchain-v2.log"
    echo "   tail -f /tmp/moltchain-v3.log"
    echo "   tail -f /tmp/moltchain-faucet.log"
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
    echo "   # Option C: Dev mode (multi-validator on one machine, bypasses fingerprint)"
    echo "   ./target/release/moltchain-validator --dev-mode --p2p-port 8000 --rpc-port 8899 --db-path \$PWD/data/state-8000"
    echo "   ./target/release/moltchain-validator --dev-mode --p2p-port 8001 --rpc-port 8901 --db-path \$PWD/data/state-8001 --bootstrap 127.0.0.1:8000"
    echo "   ./target/release/moltchain-validator --dev-mode --p2p-port 8002 --rpc-port 8903 --db-path \$PWD/data/state-8002 --bootstrap 127.0.0.1:8000"
    echo ""
    echo "   # Machine migration (import keypair from another machine)"
    echo "   ./target/release/moltchain-validator --import-key /path/to/keypair.json --p2p-port 8000 --rpc-port 8899"
    echo ""
    echo "   First validator creates genesis - start it first!"
fi
