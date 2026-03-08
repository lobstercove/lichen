#!/bin/bash
# MoltChain Seed Node Setup
# Deploy and configure a seed node for network bootstrapping

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
PURPLE='\033[0;35m'
NC='\033[0m'

# Default values
SEED_HOME="$HOME/.moltchain-seed"
NETWORK="testnet"
DOMAIN=""
PUBLIC_IP=""
P2P_PORT=7001
RPC_PORT=8899
ENABLE_RPC_PUBLIC=false
INSTALL_SERVICE=false
ENABLE_MONITORING=true

print_header() {
    echo -e "${PURPLE}"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🦞 MoltChain Seed Node Setup"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo -e "${NC}"
}

print_info() { echo -e "${BLUE}ℹ${NC} $1"; }
print_success() { echo -e "${GREEN}✓${NC} $1"; }
print_warning() { echo -e "${YELLOW}⚠${NC} $1"; }
print_error() { echo -e "${RED}✗${NC} $1"; }
print_step() {
    echo ""
    echo -e "${PURPLE}═══${NC} $1 ${PURPLE}═══${NC}"
}

usage() {
    cat <<EOF
🦞 MoltChain Seed Node Setup

USAGE:
    $0 [OPTIONS]

OPTIONS:
    --network <testnet|mainnet>    Network (default: testnet)
    --home <PATH>                  Seed node home directory (default: ~/.moltchain-seed)
    --domain <DOMAIN>              Public domain name (e.g., seed1.moltchain.io)
    --public-ip <IP>               Public IP address (auto-detected if not provided)
    --p2p-port <PORT>              P2P port (default: 7001)
    --rpc-port <PORT>              RPC port (default: 8899)
    --enable-public-rpc            Enable public RPC access
    --install-service              Install systemd service
    --no-monitoring                Disable monitoring
    --help                         Show this help

EXAMPLES:
    # Basic seed node
    $0 --network testnet --domain seed1.testnet.moltchain.io

    # Production seed with monitoring
    $0 --network mainnet --domain seed1.moltchain.io --enable-public-rpc --install-service

EOF
    exit 0
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --network) NETWORK="$2"; shift 2 ;;
        --home) SEED_HOME="$2"; shift 2 ;;
        --domain) DOMAIN="$2"; shift 2 ;;
        --public-ip) PUBLIC_IP="$2"; shift 2 ;;
        --p2p-port) P2P_PORT="$2"; shift 2 ;;
        --rpc-port) RPC_PORT="$2"; shift 2 ;;
        --enable-public-rpc) ENABLE_RPC_PUBLIC=true; shift ;;
        --install-service) INSTALL_SERVICE=true; shift ;;
        --no-monitoring) ENABLE_MONITORING=false; shift ;;
        --help) usage ;;
        *) print_error "Unknown option: $1"; usage ;;
    esac
done

# Auto-detect public IP if not provided
if [ -z "$PUBLIC_IP" ]; then
    print_info "Detecting public IP..."
    PUBLIC_IP=$(curl -s https://api.ipify.org 2>/dev/null || curl -s https://ifconfig.me 2>/dev/null || echo "unknown")
    if [ "$PUBLIC_IP" = "unknown" ]; then
        print_warning "Could not auto-detect public IP"
    else
        print_success "Detected public IP: $PUBLIC_IP"
    fi
fi

print_header
echo ""
print_info "Network: ${NETWORK}"
print_info "Domain: ${DOMAIN:-not set}"
print_info "Public IP: ${PUBLIC_IP}"
print_info "P2P port: ${P2P_PORT}"
print_info "RPC port: ${RPC_PORT}"
print_info "Public RPC: ${ENABLE_RPC_PUBLIC}"
echo ""

# ============================================================================
# STEP 1: Prerequisites check
# ============================================================================
print_step "Prerequisites check"

# Check if validator binary exists
if [ ! -f "$PROJECT_ROOT/target/release/moltchain-validator" ]; then
    print_error "Validator binary not found. Run: cargo build --release"
    exit 1
fi
print_success "Validator binary found"

# Check network connectivity
if ! nc -z 8.8.8.8 53 2>/dev/null; then
    print_warning "Limited network connectivity detected"
fi

# ============================================================================
# STEP 2: Setup directories
# ============================================================================
print_step "Setting up directories"

mkdir -p "$SEED_HOME"
mkdir -p "$SEED_HOME/data"
mkdir -p "$SEED_HOME/logs"
mkdir -p "$SEED_HOME/backups"

print_success "Directories created"

# ============================================================================
# STEP 3: Generate seed node identity
# ============================================================================
print_step "Generating seed node identity"

KEYPAIR_PATH="$SEED_HOME/seed-keypair.json"

if [ -f "$KEYPAIR_PATH" ]; then
    print_warning "Keypair already exists: $KEYPAIR_PATH"
else
    "$PROJECT_ROOT/target/release/molt" init --output "$KEYPAIR_PATH"
    chmod 600 "$KEYPAIR_PATH"
    print_success "Seed keypair generated"
fi

# Get public key
PUBKEY=$("$PROJECT_ROOT/target/release/molt" pubkey --keypair "$KEYPAIR_PATH" | grep "📍" | awk '{print $3}')
print_info "Seed node pubkey: $PUBKEY"

# ============================================================================
# STEP 4: Download/copy genesis and seeds configuration
# ============================================================================
print_step "Network configuration"

GENESIS_PATH="$SEED_HOME/genesis.json"
SEEDS_PATH="$SEED_HOME/seeds.json"

# Copy genesis from project or generate default
if [ -f "$PROJECT_ROOT/genesis.json" ]; then
    cp "$PROJECT_ROOT/genesis.json" "$GENESIS_PATH"
    print_success "Genesis configuration copied"
else
    "$PROJECT_ROOT/scripts/generate-genesis.sh" --network "$NETWORK" --output "$GENESIS_PATH"
    print_success "Genesis configuration generated"
fi

# Copy seeds configuration
if [ -f "$PROJECT_ROOT/seeds.json" ]; then
    cp "$PROJECT_ROOT/seeds.json" "$SEEDS_PATH"
    print_success "Seeds configuration copied"
else
    print_warning "Seeds configuration not found, will use embedded defaults"
fi

# ============================================================================
# STEP 5: Generate seed node configuration
# ============================================================================
print_step "Seed node configuration"

CONFIG_PATH="$SEED_HOME/config.toml"

RPC_BIND="127.0.0.1"
if [ "$ENABLE_RPC_PUBLIC" = true ]; then
    RPC_BIND="0.0.0.0"
fi

cat > "$CONFIG_PATH" <<EOF
# MoltChain Seed Node Configuration
# Generated: $(date)
# Network: ${NETWORK}

[validator]
keypair_path = "${KEYPAIR_PATH}"
data_dir = "${SEED_HOME}/data"
enable_validation = false  # Seed nodes don't validate

[network]
p2p_port = ${P2P_PORT}
rpc_port = ${RPC_PORT}
seed_nodes = []  # Seed nodes don't need other seeds
enable_p2p = true
gossip_interval = 5  # More frequent for seed nodes
cleanup_timeout = 300
max_connections = 500  # Higher for seed nodes

[consensus]
min_validator_stake = 75000000000
slot_duration_ms = 400
enable_slashing = true

[rpc]
enable_rpc = true
bind_address = "${RPC_BIND}"
enable_cors = true
max_connections = 2000  # Higher for public RPC

[logging]
level = "info"
log_to_file = true
log_file_path = "${SEED_HOME}/logs/seed.log"
log_format = "json"

[monitoring]
enable_metrics = ${ENABLE_MONITORING}
metrics_port = 9100
enable_health_check = true

[genesis]
genesis_path = "${GENESIS_PATH}"
chain_id = "moltchain-${NETWORK}-1"

[performance]
worker_threads = 0  # Auto-detect
optimize_block_production = false  # Not validating
tx_batch_size = 1000

[security]
check_firewall = true
require_encryption = false
rpc_rate_limit = 200  # Higher for seed nodes
EOF

print_success "Configuration written"

# ============================================================================
# STEP 6: Create systemd service (Linux)
# ============================================================================
if [ "$INSTALL_SERVICE" = true ] && [ "$(uname)" = "Linux" ]; then
    print_step "Installing systemd service"

    SERVICE_FILE="/etc/systemd/system/moltchain-seed.service"
    
    sudo tee "$SERVICE_FILE" > /dev/null <<EOF
[Unit]
Description=MoltChain Seed Node (${NETWORK})
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$USER
WorkingDirectory=$PROJECT_ROOT
ExecStart=$PROJECT_ROOT/target/release/moltchain-validator --genesis $GENESIS_PATH $P2P_PORT
Restart=always
RestartSec=5
StandardOutput=append:$SEED_HOME/logs/seed.log
StandardError=append:$SEED_HOME/logs/seed-error.log

# Security
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=$SEED_HOME

# Resource limits
LimitNOFILE=65536
LimitNPROC=4096

[Install]
WantedBy=multi-user.target
EOF

    sudo systemctl daemon-reload
    print_success "Systemd service installed"
fi

# ============================================================================
# STEP 7: Firewall configuration
# ============================================================================
print_step "Firewall configuration"

print_info "Required firewall rules:"
print_info "  P2P: Allow TCP ${P2P_PORT} from anywhere"
if [ "$ENABLE_RPC_PUBLIC" = true ]; then
    print_info "  RPC: Allow TCP ${RPC_PORT} from anywhere"
fi
print_info "  Metrics: Allow TCP 9100 (optional, for monitoring)"

if command -v ufw &> /dev/null; then
    print_info "Detected ufw firewall"
    read -p "Configure firewall now? (y/n) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        sudo ufw allow "${P2P_PORT}/tcp"
        if [ "$ENABLE_RPC_PUBLIC" = true ]; then
            sudo ufw allow "${RPC_PORT}/tcp"
        fi
        if [ "$ENABLE_MONITORING" = true ]; then
            sudo ufw allow 9100/tcp
        fi
        print_success "Firewall configured"
    fi
fi

# ============================================================================
# STEP 8: DNS/Registration information
# ============================================================================
print_step "Registration information"

if [ -n "$DOMAIN" ]; then
    echo ""
    print_info "📋 DNS Configuration"
    echo ""
    echo "  Add the following DNS records:"
    echo ""
    echo "  ${DOMAIN}     A     ${PUBLIC_IP}"
    if [ "$ENABLE_RPC_PUBLIC" = true ]; then
        echo "  rpc.${DOMAIN}  A     ${PUBLIC_IP}"
    fi
    echo ""
fi

echo ""
print_info "📋 Seed Node Registration"
echo ""
echo "  Register your seed node by adding to seeds.json:"
echo ""
echo "  {"
echo "    \"id\": \"${DOMAIN:-$PUBLIC_IP}\","
echo "    \"address\": \"${DOMAIN:-$PUBLIC_IP}:${P2P_PORT}\","
echo "    \"pubkey\": \"${PUBKEY}\","
echo "    \"region\": \"<your-region>\","
echo "    \"operator\": \"<your-name>\","
echo "    \"rpc\": \"https://${DOMAIN:-$PUBLIC_IP}:${RPC_PORT}\""
echo "  }"
echo ""

# ============================================================================
# Final output
# ============================================================================
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
print_success "🦞 Seed node setup complete!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

print_info "📋 Next Steps:"
echo ""
echo "1. Start the seed node:"
if [ "$INSTALL_SERVICE" = true ]; then
    echo "   sudo systemctl start moltchain-seed"
    echo "   sudo systemctl enable moltchain-seed"
else
    echo "   cd $PROJECT_ROOT"
    echo "   ./target/release/moltchain-validator --genesis $GENESIS_PATH $P2P_PORT"
fi
echo ""
echo "2. Check status:"
if [ "$INSTALL_SERVICE" = true ]; then
    echo "   sudo systemctl status moltchain-seed"
    echo "   sudo journalctl -u moltchain-seed -f"
else
    echo "   tail -f $SEED_HOME/logs/seed.log"
fi
echo ""
echo "3. Monitor connections:"
echo "   netstat -an | grep $P2P_PORT"
echo ""
if [ "$ENABLE_MONITORING" = true ]; then
    echo "4. View metrics:"
    echo "   curl http://localhost:9100/metrics"
    echo ""
fi

print_info "Seed node info:"
print_info "  Address: ${DOMAIN:-$PUBLIC_IP}:${P2P_PORT}"
print_info "  Pubkey: ${PUBKEY}"
if [ "$ENABLE_RPC_PUBLIC" = true ]; then
    print_info "  RPC: http://${DOMAIN:-$PUBLIC_IP}:${RPC_PORT}"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
print_success "🦞 Ready to serve the network! 🦞"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
