# 🦞 Lichen Validator Setup Guide

**Production-ready testnet/mainnet validator deployment**

---

## Quick Start

### 1. Build Lichen

```bash
cd lichen
cargo build --release
```

### 2. Generate Genesis Configuration

```bash
# For testnet
./scripts/generate-genesis.sh --network testnet --output ./genesis.json

# For mainnet (with security warnings)
./scripts/generate-genesis.sh --network mainnet --output ./genesis.json
```

### 3. Setup Validator

```bash
# Basic testnet setup
./scripts/setup-validator.sh --network testnet --genesis ./genesis.json

# Custom configuration
./scripts/setup-validator.sh \
  --network testnet \
  --genesis ./genesis.json \
  --home ~/.lichen \
  --p2p-port 7001 \
  --rpc-port 8899
```

### 4. Start Validator

```bash
# Using generated start script
~/.lichen/start-validator.sh

# Or manually
./target/release/lichen-validator --genesis ~/.lichen/genesis.json 7001
```

---

## Directory Structure

After setup, your Lichen installation looks like:

```
~/.lichen/
├── config.toml                    # Validator configuration
├── genesis.json                   # Genesis configuration
├── validator-keypair.json         # Validator identity (KEEP SECURE!)
├── data/                          # Blockchain state
├── logs/                          # Validator logs
├── backups/                       # Automated backups
├── start-validator.sh             # Start script
├── health-check.sh                # Health monitoring
└── backup.sh                      # Manual backup script
```

---

## Configuration Files

### config.toml

Complete validator configuration with sections for:

- **[validator]** - Identity, data directory, validation mode
- **[network]** - P2P/RPC ports, seed nodes, gossip settings
- **[consensus]** - Stake requirements, slot duration, slashing
- **[rpc]** - Server settings, CORS, rate limiting
- **[logging]** - Log level, file output, format
- **[monitoring]** - Prometheus metrics, health checks
- **[genesis]** - Genesis file path, chain ID
- **[performance]** - Worker threads, optimization flags
- **[security]** - Firewall checks, encryption, allowed methods

### genesis.json

Chain initialization parameters:

```json
{
  "chain_id": "lichen-testnet-1",
  "genesis_time": "(set to current UTC time at genesis creation)",
  "consensus": {
    "slot_duration_ms": 400,
    "epoch_slots": 432000,
    "min_validator_stake": 75000000000,
    "validator_reward_per_block": 20000000,
    "slashing_percentage_double_sign": 50,
    "slashing_downtime_per_100_missed": 1,
    "slashing_downtime_max_percent": 10,
    "finality_threshold_percent": 66
  },
  "initial_accounts": [...],
  "initial_validators": [...],
  "network": {...},
  "features": {...}
}
```

---

## Helper Scripts

### reset-blockchain.sh

**Purpose**: Stop, reset, and optionally restart the full local stack.

**Usage**:
```bash
./skills/validator/reset-blockchain.sh testnet --restart
```

You can pass external sweep RPCs when restarting:
```bash
./skills/validator/reset-blockchain.sh testnet --restart https://api.devnet.solana.com https://eth.llamarpc.com
```

### setup-validator.sh

**Purpose**: One-command validator initialization

**Features**:
- Directory structure creation
- Keypair generation with secure permissions
- Genesis configuration copying
- Config file generation
- Helper scripts creation
- Security verification
- Systemd service installation (Linux)

**Usage**:
```bash
./scripts/setup-validator.sh [OPTIONS]

Options:
  --network <testnet|mainnet>    Network to join (default: testnet)
  --home <PATH>                  Lichen home directory
  --genesis <PATH>               Path to genesis.json file (required)
  --keypair <PATH>               Path to validator keypair (optional)
  --data-dir <PATH>              Data directory
  --p2p-port <PORT>              P2P port (default: testnet=7001, mainnet=8001)
  --rpc-port <PORT>              RPC port (default: testnet=8899, mainnet=9899)
  --install-service              Install systemd service (Linux only)
  --help                         Show help message
```

**Examples**:
```bash
# Basic testnet
./scripts/setup-validator.sh --network testnet --genesis ./genesis.json

# Production mainnet with systemd
./scripts/setup-validator.sh \
  --network mainnet \
  --genesis ./genesis.json \
  --install-service

# Custom ports and directories
./scripts/setup-validator.sh \
  --genesis ./genesis.json \
  --p2p-port 8001 \
  --rpc-port 9899 \
  --data-dir /mnt/lichen
```

### health-check.sh

**Purpose**: Monitor validator status and alert on issues

**Checks**:
- ✓ RPC server responding
- ✓ Chain progressing (slot advancement)
- ✓ Active validator count
- ✓ Network metrics (TPS, total transactions, blocks)
- ✓ Disk space usage

**Usage**:
```bash
# Single check
~/.lichen/health-check.sh

# Continuous monitoring
~/.lichen/health-check.sh --watch
```

**Output**:
```
🦞 Lichen Validator Health Check
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Checking: http://localhost:9000

✓ RPC server healthy
✓ Current slot: 12345
✓ Chain progressing normally
✓ Active validators: 3
✓ TPS: 250, Total TXs: 1000000, Blocks: 5000
✓ Disk usage: 45%

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✓ All checks passed ✓
```

**Alerting**:
Set environment variables for notifications:
```bash
export LICHEN_ALERT_EMAIL="admin@example.com"
export LICHEN_SLACK_WEBHOOK="https://hooks.slack.com/..."
```

### upgrade-validator.sh

**Purpose**: Safely upgrade validator to new version

**Features**:
- Automatic backup before upgrade
- Git pull and build
- Test execution
- Rollback on failure
- Service restart
- Health verification

**Usage**:
```bash
./scripts/upgrade-validator.sh
```

**Process**:
1. ✓ Create backup
2. ✓ Stop validator
3. ✓ Pull latest code
4. ✓ Build new version
5. ✓ Run tests
6. ✓ Start validator
7. ✓ Verify upgrade
8. ✗ Rollback on failure

---

## Security Best Practices

### Keypair Management

**Critical**: Your `validator-keypair.json` is your validator identity!

✅ **DO**:
- Set permissions to 600 (owner read/write only)
- Store backups in encrypted storage
- Use hardware security modules (HSM) for production
- Keep offline backups in multiple secure locations
- Use different keypairs for testnet and mainnet

❌ **DON'T**:
- Commit keypairs to version control
- Share keypairs over unencrypted channels
- Store on shared/networked filesystems
- Reuse keypairs across environments

### Firewall Configuration

**Required ports**:
```bash
# P2P networking
ufw allow 8000/tcp

# RPC API (optional, for external access)
ufw allow 9000/tcp

# Prometheus metrics (optional)
ufw allow 9100/tcp
```

### Access Control

**Production checklist**:
- [ ] RPC bind to localhost only (`bind_address = "127.0.0.1"`)
- [ ] Enable RPC rate limiting
- [ ] Configure allowed RPC methods whitelist
- [ ] Use reverse proxy (nginx) for public RPC
- [ ] Enable TLS/SSL certificates
- [ ] Monitor access logs
- [ ] Implement IP allowlisting

---

## Systemd Service (Linux)

### Installation

Automatically installed with:
```bash
./scripts/setup-validator.sh --install-service
```

This helper is now legacy for production v0.4.5 and intentionally fails fast. Use `deploy/setup.sh` plus `docs/deployment/PRODUCTION_DEPLOYMENT.md` for current production installs.

Or manually:
```bash
sudo cp scripts/lichen-validator.service /etc/systemd/system/
sudo systemctl daemon-reload
```

### Management

```bash
# Enable auto-start on boot
sudo systemctl enable lichen-validator

# Start validator
sudo systemctl start lichen-validator

# Check status
sudo systemctl status lichen-validator

# View logs
sudo journalctl -u lichen-validator -f

# Restart
sudo systemctl restart lichen-validator

# Stop
sudo systemctl stop lichen-validator
```

### Service Features

- ✓ Automatic restart on failure
- ✓ Resource limits (memory, CPU, file handles)
- ✓ Security hardening (no new privileges, private tmp)
- ✓ Systemd journal logging
- ✓ Network dependency management

---

## Monitoring

### Health Checks

**Automated monitoring**:
```bash
# Cron job for continuous monitoring
*/5 * * * * /home/lichen/.lichen/health-check.sh
```

**Monitoring as a service** (systemd timer):
```bash
# Create timer unit
sudo systemctl enable --now lichen-health.timer
```

### Prometheus Metrics

**Exposed metrics** (port 9100):
- `lichen_slot_height` - Current slot
- `lichen_validator_count` - Active validators
- `lichen_tps` - Transactions per second
- `lichen_total_transactions` - Total transaction count
- `lichen_total_blocks` - Total block count
- `lichen_burned_licn` - Total burned LICN

**Scrape configuration**:
```yaml
scrape_configs:
  - job_name: 'lichen'
    static_configs:
      - targets: ['localhost:9100']
```

---

## Troubleshooting

### Validator won't start

**Check 1: Genesis file**
```bash
cat ~/.lichen/genesis.json | jq '.'
```

**Check 2: Keypair permissions**
```bash
ls -l ~/.lichen/validator-keypair.json
# Should be: -rw------- (600)
```

**Check 3: Port conflicts**
```bash
lsof -i :8000  # P2P port
lsof -i :9000  # RPC port
```

**Check 4: Logs**
```bash
tail -f ~/.lichen/logs/validator.log
```

### Chain not progressing

**Symptoms**: Slot number not increasing

**Causes**:
1. Not enough validators (need minimum stake)
2. Network connectivity issues
3. Clock synchronization problems

**Solutions**:
```bash
# Check validator status
curl -X POST http://localhost:9000 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getValidators","params":[]}'

# Check system time
date
timedatectl status

# Verify P2P connectivity
netstat -an | grep 8000
```

### High disk usage

**Check disk space**:
```bash
df -h ~/.lichen/data
```

**Cleanup old logs**:
```bash
find ~/.lichen/logs -name "*.log" -mtime +7 -delete
```

**Archive old data**:
```bash
~/.lichen/backup.sh
```

### RPC not responding

**Test RPC connection**:
```bash
curl -X POST http://localhost:9000 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'
```

**Check bind address**:
```bash
grep bind_address ~/.lichen/config.toml
# Should be "0.0.0.0" for external access
```

**Verify firewall**:
```bash
sudo ufw status
sudo iptables -L -n | grep 9000
```

---

## Backup & Recovery

### Manual Backup

```bash
# Run backup script
~/.lichen/backup.sh

# Backups stored in:
~/.lichen/backups/
```

**Backup contents**:
- Validator keypair
- Configuration files
- Genesis configuration
- Blockchain state data

### Automated Backups

**Daily backups via cron**:
```bash
# Add to crontab
0 2 * * * /home/lichen/.lichen/backup.sh
```

### Recovery

**Restore from backup**:
```bash
cd ~/.lichen/backups
tar -xzf lichen-backup-YYYYMMDD-HHMMSS.tar.gz -C ~/.lichen
tar -xzf lichen-backup-YYYYMMDD-HHMMSS.tar.gz.data -C ~/.lichen/data
```

---

## Performance Tuning

### Hardware Requirements

**Minimum** (Testnet):
- CPU: 2 cores
- RAM: 4 GB
- Disk: 100 GB SSD
- Network: 10 Mbps

**Recommended** (Mainnet):
- CPU: 4+ cores
- RAM: 16 GB
- Disk: 500 GB NVMe SSD
- Network: 100 Mbps

### Optimization Tips

**1. Worker threads**:
```toml
[performance]
worker_threads = 4  # Number of CPU cores
```

**2. File handle limits**:
```bash
ulimit -n 65536
```

**3. Network buffers**:
```bash
sudo sysctl -w net.core.rmem_max=134217728
sudo sysctl -w net.core.wmem_max=134217728
```

**4. Disable swap**:
```bash
sudo swapoff -a
```

---

## CLI Commands

### Validator Management

```bash
# Initialize validator keypair
lichen init --output ~/.lichen/validator-keypair.json

# Check validator identity
lichen identity show --keypair ~/.lichen/validator-keypair.json

# Get validator pubkey
lichen pubkey --keypair ~/.lichen/validator-keypair.json
```

### Monitoring

```bash
# Check balance
lichen balance <address>

# List validators
lichen validators

# Get current slot
lichen slot

# Get latest block
lichen latest

# Get total burned LICN
lichen burned
```

---

## Support

**Documentation**: https://developers.lichen.network
**GitHub**: https://github.com/lobstercove/lichen
**Email**: hello@lichen.network
**Discord**: https://discord.gg/gkQmsHXRXp
**X**: https://x.com/LichenHQ
**Telegram**: https://t.me/lichenhq

---

**🦞 Ready to grow! Let's build the economic future for agents! 🦞**
