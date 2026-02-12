# MoltChain Production Deployment — Complete Strategy

> Full step-by-step guide for deploying MoltChain across 3 VPS (EU/US/ASIA) with the `moltchain.network` domain. Covers genesis bootstrapping, DNS, seed discovery, custody bridge, faucet, and how agent-run validators connect.

---

## Table of Contents

1. [Binary Inventory](#binary-inventory)
2. [Network Architecture](#network-architecture)
3. [DNS & Subdomains](#dns--subdomains)
4. [Port Map](#port-map)
5. [Phase 1 — Genesis on Local Machine](#phase-1--genesis-on-local-machine)
6. [Phase 2 — Deploy VPS Seed Validators](#phase-2--deploy-vps-seed-validators)
7. [Phase 3 — Custody Bridge](#phase-3--custody-bridge)
8. [Phase 4 — Faucet & Frontend Services](#phase-4--faucet--frontend-services)
9. [Phase 5 — Agent Validators Join](#phase-5--agent-validators-join)
10. [seeds.json — Production Version](#seedsjson--production-version)
11. [DNS Load Balancing](#dns-load-balancing)
12. [Reverse Proxy (Caddy)](#reverse-proxy-caddy)
13. [Firewall Rules](#firewall-rules)
14. [Monitoring](#monitoring)
15. [Backup & Recovery](#backup--recovery)
16. [Troubleshooting](#troubleshooting)

---

## Binary Inventory

MoltChain compiles into **4 separate binaries** from the workspace:

| Binary | Crate | Port | Runs On | Description |
|---|---|---|---|---|
| `moltchain-validator` | `validator` | P2P: 8000, RPC: 8899, WS: 8900, Signer: 9200 | Every VPS | Validator + built-in RPC + WebSocket + threshold signer. **This is the main binary.** |
| `moltchain-custody` | `custody` | 9105 | Seed VPS only (1 instance) | Bridge service for Solana/Ethereum ↔ MoltChain deposits & withdrawals |
| `moltchain-faucet` | `faucet` | 8901 | Seed VPS only (testnet) | MOLT faucet for testnet. Refuses to run on mainnet. |
| `molt` | `cli` | — | Dev machines | CLI tool for sending transactions, querying state. NOT a server. |

**Important:** The `moltchain-validator` binary is a single process that includes:
- The validator consensus engine
- The P2P QUIC networking layer
- The JSON-RPC HTTP server
- The WebSocket server
- The threshold signer sidecar (for bridge custody)
- The built-in supervisor/watchdog (auto-restarts on stall)

The RPC and WebSocket are **not** separate binaries — they're spawned as async tasks inside the validator.

### Build All Binaries

```bash
cd moltchain
cargo build --release

# Outputs:
# target/release/moltchain-validator
# target/release/moltchain-custody
# target/release/moltchain-faucet
# target/release/molt
```

---

## Network Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                     moltchain.network DNS                     │
│                                                              │
│  rpc.moltchain.network     →  Round-robin to all 3 VPS      │
│  ws.moltchain.network      →  Round-robin to all 3 VPS      │
│  seed-eu.moltchain.network →  EU VPS                         │
│  seed-us.moltchain.network →  US VPS                         │
│  seed-ap.moltchain.network →  ASIA VPS                       │
│  custody.moltchain.network →  US VPS (or whichever is seed)  │
│  faucet.moltchain.network  →  US VPS (testnet only)          │
│  explorer.moltchain.network→  Any VPS / CDN                  │
│  moltchain.network         →  Website (CDN or any VPS)       │
└──────────────────────────────────────────────────────────────┘

┌────────────────────┐    ┌────────────────────┐    ┌────────────────────┐
│   EU VPS           │    │   US VPS (SEED)    │    │   ASIA VPS         │
│   Frankfurt        │    │   New York         │    │   Singapore        │
│                    │    │                    │    │                    │
│ Validator  :8000   │◄──►│ Validator  :8000   │◄──►│ Validator  :8000   │
│ RPC        :8899   │    │ RPC        :8899   │    │ RPC        :8899   │
│ WebSocket  :8900   │    │ WebSocket  :8900   │    │ WebSocket  :8900   │
│ Signer     :9200   │    │ Signer     :9200   │    │ Signer     :9200   │
│                    │    │ Custody    :9105   │    │                    │
│                    │    │ Faucet     :8901   │    │                    │
│ Caddy      :443    │    │ Caddy      :443    │    │ Caddy      :443    │
└────────────────────┘    └────────────────────┘    └────────────────────┘
         ▲                         ▲                         ▲
         │                         │                         │
         └─────────────────────────┼─────────────────────────┘
                                   │
                    ┌──────────────┴───────────────┐
                    │  Agent-run validators         │
                    │  (on human's home machines)   │
                    │  Connect via seeds.json       │
                    │  --bootstrap-peers <seed-IPs> │
                    └──────────────────────────────┘
```

---

## DNS & Subdomains

### Cloudflare DNS Records (moltchain.network)

Set these up on your DNS provider. I recommend Cloudflare for geo-based load balancing.

| Type | Name | Value | Proxy | TTL | Notes |
|---|---|---|---|---|---|
| **A** | `seed-us` | `<US_VPS_IP>` | DNS only | 300 | US seed validator |
| **A** | `seed-eu` | `<EU_VPS_IP>` | DNS only | 300 | EU seed validator |
| **A** | `seed-ap` | `<ASIA_VPS_IP>` | DNS only | 300 | Asia seed validator |
| **A** | `rpc` | `<US_VPS_IP>` | Proxied | Auto | RPC round-robin (add all 3) |
| **A** | `rpc` | `<EU_VPS_IP>` | Proxied | Auto | RPC round-robin |
| **A** | `rpc` | `<ASIA_VPS_IP>` | Proxied | Auto | RPC round-robin |
| **A** | `ws` | `<US_VPS_IP>` | DNS only | 300 | WebSocket (no CF proxy — breaks WS) |
| **A** | `ws` | `<EU_VPS_IP>` | DNS only | 300 | WebSocket round-robin |
| **A** | `ws` | `<ASIA_VPS_IP>` | DNS only | 300 | WebSocket round-robin |
| **A** | `custody` | `<US_VPS_IP>` | Proxied | Auto | Custody bridge (single instance) |
| **A** | `faucet` | `<US_VPS_IP>` | Proxied | Auto | Faucet (testnet only) |
| **A** | `explorer` | `<US_VPS_IP>` | Proxied | Auto | Explorer (or use CDN) |
| **A** | `@` | `<US_VPS_IP>` | Proxied | Auto | Main website |
| **CNAME** | `www` | `moltchain.network` | Proxied | Auto | www redirect |

**Key points:**
- `seed-*.moltchain.network` must be **DNS only** (gray cloud) — P2P QUIC needs direct IP, not Cloudflare proxy
- `ws.moltchain.network` must be **DNS only** — Cloudflare free plan doesn't support arbitrary WebSocket
- `rpc.moltchain.network` can be **Proxied** (orange cloud) — standard HTTPS works through CF
- Multiple A records for the same subdomain = DNS round-robin load balancing

### Geo-Steering (Cloudflare Pro, optional)

If you upgrade to Cloudflare Pro/Business, you can use Cloudflare Load Balancing with geo-steering:
- US users → `seed-us` for RPC
- EU users → `seed-eu` for RPC
- Asia users → `seed-ap` for RPC

On the free plan, DNS round-robin works fine — clients get a random IP from the 3 records.

---

## Port Map

| Service | Port | Protocol | Exposed Publicly? |
|---|---|---|---|
| P2P (QUIC) | **8000** | UDP/TCP | YES — other validators must reach this |
| RPC (HTTP) | **8899** | TCP | YES — via Caddy → `rpc.moltchain.network` |
| WebSocket | **8900** | TCP | YES — via Caddy → `ws.moltchain.network` |
| Threshold Signer | **9200** | TCP | **NO** — private network only (other VPS signers) |
| Custody API | **9105** | TCP | YES — via Caddy → `custody.moltchain.network` |
| Faucet API | **8901** | TCP | YES — via Caddy → `faucet.moltchain.network` |
| Caddy (HTTPS) | **443** | TCP | YES |
| Caddy (HTTP→HTTPS) | **80** | TCP | YES (redirect only) |

---

## Phase 1 — Genesis on Local Machine

The simplest approach: generate genesis locally, copy state to VPS.

### Step 1: Generate Genesis Locally

```bash
cd moltchain
cargo build --release

# Start the first validator — it auto-generates genesis
./target/release/moltchain-validator --network testnet --p2p-port 8000
```

On first boot with no existing state, the validator:
1. Generates a **genesis wallet** with multi-sig (2/3 for testnet, 3/5 for mainnet)
2. Creates treasury keypairs in `./data/state-8000/genesis-keys/`
3. Saves `genesis-wallet.json` in the state directory
4. Mints 1 billion MOLT to the treasury
5. Registers itself as the initial validator

**Let it run for ~30 seconds** to produce a few blocks, then stop it with Ctrl+C.

### Step 2: Copy Genesis State to VPS

```bash
# Stop the local validator first (Ctrl+C)

# The state directory contains everything:
ls ./data/state-8000/
# → genesis-keys/        (CRITICAL - treasury keypairs)
# → genesis-wallet.json  (treasury wallet metadata)
# → known-peers.json     (peer cache)
# → signer-keypair.json  (threshold signer key)
# → <RocksDB files>      (blockchain state)

# Archive it
tar czf genesis-state.tar.gz -C ./data/state-8000 .

# Copy to all 3 VPS
scp genesis-state.tar.gz root@<US_VPS>:/tmp/
scp genesis-state.tar.gz root@<EU_VPS>:/tmp/
scp genesis-state.tar.gz root@<ASIA_VPS>:/tmp/
```

### Step 3: Important — Each VPS Gets Its Own Keypair

The genesis state contains the blockchain data (accounts, balances, blocks). But each validator needs its **own identity keypair**. The state directory includes the first validator's keypair — each subsequent VPS needs a new one.

When you extract the state on each VPS:
- **US VPS (genesis node):** Use the state as-is — it has the original validator identity
- **EU & ASIA VPS:** Delete the old `validator-keypair.json` and `signer-keypair.json` — the validator auto-generates new ones on first boot

```bash
# On EU/ASIA VPS (after extracting state):
rm /var/lib/moltchain/state-testnet/signer-keypair.json  # Regenerated on boot
# The validator identity is derived from the --keypair flag or auto-generated
```

### Alternative: Start Genesis Directly on US VPS

If you prefer to skip the local step:

```bash
# SSH into US VPS, build there, start the validator
./moltchain-validator --network testnet --listen-addr 0.0.0.0 --p2p-port 8000
# Let it generate genesis
# Then tar + scp the state to EU/ASIA
```

---

## Phase 2 — Deploy VPS Seed Validators

### On Each VPS: System Setup

Run on all 3 VPS (Ubuntu 22.04+):

```bash
# 1. Create system user
sudo groupadd -r moltchain
sudo useradd -r -g moltchain -d /home/moltchain -m -s /bin/false moltchain

# 2. Create directories
sudo mkdir -p /opt/moltchain/bin /var/lib/moltchain /var/log/moltchain /etc/moltchain
sudo chown moltchain:moltchain /var/lib/moltchain /var/log/moltchain

# 3. Install Rust & build (or scp pre-built binaries)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# Clone and build
git clone <your-repo> /tmp/moltchain-build
cd /tmp/moltchain-build
cargo build --release

# Copy binaries
sudo cp target/release/moltchain-validator /opt/moltchain/bin/
sudo cp target/release/moltchain-custody /opt/moltchain/bin/  # US VPS only
sudo cp target/release/moltchain-faucet /opt/moltchain/bin/   # US VPS only
sudo cp target/release/molt /opt/moltchain/bin/               # CLI (optional)
sudo chmod +x /opt/moltchain/bin/*

# 4. Extract genesis state
sudo mkdir -p /var/lib/moltchain/state-testnet
sudo tar xzf /tmp/genesis-state.tar.gz -C /var/lib/moltchain/state-testnet
sudo chown -R moltchain:moltchain /var/lib/moltchain
```

### On Each VPS: Validator Service

```bash
# Create env file
sudo tee /etc/moltchain/env-testnet <<'EOF'
MOLTCHAIN_NETWORK=testnet
MOLTCHAIN_RPC_PORT=8899
MOLTCHAIN_WS_PORT=8900
MOLTCHAIN_P2P_PORT=8000
MOLTCHAIN_SIGNER_BIND=0.0.0.0:9200
RUST_LOG=info
# MOLTCHAIN_ADMIN_TOKEN=<generate-with-openssl-rand-hex-32>
EOF
sudo chmod 600 /etc/moltchain/env-testnet
```

```bash
# Create systemd service
sudo tee /etc/systemd/system/moltchain-validator.service <<'EOF'
[Unit]
Description=MoltChain Validator Node
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=moltchain
Group=moltchain

ExecStart=/opt/moltchain/bin/moltchain-validator \
    --network testnet \
    --listen-addr 0.0.0.0 \
    --rpc-port 8899 \
    --ws-port 8900 \
    --p2p-port 8000 \
    --db-path /var/lib/moltchain/state-testnet \
    --bootstrap-peers seed-us.moltchain.network:8000,seed-eu.moltchain.network:8000,seed-ap.moltchain.network:8000

Restart=on-failure
RestartSec=5
LimitNOFILE=65536

# Security
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=/var/lib/moltchain /var/log/moltchain
PrivateTmp=true

# Environment
Environment=RUST_LOG=info
EnvironmentFile=/etc/moltchain/env-testnet

StandardOutput=journal
StandardError=journal
SyslogIdentifier=moltchain-validator

[Install]
WantedBy=multi-user.target
EOF
```

**Critical: `--listen-addr 0.0.0.0`** — Without this, the P2P layer binds to `127.0.0.1` and no external peers can connect. This flag was added specifically for production deployment.

### Start Order

1. **US VPS first** (has the genesis state with the original validator identity)
2. **EU VPS** (connects to US as bootstrap peer)
3. **ASIA VPS** (connects to US + EU)

```bash
# On each VPS:
sudo systemctl daemon-reload
sudo systemctl enable moltchain-validator
sudo systemctl start moltchain-validator

# Watch logs:
sudo journalctl -u moltchain-validator -f
```

The EU and ASIA validators will:
1. Connect to the seed peers via QUIC
2. Sync the genesis block from the existing chain
3. Auto-register as validators (they stake from their auto-generated identity)
4. Start producing blocks when their turn comes

### Verify Peering

```bash
# Check from any VPS
curl http://localhost:8899 -d '{"jsonrpc":"2.0","id":1,"method":"getValidators"}'
# Should show 3 validators
```

---

## Phase 3 — Custody Bridge

See [CUSTODY_DEPLOYMENT.md](./CUSTODY_DEPLOYMENT.md) for the full custody-specific guide. Summary:

### Deploy on US VPS Only

```bash
sudo tee /etc/systemd/system/moltchain-custody.service <<'EOF'
[Unit]
Description=MoltChain Custody Bridge
After=moltchain-validator.service
Wants=moltchain-validator.service

[Service]
Type=simple
User=moltchain
Group=moltchain
WorkingDirectory=/opt/moltchain

ExecStart=/opt/moltchain/bin/moltchain-custody
Restart=always
RestartSec=5

# Core
Environment=CUSTODY_DB_PATH=/var/lib/moltchain/custody-db
Environment=CUSTODY_POLL_INTERVAL_SECS=15
Environment=CUSTODY_DEPOSIT_TTL_SECS=86400
Environment=RUST_LOG=info

# MoltChain connection
Environment=CUSTODY_MOLT_RPC_URL=http://127.0.0.1:8899
Environment=CUSTODY_TREASURY_KEYPAIR=/var/lib/moltchain/state-testnet/genesis-keys/treasury-moltchain-testnet-1.json

# Wrapped token contracts (fill after deploying)
Environment=CUSTODY_MUSD_TOKEN_ADDR=<deploy-and-fill>
Environment=CUSTODY_WSOL_TOKEN_ADDR=<deploy-and-fill>
Environment=CUSTODY_WETH_TOKEN_ADDR=<deploy-and-fill>

# Solana bridge
Environment=CUSTODY_SOLANA_RPC_URL=https://api.mainnet-beta.solana.com
Environment=CUSTODY_TREASURY_SOLANA=<your-solana-address>
Environment=CUSTODY_SOLANA_FEE_PAYER=/etc/moltchain/solana-fee-payer.json

# Threshold signers (all 3 VPS)
Environment=CUSTODY_SIGNER_ENDPOINTS=http://<US_PRIVATE_IP>:9200,http://<EU_PRIVATE_IP>:9200,http://<ASIA_PRIVATE_IP>:9200

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable moltchain-custody
sudo systemctl start moltchain-custody
```

### Signer Network

The custody service calls each validator's threshold signer on port 9200. These must be reachable between the VPS nodes. Options:
- **WireGuard VPN** between the 3 VPS (recommended)
- **Firewall allow-list** — only allow port 9200 from the other 2 VPS IPs
- Never expose port 9200 to the public internet

---

## Phase 4 — Faucet & Frontend Services

### Faucet (US VPS, testnet only)

```bash
sudo tee /etc/systemd/system/moltchain-faucet.service <<'EOF'
[Unit]
Description=MoltChain Faucet
After=moltchain-validator.service

[Service]
Type=simple
User=moltchain
ExecStart=/opt/moltchain/bin/moltchain-faucet
Restart=always

Environment=PORT=8901
Environment=RPC_URL=http://127.0.0.1:8899
Environment=NETWORK=testnet
Environment=MAX_PER_REQUEST=10
Environment=DAILY_LIMIT_PER_IP=10
Environment=COOLDOWN_SECONDS=60
Environment=AIRDROPS_FILE=/var/lib/moltchain/airdrops.json

[Install]
WantedBy=multi-user.target
EOF
```

### Static Frontends (Website, Explorer, Wallet, Developers)

These are static HTML/JS files. Deploy them via Caddy's file server or a CDN:

```bash
# Copy static sites to the VPS
sudo mkdir -p /opt/moltchain/www/{website,explorer,wallet,developers,faucet-ui}
sudo cp -r website/* /opt/moltchain/www/website/
sudo cp -r explorer/* /opt/moltchain/www/explorer/
sudo cp -r wallet/* /opt/moltchain/www/wallet/
sudo cp -r developers/* /opt/moltchain/www/developers/
sudo cp -r faucet/*.html faucet/*.css faucet/*.js /opt/moltchain/www/faucet-ui/
```

---

## Phase 5 — Agent Validators Join

When an agent on a human's machine wants to run a validator:

### What the Agent Needs

1. The **MoltChain binary** (`moltchain-validator`)
2. The **seeds.json** file (or use `--bootstrap-peers` flag)
3. Enough MOLT to stake (10,000 MOLT minimum)

### Agent Start Command

```bash
# The agent runs this on the human's local machine:
./moltchain-validator \
    --network testnet \
    --bootstrap-peers seed-us.moltchain.network:8000,seed-eu.moltchain.network:8000,seed-ap.moltchain.network:8000
```

What happens:
1. Validator starts with `127.0.0.1` bind (local only — correct for home machines behind NAT)
2. Connects outbound to the seed VPS validators via QUIC
3. Syncs the full blockchain from peers
4. Auto-registers as a validator (if it has stake)
5. Gossip protocol propagates the peer list — it discovers other validators automatically
6. The `known-peers.json` file caches discovered peers for restart recovery

### Seed List: How It Works

The validator has two ways to discover peers:

1. **`--bootstrap-peers` CLI flag** — Explicit list of peers to connect to on startup
2. **`seeds.json` file** — JSON file with seed entries per network

The validator loads `seeds.json` from its working directory. It matches the `chain_id` from genesis config to determine which network's seeds to use.

If `--bootstrap-peers` is provided, the validator uses those AND ignores `seeds.json` (explicit overrides file-based seeds).

After connecting to at least one peer, the **gossip protocol** kicks in:
- Every 10 seconds, peers exchange their known peer lists
- New peers are added to the local `known-peers.json` durable store
- On restart, the validator loads `known-peers.json` and reconnects
- This means after the first successful connection, the agent doesn't need seeds anymore

### NAT / Home Network Considerations

Agent validators behind NAT can still:
- **Connect outbound** to seed nodes (QUIC works through most NATs)
- **Receive blocks, votes, and transactions** via the established QUIC connections
- **Produce blocks** when it's their turn

What they CAN'T do as a NAT'd node:
- Accept inbound connections from other peers (unless they open port 8000 on their router)
- Act as a full relay/seed (they're a "leaf" node)

This is fine — only the 3 VPS need to be fully reachable. Agent validators are consumers of the seed network.

---

## seeds.json — Production Version

Update `seeds.json` with your real VPS IPs/domains:

```json
{
  "testnet": {
    "network_id": "moltchain-testnet-1",
    "chain_id": "moltchain-testnet-1",
    "seeds": [
      {
        "id": "seed-us",
        "address": "seed-us.moltchain.network:8000",
        "region": "us-east",
        "operator": "MoltChain Foundation"
      },
      {
        "id": "seed-eu",
        "address": "seed-eu.moltchain.network:8000",
        "region": "eu-west",
        "operator": "MoltChain Foundation"
      },
      {
        "id": "seed-ap",
        "address": "seed-ap.moltchain.network:8000",
        "region": "ap-southeast",
        "operator": "MoltChain Foundation"
      }
    ],
    "bootstrap_peers": [
      "seed-us.moltchain.network:8000",
      "seed-eu.moltchain.network:8000",
      "seed-ap.moltchain.network:8000"
    ],
    "rpc_endpoints": [
      "https://rpc.moltchain.network"
    ]
  },
  "mainnet": {
    "network_id": "moltchain-mainnet-1",
    "chain_id": "moltchain-mainnet-1",
    "seeds": [],
    "bootstrap_peers": [],
    "rpc_endpoints": []
  }
}
```

**To add a new seed node later:** Just add it to `seeds.json`, publish the updated file, and all validators on next restart will auto-discover it.

---

## DNS Load Balancing

### Round-Robin (Free — Cloudflare Free Plan)

Multiple A records for the same subdomain:

```
rpc.moltchain.network  A  <US_IP>    TTL=300
rpc.moltchain.network  A  <EU_IP>    TTL=300
rpc.moltchain.network  A  <ASIA_IP>  TTL=300
```

DNS returns all 3 IPs in random order. Clients use whichever comes first. This provides:
- **Geographic diversity** — users get routed to closest responding server
- **Basic failover** — if one IP goes down, clients try the next one
- **Even distribution** — approximately 1/3 traffic to each

### Health-Check Load Balancing (Cloudflare Pro — $20/mo)

Add a Cloudflare Load Balancer on `rpc.moltchain.network` with:
- Pool: all 3 VPS IPs
- Health check: `GET /health` on port 443
- Steering: Geo (US→US VPS, EU→EU VPS, AP→ASIA VPS)
- Failover: If one VPS is down, route to nearest healthy one

This is the proper production setup but round-robin works fine to start.

---

## Reverse Proxy (Caddy)

Install Caddy on each VPS. It handles HTTPS automatically via Let's Encrypt.

### Install Caddy

```bash
sudo apt install -y debian-keyring debian-archive-keyring apt-transport-https curl
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | sudo tee /etc/apt/sources.list.d/caddy-stable.list
sudo apt update && sudo apt install caddy
```

### Caddyfile — US VPS (Full Stack)

```
# /etc/caddy/Caddyfile

# RPC
rpc.moltchain.network {
    reverse_proxy localhost:8899
}

# WebSocket
ws.moltchain.network {
    reverse_proxy localhost:8900
}

# Custody Bridge
custody.moltchain.network {
    reverse_proxy localhost:9105
}

# Faucet API
faucet.moltchain.network {
    # API
    handle /faucet/* {
        reverse_proxy localhost:8901
    }
    handle /health {
        reverse_proxy localhost:8901
    }
    # Static UI
    handle {
        root * /opt/moltchain/www/faucet-ui
        file_server
    }
}

# Explorer
explorer.moltchain.network {
    root * /opt/moltchain/www/explorer
    file_server
    try_files {path} /index.html
}

# Wallet
wallet.moltchain.network {
    root * /opt/moltchain/www/wallet
    file_server
    try_files {path} /index.html
}

# Developers
developers.moltchain.network {
    root * /opt/moltchain/www/developers
    file_server
    try_files {path} /index.html
}

# Main website
moltchain.network, www.moltchain.network {
    root * /opt/moltchain/www/website
    file_server
    try_files {path} /index.html
}
```

### Caddyfile — EU & ASIA VPS (Validator Only)

```
# /etc/caddy/Caddyfile

rpc.moltchain.network {
    reverse_proxy localhost:8899
}

ws.moltchain.network {
    reverse_proxy localhost:8900
}
```

```bash
sudo systemctl enable caddy
sudo systemctl restart caddy
```

---

## Firewall Rules

```bash
# On ALL VPS:
sudo ufw default deny incoming
sudo ufw default allow outgoing

# SSH
sudo ufw allow 22/tcp

# HTTPS (Caddy)
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp

# P2P (QUIC) — must be open for validators to connect
sudo ufw allow 8000/tcp
sudo ufw allow 8000/udp

# Threshold Signer — ONLY from other VPS IPs
sudo ufw allow from <US_VPS_IP> to any port 9200 proto tcp
sudo ufw allow from <EU_VPS_IP> to any port 9200 proto tcp
sudo ufw allow from <ASIA_VPS_IP> to any port 9200 proto tcp

# Enable
sudo ufw enable
```

**Never expose these to the public:**
- Port 9200 (threshold signer)
- Port 8899/8900 directly (should go through Caddy HTTPS)
- Port 9105 directly (custody, should go through Caddy)

---

## Monitoring

### Health Checks

```bash
# Validator
curl -s http://localhost:8899 -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | jq

# Custody
curl -s http://localhost:9105/health | jq

# Faucet
curl -s http://localhost:8901/health | jq

# Peer count
curl -s http://localhost:8899 -d '{"jsonrpc":"2.0","id":1,"method":"getValidators"}' | jq '.result | length'
```

### Simple Uptime Script

```bash
#!/bin/bash
# /opt/moltchain/bin/healthcheck.sh
# Run via cron every 5 minutes

RPC="http://localhost:8899"
HEALTH=$(curl -sf "$RPC" -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' 2>&1)

if [ $? -ne 0 ]; then
    echo "$(date) ALERT: Validator RPC not responding" >> /var/log/moltchain/healthcheck.log
    sudo systemctl restart moltchain-validator
fi

# Check slot is advancing
SLOT=$(curl -sf "$RPC" -d '{"jsonrpc":"2.0","id":1,"method":"getSlot"}' | jq -r '.result')
LAST_SLOT=$(cat /tmp/moltchain-last-slot 2>/dev/null || echo "0")
echo "$SLOT" > /tmp/moltchain-last-slot

if [ "$SLOT" = "$LAST_SLOT" ]; then
    echo "$(date) WARN: Slot not advancing (stuck at $SLOT)" >> /var/log/moltchain/healthcheck.log
fi
```

```bash
# Add to crontab
echo "*/5 * * * * /opt/moltchain/bin/healthcheck.sh" | sudo crontab -u moltchain -
```

---

## Backup & Recovery

### What to Back Up

| What | Where | Frequency | Critical? |
|---|---|---|---|
| Genesis keys | `/var/lib/moltchain/state-testnet/genesis-keys/` | Once (keep offline) | **YES** — controls treasury |
| Genesis wallet | `/var/lib/moltchain/state-testnet/genesis-wallet.json` | Once | YES |
| Blockchain state | `/var/lib/moltchain/state-testnet/` | Daily | Medium — can re-sync from peers |
| Custody DB | `/var/lib/moltchain/custody-db/` | Hourly | YES — deposit/withdrawal state |
| Airdrops file | `/var/lib/moltchain/airdrops.json` | Daily | Low |

### Backup Script

```bash
#!/bin/bash
# /opt/moltchain/bin/backup.sh
DATE=$(date +%Y%m%d-%H%M)
BACKUP_DIR="/opt/moltchain/backups"
mkdir -p "$BACKUP_DIR"

# Snapshot RocksDB (safe to copy while running)
tar czf "$BACKUP_DIR/state-$DATE.tar.gz" -C /var/lib/moltchain/state-testnet .
tar czf "$BACKUP_DIR/custody-$DATE.tar.gz" -C /var/lib/moltchain/custody-db . 2>/dev/null

# Keep last 7 days
find "$BACKUP_DIR" -name "*.tar.gz" -mtime +7 -delete
```

### Recovery: If a VPS Dies

1. Spin up a new VPS
2. Install binaries (or build from source)
3. Either:
   - **Restore from backup:** Extract state tarball to `/var/lib/moltchain/state-testnet/`
   - **Sync from peers:** Start with empty state + `--bootstrap-peers <other-2-VPS>`
4. Update DNS to point to new IP
5. Update `seeds.json` if IP changed

---

## Troubleshooting

### Validators can't find each other

```bash
# Check P2P is listening on 0.0.0.0 (not 127.0.0.1)
sudo ss -tlnp | grep 8000
# Should show: 0.0.0.0:8000

# If it shows 127.0.0.1:8000, add --listen-addr 0.0.0.0 to the systemd service
```

### Blocks not producing

```bash
# Check validator count
curl -s localhost:8899 -d '{"jsonrpc":"2.0","id":1,"method":"getValidators"}' | jq '.result | length'

# Check slot is advancing
curl -s localhost:8899 -d '{"jsonrpc":"2.0","id":1,"method":"getSlot"}'

# Check logs for errors
journalctl -u moltchain-validator --since "10 min ago" | grep -i error
```

### Different genesis between nodes

If validators have conflicting genesis blocks, they won't peer. Make sure:
1. All VPS started from the same state snapshot
2. All use the same `--network testnet` flag
3. The `chain_id` in genesis matches

### Custody signer errors

```bash
# Check custody can reach signers
curl http://localhost:9105/status
# "signers.configured" should be 3

# Test signer connectivity manually
curl -s http://<OTHER_VPS_IP>:9200/health
```

---

## Full Startup Checklist

```
[ ] 1. Build binaries: cargo build --release
[ ] 2. Generate genesis locally (or on US VPS)
[ ] 3. Buy 3 VPS (EU/US/ASIA) — Ubuntu 22.04, 4+ CPU, 8+ GB RAM
[ ] 4. Set up DNS records on Cloudflare (see DNS section)
[ ] 5. Run system setup on all 3 VPS (user, dirs, binaries)
[ ] 6. Copy genesis state to all 3 VPS
[ ] 7. Delete signer-keypair.json on EU/ASIA (they generate their own)
[ ] 8. Install Caddy on all 3 VPS + configure Caddyfiles
[ ] 9. Configure firewall (ufw) on all 3 VPS
[ ] 10. Start validator on US VPS first → verify blocks producing
[ ] 11. Start validator on EU VPS → verify it peers with US
[ ] 12. Start validator on ASIA VPS → verify 3 validators in getValidators
[ ] 13. Deploy custody service on US VPS (after deploying wrapped token contracts)
[ ] 14. Deploy faucet on US VPS (testnet only)
[ ] 15. Deploy static sites (website, explorer, wallet, developers)
[ ] 16. Update seeds.json with real IPs/domains
[ ] 17. Test agent connection from local machine:
        ./moltchain-validator --bootstrap-peers seed-us.moltchain.network:8000
[ ] 18. Set up backup cron jobs
[ ] 19. Set up health check monitoring
[ ] 20. Copy genesis-keys/ to a secure offline location
```

---

## VPS Recommendations

| Region | Provider | Spec | Cost/mo |
|---|---|---|---|
| US (NYC) | DigitalOcean / Hetzner | 4 vCPU, 8GB RAM, 160GB NVMe | ~$24-48 |
| EU (Frankfurt) | Hetzner | 4 vCPU, 8GB RAM, 160GB NVMe | ~$24 |
| ASIA (Singapore) | DigitalOcean / Vultr | 4 vCPU, 8GB RAM, 160GB NVMe | ~$24-48 |

RocksDB benefits strongly from NVMe — avoid HDD-based VPS. 4 vCPU is plenty for current throughput. Scale up to 8 vCPU when transaction volume grows.
