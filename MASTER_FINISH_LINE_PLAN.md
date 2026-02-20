# MoltChain — MASTER FINISH LINE PLAN

**Generated:** February 20, 2026 — Code-Level Production Audit  
**Method:** Line-by-line file audit of every source file in the repository, cross-referenced with all existing audit documents  
**Scope:** 200+ source files across core/, validator/, p2p/, rpc/, cli/, compiler/, custody/, faucet/, 27 contracts/, sdk/ (JS/Python/Rust/DEX), explorer/, wallet/ (web+extension), marketplace/, dex/, website/, monitoring/, programs/, developers/, scripts/, deploy/, infra/, tests/  
**Total findings:** 350+ distinct issues  

---

## How to Use This Document

- **Each finding has a unique ID** (section prefix + number) for tracking
- **Status column:** `[ ]` = not started, `[~]` = in progress, `[x]` = done
- **Severity:** CRITICAL (launch blocker), HIGH (production failure risk), MEDIUM (quality/reliability), LOW (polish)
- **Fix column:** Describes the exact code change required — no ambiguity
- **Findings from FINISH_LINE_PLAN.md are tagged** `[FLP]` and merged inline with new findings

---

## TABLE OF CONTENTS

1. [SECTION A: CORE CHAIN (core/src/)](#section-a-core-chain)
2. [SECTION B: VALIDATOR](#section-b-validator)
3. [SECTION C: P2P NETWORKING](#section-c-p2p-networking)
4. [SECTION D: RPC SERVER](#section-d-rpc-server)
5. [SECTION E: CLI](#section-e-cli)
6. [SECTION F: COMPILER & CUSTODY & FAUCET](#section-f-compiler-custody-faucet)
7. [SECTION G: SMART CONTRACTS (27 contracts)](#section-g-smart-contracts)
8. [SECTION H: SDK (JS / Python / Rust / DEX)](#section-h-sdk)
9. [SECTION I: FRONTENDS (Explorer/Wallet/DEX/Marketplace/Faucet/Website/Monitoring/Programs/Developers)](#section-i-frontends)
10. [SECTION J: SCRIPTS, DEPLOY, INFRA, DOCKER](#section-j-scripts-deploy-infra)
11. [SECTION K: TESTS & COVERAGE](#section-k-tests-coverage)
12. [SECTION L: CROSS-CUTTING SYSTEMIC ISSUES](#section-l-cross-cutting)
13. [SECTION M: FINISH_LINE_PLAN.md CROSS-REFERENCE](#section-m-cross-reference)
14. [EXECUTION ORDER & DEPENDENCY GRAPH](#execution-order)
15. [PROGRESS DASHBOARD](#progress-dashboard)

---

## SECTION A: CORE CHAIN {#section-a-core-chain}

### A.1 — core/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A1-01 | MEDIUM | Re-exports | `lib.rs` re-exports all modules publicly — no encapsulation boundary. Internal types like `RateLimitEntry`, `PrivacyPool` are part of the public API | Add `pub(crate)` to internal modules, expose only necessary types via explicit `pub use` | [ ] |

### A.2 — core/src/block.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A2-01 | HIGH | Determinism | Block timestamp uses `SystemTime::now()` at creation time — different validators produce different timestamps for the same slot, breaking deterministic consensus | Use slot-based timestamp derivation: `genesis_time + (slot * slot_duration_ms)`. Only allow leader to set timestamp within a bounded window | [x] |
| A2-02 | MEDIUM | Security | `verify()` checks `hash == calculated_hash` but does NOT verify the previous_hash chain — a block with an arbitrary `previous_hash` passes verification | Add `verify_chain(previous_block_hash)` method that validates both self-hash and previous-hash linkage | [ ] |
| A2-03 | MEDIUM | Missing | No Merkle root of transactions in block header — cannot do SPV verification or prove transaction inclusion | Add `transactions_root` field computed as Merkle root of transaction hashes | [ ] |
| A2-04 | LOW | Performance | `Block::new()` clones the entire transaction vector | Take ownership via `Vec<Transaction>` parameter instead of cloning | [ ] |

### A.3 — core/src/transaction.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A3-01 | HIGH | Security | Transaction signature is ed25519 over `hash` field, but `hash` is computed from only `(sender, receiver, amount, fee, nonce)` — `data` field (contract calls, instructions) is NOT signed | Include ALL fields in hash computation, especially `data`, `program_id`, and `transaction_type` | [x] |
| A3-02 | MEDIUM | Security | No transaction versioning — future format changes will be incompatible with existing signed transactions | Add `version: u8` field included in hash computation | [ ] |
| A3-03 | MEDIUM | Atomicity | `verify()` only checks signature validity, not nonce ordering, balance sufficiency, or blockhash freshness — those checks are spread across processor.rs with no atomic validation step | Create `validate_full()` that checks signature + nonce + balance + blockhash in one atomic operation | [ ] |
| A3-04 | LOW | Naming | `TransactionType` enum uses `ContractCall` and `ContractDeploy` but also has `NftMint`, `NftTransfer`, `NftBurn` — mixing contract operations with typed NFT operations | Consider unifying NFT operations under contract calls for consistency | [ ] |

### A.4 — core/src/state.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A4-01 | HIGH | Atomicity | State mutations (transfer, contract deploy, fee distribution) are individual `put_account()` calls with no transaction/batch mechanism. A crash between sender debit and receiver credit loses funds | Implement write-ahead log (WAL) or batch-commit: collect all mutations, apply atomically via `commit_batch()` | [x] |
| A4-02 | HIGH | Performance | Account lookups in `get_account()` do a full deserialization from the DB on every call — no in-memory cache | Add LRU cache for hot accounts (validators, fee collector, contract accounts) | [ ] |
| A4-03 | MEDIUM | Missing | No state snapshot/checkpoint mechanism — cannot reconstruct state at a previous block height | Implement state versioning with block-height-keyed snapshots | [ ] |
| A4-04 | MEDIUM | Missing | No global state root hash (world state trie) — cannot verify state consistency across validators | Implement incremental state root computation (e.g., sparse Merkle trie) | [ ] |
| A4-05 | LOW | Performance | `get_all_accounts()` loads every account into memory — will not scale | Add pagination or streaming iterator | [ ] |

### A.5 — core/src/consensus.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A5-01 | HIGH | Security | Leader selection uses `hash(slot + validator_set_hash) % validator_count` — predictable and manipulable if a validator can influence the validator set | Use VRF (Verifiable Random Function) for leader election, or at minimum add a randomness beacon | [x] |
| A5-02 | HIGH | Missing | No fork choice rule — if two valid blocks exist for the same slot, there's no defined resolution | Implement longest-chain or heaviest-subtree fork choice with finality gadget | [x] |
| A5-03 | MEDIUM | Inconsistency | `[FLP M21]` Slashing: genesis.rs defines 5% flat downtime penalty; consensus.rs implements 1% per 100 missed slots, max 10% | Align to graduated approach in consensus.rs, remove flat penalty from genesis.rs | [x] |
| A5-04 | MEDIUM | Security | Equivocation detection compares block hashes only — a malicious validator can produce two different blocks with different transactions but same hash if they control the hash function | Compare full block content, not just hashes, for equivocation detection | [ ] |
| A5-05 | LOW | Missing | No vote/attestation mechanism — blocks are accepted from the leader without supermajority confirmation | Implement 2/3+1 attestation requirement for block finality | [ ] |

### A.6 — core/src/mempool.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A6-01 | MEDIUM | Performance | Transaction removal is O(n) — iterates entire mempool to find transaction by hash | Use `HashMap<Hash, usize>` index for O(1) removal | [ ] |
| A6-02 | MEDIUM | Security | No per-account transaction limit in mempool — one account can flood with unlimited pending transactions | Add per-sender limit (e.g., 64 pending transactions per account) | [ ] |
| A6-03 | MEDIUM | Missing | No transaction replacement (RBF — Replace-By-Fee) mechanism | Allow higher-fee transaction to replace same-nonce transaction | [ ] |
| A6-04 | LOW | Performance | Priority sorting re-sorts on every `get_transactions()` call rather than maintaining a sorted structure | Use `BTreeSet` or `BinaryHeap` for O(log n) insertion with maintained sort | [ ] |

### A.7 — core/src/processor.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A7-01 | CRITICAL | Atomicity | `[FLP C10]` `cross_contract_call()` is a stub that returns 0. Every financial contract that depends on token transfers is non-functional | Implement real cross-contract call via host function or message-passing system | [x] DONE — commit 0f0fd6b |
| A7-02 | HIGH | Security | Fee distribution to validators uses proportional split but doesn't verify validator is still in the active set — slashed validators keep receiving fees until removed | Check validator status before fee distribution | [ ] |
| A7-03 | HIGH | Atomicity | Parallel transaction processing excludes `CONTRACT_PROGRAM_ID` from conflict detection — two contract calls to the same contract can execute in parallel and race on state | Add contract address to the read/write set for conflict detection | [ ] |
| A7-04 | MEDIUM | Security | Gas metering is simplified — WASM execution charges gas per instruction but doesn't account for memory allocation, I/O, or host function costs | Add gas costs for memory grow, storage read/write, and host function calls | [ ] |
| A7-05 | MEDIUM | Performance | Contract WASM is deserialized from JSON on every call — should be stored as raw bytes | Store WASM as raw bytes, not JSON-encoded | [ ] |
| A7-06 | LOW | Dead code | `process_block_rewards()` has a commented-out inflation adjustment section | Remove or implement the inflation adjustment | [ ] |

### A.8 — core/src/account.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A8-01 | MEDIUM | Missing | Account model has no rent/storage deposit mechanism — accounts persist forever with no cost | Implement minimum balance requirement or rent-based cleanup | [ ] |
| A8-02 | LOW | Naming | `Account.data` field is `Vec<u8>` used for both user data and contract storage — ambiguous | Separate into `account_data` and `contract_storage` | [ ] |

### A.9 — core/src/contract.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A9-01 | CRITICAL | Stub | `[FLP C10]` `call_token_transfer` host function is a stub — returns 0 without performing any transfer. ALL contracts that need to move tokens are broken | Implement as a real host function that performs atomic balance transfer in state | [x] DONE — commit 0f0fd6b |
| A9-02 | HIGH | Security | No WASM validation — any bytes can be deployed as a "contract". Malicious WASM can exhaust memory, import unavailable functions, or contain invalid opcodes | Validate WASM module structure, imports, memory limits before deployment | [ ] |
| A9-03 | HIGH | Missing | No contract size limit — a multi-GB WASM can be deployed | Add maximum contract size (e.g., 1 MB) enforced at deployment | [ ] |
| A9-04 | MEDIUM | Security | Contract storage is unbounded — a contract can allocate unlimited storage with no cost | Add storage deposit per byte or storage size limit per contract | [ ] |

### A.10 — core/src/contract_instruction.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A10-01 | MEDIUM | Redundancy | Both `ContractInstruction` and `ContractDeploy` exist as separate instruction types with overlapping fields | Unify into a single `ContractOperation` enum | [ ] |
| A10-02 | LOW | Missing | No instruction versioning — format changes break compatibility | Add version field | [ ] |

### A.11 — core/src/evm.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A11-01 | CRITICAL | Compatibility | `[NEW]` EVM `eth_gasPrice` and `eth_estimateGas` both return `base_fee`, causing MetaMask to display fee² instead of fee | `eth_estimateGas` should return gas units (21000 for transfers), not the fee. `eth_gasPrice` returns price per gas unit | [x] |
| A11-02 | CRITICAL | Compatibility | `[NEW]` `eth_getLogs` uses SHA-256 for event topic hashing instead of Keccak-256 — all EVM tooling (Ethers.js, web3.py) will fail to match events | Use Keccak-256 for topic hashes to match EVM standard | [x] |
| A11-03 | HIGH | Precision | EVM balance conversion truncates: `molt_balance / 1e10` discards sub-Gwei precision | Use proper 18-decimal scaling with no precision loss | [ ] |
| A11-04 | HIGH | Missing | `eth_call` (read-only contract calls) — not implemented, returns error | Implement `eth_call` for read-only state queries | [ ] |
| A11-05 | MEDIUM | Missing | No `eth_getCode`, `eth_getStorageAt`, `eth_getTransactionReceipt` with logs — basic EVM RPC methods | Implement standard EVM JSON-RPC methods | [ ] |

### A.12 — core/src/genesis.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A12-01 | HIGH | Inconsistency | `[FLP C9]` Genesis assigns 250M to validator rewards and 150M to builder grants; `multisig.rs` specifies 150M/250M respectively (reversed) | Align genesis.rs to match multisig.rs (canonical source) | [x] |
| A12-02 | MEDIUM | Hardcoded | Genesis validator pubkeys are hardcoded placeholder values — not real validator keys | Replace with actual validator public keys before launch | [ ] |
| A12-03 | LOW | Missing | No genesis timestamp — uses `SystemTime::now()` at creation, making genesis non-reproducible | Add fixed genesis timestamp | [ ] |

### A.13 — core/src/multisig.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A13-01 | HIGH | Security | Multisig secret keys are stored as `[u8; 32]` in-memory — no zeroization on drop | Use `zeroize` crate to clear secret key material from memory when no longer needed | [ ] |
| A13-02 | MEDIUM | Missing | Multisig requires 2-of-3 but threshold is hardcoded — no way to change threshold without redeployment | Make threshold configurable via governance | [ ] |
| A13-03 | LOW | Naming | Wallet names use camelCase (`validatorRewards`) while rest of codebase uses snake_case | Standardize naming | [ ] |

### A.14 — core/src/privacy.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A14-01 | CRITICAL | Fake implementation | `[FLP L2]` `verify_proof()` uses HMAC-SHA256 with publicly accessible data — trivially forgeable. No ZK crate dependencies, no Merkle tree, no Pedersen commitments | Full rewrite required per ZK_PRIVACY_IMPLEMENTATION_PLAN (8-12 weeks). For launch: either disable entirely or gate behind feature flag | [ ] |
| A14-02 | HIGH | Dead code | Entire privacy module is non-functional but ships in the binary — attack surface for no benefit | Feature-gate behind `#[cfg(feature = "privacy")]` and disable by default | [ ] |

### A.15 — core/src/hash.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A15-01 | LOW | Missing | Uses SHA-256 exclusively — no support for Keccak-256 needed for EVM compatibility | Add Keccak-256 hashing function for EVM topic hashes and address derivation | [ ] |

### A.16 — core/src/nft.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A16-01 | MEDIUM | Missing | NFT transfer doesn't emit events — off-chain indexers cannot track ownership changes | Add transfer events | [ ] |
| A16-02 | MEDIUM | Missing | No royalty enforcement on secondary sales — royalty info is stored but never checked during transfers | Enforce royalty payments on transfer if royalty > 0 | [ ] |
| A16-03 | LOW | Missing | No token URI / metadata standard — metadata is a plain string, not a structured standard | Define metadata JSON schema (name, description, image, attributes) | [ ] |

### A.17 — core/src/marketplace.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A17-01 | HIGH | Atomicity | NFT marketplace buy: deducts buyer balance, adds to seller, then transfers NFT — crash between payment and NFT transfer means buyer pays but doesn't receive NFT | Use atomic batch: all three state changes in one commit | [ ] |
| A17-02 | MEDIUM | Missing | No listing expiration — listings persist forever | Add expiry timestamp and cleanup mechanism | [ ] |
| A17-03 | MEDIUM | Security | No minimum price — can list NFT for 0 tokens | Enforce minimum listing price | [ ] |

### A.18 — core/src/network.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A18-01 | MEDIUM | Hardcoded | Default port 8000, max peers 50 are hardcoded — not configurable | Read from config.toml | [ ] |
| A18-02 | LOW | Dead code | Network configuration struct is defined but most fields are unused — actual networking is in p2p/ crate | Remove or wire up to p2p/ crate | [ ] |

### A.19 — core/src/event_stream.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A19-01 | MEDIUM | Performance | Event subscribers are stored in an unbounded `Vec` — no cleanup of disconnected subscribers | Add periodic cleanup of dead subscribers; bound max subscribers | [ ] |
| A19-02 | LOW | Missing | No event filtering — subscribers receive ALL events | Add topic-based filtering per subscriber | [ ] |

### A.20 — core/src/reefstake.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| A20-01 | MEDIUM | Missing | Staking rewards calculation is simplified — fixed APY rather than dynamic based on total staked | Implement dynamic reward rate based on total staked percentage | [ ] |
| A20-02 | LOW | Naming | Module called `reefstake` but the chain is "moltchain" — naming inconsistency | Rename to `staking` or `molt_stake` | [ ] |

---

## SECTION B: VALIDATOR {#section-b-validator}

### B.1 — validator/src/main.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| B1-01 | CRITICAL | Consensus bypass | `[NEW]` Prediction market RPC endpoints write directly to contract storage bypassing block consensus — state divergence across validators | Route all contract state changes through transactions that go through consensus | [x] DONE — commit e977a44 |
| B1-02 | CRITICAL | Wiring | `[FLP C2]` All 26 contracts are deployed at genesis but their `initialize()` is never called — all contracts are inert with no admin, no config, no state | Add Phase 2 (initialize) and Phase 3 (create DEX pairs) to `genesis_auto_deploy()` | [x] |
| B1-03 | HIGH | Security | Bootstrap account creation uses hardcoded amounts without governance control | Make bootstrap amounts configurable and add multisig approval | [ ] |
| B1-04 | HIGH | Monolithic | Main.rs is 7000+ lines — contains block production, RPC wiring, genesis, contract deployment, airdrop, snapshot sync in one file | Split into modules: block_producer.rs, genesis_deploy.rs, sync.rs, airdrop.rs | [ ] |
| B1-05 | MEDIUM | Performance | Block production loop uses sleep-based polling instead of event-driven slot timing | Use precise slot timer with clock synchronization | [ ] |
| B1-06 | MEDIUM | Security | Snapshot sync trusts remote validator stats (treasury balance, total supply) — a malicious snapshot-sender can inflate treasury | Verify snapshot integrity via state root hash before applying | [ ] |

### B.2 — validator/src/keypair_loader.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| B2-01 | HIGH | Security | Keypair file is read with standard file permissions — no check for world-readable permissions | Check file permissions (0600) and warn/error if too permissive | [ ] |
| B2-02 | LOW | Error handling | `unwrap()` on keypair file read — validator crashes with unhelpful message if file missing | Provide clear error message with path and expected format | [ ] |

### B.3 — validator/src/sync.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| B3-01 | HIGH | Security | Block sync downloads blocks from peers without verifying the full chain of signatures — accepts any block sequence | Verify each block's signature and previous_hash linkage during sync | [ ] |
| B3-02 | MEDIUM | Performance | Sync is sequential block-by-block — slow for catching up thousands of blocks | Implement parallel block download with sequential verification | [ ] |

### B.4 — validator/src/threshold_signer.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| B4-01 | CRITICAL | Not wired | `[NEW]` FROST threshold signing is implemented but never called from any code path — custody uses single-signer only. All custodied funds rely on one private key | Wire threshold signer into custody sweep/credit operations | [ ] |
| B4-02 | MEDIUM | Missing | No key refresh / rotation mechanism | Implement periodic key share rotation | [ ] |

### B.5 — validator/src/updater.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| B5-01 | HIGH | Security | Auto-updater downloads binary from URL without cryptographic verification — MITM can inject malicious validator binary | Verify download against signed hash from multisig-controlled manifest | [ ] |
| B5-02 | MEDIUM | Missing | No rollback mechanism if update fails | Implement canary deploy: keep old binary, rollback on crash | [ ] |

---

## SECTION C: P2P NETWORKING {#section-c-p2p-networking}

### C.1 — p2p/src/network.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| C1-01 | CRITICAL | Security | `[NEW]` `SkipServerVerification` disables ALL TLS certificate validation — any node can impersonate any peer, enabling MITM attacks and eclipse attacks | Implement proper TLS certificate validation with pinned certificates or a CA | [x] |
| C1-02 | HIGH | Security | No peer authentication — anyone can connect and send messages. No validator identity verification | Require TLS client certificates signed by a known CA, or use noise protocol with validator pubkeys | [ ] |
| C1-03 | MEDIUM | Missing | No connection rate limiting — a single IP can open unlimited connections | Add per-IP connection limit and rate limiting | [ ] |

### C.2 — p2p/src/gossip.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| C2-01 | HIGH | Security | No gossip message deduplication — same message can be relayed infinitely, amplifying bandwidth attacks | Add seen-message cache with TTL-based expiry | [x] |
| C2-02 | MEDIUM | Performance | Gossip fans out to ALL connected peers — no intelligent peer selection | Implement gossip-sub with mesh topology and limited fanout | [ ] |

### C.3 — p2p/src/message.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| C3-01 | MEDIUM | Security | No message size limit — a peer can send arbitrarily large messages causing OOM | Add maximum message size (e.g., 10 MB) with length-prefix validation | [ ] |
| C3-02 | LOW | Missing | No message compression — blocks and transactions sent uncompressed | Add LZ4/zstd compression for large messages | [ ] |

### C.4 — p2p/src/peer.rs, peer_ban.rs, peer_store.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| C4-01 | MEDIUM | Missing | Peer banning uses in-memory map — restarts clear all bans | Persist ban list to disk | [ ] |
| C4-02 | MEDIUM | Missing | No peer scoring system — can't gradually penalize misbehaving peers before banning | Implement reputation scoring with graduated penalties | [ ] |
| C4-03 | LOW | Missing | Peer store doesn't persist known peers — restart requires re-discovery from seed nodes | Persist peer list to disk, load on startup | [ ] |

---

## SECTION D: RPC SERVER {#section-d-rpc-server}

### D.1 — rpc/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| D1-01 | HIGH | Security | CORS is set to `*` (allow all origins) — any website can make RPC calls | Restrict to known frontend origins in production | [x] |
| D1-02 | HIGH | Wiring | `[FLP M20]` MoltyID reputation RPC reads stale `ContractAccount.storage` instead of `CF_CONTRACT_STORAGE` — returns wrong reputation values | Change handlers to read from `CF_CONTRACT_STORAGE` via `state.get_contract_storage()` | [ ] |
| D1-03 | MEDIUM | Performance | No request rate limiting on RPC server — vulnerable to DoS | Add per-IP rate limiting (e.g., 100 req/s) | [ ] |
| D1-04 | MEDIUM | Missing | No API versioning — breaking changes affect all clients | Add `/v1/` prefix to all endpoints | [ ] |
| D1-05 | MEDIUM | Consistency | Mixed response formats — some endpoints return `{result: ...}`, others return raw data | Standardize all responses to `{jsonrpc: "2.0", result: ..., id: ...}` | [ ] |

### D.2 — rpc/src/dex.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| D2-01 | HIGH | Consensus bypass | `[NEW]` DEX order matching and execution happen inside RPC handler — state changes bypass block consensus | All order operations must go through transaction → consensus → processor pipeline | [ ] |
| D2-02 | MEDIUM | Missing | No pagination on order book queries — large order books return all orders in one response | Add `limit` and `offset` parameters | [ ] |

### D.3 — rpc/src/dex_ws.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| D3-01 | MEDIUM | Security | `std::sync::Mutex` used in async WebSocket context — can deadlock under load | Use `tokio::sync::Mutex` for async compatibility | [x] |
| D3-02 | MEDIUM | Missing | No WebSocket heartbeat/ping-pong — stale connections are never detected | Implement periodic ping-pong with 30s timeout | [ ] |
| D3-03 | LOW | Performance | Each new WS subscription clones full state — memory-intensive for many subscribers | Use `Arc<RwLock<State>>` with shared reads | [ ] |

### D.4 — rpc/src/launchpad.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| D4-01 | HIGH | Consensus bypass | Launchpad token creation writes directly to state without going through consensus | Route through transaction pipeline | [ ] |
| D4-02 | MEDIUM | Security | No input validation on token metadata — name, symbol, description can be arbitrary length | Add length limits and sanitization | [ ] |

### D.5 — rpc/src/prediction.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| D5-01 | CRITICAL | Consensus bypass | `[NEW]` Market creation, bet placement, resolution all write directly to contract storage — bypasses validator consensus entirely | All prediction market operations must go through consensus as transactions | [ ] |
| D5-02 | HIGH | Security | Market resolution has no oracle verification — anyone who can call the RPC can resolve any market with any outcome | Add oracle verification and multi-sig resolution | [ ] |

### D.6 — rpc/src/ws.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| D6-01 | MEDIUM | Security | `std::sync::Mutex` in async WS handler — same deadlock risk as D3-01 | Use `tokio::sync::Mutex` | [x] |
| D6-02 | MEDIUM | Missing | No subscription management — can't unsubscribe from specific topics | Implement unsubscribe mechanism with subscription IDs | [ ] |
| D6-03 | LOW | Missing | No max subscriptions per client limit | Add per-connection subscription limit | [ ] |

---

## SECTION E: CLI {#section-e-cli}

### E.1 — cli/src/main.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| E1-01 | LOW | Missing | No --version flag | Add version from Cargo.toml | [ ] |
| E1-02 | LOW | UX | Error messages are technical — no user-friendly guidance | Add contextual help on common errors | [ ] |

### E.2 — cli/src/client.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| E2-01 | MEDIUM | Error handling | HTTP errors return raw reqwest error — no parsing of RPC error responses | Parse JSON-RPC error response and display meaningful message | [ ] |
| E2-02 | MEDIUM | Missing | No retry logic for transient network errors | Add exponential backoff retry (3 attempts) | [ ] |

### E.3 — cli/src/keygen.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| E3-01 | HIGH | Security | Generated keypair is written to file with default permissions — may be world-readable | Set file permissions to 0600 immediately after creation | [ ] |
| E3-02 | MEDIUM | Security | No BIP39 mnemonic option — only raw key generation | Add mnemonic generation with proper PBKDF2 derivation | [ ] |

### E.4 — cli/src/keypair_manager.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| E4-01 | MEDIUM | Security | Keypair stored as plaintext JSON — no encryption at rest | Support password-encrypted keypair files | [ ] |

### E.5 — cli/src/transaction.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| E5-01 | MEDIUM | UX | No transaction confirmation prompt for large amounts | Add confirmation prompt for transfers > 1000 MOLT | [ ] |
| E5-02 | LOW | Missing | No offline transaction signing mode | Add `--offline` flag for air-gapped signing | [ ] |

### E.6 — cli/src/wallet.rs, query.rs, config.rs, marketplace_demo.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| E6-01 | LOW | Dead code | `marketplace_demo.rs` is a test script, not a CLI command — shouldn't be in production binary | Move to examples/ or remove from CLI build | [ ] |
| E6-02 | LOW | Missing | Config file location is hardcoded — no XDG base directory support | Use `dirs` crate for platform-appropriate config location | [ ] |

---

## SECTION F: COMPILER, CUSTODY & FAUCET {#section-f-compiler-custody-faucet}

### F.1 — compiler/src/main.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| F1-01 | HIGH | Security | `[NEW]` Compiler executes `cargo build` on user-provided code with no sandboxing — arbitrary code execution on the build server | Run compilation in a sandboxed container (Docker/WASM sandbox) with resource limits | [ ] |
| F1-02 | MEDIUM | Missing | No WASM output validation — compiled WASM isn't checked for conformance | Validate WASM module structure post-compilation | [ ] |

### F.2 — custody/src/main.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| F2-01 | CRITICAL | Security | `[NEW]` Single master seed for all custodied funds — compromise of one key compromises everything | Use per-chain HD derivation with hardware security module (HSM) integration | [x] |
| F2-02 | CRITICAL | Atomicity | `[NEW]` Deposit processing and ledger rebalancing are non-atomic — crash during rebalance can lose track of funds | Implement intent log with idempotent replay | [ ] |
| F2-03 | HIGH | Security | Master seed stored in environment variable — visible in process listing, docker inspect, crash dumps | Use secrets manager (Vault, AWS Secrets Manager) or HSM | [ ] |
| F2-04 | HIGH | Wiring | `[NEW]` EVM withdrawal path produces invalid output (`[addr][calldata]` instead of RLP-encoded transaction) | Fix EVM transaction construction to produce valid RLP-encoded output | [ ] |
| F2-05 | MEDIUM | Performance | `std::sync::Mutex` in async context — deadlock risk | Use `tokio::sync::Mutex` | [ ] |
| F2-06 | MEDIUM | Missing | No automated reconciliation — no way to detect balance discrepancies | Add periodic balance reconciliation with alerting | [ ] |

### F.3 — faucet/src/main.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| F3-01 | MEDIUM | Security | Rate limiting uses in-memory HashMap keyed by IP — restart clears all limits | Persist rate limit state or use external rate limiter (Redis) | [ ] |
| F3-02 | MEDIUM | Security | No captcha or proof-of-work — bots can drain faucet | Add captcha verification or proof-of-work challenge | [ ] |
| F3-03 | LOW | Hardcoded | Faucet amount and cooldown period are hardcoded | Make configurable via environment variables | [ ] |

---

## SECTION G: SMART CONTRACTS {#section-g-smart-contracts}

### G.1 — contracts/moltcoin/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G1-01 | CRITICAL | Security | `[FLP C3]` `approve()` has no `get_caller()` verification — any account can set allowances for any other account | Add `let caller = get_caller();` and verify caller is the token owner | [x] |
| G1-02 | CRITICAL | Security | `[FLP C3]` `mint()` uses parameter as caller identity instead of `get_caller()` — owner is spoofable. Combined with G1-01, allows total token theft | Use `get_caller()` to verify mint authority | [x] |
| G1-03 | MEDIUM | Missing | No `burn()` function — tokens can never be destroyed | Add `burn()` with caller verification | [ ] |

### G.2 — contracts/dex_core/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G2-01 | HIGH | Security | Cross-contract calls to transfer tokens fail silently — trades proceed without actual token movement | Fail the trade if token transfer fails (fail-close, not fail-open) | [ ] |
| G2-02 | HIGH | Performance | Order matching is O(n) scan of all orders in the book | Use sorted data structure (BTreeMap by price) for O(log n) matching | [ ] |
| G2-03 | MEDIUM | Missing | `[FLP M5]` Cancelled/filled orders remain in storage indefinitely — unbounded growth | Add pruning mechanism or TTL-based cleanup | [ ] |
| G2-04 | MEDIUM | Missing | `[FLP H20]` Post-Only and Reduce-Only flags exist in contract but aren't wired from the frontend | Wire flags through the full stack: UI → SDK → RPC → contract | [ ] |
| G2-05 | MEDIUM | Missing | `[FLP M8]` Market orders have limited slippage protection — no circuit breaker for extreme price impact | Add maximum slippage percentage check and dynamic circuit breakers | [ ] |

### G.3 — contracts/dex_amm/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G3-01 | CRITICAL | Math | `[FLP C4]` `tick_to_sqrt_price` uses linear approximation instead of correct exponential formula (`1.0001^(tick/2)`) — prices are wrong at all ticks | Implement correct exponential tick-to-price conversion using fixed-point arithmetic | [x] |
| G3-02 | HIGH | Performance | `[FLP M7]` Fee distribution iterates over ALL liquidity positions — O(n) gas cost | Switch to per-share fee accumulator pattern (like Uniswap V3's feeGrowthGlobal) | [ ] |
| G3-03 | MEDIUM | Missing | No fee tier configuration — single fee rate for all pools | Add configurable fee tiers (0.01%, 0.05%, 0.3%, 1%) | [ ] |

### G.4 — contracts/dex_analytics/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G4-01 | MEDIUM | Missing | `[FLP M6]` Candle retention policies defined but never enforced — storage grows indefinitely | Implement cleanup routine that deletes candles older than retention period | [ ] |
| G4-02 | LOW | Performance | Volume tracking uses unbounded maps | Add periodic aggregation and pruning | [ ] |

### G.5 — contracts/dex_governance/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G5-01 | HIGH | Security | `[FLP M18]` Accepts caller-provided reputation score — any user can claim arbitrary reputation to bypass voting thresholds | Read reputation from MoltyID contract on-chain via cross-contract call | [ ] |
| G5-02 | HIGH | Stub | `[FLP H2]` `execute_proposal()` sets status to "executed" but performs no on-chain action — governance votes have no effect | Implement proposal execution that actually changes parameters, lists tokens, disburses funds | [ ] |

### G.6 — contracts/dex_margin/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G6-01 | HIGH | Security | `[FLP H4]` No host-level collateral locking — user can open margin position and still spend the collateral | Implement locked_balance field in account model and lock collateral atomically | [x] |
| G6-02 | HIGH | Missing | `[FLP H6]` No funding rate implementation — perpetual futures prices can diverge arbitrarily from spot | Implement periodic funding rate calculation and settlement | [x] |
| G6-03 | HIGH | Security | `[FLP H8]` `close_position` returns full margin on missing oracle price — traders can close losing positions at no loss during oracle outage | Return error when oracle price unavailable, or use last known price with staleness check | [ ] |
| G6-04 | MEDIUM | Missing | `[FLP H5]` Insurance fund has no withdrawal/deployment mechanism — funds are permanently trapped | Add governance-controlled withdrawal mechanism | [ ] |
| G6-05 | LOW | Precision | `[FLP L6]` Liquidation penalty integer division remainder (up to 1 shell) is lost — dust amounts | Use rounding that favors insurance fund | [ ] |

### G.7 — contracts/dex_rewards/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G7-01 | CRITICAL | Security | `[FLP C12]` `initialize()` can be called by anyone — attacker seizes admin control of rewards emissions | Add `get_caller()` check in `initialize()` or only allow genesis block initialization | [x] |
| G7-02 | HIGH | Missing | `[FLP C11]` Reward claims update bookkeeping but never transfer MOLT tokens — no source wallet defined | Wire to builder_grants wallet (250M MOLT) for actual token transfers | [x] |
| G7-03 | MEDIUM | Missing | No epoch-based distribution — rewards are calculated per-claim with no time boundaries | Implement epoch-based reward distribution with snapshots | [ ] |

### G.8 — contracts/dex_router/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G8-01 | CRITICAL | Fake data | `[FLP C6]` Router swap uses simulation fallback that fabricates output amounts when cross-contract calls fail (which they always do) — trades return simulated values, not real movements | Remove simulation fallback; fail if cross-contract call fails | [x] |
| G8-02 | HIGH | Missing | No multi-hop routing optimization — routes are hardcoded 2-hop maximum | Implement optimal path finding (Dijkstra or BFS on pair graph) | [ ] |

### G.9 — contracts/lobsterlend/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G9-01 | HIGH | Financial | Token transfers are bookkeeping-only — no actual token movement on deposit, withdraw, borrow, or repay | Wire all operations to actual token transfers via host function | [x] |
| G9-02 | MEDIUM | Security | `[FLP M12]` Emergency pause blocks ALL operations including withdrawals — traps user funds | Allow withdrawals even when paused | [ ] |
| G9-03 | MEDIUM | Overflow | `[FLP M13]` Borrow interest calculation can overflow u64 for large positions | Use checked arithmetic or u128 intermediates | [ ] |
| G9-04 | MEDIUM | ABI | `[FLP M14]` Query functions use output pointers incompatible with JSON ABI encoder | Redesign queries to return serialized data via WASM memory | [ ] |
| G9-05 | LOW | Missing | `[FLP L7]` Flash loan fee (0.09%) truncates to 0 for loans < 1,112 shells — free flash loans | Enforce minimum fee of 1 shell | [ ] |

### G.10 — contracts/moltauction/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G10-01 | HIGH | Security | `[FLP H13]` `create_auction` uses parameter-provided creator without `get_caller()` — spoofable ownership | Use `get_caller()` for creator identity | [x] |
| G10-02 | HIGH | Atomicity | Auction settlement is non-atomic — bid refunds and winner payment are separate operations | Batch all settlement operations atomically | [ ] |
| G10-03 | MEDIUM | Inconsistency | `[FLP M11]` Mixed return code conventions — some functions return 0 for success, others return 1 | Standardize to 1=success across all functions | [ ] |

### G.11 — contracts/moltbridge/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G11-01 | HIGH | Financial | Bridge deposits and withdrawals are bookkeeping-only — no actual token locking or minting | Implement proper lock-and-mint / burn-and-release bridge mechanics | [x] |
| G11-02 | HIGH | Security | No relay/oracle mechanism for cross-chain verification — bridge trusts submitted proofs without validation | Implement relay verification or multi-sig oracle committee | [ ] |
| G11-03 | MEDIUM | Missing | No bridge fee mechanism | Add configurable bridge fees | [ ] |

### G.12 — contracts/moltcoin/src/lib.rs (continued)

(Covered in G.1 above)

### G.13 — contracts/moltdao/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G13-01 | HIGH | Security | `[FLP H12]` `cancel_proposal` has no `get_caller()` verification — anyone can cancel any proposal | Add caller verification — only proposer or admin can cancel | [x] |
| G13-02 | HIGH | Stub | `[FLP H2]` `execute_proposal` is placeholder — sets status but performs no action | Implement cross-contract execution of proposal actions | [ ] |
| G13-03 | MEDIUM | Security | Caller-provided reputation accepted for vote weight — self-reported | Read from MoltyID contract | [ ] |

### G.14 — contracts/moltmarket/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G14-01 | HIGH | Atomicity | Purchase: deducts buyer, credits seller as separate operations — non-atomic | Batch into single atomic operation | [ ] |
| G14-02 | MEDIUM | Missing | No listing fee | Add configurable listing fee | [ ] |

### G.15 — contracts/moltoracle/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G15-01 | CRITICAL | Security | `[FLP C8]` `submit_price` accepts feeder address as parameter without `get_caller()` — anyone can submit prices as any authorized feeder | Use `get_caller()` to verify feeder identity | [x] |
| G15-02 | HIGH | Security | Single-feeder model with no price deviation guard — one compromised feeder poisons all consumers (DEX, margin, prediction) | Implement multi-feeder median with deviation threshold and circuit breaker | [ ] |
| G15-03 | MEDIUM | Security | `simple_hash` is not cryptographic — VRF is forgeable | Use proper cryptographic hash for VRF | [ ] |

### G.16 — contracts/moltpunks/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G16-01 | MEDIUM | Missing | Burned NFT entries remain in storage — ghost entries can be queried | Delete NFT data on burn, not just mark as burned | [ ] |
| G16-02 | LOW | Missing | No royalty enforcement on secondary market sales | Add royalty check on transfer-with-payment | [ ] |

### G.17 — contracts/moltswap/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G17-01 | HIGH | Security | Admin key is wrong — uses incorrect hardcoded address | Fix admin key to correct multisig address | [ ] |
| G17-02 | HIGH | Financial | Swap operations are bookkeeping-only — constant product formula is applied but tokens don't move | Wire to actual token transfers | [x] |
| G17-03 | MEDIUM | Missing | No minimum liquidity lock on pool creation — LP can drain entire pool immediately | Lock minimum liquidity (e.g., 1000 shells) on first deposit | [ ] |

### G.18 — contracts/moltyid/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G18-01 | MEDIUM | Data loss | Social recovery replaces auth data — original credentials are lost if recovery is triggered maliciously | Keep backup of original auth, require cool-down period | [ ] |
| G18-02 | MEDIUM | Security | Missing reentrancy guard on auction functions | Add reentrancy flag check | [x] ✅ All 42 moltyid state-mutating functions now guarded with reentrancy_enter/exit. bid_name_auction CEI violation fixed (state before external call). |
| G18-03 | LOW | Naming | 37 exports — some have inconsistent naming (camelCase vs snake_case in ABI) | Standardize all exports to snake_case | [ ] |

### G.19 — contracts/musd_token/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G19-01 | HIGH | Bug | `[FLP C1]` Wrapped token WASMs compile to 86 bytes (empty) due to missing `#[no_mangle] pub extern "C"` annotations — mUSD is DEX quote currency, nothing works without it | Add `#[no_mangle] pub extern "C"` to all exported functions | [x] |
| G19-02 | MEDIUM | Bug | Double epoch reset — epoch can be reset twice in some edge cases | Add epoch already-reset guard | [ ] |

### G.20 — contracts/weth_token/src/lib.rs & wsol_token/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G20-01 | HIGH | Bug | `[FLP C1]` Same empty WASM issue as mUSD — missing extern annotations | Add `#[no_mangle] pub extern "C"` annotations | [x] |
| G20-02 | MEDIUM | Bug | Same double epoch reset issue as mUSD | Add guard | [ ] |

### G.21 — contracts/prediction_market/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G21-01 | CRITICAL | Financial | `[FLP C5]` No actual mUSD token transfers — tracks balances internally but never debits on buy or credits on redemption | Wire to actual token transfers | [x] |
| G21-02 | HIGH | Math | Withdrawal calculation uses u32 truncation — precision loss on large amounts | Use u64 throughout | [ ] |
| G21-03 | MEDIUM | Missing | Multi-outcome market math is incomplete — only binary (yes/no) outcomes supported properly | Complete multi-outcome AMM math | [ ] |

### G.22 — contracts/bountyboard/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G22-01 | HIGH | Financial | `[FLP H9]` `cancel_bounty` ignores transfer failure; `approve_work` records payment but never transfers tokens | Wire actual token transfers; fail on transfer failure | [x] |
| G22-02 | MEDIUM | Security | `[FLP L8]` First-caller-wins admin init — mitigated if genesis init (B1-02) is implemented | Ensure genesis initialization is done before user transactions | [x] mitigated by B1-02 |

### G.23 — contracts/clawpay/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G23-01 | HIGH | Financial | `[FLP H11]` `cancel_stream` calculates refund but never returns funds | Wire actual token transfer for refund | [x] |
| G23-02 | HIGH | Security | `[FLP H10]` Missing reentrancy guard — exploitable once transfers are wired | Add reentrancy protection flag | [x] ✅ Already complete: clawpay has CP_REENTRANCY_KEY on all 5 exports |
| G23-03 | MEDIUM | Financial | Stream claim calculates owed amount but never transfers | Wire actual token transfer | [ ] |

### G.24 — contracts/clawpump/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G24-01 | HIGH | Missing | `[FLP H3]` Graduation at 100K MOLT sets flag but doesn't create DEX pair, AMM pool, or seed liquidity | Implement full graduation: create pair + seed liquidity | [x] |
| G24-02 | HIGH | Atomicity | Partial cross-call failure during graduation is not reverted | Implement rollback on graduation failure | [ ] |

### G.25 — contracts/clawvault/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G25-01 | HIGH | Overflow | `[FLP H14]` Share-to-asset conversion and fee accumulation can overflow u64 | Use checked arithmetic or u128 intermediates | [ ] |
| G25-02 | HIGH | Fake data | `[FLP H15]` Vault APY is hardcoded/simulated — not connected to real yield sources | Connect to LobsterLend interest and MoltSwap LP fees | [x] |

### G.26 — contracts/compute_market/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G26-01 | CRITICAL | Security | `[FLP C7]` 5 admin functions accept caller parameter without `get_caller()` — anyone can modify config or pause contract | Use `get_caller()` in all admin functions | [x] |
| G26-02 | HIGH | Bug | `[FLP H19]` Paused state returns 0 (success) instead of error — callers think operations succeeded | Return error code when paused | [ ] |
| G26-03 | HIGH | Financial | `[FLP H18]` `resolve_dispute` uses wrong transfer source | Fix source account for dispute resolution transfers | [ ] |
| G26-04 | MEDIUM | Bug | `[FLP M19]` Job cancellation timeout calculated from `created_slot` instead of `claim_slot` | Use `claim_slot` for timeout calculation | [ ] |

### G.27 — contracts/reef_storage/src/lib.rs

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| G27-01 | CRITICAL | Stub | `respond_challenge` — placeholder verification accepts any response | Implement actual proof-of-storage verification | [ ] |
| G27-02 | HIGH | Financial | Storage payments are bookkeeping-only — no token transfers | Wire actual token transfers | [x] |
| G27-03 | MEDIUM | Security | First-caller-wins admin init | Ensure genesis initialization | [ ] |

---

## SECTION H: SDK {#section-h-sdk}

### H.1 — sdk/js/src/ (TypeScript SDK)

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| H1-01 | CRITICAL | Security | Private key exposed in `Keypair.toString()` — logs, serialization, debugging can leak keys | Remove private key from `toString()`, add explicit `secretKey` getter with warning JSDoc | [x] |
| H1-02 | HIGH | Serialization | Transaction serialization uses JSON+base64 — incompatible with Rust SDK's bincode format | Align all SDKs to single wire format (bincode or standardize JSON schema) | [ ] |
| H1-03 | HIGH | Precision | Amount fields use JavaScript `number` (f64) — loses precision above 2^53 (~9M MOLT) | Use `BigInt` for all amount fields | [ ] |
| H1-04 | HIGH | Missing | No WebSocket reconnection logic — connection drop = permanent loss | Implement auto-reconnect with exponential backoff | [ ] |
| H1-05 | MEDIUM | Missing | No `simulateTransaction()`, no `getProgramAccounts()` — core methods missing | Implement missing RPC methods | [ ] |
| H1-06 | MEDIUM | Error handling | RPC errors are thrown as raw strings — no structured error types | Create error class hierarchy with error codes | [ ] |
| H1-07 | LOW | Missing | No request timeout configuration | Add configurable timeout (default 30s) | [ ] |

### H.2 — sdk/python/moltchain/ (Python SDK)

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| H2-01 | HIGH | Serialization | Uses JSON+base64 format — incompatible with bincode used elsewhere | Align to standard wire format | [ ] |
| H2-02 | HIGH | Missing | No WebSocket reconnection, no heartbeat | Implement reconnection and heartbeat | [ ] |
| H2-03 | MEDIUM | Missing | Missing methods: `simulateTransaction`, `getProgramAccounts`, `getTokenAccountsByOwner` | Implement missing methods | [ ] |
| H2-04 | MEDIUM | Deprecated | Uses deprecated asyncio event loop API | Update to modern asyncio patterns | [ ] |
| H2-05 | LOW | Missing | No type hints on many functions | Add comprehensive type annotations | [ ] |

### H.3 — sdk/rust/src/ (Rust SDK)

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| H3-01 | HIGH | Serialization | Uses bincode+hex — different from JS/Python JSON+base64 format | Align to standard wire format | [ ] |
| H3-02 | MEDIUM | Missing | No WebSocket support at all — only HTTP | Add WebSocket subscription client | [ ] |
| H3-03 | MEDIUM | Error handling | Some methods unwrap internally — panics in library code | Replace all unwraps with proper error propagation | [ ] |
| H3-04 | LOW | Missing | No connection pooling | Add HTTP connection pool via reqwest | [ ] |

### H.4 — dex/sdk/src/ (DEX TypeScript SDK)

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| H4-01 | HIGH | Architecture mismatch | AMM SDK assumes concentrated liquidity (tick ranges) but on-chain contract uses basic x*y=k — API/contract mismatch | Align SDK to match on-chain contract interface | [ ] |
| H4-02 | HIGH | Missing | No WebSocket heartbeat — uses `setInterval` ping but no pong verification | Implement proper ping-pong with disconnect on timeout | [ ] |
| H4-03 | MEDIUM | Type safety | Fee parameters not persisted — set in memory but lost on reconnection | Persist configuration or fetch from chain on init | [ ] |
| H4-04 | MEDIUM | Missing | No error recovery — failed WebSocket messages are silently dropped | Add message queue with retry for critical operations | [ ] |

### H.5 — sdk/src/ (Core Rust SDK Library)

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| H5-01 | MEDIUM | Stub | `crosscall.rs` — cross-contract call builder exists but actual execution is stubbed | Wire to real cross-contract execution when A7-01/A9-01 are fixed | [ ] |
| H5-02 | LOW | Dead code | Several helper functions are defined but never called from any contract | Audit and remove unused exports | [ ] |

### H.6 — shared/wallet-connect.js

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| H6-01 | CRITICAL | Security | `[NEW]` Generates random bytes as fake addresses when wallet extension is unavailable — funds sent to these addresses are permanently lost (no private key) | Never generate fake addresses; show clear error prompting user to install wallet extension | [x] |
| H6-02 | HIGH | Security | API key transmitted in cleartext | Use HTTPS only and consider removing API key from client-side code | [ ] |

### H.7 — shared-config.js

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| H7-01 | HIGH | Security | Stores private keys in localStorage with no encryption | Encrypt keys before storage; use WebCrypto API | [ ] |
| H7-02 | MEDIUM | Consistency | Network URL inconsistency — some frontends use `localhost:8000`, others `localhost:9000` | Centralize all URLs in shared-config.js and ensure all apps use it | [ ] |

---

## SECTION I: FRONTENDS {#section-i-frontends}

### I.1 — explorer/

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| I1-01 | HIGH | Duplication | `escapeHtml()` duplicated across 11 JS files — any bug fix must be applied 11 times | Extract to shared utils.js, import everywhere | [x] ✅ All duplicates removed: escapeHtml from 11 files, formatHash/rpcCall from 6 marketplace files, bs58 from address.js, formatTimeFull/formatShells consolidated |
| I1-02 | HIGH | Duplication | Base58 encoder/decoder duplicated across 6 files | Move to shared utility | [ ] |
| I1-03 | HIGH | Duplication | RPC client class duplicated across 5 files | Create shared RPC client module | [ ] |
| I1-04 | MEDIUM | Performance | Address page makes N+1 RPC calls — one for account, one for each transaction | Batch RPC calls or add server-side transaction history endpoint | [ ] |
| I1-05 | MEDIUM | Missing | No pagination on transaction/block lists — loads everything | Add pagination with configurable page size | [ ] |
| I1-06 | MEDIUM | Accessibility | Zero ARIA attributes, no keyboard navigation, no screen reader support across all pages | Add ARIA labels, roles, keyboard handlers | [ ] |
| I1-07 | LOW | Missing | No loading states — content appears after delay with no indicator | Add loading spinners/skeletons | [ ] |

### I.2 — wallet/ (web)

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| I2-01 | CRITICAL | Security | `[NEW]` BIP39 implementation uses SHA-512 instead of PBKDF2 for seed derivation — wallets are incompatible with every other crypto wallet | Implement proper PBKDF2 with 2048 iterations per BIP39 spec | [x] |
| I2-02 | CRITICAL | Security | Secret keys stored in plaintext in localStorage — XSS exfiltrates all keys | Encrypt with user password using WebCrypto AES-GCM | [x] |
| I2-03 | HIGH | Security | No Content Security Policy (CSP) headers — XSS can inject scripts | Add strict CSP meta tag or server header | [ ] |
| I2-04 | MEDIUM | Missing | No hardware wallet support (Ledger, Trezor) | Add WebUSB/WebHID integration | [ ] |
| I2-05 | MEDIUM | Missing | No transaction history display | Add transaction list for connected wallet | [ ] |

### I.3 — wallet/extension/

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| I3-01 | CRITICAL | Security | Same BIP39 issue as web wallet — SHA-512 instead of PBKDF2 | Fix key derivation | [ ] |
| I3-02 | HIGH | Security | Unclear permission model — content script has broad page access | Minimize permissions to only necessary origins | [ ] |
| I3-03 | MEDIUM | Missing | No dApp connection approval UI — auto-connects to any requesting page | Add connection approval popup with site info | [ ] |
| I3-04 | MEDIUM | Missing | No transaction signing confirmation — signs without user approval | Add transaction review and approval flow | [ ] |

### I.4 — dex/

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| I4-01 | HIGH | Security | Private keys stored in localStorage via shared-config.js | Use wallet extension for signing, never store keys in DEX | [ ] |
| I4-02 | HIGH | Mock data | `[NEW]` Price data includes mock/simulated values — chart may show fake prices | Remove mock price feed, use only live oracle data | [ ] |
| I4-03 | MEDIUM | Missing | `[FLP M2]` Bottom panel needs consolidation — duplicate Positions/Margin tabs | Consolidate panels, add liquidation price column | [ ] |
| I4-04 | MEDIUM | Missing | `[FLP M4]` No slippage tolerance setting, no chart memory across sessions | Implement settings persistence in localStorage | [ ] |
| I4-05 | MEDIUM | Performance | TradingView charting library loaded synchronously — blocks page render | Async load with loading indicator | [ ] |

### I.5 — marketplace/

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| I5-01 | HIGH | Stub | Browse page uses hardcoded mock data — no live data from chain | Wire to RPC for real NFT listings | [ ] |
| I5-02 | HIGH | Missing | No actual buy/sell functionality — buttons are present but no transaction logic | Implement purchase transaction flow | [ ] |
| I5-03 | MEDIUM | Missing | No image upload/IPFS integration for NFT creation | Integrate IPFS or arweave for metadata storage | [ ] |
| I5-04 | LOW | Style | CSS differs significantly from other frontends — visual inconsistency | Align to shared-theme.css | [ ] |

### I.6 — faucet/ (frontend)

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| I6-01 | MEDIUM | Security | Client-side captcha only — easily bypassed | Move captcha validation server-side | [ ] |
| I6-02 | LOW | Missing | No transaction confirmation display after faucet drip | Show transaction hash with explorer link | [ ] |

### I.7 — website/

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| I7-01 | LOW | Missing | No analytics integration | Add privacy-respecting analytics (Plausible/Umami) | [ ] |
| I7-02 | LOW | Content | Some placeholder text remains | Replace with final copy | [ ] |

### I.8 — monitoring/

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| I8-01 | HIGH | Security | `[NEW]` Admin kill switch accessible without authentication — any visitor can restart validator | Add authentication to admin endpoints | [ ] |
| I8-02 | MEDIUM | Mock data | Dashboard shows simulated metrics — not connected to real Prometheus/Grafana | Wire to actual metrics endpoints | [ ] |
| I8-03 | MEDIUM | Missing | No alerting — only display, no notifications on critical events | Integrate with alertmanager/PagerDuty | [ ] |

### I.9 — programs/ (Playground IDE)

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| I9-01 | HIGH | Security | Code execution in playground happens on server without sandboxing — ties to F1-01 | Sandbox compilation and execution | [ ] |
| I9-02 | MEDIUM | Missing | No syntax highlighting for Rust in editor | Add CodeMirror or Monaco editor with Rust mode | [ ] |

### I.10 — developers/ (Documentation Portal)

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| I10-01 | MEDIUM | Accuracy | Some RPC endpoint docs may not match actual implementation | Audit docs against actual RPC handlers | [ ] |
| I10-02 | LOW | Missing | No interactive API playground (like Swagger UI) | Add try-it-now capability for RPC endpoints | [ ] |

---

## SECTION J: SCRIPTS, DEPLOY, INFRA {#section-j-scripts-deploy-infra}

### J.1 — Docker & Infrastructure

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| J1-01 | CRITICAL | Config | `[NEW]` Custody RPC URL points to wrong port in Docker Compose — custody service can't reach chain | Fix port mapping in docker-compose.yml | [x] |
| J1-02 | CRITICAL | Config | `[NEW]` EXPOSE ports in Dockerfile.moltchain don't match compose port mappings — services unreachable | Align Dockerfile EXPOSE with docker-compose ports | [x] |
| J1-03 | HIGH | Reliability | `[NEW]` `|| true` swallows contract build failures in Dockerfile and Makefile — broken contracts are silently deployed | Remove `|| true`, fail build on any contract compilation error | [x] |
| J1-04 | HIGH | Missing | No Docker health checks — compose can't detect unhealthy services | Add HEALTHCHECK to all Dockerfiles | [x] |
| J1-05 | HIGH | Missing | No resource limits (memory, CPU) in docker-compose — OOM can crash host | Add resource limits to all services | [ ] |
| J1-06 | MEDIUM | Missing | No persistent volume configuration — container restart loses all state | Add named volumes for chain data and custody state | [ ] |
| J1-07 | MEDIUM | Security | Nginx config missing rate limiting, security headers | Add rate limiting, HSTS, X-Frame-Options, CSP | [ ] |

### J.2 — Scripts

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| J2-01 | CRITICAL | Security | `[NEW]` Genesis has placeholder validator pubkeys — mainnet would launch with fake validators | Replace with real validator public keys before launch | [x] |
| J2-02 | CRITICAL | Security | `[NEW]` Deployer keypair used as contract admin instead of multisig | Switch to multisig-controlled admin addresses | [x] |
| J2-03 | HIGH | Missing | `[NEW]` No CI/CD pipeline exists anywhere in the repo | Create GitHub Actions workflow: lint → test → build → deploy | [x] |
| J2-04 | HIGH | Reliability | Several scripts have hardcoded paths (e.g., `/Users/johnrobin/...`) | Use relative paths or environment variables | [ ] |
| J2-05 | MEDIUM | Missing | No automated genesis generation from config — manual process | Script to generate genesis from validated config file | [ ] |
| J2-06 | MEDIUM | Portability | Scripts assume macOS (e.g., `brew`, BSD sed) — won't work on Linux | Add OS detection and platform-specific commands | [ ] |

### J.3 — Monitoring Infrastructure

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| J3-01 | HIGH | Missing | `[NEW]` Prometheus alertmanager has no targets configured — no alerts fire | Configure alertmanager with targets and route to notification channel | [x] |
| J3-02 | MEDIUM | Missing | Grafana dashboards reference metrics that don't exist yet | Create validator metrics exporter, then update dashboards | [ ] |
| J3-03 | MEDIUM | Missing | No log aggregation — logs are stdout only | Add structured logging with log rotation and aggregation (ELK/Loki) | [ ] |

---

## SECTION K: TESTS & COVERAGE {#section-k-tests-coverage}

### K.1 — Core Rust Tests

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| K1-01 | HIGH | Coverage | No tests for parallel transaction processing conflict detection | Add tests for concurrent access to same account | [ ] |
| K1-02 | HIGH | Coverage | No tests for fork handling / chain reorganization | Add fork choice tests with competing blocks | [ ] |
| K1-03 | HIGH | Coverage | No tests for WASM contract execution — only unit tests for host functions | Add integration tests that deploy and call WASM contracts | [ ] |
| K1-04 | MEDIUM | Coverage | No tests for genesis initialization of all 26 contracts | Add genesis deploy + init test | [ ] |
| K1-05 | MEDIUM | Coverage | No EVM compatibility tests — MetaMask, ethers.js | Add EVM RPC conformance tests | [ ] |
| K1-06 | MEDIUM | Coverage | No stress/load tests in CI | Add benchmark tests for throughput measurement | [ ] |

### K.2 — Contract Tests

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| K2-01 | CRITICAL | Coverage | No unit tests exist for ANY of the 27 contracts | Write comprehensive test suites for each contract | [ ] |
| K2-02 | HIGH | Coverage | No fuzzing targets for financial contracts (DEX, lending, swaps) | Add fuzz targets using cargo-fuzz | [ ] |
| K2-03 | HIGH | Coverage | No property-based tests for AMM math correctness | Add property tests: constant product invariant, no arbitrage | [ ] |

### K.3 — E2E Tests

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| K3-01 | HIGH | Coverage | E2E tests exist but many test mock endpoints rather than actual chain | Rewrite to test against running validator | [ ] |
| K3-02 | HIGH | Coverage | No multi-validator consensus E2E test | Add 3-validator boot + block production + finality test | [ ] |
| K3-03 | MEDIUM | Coverage | No E2E test for full DEX trading flow (deposit → trade → withdraw) | Add complete trading lifecycle test | [ ] |
| K3-04 | MEDIUM | Coverage | No E2E test for custody operations (deposit → sweep → credit) | Add custody lifecycle test | [ ] |
| K3-05 | MEDIUM | Coverage | Frontend E2E tests (test_wallet_audit.js etc.) check HTML structure, not functionality | Add Playwright/Cypress tests for user flows | [ ] |

### K.4 — SDK Tests

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| K4-01 | HIGH | Coverage | SDK tests mostly test serialization — no round-trip tests against running chain | Add integration tests that submit real transactions | [ ] |
| K4-02 | MEDIUM | Coverage | No cross-SDK compatibility tests (same transaction in JS/Python/Rust should produce same bytes) | Add cross-SDK serialization compatibility test | [ ] |

---

## SECTION L: CROSS-CUTTING SYSTEMIC ISSUES {#section-l-cross-cutting}

### L.1 — Systemic: Cross-Contract Calls Non-Functional

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| L1-01 | CRITICAL | Systemic | `[FLP C10]` The single largest blocker: `cross_contract_call()` is a stub. This breaks: ALL token transfers in contracts, DEX router, bridge, rewards, lending, auctions, bounties, streaming payments, vault yields, prediction market payouts — approximately 80% of contract functionality is non-functional | **Two-pronged fix:** (1) Implement `call_token_transfer` as a host function that atomically modifies account balances in state, (2) Implement general cross-contract call that loads target WASM and invokes with arguments | [x] DONE — commit 0f0fd6b |

### L.2 — Systemic: Wire Format Incompatibility

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| L2-01 | CRITICAL | Systemic | `[NEW]` Three different serialization formats across SDKs: JS (JSON+base64), Python (JSON+base64), Rust (bincode+hex). Transactions created in one SDK cannot be submitted by another | Standardize on ONE wire format. Recommend: bincode for efficiency with base58-encoded representation for display | [x] DONE — commit eabd791 |

### L.3 — Systemic: Consensus Bypass in RPC

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| L3-01 | CRITICAL | Systemic | `[NEW]` Multiple RPC endpoints (prediction markets, DEX orders, launchpad) write directly to state, bypassing block consensus. If multiple validators run, their states will diverge | ALL state mutations must flow through: user → transaction → mempool → consensus → block → processor → state | [x] DONE — commit e977a44 |

### L.4 — Systemic: No Atomic State Transitions

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| L4-01 | CRITICAL | Systemic | `[NEW]` Balance transfers, fee distribution, contract deployment, and bootstrap account creation are sequential `put_account()` calls. A crash between any two calls leaves inconsistent state | Implement batch-commit / write-ahead log (WAL) for all multi-step state transitions | [x] |

### L.5 — Systemic: Code Duplication Across Frontends

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| L5-01 | HIGH | Systemic | 7 utility functions duplicated across 5-14 files each (escapeHtml, Base58, RPC client, formatAmount, formatTime, copyToClipboard, createPagination). Bug fixes are nearly impossible to apply consistently | Create shared JS module: `shared/utils.js` with all common functions, import in all frontends | [x] ✅ shared/utils.js created with 20+ canonical functions, imported in 28 HTML files, removed duplicates from 22 JS files |

### L.6 — Systemic: Unchecked Arithmetic in All Contracts

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| L6-01 | MEDIUM | Systemic | `[FLP M1]` Fee accumulators, volume trackers, reward counters across all 27 contracts use unchecked u64 arithmetic — overflow possible under high volume | Global find-replace: use `saturating_add()`/`checked_add()` for all counter and accumulator operations | [x] ✅ 24 overflow fixes across 8 crates: processor.rs (u128 fee split + saturating treasury), lobsterlend (u128 collateral_seized + saturating reserves), clawvault (saturating_sub cap check + saturating fees), consensus (saturating_sub stake check + saturating slashed), dex_core (u128 protocol_fee + saturating volume), dex_rewards (4 saturating_add), dex_amm (5 saturating_add), clawpump (6 saturating_add). Regression test added. 471 tests pass. |

### L.7 — Systemic: No Structured Event/Log System

| ID | Severity | Category | Finding | Fix Required | Status |
|----|----------|----------|---------|-------------|--------|
| L7-01 | MEDIUM | Systemic | `[FLP M15]` No contract emits structured events. Off-chain indexers and UIs rely on polling storage | Implement event emission host function: `emit_event(topic, data)` available to all contracts | [ ] |

---

## SECTION M: FINISH_LINE_PLAN.md CROSS-REFERENCE {#section-m-cross-reference}

Every item from FINISH_LINE_PLAN.md has been mapped into this audit. Here is the cross-reference:

| FLP ID | This Audit ID | Status | Notes |
|--------|---------------|--------|-------|
| C1 (Empty WASMs) | G19-01, G20-01 | Merged | Confirmed via code read — missing `#[no_mangle]` |
| C2 (Genesis not initialized) | B1-02 | Merged | Confirmed — no init calls in genesis_auto_deploy |
| C3 (moltcoin caller bypass) | G1-01, G1-02 | Merged | Confirmed — approve and mint both vulnerable |
| C4 (AMM tick pricing) | G3-01 | Merged | Confirmed — linear approximation, not exponential |
| C5 (prediction no transfers) | G21-01 | Merged | Confirmed — pure accounting, no token movement |
| C6 (router simulation) | G8-01 | Merged | Confirmed — fabricated outputs on failure |
| C7 (compute_market no caller) | G26-01 | Merged | Confirmed — 5 admin functions unprotected |
| C8 (oracle no caller) | G15-01 | Merged | Confirmed — feeder address as parameter |
| C9 (genesis mismatch) | A12-01 | Merged | Confirmed — 250M/150M vs 150M/250M |
| C10 (token transfers stub) | L1-01, A7-01, A9-01 | **FIXED** | **Commit 0f0fd6b** — full re-entrant CCC with atomic state propagation, 7 tests |
| C11 (dex_rewards no transfer) | G7-02 | Merged | Confirmed |
| C12 (dex_rewards init) | G7-01 | Merged | Confirmed — no caller check |
| H1 (Stop-loss) | NOT in code audit | Tracked | Feature addition, not code defect |
| H2 (Governance execute) | G5-02, G13-02 | Merged | Confirmed — both DAO contracts |
| H3 (ClawPump graduation) | G24-01 | Merged | Confirmed |
| H4 (Collateral locking) | G6-01 | Merged | Confirmed — no locked_balance primitive |
| H5 (Insurance fund) | G6-04 | Merged | Confirmed |
| H6 (Funding rate) | G6-02 | Merged | Confirmed — not implemented |
| H7 (Tokenomics params) | NEW | Tracked | Parameter adjustment needed |
| H8 (close_position oracle) | G6-03 | Merged | Confirmed |
| H9 (bountyboard transfers) | G22-01 | Merged | Confirmed |
| H10 (clawpay reentrancy) | G23-02 | Merged | Confirmed |
| H11 (clawpay cancel) | G23-01 | Merged | Confirmed |
| H12 (moltdao cancel) | G13-01 | Merged | Confirmed |
| H13 (moltauction caller) | G10-01 | Merged | Confirmed |
| H14 (clawvault overflow) | G25-01 | Merged | Confirmed |
| H15 (vault yields fake) | G25-02 | Merged | Confirmed |
| H16 (Validator graduation) | NOT in code audit | Tracked | New feature, no code yet |
| H17 (Build validation) | J2-03 | Merged | No CI/CD pipeline |
| H18 (compute dispute source) | G26-03 | Merged | Confirmed |
| H19 (compute paused=0) | G26-02 | Merged | Confirmed |
| H20 (Post-Only not wired) | G2-04 | Merged | Confirmed |
| M1 (Unchecked arithmetic) | L6-01 | Merged | Global issue |
| M2 (DEX bottom panel) | I4-03 | Merged | |
| M3 (Margin enhancements) | Depends on G6-02 | Tracked | |
| M4 (DEX settings) | I4-04 | Merged | |
| M5 (Order pruning) | G2-03 | Merged | |
| M6 (Candle retention) | G4-01 | Merged | |
| M7 (AMM fee O(n)) | G3-02 | Merged | |
| M8 (Slippage control) | G2-05 | Merged | |
| M9 (Rebalance pricing) | F2-06 | Tracked | |
| M10 (Custody 6 items) | F2-01 through F2-06 | Merged | |
| M11 (Auction return codes) | G10-03 | Merged | |
| M12 (lobsterlend pause) | G9-02 | Merged | |
| M13 (lobsterlend overflow) | G9-03 | Merged | |
| M14 (lobsterlend ABI) | G9-04 | Merged | |
| M15 (Event system) | L7-01 | Merged | |
| M16 (MoltyID UI) | NOT in code audit | Tracked | Feature addition |
| M17 (Prediction full impl) | Depends on G21-01 | Tracked | |
| M18 (Gov reputation) | G5-01 | Merged | |
| M19 (compute timeout) | G26-04 | Merged | |
| M20 (MoltyID stale reads) | D1-02 | Merged | |
| M21 (Slashing discrepancy) | A5-03 | Merged | |
| L1 (Agent Economy) | NOT in code audit | Tracked | Future phase |
| L2 (ZK Privacy) | A14-01 | Merged | Dead code, feature-gate it |
| L3 (MoltyID Vision) | NOT in code audit | Tracked | Future phase |
| L4 (Cross-margin) | NOT in code audit | Tracked | Future feature |
| L5 (PnL share card) | NOT in code audit | Tracked | Minor feature |
| L6 (Liquidation dust) | G6-05 | Merged | |
| L7 (Flash loan fee) | G9-05 | Merged | |
| L8 (First-caller admin) | G22-02, G27-03 | Merged | Mitigated by B1-02 |
| L9 (Prediction CLOB) | NOT in code audit | Tracked | Future feature |
| L10 (Gov lifecycle) | Depends on G5-02 | Tracked | |
| L11 (Agent directory) | NOT in code audit | Tracked | Feature |
| L12 (Social recovery) | NOT in code audit | Tracked | Feature |
| L13 (Reputation decay) | NOT in code audit | Tracked | Feature |
| L14 (Open source repo) | J2-03 | Merged | CI/CD gap |
| L15 (Production deploy) | NOT in code audit | Tracked | Infra |

### New Findings NOT in FINISH_LINE_PLAN.md

The following are **net-new findings** from this code audit that were NOT captured in any previous audit document:

| ID | Severity | Description |
|----|----------|-------------|
| A2-01 | HIGH | Non-deterministic timestamps in block production |
| A3-01 | HIGH | Transaction `data` field not included in signature hash |
| A4-01 | HIGH | No atomic state commits — crash = inconsistent state |
| A5-01 | HIGH | Predictable leader selection |
| A5-02 | HIGH | No fork choice rule |
| A11-01 | CRITICAL | EVM gasPrice/estimateGas returns fee² |
| A11-02 | CRITICAL | EVM getLogs uses SHA-256 instead of Keccak-256 |
| B1-01 | CRITICAL | Prediction market bypasses consensus |
| B4-01 | CRITICAL | FROST threshold signer not wired |
| C1-01 | CRITICAL | TLS completely disabled in P2P |
| D2-01 | HIGH | DEX orders bypass consensus |
| D5-01 | CRITICAL | Prediction RPC bypasses consensus |
| F1-01 | HIGH | Unsandboxed compiler execution |
| F2-01 | CRITICAL | Single master custody seed |
| F2-02 | CRITICAL | Non-atomic custody operations |
| H1-01 | CRITICAL | Private key in Keypair.toString() | **FIXED** |
| H6-01 | CRITICAL | Fake addresses generated when wallet missing |
| I2-01 | CRITICAL | BIP39 uses SHA-512 not PBKDF2 |
| I8-01 | HIGH | Unauthenticated admin kill switch |
| J1-01 | CRITICAL | Wrong custody RPC port |
| J1-02 | CRITICAL | Docker port mismatch |
| J2-01 | CRITICAL | Placeholder validator keys in genesis |
| J2-02 | CRITICAL | Deployer keypair as contract admin |
| L2-01 | CRITICAL | Wire format incompatibility across SDKs |
| L3-01 | CRITICAL | Consensus bypass in multiple RPC endpoints |
| L4-01 | CRITICAL | No atomic state transitions |

**Total net-new CRITICAL findings: 16**  
**Total net-new HIGH findings: 10**

---

## EXECUTION ORDER & DEPENDENCY GRAPH {#execution-order}

### Phase 0: Fix Launch-Fatal Issues (Day 1 — BEFORE anything else)

```
Priority: Items that prevent the chain from functioning at all

1. L1-01  Fix cross_contract_call (THE systemic blocker)
   ├── A7-01  Implement in processor.rs
   └── A9-01  Implement call_token_transfer host function
   
2. L2-01  Standardize wire format across all SDKs
   ├── H1-02  Fix JS SDK serialization
   ├── H2-01  Fix Python SDK serialization
   └── H3-01  Fix Rust SDK serialization

3. L3-01  Route ALL state mutations through consensus
   ├── B1-01  Fix prediction market consensus bypass
   ├── D2-01  Fix DEX consensus bypass
   ├── D4-01  Fix launchpad consensus bypass
   └── D5-01  Fix prediction RPC consensus bypass

4. L4-01  Implement atomic state transitions
   └── A4-01  Add batch-commit/WAL to state.rs
```

### Phase 1: Security-Critical Fixes (Day 1-2)

```
5. Caller verification sweep (7 contracts, ~3 hours):
   ├── G1-01  moltcoin approve()
   ├── G1-02  moltcoin mint()
   ├── G7-01  dex_rewards initialize()
   ├── G10-01 moltauction create_auction()
   ├── G13-01 moltdao cancel_proposal()
   ├── G15-01 moltoracle submit_price()
   └── G26-01 compute_market 5 admin functions

6. C1-01   Fix P2P TLS — enable certificate validation
7. I2-01   Fix BIP39 key derivation (SHA-512 → PBKDF2)
8. I2-02   Encrypt wallet keys in localStorage
9. H6-01   Remove fake address generation
10. H1-01  Remove private key from toString()
```

### Phase 2: Core Chain Fixes (Day 2-3)

```
11. G19-01, G20-01  Fix wrapped token WASMs (add #[no_mangle])
12. G3-01           Fix AMM tick pricing (linear → exponential)
13. B1-02           Genesis contract initialization
14. A12-01          Align genesis distribution
15. A2-01           Deterministic block timestamps
16. A3-01           Include data field in tx signature hash
17. A11-01, A11-02  Fix EVM compatibility (gas, keccak)
18. A5-03           Align slashing parameters
```

### Phase 3: Contract Financial Wiring (Day 3-5, depends on Phase 0)

```
After cross_contract_call works:
19. G7-02   dex_rewards — wire actual MOLT transfers
20. G8-01   dex_router — remove simulation fallback
21. G9-01   lobsterlend — wire token transfers
22. G11-01  moltbridge — wire token locking
23. G17-02  moltswap — wire swap transfers
24. G21-01  prediction_market — wire mUSD transfers
25. G22-01  bountyboard — wire payment transfers
26. G23-01  clawpay — wire stream refunds
27. G24-01  clawpump — implement graduation
28. G25-02  clawvault — connect to real yield
29. G27-02  reef_storage — wire storage payments
```

### Phase 4: Infrastructure & DevOps (Day 3-4, parallel)

```
30. J1-01, J1-02  Fix Docker port mappings
31. J1-03         Remove || true from builds
32. J2-01         Replace placeholder validator keys
33. J2-02         Switch to multisig admin
34. J2-03         Create CI/CD pipeline
35. J1-04         Add Docker health checks
36. J3-01         Configure alertmanager
```

### Phase 5: Reliability & Quality (Day 4-6)

```
37. A5-01   Improve leader selection (add randomness) ✅ mix parent_hash into SHA-256 seed
38. A5-02   Implement fork choice rule ✅ wired ForkChoice cumulative weight into validator
39. C2-01   Add gossip deduplication ✅ bounded SeenMessageCache (20K, SHA-256 hash)
40. D1-01   Restrict CORS origins ✅ configurable via MOLTCHAIN_CORS_ORIGINS env var
41. D3-01   Fix async mutex usage ✅ verified: all guards scoped before .await (D6-01 also clear)
42. F2-01   Per-chain HD derivation for custody ✅ BIP-44 coin types (501/60/0)
43. G6-01   Implement collateral locking ✅ add_margin/remove_margin lock/unlock + open_position checks result
44. G6-02   Implement funding rates ✅ apply_funding crank, set_index_price, per-pair interval tracking
45. All overflow fixes (L6-01) ✅ 24 locations across 8 crates: u128 intermediates for HIGH/MEDIUM, saturating_add for all accumulators
46. All reentrancy guards (G23-02, G18-02) ✅ clawpay already complete; moltyid: 42 functions guarded + bid_name_auction CEI fix
```

### Phase 6: Frontend Consolidation (Day 5-7)

```
47. L5-01   Create shared utility module ✅ shared/utils.js with 20+ functions, imported in 28 HTML files
48. I1-01   Deduplicate explorer code ✅ escapeHtml/formatHash/rpcCall/bs58/formatTimeFull consolidated in shared/utils.js
49. I4-01   Remove key storage from DEX
50. I5-01   Wire marketplace to live data
51. I8-01   Add monitoring authentication
```

### Phase 7: Testing (Day 6-8, parallel with Phase 6)

```
52. K2-01   Write contract unit tests
53. K1-01   Test parallel processing
54. K1-02   Test fork handling
55. K3-02   Multi-validator E2E test
56. K4-02   Cross-SDK compatibility test
57. K3-03   Full DEX trading E2E test
```

### Phase 8: Feature Completion (Day 7-10, as time allows)

```
58. G5-02   Governance execution (H2)
59. G6-03   Oracle fallback handling (H8)
60. H7 params   Tokenomics parameter adjustment
61. G2-04   Wire Post-Only/Reduce-Only flags
62. M16     MoltyID UI integration
63. M17     Prediction market full implementation
```

---

## PROGRESS DASHBOARD {#progress-dashboard}

### Overall Progress

| Section | Total Items | Critical | High | Medium | Low | Done |
|---------|------------|----------|------|--------|-----|------|
| A. Core Chain | 37 | 5 | 12 | 15 | 5 | 2 |
| B. Validator | 9 | 3 | 3 | 3 | 0 | 1 |
| C. P2P | 8 | 1 | 3 | 3 | 1 | 0 |
| D. RPC | 13 | 1 | 4 | 6 | 2 | 0 |
| E. CLI | 8 | 0 | 1 | 4 | 3 | 0 |
| F. Compiler/Custody/Faucet | 9 | 2 | 3 | 3 | 1 | 0 |
| G. Contracts (27) | 68 | 9 | 28 | 22 | 9 | 0 |
| H. SDK | 19 | 2 | 8 | 7 | 2 | 0 |
| I. Frontends | 28 | 3 | 8 | 12 | 5 | 0 |
| J. Scripts/Deploy/Infra | 16 | 4 | 5 | 5 | 2 | 0 |
| K. Tests | 14 | 1 | 7 | 5 | 1 | 0 |
| L. Cross-cutting | 7 | 4 | 1 | 2 | 0 | 3 |
| **TOTAL** | **236** | **35** | **83** | **87** | **31** | **6** |

### Severity Summary

- **CRITICAL (35):** Must fix before launch — chain won't work or has exploitable vulnerabilities
- **HIGH (83):** Should fix before launch — will cause significant issues in production
- **MEDIUM (87):** Fix soon after launch — quality, reliability, technical debt
- **LOW (31):** Fix when convenient — polish, naming, minor improvements

### Completion Tracking

```
Phase 0 (Fatal):     [x] [x] [x] [x]                    4/4
Phase 1 (Security):  [x] [x] [x] [x] [x] [x]            6/6  ✅ COMPLETE
Phase 2 (Core):      [x] [x] [x] [x] [x] [x] [x] [x]    8/8  ✅ COMPLETE
Phase 3 (Contracts): [x] [x] [x] [x] [x] [x] [x] [x] [x] [x] [x]  11/11 ✅ COMPLETE
Phase 4 (Infra):     [x] [x] [x] [x] [x] [x] [x]        7/7 ✅ COMPLETE
Phase 5 (Quality):   [x] [x] [x] [x] [x] [x] [x] [x] [ ] [ ]  8/10
Phase 6 (Frontend):  [ ] [ ] [ ] [ ] [ ]                0/5
Phase 7 (Testing):   [ ] [ ] [ ] [ ] [ ] [ ]            0/6
Phase 8 (Features):  [ ] [ ] [ ] [ ] [ ] [ ]            0/6
                                              TOTAL:    44/63 phases
```

---

*This document was generated from a line-by-line code audit of every source file in the moltchain repository. It supersedes and incorporates all findings from: FINISH_LINE_PLAN.md, SECURITY_AUDIT_REPORT.md, PRODUCTION_AUDIT_ALL_CONTRACTS.md, DEX_COMPLETION_MILESTONE.md, CUSTODY_AUDIT_REPORT.md, ABI_CONFORMANCE_AUDIT.md, NEW_FINDINGS_AUDIT.md, PRODUCTION_READINESS_AUDIT.md, DEX_ARCHITECTURE_AUDIT.md, and all other audit documents in the repository.*

### Completion Notes

| Phase | Task | Commit | Date | Notes |
|-------|------|--------|------|-------|
| 0.1 | L1-01 / A7-01 / A9-01 | 0f0fd6b | Feb 20 | Full re-entrant cross-contract call. Replaced stub `host_cross_contract_call` with ~180-line implementation: loads target WASM, invokes function, propagates state changes atomically. `call_token_transfer` now debits/credits in StateStore. Added CCC constants (MAX_DEPTH=8, MAX_COMPUTE=5000). `processor.rs` applies cross_call_changes after execution. 7 new tests (unit + integration with WAT contracts). 279 total tests, 0 failures, 0 regressions. |
| 0.2 | L2-01 / H1-02 / H2-01 / H3-01 | eabd791 | Feb 20 | Fixed Transaction signature serialization: `serialize_signatures`/`deserialize_signatures` now use `is_human_readable()` to branch — bincode writes raw `Vec<[u8; 64]>` (matching JS/Python SDK manual encoders), JSON keeps hex strings. Added `Sig64Ser`/`Sig64De` helper types since serde doesn't impl Serialize for `[T; 64]`. 11 new wire format tests. 290 total tests, 0 failures, 0 regressions. |
| 0.3 | L3-01 / B1-01 / D5-01 | e977a44 | Feb 20 | Added `require_single_validator` guards to 4 unguarded state-mutating RPC endpoints: `post_create` (prediction.rs), `handle_set_fee_config`, `handle_set_rent_params`, `handle_request_airdrop`. D2-01 already fixed (DEX POST stubs). D4-01 non-issue (GET only). 290 total tests, 0 regressions. |
| 0.4 | L4-01 / A4-01 | 5e6b522 | Feb 20 | Implemented atomic state transitions. Added `atomic_put_accounts()` (N accounts + optional burn in single WriteBatch) and `atomic_put_account_with_reefstake()` to StateStore. Refactored 5 non-atomic code paths: (1) `charge_fee_direct` in processor.rs — payer debit + burn + treasury credit now atomic, (2) block reward distribution — treasury debit + producer credit now atomic, (3) ReefStake reward — treasury debit + pool update now atomic, (4) block tx reversal — all account reversals collected in HashMap overlay then flushed atomically, (5) checkpoint restoration — all restored accounts batched. Phase 0 now 4/4 COMPLETE. 9 new tests, 421 total tests, 0 regressions. |
| 1.5 | G1-01 / G1-02 / G7-01 / G10-01 / G13-01 / G15-01 / G26-01 | cf56d11 | Feb 20 | Caller verification sweep across 7 contracts (moltcoin, dex_rewards, moltauction, moltdao, moltoracle, compute_market). All 7 findings already fixed in prior sessions with `AUDIT-FIX` annotations: each vulnerable function now calls `get_caller()` and compares against parameter-supplied identity before proceeding. moltcoin: approve() + mint(), dex_rewards: initialize(), moltauction: create_auction(), moltdao: cancel_proposal(), moltoracle: submit_price(), compute_market: 5 admin fns (set_claim_timeout, set_complete_timeout, set_challenge_period, add_arbitrator, remove_arbitrator) all use get_caller() + is_admin(). Added 8 source-level regression tests in core/tests/caller_verification.rs verifying get_caller() patterns exist in all 7 contract source files. 429 total tests, 0 regressions. |
| 1.6 | C1-01 | 29b30c2 | Feb 20 | Replaced SkipServerVerification with proper TLS certificate validation. (1) Persistent node identity: cert+key saved to ~/.moltchain/node_cert.der + node_key.der, reused across restarts (NodeIdentity struct). (2) X.509 self-signature verification: verify_self_signed_cert() uses x509-parser + ring to parse and cryptographically verify certificate self-signatures — replaces the old DER-tag-only checks. (3) TOFU fingerprint pinning: PeerFingerprintStore tracks SHA-256 cert fingerprints per peer in ~/.moltchain/peer_fingerprints.json — new peers registered, known peers verified, changed fingerprints rejected with connection close. Applied to both outbound (connect_peer) and inbound (start_accepting) paths. (4) Mutual TLS: server now uses MoltClientCertVerifier (with_client_cert_verifier), clients present their node certificate via with_client_auth_cert. client_auth_mandatory=false for backwards compat. Added sha2 + x509-parser deps to p2p/Cargo.toml. 14 new tests, 443 total, 0 regressions. |
| 1.7 | I2-01 | a8f3f40 | Feb 20 | Fixed BIP39 key derivation: replaced SHA-512 single-hash with PBKDF2-HMAC-SHA512 (2048 iterations) per BIP39 spec in both wallet/js/crypto.js (MoltCrypto.mnemonicToKeypair) and wallet/extension/src/core/crypto-service.js (mnemonicToKeypair). Uses Web Crypto API: crypto.subtle.importKey('raw') + crypto.subtle.deriveBits({name: 'PBKDF2', salt: 'mnemonic'+passphrase, iterations: 2048, hash: 'SHA-512'}). Added passphrase parameter support (BIP39 "25th word"). NFKD Unicode normalization applied. Verified against BIP39 test vector: "abandon"x11+"about" → seed prefix 5eb00bbd... matches spec. 3 new JS tests (test vector, deterministic, passphrase). 443 Rust + 44 JS tests, 0 regressions. |
| 1.8 | I2-02 | f99cc70 | Feb 20 | Wallet key encryption already fully implemented: encryptPrivateKey() uses AES-256-GCM with PBKDF2 (100k iterations, SHA-256, CSPRNG salt+IV). All 9 key storage paths in wallet.js call encryptPrivateKey() before localStorage persistence. No plaintext secret material in localStorage — verified by source-level regex check (no wallet.privateKey= or wallet.seed= assignments). Extension uses chrome.storage.local with identical encryption. Added AUDIT-FIX I2-02 annotation to encryptPrivateKey(). 2 new regression tests. 443 Rust + 46 JS tests, 0 regressions. |
| 1.9 | H6-01 | 8743bfc | Feb 20 | Removed fake address generation from shared/wallet-connect.js. The _createRpcWallet fallback previously generated random bytes encoded as base58 when both RPC and nacl were unavailable — producing addresses with no private key (funds permanently lost). Replaced with: this.address = null + throw new Error with clear message prompting user to install the MoltChain wallet extension + console.error + window.alert. 1 new regression test verifying the old fake-address pattern is gone. 443 Rust + 47 JS tests, 0 regressions. |
| 1.10 | H1-01 | 7d3d002 | Feb 20 | Protected private key from accidental exposure in SDK Keypair class (sdk/js/src/keypair.ts). (1) Changed `readonly secretKey` to `private readonly _secretKey` — field is no longer publicly accessible. (2) Added `getSecretKey(): Uint8Array` with JSDoc warning about key exposure — explicit opt-in replaces implicit access. (3) Added `toString()` returning `Keypair(publicKey: <hex>)` — never reveals secret key in logs or string coercion. (4) Added `toJSON()` returning `{ publicKey: <hex> }` — prevents JSON.stringify leakage. (5) Added `[Symbol.for('nodejs.util.inspect.custom')]` returning toString() — Node.js console.log safety. (6) Rebuilt TypeScript SDK (`npx tsc`). No external consumers break — all 7 internal `secretKey` references are to nacl's raw keypair, not the SDK class; SDK consumers use `sign()` method. 5 new regression tests: toString exclusion, toJSON exclusion, getSecretKey validity, sign functionality, secretKey field inaccessibility. 443 Rust + 52 JS tests, 0 regressions. Phase 1 COMPLETE (6/6). |
| 2.11 | G19-01 / G20-01 | 1c6e3fc | Feb 20 | Wrapped token WASM export annotations: all three wrapped token contracts (musd_token, weth_token, wsol_token) already have correct `#[no_mangle] pub extern "C"` on all 20 exported functions each — verified by grep count (20/20 in every contract) and Cargo.toml `crate-type = ["cdylib", "rlib"]`. Pattern matches reference contract (moltcoin). 3 new source-level regression tests in core/tests/caller_verification.rs: verify each contract has all 8 required token functions, ≥8 `#[no_mangle]` annotations, and matching `pub extern "C"` count. 446 Rust + 52 JS tests, 0 regressions. |
| 2.16 | A3-01 | 9e96e0c | Feb 20 | Transaction data field in signature hash: already resolved by architecture change. The old flat-field Transaction model (sender, receiver, amount, fee, nonce with excluded `data`) no longer exists. Current Solana-style architecture: `Transaction { signatures, message: Message { instructions: Vec<Instruction>, recent_blockhash } }` where each Instruction contains `program_id`, `accounts`, `data`. `Message::serialize()` uses `bincode::serialize(self)` which includes ALL fields. Signing is over `message.serialize()` bytes, verification matches. 3 new regression tests: verify that changing data, program_id, or accounts each produce different message hashes. 457 Rust + 52 JS tests, 0 regressions. |
| 2.15 | A2-01 | 8412b28 | Feb 20 | Deterministic block timestamps. (1) Block production in validator/src/main.rs now uses `Block::new_with_timestamp()` with slot-derived timestamp `genesis_time + slot * slot_duration / 1000` instead of `Block::new()` which called `SystemTime::now()`. (2) Genesis time parsed from stored genesis block (slot 0) header timestamp at startup, with RFC 3339 string fallback. (3) Block reception validates incoming timestamps against expected slot time — rejects blocks with >60s drift. (4) Added `derive_slot_timestamp()` and `validate_timestamp()` as both standalone and associated functions on Block in core/src/block.rs for shared usage. 7 new tests: slot derivation (basic, deterministic, monotonicity), timestamp validation (within/outside window), new_with_timestamp usage. 454 Rust + 52 JS tests, 0 regressions. |
| 2.14 | A12-01 | 57064fc | Feb 20 | Genesis distribution alignment: genesis.rs and multisig.rs already agree — validator_rewards=150M (15%), builder_grants=250M (25%), community_treasury=400M (40%), founding_moltys=100M (10%), ecosystem_partnerships=50M (5%), reserve_pool=50M (5%). Total 1B. validator/src/main.rs REWARD_POOL_MOLT=150M also consistent. Finding description was inaccurate about which values were swapped. 1 new regression test: `a12_01_genesis_distribution_matches_multisig` verifies all 6 allocation amounts appear in both genesis.rs and multisig.rs and sum to 1B. 448 Rust + 52 JS tests, 0 regressions. |
| 2.13 | B1-02 | 8f882b7 | Feb 20 | Genesis contract initialization: all 4 genesis phases (deploy, initialize, create trading pairs, seed oracle) already existed in validator/src/main.rs. The only gap was bountyboard — skipped with comment "stateless bootstrap" but actually has `initialize()` → `set_identity_admin()` setting `identity_admin` key required by `verify_identity`, `update_reputation`, `issue_credential`. Without init, first-caller-wins vulnerability (G22-02). Fix: Added bountyboard InitSpec to `genesis_initialize_contracts()` Layer 5d. Also marks G22-02 as mitigated. 1 new source-level regression test: `b1_02_all_contracts_initialized_at_genesis` verifies all 27 contracts appear in genesis_initialize_contracts. 447 Rust + 52 JS tests, 0 regressions. |
| 2.17 | A11-01 / A11-02 | a081ebc | Feb 20 | Fixed two EVM compatibility bugs in rpc/src/lib.rs. (1) A11-01: `handle_eth_gas_price()` was returning `fee_config.base_fee` (1,000,000 shells) — MetaMask computes total = gasPrice × estimateGas, so returning base_fee for both meant fee² display. Fixed to return `"0x1"` (1 shell per gas unit); `eth_estimateGas` already returns actual fee as gas value, so 1 × fee = correct total. (2) A11-02: `eth_getLogs` topic hashing used `sha2::Sha256` instead of `sha3::Keccak256` — all EVM tooling (Ethers.js, web3.py, MetaMask) uses keccak256 for event topic matching. Changed to `sha3::Keccak256`; added `sha3 = "0.10"` to rpc/Cargo.toml. Verified keccak256("Transfer(address,address,uint256)") = `ddf252ad...` matches standard EVM topic hash. 3 new tests: source-level gasPrice returns "0x1", source-level getLogs uses Keccak256, keccak256 produces correct ERC-20 Transfer topic hash. 460 Rust + 52 JS tests, 0 regressions. |
| 2.18 | A5-03 | dce3614 | Feb 20 | Aligned slashing parameters between genesis.rs and consensus.rs. (1) Replaced dead flat `slashing_percentage_downtime: 5` field in ConsensusParams with graduated fields: `slashing_downtime_per_100_missed: 1` (1% per 100 missed slots) and `slashing_downtime_max_percent: 10` (cap at 10%), matching the actual runtime logic in consensus.rs. (2) Added `Default` impl for `ConsensusParams`. (3) Created `apply_economic_slashing_with_params()` in consensus.rs that reads all slash percentages from ConsensusParams instead of hardcoding: double_sign (50%), downtime (graduated 1%/100 max 10%), invalid_state (100%). Original `apply_economic_slashing()` preserved as backward-compat wrapper using defaults. (4) Wired validator/src/main.rs to call `apply_economic_slashing_with_params(&genesis_config.consensus)`. (5) Updated scripts/generate-genesis.sh JSON template. 3 new regression tests: source-level checks (no flat field, params used), graduated math (300 missed=3%, 2000 missed capped at 10%). Phase 2 COMPLETE (8/8). 463 Rust + 52 JS tests, 0 regressions. |
| 2.20 | G8-01 | 67a8e28 | Feb 20 | Already fixed in Phase 2 commit 67a8e28. Simulation fallback in execute_clob_swap, execute_amm_swap, execute_legacy_swap was replaced with hard failure (return 0) when cross-contract call fails. Explicit regression test test_swap_no_simulation_fallback already exists. No additional code changes needed. |
| 2.21 | G9-01 | ff6959c | Feb 20 | Wired all 8 core lobsterlend operations to actual token transfers. Incoming transfers (deposit, repay, liquidation payment, flash_repay): verified via get_value() — user must attach sufficient shells. Outgoing transfers (withdraw, borrow, flash_borrow, withdraw_reserves): self-custody pattern via call_token_transfer(molt, self, recipient) with bookkeeping revert on failure. Added set_moltcoin_address() admin function, transfer_out() helper, load_molt_addr()/is_zero_addr() helpers. Error 30=moltcoin not configured or insufficient value, error 31=transfer failed. 45 tests (up from 33) — 12 new: deposit/repay/liquidate/flash_repay insufficient value, withdraw/borrow/flash_borrow/withdraw_reserves without molt configured, set_moltcoin_address (happy+non-admin+zero), self-custody pattern verification. 465 Rust + 52 JS tests, 0 regressions. |
| 2.19 | G7-02 | e81f957 | Feb 20 | Wired real token transfers in dex_rewards using self-custody pattern. Three issues fixed: (1) MOLTCOIN_ADDRESS_KEY/REWARDS_POOL_KEY were never initialized so claims silently skipped all transfers. (2) Even if configured, call_token_transfer(molt, pool, trader) would fail because moltcoin enforces caller==from and CCC caller is the dex_rewards contract not the pool. (3) Referral earnings accumulated via record_trade but had no claim function. Fix: Added get_contract_address() to SDK+runtime (new host function host_get_contract_address reads ctx.contract); dex_rewards now uses self as from address so caller==from is always satisfied in CCC. Claims fail with error 5 when moltcoin unconfigured instead of silent success. Added claim_referral_rewards (op 19). 38 contract tests (up from 34), 2 regression tests in caller_verification.rs. 465 Rust + 52 JS tests, 0 regressions. |
| 3.22 | G11-01 | 301c9d6 | Feb 20 | Wired all 5 moltbridge financial functions to actual token transfers. lock_tokens: get_value() verification for incoming shells. submit_mint/confirm_mint: transfer_out() to recipient on threshold completion, with CANCELLED/PENDING revert on failure. submit_unlock/confirm_unlock: transfer_out() to recipient with locked amount revert on failure. Added set_token_address() admin function (owner-only, zero rejected), load_molt_addr()/transfer_out() helpers. Also fixed all 43 pre-existing test failures (missing set_caller calls from prior caller verification phase). Added setup() with MOLT_ADDR+CONTRACT_ADDR, setup_no_molt(). 7 new tests: lock_insufficient_value, mint_without_molt, unlock_without_molt, set_token_address (happy+non-owner+zero), self_custody_transfer_pattern. 50 moltbridge tests (up from 43), 465 workspace tests, 0 regressions. |
| 3.23 | G17-02 | be9d5b7 | Feb 20 | Wired all moltswap AMM financial operations to actual token transfers. add_liquidity: get_value() >= amount_a + amount_b check for incoming deposits. remove_liquidity: transfer_out() for both token A and B to provider via pool's self-custody. swap_a_for_b/swap_b_for_a: get_value() for incoming + transfer_out() for output token to swapper. flash_loan_borrow: transfer_out() to borrower. flash_loan_repay: get_value() >= repay_amount check. Pool's token_a/token_b addresses used directly as token contract targets (no separate MOLTCOIN_ADDRESS_KEY needed). 8 new tests: add_liquidity_insufficient_value, swap_a/b_insufficient_value, remove_liquidity_transfers_out, flash_loan_borrow_transfers, flash_loan_repay_insufficient_value, flash_loan_borrow_repay_cycle, self_custody_transfer_pattern. 30 moltswap tests (up from 22), 465 workspace tests, 0 regressions. |
| 3.24 | G21-01 | f5aafdd | Feb 20 | Wired all 11 prediction_market financial functions to actual mUSD transfers. Inbound (7 functions): get_value() checks for create_market (MARKET_CREATION_FEE=10M), add_initial_liquidity, add_liquidity, buy_shares, mint_complete_set, submit_resolution (DISPUTE_BOND=100M), challenge_resolution (DISPUTE_BOND). Outbound (4 functions): transfer_musd_out() for sell_shares, redeem_complete_set, withdraw_liquidity, finalize_resolution (resolver reward). Uses pre-existing transfer_musd_out() helper with graceful degradation. Updated all test helpers across 4 test files (lib.rs unit tests, core_tests.rs, adversarial_tests.rs, resolution_tests.rs) with set_value(). 7 new G21-01 tests: create_market_insufficient_fee, buy_shares_insufficient_value, mint_complete_set_insufficient_value, submit_resolution_insufficient_bond, sell_shares_transfers_musd_out, withdraw_liquidity_transfers_musd_out, add_liquidity_insufficient_value. 208 prediction_market tests (up from 197), 465 workspace tests, 0 regressions. |
| 3.25 | G22-01 | 27d4770 | Feb 20 | Wired bountyboard payment transfers with 3 bug fixes. (1) create_bounty: Added get_value() >= reward_amount inbound check (return 11 on insufficient value) — bounties were previously backed by nothing. (2) approve_work: Changed transfer from creator→worker to contract→worker using get_contract_address() (self-custody pattern) — previously assumed creator pre-approved the contract. (3) cancel_bounty: Fixed wrong storage key (b"bb_token_address" → TOKEN_ADDRESS_KEY b"bounty_token_addr"), replaced get_caller() with get_contract_address() for source address, and added proper transfer failure handling with revert (was `let _ = call_token_transfer`). Added get_value + get_contract_address imports. 5 new tests. 21 bountyboard tests (up from 16), 465 workspace tests, 0 regressions. |
| 3.26 | G23-01 | (already fixed) | Feb 20 | Already fixed in prior session. cancel_stream fully wires call_token_transfer for both refund-to-sender (L631) and earned-to-recipient (L643). create_stream and create_stream_with_cliff wire inbound escrow lock. withdraw_from_stream wires outbound disburse. All 5 transfer points present. 32 clawpay tests pass, 0 failures. No code changes needed. |
| 3.27 | G24-01 | b2d3f23 | Feb 20 | Wired clawpump financial transfers + fixed graduation. 4 bugs: (1) No get_value() — create_token()/buy() trusted parameters instead of verifying payment. (2) No call_token_transfer — sell() never sent refund, withdraw_fees() never transferred. (3) No get_contract_address for self-custody. (4) Graduation without DEX addresses didn't set graduated flag → infinite retry. Added transfer_molt_out() helper with graceful degradation, get_value() checks on create_token+buy, call_token_transfer on sell+withdraw_fees with state revert, set_molt_token() admin fn, graduation flag fix. 6 new G24-01 tests. 44 clawpump tests (up from 38), 465 workspace tests, 0 regressions. |
| 2.12 | G3-01 | 36d9084 | Feb 20 | Replaced linear tick approximation with correct exponential formula in contracts/dex_amm/src/lib.rs. (1) Precomputed 19 Q64.64 constants for 1.00005^(2^k) with 80-decimal-digit precision. (2) Implemented `mul_q64()` — 256-bit intermediate multiplication via hi/lo u64 decomposition with carry tracking and wrapping arithmetic. (3) New `tick_to_sqrt_price()` uses bit-decomposition of |tick|, multiplying accumulator by precomputed constants for each set bit; negative ticks use reciprocal. (4) New `sqrt_price_to_tick()` uses binary search for exact inversion. (5) Adjusted MAX_TICK/MIN_TICK from ±887,272 (Uniswap V3 uint160) to ±443,636 (matching u64 Q32.32 representable range). Verified against 80-digit precision reference values: tick 0,±1,±100,±600,±10000,±100000 all within ±1 ULP. 8 new tests: exponential accuracy (9 test vectors), large values, monotonicity (2001 ticks), roundtrip range, mul_q64 unit tests. Fixed 12 previously failing dex_amm tests that depended on correct pricing. 446 Rust + 52 JS tests, 0 regressions. |

*Last updated: February 20, 2026*
