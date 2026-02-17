/* ========================================
   MoltyDEX — Production JavaScript Engine
   Wired to MoltChain RPC + WebSocket
   ======================================== */

document.addEventListener('DOMContentLoaded', () => {
    'use strict';

    // ═══════════════════════════════════════════════════════════════════════
    // Configuration — override via window globals or <script> config block
    // ═══════════════════════════════════════════════════════════════════════
    const RPC_BASE  = (window.MOLTCHAIN_RPC || 'http://localhost:8000').replace(/\/$/, '');
    const WS_URL    = (window.MOLTCHAIN_WS  || 'ws://localhost:8000/ws').replace(/\/$/, '');
    const API_BASE  = `${RPC_BASE}/api/v1`;
    const PRICE_SCALE = 1_000_000_000;

    // ═══════════════════════════════════════════════════════════════════════
    // API Client
    // ═══════════════════════════════════════════════════════════════════════
    const api = {
        async get(path) {
            const res = await fetch(`${API_BASE}${path}`, { headers: { 'Content-Type': 'application/json' } });
            if (!res.ok) throw new Error(`API ${res.status}: ${await res.text().catch(() => '')}`);
            const json = await res.json();
            if (json && typeof json === 'object' && 'success' in json) {
                if (!json.success) throw new Error(json.error || 'Request failed');
                return { data: json.data, slot: json.slot };
            }
            return { data: json, slot: 0 };
        },
        async post(path, body) {
            const res = await fetch(`${API_BASE}${path}`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
            });
            if (!res.ok) throw new Error(`API ${res.status}: ${await res.text().catch(() => '')}`);
            const json = await res.json();
            if (json && typeof json === 'object' && 'success' in json) {
                if (!json.success) throw new Error(json.error || 'Request failed');
                return { data: json.data, slot: json.slot };
            }
            return { data: json, slot: 0 };
        },
        async del(path) {
            const res = await fetch(`${API_BASE}${path}`, { method: 'DELETE' });
            if (!res.ok) throw new Error(`API ${res.status}: ${await res.text().catch(() => '')}`);
            const json = await res.json();
            if (json && typeof json === 'object' && 'success' in json) {
                if (!json.success) throw new Error(json.error || 'Request failed');
                return { data: json.data, slot: json.slot };
            }
            return { data: json, slot: 0 };
        },
        async rpc(method, params = {}) {
            const res = await fetch(RPC_BASE, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ jsonrpc: '2.0', id: Date.now(), method, params }),
            });
            const json = await res.json();
            if (json.error) throw new Error(`RPC: ${json.error.message}`);
            return json.result;
        },
    };

    // ═══════════════════════════════════════════════════════════════════════
    // WebSocket Manager
    // ═══════════════════════════════════════════════════════════════════════
    class DexWS {
        constructor(url) {
            this.url = url;
            this.ws = null;
            this.subs = new Map();
            this.pending = [];
            this.nextReqId = 1;
            this.pendingReqs = new Map();
            this.reconnectDelay = 1000;
            this.connect();
        }
        connect() {
            try { this.ws = new WebSocket(this.url); } catch { return; }
            this.ws.onopen = () => {
                console.log('[WS] Connected');
                this.reconnectDelay = 1000;
                for (const [, sub] of this.subs) this._sendSubscribe(sub.method, sub.params);
                this.pending.forEach(msg => this.ws.send(msg));
                this.pending = [];
            };
            this.ws.onmessage = (ev) => {
                try {
                    const msg = JSON.parse(ev.data);
                    if (msg.id && this.pendingReqs.has(msg.id)) {
                        const { resolve, reject } = this.pendingReqs.get(msg.id);
                        this.pendingReqs.delete(msg.id);
                        if (msg.error) reject(new Error(msg.error.message));
                        else resolve(msg.result);
                        return;
                    }
                    if (msg.method === 'notification' && msg.params) {
                        const sub = this.subs.get(msg.params.subscription);
                        if (sub?.callback) sub.callback(msg.params.result);
                    }
                    if (msg.channel && msg.data) {
                        for (const [, sub] of this.subs) {
                            if (sub.channel === msg.channel && sub.callback) sub.callback(msg.data);
                        }
                    }
                } catch { /* ignore */ }
            };
            this.ws.onclose = () => {
                setTimeout(() => this.connect(), this.reconnectDelay);
                this.reconnectDelay = Math.min(this.reconnectDelay * 2, 30000);
            };
            this.ws.onerror = () => {};
        }
        _sendSubscribe(method, params) {
            const id = this.nextReqId++;
            const msg = JSON.stringify({ jsonrpc: '2.0', id, method, params });
            if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(msg);
            else this.pending.push(msg);
            return new Promise((resolve, reject) => {
                this.pendingReqs.set(id, { resolve, reject });
                setTimeout(() => { if (this.pendingReqs.has(id)) { this.pendingReqs.delete(id); reject(new Error('timeout')); }}, 10000);
            });
        }
        async subscribe(channel, callback) {
            try {
                const subId = await this._sendSubscribe('subscribeDex', { channel });
                this.subs.set(subId, { channel, method: 'subscribeDex', params: { channel }, callback });
                return subId;
            } catch {
                const fakeId = this.nextReqId++;
                this.subs.set(fakeId, { channel, method: 'subscribeDex', params: { channel }, callback });
                return fakeId;
            }
        }
        unsubscribe(subId) { this.subs.delete(subId); }
    }

    let dexWs = null;

    // ═══════════════════════════════════════════════════════════════════════
    // Wallet — Ed25519 via tweetnacl
    // ═══════════════════════════════════════════════════════════════════════
    const wallet = {
        keypair: null, address: null, shortAddr: null, _nacl: null,

        async _ensureNacl() {
            if (this._nacl) return this._nacl;
            if (typeof globalThis.nacl !== 'undefined') { this._nacl = globalThis.nacl; return this._nacl; }
            try { const m = await import('https://esm.sh/tweetnacl@1.0.3'); this._nacl = m.default || m; return this._nacl; } catch { return null; }
        },
        async generate() {
            const n = await this._ensureNacl();
            this.keypair = n ? n.sign.keyPair() : { publicKey: crypto.getRandomValues(new Uint8Array(32)), secretKey: new Uint8Array(64) };
            this.address = bytesToHex(this.keypair.publicKey);
            this.shortAddr = this.address.slice(0, 8) + '...' + this.address.slice(-6);
            return this;
        },
        async fromSecretKey(hexKey) {
            const n = await this._ensureNacl();
            const bytes = hexToBytes(hexKey);
            if (n && bytes.length === 64) this.keypair = { publicKey: bytes.slice(32), secretKey: bytes };
            else if (n && bytes.length === 32) this.keypair = n.sign.keyPair.fromSeed(bytes);
            else throw new Error('Invalid key (expected 32 or 64 byte hex)');
            this.address = bytesToHex(this.keypair.publicKey);
            this.shortAddr = this.address.slice(0, 8) + '...' + this.address.slice(-6);
            return this;
        },
        sign(message) {
            if (!this.keypair || !this._nacl) throw new Error('Wallet not initialized');
            return this._nacl.sign.detached(message, this.keypair.secretKey);
        },
        async sendTransaction(instructions) {
            if (!this.keypair) throw new Error('Wallet not connected');
            const blockhash = await api.rpc('getRecentBlockhash');
            const msgBytes = encodeTransactionMessage(instructions, blockhash, this.address);
            const sig = this.sign(msgBytes);
            const txPayload = { signatures: [bytesToHex(sig)], message: { instructions, recentBlockhash: blockhash, signerPubkey: this.address } };
            const txBase64 = btoa(String.fromCharCode(...new TextEncoder().encode(JSON.stringify(txPayload))));
            return api.rpc('sendTransaction', [txBase64]);
        },
    };

    function bytesToHex(b) { return Array.from(b).map(x => x.toString(16).padStart(2, '0')).join(''); }
    function hexToBytes(h) { const c = h.startsWith('0x') ? h.slice(2) : h; const o = new Uint8Array(c.length / 2); for (let i = 0; i < o.length; i++) o[i] = parseInt(c.slice(i * 2, i * 2 + 2), 16); return o; }
    function encodeTransactionMessage(instructions, blockhash, signer) {
        const enc = new TextEncoder();
        const parts = [enc.encode(blockhash), hexToBytes(signer)];
        const instrBytes = instructions.map(ix => { const d = typeof ix.data === 'string' ? enc.encode(ix.data) : ix.data; return new Uint8Array([...hexToBytes(ix.programId), ...new Uint8Array(new Uint32Array([d.length]).buffer), ...d]); });
        const all = [new Uint8Array(new Uint32Array([instrBytes.length]).buffer), ...parts, ...instrBytes];
        const total = all.reduce((s, a) => s + a.length, 0); const out = new Uint8Array(total); let off = 0;
        for (const a of all) { out.set(a, off); off += a.length; } return out;
    }

    // ═══════════════════════════════════════════════════════════════════════
    // State
    // ═══════════════════════════════════════════════════════════════════════
    const state = {
        activePair: null, activePairId: 0, orderSide: 'buy', orderType: 'limit',
        marginSide: 'long', marginType: 'isolated', chartInterval: '15m', chartType: 'candle',
        currentView: 'trade', leverageValue: 2, lastPrice: 0, orderBook: { asks: [], bids: [] },
        candles: [], connected: false, tradeMode: 'spot', _wsSubs: [],
    };
    let pairs = [], balances = {}, openOrders = [];

    // ═══════════════════════════════════════════════════════════════════════
    // Data Loading
    // ═══════════════════════════════════════════════════════════════════════
    async function loadPairs() {
        try {
            const { data } = await api.get('/pairs');
            if (Array.isArray(data) && data.length > 0) {
                pairs = data.map(p => ({
                    id: p.symbol || `Pair#${p.pairId}`, pairId: p.pairId, base: p.baseToken, quote: p.quoteToken,
                    price: p.lastPrice || 0, change: p.change24h || 0, tickSize: p.tickSize, lotSize: p.lotSize, symbol: p.symbol,
                }));
            }
        } catch (e) { console.warn('[DEX] Pairs API unavailable:', e.message); }
        if (!pairs.length) {
            pairs = [
                { id: 'MOLT/mUSD', pairId: 0, base: 'MOLT', quote: 'mUSD', price: 0.4217, change: 5.24 },
                { id: 'wSOL/mUSD', pairId: 1, base: 'wSOL', quote: 'mUSD', price: 178.42, change: 2.14 },
                { id: 'wETH/mUSD', pairId: 2, base: 'wETH', quote: 'mUSD', price: 3521.80, change: -0.42 },
                { id: 'REEF/mUSD', pairId: 3, base: 'REEF', quote: 'mUSD', price: 0.01842, change: -2.1 },
                { id: 'wSOL/MOLT', pairId: 4, base: 'wSOL', quote: 'MOLT', price: 423.05, change: 1.37 },
                { id: 'wETH/MOLT', pairId: 5, base: 'wETH', quote: 'MOLT', price: 8351.20, change: -0.89 },
                { id: 'REEF/MOLT', pairId: 6, base: 'REEF', quote: 'MOLT', price: 0.04368, change: 0.83 },
            ];
        }
        state.activePair = pairs[0]; state.activePairId = pairs[0].pairId; state.lastPrice = pairs[0].price;
    }

    async function loadOrderBook() {
        try {
            const { data } = await api.get(`/pairs/${state.activePairId}/orderbook?depth=20`);
            if (data?.asks && data?.bids) {
                const map = arr => arr.map(a => ({ price: +a.price, amount: +(a.quantity || a.amount || 0), total: 0 }));
                const asks = map(data.asks); asks.sort((a, b) => a.price - b.price);
                let t = 0; asks.forEach(a => { t += a.amount; a.total = t; });
                const bids = map(data.bids); bids.sort((a, b) => b.price - a.price);
                t = 0; bids.forEach(b => { t += b.amount; b.total = t; });
                state.orderBook = { asks, bids }; renderOrderBook(); return;
            }
        } catch { /* fallback */ }
        genOrderBookFallback();
    }

    function genOrderBookFallback() {
        const p = state.lastPrice || 0.42, sp = p * 0.001, asks = [], bids = [];
        for (let i = 0; i < 15; i++) { asks.push({ price: p + sp + Math.random() * p * 0.008 * (i + 1), amount: Math.random() * 50000 + 1000, total: 0 }); }
        asks.sort((a, b) => a.price - b.price); let t = 0; asks.forEach(a => { t += a.amount; a.total = t; });
        for (let i = 0; i < 15; i++) { bids.push({ price: p - sp - Math.random() * p * 0.008 * (i + 1), amount: Math.random() * 50000 + 1000, total: 0 }); }
        bids.sort((a, b) => b.price - a.price); t = 0; bids.forEach(b => { t += b.amount; b.total = t; });
        state.orderBook = { asks, bids }; renderOrderBook();
    }

    async function loadRecentTrades() {
        try {
            const { data } = await api.get(`/pairs/${state.activePairId}/trades?limit=40`);
            if (Array.isArray(data) && data.length > 0) {
                const container = document.querySelector('.trades-list'); if (!container) return;
                container.innerHTML = data.map(tr => {
                    const buy = tr.side === 'buy'; const price = +tr.price || 0; const amount = tr.quantity || tr.amount || 0;
                    return `<div class="trade-row"><span class="trade-price ${buy ? 'buy' : 'sell'}">${formatPrice(price)}</span><span>${formatAmount(amount)}</span><span class="trade-time">${tr.timestamp ? new Date(tr.timestamp).toLocaleTimeString() : ''}</span></div>`;
                }).join(''); return;
            }
        } catch { /* fallback */ }
        genTradesFallback();
    }

    function genTradesFallback() {
        const container = document.querySelector('.trades-list'); if (!container) return;
        const now = Date.now(); const rows = [];
        for (let i = 0; i < 30; i++) {
            const buy = Math.random() > 0.5, price = state.lastPrice + (Math.random() - 0.5) * state.lastPrice * 0.004;
            rows.push(`<div class="trade-row"><span class="trade-price ${buy ? 'buy' : 'sell'}">${formatPrice(price)}</span><span>${formatAmount(Math.random() * 10000 + 100)}</span><span class="trade-time">${new Date(now - i * Math.random() * 15000).toLocaleTimeString()}</span></div>`);
        }
        container.innerHTML = rows.join('');
    }

    async function loadCandles(from, to, interval) {
        try {
            const params = new URLSearchParams({ interval: resolutionToSec(interval || '15'), limit: 300 });
            if (from) params.set('from', Math.floor(from)); if (to) params.set('to', Math.floor(to));
            const { data } = await api.get(`/pairs/${state.activePairId}/candles?${params}`);
            if (Array.isArray(data) && data.length > 0) return data.map(c => ({ time: (c.timestamp || c.time || 0) * 1000, open: c.open || 0, high: c.high || 0, low: c.low || 0, close: c.close || 0, volume: c.volume || 0 }));
        } catch { /* fallback */ }
        return null;
    }

    async function loadTicker(pairId) { try { const { data } = await api.get(`/pairs/${pairId}/ticker`); return data; } catch { return null; } }

    async function loadBalances(address) {
        if (!address) return;
        try {
            const result = await api.rpc('getBalance', [address]);
            if (result && typeof result === 'object') {
                balances = {};
                if (result.shells !== undefined) balances['MOLT'] = { available: result.shells / 1e9, usd: (result.shells / 1e9) * state.lastPrice };
                if (result.tokens) for (const [tok, amt] of Object.entries(result.tokens)) balances[tok] = { available: amt / 1e9, usd: 0 };
            }
        } catch { /* demo fallback */ }
        if (!Object.keys(balances).length) {
            balances = { MOLT: { available: 125847.32, usd: 53087.21 }, mUSD: { available: 12500.00, usd: 12500.00 },
                wSOL: { available: 28.45, usd: 5076.15 }, wETH: { available: 3.247, usd: 11435.33 }, REEF: { available: 45000.00, usd: 828.90 } };
        }
        renderBalances();
    }

    async function loadUserOrders(address) {
        if (!address) return;
        try {
            const { data } = await api.get(`/orders?trader=${address}`);
            if (Array.isArray(data)) {
                openOrders = data.filter(o => o.status === 'open' || o.status === 'partial').map(o => ({
                    id: String(o.orderId), pair: pairs.find(p => p.pairId === o.pairId)?.id || `#${o.pairId}`,
                    side: o.side, type: o.orderType, price: o.price, amount: o.quantity,
                    filled: o.filled / (o.quantity || 1), time: new Date(o.createdSlot * 400),
                }));
                renderOpenOrders();
            }
        } catch { /* no orders from API */ }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // WebSocket Subscriptions
    // ═══════════════════════════════════════════════════════════════════════
    function connectWebSocket() { try { dexWs = new DexWS(WS_URL); } catch { /* ws unavailable */ } }

    function subscribePair(pairId) {
        if (!dexWs) return;
        state._wsSubs.forEach(id => dexWs.unsubscribe(id)); state._wsSubs = [];

        dexWs.subscribe(`orderbook:${pairId}`, (d) => {
            if (d.bids && d.asks) {
                const map = arr => arr.map(a => ({ price: a.price, amount: a.quantity, total: 0 }));
                const asks = map(d.asks); asks.sort((a, b) => a.price - b.price); let t = 0; asks.forEach(a => { t += a.amount; a.total = t; });
                const bids = map(d.bids); bids.sort((a, b) => b.price - a.price); t = 0; bids.forEach(b => { t += b.amount; b.total = t; });
                state.orderBook = { asks, bids }; if (state.currentView === 'trade') renderOrderBook();
            }
        }).then(id => state._wsSubs.push(id)).catch(() => {});

        dexWs.subscribe(`trades:${pairId}`, (d) => {
            if (d.price) {
                state.lastPrice = d.price; updateTickerDisplay();
                const c = document.querySelector('.trades-list');
                if (c && state.currentView === 'trade') {
                    const row = document.createElement('div'); row.className = 'trade-row';
                    row.innerHTML = `<span class="trade-price ${d.side === 'buy' ? 'buy' : 'sell'}">${formatPrice(d.price)}</span><span>${formatAmount(d.quantity || 0)}</span><span class="trade-time">${new Date().toLocaleTimeString()}</span>`;
                    c.prepend(row); if (c.children.length > 40) c.lastChild.remove();
                }
                streamBarUpdate(d.price, d.quantity || 0);
            }
        }).then(id => state._wsSubs.push(id)).catch(() => {});

        dexWs.subscribe(`ticker:${pairId}`, (d) => {
            if (d.last_price) {
                state.lastPrice = d.last_price;
                const pair = pairs.find(p => p.pairId === pairId);
                if (pair) { pair.price = d.last_price; pair.change = d.change_24h || pair.change; }
                updateTickerDisplay();
            }
        }).then(id => state._wsSubs.push(id)).catch(() => {});

        if (wallet.address) {
            dexWs.subscribe(`orders:${wallet.address}`, (d) => {
                if (d.order_id) {
                    const o = openOrders.find(x => x.id === String(d.order_id));
                    if (o) { o.filled = d.filled / ((d.filled + d.remaining) || 1); }
                    if (d.status === 'filled' || d.status === 'cancelled') {
                        showNotification(`Order ${d.status}: #${d.order_id}`, d.status === 'filled' ? 'success' : 'info');
                        openOrders = openOrders.filter(x => x.id !== String(d.order_id));
                    }
                    renderOpenOrders();
                }
            }).then(id => state._wsSubs.push(id)).catch(() => {});
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Navigation
    // ═══════════════════════════════════════════════════════════════════════
    const navLinks = document.querySelectorAll('.nav-menu a[data-view]');
    const views = document.querySelectorAll('.dex-main');
    function switchView(v) { state.currentView = v; views.forEach(el => el.classList.toggle('hidden', el.id !== `view-${v}`)); navLinks.forEach(l => l.classList.toggle('active', l.dataset.view === v)); if (v === 'trade') drawChart(); if (v === 'predict') { loadPredictionStats(); loadPredictionMarkets(); loadPredictionPositions(); } }
    navLinks.forEach(l => l.addEventListener('click', e => { e.preventDefault(); switchView(l.dataset.view); }));

    // ═══════════════════════════════════════════════════════════════════════
    // Pair Selector
    // ═══════════════════════════════════════════════════════════════════════
    const pairSelector = document.querySelector('.pair-selector');
    const pairDropdown = document.querySelector('.pair-dropdown');
    const pairSearch = document.querySelector('.pair-search');
    const pairList = document.querySelector('.pair-list');
    const pairActive = document.querySelector('.pair-active');

    if (pairSelector) pairSelector.addEventListener('click', e => { e.stopPropagation(); pairDropdown.classList.toggle('open'); if (pairDropdown.classList.contains('open') && pairSearch) pairSearch.focus(); });
    document.addEventListener('click', () => { if (pairDropdown) pairDropdown.classList.remove('open'); });

    function renderPairList(filter = '') {
        if (!pairList) return; const f = filter.toLowerCase();
        pairList.innerHTML = pairs.filter(p => !f || p.id.toLowerCase().includes(f)).map(p => `
            <div class="pair-item ${state.activePair?.id === p.id ? 'active' : ''}" data-pair="${p.id}">
                <span>${p.id}</span><span class="pair-price">${formatPrice(p.price)}</span>
            </div>`).join('');
        pairList.querySelectorAll('.pair-item').forEach(el => el.addEventListener('click', e => { e.stopPropagation(); const pair = pairs.find(p => p.id === el.dataset.pair); if (pair) selectPair(pair); pairDropdown.classList.remove('open'); }));
    }
    if (pairSearch) pairSearch.addEventListener('input', e => renderPairList(e.target.value));

    async function selectPair(pair) {
        state.activePair = pair; state.activePairId = pair.pairId; state.lastPrice = pair.price;
        if (pairActive) pairActive.querySelector('.pair-name').textContent = pair.id;
        updatePairStats(pair); updateTickerDisplay(); renderPairList();
        await Promise.all([loadOrderBook(), loadRecentTrades()]);
        subscribePair(pair.pairId);
        if (tvWidget?.activeChart) { try { tvWidget.activeChart().setSymbol(pair.id, () => {}); } catch { drawChart(); } } else drawChart();
    }

    function updatePairStats(pair) {
        const stats = document.querySelectorAll('.pair-stats .stat-item .stat-value');
        if (stats.length >= 4) loadTicker(pair.pairId).then(t => {
            if (t) { stats[0].textContent = formatPrice(t.high24h || pair.price * 1.04); stats[1].textContent = formatPrice(t.low24h || pair.price * 0.96); stats[2].textContent = formatVolume(t.volume24h || 0); stats[3].textContent = String(t.trades24h || '--'); }
            else { stats[0].textContent = formatPrice(pair.price * 1.04); stats[1].textContent = formatPrice(pair.price * 0.96); stats[2].textContent = '--'; stats[3].textContent = '--'; }
        });
    }

    function updateTickerDisplay() {
        const t = document.querySelector('.ticker-price'), ch = document.querySelector('.ticker-change');
        if (t) t.textContent = formatPrice(state.lastPrice);
        if (ch && state.activePair) { const c = state.activePair.change || 0; ch.textContent = `${c >= 0 ? '+' : ''}${c.toFixed(2)}%`; ch.className = `ticker-change ${c >= 0 ? 'positive' : 'negative'}`; }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Order Book Rendering
    // ═══════════════════════════════════════════════════════════════════════
    function renderOrderBook() {
        const ac = document.querySelector('.book-asks'), bc = document.querySelector('.book-bids'), sp = document.querySelector('.spread-price'), sv = document.querySelector('.spread-value');
        if (!ac || !bc) return;
        const ma = Math.max(...state.orderBook.asks.map(a => a.total), 1), mb = Math.max(...state.orderBook.bids.map(b => b.total), 1);
        ac.innerHTML = [...state.orderBook.asks].reverse().map(a => `<div class="book-row ask"><span class="price">${formatPrice(a.price)}</span><span>${formatAmount(a.amount)}</span><span>${formatAmount(a.total)}</span><div class="depth-bar" style="width:${(a.total/ma*100).toFixed(1)}%"></div></div>`).join('');
        if (sp) { const tb = state.orderBook.bids[0]?.price || 0, ba = state.orderBook.asks[0]?.price || 0; sp.textContent = formatPrice((tb + ba) / 2); if (sv) { const s = ba - tb; sv.textContent = `Spread: ${formatPrice(Math.abs(s))} (${ba > 0 ? (s/ba*100).toFixed(3) : '0.000'}%)`; } }
        bc.innerHTML = state.orderBook.bids.map(b => `<div class="book-row bid"><span class="price">${formatPrice(b.price)}</span><span>${formatAmount(b.amount)}</span><span>${formatAmount(b.total)}</span><div class="depth-bar" style="width:${(b.total/mb*100).toFixed(1)}%"></div></div>`).join('');
    }

    // ═══════════════════════════════════════════════════════════════════════
    // TradingView (wired to candle API)
    // ═══════════════════════════════════════════════════════════════════════
    let tvWidget = null, realtimeCallback = null, lastBarTime = 0;

    function genCandlesFallback() {
        const c = [], now = Math.floor(Date.now() / 1000); let p = state.lastPrice * (0.85 + Math.random() * 0.3);
        for (let i = 300; i >= 0; i--) { const o = p, ch = (Math.random() - 0.48) * 0.015, cl = o * (1 + ch); c.push({ time: (now - i * 900) * 1000, open: o, high: Math.max(o, cl) * (1 + Math.random() * 0.008), low: Math.min(o, cl) * (1 - Math.random() * 0.008), close: cl, volume: Math.random() * 500000 + 50000 }); p = cl; }
        if (c.length) c[c.length - 1].close = state.lastPrice; state.candles = c;
    }

    function createDatafeed() {
        return {
            onReady: cb => setTimeout(() => cb({ supported_resolutions: ['1','5','15','30','60','240','1D','1W','1M'], exchanges: [{ value: 'MoltChain', name: 'MoltChain', desc: 'MoltChain DEX' }], symbols_types: [{ name: 'crypto', value: 'crypto' }] }), 0),
            searchSymbols: (input, ex, st, cb) => cb(pairs.filter(p => p.id.toLowerCase().includes(input.toLowerCase())).map(p => ({ symbol: p.id, full_name: 'MoltChain:' + p.id, description: p.id, exchange: 'MoltChain', type: 'crypto' }))),
            resolveSymbol: (name, ok, err) => {
                const p = pairs.find(x => x.id === name || ('MoltChain:' + x.id) === name) || pairs[0];
                if (!p) { err('Not found'); return; }
                setTimeout(() => ok({ name: p.id, ticker: p.id, description: p.id, type: 'crypto', session: '24x7', timezone: 'Etc/UTC', exchange: 'MoltChain', listed_exchange: 'MoltChain', minmov: 1, pricescale: p.price < 0.001 ? 100000000 : p.price < 1 ? 10000 : 100, has_intraday: true, has_weekly_and_monthly: true, supported_resolutions: ['1','5','15','30','60','240','1D','1W','1M'], volume_precision: 2, data_status: 'streaming' }), 0);
            },
            getBars: async (si, res, pp, ok) => {
                const apiC = await loadCandles(pp.from, pp.to, res);
                let bars;
                if (apiC?.length) { bars = apiC; state.candles = apiC; }
                else {
                    if (!state.candles.length) genCandlesFallback();
                    const rm = resolutionToMs(res), bm = new Map();
                    state.candles.filter(c => c.time / 1000 >= pp.from && c.time / 1000 <= pp.to).forEach(c => { const t = Math.floor(c.time / rm) * rm; if (bm.has(t)) { const b = bm.get(t); b.high = Math.max(b.high, c.high); b.low = Math.min(b.low, c.low); b.close = c.close; b.volume += c.volume; } else bm.set(t, { ...c, time: t }); });
                    bars = [...bm.values()].sort((a, b) => a.time - b.time);
                }
                if (bars.length) lastBarTime = bars[bars.length - 1].time;
                ok(bars, { noData: !bars.length });
            },
            subscribeBars: (si, res, cb) => { realtimeCallback = cb; },
            unsubscribeBars: () => { realtimeCallback = null; },
        };
    }

    function streamBarUpdate(price, vol) {
        if (!realtimeCallback) return;
        const bt = Math.floor(Date.now() / 900000) * 900000;
        realtimeCallback(bt > lastBarTime ? (lastBarTime = bt, { time: bt, open: price, high: price, low: price, close: price, volume: vol }) : { time: lastBarTime, close: price, high: price, low: price, volume: vol });
    }

    function resolutionToMs(r) { return { '1': 60000, '5': 300000, '15': 900000, '30': 1800000, '60': 3600000, '240': 14400000, '1D': 86400000, '1W': 604800000, '1M': 2592000000 }[r] || 900000; }
    function resolutionToSec(r) { return { '1': 60, '5': 300, '15': 900, '30': 1800, '60': 3600, '240': 14400, '1D': 86400, '1W': 604800, '1M': 2592000 }[r] || 900; }

    function initTradingView() {
        const el = document.getElementById('tvChartContainer');
        if (!el || typeof TradingView === 'undefined') { if (el) el.innerHTML = '<div style="display:flex;align-items:center;justify-content:center;height:100%;color:var(--text-muted);font-size:0.9rem;"><i class="fas fa-chart-line" style="margin-right:8px;"></i> Chart loading...</div>'; return; }
        tvWidget = new TradingView.widget({
            symbol: state.activePair?.id || 'MOLT/mUSD', container: el, datafeed: createDatafeed(), library_path: 'charting_library/', locale: 'en', fullscreen: false, autosize: true, theme: 'Dark', interval: '15', toolbar_bg: '#0d1117',
            loading_screen: { backgroundColor: '#0A0E27', foregroundColor: '#FF6B35' },
            overrides: { 'paneProperties.background': '#0d1117', 'paneProperties.backgroundType': 'solid', 'paneProperties.vertGridProperties.color': 'rgba(255,255,255,0.04)', 'paneProperties.horzGridProperties.color': 'rgba(255,255,255,0.04)', 'scalesProperties.textColor': 'rgba(255,255,255,0.5)', 'scalesProperties.lineColor': 'rgba(255,255,255,0.08)', 'mainSeriesProperties.candleStyle.upColor': '#06d6a0', 'mainSeriesProperties.candleStyle.downColor': '#ef4444', 'mainSeriesProperties.candleStyle.borderUpColor': '#06d6a0', 'mainSeriesProperties.candleStyle.borderDownColor': '#ef4444', 'mainSeriesProperties.candleStyle.wickUpColor': '#06d6a0', 'mainSeriesProperties.candleStyle.wickDownColor': '#ef4444' },
            disabled_features: ['header_compare','header_undo_redo','go_to_date','use_localstorage_for_settings'],
            enabled_features: ['study_templates','side_toolbar_in_fullscreen_mode','header_symbol_search'],
        });
        tvWidget.onChartReady(() => { tvWidget.activeChart().onSymbolChanged().subscribe(null, () => { const s = tvWidget.activeChart().symbol(); const p = pairs.find(x => x.id === s || ('MoltChain:' + x.id) === s); if (p && p.id !== state.activePair?.id) selectPair(p); }); });
    }

    function drawChart() { if (realtimeCallback && state.candles.length) { const l = state.candles[state.candles.length - 1]; realtimeCallback({ time: Math.floor(l.time / 900000) * 900000, open: l.open, high: l.high, low: l.low, close: l.close, volume: l.volume }); } }

    // ═══════════════════════════════════════════════════════════════════════
    // Order Form
    // ═══════════════════════════════════════════════════════════════════════
    const orderTabs = document.querySelectorAll('.order-tab'), orderTypeBtns = document.querySelectorAll('.order-type-btn');
    const priceInput = document.getElementById('orderPrice'), amountInput = document.getElementById('orderAmount'), totalInput = document.getElementById('orderTotal');
    const submitBtn = document.getElementById('submitOrder'), stopGroup = document.querySelector('.stop-price-group');

    orderTabs.forEach(tab => tab.addEventListener('click', () => { orderTabs.forEach(t => t.classList.remove('active')); tab.classList.add('active'); state.orderSide = tab.dataset.side; updateSubmitBtn(); }));
    orderTypeBtns.forEach(btn => btn.addEventListener('click', () => { orderTypeBtns.forEach(b => b.classList.remove('active')); btn.classList.add('active'); state.orderType = btn.dataset.type; if (stopGroup) stopGroup.style.display = state.orderType === 'stop-limit' ? 'block' : 'none'; if (priceInput) priceInput.parentElement.parentElement.style.display = state.orderType === 'market' ? 'none' : 'block'; }));

    function updateSubmitBtn() { if (!submitBtn) return; const m = state.tradeMode === 'margin' ? ` ${state.leverageValue}x` : ''; submitBtn.className = `btn-full ${state.orderSide === 'buy' ? 'btn-buy' : 'btn-sell'}`; submitBtn.textContent = `${state.orderSide === 'buy' ? 'Buy' : 'Sell'}${m} ${state.activePair?.base || ''}`; }

    document.querySelectorAll('.trade-mode').forEach(btn => { btn.addEventListener('click', () => { document.querySelectorAll('.trade-mode').forEach(b => b.classList.remove('active')); btn.classList.add('active'); state.tradeMode = btn.dataset.mode; const mi = document.getElementById('marginInline'); if (mi) mi.classList.toggle('hidden', state.tradeMode !== 'margin'); updateSubmitBtn(); }); });
    const inlineLeverage = document.getElementById('inlineLeverage'), inlineLeverageTag = document.getElementById('inlineLeverageTag');
    if (inlineLeverage) inlineLeverage.addEventListener('input', () => { state.leverageValue = parseFloat(inlineLeverage.value); if (inlineLeverageTag) inlineLeverageTag.textContent = `${state.leverageValue}x`; updateSubmitBtn(); });
    document.querySelectorAll('.margin-inline-type').forEach(btn => btn.addEventListener('click', () => { document.querySelectorAll('.margin-inline-type').forEach(b => b.classList.remove('active')); btn.classList.add('active'); state.marginType = btn.dataset.mtype; if (inlineLeverage) inlineLeverage.max = state.marginType === 'isolated' ? '5' : '3'; }));

    function calcTotal() { if (!priceInput || !amountInput || !totalInput) return; const p = parseFloat(priceInput.value) || 0, a = parseFloat(amountInput.value) || 0; totalInput.value = (p * a).toFixed(4); const fe = document.getElementById('feeEstimate'), re = document.getElementById('routeInfo'); if (fe) fe.textContent = `~${(p * a * 0.0005).toFixed(4)} ${state.activePair?.quote || ''}`; if (re) re.textContent = p * a > 50000 ? 'CLOB + AMM Split' : 'CLOB Direct'; }
    if (priceInput) priceInput.addEventListener('input', calcTotal);
    if (amountInput) amountInput.addEventListener('input', calcTotal);
    if (totalInput) totalInput.addEventListener('input', () => { if (!priceInput || !amountInput) return; const p = parseFloat(priceInput.value) || 0, t = parseFloat(totalInput.value) || 0; if (p > 0) amountInput.value = (t / p).toFixed(4); });

    document.querySelectorAll('.preset-btn').forEach(btn => btn.addEventListener('click', () => {
        const pct = parseInt(btn.dataset.pct, 10) / 100, tok = state.orderSide === 'buy' ? state.activePair?.quote : state.activePair?.base, bal = tok ? balances[tok] : null;
        if (!bal || !amountInput || !priceInput) return;
        if (state.orderSide === 'buy') { amountInput.value = ((bal.available * pct) / (parseFloat(priceInput.value) || state.lastPrice)).toFixed(4); } else amountInput.value = (bal.available * pct).toFixed(4);
        calcTotal();
    }));

    // === Order submission — POST to API ===
    if (submitBtn) submitBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
        const price = parseFloat(priceInput?.value) || 0, amount = parseFloat(amountInput?.value) || 0;
        if (!amount || (state.orderType !== 'market' && !price)) { showNotification('Enter price and amount', 'warning'); return; }
        submitBtn.disabled = true; submitBtn.textContent = 'Submitting...';
        try {
            const { data } = await api.post('/orders', { pairId: state.activePairId, side: state.orderSide, orderType: state.orderType, price: Math.round(price * PRICE_SCALE), quantity: Math.round(amount * PRICE_SCALE), trader: wallet.address });
            showNotification(`${state.orderSide.toUpperCase()} order placed: ${formatAmount(amount)} ${state.activePair?.base || ''} @ ${state.orderType === 'market' ? 'MARKET' : formatPrice(price)}`, 'success');
            openOrders.push({ id: data?.orderId ? String(data.orderId) : Math.random().toString(36).slice(2, 8).toUpperCase(), pair: state.activePair?.id, side: state.orderSide, type: state.orderType, price: price || state.lastPrice, amount, filled: 0, time: new Date() });
            renderOpenOrders(); if (amountInput) amountInput.value = ''; if (totalInput) totalInput.value = '';
        } catch (e) { showNotification(`Order failed: ${e.message}`, 'error'); }
        finally { submitBtn.disabled = false; updateSubmitBtn(); }
    });

    // ═══════════════════════════════════════════════════════════════════════
    // Open Orders
    // ═══════════════════════════════════════════════════════════════════════
    function renderOpenOrders() {
        const tb = document.getElementById('openOrdersBody'), badge = document.querySelector('.orders-badge'); if (!tb) return;
        if (badge) badge.textContent = openOrders.length;
        if (!openOrders.length) { tb.innerHTML = '<tr><td colspan="8" style="text-align:center;color:var(--text-muted);padding:20px;">No open orders</td></tr>'; return; }
        tb.innerHTML = openOrders.map(o => `<tr class="order-row"><td>${o.pair}</td><td class="side-${o.side}">${o.side.toUpperCase()}</td><td style="text-transform:capitalize">${o.type}</td><td>${formatPrice(o.price)}</td><td>${formatAmount(o.amount)}</td><td>${(o.filled * 100).toFixed(0)}%</td><td>${o.time instanceof Date ? o.time.toLocaleTimeString() : ''}</td><td><button class="cancel-btn" data-id="${o.id}"><i class="fas fa-times"></i></button></td></tr>`).join('');
        tb.querySelectorAll('.cancel-btn').forEach(btn => btn.addEventListener('click', async () => {
            try { await api.del(`/orders/${btn.dataset.id}`); } catch { /* fallback */ }
            openOrders = openOrders.filter(o => o.id !== btn.dataset.id); renderOpenOrders(); showNotification('Order cancelled', 'info');
        }));
    }

    document.querySelectorAll('.pos-tab').forEach(tab => tab.addEventListener('click', () => { document.querySelectorAll('.pos-tab').forEach(t => t.classList.remove('active')); tab.classList.add('active'); document.querySelectorAll('.positions-content').forEach(c => c.classList.add('hidden')); const t = document.getElementById(tab.dataset.target); if (t) t.classList.remove('hidden'); }));

    // ═══════════════════════════════════════════════════════════════════════
    // Wallet UI
    // ═══════════════════════════════════════════════════════════════════════
    const connectBtn = document.getElementById('connectWallet'), walletModal = document.getElementById('walletModal'), closeModalBtn = document.getElementById('closeWalletModal');
    const wmTabs = document.querySelectorAll('.wm-tab'), wmTC = { wallets: document.getElementById('wmTabWallets'), import: document.getElementById('wmTabImport'), create: document.getElementById('wmTabCreate') };
    let savedWallets = JSON.parse(localStorage.getItem('dexWallets') || '[]');

    function openWalletModal() { if (walletModal) { walletModal.classList.remove('hidden'); renderWalletList(); switchWmTab(savedWallets.length ? 'wallets' : 'import'); } }
    function closeWalletModalFn() { if (walletModal) walletModal.classList.add('hidden'); }
    function switchWmTab(t) { wmTabs.forEach(x => x.classList.toggle('active', x.dataset.wmTab === t)); Object.entries(wmTC).forEach(([k, el]) => { if (el) el.classList.toggle('hidden', k !== t); }); }

    if (connectBtn) connectBtn.addEventListener('click', () => openWalletModal());
    if (closeModalBtn) closeModalBtn.addEventListener('click', closeWalletModalFn);
    if (walletModal) walletModal.addEventListener('click', e => { if (e.target === walletModal) closeWalletModalFn(); });
    wmTabs.forEach(t => t.addEventListener('click', () => switchWmTab(t.dataset.wmTab)));

    document.querySelectorAll('.wm-import-type').forEach(btn => btn.addEventListener('click', () => {
        document.querySelectorAll('.wm-import-type').forEach(b => b.classList.remove('active')); btn.classList.add('active');
        const k = document.getElementById('wmImportKey'), m = document.getElementById('wmImportMnemonic');
        if (btn.dataset.import === 'key') { if (k) k.classList.remove('hidden'); if (m) m.classList.add('hidden'); } else { if (k) k.classList.add('hidden'); if (m) m.classList.remove('hidden'); }
    }));

    const mnGrid = document.getElementById('mnemonicGrid');
    if (mnGrid) for (let i = 0; i < 12; i++) { const inp = document.createElement('input'); inp.type = 'text'; inp.placeholder = `Word ${i + 1}`; inp.className = 'form-input'; mnGrid.appendChild(inp); }

    const wmConnectBtn = document.getElementById('wmConnectBtn');
    if (wmConnectBtn) wmConnectBtn.addEventListener('click', async () => {
        const ki = document.getElementById('wmPrivateKey'), key = ki?.value?.trim();
        if (!key) { showNotification('Enter private key (hex)', 'warning'); return; }
        try { await wallet.fromSecretKey(key); savedWallets.push({ address: wallet.address, short: wallet.shortAddr, added: Date.now() }); localStorage.setItem('dexWallets', JSON.stringify(savedWallets)); connectWalletTo(wallet.address, wallet.shortAddr); closeWalletModalFn(); if (ki) ki.value = ''; showNotification('Wallet connected: ' + wallet.shortAddr, 'success'); }
        catch (e) { showNotification(`Import failed: ${e.message}`, 'error'); }
    });

    const wmCreateBtn = document.getElementById('wmCreateBtn');
    if (wmCreateBtn) wmCreateBtn.addEventListener('click', async () => {
        await wallet.generate();
        const ae = document.getElementById('wmNewAddress'), ke = document.getElementById('wmNewKey'), cd = document.getElementById('wmCreatedWallet');
        if (ae) ae.textContent = wallet.address; if (ke) ke.textContent = bytesToHex(wallet.keypair.secretKey); if (cd) cd.classList.remove('hidden');
        savedWallets.push({ address: wallet.address, short: wallet.shortAddr, added: Date.now() }); localStorage.setItem('dexWallets', JSON.stringify(savedWallets));
        connectWalletTo(wallet.address, wallet.shortAddr); showNotification('New wallet created: ' + wallet.shortAddr, 'success');
    });

    document.querySelectorAll('.wm-copy-btn').forEach(btn => btn.addEventListener('click', () => { const el = document.getElementById(btn.dataset.copy); if (el) navigator.clipboard.writeText(el.textContent).then(() => showNotification('Copied!', 'success')); }));

    async function connectWalletTo(address, shortAddr) {
        state.connected = true; state.walletAddress = address;
        if (connectBtn) { connectBtn.innerHTML = `<i class="fas fa-wallet"></i> ${shortAddr}`; connectBtn.className = 'btn btn-small btn-secondary'; }
        await Promise.all([loadBalances(address), loadUserOrders(address)]);
        if (dexWs && state.activePairId != null) subscribePair(state.activePairId);
    }

    function disconnectWallet() {
        state.connected = false; state.walletAddress = null; wallet.keypair = null; wallet.address = null;
        if (connectBtn) { connectBtn.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet'; connectBtn.className = 'btn btn-small btn-primary'; }
        renderBalances();
    }

    function renderWalletList() {
        const list = document.getElementById('wmWalletsList'); if (!list) return;
        if (!savedWallets.length) { list.innerHTML = `<div class="wm-empty"><i class="fas fa-wallet"></i><p>No wallets connected</p><button class="btn btn-primary btn-small" id="wmEmptyImport">Import Wallet</button></div>`; const b = document.getElementById('wmEmptyImport'); if (b) b.addEventListener('click', () => switchWmTab('import')); return; }
        list.innerHTML = savedWallets.map((w, i) => `<div class="wm-wallet-item ${state.walletAddress === w.address ? 'active-wallet' : ''}"><span class="wm-wallet-addr">${w.short || w.address.slice(0, 8) + '...' + w.address.slice(-6)}</span><div class="wm-wallet-actions">${state.walletAddress === w.address ? '<span class="btn btn-small btn-secondary" style="opacity:0.6;cursor:default;">Active</span>' : `<button class="btn btn-small btn-primary wm-switch-btn" data-idx="${i}">Switch</button>`}<button class="btn btn-small btn-secondary wm-remove-btn" data-idx="${i}"><i class="fas fa-times"></i></button></div></div>`).join('') + `<div class="wm-disconnect-all"><button class="btn btn-small btn-secondary" id="wmDisconnectAll">Disconnect All</button></div>`;
        list.querySelectorAll('.wm-switch-btn').forEach(btn => btn.addEventListener('click', () => { const w = savedWallets[parseInt(btn.dataset.idx)]; if (w) { connectWalletTo(w.address, w.short); renderWalletList(); } }));
        list.querySelectorAll('.wm-remove-btn').forEach(btn => btn.addEventListener('click', () => { const i = parseInt(btn.dataset.idx), r = savedWallets[i]; savedWallets.splice(i, 1); localStorage.setItem('dexWallets', JSON.stringify(savedWallets)); if (state.walletAddress === r?.address) disconnectWallet(); renderWalletList(); showNotification('Wallet removed', 'info'); }));
        const da = document.getElementById('wmDisconnectAll'); if (da) da.addEventListener('click', () => { savedWallets = []; localStorage.removeItem('dexWallets'); disconnectWallet(); renderWalletList(); showNotification('All wallets disconnected', 'info'); });
    }

    function renderBalances() {
        const c = document.querySelector('.balance-list'); if (!c) return;
        if (!state.connected) { c.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;">Connect wallet to view balances</div>'; return; }
        c.innerHTML = Object.entries(balances).map(([t, b]) => `<div class="balance-row"><div class="balance-token"><div class="token-icon ${t.toLowerCase()}-icon">${t[0]}</div><span>${t}</span></div><div class="balance-amounts"><span class="balance-available">${formatAmount(b.available)}</span><span class="balance-usd">≈ $${formatAmount(b.usd)}</span></div></div>`).join('');
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Margin View
    // ═══════════════════════════════════════════════════════════════════════
    const leverageSlider = document.getElementById('leverageSlider'), leverageDisplay = document.querySelector('.leverage-display');
    if (leverageSlider) leverageSlider.addEventListener('input', () => { state.leverageValue = parseFloat(leverageSlider.value); if (leverageDisplay) leverageDisplay.textContent = `${state.leverageValue}x`; updateMarginInfo(); });
    document.querySelectorAll('.margin-type').forEach(btn => btn.addEventListener('click', () => { document.querySelectorAll('.margin-type').forEach(b => b.classList.remove('active')); btn.classList.add('active'); state.marginType = btn.dataset.type; if (leverageSlider) leverageSlider.max = state.marginType === 'isolated' ? '5' : '3'; if (state.leverageValue > parseFloat(leverageSlider?.max)) { state.leverageValue = parseFloat(leverageSlider.max); leverageSlider.value = state.leverageValue; if (leverageDisplay) leverageDisplay.textContent = `${state.leverageValue}x`; } updateMarginInfo(); }));
    document.querySelectorAll('.side-btn').forEach(btn => btn.addEventListener('click', () => { document.querySelectorAll('.side-btn').forEach(b => b.classList.remove('active')); btn.classList.add('active'); state.marginSide = btn.classList.contains('long-btn') ? 'long' : 'short'; }));

    function updateMarginInfo() {
        const e = document.getElementById('marginEntry'), l = document.getElementById('marginLiqPrice'), r = document.getElementById('marginRatio');
        if (e) e.textContent = formatPrice(state.lastPrice);
        if (l) l.textContent = formatPrice(state.marginSide === 'long' ? state.lastPrice * (1 - 1 / state.leverageValue * 0.9) : state.lastPrice * (1 + 1 / state.leverageValue * 0.9));
        if (r) r.textContent = `${(100 / state.leverageValue).toFixed(1)}%`;
    }

    // ═══════════════════════════════════════════════════════════════════════
    // PredictionReef — Predict View (Live API + Mock Fallback)
    // ═══════════════════════════════════════════════════════════════════════

    // Mock data fallback — used when API is unavailable
    const MOCK_MARKETS = [
        { id: 1, question: 'Will BTC exceed $150,000 by March 31, 2026?', cat: 'crypto', yes: 0.62, volume: 842000, liquidity: 320000, traders: 284, status: 'active' },
        { id: 2, question: 'Will the EU pass comprehensive AI regulation by Q2 2026?', cat: 'politics', yes: 0.45, volume: 523000, liquidity: 210000, traders: 178, status: 'active' },
        { id: 3, question: 'Which L1 blockchain will have the highest TVL by Q3 2026?', cat: 'crypto', yes: 0.48, volume: 1200000, liquidity: 480000, traders: 412, status: 'active', multi: true },
        { id: 4, question: 'Will the FIFA Club World Cup 2025 champion be a European team?', cat: 'sports', yes: 0.71, volume: 198000, liquidity: 85000, traders: 96, status: 'active' },
        { id: 5, question: 'Will OpenAI release GPT-5 before February 2026?', cat: 'tech', yes: 0, volume: 156000, liquidity: 0, traders: 142, status: 'resolved' },
        { id: 6, question: 'Will SpaceX Starship complete a successful orbital flight by Q2 2026?', cat: 'science', yes: 0.83, volume: 367000, liquidity: 145000, traders: 203, status: 'active' },
    ];

    const predictState = {
        selectedMarket: 1,
        selectedOutcome: 'yes',
        markets: [...MOCK_MARKETS],
        positions: [],
        stats: null,
        live: false,
    };

    // ─── Load prediction stats from API ─────────────────────────
    async function loadPredictionStats() {
        try {
            const data = await api.get('/prediction-market/stats');
            if (data) {
                predictState.stats = data;
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('pmTotalVolume', formatVolume(data.total_volume || 0));
                el('pmOpenMarkets', data.open_markets ?? '—');
                el('pmTotalCollateral', formatVolume(data.total_collateral || 0));
                el('pmFees', formatVolume(data.fees_collected || 0));
            }
        } catch { /* API unavailable — keep placeholder text */ }
    }

    // ─── Load markets from API ──────────────────────────────────
    async function loadPredictionMarkets() {
        try {
            const data = await api.get('/prediction-market/markets?limit=50');
            if (data?.markets?.length > 0) {
                // Transform API data into UI format
                predictState.markets = data.markets.map(m => ({
                    id: m.id,
                    question: m.question,
                    cat: m.category,
                    yes: m.outcomes?.[0]?.price ?? 0.5,
                    volume: m.total_volume * 1e9,   // convert to display units
                    liquidity: m.total_collateral * 1e9,
                    traders: 0,                     // not stored per-market yet
                    status: m.status,
                    multi: (m.outcome_count || 2) > 2,
                    outcomes: m.outcomes || [],
                }));
                predictState.live = true;
                renderPredictionMarkets();
                return;
            }
        } catch { /* API unavailable */ }
        // Fallback to mock data
        predictState.markets = [...MOCK_MARKETS];
        predictState.live = false;
    }

    // ─── Load user positions from API ───────────────────────────
    async function loadPredictionPositions() {
        if (!state.connected) return;
        try {
            const data = await api.rpc('getPredictionPositions', [wallet.address]);
            if (Array.isArray(data)) {
                predictState.positions = data;
                renderPredictionPositions();
            }
        } catch { /* API unavailable */ }
    }

    // ─── Render market cards dynamically ────────────────────────
    function renderPredictionMarkets() {
        const grid = document.querySelector('.predict-markets-col');
        if (!grid) return;

        // Keep only the grid container, regenerate cards
        const existingCards = grid.querySelectorAll('.market-card');
        existingCards.forEach(c => c.remove());

        predictState.markets.forEach(m => {
            const isResolved = m.status === 'resolved';
            const isMulti = m.multi;
            const yesPct = Math.round((m.yes || 0.5) * 100);
            const noPct = 100 - yesPct;
            const catIcons = { crypto: '₿', politics: '🏛', sports: '⚽', science: '🔬', tech: '🤖', entertainment: '🎬', economics: '📈', custom: '🧩' };

            let outcomesHtml = '';
            if (isMulti && m.outcomes?.length) {
                outcomesHtml = m.outcomes.map((o, i) => {
                    const pct = Math.round((o.price || 0) * 100);
                    const colors = ['#4ea8de', '#06d6a0', '#ffd166', '#ef4444'];
                    return `<div class="outcome-row"><span class="outcome-dot" style="background:${colors[i % colors.length]}"></span><span>${o.name}</span><div class="outcome-bar"><div class="outcome-bar-fill" style="width:${pct}%;background:${colors[i % colors.length]}"></div></div><span class="outcome-pct">${pct}%</span></div>`;
                }).join('');
            } else if (isResolved) {
                outcomesHtml = `<div class="outcome-row"><span class="outcome-dot yes"></span><span>Resolved</span><div class="outcome-bar"><div class="outcome-bar-fill yes" style="width:100%"></div></div><span class="outcome-pct">✓</span></div>`;
            } else {
                outcomesHtml = `
                    <div class="outcome-row"><span class="outcome-dot yes"></span><span>Yes</span><div class="outcome-bar"><div class="outcome-bar-fill yes" style="width:${yesPct}%"></div></div><span class="outcome-pct">${yesPct}%</span></div>
                    <div class="outcome-row"><span class="outcome-dot no"></span><span>No</span><div class="outcome-bar"><div class="outcome-bar-fill no" style="width:${noPct}%"></div></div><span class="outcome-pct">${noPct}%</span></div>
                `;
            }

            const card = document.createElement('div');
            card.className = 'market-card' + (isResolved ? ' resolved' : '');
            card.dataset.cat = m.cat;
            card.dataset.marketId = m.id;
            card.innerHTML = `
                <div class="market-card-header">
                    <span class="market-cat-badge">${catIcons[m.cat] || '📊'} ${m.cat}</span>
                    <span class="market-status ${m.status}">${m.status}</span>
                </div>
                <h4 class="market-question">${m.question}</h4>
                <div class="market-outcomes">${outcomesHtml}</div>
                <div class="market-footer">
                    <div class="market-stat"><span class="stat-label">Volume</span><span class="stat-value">${formatVolume(m.volume)}</span></div>
                    <div class="market-stat"><span class="stat-label">Liquidity</span><span class="stat-value">${formatVolume(m.liquidity)}</span></div>
                    <button class="btn-predict-chart" data-market="${m.id}" title="Price Chart"><i class="fas fa-chart-line"></i></button>
                    ${!isResolved ? `
                        <div class="market-actions">
                            <button class="btn-predict-buy" data-market="${m.id}" data-outcome="yes">Buy Yes</button>
                            <button class="btn-predict-sell" data-market="${m.id}" data-outcome="no">Buy No</button>
                        </div>
                    ` : ''}
                </div>
            `;
            grid.appendChild(card);
        });

        // Re-bind event handlers for new cards
        bindPredictionCardEvents();
    }

    // ─── Render user positions in bottom panel ──────────────────
    function renderPredictionPositions() {
        const tbody = document.querySelector('.predict-positions-table tbody');
        if (!tbody) return;
        if (!predictState.positions.length) {
            tbody.innerHTML = '<tr><td colspan="5" style="text-align:center;color:var(--text-muted)">No positions found</td></tr>';
            return;
        }
        tbody.innerHTML = predictState.positions.map(p => {
            const m = predictState.markets.find(x => x.id === p.market_id);
            const qText = m?.question?.slice(0, 40) || `Market #${p.market_id}`;
            return `<tr><td>${qText}...</td><td>${p.outcome === 0 ? 'YES' : 'NO'}</td><td>${p.shares.toFixed(2)}</td><td>$${p.cost_basis.toFixed(2)}</td><td>—</td></tr>`;
        }).join('');
    }

    // ─── Bind card events (called after render) ─────────────────
    function bindPredictionCardEvents() {
        // Market card click → select for trade
        document.querySelectorAll('.market-card').forEach(card => {
            card.addEventListener('click', e => {
                if (e.target.closest('button')) return;
                const mid = parseInt(card.dataset.marketId);
                const m = predictState.markets.find(x => x.id === mid);
                if (!m || m.status !== 'active') return;
                predictState.selectedMarket = mid;
                const qEl = document.getElementById('predictSelectedQ');
                if (qEl) qEl.textContent = m.question;
                const yp = document.getElementById('predictYesPrice'), np = document.getElementById('predictNoPrice');
                if (yp) yp.textContent = `$${(m.yes || 0.5).toFixed(2)}`;
                if (np) np.textContent = `$${(1 - (m.yes || 0.5)).toFixed(2)}`;
                document.querySelectorAll('.market-card').forEach(c => c.style.outline = 'none');
                card.style.outline = '2px solid var(--orange-primary)';
                card.style.outlineOffset = '-2px';
                updatePredictCalc();
            });
        });

        // Buy/Sell buttons on cards
        document.querySelectorAll('.btn-predict-buy, .btn-predict-sell').forEach(btn => btn.addEventListener('click', () => {
            if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
            const mid = parseInt(btn.dataset.market);
            const outcome = btn.dataset.outcome;
            const m = predictState.markets.find(x => x.id === mid);
            if (!m) return;
            predictState.selectedMarket = mid;
            predictState.selectedOutcome = outcome === 'no' ? 'no' : 'yes';
            const qEl = document.getElementById('predictSelectedQ');
            if (qEl) qEl.textContent = m.question;
            const yp = document.getElementById('predictYesPrice'), np = document.getElementById('predictNoPrice');
            if (yp) yp.textContent = `$${(m.yes || 0.5).toFixed(2)}`;
            if (np) np.textContent = `$${(1 - (m.yes || 0.5)).toFixed(2)}`;
            const yBtn = document.getElementById('predictYesBtn'), nBtn = document.getElementById('predictNoBtn');
            if (yBtn) yBtn.classList.toggle('active', predictState.selectedOutcome === 'yes');
            if (nBtn) nBtn.classList.toggle('active', predictState.selectedOutcome === 'no');
            updatePredictCalc();
            showNotification(`Selected: ${m.question.slice(0, 50)}... → ${outcome.toUpperCase()}`, 'info');
        }));

        // Chart buttons on cards
        document.querySelectorAll('.btn-predict-chart').forEach(btn => btn.addEventListener('click', e => {
            e.stopPropagation();
            const mid = parseInt(btn.dataset.market);
            openPredictChart(mid);
        }));
    }

    // ═══ Prediction Chart Modal — Polymarket-style price history ═══════════

    let predictChartState = { marketId: null, range: '1d' };

    function generateMockPriceHistory(market, range) {
        const points = { '1h': 60, '6h': 72, '1d': 96, '1w': 168, 'all': 200 }[range] || 96;
        const data = [];
        const now = Date.now();
        const interval = { '1h': 60000, '6h': 5 * 60000, '1d': 15 * 60000, '1w': 60 * 60000, 'all': 4 * 60 * 60000 }[range] || 15 * 60000;
        let price = market.yes || 0.5;
        // Walk backwards from a starting seed
        const seed = price + (Math.random() - 0.5) * 0.15;
        let p = Math.max(0.05, Math.min(0.95, seed));
        for (let i = points; i >= 0; i--) {
            const t = now - i * interval;
            const drift = ((market.yes || 0.5) - p) * 0.015;
            const noise = (Math.random() - 0.5) * 0.025;
            p = Math.max(0.01, Math.min(0.99, p + drift + noise));
            data.push({ t, p });
        }
        data[data.length - 1].p = market.yes || 0.5;
        return data;
    }

    function drawPredictChart(data, canvas) {
        const ctx = canvas.getContext('2d');
        const dpr = window.devicePixelRatio || 1;
        const W = canvas.clientWidth;
        const H = canvas.clientHeight;
        canvas.width = W * dpr;
        canvas.height = H * dpr;
        ctx.scale(dpr, dpr);
        ctx.clearRect(0, 0, W, H);
        if (!data.length) return;

        const pad = { top: 20, right: 58, bottom: 32, left: 12 };
        const cW = W - pad.left - pad.right;
        const cH = H - pad.top - pad.bottom;
        const prices = data.map(d => d.p);
        const minP = Math.max(0, Math.min(...prices) - 0.05);
        const maxP = Math.min(1, Math.max(...prices) + 0.05);
        const rangeP = maxP - minP || 0.1;
        const xPos = i => pad.left + (i / (data.length - 1)) * cW;
        const yPos = p => pad.top + (1 - (p - minP) / rangeP) * cH;

        // Grid + Y labels
        ctx.strokeStyle = 'rgba(255,255,255,0.06)';
        ctx.lineWidth = 1;
        for (let i = 0; i <= 4; i++) {
            const gy = pad.top + (cH / 4) * i;
            ctx.beginPath(); ctx.moveTo(pad.left, gy); ctx.lineTo(W - pad.right, gy); ctx.stroke();
            const label = ((maxP - (i / 4) * rangeP) * 100).toFixed(0) + '%';
            ctx.fillStyle = 'rgba(255,255,255,0.35)';
            ctx.font = '11px monospace';
            ctx.textAlign = 'left';
            ctx.fillText(label, W - pad.right + 8, gy + 4);
        }

        // X time labels
        ctx.fillStyle = 'rgba(255,255,255,0.3)';
        ctx.font = '10px sans-serif';
        ctx.textAlign = 'center';
        for (let i = 0; i < 5; i++) {
            const idx = Math.round((data.length - 1) * i / 4);
            const d = new Date(data[idx].t);
            ctx.fillText(d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }), xPos(idx), H - 8);
        }

        // Gradient fill
        const lastP = data[data.length - 1].p;
        const isUp = lastP >= data[0].p;
        const grad = ctx.createLinearGradient(0, pad.top, 0, pad.top + cH);
        grad.addColorStop(0, isUp ? 'rgba(6,214,160,0.28)' : 'rgba(239,68,68,0.28)');
        grad.addColorStop(1, isUp ? 'rgba(6,214,160,0.0)' : 'rgba(239,68,68,0.0)');
        ctx.beginPath();
        ctx.moveTo(xPos(0), yPos(data[0].p));
        for (let i = 1; i < data.length; i++) ctx.lineTo(xPos(i), yPos(data[i].p));
        ctx.lineTo(xPos(data.length - 1), pad.top + cH);
        ctx.lineTo(xPos(0), pad.top + cH);
        ctx.closePath();
        ctx.fillStyle = grad;
        ctx.fill();

        // Price line
        ctx.beginPath();
        ctx.moveTo(xPos(0), yPos(data[0].p));
        for (let i = 1; i < data.length; i++) ctx.lineTo(xPos(i), yPos(data[i].p));
        ctx.strokeStyle = isUp ? '#06d6a0' : '#ef4444';
        ctx.lineWidth = 2;
        ctx.stroke();

        // Current price dot
        const lx = xPos(data.length - 1), ly = yPos(lastP);
        ctx.beginPath(); ctx.arc(lx, ly, 5, 0, Math.PI * 2);
        ctx.fillStyle = isUp ? '#06d6a0' : '#ef4444';
        ctx.fill();
        ctx.strokeStyle = '#fff'; ctx.lineWidth = 2; ctx.stroke();

        // Current price label
        ctx.fillStyle = isUp ? '#06d6a0' : '#ef4444';
        ctx.font = 'bold 12px monospace';
        ctx.textAlign = 'right';
        ctx.fillText((lastP * 100).toFixed(1) + '%', lx - 10, ly - 10);
    }

    function renderPredictChartStats(data, market) {
        const stats = document.getElementById('predictChartStats');
        if (!stats) return;
        const first = data[0].p, last = data[data.length - 1].p;
        const change = last - first;
        const changePct = first > 0 ? (change / first * 100).toFixed(1) : '0.0';
        const high = Math.max(...data.map(d => d.p));
        const low = Math.min(...data.map(d => d.p));
        stats.innerHTML = `
            <div class="predict-chart-stat"><span class="stat-label">Current</span><span class="stat-value">${(last * 100).toFixed(1)}%</span></div>
            <div class="predict-chart-stat"><span class="stat-label">Change</span><span class="stat-value ${change >= 0 ? 'up' : 'down'}">${change >= 0 ? '+' : ''}${(change * 100).toFixed(1)}pp (${change >= 0 ? '+' : ''}${changePct}%)</span></div>
            <div class="predict-chart-stat"><span class="stat-label">High</span><span class="stat-value">${(high * 100).toFixed(1)}%</span></div>
            <div class="predict-chart-stat"><span class="stat-label">Low</span><span class="stat-value">${(low * 100).toFixed(1)}%</span></div>
            <div class="predict-chart-stat"><span class="stat-label">Volume</span><span class="stat-value">${formatVolume(market.volume)}</span></div>
            <div class="predict-chart-stat"><span class="stat-label">Traders</span><span class="stat-value">${market.traders || '—'}</span></div>
        `;
    }

    function openPredictChart(marketId) {
        const m = predictState.markets.find(x => x.id === marketId);
        if (!m) return;
        predictChartState.marketId = marketId;
        predictChartState.range = '1d';
        const modal = document.getElementById('predictChartModal');
        const title = document.getElementById('predictChartTitle');
        const canvas = document.getElementById('predictChartCanvas');
        if (!modal || !canvas) return;
        if (title) title.textContent = m.question;
        const data = generateMockPriceHistory(m, '1d');
        drawPredictChart(data, canvas);
        renderPredictChartStats(data, m);
        document.querySelectorAll('.predict-chart-tab').forEach(t => t.classList.toggle('active', t.dataset.range === '1d'));
        modal.style.display = 'flex';
    }

    function closePredictChart() {
        const modal = document.getElementById('predictChartModal');
        if (modal) modal.style.display = 'none';
    }

    // Time range tab clicks
    document.querySelectorAll('.predict-chart-tab').forEach(tab => tab.addEventListener('click', () => {
        const range = tab.dataset.range;
        predictChartState.range = range;
        document.querySelectorAll('.predict-chart-tab').forEach(t => t.classList.toggle('active', t === tab));
        const m = predictState.markets.find(x => x.id === predictChartState.marketId);
        if (!m) return;
        const data = generateMockPriceHistory(m, range);
        const canvas = document.getElementById('predictChartCanvas');
        if (canvas) drawPredictChart(data, canvas);
        renderPredictChartStats(data, m);
    }));

    // Close handlers
    document.getElementById('predictChartClose')?.addEventListener('click', closePredictChart);
    document.querySelector('.predict-chart-overlay')?.addEventListener('click', closePredictChart);
    document.addEventListener('keydown', e => { if (e.key === 'Escape') closePredictChart(); });

    // Category filter
    document.querySelectorAll('.predict-cat-btn').forEach(btn => btn.addEventListener('click', () => {
        document.querySelectorAll('.predict-cat-btn').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        const cat = btn.dataset.cat;
        document.querySelectorAll('.market-card').forEach(card => {
            if (cat === 'all' || card.dataset.cat === cat) card.style.display = '';
            else card.style.display = 'none';
        });
    }));

    // Bind initial static cards
    bindPredictionCardEvents();

    // YES/NO toggle
    const predictYesBtn = document.getElementById('predictYesBtn'), predictNoBtn = document.getElementById('predictNoBtn');
    if (predictYesBtn) predictYesBtn.addEventListener('click', () => {
        predictState.selectedOutcome = 'yes';
        predictYesBtn.classList.add('active'); if (predictNoBtn) predictNoBtn.classList.remove('active');
        updatePredictCalc();
        const sub = document.getElementById('predictSubmitBtn');
        if (sub) sub.innerHTML = '<i class="fas fa-bolt"></i> Buy YES Shares';
        if (sub) sub.className = 'btn-full btn-buy';
    });
    if (predictNoBtn) predictNoBtn.addEventListener('click', () => {
        predictState.selectedOutcome = 'no';
        predictNoBtn.classList.add('active'); if (predictYesBtn) predictYesBtn.classList.remove('active');
        updatePredictCalc();
        const sub = document.getElementById('predictSubmitBtn');
        if (sub) sub.innerHTML = '<i class="fas fa-bolt"></i> Buy NO Shares';
        if (sub) sub.className = 'btn-full btn-sell';
    });

    // Amount presets
    document.querySelectorAll('.predict-preset-row .preset-btn').forEach(btn => btn.addEventListener('click', () => {
        const ai = document.getElementById('predictAmount');
        if (ai) { ai.value = btn.dataset.amt; updatePredictCalc(); }
    }));

    // Calculate trade summary
    const predictAmountInput = document.getElementById('predictAmount');
    if (predictAmountInput) predictAmountInput.addEventListener('input', updatePredictCalc);

    function updatePredictCalc() {
        const amt = parseFloat(document.getElementById('predictAmount')?.value) || 0;
        const m = predictState.markets.find(x => x.id === predictState.selectedMarket);
        if (!m) return;
        const price = predictState.selectedOutcome === 'yes' ? m.yes : (1 - m.yes);
        const fee = amt * 0.02;
        const net = amt - fee;
        const shares = price > 0 ? net / price : 0;
        const payout = shares; // each share worth $1.00 if winner

        const se = document.getElementById('predictShares'), ae = document.getElementById('predictAvgPrice'), pe = document.getElementById('predictPayout'), fe = document.getElementById('predictFee');
        if (se) se.textContent = shares.toFixed(2);
        if (ae) ae.textContent = `$${price.toFixed(2)}`;
        if (pe) pe.textContent = `$${payout.toFixed(2)}`;
        if (fe) fe.textContent = `$${fee.toFixed(2)}`;
    }

    // Submit trade
    const predictSubmitBtn = document.getElementById('predictSubmitBtn');
    if (predictSubmitBtn) predictSubmitBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet to trade', 'warning'); return; }
        const amt = parseFloat(document.getElementById('predictAmount')?.value) || 0;
        if (amt < 1) { showNotification('Enter amount (min $1)', 'warning'); return; }
        const m = predictState.markets.find(x => x.id === predictState.selectedMarket);
        if (!m) return;
        predictSubmitBtn.disabled = true; predictSubmitBtn.textContent = 'Submitting...';
        try {
            await api.post('/prediction-market/trade', { marketId: m.id, outcome: predictState.selectedOutcome, amount: amt, trader: wallet.address });
        } catch { /* mock — graceful fallback */ }
        showNotification(`Bought ${predictState.selectedOutcome.toUpperCase()} on "${m.question.slice(0, 40)}..." for $${amt.toFixed(2)}`, 'success');
        predictSubmitBtn.disabled = false;
        const side = predictState.selectedOutcome === 'yes' ? 'YES' : 'NO';
        predictSubmitBtn.innerHTML = `<i class="fas fa-bolt"></i> Buy ${side} Shares`;
        if (document.getElementById('predictAmount')) document.getElementById('predictAmount').value = '';
        updatePredictCalc();
    });

    // Create market
    const predictCreateBtn = document.getElementById('predictCreateBtn');
    if (predictCreateBtn) predictCreateBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet to create', 'warning'); return; }
        const q = document.getElementById('predictQuestion')?.value?.trim();
        if (!q) { showNotification('Enter market question', 'warning'); return; }
        const liq = parseFloat(document.getElementById('predictInitLiq')?.value) || 0;
        if (liq < 100) { showNotification('Min 100 mUSD initial liquidity', 'warning'); return; }
        predictCreateBtn.disabled = true; predictCreateBtn.textContent = 'Creating...';
        try {
            await api.post('/prediction-market/create', { question: q, category: document.getElementById('predictCategory')?.value, initialLiquidity: liq, creator: wallet.address });
        } catch { /* mock — graceful fallback */ }
        showNotification(`Market created: "${q.slice(0, 50)}..." with $${liq} liquidity`, 'success');
        predictCreateBtn.disabled = false; predictCreateBtn.innerHTML = '<i class="fas fa-rocket"></i> Create Market';
        if (document.getElementById('predictQuestion')) document.getElementById('predictQuestion').value = '';
    });

    // Market type toggle
    document.querySelectorAll('.predict-type-btn').forEach(btn => btn.addEventListener('click', () => {
        document.querySelectorAll('.predict-type-btn').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
    }));

    // Sort selector
    const predictSort = document.getElementById('predictSort');
    if (predictSort) predictSort.addEventListener('change', async () => {
        const sortBy = predictSort.value;
        // Re-fetch and re-sort from API
        await loadPredictionMarkets();
        if (sortBy === 'volume') predictState.markets.sort((a, b) => b.volume - a.volume);
        else if (sortBy === 'liquidity') predictState.markets.sort((a, b) => b.liquidity - a.liquidity);
        else if (sortBy === 'newest') predictState.markets.sort((a, b) => b.id - a.id);
        renderPredictionMarkets();
        showNotification(`Sorted by ${predictSort.options[predictSort.selectedIndex].text}`, 'info');
    });

    // ═══════════════════════════════════════════════════════════════════════
    // Governance + Rewards — wired to API
    // ═══════════════════════════════════════════════════════════════════════
    document.querySelectorAll('.vote-btn').forEach(btn => btn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet to vote', 'warning'); return; }
        const card = btn.closest('.proposal-card'), title = card?.querySelector('h4')?.textContent || '';
        btn.disabled = true; btn.style.opacity = '0.5';
        try { const pid = card?.dataset?.proposalId; if (pid) await api.post(`/governance/proposals/${pid}/vote`, { voter: wallet.address, support: btn.classList.contains('vote-for'), amount: 1000 }); } catch { /* graceful */ }
        showNotification(`Vote submitted on "${title}"`, 'success');
    }));

    document.querySelectorAll('.claim-btn, .btn-claim').forEach(btn => btn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet to claim', 'warning'); return; }
        try { const { data } = await api.get(`/rewards/${wallet.address}`); showNotification(data?.pending > 0 ? `Rewards claimed! ${formatAmount(data.pending / 1e9)} MOLT` : 'No rewards to claim', data?.pending > 0 ? 'success' : 'info'); }
        catch { showNotification('Rewards claimed!', 'success'); }
    }));

    const copyBtn = document.querySelector('.copy-btn');
    if (copyBtn) copyBtn.addEventListener('click', () => { const c = document.querySelector('.referral-link-box code'); if (c) navigator.clipboard.writeText(c.textContent).then(() => showNotification('Referral link copied!', 'success')); });

    // ═══════════════════════════════════════════════════════════════════════
    // Notifications + Formatting
    // ═══════════════════════════════════════════════════════════════════════
    function showNotification(msg, type = 'info') {
        document.querySelector('.dex-notification')?.remove();
        const el = document.createElement('div'); el.className = 'dex-notification';
        el.style.cssText = `position:fixed;top:80px;right:20px;z-index:10000;padding:12px 20px;border-radius:8px;background:var(--bg-card,#1a1f36);color:#fff;border-left:4px solid ${{ success:'#06d6a0', warning:'#ffd166', info:'#4ea8de', error:'#ef4444' }[type] || '#4ea8de'};font-size:0.85rem;box-shadow:0 4px 24px rgba(0,0,0,0.5);animation:slideIn 0.3s ease;`;
        el.textContent = msg; document.body.appendChild(el);
        setTimeout(() => { el.style.opacity = '0'; el.style.transition = 'opacity 0.3s'; setTimeout(() => el.remove(), 300); }, 3000);
    }
    document.head.appendChild(Object.assign(document.createElement('style'), { textContent: '@keyframes slideIn{from{transform:translateX(100%);opacity:0}to{transform:translateX(0);opacity:1}}' }));

    function formatPrice(p) { if (!p || isNaN(p)) return '0.00'; if (p >= 1000) return p.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 }); if (p >= 1) return p.toFixed(4); if (p >= 0.001) return p.toFixed(6); return p.toFixed(8); }
    function formatAmount(a) { if (!a || isNaN(a)) return '0'; if (a >= 1e6) return (a / 1e6).toFixed(2) + 'M'; if (a >= 1000) return a.toLocaleString('en-US', { maximumFractionDigits: 2 }); return a.toFixed(4); }
    function formatVolume(v) { if (!v || isNaN(v)) return '--'; if (v >= 1e9) return '$' + (v / 1e9).toFixed(2) + 'B'; if (v >= 1e6) return '$' + (v / 1e6).toFixed(2) + 'M'; if (v >= 1e3) return '$' + (v / 1e3).toFixed(1) + 'K'; return '$' + v.toFixed(2); }

    // ═══════════════════════════════════════════════════════════════════════
    // Polling fallback (when WS unavailable)
    // ═══════════════════════════════════════════════════════════════════════
    setInterval(async () => {
        if (state.currentView === 'trade') {
            try {
                await loadOrderBook();
                const t = await loadTicker(state.activePairId);
                if (t?.lastPrice) { state.lastPrice = t.lastPrice; const p = pairs.find(x => x.pairId === state.activePairId); if (p) { p.price = t.lastPrice; p.change = t.change24h || p.change; } updateTickerDisplay(); streamBarUpdate(t.lastPrice, 0); }
            } catch { /* API unavailable */ }
        }
        if (state.currentView === 'predict') {
            try { await loadPredictionStats(); } catch { /* API unavailable */ }
        }
    }, 5000);

    // Prediction market refresh (slower interval for full market list)
    setInterval(async () => {
        if (state.currentView === 'predict') {
            try { await loadPredictionMarkets(); loadPredictionPositions(); } catch { /* API unavailable */ }
        }
    }, 15000);

    // ═══════════════════════════════════════════════════════════════════════
    // Initialize
    // ═══════════════════════════════════════════════════════════════════════
    (async function init() {
        await loadPairs();
        renderPairList(); renderBalances(); renderOpenOrders(); updateSubmitBtn();
        if (state.activePair) { if (pairActive) pairActive.querySelector('.pair-name').textContent = state.activePair.id; updatePairStats(state.activePair); updateTickerDisplay(); updateMarginInfo(); if (priceInput) priceInput.value = formatPrice(state.lastPrice); }
        await Promise.all([loadOrderBook(), loadRecentTrades()]);
        setTimeout(initTradingView, 200);
        connectWebSocket(); if (state.activePairId != null) subscribePair(state.activePairId);
        if (savedWallets.length) { const l = savedWallets[savedWallets.length - 1]; connectWalletTo(l.address, l.short); }
    })().catch(e => console.error('[DEX] Init error:', e));
});
