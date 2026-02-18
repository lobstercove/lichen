// MoltWallet - Core Wallet Logic
// Full RPC integration, wallet management, and UI controls

// ── Number formatting helpers ──
function fmtToken(value) {
    return Number(value).toLocaleString(undefined, { maximumFractionDigits: 9 });
}
function fmtUsd(value, sym = '$') {
    return sym + Number(value).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 6 });
}

// Network configuration
const NETWORKS = {
    'mainnet': 'https://rpc.moltchain.network',
    'testnet': 'https://testnet-rpc.moltchain.network',
    'local-testnet': 'http://localhost:8899',
    'local-mainnet': 'http://localhost:8899'
};

const WS_ENDPOINTS = {
    'mainnet': 'wss://rpc.moltchain.network/ws',
    'testnet': 'wss://testnet-rpc.moltchain.network/ws',
    'local-testnet': 'ws://localhost:8900',
    'local-mainnet': 'ws://localhost:8900'
};

const CUSTODY_ENDPOINTS = {
    'mainnet': 'https://custody.moltchain.network',
    'testnet': 'https://testnet-custody.moltchain.network',
    'local-testnet': 'http://localhost:9105',
    'local-mainnet': 'http://localhost:9105'
};

function getSelectedNetwork() {
    return localStorage.getItem('moltchain_wallet_network') || 'local-testnet';
}

function getRpcEndpoint() {
    return NETWORKS[getSelectedNetwork()] || NETWORKS['local-testnet'];
}

function getWsEndpoint() {
    return WS_ENDPOINTS[getSelectedNetwork()] || WS_ENDPOINTS['local-testnet'];
}

function getCustodyEndpoint() {
    return CUSTODY_ENDPOINTS[getSelectedNetwork()] || CUSTODY_ENDPOINTS['local-testnet'];
}

// ===== LIVE BALANCE WEBSOCKET =====
let balanceWs = null;
let balanceWsSubId = null;
let bridgeLockSubId = null;
let bridgeMintSubId = null;
let balanceWsReconnectTimer = null;
let balanceWsSubscribedAddress = null;
let bridgeWsActive = false;

function connectBalanceWebSocket() {
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    // Don't reconnect if already subscribed to this address
    if (balanceWs && balanceWs.readyState === WebSocket.OPEN && balanceWsSubscribedAddress === wallet.address) {
        return;
    }
    
    // Close existing connection
    disconnectBalanceWebSocket();
    
    const wsUrl = getWsEndpoint();
    // console.log(`[WS] Connecting to ${wsUrl} for account ${wallet.address}`);
    
    try {
        balanceWs = new WebSocket(wsUrl);
    } catch (e) {
        console.warn('[WS] Failed to create WebSocket:', e);
        scheduleWsReconnect();
        return;
    }
    
    balanceWs.onopen = () => {
        // console.log('[WS] Connected, subscribing to account changes');
        balanceWsSubscribedAddress = wallet.address;
        // Subscribe to account balance changes
        balanceWs.send(JSON.stringify({
            jsonrpc: '2.0',
            id: 1,
            method: 'subscribeAccount',
            params: wallet.address
        }));
        // P0-FIX: Subscribe to bridge events for real-time deposit status
        balanceWs.send(JSON.stringify({
            jsonrpc: '2.0',
            id: 2,
            method: 'subscribeBridgeLocks',
            params: null
        }));
        balanceWs.send(JSON.stringify({
            jsonrpc: '2.0',
            id: 3,
            method: 'subscribeBridgeMints',
            params: null
        }));
    };
    
    balanceWs.onmessage = (event) => {
        try {
            const msg = JSON.parse(event.data);
            
            // Subscription confirmations
            if (msg.id === 1 && msg.result !== undefined) {
                balanceWsSubId = msg.result;
                return;
            }
            if (msg.id === 2 && msg.result !== undefined) {
                bridgeLockSubId = msg.result;
                bridgeWsActive = true;
                return;
            }
            if (msg.id === 3 && msg.result !== undefined) {
                bridgeMintSubId = msg.result;
                bridgeWsActive = true;
                return;
            }
            
            // Notification dispatch
            if (msg.method === 'subscription' && msg.params) {
                const subId = msg.params.subscription;
                const result = msg.params.result;
                
                // Balance notification
                if (subId === balanceWsSubId) {
                    refreshBalance();
                    loadAssets();
                    loadActivity();
                    // Refresh staking tab if visible — catches ReefStake deposit/unstake
                    refreshStakingIfVisible();
                    return;
                }

                // Bridge lock event — deposit detected on source chain
                if (subId === bridgeLockSubId && result) {
                    handleBridgeLockEvent(result);
                    return;
                }

                // Bridge mint event — wrapped tokens minted on MoltChain
                if (subId === bridgeMintSubId && result) {
                    handleBridgeMintEvent(result);
                    return;
                }
            }
        } catch (e) {
            console.warn('[WS] Failed to parse message:', e);
        }
    };
    
    balanceWs.onclose = (event) => {
        // console.log(`[WS] Disconnected (code: ${event.code})`);
        balanceWsSubId = null;
        bridgeLockSubId = null;
        bridgeMintSubId = null;
        bridgeWsActive = false;
        balanceWsSubscribedAddress = null;
        scheduleWsReconnect();
    };
    
    balanceWs.onerror = (error) => {
        console.warn('[WS] Error:', error);
    };
}

function handleBridgeLockEvent(data) {
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    // data: { chain, asset, amount, sender, recipient }
    // Check if this lock is relevant to our wallet (recipient matches)
    if (data.recipient !== wallet.address) return;
    
    // console.log('[Bridge] Lock event for our wallet:', data);

    // Update deposit status UI if visible
    const statusEl = document.getElementById('depositStatus');
    if (statusEl) {
        statusEl.innerHTML = `<i class="fas fa-check-circle" style="color: #06D6A0;"></i> <span>Deposit confirmed on ${escapeHtml(data.chain)}! Sweeping to treasury...</span>`;
    }
    showToast(`Bridge deposit confirmed on ${escapeHtml(data.chain) || 'source chain'}!`);
}

function handleBridgeMintEvent(data) {
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    // data: { chain, asset, amount, recipient, tx_hash }
    // Check if this mint is for our wallet
    if (data.recipient !== wallet.address) return;
    
    // console.log('[Bridge] Mint event for our wallet:', data);

    // Update deposit status UI if visible
    const statusEl = document.getElementById('depositStatus');
    if (statusEl) {
        statusEl.innerHTML = `<i class="fas fa-check-double" style="color: #06D6A0;"></i> <span>Credited to your MoltChain wallet!</span>`;
    }
    
    // Stop polling — we got the final status via WS
    stopDepositPolling();
    
    const amount = data.amount ? ` (${data.amount} ${(data.asset || '').toUpperCase()})` : '';
    showToast(`Bridge deposit credited${amount}!`, 'success');
    
    // Refresh balance to show new tokens
    refreshBalance();
    loadAssets();
}

function disconnectBalanceWebSocket() {
    if (balanceWsReconnectTimer) {
        clearTimeout(balanceWsReconnectTimer);
        balanceWsReconnectTimer = null;
    }
    if (balanceWs) {
        // Unsubscribe before closing
        if (balanceWs.readyState === WebSocket.OPEN) {
            if (balanceWsSubId !== null) {
                balanceWs.send(JSON.stringify({
                    jsonrpc: '2.0', id: 2,
                    method: 'unsubscribeAccount',
                    params: balanceWsSubId
                }));
            }
            if (bridgeLockSubId !== null) {
                balanceWs.send(JSON.stringify({
                    jsonrpc: '2.0', id: 12,
                    method: 'unsubscribeBridgeLocks',
                    params: bridgeLockSubId
                }));
            }
            if (bridgeMintSubId !== null) {
                balanceWs.send(JSON.stringify({
                    jsonrpc: '2.0', id: 13,
                    method: 'unsubscribeBridgeMints',
                    params: bridgeMintSubId
                }));
            }
        }
        balanceWs.onclose = null; // Prevent reconnect on intentional close
        balanceWs.close();
        balanceWs = null;
    }
    balanceWsSubId = null;
    bridgeLockSubId = null;
    bridgeMintSubId = null;
    bridgeWsActive = false;
    balanceWsSubscribedAddress = null;
}

function scheduleWsReconnect() {
    if (balanceWsReconnectTimer) return;
    balanceWsReconnectTimer = setTimeout(() => {
        balanceWsReconnectTimer = null;
        const dashboard = document.getElementById('walletDashboard');
        if (dashboard && dashboard.style.display !== 'none') {
            connectBalanceWebSocket();
        }
    }, 5000);
}

// ===== HTTP BALANCE POLLING FALLBACK =====
// Polls for balance updates via RPC as a supplement to WebSocket.
let _balancePollTimer = null;

function startBalancePolling() {
    if (_balancePollTimer) return;
    _balancePollTimer = setInterval(async () => {
        const dashboard = document.getElementById('walletDashboard');
        if (!dashboard || dashboard.style.display === 'none') return;
        try { await refreshBalance(); await loadActivity(); } catch (_) { /* ignore */ }
    }, 8000); // Poll every 8 seconds as supplement
}

function stopBalancePolling() {
    if (_balancePollTimer) {
        clearInterval(_balancePollTimer);
        _balancePollTimer = null;
    }
}

// ── Bincode Message Serializer ──
// Produces the same bytes as Rust's `bincode::serialize(&Message)` so signatures match.
// Message = { instructions: Vec<Instruction>, recent_blockhash: Hash([u8;32]) }
// Instruction = { program_id: Pubkey([u8;32]), accounts: Vec<Pubkey>, data: Vec<u8> }
function serializeMessageBincode(message) {
    const parts = [];

    // Helper: write u64 little-endian (8 bytes) — bincode uses fixint u64 for Vec lengths
    function writeU64LE(n) {
        const buf = new ArrayBuffer(8);
        const view = new DataView(buf);
        view.setBigUint64(0, BigInt(n), true);
        parts.push(new Uint8Array(buf));
    }

    // Helper: write raw bytes
    function writeBytes(bytes) {
        parts.push(new Uint8Array(bytes));
    }

    // instructions: Vec<Instruction>
    const ixs = message.instructions || [];
    writeU64LE(ixs.length);
    for (const ix of ixs) {
        // program_id: [u8; 32] — fixed-size, no length prefix
        writeBytes(ix.program_id);
        // accounts: Vec<Pubkey> — u64 length + N * 32 bytes
        const accounts = ix.accounts || [];
        writeU64LE(accounts.length);
        for (const acct of accounts) {
            writeBytes(acct);
        }
        // data: Vec<u8> — u64 length + N bytes
        const data = ix.data || [];
        writeU64LE(data.length);
        writeBytes(data);
    }

    // recent_blockhash: Hash([u8; 32]) — parse hex string to 32 bytes
    const hashHex = message.blockhash || message.recent_blockhash;
    const hashBytes = new Uint8Array(32);
    for (let i = 0; i < 32; i++) {
        hashBytes[i] = parseInt(hashHex.substr(i * 2, 2), 16);
    }
    writeBytes(hashBytes);

    // Concatenate all parts
    const totalLen = parts.reduce((s, p) => s + p.length, 0);
    const result = new Uint8Array(totalLen);
    let offset = 0;
    for (const p of parts) {
        result.set(p, offset);
        offset += p.length;
    }
    return result;
}

// RPC Client (same as explorer)
class MoltChainRPC {
    constructor(url) {
        this.url = url;
    }
    
    async call(method, params = []) {
        try {
            const response = await fetch(this.url, {
                method: 'POST',
                headers: {'Content-Type': 'application/json'},
                body: JSON.stringify({
                    jsonrpc: '2.0',
                    id: Date.now(),
                    method,
                    params
                })
            });
            const data = await response.json();
            if (data.error) {
                throw new Error(data.error.message || 'RPC Error');
            }
            return data.result;
        } catch (error) {
            // Don't log expected errors (e.g. new wallets with no on-chain account)
            if (!error.message?.includes('Account not found') && !error.message?.includes('does not exist on-chain')) {
                console.error('RPC Call Failed:', error);
            }
            throw error;
        }
    }
    
    async getBalance(address) { return this.call('getBalance', [address]); }
    async getAccount(address) { return this.call('getAccount', [address]); }
    async sendTransaction(txData) { return this.call('sendTransaction', [txData]); }
    async getLatestBlock() { return this.call('getLatestBlock'); }
    async getTokenBalance(tokenProgram, holder) { return this.call('getTokenBalance', [tokenProgram, holder]); }
    async getContractInfo(contractId) { return this.call('getContractInfo', [contractId]); }
}

const rpc = new MoltChainRPC(getRpcEndpoint());

// ── Wrapped Token Registry ──
// Token contract addresses — loaded from deploy manifest or configured manually
const TOKEN_REGISTRY = {
    mUSD: { symbol: 'mUSD', name: 'Molt USD',     decimals: 6, icon: '💵', address: null, color: '#4ade80' },
    wSOL: { symbol: 'wSOL', name: 'Wrapped SOL',  decimals: 9, icon: '◎',  address: null, color: '#9945FF' },
    wETH: { symbol: 'wETH', name: 'Wrapped ETH',  decimals: 9, icon: '⟠',  address: null, color: '#627EEA' },
    REEF: { symbol: 'REEF', name: 'Reef Token',    decimals: 9, icon: '🪸', address: null, color: '#a855f7' },
};

// Load deploy manifest to get token contract addresses
async function loadTokenRegistry() {
    try {
        // Try loading from the RPC node's manifest endpoint
        const endpoint = getRpcEndpoint().replace(/\/+$/, '');
        const response = await fetch(`${endpoint}/deploy-manifest.json`).catch(() => null);
        if (response && response.ok) {
            const manifest = await response.json();
            if (manifest.token_contracts) {
                for (const [symbol, addr] of Object.entries(manifest.token_contracts)) {
                    if (TOKEN_REGISTRY[symbol] && addr) {
                        TOKEN_REGISTRY[symbol].address = addr;
                    }
                }
                // console.log('Token registry loaded from manifest');
                return;
            }
        }
    } catch (e) {
        // Silently fall through to localStorage fallback
    }
    
    // Fallback: try localStorage (user can paste addresses in settings)
    try {
        const stored = localStorage.getItem('moltchain_token_addresses');
        if (stored) {
            const addrs = JSON.parse(stored);
            for (const [symbol, addr] of Object.entries(addrs)) {
                if (TOKEN_REGISTRY[symbol] && addr) {
                    TOKEN_REGISTRY[symbol].address = addr;
                }
            }
            // console.log('Token registry loaded from localStorage');
        }
    } catch (e) {
        console.warn('Could not load stored token addresses:', e);
    }
}

// Get token balance for a specific token
async function getTokenBalanceFormatted(symbol, holderAddress) {
    const token = TOKEN_REGISTRY[symbol];
    if (!token || !token.address) return 0;
    
    try {
        const result = await rpc.getTokenBalance(token.address, holderAddress);
        const rawBalance = result.balance || 0;
        return rawBalance / Math.pow(10, token.decimals);
    } catch (e) {
        return 0;
    }
}

// Get all token balances for a wallet
async function getAllTokenBalances(walletAddress) {
    const balances = {};
    const promises = Object.keys(TOKEN_REGISTRY).map(async (symbol) => {
        balances[symbol] = await getTokenBalanceFormatted(symbol, walletAddress);
    });
    await Promise.all(promises);
    return balances;
}

// Wallet State
let walletState = {
    wallets: [],
    activeWalletId: null,
    isLocked: true,
    network: 'mainnet', // 'mainnet' or 'testnet'
    settings: {
        currency: 'USD',
        lockTimeout: 300000 // 5 minutes
    }
};

// Initialize
document.addEventListener('DOMContentLoaded', () => {
    // console.log('MoltWallet loaded');
    loadWalletState();
    loadTokenRegistry();
    checkWalletStatus();
    setupEventListeners();
    initNetworkSelector();
});

// Load wallet state from localStorage
function loadWalletState() {
    const stored = localStorage.getItem('moltWalletState');
    if (stored) {
        walletState = JSON.parse(stored);
    }
}

// Save wallet state to localStorage
function saveWalletState() {
    localStorage.setItem('moltWalletState', JSON.stringify(walletState));
}

// Check if wallet exists and show appropriate screen
function checkWalletStatus() {
    if (walletState.wallets.length === 0) {
        showScreen('welcomeScreen');
    } else if (walletState.isLocked) {
        showUnlockScreen();
    } else {
        showDashboard();
    }
}

// Show unlock screen
function showUnlockScreen() {
    showScreen('welcomeScreen');
    const container = document.querySelector('.welcome-container');
    container.innerHTML = `
        <div class="unlock-card">
            <div class="welcome-logo">
                <img src="MoltWallet_Logo_256.png" class="logo-icon" alt="MoltWallet">
                <h1>MoltWallet</h1>
            </div>
            <p class="unlock-greeting">Welcome back!</p>
            
            <div class="unlock-form">
                <label class="unlock-label">Enter Password</label>
                <input type="password" id="unlockPassword" class="form-input unlock-input" placeholder="Password" 
                       onkeypress="if(event.key==='Enter') unlockWallet()" autofocus>
            </div>
            
            <button class="btn btn-primary unlock-btn" onclick="unlockWallet()">
                <i class="fas fa-unlock"></i> Unlock Wallet
            </button>
            <div class="unlock-logout">
                <button class="btn btn-danger btn-small" onclick="logoutWallet()">
                    <i class="fas fa-sign-out-alt"></i> Logout
                </button>
            </div>
        </div>
    `;
}

// Unlock wallet with password
async function unlockWallet() {
    const password = document.getElementById('unlockPassword').value;
    
    if (!password) {
        alert('Please enter password');
        return;
    }
    
    try {
        // Try to decrypt first wallet as validation
        const firstWallet = walletState.wallets[0];
        await MoltCrypto.decryptPrivateKey(firstWallet.encryptedKey, password);
        
        // Success - unlock and show dashboard
        walletState.isLocked = false;
        saveWalletState();
        showToast('Wallet unlocked!');
        showDashboard();
        resetLockTimer();
        
    } catch (error) {
        alert('Incorrect password');
        document.getElementById('unlockPassword').value = '';
    }
}

// Security: clear all sensitive input fields across all screens
function clearAllInputs() {
    document.querySelectorAll('input, textarea').forEach(el => {
        if (el.type !== 'hidden' && el.type !== 'checkbox' && el.type !== 'radio') {
            el.value = '';
        }
    });
}

// Show specific screen
function showScreen(screenId) {
    clearAllInputs();
    document.querySelectorAll('.welcome-screen, .wallet-screen, .wallet-dashboard').forEach(el => {
        el.style.display = 'none';
    });
    document.getElementById(screenId).style.display = 'block';
}

// ===== WELCOME SCREEN =====
function showWelcome() {
    showScreen('welcomeScreen');
}

function showCreateWallet() {
    showScreen('createWalletScreen');
    document.querySelectorAll('.create-step').forEach(s => s.classList.remove('active'));
    document.querySelectorAll('.wizard-step-item').forEach(s => s.classList.remove('active'));
    document.querySelector('.create-step[data-step="1"]').classList.add('active');
    document.querySelector('.wizard-step-item[data-step="1"]').classList.add('active');
}

function showImportWallet() {
    showScreen('importWalletScreen');
    setupImportTabs();
}

// ===== CREATE WALLET FLOW =====
let createdMnemonic = '';
let createdKeypair = null;

async function createWalletStep2() {
    const password = document.getElementById('createPassword').value;
    const confirm = document.getElementById('confirmPassword').value;
    
    if (!password || password.length < 8) {
        alert('Password must be at least 8 characters');
        return;
    }
    
    if (password !== confirm) {
        alert('Passwords do not match');
        return;
    }
    
    // Generate mnemonic
    createdMnemonic = MoltCrypto.generateMnemonic();
    createdKeypair = await MoltCrypto.mnemonicToKeypair(createdMnemonic);
    
    // Display seed phrase
    const seedDisplay = document.getElementById('seedPhraseDisplay');
    const seedActions = document.getElementById('seedPhraseActions');
    const words = createdMnemonic.split(' ');
    
    seedDisplay.innerHTML = words.map((word, i) => `
        <div class="seed-word">
            <span class="seed-word-number">${i + 1}.</span>
            <span>${word}</span>
        </div>
    `).join('');
    
    seedActions.innerHTML = `
        <button class="btn btn-sm btn-secondary" onclick="copyMnemonic()">
            <i class="fas fa-copy"></i> Copy
        </button>
        <button class="btn btn-sm btn-secondary" onclick="downloadMnemonic()">
            <i class="fas fa-download"></i> Download
        </button>
    `;
    
    // Move to step 2
    document.querySelectorAll('.create-step').forEach(s => s.classList.remove('active'));
    document.querySelectorAll('.wizard-step-item').forEach(s => s.classList.remove('active'));
    document.querySelector('.create-step[data-step="2"]').classList.add('active');
    document.querySelector('.wizard-step-item[data-step="2"]').classList.add('active');
}

function copyMnemonic() {
    navigator.clipboard.writeText(createdMnemonic).then(() => {
        showToast('✅ Seed phrase copied to clipboard!');
    }).catch(() => {
        showToast('❌ Failed to copy');
    });
}

function downloadMnemonic() {
    const wallet = getActiveWallet() || { name: 'new-wallet' };
    const filename = `molt-wallet-seed-${wallet.name}-${Date.now()}.txt`;
    const content = `MoltWallet Seed Phrase\n` +
                   `DO NOT SHARE THIS WITH ANYONE!\n\n` +
                   `Wallet: ${wallet.name}\n` +
                   `Created: ${new Date().toISOString()}\n\n` +
                   `Seed Phrase (12 words):\n${createdMnemonic}\n\n` +
                   `⚠️ Anyone with this phrase can access your funds!\n` +
                   `Keep it safe and offline.`;
    
    const blob = new Blob([content], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);
    showToast('✅ Seed phrase downloaded!');
}

function createWalletStep3() {
    // Show confirm interface
    const words = createdMnemonic.split(' ');
    const seedConfirm = document.getElementById('seedConfirm');
    
    // Create word buttons (shuffled)
    const shuffled = [...words].sort(() => Math.random() - 0.5);
    seedConfirm.innerHTML = `
        <div class="confirm-section">
            <p class="confirm-section-label">Select your seed words in the correct order (1-12):</p>
            <div class="confirm-slots-grid" id="confirmedWords">
                ${Array.from({length: 12}, (_, i) => `
                    <div class="confirm-slot" data-index="${i}" onclick="removeWordAt(${i})">
                        <span class="slot-number">${i + 1}.</span>
                    </div>
                `).join('')}
            </div>
        </div>
        <div class="confirm-section">
            <p class="confirm-section-label">Available words:</p>
            <div class="seed-word-pool">
                ${shuffled.map(word => `
                    <button class="confirm-word" onclick="selectWord('${word}')" data-word="${word}">
                        ${word}
                    </button>
                `).join('')}
            </div>
        </div>
    `;
    
    window.selectedWords = [];
    
    // Move to step 3
    document.querySelectorAll('.create-step').forEach(s => s.classList.remove('active'));
    document.querySelectorAll('.wizard-step-item').forEach(s => s.classList.remove('active'));
    document.querySelector('.create-step[data-step="3"]').classList.add('active');
    document.querySelector('.wizard-step-item[data-step="3"]').classList.add('active');
}

function selectWord(word) {
    if (!window.selectedWords) window.selectedWords = [];
    
    // Find next empty slot
    const nextIndex = window.selectedWords.length;
    if (nextIndex >= 12) return;
    
    window.selectedWords.push(word);
    
    // Fill the slot
    const slot = document.querySelector(`.confirm-slot[data-index="${nextIndex}"]`);
    if (slot) {
        slot.classList.add('filled');
        slot.innerHTML = `<span class="slot-number">${nextIndex + 1}.</span> ${word}`;
    }
    
    // Mark word button as used
    const btn = document.querySelector(`button.confirm-word[data-word="${word}"]`);
    if (btn) btn.classList.add('used');
    
    // Check if complete
    if (window.selectedWords.length === 12) {
        const correct = window.selectedWords.join(' ') === createdMnemonic;
        document.getElementById('confirmBtn').disabled = !correct;
        
        if (!correct) {
            setTimeout(() => {
                alert('Words are in wrong order. Try again!');
                createWalletStep3();
            }, 500);
        }
    }
}

function removeWordAt(index) {
    if (!window.selectedWords || index >= window.selectedWords.length) return;
    
    const word = window.selectedWords[index];
    
    // Remove this word and shift everything after it
    window.selectedWords.splice(index, 1);
    
    // Re-enable the word button
    const btn = document.querySelector(`button.confirm-word[data-word="${word}"]`);
    if (btn) btn.classList.remove('used');
    
    // Rebuild all slots from current state
    for (let i = 0; i < 12; i++) {
        const slot = document.querySelector(`.confirm-slot[data-index="${i}"]`);
        if (!slot) continue;
        if (i < window.selectedWords.length) {
            slot.classList.add('filled');
            slot.innerHTML = `<span class="slot-number">${i + 1}.</span> ${window.selectedWords[i]}`;
        } else {
            slot.classList.remove('filled');
            slot.innerHTML = `<span class="slot-number">${i + 1}.</span>`;
        }
    }
}

function removeWord(index) {
    removeWordAt(index);
}

function updateConfirmedWords() {
    // Slots are updated directly in selectWord/removeWordAt
}

async function finishCreateWallet() {
    const password = document.getElementById('createPassword').value;
    
    // Encrypt private key
    const encrypted = await MoltCrypto.encryptPrivateKey(createdKeypair.privateKey, password);
    
    // Create wallet object
    // Encrypt mnemonic alongside the private key (same password, separate ciphertext)
    const encryptedMnemonic = await MoltCrypto.encryptPrivateKey(createdMnemonic, password);
    
    const wallet = {
        id: MoltCrypto.generateId(),
        name: `Wallet ${walletState.wallets.length + 1}`,
        address: createdKeypair.address,
        publicKey: createdKeypair.publicKey,
        encryptedKey: encrypted,
        encryptedMnemonic: encryptedMnemonic,
        hasMnemonic: true,
        createdAt: Date.now()
    };
    
    walletState.wallets.push(wallet);
    walletState.activeWalletId = wallet.id;
    walletState.isLocked = false;
    saveWalletState();
    
    showToast('Wallet created successfully!');
    showDashboard();
    
    // Auto-register EVM address for MetaMask compatibility (non-blocking)
    registerEvmAddress(wallet, password);
}

// ===== IMPORT WALLET =====
function setupImportTabs() {
    const tabs = document.querySelectorAll('.import-tab');
    const methods = document.querySelectorAll('.import-method');
    
    tabs.forEach(tab => {
        tab.addEventListener('click', () => {
            const method = tab.dataset.method;
            
            tabs.forEach(t => t.classList.remove('active'));
            methods.forEach(m => m.classList.remove('active'));
            
            tab.classList.add('active');
            document.querySelector(`.import-method[data-method="${method}"]`).classList.add('active');
        });
    });
    
    // File upload handler
    document.getElementById('importJsonFile').addEventListener('change', (e) => {
        const file = e.target.files[0];
        if (file) {
            document.getElementById('fileName').textContent = file.name;
        }
    });
}

async function importWalletSeed() {
    const seed = document.getElementById('importSeed').value.trim();
    const password = document.getElementById('importPassword').value;
    
    if (!MoltCrypto.isValidMnemonic(seed)) {
        alert('Invalid seed phrase');
        return;
    }
    
    if (!password || password.length < 8) {
        alert('Password must be at least 8 characters');
        return;
    }
    
    const keypair = await MoltCrypto.mnemonicToKeypair(seed);
    const encrypted = await MoltCrypto.encryptPrivateKey(keypair.privateKey, password);
    const encryptedMnemonic = await MoltCrypto.encryptPrivateKey(seed, password);
    
    const wallet = {
        id: MoltCrypto.generateId(),
        name: `Wallet ${walletState.wallets.length + 1}`,
        address: keypair.address,
        publicKey: keypair.publicKey,
        encryptedKey: encrypted,
        encryptedMnemonic: encryptedMnemonic,
        hasMnemonic: true,
        createdAt: Date.now()
    };
    
    walletState.wallets.push(wallet);
    walletState.activeWalletId = wallet.id;
    walletState.isLocked = false;
    saveWalletState();
    
    showToast('Wallet imported successfully!');
    showDashboard();
    
    // Auto-register EVM address for MetaMask compatibility (non-blocking)
    registerEvmAddress(wallet, password);
}

async function importWalletPrivateKey() {
    const privateKey = document.getElementById('importPrivateKey').value.trim();
    const password = document.getElementById('importPasswordPrivate').value;
    
    if (!privateKey || privateKey.length !== 64) {
        alert('Invalid private key format');
        return;
    }
    
    if (!password || password.length < 8) {
        alert('Password must be at least 8 characters');
        return;
    }
    
    const publicKey = await MoltCrypto.derivePublicKey(MoltCrypto.hexToBytes(privateKey));
    const address = MoltCrypto.publicKeyToAddress(publicKey);
    const encrypted = await MoltCrypto.encryptPrivateKey(privateKey, password);
    
    const wallet = {
        id: MoltCrypto.generateId(),
        name: `Wallet ${walletState.wallets.length + 1}`,
        address,
        publicKey: MoltCrypto.bytesToHex(publicKey),
        encryptedKey: encrypted,
        createdAt: Date.now()
    };
    
    walletState.wallets.push(wallet);
    walletState.activeWalletId = wallet.id;
    walletState.isLocked = false;
    saveWalletState();
    
    showToast('Wallet imported successfully!');
    showDashboard();
    
    // Auto-register EVM address for MetaMask compatibility (non-blocking)
    registerEvmAddress(wallet, password);
}

async function importWalletJson() {
    const file = document.getElementById('importJsonFile').files[0];
    const password = document.getElementById('importPasswordJson').value;
    
    if (!file) {
        alert('Please select a JSON file');
        return;
    }
    
    const reader = new FileReader();
    reader.onload = async (e) => {
        try {
            const keystore = JSON.parse(e.target.result);
            
            if (!keystore.secretKey && !keystore.privateKey) {
                alert('Invalid keystore format: no key data found');
                return;
            }
            
            // Extract private key (seed) from keystore
            let seedHex;
            if (keystore.secretKey) {
                // Full 64-byte secretKey — first 32 bytes are the seed
                const secretBytes = new Uint8Array(keystore.secretKey);
                seedHex = MoltCrypto.bytesToHex(secretBytes.slice(0, 32));
            } else if (typeof keystore.privateKey === 'string') {
                seedHex = keystore.privateKey;
            } else {
                const privBytes = new Uint8Array(keystore.privateKey);
                seedHex = MoltCrypto.bytesToHex(privBytes.slice(0, 32));
            }
            
            // Reconstruct keypair
            const seed = MoltCrypto.hexToBytes(seedHex);
            const keypair = nacl.sign.keyPair.fromSeed(seed);
            const address = MoltCrypto.publicKeyToAddress(keypair.publicKey);
            
            if (!password || password.length < 8) {
                alert('Password must be at least 8 characters');
                return;
            }
            
            const encrypted = await MoltCrypto.encryptPrivateKey(seedHex, password);
            
            const wallet = {
                id: MoltCrypto.generateId(),
                name: keystore.name || `Imported ${walletState.wallets.length + 1}`,
                address: address,
                publicKey: MoltCrypto.bytesToHex(keypair.publicKey),
                encryptedKey: encrypted,
                createdAt: Date.now()
            };
            
            walletState.wallets.push(wallet);
            walletState.activeWalletId = wallet.id;
            walletState.isLocked = false;
            saveWalletState();
            
            showToast('✅ Wallet imported from JSON keystore!');
            showDashboard();
            
            // Auto-register EVM address for MetaMask compatibility (non-blocking)
            registerEvmAddress(wallet, password);
        } catch (error) {
            alert('Invalid JSON file: ' + error.message);
        }
    };
    reader.readAsText(file);
}

// ===== DASHBOARD =====
async function showDashboard() {
    showScreen('walletDashboard');
    setupDashboardTabs();
    setupWalletSelector();
    await refreshBalance();
    await loadAssets();
    await loadActivity();
    await loadStaking();
    connectBalanceWebSocket();
    startBalancePolling();
}

function setupDashboardTabs() {
    const tabs = document.querySelectorAll('.dashboard-tab');
    const contents = document.querySelectorAll('.tab-content');
    
    tabs.forEach(tab => {
        tab.addEventListener('click', () => {
            const tabName = tab.dataset.tab;
            
            tabs.forEach(t => t.classList.remove('active'));
            contents.forEach(c => c.classList.remove('active'));
            
            tab.classList.add('active');
            document.querySelector(`.tab-content[data-tab="${tabName}"]`).classList.add('active');
            
            // Refresh data when staking tab is clicked
            if (tabName === 'staking') {
                loadStaking();
            }
            if (tabName === 'nfts') {
                refreshNFTs();
            }
            if (tabName === 'identity' && typeof loadIdentity === 'function') {
                loadIdentity();
            }
        });
    });
}

function setupWalletSelector() {
    const btn = document.getElementById('walletSelectorBtn');
    const dropdown = document.getElementById('walletDropdown');
    
    // Only attach the click listener ONCE (prevent stacking on re-render)
    if (!btn._dropdownBound) {
        btn.addEventListener('click', (e) => {
            e.stopPropagation();
            dropdown.classList.toggle('show');
        });
        btn._dropdownBound = true;
    }
    
    // Populate dropdown with inline layout: "WalletName  address..." on one row
    dropdown.innerHTML = walletState.wallets.map(w => {
        const shortAddr = w.address.slice(0, 12) + '...';
        return `
        <div class="wallet-dropdown-item" onclick="switchWallet('${w.id}')" style="display: flex; align-items: center; gap: 0.5rem; white-space: nowrap;">
            <strong style="flex-shrink: 0;">${w.name}</strong>
            <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.78rem; color: var(--text-muted); overflow: hidden; text-overflow: ellipsis;">${shortAddr}</span>
        </div>`;
    }).join('') + `
        <div class="wallet-dropdown-item" onclick="showCreateWalletFromDashboard()" style="color: var(--primary); font-weight: 600; display: flex; align-items: center; gap: 0.5rem;">
            <i class="fas fa-plus"></i> Create New Wallet
        </div>
        <div class="wallet-dropdown-item" onclick="showImportWalletFromDashboard()" style="color: var(--success); font-weight: 600; display: flex; align-items: center; gap: 0.5rem;">
            <i class="fas fa-download"></i> Import Wallet
        </div>
    `;
    
    // Update current wallet name
    const activeWallet = getActiveWallet();
    if (activeWallet) {
        document.getElementById('currentWalletName').textContent = activeWallet.name;
    }
}

function getActiveWallet() {
    return walletState.wallets.find(w => w.id === walletState.activeWalletId);
}

function switchWallet(walletId) {
    walletState.activeWalletId = walletId;
    saveWalletState();
    // Close dropdown before refreshing dashboard
    document.getElementById('walletDropdown').classList.remove('show');
    // Reconnect WS + polling for new wallet address
    stopBalancePolling();
    disconnectBalanceWebSocket();
    showDashboard();
}

async function refreshBalance() {
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    try {
        const balance = await rpc.getBalance(wallet.address);
        const molt = parseFloat(balance.molt) || 0;
        
        // Fetch all token balances in parallel
        const tokenBalances = await getAllTokenBalances(wallet.address);
        
        // Calculate total USD value (using mock prices)
        const MOCK_PRICES = { MOLT: 0.10, mUSD: 1.0, wSOL: 150.0, wETH: 3000.0, REEF: 0.05 };
        let totalUsd = molt * MOCK_PRICES.MOLT;
        for (const [symbol, bal] of Object.entries(tokenBalances)) {
            totalUsd += bal * (MOCK_PRICES[symbol] || 0);
        }
        
        // Use saved display settings
        const settings = walletState.settings || {};
        const decimals = settings.decimals || 6;
        const currency = settings.currency || 'USD';
        const currencySymbols = { USD: '$', EUR: '€', GBP: '£', JPY: '¥' };
        const sym = currencySymbols[currency] || '$';
        
        document.getElementById('totalBalance').textContent = `${fmtUsd(totalUsd, sym)} ${currency}`;
        document.getElementById('balanceUsd').textContent = `${fmtToken(molt)} MOLT`;

        // Balance breakdown — show spendable/staked/locked/reef split when non-trivial
        const breakdownEl = document.getElementById('balanceBreakdown');
        if (breakdownEl) {
            const spendable = parseFloat(balance.spendable_molt) || 0;
            const staked = parseFloat(balance.staked_molt) || 0;
            const locked = parseFloat(balance.locked_molt) || 0;
            const reefStaked = parseFloat(balance.reef_staked_molt) || 0;
            const hasBreakdown = staked > 0 || locked > 0 || reefStaked > 0;
            if (hasBreakdown) {
                const parts = [`<i class="fas fa-wallet" style="opacity:0.5;"></i> Spendable: <strong>${fmtToken(spendable)}</strong>`];
                if (staked > 0) parts.push(`<i class="fas fa-lock" style="opacity:0.5;"></i> Staked: <strong>${fmtToken(staked)}</strong>`);
                if (reefStaked > 0) parts.push(`<i class="fas fa-coins" style="opacity:0.5;"></i> ReefStake: <strong>${fmtToken(reefStaked)}</strong>`);
                if (locked > 0) parts.push(`<i class="fas fa-lock" style="opacity:0.5;"></i> Locked: <strong>${fmtToken(locked)}</strong>`);
                breakdownEl.innerHTML = parts.join(' &nbsp;·&nbsp; ');
                breakdownEl.style.display = 'block';
            } else {
                breakdownEl.style.display = 'none';
            }
        }
    } catch (error) {
        // Silently handle - new wallet with no on-chain account is expected
        const settings = walletState.settings || {};
        const currency = settings.currency || 'USD';
        const currencySymbols = { USD: '$', EUR: '€', GBP: '£', JPY: '¥' };
        const sym = currencySymbols[currency] || '$';
        document.getElementById('totalBalance').textContent = `${sym}0.00 ${currency}`;
        document.getElementById('balanceUsd').textContent = '0.00 MOLT';
    }
}

async function loadAssets() {
    const assetsList = document.getElementById('assetsList');
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    const balance = await rpc.getBalance(wallet.address).catch(() => ({ molt: 0 }));
    const molt = parseFloat(balance.molt) || 0;
    
    // Fetch all token balances in parallel
    const tokenBalances = await getAllTokenBalances(wallet.address);
    
    // Mock prices for display
    const MOCK_PRICES = { MOLT: 0.10, mUSD: 1.0, wSOL: 150.0, wETH: 3000.0, REEF: 0.05 };
    const settings = walletState.settings || {};
    const decimals = settings.decimals || 6;
    const currency = settings.currency || 'USD';
    const currencySymbols = { USD: '$', EUR: '€', GBP: '£', JPY: '¥' };
    const sym = currencySymbols[currency] || '$';
    
    // Build asset list HTML
    let html = '';
    
    // MOLT (always first, always shown)
    const moltUsd = molt * MOCK_PRICES.MOLT;
    html += `
        <div class="asset-item" style="cursor: default;">
            <div class="asset-icon">🦞</div>
            <div class="asset-info">
                <div class="asset-name">MoltChain</div>
                <div class="asset-symbol">MOLT</div>
            </div>
            <div class="asset-balance">
                <div class="asset-amount">${fmtToken(molt)}</div>
                <div class="asset-value">${fmtUsd(moltUsd, sym)}</div>
            </div>
        </div>
    `;
    
    // Wrapped tokens
    for (const [symbol, token] of Object.entries(TOKEN_REGISTRY)) {
        const bal = tokenBalances[symbol] || 0;
        const usdVal = bal * (MOCK_PRICES[symbol] || 0);
        
        // Show token if it has a balance or a known contract address
        if (bal > 0 || token.address) {
            html += `
                <div class="asset-item" style="cursor: default; ${bal === 0 ? 'opacity: 0.5;' : ''}">
                    <div class="asset-icon" style="color: ${token.color};">${token.icon}</div>
                    <div class="asset-info">
                        <div class="asset-name">${token.name}</div>
                        <div class="asset-symbol">${token.symbol}</div>
                    </div>
                    <div class="asset-balance">
                        <div class="asset-amount">${fmtToken(bal)}</div>
                        <div class="asset-value">${fmtUsd(usdVal, sym)}</div>
                    </div>
                </div>
            `;
        }
    }
    
    // Token contracts are loaded dynamically from deploy-manifest; nothing to show if absent
    
    assetsList.innerHTML = html;
}

let _activityBeforeSlot = null; // Pagination cursor for activity
let _activityItems = [];        // Accumulated activity items
let _activityHasMore = true;    // Whether more items may exist
const ACTIVITY_PAGE_SIZE = 20;  // Items per page

async function loadActivity(reset = true) {
    const activityList = document.getElementById('activityList');
    const wallet = getActiveWallet();
    if (!wallet) return;

    if (reset) {
        _activityBeforeSlot = null;
        _activityItems = [];
        _activityHasMore = true;
    }
    
    const emptyHtml = `
        <div style="text-align: center; padding: 3rem; color: var(--text-muted);">
            <i class="fas fa-history" style="font-size: 3rem; margin-bottom: 1rem; opacity: 0.3;"></i>
            <p>No activity yet</p>
        </div>
    `;
    
    try {
        // Fetch on-chain transactions via proper RPC (paginated)
        let transactions = [];
        try {
            const opts = { limit: ACTIVITY_PAGE_SIZE };
            if (_activityBeforeSlot) opts.before_slot = _activityBeforeSlot;
            const result = await rpc.call('getTransactionsByAddress', [wallet.address, opts]);
            transactions = result?.transactions || (Array.isArray(result) ? result : []);
        } catch (e) {
            // Account may not exist on-chain yet
        }

        // Fetch airdrops from faucet (only on first page, only if faucet is configured)
        let airdrops = [];
        if (!_activityBeforeSlot) {
            try {
                const faucetUrl = (typeof MOLT_CONFIG !== 'undefined' && MOLT_CONFIG.faucet) ? MOLT_CONFIG.faucet : null;
                if (faucetUrl) {
                    const abortCtl = new AbortController();
                    const timer = setTimeout(() => abortCtl.abort(), 2000);
                    const resp = await fetch(`${faucetUrl}/faucet/airdrops?address=${encodeURIComponent(wallet.address)}&limit=50`, { signal: abortCtl.signal });
                    clearTimeout(timer);
                    if (resp.ok) {
                        const records = await resp.json();
                        airdrops = records.map(a => ({
                            type: 'Airdrop',
                            from: 'Treasury',
                            to: a.recipient,
                            amount: a.amount_molt * 1_000_000_000,
                            timestamp: a.timestamp_ms,
                            signature: a.signature,
                            isAirdrop: true
                        }));
                    }
                }
            } catch (e) { /* faucet API unavailable — skip silently */ }
        }

        // Track pagination cursor from last TX slot
        if (transactions.length > 0) {
            const lastTx = transactions[transactions.length - 1];
            const lastSlot = lastTx.slot || lastTx.block_slot;
            if (lastSlot) _activityBeforeSlot = lastSlot;
        }
        _activityHasMore = transactions.length >= ACTIVITY_PAGE_SIZE;

        // Merge new page into accumulated items
        // RPC returns timestamp as unix seconds — convert to ms for Date()
        const newItems = [...transactions.map(tx => ({
            ...tx,
            timestamp: (tx.block_time || tx.timestamp || 0) * 1000,
            isAirdrop: false
        })), ...airdrops];
        _activityItems = [..._activityItems, ...newItems]
            .sort((a, b) => (b.timestamp || 0) - (a.timestamp || 0));
        
        if (_activityItems.length === 0) {
            activityList.innerHTML = emptyHtml;
            return;
        }
        
        // Render activity
        activityList.innerHTML = _activityItems.map(tx => {
            let icon, color, type, address, amount, sign;

            if (tx.isAirdrop) {
                icon = 'fa-parachute-box';
                color = '#60a5fa';
                type = 'Airdrop';
                address = 'Faucet (Treasury)';
                amount = fmtToken(tx.amount / 1_000_000_000);
                sign = '+';
            } else {
                const isSent = tx.from === wallet.address;
                // Map tx.type to user-friendly labels
                const typeMap = {
                    'Transfer': isSent ? 'Sent' : 'Received',
                    'Airdrop': 'Airdrop',
                    'Stake': 'Staked',
                    'Unstake': 'Unstaked',
                    'ClaimUnstake': 'Claimed Unstake',
                    'ReefStakeDeposit': 'Staked (ReefStake)',
                    'ReefStakeUnstake': 'Unstaked (ReefStake)',
                    'ReefStakeClaim': 'Claimed (ReefStake)',
                    'ReefStakeTransfer': 'Transfer (stMOLT)',
                    'RegisterEvmAddress': 'EVM Registration',
                    'Contract': 'Contract Call',
                    'DeployContract': 'Deploy Contract',
                    'SetContractABI': 'Set Contract ABI',
                    'FaucetAirdrop': 'Faucet Airdrop',
                    'RegisterSymbol': 'Register Symbol',
                    'CreateAccount': 'Create Account',
                    'CreateCollection': 'Created Collection',
                    'MintNFT': 'Minted NFT',
                    'TransferNFT': isSent ? 'Sent NFT' : 'Received NFT',
                    'Reward': 'Reward',
                    'GrantRepay': 'Grant Repay',
                    'GenesisTransfer': 'Genesis Transfer',
                    'GenesisMint': 'Genesis Mint',
                };
                type = typeMap[tx.type] || (isSent ? 'Sent' : 'Received');
                icon = isSent ? 'fa-arrow-up' : 'fa-arrow-down';
                color = isSent ? '#ff6b35' : '#4ade80';
                // Special icons/colors for non-transfer types
                if (tx.type === 'Stake' || tx.type === 'Unstake' || tx.type === 'ClaimUnstake'
                    || tx.type === 'ReefStakeDeposit' || tx.type === 'ReefStakeUnstake'
                    || tx.type === 'ReefStakeClaim' || tx.type === 'ReefStakeTransfer') {
                    icon = 'fa-coins'; color = '#a78bfa';
                    // For staking deposits, show the staked amount as negative (outflow)
                    if (tx.type === 'ReefStakeDeposit' || tx.type === 'Stake') {
                        sign = '-';
                    }
                } else if (tx.type === 'RegisterEvmAddress') {
                    icon = 'fa-link'; color = '#94a3b8';
                } else if (tx.type === 'Contract') {
                    icon = 'fa-file-code'; color = '#f59e0b';
                } else if (tx.type === 'Reward' || tx.type === 'GenesisTransfer' || tx.type === 'GenesisMint') {
                    icon = 'fa-gift'; color = '#4ade80'; sign = '+';
                } else if (tx.type === 'Airdrop' || tx.type === 'FaucetAirdrop') {
                    icon = 'fa-parachute-box'; color = '#60a5fa';
                } else if (tx.type === 'GrantRepay') {
                    icon = 'fa-hand-holding-usd'; color = '#94a3b8'; sign = isSent ? '-' : '+';
                } else if (tx.type === 'CreateAccount') {
                    icon = 'fa-user-plus'; color = '#94a3b8';
                }
                address = isSent ? (tx.to || '') : (tx.from || '');
                const amountVal = tx.amount_shells ? tx.amount_shells : (tx.amount || 0);
                amount = fmtToken(amountVal / 1_000_000_000);
                sign = sign || (isSent ? '-' : '+');
            }

            const displayAddr = address && address.length > 20 ? address.slice(0, 8) + '...' + address.slice(-4) : (address || '');
            const date = tx.timestamp ? new Date(tx.timestamp).toLocaleString() : '';
            const sig = tx.signature || tx.hash || '';
            const shortSig = sig ? sig.slice(0, 8) + '...' + sig.slice(-4) : '';
            const explorerLink = sig ? `../explorer/transaction.html?sig=${sig}` : '#';
            const isFeeOnly = amount === '0' && (tx.type === 'RegisterEvmAddress' || tx.type === 'Contract'
                || tx.type === 'DeployContract' || tx.type === 'SetContractABI' || tx.type === 'RegisterSymbol'
                || tx.type === 'CreateAccount');
            const feeShells = tx.fee_shells || tx.fee || 0;
            const feeAmt = fmtToken(feeShells / 1_000_000_000);
            const amountStr = isFeeOnly ? `${feeAmt} MOLT` : `${sign}${amount} MOLT`;
            const feeTag = isFeeOnly ? '<span style="display:inline-block;margin-left:0.35rem;padding:0.05rem 0.4rem;border-radius:4px;font-size:0.65rem;background:rgba(245,158,11,0.15);color:#f59e0b;font-weight:600;vertical-align:middle;">FEE</span>' : '';
            
            return `
                <a href="${explorerLink}" target="_blank" class="activity-item" style="text-decoration:none; color:inherit; display:flex;">
                    <div class="activity-icon" style="background: ${color}22; color: ${color};">
                        <i class="fas ${icon}"></i>
                    </div>
                    <div class="activity-details" style="flex: 1; min-width: 0;">
                        <div class="activity-type">${type}${displayAddr ? `<span class="activity-addr">${displayAddr}</span>` : ''}</div>
                        <div class="activity-date" style="font-size: 0.75rem; opacity: 0.6;">${shortSig}</div>
                    </div>
                    <div style="text-align: right; flex-shrink: 0;">
                        <div class="activity-amount" style="color: ${color};">
                            ${amountStr}${feeTag}
                        </div>
                        <div style="font-size: 0.7rem; opacity: 0.5;">${date}</div>
                    </div>
                </a>
            `;
        }).join('');

        // Add "Load More" button if there are more
        if (_activityHasMore) {
            activityList.innerHTML += `
                <div style="text-align: center; padding: 1rem;">
                    <button onclick="loadActivity(false)" class="btn btn-small btn-secondary" style="padding: 0.5rem 1.5rem; font-size: 0.85rem;">
                        <i class="fas fa-chevron-down"></i> Load More
                    </button>
                </div>
            `;
        }
        
    } catch (error) {
        console.error('Failed to load activity:', error);
        if (_activityItems.length === 0) activityList.innerHTML = emptyHtml;
    }
}

// Load staking info (validator status, bootstrap grant, vesting)
async function loadStaking() {
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    const validatorSection = document.getElementById('stakingValidatorInfo');
    const stakingTabBtn = document.querySelector('.dashboard-tab[data-tab="staking"]');
    
    try {
        // Check if this wallet is a validator
        const validatorsResponse = await rpc.call('getValidators');
        const validators = validatorsResponse?.validators || [];
        const myValidator = validators.find(v => v.pubkey === wallet.address);
        
        // Always show staking tab (for ReefStake or validator staking)
        if (stakingTabBtn) stakingTabBtn.style.display = 'flex';
        
        if (!myValidator) {
            // Not a validator - show ReefStake community staking UI
            if (validatorSection) {
                validatorSection.style.display = 'block';
                validatorSection.innerHTML = `
                    <div class="reefstake-header" style="background: linear-gradient(135deg, rgba(59, 130, 246, 0.1), rgba(37, 99, 235, 0.1)); padding: 1.5rem; border-radius: 12px; margin-bottom: 1.5rem;">
                        <h3 style="margin: 0 0 0.5rem 0; display: flex; align-items: center; gap: 0.5rem;">
                            <i class="fas fa-water" style="color: #3b82f6;"></i>
                            ReefStake - Liquid Staking
                        </h3>
                        <p style="margin: 0; font-size: 0.9rem; color: var(--text-muted);">Stake MOLT, receive stMOLT. Earn rewards while keeping liquidity. Choose a lock tier for boosted APY.</p>
                    </div>

                    <div class="reefstake-stats" style="display: grid; grid-template-columns: repeat(3, 1fr); gap: 1rem; margin-bottom: 1.5rem;">
                        <div class="stat-card" style="background: var(--card-bg); padding: 1.25rem; border-radius: 12px; border: 1px solid var(--border);">
                            <div style="color: var(--text-muted); font-size: 0.85rem; margin-bottom: 0.5rem;">Your stMOLT</div>
                            <div id="userStMolt" style="font-size: 1.5rem; font-weight: 600; color: var(--text);">0</div>
                        </div>
                        <div class="stat-card" style="background: var(--card-bg); padding: 1.25rem; border-radius: 12px; border: 1px solid var(--border);">
                            <div style="color: var(--text-muted); font-size: 0.85rem; margin-bottom: 0.5rem;">Current Value</div>
                            <div id="userStakeValue" style="font-size: 1.5rem; font-weight: 600; color: #10b981;">0 MOLT</div>
                        </div>
                        <div class="stat-card" style="background: var(--card-bg); padding: 1.25rem; border-radius: 12px; border: 1px solid var(--border);">
                            <div style="color: var(--text-muted); font-size: 0.85rem; margin-bottom: 0.5rem;">Rewards Earned</div>
                            <div id="userRewardsEarned" style="font-size: 1.5rem; font-weight: 600; color: #f59e0b;">0 MOLT</div>
                        </div>
                        <div class="stat-card" style="background: var(--card-bg); padding: 1.25rem; border-radius: 12px; border: 1px solid var(--border);">
                            <div style="color: var(--text-muted); font-size: 0.85rem; margin-bottom: 0.5rem;">Your Tier</div>
                            <div id="userLockTier" style="font-size: 1.2rem; font-weight: 600; color: #a78bfa;">—</div>
                        </div>
                        <div class="stat-card" style="background: var(--card-bg); padding: 1.25rem; border-radius: 12px; border: 1px solid var(--border);">
                            <div style="color: var(--text-muted); font-size: 0.85rem; margin-bottom: 0.5rem;">Reward Multiplier</div>
                            <div id="userMultiplier" style="font-size: 1.5rem; font-weight: 600; color: var(--text);">1.0x</div>
                        </div>
                        <div class="stat-card" style="background: var(--card-bg); padding: 1.25rem; border-radius: 12px; border: 1px solid var(--border);">
                            <div style="color: var(--text-muted); font-size: 0.85rem; margin-bottom: 0.5rem;">Total Staked (Pool)</div>
                            <div id="totalPoolStaked" style="font-size: 1.5rem; font-weight: 600; color: var(--text);">0 MOLT</div>
                        </div>
                    </div>

                    <div id="reefstakeTiers" style="margin-bottom: 1.5rem;">
                        <h4 style="margin-bottom: 0.75rem; color: var(--text);">
                            <i class="fas fa-layer-group" style="color: #a78bfa;"></i> Staking Tiers & APY
                        </h4>
                        <div id="tiersGrid" style="display: grid; grid-template-columns: repeat(2, 1fr); gap: 0.75rem;"></div>
                    </div>

                    <div style="background: var(--card-bg); padding: 1rem; border-radius: 10px; border: 1px solid var(--border); margin-bottom: 1.5rem; font-size: 0.85rem; color: var(--text-muted);">
                        <i class="fas fa-info-circle" style="color: #3b82f6;"></i>
                        <strong>How it works:</strong> Stake MOLT to receive stMOLT (liquid receipt). Rewards auto-compound — your stMOLT value grows over time.
                        <strong>Flexible tier</strong> has a 7-day cooldown to unstake. <strong>Locked tiers</strong> earn boosted rewards but funds are locked for the chosen duration.
                        After a lock expires, you can unstake with the standard 7-day cooldown.
                    </div>

                    <div class="reefstake-actions" style="display: grid; grid-template-columns: 1fr 1fr; gap: 1rem;">
                        <button onclick="showReefStakeModal()" class="btn btn-primary" style="width: 100%; padding: 1rem;">
                            <i class="fas fa-arrow-down"></i> Stake MOLT
                        </button>
                        <button onclick="showReefUnstakeModal()" class="btn btn-secondary" style="width: 100%; padding: 1rem;">
                            <i class="fas fa-arrow-up"></i> Unstake stMOLT
                        </button>
                    </div>

                    <div id="lockStatus" style="margin-top: 1rem; display: none; padding: 0.75rem 1rem; background: rgba(249,115,22,0.1); border: 1px solid rgba(249,115,22,0.3); border-radius: 8px; font-size: 0.85rem; color: #f97316;">
                        <i class="fas fa-lock"></i> <span id="lockStatusText"></span>
                    </div>

                    <div id="pendingUnstakes" style="margin-top: 1.5rem; display: none;">
                        <h4 style="margin-bottom: 1rem;">Pending Unstakes (7-day cooldown)</h4>
                        <div id="unstakesList"></div>
                    </div>
                `;
                
                // Load ReefStake position
                loadReefStakePosition(wallet.address);
            }
            return;
        }
        
        // Is a validator - show tab and generate validator content
        if (stakingTabBtn) stakingTabBtn.style.display = 'flex';
        if (validatorSection) {
            validatorSection.style.display = 'block';
            
            // Generate staking UI dynamically
            validatorSection.innerHTML = `
                <div class="staking-header" style="background: linear-gradient(135deg, rgba(139, 92, 246, 0.1), rgba(79, 70, 229, 0.1)); padding: 1.5rem; border-radius: 12px; margin-bottom: 1.5rem;">
                    <h3 style="margin: 0 0 0.5rem 0; display: flex; align-items: center; gap: 0.5rem;">
                        <i class="fas fa-award" style="color: var(--accent);"></i>
                        Validator Status
                    </h3>
                    <div id="validatorStatus" style="font-size: 0.95rem; color: var(--text-muted);"></div>
                </div>

                <div class="staking-stats" style="display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1rem; margin-bottom: 1.5rem;">
                    <div class="stat-card" style="background: var(--card-bg); padding: 1.25rem; border-radius: 12px; border: 1px solid var(--border);">
                        <div style="color: var(--text-muted); font-size: 0.85rem; margin-bottom: 0.5rem;">Total Stake</div>
                        <div id="totalStake" style="font-size: 1.5rem; font-weight: 600; color: var(--text);">Loading...</div>
                    </div>

                    <div class="stat-card" style="background: var(--card-bg); padding: 1.25rem; border-radius: 12px; border: 1px solid var(--border);">
                        <div style="color: var(--text-muted); font-size: 0.85rem; margin-bottom: 0.5rem;">Bootstrap Grant</div>
                        <div style="font-size: 1.5rem; font-weight: 600; color: var(--text);">100,000 MOLT</div>
                    </div>

                    <div class="stat-card" style="background: var(--card-bg); padding: 1.25rem; border-radius: 12px; border: 1px solid var(--border);">
                        <div style="color: var(--text-muted); font-size: 0.85rem; margin-bottom: 0.5rem;">Debt Remaining</div>
                        <div id="debtRemaining" style="font-size: 1.5rem; font-weight: 600; color: #f59e0b;">Loading...</div>
                    </div>

                    <div class="stat-card" style="background: var(--card-bg); padding: 1.25rem; border-radius: 12px; border: 1px solid var(--border);">
                        <div style="color: var(--text-muted); font-size: 0.85rem; margin-bottom: 0.5rem;">Earned / Vested</div>
                        <div id="earnedAmount" style="font-size: 1.5rem; font-weight: 600; color: #10b981;">Loading...</div>
                    </div>
                </div>

                <div class="vesting-progress" style="background: var(--card-bg); padding: 1.5rem; border-radius: 12px; border: 1px solid var(--border);">
                    <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 1rem;">
                        <div style="color: var(--text-muted); font-size: 0.9rem;">Vesting Progress</div>
                        <div id="vestingPercent" style="font-weight: 600; color: var(--text);">0%</div>
                    </div>
                    <div style="height: 8px; background: var(--bg); border-radius: 4px; overflow: hidden;">
                        <div id="vestingProgressBar" style="height: 100%; background: linear-gradient(90deg, var(--accent), #10b981); width: 0%; transition: width 0.3s ease;"></div>
                    </div>
                    <div id="vestingInfo" style="margin-top: 1rem; font-size: 0.85rem; color: var(--text-muted);"></div>
                </div>

                <div id="graduationInfo" style="margin-top: 1.5rem; padding: 1.25rem; background: linear-gradient(135deg, rgba(16, 185, 129, 0.1), rgba(5, 150, 105, 0.1)); border-radius: 12px; border: 1px solid rgba(16, 185, 129, 0.3); display: none;">
                    <div style="display: flex; align-items: center; gap: 0.75rem;">
                        <i class="fas fa-graduation-cap" style="font-size: 1.5rem; color: #10b981;"></i>
                        <div>
                            <div style="font-weight: 600; margin-bottom: 0.25rem; color: var(--text);">Graduated! 🎉</div>
                            <div id="graduationSlot" style="font-size: 0.9rem; color: var(--text-muted);"></div>
                        </div>
                    </div>
                </div>
            `;
        }
        
        // Get validator account to check actual stake
        const account = await rpc.getAccount(wallet.address);
        const totalStake = account?.shells || 0;
        const totalStakeMOLT = totalStake / 1_000_000_000;
        
        // Bootstrap grant info
        const BOOTSTRAP_GRANT = 100000; // 100K MOLT
        const bootstrapDebt = myValidator.bootstrap_debt || 0;
        const debtMOLT = bootstrapDebt / 1_000_000_000;
        
        // Calculate earned/vested amount
        const earnedAmount = BOOTSTRAP_GRANT - debtMOLT;
        const vestingPercent = (earnedAmount / BOOTSTRAP_GRANT * 100).toFixed(2);
        
        // Check if graduated
        const isGraduated = myValidator.status === 'Active' && bootstrapDebt === 0;
        const graduationSlot = myValidator.graduation_slot;
        
        // Update UI
        document.getElementById('totalStake').textContent = `${fmtToken(totalStakeMOLT)} MOLT`;
        document.getElementById('debtRemaining').textContent = `${fmtToken(debtMOLT)} MOLT`;
        document.getElementById('earnedAmount').textContent = `${fmtToken(earnedAmount)} MOLT`;
        document.getElementById('vestingPercent').textContent = `${vestingPercent}%`;
        document.getElementById('vestingProgressBar').style.width = `${vestingPercent}%`;
        
        // Status message
        let statusHTML = '';
        if (isGraduated) {
            statusHTML = '<span style="color: #10b981;">✓ Active & Graduated</span>';
        } else if (myValidator.status === 'Active') {
            statusHTML = `<span style="color: #f59e0b;">⚡ Active (Bootstrap phase - ${fmtToken(debtMOLT)} MOLT remaining)</span>`;
        } else if (myValidator.status === 'Jailed') {
            statusHTML = '<span style="color: #ef4444;">⚠️ Jailed (Offline or misbehaving)</span>';
        } else {
            statusHTML = `<span style="color: var(--text-muted);">${myValidator.status}</span>`;
        }
        document.getElementById('validatorStatus').innerHTML = statusHTML;
        
        // Vesting info
        let vestingInfoHTML = '';
        if (isGraduated) {
            vestingInfoHTML = '<span style="color: #10b981;">✓ Fully vested - you own 100% of your stake!</span>';
        } else {
            const blocksUntilVested = Math.ceil(bootstrapDebt / 1000); // Rough estimate (depends on rewards)
            const minutesEstimate = Math.ceil(blocksUntilVested * 0.4 / 60); // ~400ms per block
            vestingInfoHTML = `Estimated time to full vesting: ~${minutesEstimate} minutes (${blocksUntilVested} blocks)`;
        }
        document.getElementById('vestingInfo').innerHTML = vestingInfoHTML;
        
        // Graduation info
        const graduationInfoDiv = document.getElementById('graduationInfo');
        if (isGraduated && graduationSlot) {
            graduationInfoDiv.style.display = 'block';
            document.getElementById('graduationSlot').textContent = `Graduated at slot ${graduationSlot}`;
        } else {
            graduationInfoDiv.style.display = 'none';
        }
        
    } catch (error) {
        console.error('Failed to load staking info:', error);
        // Show ReefStake UI even on error
        if (stakingTabBtn) stakingTabBtn.style.display = 'flex';
        if (validatorSection) {
            validatorSection.innerHTML = '<p style="text-align: center; padding: 2rem; color: var(--text-muted);">Failed to load staking info</p>';
        }
    }
}

// Refresh staking UI if the staking tab is currently visible
function refreshStakingIfVisible() {
    const wallet = getActiveWallet();
    if (!wallet) return;
    const stakingTab = document.querySelector('.dashboard-tab[data-tab="staking"]');
    const stakingSection = document.getElementById('stakingValidatorInfo');
    if (stakingTab && stakingTab.classList.contains('active') && stakingSection && stakingSection.style.display !== 'none') {
        loadReefStakePosition(wallet.address);
    }
}

// Load ReefStake position for community  stakers
async function loadReefStakePosition(address) {
    try {
        const poolInfo = await rpc.call('getReefStakePoolInfo');
        const position = await rpc.call('getStakingPosition', [address]);
        const queue = await rpc.call('getUnstakingQueue', [address]);
        
        // Update basic stats
        document.getElementById('userStMolt').textContent = fmtToken(position.st_molt_amount / 1_000_000_000);
        document.getElementById('userStakeValue').textContent = `${fmtToken(position.current_value_molt / 1_000_000_000)} MOLT`;
        document.getElementById('totalPoolStaked').textContent = `${fmtToken(poolInfo.total_molt_staked / 1_000_000_000)} MOLT`;

        // Rewards
        const rewardsEl = document.getElementById('userRewardsEarned');
        if (rewardsEl) rewardsEl.textContent = `${fmtToken(position.rewards_earned / 1_000_000_000)} MOLT`;

        // Tier info
        const tierEl = document.getElementById('userLockTier');
        if (tierEl) tierEl.textContent = position.lock_tier_name || 'Flexible';
        const multEl = document.getElementById('userMultiplier');
        if (multEl) multEl.textContent = `${position.reward_multiplier || 1.0}x`;

        // Lock status
        const lockStatus = document.getElementById('lockStatus');
        const lockText = document.getElementById('lockStatusText');
        if (lockStatus && lockText && position.lock_until > 0) {
            // Estimate time remaining
            const currentSlotEstimate = Math.floor(Date.now() / 400);
            if (position.lock_until > currentSlotEstimate) {
                const remainingSlots = position.lock_until - currentSlotEstimate;
                const remainingDays = Math.ceil(remainingSlots / 216000);
                lockStatus.style.display = 'block';
                lockText.textContent = `Position locked (${position.lock_tier_name}). ~${remainingDays} days remaining until unlock at slot ${position.lock_until.toLocaleString()}.`;
            } else {
                lockStatus.style.display = 'none';
            }
        } else if (lockStatus) {
            lockStatus.style.display = 'none';
        }

        // Render tier cards
        const tiersGrid = document.getElementById('tiersGrid');
        if (tiersGrid && poolInfo.tiers) {
            const tierColors = ['#94a3b8', '#60a5fa', '#a78bfa', '#f59e0b'];
            tiersGrid.innerHTML = poolInfo.tiers.map((t, i) => {
                const isActive = position.lock_tier === t.id && position.st_molt_amount > 0;
                const apyStr = (t.apy_percent || 0).toFixed(1);
                return `
                    <div style="background: var(--card-bg); padding: 1rem; border-radius: 10px; border: 2px solid ${isActive ? tierColors[i] : 'var(--border)'}; ${isActive ? 'box-shadow: 0 0 12px ' + tierColors[i] + '33;' : ''}">
                        <div style="font-weight: 600; font-size: 0.95rem; color: ${tierColors[i]}; margin-bottom: 0.35rem;">${t.name}</div>
                        <div style="font-size: 1.4rem; font-weight: 700; color: var(--text);">${apyStr}% <span style="font-size:0.7rem;color:var(--text-muted);">APY</span></div>
                        <div style="font-size: 0.8rem; color: var(--text-muted); margin-top: 0.25rem;">
                            ${t.lock_days > 0 ? t.lock_days + '-day lock' : '7-day cooldown'}
                            &middot; ${t.multiplier}x rewards
                        </div>
                        ${isActive ? '<div style="font-size:0.7rem;color:' + tierColors[i] + ';margin-top:0.4rem;font-weight:600;"><i class="fas fa-check-circle"></i> Active</div>' : ''}
                    </div>
                `;
            }).join('');
        }

        // Show pending unstakes if any
        if (queue.pending_requests && queue.pending_requests.length > 0) {
            document.getElementById('pendingUnstakes').style.display = 'block';
            const unstakesList = document.getElementById('unstakesList');
            unstakesList.innerHTML = queue.pending_requests.map(req => {
                const currentSlot = Math.floor(Date.now() / 400);
                const isClaimable = req.claimable_at <= currentSlot;
                const remainSlots = Math.max(0, req.claimable_at - currentSlot);
                const remainDays = (remainSlots / 216000).toFixed(1);
                return `
                    <div style="padding: 1rem; background: var(--card-bg); border-radius: 8px; border: 1px solid var(--border); margin-bottom: 0.5rem;">
                        <div style="display: flex; justify-content: space-between; align-items: center;">
                            <span style="font-weight: 600;">${fmtToken(req.molt_to_receive / 1_000_000_000)} MOLT</span>
                            <span style="display: flex; align-items: center; gap: 0.5rem;">
                                ${isClaimable
                                    ? `<button onclick="claimReefStake()" class="btn btn-small" style="padding:0.3rem 0.8rem;font-size:0.8rem;background:#10b981;border:none;border-radius:6px;color:#fff;cursor:pointer;font-weight:600;">
                                        <i class="fas fa-check-circle"></i> Claim
                                       </button>`
                                    : `<span style="color:var(--text-muted);font-size:0.85rem;"><i class="fas fa-clock"></i> ~${remainDays} days</span>`
                                }
                            </span>
                        </div>
                    </div>
                `;
            }).join('');
        } else {
            document.getElementById('pendingUnstakes').style.display = 'none';
        }
    } catch (error) {
        console.error('Failed to load ReefStake position:', error);
    }
}

// Show ReefStake modal
async function showReefStakeModal() {
    const wallet = getActiveWallet();
    if (!wallet) { showToast('No active wallet'); return; }
    
    const values = await showPasswordModal({
        title: 'Stake to ReefStake',
        message: `Enter the amount of MOLT to stake, choose a lock tier, and sign with your password.
            <div style="margin-top:0.75rem;font-size:0.8rem;color:var(--text-muted);">
                <strong>Flexible:</strong> 7-day cooldown, 1x rewards<br>
                <strong>30-Day Lock:</strong> 1.5x rewards<br>
                <strong>90-Day Lock:</strong> 2x rewards<br>
                <strong>365-Day Lock:</strong> 3x rewards
            </div>`,
        icon: 'fas fa-layer-group',
        confirmText: 'Stake MOLT',
        fields: [
            { id: 'stakeAmount', label: 'Amount (MOLT)', type: 'number', placeholder: '0.00' },
            { id: 'lockTier', label: 'Lock Tier', type: 'select',
              options: [
                  { value: '0', label: 'Flexible — 7-day cooldown, 1x rewards' },
                  { value: '1', label: '30-Day Lock — 1.5x rewards' },
                  { value: '2', label: '90-Day Lock — 2x rewards' },
                  { value: '3', label: '365-Day Lock — 3x rewards' },
              ]},
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Enter password to sign' }
        ]
    });
    
    if (!values) return;
    const amount = parseFloat(values.stakeAmount);
    if (!amount || amount <= 0) { showToast('Invalid amount'); return; }
    if (!values.password) { showToast('Password required'); return; }
    
    try {
        const shells = Math.floor(amount * 1_000_000_000);
        const tierByte = parseInt(values.lockTier || '0');
        const latestBlock = await rpc.getLatestBlock();
        const fromPubkey = MoltCrypto.hexToBytes(wallet.publicKey);
        
        // Instruction type 13 = ReefStake deposit, then amount(8), then tier(1)
        const instructionData = new Uint8Array(10);
        instructionData[0] = 13;
        const view = new DataView(instructionData.buffer);
        view.setBigUint64(1, BigInt(shells), true);
        instructionData[9] = tierByte;
        
        const message = {
            instructions: [{
                program_id: Array.from(new Uint8Array(32)), // SYSTEM_PROGRAM_ID = [0; 32]
                accounts: [Array.from(fromPubkey)],
                data: Array.from(instructionData)
            }],
            blockhash: latestBlock.hash
        };
        
        const privateKey = await MoltCrypto.decryptPrivateKey(wallet.encryptedKey, values.password);
        const messageBytes = serializeMessageBincode(message);
        const signature = await MoltCrypto.signTransaction(privateKey, messageBytes);
        
        const transaction = { signatures: [Array.from(signature)], message };
        const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
        const txBase64 = btoa(String.fromCharCode(...txBytes));
        
        showToast('Staking to ReefStake...');
        const txSig = await rpc.sendTransaction(txBase64);
        showToast(`Staked ${amount} MOLT! Sig: ${String(txSig).slice(0, 16)}...`);
        await refreshBalance();
        // Refresh staking position after a brief delay for block inclusion
        setTimeout(() => loadReefStakePosition(wallet.address), 1500);
        setTimeout(() => loadReefStakePosition(wallet.address), 4000);
    } catch (error) {
        showToast('Stake failed: ' + error.message);
    }
}

// Show ReefUnstake modal
async function showReefUnstakeModal() {
    const wallet = getActiveWallet();
    if (!wallet) { showToast('No active wallet'); return; }
    
    const values = await showPasswordModal({
        title: 'Unstake from ReefStake',
        message: `Enter the amount of stMOLT to unstake. After requesting, there is a <strong>7-day cooldown</strong> before you can claim your MOLT.
            <div style="margin-top:0.5rem;font-size:0.8rem;color:var(--text-muted);">
                <i class="fas fa-exclamation-triangle" style="color:#f59e0b;"></i>
                If your position has a lock tier, you must wait for the lock to expire before unstaking.
            </div>`,
        icon: 'fas fa-unlock-alt',
        confirmText: 'Unstake',
        fields: [
            { id: 'unstakeAmount', label: 'Amount (stMOLT)', type: 'number', placeholder: '0.00' },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Enter password to sign' }
        ]
    });
    
    if (!values) return;
    const amount = parseFloat(values.unstakeAmount);
    if (!amount || amount <= 0) { showToast('Invalid amount'); return; }
    if (!values.password) { showToast('Password required'); return; }
    
    try {
        const shells = Math.floor(amount * 1_000_000_000);
        const latestBlock = await rpc.getLatestBlock();
        const fromPubkey = MoltCrypto.hexToBytes(wallet.publicKey);
        
        // Instruction type 14 = ReefStake unstake
        const instructionData = new Uint8Array(9);
        instructionData[0] = 14;
        const view = new DataView(instructionData.buffer);
        view.setBigUint64(1, BigInt(shells), true);
        
        const message = {
            instructions: [{
                program_id: Array.from(new Uint8Array(32)), // SYSTEM_PROGRAM_ID = [0; 32]
                accounts: [Array.from(fromPubkey)],
                data: Array.from(instructionData)
            }],
            blockhash: latestBlock.hash
        };
        
        const privateKey = await MoltCrypto.decryptPrivateKey(wallet.encryptedKey, values.password);
        const messageBytes = serializeMessageBincode(message);
        const signature = await MoltCrypto.signTransaction(privateKey, messageBytes);
        
        const transaction = { signatures: [Array.from(signature)], message };
        const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
        const txBase64 = btoa(String.fromCharCode(...txBytes));
        
        showToast('Unstaking from ReefStake...');
        const txSig = await rpc.sendTransaction(txBase64);
        showToast(`Unstake initiated! 7-day cooldown. Sig: ${String(txSig).slice(0, 16)}...`);
        await refreshBalance();
        // Refresh staking position after a brief delay for block inclusion
        setTimeout(() => loadReefStakePosition(wallet.address), 1500);
        setTimeout(() => loadReefStakePosition(wallet.address), 4000);
    } catch (error) {
        showToast('Unstake failed: ' + error.message);
    }
}

// Claim matured ReefStake unstake (instruction type 15)
async function claimReefStake() {
    const wallet = getActiveWallet();
    if (!wallet) { showToast('No active wallet'); return; }

    const values = await showPasswordModal({
        title: 'Claim Unstaked MOLT',
        message: 'Enter your password to sign the claim transaction. Your matured MOLT will be returned to your spendable balance.',
        icon: 'fas fa-check-circle',
        confirmText: 'Claim',
        fields: [
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Enter password to sign' }
        ]
    });

    if (!values || !values.password) return;

    try {
        const latestBlock = await rpc.getLatestBlock();
        const fromPubkey = MoltCrypto.hexToBytes(wallet.publicKey);

        // Instruction type 15 = ReefStake claim (data: [15], accounts: [user])
        const instructionData = new Uint8Array([15]);

        const message = {
            instructions: [{
                program_id: Array.from(new Uint8Array(32)),
                accounts: [Array.from(fromPubkey)],
                data: Array.from(instructionData)
            }],
            blockhash: latestBlock.hash
        };

        const privateKey = await MoltCrypto.decryptPrivateKey(wallet.encryptedKey, values.password);
        const messageBytes = serializeMessageBincode(message);
        const signature = await MoltCrypto.signTransaction(privateKey, messageBytes);

        const transaction = { signatures: [Array.from(signature)], message };
        const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
        const txBase64 = btoa(String.fromCharCode(...txBytes));

        showToast('Claiming unstaked MOLT...');
        const txSig = await rpc.sendTransaction(txBase64);
        showToast(`Claimed! Sig: ${String(txSig).slice(0, 16)}...`);
        await refreshBalance();
        setTimeout(() => loadReefStakePosition(wallet.address), 1500);
        setTimeout(() => loadReefStakePosition(wallet.address), 4000);
    } catch (error) {
        showToast('Claim failed: ' + error.message);
    }
}

// ===== MODALS =====
async function showSend() {
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    // Dynamically populate token dropdown - always show MOLT, only tokens with balance or address
    const select = document.getElementById('sendToken');
    if (select) {
        select.innerHTML = '<option value="MOLT">MOLT</option>';
        
        try {
            const tokenBalances = await getAllTokenBalances(wallet.address);
            for (const [symbol, token] of Object.entries(TOKEN_REGISTRY)) {
                const bal = tokenBalances[symbol] || 0;
                if (bal > 0 || token.address) {
                    select.innerHTML += `<option value="${symbol}">${token.icon} ${symbol}</option>`;
                }
            }
        } catch (e) {
            // If token balance fetch fails, still show MOLT
        }

        // Add stMOLT if user has a staking position
        try {
            const position = await rpc.call('getStakingPosition', [wallet.address]);
            if (position && position.st_molt_amount > 0) {
                select.innerHTML += `<option value="stMOLT">&#x1f30a; stMOLT</option>`;
            }
        } catch (e) {
            // No staking position
        }
    }
    
    // Update balance hint
    updateSendTokenUI();
    
    document.getElementById('sendModal').classList.add('show');
}

function showReceive(tab = 'receive') {
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    // Set native Base58 address
    document.getElementById('walletAddress').value = wallet.address;
    
    // Generate EVM address (0x format)
    const evmAddress = generateEVMAddress(wallet.address);
    document.getElementById('walletAddressEVM').value = evmAddress;
    
    // Generate QR code for native address
    const qrCodeDiv = document.getElementById('qrCode');
    qrCodeDiv.innerHTML = ''; // Clear previous QR code
    
    try {
        new QRCode(qrCodeDiv, {
            text: wallet.address,
            width: 200,
            height: 200,
            colorDark: "#1a1a2e",
            colorLight: "#ffffff",
            correctLevel: QRCode.CorrectLevel.H
        });
    } catch (e) {
        // Fallback if library not loaded
        qrCodeDiv.innerHTML = `<div style="width: 200px; height: 200px; background: white; border-radius: 8px; display: flex; align-items: center; justify-content: center; color: #1a1a2e; padding: 1rem; text-align: center;"><i class="fas fa-qrcode" style="font-size: 4rem;"></i></div>`;
    }
    
    // Switch to requested tab
    switchReceiveTab(tab);
    
    document.getElementById('receiveModal').classList.add('show');
}

function switchReceiveTab(tabName) {
    // Update tab buttons
    document.querySelectorAll('.receive-tab').forEach(t => t.classList.remove('active'));
    const activeTab = document.querySelector(`.receive-tab[data-tab="${tabName}"]`);
    if (activeTab) activeTab.classList.add('active');
    
    // Update tab content
    document.querySelectorAll('.receive-tab-content').forEach(c => {
        c.style.display = 'none';
        c.classList.remove('active');
    });
    const activeContent = document.querySelector(`.receive-tab-content[data-tab="${tabName}"]`);
    if (activeContent) {
        activeContent.style.display = 'block';
        activeContent.classList.add('active');
    }
}

// ===== BRIDGE DEPOSIT =====

function getCustodyUrl() {
    return getCustodyEndpoint();
}

async function showDepositInfo(chain) {
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    const chainInfo = {
        SOL: { name: 'Solana', chain: 'solana', color: '#9945FF', icon: 'fas fa-sun', tokens: ['USDC', 'USDT'] },
        ETH: { name: 'Ethereum', chain: 'ethereum', color: '#627EEA', icon: 'fab fa-ethereum', tokens: ['USDC', 'USDT'] }
    };
    const info = chainInfo[chain];
    if (!info) return;
    
    // Ask user which token they want to bridge
    const asset = info.tokens.length === 1 ? info.tokens[0].toLowerCase() : null;
    
    // Request deposit address from custody service
    const tokenSelect = info.tokens.map(t => 
        `<button class="btn btn-secondary btn-small" style="margin: 0.25rem;" onclick="requestDepositAddress('${info.chain}', '${t.toLowerCase()}', '${info.name}', '${info.icon}')">${t}</button>`
    ).join(' ');

    showConfirmModal({
        title: `Bridge from ${info.name}`,
        message: `<div style="text-align: left; font-size: 0.9rem;">
            <p style="margin-bottom: 0.75rem;">Select a token to deposit from ${info.name} → MoltChain:</p>
            <div style="display: flex; gap: 0.5rem; flex-wrap: wrap; margin-bottom: 1rem;">
                ${tokenSelect}
            </div>
            <p style="font-size: 0.8rem; color: var(--text-muted);">
                You'll receive a unique deposit address. Send tokens there and they'll be credited
                to your MoltChain wallet automatically (~2-5 min after source chain finality).
            </p>
        </div>`,
        icon: info.icon,
        confirmText: 'Close',
        cancelText: 'Cancel'
    });
}

async function requestDepositAddress(chain, asset, chainName, icon) {
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    // AUDIT-FIX W-H1: Validate inputs before sending to custody
    const validChains = ['solana', 'ethereum'];
    const validAssets = ['usdc', 'usdt'];
    if (!validChains.includes(chain)) {
        showToast('Invalid chain selected', 'error');
        return;
    }
    if (!validAssets.includes(asset)) {
        showToast('Invalid asset selected', 'error');
        return;
    }
    if (!wallet.address || wallet.address.length < 32 || wallet.address.length > 44) {
        showToast('Invalid wallet address', 'error');
        return;
    }
    
    // Close any open modals
    document.querySelectorAll('.password-modal').forEach(m => m.remove());
    
    try {
        showToast('Requesting deposit address...');
        
        const response = await fetch(`${getCustodyUrl()}/deposits`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                user_id: wallet.address,
                chain: chain,
                asset: asset
            })
        });
        
        if (!response.ok) {
            const err = await response.json().catch(() => ({ message: 'Request failed' }));
            showToast(err.message || 'Failed to create deposit address', 'error');
            return;
        }
        
        const data = await response.json();
        const depositAddress = data.address;
        const depositId = data.deposit_id;
        
        // AUDIT-FIX W-C2: Escape all server-provided values before HTML insertion
        // to prevent XSS if custody returns malicious data.
        const safeAddress = escapeHtml(depositAddress);
        const safeDepositId = escapeHtml(depositId);
        const safeAsset = escapeHtml(asset.toUpperCase());
        const safeChainName = escapeHtml(chainName);
        
        // Show deposit address with copy + status polling
        showConfirmModal({
            title: `Send ${safeAsset} on ${safeChainName}`,
            message: `<div style="text-align: left; font-size: 0.9rem;">
                <p style="margin-bottom: 0.75rem;">Send <strong>${safeAsset}</strong> to this ${safeChainName} address:</p>
                <div id="depositAddrBox" style="padding: 0.75rem; background: rgba(255,255,255,0.06); border: 1px solid var(--border); border-radius: 10px; font-family: monospace; font-size: 0.78rem; word-break: break-all; cursor: pointer;">
                    ${safeAddress}
                </div>
                <p id="copyHint" style="text-align: center; font-size: 0.75rem; color: var(--text-muted); margin-top: 0.35rem;">Click to copy</p>
                <div id="depositStatus" style="margin-top: 1rem; padding: 0.6rem 0.8rem; background: rgba(255,255,255,0.03); border-radius: 8px; font-size: 0.82rem;">
                    <i class="fas fa-clock" style="color: var(--accent);"></i> 
                    <span>Waiting for deposit...</span>
                </div>
                <p style="margin-top: 0.75rem; font-size: 0.78rem; color: var(--text-muted);">
                    This address expires in 24 hours. Funds sent after expiry may be lost.<br>
                    Deposit ID: <code style="font-size: 0.72rem;">${safeDepositId}</code>
                </p>
            </div>`,
            icon: icon,
            confirmText: 'Done',
            cancelText: 'Copy Address'
        }).then(confirmed => {
            stopDepositPolling();
            if (!confirmed) {
                navigator.clipboard.writeText(depositAddress).then(() => {
                    showToast('Deposit address copied!');
                });
            }
        });
        
        // AUDIT-FIX W-C2: Attach copy handler via JS instead of inline onclick
        const addrBox = document.getElementById('depositAddrBox');
        if (addrBox) {
            addrBox.addEventListener('click', () => {
                navigator.clipboard.writeText(depositAddress).then(() => {
                    const hint = document.getElementById('copyHint');
                    if (hint) {
                        hint.textContent = 'Copied!';
                        setTimeout(() => { hint.textContent = 'Click to copy'; }, 1500);
                    }
                });
            });
        }
        
        // Start polling deposit status
        startDepositPolling(depositId);
        
    } catch (error) {
        console.error('Deposit request failed:', error);
        showToast('Failed to connect to bridge service. Is custody running?', 'error');
    }
}

let depositPollInterval = null;
let depositPollTimeout = null;
const MAX_DEPOSIT_POLL_DURATION = 24 * 60 * 60 * 1000; // 24h — matches deposit TTL
const MAX_DEPOSIT_POLL_ERRORS = 20; // consecutive fetch failures before giving up

function _onBeforeUnloadStopPolling() { stopDepositPolling(); }

function startDepositPolling(depositId) {
    stopDepositPolling();
    let consecutiveErrors = 0;
    // Use longer interval when WebSocket bridge subscriptions are active (WS provides real-time updates)
    // Fall back to aggressive 5s polling when WS is disconnected
    const pollInterval = bridgeWsActive ? 30000 : 5000;

    // Hard timeout — stop polling after MAX_DEPOSIT_POLL_DURATION regardless
    depositPollTimeout = setTimeout(() => {
        console.warn('[Bridge] Deposit polling timed out after 24h');
        stopDepositPolling();
        const statusEl = document.getElementById('depositStatus');
        if (statusEl) {
            statusEl.innerHTML = '<i class="fas fa-times-circle" style="color: #EF476F;"></i> <span>Polling timed out. Check deposit status manually.</span>';
        }
    }, MAX_DEPOSIT_POLL_DURATION);

    // Clean up polling if user navigates away or closes tab
    window.addEventListener('beforeunload', _onBeforeUnloadStopPolling);

    depositPollInterval = setInterval(async () => {
        try {
            const res = await fetch(`${getCustodyUrl()}/deposits/${depositId}`);
            if (!res.ok) {
                consecutiveErrors++;
                if (consecutiveErrors >= MAX_DEPOSIT_POLL_ERRORS) {
                    console.error('[Bridge] Too many consecutive polling failures, stopping');
                    stopDepositPolling();
                }
                return;
            }
            consecutiveErrors = 0; // reset on success
            const deposit = await res.json();
            const statusEl = document.getElementById('depositStatus');
            if (!statusEl) {
                stopDepositPolling();
                return;
            }
            
            const statusMap = {
                'issued':    { icon: 'fas fa-clock', color: 'var(--text-muted)', text: 'Waiting for deposit...' },
                'pending':   { icon: 'fas fa-spinner fa-spin', color: '#FFD166', text: 'Deposit detected, confirming...' },
                'confirmed': { icon: 'fas fa-check-circle', color: '#06D6A0', text: 'Deposit confirmed! Sweeping to treasury...' },
                'swept':     { icon: 'fas fa-exchange-alt', color: '#06D6A0', text: 'Swept! Minting wrapped tokens on MoltChain...' },
                'credited':  { icon: 'fas fa-check-double', color: '#06D6A0', text: 'Credited to your MoltChain wallet!' },
                'expired':   { icon: 'fas fa-times-circle', color: '#EF476F', text: 'Deposit expired — address no longer active.' },
            };
            const s = statusMap[deposit.status] || statusMap['issued'];
            statusEl.innerHTML = `<i class="${s.icon}" style="color: ${s.color};"></i> <span>${s.text}</span>`;
            
            if (deposit.status === 'credited' || deposit.status === 'expired') {
                stopDepositPolling();
                if (deposit.status === 'credited') {
                    showToast('Bridge deposit credited!', 'success');
                    refreshBalance();
                    loadAssets();
                }
            }
        } catch(e) {
            consecutiveErrors++;
            if (consecutiveErrors >= MAX_DEPOSIT_POLL_ERRORS) {
                console.error('[Bridge] Too many consecutive polling failures, stopping');
                stopDepositPolling();
            }
        }
    }, pollInterval);
}

function stopDepositPolling() {
    if (depositPollInterval) {
        clearInterval(depositPollInterval);
        depositPollInterval = null;
    }
    if (depositPollTimeout) {
        clearTimeout(depositPollTimeout);
        depositPollTimeout = null;
    }
    window.removeEventListener('beforeunload', _onBeforeUnloadStopPolling);
}

function showSwap() {
    showToast('Use the MoltSwap DEX for trading -- launching with mainnet');
}

function showBuy() {
    showReceive('deposit');
}

// ===== NFT FUNCTIONS =====

async function refreshNFTs() {
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    const grid = document.getElementById('nftsGrid');
    const empty = document.getElementById('nftsEmpty');
    const countEl = document.getElementById('nftCount');
    
    try {
        // Try to fetch NFTs from RPC (getNFTsByOwner)
        let nfts = [];
        try {
            nfts = await rpc.call('getNFTsByOwner', [wallet.address]);
        } catch (e) {
            // RPC method may not exist yet - that's OK
        }
        
        if (nfts && nfts.length > 0) {
            countEl.textContent = `${nfts.length} NFT${nfts.length !== 1 ? 's' : ''}`;
            empty.style.display = 'none';
            grid.style.display = 'grid';
            grid.innerHTML = nfts.map(nft => `
                <div class="nft-card" onclick="showNFTDetail('${nft.mint || nft.id}')">
                    <div class="nft-image">
                        ${nft.image 
                            ? `<img src="${nft.image}" alt="${nft.name}" loading="lazy">` 
                            : `<div class="nft-placeholder"><i class="fas fa-gem"></i></div>`}
                    </div>
                    <div class="nft-info">
                        <span class="nft-collection">${nft.collection || 'Unknown'}</span>
                        <span class="nft-name">${nft.name || 'Unnamed'}</span>
                    </div>
                </div>
            `).join('');
        } else {
            countEl.textContent = '0 NFTs';
            grid.style.display = 'none';
            empty.style.display = 'flex';
        }
    } catch (error) {
        console.error('Failed to load NFTs:', error);
        countEl.textContent = '0 NFTs';
        grid.style.display = 'none';
        empty.style.display = 'flex';
    }
}

function showNFTDetail(mintId) {
    showToast('NFT details - launching with mainnet');
}

function openMarketplace() {
    showToast('MoltChain NFT Marketplace - launching with mainnet');
}

function formatMolt(shells) {
    if (typeof shells === 'string') shells = parseInt(shells) || 0;
    return fmtToken(shells / 1_000_000_000) + ' MOLT';
}

function escapeHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
}

function closeModal(modalId) {
    const modal = document.getElementById(modalId);
    if (modal) {
        modal.classList.remove('show');
        // Reset send form inputs when closing send modal
        if (modalId === 'sendModal') {
            const sendTo = document.getElementById('sendTo');
            const sendAmount = document.getElementById('sendAmount');
            const sendToken = document.getElementById('sendToken');
            if (sendTo) sendTo.value = '';
            if (sendAmount) sendAmount.value = '';
            if (sendToken) sendToken.value = 'MOLT';
        }
    }
}

function closeSettingsModal() {
    closeModal('settingsModal');
}

function copyAddress(type = 'native') {
    const address = type === 'evm' 
        ? document.getElementById('walletAddressEVM').value
        : document.getElementById('walletAddress').value;
    const label = type === 'evm' ? 'EVM address' : 'Native address';
    
    navigator.clipboard.writeText(address).then(() => {
        showToast(`✅ ${label} copied to clipboard!`);
    }).catch(() => {
        showToast('❌ Failed to copy');
    });
}

// Generate EVM-compatible address from Base58 address
// Implements Keccak256(pubkey)[12:32] derivation as per core/src/account.rs
function generateEVMAddress(base58Address) {
    try {
        // Check if required libraries are loaded
        if (typeof bs58 === 'undefined' || !bs58.decode) {
            console.error('bs58 library not loaded');
            throw new Error('bs58 not available');
        }
        
        // Check for keccak_256 function (js-sha3 v0.9.x exposes it globally)
        if (typeof keccak_256 === 'undefined') {
            console.error('keccak_256 library not loaded');
            throw new Error('keccak_256 not available');
        }
        
        // Decode Base58 to get 32-byte public key
        // console.log('Decoding base58 address:', base58Address);
        const pubkeyBytes = bs58.decode(base58Address);
        // console.log('Decoded pubkey bytes:', pubkeyBytes.length, 'bytes');
        
        if (pubkeyBytes.length !== 32) {
            console.error('Invalid pubkey length:', pubkeyBytes.length, 'expected 32');
            throw new Error(`Invalid pubkey length: ${pubkeyBytes.length}`);
        }
        
        // Hash with Keccak256 - js-sha3 v0.9.x returns hex string directly
        const hashHex = keccak_256(pubkeyBytes);
        
        // Take last 20 bytes (last 40 hex chars)
        const evmAddress = '0x' + hashHex.slice(-40);
        // console.log('Generated EVM address:', evmAddress);
        return evmAddress;
    } catch (e) {
        console.error('Failed to generate EVM address:', e);
        console.error('Error details:', e.message, e.stack);
        
        // Return error placeholder instead of broken fallback
        return '0x' + '0'.repeat(40);
    }
}

// Auto-register EVM address on-chain for seamless MetaMask compatibility
// Sends system instruction opcode 12 with the 20-byte EVM address
// Flow: localStorage cache → RPC check → tx → cache
async function registerEvmAddress(wallet, password) {
    try {
        const cacheKey = `moltEvmRegistered:${wallet.address}`;

        // 1) localStorage cache hit — skip entirely (no RPC, no tx)
        try { if (localStorage.getItem(cacheKey) === '1') return; } catch (_) {}

        // 2) On-chain check via RPC
        try {
            const existing = await rpc.call('getEvmRegistration', [wallet.address]);
            if (existing && existing.evmAddress) {
                // Already registered on-chain — cache locally and return
                try { localStorage.setItem(cacheKey, '1'); } catch (_) {}
                return;
            }
        } catch (_) {} // RPC down — fall through, processor is idempotent anyway

        // 3) Skip if account not funded yet (imported wallets)
        try {
            const bal = await rpc.getBalance(wallet.address);
            if (!bal || (bal.shells === 0 && !bal.spendable)) return;
        } catch (_) { return; }

        // 4) Derive EVM address
        const evmAddress = generateEVMAddress(wallet.address);
        if (!evmAddress || evmAddress === '0x' + '0'.repeat(40)) {
            console.warn('EVM address generation failed, skipping registration');
            return;
        }

        // 5) Build and send opcode 12 instruction
        const evmHex = evmAddress.slice(2);
        const evmBytes = new Uint8Array(20);
        for (let i = 0; i < 20; i++) {
            evmBytes[i] = parseInt(evmHex.substr(i * 2, 2), 16);
        }

        const instructionData = new Uint8Array(21);
        instructionData[0] = 12;
        instructionData.set(evmBytes, 1);

        const systemProgram = new Uint8Array(32); // SYSTEM_PROGRAM_ID = [0; 32]
        const fromPubkey = MoltCrypto.hexToBytes(wallet.publicKey);
        const latestBlock = await rpc.getLatestBlock();

        const message = {
            instructions: [{
                program_id: Array.from(systemProgram),
                accounts: [Array.from(fromPubkey)],
                data: Array.from(instructionData)
            }],
            blockhash: latestBlock.hash
        };

        const privateKey = await MoltCrypto.decryptPrivateKey(wallet.encryptedKey, password);
        const messageBytes = serializeMessageBincode(message);
        const signature = await MoltCrypto.signTransaction(privateKey, messageBytes);

        const transaction = { signatures: [Array.from(signature)], message };
        const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
        const txBase64 = btoa(String.fromCharCode(...txBytes));

        await rpc.sendTransaction(txBase64);
        // console.log('EVM address registered:', evmAddress, '→', wallet.address);

        // 6) Cache after successful registration
        try { localStorage.setItem(cacheKey, '1'); } catch (_) {}
    } catch (error) {
        // Don't block wallet creation on registration failure
        console.warn('EVM address registration deferred:', error.message);
    }
}

async function setMaxAmount() {
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    const selectedToken = document.getElementById('sendToken')?.value || 'MOLT';
    
    try {
        if (selectedToken === 'MOLT') {
            const balance = await rpc.getBalance(wallet.address);
            const molt = parseFloat(balance.molt) || 0;
            // Reserve 0.001 MOLT for fees
            const maxAmount = Math.max(0, molt - 0.001);
            document.getElementById('sendAmount').value = maxAmount.toFixed(6);
        } else {
            const bal = await getTokenBalanceFormatted(selectedToken, wallet.address);
            document.getElementById('sendAmount').value = bal.toFixed(6);
        }
    } catch (error) {
        showToast('Failed to fetch balance');
    }
}

// Update send modal UI when token selection changes
async function updateSendTokenUI() {
    const wallet = getActiveWallet();
    if (!wallet) return;
    
    const selectedToken = document.getElementById('sendToken')?.value || 'MOLT';
    const balanceHint = document.getElementById('sendAvailableBalance');
    if (!balanceHint) return;
    
    try {
        if (selectedToken === 'MOLT') {
            const balance = await rpc.getBalance(wallet.address);
            const molt = parseFloat(balance.molt) || 0;
            balanceHint.textContent = `Available: ${fmtToken(molt)} MOLT`;
        } else if (selectedToken === 'stMOLT') {
            const position = await rpc.call('getStakingPosition', [wallet.address]);
            const stMolt = (position?.st_molt_amount || 0) / 1_000_000_000;
            balanceHint.textContent = `Available: ${fmtToken(stMolt)} stMOLT`;
        } else {
            const bal = await getTokenBalanceFormatted(selectedToken, wallet.address);
            balanceHint.textContent = `Available: ${fmtToken(bal)} ${selectedToken}`;
        }
    } catch (error) {
        balanceHint.textContent = 'Available: --';
    }
}

async function confirmSend() {
    const to = document.getElementById('sendTo').value.trim();
    const amount = parseFloat(document.getElementById('sendAmount').value);
    const selectedToken = document.getElementById('sendToken')?.value || 'MOLT';
    
    if (!MoltCrypto.isValidAddress(to)) {
        alert('Invalid recipient address');
        return;
    }
    
    if (!amount || amount <= 0) {
        alert('Invalid amount');
        return;
    }
    
    const wallet = getActiveWallet();
    if (!wallet) return;

    // Pre-flight balance check: ensure enough MOLT for fees (and transfer if MOLT)
    try {
        const balResult = await rpc.call('getBalance', [wallet.address]);
        const spendable = (balResult?.spendable || balResult?.balance || 0) / 1_000_000_000;
        const baseFee = 0.001; // 1M shells = 0.001 MOLT
        const totalNeeded = selectedToken === 'MOLT' ? amount + baseFee : baseFee;
        if (spendable < totalNeeded) {
            showToast(`Insufficient MOLT balance: need ${fmtToken(totalNeeded)} MOLT (${selectedToken === 'MOLT' ? 'transfer + fee' : 'fee'}), have ${fmtToken(spendable)} spendable`);
            return;
        }
    } catch (e) {
        // Non-blocking: let the RPC reject it if balance is insufficient
    }
    
    try {
        showToast('Building transaction...');
        
        // Get recent blockhash
        const latestBlock = await rpc.getLatestBlock();
        const blockhash = latestBlock.hash;
        
        const fromPubkey = MoltCrypto.hexToBytes(wallet.publicKey);
        const toPubkey = bs58.decode(to);
        let message;
        
        if (selectedToken === 'MOLT') {
            // Native MOLT transfer
            const shells = Math.floor(amount * 1_000_000_000);
            const systemProgram = new Uint8Array(32); // SYSTEM_PROGRAM_ID = [0; 32]
            
            const instructionData = new Uint8Array(9);
            instructionData[0] = 0; // Transfer type
            const view = new DataView(instructionData.buffer);
            view.setBigUint64(1, BigInt(shells), true);
            
            message = {
                instructions: [{
                    program_id: Array.from(systemProgram),
                    accounts: [Array.from(fromPubkey), Array.from(toPubkey)],
                    data: Array.from(instructionData)
                }],
                blockhash: blockhash
            };
        } else if (selectedToken === 'stMOLT') {
            // stMOLT transfer via ReefStake opcode 16
            const stMoltShells = Math.floor(amount * 1_000_000_000);
            const systemProgram = new Uint8Array(32); // SYSTEM_PROGRAM_ID = [0; 32]

            const instructionData = new Uint8Array(9);
            instructionData[0] = 16; // ReefStake transfer
            const view = new DataView(instructionData.buffer);
            view.setBigUint64(1, BigInt(stMoltShells), true);

            message = {
                instructions: [{
                    program_id: Array.from(systemProgram),
                    accounts: [Array.from(fromPubkey), Array.from(toPubkey)],
                    data: Array.from(instructionData)
                }],
                blockhash: blockhash
            };
        } else {
            // Token contract transfer (Call instruction)
            const token = TOKEN_REGISTRY[selectedToken];
            if (!token || !token.address) {
                showToast(`❌ ${selectedToken} contract not deployed yet`);
                return;
            }
            
            const rawAmount = Math.floor(amount * Math.pow(10, token.decimals));
            const contractProgramId = new Uint8Array(32).fill(0xFF); // CONTRACT_PROGRAM_ID
            const tokenProgramPubkey = bs58.decode(token.address);
            
            // Build contract call payload: {"Call": {"function": "transfer", "args": [...], "value": 0}}
            const callArgs = JSON.stringify({
                to: Array.from(toPubkey),
                amount: rawAmount,
            });
            const callPayload = JSON.stringify({
                Call: {
                    function: "transfer",
                    args: Array.from(new TextEncoder().encode(callArgs)),
                    value: 0
                }
            });
            
            message = {
                instructions: [{
                    program_id: Array.from(contractProgramId),
                    accounts: [Array.from(fromPubkey), Array.from(tokenProgramPubkey)],
                    data: Array.from(new TextEncoder().encode(callPayload))
                }],
                blockhash: blockhash
            };
        }
        
        // Sign transaction with Ed25519
        const passwordValues = await showPasswordModal({
            title: 'Sign Transaction',
            message: `Send ${amount} ${selectedToken} to ${to}`,
            icon: 'fas fa-pen-nib',
            confirmText: 'Sign & Send',
            fields: [
                { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Enter password to sign' }
            ]
        });
        if (!passwordValues || !passwordValues.password) {
            showToast('Transaction cancelled');
            return;
        }
        
        const privateKey = await MoltCrypto.decryptPrivateKey(wallet.encryptedKey, passwordValues.password);
        const messageBytes = serializeMessageBincode(message);
        const signature = await MoltCrypto.signTransaction(privateKey, messageBytes);
        
        // Build signed transaction
        const transaction = {
            signatures: [Array.from(signature)],
            message: message
        };
        
        // Serialize and encode
        const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
        const txBase64 = btoa(String.fromCharCode(...txBytes));
        
        // Send transaction
        showToast('Sending transaction...');
        const txSignature = await rpc.sendTransaction(txBase64);
        
        showToast(`✅ ${amount} ${selectedToken} sent! Signature: ${String(txSignature).slice(0, 16)}...`);
        closeModal('sendModal');
        
        // Clear form and reset token selector
        document.getElementById('sendTo').value = '';
        document.getElementById('sendAmount').value = '';
        const tokenSelect = document.getElementById('sendToken');
        if (tokenSelect) tokenSelect.value = 'MOLT';
        
        // Wait briefly for block commitment, then refresh balance + activity
        await new Promise(r => setTimeout(r, 1500));
        await refreshBalance();
        await loadActivity();
        // Second refresh after another 3s to catch slower block finality
        setTimeout(async () => { try { await refreshBalance(); await loadActivity(); } catch(_){} }, 3000);
        
    } catch (error) {
        console.error('Send failed:', error);
        showToast('❌ Transaction failed: ' + error.message);
    }
}

function lockWallet() {
    stopBalancePolling();
    disconnectBalanceWebSocket();
    clearAllInputs();
    walletState.isLocked = true;
    saveWalletState();
    showToast('Wallet locked');
    checkWalletStatus();
}

function logoutWallet() {
    showConfirmModal({
        title: 'Logout',
        message: 'Are you sure you want to logout? Make sure you have backed up your seed phrase!',
        icon: 'fas fa-sign-out-alt',
        confirmText: 'Logout',
        cancelText: 'Cancel',
        danger: true
    }).then(confirmed => {
        if (!confirmed) return;
        
        stopBalancePolling();
        disconnectBalanceWebSocket();
        
        // Security: clear all input fields immediately
        clearAllInputs();
        
        // Clear ALL wallet data
        localStorage.clear();
        sessionStorage.clear();
        
        // Reset state completely (isLocked false — no wallet exists to lock)
        walletState = {
            hasWallet: false,
            isLocked: false,
            wallets: [],
            activeWalletId: null,
            network: 'testnet',
            settings: {}
        };
        saveWalletState();
        
        // Reset identity cache
        if (typeof _identityCache !== 'undefined') _identityCache = null;
        if (typeof _moltyidAddress !== 'undefined') _moltyidAddress = null;
        
        // Remove ALL modals immediately (password modals, confirm modals, send/receive/settings)
        document.querySelectorAll('.password-modal, .modal.show').forEach(m => {
            m.classList.remove('show');
            m.remove();
        });
        // Close static modals
        ['sendModal', 'receiveModal', 'settingsModal'].forEach(id => {
            const m = document.getElementById(id);
            if (m) m.classList.remove('show');
        });
        
        // Hide all screens including dashboard
        document.querySelectorAll('.wallet-screen, .wallet-dashboard').forEach(s => s.style.display = 'none');
        
        // Show welcome screen
        document.getElementById('welcomeScreen').style.display = 'flex';
        
        showToast('Logged out successfully');
    });
}

function showSettings() {
    document.getElementById('settingsModal').classList.add('show');
}

function showImportWalletFromDashboard() {
    // Close dropdown
    document.getElementById('walletDropdown').classList.remove('show');
    
    // Show import wallet screen
    document.getElementById('walletDashboard').style.display = 'none';
    document.getElementById('importWalletScreen').style.display = 'block';
    
    // Update back button behavior to return to dashboard
    const backButtons = document.querySelectorAll('#importWalletScreen .wallet-footer a');
    backButtons.forEach(btn => {
        btn.onclick = (e) => {
            e.preventDefault();
            document.getElementById('importWalletScreen').style.display = 'none';
            document.getElementById('walletDashboard').style.display = 'block';
        };
    });
}

function showCreateWalletFromDashboard() {
    // Close dropdown
    document.getElementById('walletDropdown').classList.remove('show');
    
    // Show create wallet screen
    document.getElementById('walletDashboard').style.display = 'none';
    showCreateWallet();
    
    // Update back button behavior to return to dashboard
    const backButtons = document.querySelectorAll('#createWalletScreen .wallet-footer a');
    backButtons.forEach(btn => {
        btn.onclick = (e) => {
            e.preventDefault();
            document.getElementById('createWalletScreen').style.display = 'none';
            document.getElementById('walletDashboard').style.display = 'block';
        };
    });
}

// ===== PASSWORD INPUT MODAL HELPERS =====

function showPasswordModal(options) {
    return new Promise((resolve) => {
        const modal = document.createElement('div');
        modal.className = 'password-modal';
        
        const fields = options.fields || [{ id: 'password', label: 'Password', type: 'password' }];
        const fieldsHTML = fields.map(field => `
            <div class="form-group">
                <label>${field.label}</label>
                <input type="${field.type}" id="${field.id}" class="form-input" placeholder="${field.placeholder || ''}">
            </div>
        `).join('');
        
        modal.innerHTML = `
            <div class="password-modal-content">
                <div class="password-modal-header">
                    <h3><i class="${options.icon || 'fas fa-lock'}"></i> ${options.title}</h3>
                    <button class="modal-close password-modal-close-btn">
                        <i class="fas fa-times"></i>
                    </button>
                </div>
                <div class="password-modal-body">
                    ${options.message ? `<p>${options.message}</p>` : ''}
                    ${fieldsHTML}
                    <div class="password-modal-actions">
                        <button class="btn btn-secondary password-modal-cancel-btn">
                            <i class="fas fa-times"></i> Cancel
                        </button>
                        <button class="btn btn-primary" id="passwordModalConfirm">
                            <i class="fas fa-check"></i> ${options.confirmText || 'Confirm'}
                        </button>
                    </div>
                </div>
            </div>
        `;
        
        document.body.appendChild(modal);
        requestAnimationFrame(() => modal.classList.add('show'));
        
        // Focus first input
        setTimeout(() => {
            const firstInput = modal.querySelector('input');
            if (firstInput) firstInput.focus();
        }, 100);
        
        const dismiss = () => {
            modal.classList.remove('show');
            setTimeout(() => modal.remove(), 300);
            resolve(null);
            document.removeEventListener('keydown', handleEsc);
        };
        
        // Handle confirm
        const confirmBtn = modal.querySelector('#passwordModalConfirm');
        const handleConfirm = () => {
            const values = {};
            fields.forEach(field => {
                values[field.id] = modal.querySelector(`#${field.id}`).value;
            });
            modal.classList.remove('show');
            setTimeout(() => modal.remove(), 300);
            resolve(values);
            document.removeEventListener('keydown', handleEsc);
        };
        
        confirmBtn.onclick = handleConfirm;
        modal.querySelector('.password-modal-close-btn').onclick = dismiss;
        modal.querySelector('.password-modal-cancel-btn').onclick = dismiss;
        
        // Handle enter key
        modal.querySelectorAll('input').forEach(input => {
            input.onkeypress = (e) => {
                if (e.key === 'Enter') handleConfirm();
            };
        });
        
        // Handle ESC key
        const handleEsc = (e) => {
            if (e.key === 'Escape') dismiss();
        };
        document.addEventListener('keydown', handleEsc);
        
        // Handle backdrop click
        modal.onclick = (e) => {
            if (e.target === modal) dismiss();
        };
    });
}

function showConfirmModal(options) {
    return new Promise((resolve) => {
        const modal = document.createElement('div');
        modal.className = 'password-modal';
        
        modal.innerHTML = `
            <div class="password-modal-content">
                <div class="password-modal-header">
                    <h3><i class="${options.icon || 'fas fa-question-circle'}"></i> ${options.title}</h3>
                    <button class="modal-close password-modal-close-btn">
                        <i class="fas fa-times"></i>
                    </button>
                </div>
                <div class="password-modal-body">
                    <p>${options.message}</p>
                    ${options.requireInput ? `
                        <div class="form-group">
                            <label>${options.inputLabel}</label>
                            <input type="text" id="confirmInput" class="form-input" placeholder="${options.inputPlaceholder || ''}">
                        </div>
                    ` : ''}
                    <div class="password-modal-actions">
                        <button class="btn btn-secondary confirm-modal-cancel-btn">
                            <i class="fas fa-times"></i> ${options.cancelText || 'Cancel'}
                        </button>
                        <button class="btn ${options.danger ? 'btn-danger' : 'btn-primary'}" id="confirmModalBtn">
                            <i class="fas fa-check"></i> ${options.confirmText || 'Confirm'}
                        </button>
                    </div>
                </div>
            </div>
        `;
        
        document.body.appendChild(modal);
        requestAnimationFrame(() => modal.classList.add('show'));
        
        if (options.requireInput) {
            const input = modal.querySelector('#confirmInput');
            setTimeout(() => input.focus(), 100);
        }
        
        const dismiss = () => {
            modal.classList.remove('show');
            setTimeout(() => modal.remove(), 300);
            resolve(options.requireInput ? null : false);
            document.removeEventListener('keydown', handleEsc);
        };
        
        const confirmBtn = modal.querySelector('#confirmModalBtn');
        const handleConfirm = () => {
            modal.classList.remove('show');
            setTimeout(() => modal.remove(), 300);
            document.removeEventListener('keydown', handleEsc);
            if (options.requireInput) {
                resolve(modal.querySelector('#confirmInput').value);
            } else {
                resolve(true);
            }
        };
        
        confirmBtn.onclick = handleConfirm;
        modal.querySelector('.password-modal-close-btn').onclick = dismiss;
        modal.querySelector('.confirm-modal-cancel-btn').onclick = dismiss;
        
        if (options.requireInput) {
            modal.querySelector('#confirmInput').onkeypress = (e) => {
                if (e.key === 'Enter') handleConfirm();
            };
        }
        
        const handleEsc = (e) => {
            if (e.key === 'Escape') dismiss();
        };
        document.addEventListener('keydown', handleEsc);
        
        modal.onclick = (e) => {
            if (e.target === modal) dismiss();
        };
    });
}

// ===== EXPORT FUNCTIONS WITH MODALS =====

function showExportPrivateKey() {
    showPasswordModal({
        title: 'Export Private Key',
        message: 'Enter your password to export your private key',
        icon: 'fas fa-key',
        confirmText: 'Export'
    }).then(values => {
        if (!values) return;
        exportPrivateKeyWithPassword(values.password);
    });
}

async function exportPrivateKeyWithPassword(password) {
    
    try {
        const wallet = getActiveWallet();
        if (!wallet) {
            showToast('❌ No active wallet');
            return;
        }
        
        // Verify password
        const testDecrypt = await MoltCrypto.decryptKeypair(wallet.encryptedKey, password);
        if (!testDecrypt) {
            showToast('❌ Invalid password');
            return;
        }
        
        // Show private key in modal — export the 32-byte seed (64 hex chars)
        // so it matches the import format (which expects 64 hex chars = 32 bytes)
        const privateKeyHex = testDecrypt.privateKey || Array.from(testDecrypt.secretKey.slice(0, 32))
            .map(b => b.toString(16).padStart(2, '0'))
            .join('');
        
        closeModal('settingsModal');
        
        const modal = document.createElement('div');
        modal.className = 'modal';
        modal.innerHTML = `
            <div class="modal-content">
                <div class="modal-header">
                    <h3><i class="fas fa-key"></i> Private Key</h3>
                    <button class="modal-close" onclick="this.closest('.modal').classList.remove('show'); setTimeout(() => this.closest('.modal').remove(), 300);">
                        <i class="fas fa-times"></i>
                    </button>
                </div>
                <div class="modal-body">
                    <div class="warning-box" style="margin-bottom: 1rem;">
                        <i class="fas fa-exclamation-triangle"></i>
                        <strong>⚠️ Never share this key with anyone!</strong>
                    </div>
                    
                    <label style="font-weight: 600; margin-bottom: 0.5rem; display: block;">Private Key (Hex)</label>
                    <textarea class="form-input" readonly style="font-family: monospace; font-size: 0.85rem; height: 100px;">${privateKeyHex}</textarea>
                    
                    <div style="display: flex; gap: 0.75rem; margin-top: 1rem;">
                        <button class="btn btn-primary" onclick="navigator.clipboard.writeText('${privateKeyHex}').then(() => showToast('✅ Private key copied!')); this.closest('.modal').classList.remove('show'); setTimeout(() => this.closest('.modal').remove(), 300);">
                            <i class="fas fa-copy"></i> Copy
                        </button>
                        <button class="btn btn-secondary" onclick="downloadPrivateKey('${privateKeyHex}', '${wallet.name}');">
                            <i class="fas fa-download"></i> Download
                        </button>
                    </div>
                </div>
            </div>
        `;
        document.body.appendChild(modal);
        requestAnimationFrame(() => modal.classList.add('show'));
        
    } catch (e) {
        showToast('❌ Failed to export private key');
    }
}

function downloadPrivateKey(privateKeyHex, walletName) {
    const filename = `molt-wallet-privatekey-${walletName}-${Date.now()}.txt`;
    const content = `MoltWallet Private Key\n` +
                   `DO NOT SHARE THIS WITH ANYONE!\n\n` +
                   `Wallet: ${walletName}\n` +
                   `Exported: ${new Date().toISOString()}\n\n` +
                   `Private Key (Hex):\n${privateKeyHex}\n\n` +
                   `⚠️ Anyone with this key can access your funds!\n` +
                   `Keep it safe and offline.`;
    
    const blob = new Blob([content], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);
    showToast('✅ Private key downloaded!');
    const openModal = document.querySelector('.modal.show');
    if (openModal) { openModal.classList.remove('show'); setTimeout(() => openModal.remove(), 300); }
}

function showExportJSON() {
    showPasswordModal({
        title: 'Export JSON Keystore',
        message: 'Enter your password to export an encrypted keystore file',
        icon: 'fas fa-file-code',
        confirmText: 'Export'
    }).then(values => {
        if (!values) return;
        exportJSONWithPassword(values.password);
    });
}

async function exportJSONWithPassword(password) {
    if (!password) return;
    
    try {
        const wallet = getActiveWallet();
        if (!wallet) {
            showToast('❌ No active wallet');
            return;
        }
        
        // Verify password
        const keypair = await MoltCrypto.decryptKeypair(wallet.encryptedKey, password);
        if (!keypair) {
            showToast('❌ Invalid password');
            return;
        }
        
        // Create JSON keystore
        const keystore = {
            name: wallet.name,
            address: wallet.address,
            publicKey: Array.from(keypair.publicKey),
            secretKey: Array.from(keypair.secretKey),
            created: wallet.created,
            exported: new Date().toISOString(),
            version: '1.0'
        };
        
        const filename = `molt-wallet-keystore-${wallet.name}-${Date.now()}.json`;
        const blob = new Blob([JSON.stringify(keystore, null, 2)], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = filename;
        a.click();
        URL.revokeObjectURL(url);
        
        showToast('✅ JSON keystore exported!');
        
    } catch (e) {
        showToast('❌ Failed to export JSON keystore');
    }
}

function showExportMnemonic() {
    const wallet = getActiveWallet();
    if (!wallet || (!wallet.encryptedMnemonic && !wallet.mnemonic && !wallet.hasMnemonic)) {
        showToast('❌ No seed phrase available (imported wallet?)');
        return;
    }
    
    showPasswordModal({
        title: 'View Seed Phrase',
        message: 'Enter your password to view your seed phrase',
        icon: 'fas fa-list-ol',
        confirmText: 'View'
    }).then(values => {
        if (!values) return;
        exportMnemonicWithPassword(values.password);
    });
}

async function exportMnemonicWithPassword(password) {
    const wallet = getActiveWallet();
    if (!wallet) {
        showToast('❌ No seed phrase available');
        return;
    }
    
    try {
        // Verify password
        const keypair = await MoltCrypto.decryptKeypair(wallet.encryptedKey, password);
        if (!keypair) {
            showToast('❌ Invalid password');
            return;
        }
        
        // Decrypt the mnemonic
        let mnemonic;
        if (wallet.encryptedMnemonic) {
            mnemonic = await MoltCrypto.decryptPrivateKey(wallet.encryptedMnemonic, password);
        } else if (wallet.mnemonic) {
            // Legacy: migrate plaintext mnemonic to encrypted
            mnemonic = wallet.mnemonic;
            wallet.encryptedMnemonic = await MoltCrypto.encryptPrivateKey(mnemonic, password);
            wallet.hasMnemonic = true;
            delete wallet.mnemonic;
            saveWalletState();
        } else {
            showToast('❌ No seed phrase available');
            return;
        }
        
        const words = mnemonic.split(' ');
        const escapedMnemonic = mnemonic.replace(/'/g, "\\'");
        
        closeModal('settingsModal');
        
        const modal = document.createElement('div');
        modal.className = 'modal';
        modal.id = 'seedPhraseExportModal';
        modal.innerHTML = `
            <div class="modal-content">
                <div class="modal-header">
                    <h3><i class="fas fa-list-ol"></i> Seed Phrase</h3>
                    <button class="modal-close" onclick="this.closest('.modal').classList.remove('show'); setTimeout(() => this.closest('.modal').remove(), 300);">
                        <i class="fas fa-times"></i>
                    </button>
                </div>
                <div class="modal-body">
                    <div class="warning-box" style="margin-bottom: 1rem;">
                        <i class="fas fa-exclamation-triangle"></i>
                        <strong>⚠️ Never share your seed phrase!</strong>
                    </div>
                    
                    <div class="seed-phrase">
                        ${words.map((word, i) => `
                            <div class="seed-word">
                                <span class="seed-word-number">${i + 1}.</span>
                                <span>${word}</span>
                            </div>
                        `).join('')}
                    </div>
                    
                    <div style="display: flex; gap: 0.75rem; margin-top: 1rem;">
                        <button class="btn btn-primary" onclick="navigator.clipboard.writeText('${escapedMnemonic}').then(() => showToast('✅ Seed phrase copied!')); this.closest('.modal').classList.remove('show'); setTimeout(() => this.closest('.modal').remove(), 300);">
                            <i class="fas fa-copy"></i> Copy
                        </button>
                        <button class="btn btn-secondary" onclick="downloadMnemonicExport('${escapedMnemonic}', '${wallet.name}');">
                            <i class="fas fa-download"></i> Download
                        </button>
                    </div>
                </div>
            </div>
        `;
        document.body.appendChild(modal);
        requestAnimationFrame(() => modal.classList.add('show'));
        
    } catch (e) {
        showToast('❌ Failed to view seed phrase');
    }
}

function downloadMnemonicExport(mnemonic, walletName) {
    const filename = `molt-wallet-seed-${walletName}-${Date.now()}.txt`;
    const content = `MoltWallet Seed Phrase\n` +
                   `DO NOT SHARE THIS WITH ANYONE!\n\n` +
                   `Wallet: ${walletName}\n` +
                   `Exported: ${new Date().toISOString()}\n\n` +
                   `Seed Phrase (12 words):\n${mnemonic}\n\n` +
                   `⚠️ Anyone with this phrase can access your funds!\n` +
                   `Keep it safe and offline.`;
    
    const blob = new Blob([content], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);
    showToast('✅ Seed phrase downloaded!');
    const seedModal = document.getElementById('seedPhraseExportModal');
    if (seedModal) { seedModal.classList.remove('show'); setTimeout(() => seedModal.remove(), 300); }
}

// ===== UTILITIES =====
function showToast(message) {
    const toast = document.createElement('div');
    toast.className = 'toast';
    toast.textContent = message;
    toast.style.cssText = `
        position: fixed;
        bottom: 2rem;
        right: 2rem;
        background: var(--primary);
        color: white;
        padding: 1rem 1.5rem;
        border-radius: 8px;
        font-weight: 600;
        box-shadow: 0 4px 16px rgba(0,0,0,0.3);
        z-index: 10001;
        animation: slideIn 0.3s ease;
    `;
    document.body.appendChild(toast);
    setTimeout(() => toast.remove(), 3000);
}

function setupEventListeners() {
    // Close dropdowns when clicking outside
    document.addEventListener('click', (e) => {
        if (!e.target.closest('.wallet-selector')) {
            document.getElementById('walletDropdown').classList.remove('show');
        }
    });
    
    // Close modals when clicking outside
    document.querySelectorAll('.modal').forEach(modal => {
        modal.addEventListener('click', (e) => {
            if (e.target === modal) {
                modal.classList.remove('show');
            }
        });
    });
}

// Auto-lock timer
let lockTimer;
function resetLockTimer() {
    clearTimeout(lockTimer);
    if (!walletState.isLocked) {
        lockTimer = setTimeout(() => {
            lockWallet();
        }, walletState.settings.lockTimeout);
    }
}

document.addEventListener('mousemove', resetLockTimer);
document.addEventListener('keypress', resetLockTimer);

// ===== NETWORK SELECTOR=====
const NETWORK_LABELS = {
    'mainnet': 'Mainnet',
    'testnet': 'Testnet',
    'local-testnet': 'Local Testnet',
    'local-mainnet': 'Local Mainnet'
};

const NETWORK_COLORS = {
    'mainnet': '#4ade80',
    'testnet': '#fbbf24',
    'local-testnet': '#38bdf8',
    'local-mainnet': '#a78bfa'
};

function initNetworkSelector() {
    const networkSelect = document.getElementById('networkSelect');
    if (!networkSelect) return;

    // Restore saved network
    const savedNetwork = getSelectedNetwork();
    walletState.network = savedNetwork;
    networkSelect.value = savedNetwork;
}

function switchNetwork(network) {
    localStorage.setItem('moltchain_wallet_network', network);
    walletState.network = network;
    saveWalletState();

    // Update RPC client endpoint
    rpc.url = getRpcEndpoint();
    
    // Restart WS + polling on new endpoint
    stopBalancePolling();
    disconnectBalanceWebSocket();
    connectBalanceWebSocket();
    startBalancePolling();

    showToast(`Switched to ${NETWORK_LABELS[network] || network}`);

    // Refresh wallet data after network switch
    if (typeof showDashboard === 'function') {
        showDashboard();
    }
}

function updateNetworkDisplay() {
    const networkSelect = document.getElementById('networkSelect');
    const network = getSelectedNetwork();
    if (networkSelect) {
        networkSelect.value = network;
    }
}

// ===== SETTINGS FUNCTIONS =====

function saveNetworkSettings() {
    const mainnetRPC = document.getElementById('mainnetRPC').value;
    const testnetRPC = document.getElementById('testnetRPC').value;
    
    if (!mainnetRPC || !testnetRPC) {
        showToast('❌ Please fill in all RPC URLs');
        return;
    }
    
    walletState.settings = walletState.settings || {};
    walletState.settings.mainnetRPC = mainnetRPC;
    walletState.settings.testnetRPC = testnetRPC;
    
    saveWalletState();
    showToast('✅ Network settings saved!');
}

function saveAutoLockTimer() {
    const minutes = parseInt(document.getElementById('autoLockTimer').value);
    
    walletState.settings = walletState.settings || {};
    walletState.settings.autoLockMinutes = minutes;
    walletState.settings.lockTimeout = minutes * 60 * 1000; // Convert to milliseconds
    
    saveWalletState();
    showToast(`✅ Auto-lock set to ${minutes === 0 ? 'Never' : minutes + ' minutes'}`);
    
    // Reset timer with new value
    if (minutes > 0) {
        resetLockTimer();
    }
}

function saveCurrencyDisplay() {
    const currency = document.getElementById('currencyDisplay').value;
    
    walletState.settings = walletState.settings || {};
    walletState.settings.currency = currency;
    
    saveWalletState();
    showToast(`✅ Currency set to ${currency}`);
    
    // Refresh balance display
    refreshBalance();
}

function saveDecimalPlaces() {
    const decimals = parseInt(document.getElementById('decimalPlaces').value);
    
    walletState.settings = walletState.settings || {};
    walletState.settings.decimals = decimals;
    
    saveWalletState();
    showToast(`✅ Decimal places set to ${decimals}`);
    
    // Refresh balance display
    refreshBalance();
}

function showChangePassword() {
    showPasswordModal({
        title: 'Change Password',
        message: 'Update your wallet encryption password',
        icon: 'fas fa-lock',
        confirmText: 'Continue',
        fields: [
            { id: 'currentPassword', label: 'Current Password', type: 'password', placeholder: 'Enter current password' }
        ]
    }).then(values => {
        if (!values) return;
        changePasswordStep2(values.currentPassword);
    });
}

async function changePasswordStep2(oldPassword) {
    const wallet = getActiveWallet();
    if (!wallet) {
        showToast('❌ No active wallet');
        return;
    }
    
    // Verify old password
    const keypair = await MoltCrypto.decryptKeypair(wallet.encryptedKey, oldPassword);
    if (!keypair) {
        showToast('❌ Invalid password');
        return;
    }
    
    // Ask for new password
    showPasswordModal({
        title: 'New Password',
        message: 'Choose a strong password (minimum 8 characters)',
        icon: 'fas fa-key',
        confirmText: 'Change Password',
        fields: [
            { id: 'newPassword', label: 'New Password', type: 'password', placeholder: 'Minimum 8 characters' },
            { id: 'confirmPassword', label: 'Confirm Password', type: 'password', placeholder: 'Re-enter password' }
        ]
    }).then(async values => {
        if (!values) return;
        
        if (values.newPassword !== values.confirmPassword) {
            showToast('❌ Passwords do not match');
            return;
        }
        
        if (values.newPassword.length < 8) {
            showToast('❌ Password must be at least 8 characters');
            return;
        }
        
        // Re-encrypt with new password
        wallet.encryptedKey = await MoltCrypto.encryptKeypair(keypair, values.newPassword);
        
        // Re-encrypt mnemonic if it exists
        if (wallet.encryptedMnemonic) {
            const mnemonic = await MoltCrypto.decryptPrivateKey(wallet.encryptedMnemonic, oldPassword);
            wallet.encryptedMnemonic = await MoltCrypto.encryptPrivateKey(mnemonic, values.newPassword);
        } else if (wallet.mnemonic) {
            // Migrate plaintext mnemonic
            wallet.encryptedMnemonic = await MoltCrypto.encryptPrivateKey(wallet.mnemonic, values.newPassword);
            wallet.hasMnemonic = true;
            delete wallet.mnemonic;
        }
        
        // Update in state
        const walletIndex = walletState.wallets.findIndex(w => w.id === wallet.id);
        if (walletIndex !== -1) {
            walletState.wallets[walletIndex] = wallet;
            saveWalletState();
            showToast('✅ Password changed successfully!');
        }
    });
}

function showRenameWallet() {
    const wallet = getActiveWallet();
    if (!wallet) {
        showToast('❌ No active wallet');
        return;
    }
    
    showPasswordModal({
        title: 'Rename Wallet',
        message: 'Choose a new name for your wallet',
        icon: 'fas fa-edit',
        confirmText: 'Rename',
        fields: [
            { id: 'walletName', label: 'Wallet Name', type: 'text', placeholder: wallet.name }
        ]
    }).then(values => {
        if (!values || !values.walletName) return;
        
        const newName = values.walletName.trim();
        if (!newName || newName === wallet.name) return;
        
        wallet.name = newName;
        
        // Update in state
        const walletIndex = walletState.wallets.findIndex(w => w.id === wallet.id);
        if (walletIndex !== -1) {
            walletState.wallets[walletIndex] = wallet;
            saveWalletState();
            
            // Update UI
            document.getElementById('currentWalletName').textContent = newName;
            setupWalletSelector();
            
            showToast('✅ Wallet renamed!');
        }
    });
}

function clearTransactionHistory() {
    showConfirmModal({
        title: 'Clear History',
        message: 'Clear all cached transaction history? This will not affect your actual on-chain transactions.',
        icon: 'fas fa-eraser',
        confirmText: 'Clear',
        cancelText: 'Cancel'
    }).then(confirmed => {
        if (!confirmed) return;
        
        const wallet = getActiveWallet();
        if (!wallet) {
            showToast('❌ No active wallet');
            return;
        }
        
        // Clear cached transactions
        wallet.cachedTransactions = [];
        
        const walletIndex = walletState.wallets.findIndex(w => w.id === wallet.id);
        if (walletIndex !== -1) {
            walletState.wallets[walletIndex] = wallet;
            saveWalletState();
            showToast('✅ Transaction history cleared!');
        }
    });
}

function showDeleteWallet() {
    const wallet = getActiveWallet();
    if (!wallet) {
        showToast('❌ No active wallet');
        return;
    }
    
    if (walletState.wallets.length === 1) {
        showConfirmModal({
            title: 'Delete Wallet',
            message: 'This is your only wallet. Deleting it will log you out. Make sure you have backed up your seed phrase!',
            icon: 'fas fa-exclamation-triangle',
            confirmText: 'Delete & Logout',
            cancelText: 'Cancel',
            danger: true
        }).then(confirmed => {
            if (confirmed) logoutWallet();
        });
        return;
    }
    
    showConfirmModal({
        title: 'Delete Wallet',
        message: `Delete wallet "${wallet.name}"? This action cannot be undone!\n\nMake sure you have backed up your seed phrase!`,
        icon: 'fas fa-trash',
        confirmText: 'Continue',
        cancelText: 'Cancel',
        danger: true
    }).then(confirmed => {
        if (!confirmed) return;
        
        showConfirmModal({
            title: 'Confirm Deletion',
            message: `Type "${wallet.name}" to confirm deletion:`,
            icon: 'fas fa-exclamation-triangle',
            confirmText: 'Delete',
            cancelText: 'Cancel',
            danger: true,
            requireInput: true,
            inputLabel: 'Wallet Name',
            inputPlaceholder: wallet.name
        }).then(inputValue => {
            if (inputValue !== wallet.name) {
                if (inputValue) showToast('❌ Deletion cancelled - name did not match');
                return;
            }
            
            // Remove wallet from list
            walletState.wallets = walletState.wallets.filter(w => w.id !== wallet.id);
            
            // Switch to first remaining wallet
            if (walletState.wallets.length > 0) {
                walletState.activeWalletId = walletState.wallets[0].id;
                saveWalletState();
                
                closeSettingsModal();
                showDashboard();
                showToast('✅ Wallet deleted');
            }
        });
    });
}

// Load settings values when opening settings modal
function loadSettingsValues() {
    const settings = walletState.settings || {};
    
    if (document.getElementById('networkSelect')) {
        document.getElementById('networkSelect').value = getSelectedNetwork();
    }
    
    if (document.getElementById('mainnetRPC')) {
        document.getElementById('mainnetRPC').value = settings.mainnetRPC || 'http://localhost:8899';
    }
    
    if (document.getElementById('testnetRPC')) {
        document.getElementById('testnetRPC').value = settings.testnetRPC || 'http://localhost:8899';
    }
    
    if (document.getElementById('autoLockTimer')) {
        document.getElementById('autoLockTimer').value = settings.autoLockMinutes || 15;
    }
    
    if (document.getElementById('currencyDisplay')) {
        document.getElementById('currencyDisplay').value = settings.currency || 'USD';
    }
    
    if (document.getElementById('decimalPlaces')) {
        document.getElementById('decimalPlaces').value = settings.decimals || 6;
    }
}

// Override showSettings to load values when modal opens
const _originalShowSettings = showSettings;
showSettings = function() {
    _originalShowSettings();
    setTimeout(loadSettingsValues, 100); // Small delay to ensure modal is rendered
};

// console.log('MoltWallet fully initialized');
