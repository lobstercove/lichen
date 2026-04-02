#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
CLUSTER_SCRIPT="$ROOT_DIR/scripts/start-local-3validators.sh"

if [[ ! -x "$CLUSTER_SCRIPT" ]]; then
    echo "[start-validators] ERROR: missing executable cluster launcher at $CLUSTER_SCRIPT" >&2
    exit 1
fi

COMMAND="${1:-start}"
if [[ $# -gt 0 ]]; then
    shift
fi

exec "$CLUSTER_SCRIPT" "$COMMAND" "$@"
