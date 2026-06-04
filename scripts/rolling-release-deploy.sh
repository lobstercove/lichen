#!/usr/bin/env bash
set -euo pipefail

# Non-destructive VPS release rollout.
#
# Usage:
#   LICHEN_RELEASE_TAG=v0.5.36 bash scripts/rolling-release-deploy.sh testnet
#   LICHEN_RELEASE_TAG=v0.5.36 bash scripts/rolling-release-deploy.sh mainnet
#   LICHEN_RELEASE_TAG=v0.5.36 LICHEN_VERIFY_RELEASE_ONLY=1 bash scripts/rolling-release-deploy.sh testnet
#
# This script installs an exact GitHub Release archive on each validator and
# restarts one validator at a time. It never deletes chain state.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
NETWORK="${1:-testnet}"
case "$NETWORK" in
  testnet)
    SERVICE="lichen-validator-testnet"
    RPC_PORT="8899"
    DEFAULT_HOSTS="15.204.229.189 37.59.97.61 15.235.142.253"
    ;;
  mainnet)
    SERVICE="lichen-validator-mainnet"
    RPC_PORT="9899"
    DEFAULT_HOSTS="${LICHEN_MAINNET_VPS_HOSTS:-}"
    ;;
  *)
    echo "Usage: LICHEN_RELEASE_TAG=vX.Y.Z $0 {testnet|mainnet}" >&2
    exit 2
    ;;
esac

RELEASE_TAG="${LICHEN_RELEASE_TAG:-}"
RELEASE_REPO="${LICHEN_RELEASE_REPO:-lobstercove/lichen}"
SSH_USER="${LICHEN_SSH_USER:-ubuntu}"
SSH_PORT="${LICHEN_SSH_PORT:-2222}"
HOSTS="${LICHEN_VPS_HOSTS:-$DEFAULT_HOSTS}"
DISK_CRITICAL_PCT="${LICHEN_DISK_CRITICAL_PCT:-85}"
MAX_BLOCK_AGE_SECS="${LICHEN_MAX_BLOCK_AGE_SECS:-15}"
DEX_SMOKE_TIMEOUT_SECS="${LICHEN_DEX_SMOKE_TIMEOUT_SECS:-90}"
ARTIFACT_DIR="${LICHEN_RELEASE_ARTIFACT_DIR:-/tmp/lichen-rolling-${NETWORK}-${RELEASE_TAG:-unset}}"
RELEASE_SIGNING_ADDRESS="${LICHEN_RELEASE_SIGNING_ADDRESS:-8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk}"

if [ -z "$RELEASE_TAG" ]; then
  echo "LICHEN_RELEASE_TAG is required." >&2
  exit 2
fi

if [ -z "$HOSTS" ]; then
  echo "No VPS hosts configured. Set LICHEN_VPS_HOSTS for ${NETWORK}." >&2
  exit 2
fi

if [ -n "${LICHEN_OWNER_APPROVED_RESET:-}" ] || [ -n "${LICHEN_CLEAN_SLATE_REDEPLOY_CONFIRM:-}" ]; then
  echo "Refusing rolling deploy while reset approval variables are set." >&2
  exit 2
fi

for tool in gh node sha256sum tar ssh scp; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing required tool: $tool" >&2
    exit 2
  fi
done

ssh_run() {
  local host="$1"
  shift
  ssh -p "$SSH_PORT" \
    -o BatchMode=yes \
    -o ConnectTimeout=10 \
    -o StrictHostKeyChecking=no \
    "$SSH_USER@$host" "$@"
}

scp_to() {
  local src="$1"
  local host="$2"
  local dst="$3"
  scp -O -P "$SSH_PORT" \
    -o BatchMode=yes \
    -o ConnectTimeout=10 \
    -o StrictHostKeyChecking=no \
    "$src" "$SSH_USER@$host:$dst"
}

archive_for_arch() {
  case "$1" in
    x86_64|amd64) echo "lichen-validator-linux-x86_64.tar.gz" ;;
    aarch64|arm64) echo "lichen-validator-linux-aarch64.tar.gz" ;;
    *)
      echo "Unsupported VPS architecture: $1" >&2
      return 1
      ;;
  esac
}

archive_root() {
  local archive="$1"
  tar tzf "$ARTIFACT_DIR/$archive" | awk -F/ 'NR==1 { print $1 }'
}

archive_bin_sha() {
  local archive="$1"
  local root="$2"
  local bin="$3"
  if tar tzf "$ARTIFACT_DIR/$archive" | grep -qx "$root/$bin"; then
    tar xOf "$ARTIFACT_DIR/$archive" "$root/$bin" |
      sha256sum |
      awk '{print $1}'
  fi
}

download_release_artifacts() {
  mkdir -p "$ARTIFACT_DIR"
  gh release download "$RELEASE_TAG" --repo "$RELEASE_REPO" \
    -p SHA256SUMS \
    -p SHA256SUMS.sig \
    -D "$ARTIFACT_DIR" \
    --clobber

  local archives=()
  for host in $HOSTS; do
    local arch
    arch="$(ssh_run "$host" "uname -m")"
    archives+=("$(archive_for_arch "$arch")")
  done

  local archive
  for archive in "${archives[@]}"; do
    if [ ! -f "$ARTIFACT_DIR/$archive" ]; then
      gh release download "$RELEASE_TAG" --repo "$RELEASE_REPO" \
        -p "$archive" \
        -D "$ARTIFACT_DIR" \
      --clobber
    fi
  done

  SHA256SUMS_FILE="$ARTIFACT_DIR/SHA256SUMS" \
  SHA256SUMS_SIG_FILE="$ARTIFACT_DIR/SHA256SUMS.sig" \
  RELEASE_SIGNING_ADDRESS="$RELEASE_SIGNING_ADDRESS" \
  PQ_MODULE_PATH="$REPO_ROOT/monitoring/shared/pq.mjs" \
  node --input-type=module <<'NODE'
import { readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

const { publicKeyToAddress, verifySignature } = await import(pathToFileURL(process.env.PQ_MODULE_PATH).href);
const message = new Uint8Array(await readFile(process.env.SHA256SUMS_FILE));
const signature = JSON.parse(await readFile(process.env.SHA256SUMS_SIG_FILE, 'utf8'));
const signer = await publicKeyToAddress(signature.public_key.bytes, signature.public_key.scheme_version || signature.scheme_version || 1);
if (signer !== process.env.RELEASE_SIGNING_ADDRESS) {
  throw new Error(`SHA256SUMS signer mismatch: got ${signer}, expected ${process.env.RELEASE_SIGNING_ADDRESS}`);
}
if (!(await verifySignature(signature, message, process.env.RELEASE_SIGNING_ADDRESS))) {
  throw new Error('SHA256SUMS PQ signature verification failed');
}
console.log(`SHA256SUMS PQ signature verified by ${signer}`);
NODE

  (cd "$ARTIFACT_DIR" && sha256sum -c SHA256SUMS --ignore-missing)
}

preflight_host() {
  local host="$1"
  echo "Preflight ${host}"
  ssh_run "$host" "NETWORK='$NETWORK' RPC_PORT='$RPC_PORT' DISK_CRITICAL_PCT='$DISK_CRITICAL_PCT' bash -s" <<'REMOTE'
set -euo pipefail
pct="$(df -P / | awk 'NR==2 { gsub(/%/, "", $5); print $5 }')"
if [ "$pct" -ge "$DISK_CRITICAL_PCT" ]; then
  echo "Root filesystem is ${pct}% full; refusing deploy."
  exit 1
fi

backups="$(sudo find /var/lib/lichen -maxdepth 1 -type d \
  \( -name "state-${NETWORK}-*" -o -name "*backup*" \) -print 2>/dev/null || true)"
if [ -n "$backups" ]; then
  echo "Non-live state backup directories must be moved off-host before deploy:"
  echo "$backups"
  exit 1
fi

sudo du -sh /var/lib/lichen /var/log/journal /var/log/sudo-io 2>/dev/null || true
curl -fsS "http://127.0.0.1:${RPC_PORT}/" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' >/dev/null
REMOTE
}

install_host() {
  local host="$1"
  local arch archive root expected_validator_sha expected_custody_sha expected_faucet_sha
  arch="$(ssh_run "$host" "uname -m")"
  archive="$(archive_for_arch "$arch")"
  root="$(archive_root "$archive")"
  expected_validator_sha="$(archive_bin_sha "$archive" "$root" lichen-validator)"
  expected_custody_sha="$(archive_bin_sha "$archive" "$root" lichen-custody)"
  expected_faucet_sha="$(archive_bin_sha "$archive" "$root" lichen-faucet)"

  echo "Install ${RELEASE_TAG} on ${host} (${archive})"
  scp_to "$ARTIFACT_DIR/$archive" "$host" "/tmp/$archive"
  ssh_run "$host" "NETWORK='$NETWORK' SERVICE='$SERVICE' ARCHIVE='/tmp/$archive' EXPECTED_VALIDATOR_SHA='$expected_validator_sha' EXPECTED_CUSTODY_SHA='$expected_custody_sha' EXPECTED_FAUCET_SHA='$expected_faucet_sha' bash -s" <<'REMOTE'
set -euo pipefail
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp" "$ARCHIVE"' EXIT
before_pid="$(systemctl show "$SERVICE" -p MainPID --value || true)"
before_start="$(systemctl show "$SERVICE" -p ExecMainStartTimestampMonotonic --value || true)"
tar xzf "$ARCHIVE" -C "$tmp"
root="$(find "$tmp" -mindepth 1 -maxdepth 1 -type d | head -1)"
if [ -z "$root" ]; then
  echo "Release archive did not contain a root directory."
  exit 1
fi

for bin in lichen-validator lichen-genesis lichen zk-prove; do
  test -x "$root/$bin"
  sudo install -m 755 "$root/$bin" "/usr/local/bin/$bin"
done

install_optional_service_bin() {
  local bin="$1"
  local expected_sha="$2"
  if [ -z "$expected_sha" ]; then
    return 0
  fi
  if [ ! -f "$root/$bin" ]; then
    echo "Release archive is missing expected service binary: $bin"
    exit 1
  fi
  sudo install -m 755 "$root/$bin" "/usr/local/bin/$bin"
}

install_optional_service_bin lichen-custody "$EXPECTED_CUSTODY_SHA"
install_optional_service_bin lichen-faucet "$EXPECTED_FAUCET_SHA"

if [ -f "$root/seeds.json" ]; then
  sudo install -m 644 "$root/seeds.json" /etc/lichen/seeds.json
  sudo install -d -m 750 -o lichen -g lichen "/var/lib/lichen/state-${NETWORK}"
  sudo install -m 644 -o lichen -g lichen "$root/seeds.json" "/var/lib/lichen/state-${NETWORK}/seeds.json"
fi

installed_sha="$(sha256sum /usr/local/bin/lichen-validator | awk '{print $1}')"
if [ "$installed_sha" != "$EXPECTED_VALIDATOR_SHA" ]; then
  echo "Installed validator hash mismatch: got ${installed_sha}, expected ${EXPECTED_VALIDATOR_SHA}"
  exit 1
fi

check_installed_bin_hash() {
  local bin="$1"
  local expected_sha="$2"
  local installed_bin_sha
  if [ -z "$expected_sha" ]; then
    return 0
  fi
  installed_bin_sha="$(sha256sum "/usr/local/bin/$bin" | awk '{print $1}')"
  if [ "$installed_bin_sha" != "$expected_sha" ]; then
    echo "Installed ${bin} hash mismatch: got ${installed_bin_sha}, expected ${expected_sha}"
    exit 1
  fi
}

for bin in lichen-validator lichen-genesis lichen zk-prove; do
  expected_bin_sha="$(sha256sum "$root/$bin" | awk '{print $1}')"
  check_installed_bin_hash "$bin" "$expected_bin_sha"
done
check_installed_bin_hash lichen-custody "$EXPECTED_CUSTODY_SHA"
check_installed_bin_hash lichen-faucet "$EXPECTED_FAUCET_SHA"

sudo systemctl stop "$SERVICE" || true
sleep 2
if systemctl is-active --quiet "$SERVICE"; then
  echo "Service still active after stop; killing service control group before restart."
  sudo systemctl kill --kill-who=control-group -s SIGKILL "$SERVICE" || true
  sleep 2
fi
sudo systemctl start "$SERVICE"

for _ in $(seq 1 60); do
  after_pid="$(systemctl show "$SERVICE" -p MainPID --value || true)"
  after_start="$(systemctl show "$SERVICE" -p ExecMainStartTimestampMonotonic --value || true)"
  active="$(systemctl show "$SERVICE" -p ActiveState --value || true)"
  if [ "$active" = "active" ] && [ -n "$after_pid" ] && [ "$after_pid" != "0" ] && [ "$after_pid" != "$before_pid" ] && [ "$after_start" != "$before_start" ]; then
    break
  fi
  sleep 1
done

after_pid="$(systemctl show "$SERVICE" -p MainPID --value || true)"
after_start="$(systemctl show "$SERVICE" -p ExecMainStartTimestampMonotonic --value || true)"
active="$(systemctl show "$SERVICE" -p ActiveState --value || true)"
if [ "$active" != "active" ] || [ -z "$after_pid" ] || [ "$after_pid" = "0" ]; then
  echo "Service did not become active after restart."
  exit 1
fi
if [ "$after_pid" = "$before_pid" ] || [ "$after_start" = "$before_start" ]; then
  echo "Service restart did not replace the running process: before_pid=${before_pid} after_pid=${after_pid}."
  exit 1
fi

service_pids="$after_pid $(pgrep -P "$after_pid" || true)"
for pid in $service_pids; do
  exe_target="$(sudo readlink "/proc/${pid}/exe" 2>/dev/null || true)"
  if [[ "$exe_target" == *" (deleted)" ]]; then
    echo "Running validator process ${pid} still uses deleted executable: ${exe_target}"
    exit 1
  fi
  exe_sha="$(sudo sha256sum "/proc/${pid}/exe" 2>/dev/null | awk '{print $1}')"
  if [ "$exe_sha" != "$EXPECTED_VALIDATOR_SHA" ]; then
    echo "Running validator process ${pid} hash mismatch: exe=${exe_target} got=${exe_sha:-unreadable} expected=${EXPECTED_VALIDATOR_SHA}"
    exit 1
  fi
done
REMOTE
}

verify_host_release() {
  local host="$1"
  local arch archive root expected_validator_sha expected_custody_sha expected_faucet_sha
  arch="$(ssh_run "$host" "uname -m")"
  archive="$(archive_for_arch "$arch")"
  root="$(archive_root "$archive")"
  expected_validator_sha="$(archive_bin_sha "$archive" "$root" lichen-validator)"
  expected_custody_sha="$(archive_bin_sha "$archive" "$root" lichen-custody)"
  expected_faucet_sha="$(archive_bin_sha "$archive" "$root" lichen-faucet)"

  echo "Verify installed release ${host}"
  ssh_run "$host" "SERVICE='$SERVICE' EXPECTED_VALIDATOR_SHA='$expected_validator_sha' EXPECTED_CUSTODY_SHA='$expected_custody_sha' EXPECTED_FAUCET_SHA='$expected_faucet_sha' bash -s" <<'REMOTE'
set -euo pipefail

unit_exists() {
  local unit="$1"
  systemctl list-unit-files --no-legend "$unit" 2>/dev/null |
    awk '{print $1}' |
    grep -Fxq "$unit"
}

collect_pids() {
  local pid="$1"
  local child
  printf '%s\n' "$pid"
  for child in $(pgrep -P "$pid" 2>/dev/null || true); do
    collect_pids "$child"
  done
}

check_file_hash() {
  local path="$1"
  local expected="$2"
  local label="$3"
  local actual
  if [ -z "$expected" ]; then
    return 0
  fi
  if [ ! -x "$path" ]; then
    echo "Expected ${label} binary is missing or not executable: ${path}"
    exit 1
  fi
  actual="$(sha256sum "$path" | awk '{print $1}')"
  if [ "$actual" != "$expected" ]; then
    echo "${label} binary hash mismatch: got=${actual} expected=${expected}"
    exit 1
  fi
}

check_pid_hash() {
  local pid="$1"
  local expected="$2"
  local label="$3"
  local target actual
  target="$(sudo readlink "/proc/${pid}/exe" 2>/dev/null || true)"
  if [ -z "$target" ]; then
    echo "${label} process ${pid} executable is unreadable."
    exit 1
  fi
  if [[ "$target" == *" (deleted)" ]]; then
    echo "${label} process ${pid} still uses deleted executable: ${target}"
    exit 1
  fi
  actual="$(sudo sha256sum "/proc/${pid}/exe" 2>/dev/null | awk '{print $1}')"
  if [ "$actual" != "$expected" ]; then
    echo "${label} process ${pid} hash mismatch: exe=${target} got=${actual:-unreadable} expected=${expected}"
    exit 1
  fi
}

check_service_tree_hash() {
  local unit="$1"
  local expected="$2"
  local label="$3"
  local main_pid pid
  if [ -z "$expected" ]; then
    return 0
  fi
  if ! unit_exists "$unit"; then
    return 0
  fi
  if ! systemctl is-active --quiet "$unit"; then
    return 0
  fi
  main_pid="$(systemctl show "$unit" -p MainPID --value || true)"
  if [ -z "$main_pid" ] || [ "$main_pid" = "0" ]; then
    echo "${label} unit is active but has no MainPID."
    exit 1
  fi
  for pid in $(collect_pids "$main_pid" | sort -u); do
    check_pid_hash "$pid" "$expected" "$label"
  done
}

check_file_hash /usr/local/bin/lichen-validator "$EXPECTED_VALIDATOR_SHA" lichen-validator
check_file_hash /usr/local/bin/lichen-custody "$EXPECTED_CUSTODY_SHA" lichen-custody
check_file_hash /usr/local/bin/lichen-faucet "$EXPECTED_FAUCET_SHA" lichen-faucet

check_service_tree_hash "$SERVICE" "$EXPECTED_VALIDATOR_SHA" "$SERVICE"
check_service_tree_hash lichen-custody.service "$EXPECTED_CUSTODY_SHA" lichen-custody.service
check_service_tree_hash lichen-faucet.service "$EXPECTED_FAUCET_SHA" lichen-faucet.service
REMOTE
}

wait_healthy() {
  local host="$1"
  echo "Wait healthy ${host}"
  ssh_run "$host" "RPC_PORT='$RPC_PORT' MAX_BLOCK_AGE_SECS='$MAX_BLOCK_AGE_SECS' bash -s" <<'REMOTE'
set -euo pipefail
for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:${RPC_PORT}/" \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' |
    python3 -c '
import json, sys
payload = json.load(sys.stdin)
result = payload.get("result") or {}
disk = result.get("disk") or {}
age = int(result.get("block_age_secs") or 0)
max_age = int(__import__("os").environ.get("MAX_BLOCK_AGE_SECS", "15"))
status = result.get("status")
slot = result.get("slot")
print("status={} slot={} age={}s max_age={}s disk_critical={}".format(status, slot, age, max_age, disk.get("critical")))
sys.exit(0 if status == "ok" and age <= max_age and not disk.get("critical") else 1)
'; then
    exit 0
  fi
  sleep 2
done
exit 1
REMOTE
}

restart_custody_if_local() {
  local host="$1"
  if [ "$NETWORK" != "testnet" ]; then
    return 0
  fi

  echo "Refresh custody after validator health ${host}"
  ssh_run "$host" "RPC_PORT='$RPC_PORT' MAX_BLOCK_AGE_SECS='$MAX_BLOCK_AGE_SECS' bash -s" <<'REMOTE'
set -euo pipefail
if ! systemctl list-unit-files --no-legend lichen-custody.service 2>/dev/null | awk '{print $1}' | grep -Fxq lichen-custody.service; then
  exit 0
fi
if ! systemctl is-enabled --quiet lichen-custody.service 2>/dev/null && \
   ! systemctl is-active --quiet lichen-custody.service 2>/dev/null; then
  exit 0
fi

for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:${RPC_PORT}/" \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' |
    python3 -c '
import json, os, sys
payload = json.load(sys.stdin)
result = payload.get("result") or {}
age = int(result.get("block_age_secs") or 0)
max_age = int(os.environ.get("MAX_BLOCK_AGE_SECS", "15"))
sys.exit(0 if result.get("status") == "ok" and age <= max_age else 1)
'; then
    break
  fi
  sleep 2
done

sudo systemctl restart lichen-custody.service
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:9105/health" >/dev/null; then
    exit 0
  fi
  sleep 1
done
echo "Custody did not become healthy after validator restart."
exit 1
REMOTE
}

restart_faucet_if_local() {
  local host="$1"
  if [ "$NETWORK" != "testnet" ]; then
    return 0
  fi

  echo "Refresh faucet after validator health ${host}"
  ssh_run "$host" "bash -s" <<'REMOTE'
set -euo pipefail
if ! systemctl list-unit-files --no-legend lichen-faucet.service 2>/dev/null | awk '{print $1}' | grep -Fxq lichen-faucet.service; then
  exit 0
fi
if ! systemctl is-enabled --quiet lichen-faucet.service 2>/dev/null && \
   ! systemctl is-active --quiet lichen-faucet.service 2>/dev/null; then
  exit 0
fi

sudo systemctl restart lichen-faucet.service
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:9100/health" >/dev/null; then
    exit 0
  fi
  sleep 1
done
echo "Faucet did not become healthy after restart."
exit 1
REMOTE
}

public_smoke() {
  local public_url
  case "$NETWORK" in
    testnet) public_url="https://testnet-rpc.lichen.network" ;;
    mainnet) public_url="https://rpc.lichen.network" ;;
  esac
  echo "Public RPC smoke ${public_url}"
  curl -fsS "$public_url/" \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' |
    python3 -c '
import json, sys
payload = json.load(sys.stdin)
result = payload.get("result") or {}
print(json.dumps({"status": result.get("status"), "slot": result.get("slot"), "block_age_secs": result.get("block_age_secs")}, sort_keys=True))
sys.exit(0 if result.get("status") == "ok" else 1)
'
}

public_dex_oracle_smoke() {
  local public_url
  case "$NETWORK" in
    testnet) public_url="https://testnet-rpc.lichen.network" ;;
    mainnet) public_url="https://rpc.lichen.network" ;;
  esac
  echo "Public DEX oracle/candle smoke ${public_url}"
  PUBLIC_URL="$public_url" DEX_SMOKE_TIMEOUT_SECS="$DEX_SMOKE_TIMEOUT_SECS" python3 - <<'PY'
import json
import os
import time
import urllib.request

base = os.environ["PUBLIC_URL"].rstrip("/")
deadline = time.time() + int(os.environ.get("DEX_SMOKE_TIMEOUT_SECS", "90"))
last_error = None

def get_json(path):
    request = urllib.request.Request(
        base + path,
        headers={
            "Accept": "application/json",
            "User-Agent": "lichen-rolling-release-deploy/1.0",
        },
    )
    with urllib.request.urlopen(request, timeout=8) as response:
        return json.loads(response.read().decode())

while time.time() < deadline:
    try:
        oracle = get_json("/api/v1/oracle/prices")
        feeds = {feed.get("asset"): feed for feed in oracle.get("data", {}).get("feeds", [])}
        bad = []
        for asset in ("wSOL", "wETH", "wBNB"):
            feed = feeds.get(asset) or {}
            if int(feed.get("slot") or 0) <= 0 or feed.get("stale") is True:
                bad.append(f"{asset}:slot={feed.get('slot')} stale={feed.get('stale')}")
        candles = get_json("/api/v1/pairs/2/candles?interval=60&limit=4")
        candle_rows = candles.get("data") or []
        if not bad and candle_rows:
            print(json.dumps({
                "wsol_slot": feeds["wSOL"].get("slot"),
                "wsol_price": feeds["wSOL"].get("price"),
                "latest_wsol_1m_close": candle_rows[-1].get("close"),
            }, sort_keys=True))
            raise SystemExit(0)
        last_error = "; ".join(bad) or "missing wSOL 1m candles"
    except Exception as exc:
        last_error = str(exc)
    time.sleep(3)

raise SystemExit(f"DEX oracle/candle smoke failed: {last_error}")
PY
}

echo "Lichen rolling release deploy (${NETWORK}) ${RELEASE_TAG}"
echo "Hosts: ${HOSTS}"

download_release_artifacts

if [ "${LICHEN_VERIFY_RELEASE_ONLY:-}" = "1" ]; then
  for host in $HOSTS; do
    verify_host_release "$host"
  done
  echo "RELEASE VERIFY COMPLETE"
  exit 0
fi

for host in $HOSTS; do
  preflight_host "$host"
done

for host in $HOSTS; do
  install_host "$host"
  wait_healthy "$host"
  restart_custody_if_local "$host"
  restart_faucet_if_local "$host"
  verify_host_release "$host"
done

for host in $HOSTS; do
  verify_host_release "$host"
done

public_smoke
public_dex_oracle_smoke
echo "ROLLING RELEASE DEPLOY COMPLETE"
