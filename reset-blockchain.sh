#!/usr/bin/env bash
# ============================================================================
# MoltChain — Blockchain Reset (convenience wrapper)
# ============================================================================
#
# Usage:
#   ./reset-blockchain.sh              # Reset everything
#   ./reset-blockchain.sh --restart    # Reset + restart local testnet
#   ./reset-blockchain.sh testnet      # Reset testnet state only
#
# ============================================================================

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
exec "${SCRIPT_DIR}/skills/validator/reset-blockchain.sh" "$@"
