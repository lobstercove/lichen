# Lichen Production Deployment — Complete Strategy

> Full step-by-step guide for deploying Lichen across seed + relay infrastructure with 3 to 6 validators using the `lichen.network` domain. Covers genesis bootstrapping, DNS, seed discovery, RPC/WS rotation, custody bridge, faucet, and how agent-run validators connect.

**Release-ready status (Mar 19, 2026):** v0.4.5 validated on a fresh 3-VPS redeploy using repo-local runtime paths, explicit `lichen-genesis` initialization, and direct validator launches against `~/lichen/data/state-{testnet,mainnet}`.

> Canonical v0.4.5 production path: prefer `deploy/setup.sh`, the network-specific `/etc/lichen/env-*` files it generates, and the clean-slate runbook later in this document. Older examples in this guide that reference `/opt/lichen`, single-unit `lichen-validator.service`, or ad hoc setup scripts should be treated as historical context unless they were explicitly updated after Mar 19, 2026.

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

Lichen compiles into **4 separate binaries** from the workspace:

| Binary | Crate | Port | Runs On | Description |
|---|---|---|---|---|
| `lichen-validator` | `validator` | P2P: 7001/8001, RPC: 8899/9899, WS: 8900/9900, Signer: 9201 | Every VPS | Validator + built-in RPC + WebSocket + threshold signer. **This is the main binary.** Ports are testnet/mainnet respectively. |
| `lichen-custody` | `custody` | 9105 | Seed VPS only (1 instance) | Bridge service for Solana/Ethereum ↔ Lichen deposits & withdrawals |
| `lichen-faucet` | `faucet` | 9100 | All VPSes (testnet only) | LICN faucet for testnet. Refuses to run on mainnet. Same keypair copied to each VPS. |
| `lichen` | `cli` | — | Dev machines + all VPSes | CLI tool for sending transactions, querying state, funding faucet. Installed as `lichen-cli` on VPSes by `setup.sh`. |

**Important:** The `lichen-validator` binary is a single process that includes:
- The validator consensus engine
- The P2P QUIC networking layer
- The JSON-RPC HTTP server
- The WebSocket server
- The threshold signer sidecar (for bridge custody)
- The built-in supervisor/watchdog (auto-restarts on stall)

The RPC and WebSocket are **not** separate binaries — they're spawned as async tasks inside the validator.

### Build All Binaries

```bash
cd lichen
cargo build --release

# Outputs:
# target/release/lichen-validator
# target/release/lichen-custody
# target/release/lichen-faucet
# target/release/lichen          ← CLI binary (installed as lichen-cli)
# target/release/zk-setup        ← ZK key generator
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
  . | ssh <host> 'rm -rf ~/lichen-build && mkdir -p ~/lichen-build && tar -xf - -C ~/lichen-build'
```

---

## Network Architecture

```
┌───────────────────────────────────────────────────────────────────┐
│                      lichen.network DNS                        │
│                                                                   │
│  BACKEND (VPS)                                                    │
│  ──────────────────────────────────────────────────────────       │
│  rpc.lichen.network          →  Round-robin to all 3 VPS      │
│  ws.lichen.network           →  Round-robin to all 3 VPS      │
│  seed-eu.lichen.network      →  EU VPS (direct IP)            │
│  seed-us.lichen.network      →  US VPS (direct IP)            │
│  seed-ap.lichen.network      →  ASIA VPS (direct IP)          │
│  custody.lichen.network      →  US VPS                        │
│  faucet.lichen.network       →  Round-robin to all 3 VPS      │
│                                                                   │
│  STATIC PORTALS (Cloudflare Pages — global CDN edge)              │
│  ──────────────────────────────────────────────────────────       │
│  lichen.network              →  CF Pages (main website)        │
│  explorer.lichen.network     →  CF Pages                      │
│  wallet.lichen.network       →  CF Pages                      │
│  dex.lichen.network          →  CF Pages                      │
│  marketplace.lichen.network  →  CF Pages                      │
│  programs.lichen.network     →  CF Pages                      │
│  developers.lichen.network   →  CF Pages                      │
│  monitoring.lichen.network   →  CF Pages                      │
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
│ Signer     :9201   │    │ Signer     :9201   │    │ Signer     :9201   │
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
        | Seed validators | 3 → 6 | Consensus + bootstrap peers | `seed-*.lichen.network:8001` |
        | RPC relays | 1 → 2 | Stable HTTPS/WSS front door + upstream rotation | `rpc.lichen.network`, `ws.lichen.network` |
        | Custody host | 1 | Custody service + workers | `custody.lichen.network` |
        | Faucet host (testnet) | 3 | Test token distribution (all VPSes) | `faucet.lichen.network` (round-robin) |

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
          - set `rpc.lichen.network` and `ws.lichen.network` to relay host(s) instead of direct validator round-robin
        6. **Scale to seed-04..seed-06**
          - add new validators, update relay upstream pools, update `seeds.json`
        7. **Add relay-02 for HA**
          - run same config as relay-01 and enable DNS/LB failover across relays

        ### Why relay-first for RPC rotation

        - validators keep stable consensus ports while relay absorbs public traffic spikes
        - upstream health checks remove unhealthy validator RPC endpoints automatically
        - clients keep one canonical URL (`rpc.lichen.network`) while backend rotation changes without client config churn

        ### Seed vs relay traffic policy

        - **P2P bootstrap**: always direct to `seed-*` records on port `7001` (testnet) / `8001` (mainnet)
        - **Wallet/app/agent RPC**: route via relays (`rpc.lichen.network`)
        - **WS subscriptions**: route via relays (`ws.lichen.network`) where possible; client-side fallback list remains recommended

        ### Step-by-step checklist (operator execution)

        ```bash
        # 1) Verify all validators are healthy
        for h in seed-01.lichen.network seed-02.lichen.network seed-03.lichen.network; do
          curl -sS -X POST "https://$h" -H 'Content-Type: application/json' \
           -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'
        done

        # 2) Verify relay front door health
        curl -sS -X POST https://rpc.lichen.network -H 'Content-Type: application/json' \
          -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'

        # 3) Verify WS endpoint is reachable (handshake check)
        curl -i https://ws.lichen.network || true
        ```

        When adding seed-04..seed-06, update all relay upstream pools first, then update DNS and `seeds.json`.

---

## DNS & Subdomains

### Cloudflare DNS Records (lichen.network)

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
| **A** | `faucet` | `<US_VPS_IP>` | Proxied | Auto | Faucet round-robin (add all 3) |
| **A** | `faucet` | `<EU_VPS_IP>` | Proxied | Auto | Faucet round-robin |
| **A** | `faucet` | `<ASIA_VPS_IP>` | Proxied | Auto | Faucet round-robin |

### Recommended DNS layout for 3 → 6 validators + relays

Use stable relay hostnames as the only public RPC entrypoint; keep validator hostnames explicit for operational control.

| Record | Example | Purpose |
|---|---|---|
| `seed-01..seed-06` (A, DNS only) | `seed-01.lichen.network` | P2P bootstrap + direct validator diagnostics |
| `rpc-relay-01` / `rpc-relay-02` (A, proxied) | `rpc-relay-01.lichen.network` | RPC relay frontends |
| `ws-relay-01` / `ws-relay-02` (A, DNS only or proxied per plan) | `ws-relay-01.lichen.network` | WS relay frontends |
| `rpc` (CNAME/LB) | `rpc.lichen.network -> rpc-relay-*` | Canonical client RPC URL |
| `ws` (CNAME/LB) | `ws.lichen.network -> ws-relay-*` | Canonical client WS URL |

Operational rule: do not point `rpc`/`ws` directly at every validator once relay is in place; keep relays as the control plane for rotation.

### Static Portals (CNAME → Cloudflare Pages)

All frontend portals are **pure static HTML/CSS/JS** — no server process required. Deploy them to **Cloudflare Pages** for global CDN edge delivery (~20 ms worldwide), zero maintenance, automatic HTTPS, and no VPS dependency.

| Type | Name | Value | Proxy | Notes |
|---|---|---|---|---|
| **CNAME** | `@` | `lichen-website.pages.dev` | Proxied | Main website |
| **CNAME** | `www` | `lichen.network` | Proxied | www redirect |
| **CNAME** | `explorer` | `lichen-explorer.pages.dev` | Proxied | Block explorer |
| **CNAME** | `wallet` | `lichen-wallet.pages.dev` | Proxied | Web wallet |
| **CNAME** | `dex` | `lichen-dex.pages.dev` | Proxied | SporeSwap DEX |
| **CNAME** | `marketplace` | `lichen-marketplace.pages.dev` | Proxied | NFT / skill marketplace |
| **CNAME** | `programs` | `lichen-programs.pages.dev` | Proxied | Programs IDE |
| **CNAME** | `developers` | `lichen-developers.pages.dev` | Proxied | Developer portal & docs |
| **CNAME** | `monitoring` | `lichen-monitoring.pages.dev` | Proxied | Public network dashboard |

> **Why Cloudflare Pages over VPS?**
> - Free tier, unlimited bandwidth, 500 builds/month
> - Served from 300+ edge locations (vs 1 VPS in NYC)
> - Auto-deploy on `git push` — no SSH/scp needed
> - If the VPS goes down, all portals stay live (they don't depend on VPS)
> - Automatic HTTPS, HTTP/3, Brotli compression

**Key points:**
- `seed-*.lichen.network` must be **DNS only** (gray cloud) — P2P QUIC needs direct IP, not Cloudflare proxy
- `ws.lichen.network` must be **DNS only** — Cloudflare free plan doesn't support arbitrary WebSocket
- `rpc.lichen.network` can be **Proxied** (orange cloud) — standard HTTPS works through CF
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

**Testnet:**

| Service | Port | Protocol | Exposed Publicly? |
|---|---|---|---|
| P2P (QUIC) | **7001** | UDP/TCP | YES — other validators must reach this |
| RPC (HTTP) | **8899** | TCP | YES — via Caddy → `rpc.lichen.network` |
| WebSocket | **8900** | TCP | YES — via Caddy → `ws.lichen.network` |
| Threshold Signer | **9201** | TCP | **NO** — loopback only (inter-signer traffic) |
| Custody API | **9105** | TCP | YES — via Caddy → `custody.lichen.network` |
| Faucet API | **9100** | TCP | YES — via Caddy → `faucet.lichen.network` |
| Caddy (HTTPS) | **443** | TCP | YES |
| Caddy (HTTP→HTTPS) | **80** | TCP | YES (redirect only) |

**Mainnet:**

| Service | Port | Protocol | Exposed Publicly? |
|---|---|---|---|
| P2P (QUIC) | **8001** | UDP/TCP | YES — other validators must reach this |
| RPC (HTTP) | **9899** | TCP | YES — via Caddy → `rpc.lichen.network` |
| WebSocket | **9900** | TCP | YES — via Caddy → `ws.lichen.network` |
| Threshold Signer | **9201** | TCP | **NO** — loopback only (inter-signer traffic) |
| Custody API | **9105** | TCP | YES — via Caddy → `custody.lichen.network` |
| Faucet API | — | — | Mainnet has no faucet |
| Caddy (HTTPS) | **443** | TCP | YES |
| Caddy (HTTP→HTTPS) | **80** | TCP | YES (redirect only) |

---

## Day 0 Execution Sheet (copy/paste runbook)

Use this section as your first production worksheet before you own the VPS. Keep naming stable now so every future script/config matches.

### 0.1 Canonical node inventory (recommended naming)

| Role | Hostname | Public DNS | Private DNS (optional) | Example IP placeholder |
|---|---|---|---|---|
| Seed validator 1 (genesis) | `seed-01` | `seed-01.lichen.network` | `seed-01.internal.lichen.network` | `203.0.113.11` |
| Seed validator 2 | `seed-02` | `seed-02.lichen.network` | `seed-02.internal.lichen.network` | `203.0.113.12` |
| Seed validator 3 | `seed-03` | `seed-03.lichen.network` | `seed-03.internal.lichen.network` | `203.0.113.13` |
| Seed validator 4 (future) | `seed-04` | `seed-04.lichen.network` | `seed-04.internal.lichen.network` | `203.0.113.14` |
| Seed validator 5 (future) | `seed-05` | `seed-05.lichen.network` | `seed-05.internal.lichen.network` | `203.0.113.15` |
| Seed validator 6 (future) | `seed-06` | `seed-06.lichen.network` | `seed-06.internal.lichen.network` | `203.0.113.16` |
| RPC relay 1 | `relay-01` | `rpc-relay-01.lichen.network` | `relay-01.internal.lichen.network` | `198.51.100.21` |
| RPC relay 2 (HA) | `relay-02` | `rpc-relay-02.lichen.network` | `relay-02.internal.lichen.network` | `198.51.100.22` |
| Custody + faucet host | `custody-01` | `custody.lichen.network` / `faucet.lichen.network` | `custody-01.internal.lichen.network` | `198.51.100.31` |

### 0.2 Canonical public entrypoints

| Service | Canonical URL | Backing nodes |
|---|---|---|
| JSON-RPC | `https://rpc.lichen.network` | `relay-01`, `relay-02` |
| WebSocket | `wss://ws.lichen.network` | `relay-01`, `relay-02` |
| Custody API | `https://custody.lichen.network` | `custody-01` |
| Faucet (testnet) | `https://faucet.lichen.network` | `custody-01` |

### 0.3 Variable block (fill once, reuse everywhere)

```bash
# Domain
export DOMAIN="lichen.network"

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
rpc  CNAME  rpc-relay-01.lichen.network  (or LB pool relay-01/02)
ws   CNAME  rpc-relay-01.lichen.network  (or LB pool relay-01/02)

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
6. verify `https://rpc.lichen.network` health
7. deploy custody/faucet on `custody-01`
8. scale with `seed-04..06` + `relay-02`, then update relay upstream pools

### 0.6 Day 0 verification commands

```bash
curl -sS -X POST "https://rpc.lichen.network" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'

curl -sS "https://custody.lichen.network/health"
curl -sS "https://faucet.lichen.network/health"

for host in seed-01.lichen.network seed-02.lichen.network seed-03.lichen.network; do
  curl -sS -X POST "https://$host" -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'
done
```

---

## Phase 1 — Genesis on Local Machine

The simplest approach: generate genesis locally, copy state to VPS.

### Step 1: Generate Genesis Locally

```bash
cd lichen
cargo build --release

# Start the first validator — it auto-generates genesis
./target/release/lichen-validator --network testnet --p2p-port 7001
```

On first boot with no existing state, the validator:
1. Generates a **genesis wallet** with multi-sig (2/3 for testnet, 3/5 for mainnet)
2. Creates treasury keypairs in `./data/state-7001/genesis-keys/`
3. Saves `genesis-wallet.json` in the state directory
4. Creates the canonical 500M LICN genesis distribution across the configured treasury wallets
5. Registers itself as the initial validator
6. **Auto-deploys all 30 genesis contracts** from the `contracts/` directory

Step 6 is important — the validator binary includes a `genesis_auto_deploy` function that reads every compiled `.wasm` from the `contracts/` directory and deploys them into chain state on the first block. This means **no separate deploy script is needed**. The full catalog:

| Category | Contracts deployed |
|---|---|
| Core token | LICN (LichenCoin) |
| Wrapped tokens | LUSD, WSOL, WETH |
| DEX | DEX Core, AMM, Router, Margin, Rewards, Governance, Analytics |
| DeFi | LichenSwap, LichenBridge, ThallLend |
| Marketplace | LichenMarket, LichenAuction, LichenOracle, LichenDAO |
| NFT / Identity | LichenPunks, LichenID |
| Infrastructure | SporePay, SporePump, SporeVault, BountyBoard, Compute Market, Moss Storage |

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
rm /var/lib/lichen/state-testnet/signer-keypair.json  # Regenerated on boot
# The validator identity is derived from the --keypair flag or auto-generated
```

### Alternative: Start Genesis Directly on US VPS

If you prefer to skip the local step:

```bash
# SSH into US VPS, build there, start the validator
./lichen-validator --network testnet --listen-addr 0.0.0.0 --p2p-port 7001
# Let it generate genesis
# Then tar + scp the state to EU/ASIA
```

---

## Phase 2 — Deploy VPS Seed Validators

### On Each VPS: System Setup

Run on all 3 VPS (Ubuntu 22.04+):

```bash
# 1. Create system user
sudo groupadd -r lichen
sudo useradd -r -g lichen -d /home/lichen -m -s /bin/false lichen

# 2. Create directories
sudo mkdir -p /opt/lichen/bin /var/lib/lichen /var/log/lichen /etc/lichen
sudo chown lichen:lichen /var/lib/lichen /var/log/lichen

# 3. Install Rust & build (or scp pre-built binaries)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# Clone and build
git clone <your-repo> /tmp/lichen-build
cd /tmp/lichen-build
cargo build --release

# Copy binaries
sudo cp target/release/lichen-validator /opt/lichen/bin/
sudo cp target/release/lichen-custody /opt/lichen/bin/  # US VPS only
sudo cp target/release/lichen-faucet /opt/lichen/bin/   # All VPSes
sudo cp target/release/licn /opt/lichen/bin/               # CLI (optional)
sudo chmod +x /opt/lichen/bin/*

# 4. Extract genesis state
sudo mkdir -p /var/lib/lichen/state-testnet
sudo tar xzf /tmp/genesis-state.tar.gz -C /var/lib/lichen/state-testnet
sudo chown -R lichen:lichen /var/lib/lichen
```

### ZK Verification Keys (Auto-Generated)

Starting with v0.2.9, the validator **auto-generates** ZK verification keys on first
startup if they are not already present. The Groth16 trusted setup for 3 circuits
(shield, unshield, transfer) completes in ~30 seconds and produces 6 key files cached
at `~/.lichen/zk/` (for the user running the validator, typically
`/var/lib/lichen/.lichen/zk/` for the `lichen` systemd user).

**No manual ZK setup is required.** The validator handles it automatically.

#### Critical: File Ownership

All files under `/var/lib/lichen/` **must** be owned by `lichen:lichen`.
If you manually copy ZK keys (e.g. via rsync as the `ubuntu` user), fix ownership
immediately:

```bash
sudo chown -R lichen:lichen /var/lib/lichen/.lichen/
```

**If ownership is wrong,** the validator cannot read the key files and shielded
transactions will fail silently with a log warning. Always verify after any manual
file operation:

```bash
ls -la /var/lib/lichen/.lichen/zk/
# All files should show: lichen lichen
```

#### Manual ZK Key Copy (optional, saves ~30s startup)

If you want to skip auto-generation (e.g. deploying to many nodes at once), copy
pre-generated keys from a working node:

```bash
# From a node that already has keys:
rsync -avz -e "ssh -p 2222" \
    /var/lib/lichen/.lichen/zk/ \
    ubuntu@<TARGET_VPS>:/var/lib/lichen/.lichen/zk/

# THEN fix ownership on the target:
ssh -p 2222 ubuntu@<TARGET_VPS> \
    "sudo chown -R lichen:lichen /var/lib/lichen/.lichen/"
```

### On Each VPS: Validator Service

```bash
# Create env file
sudo tee /etc/lichen/env-testnet <<'EOF'
LICHEN_NETWORK=testnet
LICHEN_RPC_PORT=8899
LICHEN_WS_PORT=8900
LICHEN_P2P_PORT=7001
LICHEN_SIGNER_BIND=127.0.0.1:9201
LICHEN_SIGNER_AUTH_TOKEN=<shared-signer-token>
RUST_LOG=info
# LICHEN_ADMIN_TOKEN=<generate-with-openssl-rand-hex-32>
EOF
sudo chmod 600 /etc/lichen/env-testnet
```

```bash
# Create systemd service
sudo tee /etc/systemd/system/lichen-validator.service <<'EOF'
[Unit]
Description=Lichen Validator Node
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=lichen
Group=lichen

ExecStart=/opt/lichen/bin/lichen-validator \
    --network testnet \
    --listen-addr 0.0.0.0 \
    --rpc-port 8899 \
    --ws-port 8900 \
    --p2p-port 7001 \
    --db-path /var/lib/lichen/state-testnet \
    --bootstrap-peers seed-us.lichen.network:7001,seed-eu.lichen.network:7001,seed-ap.lichen.network:7001

Restart=on-failure
RestartSec=5
LimitNOFILE=65536

# Security
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=/var/lib/lichen /var/log/lichen
PrivateTmp=true

# Environment
Environment=RUST_LOG=info
EnvironmentFile=/etc/lichen/env-testnet

StandardOutput=journal
StandardError=journal
SyslogIdentifier=lichen-validator

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
sudo systemctl enable lichen-validator
sudo systemctl start lichen-validator

# Watch logs:
sudo journalctl -u lichen-validator -f
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
sudo tee /etc/systemd/system/lichen-custody.service <<'EOF'
[Unit]
Description=Lichen Custody Bridge
After=lichen-validator.service
Wants=lichen-validator.service

[Service]
Type=simple
User=lichen
Group=lichen
WorkingDirectory=/opt/lichen

ExecStart=/opt/lichen/bin/lichen-custody
Restart=always
RestartSec=5

# Core
Environment=CUSTODY_DB_PATH=/var/lib/lichen/custody-db
Environment=CUSTODY_POLL_INTERVAL_SECS=15
Environment=CUSTODY_DEPOSIT_TTL_SECS=86400
Environment=RUST_LOG=info

# Lichen connection
# CRITICAL: This keypair must match the one used to deploy_dex.py (the "admin" key).
# Copy keypairs/deployer.json to this path after deployment.
Environment=CUSTODY_LICHEN_RPC_URL=http://127.0.0.1:8899
Environment=CUSTODY_TREASURY_KEYPAIR=/etc/lichen/custody-treasury-testnet.json

# Wrapped token contracts (auto-discovered from registry, or pin manually)
Environment=CUSTODY_LUSD_TOKEN_ADDR=<deploy-and-fill>
Environment=CUSTODY_WSOL_TOKEN_ADDR=<deploy-and-fill>
Environment=CUSTODY_WETH_TOKEN_ADDR=<deploy-and-fill>
Environment=CUSTODY_WBNB_TOKEN_ADDR=<deploy-and-fill>

# Solana bridge
Environment=CUSTODY_SOLANA_RPC_URL=https://api.mainnet-beta.solana.com
Environment=CUSTODY_TREASURY_SOLANA=<your-solana-address>
Environment=CUSTODY_SOLANA_FEE_PAYER=/etc/lichen/solana-fee-payer.json

# Threshold signers (all 3 VPS)
Environment=CUSTODY_SIGNER_ENDPOINTS=http://<US_PRIVATE_IP>:9201,http://<EU_PRIVATE_IP>:9201,http://<ASIA_PRIVATE_IP>:9201
Environment=CUSTODY_SIGNER_AUTH_TOKEN=<shared-signer-token>

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable lichen-custody
sudo systemctl start lichen-custody
```

### Signer Network

The custody service calls each validator's threshold signer on port 9201. These must be reachable between the VPS nodes. Options:
- **WireGuard VPN** between the 3 VPS (recommended)
- **Firewall allow-list** — only allow port 9201 from the other 2 VPS IPs
- Never expose port 9201 to the public internet
- Use the same secret in `LICHEN_SIGNER_AUTH_TOKEN` on validators and `CUSTODY_SIGNER_AUTH_TOKEN` in custody

---

## Phase 4 — Faucet Service

### Faucet (all 3 VPSes, testnet only)

The faucet runs as a Rust binary on **all 3 VPSes**. DNS for `faucet.lichen.network` round-robins to all 3, so every VPS must have the faucet running with the **same keypair**.

#### 1. Copy faucet keypair to all VPSes

Genesis creates the faucet keypair at `state-testnet/genesis-keys/faucet-<chain-id>.json`. Copy it to the expected service path on **each** VPS:

```bash
sudo cp /var/lib/lichen/state-testnet/genesis-keys/faucet-lichen-testnet-1.json \
       /var/lib/lichen/faucet-keypair-testnet.json
sudo chown lichen:lichen /var/lib/lichen/faucet-keypair-testnet.json
sudo chmod 600 /var/lib/lichen/faucet-keypair-testnet.json
```

#### 2. Create systemd service (on each VPS)

```bash
sudo tee /etc/systemd/system/lichen-faucet.service <<'EOF'
[Unit]
Description=Lichen Faucet Service
After=lichen-validator-testnet.service
Wants=lichen-validator-testnet.service

[Service]
Type=simple
User=lichen
Group=lichen
WorkingDirectory=/var/lib/lichen
ExecStart=/usr/local/bin/lichen-faucet
Restart=always
RestartSec=5

Environment=PORT=9100
Environment=RPC_URL=http://127.0.0.1:8899
Environment=NETWORK=testnet
Environment=MAX_PER_REQUEST=10
Environment=DAILY_LIMIT_PER_IP=150
Environment=COOLDOWN_SECONDS=60
Environment=AIRDROPS_FILE=/var/lib/lichen/airdrops.json
Environment=FAUCET_KEYPAIR=/var/lib/lichen/faucet-keypair-testnet.json
Environment=RUST_LOG=info
Environment=TRUSTED_PROXY=127.0.0.1,::1

NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/lichen

StandardOutput=journal
StandardError=journal
SyslogIdentifier=lichen-faucet

[Install]
WantedBy=multi-user.target
EOF
sudo systemctl daemon-reload
sudo systemctl enable lichen-faucet
sudo systemctl start lichen-faucet
```

#### 3. Copy faucet UI files (Caddy serves them alongside the API)

```bash
sudo mkdir -p /opt/lichen/www/faucet-ui
rsync -a faucet/ /opt/lichen/www/faucet-ui/
```

The faucet frontend is also deployed to **Cloudflare Pages** (`lichen-faucet` project):

```bash
npx wrangler pages deploy faucet/ --project-name lichen-network-faucet --commit-dirty=true
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
| `lichen-website` | `Lichen/lichen` | `website` | `lichen.network` |
| `lichen-explorer` | `Lichen/lichen` | `explorer` | `explorer.lichen.network` |
| `lichen-wallet` | `Lichen/lichen` | `wallet` | `wallet.lichen.network` |
| `lichen-dex` | `Lichen/lichen` | `dex` | `dex.lichen.network` |
| `lichen-marketplace` | `Lichen/lichen` | `marketplace` | `marketplace.lichen.network` |
| `lichen-programs` | `Lichen/lichen` | `programs` | `programs.lichen.network` |
| `lichen-developers` | `Lichen/lichen` | `developers` | `developers.lichen.network` |
| `lichen-monitoring` | `Lichen/lichen` | `monitoring` | `monitoring.lichen.network` |
| `lichen-faucet` | `Lichen/lichen` | `faucet` | *(none — VPS Caddy serves frontend; Pages is backup)* |

For each project:

1. **Connect to Git** → Select the `Lichen/lichen` repo
2. **Framework preset** → `None` (these are plain static files, no build step)
3. **Build command** → *(leave empty)*
4. **Build output directory** → Set to the subdirectory (e.g. `wallet`)
5. **Deploy** → Cloudflare builds and publishes to `<project>.pages.dev`

### Step 2: Add Custom Domains

For each Pages project, go to **Custom domains** → **Set up a custom domain** → enter the subdomain (e.g. `wallet.lichen.network`). Cloudflare automatically:
- Creates the CNAME DNS record
- Provisions an SSL certificate
- Routes traffic to the Pages deployment

If you prefer to set DNS manually:

```
# In Cloudflare DNS → lichen.network zone
CNAME  explorer     lichen-explorer.pages.dev     (Proxied)
CNAME  wallet       lichen-wallet.pages.dev       (Proxied)
CNAME  dex          lichen-dex.pages.dev          (Proxied)
CNAME  marketplace  lichen-marketplace.pages.dev  (Proxied)
CNAME  programs     lichen-programs.pages.dev     (Proxied)
CNAME  developers   lichen-developers.pages.dev   (Proxied)
CNAME  monitoring   lichen-monitoring.pages.dev   (Proxied)
```

For the apex domain (`lichen.network`):
```
# Cloudflare supports CNAME flattening at the apex
CNAME  @            lichen-website.pages.dev      (Proxied)
CNAME  www          lichen.network                (Proxied)
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
wrangler pages deploy wallet/ --project-name=lichen-network-wallet
wrangler pages deploy explorer/ --project-name=lichen-network-explorer
wrangler pages deploy website/ --project-name=lichen-network-website
# ... etc for each portal (all use lichen-network-<portal> prefix)
```

### Step 4: Verify Deployments

After deploying, each portal is available at both the `*.pages.dev` URL and the custom domain:

```bash
# Check Pages URLs (available immediately)
curl -sI https://lichen-wallet.pages.dev | head -3
curl -sI https://lichen-explorer.pages.dev | head -3

# Check custom domains (may take a few minutes for DNS + SSL)
curl -sI https://wallet.lichen.network | head -3
curl -sI https://explorer.lichen.network | head -3
```

### Frontend Configuration Checklist

All portal frontends have RPC/WS endpoint configuration that defaults to `localhost`. Before deploying to production, verify the network configs point to live URLs. Most apps have a multi-network config pattern (stored in `localStorage`) that already includes the production URLs.

| Portal | Config file | Has multi-network? | Production URLs in config? | Notes |
|---|---|---|---|---|
| **wallet** | `wallet/js/wallet.js` | YES | YES (`rpc.lichen.network`) | Gold standard — also has custody endpoints |
| **explorer** | `explorer/js/explorer.js` | YES | YES | `transaction.js` L110 has hardcoded faucet URL — needs fix |
| **marketplace** | `marketplace/js/marketplace-config.js` | YES | YES | Clean — mirrors wallet pattern |
| **website** | `website/script.js` | YES | YES | Works, minor local-mainnet port mismatch |
| **monitoring** | `monitoring/js/monitoring.js` | YES | YES (RPC) | `VALIDATOR_RPCS` array is hardcoded to local ports — update for prod |
| **programs** | `programs/js/lichen-sdk.js` | YES | YES | `landing.js` only has 2-way auto-detect (local vs testnet) |
| **dex** | `dex/dex.js` | PARTIAL | NO | Uses `window.LICHEN_RPC` override — network selector not wired up |
| **faucet UI** | `faucet/faucet.js` | NO | NO | Hardcoded `localhost:9100` — needs full rewrite for prod |
| **shared** | `shared/wallet-connect.js` | Delegates | — | Fallback port `9000` should be `8899` |

**Config fixes needed before production:**

1. `faucet/faucet.js` — Change `FAUCET_API` from `http://localhost:9100` to auto-detect (`/faucet` relative path when served by Caddy, or `https://faucet.lichen.network`)
2. `dex/dex.js` — Wire up the existing `<select id="networkSelect">` to switch `LICHEN_RPC`/`LICHEN_WS`
3. `monitoring/js/monitoring.js` — Make `VALIDATOR_RPCS` configurable per-network (seed-us/eu/ap for prod)
4. `explorer/js/transaction.js` L110 — Replace hardcoded `localhost:9100` faucet URL
5. `programs/js/landing.js` — Add mainnet to the auto-detect (currently only local vs testnet)
6. `shared/wallet-connect.js` — Fix fallback port from `9000` to `8899`

---

## Phase 6 — Agent Validators Join

When an agent on a human's machine wants to run a validator:

### What the Agent Needs

1. The **Lichen binary** (`lichen-validator`)
2. The **seeds.json** file (or use `--bootstrap-peers` flag)
3. Enough LICN to stake (100,000 LICN minimum)

### Agent Start Command

```bash
# The agent runs this on the human's local machine:
./lichen-validator \
    --network testnet \
    --bootstrap-peers seed-us.lichen.network:8001,seed-eu.lichen.network:8001,seed-ap.lichen.network:8001
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
- Accept inbound connections from other peers (unless they open port 7001/8001 on their router)
- Act as a full relay/seed (they're a "leaf" node)

This is fine — only the 3 VPS need to be fully reachable. Agent validators are consumers of the seed network.

---

## seeds.json — Production Version

Update `seeds.json` with your real VPS IPs/domains:

```json
{
  "testnet": {
    "network_id": "lichen-testnet-1",
    "chain_id": "lichen-testnet-1",
    "seeds": [
      {
        "id": "seed-us",
        "address": "seed-us.lichen.network:8001",
        "region": "us-east",
        "operator": "Lichen Foundation"
      },
      {
        "id": "seed-eu",
        "address": "seed-eu.lichen.network:8001",
        "region": "eu-west",
        "operator": "Lichen Foundation"
      },
      {
        "id": "seed-ap",
        "address": "seed-ap.lichen.network:8001",
        "region": "ap-southeast",
        "operator": "Lichen Foundation"
      }
    ],
    "bootstrap_peers": [
      "seed-us.lichen.network:8001",
      "seed-eu.lichen.network:8001",
      "seed-ap.lichen.network:8001"
    ],
    "rpc_endpoints": [
      "https://rpc.lichen.network"
    ]
  },
  "mainnet": {
    "network_id": "lichen-mainnet-1",
    "chain_id": "lichen-mainnet-1",
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
rpc.lichen.network  A  <US_IP>    TTL=300
rpc.lichen.network  A  <EU_IP>    TTL=300
rpc.lichen.network  A  <ASIA_IP>  TTL=300
```

DNS returns all 3 IPs in random order. Clients use whichever comes first. This provides:
- **Geographic diversity** — users get routed to closest responding server
- **Basic failover** — if one IP goes down, clients try the next one
- **Even distribution** — approximately 1/3 traffic to each

### Health-Check Load Balancing (Cloudflare Pro — $20/mo)

Add a Cloudflare Load Balancer on `rpc.lichen.network` with:
- Pool: all 3 VPS IPs
- Health check: `GET /health` on port 443
- Steering: Geo (US→US VPS, EU→EU VPS, AP→ASIA VPS)
- Failover: If one VPS is down, route to nearest healthy one

This is the proper production setup but round-robin works fine to start.

### RPC rotation model (recommended)

For 3+ validators, use **two-layer rotation**:

1. **Layer 1 (public):** `rpc.lichen.network` load-balances across `rpc-relay-01` and `rpc-relay-02`
2. **Layer 2 (inside relay):** each relay load-balances across validator RPC upstreams (`seed-01..seed-06`)

Benefits:

- fast failover if a validator RPC stalls
- no client reconfiguration when adding/removing validators
- easier maintenance windows (drain a single upstream)

### Client-side fallback rotation (agents/wallets)

Even with relay, keep explicit fallback endpoints in clients:

```bash
RPC_ENDPOINTS=(
  "https://rpc.lichen.network"
  "https://rpc-relay-01.lichen.network"
  "https://rpc-relay-02.lichen.network"
  "https://seed-01.lichen.network"
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
rpc.lichen.network {
    reverse_proxy localhost:8899
}

# WebSocket
ws.lichen.network {
    reverse_proxy localhost:8900
}

# Custody Bridge
custody.lichen.network {
    reverse_proxy localhost:9105
}

# Faucet API + UI
faucet.lichen.network {
    # API routes
    handle /faucet/* {
        reverse_proxy localhost:9100
    }
    handle /health {
        reverse_proxy localhost:9100
    }
    # Static UI (faucet is special — served from VPS because it needs its API co-located)
    handle {
        root * /opt/lichen/www/faucet-ui
        file_server
    }
}
```

> **Note:** No static portal blocks needed — explorer, wallet, DEX, marketplace, programs, developers, monitoring, and the main website are all served by Cloudflare Pages.

### Caddyfile — EU & ASIA VPS (Validator Only)

```
# /etc/caddy/Caddyfile

rpc.lichen.network {
    reverse_proxy localhost:8899
}

ws.lichen.network {
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

rpc.lichen.network {
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

ws.lichen.network {
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
# Testnet: 7001, Mainnet: 8001
sudo ufw allow 7001/tcp
sudo ufw allow 7001/udp
# sudo ufw allow 8001/tcp   # Uncomment for mainnet
# sudo ufw allow 8001/udp   # Uncomment for mainnet

# Threshold Signer — ONLY from other VPS IPs (port 9201)
sudo ufw allow from <US_VPS_IP> to any port 9201 proto tcp
sudo ufw allow from <EU_VPS_IP> to any port 9201 proto tcp
sudo ufw allow from <ASIA_VPS_IP> to any port 9201 proto tcp

# Enable
sudo ufw enable
```

**Never expose these to the public:**
- Port 9201 (threshold signer)
- Port 8899/9899 directly (should go through Caddy HTTPS)
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
# /opt/lichen/bin/healthcheck.sh
# Run via cron every 5 minutes

RPC="http://localhost:8899"
HEALTH=$(curl -sf "$RPC" -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' 2>&1)

if [ $? -ne 0 ]; then
    echo "$(date) ALERT: Validator RPC not responding" >> /var/log/lichen/healthcheck.log
    sudo systemctl restart lichen-validator
fi

# Check slot is advancing
SLOT=$(curl -sf "$RPC" -d '{"jsonrpc":"2.0","id":1,"method":"getSlot"}' | jq -r '.result')
LAST_SLOT=$(cat /tmp/lichen-last-slot 2>/dev/null || echo "0")
echo "$SLOT" > /tmp/lichen-last-slot

if [ "$SLOT" = "$LAST_SLOT" ]; then
    echo "$(date) WARN: Slot not advancing (stuck at $SLOT)" >> /var/log/lichen/healthcheck.log
fi
```

```bash
# Add to crontab
echo "*/5 * * * * /opt/lichen/bin/healthcheck.sh" | sudo crontab -u lichen -
```

---

## Backup & Recovery

### What to Back Up

| What | Where | Frequency | Critical? |
|---|---|---|---|
| Genesis keys | `/var/lib/lichen/state-testnet/genesis-keys/` | Once (keep offline) | **YES** — controls treasury |
| Genesis wallet | `/var/lib/lichen/state-testnet/genesis-wallet.json` | Once | YES |
| Blockchain state | `/var/lib/lichen/state-testnet/` | Daily | Medium — can re-sync from peers |
| Custody DB | `/var/lib/lichen/custody-db/` | Hourly | YES — deposit/withdrawal state |
| Airdrops file | `/var/lib/lichen/airdrops.json` | Daily | Low |

### Key Material Retention Policy (online vs offline)

Keep this policy strict; do not delete genesis keys after deployment.

| Material | Keep online? | Keep offline? | Notes |
|---|---|---|---|
| `genesis-keys/` | **Minimal** (only where strictly required) | **Yes, mandatory** (multiple encrypted backups) | Root treasury/admin control; loss is catastrophic |
| `genesis-wallet.json` | Optional | Yes | Metadata/reference; not a replacement for keys |
| validator identity keypair | Yes (on each validator host) | Yes | One keypair per validator; never reused across nodes |
| signer keypair (`:9201`) | Yes (only on signer-enabled nodes) | Yes | Restrict access to private network + file perms |
| custody treasury keypair | Yes (custody host only) | Yes | Treat as production hot wallet material |

Minimum controls:

- file owner `lichen:lichen`, mode `600` for private key files
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
find /var/lib/lichen -type f \( -name "*key*" -o -name "*wallet*.json" \) -exec ls -l {} \;

# TLS endpoint check
curl -I https://rpc.lichen.network
curl -I https://custody.lichen.network
```

If any item fails, block production rollout until fixed.

### Backup Script

```bash
#!/bin/bash
# /opt/lichen/bin/backup.sh
DATE=$(date +%Y%m%d-%H%M)
BACKUP_DIR="/opt/lichen/backups"
mkdir -p "$BACKUP_DIR"

# Snapshot RocksDB (safe to copy while running)
tar czf "$BACKUP_DIR/state-$DATE.tar.gz" -C /var/lib/lichen/state-testnet .
tar czf "$BACKUP_DIR/custody-$DATE.tar.gz" -C /var/lib/lichen/custody-db . 2>/dev/null

# Keep last 7 days
find "$BACKUP_DIR" -name "*.tar.gz" -mtime +7 -delete
```

### Recovery: If a VPS Dies

1. Spin up a new VPS
2. Install binaries (or build from source)
3. Either:
   - **Restore from backup:** Extract state tarball to `/var/lib/lichen/state-testnet/`
   - **Sync from peers:** Start with empty state + `--bootstrap-peers <other-2-VPS>`
4. Update DNS to point to new IP
5. Update `seeds.json` if IP changed

---

## Troubleshooting

### ZK keys: "Failed reading shield VK" or shielded pool warnings

**Cause:** ZK verification keys are missing or unreadable. Most common when:
1. Keys haven't been generated yet (pre-v0.2.9 — now auto-generated)
2. File ownership is wrong (e.g. files owned by `ubuntu` instead of `lichen`)

**Fix:**

```bash
# Check the ZK directory exists and has correct ownership
ls -la /var/lib/lichen/.lichen/zk/
# Expected: 6 files (vk_shield.bin, pk_shield.bin, vk_unshield.bin, pk_unshield.bin,
#           vk_transfer.bin, pk_transfer.bin), all owned by lichen:lichen

# If files exist but wrong owner:
sudo chown -R lichen:lichen /var/lib/lichen/.lichen/

# If files are missing, restart the validator — v0.2.9+ auto-generates them:
sudo systemctl restart lichen-validator-mainnet
```

**Prevention:** Never rsync/scp files to `/var/lib/lichen/` as a non-lichen user
without fixing ownership afterward. Always run:
```bash
sudo chown -R lichen:lichen /var/lib/lichen/.lichen/
```

### "Seed peers found — will sync genesis from the existing network"

**Cause:** The validator sees `bootstrap_peers` in `seeds.json` and enters sync mode instead of creating genesis. This happens when you start a validator on a freshly wiped VPS where no network exists yet.

**Fix:** Use `lichen-start.sh` instead of `systemctl start`. The start script runs `lichen-genesis` before starting the validator when it detects an empty state directory.

```bash
# ✅ Correct — creates genesis first, then starts validator
bash lichen-start.sh testnet

# ❌ Wrong — tries to sync from non-existent network
sudo systemctl start lichen-validator-testnet
```

### "No genesis block found and no seed peers available"

**Cause:** The validator has no state AND no seeds to sync from (seeds.json missing or empty).

**Fix:** Make sure `seeds.json` exists in the working directory, then use `lichen-start.sh`.

### Oracle: "Binance WebSocket connect failed: HTTP error: 451"

**Cause:** The US VPS is geo-blocked from `binance.com`. The oracle defaults to binance.com and gets HTTP 451 "Unavailable For Legal Reasons".

**Fix:** Set the oracle env vars to use `binance.us` before starting the validator:

```bash
export LICHEN_ORACLE_WS_URL="wss://stream.binance.us:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade"
export LICHEN_ORACLE_REST_URL="https://api.binance.us/api/v3/ticker/price?symbols=%5B%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22%5D"
bash lichen-start.sh testnet
```

The EU VPS does not need these — it can reach `binance.com` directly.

### Validators can't find each other

```bash
# Check P2P is listening on 0.0.0.0 (not 127.0.0.1)
sudo ss -tlnp | grep 8000
# Should show: 0.0.0.0:7001 (testnet) or 0.0.0.0:8001 (mainnet)

# If it shows 127.0.0.1:7001, add --listen-addr 0.0.0.0 to the systemd service
```

### TOFU fingerprint violations

**Cause:** Lichen P2P uses Trust-On-First-Use (TOFU) for TLS peer identity.
If a VPS regenerates its P2P certificate (e.g. after a state wipe) and other
nodes have the old fingerprint cached, connections are rejected.

**Fix (v0.2.9+):** Seed/bootstrap peers are automatically treated as reserved
peers, which auto-accept fingerprint rotations. No manual action needed for
the 3 seed nodes.

**Fix (stale local cache):** If a local validator rejects known-good seed peers,
delete the cached fingerprints:

```bash
# Find and remove the TOFU fingerprint cache
find ~/.lichen/ -name "peer_fingerprints.json" -delete
# Or for the lichen systemd user:
sudo find /var/lib/lichen/.lichen/ -name "peer_fingerprints.json" -delete
```

### Blocks not producing

```bash
# Check validator count
curl -s localhost:8899 -d '{"jsonrpc":"2.0","id":1,"method":"getValidators"}' | jq '.result | length'

# Check slot is advancing
curl -s localhost:8899 -d '{"jsonrpc":"2.0","id":1,"method":"getSlot"}'

# Check logs for errors
journalctl -u lichen-validator --since "10 min ago" | grep -i error
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
curl -s http://<OTHER_VPS_IP>:9201/health
```

### Faucet: "Treasury keypair not configured — cannot sign airdrop transactions"

**Cause:** The faucet HTTP service itself does NOT sign transactions. It calls the validator's `requestAirdrop` RPC method, which signs using the treasury keypair loaded at validator startup. If the validator can't find the treasury keypair file, `treasury_keypair` is `None` in the RPC state, and every airdrop request fails with this error.

**How treasury keypair loading works:**

The validator loads the treasury keypair at boot via `load_treasury_keypair()`, which checks two locations in order:

1. **`genesis-wallet.json` → `treasury_keypair_path`** — the path stored in the genesis wallet JSON, resolved **relative to the data directory** (e.g., `data/state-testnet/genesis-keys/treasury-lichen-testnet-1.json`)
2. **Fallback:** `{data_dir}/genesis-keys/treasury-{chain_id}.json` — direct lookup in the genesis-keys directory

If neither file exists, the validator starts without a treasury keypair and all airdrop requests fail.

**Diagnosis:**

```bash
# Check if the treasury keypair file exists
ls -la ~/lichen/data/state-testnet/genesis-keys/treasury-lichen-testnet-1.json

# Check if the validator loaded the treasury keypair (should show a pubkey, not null)
curl -s http://localhost:8899 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getTreasuryInfo","params":[]}' | python3 -m json.tool

# Test airdrop directly via RPC (amount is in LICN, 1-10 range, address is base58)
curl -s http://localhost:8899 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"requestAirdrop","params":["<BASE58_ADDRESS>",5]}'

# Test faucet service
curl -s http://localhost:9100/faucet/request -H 'Content-Type: application/json' \
  -d '{"address":"<BASE58_ADDRESS>"}'
```

**Fix:**

1. **If `getTreasuryInfo` returns a pubkey** — the RPC has the treasury loaded. The faucet service itself may have a connection issue (check `RPC_URL` env var, ensure it points to `http://127.0.0.1:8899`).

2. **If `getTreasuryInfo` fails or returns no pubkey** — the treasury keypair file is missing or the validator can't find it:
   ```bash
   # Verify the genesis-keys directory exists with the treasury file
   ls -la ~/lichen/data/state-testnet/genesis-keys/treasury-*.json

   # For joining VPSes (EU/SEA), copy the treasury key from the genesis VPS (US):
   ssh -p 2222 ubuntu@15.204.229.189 "cat ~/lichen/data/state-testnet/genesis-keys/treasury-lichen-testnet-1.json" \
     | ssh -p 2222 ubuntu@<JOINING_VPS> "mkdir -p ~/lichen/data/state-testnet/genesis-keys && cat > ~/lichen/data/state-testnet/genesis-keys/treasury-lichen-testnet-1.json"

   # Restart the validator to reload the treasury keypair
   # (kill existing process, then re-run lichen-start.sh)
   ```

3. **If the file exists but the validator still doesn't load it** — check the validator logs for warnings about treasury keypair parsing:
   ```bash
   grep -i treasury /tmp/lichen-testnet/validator.log | tail -20
   ```

**Key facts:**
- `requestAirdrop` RPC params are `[base58_address, amount_in_licn]` where amount is 1-10 (whole LICN, not spores)
- Faucet runs on port 9100 (testnet only — panics if `NETWORK=mainnet`)
- The faucet keypair (`faucet-lichen-testnet-1.json`) is used for identity/logging, NOT for signing transactions
- Only the genesis VPS (US) has genesis-keys after initial creation — EU/SEA must have treasury keys copied over before the validator can sign airdrops

### Faucet: stale manual process blocking port 9100

**Cause:** A previously manually-started faucet process (`nohup ./target/release/lichen-faucet`) is still running and holds port 9100. Systemd or a new faucet instance can't bind to the port.

**Diagnosis:**

```bash
# Check what's on port 9100
sudo ss -tlnp | grep 9100

# Check for stale faucet processes
pgrep -af lichen-faucet
```

**Fix:**

```bash
# Kill the stale process
kill <PID>

# Start fresh using the documented method (not systemd — use nohup with proper env vars)
cd ~/lichen
PORT=9100 RPC_URL=http://127.0.0.1:8899 NETWORK=testnet \
  nohup ./target/release/lichen-faucet > /tmp/lichen-testnet/faucet.log 2>&1 &
```

---

## Data Directory Architecture

> **v0.2.19+** — All paths are resolved from a single canonical data directory. No CWD-dependent relative paths, no HOME overrides for key resolution.

### Single Source of Truth: `--db-path`

Every validator instance has exactly ONE data directory, set by `--db-path` (aliases: `--db`, `--data-dir`). All state, keys, logs, and configs live under this directory:

```
{data_dir}/                              # e.g., ~/lichen/data/state-testnet/
├── CURRENT, MANIFEST-*, *.sst           # RocksDB state files
├── genesis-wallet.json                  # Genesis wallet config (pubkeys + key paths)
├── genesis-keys/                        # All keypairs generated at genesis
│   ├── genesis-primary-{chain_id}.json  # Genesis primary signer
│   ├── genesis-signer-{n}-{chain_id}.json  # Additional multi-sig signers
│   ├── treasury-{chain_id}.json         # Treasury keypair (used by RPC for airdrops)
│   ├── faucet-{chain_id}.json           # Faucet identity keypair
│   ├── validator_rewards-{chain_id}.json
│   ├── community_treasury-{chain_id}.json
│   ├── builder_grants-{chain_id}.json
│   ├── founding_symbionts-{chain_id}.json
│   ├── ecosystem_partnerships-{chain_id}.json
│   └── reserve_pool-{chain_id}.json
├── validator-keypair.json               # This validator's identity keypair
├── known-peers.json                     # Cached P2P peer list
├── home/                                # Validator runtime home (P2P identity isolation)
│   └── .lichen/                      # P2P certs, TOFU fingerprints
├── logs/                                # Rolling daily log files
│   └── validator.YYYY-MM-DD.log
└── seeds.json                           # (optional) Seed peers override
```

### Path Resolution Rules

1. **`genesis-wallet.json` paths** — The `treasury_keypair_path` and `keypair_path` fields are stored as paths relative to the data directory. At load time, the validator resolves them as `{data_dir}/{path}`. Example: `genesis-keys/treasury-lichen-testnet-1.json` resolves to `{data_dir}/genesis-keys/treasury-lichen-testnet-1.json`.

2. **Validator identity keypair** — Resolved by `keypair_loader` with a 5-tier search:
   1. Explicit `--keypair` CLI argument
   2. `{data_dir}/validator-keypair.json`
   3. `$LICHEN_REAL_HOME/.lichen/validators/validator-{network}.json`
   4. Legacy: `$LICHEN_REAL_HOME/.lichen/validators/validator-{port}.json`
   5. Auto-generate new keypair (saved to both data_dir and shared HOME)

3. **Seeds** — Searched in order: `{data_dir}/seeds.json`, `/etc/lichen/seeds.json`, `./seeds.json` (CWD)

4. **ZK verification keys** — Searched in order:
   1. `LICHEN_ZK_*_VK_PATH` env vars (explicit absolute paths)
   2. `$HOME/.lichen/zk/vk_*.bin` (HOME-based, set by start script)
   3. `{exe_dir}/zk/` or `{exe_dir}/zk-keys/` (bundled with binary)
   4. `./zk-keys/` (CWD fallback)

5. **Log directory** — Always `{data_dir}/logs/` (canonicalized at startup)

### Environment Variables

| Variable | Set By | Purpose |
|---|---|---|
| `HOME` | `lichen-start.sh` | Overridden to `{data_dir}/home` for P2P identity isolation |
| `LICHEN_REAL_HOME` | `lichen-start.sh` | Preserves actual user home for shared keypair lookup |
| `LICHEN_HOME` | systemd / scripts | Explicit P2P identity home override |
| `LICHEN_ZK_*_VK_PATH` | `lichen-start.sh` | Absolute paths to ZK verification keys |

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
[ ] 15. CNAME: @            → lichen-website.pages.dev       (Proxied)
[ ] 16. CNAME: www          → lichen.network                 (Proxied)
[ ] 17. CNAME: explorer     → lichen-explorer.pages.dev      (Proxied)
[ ] 18. CNAME: wallet       → lichen-wallet.pages.dev        (Proxied)
[ ] 19. CNAME: dex          → lichen-dex.pages.dev           (Proxied)
[ ] 20. CNAME: marketplace  → lichen-marketplace.pages.dev   (Proxied)
[ ] 21. CNAME: programs     → lichen-programs.pages.dev      (Proxied)
[ ] 22. CNAME: developers   → lichen-developers.pages.dev    (Proxied)
[ ] 23. CNAME: monitoring   → lichen-monitoring.pages.dev    (Proxied)
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
[ ] 32. Copy faucet UI files to /opt/lichen/www/faucet-ui/
[ ] 33. Restart Caddy → verify HTTPS certs for rpc/ws/custody/faucet subdomains
```

### Cloudflare Pages

```
[ ] 34. Install Wrangler CLI: npm install -g wrangler && wrangler login
[ ] 35. Create CF Pages project: lichen-website     (output dir: website)
[ ] 36. Create CF Pages project: lichen-explorer     (output dir: explorer)
[ ] 37. Create CF Pages project: lichen-wallet       (output dir: wallet)
[ ] 38. Create CF Pages project: lichen-network-dex          (output dir: dex)
[ ] 39. Create CF Pages project: lichen-network-marketplace  (output dir: marketplace)
[ ] 40. Create CF Pages project: lichen-network-programs     (output dir: programs)
[ ] 41. Create CF Pages project: lichen-network-developers   (output dir: developers)
[ ] 42. Create CF Pages project: lichen-network-monitoring   (output dir: monitoring)
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
[ ] 51. https://lichen.network            — main website loads (CF Pages)
[ ] 52. https://rpc.lichen.network         — RPC responds to getHealth (VPS)
[ ] 53. https://ws.lichen.network          — WebSocket connects (VPS)
[ ] 54. https://explorer.lichen.network    — block explorer loads (CF Pages)
[ ] 55. https://wallet.lichen.network      — wallet loads, network switcher works (CF Pages)
[ ] 56. https://dex.lichen.network         — DEX loads (CF Pages)
[ ] 57. https://marketplace.lichen.network — marketplace loads (CF Pages)
[ ] 58. https://programs.lichen.network    — Programs IDE loads (CF Pages)
[ ] 59. https://developers.lichen.network  — dev portal loads (CF Pages)
[ ] 60. https://monitoring.lichen.network  — dashboard shows 3 validators (CF Pages)
[ ] 61. https://faucet.lichen.network      — faucet UI loads, airdrop works (VPS)
[ ] 62. https://custody.lichen.network     — custody /health returns OK (VPS)
```

### Post-Launch

```
[ ] 63. Update seeds.json with real IPs/domains
[ ] 64. Test agent connection from local machine:
        ./lichen-validator --bootstrap-peers seed-us.lichen.network:8001
[ ] 65. Set up backup cron jobs
[ ] 66. Set up health check monitoring
[ ] 67. Copy genesis-keys/ to a secure offline location (USB / vault)
[ ] 68. Set up WireGuard VPN between 3 VPS for signer port 9201
[ ] 69. Connect git repo to CF Pages for auto-deploy on push
```

---

## Clean Slate Deployment Runbook

> **Last validated: March 19, 2026** — This is the exact, battle-tested procedure for wiping all blockchain state and redeploying Lichen from scratch across all 3 VPS nodes. It reflects the runtime layout actually used on the live hosts during the successful v0.4.5 clean redeploy.

### ⚠️ Critical Knowledge — Read Before Starting

These are hard-won lessons from repeated deployment failures. Understand them before proceeding.

#### 1. Genesis must be created explicitly with `lichen-genesis`

On a freshly wiped node, starting `lichen-validator` directly is not enough to bootstrap a new chain. If seed peers are visible, the validator enters join mode and waits for genesis sync forever. For a clean network boot, create slot 0 first:

```bash
./target/release/lichen-genesis --network testnet --db-path ./data/state-testnet
./target/release/lichen-genesis --network mainnet --db-path ./data/state-mainnet
```

Only after those commands succeed should you launch `lichen-validator` against the matching `--db-path`.

#### 2. Seed peers cause the "join on empty state" trap

The validator checks for existing block state first. If there is no local slot 0 and any seeds are reachable, it assumes this node should join an existing network. Those seeds can come from:
- `--bootstrap-peers`
- `/etc/lichen/seeds.json`
- embedded/default seed configuration

That means an empty node can sit forever at `tip: 0` unless `lichen-genesis` has already created the database locally.

#### 3. The canonical live runtime path is repo-local

The clean v0.4.5 redeploy that restored stable consensus did **not** use the dormant systemd layout under `/var/lib/lichen`. The live validator layout was:

| Item | Path |
|---|---|
| Repo root | `~/lichen/` |
| Testnet DB | `~/lichen/data/state-testnet/` |
| Mainnet DB | `~/lichen/data/state-mainnet/` |
| Testnet log | `/tmp/lichen-testnet/validator.log` |
| Mainnet log | `/tmp/lichen-mainnet/validator.log` |
| Binaries | `~/lichen/target/release/lichen-*` |

If you wipe or inspect `/var/lib/lichen` without touching `~/lichen/data/state-*`, you are looking at the wrong runtime tree for this deployment path.

#### 4. US VPS requires binance.us oracle URLs (geo-blocking)

The US VPS (15.204.229.189) is geo-blocked from `binance.com` (HTTP 451 "Unavailable For Legal Reasons"). The oracle price feeder must use `binance.us` URLs instead:

```bash
export LICHEN_ORACLE_WS_URL="wss://stream.binance.us:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade"
export LICHEN_ORACLE_REST_URL="https://api.binance.us/api/v3/ticker/price?symbols=%5B%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22%5D"
```

The EU VPS (37.59.97.61) can use the default `binance.com` URLs. These env vars must be set **before** starting the validator process. The start script does NOT set them — you must export them in the shell before running it, or set them in the validator's environment.

#### 5. One chain per network: US creates genesis, EU and SEA join

There is one fresh testnet and one fresh mainnet. The validated order is:

```bash
# US VPS
./target/release/lichen-genesis --network testnet --db-path ./data/state-testnet
./target/release/lichen-genesis --network mainnet --db-path ./data/state-mainnet

# EU / SEA VPS
./target/release/lichen-validator --network testnet ... --bootstrap-peers 15.204.229.189:7001
./target/release/lichen-validator --network mainnet ... --bootstrap-peers 15.204.229.189:8001
```

Do not generate independent genesis on EU or SEA. They must join the US-created chain for both networks.

#### 6. Cross-compilation from macOS to Linux does NOT work

The `cross` tool (Docker-based cross-compilation) fails because `aws-lc-sys v0.37.0` detects a known GCC `memcmp` bug in the cross Docker image (GCC bugzilla #95189). Neither `AWS_LC_SYS_CMAKE_BUILDER=1` nor `AWS_LC_SYS_NO_ASM=1` resolves this for release builds.

**Solution:** Build natively on VPS or use pre-built binaries from CI. To build on VPS:
1. Install Rust: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
2. Install cmake: `sudo apt-get install -y cmake`
3. Build: `cargo build --release --bin lichen-validator --bin lichen-genesis --bin lichen-faucet --bin lichen-custody`
4. Strip: `strip target/release/lichen-{validator,genesis,faucet,custody}`

To relay pre-built binaries from one VPS to others:
```bash
# Download from source VPS to local
scp -P 2222 -O ubuntu@<SOURCE_VPS>:/usr/local/bin/lichen-validator /tmp/lichen-bins/

# Upload to target VPS
scp -P 2222 -O /tmp/lichen-bins/lichen-validator ubuntu@<TARGET_VPS>:/tmp/
ssh -p 2222 ubuntu@<TARGET_VPS> "mkdir -p ~/lichen/target/release && cp /tmp/lichen-validator ~/lichen/target/release/ && chmod +x ~/lichen/target/release/lichen-validator"
```

#### 7. CLI binary name is `lichen`, NOT `lichen-cli`

The CLI crate is named `lichen-cli` in `Cargo.toml`, but the binary target is `lichen` (via `[[bin]] name = "licn"` in `cli/Cargo.toml`). When building or referencing the CLI binary:

```bash
✅  cargo build --release --bin licn
❌  cargo build --release --bin lichen-cli   # ERROR: no bin target named 'lichen-cli'
```

#### 8. Inter-VPS SSH access

Each VPS has an `ed25519` SSH keypair and the other VPSes' public keys in `authorized_keys`. This enables direct VPS-to-VPS `scp` and `ssh`:

```bash
# From US VPS, copy a file to EU VPS:
scp -P 2222 /path/to/file ubuntu@37.59.97.61:/path/to/dest

# From EU VPS, check US VPS status:
ssh -p 2222 ubuntu@15.204.229.189 "pgrep -af lichen"
```

If VPS-to-VPS `scp` fails ("Permission denied"), verify that the source VPS's public key (`~/.ssh/id_ed25519.pub`) is in the target VPS's `~/.ssh/authorized_keys`.

#### 9. The "SINGLE GENESIS INVARIANT" in the validator binary

The validator binary has hardcoded bootstrap peers (embedded at compile time from `seeds.json`). If **any** seed peers are reachable — even without a local `seeds.json` file — the validator enters "joining" mode and will never create genesis on its own.

This means:
- Removing `seeds.json` does NOT prevent sync attempts (embedded peers still exist)
- You MUST use `lichen-genesis` (or `lichen-start.sh`) to create the genesis database before starting a new chain
- Never call `sudo systemctl start lichen-validator-testnet` on a fresh VPS — it has no genesis

#### 10. pkill over SSH kills the SSH session

Running `pkill -f lichen` over SSH kills the SSH session itself (because "lichen" appears in the process tree of the SSH session when you're cd'd into `~/lichen`).

**Safe stop pattern:**
```bash
ssh -p 2222 ubuntu@<VPS_IP> 'cat > /tmp/stop.sh << "EOF"
#!/bin/bash
for proc in lichen-validator lichen-faucet lichen-custody validator-supervisor; do
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
| Source repo | `lobstercove/lichen` on GitHub (rsync to VPS — no git on VPS) |
| Genesis binary | `~/lichen/target/release/lichen-genesis` |
| Validator binary | `~/lichen/target/release/lichen-validator` |
| Data dir | `~/lichen/data/state-{testnet,mainnet}/` |
| P2P ports | Testnet: **7001**, Mainnet: **8001** |
| RPC ports | Testnet: **8899**, Mainnet: **9899** |
| WS ports | Testnet: **8900**, Mainnet: **9900** |

### Step 0: Stop Existing Processes

Kill validators and ancillary services on each VPS without using broad `pkill -f` patterns that can self-match the SSH command.

```bash
ssh -p 2222 ubuntu@<VPS_IP> 'set +H
for proc in lichen-validator lichen-faucet lichen-custody validator-supervisor; do
  pids=$(pgrep -f "$proc" 2>/dev/null || true)
  if [ -n "$pids" ]; then
    echo "$pids" | xargs kill 2>/dev/null || true
  fi
done
sleep 2
pgrep -af lichen || true'
```

### Step 1: Wipe Repo-Local State

Remove repo-local chain state, custody state, and transient logs on all three VPSes.

> **⚠️ IMPORTANT:** When wiping state for a re-genesis, you must ALSO delete `artifacts/{network}/` (wallet + keypair files). If you only wipe the chain state (`data/state-*`) but leave stale artifacts, the next genesis will create on-chain accounts from the old wallet pubkeys but the keypair files will be stale. **Always wipe both together.**

```bash
ssh -p 2222 ubuntu@<VPS_IP> '
  rm -rf ~/lichen/data/state-testnet ~/lichen/data/state-mainnet
  rm -rf ~/lichen/artifacts/testnet ~/lichen/artifacts/mainnet
  rm -rf ~/lichen/data/custody-testnet ~/lichen/data/custody-mainnet
  rm -rf /tmp/lichen-testnet /tmp/lichen-mainnet
  rm -f ~/.lichen/peer_fingerprints.json
  mkdir -p ~/lichen/data
  echo state wiped
'
```

> **TOFU fingerprint cache:** Each validator stores peer identity fingerprints in `~/.lichen/peer_fingerprints.json`. After a wipe, validators get new identities. If old fingerprints remain, the P2P layer rejects the new identity ("TOFU verification failed"). Always clear this file when wiping state.

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
    ./ "ubuntu@${VPS_IP}:~/lichen/"
done
```

### Step 2.5: Rebuild Contract WASMs Locally (Before Rsync)

> **⚠️ CRITICAL — WASM Staleness:** Each contract in `contracts/` has a pre-built `.wasm` file at `contracts/{name}/{name}.wasm`. These are the WASMs that `lichen-genesis` deploys at chain creation. **They are NOT automatically rebuilt by `cargo build --release`**. If any contract source (`contracts/*/src/lib.rs`) has been modified since the last WASM build, the pre-built WASM is stale and the old behavior will be deployed.

**Always rebuild WASMs before a re-genesis:**

```bash
# From local repo root (requires wasm32-unknown-unknown target)
rustup target add wasm32-unknown-unknown
./scripts/build-all-contracts.sh
```

This rebuilds all 29 contracts from source and copies the output to `contracts/{name}/{name}.wasm`. The updated WASMs are then rsynced to the VPSes in Step 2.

### Step 3: Build on Each VPS

Build the validator and genesis binaries first. Build faucet and custody if you plan to restart ancillary services in the same maintenance window.

```bash
ssh -p 2222 ubuntu@<VPS_IP> "
  cd ~/lichen && source ~/.cargo/env
  cargo build --release --bin lichen-validator --bin lichen-genesis
"
```

Verify:
```bash
ssh -p 2222 ubuntu@<VPS_IP> "ls -la ~/lichen/target/release/lichen-{validator,genesis}"
```

### Step 4: Initialize Genesis Databases on US VPS

Genesis uses a two-phase process to ensure keypair consistency:

1. **`--prepare-wallet`** generates `genesis-wallet.json` + all keypair files in `artifacts/{network}/`
2. **`--wallet-file`** reads the wallet, creates chain state in `--db-path`, and **copies + validates all keypairs** into `{db-path}/genesis-keys/`

> **⚠️ CRITICAL — Keypair Consistency:** The wallet JSON and keypair files are tightly coupled. Running `--prepare-wallet` overwrites the keypair files with NEW keys. If you run `--prepare-wallet` twice without also re-running genesis, the keypair files will have different pubkeys than the wallet JSON. The genesis tool validates this at creation time and **aborts with `KEYPAIR MISMATCH`** if detected. To fix: delete the old wallet + keys from `artifacts/` and re-run both steps.

```bash
ssh -p 2222 ubuntu@15.204.229.189 "
  cd ~/lichen

  # Phase 1: Generate matching wallet + keypairs (ONLY if no wallet exists yet)
  ./target/release/lichen-genesis --prepare-wallet --network testnet --output-dir ./artifacts/testnet

  # Phase 2: Create chain from wallet (copies + validates all keypairs into db-path)
  ./target/release/lichen-genesis --network testnet \
    --wallet-file ./artifacts/testnet/genesis-wallet.json \
    --initial-validator \$(grep -m1 publicKeyBase58 ./data/state-testnet/validator-keypair.json 2>/dev/null | sed -E 's/.*\"([A-Za-z1-9]+)\".*/\1/' || echo '<VALIDATOR_PUBKEY>') \
    --db-path ./data/state-testnet
"
```

This creates the fresh slot-0 database, writes the genesis wallet and all distribution keypairs under `~/lichen/data/state-testnet/genesis-keys/`, and avoids the empty-state join trap.

> **Faucet treasury identity:** The distribution wallet `validator_rewards` doubles as the faucet treasury (`treasury-{chain_id}.json`). Both files contain the same keypair. The on-chain treasury account receives 50M LICN at genesis and is used by `requestAirdrop` RPC to sign airdrop transfers.

### Step 5: Start US Validators Against the Genesis DBs

Launch the validators directly, writing logs to `/tmp/lichen-{network}/validator.log`.

```bash
ssh -p 2222 ubuntu@15.204.229.189 "
  cd ~/lichen
  mkdir -p /tmp/lichen-testnet /tmp/lichen-mainnet
  setsid sh -c './target/release/lichen-validator --network testnet --rpc-port 8899 --ws-port 8900 --p2p-port 7001 --db-path ./data/state-testnet --listen-addr 0.0.0.0 > /tmp/lichen-testnet/validator.log 2>&1' >/dev/null 2>&1 &
  setsid sh -c './target/release/lichen-validator --network mainnet --rpc-port 9899 --ws-port 9900 --p2p-port 8001 --db-path ./data/state-mainnet --listen-addr 0.0.0.0 > /tmp/lichen-mainnet/validator.log 2>&1' >/dev/null 2>&1 &
"
```

Verify US is advancing:

```bash
ssh -p 2222 ubuntu@15.204.229.189 "
  curl -s -X POST http://127.0.0.1:8899 -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}'
  echo
  curl -s -X POST http://127.0.0.1:9899 -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}'
  echo
  tail -n 20 /tmp/lichen-testnet/validator.log
  tail -n 20 /tmp/lichen-mainnet/validator.log
"
```

### Step 6: Join EU and SEA from US

Start EU and SEA validators against empty local DBs, but with explicit US bootstrap peers.

```bash
ssh -p 2222 ubuntu@37.59.97.61 "
  cd ~/lichen
  mkdir -p /tmp/lichen-testnet /tmp/lichen-mainnet
  setsid sh -c './target/release/lichen-validator --network testnet --rpc-port 8899 --ws-port 8900 --p2p-port 7001 --db-path ./data/state-testnet --listen-addr 0.0.0.0 --bootstrap-peers 15.204.229.189:7001 > /tmp/lichen-testnet/validator.log 2>&1' >/dev/null 2>&1 &
  setsid sh -c './target/release/lichen-validator --network mainnet --rpc-port 9899 --ws-port 9900 --p2p-port 8001 --db-path ./data/state-mainnet --listen-addr 0.0.0.0 --bootstrap-peers 15.204.229.189:8001 > /tmp/lichen-mainnet/validator.log 2>&1' >/dev/null 2>&1 &
"

ssh -p 2222 ubuntu@15.235.142.253 "
  cd ~/lichen
  mkdir -p /tmp/lichen-testnet /tmp/lichen-mainnet
  setsid sh -c './target/release/lichen-validator --network testnet --rpc-port 8899 --ws-port 8900 --p2p-port 7001 --db-path ./data/state-testnet --listen-addr 0.0.0.0 --bootstrap-peers 15.204.229.189:7001 > /tmp/lichen-testnet/validator.log 2>&1' >/dev/null 2>&1 &
  setsid sh -c './target/release/lichen-validator --network mainnet --rpc-port 9899 --ws-port 9900 --p2p-port 8001 --db-path ./data/state-mainnet --listen-addr 0.0.0.0 --bootstrap-peers 15.204.229.189:8001 > /tmp/lichen-mainnet/validator.log 2>&1' >/dev/null 2>&1 &
"
```

EU and SEA will start at slot 0, fetch genesis/state from US, submit `RegisterValidator`, then move into the active proposer set once their on-chain stake is committed.

### Step 7: Verify 3-Node Convergence

```bash
for VPS_IP in 15.204.229.189 37.59.97.61 15.235.142.253; do
  echo "=== $VPS_IP ==="
  ssh -p 2222 "ubuntu@${VPS_IP}" "
    echo testnet && curl -s -X POST http://127.0.0.1:8899 -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSlot\",\"params\":[]}'
    echo
    echo mainnet && curl -s -X POST http://127.0.0.1:9899 -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getSlot\",\"params\":[]}'
    echo
    tail -n 20 /tmp/lichen-testnet/validator.log
    tail -n 20 /tmp/lichen-mainnet/validator.log
  "
done
```

Healthy steady-state signals:
- all three nodes report advancing slots on both networks
- logs show `eligible=3`
- proposers rotate across US, EU, and SEA
- BFT messages are broadcast to `2 peers`

### Step 8: Start Faucet and Custody After Validator Health Is Stable

Start the testnet faucet on all 3 VPSes. The faucet keypair is generated during genesis on the US VPS.

> **Key copying for joining VPS:** The EU and SEA VPSes (and any future VPS that joined via `--bootstrap`) do NOT have genesis keys locally — they exist only on the genesis VPS (US). You must copy them before starting faucet or custody on joining VPS.

```bash
# ── Copy faucet + treasury keys from US to EU + SEA ──
# (pipe through local machine — VPS-to-VPS scp may not work without inter-VPS SSH setup)
for JOINING_IP in 37.59.97.61 15.235.142.253; do
  echo "=== Copying keys to $JOINING_IP ==="
  for key in faucet-lichen-testnet-1.json treasury-lichen-testnet-1.json; do
    ssh -p 2222 ubuntu@15.204.229.189 "cat ~/lichen/data/state-testnet/genesis-keys/$key" \
      | ssh -p 2222 ubuntu@$JOINING_IP "mkdir -p ~/lichen/data/state-testnet/genesis-keys && cat > ~/lichen/data/state-testnet/genesis-keys/$key"
  done
  for key in faucet-lichen-mainnet-1.json treasury-lichen-mainnet-1.json; do
    ssh -p 2222 ubuntu@15.204.229.189 "cat ~/lichen/data/state-mainnet/genesis-keys/$key" \
      | ssh -p 2222 ubuntu@$JOINING_IP "mkdir -p ~/lichen/data/state-mainnet/genesis-keys && cat > ~/lichen/data/state-mainnet/genesis-keys/$key"
  done
done
```

Start faucet on all 3 VPSes:
```bash
# ── US VPS ──
ssh -p 2222 ubuntu@15.204.229.189 "
  cd ~/lichen
  PORT=9100 \
  RPC_URL=http://127.0.0.1:8899 \
  NETWORK=testnet \
  MAX_PER_REQUEST=10 \
  DAILY_LIMIT_PER_IP=150 \
  COOLDOWN_SECONDS=60 \
  AIRDROPS_FILE=/tmp/lichen-testnet/airdrops.json \
  FAUCET_KEYPAIR=\$HOME/lichen/data/state-testnet/genesis-keys/faucet-lichen-testnet-1.json \
  RUST_LOG=info \
  TRUSTED_PROXY=127.0.0.1,::1 \
  nohup ./target/release/lichen-faucet > /tmp/lichen-testnet/faucet.log 2>&1 &
  sleep 2 && pgrep -f lichen-faucet && echo 'Faucet running'
"

# ── EU VPS ──
ssh -p 2222 ubuntu@37.59.97.61 '
  cd ~/lichen
  PORT=9100 \
  RPC_URL=http://127.0.0.1:8899 \
  NETWORK=testnet \
  MAX_PER_REQUEST=10 \
  DAILY_LIMIT_PER_IP=150 \
  COOLDOWN_SECONDS=60 \
  AIRDROPS_FILE=/tmp/lichen-testnet/airdrops.json \
  FAUCET_KEYPAIR=/home/ubuntu/lichen/data/state-testnet/genesis-keys/faucet-lichen-testnet-1.json \
  RUST_LOG=info \
  TRUSTED_PROXY=127.0.0.1,::1 \
  nohup ./target/release/lichen-faucet > /tmp/lichen-testnet/faucet.log 2>&1 &
  sleep 2 && pgrep -f lichen-faucet && echo "Faucet running"
'

# ── SEA VPS ──
ssh -p 2222 ubuntu@15.235.142.253 '
  cd ~/lichen
  PORT=9100 \
  RPC_URL=http://127.0.0.1:8899 \
  NETWORK=testnet \
  MAX_PER_REQUEST=10 \
  DAILY_LIMIT_PER_IP=150 \
  COOLDOWN_SECONDS=60 \
  AIRDROPS_FILE=/tmp/lichen-testnet/airdrops.json \
  FAUCET_KEYPAIR=/home/ubuntu/lichen/data/state-testnet/genesis-keys/faucet-lichen-testnet-1.json \
  RUST_LOG=info \
  TRUSTED_PROXY=127.0.0.1,::1 \
  nohup ./target/release/lichen-faucet > /tmp/lichen-testnet/faucet.log 2>&1 &
  sleep 2 && pgrep -f lichen-faucet && echo "Faucet running"
'
```

### Step 8b: Start Custody

Start custody bridge on US VPS for testnet and mainnet. Currently custody runs on US only.

> **Port env var:** Custody uses `CUSTODY_LISTEN_PORT` (NOT `PORT`). Default is 9105. Use 9106 for mainnet.
> **Scaling note:** For 10-20K users, consider running custody on all 3 VPSes behind a load balancer (see custody scaling brainstorm in docs/architecture/).

```bash
# ── US VPS — Testnet Custody (port 9105) ──
ssh -p 2222 ubuntu@15.204.229.189 "
  cd ~/lichen && mkdir -p data/custody-testnet
  CUSTODY_DB_PATH=./data/custody-testnet \
  CUSTODY_LICHEN_RPC_URL=http://127.0.0.1:8899 \
  CUSTODY_TREASURY_KEYPAIR=\$HOME/lichen/data/state-testnet/genesis-keys/treasury-lichen-testnet-1.json \
  CUSTODY_ALLOW_INSECURE_SEED=1 \
  CUSTODY_API_AUTH_TOKEN=testnet-custody-token-2026 \
  CUSTODY_SIGNER_AUTH_TOKEN=testnet-signer-token-2026 \
  CUSTODY_SIGNER_ENDPOINTS=http://127.0.0.1:9201,http://127.0.0.1:9202,http://127.0.0.1:9203 \
  CUSTODY_SIGNER_THRESHOLD=2 \
  RUST_LOG=info \
  nohup ./target/release/lichen-custody > /tmp/lichen-testnet/custody.log 2>&1 &
  sleep 2 && tail -1 /tmp/lichen-testnet/custody.log
"

# ── US VPS — Mainnet Custody (port 9106) ──
ssh -p 2222 ubuntu@15.204.229.189 "
  cd ~/lichen && mkdir -p data/custody-mainnet
  CUSTODY_DB_PATH=./data/custody-mainnet \
  CUSTODY_LICHEN_RPC_URL=http://127.0.0.1:9899 \
  CUSTODY_TREASURY_KEYPAIR=\$HOME/lichen/data/state-mainnet/genesis-keys/treasury-lichen-mainnet-1.json \
  CUSTODY_ALLOW_INSECURE_SEED=1 \
  CUSTODY_API_AUTH_TOKEN=mainnet-custody-token-2026 \
  CUSTODY_SIGNER_AUTH_TOKEN=mainnet-signer-token-2026 \
  CUSTODY_SIGNER_ENDPOINTS=http://127.0.0.1:9201,http://127.0.0.1:9202,http://127.0.0.1:9203 \
  CUSTODY_SIGNER_THRESHOLD=2 \
  CUSTODY_LISTEN_PORT=9106 \
  RUST_LOG=info \
  nohup ./target/release/lichen-custody > /tmp/lichen-mainnet/custody.log 2>&1 &
  sleep 2 && tail -1 /tmp/lichen-mainnet/custody.log
"

# ── EU VPS — Testnet Custody (port 9105) — OPTIONAL, currently not running ──
# Uncomment these blocks when scaling custody to multiple VPSes
# ssh -p 2222 ubuntu@37.59.97.61 '
#   cd ~/lichen && mkdir -p data/custody-testnet
#   CUSTODY_DB_PATH=./data/custody-testnet \
#   CUSTODY_LICHEN_RPC_URL=http://127.0.0.1:8899 \
#   CUSTODY_TREASURY_KEYPAIR=/home/ubuntu/lichen/data/state-testnet/genesis-keys/treasury-lichen-testnet-1.json \
#   CUSTODY_ALLOW_INSECURE_SEED=1 \
#   CUSTODY_API_AUTH_TOKEN=testnet-custody-token-2026 \
#   CUSTODY_SIGNER_AUTH_TOKEN=testnet-signer-token-2026 \
#   CUSTODY_SIGNER_ENDPOINTS=http://127.0.0.1:9201,http://127.0.0.1:9202,http://127.0.0.1:9203 \
#   CUSTODY_SIGNER_THRESHOLD=2 \
#   RUST_LOG=info \
#   nohup ./target/release/lichen-custody > /tmp/lichen-testnet/custody.log 2>&1 &
#   sleep 2 && tail -1 /tmp/lichen-testnet/custody.log
# '

# ── EU VPS — Mainnet Custody (port 9106) — OPTIONAL, currently not running ──
# ssh -p 2222 ubuntu@37.59.97.61 '
#   cd ~/lichen && mkdir -p data/custody-mainnet
#   CUSTODY_DB_PATH=./data/custody-mainnet \
#   CUSTODY_LICHEN_RPC_URL=http://127.0.0.1:9899 \
#   CUSTODY_TREASURY_KEYPAIR=/home/ubuntu/lichen/data/state-mainnet/genesis-keys/treasury-lichen-mainnet-1.json \
#   CUSTODY_ALLOW_INSECURE_SEED=1 \
#   CUSTODY_API_AUTH_TOKEN=mainnet-custody-token-2026 \
#   CUSTODY_SIGNER_AUTH_TOKEN=mainnet-signer-token-2026 \
#   CUSTODY_SIGNER_ENDPOINTS=http://127.0.0.1:9201,http://127.0.0.1:9202,http://127.0.0.1:9203 \
#   CUSTODY_SIGNER_THRESHOLD=2 \
#   CUSTODY_LISTEN_PORT=9106 \
#   RUST_LOG=info \
#   nohup ./target/release/lichen-custody > /tmp/lichen-mainnet/custody.log 2>&1 &
#   sleep 2 && tail -1 /tmp/lichen-mainnet/custody.log
# '
```

### Step 9: Verify Ancillary Services

```bash
for VPS_IP in 15.204.229.189 37.59.97.61 15.235.142.253; do
  echo ""
  echo "=== $VPS_IP ==="
  ssh -p 2222 "ubuntu@${VPS_IP}" "
    echo 'Processes:'
    pgrep -af lichen | grep -v pgrep

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
./target/release/lichen-validator \
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

Logs are written to `/tmp/lichen-{network}/` (created by `lichen-start.sh`), **not** `~/lichen/logs/`.

| Log | Path |
|---|---|
| Testnet validator | `/tmp/lichen-testnet/validator.log` |
| Mainnet validator | `/tmp/lichen-mainnet/validator.log` |
| Testnet faucet | `/tmp/lichen-testnet/faucet.log` |
| Testnet custody | `/tmp/lichen-testnet/custody.log` |
| Mainnet custody | `/tmp/lichen-mainnet/custody.log` |
| First-boot deploy | `/tmp/lichen-testnet/first-boot-deploy.log` |

### Restarting a Validator (Without Re-creating Genesis)

If a validator crashes or you need to restart it, the start script detects existing state and enters **RESUME** mode (no genesis):

```bash
# The start script detects data/state-testnet/CURRENT exists → RESUME mode
ssh -p 2222 ubuntu@<VPS_IP> "
  cd ~/lichen

  # For US VPS, always set binance.us env vars
  export LICHEN_ORACLE_WS_URL='wss://stream.binance.us:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade'
  export LICHEN_ORACLE_REST_URL='https://api.binance.us/api/v3/ticker/price?symbols=%5B%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22%5D'

  bash lichen-start.sh testnet
"
```

Or restart directly via the supervisor (skipping the start script):

```bash
ssh -p 2222 ubuntu@<VPS_IP> "
  cd ~/lichen

  export LICHEN_ORACLE_WS_URL='wss://stream.binance.us:9443/ws/solusdt@aggTrade/ethusdt@aggTrade/bnbusdt@aggTrade'
  export LICHEN_ORACLE_REST_URL='https://api.binance.us/api/v3/ticker/price?symbols=%5B%22SOLUSDT%22,%22ETHUSDT%22,%22BNBUSDT%22%5D'

  ./scripts/validator-supervisor.sh testnet-primary-p7001 -- \
    ./target/release/lichen-validator \
    --network testnet --rpc-port 8899 --ws-port 8900 --p2p-port 7001 \
    --db-path ./data/state-7001 \
    --bootstrap-peers '15.204.229.189:7001,37.59.97.61:7001' \
    --listen-addr 0.0.0.0 \
    >./logs/validator.log 2>&1 &
"
```

### Files Created During Genesis

After a successful genesis boot via `lichen-start.sh`, these files exist:

```
~/lichen/
├── data/
│   ├── state-testnet/                    # Testnet blockchain state (RocksDB)
│   │   ├── genesis-keys/                 # Treasury keypairs (CRITICAL — back these up)
│   │   │   ├── genesis-primary-lichen-testnet-1.json
│   │   │   ├── faucet-lichen-testnet-1.json
│   │   │   ├── treasury-lichen-testnet-1.json
│   │   │   ├── community_treasury-lichen-testnet-1.json
│   │   │   ├── builder_grants-lichen-testnet-1.json
│   │   │   └── ...
│   │   ├── home/                         # Validator identity (validator-id.json)
│   │   ├── CURRENT                       # RocksDB marker (presence = state exists)
│   │   └── <SST files>                   # RocksDB data
│   └── state-mainnet/                    # Mainnet blockchain state (same structure)
│       ├── genesis-keys/
│       └── ...
└── target/release/
    ├── lichen-validator               # Main binary
    ├── lichen-genesis                 # Genesis creation tool
    ├── lichen-faucet                  # Faucet binary
    └── lichen-custody                 # Custody bridge binary

# Logs (written to /tmp/ by lichen-start.sh, NOT ~/lichen/logs/)
/tmp/
├── lichen-testnet/
│   ├── validator.log                     # Testnet validator log
│   ├── faucet.log                        # Faucet log
│   ├── custody.log                       # Testnet custody log
│   └── first-boot-deploy.log            # Contract deployment log
└── lichen-mainnet/
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
git tag -a v0.1.0 -m "Lichen v0.1.0 - Initial release"
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
curl -LO https://github.com/lobstercove/lichen/releases/download/v0.1.0/SHA256SUMS

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
git tag -a v0.2.0 -m "Lichen v0.2.0 - <description>"
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
tar czf lichen-validator-darwin-aarch64.tar.gz lichen-validator
cd ../..

# Compute SHA256
sha256sum target/release/lichen-validator-darwin-aarch64.tar.gz > SHA256SUMS

# Sign
./scripts/sign-release.sh SHA256SUMS /path/to/release-signing-keypair.json

# Create GitHub release manually and upload:
#   - lichen-validator-darwin-aarch64.tar.gz
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
./lichen-validator \
  --auto-update check \
  --p2p-port 7001

# Download + verify, but don't apply (manual restart needed)
./lichen-validator \
  --auto-update download \
  --p2p-port 7001

# Full automatic updates (recommended for testnet)
./lichen-validator \
  --auto-update apply \
  --p2p-port 7001

# Customize check interval and channel
./lichen-validator \
  --auto-update apply \
  --update-check-interval 600 \
  --update-channel stable \
  --p2p-port 7001
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
INFO  📦 Downloading lichen-validator-linux-x86_64.tar.gz (24MB)...
INFO  ✅ SHA256SUMS signature verified
INFO  ✅ SHA256 verified for lichen-validator-linux-x86_64.tar.gz
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
cp lichen-validator.rollback lichen-validator
# Restart the validator
```

The auto-rollback triggers automatically if the validator crashes 3 times within 60 seconds of an update.

---

## Validator CLI Reference

Complete reference for all `lichen-validator` CLI flags:

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
./lichen-validator \
  --p2p-port 7001 \
  --listen-addr 0.0.0.0 \
  --network testnet \
  --genesis genesis-testnet.json \
  --bootstrap-peers seed-us.lichen.network:8001,seed-eu.lichen.network:8001 \
  --auto-update apply \
  --admin-token "$(cat /etc/lichen/admin-token)"
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
| Deploy genesis/programs | `./target/release/lichen-validator --network testnet --p2p-port 7001` (first run) | same + state distribution to seed nodes | mainnet genesis generation + signed distribution artifacts |
| Configure services | local env defaults | systemd + `/etc/lichen/env-testnet` + Caddy | systemd + secrets manager + hardened Caddy/firewall |
| Strict gate | `STRICT_NO_SKIPS=1 bash tests/production-e2e-gate.sh` | `STRICT_NO_SKIPS=1 bash tests/production-e2e-gate.sh` | pre-launch requirement: same strict gate against prod-like env |
| Launch / operate | `./lichen-validator ...` | `systemctl start lichen-validator` (+ custody/faucet where needed) | staged rollout across seeds/relays with monitored restart windows |

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
