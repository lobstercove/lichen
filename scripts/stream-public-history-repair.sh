#!/usr/bin/env bash
set -euo pipefail

# Stream public-history pages from one verified source validator into target
# validators. This does not copy source RocksDB into target state; the source
# exports whitelisted public-history pages and each target imports them
# additively, skipping identical rows and aborting on conflicts.

usage() {
  cat >&2 <<'EOF'
Usage:
  bash scripts/stream-public-history-repair.sh [options]

Default mode is dry-run. Execute mode is fail-closed and stops one target at a time.

Options:
  --network <testnet|mainnet>       Network to repair (default: testnet)
  --source <host>                   Verified source host (default: EU)
  --targets "<host host ...>"       Target host list
  --categories <csv>                Public-history categories to stream
  --from-slot <slot>                Start slot for slot-driven range repair
  --to-slot <slot>                  Stop after the exported cursor reaches slot
  --chunk-size <n>                  Page size (default: 1000)
  --block-chunk-size <n>            Block-body page size (default: min(chunk-size, 2000))
  --page-format <json|binary>       Source page format (default: binary)
  --remote-bin <path>               Candidate validator binary on each host
  --evidence-dir <path>             Evidence output directory
  --leave-target-stopped            Required in execute mode; keep targets stopped
  --execute                         Apply writes; requires exact confirmation
  --help                            Show this help

Execute confirmation:
  export LICHEN_PUBLIC_HISTORY_STREAM_CONFIRM='<printed string>'
  export LICHEN_PUBLIC_HISTORY_BACKUP_CONFIRM='<printed string>'

Block repair requirements:
  Execute mode requires explicit --from-slot and --to-slot bounds. The source
  range must pass --verify-contiguous-block-range before any target is stopped.
EOF
}

NETWORK="testnet"
SOURCE_HOST=""
TARGET_HOSTS=""
CATEGORIES="${LICHEN_PUBLIC_HISTORY_STREAM_CATEGORIES:-}"
CHUNK_SIZE="${LICHEN_PUBLIC_HISTORY_STREAM_CHUNK_SIZE:-1000}"
BLOCK_CHUNK_SIZE="${LICHEN_PUBLIC_HISTORY_STREAM_BLOCK_CHUNK_SIZE:-}"
FROM_SLOT="${LICHEN_PUBLIC_HISTORY_STREAM_FROM_SLOT:-}"
TO_SLOT="${LICHEN_PUBLIC_HISTORY_STREAM_TO_SLOT:-}"
REMOTE_BIN="${LICHEN_PUBLIC_HISTORY_STREAM_REMOTE_BIN:-/tmp/lichen-validator-0.5.224-candidate}"
EVIDENCE_DIR=""
EXECUTE="${LICHEN_PUBLIC_HISTORY_STREAM_EXECUTE:-0}"
KEEP_SOURCE_PAGES="${LICHEN_PUBLIC_HISTORY_STREAM_KEEP_SOURCE_PAGES:-0}"
COMPRESS_SOURCE_PAGES="${LICHEN_PUBLIC_HISTORY_STREAM_COMPRESS_SOURCE_PAGES:-1}"
PAGE_FORMAT="${LICHEN_PUBLIC_HISTORY_STREAM_PAGE_FORMAT:-binary}"
FRAME_STREAM_PAGES="${LICHEN_PUBLIC_HISTORY_STREAM_FRAME_STREAM_PAGES:-1}"
LEAVE_TARGET_STOPPED="${LICHEN_PUBLIC_HISTORY_LEAVE_TARGET_STOPPED:-0}"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --network)
      NETWORK="${2:?--network requires a value}"
      shift 2
      ;;
    --source)
      SOURCE_HOST="${2:?--source requires a value}"
      shift 2
      ;;
    --targets)
      TARGET_HOSTS="${2:?--targets requires a value}"
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
    --block-chunk-size)
      BLOCK_CHUNK_SIZE="${2:?--block-chunk-size requires a value}"
      shift 2
      ;;
    --page-format)
      PAGE_FORMAT="${2:?--page-format requires a value}"
      shift 2
      ;;
    --from-slot)
      FROM_SLOT="${2:?--from-slot requires a value}"
      shift 2
      ;;
    --to-slot)
      TO_SLOT="${2:?--to-slot requires a value}"
      shift 2
      ;;
    --remote-bin)
      REMOTE_BIN="${2:?--remote-bin requires a value}"
      shift 2
      ;;
    --evidence-dir)
      EVIDENCE_DIR="${2:?--evidence-dir requires a value}"
      shift 2
      ;;
    --leave-target-stopped)
      LEAVE_TARGET_STOPPED=1
      shift
      ;;
    --execute)
      EXECUTE=1
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

case "$NETWORK" in
  testnet)
    SERVICE="lichen-validator-testnet"
    STATE_DIR="/var/lib/lichen/state-testnet"
    COLD_DIR="/var/lib/lichen/archive-testnet"
    SOURCE_HOST="${SOURCE_HOST:-37.59.97.61}"
    TARGET_HOSTS="${TARGET_HOSTS:-15.204.229.189 15.235.142.253 148.113.43.247}"
    ;;
  mainnet)
    SERVICE="lichen-validator-mainnet"
    STATE_DIR="/var/lib/lichen/state-mainnet"
    COLD_DIR="/var/lib/lichen/archive-mainnet"
    SOURCE_HOST="${SOURCE_HOST:-${LICHEN_PUBLIC_HISTORY_STREAM_SOURCE:-}}"
    TARGET_HOSTS="${TARGET_HOSTS:-${LICHEN_PUBLIC_HISTORY_STREAM_TARGETS:-}}"
    ;;
  *)
    echo "Unsupported network: $NETWORK" >&2
    exit 2
    ;;
esac

if [ -z "$SOURCE_HOST" ] || [ -z "$TARGET_HOSTS" ]; then
  echo "Source and targets are required." >&2
  exit 2
fi

if ! [[ "$CHUNK_SIZE" =~ ^[0-9]+$ ]] || [ "$CHUNK_SIZE" -lt 1 ]; then
  echo "--chunk-size must be a positive integer" >&2
  exit 2
fi
if [ -z "$BLOCK_CHUNK_SIZE" ]; then
  if [ "$CHUNK_SIZE" -gt 2000 ]; then
    BLOCK_CHUNK_SIZE=2000
  else
    BLOCK_CHUNK_SIZE="$CHUNK_SIZE"
  fi
fi
if ! [[ "$BLOCK_CHUNK_SIZE" =~ ^[0-9]+$ ]] || [ "$BLOCK_CHUNK_SIZE" -lt 1 ]; then
  echo "--block-chunk-size must be a positive integer" >&2
  exit 2
fi
case "$PAGE_FORMAT" in
  json|binary) ;;
  *)
    echo "--page-format must be json or binary" >&2
    exit 2
    ;;
esac

DEFAULT_CATEGORIES="slots,blocks,transactions,tx_by_slot,tx_to_slot,tx_meta,account_txs,events_by_slot,events,token_transfers,program_calls,evm_txs,evm_receipts,evm_logs_by_slot,shielded_txs,nft_activity,market_activity,dex_trades_by_pair,dex_trades_by_taker,dex_trades_by_pair_taker,account_snapshots"
CATEGORIES="${CATEGORIES:-$DEFAULT_CATEGORIES}"

RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)"
EVIDENCE_DIR="${EVIDENCE_DIR:-evidence/public-history-stream-repair/${NETWORK}-${RUN_ID}}"
SSH_USER="${LICHEN_PUBLIC_HISTORY_STREAM_SSH_USER:-ubuntu}"
SSH_PORT="${LICHEN_PUBLIC_HISTORY_STREAM_SSH_PORT:-2222}"
SSH_CONNECT_TIMEOUT="${LICHEN_PUBLIC_HISTORY_STREAM_SSH_CONNECT_TIMEOUT:-15}"
SSH_ATTEMPTS="${LICHEN_PUBLIC_HISTORY_STREAM_SSH_ATTEMPTS:-5}"
SSH_RETRY_DELAY_SECS="${LICHEN_PUBLIC_HISTORY_STREAM_SSH_RETRY_DELAY_SECS:-2}"
SSH_STRICT_HOST_KEY_CHECKING="${LICHEN_PUBLIC_HISTORY_STREAM_STRICT_HOST_KEY_CHECKING:-yes}"
SSH_KNOWN_HOSTS_FILE="${LICHEN_PUBLIC_HISTORY_STREAM_KNOWN_HOSTS_FILE:-$HOME/.ssh/known_hosts}"
SSH_CONTROL_DIR="$(mktemp -d /tmp/lichen-ph-repair-ssh.XXXXXX)"
CACHE_SIZE_MB="${LICHEN_PUBLIC_HISTORY_STREAM_CACHE_SIZE_MB:-256}"
NOFILE_LIMIT="${LICHEN_PUBLIC_HISTORY_STREAM_NOFILE_LIMIT:-1048576}"
REQUIRED_FREE_RESERVE_BYTES="${LICHEN_PUBLIC_HISTORY_FREE_RESERVE_BYTES:-10737418240}"
WRITE_HEADROOM_PERCENT="${LICHEN_PUBLIC_HISTORY_WRITE_HEADROOM_PERCENT:-150}"

if ! [[ "$REQUIRED_FREE_RESERVE_BYTES" =~ ^[0-9]+$ ]]; then
  echo "LICHEN_PUBLIC_HISTORY_FREE_RESERVE_BYTES must be an unsigned integer" >&2
  exit 2
fi
if ! [[ "$WRITE_HEADROOM_PERCENT" =~ ^[0-9]+$ ]] || [ "$WRITE_HEADROOM_PERCENT" -lt 100 ]; then
  echo "LICHEN_PUBLIC_HISTORY_WRITE_HEADROOM_PERCENT must be an integer >= 100" >&2
  exit 2
fi

if [ -n "$FROM_SLOT" ] && ! [[ "$FROM_SLOT" =~ ^[0-9]+$ ]]; then
  echo "--from-slot must be an unsigned integer" >&2
  exit 2
fi
if [ -n "$TO_SLOT" ] && ! [[ "$TO_SLOT" =~ ^[0-9]+$ ]]; then
  echo "--to-slot must be an unsigned integer" >&2
  exit 2
fi
if [ -n "$FROM_SLOT" ] && [ -n "$TO_SLOT" ] && [ "$TO_SLOT" -lt "$FROM_SLOT" ]; then
  echo "--to-slot must be >= --from-slot" >&2
  exit 2
fi

mkdir -p "$EVIDENCE_DIR"
exec > >(tee "$EVIDENCE_DIR/run.log") 2>&1

close_ssh_controls() {
  local host
  for host in $SOURCE_HOST $TARGET_HOSTS; do
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
  exit "$status"
}

trap finalize EXIT

ssh_base() {
  ssh -p "$SSH_PORT" \
    -o BatchMode=yes \
    -o ConnectTimeout="$SSH_CONNECT_TIMEOUT" \
    -o ConnectionAttempts=1 \
    -o ServerAliveInterval=10 \
    -o ServerAliveCountMax=3 \
    -o StrictHostKeyChecking="$SSH_STRICT_HOST_KEY_CHECKING" \
    -o UserKnownHostsFile="$SSH_KNOWN_HOSTS_FILE" \
    -o LogLevel=ERROR \
    -o ControlMaster=auto \
    -o ControlPersist=600 \
    -o ControlPath="$SSH_CONTROL_DIR/%C" \
    "$@"
}

ssh_run() {
  local host="$1"
  shift
  local attempt status
  for attempt in $(seq 1 "$SSH_ATTEMPTS"); do
    if ssh_base "$SSH_USER@$host" "$@"; then
      return 0
    fi
    status=$?
    if [ "$attempt" -lt "$SSH_ATTEMPTS" ]; then
      sleep "$SSH_RETRY_DELAY_SECS"
    fi
  done
  return "$status"
}

ssh_run_stdin_file() {
  local input_file="$1"
  local host="$2"
  shift 2
  local attempt status
  for attempt in $(seq 1 "$SSH_ATTEMPTS"); do
    if ssh_base "$SSH_USER@$host" "$@" <"$input_file"; then
      return 0
    fi
    status=$?
    if [ "$attempt" -lt "$SSH_ATTEMPTS" ]; then
      sleep "$SSH_RETRY_DELAY_SECS"
    fi
  done
  return "$status"
}

ssh_run_stdin_json_file() {
  local input_file="$1"
  local host="$2"
  shift 2
  local attempt status
  for attempt in $(seq 1 "$SSH_ATTEMPTS"); do
    if [[ "$input_file" == *.gz ]]; then
      if gzip -dc "$input_file" | ssh_base "$SSH_USER@$host" "$@"; then
        return 0
      fi
    elif ssh_base "$SSH_USER@$host" "$@" <"$input_file"; then
      return 0
    fi
    status=$?
    if [ "$attempt" -lt "$SSH_ATTEMPTS" ]; then
      sleep "$SSH_RETRY_DELAY_SECS"
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

json_field() {
  local file="$1"
  local field="$2"
  python3 - "$file" "$field" <<'PY'
import gzip
import json
import sys

path, field = sys.argv[1], sys.argv[2]
is_gzip = path.endswith(".gz")
is_binary = path.endswith(".bin") or path.endswith(".bin.gz")
if is_binary:
    opener = gzip.open if is_gzip else open
    with opener(path, "rb") as fh:
        magic = fh.readline()
        if magic != b"lichen-public-history-page-binary-v1\n":
            raise SystemExit(f"invalid binary page magic in {path}")
        data = json.loads(fh.readline().decode("utf-8"))
else:
    opener = gzip.open if is_gzip else open
    with opener(path, "rt", encoding="utf-8") as fh:
        data = json.load(fh)
value = data
for part in field.split("."):
    value = value.get(part) if isinstance(value, dict) else None
if value is None:
    print("")
elif isinstance(value, bool):
    print("true" if value else "false")
else:
    print(value)
PY
}

range_supported_category() {
  case "$1" in
    slots|blocks|transactions|tx_by_slot|tx_to_slot|tx_meta) return 0 ;;
    *) return 1 ;;
  esac
}

category_chunk_size() {
  case "$1" in
    blocks) echo "$BLOCK_CHUNK_SIZE" ;;
    *) echo "$CHUNK_SIZE" ;;
  esac
}

source_page_suffix() {
  local suffix
  if [ "$PAGE_FORMAT" = "binary" ]; then
    suffix="bin"
  else
    suffix="json"
  fi
  if [ "$COMPRESS_SOURCE_PAGES" = "1" ]; then
    echo "$suffix.gz"
  else
    echo "$suffix"
  fi
}

stream_supported_category() {
  [ "$PAGE_FORMAT" = "binary" ] && [ "$FRAME_STREAM_PAGES" = "1" ]
}

cursor_for_range_start() {
  local category="$1"
  local from_slot="$2"
  if [ -z "$from_slot" ] || [ "$from_slot" = "0" ]; then
    return 0
  fi
  local previous_slot=$((from_slot - 1))
  case "$category" in
    slots|blocks)
      printf '%016x' "$previous_slot"
      ;;
    transactions|tx_by_slot|tx_to_slot|tx_meta)
      printf '%016xffffffffffffffff' "$previous_slot"
      ;;
    *)
      echo "Range repair does not support category $category" >&2
      return 2
      ;;
  esac
}

targets_csv="${TARGET_HOSTS// /,}"
confirm_string="stream-public-history-repair:${NETWORK}:${SOURCE_HOST}:${targets_csv}"
if [ "$EXECUTE" = "1" ] && [ "${LICHEN_PUBLIC_HISTORY_STREAM_CONFIRM:-}" != "$confirm_string" ]; then
  echo "Refusing execute without exact confirmation." >&2
  echo "Set:" >&2
  echo "  export LICHEN_PUBLIC_HISTORY_STREAM_CONFIRM='$confirm_string'" >&2
  exit 2
fi

if [ "$NETWORK" = "testnet" ]; then
  backup_hosts_csv="15.204.229.189,37.59.97.61,15.235.142.253,148.113.43.247"
else
  backup_hosts_csv="${SOURCE_HOST},${targets_csv}"
fi
backup_confirm_string="current-backups-verified:${NETWORK}:${backup_hosts_csv}"
if [ "$EXECUTE" = "1" ] && [ "${LICHEN_PUBLIC_HISTORY_BACKUP_CONFIRM:-}" != "$backup_confirm_string" ]; then
  echo "Refusing execute without confirmation that current provider backups exist." >&2
  echo "Set only after recording current backup/snapshot IDs for every host:" >&2
  echo "  export LICHEN_PUBLIC_HISTORY_BACKUP_CONFIRM='$backup_confirm_string'" >&2
  exit 2
fi
if [ "$EXECUTE" = "1" ] && [ "$LEAVE_TARGET_STOPPED" != "1" ]; then
  echo "Execute requires --leave-target-stopped; restart is allowed only after offline fleet parity." >&2
  exit 2
fi

{
  echo "run_id=$RUN_ID"
  echo "network=$NETWORK"
  echo "source=$SOURCE_HOST"
  echo "targets=$TARGET_HOSTS"
  echo "service=$SERVICE"
  echo "state_dir=$STATE_DIR"
  echo "cold_dir=$COLD_DIR"
  echo "remote_bin=$REMOTE_BIN"
  echo "categories=$CATEGORIES"
  echo "chunk_size=$CHUNK_SIZE"
  echo "block_chunk_size=$BLOCK_CHUNK_SIZE"
  echo "keep_source_pages=$KEEP_SOURCE_PAGES"
  echo "compress_source_pages=$COMPRESS_SOURCE_PAGES"
  echo "page_format=$PAGE_FORMAT"
  echo "frame_stream_pages=$FRAME_STREAM_PAGES"
  echo "leave_target_stopped=$LEAVE_TARGET_STOPPED"
  echo "from_slot=${FROM_SLOT:-}"
  echo "to_slot=${TO_SLOT:-}"
  echo "nofile_limit=$NOFILE_LIMIT"
  echo "required_free_reserve_bytes=$REQUIRED_FREE_RESERVE_BYTES"
  echo "write_headroom_percent=$WRITE_HEADROOM_PERCENT"
  echo "execute=$EXECUTE"
  echo "repo_head=$(git rev-parse HEAD 2>/dev/null || true)"
} >"$EVIDENCE_DIR/context.txt"

echo "Public-history stream repair evidence: $EVIDENCE_DIR"
echo "Network: $NETWORK"
echo "Source: $SOURCE_HOST"
echo "Targets: $TARGET_HOSTS"
echo "Mode: $([ "$EXECUTE" = "1" ] && echo execute || echo dry-run)"

echo "Preflight candidate binaries"
expected_binary_hash=""
expected_binary_version=""
for host in $SOURCE_HOST $TARGET_HOSTS; do
  label="$(host_label "$host")"
  preflight_file="$EVIDENCE_DIR/preflight-bin-${label}.txt"
  ssh_run "$host" "test -x '$REMOTE_BIN' && '$REMOTE_BIN' --version && sha256sum '$REMOTE_BIN'" \
    | tee "$preflight_file"
  binary_version="$(sed -n '1p' "$preflight_file")"
  binary_hash="$(awk 'END {print $1}' "$preflight_file")"
  if ! [[ "$binary_hash" =~ ^[0-9a-f]{64}$ ]]; then
    echo "Invalid candidate hash returned by $host" >&2
    exit 1
  fi
  if [ -z "$expected_binary_hash" ]; then
    expected_binary_hash="$binary_hash"
    expected_binary_version="$binary_version"
  elif [ "$binary_hash" != "$expected_binary_hash" ] || [ "$binary_version" != "$expected_binary_version" ]; then
    echo "Candidate binary mismatch on $host" >&2
    echo "Expected: $expected_binary_version $expected_binary_hash" >&2
    echo "Actual:   $binary_version $binary_hash" >&2
    exit 1
  fi
done

IFS=',' read -r -a CATEGORY_LIST <<< "$CATEGORIES"
HAS_BLOCKS=0
for raw_category in "${CATEGORY_LIST[@]}"; do
  category="$(echo "$raw_category" | xargs)"
  if [ "$category" = "blocks" ]; then
    HAS_BLOCKS=1
  fi
done

if [ "$EXECUTE" = "1" ] && [ "$HAS_BLOCKS" = "1" ] && { [ -z "$FROM_SLOT" ] || [ -z "$TO_SLOT" ]; }; then
  echo "Execute block repair requires explicit --from-slot and --to-slot bounds." >&2
  exit 2
fi

if [ "$HAS_BLOCKS" = "1" ] && [ -n "$FROM_SLOT" ] && [ -n "$TO_SLOT" ]; then
  source_range_dir="/tmp/lichen-public-history-range-${RUN_ID}"
  echo "Verifying source block bodies are contiguous for $FROM_SLOT..$TO_SLOT"
  if ! ssh_run "$SOURCE_HOST" "sudo rm -rf '$source_range_dir' && sudo -u lichen bash -lc 'ulimit -n $NOFILE_LIMIT 2>/dev/null || ulimit -n 65535 2>/dev/null || true; exec \"\$0\" \"\$@\"' '$REMOTE_BIN' \
      --no-watchdog \
      --network '$NETWORK' \
      --db-path '$STATE_DIR' \
      --secondary-dir '$source_range_dir' \
      --cache-size-mb '$CACHE_SIZE_MB' \
      --verify-contiguous-block-range \
      --from-slot '$FROM_SLOT' \
      --to-slot '$TO_SLOT'" | tee "$EVIDENCE_DIR/source-contiguous-block-range.json"; then
    ssh_run "$SOURCE_HOST" "sudo rm -rf '$source_range_dir'" || true
    echo "Source block range is incomplete; no target was stopped or modified." >&2
    exit 1
  fi
  ssh_run "$SOURCE_HOST" "sudo rm -rf '$source_range_dir'"
fi

if [ "$EXECUTE" = "1" ]; then
  preflight_dir="$EVIDENCE_DIR/execute-preflight-dry-run"
  preflight_args=(
    --network "$NETWORK"
    --source "$SOURCE_HOST"
    --targets "$TARGET_HOSTS"
    --categories "$CATEGORIES"
    --chunk-size "$CHUNK_SIZE"
    --block-chunk-size "$BLOCK_CHUNK_SIZE"
    --page-format "$PAGE_FORMAT"
    --remote-bin "$REMOTE_BIN"
    --evidence-dir "$preflight_dir"
  )
  if [ -n "$FROM_SLOT" ]; then
    preflight_args+=(--from-slot "$FROM_SLOT")
  fi
  if [ -n "$TO_SLOT" ]; then
    preflight_args+=(--to-slot "$TO_SLOT")
  fi

  echo "Running mandatory full target dry-run before execute"
  LICHEN_PUBLIC_HISTORY_STREAM_EXECUTE=0 \
    bash "$0" "${preflight_args[@]}"

  echo "Preflight target conflicts and measured write headroom"
  for target in $TARGET_HOSTS; do
    target_label="$(host_label "$target")"
    target_preflight_dir="$preflight_dir/$target_label"
    read -r inserted_bytes conflict_rows report_count schema_errors <<EOF
$(python3 - "$target_preflight_dir" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
inserted_bytes = 0
conflict_rows = 0
report_count = 0
schema_errors = 0
for path in root.rglob("*import.json"):
    with path.open("r", encoding="utf-8") as handle:
        payload = json.load(handle)
    report = payload.get("report", payload)
    if "inserted_bytes" not in report or "conflict_rows" not in report:
        schema_errors += 1
        continue
    inserted_bytes += int(report.get("inserted_bytes", 0))
    conflict_rows += int(report.get("conflict_rows", 0))
    report_count += 1
print(inserted_bytes, conflict_rows, report_count, schema_errors)
PY
)
EOF
    if [ "$schema_errors" -ne 0 ]; then
      echo "Dry-run produced $schema_errors report(s) without byte/conflict counters on $target_label; refusing execute." >&2
      exit 1
    fi
    if [ "$report_count" -lt 1 ]; then
      echo "No import reports were produced for $target_label; refusing execute." >&2
      exit 1
    fi
    if [ "$conflict_rows" -ne 0 ]; then
      echo "Dry-run found $conflict_rows conflict row(s) on $target_label; refusing execute." >&2
      exit 1
    fi

    storage_file="$EVIDENCE_DIR/preflight-storage-${target_label}.txt"
    ssh_run "$target" "df -PB1 '$COLD_DIR' | awk 'NR == 2 {print \$2, \$4}'" | tee "$storage_file"
    read -r total_bytes free_bytes <"$storage_file"
    if ! [[ "${total_bytes:-}" =~ ^[0-9]+$ ]] || ! [[ "${free_bytes:-}" =~ ^[0-9]+$ ]]; then
      echo "Could not read archive filesystem capacity from $target_label" >&2
      exit 1
    fi
    write_headroom_bytes=$((inserted_bytes * WRITE_HEADROOM_PERCENT / 100))
    required_free_bytes=$((REQUIRED_FREE_RESERVE_BYTES + write_headroom_bytes))
    {
      echo "inserted_bytes=$inserted_bytes"
      echo "write_headroom_bytes=$write_headroom_bytes"
      echo "required_free_reserve_bytes=$REQUIRED_FREE_RESERVE_BYTES"
      echo "required_free_bytes=$required_free_bytes"
      echo "available_bytes=$free_bytes"
      echo "total_bytes=$total_bytes"
    } | tee "$EVIDENCE_DIR/preflight-capacity-${target_label}.txt"
    if [ "$free_bytes" -lt "$required_free_bytes" ]; then
      echo "Measured capacity preflight failed on $target_label: free=$free_bytes required=$required_free_bytes" >&2
      exit 1
    fi
  done
fi

if [ -n "$FROM_SLOT" ] || [ -n "$TO_SLOT" ]; then
  for raw_category in "${CATEGORY_LIST[@]}"; do
    category="$(echo "$raw_category" | xargs)"
    [ -n "$category" ] || continue
    if ! range_supported_category "$category"; then
      echo "Range repair supports only slots,blocks,transactions,tx_by_slot,tx_to_slot,tx_meta; got $category" >&2
      exit 2
    fi
  done
fi

export_source_page() {
  local category="$1"
  local cursor="$2"
  local chunk_size_value="$3"
  local page_file="$4"
  local cursor_arg=""
  local to_slot_arg=""
  if [ -n "$cursor" ]; then
    cursor_arg="--after-key-hex '$cursor'"
  fi
  if [ -n "$TO_SLOT" ]; then
    to_slot_arg="--to-slot '$TO_SLOT'"
  fi

  local export_command="sudo -u lichen bash -lc 'ulimit -n $NOFILE_LIMIT 2>/dev/null || ulimit -n 65535 2>/dev/null || true; exec \"\$0\" \"\$@\"' '$REMOTE_BIN' \
    --no-watchdog \
    --network '$NETWORK' \
    --db-path '$STATE_DIR' \
    --secondary-dir '/tmp/lichen-public-history-export-${RUN_ID}-${category}' \
    --cache-size-mb '$CACHE_SIZE_MB' \
    --chunk-size '$chunk_size_value' \
    --public-history-page-format '$PAGE_FORMAT' \
    --export-public-history-category '$category' \
    $cursor_arg \
    $to_slot_arg"
  if [ "$COMPRESS_SOURCE_PAGES" = "1" ]; then
    local raw_page_file="${page_file%.gz}"
    ssh_run "$SOURCE_HOST" "$export_command" >"$raw_page_file"
    gzip -1 -f "$raw_page_file"
  else
    ssh_run "$SOURCE_HOST" "$export_command" >"$page_file"
  fi
}

import_page_dry_run() {
  local target="$1"
  local target_label="$2"
  local category="$3"
  local page_file="$4"
  local import_file="$5"

  ssh_run_stdin_json_file "$page_file" "$target" "sudo -u lichen bash -lc 'ulimit -n $NOFILE_LIMIT 2>/dev/null || ulimit -n 65535 2>/dev/null || true; exec \"\$0\" \"\$@\"' '$REMOTE_BIN' \
    --no-watchdog \
    --network '$NETWORK' \
    --db-path '$STATE_DIR' \
    --secondary-dir '/tmp/lichen-public-history-import-${RUN_ID}-${target_label}-${category}' \
    --cache-size-mb '$CACHE_SIZE_MB' \
    --public-history-page-format '$PAGE_FORMAT' \
    --import-public-history-category '$category' \
    --dry-run" >"$import_file"
}

import_page_execute() {
  local target="$1"
  local category="$2"
  local page_file="$3"
  local import_file="$4"

  ssh_run_stdin_json_file "$page_file" "$target" "sudo -u lichen bash -lc 'ulimit -n $NOFILE_LIMIT 2>/dev/null || ulimit -n 65535 2>/dev/null || true; exec \"\$0\" \"\$@\"' '$REMOTE_BIN' \
    --no-watchdog \
    --network '$NETWORK' \
    --db-path '$STATE_DIR' \
    --cache-size-mb '$CACHE_SIZE_MB' \
    --public-history-page-format '$PAGE_FORMAT' \
    --import-public-history-category '$category' \
    --execute \
    --confirm public-history-repair:v1" >"$import_file"
}

stream_category_to_target() {
  local target="$1"
  local target_label="$2"
  local category="$3"
  local mode="$4"
  local import_file="$5"
  local chunk_size_value
  local cursor
  local cursor_arg=""
  local to_slot_arg=""
  local target_secondary_arg=""
  local target_mode_args=""

  chunk_size_value="$(category_chunk_size "$category")"
  cursor="$(cursor_for_range_start "$category" "$FROM_SLOT")"
  if [ -n "$cursor" ]; then
    cursor_arg="--after-key-hex '$cursor'"
  fi
  if [ -n "$TO_SLOT" ]; then
    to_slot_arg="--to-slot '$TO_SLOT'"
  fi

  if [ "$mode" = "execute" ]; then
    target_mode_args="--execute --confirm public-history-repair:v1"
  else
    target_secondary_arg="--secondary-dir '/tmp/lichen-public-history-import-${RUN_ID}-${target_label}-${category}'"
    target_mode_args="--dry-run"
  fi

  local source_command="sudo -u lichen bash -lc 'ulimit -n $NOFILE_LIMIT 2>/dev/null || ulimit -n 65535 2>/dev/null || true; exec \"\$0\" \"\$@\"' '$REMOTE_BIN' \
    --no-watchdog \
    --network '$NETWORK' \
    --db-path '$STATE_DIR' \
    --secondary-dir '/tmp/lichen-public-history-export-${RUN_ID}-${category}' \
    --cache-size-mb '$CACHE_SIZE_MB' \
    --chunk-size '$chunk_size_value' \
    --public-history-page-format binary \
    --stream-pages \
    --export-public-history-category '$category' \
    $cursor_arg \
    $to_slot_arg"

  local target_command="sudo -u lichen bash -lc 'ulimit -n $NOFILE_LIMIT 2>/dev/null || ulimit -n 65535 2>/dev/null || true; exec \"\$0\" \"\$@\"' '$REMOTE_BIN' \
    --no-watchdog \
    --network '$NETWORK' \
    --db-path '$STATE_DIR' \
    $target_secondary_arg \
    --cache-size-mb '$CACHE_SIZE_MB' \
    --public-history-page-format binary \
    --stream-pages \
    --import-public-history-category '$category' \
    $target_mode_args"

  ssh_run "$SOURCE_HOST" "sudo rm -rf '/tmp/lichen-public-history-export-${RUN_ID}-${category}'"
  if [ "$mode" != "execute" ]; then
    ssh_run "$target" "sudo rm -rf '/tmp/lichen-public-history-import-${RUN_ID}-${target_label}-${category}'"
  fi

  echo "Streaming $category from $SOURCE_HOST to $target_label mode=$mode chunk_size=$chunk_size_value"
  if [ "$COMPRESS_SOURCE_PAGES" = "1" ]; then
    ssh_base "$SSH_USER@$SOURCE_HOST" "$source_command" \
      | gzip -1 \
      | ssh_base "$SSH_USER@$target" "gzip -dc | $target_command" >"$import_file"
  else
    ssh_base "$SSH_USER@$SOURCE_HOST" "$source_command" \
      | ssh_base "$SSH_USER@$target" "$target_command" >"$import_file"
  fi

  ssh_run "$SOURCE_HOST" "sudo rm -rf '/tmp/lichen-public-history-export-${RUN_ID}-${category}'"
  if [ "$mode" != "execute" ]; then
    ssh_run "$target" "sudo rm -rf '/tmp/lichen-public-history-import-${RUN_ID}-${target_label}-${category}'"
  fi
}

if [ "$EXECUTE" != "1" ]; then
  for raw_category in "${CATEGORY_LIST[@]}"; do
    category="$(echo "$raw_category" | xargs)"
    [ -n "$category" ] || continue
    chunk_size_value="$(category_chunk_size "$category")"
    source_category_dir="$EVIDENCE_DIR/source/$category"
    mkdir -p "$source_category_dir"

    if stream_supported_category "$category"; then
      for target in $TARGET_HOSTS; do
        target_label="$(host_label "$target")"
        target_category_dir="$EVIDENCE_DIR/$target_label/$category"
        mkdir -p "$target_category_dir"
        stream_category_to_target "$target" "$target_label" "$category" "dry-run" "$target_category_dir/stream-import.json"
      done
      continue
    fi

    ssh_run "$SOURCE_HOST" "sudo rm -rf '/tmp/lichen-public-history-export-${RUN_ID}-${category}'"
    for target in $TARGET_HOSTS; do
      target_label="$(host_label "$target")"
      target_category_dir="$EVIDENCE_DIR/$target_label/$category"
      mkdir -p "$target_category_dir"
      ssh_run "$target" "sudo rm -rf '/tmp/lichen-public-history-import-${RUN_ID}-${target_label}-${category}'"
    done

    cursor="$(cursor_for_range_start "$category" "$FROM_SLOT")"
    page_suffix="$(source_page_suffix)"
    page_index=0
    while :; do
      page_file="$source_category_dir/page-${page_index}-export.$page_suffix"
      echo "Exporting $category page $page_index from $SOURCE_HOST chunk_size=$chunk_size_value"
      export_source_page "$category" "$cursor" "$chunk_size_value" "$page_file"

      row_count="$(json_field "$page_file" row_count)"
      has_more="$(json_field "$page_file" has_more)"
      cursor="$(json_field "$page_file" next_cursor_hex)"

      if [ "${row_count:-0}" = "0" ] && [ "$has_more" != "true" ]; then
        echo "  $category page $page_index empty and complete"
      else
        for target in $TARGET_HOSTS; do
          target_label="$(host_label "$target")"
          target_category_dir="$EVIDENCE_DIR/$target_label/$category"
          import_file="$target_category_dir/page-${page_index}-import.json"
          echo "Importing $category page $page_index into $target_label rows=$row_count"
          import_page_dry_run "$target" "$target_label" "$category" "$page_file" "$import_file"
        done
      fi

      if [ "$KEEP_SOURCE_PAGES" != "1" ]; then
        rm -f "$page_file"
      fi

      if [ "$has_more" != "true" ]; then
        break
      fi
      if [ -z "$cursor" ]; then
        echo "Export reported has_more=true without next_cursor_hex for $category page $page_index" >&2
        exit 1
      fi
      page_index=$((page_index + 1))
    done

    ssh_run "$SOURCE_HOST" "sudo rm -rf '/tmp/lichen-public-history-export-${RUN_ID}-${category}'"
    for target in $TARGET_HOSTS; do
      target_label="$(host_label "$target")"
      ssh_run "$target" "sudo rm -rf '/tmp/lichen-public-history-import-${RUN_ID}-${target_label}-${category}'"
    done
  done

  echo "Public-history stream repair complete. Evidence: $EVIDENCE_DIR"
  exit 0
fi

for target in $TARGET_HOSTS; do
  target_label="$(host_label "$target")"
  target_dir="$EVIDENCE_DIR/$target_label"
  mkdir -p "$target_dir"

  if [ "$EXECUTE" = "1" ]; then
    echo "Stopping $SERVICE on $target for execute import"
    ssh_run "$target" "sudo systemctl stop '$SERVICE'"
  fi

  for raw_category in "${CATEGORY_LIST[@]}"; do
    category="$(echo "$raw_category" | xargs)"
    [ -n "$category" ] || continue
    chunk_size_value="$(category_chunk_size "$category")"
    category_dir="$target_dir/$category"
    source_category_dir="$EVIDENCE_DIR/source/$category"
    mkdir -p "$category_dir" "$source_category_dir"

    if stream_supported_category "$category"; then
      stream_category_to_target "$target" "$target_label" "$category" "execute" "$category_dir/stream-import.json"
      continue
    fi

    cursor="$(cursor_for_range_start "$category" "$FROM_SLOT")"
    page_suffix="$(source_page_suffix)"
    page_index=0
    ssh_run "$SOURCE_HOST" "sudo rm -rf '/tmp/lichen-public-history-export-${RUN_ID}-${category}'"
    if [ "$EXECUTE" != "1" ]; then
      ssh_run "$target" "sudo rm -rf '/tmp/lichen-public-history-import-${RUN_ID}-${target_label}-${category}'"
    fi

    while :; do
      page_file="$source_category_dir/page-${page_index}-export.$page_suffix"
      import_file="$category_dir/page-${page_index}-import.json"
      cursor_arg=""
      if [ -n "$cursor" ]; then
        cursor_arg="--after-key-hex '$cursor'"
      fi

      if [ -f "$page_file" ]; then
        echo "Reusing $category page $page_index from $SOURCE_HOST for $target_label"
      else
        echo "Exporting $category page $page_index from $SOURCE_HOST for $target_label chunk_size=$chunk_size_value"
        export_source_page "$category" "$cursor" "$chunk_size_value" "$page_file"
      fi

      row_count="$(json_field "$page_file" row_count)"
      has_more="$(json_field "$page_file" has_more)"
      cursor="$(json_field "$page_file" next_cursor_hex)"

      if [ "${row_count:-0}" = "0" ] && [ "$has_more" != "true" ]; then
        echo "  $category page $page_index empty and complete"
      else
        echo "Importing $category page $page_index into $target_label rows=$row_count"
        if [ "$EXECUTE" = "1" ]; then
          import_page_execute "$target" "$category" "$page_file" "$import_file"
        else
          import_page_dry_run "$target" "$target_label" "$category" "$page_file" "$import_file"
        fi
      fi

      if [ "$KEEP_SOURCE_PAGES" != "1" ]; then
        rm -f "$page_file"
      fi

      if [ "$has_more" != "true" ]; then
        break
      fi
      if [ -z "$cursor" ]; then
        echo "Export reported has_more=true without next_cursor_hex for $category page $page_index" >&2
        exit 1
      fi
      page_index=$((page_index + 1))
    done

    ssh_run "$SOURCE_HOST" "sudo rm -rf '/tmp/lichen-public-history-export-${RUN_ID}-${category}'"
    if [ "$EXECUTE" != "1" ]; then
      ssh_run "$target" "sudo rm -rf '/tmp/lichen-public-history-import-${RUN_ID}-${target_label}-${category}'"
    fi
  done

  if [ "$EXECUTE" = "1" ]; then
    echo "Leaving $SERVICE stopped on $target for fleet-level offline parity"
  fi
done

echo "Public-history stream repair complete. Evidence: $EVIDENCE_DIR"
