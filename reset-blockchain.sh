#!/bin/bash
# ============================================================================
# Lichen - Full Blockchain Reset
# ============================================================================

set -euo pipefail

# Restore a sane tool PATH when the caller shell exported a stripped environment.
BOOTSTRAP_PATH="/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
if [ -n "${HOME:-}" ] && [ -d "${HOME}/.cargo/bin" ]; then
	BOOTSTRAP_PATH="${HOME}/.cargo/bin:${BOOTSTRAP_PATH}"
fi
if [ -n "${HOME:-}" ] && [ -d "${HOME}/.local/bin" ]; then
	BOOTSTRAP_PATH="${HOME}/.local/bin:${BOOTSTRAP_PATH}"
fi
PATH="${BOOTSTRAP_PATH}:${PATH:-}"
export PATH

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$SCRIPT_DIR"

NETWORK="all"
RESTART=false
KEEP_KEYS=false
VPS_MODE=false
EXTRA_FLAGS=""

for arg in "$@"; do
	case "$arg" in
		--restart)  RESTART=true ;;
		--no-keys)  KEEP_KEYS=true ;;
		--dev-mode) EXTRA_FLAGS="$EXTRA_FLAGS --dev-mode" ;;
		--vps)      VPS_MODE=true ;;
		testnet|mainnet|all) NETWORK="$arg" ;;
	esac
done

# Auto-detect VPS mode: if systemd services exist, we're on a VPS
if [ "$VPS_MODE" = false ] && systemctl list-unit-files 'lichen-validator-*' 2>/dev/null | grep -q lichen; then
	VPS_MODE=true
fi

if [ "$VPS_MODE" = true ]; then
	echo -e "${RED}CRITICAL: reset-blockchain.sh VPS mode is disabled for v0.4.6.${NC}"
	echo "This script's systemd restart and genesis recovery path is not aligned with the validated production runbook."
	echo "Use the private operator runbook for production resets instead."
	exit 1
fi

if [ "$RESTART" = true ] && [ "$NETWORK" = "all" ]; then
	NETWORK="testnet"
fi

if [ "$RESTART" = true ] && [[ "$EXTRA_FLAGS" != *"--dev-mode"* ]]; then
	EXTRA_FLAGS="$EXTRA_FLAGS --dev-mode"
fi

# VPS state directory (systemd services use /var/lib/lichen)
VPS_STATE_DIR="/var/lib/lichen"
VPS_CONFIG_DIR="/etc/lichen"

echo -e "${RED}=================================================${NC}"
echo -e "${RED}  Lichen FULL RESET - All State Will Be Destroyed${NC}"
echo -e "${RED}=================================================${NC}"
echo ""
echo "  Repo root: $REPO_ROOT"
echo "  Network:   $NETWORK"
echo ""

echo -e "${YELLOW}[1/7] Stopping all Lichen processes...${NC}"

# On VPS, stop systemd services first (prevents auto-restart race)
if [ "$VPS_MODE" = true ]; then
	echo -e "  VPS mode: stopping systemd services..."
	for SVC in lichen-validator-testnet lichen-validator-mainnet \
	           lichen-faucet lichen-custody lichen-custody-mainnet; do
		if systemctl is-active --quiet "$SVC" 2>/dev/null; then
			sudo systemctl stop "$SVC" 2>/dev/null && echo "  stopped $SVC" || true
		fi
	done
	sleep 2
fi

# Kill any remaining processes
pkill -9 -f lichen-validator 2>/dev/null || true
pkill -9 -f lichen-faucet    2>/dev/null || true
pkill -9 -f lichen-custody   2>/dev/null || true
pkill -9 -f validator-supervisor.sh 2>/dev/null || true
pkill -9 -f run-validator.sh        2>/dev/null || true
sleep 1

if pgrep -f "lichen-" >/dev/null 2>&1; then
	echo -e "  ${YELLOW}Retrying kill...${NC}"
	pkill -9 -f "lichen-" 2>/dev/null || true
	pkill -9 -f validator-supervisor.sh 2>/dev/null || true
	pkill -9 -f run-validator.sh        2>/dev/null || true
	sleep 2
fi

LEFTOVER=$(pgrep -f "(lichen-(validator|faucet|custody)|validator-supervisor\.sh|run-validator\.sh)" 2>/dev/null || true)
if [ -n "$LEFTOVER" ]; then
	echo -e "  ${YELLOW}Warning: PIDs still present: $LEFTOVER${NC}"
else
	echo -e "${GREEN}  All processes stopped${NC}"
fi

echo -e "${YELLOW}[2/7] Removing blockchain state directories...${NC}"
cd "$REPO_ROOT"

if [ "$NETWORK" = "all" ]; then
	rm -rf data/state-* 2>/dev/null && echo "  removed data/state-*" || true
	rm -rf data/matrix-sdk-state-* 2>/dev/null && echo "  removed data/matrix-sdk-state-*" || true
	WORKSPACE_ROOT="$(dirname "$(dirname "$REPO_ROOT")")" 2>/dev/null || true
	if [ -d "$WORKSPACE_ROOT/data" ]; then
		rm -rf "$WORKSPACE_ROOT/data/state-"* 2>/dev/null && echo "  removed workspace root orphans" || true
		rm -rf "$WORKSPACE_ROOT/data/matrix-sdk-state-"* 2>/dev/null && echo "  removed workspace root legacy matrix dirs" || true
	fi
	rm -rf ~/.lichen/data-* ~/.lichen/state-* 2>/dev/null || true
	rm -rf data/custody* 2>/dev/null || true
	rm -rf /tmp/lichen-custody* 2>/dev/null || true

	# VPS state directories (systemd uses /var/lib/lichen)
	if [ "$VPS_MODE" = true ]; then
		sudo rm -rf "$VPS_STATE_DIR"/state-* 2>/dev/null && echo "  removed $VPS_STATE_DIR/state-*" || true
		sudo rm -rf "$VPS_STATE_DIR"/custody-db 2>/dev/null && echo "  removed $VPS_STATE_DIR/custody-db" || true
		sudo rm -rf "$VPS_STATE_DIR"/custody-db-mainnet 2>/dev/null && echo "  removed $VPS_STATE_DIR/custody-db-mainnet" || true
		sudo rm -rf "$VPS_STATE_DIR"/genesis-wallet.json 2>/dev/null || true
		sudo rm -rf "$VPS_STATE_DIR"/genesis-keys 2>/dev/null || true
		sudo rm -f "$VPS_STATE_DIR"/known-peers.json 2>/dev/null || true
		sudo rm -f "$VPS_STATE_DIR"/deploy-manifest.json 2>/dev/null || true
		sudo rm -f "$VPS_STATE_DIR"/faucet-keypair.json 2>/dev/null || true
		# Recreate custody-db directories with correct ownership
		sudo mkdir -p "$VPS_STATE_DIR/custody-db" "$VPS_STATE_DIR/custody-db-mainnet"
		sudo chown -R lichen:lichen "$VPS_STATE_DIR/custody-db" "$VPS_STATE_DIR/custody-db-mainnet" 2>/dev/null || true
		echo "  recreated $VPS_STATE_DIR/custody-db and custody-db-mainnet"
	fi
else
	rm -rf data/state-${NETWORK}-* 2>/dev/null && echo "  removed data/state-${NETWORK}-*" || true
	rm -rf data/matrix-sdk-state-* 2>/dev/null && echo "  removed data/matrix-sdk-state-*" || true
	if [ "$NETWORK" = "testnet" ]; then
		for port in 7001 7002 7003 8000 8001 8002 8003 8004; do rm -rf "data/state-${port}" 2>/dev/null || true; done
		rm -rf "data/state-testnet" 2>/dev/null || true
		echo "  removed data/state-testnet and legacy testnet port dirs"
	elif [ "$NETWORK" = "mainnet" ]; then
		for port in 8001 8002 8003 9000 9001 9002 9003 9004; do rm -rf "data/state-${port}" 2>/dev/null || true; done
		rm -rf "data/state-mainnet" 2>/dev/null || true
		echo "  removed data/state-mainnet and legacy mainnet port dirs"
	fi
	rm -rf data/custody-${NETWORK}* 2>/dev/null || true
fi
echo -e "${GREEN}  State directories flushed${NC}"

if [ "$KEEP_KEYS" = true ]; then
	echo -e "${CYAN}[3/7] Keeping validator keypairs (--no-keys)${NC}"
else
	echo -e "${YELLOW}[3/7] Removing validator keypairs...${NC}"
	if [ "$NETWORK" = "all" ]; then
		rm -rf ~/.lichen/validators 2>/dev/null || true
		rm -f ~/.lichen/validator-*.json 2>/dev/null || true
		find "$REPO_ROOT/data" -maxdepth 3 -name "validator-keypair.json" -delete 2>/dev/null || true
	else
		if [ "$NETWORK" = "testnet" ]; then
			for port in 7001 7002 7003 8000 8001 8002; do rm -f "$HOME/.lichen/validators/validator-${port}.json" 2>/dev/null || true; done
			rm -f "$HOME/.lichen/validators/validator-testnet.json" 2>/dev/null || true
		else
			for port in 8001 8002 8003 9000 9001 9002; do rm -f "$HOME/.lichen/validators/validator-${port}.json" 2>/dev/null || true; done
			rm -f "$HOME/.lichen/validators/validator-mainnet.json" 2>/dev/null || true
		fi
	fi
	echo -e "${GREEN}  Validator keypairs cleared (regenerate on start)${NC}"
fi

echo -e "${YELLOW}[4/7] Cleaning signer data, peer stores, genesis files...${NC}"
if [ "$NETWORK" = "all" ]; then
	rm -rf ~/.lichen/signer-* ~/.lichen/signers 2>/dev/null || true
	rm -rf "$REPO_ROOT"/data/signer-* 2>/dev/null || true
	find "$REPO_ROOT/data" -maxdepth 3 -name "known-peers.json" -delete 2>/dev/null || true
	rm -f ~/.lichen/known-peers.json 2>/dev/null || true
	rm -f "$REPO_ROOT/genesis.json" 2>/dev/null || true
	find "$REPO_ROOT/data" -maxdepth 3 -name "genesis-wallet.json" -delete 2>/dev/null || true
	find "$REPO_ROOT/data" -maxdepth 3 -name "genesis-keys" -type d -exec rm -rf {} + 2>/dev/null || true
	rm -f ~/.lichen/genesis-wallet.json 2>/dev/null || true
	rm -rf ~/.lichen/genesis-keys 2>/dev/null || true
	rm -f "$REPO_ROOT/airdrops.json" 2>/dev/null || true
	rm -rf /tmp/lichen-* /tmp/validator-* /tmp/lichen-* 2>/dev/null || true
else
	for dir in "$REPO_ROOT"/data/state-${NETWORK}-*; do
		[ -d "$dir" ] || continue
		find "$dir" -name "known-peers.json" -delete 2>/dev/null || true
		find "$dir" -name "genesis-wallet.json" -delete 2>/dev/null || true
		find "$dir" -name "genesis-keys" -type d -exec rm -rf {} + 2>/dev/null || true
		find "$dir" -name "signer-keypair.json" -delete 2>/dev/null || true
	done
	rm -f "$REPO_ROOT/airdrops.json" 2>/dev/null || true
fi
echo -e "${GREEN}  All transient state cleaned${NC}"

echo -e "${YELLOW}[5/7] Cleaning machine fingerprint and migration artifacts...${NC}"
if [ "$NETWORK" = "all" ]; then
	rm -f /tmp/v*-migrated-keypair.json /tmp/*-keypair-backup-*.json 2>/dev/null || true
	find "$REPO_ROOT/data" -maxdepth 3 -name "migration-*.json" -delete 2>/dev/null || true
	find "$REPO_ROOT/data" -maxdepth 3 -name "fingerprint-*.dat" -delete 2>/dev/null || true
	rm -f /tmp/lichen-validator-*.pid /tmp/signer-*.json 2>/dev/null || true
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
	rm -f "$REPO_ROOT"/signed-metadata-manifest-*.json 2>/dev/null && echo "  removed signed-metadata-manifest-*.json" || true
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
	if [ "$VPS_MODE" = true ]; then
		for dir in "$VPS_STATE_DIR"/state-*; do
			if [ -d "$dir" ]; then
				echo -e "  ${RED}STILL EXISTS: $dir${NC}" && DIRTY=1
			fi
		done
	fi
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
	if [ "$VPS_MODE" = true ]; then
		# ══════════════════════════════════════════════════════════════
		# VPS RESTART (systemd services)
		# ══════════════════════════════════════════════════════════════
		echo -e "${YELLOW}[7/7] VPS restart: starting validators via systemd...${NC}"
		echo ""

		# Start testnet validator
		if [ "$NETWORK" = "all" ] || [ "$NETWORK" = "testnet" ]; then
			echo "   Starting testnet validator..."
			sudo systemctl start lichen-validator-testnet
			echo "   ✓ lichen-validator-testnet started"
		fi

		# Start mainnet validator
		if [ "$NETWORK" = "all" ] || [ "$NETWORK" = "mainnet" ]; then
			echo "   Starting mainnet validator..."
			sudo systemctl start lichen-validator-mainnet
			echo "   ✓ lichen-validator-mainnet started"
		fi

		echo "   Waiting for genesis creation (30s)..."
		sleep 30

		# ── Post-genesis: copy keypairs to service paths ──
		VPS_GENESIS_SETUP="${SCRIPT_DIR}/private-ops/vps-post-genesis.sh"
		if [ -f "$VPS_GENESIS_SETUP" ]; then
			echo "   Running post-genesis keypair setup..."
			bash "$VPS_GENESIS_SETUP" 2>&1 | sed 's/^/   /'
		else
			# Inline post-genesis setup
			echo "   Copying genesis keypairs to service paths..."

			# Testnet keypairs
			if [ "$NETWORK" = "all" ] || [ "$NETWORK" = "testnet" ]; then
				GENESIS_KEY=$(sudo find "$VPS_STATE_DIR/state-testnet/genesis-keys" -name "genesis-primary-*.json" -type f 2>/dev/null | head -1)
				if [ -n "$GENESIS_KEY" ]; then
					# Copy to custody treasury
					sudo cp "$GENESIS_KEY" "$VPS_CONFIG_DIR/custody-treasury.json"
					sudo chmod 600 "$VPS_CONFIG_DIR/custody-treasury.json"
					sudo chown lichen:lichen "$VPS_CONFIG_DIR/custody-treasury.json"
					DEPLOYER_PUBKEY=$(sudo python3 -c "import json; d=json.load(open('$GENESIS_KEY')); print(d.get('pubkey','?'))" 2>/dev/null || echo '?')
					echo "   ✓ Custody treasury keypair = genesis admin ($DEPLOYER_PUBKEY)"

					# Copy to deployer.json
					mkdir -p "$REPO_ROOT/keypairs" 2>/dev/null || true
					sudo cp "$GENESIS_KEY" "$REPO_ROOT/keypairs/deployer.json" 2>/dev/null || true
					echo "   ✓ Copied genesis keypair to keypairs/deployer.json"
				else
					echo -e "   ${RED}⚠  Genesis primary keypair not found — custody will not work${NC}"
				fi

				# Copy faucet keypair
				FAUCET_KEY=$(sudo find "$VPS_STATE_DIR/state-testnet/genesis-keys" -name "faucet-*.json" -type f 2>/dev/null | head -1)
				if [ -n "$FAUCET_KEY" ]; then
					sudo cp "$FAUCET_KEY" "$VPS_STATE_DIR/faucet-keypair.json"
					sudo chown lichen:lichen "$VPS_STATE_DIR/faucet-keypair.json"
					echo "   ✓ Copied faucet keypair"
				fi
			fi
		fi

		# Start supporting services
		if [ "$NETWORK" = "all" ] || [ "$NETWORK" = "testnet" ]; then
			echo "   Starting faucet and custody services..."
			sudo systemctl start lichen-faucet 2>/dev/null && echo "   ✓ faucet started" || echo "   ⚠  faucet start failed"
			sudo systemctl start lichen-custody 2>/dev/null && echo "   ✓ custody started" || echo "   ⚠  custody start failed"
		fi

		if [ "$NETWORK" = "all" ] || [ "$NETWORK" = "mainnet" ]; then
			echo "   Starting mainnet custody service..."
			sudo systemctl start lichen-custody-mainnet 2>/dev/null && echo "   ✓ custody-mainnet started" || echo "   ⚠  custody-mainnet start failed"
		fi

		echo ""
		echo -e "${GREEN}=================================================${NC}"
		echo -e "${GREEN}  VPS restart complete. All services started.${NC}"
		echo -e "${GREEN}=================================================${NC}"

		# Health check
		sleep 5
		echo ""
		echo "   Health check:"
		for SVC in lichen-validator-testnet lichen-validator-mainnet lichen-faucet lichen-custody lichen-custody-mainnet; do
			STATUS=$(systemctl is-active "$SVC" 2>/dev/null || echo "not-found")
			if [ "$STATUS" = "active" ]; then
				echo -e "   ${GREEN}✓${NC} $SVC: $STATUS"
			else
				echo -e "   ${RED}✗${NC} $SVC: $STATUS"
			fi
		done
	else
		# ══════════════════════════════════════════════════════════════
		# LOCAL RESTART (dev mode with run-validator.sh)
		# ══════════════════════════════════════════════════════════════
		echo -e "${YELLOW}Restarting ${NETWORK} local stack (dev mode)...${NC}"
		echo ""

		if [ "$NETWORK" = "testnet" ]; then
			PRIMARY_P2P=7001
			PRIMARY_RPC=8899
			FAUCET_PORT=9100
		elif [ "$NETWORK" = "mainnet" ]; then
			PRIMARY_P2P=8001
			PRIMARY_RPC=9899
			FAUCET_PORT=9101
		else
			echo -e "${RED}Restart requires explicit network (testnet or mainnet).${NC}"
			exit 1
		fi

		LAUNCHER="${SCRIPT_DIR}/run-validator.sh"
		if [ ! -x "$LAUNCHER" ]; then
			echo -e "${RED}Launcher not found: $LAUNCHER${NC}"
			exit 1
		fi

		export LICHEN_LOCAL_DEV=1

		echo "   Starting V1 (primary - creates genesis)..."
		nohup "$LAUNCHER" "$NETWORK" 1 $EXTRA_FLAGS > /tmp/lichen-v1.log 2>&1 &
		V1_PID=$!
		echo "   V1 PID: $V1_PID"

		echo "   Waiting for V1 genesis (25s)..."
		sleep 25

		GENESIS_KEY=""
		for _attempt in 1 2 3; do
			GENESIS_KEY=$(find "$REPO_ROOT/data/state-${PRIMARY_P2P}/genesis-keys" -name "genesis-primary-*.json" -type f 2>/dev/null | head -1)
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
		nohup "$LAUNCHER" "$NETWORK" 2 $EXTRA_FLAGS > /tmp/lichen-v2.log 2>&1 &
		echo "   V2 PID: $!"
		echo "   Waiting for V2 sync (20s)..."
		sleep 20

		echo "   Starting V3 (tertiary)..."
		nohup "$LAUNCHER" "$NETWORK" 3 $EXTRA_FLAGS > /tmp/lichen-v3.log 2>&1 &
		echo "   V3 PID: $!"

		echo ""
		echo "   Waiting for final sync (10s)..."
		sleep 10

		FAUCET_BIN="${REPO_ROOT}/target/release/lichen-faucet"
		if [ -x "$FAUCET_BIN" ]; then
			FAUCET_KEY=$(find "$REPO_ROOT/data/state-${PRIMARY_P2P}/genesis-keys" -name "faucet-*.json" -type f 2>/dev/null | head -1)
			if [ -n "$FAUCET_KEY" ]; then
				cp "$FAUCET_KEY" "$REPO_ROOT/faucet-keypair.json"
				echo "   ✓ Copied faucet keypair to faucet-keypair.json"
			fi
			echo "   Starting faucet service on port ${FAUCET_PORT}..."
			cd "$REPO_ROOT"
			FAUCET_KEYPAIR="$REPO_ROOT/faucet-keypair.json" \
			RPC_URL="http://127.0.0.1:${PRIMARY_RPC}" \
			FAUCET_PORT="${FAUCET_PORT}" \
			NETWORK="$NETWORK" \
			nohup "$FAUCET_BIN" > /tmp/lichen-faucet.log 2>&1 &
			FAUCET_PID=$!
			echo "   ✓ Faucet PID: $FAUCET_PID"
		else
			echo "   ⚠  Faucet binary not found — build with: cargo build --release --bin lichen-faucet"
		fi
	fi
else
	if [ "$VPS_MODE" = true ]; then
		echo "Next steps (VPS):"
		echo "   sudo systemctl start lichen-validator-testnet"
		echo "   sudo systemctl start lichen-validator-mainnet"
		echo "   # Wait for genesis, then follow the private operator runbook."
		echo "   sudo systemctl start lichen-faucet"
		echo "   sudo systemctl start lichen-custody"
		echo "   sudo systemctl start lichen-custody-mainnet"
	else
		echo "Next steps:"
		echo "   cd $REPO_ROOT"
		echo ""
		echo "   # Option A: Dev scripts (auto-ports)"
		echo "   ./run-validator.sh testnet 1   # V1 genesis"
		echo "   ./run-validator.sh testnet 2   # V2 sync"
		echo "   ./run-validator.sh testnet 3   # V3 sync"
	fi
fi
