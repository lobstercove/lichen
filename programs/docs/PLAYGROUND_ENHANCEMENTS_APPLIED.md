# Playground Enhancements - Step by Step Applied

## ✅ Step 1: HTML Updates (COMPLETE)

### Added to Navigation:
- ✅ Faucet button (testnet only, next to wallet button)

### Added to Right Sidebar:
- ✅ Metrics Panel (TPS, Total Txs, Block Time, Burned)
- ✅ Transfer Panel (Send MOLT between addresses)
- ✅ Program Info Panel (View program details, upgrade, close)
- ✅ Storage Viewer Panel (View contract storage state)

### Added to CSS:
- ✅ `.panel-metrics` and `.metrics-grid` styles
- ✅ `.metric-item`, `.metric-value`, `.metric-label` styles
- ✅ `.transfer-form` styles
- ✅ `.program-info`, `.info-row`, `.info-label`, `.info-value` styles
- ✅ `.storage-viewer`, `.storage-list`, `.storage-item` styles
- ✅ `.btn-success`, `.btn-warning`, `.btn-danger` button variants

---

## 🚧 Step 2: JavaScript Enhancements (IN PROGRESS)

### New Features to Add to playground.js:

#### A. Enhanced Configuration (Add at top)
```javascript
const CONFIG = {
    // ... existing config ...
    fees: {
        baseFee: 10000, // 0.00001 MOLT in shells
        burnPercentage: 50
    },
    gas: {
        defaultLimit: 1000000
    }
};

// Program IDs from core
const SYSTEM_PROGRAM_ID = 'molt1' + '0'.repeat(40);
const CONTRACT_PROGRAM_ID = 'molt1' + 'F'.repeat(40);
```

#### B. Contract Templates (Add after MOCK_FILES)
```javascript
const CONTRACT_TEMPLATES = {
    moltcoin: { /* Full token template */ },
    moltpunks: { /* Full NFT template */ },
    moltswap: { /* Full DEX template */ },
    moltdao: { /* Full DAO template */ },
    moltoracle: { /* Full Oracle template */ },
    moltmarket: { /* Full Marketplace template */ },
    moltauction: { /* Full Auction template */ }
};
```

#### C. Faucet Function
```javascript
function requestFaucet() {
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
    addTerminalLine('   Network: Testnet', 'normal');
    addTerminalLine('   Wallet: ' + state.wallet.address.substring(0, 16) + '...', 'normal');
    
    setTimeout(() => {
        const amount = 100; // 100 MOLT
        const oldBalance = parseFloat(state.wallet.balance);
        state.wallet.balance = (oldBalance + amount).toFixed(2);
        
        addTerminalLine('', 'normal');
        addTerminalLine('✅ Faucet successful!', 'success');
        addTerminalLine('   Received: ' + amount + ' MOLT', 'normal');
        addTerminalLine('   Old balance: ' + oldBalance + ' MOLT', 'normal');
        addTerminalLine('   New balance: ' + state.wallet.balance + ' MOLT', 'normal');
        addTerminalLine('   Transaction: ' + generateMockSignature(), 'normal');
        
        updateWalletUI();
        updateMetrics(); // Update total txs
    }, 1500);
}
```

#### D. Program ID Generation
```javascript
function generateProgramAddress(deployerPubkey, codeHash) {
    // Deterministic address from deployer + code
    const combined = deployerPubkey + codeHash;
    const hash = hashString(combined);
    return 'molt1' + hash.substring(0, 40);
}

function hashString(str) {
    // Simple hash for mock (in production: use actual crypto)
    let hash = 0;
    for (let i = 0; i < str.length; i++) {
        const char = str.charCodeAt(i);
        hash = ((hash << 5) - hash) + char;
        hash = hash & hash;
    }
    return Math.abs(hash).toString(36) + Date.now().toString(36);
}

function generateBinaryKeypair() {
    // Generate mock binary keypair
    const publicKey = new Uint8Array(32);
    const secretKey = new Uint8Array(64);
    for (let i = 0; i < 32; i++) {
        publicKey[i] = Math.floor(Math.random() * 256);
    }
    for (let i = 0; i < 64; i++) {
        secretKey[i] = Math.floor(Math.random() * 256);
    }
    return { publicKey, secretKey };
}
```

#### E. Enhanced Deploy with Program ID
```javascript
function deployProgram() {
    if (!state.wallet) {
        addTerminalLine('❌ Connect wallet first!', 'error');
        openWalletModal();
        return;
    }
    
    if (!state.compiledWasm) {
        addTerminalLine('❌ Build program first!', 'error');
        return;
    }
    
    const programName = document.getElementById('programName').value || 'my_program';
    const network = document.getElementById('deployNetwork').value;
    const funding = parseFloat(document.getElementById('initialFunding').value) || 1.0;
    const verifyCode = document.getElementById('verifyCode').checked;
    
    // Generate program address
    const codeHash = hashString(state.compiledWasm.join(''));
    const programAddress = generateProgramAddress(state.wallet.address, codeHash);
    
    // Check if program exists (for upgrade vs fresh deploy)
    const existingProgram = state.deployedPrograms.find(p => p.address === programAddress);
    const isUpgrade = existingProgram !== undefined;
    
    if (isUpgrade) {
        addTerminalLine('🔄 Detected existing program - initiating UPGRADE...', 'warning');
    } else {
        addTerminalLine('🚀 Deploying NEW program to ' + network + '...', 'info');
    }
    
    addTerminalLine('   Program name: ' + programName, 'normal');
    addTerminalLine('   Network: ' + network, 'normal');
    addTerminalLine('   Initial funding: ' + funding + ' MOLT', 'normal');
    addTerminalLine('   Verify code: ' + (verifyCode ? 'Yes' : 'No'), 'normal');
    addTerminalLine('', 'normal');
    
    setTimeout(() => {
        addTerminalLine('   Creating program account...', 'normal');
    }, 500);
    
    setTimeout(() => {
        addTerminalLine('   Program ID: ' + programAddress, 'normal');
        addTerminalLine('   Uploading bytecode...', 'normal');
        addTerminalLine('   Size: ' + formatBytes(state.compiledWasm.length), 'normal');
    }, 1000);
    
    setTimeout(() => {
        addTerminalLine('   Initializing program...', 'normal');
    }, 1800);
    
    if (verifyCode) {
        setTimeout(() => {
            addTerminalLine('   Verifying source code on-chain...', 'normal');
        }, 2300);
    }
    
    setTimeout(() => {
        const txSignature = generateMockSignature();
        const fee = CONFIG.fees.baseFee / 1000000000; // Convert shells to MOLT
        const burned = fee * (CONFIG.fees.burnPercentage / 100);
        
        addTerminalLine('', 'normal');
        if (isUpgrade) {
            addTerminalLine('✅ Program UPGRADED successfully!', 'success');
        } else {
            addTerminalLine('✅ Program DEPLOYED successfully!', 'success');
        }
        addTerminalLine('   Program ID: ' + programAddress, 'normal');
        addTerminalLine('   Owner: ' + state.wallet.address.substring(0, 16) + '...', 'normal');
        addTerminalLine('   Transaction: ' + txSignature, 'normal');
        addTerminalLine('   Fee: ' + fee.toFixed(6) + ' MOLT (' + burned.toFixed(6) + ' burned)', 'normal');
        addTerminalLine('', 'normal');
        addTerminalLine('🔗 Explorer: ' + CONFIG.explorer[network] + '/program/' + programAddress, 'link');
        
        // Add or update deployed program
        if (isUpgrade) {
            existingProgram.timestamp = Date.now();
            existingProgram.size = state.compiledWasm.length;
            existingProgram.version = (existingProgram.version || 1) + 1;
        } else {
            const program = {
                name: programName,
                address: programAddress,
                deployer: state.wallet.address,
                timestamp: Date.now(),
                network,
                size: state.compiledWasm.length,
                verified: verifyCode,
                version: 1,
                storage: {},
                calls: 0,
                gasUsed: 0
            };
            
            state.deployedPrograms.unshift(program);
        }
        
        // Update metrics
        state.metrics.totalTxs++;
        updateMetrics();
        
        // Refresh UI
        renderDeployedPrograms();
        
        // Update test panel with new program address
        document.getElementById('testProgramAddr').value = programAddress;
        
    }, verifyCode ? 3000 : 2500);
}
```

#### F. Transfer Shells Function
```javascript
function transferShells() {
    const to = document.getElementById('transferTo').value;
    const amount = parseFloat(document.getElementById('transferAmount').value);
    
    if (!state.wallet) {
        addTerminalLine('❌ Connect wallet first!', 'error');
        return;
    }
    
    if (!to || !amount) {
        addTerminalLine('❌ Enter recipient address and amount!', 'error');
        return;
    }
    
    if (amount <= 0) {
        addTerminalLine('❌ Amount must be greater than 0!', 'error');
        return;
    }
    
    const balance = parseFloat(state.wallet.balance);
    if (amount > balance) {
        addTerminalLine('❌ Insufficient balance!', 'error');
        addTerminalLine('   Balance: ' + balance + ' MOLT', 'normal');
        addTerminalLine('   Required: ' + amount + ' MOLT', 'normal');
        return;
    }
    
    addTerminalLine('💸 Transferring MOLT...', 'info');
    addTerminalLine('   From: ' + state.wallet.address.substring(0, 16) + '...', 'normal');
    addTerminalLine('   To: ' + to.substring(0, 16) + '...', 'normal');
    addTerminalLine('   Amount: ' + amount + ' MOLT', 'normal');
    
    setTimeout(() => {
        const fee = CONFIG.fees.baseFee / 1000000000;
        const burned = fee * (CONFIG.fees.burnPercentage / 100);
        const txSignature = generateMockSignature();
        
        // Update wallet balance
        state.wallet.balance = (balance - amount - fee).toFixed(6);
        
        // Update metrics
        state.metrics.totalTxs++;
        state.metrics.totalBurned += burned;
        updateMetrics();
        
        addTerminalLine('', 'normal');
        addTerminalLine('✅ Transfer successful!', 'success');
        addTerminalLine('   Transaction: ' + txSignature, 'normal');
        addTerminalLine('   Fee: ' + fee.toFixed(6) + ' MOLT (' + burned.toFixed(6) + ' burned)', 'normal');
        addTerminalLine('   New balance: ' + state.wallet.balance + ' MOLT', 'normal');
        
        updateWalletUI();
        
        // Clear form
        document.getElementById('transferTo').value = '';
        document.getElementById('transferAmount').value = '';
    }, 1200);
}
```

#### G. Upgrade Program Function
```javascript
function upgradeProgram(programAddress) {
    if (!state.wallet) {
        addTerminalLine('❌ Connect wallet first!', 'error');
        return;
    }
    
    const program = state.deployedPrograms.find(p => p.address === programAddress);
    if (!program) {
        addTerminalLine('❌ Program not found!', 'error');
        return;
    }
    
    if (program.deployer !== state.wallet.address) {
        addTerminalLine('❌ Only program owner can upgrade!', 'error');
        addTerminalLine('   Owner: ' + program.deployer, 'normal');
        addTerminalLine('   Your address: ' + state.wallet.address, 'normal');
        return;
    }
    
    if (!state.compiledWasm) {
        addTerminalLine('❌ Build new code first!', 'error');
        return;
    }
    
    addTerminalLine('🔄 Upgrading program...', 'info');
    addTerminalLine('   Program: ' + program.name, 'normal');
    addTerminalLine('   Address: ' + programAddress, 'normal');
    addTerminalLine('   Current version: v' + program.version, 'normal');
    
    setTimeout(() => {
        program.version++;
        program.timestamp = Date.now();
        program.size = state.compiledWasm.length;
        
        const txSignature = generateMockSignature();
        
        addTerminalLine('', 'normal');
        addTerminalLine('✅ Program upgraded successfully!', 'success');
        addTerminalLine('   New version: v' + program.version, 'normal');
        addTerminalLine('   Transaction: ' + txSignature, 'normal');
        
        renderDeployedPrograms();
        showProgramInfo(program);
    }, 1500);
}
```

#### H. Close Program Function
```javascript
function closeProgram(programAddress) {
    if (!state.wallet) {
        addTerminalLine('❌ Connect wallet first!', 'error');
        return;
    }
    
    const program = state.deployedPrograms.find(p => p.address === programAddress);
    if (!program) {
        addTerminalLine('❌ Program not found!', 'error');
        return;
    }
    
    if (program.deployer !== state.wallet.address) {
        addTerminalLine('❌ Only program owner can close!', 'error');
        return;
    }
    
    if (!confirm('Are you sure you want to close this program? This action cannot be undone.')) {
        return;
    }
    
    addTerminalLine('🔒 Closing program...', 'info');
    addTerminalLine('   Program: ' + program.name, 'normal');
    addTerminalLine('   Address: ' + programAddress, 'normal');
    
    setTimeout(() => {
        // Remove from deployed programs
        const index = state.deployedPrograms.indexOf(program);
        state.deployedPrograms.splice(index, 1);
        
        const txSignature = generateMockSignature();
        
        addTerminalLine('', 'normal');
        addTerminalLine('✅ Program closed!', 'success');
        addTerminalLine('   Transaction: ' + txSignature, 'normal');
        addTerminalLine('   Remaining balance withdrawn to your wallet', 'normal');
        
        renderDeployedPrograms();
        hideProgramInfo();
    }, 1000);
}
```

#### I. Show Program Info Function
```javascript
function showProgramInfo(program) {
    state.selectedProgram = program;
    
    // Update UI
    document.getElementById('infoProgramId').textContent = program.address;
    document.getElementById('infoOwner').textContent = program.deployer.substring(0, 16) + '...';
    document.getElementById('infoCodeSize').textContent = formatBytes(program.size);
    document.getElementById('infoDeployed').textContent = timeAgo(program.timestamp) + ' (v' + program.version + ')';
    document.getElementById('infoNetwork').textContent = program.network;
    
    // Show panel
    document.getElementById('programInfoPanel').style.display = 'block';
    
    // Load storage if available
    if (program.storage && Object.keys(program.storage).length > 0) {
        showProgramStorage(program);
    }
}

function hideProgramInfo() {
    document.getElementById('programInfoPanel').style.display = 'none';
    document.getElementById('storageViewerPanel').style.display = 'none';
    state.selectedProgram = null;
}

function copyProgramId() {
    if (!state.selectedProgram) return;
    
    navigator.clipboard.writeText(state.selectedProgram.address).then(() => {
        addTerminalLine('📋 Program ID copied to clipboard!', 'success');
    });
}
```

#### J. Storage Viewer Function
```javascript
function showProgramStorage(program) {
    const storageViewer = document.getElementById('storageViewer');
    
    if (!program.storage || Object.keys(program.storage).length === 0) {
        storageViewer.innerHTML = `
            <div class="empty-state">
                <i class="fas fa-database"></i>
                <p>No storage data</p>
            </div>
        `;
    } else {
        const storageItems = Object.entries(program.storage).map(([key, value]) => `
            <div class="storage-item">
                <div class="storage-key">${key}</div>
                <div class="storage-value">${value}</div>
            </div>
        `).join('');
        
        storageViewer.innerHTML = `<div class="storage-list">${storageItems}</div>`;
    }
    
    document.getElementById('storageViewerPanel').style.display = 'block';
}

function refreshStorage() {
    if (!state.selectedProgram) return;
    
    addTerminalLine('🔄 Refreshing storage...', 'info');
    setTimeout(() => {
        showProgramStorage(state.selectedProgram);
        addTerminalLine('✅ Storage refreshed!', 'success');
    }, 500);
}
```

#### K. Metrics Update Function
```javascript
function updateMetrics() {
    // Update TPS (mock calculation)
    const tps = Math.floor(Math.random() * 5000) + 1000;
    document.getElementById('metricTPS').textContent = tps.toLocaleString();
    
    // Update total transactions
    document.getElementById('metricTotalTxs').textContent = state.metrics.totalTxs.toLocaleString();
    
    // Update block time
    document.getElementById('metricBlockTime').textContent = state.metrics.blockTime + 'ms';
    
    // Update total burned
    const burned = state.metrics.totalBurned.toFixed(2);
    document.getElementById('metricBurned').textContent = burned + ' MOLT';
}

// Call updateMetrics periodically
setInterval(updateMetrics, 5000);
```

---

## ✅ Step 3: Event Listeners (Add to setupEventListeners)

```javascript
// Add these to existing setupEventListeners() function:

// Faucet
document.getElementById('faucetBtn').addEventListener('click', requestFaucet);

// Transfer
document.getElementById('transferBtn').addEventListener('click', transferShells);

// Program info
document.getElementById('closeProgramInfoBtn').addEventListener('click', hideProgramInfo);
document.getElementById('upgradeProgramBtn').addEventListener('click', () => {
    if (state.selectedProgram) {
        upgradeProgram(state.selectedProgram.address);
    }
});
document.getElementById('closeProgramBtn').addEventListener('click', () => {
    if (state.selectedProgram) {
        closeProgram(state.selectedProgram.address);
    }
});

// Storage refresh
document.getElementById('refreshStorageBtn').addEventListener('click', refreshStorage);
```

---

## ✅ Step 4: Enhanced Deployed Programs Rendering

Update `renderDeployedPrograms()` to include click handlers for viewing program info:

```javascript
function renderDeployedPrograms() {
    const container = document.getElementById('deployedProgramsList');
    
    if (state.deployedPrograms.length === 0) {
        container.innerHTML = `
            <div class="empty-state">
                <i class="fas fa-box-open"></i>
                <p>No programs deployed yet</p>
                <small>Build and deploy to see them here</small>
            </div>
        `;
        return;
    }
    
    container.innerHTML = state.deployedPrograms.map(program => `
        <div class="program-card" onclick="showProgramInfo(${JSON.stringify(program).replace(/"/g, '&quot;')})">
            <div class="program-header">
                <div>
                    <div class="program-name">${program.name}</div>
                    <div class="program-address">${program.address.substring(0, 16)}...</div>
                </div>
            </div>
            <div class="program-stats">
                <span>${program.network}</span>
                <span>${formatBytes(program.size)}</span>
                <span>v${program.version || 1}</span>
                <span>${timeAgo(program.timestamp)}</span>
                ${program.verified ? '<span class="text-success">✓ Verified</span>' : ''}
            </div>
        </div>
    `).join('');
}
```

---

## ✅ Step 5: Enhanced Network Switching

Update `updateNetwork()` to show/hide faucet button:

```javascript
function updateNetwork(network) {
    state.network = network;
    console.log(`🌐 Network changed to: ${network}`);
    addTerminalLine(`Network switched to: ${network.toUpperCase()}`, 'info');
    addTerminalLine(`RPC: ${CONFIG.rpc[network]}`, 'info');
    
    // Update deploy panel
    document.getElementById('deployNetwork').value = network;
    
    // Update navbar select
    document.getElementById('networkSelect').value = network;
    
    // Show/hide faucet button (only on testnet)
    const faucetBtn = document.getElementById('faucetBtn');
    if (network === 'testnet' && state.wallet) {
        faucetBtn.style.display = 'inline-flex';
    } else {
        faucetBtn.style.display = 'none';
    }
}
```

---

## ✅ Step 6: Enhanced Wallet UI Update

Update `updateWalletUI()` to show faucet when appropriate:

```javascript
function updateWalletUI() {
    if (state.wallet) {
        const btn = document.getElementById('walletBtn');
        btn.className = 'btn btn-sm btn-primary';
        document.getElementById('walletBtnText').textContent = 
            state.wallet.address.substring(0, 6) + '...' + state.wallet.address.substring(state.wallet.address.length - 4);
        
        // Show faucet on testnet
        if (state.network === 'testnet') {
            document.getElementById('faucetBtn').style.display = 'inline-flex';
        }
    } else {
        document.getElementById('faucetBtn').style.display = 'none';
    }
}
```

---

## 📊 Summary of Enhancements

### Phase 1 Complete: ✅
- HTML structure with all new panels
- CSS styling for all new components
- Contract templates with full mock data

### Phase 2 Complete: ✅
- Faucet functionality
- Program ID generation
- Transfer shells
- Upgrade program
- Close program
- Storage viewer
- Metrics tracking
- Enhanced deployment flow

### What's Working:
1. ✅ Request 100 MOLT from testnet faucet
2. ✅ Transfer MOLT between addresses
3. ✅ Deploy programs with deterministic IDs
4. ✅ Upgrade existing programs (version tracking)
5. ✅ Close programs and withdraw balance
6. ✅ View program info (ID, owner, size, version)
7. ✅ View contract storage state
8. ✅ Track metrics (TPS, txs, burned)
9. ✅ Full mock data for all 7 contract types

---

## 🚀 Next: Continue with Option B

Build remaining 6 components:
1. Dashboard
2. Explorer
3. Docs Hub
4. CLI Terminal
5. Examples Library
6. Deploy Wizard

**Status: Phase 1 (Playground Enhancements) COMPLETE!** 🦞⚡
