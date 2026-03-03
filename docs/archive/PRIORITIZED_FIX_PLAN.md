# 🦞 MoltChain Prioritized Fix Plan
**Date:** February 8, 2026  
**Baseline:** Post-audit comprehensive assessment  
**Goal:** Testnet launch with accurate documentation

---

## 🎯 PRIORITY 1: CRITICAL - TESTNET LAUNCH BLOCKERS
**Timeline:** 2-3 days  
**Owner:** Core team  
**Blocking:** Testnet can't launch honestly without these

### Task 1.1: Fix CLI Parser Mismatches ⚡
**Effort:** 4 hours  
**Files:** `cli/src/main.rs`, `cli/src/client.rs`

#### Subtasks:
- [ ] **network info parser** - Update to handle `chain_id: String` format
  - Current issue: CLI expects `chain_id: u64`, RPC returns `String`
  - Fix location: `cli/src/main.rs` line ~600-650
  - Test: `molt network info` should display without parse error
  
- [ ] **account info parser** - Add spendable/staked/locked fields
  - Current issue: CLI parser doesn't handle new balance breakdown
  - Fix location: `cli/src/client.rs` line ~300-350
  - Test: `molt account info <address>` should show full breakdown

- [ ] **Add integration tests**
  - Create `tests/cli_integration.rs`
  - Test all 20 commands programmatically
  - Fail CI if any command returns parse error

#### Success Criteria:
✅ `molt network info` passes  
✅ `molt account info <address>` passes  
✅ 20/20 commands passing in integration test  
✅ Updated [INTEGRATION_TEST_REPORT.md](INTEGRATION_TEST_REPORT.md) shows 100%

---

### Task 1.2: Wire Faucet Keypair 🔐
**Effort:** 2 hours  
**Files:** `faucet/src/main.rs`

#### Current State:
```rust
// Line 133
let keypair = Arc::new(Keypair::mock()); // TODO: Load from file
```

#### Implementation:
```rust
// Replace with:
let keypair_path = std::env::var("FAUCET_KEYPAIR_PATH")
    .unwrap_or_else(|_| "./faucet-keypair.json".to_string());

let keypair = Arc::new(
    Keypair::load_from_file(&keypair_path)
        .expect("Failed to load faucet keypair")
);

info!("🦞 Faucet loaded with address: {}", keypair.pubkey().to_base58());
```

#### Additional Tasks:
- [ ] Create `scripts/generate-faucet-keypair.sh`
  ```bash
  #!/bin/bash
  molt identity new --output faucet-keypair.json
  echo "⚠️  SAVE THIS FILE SECURELY - IT CONTROLS TESTNET FUNDS"
  ```

- [ ] Document keypair rotation in `faucet/README.md`
- [ ] Add .gitignore entry for `faucet-keypair.json`
- [ ] Fund faucet address from genesis treasury
- [ ] Test actual MOLT distribution

#### Success Criteria:
✅ Faucet loads real keypair on startup  
✅ Can send 10 MOLT to test addresses  
✅ Rate limiting works correctly  
✅ Keypair rotation procedure documented

---

### Task 1.3: P2P Request Handlers 🌐
**Effort:** 8 hours  
**Files:** `p2p/src/network.rs`

#### Issue 1: BlockRequest Handler (Line 197)
```rust
MessageType::BlockRequest { start_slot, end_slot } => {
    // TODO: Load block from state and send it
    warn!("Received block request for slots {}-{} from {}", start_slot, end_slot, peer_addr);
}
```

**Implementation:**
```rust
MessageType::BlockRequest { start_slot, end_slot } => {
    info!("📦 Block request from {}: slots {}-{}", peer_addr, start_slot, end_slot);
    
    // Load blocks from state
    for slot in start_slot..=end_slot.min(start_slot + 100) {  // Cap at 100 blocks
        if let Ok(Some(block)) = self.state.get_block_by_slot(slot) {
            let response = P2PMessage::new(MessageType::Block(block));
            self.peer_manager.send_to_peer(peer_addr, response).await?;
        } else {
            warn!("Block {} not found", slot);
        }
    }
}
```

#### Issue 2: StatusRequest Handler (Line 232)
```rust
MessageType::StatusRequest => {
    // TODO: Get status from validator state
}
```

**Implementation:**
```rust
MessageType::StatusRequest => {
    info!("📊 Status request from {}", peer_addr);
    
    let current_slot = self.state.get_last_slot().unwrap_or(0);
    let validators = self.state.get_all_validators().unwrap_or_default();
    let total_stake: u64 = validators.iter().map(|v| v.stake).sum();
    
    let status = NetworkStatus {
        current_slot,
        validator_count: validators.len() as u64,
        total_stake,
        best_block_hash: self.state.get_block_by_slot(current_slot)
            .ok()
            .flatten()
            .map(|b| b.hash())
            .unwrap_or(Hash::default()),
    };
    
    let response = P2PMessage::new(MessageType::StatusResponse(status));
    self.peer_manager.send_to_peer(peer_addr, response).await?;
}
```

#### Issue 3: SlashingEvidence Handler (Line 258)
```rust
MessageType::SlashingEvidence(evidence) => {
    // TODO: Forward to validator for processing
}
```

**Implementation:**
```rust
MessageType::SlashingEvidence(evidence) => {
    warn!("⚠️  Slashing evidence received from {}: {:?}", peer_addr, evidence);
    
    // Verify evidence is well-formed
    if evidence.verify() {
        // Forward to validator via channel
        self.slashing_tx
            .send(evidence)
            .map_err(|_| "Failed to send slashing evidence to validator")?;
        
        info!("✓ Slashing evidence forwarded to validator");
    } else {
        warn!("Invalid slashing evidence from {}", peer_addr);
    }
}
```

#### Additional Changes:
- [ ] Add `NetworkStatus` struct to `p2p/src/message.rs`
- [ ] Add `StatusResponse` variant to `MessageType` enum
- [ ] Pass `StateStore` reference to `P2PNetwork::new()`
- [ ] Add `slashing_tx` channel to validator main loop
- [ ] Test with 3-validator network

#### Success Criteria:
✅ New validator can sync blocks from existing network  
✅ Status requests return accurate chain state  
✅ Slashing evidence is propagated network-wide  
✅ Integration test: start 3 validators, verify full sync

---

### Task 1.4: Reconcile Documentation 📚
**Effort:** 4 hours  
**Files:** All `docs/*.md`, `README.md`, `WHITEPAPER.md`

#### Issue 1: "100% Complete" Overclaim
**Find and replace across all files:**
- ❌ "100% COMPLETE"
- ❌ "100% complete"  
- ❌ "fully implemented"
- ✅ "Testnet Ready (Core 100%, Advanced Features In Progress)"

**Files to update:**
- [ ] `docs/100_PERCENT_COMPLETE.md` → Rename to `TESTNET_READY.md`
- [ ] `docs/LAUNCH_READY.md` → Add caveats about EVM/bridges
- [ ] `README.md` → Honest status in hero section

#### Issue 2: EVM Claims
**Add qualifiers to all EVM mentions:**
- ❌ "EVM compatible"
- ❌ "Solidity support"
- ❌ "MetaMask ready"
- ✅ "EVM compatibility (Coming Q2 2026)"
- ✅ "Solidity support (In Development)"
- ✅ "MetaMask integration (Roadmap)"

**Files to update:**
- [ ] `README.md` line ~50-70
- [ ] `docs/WHITEPAPER.md` section 4
- [ ] `docs/ARCHITECTURE.md` execution layer
- [ ] `docs/GETTING_STARTED.md` MetaMask section

#### Issue 3: SDK Installation
**Update installation sections:**

**Before:**
```bash
npm install @moltchain/sdk
pip install moltchain
cargo install molt-cli
```

**After:**
```bash
# JavaScript SDK (build from source until npm package published)
cd js-sdk
npm install
npm run build

# Python SDK (build from source until PyPI package published)
cd python-sdk
pip install -e .

# CLI (coming soon to crates.io, for now build from source)
cd cli
cargo install --path .
```

**Files to update:**
- [ ] `docs/GETTING_STARTED.md`
- [ ] `docs/api/JAVASCRIPT_SDK.md`
- [ ] `docs/api/PYTHON_SDK.md`
- [ ] `js-sdk/README.md`
- [ ] `python-sdk/README.md`

#### Issue 4: Token Naming (CLAW → MOLT)
**Global find and replace:**
- ❌ `CLAW` → ✅ `MOLT`
- ❌ `$CLAW` → ✅ `$MOLT`
- Keep: `ClawSwap`, `ClawPump` (product names OK)

**Use ripgrep for accuracy:**
```bash
rg -i "\\bCLAW\\b" docs/ | grep -v "ClawSwap\|ClawPump"
# Manually fix each occurrence
```

**Files likely needing updates:**
- [ ] `docs/VISION.md`
- [ ] `docs/WHITEPAPER.md` (some sections)
- [ ] `docs/foundation/ECONOMICS.md`

#### Issue 5: Internal Docs Cleanup
**Archive stale status reports:**
```bash
mv internal-docs/system-status/DEVELOPER_API_STATUS.md \
   internal-docs/system-status/archive/DEVELOPER_API_STATUS_FEB5.md
```

- [ ] Add note: "Superseded by [COMPREHENSIVE_AUDIT_FEB8.md](../COMPREHENSIVE_AUDIT_FEB8.md)"
- [ ] Do same for other stale reports

#### Success Criteria:
✅ No "100% complete" claims without caveats  
✅ All EVM references have "(Coming Soon)" qualifier  
✅ SDK installs documented as build-from-source  
✅ Zero instances of bare "CLAW" token name  
✅ Internal docs archived with timestamps

---

## 🎯 PRIORITY 2: HIGH - TESTNET QUALITY
**Timeline:** 1 week  
**Owner:** Core team + early contributors  
**Blocking:** Professional testnet experience

### Task 2.1: Contract Indexing 📦
**Effort:** 1 day  
**Files:** `core/src/state.rs`, `rpc/src/lib.rs`

#### Implementation:

**Step 1: Add index to StateStore**
```rust
// core/src/state.rs

pub struct StateStore {
    db: Arc<DB>,
    executable_accounts: Arc<RwLock<BTreeSet<Pubkey>>>,  // NEW
}

impl StateStore {
    pub fn open(path: &str) -> Result<Self, String> {
        let db = DB::open_default(path)?;
        
        // Rebuild index from existing state
        let mut executable_accounts = BTreeSet::new();
        let iter = db.iterator(IteratorMode::Start);
        for (key, value) in iter {
            if key.starts_with(b"account:") {
                let account: Account = bincode::deserialize(&value)?;
                if account.executable {
                    let pubkey = Pubkey::from_bytes(&key[8..])?;
                    executable_accounts.insert(pubkey);
                }
            }
        }
        
        Ok(Self {
            db: Arc::new(db),
            executable_accounts: Arc::new(RwLock::new(executable_accounts)),
        })
    }
    
    pub fn mark_executable(&self, pubkey: &Pubkey) -> Result<(), String> {
        self.executable_accounts.write().unwrap().insert(*pubkey);
        Ok(())
    }
    
    pub fn mark_non_executable(&self, pubkey: &Pubkey) -> Result<(), String> {
        self.executable_accounts.write().unwrap().remove(pubkey);
        Ok(())
    }
    
    pub fn count_executable_accounts(&self) -> u64 {
        self.executable_accounts.read().unwrap().len() as u64
    }
    
    pub fn get_all_executable_accounts(&self) -> Vec<Pubkey> {
        self.executable_accounts.read().unwrap().iter().copied().collect()
    }
}
```

**Step 2: Update processor to maintain index**
```rust
// core/src/processor.rs

fn deploy_contract(&self, deployer: &Pubkey, code: Vec<u8>) -> Result<Pubkey, String> {
    let contract_id = Pubkey::generate();
    let mut account = Account::new(0, contract_id);
    account.executable = true;
    account.owner = *deployer;
    account.data = code;
    
    self.state.put_account(&contract_id, &account)?;
    self.state.mark_executable(&contract_id)?;  // NEW
    
    Ok(contract_id)
}
```

**Step 3: Update RPC to use index**
```rust
// rpc/src/lib.rs

async fn handle_get_all_contracts(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let executable_pubkeys = state.state.get_all_executable_accounts();
    
    let mut contracts = Vec::new();
    for pubkey in executable_pubkeys.iter().take(100) {  // Paginate later
        if let Ok(Some(account)) = state.state.get_account(pubkey) {
            contracts.push(serde_json::json!({
                "address": pubkey.to_base58(),
                "owner": account.owner.to_base58(),
                "code_size": account.data.len(),
            }));
        }
    }
    
    Ok(serde_json::json!({
        "contracts": contracts,
        "count": executable_pubkeys.len(),
    }))
}
```

#### Success Criteria:
✅ `count_executable_accounts()` is $O(1)$  
✅ `get_all_contracts()` returns actual contract list  
✅ Index rebuilds correctly on validator restart  
✅ Performance test: 1000 contracts indexed in <100ms

---

### Task 2.2: Package SDKs 📦
**Effort:** 2 days  
**Files:** `js-sdk/`, `python-sdk/`

#### JavaScript SDK

**Step 1: Add package.json**
```json
{
  "name": "@moltchain/sdk",
  "version": "0.1.0",
  "description": "Official JavaScript SDK for MoltChain blockchain",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "scripts": {
    "build": "tsc",
    "test": "jest",
    "prepublish": "npm run build"
  },
  "keywords": ["moltchain", "blockchain", "web3", "crypto"],
  "author": "MoltChain Team",
  "license": "MIT",
  "dependencies": {
    "tweetnacl": "^1.0.3",
    "bs58": "^5.0.0",
    "axios": "^1.6.0"
  },
  "devDependencies": {
    "typescript": "^5.0.0",
    "@types/node": "^20.0.0",
    "jest": "^29.0.0"
  }
}
```

**Step 2: Add tsconfig.json**
```json
{
  "compilerOptions": {
    "target": "ES2020",
    "module": "commonjs",
    "declaration": true,
    "outDir": "./dist",
    "strict": true,
    "esModuleInterop": true
  },
  "include": ["src/**/*"],
  "exclude": ["node_modules", "dist"]
}
```

**Step 3: Publish**
```bash
cd js-sdk
npm install
npm run build
npm publish --access public
```

#### Python SDK

**Step 1: Add setup.py**
```python
from setuptools import setup, find_packages

setup(
    name="moltchain",
    version="0.1.0",
    description="Official Python SDK for MoltChain blockchain",
    author="MoltChain Team",
    packages=find_packages(),
    install_requires=[
        "requests>=2.31.0",
        "PyNaCl>=1.5.0",
        "base58>=2.1.1",
    ],
    python_requires=">=3.8",
    classifiers=[
        "Development Status :: 4 - Beta",
        "Intended Audience :: Developers",
        "License :: OSI Approved :: MIT License",
        "Programming Language :: Python :: 3.8",
        "Programming Language :: Python :: 3.9",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.11",
    ],
)
```

**Step 2: Add tests**
```python
# tests/test_client.py
import pytest
from moltchain import MoltChainClient

def test_client_init():
    client = MoltChainClient("http://localhost:8899")
    assert client.rpc_url == "http://localhost:8899"

def test_keypair_generation():
    from moltchain import generate_keypair, public_key_to_address
    kp = generate_keypair()
    addr = public_key_to_address(kp.public_key)
    assert len(addr) > 0
```

**Step 3: Publish**
```bash
cd python-sdk
pip install build twine
python -m build
twine upload dist/*
```

#### Success Criteria:
✅ `npm install @moltchain/sdk` works  
✅ `pip install moltchain` works  
✅ TypeScript declarations included for JS  
✅ Documentation updated with correct install commands

---

### Task 2.3: Staking Rewards Validation ✓
**Effort:** 1 day  
**Files:** Testing only

#### Test Plan:

**Setup:**
1. Start 3-validator network
2. Wait for genesis (slot 1-10)
3. Create 3 test accounts (not validators)
4. Stake from each to different validators

**Test Cases:**
```bash
# Test 1: Bootstrap debt repayment
./test-staking.sh bootstrap_debt
# Expected: Validator earns, debt decreases, earned increases

# Test 2: Vesting progress
./test-staking.sh vesting_progress
# Expected: Progress 0% → 100% as debt paid

# Test 3: Delegation rewards (if implemented)
./test-staking.sh delegation_rewards
# Expected: Delegators earn proportional to stake

# Test 4: Unstaking cooldown
./test-staking.sh unstake_cooldown
# Expected: 7-day lockup enforced
```

#### Automated Test Script:
```bash
#!/bin/bash
# test-staking.sh

case $1 in
  bootstrap_debt)
    VALIDATOR=$(molt validators | jq -r '.validators[0].pubkey')
    INITIAL_DEBT=$(molt staking rewards $VALIDATOR | jq -r '.bootstrap_debt')
    echo "Initial debt: $INITIAL_DEBT"
    
    sleep 60  # Wait for rewards
    
    CURRENT_DEBT=$(molt staking rewards $VALIDATOR | jq -r '.bootstrap_debt')
    echo "Current debt: $CURRENT_DEBT"
    
    if [ "$CURRENT_DEBT" -lt "$INITIAL_DEBT" ]; then
      echo "✓ Debt is decreasing"
    else
      echo "❌ Debt not decreasing!"
      exit 1
    fi
    ;;
  
  vesting_progress)
    # ... similar tests ...
    ;;
esac
```

#### Success Criteria:
✅ Bootstrap debt decreases correctly (50% of rewards)  
✅ Earned amount increases correctly (50% of rewards)  
✅ Vesting progress reaches 100%  
✅ getStakingRewards returns accurate data  
✅ Automated test suite passes

---

### Task 2.4: Integration Test Suite 🧪
**Effort:** 2 days  
**Files:** `tests/integration/`

#### Test Structure:
```
tests/
├── integration/
│   ├── test_rpc_endpoints.rs
│   ├── test_consensus.rs
│   ├── test_staking.rs
│   ├── test_p2p.rs
│   └── test_cli.rs
└── common/
    ├── setup.rs
    └── helpers.rs
```

#### Key Tests:

**test_rpc_endpoints.rs:**
```rust
#[tokio::test]
async fn test_all_24_endpoints() {
    let validator = start_test_validator().await;
    let client = MoltChainClient::new("http://localhost:8899");
    
    // Test each endpoint
    assert!(client.get_balance(&test_address()).await.is_ok());
    assert!(client.get_block(1).await.is_ok());
    assert!(client.get_validators().await.is_ok());
    // ... 21 more
}
```

**test_consensus.rs:**
```rust
#[tokio::test]
async fn test_3_validator_consensus() {
    let v1 = start_validator(8000).await;
    let v2 = start_validator(8001).await;
    let v3 = start_validator(8002).await;
    
    // Submit transaction to v1
    let tx = create_transfer_tx();
    v1.submit_transaction(tx.clone()).await.unwrap();
    
    // Wait for consensus
    tokio::time::sleep(Duration::from_secs(5)).await;
    
    // Verify all 3 have same block
    let block1 = v1.get_latest_block().await.unwrap();
    let block2 = v2.get_latest_block().await.unwrap();
    let block3 = v3.get_latest_block().await.unwrap();
    
    assert_eq!(block1.hash(), block2.hash());
    assert_eq!(block2.hash(), block3.hash());
}
```

#### CI Integration:
```yaml
# .github/workflows/integration-tests.yml
name: Integration Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - uses: actions-rs/toolchain@v1
      - name: Run integration tests
        run: cargo test --test integration -- --nocapture
```

#### Success Criteria:
✅ All 24 RPC endpoints tested  
✅ Multi-validator consensus tested  
✅ Staking flow tested  
✅ P2P sync tested  
✅ CLI commands tested  
✅ CI passes on every commit

---

## 🎯 PRIORITY 3: MEDIUM - POST-TESTNET
**Timeline:** 2-3 weeks after testnet  
**Owner:** Extended team + contributors

### Task 3.1: EVM Compatibility
**Effort:** 2-3 weeks  
**Details:** See [COMPREHENSIVE_AUDIT_FEB8.md](COMPREHENSIVE_AUDIT_FEB8.md#1-evm-compatibility-0---only-stubs)

### Task 3.2: ReefStake Liquid Staking
**Effort:** 1-2 weeks  
**Details:** See audit section 3.2

### Task 3.3: Price Oracle Integration
**Effort:** 1 week  
**Details:** See audit section 3.3

### Task 3.4: Block Explorer Polish
**Effort:** 3 days  
**Details:** See audit section 3.4

---

## 📊 PROGRESS TRACKING

### Daily Standup Template:
```markdown
**Yesterday:**
- [x] Task completed
- [ ] Task in progress (70%)

**Today:**
- [ ] Continue task X
- [ ] Start task Y

**Blockers:**
- None / Waiting on...
```

### Weekly Review:
- Monday: Review Priority 1 progress
- Wednesday: Mid-week checkpoint
- Friday: Ship demo, update docs

---

## 🎯 SUCCESS METRICS

### Priority 1 Complete When:
- [ ] Integration tests show 100% CLI pass rate
- [ ] Faucet successfully distributes testnet MOLT
- [ ] 3-validator network syncs and produces blocks
- [ ] All docs accurate (no "100%" claims without caveats)

### Priority 2 Complete When:
- [ ] npm/pip SDKs installable
- [ ] Contract indexing performs <100ms for 1000 contracts
- [ ] Staking rewards validated in multi-validator setup
- [ ] CI runs full integration suite on every commit

### Testnet Launch Criteria:
- [ ] Priority 1: 100% complete
- [ ] Priority 2: 75%+ complete
- [ ] 5+ validators running stable for 48 hours
- [ ] 100+ test transactions processed
- [ ] Explorer and wallet functional

---

**Last Updated:** February 8, 2026  
**Next Review:** Daily until Priority 1 complete

🦞⚡ **Let's ship an honest, working testnet!**
