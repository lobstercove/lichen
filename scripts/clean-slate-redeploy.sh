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
#   LICHEN_RELEASE_TAG=v0.5.10 bash scripts/clean-slate-redeploy.sh testnet
#
# Prerequisites:
#   - SSH access to all VPSes (port 2222, user ubuntu, key-based auth)
#   - deploy/setup.sh already run on all VPSes (systemd, users, dirs exist)
#   - keypairs/release-signing-key.json present in repo
#   - Code committed and pushed to main
#   - Optional: LICHEN_RELEASE_TAG set to install exact signed GitHub release
#     runtime artifacts into /usr/local/bin after remote builds
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
SSH_OPTS="-p $SSH_PORT -o ConnectTimeout=10 -o ServerAliveInterval=5 -o ServerAliveCountMax=3 -o StrictHostKeyChecking=no -o BatchMode=yes -o LogLevel=ERROR"

VPS_DATA="/var/lib/lichen"
VPS_CONFIG="/etc/lichen"
STATE_DIR="state-${NETWORK}"
SERVICE="lichen-validator-${NETWORK}"
RELEASE_TAG="${LICHEN_RELEASE_TAG:-}"
RELEASE_REPO="${LICHEN_RELEASE_REPO:-lobstercove/lichen}"
RELEASE_ARTIFACT_DIR="${LICHEN_RELEASE_ARTIFACT_DIR:-/tmp/lichen-release-${NETWORK}-${RELEASE_TAG:-local}}"
RELEASE_ARCHIVE_NAMES=""
VPS_RELEASE_ARCHIVES=()

case $NETWORK in
  testnet)
    RPC_PORT=8899
    P2P_PORT=7001
    CUSTODY_SERVICE="lichen-custody"
    CUSTODY_DB_NAME="custody-db"
    CUSTODY_PORT=9105
    FAUCET_ENABLED=1
    CF_RPC_URL="https://testnet-rpc.lichen.network"
    CF_RPC_LABEL="testnet-rpc.lichen.network"
    ;;
  mainnet)
    RPC_PORT=9899
    P2P_PORT=8001
    CUSTODY_SERVICE="lichen-custody-mainnet"
    CUSTODY_DB_NAME="custody-db-mainnet"
    CUSTODY_PORT=9106
    FAUCET_ENABLED=0
    CF_RPC_URL="https://rpc.lichen.network"
    CF_RPC_LABEL="rpc.lichen.network"
    ;;
  *) echo "Usage: $0 [testnet|mainnet]"; exit 1 ;;
esac
CUSTODY_DB_PATH="$VPS_DATA/$CUSTODY_DB_NAME"
VPS_CONFIRMATION_LIST="${ALL_VPSES[*]}"
VPS_CONFIRMATION_LIST="${VPS_CONFIRMATION_LIST// /,}"
REDEPLOY_CONFIRMATION="clean-slate:${NETWORK}:${VPS_CONFIRMATION_LIST}"
FAUCET_AIRDROPS_FILE="$VPS_DATA/airdrops.json"

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

release_archive_for_uname() {
  case "$1" in
    x86_64|amd64) echo "lichen-validator-linux-x86_64.tar.gz" ;;
    aarch64|arm64) echo "lichen-validator-linux-aarch64.tar.gz" ;;
    *)
      echo -e "${RED}Unsupported VPS architecture for release artifact: $1${NC}" >&2
      return 1
      ;;
  esac
}

release_archive_root() {
  local archive=$1
  echo "${archive%.tar.gz}"
}

append_unique_release_archive() {
  local archive=$1
  case " $RELEASE_ARCHIVE_NAMES " in
    *" $archive "*) return 0 ;;
  esac
  RELEASE_ARCHIVE_NAMES="${RELEASE_ARCHIVE_NAMES}${RELEASE_ARCHIVE_NAMES:+ }$archive"
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

prepare_release_artifacts() {
  if [ -z "$RELEASE_TAG" ]; then
    echo "No LICHEN_RELEASE_TAG set; VPSes will install locally built runtime binaries."
    return 0
  fi

  if ! command -v gh >/dev/null 2>&1; then
    echo -e "${RED}LICHEN_RELEASE_TAG requires GitHub CLI 'gh' to download release artifacts.${NC}"
    exit 1
  fi

  echo "Preparing release artifacts from ${RELEASE_REPO}@${RELEASE_TAG}..."
  rm -rf "$RELEASE_ARTIFACT_DIR"
  mkdir -p "$RELEASE_ARTIFACT_DIR"

  gh release download "$RELEASE_TAG" --repo "$RELEASE_REPO" \
    -p SHA256SUMS \
    -D "$RELEASE_ARTIFACT_DIR" >/dev/null

  local archive expected actual root
  for archive in $RELEASE_ARCHIVE_NAMES; do
    gh release download "$RELEASE_TAG" --repo "$RELEASE_REPO" \
      -p "$archive" \
      -D "$RELEASE_ARTIFACT_DIR" >/dev/null

    expected=$(awk -v file="$archive" '$2 == file { print $1 }' "$RELEASE_ARTIFACT_DIR/SHA256SUMS")
    if [ -z "$expected" ]; then
      echo -e "${RED}Release checksum missing for $archive in SHA256SUMS${NC}"
      exit 1
    fi

    actual=$(sha256_file "$RELEASE_ARTIFACT_DIR/$archive")
    if [ "$actual" != "$expected" ]; then
      echo -e "${RED}Checksum mismatch for $archive${NC}"
      echo "  expected: $expected"
      echo "  actual:   $actual"
      exit 1
    fi

    root=$(release_archive_root "$archive")
    tar xzf "$RELEASE_ARTIFACT_DIR/$archive" -C "$RELEASE_ARTIFACT_DIR"
    echo -e "  ${GREEN}✓${NC} $archive verified ($actual)"
    echo "    validator binary sha: $(sha256_file "$RELEASE_ARTIFACT_DIR/$root/lichen-validator")"
  done
}

install_release_runtime_binaries() {
  local host=$1 archive=$2 root archive_path extract_dir expected_validator_sha remote_validator_sha
  root=$(release_archive_root "$archive")
  archive_path="$RELEASE_ARTIFACT_DIR/$archive"
  extract_dir="$RELEASE_ARTIFACT_DIR/$root"
  expected_validator_sha=$(sha256_file "$extract_dir/lichen-validator")

  rsync -az \
    -e "ssh -p $SSH_PORT -o StrictHostKeyChecking=no -o LogLevel=ERROR" \
    "$archive_path" "$SSH_USER@$host:/tmp/$archive"

  ssh_run "$host" "
    set -euo pipefail
    tmp=\$(mktemp -d)
    tar xzf /tmp/$archive -C \"\$tmp\"
    sudo install -m 755 \"\$tmp/$root/lichen-validator\" /usr/local/bin/lichen-validator
    sudo install -m 755 \"\$tmp/$root/lichen-genesis\" /usr/local/bin/lichen-genesis
    sudo install -m 755 \"\$tmp/$root/lichen\" /usr/local/bin/lichen
    sudo install -m 755 \"\$tmp/$root/zk-prove\" /usr/local/bin/zk-prove
    install -m 644 \"\$tmp/$root/seeds.json\" ~/lichen/seeds.json
    rm -rf \"\$tmp\" /tmp/$archive
  " >/dev/null

  remote_validator_sha=$(ssh_run "$host" "sha256sum /usr/local/bin/lichen-validator | awk '{print \$1}'")
  if [ "$remote_validator_sha" != "$expected_validator_sha" ]; then
    echo -e "${RED}Release validator binary mismatch on $host${NC}"
    echo "  expected: $expected_validator_sha"
    echo "  actual:   $remote_validator_sha"
    exit 1
  fi

  echo -e "  ${GREEN}✓${NC} $host release validator sha: $remote_validator_sha"
}

install_built_runtime_binaries() {
  local host=$1
  ssh_run "$host" "
    set -euo pipefail
    cd ~/lichen
    for bin in lichen-validator lichen-genesis lichen zk-prove; do
      if [ -f \"target/release/\$bin\" ]; then
        sudo install -m 755 \"target/release/\$bin\" \"/usr/local/bin/\$bin\"
      fi
    done
    sha256sum /usr/local/bin/lichen-validator | awk '{print \"  validator sha: \" \$1}'
  "
}

install_service_binaries() {
  local host=$1
  ssh_run "$host" "
    set -euo pipefail
    cd ~/lichen
    for bin in lichen-custody lichen-faucet; do
      if [ -f \"target/release/\$bin\" ]; then
        sudo install -m 755 \"target/release/\$bin\" \"/usr/local/bin/\$bin\"
      fi
    done
  " >/dev/null
}

require_redeploy_confirmation() {
  if [ "${LICHEN_CLEAN_SLATE_REDEPLOY_CONFIRM:-}" = "$REDEPLOY_CONFIRMATION" ]; then
    return 0
  fi

  echo -e "${RED}Refusing clean-slate redeploy without explicit confirmation.${NC}"
  echo "This stops services and deletes $VPS_DATA/$STATE_DIR, $VPS_DATA/.lichen, and $CUSTODY_DB_PATH"
  if [ "$FAUCET_ENABLED" = "1" ]; then
    echo "plus testnet faucet history at $FAUCET_AIRDROPS_FILE"
  fi
  echo "on:"
  printf '  - %s\n' "${ALL_VPSES[@]}"
  echo ""
  echo "To continue, set:"
  echo "  export LICHEN_CLEAN_SLATE_REDEPLOY_CONFIRM='$REDEPLOY_CONFIRMATION'"
  exit 1
}

# ── Preflight checks ──
echo -e "${BOLD}Lichen Clean-Slate Redeploy ($NETWORK)${NC}"
echo ""

require_redeploy_confirmation

if [ ! -f keypairs/release-signing-key.json ]; then
  echo -e "${RED}Missing keypairs/release-signing-key.json${NC}"
  exit 1
fi

echo "Verifying SSH access..."
VPS_INDEX=0
for VPS in "${ALL_VPSES[@]}"; do
  ARCH=$(ssh_run "$VPS" "uname -m" | tail -1) || { echo "Cannot SSH to $VPS"; exit 1; }
  ARCHIVE=$(release_archive_for_uname "$ARCH")
  VPS_RELEASE_ARCHIVES[$VPS_INDEX]="$ARCHIVE"
  append_unique_release_archive "$ARCHIVE"
  echo -e "  ${GREEN}✓${NC} $VPS ($ARCH -> $ARCHIVE)"
  VPS_INDEX=$((VPS_INDEX + 1))
done

prepare_release_artifacts

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
    sudo systemctl reset-failed $SERVICE 2>/dev/null || true
    if sudo pgrep -f '/usr/local/bin/[l]ichen-validator .*state-${NETWORK}' >/dev/null 2>&1; then
      sudo pkill -TERM -f '/usr/local/bin/[l]ichen-validator .*state-${NETWORK}' 2>/dev/null || true
      sleep 1
      sudo pkill -KILL -f '/usr/local/bin/[l]ichen-validator .*state-${NETWORK}' 2>/dev/null || true
    fi
    # Ensure RPC port is open between VPSes (needed for genesis sync)
    sudo ufw allow ${RPC_PORT}/tcp comment 'Lichen RPC' 2>/dev/null || true
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
    sudo rm -rf $CUSTODY_DB_PATH
    if [ "$FAUCET_ENABLED" = "1" ]; then
      sudo rm -f $FAUCET_AIRDROPS_FILE
    fi
    sudo rm -f $VPS_CONFIG/signed-metadata-manifest-${NETWORK}.json
    sudo rm -f $VPS_CONFIG/custody-treasury-${NETWORK}.json
    sudo rm -f $VPS_DATA/faucet-keypair-${NETWORK}.json
    sudo mkdir -p $VPS_DATA/$STATE_DIR
    sudo chown lichen:lichen $VPS_DATA/$STATE_DIR
    sudo mkdir -p $CUSTODY_DB_PATH
    sudo chown lichen:lichen $CUSTODY_DB_PATH
  "
done
phase_done

# ============================================================================
# Phase 2b: Pin external P2P address in node env
# ============================================================================
phase "Pin external P2P addresses"
for VPS in "${ALL_VPSES[@]}"; do
  echo "  Configuring external address on $VPS..."
  REMOTE_IP=$(ssh_run "$VPS" "hostname -I 2>/dev/null | awk '{print \$1}'" | tail -1)
  if [ -z "$REMOTE_IP" ]; then
    echo -e "${RED}Failed to detect primary IP on $VPS${NC}"
    exit 1
  fi
  ssh_run "$VPS" "
    set -euo pipefail
    ENV_FILE=$VPS_CONFIG/env-$NETWORK
    sudo sed -i '/^LICHEN_EXTERNAL_ADDR=/d' \"\$ENV_FILE\"
    printf 'LICHEN_EXTERNAL_ADDR=%s\n' '$REMOTE_IP:$P2P_PORT' | sudo tee -a \"\$ENV_FILE\" >/dev/null
  "
  echo -e "  ${GREEN}✓${NC} $VPS external P2P: $REMOTE_IP:$P2P_PORT"
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

echo "  Installing runtime binaries into /usr/local/bin..."
VPS_INDEX=0
for VPS in "${ALL_VPSES[@]}"; do
  echo "    → $VPS..."
  if [ -n "$RELEASE_TAG" ]; then
    install_release_runtime_binaries "$VPS" "${VPS_RELEASE_ARCHIVES[$VPS_INDEX]}"
  else
    install_built_runtime_binaries "$VPS"
  fi
  install_service_binaries "$VPS"
  VPS_INDEX=$((VPS_INDEX + 1))
done
echo "  Runtime binaries installed"

# Distribute WASM from genesis VPS to joining VPSes
# (rsync --exclude target/ means joiners have stale/no WASM; genesis built fresh)
echo "  Distributing WASM contracts from genesis to joiners..."
for VPS in "${JOINING_VPSES[@]}"; do
  echo "    → $VPS..."
  ssh_pipe "$GENESIS_VPS" "$VPS" \
    "cd ~/lichen && tar czf - contracts/*/target/wasm32-unknown-unknown/release/*.wasm 2>/dev/null" \
    "cd ~/lichen && tar xzf -"
done
echo "  WASM contracts synchronized"
phase_done

# ============================================================================
# Phase 4: Prepare validator identities
# ============================================================================
phase "Prepare validator identities"

VALIDATOR_PUBKEYS=()
for VPS in "${ALL_VPSES[@]}"; do
  echo "  Preparing validator keypair on $VPS..."
  PUBKEY=$(ssh_run "$VPS" "
    set -euo pipefail
    STATE=$VPS_DATA/$STATE_DIR
    KP_PASS=\$(sudo grep LICHEN_KEYPAIR_PASSWORD /etc/lichen/env-$NETWORK | cut -d= -f2-)
    sudo mkdir -p \"\$STATE\"
    sudo chown lichen:lichen \"\$STATE\"
    if [ ! -f \"\$STATE/validator-keypair.json\" ]; then
      sudo -u lichen env LICHEN_KEYPAIR_PASSWORD=\"\$KP_PASS\" \
        /usr/local/bin/lichen init --output \"\$STATE/validator-keypair.json\" >/dev/null
    fi
    sudo python3 -c \"import json,sys; print(json.load(open(sys.argv[1]))['publicKeyBase58'])\" \"\$STATE/validator-keypair.json\"
  " | tail -1)
  if [ -z "$PUBKEY" ]; then
    echo -e "${RED}Failed to prepare validator pubkey on $VPS${NC}"
    exit 1
  fi
  VALIDATOR_PUBKEYS+=("$PUBKEY")
  echo -e "  ${GREEN}✓${NC} $VPS validator: $PUBKEY"
done

UNIQUE_VALIDATOR_COUNT=$(printf '%s\n' "${VALIDATOR_PUBKEYS[@]}" | sort -u | wc -l | tr -d ' ')
if [ "$UNIQUE_VALIDATOR_COUNT" -ne "${#VALIDATOR_PUBKEYS[@]}" ]; then
  echo -e "${RED}Validator pubkeys are not unique; refusing to create bridge/oracle genesis committee${NC}"
  printf '  %s\n' "${VALIDATOR_PUBKEYS[@]}"
  exit 1
fi

GENESIS_VALIDATOR_PUBKEY="${VALIDATOR_PUBKEYS[0]}"
GENESIS_OPERATOR_FLAGS=""
for PUBKEY in "${VALIDATOR_PUBKEYS[@]}"; do
  GENESIS_OPERATOR_FLAGS="$GENESIS_OPERATOR_FLAGS --bridge-validator $PUBKEY --oracle-operator $PUBKEY"
done

echo -e "  ${GREEN}✓${NC} Genesis consensus validator: $GENESIS_VALIDATOR_PUBKEY"
echo -e "  ${GREEN}✓${NC} Bridge/oracle committee size: ${#VALIDATOR_PUBKEYS[@]}"
phase_done

# ============================================================================
# Phase 5: Genesis on seed-01
# ============================================================================
phase "Create genesis on $GENESIS_VPS"

# Build the remote script with local variables expanded via heredoc
# Remote variables use \$ to prevent local expansion
GENESIS_SCRIPT=$(cat <<GENESIS_EOF
cd ~/lichen && source ~/.cargo/env
NET=$NETWORK
STATE=$VPS_DATA/$STATE_DIR

KP_PASS=\$(sudo grep LICHEN_KEYPAIR_PASSWORD /etc/lichen/env-$NETWORK | cut -d= -f2-)

PUBKEY=\$(sudo python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['publicKeyBase58'])" "\$STATE/validator-keypair.json")
echo "  Validator pubkey: \$PUBKEY"
if [ "\$PUBKEY" != "$GENESIS_VALIDATOR_PUBKEY" ]; then
  echo "FATAL: seed validator keypair does not match planned genesis validator"
  echo "  expected: $GENESIS_VALIDATOR_PUBKEY"
  echo "  actual:   \$PUBKEY"
  exit 1
fi

# 2. Prepare wallet
echo "  Preparing wallet..."
sudo -u lichen env HOME=$VPS_DATA LICHEN_HOME=$VPS_DATA \
  LICHEN_CONTRACTS_DIR=\$HOME/lichen/contracts \
  LICHEN_KEYPAIR_PASSWORD="\$KP_PASS" \
  /usr/local/bin/lichen-genesis --prepare-wallet --network "\$NET" --output-dir "\$STATE"

# 3. Fetch live prices
echo "  Fetching prices..."
SOL=145.0; ETH=2600.0; BNB=620.0
PRICE_JSON=\$(curl -sf 'https://api.binance.com/api/v3/ticker/price?symbols=["SOLUSDT","ETHUSDT","BNBUSDT"]' 2>/dev/null || echo '[]')
if [ "\$PRICE_JSON" != "[]" ] && command -v python3 &>/dev/null; then
  eval "\$(python3 -c "
import json
try:
    data = json.loads('\$PRICE_JSON')
    m = {d['symbol']: float(d['price']) for d in data}
    print(f'SOL={m.get(\"SOLUSDT\", 145.0):.2f}')
    print(f'ETH={m.get(\"ETHUSDT\", 2600.0):.2f}')
    print(f'BNB={m.get(\"BNBUSDT\", 620.0):.2f}')
except: pass
" 2>/dev/null)" || true
fi
echo "  Prices: SOL=\$SOL ETH=\$ETH BNB=\$BNB"

# 4. Create genesis
echo "  Creating genesis block..."
sudo -u lichen env HOME=$VPS_DATA LICHEN_HOME=$VPS_DATA \
  LICHEN_CONTRACTS_DIR=\$HOME/lichen/contracts \
  LICHEN_KEYPAIR_PASSWORD="\$KP_PASS" \
  GENESIS_SOL_USD="\$SOL" GENESIS_ETH_USD="\$ETH" GENESIS_BNB_USD="\$BNB" \
  /usr/local/bin/lichen-genesis \
    --network "\$NET" \
    --db-path "\$STATE" \
    --wallet-file "\$STATE/genesis-wallet.json" \
    --initial-validator "\$PUBKEY" \
    $GENESIS_OPERATOR_FLAGS
echo "  Genesis created!"

# 5. Install seeds.json
sudo install -m 644 -o lichen -g lichen ~/lichen/seeds.json "\$STATE/seeds.json"

# 6. Start genesis validator
echo "  Starting genesis validator..."
sudo systemctl start $SERVICE
sleep 8

# 7. Verify block production
SLOT=\$(curl -sf http://127.0.0.1:$RPC_PORT -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}' \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['result'])" 2>/dev/null || echo "FAIL")
echo "  Slot: \$SLOT"
if [ "\$SLOT" = "FAIL" ] || [ "\$SLOT" -lt 1 ] 2>/dev/null; then
  echo "FATAL: Genesis validator not producing blocks!"
  exit 1
fi
GENESIS_EOF
)

ssh_run "$GENESIS_VPS" "$GENESIS_SCRIPT"
phase_done

# ============================================================================
# Phase 6: Post-genesis + first-boot-deploy on seed-01
# ============================================================================
phase "Post-genesis on $GENESIS_VPS"

POSTGENESIS_SCRIPT=$(cat <<POSTGENESIS_EOF
cd ~/lichen && source ~/.cargo/env
KP_PASS=\$(sudo grep LICHEN_KEYPAIR_PASSWORD /etc/lichen/env-$NETWORK | cut -d= -f2-)

# 1. Post-genesis keypair setup (copies treasury -> custody, faucet keypair)
echo "  Running vps-post-genesis..."
sudo bash scripts/vps-post-genesis.sh $NETWORK --no-restart 2>&1 | grep -E "✓|✗|⚠|genesis-keys" || true

# 2. Install release signing key
echo "  Installing release signing key..."
sudo install -m 640 -o root -g lichen \
  ~/lichen/keypairs/release-signing-key.json \
  /etc/lichen/secrets/release-signing-keypair-$NETWORK.json

# 3. Run first-boot-deploy (deploys 28 contracts, creates manifest)
echo "  Running first-boot-deploy..."
sudo cp /etc/lichen/secrets/release-signing-keypair-$NETWORK.json ~/release-signing-keypair-$NETWORK.json
sudo chown \$(whoami):\$(whoami) ~/release-signing-keypair-$NETWORK.json
chmod 600 ~/release-signing-keypair-$NETWORK.json

SIGNED_METADATA_KEYPAIR=\$HOME/release-signing-keypair-$NETWORK.json \
  DEPLOY_NETWORK=$NETWORK \
  LICHEN_KEYPAIR_PASSWORD="\$KP_PASS" \
  ./scripts/first-boot-deploy.sh --rpc http://127.0.0.1:$RPC_PORT --skip-build 2>&1 | tail -10

rm -f ~/release-signing-keypair-$NETWORK.json

# 4. Install signed metadata manifest
echo "  Installing signed metadata manifest..."
if [ -f ~/lichen/signed-metadata-manifest-$NETWORK.json ]; then
  sudo install -m 640 -o root -g lichen \
    ~/lichen/signed-metadata-manifest-$NETWORK.json \
    /etc/lichen/signed-metadata-manifest-$NETWORK.json
fi

# 5. Do not restart the validator just to pick up the manifest.
# RPC reads LICHEN_SIGNED_METADATA_MANIFEST_FILE on demand, so restarting here
# can interrupt an in-flight proposal and leave non-committed state in RocksDB.
sleep 1

# 6. Provision custody seeds
echo "  Provisioning custody seeds..."
sudo bash -c "openssl rand -hex 32 > /etc/lichen/secrets/custody-master-seed-$NETWORK.txt"
sudo bash -c "openssl rand -hex 32 > /etc/lichen/secrets/custody-deposit-seed-$NETWORK.txt"
sudo chown root:lichen /etc/lichen/secrets/custody-*-seed-$NETWORK.txt
sudo chmod 640 /etc/lichen/secrets/custody-*-seed-$NETWORK.txt

# 7. Start network-specific services
echo "  Starting $CUSTODY_SERVICE..."
sudo systemctl start $CUSTODY_SERVICE
if [ "$FAUCET_ENABLED" = "1" ]; then
  sudo systemctl start lichen-faucet
fi
sleep 3

# 8. Quick verify
echo "  Verifying genesis VPS..."
curl -sf http://127.0.0.1:$RPC_PORT -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' || echo "HEALTH FAIL"
echo ""

AIRDROP=\$(curl -sf http://127.0.0.1:$RPC_PORT -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"requestAirdrop","params":["11111111111111111111111111111111", 1000000000]}' 2>/dev/null || echo "FAIL")
if echo "\$AIRDROP" | grep -q "Treasury keypair not configured"; then
  echo "  FATAL: Treasury NOT loaded on genesis VPS!"
  exit 1
fi
echo "  Treasury: OK"
echo "  Genesis VPS fully operational!"
POSTGENESIS_EOF
)

ssh_run "$GENESIS_VPS" "$POSTGENESIS_SCRIPT"
phase_done

# ============================================================================
# Phase 7: Snapshot state + distribute secrets to joining VPSes
# ============================================================================
phase "Snapshot genesis state + distribute to joiners"

# Stop genesis validator for a clean RocksDB snapshot (brief downtime)
echo "  Stopping genesis validator for clean snapshot..."
ssh_run "$GENESIS_VPS" "sudo systemctl stop $SERVICE"

# Create comprehensive tarball: state snapshot + all secrets
# Excludes node-specific files (keypairs, LOCK, IDENTITY, LOG*)
echo "  Creating state + secrets bundle on $GENESIS_VPS..."
ssh_run "$GENESIS_VPS" "
  cd /
  FAUCET_BUNDLE_ARGS=''
  if [ '$FAUCET_ENABLED' = '1' ]; then
    FAUCET_BUNDLE_ARGS='var/lib/lichen/faucet-keypair-${NETWORK}.json'
  fi
  sudo tar czf /tmp/lichen-state-bundle.tar.gz \
    --exclude='validator-keypair.json' \
    --exclude='signer-keypair.json' \
    --exclude='LOCK' \
    --exclude='IDENTITY' \
    --exclude='LOG' \
    --exclude='LOG.old.*' \
    --exclude='known-peers.json' \
    --exclude='logs' \
    var/lib/lichen/$STATE_DIR/ \
    \$FAUCET_BUNDLE_ARGS \
    etc/lichen/secrets/custody-master-seed-${NETWORK}.txt \
    etc/lichen/secrets/custody-deposit-seed-${NETWORK}.txt \
    etc/lichen/signed-metadata-manifest-${NETWORK}.json \
    etc/lichen/custody-treasury-${NETWORK}.json \
    etc/lichen/secrets/release-signing-keypair-${NETWORK}.json \
    2>/dev/null
  sudo chmod 644 /tmp/lichen-state-bundle.tar.gz
  echo \"  Bundle size: \$(du -h /tmp/lichen-state-bundle.tar.gz | cut -f1)\"
"

# Restart genesis validator immediately
echo "  Restarting genesis validator..."
ssh_run "$GENESIS_VPS" "sudo systemctl start $SERVICE"
ssh_run "$GENESIS_VPS" "
  sudo systemctl start $CUSTODY_SERVICE
  if [ '$FAUCET_ENABLED' = '1' ]; then
    sudo systemctl start lichen-faucet
  fi
"

for VPS in "${JOINING_VPSES[@]}"; do
  echo "  Distributing state + secrets to $VPS..."

  # Single pipe: genesis → tarball → joining VPS → cleanup old RocksDB, extract, fix perms
  ssh_pipe "$GENESIS_VPS" "$VPS" \
    "cat /tmp/lichen-state-bundle.tar.gz" \
    "sudo mkdir -p $VPS_DATA/$STATE_DIR/genesis-keys $VPS_CONFIG/secrets && \
     sudo rm -f $VPS_DATA/$STATE_DIR/*.sst $VPS_DATA/$STATE_DIR/CURRENT $VPS_DATA/$STATE_DIR/MANIFEST-* $VPS_DATA/$STATE_DIR/OPTIONS-* $VPS_DATA/$STATE_DIR/consensus.wal 2>/dev/null; \
     sudo tar xzf - -C / && \
     sudo chown -R lichen:lichen $VPS_DATA/$STATE_DIR/ && \
     sudo chmod 640 $VPS_DATA/$STATE_DIR/genesis-wallet.json && \
     sudo find $VPS_DATA/$STATE_DIR/genesis-keys -type f -exec chmod 640 {} + && \
     if [ '$FAUCET_ENABLED' = '1' ]; then \
       sudo chown lichen:lichen $VPS_DATA/faucet-keypair-${NETWORK}.json && \
       sudo chmod 600 $VPS_DATA/faucet-keypair-${NETWORK}.json; \
     fi && \
     sudo chown root:lichen $VPS_CONFIG/secrets/custody-master-seed-${NETWORK}.txt && \
     sudo chmod 640 $VPS_CONFIG/secrets/custody-master-seed-${NETWORK}.txt && \
     sudo chown root:lichen $VPS_CONFIG/secrets/custody-deposit-seed-${NETWORK}.txt && \
     sudo chmod 640 $VPS_CONFIG/secrets/custody-deposit-seed-${NETWORK}.txt && \
     sudo chown root:lichen $VPS_CONFIG/signed-metadata-manifest-${NETWORK}.json && \
     sudo chmod 640 $VPS_CONFIG/signed-metadata-manifest-${NETWORK}.json && \
     sudo chown lichen:lichen $VPS_CONFIG/custody-treasury-${NETWORK}.json && \
     sudo chmod 600 $VPS_CONFIG/custody-treasury-${NETWORK}.json && \
     sudo chown root:lichen $VPS_CONFIG/secrets/release-signing-keypair-${NETWORK}.json && \
     sudo chmod 640 $VPS_CONFIG/secrets/release-signing-keypair-${NETWORK}.json"

  # Verify
  COUNT=$(ssh_run "$VPS" "sudo ls $VPS_DATA/$STATE_DIR/genesis-keys/ 2>/dev/null | wc -l")
  WALLET=$(ssh_run "$VPS" "sudo test -f $VPS_DATA/$STATE_DIR/genesis-wallet.json && echo YES || echo NO")
  SST=$(ssh_run "$VPS" "ls $VPS_DATA/$STATE_DIR/*.sst 2>/dev/null | wc -l")
  echo -e "  ${GREEN}✓${NC} $VPS: $COUNT genesis-keys, wallet=$WALLET, sst=$SST"
done

# Clean up bundle
ssh_run "$GENESIS_VPS" "sudo rm -f /tmp/lichen-state-bundle.tar.gz"
phase_done

# ============================================================================
# Phase 8: Start joining VPSes
# ============================================================================
phase "Start joining VPSes"

for VPS in "${JOINING_VPSES[@]}"; do
  echo "  Starting $VPS..."
  ssh_run "$VPS" '
    # Install seeds.json
    sudo install -m 644 -o lichen -g lichen ~/lichen/seeds.json '"$VPS_DATA/$STATE_DIR"'/seeds.json

    # Generate validator keypair if not present (auto-generated on first boot,
    # but we ensure it exists so the snapshot state is usable immediately)
    KP_PASS=$(sudo grep LICHEN_KEYPAIR_PASSWORD /etc/lichen/env-'"$NETWORK"' | cut -d= -f2-)
    if [ ! -f '"$VPS_DATA/$STATE_DIR"'/validator-keypair.json ]; then
      echo "  Generating validator keypair..."
      sudo -u lichen env LICHEN_KEYPAIR_PASSWORD="$KP_PASS" \
        /usr/local/bin/lichen init --output '"$VPS_DATA/$STATE_DIR"'/validator-keypair.json
    fi

    # Start validator
    sudo systemctl start '"$SERVICE"'

    # Wait for sync (up to 90s) — with state snapshot, sync should be near-instant
    echo "  Waiting for sync..."
    for i in $(seq 1 18); do
      sleep 5
      SLOT=$(curl -sf http://127.0.0.1:'"$RPC_PORT"' -X POST -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSlot\",\"params\":[]}" 2>/dev/null \
        | python3 -c "import sys,json; print(json.load(sys.stdin)['"'"'result'"'"'])" 2>/dev/null || echo "0")
      if [ "$SLOT" -gt 0 ] 2>/dev/null; then
        echo "  Synced to slot $SLOT"
        break
      fi
      echo "  Still syncing... (${i}/18)"
    done

    # Health check
    HEALTH=$(curl -sf http://127.0.0.1:'"$RPC_PORT"' -X POST -H "Content-Type: application/json" \
      -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\",\"params\":[]}" 2>/dev/null || echo "FAIL")
    echo "  Health: $HEALTH"

    # Start custody + faucet
    sudo systemctl start '"$CUSTODY_SERVICE"'
    if [ '"$FAUCET_ENABLED"' = "1" ]; then
      sudo systemctl start lichen-faucet
    fi
    echo "  Services started"
  '
done
phase_done

verify_protocol_url() {
  local url=$1
  python3 - "$url" <<'PY'
import json
import sys
import urllib.request

url = sys.argv[1]

def rpc(method):
    payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": []}).encode()
    request = urllib.request.Request(
        url,
        data=payload,
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json",
            "User-Agent": "lichen-clean-slate-redeploy/1.0",
        },
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        body = json.loads(response.read().decode())
    result = body.get("result")
    if not isinstance(result, dict):
        raise RuntimeError(f"{method} returned no result object")
    return result

bridge = rpc("getLichenBridgeStats")
oracle = rpc("getLichenOracleStats")
errors = []
validator_count = int(bridge.get("validator_count") or 0)
required_confirms = int(bridge.get("required_confirms") or 0)
if validator_count < 2:
    errors.append(f"bridge validator_count={validator_count} < 2")
if required_confirms < 2:
    errors.append(f"bridge required_confirms={required_confirms} < 2")
if validator_count < required_confirms:
    errors.append(f"bridge validator_count={validator_count} < required_confirms={required_confirms}")
if bridge.get("paused") is True or bridge.get("operational") is False:
    errors.append("bridge not operational")

contract_feeds = int(oracle.get("contract_feeds", oracle.get("feeds") or 0) or 0)
consensus_feeds = int(oracle.get("consensus_feeds") or 0)
if contract_feeds < 4:
    errors.append(f"oracle contract_feeds={contract_feeds} < 4")
if consensus_feeds < 4:
    errors.append(f"oracle consensus_feeds={consensus_feeds} < 4")
if oracle.get("paused") is True or oracle.get("operational") is False:
    errors.append("oracle not operational")

print(
    f"bridge validators={validator_count} required={required_confirms}; "
    f"oracle contract_feeds={contract_feeds} consensus_feeds={consensus_feeds}"
)
if errors:
    for error in errors:
        print(f"ERROR: {error}", file=sys.stderr)
    raise SystemExit(1)
PY
}

verify_remote_protocol() {
  local host=$1
  ssh_run "$host" "$(declare -f verify_protocol_url); verify_protocol_url http://127.0.0.1:$RPC_PORT"
}

# ============================================================================
# Phase 9: Verify everything
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

  if PROTOCOL_STATUS=$(verify_remote_protocol "$VPS" 2>&1); then
    echo -e "  ${GREEN}✓${NC} Protocol: $PROTOCOL_STATUS"
  else
    echo -e "  ${RED}✗${NC} Protocol bootstrap: ${PROTOCOL_STATUS:-FAILED}"
    ALL_GOOD=false
  fi

  # Custody on genesis VPS remains required after the snapshot restart.
  if [ "$VPS" = "$GENESIS_VPS" ]; then
    if ssh_run "$VPS" "curl -sf http://127.0.0.1:$CUSTODY_PORT/health" >/dev/null 2>&1; then
      echo -e "  ${GREEN}✓${NC} Custody: healthy"
    else
      echo -e "  ${RED}✗${NC} Custody: unavailable"
      ALL_GOOD=false
    fi
  fi

  if [ "$FAUCET_ENABLED" = "1" ]; then
    if ssh_run "$VPS" "curl -sf http://127.0.0.1:9100/health" >/dev/null 2>&1; then
      echo -e "  ${GREEN}✓${NC} Faucet: healthy"
    else
      echo -e "  ${RED}✗${NC} Faucet: unavailable"
      ALL_GOOD=false
    fi

    FAUCET_HISTORY_COUNT=$(ssh_run "$VPS" "curl -sf http://127.0.0.1:9100/faucet/airdrops?limit=1 | python3 -c 'import sys,json; print(len(json.load(sys.stdin)))'" 2>/dev/null || echo "?")
    if [ "$FAUCET_HISTORY_COUNT" = "0" ]; then
      echo -e "  ${GREEN}✓${NC} Faucet history: empty"
    else
      echo -e "  ${RED}✗${NC} Faucet history: $FAUCET_HISTORY_COUNT stale record(s)"
      ALL_GOOD=false
    fi
  fi

  # Manifest
  SYMBOLS=$(ssh_run "$VPS" "curl -sf http://127.0.0.1:$RPC_PORT -X POST -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSignedMetadataManifest\",\"params\":[]}' | python3 -c '
import sys,json
d=json.load(sys.stdin)
p=d.get(\"result\",{}).get(\"payload\",{})
if isinstance(p, str): p=json.loads(p)
print(len(p.get(\"symbol_registry\",[])))
'" 2>/dev/null || echo "?")
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
echo "  === Cloudflare (${CF_RPC_LABEL}) ==="
CF_HEALTH=$(curl -sf "$CF_RPC_URL" -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' 2>/dev/null || echo "FAIL")
if echo "$CF_HEALTH" | grep -qi 'ok'; then
  echo -e "  ${GREEN}✓${NC} Cloudflare health: OK"
else
  echo -e "  ${RED}✗${NC} Cloudflare health: $CF_HEALTH"
  ALL_GOOD=false
fi

CF_AIRDROP=$(curl -sf "$CF_RPC_URL" -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"requestAirdrop","params":["11111111111111111111111111111111", 1000000000]}' 2>/dev/null || echo "FAIL")
if echo "$CF_AIRDROP" | grep -q "Treasury keypair not configured"; then
  echo -e "  ${RED}✗${NC} Cloudflare treasury: NOT CONFIGURED"
  ALL_GOOD=false
else
  echo -e "  ${GREEN}✓${NC} Cloudflare treasury: OK"
fi

if CF_PROTOCOL_STATUS=$(verify_protocol_url "$CF_RPC_URL" 2>&1); then
  echo -e "  ${GREEN}✓${NC} Cloudflare protocol: $CF_PROTOCOL_STATUS"
else
  echo -e "  ${RED}✗${NC} Cloudflare protocol bootstrap: ${CF_PROTOCOL_STATUS:-FAILED}"
  ALL_GOOD=false
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
