# MoltChain: Finish These First
## Priority List of STARTED-BUT-INCOMPLETE Work

**For:** John (frustrated with incomplete work)  
**Date:** February 8, 2026  
**Focus:** Features that are 50-95% done - **FINISH BEFORE STARTING ANYTHING NEW**

---

## 🎯 Quick Wins (95%+ Complete - 1-2 Days Each)

### 1. RPC Ethereum Integration (95% → 100%)
**Current State:** Stub handlers exist, return mock data  
**What's Left:**
- Parse Ethereum RLP transactions in `eth_sendRawTransaction`
- Convert Ethereum transaction to MoltChain format
- Extract sender from signature
- Register EVM→Native address mapping

**Files:** `moltchain/rpc/src/lib.rs` lines 850-900  
**Effort:** 2 days  
**Impact:** MetaMask can send transactions  

**Code locations:**
```rust
// Line 850: handle_eth_send_raw_transaction
// TODO: Parse Ethereum RLP-encoded transaction
// TODO: Extract sender (recover from signature)
// TODO: Convert to MoltChain transaction format
```

### 2. Explorer Transaction History (95% → 100%)
**Current State:** Dashboard works, detail pages work, history is mock  
**What's Left:**
- Add account→transaction index in RocksDB
- Implement `getTransactionHistory` RPC method
- Add pagination to frontend

**Files:** 
- `moltchain/rpc/src/lib.rs` lines 720-750 (stub)
- `moltchain/explorer/js/explorer.js` (frontend)

**Effort:** 1 day  
**Impact:** Real transaction history in explorer  

### 3. Python SDK Documentation (95% → 100%)
**Current State:** All methods work, missing examples  
**What's Left:**
- Add contract interaction examples
- Document all RPC methods
- Create README with quickstart

**Files:** `moltchain/sdk/python/README.md` (create)  
**Effort:** 1 day  
**Impact:** Agents can use SDK easily  

### 4. Wallet Hardware Support (98% → 100%)
**Current State:** UI complete, Ledger integration stubbed  
**What's Left:**
- Integrate @ledgerhq/hw-transport-webusb
- Add device connection flow
- Test transaction signing

**Files:** `moltchain/wallet/js/wallet.js` lines 200-250  
**Effort:** 1 day  
**Impact:** Production wallet security  

---

## 🔥 High Priority (70-95% Complete - 1 Week Each)

### 5. Smart Contract Host Functions (70% → 100%)
**Current State:** WASM runtime works, only 4 host functions  
**What's Missing:**
- Token transfer functions (spl_token::transfer equivalent)
- Cross-contract calls (invoke/invoke_signed)
- Event emission (emit_log)
- Account queries (get_account_data)
- Cryptographic primitives (keccak256, secp256k1)

**Files:** `moltchain/core/src/contract.rs` lines 200-300  
**Effort:** 1 week  
**Impact:** Real dApps can be built  

**Required host functions:**
```rust
// Token operations
fn transfer_tokens(from, to, amount) -> Result<()>
fn mint_tokens(mint, to, amount) -> Result<()>
fn burn_tokens(mint, from, amount) -> Result<()>

// Cross-contract calls
fn invoke(program_id, accounts, data) -> Result<()>
fn invoke_signed(program_id, accounts, data, signer_seeds) -> Result<()>

// Events
fn emit_log(message: String) -> Result<()>
fn emit_event(event_data: Vec<u8>) -> Result<()>

// Account operations
fn get_account_data(pubkey) -> Result<Vec<u8>>
fn create_account(pubkey, space, owner) -> Result<()>

// Crypto primitives
fn keccak256(data: &[u8]) -> [u8; 32]
fn secp256k1_verify(message, signature, pubkey) -> bool
fn ed25519_verify(message, signature, pubkey) -> bool
```

### 6. JavaScript/Rust SDKs (50% → 90%)
**Current State:** Files exist but untested  
**What's Missing:**
- Test all RPC methods
- Add transaction building examples
- Document API surface
- Add WebSocket subscriptions
- Create comprehensive examples

**Files:**
- `moltchain/sdk/js/src/*.ts` (JavaScript)
- `moltchain/sdk/rust/src/*.rs` (Rust)

**Effort:** 5 days  
**Impact:** Broader developer adoption  

**Testing checklist:**
- [ ] Connection to RPC
- [ ] Account queries (getBalance, getAccount)
- [ ] Transaction building
- [ ] Transaction signing
- [ ] Transaction submission
- [ ] Block queries
- [ ] WebSocket subscriptions
- [ ] Error handling

### 7. Consensus Byzantine Testing (75% → 95%)
**Current State:** BFT consensus works with honest validators  
**What's Missing:**
- Test with 33% Byzantine validators (malicious)
- Test fork resolution with competing chains
- Test validator downtime scenarios
- Test double-vote slashing
- Performance benchmarks under attack

**Files:** 
- `moltchain/core/src/consensus.rs` (existing code)
- `moltchain/tests/consensus_byzantine.rs` (create)

**Effort:** 1 week  
**Impact:** Confidence in production safety  

**Test scenarios needed:**
```rust
// Byzantine fault tests
#[test] fn test_33_percent_malicious_validators()
#[test] fn test_fork_resolution_competing_chains()
#[test] fn test_validator_downtime_handling()
#[test] fn test_double_vote_slashing()
#[test] fn test_double_block_slashing()
#[test] fn test_invalid_signature_rejection()
#[test] fn test_finality_under_attack()
```

### 8. Networking Optimization (60% → 85%)
**Current State:** TCP gossip works but unoptimized  
**What's Missing:**
- Message compression (gzip/snappy)
- Deduplication (message hash cache)
- Bandwidth limits (rate limiting)
- Connection pooling
- Retry logic with exponential backoff

**Files:** `moltchain/p2p/src/network.rs` lines 150-300  
**Effort:** 1 week  
**Impact:** Better performance at scale  

**Optimizations needed:**
```rust
// Compression
fn compress_message(msg: &P2PMessage) -> Vec<u8> {
    // Use snappy for speed
}

// Deduplication
struct MessageCache {
    seen: HashMap<Hash, Instant>,
    max_age: Duration,
}

// Bandwidth limiting
struct BandwidthLimiter {
    bytes_per_second: usize,
    current_window: usize,
}

// Connection pooling
struct ConnectionPool {
    active: HashMap<PeerId, Connection>,
    max_connections: usize,
}
```

---

## 🎯 Medium Priority (50-70% Complete - 1-2 Weeks Each)

### 9. Validator Sync Optimization (70% → 90%)
**Current State:** Block range sync works but slow  
**What's Missing:**
- Checkpoint sync (skip to recent state)
- Snapshot support (download compressed state)
- Parallel block downloads
- Resume after disconnect

**Files:** `moltchain/validator/src/sync.rs` lines 100-300  
**Effort:** 1 week  
**Impact:** Validators sync in minutes not hours  

### 10. Testing Infrastructure (20% → 60%)
**Current State:** Basic unit tests  
**What's Missing:**
- Integration tests for multi-validator scenarios
- E2E tests for full transaction flow
- Performance benchmarks
- Fuzz testing for consensus

**Files:** 
- `moltchain/tests/integration/` (create)
- `moltchain/tests/e2e/` (create)
- `moltchain/benches/` (create)

**Effort:** 2 weeks  
**Impact:** Confidence for mainnet launch  

**Test coverage needed:**
```
Unit tests:        ✅ 70% (exists)
Integration tests: ❌ 10% (minimal)
E2E tests:         ❌ 0% (missing)
Benchmarks:        ❌ 5% (minimal)
Fuzz tests:        ❌ 0% (missing)

Target:
Unit tests:        90%
Integration tests: 70%
E2E tests:         50%
Benchmarks:        30%
Fuzz tests:        20%
```

### 11. Smart Contract Storage Optimization (65% → 85%)
**Current State:** Storage works but inefficient  
**What's Missing:**
- Storage rent (pay for persistent state)
- State compression
- Garbage collection
- Storage analytics

**Files:** `moltchain/core/src/contract.rs` lines 50-100  
**Effort:** 1 week  
**Impact:** Sustainable long-term storage  

---

## 📊 Summary: What to Finish First

### Week 1: Quick Wins (🎯 95%+ Done)
- Day 1-2: RPC Ethereum Integration
- Day 3: Explorer Transaction History  
- Day 4: Python SDK Documentation
- Day 5: Wallet Hardware Support

**Result:** Production-ready RPC + Explorer + SDK + Wallet

### Week 2-3: High Priority (🔥 70-95% Done)
- Week 2: Smart Contract Host Functions
- Week 2: JavaScript/Rust SDK Testing
- Week 3: Consensus Byzantine Testing
- Week 3: Networking Optimization

**Result:** Production-grade contracts + SDKs + consensus + networking

### Week 4-5: Medium Priority (🎯 50-70% Done)
- Week 4: Validator Sync Optimization
- Week 4-5: Testing Infrastructure
- Week 5: Storage Optimization

**Result:** Fast validator sync + comprehensive tests + efficient storage

---

## 🚫 What NOT to Do

### Stop Starting New Features

**DON'T START:**
- ❌ EVM runtime (0% done, needs 6 weeks)
- ❌ JavaScript runtime (0% done, needs 4 weeks)
- ❌ Python runtime (0% done, needs 4 weeks)
- ❌ The Reef storage (0% done, needs 8 weeks)
- ❌ Bridges (0% done, needs 12 weeks)
- ❌ New RPC methods
- ❌ New UI pages
- ❌ Code refactoring

**INSTEAD:**
- ✅ Finish the 11 items above
- ✅ Get to 100% on started features
- ✅ Test what exists
- ✅ Document what works

### The Problem

You have **40-50% of features at 50-95% complete**. This is the **most frustrating state** because:

1. Can't launch (missing critical pieces)
2. Can't demo (incomplete features)
3. Can't onboard devs (broken promises)
4. Can't validate (no user feedback)

### The Solution

**STOP STARTING. START FINISHING.**

1. **Freeze feature development** - No new work
2. **Pick 3-5 items above** - Focus until 100%
3. **Ship incrementally** - Release as soon as testnet-ready
4. **Get feedback** - Real users reveal priorities
5. **Iterate** - Build what matters

---

## 🎯 Recommended 3-Week Sprint

### Week 1: Frontend Polish (Quick Wins)
- RPC Ethereum integration
- Explorer transaction history
- Python SDK docs
- Wallet hardware support

**Outcome:** User-facing features complete

### Week 2: Smart Contracts & SDKs
- Contract host functions (token transfers, cross-contract calls)
- JavaScript SDK testing + docs
- Rust SDK testing + docs

**Outcome:** Developers can build real dApps

### Week 3: Consensus & Networking
- Byzantine fault testing
- Networking optimization
- Integration tests

**Outcome:** Production-grade core infrastructure

### Result After 3 Weeks
- ✅ All 95%+ features at 100%
- ✅ All 70-95% features at 95%+
- ✅ Testnet-ready blockchain
- ✅ Working demos
- ✅ Developer onboarding path

**THEN** you can:
1. Launch testnet
2. Get user feedback
3. Prioritize next features based on reality
4. Build token standard (3 weeks)
5. Build DeFi (6 weeks)
6. Add advanced features (JS/Python runtimes, bridges) in Phase 2

---

## 💡 Key Insight

**You're not missing 50% of features. You have 50% of features at 80% complete.**

The path forward is:
1. ✅ Finish what's started (3 weeks)
2. ✅ Launch testnet (1 week prep)
3. ✅ Get feedback (iterate)
4. ⏳ Build new features based on user needs

**NOT:**
1. ❌ Keep starting new features
2. ❌ Get to 60% on everything
3. ❌ Never ship
4. ❌ Die with 1000 half-finished features

---

## 📝 Action Items for John

### This Week
1. **Read this document** - Understand what's 50-95% done
2. **Pick 3-5 items** - What matters most for YOUR use case?
3. **Timebox 1 week per item** - Force completion
4. **Ship incrementally** - Don't wait for perfection

### This Month
1. **Complete Week 1-3 sprint** - All quick wins + high priority
2. **Launch testnet** - Even if missing features
3. **Get 10 users** - Agents or developers
4. **Collect feedback** - What do they need NEXT?

### Next 3 Months
1. **Build based on feedback** - Not roadmap
2. **Add 1 major feature per month** - Token standard, DEX, etc.
3. **Keep finishing what you start** - 100% or delete

---

**The Bottom Line**

MoltChain is **high-quality but half-built**. The foundations are **solid**. The path forward is **clear**. The discipline required is **brutal simplicity**:

> **Finish. Ship. Iterate. Repeat.**

Stop starting. Start finishing. Launch in 3 weeks.

🦞⚡

---

*For questions about specific items, see PRODUCTION_READINESS_AUDIT.md for full details.*
