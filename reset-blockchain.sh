#!/bin/bash
# ============================================================================
# MoltChain - Full Blockchain Reset
# ============================================================================

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$SCRIPT_DIR"

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

if [ "$RESTART" = true ] && [[ "$EXTRA_FLAGS" != *"--dev-mode"* ]]; then
	EXTRA_FLAGS="$EXTRA_FLAGS --dev-mode"
fi

echo -e "${RED}=================================================${NC}"
echo -e "${RED}  MoltChain FULL RESET - All State Will Be Destroyed${NC}"
echo -e "${RED}=================================================${NC}"
echo ""
echo "  Repo root: $REPO_ROOT"
echo "  Network:   $NETWORK"
echo ""

echo -e "${YELLOW}[1/7] Killing all MoltChain processes...${NC}"
pkill -9 -f moltchain-validator 2>/dev/null || true
pkill -9 -f moltchain-faucet    2>/dev/null || true
pkill -9 -f moltchain-custody   2>/dev/null || true
sleep 1

if pgrep -f "moltchain-" >/dev/null 2>&1; then
	echo -e "  ${YELLOW}Retrying kill...${NC}"
	pkill -9 -f "moltchain-" 2>/dev/null || true
	sleep 2
fi

LEFTOVER=$(pgrep -f "moltchain-(validator|faucet|custody)" 2>/dev/null || true)
if [ -n "$LEFTOVER" ]; then
	echo -e "  ${YELLOW}Warning: PIDs still present: $LEFTOVER${NC}"
else
	echo -e "${GREEN}  All processes stopped${NC}"
fi

echo -e "${YELLOW}[2/7] Removing blockchain state directories...${NC}"
cd "$REPO_ROOT"

if [ "$NETWORK" = "all" ]; then
	rm -rf data/state-* 2>/dev/null && echo "  removed data/state-*" || true
	WORKSPACE_ROOT="$(dirname "$(dirname "$REPO_ROOT")")" 2>/dev/null || true
	if [ -d "$WORKSPACE_ROOT/data" ]; then
		rm -rf "$WORKSPACE_ROOT/data/state-"* 2>/dev/null && echo "  removed workspace root orphans" || true
	fi
	rm -rf ~/.moltchain/data-* ~/.moltchain/state-* 2>/dev/null || true
	if [[ "${EXTRA_FLAGS:-}" == *"--zk-reset"* ]]; then
		rm -rf ~/.moltchain/zk 2>/dev/null && echo "  removed ZK key cache (--zk-reset)"
	fi
	rm -rf data/custody* 2>/dev/null || true
	rm -rf /tmp/moltchain-custody* 2>/dev/null || true
else
	rm -rf data/state-${NETWORK}-* 2>/dev/null && echo "  removed data/state-${NETWORK}-*" || true
	if [ "$NETWORK" = "testnet" ]; then
		for port in 8000 8001 8002 8003 8004; do rm -rf "data/state-${port}" 2>/dev/null || true; done
		echo "  removed data/state-800x (testnet ports)"
	elif [ "$NETWORK" = "mainnet" ]; then
		for port in 9000 9001 9002 9003 9004; do rm -rf "data/state-${port}" 2>/dev/null || true; done
		echo "  removed data/state-900x (mainnet ports)"
	fi
	rm -rf data/custody-${NETWORK}* 2>/dev/null || true
fi
echo -e "${GREEN}  State directories flushed${NC}"

if [ "$KEEP_KEYS" = true ]; then
	echo -e "${CYAN}[3/7] Keeping validator keypairs (--no-keys)${NC}"
else
	echo -e "${YELLOW}[3/7] Removing validator keypairs...${NC}"
	if [ "$NETWORK" = "all" ]; then
		rm -rf ~/.moltchain/validators 2>/dev/null || true
		rm -f ~/.moltchain/validator-*.json 2>/dev/null || true
		find "$REPO_ROOT/data" -maxdepth 3 -name "validator-keypair.json" -delete 2>/dev/null || true
	else
		if [ "$NETWORK" = "testnet" ]; then
			for port in 7001 7002 7003 8000 8001 8002; do rm -f "$HOME/.moltchain/validators/validator-${port}.json" 2>/dev/null || true; done
		else
			for port in 8001 8002 8003 9000 9001 9002; do rm -f "$HOME/.moltchain/validators/validator-${port}.json" 2>/dev/null || true; done
		fi
	fi
	echo -e "${GREEN}  Validator keypairs cleared (regenerate on start)${NC}"
fi

echo -e "${YELLOW}[4/7] Cleaning signer data, peer stores, genesis files...${NC}"
if [ "$NETWORK" = "all" ]; then
	rm -rf ~/.moltchain/signer-* ~/.moltchain/signers 2>/dev/null || true
	rm -rf "$REPO_ROOT"/data/signer-* 2>/dev/null || true
	find "$REPO_ROOT/data" -maxdepth 3 -name "known-peers.json" -delete 2>/dev/null || true
	rm -f ~/.moltchain/known-peers.json 2>/dev/null || true
	rm -f "$REPO_ROOT/genesis.json" 2>/dev/null || true
	find "$REPO_ROOT/data" -maxdepth 3 -name "genesis-wallet.json" -delete 2>/dev/null || true
	find "$REPO_ROOT/data" -maxdepth 3 -name "genesis-keys" -type d -exec rm -rf {} + 2>/dev/null || true
	rm -f ~/.moltchain/genesis-wallet.json 2>/dev/null || true
	rm -rf ~/.moltchain/genesis-keys 2>/dev/null || true
	rm -f "$REPO_ROOT/airdrops.json" 2>/dev/null || true
	rm -rf /tmp/moltchain-* /tmp/validator-* /tmp/molt* 2>/dev/null || true
else
	for dir in "$REPO_ROOT"/data/state-${NETWORK}-*; do
		[ -d "$dir" ] || continue
		find "$dir" -name "known-peers.json" -delete 2>/dev/null || true
		find "$dir" -name "genesis-wallet.json" -delete 2>/dev/null || true
		find "$dir" -name "genesis-keys" -type d -exec rm -rf {} + 2>/dev/null || true
		find "$dir" -name "signer-keypair.json" -delete 2>/dev/null || true
	done
fi
echo -e "${GREEN}  All transient state cleaned${NC}"

echo -e "${YELLOW}[5/7] Cleaning machine fingerprint and migration artifacts...${NC}"
if [ "$NETWORK" = "all" ]; then
	rm -f /tmp/v*-migrated-keypair.json /tmp/*-keypair-backup-*.json 2>/dev/null || true
	find "$REPO_ROOT/data" -maxdepth 3 -name "migration-*.json" -delete 2>/dev/null || true
	find "$REPO_ROOT/data" -maxdepth 3 -name "fingerprint-*.dat" -delete 2>/dev/null || true
	rm -f /tmp/moltchain-validator-*.pid /tmp/signer-*.json 2>/dev/null || true
else
	for dir in "$REPO_ROOT"/data/state-${NETWORK}-*; do
		[ -d "$dir" ] || continue
		find "$dir" -name "migration-*.json" -delete 2>/dev/null || true
		find "$dir" -name "fingerprint-*.dat" -delete 2>/dev/null || true
	done
fi
echo -e "${GREEN}  Fingerprint and migration artifacts cleaned${NC}"

echo -e "${YELLOW}[5b/7] Cleaning logs and deploy manifest...${NC}"
if [ "$NETWORK" = "all" ] || [ "$NETWORK" = "testnet" ]; then
	rm -f "$REPO_ROOT"/logs/v*.log 2>/dev/null && echo "  removed logs/v*.log" || true
	rm -f "$REPO_ROOT/deploy-manifest.json" 2>/dev/null && echo "  removed deploy-manifest.json" || true
	rm -f "$REPO_ROOT/faucet-keypair.json" 2>/dev/null || true
	rm -f "$REPO_ROOT"/artifacts/*.json 2>/dev/null && echo "  removed artifacts/*.json" || true
fi
echo -e "${GREEN}  Dev artifacts cleaned${NC}"

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

	FUND_SCRIPT="${REPO_ROOT}/scripts/fund-deployer.py"
	if [ -f "$FUND_SCRIPT" ] && command -v python3 &>/dev/null; then
		echo "   Funding deployer from treasury..."
		cd "$REPO_ROOT"
		if [ -d ".venv" ]; then source .venv/bin/activate 2>/dev/null; fi
		python3 "$FUND_SCRIPT" 2>&1 | sed 's/^/   /'
	fi

	FAUCET_BIN="${REPO_ROOT}/target/release/moltchain-faucet"
	if [ -x "$FAUCET_BIN" ]; then
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
else
	echo "Next steps:"
	echo "   cd $REPO_ROOT"
	echo ""
	echo "   # Option A: Dev scripts (auto-ports)"
	echo "   ./run-validator.sh testnet 1   # V1 genesis"
	echo "   ./run-validator.sh testnet 2   # V2 sync"
	echo "   ./run-validator.sh testnet 3   # V3 sync"
fi
