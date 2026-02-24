#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

MOLT_BIN="${MOLT_BIN:-$ROOT_DIR/target/release/molt}"
RPC_URL="${RPC_URL:-http://localhost:8899}"
WS_URL="${WS_URL:-ws://localhost:8900}"
TREASURY_KEYPAIR="${TREASURY_KEYPAIR:-}"
if [[ -z "$TREASURY_KEYPAIR" || ! -f "$TREASURY_KEYPAIR" ]]; then
  # Auto-discover treasury keypair from any state directory
  TREASURY_KEYPAIR="$(find "$ROOT_DIR/data" -name 'treasury-moltchain-testnet-1.json' -type f 2>/dev/null | head -1 || true)"
fi
REQUIRE_MULTI_VALIDATOR="${REQUIRE_MULTI_VALIDATOR:-1}"
STRICT_NO_SKIPS="${STRICT_NO_SKIPS:-1}"
RUN_SDK_SUITE="${RUN_SDK_SUITE:-1}"
RUN_DEEP_SERVICES_SUITE="${RUN_DEEP_SERVICES_SUITE:-1}"
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
ENFORCE_DOMAIN_ASSERTIONS="${ENFORCE_DOMAIN_ASSERTIONS:-0}"
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
DEX_BOOTSTRAP_BASE_SYMBOL="${DEX_BOOTSTRAP_BASE_SYMBOL-MOLT}"
DEX_BOOTSTRAP_QUOTE_SYMBOL="${DEX_BOOTSTRAP_QUOTE_SYMBOL-MUSD}"
DEX_API_URL="${DEX_API_URL:-${RPC_URL}/api/v1}"
FAUCET_URL="${FAUCET_URL:-http://localhost:9100}"
CUSTODY_URL="${CUSTODY_URL:-http://localhost:9105}"
PYTHON_BIN="${PYTHON_BIN:-$ROOT_DIR/.venv/bin/python}"

AGENT_WALLET_NAME="${AGENT_WALLET_NAME:-e2e-agent}"
HUMAN_WALLET_NAME="${HUMAN_WALLET_NAME:-e2e-human}"
TREASURY_FUND_MOLT="${TREASURY_FUND_MOLT:-1000}"
SIGNER_KEYPAIR="${SIGNER_KEYPAIR:-$HOME/.moltchain/keypairs/id.json}"

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

wait_for_rpc_health() {
  local url="$1"
  local timeout_secs="${2:-25}"
  local started
  started="$(date +%s)"
  while true; do
    if curl -sS --max-time 3 -X POST "$url" \
      -H "Content-Type: application/json" \
      -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}' | jq -e '.result' >/dev/null 2>&1; then
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
  nohup "$ROOT_DIR/run-validator.sh" testnet "$validator_num" >/tmp/e2e-validator-${validator_num}.log 2>&1 &
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
  "$MOLT_BIN" --rpc-url "$RPC_URL" wallet show "$wallet_name" 2>/dev/null | awk '/Address:/ {print $2}' | head -n1
}

wallet_keypair_path() {
  local wallet_name="$1"
  "$MOLT_BIN" --rpc-url "$RPC_URL" wallet show "$wallet_name" 2>/dev/null | awk '/Path:/ {print $2}' | head -n1
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
  if "$MOLT_BIN" --rpc-url "$RPC_URL" wallet show "$wallet_name" >/dev/null 2>&1; then
    pass "Wallet exists: $wallet_name"
  else
    if "$MOLT_BIN" --rpc-url "$RPC_URL" wallet create "$wallet_name" >/dev/null 2>&1; then
      pass "Created wallet: $wallet_name"
    else
      fail "Failed to create wallet: $wallet_name"
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

convert_secret_keypair_for_cli() {
  local src="$1"
  local dst="$2"
  python3 - "$src" "$dst" <<'PY'
import json, sys
from nacl.signing import SigningKey

src, dst = sys.argv[1], sys.argv[2]
data = json.load(open(src, 'r', encoding='utf-8'))
secret = data.get('secret_key')
if not isinstance(secret, str):
  raise SystemExit(1)
secret = secret.strip().lower().removeprefix('0x')
if len(secret) != 64:
  raise SystemExit(1)
seed = bytes.fromhex(secret)
sk = SigningKey(seed)
pk = bytes(sk.verify_key)
out = {
  'privateKey': list(seed),
  'publicKey': list(pk),
  'secretKey': list(seed + pk),
  'address': data.get('pubkey', '')
}
json.dump(out, open(dst, 'w', encoding='utf-8'))
PY
}

fund_wallet_from_treasury() {
  local to_addr="$1"
  local amount_molt="$2"

  if [[ ! -f "$TREASURY_KEYPAIR" ]]; then
    # Airdrop has a max of 100 MOLT, so cap the amount
    local airdrop_molt=$amount_molt
    if (( airdrop_molt > 100 )); then airdrop_molt=100; fi
    if rpc_has_result "requestAirdrop" "[\"$to_addr\", $airdrop_molt]"; then
      sleep 1
      pass "Airdropped $to_addr with ${airdrop_molt} MOLT (treasury keypair unavailable)"
      return 0
    fi
    local amount_shells
    amount_shells="$($PYTHON_BIN - <<PY
amt=float("$amount_molt")
print(int(amt*1_000_000_000))
PY
)"
    if rpc_has_result "requestAirdrop" "[\"$to_addr\", $amount_shells]"; then
      sleep 1
      pass "Airdropped $to_addr with ${amount_shells} shells fallback (treasury keypair unavailable)"
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

  if "$MOLT_BIN" --rpc-url "$RPC_URL" transfer "$to_addr" "$amount_molt" --keypair "$TREASURY_KEYPAIR" >/tmp/e2e-transfer.log 2>&1; then
    pass "Treasury funded $to_addr with ${amount_molt} MOLT"
  else
    if grep -qi 'Unsupported keypair format' /tmp/e2e-transfer.log; then
      local converted_keypair
      converted_keypair="$(mktemp -t e2e-treasury-cli)"
      if convert_secret_keypair_for_cli "$TREASURY_KEYPAIR" "$converted_keypair" >/dev/null 2>&1; then
        if "$MOLT_BIN" --rpc-url "$RPC_URL" transfer "$to_addr" "$amount_molt" --keypair "$converted_keypair" >/tmp/e2e-transfer.log 2>&1; then
          pass "Treasury funded $to_addr with ${amount_molt} MOLT (converted keypair format)"
          rm -f "$converted_keypair" >/dev/null 2>&1 || true
          return 0
        fi
      fi
      rm -f "$converted_keypair" >/dev/null 2>&1 || true
    fi
    # Transfer failed (keypair format mismatch etc.), fall back to airdrop
    local airdrop_molt=$amount_molt
    if (( airdrop_molt > 100 )); then airdrop_molt=100; fi
    if rpc_has_result "requestAirdrop" "[\"$to_addr\", $airdrop_molt]"; then
      sleep 1
      pass "Airdropped $to_addr with ${airdrop_molt} MOLT (treasury transfer failed, airdrop fallback)"
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
  print(d.get("spendable", d.get("shells", d.get("balance",0))))
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

run_script_stage() {
  local name="$1"
  local cmd="$2"
  local out_file
  out_file="$(mktemp)"

  log "Running stage: $name"
  if bash -lc "$cmd" | tee "$out_file"; then
    pass "Stage passed: $name"
  else
    fail "Stage failed: $name"
  fi

  if grep -Eq '(^[[:space:]]*❌[[:space:]]*FAIL([[:space:]]|$)|❌[[:space:]]*FAILED:[[:space:]]*[1-9]|FAILED:[[:space:]]*[1-9])' "$out_file"; then
    fail "Stage reported internal failures: $name"
  fi

  if [[ "$STRICT_NO_SKIPS" == "1" ]]; then
    if grep -Eq '(^[[:space:]]*SKIP[[:space:]]|⏭️|SKIPPED:[[:space:]]*[1-9])' "$out_file"; then
      fail "Stage contains skips in strict mode: $name"
    fi
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

if [[ ! -x "$MOLT_BIN" ]]; then
  log "Building CLI binary (molt)"
  if cargo build --release --bin molt >/dev/null 2>&1; then
    pass "Built CLI binary"
  else
    fail "Failed to build CLI binary"
    print_summary_and_exit
  fi
fi

if rpc_has_result "health" "[]"; then
  pass "Primary RPC healthy"
else
  fail "Primary RPC unhealthy at $RPC_URL"
  print_summary_and_exit
fi

python_can_import_modules() {
  local pybin="$1"
  "$pybin" -c "import httpx,base58,nacl,websockets" >/dev/null 2>&1
}

for candidate in "$PYTHON_BIN" "$ROOT_DIR/sdk/python/venv/bin/python" "python3"; do
  if [[ -x "$candidate" ]] && python_can_import_modules "$candidate"; then
    PYTHON_BIN="$candidate"
    break
  fi
done

if ! python_can_import_modules "$PYTHON_BIN"; then
  if "$PYTHON_BIN" -m pip install -q httpx base58 pynacl websockets >/dev/null 2>&1 && python_can_import_modules "$PYTHON_BIN"; then
    pass "Installed Python deps for write scenarios"
  else
    fail "Python runtime missing required modules (httpx/base58/pynacl/websockets): $PYTHON_BIN"
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

if [[ -f "$SIGNER_KEYPAIR" ]]; then
  AGENT_KEYPAIR="$SIGNER_KEYPAIR"
else
  AGENT_KEYPAIR="$(wallet_signer_keypair_path "$AGENT_WALLET_NAME" || true)"
fi

if [[ -z "$AGENT_KEYPAIR" || ! -f "$AGENT_KEYPAIR" ]]; then
  if [[ -n "$AGENT_WALLET_PATH" && -f "$AGENT_WALLET_PATH" ]]; then
    AGENT_KEYPAIR="$AGENT_WALLET_PATH"
    pass "Using wallet keypair path for agent signer"
  fi
fi

HUMAN_KEYPAIR="$(wallet_signer_keypair_path "$HUMAN_WALLET_NAME" || true)"
if [[ -z "$HUMAN_KEYPAIR" || ! -f "$HUMAN_KEYPAIR" ]]; then
  local_human_wallet_path="$(wallet_keypair_path "$HUMAN_WALLET_NAME" || true)"
  if [[ -n "$local_human_wallet_path" && -f "$local_human_wallet_path" ]]; then
    HUMAN_KEYPAIR="$local_human_wallet_path"
    pass "Using wallet keypair path for human signer"
  fi
fi

if [[ -z "$HUMAN_KEYPAIR" || ! -f "$HUMAN_KEYPAIR" ]]; then
  if [[ -n "$AGENT_KEYPAIR" && -f "$AGENT_KEYPAIR" ]]; then
    HUMAN_KEYPAIR="$AGENT_KEYPAIR"
    pass "Human signer fallback to agent keypair"
  fi
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
  fund_wallet_from_treasury "$AGENT_ADDR" "$TREASURY_FUND_MOLT" || true
  fund_wallet_from_treasury "$HUMAN_ADDR" "$TREASURY_FUND_MOLT" || true
  assert_balance_positive "$AGENT_ADDR" || true
  assert_balance_positive "$HUMAN_ADDR" || true
else
  fail "Actor wallet addresses not resolved"
fi

if [[ -n "$CONTRACT_WRITE_ADDR" ]]; then
  if [[ "$CONTRACT_WRITE_ADDR" != "$AGENT_ADDR" && "$CONTRACT_WRITE_ADDR" != "$HUMAN_ADDR" ]]; then
    fund_wallet_from_treasury "$CONTRACT_WRITE_ADDR" "$TREASURY_FUND_MOLT" || true
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
  if "$MOLT_BIN" --rpc-url "$RPC_URL" transfer "$HUMAN_ADDR" 1 --keypair "$AGENT_KEYPAIR" >/tmp/e2e-actor-transfer.log 2>&1; then
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

run_script_stage "RPC comprehensive" "cd '$ROOT_DIR' && bash test-rpc-comprehensive.sh"
run_script_stage "WebSocket comprehensive" "cd '$ROOT_DIR' && bash test-websocket.sh"
if [[ "$REQUIRE_MULTI_VALIDATOR" == "1" ]]; then
  run_script_stage "Live multi-validator E2E" "cd '$ROOT_DIR' && MOLTYID_G_PHASE_WRITE_TESTS=1 bash tests/live-e2e-test.sh"
else
  skip "Live multi-validator E2E disabled (set REQUIRE_MULTI_VALIDATOR=1 to enable)"
fi
if [[ "$RUN_DEEP_SERVICES_SUITE" == "1" ]]; then
  run_script_stage "Deep services E2E" "cd '$ROOT_DIR' && ROOT_DIR='$ROOT_DIR' RPC_URL='$RPC_URL' MOLT_BIN='$MOLT_BIN' AGENT_KEYPAIR='$AGENT_KEYPAIR' HUMAN_ADDR='$HUMAN_ADDR' DEX_API_URL='$DEX_API_URL' FAUCET_URL='$FAUCET_URL' CUSTODY_URL='$CUSTODY_URL' REQUIRE_DEX_API='$REQUIRE_DEX_API' REQUIRE_FAUCET='$REQUIRE_FAUCET' REQUIRE_CUSTODY='$REQUIRE_CUSTODY' REQUIRE_LAUNCHPAD='$REQUIRE_LAUNCHPAD' REQUIRE_TOKEN_WRITE='$REQUIRE_TOKEN_WRITE' REQUIRE_ALL_CONTRACTS='$REQUIRE_ALL_CONTRACTS' DEX_BOOTSTRAP_BASE_SYMBOL='$DEX_BOOTSTRAP_BASE_SYMBOL' DEX_BOOTSTRAP_QUOTE_SYMBOL='$DEX_BOOTSTRAP_QUOTE_SYMBOL' bash tests/services-deep-e2e.sh"
else
  skip "Deep services E2E disabled (set RUN_DEEP_SERVICES_SUITE=1 to enable)"
fi
if [[ "$RUN_CONTRACT_WRITE_SUITE" == "1" ]]; then
  run_script_stage "Contract write scenarios" "cd '$ROOT_DIR' && PYTHONPATH='$ROOT_DIR/sdk/python' RPC_URL='$RPC_URL' AGENT_KEYPAIR='$CONTRACT_WRITE_SIGNER' HUMAN_KEYPAIR='$HUMAN_KEYPAIR' REQUIRE_ALL_SCENARIOS='$REQUIRE_ALL_SCENARIOS' STRICT_WRITE_ASSERTIONS='$STRICT_WRITE_ASSERTIONS' TX_CONFIRM_TIMEOUT_SECS='$TX_CONFIRM_TIMEOUT_SECS' REQUIRE_FULL_WRITE_ACTIVITY='$REQUIRE_FULL_WRITE_ACTIVITY' MIN_CONTRACT_ACTIVITY_DELTA='$MIN_CONTRACT_ACTIVITY_DELTA' CONTRACT_ACTIVITY_OVERRIDES='$CONTRACT_ACTIVITY_OVERRIDES' ENFORCE_DOMAIN_ASSERTIONS='$ENFORCE_DOMAIN_ASSERTIONS' ENABLE_NEGATIVE_ASSERTIONS='$ENABLE_NEGATIVE_ASSERTIONS' REQUIRE_NEGATIVE_REASON_MATCH='$REQUIRE_NEGATIVE_REASON_MATCH' REQUIRE_NEGATIVE_CODE_MATCH='$REQUIRE_NEGATIVE_CODE_MATCH' REQUIRE_SCENARIO_FOR_DISCOVERED='$REQUIRE_SCENARIO_FOR_DISCOVERED' MIN_NEGATIVE_ASSERTIONS_EXECUTED='$MIN_NEGATIVE_ASSERTIONS_EXECUTED' REQUIRE_EXPECTED_CONTRACT_SET='$REQUIRE_EXPECTED_CONTRACT_SET' EXPECTED_CONTRACTS_FILE='$EXPECTED_CONTRACTS_FILE' WRITE_E2E_REPORT_PATH='$WRITE_E2E_REPORT_PATH' '$PYTHON_BIN' tests/contracts-write-e2e.py"
else
  skip "Contract write scenario suite disabled (set RUN_CONTRACT_WRITE_SUITE=1 to enable)"
fi
run_script_stage "Contract deployment pipeline" "cd '$ROOT_DIR' && bash test-contract-deployment.sh"
run_script_stage "CLI comprehensive" "cd '$ROOT_DIR' && bash test-cli-comprehensive.sh"

if [[ "$RUN_SDK_SUITE" == "1" ]]; then
  run_script_stage "SDK full matrix" "cd '$ROOT_DIR' && bash scripts/test-all-sdks.sh"
else
  skip "SDK full matrix disabled (set RUN_SDK_SUITE=1 to enable)"
fi

print_summary_and_exit
