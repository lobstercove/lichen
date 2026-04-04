/**
 * Lichen Shared Wallet Connect Utility
 * 
 * Provides a unified extension-backed wallet connection flow for Marketplace.
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

function normalizeRecentBlockhash(result) {
    var blockhash = typeof result === 'string' ? result : result && result.blockhash;
    if (!blockhash || typeof blockhash !== 'string' || !/^[0-9a-fA-F]{64}$/.test(blockhash)) {
        throw new Error('Recent blockhash unavailable');
    }
    return blockhash;
}

function normalizeRpcInstruction(ix, signerAddress) {
    var programId = ix.program_id || ix.programId;
    if (!programId) {
        throw new Error('Instruction missing program_id');
    }

    var accounts = Array.isArray(ix.accounts) && ix.accounts.length ? ix.accounts : [signerAddress];
    var rawData = ix.data;
    var dataBytes;
    if (rawData instanceof Uint8Array) {
        dataBytes = rawData;
    } else if (Array.isArray(rawData)) {
        dataBytes = new Uint8Array(rawData);
    } else if (typeof rawData === 'string') {
        dataBytes = new TextEncoder().encode(rawData);
    } else {
        dataBytes = new Uint8Array(0);
    }

    return {
        program_id: new Uint8Array(bs58decode(programId)),
        accounts: accounts.map(function (account) { return new Uint8Array(bs58decode(account)); }),
        data: dataBytes,
    };
}

function encodeTransactionPayload(transaction) {
    return btoa(String.fromCharCode.apply(null, new TextEncoder().encode(JSON.stringify(transaction))));
}

function unwrapTransactionResult(result) {
    return result && typeof result === 'object' && result.txHash ? result.txHash : result;
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

function extensionOnlyWalletError() {
    return new Error('Browser-local wallets are disabled in Marketplace. Use the Lichen wallet extension.');
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
    this._provider = null;

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
 * Connect wallet via the injected Lichen extension only.
 * @returns {Promise<Object>} - { address, balance }
 */
LichenWallet.prototype.connect = async function (importData) {
    if (importData) {
        throw extensionOnlyWalletError();
    }

    var injectedProvider = await waitForInjectedLichenProvider();
    if (!injectedProvider) {
        throw new Error('Lichen wallet extension not found. Install the extension to continue.');
    }

    await this._connectInjectedProvider(injectedProvider);

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

/** Browser-local and RPC wallet creation are disabled. */
LichenWallet.prototype._createRpcWallet = async function () {
    throw extensionOnlyWalletError();
};

/** Disconnect wallet and clear state */
LichenWallet.prototype.disconnect = function () {
    var oldAddr = this.address;
    if (this._provider && this._walletData && this._walletData.provider === 'extension' && typeof this._provider.disconnect === 'function') {
        this._provider.disconnect().catch(function () { });
    }
    this._clearConnectionState(true, oldAddr);
};

LichenWallet.prototype._resolveInjectedProvider = async function () {
    var provider = this._provider || getInjectedLichenProvider();
    if (!provider) {
        provider = await waitForInjectedLichenProvider(800);
    }
    if (!provider) return null;

    this._bindInjectedProvider(provider);

    if (typeof provider.getProviderState === 'function') {
        var state = await provider.getProviderState().catch(function () { return null; });
        if (state && state.connected === false) {
            return null;
        }
        if (state && Array.isArray(state.accounts) && state.accounts.length && this.address && state.accounts.indexOf(this.address) === -1) {
            return null;
        }
    }

    return provider;
};

LichenWallet.prototype.sendTransaction = async function (instructions) {
    if (!this.address) {
        throw new Error('Connect a wallet before sending transactions');
    }

    if (!Array.isArray(instructions) || !instructions.length) {
        throw new Error('At least one instruction is required');
    }

    var normalizedInstructions = instructions.map(function (ix) {
        return normalizeRpcInstruction(ix, this.address);
    }, this);
    var blockhash = normalizeRecentBlockhash(await lichenRpcCall('getRecentBlockhash', [], this.rpcUrl));

    var provider = await this._resolveInjectedProvider();
    if (!provider || typeof provider.sendTransaction !== 'function') {
        throw new Error('Lichen wallet extension not available for transaction approval');
    }

    return unwrapTransactionResult(await provider.sendTransaction({
        signatures: [],
        message: {
            instructions: normalizedInstructions.map(function (ix) {
                return {
                    program_id: Array.from(ix.program_id),
                    accounts: ix.accounts.map(function (account) { return Array.from(account); }),
                    data: Array.from(ix.data),
                };
            }),
            blockhash: blockhash,
        },
    }));
};

LichenWallet.prototype._openWalletModal = function () {
    if (this.isConnected()) {
        return Promise.resolve({ address: this.address, balance: this.balance });
    }

    return this.connect().catch(function (err) {
        console.error('Marketplace wallet connect failed:', err);
        return null;
    });
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
                if (data.provider !== 'extension') {
                    this._clearConnectionState(false);
                    return;
                }

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
        self.toggle().catch(function (err) {
            console.error('Marketplace wallet action failed:', err);
        });
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
