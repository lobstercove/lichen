#!/bin/bash
# Lichen Validator Setup Script
# Production-ready validator initialization and configuration

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
LICHEN_HOME="$HOME/.lichen"
CONFIG_PATH=""
GENESIS_PATH=""
KEYPAIR_PATH=""
DATA_DIR=""
P2P_PORT=""
RPC_PORT=""
WS_PORT=""
AUTO_STAKE=false
STAKE_AMOUNT=1000000  # 1M LICN default
INSTALL_SERVICE=false
NETWORK="testnet"

print_header() {
    echo -e "${PURPLE}"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🦞 Lichen Validator Setup"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo -e "${NC}"
}

print_info() {
    echo -e "${BLUE}ℹ${NC} $1"
}

print_success() {
    echo -e "${GREEN}✓${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

print_step() {
    echo ""
    echo -e "${PURPLE}═══${NC} $1 ${PURPLE}═══${NC}"
}

usage() {
    cat <<EOF
🦞 Lichen Validator Setup

USAGE:
    $0 [OPTIONS]

OPTIONS:
    --network <testnet|mainnet>    Network to join (default: testnet)
    --home <PATH>                  Lichen home directory (default: ~/.lichen)
    --genesis <PATH>               Path to genesis.json file (required)
    --keypair <PATH>               Path to validator keypair (optional, will generate)
    --data-dir <PATH>              Data directory (default: ~/.lichen/data)
    --p2p-port <PORT>              P2P port (default: testnet=7001, mainnet=8001)
    --rpc-port <PORT>              RPC port (default: testnet=8899, mainnet=9899)
    --auto-stake                   Automatically stake minimum required amount
    --stake-amount <LICN>          Amount to stake in LICN (default: 1000000)
    --install-service              Install systemd service (Linux only)
    --help                         Show this help message

EXAMPLES:
    # Basic testnet setup
    $0 --network testnet --genesis ./genesis.json

    # Full production setup with service
    $0 --network mainnet --genesis ./genesis.json --install-service --auto-stake

    # Custom ports and directories
    $0 --genesis ./genesis.json --p2p-port 8001 --rpc-port 9001 --data-dir /mnt/lichen

EOF
    exit 0
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --network)
            NETWORK="$2"
            shift 2
            ;;
        --home)
            LICHEN_HOME="$2"
            shift 2
            ;;
        --genesis)
            GENESIS_PATH="$2"
            shift 2
            ;;
        --keypair)
            KEYPAIR_PATH="$2"
            shift 2
            ;;
        --data-dir)
            DATA_DIR="$2"
            shift 2
            ;;
        --p2p-port)
            P2P_PORT="$2"
            shift 2
            ;;
        --rpc-port)
            RPC_PORT="$2"
            shift 2
            ;;
        --auto-stake)
            AUTO_STAKE=true
            shift
            ;;
        --stake-amount)
            STAKE_AMOUNT="$2"
            shift 2
            ;;
        --install-service)
            INSTALL_SERVICE=true
            shift
            ;;
        --help)
            usage
            ;;
        *)
            print_error "Unknown option: $1"
            usage
            ;;
    esac
done

# Validate required parameters
if [ -z "$GENESIS_PATH" ]; then
    print_warning "No genesis file specified — validator will create genesis on first boot"
    print_info "Use --genesis <path> to join an existing network"
fi

if [ -n "$GENESIS_PATH" ] && [ ! -f "$GENESIS_PATH" ]; then
    print_error "Genesis file not found: $GENESIS_PATH"
    exit 1
fi

# Set network-aware port defaults if not provided
if [ -z "$P2P_PORT" ]; then
    case $NETWORK in
        testnet) P2P_PORT=7001 ;;
        mainnet) P2P_PORT=8001 ;;
    esac
fi
if [ -z "$RPC_PORT" ]; then
    case $NETWORK in
        testnet) RPC_PORT=8899 ;;
        mainnet) RPC_PORT=9899 ;;
    esac
fi
if [ -z "$WS_PORT" ]; then
    case $NETWORK in
        testnet) WS_PORT=8900 ;;
        mainnet) WS_PORT=9900 ;;
    esac
fi

# Set defaults if not provided
if [ -z "$DATA_DIR" ]; then
    DATA_DIR="$LICHEN_HOME/data"
fi

if [ -z "$KEYPAIR_PATH" ]; then
    KEYPAIR_PATH="$LICHEN_HOME/validator-keypair.json"
fi

CONFIG_PATH="$LICHEN_HOME/config.toml"

# Start setup
print_header
echo ""
print_error "This script is legacy and unvalidated for the current production deployment model."
print_info "Use deploy/setup.sh and docs/deployment/PRODUCTION_DEPLOYMENT.md for production validator setup."
exit 1

print_info "Network: ${NETWORK}"
print_info "Home directory: ${LICHEN_HOME}"
print_info "Data directory: ${DATA_DIR}"
print_info "P2P port: ${P2P_PORT}"
print_info "RPC port: ${RPC_PORT}"
echo ""

# ============================================================================
# STEP 1: Create directory structure
# ============================================================================
print_step "Creating directory structure"

mkdir -p "$LICHEN_HOME"
mkdir -p "$DATA_DIR"
mkdir -p "$LICHEN_HOME/logs"
mkdir -p "$LICHEN_HOME/backups"

print_success "Directories created"
print_info "  Home: $LICHEN_HOME"
print_info "  Data: $DATA_DIR"
print_info "  Logs: $LICHEN_HOME/logs"

# ============================================================================
# STEP 2: Generate or verify validator keypair
# ============================================================================
print_step "Validator keypair setup"

if [ -f "$KEYPAIR_PATH" ]; then
    print_warning "Keypair already exists: $KEYPAIR_PATH"
    print_info "Using existing keypair"
    
    # Verify it's a valid keypair
    if command -v jq &> /dev/null; then
        if ! jq empty "$KEYPAIR_PATH" 2>/dev/null; then
            print_error "Existing keypair file is invalid JSON"
            exit 1
        fi
    fi
else
    print_info "Generating new validator keypair..."
    
    # Generate keypair using licn CLI or create manually
    if [ -f "$PROJECT_ROOT/target/release/licn" ]; then
        "$PROJECT_ROOT/target/release/licn" init --output "$KEYPAIR_PATH"
        print_success "Keypair generated: $KEYPAIR_PATH"
    else
        # Fallback: create a placeholder that validator will initialize
        print_warning "lichen CLI not found, validator will generate keypair on first run"
        echo '{"note":"Keypair will be generated on first validator start"}' > "$KEYPAIR_PATH"
    fi
    
    # Set secure permissions
    chmod 600 "$KEYPAIR_PATH"
    print_success "Keypair permissions set to 600 (owner read/write only)"
fi

# Display public key if possible
if command -v licn &> /dev/null && [ -s "$KEYPAIR_PATH" ]; then
    PUBKEY=$(licn pubkey "$KEYPAIR_PATH" 2>/dev/null || echo "Unable to extract")
    print_info "Public key: ${PUBKEY}"
fi

# ============================================================================
# STEP 3: Copy and configure genesis file
# ============================================================================
print_step "Genesis configuration"

GENESIS_DEST="$LICHEN_HOME/genesis.json"
cp "$GENESIS_PATH" "$GENESIS_DEST"
print_success "Genesis copied to: $GENESIS_DEST"

# Verify genesis file
if command -v jq &> /dev/null; then
    CHAIN_ID=$(jq -r '.chain_id' "$GENESIS_DEST")
    TOTAL_SUPPLY=$(jq -r '.initial_accounts | map(.balance_licn) | add' "$GENESIS_DEST")
    VALIDATOR_COUNT=$(jq '.initial_validators | length' "$GENESIS_DEST")
    
    print_info "Chain ID: $CHAIN_ID"
    print_info "Total supply: ${TOTAL_SUPPLY} LICN"
    print_info "Genesis validators: $VALIDATOR_COUNT"
fi

# ============================================================================
# STEP 4: Generate configuration file
# ============================================================================
print_step "Configuration file"

cat > "$CONFIG_PATH" <<EOF
# Lichen Validator Configuration
# Generated: $(date)
# Network: ${NETWORK}

[validator]
keypair_path = "${KEYPAIR_PATH}"
data_dir = "${DATA_DIR}"
enable_validation = true

[network]
p2p_port = ${P2P_PORT}
rpc_port = ${RPC_PORT}
seed_nodes = []
enable_p2p = true
gossip_interval = 10
cleanup_timeout = 300

[consensus]
min_validator_stake = 75000000000
slot_duration_ms = 400
enable_slashing = true

[rpc]
enable_rpc = true
bind_address = "0.0.0.0"
enable_cors = true
max_connections = 1000

[logging]
level = "info"
log_to_file = true
log_file_path = "${LICHEN_HOME}/logs/validator.log"
log_format = "text"

[monitoring]
enable_metrics = true
metrics_port = 9100
enable_health_check = true

[genesis]
genesis_path = "${GENESIS_DEST}"
chain_id = "${CHAIN_ID:-lichen-${NETWORK}-1}"

[performance]
worker_threads = 0
optimize_block_production = true
tx_batch_size = 1000

[security]
check_firewall = true
require_encryption = false
rpc_rate_limit = 100
EOF

print_success "Configuration written to: $CONFIG_PATH"

# ============================================================================
# STEP 5: Install systemd service (Linux only)
# ============================================================================
if [ "$INSTALL_SERVICE" = true ]; then
    print_step "Installing systemd service"
    
    if [ "$(uname)" != "Linux" ]; then
        print_warning "Systemd service installation only supported on Linux"
    elif [ ! -d "/etc/systemd/system" ]; then
        print_warning "Systemd not detected, skipping service installation"
    else
        SERVICE_FILE="/etc/systemd/system/lichen-validator.service"
        
        sudo tee "$SERVICE_FILE" > /dev/null <<EOF
[Unit]
Description=Lichen Validator
After=network.target
Wants=network-online.target

[Service]
Type=simple
User=$USER
WorkingDirectory=$PROJECT_ROOT
ExecStart=$PROJECT_ROOT/target/release/lichen-validator --network $NETWORK --rpc-port $RPC_PORT --ws-port $WS_PORT --p2p-port $P2P_PORT --db-path $DATA_DIR
Restart=always
RestartSec=10
StandardOutput=append:$LICHEN_HOME/logs/validator.log
StandardError=append:$LICHEN_HOME/logs/validator-error.log

# Security
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=$LICHEN_HOME $DATA_DIR

# Resource limits
LimitNOFILE=65536
LimitNPROC=4096

[Install]
WantedBy=multi-user.target
EOF

        sudo systemctl daemon-reload
        print_success "Systemd service installed: $SERVICE_FILE"
        print_info "Enable with: sudo systemctl enable lichen-validator"
        print_info "Start with: sudo systemctl start lichen-validator"
        print_info "Status: sudo systemctl status lichen-validator"
        print_info "Logs: sudo journalctl -u lichen-validator -f"
    fi
fi

# ============================================================================
# STEP 6: Create helper scripts
# ============================================================================
print_step "Creating helper scripts"

# Start script
cat > "$LICHEN_HOME/start-validator.sh" <<EOF
#!/bin/bash
# Start Lichen Validator

cd "$PROJECT_ROOT"
exec ./target/release/lichen-validator --network $NETWORK --rpc-port $RPC_PORT --ws-port $WS_PORT --p2p-port $P2P_PORT --db-path "$DATA_DIR"
EOF
chmod +x "$LICHEN_HOME/start-validator.sh"
print_success "Start script: $LICHEN_HOME/start-validator.sh"

# Health check script
cat > "$LICHEN_HOME/health-check.sh" <<EOF
#!/bin/bash
# Lichen Validator Health Check

RPC_URL="http://localhost:${RPC_PORT}"

# Check if RPC is responding
if curl -sf -X POST \$RPC_URL \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}' > /dev/null; then
    echo "✓ Validator is healthy"
    exit 0
else
    echo "✗ Validator is not responding"
    exit 1
fi
EOF
chmod +x "$LICHEN_HOME/health-check.sh"
print_success "Health check: $LICHEN_HOME/health-check.sh"

# Backup script
cat > "$LICHEN_HOME/backup.sh" <<EOF
#!/bin/bash
# Backup Lichen validator data

BACKUP_DIR="$LICHEN_HOME/backups"
TIMESTAMP=\$(date +%Y%m%d-%H%M%S)
BACKUP_FILE="\$BACKUP_DIR/lichen-backup-\$TIMESTAMP.tar.gz"

echo "Creating backup..."
tar -czf "\$BACKUP_FILE" -C "$LICHEN_HOME" validator-keypair.json config.toml genesis.json
tar -czf "\$BACKUP_FILE.data" -C "$DATA_DIR" .

echo "✓ Backup created:"
echo "  Config: \$BACKUP_FILE"
echo "  Data: \$BACKUP_FILE.data"
EOF
chmod +x "$LICHEN_HOME/backup.sh"
print_success "Backup script: $LICHEN_HOME/backup.sh"

# ============================================================================
# STEP 7: Security check
# ============================================================================
print_step "Security verification"

# Check file permissions
PERMS=$(stat -f "%OLp" "$KEYPAIR_PATH" 2>/dev/null || stat -c "%a" "$KEYPAIR_PATH" 2>/dev/null)
if [ "$PERMS" = "600" ]; then
    print_success "Keypair permissions correct (600)"
else
    print_warning "Keypair permissions: $PERMS (should be 600)"
    chmod 600 "$KEYPAIR_PATH"
    print_success "Fixed keypair permissions"
fi

# Check firewall for required ports
if command -v ufw &> /dev/null; then
    print_info "Firewall detected (ufw)"
    print_warning "Ensure ports are open:"
    print_info "  P2P: $P2P_PORT"
    print_info "  RPC: $RPC_PORT"
    print_info "  Metrics: 9100"
fi

# ============================================================================
# FINAL OUTPUT
# ============================================================================
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
print_success "🦞 Validator setup complete!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

print_info "📋 Next Steps:"
echo ""
echo "1. Review your configuration:"
echo "   cat $CONFIG_PATH"
echo ""
echo "2. Start the validator:"
echo "   $LICHEN_HOME/start-validator.sh"
echo ""
echo "   Or use the production start script:"
echo "   cd $PROJECT_ROOT"
echo "   ./lichen-start.sh $NETWORK"
echo ""
echo "   Or run the binary directly:"
echo "   ./target/release/lichen-validator --network $NETWORK --rpc-port $RPC_PORT --ws-port $WS_PORT --p2p-port $P2P_PORT --db-path $DATA_DIR"
echo ""
echo "3. Check validator health:"
echo "   $LICHEN_HOME/health-check.sh"
echo ""
echo "4. View logs:"
echo "   tail -f $LICHEN_HOME/logs/validator.log"
echo ""

if [ "$INSTALL_SERVICE" = true ] && [ "$(uname)" = "Linux" ]; then
    echo "5. Manage systemd service:"
    echo "   sudo systemctl start lichen-validator"
    echo "   sudo systemctl status lichen-validator"
    echo "   sudo journalctl -u lichen-validator -f"
    echo ""
fi

if [ "$AUTO_STAKE" = true ]; then
    print_warning "Auto-staking not yet implemented"
    print_info "To stake manually, use the licn CLI after validator starts"
fi

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
print_success "🦞 Ready to grow! 🦞"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
