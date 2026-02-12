#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════════
# MoltChain Testnet Deploy Script
# Deploys all 26 contracts + creates initial DEX pairs/pools on testnet
#
# Usage: ./testnet-deploy.sh [--rpc URL] [--skip-build] [--seed-pairs] [--seed-pools]
# ═══════════════════════════════════════════════════════════════════════════════
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

# Defaults
RPC_URL="${MOLTCHAIN_RPC_URL:-http://localhost:8000}"
SKIP_BUILD=false
SEED_PAIRS=false
SEED_POOLS=false
MANIFEST="$ROOT_DIR/deploy/deploy-manifest.json"
DEPLOY_LOG="$ROOT_DIR/deploy/testnet-deploy-$(date +%Y%m%d-%H%M%S).log"

# Parse args
while [[ $# -gt 0 ]]; do
    case "$1" in
        --rpc) RPC_URL="$2"; shift 2 ;;
        --skip-build) SKIP_BUILD=true; shift ;;
        --seed-pairs) SEED_PAIRS=true; shift ;;
        --seed-pools) SEED_POOLS=true; shift ;;
        --help)
            echo "Usage: $0 [--rpc URL] [--skip-build] [--seed-pairs] [--seed-pools]"
            exit 0 ;;
        *) echo "Unknown flag: $1"; exit 1 ;;
    esac
done

log() { echo "[$(date +%H:%M:%S)] $*" | tee -a "$DEPLOY_LOG"; }

mkdir -p "$(dirname "$DEPLOY_LOG")"

# ─────────────────────────────────────────────────────────────────────────────
# Phase 1: Validate environment
# ─────────────────────────────────────────────────────────────────────────────
log "═══ Phase 1: Environment validation ═══"
log "RPC endpoint: $RPC_URL"
log "Root directory: $ROOT_DIR"

# Check RPC health
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$RPC_URL" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"health"}' 2>/dev/null || echo "000")

if [[ "$HTTP_CODE" != "200" ]]; then
    log "⚠️  RPC not responding (HTTP $HTTP_CODE). Waiting..."
    for i in $(seq 1 30); do
        sleep 2
        HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$RPC_URL" \
            -H "Content-Type: application/json" \
            -d '{"jsonrpc":"2.0","id":1,"method":"health"}' 2>/dev/null || echo "000")
        if [[ "$HTTP_CODE" == "200" ]]; then
            log "✅ RPC is healthy"
            break
        fi
        if [[ "$i" == "30" ]]; then
            log "❌ RPC not available after 60s. Aborting."
            exit 1
        fi
    done
else
    log "✅ RPC is healthy"
fi

# Get chain info
CHAIN_STATUS=$(curl -s -X POST "$RPC_URL" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"getChainStatus"}' 2>/dev/null || echo "{}")
log "Chain status: $(echo "$CHAIN_STATUS" | head -c 200)"

# ─────────────────────────────────────────────────────────────────────────────
# Phase 2: Build WASM (if needed)
# ─────────────────────────────────────────────────────────────────────────────
if [[ "$SKIP_BUILD" != "true" ]]; then
    log ""
    log "═══ Phase 2: Building WASM contracts ═══"
    if [[ -x "$SCRIPT_DIR/build-all-contracts.sh" ]]; then
        "$SCRIPT_DIR/build-all-contracts.sh" 2>&1 | tee -a "$DEPLOY_LOG"
    else
        log "⚠️  build-all-contracts.sh not found, skipping build"
    fi
else
    log ""
    log "═══ Phase 2: Build skipped (--skip-build) ═══"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Phase 3: Deploy all contracts
# ─────────────────────────────────────────────────────────────────────────────
log ""
log "═══ Phase 3: Deploying contracts to testnet ═══"

# Deploy DEX + token contracts (10)
DEX_CONTRACTS=(musd_token wsol_token weth_token dex_core dex_amm dex_router dex_governance dex_rewards dex_margin dex_analytics)
for contract in "${DEX_CONTRACTS[@]}"; do
    WASM_PATH="$ROOT_DIR/contracts/$contract/target/wasm32-unknown-unknown/release/${contract}.wasm"
    if [[ -f "$WASM_PATH" ]]; then
        log "  Deploying $contract..."
        python3 "$ROOT_DIR/tools/deploy_dex.py" --rpc "$RPC_URL" --contract "$contract" --wasm "$WASM_PATH" 2>&1 | tee -a "$DEPLOY_LOG" || true
    else
        log "  ⚠️  WASM not found for $contract: $WASM_PATH"
    fi
done

# Deploy core contracts (16)
CORE_CONTRACTS=(moltcoin moltswap reef_storage compute_market lobsterlend clawpump moltmarket clawpay moltauction clawvault moltyid moltbridge moltdao moltoracle moltpunks molt_staking)
for contract in "${CORE_CONTRACTS[@]}"; do
    WASM_PATH="$ROOT_DIR/contracts/$contract/target/wasm32-unknown-unknown/release/${contract}.wasm"
    if [[ -f "$WASM_PATH" ]]; then
        log "  Deploying $contract..."
        python3 "$ROOT_DIR/tools/deploy_contract.py" --rpc "$RPC_URL" --name "$contract" --wasm "$WASM_PATH" 2>&1 | tee -a "$DEPLOY_LOG" || true
    else
        log "  ⚠️  WASM not found for $contract"
    fi
done

# ─────────────────────────────────────────────────────────────────────────────
# Phase 4: Create initial DEX pairs
# ─────────────────────────────────────────────────────────────────────────────
if [[ "$SEED_PAIRS" == "true" ]]; then
    log ""
    log "═══ Phase 4: Creating initial trading pairs ═══"

    # Pairs are now created by deploy_dex.py during phase_initialize_dex.
    # This phase ensures they exist by calling dex_core::create_pair for each.
    PAIRS=(
        "MOLT:mUSD"
        "wSOL:mUSD"
        "wETH:mUSD"
        "REEF:mUSD"
        "wSOL:MOLT"
        "wETH:MOLT"
        "REEF:MOLT"
    )
    for pair_spec in "${PAIRS[@]}"; do
        IFS=':' read -r base quote <<< "$pair_spec"
        log "  Creating pair: ${base}/${quote}"
        python3 -c "
import asyncio, sys
sys.path.insert(0, '$ROOT_DIR/tools')
from deploy_dex import load_or_create_deployer, call_contract
from moltchain_sdk_py import Connection
async def go():
    conn = Connection('$RPC_URL')
    deployer = load_or_create_deployer()
    import json
    manifest = json.load(open('$MANIFEST')) if __import__('os').path.exists('$MANIFEST') else {}
    addr = manifest.get('contracts', {}).get('dex_core')
    if not addr:
        print('  ⚠️  dex_core not in manifest, skipping')
        return
    try:
        sig = await call_contract(conn, deployer, addr, 'create_pair', {'base': '$base', 'quote': '$quote'})
        print(f'  ✅ {base}/{quote} → {sig}')
    except Exception as e:
        print(f'  ⚠️  {base}/{quote}: {e}')
asyncio.run(go())
" 2>&1 | tee -a "$DEPLOY_LOG" || true
    done

    log "✅ Pairs seeded"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Phase 5: Create initial AMM pools
# ─────────────────────────────────────────────────────────────────────────────
if [[ "$SEED_POOLS" == "true" ]]; then
    log ""
    log "═══ Phase 5: Creating initial AMM pools + Insurance fund ═══"

    # Create fee-tier pools for the main pairs and seed insurance fund
    python3 -c "
import asyncio, sys, json, os
sys.path.insert(0, '$ROOT_DIR/tools')
from deploy_dex import load_or_create_deployer, call_contract
from moltchain_sdk_py import Connection

async def go():
    conn = Connection('$RPC_URL')
    deployer = load_or_create_deployer()
    manifest = json.load(open('$MANIFEST')) if os.path.exists('$MANIFEST') else {}
    addrs = manifest.get('contracts', {})
    amm = addrs.get('dex_amm')
    margin = addrs.get('dex_margin')

    if amm:
        pools = [
            {'pair_id': 0, 'fee_bps': 30, 'sqrt_price': 648_000_000},   # MOLT/mUSD ~0.42
            {'pair_id': 1, 'fee_bps': 30, 'sqrt_price': 13_360_000_000}, # wSOL/mUSD ~178
            {'pair_id': 2, 'fee_bps': 30, 'sqrt_price': 59_345_000_000}, # wETH/mUSD ~3521
            {'pair_id': 3, 'fee_bps': 30, 'sqrt_price': 135_700_000},   # REEF/mUSD ~0.018
        ]
        for pool in pools:
            try:
                sig = await call_contract(conn, deployer, amm, 'create_pool', pool)
                print(f'  ✅ Pool pair_id={pool[\"pair_id\"]} → {sig}')
            except Exception as e:
                print(f'  ⚠️  Pool pair_id={pool[\"pair_id\"]}: {e}')
    else:
        print('  ⚠️  dex_amm not in manifest, skipping pools')

    if margin:
        try:
            sig = await call_contract(conn, deployer, margin, 'seed_insurance', {'amount': 10_000_000_000_000})
            print(f'  ✅ Insurance fund seeded → {sig}')
        except Exception as e:
            print(f'  ⚠️  Insurance seed: {e}')
    else:
        print('  ⚠️  dex_margin not in manifest, skipping insurance seed')

asyncio.run(go())
" 2>&1 | tee -a "$DEPLOY_LOG" || true

    log "✅ Pools + insurance seeded"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Phase 6: Verification
# ─────────────────────────────────────────────────────────────────────────────
log ""
log "═══ Phase 6: Deployment verification ═══"

# Check all contracts are deployed
CONTRACTS_RESP=$(curl -s -X POST "$RPC_URL" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"getAllContracts"}' 2>/dev/null || echo "{}")

DEPLOYED_COUNT=$(echo "$CONTRACTS_RESP" | python3 -c "
import json,sys
try:
    d = json.load(sys.stdin)
    r = d.get('result', [])
    print(len(r) if isinstance(r, list) else 0)
except:
    print(0)
" 2>/dev/null || echo "0")

log "Contracts deployed: $DEPLOYED_COUNT / 26"

# Check DEX API is responding
DEX_PAIRS=$(curl -s "$RPC_URL/api/v1/pairs" 2>/dev/null || echo "[]")
log "DEX API /api/v1/pairs: $(echo "$DEX_PAIRS" | head -c 100)"

log ""
log "═══════════════════════════════════════════════"
log "  Testnet deployment complete!"
log "  Contracts: $DEPLOYED_COUNT / 26"
log "  RPC:       $RPC_URL"
log "  DEX API:   $RPC_URL/api/v1/"
log "  Log:       $DEPLOY_LOG"
log "═══════════════════════════════════════════════"
