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
import asyncio, sys, json, os, struct
sys.path.insert(0, '${TOOLS_DIR}')
sys.path.insert(0, os.path.join('${TOOLS_DIR}', '..', 'sdk', 'python'))
from deploy_dex import load_or_create_deployer, call_contract_raw
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

    if not amm:
        print('  dex_amm not in manifest, skipping pool seeding')
        return

    # Build pubkey bytes from hex or base58 address strings in manifest
    from moltchain import PublicKey
    amm_pk = PublicKey.from_string(amm) if isinstance(amm, str) else amm
    deployer_bytes = bytes(deployer.public_key().to_bytes())

    # Resolve token addresses from manifest
    token_names = {
        'MOLT': 'moltcoin',
        'mUSD': 'musd_token',
        'wSOL': 'wsol_token',
        'wETH': 'weth_token',
    }
    token_pks = {}
    for sym, contract_name in token_names.items():
        addr_str = addrs.get(contract_name)
        if addr_str:
            token_pks[sym] = bytes(PublicKey.from_string(addr_str).to_bytes()) if isinstance(addr_str, str) else bytes(addr_str.to_bytes())
        else:
            print(f'  ⚠  {contract_name} not in manifest, skipping pools with {sym}')

    # Pools to create: (token_a_symbol, token_b_symbol, fee_tier, initial_sqrt_price)
    # sqrt_price in Q32: (1 << 32) * sqrt(real_price)
    # Prices aligned with genesis oracle: MOLT=\$0.10, wSOL=\$170, wETH=\$2,500
    pools = [
        ('MOLT', 'mUSD', 30, 1_358_187_913),        # MOLT/mUSD 0.10
        ('wSOL', 'mUSD', 30, 55_999_522_252),        # wSOL/mUSD 170
        ('wETH', 'mUSD', 30, 214_748_364_800),       # wETH/mUSD 2500
        ('wSOL', 'MOLT', 30, 177_086_038_199),        # wSOL/MOLT 1700
        ('wETH', 'MOLT', 30, 679_093_956_564),        # wETH/MOLT 25000
    ]

    for (sym_a, sym_b, fee_tier, sqrt_price) in pools:
        if sym_a not in token_pks or sym_b not in token_pks:
            print(f'  ⚠  Skipping pool {sym_a}/{sym_b}: token not deployed')
            continue
        # Build opcode-encoded binary: opcode(1) + caller(32) + token_a(32) + token_b(32) + fee_tier(1) + sqrt_price(8)
        data = bytes([1]) + deployer_bytes + token_pks[sym_a] + token_pks[sym_b] + bytes([fee_tier]) + struct.pack('<Q', sqrt_price)
        try:
            sig = await call_contract_raw(conn, deployer, amm_pk, 'call', list(data))
            print(f'  Pool {sym_a}/{sym_b} created (fee={fee_tier}bps)')
        except Exception as e:
            print(f'  Pool {sym_a}/{sym_b}: {e}')

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
