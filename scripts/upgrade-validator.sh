#!/bin/bash
# Lichen Validator Upgrade Script
# Safely upgrade validator to new version with rollback support

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

LICHEN_HOME="${LICHEN_HOME:-$HOME/.lichen}"
PROJECT_ROOT="/opt/lichen"
BACKUP_DIR="$LICHEN_HOME/backups"
ROLLBACK_VERSION=""

print_info() { echo -e "${BLUE}ℹ${NC} $1"; }
print_success() { echo -e "${GREEN}✓${NC} $1"; }
print_warning() { echo -e "${YELLOW}⚠${NC} $1"; }
print_error() { echo -e "${RED}✗${NC} $1"; }

echo "🦞 Lichen Validator Upgrade"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

print_error "This script is legacy and unvalidated for the current production deployment model."
print_info "Use deploy/setup.sh and docs/deployment/PRODUCTION_DEPLOYMENT.md for validator upgrades."
exit 1

# Check if validator is running
if systemctl is-active --quiet lichen-validator 2>/dev/null; then
    print_info "Validator is running, will stop for upgrade"
    VALIDATOR_RUNNING=true
else
    VALIDATOR_RUNNING=false
fi

# Create backup
print_info "Creating backup..."
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
BACKUP_FILE="$BACKUP_DIR/upgrade-backup-$TIMESTAMP.tar.gz"
mkdir -p "$BACKUP_DIR"

# Backup current binary and config
tar -czf "$BACKUP_FILE" \
    -C "$PROJECT_ROOT" target/release/lichen-validator \
    -C "$LICHEN_HOME" config.toml genesis.json 2>/dev/null || true

print_success "Backup created: $BACKUP_FILE"
ROLLBACK_VERSION="$BACKUP_FILE"

# Stop validator
if [ "$VALIDATOR_RUNNING" = true ]; then
    print_info "Stopping validator..."
    sudo systemctl stop lichen-validator
    sleep 2
    print_success "Validator stopped"
fi

# Pull latest code
print_info "Pulling latest code..."
cd "$PROJECT_ROOT"
git pull origin main

# Build new version
print_info "Building new version..."
cargo build --release

if [ $? -ne 0 ]; then
    print_error "Build failed!"
    print_warning "Rolling back..."
    
    # Restore from backup
    tar -xzf "$ROLLBACK_VERSION" -C /
    
    if [ "$VALIDATOR_RUNNING" = true ]; then
        sudo systemctl start lichen-validator
    fi
    
    print_error "Upgrade failed, rolled back to previous version"
    exit 1
fi

print_success "Build successful"

# Run tests
print_info "Running tests..."
cargo test --release

if [ $? -ne 0 ]; then
    print_warning "Tests failed, but continuing (review logs)"
fi

# Start validator
if [ "$VALIDATOR_RUNNING" = true ]; then
    print_info "Starting validator..."
    sudo systemctl start lichen-validator
    sleep 3
    
    if systemctl is-active --quiet lichen-validator; then
        print_success "Validator started successfully"
    else
        print_error "Validator failed to start"
        print_warning "Check logs: sudo journalctl -u lichen-validator -n 50"
        exit 1
    fi
fi

# Verify upgrade
print_info "Verifying upgrade..."
sleep 5

# Check if RPC is responding
RPC_PORT=8899
if curl -sf "http://localhost:$RPC_PORT" > /dev/null 2>&1; then
    print_success "RPC server responding"
else
    print_warning "RPC server not responding (may still be starting)"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
print_success "🦞 Upgrade complete!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
print_info "Backup available at: $BACKUP_FILE"
print_info "Monitor logs: sudo journalctl -u lichen-validator -f"
print_info "Check health: $LICHEN_HOME/health-check.sh"
