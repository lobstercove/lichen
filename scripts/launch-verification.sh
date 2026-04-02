#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

echo "[launch-verification] Running the strict launch gate via tests/production-e2e-gate.sh"
exec env \
    STRICT_NO_SKIPS="${STRICT_NO_SKIPS:-1}" \
    LICHEN_BIN="${LICHEN_BIN:-$ROOT_DIR/target/release/lichen}" \
    bash "$ROOT_DIR/tests/production-e2e-gate.sh"
