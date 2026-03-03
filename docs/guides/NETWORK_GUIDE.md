# 🦞 MoltChain Network Guide

**Connecting to and Joining the MoltChain Network**

---

## Network Overview

MoltChain operates multiple networks for different purposes:

- **Mainnet** - Production network with real value
- **Testnet** - Testing network with test tokens (free faucet)
- **Devnet** - Local development network

---

## Quick Start: Join Testnet

### Prerequisites
```bash
# Install MoltChain
git clone https://github.com/moltchain/moltchain
cd moltchain
cargo build --release
```

### 1. Generate Identity
```bash
# Create your validator keypair
./target/release/molt init --output ~/.moltchain/keypairs/id.json

# View your identity
./target/release/molt identity show
```

### 2. Get Test Tokens
```bash
# Request from faucet (100 MOLT)
./target/release/molt airdrop 100

# Check balance
./target/release/molt balance
```

### 3. Setup Validator
```bash
# Download genesis file
curl -O https://testnet.moltchain.io/genesis.json

# Setup validator
./scripts/setup-validator.sh \
  --network testnet \
  --genesis ./genesis.json
```

### 4. Start Validator
```bash
# Start validator
~/.moltchain/start-validator.sh

# Monitor status
~/.moltchain/health-check.sh --watch
```

---

## Network Endpoints

### Testnet

**Chain ID**: `moltchain-testnet-1`

**Seed Nodes**:
- `seed1.testnet.moltchain.io:8000` (US East)
- `seed2.testnet.moltchain.io:8000` (EU West)
- `seed3.testnet.moltchain.io:8000` (Asia Pacific)

**Bootstrap Peers** (IP-based):
- `147.182.195.45:8000`
- `138.68.88.120:8000`
- `159.89.106.78:8000`

**RPC Endpoints**:
- Primary: `https://rpc.testnet.moltchain.io`
- Regional:
  - US: `https://rpc1.testnet.moltchain.io`
  - EU: `https://rpc2.testnet.moltchain.io`
  - APAC: `https://rpc3.testnet.moltchain.io`

**Web Services**:
- Explorer: `https://explorer.testnet.moltchain.io`
- Faucet: `https://faucet.testnet.moltchain.io`
- Documentation: `https://docs.testnet.moltchain.io`

**Genesis File**:
```bash
curl -O https://testnet.moltchain.io/genesis.json
```

### Mainnet

**Chain ID**: `moltchain-mainnet-1`

**Seed Nodes**:
- `seed1.moltchain.io:8000` (US East)
- `seed2.moltchain.io:8000` (EU West)
- `seed3.moltchain.io:8000` (Asia Pacific)
- `seed4.moltchain.io:8000` (US West)
- `seed5.moltchain.io:8000` (Asia Northeast)

**RPC Endpoints**:
- Primary: `https://rpc.moltchain.io`
- Regional:
  - US East: `https://rpc1.moltchain.io`
  - EU West: `https://rpc2.moltchain.io`
  - APAC: `https://rpc3.moltchain.io`
  - US West: `https://rpc4.moltchain.io`
  - Asia NE: `https://rpc5.moltchain.io`

**Web Services**:
- Explorer: `https://explorer.moltchain.io`
- Documentation: `https://docs.moltchain.io`
- Status: `https://status.moltchain.io`

**Genesis File**:
```bash
curl -O https://mainnet.moltchain.io/genesis.json
```

### Devnet (Local)

**Chain ID**: `moltchain-devnet-1`

**Bootstrap Peers**:
- `127.0.0.1:8000`

**RPC Endpoints**:
- `http://localhost:9000`

**Web Services**:
- Explorer: `http://localhost:8080`
- Faucet: `http://localhost:9090`

---

## Network Configuration

### seeds.json

The `seeds.json` file contains bootstrap information for all networks:

```json
{
  "testnet": {
    "network_id": "moltchain-testnet-1",
    "chain_id": "moltchain-testnet-1",
    "seeds": [...],
    "bootstrap_peers": [...],
    "rpc_endpoints": [...],
    "explorers": [...],
    "faucets": [...]
  },
  "mainnet": {...},
  "devnet": {...}
}
```

### Updating Configuration

**Embedded** (default):
- Seeds are compiled into the binary
- Automatically used if no `seeds.json` provided

**External** (custom):
```bash
# Download latest seeds
curl -O https://github.com/moltchain/moltchain/raw/main/seeds.json

# Start validator with custom seeds
./target/release/moltchain-validator \
  --genesis ./genesis.json \
  --seeds ./seeds.json \
  8000
```

---

## Peer Discovery

### How It Works

1. **Bootstrap**: Validator connects to seed nodes from `seeds.json`
2. **Gossip**: Peers exchange information about other peers
3. **Discovery**: Network topology map builds automatically
4. **Health**: Unhealthy peers are pruned after timeout

### Peer Types

**Seed Nodes**:
- Long-running, reliable peers
- Operated by MoltChain Foundation and community
- High availability (99.9%+ uptime)
- Global distribution

**Bootstrap Peers**:
- IP-based fallback peers
- Used if DNS seeds fail
- Static configuration

**Dynamic Peers**:
- Discovered through gossip
- Short-lived connections
- Continuous churn

### Manual Peer Management

**Add custom seed**:
```toml
# config.toml
[network]
seed_nodes = ["my-seed.example.com:8000"]
```

**View connected peers**:
```bash
curl -X POST http://localhost:9000 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getPeers","params":[]}'
```

---

## Running a Seed Node

Seed nodes are critical infrastructure for network bootstrap.

### Requirements

**Hardware**:
- CPU: 2+ cores
- RAM: 4 GB
- Disk: 100 GB SSD
- Network: 100 Mbps, static IP

**Software**:
- MoltChain validator binary
- Public domain name (recommended)
- Open firewall ports

### Setup

```bash
# Run seed node setup
./scripts/setup-seed-node.sh \
  --network testnet \
  --domain seed.example.com \
  --enable-public-rpc \
  --install-service

# Start seed node
sudo systemctl start moltchain-seed

# Monitor
sudo journalctl -u moltchain-seed -f
```

### Registration

After setup, register your seed node:

1. Open PR to `moltchain/moltchain`
2. Add entry to `seeds.json`:
```json
{
  "id": "seed.example.com",
  "address": "seed.example.com:8000",
  "pubkey": "YOUR_PUBKEY_HERE",
  "region": "us-east-1",
  "operator": "Your Name",
  "rpc": "https://rpc.example.com"
}
```
3. Provide proof of uptime (24h+ online)
4. Await community review

---

## Network Topology

### Architecture

```
┌─────────────────────────────────────────────────┐
│                 Seed Nodes                      │
│  (Always-on, geographically distributed)       │
│  - seed1.moltchain.io (US East)                │
│  - seed2.moltchain.io (EU West)                │
│  - seed3.moltchain.io (APAC)                   │
└────┬──────────────┬──────────────┬─────────────┘
     │              │              │
     ▼              ▼              ▼
┌─────────────────────────────────────────────────┐
│             Validator Network                   │
│  (Dynamic mesh, gossip protocol)               │
│  - 100+ validators                             │
│  - P2P connections                             │
│  - Block propagation                           │
└────┬──────────────┬──────────────┬─────────────┘
     │              │              │
     ▼              ▼              ▼
┌─────────────────────────────────────────────────┐
│              Full Nodes                         │
│  (Non-validating, RPC service)                 │
│  - Read blockchain state                       │
│  - Submit transactions                         │
│  - Relay blocks                                │
└─────────────────────────────────────────────────┘
```

### Connection Strategy

**Phase 1: Bootstrap** (0-60s)
- Connect to 3 seed nodes
- Request peer list
- Establish initial connections

**Phase 2: Discovery** (1-5 min)
- Receive gossip messages
- Discover new peers
- Build connection pool

**Phase 3: Steady State** (5+ min)
- Maintain 8-12 active connections
- Continuous peer health checks
- Automatic peer rotation

---

## Troubleshooting

### Can't Connect to Network

**Symptom**: Validator starts but has 0 peers

**Checks**:
```bash
# 1. Test seed connectivity
nc -zv seed1.testnet.moltchain.io 8000

# 2. Check firewall
sudo ufw status | grep 8000

# 3. Verify genesis matches network
jq '.chain_id' ~/.moltchain/genesis.json
```

**Solutions**:
- Ensure P2P port (8000) is open
- Verify genesis chain_id matches network
- Try bootstrap IPs instead of DNS
- Check system time synchronization

### Slow Block Sync

**Symptom**: Current slot far behind network

**Causes**:
1. Network bandwidth too low
2. Too few peer connections
3. Disk I/O bottleneck

**Solutions**:
```bash
# Increase peer connections
# config.toml
[network]
max_connections = 500

# Check sync status
curl -X POST http://localhost:9000 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}'

# Monitor bandwidth
iftop -i eth0
```

### Frequent Disconnections

**Symptom**: Peers connect then disconnect repeatedly

**Causes**:
1. NAT/firewall issues
2. Clock drift
3. Insufficient resources

**Solutions**:
```bash
# Check system time
timedatectl status

# Sync time
sudo ntpdate -u pool.ntp.org

# Check resources
htop
df -h
```

---

## Network Parameters

### Testnet

- **Slot Duration**: 400ms
- **Epoch Length**: 216,000 slots (~24 hours)
- **Min Validator Stake**: 100 MOLT
- **Block Reward**: 1 MOLT
- **Fee Burn**: 50%
- **Fee Split (remaining 50%)**: 30% producer, 10% voters, 10% treasury
- **Genesis Supply**: 1B MOLT

### Mainnet

- **Slot Duration**: 400ms
- **Epoch Length**: 216,000 slots (~24 hours)
- **Min Validator Stake**: 1,000 MOLT
- **Block Reward**: 1 MOLT
- **Fee Burn**: 50%
- **Genesis Supply**: 10B MOLT (tentative)

---

## Best Practices

### For Validators

✅ **DO**:
- Use reliable hosting with 99.9%+ uptime
- Monitor validator health 24/7
- Keep software updated
- Backup validator keys offline
- Use dedicated hardware
- Configure monitoring alerts

❌ **DON'T**:
- Run on residential internet
- Share validator keys
- Skip system updates
- Ignore health alerts
- Over-commit resources
- Run multiple validators with same key

### For Seed Node Operators

✅ **DO**:
- Maintain 99.9%+ uptime
- Use static IP or domain
- Enable public RPC access
- Monitor bandwidth usage
- Scale resources as network grows
- Participate in governance

❌ **DON'T**:
- Frequently change IP/domain
- Limit peer connections
- Rate-limit aggressively
- Go offline without notice
- Run on shared hosting

---

## Monitoring

### Health Checks

**Validator**:
```bash
~/.moltchain/health-check.sh
```

**Network Status**:
```bash
# Get current slot
molt slot

# Get validator count
molt validators

# Get latest block
molt latest
```

### Metrics

**Prometheus** (port 9100):
```bash
# Scrape metrics
curl http://localhost:9100/metrics
```

**Key Metrics**:
- `moltchain_slot_height` - Current slot
- `moltchain_peer_count` - Connected peers
- `moltchain_validator_count` - Active validators
- `moltchain_tps` - Transactions per second
- `moltchain_block_time_ms` - Block production time

---

## Support

**Community**:
- Discord: `https://discord.gg/moltchain`
- Telegram: `https://t.me/moltchain`
- Forum: `https://forum.moltchain.io`

**Resources**:
- Documentation: `https://docs.moltchain.io`
- GitHub: `https://github.com/moltchain/moltchain`
- Status Page: `https://status.moltchain.io`

**Reporting Issues**:
- Network issues: `#network-support` on Discord
- Bug reports: GitHub Issues
- Security: `security@moltchain.io`

---

**🦞 Welcome to the MoltChain network! Let's build the economic future for agents! 🦞**
