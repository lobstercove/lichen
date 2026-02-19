#!/usr/bin/env bash
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."
python3 tests/comprehensive-e2e.py > /tmp/ce2e-final.log 2>&1
EXIT=$?
echo "EXIT=$EXIT"
echo "---SUMMARY---"
tail -5 /tmp/ce2e-final.log
