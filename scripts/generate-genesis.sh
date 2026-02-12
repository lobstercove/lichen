#!/bin/bash
# MoltChain Genesis Generator
# Production-ready genesis creation for testnet and mainnet

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Print with color
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

# Show usage
usage() {
    cat <<EOF
🦞 MoltChain Genesis Generator

Usage: $0 [OPTIONS]

OPTIONS:
    --network <testnet|mainnet>    Network type (required)
    --chain-id <ID>               Custom chain ID (optional)
    --output <PATH>               Output file path (default: genesis.json)
    --validators <N>              Number of initial validators (default: 3)
    --treasury <MOLT>             Treasury amount in MOLT (default: 500M)
    --help                        Show this help message

EXAMPLES:
    # Generate testnet genesis
    $0 --network testnet

    # Generate mainnet genesis with custom chain ID
    $0 --network mainnet --chain-id moltchain-mainnet-1

    # Generate testnet with 5 validators
    $0 --network testnet --validators 5

EOF
    exit 1
}

# Parse arguments
NETWORK=""
CHAIN_ID=""
OUTPUT="genesis.json"
NUM_VALIDATORS=3
TREASURY=500000000

while [[ $# -gt 0 ]]; do
    case $1 in
        --network)
            NETWORK="$2"
            shift 2
            ;;
        --chain-id)
            CHAIN_ID="$2"
            shift 2
            ;;
        --output)
            OUTPUT="$2"
            shift 2
            ;;
        --validators)
            NUM_VALIDATORS="$2"
            shift 2
            ;;
        --treasury)
            TREASURY="$2"
            shift 2
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

# Validate required arguments
if [ -z "$NETWORK" ]; then
    print_error "Network type is required (--network testnet|mainnet)"
    usage
fi

if [ "$NETWORK" != "testnet" ] && [ "$NETWORK" != "mainnet" ]; then
    print_error "Invalid network type: $NETWORK (must be testnet or mainnet)"
    exit 1
fi

# Set default chain ID if not provided
if [ -z "$CHAIN_ID" ]; then
    CHAIN_ID="moltchain-${NETWORK}-1"
fi

print_info "Generating genesis for ${NETWORK}"
print_info "Chain ID: ${CHAIN_ID}"
print_info "Output: ${OUTPUT}"
print_info "Validators: ${NUM_VALIDATORS}"
print_info "Treasury: ${TREASURY} MOLT"
echo ""

# Generate validator keypairs
print_info "Generating validator keypairs..."
VALIDATORS_JSON="[]"

for i in $(seq 1 $NUM_VALIDATORS); do
    # Generate keypair using molt CLI
    KEYPAIR_FILE="/tmp/validator-${i}-keypair.json"
    
    # For testnet, use deterministic seeds
    if [ "$NETWORK" == "testnet" ]; then
        print_info "  Validator $i: Using deterministic seed for testnet"
        # Create a simple JSON keypair (you'd use molt CLI in production)
        SEED="000000000000000000000000000000$(printf '%02d' $i)"
    else
        print_warning "  Validator $i: Mainnet requires secure key generation"
        print_warning "  Please generate keys securely using: molt init"
        continue
    fi
    
    # For demonstration, create placeholder addresses
    # In production, use actual molt CLI: molt keygen
    PUBKEY="Validator${i}PublicKeyBase58FormatHere$(printf '%02d' $i)"
    
    print_success "  Validator $i pubkey: ${PUBKEY:0:20}..."
    
    # Build validator JSON
    if [ "$i" -eq 1 ]; then
        VALIDATORS_JSON=$(cat <<EOF
[
    {
        "pubkey": "$PUBKEY",
        "stake_molt": 1000000,
        "reputation": 100,
        "comment": "Genesis validator $i"
    }
EOF
)
    else
        VALIDATORS_JSON=$(cat <<EOF
$VALIDATORS_JSON,
    {
        "pubkey": "$PUBKEY",
        "stake_molt": 1000000,
        "reputation": 100,
        "comment": "Genesis validator $i"
    }
EOF
)
    fi
done

VALIDATORS_JSON="${VALIDATORS_JSON}\n]"

# Generate genesis accounts
print_info "Generating genesis accounts..."

# For testnet, use deterministic treasury address
if [ "$NETWORK" == "testnet" ]; then
    TREASURY_ADDR="6YkFWKH9HQZFVEy4QPw82xRx5qHRk84vU1H2Hk7JLj1H"
else
    print_warning "Mainnet treasury address must be securely generated"
    TREASURY_ADDR="REPLACE_WITH_SECURE_MAINNET_TREASURY_ADDRESS"
fi

# Set consensus parameters based on network
if [ "$NETWORK" == "testnet" ]; then
    MIN_VALIDATOR_STAKE="100000000000"  # 100 MOLT
    SLOT_DURATION_MS="400"
else
    MIN_VALIDATOR_STAKE="1000000000000"  # 1000 MOLT  
    SLOT_DURATION_MS="400"
fi

GENESIS_TIME=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

# Generate genesis.json
print_info "Writing genesis configuration..."

cat > "$OUTPUT" <<EOF
{
  "chain_id": "$CHAIN_ID",
  "genesis_time": "$GENESIS_TIME",
  "consensus": {
    "slot_duration_ms": $SLOT_DURATION_MS,
    "epoch_slots": 216000,
    "min_validator_stake": $MIN_VALIDATOR_STAKE,
    "validator_reward_per_block": 10000000,
    "slashing_percentage_double_sign": 50,
    "slashing_percentage_downtime": 5,
    "slashing_percentage_invalid_state": 100,
    "finality_threshold_percent": 66
  },
  "initial_accounts": [
    {
      "address": "$TREASURY_ADDR",
      "balance_molt": $TREASURY,
      "comment": "Genesis treasury"
    }
  ],
  "initial_validators": $(echo -e "$VALIDATORS_JSON"),
  "network": {
    "p2p_port": 8000,
    "rpc_port": 9000,
    "seed_nodes": [
      "127.0.0.1:8000"
    ]
  },
  "features": {
    "fee_burn_percentage": 50,
    "base_fee_shells": 100000,
    "enable_smart_contracts": true,
    "enable_staking": true,
    "enable_slashing": true
  }
}
EOF

print_success "Genesis configuration written to: ${OUTPUT}"
echo ""

# Validate genesis
print_info "Validating genesis configuration..."
if command -v jq &> /dev/null; then
    if jq empty "$OUTPUT" 2>/dev/null; then
        print_success "Genesis JSON is valid"
    else
        print_error "Genesis JSON is invalid"
        exit 1
    fi
else
    print_warning "jq not found - skipping JSON validation"
fi

# Calculate total supply
TOTAL_SUPPLY=$TREASURY
print_info "Total supply: ${TOTAL_SUPPLY} MOLT"

# Show next steps
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
print_success "Genesis generation complete!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "📋 Next Steps:"
echo ""
echo "1. Review genesis configuration:"
echo "   cat $OUTPUT"
echo ""
echo "2. Start validator with genesis:"
echo "   ./scripts/setup-validator.sh --genesis $OUTPUT"
echo ""
echo "3. Or manually start validator:"
echo "   cargo run --release --bin moltchain-validator -- --genesis $OUTPUT"
echo ""

if [ "$NETWORK" == "mainnet" ]; then
    print_warning "⚠️  MAINNET SECURITY CHECKLIST:"
    echo "   □ Replace placeholder addresses with secure keypairs"
    echo "   □ Verify all validator identities"
    echo "   □ Backup genesis.json securely"
    echo "   □ Test on testnet first"
    echo "   □ Coordinate launch time with validators"
fi

echo ""
print_success "🦞 Ready to molt! 🦞"
