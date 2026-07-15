#!/bin/bash
# Canonical comprehensive RPC inventory gate.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"

command -v node >/dev/null 2>&1 || {
  echo "ERROR: Node.js is required for the comprehensive RPC gate" >&2
  exit 1
}

exec node "$ROOT_DIR/scripts/qa/e2e-rpc-coverage.js"
