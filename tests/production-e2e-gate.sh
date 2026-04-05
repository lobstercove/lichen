#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"
LICHEN_BIN="${LICHEN_BIN:-$ROOT_DIR/target/release/lichen}"
RPC_URL="${RPC_URL:-http://localhost:8899}"
WS_URL="${WS_URL:-ws://localhost:8900}"
PUBLIC_EDGE_REQUIRED="${PUBLIC_EDGE_REQUIRED:-auto}"
PUBLIC_EDGE_RPC_URL="${PUBLIC_EDGE_RPC_URL:-}"
PUBLIC_EDGE_WS_URL="${PUBLIC_EDGE_WS_URL:-}"
PUBLIC_EDGE_FAUCET_URL="${PUBLIC_EDGE_FAUCET_URL:-}"
PUBLIC_EDGE_CUSTODY_URL="${PUBLIC_EDGE_CUSTODY_URL:-}"
PUBLIC_EDGE_ORIGIN="${PUBLIC_EDGE_ORIGIN:-https://dex.lichen.network}"
TREASURY_KEYPAIR="${TREASURY_KEYPAIR:-}"
if [[ -z "$TREASURY_KEYPAIR" || ! -f "$TREASURY_KEYPAIR" ]]; then
  # Auto-discover treasury keypair from any state directory
  TREASURY_KEYPAIR="$(find "$ROOT_DIR/data" -name 'treasury-lichen-testnet-1.json' -type f 2>/dev/null | head -1 || true)"
fi
REQUIRE_MULTI_VALIDATOR="${REQUIRE_MULTI_VALIDATOR:-1}"
STRICT_NO_SKIPS="${STRICT_NO_SKIPS:-1}"
RUN_SDK_SUITE="${RUN_SDK_SUITE:-1}"
RUN_DEEP_SERVICES_SUITE="${RUN_DEEP_SERVICES_SUITE:-1}"
RUN_CUSTODY_WITHDRAWAL_E2E="${RUN_CUSTODY_WITHDRAWAL_E2E:-1}"
RUN_CONTRACT_WRITE_SUITE="${RUN_CONTRACT_WRITE_SUITE:-1}"
REQUIRE_DEX_API="${REQUIRE_DEX_API:-1}"
# Auto-detect faucet and custody availability if not explicitly set
if [[ -z "${REQUIRE_FAUCET+x}" ]]; then
  if curl -sS --max-time 2 "${FAUCET_URL:-http://localhost:9100}/health" >/dev/null 2>&1; then
    REQUIRE_FAUCET=1
  else
    REQUIRE_FAUCET=0
  fi
fi
if [[ -z "${REQUIRE_CUSTODY+x}" ]]; then
  if curl -sS --max-time 2 "${CUSTODY_URL:-http://localhost:9105}/health" >/dev/null 2>&1; then
    REQUIRE_CUSTODY=1
  else
    REQUIRE_CUSTODY=0
  fi
fi
REQUIRE_LAUNCHPAD="${REQUIRE_LAUNCHPAD:-1}"
REQUIRE_TOKEN_WRITE="${REQUIRE_TOKEN_WRITE:-1}"
REQUIRE_ALL_CONTRACTS="${REQUIRE_ALL_CONTRACTS:-1}"
REQUIRE_ALL_SCENARIOS="${REQUIRE_ALL_SCENARIOS:-1}"
STRICT_WRITE_ASSERTIONS="${STRICT_WRITE_ASSERTIONS:-0}"
TX_CONFIRM_TIMEOUT_SECS="${TX_CONFIRM_TIMEOUT_SECS:-25}"
REQUIRE_FULL_WRITE_ACTIVITY="${REQUIRE_FULL_WRITE_ACTIVITY:-0}"
MIN_CONTRACT_ACTIVITY_DELTA="${MIN_CONTRACT_ACTIVITY_DELTA:-0}"
ENFORCE_DOMAIN_ASSERTIONS="${ENFORCE_DOMAIN_ASSERTIONS:-auto}"
ENABLE_NEGATIVE_ASSERTIONS="${ENABLE_NEGATIVE_ASSERTIONS:-1}"
REQUIRE_NEGATIVE_REASON_MATCH="${REQUIRE_NEGATIVE_REASON_MATCH:-1}"
REQUIRE_NEGATIVE_CODE_MATCH="${REQUIRE_NEGATIVE_CODE_MATCH:-0}"
REQUIRE_SCENARIO_FOR_DISCOVERED="${REQUIRE_SCENARIO_FOR_DISCOVERED:-1}"
MIN_NEGATIVE_ASSERTIONS_EXECUTED="${MIN_NEGATIVE_ASSERTIONS_EXECUTED:-5}"
REQUIRE_EXPECTED_CONTRACT_SET="${REQUIRE_EXPECTED_CONTRACT_SET:-1}"
EXPECTED_CONTRACTS_FILE="${EXPECTED_CONTRACTS_FILE:-$ROOT_DIR/tests/expected-contracts.json}"
CONTRACT_ACTIVITY_OVERRIDES_DEFAULT='{}'
CONTRACT_ACTIVITY_OVERRIDES="${CONTRACT_ACTIVITY_OVERRIDES:-$CONTRACT_ACTIVITY_OVERRIDES_DEFAULT}"
WRITE_E2E_REPORT_PATH="${WRITE_E2E_REPORT_PATH:-$ROOT_DIR/tests/artifacts/contracts-write-e2e-report.json}"
CONTRACT_WRITE_KEYPAIR="${CONTRACT_WRITE_KEYPAIR:-$ROOT_DIR/keypairs/deployer.json}"
DEX_BOOTSTRAP_BASE_SYMBOL="${DEX_BOOTSTRAP_BASE_SYMBOL-LICN}"
DEX_BOOTSTRAP_QUOTE_SYMBOL="${DEX_BOOTSTRAP_QUOTE_SYMBOL-LUSD}"
DEX_API_URL="${DEX_API_URL:-${RPC_URL}/api/v1}"
FAUCET_URL="${FAUCET_URL:-http://localhost:9100}"
CUSTODY_URL="${CUSTODY_URL:-http://localhost:9105}"
PYTHON_BIN="${PYTHON_BIN:-$ROOT_DIR/.venv/bin/python}"
CUSTODY_WITHDRAWAL_FIXTURE_RESOLVER="${CUSTODY_WITHDRAWAL_FIXTURE_RESOLVER:-$ROOT_DIR/tests/resolve-custody-withdrawal-fixtures.py}"

AGENT_WALLET_NAME="${AGENT_WALLET_NAME:-e2e-agent}"
HUMAN_WALLET_NAME="${HUMAN_WALLET_NAME:-e2e-human}"
TREASURY_FUND_LICN="${TREASURY_FUND_LICN:-1000}"
SIGNER_KEYPAIR="${SIGNER_KEYPAIR:-$HOME/.lichen/keypairs/id.json}"
CUSTODY_WITHDRAWAL_AUTH_TOKEN=""
CUSTODY_WITHDRAWAL_GENESIS_KEYS_DIR=""

PASS=0
FAIL=0
SKIP=0
ERRORS=()

log() {
  echo "[E2E-GATE] $*"
}

pass() {
  echo "✅ $*"
  PASS=$((PASS + 1))
}

fail() {
  echo "❌ $*"
  FAIL=$((FAIL + 1))
  ERRORS+=("$*")
}

skip() {
  echo "⏭️  $*"
  SKIP=$((SKIP + 1))
}

require_cmd() {
  local cmd="$1"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    fail "Missing required command: $cmd"
    return 1
  fi
  pass "Command available: $cmd"
}

rpc_call() {
  local method="$1"
  local params="$2"
  curl -sS --max-time 8 -X POST "$RPC_URL" \
    -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}"
}

rpc_has_result() {
  local method="$1"
  local params="$2"
  local out
  out="$(rpc_call "$method" "$params" || true)"
  echo "$out" | jq -e '.result' >/dev/null 2>&1
}

rpc_call_at_url() {
  local url="$1"
  local method="$2"
  local params="$3"
  curl -sS --max-time 8 -X POST "$url" \
    -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}"
}

rpc_has_result_at_url() {
  local url="$1"
  local method="$2"
  local params="$3"
  local out
  out="$(rpc_call_at_url "$url" "$method" "$params" || true)"
  echo "$out" | jq -e '.result' >/dev/null 2>&1
}

url_host() {
  local url="$1"
  printf '%s' "$url" | sed -E 's#^[a-zA-Z]+://([^/:]+).*#\1#'
}

url_is_public() {
  local host
  host="$(url_host "$1")"

  [[ -n "$host" \
    && "$host" != "localhost" \
    && "$host" != "127.0.0.1" \
    && "$host" != "::1" \
    && "$host" != "0.0.0.0" ]]
}

derive_ws_url_from_rpc() {
  local rpc_url="$1"
  local normalized="${rpc_url%/}"

  case "$normalized" in
    wss://*|ws://*)
      echo "$normalized"
      ;;
    https://*/ws|http://*/ws)
      echo "${normalized/https:/wss:}" | sed 's#^http:#ws:#'
      ;;
    https://*)
      echo "${normalized/https:/wss:}/ws"
      ;;
    http://*)
      echo "${normalized/http:/ws:}/ws"
      ;;
    *)
      echo ""
      ;;
  esac
}

resolve_public_edge_defaults() {
  if [[ -z "$PUBLIC_EDGE_RPC_URL" ]] && url_is_public "$RPC_URL"; then
    PUBLIC_EDGE_RPC_URL="${RPC_URL%/}"
  fi

  if [[ -z "$PUBLIC_EDGE_WS_URL" ]]; then
    if url_is_public "$WS_URL"; then
      PUBLIC_EDGE_WS_URL="${WS_URL%/}"
    elif [[ -n "$PUBLIC_EDGE_RPC_URL" ]]; then
      PUBLIC_EDGE_WS_URL="$(derive_ws_url_from_rpc "$PUBLIC_EDGE_RPC_URL")"
    fi
  fi

  if [[ -z "$PUBLIC_EDGE_FAUCET_URL" ]] && url_is_public "$FAUCET_URL"; then
    PUBLIC_EDGE_FAUCET_URL="${FAUCET_URL%/}"
  fi

  if [[ -z "$PUBLIC_EDGE_CUSTODY_URL" ]] && url_is_public "$CUSTODY_URL"; then
    PUBLIC_EDGE_CUSTODY_URL="${CUSTODY_URL%/}"
  fi

  if [[ "$PUBLIC_EDGE_REQUIRED" == "auto" ]]; then
    if [[ -n "$PUBLIC_EDGE_RPC_URL" || -n "$PUBLIC_EDGE_WS_URL" || -n "$PUBLIC_EDGE_FAUCET_URL" || -n "$PUBLIC_EDGE_CUSTODY_URL" ]]; then
      PUBLIC_EDGE_REQUIRED=1
    else
      PUBLIC_EDGE_REQUIRED=0
    fi
  fi

  if [[ "$ENFORCE_DOMAIN_ASSERTIONS" == "auto" ]]; then
    ENFORCE_DOMAIN_ASSERTIONS="$PUBLIC_EDGE_REQUIRED"
  fi
}

check_public_preflight() {
  local url="$1"
  local origin="$2"
  local label="$3"
  local response=""
  local status_line=""
  local allow_origin=""

  response="$(curl -si --max-time 10 -X OPTIONS "$url" \
    -H "Origin: $origin" \
    -H 'Access-Control-Request-Method: POST' \
    -H 'Access-Control-Request-Headers: content-type' || true)"
  response="$(printf '%s' "$response" | tr -d '\r')"
  status_line="$(printf '%s' "$response" | head -n 1)"
  allow_origin="$(printf '%s' "$response" | awk -F': ' 'tolower($1)=="access-control-allow-origin" {print $2; exit}')"

  if printf '%s' "$status_line" | grep -Eq 'HTTP/[0-9.]+ (200|204)'; then
    pass "$label CORS preflight returned ${status_line#HTTP/* }"
  else
    fail "$label CORS preflight failed: ${status_line:-no response}"
    return 1
  fi

  if [[ "$allow_origin" == "$origin" ]]; then
    pass "$label CORS origin allows $origin"
  else
    fail "$label CORS origin mismatch (wanted $origin, got ${allow_origin:-<missing>})"
    return 1
  fi
}

check_public_rpc_method() {
  local url="$1"
  local method="$2"

  if rpc_has_result_at_url "$url" "$method" '[]'; then
    pass "Public RPC $method succeeded via $url"
  else
    fail "Public RPC $method failed via $url"
    return 1
  fi
}

check_public_http_health() {
  local url="$1"
  local label="$2"
  local response=""
  local health_url="${url%/}/health"

  response="$(curl -sS --max-time 8 "$health_url" || true)"
  if echo "$response" | jq -e '.status == "ok"' >/dev/null 2>&1 || echo "$response" | grep -qi 'ok'; then
    pass "$label public health endpoint succeeded via $health_url"
  else
    fail "$label public health endpoint failed via $health_url"
    return 1
  fi
}

check_public_websocket() {
  local url="$1"

  if "$PYTHON_BIN" - "$url" <<'PY' >/dev/null 2>&1
import asyncio
import json
import sys

import websockets

url = sys.argv[1]


async def main() -> None:
    async with websockets.connect(url, open_timeout=10, close_timeout=5, ping_interval=None) as ws:
        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "subscribeSlots", "params": []}))
        message = await asyncio.wait_for(ws.recv(), timeout=10)
        payload = json.loads(message)
        if payload.get("id") == 1 and "result" in payload:
            return
        if payload.get("method") == "subscription":
            return
        raise SystemExit(1)


asyncio.run(main())
PY
  then
    pass "Public WebSocket subscription succeeded via $url"
  else
    fail "Public WebSocket subscription failed via $url"
    return 1
  fi
}

run_public_edge_checks() {
  if [[ "$PUBLIC_EDGE_REQUIRED" != "1" ]]; then
    pass "Public edge checks not required for local gate target"
    return 0
  fi

  if [[ -z "$PUBLIC_EDGE_RPC_URL" ]]; then
    fail "Public edge checks required but PUBLIC_EDGE_RPC_URL could not be resolved"
    return 1
  fi

  log "Public edge RPC: $PUBLIC_EDGE_RPC_URL"
  check_public_preflight "$PUBLIC_EDGE_RPC_URL" "$PUBLIC_EDGE_ORIGIN" "Public RPC"
  check_public_rpc_method "$PUBLIC_EDGE_RPC_URL" "getHealth"
  check_public_rpc_method "$PUBLIC_EDGE_RPC_URL" "getIncidentStatus"
  check_public_rpc_method "$PUBLIC_EDGE_RPC_URL" "getSignedMetadataManifest"

  if [[ -n "$PUBLIC_EDGE_WS_URL" ]]; then
    check_public_websocket "$PUBLIC_EDGE_WS_URL"
  else
    fail "Public edge checks required but PUBLIC_EDGE_WS_URL could not be resolved"
  fi

  if [[ "$REQUIRE_FAUCET" == "1" ]]; then
    if [[ -n "$PUBLIC_EDGE_FAUCET_URL" ]]; then
      check_public_http_health "$PUBLIC_EDGE_FAUCET_URL" "Faucet"
    else
      fail "Public faucet checks required but PUBLIC_EDGE_FAUCET_URL was not set"
    fi
  fi

  if [[ "$REQUIRE_CUSTODY" == "1" ]]; then
    if [[ -n "$PUBLIC_EDGE_CUSTODY_URL" ]]; then
      check_public_http_health "$PUBLIC_EDGE_CUSTODY_URL" "Custody"
    else
      fail "Public custody checks required but PUBLIC_EDGE_CUSTODY_URL was not set"
    fi
  fi
}

wait_for_rpc_health() {
  local url="$1"
  local timeout_secs="${2:-25}"
  local started
  started="$(date +%s)"
  while true; do
    if curl -sS --max-time 3 -X POST "$url" \
      -H "Content-Type: application/json" \
      -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' | jq -e '.result' >/dev/null 2>&1; then
      return 0
    fi
    if (( $(date +%s) - started >= timeout_secs )); then
      return 1
    fi
    sleep 1
  done
}

ensure_secondary_validator() {
  local validator_num="$1"
  local rpc_port="$2"
  local rpc_url="http://localhost:${rpc_port}"

  if wait_for_rpc_health "$rpc_url" 3; then
    pass "Secondary validator healthy on :$rpc_port"
    return 0
  fi

  log "Secondary validator on :$rpc_port not healthy; attempting launch (validator $validator_num)"
  nohup env LICHEN_LOCAL_DEV=1 "$ROOT_DIR/run-validator.sh" testnet "$validator_num" >/tmp/e2e-validator-${validator_num}.log 2>&1 &
  sleep 2

  if wait_for_rpc_health "$rpc_url" 30; then
    pass "Secondary validator healthy on :$rpc_port (auto-started)"
    return 0
  fi

  fail "Secondary validator unhealthy on :$rpc_port"
  return 1
}

extract_wallet_address() {
  local wallet_name="$1"
  "$LICHEN_BIN" --rpc-url "$RPC_URL" wallet show "$wallet_name" 2>/dev/null | awk '/Address:/ {print $2}' | head -n1
}

wallet_keypair_path() {
  local wallet_name="$1"
  "$LICHEN_BIN" --rpc-url "$RPC_URL" wallet show "$wallet_name" 2>/dev/null | awk '/Path:/ {print $2}' | head -n1
}

wallet_signer_keypair_path() {
  local wallet_name="$1"
  local wallet_path
  wallet_path="$(wallet_keypair_path "$wallet_name")"
  if [[ -z "$wallet_path" || ! -f "$wallet_path" ]]; then
    echo ""
    return 0
  fi

  local tmp_path
  tmp_path="$(mktemp "/tmp/e2e-${wallet_name}-signer.XXXXXX")"

  if python3 - "$wallet_path" "$tmp_path" <<'PY'
import json,sys
src=sys.argv[1]
dst=sys.argv[2]
data=json.load(open(src,'r',encoding='utf-8'))
priv=data.get('privateKey')
pub=data.get('publicKey')

def hex_to_list(v):
    h=v.strip().lower()
    if h.startswith('0x'):
        h=h[2:]
    if len(h) % 2:
        raise ValueError('invalid hex length')
    return list(bytes.fromhex(h))

if isinstance(priv,str):
    priv = hex_to_list(priv)
if isinstance(pub,str):
    pub = hex_to_list(pub)

if not isinstance(priv,list) or len(priv) != 32:
    raise ValueError('unsupported wallet privateKey format')
if not isinstance(pub,list):
    pub = [0]*32

out={
    'privateKey': priv,
    'publicKey': pub,
    'publicKeyBase58': data.get('address','')
}
json.dump(out, open(dst,'w',encoding='utf-8'))
PY
  then
    echo "$tmp_path"
    return 0
  fi

  rm -f "$tmp_path" 2>/dev/null || true
  echo ""
}

keypair_address() {
  local keypair_path="$1"
  if [[ -z "$keypair_path" || ! -f "$keypair_path" ]]; then
    echo ""
    return 0
  fi

  python3 - "$keypair_path" <<'PY'
import json,sys

path = sys.argv[1]
try:
    data = json.load(open(path, 'r', encoding='utf-8'))
except Exception:
    print('')
    raise SystemExit(0)

for key in ('address', 'pubkey', 'publicKeyBase58'):
    value = data.get(key)
    if isinstance(value, str) and value.strip():
        print(value.strip())
        raise SystemExit(0)

print('')
PY
}

ensure_wallet() {
  local wallet_name="$1"
  if "$LICHEN_BIN" --rpc-url "$RPC_URL" wallet show "$wallet_name" >/dev/null 2>&1; then
    pass "Wallet exists: $wallet_name"
  else
    if "$LICHEN_BIN" --rpc-url "$RPC_URL" wallet create "$wallet_name" >/dev/null 2>&1; then
      pass "Created wallet: $wallet_name"
    else
      fail "Failed to create wallet: $wallet_name"
      return 1
    fi
  fi

  local wallet_path
  wallet_path="$(wallet_keypair_path "$wallet_name")"
  if [[ -z "$wallet_path" || ! -f "$wallet_path" ]]; then
    fail "Wallet keypair path missing: $wallet_name"
    return 1
  fi

  if "$LICHEN_BIN" --rpc-url "$RPC_URL" balance --keypair "$wallet_path" >/tmp/e2e-wallet-validate.log 2>&1; then
    pass "Wallet signer validated: $wallet_name"
  else
    log "Wallet signer for $wallet_name is stale or incompatible; recreating disposable E2E wallet"
    cat /tmp/e2e-wallet-validate.log >&2 || true
    "$LICHEN_BIN" --rpc-url "$RPC_URL" wallet remove "$wallet_name" >/dev/null 2>&1 || true
    rm -f "$wallet_path" >/dev/null 2>&1 || true

    if "$LICHEN_BIN" --rpc-url "$RPC_URL" wallet create "$wallet_name" >/dev/null 2>&1; then
      pass "Recreated wallet: $wallet_name"
    else
      fail "Failed to recreate wallet: $wallet_name"
      return 1
    fi
  fi

  local addr
  addr="$(extract_wallet_address "$wallet_name")"
  if [[ -n "$addr" ]]; then
    pass "Wallet address resolved: $wallet_name"
  else
    fail "Could not resolve address for wallet: $wallet_name"
    return 1
  fi
}

FUNDING_DEGRADED=0

fund_wallet_from_treasury() {
  local to_addr="$1"
  local amount_licn="$2"

  if [[ ! -f "$TREASURY_KEYPAIR" ]]; then
    # Airdrop has a max of 100 LICN, so cap the amount
    local airdrop_licn=$amount_licn
    if (( airdrop_licn > 100 )); then airdrop_licn=100; fi
    if rpc_has_result "requestAirdrop" "[\"$to_addr\", $airdrop_licn]"; then
      sleep 1
      pass "Airdropped $to_addr with ${airdrop_licn} LICN (treasury keypair unavailable)"
      return 0
    fi
    local amount_spores
    amount_spores="$($PYTHON_BIN - <<PY
amt=float("$amount_licn")
print(int(amt*1_000_000_000))
PY
)"
    if rpc_has_result "requestAirdrop" "[\"$to_addr\", $amount_spores]"; then
      sleep 1
      pass "Airdropped $to_addr with ${amount_spores} spores fallback (treasury keypair unavailable)"
      return 0
    fi
    FUNDING_DEGRADED=1
    if [[ "$STRICT_NO_SKIPS" == "1" ]]; then
      fail "Treasury keypair missing and airdrop fallback failed for $to_addr"
    else
      skip "Treasury keypair missing and airdrop fallback failed for $to_addr"
    fi
    return 1
  fi

  if "$LICHEN_BIN" --rpc-url "$RPC_URL" transfer "$to_addr" "$amount_licn" --keypair "$TREASURY_KEYPAIR" >/tmp/e2e-transfer.log 2>&1; then
    pass "Treasury funded $to_addr with ${amount_licn} LICN"
  else
    # Transfer failed, so fall back to airdrop instead of reviving legacy key conversion paths.
    local airdrop_licn=$amount_licn
    if (( airdrop_licn > 100 )); then airdrop_licn=100; fi
    if rpc_has_result "requestAirdrop" "[\"$to_addr\", $airdrop_licn]"; then
      sleep 1
      pass "Airdropped $to_addr with ${airdrop_licn} LICN (treasury transfer failed, airdrop fallback)"
    else
      cat /tmp/e2e-transfer.log >&2 || true
      FUNDING_DEGRADED=1
      if [[ "$STRICT_NO_SKIPS" == "1" ]]; then
        fail "Treasury transfer failed and airdrop fallback failed for $to_addr"
      else
        skip "Treasury transfer failed and airdrop fallback failed for $to_addr"
      fi
      return 1
    fi
  fi
}

get_balance_shells_with_retry() {
  local address="$1"
  local attempts="${2:-6}"
  local delay_secs="${3:-1}"
  local i=1
  while (( i <= attempts )); do
    local balance
    balance="$(rpc_call "getBalance" "[\"$address\"]" | python3 -c 'import json,sys
try:
 d=json.load(sys.stdin).get("result",0)
 if isinstance(d,dict):
  print(d.get("spendable", d.get("spores", d.get("balance",0))))
 else:
  print(d)
except Exception:
 print(0)
' 2>/dev/null || echo 0)"

    if [[ "$balance" =~ ^[0-9]+$ ]] && (( balance > 0 )); then
      echo "$balance"
      return 0
    fi

    sleep "$delay_secs"
    ((i++))
  done

  echo 0
  return 1
}

assert_balance_positive() {
  local address="$1"
  if (( FUNDING_DEGRADED == 1 )); then
    if [[ "$STRICT_NO_SKIPS" == "1" ]]; then
      fail "Balance check cannot proceed for $address (funding degraded)"
      return 1
    else
      skip "Balance check skipped for $address (funding degraded)"
      return 0
    fi
  fi
  local balance
  balance="$(get_balance_shells_with_retry "$address" 6 1 || echo 0)"

  if [[ "$balance" =~ ^[0-9]+$ ]] && (( balance > 0 )); then
    pass "Balance positive for $address"
  else
    fail "Balance check failed for $address"
    return 1
  fi
}

resolve_custody_withdrawal_fixtures() {
  if [[ "$RUN_CUSTODY_WITHDRAWAL_E2E" != "1" || "$REQUIRE_CUSTODY" != "1" ]]; then
    return 0
  fi

  local fixture_json
  fixture_json="$("$PYTHON_BIN" "$CUSTODY_WITHDRAWAL_FIXTURE_RESOLVER" 2>/dev/null || echo '{}')"

  CUSTODY_WITHDRAWAL_AUTH_TOKEN="$(echo "$fixture_json" | jq -r '.custody_api_auth_token // empty')"
  CUSTODY_WITHDRAWAL_GENESIS_KEYS_DIR="$(echo "$fixture_json" | jq -r '.genesis_keys_dir // empty')"

  local token_source genesis_source
  token_source="$(echo "$fixture_json" | jq -r '.custody_api_auth_token_source // "unresolved"')"
  genesis_source="$(echo "$fixture_json" | jq -r '.genesis_keys_source // "unresolved"')"

  if [[ -n "$CUSTODY_WITHDRAWAL_AUTH_TOKEN" && -n "$CUSTODY_WITHDRAWAL_GENESIS_KEYS_DIR" ]]; then
    pass "Custody withdrawal fixtures resolved"
    log "Custody withdrawal fixtures: token_source=$token_source genesis_source=$genesis_source"
    return 0
  fi

  fail "Custody withdrawal fixtures unresolved (token_source=$token_source genesis_source=$genesis_source)"
  return 1
}

stage_has_internal_failures() {
  local out_file="$1"
  grep -Eqi '(^[[:space:]]*❌[[:space:]]*FAIL([[:space:]]|$)|^[[:space:]]*✗[[:space:]]|❌[[:space:]]*FAILED:[[:space:]]*[1-9]|FAILED:[[:space:]]*[1-9]|[[:space:]][1-9][0-9]* failed([[:space:]]|,))' "$out_file"
}

stage_has_skip_failures() {
  local out_file="$1"
  grep -Eqi '(^[[:space:]]*SKIP[[:space:]]|^[[:space:]]*⊘[[:space:]]|⏭️|SKIPPED:[[:space:]]*[1-9]|[[:space:]][1-9][0-9]* skipped([[:space:]]|,))' "$out_file"
}

stage_has_transient_rpc_failures() {
  local out_file="$1"
  grep -Eqi '(all connection attempts failed|rpc transport error|validator unreachable|cannot reach validator|fetch failed|connection refused|connection reset|timed out|timeout|server disconnected|failed to connect)' "$out_file"
}

run_script_stage() {
  local name="$1"
  local cmd="$2"
  local out_file=""
  local allow_relaxed_skips=0
  local attempt=1
  local max_attempts=2
  local stage_exit=0
  local has_internal_failures=1
  local has_skip_failures=1
  local has_transient_failures=1

  if [[ "$name" == "Contract write scenarios" && "$STRICT_WRITE_ASSERTIONS" != "1" && "$REQUIRE_FULL_WRITE_ACTIVITY" != "1" ]]; then
    allow_relaxed_skips=1
  fi

  if [[ "$name" == "Launchpad user flows" ]]; then
    allow_relaxed_skips=1
  fi

  while (( attempt <= max_attempts )); do
    out_file="$(mktemp)"

    if ! wait_for_rpc_health "$RPC_URL" 45; then
      log "RPC not healthy before stage attempt $attempt: $name"
    fi

    if (( attempt > 1 )); then
      log "Running stage: $name (retry $attempt/$max_attempts)"
    else
      log "Running stage: $name"
    fi

    if bash -lc "$cmd" 2>&1 | tee "$out_file"; then
      stage_exit=0
    else
      stage_exit=$?
    fi

    if stage_has_internal_failures "$out_file"; then
      has_internal_failures=0
    else
      has_internal_failures=1
    fi

    has_skip_failures=1
    if [[ "$STRICT_NO_SKIPS" == "1" && "$allow_relaxed_skips" != "1" ]]; then
      if stage_has_skip_failures "$out_file"; then
        has_skip_failures=0
      fi
    fi

    if stage_has_transient_rpc_failures "$out_file"; then
      has_transient_failures=0
    else
      has_transient_failures=1
    fi

    if (( attempt < max_attempts )) && [[ $has_transient_failures -eq 0 ]] && { [[ $stage_exit -ne 0 ]] || [[ $has_internal_failures -eq 0 ]]; }; then
      log "Transient RPC failure detected in stage '$name'; waiting for recovery before retry"
      wait_for_rpc_health "$RPC_URL" 60 || true
      rm -f "$out_file"
      attempt=$((attempt + 1))
      continue
    fi

    break
  done

  if [[ $stage_exit -eq 0 ]]; then
    pass "Stage passed: $name"
  else
    fail "Stage failed: $name"
  fi

  if [[ $has_internal_failures -eq 0 ]]; then
    fail "Stage reported internal failures: $name"
  fi

  if [[ "$STRICT_NO_SKIPS" == "1" && "$allow_relaxed_skips" != "1" && $has_skip_failures -eq 0 ]]; then
    fail "Stage contains skips in strict mode: $name"
  fi

  rm -f "$out_file"
}

print_summary_and_exit() {
  echo
  echo "============================================================"
  echo "Production E2E Gate Summary"
  echo "============================================================"
  echo "PASS: $PASS"
  echo "FAIL: $FAIL"
  echo "SKIP: $SKIP"

  if (( ${#ERRORS[@]} > 0 )); then
    echo
    echo "Failures:"
    for err in "${ERRORS[@]}"; do
      echo "- $err"
    done
  fi

  if (( FAIL > 0 )); then
    exit 1
  fi

  if [[ "$STRICT_NO_SKIPS" == "1" ]] && (( SKIP > 0 )); then
    exit 1
  fi

  exit 0
}

log "Starting production E2E gate"
log "RPC: $RPC_URL"
log "WS:  $WS_URL"

require_cmd curl || true
require_cmd jq || true
if [[ -x "$PYTHON_BIN" ]]; then
  pass "Python runtime available: $PYTHON_BIN"
else
  PYTHON_BIN="python3"
  require_cmd "$PYTHON_BIN" || true
fi

if [[ ! -x "$LICHEN_BIN" ]]; then
  log "Building CLI binary (lichen)"
  if cargo build --release --bin lichen >/dev/null 2>&1; then
    pass "Built CLI binary"
  else
    fail "Failed to build CLI binary"
    print_summary_and_exit
  fi
fi

resolve_public_edge_defaults

if rpc_has_result "getHealth" "[]"; then
  pass "Primary RPC healthy"
else
  fail "Primary RPC unhealthy at $RPC_URL"
  print_summary_and_exit
fi

python_can_import_modules() {
  local pybin="$1"
  "$pybin" -c "import httpx,base58,websockets" >/dev/null 2>&1
}

for candidate in "$PYTHON_BIN" "$ROOT_DIR/sdk/python/venv/bin/python" "python3"; do
  if [[ -x "$candidate" ]] && python_can_import_modules "$candidate"; then
    PYTHON_BIN="$candidate"
    break
  fi
done

if ! python_can_import_modules "$PYTHON_BIN"; then
  if "$PYTHON_BIN" -m pip install -q httpx base58 websockets >/dev/null 2>&1 && python_can_import_modules "$PYTHON_BIN"; then
    pass "Installed Python deps for write scenarios"
  else
    fail "Python runtime missing required modules (httpx/base58/websockets): $PYTHON_BIN"
  fi
fi

run_public_edge_checks

if [[ "$REQUIRE_CUSTODY" == "1" && "$RUN_CUSTODY_WITHDRAWAL_E2E" == "1" ]]; then
  if ! resolve_custody_withdrawal_fixtures; then
    RUN_CUSTODY_WITHDRAWAL_E2E=0
  fi
fi

if [[ "$REQUIRE_MULTI_VALIDATOR" == "1" ]]; then
  ensure_secondary_validator 2 8901 || true
  ensure_secondary_validator 3 8903 || true
fi

ensure_wallet "$AGENT_WALLET_NAME" || true
ensure_wallet "$HUMAN_WALLET_NAME" || true

AGENT_ADDR="$(extract_wallet_address "$AGENT_WALLET_NAME" || true)"
HUMAN_ADDR="$(extract_wallet_address "$HUMAN_WALLET_NAME" || true)"
AGENT_KEYPAIR=""
HUMAN_KEYPAIR=""
CONTRACT_WRITE_SIGNER=""
AGENT_WALLET_PATH="$(wallet_keypair_path "$AGENT_WALLET_NAME" || true)"
HUMAN_WALLET_PATH="$(wallet_keypair_path "$HUMAN_WALLET_NAME" || true)"

if [[ -n "$AGENT_WALLET_PATH" && -f "$AGENT_WALLET_PATH" ]]; then
  AGENT_KEYPAIR="$AGENT_WALLET_PATH"
  pass "Using wallet keypair path for agent signer"
fi

if [[ -n "$HUMAN_WALLET_PATH" && -f "$HUMAN_WALLET_PATH" ]]; then
  HUMAN_KEYPAIR="$HUMAN_WALLET_PATH"
  pass "Using wallet keypair path for human signer"
fi

CONTRACT_WRITE_SIGNER="$AGENT_KEYPAIR"
if [[ -n "$CONTRACT_WRITE_KEYPAIR" && -f "$CONTRACT_WRITE_KEYPAIR" ]]; then
  CONTRACT_WRITE_SIGNER="$CONTRACT_WRITE_KEYPAIR"
  pass "Using contract write signer: $CONTRACT_WRITE_SIGNER"
fi

CONTRACT_WRITE_ADDR=""
if [[ -n "$CONTRACT_WRITE_SIGNER" && -f "$CONTRACT_WRITE_SIGNER" ]]; then
  CONTRACT_WRITE_ADDR="$(keypair_address "$CONTRACT_WRITE_SIGNER" || true)"
fi

if [[ -n "$AGENT_ADDR" && -n "$HUMAN_ADDR" ]]; then
  fund_wallet_from_treasury "$AGENT_ADDR" "$TREASURY_FUND_LICN" || true
  fund_wallet_from_treasury "$HUMAN_ADDR" "$TREASURY_FUND_LICN" || true
  assert_balance_positive "$AGENT_ADDR" || true
  assert_balance_positive "$HUMAN_ADDR" || true
else
  fail "Actor wallet addresses not resolved"
fi

if [[ -n "$CONTRACT_WRITE_ADDR" ]]; then
  if [[ "$CONTRACT_WRITE_ADDR" != "$AGENT_ADDR" && "$CONTRACT_WRITE_ADDR" != "$HUMAN_ADDR" ]]; then
    fund_wallet_from_treasury "$CONTRACT_WRITE_ADDR" "$TREASURY_FUND_LICN" || true
  fi

  if ! assert_balance_positive "$CONTRACT_WRITE_ADDR"; then
    if [[ -n "$AGENT_KEYPAIR" && -f "$AGENT_KEYPAIR" ]]; then
      CONTRACT_WRITE_SIGNER="$AGENT_KEYPAIR"
      pass "Contract write signer fallback to funded agent keypair"
    fi
  fi
else
  if [[ -n "$AGENT_KEYPAIR" && -f "$AGENT_KEYPAIR" ]]; then
    CONTRACT_WRITE_SIGNER="$AGENT_KEYPAIR"
    pass "Contract write signer fallback to agent keypair (address unresolved)"
  fi
fi

if [[ -n "$AGENT_ADDR" && -n "$HUMAN_ADDR" && -f "$AGENT_KEYPAIR" && $FUNDING_DEGRADED -eq 0 ]]; then
  if "$LICHEN_BIN" --rpc-url "$RPC_URL" transfer "$HUMAN_ADDR" 1 --keypair "$AGENT_KEYPAIR" >/tmp/e2e-actor-transfer.log 2>&1; then
    pass "Agent -> human transfer succeeded"
  else
    cat /tmp/e2e-actor-transfer.log >&2 || true
    skip "Agent -> human transfer failed"
  fi
else
  skip "Cannot run actor transfer scenario"
fi

if [[ -z "$AGENT_ADDR" || -z "$HUMAN_ADDR" || -z "$AGENT_KEYPAIR" || ! -f "$AGENT_KEYPAIR" || $FUNDING_DEGRADED -eq 1 ]]; then
  REQUIRE_TOKEN_WRITE=0
  RUN_CONTRACT_WRITE_SUITE=0
  skip "Token write path disabled due missing actor/funding prerequisites"
fi

run_script_stage "RPC comprehensive" "cd '$ROOT_DIR' && bash tests/test-rpc-comprehensive.sh"
run_script_stage "WebSocket comprehensive" "cd '$ROOT_DIR' && bash tests/test-websocket.sh"
if [[ "$REQUIRE_MULTI_VALIDATOR" == "1" ]]; then
  run_script_stage "Live multi-validator E2E" "cd '$ROOT_DIR' && LICHENID_G_PHASE_WRITE_TESTS=1 bash tests/live-e2e-test.sh"
else
  skip "Live multi-validator E2E disabled (set REQUIRE_MULTI_VALIDATOR=1 to enable)"
fi
if [[ "$RUN_DEEP_SERVICES_SUITE" == "1" ]]; then
  run_script_stage "Deep services E2E" "cd '$ROOT_DIR' && ROOT_DIR='$ROOT_DIR' RPC_URL='$RPC_URL' LICHEN_BIN='$LICHEN_BIN' AGENT_KEYPAIR='$AGENT_KEYPAIR' HUMAN_ADDR='$HUMAN_ADDR' DEX_API_URL='$DEX_API_URL' FAUCET_URL='$FAUCET_URL' CUSTODY_URL='$CUSTODY_URL' REQUIRE_DEX_API='$REQUIRE_DEX_API' REQUIRE_FAUCET='$REQUIRE_FAUCET' REQUIRE_CUSTODY='$REQUIRE_CUSTODY' REQUIRE_LAUNCHPAD='$REQUIRE_LAUNCHPAD' REQUIRE_TOKEN_WRITE='$REQUIRE_TOKEN_WRITE' REQUIRE_ALL_CONTRACTS='$REQUIRE_ALL_CONTRACTS' DEX_BOOTSTRAP_BASE_SYMBOL='$DEX_BOOTSTRAP_BASE_SYMBOL' DEX_BOOTSTRAP_QUOTE_SYMBOL='$DEX_BOOTSTRAP_QUOTE_SYMBOL' bash tests/services-deep-e2e.sh"
else
  skip "Deep services E2E disabled (set RUN_DEEP_SERVICES_SUITE=1 to enable)"
fi
run_script_stage "User services E2E" "cd '$ROOT_DIR' && RPC_URL='$RPC_URL' FAUCET_URL='$FAUCET_URL' CUSTODY_URL='$CUSTODY_URL' AGENT_KEYPAIR='$AGENT_KEYPAIR' REQUIRE_FAUCET='$REQUIRE_FAUCET' REQUIRE_CUSTODY='$REQUIRE_CUSTODY' STRICT_NO_SKIPS='$STRICT_NO_SKIPS' '$PYTHON_BIN' tests/e2e-user-services.py"
if [[ "$RUN_CUSTODY_WITHDRAWAL_E2E" == "1" && "$REQUIRE_CUSTODY" == "1" ]]; then
  run_script_stage "Custody withdrawal E2E" "cd '$ROOT_DIR' && RPC_URL='$RPC_URL' CUSTODY_URL='$CUSTODY_URL' CUSTODY_API_AUTH_TOKEN='$CUSTODY_WITHDRAWAL_AUTH_TOKEN' GENESIS_KEYS_DIR='$CUSTODY_WITHDRAWAL_GENESIS_KEYS_DIR' '$PYTHON_BIN' tests/e2e-custody-withdrawal.py"
fi
run_script_stage "Portal interaction flows" "cd '$ROOT_DIR' && node tests/e2e-portal-interactions.js"
run_script_stage "Frontend asset integrity" "cd '$ROOT_DIR' && node tests/test_frontend_asset_integrity.js"
run_script_stage "Frontend trust boundaries" "cd '$ROOT_DIR' && node tests/test_frontend_trust_boundaries.js"
run_script_stage "Explorer regression suite" "cd '$ROOT_DIR' && node explorer/explorer.test.js"
run_script_stage "Wallet user flows" "cd '$ROOT_DIR' && RPC_URL='$RPC_URL' FAUCET_URL='$FAUCET_URL' node tests/e2e-wallet-flows.js"
run_script_stage "Developer lifecycle" "cd '$ROOT_DIR' && RPC_URL='$RPC_URL' FAUCET_URL='$FAUCET_URL' AGENT_KEYPAIR='$AGENT_KEYPAIR' '$PYTHON_BIN' tests/e2e-developer-lifecycle.py"
run_script_stage "Launchpad user flows" "cd '$ROOT_DIR' && RPC_URL='$RPC_URL' FAUCET_URL='$FAUCET_URL' node tests/e2e-launchpad.js"
if [[ "$RUN_CONTRACT_WRITE_SUITE" == "1" ]]; then
  run_script_stage "Contract write scenarios" "cd '$ROOT_DIR' && PYTHONPATH='$ROOT_DIR/sdk/python' RPC_URL='$RPC_URL' AGENT_KEYPAIR='$CONTRACT_WRITE_SIGNER' HUMAN_KEYPAIR='$HUMAN_KEYPAIR' REQUIRE_ALL_SCENARIOS='$REQUIRE_ALL_SCENARIOS' STRICT_WRITE_ASSERTIONS='$STRICT_WRITE_ASSERTIONS' TX_CONFIRM_TIMEOUT_SECS='$TX_CONFIRM_TIMEOUT_SECS' REQUIRE_FULL_WRITE_ACTIVITY='$REQUIRE_FULL_WRITE_ACTIVITY' MIN_CONTRACT_ACTIVITY_DELTA='$MIN_CONTRACT_ACTIVITY_DELTA' CONTRACT_ACTIVITY_OVERRIDES='$CONTRACT_ACTIVITY_OVERRIDES' ENFORCE_DOMAIN_ASSERTIONS='$ENFORCE_DOMAIN_ASSERTIONS' ENABLE_NEGATIVE_ASSERTIONS='$ENABLE_NEGATIVE_ASSERTIONS' REQUIRE_NEGATIVE_REASON_MATCH='$REQUIRE_NEGATIVE_REASON_MATCH' REQUIRE_NEGATIVE_CODE_MATCH='$REQUIRE_NEGATIVE_CODE_MATCH' REQUIRE_SCENARIO_FOR_DISCOVERED='$REQUIRE_SCENARIO_FOR_DISCOVERED' MIN_NEGATIVE_ASSERTIONS_EXECUTED='$MIN_NEGATIVE_ASSERTIONS_EXECUTED' REQUIRE_EXPECTED_CONTRACT_SET='$REQUIRE_EXPECTED_CONTRACT_SET' EXPECTED_CONTRACTS_FILE='$EXPECTED_CONTRACTS_FILE' WRITE_E2E_REPORT_PATH='$WRITE_E2E_REPORT_PATH' '$PYTHON_BIN' tests/contracts-write-e2e.py"
else
  skip "Contract write scenario suite disabled (set RUN_CONTRACT_WRITE_SUITE=1 to enable)"
fi
run_script_stage "Contract deployment pipeline" "cd '$ROOT_DIR' && bash tests/test-contract-deployment.sh"
run_script_stage "CLI comprehensive" "cd '$ROOT_DIR' && bash tests/test-cli-comprehensive.sh"

if [[ "$RUN_SDK_SUITE" == "1" ]]; then
  run_script_stage "SDK full matrix" "cd '$ROOT_DIR' && bash scripts/test-all-sdks.sh"
else
  skip "SDK full matrix disabled (set RUN_SDK_SUITE=1 to enable)"
fi

print_summary_and_exit
