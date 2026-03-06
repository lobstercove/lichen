# MoltChain Token & Metadata Naming Consistency Audit

**Date:** 2026-03-05  
**Status:** RESOLVED — all critical/high/medium fixes implemented  
**Scope:** Contracts, SDK, Core (state/processor), RPC, Explorer JS, Faucet, Custody, Genesis deploy

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Architecture Layers](#2-architecture-layers)
3. [Inconsistency Inventory](#3-inconsistency-inventory)
4. [Layer-by-Layer Findings](#4-layer-by-layer-findings)
5. [Unified Naming Standard](#5-unified-naming-standard)
6. [Action Items](#6-action-items)

---

## 1. Executive Summary

The MoltChain codebase has **two token standards** with **incompatible storage key formats**:

| Standard | Used By | Balance Key | Supply Key | Allowance Key |
|----------|---------|------------|------------|---------------|
| **Wrapped** (native) | wBNB, wETH, wSOL, mUSD | `{prefix}_bal_{hex64}` | `{prefix}_supply` | `{prefix}_alw_{hex64}_{hex64}` |
| **SDK Token** | MoltCoin (MOLT) | `balance:{raw32bytes}` | `total_supply` | `allowance:{raw32}:{raw32}` |

This causes **three critical runtime failures**:

1. **Token balance indexing broken for SDK tokens** — `maybe_index_token_balance()` searches for `_bal_` substring, never matches SDK's `balance:` prefix
2. **Total supply readout fragile** — RPC `getContractInfo` iterates HashMap randomly, matching ANY key ending in `_supply` (could be `wbnb_supply`, `total_supply`, or even `musd_supply` on wrong contract)
3. **`confirmationStatus`** — sole camelCase field in otherwise snake_case RPC responses

The root cause is that the SDK `Token` type and the wrapped-token contracts were written independently with different storage conventions, and the core indexing layer only handles one pattern.

---

## 2. Architecture Layers

```
┌─────────────────────────────────────────────────────────┐
│  EXPLORER JS  (reads RPC responses)                     │
│  Fields expected: token_symbol, token_amount,           │
│  token_metadata.total_supply, token_metadata.decimals   │
├─────────────────────────────────────────────────────────┤
│  RPC LAYER  (rpc/src/lib.rs)                            │
│  Reads: SymbolRegistryEntry, ContractAccount.storage    │
│  Returns: snake_case JSON fields                        │
├─────────────────────────────────────────────────────────┤
│  CORE STATE  (core/src/state.rs)                        │
│  Stores: CF_SYMBOL_REGISTRY, CF_TOKEN_BALANCES,         │
│  CF_HOLDER_TOKENS, CF_SYMBOL_BY_PROGRAM                 │
├─────────────────────────────────────────────────────────┤
│  CORE PROCESSOR  (core/src/processor.rs)                │
│  Hooks: maybe_index_token_balance() — matches _bal_     │
│  Calls: b_update_token_balance()                        │
├─────────────────────────────────────────────────────────┤
│  CONTRACTS  (WASM)                                      │
│  Storage written via host storage_set()                  │
│  Wrapped: {prefix}_bal_{hex}, {prefix}_supply           │
│  SDK:     balance:{raw}, total_supply                   │
├─────────────────────────────────────────────────────────┤
│  GENESIS DEPLOY  (validator/src/main.rs)                │
│  Writes: SymbolRegistryEntry.metadata JSON              │
│  Fields: total_supply, decimals, mintable, burnable     │
└─────────────────────────────────────────────────────────┘
```

---

## 3. Inconsistency Inventory

### 3.1 CRITICAL — Storage Key Format Mismatch

| # | Issue | Location | Impact |
|---|-------|----------|--------|
| C1 | SDK Token balance key `balance:{raw32}` not matched by `_bal_` indexer | `sdk/src/token.rs:152`, `core/src/processor.rs:3509` | MoltCoin token balances NEVER indexed in CF_TOKEN_BALANCES |
| C2 | SDK Token allowance key `allowance:{raw32}:{raw32}` uses raw bytes, not hex | `sdk/src/token.rs:158` | No allowance indexing (but this is by design currently) |
| C3 | SDK Token supply key `total_supply` collides with `_supply` suffix scanner | `sdk/src/token.rs:38`, `rpc/src/lib.rs:6306-6316` | HashMap iteration may pick `total_supply` in prefix scan, then skip fallback |
| C4 | MoltCoin uses `b"owner"` with no prefix | `contracts/moltcoin/src/lib.rs:34` | Potential collision with other contracts using same key |

### 3.2 HIGH — RPC Response Field Naming

| # | Issue | Location | Impact |
|---|-------|----------|--------|
| H1 | `confirmationStatus` is camelCase; all other fields are snake_case | `rpc/src/lib.rs:3176` | Frontend must handle mixed casing |
| H2 | `token_metadata.decimals` vs contract storage `token_decimals` — different naming | `rpc/src/lib.rs:6335` → `6339` | Contract stores `token_decimals`, RPC outputs `decimals` — works but confusing |
| H3 | RPC `getContractInfo` returns `token_metadata.total_supply` but registry `metadata.total_supply` — same field in two nested paths | `rpc/src/lib.rs:6412,6424` | Explorer must check both `info.token_metadata.total_supply` AND `registry.metadata.total_supply` |

### 3.3 MEDIUM — Contract Metadata Inconsistency

| # | Issue | Location | Impact |
|---|-------|----------|--------|
| M1 | MoltCoin stores `token_name`, `token_symbol`, `token_decimals` in storage; wrapped tokens use compile-time constants only | `contracts/moltcoin/src/lib.rs:69-71` vs `contracts/wbnb_token/src/lib.rs:44-46` | RPC can read MoltCoin metadata from storage but must hardcode wrapped token metadata |
| M2 | MoltCoin `initialize()` returns void; wrapped tokens return `u32` | `contracts/moltcoin/src/lib.rs:52` vs `contracts/wbnb_token/src/lib.rs:197` | Init error handling inconsistent |
| M3 | Wrapped tokens have `{prefix}_minted` and `{prefix}_burned` counters; SDK Token has neither | Various wrapped contract files vs `sdk/src/token.rs` | No mint/burn history for SDK tokens |
| M4 | `SymbolRegistryEntry` has no `decimals` field — stored only in `metadata` JSON blob | `core/src/state.rs:96-105` | Must parse unstructured JSON to get decimals |

### 3.4 LOW — Cosmetic / Maintenance

| # | Issue | Location | Impact |
|---|-------|----------|--------|
| L1 | `getContractInfo` scans HashMap with `k.ends_with("_supply")` — `total_supply` matches this filter AND the fallback | `rpc/src/lib.rs:6308` | Double-matching possible but harmless |
| L2 | `!k.ends_with("_minted")` and `!k.ends_with("_burned")` filters are dead code — those keys don't end in `_supply` | `rpc/src/lib.rs:6310-6311` | Dead code |
| L3 | Explorer uses multiple fallback patterns: `infoMeta.total_supply ?? regMeta.total_supply ?? regMeta.supply` | `explorer/js/contract.js:221` | Works but fragile |
| L4 | Faucet JS hardcoded `MOLT_PER_REQUEST = 100` vs backend `MAX_PER_REQUEST` env | `faucet/faucet.js:12` | **ALREADY FIXED** in this session |
| L5 | Faucet HTML hardcoded "100 MOLT" placeholders | `faucet/index.html:67,81,97,162` | **ALREADY FIXED** in this session |

---

## 4. Layer-by-Layer Findings

### 4.1 Contracts Layer

#### Wrapped Tokens (wBNB, wETH, wSOL, mUSD) — CONSISTENT with each other

All four wrapped tokens share identical structure with different prefixes:

```rust
// Constants (compile-time)
const TOKEN_NAME: &[u8] = b"Wrapped BNB";
const TOKEN_SYMBOL: &[u8] = b"wBNB";
const DECIMALS: u8 = 9;

// Storage keys
const ADMIN_KEY: &[u8]        = b"wbnb_admin";
const TOTAL_SUPPLY_KEY: &[u8] = b"wbnb_supply";
const MINTED_KEY: &[u8]       = b"wbnb_minted";
const BURNED_KEY: &[u8]       = b"wbnb_burned";
const PAUSED_KEY: &[u8]       = b"wbnb_paused";
const REENTRANCY_KEY: &[u8]   = b"wbnb_reentrancy";

// Dynamic keys (address-based)
fn balance_key(addr)    → b"wbnb_bal_" + hex_encode(addr)     // 73 bytes
fn allowance_key(o,s)   → b"wbnb_alw_" + hex(o) + "_" + hex(s) // 142 bytes
```

**Functions (identical ABI across all 4):**
- `initialize(admin: *const u8) -> u32`
- `mint(caller: *const u8, to: *const u8, amount: u64) -> u32`
- `burn(caller: *const u8, amount: u64) -> u32`
- `transfer(from: *const u8, to: *const u8, amount: u64) -> u32`
- `approve(owner: *const u8, spender: *const u8, amount: u64) -> u32`
- `transfer_from(caller: *const u8, from: *const u8, to: *const u8, amount: u64) -> u32`
- `total_supply() -> u64`
- `total_minted() -> u64`
- `total_burned() -> u64`
- `balance_of(addr: *const u8) -> u64`
- `allowance(owner: *const u8, spender: *const u8) -> u64`
- `attest_reserves(caller: *const u8, reserve: u64, proof: *const u8) -> u32`
- `emergency_pause(caller: *const u8) -> u32`
- `emergency_unpause(caller: *const u8) -> u32`
- `transfer_admin(caller: *const u8, new: *const u8) -> u32`

#### MoltCoin (SDK Token) — INCOMPATIBLE storage format

```rust
// Constants (runtime via Token::new)
Token::new("MoltCoin", "MOLT", 9)

// Storage keys (SDK Token impl)
b"total_supply"                       // ← no prefix
b"balance:" + raw_bytes(addr)         // ← NOT hex, NOT _bal_ pattern
b"allowance:" + raw(owner) + ":" + raw(spender)

// MoltCoin-specific keys
b"owner"                              // ← no prefix
b"token_name"       → b"MoltCoin"     // runtime metadata
b"token_symbol"     → b"MOLT"
b"token_decimals"   → [9u8]
b"molt_reentrancy"                    // ← only key with prefix
```

### 4.2 Core Layer — Processor (Token Indexing)

**`maybe_index_token_balance`** at `core/src/processor.rs:3497`:
```rust
// Only matches: {anything}_bal_{64-hex-chars}
if let Some(pos) = key_str.find("_bal_") {
    let hex_part = &key_str[pos + 5..];
    if hex_part.len() != 64 { return Ok(()); }
    // decode hex → Pubkey → update CF_TOKEN_BALANCES
}
```

**Result:** Wrapped tokens (wbnb_bal_xxx) → INDEXED ✅. SDK tokens (balance:rawbytes) → NOT INDEXED ❌.

### 4.3 Core Layer — State (SymbolRegistryEntry)

```rust
pub struct SymbolRegistryEntry {
    pub symbol: String,           // Normalized uppercase, max 10 chars
    pub program: Pubkey,          // Contract address
    pub owner: Pubkey,            // Deployer
    pub name: Option<String>,     // e.g., "Wrapped BNB"
    pub template: Option<String>, // e.g., "wrapped", "token", "dex"
    pub metadata: Option<Value>,  // Unstructured JSON blob
}
```

**Problem:** No `decimals` field. Decimals are buried inside `metadata` JSON → requires parsing.

### 4.4 RPC Layer — getContractInfo Token Metadata Extraction

The RPC extracts token metadata from contract storage through a fragile multi-step process:

1. **Scan** HashMap for any key ending in `_supply` → use as `total_supply`
2. **Fallback** to literal `b"total_supply"` key
3. Read `b"token_decimals"` → output as `decimals`
4. Read `b"token_name"` → output as `token_name`
5. Read `b"token_symbol"` → output as `token_symbol`
6. Check ABI for `mint`/`burn` functions → output `mintable`/`burnable`
7. Merge with registry metadata fallbacks

**Output structure:**
```json
{
  "contract_id": "...",
  "owner": "...",
  "token_metadata": {
    "total_supply": 0,
    "decimals": 9,
    "token_name": "Wrapped BNB",
    "token_symbol": "wBNB",
    "mintable": true,
    "burnable": true
  },
  "is_native": false
}
```

### 4.5 RPC Layer — Transaction Token Enrichment

**`extract_token_info`** at `rpc/src/lib.rs:1102`:
- Reads contract call instruction args as raw bytes
- Parses amount at correct offset (fixed in this session)
- Returns `(symbol, amount, decimals, Option<recipient>)`
- Adds to JSON: `token_symbol`, `token_amount`, `token_amount_shells`, `token_decimals`, `token_to`, `contract_function`

**Field naming: ALL snake_case ✅** (except `confirmationStatus`)

### 4.6 Explorer JS — Field Expectations

The explorer reads these fields from RPC:

**From getContractInfo:**
- `info.token_metadata.total_supply` — primary
- `registry.metadata.total_supply` — fallback
- `registry.metadata.supply` — fallback #2
- `info.token_metadata.decimals` — primary
- `registry.metadata.decimals` — fallback

**From transaction responses:**
- `tx.token_symbol` — display token ticker
- `tx.token_amount` — display formatted amount
- `tx.amount_shells` — MOLT amount (primary)
- `tx.amount` — MOLT amount (fallback, multiplied by 1e9)

**From getMetrics:**
- `metrics.total_supply` — global MOLT supply (NOT per-token)

### 4.7 Genesis Deploy — Registry Metadata

The genesis deploy writes this metadata JSON to SymbolRegistryEntry:

**For `template: "token"` (MOLT):**
```json
{
  "genesis_deploy": true,
  "wasm_size": N,
  "total_supply": 1000000000000000000,
  "decimals": 9,
  "mintable": false,
  "burnable": true,
  "is_native": true,
  "description": "...",
  "website": "...",
  "logo_url": "...",
  "icon_class": "...",
  "twitter": "...",
  "telegram": "...",
  "discord": "..."
}
```

**For `template: "wrapped"` (wBNB, wETH, wSOL, mUSD):**
```json
{
  "genesis_deploy": true,
  "wasm_size": N,
  "total_supply": 0,
  "decimals": 9,
  "mintable": true,
  "burnable": true,
  "description": "...",
  "icon_class": "...",
  "logo_url": "..."
}
```

---

## 5. Unified Naming Standard

### 5.1 Storage Key Standard (ALL contracts must follow)

```
Balance:     {prefix}_bal_{hex64_address}     → u64 LE (8 bytes)
Allowance:   {prefix}_alw_{hex64}_{hex64}     → u64 LE (8 bytes)
Supply:      {prefix}_supply                  → u64 LE (8 bytes)
Minted:      {prefix}_minted                  → u64 LE (8 bytes)
Burned:      {prefix}_burned                  → u64 LE (8 bytes)
Admin:       {prefix}_admin                   → [u8; 32]
Paused:      {prefix}_paused                  → [u8; 1]
Reentrancy:  {prefix}_reentrancy              → [u8; 1]
```

Where `{prefix}` = lowercase symbol (e.g., `molt`, `wbnb`, `musd`).

### 5.2 SDK Token Changes Required

The SDK `Token` type must switch from:
- `balance:{raw32}` → `{prefix}_bal_{hex64}`
- `allowance:{raw}:{raw}` → `{prefix}_alw_{hex64}_{hex64}`
- `total_supply` → `{prefix}_supply`

This requires passing the prefix to `Token::new()` and updating all key generation.

### 5.3 MoltCoin Contract Changes Required

- `b"owner"` → `b"molt_admin"` (matches wrapped token pattern)
- `b"token_name"` → remove (use compile-time constant)
- `b"token_symbol"` → remove (use compile-time constant)
- `b"token_decimals"` → remove (use compile-time constant)
- Use wrapped-token-style direct storage instead of SDK Token abstraction

### 5.4 RPC Response Field Standard

ALL response fields: **snake_case**, no exceptions.

- `confirmationStatus` → `confirmation_status`
- `token_metadata` nested fields: `total_supply`, `decimals`, `token_name`, `token_symbol`, `mintable`, `burnable`

### 5.5 SymbolRegistryEntry Enhancement

Add `decimals` as a first-class field:
```rust
pub struct SymbolRegistryEntry {
    pub symbol: String,
    pub program: Pubkey,
    pub owner: Pubkey,
    pub name: Option<String>,
    pub template: Option<String>,
    pub decimals: Option<u8>,     // NEW: first-class field
    pub metadata: Option<Value>,
}
```

---

## 6. Action Items

### Phase 1: SDK Token Storage Key Alignment (C1, C2, C3)

**Files:** `sdk/src/token.rs`, `contracts/moltcoin/src/lib.rs`

1. Add `prefix: &'static str` field to SDK `Token` struct
2. Change `balance_key()` from `balance:{raw}` to `{prefix}_bal_{hex64}`
3. Change `allowance_key()` from `allowance:{raw}:{raw}` to `{prefix}_alw_{hex64}_{hex64}`
4. Change supply key from `total_supply` to `{prefix}_supply`
5. Update `Token::new()` to accept prefix parameter
6. Update MoltCoin contract to pass `"molt"` as prefix

### Phase 2: MoltCoin Contract Alignment (C4, M1, M2)

**Files:** `contracts/moltcoin/src/lib.rs`

1. Change `b"owner"` to `b"molt_admin"`
2. Remove runtime storage of `token_name`/`token_symbol`/`token_decimals` from `initialize()` — use compile-time constants like wrapped tokens
3. Change `initialize()` return type from void to `u32`
4. Add `{prefix}_minted` and `{prefix}_burned` counters to match wrapped tokens

### Phase 3: RPC getContractInfo Supply Scan Fix (L1, L2)

**Files:** `rpc/src/lib.rs`

1. Replace HashMap iteration with explicit prefixed-key lookup: try `{symbol.lower()}_supply` first, then `total_supply` fallback
2. Remove dead code filters (`_minted`, `_burned` exclusions)
3. Use SymbolRegistryEntry symbol to derive the correct prefix

### Phase 4: RPC Response Naming Fix (H1)

**Files:** `rpc/src/lib.rs`

1. Change `confirmationStatus` → `confirmation_status`
2. Update any SDK/test code that reads `confirmationStatus`

### Phase 5: Core Balance Indexing Enhancement (C1)

**Files:** `core/src/processor.rs`

1. After Phase 1, SDK Token will use `_bal_` pattern → indexing works automatically
2. No processor changes needed if SDK keys are fixed

### Phase 6: SymbolRegistryEntry Schema Update (M4)

**Files:** `core/src/state.rs`, `validator/src/main.rs`, `rpc/src/lib.rs`

1. Add `decimals: Option<u8>` to `SymbolRegistryEntry`
2. Populate in genesis deploy
3. Read in RPC as primary source instead of parsing contract storage

### Phase 7: Explorer JS Cleanup (L3)

**Files:** `explorer/js/contract.js`

1. After RPC returns consistent `token_metadata.total_supply`, remove fallback chains
2. Single source of truth: `info.token_metadata.total_supply`

---

## Already Fixed in This Session

| Item | File | Change |
|------|------|--------|
| **RPC token amount offset** | `rpc/src/lib.rs` | `extract_token_info` — mint reads `args[64..72]`, burn reads `args[32..40]` (was wrong) |
| **Token recipient extraction** | `rpc/src/lib.rs` | Added `token_to` field with recipient from `args[32..64]` for mint/transfer |
| **Token recipient indexing** | `core/src/state.rs` | `index_account_transactions` now also indexes token recipients embedded in args |
| **Faucet amount hardcode** | `faucet/faucet.js` | `MOLT_PER_REQUEST` now fetched from backend config |
| **Faucet HTML placeholders** | `faucet/index.html` | Removed hardcoded "100 MOLT" text |
