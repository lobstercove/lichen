# MoltChain Production Readiness Plan

> **Created:** 2026-02-19  
> **Goal:** Systematic, section-by-section audit of every component to reach 100% production readiness  
> **Rule:** Nothing is "done" until code has been read, validated, and double-confirmed with a test  

---

## How This Plan Works

1. **Each section** covers one crate, frontend, or subsystem  
2. **Every task** has a checkbox — only checked after the code is read + validated  
3. **Status codes:** `[ ]` = not started, `[~]` = in progress, `[x]` = done + confirmed  
4. **No guessing.** Every task requires reading the actual code, not assuming  
5. **Findings** are logged inline — if something is broken, stubbed, or hardcoded, it's noted right there  
6. **We work top-down:** Core → Contracts → RPC → P2P → Validator → CLI → SDKs → Frontends → Infra → Tests  

---

## Codebase Inventory

| Component | Location | Lines | Files |
|---|---|---|---|
| Core Runtime | `core/src/` | 19,316 | 20 .rs |
| RPC Server | `rpc/src/` | 13,772 | 5 .rs |
| P2P Network | `p2p/src/` | 2,211 | 7 .rs |
| Validator | `validator/src/` | 9,132 | 5 .rs |
| CLI | `cli/src/` | 4,171 | 9 .rs |
| Compiler | `compiler/src/` | 649 | 1 .rs |
| Custody | `custody/src/` | 7,150 | 1 .rs |
| Faucet Backend | `faucet/src/` | 560 | 1 .rs |
| SDK (Rust) | `sdk/rust/src/` + `sdk/src/` | 1,953 | 11 .rs |
| SDK (JS) | `sdk/js/src/` | 1,114 | 6 .ts |
| SDK (Python) | `sdk/python/moltchain/` | 853 | 6 .py |
| Smart Contracts (27) | `contracts/*/src/lib.rs` | ~42,000 | 27 .rs |
| DEX Frontend | `dex/` | 5,341 | 3 files |
| Wallet App | `wallet/` | 9,340 | 5 files |
| Wallet Extension | `wallet/extension/` | ~20,000 | 22 files |
| Explorer | `explorer/` | 11,472 | 21 files |
| Programs Playground | `programs/` | 18,014 | 9 files |
| Marketplace | `marketplace/` | 6,152 | 12 files |
| Faucet Frontend | `faucet/` | 839 | 3 files |
| Website | `website/` | 4,495 | 4 files |
| Monitoring | `monitoring/` | 3,207 | 3 files |
| Developer Docs | `developers/` | 14,691 | 17 files |
| Test Suites | `tests/` | 8,925 | 15 files |
| Scripts | `scripts/` | 5,834 | 32 files |
| Infra / Deploy | `infra/` + `deploy/` | — | docker, nginx, etc |

**Total: ~205,000+ lines of code across ~250+ files**

---

## PHASE 1: CORE RUNTIME (`core/src/` — 19,316 lines)

The foundation. If this is wrong, everything is wrong.

### 1.1 State Management (`state.rs` — 5,719 lines)
- [x] Read all of state.rs — understand data model
- [x] Verify RocksDB column families are properly defined and used
- [x] Verify account creation / lookup — no phantom accounts
- [x] Verify balance updates — overflow/underflow protection
- [x] Verify contract storage read/write — proper key isolation
- [x] Verify genesis seeding — all initial state is correct
- [x] Verify state snapshots / pruning — no unbounded growth
- [x] Check for any hardcoded addresses or balances outside genesis
- [x] Check for race conditions in concurrent state access
- [x] Verify fee deduction happens atomically with transaction execution
- **Findings (7 items — all fixed):**
  - **S-2 (Medium) FIXED:** `next_tx_slot_seq()` lacked mutex — added `tx_slot_seq_lock` (mirrors `event_seq_lock` pattern)
  - **S-3 (Low) FIXED:** `clear_evm_storage()` deleted entries one-by-one — now uses WriteBatch
  - **S-4 (Low) FIXED:** `save_validator_set()` was clear+loop — now single atomic WriteBatch
  - **S-5 (Low) FIXED:** `set_fee_config_full()` wrote 9 keys — now single WriteBatch
  - **S-6 (Low) FIXED:** `set_rent_params()` wrote 2 keys — now single WriteBatch
  - **S-7 (Info) NOTED:** `export_accounts_iter()` loads all entries into memory Vec (acceptable for snapshot exports)
  - **S-8 (Low) FIXED:** `register_evm_address()` forward+reverse — now single WriteBatch

### 1.2 Consensus (`consensus.rs` — 3,355 lines)
- [x] Read all consensus logic
- [x] Verify PoS validator selection is fair and based on stake
- [x] Verify block validation rules — signature, slot, parent hash
- [x] Verify fork choice rule works correctly
- [x] Verify epoch boundaries and leader schedule rotation
- [x] Verify slashing conditions are enforced (not just defined)
- [x] Verify vote processing and finality
- [x] Check for any hardcoded validator keys or skip conditions
- [x] Verify timeout / liveness handling
- **Findings: None.** Solid code — integer sqrt for determinism, u128 intermediates for overflow safety, atomic bootstrap grants, lock-free FinalityTracker, O(1) equivocation detection, 25+ comprehensive tests.

### 1.3 Transaction Processing (`processor.rs` — 3,335 lines)
- [x] Read all of processor.rs
- [x] Verify transaction signature validation
- [x] Verify nonce / replay protection
- [x] Verify fee calculation and deduction
- [x] Verify instruction dispatch to correct contract/program
- [x] Verify multi-instruction transactions execute atomically
- [x] Verify error handling — failed txs don't corrupt state
- [x] Verify cross-program invocation (CPI) if supported
- [x] Check for panics / unwraps that could crash the node
- **Findings: None.** Fee charged before batch (M4 fix), union-find parallel scheduling (O(n) conflict detection), proper batch commit/rollback, 20+ system instruction types fully validated.

### 1.4 Contract Runtime (`contract.rs` — 1,831 lines)
- [x] Read WASM contract execution engine
- [x] Verify memory isolation between contracts
- [x] Verify gas/compute metering is enforced
- [x] Verify contract deployment stores WASM correctly
- [x] Verify contract calls pass correct accounts and data
- [x] Verify return values are properly propagated
- [x] Verify contract upgrade mechanism (if any)
- [x] Check for hardcoded program IDs
- **Findings: None.** Well-sandboxed — no WASI, env-only imports, memory page limits (256p=16MB), unified compute budget (WASM+host=10M), 10K storage entry cap, compiled module cache (PERF-FIX 2), thread-local runtime pool (PERF-FIX 7).

### 1.5 Contract Instruction (`contract_instruction.rs` — 124 lines)
- [x] Verify instruction encoding/decoding
- [x] Verify ABI conformance with what contracts expect
- [x] Verify `Call` vs `Deploy` distinction
- **Findings: None.** Clean enum with JSON serde, roundtrip tested.

### 1.6 Block Production (`block.rs` — 416 lines)
- [x] Verify block structure — header, transactions, hash
- [x] Verify block serialization/deserialization roundtrip
- [x] Verify block size limits
- [x] Verify timestamp handling
- **Findings: None.** Signable hash excludes signature (T3.5 fix), unsigned non-genesis blocks rejected (T1.6 fix), MAX_BLOCK_SIZE=10MB, MAX_TX_PER_BLOCK=10K, proper structure validation.

### 1.7 Account Model (`account.rs` — 350 lines)
- [x] Verify account structure — pubkey, lamports, owner, data
- [x] Verify system program ownership rules
- [x] Verify rent / rent-exempt logic (if applicable)
- **Findings: None.** Balance separation (spendable/staked/locked) with invariant checks, all operations use checked_add/checked_sub with compute-before-mutate pattern, legacy fixup for pre-separation accounts.

### 1.8 Transaction Structure (`transaction.rs` — 255 lines)
- [x] Verify transaction format — signatures, message, instructions
- [x] Verify serialization matches what SDKs send
- [x] Verify signature verification logic
- **Findings: None.** Hash covers both message AND signatures (T3.4 fix), deploy instructions get 4MB data limit (H16 fix), proper structure validation.

### 1.9 Genesis (`genesis.rs` — 468 lines)
- [x] Verify genesis block creation
- [x] Verify initial token supply and distribution
- [x] Verify system accounts are properly created
- [x] Verify all 27 contracts get deployed at genesis
- [x] Verify DEX pairs and AMM pools are seeded
- [x] Verify oracle price feed is seeded
- **Findings: None.** Fee percentage sum validation (AUDIT-FIX 0.8), all-zero fee rejection (AUDIT-FIX 3.23), testnet/mainnet differentiated (AUDIT-FIX 3.22), distribution sums to exactly 1B MOLT.

### 1.10 Mempool (`mempool.rs` — 409 lines)
- [x] Verify transaction queuing and ordering
- [x] Verify duplicate transaction rejection
- [x] Verify mempool size limits
- [x] Verify priority fee ordering
- **Findings: None.** Express lane for Tier 4+ agents, reputation-weighted priority (MoltyID trust tiers), bulk removal optimization (PERF-FIX 9), expiration cleanup.

### 1.11 Network Types (`network.rs` — 314 lines)
- [x] Verify network ID handling (testnet vs mainnet)
- [x] Verify chain ID is enforced in transactions
- **Findings: None.** Clean config with seed nodes, bootstrap peers, peer discovery.

### 1.12 Hash Functions (`hash.rs` — 85 lines)
- [x] Verify SHA-256 or equivalent is used correctly
- [x] Verify no weak hashing for critical paths
- **Findings: None.** SHA-256 via `sha2` crate, zero-alloc `hash_two_parts` (PERF-OPT 7).

### 1.13 EVM Compatibility (`evm.rs` — 979 lines)
- [x] Read EVM module — is it real or stub?
- [x] If real: verify opcode coverage, gas handling
- [x] If stub: document clearly, decide keep or remove
- **Findings: None.** Real implementation via REVM (Prague spec). Deferred state changes through StateBatch (H3 fix), chain ID enforcement (T3.10), spendable-only balance bridging, overflow rejection (M9 fix), u256 shell-boundary validation.

### 1.14 Privacy / ZK (`privacy.rs` — 311 lines)
- [x] Read privacy module — is it real or stub?
- [x] If stub: document, decide scope
- **Findings: None (expected).** Placeholder ZK proofs — correctly defaults to `allow_placeholder_proofs = false` (C10 fix). Framework in place for future Groth16/PLONK implementation.

### 1.15 Multisig (`multisig.rs` — 357 lines)
- [x] Verify multisig account creation
- [x] Verify M-of-N signature validation
- [x] Verify execution when threshold is met
- **Findings: None.** Deduplication in `verify_threshold` (C6 fix), genesis distribution wallets (6 allocations = 1B MOLT), 3/5 mainnet threshold, 2/3 testnet threshold.

### 1.16 NFT (`nft.rs` — 96 lines)
- [x] Verify NFT mint / transfer / burn
- [x] Check if this is a real implementation or stub
- **Findings: None.** Data types only (CollectionState, TokenState, MintNftData). Implementation is in processor.rs system instructions.

### 1.17 Marketplace Core (`marketplace.rs` — 36 lines)
- [x] Check if real or stub
- **Findings: None.** Activity tracking types only (MarketActivity, MarketActivityKind). Actual marketplace logic is in the WASM contract.

### 1.18 ReefStake (`reefstake.rs` — 622 lines)
- [x] Verify staking logic
- [x] Verify reward calculation
- [x] Verify delegation model
- [x] Verify unstaking with cooldown
- **Findings: None.** Integer-only math (u128 intermediates), 4 lock tiers with reward multipliers, exchange rate via fixed-point (1e9 precision), dust-free reward distribution (AUDIT-FIX CP-5), cooldown from consensus constant (AUDIT-FIX CP-4), total_molt_staked decremented at request time (M10 fix).

### 1.19 Event Stream (`event_stream.rs` — 182 lines)
- [x] Verify event emission for subscriptions
- [x] Verify event types cover all important state changes
- **Findings: None.** Clean typed event enum (10 variants), buffer with drain semantics.

### 1.20 Lib / Exports (`lib.rs` — 72 lines)
- [x] Verify all modules are properly exported
- **Findings: None.** All 20 modules exported, comprehensive re-exports.

---

## PHASE 2: SMART CONTRACTS (27 contracts — ~42,000 lines)

Each contract must be validated for: correct opcode dispatch, proper authority checks, no overflow, no re-entrancy, proper error handling, and ABI accuracy.

### 2.1 Token Contracts
- [ ] `moltcoin` (380 lines) — Native token, mint/transfer/burn, supply cap
- [ ] `musd_token` (1,178 lines) — Stablecoin, mint/burn authority, peg mechanism
- [ ] `weth_token` (853 lines) — Wrapped ETH, 1:1 bridge backing
- [ ] `wsol_token` (853 lines) — Wrapped SOL, 1:1 bridge backing
- [ ] Verify all 4 tokens: transfer, approve, balance, supply cap enforcement
- [ ] Verify ABI matches actual opcodes for all 4
- [ ] **Findings:**

### 2.2 DEX Contracts
- [ ] `dex_core` (3,062 lines) — CLOB engine, order matching, pair management
- [ ] `dex_amm` (1,507 lines) — AMM pools, add/remove liquidity, swap
- [ ] `dex_router` (1,156 lines) — Multi-hop routing, best price
- [ ] `dex_margin` (1,679 lines) — Margin positions, liquidation
- [ ] `dex_rewards` (1,032 lines) — Trading rewards, tier system
- [ ] `dex_analytics` (1,085 lines) — Volume tracking, 24h stats
- [ ] `dex_governance` (1,431 lines) — DEX parameter proposals, voting
- [ ] Verify cross-contract calls between DEX contracts
- [ ] Verify order matching engine correctness
- [ ] Verify liquidation math (no bad debt)
- [ ] Verify ABI matches actual opcodes for all 7
- [ ] **Findings:**

### 2.3 DeFi Contracts
- [ ] `lobsterlend` (1,450 lines) — Lending/borrowing, interest rates, collateral
- [ ] `moltswap` (1,405 lines) — Token swap, AMM, staking
- [ ] `clawpay` (1,375 lines) — Payment streams/splits
- [ ] `clawvault` (1,445 lines) — Vault strategy, yield
- [ ] `clawpump` (1,687 lines) — Token launchpad, bonding curve
- [ ] Verify interest rate math (no overflow at scale)
- [ ] Verify collateral ratio enforcement
- [ ] Verify ABI matches actual opcodes for all 5
- [ ] **Findings:**

### 2.4 Infrastructure Contracts
- [ ] `moltbridge` (2,078 lines) — Cross-chain bridge, relayers, proofs
- [ ] `moltoracle` (1,248 lines) — Price feeds, data providers, staleness
- [ ] `moltdao` (1,380 lines) — Governance, proposals, voting, treasury
- [ ] `reef_storage` (1,346 lines) — Decentralized storage, pinning
- [ ] `compute_market` (2,017 lines) — Compute job marketplace
- [ ] `bountyboard` (1,136 lines) — Bug bounties, task rewards
- [ ] Verify oracle staleness protection
- [ ] Verify bridge security (no unauthorized mints)
- [ ] Verify ABI matches actual opcodes for all 6
- [ ] **Findings:**

### 2.5 NFT & Social Contracts
- [ ] `moltpunks` (586 lines) — NFT collection, mint, trade
- [ ] `moltmarket` (943 lines) — NFT marketplace, listings, bids
- [ ] `moltauction` (1,314 lines) — Auction mechanism, timed bids
- [ ] `moltyid` (5,590 lines) — Decentralized identity, credentials, agents
- [ ] `prediction_market` (3,560 lines) — Binary + multi-outcome markets
- [ ] Verify MoltyID auth chain is real
- [ ] Verify prediction market resolution is trustless
- [ ] Verify ABI matches actual opcodes for all 5
- [ ] **Findings:**

---

## PHASE 3: RPC SERVER (`rpc/src/` — 13,772 lines)

### 3.1 Core RPC (`lib.rs` — 9,004 lines)
- [x] Read all RPC methods — 80+ native Molt, 12 Solana-compat, 15+ EVM-compat
- [x] Verify `getBalance` reads real state ✅ (shells, spendable, staked, locked, ReefStake)
- [x] Verify `getAccountInfo` returns real data ✅ (full account structure)
- [x] Verify `getTransaction` / `getBlock` return real data ✅ (O(1) via tx_slot index)
- [x] Verify `sendTransaction` — full lifecycle ✅ (sig verify → payer balance → fee+transfer afford → execute)
- [x] Verify `getSlot` / `getRecentBlockhash` accuracy ✅ (commitment levels supported)
- [x] Verify `getTokenBalance` accuracy ✅ (reads from contract storage)
- [x] Verify `getProgramStorage` reads real data ✅ (CF_CONTRACT_STORAGE prefix iter O(limit))
- [x] Verify error codes match expected spec ✅
- [x] Check for any stubbed methods returning fake data — **8 EVM stubs found** (see findings)
- [x] Check for any methods that should exist but don't — N/A, comprehensive coverage
- [x] Verify rate limiting / request size limits ✅ (per-IP 5000/sec, 2MB body, no X-Forwarded-For)
- [x] Verify CORS configuration ✅ (restrictive allowlist: localhost, 127.0.0.1, moltchain.io subdomains)
- [x] **Findings:**
  - **F1 (STUB):** `preBalances`/`postBalances` always empty arrays in Solana TX JSON (~L535,560,605)
  - **F2 (STUB):** `eth_getCode` always returns "0x" — never checks contract storage (~L1170)
  - **F3 (STUB):** `eth_getTransactionCount` always returns "0x0" (~L1182)
  - **F4 (STUB):** `eth_estimateGas` hardcoded to 21000 (~L1163)
  - **F5 (STUB):** `eth_gasPrice` hardcoded to 1 Gwei (~L1161)
  - **F6 (STUB):** `eth_getBlockByNumber`/`eth_getBlockByHash` minimal fake structures (~L1186-1203)
  - **F7 (STUB):** `eth_getLogs` returns empty array (~L1206)
  - **F8 (STUB):** `eth_getStorageAt` returns zero (~L1207)
  - **F9 (LOW):** `commission_rate: 5` hardcoded in getValidatorInfo (~L3900)
  - **F10 (LOW):** `is_active: true` hardcoded in getValidatorInfo (~L3901)
  - **VERIFIED OK:** All 80+ native Molt methods, all 12 Solana-compat methods, 7 real EVM methods (eth_getBalance, eth_sendRawTransaction, eth_call, eth_blockNumber, eth_getTransactionReceipt, eth_getTransactionByHash), all staking/MoltyID/NFT/marketplace/token/airdrop/prediction/DEX stats endpoints — ALL read real on-chain data.

### 3.2 DEX REST API (`dex.rs` — 2,045 lines)
- [x] Verify all GET endpoints read real on-chain data ✅ (all use `get_program_storage()` O(1) reads)
- [x] Verify POST endpoints return 405 correctly ✅ (orders, margin open/close, vote)
- [x] Verify symbol enrichment works ✅ (30s TTL symbol map cache)
- [x] Verify pagination / limits on list endpoints ✅
- [x] Verify binary decode functions validate lengths ✅ (pair=112B, order=128B, trade=80B, pool=96B, etc.)
- [x] Verify no endpoint returns hardcoded/mock data — **3 issues found** (see findings)
- [x] **Findings:**
  - **F11 (MEDIUM):** Orderbook scans up to 10,000 orders linearly — O(N) per request (perf concern at scale)
  - **F14 (MEDIUM):** `post_router_swap` emits WS trade/ticker events from READ-ONLY quote — phantom trades pollute real-time feed
  - **F15 (BUG):** Router slippage check uses `amount_in * (1 - slippage/100)` as min output — wrong for non-1:1 pairs, should be based on expected output
  - **F16 (LOW):** `post_create_proposal` returns 200 with proposal JSON but does NOT persist to storage (comment says "use sendTransaction" but response is misleading)
  - **F17 (LOW):** CLOB route type fallback: if no AMM pool, uses 1:1 quote with no actual CLOB quoting logic
  - **VERIFIED OK:** All GET endpoints (pairs, orderbook, trades, candles, stats, tickers, orders, pools, positions, margin, leaderboard, rewards, governance, all stats), AMM math mirrors contract exactly, DELETE /orders returns 405.

### 3.3 Prediction Market API (`prediction.rs` — 959 lines)
- [x] Verify POST /create goes through WASM contract — **NO, writes directly to CF_CONTRACT_STORAGE** (F13)
- [x] Verify POST /trade executes on-chain — **NO, preview only but emits WS event** (F12)
- [x] Verify GET endpoints return real on-chain data ✅ (all use O(1) CF_CONTRACT_STORAGE reads)
- [x] Verify market lifecycle — create works (but no auth), trade is preview-only, resolve/claim not in REST API
- [x] **Findings:**
  - **F12 (MEDIUM):** POST /trade returns `status: "preview"` but emits WS trade event — phantom events in real-time feed
  - **F13 (MEDIUM):** POST /create writes directly to CF_CONTRACT_STORAGE without admin auth or signature verification — anyone can create markets
  - **VERIFIED OK:** GET stats, markets (paginated+filtered), markets/:id, price-history, analytics, positions, traders/:addr/stats, leaderboard, trending — all read real data.

### 3.4 WebSocket Server (`ws.rs` — 1,429 lines)
- [x] Verify subscription to blocks / transactions / accounts ✅ (20+ subscription types)
- [x] Verify real-time notifications work ✅ (broadcast channels, event forwarding tasks)
- [x] Verify unsubscribe works ✅ (subscription removal on request)
- [x] Verify connection cleanup on disconnect ✅ (IP counter decrement, task abort)
- [x] Verify heartbeat / ping-pong ✅ (15s ping interval)
- [x] **Findings:**
  - No critical issues. DDoS protection in place (MAX_WS=500, per-IP=10, per-conn subs=100). Lagged subscribers handled gracefully. All clean.

### 3.5 DEX WebSocket (`dex_ws.rs` — 340 lines)
- [x] Verify orderbook streaming ✅ (DexEvent::OrderBookUpdate)
- [x] Verify trade notifications ✅ (DexEvent::TradeExecution)
- [x] Verify subscription management ✅ (channel parsing, matching)
- [x] **Findings:**
  - No issues. Clean, well-structured code.

---

## PHASE 4: P2P NETWORK (`p2p/src/` — 2,211 lines)

### 4.1 Network Layer (`network.rs` — 603 lines)
- [ ] Verify peer discovery / bootstrap
- [ ] Verify QUIC transport setup
- [ ] Verify message routing
- [ ] **Findings:**

### 4.2 Peer Management (`peer.rs` — 656 lines)
- [ ] Verify peer handshake / versioning
- [ ] Verify peer scoring
- [ ] Verify max peer limits
- [ ] **Findings:**

### 4.3 Gossip Protocol (`gossip.rs` — 325 lines)
- [ ] Verify block propagation
- [ ] Verify transaction propagation
- [ ] Verify no message amplification attacks
- [ ] **Findings:**

### 4.4 Peer Banning (`peer_ban.rs` — 191 lines)
- [ ] Verify ban criteria and duration
- [ ] Verify ban persistence across restarts
- [ ] **Findings:**

### 4.5 Peer Store (`peer_store.rs` — 179 lines)
- [ ] Verify peer persistence
- [ ] Verify address rotation
- [ ] **Findings:**

### 4.6 Messages (`message.rs` — 238 lines)
- [ ] Verify message types cover all network operations
- [ ] Verify serialization format
- [ ] Verify size limits
- [ ] **Findings:**

---

## PHASE 5: VALIDATOR (`validator/src/` — 9,132 lines)

### 5.1 Main Loop (`main.rs` — 7,517 lines)
- [ ] Verify block production cycle
- [ ] Verify transaction processing pipeline
- [ ] Verify leader rotation
- [ ] Verify RocksDB initialization and column families
- [ ] Verify genesis creation on first boot
- [ ] Verify state persistence across restarts
- [ ] Verify graceful shutdown
- [ ] Verify dev-mode flag behavior
- [ ] Verify CLI argument parsing
- [ ] Check for any hardcoded genesis data that should be configurable
- [ ] Verify contract deployment at genesis
- [ ] Verify DEX pair / pool / oracle seeding at genesis
- [ ] **Findings:**

### 5.2 Sync (`sync.rs` — 412 lines)
- [ ] Verify block sync from peers
- [ ] Verify chain catch-up logic
- [ ] Verify sync doesn't accept invalid blocks
- [ ] **Findings:**

### 5.3 Keypair Loader (`keypair_loader.rs` — 141 lines)
- [ ] Verify keypair generation and persistence
- [ ] Verify keypair file format
- [ ] Verify machine migration support
- [ ] **Findings:**

### 5.4 Threshold Signer (`threshold_signer.rs` — 303 lines)
- [ ] Verify threshold signature scheme
- [ ] Verify key share generation and reconstruction
- [ ] **Findings:**

### 5.5 Updater (`updater.rs` — 759 lines)
- [ ] Verify update mechanism (auto-update binary)
- [ ] Verify signature verification on updates
- [ ] Verify rollback capability
- [ ] **Findings:**

---

## PHASE 6: CLI (`cli/src/` — 4,171 lines)

### 6.1 Command Coverage
- [ ] Verify all commands: balance, transfer, airdrop, deploy, call
- [ ] Verify keypair generation and management
- [ ] Verify transaction signing
- [ ] Verify RPC client connectivity
- [ ] Verify contract deployment via CLI
- [ ] Verify contract call via CLI
- [ ] Verify output formatting (JSON, human-readable)
- [ ] **Findings:**

### 6.2 Client (`client.rs` — 783 lines)
- [ ] Verify RPC call construction
- [ ] Verify error handling from RPC
- [ ] **Findings:**

### 6.3 Wallet (`wallet.rs` — 287 lines)
- [ ] Verify wallet create / import / export
- [ ] Verify private key handling (secure, not logged)
- [ ] **Findings:**

---

## PHASE 7: COMPILER (`compiler/src/` — 649 lines)

- [ ] Verify Rust-to-WASM compilation pipeline
- [ ] Verify output format matches what contract runtime expects
- [ ] Verify optimization passes
- [ ] Verify error reporting
- [ ] Test: compile a sample contract and deploy it
- [ ] **Findings:**

---

## PHASE 8: CUSTODY SERVICE (`custody/src/` — 7,150 lines)

- [ ] Verify key management — HSM integration or secure storage
- [ ] Verify signing flow — approval, threshold, audit trail
- [ ] Verify API surface — no unauthorized signing
- [ ] Verify rate limits on signing operations
- [ ] Verify audit logging
- [ ] **Findings:**

---

## PHASE 9: SDKs

### 9.1 Rust SDK (`sdk/rust/src/` — 614 lines, `sdk/src/` — 1,339 lines)
- [ ] Verify client connection to RPC
- [ ] Verify transaction construction
- [ ] Verify keypair generation
- [ ] Verify all RPC methods are wrapped
- [ ] Verify DEX / NFT / Token helper modules
- [ ] Test: send a real transaction using Rust SDK
- [ ] **Findings:**

### 9.2 JavaScript SDK (`sdk/js/src/` — 1,114 lines)
- [ ] Verify connection module — all RPC methods
- [ ] Verify keypair module — ed25519 signing
- [ ] Verify transaction module — serialization format
- [ ] Verify bincode module — encoding/decoding
- [ ] Test: send a real transaction using JS SDK
- [ ] **Findings:**

### 9.3 Python SDK (`sdk/python/moltchain/` — 853 lines)
- [ ] Verify connection module — all RPC methods
- [ ] Verify keypair module — ed25519 signing
- [ ] Verify transaction module — serialization
- [ ] Test: send a real transaction using Python SDK
- [ ] **Findings:**

---

## PHASE 10: DEX FRONTEND (`dex/` — 5,341 lines)

### 10.1 Trading View
- [ ] Verify pair selector loads real pairs from API
- [ ] Verify orderbook renders real data
- [ ] Verify chart/TradingView integration
- [ ] Verify trade history loads real trades
- [ ] Verify order form submits via sendTransaction (not REST POST)
- [ ] Verify open orders tab shows user's real orders
- [ ] Verify order cancellation works
- [ ] Verify ticker updates reflect real state
- [ ] **Findings:**

### 10.2 Pool / Liquidity
- [ ] Verify pool list loads from API
- [ ] Verify add/remove liquidity forms work
- [ ] Verify LP position display
- [ ] **Findings:**

### 10.3 Margin Trading
- [ ] Verify margin position display
- [ ] Verify position open/close flow
- [ ] Verify liquidation warnings
- [ ] **Findings:**

### 10.4 Prediction Markets
- [ ] Verify market list loads from API
- [ ] Verify binary market creation works end-to-end
- [ ] Verify multi-outcome market creation (2-8 outcomes)
- [ ] Verify trading on markets works
- [ ] Verify position display
- [ ] Verify resolution and claim flow
- [ ] **Findings:**

### 10.5 Governance
- [ ] Verify proposal list loads from API
- [ ] Verify proposal creation flow
- [ ] Verify voting mechanism
- [ ] Verify proposal state display (active/passed/executed)
- [ ] Verify governance parameters display in detail panel
- [ ] **Findings:**

### 10.6 Rewards
- [ ] Verify reward stats load from API
- [ ] Verify pending/claimed amounts display
- [ ] Verify tier display
- [ ] Verify claim button works
- [ ] **Findings:**

### 10.7 Wallet Integration
- [ ] Verify connect wallet flow
- [ ] Verify import via private key
- [ ] Verify wallet create generates real keypair
- [ ] Verify balance display after connect
- [ ] Verify all wallet-gated sections hide when disconnected
- [ ] Verify wallet-gated sections show when connected
- [ ] Verify no stale wallet data after disconnect
- [ ] **Findings:**

### 10.8 General UI
- [ ] Verify all icons are valid Font Awesome 6
- [ ] Verify responsive/mobile layout
- [ ] Verify dark theme consistency
- [ ] Verify no console errors on load
- [ ] Verify shared-config.js integration
- [ ] Verify WebSocket connection and real-time updates
- [ ] **Findings:**

---

## PHASE 11: WALLET APP (`wallet/` — 9,340 lines)

### 11.1 Core Wallet (`wallet/js/wallet.js` — 3,716 lines)
- [ ] Verify wallet creation (keypair generation)
- [ ] Verify private key import (hex, base58)
- [ ] Verify balance loading from RPC
- [ ] Verify transaction history loading
- [ ] Verify send transaction flow — sign + submit
- [ ] Verify token balance display
- [ ] Verify staking / delegation UI
- [ ] Verify address display is always base58 (never 0x)
- [ ] **Findings:**

### 11.2 Crypto Module (`wallet/js/crypto.js` — 470 lines)
- [ ] Verify ed25519 key generation
- [ ] Verify signing / verification
- [ ] Verify base58 encoding/decoding
- [ ] **Findings:**

### 11.3 Identity Module (`wallet/js/identity.js` — 1,180 lines)
- [ ] Verify MoltyID integration
- [ ] Verify credential management
- [ ] Verify agent registration
- [ ] **Findings:**

### 11.4 UI / HTML (`wallet/index.html` — 926 lines)
- [ ] Verify all sections render correctly
- [ ] Verify responsive layout
- [ ] Verify no broken links or icons
- [ ] **Findings:**

---

## PHASE 12: WALLET EXTENSION (`wallet/extension/` — ~20,000 lines)

### 12.1 Popup (`popup.js` — 54K, `popup.html` — 23K)
- [ ] Verify account management
- [ ] Verify transaction approval flow
- [ ] Verify dApp connection
- [ ] Verify network switching
- [ ] **Findings:**

### 12.2 Full Page (`full.js` — 99K, `full.html` — 40K)
- [ ] Verify extended wallet features
- [ ] Verify settings management
- [ ] Verify backup/restore
- [ ] **Findings:**

### 12.3 Content Script (`content-script.js` — 3.6K)
- [ ] Verify injection mechanism
- [ ] Verify message passing to/from popup
- [ ] **Findings:**

### 12.4 In-Page Provider (`inpage-provider.js` — 4.5K)
- [ ] Verify `window.moltwallet` API surface
- [ ] Verify `window.ethereum` compatibility shim
- [ ] Verify no 0x address leaks into MoltChain pages
- [ ] Verify event handling (accountsChanged, etc.)
- [ ] **Findings:**

### 12.5 Approval Pages
- [ ] Verify transaction approval UI
- [ ] Verify permission request UI
- [ ] **Findings:**

---

## PHASE 13: EXPLORER (`explorer/` — 11,472 lines)

### 13.1 Dashboard (`index.html` + `explorer.js` — 789 lines)
- [ ] Verify latest blocks display
- [ ] Verify latest transactions display
- [ ] Verify network stats (TPS, slot, epoch)
- [ ] Verify search functionality
- [ ] **Findings:**

### 13.2 Block Detail (`block.html` + `block.js` — 295 lines)
- [ ] Verify block data loads from RPC
- [ ] Verify transactions list in block
- [ ] **Findings:**

### 13.3 Transaction Detail (`transaction.html` + `transaction.js` — 471 lines)
- [ ] Verify transaction data loads from RPC
- [ ] Verify instruction display
- [ ] Verify signature verification UI
- [ ] **Findings:**

### 13.4 Address / Account (`address.html` + `address.js` — 2,039 lines)
- [ ] Verify account balance display
- [ ] Verify transaction history for address
- [ ] Verify token balances
- [ ] Verify contract data display
- [ ] **Findings:**

### 13.5 Contracts List (`contracts.html` + `contracts.js` — 241 lines)
- [ ] Verify deployed contracts list
- [ ] Verify contract detail view
- [ ] **Findings:**

### 13.6 Validators (`validators.html` + `validators.js` — 162 lines)
- [ ] Verify validator list from RPC
- [ ] Verify stake / commission display
- [ ] **Findings:**

### 13.7 Agents (`agents.html` + `agents.js` — 215 lines)
- [ ] Verify agent list display
- [ ] Verify MoltyID integration
- [ ] **Findings:**

---

## PHASE 14: PROGRAMS PLAYGROUND (`programs/` — 18,014 lines)

### 14.1 Landing Page (`index.html` — 1,896 lines)
- [ ] Verify showcase / documentation
- [ ] Verify links to playground
- [ ] **Findings:**

### 14.2 Playground (`playground.html` + `playground-complete.js` — 8,772 lines)
- [ ] Verify code editor works
- [ ] Verify compile button compiles real Rust to WASM
- [ ] Verify deploy sends real transaction to chain
- [ ] Verify contract interaction after deploy
- [ ] Verify example contracts load correctly
- [ ] Verify error display from compilation/deployment
- [ ] Verify wallet connection for signing deploys
- [ ] **Findings:**

### 14.3 SDK Module (`moltchain-sdk.js` — 1,387 lines)
- [ ] Verify RPC methods
- [ ] Verify transaction construction
- [ ] Verify keypair handling
- [ ] **Findings:**

---

## PHASE 15: MARKETPLACE (`marketplace/` — 6,152 lines)

### 15.1 Browse / Listings
- [ ] Verify NFT listings load from chain
- [ ] Verify search / filter
- [ ] Verify listing detail
- [ ] **Findings:**

### 15.2 Create / Mint
- [ ] Verify NFT minting flow
- [ ] Verify metadata upload
- [ ] Verify listing creation
- [ ] **Findings:**

### 15.3 Profile
- [ ] Verify user profile loads owned NFTs
- [ ] Verify transaction history
- [ ] **Findings:**

---

## PHASE 16: FAUCET (`faucet/` — backend 560 lines + frontend 839 lines)

- [ ] Verify airdrop request flow — frontend to backend to chain
- [ ] Verify rate limiting (no faucet drain)
- [ ] Verify amount limits
- [ ] Verify address validation
- [ ] Verify transaction confirmation display
- [ ] **Findings:**

---

## PHASE 17: MONITORING (`monitoring/` — 3,207 lines)

- [ ] Verify dashboard connects to real validator metrics
- [ ] Verify TPS / block time / peer count display
- [ ] Verify alerting thresholds
- [ ] **Findings:**

---

## PHASE 18: WEBSITE (`website/` — 4,495 lines)

- [ ] Verify landing page content accuracy
- [ ] Verify ecosystem links work
- [ ] Verify no broken assets
- [ ] Verify responsive layout
- [ ] **Findings:**

---

## PHASE 19: DEVELOPER DOCS (`developers/` — 14,691 lines)

### 19.1 API Documentation
- [ ] Verify RPC reference matches actual RPC methods
- [ ] Verify WebSocket reference matches actual WS methods
- [ ] Verify CLI reference matches actual CLI commands
- [ ] Verify contract reference matches actual ABI functions
- [ ] **Findings:**

### 19.2 SDK Documentation
- [ ] Verify JS SDK docs match actual API
- [ ] Verify Python SDK docs match actual API
- [ ] Verify Rust SDK docs match actual API
- [ ] **Findings:**

### 19.3 Tutorials
- [ ] Verify getting-started guide works end-to-end
- [ ] Verify validator setup guide works
- [ ] Verify contract deployment guide works
- [ ] **Findings:**

---

## PHASE 20: INFRASTRUCTURE & DEPLOYMENT

### 20.1 Docker
- [ ] Verify Dockerfile builds correctly
- [ ] Verify docker-compose.yml starts full stack
- [ ] Verify all services connect properly
- [ ] **Findings:**

### 20.2 Nginx Config
- [ ] Verify reverse proxy routes for all services
- [ ] Verify SSL/TLS configuration
- [ ] Verify CORS headers
- [ ] **Findings:**

### 20.3 Monitoring Stack
- [ ] Verify Prometheus metrics collection
- [ ] Verify Grafana dashboards
- [ ] **Findings:**

### 20.4 Deployment Scripts
- [ ] Verify `deploy/setup.sh` works
- [ ] Verify systemd service file
- [ ] Verify `scripts/setup-validator.sh` for new validators
- [ ] Verify `scripts/testnet-deploy.sh` for testnet launch
- [ ] **Findings:**

---

## PHASE 21: TEST COVERAGE & E2E

### 21.1 Unit Tests
- [ ] Run `cargo test` — all must pass
- [ ] Identify modules with 0 test coverage
- [ ] Add tests for critical paths lacking coverage
- [ ] **Findings:**

### 21.2 E2E Test Suites
- [ ] `test-dex-api-comprehensive.sh` — REST API (98 tests)
- [ ] `e2e-dex-trading.py` — Full trading flow
- [ ] `comprehensive-e2e.py` — RPC + contract calls
- [ ] `contracts-write-e2e.py` — Contract write operations
- [ ] `e2e-websocket-upgrade.py` — WebSocket upgrade tests
- [ ] `production-e2e-gate.sh` — Production gate check
- [ ] Verify all tests pass on fresh chain
- [ ] **Findings:**

### 21.3 SDK Tests
- [ ] Test JS SDK end-to-end
- [ ] Test Python SDK end-to-end
- [ ] Test Rust SDK end-to-end
- [ ] **Findings:**

---

## PHASE 22: CROSS-CUTTING CONCERNS

### 22.1 Security
- [ ] No private keys logged or exposed in responses
- [ ] No SQL/command injection vectors
- [ ] All inputs validated and bounded
- [ ] No unsigned integer overflow in financial math
- [ ] All authority checks enforced in contracts
- [ ] Rate limiting on public endpoints
- [ ] DoS protection (request size, connection limits)
- [ ] **Findings:**

### 22.2 Performance
- [ ] RocksDB compaction settings optimized
- [ ] No unbounded vectors or allocations
- [ ] No O(n²) loops on chain data
- [ ] WebSocket connection limits
- [ ] Memory profiling under load
- [ ] **Findings:**

### 22.3 Data Integrity
- [ ] All state transitions are atomic
- [ ] No partial writes on crash
- [ ] Serialization format is versioned
- [ ] **Findings:**

### 22.4 Code Quality
- [ ] `cargo clippy` — zero warnings
- [ ] No `unwrap()` in production paths (only in tests)
- [ ] No `todo!()` or `unimplemented!()` in production code
- [ ] No dead code or unused imports
- [ ] No redundant files or old audit docs in repo root
- [ ] `.gitignore` covers all build artifacts
- [ ] **Findings:**

### 22.5 Shared Config & Consistency
- [ ] `shared-config.js` has all service URLs
- [ ] All frontends use `shared-config.js`
- [ ] `shared-theme.css` + `shared-base-styles.css` used everywhere
- [ ] All frontends use same Font Awesome version
- [ ] All frontends have favicon
- [ ] **Findings:**

---

## Progress Summary

| Phase | Section | Tasks | Done | Status |
|---|---|---|---|---|
| 1 | Core Runtime | 65 | 0 | `[ ]` |
| 2 | Smart Contracts | 30 | 0 | `[ ]` |
| 3 | RPC Server | 30 | 0 | `[ ]` |
| 4 | P2P Network | 15 | 0 | `[ ]` |
| 5 | Validator | 18 | 0 | `[ ]` |
| 6 | CLI | 10 | 0 | `[ ]` |
| 7 | Compiler | 5 | 0 | `[ ]` |
| 8 | Custody | 5 | 0 | `[ ]` |
| 9 | SDKs | 15 | 0 | `[ ]` |
| 10 | DEX Frontend | 40 | 0 | `[ ]` |
| 11 | Wallet App | 15 | 0 | `[ ]` |
| 12 | Wallet Extension | 12 | 0 | `[ ]` |
| 13 | Explorer | 14 | 0 | `[ ]` |
| 14 | Programs Playground | 10 | 0 | `[ ]` |
| 15 | Marketplace | 8 | 0 | `[ ]` |
| 16 | Faucet | 5 | 0 | `[ ]` |
| 17 | Monitoring | 3 | 0 | `[ ]` |
| 18 | Website | 4 | 0 | `[ ]` |
| 19 | Developer Docs | 10 | 0 | `[ ]` |
| 20 | Infrastructure | 10 | 0 | `[ ]` |
| 21 | Tests | 10 | 0 | `[ ]` |
| 22 | Cross-Cutting | 25 | 0 | `[ ]` |
| **TOTAL** | | **~359** | **0** | **0%** |

---

## Ground Rules

1. **We go in order.** Phase 1 before Phase 2. No skipping.
2. **Every task = reading actual code.** Not assuming, not guessing.
3. **Findings are logged immediately.** If something is broken, stubbed, hardcoded, missing — it goes in the Findings section right there.
4. **Fixes happen after the phase audit.** We audit first, then batch-fix. No whack-a-mole.
5. **Every fix gets a test.** No "I fixed it, trust me."
6. **Commit after each phase** with a clear message listing what was audited and what was fixed.
7. **No new features.** This plan is about making what exists production-quality.
8. **If something should be removed, we remove it.** Dead code, stub modules, unused files — gone.
9. **If something is a stub and won't be finished for v1, document it clearly** as "not in v1 scope" and disable the UI entry point.

---

## Ready to Begin

Say **"Start Phase 1"** and we begin reading `core/src/state.rs` line by line.
