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
let currentNetwork = localStorage.getItem(NETWORK_STORAGE_KEY) || 'testnet';
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
        this.reconnectTimer = null;
        this.keepaliveTimer = null;
        this.nextId = 1;
        this.pending = new Map();
        this.subscriptions = new Map();
        this.desired = [];
        this.openHandlers = [];
        this.closeHandlers = [];
        this._closing = false;
    }
    
    connect() {
        if (this._closing) return;
        if (this.ws && (this.ws.readyState === WebSocket.OPEN || this.ws.readyState === WebSocket.CONNECTING)) {
            return;
        }

        if (this.reconnectTimer) {
            clearTimeout(this.reconnectTimer);
            this.reconnectTimer = null;
        }

        try {
            this.ws = new WebSocket(this.url);

            this.ws.onopen = () => {
                this.reconnectDelay = 1000;
                this.subscriptions.clear();
                this.resubscribeAll();
                this.openHandlers.forEach((handler) => handler());
                // Client-side keepalive ping every 25s
                if (this.keepaliveTimer) clearInterval(this.keepaliveTimer);
                this.keepaliveTimer = setInterval(() => {
                    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
                        this.ws.send(JSON.stringify({ method: 'ping' }));
                    }
                }, 25000);
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
                this.ws = null;
                if (this.keepaliveTimer) { clearInterval(this.keepaliveTimer); this.keepaliveTimer = null; }
                this.closeHandlers.forEach((handler) => handler());
                if (this._closing) return;
                this.reconnectTimer = setTimeout(() => {
                    this.reconnectTimer = null;
                    this.connect();
                }, this.reconnectDelay);
                this.reconnectDelay = Math.min(this.reconnectDelay * 2, 30000);
            };
        } catch (error) {
            console.error('WebSocket connection failed:', error);
            if (!this._closing) {
                this.reconnectTimer = setTimeout(() => {
                    this.reconnectTimer = null;
                    this.connect();
                }, this.reconnectDelay);
            }
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

    close() {
        this._closing = true;
        if (this.reconnectTimer) {
            clearTimeout(this.reconnectTimer);
            this.reconnectTimer = null;
        }
        if (this.ws) {
            this.ws.onclose = null;
            this.ws.close();
            this.ws = null;
        }
        this.pending.forEach(({ reject }) => reject(new Error('WebSocket closed')));
        this.pending.clear();
        this.subscriptions.clear();
        this.desired = [];
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
    if (ws && typeof ws.close === 'function') {
        ws.close();
    }
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

    const encoded = encodeURIComponent(value);

    if (/^\d+$/.test(value)) {
        window.location.href = `block.html?slot=${encoded}`;
        return;
    }
    if (/^[0-9a-fA-F]{64}$/.test(value)) {
        window.location.href = `transaction.html?sig=${encoded}`;
        return;
    }
    const isAddressLike = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(value) || /^0x[0-9a-fA-F]{40}$/i.test(value);
    if (isAddressLike) {
        try {
            const contractInfo = await rpc.getContractInfo(value);
            if (contractInfo && contractInfo.is_executable === true) {
                window.location.href = `contract.html?address=${encoded}`;
                return;
            }
        } catch (_) { /* fallback below */ }

        window.location.href = `address.html?address=${encoded}`;
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
                    window.location.href = `address.html?address=${encodeURIComponent(owner)}`;
                    return;
                }
            } catch (_) { /* fallback below */ }
        }
    }

    try {
        const symbol = await rpc.call('getSymbolRegistry', [value.toUpperCase()]);
        if (symbol && symbol.program) {
            window.location.href = `contract.html?address=${encodeURIComponent(symbol.program)}`;
            return;
        }
    } catch (_) { /* fallback below */ }

    window.location.href = `address.html?address=${encoded}`;
}

window.navigateExplorerSearch = navigateExplorerSearch;

// Utility functions are in utils.js (loaded before explorer.js).
// NETWORKS, SYSTEM_PROGRAM_ID, MoltChainRPC, MoltChainWS stay here.

// Dashboard Updates
async function updateDashboardStats() {
    // Only run on dashboard page (index.html)
    if (!document.getElementById('latestBlock')) return;
    const chainStatusTopEl = document.getElementById('chainStatusTop');
    
    try {
        // Get latest block/slot
        const slot = await rpc.getSlot();
        if (slot !== null) {
            const latestBlockEl = document.getElementById('latestBlock');
            if (latestBlockEl) {
                latestBlockEl.textContent = formatSlot(slot);
            }
            
            // Chain is online if we got a response
            if (chainStatusTopEl) {
                chainStatusTopEl.textContent = 'Online';
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
            // Wire burn percentage and slot duration from API (no hardcoding)
            if (metrics.fee_burn_percent !== undefined) {
                const burnEl = document.getElementById('burnPctLabel');
                if (burnEl) burnEl.textContent = metrics.fee_burn_percent;
            }
            if (metrics.slot_duration_ms !== undefined) {
                const slotEl = document.getElementById('slotTimeLabel');
                if (slotEl) slotEl.textContent = metrics.slot_duration_ms;
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
                const totalStakeMOLT = totalStake / SHELLS_PER_MOLT;
                totalStakeEl.textContent = formatNumber(Math.floor(totalStakeMOLT)) + ' MOLT';
            }
        }
        
        // Get latest blocks
        await updateLatestBlocks();
        await updateShieldedOverview();
        
    } catch (error) {
        console.error('Dashboard update failed:', error);
        
        // Chain is offline — reset all metrics so stale data doesn't persist
        if (chainStatusTopEl) {
            chainStatusTopEl.textContent = 'Offline';
        }
        const resetMap = {
            latestBlock: '—', tpsValue: '0', totalTxs: '—', txsToday: '',
            activeAccounts: '—', accountBreakdown: '', totalBurned: '—',
            validatorCount: '0', activeValidators: '0', totalStake: '—',
            shieldedBalance: '0 MOLT', shieldedBalanceShells: '0 shells', commitmentCount: '0',
            nullifierCount: '0', shieldedTxCount: '0',
            shieldedTxBreakdown: 'Shield: 0 | Unshield: 0 | Transfer: 0', merkleRoot: '0x0',
            burnPctLabel: '—', slotTimeLabel: '—'
        };
        for (const [id, val] of Object.entries(resetMap)) {
            const el = document.getElementById(id);
            if (el) el.textContent = val;
        }
    }
}

async function updateShieldedOverview() {
    if (!document.getElementById('shieldedBalance')) return;

    try {
        const stats = await rpc.call('getShieldedPoolState');
        const pick = (...vals) => vals.find(v => v !== undefined && v !== null);

        const totalShielded = pick(stats?.totalShielded, stats?.pool_balance, 0);
        const balanceMolt = pick(stats?.totalShieldedMolt, stats?.pool_balance_molt, (totalShielded / SHELLS_PER_MOLT));
        const commitmentCount = pick(stats?.commitmentCount, stats?.commitment_count, 0);
        const nullifierCount = pick(stats?.nullifierCount, stats?.nullifier_count, 0);
        const shieldCount = pick(stats?.shieldCount, stats?.shield_count, 0);
        const unshieldCount = pick(stats?.unshieldCount, stats?.unshield_count, 0);
        const transferCount = pick(stats?.transferCount, stats?.transfer_count, 0);
        const txCount = shieldCount + unshieldCount + transferCount;
        const merkleRoot = pick(stats?.merkleRoot, stats?.merkle_root, '0'.repeat(64));

        const shieldedBalanceEl = document.getElementById('shieldedBalance');
        const shieldedBalanceShellsEl = document.getElementById('shieldedBalanceShells');
        const commitmentCountEl = document.getElementById('commitmentCount');
        const nullifierCountEl = document.getElementById('nullifierCount');
        const shieldedTxCountEl = document.getElementById('shieldedTxCount');
        const shieldedTxBreakdownEl = document.getElementById('shieldedTxBreakdown');
        const merkleRootEl = document.getElementById('merkleRoot');

        if (shieldedBalanceEl) {
            const balance = Number(balanceMolt) || 0;
            shieldedBalanceEl.textContent = balance.toLocaleString(undefined, {
                minimumFractionDigits: 2,
                maximumFractionDigits: 4,
            }) + ' MOLT';
        }
        if (shieldedBalanceShellsEl) shieldedBalanceShellsEl.textContent = formatNumber(totalShielded) + ' shells';
        if (commitmentCountEl) commitmentCountEl.textContent = formatNumber(commitmentCount);
        if (nullifierCountEl) nullifierCountEl.textContent = formatNumber(nullifierCount);
        if (shieldedTxCountEl) shieldedTxCountEl.textContent = formatNumber(txCount);
        if (shieldedTxBreakdownEl) {
            shieldedTxBreakdownEl.textContent = `Shield: ${formatNumber(shieldCount)} | Unshield: ${formatNumber(unshieldCount)} | Transfer: ${formatNumber(transferCount)}`;
        }
        if (merkleRootEl) {
            const value = String(merkleRoot || '0').replace(/^0x/, '');
            merkleRootEl.textContent = '0x' + (value.length > 16 ? `${value.slice(0, 8)}...${value.slice(-8)}` : value);
        }
    } catch (_) {
        // Ignore shielded metric errors on non-ZK networks
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
                <span class="hash-short" title="${escapeExplorerHtml(block.hash)}">${formatHash(block.hash)}</span>
                    <i class="fas fa-copy copy-hash" data-copy="${escapeExplorerHtml(block.hash)}" onclick="safeCopy(this)" title="Copy hash"></i>
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
            const pillClass = getTransactionPillClass(type);
            const amountShells = tx.amount_shells || (tx.amount !== undefined ? Math.round(tx.amount * SHELLS_PER_MOLT) : 0);
            const amountDisplay = amountShells ? formatMolt(amountShells) : '-';
            const timestamp = tx.timestamp || 0;
            
            return `
            <tr>
                <td>
                    <a href="transaction.html?sig=${encodeURIComponent(signature)}" title="${escapeExplorerHtml(signature)}">${formatHash(signature)}</a>
                    <i class="fas fa-copy copy-hash" data-copy="${escapeExplorerHtml(signature)}" onclick="safeCopy(this)" title="Copy signature"></i>
                </td>
                <td><span class="pill pill-${escapeExplorerHtml(pillClass)}">${escapeExplorerHtml(type)}</span></td>
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

function getTransactionPillClass(type) {
    const normalized = String(type || '').trim().toLowerCase();

    const mapped = {
        transfer: 'transfer',
        shield: 'shield',
        unshield: 'unshield',
        shieldedtransfer: 'shieldedtransfer',
        'shielded transfer': 'shieldedtransfer',
        contract: 'contract',
        'contract deploy': 'contractdeploy',
        deploy: 'contractdeploy',
        'contract call': 'contractcall',
        call: 'contractcall',
        mint: 'mint',
        burn: 'burn',
        stake: 'stake',
        unstake: 'unstake',
    };

    if (mapped[normalized]) {
        return mapped[normalized];
    }

    return normalized.replace(/[^a-z0-9]/g, '') || 'transfer';
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
    // Poll REST as a continuous background regardless of WS — ensures
    // the dashboard always shows fresh data even during chain stalls
    let backgroundPolling = null;
    const WS_STALE_THRESHOLD = 6000; // 6 seconds (reduced from 10s for faster detection)

    const startPolling = () => {
        if (dashboardPolling || !isDashboard) return;
        dashboardPolling = setInterval(() => {
            updateDashboardStats();
            updateLatestTransactions();
        }, 3000); // Poll every 3s when WS is down (was 5s)
    };

    const stopPolling = () => {
        if (dashboardPolling) {
            clearInterval(dashboardPolling);
            dashboardPolling = null;
        }
    };

    // Background REST poll: always runs at low frequency to ensure data freshness
    // even when WS appears connected but chain is stalled
    const startBackgroundPolling = () => {
        if (backgroundPolling || !isDashboard) return;
        backgroundPolling = setInterval(() => {
            updateDashboardStats();
        }, 10000); // Every 10s as a safety net
    };
    startBackgroundPolling();

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
                    try { if (ws && ws.ws) ws.ws.close(); } catch (_) {}
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
        // Subscribe once — resubscribeAll() in the WS class re-sends
        // on every reconnect automatically.  Calling subscribe() inside
        // onOpen would push a duplicate to `desired` on every reconnect,
        // eventually exceeding the server's per-connection subscription limit.
        ws.subscribe('subscribeBlocks', () => {
            lastWsBlockTime = Date.now();
            updateLatestBlocks();
            updateLatestTransactions();
            updateDashboardStats();
        });

        ws.onOpen(() => {
            stopPolling();
            lastWsBlockTime = Date.now();
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
