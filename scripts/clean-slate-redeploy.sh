#!/bin/bash
# ============================================================================
# Lichen Clean-Slate VPS Redeploy — Fully Automated
# ============================================================================
#
# Stops all services, flushes state, rebuilds, creates genesis, distributes
# secrets, starts everything, and verifies — all in one shot.
#
# Usage:
#   bash scripts/clean-slate-redeploy.sh              # testnet (default)
#   bash scripts/clean-slate-redeploy.sh mainnet       # mainnet
#
# Prerequisites:
#   - SSH access to all VPSes (port 2222, user ubuntu, key-based auth)
#   - deploy/setup.sh already run on all VPSes (systemd, users, dirs exist)
#   - keypairs/release-signing-key.json present in repo
#   - Code committed and pushed to main
#
# Secrets distributed automatically via tarball (atomic, no partial copies):
#   - genesis-wallet.json + genesis-keys/ (treasury for airdrop)
#   - custody-treasury, faucet keypair
#   - custody master+deposit seeds
#   - signed metadata manifest
#   - release signing key
#
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/.."
cd "$REPO_ROOT"

# ── Configuration ──
NETWORK="${1:-testnet}"

GENESIS_VPS="15.204.229.189"
JOINING_VPSES=("37.59.97.61" "15.235.142.253")
ALL_VPSES=("$GENESIS_VPS" "${JOINING_VPSES[@]}")

SSH_PORT=2222
SSH_USER=ubuntu
SSH_OPTS="-p $SSH_PORT -o ConnectTimeout=10 -o ServerAliveInterval=5 -o ServerAliveCountMax=3 -o StrictHostKeyChecking=no -o BatchMode=yes"

VPS_DATA="/var/lib/lichen"
VPS_CONFIG="/etc/lichen"
STATE_DIR="state-${NETWORK}"
SERVICE="lichen-validator-${NETWORK}"

case $NETWORK in
  testnet) RPC_PORT=8899 ;;
  mainnet) RPC_PORT=9899 ;;
  *) echo "Usage: $0 [testnet|mainnet]"; exit 1 ;;
esac

# ── Colors ──
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; BOLD='\033[1m'; NC='\033[0m'

# ── Helpers ──
TOTAL_START=$(date +%s)
PHASE=0

phase() {
  PHASE=$((PHASE + 1))
  PHASE_START=$(date +%s)
  echo ""
  echo -e "${BOLD}${CYAN}═══ Phase $PHASE: $1 ═══${NC}"
}

phase_done() {
  local elapsed=$(( $(date +%s) - PHASE_START ))
  echo -e "${GREEN}  ✓ Phase $PHASE done (${elapsed}s)${NC}"
}

ssh_run() {
  local host=$1; shift
  local retries=3 delay=3
  for i in $(seq 1 $retries); do
    if ssh $SSH_OPTS $SSH_USER@"$host" "$@" 2>&1; then
      return 0
    fi
    if [ "$i" -lt "$retries" ]; then
      echo -e "  ${YELLOW}SSH $host failed (attempt $i/$retries), retry in ${delay}s${NC}" >&2
      sleep $delay
      delay=$((delay * 2))
    fi
  done
  echo -e "${RED}FATAL: SSH $host failed after $retries attempts${NC}" >&2
  return 1
}

ssh_pipe() {
  # Pipe from one VPS to another: ssh_pipe SRC DST "src_cmd" "dst_cmd"
  local src=$1 dst=$2 src_cmd=$3 dst_cmd=$4
  ssh $SSH_OPTS $SSH_USER@"$src" "$src_cmd" \
    | ssh $SSH_OPTS $SSH_USER@"$dst" "$dst_cmd"
}

# ── Preflight checks ──
echo -e "${BOLD}Lichen Clean-Slate Redeploy ($NETWORK)${NC}"
echo ""

if [ ! -f keypairs/release-signing-key.json ]; then
  echo -e "${RED}Missing keypairs/release-signing-key.json${NC}"
  exit 1
fi

echo "Verifying SSH access..."
for VPS in "${ALL_VPSES[@]}"; do
  ssh_run "$VPS" "echo OK" > /dev/null || { echo "Cannot SSH to $VPS"; exit 1; }
  echo -e "  ${GREEN}✓${NC} $VPS"
done

# ============================================================================
# Phase 1: Stop everything
# ============================================================================
phase "Stop all services"
for VPS in "${ALL_VPSES[@]}"; do
  echo "  Stopping $VPS..."
  ssh_run "$VPS" "
    sudo systemctl stop lichen-faucet 2>/dev/null || true
    sudo systemctl stop lichen-custody 2>/dev/null || true
    sudo systemctl stop lichen-custody-mainnet 2>/dev/null || true
    sudo systemctl stop $SERVICE 2>/dev/null || true
  "
done
phase_done

# ============================================================================
# Phase 2: Flush state
# ============================================================================
phase "Flush state on all VPSes"
for VPS in "${ALL_VPSES[@]}"; do
  echo "  Flushing $VPS..."
  ssh_run "$VPS" "
    sudo rm -rf $VPS_DATA/$STATE_DIR
    sudo rm -rf $VPS_DATA/.lichen
    sudo rm -rf $VPS_DATA/custody-db
    sudo rm -f $VPS_CONFIG/signed-metadata-manifest-${NETWORK}.json
    sudo rm -f $VPS_CONFIG/custody-treasury-${NETWORK}.json
    sudo rm -f $VPS_DATA/faucet-keypair-${NETWORK}.json
    sudo mkdir -p $VPS_DATA/$STATE_DIR
    sudo chown lichen:lichen $VPS_DATA/$STATE_DIR
    sudo mkdir -p $VPS_DATA/custody-db
    sudo chown lichen:lichen $VPS_DATA/custody-db
  "
done
phase_done

# ============================================================================
# Phase 3: Git pull + Build
# ============================================================================
phase "Sync latest code and build"

# Rsync code to all VPSes (they may not have .git — rsynced previously)
for VPS in "${ALL_VPSES[@]}"; do
  echo "  Syncing code to $VPS..."
  rsync -az --delete \
    --exclude target/ --exclude compiler/target/ --exclude node_modules/ \
    --exclude data/ --exclude logs/ --exclude .git/ --exclude .venv/ \
    --exclude '*.pyc' --exclude __pycache__/ \
    -e "ssh -p $SSH_PORT -o StrictHostKeyChecking=no" \
    "$REPO_ROOT/" "$SSH_USER@$VPS:~/lichen/"
done

# Build joining VPSes in background (they only need validator + support binaries)
JOINER_PIDS=()
for VPS in "${JOINING_VPSES[@]}"; do
  echo "  Building $VPS (background)..."
  ssh_run "$VPS" "
    cd ~/lichen && source ~/.cargo/env
    cargo build --release --bin lichen-validator --bin lichen --bin lichen-custody --bin lichen-faucet --bin zk-prove 2>&1 | tail -3
  " &
  JOINER_PIDS+=($!)
done

# Build genesis VPS (all binaries + WASM contracts) — blocking
echo "  Building $GENESIS_VPS (all + WASM)..."
ssh_run "$GENESIS_VPS" '
  cd ~/lichen && source ~/.cargo/env
  cargo build --release 2>&1 | tail -3
  echo "  Binaries done, building WASM contracts..."
  make build-contracts-wasm 2>&1 | tail -5
  echo "  Build complete"
'

# Wait for joining VPS builds
for pid in "${JOINER_PIDS[@]}"; do
  wait "$pid" || { echo -e "${RED}Joining VPS build failed${NC}"; exit 1; }
done
phase_done

# ============================================================================
# Phase 4: Genesis on seed-01
# ============================================================================
phase "Create genesis on $GENESIS_VPS"
ssh_run "$GENESIS_VPS" '
  cd ~/lichen && source ~/.cargo/env
  NET='"$NETWORK"'
  STATE='"$VPS_DATA/$STATE_DIR"'

  KP_PASS=$(sudo grep LICHEN_KEYPAIR_PASSWORD /etc/lichen/env-'"$NETWORK"' | cut -d= -f2-)

  # 1. Generate validator keypair
  echo "  Generating validator keypair..."
  sudo -u lichen env LICHEN_KEYPAIR_PASSWORD="$KP_PASS" \
    ./target/release/lichen init --output "$STATE/validator-keypair.json"

  PUBKEY=$(sudo python3 -c "import json; print(json.load(open('"'"''"$STATE"'/validator-keypair.json'"'"'))['publicKeyBase58'])")
  echo "  Validator pubkey: $PUBKEY"

  # 2. Prepare wallet
  echo "  Preparing wallet..."
  sudo -u lichen env HOME='"$VPS_DATA"' LICHEN_HOME='"$VPS_DATA"' \
    LICHEN_CONTRACTS_DIR=$HOME/lichen/contracts \
    LICHEN_KEYPAIR_PASSWORD="$KP_PASS" \
    ./target/release/lichen-genesis --prepare-wallet --network "$NET" --output-dir "$STATE"

  # 3. Fetch live prices
  echo "  Fetching prices..."
  PRICE_JSON=$(curl -sf "https://api.binance.com/api/v3/ticker/price?symbols=[\"SOLUSDT\",\"ETHUSDT\",\"BNBUSDT\"]" 2>/dev/null || echo "[]")
  SOL=145.0; ETH=2600.0; BNB=620.0
  if [ "$PRICE_JSON" != "[]" ] && command -v python3 &>/dev/null; then
    eval "$(python3 -c "
import json
try:
    data = json.loads('"'"'$PRICE_JSON'"'"')
    m = {d['"'"'symbol'"'"']: float(d['"'"'price'"'"']) for d in data}
    print(f'"'"'SOL={m.get(\"SOLUSDT\", 145.0):.2f}'"'"')
    print(f'"'"'ETH={m.get(\"ETHUSDT\", 2600.0):.2f}'"'"')
    print(f'"'"'BNB={m.get(\"BNBUSDT\", 620.0):.2f}'"'"')
except: pass
" 2>/dev/null)" || true
  fi
  echo "  Prices: SOL=$SOL ETH=$ETH BNB=$BNB"

  # 4. Create genesis
  echo "  Creating genesis block..."
  sudo -u lichen env HOME='"$VPS_DATA"' LICHEN_HOME='"$VPS_DATA"' \
    LICHEN_CONTRACTS_DIR=$HOME/lichen/contracts \
    LICHEN_KEYPAIR_PASSWORD="$KP_PASS" \
    GENESIS_SOL_USD="$SOL" GENESIS_ETH_USD="$ETH" GENESIS_BNB_USD="$BNB" \
    ./target/release/lichen-genesis \
      --network "$NET" \
      --db-path "$STATE" \
      --wallet-file "$STATE/genesis-wallet.json" \
      --initial-validator "$PUBKEY"
  echo "  Genesis created!"

  # 5. Install seeds.json
  sudo install -m 644 -o lichen -g lichen ~/lichen/seeds.json "$STATE/seeds.json"

  # 6. Start genesis validator
  echo "  Starting genesis validator..."
  sudo systemctl start '"$SERVICE"'
  sleep 8

  # 7. Verify block production
  SLOT=$(curl -sf http://127.0.0.1:'"$RPC_PORT"' -X POST -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSlot\",\"params\":[]}" \
    | python3 -c "import sys,json; print(json.load(sys.stdin)[\"result\"])" 2>/dev/null || echo "FAIL")
  echo "  Slot: $SLOT"
  if [ "$SLOT" = "FAIL" ] || [ "$SLOT" -lt 1 ] 2>/dev/null; then
    echo "FATAL: Genesis validator not producing blocks!"
    exit 1
  fi
'
phase_done

# ============================================================================
# Phase 5: Post-genesis + first-boot-deploy on seed-01
# ============================================================================
phase "Post-genesis on $GENESIS_VPS"
ssh_run "$GENESIS_VPS" '
  cd ~/lichen && source ~/.cargo/env
  NET='"$NETWORK"'
  KP_PASS=$(sudo grep LICHEN_KEYPAIR_PASSWORD /etc/lichen/env-'"$NETWORK"' | cut -d= -f2-)

  # 1. Post-genesis keypair setup (copies treasury → custody, faucet keypair)
  echo "  Running vps-post-genesis..."
  sudo bash scripts/vps-post-genesis.sh '"$NETWORK"' --no-restart 2>&1 | grep -E "✓|✗|⚠|genesis-keys" || true

  # 2. Install release signing key
  echo "  Installing release signing key..."
  sudo install -m 640 -o root -g lichen \
    ~/lichen/keypairs/release-signing-key.json \
    /etc/lichen/secrets/release-signing-keypair-'"$NETWORK"'.json

  # 3. Run first-boot-deploy (deploys 28 contracts, creates manifest)
  echo "  Running first-boot-deploy..."
  sudo cp /etc/lichen/secrets/release-signing-keypair-'"$NETWORK"'.json ~/release-signing-keypair-'"$NETWORK"'.json
  sudo chown $(whoami):$(whoami) ~/release-signing-keypair-'"$NETWORK"'.json
  chmod 600 ~/release-signing-keypair-'"$NETWORK"'.json

  SIGNED_METADATA_KEYPAIR=$HOME/release-signing-keypair-'"$NETWORK"'.json \
    DEPLOY_NETWORK='"$NETWORK"' \
    LICHEN_KEYPAIR_PASSWORD="$KP_PASS" \
    ./scripts/first-boot-deploy.sh --rpc http://127.0.0.1:'"$RPC_PORT"' --skip-build 2>&1 | tail -10

  rm -f ~/release-signing-keypair-'"$NETWORK"'.json

  # 4. Install signed metadata manifest
  echo "  Installing signed metadata manifest..."
  if [ -f ~/lichen/signed-metadata-manifest-'"$NETWORK"'.json ]; then
    sudo install -m 640 -o root -g lichen \
      ~/lichen/signed-metadata-manifest-'"$NETWORK"'.json \
      /etc/lichen/signed-metadata-manifest-'"$NETWORK"'.json
  fi

  # 5. Restart validator to pick up manifest
  sudo systemctl restart '"$SERVICE"'
  sleep 5

  # 6. Provision custody seeds
  echo "  Provisioning custody seeds..."
  sudo bash -c "openssl rand -hex 32 > /etc/lichen/secrets/custody-master-seed-'"$NETWORK"'.txt"
  sudo bash -c "openssl rand -hex 32 > /etc/lichen/secrets/custody-deposit-seed-'"$NETWORK"'.txt"
  sudo chown root:lichen /etc/lichen/secrets/custody-*-seed-'"$NETWORK"'.txt
  sudo chmod 640 /etc/lichen/secrets/custody-*-seed-'"$NETWORK"'.txt

  # 7. Start custody and faucet
  echo "  Starting custody and faucet..."
  sudo systemctl start lichen-custody
  sudo systemctl start lichen-faucet
  sleep 3

  # 8. Quick verify
  echo "  Verifying genesis VPS..."
  curl -sf http://127.0.0.1:'"$RPC_PORT"' -X POST -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\",\"params\":[]}" || echo "HEALTH FAIL"
  echo ""

  AIRDROP=$(curl -sf http://127.0.0.1:'"$RPC_PORT"' -X POST -H "Content-Type: application/json" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"requestAirdrop\",\"params\":[\"11111111111111111111111111111111\", 1000000000]}" 2>/dev/null || echo "FAIL")
  if echo "$AIRDROP" | grep -q "Treasury keypair not configured"; then
    echo "  FATAL: Treasury NOT loaded on genesis VPS!"
    exit 1
  fi
  echo "  Treasury: OK"
  echo "  Genesis VPS fully operational!"
'
phase_done

# ============================================================================
# Phase 6: Bundle and distribute secrets to joining VPSes
# ============================================================================
phase "Distribute secrets via tarball"

# Create tarball on genesis VPS with ALL secrets in one shot
echo "  Creating secrets bundle on $GENESIS_VPS..."
ssh_run "$GENESIS_VPS" "
  sudo tar czf /tmp/lichen-secrets-bundle.tar.gz -C / \
    var/lib/lichen/$STATE_DIR/genesis-wallet.json \
    var/lib/lichen/$STATE_DIR/genesis-keys \
    var/lib/lichen/faucet-keypair-${NETWORK}.json \
    etc/lichen/secrets/custody-master-seed-${NETWORK}.txt \
    etc/lichen/secrets/custody-deposit-seed-${NETWORK}.txt \
    etc/lichen/signed-metadata-manifest-${NETWORK}.json \
    etc/lichen/custody-treasury-${NETWORK}.json \
    etc/lichen/secrets/release-signing-keypair-${NETWORK}.json \
    2>/dev/null
  sudo chmod 644 /tmp/lichen-secrets-bundle.tar.gz
  echo \"  Bundle size: \$(du -h /tmp/lichen-secrets-bundle.tar.gz | cut -f1)\"
"

for VPS in "${JOINING_VPSES[@]}"; do
  echo "  Distributing to $VPS (single atomic transfer)..."

  # Single pipe: genesis → tar → joining VPS → extract + fix perms
  ssh_pipe "$GENESIS_VPS" "$VPS" \
    "cat /tmp/lichen-secrets-bundle.tar.gz" \
    "sudo tar xzf - -C / && \
     sudo chown -R lichen:lichen $VPS_DATA/$STATE_DIR/ && \
     sudo chmod 640 $VPS_DATA/$STATE_DIR/genesis-wallet.json && \
     sudo find $VPS_DATA/$STATE_DIR/genesis-keys -type f -exec chmod 640 {} + && \
     sudo chown lichen:lichen $VPS_DATA/faucet-keypair-${NETWORK}.json && \
     sudo chmod 600 $VPS_DATA/faucet-keypair-${NETWORK}.json && \
     sudo chown root:lichen $VPS_CONFIG/secrets/custody-*-seed-${NETWORK}.txt && \
     sudo chmod 640 $VPS_CONFIG/secrets/custody-*-seed-${NETWORK}.txt && \
     sudo chown root:lichen $VPS_CONFIG/signed-metadata-manifest-${NETWORK}.json && \
     sudo chmod 640 $VPS_CONFIG/signed-metadata-manifest-${NETWORK}.json && \
     sudo chown lichen:lichen $VPS_CONFIG/custody-treasury-${NETWORK}.json && \
     sudo chmod 600 $VPS_CONFIG/custody-treasury-${NETWORK}.json && \
     sudo chown root:lichen $VPS_CONFIG/secrets/release-signing-keypair-${NETWORK}.json && \
     sudo chmod 640 $VPS_CONFIG/secrets/release-signing-keypair-${NETWORK}.json"

  # Verify
  COUNT=$(ssh_run "$VPS" "sudo ls $VPS_DATA/$STATE_DIR/genesis-keys/ 2>/dev/null | wc -l")
  WALLET=$(ssh_run "$VPS" "sudo test -f $VPS_DATA/$STATE_DIR/genesis-wallet.json && echo YES || echo NO")
  echo -e "  ${GREEN}✓${NC} $VPS: $COUNT genesis-keys, wallet=$WALLET"
done

# Clean up bundle
ssh_run "$GENESIS_VPS" "sudo rm -f /tmp/lichen-secrets-bundle.tar.gz"
phase_done

# ============================================================================
# Phase 7: Start joining VPSes
# ============================================================================
phase "Start joining VPSes"

for VPS in "${JOINING_VPSES[@]}"; do
  echo "  Starting $VPS..."
  ssh_run "$VPS" '
    # Install seeds.json
    sudo install -m 644 -o lichen -g lichen ~/lichen/seeds.json '"$VPS_DATA/$STATE_DIR"'/seeds.json

    # Start validator
    sudo systemctl start '"$SERVICE"'
    sleep 12

    # Verify syncing
    HEALTH=$(curl -sf http://127.0.0.1:'"$RPC_PORT"' -X POST -H "Content-Type: application/json" \
      -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\",\"params\":[]}" 2>/dev/null || echo "FAIL")
    echo "  Health: $HEALTH"

    # Start custody + faucet
    sudo systemctl start lichen-custody
    sudo systemctl start lichen-faucet
    echo "  Services started"
  '
done
phase_done

# ============================================================================
# Phase 8: Verify everything
# ============================================================================
phase "Verify all nodes"

ALL_GOOD=true
for VPS in "${ALL_VPSES[@]}"; do
  echo ""
  echo "  === $VPS ==="

  # Health
  HEALTH=$(ssh_run "$VPS" "curl -sf http://127.0.0.1:$RPC_PORT -X POST -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\",\"params\":[]}'" 2>/dev/null || echo "FAIL")
  if echo "$HEALTH" | grep -qi 'ok'; then
    echo -e "  ${GREEN}✓${NC} Health: OK"
  else
    echo -e "  ${RED}✗${NC} Health: $HEALTH"
    ALL_GOOD=false
  fi

  # Slot
  SLOT=$(ssh_run "$VPS" "curl -sf http://127.0.0.1:$RPC_PORT -X POST -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSlot\",\"params\":[]}' | python3 -c 'import sys,json; print(json.load(sys.stdin)[\"result\"])'" 2>/dev/null || echo "?")
  echo "  Slot: $SLOT"

  # Treasury
  AIRDROP=$(ssh_run "$VPS" "curl -sf http://127.0.0.1:$RPC_PORT -X POST -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"requestAirdrop\",\"params\":[\"11111111111111111111111111111111\", 1000000000]}'" 2>/dev/null || echo "FAIL")
  if echo "$AIRDROP" | grep -q "Treasury keypair not configured"; then
    echo -e "  ${RED}✗${NC} Treasury: NOT CONFIGURED"
    ALL_GOOD=false
  else
    echo -e "  ${GREEN}✓${NC} Treasury: loaded"
  fi

  # Manifest
  SYMBOLS=$(ssh_run "$VPS" "curl -sf http://127.0.0.1:$RPC_PORT -X POST -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSignedMetadataManifest\",\"params\":[]}' | python3 -c 'import sys,json; d=json.load(sys.stdin); p=json.loads(d[\"result\"][\"payload\"]); print(len(p.get(\"symbol_registry\",[])))'" 2>/dev/null || echo "?")
  if [ "$SYMBOLS" = "28" ]; then
    echo -e "  ${GREEN}✓${NC} Manifest: $SYMBOLS symbols"
  elif [ "$SYMBOLS" = "?" ]; then
    echo -e "  ${YELLOW}?${NC} Manifest: could not read"
  else
    echo -e "  ${YELLOW}⚠${NC} Manifest: $SYMBOLS symbols (expected 28)"
  fi
done

# Verify via Cloudflare (external)
echo ""
echo "  === Cloudflare (testnet-rpc.lichen.network) ==="
CF_HEALTH=$(curl -sf https://testnet-rpc.lichen.network -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' 2>/dev/null || echo "FAIL")
if echo "$CF_HEALTH" | grep -qi 'ok'; then
  echo -e "  ${GREEN}✓${NC} Cloudflare health: OK"
else
  echo -e "  ${RED}✗${NC} Cloudflare health: $CF_HEALTH"
  ALL_GOOD=false
fi

CF_AIRDROP=$(curl -sf https://testnet-rpc.lichen.network -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"requestAirdrop","params":["11111111111111111111111111111111", 1000000000]}' 2>/dev/null || echo "FAIL")
if echo "$CF_AIRDROP" | grep -q "Treasury keypair not configured"; then
  echo -e "  ${RED}✗${NC} Cloudflare treasury: NOT CONFIGURED"
  ALL_GOOD=false
else
  echo -e "  ${GREEN}✓${NC} Cloudflare treasury: OK"
fi

phase_done

# ============================================================================
# Summary
# ============================================================================
TOTAL_ELAPSED=$(( $(date +%s) - TOTAL_START ))
MINS=$((TOTAL_ELAPSED / 60))
SECS=$((TOTAL_ELAPSED % 60))

echo ""
echo -e "${BOLD}${CYAN}══════════════════════════════════════════════════════${NC}"
if $ALL_GOOD; then
  echo -e "${BOLD}${GREEN}  ✓ CLEAN-SLATE REDEPLOY COMPLETE (${MINS}m${SECS}s)${NC}"
  echo -e "${GREEN}    All $((${#ALL_VPSES[@]})) nodes healthy, treasury loaded, manifest served${NC}"
else
  echo -e "${BOLD}${RED}  ✗ REDEPLOY COMPLETED WITH ISSUES (${MINS}m${SECS}s)${NC}"
  echo -e "${RED}    Check output above for failures${NC}"
fi
echo -e "${BOLD}${CYAN}══════════════════════════════════════════════════════${NC}"
