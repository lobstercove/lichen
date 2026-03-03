# MoltChain Comprehensive Research Audit

**Date:** 2025-02-18  
**Scope:** 10-category audit — REST API routes, test failures, RPC endpoints, contract storage consistency, genesis initialization, opcode dispatch, RPC stats handlers, and error handling.  
**Mode:** RESEARCH ONLY — no code changes made.

---

## Table of Contents

1. [REST API Routes → E2E SKIP Failures](#1-rest-api-routes--e2e-skip-failures)
2. [contracts-write-e2e.py → 107 FAILs](#2-contracts-write-e2epy--107-fails)
3. [e2e-dex-trading.py Failures](#3-e2e-dex-tradingpy-failures)
4. [e2e-websocket-upgrade.py Failures](#4-e2e-websocket-upgradepy-failures)
5. [RPC Endpoint Audit](#5-rpc-endpoint-audit)
6. [Contract Storage Consistency (CRITICAL)](#6-contract-storage-consistency-critical)
7. [Genesis Contract Initialization](#7-genesis-contract-initialization)
8. [Opcode Dispatch Length Checks](#8-opcode-dispatch-length-checks)
9. [RPC Stats Handlers](#9-rpc-stats-handlers)
10. [Missing Error Handling (unwrap)](#10-missing-error-handling-unwrap)

---

## 1. REST API Routes → E2E SKIP Failures

**Severity:** MEDIUM  
**Impact:** 8 test cases SKIP'd in `tests/comprehensive-e2e.py`

### Root Cause

The REST router in `rpc/src/dex.rs` registers stats endpoints at these paths:

```
/stats/core
/stats/amm
/stats/margin
/stats/router
/stats/rewards
/stats/analytics
/stats/governance
/stats/moltswap
```

(See `build_dex_router()` at `rpc/src/dex.rs` lines 2617–2624.)

This router is nested into the main Axum app at `/api/v1`:

```rust
// rpc/src/lib.rs line ~1179
.nest("/api/v1", dex::build_dex_router())
```

So the actual endpoint URLs are: `/api/v1/stats/core`, `/api/v1/stats/amm`, etc.

However, the E2E test at `tests/comprehensive-e2e.py` lines 1081–1084 constructs URLs with an **extra `/dex/` segment**:

```python
f"{base_url}/api/v1/dex/stats/core"
f"{base_url}/api/v1/dex/stats/amm"
# ... etc.
```

These resolve to `/api/v1/dex/stats/core` which does not exist → 404 → SKIP.

### Suggested Fix

**Option A (test fix):** Remove the `/dex/` segment from all 8 test URLs. Change `/api/v1/dex/stats/*` to `/api/v1/stats/*`.

**Option B (router fix):** Nest the DEX router at `/api/v1/dex` instead of `/api/v1`. This would break all other endpoints currently at `/api/v1/*` unless they're also migrated.

Option A is the correct fix — the test URLs are wrong, not the server routes.

---

## 2. contracts-write-e2e.py → 107 FAILs

**Severity:** LOW (test architecture issue, not a code bug)  
**Impact:** 107 test cases FAIL in `tests/contracts-write-e2e.py`

### Root Cause

`contracts-write-e2e.py` uses a **named-export JSON calling convention** for ALL contracts. At line 210, `call_contract()` wraps every call like:

```python
{"Call": {"function": func_name, "args": list(args_bytes), "value": 0}}
```

This works for **named-export contracts** (e.g., `moltcoin`, `musd_token`, `wrapped_btc`, `wrapped_eth`) which expose individual exported functions like `mint()`, `transfer()`, `burn()`, etc.

However, 8 contracts use **opcode dispatch** — they export only a single `call()` entry point that reads an opcode byte from `args[0]` and dispatches internally:

- `dex_core` (opcodes 0–30)
- `dex_amm` (opcodes 0–19)
- `dex_router` (opcodes 0–13)
- `dex_governance` (opcodes 0–19)
- `dex_margin` (opcodes 0–26)
- `dex_rewards` (opcodes 0–19)
- `dex_analytics` (opcodes 0–12)
- `prediction_market` (opcodes 0–37)

When the test sends `{"Call": {"function": "create_pair", "args": [...]}}` to `dex_core`, the WASM runtime looks for a `create_pair` export, doesn't find one (only `call` is exported), and fails.

The `e2e-dex-trading.py` test handles this correctly by building raw binary instructions with opcode prefixes via `build_dispatcher_ix()`.

### Suggested Fix

Either:
1. Add a `build_dispatcher_ix()` path for opcode-dispatch contracts in `contracts-write-e2e.py`, OR
2. Remove opcode-dispatch contracts from this test file's scope (since they're already covered by `e2e-dex-trading.py`).

---

## 3. e2e-dex-trading.py Failures

**Severity:** LOW (test setup issue)  
**Impact:** Multiple FAILs with "Payer account does not exist"

### Root Cause

`e2e-dex-trading.py` creates secondary trader keypairs (`trader_a`, `trader_b`) and attempts to fund them via `fund_account()` (line 300). This function first tries a faucet airdrop, then falls back to a deployer-to-trader transfer.

The failures occur because **neither funding method succeeds** — the airdrop fails (faucet may not be running in the test environment) and the transfer also fails (deployer may not have sufficient balance or the transfer RPC method may not be available for native token outside of the `moltcoin` contract).

When the test proceeds to submit contract call transactions, the `trader_a`/`trader_b` accounts don't exist on-chain, causing "Payer account does not exist" from the transaction processor.

### Suggested Fix

1. Ensure the faucet is running during E2E tests, OR
2. Have the test use the `moltcoin` contract's `mint()` function to create funded accounts via the deployer's authority, OR
3. Add a retry/validation loop in `fund_account()` that confirms the account exists before proceeding.

---

## 4. e2e-websocket-upgrade.py Failures

**Severity:** LOW (tests expected behavior)  
**Impact:** 4 FAILs in REST POST tests

### Root Cause

| Endpoint | Status | Explanation |
|---|---|---|
| `POST /api/v1/orders` | 405 | `post_order()` at `dex.rs` line 1581 returns `api_method_not_allowed()` — **by design**. Orders must be submitted via `sendTransaction`. |
| `POST /api/v1/margin/open` | 405 | `post_margin_open()` at `dex.rs` line 2177 returns `api_method_not_allowed()` — **by design**. Margin positions must be opened via `sendTransaction`. |
| `POST /api/v1/prediction-market/create` | 400 | `post_create()` at `prediction.rs` line 682 requires an `admin_token` header matching the config. The test either omits this header or sends an incorrect token. |
| `POST /api/v1/prediction-market/trade` | 404 | No market exists (depends on the create step above succeeding). |

The 405 responses are **intentional REST stubs** — these endpoints exist to return a helpful error message directing users to use `sendTransaction` instead. They are not bugs.

### Suggested Fix

Update the test expectations:
- For `POST /orders` and `POST /margin/open`: expect 405 (these are intentional stubs).
- For `POST /prediction-market/create`: send the correct `admin_token` header from the config.

---

## 5. RPC Endpoint Audit

**Severity:** INFO  
**Impact:** Documentation of all registered RPC methods

The full RPC dispatch table is at `rpc/src/lib.rs` lines 1216–1371. All methods are registered as JSON-RPC handlers. Here is the complete catalog:

### Balance & Account (11 methods)
| Method | Line |
|---|---|
| `getBalance` | 1218 |
| `getAccountInfo` | 1219 |
| `getMultipleAccounts` | 1220 |
| `getTokenAccountsByOwner` | 1221 |
| `getLargestAccounts` | 1222 |
| `getTokenBalance` | 1223 |
| `getRecentBlockhash` | 1224 |
| `getMinimumBalanceForRentExemption` | 1225 |
| `getInflationRate` | 1226 |
| `getStakeMinimumDelegation` | 1228 |
| `getAccountHistory` | 1229 |

### Transaction (9 methods)
| Method | Line |
|---|---|
| `sendTransaction` | 1232 |
| `simulateTransaction` | 1233 |
| `getTransaction` | 1234 |
| `getSignatureStatuses` | 1235 |
| `getRecentPerformanceSamples` | 1236 |
| `requestAirdrop` | 1237 |
| `getConfirmedTransaction` | 1238 |
| `getSignaturesForAddress` | 1239 |
| `getTransactionCount` | 1240 |

### Block & Slot (11 methods)
| Method | Line |
|---|---|
| `getBlockHeight` | 1243 |
| `getSlot` | 1244 |
| `getBlock` | 1245 |
| `getBlockTime` | 1246 |
| `getEpochInfo` | 1247 |
| `getEpochSchedule` | 1248 |
| `getLeaderSchedule` | 1249 |
| `getBlockProduction` | 1250 |
| `getVersion` | 1251 |
| `getHealth` | 1252 |
| `getGenesisHash` | 1253 |

### Staking (5 methods)
| Method | Line |
|---|---|
| `getStakeActivation` | 1256 |
| `getVoteAccounts` | 1257 |
| `getStakingStats` | 1258 |
| `getStakeAccount` | 1259 |
| `getValidators` | 1260 |

### Contracts (5 methods)
| Method | Line |
|---|---|
| `getContractStorage` | 1263 |
| `getContractInfo` | 1264 |
| `getContractEvents` | 1265 |
| `getDeployedContracts` | 1266 |
| `callContract` | 1267 |

### Programs (3 methods)
| Method | Line |
|---|---|
| `getProgramAccounts` | 1270 |
| `getStorageValue` | 1271 |
| `getProgramStorage` | 1272 |

### MoltyID (5 methods)
| Method | Line |
|---|---|
| `getMoltyIdProfile` | 1275 |
| `getReputationHistory` | 1276 |
| `getMoltyIdStats` | 1277 |
| `searchMoltyId` | 1278 |
| `getMoltyIdActivity` | 1279 |

### NFT & Marketplace (6 methods)
| Method | Line |
|---|---|
| `getNFTMetadata` | 1282 |
| `getNFTsByOwner` | 1283 |
| `getNFTCollections` | 1284 |
| `getMarketplaceStats` | 1285 |
| `getMarketplaceListings` | 1286 |
| `getMarketplaceActivity` | 1287 |

### DEX Stats (8 methods)
| Method | Line |
|---|---|
| `getDexCoreStats` | 1290 |
| `getDexAmmStats` | 1291 |
| `getDexRouterStats` | 1292 |
| `getDexGovernanceStats` | 1293 |
| `getDexMarginStats` | 1294 |
| `getDexRewardsStats` | 1295 |
| `getDexAnalyticsStats` | 1296 |
| `getMoltSwapStats` | 1297 |

### DEX Trading (11 methods)
| Method | Line |
|---|---|
| `getDexPairs` | 1300 |
| `getDexOrderbook` | 1301 |
| `getDexTrades` | 1302 |
| `getDexUserOrders` | 1303 |
| `getDexCandles` | 1304 |
| `getDexUserTrades` | 1305 |
| `getDexOrders` | 1306 |
| `getDexPairInfo` | 1307 |
| `getDexMarginPositions` | 1308 |
| `getDexMarginPosition` | 1309 |
| `getDexUserMarginPositions` | 1310 |

### Prediction Market (6 methods)
| Method | Line |
|---|---|
| `getPredictionMarkets` | 1313 |
| `getPredictionMarket` | 1314 |
| `getPredictionUserPositions` | 1315 |
| `getPredictionMarketStats` | 1316 |
| `getPredictionMarketTrades` | 1317 |
| `getPredictionMarketResolution` | 1318 |

### Token & Supply (8 methods)
| Method | Line |
|---|---|
| `getTokenStats` | 1321 |
| `getTokenList` | 1322 |
| `getTokenMetadata` | 1323 |
| `getTokenHolders` | 1324 |
| `getTokenTransfers` | 1325 |
| `getWrappedTokenStats` | 1326 |
| `getCrossChainStats` | 1327 |
| `getCirculatingSupply` | 1328 |

### Oracle (2 methods)
| Method | Line |
|---|---|
| `getMoltOracleStats` | 1331 |
| `getMoltOraclePrice` | 1332 |

### Subscription (2 methods)
| Method | Line |
|---|---|
| `subscribe` | 1335 |
| `unsubscribe` | 1336 |

### Launchpad & Agent (13 methods)
| Method | Line |
|---|---|
| `getLaunchpadStats` | 1339 |
| `getLaunchpadProjects` | 1340 |
| `getLaunchpadProject` | 1341 |
| `getLaunchpadContributions` | 1342 |
| `getAgentRegistry` | 1345 |
| `getAgentInfo` | 1346 |
| `getAgentEarnings` | 1347 |
| `getAgentTasks` | 1348 |
| `getAgentReputation` | 1349 |
| `getAgentStats` | 1350 |
| `getAgentLeaderboard` | 1351 |
| `getAgentCategories` | 1352 |
| `getAgentReviews` | 1353 |

### System (6 methods)
| Method | Line |
|---|---|
| `getSupplyInfo` | 1356 |
| `getTotalSupply` | 1357 |
| `getCluster` | 1358 |
| `getClusterNodes` | 1359 |
| `getIdentity` | 1360 |
| `getFirstAvailableBlock` | 1361 |

### Solana Compatibility (5 methods)
| Method | Line |
|---|---|
| `getFeeForMessage` | 1364 |
| `isBlockhashValid` | 1365 |
| `getLatestBlockhash` | 1366 |
| `getMaxRetransmitSlot` | 1367 |
| `getMaxShredInsertSlot` | 1368 |

**Total: ~116 JSON-RPC methods + REST endpoints**

---

## 6. Contract Storage Consistency (CRITICAL)

**Severity:** CRITICAL  
**Impact:** REST stats and RPC stats can return **different values** for the same contract data.

### Architecture

MoltChain maintains **two independent storage paths** for contract data:

| Path | Column Family / Location | Used By |
|---|---|---|
| **CF_CONTRACT_STORAGE** | Dedicated RocksDB column family. Key = `{program_pubkey}:{storage_key}` | REST handlers in `dex.rs` via `read_u64()` / `read_bytes()` → `state.get_program_storage()` |
| **Embedded ContractAccount.storage** | Serialized JSON inside `Account.data` → `ContractAccount { storage: HashMap<Vec<u8>, Vec<u8>>, ... }` | RPC stats handlers in `lib.rs` via `load_contract_by_symbol()` → `get_account()` → `serde_json::from_slice()` → `stats_u64()` |

### Where They Stay In Sync

**Normal contract execution** (via `sendTransaction`) keeps both in sync. In `core/src/processor.rs` lines 2830–2853:

```rust
for (key, value_opt) in &result.storage_changes {
    Some(val) => {
        contract.set_storage(key.clone(), val.clone());           // ← embedded
        self.b_put_contract_storage(contract_address, key, val)?; // ← CF
    }
}
// Then re-serializes ContractAccount and calls b_put_account()     // ← persists embedded
```

**Genesis initialization** also keeps both in sync. In `validator/src/main.rs` lines 2997–3000 (`genesis_exec_contract`):

```rust
contract.set_storage(key.clone(), val.clone());
state.put_contract_storage(program_pubkey, key, val)?;
// Then re-serializes and calls put_account()
```

### Where They DIVERGE (The Bug)

The **validator's post-block hooks** write ONLY to CF_CONTRACT_STORAGE and do NOT update the embedded ContractAccount.storage:

1. **`reset_24h_candles_if_needed()`** — `validator/src/main.rs` lines 770–820  
   Writes `ana_24h_*` and `ana_24h_ts_*` keys → CF_CONTRACT_STORAGE only.

2. **`run_sltp_trigger_engine()`** — `validator/src/main.rs` lines 830–960  
   Writes `dex_order_*`, `dex_bid_*`, `dex_ask_*`, `dex_best_bid_*`, `dex_best_ask_*` → CF_CONTRACT_STORAGE only.  
   Writes `margin_pos_*`, `mrg_insurance`, `mrg_pnl_profit`, `mrg_pnl_loss`, `balance_*` → CF_CONTRACT_STORAGE only.

3. **`bridge_dex_trades_to_analytics()`** — `validator/src/main.rs` lines 1170–1310  
   Writes `ana_lp_*`, `ana_last_trade_ts_*`, `ana_24h_*`, candle keys → CF_CONTRACT_STORAGE only.  
   Writes `ana_rec_count`, `ana_total_volume`, `ana_trader_count` → CF_CONTRACT_STORAGE only.

**None of these functions** load the ContractAccount, update its embedded storage HashMap, re-serialize it, and call `put_account()`.

### Consequence

After any post-block hook runs:

| Query Type | Source | Data |
|---|---|---|
| `GET /api/v1/stats/analytics` (REST) | CF_CONTRACT_STORAGE | **Fresh** — includes bridge-written data |
| `getDexAnalyticsStats` (RPC) | Embedded ContractAccount.storage | **Stale** — only has data from genesis or last `sendTransaction` |

This means:
- The REST endpoint at `/api/v1/stats/analytics` returns up-to-date 24h volume, trade counts, and candle data.
- The RPC method `getDexAnalyticsStats` returns genesis-era values for the same fields.
- Similarly, after SL/TP triggers fire, `getDexMarginStats`'s `mrg_insurance` / `mrg_pnl_*` values via RPC will be stale compared to REST.

### Suggested Fix

For each post-block hook function, after all `put_contract_storage()` writes, add:

```rust
// Load ContractAccount, apply all changed keys to embedded storage, re-serialize, put_account()
let mut account = state.get_account(&program_pk).unwrap().unwrap();
let mut contract: ContractAccount = serde_json::from_slice(&account.data).unwrap();
for (key, val) in &changed_keys {
    contract.set_storage(key.clone(), val.clone());
}
account.data = serde_json::to_vec(&contract).unwrap();
state.put_account(&program_pk, &account).unwrap();
```

Alternatively, refactor the RPC stats handlers to read from CF_CONTRACT_STORAGE (the same path REST uses) — this would be a simpler fix and would also make RPC stats faster (avoids deserializing the entire ContractAccount).

---

## 7. Genesis Contract Initialization

**Severity:** LOW  
**Impact:** Documentation audit

### Genesis Contracts (25 calls)

The validator initializes contracts at genesis via `genesis_exec_contract()` in `validator/src/main.rs`. The function is defined at line 2937.

Key contracts initialized:
- **Token contracts:** `moltcoin`, `musd_token`, `wrapped_btc`, `wrapped_eth`
- **DEX contracts:** `dex_core`, `dex_amm`, `dex_router`, `dex_governance`, `dex_margin`, `dex_rewards`, `dex_analytics`
- **Other:** `prediction_market`, `moltyid`, `nft_core`, `marketplace`, `launchpad`, `agent_registry`, `moltswap`, `crosschain`, `molt_oracle`

### Symbol Registry

13 symbols are registered with `register_symbol().unwrap()` at lines ~11017–11158:
- `MOLTCOIN`, `MUSD`, `WBTC`, `WETH`
- `DEX`, `AMM`, `ROUTER`, `GOVERNANCE`, `MARGIN`, `REWARDS`, `ANALYTICS`
- `PREDICTION`, `MOLTYID`

### Observation

The 13 `unwrap()` calls on `register_symbol()` will panic if symbol registration fails during genesis. This is acceptable for genesis (the chain can't start if symbol registration fails), but the error messages would be opaque. Consider using `.expect("Failed to register MOLTCOIN symbol")` for better diagnostics.

---

## 8. Opcode Dispatch Length Checks

**Severity:** PASS  
**Impact:** All 8 opcode-dispatch contracts have correct argument length validation.

Each contract's `call()` function reads `args[0]` as the opcode and validates `args.len() >= N` before parsing arguments. I audited every opcode in all 8 contracts:

| Contract | File | Opcodes | Status |
|---|---|---|---|
| `dex_core` | `contracts/dex_core/src/lib.rs:2098` | 0–30 | ✅ All length checks correct |
| `dex_amm` | `contracts/dex_amm/src/lib.rs:1039` | 0–19 | ✅ All length checks correct |
| `dex_router` | `contracts/dex_router/src/lib.rs:625` | 0–13 | ✅ All length checks correct |
| `dex_governance` | `contracts/dex_governance/src/lib.rs:993` | 0–19 | ✅ All length checks correct |
| `dex_margin` | `contracts/dex_margin/src/lib.rs:1368` | 0–26 | ✅ All length checks correct |
| `dex_rewards` | `contracts/dex_rewards/src/lib.rs:613` | 0–19 | ✅ All length checks correct |
| `dex_analytics` | `contracts/dex_analytics/src/lib.rs:804` | 0–12 | ✅ All length checks correct |
| `prediction_market` | `contracts/prediction_market/src/lib.rs:3389` | 0–37 | ✅ All length checks correct |

### Methodology

For each opcode, I verified that the `args.len() >= N` check accounts for:
- 1 byte for the opcode itself
- 32 bytes per pubkey argument
- 8 bytes per u64 argument
- Variable lengths for strings (prefixed with 8-byte length + data)
- 1 byte per boolean/flag

No mismatches were found.

---

## 9. RPC Stats Handlers

**Severity:** HIGH (related to Category 6)  
**Impact:** RPC stats return stale data for fields updated by post-block hooks.

### Handler Architecture

All 8 DEX stats RPC methods and 2 oracle/analytics methods follow the same pattern. They are defined at `rpc/src/lib.rs` lines 10277–10590.

```rust
fn handle_get_dex_core_stats(state: &StateStore) -> Value {
    let contract = match load_contract_by_symbol(state, "DEX") {
        Some(c) => c,
        None => return json!({"error": "DEX contract not found"}),
    };
    json!({
        "pair_count": stats_u64(&contract, b"pair_count"),
        "order_count": stats_u64(&contract, b"dex_order_count"),
        // ...
    })
}
```

Where `load_contract_by_symbol()` (line 10234) does:
```rust
fn load_contract_by_symbol(state: &StateStore, symbol: &str) -> Option<ContractAccount> {
    let entry = state.get_symbol_registry(symbol).ok()??;
    let account = state.get_account(&entry.program).ok()??;
    serde_json::from_slice(&account.data).ok()
}
```

And `stats_u64()` (line 10263) reads from the embedded HashMap:
```rust
fn stats_u64(contract: &ContractAccount, key: &[u8]) -> u64 {
    contract.storage.get(key.to_vec())
        .map(|v| if v.len() >= 8 { u64::from_le_bytes(...) } else { 0 })
        .unwrap_or(0)
}
```

### Contrast with REST Handlers

The REST handlers in `dex.rs` use a completely different path:
```rust
fn read_u64(state: &StateStore, key: &[u8]) -> u64  // line 484
fn read_bytes(state: &StateStore, key: &[u8]) -> Option<Vec<u8>>  // line 479
```

These call `state.get_program_storage()` which reads from **CF_CONTRACT_STORAGE** — the same column family that post-block hooks write to.

### Specific Functions Affected

| RPC Method | Handler Location | Stale Fields After Post-Block Hooks |
|---|---|---|
| `getDexCoreStats` | lib.rs:10277 | `dex_order_*`, `dex_best_bid_*`, `dex_best_ask_*` (after SL/TP triggers) |
| `getDexMarginStats` | lib.rs:10365 | `mrg_insurance`, `mrg_pnl_profit`, `mrg_pnl_loss` (after margin SL/TP close) |
| `getDexAnalyticsStats` | lib.rs:10443 | `ana_rec_count`, `ana_total_volume`, `ana_trader_count`, `ana_24h_*`, `ana_lp_*` (after analytics bridge) |
| `getMoltOracleStats` | lib.rs:10557 | Oracle prices derived from analytics data |

### Suggested Fix

Same as Category 6. Either:
1. Update post-block hooks to also write to embedded ContractAccount.storage, OR
2. Rewrite RPC stats handlers to read from CF_CONTRACT_STORAGE via `get_program_storage()` instead of embedded storage.

Option 2 is recommended — it's less code, faster (no full ContractAccount deserialization), and eliminates the dual-storage problem entirely by making both REST and RPC read from the same source.

---

## 10. Missing Error Handling (unwrap)

**Severity:** MEDIUM  
**Impact:** Potential panics in RPC server and validator

### Validator — `validator/src/main.rs`

| Lines | Code | Risk |
|---|---|---|
| ~11017–11158 (13 sites) | `register_symbol(...).unwrap()` | LOW — genesis only. If this fails, the chain can't start anyway. Consider `.expect("msg")` for better error messages. |

### RPC Server — `rpc/src/lib.rs`

| Line | Code | Risk |
|---|---|---|
| ~1040 | `NonZeroUsize::new(1000).unwrap()` | NONE — `1000` is a compile-time constant, always non-zero. |
| ~5233 | `obj.as_object_mut().unwrap()` | LOW — panics if `serde_json::to_value(contract)` produces a non-object. This would only happen if `ContractAccount` serializes to a non-object type, which won't happen with its struct definition. |
| ~5239 | `obj.as_object_mut().unwrap()` | Same as above. |
| ~5246 | `obj.as_object_mut().unwrap()` | Same as above. |

### Post-Block Hooks — `validator/src/main.rs`

| Lines | Code | Risk |
|---|---|---|
| 774 | `d[0..8].try_into().unwrap_or([0; 8])` | NONE — uses `unwrap_or`, handles failure. |
| Multiple | `let _ = state.put_contract_storage(...)` | MEDIUM — errors are silently discarded. If RocksDB write fails (disk full, corruption), the validator silently loses data. Consider at least logging the error. |

### Observation

The most concerning pattern is not `unwrap()` but `let _ =` on `put_contract_storage()` results in post-block hooks. There are **20+ sites** in `validator/src/main.rs` (lines 780–1300) where storage write errors are discarded. A RocksDB write failure would cause silent data loss without any log entry.

---

## Summary of Findings

| # | Category | Severity | Root Cause |
|---|---|---|---|
| 1 | REST API Routes → 8 SKIPs | MEDIUM | Test URLs include extra `/dex/` segment |
| 2 | contracts-write-e2e.py → 107 FAILs | LOW | Test uses named-export calling convention for opcode-dispatch contracts |
| 3 | e2e-dex-trading.py FAILs | LOW | Trader keypairs not funded before contract calls |
| 4 | e2e-websocket-upgrade.py FAILs | LOW | POST stubs return 405 by design; prediction create needs admin_token |
| 5 | RPC Endpoints | INFO | 116 methods cataloged, all registered |
| 6 | **Contract Storage Consistency** | **CRITICAL** | Post-block hooks write to CF_CONTRACT_STORAGE only, not embedded ContractAccount.storage. REST and RPC return different data. |
| 7 | Genesis Initialization | LOW | 25 contracts initialized; 13 `unwrap()` on symbol registration (acceptable for genesis) |
| 8 | Opcode Dispatch Length Checks | PASS | All 8 contracts verified correct |
| 9 | RPC Stats Handlers | HIGH | Read from embedded storage (stale after post-block hooks); should read from CF_CONTRACT_STORAGE |
| 10 | Missing Error Handling | MEDIUM | 20+ silent `let _ =` on storage writes in post-block hooks; 3 `unwrap()` on JSON mutation in RPC |

### Priority Ranking

1. **CRITICAL** — Category 6/9: Dual-storage divergence. Fix post-block hooks or migrate RPC stats to CF_CONTRACT_STORAGE reads.
2. **MEDIUM** — Category 1: Fix test URLs (trivial one-line fix per endpoint).
3. **MEDIUM** — Category 10: Add error logging for `put_contract_storage()` failures in post-block hooks.
4. **LOW** — Categories 2, 3, 4: Test infrastructure fixes (not code bugs).
