#!/usr/bin/env bash
set -euo pipefail

# Read-only public-history parity verifier for the four-validator public fleet.
# By default it runs bounded host/RPC probes only. A full manifest is an explicit
# diagnostic (`--live-manifest`) or strict fixed-tip gate (`--stop-for-manifest`),
# because a moving live manifest is expensive and cannot prove release parity.
# Strict mode stops all validator services, computes manifests concurrently into
# durable host-local files, and restarts every service after a matching precheck.

usage() {
  cat >&2 <<'EOF'
Usage:
  bash scripts/verify-testnet-archive-parity.sh [options]

Options:
  --network <testnet|mainnet>       Network to inspect (default: testnet)
  --hosts "<host host ...>"         Override VPS host list
  --evidence-dir <path>             Evidence output directory
  --categories <csv>                Public-history categories to scan
  --chunk-size <n>                  Manifest chunk size (default: 20000)
  --sample-slots <csv>              getBlock slots to compare
  --sample-txs <csv>                getTransaction signatures to compare
  --sample-addresses <csv>          getTransactionsByAddress addresses to compare
  --skip-manifest                   Only run RPC probes and host preflight (default)
  --live-manifest                   Explicit moving-tip manifest diagnostic
  --stop-for-manifest               Stop validators for strict offline manifests
  --offline-repair-gate             Stop, verify fixed-tip manifests, and leave stopped
  --help                            Show this help

Environment:
  LICHEN_ARCHIVE_PARITY_STOP_CONFIRM must equal the confirmation string when
  --stop-for-manifest is used.
EOF
}

NETWORK="testnet"
HOSTS=""
EVIDENCE_DIR=""
CATEGORIES="${LICHEN_ARCHIVE_PARITY_CATEGORIES:-}"
CHUNK_SIZE="${LICHEN_ARCHIVE_PARITY_CHUNK_SIZE:-20000}"
SAMPLE_SLOTS="${LICHEN_ARCHIVE_PARITY_SAMPLE_SLOTS:-}"
SAMPLE_TXS="${LICHEN_ARCHIVE_PARITY_SAMPLE_TXS:-}"
SAMPLE_ADDRESSES="${LICHEN_ARCHIVE_PARITY_SAMPLE_ADDRESSES:-}"
SKIP_MANIFEST=1
LIVE_MANIFEST=0
STOP_FOR_MANIFEST="${LICHEN_ARCHIVE_PARITY_STOP_FOR_MANIFEST:-0}"
OFFLINE_REPAIR_GATE=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --network)
      NETWORK="${2:?--network requires a value}"
      shift 2
      ;;
    --hosts)
      HOSTS="${2:?--hosts requires a value}"
      shift 2
      ;;
    --evidence-dir)
      EVIDENCE_DIR="${2:?--evidence-dir requires a value}"
      shift 2
      ;;
    --categories)
      CATEGORIES="${2:?--categories requires a value}"
      shift 2
      ;;
    --chunk-size)
      CHUNK_SIZE="${2:?--chunk-size requires a value}"
      shift 2
      ;;
    --sample-slots)
      SAMPLE_SLOTS="${2:?--sample-slots requires a value}"
      shift 2
      ;;
    --sample-txs)
      SAMPLE_TXS="${2:?--sample-txs requires a value}"
      shift 2
      ;;
    --sample-addresses)
      SAMPLE_ADDRESSES="${2:?--sample-addresses requires a value}"
      shift 2
      ;;
    --skip-manifest)
      SKIP_MANIFEST=1
      shift
      ;;
    --live-manifest)
      SKIP_MANIFEST=0
      LIVE_MANIFEST=1
      shift
      ;;
    --stop-for-manifest)
      SKIP_MANIFEST=0
      STOP_FOR_MANIFEST=1
      shift
      ;;
    --offline-repair-gate)
      SKIP_MANIFEST=0
      STOP_FOR_MANIFEST=1
      OFFLINE_REPAIR_GATE=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [ "$STOP_FOR_MANIFEST" = "1" ]; then
  SKIP_MANIFEST=0
fi

if [ "$LIVE_MANIFEST" = "1" ] && [ "$STOP_FOR_MANIFEST" = "1" ]; then
  echo "--live-manifest and stopped manifest modes are mutually exclusive" >&2
  exit 2
fi

if ! [[ "$CHUNK_SIZE" =~ ^[1-9][0-9]*$ ]]; then
  echo "--chunk-size must be a positive integer" >&2
  exit 2
fi

case "$NETWORK" in
  testnet)
    SERVICE="lichen-validator-testnet"
    RPC_PORT="${LICHEN_ARCHIVE_PARITY_RPC_PORT:-8899}"
    STATE_DIR="${LICHEN_ARCHIVE_PARITY_STATE_DIR:-/var/lib/lichen/state-testnet}"
    COLD_DIR="${LICHEN_ARCHIVE_PARITY_COLD_DIR:-/var/lib/lichen/archive-testnet}"
    DEFAULT_HOSTS="15.204.229.189 37.59.97.61 15.235.142.253 148.113.43.247"
    ;;
  mainnet)
    SERVICE="lichen-validator-mainnet"
    RPC_PORT="${LICHEN_ARCHIVE_PARITY_RPC_PORT:-9899}"
    STATE_DIR="${LICHEN_ARCHIVE_PARITY_STATE_DIR:-/var/lib/lichen/state-mainnet}"
    COLD_DIR="${LICHEN_ARCHIVE_PARITY_COLD_DIR:-/var/lib/lichen/archive-mainnet}"
    DEFAULT_HOSTS="${LICHEN_MAINNET_VPS_HOSTS:-}"
    ;;
  *)
    echo "Unsupported network: $NETWORK" >&2
    exit 2
    ;;
esac

HOSTS="${HOSTS:-${LICHEN_ARCHIVE_PARITY_HOSTS:-$DEFAULT_HOSTS}}"
if [ -z "$HOSTS" ]; then
  echo "No hosts configured for $NETWORK" >&2
  exit 2
fi

RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)"
EVIDENCE_DIR="${EVIDENCE_DIR:-evidence/archive-parity/${NETWORK}-${RUN_ID}}"
SSH_USER="${LICHEN_ARCHIVE_PARITY_SSH_USER:-ubuntu}"
SSH_PORT="${LICHEN_ARCHIVE_PARITY_SSH_PORT:-2222}"
SSH_CONNECT_TIMEOUT="${LICHEN_ARCHIVE_PARITY_SSH_CONNECT_TIMEOUT:-15}"
SSH_ATTEMPTS="${LICHEN_ARCHIVE_PARITY_SSH_ATTEMPTS:-5}"
SSH_RETRY_DELAY_SECS="${LICHEN_ARCHIVE_PARITY_SSH_RETRY_DELAY_SECS:-31}"
SSH_STRICT_HOST_KEY_CHECKING="${LICHEN_ARCHIVE_PARITY_STRICT_HOST_KEY_CHECKING:-yes}"
SSH_KNOWN_HOSTS_FILE="${LICHEN_ARCHIVE_PARITY_KNOWN_HOSTS_FILE:-$HOME/.ssh/known_hosts}"
SSH_CONTROL_DIR="$(mktemp -d /tmp/lichen-ap-ssh.XXXXXX)"
MAX_SLOT_SPREAD="${LICHEN_ARCHIVE_PARITY_MAX_SLOT_SPREAD:-180}"
CACHE_SIZE_MB="${LICHEN_ARCHIVE_PARITY_CACHE_SIZE_MB:-256}"
VALIDATOR_BIN="${LICHEN_ARCHIVE_PARITY_VALIDATOR_BIN:-/usr/local/bin/lichen-validator}"
NOFILE_LIMIT="${LICHEN_ARCHIVE_PARITY_NOFILE_LIMIT:-1048576}"

mkdir -p "$EVIDENCE_DIR"
RUN_LOG="$EVIDENCE_DIR/run.log"
exec 3>&1 4>&2
exec >"$RUN_LOG" 2>&1

close_ssh_controls() {
  local host
  for host in $HOSTS; do
    ssh -p "$SSH_PORT" \
      -o ControlPath="$SSH_CONTROL_DIR/%C" \
      -O exit \
      "$SSH_USER@$host" >/dev/null 2>&1 || true
  done
  rm -rf "$SSH_CONTROL_DIR"
}

finalize() {
  local status=$?
  trap - EXIT
  close_ssh_controls
  exec 1>&3 2>&4
  cat "$RUN_LOG"
  exit "$status"
}

trap finalize EXIT

ssh_run() {
  local host="$1"
  shift
  local attempt status
  for attempt in $(seq 1 "$SSH_ATTEMPTS"); do
    if ssh -p "$SSH_PORT" \
      -o BatchMode=yes \
      -o ConnectTimeout="$SSH_CONNECT_TIMEOUT" \
      -o ConnectionAttempts=1 \
      -o ServerAliveInterval=10 \
      -o ServerAliveCountMax=3 \
      -o StrictHostKeyChecking="$SSH_STRICT_HOST_KEY_CHECKING" \
      -o UserKnownHostsFile="$SSH_KNOWN_HOSTS_FILE" \
      -o LogLevel=ERROR \
      -o ControlMaster=auto \
      -o ControlPersist=120 \
      -o ControlPath="$SSH_CONTROL_DIR/%C" \
      "$SSH_USER@$host" "$@"; then
      return 0
    else
      status=$?
    fi
    if [ "$attempt" -lt "$SSH_ATTEMPTS" ]; then
      sleep "$SSH_RETRY_DELAY_SECS"
    fi
  done
  return "$status"
}

wait_pids() {
  local status=0 pid
  for pid in "$@"; do
    if ! wait "$pid"; then
      status=1
    fi
  done
  return "$status"
}

host_label() {
  case "$1" in
    15.204.229.189|seed-01.lichen.network) echo "us" ;;
    37.59.97.61|seed-02.lichen.network) echo "eu" ;;
    15.235.142.253|seed-03.lichen.network) echo "sea" ;;
    148.113.43.247|seed-04.lichen.network) echo "in" ;;
    *) echo "$1" | tr -c 'A-Za-z0-9_-' '_' ;;
  esac
}

csv_to_json_array() {
  python3 - "$1" <<'PY'
import json
import sys

raw = sys.argv[1].strip()
items = [part.strip() for part in raw.split(",") if part.strip()]
print(json.dumps(items))
PY
}

csv_to_json_number_array() {
  python3 - "$1" <<'PY'
import json
import sys

items = []
for part in sys.argv[1].split(","):
    part = part.strip()
    if not part:
        continue
    items.append(int(part))
print(json.dumps(items))
PY
}

rpc_payload() {
  local method="$1"
  local params_json="$2"
  python3 - "$method" "$params_json" <<'PY'
import json
import sys

method = sys.argv[1]
params = json.loads(sys.argv[2])
print(json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}, separators=(",", ":")))
PY
}

rpc_call() {
  local host="$1"
  local method="$2"
  local params_json="$3"
  local output="$4"
  local payload
  payload="$(rpc_payload "$method" "$params_json")"
  printf '%s' "$payload" |
    ssh_run "$host" "curl -fsS --max-time 30 -H 'Content-Type: application/json' --data-binary @- http://127.0.0.1:${RPC_PORT}/" \
      >"$output"
}

capture_historical_rpc_probes() {
  local host="$1"
  local label="$2"
  local slots_json="$3"
  local txs_json="$4"
  local addresses_json="$5"
  ssh_run "$host" "python3 - '$slots_json' '$txs_json' '$addresses_json' <<'PY'
import json
import subprocess
import sys

slots = json.loads(sys.argv[1])
txs = json.loads(sys.argv[2])
addresses = json.loads(sys.argv[3])
url = 'http://127.0.0.1:${RPC_PORT}/'

def rpc(method, params):
    payload = json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': params})
    completed = subprocess.run(
        [
            'curl',
            '-fsS',
            '--max-time',
            '30',
            '-H',
            'Content-Type: application/json',
            '--data',
            payload,
            url,
        ],
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        return {'_transport_error': completed.stderr.strip() or f'curl exited {completed.returncode}'}
    try:
        return json.loads(completed.stdout)
    except Exception as exc:
        return {'_parse_error': str(exc), '_raw': completed.stdout[:500]}

results = {}
for slot in slots:
    results[f'getBlock:{slot}'] = rpc('getBlock', [slot])
for tx in txs:
    results[f'getTransaction:{tx}'] = rpc('getTransaction', [tx])
for address in addresses:
    results[f'getTransactionsByAddress:{address}'] = rpc(
        'getTransactionsByAddress',
        [address, {'limit': 30}],
    )
print(json.dumps(results, indent=2, sort_keys=True))
PY" >"$EVIDENCE_DIR/rpc-probes-${label}.json"
}

remote_manifest() {
  local host="$1"
  local label="$2"
  local mode="$3"
  local output="$EVIDENCE_DIR/manifest-${label}.json"
  local secondary_dir="/tmp/lichen-public-history-manifest-${NETWORK}-${label}-${RUN_ID}"
  local remote_dir="/var/tmp/lichen-archive-parity/${NETWORK}-${RUN_ID}-${label}"
  local remote_output="$remote_dir/manifest.json"
  local remote_stderr="$remote_dir/stderr.log"
  local remote_status="$remote_dir/status"
  local remote_pid="$remote_dir/pid"
  local wrapper command_q="" launch q_wrapper q_output q_stderr q_status q_pid
  local poll_result status started_at now
  local -a command=(
    "$VALIDATOR_BIN"
    --no-watchdog
    --network "$NETWORK"
    --db-path "$STATE_DIR"
    --cache-size-mb "$CACHE_SIZE_MB"
    --chunk-size "$CHUNK_SIZE"
  )

  if [ -n "$CATEGORIES" ]; then
    command+=(--categories "$CATEGORIES")
  fi
  if [ "$mode" != "offline" ]; then
    command+=(--secondary-dir "$secondary_dir")
  fi
  command+=(--public-history-manifest)
  printf -v command_q ' %q' "${command[@]}"

  # The wrapper is intentionally expanded only by the remote bash process.
  # shellcheck disable=SC2016
  wrapper='set +e
out="$1"
err="$2"
status_file="$3"
pid_file="$4"
shift 4
printf "%s\n" "$BASHPID" >"${pid_file}.partial"
mv -f "${pid_file}.partial" "$pid_file"
ulimit -n '"$NOFILE_LIMIT"' 2>/dev/null || ulimit -n 65535 2>/dev/null || true
"$@" >"${out}.partial" 2>"$err"
status=$?
if [ "$status" -eq 0 ]; then mv -f "${out}.partial" "$out"; fi
printf "%s\n" "$status" >"${status_file}.partial"
mv -f "${status_file}.partial" "$status_file"
exit "$status"'
  printf -v q_wrapper %q "$wrapper"
  printf -v q_output %q "$remote_output"
  printf -v q_stderr %q "$remote_stderr"
  printf -v q_status %q "$remote_status"
  printf -v q_pid %q "$remote_pid"
  printf -v launch 'sudo -u lichen nohup bash -c %s bash %s %s %s %s%s >/dev/null 2>&1 </dev/null &' \
    "$q_wrapper" "$q_output" "$q_stderr" "$q_status" "$q_pid" "$command_q"

  if [ "$mode" != "offline" ]; then
    ssh_run "$host" "sudo rm -rf '$secondary_dir'"
  fi
  ssh_run "$host" "
    set -e
    sudo install -d -o lichen -g lichen -m 0700 '$remote_dir'
    if sudo -u lichen test -s '$remote_status'; then exit 0; fi
    if sudo -u lichen test -s '$remote_pid' && sudo -u lichen kill -0 \$(sudo -u lichen cat '$remote_pid') 2>/dev/null; then exit 0; fi
    sudo -u lichen rm -f '$remote_output' '${remote_output}.partial' '$remote_stderr' '$remote_status' '${remote_status}.partial' '$remote_pid'
    $launch
    for attempt in \$(seq 1 50); do
      if sudo -u lichen test -s '$remote_pid' || sudo -u lichen test -s '$remote_status'; then exit 0; fi
      sleep 0.1
    done
    exit 1
  "

  started_at="$(date +%s)"
  while :; do
    poll_result="$(ssh_run "$host" "
      if sudo -u lichen test -s '$remote_status'; then
        printf 'done:'
        sudo -u lichen cat '$remote_status'
      elif sudo -u lichen test -s '$remote_pid' && sudo -u lichen kill -0 \$(sudo -u lichen cat '$remote_pid') 2>/dev/null; then
        echo running
      else
        echo missing
      fi
    ")"
    case "$poll_result" in
      done:*)
        status="${poll_result#done:}"
        break
        ;;
      running) ;;
      *)
        echo "Remote manifest job disappeared on $label: $poll_result" >&2
        return 1
        ;;
    esac
    now="$(date +%s)"
    if [ $(((now - started_at) % 60)) -lt 15 ]; then
      echo "  - $label manifest still running ($((now - started_at))s)"
    fi
    sleep 15
  done

  ssh_run "$host" "sudo -u lichen cat '$remote_stderr'" >"$EVIDENCE_DIR/manifest-${label}.stderr.log" || true
  if [ "$status" != "0" ]; then
    echo "Remote manifest failed on $label with status $status" >&2
    cat "$EVIDENCE_DIR/manifest-${label}.stderr.log" >&2
    return 1
  fi
  ssh_run "$host" "sudo -u lichen cat '$remote_output'" >"$output"
  if [ "$mode" != "offline" ]; then
    ssh_run "$host" "sudo rm -rf '$secondary_dir'"
  fi
}

capture_preflight() {
  local host="$1"
  local label="$2"
  ssh_run "$host" "
    set -e
    echo host=\$(hostname)
    echo date_utc=\$(date -u +%Y-%m-%dT%H:%M:%SZ)
    echo service_active=\$(systemctl is-active '$SERVICE' 2>/dev/null || true)
    echo service_main_pid=\$(systemctl show '$SERVICE' -p MainPID --value 2>/dev/null || true)
    echo service_started=\$(systemctl show '$SERVICE' -p ExecMainStartTimestamp --value 2>/dev/null || true)
    echo validator_sha256=\$(sha256sum '$VALIDATOR_BIN' 2>/dev/null | awk '{print \$1}' || true)
    echo validator_version=\$('$VALIDATOR_BIN' --version 2>/dev/null || true)
    echo state_dir='$STATE_DIR'
    echo cold_dir='$COLD_DIR'
    echo state_du_bytes=\$(sudo du -sb '$STATE_DIR' 2>/dev/null | awk '{print \$1}' || true)
    echo cold_du_bytes=\$(sudo du -sb '$COLD_DIR' 2>/dev/null | awk '{print \$1}' || true)
    echo df_begin
    df -h / '$STATE_DIR' '$COLD_DIR' 2>/dev/null || df -h /
    echo df_end
    echo mounts_begin
    findmnt -R /var/lib/lichen 2>/dev/null || true
    echo mounts_end
  " >"$EVIDENCE_DIR/preflight-${label}.txt"
}

start_services() {
  local -a pids=()
  for host in $HOSTS; do
    echo "Starting $SERVICE on $host"
    ssh_run "$host" "
      set -e
      sudo systemctl start '$SERVICE'
      for attempt in \$(seq 1 30); do
        if systemctl is-active --quiet '$SERVICE'; then exit 0; fi
        sleep 1
      done
      exit 1
    " &
    pids+=("$!")
  done
  wait_pids "${pids[@]}"
}

if [ "$STOP_FOR_MANIFEST" = "1" ]; then
  host_list_csv="${HOSTS// /,}"
  stop_confirmation="archive-parity-stop:${NETWORK}:${host_list_csv}"
  if [ "${LICHEN_ARCHIVE_PARITY_STOP_CONFIRM:-}" != "$stop_confirmation" ]; then
    echo "Refusing to stop validators without exact confirmation." >&2
    echo "Set:" >&2
    echo "  export LICHEN_ARCHIVE_PARITY_STOP_CONFIRM='$stop_confirmation'" >&2
    exit 2
  fi
fi

{
  echo "run_id=$RUN_ID"
  echo "network=$NETWORK"
  echo "hosts=$HOSTS"
  echo "service=$SERVICE"
  echo "rpc_port=$RPC_PORT"
  echo "state_dir=$STATE_DIR"
  echo "cold_dir=$COLD_DIR"
  echo "skip_manifest=$SKIP_MANIFEST"
  echo "stop_for_manifest=$STOP_FOR_MANIFEST"
  echo "offline_repair_gate=$OFFLINE_REPAIR_GATE"
  echo "live_manifest=$LIVE_MANIFEST"
  echo "categories=${CATEGORIES:-default}"
  echo "chunk_size=$CHUNK_SIZE"
  echo "nofile_limit=$NOFILE_LIMIT"
  echo "max_slot_spread=$MAX_SLOT_SPREAD"
  echo "ssh_connection_reuse=control-master"
  echo "ssh_retry_delay_secs=$SSH_RETRY_DELAY_SECS"
  echo "repo_head=$(git rev-parse HEAD 2>/dev/null || true)"
} >"$EVIDENCE_DIR/context.txt"

echo "Archive parity verifier evidence: $EVIDENCE_DIR"
echo "Network: $NETWORK"
echo "Hosts: $HOSTS"

echo "Capturing host preflight and RPC health"
preflight_pids=()
for host in $HOSTS; do
  label="$(host_label "$host")"
  echo "  - $label ($host)"
  (
    capture_preflight "$host" "$label"
    rpc_call "$host" "getHealth" "[]" "$EVIDENCE_DIR/rpc-getHealth-${label}.json" || true
    rpc_call "$host" "getSlot" "[]" "$EVIDENCE_DIR/rpc-getSlot-${label}.json" || true
    rpc_call "$host" "getLatestBlock" "[]" "$EVIDENCE_DIR/rpc-getLatestBlock-${label}.json" || true
    rpc_call "$host" "getMetrics" "[]" "$EVIDENCE_DIR/rpc-getMetrics-${label}.json" || true
  ) &
  preflight_pids+=("$!")
done
wait_pids "${preflight_pids[@]}"

if [ "$SKIP_MANIFEST" != "1" ]; then
  manifest_mode="live"
  if [ "$STOP_FOR_MANIFEST" = "1" ]; then
    manifest_mode="offline"
    echo "Stopping all validators for strict offline manifest parity"
    stop_pids=()
    for host in $HOSTS; do
      echo "  - stopping $host"
      ssh_run "$host" "sudo systemctl stop '$SERVICE'" &
      stop_pids+=("$!")
    done
    wait_pids "${stop_pids[@]}"
    sleep 2
  fi

  echo "Computing public-history manifests concurrently ($manifest_mode)"
  manifest_pids=()
  for host in $HOSTS; do
    label="$(host_label "$host")"
    echo "  - $label ($host)"
    remote_manifest "$host" "$label" "$manifest_mode" &
    manifest_pids+=("$!")
  done
  wait_pids "${manifest_pids[@]}"

  if [ "$STOP_FOR_MANIFEST" = "1" ]; then
    expected_manifest_count="$(wc -w <<<"$HOSTS" | xargs)"
    python3 - "$EVIDENCE_DIR" "$expected_manifest_count" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
expected_count = int(sys.argv[2])
paths = sorted(root.glob("manifest-*.json"))
if len(paths) != expected_count:
    raise SystemExit(
        f"offline manifest precheck found {len(paths)} manifests, expected {expected_count}"
    )
manifests = [json.loads(path.read_text()) for path in paths]
roots = [manifest.get("manifest_root") for manifest in manifests]
last_slots = [manifest.get("last_slot") for manifest in manifests]
if not roots or not roots[0] or any(value != roots[0] for value in roots):
    raise SystemExit("offline manifest roots differ; validators remain stopped")
if any(value != last_slots[0] for value in last_slots):
    raise SystemExit("offline manifest last slots differ; validators remain stopped")
print(f"Offline manifest precheck passed: root={roots[0]} last_slot={last_slots[0]}")
PY
  fi

  if [ "$STOP_FOR_MANIFEST" = "1" ] && [ "$OFFLINE_REPAIR_GATE" != "1" ]; then
    echo "Restarting validators after offline manifest capture"
    start_services
    sleep 5
    for host in $HOSTS; do
      label="$(host_label "$host")"
      rpc_call "$host" "getHealth" "[]" "$EVIDENCE_DIR/rpc-post-getHealth-${label}.json" || true
      rpc_call "$host" "getSlot" "[]" "$EVIDENCE_DIR/rpc-post-getSlot-${label}.json" || true
    done
  elif [ "$OFFLINE_REPAIR_GATE" = "1" ]; then
    echo "Offline repair gate: all validator services remain stopped pending parity decision"
  fi
fi

slot_probe_csv="$SAMPLE_SLOTS"
if [ -z "$slot_probe_csv" ]; then
  # HOSTS is intentionally expanded as words here so the helper receives the
  # same host list used by the collection loops.
  # shellcheck disable=SC2086
  slot_probe_csv="$(python3 - "$EVIDENCE_DIR" "$NETWORK" $HOSTS <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
network = sys.argv[2]
hosts = sys.argv[3:]

def label(host):
    return {
        "15.204.229.189": "us",
        "37.59.97.61": "eu",
        "15.235.142.253": "sea",
        "148.113.43.247": "in",
        "seed-01.lichen.network": "us",
        "seed-02.lichen.network": "eu",
        "seed-03.lichen.network": "sea",
        "seed-04.lichen.network": "in",
    }.get(host, "".join(ch if ch.isalnum() or ch in "_-" else "_" for ch in host))

slots = []
for host in hosts:
    path = root / f"rpc-getSlot-{label(host)}.json"
    try:
        payload = json.loads(path.read_text())
        value = payload.get("result")
        if isinstance(value, int):
            slots.append(value)
    except Exception:
        pass

if not slots:
    print("0")
else:
    tip = max(0, min(slots) - 2)
    existing_testnet_hosts = {
        "15.204.229.189",
        "37.59.97.61",
        "15.235.142.253",
        "148.113.43.247",
    }
    normalized_hosts = {
        {
            "seed-01.lichen.network": "15.204.229.189",
            "seed-02.lichen.network": "37.59.97.61",
            "seed-03.lichen.network": "15.235.142.253",
            "seed-04.lichen.network": "148.113.43.247",
        }.get(host, host)
        for host in hosts
    }
    if network == "testnet" and normalized_hosts == existing_testnet_hosts:
        # Existing lichen-testnet-1 has one explicit legacy-loss interval.
        # Probe the first available post-gap block, the repaired former
        # singleton exception, a known July boundary, and two fresh slots.
        candidates = [4_299_000, 5_276_000, 9_236_790, max(0, tip - 1000), tip]
    else:
        # Fresh testnets and mainnet have no waiver and must expose genesis.
        candidates = [0, 1, tip // 2, max(0, tip - 1000), tip]
    seen = []
    for slot in candidates:
        if slot not in seen:
            seen.append(slot)
    print(",".join(str(slot) for slot in seen))
PY
)"
fi

echo "Running historical RPC probes"
slots_json="$(csv_to_json_number_array "$slot_probe_csv")"
txs_json="$(csv_to_json_array "$SAMPLE_TXS")"
addresses_json="$(csv_to_json_array "$SAMPLE_ADDRESSES")"
if [ "$OFFLINE_REPAIR_GATE" != "1" ]; then
  probe_pids=()
  for host in $HOSTS; do
    label="$(host_label "$host")"
    capture_historical_rpc_probes "$host" "$label" "$slots_json" "$txs_json" "$addresses_json" &
    probe_pids+=("$!")
  done
  wait_pids "${probe_pids[@]}" || true
else
  echo "Skipping RPC probes while validators remain stopped"
fi

# HOSTS is intentionally expanded into one argument per validator.
# shellcheck disable=SC2086
python3 - "$EVIDENCE_DIR" "$MAX_SLOT_SPREAD" "$SKIP_MANIFEST" "$STOP_FOR_MANIFEST" "$OFFLINE_REPAIR_GATE" "$slot_probe_csv" "$SAMPLE_TXS" "$SAMPLE_ADDRESSES" $HOSTS <<'PY'
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
max_slot_spread = int(sys.argv[2])
skip_manifest = sys.argv[3] == "1"
stop_for_manifest = sys.argv[4] == "1"
offline_repair_gate = sys.argv[5] == "1"
slot_probe_csv = sys.argv[6]
sample_txs = [part.strip() for part in sys.argv[7].split(",") if part.strip()]
sample_addresses = [part.strip() for part in sys.argv[8].split(",") if part.strip()]
hosts = sys.argv[9:]

def label(host):
    labels = {
        "15.204.229.189": "us",
        "37.59.97.61": "eu",
        "15.235.142.253": "sea",
        "148.113.43.247": "in",
        "seed-01.lichen.network": "us",
        "seed-02.lichen.network": "eu",
        "seed-03.lichen.network": "sea",
        "seed-04.lichen.network": "in",
    }
    return labels.get(host, "".join(ch if ch.isalnum() or ch in "_-" else "_" for ch in host))

def read_json(path):
    try:
        return json.loads(path.read_text())
    except Exception as exc:
        return {"_read_error": str(exc)}

def digest_result(payload):
    if "_read_error" in payload:
        return None
    if "error" in payload and payload["error"]:
        return None
    if "result" not in payload or payload["result"] is None:
        return None
    result = payload["result"]
    if isinstance(result, dict) and {"slot", "hash", "parent_hash", "state_root", "tx_root"}.issubset(result):
        result = dict(result)
        for key in tuple(result):
            if key.startswith("commit_"):
                result.pop(key)
    raw = json.dumps(result, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(raw).hexdigest()

def read_probe_payload(name, pattern, label_name):
    aggregate = read_json(root / f"rpc-probes-{label_name}.json")
    if isinstance(aggregate, dict) and name in aggregate:
        return aggregate[name]
    return read_json(root / pattern.format(label=label_name))

summary = {
    "hosts": {},
    "max_slot_spread": max_slot_spread,
    "skip_manifest": skip_manifest,
    "stop_for_manifest": stop_for_manifest,
    "offline_repair_gate": offline_repair_gate,
    "health_ok": False,
    "manifest_roots_match": None,
    "manifest_last_slots_match": None,
    "slot_spread_ok": False,
    "rpc_probes_match": True,
    "errors": [],
}

slots = []
health_ok = True
for host in hosts:
    name = label(host)
    health = read_json(root / f"rpc-getHealth-{name}.json")
    slot_payload = read_json(root / f"rpc-getSlot-{name}.json")
    latest = read_json(root / f"rpc-getLatestBlock-{name}.json")
    slot = slot_payload.get("result")
    if isinstance(slot, int):
        slots.append(slot)
    summary["hosts"][name] = {
        "host": host,
        "health": health.get("result", health),
        "slot": slot,
        "latest_block_present": "result" in latest and latest.get("result") is not None,
    }
    health_result = health.get("result")
    if not offline_repair_gate and (
        not isinstance(health_result, dict) or health_result.get("status") != "ok"
    ):
        health_ok = False
        summary["errors"].append(f"{name}: getHealth is not ok")
    if not skip_manifest:
        manifest = read_json(root / f"manifest-{name}.json")
        summary["hosts"][name]["manifest_root"] = manifest.get("manifest_root")
        summary["hosts"][name]["manifest_last_slot"] = manifest.get("last_slot")
        if "_read_error" in manifest:
            summary["errors"].append(f"{name}: manifest parse failed: {manifest['_read_error']}")

if slots:
    summary["slot_min"] = min(slots)
    summary["slot_max"] = max(slots)
    summary["slot_spread"] = max(slots) - min(slots)
    summary["slot_spread_ok"] = summary["slot_spread"] <= max_slot_spread
elif not offline_repair_gate:
    summary["errors"].append("no getSlot responses were usable")
summary["health_ok"] = health_ok if not offline_repair_gate else None

if not skip_manifest:
    roots = [data.get("manifest_root") for data in summary["hosts"].values()]
    last_slots = [data.get("manifest_last_slot") for data in summary["hosts"].values()]
    roots_ok = bool(roots) and all(root_value and root_value == roots[0] for root_value in roots)
    last_slots_ok = bool(last_slots) and all(slot == last_slots[0] for slot in last_slots)
    summary["manifest_roots_match"] = roots_ok
    summary["manifest_last_slots_match"] = last_slots_ok
    if not roots_ok:
        summary["errors"].append("public-history manifest roots differ across validators")
    if not stop_for_manifest and not last_slots_ok:
        summary["errors"].append(
            "live manifest last_slot differs across validators; rerun with --stop-for-manifest for the strict release gate"
        )

probe_groups = []
for slot in [part.strip() for part in slot_probe_csv.split(",") if part.strip()]:
    probe_groups.append((f"getBlock:{slot}", f"rpc-getBlock-slot-{slot}-{{label}}.json"))
for tx in sample_txs:
    probe_groups.append((f"getTransaction:{tx}", f"rpc-getTransaction-{tx}-{{label}}.json"))
for address in sample_addresses:
    probe_groups.append(
        (
            f"getTransactionsByAddress:{address}",
            f"rpc-getTransactionsByAddress-{address}-{{label}}.json",
        )
    )
if offline_repair_gate:
    probe_groups = []

summary["rpc_probe_digests"] = {}
for probe_name, pattern in probe_groups:
    digests = {}
    for host in hosts:
        name = label(host)
        payload = read_probe_payload(probe_name, pattern, name)
        digest = digest_result(payload)
        digests[name] = digest
        if digest is None:
            err = (
                payload.get("error")
                or payload.get("_transport_error")
                or payload.get("_parse_error")
                or payload.get("_read_error")
                or "missing result"
            )
            summary["errors"].append(f"{probe_name} failed on {name}: {err}")
    non_empty = [value for value in digests.values() if value]
    matches = bool(non_empty) and all(value == non_empty[0] for value in non_empty)
    if not matches or len(non_empty) != len(hosts):
        summary["rpc_probes_match"] = False
    summary["rpc_probe_digests"][probe_name] = {
        "matches": matches and len(non_empty) == len(hosts),
        "digests": digests,
    }

if offline_repair_gate:
    passed = (
        not skip_manifest
        and summary["manifest_roots_match"]
        and summary["manifest_last_slots_match"]
        and not summary["errors"]
    )
else:
    passed = (
        summary["health_ok"]
        and summary["slot_spread_ok"]
        and summary["rpc_probes_match"]
        and (skip_manifest or summary["manifest_roots_match"])
        and (not stop_for_manifest or summary["manifest_last_slots_match"])
        and not summary["errors"]
    )
summary["passed"] = passed

(root / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
print(json.dumps(summary, indent=2, sort_keys=True))
sys.exit(0 if passed else 1)
PY

echo "Archive parity verification complete. Evidence: $EVIDENCE_DIR"
