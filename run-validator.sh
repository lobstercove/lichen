#!/bin/bash
# ============================================================================
# MoltChain Validator Launcher
# ============================================================================
#
# Usage: ./run-validator.sh [network] <validator_number>
#   network: testnet | mainnet (default: testnet)
#
# Port Assignments (testnet):
#   V1: p2p=7001  rpc=8899  ws=8900
#   V2: p2p=7002  rpc=8901  ws=8902
#   V3: p2p=7003  rpc=8903  ws=8904
#
# Port Assignments (mainnet):
#   V1: p2p=8001  rpc=9899  ws=9900
#   V2: p2p=8002  rpc=9901  ws=9902
#   V3: p2p=8003  rpc=9903  ws=9904
#
# DB paths are always absolute: $REPO_ROOT/data/state-{p2p_port}
# ============================================================================

NETWORK=${1:-testnet}
VALIDATOR_NUM=${2:-1}
ORIG_ARGS=("$@")

if [[ "$NETWORK" =~ ^[0-9]+$ ]]; then
	VALIDATOR_NUM=$NETWORK
	NETWORK=testnet
fi

NETWORK=$(echo "$NETWORK" | tr '[:upper:]' '[:lower:]')
NETWORK_UPPER=$(echo "$NETWORK" | tr '[:lower:]' '[:upper:]')

case $NETWORK in
	testnet)
		BASE_P2P=7001
		BASE_RPC=8899
		BASE_WS=8900
		;;
	mainnet)
		BASE_P2P=8001
		BASE_RPC=9899
		BASE_WS=9900
		;;
	*)
		echo "Usage: $0 [testnet|mainnet] <1|2|3>"
		exit 1
		;;
esac

if ! [[ "$VALIDATOR_NUM" =~ ^[1-9][0-9]*$ ]]; then
	echo "Usage: $0 [testnet|mainnet] <validator_number>"
	exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$SCRIPT_DIR"
cd "$REPO_ROOT" || exit 1

SUPERVISOR_SCRIPT="$REPO_ROOT/scripts/validator-supervisor.sh"

P2P_PORT=$((BASE_P2P + (VALIDATOR_NUM - 1)))
RPC_PORT=$((BASE_RPC + 2 * (VALIDATOR_NUM - 1)))
WS_PORT=$((BASE_WS + 2 * (VALIDATOR_NUM - 1)))
SIGNER_PORT=$((9200 + VALIDATOR_NUM))

DB_PATH="${REPO_ROOT}/data/state-${P2P_PORT}"
VALIDATOR_HOME="${DB_PATH}/home"
mkdir -p "$VALIDATOR_HOME"

LOCAL_LISTEN_ADDR="${MOLTCHAIN_LOCAL_LISTEN_ADDR:-127.0.0.1}"
VALIDATOR_KEYPAIR_FILE="${DB_PATH}/validator-keypair.json"
GENESIS_WALLET_FILE="${DB_PATH}/genesis-wallet.json"
LOCAL_SEEDS_FILE="${DB_PATH}/seeds.json"
CLI_BIN="${REPO_ROOT}/target/release/molt"
GENESIS_BIN="${REPO_ROOT}/target/release/moltchain-genesis"
VALIDATOR_BIN="${REPO_ROOT}/target/release/moltchain-validator"

# Save real user home BEFORE overriding — needed for shared ZK verification keys
REAL_USER_HOME="${HOME}"

# Ensure each validator has isolated persistent identity/fingerprint stores.
# Without this, multiple local validators share ~/.moltchain/node_cert.der and
# can be rejected as banned/duplicate peers.
export HOME="$VALIDATOR_HOME"

# Point ZK verification keys to the shared cache in the REAL user home.
# The per-validator HOME override above prevents dirs::home_dir() from finding
# ~/.moltchain/zk/ — we fix that by setting explicit env vars.
if [[ -d "${REAL_USER_HOME}/.moltchain/zk" ]]; then
	export MOLTCHAIN_ZK_SHIELD_VK_PATH="${REAL_USER_HOME}/.moltchain/zk/vk_shield.bin"
	export MOLTCHAIN_ZK_UNSHIELD_VK_PATH="${REAL_USER_HOME}/.moltchain/zk/vk_unshield.bin"
	export MOLTCHAIN_ZK_TRANSFER_VK_PATH="${REAL_USER_HOME}/.moltchain/zk/vk_transfer.bin"
fi

if [[ "${MOLTCHAIN_SUPERVISED:-0}" != "1" && "${MOLTCHAIN_DISABLE_SUPERVISOR:-0}" != "1" && -x "$SUPERVISOR_SCRIPT" ]]; then
	SUPERVISOR_INSTANCE="${NETWORK}-v${VALIDATOR_NUM}-p${P2P_PORT}"
	exec "$SUPERVISOR_SCRIPT" "$SUPERVISOR_INSTANCE" -- env MOLTCHAIN_SUPERVISED=1 "$REPO_ROOT/run-validator.sh" "${ORIG_ARGS[@]}"
fi

write_local_seeds_file() {
	cat > "$LOCAL_SEEDS_FILE" <<EOF
{
  "$NETWORK": {
    "network_id": "moltchain-${NETWORK}-local",
    "chain_id": "moltchain-${NETWORK}-1",
    "seeds": [],
    "bootstrap_peers": [
      "127.0.0.1:${BASE_P2P}"
    ],
    "rpc_endpoints": [
      "http://127.0.0.1:${BASE_RPC}"
    ],
    "explorers": [],
    "faucets": []
  }
}
EOF
}

ensure_local_genesis() {
	if [[ "$VALIDATOR_NUM" != "1" ]]; then
		return
	fi

	if [[ -f "$DB_PATH/CURRENT" || -f "$GENESIS_WALLET_FILE" ]]; then
		return
	fi

	echo "Preparing local genesis state for $NAME"

	# Fetch real-time prices from Binance for genesis pool pricing
	PRICE_JSON=$(curl -gsf --max-time 10 \
		'https://api.binance.us/api/v3/ticker/price?symbols=["SOLUSDT","ETHUSDT","BNBUSDT"]' 2>/dev/null \
		|| curl -gsf --max-time 10 \
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
		echo "  Genesis prices: SOL=\$${GENESIS_SOL_USD:-?} ETH=\$${GENESIS_ETH_USD:-?} BNB=\$${GENESIS_BNB_USD:-?} MOLT=\$${GENESIS_MOLT_USD}"
	else
		echo "  Could not fetch live prices, using defaults"
	fi

	if [[ ! -x "$CLI_BIN" || ! -x "$GENESIS_BIN" || ! -x "$VALIDATOR_BIN" ]]; then
		echo "Building required release binaries..."
		cargo build --release --bin molt --bin moltchain-genesis --bin moltchain-validator || exit 1
	fi

	if [[ ! -f "$VALIDATOR_KEYPAIR_FILE" ]]; then
		"$CLI_BIN" init --output "$VALIDATOR_KEYPAIR_FILE" >/dev/null || exit 1
	fi

	if [[ ! -f "$GENESIS_WALLET_FILE" ]]; then
		"$GENESIS_BIN" --prepare-wallet --network "$NETWORK" --output-dir "$DB_PATH" || exit 1
	fi

	VALIDATOR_PUBKEY="$(sed -n 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$VALIDATOR_KEYPAIR_FILE" | head -n 1)"
	if [[ -z "$VALIDATOR_PUBKEY" ]]; then
		echo "Failed to derive validator pubkey from $VALIDATOR_KEYPAIR_FILE"
		exit 1
	fi

	"$GENESIS_BIN" \
		--network "$NETWORK" \
		--db-path "$DB_PATH" \
		--wallet-file "$GENESIS_WALLET_FILE" \
		--initial-validator "$VALIDATOR_PUBKEY" || exit 1
}

BOOTSTRAP=""
case $VALIDATOR_NUM in
	1)
		NAME="${NETWORK_UPPER}-V1-PRIMARY"
		;;
	*)
		NAME="${NETWORK_UPPER}-V${VALIDATOR_NUM}-SECONDARY"
		BOOTSTRAP="--bootstrap-peers 127.0.0.1:${BASE_P2P}"
		;;
esac

echo "MoltChain Validator: $NAME"
echo "=================================="
echo "Network: $NETWORK"
echo "RPC:     http://localhost:$RPC_PORT"
echo "WS:      ws://localhost:$WS_PORT"
echo "P2P:     ${LOCAL_LISTEN_ADDR}:$P2P_PORT"
echo "Signer:  http://localhost:$SIGNER_PORT"
echo "DB:      $DB_PATH"
echo "HOME:    $HOME"
echo ""

if [ "$VALIDATOR_NUM" = "1" ]; then
	echo "This is the PRIMARY validator (creates genesis)"
else
	echo "Bootstrapping from: 127.0.0.1:$BASE_P2P"
fi

echo ""
echo "Block Production (Tendermint BFT):"
echo "   No TXs: Heartbeat ~800ms (0.01 MOLT)"
echo "   With TXs: ~400ms blocks (0.02 MOLT)"
echo ""

if [ -z "${MOLTCHAIN_SIGNER_BIND:-}" ]; then
	export MOLTCHAIN_SIGNER_BIND="0.0.0.0:${SIGNER_PORT}"
fi

if [ "$NETWORK" = "testnet" ]; then
	EXTRA_FLAGS="--dev-mode"
else
	EXTRA_FLAGS=""
fi
for arg in "$@"; do
		case "$arg" in
				--dev-mode)
						EXTRA_FLAGS="$EXTRA_FLAGS --dev-mode"
						echo "⚠️  DEV MODE: Machine fingerprint bypassed (SHA-256 of pubkey)"
						;;
				--import-key)
						;;
		esac
done
for i in $(seq 1 $#); do
		if [ "${!i}" = "--import-key" ]; then
				next=$((i+1))
				if [ -n "${!next:-}" ]; then
						EXTRA_FLAGS="$EXTRA_FLAGS --import-key ${!next}"
						echo "📦 Importing keypair from: ${!next}"
				fi
		fi
done

write_local_seeds_file
ensure_local_genesis

if [[ "${MOLTCHAIN_SUPERVISED:-0}" == "1" ]]; then
	# External supervisor is active; run validator in direct worker mode to
	# avoid nested watchdogs and orphaned child processes.
	EXTRA_FLAGS="$EXTRA_FLAGS --supervised --no-watchdog"
fi

if [ -x "$VALIDATOR_BIN" ]; then
	exec "$VALIDATOR_BIN" \
		--network "$NETWORK" \
		--rpc-port "$RPC_PORT" \
		--ws-port "$WS_PORT" \
		--p2p-port "$P2P_PORT" \
		--listen-addr "$LOCAL_LISTEN_ADDR" \
		--db-path "$DB_PATH" \
		$BOOTSTRAP $EXTRA_FLAGS
else
	echo "Release binary not found at $VALIDATOR_BIN"
	echo "Building with cargo..."
	exec cargo run --release --bin moltchain-validator -- \
		--network "$NETWORK" \
		--rpc-port "$RPC_PORT" \
		--ws-port "$WS_PORT" \
		--p2p-port "$P2P_PORT" \
		--listen-addr "$LOCAL_LISTEN_ADDR" \
		--db-path "$DB_PATH" \
		$BOOTSTRAP $EXTRA_FLAGS
fi
