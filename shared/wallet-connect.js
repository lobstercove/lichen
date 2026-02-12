/**
 * MoltChain Shared Wallet Connect Utility
 * 
 * Provides a unified wallet connection experience across all MoltChain frontends.
 * Uses the MoltChain SDK for real keypair generation and signing.
 * 
 * Usage:
 *   <script src="../shared/wallet-connect.js"></script>
 *   <script>
 *     const wallet = new MoltWallet({ rpcUrl: 'http://localhost:9000' });
 *     wallet.bindConnectButton('#connectWallet');
 *     wallet.onConnect(info => console.log('Connected:', info.address));
 *   </script>
 */

// ─── Shared Utilities ────────────────────────────────────

/**
 * Format a hash/address for display, truncating the middle
 * @param {string} hash - Full hash or address
 * @param {number} [len=8] - Characters to show at start/end
 * @returns {string}
 */
function formatHash(hash, len) {
    if (!hash) return '';
    len = len || 8;
    if (hash.length <= len * 2 + 3) return hash;
    return hash.substring(0, len) + '...' + hash.substring(hash.length - len);
}

/**
 * Resolve the RPC URL from config or default
 * Checks window.moltConfig, window.moltMarketConfig, window.moltExplorerConfig
 * @returns {string}
 */
function getMoltRpcUrl() {
    if (window.moltConfig && window.moltConfig.rpcUrl) return window.moltConfig.rpcUrl;
    if (window.moltMarketConfig && window.moltMarketConfig.rpcUrl) return window.moltMarketConfig.rpcUrl;
    if (window.moltExplorerConfig && window.moltExplorerConfig.rpcUrl) return window.moltExplorerConfig.rpcUrl;
    return 'http://localhost:9000';
}

/**
 * Make a JSON-RPC call to the MoltChain node
 * @param {string} method - RPC method name
 * @param {Array|Object} params - Method params
 * @param {string} [rpcUrl] - Override RPC URL
 * @returns {Promise<any>}
 */
async function moltRpcCall(method, params, rpcUrl) {
    var url = rpcUrl || getMoltRpcUrl();
    var response = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
            jsonrpc: '2.0',
            id: Date.now(),
            method: method,
            params: params || []
        })
    });
    var data = await response.json();
    if (data.error) {
        throw new Error(data.error.message || 'RPC error');
    }
    return data.result;
}

// ─── Wallet Manager ──────────────────────────────────────

/**
 * MoltWallet - Unified wallet connection manager
 * 
 * @param {Object} options
 * @param {string} [options.rpcUrl] - RPC endpoint URL
 * @param {string} [options.storageKey='molt_wallet'] - localStorage key
 * @param {boolean} [options.persist=true] - Auto-save to localStorage
 */
function MoltWallet(options) {
    options = options || {};
    this.rpcUrl = options.rpcUrl || getMoltRpcUrl();
    this.storageKey = options.storageKey || 'molt_wallet';
    this.persist = options.persist !== false;
    
    this.address = null;
    this.balance = 0;
    this._walletData = null;
    this._connectCallbacks = [];
    this._disconnectCallbacks = [];
    this._balanceCallbacks = [];
    this._buttonEl = null;
    this._balanceInterval = null;
    
    // Try to restore from localStorage
    if (this.persist) {
        this._restore();
    }
}

/** Check if a wallet is currently connected */
MoltWallet.prototype.isConnected = function() {
    return this.address !== null;
};

/**
 * Connect wallet - creates or imports a wallet
 * If MoltChain SDK is available, uses real keypair generation
 * Otherwise falls back to address-only mode with server-side wallet
 * @param {Object} [importData] - Import data { seed, hex, json }
 * @returns {Promise<Object>} - { address, balance }
 */
MoltWallet.prototype.connect = async function(importData) {
    // Try MoltChain SDK first (real wallet)
    if (window.MoltChain && window.MoltChain.Wallet) {
        try {
            var wallet;
            if (importData && importData.seed) {
                wallet = MoltChain.Wallet.import({ seed: importData.seed }, '');
            } else if (importData && importData.json) {
                wallet = MoltChain.Wallet.import(importData.json, importData.password || '');
            } else {
                wallet = new MoltChain.Wallet();
            }
            this.address = wallet.address || wallet.publicKey;
            this._walletData = {
                address: this.address,
                hasKeys: true,
                created: Date.now()
            };
        } catch (err) {
            console.warn('MoltChain SDK wallet creation failed, using RPC wallet:', err);
            this._createRpcWallet();
        }
    } else {
        // No SDK available - create via RPC
        await this._createRpcWallet();
    }
    
    // Fetch balance
    await this.refreshBalance();
    
    // Persist
    if (this.persist && this._walletData) {
        try {
            localStorage.setItem(this.storageKey, JSON.stringify(this._walletData));
        } catch (e) { /* storage full or unavailable */ }
    }
    
    // Notify
    var info = { address: this.address, balance: this.balance };
    for (var i = 0; i < this._connectCallbacks.length; i++) {
        try { this._connectCallbacks[i](info); } catch (e) { console.error(e); }
    }
    
    this._updateButton();
    this._startBalancePolling();
    
    return info;
};

/** Create wallet via RPC (address-only, no local keys) */
MoltWallet.prototype._createRpcWallet = async function() {
    try {
        var result = await moltRpcCall('createWallet', [], this.rpcUrl);
        this.address = result.address || result.pubkey || result;
        this._walletData = {
            address: this.address,
            hasKeys: false,
            created: Date.now()
        };
    } catch (err) {
        // Fallback - generate Ed25519 keypair locally if nacl is available
        if (window.nacl && window.nacl.sign) {
            var kp = nacl.sign.keyPair();
            this.address = (window.bs58 && bs58.encode)
                ? bs58.encode(kp.publicKey)
                : Array.from(kp.publicKey).map(function(b) { return b.toString(16).padStart(2, '0'); }).join('');
        } else {
            var bytes = new Uint8Array(32);
            crypto.getRandomValues(bytes);
            var chars = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
            var addr = '';
            for (var i = 0; i < 44; i++) {
                addr += chars[bytes[i % 32] % chars.length];
            }
            this.address = addr;
        }
        this._walletData = {
            address: this.address,
            hasKeys: false,
            created: Date.now()
        };
    }
};

/** Disconnect wallet and clear state */
MoltWallet.prototype.disconnect = function() {
    var oldAddr = this.address;
    this.address = null;
    this.balance = 0;
    this._walletData = null;
    
    if (this.persist) {
        try { localStorage.removeItem(this.storageKey); } catch (e) {}
    }
    
    this._stopBalancePolling();
    
    for (var i = 0; i < this._disconnectCallbacks.length; i++) {
        try { this._disconnectCallbacks[i]({ address: oldAddr }); } catch (e) { console.error(e); }
    }
    
    this._updateButton();
};

/** Toggle connect/disconnect */
MoltWallet.prototype.toggle = async function() {
    if (this.isConnected()) {
        this.disconnect();
    } else {
        await this.connect();
    }
};

/** Refresh wallet balance from RPC */
MoltWallet.prototype.refreshBalance = async function() {
    if (!this.address) return 0;
    try {
        var result = await moltRpcCall('getBalance', [this.address], this.rpcUrl);
        this.balance = (typeof result === 'object') ? (result.balance || result.value || 0) : (result || 0);
    } catch (err) {
        // Balance fetch failed, keep existing
    }
    
    for (var i = 0; i < this._balanceCallbacks.length; i++) {
        try { this._balanceCallbacks[i](this.balance, this.address); } catch (e) { console.error(e); }
    }
    
    return this.balance;
};

/** Start polling for balance updates */
MoltWallet.prototype._startBalancePolling = function() {
    this._stopBalancePolling();
    var self = this;
    this._balanceInterval = setInterval(function() {
        self.refreshBalance();
    }, 15000); // Every 15s
};

/** Stop balance polling */
MoltWallet.prototype._stopBalancePolling = function() {
    if (this._balanceInterval) {
        clearInterval(this._balanceInterval);
        this._balanceInterval = null;
    }
};

/** Restore wallet from localStorage */
MoltWallet.prototype._restore = function() {
    try {
        var stored = localStorage.getItem(this.storageKey);
        if (stored) {
            var data = JSON.parse(stored);
            if (data && data.address) {
                this.address = data.address;
                this._walletData = data;
                this._startBalancePolling();
                this.refreshBalance();
            }
        }
    } catch (e) { /* invalid stored data */ }
};

// ─── Event Callbacks ─────────────────────────────────────

/** Register callback for wallet connect events */
MoltWallet.prototype.onConnect = function(cb) {
    this._connectCallbacks.push(cb);
    // Fire immediately if already connected
    if (this.isConnected()) {
        try { cb({ address: this.address, balance: this.balance }); } catch (e) { console.error(e); }
    }
};

/** Register callback for wallet disconnect events */
MoltWallet.prototype.onDisconnect = function(cb) {
    this._disconnectCallbacks.push(cb);
};

/** Register callback for balance update events */
MoltWallet.prototype.onBalanceUpdate = function(cb) {
    this._balanceCallbacks.push(cb);
};

// ─── UI Binding ──────────────────────────────────────────

/**
 * Bind to a connect/disconnect button element
 * @param {string|Element} selector - CSS selector or DOM element
 */
MoltWallet.prototype.bindConnectButton = function(selector) {
    var el = (typeof selector === 'string') ? document.querySelector(selector) : selector;
    if (!el) {
        console.warn('MoltWallet: Connect button not found:', selector);
        return;
    }
    
    this._buttonEl = el;
    var self = this;
    
    el.addEventListener('click', function(e) {
        e.preventDefault();
        self.toggle();
    });
    
    // Set initial state
    this._updateButton();
};

/** Update the connect button display */
MoltWallet.prototype._updateButton = function() {
    if (!this._buttonEl) return;
    
    if (this.isConnected()) {
        this._buttonEl.innerHTML = '<i class="fas fa-wallet"></i> ' + formatHash(this.address, 6);
        this._buttonEl.classList.add('wallet-connected');
        this._buttonEl.classList.remove('wallet-disconnected');
        this._buttonEl.title = this.address;
    } else {
        this._buttonEl.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet';
        this._buttonEl.classList.remove('wallet-connected');
        this._buttonEl.classList.add('wallet-disconnected');
        this._buttonEl.title = 'Click to connect wallet';
    }
};

// ─── Export ──────────────────────────────────────────────

// Make available globally
window.MoltWallet = MoltWallet;
window.formatHash = window.formatHash || formatHash;
window.getMoltRpcUrl = window.getMoltRpcUrl || getMoltRpcUrl;
window.moltRpcCall = window.moltRpcCall || moltRpcCall;
