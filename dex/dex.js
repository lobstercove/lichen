/* ========================================
   Lichen DEX — Production JavaScript Engine
   Wired to Lichen RPC + WebSocket
   ======================================== */

document.addEventListener('DOMContentLoaded', () => {
    'use strict';

    // ═══════════════════════════════════════════════════════════════════════
    // Configuration — override via window globals or <script> config block
    // Uses LICHEN_CONFIG (shared-config.js) for environment-aware defaults.
    // ═══════════════════════════════════════════════════════════════════════
    const _licnRpc = (typeof LICHEN_CONFIG !== 'undefined') ? LICHEN_CONFIG.rpc('dexNetwork') : 'http://localhost:8899';
    const _licnWs = (typeof LICHEN_CONFIG !== 'undefined') ? LICHEN_CONFIG.ws('dexNetwork') : 'ws://localhost:8900';
    const RPC_BASE = (localStorage.getItem('dexRpcUrl') || window.LICHEN_RPC || _licnRpc).replace(/\/$/, '');
    const WS_URL = (localStorage.getItem('dexWsUrl') || window.LICHEN_WS || _licnWs).replace(/\/$/, '');
    const API_BASE = `${RPC_BASE}/api/v1`;
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
            // F20.1: Check HTTP status before parsing JSON (avoids confusing SyntaxError on HTML error pages)
            if (!res.ok) throw new Error(`RPC HTTP ${res.status}: ${await res.text().catch(() => 'Unknown error')}`);
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
            this.subs = new Map(); // channel -> { subId, method, params, callback }
            this.pending = [];
            this.nextReqId = 1;
            this.pendingReqs = new Map();
            this.reconnectDelay = 1000;
            this.reconnectTimer = null;
            this.connected = false;
            this._closing = false;
            this.connect();
        }
        connect() {
            if (this._closing) return;
            if (this.reconnectTimer) {
                clearTimeout(this.reconnectTimer);
                this.reconnectTimer = null;
            }
            try { this.ws = new WebSocket(this.url); } catch {
                this.reconnectTimer = setTimeout(() => this.connect(), this.reconnectDelay);
                return;
            }
            this.ws.onopen = () => {
                this.reconnectDelay = 1000;
                this.connected = true;
                if (this.onConnectionChange) this.onConnectionChange(true);
                for (const [channel, sub] of this.subs) {
                    this._sendSubscribe(sub.method, sub.params)
                        .then((newSubId) => { sub.subId = newSubId; this.subs.set(channel, sub); })
                        .catch(() => { });
                }
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
                    if (msg.method === 'subscription' && msg.params) {
                        const sub = Array.from(this.subs.values()).find((entry) => entry.subId === msg.params.subscription);
                        if (sub?.callback) sub.callback(msg.params.result);
                    }
                    if (msg.channel && msg.data) {
                        const sub = this.subs.get(msg.channel);
                        if (sub?.callback) sub.callback(msg.data);
                        else {
                            for (const [, fallbackSub] of this.subs) {
                                if (fallbackSub.channel === msg.channel && fallbackSub.callback) fallbackSub.callback(msg.data);
                            }
                        }
                    }
                } catch { /* ignore */ }
            };
            this.ws.onclose = () => {
                this.connected = false;
                this.ws = null;
                if (this.onConnectionChange) this.onConnectionChange(false);
                if (this._closing) return;
                this.reconnectTimer = setTimeout(() => {
                    this.reconnectTimer = null;
                    this.connect();
                }, this.reconnectDelay);
                this.reconnectDelay = Math.min(this.reconnectDelay * 2, 30000);
            };
            this.ws.onerror = () => { };
        }
        _sendSubscribe(method, params) {
            const id = this.nextReqId++;
            const msg = JSON.stringify({ jsonrpc: '2.0', id, method, params });
            if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(msg);
            else this.pending.push(msg);
            return new Promise((resolve, reject) => {
                this.pendingReqs.set(id, { resolve, reject });
                setTimeout(() => { if (this.pendingReqs.has(id)) { this.pendingReqs.delete(id); reject(new Error('timeout')); } }, 10000);
            });
        }
        async subscribeWithMethod(channelKey, method, params, unsubscribeMethod, callback) {
            const existing = this.subs.get(channelKey);
            if (existing) {
                existing.callback = callback;
                this.subs.set(channelKey, existing);
                return existing.subId || channelKey;
            }

            const entry = { channel: channelKey, method, params, unsubscribeMethod, callback, subId: null };
            this.subs.set(channelKey, entry);

            try {
                const subId = await this._sendSubscribe(method, params);
                entry.subId = subId;
                this.subs.set(channelKey, entry);
                return subId || channelKey;
            } catch {
                return channelKey;
            }
        }
        async subscribe(channel, callback) {
            return this.subscribeWithMethod(channel, 'subscribeDex', { channel }, 'unsubscribeDex', callback);
        }
        async subscribePrediction(channel, callback) {
            const key = `prediction:${channel}`;
            return this.subscribeWithMethod(key, 'subscribePrediction', { channel }, 'unsubscribePrediction', callback);
        }
        unsubscribe(subscriptionOrChannel) {
            let channel = null;
            let sub = null;

            if (this.subs.has(subscriptionOrChannel)) {
                channel = subscriptionOrChannel;
                sub = this.subs.get(channel);
            } else {
                for (const [key, value] of this.subs) {
                    if (value.subId === subscriptionOrChannel) {
                        channel = key;
                        sub = value;
                        break;
                    }
                }
            }

            if (!channel || !sub) return;
            this.subs.delete(channel);

            if (sub.subId && this.ws?.readyState === WebSocket.OPEN) {
                try {
                    this.ws.send(JSON.stringify({
                        jsonrpc: '2.0',
                        id: this.nextReqId++,
                        method: sub.unsubscribeMethod || 'unsubscribeDex',
                        params: { subscription: sub.subId }
                    }));
                } catch { /* connection may have closed */ }
            }
        }
        close() {
            this._closing = true;
            this.connected = false;
            if (this.reconnectTimer) {
                clearTimeout(this.reconnectTimer);
                this.reconnectTimer = null;
            }
            if (this.ws) { this.ws.onclose = null; this.ws.close(); this.ws = null; }
            this.pendingReqs.forEach(({ reject }) => reject(new Error('WebSocket closed')));
            this.pendingReqs.clear();
            this.pending = [];
            this.subs.clear();
        }
    }

    let dexWs = null;

    // ═══════════════════════════════════════════════════════════════════════
    // Wallet
    // ═══════════════════════════════════════════════════════════════════════
    const wallet = {
        keypair: null, address: null, shortAddr: null, _lichenWallet: null,

        async _ensureWallet() {
            if (this._lichenWallet) return this._lichenWallet;
            // Try wallet extension first
            if (typeof LichenWallet !== 'undefined') {
                this._lichenWallet = new LichenWallet({ rpcUrl: RPC_BASE });
                return this._lichenWallet;
            }
            return null;
        },
        async connect() {
            const w = await this._ensureWallet();
            if (!w) throw new Error('Lichen Wallet extension not found — install it from the wallet page');
            const result = await w.connect();
            if (result && result.address) {
                this.address = result.address;
                this.shortAddr = this.address.slice(0, 8) + '...' + this.address.slice(-6);
                this.keypair = { connected: true }; // Compatibility flag — no actual key material
                this.signingReady = true;
                return this;
            }
            throw new Error('Wallet connection failed');
        },
        async connectAddress(addr, options = {}) {
            const { signingReady = false, localKeypair = null } = options;
            // Connect to a known address (e.g. from saved wallets list) — read-only until extension signs
            this.address = addr;
            this.shortAddr = addr.slice(0, 8) + '...' + addr.slice(-6);
            this.signingReady = !!signingReady;
            this.keypair = localKeypair || (this.signingReady ? { connected: true } : null);
            return this;
        },
        /** Import wallet from hex or base58 private key */
        async fromSecretKey(secretInput) {
            const text = (secretInput || '').trim();
            if (!text) throw new Error('Private key is required');
            let bytes;
            try {
                const hex = text.startsWith('0x') ? text.slice(2) : text;
                if (/^[0-9a-fA-F]+$/.test(hex) && (hex.length === 64 || hex.length === 128)) {
                    bytes = hexToBytes(hex);
                } else {
                    bytes = bs58decode(text);
                }
            } catch {
                throw new Error('Invalid private key format (expected hex or base58)');
            }

            let kp;
            if (bytes.length === 64) {
                kp = nacl.sign.keyPair.fromSecretKey(bytes);
            } else if (bytes.length === 32) {
                kp = nacl.sign.keyPair.fromSeed(bytes);
            } else {
                throw new Error('Private key must be 32-byte seed or 64-byte Ed25519 secret key');
            }

            this.keypair = kp;
            this.address = bs58encode(kp.publicKey);
            this.shortAddr = this.address.slice(0, 8) + '...' + this.address.slice(-6);
            this.signingReady = true;
            return this;
        },
        /** Generate a fresh Ed25519 keypair */
        async generate() {
            const kp = nacl.sign.keyPair();
            this.keypair = kp;
            this.address = bs58encode(kp.publicKey);
            this.shortAddr = this.address.slice(0, 8) + '...' + this.address.slice(-6);
            this.signingReady = true;
            return this;
        },
        sign(message) {
            if (!this.keypair || !this.keypair.secretKey) throw new Error('No local keypair available for signing');
            return nacl.sign.detached(message, this.keypair.secretKey);
        },
        async sendTransaction(instructions) {
            if (!this.address) throw new Error('Wallet not connected');
            if (!this.signingReady) throw new Error('Signing session not active. Reconnect wallet extension to sign.');
            // Prefer local keypair (import/create)
            if (this.keypair && this.keypair.secretKey) {
                const blockhash = await api.rpc('getRecentBlockhash');
                const normalizedIx = instructions.map(ix => {
                    const accounts = ix.accounts || [this.address];
                    const dataBytes = typeof ix.data === 'string' ? Array.from(new TextEncoder().encode(ix.data)) : Array.from(ix.data);
                    return { program_id: ix.program_id, accounts, data: dataBytes };
                });
                const msg = encodeTransactionMessage(normalizedIx, blockhash, this.address);
                const sig = this.sign(msg);
                const txPayload = {
                    signatures: [bytesToHex(sig)],
                    message: { instructions: normalizedIx, blockhash: blockhash },
                };
                const txBase64 = btoa(String.fromCharCode(...new TextEncoder().encode(JSON.stringify(txPayload))));
                return api.rpc('sendTransaction', [txBase64]);
            }
            // Delegate signing to wallet extension
            const w = await this._ensureWallet();
            if (w && typeof w.sendTransaction === 'function') {
                return w.sendTransaction(instructions);
            }
            // Fallback: build unsigned TX and request extension signature
            if (typeof window !== 'undefined' && window.Lichen && window.Lichen.Wallet) {
                return window.Lichen.Wallet.signAndSend(instructions);
            }
            // Last resort: submit via RPC (for server-side wallets)
            const blockhash = await api.rpc('getRecentBlockhash');
            const normalizedIx = instructions.map(ix => {
                const accounts = ix.accounts || [this.address];
                const dataBytes = typeof ix.data === 'string' ? Array.from(new TextEncoder().encode(ix.data)) : Array.from(ix.data);
                return { program_id: ix.program_id, accounts, data: dataBytes };
            });
            const txPayload = {
                signatures: [],
                message: { instructions: normalizedIx, blockhash: blockhash },
                signer: this.address,
            };
            const txBase64 = btoa(String.fromCharCode(...new TextEncoder().encode(JSON.stringify(txPayload))));
            return api.rpc('sendTransaction', [txBase64]);
        },
    };

    function walletCanSign() {
        return !!(wallet.address && wallet.signingReady);
    }

    function bytesToHex(b) { return Array.from(b).map(x => x.toString(16).padStart(2, '0')).join(''); }
    function keypairSeedHex(keypair) {
        if (!keypair || !keypair.secretKey) return '';
        return bytesToHex(keypair.secretKey.slice(0, 32));
    }
    function hexToBytes(h) {
        const c = h.startsWith('0x') ? h.slice(2) : h;
        // F20.14: Validate hex format before parsing
        if (!/^[0-9a-fA-F]*$/.test(c)) throw new Error('Key must be hexadecimal');
        if (c.length % 2 !== 0) throw new Error('Key has odd number of hex characters');
        const o = new Uint8Array(c.length / 2); for (let i = 0; i < o.length; i++) o[i] = parseInt(c.slice(i * 2, i * 2 + 2), 16); return o;
    }

    // F20.13: Retry utility for transient network errors on write operations
    async function withRetry(fn, maxRetries = 2, delay = 1000) {
        for (let i = 0; i <= maxRetries; i++) {
            try { return await fn(); }
            catch (e) { if (i === maxRetries) throw e; await new Promise(r => setTimeout(r, delay * (i + 1))); }
        }
    }

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

    function walletAddressToHex(addr) {
        if (!addr) return '';
        try {
            return bytesToHex(bs58decode(addr)).toLowerCase();
        } catch {
            return '';
        }
    }

    function formatAddressShort(addr) {
        const text = String(addr || '');
        if (!text) return '—';
        if (text.length <= 16) return text;
        return `${text.slice(0, 8)}...${text.slice(-6)}`;
    }

    // ═══════════════════════════════════════════════════════════════════════
    // F8.2 SYSTEMIC FIX: Contract call infrastructure
    // All contract calls must use CONTRACT_PROGRAM_ID (0xFF*32),
    // include [caller, contract] in accounts, and serialize data as
    // ContractInstruction::Call { function: "call", args, value: 0 }
    // ═══════════════════════════════════════════════════════════════════════
    const CONTRACT_PROGRAM_ID = bs58encode(new Uint8Array(32).fill(0xFF));

    // Build ContractInstruction::Call JSON (matches Rust's serde serialization)
    function buildContractCall(argsBytes, value = 0) {
        return JSON.stringify({ Call: { function: "call", args: Array.from(argsBytes), value } });
    }

    // Build a sendTransaction instruction with correct program_id + accounts
    function contractIx(contractAddr, argsBytes, value = 0) {
        return {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: [wallet.address, contractAddr],
            data: buildContractCall(argsBytes, value),
        };
    }

    // Build a named-export contract call (for SporePump ABI which uses function names, not opcodes)
    function namedCallIx(contractAddr, funcName, argsBytes, value = 0) {
        const data = JSON.stringify({ Call: { function: funcName, args: Array.from(argsBytes), value } });
        return {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: [wallet.address, contractAddr],
            data,
        };
    }

    // Binary encoding helpers
    function writeU64LE(view, offset, n) {
        const bn = BigInt(Math.round(n));
        view.setBigUint64(offset, bn, true);
    }
    function writeI32LE(view, offset, n) {
        view.setInt32(offset, n, true);
    }
    function writeU8(arr, offset, n) {
        arr[offset] = n & 0xFF;
    }
    function writePubkey(arr, offset, base58Addr) {
        const bytes = bs58decode(base58Addr);
        arr.set(bytes.subarray(0, 32), offset);
    }

    // ── DEX Core instruction builders ──
    // Opcode 2: place_order(trader, pair_id, side, type, price, qty, expiry, trigger_price)
    // Order types: 0=limit, 1=market, 2=stop-limit, 3=post-only
    function buildPlaceOrderArgs(trader, pairId, side, orderType, price, quantity, stopPrice, reduceOnly) {
        const buf = new ArrayBuffer(75);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 2); // opcode
        writePubkey(arr, 1, trader);
        writeU64LE(view, 33, pairId);
        writeU8(arr, 41, side === 'buy' ? 0 : 1);
        // Map order type string to contract constant
        let typeByte = 0; // ORDER_LIMIT
        if (orderType === 'market') typeByte = 1;      // ORDER_MARKET
        else if (orderType === 'stop-limit') typeByte = 2;  // ORDER_STOP_LIMIT
        else if (orderType === 'post-only') typeByte = 3;   // ORDER_POST_ONLY
        // G2-04: OR reduce-only flag (0x80) onto type byte
        if (reduceOnly) typeByte |= 0x80;
        writeU8(arr, 42, typeByte);
        writeU64LE(view, 43, price);
        writeU64LE(view, 51, quantity);
        writeU64LE(view, 59, 0); // expiry: 0 = no expiry
        writeU64LE(view, 67, stopPrice || 0); // trigger_price for stop-limit orders
        return arr;
    }

    // Opcode 3: cancel_order(trader, order_id)
    function buildCancelOrderArgs(trader, orderId) {
        const buf = new ArrayBuffer(41);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 3); // opcode
        writePubkey(arr, 1, trader);
        writeU64LE(view, 33, orderId);
        return arr;
    }

    // Opcode 16: modify_order(caller[32], order_id[8], new_price[8], new_qty[8])
    function buildModifyOrderArgs(trader, orderId, newPrice, newQty) {
        const buf = new ArrayBuffer(57);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 16); // opcode
        writePubkey(arr, 1, trader);
        writeU64LE(view, 33, orderId);
        writeU64LE(view, 41, newPrice);
        writeU64LE(view, 49, newQty);
        return arr;
    }

    // Opcode 17: cancel_all_orders(caller[32], pair_id[8])
    function buildCancelAllOrdersArgs(trader, pairId) {
        const buf = new ArrayBuffer(41);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 17); // opcode
        writePubkey(arr, 1, trader);
        writeU64LE(view, 33, pairId);
        return arr;
    }

    // ── DEX AMM instruction builders ──
    // Opcode 3: add_liquidity(provider, pool_id, lower_tick, upper_tick, amount_a, amount_b)
    function buildAddLiquidityArgs(provider, poolId, lowerTick, upperTick, amountA, amountB) {
        const buf = new ArrayBuffer(65);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 3); // opcode
        writePubkey(arr, 1, provider);
        writeU64LE(view, 33, poolId);
        writeI32LE(view, 41, lowerTick);
        writeI32LE(view, 45, upperTick);
        writeU64LE(view, 49, amountA);
        writeU64LE(view, 57, amountB);
        return arr;
    }

    // Opcode 4: remove_liquidity(provider, position_id, liquidity_amount)
    function buildRemoveLiquidityArgs(provider, positionId, liquidityAmount) {
        const buf = new ArrayBuffer(49);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 4); // opcode
        writePubkey(arr, 1, provider);
        writeU64LE(view, 33, positionId);
        writeU64LE(view, 41, liquidityAmount);
        return arr;
    }

    // Opcode 5: collect_fees(provider, position_id)
    function buildCollectFeesArgs(provider, positionId) {
        const buf = new ArrayBuffer(41);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 5); // opcode
        writePubkey(arr, 1, provider);
        writeU64LE(view, 33, positionId);
        return arr;
    }

    // ── DEX Margin instruction builders ──
    // Opcode 2: open_position(trader, pair_id, side, size, leverage, margin, margin_mode)
    function buildOpenPositionArgs(trader, pairId, side, size, leverage, margin, marginType = 'isolated') {
        const buf = new ArrayBuffer(67);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 2); // opcode
        writePubkey(arr, 1, trader);
        writeU64LE(view, 33, pairId);
        writeU8(arr, 41, side === 'long' ? 0 : 1);
        writeU64LE(view, 42, size);
        writeU64LE(view, 50, leverage);
        writeU64LE(view, 58, margin);
        writeU8(arr, 66, marginType === 'cross' ? 1 : 0);
        return arr;
    }

    // Opcode 3: close_position(caller, position_id)
    function buildClosePositionArgs(caller, positionId) {
        const buf = new ArrayBuffer(41);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 3); // opcode
        writePubkey(arr, 1, caller);
        writeU64LE(view, 33, positionId);
        return arr;
    }

    // Opcode 27: close_position_limit(caller, position_id, limit_price)
    function buildClosePositionLimitArgs(caller, positionId, limitPrice) {
        const buf = new ArrayBuffer(49);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 27); // opcode
        writePubkey(arr, 1, caller);
        writeU64LE(view, 33, positionId);
        writeU64LE(view, 41, limitPrice);
        return arr;
    }

    // Opcode 28: partial_close_limit(caller, position_id, close_amount, limit_price)
    function buildPartialCloseLimitArgs(caller, positionId, closeAmount, limitPrice) {
        const buf = new ArrayBuffer(57);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 28); // opcode
        writePubkey(arr, 1, caller);
        writeU64LE(view, 33, positionId);
        writeU64LE(view, 41, closeAmount);
        writeU64LE(view, 49, limitPrice);
        return arr;
    }

    // Opcode 25: partial_close(caller[32], position_id[8], close_amount[8])
    function buildPartialCloseArgs(caller, positionId, closeAmount) {
        const buf = new ArrayBuffer(49);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 25); // opcode
        writePubkey(arr, 1, caller);
        writeU64LE(view, 33, positionId);
        writeU64LE(view, 41, closeAmount);
        return arr;
    }

    // Opcode 4: add_margin(caller[32], position_id[8], amount[8])
    function buildAddMarginArgs(caller, positionId, amount) {
        const buf = new ArrayBuffer(49);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 4); // opcode
        writePubkey(arr, 1, caller);
        writeU64LE(view, 33, positionId);
        writeU64LE(view, 41, amount);
        return arr;
    }

    // Opcode 5: remove_margin(caller[32], position_id[8], amount[8])
    function buildRemoveMarginArgs(caller, positionId, amount) {
        const buf = new ArrayBuffer(49);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 5); // opcode
        writePubkey(arr, 1, caller);
        writeU64LE(view, 33, positionId);
        writeU64LE(view, 41, amount);
        return arr;
    }

    // Opcode 6: liquidate(liquidator[32], position_id[8])
    function buildLiquidateArgs(liquidator, positionId) {
        const buf = new ArrayBuffer(41);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 6); // opcode
        writePubkey(arr, 1, liquidator);
        writeU64LE(view, 33, positionId);
        return arr;
    }

    // Opcode 24: set_position_sl_tp(caller[32], position_id[8], sl_price[8], tp_price[8])
    function buildSetPositionSlTpArgs(caller, positionId, slPrice, tpPrice) {
        const buf = new ArrayBuffer(57);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 24); // opcode
        writePubkey(arr, 1, caller);
        writeU64LE(view, 33, positionId);
        writeU64LE(view, 41, slPrice || 0);
        writeU64LE(view, 49, tpPrice || 0);
        return arr;
    }

    // ── Governance instruction builders ──
    // Opcode 2: vote(voter, proposal_id, support)
    function buildVoteArgs(voter, proposalId, support) {
        const buf = new ArrayBuffer(42);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 2); // opcode
        writePubkey(arr, 1, voter);
        writeU64LE(view, 33, proposalId);
        writeU8(arr, 41, support ? 1 : 0);
        return arr;
    }

    // Task 6.1: Opcode 3: finalize_proposal(proposal_id)
    function buildFinalizeProposalArgs(proposalId) {
        const buf = new ArrayBuffer(9);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 3); // opcode
        writeU64LE(view, 1, proposalId);
        return arr;
    }

    // Task 6.2: Opcode 4: execute_proposal(proposal_id)
    function buildExecuteProposalArgs(proposalId) {
        const buf = new ArrayBuffer(9);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 4); // opcode
        writeU64LE(view, 1, proposalId);
        return arr;
    }

    // ── Prediction Market instruction builders ──
    // Opcode 4: buy_shares(buyer, market_id, outcome, amount)
    function buildBuySharesArgs(buyer, marketId, outcome, amount) {
        const buf = new ArrayBuffer(50);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 4); // opcode
        writePubkey(arr, 1, buyer);
        writeU64LE(view, 33, marketId);
        writeU8(arr, 41, outcome);
        writeU64LE(view, 42, amount);
        return arr;
    }

    // Opcode 13: redeem_shares(user, market_id, outcome)
    function buildRedeemSharesArgs(user, marketId, outcome) {
        const buf = new ArrayBuffer(42);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 13); // opcode
        writePubkey(arr, 1, user);
        writeU64LE(view, 33, marketId);
        writeU8(arr, 41, outcome);
        return arr;
    }

    // F12.3 FIX: Use opcode 8 (submit_resolution) — proper resolution path
    // dao_resolve (opcode 11) requires admin/DAO; submit_resolution works for any resolver with reputation
    // Layout: op[0]=8, resolver[1:33], market_id[33:41], winning_outcome[41], attestation_hash[42:74], bond[74:82] = 82 bytes
    function buildResolveMarketArgs(caller, marketId, winningOutcome) {
        const buf = new ArrayBuffer(82);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 8); // opcode 8 = submit_resolution
        writePubkey(arr, 1, caller);
        writeU64LE(view, 33, marketId);
        writeU8(arr, 41, winningOutcome);
        // attestation_hash: 32 zero bytes (oracle verification skipped when not configured)
        // bond: DISPUTE_BOND = 100_000_000 (100 lUSD)
        writeU64LE(view, 74, 100_000_000);
        return arr;
    }

    // Opcode 1: create_market(creator, category, close_slot, outcome_count, question_hash, question)
    async function buildCreateMarketArgs(creator, question, category, outcomeCount, closeSlot) {
        const encoder = new TextEncoder();
        const qBytes = encoder.encode(question);
        const totalLen = 79 + qBytes.length;
        const buf = new ArrayBuffer(totalLen);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 1); // opcode
        writePubkey(arr, 1, creator);
        // F11.1 FIX: Category map matches contract (politics=0, sports=1, crypto=2, ...)
        const catMap = { politics: 0, sports: 1, crypto: 2, science: 3, entertainment: 4, economics: 5, tech: 6, custom: 7 };
        writeU8(arr, 33, catMap[category] ?? 0);
        // F11.2 FIX: close_slot must be > current_slot; caller must provide valid slot
        writeU64LE(view, 34, closeSlot || 0);
        writeU8(arr, 42, outcomeCount || 2);
        // question_hash: SHA-256(question), must match prediction contract validation
        const digest = await crypto.subtle.digest('SHA-256', qBytes);
        const hashBytes = new Uint8Array(digest);
        arr.set(hashBytes, 43);
        view.setUint32(75, qBytes.length, true); // question_len
        arr.set(qBytes, 79);
        return arr;
    }

    // F12.8 FIX: Opcode 2: add_initial_liquidity(provider, market_id, amount_musd)
    // Layout: op[0]=2, provider[1:33], market_id[33:41], amount_musd[41:49] = 49 bytes min
    function buildAddInitialLiquidityArgs(provider, marketId, amountMusd) {
        const buf = new ArrayBuffer(49);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 2); // opcode
        writePubkey(arr, 1, provider);
        writeU64LE(view, 33, marketId);
        writeU64LE(view, 41, amountMusd);
        return arr;
    }

    // Task 8.1: Opcode 9: challenge_resolution(challenger, market_id, evidence_hash, bond)
    // Layout: op[0]=9, challenger[1:33], market_id[33:41], evidence_hash[41:73], bond[73:81] = 81 bytes
    function buildChallengeResolutionArgs(challenger, marketId, evidenceHash) {
        const buf = new ArrayBuffer(81);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 9); // opcode 9 = challenge_resolution
        writePubkey(arr, 1, challenger);
        writeU64LE(view, 33, marketId);
        // evidence_hash: 32 bytes — hash of challenge evidence
        if (evidenceHash && evidenceHash.length === 32) {
            arr.set(evidenceHash, 41);
        } else {
            // If string evidence provided, hash it into 32 bytes
            const encoder = new TextEncoder();
            const evBytes = encoder.encode(evidenceHash || '');
            const hashBytes = new Uint8Array(32);
            for (let i = 0; i < evBytes.length; i++) hashBytes[i % 32] ^= evBytes[i];
            arr.set(hashBytes, 41);
        }
        // bond: DISPUTE_BOND = 100_000_000 (100 lUSD)
        writeU64LE(view, 73, 100_000_000);
        return arr;
    }

    // Task 8.1: Opcode 10: finalize_resolution(caller, market_id)
    // Layout: op[0]=10, caller[1:33], market_id[33:41] = 41 bytes
    function buildFinalizeResolutionArgs(caller, marketId) {
        const buf = new ArrayBuffer(41);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 10); // opcode 10 = finalize_resolution
        writePubkey(arr, 1, caller);
        writeU64LE(view, 33, marketId);
        return arr;
    }

    // ── Rewards instruction builders ──
    // Opcode 2: claim_trading_rewards(trader)
    function buildClaimRewardsArgs(trader) {
        const buf = new ArrayBuffer(33);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 2); // opcode
        writePubkey(arr, 1, trader);
        return arr;
    }

    // Opcode 3: claim_lp_rewards(provider[32], position_id[8])
    function buildClaimLpRewardsArgs(provider, positionId) {
        const buf = new ArrayBuffer(41);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 3); // opcode
        writePubkey(arr, 1, provider);
        writeU64LE(view, 33, positionId);
        return arr;
    }

    // ── SporePump (Launchpad) instruction builders ──
    // Uses named-export ABI — function names instead of opcode bytes
    // create_token(creator_ptr[32], fee_paid[8]) = 40 bytes
    function buildCPCreateTokenArgs(creator) {
        const buf = new ArrayBuffer(40);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writePubkey(arr, 0, creator);
        writeU64LE(view, 32, 10_000_000_000); // 10 LICN creation fee
        return arr;
    }
    // buy(buyer_ptr[32], token_id[8], licn_amount[8]) = 48 bytes
    function buildCPBuyArgs(buyer, tokenId, licnSpores) {
        const buf = new ArrayBuffer(48);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writePubkey(arr, 0, buyer);
        writeU64LE(view, 32, tokenId);
        writeU64LE(view, 40, licnSpores);
        return arr;
    }
    // sell(seller_ptr[32], token_id[8], token_amount[8]) = 48 bytes
    function buildCPSellArgs(seller, tokenId, tokenSpores) {
        const buf = new ArrayBuffer(48);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writePubkey(arr, 0, seller);
        writeU64LE(view, 32, tokenId);
        writeU64LE(view, 40, tokenSpores);
        return arr;
    }
    // get_token_info(token_id[8]) = 8 bytes
    function buildCPGetTokenInfoArgs(tokenId) {
        const buf = new ArrayBuffer(8);
        const view = new DataView(buf);
        writeU64LE(view, 0, tokenId);
        return new Uint8Array(buf);
    }
    // get_buy_quote(token_id[8], licn_amount[8]) = 16 bytes
    function buildCPGetBuyQuoteArgs(tokenId, licnSpores) {
        const buf = new ArrayBuffer(16);
        const view = new DataView(buf);
        writeU64LE(view, 0, tokenId);
        writeU64LE(view, 8, licnSpores);
        return new Uint8Array(buf);
    }

    // ── Tick math for AMM (Uniswap V3 style) ──
    const MIN_TICK = -887272;
    const MAX_TICK = 887272;
    function sqrtPriceQ32ToPrice(sqrtPrice) {
        if (!sqrtPrice) return 0;
        const sqrtP = sqrtPrice / (2 ** 32);
        return sqrtP * sqrtP;
    }
    function priceToTick(price) {
        if (price <= 0) return MIN_TICK;
        return Math.floor(Math.log(price) / Math.log(1.0001));
    }
    function alignTickToSpacing(tick, spacing) {
        return Math.floor(tick / spacing) * spacing;
    }
    // Fee tier → tick spacing mapping (matches contract)
    const FEE_TIER_SPACING = { 1: 1, 5: 10, 30: 60, 100: 200 };
    // AUDIT-FIX F10.9: Bincode-compatible message serialization for signing.
    // Must match Rust's bincode::serialize(Message { instructions, recent_blockhash })
    // where Message/Instruction use Vec (u64 LE length prefix) and fixed [u8; 32] arrays.
    function encodeTransactionMessage(instructions, blockhash, signer) {
        const parts = [];
        // Helper: write u64 LE
        function pushU64LE(n) {
            const buf = new ArrayBuffer(8);
            const view = new DataView(buf);
            view.setUint32(0, n & 0xFFFFFFFF, true);
            view.setUint32(4, Math.floor(n / 0x100000000) & 0xFFFFFFFF, true);
            parts.push(new Uint8Array(buf));
        }
        // instructions: Vec<Instruction> — u64 length + each instruction
        pushU64LE(instructions.length);
        for (const ix of instructions) {
            // program_id: [u8; 32] — base58 decoded
            parts.push(bs58decode(ix.program_id));
            // accounts: Vec<Pubkey> — u64 length + each [u8; 32]
            const accounts = ix.accounts || [signer];
            pushU64LE(accounts.length);
            for (const acct of accounts) parts.push(bs58decode(acct));
            // data: Vec<u8> — u64 length + raw bytes
            const dataBytes = typeof ix.data === 'string' ? new TextEncoder().encode(ix.data) : ix.data;
            pushU64LE(dataBytes.length);
            parts.push(dataBytes);
        }
        // recent_blockhash: [u8; 32] — hex decoded
        parts.push(hexToBytes(blockhash));
        // compute_budget: Option<u64> = None (0x00)
        parts.push(new Uint8Array([0x00]));
        // compute_unit_price: Option<u64> = None (0x00)
        parts.push(new Uint8Array([0x00]));
        // Concatenate all parts
        const total = parts.reduce((s, a) => s + a.length, 0);
        const out = new Uint8Array(total);
        let off = 0;
        for (const a of parts) { out.set(a, off); off += a.length; }
        return out;
    }

    // ═══════════════════════════════════════════════════════════════════════
    // State
    // ═══════════════════════════════════════════════════════════════════════
    // F10E.6: LICN genesis price — $0.10 per LICN at network launch.
    // Used ONLY as bootstrap fallback before any on-chain trading data exists.
    // Once the oracle or first trade provides a real price, this value is never used.
    const LICHEN_GENESIS_PRICE = 0.10;
    let _genesisOverridden = false;
    const MAX_OPEN_ORDERS_PER_USER = 100;
    const GOVERNANCE_SLOT_SECONDS = 0.4;
    const GOVERNANCE_MIN_QUORUM_DEFAULT = 3;
    const ENABLE_EXTERNAL_PRICE_WS = localStorage.getItem('dexEnableExternalPriceWs') === '1';

    // Display-inversion helpers for LICN-quoted wrapped pairs.
    // On-chain pairs are stored as wBNB/LICN (base=wBNB, quote=LICN) with price
    // in LICN/wBNB (~5850). The UI displays these as LICN/wBNB with price in
    // wBNB/LICN (~0.000171). All data from the API/WS must be inverted to match
    // the display convention before reaching the chart.
    function isDisplayInvertedPair(pair) {
        if (!pair) return false;
        const base = (pair.base || '').toUpperCase();
        const quote = (pair.quote || '').toUpperCase();
        return quote === 'LICN' && ['WSOL', 'WETH', 'WBNB'].includes(base);
    }

    function invertPrice(price) {
        return (price && price > 0) ? 1 / price : 0;
    }

    function invertCandle(c) {
        if (!c || !c.open) return c;
        return {
            time: c.time,
            open: invertPrice(c.open),
            high: invertPrice(c.low),    // high and low swap when inverting
            low: invertPrice(c.high),
            close: invertPrice(c.close),
            volume: c.volume
        };
    }

    const state = {
        activePair: null, activePairId: 0, orderSide: 'buy', orderType: 'limit',
        marginSide: 'long', marginType: 'isolated', chartInterval: '15m', chartType: 'candle',
        currentView: 'trade', leverageValue: 2, lastPrice: LICHEN_GENESIS_PRICE, orderBook: { asks: [], bids: [] },
        candles: [], connected: false, tradeMode: 'spot', _wsSubs: [], _predictWsSub: null, _predictWsRefreshTimer: null, _predictCountdownTimer: null, _marginRealtimeRefreshTimer: null, marginMaxLeverage: 100,
        _predictSlotAnchor: { slot: 0, ts: 0 },
        governanceMinQuorum: GOVERNANCE_MIN_QUORUM_DEFAULT,
        // Task 5.1: Slippage tolerance (loaded from localStorage)
        slippagePct: parseFloat(localStorage.getItem('dexSlippage')) || 0.5,
        // Task 5.2: Notification preferences
        notifPrefs: (() => { try { return JSON.parse(localStorage.getItem('dexNotifPrefs')) || { fills: true, partials: true, liquidation: true }; } catch { return { fills: true, partials: true, liquidation: true }; } })(),
    };
    let pairs = [], balances = {}, openOrders = [];

    // AUDIT-FIX F10.10: Contract addresses loaded from RPC symbol registry.
    // These are base58-encoded 32-byte pubkeys — the actual deployed addresses
    // from deploy-manifest.json, resolved at runtime via getSymbolRegistry.
    const contracts = {
        dex_core: null, dex_amm: null, dex_router: null, dex_margin: null,
        dex_rewards: null, dex_governance: null, dex_analytics: null, prediction_market: null,
        sporepump: null,
    };

    async function loadContractAddresses() {
        try {
            const result = await api.rpc('getAllSymbolRegistry', [100]);
            if (result?.entries?.length) {
                const map = {};
                for (const e of result.entries) map[e.symbol] = e.program;
                contracts.dex_core = map['DEX'] || null;
                contracts.dex_amm = map['DEXAMM'] || null;
                contracts.dex_router = map['DEXROUTER'] || null;
                contracts.dex_margin = map['DEXMARGIN'] || null;
                contracts.dex_rewards = map['DEXREWARDS'] || null;
                contracts.dex_governance = map['DEXGOV'] || null;
                contracts.dex_analytics = map['ANALYTICS'] || null;
                contracts.prediction_market = map['PREDICT'] || null;
                contracts.sporepump = map['SPOREPUMP'] || null;
                // Contract addresses loaded from symbol registry
            }
        } catch (e) {
            console.warn('[DEX] Symbol registry unavailable, trying deploy manifest:', e.message);
        }
        // If symbol registry is unavailable, use hardcoded fallback addresses as safety net.
        // Registry is always preferred; fallbacks are a resilience measure for cold-start scenarios.
        let needsFallback = false;
        if (!contracts.dex_core) {
            needsFallback = true;
            contracts.dex_core = '7QvQ1dxFTdSk9aSzbBe2gHCJH1bSRBDwVdPTn9M5iCds';
            contracts.dex_amm = '72AvbSmnkv82Bsci9BHAufeAGMTycKQX5Y6DL9ghTHay';
            contracts.dex_router = 'FwAxYo2bKmCe1c5gZZjvuyopJMDgm1T9CAWr2svB1GPf';
            contracts.prediction_market = 'J8sMvYFXW4ZCHc488KJ1zmZq1sQMTWyWfr8qnzUwwEyD';
            console.warn('[DEX] Using fallback contract addresses — registry unavailable');
            const banner = document.getElementById('dexWarningBanner');
            if (banner) {
                banner.textContent = '⚠ Using fallback contract addresses — symbol registry unavailable.';
                banner.style.display = 'block';
            }
        }
    }

    // ─── Load contract protocol params and populate dynamic UI text ──────
    async function loadProtocolParams() {
        const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
        const pill = (id, v) => { const e = document.getElementById(id); if (e) e.innerHTML = e.innerHTML.replace(/>([^<]+)$/, '> ' + v); };
        try {
            const { data } = await api.get('/stats/governance');
            if (data) {
                const votePeriod = data.voting_period_hours ?? 48;
                const timelock = data.execution_timelock_hours ?? 1;
                const minQuorum = Number(data.min_quorum ?? data.minQuorum ?? GOVERNANCE_MIN_QUORUM_DEFAULT);
                const threshold = data.approval_threshold ?? 66;
                const minRep = data.min_reputation ?? 500;
                const minLiq = data.min_listing_liquidity ?? 100000;
                const minHolders = data.min_token_holders ?? 10;
                const maxPools = data.max_pools ?? 100;
                const feeTreasury = data.fee_split_treasury ?? 40;
                const feeLps = data.fee_split_lps ?? 30;
                const feeStakers = data.fee_split_stakers ?? 30;
                const predFee = data.prediction_fee ?? 2;

                if (Number.isFinite(minQuorum) && minQuorum > 0) {
                    state.governanceMinQuorum = minQuorum;
                }

                el('govReqReputation', `≥ ${formatNumber(minRep)}`);
                el('govReqMinLiq', `${formatNumber(minLiq)} LICN`);
                el('govReqMinHolders', String(minHolders));
                el('govReqVotePeriod', `${votePeriod} hours`);
                el('govReqTimelock', `${timelock} hour${timelock !== 1 ? 's' : ''}`);
                el('pmReqReputation', `≥ ${formatNumber(minRep)}`);
                el('pmReqDisputePeriod', `${votePeriod} hours`);
                el('predictFeeLabel', `Fee (${predFee}%)`);

                const feeSplitPill = document.getElementById('poolFeeSplitPill');
                if (feeSplitPill) feeSplitPill.innerHTML = `<i class="fas fa-chart-pie"></i> Fee Split: ${feeTreasury}% Treasury · ${feeLps}% LPs · ${feeStakers}% Stakers`;
                const maxPoolsPill = document.getElementById('poolMaxPoolsPill');
                if (maxPoolsPill) maxPoolsPill.innerHTML = `<i class="fas fa-layer-group"></i> Max ${formatNumber(maxPools)} Pools`;
            }
        } catch { /* governance stats unavailable — keep defaults */ }

        try {
            const { data } = await api.get('/launchpad/config');
            if (data) {
                const creationFee = data.creation_fee ?? 10;
                const gradThreshold = data.graduation_threshold ?? 100000;
                const platformFee = data.platform_fee_pct ?? 1;
                const feePill = document.getElementById('launchFeePill');
                if (feePill) feePill.innerHTML = `<i class="fas fa-coins"></i> ${creationFee} LICN to launch`;
                const gradPill = document.getElementById('launchGradPill');
                if (gradPill) gradPill.innerHTML = `<i class="fas fa-trophy"></i> Graduates at ${formatNumber(gradThreshold)} LICN mcap`;
            }
        } catch { /* launchpad config unavailable — keep defaults */ }

        try {
            const { data } = await api.get('/prediction-market/config');
            if (data) {
                const minRep = data.min_reputation ?? 500;
                const minLiq = data.min_collateral ?? 100;
                const feePct = data.trading_fee_bps ? (data.trading_fee_bps / 100) : 2;
                const repPill = document.getElementById('pmRepPill');
                if (repPill) repPill.innerHTML = `<i class="fas fa-id-badge"></i> ${formatNumber(minRep)}+ LichenID rep to create`;
                el('pmReqMinLiq', `${formatNumber(minLiq)} lUSD`);
                el('pmReqReputation', `≥ ${formatNumber(minRep)}`);
                el('predictFeeLabel', `Fee (${feePct}%)`);
            }
        } catch { /* prediction config unavailable — keep defaults */ }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Data Loading
    // ═══════════════════════════════════════════════════════════════════════
    function normalizePairDisplay(baseSymbol, quoteSymbol, fallbackSymbol) {
        const base = String(baseSymbol || '').trim();
        const quote = String(quoteSymbol || '').trim();

        const display = (sym) => {
            const upper = String(sym || '').toUpperCase();
            if (upper === 'WBNB') return 'BNB';
            return sym || '';
        };

        if (!base || !quote) {
            const id = fallbackSymbol || `${base || 'LICN'}/${quote || 'lUSD'}`;
            return { id, baseDisplay: display(base), quoteDisplay: display(quote) };
        }

        const quoteUpper = quote.toUpperCase();
        const baseUpper = base.toUpperCase();

        if (quoteUpper === 'LICN' && ['WSOL', 'WETH', 'WBNB'].includes(baseUpper)) {
            return { id: `LICN/${base}`, baseDisplay: 'LICN', quoteDisplay: display(base) };
        }

        if (baseUpper === 'WBNB' && quoteUpper === 'LUSD') {
            return { id: 'BNB/lUSD', baseDisplay: 'BNB', quoteDisplay: 'lUSD' };
        }

        const id = fallbackSymbol || `${base}/${quote}`;
        return { id, baseDisplay: display(base), quoteDisplay: display(quote) };
    }

    async function loadPairs() {
        try {
            const { data } = await api.get('/pairs');
            if (Array.isArray(data) && data.length > 0) {
                pairs = data.map(p => {
                    const pair = {
                        ...normalizePairDisplay(
                            p.baseSymbol || p.baseToken,
                            p.quoteSymbol || p.quoteToken,
                            p.symbol || `${p.baseSymbol || p.baseToken || 'LICN'}/${p.quoteSymbol || p.quoteToken || 'lUSD'}`
                        ),
                        pairId: p.pairId,
                        base: p.baseSymbol || p.baseToken,
                        quote: p.quoteSymbol || p.quoteToken,
                        price: p.lastPrice || 0, change: p.change24h ?? 0, tickSize: p.tickSize, lotSize: p.lotSize, symbol: p.symbol,
                        hasMarketPrice: (p.lastPrice || 0) > 0,
                    };
                    // Invert API price for display-inverted pairs (on-chain wBNB/LICN → display LICN/wBNB)
                    if (isDisplayInvertedPair(pair) && pair.price > 0) {
                        pair.price = invertPrice(pair.price);
                    }
                    return pair;
                });
            }
        } catch (e) { console.warn('[DEX] Pairs API unavailable:', e.message); }
        if (pairs.length) {
            // Task 5.4: Restore last selected pair from localStorage
            const savedPairId = parseInt(localStorage.getItem('dexLastPair'));
            const savedPair = savedPairId ? pairs.find(p => p.pairId === savedPairId) : null;
            if (savedPair) {
                state.activePair = savedPair; state.activePairId = savedPair.pairId;
                state.lastPrice = savedPair.hasMarketPrice
                    ? savedPair.price
                    : ((savedPair.id === 'LICN/lUSD' || savedPair.base === 'LICN') ? LICHEN_GENESIS_PRICE : 0);
            } else {
                state.activePair = pairs[0]; state.activePairId = pairs[0].pairId;
                state.lastPrice = pairs[0].hasMarketPrice
                    ? pairs[0].price
                    : ((pairs[0].id === 'LICN/lUSD' || pairs[0].base === 'LICN') ? LICHEN_GENESIS_PRICE : 0);
            }
            // F10E.6: Ensure pairs with zero price get genesis fallback (only before first real trade)
            pairs.forEach(p => {
                if (!p.price) {
                    p.price = (!_genesisOverridden && (p.id === 'LICN/lUSD' || p.base === 'LICN')) ? LICHEN_GENESIS_PRICE : 0;
                } else if (p.id === 'LICN/lUSD' || p.base === 'LICN') {
                    _genesisOverridden = true; // real price from chain — disable genesis fallback
                }
            });
        } else {
            // F10E.6: No pairs from API — create genesis LICN/lUSD pair
            pairs = [{ pairId: 1, id: 'LICN/lUSD', base: 'LICN', quote: 'lUSD', price: LICHEN_GENESIS_PRICE, change: 0, tickSize: 0.0001, lotSize: 0.01, symbol: 'LICN/lUSD', hasMarketPrice: false }];
            state.activePair = pairs[0]; state.activePairId = 1; state.lastPrice = LICHEN_GENESIS_PRICE;
        }
        // Populate all select dropdowns from real pairs
        populateSelectsFromPairs();
    }

    function populateSelectsFromPairs() {
        const poolSelect = document.getElementById('liqPoolSelect');
        const feeSelect = document.getElementById('propFeePair');
        const delistSelect = document.getElementById('propDelistPair');
        const opts = pairs.map((p, i) => `<option value="${escapeHtml(String(p.pairId))}">${escapeHtml(p.id)}</option>`).join('');
        if (poolSelect) poolSelect.innerHTML = opts || '<option>No pairs available</option>';
        if (feeSelect) feeSelect.innerHTML = opts || '<option>No pairs available</option>';
        if (delistSelect) delistSelect.innerHTML = opts || '<option>No pairs available</option>';
    }

    async function loadOrderBook() {
        try {
            const { data } = await api.get(`/pairs/${state.activePairId}/orderbook?depth=20`);
            if (data?.asks && data?.bids) {
                const inv = isDisplayInvertedPair(state.activePair);
                const map = arr => arr.map(a => ({ price: inv ? invertPrice(+a.price) : +a.price, amount: +(a.quantity || a.amount || 0) / 1e9, total: 0 }));
                let asks, bids;
                if (inv) {
                    // Inverted: on-chain bids become display asks, on-chain asks become display bids
                    asks = map(data.bids); asks.sort((a, b) => a.price - b.price);
                    let t = 0; asks.forEach(a => { t += a.amount; a.total = t; });
                    bids = map(data.asks); bids.sort((a, b) => b.price - a.price);
                    t = 0; bids.forEach(b => { t += b.amount; b.total = t; });
                } else {
                    asks = map(data.asks); asks.sort((a, b) => a.price - b.price);
                    let t = 0; asks.forEach(a => { t += a.amount; a.total = t; });
                    bids = map(data.bids); bids.sort((a, b) => b.price - a.price);
                    t = 0; bids.forEach(b => { t += b.amount; b.total = t; });
                }
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
                const inv = isDisplayInvertedPair(state.activePair);
                container.innerHTML = data.map(tr => {
                    const buy = tr.side === 'buy'; const rawPrice = +tr.price || 0;
                    const price = inv ? invertPrice(rawPrice) : rawPrice;
                    const amount = (tr.quantity || tr.amount || 0) / 1e9;
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
            if (Array.isArray(data) && data.length > 0) {
                const invert = isDisplayInvertedPair(state.activePair);
                return data.map(c => {
                    const bar = { time: (c.timestamp || c.time || 0) * 1000, open: c.open || 0, high: c.high || 0, low: c.low || 0, close: c.close || 0, volume: c.volume || 0 };
                    return invert ? invertCandle(bar) : bar;
                });
            }
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
                // F19.4b: Use spendable (excludes staked/locked) instead of total spores
                if (result.spendable !== undefined) {
                    balances['LICN'] = { available: result.spendable / 1e9, usd: (result.spendable / 1e9) * state.lastPrice };
                } else if (result.spores !== undefined) {
                    balances['LICN'] = { available: result.spores / 1e9, usd: (result.spores / 1e9) * state.lastPrice };
                }
            }
            // F19.4a: Fetch token balances via getTokenAccounts
            const tokenResult = await api.rpc('getTokenAccounts', [address]);
            if (tokenResult && tokenResult.accounts) {
                for (const ta of tokenResult.accounts) {
                    if (ta.symbol && ta.symbol !== 'LICN') {
                        const decimals = ta.decimals ?? 9;
                        const amt = ta.ui_amount || (ta.balance / Math.pow(10, decimals));
                        // Task 7.1: Derive USD value from pair prices
                        const usd = computeTokenUsd(ta.symbol, amt);
                        balances[ta.symbol] = { available: amt, usd };
                    }
                }
            }
        } catch { /* RPC unavailable */ }
        if (!Object.keys(balances).length) {
            balances = { LICN: { available: 0, usd: 0 }, lUSD: { available: 0, usd: 0 } };
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
    function connectWebSocket() {
        try {
            if (dexWs) dexWs.close();
            dexWs = new DexWS(WS_URL);
            dexWs.onConnectionChange = (connected) => {
                if (typeof updateFooterStatus === 'function') updateFooterStatus(footerBlockHeight, connected);
            };
        } catch { /* ws unavailable */ }
    }

    function setPredictSlotAnchor(slot) {
        const parsed = Number(slot || 0);
        if (!Number.isFinite(parsed) || parsed <= 0) return;
        state._predictSlotAnchor = { slot: parsed, ts: Date.now() };
    }

    function getRealtimePredictSlot(fallback = 0) {
        const anchor = state._predictSlotAnchor || { slot: 0, ts: 0 };
        if (!anchor.slot || !anchor.ts) return Number(fallback || 0);
        const elapsedMs = Math.max(0, Date.now() - anchor.ts);
        const advancedSlots = Math.floor(elapsedMs / 500);
        return anchor.slot + advancedSlots;
    }

    function stopPredictCountdownTicker() {
        if (state._predictCountdownTimer) {
            clearInterval(state._predictCountdownTimer);
            state._predictCountdownTimer = null;
        }
    }

    function startPredictCountdownTicker() {
        stopPredictCountdownTicker();
        state._predictCountdownTimer = setInterval(() => {
            if (state.currentView !== 'predict') return;
            if (!predictState.markets.length) return;
            renderPredictionMarkets();
            if (state.connected && predictState.positions.length) {
                renderPredictionPositions();
            }
        }, 1000);
    }

    function scheduleMarginRealtimeRefresh() {
        if (state._marginRealtimeRefreshTimer) clearTimeout(state._marginRealtimeRefreshTimer);
        state._marginRealtimeRefreshTimer = setTimeout(() => {
            const shouldRefresh = state.connected && (state.currentView === 'trade' || state.currentView === 'margin');
            if (!shouldRefresh) return;
            loadMarginPositions().catch(() => { });
            if (state.currentView === 'margin') {
                loadMarginHistory().catch(() => { });
            }
        }, 300);
    }

    function schedulePredictRealtimeRefresh() {
        if (state._predictWsRefreshTimer) clearTimeout(state._predictWsRefreshTimer);
        state._predictWsRefreshTimer = setTimeout(() => {
            loadPredictionMarkets().catch(() => { });
            if (state.connected && state.currentView === 'predict') {
                loadPredictionPositions().catch(() => { });
                loadPredictionHistory().catch(() => { });
            }
            if (state.connected) {
                loadCreatedMarkets().catch(() => { });
            }
        }, 120);
    }

    function unsubscribePredictionRealtime() {
        if (!dexWs || !state._predictWsSub) return;
        dexWs.unsubscribe(state._predictWsSub);
        state._predictWsSub = null;
    }

    function subscribePredictionRealtime() {
        if (!dexWs) return;
        unsubscribePredictionRealtime();
        dexWs.subscribePrediction('all', (event) => {
            if (!event || typeof event !== 'object') return;
            setPredictSlotAnchor(event.slot || 0);
            schedulePredictRealtimeRefresh();
        }).then(id => { state._predictWsSub = id; }).catch(() => { });
    }

    // F6.11: RAF-throttle for high-frequency WS order book updates
    function rafThrottle(fn) { let pending = false, lastArgs; return function (...args) { lastArgs = args; if (!pending) { pending = true; requestAnimationFrame(() => { pending = false; fn(...lastArgs); }); } }; }
    const throttledRenderOrderBook = rafThrottle(() => { if (state.currentView === 'trade') renderOrderBook(); });

    function subscribePair(pairId) {
        if (!dexWs) return;
        state._wsSubs.forEach(id => dexWs.unsubscribe(id)); state._wsSubs = [];

        dexWs.subscribe(`orderbook:${pairId}`, (d) => {
            if (d.bids && d.asks) {
                const inv = isDisplayInvertedPair(state.activePair);
                const map = arr => arr.map(a => ({ price: inv ? invertPrice(a.price) : a.price, amount: (a.quantity || 0) / 1e9, total: 0 }));
                // When inverted, on-chain bids become display asks and vice versa
                let asks, bids;
                if (inv) {
                    asks = map(d.bids); asks.sort((a, b) => a.price - b.price); let t = 0; asks.forEach(a => { t += a.amount; a.total = t; });
                    bids = map(d.asks); bids.sort((a, b) => b.price - a.price); t = 0; bids.forEach(b => { t += b.amount; b.total = t; });
                } else {
                    asks = map(d.asks); asks.sort((a, b) => a.price - b.price); let t = 0; asks.forEach(a => { t += a.amount; a.total = t; });
                    bids = map(d.bids); bids.sort((a, b) => b.price - a.price); t = 0; bids.forEach(b => { t += b.amount; b.total = t; });
                }
                state.orderBook = { asks, bids }; throttledRenderOrderBook();
            }
        }).then(id => state._wsSubs.push(id)).catch(() => { });

        dexWs.subscribe(`trades:${pairId}`, (d) => {
            if (d.price) {
                // Invert on-chain price for display-inverted pairs (wBNB/LICN → LICN/wBNB)
                const displayPrice = isDisplayInvertedPair(state.activePair) ? invertPrice(d.price) : d.price;
                if (state.activePair) {
                    state.activePair.hasMarketPrice = true;
                    state.activePair.price = displayPrice; // Sync dropdown with header
                }
                state.lastPrice = displayPrice; updateTickerDisplay();
                throttledRenderPairList();
                // C1-FIX: Feed live trades into TradingView chart so header and chart stay in sync
                streamBarUpdate(displayPrice, (d.quantity || 0) / 1e9);
                const c = document.querySelector('.trades-list');
                if (c && state.currentView === 'trade') {
                    const row = document.createElement('div'); row.className = 'trade-row';
                    row.innerHTML = `<span class="trade-price ${d.side === 'buy' ? 'buy' : 'sell'}">${formatPrice(displayPrice)}</span><span>${formatAmount((d.quantity || 0) / 1e9)}</span><span class="trade-time">${new Date().toLocaleTimeString()}</span>`;
                    c.prepend(row); if (c.children.length > 40) c.lastChild.remove();
                }
                scheduleMarginRealtimeRefresh();
            }
        }).then(id => state._wsSubs.push(id)).catch(() => { });

        if (wallet.address) {
            dexWs.subscribe(`orders:${wallet.address}`, (d) => {
                if (d.orderId) {
                    const o = openOrders.find(x => x.id === String(d.orderId));
                    if (o) { o.filled = d.filled / ((d.filled + d.remaining) || 1); }
                    if (d.status === 'filled' || d.status === 'cancelled') {
                        // Task 5.2: Respect notification preferences
                        const isFill = d.status === 'filled';
                        const isPartial = o && o.filled > 0 && o.filled < 1;
                        if (isFill && state.notifPrefs.fills !== false) {
                            showNotification(`Order ${d.status}: #${d.orderId}`, 'success');
                        } else if (isPartial && state.notifPrefs.partials !== false) {
                            showNotification(`Order partially filled: #${d.orderId}`, 'info');
                        } else if (!isFill && !isPartial) {
                            showNotification(`Order ${d.status}: #${d.orderId}`, 'info');
                        }
                        openOrders = openOrders.filter(x => x.id !== String(d.orderId));
                    }
                    renderOpenOrders();
                }
            }).then(id => state._wsSubs.push(id)).catch(() => { });
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Global ticker subscriptions — all pairs over WS (replaces 10s REST poll)
    // ═══════════════════════════════════════════════════════════════════════
    let _tickerSubs = [];
    const throttledRenderPairList = rafThrottle(() => renderPairList());

    function subscribeAllTickers() {
        if (!dexWs) return;
        // Clean up previous global ticker subs
        _tickerSubs.forEach(id => dexWs.unsubscribe(id));
        _tickerSubs = [];

        for (const p of pairs) {
            dexWs.subscribe(`ticker:${p.pairId}`, (d) => {
                if (!d.lastPrice) return;
                const displayPrice = isDisplayInvertedPair(p) ? invertPrice(d.lastPrice) : d.lastPrice;
                p.price = displayPrice;
                p.hasMarketPrice = true;
                p.change = d.change24h ?? p.change;
                // C2-FIX: Sync header price from ticker WS for active pair
                if (p.pairId === state.activePairId) {
                    state.lastPrice = displayPrice;
                    updateTickerDisplay();
                }
                throttledRenderPairList();
            }).then(id => _tickerSubs.push(id)).catch(() => { });
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Navigation
    // ═══════════════════════════════════════════════════════════════════════
    const navLinks = document.querySelectorAll('.nav-menu a[data-view]');
    const views = document.querySelectorAll('.dex-main');
    function switchView(v) {
        state.currentView = v;
        views.forEach(el => el.classList.toggle('hidden', el.id !== `view-${v}`));
        navLinks.forEach(l => l.classList.toggle('active', l.dataset.view === v));
        if (v === 'trade') {
            drawChart();
            loadTradeHistory();
            loadMarginStats();
            loadMarginPositions();
            loadMarginHistory();
        }
        if (v === 'predict') {
            loadPredictionStats();
            loadPredictionMarkets();
            loadPredictionPositions();
            loadPredictionHistory();
            loadCreatedMarkets();
            refreshPredictWalletBalance();
            subscribePredictionRealtime();
            startPredictCountdownTicker();
        } else {
            unsubscribePredictionRealtime();
            stopPredictCountdownTicker();
        }
        if (v === 'pool') { loadPoolStats(); loadPools(); loadLPPositions(); }
        if (v === 'rewards') { loadRewardsStats(); }
        if (v === 'governance') { loadGovernanceStats(); loadProposals(); }
        if (v === 'launchpad') { loadLaunchpadStats(); loadLaunchpadTokens(); }
        applyWalletGateAll();
    }
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
        if (!pairList) return;
        const f = filter.toLowerCase();
        const filtered = pairs.filter(p => !f || p.id.toLowerCase().includes(f));
        if (!filtered.length) {
            pairList.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-search" style="margin-right:6px;"></i>No trading pairs available</div>';
            return;
        }
        pairList.innerHTML = filtered.map(p => `
            <div class="pair-item ${state.activePair?.id === p.id ? 'active' : ''}" data-pair="${escapeHtml(p.id)}">
                <span>${escapeHtml(p.id)}</span><span class="pair-price">${formatPrice(p.price)}</span>
            </div>`).join('');
        pairList.querySelectorAll('.pair-item').forEach(el => el.addEventListener('click', e => { e.stopPropagation(); const pair = pairs.find(p => p.id === el.dataset.pair); if (pair) selectPair(pair); pairDropdown.classList.remove('open'); }));
    }
    if (pairSearch) pairSearch.addEventListener('input', e => renderPairList(e.target.value));

    async function selectPair(pair) {
        state.activePair = pair; state.activePairId = pair.pairId;
        state.lastPrice = pair.hasMarketPrice
            ? pair.price
            : ((pair.id === 'LICN/lUSD' || pair.base === 'LICN') ? LICHEN_GENESIS_PRICE : 0);
        // Task 5.4: Remember last selected pair
        localStorage.setItem('dexLastPair', String(pair.pairId));
        if (pairActive) pairActive.querySelector('.pair-name').textContent = pair.id;
        updateOrderBookLabels(pair);
        updatePairStats(pair); updateTickerDisplay(); renderPairList();
        await Promise.all([loadOrderBook(), loadRecentTrades()]);
        subscribePair(pair.pairId);
        if (tvWidget?.activeChart) { try { tvWidget.activeChart().setSymbol(pair.id, () => { }); } catch { drawChart(); } } else drawChart();
        // Update margin mode availability for new pair
        if (state.tradeMode === 'margin' && !isMarginEnabledForActivePair()) {
            setTradeMode('spot', { notifyOnBlocked: true });
        } else {
            syncMarginAvailabilityUi();
            if (state.tradeMode === 'margin') updateMarginInfo();
        }
    }

    function updatePairStats(pair) {
        const stats = document.querySelectorAll('.pair-stats .stat-item .stat-value');
        if (stats.length >= 5) loadTicker(pair.pairId).then(t => {
            if (t) {
                const ch = t.change24h ?? 0;
                const chEl = stats[0];
                chEl.textContent = `${ch >= 0 ? '+' : ''}${ch.toFixed(2)}%`;
                chEl.className = `stat-value ${ch >= 0 ? 'positive' : 'negative'}`;
                // Invert high/low for display-inverted pairs (note: inverted high = 1/low, inverted low = 1/high)
                const inv = isDisplayInvertedPair(pair);
                const high = inv ? invertPrice(t.low24h || 0) : (t.high24h || 0);
                const low = inv ? invertPrice(t.high24h || 0) : (t.low24h || 0);
                stats[1].textContent = formatPrice(high);
                stats[2].textContent = formatPrice(low);
                stats[3].textContent = formatVolume((t.volume24h || 0) / 1e9);
                stats[4].textContent = String(t.trades24h || '0');
            } else {
                stats[0].textContent = '--'; stats[0].className = 'stat-value';
                stats[1].textContent = '--'; stats[2].textContent = '--';
                stats[3].textContent = '--'; stats[4].textContent = '0';
            }
        });
    }

    function updateTickerDisplay() {
        const tp = document.querySelector('.ticker-pair'), t = document.querySelector('.ticker-price'), ch = document.querySelector('.ticker-change');
        if (tp && state.activePair) tp.textContent = state.activePair.id || 'LICN/lUSD';
        if (t) t.textContent = formatPrice(state.lastPrice);
        if (ch && state.activePair) { const c = state.activePair.change || 0; ch.textContent = `${c >= 0 ? '+' : ''}${c.toFixed(2)}%`; ch.className = `ticker-change ${c >= 0 ? 'positive' : 'negative'}`; }
    }

    function updateOrderBookLabels(pair) {
        const priceLabel = document.getElementById('bookPriceLabel');
        const sizeLabel = document.getElementById('bookSizeLabel');
        const quote = pair?.quote || pair?.quoteSymbol || 'lUSD';
        const base = pair?.base || pair?.baseSymbol || 'LICN';
        if (priceLabel) priceLabel.textContent = `Price (${quote})`;
        if (sizeLabel) sizeLabel.textContent = `Size (${base})`;
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
        ac.innerHTML = [...state.orderBook.asks].reverse().map(a => `<div class="book-row ask"><span class="price">${formatPrice(a.price)}</span><span>${formatAmount(a.amount)}</span><span>${formatAmount(a.total)}</span><div class="depth-bar" style="width:${(a.total / ma * 100).toFixed(1)}%"></div></div>`).join('');
        if (sp) {
            // Show last traded price in the spread bar (like Binance), not mid-price
            const marketPrice = state.lastPrice || 0;
            sp.textContent = formatPrice(marketPrice);
            // Color the market price: green if >= previous, red if below
            const prevDir = state._prevSpreadPrice || 0;
            sp.className = `spread-price ${marketPrice >= prevDir ? 'positive' : 'negative'}`;
            state._prevSpreadPrice = marketPrice;
            if (sv) {
                const tb = state.orderBook.bids[0]?.price || 0, ba = state.orderBook.asks[0]?.price || 0;
                const s = ba - tb;
                sv.textContent = `Spread: ${formatPrice(Math.abs(s))} (${ba > 0 ? (s / ba * 100).toFixed(3) : '0.000'}%)`;
            }
        }
        bc.innerHTML = state.orderBook.bids.map(b => `<div class="book-row bid"><span class="price">${formatPrice(b.price)}</span><span>${formatAmount(b.amount)}</span><span>${formatAmount(b.total)}</span><div class="depth-bar" style="width:${(b.total / mb * 100).toFixed(1)}%"></div></div>`).join('');

        // ── Order book click-to-fill: clicking a row fills the price input ──
        document.querySelectorAll('.book-row').forEach(row => {
            row.style.cursor = 'pointer';
            row.addEventListener('click', () => {
                const priceSpan = row.querySelector('.price');
                if (priceSpan && priceInput) {
                    priceInput.value = priceSpan.textContent.replace(/,/g, '');
                    priceInput.dispatchEvent(new Event('input'));
                    // Switch to limit order if currently on market (price fill implies limit intent)
                    if (state.orderType === 'market') {
                        const limitBtn = document.querySelector('[data-type="limit"]');
                        if (limitBtn) limitBtn.click();
                    }
                }
            });
        });
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Book Layout Buttons (Both / Bids Only / Asks Only)
    // ═══════════════════════════════════════════════════════════════════════
    document.querySelectorAll('.book-layout').forEach(btn => {
        btn.addEventListener('click', () => {
            document.querySelectorAll('.book-layout').forEach(b => b.classList.remove('active'));
            btn.classList.add('active');
            const layout = btn.dataset.layout;
            const asksEl = document.getElementById('bookAsks');
            const bidsEl = document.querySelector('.book-bids');
            if (!asksEl || !bidsEl) return;
            switch (layout) {
                case 'both':
                    asksEl.style.display = ''; bidsEl.style.display = '';
                    break;
                case 'bids':
                    asksEl.style.display = 'none'; bidsEl.style.display = '';
                    break;
                case 'asks':
                    asksEl.style.display = ''; bidsEl.style.display = 'none';
                    break;
            }
        });
    });

    // ═══════════════════════════════════════════════════════════════════════
    // Footer: Block Height + Connection Status
    // ═══════════════════════════════════════════════════════════════════════
    const footerBlockEl = document.getElementById('footerBlock');
    const statusDotEl = document.querySelector('.status-dot');
    let footerBlockHeight = 0;

    function updateFooterStatus(height, connected = true) {
        if (height > footerBlockHeight) footerBlockHeight = height;
        if (footerBlockEl) {
            footerBlockEl.textContent = connected
                ? `Block #${footerBlockHeight.toLocaleString()}`
                : 'Reconnecting...';
        }
        if (statusDotEl) {
            statusDotEl.classList.toggle('green', connected);
            statusDotEl.classList.toggle('red', !connected);
        }
    }

    // Poll block height for footer display
    async function pollBlockHeight() {
        try {
            const slot = await api.rpc('getSlot', []);
            if (typeof slot === 'number') {
                updateFooterStatus(slot, true);
                setPredictSlotAnchor(slot);
            }
        } catch {
            updateFooterStatus(footerBlockHeight, false);
        }
    }
    // DEX-05: Prefer WS subscription for real-time block height if available,
    // fall back to polling at reduced interval. WS subscription is set up in
    // the WebSocket connection handler (subscribeSlot).
    pollBlockHeight();
    setInterval(pollBlockHeight, 3000);

    // Footer links (data-lichen-app) are auto-resolved by shared-config.js

    // ═══════════════════════════════════════════════════════════════════════
    // Network Selector — wired to LICHEN_CONFIG (shared-config.js)
    // ═══════════════════════════════════════════════════════════════════════
    const networkSelect = document.getElementById('networkSelect');
    if (networkSelect) {
        LICHEN_CONFIG.initNetworkSelector(networkSelect, 'dexNetwork', (network, cfg) => {
            localStorage.setItem('dexRpcUrl', cfg.rpc);
            localStorage.setItem('dexWsUrl', cfg.ws);
            window.location.reload();
        });

        // Restore saved network selection
        const savedNetwork = localStorage.getItem('dexNetwork');
        if (savedNetwork && networkSelect.querySelector(`option[value="${savedNetwork}"]`)) {
            networkSelect.value = savedNetwork;
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // TradingView (wired to candle API)
    // ═══════════════════════════════════════════════════════════════════════
    let tvWidget = null, realtimeCallback = null, lastBarTime = 0, activeResolution = localStorage.getItem('dexChartInterval') || '15', currentBarOpen = 0, currentBarHigh = 0, currentBarLow = Infinity, currentSubscriberUID = null;


    function createDatafeed() {
        return {
            onReady: cb => setTimeout(() => cb({ supported_resolutions: ['1', '5', '15', '60', '240', '1D', '3D', '1W'], exchanges: [{ value: 'Lichen', name: 'Lichen', desc: 'Lichen DEX' }], symbols_types: [{ name: 'crypto', value: 'crypto' }] }), 0),
            searchSymbols: (input, ex, st, cb) => cb(pairs.filter(p => p.id.toLowerCase().includes(input.toLowerCase())).map(p => ({ symbol: p.id, full_name: 'Lichen:' + p.id, description: p.id, exchange: 'Lichen', type: 'crypto' }))),
            resolveSymbol: (name, ok, err) => {
                const p = pairs.find(x => x.id === name || ('Lichen:' + x.id) === name) || pairs[0];
                if (!p) { err('Not found'); return; }
                // Display-inverted pairs have very small prices (~0.000171) — always use high pricescale
                const ps = isDisplayInvertedPair(p) ? 100000000 : (p.price < 0.001 ? 100000000 : p.price < 1 ? 10000 : 100);
                setTimeout(() => ok({ name: p.id, ticker: p.id, description: p.id, type: 'crypto', session: '24x7', timezone: 'Etc/UTC', exchange: 'Lichen', listed_exchange: 'Lichen', minmov: 1, pricescale: ps, has_intraday: true, has_weekly_and_monthly: true, supported_resolutions: ['1', '5', '15', '60', '240', '1D', '3D', '1W'], volume_precision: 2, data_status: 'streaming' }), 0);
            },
            getBars: async (si, res, pp, ok) => {
                const apiC = await loadCandles(pp.from, pp.to, res);
                let bars = apiC?.length ? apiC : [];
                if (bars.length) {
                    state.candles = bars;
                    lastBarTime = bars[bars.length - 1].time;
                    currentBarOpen = bars[bars.length - 1].open;
                    currentBarHigh = bars[bars.length - 1].high;
                    currentBarLow = bars[bars.length - 1].low;
                }
                ok(bars, { noData: !bars.length });
                // Only seed a synthetic live bar when there is no market history yet.
                if (!bars.length && state.lastPrice > 0) setTimeout(() => streamBarUpdate(state.lastPrice, 0), 100);
            },
            subscribeBars: (si, res, cb, uid) => {
                realtimeCallback = cb; activeResolution = res;
                currentSubscriberUID = uid;
                localStorage.setItem('dexChartInterval', res);
                // Subscribe to candle WS channel for real-time OHLCV streaming
                subscribeCandleWs(state.activePairId, res);
            },
            unsubscribeBars: (uid) => { if (uid === currentSubscriberUID) { realtimeCallback = null; currentSubscriberUID = null; unsubscribeCandleWs(); } },
        };
    }

    function streamBarUpdate(price, vol) {
        if (!realtimeCallback || !price || price <= 0) return;
        const ms = resolutionToMs(activeResolution);
        const bt = Math.floor(Date.now() / ms) * ms;
        if (bt > lastBarTime) {
            // New candle period
            lastBarTime = bt;
            currentBarOpen = price;
            currentBarHigh = price;
            currentBarLow = price;
            realtimeCallback({ time: bt, open: price, high: price, low: price, close: price, volume: vol || 0 });
        } else {
            // Update existing candle — track real high/low across all ticks
            currentBarHigh = Math.max(currentBarHigh, price);
            currentBarLow = Math.min(currentBarLow, price);
            realtimeCallback({ time: lastBarTime, open: currentBarOpen, high: currentBarHigh, low: currentBarLow, close: price, volume: vol || 0 });
        }
    }

    function resolutionToMs(r) { return { '1': 60000, '5': 300000, '15': 900000, '60': 3600000, '240': 14400000, '1D': 86400000, '3D': 259200000, '1W': 604800000 }[r] || 900000; }
    function resolutionToSec(r) { return { '1': 60, '5': 300, '15': 900, '60': 3600, '240': 14400, '1D': 86400, '3D': 259200, '1W': 604800 }[r] || 900; }

    // ═══════════════════════════════════════════════════════════════════════
    // Candle WS subscription — real-time OHLCV streaming from validator
    // ═══════════════════════════════════════════════════════════════════════
    let _candleWsSub = null;
    function subscribeCandleWs(pairId, resolution) {
        unsubscribeCandleWs();
        if (!dexWs || !pairId) return;
        const interval = resolutionToSec(resolution);
        dexWs.subscribe(`candles:${pairId}:${interval}`, (d) => {
            if (!realtimeCallback || !d.close || d.close <= 0) return;
            const inv = isDisplayInvertedPair(state.activePair);
            const o = inv ? invertPrice(d.open) : d.open;
            const h_raw = inv ? invertPrice(d.low) : d.high;
            const l_raw = inv ? invertPrice(d.high) : d.low;
            const c = inv ? invertPrice(d.close) : d.close;
            const h = Math.max(h_raw, l_raw);
            const l = Math.min(h_raw, l_raw);
            const ms = resolutionToMs(resolution);
            const bt = d.slot ? Math.floor(Date.now() / ms) * ms : lastBarTime;
            if (bt > lastBarTime) {
                lastBarTime = bt; currentBarOpen = o; currentBarHigh = h; currentBarLow = l;
            } else {
                currentBarHigh = Math.max(currentBarHigh, h);
                currentBarLow = Math.min(currentBarLow, l);
            }
            // Also sync header price from candle close so header matches chart
            state.lastPrice = c; updateTickerDisplay();
            realtimeCallback({ time: lastBarTime || bt, open: currentBarOpen || o, high: currentBarHigh, low: currentBarLow, close: c, volume: d.volume || 0 });
        }).then(id => { _candleWsSub = id; }).catch(() => { });
    }
    function unsubscribeCandleWs() {
        if (_candleWsSub && dexWs) { dexWs.unsubscribe(_candleWsSub); _candleWsSub = null; }
    }

    let tvRetryCount = 0;
    function initTradingView() {
        const el = document.getElementById('tvChartContainer');
        if (!el || typeof TradingView === 'undefined') { if (el) el.innerHTML = '<div style="display:flex;align-items:center;justify-content:center;height:100%;color:var(--text-muted);font-size:0.9rem;"><i class="fas fa-chart-line" style="margin-right:8px;"></i> Chart unavailable — library failed to load</div>'; if (++tvRetryCount < 5) setTimeout(initTradingView, 5000); return; }
        tvWidget = new TradingView.widget({
            symbol: state.activePair?.id || 'LICN/lUSD', container: el, datafeed: createDatafeed(), library_path: 'charting_library/', locale: 'en', fullscreen: false, autosize: true, theme: 'Dark', interval: localStorage.getItem('dexChartInterval') || '15', toolbar_bg: '#0d1117',
            loading_screen: { backgroundColor: '#0A0E27', foregroundColor: '#00C9DB' },
            overrides: { 'paneProperties.background': '#0d1117', 'paneProperties.backgroundType': 'solid', 'paneProperties.vertGridProperties.color': 'rgba(255,255,255,0.04)', 'paneProperties.horzGridProperties.color': 'rgba(255,255,255,0.04)', 'scalesProperties.textColor': 'rgba(255,255,255,0.5)', 'scalesProperties.lineColor': 'rgba(255,255,255,0.08)', 'mainSeriesProperties.candleStyle.upColor': '#06d6a0', 'mainSeriesProperties.candleStyle.downColor': '#ef4444', 'mainSeriesProperties.candleStyle.borderUpColor': '#06d6a0', 'mainSeriesProperties.candleStyle.borderDownColor': '#ef4444', 'mainSeriesProperties.candleStyle.wickUpColor': '#06d6a0', 'mainSeriesProperties.candleStyle.wickDownColor': '#ef4444' },
            disabled_features: ['header_compare', 'header_undo_redo', 'go_to_date', 'use_localstorage_for_settings', 'study_templates'],
            enabled_features: ['side_toolbar_in_fullscreen_mode', 'header_symbol_search'],
        });
        tvWidget.onChartReady(() => { tvWidget.activeChart().onSymbolChanged().subscribe(null, () => { const s = tvWidget.activeChart().symbol(); const p = pairs.find(x => x.id === s || ('Lichen:' + x.id) === s); if (p && p.id !== state.activePair?.id) selectPair(p); }); });
    }

    function drawChart() { if (realtimeCallback && state.candles.length) { const l = state.candles[state.candles.length - 1]; const ms = resolutionToMs(activeResolution); realtimeCallback({ time: Math.floor(l.time / ms) * ms, open: l.open, high: l.high, low: l.low, close: l.close, volume: l.volume }); } }

    // ═══════════════════════════════════════════════════════════════════════
    // Order Form
    // ═══════════════════════════════════════════════════════════════════════
    const orderTabs = document.querySelectorAll('.order-tab'), orderTypeBtns = document.querySelectorAll('.order-type-btn');
    const priceInput = document.getElementById('orderPrice'), amountInput = document.getElementById('orderAmount'), totalInput = document.getElementById('orderTotal');
    const submitBtn = document.getElementById('submitOrder'), stopGroup = document.querySelector('.stop-price-group');

    function updateMarginSltpVisibility() {
        const sltpRow = document.querySelector('.margin-sltp-row');
        if (!sltpRow) return;
        const hidden = state.tradeMode === 'margin' && state.orderType === 'stop-limit';
        sltpRow.style.display = hidden ? 'none' : 'flex';
    }

    orderTabs.forEach(tab => tab.addEventListener('click', () => {
        orderTabs.forEach(t => t.classList.remove('active'));
        tab.classList.add('active');
        state.orderSide = tab.dataset.side;
        state.marginSide = state.orderSide === 'buy' ? 'long' : 'short';
        updateSubmitBtn();
        if (state.tradeMode === 'margin') updateMarginInfo();
    }));
    orderTypeBtns.forEach(btn => btn.addEventListener('click', () => {
        orderTypeBtns.forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        state.orderType = btn.dataset.type;
        if (stopGroup) stopGroup.style.display = state.orderType === 'stop-limit' ? 'block' : 'none';
        if (priceInput) priceInput.parentElement.parentElement.style.display = state.orderType === 'market' ? 'none' : 'block';
        updateMarginSltpVisibility();
        if (state.tradeMode === 'margin') updateMarginInfo();
        updateSubmitBtn();
    }));

    function updateOrderSideLabels() {
        const sideBuy = document.querySelector('.order-tab[data-side="buy"]');
        const sideSell = document.querySelector('.order-tab[data-side="sell"]');
        if (!sideBuy || !sideSell) return;
        if (state.tradeMode === 'margin') {
            sideBuy.textContent = 'Long';
            sideSell.textContent = 'Short';
        } else {
            sideBuy.textContent = 'Buy';
            sideSell.textContent = 'Sell';
        }
    }

    function getQuoteAvailableMargin() {
        const quoteSymbol = state.activePair?.quote;
        if (!quoteSymbol) return 0;
        return Math.max(0, balances[quoteSymbol]?.available || 0);
    }

    function updateSubmitBtn() {
        if (!submitBtn) return;
        const sideLabel = state.tradeMode === 'margin'
            ? (state.orderSide === 'buy' ? 'Long' : 'Short')
            : (state.orderSide === 'buy' ? 'Buy' : 'Sell');
        const m = state.tradeMode === 'margin' ? ` ${state.leverageValue}x` : '';
        submitBtn.className = `btn-full ${state.orderSide === 'buy' ? 'btn-buy' : 'btn-sell'}`;
        submitBtn.textContent = `${sideLabel}${m} ${state.activePair?.base || ''}`;

        if (!state.connected || !state.activePair) {
            submitBtn.disabled = true;
            submitBtn.title = state.connected ? 'No active pair selected' : 'Connect wallet to trade';
            return;
        }

        const amount = Math.max(0, parseFloat(amountInput?.value || '0') || 0);
        const price = state.orderType === 'market'
            ? (state.lastPrice || 0)
            : Math.max(0, parseFloat(priceInput?.value || '0') || 0);
        const stopPriceInput = document.getElementById('stopPrice');
        const stopPrice = Math.max(0, parseFloat(stopPriceInput?.value || '0') || 0);
        const feeRate = 0.0005;

        let disabledReason = '';
        if (amount <= 0) disabledReason = 'Enter an amount';
        else if (state.orderType !== 'market' && price <= 0) disabledReason = 'Enter a valid price';
        else if (state.orderType === 'market' && price <= 0) disabledReason = 'Waiting for market price';
        else if (state.orderType === 'stop-limit' && stopPrice <= 0) disabledReason = 'Enter stop price';
        else {
            const notional = price * amount;
            if (state.tradeMode === 'margin') {
                const quoteSymbol = state.activePair.quote;
                const availableQuote = getQuoteAvailableMargin();
                if (state.marginType === 'cross') {
                    if (availableQuote <= 0) disabledReason = `Insufficient ${quoteSymbol}`;
                    else {
                        const maxOpenNotional = (availableQuote * Math.max(1, Number(state.leverageValue || 1))) / (1 + feeRate);
                        if (notional > maxOpenNotional) disabledReason = `Insufficient ${quoteSymbol}`;
                    }
                } else {
                    const leverage = Math.max(1, Number(state.leverageValue || 1));
                    const requiredMargin = (notional / leverage) * (1 + feeRate);
                    if (requiredMargin > availableQuote) disabledReason = `Insufficient ${quoteSymbol}`;
                }
            } else if (state.orderSide === 'buy') {
                const quoteSymbol = state.activePair.quote;
                const requiredQuote = notional * (1 + feeRate);
                const availableQuote = balances[quoteSymbol]?.available || 0;
                if (requiredQuote > availableQuote) disabledReason = `Insufficient ${quoteSymbol}`;
            } else {
                const baseSymbol = state.activePair.base;
                const requiredBase = amount * (1 + feeRate);
                const availableBase = balances[baseSymbol]?.available || 0;
                if (requiredBase > availableBase) disabledReason = `Insufficient ${baseSymbol}`;
            }
        }

        submitBtn.disabled = Boolean(disabledReason);
        submitBtn.title = disabledReason;
    }

    function getAllowedLeverageLevels(maxLeverage) {
        const contractLevels = [2, 3, 5, 10, 25, 50, 100];
        return contractLevels.filter(level => level <= Math.max(2, maxLeverage));
    }

    function snapLeverageToAllowed(rawValue, maxLeverage) {
        const levels = getAllowedLeverageLevels(maxLeverage);
        if (!levels.length) return 2;
        const value = Number(rawValue || levels[0]);
        return levels.reduce((closest, level) => (
            Math.abs(level - value) < Math.abs(closest - value) ? level : closest
        ), levels[0]);
    }

    function applyLeverageConstraints() {
        if (!inlineLeverage) return;
        const crossCap = 3;
        const modeMax = state.marginType === 'cross'
            ? Math.min(state.marginMaxLeverage || 100, crossCap)
            : (state.marginMaxLeverage || 100);
        const effectiveMax = Math.max(2, modeMax);
        inlineLeverage.max = String(effectiveMax);
        inlineLeverage.min = '2';
        inlineLeverage.step = '1';
        state.leverageValue = snapLeverageToAllowed(state.leverageValue, effectiveMax);
        inlineLeverage.value = String(state.leverageValue);
        if (inlineLeverageTag) inlineLeverageTag.textContent = `${state.leverageValue}x`;
    }

    function isMarginEnabledForActivePair() {
        return marginEnabledPairIds.includes(state.activePairId);
    }

    function setTradeMode(mode, { notifyOnBlocked = false } = {}) {
        if (mode === 'margin' && !isMarginEnabledForActivePair()) {
            syncMarginAvailabilityUi();
            if (notifyOnBlocked) showNotification('Margin is unavailable for this pair', 'warning');
            return false;
        }
        state.tradeMode = mode;
        updateOrderSideLabels();
        document.querySelectorAll('.trade-mode').forEach(b => b.classList.toggle('active', b.dataset.mode === mode));
        const mi = document.getElementById('marginInline');
        if (mi) mi.classList.toggle('hidden', state.tradeMode !== 'margin');
        updateSubmitBtn();
        updateMarginSltpVisibility();
        if (state.tradeMode === 'margin') {
            applyLeverageConstraints();
            loadMarginStats();
            loadMarginPositions();
            updateMarginInfo();
        }
        syncMarginAvailabilityUi();
        return true;
    }

    document.querySelectorAll('.trade-mode').forEach(btn => {
        btn.addEventListener('click', () => {
            setTradeMode(btn.dataset.mode, { notifyOnBlocked: true });
        });
    });
    const inlineLeverage = document.getElementById('inlineLeverage'), inlineLeverageTag = document.getElementById('inlineLeverageTag');
    if (inlineLeverage) inlineLeverage.addEventListener('input', () => { const maxLev = state.marginType === 'cross' ? Math.min(state.marginMaxLeverage || 100, 3) : (state.marginMaxLeverage || 100); state.leverageValue = snapLeverageToAllowed(parseFloat(inlineLeverage.value), maxLev); inlineLeverage.value = String(state.leverageValue); if (inlineLeverageTag) inlineLeverageTag.textContent = `${state.leverageValue}x`; updateSubmitBtn(); updateMarginInfo(); });
    document.querySelectorAll('.margin-inline-type').forEach(btn => btn.addEventListener('click', () => { document.querySelectorAll('.margin-inline-type').forEach(b => b.classList.remove('active')); btn.classList.add('active'); state.marginType = btn.dataset.mtype; applyLeverageConstraints(); updateMarginInfo(); updateSubmitBtn(); }));

    // F9.5a/F9.5b/F9.12a: Route info and fee estimate from actual router quote API
    let _routeQuoteTimer = null;
    const ROUTE_TYPE_LABELS = { clob: 'CLOB Direct', amm: 'AMM Pool', split: 'CLOB + AMM Split', multi_hop: 'Multi-Hop', legacy_swap: 'Legacy Swap' };
    function calcTotal() {
        if (!priceInput || !amountInput || !totalInput) return;
        const rawP = parseFloat(priceInput.value);
        const rawA = parseFloat(amountInput.value);
        const p = Math.max(0, Number.isFinite(rawP) ? rawP : 0);
        const a = Math.max(0, Number.isFinite(rawA) ? rawA : 0);
        if (Number.isFinite(rawP) && rawP < 0) priceInput.value = String(p);
        if (Number.isFinite(rawA) && rawA < 0) amountInput.value = String(a);
        totalInput.value = (p * a).toFixed(4);
        if (state.tradeMode === 'margin') updateMarginInfo();
        updateSubmitBtn();
        const fe = document.getElementById('feeEstimate'), re = document.getElementById('routeInfo');
        // Show inline estimate immediately, then refine via API
        if (fe) fe.textContent = `~${(p * a * 0.0005).toFixed(4)} ${state.activePair?.quote || ''}`;
        if (re) re.textContent = 'Routing...';
        // Debounce router quote call
        clearTimeout(_routeQuoteTimer);
        if (p > 0 && a > 0 && state.activePair) {
            _routeQuoteTimer = setTimeout(async () => {
                try {
                    const tokenIn = state.orderSide === 'buy' ? state.activePair.quote : state.activePair.base;
                    const tokenOut = state.orderSide === 'buy' ? state.activePair.base : state.activePair.quote;
                    const amountIn = Math.round(p * a * 1e9);
                    const { data } = await api.post('/router/quote', { token_in: tokenIn, token_out: tokenOut, amount_in: amountIn, slippage: state.slippagePct });
                    if (data && data.routeType) {
                        if (re) re.textContent = ROUTE_TYPE_LABELS[data.routeType] || data.routeType;
                        if (fe && data.feeRate !== undefined) {
                            const feeRate = data.feeRate / 10000;
                            fe.textContent = `~${(p * a * feeRate).toFixed(4)} ${state.activePair?.quote || ''} (${data.feeRate}bps)`;
                        }
                    }
                } catch {
                    // Fallback to heuristic if API unavailable
                    if (re) re.textContent = p * a > 50000 ? 'CLOB + AMM Split' : 'CLOB Direct';
                }
            }, 300);
        } else {
            if (re) re.textContent = '—';
        }
    }
    if (priceInput) priceInput.addEventListener('input', calcTotal);
    if (amountInput) amountInput.addEventListener('input', calcTotal);
    if (totalInput) totalInput.addEventListener('input', () => {
        if (!priceInput || !amountInput) return;
        const p = Math.max(0, parseFloat(priceInput.value) || 0);
        const rawT = parseFloat(totalInput.value);
        const t = Math.max(0, Number.isFinite(rawT) ? rawT : 0);
        if (Number.isFinite(rawT) && rawT < 0) totalInput.value = String(t);
        if (p > 0) amountInput.value = (t / p).toFixed(4);
        if (state.tradeMode === 'margin') updateMarginInfo();
        updateSubmitBtn();
    });

    document.querySelectorAll('.preset-btn').forEach(btn => btn.addEventListener('click', () => {
        const pct = parseInt(btn.dataset.pct, 10) / 100, tok = state.orderSide === 'buy' ? state.activePair?.quote : state.activePair?.base, bal = tok ? balances[tok] : null;
        if (!bal || !amountInput || !priceInput) return;
        if (state.orderSide === 'buy') { amountInput.value = ((bal.available * pct) / (parseFloat(priceInput.value) || state.lastPrice)).toFixed(4); } else amountInput.value = (bal.available * pct).toFixed(4);
        calcTotal();
        updateSubmitBtn();
    }));

    // ═══════════════════════════════════════════════════════════════════════
    // Task 5.1/5.2: Settings Popover (slippage + notification prefs)
    // ═══════════════════════════════════════════════════════════════════════
    {
        const gearBtn = document.getElementById('settingsGearBtn');
        const popover = document.getElementById('settingsPopover');
        const closeBtn = document.getElementById('settingsCloseBtn');
        if (gearBtn && popover) {
            gearBtn.addEventListener('click', () => popover.classList.toggle('hidden'));
            if (closeBtn) closeBtn.addEventListener('click', () => popover.classList.add('hidden'));
            // Close on outside click
            document.addEventListener('click', (e) => {
                if (!popover.classList.contains('hidden') && !popover.contains(e.target) && e.target !== gearBtn && !gearBtn.contains(e.target)) {
                    popover.classList.add('hidden');
                }
            });
        }

        // Slippage preset buttons
        const slippageBtns = document.querySelectorAll('.slippage-btn');
        const slippageCustom = document.getElementById('slippageCustom');
        function setSlippage(val) {
            state.slippagePct = val;
            localStorage.setItem('dexSlippage', String(val));
            slippageBtns.forEach(b => b.classList.toggle('active', parseFloat(b.dataset.slip) === val));
            if (slippageCustom && ![0.1, 0.5, 1.0].includes(val)) {
                slippageCustom.value = val;
            } else if (slippageCustom) {
                slippageCustom.value = '';
            }
        }
        slippageBtns.forEach(btn => btn.addEventListener('click', () => setSlippage(parseFloat(btn.dataset.slip))));
        if (slippageCustom) {
            slippageCustom.addEventListener('change', () => {
                const v = parseFloat(slippageCustom.value);
                if (v > 0 && v <= 50) {
                    setSlippage(v);
                    slippageBtns.forEach(b => b.classList.remove('active'));
                }
            });
        }
        // Restore saved slippage on load
        const savedSlip = parseFloat(localStorage.getItem('dexSlippage'));
        if (savedSlip > 0) {
            setSlippage(savedSlip);
        }

        // Notification preference toggles
        const notifFills = document.getElementById('notifFills');
        const notifPartials = document.getElementById('notifPartials');
        const notifLiquidation = document.getElementById('notifLiquidation');
        function saveNotifPrefs() {
            state.notifPrefs = {
                fills: notifFills?.checked ?? true,
                partials: notifPartials?.checked ?? true,
                liquidation: notifLiquidation?.checked ?? true,
            };
            localStorage.setItem('dexNotifPrefs', JSON.stringify(state.notifPrefs));
        }
        // Restore saved prefs
        if (notifFills) notifFills.checked = state.notifPrefs.fills !== false;
        if (notifPartials) notifPartials.checked = state.notifPrefs.partials !== false;
        if (notifLiquidation) notifLiquidation.checked = state.notifPrefs.liquidation !== false;
        [notifFills, notifPartials, notifLiquidation].forEach(el => {
            if (el) el.addEventListener('change', saveNotifPrefs);
        });
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Full Preflight Order Validation
    // ═══════════════════════════════════════════════════════════════════════
    // Checks ALL contract-enforceable rules client-side before submission.
    // Returns { ok: true } or { ok: false, error: string, code: string }.
    async function preflightOrder({ side, orderType, price, amount, stopPrice, pair, tradeMode, leverage }) {
        // 1. Wallet & connectivity
        if (!state.connected) return { ok: false, error: 'Connect wallet first', code: 'NO_WALLET' };
        if (!walletCanSign()) return { ok: false, error: 'Reconnect wallet to sign transactions', code: 'NO_KEYPAIR' };

        // 2. Basic input validation
        if (!amount || amount <= 0) return { ok: false, error: 'Amount must be positive', code: 'BAD_AMOUNT' };
        if (orderType !== 'market' && (!price || price <= 0)) return { ok: false, error: 'Price must be positive', code: 'BAD_PRICE' };
        if (amount > 9_000_000) return { ok: false, error: 'Amount too large (max 9M)', code: 'OVERFLOW_AMOUNT' };
        if (price > 9_000_000) return { ok: false, error: 'Price too large (max 9M)', code: 'OVERFLOW_PRICE' };

        // 3. Contract availability
        if (!contracts.dex_core) return { ok: false, error: 'Contract addresses not loaded', code: 'NO_CONTRACT' };
        if (tradeMode === 'margin' && !contracts.dex_margin) return { ok: false, error: 'Margin contract not loaded', code: 'NO_MARGIN' };

        // 4. Stop-limit validation
        if (orderType === 'stop-limit') {
            if (!stopPrice || stopPrice <= 0) return { ok: false, error: 'Stop price required for stop-limit orders', code: 'BAD_STOP' };
            if (stopPrice > 9_000_000) return { ok: false, error: 'Stop price too large (max 9M)', code: 'OVERFLOW_STOP' };
            const ref = state.lastPrice || 0;
            if (ref > 0) {
                if (side === 'sell' && stopPrice >= ref)
                    return { ok: false, error: 'Sell-stop price must be below current market price', code: 'STOP_DIRECTION' };
                if (side === 'buy' && stopPrice <= ref)
                    return { ok: false, error: 'Buy-stop price must be above current market price', code: 'STOP_DIRECTION' };
            }
        }

        // 5. Tick size alignment — contract enforces, warn before submission
        if (pair && orderType !== 'market') {
            const tickRaw = Number(pair.tickSize);
            const tickSize = Number.isFinite(tickRaw) && tickRaw > 0
                ? (tickRaw >= 1 ? tickRaw / PRICE_SCALE : tickRaw)
                : 0.0001;
            const tickDecimals = (() => {
                const s = String(tickSize);
                if (s.includes('e-')) return Math.min(9, parseInt(s.split('e-')[1], 10) || 6);
                return Math.min(9, (s.split('.')[1] || '').length || 4);
            })();
            const scaled = price / tickSize;
            const nearest = Math.round(scaled) * tickSize;
            if (Math.abs(scaled - Math.round(scaled)) > 1e-8) {
                return { ok: false, error: `Price must be aligned to tick size ${tickSize.toFixed(tickDecimals)} (nearest: ${nearest.toFixed(tickDecimals)})`, code: 'TICK_ALIGN' };
            }
        }

        // 6. Lot size alignment — contract enforces, warn before submission
        if (pair) {
            const lotRaw = Number(pair.lotSize);
            const lotSize = Number.isFinite(lotRaw) && lotRaw > 0
                ? (lotRaw >= 1 ? lotRaw / PRICE_SCALE : lotRaw)
                : 0.01;
            const lotDecimals = (() => {
                const s = String(lotSize);
                if (s.includes('e-')) return Math.min(9, parseInt(s.split('e-')[1], 10) || 6);
                return Math.min(9, (s.split('.')[1] || '').length || 4);
            })();
            const scaled = amount / lotSize;
            const nearest = Math.round(scaled) * lotSize;
            if (Math.abs(scaled - Math.round(scaled)) > 1e-8) {
                return { ok: false, error: `Amount must be aligned to lot size ${lotSize.toFixed(lotDecimals)} (nearest: ${nearest.toFixed(lotDecimals)})`, code: 'LOT_ALIGN' };
            }
        }

        // 7. Minimum notional check (MIN_ORDER_VALUE = 1000 spores = 0.000001 in human)
        {
            const notional = orderType === 'market' ? amount * (state.lastPrice || 1) : price * amount;
            const minNotionalHuman = 1000 / PRICE_SCALE; // MIN_ORDER_VALUE in spores
            if (notional < minNotionalHuman && notional > 0) {
                return { ok: false, error: `Order notional ${formatAmount(notional)} below minimum (${formatAmount(minNotionalHuman)})`, code: 'MIN_NOTIONAL' };
            }
        }

        // 8. Oracle band check — reject if limit price is outside ±10% of reference
        if (orderType === 'limit' || orderType === 'post-only') {
            const ref = state.lastPrice || 0;
            if (ref > 0) {
                const bandPct = orderType === 'limit' ? 0.10 : 0.05; // limits ±10%, post-only ±5%
                const lowerBand = ref * (1 - bandPct);
                const upperBand = ref * (1 + bandPct);
                if (price < lowerBand || price > upperBand) {
                    return { ok: false, error: `Price ${formatPrice(price)} is outside the oracle band (${formatPrice(lowerBand)} – ${formatPrice(upperBand)}). Adjust price or use market order.`, code: 'ORACLE_BAND' };
                }
            }
        }

        // 9. Post-only crossing check — reject if order would immediately match
        if (orderType === 'post-only') {
            const book = state.orderBook;
            if (side === 'buy' && book.asks?.length > 0) {
                const bestAsk = book.asks[0]?.price || 0;
                if (bestAsk > 0 && price >= bestAsk) {
                    return { ok: false, error: `Post-only buy at ${formatPrice(price)} would cross best ask ${formatPrice(bestAsk)} — use limit order instead`, code: 'POST_ONLY_CROSS' };
                }
            }
            if (side === 'sell' && book.bids?.length > 0) {
                const bestBid = book.bids[0]?.price || 0;
                if (bestBid > 0 && price <= bestBid) {
                    return { ok: false, error: `Post-only sell at ${formatPrice(price)} would cross best bid ${formatPrice(bestBid)} — use limit order instead`, code: 'POST_ONLY_CROSS' };
                }
            }
        }

        // 10. Open order limit check
        if (openOrders.length >= MAX_OPEN_ORDERS_PER_USER) {
            return { ok: false, error: `Maximum open orders reached (${MAX_OPEN_ORDERS_PER_USER}). Cancel an order first.`, code: 'ORDER_LIMIT' };
        }

        // 11. Live balance check — refresh from on-chain before validating
        if (wallet.address) {
            try {
                await loadBalances(wallet.address);
            } catch { }
        }
        {
            const neededToken = side === 'buy' ? (pair?.quote || 'lUSD') : (pair?.base || 'LICN');
            const effectivePrice = orderType === 'market' ? (state.lastPrice || 0) : price;
            const neededAmount = side === 'buy' ? (effectivePrice * amount) : amount;
            const available = balances[neededToken]?.available || 0;
            if (neededAmount > available) {
                return { ok: false, error: `Insufficient ${neededToken}: need ${formatAmount(neededAmount)}, have ${formatAmount(available)}`, code: 'BALANCE' };
            }
        }

        // 12. Reduce-only validation (margin mode)
        if (tradeMode === 'margin') {
            const reduceOnlyEl = document.getElementById('reduceOnly');
            if (reduceOnlyEl && reduceOnlyEl.checked) {
                try {
                    const { data } = await api.get(`/margin/positions?trader=${wallet.address}`);
                    if (Array.isArray(data) && data.length > 0) {
                        const activePairPositions = data.filter(p =>
                            (p.pairId === state.activePairId || p.pair === pair?.id) &&
                            p.status !== 'closed' && p.status !== 'liquidated'
                        );
                        const targetSide = side === 'sell' ? 'long' : 'short';
                        const matchingPos = activePairPositions.filter(p => p.side === targetSide);
                        const totalSize = matchingPos.reduce((sum, p) => sum + ((p.size || 0) / PRICE_SCALE), 0);
                        if (!matchingPos.length) return { ok: false, error: `Reduce-only: No ${targetSide} position to reduce on this pair`, code: 'REDUCE_NO_POS' };
                        if (amount > totalSize) return { ok: false, error: `Reduce-only: Amount ${formatAmount(amount)} exceeds position size ${formatAmount(totalSize)}`, code: 'REDUCE_SIZE' };
                    } else {
                        return { ok: false, error: 'Reduce-only: No open positions to reduce', code: 'REDUCE_NO_POS' };
                    }
                } catch {
                    return { ok: false, error: 'Reduce-only: Could not verify positions', code: 'REDUCE_FAIL' };
                }
            }

            // 13. Margin pair eligibility
            if (!marginEnabledPairIds.includes(state.activePairId)) {
                return { ok: false, error: 'This pair is not enabled for margin trading', code: 'MARGIN_PAIR' };
            }
        }

        return { ok: true };
    }

    // === Order submission via signed sendTransaction ===
    if (submitBtn) submitBtn.addEventListener('click', async () => {
        const price = parseFloat(priceInput?.value) || 0, amount = parseFloat(amountInput?.value) || 0;
        const stopPriceInput = document.getElementById('stopPrice');
        const stopPrice = parseFloat(stopPriceInput?.value) || 0;

        // Post-Only checkbox — override order type to post-only (ORDER_POST_ONLY=3)
        const postOnlyEl = document.getElementById('postOnly');
        let effectiveOrderType = state.orderType;
        if (postOnlyEl && postOnlyEl.checked && state.orderType === 'limit') {
            effectiveOrderType = 'post-only';
        }

        // Run full preflight validation
        const preflight = await preflightOrder({
            side: state.orderSide,
            orderType: effectiveOrderType,
            price, amount, stopPrice,
            pair: state.activePair,
            tradeMode: state.tradeMode,
            leverage: state.leverageValue,
        });
        if (!preflight.ok) {
            showNotification(preflight.error, 'warning');
            return;
        }

        // Task 3.5: Order confirmation dialog for margin trades or spot orders > $100 equivalent
        const estTotal = price * amount;
        const skipConfirm = localStorage.getItem('dexSkipOrderConfirm') === 'true';
        const needsConfirm = !skipConfirm && (state.tradeMode === 'margin' || estTotal > 100);
        if (needsConfirm) {
            const confirmed = await showOrderConfirmation({
                side: state.orderSide,
                type: effectiveOrderType,
                price: price,
                amount: amount,
                total: estTotal,
                pair: state.activePair?.id || '',
                base: state.activePair?.base || '',
                quote: state.activePair?.quote || '',
                leverage: state.tradeMode === 'margin' ? state.leverageValue : null,
                isMargin: state.tradeMode === 'margin',
                stopPrice: effectiveOrderType === 'stop-limit' ? stopPrice : null,
            });
            if (!confirmed) return;
        }

        submitBtn.disabled = true; submitBtn.textContent = 'Submitting...';
        try {
            // Route to margin contract when tradeMode is margin
            if (state.tradeMode === 'margin') {
                const marginSide = state.orderSide === 'buy' ? 'long' : 'short';
                const size = Math.round(amount * PRICE_SCALE);
                const leverage = state.leverageValue;
                const notional = amount * (price || state.lastPrice);
                const quoteAvailable = getQuoteAvailableMargin();
                const isolatedMarginDeposit = Math.round((notional / leverage) * PRICE_SCALE);
                const crossMarginDeposit = Math.round(quoteAvailable * PRICE_SCALE);
                const marginDeposit = state.marginType === 'cross' ? crossMarginDeposit : isolatedMarginDeposit;
                if (marginDeposit <= 0) {
                    showNotification(`Insufficient ${state.activePair?.quote || 'quote'} balance for margin`, 'warning');
                    return;
                }
                const result = await wallet.sendTransaction([contractIx(
                    contracts.dex_margin,
                    buildOpenPositionArgs(wallet.address, state.activePairId, marginSide, size, leverage, marginDeposit, state.marginType)
                )]);
                showNotification(`${marginSide.toUpperCase()} ${state.leverageValue}x opened: ${formatAmount(amount)} ${state.activePair?.base || ''} @ ${formatPrice(price || state.lastPrice)}`, 'success');
                // Auto-set SL/TP on newly opened position if the user specified values
                const marginSLInput = document.getElementById('marginSL');
                const marginTPInput = document.getElementById('marginTP');
                const slVal = parseFloat(marginSLInput?.value) || 0;
                const tpVal = parseFloat(marginTPInput?.value) || 0;
                if (state.orderType !== 'stop-limit' && (slVal > 0 || tpVal > 0)) {
                    try {
                        // Get position count to determine the new position's ID
                        const { data: posData } = await api.get(`/margin/positions?trader=${wallet.address}`);
                        const openPositions = Array.isArray(posData) ? posData.filter(p => p.status === 'open' || p.status === 0) : [];
                        const newPos = openPositions.length > 0 ? openPositions[openPositions.length - 1] : null;
                        if (newPos && newPos.positionId) {
                            await wallet.sendTransaction([contractIx(
                                contracts.dex_margin,
                                buildSetPositionSlTpArgs(wallet.address, newPos.positionId, slVal > 0 ? Math.round(slVal * PRICE_SCALE) : 0, tpVal > 0 ? Math.round(tpVal * PRICE_SCALE) : 0)
                            )]);
                            showNotification(`SL/TP set: ${slVal > 0 ? 'SL @ ' + formatPrice(slVal) : ''}${slVal > 0 && tpVal > 0 ? ' / ' : ''}${tpVal > 0 ? 'TP @ ' + formatPrice(tpVal) : ''}`, 'success');
                            if (marginSLInput) marginSLInput.value = '';
                            if (marginTPInput) marginTPInput.value = '';
                        }
                    } catch (e) { showNotification('Position opened but SL/TP failed: ' + e.message, 'warning'); }
                }
                // F17.8: Immediate panel refresh after margin trade
                loadMarginPositions().catch(() => { });
            } else {
                // G2-04: Check reduce-only checkbox state for the contract flag
                const reduceOnlyEl = document.getElementById('reduceOnly');
                const isReduceOnly = !!(reduceOnlyEl && reduceOnlyEl.checked);
                const result = await wallet.sendTransaction([contractIx(
                    contracts.dex_core,
                    buildPlaceOrderArgs(wallet.address, state.activePairId, state.orderSide, effectiveOrderType, Math.round(price * PRICE_SCALE), Math.round(amount * PRICE_SCALE), effectiveOrderType === 'stop-limit' ? Math.round(stopPrice * PRICE_SCALE) : 0, isReduceOnly)
                )]);
                showNotification(`${state.orderSide.toUpperCase()} order placed: ${formatAmount(amount)} ${state.activePair?.base || ''} @ ${effectiveOrderType === 'market' ? 'MARKET' : formatPrice(price)}`, 'success');
                // F24.16: Refresh from API instead of pushing client-side stub (avoids stale/duplicate entries)
                loadTradeHistory().catch(() => { });
                loadUserOrders(wallet.address).catch(() => { });
            }
            if (amountInput) amountInput.value = ''; if (totalInput) totalInput.value = '';
            // F17.8: Immediate panel refresh after trade execution — update balances + orderbook
            if (wallet.address) loadBalances(wallet.address).then(() => renderBalances()).catch(() => { });
            loadOrderBook().catch(() => { });
        } catch (e) { showNotification(`Order failed: ${e.message}`, 'error'); }
        finally { submitBtn.disabled = false; updateSubmitBtn(); }
    });

    // ═══════════════════════════════════════════════════════════════════════
    // Open Orders
    // ═══════════════════════════════════════════════════════════════════════
    function renderOpenOrders() {
        const tb = document.getElementById('openOrdersBody'), badge = document.querySelector('.orders-badge'); if (!tb) return;
        if (badge) badge.textContent = openOrders.length || '';
        // Task 3.3: Cancel All button in tab header
        const cancelAllBtn = document.getElementById('cancelAllOrdersBtn');
        if (cancelAllBtn) cancelAllBtn.style.display = openOrders.length > 0 ? 'inline-flex' : 'none';
        if (!state.connected) { tb.innerHTML = '<tr><td colspan="9" style="text-align:center;color:var(--text-muted);padding:20px;"><i class="fas fa-wallet" style="margin-right:6px;"></i>Connect wallet to view orders</td></tr>'; return; }
        if (!openOrders.length) { tb.innerHTML = '<tr><td colspan="9" style="text-align:center;color:var(--text-muted);padding:20px;">No open orders</td></tr>'; return; }
        tb.innerHTML = openOrders.map(o => `<tr class="order-row" data-order-id="${escapeHtml(String(o.id))}"><td>${escapeHtml(o.pair)}</td><td class="side-${escapeHtml(o.side)}">${escapeHtml(o.side.toUpperCase())}</td><td style="text-transform:capitalize">${escapeHtml(o.type)}</td><td class="order-price-cell">${formatPrice(o.price)}</td><td class="order-qty-cell">${formatAmount(o.amount)}</td><td>${(o.filled * 100).toFixed(0)}%</td><td>${o.time instanceof Date ? o.time.toLocaleTimeString() : ''}</td><td><button class="edit-order-btn" data-id="${escapeHtml(String(o.id))}" data-price="${o.price}" data-amount="${o.amount}" title="Edit order"><i class="fas fa-pencil-alt"></i></button></td><td><button class="cancel-btn" data-id="${escapeHtml(String(o.id))}"><i class="fas fa-times"></i></button></td></tr>`).join('');
        // Task 3.4: Edit order buttons
        tb.querySelectorAll('.edit-order-btn').forEach(btn => btn.addEventListener('click', () => {
            if (!state.connected || !walletCanSign()) { showNotification('Reconnect wallet to sign', 'warning'); return; }
            const orderId = btn.dataset.id;
            const row = btn.closest('tr');
            if (!row) return;
            const priceCell = row.querySelector('.order-price-cell');
            const qtyCell = row.querySelector('.order-qty-cell');
            if (!priceCell || !qtyCell) return;
            // Toggle inline editing
            if (row.classList.contains('editing')) {
                row.classList.remove('editing');
                const origOrder = openOrders.find(o => o.id === orderId);
                priceCell.textContent = formatPrice(origOrder?.price || 0);
                qtyCell.textContent = formatAmount(origOrder?.amount || 0);
                return;
            }
            row.classList.add('editing');
            const origPrice = parseFloat(btn.dataset.price) || 0;
            const origAmount = parseFloat(btn.dataset.amount) || 0;
            priceCell.innerHTML = `<input type="number" class="edit-price-input" value="${origPrice}" step="0.0001" style="width:80px;padding:2px 4px;font-size:0.78rem;background:var(--bg-input);color:var(--text-primary);border:1px solid var(--teal-primary);border-radius:3px;font-family:'JetBrains Mono',monospace;">`;
            qtyCell.innerHTML = `<input type="number" class="edit-qty-input" value="${origAmount}" step="0.01" style="width:70px;padding:2px 4px;font-size:0.78rem;background:var(--bg-input);color:var(--text-primary);border:1px solid var(--teal-primary);border-radius:3px;font-family:'JetBrains Mono',monospace;">`;
            // Change pencil to save icon
            btn.innerHTML = '<i class="fas fa-check" style="color:var(--green-success);"></i>';
            btn.title = 'Save changes';
            // Re-bind this button for save action
            btn.replaceWith(btn.cloneNode(true));
            const saveBtn = row.querySelector('.edit-order-btn');
            saveBtn.addEventListener('click', async () => {
                const newPrice = parseFloat(row.querySelector('.edit-price-input')?.value);
                const newQty = parseFloat(row.querySelector('.edit-qty-input')?.value);
                if (!newPrice || newPrice <= 0 || !newQty || newQty <= 0) {
                    showNotification('Enter valid price and amount', 'warning');
                    return;
                }
                if (newPrice > 9_000_000 || newQty > 9_000_000) {
                    showNotification('Value too large', 'warning');
                    return;
                }
                saveBtn.disabled = true;
                try {
                    await wallet.sendTransaction([contractIx(
                        contracts.dex_core,
                        buildModifyOrderArgs(wallet.address, parseInt(orderId), Math.round(newPrice * PRICE_SCALE), Math.round(newQty * PRICE_SCALE))
                    )]);
                    showNotification('Order modified', 'success');
                    // Refresh orders from API
                    await loadUserOrders(wallet.address);
                    if (wallet.address) loadBalances(wallet.address).then(() => renderBalances()).catch(() => { });
                    loadOrderBook().catch(() => { });
                } catch (e) {
                    showNotification(`Modify failed: ${e.message}`, 'error');
                }
                saveBtn.disabled = false;
            });
        }));
        tb.querySelectorAll('.cancel-btn').forEach(btn => btn.addEventListener('click', async () => {
            // AUDIT-FIX F10.2: Cancel order via signed sendTransaction (not unsigned DELETE)
            if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
            if (!walletCanSign()) { showNotification('Reconnect wallet to sign', 'warning'); return; }
            try {
                await wallet.sendTransaction([contractIx(
                    contracts.dex_core,
                    buildCancelOrderArgs(wallet.address, parseInt(btn.dataset.id) || 0)
                )]);
                // F20.11: Only update local state and show success after confirmed cancel
                openOrders = openOrders.filter(o => o.id !== btn.dataset.id); renderOpenOrders(); showNotification('Order cancelled', 'info');
                // F24.5b: Refresh balances and orderbook after cancel
                if (wallet.address) loadBalances(wallet.address).then(() => renderBalances()).catch(() => { });
                loadOrderBook().catch(() => { });
            } catch (e) { showNotification(`Cancel failed: ${e.message}`, 'error'); }
        }));
    }

    // Task 3.3: Cancel All Orders button handler
    const cancelAllOrdersBtn = document.getElementById('cancelAllOrdersBtn');
    if (cancelAllOrdersBtn) {
        cancelAllOrdersBtn.addEventListener('click', async () => {
            if (!state.connected || !walletCanSign()) {
                showNotification('Connect wallet first', 'warning');
                return;
            }
            if (!openOrders.length) {
                showNotification('No open orders to cancel', 'info');
                return;
            }
            const pairLabel = state.activePair || `pair ${state.activePairId}`;
            if (!confirm(`Cancel all ${openOrders.length} open order(s) on ${pairLabel}?`)) return;
            cancelAllOrdersBtn.disabled = true;
            try {
                await wallet.sendTransaction([contractIx(
                    contracts.dex_core,
                    buildCancelAllOrdersArgs(wallet.address, state.activePairId)
                )]);
                openOrders = [];
                renderOpenOrders();
                showNotification('All orders cancelled', 'success');
                if (wallet.address) loadBalances(wallet.address).then(() => renderBalances()).catch(() => { });
                loadOrderBook().catch(() => { });
            } catch (e) {
                showNotification(`Cancel all failed: ${e.message}`, 'error');
            }
            cancelAllOrdersBtn.disabled = false;
        });
    }

    document.querySelectorAll('.positions-tabs').forEach(tabGroup => {
        const panelRoot = tabGroup.closest('.positions-panel, .panel-card') || document;
        const tabs = Array.from(tabGroup.querySelectorAll('.pos-tab'));
        tabs.forEach(tab => tab.addEventListener('click', () => {
            tabs.forEach(t => t.classList.remove('active'));
            tab.classList.add('active');
            panelRoot.querySelectorAll('.positions-content').forEach(content => content.classList.add('hidden'));
            const target = document.getElementById(tab.dataset.target);
            if (target) target.classList.remove('hidden');
            if (tab.dataset.target === 'content-positions') {
                loadMarginStats();
                loadMarginPositions();
            } else if (tab.dataset.target === 'content-history') {
                loadTradeHistory();
            } else if (tab.dataset.target === 'content-margin-history') {
                loadMarginHistory();
            } else if (tab.dataset.target === 'predict-positions') {
                loadPredictionPositions();
            } else if (tab.dataset.target === 'predict-history') {
                loadPredictionHistory();
            } else if (tab.dataset.target === 'predict-created') {
                loadCreatedMarkets();
            }
        }));
    });

    // ═══════════════════════════════════════════════════════════════════════
    // Wallet UI
    // ═══════════════════════════════════════════════════════════════════════
    const connectBtn = document.getElementById('connectWallet'), walletModal = document.getElementById('walletModal'), closeModalBtn = document.getElementById('closeWalletModal');
    const wmTabs = document.querySelectorAll('.wm-tab'), wmTC = { wallets: document.getElementById('wmTabWallets'), import: document.getElementById('wmTabImport'), extension: document.getElementById('wmTabExtension'), create: document.getElementById('wmTabCreate') };
    const ACTIVE_WALLET_KEY = 'dexActiveWallet';
    const LOCAL_WALLET_SESSION_KEY = 'dexWalletSessionsV1';
    const LOCAL_WALLET_PASSWORD_PREFIX = 'dexWalletPassword:';
    let savedWallets = (() => {
        try {
            const parsed = JSON.parse(localStorage.getItem('dexWallets') || '[]');
            if (!Array.isArray(parsed)) return [];
            return parsed.filter(w => w && typeof w.address === 'string' && w.address.length > 0);
        } catch {
            return [];
        }
    })();
    const localWalletSessions = new Map();
    let persistedLocalWalletSessions = (() => {
        try {
            const parsed = JSON.parse(sessionStorage.getItem(LOCAL_WALLET_SESSION_KEY) || '{}');
            return parsed && typeof parsed === 'object' ? parsed : {};
        } catch {
            return {};
        }
    })();

    function getWalletPasswordCacheKey(address) {
        return `${LOCAL_WALLET_PASSWORD_PREFIX}${address}`;
    }

    async function deriveSessionKey(password, saltBytes) {
        const passwordKey = await crypto.subtle.importKey(
            'raw',
            new TextEncoder().encode(password),
            'PBKDF2',
            false,
            ['deriveKey']
        );
        return crypto.subtle.deriveKey(
            {
                name: 'PBKDF2',
                salt: saltBytes,
                iterations: 100000,
                hash: 'SHA-256'
            },
            passwordKey,
            { name: 'AES-GCM', length: 256 },
            false,
            ['encrypt', 'decrypt']
        );
    }

    async function encryptSeedHex(seedHex, password) {
        if (window.LichenCrypto && typeof window.LichenCrypto.encryptPrivateKey === 'function') {
            return window.LichenCrypto.encryptPrivateKey(seedHex, password);
        }
        const salt = crypto.getRandomValues(new Uint8Array(16));
        const key = await deriveSessionKey(password, salt);
        const iv = crypto.getRandomValues(new Uint8Array(12));
        const payload = new TextEncoder().encode(seedHex);
        const encrypted = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, payload);
        return {
            encrypted: bytesToHex(new Uint8Array(encrypted)),
            salt: bytesToHex(salt),
            iv: bytesToHex(iv)
        };
    }

    async function decryptSeedHex(encryptedData, password) {
        if (window.LichenCrypto && typeof window.LichenCrypto.decryptPrivateKey === 'function') {
            return window.LichenCrypto.decryptPrivateKey(encryptedData, password);
        }
        const saltBytes = hexToBytes(encryptedData.salt || '');
        const ivBytes = hexToBytes(encryptedData.iv || '');
        const encryptedBytes = hexToBytes(encryptedData.encrypted || '');
        const key = await deriveSessionKey(password, saltBytes);
        try {
            const decrypted = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: ivBytes }, key, encryptedBytes);
            return new TextDecoder().decode(decrypted);
        } catch {
            throw new Error('Invalid password');
        }
    }

    function walletSeedHex(keypair) {
        if (!keypair || !keypair.secretKey) return '';
        return bytesToHex(keypair.secretKey.slice(0, 32));
    }

    function persistSessionStoragePassword(address, password) {
        if (!address || !password) return;
        try { sessionStorage.setItem(getWalletPasswordCacheKey(address), password); } catch { }
    }

    function getCachedSessionPassword(address) {
        if (!address) return '';
        try { return sessionStorage.getItem(getWalletPasswordCacheKey(address)) || ''; } catch { return ''; }
    }

    function clearCachedSessionPassword(address) {
        if (!address) return;
        try { sessionStorage.removeItem(getWalletPasswordCacheKey(address)); } catch { }
    }

    // DEX-10: Wallet sessions are AES-encrypted before storage.
    // Use sessionStorage (cleared on tab close) instead of localStorage
    // to limit the attack window for XSS-based session theft.
    async function persistLocalWalletSession(address, keypair, password) {
        if (!address || !keypair || !keypair.secretKey) return;
        if (!password) throw new Error('Password is required');
        const seedHex = walletSeedHex(keypair);
        const encrypted = await encryptSeedHex(seedHex, password);
        persistedLocalWalletSessions[address] = { ...encrypted, updatedAt: Date.now() };
        try { sessionStorage.setItem(LOCAL_WALLET_SESSION_KEY, JSON.stringify(persistedLocalWalletSessions)); } catch { }
        persistSessionStoragePassword(address, password);
    }

    function removePersistedLocalWalletSession(address) {
        if (!address) return;
        delete persistedLocalWalletSessions[address];
        localWalletSessions.delete(address);
        clearCachedSessionPassword(address);
        try { sessionStorage.setItem(LOCAL_WALLET_SESSION_KEY, JSON.stringify(persistedLocalWalletSessions)); } catch { }
    }

    function restoreLocalWalletSessions() {
        try {
            const parsed = JSON.parse(sessionStorage.getItem(LOCAL_WALLET_SESSION_KEY) || '{}');
            persistedLocalWalletSessions = parsed && typeof parsed === 'object' ? parsed : {};
        } catch {
            persistedLocalWalletSessions = {};
        }
    }

    async function unlockLocalWalletSession(address) {
        if (!address) return null;
        const existing = localWalletSessions.get(address);
        if (existing) return existing;
        const encrypted = persistedLocalWalletSessions[address];
        if (!encrypted || !encrypted.encrypted || !encrypted.salt || !encrypted.iv) return null;

        let password = getCachedSessionPassword(address);
        if (!password) {
            const entered = window.prompt('Enter wallet password to unlock signing for this address.');
            if (!entered) return null;
            password = entered;
        }
        try {
            const seedHex = await decryptSeedHex(encrypted, password);
            const seedBytes = hexToBytes(seedHex);
            if (seedBytes.length !== 32) throw new Error('Invalid wallet seed length');
            const kp = nacl.sign.keyPair.fromSeed(seedBytes);
            const derivedAddress = bs58encode(kp.publicKey);
            if (String(derivedAddress) !== String(address)) throw new Error('Password does not match wallet');
            localWalletSessions.set(address, kp);
            persistSessionStoragePassword(address, password);
            return kp;
        } catch {
            clearCachedSessionPassword(address);
            return null;
        }
    }

    async function tryHydrateExtensionSigner(targetAddress) {
        if (!targetAddress) return false;
        try {
            const ext = await wallet.connect();
            return !!(ext?.address && String(ext.address).trim() === String(targetAddress).trim());
        } catch {
            return false;
        }
    }

    async function resolveWalletSigningState(address) {
        const sessionKp = await unlockLocalWalletSession(address);
        if (sessionKp) {
            return { signingReady: true, localKeypair: sessionKp, source: 'local' };
        }
        const extensionReady = await tryHydrateExtensionSigner(address);
        return { signingReady: extensionReady, localKeypair: null, source: extensionReady ? 'extension' : 'none' };
    }

    restoreLocalWalletSessions();

    function openWalletModal() { if (walletModal) { walletModal.classList.remove('hidden'); renderWalletList(); switchWmTab(savedWallets.length ? 'wallets' : 'extension'); } }
    function closeWalletModalFn() {
        if (walletModal) walletModal.classList.add('hidden');
        resetWalletModalInputs();
    }
    function switchWmTab(t) { wmTabs.forEach(x => x.classList.toggle('active', x.dataset.wmTab === t)); Object.entries(wmTC).forEach(([k, el]) => { if (el) el.classList.toggle('hidden', k !== t); }); }
    function resetWalletModalInputs(options = {}) {
        const { clearCreated = true } = options;
        const privateKeyInput = document.getElementById('wmPrivateKey');
        if (privateKeyInput) privateKeyInput.value = '';

        const passwordInput = document.getElementById('wmPassword');
        if (passwordInput) passwordInput.value = '';

        const createPasswordInput = document.getElementById('wmCreatePassword');
        if (createPasswordInput) createPasswordInput.value = '';

        const importTypeButtons = document.querySelectorAll('.wm-import-type');
        importTypeButtons.forEach(btn => btn.classList.toggle('active', btn.dataset.import === 'key'));

        const importKeyPanel = document.getElementById('wmImportKey');
        const importMnemonicPanel = document.getElementById('wmImportMnemonic');
        if (importKeyPanel) importKeyPanel.classList.remove('hidden');
        if (importMnemonicPanel) importMnemonicPanel.classList.add('hidden');

        const mnemonicInputs = document.querySelectorAll('#mnemonicGrid input');
        mnemonicInputs.forEach((input, idx) => {
            input.value = '';
            input.style.display = idx >= 12 ? 'none' : '';
        });

        const createdBox = document.getElementById('wmCreatedWallet');
        const newAddrEl = document.getElementById('wmNewAddress');
        const newKeyEl = document.getElementById('wmNewKey');
        const createBtn = document.getElementById('wmCreateBtn');
        if (clearCreated) {
            if (createdBox) createdBox.classList.add('hidden');
            if (newAddrEl) newAddrEl.textContent = '';
            if (newKeyEl) newKeyEl.textContent = '';
            if (createBtn) createBtn.classList.remove('hidden');
        }
    }

    if (connectBtn) connectBtn.addEventListener('click', () => openWalletModal());
    if (closeModalBtn) closeModalBtn.addEventListener('click', closeWalletModalFn);
    if (walletModal) walletModal.addEventListener('click', e => { if (e.target === walletModal) closeWalletModalFn(); });
    document.addEventListener('keydown', e => { if (e.key === 'Escape' && walletModal && !walletModal.classList.contains('hidden')) closeWalletModalFn(); });
    wmTabs.forEach(t => t.addEventListener('click', () => switchWmTab(t.dataset.wmTab)));

    // Import-type toggle (Private Key / Mnemonic)
    document.querySelectorAll('.wm-import-type').forEach(btn => btn.addEventListener('click', () => {
        document.querySelectorAll('.wm-import-type').forEach(b => b.classList.remove('active')); btn.classList.add('active');
        const k = document.getElementById('wmImportKey'), m = document.getElementById('wmImportMnemonic');
        if (btn.dataset.import === 'key') { if (k) k.classList.remove('hidden'); if (m) m.classList.add('hidden'); } else { if (k) k.classList.add('hidden'); if (m) m.classList.remove('hidden'); }
    }));

    // Build mnemonic word grid (supports 12 or 24 words — show 12 by default, expand on paste)
    const mnGrid = document.getElementById('mnemonicGrid');
    if (mnGrid) {
        for (let i = 0; i < 24; i++) {
            const inp = document.createElement('input'); inp.type = 'text'; inp.placeholder = `Word ${i + 1}`; inp.className = 'form-input'; inp.dataset.wordIdx = i;
            if (i >= 12) inp.style.display = 'none'; // hide 13-24 by default
            mnGrid.appendChild(inp);
        }
        // Handle paste of full mnemonic phrase into any word input
        mnGrid.addEventListener('paste', (e) => {
            const text = (e.clipboardData || window.clipboardData).getData('text').trim();
            const words = text.split(/\s+/);
            if (words.length >= 2) {
                e.preventDefault();
                const inputs = mnGrid.querySelectorAll('input');
                // Show all 24 fields if >12 words
                if (words.length > 12) inputs.forEach(inp => inp.style.display = '');
                words.forEach((w, i) => { if (inputs[i]) inputs[i].value = w; });
            }
        });
    }

    // Import tab — connect from private key or mnemonic
    const wmConnectBtn = document.getElementById('wmConnectBtn');
    if (wmConnectBtn) wmConnectBtn.addEventListener('click', async () => {
        try {
            const activeImport = document.querySelector('.wm-import-type.active')?.dataset.import || 'key';
            const password = (document.getElementById('wmPassword')?.value || '').trim();
            if (!password) throw new Error('Password is required');
            let connectedWallet;
            if (activeImport === 'mnemonic') {
                const words = Array.from(document.querySelectorAll('#mnemonicGrid input'))
                    .map(i => (i.value || '').trim())
                    .filter(Boolean);
                if (words.length !== 12 && words.length !== 24) throw new Error('Mnemonic must have 12 or 24 words');
                const phrase = words.join(' ').toLowerCase();
                // BIP39: PBKDF2-HMAC-SHA512, salt="mnemonic", 2048 iterations (matches wallet app & extension)
                const mnemonicBytes = new TextEncoder().encode(phrase.normalize('NFKD').trim());
                const saltBytes = new TextEncoder().encode('mnemonic');
                const keyMaterial = await crypto.subtle.importKey('raw', mnemonicBytes, 'PBKDF2', false, ['deriveBits']);
                const seedBuffer = await crypto.subtle.deriveBits(
                    { name: 'PBKDF2', salt: saltBytes, iterations: 2048, hash: 'SHA-512' },
                    keyMaterial, 512
                );
                const seed = new Uint8Array(seedBuffer).slice(0, 32);
                const kp = nacl.sign.keyPair.fromSeed(seed);
                connectedWallet = await wallet.connectAddress(bs58encode(kp.publicKey), { signingReady: true, localKeypair: kp });
            } else {
                const pkInput = document.getElementById('wmPrivateKey')?.value || '';
                connectedWallet = await wallet.fromSecretKey(pkInput);
            }

            localWalletSessions.set(connectedWallet.address, connectedWallet.keypair);
            await persistLocalWalletSession(connectedWallet.address, connectedWallet.keypair, password);
            if (!savedWallets.some(w => w.address === connectedWallet.address)) {
                savedWallets.push({ address: connectedWallet.address, short: connectedWallet.shortAddr, added: Date.now() });
                localStorage.setItem('dexWallets', JSON.stringify(savedWallets));
            }
            await connectWalletTo(connectedWallet.address, connectedWallet.shortAddr, { signingReady: true, localKeypair: connectedWallet.keypair });
            closeWalletModalFn();
            showNotification('Wallet connected: ' + connectedWallet.shortAddr, 'success');
        } catch (e) {
            showNotification(`Import failed: ${e.message}`, 'error');
        }
    });

    // Extension tab — connect via wallet extension
    const wmExtensionBtn = document.getElementById('wmExtensionBtn');
    if (wmExtensionBtn) wmExtensionBtn.addEventListener('click', async () => {
        try {
            await wallet.connect();
            if (!savedWallets.some(w => w.address === wallet.address)) {
                savedWallets.push({ address: wallet.address, short: wallet.shortAddr, added: Date.now() });
                localStorage.setItem('dexWallets', JSON.stringify(savedWallets));
            }
            await connectWalletTo(wallet.address, wallet.shortAddr, { signingReady: true });
            closeWalletModalFn();
            showNotification('Wallet connected: ' + wallet.shortAddr, 'success');
        } catch (e) { showNotification(`Extension connection failed: ${e.message}`, 'error'); }
    });

    // Create tab — generate a new Ed25519 keypair
    const wmCreateBtn = document.getElementById('wmCreateBtn');
    if (wmCreateBtn) wmCreateBtn.addEventListener('click', async () => {
        try {
            const createPassword = (document.getElementById('wmCreatePassword')?.value || '').trim();
            if (!createPassword) throw new Error('Password is required');
            const generated = await wallet.generate();
            localWalletSessions.set(generated.address, generated.keypair);
            await persistLocalWalletSession(generated.address, generated.keypair, createPassword);
            if (!savedWallets.some(w => w.address === generated.address)) {
                savedWallets.push({ address: generated.address, short: generated.shortAddr, added: Date.now() });
                localStorage.setItem('dexWallets', JSON.stringify(savedWallets));
            }
            const newAddrEl = document.getElementById('wmNewAddress');
            const newKeyEl = document.getElementById('wmNewKey');
            const createdBox = document.getElementById('wmCreatedWallet');
            if (newAddrEl) newAddrEl.textContent = generated.address;
            if (newKeyEl) newKeyEl.textContent = keypairSeedHex(generated.keypair);
            if (createdBox) createdBox.classList.remove('hidden');
            if (wmCreateBtn) wmCreateBtn.classList.add('hidden');
            await connectWalletTo(generated.address, generated.shortAddr, { signingReady: true, localKeypair: generated.keypair, preserveCreatedDetails: true });
            showNotification('New wallet created and connected', 'success');
        } catch (e) {
            showNotification(`Create wallet failed: ${e.message}`, 'error');
        }
    });

    document.querySelectorAll('.wm-copy-btn').forEach(btn => btn.addEventListener('click', () => { const el = document.getElementById(btn.dataset.copy); if (el) navigator.clipboard.writeText(el.textContent).then(() => showNotification('Copied!', 'success')); }));

    async function connectWalletTo(address, shortAddr, options = {}) {
        const { signingReady = false, localKeypair = null, preserveCreatedDetails = false } = options;
        state.connected = true; state.walletAddress = address;
        localStorage.setItem(ACTIVE_WALLET_KEY, address);
        // AUDIT-FIX I4-01: Keep wallet object in sync (address + compatibility flag)
        let resolvedSessionKey = localKeypair || localWalletSessions.get(address) || null;
        let resolvedSigningReady = !!signingReady;
        if (!resolvedSessionKey && !resolvedSigningReady) {
            const resolved = await resolveWalletSigningState(address);
            resolvedSessionKey = resolved.localKeypair || null;
            resolvedSigningReady = !!resolved.signingReady;
        }
        await wallet.connectAddress(address, { signingReady: resolvedSigningReady || !!resolvedSessionKey, localKeypair: resolvedSessionKey });
        // M16: Resolve .lichen name and fetch LichenID profile for connected trader
        let displayLabel = shortAddr;
        try {
            const reverseResult = await api.rpc('reverseLichenName', [address]);
            if (reverseResult && reverseResult.name) {
                state.lichenName = reverseResult.name + '.lichen';
                displayLabel = state.lichenName;
            } else {
                state.lichenName = null;
            }
        } catch { state.lichenName = null; }
        try {
            const profileResult = await api.rpc('getLichenIdProfile', [address]);
            if (profileResult) {
                state.lichenIdProfile = profileResult;
                state.reputation = profileResult.reputation || 0;
                state.trustTier = profileResult.trustTier || profileResult.trust_tier || 0;
            } else {
                state.lichenIdProfile = null; state.reputation = 0; state.trustTier = 0;
            }
        } catch { state.lichenIdProfile = null; state.reputation = 0; state.trustTier = 0; }
        const repBadge = state.reputation > 0
            ? ` <span class="lichenid-rep-badge" title="LichenID Reputation: ${state.reputation}">\u2b50${state.reputation}</span>`
            : '';
        if (connectBtn) { connectBtn.innerHTML = `<i class="fas fa-wallet"></i> ${escapeHtml(displayLabel)}${repBadge}`; connectBtn.className = 'btn btn-small btn-secondary'; }
        toggleWalletPanels(true);
        applyWalletGateAll();
        resetWalletModalInputs({ clearCreated: !preserveCreatedDetails });
        await Promise.all([loadBalances(address), loadUserOrders(address)]);
        renderBalances(); renderOpenOrders(); loadTradeHistory(); loadMarginStats(); loadMarginPositions(); loadMarginHistory();
        loadPredictionHistory();
        renderWalletList();
        if (dexWs && state.activePairId != null) subscribePair(state.activePairId);
    }

    function disconnectWallet() {
        state.connected = false; state.walletAddress = null; wallet.keypair = null; wallet.signingReady = false; wallet.address = null; wallet._lichenWallet = null;
        localStorage.removeItem(ACTIVE_WALLET_KEY);
        state.lichenName = null; state.lichenIdProfile = null; state.reputation = 0; state.trustTier = 0;
        if (connectBtn) { connectBtn.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet'; connectBtn.className = 'btn btn-small btn-primary'; }
        openOrders = []; balances = {};
        toggleWalletPanels(false);
        applyWalletGateAll();
        renderBalances(); renderOpenOrders();
        // Clear wallet-gated sections
        loadTradeHistory(); loadMarginPositions(); loadMarginHistory(); loadLPPositions(); loadPredictionPositions(); loadPredictionHistory(); loadCreatedMarkets();
    }

    function toggleWalletPanels(show) {
        const bp = document.getElementById('walletBalancePanel');
        const tp = document.getElementById('tradeBottomPanel');
        // F10E.3: Toggle ALL bottom panels consistently across views
        const pp = document.getElementById('predictBottomPanel');
        const plp = document.getElementById('poolBottomPanel');
        const rp = document.getElementById('rewardsBottomPanel');
        if (bp) bp.classList.toggle('hidden', !show);
        if (tp) tp.classList.toggle('hidden', !show);
        if (pp) pp.classList.toggle('hidden', !show);
        if (plp) plp.classList.toggle('hidden', !show);
        if (rp) rp.classList.toggle('hidden', !show);
    }

    async function renderWalletList() {
        const list = document.getElementById('wmWalletsList'); if (!list) return;
        if (!savedWallets.length) { list.innerHTML = `<div class="wm-empty"><i class="fas fa-wallet"></i><p>No wallets connected</p><button class="btn btn-primary btn-small" id="wmEmptyImport">Import Wallet</button></div>`; const b = document.getElementById('wmEmptyImport'); if (b) b.addEventListener('click', () => switchWmTab('import')); return; }
        // M16: Batch-resolve .lichen names for saved wallets
        const nameMap = {};
        try {
            const result = await api.rpc('batchReverseLichenNames', [savedWallets.map(w => w.address)]);
            if (result && typeof result === 'object') { for (const [addr, name] of Object.entries(result)) { if (name) nameMap[addr] = name + '.lichen'; } }
        } catch { /* RPC unavailable — show plain addresses */ }
        list.innerHTML = savedWallets.map((w, i) => {
            const label = nameMap[w.address] || w.short || w.address.slice(0, 8) + '...' + w.address.slice(-6);
            return `<div class="wm-wallet-item ${state.walletAddress === w.address ? 'active-wallet' : ''}"><span class="wm-wallet-addr">${escapeHtml(label)}</span><div class="wm-wallet-actions">${state.walletAddress === w.address ? '<span class="btn btn-small btn-secondary" style="opacity:0.6;cursor:default;">Active</span>' : `<button class="btn btn-small btn-primary wm-switch-btn" data-idx="${i}">Switch</button>`}<button class="btn btn-small btn-secondary wm-remove-btn" data-idx="${i}"><i class="fas fa-times"></i></button></div></div>`;
        }).join('') + `<div class="wm-disconnect-all"><button class="btn btn-small btn-secondary" id="wmDisconnectAll">Disconnect All</button></div>`;
        list.querySelectorAll('.wm-switch-btn').forEach(btn => btn.addEventListener('click', async () => {
            const w = savedWallets[parseInt(btn.dataset.idx)];
            if (w) {
                const currentAddress = state.walletAddress;
                const { signingReady, localKeypair } = await resolveWalletSigningState(w.address);
                if (!signingReady && walletCanSign() && currentAddress && currentAddress !== w.address) {
                    showNotification('Switch blocked: target wallet is not ready to sign. Connect matching extension account or import its key first.', 'warning');
                    return;
                }
                if (!signingReady) {
                    showNotification('Switched wallet in read-only mode. Import key or reconnect matching extension account to sign.', 'warning');
                }
                await connectWalletTo(w.address, w.short || w.address.slice(0, 8) + '...', { signingReady, localKeypair });
                renderWalletList();
            }
        }));
        list.querySelectorAll('.wm-remove-btn').forEach(btn => btn.addEventListener('click', async () => {
            const i = parseInt(btn.dataset.idx, 10);
            const removedWallet = savedWallets[i];
            if (Number.isNaN(i) || !removedWallet) return;

            savedWallets.splice(i, 1);
            localStorage.setItem('dexWallets', JSON.stringify(savedWallets));
            removePersistedLocalWalletSession(removedWallet.address);

            if (state.walletAddress === removedWallet.address) {
                const fallbackWallet = savedWallets[0] || null;
                if (fallbackWallet) {
                    const { signingReady, localKeypair } = await resolveWalletSigningState(fallbackWallet.address);
                    await connectWalletTo(
                        fallbackWallet.address,
                        fallbackWallet.short || fallbackWallet.address.slice(0, 8) + '...',
                        { signingReady, localKeypair }
                    );
                    showNotification('Wallet removed. Switched to another wallet', 'info');
                } else {
                    disconnectWallet();
                    showNotification('Wallet removed', 'info');
                }
            } else {
                showNotification('Wallet removed', 'info');
            }

            renderWalletList();
        }));
        const da = document.getElementById('wmDisconnectAll'); if (da) da.addEventListener('click', () => {
            savedWallets = [];
            localWalletSessions.clear();
            persistedLocalWalletSessions = {};
            try {
                Object.keys(sessionStorage)
                    .filter(k => k.startsWith(LOCAL_WALLET_PASSWORD_PREFIX))
                    .forEach(k => sessionStorage.removeItem(k));
            } catch { }
            localStorage.removeItem('dexWallets');
            localStorage.removeItem(ACTIVE_WALLET_KEY);
            sessionStorage.removeItem(LOCAL_WALLET_SESSION_KEY);
            disconnectWallet();
            renderWalletList();
            showNotification('All wallets disconnected', 'info');
        });
    }

    function renderBalances() {
        const c = document.querySelector('.balance-list'); if (!c) return;
        if (!state.connected) { c.innerHTML = ''; renderPortfolioSummary(); return; }
        c.innerHTML = Object.entries(balances).map(([t, b]) => `<div class="balance-row"><div class="balance-token"><div class="token-icon ${escapeHtml(t.toLowerCase())}-icon">${escapeHtml(t[0])}</div><span>${escapeHtml(t)}</span></div><div class="balance-amounts"><span class="balance-available">${formatAmount(b.available)}</span><span class="balance-usd">≈ $${formatAmount(b.usd)}</span></div></div>`).join('');
        renderPortfolioSummary();
        refreshPredictWalletBalance();
    }

    function refreshPredictWalletBalance() {
        const el = document.getElementById('predictMusdBalance');
        if (!el) return;
        if (!state.connected) {
            el.textContent = 'Connect wallet';
            return;
        }
        const available = balances.lUSD?.available ?? 0;
        el.textContent = `${formatAmount(available)} lUSD`;
    }

    // Task 7.1: Derive USD value for a token using pair prices
    function computeTokenUsd(symbol, amount) {
        if (symbol === 'lUSD' || symbol === 'USDT' || symbol === 'USDC') return amount; // stablecoins ≈ $1
        // Find a pair where this symbol is the base (e.g., LICN/lUSD → LICN price)
        const directPair = pairs.find(p => p.base === symbol && (p.quote === 'lUSD' || p.quote === 'USDT' || p.quote === 'USDC'));
        if (directPair && directPair.price > 0) return amount * directPair.price;
        // Find a pair where this symbol is the quote and invert
        const inversePair = pairs.find(p => p.quote === symbol && (p.base === 'lUSD' || p.base === 'USDT' || p.base === 'USDC'));
        if (inversePair && inversePair.price > 0) return amount / inversePair.price;
        // Cross-reference via LICN if available
        const licnPair = pairs.find(p => p.base === symbol && p.quote === 'LICN');
        if (licnPair && licnPair.price > 0) {
            const licnUsd = pairs.find(p => p.base === 'LICN' && (p.quote === 'lUSD' || p.quote === 'USDT'));
            if (licnUsd && licnUsd.price > 0) return amount * licnPair.price * licnUsd.price;
        }
        return 0;
    }

    // Task 7.1: Portfolio summary — total value, unrealized P&L, 24h change
    function computePortfolioSummary() {
        let totalValue = 0;
        Object.values(balances).forEach(b => { totalValue += b.usd || 0; });
        // Cache for 24h comparison
        const cacheKey = 'dexPortfolioCache';
        const now = Date.now();
        let change24h = 0;
        try {
            const cached = JSON.parse(localStorage.getItem(cacheKey));
            if (cached && cached.ts && cached.value !== undefined) {
                const age = now - cached.ts;
                if (age < 86400000) { // within 24h
                    change24h = totalValue - cached.value;
                } else {
                    // Cache expired, save new baseline
                    localStorage.setItem(cacheKey, JSON.stringify({ ts: now, value: totalValue }));
                }
            } else {
                localStorage.setItem(cacheKey, JSON.stringify({ ts: now, value: totalValue }));
            }
        } catch {
            localStorage.setItem(cacheKey, JSON.stringify({ ts: now, value: totalValue }));
        }
        // Update cache if value changed significantly (>1%) or no recent save
        try {
            const cached = JSON.parse(localStorage.getItem(cacheKey));
            if (!cached || (now - cached.ts > 300000)) { // re-cache every 5 min
                localStorage.setItem(cacheKey, JSON.stringify({ ts: now, value: totalValue }));
            }
        } catch { /* ignore */ }
        return { totalValue, change24h };
    }

    function computeUnrealizedPnl() {
        // Sum P&L from current margin positions DOM if available
        const rows = document.querySelectorAll('.margin-pos-row');
        let totalPnl = 0;
        rows.forEach(row => {
            const pnlEl = row.querySelector('.positive, .negative');
            if (pnlEl) {
                const text = pnlEl.textContent || '';
                const match = text.match(/P&L:\s*([+-]?[\d,.]+)/);
                if (match) totalPnl += parseFloat(match[1].replace(/,/g, '')) || 0;
            }
        });
        return totalPnl;
    }

    function renderPortfolioSummary() {
        let container = document.getElementById('portfolioSummary');
        if (!state.connected) {
            if (container) container.innerHTML = '';
            return;
        }
        if (!container) {
            // Create container after balance-list
            const balList = document.querySelector('.balance-list');
            if (!balList) return;
            container = document.createElement('div');
            container.id = 'portfolioSummary';
            container.className = 'portfolio-summary';
            balList.parentNode.insertBefore(container, balList.nextSibling);
        }
        const { totalValue, change24h } = computePortfolioSummary();
        const unrealizedPnl = computeUnrealizedPnl();
        const changeClass = change24h >= 0 ? 'positive' : 'negative';
        const changeSign = change24h >= 0 ? '+' : '';
        const pnlClass = unrealizedPnl >= 0 ? 'positive' : 'negative';
        const pnlSign = unrealizedPnl >= 0 ? '+' : '';
        container.innerHTML = `<div class="portfolio-total"><span class="portfolio-label">Portfolio Value</span><span class="portfolio-value">$${formatAmount(totalValue)}</span></div><div class="portfolio-metrics"><span class="${pnlClass}">P&L: ${pnlSign}$${formatAmount(Math.abs(unrealizedPnl))}</span><span class="portfolio-change ${changeClass}">${changeSign}$${formatAmount(Math.abs(change24h))}</span></div>`;
    }

    // ═══════════════════════════════════════════════════════════════════════
    // F10E.1/E2/E4/E9/E10 — Wallet-Gate All Interactive Forms
    // ═══════════════════════════════════════════════════════════════════════
    function applyWalletGateAll() {
        const connected = !!(state.connected && (state.walletAddress || wallet.address));
        const canSign = walletCanSign();

        // --- Trade view: Order Form Panel (F15.2: gate entire panel including tabs/type/mode) ---
        const orderFormPanel = document.querySelector('.order-form-panel');
        if (orderFormPanel) orderFormPanel.classList.toggle('wallet-gated-disabled', !connected);
        if (submitBtn) {
            if (canSign) {
                submitBtn.disabled = false;
                submitBtn.classList.remove('btn-wallet-gate');
                updateSubmitBtn();
            } else {
                submitBtn.disabled = true;
                submitBtn.className = 'btn-full btn-wallet-gate';
                submitBtn.innerHTML = connected
                    ? '<i class="fas fa-key"></i> Unlock Wallet to Sign'
                    : '<i class="fas fa-wallet"></i> Connect Wallet to Trade';
            }
        }

        // --- Predict view: Quick Trade + Create Market (F10E.2) ---
        const predictTradePanel = document.querySelector('.predict-trade-panel');
        const predictCreatePanel = document.querySelector('.predict-create-panel');
        if (predictTradePanel) predictTradePanel.classList.toggle('wallet-gated-disabled', !connected);
        if (predictCreatePanel) predictCreatePanel.classList.toggle('wallet-gated-disabled', !connected);

        const predictSubmit = document.getElementById('predictSubmitBtn');
        const predictYesToggle = document.getElementById('predictYesBtn');
        const predictNoToggle = document.getElementById('predictNoBtn');
        if (predictYesToggle) {
            predictYesToggle.disabled = !canSign;
            predictYesToggle.title = canSign ? '' : (connected ? 'Reconnect wallet to sign' : 'Connect wallet to trade');
        }
        if (predictNoToggle) {
            predictNoToggle.disabled = !canSign;
            predictNoToggle.title = canSign ? '' : (connected ? 'Reconnect wallet to sign' : 'Connect wallet to trade');
        }
        if (predictSubmit) {
            if (canSign) {
                predictSubmit.disabled = false;
                predictSubmit.classList.remove('btn-wallet-gate');
                const side = (typeof predictState !== 'undefined' && predictState.selectedOutcome === 'no') ? 'NO' : 'YES';
                predictSubmit.innerHTML = `<i class="fas fa-bolt"></i> Buy ${side} Shares`;
            } else {
                predictSubmit.disabled = true;
                predictSubmit.className = 'btn-full btn-wallet-gate';
                predictSubmit.innerHTML = connected
                    ? '<i class="fas fa-key"></i> Unlock Wallet to Sign'
                    : '<i class="fas fa-wallet"></i> Connect Wallet to Trade';
            }
        }
        const predictCreate = document.getElementById('predictCreateBtn');
        if (predictCreate) {
            if (canSign) {
                predictCreate.disabled = false;
                predictCreate.classList.remove('btn-wallet-gate');
                predictCreate.innerHTML = '<i class="fas fa-rocket"></i> Create Market';
            } else {
                predictCreate.disabled = true;
                predictCreate.className = 'btn btn-full btn-wallet-gate';
                predictCreate.innerHTML = connected
                    ? '<i class="fas fa-key"></i> Unlock Wallet to Sign'
                    : '<i class="fas fa-wallet"></i> Connect Wallet to Create';
            }
        }

        // --- Pool view: Add Liquidity (F10E.10) ---
        const addLiqForm = document.getElementById('addLiqForm');
        if (addLiqForm) addLiqForm.classList.toggle('wallet-gated-disabled', !connected);
        const addLiqSubmit = document.getElementById('addLiqBtn');
        if (addLiqSubmit) {
            if (canSign) {
                addLiqSubmit.disabled = false;
                addLiqSubmit.classList.remove('btn-wallet-gate');
                addLiqSubmit.textContent = 'Add Liquidity';
            } else {
                addLiqSubmit.disabled = true;
                addLiqSubmit.className = 'btn btn-full btn-wallet-gate';
                addLiqSubmit.innerHTML = connected
                    ? '<i class="fas fa-key"></i> Unlock Wallet to Sign'
                    : '<i class="fas fa-wallet"></i> Connect Wallet';
            }
        }

        // --- Governance: New Proposal (F10E.4) ---
        const proposalForm = document.getElementById('proposalForm');
        if (proposalForm) proposalForm.classList.toggle('wallet-gated-disabled', !connected);
        const proposalSubmit = document.getElementById('submitProposalBtn');
        if (proposalSubmit) {
            if (canSign) {
                proposalSubmit.disabled = false;
                proposalSubmit.classList.remove('btn-wallet-gate');
                proposalSubmit.innerHTML = '<i class="fas fa-paper-plane"></i> Submit Proposal';
            } else {
                proposalSubmit.disabled = true;
                proposalSubmit.className = 'btn btn-full btn-wallet-gate';
                proposalSubmit.innerHTML = connected
                    ? '<i class="fas fa-key"></i> Unlock Wallet to Sign'
                    : '<i class="fas fa-wallet"></i> Connect Wallet to Propose';
            }
        }

        // --- Rewards: Claim button (already has wallet check, but disable visually) ---
        const claimAll = document.getElementById('claimAllBtn');
        if (claimAll) {
            claimAll.disabled = !canSign;
        }

        // --- Rewards: Source panels wallet-gated (F13.5) ---
        const rewardsSources = document.querySelector('.rewards-sources');
        if (rewardsSources) rewardsSources.classList.toggle('wallet-gated-disabled', !connected);
        const tierPanel = document.querySelector('.tier-your-progress');
        if (tierPanel) tierPanel.classList.toggle('wallet-gated-disabled', !connected);

        // --- Rewards: Per-source Claim buttons (F15.9) ---
        document.querySelectorAll('.rewards-sources .claim-btn').forEach(btn => {
            btn.disabled = !canSign;
        });

        // --- Pool: Per-row Add buttons (F15.7) ---
        document.querySelectorAll('.pool-add-btn').forEach(btn => {
            btn.disabled = !canSign;
            btn.classList.toggle('btn-wallet-gate', !canSign);
        });

        // --- Governance: Vote buttons (F15.10) ---
        document.querySelectorAll('.vote-btn').forEach(btn => {
            btn.disabled = !canSign;
            btn.classList.toggle('btn-wallet-gate', !canSign);
        });

        // --- Prediction: Card action buttons (dynamically rendered) ---
        document.querySelectorAll('.btn-predict-buy, .btn-predict-buy-no').forEach(btn => {
            btn.disabled = !canSign;
            btn.classList.toggle('btn-wallet-gate', !canSign);
        });
        document.querySelectorAll('.btn-predict-resolve, .btn-predict-claim, .btn-predict-claim-pos').forEach(btn => {
            btn.disabled = !canSign;
            btn.classList.toggle('btn-wallet-gate', !canSign);
        });

        // --- Trade: Margin close & cancel order buttons ---
        document.querySelectorAll('.margin-close-btn, .cancel-btn').forEach(btn => {
            btn.disabled = !connected;
            btn.classList.toggle('btn-wallet-gate', !connected);
        });

        // --- Launchpad: Buy/Sell/Create buttons ---
        document.querySelectorAll('.launch-quick-buy, .launch-quick-sell').forEach(btn => {
            btn.disabled = !connected;
            btn.classList.toggle('btn-wallet-gate', !connected);
        });
        const launchTradeGate = document.getElementById('launchTradeBtn');
        if (launchTradeGate) {
            if (canSign) {
                launchTradeGate.disabled = false;
                launchTradeGate.classList.remove('btn-wallet-gate');
            } else {
                launchTradeGate.disabled = true;
                launchTradeGate.className = 'btn btn-full btn-wallet-gate';
                launchTradeGate.innerHTML = connected
                    ? '<i class="fas fa-key"></i> Unlock Wallet to Sign'
                    : '<i class="fas fa-wallet"></i> Connect Wallet to Trade';
            }
        }
        const launchCreateGate = document.getElementById('launchCreateBtn');
        if (launchCreateGate) {
            if (canSign) {
                launchCreateGate.disabled = false;
                launchCreateGate.classList.remove('btn-wallet-gate');
                launchCreateGate.innerHTML = '<i class="fas fa-rocket"></i> Launch Token (10 LICN)';
            } else {
                launchCreateGate.disabled = true;
                launchCreateGate.className = 'btn btn-full btn-wallet-gate';
                launchCreateGate.innerHTML = connected
                    ? '<i class="fas fa-key"></i> Unlock Wallet to Sign'
                    : '<i class="fas fa-wallet"></i> Connect Wallet to Launch';
            }
        }
        const launchTradeForm = document.getElementById('launchTradeForm');
        if (launchTradeForm) launchTradeForm.classList.toggle('wallet-gated-disabled', !connected);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // F10E.7 — Oracle Fast-Poll Price Overlay
    // FE-06 FIX: Replaced direct Binance WebSocket with internal oracle
    // polling. All price data now flows through the Lichen oracle API
    // (/api/v1/oracle/prices) — no external third-party connections.
    // When ENABLE_EXTERNAL_PRICE_WS is true, poll rate increases from 5s
    // to 2s for more responsive real-time price overlay on pair list and
    // TradingView chart. Primary prices still come from on-chain DEX data.
    // ═══════════════════════════════════════════════════════════════════════
    let oracleFastPollTimer = null;

    function startOracleFastPoll() {
        if (oracleFastPollTimer) return; // already running
        console.info('[DEX] Oracle fast-poll overlay enabled (10s fallback interval)');
        oracleFastPollTimer = setInterval(async () => {
            try {
                const resp = await fetch(`${API_BASE}/oracle/prices`);
                if (!resp.ok) return;
                const json = await resp.json();
                const feeds = json.data?.feeds || json.feeds;
                if (!feeds) return;
                for (const feed of feeds) {
                    if (feed.price > 0 && !feed.stale) {
                        oracleRefPrices[feed.asset] = feed.price;
                    }
                }
                applyOracleRealTimeOverlay();
            } catch { /* network error — skip */ }
        }, 10000);
    }

    function applyOracleRealTimeOverlay(skipRender) {
        // Update ALL oracle-priced pairs in the dropdown + active pair ticker
        const licnPairRef = pairs.find(p => (p.base || '').toUpperCase() === 'LICN' && (p.quote || '').toUpperCase() === 'LUSD');
        const licnUsd = licnPairRef?.price || LICHEN_GENESIS_PRICE;
        let dropdownChanged = false;

        for (const p of pairs) {
            const base = (p.base || '').toUpperCase();
            const quote = (p.quote || '').toUpperCase();
            let extPrice = 0;
            if ((base === 'WSOL' || base === 'SOL') && oracleRefPrices['wSOL'] > 0) extPrice = oracleRefPrices['wSOL'];
            else if ((base === 'WETH' || base === 'ETH') && oracleRefPrices['wETH'] > 0) extPrice = oracleRefPrices['wETH'];
            else if ((base === 'WBNB' || base === 'BNB') && oracleRefPrices['wBNB'] > 0) extPrice = oracleRefPrices['wBNB'];

            // Safety guard: display-inverted pairs should NEVER show price > 1
            // (LICN/wSOL ≈ 0.001, LICN/wETH ≈ 0.00005, LICN/wBNB ≈ 0.00016)
            // If price > 1 it means the raw on-chain LICN-denominated price leaked through
            if (!p.hasMarketPrice && extPrice <= 0 && isDisplayInvertedPair(p) && p.price > 1) {
                p.price = invertPrice(p.price);
                dropdownChanged = true;
                if (p.pairId === state.activePairId) { state.lastPrice = p.price; updateTickerDisplay(); }
                continue;
            }
            if (p.hasMarketPrice) continue;
            if (extPrice <= 0) continue;

            // For LICN-quoted pairs (on-chain: wSOL/LICN, display: LICN/wSOL),
            // the display price = LICN_USD / asset_USD (how much asset per 1 LICN)
            if (quote === 'LICN' && licnUsd > 0) extPrice = licnUsd / extPrice;
            else if (quote !== 'LUSD' && quote !== 'USD') continue;

            p.price = extPrice;
            dropdownChanged = true;

            // Also update active pair's ticker display
            if (p.pairId === state.activePairId) {
                state.lastPrice = extPrice;
                updateTickerDisplay();
                streamBarUpdate(extPrice, 0);
            }
        }
        if (dropdownChanged && !skipRender) renderPairList();
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Oracle Price Polling — fetches /api/v1/oracle/prices every 5s,
    // updates dropdown prices and chart streaming bars.
    // ═══════════════════════════════════════════════════════════════════════
    let oracleRefPrices = {};

    async function fetchOracleRefPrices() {
        try {
            const resp = await fetch(`${API_BASE}/oracle/prices`);
            if (!resp.ok) return;
            const json = await resp.json();
            const feeds = json.data?.feeds || json.feeds;
            if (feeds) {
                for (const feed of feeds) {
                    if (feed.price > 0 && !feed.stale) {
                        oracleRefPrices[feed.asset] = feed.price;
                    }
                }
            }
            applyOracleRealTimeOverlay();
        } catch { /* network error — skip */ }
    }

    // Poll oracle prices every 30s as WS fallback (primary updates via WS ticker)
    setInterval(fetchOracleRefPrices, 30000);
    // Initial fetch after short delay
    setTimeout(fetchOracleRefPrices, 2000);

    // ═══════════════════════════════════════════════════════════════════════
    // Margin — Inline in Trade View (no standalone Margin tab)
    // ═══════════════════════════════════════════════════════════════════════
    // Margin enabled-pairs cache
    let marginEnabledPairIds = [];
    async function loadMarginEnabledPairs() {
        try {
            const { data } = await api.get('/margin/enabled-pairs');
            if (data && Array.isArray(data.enabledPairIds)) marginEnabledPairIds = data.enabledPairIds;
        } catch { /* keep empty */ }
        syncMarginAvailabilityUi();
        if (state.tradeMode === 'margin' && !isMarginEnabledForActivePair()) {
            setTradeMode('spot');
        }
    }
    function checkMarginPairEnabled() {
        syncMarginAvailabilityUi();
    }

    function syncMarginAvailabilityUi() {
        const enabled = isMarginEnabledForActivePair();
        const warn = document.getElementById('marginPairWarning');
        if (warn) warn.classList.toggle('hidden', enabled || state.tradeMode !== 'margin');

        const marginModeBtn = document.querySelector('.trade-mode[data-mode="margin"]');
        if (marginModeBtn) {
            marginModeBtn.disabled = !enabled;
            marginModeBtn.classList.toggle('mode-disabled', !enabled);
            marginModeBtn.title = enabled ? '' : 'Margin unavailable for this pair';
        }

        document.querySelectorAll('.margin-inline-type').forEach(btn => {
            btn.disabled = !enabled;
            btn.classList.toggle('mode-disabled', !enabled);
        });
    }

    // F10.7 FIX: Maintenance margin BPS lookup matching contract tier table
    function getMaintenanceBps(leverage) {
        if (leverage <= 2) return 2500;   // 25%
        if (leverage <= 3) return 1700;   // 17%
        if (leverage <= 5) return 1000;   // 10%
        if (leverage <= 10) return 500;   //  5%
        if (leverage <= 25) return 200;   //  2%
        if (leverage <= 50) return 100;   //  1%
        return 50;                        // 0.5%
    }

    function updateMarginInfo() {
        const e = document.getElementById('marginEntry'), l = document.getElementById('marginLiqPrice'), r = document.getElementById('marginRatio');
        const manualPrice = parseFloat(priceInput?.value || '0') || 0;
        const referencePrice = manualPrice > 0 ? manualPrice : state.lastPrice;
        const amount = Math.max(0, parseFloat(amountInput?.value || '0') || 0);
        const notional = Math.max(0, referencePrice * amount);
        if (e) e.textContent = formatPrice(referencePrice);
        // F10.7 FIX: Liquidation price uses tier-appropriate maintenance BPS
        // Liq occurs when margin_ratio drops to maintenance level
        // For long: liq_price = entry * (1 - (margin/notional - maintBps/10000))
        //         = entry * (1 - 1/leverage + maintBps/10000) — simplified
        // For short: liq_price = entry * (1 + 1/leverage - maintBps/10000)
        const maintBps = getMaintenanceBps(state.leverageValue);
        const maintFrac = maintBps / 10000;
        const isolatedMargin = state.leverageValue > 0 ? (notional / state.leverageValue) : 0;
        const effectiveMargin = state.marginType === 'cross' ? getQuoteAvailableMargin() : isolatedMargin;
        const effectiveLeverage = effectiveMargin > 0 && notional > 0
            ? Math.max(1, notional / effectiveMargin)
            : Math.max(1, state.leverageValue);
        if (l) l.textContent = formatPrice(state.marginSide === 'long'
            ? referencePrice * (1 - 1 / effectiveLeverage + maintFrac)
            : referencePrice * (1 + 1 / effectiveLeverage - maintFrac));
        if (r) r.textContent = `${(100 / effectiveLeverage).toFixed(1)}%`;
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Pool View — Load data from API
    // ═══════════════════════════════════════════════════════════════════════
    async function loadPoolStats() {
        try {
            const { data } = await api.get('/stats/amm');
            if (data) {
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('poolTvl', formatVolume(data.tvl || data.totalVolume || 0));
                el('poolVolume24h', formatVolume(data.volume_24h || 0));
                el('poolFees24h', formatVolume(data.fees24h || data.totalFees || 0));
                el('poolCount', data.poolCount ?? '—');
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
                        const pair = `${escapeHtml(p.tokenASymbol || 'Token A')}/${escapeHtml(p.tokenBSymbol || 'Token B')}`;
                        const feeBps = parseInt(p.feeTier) || 30;
                        const fee = (feeBps / 100).toFixed(2) + '%';
                        const tvl = p.tvl ? formatVolume(p.tvl) : formatAmount((p.liquidity || 0) / 1e9) + ' LP';
                        const vol = p.totalVolume ? formatVolume(p.totalVolume) : '—';
                        const apr = p.apr ? p.apr.toFixed(1) + '%' : '—';
                        return `<tr class="pool-row" data-pool-id="${p.poolId || p.id || 0}">
                            <td class="pool-pair">${pair}</td>
                            <td><span class="fee-badge">${fee}</span></td>
                            <td class="mono-value">${tvl}</td>
                            <td class="mono-value">${vol}</td>
                            <td class="apr-value">${apr}</td>
                            <td><button class="btn btn-small btn-secondary pool-add-btn${!state.connected ? ' btn-wallet-gate' : ''}" data-pool-id="${p.poolId || p.id || 0}"${!state.connected ? ' disabled' : ''}>Add</button></td>
                        </tr>`;
                    }).join('');
                }
                // F7.17: Populate liqPoolSelect from actual pools instead of CLOB pairs
                const poolSelect = document.getElementById('liqPoolSelect');
                if (poolSelect) {
                    poolSelect.innerHTML = data.map(p => {
                        const label = `${p.tokenASymbol || 'A'}/${p.tokenBSymbol || 'B'}`;
                        return `<option value="${p.poolId || p.id || 0}">${escapeHtml(label)}</option>`;
                    }).join('') || '<option>No pools available</option>';
                }
                // F7.18: Store pools for price lookup on select change
                state.poolsCache = data;
                function poolSelectHandler() {
                    const sel = document.getElementById('liqPoolSelect');
                    const pool = state.poolsCache?.find(p => String(p.poolId || p.id) === sel?.value);
                    const priceEl = document.getElementById('liqCurrentPrice');
                    if (pool && pool.sqrtPrice && priceEl) {
                        const price = sqrtPriceQ32ToPrice(pool.sqrtPrice);
                        priceEl.textContent = price >= 0.01 ? price.toFixed(6) : price.toExponential(4);
                    } else if (priceEl) { priceEl.textContent = '—'; }
                }
                const liqSel = document.getElementById('liqPoolSelect');
                if (liqSel) { liqSel.onchange = poolSelectHandler; poolSelectHandler(); }
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
            const { data } = await api.get(`/pools/positions?owner=${wallet.address}`);
            if (Array.isArray(data) && data.length > 0) {
                if (container) {
                    container.innerHTML = data.map(pos => `
                        <div class="lp-position-card" data-position-id="${pos.positionId || 0}" data-pool-id="${pos.poolId || 0}">
                            <div class="lp-pos-header">
                                <div class="lp-pos-pair">
                                    <span class="lp-pair-name">${escapeHtml(pos.pair || 'Pool #' + (pos.poolId || '?'))}</span>
                                    <span class="fee-badge">LP</span>
                                </div>
                                <span class="range-badge in-range"><i class="fas fa-circle"></i> Active</span>
                            </div>
                            <div class="lp-pos-details">
                                <div class="lp-detail"><span>Tick Range</span><span class="mono-value">${pos.lowerTick ?? 0} — ${pos.upperTick ?? 0}</span></div>
                                <div class="lp-detail"><span>Liquidity</span><span class="mono-value" data-raw-liquidity="${pos.liquidity || 0}">${formatAmount((pos.liquidity || 0) / 1e9)}</span></div>
                                <div class="lp-detail"><span>Uncollected Fees</span><span class="mono-value accent-text">${formatVolume(((pos.feeAOwed || 0) + (pos.feeBOwed || 0)) / 1e9)}</span></div>
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

    // F8.10/F8.11/F8.12: LP position action handlers via event delegation
    const poolPositionsContainer = document.getElementById('pool-positions');
    if (poolPositionsContainer) poolPositionsContainer.addEventListener('click', async (e) => {
        const collectBtn = e.target.closest('.lp-collect-btn');
        const removeBtn = e.target.closest('.lp-remove-btn');
        const addBtn = e.target.closest('.lp-add-btn');
        if (!collectBtn && !removeBtn && !addBtn) return;
        e.stopPropagation();
        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
        if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }

        if (collectBtn) {
            const posId = parseInt(collectBtn.dataset.positionId) || 0;
            collectBtn.disabled = true; const origText = collectBtn.innerHTML; collectBtn.textContent = 'Collecting...';
            try {
                await wallet.sendTransaction([contractIx(contracts.dex_amm, buildCollectFeesArgs(wallet.address, posId))]);
                showNotification('Fees collected successfully!', 'success');
                await loadLPPositions();
            } catch (err) { showNotification(`Collect failed: ${err.message}`, 'error'); }
            collectBtn.disabled = false; collectBtn.innerHTML = origText;
        }

        if (removeBtn) {
            const posId = parseInt(removeBtn.dataset.positionId) || 0;
            const card = removeBtn.closest('.lp-position-card');
            // F24.2 FIX: Read raw liquidity from data attribute instead of parsing display text
            const rawLiqEl = card?.querySelector('[data-raw-liquidity]');
            const rawLiq = parseInt(rawLiqEl?.dataset?.rawLiquidity) || 0;
            if (!confirm(`Remove all liquidity from position #${posId}? This cannot be undone.`)) return;
            removeBtn.disabled = true; const origText = removeBtn.innerHTML; removeBtn.textContent = 'Removing...';
            try {
                await wallet.sendTransaction([contractIx(contracts.dex_amm, buildRemoveLiquidityArgs(wallet.address, posId, rawLiq))]);
                showNotification('Liquidity removed successfully!', 'success');
                await loadLPPositions();
            } catch (err) { showNotification(`Remove failed: ${err.message}`, 'error'); }
            removeBtn.disabled = false; removeBtn.innerHTML = origText;
        }

        if (addBtn) {
            const posId = parseInt(addBtn.dataset.positionId) || 0;
            const card = addBtn.closest('.lp-position-card');
            const poolId = parseInt(card?.dataset?.poolId) || 0;
            // Scroll to add liquidity form and pre-select the pool
            const poolSelect = document.getElementById('liqPoolSelect');
            if (poolSelect) {
                poolSelect.value = poolId;
                poolSelect.dispatchEvent(new Event('change'));
            }
            const addLiqSection = document.getElementById('addLiqBtn')?.closest('.pool-add-section') || document.getElementById('addLiqBtn')?.parentElement;
            if (addLiqSection) addLiqSection.scrollIntoView({ behavior: 'smooth', block: 'center' });
            showNotification(`Add more liquidity to pool #${poolId} — fill in amounts below`, 'info');
        }
    });

    // Add Liquidity submit handler
    const addLiqBtn = document.getElementById('addLiqBtn');
    if (addLiqBtn) addLiqBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
        const amtA = parseFloat(document.getElementById('liqAmountA')?.value) || 0;
        const amtB = parseFloat(document.getElementById('liqAmountB')?.value) || 0;
        if (!amtA && !amtB) { showNotification('Enter deposit amounts', 'warning'); return; }
        if (amtA < 0 || amtB < 0) { showNotification('Amounts must be positive', 'warning'); return; }
        if (amtA > 9_000_000 || amtB > 9_000_000) { showNotification('Amount too large (max 9M)', 'warning'); return; }
        const minPrice = parseFloat(document.getElementById('liqMinPrice')?.value) || 0;
        const maxPrice = parseFloat(document.getElementById('liqMaxPrice')?.value) || 0;
        const fullRange = document.getElementById('fullRangeToggle')?.checked;
        addLiqBtn.disabled = true; addLiqBtn.textContent = 'Adding...';
        try {
            const poolSelect = document.getElementById('liqPoolSelect');
            const poolId = poolSelect ? parseInt(poolSelect.value) || 0 : 0;
            // F8.3: Convert price to ticks using log(price)/log(1.0001) formula
            const spacing = FEE_TIER_SPACING[state.selectedFeeTier] || 60;
            const lt = fullRange ? MIN_TICK : alignTickToSpacing(priceToTick(minPrice), spacing);
            const ut = fullRange ? MAX_TICK : alignTickToSpacing(priceToTick(maxPrice), spacing);
            // AUDIT-FIX F10.10: Use real contract address from symbol registry (not hardcoded hex placeholder)
            await wallet.sendTransaction([contractIx(
                contracts.dex_amm,
                buildAddLiquidityArgs(wallet.address, poolId, lt, ut, Math.round(amtA * 1e9), Math.round(amtB * 1e9))
            )]);
            showNotification(`Liquidity added: ${formatAmount(amtA)} + ${formatAmount(amtB)}`, 'success');
            // F24.10 FIX: Refresh LP positions and pools after adding liquidity
            loadLPPositions().catch(() => { }); loadPools().catch(() => { });
        } catch (e) { showNotification(`Add liquidity: ${e.message}`, 'error'); }
        finally { addLiqBtn.disabled = false; addLiqBtn.textContent = 'Add Liquidity'; }
    });

    // Fee tier selector — F7.20: store selected value in state
    state.selectedFeeTier = 30; // default 30bps
    document.querySelectorAll('.fee-tier-btn').forEach(btn => btn.addEventListener('click', () => {
        document.querySelectorAll('.fee-tier-btn').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        state.selectedFeeTier = parseInt(btn.dataset.fee) || 30;
    }));

    // Pool filter pills
    document.querySelectorAll('.pool-table-panel .filter-pill').forEach(btn => btn.addEventListener('click', () => {
        document.querySelectorAll('.pool-table-panel .filter-pill').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        // F10E.11: "My Pools" filter — show only rows for pools where user has LP positions
        const filter = btn.dataset.filter;
        if (filter === 'my' && !state.connected) {
            showNotification('Connect wallet to view your pools', 'warning');
            return;
        }
        const rows = document.querySelectorAll('.pool-table tbody .pool-row');
        if (filter === 'all') {
            rows.forEach(r => r.style.display = '');
        } else {
            // Get pool IDs from LP positions
            const myPools = document.querySelectorAll('#pool-positions .lp-position-card');
            const myPoolIds = new Set();
            myPools.forEach(card => { const pid = card.dataset.poolId; if (pid) myPoolIds.add(pid); });
            let visibleCount = 0;
            rows.forEach(r => {
                const poolId = r.dataset.poolId;
                const show = myPoolIds.has(poolId);
                r.style.display = show ? '' : 'none';
                if (show) visibleCount++;
            });
            if (!visibleCount) showNotification('No liquidity positions found', 'info');
        }
    }));

    // F7.9: Pool row / Add button click delegation — select pool in Add Liquidity form
    document.getElementById('poolTableBody')?.addEventListener('click', (e) => {
        const btn = e.target.closest('.pool-add-btn');
        const row = e.target.closest('.pool-row');
        if (btn || row) {
            const poolId = (btn || row).dataset.poolId;
            const poolSelect = document.getElementById('liqPoolSelect');
            if (poolSelect) {
                poolSelect.value = poolId;
                poolSelect.dispatchEvent(new Event('change'));
            }
            document.getElementById('addLiqForm')?.scrollIntoView({ behavior: 'smooth' });
        }
    });

    // F7.19: Pool share estimate — update on amount input
    ['liqAmountA', 'liqAmountB'].forEach(id => {
        document.getElementById(id)?.addEventListener('input', () => {
            const shareEl = document.getElementById('liqPoolShare');
            if (!shareEl) return;
            const sel = document.getElementById('liqPoolSelect');
            const pool = state.poolsCache?.find(p => String(p.poolId || p.id) === sel?.value);
            if (!pool || !pool.liquidity) { shareEl.textContent = '—'; return; }
            const amtA = parseFloat(document.getElementById('liqAmountA')?.value) || 0;
            const amtB = parseFloat(document.getElementById('liqAmountB')?.value) || 0;
            // F24.17 FIX: Weight deposit by pool price ratio for accurate share estimate
            const poolPrice = pool.sqrtPrice ? sqrtPriceQ32ToPrice(pool.sqrtPrice) : 1;
            const deposit = (amtA * poolPrice + amtB) * 1e9; // scale to match liquidity units
            const share = deposit / (pool.liquidity + deposit) * 100;
            shareEl.textContent = share >= 0.01 ? share.toFixed(2) + '%' : '< 0.01%';
        });
    });

    // ═══════════════════════════════════════════════════════════════════════
    // Margin — Open/Close Positions + Load from API
    // ═══════════════════════════════════════════════════════════════════════

    // Leverage tier table — mirrors contract get_tier_params()
    // Returns { initialBps, maintenanceBps, liquidationPenaltyBps }
    function getMarginTierParams(leverage) {
        if (leverage <= 2) return { initialBps: 5000, maintenanceBps: 2500, liquidationPenaltyBps: 300 };
        if (leverage <= 3) return { initialBps: 3333, maintenanceBps: 1700, liquidationPenaltyBps: 300 };
        if (leverage <= 5) return { initialBps: 2000, maintenanceBps: 1000, liquidationPenaltyBps: 500 };
        if (leverage <= 10) return { initialBps: 1000, maintenanceBps: 500, liquidationPenaltyBps: 500 };
        if (leverage <= 25) return { initialBps: 400, maintenanceBps: 200, liquidationPenaltyBps: 700 };
        if (leverage <= 50) return { initialBps: 200, maintenanceBps: 100, liquidationPenaltyBps: 1000 };
        return { initialBps: 100, maintenanceBps: 50, liquidationPenaltyBps: 1500 }; // ≤100x
    }

    // Compute liquidation price for a margin position
    // Long:  liqPrice = entryPrice * (1 - margin / (size * entryPrice) + maintenanceBps / 10000)
    // Short: liqPrice = entryPrice * (1 + margin / (size * entryPrice) - maintenanceBps / 10000)
    function computeLiquidationPrice(side, entryPrice, margin, size, leverage) {
        if (!entryPrice || !size || !margin) return 0;
        const { maintenanceBps } = getMarginTierParams(leverage);
        const marginRatio = margin / (size * entryPrice);
        const maintRate = maintenanceBps / 10000;
        if (side === 'Long') {
            return entryPrice * (1 - marginRatio + maintRate);
        } else {
            return entryPrice * (1 + marginRatio - maintRate);
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // PnL SHARE CARD — Canvas-based branded position card
    // ═══════════════════════════════════════════════════════════════════════════

    function generatePnlShareCard(opts) {
        const { pair, side, entry, mark, pnl, pnlPct, leverage, createdSlot } = opts;
        const isProfit = pnl >= 0;
        const W = 480, H = 280;
        const canvas = document.createElement('canvas');
        canvas.width = W; canvas.height = H;
        const ctx = canvas.getContext('2d');

        // Background gradient — green for profit, red for loss
        const grad = ctx.createLinearGradient(0, 0, W, H);
        if (isProfit) {
            grad.addColorStop(0, '#0a2e1a');
            grad.addColorStop(1, '#134e2a');
        } else {
            grad.addColorStop(0, '#2e0a0a');
            grad.addColorStop(1, '#4e1313');
        }
        ctx.fillStyle = grad;
        ctx.beginPath();
        ctx.roundRect(0, 0, W, H, 12);
        ctx.fill();

        // Border glow
        ctx.strokeStyle = isProfit ? 'rgba(0,255,127,0.3)' : 'rgba(255,80,80,0.3)';
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.roundRect(1, 1, W - 2, H - 2, 12);
        ctx.stroke();

        // Header: Lichen DEX branding
        ctx.fillStyle = '#ffffff';
        ctx.font = 'bold 14px monospace';
        ctx.fillText('Lichen DEX', 20, 30);
        ctx.fillStyle = 'rgba(255,255,255,0.5)';
        ctx.font = '11px monospace';
        ctx.fillText('Margin Trade', 160, 30);

        // Pair + Side + Leverage
        ctx.fillStyle = isProfit ? '#00ff7f' : '#ff5050';
        ctx.font = 'bold 22px monospace';
        ctx.fillText(`${side.toUpperCase()} ${pair}`, 20, 65);
        ctx.fillStyle = '#ffffff';
        ctx.font = 'bold 16px monospace';
        ctx.fillText(`${leverage}x`, 20, 88);

        // Divider
        ctx.strokeStyle = 'rgba(255,255,255,0.15)';
        ctx.lineWidth = 1;
        ctx.beginPath(); ctx.moveTo(20, 100); ctx.lineTo(W - 20, 100); ctx.stroke();

        // Stats grid — two columns
        const leftX = 20, rightX = 260, rowH = 28;
        let y = 125;
        ctx.font = '12px monospace';

        const drawStat = (x, yy, label, value, color) => {
            ctx.fillStyle = 'rgba(255,255,255,0.6)';
            ctx.fillText(label, x, yy);
            ctx.fillStyle = color || '#ffffff';
            ctx.fillText(value, x + 80, yy);
        };

        drawStat(leftX, y, 'Entry:', formatPrice(entry));
        drawStat(rightX, y, 'Mark:', formatPrice(mark));
        y += rowH;
        drawStat(leftX, y, 'PnL $:', `${pnl >= 0 ? '+' : ''}${formatPrice(pnl)}`, isProfit ? '#00ff7f' : '#ff5050');
        drawStat(rightX, y, 'PnL %:', pnlPct, isProfit ? '#00ff7f' : '#ff5050');
        y += rowH;
        drawStat(leftX, y, 'Leverage:', `${leverage}x`);
        if (createdSlot > 0) {
            drawStat(rightX, y, 'Slot:', String(createdSlot));
        }

        // Big PnL display
        y += 38;
        ctx.fillStyle = isProfit ? '#00ff7f' : '#ff5050';
        ctx.font = 'bold 32px monospace';
        const bigPnl = `${pnl >= 0 ? '+' : ''}${formatPrice(pnl)}`;
        ctx.fillText(bigPnl, 20, y);

        // Footer
        ctx.fillStyle = 'rgba(255,255,255,0.3)';
        ctx.font = '10px monospace';
        ctx.fillText(`lichen.network • ${new Date().toISOString().slice(0, 10)}`, 20, H - 12);

        return canvas;
    }

    function showPnlShareCard(opts) {
        // Remove existing modal if any
        const existing = document.getElementById('pnlShareModal');
        if (existing) existing.remove();

        const canvas = generatePnlShareCard(opts);

        const modal = document.createElement('div');
        modal.id = 'pnlShareModal';
        modal.style.cssText = 'position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,0.7);display:flex;align-items:center;justify-content:center;z-index:10000;';

        const card = document.createElement('div');
        card.style.cssText = 'background:var(--bg-primary,#1a1a2e);padding:16px;border-radius:12px;display:flex;flex-direction:column;gap:12px;align-items:center;';

        card.appendChild(canvas);

        const btnRow = document.createElement('div');
        btnRow.style.cssText = 'display:flex;gap:10px;';

        const copyBtn = document.createElement('button');
        copyBtn.className = 'btn btn-primary btn-small';
        copyBtn.textContent = '📋 Copy Image';
        copyBtn.addEventListener('click', async () => {
            try {
                const blob = await new Promise(r => canvas.toBlob(r, 'image/png'));
                await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]);
                showNotification('PnL card copied to clipboard', 'success');
            } catch {
                showNotification('Copy failed — try Download instead', 'warning');
            }
        });

        const dlBtn = document.createElement('button');
        dlBtn.className = 'btn btn-secondary btn-small';
        dlBtn.textContent = '⬇ Download PNG';
        dlBtn.addEventListener('click', () => {
            const a = document.createElement('a');
            a.download = `lichen-pnl-${opts.pair.replace('/', '-')}-${Date.now()}.png`;
            a.href = canvas.toDataURL('image/png');
            a.click();
        });

        const closeBtn = document.createElement('button');
        closeBtn.className = 'btn btn-secondary btn-small';
        closeBtn.textContent = '✕ Close';
        closeBtn.addEventListener('click', () => modal.remove());

        btnRow.appendChild(copyBtn);
        btnRow.appendChild(dlBtn);
        btnRow.appendChild(closeBtn);
        card.appendChild(btnRow);
        modal.appendChild(card);

        modal.addEventListener('click', e => { if (e.target === modal) modal.remove(); });
        document.body.appendChild(modal);
    }

    async function loadMarginStats() {
        try {
            const { data } = await api.get('/stats/margin');
            if (data) {
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('marginInsurance', formatVolume((data.insuranceFund || 0) / 1e9));
                const maxLev = Number(data.maxLeverage ?? data.max_leverage ?? 0);
                if (maxLev > 0) state.marginMaxLeverage = Math.min(100, maxLev);
            }
        } catch { /* API unavailable */ }
        // Load margin info
        try {
            const { data } = await api.get('/margin/info');
            if (data) {
                const maxLev = Number(data.maxLeverage ?? data.max_leverage ?? 0);
                if (maxLev > 0) state.marginMaxLeverage = Math.min(100, maxLev);
            }
        } catch { /* keep defaults */ }
        applyLeverageConstraints();
        updateMarginInfo();
        // Load funding rate
        try {
            const { data } = await api.get('/margin/funding-rate');
            if (data) {
                const el = document.getElementById('marginFundingRate');
                if (el) {
                    const rate = (data.baseRateBps / 100).toFixed(2);
                    el.textContent = `${rate}%/${data.intervalHours || 8}h`;
                    el.title = `Base rate: ${rate}%/8h, Max: ${(data.maxRateBps / 100).toFixed(1)}%, Tiers: ${(data.tiers || []).length}`;
                }
            }
        } catch { /* keep default */ }
    }

    async function submitMarginClose(positionId, fullSizeRaw, closeAmountRaw, closeType = 'market', limitPriceRaw = 0) {
        if (!state.connected || !walletCanSign()) throw new Error('Wallet not ready');
        if (!contracts.dex_margin) throw new Error('Margin contract not loaded');
        let ix;
        if (closeType === 'limit') {
            if (!limitPriceRaw || limitPriceRaw <= 0) throw new Error('Enter a valid limit close price');
            if (closeAmountRaw >= fullSizeRaw) {
                ix = contractIx(contracts.dex_margin, buildClosePositionLimitArgs(wallet.address, positionId, limitPriceRaw));
            } else {
                ix = contractIx(contracts.dex_margin, buildPartialCloseLimitArgs(wallet.address, positionId, closeAmountRaw, limitPriceRaw));
            }
        } else if (closeAmountRaw >= fullSizeRaw) {
            ix = contractIx(contracts.dex_margin, buildClosePositionArgs(wallet.address, positionId));
        } else {
            ix = contractIx(contracts.dex_margin, buildPartialCloseArgs(wallet.address, positionId, closeAmountRaw));
        }
        await wallet.sendTransaction([ix]);
        await loadMarginPositions();
        await loadMarginHistory();
        if (wallet.address) loadBalances(wallet.address).then(() => renderBalances()).catch(() => { });
    }

    async function submitMarginLiquidation(positionId) {
        if (!state.connected || !walletCanSign()) throw new Error('Wallet not ready');
        if (!contracts.dex_margin) throw new Error('Margin contract not loaded');
        if (!Number.isFinite(positionId) || positionId <= 0) throw new Error('Invalid position ID');
        await wallet.sendTransaction([contractIx(contracts.dex_margin, buildLiquidateArgs(wallet.address, positionId))]);
        await loadMarginPositions();
        await loadMarginHistory();
        if (wallet.address) loadBalances(wallet.address).then(() => renderBalances()).catch(() => { });
    }

    function showMarginCloseModal(positionId, fullSizeRaw, positionSide = 'long') {
        const overlay = document.createElement('div');
        overlay.className = 'order-confirm-overlay';
        overlay.innerHTML = `
            <div class="order-confirm-modal" style="max-width:460px;">
                <div class="order-confirm-header">
                    <h3><i class="fas fa-times-circle"></i> Close Margin Position</h3>
                    <button class="order-confirm-close" aria-label="Close">×</button>
                </div>
                <div class="order-confirm-body" style="display:flex;flex-direction:column;gap:10px;">
                    <div style="display:flex;gap:8px;">
                        <button class="btn btn-small btn-primary" data-close-type="market">Market</button>
                        <button class="btn btn-small btn-secondary" data-close-type="limit">Limit</button>
                    </div>
                    <div style="font-size:0.8rem;color:var(--text-secondary);" id="marginCloseTypeHint">Market close executes immediately at current mark price.</div>
                    <label id="marginCloseLimitPriceRow" class="hidden" style="display:flex;flex-direction:column;gap:4px;font-size:0.82rem;">
                        <span>Limit close price</span>
                        <input id="marginCloseLimitPriceInput" type="number" min="0.000000001" step="0.000000001" placeholder="e.g. 0.1250" />
                    </label>
                    <div style="display:flex;gap:6px;flex-wrap:wrap;">
                        <button class="btn btn-small btn-secondary" data-close-pct="25">25%</button>
                        <button class="btn btn-small btn-secondary" data-close-pct="50">50%</button>
                        <button class="btn btn-small btn-secondary" data-close-pct="75">75%</button>
                        <button class="btn btn-small btn-primary" data-close-pct="100">100%</button>
                    </div>
                    <label style="display:flex;flex-direction:column;gap:4px;font-size:0.82rem;">
                        <span>Custom quantity</span>
                        <input id="marginCloseQtyInput" type="number" min="0.001" step="0.001" placeholder="e.g. 0.2500" />
                    </label>
                </div>
                <div class="order-confirm-footer">
                    <button class="btn btn-small btn-secondary" data-close-action="cancel">Cancel</button>
                    <button class="btn btn-small btn-primary" data-close-action="submit">Close Position</button>
                </div>
            </div>`;
        document.body.appendChild(overlay);

        let selectedType = 'market';
        let selectedPct = 100;
        const closeTypeHint = overlay.querySelector('#marginCloseTypeHint');
        const limitPriceRow = overlay.querySelector('#marginCloseLimitPriceRow');
        const limitPriceInput = overlay.querySelector('#marginCloseLimitPriceInput');
        const qtyInput = overlay.querySelector('#marginCloseQtyInput');
        const submitBtn = overlay.querySelector('[data-close-action="submit"]');
        const closeBtn = overlay.querySelector('.order-confirm-close');
        const cancelBtn = overlay.querySelector('[data-close-action="cancel"]');

        const closeOverlay = () => {
            try { overlay.remove(); } catch { /* no-op */ }
        };

        overlay.querySelectorAll('[data-close-type]').forEach(btn => btn.addEventListener('click', () => {
            selectedType = btn.dataset.closeType;
            overlay.querySelectorAll('[data-close-type]').forEach(b => b.className = `btn btn-small ${b.dataset.closeType === selectedType ? 'btn-primary' : 'btn-secondary'}`);
            if (selectedType === 'limit') {
                closeTypeHint.textContent = `Limit close executes when mark ${positionSide === 'short' ? '≤' : '≥'} your limit price.`;
                if (limitPriceRow) limitPriceRow.classList.remove('hidden');
                submitBtn.disabled = false;
                submitBtn.title = '';
            } else {
                closeTypeHint.textContent = 'Market close executes immediately at current mark price.';
                if (limitPriceRow) limitPriceRow.classList.add('hidden');
                submitBtn.disabled = false;
                submitBtn.title = '';
            }
        }));

        overlay.querySelectorAll('[data-close-pct]').forEach(btn => btn.addEventListener('click', () => {
            selectedPct = Math.max(1, Math.min(100, parseInt(btn.dataset.closePct || '100')));
            overlay.querySelectorAll('[data-close-pct]').forEach(b => b.className = `btn btn-small ${parseInt(b.dataset.closePct || '0') === selectedPct ? 'btn-primary' : 'btn-secondary'}`);
            if (qtyInput) qtyInput.value = '';
        }));

        if (qtyInput) qtyInput.addEventListener('input', () => {
            const customQty = parseFloat(qtyInput.value || '0');
            if (customQty > 0) {
                overlay.querySelectorAll('[data-close-pct]').forEach(b => b.className = 'btn btn-small btn-secondary');
            }
        });

        const submit = async () => {
            const customQty = parseFloat(qtyInput?.value || '0');
            let closeAmountRaw;
            if (customQty > 0) {
                closeAmountRaw = Math.floor(customQty * PRICE_SCALE);
            } else {
                closeAmountRaw = Math.floor(fullSizeRaw * selectedPct / 100);
            }
            if (!closeAmountRaw || closeAmountRaw <= 0) {
                showNotification('Close amount too small', 'warning');
                return;
            }
            closeAmountRaw = Math.min(closeAmountRaw, fullSizeRaw);

            let limitPriceRaw = 0;
            if (selectedType === 'limit') {
                const limitPriceHuman = parseFloat(limitPriceInput?.value || '0');
                if (!limitPriceHuman || limitPriceHuman <= 0) {
                    showNotification('Enter a valid limit close price', 'warning');
                    return;
                }
                limitPriceRaw = Math.round(limitPriceHuman * PRICE_SCALE);
            }

            submitBtn.disabled = true;
            try {
                await submitMarginClose(positionId, fullSizeRaw, closeAmountRaw, selectedType, limitPriceRaw);
                showNotification(closeAmountRaw >= fullSizeRaw ? 'Position closed' : 'Position partially closed', 'success');
                closeOverlay();
            } catch (e) {
                showNotification(`Close failed: ${e.message}`, 'error');
            }
            submitBtn.disabled = false;
        };

        closeBtn?.addEventListener('click', closeOverlay);
        cancelBtn?.addEventListener('click', closeOverlay);
        submitBtn?.addEventListener('click', submit);
        overlay.addEventListener('click', (e) => { if (e.target === overlay) closeOverlay(); });
    }

    async function loadMarginPositions() {
        const badge = document.querySelector('.margin-badge');
        const container = document.getElementById('marginPositionsList');
        if (!state.connected) {
            const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
            el('marginEquity', '—'); el('marginUsed', '—'); el('marginAvailable', '—');
            if (badge) badge.textContent = '';
            if (container) container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:30px;font-size:0.85rem;"><i class="fas fa-wallet" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>Connect wallet to view positions</div>';
            return;
        }
        try {
            const { data } = await api.get(`/margin/positions?trader=${wallet.address}`);
            if (badge) badge.textContent = Array.isArray(data) && data.length > 0 ? data.length : '';
            if (Array.isArray(data) && data.length > 0) {
                if (container) {
                    container.className = 'margin-positions-list';
                    container.innerHTML = data.map(pos => {
                        const side = pos.side === 'long' ? 'Long' : 'Short';
                        const sideClass = side === 'Long' ? 'side-buy' : 'side-sell';
                        const marginType = pos.marginType === 'cross' ? 'Cross' : 'Isolated';
                        const leverage = pos.leverage || state.leverageValue || 2;
                        // Unrealized PnL computation
                        const mark = pos.markPrice || state.lastPrice;
                        const entry = pos.entryPrice || 0;
                        const sizeRaw = pos.size || 0;
                        const marginRaw = pos.margin || 0;
                        let pnl;
                        if (pos.status === 'closed' || pos.status === 'liquidated') {
                            pnl = pos.realizedPnl || 0;
                        } else if (entry > 0 && mark > 0) {
                            pnl = side === 'Long'
                                ? (mark - entry) * sizeRaw / PRICE_SCALE
                                : (entry - mark) * sizeRaw / PRICE_SCALE;
                        } else {
                            pnl = 0;
                        }
                        // PnL % relative to margin deposited
                        const marginHuman = marginRaw / PRICE_SCALE;
                        const pnlPct = marginHuman > 0 ? (pnl / marginHuman) * 100 : 0;
                        const pnlPctStr = `${pnlPct >= 0 ? '+' : ''}${pnlPct.toFixed(2)}%`;
                        // Liquidation price
                        const entryHuman = entry / PRICE_SCALE || entry;
                        const sizeHuman = sizeRaw / PRICE_SCALE || sizeRaw;
                        const liqPrice = computeLiquidationPrice(side, entryHuman, marginHuman, sizeHuman, leverage);
                        const posId = pos.positionId || pos.id || 0;
                        // Task 5.2: Liquidation proximity — margin ratio < 120%
                        const notionalValue = sizeHuman * (mark > 0 ? mark / PRICE_SCALE : entryHuman);
                        const marginRatioPct = notionalValue > 0 ? ((marginHuman + pnl) / notionalValue) * 100 : 999;
                        const isLiqWarning = marginRatioPct < 120 && pos.status !== 'closed' && pos.status !== 'liquidated';
                        const rowClass = isLiqWarning ? 'margin-pos-row liq-warning-flash' : 'margin-pos-row';
                        // SL/TP display values
                        const slPrice = pos.slPrice || 0;
                        const tpPrice = pos.tpPrice || 0;
                        const isOpen = pos.status !== 'closed' && pos.status !== 'liquidated';
                        return `<div class="${rowClass}" data-position-id="${posId}">
                            <div class="margin-pos-info">
                                <span class="${sideClass}">${escapeHtml(side)} ${escapeHtml(pos.pair || 'LICN/lUSD')}</span>
                                <span class="mono-value">${leverage}x · ${marginType}</span>
                            </div>
                            <div class="margin-pos-details">
                                <span>Size: ${formatAmount(sizeRaw / 1e9)}</span>
                                <span>Entry: ${formatPrice(pos.entryPrice || 0)}</span>
                                <span>Mark: ${formatPrice(mark)}</span>
                                <span>Liq: <span class="text-warning">${liqPrice > 0 ? formatPrice(liqPrice) : '—'}</span></span>
                                <span class="${pnl >= 0 ? 'positive' : 'negative'}">P&L: ${pnl >= 0 ? '+' : ''}${formatPrice(pnl)} (${pnlPctStr})</span>
                                <span>Margin: ${formatAmount(marginHuman)}</span>
                                <span>SL: ${slPrice > 0 ? formatPrice(slPrice / PRICE_SCALE) : '—'}</span>
                                <span>TP: ${tpPrice > 0 ? formatPrice(tpPrice / PRICE_SCALE) : '—'}</span>
                            </div>
                            <div class="margin-pos-actions">
                                <button class="btn btn-small btn-margin-add" data-position-id="${posId}" title="Add Margin">＋</button>
                                <button class="btn btn-small btn-margin-remove" data-position-id="${posId}" title="Remove Margin">−</button>
                                ${isOpen ? `<button class="btn btn-small btn-margin-sltp" data-position-id="${posId}" title="Edit SL/TP" style="font-size:0.72rem;">SL/TP</button>` : ''}
                                ${isOpen ? `<button class="btn btn-small btn-secondary margin-close-btn" data-position-id="${posId}" data-size="${sizeRaw}" data-side="${side.toLowerCase()}">Close ▾</button>` : ''}
                                ${isOpen ? `<button class="btn btn-small btn-outline margin-share-btn" data-position-id="${posId}" data-pair="${escapeHtml(pos.pair || 'LICN/lUSD')}" data-side="${escapeHtml(side)}" data-entry="${pos.entryPrice || 0}" data-mark="${mark}" data-pnl="${pnl}" data-pnlpct="${pnlPctStr}" data-leverage="${leverage}" data-slot="${pos.createdSlot || 0}" title="Share PnL">📤</button>` : ''}
                            </div>
                            <div class="margin-sltp-inline hidden" data-position-id="${posId}" data-side="${side.toLowerCase()}" data-entry-price="${entryHuman}">
                                <div style="display:flex;gap:6px;align-items:center;">
                                    <input type="number" class="sltp-sl-input" placeholder="Stop-Loss" step="0.0001" value="${slPrice > 0 ? (slPrice / PRICE_SCALE).toFixed(4) : ''}" style="flex:1;font-size:0.8rem;" />
                                    <input type="number" class="sltp-tp-input" placeholder="Take-Profit" step="0.0001" value="${tpPrice > 0 ? (tpPrice / PRICE_SCALE).toFixed(4) : ''}" style="flex:1;font-size:0.8rem;" />
                                    <button class="btn btn-small btn-primary sltp-save-btn" data-position-id="${posId}">Save</button>
                                    <button class="btn btn-small btn-secondary sltp-cancel-btn" data-position-id="${posId}">×</button>
                                </div>
                            </div>
                            <div class="margin-pclose-inline hidden" data-position-id="${posId}" data-size="${sizeRaw}">
                                <div style="display:flex;gap:6px;align-items:center;flex-wrap:wrap;">
                                    <button class="btn btn-small btn-secondary pclose-pct-btn" data-position-id="${posId}" data-pct="25">25%</button>
                                    <button class="btn btn-small btn-secondary pclose-pct-btn" data-position-id="${posId}" data-pct="50">50%</button>
                                    <button class="btn btn-small btn-secondary pclose-pct-btn" data-position-id="${posId}" data-pct="75">75%</button>
                                    <button class="btn btn-small btn-primary pclose-pct-btn" data-position-id="${posId}" data-pct="100">100%</button>
                                    <input type="number" class="pclose-custom-input" placeholder="Custom qty" step="0.001" min="0.001" style="flex:1;font-size:0.8rem;max-width:100px;" />
                                    <button class="btn btn-small btn-primary pclose-custom-btn" data-position-id="${posId}">Go</button>
                                    <button class="btn btn-small btn-secondary pclose-cancel-btn" data-position-id="${posId}">×</button>
                                </div>
                            </div>
                            <div class="margin-adjust-inline hidden" data-position-id="${posId}">
                                <input type="number" class="margin-adjust-input" placeholder="Amount" step="0.001" min="0.001" />
                                <button class="btn btn-small btn-primary margin-adjust-confirm" data-position-id="${posId}" data-action="">Confirm</button>
                                <button class="btn btn-small btn-secondary margin-adjust-cancel" data-position-id="${posId}">Cancel</button>
                            </div>
                        </div>`;
                    }).join('');
                    // Task 5.2: Liquidation proximity notification
                    if (state.notifPrefs.liquidation !== false) {
                        const warningRows = container.querySelectorAll('.liq-warning-flash');
                        if (warningRows.length > 0) {
                            showNotification(`⚠ ${warningRows.length} position(s) near liquidation — margin ratio < 120%`, 'warning');
                        }
                    }
                    // Bind close buttons — open close modal
                    container.querySelectorAll('.margin-close-btn').forEach(btn => btn.addEventListener('click', () => {
                        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
                        if (!walletCanSign()) { showNotification('Reconnect wallet to sign', 'warning'); return; }
                        const posId = btn.dataset.positionId;
                        const fullSize = parseInt(btn.dataset.size || '0');
                        const posSide = String(btn.dataset.side || 'long').toLowerCase();
                        if (!posId || !fullSize) { showNotification('Invalid position size', 'warning'); return; }
                        showMarginCloseModal(parseInt(posId), fullSize, posSide);
                    }));
                    // Bind partial close percentage buttons
                    container.querySelectorAll('.pclose-pct-btn').forEach(btn => btn.addEventListener('click', async () => {
                        if (!state.connected || !walletCanSign()) return;
                        const posId = parseInt(btn.dataset.positionId);
                        const panel = btn.closest('.margin-pclose-inline');
                        const fullSize = parseInt(panel.dataset.size);
                        const pct = parseInt(btn.dataset.pct);
                        btn.disabled = true;
                        try {
                            let ix;
                            if (pct >= 100) {
                                ix = contractIx(contracts.dex_margin, buildClosePositionArgs(wallet.address, posId));
                            } else {
                                const closeAmt = Math.floor(fullSize * pct / 100);
                                if (closeAmt <= 0) { showNotification('Close amount too small', 'warning'); btn.disabled = false; return; }
                                ix = contractIx(contracts.dex_margin, buildPartialCloseArgs(wallet.address, posId, closeAmt));
                            }
                            await wallet.sendTransaction([ix]);
                            showNotification(pct >= 100 ? 'Position closed' : `Closed ${pct}% of position`, 'success');
                            await loadMarginPositions();
                            if (wallet.address) loadBalances(wallet.address).then(() => renderBalances()).catch(() => { });
                        } catch (e) { showNotification(`Close failed: ${e.message}`, 'error'); }
                        btn.disabled = false;
                    }));
                    // Bind partial close custom button
                    container.querySelectorAll('.pclose-custom-btn').forEach(btn => btn.addEventListener('click', async () => {
                        if (!state.connected || !walletCanSign()) return;
                        const posId = parseInt(btn.dataset.positionId);
                        const panel = btn.closest('.margin-pclose-inline');
                        const input = panel.querySelector('.pclose-custom-input');
                        const qty = parseFloat(input.value || '0');
                        if (qty <= 0) { showNotification('Enter a valid quantity', 'warning'); return; }
                        const closeAmt = Math.floor(qty * 1e9);
                        btn.disabled = true;
                        try {
                            await wallet.sendTransaction([contractIx(
                                contracts.dex_margin,
                                buildPartialCloseArgs(wallet.address, posId, closeAmt)
                            )]);
                            showNotification(`Closed ${qty} units of position`, 'success');
                            await loadMarginPositions();
                            if (wallet.address) loadBalances(wallet.address).then(() => renderBalances()).catch(() => { });
                        } catch (e) { showNotification(`Close failed: ${e.message}`, 'error'); }
                        btn.disabled = false;
                    }));
                    // Bind partial close cancel buttons
                    container.querySelectorAll('.pclose-cancel-btn').forEach(btn => btn.addEventListener('click', () => {
                        const panel = btn.closest('.margin-pclose-inline');
                        if (panel) panel.classList.add('hidden');
                    }));
                    // Bind Share PnL buttons
                    container.querySelectorAll('.margin-share-btn').forEach(btn => btn.addEventListener('click', () => {
                        showPnlShareCard({
                            pair: btn.dataset.pair,
                            side: btn.dataset.side,
                            entry: parseFloat(btn.dataset.entry),
                            mark: parseFloat(btn.dataset.mark),
                            pnl: parseFloat(btn.dataset.pnl),
                            pnlPct: btn.dataset.pnlpct,
                            leverage: btn.dataset.leverage,
                            createdSlot: parseInt(btn.dataset.slot) || 0,
                        });
                    }));
                    // Bind Add Margin buttons
                    container.querySelectorAll('.btn-margin-add').forEach(btn => btn.addEventListener('click', () => {
                        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
                        if (!walletCanSign()) { showNotification('Reconnect wallet to sign', 'warning'); return; }
                        const posId = btn.dataset.positionId;
                        const row = container.querySelector(`.margin-adjust-inline[data-position-id="${posId}"]`);
                        if (!row) return;
                        row.classList.remove('hidden');
                        row.querySelector('.margin-adjust-confirm').dataset.action = 'add';
                        row.querySelector('.margin-adjust-input').value = '';
                        row.querySelector('.margin-adjust-input').focus();
                    }));
                    // Bind Remove Margin buttons
                    container.querySelectorAll('.btn-margin-remove').forEach(btn => btn.addEventListener('click', () => {
                        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
                        if (!walletCanSign()) { showNotification('Reconnect wallet to sign', 'warning'); return; }
                        const posId = btn.dataset.positionId;
                        const row = container.querySelector(`.margin-adjust-inline[data-position-id="${posId}"]`);
                        if (!row) return;
                        row.classList.remove('hidden');
                        row.querySelector('.margin-adjust-confirm').dataset.action = 'remove';
                        row.querySelector('.margin-adjust-input').value = '';
                        row.querySelector('.margin-adjust-input').focus();
                    }));
                    // Bind cancel buttons
                    container.querySelectorAll('.margin-adjust-cancel').forEach(btn => btn.addEventListener('click', () => {
                        const posId = btn.dataset.positionId;
                        const row = container.querySelector(`.margin-adjust-inline[data-position-id="${posId}"]`);
                        if (row) row.classList.add('hidden');
                    }));
                    // Bind confirm buttons (add/remove margin)
                    container.querySelectorAll('.margin-adjust-confirm').forEach(btn => btn.addEventListener('click', async () => {
                        if (!state.connected || !walletCanSign()) { showNotification('Wallet not ready', 'warning'); return; }
                        if (!contracts.dex_margin) { showNotification('Margin contract not loaded', 'error'); return; }
                        const posId = parseInt(btn.dataset.positionId);
                        const action = btn.dataset.action;
                        const row = container.querySelector(`.margin-adjust-inline[data-position-id="${btn.dataset.positionId}"]`);
                        const input = row?.querySelector('.margin-adjust-input');
                        const amountHuman = parseFloat(input?.value);
                        if (!amountHuman || amountHuman <= 0) { showNotification('Enter a valid amount', 'warning'); return; }
                        if (amountHuman > 9_000_000) { showNotification('Amount too large', 'warning'); return; }
                        const amountScaled = Math.round(amountHuman * PRICE_SCALE);
                        btn.disabled = true;
                        try {
                            if (action === 'add') {
                                await wallet.sendTransaction([contractIx(
                                    contracts.dex_margin,
                                    buildAddMarginArgs(wallet.address, posId, amountScaled)
                                )]);
                                showNotification(`Added ${formatAmount(amountHuman)} margin`, 'success');
                            } else {
                                await wallet.sendTransaction([contractIx(
                                    contracts.dex_margin,
                                    buildRemoveMarginArgs(wallet.address, posId, amountScaled)
                                )]);
                                showNotification(`Removed ${formatAmount(amountHuman)} margin`, 'success');
                            }
                            await loadMarginPositions();
                            if (wallet.address) loadBalances(wallet.address).then(() => renderBalances()).catch(() => { });
                        } catch (e) { showNotification(`${action === 'add' ? 'Add' : 'Remove'} margin failed: ${e.message}`, 'error'); }
                        btn.disabled = false;
                    }));
                    // Bind SL/TP edit buttons
                    container.querySelectorAll('.btn-margin-sltp').forEach(btn => btn.addEventListener('click', () => {
                        const posId = btn.dataset.positionId;
                        const row = container.querySelector(`.margin-sltp-inline[data-position-id="${posId}"]`);
                        if (row) row.classList.toggle('hidden');
                    }));
                    // Bind SL/TP cancel buttons
                    container.querySelectorAll('.sltp-cancel-btn').forEach(btn => btn.addEventListener('click', () => {
                        const posId = btn.dataset.positionId;
                        const row = container.querySelector(`.margin-sltp-inline[data-position-id="${posId}"]`);
                        if (row) row.classList.add('hidden');
                    }));
                    // Bind SL/TP save buttons
                    container.querySelectorAll('.sltp-save-btn').forEach(btn => btn.addEventListener('click', async () => {
                        if (!state.connected || !walletCanSign()) { showNotification('Wallet not ready', 'warning'); return; }
                        if (!contracts.dex_margin) { showNotification('Margin contract not loaded', 'error'); return; }
                        const posId = parseInt(btn.dataset.positionId);
                        const row = container.querySelector(`.margin-sltp-inline[data-position-id="${btn.dataset.positionId}"]`);
                        const slInput = row?.querySelector('.sltp-sl-input');
                        const tpInput = row?.querySelector('.sltp-tp-input');
                        const slVal = parseFloat(slInput?.value) || 0;
                        const tpVal = parseFloat(tpInput?.value) || 0;
                        if (slVal < 0 || tpVal < 0) { showNotification('SL/TP cannot be negative', 'warning'); return; }
                        if (slVal > 9_000_000 || tpVal > 9_000_000) { showNotification('SL/TP price too large', 'warning'); return; }
                        const side = String(row?.dataset.side || 'long').toLowerCase();
                        const entryRef = Math.max(0, parseFloat(row?.dataset.entryPrice || '0') || 0);
                        const refPrice = entryRef > 0 ? entryRef : (state.lastPrice || 0);
                        if (side === 'long') {
                            if (slVal > 0 && refPrice > 0 && slVal >= refPrice) { showNotification('For LONG, SL must be below entry', 'warning'); return; }
                            if (tpVal > 0 && refPrice > 0 && tpVal <= refPrice) { showNotification('For LONG, TP must be above entry', 'warning'); return; }
                        } else {
                            if (slVal > 0 && refPrice > 0 && slVal <= refPrice) { showNotification('For SHORT, SL must be above entry', 'warning'); return; }
                            if (tpVal > 0 && refPrice > 0 && tpVal >= refPrice) { showNotification('For SHORT, TP must be below entry', 'warning'); return; }
                        }
                        btn.disabled = true;
                        try {
                            await wallet.sendTransaction([contractIx(
                                contracts.dex_margin,
                                buildSetPositionSlTpArgs(wallet.address, posId, slVal > 0 ? Math.round(slVal * PRICE_SCALE) : 0, tpVal > 0 ? Math.round(tpVal * PRICE_SCALE) : 0)
                            )]);
                            if (slVal <= 0 && tpVal <= 0) showNotification('SL/TP cleared', 'success');
                            else showNotification(`SL/TP updated${slVal > 0 ? ' SL: ' + formatPrice(slVal) : ''}${tpVal > 0 ? ' TP: ' + formatPrice(tpVal) : ''}`, 'success');
                            await loadMarginPositions();
                            await loadMarginHistory();
                        } catch (e) { showNotification(`SL/TP update failed: ${e.message}`, 'error'); }
                        btn.disabled = false;
                    }));
                }
                // Update equity stats
                let totalMargin = 0, totalUnrealizedPnl = 0;
                data.forEach(p => {
                    totalMargin += (p.margin || 0) / 1e9;
                    const mark = p.markPrice || state.lastPrice;
                    const entry = p.entryPrice || 0;
                    const size = p.size || 0;
                    const side = p.side === 'long' ? 'Long' : 'Short';
                    let uPnl = 0;
                    if (p.status !== 'closed' && p.status !== 'liquidated' && entry > 0 && mark > 0) {
                        uPnl = side === 'Long' ? (mark - entry) * size / PRICE_SCALE : (entry - mark) * size / PRICE_SCALE;
                    }
                    totalUnrealizedPnl += uPnl;
                });
                const eq = (balances.lUSD?.available || 0) + totalMargin + totalUnrealizedPnl;
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('marginEquity', formatVolume(eq));
                el('marginUsed', formatVolume(totalMargin));
                el('marginAvailable', formatVolume(eq - totalMargin));
                return;
            } else {
                // No positions — show empty state
                if (container) container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-chart-line" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>No open positions</div>';
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('marginEquity', formatVolume(balances.lUSD?.available || 0));
                el('marginUsed', '$0.00');
                el('marginAvailable', formatVolume(balances.lUSD?.available || 0));
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
                state._tradeHistoryData = data; // Task 7.2: Cache for CSV export
                container.innerHTML = `<div class="trade-history-header"><button class="btn btn-tiny btn-secondary export-csv-btn" id="exportCsvBtn" title="Export trades as CSV"><i class="fas fa-download"></i> Export CSV</button></div><table class="orders-table"><thead><tr><th>Pair</th><th>Side</th><th>Price</th><th>Amount</th><th>Total</th><th>Fee</th><th>Time</th></tr></thead><tbody>${data.map(tr => { const qty = (tr.quantity || tr.amount || 0) / 1e9; const fee = (tr.fee || 0) / 1e9; return `<tr><td>${escapeHtml(state.activePair?.id || '')}</td><td class="side-${escapeHtml(tr.side || 'buy')}">${escapeHtml((tr.side || 'buy').toUpperCase())}</td><td class="mono-value">${formatPrice(tr.price || 0)}</td><td class="mono-value">${formatAmount(qty)}</td><td class="mono-value">${formatPrice((tr.price || 0) * qty)}</td><td class="mono-value">${formatAmount(fee)}</td><td class="mono-value" style="color:var(--text-muted)">${tr.timestamp ? new Date(tr.timestamp).toLocaleString() : ''}</td></tr>`; }).join('')
                    }</tbody></table>`;
                const exportBtn = document.getElementById('exportCsvBtn');
                if (exportBtn) exportBtn.addEventListener('click', exportTradeHistoryCSV);
                return;
            }
            container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-history" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>No spot trade history</div>';
        } catch {
            container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-circle-exclamation" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>Unable to load trade history</div>';
        }
    }

    async function loadMarginHistory() {
        const container = document.getElementById('content-margin-history');
        if (!container) return;
        if (!state.connected) {
            container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:30px;font-size:0.85rem;"><i class="fas fa-wallet" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>Connect wallet to view margin history</div>';
            return;
        }
        try {
            const { data } = await api.get(`/margin/positions?trader=${wallet.address}`);
            const rows = Array.isArray(data) ? data.slice() : [];
            rows.sort((a, b) => Number(b.updatedSlot || b.closedSlot || b.createdSlot || 0) - Number(a.updatedSlot || a.closedSlot || a.createdSlot || 0));
            if (!rows.length) {
                container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-history" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>No margin history</div>';
                return;
            }
            container.innerHTML = `<table class="orders-table"><thead><tr><th>Pair</th><th>Side</th><th>Mode</th><th>Entry</th><th>Exit/Mark</th><th>Size</th><th>PnL</th><th>Status</th></tr></thead><tbody>${rows.map(pos => {
                const sideRaw = String(pos.side || 'long').toLowerCase();
                const sideLabel = sideRaw === 'short' ? 'SHORT' : 'LONG';
                const sideClass = sideRaw === 'short' ? 'side-sell' : 'side-buy';
                const modeLabel = pos.marginType === 'cross' ? 'Cross' : 'Isolated';
                const pairLabel = escapeHtml(pos.pair || state.activePair?.id || '—');
                const statusRaw = String(pos.status ?? '').toLowerCase();
                const statusLabel = statusRaw || 'open';
                const entryValue = Number(pos.entryPrice || 0);
                const exitValue = Number(pos.exitPrice || pos.markPrice || state.lastPrice || 0);
                const sizeValue = Number(pos.size || 0) / PRICE_SCALE;
                const pnlRaw = Number(pos.realizedPnl ?? pos.pnl ?? pos.unrealizedPnl ?? 0);
                const pnlClass = pnlRaw >= 0 ? 'positive' : 'negative';
                const pnlPrefix = pnlRaw >= 0 ? '+' : '';
                return `<tr>
                    <td>${pairLabel}</td>
                    <td class="${sideClass}">${sideLabel}</td>
                    <td>${modeLabel}</td>
                    <td class="mono-value">${formatPrice(entryValue)}</td>
                    <td class="mono-value">${formatPrice(exitValue)}</td>
                    <td class="mono-value">${formatAmount(sizeValue)}</td>
                    <td class="mono-value ${pnlClass}">${pnlPrefix}${formatPrice(pnlRaw)}</td>
                    <td>${escapeHtml(statusLabel.toUpperCase())}</td>
                </tr>`;
            }).join('')}</tbody></table>`;
        } catch {
            container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-exclamation-triangle" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>Failed to load margin history</div>';
        }
    }

    // Task 7.2: Export trade history as CSV
    function exportTradeHistoryCSV() {
        const data = state._tradeHistoryData;
        if (!data || !data.length) { showNotification('No trade data to export', 'warning'); return; }
        const pair = state.activePair?.id || 'UNKNOWN';
        const rows = [['Date', 'Pair', 'Side', 'Price', 'Amount', 'Total', 'Fee']];
        data.forEach(tr => {
            const qty = (tr.quantity || tr.amount || 0) / 1e9;
            const price = tr.price || 0;
            const total = price * qty;
            const fee = (tr.fee || 0) / 1e9;
            const date = tr.timestamp ? new Date(tr.timestamp).toISOString() : '';
            const side = (tr.side || 'buy').toUpperCase();
            rows.push([date, pair, side, price, qty, total, fee]);
        });
        const csv = rows.map(r => r.map(c => typeof c === 'string' && c.includes(',') ? `"${c}"` : c).join(',')).join('\n');
        const blob = new Blob([csv], { type: 'text/csv;charset=utf-8;' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        const dateStr = new Date().toISOString().split('T')[0];
        a.href = url; a.download = `lichen-trades-${dateStr}.csv`;
        document.body.appendChild(a); a.click(); document.body.removeChild(a);
        URL.revokeObjectURL(url);
        showNotification('Trade history exported', 'success');
    }

    // Margin open position is now handled inline in the Trade view submit handler

    // ═══════════════════════════════════════════════════════════════════════
    // Rewards View — Load from API
    // ═══════════════════════════════════════════════════════════════════════
    // F13.2: Compute tier from volume client-side (contract thresholds in spores: 1 LICN = 1e9 spores)
    const TIER_THRESHOLDS = [
        { name: 'Bronze', max: 100_000_000_000_000, mult: 1.0 },  // < 100K LICN
        { name: 'Silver', max: 1_000_000_000_000_000, mult: 1.5 },  // 100K — 1M LICN
        { name: 'Gold', max: 10_000_000_000_000_000, mult: 2.0 },  // 1M — 10M LICN
        { name: 'Diamond', max: Infinity, mult: 3.0 },  // >= 10M LICN
    ];

    function computeRewardTier(volumeSpores) {
        for (let i = 0; i < TIER_THRESHOLDS.length; i++) {
            if (volumeSpores < TIER_THRESHOLDS[i].max) return i;
        }
        return TIER_THRESHOLDS.length - 1;
    }

    async function loadRewardsStats() {
        // Global stats
        try {
            const { data } = await api.get('/stats/rewards');
            if (data) {
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('rewardsTotalDist', formatAmount(data.totalDistributed ? data.totalDistributed / 1e9 : 0) + ' LICN');
            }
        } catch { /* API unavailable */ }
        // F13.4: Generate referral link when wallet connected
        if (state.connected) {
            const refEl = document.getElementById('referralLink');
            if (refEl) refEl.textContent = `${location.origin}?ref=${wallet.address}`;
        }
        // User rewards
        if (!state.connected) return;
        try {
            const { data } = await api.get(`/rewards/${wallet.address}`);
            if (data) {
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                const pending = data.pending ? data.pending / 1e9 : 0;
                el('rewardsPending', formatAmount(pending) + ' LICN');
                el('rewardsPendingUsd', `≈ $${formatAmount(pending * state.lastPrice)}`);
                // F13.2: Compute tier from totalVolume (camelCase from RPC)
                const volume = data.totalVolume || 0;
                const tierNum = computeRewardTier(volume);
                const tier = TIER_THRESHOLDS[tierNum];
                const tierName = tier.name;
                // F13.14: Use innerHTML directly, no redundant textContent
                const tierEl = document.getElementById('rewardsTier');
                if (tierEl) tierEl.innerHTML = `<span class="tier-badge ${tierName.toLowerCase()}">${tierName}</span>`;
                el('rewardsMultiplier', `${tier.mult}x`);
                el('rewardsMultiplierSub', `${tierName} tier bonus`);
                // F13.6: Update tier progress bar
                const tierMin = tierNum > 0 ? TIER_THRESHOLDS[tierNum - 1].max : 0;
                const tierMax = tier.max === Infinity ? tierMin * 10 : tier.max;
                const pct = tierMax > tierMin ? Math.min(100, ((volume - tierMin) / (tierMax - tierMin)) * 100) : 100;
                const tierBar = document.querySelector('.tier-bar');
                if (tierBar) tierBar.style.width = `${pct.toFixed(1)}%`;
                // Update tier progress text
                const volLicn = volume / 1e9;
                const progStats = document.querySelectorAll('.tier-your-progress .progress-stat .mono-value');
                if (progStats.length >= 2) {
                    progStats[0].textContent = formatAmount(volLicn) + ' LICN';
                    if (tierNum < TIER_THRESHOLDS.length - 1) {
                        const nextTier = TIER_THRESHOLDS[tierNum + 1] || TIER_THRESHOLDS[tierNum];
                        const nextName = nextTier === TIER_THRESHOLDS[tierNum] ? tierName : TIER_THRESHOLDS[tierNum + 1].name;
                        const remaining = (tier.max / 1e9) - volLicn;
                        progStats[1].textContent = `${formatAmount(remaining)} LICN to ${tierNum < 3 ? TIER_THRESHOLDS[tierNum + 1].name : tierName}`;
                    } else {
                        progStats[1].textContent = 'Max tier reached!';
                    }
                }
                // Highlight active tier row in table
                const tierRows = document.querySelectorAll('.tier-table-row:not(.header-row)');
                tierRows.forEach((row, idx) => {
                    row.classList.toggle('active-tier', idx === tierNum);
                });
                // Trading reward card metrics
                el('rewardTradePending', formatAmount(pending) + ' LICN');
                // F13.7: Use claimed (available from RPC) for "All Time"; no monthly field in contract
                const claimed = data.claimed ? data.claimed / 1e9 : 0;
                el('rewardTradeMonth', '—');
                el('rewardTradeAll', formatAmount(claimed + pending) + ' LICN');
                // LP Mining card — no per-user LP reward data in contract; show pending or —
                el('rewardLpPending', '—');
                el('rewardLpPositions', '—');
                el('rewardLpLiquidity', '—');
                // F13.3: Referral card metrics — use camelCase field names from RPC
                el('rewardRefCount', (data.referralCount ?? 0) + ' traders');
                el('rewardRefEarnings', formatAmount(data.referralEarnings ? data.referralEarnings / 1e9 : 0) + ' LICN');
                el('rewardRefRate', '10%');
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
                el('govTotalProposals', data.proposalCount ?? '—');
                el('govActiveProposals', data.activeProposals ?? '—');
            }
        } catch { /* API unavailable */ }
    }

    async function loadProposals() {
        try {
            const { data, slot: currentSlot } = await api.get('/governance/proposals');
            // F16.9: Time remaining uses currentSlot * 0.4 seconds per Lichen slot
            if (Array.isArray(data) && data.length > 0) {
                const container = document.getElementById('proposalsList');
                if (container) {
                    container.innerHTML = data.map(p => {
                        const status = p.status || 'active';
                        const yesVotes = p.yesVotes || 0;
                        const noVotes = p.noVotes || 0;
                        const totalVotes = yesVotes + noVotes;
                        const minQuorum = Number(state.governanceMinQuorum || GOVERNANCE_MIN_QUORUM_DEFAULT);
                        const quorumMet = totalVotes >= minQuorum;
                        const quorumShortfall = Math.max(0, minQuorum - totalVotes);
                        const yesPct = totalVotes > 0 ? Math.round(yesVotes / totalVotes * 100) : 50;
                        const statusClass = status === 'active' ? 'active-proposal' : status === 'passed' ? 'passed-proposal' : status === 'rejected' ? 'rejected-proposal' : 'executed-proposal';
                        // F14.5: Generate title from proposalType + proposalId
                        const typeLabels = { new_pair: 'New Pair Listing', fee_change: 'Fee Change', delist: 'Pair Delisting', param_change: 'Parameter Change' };
                        const safeTitle = escapeHtml(typeLabels[p.proposalType] || p.proposalType || 'Proposal') + ` #${p.proposalId || 0}`;
                        const safeType = escapeHtml(p.proposalType || 'New Pair');
                        const safeStatus = escapeHtml(status.charAt(0).toUpperCase() + status.slice(1));
                        // F16.9: Compute time remaining from endSlot using API slot (1s per slot)
                        let timeStr = '';
                        const nowSlot = currentSlot || 0;
                        const votingEnded = p.endSlot && nowSlot > p.endSlot;
                        if (p.endSlot && status === 'active') {
                            const remaining = (p.endSlot - nowSlot) * 0.4; // slot-to-seconds
                            if (remaining > 3600) timeStr = `${Math.floor(remaining / 3600)}h ${Math.floor((remaining % 3600) / 60)}m remaining`;
                            else if (remaining > 0) timeStr = `${Math.floor(remaining / 60)}m remaining`;
                            else timeStr = 'Voting ended';
                        }
                        // F14.6: Show evidence if available
                        let evidenceHtml = '';
                        if (p.proposalType === 'new_pair' && p.baseToken) {
                            evidenceHtml = `<p class="proposal-desc text-secondary">Base: ${escapeHtml(p.baseToken.substring(0, 8))}... Quote: Pair #${p.pairId || 0}</p>`;
                        } else if (p.proposalType === 'fee_change' && (p.newMakerFee !== undefined || p.newTakerFee !== undefined)) {
                            evidenceHtml = `<p class="proposal-desc text-secondary">Maker: ${p.newMakerFee ?? '—'} bps, Taker: ${p.newTakerFee ?? '—'} bps (Pair #${p.pairId || 0})</p>`;
                        } else {
                            evidenceHtml = `<p class="proposal-desc text-secondary">Pair #${p.pairId || 0}</p>`;
                        }

                        // Task 6.3: Proposal status pipeline
                        const pipelineStages = ['Created', 'Voting', 'Finalized', 'Executed'];
                        let activeStage = 0;
                        if (status === 'active' && !votingEnded) activeStage = 1;
                        else if (status === 'active' && votingEnded) activeStage = 1; // ready to finalize
                        else if (status === 'passed') activeStage = 2;
                        else if (status === 'executed') activeStage = 3;
                        else if (status === 'rejected') activeStage = 2; // finalized as rejected
                        const pipelineHtml = `<div class="proposal-pipeline">${pipelineStages.map((s, i) => {
                            let cls = 'pipeline-step';
                            if (i < activeStage) cls += ' completed';
                            else if (i === activeStage) cls += ' active';
                            if (status === 'rejected' && i === 2) cls += ' rejected';
                            if (status === 'rejected' && i === 3) cls = 'pipeline-step skipped';
                            return `<div class="${cls}"><div class="pipeline-dot"></div><span>${status === 'rejected' && i === 2 ? 'Rejected' : s}</span></div>`;
                        }).join('<div class="pipeline-line"></div>')}</div>`;

                        // Task 6.1/6.2: Action buttons based on lifecycle
                        let actionHtml = '';
                        if (status === 'active' && !votingEnded) {
                            actionHtml = `<div class="proposal-actions">
                                <button class="btn btn-small btn-primary vote-btn vote-for">Vote Yes</button>
                                <button class="btn btn-small btn-secondary vote-btn vote-against">Vote No</button>
                            </div>`;
                        } else if (status === 'active' && votingEnded) {
                            if (quorumMet) {
                                actionHtml = `<div class="proposal-actions">
                                    <button class="btn btn-small btn-primary finalize-btn" data-proposal-id="${p.proposalId || p.id || 0}">Finalize</button>
                                </div>`;
                            } else {
                                actionHtml = `<div class="proposal-actions"><span class="text-secondary" style="font-size:0.82rem;">Quorum not met (${totalVotes}/${minQuorum}). ${quorumShortfall} more vote(s) required.</span></div>`;
                            }
                        } else if (status === 'passed') {
                            actionHtml = `<div class="proposal-actions">
                                <button class="btn btn-small btn-primary execute-btn" data-proposal-id="${p.proposalId || p.id || 0}">Execute</button>
                            </div>`;
                        }

                        return `<div class="proposal-card ${statusClass}" data-proposal-id="${p.proposalId || p.id || 0}">
                            <div class="proposal-top-row">
                                <div class="proposal-status-badge ${status}">${safeStatus}</div>
                                <span class="proposal-type-tag">${safeType}</span>
                                <span class="proposal-id">#${p.proposalId || p.id || 0}</span>
                            </div>
                            <h4>${safeTitle}</h4>
                            ${evidenceHtml}
                            ${pipelineHtml}
                            <div class="proposal-votes">
                                <div class="vote-bar"><div class="vote-yes" style="width: ${yesPct}%"></div></div>
                                <div class="vote-counts">
                                    <span class="vote-yes-text"><i class="fas fa-check"></i> ${yesPct}% Yes (${yesVotes} votes)</span>
                                    <span class="vote-no-text"><i class="fas fa-times"></i> ${100 - yesPct}% No (${noVotes} votes)</span>
                                </div>
                            </div>
                            <div class="proposal-footer">
                                <span class="proposal-time"><i class="fas fa-clock"></i> ${timeStr}</span>
                                ${actionHtml}
                            </div>
                        </div>`;
                    }).join('');
                    // Rebind vote buttons
                    bindVoteButtons();
                    // Task 6.1: Bind finalize buttons
                    bindFinalizeButtons();
                    // Task 6.2: Bind execute buttons
                    bindExecuteButtons();
                    // F14.10: Re-apply filter after DOM rebuild
                    applyGovernanceFilter();
                    // Re-apply wallet gating on dynamically rendered vote buttons
                    applyWalletGateAll();
                }
                return;
            }
        } catch { /* API unavailable — keep empty state */ }
        // Bind vote buttons on static content
        bindVoteButtons();
        applyWalletGateAll();
    }

    function bindVoteButtons() {
        document.querySelectorAll('.vote-btn').forEach(btn => btn.addEventListener('click', async () => {
            if (!state.connected) { showNotification('Connect wallet to vote', 'warning'); return; }
            if (!walletCanSign()) { showNotification('Reconnect wallet to sign', 'warning'); return; }
            const card = btn.closest('.proposal-card');
            // F14.7: Contract uses LichenID reputation check (>=500), not LICN balance
            // Vote via signed sendTransaction
            const pid = card?.dataset?.proposalId;
            const title = card?.querySelector('h4')?.textContent || '';
            btn.disabled = true; btn.style.opacity = '0.5';
            try {
                if (pid) {
                    await wallet.sendTransaction([contractIx(
                        contracts.dex_governance,
                        buildVoteArgs(wallet.address, parseInt(pid), btn.classList.contains('vote-for'))
                    )]);
                }
            } catch (e) { showNotification(`Vote failed: ${e.message}`, 'error'); return; }
            showNotification(`Vote submitted on "${escapeHtml(title)}"`, 'success');
            // F24.6 FIX: Refresh proposals after vote
            loadProposals().catch(() => { });
        }));
    }

    // Task 6.1: Finalize proposal button handler
    function bindFinalizeButtons() {
        document.querySelectorAll('.finalize-btn').forEach(btn => btn.addEventListener('click', async () => {
            if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
            if (!walletCanSign()) { showNotification('Reconnect wallet to sign', 'warning'); return; }
            if (!contracts.dex_governance) { showNotification('Governance contract not loaded', 'error'); return; }
            const pid = parseInt(btn.dataset.proposalId);
            if (!pid) return;
            btn.disabled = true; btn.textContent = 'Finalizing...';
            try {
                await wallet.sendTransaction([contractIx(
                    contracts.dex_governance,
                    buildFinalizeProposalArgs(pid)
                )]);
                showNotification('Proposal finalized', 'success');
                loadProposals().catch(() => { });
            } catch (e) {
                showNotification(`Finalize failed: ${e.message}`, 'error');
            }
            btn.disabled = false; btn.textContent = 'Finalize';
        }));
    }

    // Task 6.2: Execute proposal button handler
    function bindExecuteButtons() {
        document.querySelectorAll('.execute-btn').forEach(btn => btn.addEventListener('click', async () => {
            if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
            if (!walletCanSign()) { showNotification('Reconnect wallet to sign', 'warning'); return; }
            if (!contracts.dex_governance) { showNotification('Governance contract not loaded', 'error'); return; }
            const pid = parseInt(btn.dataset.proposalId);
            if (!pid) return;
            btn.disabled = true; btn.textContent = 'Executing...';
            try {
                await wallet.sendTransaction([contractIx(
                    contracts.dex_governance,
                    buildExecuteProposalArgs(pid)
                )]);
                showNotification('Proposal executed', 'success');
                loadProposals().catch(() => { });
            } catch (e) {
                showNotification(`Execute failed: ${e.message}`, 'error');
            }
            btn.disabled = false; btn.textContent = 'Execute';
        }));
    }

    // Proposal type toggle
    const proposalTypeBtns = document.querySelectorAll('.proposal-type-btn');
    const pairFields = document.getElementById('pairFields');
    const feeFields = document.getElementById('feeFields');
    const delistFields = document.getElementById('delistFields');
    const paramFields = document.getElementById('paramFields');
    const SUPPORTED_PROPOSAL_TYPES = new Set(['pair', 'fee', 'delist', 'param']);
    proposalTypeBtns.forEach(btn => btn.addEventListener('click', () => {
        proposalTypeBtns.forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        const ptype = SUPPORTED_PROPOSAL_TYPES.has(btn.dataset.ptype) ? btn.dataset.ptype : 'pair';
        if (pairFields) pairFields.classList.toggle('hidden', ptype !== 'pair');
        if (feeFields) feeFields.classList.toggle('hidden', ptype !== 'fee');
        if (delistFields) delistFields.classList.toggle('hidden', ptype !== 'delist');
        if (paramFields) paramFields.classList.toggle('hidden', ptype !== 'param');
    }));

    // F10E.5: Parameter selector — show current value + description
    const propParamName = document.getElementById('propParamName');
    if (propParamName) propParamName.addEventListener('change', () => {
        const opt = propParamName.options[propParamName.selectedIndex];
        const current = opt?.dataset?.current || '—';
        const unit = opt?.dataset?.unit || '';
        const desc = opt?.dataset?.desc || '';
        const curEl = document.getElementById('propParamCurrent');
        const unitEl = document.getElementById('propParamUnit');
        const descEl = document.getElementById('propParamDesc');
        if (curEl) curEl.textContent = `${current}${unit}`;
        if (unitEl) unitEl.textContent = unit;
        if (descEl) descEl.textContent = desc;
    });

    // F14.10: Reusable governance filter function
    function applyGovernanceFilter() {
        const activeBtn = document.querySelector('.proposals-section .filter-pill.active');
        const filter = activeBtn?.dataset?.filter || 'all';
        document.querySelectorAll('.proposal-card').forEach(card => {
            if (filter === 'all') card.style.display = '';
            else card.style.display = card.classList.contains('active-proposal') ? '' : 'none';
        });
    }

    // Governance filter pills
    document.querySelectorAll('.proposals-section .filter-pill').forEach(btn => btn.addEventListener('click', () => {
        document.querySelectorAll('.proposals-section .filter-pill').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        applyGovernanceFilter();
    }));

    // Submit Proposal handler
    const submitProposalBtn = document.getElementById('submitProposalBtn');
    if (submitProposalBtn) submitProposalBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet to propose', 'warning'); return; }
        if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }
        const activeType = document.querySelector('.proposal-type-btn.active');
        const ptype = SUPPORTED_PROPOSAL_TYPES.has(activeType?.dataset?.ptype) ? activeType.dataset.ptype : 'pair';
        submitProposalBtn.disabled = true; submitProposalBtn.textContent = 'Submitting...';
        try {
            // AUDIT-FIX: Proposal submission via signed sendTransaction (not unsigned REST)
            let proposalData = { op: 'create_proposal', proposal_type: ptype };
            if (ptype === 'pair') {
                const base = document.getElementById('propBaseToken')?.value?.trim();
                const quote = document.getElementById('propQuoteToken')?.value?.trim();
                if (!base || !quote) { showNotification('Enter base and quote tokens', 'warning'); submitProposalBtn.disabled = false; submitProposalBtn.innerHTML = '<i class="fas fa-paper-plane"></i> Submit Proposal'; return; }
                proposalData.base_token = base;
                proposalData.quote_token = quote;
            } else if (ptype === 'fee') {
                proposalData.pair = document.getElementById('propFeePair')?.value || 'LICN/lUSD';
                proposalData.maker_fee = parseInt(document.getElementById('propMakerFee')?.value) || -1;
                proposalData.taker_fee = parseInt(document.getElementById('propTakerFee')?.value) || 5;
            } else if (ptype === 'delist') {
                const propDelistReason = document.getElementById('propDelistReason');
                const delistReason = propDelistReason?.value?.trim() || '';
                proposalData.pair_id = parseInt(document.getElementById('propDelistPair')?.value) || 0;
                proposalData.reason = delistReason;
            } else if (ptype === 'param') {
                const propParamValue = document.getElementById('propParamValue');
                proposalData.parameter = document.getElementById('propParamName')?.value?.trim() || '';
                proposalData.proposed_value = propParamValue?.value?.trim() || '';
            }
            // Build binary args based on proposal type
            let govArgs;
            if (ptype === 'pair' && proposalData.base_token && proposalData.quote_token) {
                // F14.1: opcode 1 — propose_new_pair(proposer[32], base_token[32], quote_token[32]) = 97 bytes
                const buf = new ArrayBuffer(97);
                const a = new Uint8Array(buf);
                writeU8(a, 0, 1);
                writePubkey(a, 1, wallet.address);
                // base_token and quote_token must be valid base58 addresses
                try {
                    writePubkey(a, 33, proposalData.base_token);
                    writePubkey(a, 65, proposalData.quote_token);
                } catch {
                    showNotification('Invalid token address — enter a valid base58 address', 'warning');
                    submitProposalBtn.disabled = false; submitProposalBtn.innerHTML = '<i class="fas fa-paper-plane"></i> Submit Proposal';
                    return;
                }
                govArgs = a;
            } else if (ptype === 'fee' && proposalData.pair) {
                // opcode 9: propose_fee_change(proposer, pair_id, maker_fee, taker_fee)
                const pairObj = pairs.find(p => p.id === proposalData.pair || String(p.pairId) === String(proposalData.pair));
                const pairIdVal = pairObj?.pairId || parseInt(proposalData.pair) || 0;
                const buf = new ArrayBuffer(45);
                const v = new DataView(buf);
                const a = new Uint8Array(buf);
                writeU8(a, 0, 9);
                writePubkey(a, 1, wallet.address);
                writeU64LE(v, 33, pairIdVal);
                v.setInt16(41, proposalData.maker_fee || -1, true);
                v.setUint16(43, proposalData.taker_fee || 5, true);
                govArgs = a;
            } else if (ptype === 'delist') {
                // Delist proposals not yet supported on-chain — governance contract lacks delist opcode
                showNotification('Delist proposals are not yet supported on-chain. Governance contract update required.', 'warning');
                submitProposalBtn.disabled = false; submitProposalBtn.innerHTML = '<i class="fas fa-paper-plane"></i> Submit Proposal';
                return;
            } else if (ptype === 'param') {
                // Param proposals not yet supported on-chain — governance contract lacks param_change opcode
                showNotification('Parameter change proposals are not yet supported on-chain. Governance contract update required.', 'warning');
                submitProposalBtn.disabled = false; submitProposalBtn.innerHTML = '<i class="fas fa-paper-plane"></i> Submit Proposal';
                return;
            } else {
                showNotification('Please fill in all required fields', 'warning');
                submitProposalBtn.disabled = false; submitProposalBtn.innerHTML = '<i class="fas fa-paper-plane"></i> Submit Proposal';
                return;
            }
            await wallet.sendTransaction([contractIx(contracts.dex_governance, govArgs)]);
            if (ptype === 'pair') {
                showNotification(`Proposal submitted: List ${escapeHtml(proposalData.base_token)}/${escapeHtml(proposalData.quote_token)}`, 'success');
            } else if (ptype === 'fee') {
                showNotification(`Fee change proposal submitted for ${escapeHtml(proposalData.pair)}`, 'success');
            } else {
                showNotification('Proposal submitted', 'success');
            }
        } catch (e) { showNotification(`Proposal failed: ${e.message}`, 'error'); }
        finally { submitProposalBtn.disabled = false; submitProposalBtn.innerHTML = '<i class="fas fa-paper-plane"></i> Submit Proposal'; }
    });

    // ═══════════════════════════════════════════════════════════════════════
    // Prediction Market — Predict View (Live API)
    // ═══════════════════════════════════════════════════════════════════════

    // Only real on-chain prediction markets displayed
    const INITIAL_MARKETS = [];
    const PREDICT_DISPUTE_BOND = 100_000_000; // 100 lUSD

    const predictState = {
        selectedMarket: 1,
        selectedOutcome: 'yes',
        activeCategory: 'all',
        markets: [...INITIAL_MARKETS],
        positions: [],
        stats: null,
        live: false,
        lastMarketsError: null,
    };

    // ─── Load prediction stats from API ─────────────────────────
    async function loadPredictionStats() {
        try {
            const { data } = await api.get('/prediction-market/stats');
            if (data) {
                predictState.stats = data;
                setPredictSlotAnchor(data.current_slot || 0);
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
            setPredictSlotAnchor(data?.current_slot || 0);
            const liveSlot = getRealtimePredictSlot(predictState.stats?.current_slot || 0);
            let marketRows = Array.isArray(data) ? data : (Array.isArray(data?.markets) ? data.markets : []);
            if (!marketRows.length) {
                try {
                    const rpcMarkets = await api.rpc('getPredictionMarkets', [{ limit: 50, offset: 0 }]);
                    marketRows = Array.isArray(rpcMarkets)
                        ? rpcMarkets
                        : (Array.isArray(rpcMarkets?.markets) ? rpcMarkets.markets : []);
                } catch { /* fallback unavailable */ }
            }
            if (marketRows.length > 0) {
                // Transform API data into UI format
                predictState.markets = marketRows.map(m => ({
                    id: m.id,
                    question: m.question,
                    cat: m.category,
                    yes: m.outcomes?.[0]?.price ?? 0.5,
                    // F11.4 FIX: RPC already divides by PRICE_SCALE — no *1e9
                    volume: Number(m.total_volume ?? m.volume ?? 0),
                    liquidity: Number(m.total_collateral ?? m.liquidity ?? 0),
                    // F11.9 FIX: Use unique_traders from market response (no N+1 query)
                    traders: Number(m.unique_traders ?? m.traders ?? 0),
                    status: (m.status === 'active' && Number(m.close_slot || 0) > 0 && liveSlot >= Number(m.close_slot || 0))
                        ? 'closed'
                        : m.status,
                    multi: (m.outcome_count || 2) > 2,
                    outcomes: m.outcomes || [],
                    // F11.7 FIX: Map close_slot and creator for time remaining and attribution
                    closes: m.close_slot || 0,
                    creator: m.creator || '',
                    // Task 8.1: Fields for challenge/dispute lifecycle
                    dispute_end_slot: m.dispute_end_slot || 0,
                    current_slot: Number(m.current_slot ?? liveSlot ?? predictState.stats?.current_slot ?? data.current_slot ?? 0),
                    resolver: m.resolver || '',
                    winning_outcome: m.winning_outcome,
                    resolved_outcome: typeof m.resolved_outcome === 'string'
                        ? m.resolved_outcome.toLowerCase()
                        : (m.winning_outcome === 0 ? 'yes' : (m.winning_outcome === 1 ? 'no' : '')),
                }));
                predictState.live = true;
                predictState.lastMarketsError = null;
                renderPredictionMarkets();
                return;
            }
            // Empty state — no markets from API
            predictState.markets = [];
            predictState.live = true;
            predictState.lastMarketsError = null;
            renderPredictionMarkets();
        } catch {
            predictState.live = true;
            predictState.lastMarketsError = 'Unable to load markets right now';
            if (!predictState.markets.length) renderPredictionMarkets();
        }
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
        } catch {
            const tbody = document.querySelector('.predict-positions-table tbody') || document.getElementById('predictPositionsBody');
            if (tbody) tbody.innerHTML = '<tr><td colspan="8" style="text-align:center;color:var(--text-muted);padding:20px;"><i class="fas fa-circle-exclamation" style="margin-right:6px;"></i>Unable to load positions</td></tr>';
        }
    }

    async function loadPredictionHistory() {
        const container = document.getElementById('predict-history');
        if (!container) return;
        if (!state.connected) {
            container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:30px;font-size:0.85rem;"><i class="fas fa-wallet" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>Connect wallet to view trade history</div>';
            return;
        }

        const addrEncoded = encodeURIComponent(wallet.address || '');

        try {
            const [{ data: stats }, { data: trades }] = await Promise.all([
                api.get(`/prediction-market/traders/${addrEncoded}/stats`),
                api.get(`/prediction-market/trades?address=${addrEncoded}&limit=50`),
            ]);

            const rows = Array.isArray(trades) ? trades : [];
            const tradeCount = Number(stats?.trade_count || 0);
            const totalVolume = Number(stats?.total_volume || 0);
            const lastSlot = Number(stats?.last_trade_slot || 0);

            const tableRows = rows.length
                ? rows.map((t) => {
                    const actionRaw = String(t.action || '');
                    const actionLabel = actionRaw.replace(/_/g, ' ').toUpperCase();
                    const actionClass = actionRaw === 'buy' ? 'positive' : (actionRaw === 'sell' ? 'negative' : '');
                    const outcomeLabel = t.outcome === 0 ? 'YES' : (t.outcome === 1 ? 'NO' : '—');
                    const amount = Number(t.amount || 0);
                    const timestamp = Number(t.timestamp || 0);
                    const timeLabel = timestamp > 0 ? new Date(timestamp * 1000).toLocaleString() : `Slot ${formatNumber(t.slot || 0)}`;
                    return `<tr>
                        <td class="mono-value" style="padding:8px;">#${formatNumber(t.market_id || 0)}</td>
                        <td class="${actionClass}" style="padding:8px;font-weight:700;">${escapeHtml(actionLabel)}</td>
                        <td style="padding:8px;">${escapeHtml(outcomeLabel)}</td>
                        <td class="mono-value" style="padding:8px;">$${formatAmount(amount)}</td>
                        <td class="mono-value" style="padding:8px;">${formatNumber(t.slot || 0)}</td>
                        <td style="padding:8px;font-size:0.8rem;color:var(--text-muted);">${escapeHtml(timeLabel)}</td>
                    </tr>`;
                }).join('')
                : '<tr><td colspan="6" style="text-align:center;color:var(--text-muted);padding:16px;">No prediction trades yet</td></tr>';

            container.innerHTML = `
                <div style="display:grid;grid-template-columns:repeat(3,minmax(120px,1fr));gap:12px;padding:12px 8px 14px;">
                    <div class="panel-card" style="padding:10px;"><div style="font-size:0.75rem;color:var(--text-muted)">Trades</div><div class="mono-value" style="font-size:1rem;">${formatNumber(tradeCount)}</div></div>
                    <div class="panel-card" style="padding:10px;"><div style="font-size:0.75rem;color:var(--text-muted)">Total Volume</div><div class="mono-value" style="font-size:1rem;">${formatVolume(totalVolume)}</div></div>
                    <div class="panel-card" style="padding:10px;"><div style="font-size:0.75rem;color:var(--text-muted)">Last Trade Slot</div><div class="mono-value" style="font-size:1rem;">${lastSlot > 0 ? formatNumber(lastSlot) : '—'}</div></div>
                </div>
                <div style="overflow:auto;padding:0 8px 14px;">
                    <table style="width:100%;border-collapse:collapse;font-size:0.82rem;">
                        <thead>
                            <tr style="text-align:left;color:var(--text-muted);border-bottom:1px solid var(--border-color);">
                                <th style="padding:8px;">Market</th>
                                <th style="padding:8px;">Action</th>
                                <th style="padding:8px;">Outcome</th>
                                <th style="padding:8px;">Amount</th>
                                <th style="padding:8px;">Slot</th>
                                <th style="padding:8px;">Time</th>
                            </tr>
                        </thead>
                        <tbody>${tableRows}</tbody>
                    </table>
                </div>`;
        } catch {
            container.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:30px;font-size:0.85rem;"><i class="fas fa-circle-exclamation" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>Unable to load prediction activity</div>';
        }
    }

    // F12.5 FIX: Load and render "My Markets" tab — markets created by connected wallet
    async function loadCreatedMarkets() {
        const tbody = document.getElementById('predictCreatedBody');
        if (!tbody) return;
        if (!state.connected) {
            tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;color:var(--text-muted);padding:20px;"><i class="fas fa-wallet" style="margin-right:6px;"></i>Connect wallet to view your markets</td></tr>';
            return;
        }
        try {
            const creatorFilters = [walletAddressToHex(wallet.address), wallet.address].filter(Boolean);
            let markets = [];
            for (const creator of creatorFilters) {
                const resp = await api.get(`/prediction-market/markets?creator=${encodeURIComponent(creator)}`);
                const rows = Array.isArray(resp?.data) ? resp.data : (resp?.data?.markets || []);
                if (rows.length > 0) {
                    markets = rows;
                    break;
                }
            }
            if (!markets.length) {
                tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;color:var(--text-muted);padding:20px;"><i class="fas fa-chart-pie" style="margin-right:6px;"></i>No markets created yet</td></tr>';
                return;
            }
            tbody.innerHTML = markets.map(m => {
                const volumeValue = Number(m.total_volume || m.volume || 0);
                const closeDate = m.close_date || (m.close_slot ? `Slot ${formatNumber(m.close_slot)}` : '—');
                return `<tr>
                    <td style="max-width:200px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;" title="${escapeHtml(m.question)}">${escapeHtml(m.question.slice(0, 60))}</td>
                    <td>${escapeHtml(m.category || '—')}</td>
                    <td><span class="status-badge status-${escapeHtml(m.status || 'active')}">${escapeHtml(m.status || 'Active')}</span></td>
                    <td>${formatVolume(volumeValue)}</td>
                    <td>${m.unique_traders || 0}</td>
                    <td>${closeDate}</td>
                </tr>`;
            }).join('');
        } catch { tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;color:var(--text-muted);padding:20px;">Failed to load markets</td></tr>'; }
    }

    function formatPredictCloseLabel(closeSlot, currentSlot) {
        const close = Number(closeSlot || 0);
        if (!close) return '';
        const now = Number(currentSlot || getRealtimePredictSlot(predictState.stats?.current_slot || 0) || 0);
        if (!now) return `Slot ${formatNumber(close)}`;
        const remainingSlots = close - now;
        if (remainingSlots <= 0) return 'Closed';
        // DEX-09: Lichen slot time is 400ms (0.4s), not 500ms
        const totalSeconds = Math.floor(remainingSlots * 0.4);
        const days = Math.floor(totalSeconds / 86400);
        const hours = Math.floor((totalSeconds % 86400) / 3600);
        const minutes = Math.floor((totalSeconds % 3600) / 60);
        const seconds = totalSeconds % 60;
        if (days > 0) return `${days}d ${hours}h left`;
        if (hours > 0) return `${hours}h ${minutes}m left`;
        if (minutes > 0) return `${minutes}m ${seconds}s left`;
        return `${Math.max(0, seconds)}s left`;
    }

    // ─── Render market cards dynamically ────────────────────────
    function renderPredictionMarkets() {
        const grid = document.querySelector('#predictMarketGrid') || document.querySelector('.predict-markets-section');
        if (!grid) return;

        // Keep only the grid container, regenerate cards
        // Remove all previously rendered cards AND empty-state placeholders
        grid.querySelectorAll('.market-card, .predict-empty-state').forEach(c => c.remove());

        if (!predictState.markets.length) {
            const emptyEl = document.createElement('div');
            emptyEl.className = 'predict-empty-state';
            emptyEl.style.cssText = 'text-align:center;color:var(--text-muted);padding:40px;font-size:0.9rem;';
            const hasError = !!predictState.lastMarketsError;
            emptyEl.innerHTML = hasError
                ? '<i class="fas fa-circle-exclamation" style="font-size:2rem;margin-bottom:12px;display:block;opacity:0.4;"></i><p>Unable to load prediction markets</p><p style="font-size:0.8rem;margin-top:8px;">Retrying automatically...</p>'
                : '<i class="fas fa-chart-line" style="font-size:2rem;margin-bottom:12px;display:block;opacity:0.4;"></i><p>No prediction markets yet</p><p style="font-size:0.8rem;margin-top:8px;">Create a market to get started</p>';
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
                        <div class="outcome-label"><span class="outcome-dot ${multiDotClasses[i % 4]}"></span><span>${escapeHtml(o.name)}</span></div>
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
                    </div>
                    <div class="outcome-row resolved-actions" style="margin-top:8px;display:flex;gap:8px;justify-content:center;">
                        <button class="btn btn-small btn-predict-claim" data-market="${m.id}" style="background:var(--accent);color:#fff;"><i class="fas fa-gift"></i> Claim Winnings</button>
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
                        <button class="btn btn-small btn-predict-buy-no" data-outcome="no" data-market="${m.id}">Buy</button>
                    </div>`;
            }

            // F11.8 FIX: Handle all market statuses with appropriate badges
            const statusMap = {
                active: { cls: 'active', label: 'Active' },
                pending: { cls: 'pending', label: 'Pending' },
                closed: { cls: 'closed', label: 'Closed' },
                resolving: { cls: 'resolving', label: 'Resolving' },
                resolved: { cls: 'resolved', label: 'Resolved' },
                disputed: { cls: 'disputed', label: 'Disputed' },
                voided: { cls: 'voided', label: 'Voided' },
            };
            const statusInfo = statusMap[m.status] || { cls: 'active', label: m.status || 'Active' };
            if (isResolved) { statusInfo.cls = 'resolved'; statusInfo.label = 'Resolved'; }
            const statusClass = escapeHtml(statusInfo.cls);
            const statusLabel = escapeHtml(statusInfo.label);
            const catTag = catIconsHtml[m.cat] || '<i class="fas fa-chart-pie"></i> ' + escapeHtml(m.cat || 'Other');
            const idTag = escapeHtml(m.pm_id || `#PM-${String(m.id).padStart(3, '0')}`);
            const closesText = formatPredictCloseLabel(m.closes, getRealtimePredictSlot(m.current_slot || predictState.stats?.current_slot || 0));
            const closesLabel = closesText ? `<span><i class="fas fa-clock"></i> ${escapeHtml(closesText)}</span>` : '';
            const creatorLabel = m.creator ? `<span><i class="fas fa-user"></i> Creator: ${escapeHtml(formatAddressShort(m.creator))}</span>` : '';
            const volLabel = formatVolume(m.volume);
            const liqLabel = formatVolume(m.liquidity);

            // AUDIT-FIX F10.5: Show resolve button if user is creator and market is active
            const creatorRaw = String(m.creator || '').toLowerCase();
            const walletRaw = String(wallet.address || '').toLowerCase();
            const walletHex = walletAddressToHex(wallet.address);
            const isCreator = !!m.creator && !!wallet.address && (creatorRaw === walletRaw || (walletHex && creatorRaw === walletHex));
            const resolveBtn = (!isResolved && isCreator) ? `<button class="btn btn-small btn-predict-resolve" data-market="${m.id}" style="background:var(--warning,#ffd166);color:#000;margin-left:8px;" title="Resolve this market"><i class="fas fa-gavel"></i> Resolve</button>` : '';

            // Task 8.1: Challenge/Finalize buttons for resolving/disputed markets
            let disputeHtml = '';
            if (m.status === 'resolving') {
                // Dispute window countdown
                const disputeEndSlot = m.dispute_end_slot || m.disputeEndSlot || 0;
                const currentSlot = m.current_slot || m.currentSlot || 0;
                const slotsRemaining = disputeEndSlot > currentSlot ? disputeEndSlot - currentSlot : 0;
                const secondsRemaining = slotsRemaining * 0.4; // DEX-09: 400ms per slot
                const hoursRemaining = Math.floor(secondsRemaining / 3600);
                const minutesRemaining = Math.floor((secondsRemaining % 3600) / 60);
                const disputeExpired = slotsRemaining <= 0;
                const resolverAddr = m.resolver ? escapeHtml(m.resolver.slice(0, 8) + '...' + m.resolver.slice(-6)) : 'Unknown';
                const outcomeLabel = m.winning_outcome !== undefined ? (m.winning_outcome === 0 ? 'YES' : 'NO') : '—';
                disputeHtml = `<div class="dispute-panel" data-market="${m.id}">
                    <div class="dispute-info">
                        <span class="dispute-label">Resolution: <strong>${outcomeLabel}</strong> by ${resolverAddr}</span>
                        <span class="dispute-countdown ${disputeExpired ? 'expired' : ''}">${disputeExpired ? 'Dispute period ended' : `<i class="fas fa-hourglass-half"></i> ${hoursRemaining}h ${minutesRemaining}m remaining`}</span>
                    </div>
                    <div class="dispute-actions">
                        ${disputeExpired
                        ? `<button class="btn btn-small btn-predict-finalize" data-market="${m.id}" title="Finalize resolution"><i class="fas fa-check-circle"></i> Finalize</button>`
                        : `<button class="btn btn-small btn-predict-challenge" data-market="${m.id}" title="Challenge this resolution"><i class="fas fa-exclamation-triangle"></i> Challenge</button>`
                    }
                    </div>
                </div>`;
            } else if (m.status === 'disputed') {
                disputeHtml = `<div class="dispute-panel disputed-state" data-market="${m.id}">
                    <div class="dispute-info">
                        <span class="dispute-label"><i class="fas fa-exclamation-circle"></i> Market disputed — awaiting DAO resolution</span>
                    </div>
                </div>`;
            }

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
                    <h4 class="market-question">${escapeHtml(m.question)}</h4>
                    <div class="market-meta">
                        ${closesLabel}${creatorLabel}
                    </div>
                </div>
                <div class="market-outcomes">${outcomesHtml}</div>
                ${disputeHtml}
                <div class="market-footer">
                    <div class="market-stats-mini">
                        <span><i class="fas fa-exchange-alt"></i> ${volLabel} vol</span>
                        <span><i class="fas fa-lock"></i> ${liqLabel} liq</span>
                        <span><i class="fas fa-users"></i> ${m.traders || 0} traders</span>
                        <button class="btn-predict-chart" data-market="${m.id}" title="Price Chart"><i class="fas fa-chart-line"></i></button>
                        ${resolveBtn}
                    </div>
                </div>
            `;
            grid.appendChild(card);
        });

        // Re-bind event handlers for new cards
        bindPredictionCardEvents();
        // Re-apply wallet gating on dynamically rendered prediction buttons
        applyWalletGateAll();

        // Re-apply active category filter so websocket refreshes don't reset it
        applyPredictCategoryFilter();

        // Apply default selection highlight
        const selCard = document.querySelector(`.market-card[data-market-id="${predictState.selectedMarket}"]`);
        if (selCard) selCard.classList.add('selected');
    }

    // ─── Render user positions in bottom panel ──────────────────
    function renderPredictionPositions() {
        const tbody = document.getElementById('predictPositionsBody');
        if (!tbody) return;
        if (!predictState.positions.length) {
            tbody.innerHTML = '<tr><td colspan="8" style="text-align:center;color:var(--text-muted)">No positions found</td></tr>';
            return;
        }
        // AUDIT-FIX F10.5: Show claim button for positions in resolved markets
        tbody.innerHTML = predictState.positions.map(p => {
            const m = predictState.markets.find(x => x.id === p.market_id);
            const qText = escapeHtml(m?.question?.slice(0, 40) || `Market #${p.market_id}`);
            const isResolved = m?.status === 'resolved';
            const won = isResolved && ((m.resolved_outcome === 'yes' && p.outcome === 0) || (m.resolved_outcome === 'no' && p.outcome === 1));
            const shares = Number(p.shares || 0);
            const costBasis = Number(p.cost_basis || 0);
            const avgPrice = shares > 0 ? (costBasis / shares) : 0;
            const currentPrice = p.outcome === 0
                ? Number(m?.yes || 0)
                : (1 - Number(m?.yes || 0));
            const value = shares * currentPrice;
            const pnl = value - costBasis;
            const pnlCls = pnl >= 0 ? 'positive' : 'negative';
            const pnlSign = pnl >= 0 ? '+' : '';
            const actionCol = isResolved
                ? (won ? `<button class="btn btn-small btn-predict-claim-pos" data-market="${p.market_id}" style="background:var(--accent);color:#fff;font-size:0.75rem;"><i class="fas fa-gift"></i> Claim</button>` : '<span style="color:var(--text-muted)">Lost</span>')
                : '<span style="color:var(--text-muted)">Active</span>';
            return `<tr>
                <td>${qText}</td>
                <td>${p.outcome === 0 ? 'YES' : 'NO'}</td>
                <td class="mono-value">${shares.toFixed(2)}</td>
                <td class="mono-value">$${avgPrice.toFixed(4)}</td>
                <td class="mono-value">$${currentPrice.toFixed(4)}</td>
                <td class="mono-value ${pnlCls}">${pnlSign}$${Math.abs(pnl).toFixed(2)}</td>
                <td class="mono-value">$${value.toFixed(2)}</td>
                <td>${actionCol}</td>
            </tr>`;
        }).join('');

        // Bind claim buttons in positions table
        tbody.querySelectorAll('.btn-predict-claim-pos').forEach(btn => btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
            if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }
            const mid = parseInt(btn.dataset.market);
            btn.disabled = true; btn.textContent = 'Claiming...';
            try {
                const posData = predictState.positions?.find(p => p.market_id === mid);
                const outcomeIdx = posData ? posData.outcome : 0;
                await wallet.sendTransaction([contractIx(contracts.prediction_market, buildRedeemSharesArgs(wallet.address, mid, outcomeIdx))]);
                showNotification('Prediction winnings claimed!', 'success');
            } catch (err) { showNotification(`Claim failed: ${err.message}`, 'error'); }
            btn.disabled = false; btn.innerHTML = '<i class="fas fa-gift"></i> Claim';
        }));
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
        document.querySelectorAll('.btn-predict-buy, .btn-predict-buy-no').forEach(btn => btn.addEventListener('click', () => {
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

        // AUDIT-FIX F10.5: Resolve market button (creator only)
        document.querySelectorAll('.btn-predict-resolve').forEach(btn => btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
            if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }
            const mid = parseInt(btn.dataset.market);
            const m = predictState.markets.find(x => x.id === mid);
            if (!m) return;
            // Prompt for resolution outcome
            const outcome = prompt(`Resolve "${m.question}"?\n\nEnter the winning outcome: "yes" or "no"`);
            if (!outcome || !['yes', 'no'].includes(outcome.toLowerCase())) { showNotification('Invalid outcome — enter "yes" or "no"', 'warning'); return; }
            btn.disabled = true; btn.textContent = 'Resolving...';
            try {
                const winIdx = outcome.toLowerCase() === 'yes' ? 0 : 1;
                await wallet.sendTransaction([contractIx(contracts.prediction_market, buildResolveMarketArgs(wallet.address, mid, winIdx), PREDICT_DISPUTE_BOND)]);
                showNotification(`Market resolved: ${outcome.toUpperCase()} wins`, 'success');
                await loadPredictionMarkets();
            } catch (err) { showNotification(`Resolve failed: ${err.message}`, 'error'); }
            btn.disabled = false; btn.innerHTML = '<i class="fas fa-gavel"></i> Resolve';
        }));

        // AUDIT-FIX F10.5: Claim winnings on resolved markets
        // F12.7 FIX: Use position's actual outcome, not default 0
        document.querySelectorAll('.btn-predict-claim').forEach(btn => btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
            if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }
            const mid = parseInt(btn.dataset.market);
            btn.disabled = true; btn.textContent = 'Claiming...';
            try {
                const cardPos = predictState.positions?.find(p => p.market_id === mid);
                if (!cardPos) {
                    showNotification('No position found for this market', 'warning');
                    btn.disabled = false; btn.innerHTML = '<i class="fas fa-gift"></i> Claim Winnings';
                    return;
                }
                await wallet.sendTransaction([contractIx(contracts.prediction_market, buildRedeemSharesArgs(wallet.address, mid, cardPos.outcome))]);
                showNotification('Prediction winnings claimed!', 'success');
            } catch (err) { showNotification(`Claim failed: ${err.message}`, 'error'); }
            btn.disabled = false; btn.innerHTML = '<i class="fas fa-gift"></i> Claim Winnings';
        }));

        // Task 8.1: Challenge resolution button handler
        document.querySelectorAll('.btn-predict-challenge').forEach(btn => btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
            if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }
            const mid = parseInt(btn.dataset.market);
            const m = predictState.markets.find(x => x.id === mid);
            if (!m) return;
            const evidence = prompt(`Challenge resolution of "${m.question}"?\n\nThis requires a bond of 100 lUSD.\n\nProvide evidence or reason for challenge:`);
            if (!evidence) { showNotification('Challenge cancelled', 'info'); return; }
            btn.disabled = true; btn.textContent = 'Challenging...';
            try {
                await wallet.sendTransaction([contractIx(contracts.prediction_market, buildChallengeResolutionArgs(wallet.address, mid, evidence), PREDICT_DISPUTE_BOND)]);
                showNotification('Resolution challenged! Awaiting DAO review.', 'success');
                await loadPredictionMarkets();
            } catch (err) { showNotification(`Challenge failed: ${err.message}`, 'error'); }
            btn.disabled = false; btn.innerHTML = '<i class="fas fa-exclamation-triangle"></i> Challenge';
        }));

        // Task 8.1: Finalize resolution button handler
        document.querySelectorAll('.btn-predict-finalize').forEach(btn => btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
            if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }
            const mid = parseInt(btn.dataset.market);
            btn.disabled = true; btn.textContent = 'Finalizing...';
            try {
                await wallet.sendTransaction([contractIx(contracts.prediction_market, buildFinalizeResolutionArgs(wallet.address, mid))]);
                showNotification('Market resolution finalized!', 'success');
                await loadPredictionMarkets();
            } catch (err) { showNotification(`Finalize failed: ${err.message}`, 'error'); }
            btn.disabled = false; btn.innerHTML = '<i class="fas fa-check-circle"></i> Finalize';
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

    // F11.6 FIX: Time range filtering helper
    function filterByRange(data, range) {
        if (!data || data.length === 0) return data;
        const now = Date.now();
        const rangeMs = { '1h': 3600e3, '6h': 21600e3, '1d': 86400e3, '1w': 604800e3, '30d': 2592000e3 };
        const cutoff = rangeMs[range];
        if (!cutoff) return data; // 'all' or unknown → return full dataset
        return data.filter(d => d.t >= now - cutoff);
    }

    // Time range tab clicks
    document.querySelectorAll('.predict-chart-tab').forEach(tab => tab.addEventListener('click', () => {
        const range = tab.dataset.range;
        predictChartState.range = range;
        document.querySelectorAll('.predict-chart-tab').forEach(t => t.classList.toggle('active', t === tab));
        const m = predictState.markets.find(x => x.id === predictChartState.marketId);
        if (!m) return;
        // F11.6 FIX: Filter real data by selected time range
        const raw = (predictChartState.realData && predictChartState.realData.length > 0) ? predictChartState.realData : generateEmptyPriceHistory(m);
        const chartData = filterByRange(raw, range);
        const canvas = document.getElementById('predictChartCanvas');
        if (canvas) drawPredictChart(chartData.length > 0 ? chartData : raw, canvas);
        renderPredictChartStats(chartData.length > 0 ? chartData : raw, m);
    }));

    // Close handlers
    document.getElementById('predictChartClose')?.addEventListener('click', closePredictChart);
    document.querySelector('.predict-chart-overlay')?.addEventListener('click', closePredictChart);
    document.addEventListener('keydown', e => { if (e.key === 'Escape') closePredictChart(); });

    // Category filter — persist selection in predictState so it survives
    // websocket-driven re-renders of the market grid
    function applyPredictCategoryFilter() {
        const cat = predictState.activeCategory || 'all';
        document.querySelectorAll('.predict-cat-btn').forEach(b => {
            b.classList.toggle('active', b.dataset.cat === cat);
        });
        document.querySelectorAll('.market-card').forEach(card => {
            if (cat === 'all' || card.dataset.cat === cat) card.style.display = '';
            else card.style.display = 'none';
        });
    }
    document.querySelectorAll('.predict-cat-btn').forEach(btn => btn.addEventListener('click', () => {
        predictState.activeCategory = btn.dataset.cat || 'all';
        applyPredictCategoryFilter();
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

    // F12.2 FIX: CPMM pricing formula matching contract's calculate_buy
    function updatePredictCalc() {
        const amt = parseFloat(document.getElementById('predictAmount')?.value) || 0;
        const m = predictState.markets.find(x => x.id === predictState.selectedMarket);
        if (!m) return;
        const outcomeIdx = predictState.selectedOutcome === 'yes' ? 0 : 1;

        // Contract CPMM: mint complete sets (1:1) + swap non-desired shares into pool
        // For binary: shares_per_set = amount, a_received = reserve_a * b_sold / (reserve_b + b_sold)
        // Fee applied to swap portion only (2% = 200 bps)
        let shares = 0, fee = 0;
        if (m.outcomes && m.outcomes.length === 2) {
            const selfReserve = m.outcomes[outcomeIdx]?.pool_yes || 0;
            const otherReserve = m.outcomes[1 - outcomeIdx]?.pool_yes || 0;
            if (selfReserve > 0 && otherReserve > 0) {
                const bSold = amt; // shares minted = amount (1:1)
                const aFromSwap = (selfReserve * bSold) / (otherReserve + bSold);
                const totalShares = amt + aFromSwap;
                const feeShares = aFromSwap * 0.02; // 2% on swap portion
                shares = totalShares - feeShares;
                fee = feeShares;
            } else {
                // No liquidity — estimate linearly
                const price = predictState.selectedOutcome === 'yes' ? m.yes : (1 - m.yes);
                fee = amt * 0.02;
                shares = price > 0 ? (amt - fee) / price : 0;
            }
        } else {
            // Multi-outcome fallback — simple linear
            const price = m.outcomes?.[outcomeIdx]?.price || 0.5;
            fee = amt * 0.02;
            shares = price > 0 ? (amt - fee) / price : 0;
        }
        const payout = shares; // each share worth $1.00 if winner

        const se = document.getElementById('predictShares'), ae = document.getElementById('predictAvgPrice'), pe = document.getElementById('predictPayout'), fe = document.getElementById('predictFee');
        if (se) se.textContent = shares.toFixed(2);
        if (ae) ae.textContent = shares > 0 ? `$${(amt / shares).toFixed(4)}` : '$0.00';
        if (pe) pe.textContent = `$${payout.toFixed(2)}`;
        if (fe) fe.textContent = `$${fee.toFixed(2)}`;
    }

    // Submit trade
    const predictSubmitBtn = document.getElementById('predictSubmitBtn');
    if (predictSubmitBtn) predictSubmitBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet to trade', 'warning'); return; }
        if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }
        const amt = parseFloat(document.getElementById('predictAmount')?.value) || 0;
        if (amt < 1) { showNotification('Enter amount (min $1)', 'warning'); return; }
        if (amt > 9_000_000) { showNotification('Amount too large (max 9M)', 'warning'); return; }
        const m = predictState.markets.find(x => x.id === predictState.selectedMarket);
        if (!m) return;
        // F20.5: Check market is still active before submitting buy transaction
        if (m.status && m.status !== 'active') { showNotification('Market is no longer active', 'warning'); return; }
        predictSubmitBtn.disabled = true; predictSubmitBtn.textContent = 'Submitting...';
        try {
            // AUDIT-FIX F10.4: Prediction trade via signed sendTransaction (not unsigned REST)
            const outcomeVal = predictState.selectedOutcome === 'yes' ? 0 : 1;
            // F12.1 FIX: Contract uses LUSD_UNIT (1e6), not PRICE_SCALE (1e9)
            const tradeAmountMicros = Math.round(amt * 1e6);
            await wallet.sendTransaction([contractIx(contracts.prediction_market, buildBuySharesArgs(wallet.address, m.id, outcomeVal, tradeAmountMicros), tradeAmountMicros)]);
            showNotification(`Bought ${predictState.selectedOutcome.toUpperCase()} on "${escapeHtml(m.question.slice(0, 40))}..." for $${amt.toFixed(2)}`, 'success');
            // F24.7 FIX: Refresh prediction data after buy
            loadPredictionMarkets().catch(() => { }); loadPredictionPositions().catch(() => { });
        } catch (e) { showNotification(`Trade failed: ${e.message}`, 'error'); }
        predictSubmitBtn.disabled = false;
        const side = predictState.selectedOutcome === 'yes' ? 'YES' : 'NO';
        predictSubmitBtn.innerHTML = `<i class="fas fa-bolt"></i> Buy ${side} Shares`;
        if (document.getElementById('predictAmount')) document.getElementById('predictAmount').value = '';
        updatePredictCalc();
    });

    // F12.6 FIX: Set close date min to today to prevent past dates
    const closeDateEl = document.getElementById('predictCloseDate');
    if (closeDateEl) {
        const today = new Date().toISOString().split('T')[0];
        closeDateEl.setAttribute('min', today);
    }

    // Create market
    const predictCreateBtn = document.getElementById('predictCreateBtn');
    if (predictCreateBtn) predictCreateBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet to create', 'warning'); return; }
        if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }
        const q = document.getElementById('predictQuestion')?.value?.trim();
        if (!q) { showNotification('Enter market question', 'warning'); return; }
        const liq = parseFloat(document.getElementById('predictInitLiq')?.value) || 0;
        if (liq < 100) { showNotification('Min 100 lUSD initial liquidity', 'warning'); return; }

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
            // AUDIT-FIX F10.4 + F11.2: Create market via signed sendTransaction with valid close_slot
            const catVal = document.getElementById('predictCategory')?.value || 'crypto';
            const ocCount = outcomes.length > 0 ? outcomes.length : 2;
            // F11.2 FIX: Compute close_slot from date input or default 7 days
            // F12.6 FIX: Validate close date is in the future
            const closeDateInput = document.getElementById('predictCloseDate')?.value;
            let durationSlots = 7 * 24 * 60 * 60 * 2.5; // default 7 days at 0.4s/slot = 1_512_000
            if (closeDateInput) {
                const closeMs = new Date(closeDateInput).getTime();
                const nowMs = Date.now();
                if (closeMs <= nowMs) {
                    showNotification('Close date must be in the future', 'warning');
                    predictCreateBtn.disabled = false; predictCreateBtn.textContent = 'Create Market';
                    return;
                }
                durationSlots = Math.round((closeMs - nowMs) / 400); // DEX-09: 400ms per slot
            }
            // Fetch current slot from stats to compute absolute close_slot
            let currentSlot = 0;
            try {
                const statsResp = await api.get('/prediction-market/stats');
                currentSlot = statsResp?.data?.current_slot || 0;
            } catch { /* will use fallback */ }
            // If we couldn't get current slot, use a large estimate
            if (!currentSlot) currentSlot = Math.round(Date.now() / 400); // F16.9: 400ms per slot
            const closeSlot = currentSlot + durationSlots;
            const predictCreateFee = 10_000_000; // 10 lUSD, matches prediction contract MARKET_CREATION_FEE
            const createIx = contractIx(contracts.prediction_market, await buildCreateMarketArgs(wallet.address, q, catVal, ocCount, closeSlot), predictCreateFee);
            await wallet.sendTransaction([createIx]);

            const findCreatedMarketId = async () => {
                for (let attempt = 0; attempt < 8; attempt++) {
                    try {
                        const resp = await api.get(`/prediction-market/markets?creator=${encodeURIComponent(wallet.address)}&limit=100`);
                        const rows = Array.isArray(resp?.data?.markets) ? resp.data.markets : (Array.isArray(resp?.markets) ? resp.markets : []);
                        if (rows.length > 0) {
                            const exact = rows
                                .filter(mm => String(mm.question || '').trim() === q && Number(mm.close_slot || 0) === closeSlot)
                                .sort((a, b) => Number(b.id || 0) - Number(a.id || 0));
                            if (exact[0]?.id) return Number(exact[0].id);
                            const byQuestion = rows
                                .filter(mm => String(mm.question || '').trim() === q)
                                .sort((a, b) => Number(b.id || 0) - Number(a.id || 0));
                            if (byQuestion[0]?.id) return Number(byQuestion[0].id);
                        }
                    } catch { /* retry */ }
                    await new Promise(resolve => setTimeout(resolve, 400 + attempt * 200));
                }
                return null;
            };

            const createdMarketId = await findCreatedMarketId();
            const initialLiquidityMicros = Math.round(liq * 1e6);
            if (createdMarketId) {
                const liqIx = contractIx(contracts.prediction_market, buildAddInitialLiquidityArgs(wallet.address, createdMarketId, initialLiquidityMicros), initialLiquidityMicros);
                await wallet.sendTransaction([liqIx]);
                showNotification(`Market created: "${escapeHtml(q.slice(0, 50))}..." with ${liq} lUSD liquidity`, 'success');
            } else {
                showNotification('Market created, but liquidity step is pending (could not resolve market id yet)', 'warning');
            }
            await loadPredictionMarkets();
        } catch (e) { showNotification(`Create failed: ${e.message}`, 'error'); }
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
        // F11.5 FIX: Add "ending" sort by close_slot (soonest first)
        else if (sortBy === 'ending') predictState.markets.sort((a, b) => (a.closes || Infinity) - (b.closes || Infinity));
        // F11.5 FIX: Add "traders" sort by unique trader count
        else if (sortBy === 'traders') predictState.markets.sort((a, b) => b.traders - a.traders);
        renderPredictionMarkets();
        showNotification(`Sorted by ${predictSort.options[predictSort.selectedIndex].text}`, 'info');
    });

    // ═══════════════════════════════════════════════════════════════════════
    // Governance + Rewards — claim handlers
    // ═══════════════════════════════════════════════════════════════════════
    const claimTradingButtons = [
        document.getElementById('claimAllBtn'),
        document.getElementById('claimTradingBtn'),
    ].filter(Boolean);

    claimTradingButtons.forEach(btn => btn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet to claim', 'warning'); return; }
        if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }
        if (!contracts.dex_rewards) { showNotification('Rewards contract not loaded', 'error'); return; }
        btn.disabled = true;
        const origText = btn.innerHTML;
        btn.textContent = 'Claiming...';
        try {
            await wallet.sendTransaction([contractIx(contracts.dex_rewards, buildClaimRewardsArgs(wallet.address))]);
            showNotification('Rewards claimed successfully!', 'success');
            loadRewardsStats().catch(() => { });
        } catch (e) {
            showNotification(`Claim failed: ${e.message}`, 'error');
        }
        btn.disabled = false;
        btn.innerHTML = origText;
    }));

    const claimLpBtn = document.getElementById('claimLpBtn');
    if (claimLpBtn) claimLpBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet to claim LP rewards', 'warning'); return; }
        if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }
        if (!contracts.dex_rewards) { showNotification('Rewards contract not loaded', 'error'); return; }

        claimLpBtn.disabled = true;
        const origText = claimLpBtn.innerHTML;
        claimLpBtn.textContent = 'Claiming...';
        try {
            const { data } = await api.get(`/pools/positions?owner=${wallet.address}`);
            const positions = Array.isArray(data) ? data : [];
            const instructions = positions
                .map(pos => Number(pos.positionId || pos.id || 0))
                .filter(positionId => Number.isFinite(positionId) && positionId > 0)
                .map(positionId => contractIx(contracts.dex_rewards, buildClaimLpRewardsArgs(wallet.address, positionId)));

            if (!instructions.length) {
                showNotification('No LP positions found to claim', 'warning');
                return;
            }

            await wallet.sendTransaction(instructions);
            showNotification(`LP rewards claimed for ${instructions.length} position(s)`, 'success');
            loadRewardsStats().catch(() => { });
        } catch (e) {
            showNotification(`LP claim failed: ${e.message}`, 'error');
        }
        claimLpBtn.disabled = false;
        claimLpBtn.innerHTML = origText;
    });

    const marginLiquidateBtn = document.getElementById('marginLiquidateBtn');
    if (marginLiquidateBtn) marginLiquidateBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
        if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }

        const raw = window.prompt('Enter position ID to liquidate:');
        if (raw === null) return;
        const positionId = parseInt(raw.trim(), 10);
        if (!Number.isFinite(positionId) || positionId <= 0) {
            showNotification('Invalid position ID', 'warning');
            return;
        }

        marginLiquidateBtn.disabled = true;
        const origText = marginLiquidateBtn.textContent;
        marginLiquidateBtn.textContent = 'Liquidating...';
        try {
            await submitMarginLiquidation(positionId);
            showNotification(`Liquidation submitted for position #${positionId}`, 'success');
        } catch (e) {
            showNotification(`Liquidation failed: ${e.message}`, 'error');
        }
        marginLiquidateBtn.disabled = false;
        marginLiquidateBtn.textContent = origText || 'Liquidate';
    });

    const copyBtn = document.querySelector('.copy-btn');
    if (copyBtn) copyBtn.addEventListener('click', () => { const c = document.querySelector('.referral-link-box code'); if (c) navigator.clipboard.writeText(c.textContent).then(() => showNotification('Referral link copied!', 'success')); });

    // ═══════════════════════════════════════════════════════════════════════
    // SporePump Launchpad — Full token launch + bonding curve UI
    // ═══════════════════════════════════════════════════════════════════════

    const launchState = { tokens: [], selectedToken: null, tradeMode: 'buy', quoteTimer: null };

    function renderLaunchpadEmptyState(grid, title, subtitle = '') {
        if (!grid) return;
        grid.innerHTML = `<div class="launch-empty-state" style="display:flex;align-items:center;justify-content:center;min-height:220px;width:100%;text-align:center;color:var(--text-muted);padding:40px;font-size:0.9rem;"><div><i class="fas fa-rocket" style="font-size:2rem;margin-bottom:12px;display:block;opacity:0.4;"></i><p>${title}</p>${subtitle ? `<p style="font-size:0.8rem;margin-top:8px;">${subtitle}</p>` : ''}</div></div>`;
    }

    async function loadLaunchpadStats() {
        try {
            const { data } = await api.get('/launchpad/stats');
            if (data) {
                const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
                el('launchTokenCount', data.token_count || 0);
                el('launchTotalRaised', formatVolume(parseFloat(data.fees_collected || 0) * 0.10)); // rough USD estimate
                el('launchGraduated', data.total_graduated || 0);
                el('launchFees', formatAmount(data.fees_collected || 0) + ' LICN');
            }
        } catch { /* API unavailable */ }
    }

    async function loadLaunchpadTokens() {
        const grid = document.getElementById('launchTokenGrid');
        if (!grid) return;
        try {
            const activeFilter = document.querySelector('.filter-pill[data-lfilter].active');
            const filter = activeFilter?.dataset.lfilter || 'active';
            const sort = document.getElementById('launchSortSelect')?.value || 'newest';
            const { data } = await api.get(`/launchpad/tokens?filter=${filter}&sort=${sort}&limit=50`);
            if (data?.tokens?.length) {
                launchState.tokens = data.tokens;
                renderLaunchpadTokens();
                return;
            }
        } catch { /* API unavailable */ }
        // Empty state
        launchState.tokens = [];
        renderLaunchpadEmptyState(grid, 'No tokens launched yet', 'Be the first to launch a token!');
    }

    function renderLaunchpadTokens() {
        const grid = document.getElementById('launchTokenGrid');
        if (!grid) return;
        if (!launchState.tokens.length) {
            renderLaunchpadEmptyState(grid, 'No tokens found');
            return;
        }

        grid.innerHTML = launchState.tokens.map(t => {
            const gradPct = (t.graduation_pct || 0).toFixed(1);
            const isGrad = t.graduated;
            const priceStr = formatPrice(t.current_price || 0);
            const raisedStr = formatAmount(t.licn_raised || 0);
            const mcapStr = formatAmount(t.market_cap || 0);
            const creatorShort = t.creator ? (t.creator.slice(0, 6) + '...' + t.creator.slice(-4)) : '—';
            const selectedClass = launchState.selectedToken === t.id ? 'selected' : '';
            return `<div class="launch-token-card ${selectedClass}" data-token-id="${t.id}">
                <div class="ltc-header">
                    <span class="ltc-name"><i class="fas fa-coins"></i> Token #${t.id}</span>
                    <span class="ltc-badge ${isGrad ? 'graduated' : 'active'}">${isGrad ? 'Graduated' : 'Active'}</span>
                </div>
                <div class="ltc-creator"><i class="fas fa-user"></i> ${escapeHtml(creatorShort)}</div>
                <div class="ltc-stats">
                    <div class="ltc-stat"><span class="ltc-stat-label">Price</span><span class="ltc-stat-value mono-value">${priceStr} LICN</span></div>
                    <div class="ltc-stat"><span class="ltc-stat-label">Raised</span><span class="ltc-stat-value mono-value">${raisedStr} LICN</span></div>
                    <div class="ltc-stat"><span class="ltc-stat-label">MktCap</span><span class="ltc-stat-value mono-value">${mcapStr} LICN</span></div>
                </div>
                <div class="ltc-grad-bar">
                    <div class="ltc-grad-track"><div class="ltc-grad-fill" style="width:${gradPct}%"></div></div>
                    <span class="ltc-grad-label">${gradPct}% to graduation</span>
                </div>
                ${!isGrad ? `<div class="ltc-actions">
                    <button class="btn btn-small btn-buy launch-quick-buy" data-token-id="${t.id}"><i class="fas fa-arrow-up"></i> Buy</button>
                    <button class="btn btn-small btn-sell launch-quick-sell" data-token-id="${t.id}"><i class="fas fa-arrow-down"></i> Sell</button>
                </div>` : '<div class="ltc-actions"><span style="color:var(--accent);font-size:0.8rem;"><i class="fas fa-exchange-alt"></i> Trade on DEX</span></div>'}
            </div>`;
        }).join('');

        bindLaunchTokenEvents();
        applyWalletGateAll();
    }

    function bindLaunchTokenEvents() {
        // Card click → select token
        document.querySelectorAll('.launch-token-card').forEach(card => {
            card.addEventListener('click', e => {
                if (e.target.closest('button')) return;
                selectLaunchToken(parseInt(card.dataset.tokenId));
            });
        });
        // Quick buy/sell buttons
        document.querySelectorAll('.launch-quick-buy').forEach(btn => btn.addEventListener('click', () => {
            if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
            selectLaunchToken(parseInt(btn.dataset.tokenId));
            setLaunchSide('buy');
        }));
        document.querySelectorAll('.launch-quick-sell').forEach(btn => btn.addEventListener('click', () => {
            if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
            selectLaunchToken(parseInt(btn.dataset.tokenId));
            setLaunchSide('sell');
        }));
    }

    function selectLaunchToken(id) {
        launchState.selectedToken = id;
        const t = launchState.tokens.find(x => x.id === id);
        // Update selection highlight
        document.querySelectorAll('.launch-token-card').forEach(c => c.classList.toggle('selected', parseInt(c.dataset.tokenId) === id));
        // Update sidebar
        const titleEl = document.getElementById('launchChartTitle');
        if (titleEl) titleEl.textContent = t ? `Token #${t.id} — Bonding Curve` : 'Bonding Curve';
        const labelEl = document.getElementById('launchSelectedLabel');
        if (labelEl) labelEl.innerHTML = t ? `<i class="fas fa-coins"></i> Token #${t.id} selected` : '<i class="fas fa-info-circle"></i> Select a token from the list';
        updateLaunchSidebar(t);
        if (t) drawBondingCurve(t);
        updateLaunchQuote();
        loadLaunchHoldings();
    }

    function updateLaunchSidebar(t) {
        const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
        if (!t) {
            el('launchCurrentPrice', '—'); el('launchMarketCap', '—');
            el('launchLicnRaised', '—'); el('launchSupplySold', '—');
            el('launchGradPct', '0%');
            const fill = document.getElementById('launchGradFill');
            if (fill) fill.style.width = '0%';
            return;
        }
        el('launchCurrentPrice', formatPrice(t.current_price) + ' LICN');
        el('launchMarketCap', formatAmount(t.market_cap) + ' LICN');
        el('launchLicnRaised', formatAmount(t.licn_raised) + ' LICN');
        el('launchSupplySold', formatAmount(t.supply_sold));
        const gradPct = (t.graduation_pct || 0).toFixed(1);
        el('launchGradPct', gradPct + '%');
        const fill = document.getElementById('launchGradFill');
        if (fill) fill.style.width = gradPct + '%';
    }

    function drawBondingCurve(token) {
        const canvas = document.getElementById('bondingCurveCanvas');
        if (!canvas) return;
        const ctx = canvas.getContext('2d');
        const dpr = window.devicePixelRatio || 1;
        const W = canvas.clientWidth || 400;
        const H = canvas.clientHeight || 200;
        canvas.width = W * dpr;
        canvas.height = H * dpr;
        ctx.scale(dpr, dpr);
        ctx.clearRect(0, 0, W, H);

        // Bonding curve: price(s) = BASE_PRICE + s * SLOPE / SLOPE_SCALE
        // In LICN: price(s) = (1000 + s / 1e6) / 1e9
        const BASE = 1000;
        const supplySoldRaw = (token.supply_sold || 0) * 1e9; // convert back to raw
        // Plot from 0 to 2x current supply (or min 1M if zero)
        const maxPlotSupply = Math.max(supplySoldRaw * 2, 1e6);
        const points = 100;
        const pad = { top: 15, right: 55, bottom: 30, left: 12 };
        const cW = W - pad.left - pad.right;
        const cH = H - pad.top - pad.bottom;

        // Generate price points
        const data = [];
        for (let i = 0; i <= points; i++) {
            const s = (i / points) * maxPlotSupply;
            const p = (BASE + s / 1e6) / 1e9;
            data.push({ s, p });
        }

        const maxP = data[data.length - 1].p;
        const minP = data[0].p;
        const rangeP = maxP - minP || 1e-9;

        const xPos = (s) => pad.left + (s / maxPlotSupply) * cW;
        const yPos = (p) => pad.top + (1 - (p - minP) / rangeP) * cH;

        // Grid lines
        ctx.strokeStyle = 'rgba(255,255,255,0.05)';
        ctx.lineWidth = 1;
        for (let i = 0; i <= 4; i++) {
            const y = pad.top + (i / 4) * cH;
            ctx.beginPath(); ctx.moveTo(pad.left, y); ctx.lineTo(W - pad.right, y); ctx.stroke();
        }

        // Draw curve
        ctx.beginPath();
        ctx.strokeStyle = 'var(--accent, #4ea8de)';
        ctx.lineWidth = 2;
        data.forEach((d, i) => {
            const x = xPos(d.s), y = yPos(d.p);
            if (i === 0) ctx.moveTo(x, y);
            else ctx.lineTo(x, y);
        });
        ctx.stroke();

        // Fill area under curve
        ctx.lineTo(xPos(maxPlotSupply), yPos(minP));
        ctx.lineTo(xPos(0), yPos(minP));
        ctx.closePath();
        const grad = ctx.createLinearGradient(0, pad.top, 0, H - pad.bottom);
        grad.addColorStop(0, 'rgba(78,168,222,0.15)');
        grad.addColorStop(1, 'rgba(78,168,222,0.01)');
        ctx.fillStyle = grad;
        ctx.fill();

        // Current position marker
        if (supplySoldRaw > 0) {
            const cx = xPos(supplySoldRaw);
            const cy = yPos((BASE + supplySoldRaw / 1e6) / 1e9);
            ctx.beginPath();
            ctx.arc(cx, cy, 5, 0, Math.PI * 2);
            ctx.fillStyle = '#10b981';
            ctx.fill();
            ctx.strokeStyle = '#fff';
            ctx.lineWidth = 1.5;
            ctx.stroke();
            // Label
            ctx.fillStyle = '#10b981';
            ctx.font = '10px Inter, sans-serif';
            ctx.textAlign = 'center';
            ctx.fillText('You are here', cx, cy - 10);
        }

        // Y-axis labels
        ctx.fillStyle = 'rgba(255,255,255,0.4)';
        ctx.font = '9px JetBrains Mono, monospace';
        ctx.textAlign = 'left';
        for (let i = 0; i <= 4; i++) {
            const p = minP + (1 - i / 4) * rangeP;
            ctx.fillText(formatPrice(p), W - pad.right + 4, pad.top + (i / 4) * cH + 3);
        }

        // X-axis labels
        ctx.textAlign = 'center';
        ctx.fillText('0', pad.left, H - 8);
        ctx.fillText(formatAmount(maxPlotSupply / 1e9), W - pad.right, H - 8);
        ctx.fillText('Supply', pad.left + cW / 2, H - 8);
    }

    function setLaunchSide(side) {
        launchState.tradeMode = side;
        document.querySelectorAll('.launch-side-btn').forEach(b => b.classList.toggle('active', b.dataset.lside === side));
        const amtLabel = document.getElementById('launchAmountLabel');
        const amtUnit = document.getElementById('launchAmountUnit');
        const tradeBtn = document.getElementById('launchTradeBtn');
        if (side === 'buy') {
            if (amtLabel) amtLabel.textContent = 'Amount (LICN)';
            if (amtUnit) amtUnit.textContent = 'LICN';
            if (tradeBtn) { tradeBtn.innerHTML = '<i class="fas fa-bolt"></i> Buy Tokens'; tradeBtn.className = 'btn btn-full btn-buy'; }
        } else {
            if (amtLabel) amtLabel.textContent = 'Amount (Tokens)';
            if (amtUnit) amtUnit.textContent = 'TOKENS';
            if (tradeBtn) { tradeBtn.innerHTML = '<i class="fas fa-bolt"></i> Sell Tokens'; tradeBtn.className = 'btn btn-full btn-sell'; }
        }
        updateLaunchQuote();
    }

    function updateLaunchQuote() {
        const amt = parseFloat(document.getElementById('launchAmountInput')?.value) || 0;
        const t = launchState.tokens.find(x => x.id === launchState.selectedToken);
        const el = (id, v) => { const e = document.getElementById(id); if (e) e.textContent = v; };
        if (!t || !amt) {
            el('launchQuoteTokens', '—'); el('launchQuoteImpact', '—'); el('launchQuoteFee', '—');
            return;
        }
        if (launchState.tradeMode === 'buy') {
            // Client-side bonding curve estimate
            const licnSpores = amt * 1e9;
            const afterFee = licnSpores * 0.99;
            const supplyRaw = (t.supply_sold || 0) * 1e9;
            // Quadratic solve for tokens out (same as REST API)
            const aCoeff = 1 / (2 * 1e6);
            const bCoeff = 1000 + supplyRaw / 1e6;
            const disc = bCoeff * bCoeff + 4 * aCoeff * afterFee;
            const tokensOut = disc > 0 ? (-bCoeff + Math.sqrt(disc)) / (2 * aCoeff) : 0;
            const tokensF = tokensOut / 1e9;
            const priceBefore = (1000 + supplyRaw / 1e6) / 1e9;
            const priceAfter = (1000 + (supplyRaw + tokensOut) / 1e6) / 1e9;
            const impact = priceBefore > 0 ? ((priceAfter - priceBefore) / priceBefore * 100) : 0;
            el('launchQuoteTokens', formatAmount(tokensF) + ' tokens');
            el('launchQuoteImpact', impact.toFixed(2) + '%');
            el('launchQuoteFee', formatAmount(amt * 0.01) + ' LICN');
        } else {
            // Sell estimate
            const tokenSpores = amt * 1e9;
            const supplyRaw = (t.supply_sold || 0) * 1e9;
            if (tokenSpores > supplyRaw) { el('launchQuoteTokens', 'Exceeds supply'); el('launchQuoteImpact', '—'); el('launchQuoteFee', '—'); return; }
            // Sell refund: (BASE_PRICE * a + SLOPE * a * (2*s - a) / (2 * SLOPE_SCALE)) / norm
            const a = tokenSpores, s = supplyRaw;
            const refundRaw = (1000 * a + 1 * a * (2 * s - a) / (2 * 1e6)) / 1e9;
            const refundAfterFee = refundRaw * 0.99;
            const priceBefore = (1000 + s / 1e6) / 1e9;
            const priceAfter = (1000 + (s - a) / 1e6) / 1e9;
            const impact = priceBefore > 0 ? ((priceAfter - priceBefore) / priceBefore * 100) : 0;
            el('launchQuoteTokens', formatAmount(refundAfterFee) + ' LICN');
            el('launchQuoteImpact', impact.toFixed(2) + '%');
            el('launchQuoteFee', formatAmount(refundRaw * 0.01) + ' LICN');
        }
    }

    let launchHoldingsSeq = 0;
    async function loadLaunchHoldings() {
        const seq = ++launchHoldingsSeq;
        const list = document.getElementById('launchHoldingsList');
        if (!list) return;
        if (!state.connected || !wallet.address) {
            list.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-wallet" style="font-size:1.2rem;margin-bottom:8px;display:block;opacity:0.4;"></i>Connect wallet to view holdings</div>';
            return;
        }
        // Load balance for selected token (or all tokens)
        const tokensToCheck = launchState.selectedToken ? [launchState.selectedToken] : launchState.tokens.map(t => t.id);
        const holdings = [];
        for (const tid of tokensToCheck.slice(0, 20)) {
            if (seq !== launchHoldingsSeq) return; // stale — newer call superseded
            try {
                const { data } = await api.get(`/launchpad/tokens/${tid}/holders?address=${wallet.address}`);
                if (data && data.balance > 0) {
                    holdings.push({ id: tid, balance: data.balance });
                }
            } catch { /* skip */ }
        }
        if (seq !== launchHoldingsSeq) return; // stale
        if (!holdings.length) {
            list.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;">No holdings found</div>';
            return;
        }
        list.innerHTML = holdings.map(h => `<div class="launch-holding-row">
            <span><i class="fas fa-coins"></i> Token #${h.id}</span>
            <span class="mono-value">${formatAmount(h.balance)}</span>
        </div>`).join('');
    }

    // ── Launchpad event bindings ──
    // Side toggle
    document.querySelectorAll('.launch-side-btn').forEach(btn => btn.addEventListener('click', () => setLaunchSide(btn.dataset.lside)));

    // Filter pills
    document.querySelectorAll('.filter-pill[data-lfilter]').forEach(pill => pill.addEventListener('click', () => {
        document.querySelectorAll('.filter-pill[data-lfilter]').forEach(p => p.classList.remove('active'));
        pill.classList.add('active');
        loadLaunchpadTokens();
    }));

    // Sort select
    const launchSortSel = document.getElementById('launchSortSelect');
    if (launchSortSel) launchSortSel.addEventListener('change', () => loadLaunchpadTokens());

    // Amount input → live quote
    const launchAmtInput = document.getElementById('launchAmountInput');
    if (launchAmtInput) launchAmtInput.addEventListener('input', () => {
        clearTimeout(launchState.quoteTimer);
        launchState.quoteTimer = setTimeout(updateLaunchQuote, 150);
    });

    // Presets
    document.querySelectorAll('.launch-preset').forEach(btn => btn.addEventListener('click', () => {
        const inp = document.getElementById('launchAmountInput');
        if (inp) { inp.value = btn.dataset.lamount; updateLaunchQuote(); }
    }));

    // Trade button (Buy / Sell)
    const launchTradeBtn = document.getElementById('launchTradeBtn');
    if (launchTradeBtn) launchTradeBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
        if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }
        if (!contracts.sporepump) { showNotification('SporePump contract not found in registry', 'error'); return; }
        const tid = launchState.selectedToken;
        if (!tid) { showNotification('Select a token first', 'warning'); return; }
        const amt = parseFloat(document.getElementById('launchAmountInput')?.value) || 0;
        if (amt <= 0) { showNotification('Enter a positive amount', 'warning'); return; }
        if (amt > 9_000_000) { showNotification('Amount too large (max 9M)', 'warning'); return; }

        launchTradeBtn.disabled = true;
        const origText = launchTradeBtn.innerHTML;
        launchTradeBtn.textContent = 'Submitting...';

        try {
            if (launchState.tradeMode === 'buy') {
                const licnSpores = Math.round(amt * 1e9);
                await wallet.sendTransaction([namedCallIx(contracts.sporepump, 'buy', buildCPBuyArgs(wallet.address, tid, licnSpores), licnSpores)]);
                showNotification(`Bought tokens on Token #${tid}!`, 'success');
            } else {
                const tokenSpores = Math.round(amt * 1e9);
                await wallet.sendTransaction([namedCallIx(contracts.sporepump, 'sell', buildCPSellArgs(wallet.address, tid, tokenSpores))]);
                showNotification(`Sold tokens on Token #${tid}!`, 'success');
            }
            // Refresh
            await loadLaunchpadTokens();
            await loadLaunchpadStats();
            selectLaunchToken(tid);
        } catch (e) {
            showNotification(`Trade failed: ${e.message}`, 'error');
        }
        launchTradeBtn.disabled = false;
        launchTradeBtn.innerHTML = origText;
    });

    // Create token button
    const launchCreateBtn = document.getElementById('launchCreateBtn');
    if (launchCreateBtn) launchCreateBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
        if (!walletCanSign()) { showNotification('Reconnect wallet to sign transactions', 'warning'); return; }
        if (!contracts.sporepump) { showNotification('SporePump contract not found in registry', 'error'); return; }

        // Check balance
        const licnBal = balances['LICN']?.available || 0;
        if (licnBal < 10) { showNotification(`Insufficient LICN: need 10, have ${formatAmount(licnBal)}`, 'warning'); return; }

        launchCreateBtn.disabled = true;
        launchCreateBtn.textContent = 'Launching...';
        try {
            const creationFee = 10_000_000_000; // 10 LICN in spores
            await wallet.sendTransaction([namedCallIx(contracts.sporepump, 'create_token', buildCPCreateTokenArgs(wallet.address), creationFee)]);
            showNotification('Token launched! 🚀', 'success');
            // Refresh
            await loadLaunchpadStats();
            await loadLaunchpadTokens();
        } catch (e) {
            showNotification(`Launch failed: ${e.message}`, 'error');
        }
        launchCreateBtn.disabled = false;
        launchCreateBtn.innerHTML = '<i class="fas fa-rocket"></i> Launch Token (10 LICN)';
    });

    // ═══════════════════════════════════════════════════════════════════════
    // Notifications + Formatting
    // ═══════════════════════════════════════════════════════════════════════
    function showNotification(msg, type = 'info') {
        document.querySelector('.dex-notification')?.remove();
        const el = document.createElement('div'); el.className = 'dex-notification';
        el.style.cssText = `position:fixed;top:80px;right:20px;z-index:10000;padding:12px 20px;border-radius:8px;background:var(--bg-card,#1a1f36);color:#fff;border-left:4px solid ${{ success: '#06d6a0', warning: '#ffd166', info: '#4ea8de', error: '#ef4444' }[type] || '#4ea8de'};font-size:0.85rem;box-shadow:0 4px 24px rgba(0,0,0,0.5);animation:slideIn 0.3s ease;`;
        el.textContent = msg; document.body.appendChild(el);
        setTimeout(() => { el.style.opacity = '0'; el.style.transition = 'opacity 0.3s'; setTimeout(() => el.remove(), 300); }, 3000);
    }
    document.head.appendChild(Object.assign(document.createElement('style'), { textContent: '@keyframes slideIn{from{transform:translateX(100%);opacity:0}to{transform:translateX(0);opacity:1}}' }));

    function formatPrice(p) { if (p == null || isNaN(p)) return '0.00'; if (p === 0) return '0.00'; const a = Math.abs(p), sign = p < 0 ? '-' : ''; if (a >= 1000) return sign + a.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 }); if (a >= 1) return sign + a.toFixed(4); if (a >= 0.001) return sign + a.toFixed(6); return sign + a.toFixed(8); }
    function formatPriceRaw(p) { if (p == null || isNaN(p)) return '0.00'; if (p === 0) return '0.00'; const a = Math.abs(p), sign = p < 0 ? '-' : ''; if (a >= 1000) return sign + a.toFixed(2); if (a >= 1) return sign + a.toFixed(4); if (a >= 0.001) return sign + a.toFixed(6); return sign + a.toFixed(8); }
    function formatAmount(a) { if (a == null || isNaN(a) || a === 0) return '0'; if (a >= 1e6) return (a / 1e6).toFixed(2) + 'M'; if (a >= 1000) return a.toLocaleString('en-US', { maximumFractionDigits: 2 }); if (a >= 0.0001) return a.toFixed(4); if (a >= 0.000001) return a.toFixed(6); return '< 0.000001'; }
    function formatVolume(v) { if (v == null || isNaN(v)) return '--'; if (v === 0) return '$0.00'; if (v >= 1e9) return '$' + (v / 1e9).toFixed(2) + 'B'; if (v >= 1e6) return '$' + (v / 1e6).toFixed(2) + 'M'; if (v >= 1e3) return '$' + (v / 1e3).toFixed(1) + 'K'; return '$' + v.toFixed(2); }

    // ═══════════════════════════════════════════════════════════════════════
    // Task 3.5: Order Confirmation Dialog
    // ═══════════════════════════════════════════════════════════════════════
    function showOrderConfirmation(order) {
        return new Promise(resolve => {
            const overlay = document.createElement('div');
            overlay.className = 'order-confirm-overlay';
            const sideColor = order.side === 'buy' ? 'var(--green-success, #06d6a0)' : '#ef4444';
            const feeEst = order.total * 0.001; // ~10bps estimate
            overlay.innerHTML = `
                <div class="order-confirm-modal">
                    <h3 style="margin:0 0 16px;font-size:1rem;color:var(--text-primary);">Confirm Order</h3>
                    <div class="order-confirm-details">
                        <div class="confirm-row"><span>Side</span><span style="color:${sideColor};font-weight:700;">${escapeHtml(order.side.toUpperCase())}</span></div>
                        <div class="confirm-row"><span>Type</span><span>${escapeHtml(order.type)}</span></div>
                        <div class="confirm-row"><span>Pair</span><span>${escapeHtml(order.pair)}</span></div>
                        <div class="confirm-row"><span>Price</span><span class="mono-value">${order.type === 'market' ? 'MARKET' : formatPrice(order.price)} ${escapeHtml(order.quote)}</span></div>
                        <div class="confirm-row"><span>Amount</span><span class="mono-value">${formatAmount(order.amount)} ${escapeHtml(order.base)}</span></div>
                        <div class="confirm-row"><span>Total</span><span class="mono-value">${formatPrice(order.total)} ${escapeHtml(order.quote)}</span></div>
                        <div class="confirm-row"><span>Est. Fee</span><span class="mono-value">~${formatPrice(feeEst)} ${escapeHtml(order.quote)}</span></div>
                        ${order.isMargin ? `<div class="confirm-row"><span>Leverage</span><span class="mono-value">${order.leverage}x</span></div>` : ''}
                        ${order.stopPrice ? `<div class="confirm-row"><span>Stop Price</span><span class="mono-value">${formatPrice(order.stopPrice)} ${escapeHtml(order.quote)}</span></div>` : ''}
                    </div>
                    <label class="checkbox-label" style="margin:12px 0 16px;font-size:0.78rem;color:var(--text-muted);">
                        <input type="checkbox" id="orderConfirmSkip"> Don't show again for small orders
                    </label>
                    <div class="order-confirm-btns">
                        <button class="btn btn-small btn-secondary order-confirm-cancel-btn">Cancel</button>
                        <button class="btn btn-small ${order.side === 'buy' ? 'btn-buy' : 'btn-sell'} order-confirm-ok-btn">Confirm ${escapeHtml(order.side.toUpperCase())}</button>
                    </div>
                </div>`;
            document.body.appendChild(overlay);
            const cancel = () => { overlay.remove(); resolve(false); };
            const confirm = () => {
                const skipBox = overlay.querySelector('#orderConfirmSkip');
                if (skipBox && skipBox.checked) localStorage.setItem('dexSkipOrderConfirm', 'true');
                overlay.remove();
                resolve(true);
            };
            overlay.querySelector('.order-confirm-cancel-btn').addEventListener('click', cancel);
            overlay.querySelector('.order-confirm-ok-btn').addEventListener('click', confirm);
            overlay.addEventListener('click', (e) => { if (e.target === overlay) cancel(); });
        });
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Polling fallback — WS handles real-time data; REST polls catch up
    // F17.2: Fast (15s) for trade/pool/margin/predict, slow (30s) for governance/rewards
    // ═══════════════════════════════════════════════════════════════════════
    let pollFastRunning = false, pollSlowRunning = false, pollPredictRunning = false;
    setInterval(async () => {
        if (pollFastRunning) return;
        pollFastRunning = true;
        try {
            if (state.currentView === 'trade' && state.activePairId != null) {
                try {
                    await loadOrderBook();
                    const t = await loadTicker(state.activePairId);
                    if (t?.lastPrice) {
                        // Invert on-chain price for display-inverted pairs (wBNB/LICN → LICN/wBNB)
                        const dp = isDisplayInvertedPair(state.activePair) ? invertPrice(t.lastPrice) : t.lastPrice;
                        state.lastPrice = dp; const p = pairs.find(x => x.pairId === state.activePairId); if (p) { p.price = dp; p.change = t.change24h ?? p.change; } updateTickerDisplay(); updatePairStats(state.activePair);
                        // H2-FIX: Don't inject zero-volume synthetic bars from REST poll — let WS candle subscription handle chart updates
                    }
                    if (state.connected) {
                        await loadMarginPositions();
                    }
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
        } finally { pollFastRunning = false; }
    }, 15000);

    // F17.2: Slow polling for low-frequency data (governance + rewards + launchpad) — 30s
    setInterval(async () => {
        if (pollSlowRunning) return;
        pollSlowRunning = true;
        try {
            if (state.currentView === 'rewards') {
                try { await loadRewardsStats(); } catch { /* API unavailable */ }
            }
            if (state.currentView === 'governance') {
                try { await loadGovernanceStats(); } catch { /* API unavailable */ }
            }
            if (state.currentView === 'launchpad') {
                try { await loadLaunchpadStats(); await loadLaunchpadTokens(); } catch { /* API unavailable */ }
            }
        } finally { pollSlowRunning = false; }
    }, 30000);

    // Prediction market refresh (tight fallback cadence while WS events are unavailable)
    setInterval(async () => {
        if (pollPredictRunning) return;
        pollPredictRunning = true;
        try {
            if (state.currentView === 'predict') {
                try {
                    await loadPredictionStats();
                    await loadPredictionMarkets();
                    if (state.connected) {
                        await loadPredictionPositions();
                        loadPredictionHistory();
                        loadCreatedMarkets();
                    }
                } catch { /* API unavailable */ }
            }
        } finally { pollPredictRunning = false; }
    }, 15000);

    // Pair dropdown prices are kept live via subscribeAllTickers() over WS.
    // No REST polling needed — all ticker updates flow through the WebSocket.

    // ═══════════════════════════════════════════════════════════════════════
    // Initialize
    // ═══════════════════════════════════════════════════════════════════════
    (async function init() {
        // AUDIT-FIX F10.10: Load contract addresses before any operations
        await loadContractAddresses();
        loadProtocolParams(); // async, non-blocking — populates dynamic UI text
        await loadPairs();
        loadMarginEnabledPairs(); // async, non-blocking
        renderPairList(); renderBalances(); renderOpenOrders(); updateOrderSideLabels(); updateSubmitBtn();
        applyWalletGateAll(); // F10E.1: Apply wallet-gate to all forms on load
        loadTradeHistory(); loadMarginStats(); loadMarginPositions(); loadMarginHistory(); loadPredictionHistory();
        if (state.activePair) {
            if (pairActive) pairActive.querySelector('.pair-name').textContent = state.activePair.id;
            updateOrderBookLabels(state.activePair);
            updatePairStats(state.activePair); updateTickerDisplay(); updateMarginInfo();
            if (priceInput) priceInput.value = formatPriceRaw(state.lastPrice);
            await Promise.all([loadOrderBook(), loadRecentTrades()]);
            setTimeout(initTradingView, 200);
            connectWebSocket(); subscribePair(state.activePairId); subscribeAllTickers();
        } else {
            // No pairs on-chain — show empty state
            if (pairActive) pairActive.querySelector('.pair-name').textContent = 'No pairs';
            updateOrderBookLabels(null);
            state.orderBook = { asks: [], bids: [] }; renderOrderBook();
            const tc = document.querySelector('.trades-list');
            if (tc) tc.innerHTML = '<div style="text-align:center;color:var(--text-muted);padding:20px;font-size:0.85rem;"><i class="fas fa-info-circle" style="margin-right:6px;"></i>No trading pairs available. Bootstrap pairs via dex_core contract.</div>';
            setTimeout(initTradingView, 200);
            connectWebSocket(); subscribeAllTickers();
        }
        // F10E.7 / DEX-M01 / FE-06: oracle fast-poll overlay (opt-in, default off).
        // Replaces former Binance WS — all prices now flow through internal oracle.
        if (ENABLE_EXTERNAL_PRICE_WS) startOracleFastPoll();
        if (savedWallets.length) {
            const activeWalletAddress = localStorage.getItem(ACTIVE_WALLET_KEY);
            const restored = savedWallets.find(w => w.address === activeWalletAddress) || savedWallets[savedWallets.length - 1];
            if (restored && restored.address) {
                const shortAddr = restored.short || restored.address.slice(0, 8) + '...';
                const { signingReady, localKeypair } = await resolveWalletSigningState(restored.address);
                await connectWalletTo(restored.address, shortAddr, { signingReady, localKeypair });
            }
        }
    })().catch(e => console.error('[DEX] Init error:', e));

    // F6.12: Clean up WebSocket connections on page unload
    window.addEventListener('beforeunload', () => {
        if (dexWs) dexWs.close();
    });
});
