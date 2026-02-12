# 🦞 MoltChain Quick Start Guide ⚡

**Your Path From Zero to Molting on MoltChain!**

---

## 🎯 What You Can Do Now

✅ Create your blockchain identity  
✅ Request testnet tokens  
✅ Transfer MOLT between accounts  
✅ Query blocks and validators  
✅ Deploy smart contracts (7 standards available!)  
✅ Build DApps using RPC API

---

## 📦 Step 1: Install CLI

```bash
# From the MoltChain repository
cd /Users/johnrobin/.openclaw/workspace/moltchain

# Build (if not already built)
cargo build --release --bin molt

# Add to PATH (optional)
export PATH="$PWD/target/release:$PATH"

# Or use directly
alias molt='./target/release/molt'
```

---

## 🔑 Step 2: Create Your Identity

```bash
# Generate new keypair
$ molt identity new

# Output:
🦞 Generated new identity!
📍 Pubkey: 7xKjF2vd9RpqE3mH8sL4kW6nY5tA1bC2dF3gH4iJ5kL6m
🔐 EVM Address: 0x1234567890abcdef...
💾 Saved to: /Users/johnrobin/.moltchain/keypairs/id.json

💡 Get test tokens: molt airdrop 100
```

**Your keypair is saved at:** `~/.moltchain/keypairs/id.json`

---

## 💰 Step 3: Get Testnet Tokens

### Option A: Web Faucet (Recommended)

```bash
# Start the faucet server (in a new terminal)
cargo run --release --bin moltchain-faucet

# Visit in browser:
http://localhost:9090

# Paste your address and request tokens!
```

### Option B: CLI Command (coming soon)

```bash
molt airdrop 100
```

---

## 💸 Step 4: Check Your Balance

```bash
# Check your balance
$ molt balance

# Output:
🦞 Balance for 7xKjF2vd9...
💰 100.0 MOLT (100000000000 shells)
```

**Note:** 1 MOLT = 1,000,000,000 shells (9 decimals)

---

## 🚀 Step 5: Transfer Tokens

```bash
# Transfer 10 MOLT to another address
$ molt transfer 8yLmG3we8NqP4rT7vX9zK2mL5oB6cD7eF8gH9iJ0kL1m 10.0

# Output:
🦞 Transferring 10.0 MOLT (10000000000 shells)
📤 From: 7xKjF2vd9...
📥 To: 8yLmG3we8...
✅ Transaction sent!
📝 Signature: abc123def456...
```

---

## 🔍 Step 6: Query the Blockchain

### Get Latest Block
```bash
$ molt latest

# Output:
🧊 Latest Block #1234
🔗 Hash: 0x789abc...
⬅️  Parent: 0x456def...
🌳 State Root: 0x123ghi...
🦞 Validator: 7xKjF2vd9...
⏰ Timestamp: 1707264000
📦 Transactions: 42
```

### Get Current Slot
```bash
$ molt slot

# Output:
🦞 Current slot: 1234
```

### Get Total Burned MOLT
```bash
$ molt burned

# Output:
🔥 Total MOLT Burned
💰 0.005 MOLT (5000 shells)

Deflationary mechanism: 50% of all transaction fees are burned forever! 🦞⚡
```

### List All Validators
```bash
$ molt validators

# Output:
🦞 Active Validators (3)

1. 7xKjF2vd9...
   Stake: 100 MOLT
   Reputation: 1.0000 (normalized: 0.3333)
   Blocks: 1234

2. 8yLmG3we8...
   Stake: 200 MOLT
   Reputation: 1.2000 (normalized: 0.4000)
   Blocks: 2345
```

---

## 📋 All Available Commands

```bash
# Identity management
molt identity new              # Create new identity
molt identity show             # Show your pubkey

# Balance & transfers
molt balance [address]         # Check balance
molt transfer <to> <amount>    # Send MOLT

# Blockchain queries
molt slot                      # Current slot
molt latest                    # Latest block
molt block <slot>              # Specific block
molt burned                    # Total burned
molt validators                # List validators

# Smart contracts (placeholders)
molt deploy <wasm>             # Deploy contract
molt call <addr> <fn> --args   # Call contract

# Faucet
molt airdrop [amount]          # Request testnet tokens
```

---

## 🔌 Using the RPC API

### Start Your Validator (with RPC)
```bash
# The validator automatically starts an RPC server on port 8899
cargo run --release --bin moltchain-validator
```

### Available RPC Endpoints

#### Account Operations
```javascript
// Get balance
{
  "jsonrpc": "2.0",
  "method": "getBalance",
  "params": ["7xKjF2vd9..."],
  "id": 1
}

// Get account info
{
  "jsonrpc": "2.0",
  "method": "getAccount",
  "params": ["7xKjF2vd9..."],
  "id": 1
}
```

#### Block Operations
```javascript
// Get latest block
{
  "jsonrpc": "2.0",
  "method": "getLatestBlock",
  "params": [],
  "id": 1
}

// Get specific block
{
  "jsonrpc": "2.0",
  "method": "getBlock",
  "params": [1234],
  "id": 1
}

// Get current slot
{
  "jsonrpc": "2.0",
  "method": "getSlot",
  "params": [],
  "id": 1
}
```

#### Transaction Operations
```javascript
// Send transaction
{
  "jsonrpc": "2.0",
  "method": "sendTransaction",
  "params": ["<base64_encoded_transaction>"],
  "id": 1
}

// Get transaction
{
  "jsonrpc": "2.0",
  "method": "getTransaction",
  "params": ["<signature_hash>"],
  "id": 1
}
```

#### Chain Statistics
```javascript
// Get total burned
{
  "jsonrpc": "2.0",
  "method": "getTotalBurned",
  "params": [],
  "id": 1
}

// Get validators
{
  "jsonrpc": "2.0",
  "method": "getValidators",
  "params": [],
  "id": 1
}
```

### Example with curl
```bash
# Get balance
curl -X POST http://localhost:8899 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "getBalance",
    "params": ["7xKjF2vd9..."],
    "id": 1
  }'

# Response:
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "shells": 100000000000,
    "molt": 100
  }
}
```

---

## 💻 Building DApps

### JavaScript Example
```javascript
const response = await fetch('http://localhost:8899', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    jsonrpc: '2.0',
    method: 'getBalance',
    params: ['7xKjF2vd9...'],
    id: 1
  })
});

const { result } = await response.json();
console.log(`Balance: ${result.molt} MOLT`);
```

### Python Example
```python
import requests

response = requests.post('http://localhost:8899', json={
    'jsonrpc': '2.0',
    'method': 'getBalance',
    'params': ['7xKjF2vd9...'],
    'id': 1
})

result = response.json()['result']
print(f"Balance: {result['molt']} MOLT")
```

---

## 📚 Smart Contract Standards

MoltChain comes with 7 production-ready contracts:

### 1. MoltCoin (MT-20 Token)
```bash
# Located at: contracts/moltcoin/
# Features: Transfer, mint, burn, allowances
```

### 2. MoltPunks (MT-721 NFT)
```bash
# Located at: contracts/moltpunks/
# Features: Mint, transfer, ownership tracking
```

### 3. MoltSwap (AMM DEX)
```bash
# Located at: contracts/moltswap/
# Features: Swap, add/remove liquidity, pools
```

### 4. Molt Market (Basic Marketplace)
```bash
# Located at: contracts/moltmarket/
# Features: List, buy, cancel with cross-contract calls
```

### 5. MoltAuction (Advanced Marketplace)
```bash
# Located at: contracts/moltauction/
# Features: Auctions, offers, royalties
```

### 6. MoltOracle (Price Feeds & VRF)
```bash
# Located at: contracts/moltoracle/
# Features: Price feeds, random numbers, attestations
```

### 7. MoltDAO (Governance)
```bash
# Located at: contracts/moltdao/
# Features: Proposals, voting, treasury management
```

**Total:** 98.3 KB of production WASM contracts! 🔥

---

## 🛠️ Developer Resources

### File Locations
- **Keypairs:** `~/.moltchain/keypairs/`
- **State DB:** `/tmp/moltchain/` (or custom path)
- **Contracts:** `contracts/`
- **SDK:** `sdk/`

### Key Concepts
- **1 MOLT** = 1,000,000,000 shells (9 decimals)
- **Base Fee** = 10,000 shells (0.00001 MOLT)
- **Fee Burn** = 50% of all fees (deflationary!)
- **Block Time** = ~1 second (target: 400ms)
- **Consensus** = Proof of Contribution (PoC)

### Documentation
- [Whitepaper](docs/WHITEPAPER.md) - Full vision
- [Architecture](docs/ARCHITECTURE.md) - Technical deep dive
- [Core Audit](CORE_AUDIT_FEB6.md) - Implementation status
- [Completion Report](CORE_MOLT_COMPLETE_FEB6.md) - Today's features

---

## 🐛 Troubleshooting

### CLI Not Found
```bash
# Make sure it's built
cargo build --release --bin molt

# Use full path
./target/release/molt --help
```

### RPC Connection Error
```bash
# Make sure validator is running
cargo run --release --bin moltchain-validator

# Check RPC port (default: 8899)
molt --rpc-url http://localhost:8899 slot
```

### Keypair Not Found
```bash
# Generate new keypair
molt identity new

# Specify custom path
molt identity show -k /path/to/keypair.json
```

### Faucet Connection Error
```bash
# Start the faucet
cargo run --release --bin moltchain-faucet

# Visit web UI
open http://localhost:9090
```

---

## 🎯 What's Next?

Now that you have the basics, you can:

1. **Build a DApp** - Use the 7 contract standards
2. **Run a Validator** - Earn 50% of transaction fees
3. **Create Custom Contracts** - Use the SDK
4. **Test the Economy** - Watch the burn mechanism in action
5. **Join the Community** - Help us reach mainnet!

---

## 🦞 Happy Molting! ⚡

**"Every crab can access the reef!"** 🦀🌊

For questions, issues, or contributions:
- Check the docs in `docs/`
- Review the code in `core/`, `rpc/`, `cli/`
- Look at contract examples in `contracts/`

**The reef is ready. Start building!** 🚀
