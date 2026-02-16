#!/bin/bash
# ============================================================================
# Build All MoltChain Smart Contracts to WASM
# ============================================================================
#
# Compiles all 27 smart contracts (16 core + 8 DEX + 3 wrapped tokens) to
# WebAssembly using the wasm32-unknown-unknown target. Copies final .wasm
# files to each contract's crate root for deploy_dex.py / deploy_contract.py
# to find them.
#
# Usage:
#   ./scripts/build-all-contracts.sh           # build all
#   ./scripts/build-all-contracts.sh --dex     # build only DEX + wrapped tokens
#   ./scripts/build-all-contracts.sh --test    # build + run tests
#
# Requirements:
#   - rustup target add wasm32-unknown-unknown
#   - Rust nightly or stable with wasm support
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/.."
CONTRACTS_DIR="${REPO_ROOT}/contracts"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# All 26 contracts in dependency order
CORE_CONTRACTS=(
    moltcoin
    moltdao
    moltswap
    moltbridge
    moltmarket
    moltoracle
    moltauction
    moltpunks
    moltyid
    lobsterlend
    clawpay
    clawpump
    clawvault
    bountyboard
    compute_market
    reef_storage
)

DEX_CONTRACTS=(
    dex_core
    dex_amm
    dex_router
    dex_governance
    dex_margin
    dex_rewards
    dex_analytics
    prediction_market
)

WRAPPED_TOKEN_CONTRACTS=(
    musd_token
    wsol_token
    weth_token
)

# Parse args
BUILD_SCOPE="all"
RUN_TESTS=false
for arg in "$@"; do
    case "$arg" in
        --dex)     BUILD_SCOPE="dex" ;;
        --tokens)  BUILD_SCOPE="tokens" ;;
        --core)    BUILD_SCOPE="core" ;;
        --test)    RUN_TESTS=true ;;
        --help|-h)
            echo "Usage: $0 [--dex|--tokens|--core] [--test]"
            echo "  --dex     Build DEX + wrapped token contracts only"
            echo "  --tokens  Build wrapped token contracts only"
            echo "  --core    Build core contracts only"
            echo "  --test    Run cargo test after building"
            exit 0
            ;;
    esac
done

# Select contracts to build
case "$BUILD_SCOPE" in
    all)     CONTRACTS=("${CORE_CONTRACTS[@]}" "${DEX_CONTRACTS[@]}" "${WRAPPED_TOKEN_CONTRACTS[@]}") ;;
    dex)     CONTRACTS=("${DEX_CONTRACTS[@]}" "${WRAPPED_TOKEN_CONTRACTS[@]}") ;;
    tokens)  CONTRACTS=("${WRAPPED_TOKEN_CONTRACTS[@]}") ;;
    core)    CONTRACTS=("${CORE_CONTRACTS[@]}") ;;
esac

echo -e "${CYAN}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  🦞 MoltChain Contract Builder                          ║${NC}"
echo -e "${CYAN}║  Building ${#CONTRACTS[@]} contracts to WASM                         ║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════╝${NC}"

# Ensure wasm32 target is installed
if ! rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown; then
    echo -e "${YELLOW}Installing wasm32-unknown-unknown target...${NC}"
    rustup target add wasm32-unknown-unknown
fi

BUILT=0
FAILED=0
SKIPPED=0
TOTAL=${#CONTRACTS[@]}
FAILED_LIST=()

for contract in "${CONTRACTS[@]}"; do
    CONTRACT_DIR="${CONTRACTS_DIR}/${contract}"
    
    if [ ! -d "$CONTRACT_DIR" ]; then
        echo -e "  ${YELLOW}⚠  ${contract}: directory not found, skipping${NC}"
        ((SKIPPED++))
        continue
    fi
    
    if [ ! -f "$CONTRACT_DIR/Cargo.toml" ]; then
        echo -e "  ${YELLOW}⚠  ${contract}: no Cargo.toml, skipping${NC}"
        ((SKIPPED++))
        continue
    fi

    echo -e "\n${CYAN}[$((BUILT + FAILED + SKIPPED + 1))/${TOTAL}]${NC} Building ${contract}..."
    
    # Build WASM
    if (cd "$CONTRACT_DIR" && cargo build --target wasm32-unknown-unknown --release 2>&1); then
        # Find the output .wasm file
        # The crate name in Cargo.toml uses hyphens, but the .wasm file uses underscores
        CRATE_NAME=$(grep '^name' "$CONTRACT_DIR/Cargo.toml" | head -1 | sed 's/.*= *"//;s/".*//' | tr '-' '_')
        WASM_SOURCE="${CONTRACT_DIR}/target/wasm32-unknown-unknown/release/${CRATE_NAME}.wasm"
        WASM_DEST="${CONTRACT_DIR}/${contract}.wasm"
        
        if [ -f "$WASM_SOURCE" ]; then
            cp "$WASM_SOURCE" "$WASM_DEST"
            SIZE=$(wc -c < "$WASM_DEST" | tr -d ' ')
            echo -e "  ${GREEN}✅ ${contract}.wasm — ${SIZE} bytes${NC}"
            ((BUILT++))
        else
            # Try with the directory name directly
            ALT_SOURCE="${CONTRACT_DIR}/target/wasm32-unknown-unknown/release/${contract}.wasm"
            if [ -f "$ALT_SOURCE" ]; then
                cp "$ALT_SOURCE" "$WASM_DEST"
                SIZE=$(wc -c < "$WASM_DEST" | tr -d ' ')
                echo -e "  ${GREEN}✅ ${contract}.wasm — ${SIZE} bytes${NC}"
                ((BUILT++))
            else
                echo -e "  ${RED}❌ ${contract}: build succeeded but .wasm not found${NC}"
                echo "     Expected: ${WASM_SOURCE}"
                ((FAILED++))
                FAILED_LIST+=("$contract")
            fi
        fi
    else
        echo -e "  ${RED}❌ ${contract}: compilation failed${NC}"
        ((FAILED++))
        FAILED_LIST+=("$contract")
    fi
done

# Run tests if requested
if $RUN_TESTS; then
    echo -e "\n${CYAN}╔══════════════════════════════════════════════════════════╗${NC}"
    echo -e "${CYAN}║  Running tests...                                        ║${NC}"
    echo -e "${CYAN}╚══════════════════════════════════════════════════════════╝${NC}"
    
    TEST_PASSED=0
    TEST_FAILED=0
    for contract in "${CONTRACTS[@]}"; do
        CONTRACT_DIR="${CONTRACTS_DIR}/${contract}"
        [ ! -d "$CONTRACT_DIR" ] && continue
        
        echo -e "\n  Testing ${contract}..."
        if (cd "$CONTRACT_DIR" && cargo test 2>&1); then
            echo -e "  ${GREEN}✅ ${contract} tests passed${NC}"
            ((TEST_PASSED++))
        else
            echo -e "  ${RED}❌ ${contract} tests failed${NC}"
            ((TEST_FAILED++))
        fi
    done
    
    echo -e "\n  Tests: ${GREEN}${TEST_PASSED} passed${NC}, ${RED}${TEST_FAILED} failed${NC}"
fi

# Summary
echo -e "\n${CYAN}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  BUILD SUMMARY                                           ║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════╝${NC}"
echo -e "  ${GREEN}Built:   ${BUILT}${NC}"
echo -e "  ${RED}Failed:  ${FAILED}${NC}"
echo -e "  ${YELLOW}Skipped: ${SKIPPED}${NC}"
echo -e "  Total:   ${TOTAL}"

if [ ${#FAILED_LIST[@]} -gt 0 ]; then
    echo -e "\n  ${RED}Failed contracts:${NC}"
    for f in "${FAILED_LIST[@]}"; do
        echo -e "    - $f"
    done
fi

# List all .wasm files
echo -e "\n  WASM files:"
for contract in "${CONTRACTS[@]}"; do
    WASM="${CONTRACTS_DIR}/${contract}/${contract}.wasm"
    if [ -f "$WASM" ]; then
        SIZE=$(wc -c < "$WASM" | tr -d ' ')
        echo -e "    ${GREEN}✓${NC} contracts/${contract}/${contract}.wasm  (${SIZE} bytes)"
    else
        echo -e "    ${RED}✗${NC} contracts/${contract}/${contract}.wasm  (missing)"
    fi
done

exit $FAILED
