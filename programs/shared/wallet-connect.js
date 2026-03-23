/**
 * Lichen Shared Wallet Connect Utility
 * 
 * Provides a unified wallet connection experience across all Lichen frontends.
 * Uses the Lichen SDK for real keypair generation and signing.
 * 
 * Usage:
 *   <script src="../shared/wallet-connect.js"></script>
 *   <script>
 *     const wallet = new LichenWallet({ rpcUrl: 'http://localhost:8899' });
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
 * Checks window.lichenConfig, window.lichenMarketConfig, window.lichenExplorerConfig
 * @returns {string}
 */
function getLichenRpcUrl() {
    if (typeof LICHEN_CONFIG !== 'undefined' && typeof LICHEN_CONFIG.rpc === 'function') return LICHEN_CONFIG.rpc();
    if (window.lichenConfig && window.lichenConfig.rpcUrl) return window.lichenConfig.rpcUrl;
    if (window.lichenMarketConfig && window.lichenMarketConfig.rpcUrl) return window.lichenMarketConfig.rpcUrl;
    if (window.lichenExplorerConfig && window.lichenExplorerConfig.rpcUrl) return window.lichenExplorerConfig.rpcUrl;
    return 'http://localhost:8899';
}

/**
 * Make a JSON-RPC call to the Lichen node
 * @param {string} method - RPC method name
 * @param {Array|Object} params - Method params
 * @param {string} [rpcUrl] - Override RPC URL
 * @returns {Promise<any>}
 */
async function lichenRpcCall(method, params, rpcUrl) {
    var url = rpcUrl || getLichenRpcUrl();
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
 * LichenWallet - Unified wallet connection manager
 * 
 * @param {Object} options
 * @param {string} [options.rpcUrl] - RPC endpoint URL
 * @param {string} [options.storageKey='lichen_wallet'] - localStorage key
 * @param {boolean} [options.persist=true] - Auto-save to localStorage
 */
function LichenWallet(options) {
    options = options || {};
    this.rpcUrl = options.rpcUrl || getLichenRpcUrl();
    this.storageKey = options.storageKey || 'lichen_wallet';
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
LichenWallet.prototype.isConnected = function() {
    return this.address !== null;
};

/**
 * Connect wallet - creates or imports a wallet
 * If Lichen SDK is available, uses real keypair generation
 * Otherwise falls back to address-only mode with server-side wallet
 * @param {Object} [importData] - Import data { seed, hex, json }
 * @returns {Promise<Object>} - { address, balance }
 */
LichenWallet.prototype.connect = async function(importData) {
    // Try Lichen SDK first (real wallet)
    if (window.Lichen && window.Lichen.Wallet) {
        try {
            var wallet;
            if (importData && importData.seed) {
                wallet = Lichen.Wallet.import({ seed: importData.seed }, '');
            } else if (importData && importData.json) {
                wallet = Lichen.Wallet.import(importData.json, importData.password || '');
            } else {
                wallet = new Lichen.Wallet();
            }
            this.address = wallet.address || wallet.publicKey;
            this._walletData = {
                address: this.address,
                hasKeys: true,
                created: Date.now()
            };
        } catch (err) {
            console.warn('Lichen SDK wallet creation failed, using RPC wallet:', err);
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
LichenWallet.prototype._createRpcWallet = async function() {
    try {
        var result = await lichenRpcCall('createWallet', [], this.rpcUrl);
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
            this._walletData = {
                address: this.address,
                hasKeys: true,
                created: Date.now()
            };
        } else {
            // No crypto library available — prompt user to install extension
            this.address = null;
            var errorMsg = 'Wallet creation failed: no wallet extension or cryptographic library available. ' +
                'Please install the Lichen wallet extension or import a private key directly.';
            console.error(errorMsg);
            throw new Error(errorMsg);
        }
    }
};

/** Disconnect wallet and clear state */
LichenWallet.prototype.disconnect = function() {
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
LichenWallet.prototype.toggle = async function() {
    if (this.isConnected()) {
        this.disconnect();
    } else {
        await this.connect();
    }
};

/** Refresh wallet balance from RPC */
LichenWallet.prototype.refreshBalance = async function() {
    if (!this.address) return 0;
    try {
        var result = await lichenRpcCall('getBalance', [this.address], this.rpcUrl);
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
LichenWallet.prototype._startBalancePolling = function() {
    this._stopBalancePolling();
    var self = this;
    this._balanceInterval = setInterval(function() {
        self.refreshBalance();
    }, 15000); // Every 15s
};

/** Stop balance polling */
LichenWallet.prototype._stopBalancePolling = function() {
    if (this._balanceInterval) {
        clearInterval(this._balanceInterval);
        this._balanceInterval = null;
    }
};

/** Restore wallet from localStorage */
LichenWallet.prototype._restore = function() {
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
LichenWallet.prototype.onConnect = function(cb) {
    this._connectCallbacks.push(cb);
    // Fire immediately if already connected
    if (this.isConnected()) {
        try { cb({ address: this.address, balance: this.balance }); } catch (e) { console.error(e); }
    }
};

/** Register callback for wallet disconnect events */
LichenWallet.prototype.onDisconnect = function(cb) {
    this._disconnectCallbacks.push(cb);
};

/** Register callback for balance update events */
LichenWallet.prototype.onBalanceUpdate = function(cb) {
    this._balanceCallbacks.push(cb);
};

// ─── UI Binding ──────────────────────────────────────────

/**
 * Bind to a connect/disconnect button element
 * @param {string|Element} selector - CSS selector or DOM element
 */
LichenWallet.prototype.bindConnectButton = function(selector) {
    var el = (typeof selector === 'string') ? document.querySelector(selector) : selector;
    if (!el) {
        console.warn('LichenWallet: Connect button not found:', selector);
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
LichenWallet.prototype._updateButton = function() {
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
window.LichenWallet = LichenWallet;
window.formatHash = window.formatHash || formatHash;
window.getLichenRpcUrl = window.getLichenRpcUrl || getLichenRpcUrl;
window.lichenRpcCall = window.lichenRpcCall || lichenRpcCall;
