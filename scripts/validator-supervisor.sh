#!/usr/bin/env bash
set -euo pipefail

# Pin LICHEN_HOME so the P2P identity (cert/key) is always resolved from
# the same directory.  Without this, runtime_home autodetection can pick a
# different path after updates, regenerating a new TLS certificate and
# breaking TOFU trust with every peer.
# Default: user home directory.  Override via env for systemd / containers.
export LICHEN_HOME="${LICHEN_HOME:-$HOME}"

if [[ "$#" -lt 3 ]]; then
  echo "usage: $0 <instance-name> -- <command...>"
  exit 2
fi

INSTANCE="$1"
shift

if [[ "$1" != "--" ]]; then
  echo "usage: $0 <instance-name> -- <command...>"
  exit 2
fi
shift

if [[ "$#" -eq 0 ]]; then
  echo "usage: $0 <instance-name> -- <command...>"
  exit 2
fi

RESTART_DELAY_SECS="${LICHEN_RESTART_DELAY_SECS:-2}"
MAX_RESTART_DELAY_SECS="${LICHEN_MAX_RESTART_DELAY_SECS:-15}"
# Exit code 75 = auto-update restart (immediate, no backoff)
EXIT_CODE_UPDATE_RESTART=75
# Reset backoff after this many seconds of stable runtime
STABLE_RUNTIME_RESET_SECS="${LICHEN_STABLE_RESET_SECS:-180}"

stop_requested=0
child_pid=""
restart_count=0
current_delay="$RESTART_DELAY_SECS"

ts() {
  date '+%Y-%m-%d %H:%M:%S'
}

on_stop_signal() {
  stop_requested=1
  if [[ -n "$child_pid" ]] && kill -0 "$child_pid" 2>/dev/null; then
    kill "$child_pid" 2>/dev/null || true
    wait "$child_pid" 2>/dev/null || true
  fi
}

trap on_stop_signal INT TERM

echo "[$(ts)] [validator-supervisor:$INSTANCE] starting command: $*"

while true; do
  if [[ "$stop_requested" -eq 1 ]]; then
    echo "[$(ts)] [validator-supervisor:$INSTANCE] stop requested; exiting"
    exit 0
  fi

  start_epoch=$(date +%s)
  "$@" &
  child_pid=$!

  set +e
  wait "$child_pid"
  exit_code=$?
  set -e
  child_pid=""
  end_epoch=$(date +%s)
  runtime=$((end_epoch - start_epoch))

  if [[ "$stop_requested" -eq 1 ]]; then
    echo "[$(ts)] [validator-supervisor:$INSTANCE] child stopped by signal; exiting"
    exit 0
  fi

  restart_count=$((restart_count + 1))

  # Exit code 75 = auto-update restart: immediate restart, no backoff
  if [[ "$exit_code" -eq "$EXIT_CODE_UPDATE_RESTART" ]]; then
    echo "[$(ts)] [validator-supervisor:$INSTANCE] child exited rc=$exit_code (auto-update restart #$restart_count), restarting immediately"
    current_delay="$RESTART_DELAY_SECS"
    continue
  fi

  # If the child ran for long enough, reset backoff (it was stable)
  if [[ "$runtime" -ge "$STABLE_RUNTIME_RESET_SECS" ]]; then
    current_delay="$RESTART_DELAY_SECS"
  fi

  echo "[$(ts)] [validator-supervisor:$INSTANCE] child exited rc=$exit_code after ${runtime}s (restart #$restart_count), restarting in ${current_delay}s"
  sleep "$current_delay"

  if [[ "$current_delay" -lt "$MAX_RESTART_DELAY_SECS" ]]; then
    next_delay=$((current_delay * 2))
    if [[ "$next_delay" -gt "$MAX_RESTART_DELAY_SECS" ]]; then
      current_delay="$MAX_RESTART_DELAY_SECS"
    else
      current_delay="$next_delay"
    fi
  fi
done
