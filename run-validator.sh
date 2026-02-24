#!/bin/bash
# ============================================================================
# MoltChain Validator Launcher
# ============================================================================
#
# Usage: ./run-validator.sh [network] <validator_number>
#   network: testnet | mainnet (default: testnet)
#
# Port Assignments (testnet):
#   V1: p2p=8000  rpc=8899  ws=8900
#   V2: p2p=8001  rpc=8901  ws=8902
#   V3: p2p=8002  rpc=8903  ws=8904
#
# Port Assignments (mainnet):
#   V1: p2p=9000  rpc=9899  ws=9900
#   V2: p2p=9001  rpc=9901  ws=9902
#   V3: p2p=9002  rpc=9903  ws=9904
#
# DB paths are always absolute: $REPO_ROOT/data/state-{p2p_port}
# ============================================================================

NETWORK=${1:-testnet}
VALIDATOR_NUM=${2:-1}

if [[ "$NETWORK" =~ ^[0-9]+$ ]]; then
	VALIDATOR_NUM=$NETWORK
	NETWORK=testnet
fi

NETWORK=$(echo "$NETWORK" | tr '[:upper:]' '[:lower:]')
NETWORK_UPPER=$(echo "$NETWORK" | tr '[:lower:]' '[:upper:]')

case $NETWORK in
	testnet)
		BASE_P2P=8000
		BASE_RPC=8899
		BASE_WS=8900
		;;
	mainnet)
		BASE_P2P=9000
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

P2P_PORT=$((BASE_P2P + (VALIDATOR_NUM - 1)))
RPC_PORT=$((BASE_RPC + 2 * (VALIDATOR_NUM - 1)))
WS_PORT=$((BASE_WS + 2 * (VALIDATOR_NUM - 1)))
SIGNER_PORT=$((9200 + VALIDATOR_NUM))

DB_PATH="${REPO_ROOT}/data/state-${P2P_PORT}"

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
echo "P2P:     0.0.0.0:$P2P_PORT"
echo "Signer:  http://localhost:$SIGNER_PORT"
echo "DB:      $DB_PATH"
echo ""

if [ "$VALIDATOR_NUM" = "1" ]; then
	echo "This is the PRIMARY validator (creates genesis)"
else
	echo "Bootstrapping from: 127.0.0.1:$BASE_P2P"
fi

echo ""
echo "Block Production:"
echo "   No TXs: Heartbeat every 5s (0.135 MOLT)"
echo "   With TXs: 400ms blocks (0.9 MOLT)"
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

BIN_PATH="${REPO_ROOT}/target/release/moltchain-validator"
if [ -x "$BIN_PATH" ]; then
	exec "$BIN_PATH" \
		--network "$NETWORK" \
		--rpc-port "$RPC_PORT" \
		--ws-port "$WS_PORT" \
		--p2p-port "$P2P_PORT" \
		--db-path "$DB_PATH" \
		$BOOTSTRAP $EXTRA_FLAGS
else
	echo "Release binary not found at $BIN_PATH"
	echo "Building with cargo..."
	exec cargo run --release --bin moltchain-validator -- \
		--network "$NETWORK" \
		--rpc-port "$RPC_PORT" \
		--ws-port "$WS_PORT" \
		--p2p-port "$P2P_PORT" \
		--db-path "$DB_PATH" \
		$BOOTSTRAP $EXTRA_FLAGS
fi
