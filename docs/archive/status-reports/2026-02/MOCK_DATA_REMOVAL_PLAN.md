# 🧹 MOCK DATA & STUB REMOVAL - COMPREHENSIVE PLAN

**Date:** February 8, 2026  
**Status:** In Progress  
**Goal:** Remove ALL mock data, stubs, placeholders, and TODO items

---

## 📋 INVENTORY OF ISSUES

###  Total Found: **52 instances**

### 🔴 CRITICAL - RPC Mock Data (12 instances)

| File | Line | Issue | Fix Required |
|------|------|-------|--------------|
| rpc/src/lib.rs | 570 | `total_contracts`: 7 (hardcoded) | Count from state |
| rpc/src/lib.rs | 581 | Mock peers data | Get from P2P layer |
| rpc/src/lib.rs | 615 | `peer_count`: 1 (mock) | Get from P2P layer |
| rpc/src/lib.rs | 665 | `commission_rate`: 0 (mock) | Get from validator config |
| rpc/src/lib.rs | 717 | `uptime`: 99.5 (mock) | Calculate from last_active_slot |
| rpc/src/lib.rs | 758 | `peer_count`: 1 (mock) | Get from P2P layer |
| rpc/src/lib.rs | 810 | Mock transaction signature | Return proper signature or error |
| rpc/src/lib.rs | 943 | Mock rewards data | Calculate from stake pool |
| rpc/src/lib.rs | 1083 | `deployed_at`: 0 (mock) | Get from contract metadata |
| rpc/src/lib.rs | 1465 | Placeholder tx signature | Generate proper sig or error |
| rpc/src/lib.rs | 1505 | Placeholder tx signature | Generate proper sig or error |
| rpc/src/lib.rs | 1634 | MockOracle for testnet | Acceptable for testnet |

### 🟡 MEDIUM - RPC Unimplemented Features (15 instances)

| File | Line | Issue | Status |
|------|------|-------|--------|
| rpc/src/lib.rs | 1202-1206 | EVM transaction parsing (5 TODOs) | Phase 2 - EVM compat |
| rpc/src/lib.rs | 1215 | EVM mock transaction hash | Phase 2 - EVM compat |
| rpc/src/lib.rs | 1225-1226 | EVM call execution (2 TODOs) | Phase 2 - EVM compat |
| rpc/src/lib.rs | 1236-1237 | EVM gas estimation (2 TODOs) | Phase 2 - EVM compat |
| rpc/src/lib.rs | 1287 | Ethereum receipt format | Phase 2 - EVM compat |
| rpc/src/lib.rs | 1334 | Transaction format conversion | Phase 2 - EVM compat |

### 🟢 LOW - P2P TODOs (6 instances)

| File | Line | Issue | Priority |
|------|------|-------|----------|
| p2p/src/gossip.rs | 94 | Track validator pubkeys | Medium |
| p2p/src/network.rs | 177 | Track validator pubkeys | Medium |
| p2p/src/network.rs | 197 | Load block from state | Medium |
| p2p/src/network.rs | 232 | Get status from validator | Low |
| p2p/src/network.rs | 258 | Forward to validator | Low |

### ⚪ DEFER - Consensus TODOs (3 instances)

| File | Line | Issue | Status |
|------|------|-------|--------|
| core/src/consensus.rs | 454, 471, 478 | Track individual delegations | Phase 2 Feature |

###  ACCEPTABLE - Testnet Mocks (2 instances)

| File | Line | Issue | Reason |
|----|------|-------|--------|
| rpc/src/lib.rs | 1634 | MockOracle for testnet | Legitimate testnet behavior |
| core/src/consensus.rs | 40-42 | MockOracle struct | Legitimate testnet behavior |

---

## 🎯 FIX PRIORITY ORDER

### Priority 1: RPC Mock Data (2-4 hours)
1. ✅ Total/active accounts tracking (DONE)
2. ⏳ Get peer_count from P2P layer
3. ⏳ Calculate validator uptime from slots
4. ⏳ Get commission_rate from validator state
5. ⏳ Count contracts from state (remove hardcoded 7)
6. ⏳ Fix mock transaction signatures
7. ⏳ Calculate real rewards data

### Priority 2: P2P Integration (4-6 hours)
1. Expose peer count from P2P network
2. Track validator pubkeys in gossip
3. Implement block loading from state
4. Forward messages to validator properly

### Priority 3: EVM Features (DEFER to Phase 2)
- All EVM-related TODOs are for future EVM compatibility
- Not blocking testnet/mainnet launch
- Can be added post-launch

---

## 🚀 IMPLEMENTATION PLAN

### Session 1: RPC Peer Count (NOW)
- Add `get_peer_count()` method to P2P network
- Wire it through to RPC state
- Replace all `peer_count: 1` mocks

### Session 2: Validator Metrics  
- Calculate uptime from `last_active_slot`
- Get commission_rate from storage
- Calculate rewards from stake pool

### Session 3: Contract Counting
- Add `count_contracts()` to state
- Replace hardcoded `7` with actual count

### Session 4: Transaction Signatures
- Replace placeholder signatures with proper hash generation
- Or return appropriate errors if operations not supported yet

---

## ✅ COMPLETED

1. ✅ Fixed validators.js duplicate MoltChainRPC declaration
2. ✅ Added active_accounts tracking to metrics
3. ✅ Updated RPC to include active_accounts field
4. ✅ Compiled and deployed validator with fixes

---

## 📊 PROGRESS TRACKER

- **Total Issues:** 52
- **Critical (RPC Mocks):** 12
- **Fixed:** 2
- **In Progress:** 0
- **Remaining:** 50
- **Deferred (Phase 2):** 15
- **Acceptable:** 2

**Effective Remaining:** 33 (excluding Phase 2 EVM features)

---

## 🎯 NEXT ACTIONS

1. Implement `get_peer_count()` in P2P layer
2. Wire peer count to RPC endpoints (3 locations)
3. Calculate validator uptime from slots
4. Get commission rate from state
5. Count contracts dynamically
6. Fix placeholder transaction signatures
7. Implement real rewards calculation

**Estimated Time to Complete Priority 1 & 2:** 6-10 hours
