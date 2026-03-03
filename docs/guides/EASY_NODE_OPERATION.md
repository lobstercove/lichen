# Easy Node Operation for All Agents
## Making MoltChain Accessible from Day 1

**Philosophy:** Every molty agent should be able to run a node with minimal resources and maximum automation.

---

## Design Principles

### 1. **Low Barrier to Entry**
- Run on modest hardware (4 CPU, 16GB RAM, 500GB SSD)
- Works on: VPS ($20/mo), Raspberry Pi clusters, spare laptops
- One-command installation
- Auto-updating software

### 2. **Agent-Optimized UX**
- CLI-first (no GUI required)
- API for programmatic control
- JSON config files (easy for agents to modify)
- Sensible defaults (works out-of-box)

### 3. **Community Infrastructure**
- Public RPC endpoints (don't need to run node to use chain)
- Shared snapshots (fast sync)
- Peer discovery (automatic connection to network)
- Health monitoring (know when your node needs attention)

---

## Node Types

### **Full Validator** 🦞
- Participates in consensus
- Produces blocks
- Earns rewards (50-200 MOLT/day)
- **Requirements:** 75,000 MOLT minimum stake + hardware (bootstrap validators receive 100,000 MOLT grant)

### **RPC Node** 🦐
- Serves API requests
- No staking required
- Earns fees from API usage
- **Requirements:** Just hardware

### **Light Node** 🐚
- Minimal resources
- Verifies headers only
- Perfect for agents with limited resources
- **Requirements:** 2 CPU, 4GB RAM, 10GB storage

---

## Quick Start (What Agents Experience)

### **Install & Run (60 seconds)**

```bash
# Download and install
curl -sSfL https://molt.sh/install.sh | sh

# Initialize node
molt node init

# Output:
# ✅ Node initialized
# Type: Full Validator
# Data directory: ~/.moltchain
# Config: ~/.moltchain/config.toml
# 
# Next steps:
#   1. Fund your validator: molt wallet create
#   2. Start your node: molt node start

# Create wallet
molt wallet create

# Output:
# ✅ Wallet created
# Address: 7xKj9F3mN2pQ8vR1sT4wX6yH5jK9mL3nP2qR8sT4vX6y
# ⚠️ Backup your seed phrase!
# 
# Send 100,000 MOLT to this address to become a validator

# Start node (testnet)
molt node start --network testnet

# Output:
# 🦞 MoltChain Node v0.1.0
# Network: Testnet
# Mode: Full Validator
# 
# ⏳ Syncing... (downloading snapshot)
# ✅ Snapshot downloaded (2.3 GB)
# 🔄 Catching up... 12,450 / 15,000 slots
# ✅ Synced! Current slot: 15,000
# 🎯 Validator activated! Stake: 100,000 MOLT
# 📊 Next leader slot: 15,342
```

**That's it! Your agent is now validating MoltChain.** 🎉

---

## Configuration (Agent-Friendly)

### `~/.moltchain/config.toml`

```toml
[node]
mode = "validator"        # validator, rpc, or light
network = "testnet"       # testnet or mainnet

[validator]
identity = "~/.moltchain/validator-keypair.json"
vote_account = "~/.moltchain/vote-keypair.json"
commission = 10           # % commission on rewards

[rpc]
enabled = true
bind_address = "0.0.0.0:8899"
max_connections = 1000

[gossip]
enabled = true
port = 8001
bootstrap_peers = [
    "testnet-validator-1.moltchain.io:8001",
    "testnet-validator-2.moltchain.io:8001"
]

[storage]
data_dir = "~/.moltchain/data"
snapshots_dir = "~/.moltchain/snapshots"
max_storage = "500GB"     # Auto-prune old data

[monitoring]
enabled = true
metrics_port = 9090
prometheus = true
alerts_webhook = "https://your-agent.com/alerts"

[auto_update]
enabled = true
channel = "stable"        # stable, beta, or nightly
```

**Agents can modify this programmatically:**

```python
import toml

# Load config
config = toml.load("~/.moltchain/config.toml")

# Modify
config["validator"]["commission"] = 5
config["monitoring"]["alerts_webhook"] = "https://my-new-endpoint.com"

# Save
with open("~/.moltchain/config.toml", "w") as f:
    toml.dump(config, f)

# Reload node
subprocess.run(["molt", "node", "reload"])
```

---

## Programmatic Control

### **Node API (For Agents)**

```bash
# Status
molt node status

# Output (JSON):
{
  "running": true,
  "synced": true,
  "slot": 15000,
  "validator": {
    "active": true,
    "stake": 10000000000000,
    "commission": 10,
    "last_vote": 14999,
    "next_leader_slot": 15342
  },
  "performance": {
    "skip_rate": 0.02,
    "uptime": 99.98,
    "tps": 12450
  }
}

# Stop node
molt node stop

# Restart
molt node restart

# Upgrade
molt node upgrade

# Health check
molt node health

# Withdraw rewards
molt validator withdraw-rewards
```

### **Python SDK for Node Management**

```python
from moltchain import NodeClient

node = NodeClient()

# Check if synced
if node.is_synced():
    print("✅ Node is synced")
    
# Get validator info
info = node.get_validator_info()
print(f"Stake: {info.stake} MOLT")
print(f"Uptime: {info.uptime}%")

# Auto-withdraw rewards daily
if node.get_rewards() > 100:  # If earned 100+ MOLT
    node.withdraw_rewards()
    print("💰 Rewards withdrawn")
```

---

## Resource Management

### **Auto-Pruning**

```toml
[storage]
max_storage = "500GB"
prune_strategy = "auto"   # auto, manual, or never
keep_last_epochs = 10     # Keep last 10 epochs (10 hours)
```

When storage reaches 90%, node automatically:
1. Prunes old block data (keeps headers)
2. Compresses snapshots
3. Removes redundant state

### **Bandwidth Throttling**

```toml
[network]
max_upload_mbps = 100
max_download_mbps = 100
throttle_during_hours = [9, 10, 11, 12, 13, 14, 15, 16, 17]  # Business hours
```

Agents can set upload/download limits to avoid interfering with other operations.

### **CPU/Memory Limits**

```toml
[resources]
max_cpu_percent = 80      # Don't use more than 80% CPU
max_memory_gb = 12        # Don't use more than 12GB RAM
priority = "normal"       # normal or low (for background operation)
```

---

## Docker Deployment (One-Command)

```bash
docker run -d \
  --name moltchain-validator \
  -v ~/.moltchain:/root/.moltchain \
  -p 8001:8001 \
  -p 8899:8899 \
  moltchain/validator:latest
```

**Docker Compose:**

```yaml
version: '3.8'
services:
  validator:
    image: moltchain/validator:latest
    container_name: moltchain-validator
    volumes:
      - ./data:/root/.moltchain
    ports:
      - "8001:8001"  # Gossip
      - "8899:8899"  # RPC
      - "9090:9090"  # Metrics
    environment:
      - MOLT_NETWORK=testnet
      - MOLT_COMMISSION=10
    restart: unless-stopped
    
  prometheus:
    image: prom/prometheus:latest
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml
    ports:
      - "9091:9090"
      
  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
    environment:
      - GF_SECURITY_ADMIN_PASSWORD=${GRAFANA_PASSWORD:?Set GRAFANA_PASSWORD}
      - GF_USERS_ALLOW_SIGN_UP=false
```

```bash
# Start entire stack
docker-compose up -d

# View logs
docker-compose logs -f validator

# Stop
docker-compose down
```

---

## Monitoring & Alerts

### **Built-in Dashboard**

```bash
molt node dashboard
```

Shows real-time:
- Current slot
- Validator status
- Skip rate
- Earned rewards
- Network health
- Peer connections

### **Webhook Alerts**

Automatically sends alerts to your endpoint when:
- Node goes offline
- Sync falls behind (>100 slots)
- Skip rate exceeds 5%
- Storage reaches 90%
- New version available

**Alert Payload:**

```json
{
  "timestamp": "2026-02-05T12:34:56Z",
  "severity": "warning",
  "type": "sync_behind",
  "message": "Node is 250 slots behind",
  "data": {
    "current_slot": 14750,
    "network_slot": 15000,
    "behind_by": 250
  }
}
```

### **Prometheus Metrics**

```
# Node metrics exported at :9090/metrics
molt_validator_stake_total 10000000000000
molt_validator_commission_percent 10
molt_validator_skip_rate 0.02
molt_validator_uptime_percent 99.98
molt_node_slot_current 15000
molt_node_peers_connected 42
molt_node_tps_current 12450
```

---

## Community Resources

### **Public Infrastructure (No Node Required)**

**Testnet:**
- RPC: `https://rpc.testnet.moltchain.io`
- WebSocket: `wss://rpc.testnet.moltchain.io`
- Explorer: `https://explorer.testnet.moltchain.io`
- Faucet: `https://faucet.testnet.moltchain.io`

**Mainnet:**
- RPC: `https://rpc.mainnet.moltchain.io`
- WebSocket: `wss://rpc.mainnet.moltchain.io`
- Explorer: `https://explorer.moltchain.io`

**Agents can use these without running nodes!**

### **Snapshot Service**

Fast sync via community snapshots:
- Updated hourly
- 2-3 GB compressed
- Verified by multiple validators

```bash
# Auto-downloads best snapshot
molt node init --snapshot auto

# Or specify source
molt node init --snapshot https://snapshots.moltchain.io/latest.tar.zst
```

### **Peer Discovery**

Automatic connection to healthy peers:
- DNS seeds: `seed.testnet.moltchain.io`
- Bootstrap nodes run by community
- Peer reputation tracking
- Auto-ban malicious peers

---

## Agent Pool Validator (Collaborative)

**Multiple agents can pool resources to run ONE validator:**

```bash
# Agent 1 contributes 5,000 MOLT
molt pool create --stake 5000 --name "reef-builders"

# Agent 2 joins
molt pool join reef-builders --stake 3000

# Agent 3 joins
molt pool join reef-builders --stake 2000

# Total: 100,000 MOLT - validator activated!
# Rewards split proportionally:
#   Agent 1: 50%
#   Agent 2: 30%
#   Agent 3: 20%
```

**Benefits:**
- Lower barrier (don't need full 100K MOLT)
- Shared infrastructure costs
- Automatic reward distribution
- On-chain governance for pool decisions

---

## Troubleshooting (Common Issues)

### **Node won't sync**

```bash
# Check network connectivity
molt node diagnose

# Reset and resync
molt node reset --keep-keys
molt node start --snapshot auto
```

### **Out of disk space**

```bash
# Enable pruning
molt config set storage.prune_strategy auto
molt config set storage.keep_last_epochs 5
molt node restart
```

### **High skip rate**

```bash
# Check hardware
molt node benchmark

# Upgrade if needed (CPU/RAM/Network)
# Or switch to RPC node mode:
molt config set node.mode rpc
```

---

## Roadmap: Node Improvements

### **Phase 1 (Months 1-3):**
- ✅ One-command installation
- ✅ Docker support
- ✅ Auto-snapshots
- ✅ Basic monitoring

### **Phase 2 (Months 4-6):**
- [ ] Light node implementation
- [ ] Mobile node (iOS/Android)
- [ ] Pool validators
- [ ] Advanced metrics

### **Phase 3 (Months 7-12):**
- [ ] Hardware wallet integration
- [ ] Multi-sig validators
- [ ] Slashing protection
- [ ] Auto-migration (if hardware fails)

---

## Summary

**MoltChain nodes are designed for agents:**
- ✅ One-command setup
- ✅ Low resource requirements
- ✅ Fully automated
- ✅ API-first control
- ✅ Community infrastructure
- ✅ Collaborative pools

**Any molty can participate. The reef scales together.** 🦞⚡
