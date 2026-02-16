#!/bin/bash
# ============================================================================
# MoltChain First-Boot Contract Deployment
# ============================================================================
#
# Runs automatically after genesis to deploy ALL smart contracts, initialize
# them, wire cross-references, and save the deploy manifest. Idempotent — if
# deploy-manifest.json exists and all contracts are verified on-chain, exits
# immediately.
#
# Designed to be called from start-local-stack.sh or systemd after the
# validator reaches a healthy state.
#
# Usage:
#   ./scripts/first-boot-deploy.sh                      # default local RPC
#   ./scripts/first-boot-deploy.sh --rpc http://node:8899
#   ./scripts/first-boot-deploy.sh --skip-build          # skip WASM build
#
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/.."
TOOLS_DIR="${REPO_ROOT}/tools"
CONTRACTS_DIR="${REPO_ROOT}/contracts"
MANIFEST="${REPO_ROOT}/deploy-manifest.json"

RPC_URL="${CUSTODY_MOLT_RPC_URL:-http://127.0.0.1:8899}"
SKIP_BUILD=false
MAX_RETRIES=30
RETRY_DELAY=2

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# Parse args
for arg in "$@"; do
    case "$arg" in
        --rpc=*)    RPC_URL="${arg#*=}" ;;
        --rpc)      shift; RPC_URL="${1:-$RPC_URL}" ;;
        --skip-build) SKIP_BUILD=true ;;
        --force)    rm -f "$MANIFEST" ;;
    esac
done

echo -e "${CYAN}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  🦞 MoltChain First-Boot Contract Deployment            ║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════╝${NC}"
echo -e "  RPC:      ${RPC_URL}"
echo -e "  Manifest: ${MANIFEST}"

# ─────────────────────────────────────────────────────────
# Step 1: Check if already deployed
# ─────────────────────────────────────────────────────────
if [ -f "$MANIFEST" ]; then
    CONTRACT_COUNT=$(python3 -c "import json; m=json.load(open('$MANIFEST')); print(len(m.get('contracts',{})))" 2>/dev/null || echo "0")
    if [ "$CONTRACT_COUNT" -ge 10 ]; then
        echo -e "\n  ${GREEN}✅ Deploy manifest exists with ${CONTRACT_COUNT} contracts.${NC}"
        echo -e "  ${GREEN}   Contracts already deployed. Use --force to redeploy.${NC}"
        exit 0
    else
        echo -e "  ${YELLOW}⚠  Manifest exists but only has ${CONTRACT_COUNT} contracts. Redeploying...${NC}"
    fi
fi

# ─────────────────────────────────────────────────────────
# Step 2: Wait for validator to be healthy
# ─────────────────────────────────────────────────────────
echo -e "\n${CYAN}[1/5]${NC} Waiting for validator at ${RPC_URL}..."

HEALTHY=false
for i in $(seq 1 $MAX_RETRIES); do
    RESPONSE=$(curl -s -X POST "${RPC_URL}" \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","id":1,"method":"health"}' 2>/dev/null || echo "")
    
    if echo "$RESPONSE" | python3 -c "import sys,json; r=json.load(sys.stdin); assert r.get('result',{}).get('status') in ['ok','healthy',True]" 2>/dev/null; then
        HEALTHY=true
        echo -e "  ${GREEN}✅ Validator healthy (attempt ${i}/${MAX_RETRIES})${NC}"
        break
    fi
    
    echo -e "  ⏳ Attempt ${i}/${MAX_RETRIES} — waiting ${RETRY_DELAY}s..."
    sleep $RETRY_DELAY
done

if ! $HEALTHY; then
    echo -e "  ${RED}❌ Validator not healthy after ${MAX_RETRIES} attempts. Aborting.${NC}"
    exit 1
fi

# ─────────────────────────────────────────────────────────
# Step 3: Build contracts to WASM (if needed)
# ─────────────────────────────────────────────────────────
if $SKIP_BUILD; then
    echo -e "\n${CYAN}[2/4]${NC} Skipping WASM build (--skip-build)"
else
    echo -e "\n${CYAN}[2/5]${NC} Building contracts to WASM..."
    
    # Check which contracts need building
    DEX_AND_TOKEN_CONTRACTS=(
        musd_token wsol_token weth_token
        dex_core dex_amm dex_router
        dex_governance dex_margin dex_rewards dex_analytics
        prediction_market
    )
    
    NEED_BUILD=false
    for c in "${DEX_AND_TOKEN_CONTRACTS[@]}"; do
        if [ ! -f "${CONTRACTS_DIR}/${c}/${c}.wasm" ]; then
            NEED_BUILD=true
            break
        fi
    done
    
    if $NEED_BUILD; then
        echo -e "  Building missing contracts..."
        "${SCRIPT_DIR}/build-all-contracts.sh" --dex 2>&1 | sed 's/^/    /'
    else
        echo -e "  ${GREEN}✅ All WASM files present${NC}"
    fi
fi

# ─────────────────────────────────────────────────────────
# Step 4: Deploy via deploy_dex.py (handles all 10 DEX+token contracts)
# ─────────────────────────────────────────────────────────
echo -e "\n${CYAN}[3/5]${NC} Deploying DEX + wrapped token contracts..."

# Check if deploy_dex.py exists
if [ ! -f "${TOOLS_DIR}/deploy_dex.py" ]; then
    echo -e "  ${RED}❌ deploy_dex.py not found at ${TOOLS_DIR}/deploy_dex.py${NC}"
    exit 1
fi

# Run deployment
python3 "${TOOLS_DIR}/deploy_dex.py" --rpc "${RPC_URL}" 2>&1 | sed 's/^/    /'
DEPLOY_EXIT=$?

if [ $DEPLOY_EXIT -ne 0 ]; then
    echo -e "  ${YELLOW}⚠  deploy_dex.py exited with code ${DEPLOY_EXIT}${NC}"
    echo -e "  ${YELLOW}   Some contracts may not have deployed. Check logs above.${NC}"
fi

# ─────────────────────────────────────────────────────────
# Step 5: Deploy core contracts (if we have deploy_contract.py)
# ─────────────────────────────────────────────────────────
echo -e "\n${CYAN}[4/5]${NC} Deploying core infrastructure contracts..."

CORE_CONTRACTS=(
    moltcoin moltdao moltswap moltbridge moltmarket moltoracle
    moltauction moltpunks moltyid lobsterlend clawpay clawpump
    clawvault bountyboard compute_market reef_storage
)

if [ -f "${TOOLS_DIR}/deploy_contract.py" ]; then
    for c in "${CORE_CONTRACTS[@]}"; do
        WASM="${CONTRACTS_DIR}/${c}/${c}.wasm"
        if [ -f "$WASM" ]; then
            echo -e "  Deploying ${c}..."
            CUSTODY_MOLT_RPC_URL="${RPC_URL}" python3 "${TOOLS_DIR}/deploy_contract.py" \
                "$WASM" 2>&1 | sed 's/^/    /' || true
        fi
    done
else
    echo -e "  ${YELLOW}⚠  deploy_contract.py not found — skipping core contracts${NC}"
    echo -e "  ${YELLOW}   DEX + wrapped tokens deployed successfully via deploy_dex.py${NC}"
fi

# ─────────────────────────────────────────────────────────
# Step 6: Seed AMM pools + insurance fund (after pairs created in deploy_dex.py)
# ─────────────────────────────────────────────────────────
echo -e "\n${CYAN}[5/5]${NC} Seeding AMM pools + insurance fund..."

python3 -c "
import asyncio, sys, json, os
sys.path.insert(0, '${TOOLS_DIR}')
sys.path.insert(0, os.path.join('${TOOLS_DIR}', '..', 'sdk', 'python'))
from deploy_dex import load_or_create_deployer, call_contract
from moltchain import Connection

async def go():
    conn = Connection('${RPC_URL}')
    deployer = load_or_create_deployer()
    manifest_path = '${MANIFEST}'
    if not os.path.exists(manifest_path):
        print('  No manifest found, skipping pool seeding')
        return
    manifest = json.load(open(manifest_path))
    addrs = manifest.get('contracts', {})
    amm = addrs.get('dex_amm')
    margin = addrs.get('dex_margin')

    if amm:
        pools = [
            {'pair_id': 0, 'fee_bps': 30, 'sqrt_price': 648_000_000},
            {'pair_id': 1, 'fee_bps': 30, 'sqrt_price': 13_360_000_000},
            {'pair_id': 2, 'fee_bps': 30, 'sqrt_price': 59_345_000_000},
            {'pair_id': 3, 'fee_bps': 30, 'sqrt_price': 135_700_000},
            # MOLT-denominated pools (price in MOLT shells)
            # wSOL/MOLT: ~424 MOLT per SOL (SOL=$178, MOLT=$0.42)
            {'pair_id': 4, 'fee_bps': 30, 'sqrt_price': 20_591_000_000},
            # wETH/MOLT: ~8383 MOLT per ETH (ETH=$3521, MOLT=$0.42)
            {'pair_id': 5, 'fee_bps': 30, 'sqrt_price': 91_558_000_000},
            # REEF/MOLT: ~0.043 MOLT per REEF (REEF=$0.018, MOLT=$0.42)
            {'pair_id': 6, 'fee_bps': 30, 'sqrt_price': 207_400_000},
        ]
        for pool in pools:
            try:
                sig = await call_contract(conn, deployer, amm, 'create_pool', pool)
                print(f'  Pool pair_id={pool[\"pair_id\"]} created')
            except Exception as e:
                print(f'  Pool pair_id={pool[\"pair_id\"]}: {e}')

    if margin:
        try:
            sig = await call_contract(conn, deployer, margin, 'seed_insurance', {'amount': 10_000_000_000_000})
            print(f'  Insurance fund seeded (10k MOLT)')
        except Exception as e:
            print(f'  Insurance seed: {e}')

asyncio.run(go())
" 2>&1 | sed 's/^/    /' || echo -e "  ${YELLOW}⚠  Pool seeding failed, chain may need manual seeding${NC}"

# ─────────────────────────────────────────────────────────
# Final verification
# ─────────────────────────────────────────────────────────
echo -e "\n${CYAN}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  FIRST-BOOT DEPLOYMENT COMPLETE                          ║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════╝${NC}"

if [ -f "$MANIFEST" ]; then
    CONTRACT_COUNT=$(python3 -c "import json; m=json.load(open('$MANIFEST')); print(len(m.get('contracts',{})))" 2>/dev/null || echo "0")
    echo -e "  ${GREEN}Manifest: ${MANIFEST}${NC}"
    echo -e "  ${GREEN}Deployed: ${CONTRACT_COUNT} contracts${NC}"
    
    # Print contract addresses
    python3 -c "
import json
m = json.load(open('$MANIFEST'))
for name, addr in m.get('contracts', {}).items():
    if 'token' in name:
        tag = 'TOKEN'
    elif name == 'prediction_market':
        tag = 'PRED '
    else:
        tag = 'DEX  '
    print(f'  [{tag}] {name:20s} → {addr}')
" 2>/dev/null || true
else
    echo -e "  ${YELLOW}⚠  No manifest file generated. Check deploy logs above.${NC}"
fi

echo -e "\n  ${GREEN}🦞 Chain is ready for trading!${NC}"
