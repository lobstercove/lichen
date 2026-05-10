#!/bin/bash
# ============================================================================
# Lichen Testnet Restriction State-Root Schema Activation
# ============================================================================
#
# Activates the restriction-committing state-root schema on the running testnet
# without resetting chain state and without copying validator databases. This is
# a coordinated in-place metadata activation: stop validators, set the local
# state-root schema flag on each node, restart validators, then record sync
# evidence.
#
# This script is intentionally testnet-only and requires explicit owner approval.
#
# Usage:
#   LICHEN_OWNER_APPROVED_RESTRICTION_SCHEMA_ACTIVATION='owner-approved:restriction-schema:testnet:15.204.229.189,37.59.97.61,15.235.142.253' \
#   LICHEN_RESTRICTION_SCHEMA_ACTIVATION_CONFIRM='activate-restriction-schema:testnet:15.204.229.189,37.59.97.61,15.235.142.253' \
#     bash scripts/activate-restriction-schema-testnet.sh
#
# Optional:
#   LICHEN_ACTIVATION_VPSES='15.204.229.189 37.59.97.61 15.235.142.253'
#   LICHEN_RESTRICTION_SCHEMA_EVIDENCE_DIR='artifacts/restriction-schema-activation/<run-id>'
#
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/.."
cd "$REPO_ROOT"

NETWORK="${1:-testnet}"
if [ "$NETWORK" != "testnet" ]; then
  echo "Refusing restriction schema activation for '$NETWORK': RG-804 is testnet-only." >&2
  exit 1
fi

DEFAULT_VPSES="15.204.229.189 37.59.97.61 15.235.142.253"
read -r -a ALL_VPSES <<< "${LICHEN_ACTIVATION_VPSES:-$DEFAULT_VPSES}"
if [ "${#ALL_VPSES[@]}" -eq 0 ]; then
  echo "No VPS hosts configured." >&2
  exit 1
fi

SSH_PORT="${LICHEN_ACTIVATION_SSH_PORT:-2222}"
SSH_USER="${LICHEN_ACTIVATION_SSH_USER:-ubuntu}"
SSH_OPTS="-p $SSH_PORT -o ConnectTimeout=10 -o ServerAliveInterval=5 -o ServerAliveCountMax=3 -o StrictHostKeyChecking=no -o BatchMode=yes -o LogLevel=ERROR"
RPC_PORT=8899
SERVICE="lichen-validator-testnet"
STATE_DIR="/var/lib/lichen/state-testnet"
VALIDATOR_BIN="/usr/local/bin/lichen-validator"
MAX_SLOT_SPREAD="${LICHEN_ACTIVATION_MAX_SLOT_SPREAD:-10}"

VPS_CONFIRMATION_LIST="${ALL_VPSES[*]}"
VPS_CONFIRMATION_LIST="${VPS_CONFIRMATION_LIST// /,}"
OWNER_APPROVAL="owner-approved:restriction-schema:${NETWORK}:${VPS_CONFIRMATION_LIST}"
ACTIVATION_CONFIRMATION="activate-restriction-schema:${NETWORK}:${VPS_CONFIRMATION_LIST}"

RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)"
EVIDENCE_DIR="${LICHEN_RESTRICTION_SCHEMA_EVIDENCE_DIR:-artifacts/restriction-schema-activation/${RUN_ID}}"
mkdir -p "$EVIDENCE_DIR"
exec > >(tee "$EVIDENCE_DIR/rollout.log") 2>&1

ssh_run() {
  local host=$1
  shift
  ssh $SSH_OPTS "$SSH_USER@$host" "$@"
}

capture_rpc() {
  local host=$1
  local label=$2
  local method=$3
  local file="$EVIDENCE_DIR/${label}-${host}.json"
  ssh_run "$host" "curl -fsS -H 'Content-Type: application/json' --data '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":[]}' http://127.0.0.1:${RPC_PORT}/" \
    >"$file"
}

require_confirmation() {
  if [ "${LICHEN_OWNER_APPROVED_RESTRICTION_SCHEMA_ACTIVATION:-}" = "$OWNER_APPROVAL" ] && \
     [ "${LICHEN_RESTRICTION_SCHEMA_ACTIVATION_CONFIRM:-}" = "$ACTIVATION_CONFIRMATION" ]; then
    return 0
  fi

  echo "Refusing restriction schema activation without explicit owner approval." >&2
  echo "This script does not reset state, does not delete state, and does not copy state." >&2
  echo "It will stop and restart ${SERVICE} on:" >&2
  printf '  - %s\n' "${ALL_VPSES[@]}" >&2
  echo "" >&2
  echo "To continue, set:" >&2
  echo "  export LICHEN_OWNER_APPROVED_RESTRICTION_SCHEMA_ACTIVATION='$OWNER_APPROVAL'" >&2
  echo "  export LICHEN_RESTRICTION_SCHEMA_ACTIVATION_CONFIRM='$ACTIVATION_CONFIRMATION'" >&2
  exit 1
}

write_local_evidence() {
  {
    echo "run_id=$RUN_ID"
    echo "network=$NETWORK"
    echo "hosts=$VPS_CONFIRMATION_LIST"
    echo "owner_approval=$OWNER_APPROVAL"
    echo "activation_confirmation=$ACTIVATION_CONFIRMATION"
    echo "max_slot_spread=$MAX_SLOT_SPREAD"
    echo "repo_head=$(git rev-parse HEAD 2>/dev/null || true)"
    echo "repo_dirty_status_begin"
    git status --short 2>/dev/null || true
    echo "repo_dirty_status_end"
  } >"$EVIDENCE_DIR/local-context.txt"
}

verify_matching_validator_hashes() {
  local expected_hash=""
  local hash

  for host in "${ALL_VPSES[@]}"; do
    hash="$(awk 'NR == 1 { print $1 }' "$EVIDENCE_DIR/preflight-${host}.txt")"
    if [ -z "$hash" ]; then
      echo "Missing installed validator hash for $host" >&2
      exit 1
    fi
    if [ -z "$expected_hash" ]; then
      expected_hash="$hash"
    elif [ "$hash" != "$expected_hash" ]; then
      echo "Installed validator hash mismatch on $host: $hash != $expected_hash" >&2
      exit 1
    fi
  done

  echo "validator_hash=$expected_hash" >"$EVIDENCE_DIR/validator-hash.txt"
}

write_sync_summary() {
  python3 - "$EVIDENCE_DIR" "$MAX_SLOT_SPREAD" "${ALL_VPSES[@]}" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
max_slot_spread = int(sys.argv[2])
hosts = sys.argv[3:]

summary = {"hosts": {}, "max_allowed_slot_spread": max_slot_spread}
for host in hosts:
    host_summary = {}
    for label in ("post-health", "post-slot", "post-latest-block"):
        path = root / f"{label}-{host}.json"
        try:
            payload = json.loads(path.read_text())
        except Exception as exc:
            host_summary[label] = {"error": f"failed to parse {path.name}: {exc}"}
            continue
        host_summary[label] = payload.get("result", payload)
    summary["hosts"][host] = host_summary

slots = []
for host, host_summary in summary["hosts"].items():
    slot = host_summary.get("post-slot")
    if isinstance(slot, int):
        slots.append(slot)
    elif isinstance(slot, str) and slot.isdigit():
        slots.append(int(slot))

summary["slot_min"] = min(slots) if slots else None
summary["slot_max"] = max(slots) if slots else None
summary["slot_spread"] = (max(slots) - min(slots)) if slots else None
summary["synced_within_threshold"] = (
    bool(slots) and summary["slot_spread"] <= max_slot_spread
)
(root / "sync-evidence-summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
print(json.dumps(summary, indent=2, sort_keys=True))
if not slots:
    print("no post-activation slots were captured", file=sys.stderr)
    sys.exit(1)
if summary["slot_spread"] > max_slot_spread:
    print(
        f"slot spread {summary['slot_spread']} exceeds allowed {max_slot_spread}",
        file=sys.stderr,
    )
    sys.exit(1)
PY
}

capture_post_rpc_evidence() {
  for host in "${ALL_VPSES[@]}"; do
    capture_rpc "$host" "post-health" "getHealth"
    capture_rpc "$host" "post-slot" "getSlot"
    capture_rpc "$host" "post-latest-block" "getLatestBlock"
  done
}

echo "Lichen restriction schema activation (${NETWORK})"
echo "Evidence directory: $EVIDENCE_DIR"
echo ""

require_confirmation
write_local_evidence

echo "Preflight: capture live health and installed validator hashes"
for host in "${ALL_VPSES[@]}"; do
  echo "  - $host"
  ssh_run "$host" "sha256sum ${VALIDATOR_BIN}; systemctl is-active ${SERVICE}" \
    | tee "$EVIDENCE_DIR/preflight-${host}.txt"
  capture_rpc "$host" "pre-health" "getHealth" || true
  capture_rpc "$host" "pre-slot" "getSlot" || true
  capture_rpc "$host" "pre-latest-block" "getLatestBlock" || true
done
verify_matching_validator_hashes

echo "Stopping validators"
for host in "${ALL_VPSES[@]}"; do
  echo "  - stopping $host"
  ssh_run "$host" "sudo systemctl stop ${SERVICE}"
done

echo "Activating schema on each stopped validator"
for host in "${ALL_VPSES[@]}"; do
  echo "  - $host before"
  ssh_run "$host" "sudo -u lichen ${VALIDATOR_BIN} --no-watchdog --show-restriction-schema --network testnet --db-path ${STATE_DIR}" \
    | tee "$EVIDENCE_DIR/schema-before-${host}.txt"

  echo "  - $host activate"
  ssh_run "$host" "sudo -u lichen ${VALIDATOR_BIN} --no-watchdog --activate-restriction-schema --network testnet --db-path ${STATE_DIR}" \
    | tee "$EVIDENCE_DIR/schema-activate-${host}.txt"

  echo "  - $host after"
  ssh_run "$host" "sudo -u lichen ${VALIDATOR_BIN} --no-watchdog --show-restriction-schema --network testnet --db-path ${STATE_DIR}" \
    | tee "$EVIDENCE_DIR/schema-after-${host}.txt"

  if ! grep -q '^after_schema=active$' "$EVIDENCE_DIR/schema-after-${host}.txt"; then
    echo "Schema activation did not persist on $host" >&2
    exit 1
  fi
done

echo "Starting validators"
for host in "${ALL_VPSES[@]}"; do
  echo "  - starting $host"
  ssh_run "$host" "sudo systemctl start ${SERVICE}"
done

echo "Waiting for validators to serve RPC after restart"
for attempt in $(seq 1 60); do
  ready=1
  for host in "${ALL_VPSES[@]}"; do
    if ! capture_rpc "$host" "post-health" "getHealth" >/dev/null 2>&1; then
      ready=0
      break
    fi
    if ! capture_rpc "$host" "post-slot" "getSlot" >/dev/null 2>&1; then
      ready=0
      break
    fi
  done
  if [ "$ready" = "1" ]; then
    break
  fi
  if [ "$attempt" = "60" ]; then
    echo "Validators did not become RPC-ready after restart" >&2
    exit 1
  fi
  sleep 2
done

echo "Capturing post-activation sync evidence"
for attempt in $(seq 1 60); do
  if capture_post_rpc_evidence && write_sync_summary >/dev/null; then
    break
  fi
  if [ "$attempt" = "60" ]; then
    capture_post_rpc_evidence || true
    write_sync_summary
    echo "Post-activation sync evidence did not meet max slot spread ${MAX_SLOT_SPREAD}" >&2
    exit 1
  fi
  sleep 2
done

write_sync_summary
for host in "${ALL_VPSES[@]}"; do
  ssh_run "$host" "systemctl is-active ${SERVICE}; journalctl -u ${SERVICE} --since '5 minutes ago' --no-pager | grep -E 'state-root mismatch|STATE INTEGRITY|panic|fatal|error' || true" \
    | tee "$EVIDENCE_DIR/post-journal-scan-${host}.txt"
done

echo "Restriction schema activation complete. Evidence: $EVIDENCE_DIR"
