#!/bin/bash
# MoltChain Validator Health Check
# Monitors validator status and alerts on issues

RPC_URL="${MOLTCHAIN_RPC_URL:-http://localhost:8899}"
ALERT_EMAIL="${MOLTCHAIN_ALERT_EMAIL:-}"
SLACK_WEBHOOK="${MOLTCHAIN_SLACK_WEBHOOK:-}"
CHECK_INTERVAL=30
MAX_MISSED_SLOTS=10

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

print_error() {
    echo -e "${RED}✗${NC} $1"
}

print_success() {
    echo -e "${GREEN}✓${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

send_alert() {
    local message="$1"
    
    # Send email if configured
    if [ -n "$ALERT_EMAIL" ] && command -v mail &> /dev/null; then
        echo "$message" | mail -s "MoltChain Validator Alert" "$ALERT_EMAIL"
    fi
    
    # Send Slack notification if configured
    if [ -n "$SLACK_WEBHOOK" ]; then
        curl -X POST "$SLACK_WEBHOOK" \
            -H 'Content-Type: application/json' \
            -d "{\"text\":\"🦞 MoltChain Alert: $message\"}" \
            2>/dev/null
    fi
}

rpc_call() {
    local method="$1"
    local params="${2:-[]}"
    
    curl -sf -X POST "$RPC_URL" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}" 2>/dev/null
}

check_health() {
    local response=$(rpc_call "health")
    
    if [ -z "$response" ]; then
        print_error "RPC server not responding"
        return 1
    fi
    
    local status=$(echo "$response" | jq -r '.result.status' 2>/dev/null)
    if [ "$status" = "ok" ]; then
        print_success "RPC server healthy"
        return 0
    else
        print_error "RPC server returned error"
        return 1
    fi
}

check_sync_status() {
    local response=$(rpc_call "getSlot")
    
    if [ -z "$response" ]; then
        print_error "Cannot get current slot"
        return 1
    fi
    
    local current_slot=$(echo "$response" | jq -r '.result' 2>/dev/null)
    
    if [ -z "$current_slot" ] || [ "$current_slot" = "null" ]; then
        print_error "Invalid slot response"
        return 1
    fi
    
    print_success "Current slot: $current_slot"
    
    # Check if chain is progressing
    sleep 2
    local next_response=$(rpc_call "getSlot")
    local next_slot=$(echo "$next_response" | jq -r '.result' 2>/dev/null)
    
    if [ "$next_slot" -le "$current_slot" ]; then
        print_warning "Chain not progressing (slot: $current_slot)"
        return 1
    fi
    
    print_success "Chain progressing normally"
    return 0
}

check_validators() {
    local response=$(rpc_call "getValidators")
    
    if [ -z "$response" ]; then
        print_warning "Cannot get validator list"
        return 1
    fi
    
    local validator_count=$(echo "$response" | jq '.result.validators | length' 2>/dev/null)
    
    if [ -z "$validator_count" ] || [ "$validator_count" = "0" ]; then
        print_error "No validators found"
        return 1
    fi
    
    print_success "Active validators: $validator_count"
    return 0
}

check_metrics() {
    local response=$(rpc_call "getMetrics")
    
    if [ -z "$response" ]; then
        print_warning "Cannot get metrics"
        return 1
    fi
    
    if command -v jq &> /dev/null; then
        local tps=$(echo "$response" | jq -r '.result.tps' 2>/dev/null)
        local total_txs=$(echo "$response" | jq -r '.result.total_transactions' 2>/dev/null)
        local blocks=$(echo "$response" | jq -r '.result.total_blocks' 2>/dev/null)
        
        print_success "TPS: $tps, Total TXs: $total_txs, Blocks: $blocks"
    fi
    
    return 0
}

check_disk_space() {
    local data_dir="${MOLTCHAIN_DATA_DIR:-$HOME/.moltchain/data}"
    
    if [ ! -d "$data_dir" ]; then
        print_warning "Data directory not found: $data_dir"
        return 1
    fi
    
    local usage=$(df -h "$data_dir" | awk 'NR==2 {print $5}' | sed 's/%//')
    
    if [ "$usage" -gt 90 ]; then
        print_error "Disk usage critical: ${usage}%"
        send_alert "Disk usage critical: ${usage}% at $data_dir"
        return 1
    elif [ "$usage" -gt 80 ]; then
        print_warning "Disk usage high: ${usage}%"
        return 1
    else
        print_success "Disk usage: ${usage}%"
        return 0
    fi
}

# Main health check
main() {
    echo "🦞 MoltChain Validator Health Check"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Checking: $RPC_URL"
    echo ""
    
    local all_healthy=true
    
    # Run all checks
    check_health || all_healthy=false
    check_sync_status || all_healthy=false
    check_validators || all_healthy=false
    check_metrics || all_healthy=false
    check_disk_space || all_healthy=false
    
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    
    if [ "$all_healthy" = true ]; then
        print_success "All checks passed ✓"
        exit 0
    else
        print_error "Some checks failed ✗"
        send_alert "Validator health check failed"
        exit 1
    fi
}

# Run once or continuously
if [ "$1" = "--watch" ]; then
    echo "Running continuous health monitoring (interval: ${CHECK_INTERVAL}s)"
    echo "Press Ctrl+C to stop"
    echo ""
    
    while true; do
        main
        sleep "$CHECK_INTERVAL"
        echo ""
    done
else
    main
fi
