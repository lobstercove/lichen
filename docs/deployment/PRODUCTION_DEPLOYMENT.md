# MoltChain Production Deployment — Complete Strategy

> Full step-by-step guide for deploying MoltChain across seed + relay infrastructure with 3 to 6 validators using the `moltchain.network` domain. Covers genesis bootstrapping, DNS, seed discovery, RPC/WS rotation, custody bridge, faucet, and how agent-run validators connect.

**Release-ready status (Feb 24, 2026):** main validated and published at commit `4059403`.

---

## Table of Contents

1. [Binary Inventory](#binary-inventory)
2. [Network Architecture](#network-architecture)
3. [Operator Runbook — Relay/Seed Topology (3 → 6 Validators)](#operator-runbook--relayseed-topology-3--6-validators)
3. [DNS & Subdomains](#dns--subdomains)
4. [Port Map](#port-map)
5. [Phase 1 — Genesis on Local Machine](#phase-1--genesis-on-local-machine)
6. [Phase 2 — Deploy VPS Seed Validators](#phase-2--deploy-vps-seed-validators)
7. [Phase 3 — Custody Bridge](#phase-3--custody-bridge)
8. [Phase 4 — Faucet Service](#phase-4--faucet-service)
9. [Phase 5 — Cloudflare Pages (Static Portals)](#phase-5--cloudflare-pages-static-portals)
10. [Phase 6 — Agent Validators Join](#phase-6--agent-validators-join)
11. [seeds.json — Production Version](#seedsjson--production-version)
12. [DNS Load Balancing](#dns-load-balancing)
13. [Reverse Proxy (Caddy)](#reverse-proxy-caddy)
14. [Firewall Rules](#firewall-rules)
15. [Monitoring](#monitoring)
16. [Backup & Recovery](#backup--recovery)
17. [Troubleshooting](#troubleshooting)
18. [Clean Slate Deployment Runbook](#clean-slate-deployment-runbook)
19. [Release Signing Setup](#release-signing-setup)
20. [Creating a Release](#creating-a-release)
21. [Auto-Update System](#auto-update-system)
22. [Validator CLI Reference](#validator-cli-reference)
23. [Admin Key Management Lifecycle](#admin-key-management-lifecycle)
24. [Environment Command Matrix](#environment-command-matrix)

---

## Binary Inventory

MoltChain compiles into **4 separate binaries** from the workspace:

| Binary | Crate | Port | Runs On | Description |
|---|---|---|---|---|
| `moltchain-validator` | `validator` | P2P: 8000, RPC: 8899, WS: 8900, Signer: 9200 | Every VPS | Validator + built-in RPC + WebSocket + threshold signer. **This is the main binary.** |
| `moltchain-custody` | `custody` | 9105 | Seed VPS only (1 instance) | Bridge service for Solana/Ethereum ↔ MoltChain deposits & withdrawals |
| `moltchain-faucet` | `faucet` | 9100 | Seed VPS only (testnet) | MOLT faucet for testnet. Refuses to run on mainnet. |
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

### VPS Source Staging Rule

When staging source to a VPS for an in-place rebuild, do not use `git archive` for this repo.

- This repository intentionally ignores local-only directories such as `genesis/`, `docs/`, and the static portals.
- The validator build and deployment workflow can still depend on ignored source directories being present on the build host.
- For VPS rebuilds, stage the workspace with a full tarball copy from the local machine, excluding only heavy machine-local artifacts such as `.git/`, `target/`, `data/`, `logs/`, `node_modules/`, and `dist/`.
- If you stage with `git archive`, the VPS tree will be incomplete and rebuilds can fail because ignored source folders are missing.

Reference pattern:

```bash
tar -cf - \
  --exclude='.git' \
  --exclude='target' \
  --exclude='compiler/target' \
  --exclude='data' \
  --exclude='logs' \
  --exclude='node_modules' \
  --exclude='dist' \
  . | ssh <host> 'rm -rf ~/moltchain-build && mkdir -p ~/moltchain-build && tar -xf - -C ~/moltchain-build'
```

---

## Network Architecture

```
┌───────────────────────────────────────────────────────────────────┐
│                      moltchain.network DNS                        │
│                                                                   │
│  BACKEND (VPS)                                                    │
│  ──────────────────────────────────────────────────────────       │
│  rpc.moltchain.network          →  Round-robin to all 3 VPS      │
│  ws.moltchain.network           →  Round-robin to all 3 VPS      │
│  seed-eu.moltchain.network      →  EU VPS (direct IP)            │
│  seed-us.moltchain.network      →  US VPS (direct IP)            │
│  seed-ap.moltchain.network      →  ASIA VPS (direct IP)          │
│  custody.moltchain.network      →  US VPS                        │
│  faucet.moltchain.network       →  US VPS (testnet only)         │
│                                                                   │
│  STATIC PORTALS (Cloudflare Pages — global CDN edge)              │
│  ──────────────────────────────────────────────────────────       │
│  moltchain.network              →  CF Pages (main website)        │
│  explorer.moltchain.network     →  CF Pages                      │
│  wallet.moltchain.network       →  CF Pages                      │
│  dex.moltchain.network          →  CF Pages                      │
│  marketplace.moltchain.network  →  CF Pages                      │
│  programs.moltchain.network     →  CF Pages                      │
│  developers.moltchain.network   →  CF Pages                      │
│  monitoring.moltchain.network   →  CF Pages                      │
└───────────────────────────────────────────────────────────────────┘

┌ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┐
│  Cloudflare Pages (300+ edge locations)                         │
│  Static HTML/CSS/JS — auto-deploy on git push                   │
│  website · explorer · wallet · dex · marketplace                │
│  programs · developers · monitoring                             │
└ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┘
        ▲ users worldwide (~20ms latency)
        │
┌────────────────────┐    ┌────────────────────┐    ┌────────────────────┐
│   EU VPS           │    │   US VPS (SEED)    │    │   ASIA VPS         │
│   Frankfurt        │    │   New York         │    │   Singapore        │
│                    │    │                    │    │                    │
│ Validator  :7001/:8001│◄──►│ Validator  :7001/:8001│◄──►│ Validator  :7001/:8001│
│ RPC        :8899   │    │ RPC        :8899   │    │ RPC        :8899   │
│ WebSocket  :8900   │    │ WebSocket  :8900   │    │ WebSocket  :8900   │
│ Signer     :9200   │    │ Signer     :9200   │    │ Signer     :9200   │
│                    │    │ Custody    :9105   │    │                    │
│                    │    │ Faucet     :9100   │    │                    │
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

        ## Operator Runbook — Relay/Seed Topology (3 → 6 Validators)

        This is the recommended production topology when you start at 3 validators and plan to scale to 6.

        ### Node roles

        | Role | Count (start → scale) | Purpose | Public surface |
        |---|---:|---|---|
        | Seed validators | 3 → 6 | Consensus + bootstrap peers | `seed-*.moltchain.network:8001` |
        | RPC relays | 1 → 2 | Stable HTTPS/WSS front door + upstream rotation | `rpc.moltchain.network`, `ws.moltchain.network` |
        | Custody host | 1 | Custody service + workers | `custody.moltchain.network` |
        | Faucet host (testnet) | 1 | Test token distribution | `faucet.moltchain.network` |

        ### Phased rollout (exact order)

        1. **Provision VPS + DNS skeleton**
          - reserve subdomains for `seed-01..seed-06`, `rpc-relay-01`, `rpc-relay-02`, `rpc`, `ws`, `custody`, `faucet`
        2. **Bootstrap genesis on seed-01**
          - generate initial state, verify chain health, export seeds list
        3. **Join seed-02 and seed-03**
          - copy state snapshot, regenerate node-specific keys, start validator with bootstrap peers
        4. **Deploy relay-01**
          - configure Caddy upstream pool to seed validators for `/` (RPC) and WS endpoint rotation
        5. **Switch public client traffic to relay**
          - set `rpc.moltchain.network` and `ws.moltchain.network` to relay host(s) instead of direct validator round-robin
        6. **Scale to seed-04..seed-06**
          - add new validators, update relay upstream pools, update `seeds.json`
        7. **Add relay-02 for HA**
          - run same config as relay-01 and enable DNS/LB failover across relays

        ### Why relay-first for RPC rotation

        - validators keep stable consensus ports while relay absorbs public traffic spikes
        - upstream health checks remove unhealthy validator RPC endpoints automatically
        - clients keep one canonical URL (`rpc.moltchain.network`) while backend rotation changes without client config churn

        ### Seed vs relay traffic policy

        - **P2P bootstrap**: always direct to `seed-*` records on port `8000`
        - **Wallet/app/agent RPC**: route via relays (`rpc.moltchain.network`)
        - **WS subscriptions**: route via relays (`ws.moltchain.network`) where possible; client-side fallback list remains recommended

        ### Step-by-step checklist (operator execution)

        ```bash
        # 1) Verify all validators are healthy
        for h in seed-01.moltchain.network seed-02.moltchain.network seed-03.moltchain.network; do
          curl -sS -X POST "https://$h" -H 'Content-Type: application/json' \
           -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'
        done

        # 2) Verify relay front door health
        curl -sS -X POST https://rpc.moltchain.network -H 'Content-Type: application/json' \
          -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'

        # 3) Verify WS endpoint is reachable (handshake check)
        curl -i https://ws.moltchain.network || true
        ```

        When adding seed-04..seed-06, update all relay upstream pools first, then update DNS and `seeds.json`.

---

## DNS & Subdomains

### Cloudflare DNS Records (moltchain.network)

Set these up on your DNS provider. I recommend Cloudflare for geo-based load balancing.

### Backend Services (A records → VPS)

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

### Recommended DNS layout for 3 → 6 validators + relays

Use stable relay hostnames as the only public RPC entrypoint; keep validator hostnames explicit for operational control.

| Record | Example | Purpose |
|---|---|---|
| `seed-01..seed-06` (A, DNS only) | `seed-01.moltchain.network` | P2P bootstrap + direct validator diagnostics |
| `rpc-relay-01` / `rpc-relay-02` (A, proxied) | `rpc-relay-01.moltchain.network` | RPC relay frontends |
| `ws-relay-01` / `ws-relay-02` (A, DNS only or proxied per plan) | `ws-relay-01.moltchain.network` | WS relay frontends |
| `rpc` (CNAME/LB) | `rpc.moltchain.network -> rpc-relay-*` | Canonical client RPC URL |
| `ws` (CNAME/LB) | `ws.moltchain.network -> ws-relay-*` | Canonical client WS URL |

Operational rule: do not point `rpc`/`ws` directly at every validator once relay is in place; keep relays as the control plane for rotation.

### Static Portals (CNAME → Cloudflare Pages)

All frontend portals are **pure static HTML/CSS/JS** — no server process required. Deploy them to **Cloudflare Pages** for global CDN edge delivery (~20 ms worldwide), zero maintenance, automatic HTTPS, and no VPS dependency.

| Type | Name | Value | Proxy | Notes |
|---|---|---|---|---|
| **CNAME** | `@` | `moltchain-website.pages.dev` | Proxied | Main website |
| **CNAME** | `www` | `moltchain.network` | Proxied | www redirect |
| **CNAME** | `explorer` | `moltchain-explorer.pages.dev` | Proxied | Block explorer |
| **CNAME** | `wallet` | `moltchain-wallet.pages.dev` | Proxied | Web wallet |
| **CNAME** | `dex` | `moltchain-dex.pages.dev` | Proxied | ClawSwap DEX |
| **CNAME** | `marketplace` | `moltchain-marketplace.pages.dev` | Proxied | NFT / skill marketplace |
| **CNAME** | `programs` | `moltchain-programs.pages.dev` | Proxied | Programs IDE |
| **CNAME** | `developers` | `moltchain-developers.pages.dev` | Proxied | Developer portal & docs |
| **CNAME** | `monitoring` | `moltchain-monitoring.pages.dev` | Proxied | Public network dashboard |

> **Why Cloudflare Pages over VPS?**
> - Free tier, unlimited bandwidth, 500 builds/month
> - Served from 300+ edge locations (vs 1 VPS in NYC)
> - Auto-deploy on `git push` — no SSH/scp needed
> - If the VPS goes down, all portals stay live (they don't depend on VPS)
> - Automatic HTTPS, HTTP/3, Brotli compression

**Key points:**
- `seed-*.moltchain.network` must be **DNS only** (gray cloud) — P2P QUIC needs direct IP, not Cloudflare proxy
- `ws.moltchain.network` must be **DNS only** — Cloudflare free plan doesn't support arbitrary WebSocket
- `rpc.moltchain.network` can be **Proxied** (orange cloud) — standard HTTPS works through CF
- All portal CNAMEs point to `*.pages.dev` — Cloudflare auto-provisions SSL for the custom domain
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
| Faucet API | **9100** | TCP | YES — via Caddy → `faucet.moltchain.network` |
| Caddy (HTTPS) | **443** | TCP | YES |
| Caddy (HTTP→HTTPS) | **80** | TCP | YES (redirect only) |

---

## Day 0 Execution Sheet (copy/paste runbook)

Use this section as your first production worksheet before you own the VPS. Keep naming stable now so every future script/config matches.

### 0.1 Canonical node inventory (recommended naming)

| Role | Hostname | Public DNS | Private DNS (optional) | Example IP placeholder |
|---|---|---|---|---|
| Seed validator 1 (genesis) | `seed-01` | `seed-01.moltchain.network` | `seed-01.internal.moltchain.network` | `203.0.113.11` |
| Seed validator 2 | `seed-02` | `seed-02.moltchain.network` | `seed-02.internal.moltchain.network` | `203.0.113.12` |
| Seed validator 3 | `seed-03` | `seed-03.moltchain.network` | `seed-03.internal.moltchain.network` | `203.0.113.13` |
| Seed validator 4 (future) | `seed-04` | `seed-04.moltchain.network` | `seed-04.internal.moltchain.network` | `203.0.113.14` |
| Seed validator 5 (future) | `seed-05` | `seed-05.moltchain.network` | `seed-05.internal.moltchain.network` | `203.0.113.15` |
| Seed validator 6 (future) | `seed-06` | `seed-06.moltchain.network` | `seed-06.internal.moltchain.network` | `203.0.113.16` |
| RPC relay 1 | `relay-01` | `rpc-relay-01.moltchain.network` | `relay-01.internal.moltchain.network` | `198.51.100.21` |
| RPC relay 2 (HA) | `relay-02` | `rpc-relay-02.moltchain.network` | `relay-02.internal.moltchain.network` | `198.51.100.22` |
| Custody + faucet host | `custody-01` | `custody.moltchain.network` / `faucet.moltchain.network` | `custody-01.internal.moltchain.network` | `198.51.100.31` |

### 0.2 Canonical public entrypoints

| Service | Canonical URL | Backing nodes |
|---|---|---|
| JSON-RPC | `https://rpc.moltchain.network` | `relay-01`, `relay-02` |
| WebSocket | `wss://ws.moltchain.network` | `relay-01`, `relay-02` |
| Custody API | `https://custody.moltchain.network` | `custody-01` |
| Faucet (testnet) | `https://faucet.moltchain.network` | `custody-01` |

### 0.3 Variable block (fill once, reuse everywhere)

```bash
# Domain
export DOMAIN="moltchain.network"

# Seed validators (public IP placeholders)
export SEED01_IP="203.0.113.11"
export SEED02_IP="203.0.113.12"
export SEED03_IP="203.0.113.13"
export SEED04_IP="203.0.113.14"
export SEED05_IP="203.0.113.15"
export SEED06_IP="203.0.113.16"

# Relays
export RELAY01_IP="198.51.100.21"
export RELAY02_IP="198.51.100.22"

# Custody/faucet host
export CUSTODY01_IP="198.51.100.31"

# Hostnames
export SEED01_HOST="seed-01.${DOMAIN}"
export SEED02_HOST="seed-02.${DOMAIN}"
export SEED03_HOST="seed-03.${DOMAIN}"
export SEED04_HOST="seed-04.${DOMAIN}"
export SEED05_HOST="seed-05.${DOMAIN}"
export SEED06_HOST="seed-06.${DOMAIN}"
export RELAY01_HOST="rpc-relay-01.${DOMAIN}"
export RELAY02_HOST="rpc-relay-02.${DOMAIN}"
export RPC_HOST="rpc.${DOMAIN}"
export WS_HOST="ws.${DOMAIN}"
export CUSTODY_HOST="custody.${DOMAIN}"
export FAUCET_HOST="faucet.${DOMAIN}"
```

### 0.4 DNS records to create Day 0

Create these first, before server hardening and service startup.

```text
# Seeds (DNS only)
seed-01  A  203.0.113.11
seed-02  A  203.0.113.12
seed-03  A  203.0.113.13
seed-04  A  203.0.113.14
seed-05  A  203.0.113.15
seed-06  A  203.0.113.16

# Relays
rpc-relay-01  A  198.51.100.21
rpc-relay-02  A  198.51.100.22

# Canonical client endpoints -> relays
rpc  CNAME  rpc-relay-01.moltchain.network  (or LB pool relay-01/02)
ws   CNAME  rpc-relay-01.moltchain.network  (or LB pool relay-01/02)

# Custody/faucet
custody  A  198.51.100.31
faucet   A  198.51.100.31
```

### 0.5 Bootstrap order (operator timeline)

1. provision `seed-01`, `seed-02`, `seed-03`, `relay-01`, `custody-01`
2. create DNS records from section 0.4
3. run genesis on `seed-01`; snapshot state
4. restore state on `seed-02` + `seed-03`, regenerate per-node identity/signer keys
5. bring up relay-01 with upstreams `seed-01..03`
6. verify `https://rpc.moltchain.network` health
7. deploy custody/faucet on `custody-01`
8. scale with `seed-04..06` + `relay-02`, then update relay upstream pools

### 0.6 Day 0 verification commands

```bash
curl -sS -X POST "https://rpc.moltchain.network" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'

curl -sS "https://custody.moltchain.network/health"
curl -sS "https://faucet.moltchain.network/health"

for host in seed-01.moltchain.network seed-02.moltchain.network seed-03.moltchain.network; do
  curl -sS -X POST "https://$host" -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'
done
```

---

## Phase 1 — Genesis on Local Machine

The simplest approach: generate genesis locally, copy state to VPS.

### Step 1: Generate Genesis Locally

```bash
cd moltchain
cargo build --release

# Start the first validator — it auto-generates genesis
./target/release/moltchain-validator --network testnet --p2p-port 7001
```

On first boot with no existing state, the validator:
1. Generates a **genesis wallet** with multi-sig (2/3 for testnet, 3/5 for mainnet)
2. Creates treasury keypairs in `./data/state-7001/genesis-keys/`
3. Saves `genesis-wallet.json` in the state directory
4. Mints 1 billion MOLT to the treasury
5. Registers itself as the initial validator
6. **Auto-deploys all 30 genesis contracts** from the `contracts/` directory

Step 6 is important — the validator binary includes a `genesis_auto_deploy` function that reads every compiled `.wasm` from the `contracts/` directory and deploys them into chain state on the first block. This means **no separate deploy script is needed**. The full catalog:

| Category | Contracts deployed |
|---|---|
| Core token | MOLT (MoltCoin) |
| Wrapped tokens | MUSD, WSOL, WETH |
| DEX | DEX Core, AMM, Router, Margin, Rewards, Governance, Analytics |
| DeFi | MoltSwap, MoltBridge, LobsterLend |
| Marketplace | MoltMarket, MoltAuction, MoltOracle, MoltDAO |
| NFT / Identity | MoltPunks, MoltyID |
| Infrastructure | ClawPay, ClawPump, ClawVault, BountyBoard, Compute Market, Reef Storage |

Contract addresses are derived deterministically: `SHA-256(deployer_pubkey + dir_name + wasm_bytes)`. The deploy is **idempotent** — if contracts already exist in state, they're skipped.

> **Prerequisite:** The `contracts/` directory with compiled `.wasm` files must be present in the working directory where the validator starts. On VPS, either build locally or include the WASM files in the state tarball.

**Let it run for ~30 seconds** to produce a few blocks, then stop it with Ctrl+C.

### Step 2: Copy Genesis State to VPS

```bash
# Stop the local validator first (Ctrl+C)

# The state directory contains everything:
ls ./data/state-testnet/
# → genesis-keys/        (CRITICAL - treasury keypairs)
# → genesis-wallet.json  (treasury wallet metadata)
# → known-peers.json     (peer cache)
# → signer-keypair.json  (threshold signer key)
# → <RocksDB files>      (blockchain state)

# Archive it
tar czf genesis-state.tar.gz -C ./data/state-testnet .

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
./moltchain-validator --network testnet --listen-addr 0.0.0.0 --p2p-port 7001
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
MOLTCHAIN_P2P_PORT=7001
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
    --p2p-port 7001 \
    --db-path /var/lib/moltchain/state-testnet \
    --bootstrap-peers seed-us.moltchain.network:7001,seed-eu.moltchain.network:7001,seed-ap.moltchain.network:7001

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
# CRITICAL: This keypair must match the one used to deploy_dex.py (the "admin" key).
# Copy keypairs/deployer.json to this path after deployment.
Environment=CUSTODY_MOLT_RPC_URL=http://127.0.0.1:8899
Environment=CUSTODY_TREASURY_KEYPAIR=/etc/moltchain/custody-treasury-testnet.json

# Wrapped token contracts (auto-discovered from registry, or pin manually)
Environment=CUSTODY_MUSD_TOKEN_ADDR=<deploy-and-fill>
Environment=CUSTODY_WSOL_TOKEN_ADDR=<deploy-and-fill>
Environment=CUSTODY_WETH_TOKEN_ADDR=<deploy-and-fill>
Environment=CUSTODY_WBNB_TOKEN_ADDR=<deploy-and-fill>

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

## Phase 4 — Faucet Service

### Faucet (US VPS, testnet only)

The faucet is the only "frontend" that runs as a server process (Rust binary). It serves both the API and its own static UI.

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

The faucet UI files should still be copied to the VPS since Caddy serves them alongside the API:

```bash
sudo mkdir -p /opt/moltchain/www/faucet-ui
sudo cp -r faucet/*.html faucet/*.css faucet/*.js /opt/moltchain/www/faucet-ui/ 2>/dev/null
```

---

## Phase 5 — Cloudflare Pages (Static Portals)

All frontend portals (website, explorer, wallet, DEX, marketplace, programs, developers, monitoring) are **pure static HTML/CSS/JS**. Instead of hosting them on the VPS, deploy them to **Cloudflare Pages** for global edge delivery.

### Why Cloudflare Pages

| | VPS (Caddy file_server) | Cloudflare Pages |
|---|---|---|
| Latency for Asia users | ~200 ms to NYC | ~20 ms to nearest edge |
| VPS goes down | All portals down | Portals stay live |
| SSL certificates | Caddy manages Let's Encrypt | Automatic, zero config |
| Deploy workflow | `scp` files to VPS via SSH | `git push` (auto-deploy) |
| Cost | Uses VPS bandwidth | Free (unlimited sites, 500 builds/mo) |
| Scaling | Limited to VPS capacity | Unlimited (Cloudflare CDN) |

### Step 1: Create Cloudflare Pages Projects

In the Cloudflare dashboard → **Workers & Pages** → **Create**:

| Project name | Git repo | Build output directory | Custom domain |
|---|---|---|---|
| `moltchain-website` | `MoltChain/moltchain` | `website` | `moltchain.network` |
| `moltchain-explorer` | `MoltChain/moltchain` | `explorer` | `explorer.moltchain.network` |
| `moltchain-wallet` | `MoltChain/moltchain` | `wallet` | `wallet.moltchain.network` |
| `moltchain-dex` | `MoltChain/moltchain` | `dex` | `dex.moltchain.network` |
| `moltchain-marketplace` | `MoltChain/moltchain` | `marketplace` | `marketplace.moltchain.network` |
| `moltchain-programs` | `MoltChain/moltchain` | `programs` | `programs.moltchain.network` |
| `moltchain-developers` | `MoltChain/moltchain` | `developers` | `developers.moltchain.network` |
| `moltchain-monitoring` | `MoltChain/moltchain` | `monitoring` | `monitoring.moltchain.network` |

For each project:

1. **Connect to Git** → Select the `MoltChain/moltchain` repo
2. **Framework preset** → `None` (these are plain static files, no build step)
3. **Build command** → *(leave empty)*
4. **Build output directory** → Set to the subdirectory (e.g. `wallet`)
5. **Deploy** → Cloudflare builds and publishes to `<project>.pages.dev`

### Step 2: Add Custom Domains

For each Pages project, go to **Custom domains** → **Set up a custom domain** → enter the subdomain (e.g. `wallet.moltchain.network`). Cloudflare automatically:
- Creates the CNAME DNS record
- Provisions an SSL certificate
- Routes traffic to the Pages deployment

If you prefer to set DNS manually:

```
# In Cloudflare DNS → moltchain.network zone
CNAME  explorer     moltchain-explorer.pages.dev     (Proxied)
CNAME  wallet       moltchain-wallet.pages.dev       (Proxied)
CNAME  dex          moltchain-dex.pages.dev          (Proxied)
CNAME  marketplace  moltchain-marketplace.pages.dev  (Proxied)
CNAME  programs     moltchain-programs.pages.dev     (Proxied)
CNAME  developers   moltchain-developers.pages.dev   (Proxied)
CNAME  monitoring   moltchain-monitoring.pages.dev   (Proxied)
```

For the apex domain (`moltchain.network`):
```
# Cloudflare supports CNAME flattening at the apex
CNAME  @            moltchain-website.pages.dev      (Proxied)
CNAME  www          moltchain.network                (Proxied)
```

### Step 3: Auto-Deploy on Git Push

Once connected, every push to `main` triggers a rebuild of all Pages projects. Cloudflare detects which files changed and only rebuilds affected projects.

To deploy manually (e.g. from CI or local):

```bash
# Install Wrangler CLI
npm install -g wrangler

# Authenticate (one-time)
wrangler login

# Deploy a specific portal
wrangler pages deploy wallet/ --project-name=moltchain-wallet
wrangler pages deploy explorer/ --project-name=moltchain-explorer
wrangler pages deploy website/ --project-name=moltchain-website
# ... etc for each portal
```

### Step 4: Verify Deployments

After deploying, each portal is available at both the `*.pages.dev` URL and the custom domain:

```bash
# Check Pages URLs (available immediately)
curl -sI https://moltchain-wallet.pages.dev | head -3
curl -sI https://moltchain-explorer.pages.dev | head -3

# Check custom domains (may take a few minutes for DNS + SSL)
curl -sI https://wallet.moltchain.network | head -3
curl -sI https://explorer.moltchain.network | head -3
```

### Frontend Configuration Checklist

All portal frontends have RPC/WS endpoint configuration that defaults to `localhost`. Before deploying to production, verify the network configs point to live URLs. Most apps have a multi-network config pattern (stored in `localStorage`) that already includes the production URLs.

| Portal | Config file | Has multi-network? | Production URLs in config? | Notes |
|---|---|---|---|---|
| **wallet** | `wallet/js/wallet.js` | YES | YES (`rpc.moltchain.network`) | Gold standard — also has custody endpoints |
| **explorer** | `explorer/js/explorer.js` | YES | YES | `transaction.js` L110 has hardcoded faucet URL — needs fix |
| **marketplace** | `marketplace/js/marketplace-config.js` | YES | YES | Clean — mirrors wallet pattern |
| **website** | `website/script.js` | YES | YES | Works, minor local-mainnet port mismatch |
| **monitoring** | `monitoring/js/monitoring.js` | YES | YES (RPC) | `VALIDATOR_RPCS` array is hardcoded to local ports — update for prod |
| **programs** | `programs/js/moltchain-sdk.js` | YES | YES | `landing.js` only has 2-way auto-detect (local vs testnet) |
| **dex** | `dex/dex.js` | PARTIAL | NO | Uses `window.MOLTCHAIN_RPC` override — network selector not wired up |
| **faucet UI** | `faucet/faucet.js` | NO | NO | Hardcoded `localhost:9100` — needs full rewrite for prod |
| **shared** | `shared/wallet-connect.js` | Delegates | — | Fallback port `9000` should be `8899` |

**Config fixes needed before production:**

1. `faucet/faucet.js` — Change `FAUCET_API` from `http://localhost:9100` to auto-detect (`/faucet` relative path when served by Caddy, or `https://faucet.moltchain.network`)
2. `dex/dex.js` — Wire up the existing `<select id="networkSelect">` to switch `MOLTCHAIN_RPC`/`MOLTCHAIN_WS`
3. `monitoring/js/monitoring.js` — Make `VALIDATOR_RPCS` configurable per-network (seed-us/eu/ap for prod)
4. `explorer/js/transaction.js` L110 — Replace hardcoded `localhost:9100` faucet URL
5. `programs/js/landing.js` — Add mainnet to the auto-detect (currently only local vs testnet)
6. `shared/wallet-connect.js` — Fix fallback port from `9000` to `8899`

---

## Phase 6 — Agent Validators Join

When an agent on a human's machine wants to run a validator:

### What the Agent Needs

1. The **MoltChain binary** (`moltchain-validator`)
2. The **seeds.json** file (or use `--bootstrap-peers` flag)
3. Enough MOLT to stake (100,000 MOLT minimum)

### Agent Start Command

```bash
# The agent runs this on the human's local machine:
./moltchain-validator \
    --network testnet \
    --bootstrap-peers seed-us.moltchain.network:8001,seed-eu.moltchain.network:8001,seed-ap.moltchain.network:8001
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
        "address": "seed-us.moltchain.network:8001",
        "region": "us-east",
        "operator": "MoltChain Foundation"
      },
      {
        "id": "seed-eu",
        "address": "seed-eu.moltchain.network:8001",
        "region": "eu-west",
        "operator": "MoltChain Foundation"
      },
      {
        "id": "seed-ap",
        "address": "seed-ap.moltchain.network:8001",
        "region": "ap-southeast",
        "operator": "MoltChain Foundation"
      }
    ],
    "bootstrap_peers": [
      "seed-us.moltchain.network:8001",
      "seed-eu.moltchain.network:8001",
      "seed-ap.moltchain.network:8001"
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

### RPC rotation model (recommended)

For 3+ validators, use **two-layer rotation**:

1. **Layer 1 (public):** `rpc.moltchain.network` load-balances across `rpc-relay-01` and `rpc-relay-02`
2. **Layer 2 (inside relay):** each relay load-balances across validator RPC upstreams (`seed-01..seed-06`)

Benefits:

- fast failover if a validator RPC stalls
- no client reconfiguration when adding/removing validators
- easier maintenance windows (drain a single upstream)

### Client-side fallback rotation (agents/wallets)

Even with relay, keep explicit fallback endpoints in clients:

```bash
RPC_ENDPOINTS=(
  "https://rpc.moltchain.network"
  "https://rpc-relay-01.moltchain.network"
  "https://rpc-relay-02.moltchain.network"
  "https://seed-01.moltchain.network"
)

for rpc in "${RPC_ENDPOINTS[@]}"; do
  if curl -fsS -X POST "$rpc" -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}' >/dev/null; then
    export RPC_URL="$rpc"
    break
  fi
done
```

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

### Caddyfile — US VPS (Backend Services Only)

Since all static portals are served by Cloudflare Pages, the US VPS Caddy only handles **backend services** (RPC, WebSocket, custody, faucet):

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

# Faucet API + UI
faucet.moltchain.network {
    # API routes
    handle /faucet/* {
        reverse_proxy localhost:9100
    }
    handle /health {
        reverse_proxy localhost:9100
    }
    # Static UI (faucet is special — served from VPS because it needs its API co-located)
    handle {
        root * /opt/moltchain/www/faucet-ui
        file_server
    }
}
```

> **Note:** No static portal blocks needed — explorer, wallet, DEX, marketplace, programs, developers, monitoring, and the main website are all served by Cloudflare Pages.

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

### Caddyfile — Dedicated RPC relay (recommended for 3 → 6 validators)

Deploy this on `rpc-relay-01` and `rpc-relay-02`.

```
# /etc/caddy/Caddyfile

rpc.moltchain.network {
    reverse_proxy \
      http://10.0.10.11:8899 \
      http://10.0.10.12:8899 \
      http://10.0.10.13:8899 {
        lb_policy least_conn
        fail_duration 30s
        max_fails 2
        unhealthy_status 500
        unhealthy_status 502
        unhealthy_status 503
        unhealthy_status 504
    }
}

ws.moltchain.network {
    reverse_proxy \
      http://10.0.10.11:8900 \
      http://10.0.10.12:8900 \
      http://10.0.10.13:8900 {
        lb_policy random
        fail_duration 30s
        max_fails 2
    }
}
```

When scaling to seed-04..seed-06, append those upstreams and reload Caddy:

```bash
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl reload caddy
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
curl -s http://localhost:9100/health | jq

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

### Key Material Retention Policy (online vs offline)

Keep this policy strict; do not delete genesis keys after deployment.

| Material | Keep online? | Keep offline? | Notes |
|---|---|---|---|
| `genesis-keys/` | **Minimal** (only where strictly required) | **Yes, mandatory** (multiple encrypted backups) | Root treasury/admin control; loss is catastrophic |
| `genesis-wallet.json` | Optional | Yes | Metadata/reference; not a replacement for keys |
| validator identity keypair | Yes (on each validator host) | Yes | One keypair per validator; never reused across nodes |
| signer keypair (`:9200`) | Yes (only on signer-enabled nodes) | Yes | Restrict access to private network + file perms |
| custody treasury keypair | Yes (custody host only) | Yes | Treat as production hot wallet material |

Minimum controls:

- file owner `moltchain:moltchain`, mode `600` for private key files
- no keys in git, CI artifacts, chat logs, or support tickets
- encrypted offline backups in at least two independent locations

### Encryption Status Checklist (must pass before mainnet)

This document defines required encryption posture; actual VPS state must be verified during deployment.

Required:

- **at rest:** full-disk or volume encryption on hosts storing keys/state/backups
- **in transit:** TLS for public endpoints (`rpc`, `ws`, `custody`, `faucet`)
- **backup encryption:** encrypted backup archives/object storage + key separation

Quick verification commands (Ubuntu examples):

```bash
# disk/volume encryption (example check)
lsblk -f
sudo cryptsetup status <luks_mapping_name> 2>/dev/null || true

# key file permissions
find /var/lib/moltchain -type f \( -name "*key*" -o -name "*wallet*.json" \) -exec ls -l {} \;

# TLS endpoint check
curl -I https://rpc.moltchain.network
curl -I https://custody.moltchain.network
```

If any item fails, block production rollout until fixed.

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

### "Seed peers found — will sync genesis from the existing network"

**Cause:** The validator sees `bootstrap_peers` in `seeds.json` and enters sync mode instead of creating genesis. This happens when you start a validator on a freshly wiped VPS where no network exists yet.

**Fix:** Use `moltchain-start.sh` instead of `systemctl start`. The start script runs `moltchain-genesis` before starting the validator when it detects an empty state directory.

```bash
# ✅ Correct — creates genesis first, then starts validator
bash moltchain-start.sh testnet

# ❌ Wrong — tries to sync from non-existent network
sudo systemctl start moltchain-validator-testnet
```

### "No genesis block found and no seed peers available"

**Cause:** The validator has no state AND no seeds to sync from (seeds.json missing or empty).

**Fix:** Make sure `seeds.json` exists in the working directory, then use `moltchain-start.sh`.

### Oracle: "Binance WebSocket connect failed: HTTP error: 451"

**Cause:** The US VPS is geo-blocked from `binance.com`. The oracle defaults to binance.com and gets HTTP 451 "Unavailable For Legal Reasons".

**Fix:** Set the oracle env vars to use `binance.us` before starting the validator:

```bash
export MOLTCHAIN_ORACLE_WS_URL="wss://stream.binance.us:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade"
export MOLTCHAIN_ORACLE_REST_URL="https://api.binance.us/api/v3/ticker/price?symbols=%5B%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22%5D"
bash moltchain-start.sh testnet
```

The EU VPS does not need these — it can reach `binance.com` directly.

### Validators can't find each other

```bash
# Check P2P is listening on 0.0.0.0 (not 127.0.0.1)
sudo ss -tlnp | grep 8000
# Should show: 0.0.0.0:7001 (testnet) or 0.0.0.0:8001 (mainnet)

# If it shows 127.0.0.1:7001, add --listen-addr 0.0.0.0 to the systemd service
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

### Infrastructure

```
[ ] 1. Build binaries: cargo build --release
[ ] 2. Compile all WASM contracts: cd contracts && cargo build --release --target wasm32-unknown-unknown
[ ] 3. Generate genesis locally (or on US VPS) — contracts/ dir must be present
[ ] 4. Verify genesis deployed all 30 contracts: curl localhost:8899 -d '{"method":"getAllContracts"}'
[ ] 5. Buy 3 VPS (EU/US/ASIA) — Ubuntu 22.04, 4+ CPU, 8+ GB RAM, NVMe
[ ] 6. Copy genesis state + contracts/ WASM dir to all 3 VPS
[ ] 7. Delete signer-keypair.json on EU/ASIA (they generate their own)
```

### DNS — Backend (A records → VPS)

```
[ ] 8.  A record: seed-us   → <US_IP>     (DNS only, gray cloud)
[ ] 9.  A record: seed-eu   → <EU_IP>     (DNS only, gray cloud)
[ ] 10. A record: seed-ap   → <ASIA_IP>   (DNS only, gray cloud)
[ ] 11. A records: rpc      → all 3 IPs   (Proxied, orange cloud)
[ ] 12. A records: ws       → all 3 IPs   (DNS only — CF free breaks WS)
[ ] 13. A record: custody   → <US_IP>     (Proxied)
[ ] 14. A record: faucet    → <US_IP>     (Proxied)
```

### DNS — Portals (CNAME → Cloudflare Pages)

```
[ ] 15. CNAME: @            → moltchain-website.pages.dev       (Proxied)
[ ] 16. CNAME: www          → moltchain.network                 (Proxied)
[ ] 17. CNAME: explorer     → moltchain-explorer.pages.dev      (Proxied)
[ ] 18. CNAME: wallet       → moltchain-wallet.pages.dev        (Proxied)
[ ] 19. CNAME: dex          → moltchain-dex.pages.dev           (Proxied)
[ ] 20. CNAME: marketplace  → moltchain-marketplace.pages.dev   (Proxied)
[ ] 21. CNAME: programs     → moltchain-programs.pages.dev      (Proxied)
[ ] 22. CNAME: developers   → moltchain-developers.pages.dev    (Proxied)
[ ] 23. CNAME: monitoring   → moltchain-monitoring.pages.dev    (Proxied)
```

### VPS Setup

```
[ ] 24. Run system setup on all 3 VPS (user, dirs, binaries)
[ ] 25. Install Caddy on all 3 VPS + configure Caddyfiles (backend services only)
[ ] 26. Configure firewall (ufw) on all 3 VPS
[ ] 27. Start validator on US VPS first → verify blocks + contracts deployed
[ ] 28. Start validator on EU VPS → verify it peers with US
[ ] 29. Start validator on ASIA VPS → verify 3 validators in getValidators
```

### Services

```
[ ] 30. Deploy custody service on US VPS
[ ] 31. Deploy faucet on US VPS (testnet only)
[ ] 32. Copy faucet UI files to /opt/moltchain/www/faucet-ui/
[ ] 33. Restart Caddy → verify HTTPS certs for rpc/ws/custody/faucet subdomains
```

### Cloudflare Pages

```
[ ] 34. Install Wrangler CLI: npm install -g wrangler && wrangler login
[ ] 35. Create CF Pages project: moltchain-website     (output dir: website)
[ ] 36. Create CF Pages project: moltchain-explorer     (output dir: explorer)
[ ] 37. Create CF Pages project: moltchain-wallet       (output dir: wallet)
[ ] 38. Create CF Pages project: moltchain-dex          (output dir: dex)
[ ] 39. Create CF Pages project: moltchain-marketplace  (output dir: marketplace)
[ ] 40. Create CF Pages project: moltchain-programs     (output dir: programs)
[ ] 41. Create CF Pages project: moltchain-developers   (output dir: developers)
[ ] 42. Create CF Pages project: moltchain-monitoring   (output dir: monitoring)
[ ] 43. Attach custom domains to each Pages project (auto-creates CNAME records)
[ ] 44. Deploy all portals: wrangler pages deploy <dir> --project-name=<name>
```

### Frontend Config (fix before deploying to Pages)

```
[ ] 45. Fix faucet/faucet.js: FAUCET_API → production URL
[ ] 46. Fix dex/dex.js: wire network selector dropdown
[ ] 47. Fix monitoring/js/monitoring.js: VALIDATOR_RPCS → seed-us/eu/ap
[ ] 48. Fix explorer/js/transaction.js L110: hardcoded localhost:9100
[ ] 49. Fix programs/js/landing.js: add mainnet to auto-detect
[ ] 50. Fix shared/wallet-connect.js: fallback port 9000 → 8899
```

### Verify Everything

```
[ ] 51. https://moltchain.network            — main website loads (CF Pages)
[ ] 52. https://rpc.moltchain.network         — RPC responds to getHealth (VPS)
[ ] 53. https://ws.moltchain.network          — WebSocket connects (VPS)
[ ] 54. https://explorer.moltchain.network    — block explorer loads (CF Pages)
[ ] 55. https://wallet.moltchain.network      — wallet loads, network switcher works (CF Pages)
[ ] 56. https://dex.moltchain.network         — DEX loads (CF Pages)
[ ] 57. https://marketplace.moltchain.network — marketplace loads (CF Pages)
[ ] 58. https://programs.moltchain.network    — Programs IDE loads (CF Pages)
[ ] 59. https://developers.moltchain.network  — dev portal loads (CF Pages)
[ ] 60. https://monitoring.moltchain.network  — dashboard shows 3 validators (CF Pages)
[ ] 61. https://faucet.moltchain.network      — faucet UI loads, airdrop works (VPS)
[ ] 62. https://custody.moltchain.network     — custody /health returns OK (VPS)
```

### Post-Launch

```
[ ] 63. Update seeds.json with real IPs/domains
[ ] 64. Test agent connection from local machine:
        ./moltchain-validator --bootstrap-peers seed-us.moltchain.network:8001
[ ] 65. Set up backup cron jobs
[ ] 66. Set up health check monitoring
[ ] 67. Copy genesis-keys/ to a secure offline location (USB / vault)
[ ] 68. Set up WireGuard VPN between 3 VPS for signer port 9200
[ ] 69. Connect git repo to CF Pages for auto-deploy on push
```

---

## Clean Slate Deployment Runbook

> **Last validated: March 10, 2026** — This is the exact, battle-tested procedure for wiping all blockchain state and redeploying MoltChain from scratch across all VPS nodes. Every step has been validated in production. Follow them in order.

### ⚠️ Critical Knowledge — Read Before Starting

These are hard-won lessons from repeated deployment failures. Understand them before proceeding.

#### 1. Genesis is created by `moltchain-start.sh`, NOT by systemd services

The **systemd services** (`moltchain-validator-testnet`, `moltchain-validator-mainnet`) simply start the validator binary. They do **NOT** create genesis. If you start them on a freshly wiped VPS, the validator will either:
- Try to sync from seed peers (and fail because no network exists yet), or
- Exit with "No genesis block found and no seed peers available"

**Genesis creation requires the `moltchain-genesis` binary**, which is called automatically by the `moltchain-start.sh` script. Always use the start script for first boot.

```
✅  bash moltchain-start.sh testnet     ← Creates genesis + starts validator
❌  sudo systemctl start moltchain-validator-testnet   ← Does NOT create genesis
```

#### 2. seeds.json and the "sync trap"

`seeds.json` lists known peers (e.g., `15.204.229.189:7001`). When the validator binary starts and finds these peers but has no local state, it enters sync mode — trying to download the chain from those peers. If the peer network doesn't exist yet, it loops forever.

This is **not a problem in practice** because `moltchain-start.sh` handles it: on genesis boot it creates state BEFORE starting the validator, and on join boot (`--bootstrap`) it syncs from the specified peer. The trap only bites if you bypass the script and start the validator binary directly (e.g., via systemd on a fresh VPS).

Don't remove `seeds.json` — the validator uses it for peer discovery after initial boot.

#### 3. Two different data paths — don't mix them up

| Method | Testnet state path | Mainnet state path |
|---|---|---|
| `moltchain-start.sh` | `~/moltchain/data/state-testnet/` | `~/moltchain/data/state-mainnet/` |
| systemd services | `/var/lib/moltchain/state-testnet/` | `/var/lib/moltchain/state-mainnet/` |

If you create genesis via the start script, the state lives in `~/moltchain/data/state-{port}/`. The systemd services look in `/var/lib/moltchain/state-{network}/`. Don't flush one expecting the other to be affected.

**Recommendation:** Use `moltchain-start.sh` for all VPS deployments. It stores state at `~/moltchain/data/state-{port}/` and manages the validator process via `scripts/validator-supervisor.sh` with auto-restart.

#### 4. US VPS requires binance.us oracle URLs (geo-blocking)

The US VPS (15.204.229.189) is geo-blocked from `binance.com` (HTTP 451 "Unavailable For Legal Reasons"). The oracle price feeder must use `binance.us` URLs instead:

```bash
export MOLTCHAIN_ORACLE_WS_URL="wss://stream.binance.us:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade"
export MOLTCHAIN_ORACLE_REST_URL="https://api.binance.us/api/v3/ticker/price?symbols=%5B%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22%5D"
```

The EU VPS (37.59.97.61) can use the default `binance.com` URLs. These env vars must be set **before** starting the validator process. The start script does NOT set them — you must export them in the shell before running it, or set them in the validator's environment.

#### 5. One chain — genesis VPS first, all others join

There is ONE testnet chain and ONE mainnet chain. The first VPS (US) creates genesis. Every other VPS joins that chain by syncing from the genesis VPS:

```bash
# US VPS — creates genesis (first boot, no --bootstrap)
bash moltchain-start.sh testnet

# EU VPS — joins the existing chain (testnet P2P = 7001, mainnet P2P = 8001)
bash moltchain-start.sh testnet --bootstrap 15.204.229.189:7001

# SEA VPS — also joins
bash moltchain-start.sh testnet --bootstrap 15.204.229.189:7001
```

**Port numbers:** `moltchain-start.sh` uses P2P port **7001** for testnet and **8001** for mainnet. When bootstrapping, use the P2P port, not the RPC port.

If you accidentally run `moltchain-start.sh` without `--bootstrap` on a second VPS, it creates a separate chain with a different genesis hash. Those two chains cannot peer. Wipe the state and re-run with `--bootstrap`.

#### 6. Cross-compilation from macOS to Linux does NOT work

The `cross` tool (Docker-based cross-compilation) fails because `aws-lc-sys v0.37.0` detects a known GCC `memcmp` bug in the cross Docker image (GCC bugzilla #95189). Neither `AWS_LC_SYS_CMAKE_BUILDER=1` nor `AWS_LC_SYS_NO_ASM=1` resolves this for release builds.

**Solution:** Build natively on VPS or use pre-built binaries from CI. To build on VPS:
1. Install Rust: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
2. Install cmake: `sudo apt-get install -y cmake`
3. Build: `cargo build --release --bin moltchain-validator --bin moltchain-genesis --bin moltchain-faucet --bin moltchain-custody`
4. Strip: `strip target/release/moltchain-{validator,genesis,faucet,custody}`

To relay pre-built binaries from one VPS to others:
```bash
# Download from source VPS to local
scp -P 2222 -O ubuntu@<SOURCE_VPS>:/usr/local/bin/moltchain-validator /tmp/moltchain-bins/

# Upload to target VPS
scp -P 2222 -O /tmp/moltchain-bins/moltchain-validator ubuntu@<TARGET_VPS>:/tmp/
ssh -p 2222 ubuntu@<TARGET_VPS> "mkdir -p ~/moltchain/target/release && cp /tmp/moltchain-validator ~/moltchain/target/release/ && chmod +x ~/moltchain/target/release/moltchain-validator"
```

#### 7. CLI binary name is `molt`, NOT `molt-cli`

The CLI crate is named `molt-cli` in `Cargo.toml`, but the binary target is `molt` (via `[[bin]] name = "molt"` in `cli/Cargo.toml`). When building or referencing the CLI binary:

```bash
✅  cargo build --release --bin molt
❌  cargo build --release --bin molt-cli   # ERROR: no bin target named 'molt-cli'
```

#### 8. Inter-VPS SSH access

Each VPS has an `ed25519` SSH keypair and the other VPSes' public keys in `authorized_keys`. This enables direct VPS-to-VPS `scp` and `ssh`:

```bash
# From US VPS, copy a file to EU VPS:
scp -P 2222 /path/to/file ubuntu@37.59.97.61:/path/to/dest

# From EU VPS, check US VPS status:
ssh -p 2222 ubuntu@15.204.229.189 "pgrep -af moltchain"
```

If VPS-to-VPS `scp` fails ("Permission denied"), verify that the source VPS's public key (`~/.ssh/id_ed25519.pub`) is in the target VPS's `~/.ssh/authorized_keys`.

#### 9. The "SINGLE GENESIS INVARIANT" in the validator binary

The validator binary has hardcoded bootstrap peers (embedded at compile time from `seeds.json`). If **any** seed peers are reachable — even without a local `seeds.json` file — the validator enters "joining" mode and will never create genesis on its own.

This means:
- Removing `seeds.json` does NOT prevent sync attempts (embedded peers still exist)
- You MUST use `moltchain-genesis` (or `moltchain-start.sh`) to create the genesis database before starting a new chain
- Never call `sudo systemctl start moltchain-validator-testnet` on a fresh VPS — it has no genesis

#### 10. pkill over SSH kills the SSH session

Running `pkill -f moltchain` over SSH kills the SSH session itself (because "moltchain" appears in the process tree of the SSH session when you're cd'd into `~/moltchain`).

**Safe stop pattern:**
```bash
ssh -p 2222 ubuntu@<VPS_IP> 'cat > /tmp/stop.sh << "EOF"
#!/bin/bash
for proc in moltchain-validator moltchain-faucet moltchain-custody validator-supervisor; do
  pids=$(pgrep -f "$proc" 2>/dev/null)
  [ -n "$pids" ] && echo "Stopping $proc" && echo "$pids" | xargs kill 2>/dev/null
done
EOF
chmod +x /tmp/stop.sh && bash /tmp/stop.sh'
```

---

### Prerequisites

| Requirement | Value |
|---|---|
| Local machine | macOS/Linux with Rust toolchain, SSH access to all VPS |
| US VPS | `ubuntu@15.204.229.189` (SSH port 2222) — genesis creator, custody host |
| EU VPS | `ubuntu@37.59.97.61` (SSH port 2222) — joins US |
| SEA VPS | `ubuntu@15.235.142.253` (SSH port 2222) — joins US |
| SSH command | `ssh -p 2222 ubuntu@<IP>` |
| Source repo | `lobstercove/moltchain` on GitHub (rsync to VPS — no git on VPS) |
| Start script | `moltchain-start.sh` — handles genesis + validator + contract deployment |
| Data dir (start script) | `~/moltchain/data/state-{testnet,mainnet}/` |
| P2P ports | Testnet: **7001**, Mainnet: **8001** |
| RPC ports | Testnet: **8899**, Mainnet: **9899** |

### Step 0: Stop Everything

Kill all validator processes on both VPS and local machine.

> **Warning:** Running `pkill -f moltchain` over SSH will kill the SSH session itself (the command line contains "moltchain" since you're cd'd into `~/moltchain`). Use `pgrep` to find PIDs and `kill` them individually, or write a script to `/tmp/` and execute it.

```bash
# ── Local machine ──
pkill -f moltchain-validator 2>/dev/null || true
for port in 7001 8001 8899 8900 9899 9900; do
  lsof -i ":$port" -t 2>/dev/null | xargs kill -9 2>/dev/null
done

# ── Each VPS (run the same commands on US, EU, and SEA) ──
# Write stop script to avoid SSH self-kill
ssh -p 2222 ubuntu@<VPS_IP> 'cat > /tmp/stop-moltchain.sh << '\''SCRIPT'\''
#!/bin/bash
for proc in moltchain-validator moltchain-faucet moltchain-custody validator-supervisor; do
  pids=$(pgrep -f "$proc" 2>/dev/null)
  if [ -n "$pids" ]; then
    echo "Stopping $proc: $pids"
    echo "$pids" | xargs kill 2>/dev/null
  fi
done
sleep 2
pgrep -af moltchain || echo "all stopped"
SCRIPT
chmod +x /tmp/stop-moltchain.sh && bash /tmp/stop-moltchain.sh'
```

### Step 1: Wipe All State

Remove all blockchain state, logs, custody databases, and TOFU peer fingerprint caches.

```bash
# ── Local machine ──
rm -rf ./data/state-* 2>/dev/null

# ── Each VPS (run on US, EU, and SEA) ──
ssh -p 2222 ubuntu@<VPS_IP> "
  # Start-script data paths
  rm -rf ~/moltchain/data/state-testnet ~/moltchain/data/state-mainnet

  # Custody databases
  rm -rf ~/moltchain/data/custody-testnet ~/moltchain/data/custody-mainnet

  # Validator logs (actual location used by moltchain-start.sh)
  rm -rf /tmp/moltchain-testnet /tmp/moltchain-mainnet

  # TOFU peer fingerprint cache — MUST clear when validator identities change
  rm -f ~/.moltchain/peer_fingerprints.json

  # Systemd data paths (in case they were used previously)
  sudo rm -rf /var/lib/moltchain/state-testnet /var/lib/moltchain/state-mainnet
  sudo rm -rf /var/lib/moltchain/custody-db /var/lib/moltchain/custody-db-mainnet

  echo 'state wiped'
"
```

> **TOFU fingerprint cache:** Each validator stores peer identity fingerprints in `~/.moltchain/peer_fingerprints.json`. After a wipe, validators get new identities. If old fingerprints remain, the P2P layer rejects the new identity ("TOFU verification failed"). Always clear this file when wiping state.

### Step 2: Rsync Fresh Code

Push the latest code from local to both VPS.

```bash
# ── From local repo root ──
for VPS_IP in 15.204.229.189 37.59.97.61 15.235.142.253; do
  rsync -az --progress \
    --exclude 'target/' --exclude '.git/' --exclude 'data/' \
    --exclude 'node_modules/' --exclude 'nohup.out' --exclude 'typescript' \
    --exclude '.venv/' --exclude 'compiler/target/' --exclude 'logs/' \
    -e 'ssh -p 2222' \
    ./ "ubuntu@${VPS_IP}:~/moltchain/"
done
```

### Step 3: Build on Each VPS

Build the validator, faucet, and custody binaries. Takes 4-7 minutes per VPS.

```bash
ssh -p 2222 ubuntu@<VPS_IP> "
  cd ~/moltchain && source ~/.cargo/env
  cargo build --release --bin moltchain-validator --bin moltchain-genesis --bin moltchain-faucet --bin moltchain-custody
"
```

Verify:
```bash
ssh -p 2222 ubuntu@<VPS_IP> "ls -la ~/moltchain/target/release/moltchain-{validator,genesis,faucet,custody}"
```

### Step 4: Start Genesis — US VPS (Testnet)

**Use `moltchain-start.sh`**, not systemd. Set binance.us oracle env vars first.

```bash
ssh -p 2222 ubuntu@15.204.229.189 "
  cd ~/moltchain

  # US requires binance.us (binance.com returns HTTP 451)
  export MOLTCHAIN_ORACLE_WS_URL='wss://stream.binance.us:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade'
  export MOLTCHAIN_ORACLE_REST_URL='https://api.binance.us/api/v3/ticker/price?symbols=%5B%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22%5D'

  bash moltchain-start.sh testnet
"
```

What happens:
1. Detects empty `data/state-testnet/` → enters **GENESIS** mode
2. Fetches live prices from Binance for genesis pool seeding
3. Runs `moltchain-genesis --network testnet --db-path ./data/state-testnet`
4. Creates genesis block, treasury keys, deploys 29 contracts, 7 trading pairs, 7 AMM pools
5. Starts validator via `validator-supervisor.sh` (auto-restarts on crash)
6. Runs `first-boot-deploy.sh` in background (contract initialization)

Verify testnet is running:
```bash
ssh -p 2222 ubuntu@15.204.229.189 "
  tail -10 /tmp/moltchain-testnet/validator.log
  # Look for: 'Binance WebSocket connected' and 'HEARTBEAT' lines
"
```

### Step 5: Start Genesis — US VPS (Mainnet)

Same procedure for mainnet (ports 9000/9899/9900):

```bash
ssh -p 2222 ubuntu@15.204.229.189 "
  cd ~/moltchain

  export MOLTCHAIN_ORACLE_WS_URL='wss://stream.binance.us:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade'
  export MOLTCHAIN_ORACLE_REST_URL='https://api.binance.us/api/v3/ticker/price?symbols=%5B%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22%5D'

  bash moltchain-start.sh mainnet
"
```

Verify both networks:
```bash
ssh -p 2222 ubuntu@15.204.229.189 "
  pgrep -af moltchain-validator
  # Should show 2 supervisor + 2 validator processes (testnet + mainnet)
"
```

### Step 6: Join Chain — EU VPS (Testnet + Mainnet)

EU VPS joins the chains already created by US VPS. Uses `--bootstrap` to sync genesis + state from the US peer. No oracle env vars needed (binance.com works from EU).

```bash
ssh -p 2222 ubuntu@37.59.97.61 "
  cd ~/moltchain
  bash moltchain-start.sh testnet --bootstrap 15.204.229.189:7001
"

# Wait for testnet sync to complete, then join mainnet
ssh -p 2222 ubuntu@37.59.97.61 "
  cd ~/moltchain
  bash moltchain-start.sh mainnet --bootstrap 15.204.229.189:8001
"
```

### Step 6b: Join Chain — SEA VPS (Testnet + Mainnet)

Same as EU — joins via `--bootstrap`:

```bash
ssh -p 2222 ubuntu@15.235.142.253 "
  cd ~/moltchain
  nohup bash moltchain-start.sh testnet --bootstrap 15.204.229.189:7001 > /tmp/start-testnet.log 2>&1 & disown
"

# Wait for testnet sync, then mainnet
ssh -p 2222 ubuntu@15.235.142.253 "
  cd ~/moltchain
  nohup bash moltchain-start.sh mainnet --bootstrap 15.204.229.189:8001 > /tmp/start-mainnet.log 2>&1 & disown
"
```

Verify all 6 validators across all VPS:
```bash
for VPS_IP in 15.204.229.189 37.59.97.61 15.235.142.253; do
  echo "=== $VPS_IP ==="
  ssh -p 2222 "ubuntu@${VPS_IP}" "pgrep -c -f moltchain-validator && echo 'validators running'"
done
```

### Step 7: Start Faucet

Start the testnet faucet on all 3 VPSes. The faucet keypair is generated during genesis on the US VPS.

> **Key copying for joining VPS:** The EU and SEA VPSes (and any future VPS that joined via `--bootstrap`) do NOT have genesis keys locally — they exist only on the genesis VPS (US). You must copy them before starting faucet or custody on joining VPS.

```bash
# ── Copy faucet + treasury keys from US to EU + SEA ──
# (pipe through local machine — VPS-to-VPS scp may not work without inter-VPS SSH setup)
for JOINING_IP in 37.59.97.61 15.235.142.253; do
  echo "=== Copying keys to $JOINING_IP ==="
  for key in faucet-moltchain-testnet-1.json treasury-moltchain-testnet-1.json; do
    ssh -p 2222 ubuntu@15.204.229.189 "cat ~/moltchain/data/state-testnet/genesis-keys/$key" \
      | ssh -p 2222 ubuntu@$JOINING_IP "mkdir -p ~/moltchain/data/state-testnet/genesis-keys && cat > ~/moltchain/data/state-testnet/genesis-keys/$key"
  done
  for key in faucet-moltchain-mainnet-1.json treasury-moltchain-mainnet-1.json; do
    ssh -p 2222 ubuntu@15.204.229.189 "cat ~/moltchain/data/state-mainnet/genesis-keys/$key" \
      | ssh -p 2222 ubuntu@$JOINING_IP "mkdir -p ~/moltchain/data/state-mainnet/genesis-keys && cat > ~/moltchain/data/state-mainnet/genesis-keys/$key"
  done
done
```

Start faucet on all 3 VPSes:
```bash
# ── US VPS ──
ssh -p 2222 ubuntu@15.204.229.189 "
  cd ~/moltchain
  PORT=9100 \
  RPC_URL=http://127.0.0.1:8899 \
  NETWORK=testnet \
  MAX_PER_REQUEST=10 \
  DAILY_LIMIT_PER_IP=50 \
  COOLDOWN_SECONDS=60 \
  AIRDROPS_FILE=/tmp/moltchain-testnet/airdrops.json \
  FAUCET_KEYPAIR=\$HOME/moltchain/data/state-testnet/genesis-keys/faucet-moltchain-testnet-1.json \
  RUST_LOG=info \
  TRUSTED_PROXY=127.0.0.1,::1 \
  nohup ./target/release/moltchain-faucet > /tmp/moltchain-testnet/faucet.log 2>&1 &
  sleep 2 && pgrep -f moltchain-faucet && echo 'Faucet running'
"

# ── EU VPS ──
ssh -p 2222 ubuntu@37.59.97.61 '
  cd ~/moltchain
  PORT=9100 \
  RPC_URL=http://127.0.0.1:8899 \
  NETWORK=testnet \
  MAX_PER_REQUEST=10 \
  DAILY_LIMIT_PER_IP=50 \
  COOLDOWN_SECONDS=60 \
  AIRDROPS_FILE=/tmp/moltchain-testnet/airdrops.json \
  FAUCET_KEYPAIR=/home/ubuntu/moltchain/data/state-testnet/genesis-keys/faucet-moltchain-testnet-1.json \
  RUST_LOG=info \
  TRUSTED_PROXY=127.0.0.1,::1 \
  nohup ./target/release/moltchain-faucet > /tmp/moltchain-testnet/faucet.log 2>&1 &
  sleep 2 && pgrep -f moltchain-faucet && echo "Faucet running"
'

# ── SEA VPS ──
ssh -p 2222 ubuntu@15.235.142.253 '
  cd ~/moltchain
  PORT=9100 \
  RPC_URL=http://127.0.0.1:8899 \
  NETWORK=testnet \
  MAX_PER_REQUEST=10 \
  DAILY_LIMIT_PER_IP=50 \
  COOLDOWN_SECONDS=60 \
  AIRDROPS_FILE=/tmp/moltchain-testnet/airdrops.json \
  FAUCET_KEYPAIR=/home/ubuntu/moltchain/data/state-testnet/genesis-keys/faucet-moltchain-testnet-1.json \
  RUST_LOG=info \
  TRUSTED_PROXY=127.0.0.1,::1 \
  nohup ./target/release/moltchain-faucet > /tmp/moltchain-testnet/faucet.log 2>&1 &
  sleep 2 && pgrep -f moltchain-faucet && echo "Faucet running"
'
```

### Step 7b: Start Custody

Start custody bridge on US VPS for testnet and mainnet. Currently custody runs on US only.

> **Port env var:** Custody uses `CUSTODY_LISTEN_PORT` (NOT `PORT`). Default is 9105. Use 9106 for mainnet.
> **Scaling note:** For 10-20K users, consider running custody on all 3 VPSes behind a load balancer (see custody scaling brainstorm in docs/architecture/).

```bash
# ── US VPS — Testnet Custody (port 9105) ──
ssh -p 2222 ubuntu@15.204.229.189 "
  cd ~/moltchain && mkdir -p data/custody-testnet
  CUSTODY_DB_PATH=./data/custody-testnet \
  CUSTODY_MOLT_RPC_URL=http://127.0.0.1:8899 \
  CUSTODY_TREASURY_KEYPAIR=\$HOME/moltchain/data/state-testnet/genesis-keys/treasury-moltchain-testnet-1.json \
  CUSTODY_ALLOW_INSECURE_SEED=1 \
  CUSTODY_API_AUTH_TOKEN=testnet-custody-token-2026 \
  CUSTODY_SIGNER_ENDPOINTS=http://127.0.0.1:9201,http://127.0.0.1:9202,http://127.0.0.1:9203 \
  CUSTODY_SIGNER_THRESHOLD=2 \
  CUSTODY_ALLOW_UNSAFE_MULTISIGNER=1 \
  RUST_LOG=info \
  nohup ./target/release/moltchain-custody > /tmp/moltchain-testnet/custody.log 2>&1 &
  sleep 2 && tail -1 /tmp/moltchain-testnet/custody.log
"

# ── US VPS — Mainnet Custody (port 9106) ──
ssh -p 2222 ubuntu@15.204.229.189 "
  cd ~/moltchain && mkdir -p data/custody-mainnet
  CUSTODY_DB_PATH=./data/custody-mainnet \
  CUSTODY_MOLT_RPC_URL=http://127.0.0.1:9899 \
  CUSTODY_TREASURY_KEYPAIR=\$HOME/moltchain/data/state-mainnet/genesis-keys/treasury-moltchain-mainnet-1.json \
  CUSTODY_ALLOW_INSECURE_SEED=1 \
  CUSTODY_API_AUTH_TOKEN=mainnet-custody-token-2026 \
  CUSTODY_SIGNER_ENDPOINTS=http://127.0.0.1:9201,http://127.0.0.1:9202,http://127.0.0.1:9203 \
  CUSTODY_SIGNER_THRESHOLD=2 \
  CUSTODY_ALLOW_UNSAFE_MULTISIGNER=1 \
  CUSTODY_LISTEN_PORT=9106 \
  RUST_LOG=info \
  nohup ./target/release/moltchain-custody > /tmp/moltchain-mainnet/custody.log 2>&1 &
  sleep 2 && tail -1 /tmp/moltchain-mainnet/custody.log
"

# ── EU VPS — Testnet Custody (port 9105) — OPTIONAL, currently not running ──
# Uncomment these blocks when scaling custody to multiple VPSes
# ssh -p 2222 ubuntu@37.59.97.61 '
#   cd ~/moltchain && mkdir -p data/custody-testnet
#   CUSTODY_DB_PATH=./data/custody-testnet \
#   CUSTODY_MOLT_RPC_URL=http://127.0.0.1:8899 \
#   CUSTODY_TREASURY_KEYPAIR=/home/ubuntu/moltchain/data/state-testnet/genesis-keys/treasury-moltchain-testnet-1.json \
#   CUSTODY_ALLOW_INSECURE_SEED=1 \
#   CUSTODY_API_AUTH_TOKEN=testnet-custody-token-2026 \
#   CUSTODY_SIGNER_ENDPOINTS=http://127.0.0.1:9201,http://127.0.0.1:9202,http://127.0.0.1:9203 \
#   CUSTODY_SIGNER_THRESHOLD=2 \
#   CUSTODY_ALLOW_UNSAFE_MULTISIGNER=1 \
#   RUST_LOG=info \
#   nohup ./target/release/moltchain-custody > /tmp/moltchain-testnet/custody.log 2>&1 &
#   sleep 2 && tail -1 /tmp/moltchain-testnet/custody.log
# '

# ── EU VPS — Mainnet Custody (port 9106) — OPTIONAL, currently not running ──
# ssh -p 2222 ubuntu@37.59.97.61 '
#   cd ~/moltchain && mkdir -p data/custody-mainnet
#   CUSTODY_DB_PATH=./data/custody-mainnet \
#   CUSTODY_MOLT_RPC_URL=http://127.0.0.1:9899 \
#   CUSTODY_TREASURY_KEYPAIR=/home/ubuntu/moltchain/data/state-mainnet/genesis-keys/treasury-moltchain-mainnet-1.json \
#   CUSTODY_ALLOW_INSECURE_SEED=1 \
#   CUSTODY_API_AUTH_TOKEN=mainnet-custody-token-2026 \
#   CUSTODY_SIGNER_ENDPOINTS=http://127.0.0.1:9201,http://127.0.0.1:9202,http://127.0.0.1:9203 \
#   CUSTODY_SIGNER_THRESHOLD=2 \
#   CUSTODY_ALLOW_UNSAFE_MULTISIGNER=1 \
#   CUSTODY_LISTEN_PORT=9106 \
#   RUST_LOG=info \
#   nohup ./target/release/moltchain-custody > /tmp/moltchain-mainnet/custody.log 2>&1 &
#   sleep 2 && tail -1 /tmp/moltchain-mainnet/custody.log
# '
```

### Step 8: Verify Everything

```bash
for VPS_IP in 15.204.229.189 37.59.97.61 15.235.142.253; do
  echo ""
  echo "=== $VPS_IP ==="
  ssh -p 2222 "ubuntu@${VPS_IP}" "
    echo 'Processes:'
    pgrep -af moltchain | grep -v pgrep

    echo ''
    echo 'Testnet RPC:'
    curl -sf http://localhost:8899 -H 'content-type: application/json' \
      -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSlot\"}' || echo 'FAIL'

    echo ''
    echo 'Mainnet RPC:'
    curl -sf http://localhost:9899 -H 'content-type: application/json' \
      -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSlot\"}' || echo 'FAIL'

    echo ''
    echo 'Faucet:'
    curl -sf http://localhost:9100/health || echo 'FAIL'
  "
done
```

Expected per VPS:
- **US VPS**: 7 processes (2 supervisors + 2 validators + 1 faucet + 2 custody)
- **EU/SEA VPS**: 5 processes (2 supervisors + 2 validators + 1 faucet)

All VPSes should show advancing slot numbers on both testnet and mainnet RPC.

### Step 9 (Optional): Start Local Validator

Connect your local machine as an additional validator on mainnet:

```bash
./target/release/moltchain-validator \
  --network mainnet \
  --rpc-port 9899 \
  --ws-port 9900 \
  --p2p-port 8001 \
  --db-path ./data/state-mainnet \
  --bootstrap-peers "15.204.229.189:8001,37.59.97.61:8001,15.235.142.253:8001" \
  --listen-addr 0.0.0.0
```

### Port Map Summary

| Service | Testnet | Mainnet |
|---|---|---|
| P2P (QUIC) | 7001 | 8001 |
| RPC (HTTP) | 8899 | 9899 |
| WebSocket | 8900 | 9900 |
| Faucet | 9100 | — |
| Custody | 9105 | 9106 |
| Signer | 9201 | 9202 |

### Validator Log Locations

Logs are written to `/tmp/moltchain-{network}/` (created by `moltchain-start.sh`), **not** `~/moltchain/logs/`.

| Log | Path |
|---|---|
| Testnet validator | `/tmp/moltchain-testnet/validator.log` |
| Mainnet validator | `/tmp/moltchain-mainnet/validator.log` |
| Testnet faucet | `/tmp/moltchain-testnet/faucet.log` |
| Testnet custody | `/tmp/moltchain-testnet/custody.log` |
| Mainnet custody | `/tmp/moltchain-mainnet/custody.log` |
| First-boot deploy | `/tmp/moltchain-testnet/first-boot-deploy.log` |

### Restarting a Validator (Without Re-creating Genesis)

If a validator crashes or you need to restart it, the start script detects existing state and enters **RESUME** mode (no genesis):

```bash
# The start script detects data/state-testnet/CURRENT exists → RESUME mode
ssh -p 2222 ubuntu@<VPS_IP> "
  cd ~/moltchain

  # For US VPS, always set binance.us env vars
  export MOLTCHAIN_ORACLE_WS_URL='wss://stream.binance.us:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade'
  export MOLTCHAIN_ORACLE_REST_URL='https://api.binance.us/api/v3/ticker/price?symbols=%5B%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22%5D'

  bash moltchain-start.sh testnet
"
```

Or restart directly via the supervisor (skipping the start script):

```bash
ssh -p 2222 ubuntu@<VPS_IP> "
  cd ~/moltchain

  export MOLTCHAIN_ORACLE_WS_URL='wss://stream.binance.us:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade'
  export MOLTCHAIN_ORACLE_REST_URL='https://api.binance.us/api/v3/ticker/price?symbols=%5B%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22%5D'

  ./scripts/validator-supervisor.sh testnet-primary-p7001 -- \
    ./target/release/moltchain-validator \
    --network testnet --rpc-port 8899 --ws-port 8900 --p2p-port 7001 \
    --db-path ./data/state-7001 \
    --bootstrap-peers '15.204.229.189:7001,37.59.97.61:7001' \
    --listen-addr 0.0.0.0 \
    >./logs/validator.log 2>&1 &
"
```

### Files Created During Genesis

After a successful genesis boot via `moltchain-start.sh`, these files exist:

```
~/moltchain/
├── data/
│   ├── state-testnet/                    # Testnet blockchain state (RocksDB)
│   │   ├── genesis-keys/                 # Treasury keypairs (CRITICAL — back these up)
│   │   │   ├── genesis-primary-moltchain-testnet-1.json
│   │   │   ├── faucet-moltchain-testnet-1.json
│   │   │   ├── treasury-moltchain-testnet-1.json
│   │   │   ├── community_treasury-moltchain-testnet-1.json
│   │   │   ├── builder_grants-moltchain-testnet-1.json
│   │   │   └── ...
│   │   ├── home/                         # Validator identity (validator-id.json)
│   │   ├── CURRENT                       # RocksDB marker (presence = state exists)
│   │   └── <SST files>                   # RocksDB data
│   └── state-mainnet/                    # Mainnet blockchain state (same structure)
│       ├── genesis-keys/
│       └── ...
└── target/release/
    ├── moltchain-validator               # Main binary
    ├── moltchain-genesis                 # Genesis creation tool
    ├── moltchain-faucet                  # Faucet binary
    └── moltchain-custody                 # Custody bridge binary

# Logs (written to /tmp/ by moltchain-start.sh, NOT ~/moltchain/logs/)
/tmp/
├── moltchain-testnet/
│   ├── validator.log                     # Testnet validator log
│   ├── faucet.log                        # Faucet log
│   ├── custody.log                       # Testnet custody log
│   └── first-boot-deploy.log            # Contract deployment log
└── moltchain-mainnet/
    ├── validator.log                     # Mainnet validator log
    └── custody.log                       # Mainnet custody log
```

---

## Release Signing Setup

Before pushing your first release, generate an Ed25519 keypair for signing release artifacts. This is a **one-time** operation.

### 1. Generate the Signing Keypair

```bash
./scripts/generate-release-keys.sh

# Output:
# ✅ Keypair generated successfully!
# 📁 Keypair file: ./release-signing-keypair.json
#    ⚠️  KEEP THIS FILE SECRET AND OFFLINE!
#
# 🔑 Public key (embed in validator/src/updater.rs):
# a1b2c3d4e5f6...  (64 hex chars)
```

### 2. Embed the Public Key

Copy the public key hex output and paste it into `validator/src/updater.rs`:

```rust
const RELEASE_SIGNING_PUBKEY_HEX: &str =
    "a1b2c3d4e5f6...your_actual_public_key_hex...";
```

Then rebuild the validator binary.

### 3. Secure the Private Key

- **Move** `release-signing-keypair.json` to a secure offline location (USB drive, vault)
- **Never** commit it to git
- **Never** store it on any VPS or CI server
- Add `release-signing-keypair.json` to `.gitignore` (already included)
- You'll need it only when signing releases (step performed offline by a maintainer)

---

## Creating a Release

### First Release (v0.1.0)

```bash
# 1. Ensure everything is committed and tested
cargo test
cargo build --release

# 2. Tag the release
git tag -a v0.1.0 -m "MoltChain v0.1.0 - Initial release"
git push origin v0.1.0
```

This triggers the GitHub Actions release workflow (`.github/workflows/release.yml`):
- Builds binaries for Linux x86_64/aarch64 + macOS x86_64/aarch64
- Creates SHA256SUMS
- Publishes a **draft** GitHub Release with all artifacts

### Sign the Release

After CI completes:

```bash
# 1. Download SHA256SUMS from the draft release
curl -LO https://github.com/lobstercove/moltchain/releases/download/v0.1.0/SHA256SUMS

# 2. Sign it offline with your private key
./scripts/sign-release.sh SHA256SUMS /path/to/release-signing-keypair.json

# Output: SHA256SUMS.sig

# 3. Upload SHA256SUMS.sig to the GitHub Release
# (use the GitHub UI or gh CLI)
gh release upload v0.1.0 SHA256SUMS.sig

# 4. Publish the release (remove draft status)
gh release edit v0.1.0 --draft=false
```

### Subsequent Releases

```bash
# 1. Bump version in validator/Cargo.toml
# 2. Commit and tag
git tag -a v0.2.0 -m "MoltChain v0.2.0 - <description>"
git push origin v0.2.0

# 3. Wait for CI to build (~10 min)
# 4. Sign and publish (same steps as above)
```

### Manual Binary Distribution (Without CI)

If you need to distribute binaries before CI is set up:

```bash
# Build locally
cargo build --release

# Create archive
cd target/release
tar czf moltchain-validator-darwin-aarch64.tar.gz moltchain-validator
cd ../..

# Compute SHA256
sha256sum target/release/moltchain-validator-darwin-aarch64.tar.gz > SHA256SUMS

# Sign
./scripts/sign-release.sh SHA256SUMS /path/to/release-signing-keypair.json

# Create GitHub release manually and upload:
#   - moltchain-validator-darwin-aarch64.tar.gz
#   - SHA256SUMS
#   - SHA256SUMS.sig
```

---

## Auto-Update System

The validator includes a built-in auto-update system that can check for, download, verify, and apply new releases from GitHub.

### How It Works

1. **Check**: Periodically queries GitHub Releases API for a newer version
2. **Download**: Downloads the platform-specific archive (`.tar.gz`)
3. **Verify**: Checks Ed25519 signature (SHA256SUMS.sig → SHA256SUMS) + SHA256 hash of archive
4. **Apply**: Atomic binary swap (current → `.rollback`, staging → current)
5. **Restart**: Exits with code 75 → supervisor picks up the new binary

### Security

- **Ed25519 signature verification** — the release signing public key is compiled into the binary
- **SHA256 hash verification** — archive integrity is verified against signed SHA256SUMS
- **Rollback guard** — if the validator crashes 3 times within 60 seconds of an update, it automatically rolls back to the previous binary
- **Staggered updates** — random jitter (0–60s) prevents all validators from restarting simultaneously

### Enable Auto-Update on a Validator

```bash
# Check-only mode (logs new versions, doesn't download)
./moltchain-validator \
  --auto-update check \
  --p2p-port 8000

# Download + verify, but don't apply (manual restart needed)
./moltchain-validator \
  --auto-update download \
  --p2p-port 8000

# Full automatic updates (recommended for testnet)
./moltchain-validator \
  --auto-update apply \
  --p2p-port 8000

# Customize check interval and channel
./moltchain-validator \
  --auto-update apply \
  --update-check-interval 600 \
  --update-channel stable \
  --p2p-port 8000
```

### Production Recommendations

| Environment | Recommended Mode | Reasoning |
|---|---|---|
| **Testnet** | `--auto-update apply` | Fast iteration, rollback guard protects against bad releases |
| **Mainnet (seed validators)** | `--auto-update download` | Stage updates, verify manually, then restart on your schedule |
| **Mainnet (community validators)** | `--auto-update apply --update-check-interval 600` | Automatic with longer check interval for stability |

### Monitoring Updates

The validator logs all update activity:
```
INFO  🔄 Auto-updater: enabled (mode=apply, interval=300s, channel=stable)
INFO  🔄 Up to date (current: v0.1.0, latest: v0.1.0)
INFO  🆕 New version available: v0.2.0 (current: v0.1.0)
INFO  📦 Downloading moltchain-validator-linux-x86_64.tar.gz (24MB)...
INFO  ✅ SHA256SUMS signature verified
INFO  ✅ SHA256 verified for moltchain-validator-linux-x86_64.tar.gz
INFO  ✅ Binary swapped: v0.1.0 → v0.2.0
INFO  🔄 Requesting supervisor restart to pick up new binary...
```

### Version Gossip

Validators broadcast their version in P2P `ValidatorAnnounce` messages. This means:
- The explorer/monitoring can show what version each validator is running
- You can verify all validators have picked up a new release
- The `getClusterInfo` RPC endpoint includes version data

### Rollback

If an update goes wrong:

```bash
# Manual rollback (if auto-rollback didn't trigger)
cd /path/to/validator
cp moltchain-validator.rollback moltchain-validator
# Restart the validator
```

The auto-rollback triggers automatically if the validator crashes 3 times within 60 seconds of an update.

---

## Validator CLI Reference

Complete reference for all `moltchain-validator` CLI flags:

### Core Flags

| Flag | Default | Description |
|---|---|---|
| `--p2p-port <port>` | `8000` | P2P QUIC listen port |
| `--rpc-port <port>` | `8899` | JSON-RPC HTTP port (derived from p2p: +899) |
| `--ws-port <port>` | `8900` | WebSocket port (derived from p2p: +900) |
| `--db-path <path>` | `./data/state-<p2p_port>` | RocksDB state directory |
| `--genesis <file>` | auto | Path to genesis config JSON |
| `--keypair <file>` | auto | Path to validator keypair JSON |
| `--network <name>` | `testnet` | Network: `testnet` or `mainnet` |
| `--listen-addr <ip>` | `127.0.0.1` | P2P bind address (`0.0.0.0` for VPS) |
| `--admin-token <token>` | none | Admin API bearer token |

### P2P Flags

| Flag | Default | Description |
|---|---|---|
| `--bootstrap <host:port>` | none | Single bootstrap peer |
| `--bootstrap-peers <h1:p1,h2:p2>` | none | Comma-separated bootstrap peers |

### Supervisor Flags

| Flag | Default | Description |
|---|---|---|
| `--no-watchdog` | false | Disable supervisor/watchdog (run single process) |
| `--watchdog-timeout <secs>` | `120` | Seconds without blocks before stall restart |
| `--max-restarts <n>` | `50` | Max supervisor restarts before giving up |

### Auto-Update Flags

| Flag | Default | Description |
|---|---|---|
| `--auto-update <mode>` | `off` | Update mode: `off`, `check`, `download`, `apply` |
| `--update-check-interval <secs>` | `300` | Seconds between update checks |
| `--update-channel <channel>` | `stable` | Release channel: `stable`, `beta`, `edge` |
| `--no-auto-restart` | false | Download + verify but don't apply (manual restart) |

### Example: Production VPS Validator

```bash
./moltchain-validator \
  --p2p-port 7001 \
  --listen-addr 0.0.0.0 \
  --network testnet \
  --genesis genesis-testnet.json \
  --bootstrap-peers seed-us.moltchain.network:8001,seed-eu.moltchain.network:8001 \
  --auto-update apply \
  --admin-token "$(cat /etc/moltchain/admin-token)"
```

---

## VPS Recommendations

| Region | Provider | Spec | Cost/mo |
|---|---|---|---|
| US (NYC) | DigitalOcean / Hetzner | 4 vCPU, 8GB RAM, 160GB NVMe | ~$24-48 |
| EU (Frankfurt) | Hetzner | 4 vCPU, 8GB RAM, 160GB NVMe | ~$24 |
| ASIA (Singapore) | DigitalOcean / Vultr | 4 vCPU, 8GB RAM, 160GB NVMe | ~$24-48 |

RocksDB benefits strongly from NVMe — avoid HDD-based VPS. 4 vCPU is plenty for current throughput. Scale up to 8 vCPU when transaction volume grows.

---

## Admin Key Management Lifecycle

This section is mandatory for production operations and aligns with tested admin/write workflows used by gate suites.

### Key Classes

| Key class | Primary use | Storage location | Human access policy |
|---|---|---|---|
| Genesis distribution keys (`genesis-keys/*`) | Treasury/distribution control | Offline encrypted vault only | Dual-control (2 maintainers) |
| Validator identity keypair | Validator identity + signing | VPS filesystem (`0600`) + encrypted backup | Single operator + break-glass approver |
| RPC admin token (`--admin-token`) | Privileged RPC methods (`setFeeConfig`, `setRentParams`, `setContractAbi`) | Secret manager / root-readable env file | Operations leads only |
| Custody treasury keypair | Custody withdrawals/rebalancing | Custody host secure path + encrypted backup | Finance ops + security lead |
| Release signing keypair | Binary release signatures | Air-gapped device only | Release manager + security approver |

### Rotation Policy

| Secret | Rotation cadence | Triggered rotation events |
|---|---|---|
| RPC admin token | Every 30 days | suspected leak, operator offboarding, incident response |
| Validator keypair | Every 90 days or major incident | node compromise, key exposure, host rebuild |
| Custody treasury keypair | Every 30 days in testnet, 90 days mainnet | custody host compromise, unauthorized access signal |
| Release signing keypair | Every 180 days | signing workflow compromise, maintainer change |

### Backup & Recovery Requirements

1. Keep two encrypted backups for each critical key class in separate regions.
2. Validate restore quarterly on isolated hosts (never in production namespace).
3. Record recovery drill evidence in internal audit notes (date, operator, success/failure).

### Revocation / Emergency Procedure

1. Freeze privileged operations (disable admin endpoints via token rotation + service restart).
2. Rotate compromised secrets immediately.
3. Reissue operational credentials and verify non-admin rejection checks:
   - `setFeeConfig` without valid token must return RPC error.
   - `setRentParams` without valid token must return RPC error.
4. Re-run strict gate before reopening production write paths.

### Minimum Deployment Secrets Checklist

- [ ] Admin token stored in secret manager; not present in shell history.
- [ ] Validator keypair file permissions set to `0600`.
- [ ] Custody keypair + seed paths validated and encrypted at rest.
- [ ] Backup restore test completed within last 90 days.
- [ ] Revocation runbook reviewed by on-call operators.

---

## Environment Command Matrix

This matrix aligns local/testnet/prod procedures with the tested sequence: build → deploy genesis/programs → configure services → strict gate → launch.

| Stage | Local Dev | Testnet (3 validators) | Production/Mainnet |
|---|---|---|---|
| Build | `cargo build --release` | `cargo build --release` | `cargo build --release` |
| Deploy genesis/programs | `./target/release/moltchain-validator --network testnet --p2p-port 8000` (first run) | same + state distribution to seed nodes | mainnet genesis generation + signed distribution artifacts |
| Configure services | local env defaults | systemd + `/etc/moltchain/env-testnet` + Caddy | systemd + secrets manager + hardened Caddy/firewall |
| Strict gate | `STRICT_NO_SKIPS=1 bash tests/production-e2e-gate.sh` | `STRICT_NO_SKIPS=1 bash tests/production-e2e-gate.sh` | pre-launch requirement: same strict gate against prod-like env |
| Launch / operate | `./moltchain-validator ...` | `systemctl start moltchain-validator` (+ custody/faucet where needed) | staged rollout across seeds/relays with monitored restart windows |

### Tested Gate Sequence (Reference)

Run this exact sequence before declaring deployment-ready state:

```bash
# 1) Build
cargo build --release

# 2) Ensure validators/services are up and healthy
curl -s http://localhost:8899 -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'

# 3) Strict production gate (no critical skips)
STRICT_NO_SKIPS=1 bash tests/production-e2e-gate.sh
```

For testnet and production readiness sign-off, require 3 consecutive strict gate passes plus artifact archival.
