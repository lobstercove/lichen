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
                if (this._closing) return;
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
        close() {
            this._closing = true;
            if (this.ws) { this.ws.onclose = null; this.ws.close(); this.ws = null; }
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
        // AUDIT-FIX F10.9: sendTransaction produces validator-compatible JSON.
        // Wire format must match parse_json_transaction():
        //   - signatures: array of hex strings (64 bytes each)
        //   - message.instructions[].program_id: base58 string
        //   - message.instructions[].accounts: array of base58 strings
        //   - message.instructions[].data: array of u8 numbers
        //   - message.blockhash: hex string of 32-byte hash
        async sendTransaction(instructions) {
            if (!this.keypair) throw new Error('Wallet not connected');
            if (!this._nacl) throw new Error('Signing library not loaded');
            const blockhash = await api.rpc('getRecentBlockhash');
            // Normalize instructions: ensure accounts + data format
            const normalizedIx = instructions.map(ix => {
                const accounts = ix.accounts || [this.address];
                const dataBytes = typeof ix.data === 'string' ? Array.from(new TextEncoder().encode(ix.data)) : Array.from(ix.data);
                return { program_id: ix.program_id, accounts, data: dataBytes };
            });
            // Sign: bincode-compatible message bytes (must match validator's message.serialize())
            const msgBytes = encodeTransactionMessage(normalizedIx, blockhash, this.address);
            const sig = this.sign(msgBytes);
            // Wire format: JSON matching parse_json_transaction()
            const txPayload = {
                signatures: [bytesToHex(sig)],
                message: {
                    instructions: normalizedIx,
                    blockhash: blockhash,
                },
            };
            const txBase64 = btoa(String.fromCharCode(...new TextEncoder().encode(JSON.stringify(txPayload))));
            return api.rpc('sendTransaction', [txBase64]);
        },
    };

    function bytesToHex(b) { return Array.from(b).map(x => x.toString(16).padStart(2, '0')).join(''); }
    function hexToBytes(h) { const c = h.startsWith('0x') ? h.slice(2) : h; const o = new Uint8Array(c.length / 2); for (let i = 0; i < o.length; i++) o[i] = parseInt(c.slice(i * 2, i * 2 + 2), 16); return o; }

    // AUDIT-FIX F10.8: Sanitize all API-sourced data before innerHTML injection
    function escapeHtml(str) {
        if (typeof str !== 'string') return String(str ?? '');
        return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
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

    // ═══════════════════════════════════════════════════════════════════════
    // F8.2 SYSTEMIC FIX: Contract call infrastructure
    // All contract calls must use CONTRACT_PROGRAM_ID (0xFF*32),
    // include [caller, contract] in accounts, and serialize data as
    // ContractInstruction::Call { function: "call", args, value: 0 }
    // ═══════════════════════════════════════════════════════════════════════
    const CONTRACT_PROGRAM_ID = bs58encode(new Uint8Array(32).fill(0xFF));

    // Build ContractInstruction::Call JSON (matches Rust's serde serialization)
    function buildContractCall(argsBytes) {
        return JSON.stringify({ Call: { function: "call", args: Array.from(argsBytes), value: 0 }});
    }

    // Build a sendTransaction instruction with correct program_id + accounts
    function contractIx(contractAddr, argsBytes) {
        return {
            program_id: CONTRACT_PROGRAM_ID,
            accounts: [wallet.address, contractAddr],
            data: buildContractCall(argsBytes),
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
    // Opcode 2: place_order(trader, pair_id, side, type, price, qty, expiry)
    function buildPlaceOrderArgs(trader, pairId, side, orderType, price, quantity) {
        const buf = new ArrayBuffer(67);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 2); // opcode
        writePubkey(arr, 1, trader);
        writeU64LE(view, 33, pairId);
        writeU8(arr, 41, side === 'buy' ? 0 : 1);
        writeU8(arr, 42, orderType === 'market' ? 1 : 0);
        writeU64LE(view, 43, price);
        writeU64LE(view, 51, quantity);
        writeU64LE(view, 59, 0); // expiry: 0 = no expiry
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
    // Opcode 2: open_position(trader, pair_id, side, size, leverage, margin)
    function buildOpenPositionArgs(trader, pairId, side, size, leverage, margin) {
        const buf = new ArrayBuffer(66);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 2); // opcode
        writePubkey(arr, 1, trader);
        writeU64LE(view, 33, pairId);
        writeU8(arr, 41, side === 'long' ? 0 : 1);
        writeU64LE(view, 42, size);
        writeU64LE(view, 50, leverage);
        writeU64LE(view, 58, margin);
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

    // Opcode 11: dao_resolve(caller, market_id, winning_outcome)
    function buildResolveMarketArgs(caller, marketId, winningOutcome) {
        const buf = new ArrayBuffer(42);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 11); // opcode
        writePubkey(arr, 1, caller);
        writeU64LE(view, 33, marketId);
        writeU8(arr, 41, winningOutcome);
        return arr;
    }

    // Opcode 1: create_market(creator, category, close_slot, outcome_count, question_hash, question)
    function buildCreateMarketArgs(creator, question, category, outcomeCount) {
        const encoder = new TextEncoder();
        const qBytes = encoder.encode(question);
        const totalLen = 79 + qBytes.length;
        const buf = new ArrayBuffer(totalLen);
        const view = new DataView(buf);
        const arr = new Uint8Array(buf);
        writeU8(arr, 0, 1); // opcode
        writePubkey(arr, 1, creator);
        // Category: map string to u8 enum (0=general, 1=crypto, 2=sports, 3=politics, 4=entertainment, 5=science)
        const catMap = { general: 0, crypto: 1, sports: 2, politics: 3, entertainment: 4, science: 5 };
        writeU8(arr, 33, catMap[category] ?? 0);
        writeU64LE(view, 34, 0); // close_slot: 0 = open-ended
        writeU8(arr, 42, outcomeCount || 2);
        // question_hash: simple hash of question string (fill 32 bytes)
        const hashBytes = new Uint8Array(32);
        for (let i = 0; i < qBytes.length; i++) hashBytes[i % 32] ^= qBytes[i];
        arr.set(hashBytes, 43);
        view.setUint32(75, qBytes.length, true); // question_len
        arr.set(qBytes, 79);
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

    // ── Tick math for AMM (Uniswap V3 style) ──
    const MIN_TICK = -887272;
    const MAX_TICK = 887272;
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
    // F10E.6: MOLT genesis price — $0.10 per MOLT at network launch
    const MOLT_GENESIS_PRICE = 0.10;

    const state = {
        activePair: null, activePairId: 0, orderSide: 'buy', orderType: 'limit',
        marginSide: 'long', marginType: 'isolated', chartInterval: '15m', chartType: 'candle',
        currentView: 'trade', leverageValue: 2, lastPrice: MOLT_GENESIS_PRICE, orderBook: { asks: [], bids: [] },
        candles: [], connected: false, tradeMode: 'spot', _wsSubs: [],
    };
    let pairs = [], balances = {}, openOrders = [];

    // AUDIT-FIX F10.10: Contract addresses loaded from RPC symbol registry.
    // These are base58-encoded 32-byte pubkeys — the actual deployed addresses
    // from deploy-manifest.json, resolved at runtime via getSymbolRegistry.
    const contracts = {
        dex_core: null, dex_amm: null, dex_router: null, dex_margin: null,
        dex_rewards: null, dex_governance: null, dex_analytics: null, prediction_market: null,
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
                console.log('[DEX] Contract addresses loaded from symbol registry');
            }
        } catch (e) {
            console.warn('[DEX] Symbol registry unavailable, trying deploy manifest:', e.message);
        }
        // Fallback: genesis-deployed addresses (deterministic from deployer + WASM)
        // WARNING: These MUST match the live genesis auto-deploy. If contracts are
        // recompiled, addresses change. Always prefer the symbol registry (above).
        const needsFallback = !contracts.dex_core;
        if (!contracts.dex_core) contracts.dex_core = '7QvQ1dxFTdSk9aSzbBe2gHCJH1bSRBDwVdPTn9M5iCds';
        if (!contracts.dex_amm) contracts.dex_amm = '72AvbSmnkv82Bsci9BHAufeAGMTycKQX5Y6DL9ghTHay';
        if (!contracts.dex_router) contracts.dex_router = 'FwAxYo2bKmCe1c5gZZjvuyopJMDgm1T9CAWr2svB1GPf';
        if (!contracts.dex_margin) contracts.dex_margin = '8rTFuvbHZY89c3d9NktefAbHfjRoYh3vYJoC7eVgcw3W';
        if (!contracts.dex_rewards) contracts.dex_rewards = '2okkNYSYPdN1jvhnhpXTmseFdXzgAgQXSCkQhgCkNiqC';
        if (!contracts.dex_governance) contracts.dex_governance = '7BKw55h387pVAUs1dNApn2rfARBcGnnncXyb4WZDdGru';
        if (!contracts.dex_analytics) contracts.dex_analytics = 'FBE25S5yGHUa6q38P8SjVXviw6dkoqD7oCMUuxj1aRof';
        if (!contracts.prediction_market) contracts.prediction_market = 'J8sMvYFXW4ZCHc488KJ1zmZq1sQMTWyWfr8qnzUwwEyD';
        if (needsFallback) {
            console.warn('[DEX] Using fallback contract addresses — symbol registry was unavailable. Transactions may fail if contracts were recompiled.');
        }
    }

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
            state.activePair = pairs[0]; state.activePairId = pairs[0].pairId;
            state.lastPrice = pairs[0].price || MOLT_GENESIS_PRICE;
            // F10E.6: Ensure pairs with zero price get genesis fallback
            pairs.forEach(p => { if (!p.price) p.price = (p.id === 'MOLT/mUSD' || p.base === 'MOLT') ? MOLT_GENESIS_PRICE : 0; });
        } else {
            // F10E.6: No pairs from API — create genesis default MOLT/mUSD pair
            pairs = [{ pairId: 1, id: 'MOLT/mUSD', base: 'MOLT', quote: 'mUSD', price: MOLT_GENESIS_PRICE, change: 0, tickSize: 0.0001, lotSize: 0.01, symbol: 'MOLT/mUSD' }];
            state.activePair = pairs[0]; state.activePairId = 1; state.lastPrice = MOLT_GENESIS_PRICE;
            console.info('[DEX] No trading pairs on-chain — using genesis MOLT/mUSD @ $0.10');
        }
        // Populate all select dropdowns from real pairs
        populateSelectsFromPairs();
    }

    function populateSelectsFromPairs() {
        const poolSelect = document.getElementById('liqPoolSelect');
        const marginSelect = document.getElementById('marginPairSelect');
        const feeSelect = document.getElementById('propFeePair');
        const delistSelect = document.getElementById('propDelistPair');
        const opts = pairs.map((p, i) => `<option value="${escapeHtml(String(p.pairId))}">${escapeHtml(p.id)}</option>`).join('');
        if (poolSelect) poolSelect.innerHTML = opts || '<option>No pairs available</option>';
        if (marginSelect) marginSelect.innerHTML = opts || '<option>No pairs available</option>';
        if (feeSelect) feeSelect.innerHTML = opts || '<option>No pairs available</option>';
        if (delistSelect) delistSelect.innerHTML = opts || '<option>No pairs available</option>';
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

    // F6.11: RAF-throttle for high-frequency WS order book updates
    function rafThrottle(fn) { let pending = false, lastArgs; return function(...args) { lastArgs = args; if (!pending) { pending = true; requestAnimationFrame(() => { pending = false; fn(...lastArgs); }); } }; }
    const throttledRenderOrderBook = rafThrottle(() => { if (state.currentView === 'trade') renderOrderBook(); });

    function subscribePair(pairId) {
        if (!dexWs) return;
        state._wsSubs.forEach(id => dexWs.unsubscribe(id)); state._wsSubs = [];

        dexWs.subscribe(`orderbook:${pairId}`, (d) => {
            if (d.bids && d.asks) {
                const map = arr => arr.map(a => ({ price: a.price, amount: a.quantity, total: 0 }));
                const asks = map(d.asks); asks.sort((a, b) => a.price - b.price); let t = 0; asks.forEach(a => { t += a.amount; a.total = t; });
                const bids = map(d.bids); bids.sort((a, b) => b.price - a.price); t = 0; bids.forEach(b => { t += b.amount; b.total = t; });
                state.orderBook = { asks, bids }; throttledRenderOrderBook();
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
            if (d.lastPrice) {
                state.lastPrice = d.lastPrice;
                const pair = pairs.find(p => p.pairId === pairId);
                if (pair) { pair.price = d.lastPrice; pair.change = d.change24h || pair.change; }
                updateTickerDisplay();
            }
        }).then(id => state._wsSubs.push(id)).catch(() => {});

        if (wallet.address) {
            dexWs.subscribe(`orders:${wallet.address}`, (d) => {
                if (d.orderId) {
                    const o = openOrders.find(x => x.id === String(d.orderId));
                    if (o) { o.filled = d.filled / ((d.filled + d.remaining) || 1); }
                    if (d.status === 'filled' || d.status === 'cancelled') {
                        showNotification(`Order ${d.status}: #${d.orderId}`, d.status === 'filled' ? 'success' : 'info');
                        openOrders = openOrders.filter(x => x.id !== String(d.orderId));
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
            <div class="pair-item ${state.activePair?.id === p.id ? 'active' : ''}" data-pair="${escapeHtml(p.id)}">
                <span>${escapeHtml(p.id)}</span><span class="pair-price">${formatPrice(p.price)}</span>
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
        // Update oracle reference line for new pair
        updateOracleReferenceLine();
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
    let tvWidget = null, realtimeCallback = null, lastBarTime = 0, activeResolution = '15';


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
            subscribeBars: (si, res, cb) => { realtimeCallback = cb; activeResolution = res; },
            unsubscribeBars: () => { realtimeCallback = null; },
        };
    }

    function streamBarUpdate(price, vol) {
        if (!realtimeCallback) return;
        const ms = resolutionToMs(activeResolution);
        const bt = Math.floor(Date.now() / ms) * ms;
        realtimeCallback(bt > lastBarTime ? (lastBarTime = bt, { time: bt, open: price, high: price, low: price, close: price, volume: vol }) : { time: lastBarTime, close: price, high: price, low: price, volume: vol });
    }

    function resolutionToMs(r) { return { '1': 60000, '5': 300000, '15': 900000, '30': 1800000, '60': 3600000, '240': 14400000, '1D': 86400000, '1W': 604800000, '1M': 2592000000 }[r] || 900000; }
    function resolutionToSec(r) { return { '1': 60, '5': 300, '15': 900, '30': 1800, '60': 3600, '240': 14400, '1D': 86400, '1W': 604800, '1M': 2592000 }[r] || 900; }

    function initTradingView() {
        const el = document.getElementById('tvChartContainer');
        if (!el || typeof TradingView === 'undefined') { if (el) el.innerHTML = '<div style="display:flex;align-items:center;justify-content:center;height:100%;color:var(--text-muted);font-size:0.9rem;"><i class="fas fa-chart-line" style="margin-right:8px;"></i> Chart unavailable — library failed to load</div>'; setTimeout(initTradingView, 5000); return; }
        tvWidget = new TradingView.widget({
            symbol: state.activePair?.id || 'MOLT/mUSD', container: el, datafeed: createDatafeed(), library_path: 'charting_library/', locale: 'en', fullscreen: false, autosize: true, theme: 'Dark', interval: '15', toolbar_bg: '#0d1117',
            loading_screen: { backgroundColor: '#0A0E27', foregroundColor: '#FF6B35' },
            overrides: { 'paneProperties.background': '#0d1117', 'paneProperties.backgroundType': 'solid', 'paneProperties.vertGridProperties.color': 'rgba(255,255,255,0.04)', 'paneProperties.horzGridProperties.color': 'rgba(255,255,255,0.04)', 'scalesProperties.textColor': 'rgba(255,255,255,0.5)', 'scalesProperties.lineColor': 'rgba(255,255,255,0.08)', 'mainSeriesProperties.candleStyle.upColor': '#06d6a0', 'mainSeriesProperties.candleStyle.downColor': '#ef4444', 'mainSeriesProperties.candleStyle.borderUpColor': '#06d6a0', 'mainSeriesProperties.candleStyle.borderDownColor': '#ef4444', 'mainSeriesProperties.candleStyle.wickUpColor': '#06d6a0', 'mainSeriesProperties.candleStyle.wickDownColor': '#ef4444' },
            disabled_features: ['header_compare','header_undo_redo','go_to_date','use_localstorage_for_settings'],
            enabled_features: ['study_templates','side_toolbar_in_fullscreen_mode','header_symbol_search'],
        });
        tvWidget.onChartReady(() => { tvWidget.activeChart().onSymbolChanged().subscribe(null, () => { const s = tvWidget.activeChart().symbol(); const p = pairs.find(x => x.id === s || ('MoltChain:' + x.id) === s); if (p && p.id !== state.activePair?.id) selectPair(p); }); });
    }

    function drawChart() { if (realtimeCallback && state.candles.length) { const l = state.candles[state.candles.length - 1]; const ms = resolutionToMs(activeResolution); realtimeCallback({ time: Math.floor(l.time / ms) * ms, open: l.open, high: l.high, low: l.low, close: l.close, volume: l.volume }); } }

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

    // F9.5a/F9.5b/F9.12a: Route info and fee estimate from actual router quote API
    let _routeQuoteTimer = null;
    const ROUTE_TYPE_LABELS = { clob: 'CLOB Direct', amm: 'AMM Pool', split: 'CLOB + AMM Split', multi_hop: 'Multi-Hop', legacy_swap: 'Legacy Swap' };
    function calcTotal() {
        if (!priceInput || !amountInput || !totalInput) return;
        const p = parseFloat(priceInput.value) || 0, a = parseFloat(amountInput.value) || 0;
        totalInput.value = (p * a).toFixed(4);
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
                    const { data } = await api.post('/router/quote', { token_in: tokenIn, token_out: tokenOut, amount_in: amountIn, slippage: 0.5 });
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
    if (totalInput) totalInput.addEventListener('input', () => { if (!priceInput || !amountInput) return; const p = parseFloat(priceInput.value) || 0, t = parseFloat(totalInput.value) || 0; if (p > 0) amountInput.value = (t / p).toFixed(4); });

    document.querySelectorAll('.preset-btn').forEach(btn => btn.addEventListener('click', () => {
        const pct = parseInt(btn.dataset.pct, 10) / 100, tok = state.orderSide === 'buy' ? state.activePair?.quote : state.activePair?.base, bal = tok ? balances[tok] : null;
        if (!bal || !amountInput || !priceInput) return;
        if (state.orderSide === 'buy') { amountInput.value = ((bal.available * pct) / (parseFloat(priceInput.value) || state.lastPrice)).toFixed(4); } else amountInput.value = (bal.available * pct).toFixed(4);
        calcTotal();
    }));

    // === AUDIT-FIX F10.1: Order submission via signed sendTransaction (not REST POST) ===
    if (submitBtn) submitBtn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
        if (!wallet.keypair) { showNotification('Re-import wallet to sign transactions', 'warning'); return; }
        const price = parseFloat(priceInput?.value) || 0, amount = parseFloat(amountInput?.value) || 0;
        if (!amount || (state.orderType !== 'market' && !price)) { showNotification('Enter price and amount', 'warning'); return; }
        if (!contracts.dex_core) { showNotification('Contract addresses not loaded', 'error'); return; }
        // F4.3: Client-side balance check before submitting order
        {
            const pair = state.activePair;
            const neededToken = state.orderSide === 'buy' ? (pair?.quote || 'mUSD') : (pair?.base || 'MOLT');
            const neededAmount = state.orderSide === 'buy' ? (price * amount) : amount;
            const available = balances[neededToken]?.available || 0;
            if (neededAmount > available) {
                showNotification(`Insufficient ${neededToken} balance: need ${formatAmount(neededAmount)}, have ${formatAmount(available)}`, 'warning');
                return;
            }
        }
        submitBtn.disabled = true; submitBtn.textContent = 'Submitting...';
        try {
            // F10.6 FIX: Route to margin contract when tradeMode is margin
            if (state.tradeMode === 'margin') {
                if (!contracts.dex_margin) { showNotification('Margin contract not loaded', 'error'); submitBtn.disabled = false; updateSubmitBtn(); return; }
                const marginSide = state.orderSide === 'buy' ? 'long' : 'short';
                const size = Math.round(amount * PRICE_SCALE);
                const leverage = state.leverageValue;
                const marginDeposit = Math.round((amount * (price || state.lastPrice) / leverage) * PRICE_SCALE);
                const result = await wallet.sendTransaction([contractIx(
                    contracts.dex_margin,
                    buildOpenPositionArgs(wallet.address, state.activePairId, marginSide, size, leverage, marginDeposit)
                )]);
                showNotification(`${marginSide.toUpperCase()} ${state.leverageValue}x opened: ${formatAmount(amount)} ${state.activePair?.base || ''} @ ${formatPrice(price || state.lastPrice)}`, 'success');
            } else {
                const result = await wallet.sendTransaction([contractIx(
                    contracts.dex_core,
                    buildPlaceOrderArgs(wallet.address, state.activePairId, state.orderSide, state.orderType, Math.round(price * PRICE_SCALE), Math.round(amount * PRICE_SCALE))
                )]);
                showNotification(`${state.orderSide.toUpperCase()} order placed: ${formatAmount(amount)} ${state.activePair?.base || ''} @ ${state.orderType === 'market' ? 'MARKET' : formatPrice(price)}`, 'success');
                const orderId = result?.order_id || result?.orderId || Math.random().toString(36).slice(2, 8).toUpperCase();
                openOrders.push({ id: String(orderId), pair: state.activePair?.id, side: state.orderSide, type: state.orderType, price: price || state.lastPrice, amount, filled: 0, time: new Date() });
                renderOpenOrders();
            }
            if (amountInput) amountInput.value = ''; if (totalInput) totalInput.value = '';
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
        tb.innerHTML = openOrders.map(o => `<tr class="order-row"><td>${escapeHtml(o.pair)}</td><td class="side-${o.side}">${escapeHtml(o.side.toUpperCase())}</td><td style="text-transform:capitalize">${escapeHtml(o.type)}</td><td>${formatPrice(o.price)}</td><td>${formatAmount(o.amount)}</td><td>${(o.filled * 100).toFixed(0)}%</td><td>${o.time instanceof Date ? o.time.toLocaleTimeString() : ''}</td><td><button class="cancel-btn" data-id="${o.id}"><i class="fas fa-times"></i></button></td></tr>`).join('');
        tb.querySelectorAll('.cancel-btn').forEach(btn => btn.addEventListener('click', async () => {
            // AUDIT-FIX F10.2: Cancel order via signed sendTransaction (not unsigned DELETE)
            if (!wallet.keypair) { showNotification('Re-import wallet to sign', 'warning'); return; }
            try {
                await wallet.sendTransaction([contractIx(
                    contracts.dex_core,
                    buildCancelOrderArgs(wallet.address, parseInt(btn.dataset.id) || 0)
                )]);
            } catch { /* fallback — order may already be cancelled/filled */ }
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
        applyWalletGateAll();
        await Promise.all([loadBalances(address), loadUserOrders(address)]);
        renderBalances(); renderOpenOrders(); loadTradeHistory(); loadPositionsTab();
        if (dexWs && state.activePairId != null) subscribePair(state.activePairId);
    }

    function disconnectWallet() {
        state.connected = false; state.walletAddress = null; wallet.keypair = null; wallet.address = null;
        if (connectBtn) { connectBtn.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet'; connectBtn.className = 'btn btn-small btn-primary'; }
        openOrders = []; balances = {};
        toggleWalletPanels(false);
        applyWalletGateAll();
        renderBalances(); renderOpenOrders();
        // Clear wallet-gated sections
        loadTradeHistory(); loadPositionsTab(); loadLPPositions(); loadPredictionPositions();
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
        c.innerHTML = Object.entries(balances).map(([t, b]) => `<div class="balance-row"><div class="balance-token"><div class="token-icon ${escapeHtml(t.toLowerCase())}-icon">${escapeHtml(t[0])}</div><span>${escapeHtml(t)}</span></div><div class="balance-amounts"><span class="balance-available">${formatAmount(b.available)}</span><span class="balance-usd">≈ $${formatAmount(b.usd)}</span></div></div>`).join('');
    }

    // ═══════════════════════════════════════════════════════════════════════
    // F10E.1/E2/E4/E9/E10 — Wallet-Gate All Interactive Forms
    // ═══════════════════════════════════════════════════════════════════════
    function applyWalletGateAll() {
        const connected = state.connected;

        // --- Trade view: Order Form (F10E.1) ---
        const orderForm = document.querySelector('.order-form');
        if (orderForm) orderForm.classList.toggle('wallet-gated-disabled', !connected);
        if (submitBtn) {
            if (connected) {
                submitBtn.disabled = false;
                submitBtn.classList.remove('btn-wallet-gate');
                updateSubmitBtn();
            } else {
                submitBtn.disabled = true;
                submitBtn.className = 'btn-full btn-wallet-gate';
                submitBtn.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet to Trade';
            }
        }

        // --- Predict view: Quick Trade + Create Market (F10E.2) ---
        const predictTradePanel = document.querySelector('.predict-trade-panel');
        const predictCreatePanel = document.querySelector('.predict-create-panel');
        if (predictTradePanel) predictTradePanel.classList.toggle('wallet-gated-disabled', !connected);
        if (predictCreatePanel) predictCreatePanel.classList.toggle('wallet-gated-disabled', !connected);

        const predictSubmit = document.getElementById('predictSubmitBtn');
        if (predictSubmit) {
            if (connected) {
                predictSubmit.disabled = false;
                predictSubmit.classList.remove('btn-wallet-gate');
                const side = (typeof predictState !== 'undefined' && predictState.selectedOutcome === 'no') ? 'NO' : 'YES';
                predictSubmit.innerHTML = `<i class="fas fa-bolt"></i> Buy ${side} Shares`;
            } else {
                predictSubmit.disabled = true;
                predictSubmit.className = 'btn-full btn-wallet-gate';
                predictSubmit.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet to Trade';
            }
        }
        const predictCreate = document.getElementById('predictCreateBtn');
        if (predictCreate) {
            if (connected) {
                predictCreate.disabled = false;
                predictCreate.classList.remove('btn-wallet-gate');
                predictCreate.innerHTML = '<i class="fas fa-rocket"></i> Create Market';
            } else {
                predictCreate.disabled = true;
                predictCreate.className = 'btn btn-full btn-wallet-gate';
                predictCreate.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet to Create';
            }
        }

        // --- Pool view: Add Liquidity (F10E.10) ---
        const addLiqForm = document.getElementById('addLiqForm');
        if (addLiqForm) addLiqForm.classList.toggle('wallet-gated-disabled', !connected);
        const addLiqSubmit = document.getElementById('addLiqBtn');
        if (addLiqSubmit) {
            if (connected) {
                addLiqSubmit.disabled = false;
                addLiqSubmit.classList.remove('btn-wallet-gate');
                addLiqSubmit.textContent = 'Add Liquidity';
            } else {
                addLiqSubmit.disabled = true;
                addLiqSubmit.className = 'btn btn-full btn-wallet-gate';
                addLiqSubmit.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet';
            }
        }

        // --- Margin view: Open Position (F10E.9) ---
        const marginFormCard = document.querySelector('.margin-form-card');
        if (marginFormCard) marginFormCard.classList.toggle('wallet-gated-disabled', !connected);
        const marginOpen = document.getElementById('marginOpenBtn');
        if (marginOpen) {
            if (connected) {
                marginOpen.disabled = false;
                marginOpen.classList.remove('btn-wallet-gate');
                marginOpen.textContent = `Open ${state.marginSide === 'long' ? 'Long' : 'Short'}`;
            } else {
                marginOpen.disabled = true;
                marginOpen.className = 'btn btn-full btn-wallet-gate';
                marginOpen.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet';
            }
        }

        // --- Governance: New Proposal (F10E.4) ---
        const proposalForm = document.getElementById('proposalForm');
        if (proposalForm) proposalForm.classList.toggle('wallet-gated-disabled', !connected);
        const proposalSubmit = document.getElementById('submitProposalBtn');
        if (proposalSubmit) {
            if (connected) {
                proposalSubmit.disabled = false;
                proposalSubmit.classList.remove('btn-wallet-gate');
                proposalSubmit.innerHTML = '<i class="fas fa-paper-plane"></i> Submit Proposal';
            } else {
                proposalSubmit.disabled = true;
                proposalSubmit.className = 'btn btn-full btn-wallet-gate';
                proposalSubmit.innerHTML = '<i class="fas fa-wallet"></i> Connect Wallet to Propose';
            }
        }

        // --- Rewards: Claim button (already has wallet check, but disable visually) ---
        const claimAll = document.getElementById('claimAllBtn');
        if (claimAll) {
            claimAll.disabled = !connected;
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // F10E.7 — External Price Feed (Binance WebSocket for real-time wSOL, wETH)
    // The backend oracle price feeder connects to Binance WebSocket
    // (aggTrade streams) for real-time SOL/ETH prices and writes to
    // moltoracle + dex_analytics every 1s when prices change. The frontend
    // Binance WebSocket supplements this with sub-second ticker updates
    // for a responsive UI between API polls.
    // ═══════════════════════════════════════════════════════════════════════
    const externalPrices = { wSOL: 0, wETH: 0 };
    let binanceWs = null;

    function connectBinancePriceFeed() {
        // Streams: SOL/USDT, ETH/USDT mini tickers
        const streams = 'solusdt@miniTicker/ethusdt@miniTicker';
        const url = `wss://stream.binance.com:9443/ws/${streams}`;
        try {
            binanceWs = new WebSocket(url);
            binanceWs.onmessage = (evt) => {
                try {
                    const d = JSON.parse(evt.data);
                    const price = parseFloat(d.c); // close price
                    if (!price || isNaN(price)) return;
                    const sym = (d.s || '').toUpperCase();
                    if (sym === 'SOLUSDT') externalPrices.wSOL = price;
                    else if (sym === 'ETHUSDT') externalPrices.wETH = price;
                    // Real-time overlay: update active pair price between API polls
                    applyBinanceRealTimeOverlay();
                } catch { /* malformed message */ }
            };
            binanceWs.onclose = () => { setTimeout(connectBinancePriceFeed, 5000); };
            binanceWs.onerror = () => { try { binanceWs.close(); } catch { /* already closed */ } };
            console.log('[DEX] Binance price feed connected (real-time overlay)');
        } catch (e) {
            console.warn('[DEX] Binance price feed unavailable:', e.message);
        }
    }

    function applyBinanceRealTimeOverlay() {
        // Only update the active pair's ticker display in real-time.
        // The pair list prices come from the backend oracle feeder via loadPairs().
        if (!state.activePair) return;
        const base = (state.activePair.base || '').toUpperCase();
        const quote = (state.activePair.quote || '').toUpperCase();
        let realtimePrice = 0;
        if ((base === 'WSOL' || base === 'SOL') && externalPrices.wSOL > 0) {
            realtimePrice = externalPrices.wSOL;
        } else if ((base === 'WETH' || base === 'ETH') && externalPrices.wETH > 0) {
            realtimePrice = externalPrices.wETH;
        }
        if (realtimePrice <= 0) return;
        // For MOLT-quoted pairs, convert using MOLT genesis price or API price
        if (quote === 'MOLT') {
            const moltPair = pairs.find(p => (p.base || '').toUpperCase() === 'MOLT' && (p.quote || '').toUpperCase() === 'MUSD');
            const moltUsd = moltPair?.price || MOLT_GENESIS_PRICE;
            if (moltUsd > 0) realtimePrice = realtimePrice / moltUsd;
            else return;
        }
        // Update ticker display with sub-second price
        state.lastPrice = realtimePrice;
        updateTickerDisplay();
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Phase D — Oracle Price Reference Line on Chart
    // Fetches oracle prices from /api/v1/oracle/prices every 5s and draws
    // a horizontal dashed line on the TradingView chart showing the
    // Binance oracle reference price for the active pair. This gives
    // traders a visual comparison between on-chain trade price and the
    // external oracle index price.
    // ═══════════════════════════════════════════════════════════════════════
    let oracleLineId = null;
    let oracleRefPrices = {};

    async function fetchOracleRefPrices() {
        try {
            const resp = await fetch(`${API}/oracle/prices`);
            if (!resp.ok) return;
            const data = await resp.json();
            if (data.feeds) {
                for (const feed of data.feeds) {
                    if (feed.price > 0 && !feed.stale) {
                        oracleRefPrices[feed.asset] = feed.price;
                    }
                }
            }
            updateOracleReferenceLine();
        } catch { /* network error — skip */ }
    }

    function getOracleRefForPair() {
        if (!state.activePair) return 0;
        const base = (state.activePair.base || '').toUpperCase();
        const quote = (state.activePair.quote || '').toUpperCase();
        let refPrice = 0;
        if ((base === 'WSOL' || base === 'SOL') && oracleRefPrices['wSOL']) {
            refPrice = oracleRefPrices['wSOL'];
        } else if ((base === 'WETH' || base === 'ETH') && oracleRefPrices['wETH']) {
            refPrice = oracleRefPrices['wETH'];
        } else if (base === 'MOLT' && oracleRefPrices['MOLT']) {
            refPrice = oracleRefPrices['MOLT'];
        }
        if (refPrice <= 0) return 0;
        // Convert for MOLT-quoted pairs
        if (quote === 'MOLT') {
            const moltUsd = oracleRefPrices['MOLT'] || MOLT_GENESIS_PRICE;
            if (moltUsd > 0) refPrice = refPrice / moltUsd;
            else return 0;
        }
        return refPrice;
    }

    function updateOracleReferenceLine() {
        if (!tvWidget?.activeChart) return;
        try {
            const chart = tvWidget.activeChart();
            const refPrice = getOracleRefForPair();
            // Remove old line
            if (oracleLineId) {
                try { chart.removeEntity(oracleLineId); } catch { /* already removed */ }
                oracleLineId = null;
            }
            if (refPrice <= 0) return;
            // Draw horizontal dashed line at oracle reference price
            oracleLineId = chart.createShape(
                { time: 0, price: refPrice },
                {
                    shape: 'horizontal_line',
                    lock: true,
                    disableSelection: true,
                    disableSave: true,
                    disableUndo: true,
                    overrides: {
                        linecolor: '#FFD700',
                        linewidth: 1,
                        linestyle: 2, // dashed
                        showLabel: true,
                        text: `Oracle: $${refPrice < 1 ? refPrice.toFixed(4) : refPrice.toFixed(2)}`,
                        textcolor: '#FFD700',
                        fontsize: 10,
                        horzLabelsAlign: 'right',
                        showPrice: false,
                    },
                }
            );
        } catch { /* TradingView API not ready yet */ }
    }

    // Poll oracle prices every 5 seconds for the reference line
    setInterval(fetchOracleRefPrices, 5000);
    // Initial fetch after short delay to allow TradingView to initialize
    setTimeout(fetchOracleRefPrices, 2000);

    // ═══════════════════════════════════════════════════════════════════════
    // Margin View
    // ═══════════════════════════════════════════════════════════════════════
    const leverageSlider = document.getElementById('leverageSlider'), leverageDisplay = document.querySelector('.leverage-display');
    if (leverageSlider) leverageSlider.addEventListener('input', () => { state.leverageValue = parseFloat(leverageSlider.value); if (leverageDisplay) leverageDisplay.textContent = `${state.leverageValue}x`; updateMarginInfo(); });
    document.querySelectorAll('.margin-type').forEach(btn => btn.addEventListener('click', () => { document.querySelectorAll('.margin-type').forEach(b => b.classList.remove('active')); btn.classList.add('active'); state.marginType = btn.dataset.type; if (leverageSlider) leverageSlider.max = state.marginType === 'isolated' ? '5' : '3'; if (state.leverageValue > parseFloat(leverageSlider?.max)) { state.leverageValue = parseFloat(leverageSlider.max); leverageSlider.value = state.leverageValue; if (leverageDisplay) leverageDisplay.textContent = `${state.leverageValue}x`; } updateMarginInfo(); }));
    document.querySelectorAll('.side-btn').forEach(btn => btn.addEventListener('click', () => { document.querySelectorAll('.side-btn').forEach(b => b.classList.remove('active')); btn.classList.add('active'); state.marginSide = btn.classList.contains('long-btn') ? 'long' : 'short'; const ob = document.getElementById('marginOpenBtn'); if (ob) { ob.textContent = `Open ${state.marginSide === 'long' ? 'Long' : 'Short'}`; ob.className = `btn btn-full ${state.marginSide === 'long' ? 'btn-buy' : 'btn-sell'}`; } }));

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
        if (e) e.textContent = formatPrice(state.lastPrice);
        // F10.7 FIX: Liquidation price uses tier-appropriate maintenance BPS
        // Liq occurs when margin_ratio drops to maintenance level
        // For long: liq_price = entry * (1 - (margin/notional - maintBps/10000))
        //         = entry * (1 - 1/leverage + maintBps/10000) — simplified
        // For short: liq_price = entry * (1 + 1/leverage - maintBps/10000)
        const maintBps = getMaintenanceBps(state.leverageValue);
        const maintFrac = maintBps / 10000;
        if (l) l.textContent = formatPrice(state.marginSide === 'long'
            ? state.lastPrice * (1 - 1 / state.leverageValue + maintFrac)
            : state.lastPrice * (1 + 1 / state.leverageValue - maintFrac));
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
                el('poolTvl', formatVolume(data.tvl || data.total_volume || 0));
                el('poolVolume24h', formatVolume(data.volume_24h || 0));
                el('poolFees24h', formatVolume(data.fees_24h || data.total_fees || 0));
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
                        const pair = `${escapeHtml(p.tokenASymbol || 'Token A')}/${escapeHtml(p.tokenBSymbol || 'Token B')}`;
                        const feeBps = parseInt(p.feeTier) || 30;
                        const fee = (feeBps / 100).toFixed(2) + '%';
                        const tvl = formatVolume(p.liquidity || 0);
                        const vol = p.totalVolume ? formatVolume(p.totalVolume) : '—';
                        const apr = p.apr ? p.apr.toFixed(1) + '%' : '—';
                        return `<tr class="pool-row" data-pool-id="${p.poolId || p.id || 0}">
                            <td class="pool-pair"><span class="token-pair-icons"><span class="mini-icon">${escapeHtml((p.tokenASymbol || 'A')[0])}</span><span class="mini-icon">${escapeHtml((p.tokenBSymbol || 'B')[0])}</span></span> ${pair}</td>
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
                const selEvt = () => {
                    const sel = document.getElementById('liqPoolSelect');
                    const pool = state.poolsCache?.find(p => String(p.poolId || p.id) === sel?.value);
                    const priceEl = document.getElementById('liqCurrentPrice');
                    if (pool && pool.sqrtPrice && priceEl) {
                        const sqrtP = pool.sqrtPrice / (1 << 16) / (1 << 16); // Q32.32 → float
                        const price = sqrtP * sqrtP;
                        priceEl.textContent = price >= 0.01 ? price.toFixed(6) : price.toExponential(4);
                    } else if (priceEl) { priceEl.textContent = '—'; }
                };
                const liqSel = document.getElementById('liqPoolSelect');
                if (liqSel) { liqSel.removeEventListener('change', selEvt); liqSel.addEventListener('change', selEvt); selEvt(); }
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
                const container = document.getElementById('pool-positions');
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

    // F8.10/F8.11/F8.12: LP position action handlers via event delegation
    const poolPositionsContainer = document.getElementById('pool-positions');
    if (poolPositionsContainer) poolPositionsContainer.addEventListener('click', async (e) => {
        const collectBtn = e.target.closest('.lp-collect-btn');
        const removeBtn = e.target.closest('.lp-remove-btn');
        const addBtn = e.target.closest('.lp-add-btn');
        if (!collectBtn && !removeBtn && !addBtn) return;
        e.stopPropagation();
        if (!state.connected) { showNotification('Connect wallet first', 'warning'); return; }
        if (!wallet.keypair) { showNotification('Re-import wallet to sign transactions', 'warning'); return; }

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
            const liquidityText = card?.querySelector('.lp-detail:nth-child(2) .mono-value')?.textContent || '0';
            // Parse displayed liquidity back to raw — formatVolume shows $X.XXM/K etc.
            let liqAmount = 0;
            const liqMatch = liquidityText.replace(/[$,]/g, '');
            if (liqMatch.endsWith('M')) liqAmount = parseFloat(liqMatch) * 1e6;
            else if (liqMatch.endsWith('K')) liqAmount = parseFloat(liqMatch) * 1e3;
            else if (liqMatch.endsWith('B')) liqAmount = parseFloat(liqMatch) * 1e9;
            else liqAmount = parseFloat(liqMatch) || 0;
            const rawLiq = Math.round(liqAmount * 1e9);
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
            const deposit = (amtA + amtB) * 1e9; // scale to match liquidity units
            const share = deposit / (pool.liquidity + deposit) * 100;
            shareEl.textContent = share >= 0.01 ? share.toFixed(2) + '%' : '< 0.01%';
        });
    });

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
                        // F10.12 FIX: Compute unrealized PnL for open positions
                        const mark = pos.markPrice || state.lastPrice;
                        const entry = pos.entryPrice || 0;
                        let pnl;
                        if (pos.status === 'closed' || pos.status === 'liquidated') {
                            pnl = pos.realizedPnl || 0;
                        } else if (entry > 0 && mark > 0) {
                            pnl = side === 'Long' ? (mark - entry) * (pos.size || 0) / PRICE_SCALE : (entry - mark) * (pos.size || 0) / PRICE_SCALE;
                        } else {
                            pnl = 0;
                        }
                        return `<div class="margin-pos-row">
                            <div class="margin-pos-info">
                                <span class="${sideClass}">${escapeHtml(side)} ${escapeHtml(pos.pair || 'MOLT/mUSD')}</span>
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
                        // AUDIT-FIX F10.3: Close margin position via signed sendTransaction
                        if (!wallet.keypair) { showNotification('Re-import wallet to sign', 'warning'); return; }
                        btn.disabled = true;
                        try {
                            await wallet.sendTransaction([contractIx(
                                contracts.dex_margin,
                                buildClosePositionArgs(wallet.address, parseInt(btn.dataset.positionId))
                            )]);
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
                    data.map(tr => `<tr><td>${escapeHtml(state.activePair?.id || '')}</td><td class="side-${tr.side || 'buy'}">${escapeHtml((tr.side || 'buy').toUpperCase())}</td><td class="mono-value">${formatPrice(tr.price || 0)}</td><td class="mono-value">${formatAmount(tr.quantity || tr.amount || 0)}</td><td class="mono-value">${formatPrice((tr.price || 0) * (tr.quantity || tr.amount || 0))}</td><td class="mono-value" style="color:var(--text-muted)">${tr.timestamp ? new Date(tr.timestamp).toLocaleString() : ''}</td></tr>`).join('')
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
                        // F10.12 FIX: Compute unrealized PnL from entry vs mark price
                        const mark = p.markPrice || state.lastPrice;
                        const entry = p.entryPrice || 0;
                        const size = p.size || 0;
                        let pnl;
                        if (p.status === 'closed' || p.status === 'liquidated') {
                            pnl = p.realizedPnl || 0;
                        } else if (entry > 0 && mark > 0) {
                            pnl = side === 'Long' ? (mark - entry) * size / PRICE_SCALE : (entry - mark) * size / PRICE_SCALE;
                        } else {
                            pnl = 0;
                        }
                        return `<tr><td>${escapeHtml(p.pair || state.activePair?.id || '')}</td><td class="side-${side.toLowerCase()}">${escapeHtml(side)}</td><td class="mono-value">${formatAmount(p.size || 0)}</td><td class="mono-value">${formatPrice(p.entryPrice || 0)}</td><td class="mono-value">${formatPrice(mark)}</td><td class="mono-value ${pnl >= 0 ? 'positive' : 'negative'}">${pnl >= 0 ? '+' : ''}${formatPrice(pnl)}</td><td>${p.leverage || '2'}x</td><td><button class="btn btn-small btn-secondary">Close</button></td></tr>`;
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
        if (!wallet.keypair) { showNotification('Re-import wallet to sign transactions', 'warning'); return; }
        const size = parseFloat(document.getElementById('marginSize')?.value) || 0;
        const margin = parseFloat(document.getElementById('marginAmount')?.value) || 0;
        if (!size || !margin) { showNotification('Enter size and margin', 'warning'); return; }
        const pairSelect = document.getElementById('marginPairSelect');
        const pairId = pairSelect ? parseInt(pairSelect.value) : 0;
        marginOpenBtn.disabled = true; marginOpenBtn.textContent = 'Opening...';
        try {
            // AUDIT-FIX F10.3: Open margin position via signed sendTransaction (not unsigned REST)
            await wallet.sendTransaction([contractIx(
                contracts.dex_margin,
                buildOpenPositionArgs(wallet.address, pairId, state.marginSide, Math.round(size * 1e9), state.leverageValue, Math.round(margin * 1e9))
            )]);
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
                        // AUDIT-FIX F10.8: Escape all user-submitted proposal text
                        const safeTitle = escapeHtml(p.title || p.description || 'Proposal');
                        const safeDesc = escapeHtml(p.description || '');
                        const safeType = escapeHtml(p.proposalType || 'New Pair');
                        const safeStatus = escapeHtml(status.charAt(0).toUpperCase() + status.slice(1));
                        const safeTime = escapeHtml(p.timeRemaining || '');
                        return `<div class="proposal-card ${statusClass}" data-proposal-id="${p.proposalId || p.id || 0}">
                            <div class="proposal-top-row">
                                <div class="proposal-status-badge ${status}">${safeStatus}</div>
                                <span class="proposal-type-tag">${safeType}</span>
                                <span class="proposal-id">#${p.proposalId || p.id || 0}</span>
                            </div>
                            <h4>${safeTitle}</h4>
                            <p class="proposal-desc text-secondary">${safeDesc}</p>
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
            if (!wallet.keypair) { showNotification('Re-import wallet to sign', 'warning'); return; }
            const card = btn.closest('.proposal-card');
            const pid = card?.dataset?.proposalId;
            const title = card?.querySelector('h4')?.textContent || '';
            btn.disabled = true; btn.style.opacity = '0.5';
            try {
                // AUDIT-FIX F10.6: Vote via signed sendTransaction (not unsigned REST) with real token weight
                const moltBalance = Math.round((balances.MOLT?.available || 0) * 1e9);
                if (moltBalance <= 0) { showNotification('No MOLT balance to vote with', 'warning'); return; }
                if (pid) {
                    await wallet.sendTransaction([contractIx(
                        contracts.dex_governance,
                        buildVoteArgs(wallet.address, parseInt(pid), btn.classList.contains('vote-for'))
                    )]);
                }
            } catch (e) { showNotification(`Vote failed: ${e.message}`, 'error'); return; }
            showNotification(`Vote submitted on "${escapeHtml(title)}"`, 'success');
        }));
    }

    // Proposal type toggle
    const proposalTypeBtns = document.querySelectorAll('.proposal-type-btn');
    const pairFields = document.getElementById('pairFields');
    const feeFields = document.getElementById('feeFields');
    const delistFields = document.getElementById('delistFields');
    const paramFields = document.getElementById('paramFields');
    proposalTypeBtns.forEach(btn => btn.addEventListener('click', () => {
        proposalTypeBtns.forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        const ptype = btn.dataset.ptype;
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
        if (!wallet.keypair) { showNotification('Re-import wallet to sign transactions', 'warning'); return; }
        const activeType = document.querySelector('.proposal-type-btn.active');
        const ptype = activeType?.dataset?.ptype || 'pair';
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
                proposalData.pair = document.getElementById('propFeePair')?.value || 'MOLT/mUSD';
                proposalData.maker_fee = parseInt(document.getElementById('propMakerFee')?.value) || -1;
                proposalData.taker_fee = parseInt(document.getElementById('propTakerFee')?.value) || 5;
            } else if (ptype === 'delist') {
                // F10E.5: Delist proposal with pair selection + reason
                const delistPair = document.getElementById('propDelistPair')?.value;
                const delistReason = document.getElementById('propDelistReason')?.value?.trim();
                if (!delistPair) { showNotification('Select a pair to delist', 'warning'); submitProposalBtn.disabled = false; submitProposalBtn.innerHTML = '<i class="fas fa-paper-plane"></i> Submit Proposal'; return; }
                if (!delistReason) { showNotification('Provide a reason for delisting', 'warning'); submitProposalBtn.disabled = false; submitProposalBtn.innerHTML = '<i class="fas fa-paper-plane"></i> Submit Proposal'; return; }
                const pair = pairs.find(p => String(p.pairId) === delistPair);
                proposalData.pair_id = parseInt(delistPair);
                proposalData.pair_symbol = pair?.id || `Pair#${delistPair}`;
                proposalData.reason = delistReason;
            } else if (ptype === 'param') {
                // F10E.5: Parameter change proposal
                const paramName = document.getElementById('propParamName')?.value;
                const paramValue = document.getElementById('propParamValue')?.value;
                if (!paramName || paramValue === '' || paramValue === undefined) { showNotification('Select parameter and enter new value', 'warning'); submitProposalBtn.disabled = false; submitProposalBtn.innerHTML = '<i class="fas fa-paper-plane"></i> Submit Proposal'; return; }
                const opt = document.getElementById('propParamName')?.options[document.getElementById('propParamName')?.selectedIndex];
                proposalData.parameter = paramName;
                proposalData.current_value = opt?.dataset?.current || '';
                proposalData.proposed_value = paramValue;
            }
            // Build binary args based on proposal type
            let govArgs;
            if (ptype === 'pair' && proposalData.base_token && proposalData.quote_token) {
                // opcode 1: propose_new_pair(proposer, base_token_address, quote_token_address)
                // NOTE: base/quote are token symbols, need address lookup — use generic JSON path for now
                govArgs = new TextEncoder().encode(JSON.stringify(proposalData));
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
            } else if (ptype === 'delist' && proposalData.pair_id) {
                // opcode 10: emergency_delist(admin, pair_id)
                const buf = new ArrayBuffer(41);
                const v = new DataView(buf);
                const a = new Uint8Array(buf);
                writeU8(a, 0, 10);
                writePubkey(a, 1, wallet.address);
                writeU64LE(v, 33, proposalData.pair_id);
                govArgs = a;
            } else {
                govArgs = new TextEncoder().encode(JSON.stringify(proposalData));
            }
            await wallet.sendTransaction([contractIx(contracts.dex_governance, govArgs)]);
            if (ptype === 'pair') {
                showNotification(`Proposal submitted: List ${escapeHtml(proposalData.base_token)}/${escapeHtml(proposalData.quote_token)}`, 'success');
            } else if (ptype === 'fee') {
                showNotification(`Fee change proposal submitted for ${escapeHtml(proposalData.pair)}`, 'success');
            } else if (ptype === 'delist') {
                showNotification(`Delist proposal submitted for ${escapeHtml(proposalData.pair_symbol)}`, 'success');
            } else if (ptype === 'param') {
                showNotification(`Parameter change proposal: ${escapeHtml(proposalData.parameter)} → ${escapeHtml(proposalData.proposed_value)}`, 'success');
            } else {
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
                        <button class="btn btn-small btn-predict-sell" data-outcome="no" data-market="${m.id}">Buy</button>
                    </div>`;
            }

            const statusClass = isResolved ? 'resolved' : m.status === 'disputed' ? 'disputed' : 'active';
            const statusLabel = isResolved ? 'Resolved' : m.status === 'disputed' ? 'Disputed' : 'Active';
            const catTag = catIconsHtml[m.cat] || '<i class="fas fa-chart-pie"></i> ' + escapeHtml(m.cat || 'Other');
            const idTag = m.pm_id || `#PM-${String(m.id).padStart(3, '0')}`;
            const closesLabel = m.closes ? `<span><i class="fas fa-clock"></i> ${escapeHtml(m.closes)}</span>` : '';
            const creatorLabel = m.creator ? `<span><i class="fas fa-user"></i> Creator: ${escapeHtml(m.creator)}</span>` : '';
            const volLabel = formatVolume(m.volume);
            const liqLabel = formatVolume(m.liquidity);

            // AUDIT-FIX F10.5: Show resolve button if user is creator and market is active
            const isCreator = m.creator && wallet.address && m.creator === wallet.address;
            const resolveBtn = (!isResolved && isCreator) ? `<button class="btn btn-small btn-predict-resolve" data-market="${m.id}" style="background:var(--warning,#ffd166);color:#000;margin-left:8px;" title="Resolve this market"><i class="fas fa-gavel"></i> Resolve</button>` : '';

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

        // Apply default selection highlight
        const selCard = document.querySelector(`.market-card[data-market-id="${predictState.selectedMarket}"]`);
        if (selCard) selCard.classList.add('selected');
    }

    // ─── Render user positions in bottom panel ──────────────────
    function renderPredictionPositions() {
        const tbody = document.getElementById('predictPositionsBody');
        if (!tbody) return;
        if (!predictState.positions.length) {
            tbody.innerHTML = '<tr><td colspan="6" style="text-align:center;color:var(--text-muted)">No positions found</td></tr>';
            return;
        }
        // AUDIT-FIX F10.5: Show claim button for positions in resolved markets
        tbody.innerHTML = predictState.positions.map(p => {
            const m = predictState.markets.find(x => x.id === p.market_id);
            const qText = escapeHtml(m?.question?.slice(0, 40) || `Market #${p.market_id}`);
            const isResolved = m?.status === 'resolved';
            const won = isResolved && ((m.resolved_outcome === 'yes' && p.outcome === 0) || (m.resolved_outcome === 'no' && p.outcome === 1));
            const actionCol = isResolved
                ? (won ? `<button class="btn btn-small btn-predict-claim-pos" data-market="${p.market_id}" style="background:var(--accent);color:#fff;font-size:0.75rem;"><i class="fas fa-gift"></i> Claim</button>` : '<span style="color:var(--text-muted)">Lost</span>')
                : '<span style="color:var(--text-muted)">Active</span>';
            return `<tr><td>${qText}...</td><td>${p.outcome === 0 ? 'YES' : 'NO'}</td><td>${p.shares.toFixed(2)}</td><td>$${p.cost_basis.toFixed(2)}</td><td>${actionCol}</td></tr>`;
        }).join('');

        // Bind claim buttons in positions table
        tbody.querySelectorAll('.btn-predict-claim-pos').forEach(btn => btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            if (!wallet.keypair) { showNotification('Re-import wallet to sign transactions', 'warning'); return; }
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

        // AUDIT-FIX F10.5: Resolve market button (creator only)
        document.querySelectorAll('.btn-predict-resolve').forEach(btn => btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            if (!wallet.keypair) { showNotification('Re-import wallet to sign transactions', 'warning'); return; }
            const mid = parseInt(btn.dataset.market);
            const m = predictState.markets.find(x => x.id === mid);
            if (!m) return;
            // Prompt for resolution outcome
            const outcome = prompt(`Resolve "${m.question}"?\n\nEnter the winning outcome: "yes" or "no"`);
            if (!outcome || !['yes', 'no'].includes(outcome.toLowerCase())) { showNotification('Invalid outcome — enter "yes" or "no"', 'warning'); return; }
            btn.disabled = true; btn.textContent = 'Resolving...';
            try {
                const winIdx = outcome.toLowerCase() === 'yes' ? 0 : 1;
                await wallet.sendTransaction([contractIx(contracts.prediction_market, buildResolveMarketArgs(wallet.address, mid, winIdx))]);
                showNotification(`Market resolved: ${outcome.toUpperCase()} wins`, 'success');
                await loadPredictionMarkets();
            } catch (err) { showNotification(`Resolve failed: ${err.message}`, 'error'); }
            btn.disabled = false; btn.innerHTML = '<i class="fas fa-gavel"></i> Resolve';
        }));

        // AUDIT-FIX F10.5: Claim winnings on resolved markets
        document.querySelectorAll('.btn-predict-claim').forEach(btn => btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            if (!wallet.keypair) { showNotification('Re-import wallet to sign transactions', 'warning'); return; }
            const mid = parseInt(btn.dataset.market);
            btn.disabled = true; btn.textContent = 'Claiming...';
            try {
                const cardPos = predictState.positions?.find(p => p.market_id === mid);
                const cardOutcome = cardPos ? cardPos.outcome : 0;
                await wallet.sendTransaction([contractIx(contracts.prediction_market, buildRedeemSharesArgs(wallet.address, mid, cardOutcome))]);
                showNotification('Prediction winnings claimed!', 'success');
            } catch (err) { showNotification(`Claim failed: ${err.message}`, 'error'); }
            btn.disabled = false; btn.innerHTML = '<i class="fas fa-gift"></i> Claim Winnings';
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
        if (!wallet.keypair) { showNotification('Re-import wallet to sign transactions', 'warning'); return; }
        const amt = parseFloat(document.getElementById('predictAmount')?.value) || 0;
        if (amt < 1) { showNotification('Enter amount (min $1)', 'warning'); return; }
        const m = predictState.markets.find(x => x.id === predictState.selectedMarket);
        if (!m) return;
        predictSubmitBtn.disabled = true; predictSubmitBtn.textContent = 'Submitting...';
        try {
            // AUDIT-FIX F10.4: Prediction trade via signed sendTransaction (not unsigned REST)
            const outcomeVal = predictState.selectedOutcome === 'yes' ? 0 : 1;
            await wallet.sendTransaction([contractIx(contracts.prediction_market, buildBuySharesArgs(wallet.address, m.id, outcomeVal, Math.round(amt * 1e9)))]);
            showNotification(`Bought ${predictState.selectedOutcome.toUpperCase()} on "${escapeHtml(m.question.slice(0, 40))}..." for $${amt.toFixed(2)}`, 'success');
        } catch (e) { showNotification(`Trade failed: ${e.message}`, 'error'); }
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
        if (!wallet.keypair) { showNotification('Re-import wallet to sign transactions', 'warning'); return; }
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
            // AUDIT-FIX F10.4: Create market via signed sendTransaction
            const catVal = document.getElementById('predictCategory')?.value || 'general';
            const ocCount = outcomes.length > 0 ? outcomes.length : 2;
            await wallet.sendTransaction([contractIx(contracts.prediction_market, buildCreateMarketArgs(wallet.address, q, catVal, ocCount))]);
            showNotification(`Market created: "${escapeHtml(q.slice(0, 50))}..." with $${liq} liquidity`, 'success');
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
        renderPredictionMarkets();
        showNotification(`Sorted by ${predictSort.options[predictSort.selectedIndex].text}`, 'info');
    });

    // ═══════════════════════════════════════════════════════════════════════
    // Governance + Rewards — claim handlers
    // ═══════════════════════════════════════════════════════════════════════
    document.querySelectorAll('.claim-btn, .btn-claim').forEach(btn => btn.addEventListener('click', async () => {
        if (!state.connected) { showNotification('Connect wallet to claim', 'warning'); return; }
        if (!wallet.keypair) { showNotification('Re-import wallet to sign transactions', 'warning'); return; }
        btn.disabled = true; const origText = btn.innerHTML; btn.textContent = 'Claiming...';
        try {
            // AUDIT-FIX F10.7: Reward claim via signed sendTransaction (not fake GET)
            await wallet.sendTransaction([contractIx(contracts.dex_rewards, buildClaimRewardsArgs(wallet.address))]);
            showNotification('Rewards claimed successfully!', 'success');
        } catch (e) { showNotification(`Claim failed: ${e.message}`, 'error'); }
        btn.disabled = false; btn.innerHTML = origText;
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
        // AUDIT-FIX F10.10: Load contract addresses before any operations
        await loadContractAddresses();
        await loadPairs();
        renderPairList(); renderBalances(); renderOpenOrders(); updateSubmitBtn();
        applyWalletGateAll(); // F10E.1: Apply wallet-gate to all forms on load
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
        // F10E.7: Connect Binance price feed for wSOL/wETH alignment
        connectBinancePriceFeed();
        if (savedWallets.length) {
            const l = savedWallets[savedWallets.length - 1];
            // AUDIT-FIX F10.11: Auto-connect is display-only — no keypair stored.
            // Show "(view only)" indicator so user knows signing is disabled.
            state.connected = true; state.walletAddress = l.address;
            wallet.address = l.address;
            const shortAddr = l.short || l.address.slice(0, 8) + '...';
            if (connectBtn) {
                connectBtn.innerHTML = `<i class="fas fa-wallet"></i> ${shortAddr} <span style="font-size:0.65rem;opacity:0.7;margin-left:4px;">(view only)</span>`;
                connectBtn.className = 'btn btn-small btn-secondary';
                connectBtn.title = 'View-only mode — click to import keypair for signing';
            }
            toggleWalletPanels(true);
            applyWalletGateAll(); // F10E.1: Re-apply wallet-gate after auto-connect
            try { await loadBalances(l.address); await loadUserOrders(l.address); } catch { /* API unavailable */ }
            renderBalances(); renderOpenOrders(); loadTradeHistory(); loadPositionsTab();
            if (dexWs && state.activePairId != null) subscribePair(state.activePairId);
        }
    })().catch(e => console.error('[DEX] Init error:', e));

    // F6.12: Clean up WebSocket connections on page unload
    window.addEventListener('beforeunload', () => {
        if (dexWs) dexWs.close();
    });
});
