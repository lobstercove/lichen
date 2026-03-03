# 🦞 CLOSE THE GAPS - Priority Action Plan ⚡

**Date:** February 8, 2026  
**Purpose:** Focus ONLY on finishing what's been started (50-95% done)  
**Philosophy:** Ship what we started before starting new things

---

## 🎯 THE PROBLEM

MoltChain has **too much partially-implemented work**. Instead of starting new features, we need to **COMPLETE what exists**.

This plan identifies everything that's **50-95% done** and provides a surgical path to **100% completion**.

---

## 📊 PARTIALLY-DONE INVENTORY

### 🔴 CRITICAL (Blocks Testnet)

| Feature | Status | Missing | Effort | Priority |
|---------|--------|---------|--------|----------|
| Fee Burn Mechanism | 80% | Add burn logic to processor | 2-4 hours | P0 |
| CLI Testing | 80% | Test all commands | 1-2 days | P0 |
| RPC Endpoint Verification | 75% | Test/fix all 24 endpoints | 2-3 days | P0 |

### 🟡 HIGH (Pre-Mainnet)

| Feature | Status | Missing | Effort | Priority |
|---------|--------|---------|--------|----------|
| Wallet Integration | 95% | Connect to real blockchain | 1-2 days | P1 |
| Marketplace Integration | 95% | Contract integration | 1-2 days | P1 |
| Programs UI Integration | 95% | Backend wiring | 1-2 days | P1 |
| WebSocket API | 50% | Real-time subscriptions | 2-3 days | P1 |

### 🟢 MEDIUM (Nice to Have)

| Feature | Status | Missing | Effort | Priority |
|---------|--------|---------|--------|----------|
| P2P Network Hardening | 90% | NAT traversal, peer reputation | 3-5 days | P2 |
| Transaction History Indexing | 70% | Full account history | 2-3 days | P2 |
| Contract Logs Storage | 60% | Persistent logs | 1-2 days | P2 |

---

## 🚀 EXECUTION PLAN

### Phase 1: CRITICAL GAPS (5-7 days)

Close all P0 items. Nothing else matters until these are done.

#### Day 1: Fee Burn Implementation ⚡

**Goal:** Add 50% fee burn to transaction processor

**Files to Edit:**
- `core/src/processor.rs`

**Implementation:**
```rust
// In core/src/processor.rs, in the execute_transaction function

pub fn execute_transaction(&mut self, tx: &Transaction) -> Result<(), String> {
    // ... existing code ...
    
    // Calculate fees
    let total_fee = calculate_transaction_fee(tx);
    
    // NEW: Burn 50% of fees
    let burn_amount = total_fee / 2;
    let validator_amount = total_fee - burn_amount;
    
    // Burn half (send to zero address or subtract from supply)
    self.state.burn_tokens(burn_amount)?;
    
    // Give half to validator
    self.state.credit_validator(validator_pubkey, validator_amount)?;
    
    // Track total burned
    self.state.increment_total_burned(burn_amount)?;
    
    // ... rest of code ...
}
```

**Testing:**
```bash
# 1. Start validator
cargo run --release --bin moltchain-validator

# 2. Send transaction
molt transfer <address> 1.0

# 3. Check total burned
molt burned

# 4. Verify: burned amount should be 50% of tx fees
```

**Success Criteria:**
- ✅ 50% of fees burned on every transaction
- ✅ `getTotalBurned` RPC returns correct value
- ✅ Economics match ECONOMICS.md spec

**Effort:** 2-4 hours

---

#### Day 2-3: CLI Comprehensive Testing 🔧

**Goal:** Test every single CLI command and fix stubs

**Process:**

1. **Compile CLI:**
```bash
cd /Users/johnrobin/.openclaw/workspace/moltchain
cargo build --release --bin molt
```

2. **Test Identity Commands:**
```bash
./target/release/molt identity new
./target/release/molt identity show
./target/release/molt identity list
./target/release/molt identity delete <name>
./target/release/molt identity export <name>
./target/release/molt identity import <file>
```

3. **Test Wallet Commands:**
```bash
./target/release/molt wallet create <name>
./target/release/molt wallet list
./target/release/molt wallet set <name>
./target/release/molt wallet show
```

4. **Test Transaction Commands:**
```bash
./target/release/molt balance
./target/release/molt transfer <to> 10.0
./target/release/molt airdrop 100
```

5. **Test Contract Commands:**
```bash
./target/release/molt deploy contracts/moltcoin/target/wasm32-unknown-unknown/release/moltcoin_token.wasm
./target/release/molt call <contract> transfer '["<to>", 100]'
```

6. **Test Query Commands:**
```bash
./target/release/molt block 1
./target/release/molt latest
./target/release/molt slot
./target/release/molt burned
./target/release/molt validators
./target/release/molt status
./target/release/molt metrics
```

7. **Test Network Commands:**
```bash
./target/release/molt network status
./target/release/molt network peers
./target/release/molt network info
```

8. **Test Validator Commands:**
```bash
./target/release/molt validator info <pubkey>
./target/release/molt validator performance <pubkey>
./target/release/molt validator list
```

9. **Test Staking Commands:**
```bash
./target/release/molt stake add 10000000000
./target/release/molt stake status
./target/release/molt stake rewards
```

10. **Test Account Commands:**
```bash
./target/release/molt account info <address>
./target/release/molt account history <address>
```

11. **Test Contract Commands:**
```bash
./target/release/molt contract info <contract>
./target/release/molt contract logs <contract>
./target/release/molt contract list
```

**Action for Each Command:**
- ✅ Works perfectly → Document it
- ⚠️ Works with bugs → File issue, fix it
- ❌ Returns "not implemented" → Implement it or remove command

**Document Results:**
```markdown
# CLI COMMAND STATUS

## Working (40 commands)
- molt identity new ✅
- molt identity show ✅
...

## Broken (5 commands)
- molt stake add ⚠️ (RPC error, needs fix)
...

## Not Implemented (3 commands)
- molt contract logs ❌ (stub, needs implementation)
...
```

**Success Criteria:**
- ✅ All commands either work or are removed
- ✅ Zero "not implemented" errors remain
- ✅ Documentation updated with working commands

**Effort:** 1-2 days

---

#### Day 4-6: RPC Endpoint Verification 🌐

**Goal:** Verify all 24 documented endpoints actually work

**Process:**

1. **Start Validator:**
```bash
cargo run --release --bin moltchain-validator
```

2. **Test Each Endpoint with curl:**

**Basic Queries (11 endpoints):**
```bash
# getBalance
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getBalance","params":["YOUR_PUBKEY"]}'

# getAccount
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getAccount","params":["YOUR_PUBKEY"]}'

# getBlock
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getBlock","params":[1]}'

# getLatestBlock
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getLatestBlock","params":[]}'

# getSlot
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}'

# getTransaction
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getTransaction","params":["TX_SIG"]}'

# sendTransaction
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"sendTransaction","params":["BASE58_TX"]}'

# getTotalBurned
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getTotalBurned","params":[]}'

# getValidators
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getValidators","params":[]}'

# getMetrics
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getMetrics","params":[]}'

# health
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'
```

**Network Endpoints (2):**
```bash
# getPeers
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getPeers","params":[]}'

# getNetworkInfo
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getNetworkInfo","params":[]}'
```

**Validator Endpoints (3):**
```bash
# getValidatorInfo
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getValidatorInfo","params":["VALIDATOR_PUBKEY"]}'

# getValidatorPerformance
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getValidatorPerformance","params":["VALIDATOR_PUBKEY"]}'

# getChainStatus
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getChainStatus","params":[]}'
```

**Staking Endpoints (4):**
```bash
# stake
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"stake","params":["FROM_PUBKEY","VALIDATOR_PUBKEY",10000000000]}'

# unstake
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"unstake","params":["FROM_PUBKEY","VALIDATOR_PUBKEY",10000000000]}'

# getStakingStatus
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getStakingStatus","params":["PUBKEY"]}'

# getStakingRewards
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getStakingRewards","params":["PUBKEY"]}'
```

**Account Endpoints (2):**
```bash
# getAccountInfo
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getAccountInfo","params":["PUBKEY"]}'

# getTransactionHistory
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getTransactionHistory","params":["PUBKEY", 10]}'
```

**Contract Endpoints (3):**
```bash
# getContractInfo
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getContractInfo","params":["CONTRACT_ID"]}'

# getContractLogs
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getContractLogs","params":["CONTRACT_ID"]}'

# getAllContracts
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getAllContracts","params":[]}'
```

3. **Document Results:**

Create `RPC_ENDPOINT_STATUS.md`:
```markdown
# RPC Endpoint Test Results

## Working (18 endpoints) ✅
- getBalance ✅
- getAccount ✅
- health ✅
...

## Broken (4 endpoints) ⚠️
- getTransactionHistory ⚠️ (returns empty, needs indexing)
- getStakingRewards ⚠️ (not calculating correctly)
...

## Not Implemented (2 endpoints) ❌
- getContractLogs ❌ (returns error)
- getAllContracts ❌ (stub)
```

4. **Fix Broken/Missing Endpoints:**

For each broken endpoint:
- Find implementation in `rpc/src/lib.rs`
- Fix bug or implement stub
- Re-test with curl
- Mark as ✅ when working

**Success Criteria:**
- ✅ All 24 endpoints return valid responses (not errors)
- ✅ RPC_API_REFERENCE.md updated with actual status
- ✅ Zero "not implemented" errors

**Effort:** 2-3 days

---

### Phase 2: HIGH VALUE (10-12 days)

Once P0 items are done, tackle P1 items.

#### Days 7-8: Wallet Integration 💰

**Goal:** Connect wallet UI to real blockchain

**Current Status:**
- UI exists: `wallet/index.html` (36KB)
- Missing: Real RPC calls

**Implementation:**

1. **Update wallet/js/api.js:**
```javascript
class MoltChainAPI {
    constructor(rpcUrl = 'http://localhost:8899') {
        this.rpcUrl = rpcUrl;
    }

    async getBalance(address) {
        return this.rpcCall('getBalance', [address]);
    }

    async sendTransaction(signedTx) {
        return this.rpcCall('sendTransaction', [signedTx]);
    }

    async getTransactionHistory(address, limit = 10) {
        return this.rpcCall('getTransactionHistory', [address, limit]);
    }

    async rpcCall(method, params) {
        const response = await fetch(this.rpcUrl, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                jsonrpc: '2.0',
                id: 1,
                method,
                params
            })
        });
        const data = await response.json();
        if (data.error) throw new Error(data.error.message);
        return data.result;
    }
}
```

2. **Update wallet/index.html:**
- Replace all mock data with real API calls
- Test keypair generation
- Test sending MOLT
- Test balance refresh
- Test transaction history

**Testing:**
```bash
# 1. Start validator
cargo run --release --bin moltchain-validator

# 2. Open wallet
cd wallet
python3 -m http.server 8081
open http://localhost:8081

# 3. Test:
- Generate new keypair
- Get tokens from faucet
- Check balance (should show real balance)
- Send 1 MOLT to another address
- Verify transaction appears in history
```

**Success Criteria:**
- ✅ Wallet shows real balances
- ✅ Send transaction works end-to-end
- ✅ Transaction history displays
- ✅ Keypair export/import works

**Effort:** 1-2 days

---

#### Days 9-10: Marketplace Integration 🎨

**Goal:** Connect marketplace UI to NFT contracts

**Current Status:**
- UI exists: `marketplace/*.html`
- Contracts exist: MoltPunks, Molt Market, MoltAuction
- Missing: Integration

**Implementation:**

1. **Update marketplace/js/contracts.js:**
```javascript
class NFTMarketplace {
    constructor(api, contractAddress) {
        this.api = api;
        this.contractAddress = contractAddress;
    }

    async mint(metadata, to) {
        return this.api.callContract(this.contractAddress, 'mint', [metadata, to]);
    }

    async list(tokenId, price) {
        return this.api.callContract(this.contractAddress, 'list', [tokenId, price]);
    }

    async buy(tokenId, buyer) {
        return this.api.callContract(this.contractAddress, 'buy', [tokenId, buyer]);
    }

    async getListings() {
        return this.api.callContract(this.contractAddress, 'get_listings', []);
    }
}
```

2. **Update browse.html:**
- Load real NFT listings from contract
- Display actual NFT metadata
- Enable real purchases

**Testing:**
```bash
# 1. Start validator
cargo run --release --bin moltchain-validator

# 2. Deploy contracts
molt deploy contracts/moltpunks/target/wasm32-unknown-unknown/release/moltpunks_nft.wasm
molt deploy contracts/moltmarket/target/wasm32-unknown-unknown/release/moltmarket_marketplace.wasm

# 3. Mint test NFT
molt call <moltpunks_contract> mint '["ipfs://test", "<your_address>"]'

# 4. List NFT
molt call <moltmarket_contract> list '[1, 1000000000]'

# 5. Open marketplace
cd marketplace
python3 -m http.server 8082
open http://localhost:8082

# 6. Verify listing appears
```

**Success Criteria:**
- ✅ Browse page shows real NFTs
- ✅ Create page mints real NFTs
- ✅ Purchase flow works end-to-end
- ✅ Profile shows owned NFTs

**Effort:** 1-2 days

---

#### Days 11-12: Programs UI Integration 🔧

**Goal:** Enable real contract deployment and interaction

**Current Status:**
- UI exists: `programs/index.html`, `programs/playground.html`
- Missing: Backend integration

**Implementation:**

1. **Deploy Flow:**
- File upload → Read WASM bytes
- Sign transaction with keypair
- Call `molt deploy <wasm_file>`
- Display contract address

2. **Call Flow:**
- Select deployed contract
- Choose function
- Build arguments JSON
- Call contract
- Display result

**Testing:**
```bash
# 1. Start validator
cargo run --release --bin moltchain-validator

# 2. Open programs UI
cd programs
python3 -m http.server 8083
open http://localhost:8083

# 3. Test:
- Upload MoltCoin WASM
- Deploy it
- Call transfer function
- Verify state changed
```

**Success Criteria:**
- ✅ Contract deployment works
- ✅ Contract calls execute
- ✅ Results displayed correctly
- ✅ Playground editor functional

**Effort:** 1-2 days

---

#### Days 13-15: WebSocket API 🔌

**Goal:** Real-time subscriptions for live data

**Current Status:**
- File exists: `rpc/src/ws.rs`
- Partial implementation

**Implementation:**

1. **Complete WebSocket Server:**
```rust
// rpc/src/ws.rs

pub async fn start_websocket_server(state: Arc<Mutex<State>>) {
    let server = Server::bind("127.0.0.1:9000").await.unwrap();
    
    while let Ok((mut socket, _)) = server.accept().await {
        let state = state.clone();
        
        tokio::spawn(async move {
            handle_client(&mut socket, state).await;
        });
    }
}

async fn handle_client(socket: &mut WebSocket, state: Arc<Mutex<State>>) {
    // Subscribe to block updates
    let mut block_rx = subscribe_to_blocks();
    
    while let Some(block) = block_rx.recv().await {
        let msg = json!({
            "method": "blockNotification",
            "params": {
                "slot": block.slot,
                "hash": block.hash(),
                "transactions": block.transactions.len()
            }
        });
        
        socket.send(Message::Text(msg.to_string())).await;
    }
}
```

2. **Subscription Types:**
- Block notifications
- Transaction confirmations
- Account updates
- Slot updates

**Testing:**
```bash
# 1. Start validator with WebSocket
cargo run --release --bin moltchain-validator

# 2. Connect with wscat
wscat -c ws://localhost:9000

# 3. Subscribe
{"jsonrpc":"2.0","id":1,"method":"subscribe","params":["blocks"]}

# 4. Verify: Receive block notifications every ~1s
```

**Success Criteria:**
- ✅ WebSocket server accepts connections
- ✅ Block subscription works
- ✅ Account subscription works
- ✅ Explorer uses WebSocket for live updates

**Effort:** 2-3 days

---

### Phase 3: MEDIUM VALUE (Optional, 5-10 days)

These can wait until after testnet launch.

#### P2P Network Hardening (3-5 days)

**Goal:** Production-ready networking

**Improvements:**
1. NAT traversal (STUN/TURN)
2. Peer reputation scoring
3. Connection limits
4. DDoS protection

#### Transaction History Indexing (2-3 days)

**Goal:** Full account history

**Implementation:**
- Index all transactions by sender/receiver
- Store in RocksDB
- Enable `getTransactionHistory` RPC

#### Contract Logs Storage (1-2 days)

**Goal:** Persistent contract logs

**Implementation:**
- Store logs in RocksDB
- Index by contract ID
- Enable `getContractLogs` RPC

---

## 📊 COMPLETION TRACKING

**Use this checklist to track progress:**

### Critical (P0)
- [ ] Fee burn implemented and tested
- [ ] CLI fully tested (all commands work)
- [ ] RPC endpoints verified (all 24 working)

### High (P1)
- [ ] Wallet integration complete
- [ ] Marketplace integration complete
- [ ] Programs UI integration complete
- [ ] WebSocket API functional

### Medium (P2)
- [ ] P2P network hardened
- [ ] Transaction history indexed
- [ ] Contract logs stored

---

## 🎯 SUCCESS METRICS

**We're done when:**

1. ✅ **Zero "not implemented" errors** across CLI and RPC
2. ✅ **All UIs work end-to-end** with real blockchain
3. ✅ **Economics match spec** (50% fee burn verified)
4. ✅ **No stubs remain** in critical paths
5. ✅ **Full testnet demo possible** (any developer can use it)

**Timeline:** 12-18 days to 100% completion

---

## 🦞 FINAL THOUGHTS

**The shell is 85% formed. Let's harden it to 100%.** 🐚

Stop starting. Start finishing.

No new features until these gaps close.

**The molt is nearly complete. Let's finish strong.** 🦞⚡

---

*Last Updated: February 8, 2026*  
*Status: Action plan ready, awaiting execution*
