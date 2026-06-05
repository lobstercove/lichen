#!/usr/bin/env bash
set -euo pipefail

# End-to-end local BTC custody smoke using Bitcoin Core regtest.
# Exercises: RPC createBridgeDeposit -> custody BTC watcher -> sweep ->
# confirmed credit/mint -> burn submission -> BTC withdrawal broadcast/confirm.

BOOTSTRAP_PATH="/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
if [ -n "${HOME:-}" ] && [ -d "${HOME}/.cargo/bin" ]; then
  BOOTSTRAP_PATH="${HOME}/.cargo/bin:${BOOTSTRAP_PATH}"
fi
PATH="${BOOTSTRAP_PATH}:${PATH:-}"
export PATH

cd "$(dirname "$0")/.."
ROOT="$PWD"

RPC_URL="${LICHEN_RPC_URL:-http://127.0.0.1:8899}"
CUSTODY_HOST="${CUSTODY_HOST:-127.0.0.1}"
CUSTODY_PORT="${CUSTODY_LISTEN_PORT:-9105}"
CUSTODY_URL="http://${CUSTODY_HOST}:${CUSTODY_PORT}"
SMOKE_DIR="${BTC_SMOKE_DIR:-$ROOT/data/btc-regtest-smoke}"
BTC_DIR="$SMOKE_DIR/bitcoin"
LOG_DIR="$SMOKE_DIR/logs"
CUSTODY_DB="$SMOKE_DIR/custody-db"
RESULTS_FILE="$SMOKE_DIR/result.json"
BITCOIN_RPC_USER="${BITCOIN_RPC_USER:-lichen}"
BITCOIN_RPC_PASSWORD="${BITCOIN_RPC_PASSWORD:-lichen-regtest-pass}"
BITCOIN_RPC_PORT="${BITCOIN_RPC_PORT:-18443}"
BITCOIN_RPC_URL="http://127.0.0.1:${BITCOIN_RPC_PORT}"
BTC_DEPOSIT_AMOUNT="${BTC_DEPOSIT_AMOUNT:-0.00100000}"
WITHDRAWAL_WBTC_UNITS="${WITHDRAWAL_WBTC_UNITS:-500000}"
USER_SEED_BYTE="${USER_SEED_BYTE:-42}"
POLL_INTERVAL_SECS="${BTC_SMOKE_POLL_INTERVAL_SECS:-2}"
POLL_TIMEOUT_SECS="${BTC_SMOKE_TIMEOUT_SECS:-180}"

BITCOIND="${BITCOIND:-$(command -v bitcoind || true)}"
BITCOIN_CLI="${BITCOIN_CLI:-$(command -v bitcoin-cli || true)}"

if [ -z "$BITCOIND" ] || [ -z "$BITCOIN_CLI" ]; then
  echo "FATAL: bitcoind and bitcoin-cli are required. Install Bitcoin Core first." >&2
  exit 1
fi

if [ ! -x "$ROOT/target/release/lichen-custody" ]; then
  echo "FATAL: target/release/lichen-custody is missing; run cargo build --release --bin lichen-custody" >&2
  exit 1
fi

mkdir -p "$SMOKE_DIR" "$BTC_DIR" "$LOG_DIR"

TOKEN_FILE="$ROOT/data/local-cluster/custody-api-auth-token"
if [ -n "${CUSTODY_API_AUTH_TOKEN:-}" ]; then
  AUTH_TOKEN="$CUSTODY_API_AUTH_TOKEN"
elif [ -f "$TOKEN_FILE" ]; then
  AUTH_TOKEN="$(cat "$TOKEN_FILE")"
else
  AUTH_TOKEN="$(python3 - <<'PY'
import secrets
print(secrets.token_hex(24))
PY
)"
  mkdir -p "$(dirname "$TOKEN_FILE")"
  printf '%s' "$AUTH_TOKEN" > "$TOKEN_FILE"
  chmod 600 "$TOKEN_FILE" 2>/dev/null || true
fi

KEYPAIR_PASSWORD_FILE="$ROOT/data/local-cluster/keypair-password"
if [ -z "${LICHEN_KEYPAIR_PASSWORD:-}" ] && [ -f "$KEYPAIR_PASSWORD_FILE" ]; then
  export LICHEN_KEYPAIR_PASSWORD="$(cat "$KEYPAIR_PASSWORD_FILE")"
fi

GENESIS_MINTER_KEYPAIR="${CUSTODY_TREASURY_KEYPAIR:-$ROOT/data/state-7001/genesis-keys/genesis-primary-lichen-testnet-1.json}"
if [ ! -f "$GENESIS_MINTER_KEYPAIR" ]; then
  echo "FATAL: WBTC minter keypair not found at $GENESIS_MINTER_KEYPAIR" >&2
  exit 1
fi
FUNDING_KEYPAIR="${BTC_SMOKE_FUNDING_KEYPAIR:-$ROOT/data/state-7001/genesis-keys/treasury-lichen-testnet-1.json}"
if [ ! -f "$FUNDING_KEYPAIR" ]; then
  echo "FATAL: local funding keypair not found at $FUNDING_KEYPAIR" >&2
  exit 1
fi
GENESIS_MINTER_PUBKEY="$(python3 - "$GENESIS_MINTER_KEYPAIR" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
print(data.get("publicKeyBase58") or data.get("pubkey") or data.get("public_key") or "")
PY
)"
if [ -z "$GENESIS_MINTER_PUBKEY" ]; then
  echo "FATAL: could not read minter pubkey from $GENESIS_MINTER_KEYPAIR" >&2
  exit 1
fi

rpc_json() {
  local method="$1"
  local params="${2:-[]}"
  python3 - "$RPC_URL" "$method" "$params" <<'PY'
import json, sys, urllib.request
url, method, params_json = sys.argv[1:4]
payload = json.dumps({
    "jsonrpc": "2.0",
    "id": 1,
    "method": method,
    "params": json.loads(params_json),
}).encode()
req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"})
with urllib.request.urlopen(req, timeout=15) as resp:
    data = json.loads(resp.read().decode())
if data.get("error"):
    raise SystemExit(f"RPC {method} failed: {data['error']}")
print(json.dumps(data.get("result")))
PY
}

json_get() {
  python3 - "$1" "$2" <<'PY'
import json, sys
value = json.loads(sys.argv[1])
for part in sys.argv[2].split("."):
    if isinstance(value, list):
        value = value[int(part)]
    else:
        value = value[part]
print(value if isinstance(value, str) else json.dumps(value))
PY
}

btc_cli() {
  "$BITCOIN_CLI" \
    -regtest \
    -datadir="$BTC_DIR" \
    -rpcuser="$BITCOIN_RPC_USER" \
    -rpcpassword="$BITCOIN_RPC_PASSWORD" \
    -rpcport="$BITCOIN_RPC_PORT" \
    "$@"
}

btc_wallet_cli() {
  btc_cli -rpcwallet=miner "$@"
}

wait_for_bitcoin() {
  local deadline=$((SECONDS + 60))
  until btc_cli getblockchaininfo >/dev/null 2>&1; do
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "FATAL: bitcoind did not become ready" >&2
      tail -n 80 "$LOG_DIR/bitcoind.log" >&2 || true
      exit 1
    fi
    sleep 1
  done
}

start_bitcoind() {
  if [ -f "$SMOKE_DIR/bitcoind.pid" ] && kill -0 "$(cat "$SMOKE_DIR/bitcoind.pid")" 2>/dev/null; then
    wait_for_bitcoin
    return
  fi

  "$BITCOIND" \
    -regtest \
    -datadir="$BTC_DIR" \
    -server=1 \
    -txindex=1 \
    -fallbackfee=0.0001 \
    -rpcbind=127.0.0.1 \
    -rpcallowip=127.0.0.1 \
    -rpcuser="$BITCOIN_RPC_USER" \
    -rpcpassword="$BITCOIN_RPC_PASSWORD" \
    -rpcport="$BITCOIN_RPC_PORT" \
    -listen=0 \
    >"$LOG_DIR/bitcoind.log" 2>&1 &
  echo "$!" > "$SMOKE_DIR/bitcoind.pid"
  wait_for_bitcoin
}

ensure_miner_wallet() {
  if ! btc_wallet_cli getwalletinfo >/dev/null 2>&1; then
    if ! btc_cli -named createwallet wallet_name=miner descriptors=true load_on_startup=true >/dev/null 2>&1; then
      btc_cli loadwallet miner >/dev/null
    fi
  fi
  local blocks
  blocks="$(btc_cli getblockcount)"
  if [ "$blocks" -lt 101 ]; then
    local addr
    addr="$(btc_wallet_cli getnewaddress "miner" bech32)"
    btc_cli generatetoaddress $((101 - blocks)) "$addr" >/dev/null
  fi
}

mine_blocks() {
  local count="$1"
  local addr
  addr="$(btc_wallet_cli getnewaddress "miner" bech32)"
  btc_cli generatetoaddress "$count" "$addr" >/dev/null
}

free_custody_port() {
  local pids
  pids="$(lsof -ti tcp:"$CUSTODY_PORT" 2>/dev/null || true)"
  if [ -n "$pids" ]; then
    for pid in $pids; do
      kill "$pid" 2>/dev/null || true
    done
    sleep 1
    pids="$(lsof -ti tcp:"$CUSTODY_PORT" 2>/dev/null || true)"
    for pid in $pids; do
      kill -9 "$pid" 2>/dev/null || true
    done
  fi
}

wait_for_custody() {
  local deadline=$((SECONDS + 60))
  until curl -fsS "$CUSTODY_URL/health" >/dev/null 2>&1; do
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "FATAL: lichen-custody did not become ready" >&2
      tail -n 120 "$LOG_DIR/custody.log" >&2 || true
      exit 1
    fi
    sleep 1
  done
}

start_custody() {
  free_custody_port
  rm -rf "$CUSTODY_DB"

  CUSTODY_DB_PATH="$CUSTODY_DB" \
  CUSTODY_API_AUTH_TOKEN="$AUTH_TOKEN" \
  CUSTODY_MASTER_SEED="${CUSTODY_MASTER_SEED:-local-btc-smoke-treasury-seed-v01}" \
  CUSTODY_DEPOSIT_MASTER_SEED="${CUSTODY_DEPOSIT_MASTER_SEED:-local-btc-smoke-deposit-seed-v01}" \
  CUSTODY_SIGNER_ENDPOINTS="" \
  CUSTODY_SIGNER_THRESHOLD=0 \
  CUSTODY_LICHEN_RPC_URL="$RPC_URL" \
  CUSTODY_TREASURY_KEYPAIR="$GENESIS_MINTER_KEYPAIR" \
  CUSTODY_BTC_RPC_URL="$BITCOIN_RPC_URL" \
  CUSTODY_BTC_RPC_USER="$BITCOIN_RPC_USER" \
  CUSTODY_BTC_RPC_PASSWORD="$BITCOIN_RPC_PASSWORD" \
  CUSTODY_BTC_NETWORK=regtest \
  CUSTODY_BTC_CONFIRMATIONS=1 \
  CUSTODY_BTC_FEE_RATE_SATS_VB=2 \
  CUSTODY_WBTC_TOKEN_ADDR="$WBTC_ADDR" \
  CUSTODY_POLL_INTERVAL_SECS="$POLL_INTERVAL_SECS" \
  CUSTODY_LISTEN_PORT="$CUSTODY_PORT" \
  "$ROOT/target/release/lichen-custody" \
    >"$LOG_DIR/custody.log" 2>&1 &
  echo "$!" > "$SMOKE_DIR/custody.pid"
  wait_for_custody
}

custody_get() {
  local path="$1"
  curl -fsS \
    -H "Authorization: Bearer $AUTH_TOKEN" \
    "$CUSTODY_URL$path"
}

custody_post_file() {
  local path="$1"
  local file="$2"
  curl -fsS \
    -X POST \
    -H "Authorization: Bearer $AUTH_TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary "@$file" \
    "$CUSTODY_URL$path"
}

custody_put_json() {
  local path="$1"
  local json="$2"
  curl -fsS \
    -X PUT \
    -H "Authorization: Bearer $AUTH_TOKEN" \
    -H "Content-Type: application/json" \
    -d "$json" \
    "$CUSTODY_URL$path"
}

wait_event() {
  local event_type="$1"
  local entity_id="$2"
  local deadline=$((SECONDS + POLL_TIMEOUT_SECS))
  while [ "$SECONDS" -lt "$deadline" ]; do
    local events
    events="$(custody_get "/events?event_type=${event_type}&limit=50" || true)"
    if python3 - "$events" "$event_type" "$entity_id" >/dev/null <<'PY'
import json, sys
try:
    data = json.loads(sys.argv[1] or "{}")
except Exception:
    raise SystemExit(1)
etype, entity = sys.argv[2:4]
for event in data.get("events", []):
    if event.get("event_type") == etype and (
        event.get("entity_id") == entity or event.get("deposit_id") == entity
    ):
        print(json.dumps(event))
        raise SystemExit(0)
raise SystemExit(1)
PY
    then
      python3 - "$events" "$event_type" "$entity_id" <<'PY'
import json, sys
data = json.loads(sys.argv[1])
etype, entity = sys.argv[2:4]
for event in data.get("events", []):
    if event.get("event_type") == etype and (
        event.get("entity_id") == entity or event.get("deposit_id") == entity
    ):
        print(json.dumps(event))
        break
PY
      return 0
    fi
    sleep "$POLL_INTERVAL_SECS"
  done
  echo "FATAL: timed out waiting for event_type=${event_type} entity_id=${entity_id}" >&2
  tail -n 160 "$LOG_DIR/custody.log" >&2 || true
  exit 1
}

wait_status_field() {
  local kind="$1"
  local status="$2"
  local min_count="$3"
  local deadline=$((SECONDS + POLL_TIMEOUT_SECS))
  while [ "$SECONDS" -lt "$deadline" ]; do
    local status_json
    status_json="$(custody_get "/status" || true)"
    if python3 - "$status_json" "$kind" "$status" "$min_count" <<'PY'
import json, sys
data = json.loads(sys.argv[1] or "{}")
kind, status, minimum = sys.argv[2], sys.argv[3], int(sys.argv[4])
count = int(data.get(kind, {}).get(status, 0))
raise SystemExit(0 if count >= minimum else 1)
PY
    then
      return 0
    fi
    sleep "$POLL_INTERVAL_SECS"
  done
  echo "FATAL: timed out waiting for ${kind}.${status} >= ${min_count}" >&2
  custody_get "/status" >&2 || true
  tail -n 160 "$LOG_DIR/custody.log" >&2 || true
  exit 1
}

create_bridge_deposit_via_rpc() {
  local payload_file="$1"
  python3 - "$RPC_URL" "$payload_file" <<'PY'
import json, sys, urllib.request
url, payload_file = sys.argv[1:3]
payload = json.load(open(payload_file))
req_body = json.dumps({
    "jsonrpc": "2.0",
    "id": 1,
    "method": "createBridgeDeposit",
    "params": [payload],
}).encode()
req = urllib.request.Request(url, data=req_body, headers={"Content-Type": "application/json"})
with urllib.request.urlopen(req, timeout=20) as resp:
    data = json.loads(resp.read().decode())
if data.get("error"):
    raise SystemExit(f"createBridgeDeposit failed: {data['error']}")
print(json.dumps(data["result"]))
PY
}

WBTC_ADDR="$(rpc_json getSymbolRegistry '["WBTC"]' | python3 -c 'import json,sys; v=json.load(sys.stdin); print(v["program"])')"
if [ -z "$WBTC_ADDR" ] || [ "$WBTC_ADDR" = "null" ]; then
  echo "FATAL: WBTC is not registered on $RPC_URL" >&2
  exit 1
fi

echo "[btc-smoke] WBTC=$WBTC_ADDR"
echo "[btc-smoke] starting Bitcoin Core regtest"
start_bitcoind
ensure_miner_wallet

echo "[btc-smoke] starting BTC-enabled custody on $CUSTODY_URL"
start_custody

BRIDGE_AUTH_FILE="$SMOKE_DIR/bridge-auth.json"
"$ROOT/target/release/bridge_auth_payload" \
  --chain bitcoin \
  --asset btc \
  --seed-byte "$USER_SEED_BYTE" \
  --ttl-secs 3600 \
  --nonce "btc-regtest-smoke-deposit-$(date +%s)" \
  > "$BRIDGE_AUTH_FILE"
USER_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["user_id"])' "$BRIDGE_AUTH_FILE")"

echo "[btc-smoke] funding smoke user $USER_ID with local LICN for burn fees"
echo "[btc-smoke] funding WBTC minter $GENESIS_MINTER_PUBKEY with local LICN for mint fees"
"$ROOT/target/release/lichen" \
  --rpc-url "$RPC_URL" \
  transfer "$GENESIS_MINTER_PUBKEY" 5 \
  --keypair "$FUNDING_KEYPAIR" \
  --output json \
  > "$SMOKE_DIR/minter-licn-transfer.json"
"$ROOT/target/release/lichen" \
  --rpc-url "$RPC_URL" \
  transfer "$USER_ID" 2 \
  --keypair "$FUNDING_KEYPAIR" \
  --output json \
  > "$SMOKE_DIR/licn-transfer.json"

echo "[btc-smoke] creating BTC deposit via validator RPC proxy"
DEPOSIT_JSON="$(create_bridge_deposit_via_rpc "$BRIDGE_AUTH_FILE")"
DEPOSIT_ID="$(json_get "$DEPOSIT_JSON" deposit_id)"
DEPOSIT_ADDR="$(json_get "$DEPOSIT_JSON" address)"
echo "[btc-smoke] deposit_id=$DEPOSIT_ID address=$DEPOSIT_ADDR"

echo "[btc-smoke] sending $BTC_DEPOSIT_AMOUNT BTC to deposit address"
DEPOSIT_TXID="$(btc_wallet_cli sendtoaddress "$DEPOSIT_ADDR" "$BTC_DEPOSIT_AMOUNT")"
mine_blocks 1
echo "[btc-smoke] deposit tx mined: $DEPOSIT_TXID"

wait_event deposit.confirmed "$DEPOSIT_ID" > "$SMOKE_DIR/event-deposit-confirmed.json"
wait_event sweep.submitted "$DEPOSIT_ID" > "$SMOKE_DIR/event-sweep-submitted.json"
SWEEP_TXID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("tx_hash",""))' "$SMOKE_DIR/event-sweep-submitted.json")"
echo "[btc-smoke] sweep submitted: $SWEEP_TXID"
mine_blocks 1
wait_event sweep.confirmed "$DEPOSIT_ID" > "$SMOKE_DIR/event-sweep-confirmed.json"
wait_event credit.confirmed "$DEPOSIT_ID" > "$SMOKE_DIR/event-credit-confirmed.json"
echo "[btc-smoke] credit confirmed"

WBTC_STATS_AFTER_CREDIT="$(rpc_json getWbtcStats '[]')"
WBTC_SUPPLY_AFTER_CREDIT="$(json_get "$WBTC_STATS_AFTER_CREDIT" supply)"
if [ "$WBTC_SUPPLY_AFTER_CREDIT" -lt "$WITHDRAWAL_WBTC_UNITS" ]; then
  echo "FATAL: WBTC supply $WBTC_SUPPLY_AFTER_CREDIT is below withdrawal amount $WITHDRAWAL_WBTC_UNITS" >&2
  exit 1
fi

DEST_BTC_ADDR="$(btc_wallet_cli getnewaddress "withdrawal-dest" bech32)"
WITHDRAW_AUTH_FILE="$SMOKE_DIR/withdrawal-auth.json"
"$ROOT/target/release/withdrawal_auth_payload" \
  --asset wbtc \
  --amount "$WITHDRAWAL_WBTC_UNITS" \
  --dest-chain bitcoin \
  --dest-address "$DEST_BTC_ADDR" \
  --seed-byte "$USER_SEED_BYTE" \
  --ttl-secs 3600 \
  --nonce "btc-regtest-smoke-withdraw-$(date +%s)" \
  > "$WITHDRAW_AUTH_FILE"

echo "[btc-smoke] creating WBTC withdrawal to $DEST_BTC_ADDR"
WITHDRAW_JSON="$(custody_post_file /withdrawals "$WITHDRAW_AUTH_FILE")"
WITHDRAW_JOB_ID="$(json_get "$WITHDRAW_JSON" job_id)"
echo "[btc-smoke] withdrawal_job=$WITHDRAW_JOB_ID"

echo "[btc-smoke] burning $WITHDRAWAL_WBTC_UNITS wBTC"
BURN_JSON="$("$ROOT/target/release/wrapped_burn" \
  --rpc-url "$RPC_URL" \
  --contract "$WBTC_ADDR" \
  --amount "$WITHDRAWAL_WBTC_UNITS" \
  --seed-byte "$USER_SEED_BYTE")"
BURN_SIG="$(json_get "$BURN_JSON" signature)"
printf '%s\n' "$BURN_JSON" > "$SMOKE_DIR/burn.json"

custody_put_json "/withdrawals/${WITHDRAW_JOB_ID}/burn" "{\"burn_tx_signature\":\"${BURN_SIG}\"}" \
  > "$SMOKE_DIR/submit-burn.json"

wait_event withdrawal.burn_confirmed "$WITHDRAW_JOB_ID" > "$SMOKE_DIR/event-withdrawal-burn-confirmed.json"
wait_event withdrawal.broadcast "$WITHDRAW_JOB_ID" > "$SMOKE_DIR/event-withdrawal-broadcast.json"
WITHDRAW_TXID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("tx_hash",""))' "$SMOKE_DIR/event-withdrawal-broadcast.json")"
echo "[btc-smoke] withdrawal broadcast: $WITHDRAW_TXID"
mine_blocks 1
wait_event withdrawal.confirmed "$WITHDRAW_JOB_ID" > "$SMOKE_DIR/event-withdrawal-confirmed.json"

FINAL_WITHDRAWAL="$(custody_get "/withdrawals/${WITHDRAW_JOB_ID}")"
FINAL_STATUS="$(json_get "$FINAL_WITHDRAWAL" withdrawal.status)"
if [ "$FINAL_STATUS" != "confirmed" ]; then
  echo "FATAL: withdrawal status is $FINAL_STATUS, expected confirmed" >&2
  exit 1
fi

FINAL_WBTC_STATS="$(rpc_json getWbtcStats '[]')"
FINAL_STATUS_JSON="$(custody_get /status)"

python3 - "$RESULTS_FILE" <<PY
import json
result = {
    "status": "passed",
    "rpc_url": "$RPC_URL",
    "custody_url": "$CUSTODY_URL",
    "bitcoin_rpc_url": "$BITCOIN_RPC_URL",
    "wbtc_contract": "$WBTC_ADDR",
    "user_id": "$USER_ID",
    "deposit_id": "$DEPOSIT_ID",
    "deposit_address": "$DEPOSIT_ADDR",
    "deposit_txid": "$DEPOSIT_TXID",
    "sweep_txid": "$SWEEP_TXID",
    "withdrawal_job_id": "$WITHDRAW_JOB_ID",
    "withdrawal_dest_address": "$DEST_BTC_ADDR",
    "withdrawal_txid": "$WITHDRAW_TXID",
    "withdrawal_status": "$FINAL_STATUS",
    "wbtc_supply_after_credit": int("$WBTC_SUPPLY_AFTER_CREDIT"),
    "final_wbtc_stats": json.loads('''$FINAL_WBTC_STATS'''),
    "custody_status": json.loads('''$FINAL_STATUS_JSON'''),
}
open("$RESULTS_FILE", "w").write(json.dumps(result, indent=2, sort_keys=True) + "\\n")
print(json.dumps(result, indent=2, sort_keys=True))
PY

echo "[btc-smoke] PASS result=$RESULTS_FILE"
echo "[btc-smoke] services left running: bitcoind pid=$(cat "$SMOKE_DIR/bitcoind.pid"), custody pid=$(cat "$SMOKE_DIR/custody.pid")"
