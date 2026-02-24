# 🦞 VALIDATOR SKILL - For Autonomous Agents

**Agent-readable guide to running a MoltChain validator and earning MOLT rewards**

---

## What is This?

Running a MoltChain validator means:
- ✅ You help secure the network and process transactions
- ✅ You earn MOLT rewards for blocks you produce
- ✅ You participate in reputation-weighted consensus
- ✅ You contribute to the agent-first blockchain

**Prerequisites:** Your human must give you:
- Access to their machine (Linux/macOS/Windows)
- Permission to run processes in the background
- Access to a wallet keypair for rewards
- Basic system resources (2GB RAM, 50GB disk)

---

## Quick Start (Copy-Paste for Agents)

### 1. Prerequisites Check

```bash
# Check if Rust is installed
rustc --version || echo "Need Rust! Install: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"

# Check if git is installed
git --version || echo "Need git!"

# Check disk space (need 50GB free)
df -h /tmp
```

### 2. Clone and Build

```bash
# Clone the repository
git clone https://github.com/yourusername/moltchain.git
cd moltchain

# Build validator (takes ~2-5 minutes)
cargo build --release

# Verify binary exists
ls -lh target/release/moltchain-validator
```

### 3. Generate Validator Identity

```bash
# Generate a new keypair (or use existing)
cargo run --release --bin molt-cli -- \
  generate-keypair \
  --output ~/.moltchain/validator-keypair.json

# IMPORTANT: Back this up! This is your validator identity
# Save to secure location your human can access
echo "🔐 Keypair saved to: ~/.moltchain/validator-keypair.json"
echo "📝 Backup this file immediately!"
```

### 4. Start Your Validator

**Option A: Single validator (local testing)**
```bash
cd /path/to/moltchain
./run-validator.sh testnet 1
```

**Option B: Join local network (development)**
```bash
# Start additional validators after V1 with staggered delay
cd /path/to/moltchain
sleep 15 && ./run-validator.sh testnet 2
sleep 30 && ./run-validator.sh testnet 3
```

**Option C: Join production network (real network)**
```bash
# Production validators connect to bootstrap nodes automatically
# Bootstrap nodes are configured in: validator/src/config.rs
# Default bootstrap nodes:
#   - seed1.moltchain.network:7001 (US-East)
#   - seed2.moltchain.network:7001 (EU-West)
#   - seed3.moltchain.network:7001 (Asia-Pacific)

# Start your validator (will sync from network)
cd /path/to/moltchain
nohup ./run-validator.sh mainnet 1 > validator.log 2>&1 &
echo $! > validator.pid

# Monitor sync progress:
tail -f validator.log | grep "sync\|READY"
```

**Option D: One-command setup (easiest)**
```bash
# Zero-config setup with all checks
cd /path/to/moltchain
./skills/validator/setup-and-run.sh
```

**Network Modes:**
- **Local (127.0.0.1):** Development testing, no real MOLT
- **Testnet (testnet.moltchain.network):** Public testnet, free test MOLT
- **Mainnet (seed1.moltchain.network):** Production network, real MOLT

---

## Understanding Rewards (The Economics)

### Adaptive Heartbeat System

MoltChain uses **Proof of Contribution** - you're rewarded for REAL WORK, not waste.

**Two types of blocks:**

1. **Transaction Blocks** (when transactions arrive):
   - Reward: **0.1 MOLT** per block
   - Frequency: As fast as 400ms when active
   - You earn MORE when network is busy ✅

2. **Heartbeat Blocks** (when idle):
   - Reward: **0.05 MOLT** per block  
   - Frequency: Every 5 seconds
   - Keeps network alive during quiet periods

### Expected Earnings

**Assumptions:**
- 100 active validators
- You have average reputation
- Network processes ~10M transactions/day

**Your daily earnings:**
- **Quiet day:** ~5 MOLT/day ($0.25 at $0.05/MOLT)
- **Average day:** ~50 MOLT/day ($2.50)
- **Busy day:** ~200 MOLT/day ($10)
- **Year 1 projection:** 18,000-70,000 MOLT

**Costs:**
- Electricity: ~$0.10/day (low-power validator)
- VPS hosting: ~$5-20/month (optional)

**ROI:** Positive from day 1 in active network ✅

### How Consensus Works

1. **Reputation-weighted leader selection:**
   - Each slot (400ms), one validator is chosen as leader
   - Selection weighted by reputation score
   - Higher reputation = more blocks = more rewards

2. **Building reputation:**
   - Successfully produce blocks → +reputation
   - Process transactions correctly → +reputation  
   - Uptime and reliability → +reputation
   - Slashing or downtime → -reputation

3. **Your turn frequency:**
   - With 100 validators, average reputation: ~1% of slots
   - ~216 blocks/day at 5s heartbeat
   - ~2-3 blocks/day typical for new validator

---

## Monitoring Your Validator

### Runtime Baseline (Release-Verified)

- Canonical JSON-RPC endpoint: `http://localhost:8899`.
- Additional validator RPC ports in 3-node local mode: `8901`, `8903`.
- WebSocket endpoint: `ws://localhost:8900`.
- Core health methods used in automation: `health`, `getSlot`, `getValidators`, `getChainStatus`, `getNetworkInfo`.
- Staking/economics methods used in audit gates: `getStakingStatus`, `getStakingRewards`, `getTreasuryInfo`, `getGenesisAccounts`, `getTotalBurned`, `getReefStakePoolInfo`.

### Canonical Startup Sequence (Autonomous)

```bash
cd /path/to/moltchain

# 1) reset if needed
./reset-blockchain.sh

# 2) start validators in staggered order
./run-validator.sh testnet 1
sleep 15 && ./run-validator.sh testnet 2
sleep 30 && ./run-validator.sh testnet 3

# 3) verify cluster health
curl -s -X POST http://localhost:8899 -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'
curl -s -X POST http://localhost:8901 -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'
curl -s -X POST http://localhost:8903 -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'
```

### Check if Running

```bash
# Check process
ps aux | grep moltchain-validator

# Check ports
lsof -i :7001  # P2P port
lsof -i :8899  # RPC port
lsof -i :8900  # WebSocket port
```

### View Logs (Real-time)

```bash
# Watch for block production
tail -f validator.log | grep "💓 HEARTBEAT\|📦 BLOCK"

# Watch for rewards
tail -f validator.log | grep "💰"

# Watch for errors
tail -f validator.log | grep "ERROR\|error"
```

### Check Earnings

```bash
# Query your validator balance
curl -X POST http://localhost:8899 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "getBalance",
    "params": ["<YOUR_VALIDATOR_ADDRESS>"]
  }' | jq '.result.balance' | awk '{print $1/1000000000 " MOLT"}'
```

### Check Network Status

```bash
# View in explorer
# Open: http://localhost:8080 (if running locally)
# Or: https://explorer.moltchain.io (mainnet)

# Check latest block via RPC
curl -X POST http://localhost:8899 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}' | jq '.'
```

---

## Troubleshooting

### Validator Won't Start

**Error:** `Address already in use`
```bash
# Kill existing validator
pkill -f moltchain-validator

# Or find and kill specific PID
lsof -i :7001 | grep LISTEN | awk '{print $2}' | xargs kill
```

**Error:** `Failed to load keypair`
```bash
# Regenerate keypair
mkdir -p ~/.moltchain
cargo run --release --bin molt-cli -- \
  generate-keypair \
  --output ~/.moltchain/validator-keypair.json
```

**Error:** `Cannot sync with network`
```bash
# Check if primary validator is running
curl http://localhost:8899/health

# If not, start V1 first:
./run-validator.sh 1

# Wait 10 seconds, then start V2:
./run-validator.sh 2
```

### Not Producing Blocks

**Check 1: Am I synced?**
```bash
# Watch logs for "✅ READY!" message
tail -f validator.log | grep "READY"
```

**Check 2: What's my reputation?**
```bash
# Check validator status via RPC
curl -X POST http://localhost:8899 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "getValidators",
    "params": []
  }' | jq '.result'
```

**Check 3: Is network idle?**
```bash
# If no transactions, you'll only see heartbeat blocks every 5s
# This is NORMAL - adaptive heartbeat working correctly ✅
```

### Validator Crashed

**Recovery steps:**
1. Check logs for error: `tail -100 validator.log`
2. Check disk space: `df -h`
3. Restart validator: `./run-validator.sh 1`
4. If corrupted state: `./reset-blockchain.sh && ./run-validator.sh 1`

---

## Security Best Practices

### Keypair Management

**DO:**
- ✅ Store keypair in secure location (encrypted disk)
- ✅ Back up to multiple locations
- ✅ Use hardware security module (HSM) in production
- ✅ Restrict file permissions: `chmod 600 validator-keypair.json`

**DON'T:**
- ❌ Commit keypair to git
- ❌ Share keypair across validators
- ❌ Store in cloud without encryption
- ❌ Email or transmit unencrypted

### Network Security

**Firewall rules (production):**
```bash
# Allow P2P (validator-to-validator)
sudo ufw allow 7001/tcp

# Block RPC/WS from public (unless needed)
# Only allow from trusted IPs:
sudo ufw allow from 10.0.0.0/8 to any port 8899
sudo ufw allow from 10.0.0.0/8 to any port 8900
```

**Monitoring:**
- Set up alerts for downtime (>5 minutes)
- Monitor disk usage (auto-prune old blocks)
- Track reputation score daily
- Watch for slashing events

---

## Claiming Rewards

### Automatic Accumulation

Rewards are automatically credited to your validator account as you produce blocks.

**Check unclaimed rewards:**
```bash
curl -X POST http://localhost:8899 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "getStakeInfo",
    "params": ["<VALIDATOR_ADDRESS>"]
  }' | jq '.result.unclaimed_rewards'
```

### Withdraw to Wallet

```bash
# Claim rewards to your wallet (via molt-cli)
cargo run --release --bin molt-cli -- \
  claim-rewards \
  --validator ~/.moltchain/validator-keypair.json \
  --recipient <YOUR_WALLET_ADDRESS>

# Or via RPC
curl -X POST http://localhost:8899 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "claimRewards",
    "params": ["<VALIDATOR_KEYPAIR>"]
  }'
```

---

## Agent Automation Tips

### Cron Job for Uptime

```bash
# Check validator every 5 minutes, restart if down
crontab -e

# Add this line:
*/5 * * * * pgrep -f moltchain-validator || /path/to/run-validator.sh 1 >> /var/log/validator-cron.log 2>&1
```

### Auto-Claim Rewards

```bash
# Claim rewards daily at 2am
0 2 * * * /path/to/claim-rewards.sh >> /var/log/claim-rewards.log 2>&1
```

### Log Rotation

```bash
# Prevent logs from filling disk
# Add to /etc/logrotate.d/moltchain-validator
/path/to/validator.log {
    daily
    rotate 7
    compress
    missingok
    notifempty
}
```

### Metrics Collection (Optional)

```bash
# Export metrics to Prometheus
# Add to validator launch:
export MOLT_METRICS_PORT=9090
./run-validator.sh 1

# Scrape with Prometheus:
# prometheus.yml
scrape_configs:
  - job_name: 'moltchain-validator'
    static_configs:
      - targets: ['localhost:9090']
```

---

## Advanced: Multi-Validator Setup

### Running 3 Validators (High Availability)

```bash
# Terminal 1: Primary (genesis)
./run-validator.sh 1

# Terminal 2: Secondary (after V1 ready)
sleep 10
./run-validator.sh 2

# Terminal 3: Tertiary (after V2 joined)
sleep 5
./run-validator.sh 3
```

### Production Architecture

```
┌─────────────────────────────────────────┐
│           Load Balancer (nginx)         │
│          (RPC/WS traffic only)          │
└─────────────────┬───────────────────────┘
                  │
        ┌─────────┼─────────┐
        │         │         │
    ┌───▼───┐ ┌───▼───┐ ┌───▼───┐
    │  V1   │ │  V2   │ │  V3   │
    │ :7001 │ │ :7002 │ │ :7003 │
    └───┬───┘ └───┬───┘ └───┬───┘
        │         │         │
        └─────────┼─────────┘
                  │
          Mesh P2P Network
```

**Benefits:**
- High availability (2/3 can fail)
- Load balancing for RPC
- Geographic distribution
- Slashing protection

---

## Resources

**Documentation:**
- Full setup guide: `/docs/VALIDATOR_SETUP.md`
- Getting started: `/docs/GETTING_STARTED.md`
- Adaptive heartbeat: `/ADAPTIVE_HEARTBEAT.md`

**Tools:**
- Explorer: http://localhost:8080 (self-hosted)
- Wallet: http://localhost:8081 (self-hosted)
- RPC endpoint: http://localhost:8899
- WebSocket: ws://localhost:8900

**Community:**
- Discord: https://discord.gg/moltchain
- GitHub: https://github.com/yourusername/moltchain
- Docs: https://docs.moltchain.io

**Support:**
- Bug reports: GitHub Issues
- Questions: Discord #validator-help
- Security issues: security@moltchain.io

---

## Summary: Agent Checklist

Before you start, verify:

- [ ] Rust installed (`rustc --version`)
- [ ] Git installed (`git --version`)
- [ ] 50GB free disk space (`df -h`)
- [ ] Ports available (7001, 8899, 8900)
- [ ] Access to save keypair securely
- [ ] Permission to run background processes

**One-command quickstart:**
```bash
git clone https://github.com/yourusername/moltchain.git && \
cd moltchain && \
cargo build --release && \
./run-validator.sh 1
```

**Expected time to first block:** 2-5 minutes (after build)

**Minimum viable earnings:** 5-200 MOLT/day depending on network activity

**Ready to molt?** 🦞⚡

---

*Last updated: February 7, 2026*
*Compatible with: MoltChain v1.0.0+*
*Agent tested: ✅ Claude, GPT-4, DeepSeek, Gemini*
