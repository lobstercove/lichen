# MoltChain Production Readiness Audit — Full Source Review

**Scope:** CLI, P2P, Validator, SDK (Rust/JS/Python), Compiler, Custody, Programs  
**Date:** 2026-02-25  
**Methodology:** Line-by-line source code review of all files in scope.

---

## Summary

| Severity | Count |
|----------|-------|
| Critical | 3     |
| High     | 5     |
| Medium   | 9     |
| Low      | 10    |
| **Total** | **27** |

---

## Findings

### 1. Custody: FROST Multi-Signer Pipeline Not Wired — Silently Produces Invalid Signatures

- **Component:** Custody
- **File:** `custody/src/main.rs` line 441–451
- **Category:** Incomplete Implementation / Security
- **Severity:** CRITICAL

```rust
// AUDIT-FIX R-C3: Multi-signer FROST protocol is implemented but the 2-round
// commit/sign flow (collect_frost_signatures) is not yet wired into the sweep
// and withdrawal pipelines. The generic collect_signatures uses single-round
// /sign which will NOT produce valid FROST shares. Multi-signer mode currently
// only works correctly in single-signer mode. DO NOT deploy with >1 signer
// until the FROST integration is completed and tested end-to-end.
tracing::error!(
    "🚨 MULTI-SIGNER MODE DETECTED ({}-of-{}). WARNING: FROST 2-round signing \
     is NOT yet wired into sweep/withdrawal pipelines. Only single-signer mode \
     is production-ready. Multi-signer deployments will produce invalid signatures.",
    config.signer_threshold,
    config.signer_endpoints.len()
);
```

The code itself documents that multi-signer FROST integration is incomplete. If deployed with >1 signer, sweep and withdrawal transactions will silently fail with invalid signatures. The runtime guard (`CUSTODY_ALLOW_UNSAFE_MULTISIGNER`) is present but this is still a critical gap for any multi-party custody deployment.

---

### 2. Custody: Insecure Default Master Seed Fallback

- **Component:** Custody
- **File:** `custody/src/main.rs` line 1520–1524
- **Category:** Security / Hardcoded Credentials
- **Severity:** CRITICAL

```rust
if std::env::var("CUSTODY_ALLOW_INSECURE_SEED").unwrap_or_default() == "1" {
    tracing::warn!(
        "⚠️  No master seed configured — using insecure default (dev mode)!"
    );
    "INSECURE_DEFAULT_SEED_DO_NOT_USE_IN_PRODUCTION".to_string()
}
```

When `CUSTODY_ALLOW_INSECURE_SEED=1`, the custody service falls back to a hardcoded deterministic master seed. All deposit addresses derived from this seed are publicly known. If a production operator accidentally sets this flag, all custodied funds are compromised. The env-var guard mitigates risk, but the hardcoded seed string should not exist in production builds.

---

### 3. CLI: Hardcoded Zero Blockhash in Marketplace Demo

- **Component:** CLI
- **File:** `cli/src/marketplace_demo.rs` line 294
- **Category:** Hardcoded Data / Stub
- **Severity:** CRITICAL

```rust
let message = Message {
    instructions,
    recent_blockhash: Hash::new([0u8; 32]),
};
```

The `send_tx` function in the marketplace demo uses a zero-value blockhash instead of fetching a recent blockhash from the validator. This means:

1. Transactions are immediately stale or replay-vulnerable.
2. Any validator with replay protection will reject them.
3. Without replay protection, they are infinitely replayable.

This function is called by the `molt marketplace seed` command, making it a real user-facing path.

---

### 4. CLI: Hardcoded `Pubkey([0xFFu8; 32])` as Contract Program ID

- **Component:** CLI
- **File:** `cli/src/client.rs` lines 383, 430, 483
- **Category:** Hardcoded Data
- **Severity:** HIGH

```rust
program_id: Pubkey::new([0xFFu8; 32]), // Contract program
```

The contract deploy, upgrade, and call operations use a hardcoded all-`0xFF` pubkey as the "contract program" ID. This is a magic constant that must exactly match the validator's internal routing. If the validator changes how contract instructions are dispatched, these CLI operations silently break. This should reference a named constant from `moltchain_core` (like `SYSTEM_PROGRAM_ID` is used elsewhere).

---

### 5. CLI: Hardcoded `Pubkey([0xDA; 32])` as DAO Contract Address

- **Component:** CLI
- **File:** `cli/src/main.rs` lines 1664, 1702, 1716, 1745, 1781, 1806
- **Category:** Hardcoded Data
- **Severity:** HIGH

```rust
let dao_addr = moltchain_core::Pubkey([0xDA; 32]); // DAO marker address
```

All six governance commands (propose, vote, list, info, execute, veto) reference a hardcoded `[0xDA; 32]` address as the DAO contract. If the actual deployed DAO contract has a different address (e.g., after redeployment), the entire governance subsystem silently fails. This should be a well-known constant or discoverable via on-chain lookup.

---

### 6. CLI: Hardcoded Explorer URL

- **Component:** CLI
- **File:** `cli/src/transaction.rs` line 152
- **Category:** Hardcoded Data
- **Severity:** HIGH

```rust
println!("\n💡 View in explorer: http://localhost:3000/transaction.html?sig={}", tx_hash);
```

The transaction helper prints a hardcoded `http://localhost:3000` explorer link after every successful transaction. This is incorrect for any non-local deployment (testnet, mainnet) and would confuse users. The URL should be derived from the current network configuration or omitted when not localhost.

---

### 7. Custody: Hardcoded Bind Address

- **Component:** Custody
- **File:** `custody/src/main.rs` line 587
- **Category:** Hardcoded Data
- **Severity:** HIGH

```rust
let addr: SocketAddr = "0.0.0.0:9105".parse().expect("valid bind addr");
```

The custody service binds to a hardcoded address `0.0.0.0:9105`. Unlike the validator (which accepts `--listen-addr` and `--rpc-port` flags), the custody service has no way to configure its bind address without modifying source code. In containerized or multi-instance deployments, this is inflexible.

---

### 8. Compiler: Hardcoded CORS Origin Fallback

- **Component:** Compiler
- **File:** `compiler/src/main.rs` line 134
- **Category:** Hardcoded Data
- **Severity:** HIGH

```rust
.unwrap_or_else(|_| "http://localhost:3000".to_string());
```

When `COMPILER_CORS_ORIGIN` env var is unset, the CORS allowed origin falls back to `http://localhost:3000`. In production, forgetting to set this env var would block all cross-origin requests from the actual frontend domain, causing silent compilation failures.

---

### 9. Validator: `#[allow(dead_code)]` on `genesis_exec_contract_with_value`

- **Component:** Validator
- **File:** `validator/src/main.rs` line 3373
- **Category:** Dead Code
- **Severity:** MEDIUM

```rust
#[allow(dead_code)]
fn genesis_exec_contract_with_value(
    state: &StateStore,
    program_pubkey: &Pubkey,
    deployer_pubkey: &Pubkey,
    function_name: &str,
    args: &[u8],
    value: u64,
    label: &str,
) -> bool {
```

A complete function (`genesis_exec_contract_with_value`) is annotated with `#[allow(dead_code)]`, indicating it is unused. This is a 40+ line function that should either be used or removed.

---

### 10. Validator: `#[allow(dead_code)]` on `SeedNetwork::chain_id`

- **Component:** Validator
- **File:** `validator/src/main.rs` line 244
- **Category:** Dead Code
- **Severity:** MEDIUM

```rust
#[allow(dead_code)]
chain_id: String,
```

The `chain_id` field in `SeedNetwork` is deserialized from `seeds.json` but never used, suppressed with `#[allow(dead_code)]`.

---

### 11. P2P: Unused `pub const MAX_PEERS: usize = 50`

- **Component:** P2P
- **File:** `p2p/src/peer.rs` line 243
- **Category:** Dead Code
- **Severity:** MEDIUM

```rust
pub const MAX_PEERS: usize = 50;
```

This constant is defined but never used for any logic. The actual peer limit is controlled by the `max_peers` instance field (set from `P2PConfig::effective_max_peers()`). The constant is misleading since the default for validators is 20, not 50.

---

### 12. Validator: `.unwrap()` in Test-Like Code Inside Production Binary

- **Component:** Validator
- **File:** `validator/src/main.rs` lines 12011–12152
- **Category:** Missing Error Handling
- **Severity:** MEDIUM

```rust
.unwrap();  // repeated 13 times in test functions
```

While these are in `#[cfg(test)]` blocks, the validator's 12K-line `main.rs` contains 13 `.unwrap()` calls at the end of the file in test code. This is acceptable test practice but worth flagging — no production `.unwrap()` calls were found.

---

### 13. Custody: `.unwrap()` on Hex Decode in EVM Multi-Sig Encoding

- **Component:** Custody
- **File:** `custody/src/main.rs` line 6965
- **Category:** Missing Error Handling
- **Severity:** MEDIUM

```rust
calldata.extend_from_slice(&hex::decode("6a761202").unwrap()); // execTransaction selector
```

This `unwrap()` on a compile-time-constant hex literal will never fail in practice, but it violates the pattern of returning `Result<_, String>` used by the rest of the function. Should use a `const` byte array instead.

---

### 14. Custody: Panic in Configuration Loading (Multiple)

- **Component:** Custody
- **File:** `custody/src/main.rs` lines 432, 454, 1486, 1492, 1526, 1583
- **Category:** Error Handling
- **Severity:** MEDIUM

```rust
panic!("FATAL: CUSTODY_MASTER_SEED_FILE '{}' is empty.", seed_path);
panic!("FATAL: Cannot read CUSTODY_MASTER_SEED_FILE '{}': {}", seed_path, e);
panic!("CRITICAL: CUSTODY_API_AUTH_TOKEN must be set and non-empty...");
panic!("CRITICAL: Multi-signer mode is not production-ready...");
```

Six `panic!()` calls during configuration loading. While panicking early for misconfiguration is a valid fail-fast strategy, these should be replaced with structured error returns (e.g., `Result`) so an orchestration layer can handle restart logic gracefully and log the error through a structured logging pipeline.

---

### 15. SDK (Python): Hardcoded Default URLs

- **Component:** SDK Python
- **File:** `sdk/python/moltchain/__init__.py` lines 29–30
- **Category:** Hardcoded Data
- **Severity:** MEDIUM

```python
DEFAULT_RPC_URL = "http://localhost:8899"
DEFAULT_WS_URL = "ws://localhost:8900"
```

The Python SDK exports hardcoded localhost URLs as module-level defaults. Third-party developers who do `from moltchain import DEFAULT_RPC_URL` and forget to override will silently connect to localhost, which may be a different service or timeout.

---

### 16. SDK (JS): Hardcoded Default URLs

- **Component:** SDK JS
- **File:** `sdk/js/src/index.ts` (top-level exports)
- **Category:** Hardcoded Data
- **Severity:** MEDIUM

```typescript
export const DEFAULT_RPC_URL = 'http://localhost:8899';
export const DEFAULT_WS_URL = 'ws://localhost:8900';
```

Same issue as the Python SDK. These are exported and easily misused in production applications.

---

### 17. P2P: Hardcoded Default Listen Address

- **Component:** P2P
- **File:** `p2p/src/network.rs` line 82
- **Category:** Hardcoded Data
- **Severity:** MEDIUM

```rust
listen_addr: "127.0.0.1:7001".parse().unwrap(),
```

The `P2PConfig::default()` binds to `127.0.0.1:7001`. This is safe (loopback-only) but the `.unwrap()` on a compile-time constant is technically unnecessary — a `const` or lazy static would be cleaner.

---

### 18. CLI: Hardcoded Default RPC URL in Clap Arg

- **Component:** CLI
- **File:** `cli/src/main.rs` line 24
- **Category:** Hardcoded Data
- **Severity:** LOW

```rust
#[arg(long, default_value = "http://localhost:8899")]
```

The CLI's `--rpc-url` flag defaults to localhost. This is standard practice for CLI tools and low severity, but worth documenting.

---

### 19. CLI: Hardcoded Default URLs in Config Module

- **Component:** CLI
- **File:** `cli/src/config.rs`
- **Category:** Hardcoded Data
- **Severity:** LOW

```rust
rpc_url: "http://localhost:8899"
ws_url: "ws://localhost:8900"
```

Config defaults for localhost URLs. Standard for local development.

---

### 20. CLI: `#[allow(dead_code)]` Annotations in keygen.rs

- **Component:** CLI
- **File:** `cli/src/keygen.rs` lines 114, 143, 288, 297, 364
- **Category:** Dead Code
- **Severity:** LOW

Five `#[allow(dead_code)]` annotations on KeypairFile fields and helper functions. These appear to be used by the test suite but not by non-test code.

---

### 21. CLI: `#[allow(dead_code)]` in keypair_manager.rs

- **Component:** CLI
- **File:** `cli/src/keypair_manager.rs` lines 30, 37, 148
- **Category:** Dead Code
- **Severity:** LOW

Three `#[allow(dead_code)]` annotations on KeypairFile variants and a test helper.

---

### 22. Marketplace Demo: Stub WASM Contract Functions

- **Component:** CLI
- **File:** `cli/src/marketplace_demo.rs` (build_marketplace_wasm function)
- **Category:** Stub / Placeholder
- **Severity:** LOW

The marketplace demo builds a WASM module with empty no-op exported functions (`list_nft`, `buy_nft`). These are explicitly a demo seeding tool, so the stub nature is by design, but the `send_tx` using zero blockhash (Finding #3) elevates this concern.

---

### 23. Programs JS: Hardcoded Localhost URLs

- **Component:** Programs
- **File:** `programs/js/landing.js` lines 14, 17; `programs/js/moltchain-sdk.js` lines 35–37
- **Category:** Hardcoded Data
- **Severity:** LOW

```javascript
rpc: 'http://localhost:8899',
ws: 'ws://localhost:8900',
explorer: 'http://localhost:8080',
```

The landing page and browser SDK contain localhost URLs as the "local" network config. These are explicitly for local development and the SDK also defines mainnet/testnet URLs. Low severity since these are labeled as "local" targets.

---

### 24. Validator: Placeholder State Root in Block Construction

- **Component:** Validator
- **File:** `validator/src/main.rs` lines 11752
- **Category:** Placeholder
- **Severity:** LOW

```rust
Hash::default(), // placeholder — will be set after effects
```

The block construction uses a two-step approach: create block with placeholder state root, apply effects, then compute real state root. This is documented and correct — the placeholder is overwritten before the block is finalized. Flagged for documentation completeness only.

---

### 25. Validator Updater: Release Signing Key Comment Residue

- **Component:** Validator
- **File:** `validator/src/updater.rs` lines 53–54
- **Category:** Comment Residue
- **Severity:** LOW

```rust
// PLACEHOLDER — replace with actual key after running generate-release-keys.sh
// AUDIT-FIX V5.5: Replaced placeholder all-zeros key with real Ed25519
```

The comments still say "PLACEHOLDER — replace with actual key" even though the fix comment directly below says it was replaced. The contradictory comments are confusing but the actual key value is non-zero (verified by test at line 775).

---

### 26. Custody: Test Config Uses Localhost URLs

- **Component:** Custody
- **File:** `custody/src/main.rs` lines 7819–7820
- **Category:** Hardcoded Test Data
- **Severity:** LOW

```rust
solana_rpc_url: Some("http://localhost:8899".to_string()),
evm_rpc_url: Some("http://localhost:8545".to_string()),
```

Test configuration with localhost URLs. This is expected and correct for unit tests.

---

### 27. Custody: Webhook URL Allows `http://localhost` Without TLS

- **Component:** Custody
- **File:** `custody/src/main.rs` lines 1605, 7146–7148
- **Category:** Security Exception
- **Severity:** LOW

```rust
if raw_url.starts_with("http://localhost") {
    return Ok(());
}
// ...
if !payload.url.starts_with("https://") && !payload.url.starts_with("http://localhost") {
    return Err(Json(ErrorResponse::invalid(
        "webhook url must use HTTPS (http://localhost allowed for dev)",
    )));
}
```

Webhooks allow unencrypted HTTP for localhost. This is a deliberate dev convenience, but the `http://localhost` prefix check is broad — it would also match `http://localhost.evil.com`. Should use a proper URL parser to verify the host is exactly `localhost` or `127.0.0.1`.

---

## Components with No Findings

The following components had no production readiness issues:

- **SDK Rust** (`sdk/src/`): Clean `no_std` WASM SDK. All five files (`lib.rs`, `crosscall.rs`, `dex.rs`, `nft.rs`, `token.rs`) use proper error handling, overflow-safe arithmetic (`u128`), and test mocks gated behind `#[cfg(test)]`.
- **P2P Gossip** (`p2p/src/gossip.rs`): Exponential backoff reconnection, properly bounded.
- **P2P Peer Ban** (`p2p/src/peer_ban.rs`): Escalating ban durations with RwLock-based thread safety.
- **P2P Peer Store** (`p2p/src/peer_store.rs`): Durable peer store with atomic fsync writes.
- **Validator Keypair Loader** (`validator/src/keypair_loader.rs`): Secure permission checks on keypair files.
- **Validator Threshold Signer** (`validator/src/threshold_signer.rs`): Bearer token auth for signing requests.
- **SDK Python** (`sdk/python/moltchain/`): Clean implementation with proper validation (keypair encryption, transaction building, shielded instructions). The `keypair.py` uses PBKDF2-HMAC-SHA256 with 600K rounds and AES-256-GCM — well-implemented.
- **SDK JS** (`sdk/js/src/`): Private key protection in `Keypair` class, defensive `toBytes()` copy, bigint for large amounts, constant-time-safe JSON parse guards on WebSocket messages.

---

## Recommendations

1. **Fix Finding #3 immediately** — the zero blockhash in `marketplace_demo.rs::send_tx()` makes all marketplace seed transactions invalid or replayable.
2. **Extract magic constants** (#4, #5) from CLI code into named constants in `moltchain_core` to prevent silent breakage.
3. **Make the explorer URL configurable** (#6) based on the active network config.
4. **Remove the insecure default seed fallback** (#2) from production builds entirely, or compile it out behind a `#[cfg(feature = "dev")]` gate.
5. **Complete FROST multi-signer integration** (#1) before any multi-party custody deployment.
6. **Make custody bind address configurable** (#7) via env var (e.g., `CUSTODY_BIND_ADDR`).
7. **Tighten the `http://localhost` webhook check** (#27) to use proper URL parsing.
