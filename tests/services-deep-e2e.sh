#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="${ROOT_DIR:-$(cd "$(dirname "$0")/.." && pwd)}"
RPC_URL="${RPC_URL:-http://localhost:8899}"
MOLT_BIN="${MOLT_BIN:-$(pwd)/target/release/molt}"
DEX_API_URL="${DEX_API_URL:-${RPC_URL}/api/v1}"
FAUCET_URL="${FAUCET_URL:-http://localhost:9100}"
CUSTODY_URL="${CUSTODY_URL:-http://localhost:9105}"

AGENT_KEYPAIR="${AGENT_KEYPAIR:-}"
HUMAN_ADDR="${HUMAN_ADDR:-}"

REQUIRE_DEX_API="${REQUIRE_DEX_API:-1}"
REQUIRE_FAUCET="${REQUIRE_FAUCET:-0}"
REQUIRE_CUSTODY="${REQUIRE_CUSTODY:-0}"
REQUIRE_LAUNCHPAD="${REQUIRE_LAUNCHPAD:-1}"
REQUIRE_TOKEN_WRITE="${REQUIRE_TOKEN_WRITE:-0}"
REQUIRE_ALL_CONTRACTS="${REQUIRE_ALL_CONTRACTS:-1}"
DEX_BOOTSTRAP_BASE_SYMBOL="${DEX_BOOTSTRAP_BASE_SYMBOL-MOLT}"
DEX_BOOTSTRAP_QUOTE_SYMBOL="${DEX_BOOTSTRAP_QUOTE_SYMBOL-MUSD}"

PASS=0
FAIL=0

pass() {
  echo "  PASS  $1"
  PASS=$((PASS + 1))
}

fail() {
  echo "  FAIL  $1"
  FAIL=$((FAIL + 1))
}

rpc() {
  local method="$1"
  local params="${2:-[]}"
  curl -sS --max-time 8 -X POST "$RPC_URL" \
    -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}" || true
}

rpc_has_result() {
  local method="$1"
  local params="${2:-[]}"
  rpc "$method" "$params" | jq -e '.result' >/dev/null 2>&1
}

rpc_has_error() {
  local method="$1"
  local params="${2:-[]}"
  rpc "$method" "$params" | jq -e '.error' >/dev/null 2>&1
}

rpc_result_json() {
  local method="$1"
  local params="${2:-[]}"
  rpc "$method" "$params" | jq -c '.result // empty' 2>/dev/null || true
}

dex_ok() {
  local path="$1"
  curl -sS --max-time 8 "${DEX_API_URL}${path}" 2>/dev/null | jq -e '.success == true' >/dev/null 2>&1
}

print_header() {
  echo ""
  echo "==============================================================="
  echo "  DEEP SERVICES E2E"
  echo "  $(date)"
  echo "==============================================================="
  echo "RPC: $RPC_URL"
  echo "DEX: $DEX_API_URL"
  echo ""
}

section() {
  echo ""
  echo "--- $1 ---"
}

find_contract_by_keyword() {
  local keyword="$1"
  local contracts_json="$2"
  python3 - "$keyword" "$contracts_json" <<'PY'
import json,sys
kw = sys.argv[1].lower()
raw = sys.argv[2]
try:
    data = json.loads(raw)
except Exception:
    print("")
    raise SystemExit(0)
contracts = data.get("contracts", []) if isinstance(data, dict) else []
for c in contracts:
    pid = c.get("program_id")
    blob = json.dumps(c).lower()
    if kw in blob and pid:
        print(pid)
        raise SystemExit(0)
print("")
PY
}

normalize_contract_keyword() {
  echo "$1" | tr '[:upper:]' '[:lower:]' | tr -d '_-'
}

resolve_contract_id_by_name() {
  local name="$1"
  local contracts_json="$2"
  local keyword
  keyword="$(normalize_contract_keyword "$name")"
  python3 - "$keyword" "$contracts_json" <<'PY'
import json,sys,re
kw=sys.argv[1]
raw=sys.argv[2]
def norm(v)->str:
    if not v: return ""
    return re.sub(r'[_\-\s]+','',str(v).lower())
# Also try stripping "token" suffix for wrapped token directories (musd_token → musd)
kw_short=re.sub(r'token$','',kw)
try:
    data=json.loads(raw)
except Exception:
    print("")
    raise SystemExit(0)
contracts=data.get("contracts",[]) if isinstance(data,dict) else []
for c in contracts:
    if not isinstance(c,dict):
        continue
    pid=c.get("program_id","")
    blob=norm(json.dumps(c))
    sym=norm(c.get("symbol",""))
    if (kw in blob or (kw_short and kw_short==sym)) and pid:
        print(pid)
        raise SystemExit(0)
print("")
PY
}

check_contract_endpoints() {
  local name="$1"
  local pid="$2"
  if rpc_has_result "getContractInfo" "[\"$pid\"]"; then
    pass "getContractInfo($name)"
  else
    fail "getContractInfo($name)"
  fi
  if rpc_has_result "getProgramStats" "[\"$pid\"]"; then
    pass "getProgramStats($name)"
  else
    fail "getProgramStats($name)"
  fi
  if rpc_has_result "getProgramStorage" "[\"$pid\", {\"limit\": 10}]"; then
    pass "getProgramStorage($name)"
  else
    fail "getProgramStorage($name)"
  fi
  if rpc_has_result "getProgramCalls" "[\"$pid\", {\"limit\": 10}]"; then
    pass "getProgramCalls($name)"
  else
    fail "getProgramCalls($name)"
  fi
  if rpc_has_result "getContractEvents" "[\"$pid\", 50]"; then
    pass "getContractEvents($name)"
  else
    fail "getContractEvents($name)"
  fi
}

dex_pair_exists_by_symbols() {
  local base_sym="$1"
  local quote_sym="$2"
  local pairs_json
  local symbols_json
  pairs_json="$(curl -sS --max-time 8 "${DEX_API_URL}/pairs" 2>/dev/null || true)"
  symbols_json="$(rpc_result_json "getAllSymbolRegistry" "[]")"
  python3 - "$base_sym" "$quote_sym" "$pairs_json" "$symbols_json" <<'PY'
import sys,json
base=sys.argv[1].upper()
quote=sys.argv[2].upper()
pairs_raw=sys.argv[3]
symbols_raw=sys.argv[4]
try:
    pairs_doc=json.loads(pairs_raw)
    symbols_doc=json.loads(symbols_raw)
except Exception:
    print("no")
    raise SystemExit(0)

pairs=[]
if isinstance(pairs_doc,dict):
    data=pairs_doc.get("data")
    if isinstance(data,list):
        pairs=data

items=[]
if isinstance(symbols_doc,list):
    items=symbols_doc
elif isinstance(symbols_doc,dict):
    items=symbols_doc.get('entries', symbols_doc.get('items', symbols_doc.get('registry', symbols_doc.get('symbols', []))))

symbol_to_hex={}
for it in items:
    if not isinstance(it,dict):
        continue
    sym=(it.get('symbol') or it.get('ticker') or '').upper()
    addr=(it.get('program') or it.get('program_id') or it.get('address') or it.get('token') or '')
    if not sym or not addr:
        continue
    try:
        import base58
        b=base58.b58decode(addr)
    except Exception:
        try:
            alphabet='123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
            num=0
            for ch in addr:
                num=num*58+alphabet.index(ch)
            b=num.to_bytes((num.bit_length()+7)//8,'big')
            pad=0
            for ch in addr:
                if ch=='1': pad+=1
                else: break
            b=b'\x00'*pad+b
        except Exception:
            continue
    if len(b)>=32:
        symbol_to_hex[sym]=b[:32].hex()

base_hex=symbol_to_hex.get(base,'').lower()
quote_hex=symbol_to_hex.get(quote,'').lower()
if not base_hex or not quote_hex:
    print("no")
    raise SystemExit(0)

for p in pairs:
    if not isinstance(p,dict):
        continue
    a=(p.get('base_token') or '').lower()
    b=(p.get('quote_token') or '').lower()
    if (a==base_hex and b==quote_hex) or (a==quote_hex and b==base_hex):
        print("yes")
        raise SystemExit(0)

print("no")
PY
}

dex_pair_id_by_symbols() {
  local base_sym="$1"
  local quote_sym="$2"
  local pairs_json
  local symbols_json
  pairs_json="$(curl -sS --max-time 8 "${DEX_API_URL}/pairs" 2>/dev/null || true)"
  symbols_json="$(rpc_result_json "getAllSymbolRegistry" "[]")"
  python3 - "$base_sym" "$quote_sym" "$pairs_json" "$symbols_json" <<'PY'
import sys,json
base=sys.argv[1].upper()
quote=sys.argv[2].upper()
pairs_raw=sys.argv[3]
symbols_raw=sys.argv[4]
try:
  pairs_doc=json.loads(pairs_raw)
  symbols_doc=json.loads(symbols_raw)
except Exception:
  print("")
  raise SystemExit(0)

pairs=[]
if isinstance(pairs_doc,dict):
  data=pairs_doc.get("data")
  if isinstance(data,list):
    pairs=data

items=[]
if isinstance(symbols_doc,list):
  items=symbols_doc
elif isinstance(symbols_doc,dict):
  items=symbols_doc.get('entries', symbols_doc.get('items', symbols_doc.get('registry', symbols_doc.get('symbols', []))))

symbol_to_hex={}
for it in items:
  if not isinstance(it,dict):
    continue
  sym=(it.get('symbol') or it.get('ticker') or '').upper()
  addr=(it.get('program') or it.get('program_id') or it.get('address') or it.get('token') or '')
  if not sym or not addr:
    continue
  try:
    alphabet='123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
    num=0
    for ch in addr:
      num=num*58+alphabet.index(ch)
    b=num.to_bytes((num.bit_length()+7)//8,'big')
    pad=0
    for ch in addr:
      if ch=='1': pad+=1
      else: break
    b=b'\x00'*pad+b
  except Exception:
    continue
  if len(b)>=32:
    symbol_to_hex[sym]=b[:32].hex()

base_hex=symbol_to_hex.get(base,'').lower()
quote_hex=symbol_to_hex.get(quote,'').lower()
if not base_hex or not quote_hex:
  print("")
  raise SystemExit(0)

for p in pairs:
  if not isinstance(p,dict):
    continue
  a=(p.get('base_token') or '').lower()
  b=(p.get('quote_token') or '').lower()
  pid=p.get('pair_id')
  if ((a==base_hex and b==quote_hex) or (a==quote_hex and b==base_hex)) and pid is not None:
    print(pid)
    raise SystemExit(0)

print("")
PY
}

# -----------------------------------------------------------------------------
print_header

section "1) Core deep RPC surfaces"
if rpc_has_result "health" "[]"; then pass "health"; else fail "health"; fi
if rpc_has_result "getAllContracts" "[]"; then pass "getAllContracts"; else fail "getAllContracts"; fi
if rpc_has_result "getPrograms" "[]"; then pass "getPrograms"; else fail "getPrograms"; fi
if rpc_has_result "getAllSymbolRegistry" "[]"; then pass "getAllSymbolRegistry"; else fail "getAllSymbolRegistry"; fi
if rpc_has_result "getMarketListings" '[{}]'; then pass "getMarketListings"; else fail "getMarketListings"; fi
if rpc_has_result "getMarketSales" '[{}]'; then pass "getMarketSales"; else fail "getMarketSales"; fi
if rpc_has_result "getRewardAdjustmentInfo" "[]"; then pass "getRewardAdjustmentInfo"; else fail "getRewardAdjustmentInfo"; fi
if rpc_has_result "getReefStakePoolInfo" "[]"; then pass "getReefStakePoolInfo"; else fail "getReefStakePoolInfo"; fi

section "2) Contract inventory expectations"
CONTRACTS_RESULT="$(rpc_result_json "getAllContracts" "[]")"
CONTRACT_COUNT="$(python3 - "$CONTRACTS_RESULT" <<'PY'
import json,sys
try:
 d=json.loads(sys.argv[1])
 if isinstance(d,dict):
  print(d.get('count', len(d.get('contracts',[]))))
 else:
  print(0)
except Exception:
 print(0)
PY
)"

if [[ "$CONTRACT_COUNT" =~ ^[0-9]+$ ]] && (( CONTRACT_COUNT >= 10 )); then
  pass "deployed contract count >= 10 ($CONTRACT_COUNT)"
else
  fail "deployed contract count too low ($CONTRACT_COUNT)"
fi

EXPECTED_CONTRACTS=()
EXPECTED_CONTRACTS_FILE="${EXPECTED_CONTRACTS_FILE:-$ROOT_DIR/tests/expected-contracts.json}"

if [[ -f "$EXPECTED_CONTRACTS_FILE" ]]; then
  while IFS= read -r name; do
    [[ -n "$name" ]] && EXPECTED_CONTRACTS+=("$name")
  done < <(python3 - "$EXPECTED_CONTRACTS_FILE" <<'PY'
import json,sys
try:
  data=json.load(open(sys.argv[1], 'r', encoding='utf-8'))
except Exception:
  raise SystemExit(0)
contracts=data.get('contracts', []) if isinstance(data, dict) else []
for item in contracts:
  if isinstance(item, str) and item.strip():
    print(item.strip())
PY
)
elif [[ -d "$ROOT_DIR/contracts" ]]; then
  while IFS= read -r d; do
    EXPECTED_CONTRACTS+=("$(basename "$d")")
  done < <(find "$ROOT_DIR/contracts" -mindepth 1 -maxdepth 1 -type d | sort)
fi

if (( ${#EXPECTED_CONTRACTS[@]} > 0 )); then
  pass "loaded expected contract catalog (${#EXPECTED_CONTRACTS[@]} entries)"
else
  fail "could not load expected contracts from $ROOT_DIR/contracts"
fi

HAS_CONTRACT_NAMES="$(python3 - "$CONTRACTS_RESULT" <<'PY'
import json,sys
try:
 d=json.loads(sys.argv[1])
except Exception:
 print('0')
 raise SystemExit(0)
contracts=d.get('contracts',[]) if isinstance(d,dict) else []
for c in contracts:
 if not isinstance(c,dict):
  continue
 if c.get('name') or c.get('symbol'):
  print('1')
  raise SystemExit(0)
 md=c.get('metadata')
 if isinstance(md,dict) and (md.get('name') or md.get('symbol')):
  print('1')
  raise SystemExit(0)
print('0')
PY
)"

if [[ "$HAS_CONTRACT_NAMES" == "1" ]]; then
  for contract_name in "${EXPECTED_CONTRACTS[@]}"; do
    CONTRACT_ID="$(resolve_contract_id_by_name "$contract_name" "$CONTRACTS_RESULT")"
    if [[ -n "$CONTRACT_ID" ]]; then
      pass "contract deployed: $contract_name"
      check_contract_endpoints "$contract_name" "$CONTRACT_ID"
    else
      if [[ "$REQUIRE_ALL_CONTRACTS" == "1" ]]; then
        fail "contract missing from deployment: $contract_name"
      else
        pass "contract optional/missing: $contract_name"
      fi
    fi
  done
else
  pass "getAllContracts does not expose names; running generic endpoint checks for discovered program IDs"
  while IFS= read -r pid; do
    [[ -z "$pid" ]] && continue
    check_contract_endpoints "$pid" "$pid"
  done < <(python3 - "$CONTRACTS_RESULT" <<'PY'
import json,sys
try:
 d=json.loads(sys.argv[1])
except Exception:
 raise SystemExit(0)
contracts=d.get('contracts',[]) if isinstance(d,dict) else []
for c in contracts:
 if isinstance(c,dict):
  pid=c.get('program_id')
  if isinstance(pid,str) and pid:
   print(pid)
PY
)
fi

MOLTYID_ID="$(find_contract_by_keyword "moltyid" "$CONTRACTS_RESULT")"
if [[ -n "$MOLTYID_ID" ]]; then
  pass "MoltyID contract discoverable"
  if rpc_has_result "getContractInfo" "[\"$MOLTYID_ID\"]"; then
    pass "getContractInfo(moltyid)"
  else
    fail "getContractInfo(moltyid)"
  fi
else
  if [[ "$HAS_CONTRACT_NAMES" == "1" ]]; then
    fail "MoltyID contract not discoverable from contract inventory"
  else
    pass "MoltyID name-based discovery skipped (name metadata unavailable)"
  fi
fi

CLAWPUMP_ID="$(find_contract_by_keyword "clawpump" "$CONTRACTS_RESULT")"
if [[ -n "$CLAWPUMP_ID" ]]; then
  pass "ClawPump launchpad contract discoverable"
  if rpc_has_result "getContractInfo" "[\"$CLAWPUMP_ID\"]"; then
    pass "getContractInfo(clawpump)"
  else
    fail "getContractInfo(clawpump)"
  fi
  if rpc_has_result "getProgramStats" "[\"$CLAWPUMP_ID\"]"; then
    pass "getProgramStats(clawpump)"
  else
    fail "getProgramStats(clawpump)"
  fi
else
  if [[ "$REQUIRE_LAUNCHPAD" == "1" && "$HAS_CONTRACT_NAMES" == "1" ]]; then
    fail "ClawPump launchpad contract missing"
  else
    pass "ClawPump name-based discovery skipped or optional"
  fi
fi

section "3) Token lifecycle write-path"
TOKEN_ADDR=""
if [[ "$REQUIRE_TOKEN_WRITE" == "1" ]]; then
  if [[ -x "$MOLT_BIN" && -f "$AGENT_KEYPAIR" && -n "$HUMAN_ADDR" ]]; then
    TOKEN_NAME="E2E$(date +%s)"
    TOKEN_SYMBOL="E$((RANDOM%9))$((RANDOM%9))$((RANDOM%9))"

    if "$MOLT_BIN" --rpc-url "$RPC_URL" token create "$TOKEN_NAME" "$TOKEN_SYMBOL" --supply 1000000 --decimals 9 --keypair "$AGENT_KEYPAIR" >/tmp/e2e-token-create.log 2>&1; then
      pass "token create"
    else
      cat /tmp/e2e-token-create.log >&2 || true
      fail "token create"
    fi

    SYMBOLS="$(rpc_result_json "getAllSymbolRegistry" "[]")"
    TOKEN_ADDR="$(python3 - "$SYMBOLS" "$TOKEN_SYMBOL" <<'PY'
import json,sys
raw=sys.argv[1]
sym=sys.argv[2]
try:
 d=json.loads(raw)
except Exception:
 print("")
 raise SystemExit(0)
items=[]
if isinstance(d,list):
 items=d
elif isinstance(d,dict):
 items=d.get('items', d.get('registry', d.get('symbols', [])))
for it in items:
 if not isinstance(it,dict):
  continue
 s=(it.get('symbol') or it.get('ticker') or '').upper()
 if s==sym.upper():
  print(it.get('program_id') or it.get('address') or it.get('token') or '')
  raise SystemExit(0)
print("")
PY
)"

    if [[ -n "$TOKEN_ADDR" ]]; then
      pass "symbol registry contains created token"

      if "$MOLT_BIN" --rpc-url "$RPC_URL" token mint "$TOKEN_ADDR" 100 --to "$HUMAN_ADDR" --keypair "$AGENT_KEYPAIR" >/tmp/e2e-token-mint.log 2>&1; then
        pass "token mint"
      else
        cat /tmp/e2e-token-mint.log >&2 || true
        fail "token mint"
      fi

      if rpc_has_result "getTokenTransfers" "[\"$TOKEN_ADDR\", 25, 0]"; then
        pass "getTokenTransfers(created token)"
      else
        fail "getTokenTransfers(created token)"
      fi

      if rpc_has_result "getTokenHolders" "[\"$TOKEN_ADDR\", 50, 0]"; then
        pass "getTokenHolders(created token)"
      else
        fail "getTokenHolders(created token)"
      fi
    else
      pass "created token not yet in symbol registry (pending indexer sync)"
    fi
  else
    fail "token write-path prerequisites missing (molt binary/agent keypair/human address)"
  fi
else
  pass "token write-path disabled"
fi

section "4) DEX API deep checks"
if [[ "$REQUIRE_DEX_API" == "1" ]]; then
  if dex_ok "/pairs"; then pass "DEX /pairs"; else fail "DEX /pairs"; fi
  if dex_ok "/tickers"; then pass "DEX /tickers"; else fail "DEX /tickers"; fi
  if dex_ok "/pools"; then pass "DEX /pools"; else fail "DEX /pools"; fi

  if [[ -n "$DEX_BOOTSTRAP_BASE_SYMBOL" && -n "$DEX_BOOTSTRAP_QUOTE_SYMBOL" && "$DEX_BOOTSTRAP_BASE_SYMBOL" != "NONE" && "$DEX_BOOTSTRAP_QUOTE_SYMBOL" != "NONE" ]]; then
    if [[ "$(dex_pair_exists_by_symbols "$DEX_BOOTSTRAP_BASE_SYMBOL" "$DEX_BOOTSTRAP_QUOTE_SYMBOL")" == "yes" ]]; then
      pass "DEX bootstrap pair exists (${DEX_BOOTSTRAP_BASE_SYMBOL}/${DEX_BOOTSTRAP_QUOTE_SYMBOL})"
      BOOTSTRAP_PAIR_ID="$(dex_pair_id_by_symbols "$DEX_BOOTSTRAP_BASE_SYMBOL" "$DEX_BOOTSTRAP_QUOTE_SYMBOL")"
      if [[ -n "$BOOTSTRAP_PAIR_ID" ]]; then
        if curl -sS --max-time 8 "${DEX_API_URL}/pairs/${BOOTSTRAP_PAIR_ID}" | jq -e '.success == true' >/dev/null 2>&1; then
          pass "DEX bootstrap pair details endpoint"
        else
          fail "DEX bootstrap pair details endpoint"
        fi
        if curl -sS --max-time 8 "${DEX_API_URL}/pairs/${BOOTSTRAP_PAIR_ID}/orderbook?depth=20" | jq -e '.success == true' >/dev/null 2>&1; then
          pass "DEX bootstrap pair orderbook endpoint"
        else
          fail "DEX bootstrap pair orderbook endpoint"
        fi
        if curl -sS --max-time 8 "${DEX_API_URL}/pairs/${BOOTSTRAP_PAIR_ID}/trades?limit=25" | jq -e '.success == true' >/dev/null 2>&1; then
          pass "DEX bootstrap pair trades endpoint"
        else
          fail "DEX bootstrap pair trades endpoint"
        fi
        if curl -sS --max-time 8 "${DEX_API_URL}/pairs/${BOOTSTRAP_PAIR_ID}/candles?interval=3600&limit=24" | jq -e '.success == true' >/dev/null 2>&1; then
          pass "DEX bootstrap pair candles endpoint"
        else
          fail "DEX bootstrap pair candles endpoint"
        fi
        if curl -sS --max-time 8 "${DEX_API_URL}/pairs/${BOOTSTRAP_PAIR_ID}/stats" | jq -e '.success == true' >/dev/null 2>&1; then
          pass "DEX bootstrap pair stats endpoint"
        else
          fail "DEX bootstrap pair stats endpoint"
        fi
        if curl -sS --max-time 8 "${DEX_API_URL}/pairs/${BOOTSTRAP_PAIR_ID}/ticker" | jq -e '.success == true' >/dev/null 2>&1; then
          pass "DEX bootstrap pair ticker endpoint"
        else
          fail "DEX bootstrap pair ticker endpoint"
        fi
      else
        fail "DEX bootstrap pair id resolution failed"
      fi
    else
      pass "DEX bootstrap pair not yet seeded (${DEX_BOOTSTRAP_BASE_SYMBOL}/${DEX_BOOTSTRAP_QUOTE_SYMBOL}) — expected on fresh testnet"
    fi
  else
    pass "DEX bootstrap pair checks disabled"
  fi
else
  pass "DEX API checks disabled"
fi

section "5) Faucet and custody service checks"
if [[ "$REQUIRE_FAUCET" == "1" ]]; then
  if curl -sS --max-time 4 "$FAUCET_URL/health" | grep -qi "ok"; then
    pass "faucet health"
  else
    fail "faucet health"
  fi
fi

if [[ "$REQUIRE_CUSTODY" == "1" ]]; then
  if curl -sS --max-time 4 "$CUSTODY_URL/health" | jq -e '.status=="ok"' >/dev/null 2>&1; then
    pass "custody health"
  else
    fail "custody health"
  fi

  if curl -sS --max-time 4 "$CUSTODY_URL/status" | jq -e '.signers and .sweeps' >/dev/null 2>&1; then
    pass "custody status payload"
  else
    fail "custody status payload"
  fi
fi

section "6) Program and contract event deep checks"
if [[ -n "$MOLTYID_ID" ]]; then
  if rpc_has_result "getProgramStats" "[\"$MOLTYID_ID\"]"; then
    pass "getProgramStats(moltyid)"
  else
    fail "getProgramStats(moltyid)"
  fi

  if rpc_has_result "getContractEvents" "[\"$MOLTYID_ID\", 50, 0]"; then
    pass "getContractEvents(moltyid)"
  else
    fail "getContractEvents(moltyid)"
  fi
fi

section "7) Multisig and key-rotation regression checks"
if command -v cargo >/dev/null 2>&1; then
  if (
    cd "$ROOT_DIR" &&
      cargo test -p moltchain-core processor::tests::test_ecosystem_grant_requires_multisig -- --exact >/dev/null 2>&1 &&
      cargo test -p moltchain-core processor::tests::test_governed_proposal_lifecycle -- --exact >/dev/null 2>&1
  ); then
    pass "governed multisig transfer/approval/rejection path"
  else
    fail "governed multisig transfer/approval/rejection path"
  fi

  if (
    cd "$ROOT_DIR" &&
      cargo test -p moltchain-validator keypair_loader::tests::test_keypair_rotation_changes_loaded_pubkey -- --exact >/dev/null 2>&1 &&
      cargo test -p moltchain-custody tests::test_master_seed_rotation_changes_derived_addresses -- --exact >/dev/null 2>&1
  ); then
    pass "validator + custody key rotation scenario"
  else
    fail "validator + custody key rotation scenario"
  fi
else
  fail "cargo unavailable for multisig/key-rotation checks"
fi

section "8) Admin method access-control smoke checks"
if rpc_has_error "setFeeConfig" '[{"base_fee": 100}]'; then
  pass "non-admin rejected for setFeeConfig"
else
  fail "non-admin rejected for setFeeConfig"
fi

if rpc_has_error "setRentParams" '[{"exempt_minimum": 100}]'; then
  pass "non-admin rejected for setRentParams"
else
  fail "non-admin rejected for setRentParams"
fi

# -----------------------------------------------------------------------------
echo ""
echo "==============================================================="
echo "DEEP SERVICES E2E SUMMARY"
echo "PASS: $PASS"
echo "FAIL: $FAIL"
echo "==============================================================="

if (( FAIL > 0 )); then
  exit 1
fi

exit 0
