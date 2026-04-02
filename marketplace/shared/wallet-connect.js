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

function getInjectedLichenProvider() {
    if (window.licnwallet && window.licnwallet.isLichenWallet) {
        return window.licnwallet;
    }
    return null;
}

function waitForInjectedLichenProvider(timeoutMs) {
    var existing = getInjectedLichenProvider();
    if (existing) return Promise.resolve(existing);

    timeoutMs = typeof timeoutMs === 'number' ? timeoutMs : 400;

    return new Promise(function (resolve) {
        var settled = false;
        var pollTimer = null;
        var timeoutTimer = null;

        function cleanup() {
            window.removeEventListener('lichenwallet#initialized', onReady);
            if (pollTimer) clearInterval(pollTimer);
            if (timeoutTimer) clearTimeout(timeoutTimer);
        }

        function finish(provider) {
            if (settled) return;
            settled = true;
            cleanup();
            resolve(provider || null);
        }

        function onReady() {
            finish(getInjectedLichenProvider());
        }

        window.addEventListener('lichenwallet#initialized', onReady);
        pollTimer = setInterval(function () {
            var provider = getInjectedLichenProvider();
            if (provider) finish(provider);
        }, 50);
        timeoutTimer = setTimeout(function () {
            finish(null);
        }, timeoutMs);
    });
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
    this._provider = null;
    this._providerListenersBound = false;

    // Try to restore from localStorage
    if (this.persist) {
        this._restore();
    }
}

/** Check if a wallet is currently connected */
LichenWallet.prototype.isConnected = function () {
    return this.address !== null;
};

LichenWallet.prototype._clearConnectionState = function (notifyDisconnect, oldAddr) {
    var previousAddress = oldAddr !== undefined ? oldAddr : this.address;

    this.address = null;
    this.balance = 0;
    this._walletData = null;

    if (this.persist) {
        try { localStorage.removeItem(this.storageKey); } catch (e) { }
    }

    this._stopBalancePolling();

    if (notifyDisconnect && previousAddress) {
        for (var i = 0; i < this._disconnectCallbacks.length; i++) {
            try { this._disconnectCallbacks[i]({ address: previousAddress }); } catch (e) { console.error(e); }
        }
    }

    this._updateButton();
};

LichenWallet.prototype._bindInjectedProvider = function (provider) {
    if (!provider) return;
    this._provider = provider;

    if (this._providerListenersBound || typeof provider.on !== 'function') {
        return;
    }

    this._providerListenersBound = true;
    var self = this;

    provider.on('accountsChanged', function (accounts) {
        var nextAddress = Array.isArray(accounts) && accounts.length ? accounts[0] : null;
        if (!nextAddress) {
            self._clearConnectionState(false);
            return;
        }

        self.address = nextAddress;
        self._walletData = {
            address: nextAddress,
            hasKeys: false,
            provider: 'extension',
            created: (self._walletData && self._walletData.created) || Date.now()
        };

        if (self.persist) {
            try { localStorage.setItem(self.storageKey, JSON.stringify(self._walletData)); } catch (e) { }
        }

        self.refreshBalance();
        self._updateButton();
    });

    provider.on('disconnect', function () {
        self._clearConnectionState(false);
    });
};

LichenWallet.prototype._connectInjectedProvider = async function (provider) {
    this._bindInjectedProvider(provider);

    var accounts = [];
    if (typeof provider.getProviderState === 'function') {
        var state = await provider.getProviderState().catch(function () { return null; });
        if (state && state.connected && Array.isArray(state.accounts)) {
            accounts = state.accounts;
        }
    }

    if (!accounts.length) {
        if (typeof provider.requestAccounts === 'function') {
            accounts = await provider.requestAccounts();
        } else if (typeof provider.connect === 'function') {
            var result = await provider.connect();
            if (Array.isArray(result)) {
                accounts = result;
            } else if (result && Array.isArray(result.accounts)) {
                accounts = result.accounts;
            }
        } else if (typeof provider.accounts === 'function') {
            accounts = await provider.accounts();
        }
    }

    if (!Array.isArray(accounts) || !accounts.length) {
        throw new Error('Lichen wallet extension returned no accounts');
    }

    this.address = accounts[0];
    this._walletData = {
        address: this.address,
        hasKeys: false,
        provider: 'extension',
        created: Date.now()
    };
};

/**
 * Connect wallet - creates or imports a wallet
 * If Lichen SDK is available, uses real keypair generation
 * Otherwise falls back to address-only mode with server-side wallet
 * @param {Object} [importData] - Import data { seed, hex, json }
 * @returns {Promise<Object>} - { address, balance }
 */
LichenWallet.prototype.connect = async function (importData) {
    var injectedProvider = !importData ? await waitForInjectedLichenProvider() : null;

    if (injectedProvider) {
        await this._connectInjectedProvider(injectedProvider);
    } else if (window.Lichen && window.Lichen.Wallet) {
        // Try Lichen SDK next (real local wallet)
        try {
            var wallet;
            if (importData && importData.seed) {
                wallet = await Promise.resolve(Lichen.Wallet.import({ seed: importData.seed }, ''));
            } else if (importData && importData.json) {
                wallet = await Promise.resolve(Lichen.Wallet.import(importData.json, importData.password || ''));
            } else if (typeof Lichen.Wallet.create === 'function') {
                wallet = await Lichen.Wallet.create();
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
            await this._createRpcWallet();
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
LichenWallet.prototype._createRpcWallet = async function () {
    try {
        var result = await lichenRpcCall('createWallet', [], this.rpcUrl);
        this.address = result.address || result.pubkey || result;
        this._walletData = {
            address: this.address,
            hasKeys: false,
            created: Date.now()
        };
    } catch (err) {
        // Fallback - generate a native PQ wallet locally if the shared runtime is available
        if (window.LichenPQ && typeof window.LichenPQ.generateKeypair === 'function') {
            var kp = await window.LichenPQ.generateKeypair();
            this.address = kp.address;
            this._walletData = {
                address: this.address,
                publicKey: kp.publicKeyHex,
                hasKeys: true,
                created: Date.now()
            };
        } else {
            // No crypto library available — prompt user to install extension
            this.address = null;
            var errorMsg = 'Wallet creation failed: no wallet extension or PQ runtime available. ' +
                'Please install the Lichen wallet extension or import a private key directly.';
            console.error(errorMsg);
            throw new Error(errorMsg);
        }
    }
};

/** Disconnect wallet and clear state */
LichenWallet.prototype.disconnect = function () {
    var oldAddr = this.address;
    if (this._provider && this._walletData && this._walletData.provider === 'extension' && typeof this._provider.disconnect === 'function') {
        this._provider.disconnect().catch(function () { });
    }
    this._clearConnectionState(true, oldAddr);
};

/** Toggle connect/disconnect */
LichenWallet.prototype.toggle = async function () {
    if (this.isConnected()) {
        this.disconnect();
    } else {
        await this.connect();
    }
};

/** Refresh wallet balance from RPC */
LichenWallet.prototype.refreshBalance = async function () {
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
LichenWallet.prototype._startBalancePolling = function () {
    this._stopBalancePolling();
    var self = this;
    this._balanceInterval = setInterval(function () {
        self.refreshBalance();
    }, 15000); // Every 15s
};

/** Stop balance polling */
LichenWallet.prototype._stopBalancePolling = function () {
    if (this._balanceInterval) {
        clearInterval(this._balanceInterval);
        this._balanceInterval = null;
    }
};

/** Restore wallet from localStorage */
LichenWallet.prototype._restore = function () {
    var self = this;
    try {
        var stored = localStorage.getItem(this.storageKey);
        if (stored) {
            var data = JSON.parse(stored);
            if (data && data.address) {
                this.address = data.address;
                this._walletData = data;
                this._startBalancePolling();
                this.refreshBalance();

                if (data.provider === 'extension') {
                    waitForInjectedLichenProvider(1000).then(function (provider) {
                        if (!provider) return;
                        self._bindInjectedProvider(provider);
                    });
                }
            }
        }
    } catch (e) { /* invalid stored data */ }
};

// ─── Event Callbacks ─────────────────────────────────────

/** Register callback for wallet connect events */
LichenWallet.prototype.onConnect = function (cb) {
    this._connectCallbacks.push(cb);
    // Fire immediately if already connected
    if (this.isConnected()) {
        try { cb({ address: this.address, balance: this.balance }); } catch (e) { console.error(e); }
    }
};

/** Register callback for wallet disconnect events */
LichenWallet.prototype.onDisconnect = function (cb) {
    this._disconnectCallbacks.push(cb);
};

/** Register callback for balance update events */
LichenWallet.prototype.onBalanceUpdate = function (cb) {
    this._balanceCallbacks.push(cb);
};

// ─── UI Binding ──────────────────────────────────────────

/**
 * Bind to a connect/disconnect button element
 * @param {string|Element} selector - CSS selector or DOM element
 */
LichenWallet.prototype.bindConnectButton = function (selector) {
    var el = (typeof selector === 'string') ? document.querySelector(selector) : selector;
    if (!el) {
        console.warn('LichenWallet: Connect button not found:', selector);
        return;
    }

    this._buttonEl = el;
    var self = this;

    el.addEventListener('click', function (e) {
        e.preventDefault();
        self.toggle();
    });

    // Set initial state
    this._updateButton();
};

/** Update the connect button display */
LichenWallet.prototype._updateButton = function () {
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
