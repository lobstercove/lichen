#!/bin/bash
# ============================================================================
# Lichen VPS Post-Genesis Keypair Setup
# ============================================================================
#
# Run ONCE after the validator creates genesis on a VPS.
# Copies genesis-generated keypairs to the paths expected by custody/faucet.
#
# Genesis creates the contract catalog. This script only copies the resulting
# key material into the system paths expected by custody and faucet.
# Run scripts/first-boot-deploy.sh separately if you need deploy-manifest.json
# and signed metadata rebuilt from the live symbol registry.
#
# What this script does:
#   1. Copies genesis primary keypair → /etc/lichen/custody-treasury-<network>.json
#      (so custody signs wrapped-token mint() calls with the operational minter key;
#       wrapped-token admin ownership moves to governance during genesis)
#   2. Copies faucet keypair → /var/lib/lichen/faucet-keypair-<network>.json
#      (falls back to the genesis treasury keypair when no dedicated faucet
#       keypair was emitted by genesis)
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

VPS_STATE="/var/lib/lichen"
VPS_CONFIG="/etc/lichen"
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
echo -e "${CYAN}  Lichen VPS Post-Genesis Setup ($NETWORK)${NC}"
echo -e "${CYAN}══════════════════════════════════════════════════════${NC}"
echo ""

# ── 1. Genesis primary keypair → custody treasury ──
read_keypair_pubkey() {
	local key_file="$1"

	sudo python3 - "$key_file" <<'PY' 2>/dev/null || echo '?'
import json
import sys

data = json.load(open(sys.argv[1], encoding='utf-8'))
for key in ('publicKeyBase58', 'pubkey', 'address'):
    value = data.get(key)
    if isinstance(value, str) and value.strip():
        print(value.strip())
        raise SystemExit(0)
print('?')
PY
}

GENESIS_KEY=$(sudo find "$GENESIS_KEYS_DIR" -name "genesis-primary-*.json" -type f 2>/dev/null | head -1)
if [ -n "$GENESIS_KEY" ]; then
	sudo install -m 600 -o lichen -g lichen "$GENESIS_KEY" "$CUSTODY_KEY_TARGET"

	PUBKEY=$(read_keypair_pubkey "$GENESIS_KEY")
	echo -e "  ${GREEN}✓${NC} Custody wrapped-token minter = genesis primary: $PUBKEY"
	echo -e "    $GENESIS_KEY → $CUSTODY_KEY_TARGET"
else
	echo -e "  ${RED}✗${NC} Genesis primary keypair not found in $GENESIS_KEYS_DIR"
fi

# ── 2. Faucet keypair ──
FAUCET_KEY=$(sudo find "$GENESIS_KEYS_DIR" -name "faucet-*.json" -type f 2>/dev/null | head -1)
FAUCET_SOURCE_LABEL="faucet keypair"
if [ -z "$FAUCET_KEY" ]; then
	FAUCET_KEY=$(sudo find "$GENESIS_KEYS_DIR" -name "treasury-*.json" -type f 2>/dev/null | head -1)
	FAUCET_SOURCE_LABEL="treasury fallback"
fi
if [ -n "$FAUCET_KEY" ]; then
	sudo install -m 600 -o lichen -g lichen "$FAUCET_KEY" "$FAUCET_KEY_TARGET"

	FAUCET_PK=$(read_keypair_pubkey "$FAUCET_KEY")
	echo -e "  ${GREEN}✓${NC} Faucet keypair ($FAUCET_SOURCE_LABEL): $FAUCET_PK"
	echo -e "    $FAUCET_KEY → $FAUCET_KEY_TARGET"
else
	echo -e "  ${YELLOW}⚠${NC} Faucet keypair not found (faucet may not work)"
fi

# ── 3. Copy to repo keypairs/ for convenience ──
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/.."
if [ -d "$REPO_ROOT" ] && [ -n "$GENESIS_KEY" ]; then
	mkdir -p "$REPO_ROOT/keypairs" 2>/dev/null || true
	sudo install -m 600 "$GENESIS_KEY" "$REPO_ROOT/keypairs/deployer.json" 2>/dev/null && \
		echo -e "  ${GREEN}✓${NC} Copied to keypairs/deployer.json" || true
fi

# ── 4. Ensure custody-db exists ──
if [ ! -d "$VPS_STATE/custody-db" ]; then
	sudo mkdir -p "$VPS_STATE/custody-db"
	sudo chown lichen:lichen "$VPS_STATE/custody-db"
	echo -e "  ${GREEN}✓${NC} Created $VPS_STATE/custody-db"
fi

# ── 5. Restart services ──
if [ "$DO_RESTART" = true ]; then
	echo ""
	echo -e "  Restarting services..."
	sudo systemctl restart lichen-custody 2>/dev/null && echo -e "  ${GREEN}✓${NC} custody restarted" || echo -e "  ${YELLOW}⚠${NC} custody restart failed"
	sudo systemctl restart lichen-faucet 2>/dev/null && echo -e "  ${GREEN}✓${NC} faucet restarted" || echo -e "  ${YELLOW}⚠${NC} faucet restart failed"
fi

echo ""
echo -e "${GREEN}══════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  Post-genesis setup complete.${NC}"
echo -e "${GREEN}  Genesis already deployed the contract catalog.${NC}"
echo -e "${GREEN}  Run scripts/first-boot-deploy.sh next if you need deploy-manifest.json and signed metadata.${NC}"
echo -e "${GREEN}══════════════════════════════════════════════════════${NC}"
