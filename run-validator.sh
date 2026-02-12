#!/usr/bin/env bash
# ============================================================================
# MoltChain — Local Validator Launcher (convenience wrapper)
# ============================================================================
#
# Wraps skills/validator/run-validator.sh from repo root.
#
# Usage:
#   ./run-validator.sh [testnet|mainnet] <1|2|3>
#
# Same as running: ./skills/validator/run-validator.sh [network] <validator_num>
#
# For production (single validator per network), use instead:
#   ./moltchain-start.sh testnet|mainnet
#
# ============================================================================

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
exec "${SCRIPT_DIR}/skills/validator/run-validator.sh" "$@"
