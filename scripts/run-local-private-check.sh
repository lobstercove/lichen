#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 3 ]]; then
  echo "usage: $0 <required-path> -- <command...>" >&2
  exit 2
fi

required_path="$1"
shift

if [[ "$1" != "--" ]]; then
  echo "usage: $0 <required-path> -- <command...>" >&2
  exit 2
fi
shift

if [[ -e "$required_path" ]]; then
  "$@"
  exit 0
fi

echo "Skipping local-private check because '$required_path' is not present in this clone."