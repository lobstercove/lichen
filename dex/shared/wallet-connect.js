/**
 * Lichen Shared Wallet Connect Utility
 * 
 * Provides a unified extension-backed wallet connection experience across
 * Lichen frontends.
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

function getWalletAppUrl(entry) {
    var overrideUrl = null;
    try {
        overrideUrl = typeof window !== 'undefined' && window.localStorage
            ? window.localStorage.getItem('lichen_app_url_wallet')
            : null;
    } catch (e) { }

    var baseUrl = overrideUrl || ((typeof LICHEN_CONFIG !== 'undefined' && LICHEN_CONFIG.wallet)
        ? LICHEN_CONFIG.wallet
        : 'https://wallet.lichen.network');
    var url = new URL(baseUrl, window.location.origin);
    if (entry) {
        url.searchParams.set('entry', entry);
    }
    return url;
}

function getDexSelectedNetwork() {
    if (typeof LICHEN_CONFIG !== 'undefined' && typeof LICHEN_CONFIG.currentNetwork === 'function') {
        return LICHEN_CONFIG.currentNetwork('dexNetwork');
    }

    try {
        return localStorage.getItem('dexNetwork') || 'testnet';
    } catch (e) {
        return 'testnet';
    }
}

function getWalletPopupUrl(entry) {
    var url = getWalletAppUrl(entry);
    url.searchParams.set('bridge', 'popup');
    url.searchParams.set('source', 'dex');
    url.searchParams.set('network', getDexSelectedNetwork());
    url.searchParams.set('returnTo', window.location.href);
    return url;
}

var WALLET_POPUP_REQUEST_TARGET = 'LICHEN_WEB_WALLET_BRIDGE';
var WALLET_POPUP_RESPONSE_TARGET = 'LICHEN_WEB_WALLET_RESPONSE';
var WALLET_POPUP_EVENT_TARGET = 'LICHEN_WEB_WALLET_EVENT';
var WALLET_POPUP_STATE_KEY = 'lichen_web_wallet_popup_state_v1';
var walletPopupProviderInstance = null;

function popupProviderDisconnectedState(previous) {
    var network = (typeof LICHEN_CONFIG !== 'undefined' && typeof LICHEN_CONFIG.currentNetwork === 'function')
        ? LICHEN_CONFIG.currentNetwork()
        : 'testnet';
    return {
        connected: false,
        origin: window.location.origin,
        chainId: previous && previous.chainId ? previous.chainId : (network === 'mainnet' ? '0x2710' : network === 'testnet' ? '0x2711' : '0x539'),
        network: previous && previous.network ? previous.network : network,
        activeAddress: '',
        accounts: [],
        isLocked: false,
        providerType: 'web-wallet'
    };
}

function normalizePopupProviderState(state, previous) {
    var fallback = popupProviderDisconnectedState(previous);
    if (!state || typeof state !== 'object') {
        return fallback;
    }

    var accounts = Array.isArray(state.accounts)
        ? state.accounts.map(function (address) { return typeof address === 'string' ? address.trim() : ''; }).filter(Boolean)
        : [];

    return {
        connected: Boolean(state.connected),
        origin: typeof state.origin === 'string' ? state.origin : fallback.origin,
        chainId: typeof state.chainId === 'string' ? state.chainId : fallback.chainId,
        network: typeof state.network === 'string' ? state.network : fallback.network,
        activeAddress: accounts[0] || '',
        accounts: accounts,
        isLocked: Boolean(state.isLocked),
        providerType: 'web-wallet'
    };
}

function readStoredPopupProviderState() {
    try {
        var raw = localStorage.getItem(WALLET_POPUP_STATE_KEY);
        if (!raw) return null;
        return JSON.parse(raw);
    } catch (e) {
        return null;
    }
}

function writeStoredPopupProviderState(state) {
    try {
        if (!state || !state.connected || !Array.isArray(state.accounts) || !state.accounts.length) {
            localStorage.removeItem(WALLET_POPUP_STATE_KEY);
            return;
        }

        localStorage.setItem(WALLET_POPUP_STATE_KEY, JSON.stringify({
            connected: true,
            origin: typeof state.origin === 'string' ? state.origin : window.location.origin,
            chainId: typeof state.chainId === 'string' ? state.chainId : '',
            network: typeof state.network === 'string' ? state.network : '',
            accounts: state.accounts,
            isLocked: Boolean(state.isLocked),
            providerType: 'web-wallet'
        }));
    } catch (e) { }
}

function PopupLichenProvider(options) {
    options = options || {};
    this.isLichenWallet = true;
    this.isPopupWallet = true;
    this.popupName = options.popupName || 'lichen-wallet-connect';
    this.popup = null;
    this.walletOrigin = getWalletAppUrl().origin;
    this._requestId = 0;
    this._pending = new Map();
    this._listeners = new Map();
    this._windowMonitor = null;
    this._boundHandleMessage = this._handleMessage.bind(this);
    this._lastState = normalizePopupProviderState(readStoredPopupProviderState(), popupProviderDisconnectedState());

    window.addEventListener('message', this._boundHandleMessage);
}

PopupLichenProvider.prototype._persistState = function () {
    writeStoredPopupProviderState(this._lastState);
};

PopupLichenProvider.prototype._emit = function (event, payload) {
    var listeners = this._listeners.get(event);
    if (!listeners) return;
    listeners.forEach(function (handler) {
        try { handler(payload); } catch (e) { }
    });
};

PopupLichenProvider.prototype.on = function (event, handler) {
    if (!event || typeof handler !== 'function') return;
    var listeners = this._listeners.get(event);
    if (!listeners) {
        listeners = new Set();
        this._listeners.set(event, listeners);
    }
    listeners.add(handler);
};

PopupLichenProvider.prototype.removeListener = function (event, handler) {
    var listeners = this._listeners.get(event);
    if (!listeners) return;
    listeners.delete(handler);
    if (!listeners.size) {
        this._listeners.delete(event);
    }
};

PopupLichenProvider.prototype._clearPending = function (id) {
    var pending = this._pending.get(id);
    if (!pending) return null;
    if (pending.retryTimer) clearInterval(pending.retryTimer);
    if (pending.timeoutTimer) clearTimeout(pending.timeoutTimer);
    this._pending.delete(id);
    return pending;
};

PopupLichenProvider.prototype._setDisconnected = function () {
    var previous = this._lastState;
    this._lastState = popupProviderDisconnectedState(previous);
    this._persistState();
    if (previous && previous.connected) {
        this._emit('accountsChanged', []);
        this._emit('disconnect', { origin: previous.origin || window.location.origin });
    }
};

PopupLichenProvider.prototype._handlePopupClosed = function () {
    var self = this;
    this.popup = null;
    this._pending.forEach(function (_value, id) {
        var pending = self._clearPending(id);
        if (pending) {
            pending.reject(new Error('Web wallet window closed before the request completed'));
        }
    });
};

PopupLichenProvider.prototype._startWindowMonitor = function () {
    var self = this;
    if (this._windowMonitor) return;
    this._windowMonitor = setInterval(function () {
        if (!self.popup || !self.popup.closed) return;
        clearInterval(self._windowMonitor);
        self._windowMonitor = null;
        self._handlePopupClosed();
    }, 300);
};

PopupLichenProvider.prototype.isWindowOpen = function () {
    return Boolean(this.popup && !this.popup.closed);
};

PopupLichenProvider.prototype.focus = function (entry) {
    this._openWindow(entry || 'web-wallet');
};

PopupLichenProvider.prototype._openWindow = function (entry) {
    var popupUrl = getWalletPopupUrl(entry || 'web-wallet').toString();
    if (!this.popup || this.popup.closed) {
        this.popup = window.open(popupUrl, this.popupName, 'popup=yes,width=480,height=760,resizable=yes,scrollbars=yes');
    }

    if (!this.popup) {
        throw new Error('Popup blocked. Allow popups for this site to use the Lichen web wallet.');
    }

    try { this.popup.focus(); } catch (e) { }
    this._startWindowMonitor();
    return this.popup;
};

PopupLichenProvider.prototype._updateStateFromMethod = function (method, result) {
    if (method === 'licn_getProviderState') {
        this._lastState = normalizePopupProviderState(result, this._lastState);
        this._persistState();
        return;
    }

    if (method === 'licn_requestAccounts') {
        var accounts = Array.isArray(result) ? result.map(function (address) { return String(address || '').trim(); }).filter(Boolean) : [];
        this._lastState = normalizePopupProviderState({
            connected: accounts.length > 0,
            accounts: accounts,
            activeAddress: accounts[0] || '',
            chainId: this._lastState.chainId,
            network: this._lastState.network,
            isLocked: false
        }, this._lastState);
        this._persistState();
        return;
    }

    if (method === 'licn_disconnect') {
        this._setDisconnected();
    }
};

PopupLichenProvider.prototype._handleMessage = function (event) {
    if (event.origin !== this.walletOrigin || !event.data) {
        return;
    }

    if (event.data.target === WALLET_POPUP_EVENT_TARGET) {
        if (event.data.event === 'connect') {
            this._lastState = normalizePopupProviderState(event.data.payload, this._lastState);
        } else if (event.data.event === 'disconnect') {
            this._setDisconnected();
            return;
        } else if (event.data.event === 'accountsChanged') {
            var accounts = Array.isArray(event.data.payload) ? event.data.payload : [];
            this._lastState = normalizePopupProviderState({
                connected: this._lastState.connected,
                chainId: this._lastState.chainId,
                network: this._lastState.network,
                accounts: accounts,
                isLocked: accounts.length === 0 ? true : false
            }, this._lastState);
        } else if (event.data.event === 'chainChanged') {
            this._lastState = normalizePopupProviderState({
                connected: this._lastState.connected,
                accounts: this._lastState.accounts,
                network: this._lastState.network,
                chainId: event.data.payload,
                isLocked: this._lastState.isLocked
            }, this._lastState);
        }

        this._persistState();

        this._emit(event.data.event, event.data.payload);
        return;
    }

    if (event.data.target !== WALLET_POPUP_RESPONSE_TARGET) {
        return;
    }

    var pending = this._clearPending(event.data.id);
    if (!pending) {
        return;
    }

    var response = event.data.response;
    if (response && response.ok) {
        this._updateStateFromMethod(pending.method, response.result);
        pending.resolve(response.result);
        return;
    }

    pending.reject(new Error(response && response.error ? response.error : 'Web wallet request failed'));
};

PopupLichenProvider.prototype._request = function (payload, options) {
    var self = this;
    options = options || {};
    var requestId = 'wallet-popup-' + (++this._requestId) + '-' + Date.now();
    var requestPayload = {
        target: WALLET_POPUP_REQUEST_TARGET,
        id: requestId,
        payload: payload,
    };

    this._openWindow(options.entry || 'web-wallet');

    return new Promise(function (resolve, reject) {
        var pending = {
            method: String(payload && payload.method || '').trim(),
            resolve: resolve,
            reject: reject,
            retryTimer: null,
            timeoutTimer: null,
        };

        function postRequest() {
            if (!self.popup || self.popup.closed) {
                return;
            }
            try {
                self.popup.postMessage(requestPayload, self.walletOrigin);
            } catch (e) { }
        }

        pending.retryTimer = setInterval(postRequest, 350);
        pending.timeoutTimer = setTimeout(function () {
            var expired = self._clearPending(requestId);
            if (expired) {
                expired.reject(new Error('Web wallet request timed out'));
            }
        }, options.timeoutMs || 600000);

        self._pending.set(requestId, pending);
        postRequest();
    });
};

PopupLichenProvider.prototype.request = function (payload) {
    return this._request(payload, { entry: 'web-wallet' });
};

PopupLichenProvider.prototype.getProviderState = function () {
    if (!this.isWindowOpen()) {
        return Promise.resolve(this._lastState);
    }
    return this._request({ method: 'licn_getProviderState' }, { entry: 'web-wallet', timeoutMs: 30000 })
        .catch(function () {
            return this._lastState;
        }.bind(this));
};

PopupLichenProvider.prototype.isConnected = function () {
    return this.getProviderState().then(function (state) {
        return Boolean(state && state.connected && Array.isArray(state.accounts) && state.accounts.length);
    });
};

PopupLichenProvider.prototype.accounts = function () {
    return this.getProviderState().then(function (state) {
        return Array.isArray(state && state.accounts) ? state.accounts : [];
    });
};

PopupLichenProvider.prototype.requestAccounts = function () {
    return this._request({ method: 'licn_requestAccounts' }, { entry: 'web-wallet' });
};

PopupLichenProvider.prototype.connect = function () {
    return this.requestAccounts();
};

PopupLichenProvider.prototype.disconnect = function () {
    var self = this;
    if (!this.isWindowOpen()) {
        this._setDisconnected();
        return Promise.resolve(true);
    }
    return this._request({ method: 'licn_disconnect' }, { entry: 'web-wallet', timeoutMs: 30000 })
        .then(function (result) {
            self._setDisconnected();
            return result;
        });
};

PopupLichenProvider.prototype.getPermissions = function () {
    if (!this.isWindowOpen()) {
        return Promise.resolve(this._lastState.connected && Array.isArray(this._lastState.accounts) && this._lastState.accounts.length
            ? [{
                parentCapability: 'eth_accounts',
                caveats: [{ type: 'filterResponse', value: this._lastState.accounts }],
                date: Date.now(),
                invoker: this._lastState.origin || window.location.origin,
            }]
            : []);
    }
    return this._request({ method: 'wallet_getPermissions' }, { entry: 'web-wallet', timeoutMs: 30000 })
        .catch(function () { return []; });
};

PopupLichenProvider.prototype.sendTransaction = function (transaction) {
    return this._request({ method: 'licn_sendTransaction', params: [{ transaction: transaction }] }, { entry: 'sign' });
};

function getPopupLichenProvider() {
    if (!walletPopupProviderInstance) {
        walletPopupProviderInstance = new PopupLichenProvider();
    }
    return walletPopupProviderInstance;
}

function extensionOnlyWalletError() {
    return new Error('Browser-local wallets are disabled. Use the Lichen wallet extension or the Lichen web wallet.');
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

LichenWallet.prototype._bindProvider = function (provider) {
    if (!provider) return;
    this._provider = provider;

    if (this._providerListenersBound || typeof provider.on !== 'function') {
        return;
    }

    this._providerListenersBound = true;
    var self = this;
    var providerType = provider.isPopupWallet ? 'web-wallet' : 'extension';

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
            provider: providerType,
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

LichenWallet.prototype._connectProvider = async function (provider) {
    this._bindProvider(provider);

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
        provider: provider.isPopupWallet ? 'web-wallet' : 'extension',
        created: Date.now()
    };
};

/**
 * Connect wallet via the injected Lichen extension or the popup-backed web wallet.
 * @returns {Promise<Object>} - { address, balance }
 */
LichenWallet.prototype.connect = async function (options) {
    if (options && typeof options !== 'object') {
        throw extensionOnlyWalletError();
    }

    var requestedProvider = options && typeof options.provider === 'string'
        ? options.provider.trim()
        : '';
    var provider = null;

    if (requestedProvider !== 'web-wallet') {
        provider = await waitForInjectedLichenProvider(requestedProvider === 'extension' ? 600 : 250);
    }

    if (!provider && requestedProvider !== 'extension') {
        provider = getPopupLichenProvider();
    }

    if (!provider) {
        throw new Error('No wallet provider available. Install the extension or connect through the web wallet.');
    }

    await this._connectProvider(provider);

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

    info.provider = this._walletData ? this._walletData.provider : 'extension';
    return info;
};

/** Browser-local and RPC wallet creation are disabled. */
LichenWallet.prototype._createRpcWallet = async function () {
    throw extensionOnlyWalletError();
};

/** Disconnect wallet and clear state */
LichenWallet.prototype.disconnect = function () {
    var oldAddr = this.address;
    if (this._provider && this._walletData && typeof this._provider.disconnect === 'function') {
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
                if (data.provider !== 'extension' && data.provider !== 'web-wallet') {
                    localStorage.removeItem(this.storageKey);
                    return;
                }
                this.address = data.address;
                this._walletData = data;
                this._startBalancePolling();
                this.refreshBalance();

                if (data.provider === 'extension') {
                    waitForInjectedLichenProvider(1000).then(function (provider) {
                        if (!provider) return;
                        self._bindProvider(provider);
                    });
                } else if (data.provider === 'web-wallet') {
                    self._bindProvider(getPopupLichenProvider());
                }
            }
        }
    } catch (e) { /* invalid stored data */ }
};

LichenWallet.prototype.getProvider = function () {
    return this._provider;
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
            console.error('Monitoring wallet action failed:', err);
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
window.getInjectedLichenProvider = window.getInjectedLichenProvider || getInjectedLichenProvider;
window.waitForInjectedLichenProvider = window.waitForInjectedLichenProvider || waitForInjectedLichenProvider;
window.getPopupLichenProvider = window.getPopupLichenProvider || getPopupLichenProvider;
window.formatHash = window.formatHash || formatHash;
window.getLichenRpcUrl = window.getLichenRpcUrl || getLichenRpcUrl;
window.lichenRpcCall = window.lichenRpcCall || lichenRpcCall;
