// LichenWallet - Core Wallet Logic
// Full RPC integration, wallet management, and UI controls

// Wallet State — declared early so all helpers can reference it safely
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

// ── Number formatting helpers ──
function fmtToken(value, maxDecimals) {
    const d = maxDecimals !== undefined ? maxDecimals : (walletState?.settings?.decimals || 9);
    return Number(value).toLocaleString(undefined, { maximumFractionDigits: d });
}
function fmtUsd(value, sym = '$') {
    return sym + Number(value).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 6 });
}

// Live token prices — fetched from DEX oracle via RPC, with offline fallbacks.
// Fallback values used ONLY when RPC is unreachable (never displayed as "live").
const _OFFLINE_FALLBACK_PRICES = { LICN: 0.10, lUSD: 1.0, wSOL: 150.0, wETH: 3000.0, wBNB: 600.0 };
const livePrices = { LICN: 0, lUSD: 1.0, wSOL: 0, wETH: 0, wBNB: 0 };
let _pricesLoaded = false;

async function fetchLivePrices() {
    try {
        const result = await rpc.call('getDexPairs', []);
        if (result && Array.isArray(result)) {
            for (const pair of result) {
                const base = (pair.base || '').toUpperCase();
                if (pair.price && livePrices.hasOwnProperty(base)) {
                    livePrices[base] = parseFloat(pair.price) || 0;
                }
            }
            // LICN price: look for LICN/lUSD pair
            const licnPair = result.find(p =>
                (p.base || '').toUpperCase() === 'LICN' && (p.quote || '').toUpperCase() === 'LUSD'
            );
            if (licnPair && licnPair.price) livePrices.LICN = parseFloat(licnPair.price) || 0;
            _pricesLoaded = true;
        }
    } catch {
        // RPC unavailable — try oracle endpoint as backup
        try {
            const oracleResult = await rpc.call('getOraclePrices', []);
            if (oracleResult && typeof oracleResult === 'object') {
                for (const [sym, price] of Object.entries(oracleResult)) {
                    const key = sym.toUpperCase();
                    if (livePrices.hasOwnProperty(key)) {
                        livePrices[key] = parseFloat(price) || 0;
                    }
                }
                _pricesLoaded = true;
            }
        } catch {
            // Both sources unavailable — use offline fallbacks
            if (!_pricesLoaded) {
                Object.assign(livePrices, _OFFLINE_FALLBACK_PRICES);
            }
        }
    }
}

// Refresh prices every 30 seconds
setInterval(fetchLivePrices, 30000);

function getPrice(symbol) {
    return livePrices[symbol] || _OFFLINE_FALLBACK_PRICES[symbol] || 0;
}

// Network configuration — centralized in shared-config.js (LICHEN_CONFIG)
const NETWORKS = LICHEN_CONFIG.networks;
const WS_ENDPOINTS = {};
for (const [k, v] of Object.entries(NETWORKS)) { WS_ENDPOINTS[k] = v.ws; }

function getSelectedNetwork() {
    return LICHEN_CONFIG.currentNetwork('lichen_wallet_network');
}

function getNetworkLabel(network = getSelectedNetwork()) {
    return LICHEN_CONFIG.networks?.[network]?.label || network;
}

function isLocalNetwork(network = getSelectedNetwork()) {
    return Boolean(LICHEN_CONFIG.networks?.[network]?.local);
}

function getConfiguredRpcOverride(network = getSelectedNetwork()) {
    const settings = walletState.settings || {};
    if (network === 'mainnet') return String(settings.mainnetRPC || '').trim();
    if (network === 'testnet') return String(settings.testnetRPC || '').trim();
    return '';
}

function getTrustedRpcEndpoint(network = getSelectedNetwork()) {
    return LICHEN_CONFIG.rpc(network);
}

function getRpcEndpoint() {
    const net = getSelectedNetwork();
    const override = getConfiguredRpcOverride(net);
    return override || getTrustedRpcEndpoint(net);
}

function normalizeRpcOverride(value, network) {
    const raw = String(value || '').trim();
    if (!raw) return '';

    let parsed;
    try {
        parsed = new URL(raw);
    } catch (_) {
        throw new Error(`${getNetworkLabel(network)} RPC must be a valid http(s) URL`);
    }

    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
        throw new Error(`${getNetworkLabel(network)} RPC must use http:// or https://`);
    }

    const normalized = parsed.toString().replace(/\/+$/, '');
    const trusted = getTrustedRpcEndpoint(network).replace(/\/+$/, '');
    return normalized === trusted ? '' : normalized;
}

function getWsEndpoint() {
    return LICHEN_CONFIG.ws(getSelectedNetwork());
}

// ===== LIVE BALANCE WEBSOCKET =====
let balanceWs = null;
let balanceWsSubId = null;
let bridgeLockSubId = null;
let bridgeMintSubId = null;
let balanceWsReconnectTimer = null;
let balanceWsSubscribedAddress = null;
let bridgeWsActive = false;
let _wsReconnectDelay = 1000;  // exponential backoff: 1s → 2s → 4s → … → 30s
let _wsKeepaliveTimer = null;
let _wsManualClose = false;

function connectBalanceWebSocket() {
    const wallet = getActiveWallet();
    if (!wallet) return;
    _wsManualClose = false;

    // Don't reconnect if already connected or connecting for this address
    if (balanceWs && balanceWsSubscribedAddress === wallet.address) {
        if (balanceWs.readyState === WebSocket.OPEN || balanceWs.readyState === WebSocket.CONNECTING) {
            return;
        }
    }

    // Close existing connection without entering manual-stop mode
    disconnectBalanceWebSocket(false);

    const wsUrl = getWsEndpoint();

    try {
        balanceWs = new WebSocket(wsUrl);
        balanceWsSubscribedAddress = wallet.address;  // Mark intent immediately
    } catch (e) {
        console.warn('[WS] Failed to create WebSocket:', e);
        balanceWsSubscribedAddress = null;
        scheduleWsReconnect();
        return;
    }

    balanceWs.onopen = () => {
        _wsReconnectDelay = 1000;  // Reset backoff on successful connect
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

        // Client-side keepalive: send a lightweight ping every 25s
        // (server sends Ping frames at 15s; this ensures bidirectional liveness)
        if (_wsKeepaliveTimer) clearInterval(_wsKeepaliveTimer);
        _wsKeepaliveTimer = setInterval(() => {
            if (balanceWs && balanceWs.readyState === WebSocket.OPEN) {
                balanceWs.send(JSON.stringify({ method: 'ping' }));
            }
        }, 25000);
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
                    // Refresh staking tab if visible — catches MossStake deposit/unstake
                    refreshStakingIfVisible();
                    return;
                }

                // Bridge lock event — deposit detected on source chain
                if (subId === bridgeLockSubId && result) {
                    handleBridgeLockEvent(result);
                    return;
                }

                // Bridge mint event — wrapped tokens minted on Lichen
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
        if (_wsKeepaliveTimer) { clearInterval(_wsKeepaliveTimer); _wsKeepaliveTimer = null; }
        balanceWsSubId = null;
        bridgeLockSubId = null;
        bridgeMintSubId = null;
        bridgeWsActive = false;
        balanceWsSubscribedAddress = null;
        balanceWs = null;
        if (_wsManualClose) return;
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

    // Update deposit status UI if visible
    const statusEl = document.getElementById('depositStatus');
    if (statusEl) {
        statusEl.innerHTML = `<i class="fas fa-check-double" style="color: #06D6A0;"></i> <span>Credited to your Lichen wallet!</span>`;
    }

    // Stop polling — we got the final status via WS
    stopDepositPolling();

    const amount = data.amount ? ` (${data.amount} ${(data.asset || '').toUpperCase()})` : '';
    showToast(`Bridge deposit credited${amount}!`, 'success');

    // Refresh balance to show new tokens
    refreshBalance();
    loadAssets();
}

function disconnectBalanceWebSocket(manual = true) {
    _wsManualClose = !!manual;
    if (_wsKeepaliveTimer) { clearInterval(_wsKeepaliveTimer); _wsKeepaliveTimer = null; }
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
    if (_wsManualClose) return;
    if (typeof navigator !== 'undefined' && !navigator.onLine) return;
    if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
    const delay = _wsReconnectDelay;
    _wsReconnectDelay = Math.min(_wsReconnectDelay * 2, 30000);  // exponential backoff: max 30s
    balanceWsReconnectTimer = setTimeout(() => {
        balanceWsReconnectTimer = null;
        const dashboard = document.getElementById('walletDashboard');
        if (dashboard && dashboard.style.display !== 'none') {
            connectBalanceWebSocket();
        }
    }, delay);
}

if (typeof window !== 'undefined') {
    window.addEventListener('online', () => {
        if (!_wsManualClose) connectBalanceWebSocket();
    });
}

if (typeof document !== 'undefined') {
    document.addEventListener('visibilitychange', () => {
        if (document.visibilityState === 'visible' && !_wsManualClose) {
            connectBalanceWebSocket();
        }
    });
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

// RPC Client (same as explorer)
class LichenRPC {
    constructor(url) {
        this.url = url;
    }

    async call(method, params = []) {
        try {
            const response = await fetch(this.url, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
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

    // WL-07: Poll for transaction confirmation after submission.
    // Avoids fire-and-forget pattern — callers can await confirmation.
    async confirmTransaction(signature, timeoutMs = 30000) {
        const start = Date.now();
        while (Date.now() - start < timeoutMs) {
            try {
                const statuses = await this.call('getSignatureStatuses', [[signature]]);
                const status = statuses?.value?.[0];
                if (status && status.confirmation_status === 'confirmed') {
                    return { confirmed: true, status };
                }
                if (status && status.err) {
                    return { confirmed: false, error: status.err };
                }
            } catch { /* retry */ }
            await new Promise(r => setTimeout(r, 800));
        }
        return { confirmed: false, error: 'Timeout waiting for confirmation' };
    }

    // WL-07: Send + confirm in one call
    async sendAndConfirmTransaction(txData, timeoutMs = 30000) {
        const sig = await this.sendTransaction(txData);
        const result = await this.confirmTransaction(sig, timeoutMs);
        return { signature: sig, ...result };
    }
}

const rpc = new LichenRPC(getRpcEndpoint());
const trustedRpc = new LichenRPC(getTrustedRpcEndpoint());

function getTrustedRpcClient() {
    trustedRpc.url = getTrustedRpcEndpoint();
    return trustedRpc;
}

async function trustedRpcCall(method, params = []) {
    if (typeof signedMetadataRpcCall === 'function') {
        return signedMetadataRpcCall(method, params, getSelectedNetwork(), function (resolvedMethod, resolvedParams) {
            return getTrustedRpcClient().call(resolvedMethod, resolvedParams);
        });
    }
    return getTrustedRpcClient().call(method, params);
}

let _networkBaseFeeLicn = typeof BASE_FEE_LICN === 'number' ? BASE_FEE_LICN : 0.001;

function getNetworkBaseFeeLicn() {
    return (Number.isFinite(_networkBaseFeeLicn) && _networkBaseFeeLicn > 0)
        ? _networkBaseFeeLicn
        : (typeof BASE_FEE_LICN === 'number' ? BASE_FEE_LICN : 0.001);
}

function updateSendFeeEstimateUI() {
    const feeEl = document.getElementById('sendNetworkFeeDisplay');
    if (!feeEl) return;
    feeEl.textContent = `${fmtToken(getNetworkBaseFeeLicn())} LICN`;
}

async function refreshDynamicFeeConfig() {
    try {
        const feeConfig = await rpc.call('getFeeConfig', []);
        const baseFeeSpores = Number(
            feeConfig?.base_fee_spores
            ?? feeConfig?.baseFeeSpores
            ?? feeConfig?.base_fee
            ?? 0
        );
        if (Number.isFinite(baseFeeSpores) && baseFeeSpores > 0) {
            _networkBaseFeeLicn = baseFeeSpores / SPORES_PER_LICN;
        }
    } catch (_) {
        _networkBaseFeeLicn = typeof BASE_FEE_LICN === 'number' ? BASE_FEE_LICN : 0.001;
    }
    updateSendFeeEstimateUI();
}

// ── Wrapped Token Registry ──
// Token contract addresses — loaded from deploy manifest or configured manually
const DEFAULT_TOKEN_REGISTRY = {
    lUSD: { symbol: 'lUSD', name: 'Licn USD', decimals: 9, icon: 'fas fa-dollar-sign', address: null, color: '#4ade80', logoUrl: 'https://lichen.network/assets/img/coins/128x128/lusd.png' },
    wSOL: { symbol: 'wSOL', name: 'Wrapped SOL', decimals: 9, icon: 'fab fa-solana', address: null, color: '#9945FF', logoUrl: 'https://s2.coinmarketcap.com/static/img/coins/128x128/5426.png' },
    wETH: { symbol: 'wETH', name: 'Wrapped ETH', decimals: 9, icon: 'fab fa-ethereum', address: null, color: '#627EEA', logoUrl: 'https://s2.coinmarketcap.com/static/img/coins/128x128/1027.png' },
    wBNB: { symbol: 'wBNB', name: 'Wrapped BNB', decimals: 9, icon: 'fas fa-coins', address: null, color: '#F0B90B', logoUrl: 'https://s2.coinmarketcap.com/static/img/coins/128x128/1839.png' },
};

const TOKEN_REGISTRY = {};

function resetTokenRegistry() {
    for (const symbol of Object.keys(TOKEN_REGISTRY)) {
        delete TOKEN_REGISTRY[symbol];
    }
    for (const [symbol, token] of Object.entries(DEFAULT_TOKEN_REGISTRY)) {
        TOKEN_REGISTRY[symbol] = { ...token };
    }
}

resetTokenRegistry();

const LICN_LOGO_URL = 'https://lichen.network/assets/img/coins/128x128/licn.png';

// Load signed registry metadata to get token contract addresses
async function loadTokenRegistry() {
    resetTokenRegistry();

    try {
        const registryResult = await trustedRpcCall('getAllSymbolRegistry', [{ limit: 2000 }]);
        const entries = Array.isArray(registryResult?.entries) ? registryResult.entries : [];
        const registryKeyBySymbol = {
            LUSD: 'lUSD',
            WSOL: 'wSOL',
            WETH: 'wETH',
            WBNB: 'wBNB',
        };

        entries.forEach((entry) => {
            const registryKey = registryKeyBySymbol[String(entry?.symbol || '').toUpperCase()];
            if (!registryKey || !TOKEN_REGISTRY[registryKey]) return;
            const token = TOKEN_REGISTRY[registryKey];
            if (entry.program) token.address = entry.program;
            if (entry.name) token.name = entry.name;
            if (entry.decimals !== undefined && entry.decimals !== null) {
                const decimals = Number(entry.decimals);
                if (Number.isFinite(decimals)) token.decimals = decimals;
            }
            if (entry.metadata) {
                if (entry.metadata.icon_class) token.icon = entry.metadata.icon_class;
                if (entry.metadata.logo_url) token.logoUrl = entry.metadata.logo_url;
                if (entry.metadata.name) token.name = entry.metadata.name;
            }
        });
    } catch (e) {
        // Silently fall through to local-network override fallback
    }

    // Local token address overrides stay available only on explicit local networks.
    if (isLocalNetwork()) {
        try {
            const stored = localStorage.getItem('lichen_token_addresses');
            if (stored) {
                const addrs = JSON.parse(stored);
                for (const [symbol, addr] of Object.entries(addrs)) {
                    if (TOKEN_REGISTRY[symbol] && addr) {
                        TOKEN_REGISTRY[symbol].address = addr;
                    }
                }
            }
        } catch (e) {
            console.warn('Could not load stored token addresses:', e);
        }
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

// Wallet State — declared at top of file (before helpers that reference it)

// Initialize
// Cache original welcome HTML before any lock screen overwrites it  
let _originalWelcomeHTML = '';

document.addEventListener('DOMContentLoaded', () => {
    // Capture original welcome-container before showUnlockScreen can overwrite it
    const welcomeContainer = document.querySelector('.welcome-container');
    if (welcomeContainer) _originalWelcomeHTML = welcomeContainer.innerHTML;

    bindStaticControls();
    loadWalletState();
    loadTokenRegistry();
    checkWalletStatus();
    setupEventListeners();
    initNetworkSelector();
});

function normalizeWalletActionArg(actionName, rawValue) {
    if (rawValue === undefined) return undefined;
    if (rawValue === 'true') return true;
    if (rawValue === 'false') return false;
    if (actionName === 'removeWordAt') return Number(rawValue);
    return rawValue;
}

function invokeWalletAction(actionName, actionArg, triggerEl) {
    const fn = actionName ? window[actionName] : null;
    if (typeof fn !== 'function') return;

    if (actionArg !== undefined && triggerEl) {
        fn(actionArg, triggerEl);
        return;
    }

    if (actionArg !== undefined) {
        fn(actionArg);
        return;
    }

    if (triggerEl) {
        fn(triggerEl);
        return;
    }

    fn();
}

function bindStaticControls() {
    if (bindStaticControls.bound) return;
    bindStaticControls.bound = true;

    document.addEventListener('click', (event) => {
        const target = event.target instanceof Element ? event.target : null;
        if (!target) return;

        const actionEl = target.closest('[data-wallet-action], [data-wallet-trigger]');
        if (!actionEl) return;

        if (actionEl.tagName === 'A') {
            event.preventDefault();
        }

        const triggerId = actionEl.dataset.walletTrigger;
        if (triggerId) {
            document.getElementById(triggerId)?.click();
            return;
        }

        const actionName = actionEl.dataset.walletAction;
        if (!actionName) return;

        if (actionName === 'fillShieldMax') {
            const shieldAmountInput = document.getElementById('shieldAmount');
            if (shieldAmountInput) {
                const maxShieldable = Math.max(0, (window.walletBalance || 0) - getNetworkBaseFeeLicn());
                shieldAmountInput.value = maxShieldable.toFixed(4);
            }
            return;
        }

        const actionArg = normalizeWalletActionArg(actionName, actionEl.dataset.walletArg);
        const passElement = actionEl.dataset.walletPassEl === 'true';
        invokeWalletAction(actionName, actionArg, passElement ? actionEl : null);
    });

    document.addEventListener('change', (event) => {
        const target = event.target instanceof Element ? event.target : null;
        if (!target) return;

        const changeEl = target.closest('[data-wallet-change]');
        if (!changeEl) return;
        invokeWalletAction(changeEl.dataset.walletChange);
    });

    document.addEventListener('keydown', (event) => {
        const target = event.target;
        if (!(target instanceof HTMLElement)) return;

        if (target.id === 'unlockPassword' && event.key === 'Enter') {
            event.preventDefault();
            unlockWallet();
        }
    });
}

// WL-09: Load wallet state from localStorage.
// Private keys are encrypted at rest; the lock screen gates access on reopen.
function loadWalletState() {
    const stored = localStorage.getItem('lichenWalletState');
    if (stored) {
        try {
            const parsed = JSON.parse(stored);
            // AUDIT-FIX W-9: Validate structure before trusting parsed data
            if (parsed && typeof parsed === 'object' && Array.isArray(parsed.wallets)) {
                walletState = {
                    wallets: parsed.wallets,
                    activeWalletId: parsed.activeWalletId || null,
                    isLocked: parsed.isLocked !== false,
                    network: parsed.network || LICHEN_CONFIG.defaultNetwork,
                    settings: {
                        currency: (parsed.settings && parsed.settings.currency) || 'USD',
                        lockTimeout: (parsed.settings && typeof parsed.settings.lockTimeout === 'number') ? parsed.settings.lockTimeout : 300000,
                        ...(parsed.settings || {})
                    }
                };
            }
        } catch (e) {
            console.warn('Failed to parse wallet state, using defaults:', e);
        }
    }
}

// WL-09: Save wallet state to localStorage
function saveWalletState() {
    localStorage.setItem('lichenWalletState', JSON.stringify(walletState));
}

// Check if wallet exists and show appropriate screen
function checkWalletStatus() {
    if (walletState.wallets.length === 0) {
        showScreen('welcomeScreen');
    } else {
        // Always require unlock on page load for security
        walletState.isLocked = true;
        showUnlockScreen();
    }
}

// Show unlock screen
function showUnlockScreen() {
    showScreen('welcomeScreen');
    const container = document.querySelector('.welcome-container');
    container.innerHTML = `
        <div class="unlock-card">
            <div class="welcome-logo">
                <img src="LichenWallet_Logo_256.png" class="logo-icon" alt="LichenWallet">
                <h1>LichenWallet</h1>
            </div>
            <p class="unlock-greeting">Welcome back!</p>
            
            <div class="unlock-form">
                <label class="unlock-label">Enter Password</label>
                <input type="password" id="unlockPassword" class="form-input unlock-input" placeholder="Password" autofocus>
            </div>
            
            <button class="btn btn-primary unlock-btn" data-wallet-action="unlockWallet">
                <i class="fas fa-unlock"></i> Unlock Wallet
            </button>
            <div class="unlock-logout">
                <button class="btn btn-danger btn-small" data-wallet-action="logoutWallet">
                    <i class="fas fa-sign-out-alt"></i> Logout
                </button>
            </div>
        </div>
    `;

    document.getElementById('unlockPassword')?.focus();
}

// Unlock wallet with password
async function unlockWallet() {
    const password = document.getElementById('unlockPassword').value;

    if (!password) {
        showToast('Please enter password', 'error');
        return;
    }

    try {
        // Try to decrypt first wallet as validation
        const firstWallet = walletState.wallets[0];
        await LichenCrypto.decryptPrivateKey(firstWallet.encryptedKey, password);

        // Success - unlock and show dashboard
        walletState.isLocked = false;
        saveWalletState();
        showToast('Wallet unlocked!');
        showDashboard();
        resetLockTimer();

    } catch (error) {
        showToast('Incorrect password', 'error');
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
        showToast('Password must be at least 8 characters', 'error');
        return;
    }

    if (password !== confirm) {
        showToast('Passwords do not match', 'error');
        return;
    }

    // Generate mnemonic
    createdMnemonic = await LichenCrypto.generateMnemonic();
    createdKeypair = await LichenCrypto.mnemonicToKeypair(createdMnemonic);

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
        <button class="btn btn-sm btn-secondary" data-wallet-action="copyMnemonic">
            <i class="fas fa-copy"></i> Copy
        </button>
        <button class="btn btn-sm btn-secondary" data-wallet-action="downloadMnemonic">
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
    const filename = `lichen-wallet-seed-${wallet.name}-${Date.now()}.txt`;
    const content = `LichenWallet Seed Phrase\n` +
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
                ${Array.from({ length: 12 }, (_, i) => `
                    <div class="confirm-slot" data-index="${i}" data-wallet-action="removeWordAt" data-wallet-arg="${i}">
                        <span class="slot-number">${i + 1}.</span>
                    </div>
                `).join('')}
            </div>
        </div>
        <div class="confirm-section">
            <p class="confirm-section-label">Available words:</p>
            <div class="seed-word-pool">
                ${shuffled.map(word => `
                    <button class="confirm-word" data-wallet-action="selectWord" data-wallet-arg="${escapeHtml(word)}" data-word="${escapeHtml(word)}">
                        ${escapeHtml(word)}
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
                showToast('Words are in wrong order. Try again!', 'error');
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
    const encrypted = await LichenCrypto.encryptPrivateKey(createdKeypair.privateKey, password);

    // Create wallet object
    // Encrypt mnemonic alongside the private key (same password, separate ciphertext)
    const encryptedMnemonic = await LichenCrypto.encryptPrivateKey(createdMnemonic, password);

    const wallet = {
        id: LichenCrypto.generateId(),
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

    // AUDIT-FIX W-9: Zero sensitive globals immediately after use
    if (createdKeypair && createdKeypair.seed) zeroBytes(createdKeypair.seed);
    createdKeypair = null;
    createdMnemonic = '';

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

    buildImportMnemonicGrid();
}

function buildImportMnemonicGrid() {
    const grid = document.getElementById('importSeedGrid');
    if (!grid) return;
    if (grid.dataset.ready === '1') return;

    for (let i = 0; i < 24; i++) {
        const input = document.createElement('input');
        input.type = 'text';
        input.placeholder = `Word ${i + 1}`;
        input.className = 'form-input';
        input.dataset.wordIdx = i;
        if (i >= 12) input.style.display = 'none';
        grid.appendChild(input);
    }

    grid.addEventListener('paste', (e) => {
        const text = (e.clipboardData || window.clipboardData).getData('text').trim();
        const words = text.split(/\s+/).filter(Boolean);
        if (words.length < 2) return;

        e.preventDefault();
        const inputs = Array.from(grid.querySelectorAll('input'));
        if (words.length > 12) inputs.forEach(inp => { inp.style.display = ''; });
        words.slice(0, 24).forEach((word, idx) => {
            if (inputs[idx]) inputs[idx].value = word.toLowerCase();
        });
    });

    grid.dataset.ready = '1';
}

function getImportMnemonicFromGrid() {
    const inputs = Array.from(document.querySelectorAll('#importSeedGrid input'));
    const words = inputs.map(i => (i.value || '').trim().toLowerCase()).filter(Boolean);
    return words.join(' ');
}

async function importWalletSeed() {
    const seed = getImportMnemonicFromGrid();
    const password = document.getElementById('importPassword').value;

    if (!LichenCrypto.isValidMnemonic(seed)) {
        showToast('Invalid seed phrase', 'error');
        return;
    }

    // AUDIT-FIX W-7: Full async BIP39 checksum verification on import
    if (LichenCrypto.isValidMnemonicAsync) {
        const checksumValid = await LichenCrypto.isValidMnemonicAsync(seed);
        if (!checksumValid) {
            showToast('Invalid seed phrase — BIP39 checksum mismatch. Please check your words and try again.', 'error');
            return;
        }
    }

    if (!password || password.length < 8) {
        showToast('Password must be at least 8 characters', 'error');
        return;
    }

    const keypair = await LichenCrypto.mnemonicToKeypair(seed);
    const encrypted = await LichenCrypto.encryptPrivateKey(keypair.privateKey, password);
    const encryptedMnemonic = await LichenCrypto.encryptPrivateKey(seed, password);

    const wallet = {
        id: LichenCrypto.generateId(),
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

    let normalizedKey = privateKey.replace(/^0x/, '');
    if (!normalizedKey || !/^[0-9a-fA-F]+$/.test(normalizedKey)) {
        showToast('Invalid private key format (must be hex characters)', 'error');
        return;
    }
    if (normalizedKey.length !== 64) {
        showToast('Invalid private key length (must be 64 hex characters)', 'error');
        return;
    }

    if (!password || password.length < 8) {
        showToast('Password must be at least 8 characters', 'error');
        return;
    }

    const publicKey = await LichenCrypto.derivePublicKey(LichenCrypto.hexToBytes(normalizedKey));
    const address = await LichenCrypto.publicKeyToAddress(publicKey);
    const encrypted = await LichenCrypto.encryptPrivateKey(normalizedKey, password);

    const wallet = {
        id: LichenCrypto.generateId(),
        name: `Wallet ${walletState.wallets.length + 1}`,
        address,
        publicKey: LichenCrypto.bytesToHex(publicKey),
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
        showToast('Please select a JSON file', 'error');
        return;
    }

    const reader = new FileReader();
    reader.onload = async (e) => {
        try {
            const keystore = JSON.parse(e.target.result);

            if (!keystore.privateKey && !keystore.encryptedSeed && !keystore.seed) {
                showToast('Invalid keystore format: no key data found', 'error');
                return;
            }

            // Extract private key (seed) from keystore
            let seedHex;
            if (keystore.encryptedSeed) {
                const importPw = document.getElementById('importPasswordJson').value;
                seedHex = await LichenCrypto.decryptPrivateKey(keystore.encryptedSeed, importPw);
            } else if (typeof keystore.seed === 'string') {
                seedHex = keystore.seed;
            } else if (Array.isArray(keystore.seed)) {
                const seedBytes = new Uint8Array(keystore.seed);
                if (seedBytes.length !== 32) {
                    showToast('Invalid keystore seed length (must be 32 bytes)', 'error');
                    return;
                }
                seedHex = LichenCrypto.bytesToHex(seedBytes);
            } else if (typeof keystore.privateKey === 'string') {
                seedHex = keystore.privateKey;
            } else {
                const privBytes = new Uint8Array(keystore.privateKey);
                if (privBytes.length !== 32) {
                    showToast('Invalid keystore privateKey length (must be 32 bytes)', 'error');
                    return;
                }
                seedHex = LichenCrypto.bytesToHex(privBytes);
            }

            // Reconstruct keypair
            const seed = LichenCrypto.hexToBytes(seedHex);
            const publicKey = await LichenCrypto.derivePublicKey(seed);
            const address = await LichenCrypto.publicKeyToAddress(publicKey);

            if (!password || password.length < 8) {
                showToast('Password must be at least 8 characters', 'error');
                return;
            }

            const encrypted = await LichenCrypto.encryptPrivateKey(seedHex, password);

            const wallet = {
                id: LichenCrypto.generateId(),
                name: keystore.name || `Imported ${walletState.wallets.length + 1}`,
                address: address,
                publicKey: LichenCrypto.bytesToHex(publicKey),
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
            showToast('Invalid JSON file: ' + error.message, 'error');
        }
    };
    reader.readAsText(file);
}

// ===== DASHBOARD =====
async function showDashboard() {
    showScreen('walletDashboard');
    setupDashboardTabs();
    setupWalletSelector();
    // Fetch live prices before rendering balances
    await fetchLivePrices();
    await refreshBalance();
    await loadAssets();
    await loadActivity();
    await loadStaking();
    refreshNFTs();
    connectBalanceWebSocket();
    startBalancePolling();
}

async function initShieldedForActiveWallet() {
    const wallet = getActiveWallet();
    if (!wallet || !wallet.encryptedKey || typeof initShielded !== 'function') return;

    // AUDIT-FIX W-2: Use secure password modal instead of window.prompt()
    const values = await showPasswordModal({
        title: 'Initialize Shielded Wallet',
        message: 'Enter your wallet password to initialize the shielded pool',
        icon: 'fas fa-shield-alt',
        confirmText: 'Initialize',
        fields: [
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Enter password' }
        ]
    });
    const password = values ? values.password : null;
    if (!password) {
        showToast('Shielded initialization cancelled');
        return;
    }

    let decryptedSeedHex = null;
    try {
        if (wallet.encryptedMnemonic) {
            try {
                const mnemonic = await LichenCrypto.decryptPrivateKey(wallet.encryptedMnemonic, password);
                if (mnemonic && LichenCrypto.isValidMnemonic && LichenCrypto.isValidMnemonic(mnemonic)) {
                    const keypair = await LichenCrypto.mnemonicToKeypair(mnemonic);
                    decryptedSeedHex = keypair.privateKey;
                    zeroBytes(keypair.seed);
                }
            } catch (_) {
                // Fall back to encrypted private key path.
            }
        }

        if (!decryptedSeedHex) {
            decryptedSeedHex = await LichenCrypto.decryptPrivateKey(wallet.encryptedKey, password);
        }

        if (!/^[0-9a-fA-F]{64}$/.test(decryptedSeedHex || '')) {
            throw new Error('Invalid decrypted wallet seed');
        }

        const domain = new TextEncoder().encode('lichen-shielded-spending-seed-v1');
        const seedBytes = LichenCrypto.hexToBytes(decryptedSeedHex);
        const keyMaterial = new Uint8Array(seedBytes.length + domain.length);
        keyMaterial.set(seedBytes, 0);
        keyMaterial.set(domain, seedBytes.length);

        const digest = await crypto.subtle.digest('SHA-256', keyMaterial);
        const shieldSeed = new Uint8Array(digest);

        zeroBytes(seedBytes);
        zeroBytes(keyMaterial);

        await initShielded(shieldSeed);
        zeroBytes(shieldSeed);
    } catch (error) {
        showToast('Failed to initialize shielded wallet: ' + (error?.message || 'unknown error'));
    } finally {
        decryptedSeedHex = null;
    }
}

let _dashboardTabsInitialized = false;
function setupDashboardTabs() {
    if (_dashboardTabsInitialized) return;
    _dashboardTabsInitialized = true;
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
            if (tabName === 'shield' && typeof initShielded === 'function') {
                if (!shieldedState?.initialized) {
                    initShieldedForActiveWallet();
                } else if (typeof syncShieldedState === 'function') {
                    syncShieldedState();
                }
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
    // AUDIT-FIX FE-1: Escape user-controlled wallet names to prevent XSS
    dropdown.innerHTML = walletState.wallets.map(w => {
        const shortAddr = escapeHtml(w.address.slice(0, 12) + '...');
        const safeName = escapeHtml(w.name);
        const safeId = escapeHtml(w.id);
        return `
        <div class="wallet-dropdown-item" data-wallet-action="switchWallet" data-wallet-arg="${safeId}" style="display: flex; align-items: center; gap: 0.5rem; white-space: nowrap;">
            <strong style="flex-shrink: 0;">${safeName}</strong>
            <span style="font-family: 'JetBrains Mono', monospace; font-size: 0.78rem; color: var(--text-muted); overflow: hidden; text-overflow: ellipsis;">${shortAddr}</span>
        </div>`;
    }).join('') + `
        <div class="wallet-dropdown-item" data-wallet-action="showCreateWalletFromDashboard" style="color: var(--primary); font-weight: 600; display: flex; align-items: center; gap: 0.5rem;">
            <i class="fas fa-plus"></i> Create New Wallet
        </div>
        <div class="wallet-dropdown-item" data-wallet-action="showImportWalletFromDashboard" style="color: var(--success); font-weight: 600; display: flex; align-items: center; gap: 0.5rem;">
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
    clearStakingValidatorsCache();
    if (typeof clearIdentityCache === 'function') {
        clearIdentityCache();
    }
    // Close dropdown before refreshing dashboard
    document.getElementById('walletDropdown').classList.remove('show');
    // Reconnect WS + polling for new wallet address
    stopBalancePolling();
    disconnectBalanceWebSocket();
    _wsReconnectDelay = 1000;  // Reset backoff for intentional switch
    showDashboard();
}

async function refreshBalance() {
    const wallet = getActiveWallet();
    if (!wallet) return;

    try {
        const balance = await rpc.getBalance(wallet.address);
        const licn = parseFloat(balance.licn) || 0;
        const parsedSpendable = parseFloat(balance.spendable_licn);
        const spendableLicn = Number.isFinite(parsedSpendable) ? parsedSpendable : licn;
        window.totalWalletBalance = licn;
        window.walletBalance = spendableLicn;

        // Fetch all token balances in parallel
        const tokenBalances = await getAllTokenBalances(wallet.address);

        // Calculate total USD value from live prices
        let totalUsd = licn * getPrice('LICN');
        for (const [symbol, bal] of Object.entries(tokenBalances)) {
            totalUsd += bal * getPrice(symbol);
        }

        // Use saved display settings
        const settings = walletState.settings || {};
        const decimals = settings.decimals || 6;
        const currency = settings.currency || 'USD';
        const currencySymbols = { USD: '$', EUR: '€', GBP: '£', JPY: '¥' };
        const sym = currencySymbols[currency] || '$';

        document.getElementById('totalBalance').textContent = `${fmtToken(licn)} LICN`;
        document.getElementById('balanceUsd').textContent = `${fmtUsd(totalUsd, sym)} ${currency}`;

        // Balance breakdown — show spendable/staked/locked/moss split when non-trivial
        const breakdownEl = document.getElementById('balanceBreakdown');
        if (breakdownEl) {
            const spendable = parseFloat(balance.spendable_licn) || 0;
            const staked = parseFloat(balance.staked_licn) || 0;
            const locked = parseFloat(balance.locked_licn) || 0;
            const mossStaked = parseFloat(balance.moss_staked_licn) || 0;
            const hasBreakdown = staked > 0 || locked > 0 || mossStaked > 0;
            if (hasBreakdown) {
                const parts = [`<i class="fas fa-wallet" style="opacity:0.5;"></i> Spendable: <strong>${fmtToken(spendable)}</strong>`];
                if (staked > 0) parts.push(`<i class="fas fa-lock" style="opacity:0.5;"></i> Staked: <strong>${fmtToken(staked)}</strong>`);
                if (mossStaked > 0) parts.push(`<i class="fas fa-coins" style="opacity:0.5;"></i> Liquid Staking: <strong>${fmtToken(mossStaked)}</strong>`);
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
        window.totalWalletBalance = 0;
        window.walletBalance = 0;
        document.getElementById('totalBalance').textContent = '0.00 LICN';
        document.getElementById('balanceUsd').textContent = `${sym}0.00 ${currency}`;
    }
}

async function loadAssets() {
    const assetsList = document.getElementById('assetsList');
    const wallet = getActiveWallet();
    if (!wallet) return;

    const balance = await rpc.getBalance(wallet.address).catch(() => ({ licn: 0 }));
    const licn = parseFloat(balance.licn) || 0;

    // Fetch all token balances in parallel
    const tokenBalances = await getAllTokenBalances(wallet.address);

    // Live prices for display
    const settings = walletState.settings || {};
    const decimals = settings.decimals || 6;
    const currency = settings.currency || 'USD';
    const currencySymbols = { USD: '$', EUR: '€', GBP: '£', JPY: '¥' };
    const sym = currencySymbols[currency] || '$';

    // Build asset list HTML
    let html = '';

    // LICN (always first, always shown)
    const licnUsd = licn * getPrice('LICN');
    html += `
        <div class="asset-item" style="cursor: default;">
            <div class="asset-icon asset-icon-lichen"><img src="${LICN_LOGO_URL}" alt="LICN" style="width:20px;height:20px;border-radius:50%;object-fit:cover;"></div>
            <div class="asset-info">
                <div class="asset-name">Lichen</div>
                <div class="asset-symbol">LICN</div>
            </div>
            <div class="asset-balance">
                <div class="asset-amount">${fmtToken(licn)}</div>
                <div class="asset-value">${fmtUsd(licnUsd, sym)}</div>
            </div>
        </div>
    `;

    // Wrapped tokens (only show when balance > 0)
    for (const [symbol, token] of Object.entries(TOKEN_REGISTRY)) {
        const bal = tokenBalances[symbol] || 0;
        const usdVal = bal * getPrice(symbol);

        if (bal > 0) {
            const tokenIcon = token.logoUrl
                ? `<img src="${token.logoUrl}" alt="${token.symbol}" style="width:20px;height:20px;border-radius:50%;object-fit:cover;">`
                : `<i class="${token.icon}"></i>`;
            html += `
                <div class="asset-item" style="cursor: default;">
                    <div class="asset-icon" style="color: ${token.color};">${tokenIcon}</div>
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
const STAKING_VALIDATORS_CACHE_TTL_MS = 30 * 1000;
let _stakingValidatorsCache = {
    network: null,
    updatedAt: 0,
    validators: []
};

function clearStakingValidatorsCache() {
    _stakingValidatorsCache = {
        network: null,
        updatedAt: 0,
        validators: []
    };
}

async function getStakingValidators() {
    const now = Date.now();
    const network = getSelectedNetwork();
    const isFresh = (
        _stakingValidatorsCache.network === network
        && (now - Number(_stakingValidatorsCache.updatedAt || 0)) < STAKING_VALIDATORS_CACHE_TTL_MS
        && Array.isArray(_stakingValidatorsCache.validators)
    );
    if (isFresh) {
        return _stakingValidatorsCache.validators;
    }

    const validatorsResponse = await rpc.call('getValidators');
    const validators = Array.isArray(validatorsResponse?.validators) ? validatorsResponse.validators : [];
    _stakingValidatorsCache = {
        network,
        updatedAt: now,
        validators
    };
    return validators;
}

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
        const requestBeforeSlot = _activityBeforeSlot;
        let rpcHasMore = null;
        let rpcNextBeforeSlot = null;
        try {
            const opts = { limit: ACTIVITY_PAGE_SIZE };
            if (requestBeforeSlot) opts.before_slot = requestBeforeSlot;
            const result = await rpc.call('getTransactionsByAddress', [wallet.address, opts]);
            transactions = result?.transactions || (Array.isArray(result) ? result : []);
            if (result && !Array.isArray(result)) {
                if (typeof result.has_more === 'boolean') rpcHasMore = result.has_more;
                const nextBeforeSlot = Number(result.next_before_slot);
                if (Number.isFinite(nextBeforeSlot) && nextBeforeSlot > 0) rpcNextBeforeSlot = nextBeforeSlot;
            }
        } catch (e) {
            // Account may not exist on-chain yet
        }

        // Fetch airdrops from faucet (only on first page, only if faucet is configured)
        let airdrops = [];
        if (!requestBeforeSlot) {
            try {
                const faucetUrl = (typeof LICHEN_CONFIG !== 'undefined' && LICHEN_CONFIG.faucet) ? LICHEN_CONFIG.faucet : null;
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
                            amount: a.amount_licn * SPORES_PER_LICN,
                            timestamp: a.timestamp_ms,
                            signature: a.signature,
                            isAirdrop: true
                        }));
                    }
                }
            } catch (e) { /* faucet API unavailable — skip silently */ }
        }

        if (typeof rpcHasMore === 'boolean') {
            _activityHasMore = rpcHasMore;
            if (_activityHasMore) {
                if (rpcNextBeforeSlot && rpcNextBeforeSlot !== requestBeforeSlot) {
                    _activityBeforeSlot = rpcNextBeforeSlot;
                } else {
                    const lastTx = transactions[transactions.length - 1];
                    const lastSlot = Number(lastTx?.slot || lastTx?.block_slot || 0);
                    if (Number.isFinite(lastSlot) && lastSlot > 0 && lastSlot !== requestBeforeSlot) {
                        _activityBeforeSlot = lastSlot;
                    } else {
                        _activityHasMore = false;
                    }
                }
            } else {
                _activityBeforeSlot = null;
            }
        } else {
            // Legacy fallback: infer pagination from page size + last tx slot
            if (transactions.length > 0) {
                const lastTx = transactions[transactions.length - 1];
                const lastSlot = Number(lastTx?.slot || lastTx?.block_slot || 0);
                if (Number.isFinite(lastSlot) && lastSlot > 0) _activityBeforeSlot = lastSlot;
            }
            _activityHasMore = transactions.length >= ACTIVITY_PAGE_SIZE;
        }

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
                amount = fmtToken(tx.amount / SPORES_PER_LICN);
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
                    'MossStakeDeposit': 'Staked (Liquid Staking)',
                    'MossStakeUnstake': 'Unstaked (Liquid Staking)',
                    'MossStakeClaim': 'Claimed (Liquid Staking)',
                    'MossStakeTransfer': 'Transfer (stLICN)',
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
                // Enhance Contract Call labels with function name from RPC
                if (tx.type === 'Contract' && tx.contract_function) {
                    const fnMap = {
                        'register_identity': 'Register Identity',
                        'register_name': 'Name Registration',
                        'update_profile': 'Update Profile',
                        'set_primary_name': 'Set Primary Name',
                        'add_achievement': 'Achievement',
                        'grant_reputation': 'Reputation Grant',
                        'create_agent': 'Create Agent',
                        'update_agent': 'Update Agent',
                        'transfer': 'Token Transfer',
                        'approve': 'Token Approval',
                        'mint': 'Token Mint',
                        'burn': 'Token Burn',
                    };
                    type = fnMap[tx.contract_function] || `Contract: ${tx.contract_function.replace(/_/g, ' ')}`;
                }
                icon = isSent ? 'fa-arrow-up' : 'fa-arrow-down';
                color = isSent ? '#00C9DB' : '#4ade80';
                // Special icons/colors for non-transfer types
                if (tx.type === 'Stake' || tx.type === 'Unstake' || tx.type === 'ClaimUnstake'
                    || tx.type === 'MossStakeDeposit' || tx.type === 'MossStakeUnstake'
                    || tx.type === 'MossStakeClaim' || tx.type === 'MossStakeTransfer') {
                    icon = 'fa-coins'; color = '#a78bfa';
                    // For staking deposits, show the staked amount as negative (outflow)
                    if (tx.type === 'MossStakeDeposit' || tx.type === 'Stake') {
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
                const amountVal = tx.amount_spores ? tx.amount_spores : (tx.amount || 0);
                amount = fmtToken(amountVal / SPORES_PER_LICN);
                sign = sign || (isSent ? '-' : '+');
            }

            const displayAddr = address && address.length > 20 ? address.slice(0, 8) + '...' + address.slice(-4) : (address || '');
            const date = tx.timestamp ? formatTime(tx.timestamp) : '';
            const sig = tx.signature || tx.hash || '';
            const shortSig = sig ? sig.slice(0, 8) + '...' + sig.slice(-4) : '';
            const explorerBase = (typeof LICHEN_CONFIG !== 'undefined' && LICHEN_CONFIG.explorer)
                ? String(LICHEN_CONFIG.explorer).replace(/\/$/, '')
                : '../explorer';
            const explorerLink = sig ? `${explorerBase}/transaction.html?sig=${encodeURIComponent(sig)}` : '#';
            const isFeeOnly = amount === '0' && (tx.type === 'RegisterEvmAddress' || tx.type === 'Contract'
                || tx.type === 'DeployContract' || tx.type === 'SetContractABI' || tx.type === 'RegisterSymbol'
                || tx.type === 'CreateAccount');
            const isPaidContract = tx.type === 'Contract' && amount !== '0' && parseFloat(amount) > 0;
            const feeSpores = tx.fee_spores || tx.fee || 0;
            const feeAmt = fmtToken(feeSpores / SPORES_PER_LICN);
            const amountStr = isFeeOnly ? `${feeAmt} LICN` : `${sign}${amount} LICN`;
            const feeTag = isFeeOnly ? '<span style="display:inline-block;margin-left:0.35rem;padding:0.05rem 0.4rem;border-radius:4px;font-size:0.65rem;background:rgba(245,158,11,0.15);color:#f59e0b;font-weight:600;vertical-align:middle;">FEE</span>' : '';
            const paidTag = isPaidContract ? '<span style="display:inline-block;margin-left:0.35rem;padding:0.05rem 0.4rem;border-radius:4px;font-size:0.65rem;background:rgba(139,92,246,0.15);color:#a78bfa;font-weight:600;vertical-align:middle;">PAID</span>' : '';

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
                            ${amountStr}${feeTag}${paidTag}
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
                    <button class="btn btn-small btn-secondary" data-wallet-action="loadActivity" data-wallet-arg="false" style="padding: 0.5rem 1.5rem; font-size: 0.85rem;">
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
        const validators = await getStakingValidators();
        const myValidator = validators.find(v => v.pubkey === wallet.address);

        // Always show staking tab (for MossStake or validator staking)
        if (stakingTabBtn) stakingTabBtn.style.display = 'flex';

        if (!myValidator) {
            // Not a validator - show MossStake community staking UI
            if (validatorSection) {
                validatorSection.style.display = 'block';
                validatorSection.innerHTML = `
                    <div class="tab-banner staking">
                        <div class="tab-banner-icon"><i class="fas fa-water"></i></div>
                        <div class="tab-banner-text">
                            <h3>Liquid Staking</h3>
                            <p>Stake LICN, receive stLICN. Earn rewards while keeping liquidity. Choose a lock tier for boosted APY.</p>
                        </div>
                    </div>

                    <div class="staking-stats-grid">
                        <div class="staking-stat-card">
                            <div class="staking-stat-label">Your stLICN</div>
                            <div class="staking-stat-value" id="userStLicn">0</div>
                        </div>
                        <div class="staking-stat-card">
                            <div class="staking-stat-label">Current Value</div>
                            <div class="staking-stat-value green" id="userStakeValue">0 LICN</div>
                        </div>
                        <div class="staking-stat-card">
                            <div class="staking-stat-label">Rewards Earned</div>
                            <div class="staking-stat-value amber" id="userRewardsEarned">0 LICN</div>
                        </div>
                        <div class="staking-stat-card">
                            <div class="staking-stat-label">Your Tier</div>
                            <div class="staking-stat-value purple" id="userLockTier">—</div>
                        </div>
                        <div class="staking-stat-card">
                            <div class="staking-stat-label">Reward Multiplier</div>
                            <div class="staking-stat-value" id="userMultiplier">1.0x</div>
                        </div>
                        <div class="staking-stat-card">
                            <div class="staking-stat-label">Total Staked (Pool)</div>
                            <div class="staking-stat-value" id="totalPoolStaked">0 LICN</div>
                        </div>
                    </div>

                    <div id="mossstakeTiers" style="margin-bottom: 1.5rem;">
                        <h4 class="staking-tiers-heading">
                            <i class="fas fa-layer-group"></i> Staking Tiers & APY
                        </h4>
                        <div id="tiersGrid" class="staking-tiers-grid"></div>
                    </div>

                    <div class="staking-info-box">
                        <i class="fas fa-info-circle"></i>
                        <strong>How it works:</strong> Stake LICN to receive stLICN (liquid receipt). Rewards auto-compound — your stLICN value grows over time.
                        <strong>Flexible tier</strong> has a 7-day cooldown to unstake. <strong>Locked tiers</strong> earn boosted rewards but funds are locked for the chosen duration.
                        After a lock expires, you can unstake with the standard 7-day cooldown.
                    </div>

                    <div class="staking-actions">
                        <button class="btn btn-primary" data-wallet-action="showMossStakeModal">
                            <i class="fas fa-arrow-down"></i> Stake LICN
                        </button>
                        <button id="mossUnstakeBtn" class="btn btn-secondary" data-wallet-action="showMossUnstakeModal">
                            <i class="fas fa-arrow-up"></i> Unstake stLICN
                        </button>
                    </div>

                    <div id="lockStatus" class="staking-lock-status" style="display: none;">
                        <i class="fas fa-lock"></i> <span id="lockStatusText"></span>
                    </div>

                    <div id="pendingUnstakes" class="staking-pending" style="display: none;">
                        <h4>Pending Unstakes (7-day cooldown)</h4>
                        <div id="unstakesList"></div>
                    </div>
                `;

                // Load MossStake position
                loadMossStakePosition(wallet.address);
            }
            return;
        }

        // Is a validator - show tab and generate validator content
        if (stakingTabBtn) stakingTabBtn.style.display = 'flex';
        if (validatorSection) {
            validatorSection.style.display = 'block';

            // Generate staking UI dynamically
            validatorSection.innerHTML = `
                <div class="tab-banner validator">
                    <div class="tab-banner-icon"><i class="fas fa-award"></i></div>
                    <div class="tab-banner-text">
                        <h3>Validator Status</h3>
                        <div id="validatorStatus" class="tab-banner-sub"></div>
                    </div>
                </div>

                <div class="staking-stats-grid">
                    <div class="staking-stat-card">
                        <div class="staking-stat-label">Total Stake</div>
                        <div class="staking-stat-value" id="totalStake">Loading...</div>
                    </div>
                    <div class="staking-stat-card">
                        <div class="staking-stat-label">Bootstrap Grant</div>
                        <div class="staking-stat-value">1,000 LICN</div>
                    </div>
                    <div class="staking-stat-card">
                        <div class="staking-stat-label">Debt Remaining</div>
                        <div class="staking-stat-value amber" id="debtRemaining">Loading...</div>
                    </div>
                    <div class="staking-stat-card">
                        <div class="staking-stat-label">Earned / Vested</div>
                        <div class="staking-stat-value green" id="earnedAmount">Loading...</div>
                    </div>
                </div>

                <div class="staking-stat-card" style="margin-bottom: 1.5rem;">
                    <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 1rem;">
                        <div class="staking-stat-label" style="margin-bottom:0;">Vesting Progress</div>
                        <div id="vestingPercent" style="font-weight: 600; color: var(--text);">0%</div>
                    </div>
                    <div style="height: 8px; background: var(--bg-darker, #060812); border-radius: 4px; overflow: hidden;">
                        <div id="vestingProgressBar" style="height: 100%; background: linear-gradient(90deg, var(--accent), #10b981); width: 0%; transition: width 0.3s ease;"></div>
                    </div>
                    <div id="vestingInfo" style="margin-top: 1rem; font-size: 0.85rem; color: var(--text-muted);"></div>
                </div>

                <div id="graduationInfo" class="staking-info-box" style="display: none; background: linear-gradient(135deg, rgba(16, 185, 129, 0.1), rgba(5, 150, 105, 0.1)); border-color: rgba(16, 185, 129, 0.3);">
                    <div style="display: flex; align-items: center; gap: 0.75rem;">
                        <i class="fas fa-graduation-cap" style="font-size: 1.5rem; color: #10b981;"></i>
                        <div>
                            <div style="font-weight: 600; margin-bottom: 0.25rem; color: var(--text);">Graduated!</div>
                            <div id="graduationSlot" style="font-size: 0.9rem; color: var(--text-muted);"></div>
                        </div>
                    </div>
                </div>
            `;
        }

        // Get validator account to check actual stake
        const account = await rpc.getAccount(wallet.address);
        const totalStake = account?.spores || 0;
        const totalStakeLICN = totalStake / SPORES_PER_LICN;

        // Bootstrap grant info
        const BOOTSTRAP_GRANT = 100000; // 100K LICN
        const bootstrapDebt = myValidator.bootstrap_debt || 0;
        const debtLICN = bootstrapDebt / SPORES_PER_LICN;

        // Calculate earned/vested amount
        const earnedAmount = BOOTSTRAP_GRANT - debtLICN;
        const vestingPercent = (earnedAmount / BOOTSTRAP_GRANT * 100).toFixed(2);

        // Check if graduated
        const isGraduated = myValidator.status === 'Active' && bootstrapDebt === 0;
        const graduationSlot = myValidator.graduation_slot;

        // Update UI
        document.getElementById('totalStake').textContent = `${fmtToken(totalStakeLICN)} LICN`;
        document.getElementById('debtRemaining').textContent = `${fmtToken(debtLICN)} LICN`;
        document.getElementById('earnedAmount').textContent = `${fmtToken(earnedAmount)} LICN`;
        document.getElementById('vestingPercent').textContent = `${vestingPercent}%`;
        document.getElementById('vestingProgressBar').style.width = `${vestingPercent}%`;

        // Status message
        let statusHTML = '';
        if (isGraduated) {
            statusHTML = '<span style="color: #10b981;">✓ Active & Graduated</span>';
        } else if (myValidator.status === 'Active') {
            statusHTML = `<span style="color: #f59e0b;">⚡ Active (Bootstrap phase - ${fmtToken(debtLICN)} LICN remaining)</span>`;
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
            const minutesEstimate = Math.ceil(blocksUntilVested * (MS_PER_SLOT / 1000) / 60); // slot time from shared constants
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
        // Show MossStake UI even on error
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
        loadMossStakePosition(wallet.address);
    }
}

// Load MossStake position for community  stakers
async function loadMossStakePosition(address) {
    try {
        const poolInfo = await rpc.call('getMossStakePoolInfo');
        const position = await rpc.call('getStakingPosition', [address]);
        const queue = await rpc.call('getUnstakingQueue', [address]);

        // Update basic stats
        document.getElementById('userStLicn').textContent = fmtToken(position.st_licn_amount / SPORES_PER_LICN);
        document.getElementById('userStakeValue').textContent = `${fmtToken(position.current_value_licn / SPORES_PER_LICN)} LICN`;
        document.getElementById('totalPoolStaked').textContent = `${fmtToken(poolInfo.total_licn_staked / SPORES_PER_LICN)} LICN`;

        // Rewards
        const rewardsEl = document.getElementById('userRewardsEarned');
        if (rewardsEl) rewardsEl.textContent = `${fmtToken(position.rewards_earned / SPORES_PER_LICN)} LICN`;

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
            const currentSlotEstimate = Math.floor(Date.now() / MS_PER_SLOT);
            if (position.lock_until > currentSlotEstimate) {
                const remainingSlots = position.lock_until - currentSlotEstimate;
                const remainingDays = Math.ceil(remainingSlots / SLOTS_PER_DAY);
                lockStatus.style.display = 'block';
                lockText.textContent = `Position locked (${position.lock_tier_name}). ~${remainingDays} days remaining until unlock at slot ${position.lock_until.toLocaleString()}.`;
            } else {
                lockStatus.style.display = 'none';
            }
        } else if (lockStatus) {
            lockStatus.style.display = 'none';
        }

        // Disable unstake button when position is locked
        const unstakeBtn = document.getElementById('mossUnstakeBtn');
        if (unstakeBtn) {
            const currentSlot = Math.floor(Date.now() / MS_PER_SLOT);
            const posLocked = position.lock_until > 0 && position.lock_until > currentSlot;
            if (posLocked) {
                unstakeBtn.disabled = true;
                unstakeBtn.classList.add('btn-disabled');
                unstakeBtn.title = `Locked until slot ${position.lock_until.toLocaleString()}`;
            } else {
                unstakeBtn.disabled = false;
                unstakeBtn.classList.remove('btn-disabled');
                unstakeBtn.title = '';
            }
        }

        // Render tier cards
        const tiersGrid = document.getElementById('tiersGrid');
        if (tiersGrid && poolInfo.tiers) {
            const tierColorClasses = ['flexible', 'lock30', 'lock180', 'lock365'];
            tiersGrid.innerHTML = poolInfo.tiers.map((t, i) => {
                const isActive = position.lock_tier === t.id && position.st_licn_amount > 0;
                const apyStr = (t.apy_percent || 0).toFixed(1);
                const apyDisplay = poolInfo.total_licn_staked > 0 && t.apy_percent > 0
                    ? `${apyStr}% APY`
                    : `${t.multiplier}x rewards`;
                return `
                    <div class="staking-tier-card ${tierColorClasses[i]} ${isActive ? 'staking-tier-active' : ''}">
                        <div class="staking-tier-name">${t.name}</div>
                        <div class="staking-tier-apy">${apyDisplay}</div>
                        <div class="staking-tier-meta">
                            ${t.lock_days > 0 ? t.lock_days + '-day lock' : '7-day cooldown'}
                            &middot; ${t.multiplier}x rewards
                        </div>
                        ${isActive ? '<div class="staking-tier-badge"><i class="fas fa-check-circle"></i> Active</div>' : ''}
                    </div>
                `;
            }).join('');
        }

        // Show pending unstakes if any
        if (queue.pending_requests && queue.pending_requests.length > 0) {
            document.getElementById('pendingUnstakes').style.display = 'block';
            const unstakesList = document.getElementById('unstakesList');
            unstakesList.innerHTML = queue.pending_requests.map(req => {
                const currentSlot = Math.floor(Date.now() / MS_PER_SLOT);
                const isClaimable = req.claimable_at <= currentSlot;
                const remainSlots = Math.max(0, req.claimable_at - currentSlot);
                const remainDays = (remainSlots / SLOTS_PER_DAY).toFixed(1);
                return `
                    <div class="staking-unstake-item">
                        <span class="staking-unstake-amount">${fmtToken(req.licn_to_receive / SPORES_PER_LICN)} LICN</span>
                        <span class="staking-unstake-status">
                            ${isClaimable
                        ? `<button class="btn btn-small btn-claim" data-wallet-action="claimMossStake">
                                        <i class="fas fa-check-circle"></i> Claim
                                   </button>`
                        : `<span class="staking-unstake-timer"><i class="fas fa-clock"></i> ~${remainDays} days</span>`
                    }
                        </span>
                    </div>
                `;
            }).join('');
        } else {
            document.getElementById('pendingUnstakes').style.display = 'none';
        }
    } catch (error) {
        console.error('Failed to load MossStake position:', error);
    }
}

// Show MossStake modal
async function showMossStakeModal() {
    const wallet = getActiveWallet();
    if (!wallet) { showToast('No active wallet'); return; }

    const values = await showPasswordModal({
        title: 'Stake to Liquid Staking',
        message: `Enter the amount of LICN to stake, choose a lock tier, and sign with your password.
            <div style="margin-top:0.75rem;font-size:0.8rem;color:var(--text-muted);">
                <strong>Flexible:</strong> 7-day cooldown, 1x rewards<br>
                <strong>30-Day Lock:</strong> 1.6x rewards<br>
                <strong>180-Day Lock:</strong> 2.4x rewards<br>
                <strong>365-Day Lock:</strong> 3.6x rewards
            </div>`,
        icon: 'fas fa-layer-group',
        confirmText: 'Stake LICN',
        requiredLicn: typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001,
        fields: [
            { id: 'stakeAmount', label: 'Amount (LICN)', type: 'number', placeholder: '0.00', min: 0, step: 'any' },
            {
                id: 'lockTier', label: 'Lock Tier', type: 'select',
                options: [
                    { value: '0', label: 'Flexible — 7-day cooldown, 1x rewards' },
                    { value: '1', label: '30-Day Lock — 1.6x rewards' },
                    { value: '2', label: '180-Day Lock — 2.4x rewards' },
                    { value: '3', label: '365-Day Lock — 3.6x rewards' },
                ]
            },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Enter password to sign' }
        ]
    });

    if (!values) return;
    let amount = parseFloat(values.stakeAmount);
    if (!amount || amount <= 0) { showToast('Invalid amount'); return; }
    if (!values.password) { showToast('Password required'); return; }

    // Balance guard: check spendable LICN and auto-correct
    try {
        const balResult = await rpc.call('getBalance', [wallet.address]);
        const spendable = (balResult?.spendable || balResult?.balance || 0) / SPORES_PER_LICN;
        const maxStakable = Math.max(0, spendable - BASE_FEE_LICN);
        if (maxStakable <= 0) {
            showToast('Insufficient LICN balance for staking');
            return;
        }
        if (amount > maxStakable) {
            amount = parseFloat(maxStakable.toFixed(6));
            showToast(`Stake amount adjusted to available balance: ${fmtToken(amount)} LICN`);
        }
    } catch (e) { /* let RPC reject */ }

    try {
        const spores = Math.floor(amount * SPORES_PER_LICN);
        const tierByte = parseInt(values.lockTier || '0');
        const latestBlock = await rpc.getLatestBlock();
        const fromPubkey = LichenCrypto.addressToBytes(wallet.address);

        // Instruction type 13 = MossStake deposit, then amount(8), then tier(1)
        const instructionData = new Uint8Array(10);
        instructionData[0] = 13;
        const view = new DataView(instructionData.buffer);
        view.setBigUint64(1, BigInt(spores), true);
        instructionData[9] = tierByte;

        const message = {
            instructions: [{
                program_id: Array.from(new Uint8Array(32)), // SYSTEM_PROGRAM_ID = [0; 32]
                accounts: [Array.from(fromPubkey)],
                data: Array.from(instructionData)
            }],
            blockhash: latestBlock.hash
        };

        const privateKey = await LichenCrypto.decryptPrivateKey(wallet.encryptedKey, values.password);
        const messageBytes = serializeMessageBincode(message);
        const signature = await LichenCrypto.signTransaction(privateKey, messageBytes);

        const transaction = { signatures: [signature], message };
        const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
        const txBase64 = btoa(String.fromCharCode(...txBytes));

        showToast('Submitting liquid staking transaction...');
        const txSig = await rpc.sendTransaction(txBase64);
        showToast(`Staked ${amount} LICN! Sig: ${String(txSig).slice(0, 16)}...`);
        await refreshBalance();
        // Refresh staking position after a brief delay for block inclusion
        setTimeout(() => loadMossStakePosition(wallet.address), 1500);
        setTimeout(() => loadMossStakePosition(wallet.address), 4000);
    } catch (error) {
        showToast('Stake failed: ' + error.message);
    }
}

// Show MossUnstake modal
async function showMossUnstakeModal() {
    const wallet = getActiveWallet();
    if (!wallet) { showToast('No active wallet'); return; }

    const values = await showPasswordModal({
        title: 'Unstake from Liquid Staking',
        message: `Enter the amount of stLICN to unstake. After requesting, there is a <strong>7-day cooldown</strong> before you can claim your LICN.
            <div style="margin-top:0.5rem;font-size:0.8rem;color:var(--text-muted);">
                <i class="fas fa-exclamation-triangle" style="color:#f59e0b;"></i>
                If your position has a lock tier, you must wait for the lock to expire before unstaking.
            </div>`,
        icon: 'fas fa-unlock-alt',
        confirmText: 'Unstake',
        requiredLicn: typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001,
        fields: [
            { id: 'unstakeAmount', label: 'Amount (stLICN)', type: 'number', placeholder: '0.00', min: 0, step: 'any' },
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Enter password to sign' }
        ]
    });

    if (!values) return;
    let amount = parseFloat(values.unstakeAmount);
    if (!amount || amount <= 0) { showToast('Invalid amount'); return; }
    if (!values.password) { showToast('Password required'); return; }

    // Balance guard: check stLICN position and auto-correct
    try {
        const position = await rpc.call('getStakingPosition', [wallet.address]);
        const stLicn = (position?.st_licn_amount || 0) / SPORES_PER_LICN;
        if (stLicn <= 0) {
            showToast('No stLICN balance to unstake');
            return;
        }
        if (amount > stLicn) {
            amount = parseFloat(stLicn.toFixed(6));
            showToast(`Unstake amount adjusted to stLICN balance: ${fmtToken(amount)} stLICN`);
        }
    } catch (e) { /* let RPC reject */ }

    // Fee guard: need LICN for tx fee
    try {
        const balResult = await rpc.call('getBalance', [wallet.address]);
        const spendable = (balResult?.spendable || balResult?.balance || 0) / SPORES_PER_LICN;
        if (spendable < BASE_FEE_LICN) {
            showToast(`Insufficient LICN for fee: need ${fmtToken(BASE_FEE_LICN)} LICN`);
            return;
        }
    } catch (e) { /* let RPC reject */ }

    try {
        const spores = Math.floor(amount * SPORES_PER_LICN);
        const latestBlock = await rpc.getLatestBlock();
        const fromPubkey = LichenCrypto.addressToBytes(wallet.address);

        // Instruction type 14 = MossStake unstake
        const instructionData = new Uint8Array(9);
        instructionData[0] = 14;
        const view = new DataView(instructionData.buffer);
        view.setBigUint64(1, BigInt(spores), true);

        const message = {
            instructions: [{
                program_id: Array.from(new Uint8Array(32)), // SYSTEM_PROGRAM_ID = [0; 32]
                accounts: [Array.from(fromPubkey)],
                data: Array.from(instructionData)
            }],
            blockhash: latestBlock.hash
        };

        const privateKey = await LichenCrypto.decryptPrivateKey(wallet.encryptedKey, values.password);
        const messageBytes = serializeMessageBincode(message);
        const signature = await LichenCrypto.signTransaction(privateKey, messageBytes);

        const transaction = { signatures: [signature], message };
        const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
        const txBase64 = btoa(String.fromCharCode(...txBytes));

        showToast('Submitting liquid unstake transaction...');
        const txSig = await rpc.sendTransaction(txBase64);
        showToast(`Unstake initiated! 7-day cooldown. Sig: ${String(txSig).slice(0, 16)}...`);
        await refreshBalance();
        // Refresh staking position after a brief delay for block inclusion
        setTimeout(() => loadMossStakePosition(wallet.address), 1500);
        setTimeout(() => loadMossStakePosition(wallet.address), 4000);
    } catch (error) {
        showToast('Unstake failed: ' + error.message);
    }
}

// Claim matured MossStake unstake (instruction type 15)
async function claimMossStake() {
    const wallet = getActiveWallet();
    if (!wallet) { showToast('No active wallet'); return; }

    // Balance guard: verify there is a claimable unstake
    try {
        const queue = await rpc.call('getUnstakingQueue', [wallet.address]);
        const pending = queue?.pending_requests || [];
        const currentSlot = Math.floor(Date.now() / MS_PER_SLOT);
        const claimable = pending.filter(r => r.claimable_at <= currentSlot);
        if (claimable.length === 0) {
            showToast('No matured unstakes to claim');
            return;
        }
    } catch (e) { /* let RPC reject */ }

    // Fee guard: need at least the base fee in spendable LICN
    try {
        const balResult = await rpc.call('getBalance', [wallet.address]);
        const spendable = (balResult?.spendable || balResult?.balance || 0) / SPORES_PER_LICN;
        if (spendable < BASE_FEE_LICN) {
            showToast(`Insufficient LICN for fee: need ${fmtToken(BASE_FEE_LICN)} LICN`);
            return;
        }
    } catch (e) { /* let RPC reject */ }

    const values = await showPasswordModal({
        title: 'Claim Unstaked LICN',
        message: 'Enter your password to sign the claim transaction. Your matured LICN will be returned to your spendable balance.',
        icon: 'fas fa-check-circle',
        confirmText: 'Claim',
        requiredLicn: typeof BASE_FEE_LICN !== 'undefined' ? BASE_FEE_LICN : 0.001,
        fields: [
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Enter password to sign' }
        ]
    });

    if (!values || !values.password) return;

    try {
        const latestBlock = await rpc.getLatestBlock();
        const fromPubkey = LichenCrypto.addressToBytes(wallet.address);

        // Instruction type 15 = MossStake claim (data: [15], accounts: [user])
        const instructionData = new Uint8Array([15]);

        const message = {
            instructions: [{
                program_id: Array.from(new Uint8Array(32)),
                accounts: [Array.from(fromPubkey)],
                data: Array.from(instructionData)
            }],
            blockhash: latestBlock.hash
        };

        const privateKey = await LichenCrypto.decryptPrivateKey(wallet.encryptedKey, values.password);
        const messageBytes = serializeMessageBincode(message);
        const signature = await LichenCrypto.signTransaction(privateKey, messageBytes);

        const transaction = { signatures: [signature], message };
        const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
        const txBase64 = btoa(String.fromCharCode(...txBytes));

        showToast('Claiming unstaked LICN...');
        const txSig = await rpc.sendTransaction(txBase64);
        showToast(`Claimed! Sig: ${String(txSig).slice(0, 16)}...`);
        await refreshBalance();
        setTimeout(() => loadMossStakePosition(wallet.address), 1500);
        setTimeout(() => loadMossStakePosition(wallet.address), 4000);
    } catch (error) {
        showToast('Claim failed: ' + error.message);
    }
}

// ===== MODALS =====
async function showSend() {
    const wallet = getActiveWallet();
    if (!wallet) return;

    await refreshDynamicFeeConfig();

    // Dynamically populate token dropdown from on-chain data — only show tokens with balance
    const select = document.getElementById('sendToken');
    if (select) {
        select.innerHTML = '<option value="LICN">LICN</option>';

        try {
            // Fetch all token accounts for this address from the chain
            const tokenAccounts = await rpc.call('getTokenAccounts', [wallet.address]);
            if (Array.isArray(tokenAccounts)) {
                const seen = new Set();
                for (const acct of tokenAccounts) {
                    const sym = acct.symbol || acct.token_symbol || '';
                    const bal = Number(acct.balance || acct.amount || 0);
                    if (sym && bal > 0 && !seen.has(sym)) {
                        seen.add(sym);
                        select.innerHTML += `<option value="${sym}">${sym}</option>`;
                    }
                }
            }
        } catch (e) {
            // Fallback: check TOKEN_REGISTRY balances
            try {
                const tokenBalances = await getAllTokenBalances(wallet.address);
                for (const [symbol, token] of Object.entries(TOKEN_REGISTRY)) {
                    const bal = tokenBalances[symbol] || 0;
                    if (bal > 0) {
                        select.innerHTML += `<option value="${symbol}">${symbol}</option>`;
                    }
                }
            } catch (e2) { /* still show LICN */ }
        }

        // Add stLICN if user has a staking position
        try {
            const position = await rpc.call('getStakingPosition', [wallet.address]);
            if (position && position.st_licn_amount > 0) {
                select.innerHTML += `<option value="stLICN">stLICN</option>`;
            }
        } catch (e) {
            // No staking position
        }
    }

    // Update balance hint
    updateSendTokenUI();

    // Clear previous amount
    const sendAmtInput = document.getElementById('sendAmount');
    if (sendAmtInput) {
        sendAmtInput.value = '';
        sendAmtInput.onblur = async function () {
            let v = parseFloat(this.value);
            if (isNaN(v) || v < 0) { this.value = ''; return; }
            const sel = document.getElementById('sendToken')?.value || 'LICN';
            let maxSend = 0;
            try {
                const baseFee = getNetworkBaseFeeLicn();
                if (sel === 'LICN') {
                    maxSend = Math.max(0, (window.walletBalance || 0) - baseFee);
                } else if (sel === 'stLICN') {
                    const pos = await rpc.call('getStakingPosition', [getActiveWallet()?.address]);
                    maxSend = (pos?.st_licn_amount || 0) / SPORES_PER_LICN;
                } else {
                    maxSend = await getTokenBalanceFormatted(sel, getActiveWallet()?.address) || 0;
                }
            } catch (_) { /* keep 0 */ }
            if (v > maxSend) this.value = maxSend > 0 ? maxSend : '';
        };
    }

    // Disable Send button when LICN balance can't cover fee
    const sendConfirmBtn = document.querySelector('#sendModal .modal-footer .btn-primary');
    if (sendConfirmBtn) {
        const baseFee = getNetworkBaseFeeLicn();
        const bal = window.walletBalance || 0;
        if (bal <= baseFee) {
            sendConfirmBtn.disabled = true;
            sendConfirmBtn.style.opacity = '0.5';
            sendConfirmBtn.style.cursor = 'not-allowed';
            sendConfirmBtn.title = `Insufficient balance — need at least ${fmtToken(baseFee)} LICN for the fee`;
        } else {
            sendConfirmBtn.disabled = false;
            sendConfirmBtn.style.opacity = '';
            sendConfirmBtn.style.cursor = '';
            sendConfirmBtn.title = '';
        }
    }

    document.getElementById('sendModal').classList.add('show');
}

async function showReceive(tab = 'receive') {
    const wallet = getActiveWallet();
    if (!wallet) return;

    // Set native Base58 address
    document.getElementById('walletAddress').value = wallet.address;

    const evmAddressSection = document.getElementById('evmAddressSection');
    const evmAddressInfo = document.getElementById('evmAddressInfo');
    const evmAddressInput = document.getElementById('walletAddressEVM');
    const evmAddress = await getRegisteredEvmAddress(wallet.address);
    if (evmAddress) {
        if (evmAddressInput) evmAddressInput.value = evmAddress;
        if (evmAddressSection) evmAddressSection.style.display = 'block';
        if (evmAddressInfo) evmAddressInfo.style.display = 'none';
    } else {
        if (evmAddressInput) evmAddressInput.value = '';
        if (evmAddressSection) evmAddressSection.style.display = 'none';
        if (evmAddressInfo) evmAddressInfo.style.display = 'block';
    }

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

const BRIDGE_AUTH_TTL_SECS = 24 * 60 * 60;
let activeBridgeAuth = null;

function buildBridgeAccessMessage(userId, issuedAt, expiresAt) {
    return `LICHEN_BRIDGE_ACCESS_V1\nuser_id=${userId}\nissued_at=${issuedAt}\nexpires_at=${expiresAt}\n`;
}

function hasValidBridgeAuth(wallet) {
    if (!wallet || !activeBridgeAuth) return false;
    const now = Math.floor(Date.now() / 1000);
    return activeBridgeAuth.user_id === wallet.address && activeBridgeAuth.expires_at > now + 30;
}

function currentBridgeAuthPayload(wallet) {
    if (!hasValidBridgeAuth(wallet)) return null;
    return {
        issued_at: activeBridgeAuth.issued_at,
        expires_at: activeBridgeAuth.expires_at,
        signature: activeBridgeAuth.signature
    };
}

async function ensureBridgeAccessAuth(wallet) {
    if (hasValidBridgeAuth(wallet)) return activeBridgeAuth;

    const values = await showPasswordModal({
        title: 'Authorize Bridge Access',
        icon: 'fas fa-link',
        message: 'Sign a bridge access authorization for this wallet. The authorization stays only in memory and is used to request a deposit address and poll its status without exposing custody credentials.',
        confirmText: 'Sign Authorization',
        fields: [
            { id: 'password', label: 'Wallet Password', type: 'password', placeholder: 'Enter password to sign' }
        ]
    });

    if (!values || !values.password) {
        throw new Error('Bridge authorization cancelled');
    }

    const privateKey = await LichenCrypto.decryptPrivateKey(wallet.encryptedKey, values.password);
    const issuedAt = Math.floor(Date.now() / 1000);
    const expiresAt = issuedAt + BRIDGE_AUTH_TTL_SECS;
    const messageBytes = new TextEncoder().encode(
        buildBridgeAccessMessage(wallet.address, issuedAt, expiresAt)
    );
    const signature = await LichenCrypto.signTransaction(privateKey, messageBytes);

    activeBridgeAuth = {
        user_id: wallet.address,
        issued_at: issuedAt,
        expires_at: expiresAt,
        signature
    };

    return activeBridgeAuth;
}

async function showDepositInfo(chain) {
    const wallet = getActiveWallet();
    if (!wallet) return;

    const chainInfo = {
        SOL: { name: 'Solana', chain: 'solana', color: '#9945FF', icon: 'fas fa-sun', iconImage: 'https://s2.coinmarketcap.com/static/img/coins/128x128/5426.png', tokens: ['SOL', 'USDC', 'USDT'] },
        ETH: { name: 'Ethereum', chain: 'ethereum', color: '#627EEA', icon: 'fab fa-ethereum', iconImage: 'https://s2.coinmarketcap.com/static/img/coins/128x128/1027.png', tokens: ['ETH', 'USDC', 'USDT'] },
        BNB: { name: 'BNB Chain', chain: 'bnb', color: '#F0B90B', icon: 'fas fa-coins', iconImage: 'https://s2.coinmarketcap.com/static/img/coins/128x128/1839.png', tokens: ['BNB', 'USDC', 'USDT'] }
    };
    const info = chainInfo[chain];
    if (!info) return;

    // Build token buttons with data attributes (no inline onclick)
    const tokenSelect = info.tokens.map(t =>
        `<button class="btn btn-secondary btn-small bridge-token-btn" style="margin: 0.25rem;" data-chain="${escapeHtml(info.chain)}" data-asset="${escapeHtml(t.toLowerCase())}" data-chain-name="${escapeHtml(info.name)}" data-icon="${escapeHtml(info.icon)}">${escapeHtml(t)}</button>`
    ).join(' ');

    showConfirmModal({
        title: `Bridge from ${info.name}`,
        message: `<div style="text-align: left; font-size: 0.9rem;">
            <p style="margin-bottom: 0.75rem;">Select a token to deposit from ${escapeHtml(info.name)} → Lichen:</p>
            <div id="bridgeTokenSelect" style="display: flex; gap: 0.5rem; flex-wrap: wrap; margin-bottom: 1rem;">
                ${tokenSelect}
            </div>
            <div id="bridgeLoadingState" style="display:none; text-align: center; padding: 1.5rem 0;">
                <i class="fas fa-spinner fa-spin" style="font-size: 1.5rem; color: var(--primary);"></i>
                <p style="margin-top: 0.5rem; color: var(--text-secondary);">Generating deposit address...</p>
            </div>
            <div id="bridgeDepositResult" style="display:none;"></div>
            <p id="bridgeHelpText" style="font-size: 0.8rem; color: var(--text-muted);">
                You'll receive a unique deposit address. Send tokens there and they'll be credited
                to your Lichen wallet automatically (~2-5 min after source chain finality).
            </p>
        </div>`,
        icon: info.icon,
        iconImage: info.iconImage,
        confirmText: 'Close',
        cancelText: null
    });

    // Wire token button click handlers
    document.querySelectorAll('.bridge-token-btn').forEach(btn => {
        btn.addEventListener('click', async () => {
            const c = btn.dataset.chain;
            const a = btn.dataset.asset;
            const cn = btn.dataset.chainName;
            const ic = btn.dataset.icon;
            await requestDepositAddress(c, a, cn, ic);
        });
    });
}

async function requestDepositAddress(chain, asset, chainName, icon) {
    const wallet = getActiveWallet();
    if (!wallet) return;

    // Validate inputs
    const validChains = ['solana', 'ethereum', 'bnb'];
    const validAssets = ['usdc', 'usdt', 'sol', 'eth', 'bnb'];
    if (!validChains.includes(chain)) { showToast('Invalid chain selected', 'error'); return; }
    if (!validAssets.includes(asset)) { showToast('Invalid asset selected', 'error'); return; }
    if (!wallet.address || wallet.address.length < 32 || wallet.address.length > 44) { showToast('Invalid wallet address', 'error'); return; }

    let bridgeAuth;
    try {
        bridgeAuth = await ensureBridgeAccessAuth(wallet);
    } catch (error) {
        showToast(error.message || 'Bridge authorization failed', 'error');
        return;
    }

    // Show loading state in the CURRENT modal (don't close it)
    const tokenSelect = document.getElementById('bridgeTokenSelect');
    const loadingState = document.getElementById('bridgeLoadingState');
    const depositResult = document.getElementById('bridgeDepositResult');
    const helpText = document.getElementById('bridgeHelpText');

    if (tokenSelect) tokenSelect.style.display = 'none';
    if (loadingState) loadingState.style.display = 'block';
    if (helpText) helpText.style.display = 'none';

    try {
        // Route through authenticated RPC bridge proxy — custody auth stays server-side
        const data = await trustedRpcCall('createBridgeDeposit', [{
            user_id: wallet.address,
            chain: chain,
            asset: asset,
            auth: {
                issued_at: bridgeAuth.issued_at,
                expires_at: bridgeAuth.expires_at,
                signature: bridgeAuth.signature
            }
        }]);

        const depositAddress = data.address;
        const depositId = data.deposit_id;

        if (!depositAddress || !depositId) {
            throw new Error('Invalid response from bridge service');
        }

        // Escape server-provided values
        const safeAddress = escapeHtml(depositAddress);
        const safeDepositId = escapeHtml(depositId);
        const safeAsset = escapeHtml(asset.toUpperCase());
        const safeChainName = escapeHtml(chainName);

        // Hide loading, show deposit result in the SAME modal
        if (loadingState) loadingState.style.display = 'none';
        if (depositResult) {
            depositResult.style.display = 'block';
            depositResult.innerHTML = `
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
            `;

            // Attach copy handler
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
        }

        // Start polling deposit status via RPC proxy
        startDepositPolling(depositId);
        showToast(`Deposit address generated for ${safeAsset}!`);

    } catch (error) {
        console.error('Deposit request failed:', error);
        // Restore the token selection UI
        if (tokenSelect) tokenSelect.style.display = 'flex';
        if (loadingState) loadingState.style.display = 'none';
        if (helpText) helpText.style.display = 'block';
        showToast(error.message || 'Failed to connect to bridge service', 'error');
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
            const wallet = getActiveWallet();
            const auth = currentBridgeAuthPayload(wallet);
            if (!wallet || !auth) {
                stopDepositPolling();
                const statusEl = document.getElementById('depositStatus');
                if (statusEl) {
                    statusEl.innerHTML = '<i class="fas fa-lock" style="color: #EF476F;"></i> <span>Bridge authorization expired. Re-open the bridge flow to continue status checks.</span>';
                }
                return;
            }

            const deposit = await trustedRpcCall('getBridgeDeposit', [{
                deposit_id: depositId,
                user_id: wallet.address,
                auth
            }]);
            consecutiveErrors = 0; // reset on success
            const statusEl = document.getElementById('depositStatus');
            if (!statusEl) {
                stopDepositPolling();
                return;
            }

            const statusMap = {
                'issued': { icon: 'fas fa-clock', color: 'var(--text-muted)', text: 'Waiting for deposit...' },
                'pending': { icon: 'fas fa-spinner fa-spin', color: '#FFD166', text: 'Deposit detected, confirming...' },
                'confirmed': { icon: 'fas fa-check-circle', color: '#06D6A0', text: 'Deposit confirmed! Sweeping to treasury...' },
                'swept': { icon: 'fas fa-exchange-alt', color: '#06D6A0', text: 'Swept! Minting wrapped tokens on Lichen...' },
                'credited': { icon: 'fas fa-check-double', color: '#06D6A0', text: 'Credited to your Lichen wallet!' },
                'expired': { icon: 'fas fa-times-circle', color: '#EF476F', text: 'Deposit expired — address no longer active.' },
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
        } catch (e) {
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
    showToast('Use the LichenSwap DEX for trading -- launching with mainnet');
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

    // Show loading state
    grid.style.display = 'none';
    empty.style.display = 'none';
    countEl.innerHTML = '<i class="fas fa-spinner fa-spin"></i> Loading...';

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
            // AUDIT-FIX W-1: Escape all server-provided NFT data to prevent XSS
            grid.innerHTML = nfts.map(nft => {
                const safeMint = escapeHtml(String(nft.mint || nft.id || ''));
                const safeName = escapeHtml(String(nft.name || 'Unnamed'));
                const safeCollection = escapeHtml(String(nft.collection || 'Unknown'));
                // Only allow http/https image URLs to prevent javascript: XSS
                const rawImage = String(nft.image || '');
                const safeImage = /^https?:\/\//i.test(rawImage) ? escapeHtml(rawImage) : '';
                return `
                <div class="nft-card" data-wallet-action="showNFTDetail" data-wallet-arg="${safeMint}">
                    <div class="nft-image">
                        ${safeImage
                        ? `<img src="${safeImage}" alt="${safeName}" loading="lazy">`
                        : `<div class="nft-placeholder"><i class="fas fa-gem"></i></div>`}
                    </div>
                    <div class="nft-info">
                        <span class="nft-collection">${safeCollection}</span>
                        <span class="nft-name">${safeName}</span>
                    </div>
                </div>
            `;
            }).join('');
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
    // Navigate to marketplace item detail page
    const id = encodeURIComponent(mintId || '');
    const marketBase = (typeof LICHEN_CONFIG !== 'undefined' && LICHEN_CONFIG.marketplace)
        ? String(LICHEN_CONFIG.marketplace).replace(/\/+$/, '')
        : '../marketplace';
    window.open(marketBase + '/item.html?id=' + id, '_blank');
}

function openMarketplace() {
    const marketBase = (typeof LICHEN_CONFIG !== 'undefined' && LICHEN_CONFIG.marketplace)
        ? String(LICHEN_CONFIG.marketplace).replace(/\/+$/, '')
        : '../marketplace';
    window.open(marketBase + '/index.html', '_blank');
}

function formatLicn(spores) {
    if (typeof spores === 'string') spores = parseInt(spores) || 0;
    return fmtToken(spores / SPORES_PER_LICN) + ' LICN';
}

// escapeHtml provided by shared/utils.js (loaded before this file)

// AUDIT-FIX W-5: Best-effort zeroing of sensitive byte arrays after use
function zeroBytes(arr) {
    if (arr instanceof Uint8Array) {
        arr.fill(0);
    }
}

function wipeSensitiveWalletData(wallet) {
    if (!wallet || typeof wallet !== 'object') return;
    const wipeString = (value) => (typeof value === 'string' ? '0'.repeat(value.length) : value);

    wallet.encryptedKey = wipeString(wallet.encryptedKey) || null;
    for (const field of ['encryptedMnemonic', 'encryptedSeed', 'privateKey']) {
        wallet[field] = wipeString(wallet[field]) || null;
    }
    wallet.cachedTransactions = [];
    wallet.txHistory = [];
    wallet.shieldedNotes = [];
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
            if (sendToken) sendToken.value = 'LICN';
        }
    }
}

function closeSettingsModal() {
    closeModal('settingsModal');
}

function pulseCopyButton(buttonEl) {
    if (!buttonEl) return;
    const icon = buttonEl.querySelector('i');
    if (!icon) return;
    const originalClass = icon.className;
    icon.className = 'fas fa-check';
    setTimeout(() => { icon.className = originalClass; }, 1200);
}

function copyAddress(type = 'native', triggerEl = null) {
    const address = type === 'evm'
        ? document.getElementById('walletAddressEVM').value
        : document.getElementById('walletAddress').value;
    const label = type === 'evm' ? 'EVM address' : 'Native address';

    if (!address) {
        showToast(type === 'evm' ? 'EVM address is available after on-chain registration' : 'Address unavailable');
        return;
    }

    navigator.clipboard.writeText(address).then(() => {
        pulseCopyButton(triggerEl);
        showToast(`✅ ${label} copied to clipboard!`);
    }).catch(() => {
        showToast('❌ Failed to copy');
    });
}

// Generate EVM-compatible address from Base58 address
// Implements Keccak256(pubkey)[12:32] derivation as per core/src/account.rs
function generateEVMAddress(base58Address) {
    try {
        if (window.LichenCrypto && typeof window.LichenCrypto.generateEVMAddress === 'function') {
            const evmAddress = window.LichenCrypto.generateEVMAddress(base58Address);
            if (evmAddress) {
                return evmAddress;
            }
            throw new Error('Invalid base58 address');
        }

        if (typeof bs58 === 'undefined' || !bs58.decode || typeof keccak_256 === 'undefined') {
            throw new Error('Required address derivation libraries unavailable');
        }

        const pubkeyBytes = bs58.decode(base58Address);
        if (pubkeyBytes.length !== 32) {
            throw new Error(`Invalid pubkey length: ${pubkeyBytes.length}`);
        }
        return '0x' + keccak_256(pubkeyBytes).slice(-40);
    } catch (e) {
        console.error('Failed to generate EVM address:', e);
        console.error('Error details:', e.message, e.stack);

        // Return error placeholder instead of broken fallback
        return '0x' + '0'.repeat(40);
    }
}

async function getRegisteredEvmAddress(nativeAddress) {
    if (!nativeAddress) return null;
    const cacheKey = `licnEvmRegistered:${nativeAddress}`;

    try {
        if (localStorage.getItem(cacheKey) === '1') {
            return generateEVMAddress(nativeAddress);
        }
    } catch (_) { }

    try {
        const existing = await rpc.call('getEvmRegistration', [nativeAddress]);
        if (existing && existing.evmAddress) {
            try { localStorage.setItem(cacheKey, '1'); } catch (_) { }
            return existing.evmAddress;
        }
    } catch (_) { }

    return null;
}

// Auto-register EVM address on-chain for seamless MetaMask compatibility
// Sends system instruction opcode 12 with the 20-byte EVM address
// Flow: localStorage cache → RPC check → tx → cache
async function registerEvmAddress(wallet, password) {
    try {
        const cacheKey = `licnEvmRegistered:${wallet.address}`;

        // 1) localStorage cache hit — skip entirely (no RPC, no tx)
        try { if (localStorage.getItem(cacheKey) === '1') return; } catch (_) { }

        // 2) On-chain check via RPC
        try {
            const existing = await rpc.call('getEvmRegistration', [wallet.address]);
            if (existing && existing.evmAddress) {
                // Already registered on-chain — cache locally and return
                try { localStorage.setItem(cacheKey, '1'); } catch (_) { }
                return;
            }
        } catch (_) { } // RPC down — fall through, processor is idempotent anyway

        // 3) Skip if account not funded yet (imported wallets)
        try {
            const bal = await rpc.getBalance(wallet.address);
            if (!bal || (bal.spores === 0 && !bal.spendable)) return;
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
        const fromPubkey = LichenCrypto.addressToBytes(wallet.address);
        const latestBlock = await rpc.getLatestBlock();

        const message = {
            instructions: [{
                program_id: Array.from(systemProgram),
                accounts: [Array.from(fromPubkey)],
                data: Array.from(instructionData)
            }],
            blockhash: latestBlock.hash
        };

        const privateKey = await LichenCrypto.decryptPrivateKey(wallet.encryptedKey, password);
        const messageBytes = serializeMessageBincode(message);
        const signature = await LichenCrypto.signTransaction(privateKey, messageBytes);

        const transaction = { signatures: [signature], message };
        const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
        const txBase64 = btoa(String.fromCharCode(...txBytes));

        await rpc.sendTransaction(txBase64);

        // 6) Cache after successful registration
        try { localStorage.setItem(cacheKey, '1'); } catch (_) { }
    } catch (error) {
        // Don't block wallet creation on registration failure
        console.warn('EVM address registration deferred:', error.message);
    }
}

async function setMaxAmount() {
    const wallet = getActiveWallet();
    if (!wallet) return;

    const selectedToken = document.getElementById('sendToken')?.value || 'LICN';

    try {
        if (selectedToken === 'LICN') {
            const balance = await rpc.getBalance(wallet.address);
            const licn = parseFloat(balance.licn) || 0;
            // Reserve base fee for gas
            const maxAmount = Math.max(0, licn - getNetworkBaseFeeLicn());
            document.getElementById('sendAmount').value = maxAmount.toFixed(6);
        } else if (selectedToken === 'stLICN') {
            // AUDIT-FIX W-3: Fetch stLICN balance from staking position
            const position = await rpc.call('getStakingPosition', [wallet.address]);
            const stLicn = (position?.st_licn_amount || 0) / SPORES_PER_LICN;
            document.getElementById('sendAmount').value = stLicn.toFixed(6);
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

    const selectedToken = document.getElementById('sendToken')?.value || 'LICN';
    const balanceHint = document.getElementById('sendAvailableBalance');
    if (!balanceHint) return;

    try {
        if (selectedToken === 'LICN') {
            const balance = await rpc.getBalance(wallet.address);
            const parsedSpendable = parseFloat(balance.spendable_licn);
            const spendableLicn = Number.isFinite(parsedSpendable) ? parsedSpendable : (parseFloat(balance.licn) || 0);
            balanceHint.textContent = `Available: ${fmtToken(spendableLicn)} LICN`;
        } else if (selectedToken === 'stLICN') {
            const position = await rpc.call('getStakingPosition', [wallet.address]);
            const stLicn = (position?.st_licn_amount || 0) / SPORES_PER_LICN;
            balanceHint.textContent = `Available: ${fmtToken(stLicn)} stLICN`;
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
    let amount = parseFloat(document.getElementById('sendAmount').value);
    const selectedToken = document.getElementById('sendToken')?.value || 'LICN';

    if (!LichenCrypto.isValidAddress(to)) {
        showToast('Invalid recipient address', 'error');
        return;
    }

    if (!amount || amount <= 0) {
        showToast('Invalid amount', 'error');
        return;
    }

    const wallet = getActiveWallet();
    if (!wallet) return;

    // Pre-flight balance check with auto-correction
    try {
        await refreshDynamicFeeConfig();
        const balResult = await rpc.call('getBalance', [wallet.address]);
        const spendable = (balResult?.spendable || balResult?.balance || 0) / SPORES_PER_LICN;
        const baseFee = getNetworkBaseFeeLicn();

        if (selectedToken === 'LICN') {
            const maxSendable = Math.max(0, spendable - baseFee);
            if (maxSendable <= 0) {
                showToast('Insufficient LICN balance (not enough to cover fee)');
                document.getElementById('sendAmount').value = '0';
                return;
            }
            if (amount > maxSendable) {
                amount = parseFloat(maxSendable.toFixed(6));
                document.getElementById('sendAmount').value = amount;
                showToast(`Amount adjusted to available balance: ${fmtToken(amount)} LICN`);
                return; // Let user review the adjusted amount
            }
        } else {
            // Check fee coverage for non-LICN tokens
            if (spendable < baseFee) {
                showToast(`Insufficient LICN for fee: need ${fmtToken(baseFee)} LICN, have ${fmtToken(spendable)}`);
                return;
            }
            // Check token balance
            const tokenBal = await getTokenBalanceFormatted(selectedToken, wallet.address);
            if (tokenBal <= 0) {
                showToast(`No ${selectedToken} balance available`);
                document.getElementById('sendAmount').value = '0';
                return;
            }
            if (amount > tokenBal) {
                amount = parseFloat(tokenBal.toFixed(6));
                document.getElementById('sendAmount').value = amount;
                showToast(`Amount adjusted to available balance: ${fmtToken(amount)} ${selectedToken}`);
                return; // Let user review the adjusted amount
            }
        }
    } catch (e) {
        // Non-blocking: let the RPC reject it if balance is insufficient
    }

    try {
        showToast('Building transaction...');

        // Get recent blockhash
        const latestBlock = await rpc.getLatestBlock();
        const blockhash = latestBlock.hash;

        const fromPubkey = LichenCrypto.addressToBytes(wallet.address);
        const toPubkey = LichenCrypto.addressToBytes(to);
        let message;

        if (selectedToken === 'LICN') {
            // Native LICN transfer
            const spores = Math.floor(amount * SPORES_PER_LICN);
            const systemProgram = new Uint8Array(32); // SYSTEM_PROGRAM_ID = [0; 32]

            const instructionData = new Uint8Array(9);
            instructionData[0] = 0; // Transfer type
            const view = new DataView(instructionData.buffer);
            view.setBigUint64(1, BigInt(spores), true);

            message = {
                instructions: [{
                    program_id: Array.from(systemProgram),
                    accounts: [Array.from(fromPubkey), Array.from(toPubkey)],
                    data: Array.from(instructionData)
                }],
                blockhash: blockhash
            };
        } else if (selectedToken === 'stLICN') {
            // stLICN transfer via MossStake opcode 16
            const stLicnSpores = Math.floor(amount * SPORES_PER_LICN);
            const systemProgram = new Uint8Array(32); // SYSTEM_PROGRAM_ID = [0; 32]

            const instructionData = new Uint8Array(9);
            instructionData[0] = 16; // MossStake transfer
            const view = new DataView(instructionData.buffer);
            view.setBigUint64(1, BigInt(stLicnSpores), true);

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

        // Sign the transaction with the native PQ wallet key
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

        const privateKey = await LichenCrypto.decryptPrivateKey(wallet.encryptedKey, passwordValues.password);
        const messageBytes = serializeMessageBincode(message);
        const signature = await LichenCrypto.signTransaction(privateKey, messageBytes);

        // AUDIT-FIX W-5: Zero sensitive key material after signing
        // (privateKey is a hex string — overwrite not possible; signTransaction zeros seed internally)

        // Build signed transaction
        const transaction = {
            signatures: [signature],
            message: message
        };

        // Serialize and encode
        const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
        const txBase64 = btoa(String.fromCharCode(...txBytes));

        // Send transaction
        showToast('Sending transaction...');
        const txSignature = await rpc.sendTransaction(txBase64);

        // WL-07: Confirm transaction instead of fire-and-forget
        rpc.confirmTransaction(txSignature, 15000).then(result => {
            if (result.confirmed) {
                showToast(`✅ Transaction confirmed on-chain`);
            } else if (result.error) {
                showToast(`⚠️ Transaction may have failed: ${result.error}`);
            }
            refreshBalance();
        }).catch(() => { /* confirmation polling failed, balance will refresh anyway */ });

        showToast(`✅ ${amount} ${selectedToken} sent! Signature: ${String(txSignature).slice(0, 16)}...`);
        closeModal('sendModal');

        // Clear form and reset token selector
        document.getElementById('sendTo').value = '';
        document.getElementById('sendAmount').value = '';
        const tokenSelect = document.getElementById('sendToken');
        if (tokenSelect) tokenSelect.value = 'LICN';

        // Wait briefly for block commitment, then refresh balance + activity
        await new Promise(r => setTimeout(r, 1500));
        await refreshBalance();
        await loadActivity();
        // Second refresh after another 3s to catch slower block finality
        setTimeout(async () => { try { await refreshBalance(); await loadActivity(); } catch (_) { } }, 3000);

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

        // K-3: Only remove wallet-prefixed keys to avoid wiping other app data on shared origins
        Object.keys(localStorage).filter(function (k) {
            return k.startsWith('lichen_wallet_') || k.startsWith('walletState') || k.startsWith('wallet_')
                || k.startsWith('lichenWallet') || k.startsWith('lichen_') || k.startsWith('licnEvmRegistered');
        }).forEach(function (k) { localStorage.removeItem(k); });
        Object.keys(sessionStorage).filter(function (k) {
            return k.startsWith('lichen_wallet_') || k.startsWith('walletState') || k.startsWith('wallet_')
                || k.startsWith('lichenWallet') || k.startsWith('lichen_') || k.startsWith('licnEvmRegistered');
        }).forEach(function (k) { sessionStorage.removeItem(k); });

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
        if (typeof _lichenidAddress !== 'undefined') _lichenidAddress = null;

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

        // Restore original welcome HTML (showUnlockScreen may have overwritten it)
        const welcomeContainer = document.querySelector('.welcome-container');
        if (welcomeContainer && _originalWelcomeHTML) {
            welcomeContainer.innerHTML = _originalWelcomeHTML;
        }

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
        const fieldsHTML = fields.map(field => {
            if (field.type === 'select' && Array.isArray(field.options)) {
                const optionsHTML = field.options.map(opt =>
                    `<option value="${escapeHtml(String(opt.value))}"${opt.selected ? ' selected' : ''}>${escapeHtml(opt.label)}</option>`
                ).join('');
                return `<div class="form-group"><label>${field.label}</label><select id="${field.id}" class="form-input">${optionsHTML}</select></div>`;
            }
            const val = field.value !== undefined ? ` value="${escapeHtml(String(field.value))}"` : '';
            const minAttr = field.min !== undefined ? ` min="${field.min}"` : '';
            const maxAttr = field.max !== undefined ? ` max="${field.max}"` : '';
            const stepAttr = field.step !== undefined ? ` step="${field.step}"` : '';
            return `<div class="form-group"><label>${field.label}</label><input type="${field.type}" id="${field.id}" class="form-input" placeholder="${field.placeholder || ''}"${val}${minAttr}${maxAttr}${stepAttr}></div>`;
        }).join('');

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

        // Balance guard: disable confirm when wallet balance is too low
        if (options.requiredLicn != null && options.requiredLicn > 0) {
            const confirmBtn = modal.querySelector('#passwordModalConfirm');
            const spendable = window.walletBalance || 0;
            if (spendable < options.requiredLicn) {
                confirmBtn.disabled = true;
                confirmBtn.title = `Insufficient balance (need ${options.requiredLicn} LICN, have ${spendable.toFixed(4)})`;
                confirmBtn.style.opacity = '0.5';
                confirmBtn.style.cursor = 'not-allowed';
                // Add warning below the fields
                const warning = document.createElement('div');
                warning.style.cssText = 'color:#ef4444;font-size:0.82rem;margin:0.5rem 0 0.75rem;padding:0.5rem 0.75rem;background:rgba(239,68,68,0.08);border-radius:6px;';
                warning.innerHTML = `<i class="fas fa-exclamation-triangle"></i> Insufficient balance — need at least ${options.requiredLicn} LICN`;
                const body = modal.querySelector('.password-modal-body');
                if (body) body.insertBefore(warning, body.querySelector('.password-modal-actions'));
            }
        }

        // Call onRender callback for dynamic behavior (e.g. cost previews)
        if (typeof options.onRender === 'function') {
            try { options.onRender(modal); } catch (_) { }
        }

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
        const iconHtml = options.iconImage
            ? `<img src="${escapeHtml(options.iconImage)}" alt="" style="width:18px;height:18px;border-radius:50%;object-fit:cover;vertical-align:middle;">`
            : `<i class="${options.icon || 'fas fa-question-circle'}"></i>`;

        modal.innerHTML = `
            <div class="password-modal-content">
                <div class="password-modal-header">
                    <h3>${iconHtml} ${options.title}</h3>
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
                        ${options.cancelText !== null ? `<button class="btn btn-secondary confirm-modal-cancel-btn">
                            <i class="fas fa-times"></i> ${options.cancelText || 'Cancel'}
                        </button>` : ''}
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
        const cancelBtn = modal.querySelector('.confirm-modal-cancel-btn');
        if (cancelBtn) cancelBtn.onclick = dismiss;

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
        const testDecrypt = await LichenCrypto.decryptKeypair(wallet.encryptedKey, password);
        if (!testDecrypt) {
            showToast('❌ Invalid password');
            return;
        }

        // Show private key in modal — export the 32-byte seed (64 hex chars)
        // so it matches the import format (which expects 64 hex chars = 32 bytes)
        const privateKeyHex = testDecrypt.privateKey;

        closeModal('settingsModal');

        // AUDIT-FIX W-2: Use event listeners instead of inline onclick with interpolated values
        const modal = document.createElement('div');
        modal.className = 'modal';
        modal.innerHTML = `
            <div class="modal-content">
                <div class="modal-header">
                    <h3><i class="fas fa-key"></i> Private Key</h3>
                    <button class="modal-close" id="exportPkClose">
                        <i class="fas fa-times"></i>
                    </button>
                </div>
                <div class="modal-body">
                    <div class="warning-box" style="margin-bottom: 1rem;">
                        <i class="fas fa-exclamation-triangle"></i>
                        <strong>⚠️ Never share this key with anyone!</strong>
                    </div>
                    
                    <label style="font-weight: 600; margin-bottom: 0.5rem; display: block;">Private Key (Hex)</label>
                    <textarea class="form-input" readonly style="font-family: monospace; font-size: 0.85rem; height: 100px;" id="exportPkValue"></textarea>
                    
                    <div style="display: flex; gap: 0.75rem; margin-top: 1rem;">
                        <button class="btn btn-primary" id="exportPkCopy">
                            <i class="fas fa-copy"></i> Copy
                        </button>
                        <button class="btn btn-secondary" id="exportPkDownload">
                            <i class="fas fa-download"></i> Download
                        </button>
                    </div>
                </div>
            </div>
        `;
        document.body.appendChild(modal);
        // Set value via DOM property (not innerHTML) to prevent injection
        modal.querySelector('#exportPkValue').value = privateKeyHex;
        const dismissModal = () => { modal.classList.remove('show'); setTimeout(() => modal.remove(), 300); };
        modal.querySelector('#exportPkClose').addEventListener('click', dismissModal);
        modal.querySelector('#exportPkCopy').addEventListener('click', (e) => {
            navigator.clipboard.writeText(privateKeyHex).then(() => {
                pulseCopyButton(e.currentTarget);
                showToast('✅ Private key copied!');
            });
            dismissModal();
        });
        modal.querySelector('#exportPkDownload').addEventListener('click', () => {
            downloadPrivateKey(privateKeyHex, wallet.name);
        });
        requestAnimationFrame(() => modal.classList.add('show'));

    } catch (e) {
        showToast('❌ Failed to export private key');
    }
}

function downloadPrivateKey(privateKeyHex, walletName) {
    const filename = `lichen-wallet-privatekey-${walletName}-${Date.now()}.txt`;
    const content = `LichenWallet Private Key\n` +
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

        const privateKeyHex = await LichenCrypto.decryptPrivateKey(wallet.encryptedKey, password);
        const encryptedSeed = await LichenCrypto.encryptPrivateKey(privateKeyHex, password);

        // Create JSON keystore with encrypted seed and canonical PQ verifying key
        const keystore = {
            name: wallet.name,
            address: wallet.address,
            publicKey: {
                scheme_version: 1,
                bytes: wallet.publicKey
            },
            encryptedSeed: encryptedSeed,
            created: wallet.createdAt || wallet.created,
            exported: new Date().toISOString(),
            version: '3.0',
            keyType: 'ML-DSA-65',
            encryption: 'AES-256-GCM-PBKDF2'
        };

        const filename = `lichen-wallet-keystore-${wallet.name}-${Date.now()}.json`;
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
    if (!wallet || (!wallet.encryptedMnemonic && !wallet.hasMnemonic)) {
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
        const keypair = await LichenCrypto.decryptKeypair(wallet.encryptedKey, password);
        if (!keypair) {
            showToast('❌ Invalid password');
            return;
        }

        // Decrypt the mnemonic
        let mnemonic;
        if (wallet.encryptedMnemonic) {
            mnemonic = await LichenCrypto.decryptPrivateKey(wallet.encryptedMnemonic, password);
        } else {
            showToast('❌ No seed phrase available');
            return;
        }

        const words = mnemonic.split(' ');

        closeModal('settingsModal');

        // AUDIT-FIX W-2: Use event listeners instead of inline onclick with interpolated values
        const modal = document.createElement('div');
        modal.className = 'modal';
        modal.id = 'seedPhraseExportModal';
        modal.innerHTML = `
            <div class="modal-content">
                <div class="modal-header">
                    <h3><i class="fas fa-list-ol"></i> Seed Phrase</h3>
                    <button class="modal-close" id="seedExportClose">
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
                                <span>${escapeHtml(word)}</span>
                            </div>
                        `).join('')}
                    </div>
                    
                    <div style="display: flex; gap: 0.75rem; margin-top: 1rem;">
                        <button class="btn btn-primary" id="seedExportCopy">
                            <i class="fas fa-copy"></i> Copy
                        </button>
                        <button class="btn btn-secondary" id="seedExportDownload">
                            <i class="fas fa-download"></i> Download
                        </button>
                    </div>
                </div>
            </div>
        `;
        document.body.appendChild(modal);
        const dismissSeedModal = () => { modal.classList.remove('show'); setTimeout(() => modal.remove(), 300); };
        modal.querySelector('#seedExportClose').addEventListener('click', dismissSeedModal);
        modal.querySelector('#seedExportCopy').addEventListener('click', (e) => {
            navigator.clipboard.writeText(mnemonic).then(() => {
                pulseCopyButton(e.currentTarget);
                showToast('✅ Seed phrase copied!');
            });
            dismissSeedModal();
        });
        modal.querySelector('#seedExportDownload').addEventListener('click', () => {
            downloadMnemonicExport(mnemonic, wallet.name);
        });
        requestAnimationFrame(() => modal.classList.add('show'));

    } catch (e) {
        showToast('❌ Failed to view seed phrase');
    }
}

function downloadMnemonicExport(mnemonic, walletName) {
    const filename = `lichen-wallet-seed-${walletName}-${Date.now()}.txt`;
    const content = `LichenWallet Seed Phrase\n` +
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
    // AUDIT-FIX W-4: Don't schedule lock when timeout is 0 ("Never")
    const timeout = walletState.settings.lockTimeout;
    if (!walletState.isLocked && timeout > 0) {
        lockTimer = setTimeout(() => {
            lockWallet();
        }, timeout);
    }
}

// AUDIT-FIX W-5: Register all interaction events for auto-lock reset
document.addEventListener('mousemove', resetLockTimer);
document.addEventListener('keypress', resetLockTimer);
document.addEventListener('keydown', resetLockTimer);
document.addEventListener('click', resetLockTimer);
document.addEventListener('touchstart', resetLockTimer);
document.addEventListener('touchmove', resetLockTimer);
document.addEventListener('scroll', resetLockTimer, true);

// ===== NETWORK SELECTOR=====
const NETWORK_LABELS = {};
for (const [k, v] of Object.entries(LICHEN_CONFIG.networks)) { NETWORK_LABELS[k] = v.label; }

const NETWORK_COLORS = {
    'mainnet': '#4ade80',
    'testnet': '#fbbf24',
    'local-testnet': '#38bdf8',
    'local-mainnet': '#a78bfa'
};

function initNetworkSelector() {
    LICHEN_CONFIG.initNetworkSelector('networkSelect', 'lichen_wallet_network', (network) => {
        switchNetwork(network);
    });

    // Restore saved network into wallet state
    const savedNetwork = getSelectedNetwork();
    walletState.network = savedNetwork;
}

async function switchNetwork(network) {
    localStorage.setItem('lichen_wallet_network', network);
    walletState.network = network;
    saveWalletState();

    // Update RPC client endpoint
    rpc.url = getRpcEndpoint();
    await LICHEN_CONFIG.refreshIncidentStatusBanner(network);

    if (typeof window.resetIdentityNetworkCaches === 'function') {
        window.resetIdentityNetworkCaches();
    }

    // Tear down old connections — showDashboard() will re-establish them
    stopBalancePolling();
    disconnectBalanceWebSocket();
    _wsReconnectDelay = 1000;  // Reset backoff for intentional switch

    try {
        await loadTokenRegistry();
    } catch (error) {
        console.warn('Failed to reload trusted wallet metadata on network switch:', error);
    }

    showToast(`Switched to ${NETWORK_LABELS[network] || network}`);

    // Refresh wallet data after network switch (this re-connects WS + polling)
    if (typeof showDashboard === 'function') {
        await showDashboard();
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
    let mainnetRPC;
    let testnetRPC;

    try {
        mainnetRPC = normalizeRpcOverride(document.getElementById('mainnetRPC').value, 'mainnet');
        testnetRPC = normalizeRpcOverride(document.getElementById('testnetRPC').value, 'testnet');
    } catch (error) {
        showToast(`❌ ${error.message}`);
        return;
    }

    walletState.settings = walletState.settings || {};
    if (mainnetRPC) walletState.settings.mainnetRPC = mainnetRPC;
    else delete walletState.settings.mainnetRPC;
    if (testnetRPC) walletState.settings.testnetRPC = testnetRPC;
    else delete walletState.settings.testnetRPC;

    saveWalletState();
    loadSettingsValues();

    rpc.url = getRpcEndpoint();

    if (mainnetRPC || testnetRPC) {
        showToast('✅ Custom RPC transport saved. Bridge routing stays pinned to trusted endpoints, and token or contract metadata is verified against signed manifests.');
    } else {
        showToast('✅ Network settings reset to official endpoints and signed metadata sources.');
    }
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
    const keypair = await LichenCrypto.decryptKeypair(wallet.encryptedKey, oldPassword);
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

        // AUDIT-FIX W-10: Re-encrypt ALL wallets, not just active
        for (let i = 0; i < walletState.wallets.length; i++) {
            const w = walletState.wallets[i];
            try {
                const wKeypair = await LichenCrypto.decryptKeypair(w.encryptedKey, oldPassword);
                if (wKeypair) {
                    w.encryptedKey = await LichenCrypto.encryptKeypair(wKeypair, values.newPassword);
                    zeroBytes(wKeypair.seed);
                }
                if (w.encryptedMnemonic) {
                    const wMnemonic = await LichenCrypto.decryptPrivateKey(w.encryptedMnemonic, oldPassword);
                    if (wMnemonic) w.encryptedMnemonic = await LichenCrypto.encryptPrivateKey(wMnemonic, values.newPassword);
                }
            } catch (reEncryptErr) {
                console.error(`Failed to re-encrypt wallet ${w.id}:`, reEncryptErr);
            }
        }

        saveWalletState();
        showToast('✅ Password changed for all wallets!');
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

            const wipeTarget = walletState.wallets.find(w => w.id === wallet.id);
            wipeSensitiveWalletData(wipeTarget);

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
        document.getElementById('mainnetRPC').placeholder = LICHEN_CONFIG.rpc('mainnet');
        document.getElementById('mainnetRPC').value = settings.mainnetRPC || '';
    }

    if (document.getElementById('testnetRPC')) {
        document.getElementById('testnetRPC').placeholder = LICHEN_CONFIG.rpc('testnet');
        document.getElementById('testnetRPC').value = settings.testnetRPC || '';
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
showSettings = function () {
    _originalShowSettings();
    setTimeout(loadSettingsValues, 100); // Small delay to ensure modal is rendered
};

// ═══════════════════════════════════════════════════════════════════════
// Chain Status Bar — live block height poller
// ═══════════════════════════════════════════════════════════════════════
(function initChainStatusBar() {
    // Claim ownership so the shared/utils.js generic poller yields to us
    window.__chainStatusBarOwned = true;
    const blockEl = document.getElementById('chainBlockHeight');
    const dotEl = document.getElementById('chainDot');
    const latEl = document.getElementById('chainLatency');
    if (!blockEl) return;

    let currentBlock = 0;
    let everConnected = false;

    function isWsHealthy() {
        return Boolean(
            balanceWs &&
            balanceWs.readyState === WebSocket.OPEN &&
            balanceWsSubId !== null
        );
    }

    async function pollBlock() {
        const t0 = performance.now();
        try {
            const slot = await rpc.call('getSlot', []);
            const ms = Math.round(performance.now() - t0);
            if (typeof slot === 'number' && slot > currentBlock) currentBlock = slot;
            blockEl.textContent = 'Block #' + currentBlock.toLocaleString();
            if (latEl) latEl.textContent = ms + ' ms';
            if (dotEl) dotEl.classList.add('connected');
            everConnected = true;
        } catch {
            if (isWsHealthy()) {
                blockEl.textContent = currentBlock > 0
                    ? 'Block #' + currentBlock.toLocaleString() + ' (WS live)'
                    : 'Connected (WS live)';
                if (latEl) latEl.textContent = '';
                if (dotEl) dotEl.classList.add('connected');
            } else {
                blockEl.textContent = everConnected ? 'Reconnecting\u2026' : 'Connecting\u2026';
                if (latEl) latEl.textContent = '';
                if (dotEl) dotEl.classList.remove('connected');
            }
        }
    }

    pollBlock();
    setInterval(pollBlock, 5000);
})();
