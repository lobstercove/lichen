#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.." || exit 1
mkdir -p tests/artifacts/full_matrix_feb24_2026
LOG="tests/artifacts/full_matrix_feb24_2026/full-matrix.log"
REPORT="tests/artifacts/full_matrix_feb24_2026/full-matrix-report.txt"
LOCK_DIR="tests/artifacts/full_matrix_feb24_2026/.full-matrix.lock"
CLUSTER_FORCE_MANAGED="${FORCE_MANAGED_MATRIX_CLUSTER:-1}"
CLUSTER_RESET_STATE="${RESET_MATRIX_STATE:-1}"
CLUSTER_BUILD_FIRST="${MATRIX_BUILD_FIRST:-1}"
CLUSTER_STAGGER_SECS="${MATRIX_STAGGER_SECS:-15}"
CLUSTER_MIN_VALIDATORS="${MATRIX_MIN_VALIDATORS:-3}"
MATRIX_REUSE_HEALTHY_CLUSTER="${MATRIX_REUSE_HEALTHY_CLUSTER:-0}"

if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  echo "[full-matrix] another matrix run is already active (lock: $LOCK_DIR)" >&2
  exit 1
fi

release_lock() {
  rmdir "$LOCK_DIR" 2>/dev/null || true
}

rpc_ok() {
  curl -sf --connect-timeout 1 --max-time 2 http://127.0.0.1:8899 \
    -X POST -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}' >/dev/null 2>&1
}

wait_cluster_ready() {
  local attempts="${1:-60}"
  local delay="${2:-1}"
  for _ in $(seq 1 "$attempts"); do
    local validators
    validators="$(curl -sf --connect-timeout 1 --max-time 2 http://127.0.0.1:8899 -X POST -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getValidators","params":[]}' | python3 -c 'import sys,json; d=json.load(sys.stdin).get("result",[]); v=d.get("validators", d) if isinstance(d, dict) else d; print(len(v) if isinstance(v, list) else 0)' 2>/dev/null || echo 0)"
    if [[ "$validators" -ge "$CLUSTER_MIN_VALIDATORS" ]]; then
      return 0
    fi
    sleep "$delay"
  done
  return 1
}

cluster_has_quorum_now() {
  # All 3 RPC ports must respond — not just the registered validator count
  for port in 8899 8901 8903; do
    if ! curl -sf --connect-timeout 1 --max-time 2 "http://127.0.0.1:$port" \
      -X POST -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}' >/dev/null 2>&1; then
      return 1
    fi
  done
  return 0
}

ensure_cluster_ready() {
  # All 3 RPC ports must respond for the cluster to be considered ready
  local all_alive=1
  for port in 8899 8901 8903; do
    if ! curl -sf --connect-timeout 1 --max-time 2 "http://127.0.0.1:$port" \
      -X POST -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}' >/dev/null 2>&1; then
      all_alive=0
      break
    fi
  done
  if [[ "$all_alive" == "1" ]] && wait_cluster_ready 5 1; then
    return 0
  fi

  echo "[full-matrix] cluster health degraded; restarting managed cluster with full reset" | tee -a "$LOG"
  FORCE_MANAGED_MATRIX_CLUSTER=1 RESET_MATRIX_STATE=1 MATRIX_BUILD_FIRST=0 MATRIX_STAGGER_SECS=10 \
    bash tests/matrix-sdk-cluster.sh restart >> "$LOG" 2>&1

  if ! wait_cluster_ready 90 1; then
    echo "[full-matrix] ERROR: cluster did not become ready" | tee -a "$LOG"
    return 1
  fi
  return 0
}

command_needs_cluster() {
  local cmd="$1"
  if [[ "$cmd" == *"matrix-sdk-cluster.sh stop"* ]]; then
    return 1
  fi
  return 0
}

max_attempts_for_command() {
  local cmd="$1"
  if [[ "$cmd" == *"live-e2e-test.sh"* || "$cmd" == *"contracts-write-e2e.py"* || "$cmd" == *"e2e-dex.js"* || "$cmd" == *"e2e-dex-trading.py"* || "$cmd" == *"e2e-volume.js"* || "$cmd" == *"comprehensive-e2e.py"* || "$cmd" == *"comprehensive-e2e-parallel.py"* || "$cmd" == *"e2e-websocket-upgrade.py"* || "$cmd" == *"load-test-5k-traders.py"* ]]; then
    if [[ "$cmd" == *"live-e2e-test.sh"* || "$cmd" == *"comprehensive-e2e.py"* || "$cmd" == *"comprehensive-e2e-parallel.py"* ]]; then
      echo 3
      return
    fi
    echo 2
    return
  fi
  echo 1
}

resolve_signers() {
  local json
  json="$(python3 tests/resolve-funded-signers.py 2>/dev/null || echo '{}')"
  MATRIX_AGENT_KEYPAIR="$(echo "$json" | python3 -c 'import sys,json; d=json.load(sys.stdin); a=d.get("agent") or {}; print(a.get("path", ""))' 2>/dev/null || true)"
  MATRIX_HUMAN_KEYPAIR="$(echo "$json" | python3 -c 'import sys,json; d=json.load(sys.stdin); h=d.get("human") or {}; print(h.get("path", ""))' 2>/dev/null || true)"
  MATRIX_SIGNER_COUNT="$(echo "$json" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(int(d.get("count",0)))' 2>/dev/null || echo 0)"

  if [[ -z "$MATRIX_AGENT_KEYPAIR" || ! -f "$MATRIX_AGENT_KEYPAIR" ]]; then
    MATRIX_AGENT_KEYPAIR="$PWD/keypairs/deployer.json"
  fi
  if [[ -z "$MATRIX_HUMAN_KEYPAIR" || ! -f "$MATRIX_HUMAN_KEYPAIR" ]]; then
    MATRIX_HUMAN_KEYPAIR="$MATRIX_AGENT_KEYPAIR"
  fi
}

get_spendable_shells() {
  local pubkey="$1"
  python3 - "$pubkey" <<'PY'
import json, sys, urllib.request
pubkey = sys.argv[1]
payload = json.dumps({"jsonrpc":"2.0","id":1,"method":"getBalance","params":[pubkey]}).encode()
req = urllib.request.Request("http://127.0.0.1:8899", data=payload, headers={"Content-Type":"application/json"})
try:
    with urllib.request.urlopen(req, timeout=5) as resp:
        out = json.loads(resp.read())
    result = out.get("result") or {}
    shells = int(result.get("spendable", result.get("shells", 0)))
    print(shells)
except Exception:
    print(0)
PY
}

ensure_funded_signers() {
  local min_shells="${MIN_FUNDED_SHELLS:-20000000000}"
  local agent_pub human_pub agent_shells human_shells
  agent_pub="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d.get("pubkey",""))' "$MATRIX_AGENT_KEYPAIR" 2>/dev/null || true)"
  human_pub="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d.get("pubkey",""))' "$MATRIX_HUMAN_KEYPAIR" 2>/dev/null || true)"

  if [[ -n "$agent_pub" ]]; then
    agent_shells="$(get_spendable_shells "$agent_pub")"
  else
    agent_shells=0
  fi

  if [[ -n "$human_pub" ]]; then
    human_shells="$(get_spendable_shells "$human_pub")"
  else
    human_shells=0
  fi

  echo "[full-matrix] signer funding agent=${agent_shells} human=${human_shells} min=${min_shells}" | tee -a "$LOG"

  if [[ "$agent_shells" -lt "$min_shells" || "$human_shells" -lt "$min_shells" ]]; then
    echo "[full-matrix] signer funding below minimum; attempting requestAirdrop fallback" | tee -a "$LOG"
    if [[ -n "$agent_pub" ]]; then
      curl -s -X POST http://127.0.0.1:8899 -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"requestAirdrop\",\"params\":[\"$agent_pub\",100]}" >/dev/null 2>&1 || true
    fi
    if [[ -n "$human_pub" ]]; then
      curl -s -X POST http://127.0.0.1:8899 -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"requestAirdrop\",\"params\":[\"$human_pub\",100]}" >/dev/null 2>&1 || true
    fi
    sleep 2
  fi

  if [[ -n "$agent_pub" ]]; then
    agent_shells="$(get_spendable_shells "$agent_pub")"
  fi
  if [[ -n "$human_pub" ]]; then
    human_shells="$(get_spendable_shells "$human_pub")"
  fi

  echo "[full-matrix] signer funding after preflight agent=${agent_shells} human=${human_shells}" | tee -a "$LOG"

  if [[ "$agent_shells" -lt "$min_shells" || "$human_shells" -lt "$min_shells" ]]; then
    echo "[full-matrix] ERROR: funded signer preflight failed (agent=${agent_shells}, human=${human_shells}, min=${min_shells})" | tee -a "$LOG"
    return 1
  fi

  return 0
}

MATRIX_AGENT_KEYPAIR="$PWD/keypairs/deployer.json"
MATRIX_HUMAN_KEYPAIR="$MATRIX_AGENT_KEYPAIR"
MATRIX_SIGNER_COUNT=0

: > "$LOG"
: > "$REPORT"

echo "[full-matrix] preflight: start matrix cluster" | tee -a "$LOG"
if [[ "$MATRIX_REUSE_HEALTHY_CLUSTER" == "1" ]] && cluster_has_quorum_now; then
  echo "[full-matrix] healthy cluster already running; reusing existing validators (skip reset/rebuild)" | tee -a "$LOG"
  CLUSTER_FORCE_MANAGED=0
  CLUSTER_RESET_STATE=0
  CLUSTER_BUILD_FIRST=0
fi
FORCE_MANAGED_MATRIX_CLUSTER="$CLUSTER_FORCE_MANAGED" RESET_MATRIX_STATE="$CLUSTER_RESET_STATE" MATRIX_BUILD_FIRST="$CLUSTER_BUILD_FIRST" MATRIX_STAGGER_SECS="$CLUSTER_STAGGER_SECS" \
  bash tests/matrix-sdk-cluster.sh start >> "$LOG" 2>&1

if ! wait_cluster_ready 90 1; then
  echo "[full-matrix] ERROR: cluster did not become ready after preflight start" | tee -a "$LOG"
  release_lock
  exit 1
fi

resolve_signers
if [[ "$MATRIX_SIGNER_COUNT" -eq 0 ]]; then
  fallback_agent="$PWD/data/state-8000/genesis-keys/builder_grants-moltchain-testnet-1.json"
  fallback_human="$PWD/data/state-8000/genesis-keys/community_treasury-moltchain-testnet-1.json"
  if [[ -f "$fallback_agent" && -f "$fallback_human" ]]; then
    MATRIX_AGENT_KEYPAIR="$fallback_agent"
    MATRIX_HUMAN_KEYPAIR="$fallback_human"
    MATRIX_SIGNER_COUNT=2
    echo "[full-matrix] signer discovery fallback enabled (builder_grants/community_treasury)" | tee -a "$LOG"
  fi
fi
if ! ensure_funded_signers; then
  echo "[full-matrix] signer preflight failed; attempting one-shot seeded reset bootstrap" | tee -a "$LOG"
  FORCE_MANAGED_MATRIX_CLUSTER=1 RESET_MATRIX_STATE=1 MATRIX_BUILD_FIRST=1 MATRIX_STAGGER_SECS="$CLUSTER_STAGGER_SECS" \
    bash tests/matrix-sdk-cluster.sh restart >> "$LOG" 2>&1

  if ! wait_cluster_ready 120 1; then
    echo "[full-matrix] ERROR: seeded reset bootstrap did not become ready" | tee -a "$LOG"
    echo "TOTAL=0 PASS=0 FAIL=1" | tee -a "$REPORT"
    echo "LOG=$LOG" | tee -a "$REPORT"
    release_lock
    exit 1
  fi

  resolve_signers
  if [[ "$MATRIX_SIGNER_COUNT" -eq 0 ]]; then
    fallback_agent="$PWD/data/state-8000/genesis-keys/builder_grants-moltchain-testnet-1.json"
    fallback_human="$PWD/data/state-8000/genesis-keys/community_treasury-moltchain-testnet-1.json"
    if [[ -f "$fallback_agent" && -f "$fallback_human" ]]; then
      MATRIX_AGENT_KEYPAIR="$fallback_agent"
      MATRIX_HUMAN_KEYPAIR="$fallback_human"
      MATRIX_SIGNER_COUNT=2
      echo "[full-matrix] signer discovery fallback enabled after reset bootstrap" | tee -a "$LOG"
    fi
  fi

  if ! ensure_funded_signers; then
    echo "[full-matrix] ERROR: aborting matrix run due to unfunded signer preflight" | tee -a "$LOG"
    echo "TOTAL=0 PASS=0 FAIL=1" | tee -a "$REPORT"
    echo "LOG=$LOG" | tee -a "$REPORT"
    release_lock
    exit 1
  fi
fi

commands=(
  "bash tests/test-rpc-comprehensive.sh"
  "bash tests/test-websocket.sh"
  "bash tests/test-cli-comprehensive.sh"
  "bash tests/live-e2e-test.sh"
  "REQUIRE_ALL_CONTRACTS=0 bash tests/services-deep-e2e.sh"
  "AGENT_KEYPAIR='$MATRIX_AGENT_KEYPAIR' HUMAN_KEYPAIR='$MATRIX_HUMAN_KEYPAIR' REQUIRE_FULL_WRITE_ACTIVITY=0 STRICT_WRITE_ASSERTIONS=0 ENFORCE_DOMAIN_ASSERTIONS=0 MIN_CONTRACT_ACTIVITY_DELTA=0 python3 tests/contracts-write-e2e.py"
  "bash tests/test-contract-deployment.sh"
  "bash scripts/test-all-sdks.sh"
  "node tests/e2e-dex.js"
  "AGENT_KEYPAIR='$MATRIX_AGENT_KEYPAIR' python3 tests/e2e-dex-trading.py"
  "node tests/e2e-launchpad.js"
  "node tests/e2e-prediction.js"
  "PREDICTION_MULTI_OUTCOME_ONLY=1 node tests/e2e-prediction.js"
  "node tests/e2e-volume.js"
  "bash tests/test-dex-api-comprehensive.sh"
  "node tests/test-ws-dex.js"
  "node tests/test_wallet_audit.js"
  "node tests/test_wallet_extension_audit.js"
  "node tests/test_wallet_modal_parity.js"
  "node tests/test_programs_override_wiring.js"
  "node tests/test_marketplace_audit.js"
  "bash tests/test-mkt-featured-filter.sh"
  "bash tests/test-critical-security.sh"
  "node tests/test_developers_audit.js"
  "node tests/test_website_audit.js"
  "node tests/test_cross_cutting_audit.js"
  "node tests/test_coverage_audit.js"
  "python3 tests/e2e-genesis-wiring.py"
  "bash tests/multi-validator-e2e.sh"
  "AGENT_KEYPAIR='$MATRIX_AGENT_KEYPAIR' REQUIRE_FUNDED_DEPLOYER=0 RPC_ENDPOINTS='http://127.0.0.1:8899' python3 tests/comprehensive-e2e.py"
  "AGENT_KEYPAIR='$MATRIX_AGENT_KEYPAIR' REQUIRE_FUNDED_DEPLOYER=0 RPC_ENDPOINTS='http://127.0.0.1:8899' python3 tests/comprehensive-e2e-parallel.py"
  "AGENT_KEYPAIR='$MATRIX_AGENT_KEYPAIR' python3 tests/e2e-websocket-upgrade.py"
  "AGENT_KEYPAIR='$MATRIX_AGENT_KEYPAIR' python3 tests/load-test-5k-traders.py"
  "bash tests/matrix-sdk-cluster.sh status"
  "python3 sdk/python/test_sdk_live.py"
  "python3 sdk/python/test_websocket_sdk.py"
  "python3 sdk/python/test_websocket_simple.py"
  "python3 sdk/python/test_cross_sdk_compat.py"
  "npx --yes ts-node sdk/js/test-all-features.ts"
  "node sdk/js/test_cross_sdk_compat.js"
  "node sdk/js/test-subscriptions.js"
  "cargo run --manifest-path sdk/rust/Cargo.toml --example test_transactions"
  "bash tests/matrix-sdk-cluster.sh stop"
)

echo "[full-matrix] signer_count=$MATRIX_SIGNER_COUNT agent=$MATRIX_AGENT_KEYPAIR human=$MATRIX_HUMAN_KEYPAIR" | tee -a "$LOG"

cleanup() {
  bash tests/matrix-sdk-cluster.sh stop >> "$LOG" 2>&1 || true
  release_lock
}
trap cleanup EXIT INT TERM

total=${#commands[@]}
pass=0
fail=0

echo "[full-matrix] start total=$total" | tee -a "$LOG"

for i in "${!commands[@]}"; do
  n=$((i+1))
  cmd="${commands[$i]}"

  if command_needs_cluster "$cmd"; then
    if ! ensure_cluster_ready; then
      fail=$((fail+1))
      echo "[$n/$total] FAIL (0s) precheck cluster-not-ready $cmd" | tee -a "$REPORT"
      continue
    fi
  fi

  echo "[$n/$total] RUN $cmd" | tee -a "$LOG"
  start_ts=$(date +%s)
  attempts="$(max_attempts_for_command "$cmd")"
  code=1
  attempt=1
  while [[ "$attempt" -le "$attempts" ]]; do
    set +e
    bash -lc "$cmd" >> "$LOG" 2>&1
    code=$?
    set -e
    if [[ "$code" -eq 0 ]]; then
      break
    fi
    if [[ "$attempt" -lt "$attempts" ]]; then
      echo "[$n/$total] RETRY attempt=$((attempt+1))/$attempts after exit=$code" | tee -a "$LOG"
      ensure_cluster_ready || true
      sleep 2
    fi
    attempt=$((attempt+1))
  done
  dur=$(( $(date +%s) - start_ts ))
  if [ $code -eq 0 ]; then
    pass=$((pass+1))
    echo "[$n/$total] PASS (${dur}s) $cmd" | tee -a "$REPORT"
  else
    fail=$((fail+1))
    echo "[$n/$total] FAIL (${dur}s) exit=$code $cmd" | tee -a "$REPORT"
  fi

done

echo "TOTAL=$total PASS=$pass FAIL=$fail" | tee -a "$REPORT"
echo "LOG=$LOG" | tee -a "$REPORT"
