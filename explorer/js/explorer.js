// Reef Explorer - MoltChain Blockchain Explorer
// Real-time blockchain data with RPC

const NETWORKS = {
    mainnet: {
        rpc: 'https://rpc.moltchain.network',
        ws: null,
    },
    testnet: {
        rpc: 'https://testnet-rpc.moltchain.network',
        ws: null,
    },
    'local-testnet': {
        rpc: 'http://localhost:8899',
        ws: 'ws://localhost:8900',
    },
    'local-mainnet': {
        rpc: 'http://localhost:9899',
        ws: 'ws://localhost:9900',
    }
};

const NETWORK_STORAGE_KEY = 'explorer_network';
let currentNetwork = localStorage.getItem(NETWORK_STORAGE_KEY) || 'mainnet';
currentNetwork = resolveNetwork(currentNetwork);

function resolveNetwork(name) {
    if (name === 'local') {
        return 'local-testnet';
    }
    return NETWORKS[name] ? name : 'mainnet';
}

function getNetworkConfig(name) {
    const resolved = resolveNetwork(name);
    return NETWORKS[resolved];
}

let RPC_URL = getNetworkConfig(currentNetwork).rpc;
let WS_URL = getNetworkConfig(currentNetwork).ws;
const SYSTEM_PROGRAM_ID = '11111111111111111111111111111111';

// RPC Client (from actual MoltChain RPC implementation)
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
                // Only log unexpected errors (not "Block not found" which is normal)
                if (data.error.code !== -32001) {
                    console.error('RPC Error:', data.error);
                }
                return null;
            }
            return data.result;
        } catch (error) {
            console.error('RPC Call Failed:', error);
            return null;
        }
    }
    
    // Account Operations (from rpc/src/lib.rs)
    async getBalance(pubkey) { return this.call('getBalance', [pubkey]); }
    async getAccount(pubkey) { return this.call('getAccount', [pubkey]); }
    
    // Block Operations
    async getBlock(slot) { return this.call('getBlock', [slot]); }
    async getLatestBlock() { return this.call('getLatestBlock'); }
    async getSlot() { return this.call('getSlot'); }
    
    // Transaction Operations
    async getTransaction(signature) { return this.call('getTransaction', [signature]); }
    async sendTransaction(txData) { return this.call('sendTransaction', [txData]); }
    
    // Chain Statistics
    async getTotalBurned() { return this.call('getTotalBurned'); }
    async getValidators() { return this.call('getValidators'); }
    async getMetrics() { return this.call('getMetrics'); }
    async health() { return this.call('health'); }

    // Address & History
    async getTransactionsByAddress(pubkey, options = {}) { return this.call('getTransactionsByAddress', [pubkey, options]); }
    async getAccountTxCount(pubkey) { return this.call('getAccountTxCount', [pubkey]); }
    async getAccountInfo(pubkey) { return this.call('getAccountInfo', [pubkey]); }
    async getTransactionHistory(pubkey, options = {}) { return this.call('getTransactionHistory', [pubkey, options]); }

    // Contract / Program
    async getContractInfo(contractId) { return this.call('getContractInfo', [contractId]); }
    async getContractAbi(contractId) { return this.call('getContractAbi', [contractId]); }
    async getContractLogs(contractId, limit = 100) { return this.call('getContractLogs', [contractId, limit]); }
    async getAllContracts() { return this.call('getAllContracts'); }
    async getProgram(programId) { return this.call('getProgram', [programId]); }
    async getProgramStats(programId) { return this.call('getProgramStats', [programId]); }
    async getSymbolRegistryByProgram(programId) { return this.call('getSymbolRegistryByProgram', [programId]); }

    // Token
    async getTokenBalance(tokenMint, owner) { return this.call('getTokenBalance', [tokenMint, owner]); }
    async getTokenHolders(tokenMint, limit = 100) { return this.call('getTokenHolders', [tokenMint, limit]); }
    async getTokenTransfers(tokenMint, limit = 100) { return this.call('getTokenTransfers', [tokenMint, limit]); }
    async getContractEvents(contractId, limit = 100) { return this.call('getContractEvents', [contractId, limit]); }

    // NFT
    async getCollection(collectionId) { return this.call('getCollection', [collectionId]); }
    async getNFT(tokenId) { return this.call('getNFT', [tokenId]); }
    async getNFTsByOwner(owner) { return this.call('getNFTsByOwner', [owner]); }
    async getMarketListings(options = {}) { return this.call('getMarketListings', [options]); }
    async getMarketSales(options = {}) { return this.call('getMarketSales', [options]); }

    // Simulation
    async simulateTransaction(txBase64) { return this.call('simulateTransaction', [txBase64]); }

    // Staking
    async getStakingStatus(pubkey) { return this.call('getStakingStatus', [pubkey]); }
    async getReefStakePoolInfo() { return this.call('getReefStakePoolInfo'); }
}

let rpc = new MoltChainRPC(RPC_URL);

// WebSocket Client (for real-time updates)
class MoltChainWS {
    constructor(url) {
        this.url = url;
        this.ws = null;
        this.reconnectDelay = 1000;
        this.nextId = 1;
        this.pending = new Map();
        this.subscriptions = new Map();
        this.desired = [];
        this.openHandlers = [];
        this.closeHandlers = [];
    }
    
    connect() {
        if (this.ws && (this.ws.readyState === WebSocket.OPEN || this.ws.readyState === WebSocket.CONNECTING)) {
            return;
        }

        // Clear any existing keepalive interval
        if (this._keepalive) { clearInterval(this._keepalive); this._keepalive = null; }

        try {
            this.ws = new WebSocket(this.url);

            this.ws.onopen = () => {
                // console.log('WebSocket connected');
                this.reconnectDelay = 1000;
                this.subscriptions.clear();
                this.resubscribeAll();
                this.openHandlers.forEach((handler) => handler());
                // Client-side keepalive: send a tiny message every 20s
                // to prevent idle-timeout disconnects
                this._keepalive = setInterval(() => {
                    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
                        try { this.ws.send('{"method":"ping"}'); } catch(_) {}
                    }
                }, 20000);
            };

            this.ws.onmessage = (event) => {
                let msg = null;
                try {
                    msg = JSON.parse(event.data);
                } catch (error) {
                    console.error('WebSocket message parse failed:', error);
                    return;
                }

                if (msg && msg.method === 'subscription' && msg.params) {
                    const subscriptionId = msg.params.subscription;
                    const handler = this.subscriptions.get(subscriptionId);
                    if (handler) {
                        handler(msg.params.result);
                    }
                    return;
                }

                if (msg && msg.id && this.pending.has(msg.id)) {
                    const { resolve, reject } = this.pending.get(msg.id);
                    this.pending.delete(msg.id);
                    if (msg.error) {
                        reject(new Error(msg.error.message || 'WebSocket error'));
                    } else {
                        resolve(msg.result);
                    }
                }
            };

            this.ws.onerror = (error) => {
                console.error('WebSocket error:', error);
            };

            this.ws.onclose = () => {
                // console.log('WebSocket closed, reconnecting...');
                if (this._keepalive) { clearInterval(this._keepalive); this._keepalive = null; }
                this.closeHandlers.forEach((handler) => handler());
                setTimeout(() => this.connect(), this.reconnectDelay);
                this.reconnectDelay = Math.min(this.reconnectDelay * 1.5, 5000);
            };
        } catch (error) {
            console.error('WebSocket connection failed:', error);
            setTimeout(() => this.connect(), this.reconnectDelay);
        }
    }
    
    isConnected() {
        return this.ws && this.ws.readyState === WebSocket.OPEN;
    }

    onOpen(handler) {
        this.openHandlers.push(handler);
    }

    onClose(handler) {
        this.closeHandlers.push(handler);
    }

    sendRpc(method, params = null) {
        if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
            return Promise.reject(new Error('WebSocket not connected'));
        }

        const id = this.nextId++;
        const payload = {
            jsonrpc: '2.0',
            id,
            method,
        };

        if (params !== null) {
            payload.params = params;
        }

        return new Promise((resolve, reject) => {
            this.pending.set(id, { resolve, reject });
            this.ws.send(JSON.stringify(payload));
        });
    }

    resubscribeAll() {
        this.desired.forEach((entry) => {
            this.sendSubscribe(entry).catch((error) => {
                console.error('WebSocket resubscribe failed:', error);
            });
        });
    }

    sendSubscribe(entry) {
        return this.sendRpc(entry.method, entry.params).then((subId) => {
            entry.subId = subId;
            this.subscriptions.set(subId, entry.callback);
            return subId;
        });
    }

    subscribe(method, callback, params = null) {
        const entry = { method, params, callback, subId: null };
        this.desired.push(entry);

        if (this.isConnected()) {
            return this.sendSubscribe(entry);
        }

        return Promise.resolve(null);
    }
}

let ws;
if (WS_URL) {
    ws = new MoltChainWS(WS_URL);
}

function getExplorerRpcUrl() {
    return RPC_URL;
}

function getExplorerNetwork() {
    return currentNetwork;
}

function setExplorerNetwork(name, options = {}) {
    const { reload = true } = options;
    currentNetwork = resolveNetwork(name);
    localStorage.setItem(NETWORK_STORAGE_KEY, currentNetwork);

    const config = getNetworkConfig(currentNetwork);
    RPC_URL = config.rpc;
    WS_URL = config.ws;
    rpc = new MoltChainRPC(RPC_URL);
    ws = WS_URL ? new MoltChainWS(WS_URL) : undefined;

    if (reload) {
        window.location.reload();
        return;
    }

    window.dispatchEvent(new CustomEvent('explorer:network-changed', {
        detail: { network: currentNetwork }
    }));
}

function initExplorerNetworkSelector() {
    const select = document.getElementById('explorerNetworkSelect');
    if (!select) return;
    select.value = currentNetwork;
    select.addEventListener('change', () => {
        setExplorerNetwork(select.value);
    });
}

window.getExplorerRpcUrl = getExplorerRpcUrl;
window.getExplorerNetwork = getExplorerNetwork;
window.setExplorerNetwork = setExplorerNetwork;
window.initExplorerNetworkSelector = initExplorerNetworkSelector;

const moltNameCache = new Map();

function escapeExplorerHtml(value) {
    return String(value)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/\"/g, '&quot;')
        .replace(/'/g, '&#039;');
}

function isLikelyMoltAddress(value) {
    if (!value || typeof value !== 'string') return false;
    return /^[1-9A-HJ-NP-Za-km-z]{32,64}$/.test(value);
}

async function resolveMoltNameForAddress(address) {
    if (!isLikelyMoltAddress(address)) return null;
    if (moltNameCache.has(address)) return moltNameCache.get(address);

    let resolved = null;
    try {
        const result = await rpc.call('reverseMoltName', [address]);
        if (typeof result === 'string' && result) {
            resolved = result;
        } else if (result && typeof result.name === 'string' && result.name) {
            resolved = result.name;
        }
    } catch (_) {
        resolved = null;
    }
    moltNameCache.set(address, resolved);
    return resolved;
}

async function batchResolveMoltNames(addresses) {
    const unique = [...new Set((addresses || []).filter(isLikelyMoltAddress))];
    const unresolved = unique.filter(address => !moltNameCache.has(address));

    if (unresolved.length > 0) {
        let batchMap = null;
        try {
            batchMap = await rpc.call('batchReverseMoltNames', [unresolved]);
        } catch (_) {
            batchMap = null;
        }

        if (batchMap && typeof batchMap === 'object') {
            unresolved.forEach(address => {
                const value = batchMap[address];
                if (typeof value === 'string' && value) {
                    moltNameCache.set(address, value);
                } else if (value && typeof value.name === 'string' && value.name) {
                    moltNameCache.set(address, value.name);
                } else {
                    moltNameCache.set(address, null);
                }
            });
        } else {
            await Promise.all(unresolved.map(async (address) => {
                await resolveMoltNameForAddress(address);
            }));
        }
    }

    const result = {};
    unique.forEach(address => {
        result[address] = moltNameCache.get(address) || null;
    });
    return result;
}

function formatAddressWithMoltName(address, name, options = {}) {
    const { includeAddressInLabel = false } = options;
    if (!address) return 'N/A';

    const addr = String(address);
    const shortAddress = formatHash(addr, 6);

    if (name && typeof name === 'string') {
        const safeName = escapeExplorerHtml(name.endsWith('.molt') ? name : `${name}.molt`);
        if (includeAddressInLabel) {
            return `<span title="${escapeExplorerHtml(addr)}">${safeName} (${escapeExplorerHtml(shortAddress)})</span>`;
        }
        return `<span title="${escapeExplorerHtml(addr)}">${safeName}</span>`;
    }
    return `<span title="${escapeExplorerHtml(addr)}">${escapeExplorerHtml(shortAddress)}</span>`;
}

window.resolveMoltNameForAddress = resolveMoltNameForAddress;
window.batchResolveMoltNames = batchResolveMoltNames;
window.formatAddressWithMoltName = formatAddressWithMoltName;
window.isLikelyMoltAddress = isLikelyMoltAddress;

async function navigateExplorerSearch(query) {
    const value = String(query || '').trim();
    if (!value) return;

    if (/^\d+$/.test(value)) {
        window.location.href = `block.html?slot=${value}`;
        return;
    }
    if (/^[0-9a-fA-F]{64}$/.test(value)) {
        window.location.href = `transaction.html?sig=${value}`;
        return;
    }
    if (/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(value) || /^0x[0-9a-fA-F]{40}$/i.test(value)) {
        window.location.href = `address.html?address=${value}`;
        return;
    }

    const lower = value.toLowerCase();
    if (lower.endsWith('.molt')) {
        const label = lower.slice(0, -5);
        if (label.length > 0) {
            try {
                const resolved = await rpc.call('resolveMoltName', [label]);
                const owner = resolved?.owner || resolved?.address || null;
                if (owner) {
                    window.location.href = `address.html?address=${owner}`;
                    return;
                }
            } catch (_) { /* fallback below */ }
        }
    }

    try {
        const symbol = await rpc.call('getSymbolRegistry', [value.toUpperCase()]);
        if (symbol && symbol.program) {
            window.location.href = `address.html?address=${symbol.program}`;
            return;
        }
    } catch (_) { /* fallback below */ }

    window.location.href = `address.html?address=${value}`;
}

window.navigateExplorerSearch = navigateExplorerSearch;

// Utility functions are in utils.js (loaded before explorer.js).
// NETWORKS, SYSTEM_PROGRAM_ID, MoltChainRPC, MoltChainWS stay here.

// Dashboard Updates
async function updateDashboardStats() {
    // Only run on dashboard page (index.html)
    if (!document.getElementById('latestBlock')) return;
    const chainStatusEl = document.getElementById('chainStatus');
    
    try {
        // Get latest block/slot
        const slot = await rpc.getSlot();
        if (slot !== null) {
            const latestBlockEl = document.getElementById('latestBlock');
            if (latestBlockEl) {
                latestBlockEl.textContent = formatSlot(slot);
            }
            
            // Chain is online if we got a response
            if (chainStatusEl) {
                chainStatusEl.className = 'stat-box-value status-online';
                chainStatusEl.innerHTML = '<span class="status-dot"></span> Online';
            }
        }
        
        // Get metrics
        const metrics = await rpc.getMetrics();
        if (metrics) {
            if (metrics.tps !== undefined) {
                const tpsEl = document.getElementById('tpsValue');
                if (tpsEl) tpsEl.textContent = formatNumber(Math.floor(metrics.tps));
                const peakEl = document.getElementById('peakTps');
                if (peakEl && metrics.peak_tps !== undefined) {
                    peakEl.textContent = metrics.peak_tps.toFixed(1);
                }
            }
            if (metrics.total_transactions !== undefined) {
                const totalTxsEl = document.getElementById('totalTxs');
                if (totalTxsEl) totalTxsEl.textContent = formatNumber(metrics.total_transactions);

                const txsTodayEl = document.getElementById('txsToday');
                if (txsTodayEl) {
                    // Use server-side daily counter (same for all visitors)
                    const dailyTxs = metrics.daily_transactions !== undefined
                        ? metrics.daily_transactions
                        : 0;
                    txsTodayEl.textContent = `+${formatNumber(dailyTxs)} today`;
                }
            }
            if (metrics.total_accounts !== undefined) {
                const activeAccountsEl = document.getElementById('activeAccounts');
                const totalContracts = metrics.total_contracts || 0;
                const totalAll = (metrics.total_accounts || 0) + totalContracts;
                if (activeAccountsEl) activeAccountsEl.textContent = formatNumber(totalAll);
                const breakdownEl = document.getElementById('accountBreakdown');
                if (breakdownEl) breakdownEl.textContent = `${formatNumber(metrics.active_accounts || 0)} funded · ${formatNumber(totalContracts)} contracts`;
            }
        }
        
        // Get total burned
        const burned = await rpc.getTotalBurned();
        if (burned && burned.molt !== undefined) {
            const totalBurnedEl = document.getElementById('totalBurned');
            if (totalBurnedEl) totalBurnedEl.textContent = burned.molt.toFixed(4) + ' MOLT';
        }
        
        // Get validators
        const validatorsResult = await rpc.getValidators();
        if (validatorsResult) {
            const validators = validatorsResult.validators || [];
            const onlineCount = slot !== null
                ? validators.filter((validator) => {
                    const lastActive = validator.last_active_slot || validator.lastActiveSlot || 0;
                    return slot - lastActive <= 100;
                }).length
                : validators.length;

            // Top metric: "Validators ... Online now" — use online count
            const validatorCountEl = document.getElementById('validatorCount');
            if (validatorCountEl) validatorCountEl.textContent = onlineCount;
            
            // Bottom metric: "Active Validators" — same online count
            const activeValidatorsEl = document.getElementById('activeValidators');
            if (activeValidatorsEl) activeValidatorsEl.textContent = onlineCount;
            
            // Calculate total stake from all validators
            const totalStakeEl = document.getElementById('totalStake');
            if (totalStakeEl && validatorsResult.validators) {
                const totalStake = validatorsResult.validators.reduce((sum, v) => {
                    return sum + (v.stake || 0);
                }, 0);
                // Convert shells to MOLT (1 MOLT = 1B shells)
                const totalStakeMOLT = totalStake / 1_000_000_000;
                totalStakeEl.textContent = formatNumber(Math.floor(totalStakeMOLT)) + ' MOLT';
            }
        }
        
        // Get latest blocks
        await updateLatestBlocks();
        
    } catch (error) {
        console.error('Dashboard update failed:', error);
        
        // Chain is offline — reset all metrics so stale data doesn't persist
        if (chainStatusEl) {
            chainStatusEl.className = 'stat-box-value status-offline';
            chainStatusEl.innerHTML = '<span class="status-dot"></span> Offline';
        }
        const resetMap = {
            latestBlock: '—', tpsValue: '0', totalTxs: '—', txsToday: '',
            activeAccounts: '—', accountBreakdown: '', totalBurned: '—',
            validatorCount: '0', activeValidators: '0', totalStake: '—'
        };
        for (const [id, val] of Object.entries(resetMap)) {
            const el = document.getElementById(id);
            if (el) el.textContent = val;
        }
    }
}

async function updateLatestBlocks() {
    const blocksTable = document.getElementById('blocksTable');
    if (!blocksTable) return;
    
    try {
        const latestBlock = await rpc.getLatestBlock();
        if (!latestBlock) {
            blocksTable.innerHTML = '<tr><td colspan="5" style="text-align:center; color: var(--text-muted);">No blocks found</td></tr>';
            return;
        }
        
        // Get last 10 blocks in parallel
        const blocks = [latestBlock];
        const currentSlot = latestBlock.slot;
        
        const slotsToFetch = [];
        for (let i = 1; i < 10 && (currentSlot - i) >= 0; i++) {
            slotsToFetch.push(currentSlot - i);
        }
        const fetched = await Promise.all(
            slotsToFetch.map(s => rpc.call('getBlock', [s]).catch(() => null))
        );
        fetched.forEach(b => { if (b) blocks.push(b); });
        
        // Render blocks
        blocksTable.innerHTML = blocks.map(block => `
            <tr>
                <td><a href="block.html?slot=${block.slot}">#${formatSlot(block.slot)}</a></td>
                <td>
                <span class="hash-short" title="${block.hash}">${formatHash(block.hash)}</span>
                    <i class="fas fa-copy copy-hash" onclick="copyToClipboard('${block.hash}')" title="Copy hash"></i>
                </td>
                <td><span class="pill pill-info">${block.transaction_count || 0} txs</span></td>
                <td>${formatValidator(block.validator)}</td>
                <td>${formatTime(block.timestamp)}</td>
            </tr>
        `).join('');
        
    } catch (error) {
        console.error('Failed to update blocks:', error);
        blocksTable.innerHTML = '<tr><td colspan="5" style="text-align:center; color: #FF6B6B;">Failed to load blocks</td></tr>';
    }
}

async function updateLatestTransactions() {
    const txsTable = document.getElementById('txsTable');
    if (!txsTable) return;
    
    try {
        const result = await rpc.call('getRecentTransactions', [{ limit: 10 }]);
        const txs = result?.transactions || [];

        if (txs.length === 0) {
            txsTable.innerHTML = '<tr><td colspan="5" style="text-align:center; color: var(--text-muted);">No transactions found</td></tr>';
            return;
        }

        txsTable.innerHTML = txs.map(tx => {
            const signature = tx.hash || tx.signature || 'unknown';
            const type = tx.type || 'Transfer';
            const amountShells = tx.amount_shells || (tx.amount !== undefined ? Math.round(tx.amount * 1_000_000_000) : 0);
            const amountDisplay = amountShells ? formatMolt(amountShells) : '-';
            const timestamp = tx.timestamp || 0;
            
            return `
            <tr>
                <td>
                    <a href="transaction.html?sig=${signature}" title="${signature}">${formatHash(signature)}</a>
                    <i class="fas fa-copy copy-hash" onclick="copyToClipboard('${signature}')" title="Copy signature"></i>
                </td>
                <td><span class="pill pill-${type.toLowerCase()}">${type}</span></td>
                <td><span class="pill pill-success"><i class="fas fa-check"></i> Success</span></td>
                <td><span style="font-family: 'JetBrains Mono', monospace; font-weight: 600;">${amountDisplay}</span></td>
                <td>${formatTime(timestamp)}</td>
            </tr>
        `}).join('');
        
    } catch (error) {
        console.error('Failed to update transactions:', error);
        txsTable.innerHTML = '<tr><td colspan="5" style="text-align:center; color: #FF6B6B;">Failed to load transactions</td></tr>';
    }
}

// resolveTxType is in utils.js (loaded before explorer.js)

// Search Functionality
function setupSearch() {
    const searchInput = document.getElementById('searchInput');
    if (!searchInput) return;
    
    searchInput.addEventListener('keypress', async (e) => {
        if (e.key === 'Enter') {
            const query = searchInput.value.trim();
            if (!query) return;
            await navigateExplorerSearch(query);
        }
    });
}

// Initialize Dashboard
document.addEventListener('DOMContentLoaded', () => {
    // console.log('🦞 Reef Explorer loaded');

    initExplorerNetworkSelector();
    
    // Only run dashboard-specific updates on index.html
    const isDashboard = !!document.getElementById('latestBlock');
    if (isDashboard) {
        updateDashboardStats();
        updateLatestTransactions();
    }
    
    // Setup search
    setupSearch();
    
    let dashboardPolling = null;
    let lastWsBlockTime = 0;
    let staleCheckInterval = null;
    const WS_STALE_THRESHOLD = 10000; // 10 seconds

    const startPolling = () => {
        if (dashboardPolling || !isDashboard) return;
        dashboardPolling = setInterval(() => {
            updateDashboardStats();
            updateLatestTransactions();
        }, 5000);
    };

    const stopPolling = () => {
        if (dashboardPolling) {
            clearInterval(dashboardPolling);
            dashboardPolling = null;
        }
    };

    // Stale data detector: if WS is "connected" but no block event
    // has arrived in WS_STALE_THRESHOLD ms, force a REST poll and
    // close + reconnect the WebSocket to re-establish subscriptions.
    const startStaleCheck = () => {
        if (staleCheckInterval || !isDashboard) return;
        staleCheckInterval = setInterval(() => {
            if (typeof ws !== 'undefined' && ws.isConnected() && lastWsBlockTime > 0) {
                const elapsed = Date.now() - lastWsBlockTime;
                if (elapsed > WS_STALE_THRESHOLD) {
                    console.warn(`WebSocket stale: no block event for ${Math.round(elapsed / 1000)}s — forcing REST poll and WS reconnect`);
                    // Immediate REST fallback so the UI stays fresh
                    updateDashboardStats();
                    updateLatestTransactions();
                    // Force-close so onclose fires and triggers reconnect + resubscribe
                    try { ws.ws.close(); } catch (_) {}
                }
            }
        }, WS_STALE_THRESHOLD);
    };

    const stopStaleCheck = () => {
        if (staleCheckInterval) {
            clearInterval(staleCheckInterval);
            staleCheckInterval = null;
        }
    };

    if (typeof ws !== 'undefined') {
        ws.onOpen(() => {
            stopPolling();
            lastWsBlockTime = Date.now();
            ws.subscribe('subscribeBlocks', () => {
                lastWsBlockTime = Date.now();
                updateLatestBlocks();
                updateLatestTransactions();
                updateDashboardStats();
            });
            startStaleCheck();
        });

        ws.onClose(() => {
            stopStaleCheck();
            startPolling();
        });

        ws.connect();
        setTimeout(() => {
            if (!ws.isConnected()) {
                startPolling();
            }
        }, 2000);
    } else {
        startPolling();
    }
    
    // Mobile nav toggle
    const navToggle = document.getElementById('navToggle');
    const navMenu = document.querySelector('.nav-menu');
    if (navToggle && navMenu) {
        navToggle.addEventListener('click', () => {
            navMenu.classList.toggle('active');
            navToggle.classList.toggle('active');
        });
    }
});

// Toast animation CSS is in utils.js
