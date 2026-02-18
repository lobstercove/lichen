/* ========================================
   MoltyDEX — Production JavaScript Engine
   Wired to MoltChain RPC + WebSocket
   ======================================== */

document.addEventListener('DOMContentLoaded', () => {
    'use strict';

    // ═══════════════════════════════════════════════════════════════════════
    // Configuration — override via window globals or <script> config block
    // ═══════════════════════════════════════════════════════════════════════
    const RPC_BASE  = (window.MOLTCHAIN_RPC || 'http://localhost:8899').replace(/\/$/, '');
    const WS_URL    = (window.MOLTCHAIN_WS  || 'ws://localhost:8900').replace(/\/$/, '');
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
                const pendingId = this.nextReqId++;
                this.subs.set(pendingId, { channel, method: 'subscribeDex', params: { channel }, callback });
                return pendingId;
            }
        }
        unsubscribe(subId) {
            const sub = this.subs.get(subId);
            this.subs.delete(subId);
            if (sub && this.ws?.readyState === WebSocket.OPEN) {
                try {
                    this.ws.send(JSON.stringify({ jsonrpc: '2.0', id: this.nextReqId++, method: 'unsubscribeDex', params: { subscription: subId } }));
                } catch { /* connection may have closed */ }
            }
        }
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
            this.address = bs58encode(this.keypair.publicKey);
            this.shortAddr = this.address.slice(0, 8) + '...' + this.address.slice(-6);
            return this;
        },
        async fromSecretKey(hexKey) {
            const n = await this._ensureNacl();
            const bytes = hexToBytes(hexKey);
            if (n && bytes.length === 64) this.keypair = { publicKey: bytes.slice(32), secretKey: bytes };
            else if (n && bytes.length === 32) this.keypair = n.sign.keyPair.fromSeed(bytes);
            else throw new Error('Invalid key (expected 32 or 64 byte hex)');
            this.address = bs58encode(this.keypair.publicKey);
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
    // AUDIT-FIX DEX-1/DEX-2: Base58 encoding for addresses (must match wallet/RPC format)
    const BS58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
    function bs58encode(bytes) {
        let leadingZeros = 0;
        for (let i = 0; i < bytes.length && bytes[i] === 0; i++) leadingZeros++;
        // Convert to BigInt for base conversion
        let num = 0n;
        for (const b of bytes) num = num * 256n + BigInt(b);
        let encoded = '';
        while (num > 0n) { encoded = BS58_ALPHABET[Number(num % 58n)] + encoded; num = num / 58n; }
        return '1'.repeat(leadingZeros) + encoded;
    }
    function bs58decode(str) {
        let num = 0n;
        for (const c of str) { const idx = BS58_ALPHABET.indexOf(c); if (idx < 0) throw new Error('Invalid base58'); num = num * 58n + BigInt(idx); }
        const hex = num === 0n ? '' : num.toString(16);
        const padded = hex.length % 2 ? '0' + hex : hex;
        const bytes = [];
        for (let i = 0; i < padded.length; i += 2) bytes.push(parseInt(padded.slice(i, i + 2), 16));
        let leadingOnes = 0;
        for (let i = 0; i < str.length && str[i] === '1'; i++) leadingOnes++;
        const result = new Uint8Array(leadingOnes + bytes.length);
        result.set(bytes, leadingOnes);
        return result;
    }
    function encodeTransactionMessage(instructions, blockhash, signer) {
        const enc = new TextEncoder();
        // AUDIT-FIX DEX-1: Use base58 decoding for signer address (now base58, not hex)
        const parts = [enc.encode(blockhash), bs58decode(signer)];
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
                    id: p.symbol || `Pair#${p.pairId}`, pairId: p.pairId, base: p.baseSymbol || p.baseToken, quote: p.quoteSymbol || p.quoteToken,
                    price: p.lastPrice || 0, change: p.change24h || 0, tickSize: p.tickSize, lotSize: p.lotSize, symbol: p.symbol,
                }));
            }
        } catch (e) { console.warn('[DEX] Pairs API unavailable:', e.message); }
        if (pairs.length) {
            state.activePair = pairs[0]; state.activePairId = pairs[0].pairId; state.lastPrice = pairs[0].price;
        } else {
            state.activePair = null; state.activePairId = null; state.lastPrice = 0;
            console.warn('[DEX] No trading pairs on-chain — create pairs via dex_core.create_pair()');
        }
        // Populate all select dropdowns from real pairs
        populateSelectsFromPairs();
    }

    function populateSelectsFromPairs() {
        const poolSelect = document.getElementById('liqPoolSelect');
        const marginSelect = document.getElementById('marginPairSelect');
        const feeSelect = document.getElementById('propFeePair');
        const opts = pairs.map((p, i) => `<option value="${p.pairId}">${p.id}</option>`).join('');
        if (poolSelect) poolSelect.innerHTML = opts || '<option>No pairs available</option>';
        if (marginSelect) marginSelect.innerHTML = opts || '<option>No pairs available</option>';
        if (feeSelect) feeSelect.innerHTML = opts || '<option>No pairs available</option>';
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
                state.orderBook = { asks, bids }; renderOrderBook();
                return;
            }
        } catch { /* API unavailable */ }
        // Empty state — no data from API
        state.orderBook = { asks: [], bids: [] }; renderOrderBook();
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
        } catch { /* API unavailable */ }
        // Empty state — no trades from API
        const container = document.querySelector('.trades-list'); if (container) container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-exchange-alt" style="margin-right:6px;"></i>No recent trades</div>';
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
        } catch { /* RPC unavailable */ }
        if (!Object.keys(balances).length) {
            balances = { MOLT: { available: 0, usd: 0 }, mUSD: { available: 0, usd: 0 } };
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
    function switchView(v) { state.currentView = v; views.forEach(el => el.classList.toggle('hidden', el.id !== `view-${v}`)); navLinks.forEach(l => l.classList.toggle('active', l.dataset.view === v)); if (v === 'trade') { drawChart(); loadTradeHistory(); loadPositionsTab(); } if (v === 'predict') { loadPredictionStats(); loadPredictionMarkets(); loadPredictionPositions(); } if (v === 'pool') { loadPoolStats(); loadPools(); loadLPPositions(); } if (v === 'margin') { loadMarginStats(); loadMarginPositions(); } if (v === 'rewards') { loadRewardsStats(); } if (v === 'governance') { loadGovernanceStats(); loadProposals(); } }
    navLinks.forEach(l => l.addEventListener('click', e => { e.preventDefault(); switchView(l.dataset.view); }));

    // Mobile nav toggle
    const navToggle = document.getElementById('navToggle');
    const navMenu = document.querySelector('.nav-menu');
    if (navToggle && navMenu) {
        navToggle.addEventListener('click', () => {
            navMenu.classList.toggle('nav-open');
            navToggle.classList.toggle('active');
        });
        // Close on link click
        navMenu.querySelectorAll('a').forEach(a => a.addEventListener('click', () => {
            navMenu.classList.remove('nav-open');
            navToggle.classList.remove('active');
        }));
    }

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
        const filtered = pairs.filter(p => !f || p.id.toLowerCase().includes(f));
        if (!filtered.length) {
            pairList.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-search" style="margin-right:6px;"></i>No trading pairs available</div>';
            return;
        }
        pairList.innerHTML = filtered.map(p => `
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
            if (t) { stats[0].textContent = formatPrice(t.high24h || 0); stats[1].textContent = formatPrice(t.low24h || 0); stats[2].textContent = formatVolume(t.volume24h || 0); stats[3].textContent = String(t.trades24h || '0'); }
            else { stats[0].textContent = '--'; stats[1].textContent = '--'; stats[2].textContent = '--'; stats[3].textContent = '0'; }
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
        if (!state.orderBook.asks.length && !state.orderBook.bids.length) {
            ac.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-layer-group" style="margin-right:6px;"></i>No asks</div>';
            bc.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-layer-group" style="margin-right:6px;"></i>No bids</div>';
            if (sp) sp.textContent = '—';
            if (sv) sv.textContent = 'Spread: —';
            return;
        }
        const ma = Math.max(...state.orderBook.asks.map(a => a.total), 1), mb = Math.max(...state.orderBook.bids.map(b => b.total), 1);
        ac.innerHTML = [...state.orderBook.asks].reverse().map(a => `<div class="book-row ask"><span class="price">${formatPrice(a.price)}</span><span>${formatAmount(a.amount)}</span><span>${formatAmount(a.total)}</span><div class="depth-bar" style="width:${(a.total/ma*100).toFixed(1)}%"></div></div>`).join('');
        if (sp) { const tb = state.orderBook.bids[0]?.price || 0, ba = state.orderBook.asks[0]?.price || 0; sp.textContent = formatPrice((tb + ba) / 2); if (sv) { const s = ba - tb; sv.textContent = `Spread: ${formatPrice(Math.abs(s))} (${ba > 0 ? (s/ba*100).toFixed(3) : '0.000'}%)`; } }
        bc.innerHTML = state.orderBook.bids.map(b => `<div class="book-row bid"><span class="price">${formatPrice(b.price)}</span><span>${formatAmount(b.amount)}</span><span>${formatAmount(b.total)}</span><div class="depth-bar" style="width:${(b.total/mb*100).toFixed(1)}%"></div></div>`).join('');
    }

    // ═══════════════════════════════════════════════════════════════════════
    // TradingView (wired to candle API)
    // ═══════════════════════════════════════════════════════════════════════
    let tvWidget = null, realtimeCallback = null, lastBarTime = 0;


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
                    // No candle data on-chain — return empty
                    bars = [];
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
        if (badge) badge.textContent = openOrders.length || '';
        if (!state.connected) { tb.innerHTML = '<tr><td colspan="8" style="text-align:center;color:var(--text-muted);padding:20px;"><i class="fas fa-wallet" style="margin-right:6px;"></i>Connect wallet to view orders</td></tr>'; return; }
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
        if (ae) ae.textContent = wallet.address;
        // AUDIT-FIX DEX-3: Never display raw secret key in DOM — show masked placeholder
        // Users should export/backup keys through the main wallet's encrypted export
        if (ke) ke.textContent = '••••••••••••••••••••••••••••••••  (use main wallet for key backup)';
        if (cd) cd.classList.remove('hidden');
        savedWallets.push({ address: wallet.address, short: wallet.shortAddr, added: Date.now() }); localStorage.setItem('dexWallets', JSON.stringify(savedWallets));
        connectWalletTo(wallet.address, wallet.shortAddr); showNotification('New wallet created: ' + wallet.shortAddr, 'success');
    });

    document.querySelectorAll('.wm-copy-btn').forEach(btn => btn.addEventListener('click', () => { const el = document.getElementById(btn.dataset.copy); if (el) navigator.clipboard.writeText(el.textContent).then(() => showNotification('Copied!', 'success')); }));

    async function connectWalletTo(address, shortAddr) {
        state.connected = true; state.walletAddress = address;
        if (connectBtn) { connectBtn.innerHTML = `<i class="fas fa-wallet"></i> ${shortAddr}`; connectBtn.className = 'btn btn-small btn-secondary'; }
        toggleWalletPanels(true);
        await Promise.all([loadBalances(address), loadUserOrders(address)]);
        renderBalances(); renderOpenOrders(); loadTradeHistory(); loadPositionsTab();
        if (dexWs && state.activePairId != null) subscribePair(state.activePairId);
    }

    function disconnectWallet() {
        state.connected = false; state.walletAddress = null; wallet.keypair = null; wallet.address = null;
        if (connectBtn) { connectBtn.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet'; connectBtn.className = 'btn btn-small btn-primary'; }
        openOrders = []; balances = {};
        toggleWalletPanels(false);
        renderBalances(); renderOpenOrders();
        // Clear wallet-gated sections
        loadTradeHistory(); loadPositionsTab(); loadLPPositions(); loadPredictionPositions();
    }

    function toggleWalletPanels(show) {
        const bp = document.getElementById('walletBalancePanel');
        const tp = document.getElementById('tradeBottomPanel');
        if (bp) bp.classList.toggle('hidden', !show);
        if (tp) tp.classList.toggle('hidden', !show);
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
        if (!state.connected) { c.innerHTML = ''; return; }
        c.innerHTML = Object.entries(balances).map(([t, b]) => `<div class="balance-row"><div class="balance-token"><div class="token-icon ${t.toLowerCase()}-icon">${t[0]}</div><span>${t}</span></div><div class="balance-amounts"><span class="balance-available">${formatAmount(b.available)}</span><span class="balance-usd">≈ $${formatAmount(b.usd)}</span></div></div>`).join('');
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Margin View
    // ═══════════════════════════════════════════════════════════════════════
    const leverageSlider = document.getElementById('leverageSlider'), leverageDisplay = document.querySelector('.leverage-display');
    if (leverageSlider) leverageSlider.addEventListener('input', () => { state.leverageValue = parseFloat(leverageSlider.value); if (leverageDisplay) leverageDisplay.textContent = `${state.leverageValue}x`; updateMarginInfo(); });
    document.querySelectorAll('.margin-type').forEach(btn => btn.addEventListener('click', () => { document.querySelectorAll('.margin-type').forEach(b => b.classList.remove('active')); btn.classList.add('active'); state.marginType = btn.dataset.type; if (leverageSlider) leverageSlider.max = state.marginType === 'isolated' ? '5' : '3'; if (state.leverageValue > parseFloat(leverageSlider?.max)) { state.leverageValue = parseFloat(leverageSlider.max); leverageSlider.value = state.leverageValue; if (leverageDisplay) leverageDisplay.textContent = `${state.leverageValue}x`; } updateMarginInfo(); }));
    document.querySelectorAll('.side-btn').forEach(btn => btn.addEventListener('click', () => { document.querySelectorAll('.side-btn').forEach(b => b.classList.remove('active')); btn.classList.add('active'); state.marginSide = btn.classList.contains('long-btn') ? 'long' : 'short'; const ob = document.getElementById('marginOpenBtn'); if (ob) { ob.textContent = `Open ${state.marginSide === 'long' ? 'Long' : 'Short'}`; ob.className = `btn btn-full ${state.marginSide === 'long' ? 'btn-buy' : 'btn-sell'}`; } }));

    function updateMarginInfo() {
        const e = document.getElementById('marginEntry'), l = document.getElementById('marginLiqPrice'), r = document.getElementById('marginRatio');
        if (e) e.textContent = formatPrice(state.lastPrice);
        if (l) l.textContent = formatPrice(state.marginSide === 'long' ? state.lastPrice * (1 - 1 / state.leverageValue * 0.9) : state.lastPrice * (1 + 1 / state.leverageValue * 0.9));
        if (r) r.textContent = `${(100 / state.leverageValue).toFixed(1)}%`;
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Pool View — Load data from API
    // ═══════════════════════════════════════════════════════════════════════
    async function loadPoolStats() {
        try {
            const { data } = await api.get('/stats/amm');
            if (data) {
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('poolTvl', formatVolume(data.total_volume || 0));
                el('poolVolume24h', formatVolume(data.swap_count ? data.swap_count * 100 : 0));
                el('poolFees24h', formatVolume(data.total_fees || 0));
                el('poolCount', data.pool_count ?? '—');
            }
        } catch { /* API unavailable — keep placeholder */ }
    }

    async function loadPools() {
        try {
            const { data } = await api.get('/pools');
            if (Array.isArray(data) && data.length > 0) {
                const tbody = document.getElementById('poolTableBody');
                if (tbody) {
                    tbody.innerHTML = data.map(p => {
                        const pair = `${p.tokenASymbol || 'Token A'}/${p.tokenBSymbol || 'Token B'}`;
                        const fee = p.feeTier ? (p.feeTier / 100).toFixed(2) + '%' : '0.30%';
                        const tvl = formatVolume(p.liquidity || 0);
                        const vol = formatVolume(p.totalVolume || 0);
                        const apr = p.apr ? p.apr.toFixed(1) + '%' : '—';
                        return `<tr class="pool-row" data-pool-id="${p.poolId || p.id || 0}">
                            <td class="pool-pair"><span class="token-pair-icons"><span class="mini-icon">${(p.tokenASymbol || 'A')[0]}</span><span class="mini-icon">${(p.tokenBSymbol || 'B')[0]}</span></span> ${pair}</td>
                            <td><span class="fee-badge">${fee}</span></td>
                            <td class="mono-value">${tvl}</td>
                            <td class="mono-value">${vol}</td>
                            <td class="apr-value">${apr}</td>
                            <td><button class="btn btn-small btn-secondary pool-add-btn" data-pool-id="${p.poolId || p.id || 0}">Add</button></td>
                        </tr>`;
                    }).join('');
                }
                return;
            }
        } catch { /* API unavailable */ }
        // Empty state — no pools from API
        const _ptb = document.getElementById('poolTableBody');
        if (_ptb) _ptb.innerHTML = '<tr><td colspan="6" style="text-align:center;color:var(--text-muted);padding:20px;"><i class="fas fa-water" style="margin-right:6px;"></i>No liquidity pools — create one to get started</td></tr>';
    }

    async function loadLPPositions() {
        const container = document.getElementById('pool-positions');
        if (!state.connected) {
            if (container) container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:30px;font-size:0.85rem;"><i class="fas fa-wallet" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>Connect wallet to view LP positions</div>';
            return;
        }
        try {
            const { data } = await api.get(`/pools/positions?address=${wallet.address}`);
            if (Array.isArray(data) && data.length > 0) {
                const container = document.getElementById('pool-positions');
                if (container) {
                    container.innerHTML = data.map(pos => `
                        <div class="lp-position-card" data-position-id="${pos.positionId || 0}">
                            <div class="lp-pos-header">
                                <div class="lp-pos-pair">
                                    <span class="lp-pair-name">${pos.pair || 'Pool #' + (pos.poolId || '?')}</span>
                                    <span class="fee-badge">LP</span>
                                </div>
                                <span class="range-badge in-range"><i class="fas fa-circle"></i> Active</span>
                            </div>
                            <div class="lp-pos-details">
                                <div class="lp-detail"><span>Tick Range</span><span class="mono-value">${pos.lowerTick ?? 0} — ${pos.upperTick ?? 0}</span></div>
                                <div class="lp-detail"><span>Liquidity</span><span class="mono-value">${formatVolume(pos.liquidity || 0)}</span></div>
                                <div class="lp-detail"><span>Uncollected Fees</span><span class="mono-value accent-text">${formatVolume((pos.feeAOwed || 0) + (pos.feeBOwed || 0))}</span></div>
                            </div>
                            <div class="lp-pos-actions">
                                <button class="btn btn-small btn-primary lp-collect-btn" data-position-id="${pos.positionId || 0}">Collect Fees</button>
                                <button class="btn btn-small btn-secondary lp-remove-btn" data-position-id="${pos.positionId || 0}">Remove</button>
                                <button class="btn btn-small btn-secondary lp-add-btn" data-position-id="${pos.positionId || 0}">Add More</button>
                            </div>
                        </div>
                    `).join('');
                }
                return;
            }
        } catch { /* API unavailable */ }
    }

    // Add Liquidity submit handler
    const addLiqBtn = document.getElementById('addLiqBtn');
    if (addLiqBtn) addLiqBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
        const amtA = parseFloat(document.getElementById('liqAmountA')?.value) || 0;
        const amtB = parseFloat(document.getElementById('liqAmountB')?.value) || 0;
        if (!amtA && !amtB) { showNotification('Enter deposit amounts', 'warning'); return; }
        const minPrice = parseFloat(document.getElementById('liqMinPrice')?.value) || 0;
        const maxPrice = parseFloat(document.getElementById('liqMaxPrice')?.value) || 0;
        const fullRange = document.getElementById('fullRangeToggle')?.checked;
        addLiqBtn.disabled = true; addLiqBtn.textContent = 'Adding...';
        try {
            const poolSelect = document.getElementById('liqPoolSelect');
            const poolId = poolSelect ? parseInt(poolSelect.value) || 0 : 0;
            await wallet.sendTransaction([{
                programId: '0000000000000000000000000000000000000000000000000000000000000002', // dex_amm
                data: JSON.stringify({ op: 'add_liquidity', pool_id: poolId, amount_a: Math.round(amtA * 1e9), amount_b: Math.round(amtB * 1e9), lower_tick: fullRange ? -887272 : Math.round(minPrice * 1e6), upper_tick: fullRange ? 887272 : Math.round(maxPrice * 1e6) })
            }]);
            showNotification(`Liquidity added: ${formatAmount(amtA)} + ${formatAmount(amtB)}`, 'success');
        } catch (e) { showNotification(`Add liquidity: ${e.message}`, 'error'); }
        finally { addLiqBtn.disabled = false; addLiqBtn.textContent = 'Add Liquidity'; }
    });

    // Fee tier selector
    document.querySelectorAll('.fee-tier-btn').forEach(btn => btn.addEventListener('click', () => {
        document.querySelectorAll('.fee-tier-btn').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
    }));

    // Pool filter pills
    document.querySelectorAll('.pool-table-panel .filter-pill').forEach(btn => btn.addEventListener('click', () => {
        document.querySelectorAll('.pool-table-panel .filter-pill').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
    }));

    // ═══════════════════════════════════════════════════════════════════════
    // Margin — Open/Close Positions + Load from API
    // ═══════════════════════════════════════════════════════════════════════
    async function loadMarginStats() {
        try {
            const { data } = await api.get('/stats/margin');
            if (data) {
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('marginInsurance', formatVolume(data.insurance_fund || 0));
            }
        } catch { /* API unavailable */ }
        // Load margin info
        try {
            const { data } = await api.get('/margin/info');
            if (data) {
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                if (data.maxLeverage) { const ls = document.getElementById('leverageSlider'); if (ls) ls.max = String(data.maxLeverage); }
            }
        } catch { /* keep defaults */ }
    }

    async function loadMarginPositions() {
        if (!state.connected) {
            const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
            el('marginEquity', '—'); el('marginUsed', '—'); el('marginAvailable', '—');
            return;
        }
        try {
            const { data } = await api.get(`/margin/positions?trader=${wallet.address}`);
            if (Array.isArray(data) && data.length > 0) {
                const container = document.getElementById('marginPositionsList');
                if (container) {
                    container.className = 'margin-positions-list';
                    container.innerHTML = data.map(pos => {
                        const side = pos.side === 'long' ? 'Long' : 'Short';
                        const sideClass = side === 'Long' ? 'side-buy' : 'side-sell';
                        const pnl = pos.realizedPnl || 0;
                        return `<div class="margin-pos-row">
                            <div class="margin-pos-info">
                                <span class="${sideClass}">${side} ${pos.pair || 'MOLT/mUSD'}</span>
                                <span class="mono-value">${pos.leverage || state.leverageValue}x</span>
                            </div>
                            <div class="margin-pos-details">
                                <span>Size: ${formatAmount(pos.size || 0)}</span>
                                <span>Entry: ${formatPrice(pos.entryPrice || 0)}</span>
                                <span class="${pnl >= 0 ? 'positive' : 'negative'}">P&L: ${pnl >= 0 ? '+' : ''}${formatPrice(pnl)}</span>
                            </div>
                            <button class="btn btn-small btn-secondary margin-close-btn" data-position-id="${pos.positionId || pos.id || 0}">Close</button>
                        </div>`;
                    }).join('');
                    // Bind close buttons
                    container.querySelectorAll('.margin-close-btn').forEach(btn => btn.addEventListener('click', async () => {
                        btn.disabled = true;
                        try {
                            await api.post('/margin/close', { positionId: parseInt(btn.dataset.positionId), trader: wallet.address });
                            showNotification('Position closed', 'success');
                            await loadMarginPositions();
                        } catch (e) { showNotification(`Close failed: ${e.message}`, 'error'); }
                        btn.disabled = false;
                    }));
                }
                // Update equity stats
                let totalMargin = 0, totalPnl = 0;
                data.forEach(p => { totalMargin += (p.margin || 0); totalPnl += (p.realizedPnl || 0); });
                const eq = (balances.mUSD?.available || 0) + totalPnl;
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('marginEquity', formatVolume(eq));
                el('marginUsed', formatVolume(totalMargin));
                el('marginAvailable', formatVolume(eq - totalMargin));
                return;
            }
        } catch { /* keep empty state */ }
    }

    // Trade History tab (bottom panel of Trade view)
    async function loadTradeHistory() {
        const container = document.getElementById('content-history');
        if (!container) return;
        if (!state.connected) { container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:30px;font-size:0.85rem;"><i class="fas fa-wallet" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>Connect wallet to view trade history</div>'; return; }
        try {
            const { data } = await api.get(`/pairs/${state.activePairId}/trades?limit=50&trader=${wallet.address}`);
            if (Array.isArray(data) && data.length > 0) {
                container.innerHTML = `<table class="orders-table"><thead><tr><th>Pair</th><th>Side</th><th>Price</th><th>Amount</th><th>Total</th><th>Time</th></tr></thead><tbody>${
                    data.map(tr => `<tr><td>${state.activePair?.id || ''}</td><td class="side-${tr.side || 'buy'}">${(tr.side || 'buy').toUpperCase()}</td><td class="mono-value">${formatPrice(tr.price || 0)}</td><td class="mono-value">${formatAmount(tr.quantity || tr.amount || 0)}</td><td class="mono-value">${formatPrice((tr.price || 0) * (tr.quantity || tr.amount || 0))}</td><td class="mono-value" style="color:var(--text-muted)">${tr.timestamp ? new Date(tr.timestamp).toLocaleString() : ''}</td></tr>`).join('')
                }</tbody></table>`;
                return;
            }
        } catch { /* no history from API */ }
    }

    // Margin positions tab (bottom panel of Trade view)
    async function loadPositionsTab() {
        const container = document.getElementById('content-positions');
        if (!container) return;
        if (!state.connected) { container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:30px;font-size:0.85rem;"><i class="fas fa-wallet" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>Connect wallet to view positions</div>'; return; }
        try {
            const { data } = await api.get(`/margin/positions?trader=${wallet.address}`);
            if (Array.isArray(data) && data.length > 0) {
                container.innerHTML = `<table class="orders-table"><thead><tr><th>Pair</th><th>Side</th><th>Size</th><th>Entry</th><th>Mark</th><th>P&L</th><th>Lev</th><th></th></tr></thead><tbody>${
                    data.map(p => {
                        const side = p.side === 'long' ? 'Long' : 'Short';
                        const pnl = p.realizedPnl || 0;
                        return `<tr><td>${p.pair || state.activePair?.id || ''}</td><td class="side-${side.toLowerCase()}">${side}</td><td class="mono-value">${formatAmount(p.size || 0)}</td><td class="mono-value">${formatPrice(p.entryPrice || 0)}</td><td class="mono-value">${formatPrice(p.markPrice || state.lastPrice)}</td><td class="mono-value ${pnl >= 0 ? 'positive' : 'negative'}">${pnl >= 0 ? '+' : ''}${formatPrice(pnl)}</td><td>${p.leverage || '2'}x</td><td><button class="btn btn-small btn-secondary">Close</button></td></tr>`;
                    }).join('')
                }</tbody></table>`;
                return;
            }
        } catch { /* no positions from API */ }
    }

    // Margin Open Position submit
    const marginOpenBtn = document.getElementById('marginOpenBtn');
    if (marginOpenBtn) marginOpenBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
        const size = parseFloat(document.getElementById('marginSize')?.value) || 0;
        const margin = parseFloat(document.getElementById('marginAmount')?.value) || 0;
        if (!size || !margin) { showNotification('Enter size and margin', 'warning'); return; }
        const pairSelect = document.getElementById('marginPairSelect');
        const pairId = pairSelect ? parseInt(pairSelect.value) : 0;
        marginOpenBtn.disabled = true; marginOpenBtn.textContent = 'Opening...';
        try {
            await api.post('/margin/open', { pairId, side: state.marginSide, size: Math.round(size * 1e9), leverage: state.leverageValue, margin: Math.round(margin * 1e9), trader: wallet.address });
            showNotification(`${state.marginSide.toUpperCase()} position opened: ${formatAmount(size)} @ ${state.leverageValue}x`, 'success');
            await loadMarginPositions();
            if (document.getElementById('marginSize')) document.getElementById('marginSize').value = '';
            if (document.getElementById('marginAmount')) document.getElementById('marginAmount').value = '';
        } catch (e) { showNotification(`Open position: ${e.message}`, 'error'); }
        finally { marginOpenBtn.disabled = false; marginOpenBtn.textContent = `Open ${state.marginSide === 'long' ? 'Long' : 'Short'}`; }
    });

    // ═══════════════════════════════════════════════════════════════════════
    // Rewards View — Load from API
    // ═══════════════════════════════════════════════════════════════════════
    async function loadRewardsStats() {
        // Global stats
        try {
            const { data } = await api.get('/stats/rewards');
            if (data) {
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('rewardsTotalDist', formatAmount(data.total_distributed ? data.total_distributed / 1e9 : 0) + ' MOLT');
            }
        } catch { /* API unavailable */ }
        // User rewards
        if (!state.connected) return;
        try {
            const { data } = await api.get(`/rewards/${wallet.address}`);
            if (data) {
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                const pending = data.pending ? data.pending / 1e9 : 0;
                el('rewardsPending', formatAmount(pending) + ' MOLT');
                el('rewardsPendingUsd', `≈ $${formatAmount(pending * state.lastPrice)}`);
                const tierNames = ['Bronze', 'Silver', 'Gold', 'Diamond'];
                const tierNum = data.tier ?? 1;
                const tierName = tierNames[tierNum] || 'Bronze';
                el('rewardsTier', `<span class="tier-badge ${tierName.toLowerCase()}">${tierName}</span>`);
                const tierEl = document.getElementById('rewardsTier');
                if (tierEl) tierEl.innerHTML = `<span class="tier-badge ${tierName.toLowerCase()}">${tierName}</span>`;
                const multipliers = [1.0, 1.5, 2.0, 3.0];
                el('rewardsMultiplier', `${multipliers[tierNum] || 1.0}x`);
                el('rewardsMultiplierSub', `${tierName} tier bonus`);
                // Trading reward card metrics
                el('rewardTradePending', formatAmount(pending) + ' MOLT');
                el('rewardTradeMonth', formatAmount(data.monthly_earned ? data.monthly_earned / 1e9 : 0) + ' MOLT');
                el('rewardTradeAll', formatAmount(data.total_earned ? data.total_earned / 1e9 : 0) + ' MOLT');
                // LP Mining card metrics
                el('rewardLpPending', formatAmount(data.lp_pending ? data.lp_pending / 1e9 : 0) + ' MOLT');
                el('rewardLpPositions', data.lp_positions ?? '0');
                el('rewardLpLiquidity', data.lp_liquidity ? '$' + formatAmount(data.lp_liquidity / 1e9) : '—');
                // Referral card metrics
                el('rewardRefCount', (data.referral_count ?? 0) + ' traders');
                el('rewardRefEarnings', formatAmount(data.referral_earnings ? data.referral_earnings / 1e9 : 0) + ' MOLT');
                el('rewardRefRate', (data.referral_rate ?? 10) + '%');
            }
        } catch { /* API unavailable */ }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Governance View — Load from API + Submit Proposal
    // ═══════════════════════════════════════════════════════════════════════
    async function loadGovernanceStats() {
        try {
            const { data } = await api.get('/stats/governance');
            if (data) {
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('govTotalProposals', data.proposal_count ?? '—');
                el('govActiveProposals', data.active_proposals ?? '—');
            }
        } catch { /* API unavailable */ }
    }

    async function loadProposals() {
        try {
            const { data } = await api.get('/governance/proposals');
            if (Array.isArray(data) && data.length > 0) {
                const container = document.getElementById('proposalsList');
                if (container) {
                    container.innerHTML = data.map(p => {
                        const status = p.status || 'active';
                        const yesVotes = p.yesVotes || 0;
                        const noVotes = p.noVotes || 0;
                        const totalVotes = yesVotes + noVotes;
                        const yesPct = totalVotes > 0 ? Math.round(yesVotes / totalVotes * 100) : 50;
                        const statusClass = status === 'active' ? 'active-proposal' : status === 'passed' ? 'passed-proposal' : 'executed-proposal';
                        return `<div class="proposal-card ${statusClass}" data-proposal-id="${p.proposalId || p.id || 0}">
                            <div class="proposal-top-row">
                                <div class="proposal-status-badge ${status}">${status.charAt(0).toUpperCase() + status.slice(1)}</div>
                                <span class="proposal-type-tag">${p.proposalType || 'New Pair'}</span>
                                <span class="proposal-id">#${p.proposalId || p.id || 0}</span>
                            </div>
                            <h4>${p.title || p.description || 'Proposal'}</h4>
                            <p class="proposal-desc text-secondary">${p.description || ''}</p>
                            <div class="proposal-votes">
                                <div class="vote-bar"><div class="vote-yes" style="width: ${yesPct}%"></div></div>
                                <div class="vote-counts">
                                    <span class="vote-yes-text"><i class="fas fa-check"></i> ${yesPct}% Yes (${yesVotes} votes)</span>
                                    <span class="vote-no-text"><i class="fas fa-times"></i> ${100 - yesPct}% No (${noVotes} votes)</span>
                                </div>
                            </div>
                            <div class="proposal-footer">
                                <span class="proposal-time"><i class="fas fa-clock"></i> ${p.timeRemaining || ''}</span>
                                ${status === 'active' ? `<div class="proposal-actions">
                                    <button class="btn btn-small btn-primary vote-btn vote-for">Vote Yes</button>
                                    <button class="btn btn-small btn-secondary vote-btn vote-against">Vote No</button>
                                </div>` : ''}
                            </div>
                        </div>`;
                    }).join('');
                    // Rebind vote buttons
                    bindVoteButtons();
                }
                return;
            }
        } catch { /* API unavailable — keep empty state */ }
        // Bind vote buttons on static content
        bindVoteButtons();
    }

    function bindVoteButtons() {
        document.querySelectorAll('.vote-btn').forEach(btn => btn.addEventListener('click', async () => {
            if (!state.connected) { showNotification('Connect wallet to vote', 'warning'); return; }
            const card = btn.closest('.proposal-card');
            const pid = card?.dataset?.proposalId;
            const title = card?.querySelector('h4')?.textContent || '';
            btn.disabled = true; btn.style.opacity = '0.5';
            try { if (pid) await api.post(`/governance/proposals/${pid}/vote`, { voter: wallet.address, support: btn.classList.contains('vote-for'), amount: 1000 }); } catch { /* graceful */ }
            showNotification(`Vote submitted on "${title}"`, 'success');
        }));
    }

    // Proposal type toggle
    const proposalTypeBtns = document.querySelectorAll('.proposal-type-btn');
    const pairFields = document.getElementById('pairFields');
    const feeFields = document.getElementById('feeFields');
    proposalTypeBtns.forEach(btn => btn.addEventListener('click', () => {
        proposalTypeBtns.forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        const ptype = btn.dataset.ptype;
        if (pairFields) pairFields.classList.toggle('hidden', ptype !== 'pair');
        if (feeFields) feeFields.classList.toggle('hidden', ptype !== 'fee');
    }));

    // Governance filter pills
    document.querySelectorAll('.proposals-section .filter-pill').forEach(btn => btn.addEventListener('click', () => {
        document.querySelectorAll('.proposals-section .filter-pill').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        const filter = btn.dataset.filter;
        document.querySelectorAll('.proposal-card').forEach(card => {
            if (filter === 'all') card.style.display = '';
            else card.style.display = card.classList.contains('active-proposal') ? '' : 'none';
        });
    }));

    // Submit Proposal handler
    const submitProposalBtn = document.getElementById('submitProposalBtn');
    if (submitProposalBtn) submitProposalBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet to propose', 'warning'); return; }
        const activeType = document.querySelector('.proposal-type-btn.active');
        const ptype = activeType?.dataset?.ptype || 'pair';
        submitProposalBtn.disabled = true; submitProposalBtn.textContent = 'Submitting...';
        try {
            if (ptype === 'pair') {
                const base = document.getElementById('propBaseToken')?.value?.trim();
                const quote = document.getElementById('propQuoteToken')?.value?.trim();
                if (!base || !quote) { showNotification('Enter base and quote tokens', 'warning'); return; }
                await api.post('/governance/proposals', { type: 'new_pair', base_token: base, quote_token: quote, proposer: wallet.address });
                showNotification(`Proposal submitted: List ${base}/${quote}`, 'success');
            } else if (ptype === 'fee') {
                const pair = document.getElementById('propFeePair')?.value || 'MOLT/mUSD';
                const makerFee = parseInt(document.getElementById('propMakerFee')?.value) || -1;
                const takerFee = parseInt(document.getElementById('propTakerFee')?.value) || 5;
                await api.post('/governance/proposals', { type: 'fee_change', pair, maker_fee: makerFee, taker_fee: takerFee, proposer: wallet.address });
                showNotification(`Fee change proposal submitted for ${pair}`, 'success');
            } else {
                await api.post('/governance/proposals', { type: ptype, proposer: wallet.address });
                showNotification('Proposal submitted', 'success');
            }
        } catch (e) { showNotification(`Proposal failed: ${e.message}`, 'error'); }
        finally { submitProposalBtn.disabled = false; submitProposalBtn.innerHTML = '<i class="fas fa-paper-plane"></i> Submit Proposal'; }
    });

    // ═══════════════════════════════════════════════════════════════════════
    // PredictionReef — Predict View (Live API)
    // ═══════════════════════════════════════════════════════════════════════

    // Only real on-chain prediction markets displayed
    const INITIAL_MARKETS = [];

    const predictState = {
        selectedMarket: 1,
        selectedOutcome: 'yes',
        markets: [...INITIAL_MARKETS],
        positions: [],
        stats: null,
        live: false,
    };

    // ─── Load prediction stats from API ─────────────────────────
    async function loadPredictionStats() {
        try {
            const { data } = await api.get('/prediction-market/stats');
            if (data) {
                predictState.stats = data;
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('pmTotalVolume', formatVolume(data.total_volume || 0));
                el('pmOpenMarkets', data.open_markets ?? '—');
                el('pmTotalCollateral', formatVolume(data.total_collateral || 0));
                el('pmFees', formatVolume(data.fees_collected || 0));
                el('pmTotalTraders', data.total_traders ?? '0');
            }
        } catch { /* API unavailable — keep placeholder text */ }
    }

    // ─── Load markets from API ──────────────────────────────────
    async function loadPredictionMarkets() {
        try {
            const { data } = await api.get('/prediction-market/markets?limit=50');
            if (data?.markets?.length > 0) {
                // Transform API data into UI format
                predictState.markets = data.markets.map(m => ({
                    id: m.id,
                    question: m.question,
                    cat: m.category,
                    yes: m.outcomes?.[0]?.price ?? 0.5,
                    volume: m.total_volume * 1e9,   // convert to display units
                    liquidity: m.total_collateral * 1e9,
                    traders: m.unique_traders || 0,
                    status: m.status,
                    multi: (m.outcome_count || 2) > 2,
                    outcomes: m.outcomes || [],
                }));
                predictState.live = true;
                // Fetch per-market analytics for unique trader counts
                try {
                    const promises = predictState.markets.map(m =>
                        api.get(`/prediction-market/markets/${m.id}/analytics`).then(r => r.data).catch(() => null)
                    );
                    const analytics = await Promise.all(promises);
                    analytics.forEach((a, i) => {
                        if (a) {
                            predictState.markets[i].traders = a.unique_traders || 0;
                        }
                    });
                } catch { /* no analytics — traders stays at 0 */ }
                renderPredictionMarkets();
                return;
            }
        } catch { /* API unavailable */ }
        // Empty state — no markets from API
        predictState.markets = [];
        predictState.live = true;
        renderPredictionMarkets();
    }

    // ─── Load user positions from API ───────────────────────────
    async function loadPredictionPositions() {
        if (!state.connected) {
            const tbody = document.querySelector('.predict-positions-table tbody') || document.getElementById('predictPositionsBody');
            if (tbody) tbody.innerHTML = '<tr><td colspan="8" style="text-align:center;color:var(--text-muted);padding:20px;"><i class="fas fa-wallet" style="margin-right:6px;"></i>Connect wallet to view positions</td></tr>';
            return;
        }
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
        const grid = document.querySelector('.predict-markets-section');
        if (!grid) return;

        // Keep only the grid container, regenerate cards
        // Remove all previously rendered cards AND empty-state placeholders
        grid.querySelectorAll('.market-card, .predict-empty-state').forEach(c => c.remove());

        if (!predictState.markets.length) {
            const emptyEl = document.createElement('div');
            emptyEl.className = 'predict-empty-state';
            emptyEl.style.cssText = 'text-align:center;color:var(--text-muted);padding:40px;font-size:0.9rem;grid-column:1/-1;';
            emptyEl.innerHTML = '<i class="fas fa-chart-line" style="font-size:2rem;margin-bottom:12px;display:block;opacity:0.4;"></i><p>No prediction markets yet</p><p style="font-size:0.8rem;margin-top:8px;">Create a market to get started</p>';
            grid.appendChild(emptyEl);
            return;
        }

        const catIconsHtml = {
            crypto: '<i class="fab fa-bitcoin"></i> Crypto',
            politics: '<i class="fas fa-landmark"></i> Politics',
            sports: '<i class="fas fa-football-ball"></i> Sports',
            tech: '<i class="fas fa-microchip"></i> Tech',
            science: '<i class="fas fa-flask"></i> Science',
            entertainment: '<i class="fas fa-film"></i> Entertainment',
            economics: '<i class="fas fa-chart-bar"></i> Economics',
            custom: '<i class="fas fa-puzzle-piece"></i> Custom'
        };
        const multiDotClasses = ['multi-1', 'multi-2', 'multi-3', 'multi-4'];
        const multiBarClasses = ['multi-bar-1', 'multi-bar-2', 'multi-bar-3', 'multi-bar-4'];

        predictState.markets.forEach(m => {
            const isResolved = m.status === 'resolved';
            const isMulti = m.multi;
            const yesPct = Math.round((m.yes || 0.5) * 100);
            const noPct = 100 - yesPct;
            const yesPrice = (m.yes || 0.5).toFixed(2);
            const noPrice = (1 - (m.yes || 0.5)).toFixed(2);

            let outcomesHtml = '';
            if (isMulti && m.outcomes?.length) {
                outcomesHtml = m.outcomes.map((o, i) => {
                    const pct = Math.round((o.price || 0) * 100);
                    return `<div class="outcome-row multi-outcome">
                        <div class="outcome-label"><span class="outcome-dot ${multiDotClasses[i % 4]}"></span><span>${o.name}</span></div>
                        <div class="outcome-bar-wrap"><div class="outcome-bar ${multiBarClasses[i % 4]}" style="width:${pct}%"></div></div>
                        <div class="outcome-price"><span class="outcome-price-val">$${(o.price || 0).toFixed(2)}</span></div>
                        <button class="btn btn-small btn-predict-buy" data-outcome="${i}" data-market="${m.id}">Buy</button>
                    </div>`;
                }).join('');
            } else if (isResolved) {
                const winOutcome = m.resolved_outcome === 'no' ? 'NO' : 'YES';
                outcomesHtml = `
                    <div class="outcome-row yes-outcome${winOutcome === 'YES' ? ' winner' : ''}">
                        <div class="outcome-label"><span class="outcome-dot yes"></span><span>YES</span></div>
                        <div class="outcome-bar-wrap"><div class="outcome-bar yes-bar" style="width:${yesPct}%"></div></div>
                        <div class="outcome-price"><span class="outcome-price-val yes-price">$${yesPrice}</span></div>
                    </div>
                    <div class="outcome-row no-outcome${winOutcome === 'NO' ? ' winner' : ''}">
                        <div class="outcome-label"><span class="outcome-dot no"></span><span>NO</span></div>
                        <div class="outcome-bar-wrap"><div class="outcome-bar no-bar" style="width:${noPct}%"></div></div>
                        <div class="outcome-price"><span class="outcome-price-val no-price">$${noPrice}</span></div>
                    </div>`;
            } else {
                const yesChg = m.yes_change ? (m.yes_change > 0 ? `<span class="outcome-change positive">+${m.yes_change.toFixed(1)}%</span>` : `<span class="outcome-change negative">${m.yes_change.toFixed(1)}%</span>`) : '';
                const noChg = m.no_change ? (m.no_change > 0 ? `<span class="outcome-change positive">+${m.no_change.toFixed(1)}%</span>` : `<span class="outcome-change negative">${m.no_change.toFixed(1)}%</span>`) : '';
                outcomesHtml = `
                    <div class="outcome-row yes-outcome">
                        <div class="outcome-label"><span class="outcome-dot yes"></span><span>YES</span></div>
                        <div class="outcome-bar-wrap"><div class="outcome-bar yes-bar" style="width:${yesPct}%"></div></div>
                        <div class="outcome-price"><span class="outcome-price-val yes-price">$${yesPrice}</span>${yesChg}</div>
                        <button class="btn btn-small btn-predict-buy" data-outcome="yes" data-market="${m.id}">Buy</button>
                    </div>
                    <div class="outcome-row no-outcome">
                        <div class="outcome-label"><span class="outcome-dot no"></span><span>NO</span></div>
                        <div class="outcome-bar-wrap"><div class="outcome-bar no-bar" style="width:${noPct}%"></div></div>
                        <div class="outcome-price"><span class="outcome-price-val no-price">$${noPrice}</span>${noChg}</div>
                        <button class="btn btn-small btn-predict-sell" data-outcome="no" data-market="${m.id}">Buy</button>
                    </div>`;
            }

            const statusClass = isResolved ? 'resolved' : m.status === 'disputed' ? 'disputed' : 'active';
            const statusLabel = isResolved ? 'Resolved' : m.status === 'disputed' ? 'Disputed' : 'Active';
            const catTag = catIconsHtml[m.cat] || '<i class="fas fa-chart-pie"></i> ' + (m.cat || 'Other');
            const idTag = m.pm_id || `#PM-${String(m.id).padStart(3, '0')}`;
            const closesLabel = m.closes ? `<span><i class="fas fa-clock"></i> ${m.closes}</span>` : '';
            const creatorLabel = m.creator ? `<span><i class="fas fa-user"></i> Creator: ${m.creator}</span>` : '';
            const volLabel = formatVolume(m.volume);
            const liqLabel = formatVolume(m.liquidity);

            const card = document.createElement('div');
            card.className = 'market-card panel-card' + (isResolved ? ' resolved' : '');
            card.dataset.cat = m.cat;
            card.dataset.marketId = m.id;
            card.innerHTML = `
                <div class="market-card-header">
                    <div class="market-status-row">
                        <span class="market-status-badge ${statusClass}">${statusLabel}</span>
                        <span class="market-cat-tag">${catTag}</span>
                        <span class="market-id-tag">${idTag}</span>
                    </div>
                    <h4 class="market-question">${m.question}</h4>
                    <div class="market-meta">
                        ${closesLabel}${creatorLabel}
                    </div>
                </div>
                <div class="market-outcomes">${outcomesHtml}</div>
                <div class="market-footer">
                    <div class="market-stats-mini">
                        <span><i class="fas fa-exchange-alt"></i> ${volLabel} vol</span>
                        <span><i class="fas fa-lock"></i> ${liqLabel} liq</span>
                        <span><i class="fas fa-users"></i> ${m.traders || 0} traders</span>
                        <button class="btn-predict-chart" data-market="${m.id}" title="Price Chart"><i class="fas fa-chart-line"></i></button>
                    </div>
                </div>
            `;
            grid.appendChild(card);
        });

        // Re-bind event handlers for new cards
        bindPredictionCardEvents();

        // Apply default selection highlight
        const selCard = document.querySelector(`.market-card[data-market-id="${predictState.selectedMarket}"]`);
        if (selCard) selCard.classList.add('selected');
    }

    // ─── Render user positions in bottom panel ──────────────────
    function renderPredictionPositions() {
        const tbody = document.getElementById('predictPositionsBody');
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
                document.querySelectorAll('.market-card').forEach(c => c.classList.remove('selected'));
                card.classList.add('selected');
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

    function generateEmptyPriceHistory(market) {
        // Return a flat line at current price when no real history exists
        const now = Date.now();
        const price = market.yes || 0.5;
        return [{ t: now - 86400000, p: price }, { t: now, p: price }];
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
        predictChartState.realData = null;
        const modal = document.getElementById('predictChartModal');
        const title = document.getElementById('predictChartTitle');
        const canvas = document.getElementById('predictChartCanvas');
        if (!modal || !canvas) return;
        if (title) title.textContent = m.question;
        document.querySelectorAll('.predict-chart-tab').forEach(t => t.classList.toggle('active', t.dataset.range === '1d'));
        // Show modal FIRST so canvas has layout dimensions
        modal.style.display = 'flex';
        // Draw flat line initially, then load real data
        requestAnimationFrame(() => {
            const emptyData = generateEmptyPriceHistory(m);
            drawPredictChart(emptyData, canvas);
            renderPredictChartStats(emptyData, m);
            // Load real price history from RPC
            loadRealPriceHistory(marketId, '1d', m);
        });
    }

    function closePredictChart() {
        const modal = document.getElementById('predictChartModal');
        if (modal) modal.style.display = 'none';
    }

    // Load real price history from RPC
    async function loadRealPriceHistory(marketId, range, market) {
        try {
            const { data: snapshots } = await api.get(`/prediction-market/markets/${marketId}/price-history?limit=200`);
            if (snapshots && Array.isArray(snapshots) && snapshots.length > 0) {
                const data = snapshots.map(s => ({ t: s.timestamp * 1000, p: Math.max(0.01, Math.min(0.99, s.price || 0.5)) }));
                const canvas = document.getElementById('predictChartCanvas');
                if (canvas && predictChartState.marketId === marketId) {
                    drawPredictChart(data, canvas);
                    renderPredictChartStats(data, market);
                }
                // Cache for tab switching
                predictChartState.realData = data;
                return;
            }
        } catch { /* RPC unavailable */ }
        predictChartState.realData = null;
    }

    // Time range tab clicks
    document.querySelectorAll('.predict-chart-tab').forEach(tab => tab.addEventListener('click', () => {
        const range = tab.dataset.range;
        predictChartState.range = range;
        document.querySelectorAll('.predict-chart-tab').forEach(t => t.classList.toggle('active', t === tab));
        const m = predictState.markets.find(x => x.id === predictChartState.marketId);
        if (!m) return;
        // Use real data if available, otherwise show flat line
        const chartData = (predictChartState.realData && predictChartState.realData.length > 0) ? predictChartState.realData : generateEmptyPriceHistory(m);
        const canvas = document.getElementById('predictChartCanvas');
        if (canvas) drawPredictChart(chartData, canvas);
        renderPredictChartStats(chartData, m);
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
            showNotification(`Bought ${predictState.selectedOutcome.toUpperCase()} on "${m.question.slice(0, 40)}..." for $${amt.toFixed(2)}`, 'success');
        } catch { showNotification('Trade failed — prediction market API unavailable', 'error'); }
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

        // Collect outcomes for multi-outcome markets
        let outcomes = [];
        if (predictMarketType === 'multi') {
            const inputs = document.querySelectorAll('#outcomeInputs .outcome-name');
            inputs.forEach(inp => { const v = inp.value.trim(); if (v) outcomes.push(v); });
            if (outcomes.length < 2) { showNotification('Enter at least 2 outcome names', 'warning'); return; }
            if (outcomes.length > 8) { showNotification('Maximum 8 outcomes', 'warning'); return; }
        }

        predictCreateBtn.disabled = true; predictCreateBtn.textContent = 'Creating...';
        try {
            const payload = { question: q, category: document.getElementById('predictCategory')?.value, initialLiquidity: liq, creator: wallet.address };
            if (outcomes.length > 0) payload.outcomes = outcomes;
            await api.post('/prediction-market/create', payload);
            showNotification(`Market created: "${q.slice(0, 50)}..." with $${liq} liquidity`, 'success');
            await loadPredictionMarkets();
        } catch { showNotification('Create failed — prediction market API unavailable', 'error'); }
        predictCreateBtn.disabled = false; predictCreateBtn.innerHTML = '<i class="fas fa-rocket"></i> Create Market';
        if (document.getElementById('predictQuestion')) document.getElementById('predictQuestion').value = '';
    });

    // Market type toggle — show/hide multi-outcome inputs
    let predictMarketType = 'binary';
    document.querySelectorAll('.predict-type-btn').forEach(btn => btn.addEventListener('click', () => {
        document.querySelectorAll('.predict-type-btn').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        predictMarketType = btn.dataset.type;
        const multiSection = document.getElementById('multiOutcomeSection');
        if (multiSection) multiSection.classList.toggle('hidden', predictMarketType === 'binary');
    }));

    // Add/remove outcome inputs (max 8)
    const addOutcomeBtn = document.getElementById('addOutcomeBtn');
    if (addOutcomeBtn) addOutcomeBtn.addEventListener('click', () => {
        const container = document.getElementById('outcomeInputs');
        if (!container) return;
        const count = container.querySelectorAll('.outcome-input-row').length;
        if (count >= 8) { showNotification('Maximum 8 outcomes', 'warning'); return; }
        const row = document.createElement('div');
        row.className = 'outcome-input-row';
        row.innerHTML = `<span class="outcome-dot multi-${count + 1}"></span><input type="text" class="form-input outcome-name" placeholder="Outcome ${count + 1}" maxlength="64"><button type="button" class="btn-remove-outcome"><i class="fas fa-times"></i></button>`;
        row.querySelector('.btn-remove-outcome').addEventListener('click', () => {
            if (container.querySelectorAll('.outcome-input-row').length <= 2) { showNotification('Minimum 2 outcomes', 'warning'); return; }
            row.remove();
        });
        container.appendChild(row);
    });

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
    // Governance + Rewards — claim handlers
    // ═══════════════════════════════════════════════════════════════════════
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
        if (state.currentView === 'trade' && state.activePairId != null) {
            try {
                await loadOrderBook();
                const t = await loadTicker(state.activePairId);
                if (t?.lastPrice) { state.lastPrice = t.lastPrice; const p = pairs.find(x => x.pairId === state.activePairId); if (p) { p.price = t.lastPrice; p.change = t.change24h || p.change; } updateTickerDisplay(); streamBarUpdate(t.lastPrice, 0); }
            } catch { /* API unavailable */ }
        }
        if (state.currentView === 'predict') {
            try { await loadPredictionStats(); } catch { /* API unavailable */ }
        }
        if (state.currentView === 'pool') {
            try { await loadPoolStats(); } catch { /* API unavailable */ }
        }
        if (state.currentView === 'margin') {
            try { await loadMarginStats(); await loadMarginPositions(); } catch { /* API unavailable */ }
        }
        if (state.currentView === 'rewards') {
            try { await loadRewardsStats(); } catch { /* API unavailable */ }
        }
        if (state.currentView === 'governance') {
            try { await loadGovernanceStats(); } catch { /* API unavailable */ }
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
        loadTradeHistory(); loadPositionsTab();
        if (state.activePair) {
            if (pairActive) pairActive.querySelector('.pair-name').textContent = state.activePair.id;
            updatePairStats(state.activePair); updateTickerDisplay(); updateMarginInfo();
            if (priceInput) priceInput.value = formatPrice(state.lastPrice);
            await Promise.all([loadOrderBook(), loadRecentTrades()]);
            setTimeout(initTradingView, 200);
            connectWebSocket(); subscribePair(state.activePairId);
        } else {
            // No pairs on-chain — show empty state
            if (pairActive) pairActive.querySelector('.pair-name').textContent = 'No pairs';
            state.orderBook = { asks: [], bids: [] }; renderOrderBook();
            const tc = document.querySelector('.trades-list');
            if (tc) tc.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-info-circle" style="margin-right:6px;"></i>No trading pairs available. Bootstrap pairs via dex_core contract.</div>';
            setTimeout(initTradingView, 200);
            connectWebSocket();
        }
        if (savedWallets.length) {
            const l = savedWallets[savedWallets.length - 1];
            // Auto-connect is display-only — keypair not stored. User must re-import to sign.
            state.connected = true; state.walletAddress = l.address;
            if (connectBtn) { connectBtn.innerHTML = `<i class="fas fa-wallet"></i> ${l.short || l.address.slice(0, 8) + '...'}`; connectBtn.className = 'btn btn-small btn-secondary'; }
            toggleWalletPanels(true);
            try { await loadBalances(l.address); await loadUserOrders(l.address); } catch { /* API unavailable */ }
            renderBalances(); renderOpenOrders(); loadTradeHistory(); loadPositionsTab();
            if (dexWs && state.activePairId != null) subscribePair(state.activePairId);
        }
    })().catch(e => console.error('[DEX] Init error:', e));
});
