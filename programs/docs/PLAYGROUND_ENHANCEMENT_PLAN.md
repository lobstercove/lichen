# Playground Enhancement Plan - Complete Solana Playground Equivalent

## 🎯 Goal
Transform the Playground into a comprehensive demo matching Solana Playground (beta.solpg.io) with FULL support for everything in MoltChain core code.

## 📋 Features from Core Code Analysis

### From processor.rs:
- ✅ System Program (SYSTEM_PROGRAM_ID = all 0s)
  - Transfer shells between accounts
  - Create new accounts
- ✅ Contract Program (CONTRACT_PROGRAM_ID = all 0xFFs)
  - Deploy: New contract deployment
  - Call: Execute contract functions
  - Upgrade: Owner-based contract upgrades
  - Close: Close contract and withdraw
- ✅ Fee System
  - BASE_FEE = 1,000,000 shells (0.001 MOLT)
  - 40% burned, 30% producer, 10% voters, 10% validator pool, 10% community
- ✅ Transaction structure with signatures
- ✅ Gas metering (DEFAULT_GAS_LIMIT = 1,000,000)

### From genesis.rs:
- ✅ Testnet/Mainnet configurations
- ✅ Initial account balances (FAUCET capability)
- ✅ Initial validators
- ✅ Consensus parameters
- ✅ Feature flags

### From state.rs:
- ✅ Account storage (shells, data, owner, executable)
- ✅ Block storage
- ✅ Transaction storage
- ✅ Metrics (TPS, total txs, blocks, avg block time)
- ✅ Burned amount tracking
- ✅ Validator management

### From contract.rs:
- ✅ WASM bytecode validation
- ✅ Contract storage (key-value)
- ✅ Gas consumption tracking
- ✅ Contract context (caller, value, slot, gas)
- ✅ Contract logs
- ✅ Owner-based permissions

---

## 🚀 NEW Features to Add

### 1. Faucet System ✨
**What**: Request test MOLT from testnet faucet

**UI Location**: Top-right near wallet button

**Implementation**:
```javascript
async function requestFaucet() {
    if (!state.wallet) {
        addTerminalLine('❌ Connect wallet first!', 'error');
        openWalletModal();
        return;
    }
    
    if (state.network !== 'testnet') {
        addTerminalLine('❌ Faucet only available on testnet!', 'error');
        return;
    }
    
    addTerminalLine('💧 Requesting faucet...', 'info');
    
    setTimeout(() => {
        const amount = 100; // 100 MOLT
        state.wallet.balance = (parseFloat(state.wallet.balance) + amount).toFixed(2);
        
        addTerminalLine('✅ Faucet successful!', 'success');
        addTerminalLine(`   Received: ${amount} MOLT`, 'normal');
        addTerminalLine(`   New balance: ${state.wallet.balance} MOLT`, 'normal');
        
        updateWalletUI();
    }, 1500);
}
```

**UI**:
```html
<button class="btn btn-sm btn-success" id="faucetBtn" style="display: none;">
    <i class="fas fa-faucet"></i> Request Faucet
</button>
```

---

### 2. Program ID Management ✨
**What**: Generate, display, and manage program addresses

**Features**:
- Auto-generate program address (derived from deployer + code)
- Show program keypair in binary format
- Display program ID prominently after deploy
- Support program upgrades (not just fresh deploy)

**Implementation**:
```javascript
// Generate program address (deterministic from deployer + code)
function generateProgramAddress(deployerPubkey, code) {
    // In production: use actual derivation (hash of deployer + code)
    // For mock: generate deterministic address
    const combined = deployerPubkey + code.slice(0, 100);
    const hash = hashString(combined);
    return 'molt1' + hash.substring(0, 40);
}

// Show program keypair
function showProgramKeypair(programId) {
    // Generate mock binary keypair
    const keypair = {
        publicKey: programId,
        secretKey: generateSecretKey(),
        binary: generateBinaryKeypair()
    };
    
    return keypair;
}

// Check if program exists (for upgrade vs deploy)
function programExists(programId) {
    return state.deployedPrograms.some(p => p.address === programId);
}

// Upgrade program
async function upgradeProgram(programId) {
    if (!state.wallet) {
        addTerminalLine('❌ Connect wallet first!', 'error');
        return;
    }
    
    const program = state.deployedPrograms.find(p => p.address === programId);
    if (!program) {
        addTerminalLine('❌ Program not found!', 'error');
        return;
    }
    
    if (program.deployer !== state.wallet.address) {
        addTerminalLine('❌ Only owner can upgrade!', 'error');
        return;
    }
    
    addTerminalLine('🔄 Upgrading program...', 'info');
    // ... upgrade flow
}
```

---

### 3. Comprehensive Contract Deployment Mock Data ✨

**Support ALL 7 production contracts with full mock data:**

#### A. MoltCoin (Token)
```javascript
const MOLTCOIN_MOCK = {
    name: 'MoltCoin',
    type: 'token',
    functions: ['initialize', 'mint', 'transfer', 'burn', 'balance_of', 'approve', 'transfer_from'],
    storage: {
        'total_supply': '1000000000000000000', // 1B tokens
        'decimals': '9',
        'name': 'MoltCoin',
        'symbol': 'MOLT'
    },
    transactions: []
};
```

#### B. MoltPunks (NFT)
```javascript
const MOLTPUNKS_MOCK = {
    name: 'MoltPunks',
    type: 'nft',
    functions: ['mint', 'transfer', 'approve', 'get_owner', 'get_metadata'],
    storage: {
        'total_supply': '10000',
        'minted': '347',
        'base_uri': 'ipfs://QmXXX...'
    },
    nfts: [
        { id: 1, owner: 'molt1...', uri: 'ipfs://QmXXX/1.json' },
        { id: 2, owner: 'molt1...', uri: 'ipfs://QmXXX/2.json' }
    ]
};
```

#### C. MoltSwap (DEX)
```javascript
const MOLTSWAP_MOCK = {
    name: 'MoltSwap',
    type: 'dex',
    functions: ['create_pool', 'add_liquidity', 'remove_liquidity', 'swap', 'get_price'],
    storage: {
        'pools': '5',
        'total_liquidity': '10000000000000',
        'total_volume_24h': '5000000000000'
    },
    pools: [
        { token_a: 'MOLT', token_b: 'USDC', reserve_a: '1000000', reserve_b: '50000' },
        { token_a: 'MOLT', token_b: 'ETH', reserve_a: '500000', reserve_b: '200' }
    ]
};
```

#### D. MoltDAO (Governance)
```javascript
const MOLTDAO_MOCK = {
    name: 'MoltDAO',
    type: 'dao',
    functions: ['create_proposal', 'vote', 'execute', 'get_proposal'],
    storage: {
        'total_proposals': '23',
        'active_proposals': '3',
        'total_members': '156'
    },
    proposals: [
        { id: 1, title: 'Increase block reward', votes_for: 120, votes_against: 30, status: 'active' },
        { id: 2, title: 'Add new validator', votes_for: 145, votes_against: 10, status: 'executed' }
    ]
};
```

#### E. MoltOracle (Price Feeds)
```javascript
const MOLTORACLE_MOCK = {
    name: 'MoltOracle',
    type: 'oracle',
    functions: ['submit_price', 'get_price', 'get_history'],
    storage: {
        'feeds': '10',
        'total_updates_24h': '2340'
    },
    prices: [
        { asset: 'MOLT/USD', price: '0.05', timestamp: Date.now(), source: 'molt1...' },
        { asset: 'ETH/USD', price: '2340.50', timestamp: Date.now(), source: 'molt1...' }
    ]
};
```

#### F. Molt Market (NFT Marketplace)
```javascript
const MOLTMARKET_MOCK = {
    name: 'Molt Market',
    type: 'marketplace',
    functions: ['list', 'delist', 'buy', 'make_offer', 'accept_offer'],
    storage: {
        'total_listings': '450',
        'total_sales': '1234',
        'total_volume': '50000000000000'
    },
    listings: [
        { nft_id: 1, price: '100000000000', seller: 'molt1...', status: 'active' },
        { nft_id: 5, price: '250000000000', seller: 'molt1...', status: 'active' }
    ]
};
```

#### G. MoltAuction (Auction System)
```javascript
const MOLTAUCTION_MOCK = {
    name: 'MoltAuction',
    type: 'auction',
    functions: ['create_auction', 'bid', 'settle', 'cancel'],
    storage: {
        'active_auctions': '12',
        'total_settled': '89'
    },
    auctions: [
        { id: 1, item: 'MoltPunk #42', highest_bid: '500000000000', bids: 23, ends: Date.now() + 3600000 }
    ]
};
```

---

### 4. Advanced Transaction Types ✨

**A. Transfer Shells**
```html
<!-- Add to right panel -->
<div class="panel-section">
    <div class="panel-header">
        <h3><i class="fas fa-exchange-alt"></i> Transfer</h3>
    </div>
    <div class="panel-content">
        <div class="form-group">
            <label>To Address</label>
            <input type="text" id="transferTo" class="form-input-sm" placeholder="molt1...">
        </div>
        <div class="form-group">
            <label>Amount (MOLT)</label>
            <input type="number" id="transferAmount" class="form-input-sm" placeholder="0.00">
        </div>
        <button class="btn btn-primary btn-block btn-sm" onclick="transferShells()">
            <i class="fas fa-paper-plane"></i> Send
        </button>
    </div>
</div>
```

**B. Create Account**
```javascript
function createAccount(pubkey) {
    addTerminalLine('🆕 Creating account...', 'info');
    setTimeout(() => {
        addTerminalLine('✅ Account created!', 'success');
        addTerminalLine(`   Address: ${pubkey}`, 'normal');
    }, 500);
}
```

**C. Call Contract with Gas**
```javascript
function callContract(address, functionName, args, gasLimit, value) {
    addTerminalLine(`🔧 Calling ${functionName}() on ${address.substring(0, 16)}...`, 'info');
    addTerminalLine(`   Gas limit: ${gasLimit.toLocaleString()}`, 'normal');
    addTerminalLine(`   Value: ${value} shells`, 'normal');
    
    setTimeout(() => {
        const gasUsed = Math.floor(Math.random() * gasLimit * 0.5);
        
        addTerminalLine('✅ Contract call successful!', 'success');
        addTerminalLine(`   Gas used: ${gasUsed.toLocaleString()} / ${gasLimit.toLocaleString()}`, 'normal');
        addTerminalLine(`   Gas remaining: ${(gasLimit - gasUsed).toLocaleString()}`, 'normal');
    }, 1200);
}
```

**D. Upgrade Contract**
```html
<button class="btn btn-sm btn-warning" onclick="upgradeContract()">
    <i class="fas fa-arrow-up"></i> Upgrade
</button>
```

**E. Close Contract**
```javascript
function closeContract(address) {
    const program = state.deployedPrograms.find(p => p.address === address);
    if (program.deployer !== state.wallet.address) {
        addTerminalLine('❌ Only owner can close!', 'error');
        return;
    }
    
    addTerminalLine('🔒 Closing contract...', 'info');
    // Withdraw balance and mark as closed
}
```

---

### 5. Enhanced UI Components ✨

**A. Program Info Panel**
```html
<div class="program-info-panel">
    <div class="info-row">
        <span class="info-label">Program ID:</span>
        <span class="info-value monospace">molt1abc...xyz</span>
        <button class="btn-icon-sm" onclick="copyProgramId()">
            <i class="fas fa-copy"></i>
        </button>
    </div>
    <div class="info-row">
        <span class="info-label">Owner:</span>
        <span class="info-value monospace">molt1def...uvw</span>
    </div>
    <div class="info-row">
        <span class="info-label">Code Size:</span>
        <span class="info-value">45.2 KB</span>
    </div>
    <div class="info-row">
        <span class="info-label">Gas Used:</span>
        <span class="info-value">2.3M</span>
    </div>
    <div class="info-row">
        <span class="info-label">Deployed:</span>
        <span class="info-value">2 hours ago</span>
    </div>
</div>
```

**B. Contract Storage Viewer**
```html
<div class="storage-viewer">
    <h4>Storage</h4>
    <div class="storage-list">
        <div class="storage-item">
            <span class="storage-key">total_supply</span>
            <span class="storage-value">1000000000000000000</span>
        </div>
        <div class="storage-item">
            <span class="storage-key">owner</span>
            <span class="storage-value">molt1abc...xyz</span>
        </div>
    </div>
</div>
```

**C. Transaction History**
```html
<div class="tx-history">
    <h4>Recent Transactions</h4>
    <div class="tx-list">
        <div class="tx-item">
            <div class="tx-signature">sig:abc123...</div>
            <div class="tx-info">
                <span class="tx-type">Deploy</span>
                <span class="tx-time">2 min ago</span>
                <span class="tx-status success">✓</span>
            </div>
        </div>
    </div>
</div>
```

---

### 6. Metrics Display ✨

**Show real-time metrics from state.rs:**
```html
<div class="metrics-panel">
    <div class="metric-item">
        <span class="metric-label">TPS:</span>
        <span class="metric-value" id="metricTPS">1,234</span>
    </div>
    <div class="metric-item">
        <span class="metric-label">Total Txs:</span>
        <span class="metric-value" id="metricTotalTxs">5,678,901</span>
    </div>
    <div class="metric-item">
        <span class="metric-label">Total Blocks:</span>
        <span class="metric-value" id="metricTotalBlocks">123,456</span>
    </div>
    <div class="metric-item">
        <span class="metric-label">Avg Block Time:</span>
        <span class="metric-value" id="metricBlockTime">400ms</span>
    </div>
    <div class="metric-item">
        <span class="metric-label">Total Burned:</span>
        <span class="metric-value" id="metricBurned">1,234.56 MOLT</span>
    </div>
</div>
```

---

## 📝 Implementation Priority

### Phase 1: Core Enhancements (High Priority) ⚡
1. ✅ Faucet button & functionality
2. ✅ Program ID generation & display
3. ✅ Upgrade program support
4. ✅ Transfer shells UI & logic
5. ✅ Contract storage viewer

### Phase 2: Mock Data Expansion (High Priority) ⚡
1. ✅ All 7 contracts with full mock data
2. ✅ Contract-specific functions
3. ✅ Storage state for each contract
4. ✅ Transaction history per contract
5. ✅ Realistic gas consumption

### Phase 3: Advanced Features (Medium Priority) 📊
1. ✅ Metrics dashboard
2. ✅ Close contract functionality
3. ✅ Create account UI
4. ✅ Binary keypair display
5. ✅ Enhanced transaction viewer

### Phase 4: Polish & UX (Medium Priority) ✨
1. ✅ Better error messages
2. ✅ Loading states
3. ✅ Success animations
4. ✅ Tooltips everywhere
5. ✅ Keyboard shortcuts

---

## 🎯 Success Criteria

- [ ] Faucet works on testnet
- [ ] Program IDs generated deterministically
- [ ] Can upgrade deployed programs
- [ ] Can transfer shells between accounts
- [ ] All 7 contracts deployable with full mock data
- [ ] Contract storage viewable
- [ ] Metrics display real-time
- [ ] Matches Solana Playground feature parity

---

## 📊 Comparison: Solana Playground vs MoltChain Playground

| Feature | Solana PG | Molt PG (Current) | Molt PG (Enhanced) |
|---------|-----------|-------------------|---------------------|
| Monaco Editor | ✅ | ✅ | ✅ |
| Build & Deploy | ✅ | ✅ | ✅ |
| Faucet | ✅ | ❌ | ✅ |
| Program ID Gen | ✅ | ❌ | ✅ |
| Upgrade Support | ✅ | ❌ | ✅ |
| Transfer UI | ✅ | ❌ | ✅ |
| Storage Viewer | ✅ | ❌ | ✅ |
| Transaction History | ✅ | ❌ | ✅ |
| Metrics Display | ✅ | ❌ | ✅ |
| Wallet Management | ✅ | ✅ | ✅ |
| Examples Library | ✅ | ✅ | ✅ (enhanced) |
| Test Execution | ✅ | ✅ | ✅ |

**Goal**: 100% feature parity + MoltChain-specific enhancements

---

## 🚀 Next Steps

1. Implement Phase 1 enhancements
2. Test all new features
3. Add comprehensive mock data
4. Polish UI/UX
5. Document everything
6. Continue with remaining 6 components (Dashboard, Explorer, Docs, Terminal, Examples, Deploy Wizard)

---

**THE BIG MOLT CONTINUES!** 🦞⚡
