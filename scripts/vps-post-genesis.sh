#!/bin/bash
# ============================================================================
# MoltChain VPS Post-Genesis Keypair Setup
# ============================================================================
#
# Run ONCE after the validator creates genesis on a VPS.
# Copies genesis-generated keypairs to the paths expected by custody/faucet.
#
# Genesis creates all contracts, initializes them, and funds the deployer —
# no deploy_dex.py or first-boot-deploy.sh needed.
#
# What this script does:
#   1. Copies genesis primary keypair → /etc/moltchain/custody-treasury-<network>.json
#      (so custody signs mint() calls with the matching contract admin key)
#   2. Copies faucet keypair → /var/lib/moltchain/faucet-keypair-<network>.json
#   3. Optionally restarts custody + faucet systemd services
#
# Usage:
#   bash scripts/vps-post-genesis.sh              # auto-detect network
#   bash scripts/vps-post-genesis.sh testnet       # explicit network
#   bash scripts/vps-post-genesis.sh --no-restart   # copy only, don't restart
#
# ============================================================================

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'

VPS_STATE="/var/lib/moltchain"
VPS_CONFIG="/etc/moltchain"
NETWORK=""
DO_RESTART=true

for arg in "$@"; do
	case "$arg" in
		testnet|mainnet) NETWORK="$arg" ;;
		--no-restart) DO_RESTART=false ;;
	esac
done

# Auto-detect network from running validators
if [ -z "$NETWORK" ]; then
	if sudo test -d "$VPS_STATE/state-testnet/genesis-keys" 2>/dev/null; then
		NETWORK="testnet"
	elif sudo test -d "$VPS_STATE/state-mainnet/genesis-keys" 2>/dev/null; then
		NETWORK="mainnet"
	else
		echo -e "${RED}No genesis-keys directory found. Is the validator running?${NC}"
		echo "  Expected: $VPS_STATE/state-{testnet,mainnet}/genesis-keys/"
		echo "  Start the validator first and wait for genesis creation (30s)."
		exit 1
	fi
fi

GENESIS_KEYS_DIR="$VPS_STATE/state-$NETWORK/genesis-keys"
CUSTODY_KEY_TARGET="$VPS_CONFIG/custody-treasury-$NETWORK.json"
FAUCET_KEY_TARGET="$VPS_STATE/faucet-keypair-$NETWORK.json"
if ! sudo test -d "$GENESIS_KEYS_DIR"; then
	echo -e "${RED}Genesis keys not yet created: $GENESIS_KEYS_DIR${NC}"
	echo "  The validator must complete genesis first. Wait 30s after starting it."
	exit 1
fi

echo -e "${CYAN}══════════════════════════════════════════════════════${NC}"
echo -e "${CYAN}  MoltChain VPS Post-Genesis Setup ($NETWORK)${NC}"
echo -e "${CYAN}══════════════════════════════════════════════════════${NC}"
echo ""

# ── 1. Genesis primary keypair → custody treasury ──
GENESIS_KEY=$(sudo find "$GENESIS_KEYS_DIR" -name "genesis-primary-*.json" -type f 2>/dev/null | head -1)
if [ -n "$GENESIS_KEY" ]; then
	sudo cp "$GENESIS_KEY" "$CUSTODY_KEY_TARGET"
	sudo chmod 600 "$CUSTODY_KEY_TARGET"
	sudo chown moltchain:moltchain "$CUSTODY_KEY_TARGET"

	PUBKEY=$(sudo python3 -c "import json; d=json.load(open('$GENESIS_KEY')); print(d.get('pubkey','?'))" 2>/dev/null || echo '?')
	echo -e "  ${GREEN}✓${NC} Custody treasury = genesis admin: $PUBKEY"
	echo -e "    $GENESIS_KEY → $CUSTODY_KEY_TARGET"
else
	echo -e "  ${RED}✗${NC} Genesis primary keypair not found in $GENESIS_KEYS_DIR"
fi

# ── 2. Faucet keypair ──
FAUCET_KEY=$(sudo find "$GENESIS_KEYS_DIR" -name "faucet-*.json" -type f 2>/dev/null | head -1)
if [ -n "$FAUCET_KEY" ]; then
	sudo cp "$FAUCET_KEY" "$FAUCET_KEY_TARGET"
	sudo chmod 600 "$FAUCET_KEY_TARGET"
	sudo chown moltchain:moltchain "$FAUCET_KEY_TARGET"

	FAUCET_PK=$(sudo python3 -c "import json; d=json.load(open('$FAUCET_KEY')); print(d.get('pubkey','?'))" 2>/dev/null || echo '?')
	echo -e "  ${GREEN}✓${NC} Faucet keypair: $FAUCET_PK"
	echo -e "    $FAUCET_KEY → $FAUCET_KEY_TARGET"
else
	echo -e "  ${YELLOW}⚠${NC} Faucet keypair not found (faucet may not work)"
fi

# ── 3. Copy to repo keypairs/ for convenience ──
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/.."
if [ -d "$REPO_ROOT" ] && [ -n "$GENESIS_KEY" ]; then
	mkdir -p "$REPO_ROOT/keypairs" 2>/dev/null || true
	sudo cp "$GENESIS_KEY" "$REPO_ROOT/keypairs/deployer.json" 2>/dev/null && \
		echo -e "  ${GREEN}✓${NC} Copied to keypairs/deployer.json" || true
fi

# ── 4. Ensure custody-db exists ──
if [ ! -d "$VPS_STATE/custody-db" ]; then
	sudo mkdir -p "$VPS_STATE/custody-db"
	sudo chown moltchain:moltchain "$VPS_STATE/custody-db"
	echo -e "  ${GREEN}✓${NC} Created $VPS_STATE/custody-db"
fi

# ── 5. Restart services ──
if [ "$DO_RESTART" = true ]; then
	echo ""
	echo -e "  Restarting services..."
	sudo systemctl restart moltchain-custody 2>/dev/null && echo -e "  ${GREEN}✓${NC} custody restarted" || echo -e "  ${YELLOW}⚠${NC} custody restart failed"
	sudo systemctl restart moltchain-faucet 2>/dev/null && echo -e "  ${GREEN}✓${NC} faucet restarted" || echo -e "  ${YELLOW}⚠${NC} faucet restart failed"
fi

echo ""
echo -e "${GREEN}══════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  Post-genesis setup complete.${NC}"
echo -e "${GREEN}  All contracts deployed + initialized at genesis.${NC}"
echo -e "${GREEN}  No deploy_dex.py or first-boot-deploy.sh needed.${NC}"
echo -e "${GREEN}══════════════════════════════════════════════════════${NC}"
