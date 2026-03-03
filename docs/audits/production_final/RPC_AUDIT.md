# MoltChain RPC Layer — Exhaustive Production Audit

> Audit date: 2026-02-27 | Root: `/moltchain/` | Auditor: Senior Blockchain/DB Engineer

---

## A. COLUMN FAMILY INVENTORY

Defined in `core/src/state.rs` L42-L81. MoltChain uses RocksDB with 34 column families:

| CF Name | Key Format | Value | Scan Strategy |
|---|---|---|---|
| `CF_ACCOUNTS` | pubkey(32) | bincode `Account` (prefix `0xBC`) or JSON | Prefix bloom filter (32-byte SliceTransform). O(1) point lookup. |
| `CF_BLOCKS` | block\_hash(32) | JSON `Block` | Point lookup only. |
| `CF_TRANSACTIONS` | tx\_hash(32) | JSON/bincode `Transaction` | Point lookup only. |
| `CF_ACCOUNT_TXS` | pubkey(32)+slot(8,BE)+seq(4,BE)+tx\_hash(32) | empty | Prefix-reverse scan by pubkey → O(limit), correct. |
| `CF_SLOTS` | slot(8,LE) | block\_hash(32) | Point lookup by slot. |
| `CF_VALIDATORS` | pubkey(32) | JSON `ValidatorInfo` | **Full CF scan only** — no secondary index, but CF is bounded (max ~thousands). |
| `CF_STATS` | string keys | various u64/bytes | Point lookup by named key. |
| `CF_EVM_MAP` | EVM addr (20B) | native pubkey(32) | Point lookup. |
| `CF_EVM_ACCOUNTS` | EVM addr (20B) | EVM account JSON | Point lookup. |
| `CF_EVM_STORAGE` | EVM addr(20)+slot(32) | storage value(32) | Point lookup. |
| `CF_EVM_TXS` / `CF_EVM_RECEIPTS` | EVM tx\_hash | metadata / receipt | Point lookup. |
| `CF_REEFSTAKE` / `CF_STAKE_POOL` | pool\_id / pubkey | pool JSON / stake JSON | Point lookup. |
| `CF_NFT_BY_OWNER` | owner(32)+token(32) | empty | Prefix scan by owner → O(limit). |
| `CF_NFT_BY_COLLECTION` | collection(32)+token(32) OR `"tid:"`+collection(32)+token\_id(8) | empty / token\_pubkey(32) | Prefix scan by collection → O(limit). |
| `CF_NFT_ACTIVITY` | collection(32)+slot(8,BE)+seq(4,BE)+token(32) | activity JSON | Prefix-reverse scan by collection → O(limit). |
| `CF_PROGRAMS` | program\_pubkey(32) | empty or metadata JSON | **Full CF scan** — no secondary index. Grows with every deployed contract. |
| `CF_PROGRAM_CALLS` | program(32)+slot(8,BE)+seq(4,BE)+tx(32) | call JSON | Prefix-reverse scan by program → O(limit). |
| `CF_MARKET_ACTIVITY` | collection(32)+slot(8,BE)+seq(4,BE)+tx(32) | activity JSON | Prefix scan by collection+kind filter, or full scan if no collection. |
| `CF_SYMBOL_REGISTRY` | symbol bytes (variable length) | JSON `SymbolRegistryEntry` | **Full CF scan** in `get_all_symbol_registry`. |
| `CF_EVENTS` | program(32)+slot(8,BE)+seq(8,BE) | JSON `ContractEvent` | Prefix scan by program → O(limit). |
| `CF_TOKEN_BALANCES` | token\_program(32)+holder(32) | u64(8) | Prefix scan by token\_program → O(limit). Reverse-lookup via `CF_HOLDER_TOKENS`. |
| `CF_TOKEN_TRANSFERS` | token\_program(32)+slot(8,BE)+seq(8,BE) | JSON `TokenTransfer` | Prefix-reverse scan by token\_program → O(limit). |
| `CF_TX_BY_SLOT` | slot(8,BE)+seq(8,BE) | tx\_hash(32) | Reverse range scan (total\_order\_seek) across all slots → paginated. |
| `CF_TX_TO_SLOT` | tx\_hash(32) | slot(8,LE) | O(1) reverse index to find slot from hash. |
| `CF_HOLDER_TOKENS` | holder(32)+token\_program(32) | u64(8) balance | Prefix scan by holder → O(limit). |
| `CF_SYMBOL_BY_PROGRAM` | program\_pubkey(32) | symbol bytes | O(1) reverse index. |
| `CF_EVENTS_BY_SLOT` | slot(8,BE)+seq(8,BE) | event\_key | Secondary index for event lookup by slot. |
| `CF_CONTRACT_STORAGE` | contract(32)+storage\_key(var) | storage value | Prefix scan by contract → O(limit). |
| `CF_MERKLE_LEAVES` | pubkey(32) | leaf\_hash(32) | Prefix scan (initial rebuild only). |
| `CF_SHIELDED_COMMITMENTS` | index(8,LE) | commitment(32) | Point lookup by sequential index. |
| `CF_SHIELDED_NULLIFIERS` | nullifier(32) | `0x01` | Point lookup. |
| `CF_SHIELDED_POOL` | `"state"` singleton | JSON `ShieldedPoolState` | Point lookup. |

---

## B. FULL SCAN AUDIT

### Active Full Scans (O(N) over entire CF)

#### ❌ CRITICAL — `get_programs` / `get_all_programs` — CF_PROGRAMS

**Functions**: `get_programs()` (core/src/state.rs#L2687) and `get_all_programs()` (core/src/state.rs#L5940)
**RPC handlers calling them**: `getPrograms` (rpc/src/lib.rs#L6454) and `getAllContracts` (rpc/src/lib.rs#L5728)
**CF scanned**: `CF_PROGRAMS`
**Growth**: 1 entry per deployed contract. With permissionless deployment this could reach millions.
**Current mitigation**: `limit` parameter (max 500 for `getPrograms`, hardcoded 1000 for `getAllContracts`). Loop breaks at limit — not a true "full scan", but it reads from the START every call with no cursor, meaning it always returns the oldest 1000 contracts. Newer contracts are invisible without pagination support.
**Fix**: Add a cursor-based paginated iterator (pass `after_program_pubkey` as an `IteratorMode::From` seek key).

#### ❌ HIGH — `get_all_symbol_registry` — CF_SYMBOL_REGISTRY

**Function**: `get_all_symbol_registry()` (core/src/state.rs#L2755)
**RPC handler**: `getAllSymbolRegistry` (rpc/src/lib.rs#L1452)
**CF scanned**: `CF_SYMBOL_REGISTRY` (variable-length string keys, no ordered cursor)
**Growth**: 1 entry per registered token symbol. Could reach thousands/tens-of-thousands in production.
**Current mitigation**: `limit` cap applied, but no cursor → always reads from CF start.
**Fix**: Order by some deterministic key (e.g., `creation_slot` prefix) with an `after_symbol` cursor parameter.

#### ❌ HIGH — `get_all_validators` — CF_VALIDATORS (partially cached, but bypassed)

**Function**: `get_all_validators()` (core/src/state.rs#L3930)
**Called from**:
- `cached_validators()` (rpc/src/lib.rs#L213) — cached at 400ms TTL ✅
- `require_single_validator()` (rpc/src/lib.rs#L187) — **bypasses cache** ❌

`require_single_validator` is called every time `setFeeConfig`, `setRentParams`, `setContractAbi`, `deployContract`, `upgradeContract`, `requestAirdrop` is invoked — an uncached full CF_VALIDATORS scan on every admin call.

**CF scanned**: `CF_VALIDATORS`
**Growth**: Bounded by consensus design (tens to hundreds of validators). Not catastrophic in isolation.
**Fail-open risk**: `get_all_validators().unwrap_or_default()` — if the DB errors, it returns an empty list, meaning `require_single_validator` silently **grants admin access on DB error**.
**Fix**: Route `require_single_validator` through the same `cached_validators()` function. Return explicit error on DB failure instead of defaulting to empty vec.

#### ⚠️ MEDIUM — `get_market_activity` (without collection filter) — CF_MARKET_ACTIVITY

**Function**: `get_market_activity(collection=None, kind, limit)` (core/src/state.rs#L2508)
**Growth**: Unbounded; accumulates all marketplace activity across all collections.
**Current mitigation**: When `collection=None`, `fetch_limit = limit * 5` up to **2000 records** are fetched then filtered in Rust memory (rpc/src/lib.rs#L8345-L8354).
**Fix**: Add a `kind` prefix to the market activity key: `kind(1)+collection(32)+slot(8)+seq(4)+tx(32)`, enabling prefix scans by kind.

#### ✅ Dead Code (not callable from RPC)

These full scans exist but are private or dead code:
- core/src/state.rs#L1495 — `compute_state_root_full_scan`, CF_ACCOUNTS — legacy fallback
- core/src/state.rs#L1789 — `count_accounts`, CF_ACCOUNTS — migration only
- core/src/state.rs#L1825 — `count_active_accounts_full_scan`, CF_ACCOUNTS — reconciliation only

#### ⚠️ Admin-only Full Scan

- core/src/state.rs#L6155 — `export_cf_page(cf_name, offset, limit)` — scans any CF from the beginning to compute offset+limit. This is O(offset+limit), NOT O(limit), so exporting page 1000 at limit 100 reads 1100 entries. Used only for state sync between validators.
**Fix**: Use a cursor-based seek key instead of count-based offset.

---

## C. N+1 QUERY PATTERNS

### N+1-1: `getAllContracts` — N symbol registry lookups

**Handler**: rpc/src/lib.rs#L5728
**Outer query**: `get_all_programs(1000)` → CF_PROGRAMS full scan up to 1000 programs
**Inner query per program**:
```rust
state.state.get_symbol_registry_by_program(pk)  // CF_SYMBOL_BY_PROGRAM point lookup × N
```
**Estimated DB calls**: 1 + N (up to 1001 for 1000 contracts)
**Fix**: Use `db.multi_get_cf()` to batch all N symbol lookups in one RocksDB call, or store the symbol inline in the CF_PROGRAMS value at write time.

### N+1-2: `getTransactionsByAddress` — N tx lookups + N block lookups

**Handler**: rpc/src/lib.rs#L2511
**Outer query**: `get_account_tx_signatures_paginated` → O(limit) reverse prefix scan of CF_ACCOUNT_TXS
**Inner query per (hash, slot)**:
```rust
state.state.get_transaction(hash)          // CF_TRANSACTIONS point lookup × N
state.state.get_block_by_slot(slot)        // CF_SLOTS + CF_BLOCKS × N (miss fallback)
state.state.get_block_by_slot(slot)        // CF_BLOCKS × distinct_slots (for timestamps)
```
**Estimated DB calls**: limit=50 → 1 scan + up to 50 tx lookups + up to 50 block lookups + up to 50 timestamp lookups = **up to 151 DB reads**
**Fix**: Denormalize timestamp into `CF_ACCOUNT_TXS` value (currently empty — add 8-byte timestamp).

### N+1-3: `getRecentTransactions` — N tx lookups + N block timestamp lookups

**Handler**: rpc/src/lib.rs#L2645
**Pattern**: Identical to above but uses `get_recent_txs()` from CF_TX_BY_SLOT.
**Estimated DB calls**: limit=500 → 1 scan + 500 tx lookups + up to 500 block timestamp lookups = **up to ~600 DB reads for max-limit request**
**Fix**: Denormalize `(timestamp, tx_type)` into the CF_TX_BY_SLOT value (currently only tx_hash stored). This eliminates both the tx lookup AND block lookup for the recent transactions path.

### N+1-4: `getNFTsByOwner` + `getNFTsByCollection` — N account lookups

**Handlers**: rpc/src/lib.rs#L7979 / rpc/src/lib.rs#L8075
**Outer query**: O(limit) prefix scan of CF_NFT_BY_OWNER/CF_NFT_BY_COLLECTION → N token pubkeys (empty values)
**Inner query per token**:
```rust
state.state.get_account(&token_pubkey)  // CF_ACCOUNTS point lookup × N
decode_token_state(&account.data)       // pure Rust, no DB
```
**Estimated DB calls**: limit=50 → 51+ DB reads
**Fix**: Store a compact token summary (collection, token_id, owner, metadata_uri) as the value in `CF_NFT_BY_OWNER` and `CF_NFT_BY_COLLECTION` instead of empty bytes. Reduces to 1 scan with zero per-item DB reads.

### N+1-5: `getValidators` — N account fallback lookups

**Handler**: rpc/src/lib.rs#L4149
**Pattern**: When `state.stake_pool` is `None`, for each validator:
```rust
state.state.get_account(&v.pubkey)  // CF_ACCOUNTS × N (fallback path)
```
**Affected when**: Stake pool mutex is contended (`try_read()` fails) or pool has no data.
**Fix**: Pre-load stake pool data once outside the iterator; use blocking `read().await` via the async interface.

---

## D. CACHING AUDIT

### Existing Caches

**1. `validator_cache`** (rpc/src/lib.rs#L163)
- Type: `Arc<RwLock<(Instant, Vec<ValidatorInfo>)>>`
- TTL: 400ms (`VALIDATOR_CACHE_TTL_MS`)
- Covers: `getValidators`, `getMetrics`, indirectly `compute_metrics`
- Thread safety: ✅ Double-checked locking (read lock first, write lock only on miss)
- **Bug**: `require_single_validator` (line 187) calls `state.state.get_all_validators().unwrap_or_default()` directly, bypassing the cache entirely. Every admin call triggers an uncached CF scan.
- **Fail-open risk**: If DB errors, `unwrap_or_default()` returns empty → admin access granted.

**2. `metrics_cache`** (rpc/src/lib.rs#L164)
- Type: `Arc<RwLock<(Instant, Option<serde_json::Value>)>>`
- TTL: 400ms (`METRICS_CACHE_TTL_MS`)
- Same double-checked locking pattern — ✅ correct
- Stale window acceptable (1 slot ~400ms)

**3. `solana_tx_cache`** (rpc/src/lib.rs#L149)
- Type: `Arc<Mutex<LruCache<Hash, SolanaTxRecord>>>` — capacity 10,000
- No TTL, pure LRU eviction
- ⚠️ Uses `Mutex` (not `RwLock`): every cache read blocks all other readers. Under Solana explorer polling this could be a contention hotspot.
- **Fix**: Switch to `RwLock<LruCache>` or a concurrent map.

**4. `orderbook_cache`** (rpc/src/lib.rs#L170)
- Type: `Arc<RwLock<HashMap<u64, (Instant, serde_json::Value)>>>`
- 1-second TTL per pair
- Prevents O(total_orders) scan per DEX orderbook request — ✅ correct design

### Handlers That Should Be Cached But Are Not

| Handler | Reason Caching Needed | Recommended TTL |
|---|---|---|
| `getAllContracts` | Full CF_PROGRAMS scan + N symbol lookups; explorer polls this on contracts page | 10 seconds |
| `getAllSymbolRegistry` | Full CF_SYMBOL_REGISTRY scan; used by DEX token dropdowns | 30 seconds |
| `getPrograms` | Full CF_PROGRAMS scan; called by monitoring/admin UIs | 10 seconds |
| `getMarketListings` (unfiltered) | CF_MARKET_ACTIVITY scan; frequently polled by marketplace | 2 seconds |

---

## E. RATE LIMITING AUDIT

### Tier Assignments

```
Cheap   (5,000/s per IP): getBalance, getAccount, getBlock, getSlot, getTransaction,
                          getFeeConfig, getRentParams, getNetworkInfo, getChainStatus,
                          getValidators, getMetrics, health, getContractInfo, getNFT,
                          getCollection, getTokenBalance, confirmTransaction,
                          ALL Solana-compat except sendTransaction/getSignaturesForAddress,
                          ALL EVM-compat except eth_sendRawTransaction/eth_call/eth_estimateGas/eth_getLogs

Moderate (2,000/s per IP): getTransactionsByAddress, getTransactionHistory,
                            getRecentTransactions, getTokenHolders, getTokenTransfers,
                            getContractEvents, getContractLogs, getNFTsByOwner,
                            getNFTsByCollection, getNFTActivity, getMarketListings,
                            getMarketSales, getProgramCalls, getProgramStorage,
                            getPrograms, getAllContracts, getAllSymbolRegistry,
                            getPredictionMarkets, getPredictionLeaderboard,
                            batchReverseMoltNames, searchMoltNames, getUnstakingQueue

Expensive (500/s per IP): sendTransaction, simulateTransaction, deployContract,
                           upgradeContract, stake, unstake, stakeToReefStake,
                           unstakeFromReefStake, claimUnstakedTokens, requestAirdrop,
                           setFeeConfig, setRentParams, setContractAbi
```

### Issues Found

#### ❌ CRITICAL — DEX REST API (`/api/v1/*`) has NO tier-based rate limiting

The DEX REST router is mounted at `Router::new().nest("/api/v1", dex::build_dex_router())` (rpc/src/lib.rs#L1300). It goes through the global `rate_limit_middleware` (global: 5,000/s) but `classify_method` is never called for REST requests. Heavy endpoints like `/api/v1/candles/{pair}` (iterates entire candle history) or `/api/v1/orderbook/{pair}` are effectively in the Cheap tier at 5,000/s.
**Fix**: Apply per-route rate limit middleware directly on the DEX router, or add route-specific tier checks inside DEX handlers.

#### ❌ CRITICAL — `require_single_validator` fail-opens on DB error

Every admin call triggers `get_all_validators().unwrap_or_default()`. If this DB read fails (disk I/O error, RocksDB compaction stall), `unwrap_or_default()` returns empty → the function sees zero validators → single-validator mode → admin access granted. An attacker who can induce DB errors (via I/O exhaustion) could bypass multi-validator checks.
**Fix**: Return explicit error on DB failure. Route through cached validator list.

#### ⚠️ MEDIUM — No per-IP rate limits shared across nodes

Each RPC server process maintains its own in-process `HashMap<IpAddr, ...>`. Behind a load balancer, an attacker distributes requests across N nodes to multiply their effective rate by N.
**Fix**: Use Redis-based sliding window counters for production deployments.

#### ⚠️ MEDIUM — Solana compat endpoint under-classifies heavy methods

`handle_solana_rpc` only classifies `sendTransaction` (Expensive) and `getSignaturesForAddress` (Moderate). `getBlock` on large blocks, `getSignatureStatuses` with 100 signatures, and `getAccountInfo` for large contract accounts fall to the Cheap tier.
**Fix**: Expand Solana method tier table to cover `getBlock`, `getSignatureStatuses`, `getTransaction`.

#### ⚠️ MEDIUM — Tier check happens AFTER full body deserialization

At 10,000 req/s to `sendTransaction` from one IP: global limit (5,000/s) clears → full body deserialized → 500 cleared the Expensive tier → 499 `simulate_transaction()` calls simultaneously. 499 concurrent simulations could cause CPU exhaustion via the contract execution engine.
**Fix**: Apply tier check BEFORE body deserialization via Axum middleware that reads only the `method` field first.

---

## F. WEBSOCKET EMISSION AUDIT

### Active Events (Have Confirmed Send Calls)

| Event | Send Location | Trigger | Emission Timing |
|---|---|---|---|
| `Event::Slot(u64)` | validator/src/main.rs#L10891 | After every slot loop iteration | Post-slot processing ✅ |
| `Event::Block(Block)` | validator/src/main.rs#L11512 | After `emit_program_and_nft_events` | Post-block-finalization ✅ |
| `Event::Transaction(tx)` | validator/src/main.rs#L1600 | Per-tx in `emit_program_and_nft_events` | After block applied to state ✅ |
| `Event::AccountChange { pubkey, balance }` | validator/src/main.rs#L1608 | After system transfers (instruction type 0) | Post-state-update ✅ |
| `Event::NftMint { collection }` | validator/src/main.rs#L1624 | On MintNFT instruction (type 7) | Post-state ✅ |
| `Event::NftTransfer { collection }` | validator/src/main.rs#L1643 | On TransferNFT instruction (type 8) | Post-state ✅ |
| `Event::ProgramUpdate { program, kind }` | validator/src/main.rs#L1654-L1670 | On Deploy/Upgrade/SetABI | Post-state ✅ |
| `Event::ProgramCall { program }` | validator/src/main.rs#L1678 | On contract call instruction | Post-state ✅ |
| `Event::Log { contract, message }` | validator/src/main.rs#L1683 | On contract log output during execution | Post-execution ✅ |
| `Event::MarketListing { activity }` | validator/src/main.rs#L1727 | On NFT listing market activity | Post-state ✅ |
| `Event::MarketSale { activity }` | validator/src/main.rs#L1727 | On sale market activity | Post-state ✅ |
| `Event::BridgeLock { ... }` | validator/src/main.rs#L1776 | On bridge lock event | Post-state ✅ |
| `Event::BridgeMint { ... }` | validator/src/main.rs#L1816 | On bridge mint event | Post-state ✅ |

**Emission context is correct**: All events emitted inside `emit_program_and_nft_events()` (validator/src/main.rs#L1586), called after the block has been applied to state and written to RocksDB — never pre-broadcast.

### ❌ CRITICAL — Dead Subscriptions (Zero Sends Anywhere)

Zero `Event::SignatureStatus|ValidatorUpdate|TokenBalanceChange|EpochChange|GovernanceEvent` sends anywhere in the codebase:

| Subscription | Subscribe method | Impact |
|---|---|---|
| `signatureSubscribe` | rpc/src/ws.rs | **DEAD — clients hang forever waiting. TX confirmation UX broken.** |
| `subscribeValidators` | rpc/src/ws.rs | DEAD — validator dashboard never auto-updates |
| `subscribeTokenBalance` | rpc/src/ws.rs | DEAD — token balance notifications never fire |
| `subscribeEpochs` | rpc/src/ws.rs | DEAD — epoch change notifications never fire |
| `subscribeGovernance` | rpc/src/ws.rs | DEAD — governance event notifications never fire |

**Impact**: The SDK and developer docs advertise `signatureSubscribe` for transaction confirmation. Wallets relying on this for TX confirmation UX will never receive a notification.

**Fix for SignatureStatus**: In the block loop where `emit_program_and_nft_events` is called, after writing the block, emit `Event::SignatureStatus` for each transaction:
```rust
for tx in &block.transactions {
    let sig_hex = tx.signature().to_hex();
    let _ = ws_event_tx.send(Event::SignatureStatus {
        signature: sig_hex,
        status: "finalized".to_string(),
        slot: block.header.slot,
        err: None,
    });
}
```

---

## G. TRANSACTION PROCESSING PIPELINE

**Handler**: `handle_send_transaction` (rpc/src/lib.rs#L3244)

### Input Validation Steps (in order)

1. `params` not None — else `-32602 "Missing params"`
2. `params[0]` is a string (base64) — else `-32602`
3. Base64 decode — else `-32602 "Invalid base64"`
4. First-byte heuristic dispatch: `'{'` → JSON-first, else bincode-first
5. `parse_json_transaction` (JSON path): validates signature arrays (64-byte each), blockhash hex, instruction field types including 32-byte pubkeys
6. `bounded_bincode_deserialize` (bincode path): 4 MiB limit, `catch_unwind` around bincode 1.x panic vectors
7. EVM sentinel blockhash rejection: `recent_blockhash == Hash([0xEE;32])` → `-32003`
8. Empty `signatures` array → `-32003`
9. Zero-byte signature (`sig.iter().all(|b| *b == 0)`) → `-32003`
10. Empty `instructions` array → `-32003`
11. Pre-balance check: `payer.spendable >= compute_transaction_fee(tx, fee_config)` → `-32003`
12. For Transfer instructions: `payer.spendable >= fee + transfer_amount` → `-32003`
13. Preflight simulation (unless `skipPreflight: true`): `TxProcessor::simulate_transaction(&tx)` → `-32002 "Transaction simulation failed: {reason}"`

### Deserialization Formats Accepted

```
Binary (bincode 1.x):   [transaction_bytes] → bounded_bincode_deserialize (4 MiB limit)
JSON (wallet format):   { "signatures": [[byte,...]|"hex64"],
                          "message": {
                            "instructions": [{ "program_id": [bytes32]|"base58",
                                               "accounts": [[bytes32]|"base58",...],
                                               "data": [byte,...] },...],
                            "blockhash": "hex64"   // also "recent_blockhash" or "recentBlockhash"
                          } }
```

Both `program_id`/`programId` and `recent_blockhash`/`recentBlockhash`/`blockhash` naming conventions accepted.

### Mempool Submission

`submit_transaction` (rpc/src/lib.rs#L2859): calls `mpsc::Sender::try_send(tx)`. Channel is bounded — returns `-32003 "Transaction queue full"` if backpressure hits.

### ⚠️ Missing Limits

- **Max instruction count**: No explicit limit on `instructions` array length. A TX with 65,535 empty-data instructions would pass format validation.
- **Max instruction data size**: No per-field limit in `parse_json_transaction` for instruction data array size. A request with 1 instruction containing `data: [0,...,0]` up to the 2MB body limit would be accepted.

---

## H. ENDPOINT SECURITY AUDIT

### Admin Endpoint Guard Pattern

All 6 state-mutating admin endpoints follow this pattern:
```rust
require_single_validator(state, "endpoint")?;   // line 1: blocks multi-validator mode
verify_admin_auth(state, &params)?;              // line 2: constant-time token check
// ... DB work only after both guards pass
```
**Auth check position**: ✅ Correct — both guards are the first two operations.

**Timing attack protection**: `constant_time_eq` (rpc/src/lib.rs#L270) folds XOR over all bytes before returning. ✅ Safe from byte-by-byte timing attacks.

**Length check**: Checked first (early return if lengths differ), which leaks whether token length matches — acceptable since token length is constant at rotation time.

### Specific Security Issues

#### ❌ CRITICAL — Fail-open admin access on DB error

As noted: `require_single_validator` calls `get_all_validators().unwrap_or_default()`. DB error → empty list → multi-validator check bypassed → admin access granted without valid token in a multi-validator setup.

#### ⚠️ MEDIUM — `airdrop_cooldowns` HashMap grows unboundedly

Uses per-address `HashMap<String, Instant>` with cooldown check. The cooldown map grows without bound until node restart. Under sustained unique-address airdrop flooding, memory consumption grows indefinitely.
**Fix**: Use a bounded LRU map (limit to ~100,000 entries).

#### ⚠️ MEDIUM — `airdrop_cooldowns` uses `std::sync::Mutex` in async handler

Synchronous Mutex inside an async handler (`lock()` in async context) blocks the Tokio thread.
**Fix**: Use `tokio::sync::Mutex` or restructure to avoid holding the lock across await points.

### User-Input Sanitization

- **RocksDB key injection**: Not applicable — all key construction uses binary fixed-length components (pubkeys, slot numbers, hash bytes). No user string is interpolated directly into a key. ✅
- **Max payload size**: HTTP body limit: 2 MB (`DefaultBodyLimit`). ✅
- **Pubkey validation**: `Pubkey::from_base58()` validates correct length (32 bytes) and base58 encoding. ✅
- **No SQL/NoSQL injection vectors**: Binary keys, no string interpolation in DB paths. ✅

---

## I. ERROR HANDLING PATTERNS

### JSON-RPC Error Code Usage

| Code | Meaning | Status |
|---|---|---|
| `-32601` | Method not found | ✅ Returned for unknown methods |
| `-32602` | Invalid params | ✅ Consistent across handlers |
| `-32600` | Invalid request | ❌ Never used (malformed JSON → Axum 400 before handler) |
| `-32003` | Authorization / validation error | ✅ Used for bad auth, blockhash, balance checks |
| `-32002` | Preflight simulation failed | ✅ Specific to sendTransaction |
| `-32001` | Not found | ✅ Account/TX/block not found |
| `-32000` | Internal/DB error | ✅ Returns with raw DB error string |
| `-32005` | Rate limit exceeded | ✅ Used consistently |

### Panic Paths

No bare `.unwrap()` calls in request handler paths. Notable safe uses:
- `NonZeroUsize::new(10_000).unwrap()` (rpc/src/lib.rs#L1177) — infallible constant in startup
- `to_le_bytes().try_into().unwrap()` patterns in state.rs — on data known to be exact size

`bounded_bincode_deserialize` wraps bincode in `std::panic::catch_unwind`, explicitly protecting against bincode 1.x adversarial panics. ✅

### ❌ HIGH — Raw RocksDB Error String Exposure

**84 raw RocksDB error strings** are returned to RPC clients via `format!("Database error: {}", e)`. Examples of real-world RocksDB errors that would be exposed:
- `"IO error: /var/moltchain/db/OPTIONS-000003: too many open files"`
- `"Corruption: block checksum mismatch: address column family accounts"`
- `"Resource busy: lock file /var/moltchain/db/LOCK: Resource temporarily unavailable"`

These leak:
- Absolute filesystem paths of the database
- Column family names confirming internal architecture
- Internal key formats in some error variants

**Fix**: Map all DB errors to a generic `-32000` response with an opaque correlation ID; log the full error server-side keyed to the correlation ID.

---

## J. MISSING / MISMATCHED ENDPOINTS

### Confirmed Present — All Frontend Methods Wired

All RPC methods called by the explorer, wallet, DEX, and marketplace frontends are wired in `handle_rpc`:
- `getBalance`, `getAccount`, `getBlock`, `getLatestBlock`, `getSlot` ✅
- `getTransaction`, `getTransactionsByAddress`, `getAccountTxCount` ✅
- `getValidators`, `getMetrics`, `getTotalBurned`, `health` ✅
- `getContractInfo`, `getContractLogs`, `getContractAbi`, `getAllContracts` ✅
- `getNFTsByOwner`, `getNFTsByCollection`, `getNFTActivity` ✅
- `getCollection`, `getNFT`, `getMarketListings`, `getMarketSales` ✅
- `getTokenBalance`, `getTokenHolders`, `getTokenTransfers` ✅

### Missing / Mismatched Endpoints

1. **`checkNullifier` vs `isNullifierSpent` (shielded pool)**: Wallet calls `checkNullifier` (rpc/src/lib.rs dispatch table), but the actual implementation uses `isNullifierSpent` naming. Note has mismatch — nullifiers never confirmed spent client-side.

3. **`getShieldedPoolStats` vs `getShieldedPoolState` (shielded pool)**: Wallet calls `getShieldedPoolStats`; server exposes `getShieldedPoolState`. Pool stats panel always returns empty.

4. **`getBlock` param ambiguity**: Explorer calls `getBlock(slot)` with a slot number AND sometimes `getBlock(blockHash)`. Verify `handle_get_block` correctly dispatches on parameter type (u64 vs hex string).

5. **`getTransactionsByAddress` pagination cursor mismatch**: Native endpoint uses `before_slot` cursor; Solana-compat `getSignaturesForAddress` uses `before` (base58 signature). Incompatible cursors across namespaces.

6. **`getContractEvents` vs `getContractLogs`**: Both exist with overlapping behavior — documentation ambiguity may cause frontend confusion. `getContractEvents` maps to `get_events_by_program`; `getContractLogs` maps to a different query path.

---

## K. SUMMARY PRIORITY MATRIX

| # | Severity | Category | Finding |
|---|---|---|---|
| 1 | 🔴 CRITICAL | Rate Limiting | DEX REST API (`/api/v1/*`) has no tier rate limiting — scans at 5,000/s |
| 2 | 🔴 CRITICAL | Security | `require_single_validator` fail-opens on DB error — admin access bypassed |
| 3 | 🔴 CRITICAL | WebSocket | 5 dead subscriptions — `signatureSubscribe` never fires; TX confirmation UX broken |
| 4 | 🔴 CRITICAL | N+1 | `getAllContracts` = 1 full scan + N symbol lookups (up to 1001 DB calls) |
| 5 | 🟠 HIGH | Full Scan | `require_single_validator` uncached CF_VALIDATORS scan on every admin call |
| 6 | 🟠 HIGH | Full Scan | `getAllContracts` + `getPrograms` always scans CF_PROGRAMS from start (no cursor) |
| 7 | 🟠 HIGH | Full Scan | `getAllSymbolRegistry` full CF_SYMBOL_REGISTRY scan with no cursor |
| 8 | 🟠 HIGH | N+1 | `getNFTsByOwner` / `getNFTsByCollection` = O(limit) account lookups |
| 9 | 🟠 HIGH | N+1 | `getTransactionsByAddress` / `getRecentTransactions` = up to 600 DB reads per call |
| 10 | 🟠 HIGH | Security | 84 handlers return raw RocksDB error strings (path/CF name leakage) |
| 11 | 🟠 HIGH | Endpoint | `checkNullifier` vs `isNullifierSpent` — shielded nullifiers never confirmed |
| 12 | 🟠 HIGH | Endpoint | `getShieldedPoolStats` vs `getShieldedPoolState` — pool stats always empty |
| 13 | 🟡 MEDIUM | Caching | `getAllContracts`, `getAllSymbolRegistry`, `getPrograms`, `getMarketListings` uncached |
| 14 | 🟡 MEDIUM | Full Scan | `getMarketListings` with filter fetches up to 2000 rows for in-memory filtering |
| 15 | 🟡 MEDIUM | Rate Limiting | Solana-compat under-classifies `getBlock`, `getSignatureStatuses` as Cheap (5k/s) |
| 16 | 🟡 MEDIUM | Caching | `solana_tx_cache` uses `Mutex` instead of `RwLock` — reader contention under polling |
| 17 | 🟡 MEDIUM | Security | `airdrop_cooldowns` HashMap unbounded growth + sync Mutex in async handler |
| 18 | 🟡 MEDIUM | Full Scan | `export_cf_page` uses count-offset = O(offset+limit) instead of cursor-seek |
| 19 | 🟡 MEDIUM | Rate Limiting | Tier check after full body deserialization — 499 simulations per burst possible |
| 20 | 🟢 LOW | Validation | No instruction count/data-length limit in `parse_json_transaction` |
| 21 | 🟢 LOW | Rate Limiting | Per-IP limits not shared across RPC processes (load balancer bypass) |
| 22 | 🟢 LOW | WebSocket | WS broadcast channel capacity 1000 — fast validators emitting many TXs can lag |
| 23 | 🟢 LOW | Endpoint | `getBlock` param ambiguity — slot number vs. block hash needs explicit dispatch |
