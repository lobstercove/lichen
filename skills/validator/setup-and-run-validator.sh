#!/bin/bash
# Lichen Validator - One-Command Setup and Run
# For agents and humans alike 🦞⚡

set -e  # Exit on error

echo "🦞 Lichen Validator Setup"
echo "============================"
echo ""

# Color codes
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Check prerequisites
echo "📋 Checking prerequisites..."

# Check Rust
if ! command -v rustc &> /dev/null; then
    echo -e "${RED}❌ Rust not found${NC}"
    echo "   Install with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi
echo -e "${GREEN}✓${NC} Rust $(rustc --version | awk '{print $2}')"

# Check Cargo
if ! command -v cargo &> /dev/null; then
    echo -e "${RED}❌ Cargo not found${NC}"
    exit 1
fi
echo -e "${GREEN}✓${NC} Cargo installed"

# Check disk space (need 50GB)
available_space=$(df -k . | tail -1 | awk '{print $4}')
required_space=52428800  # 50GB in KB
if [ "$available_space" -lt "$required_space" ]; then
    echo -e "${YELLOW}⚠️  Low disk space: $(echo "scale=1; $available_space/1048576" | bc)GB available${NC}"
    echo "   Recommended: 50GB+ free"
    read -p "Continue anyway? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
else
    echo -e "${GREEN}✓${NC} Disk space: $(echo "scale=1; $available_space/1048576" | bc)GB available"
fi

# Choose network
echo ""
echo "🌐 Select network:"
echo "  1) Testnet (local)"
echo "  2) Mainnet (local)"
read -p "Enter number (1-2) [1]: " NETWORK_CHOICE
NETWORK_CHOICE=${NETWORK_CHOICE:-1}

if [[ "$NETWORK_CHOICE" == "2" ]]; then
    NETWORK="mainnet"
    PORTS=(8001 9899 9900)
else
    NETWORK="testnet"
    PORTS=(7001 8899 8900)
fi

# Check ports
echo ""
echo "🔌 Checking ports..."
for port in "${PORTS[@]}"; do
    if lsof -Pi :$port -sTCP:LISTEN -t >/dev/null 2>&1; then
        echo -e "${YELLOW}⚠️  Port $port is in use${NC}"
        echo "   Kill existing process? (pkill -f lichen-validator)"
        read -p "Kill and continue? (y/N): " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            pkill -f lichen-validator || true
            sleep 2
        else
            exit 1
        fi
    else
        echo -e "${GREEN}✓${NC} Port $port available"
    fi
done

# Build if needed
echo ""
echo "🔨 Building validator..."
if [ ! -f "target/release/lichen-validator" ]; then
    echo "   First build - this will take 2-5 minutes..."
    cargo build --release
    echo -e "${GREEN}✓${NC} Build complete"
else
    echo "   Binary exists, checking if rebuild needed..."
    if [ "$(find . -name '*.rs' -newer target/release/lichen-validator | wc -l)" -gt 0 ]; then
        echo "   Source changed, rebuilding..."
        cargo build --release
        echo -e "${GREEN}✓${NC} Rebuild complete"
    else
        echo -e "${GREEN}✓${NC} Binary up to date"
    fi
fi

# Check for keypair
echo ""
echo "🔐 Checking validator identity..."
KEYPAIR_DIR="$HOME/.lichen"
KEYPAIR_PATH="$KEYPAIR_DIR/validator-keypair.json"

mkdir -p "$KEYPAIR_DIR"

if [ ! -f "$KEYPAIR_PATH" ]; then
    echo "   No keypair found, generating..."
    cargo run --release --bin lichen-cli -- \
        generate-keypair \
        --output "$KEYPAIR_PATH" 2>/dev/null
    
    echo -e "${GREEN}✓${NC} Keypair generated: $KEYPAIR_PATH"
    echo -e "${YELLOW}⚠️  IMPORTANT: Back up this file!${NC}"
    echo "   This is your validator identity."
    echo ""
    
    # Set secure permissions
    chmod 600 "$KEYPAIR_PATH"
    echo -e "${GREEN}✓${NC} Keypair permissions secured (600)"
else
    echo -e "${GREEN}✓${NC} Using existing keypair: $KEYPAIR_PATH"
fi

# Get validator address
VALIDATOR_ADDRESS=$(cargo run --release --bin lichen-cli -- \
    pubkey --keypair "$KEYPAIR_PATH" 2>/dev/null | tail -1)
echo "   Validator address: $VALIDATOR_ADDRESS"

# Ask which validator to run
echo ""
echo "🚀 Ready to launch validator!"
echo ""
echo "Which validator would you like to run?"
if [ "$NETWORK" = "mainnet" ]; then
    echo "  1) V1-PRIMARY   (genesis validator, port 8001)"
    echo "  2) V2-SECONDARY (joins V1, port 8002)"
    echo "  3) V3-TERTIARY  (joins V1, port 8003)"
else
    echo "  1) V1-PRIMARY   (genesis validator, port 7001)"
    echo "  2) V2-SECONDARY (joins V1, port 7002)"
    echo "  3) V3-TERTIARY  (joins V1, port 7003)"
fi
echo ""
read -p "Enter number (1-3) [1]: " VALIDATOR_NUM
VALIDATOR_NUM=${VALIDATOR_NUM:-1}

# Validate input
if [[ ! "$VALIDATOR_NUM" =~ ^[1-3]$ ]]; then
    echo -e "${RED}❌ Invalid choice: $VALIDATOR_NUM${NC}"
    exit 1
fi

# Check if joining network (V2 or V3)
if [ "$VALIDATOR_NUM" -gt 1 ]; then
    echo ""
    echo -e "${YELLOW}⚠️  You selected a secondary validator (V$VALIDATOR_NUM)${NC}"
    echo "   Make sure V1-PRIMARY is already running!"
    echo ""
    
    # Check if V1 is up
    if [ "$NETWORK" = "mainnet" ]; then
        HEALTH_URL="http://localhost:9899/health"
    else
        HEALTH_URL="http://localhost:8899/health"
    fi

    if ! curl -s "$HEALTH_URL" &>/dev/null; then
        echo -e "${RED}❌ V1-PRIMARY not detected at ${HEALTH_URL}${NC}"
        echo ""
        echo "   Start V1 first:"
        echo "   ./setup-and-run-validator.sh"
        echo "   (and select option 1)"
        echo ""
        exit 1
    else
        echo -e "${GREEN}✓${NC} V1-PRIMARY is running"
    fi

    # Propagate genesis-keys from V1 to this validator's data dir
    # Without genesis-keys, this node won't be able to serve RPC airdrop requests
    if [ "$NETWORK" = "mainnet" ]; then
        V1_DB="data/state-8001"
        MY_DB="data/state-$((8000 + VALIDATOR_NUM))"
    else
        V1_DB="data/state-7001"
        MY_DB="data/state-$((7000 + VALIDATOR_NUM))"
    fi

    if [ -d "$V1_DB/genesis-keys" ]; then
        mkdir -p "$MY_DB/genesis-keys"
        if [ -f "$V1_DB/genesis-wallet.json" ] && [ ! -f "$MY_DB/genesis-wallet.json" ]; then
            cp "$V1_DB/genesis-wallet.json" "$MY_DB/genesis-wallet.json"
            echo -e "${GREEN}✓${NC} Copied genesis-wallet.json from V1"
        fi
        for keyfile in "$V1_DB/genesis-keys/"*.json; do
            [ -f "$keyfile" ] || continue
            fname="$(basename "$keyfile")"
            if [ ! -f "$MY_DB/genesis-keys/$fname" ]; then
                cp "$keyfile" "$MY_DB/genesis-keys/$fname"
                echo -e "${GREEN}✓${NC} Copied genesis-keys/$fname from V1"
            fi
        done
    else
        echo -e "${YELLOW}⚠️  V1 genesis-keys not found at $V1_DB/genesis-keys/${NC}"
        echo "   This node will not be able to serve airdrop requests."
        echo "   Copy genesis-wallet.json and genesis-keys/ from V1 after genesis completes."
    fi
fi

# Launch validator
echo ""
echo "🎯 Launching validator V$VALIDATOR_NUM..."
echo ""
echo "Monitor with:"
echo "  - Logs: tail -f validator.log"
echo "  - Explorer: http://localhost:8080"
echo "  - RPC: curl http://localhost:8899/health"
echo ""

# Run in foreground (user can background with Ctrl+Z, bg)
exec ./run-validator.sh "$NETWORK" "$VALIDATOR_NUM"
