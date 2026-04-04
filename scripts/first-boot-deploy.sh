#!/bin/bash
# ============================================================================
# Lichen Post-Genesis Bootstrap
# ============================================================================
#
# Runs automatically after genesis to rebuild deploy-manifest.json from the
# live genesis deployment, align local helper key material, and regenerate the
# signed metadata manifest. Idempotent — if deploy-manifest.json already
# matches the live chain, the script refreshes the remaining bootstrap outputs
# without rebuilding the manifest.
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
SDK_PYTHON_DIR="${REPO_ROOT}/sdk/python"
SDK_REQUIREMENTS_FILE="${SDK_PYTHON_DIR}/requirements.txt"
MANIFEST="${REPO_ROOT}/deploy-manifest.json"
PYTHON_BIN="${PYTHON_BIN:-$REPO_ROOT/.venv/bin/python}"
if [[ ! -x "$PYTHON_BIN" ]]; then
    PYTHON_BIN="python3"
fi

RPC_URL="${CUSTODY_LICHEN_RPC_URL:-http://127.0.0.1:8899}"
SKIP_BUILD=false
MAX_RETRIES=30
RETRY_DELAY=2
SIGNED_METADATA_MANIFEST="${SIGNED_METADATA_MANIFEST:-${LICHEN_SIGNED_METADATA_MANIFEST_FILE:-}}"
SIGNED_METADATA_KEYPAIR="${SIGNED_METADATA_KEYPAIR:-${REPO_ROOT}/keypairs/release-signing-key.json}"
SIGNED_METADATA_NETWORK="${SIGNED_METADATA_NETWORK:-}"
SIGNED_METADATA_TARGET_HINT=""
SIGNED_METADATA_REQUIRED=false
DEPLOY_NETWORK="${DEPLOY_NETWORK:-}"
DEX_ADMIN_PUBKEY="${DEX_ADMIN_PUBKEY:-}"
MANIFEST_IN_SYNC=false

read_env_file_value() {
    local env_file="$1"
    local key="$2"

    if [[ ! -f "$env_file" ]]; then
        return 1
    fi

    if [[ -r "$env_file" ]]; then
        grep "^${key}=" "$env_file" | tail -1 | cut -d= -f2- || true
        return 0
    fi

    if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
        sudo grep "^${key}=" "$env_file" | tail -1 | cut -d= -f2- || true
        return 0
    fi

    return 1
}

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

ensure_python_runtime() {
    local bootstrap_python="python3"
    local venv_dir="${REPO_ROOT}/.venv"

    if ! command -v "$bootstrap_python" >/dev/null 2>&1; then
        echo -e "  ${RED}❌ python3 is required for post-genesis bootstrap${NC}"
        exit 1
    fi

    if [[ -x "$PYTHON_BIN" ]]; then
        if "$PYTHON_BIN" - <<'PY' >/dev/null 2>&1
import importlib
for module in ("base58", "cryptography", "dilithium_py", "httpx", "websockets"):
    importlib.import_module(module)
PY
        then
            return 0
        fi
    fi

    if [[ ! -f "$SDK_REQUIREMENTS_FILE" ]]; then
        echo -e "  ${RED}❌ Python requirements file not found at ${SDK_REQUIREMENTS_FILE}${NC}"
        exit 1
    fi

    echo -e "  ${YELLOW}⚠  Bootstrapping repo Python environment for deployment helpers...${NC}"
    if [[ -d "$venv_dir" ]]; then
        rm -rf "$venv_dir"
    fi
    "$bootstrap_python" -m venv "$venv_dir"
    "$venv_dir/bin/python" -m pip install --upgrade pip >/dev/null
    "$venv_dir/bin/pip" install -r "$SDK_REQUIREMENTS_FILE"
    PYTHON_BIN="$venv_dir/bin/python"
}

manifest_matches_live_chain() {
    local response

    response=$(curl -s -X POST "${RPC_URL}" \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","id":1,"method":"getAllSymbolRegistry","params":[100]}' 2>/dev/null || echo "")

    if [ -z "$response" ]; then
        return 1
    fi

    RPC_RESPONSE="$response" "$PYTHON_BIN" - "$MANIFEST" <<'PY' >/dev/null 2>&1
import json
import os
import sys

manifest_path = sys.argv[1]
manifest = json.load(open(manifest_path, encoding="utf-8"))
rpc = json.loads(os.environ["RPC_RESPONSE"])
result = rpc.get("result")
if not isinstance(result, dict):
    raise SystemExit(1)
entries = result.get("entries")
if not isinstance(entries, list):
    raise SystemExit(1)

registry = {
    entry.get("symbol"): entry.get("program")
    for entry in entries
    if isinstance(entry, dict) and entry.get("symbol") and entry.get("program")
}
expected = {
    "lusd_token": "LUSD",
    "wsol_token": "WSOL",
    "weth_token": "WETH",
    "wbnb_token": "WBNB",
    "lichenbridge": "BRIDGE",
    "lichenmarket": "MARKET",
    "lichenoracle": "ORACLE",
    "lichenauction": "AUCTION",
    "lichendao": "DAO",
    "thalllend": "LEND",
    "lichenpunks": "PUNKS",
    "lichenid": "YID",
    "lichenswap": "LICHENSWAP",
    "sporepay": "SPOREPAY",
    "sporepump": "SPOREPUMP",
    "sporevault": "SPOREVAULT",
    "bountyboard": "BOUNTY",
    "compute_market": "COMPUTE",
    "moss_storage": "MOSS",
    "shielded_pool": "SHIELDED",
    "dex_core": "DEX",
    "dex_amm": "DEXAMM",
    "dex_router": "DEXROUTER",
    "dex_margin": "DEXMARGIN",
    "dex_rewards": "DEXREWARDS",
    "dex_governance": "DEXGOV",
    "dex_analytics": "ANALYTICS",
    "prediction_market": "PREDICT",
}

contracts = manifest.get("contracts")
if not isinstance(contracts, dict):
    raise SystemExit(1)

for contract_name, symbol in expected.items():
    manifest_address = contracts.get(contract_name)
    live_address = registry.get(symbol)
    if not manifest_address or manifest_address != live_address:
        raise SystemExit(1)
PY
}

resolve_signed_metadata_defaults() {
    local env_file="/etc/lichen/env-${DEPLOY_NETWORK}"
    local configured_manifest=""

    if [[ -f "$env_file" ]]; then
        SIGNED_METADATA_REQUIRED=true
        configured_manifest="$(read_env_file_value "$env_file" "LICHEN_SIGNED_METADATA_MANIFEST_FILE")"
        if [[ -n "$configured_manifest" ]]; then
            SIGNED_METADATA_TARGET_HINT="$configured_manifest"
            if [[ -z "$SIGNED_METADATA_MANIFEST" && -w "$(dirname "$configured_manifest")" ]]; then
                SIGNED_METADATA_MANIFEST="$configured_manifest"
            fi
        fi
    fi

    if [[ -z "$SIGNED_METADATA_MANIFEST" ]]; then
        SIGNED_METADATA_MANIFEST="${REPO_ROOT}/signed-metadata-manifest-${DEPLOY_NETWORK}.json"
    fi

    if [[ -z "$SIGNED_METADATA_NETWORK" ]]; then
        if [[ -f "$env_file" ]]; then
            SIGNED_METADATA_NETWORK="$DEPLOY_NETWORK"
        else
            case "$DEPLOY_NETWORK" in
                mainnet) SIGNED_METADATA_NETWORK="local-mainnet" ;;
                *) SIGNED_METADATA_NETWORK="local-testnet" ;;
            esac
        fi
    fi
}

install_signed_metadata_manifest() {
    if [[ -z "$SIGNED_METADATA_TARGET_HINT" || "$SIGNED_METADATA_MANIFEST" == "$SIGNED_METADATA_TARGET_HINT" ]]; then
        return 0
    fi

    if [[ ! -f "$SIGNED_METADATA_MANIFEST" ]]; then
        return 1
    fi

    if [ "$(id -u)" -eq 0 ]; then
        install -m 640 -o root -g lichen "$SIGNED_METADATA_MANIFEST" "$SIGNED_METADATA_TARGET_HINT"
        return 0
    fi

    if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then
        sudo install -m 640 -o root -g lichen "$SIGNED_METADATA_MANIFEST" "$SIGNED_METADATA_TARGET_HINT"
        return 0
    fi

    return 1
}

verify_signed_metadata_served() {
    local response

    response=$(curl -s -X POST "${RPC_URL}" \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","id":1,"method":"getSignedMetadataManifest","params":[]}' 2>/dev/null || echo "")

    if [[ -z "$response" ]]; then
        return 1
    fi

    RPC_RESPONSE="$response" "$PYTHON_BIN" - "$MANIFEST" <<'PY' >/dev/null 2>&1
import json
import os
import sys

manifest_path = sys.argv[1]
manifest = json.load(open(manifest_path, encoding="utf-8"))
rpc = json.loads(os.environ["RPC_RESPONSE"])
result = rpc.get("result")
if not isinstance(result, dict):
    raise SystemExit(1)

payload = result.get("payload")
if not isinstance(payload, dict):
    raise SystemExit(1)

entries = payload.get("symbol_registry")
if not isinstance(entries, list):
    raise SystemExit(1)

registry = {
    entry.get("symbol"): entry.get("program")
    for entry in entries
    if isinstance(entry, dict) and entry.get("symbol") and entry.get("program")
}
expected = {
    "lusd_token": "LUSD",
    "wsol_token": "WSOL",
    "weth_token": "WETH",
    "wbnb_token": "WBNB",
    "lichenbridge": "BRIDGE",
    "lichenmarket": "MARKET",
    "lichenoracle": "ORACLE",
    "lichenauction": "AUCTION",
    "lichendao": "DAO",
    "thalllend": "LEND",
    "lichenpunks": "PUNKS",
    "lichenid": "YID",
    "lichenswap": "LICHENSWAP",
    "sporepay": "SPOREPAY",
    "sporepump": "SPOREPUMP",
    "sporevault": "SPOREVAULT",
    "bountyboard": "BOUNTY",
    "compute_market": "COMPUTE",
    "moss_storage": "MOSS",
    "shielded_pool": "SHIELDED",
    "dex_core": "DEX",
    "dex_amm": "DEXAMM",
    "dex_router": "DEXROUTER",
    "dex_margin": "DEXMARGIN",
    "dex_rewards": "DEXREWARDS",
    "dex_governance": "DEXGOV",
    "dex_analytics": "ANALYTICS",
    "prediction_market": "PREDICT",
}

contracts = manifest.get("contracts")
if not isinstance(contracts, dict):
    raise SystemExit(1)

for contract_name, symbol in expected.items():
    manifest_address = contracts.get(contract_name)
    served_address = registry.get(symbol)
    if not manifest_address or manifest_address != served_address:
        raise SystemExit(1)
PY
}

write_manifest_from_chain() {
    local response

    response=$(curl -s -X POST "${RPC_URL}" \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","id":1,"method":"getAllSymbolRegistry","params":[100]}' 2>/dev/null || echo "")

    if [[ -z "$response" ]]; then
        return 1
    fi

    RPC_RESPONSE="$response" "$PYTHON_BIN" - "$MANIFEST" <<'PY'
import datetime
import json
import os
import sys

manifest_path = sys.argv[1]
rpc = json.loads(os.environ["RPC_RESPONSE"])
result = rpc.get("result")
entries = []
if isinstance(result, dict):
    entries = result.get("entries", [])
elif isinstance(result, list):
    entries = result
if not isinstance(entries, list):
    raise SystemExit(1)

symbol_to_contract = {
    "LUSD": "lusd_token",
    "WSOL": "wsol_token",
    "WETH": "weth_token",
    "WBNB": "wbnb_token",
    "DEX": "dex_core",
    "DEXAMM": "dex_amm",
    "DEXROUTER": "dex_router",
    "DEXMARGIN": "dex_margin",
    "DEXREWARDS": "dex_rewards",
    "DEXGOV": "dex_governance",
    "ANALYTICS": "dex_analytics",
    "LICHENSWAP": "lichenswap",
    "BRIDGE": "lichenbridge",
    "MARKET": "lichenmarket",
    "ORACLE": "lichenoracle",
    "AUCTION": "lichenauction",
    "DAO": "lichendao",
    "LEND": "thalllend",
    "PUNKS": "lichenpunks",
    "YID": "lichenid",
    "SPOREPAY": "sporepay",
    "SPOREPUMP": "sporepump",
    "SPOREVAULT": "sporevault",
    "BOUNTY": "bountyboard",
    "COMPUTE": "compute_market",
    "MOSS": "moss_storage",
    "SHIELDED": "shielded_pool",
    "PREDICT": "prediction_market",
}

contracts = {}
owners = set()
for entry in entries:
    if not isinstance(entry, dict):
        continue
    symbol = str(entry.get("symbol") or "").upper()
    program = entry.get("program")
    owner = entry.get("owner")
    contract_name = symbol_to_contract.get(symbol)
    if not contract_name or not isinstance(program, str) or not program:
        continue
    contracts[contract_name] = program
    if isinstance(owner, str) and owner:
        owners.add(owner)

missing = sorted(set(symbol_to_contract.values()) - set(contracts))
if missing:
    print("missing registry entries: " + ", ".join(missing), file=sys.stderr)
    raise SystemExit(1)

sorted_contracts = {name: contracts[name] for name in sorted(contracts)}
manifest = {
    "deployer": next(iter(owners)) if len(owners) == 1 else "",
    "deployed_at": datetime.datetime.utcnow().replace(microsecond=0).isoformat() + "Z",
    "contracts": sorted_contracts,
    "token_contracts": {
        "lUSD": sorted_contracts.get("lusd_token"),
        "wSOL": sorted_contracts.get("wsol_token"),
        "wETH": sorted_contracts.get("weth_token"),
        "wBNB": sorted_contracts.get("wbnb_token"),
    },
    "dex_contracts": {
        name: sorted_contracts[name]
        for name in [
            "dex_core",
            "dex_amm",
            "dex_router",
            "dex_margin",
            "dex_rewards",
            "dex_governance",
            "dex_analytics",
            "prediction_market",
        ]
    },
    "trading_pairs": [
        "LICN/lUSD",
        "wSOL/lUSD",
        "wETH/lUSD",
        "wBNB/lUSD",
        "wSOL/LICN",
        "wETH/LICN",
        "wBNB/LICN",
    ],
}

with open(manifest_path, "w", encoding="utf-8") as handle:
    json.dump(manifest, handle, indent=2)
    handle.write("\n")
PY
}

align_local_helper_keypair() {
    local source_key=""

    source_key=$(command find "$REPO_ROOT/data" -type f -path '*/genesis-keys/genesis-primary-*.json' | sort | head -1 || true)
    if [[ -z "$source_key" ]]; then
        return 1
    fi

    mkdir -p "$REPO_ROOT/keypairs"
    cp "$source_key" "$REPO_ROOT/keypairs/deployer.json"
    return 0
}


ensure_python_runtime

# Parse args
while [[ $# -gt 0 ]]; do
    case "$1" in
        --rpc=*)
            RPC_URL="${1#*=}"
            shift
            ;;
        --rpc)
            if [[ $# -lt 2 ]]; then
                echo -e "  ${RED}❌ --rpc requires a value${NC}"
                exit 1
            fi
            RPC_URL="$2"
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=true
            shift
            ;;
        --force)
            rm -f "$MANIFEST"
            shift
            ;;
        *)
            echo -e "  ${RED}❌ Unknown argument: $1${NC}"
            exit 1
            ;;
    esac
done

if [ -z "$DEPLOY_NETWORK" ]; then
    case "$RPC_URL" in
        *:9899*|*:9901*|*:9903*|*:9905*) DEPLOY_NETWORK="mainnet" ;;
        *) DEPLOY_NETWORK="testnet" ;;
    esac
fi

CUSTODY_TREASURY_TARGET="${CUSTODY_TREASURY_TARGET:-/etc/lichen/custody-treasury-${DEPLOY_NETWORK}.json}"
resolve_signed_metadata_defaults

echo -e "${CYAN}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  🦞 Lichen Post-Genesis Bootstrap                 ║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════╝${NC}"
echo -e "  RPC:      ${RPC_URL}"
echo -e "  Manifest: ${MANIFEST}"
echo -e "  Metadata: ${SIGNED_METADATA_MANIFEST}"

# ─────────────────────────────────────────────────────────
# Step 1: Check if already deployed
# ─────────────────────────────────────────────────────────
if [ -f "$MANIFEST" ]; then
    CONTRACT_COUNT=$($PYTHON_BIN -c "import json; m=json.load(open('$MANIFEST')); print(len(m.get('contracts',{})))" 2>/dev/null || echo "0")
    if [ "$CONTRACT_COUNT" -ge 10 ] && manifest_matches_live_chain; then
        MANIFEST_IN_SYNC=true
        echo -e "\n  ${GREEN}✅ Deploy manifest exists with ${CONTRACT_COUNT} contracts.${NC}"
        echo -e "  ${GREEN}   Manifest matches live symbol registry; refreshing bootstrap outputs.${NC}"
    else
        echo -e "  ${YELLOW}⚠  Existing manifest does not match the live chain. Redeploying...${NC}"
    fi
fi

# ─────────────────────────────────────────────────────────
# Step 2: Wait for validator to be healthy
# ─────────────────────────────────────────────────────────
echo -e "\n${CYAN}[1/4]${NC} Waiting for validator at ${RPC_URL}..."

HEALTHY=false
for i in $(seq 1 $MAX_RETRIES); do
    RESPONSE=$(curl -s -X POST "${RPC_URL}" \
        -H "Content-Type: application/json" \
        -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' 2>/dev/null || echo "")
    
    if echo "$RESPONSE" | $PYTHON_BIN -c "import json,sys; r=json.load(sys.stdin); result=r.get('result'); assert result is True or result in ['ok','healthy'] or (isinstance(result, dict) and result.get('status') in ['ok','healthy',True])" 2>/dev/null; then
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
# Step 3: Rebuild deploy manifest from the live genesis catalog
# ─────────────────────────────────────────────────────────
if $SKIP_BUILD; then
    echo -e "\n${CYAN}[2/4]${NC} Contract build skipped (--skip-build); genesis already deployed the catalog"
else
    echo -e "\n${CYAN}[2/4]${NC} Skipping contract builds; genesis already deployed the catalog"
fi

if $MANIFEST_IN_SYNC; then
    echo -e "\n${CYAN}[3/4]${NC} Deploy manifest already matches live symbol registry"
else
    echo -e "\n${CYAN}[3/4]${NC} Rebuilding deploy manifest from live symbol registry..."
    if write_manifest_from_chain; then
        echo -e "  ${GREEN}✅ Manifest rebuilt from live genesis deployment${NC}"
    else
        echo -e "  ${RED}❌ Failed to rebuild deploy manifest from the live chain${NC}"
        exit 1
    fi
fi

# ─────────────────────────────────────────────────────────
# Step 4: Keep local helper key material aligned with genesis admin
# ─────────────────────────────────────────────────────────
echo -e "\n${CYAN}[4/4]${NC} Aligning local helper key material..."
if align_local_helper_keypair; then
    echo -e "  ${GREEN}✅ Local deployer helper aligned to genesis primary keypair${NC}"
else
    echo -e "  ${YELLOW}⚠  No repo-local genesis primary keypair found under ${REPO_ROOT}/data${NC}"
fi

# ─────────────────────────────────────────────────────────
# Final verification
# ─────────────────────────────────────────────────────────
if [ -f "$SIGNED_METADATA_KEYPAIR" ] && command -v node >/dev/null 2>&1; then
    echo -e "\n${CYAN}[4b/4]${NC} Generating signed metadata manifest..."
    if node "${SCRIPT_DIR}/generate-signed-metadata-manifest.js" \
        --rpc "$RPC_URL" \
        --network "$SIGNED_METADATA_NETWORK" \
        --keypair "$SIGNED_METADATA_KEYPAIR" \
        --out "$SIGNED_METADATA_MANIFEST" 2>&1 | sed 's/^/    /'; then
        echo -e "  ${GREEN}✅ Signed metadata manifest generated${NC}"
    else
        echo -e "  ${YELLOW}⚠  Signed metadata manifest generation failed${NC}"
        if $SIGNED_METADATA_REQUIRED; then
            echo -e "  ${RED}❌ Deployment aborted: public RPC nodes must regenerate signed metadata after contract deployment${NC}"
            exit 1
        fi
    fi
elif ! command -v node >/dev/null 2>&1; then
    if $SIGNED_METADATA_REQUIRED; then
        echo -e "\n  ${RED}❌ Deployment aborted: Node.js is required to generate signed metadata on VPS deploys${NC}"
        exit 1
    fi
    echo -e "\n  ${YELLOW}⚠  Skipping signed metadata manifest generation because Node.js is unavailable${NC}"
else
    if $SIGNED_METADATA_REQUIRED; then
        echo -e "\n  ${RED}❌ Deployment aborted: signing keypair missing at ${SIGNED_METADATA_KEYPAIR}${NC}"
        exit 1
    fi
    echo -e "\n  ${YELLOW}⚠  Skipping signed metadata manifest generation because ${SIGNED_METADATA_KEYPAIR} is missing${NC}"
fi

if [ -f "$SIGNED_METADATA_MANIFEST" ] \
    && [ -n "$SIGNED_METADATA_TARGET_HINT" ] \
    && [ "$SIGNED_METADATA_MANIFEST" != "$SIGNED_METADATA_TARGET_HINT" ]; then
    echo -e "  Installing signed metadata manifest into ${SIGNED_METADATA_TARGET_HINT}"
    if install_signed_metadata_manifest; then
        echo -e "  ${GREEN}✅ Installed signed metadata manifest at ${SIGNED_METADATA_TARGET_HINT}${NC}"
    else
        echo -e "  ${YELLOW}⚠  Could not install signed metadata manifest into ${SIGNED_METADATA_TARGET_HINT}${NC}"
        echo -e "      sudo install -m 640 -o root -g lichen \"$SIGNED_METADATA_MANIFEST\" \"$SIGNED_METADATA_TARGET_HINT\""
        if $SIGNED_METADATA_REQUIRED; then
            echo -e "  ${RED}❌ Deployment aborted: public RPC nodes must serve the updated signed metadata manifest${NC}"
            exit 1
        fi
    fi
fi

if [ -f "$SIGNED_METADATA_MANIFEST" ]; then
    echo -e "  Verifying signed metadata served by RPC..."
    if verify_signed_metadata_served; then
        echo -e "  ${GREEN}✅ RPC serves signed metadata matching deployed contract addresses${NC}"
    else
        echo -e "  ${YELLOW}⚠  RPC signed metadata does not match deployed contracts${NC}"
        if $SIGNED_METADATA_REQUIRED; then
            echo -e "  ${RED}❌ Deployment aborted: DEX-critical symbol metadata is not being served correctly${NC}"
            exit 1
        fi
    fi
fi

echo -e "\n${CYAN}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║  POST-GENESIS BOOTSTRAP COMPLETE                         ║${NC}"
echo -e "${CYAN}╚══════════════════════════════════════════════════════════╝${NC}"

if [ -f "$MANIFEST" ]; then
    CONTRACT_COUNT=$($PYTHON_BIN -c "import json; m=json.load(open('$MANIFEST')); print(len(m.get('contracts',{})))" 2>/dev/null || echo "0")
    echo -e "  ${GREEN}Manifest: ${MANIFEST}${NC}"
    echo -e "  ${GREEN}Deployed: ${CONTRACT_COUNT} contracts${NC}"
    
    # Print contract addresses
    $PYTHON_BIN -c "
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
