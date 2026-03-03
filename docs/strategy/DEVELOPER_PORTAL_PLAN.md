# MoltChain Developer Portal — Build Plan

**Date:** February 10, 2026  
**Location:** `developers/`  
**Design:** Same shared theme as Explorer + Marketplace (`shared-base-styles.css` + `shared-theme.css`)  
**Stack:** Static HTML + vanilla JS + Font Awesome + Inter/JetBrains Mono — same as other frontends  

---

## Overview

The Developer Portal is the one-stop reference for anyone building on MoltChain. It consolidates all API docs, SDK references, contract guides, CLI docs, and tutorials into a cohesive, searchable portal with the same dark-navy + orange design system used by Explorer and Marketplace.

---

## Site Map — 14 Pages

### Navigation Structure

```
[Home/Hub]  [Guides]  [API Reference]  [SDK]  [Contracts]  [CLI]  [MoltyID]
```

### Page List

| # | Page | File | Description |
|---|------|------|-------------|
| 1 | **Hub / Landing** | `index.html` | Overview cards linking to all sections. Live stats (RPC), quick-start code snippet, ecosystem diagram |
| 2 | **Getting Started** | `getting-started.html` | Install CLI, create wallet, get testnet tokens, first transfer, deploy first contract — 5-step wizard |
| 3 | **Architecture** | `architecture.html` | Network diagram, consensus (PoC), execution model, state management, block lifecycle |
| 4 | **JSON-RPC Reference** | `rpc-reference.html` | Every RPC method: name, params, return type, example request/response. Grouped by category |
| 5 | **WebSocket Reference** | `ws-reference.html` | Subscription methods, event types, connection lifecycle, code examples |
| 6 | **JS/TS SDK** | `sdk-js.html` | `@moltchain/sdk` — Connection, Keypair, PublicKey, Transaction, all methods with types |
| 7 | **Python SDK** | `sdk-python.html` | `moltchain-sdk` — Connection, Keypair, Transaction, all methods with docstrings |
| 8 | **Rust SDK** | `sdk-rust.html` | `moltchain-client` — Client, types, error handling, async patterns |
| 9 | **Smart Contract Dev** | `contracts.html` | How WASM contracts work, SDK functions, storage model, cross-contract calls, testing |
| 10 | **Contract Reference** | `contract-reference.html` | All 16 contracts: name, address, exported functions, parameters, return codes, storage keys |
| 11 | **MoltyID Guide** | `moltyid.html` | .molt naming, reputation, trust tiers, identity-gated access, agent discovery, integration guide |
| 12 | **CLI Reference** | `cli-reference.html` | Every CLI command with flags, examples, output format |
| 13 | **Validator Guide** | `validator.html` | Setup, config.toml reference, staking, monitoring, systemd deployment |
| 14 | **Changelog** | `changelog.html` | Version history, breaking changes, new features |

---

## Shared Assets

```
developers/
├── index.html                  # Hub landing
├── getting-started.html        # Quick-start wizard
├── architecture.html           # Technical architecture
├── rpc-reference.html          # JSON-RPC API docs
├── ws-reference.html           # WebSocket docs
├── sdk-js.html                 # JS/TS SDK reference
├── sdk-python.html             # Python SDK reference
├── sdk-rust.html               # Rust SDK reference
├── contracts.html              # Smart contract dev guide
├── contract-reference.html     # All 16 contracts reference
├── moltyid.html                # MoltyID integration guide
├── cli-reference.html          # CLI command reference
├── validator.html              # Validator setup guide
├── changelog.html              # Version history
├── css/
│   └── developers.css          # Portal-specific styles (sidebar, code blocks, TOC)
├── js/
│   ├── developers.js           # Sidebar nav, search, code copy, tab switching
│   ├── rpc-data.js             # RPC method definitions (name, params, returns, examples)
│   ├── contract-data.js        # Contract function definitions from all 16 contracts
│   └── cli-data.js             # CLI command definitions
├── assets/
│   ├── MoltDev_Logo_256.png    # Portal logo (to be created)
│   └── diagrams/               # Architecture SVG diagrams
└── docs/                       # (optional) raw markdown source for migration
```

---

## Page Specifications

### Page 1: Hub Landing (`index.html`)

**Layout:** Full-width hero → 3-column card grid → live stats bar → code snippet

- **Hero:** "Build on MoltChain" headline, subtitle, two CTAs ("Get Started" → getting-started, "API Docs" → rpc-reference)
- **Card Grid (6 cards):** Quick Start, API Reference, SDKs, Smart Contracts, MoltyID, Validator Guide — each with icon, title, description, link
- **Live Stats Bar:** Block height, TPS, validators, total accounts — fetched from RPC (same pattern as explorer)
- **Quick Code Snippet:** Tabbed code block (JS / Python / Rust / CLI) showing "Connect and get balance" in each language
- **Footer:** Links to GitHub, Discord, Explorer, Marketplace

### Page 2: Getting Started (`getting-started.html`)

**Layout:** Sidebar TOC + main content with numbered steps

- **Step 1 — Install CLI:** `curl` one-liner, build from source, verify with `molt --version`
- **Step 2 — Create Wallet:** `molt wallet new`, show output, explain mnemonic safety
- **Step 3 — Get Testnet Tokens:** Web faucet link + `molt faucet request` (if available)
- **Step 4 — First Transfer:** `molt transfer` command with full example
- **Step 5 — Deploy a Contract:** Write minimal contract, compile to WASM, deploy with CLI
- **Each step:** Collapsible code block, expected output, "Next" button

### Page 3: Architecture (`architecture.html`)

**Layout:** Sidebar TOC + content with diagrams

Sections:
- Overview diagram (validator → P2P → consensus → execution → state)
- Proof of Contribution consensus (leader schedule, slot timing, PBFT finality)
- Transaction lifecycle (submit → mempool → block → commit)
- State model (RocksDB column families, account structure)
- WASM execution (Wasmer runtime, host functions, gas metering)
- MoltyID trust tier integration in mempool/processor
- Network topology (gossip, sync, bootstrap)

### Page 4: JSON-RPC Reference (`rpc-reference.html`)

**Layout:** Sidebar method list + main content with method cards

Data source: Parse all methods from `rpc/src/lib.rs`. Group by category:

| Category | Methods |
|----------|---------|
| **Chain** | `getSlot`, `getBlockHeight`, `getLatestBlock`, `getGenesisHash`, `getHealth`, `getVersion`, `getMetrics` |
| **Account** | `getBalance`, `getAccountInfo`, `getMultipleAccounts`, `getRecentBlockhash` |
| **Block** | `getBlock`, `getBlocks`, `getBlockProduction` |
| **Transaction** | `getTransaction`, `sendTransaction`, `simulateTransaction`, `getTransactionCount`, `getRecentTransactions`, `getTransactionsByAddress` |
| **Validator** | `getValidators`, `getValidatorInfo`, `getStakeInfo` |
| **Contract** | `getContractInfo`, `getAllContracts`, `getContractStorage`, `callContract` |
| **Token** | `getTokenBalance`, `getTokenSupply`, `getTokenAccounts` |
| **DeFi** | `getMarketListings`, `getMarketSales`, `getAuctionInfo` |
| **Burn** | `getTotalBurned`, `getBurnInfo`, `getBurnHistory` |

Each method card:
```
┌─────────────────────────────────────────┐
│ ● getBalance                            │
│ Returns the balance of an account       │
│                                         │
│ Parameters:                             │
│   address (string) — Base58 pubkey      │
│                                         │
│ Returns:                                │
│   { balance: number, reputation: number }│
│                                         │
│ Example Request:                        │
│   ┌──────────────────────────────────┐  │
│   │ curl -X POST http://localhost... │  │
│   └──────────────────────────────────┘  │
│ Example Response:                       │
│   ┌──────────────────────────────────┐  │
│   │ { "result": { "balance": 1000 } }│  │
│   └──────────────────────────────────┘  │
└─────────────────────────────────────────┘
```

### Page 5: WebSocket Reference (`ws-reference.html`)

**Layout:** Same as RPC reference

Sections:
- Connection (endpoint, authentication)
- Subscribe methods: `blockSubscribe`, `transactionSubscribe`, `accountSubscribe`, `slotSubscribe`
- Event payloads with types
- Reconnection handling
- Code examples (JS, Python)

### Page 6-8: SDK References (`sdk-js.html`, `sdk-python.html`, `sdk-rust.html`)

**Layout:** Sidebar class list + main content with class/method documentation

Each SDK page covers:
- Installation (`npm install` / `pip install` / `cargo add`)
- Quick start (connect, create keypair, send transaction)
- **Class reference** with every public method:
  - `Connection` — constructor, `getBalance()`, `getBlock()`, `sendTransaction()`, `subscribe()`, etc.
  - `Keypair` — `generate()`, `fromSeed()`, `publicKey`, `sign()`
  - `PublicKey` — `constructor()`, `toBase58()`, `toBytes()`, `equals()`
  - `Transaction` / `TransactionBuilder` — `addInstruction()`, `sign()`, `serialize()`
- Error handling patterns
- TypeScript types (for JS SDK)

Data populated from actual SDK source:
- JS: `sdk/js/src/` (1,093 lines, 6 files)
- Python: `sdk/python/` (822 lines, 6 files)
- Rust: `sdk/rust/` (614 lines, 6 files)

### Page 9: Smart Contract Development (`contracts.html`)

**Layout:** Tutorial-style with code blocks

Sections:
1. **How Contracts Work** — WASM target, `#![no_std]`, `extern "C"` exports, `moltchain-sdk`
2. **SDK Functions** — `storage_get/set/remove`, `get_caller`, `get_value`, `get_timestamp`, `get_slot`, `emit_event`, `log`, `contract_return`
3. **Cross-Contract Calls** — `call_contract()`, `call_token_balance()`
4. **Token Module** — `token_transfer`, `token_mint`, `token_burn`, `token_create`
5. **NFT Module** — `nft_mint`, `nft_transfer`, `nft_metadata`
6. **DEX Module** — `create_pool`, `add_liquidity`, `swap`
7. **Storage Model** — key patterns, serialization, byte packing
8. **Testing** — `test_mock` module usage: `set_caller`, `set_args`, `set_value`, `get_return_data`, `get_events`
9. **Build & Deploy** — Cargo.toml setup, `cargo build --target wasm32`, CLI deploy command
10. **Security Best Practices** — reentrancy guards, overflow protection, access control patterns

### Page 10: Contract Reference (`contract-reference.html`)

**Layout:** Accordion/tabs per contract

All 16 contracts documented:
| Contract | Category | Functions |
|----------|----------|-----------|
| moltcoin | Token | 9 |
| moltdao | Governance | 6 |
| moltswap | DEX | 10 |
| moltyid | Identity | 34 |
| moltoracle | Oracle | 13 |
| moltauction | NFT Auction | 10 |
| moltmarket | NFT Market | 6 |
| moltpunks | NFT Collection | 8 |
| clawpump | Bonding Curve | 8 |
| clawvault | Yield Vault | 7 |
| lobsterlend | Lending | 7 |
| reef_storage | Storage | 7 |
| moltbridge | Bridge | 8 |
| compute_market | Compute | 8 |
| bountyboard | Bounties | 8 |
| clawpay | Streaming | 7 |

Each function: name, parameters (types), return codes, description, storage keys affected. Data defined in `js/contract-data.js`.

### Page 11: MoltyID Guide (`moltyid.html`)

**Layout:** Guide-style with diagrams

Sections:
1. **What is MoltyID** — Universal AI agent identity layer
2. **.molt Naming** — Registration, resolution, reverse lookup, transfer, renewal, costs
3. **Reputation System** — How reputation accrues, typed feedback, achievements
4. **Trust Tiers** — Table: tier 0-5, thresholds, privileges, fee discounts, mempool priority
5. **Agent Discovery** — Endpoints, metadata, availability, rate setting
6. **Agent Profile** — Full profile assembly, what data is returned
7. **Identity-Gated Access** — How other contracts use MoltyID (MoltBridge, Compute Market, BountyBoard, ClawPay, MoltSwap)
8. **Integration Guide** — Step-by-step for contract developers to add MoltyID gates
9. **Web of Trust** — Vouching, attestations, skill verification
10. **SDK Examples** — Register identity, register name, check reputation (JS/Python/Rust)

### Page 12: CLI Reference (`cli-reference.html`)

**Layout:** Command list sidebar + main content

Categories:
- **Identity:** `molt identity new`, `molt identity show`, `molt identity export`
- **Wallet:** `molt wallet new`, `molt wallet import`, `molt balance`, `molt transfer`
- **Contracts:** `molt deploy`, `molt call`, `molt contract info`
- **Validator:** `molt validator start`, `molt validator stake`, `molt validator status`
- **Network:** `molt health`, `molt version`, `molt config`
- **Query:** `molt block`, `molt tx`, `molt account`

Each command: syntax, flags/options, example, expected output.

### Page 13: Validator Guide (`validator.html`)

**Layout:** Step-by-step guide

Sections:
1. **Requirements** — Hardware, OS, network ports
2. **Installation** — Build from source, Docker
3. **Configuration** — `config.toml` reference (every field documented)
4. **Genesis** — Joining a network, genesis block
5. **Staking** — How to stake, validator set, ReefStake
6. **Monitoring** — Prometheus metrics, health endpoints, log levels
7. **Deployment** — systemd service, Docker compose, security hardening
8. **Troubleshooting** — Common issues and resolutions

### Page 14: Changelog (`changelog.html`)

**Layout:** Timeline-style

Entries:
- Version, date, breaking changes, new features, fixes
- Auto-linkable (anchor IDs per version)

---

## Design Specifications

### Matches Explorer + Marketplace exactly:

```html
<link rel="stylesheet" href="../shared-base-styles.css">
<link rel="stylesheet" href="../shared-theme.css">
<link rel="stylesheet" href="css/developers.css">
```

### Portal-Specific CSS (`css/developers.css`)

Needed components beyond shared theme:
- **Sidebar navigation** — Fixed left sidebar (250px) with collapsible sections, active state highlighting, scroll sync with content headers
- **Code blocks** — Syntax-highlighted `<pre><code>` blocks with copy button, language label pill, line numbers for long blocks
- **Tabbed content** — JS/Python/Rust/CLI tab switcher on code examples (pills style matching shared `.pill` class)
- **Method cards** — Bordered cards for API methods with param tables, copy-able curl examples
- **Search overlay** — `Ctrl+K` / `Cmd+K` command palette for searching all methods/commands
- **Table of contents** — Auto-generated from h2/h3 headings, sticky position, scroll-spy active state
- **Accordion** — For contract reference expandable sections
- **Breadcrumbs** — `Developers > SDK > JavaScript > Connection` navigation trail
- **Version badge** — Shows current API version in nav
- **Responsive** — Sidebar collapses to hamburger on mobile, code blocks scroll horizontally

### Color Usage (from shared-base-styles.css):
- Background: `#0A0E27` (body), `#141830` (cards)
- Primary: `#FF6B35` (orange — CTAs, active nav, links)
- Secondary: `#004E89` (blue — info boxes)
- Success: `#06D6A0` (green — success codes)
- Text: `#FFFFFF` (headings), `#A0AEC0` (body), `#718096` (muted)
- Code: `#1A1F3A` background, JetBrains Mono font
- Borders: `rgba(255, 255, 255, 0.06)`

### Nav Pattern:
```html
<nav class="nav">
  <div class="nav-container">
    <div class="nav-logo">
      <img src="assets/MoltDev_Logo_256.png" class="logo-icon" alt="MoltChain Dev">
      <span class="logo-text">MoltChain Dev</span>
    </div>
    <ul class="nav-menu">
      <li><a href="index.html">Hub</a></li>
      <li><a href="getting-started.html">Guides</a></li>
      <li><a href="rpc-reference.html">API</a></li>
      <li><a href="sdk-js.html">SDK</a></li>
      <li><a href="contracts.html">Contracts</a></li>
      <li><a href="cli-reference.html">CLI</a></li>
      <li><a href="moltyid.html">MoltyID</a></li>
    </ul>
    <div class="nav-actions">
      <div class="search-container">
        <input type="text" id="searchInput" class="search-input" placeholder="Search docs... (⌘K)">
        <i class="fas fa-search search-icon"></i>
      </div>
      <select id="devNetworkSelect" class="network-select">
        <option value="mainnet">Mainnet</option>
        <option value="testnet">Testnet</option>
        <option value="local-testnet" selected>Local Testnet</option>
      </select>
    </div>
  </div>
</nav>
```

---

## JavaScript Specifications

### `js/developers.js` (~300 lines)
- Sidebar toggle + mobile responsive
- Scroll-spy: highlights current section in sidebar/TOC as user scrolls
- Code copy button: click to copy code block contents
- Tab switcher: JS/Python/Rust/CLI language tabs (persist choice in localStorage)
- Search: `Cmd+K` opens overlay, fuzzy search through method/command index
- Network selector: same pattern as explorer (stores in localStorage, updates API example endpoints)
- Smooth-scroll anchor navigation

### `js/rpc-data.js` (~500 lines)
Array of all RPC methods extracted from `rpc/src/lib.rs`:
```js
const RPC_METHODS = [
  {
    name: "getBalance",
    category: "Account",
    description: "Returns the balance of an account in shells",
    params: [{ name: "address", type: "string", description: "Base58-encoded public key" }],
    returns: { type: "object", fields: [{ name: "balance", type: "number" }, { name: "reputation", type: "number" }] },
    example: {
      request: '{"jsonrpc":"2.0","id":1,"method":"getBalance","params":["<address>"]}',
      response: '{"jsonrpc":"2.0","id":1,"result":{"balance":1000000000,"reputation":0}}'
    }
  },
  // ... all methods
];
```

### `js/contract-data.js` (~800 lines)
Array of all 16 contracts with every exported function:
```js
const CONTRACTS = [
  {
    name: "moltyid",
    displayName: "MoltyID — Universal Identity",
    category: "Identity",
    description: "AI agent identity, .molt naming, reputation, trust tiers",
    functions: [
      {
        name: "register_identity",
        description: "Register a new MoltyID identity",
        params: [{ name: "name_ptr", type: "*const u8" }, { name: "name_len", type: "u32" }],
        returns: "0 = success, 1 = already registered, 2 = name too long",
        storageKeys: ["id:{hex(caller)}", "id_count"]
      },
      // ... all 34 functions
    ]
  },
  // ... all 16 contracts
];
```

### `js/cli-data.js` (~200 lines)
Array of CLI commands parsed from `cli/src/main.rs`:
```js
const CLI_COMMANDS = [
  {
    command: "molt wallet new",
    category: "Wallet",
    description: "Create a new wallet with BIP39 mnemonic",
    flags: ["--output <path>", "--words <12|24>"],
    example: "molt wallet new --words 24",
    output: "Mnemonic: word1 word2 ...\nPublic Key: 5abc...\nSaved to ~/.moltchain/wallet.json"
  },
  // ...
];
```

---

## Build Order (14 steps)

| Step | Files | Depends On | Est. Lines |
|------|-------|------------|------------|
| 1 | `css/developers.css` | shared-base-styles, shared-theme | ~400 |
| 2 | `js/developers.js` | — | ~300 |
| 3 | `index.html` (Hub) | CSS, JS | ~250 |
| 4 | `getting-started.html` | CSS, JS | ~400 |
| 5 | `js/rpc-data.js` | Parse from `rpc/src/lib.rs` | ~500 |
| 6 | `rpc-reference.html` | CSS, JS, rpc-data | ~300 |
| 7 | `ws-reference.html` | CSS, JS | ~250 |
| 8 | `sdk-js.html` | CSS, JS, parse `sdk/js/src/` | ~350 |
| 9 | `sdk-python.html` | CSS, JS, parse `sdk/python/` | ~300 |
| 10 | `sdk-rust.html` | CSS, JS, parse `sdk/rust/` | ~300 |
| 11 | `js/contract-data.js` | Parse all 16 contracts | ~800 |
| 12 | `contracts.html` + `contract-reference.html` + `moltyid.html` | CSS, JS, contract-data | ~900 |
| 13 | `js/cli-data.js` + `cli-reference.html` | Parse `cli/src/main.rs` | ~400 |
| 14 | `architecture.html` + `validator.html` + `changelog.html` | CSS, JS | ~700 |

**Total estimated: ~6,150 lines across 20 files**

---

## Priority: What to Build First

1. **CSS + JS framework** (steps 1-2) — everything else depends on this
2. **Hub landing** (step 3) — the entry point
3. **RPC Reference** (steps 5-6) — most useful to developers immediately
4. **Contract guide + MoltyID** (steps 11-12) — the differentiator
5. **SDK docs** (steps 8-10) — needed for onboarding
6. **Getting Started + CLI + Validator** (steps 4, 13-14) — tutorials and ops
