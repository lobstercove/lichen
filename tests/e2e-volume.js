#!/usr/bin/env node
/**
 * Lichen Volume Simulation E2E Test Suite
 *
 * Simulates realistic trading activity across the DEX with multiple wallets:
 *   1. Multi-wallet parallel trading (5 wallets, cross-matched orders)
 *   2. Multi-pair volume sweep (trades on all 5 pairs)
 *   3. LP lifecycle — add liquidity, swap, collect fees, remove
 *   4. Margin trading — open long/short, close
 *   5. Prediction market — create, buy shares, verify positions
 *   6. Analytics verification — 24h stats, candles, ticker updates
 *   7. WebSocket live event verification
 *   8. Orderbook depth stress test (10+ orders per side)
 *   9. Router swap test
 *  10. Governance flow — propose, vote
 *
 * Usage:
 *   node tests/e2e-volume.js
 *
 * Prerequisites:
 *   - Validator running with --dev-mode on port 8899
 *   - DEX contracts deployed (genesis auto-deploy)
 */
'use strict';

const pq = require('./helpers/pq-node');
const { loadFundedWallets, findGenesisAdminKeypair } = require('./helpers/funded-wallets');
const { waitForSuccessfulTransaction } = require('./helpers/tx-receipt');
const { encodeNativeTransactionBase64, signNativeTransaction } = require('./helpers/tx-wire');
const { setupDexEnvironment } = require('./helpers/dex-setup');

let WebSocket;
try { WebSocket = require('ws'); }
catch { WebSocket = null; console.warn('ws module not found — WebSocket tests will be skipped'); }

const RPC_URL = process.env.LICHEN_RPC || 'http://127.0.0.1:8899';
const REST_BASE = `${RPC_URL}/api/v1`;
const WS_URL = process.env.LICHEN_WS || RPC_URL.replace('https://', 'wss://').replace('http://', 'ws://').replace(':8899', ':8900');
const PRICE_SCALE = 1_000_000_000;  // 1 LICN = 1e9 spores
const PM_SCALE = 1_000_000_000;     // 1 lUSD = 10^9 base units

// ═══════════════════════════════════════════════════════════════════════════════
// Test harness
// ═══════════════════════════════════════════════════════════════════════════════
let passed = 0, failed = 0, skipped = 0;
function assert(cond, msg) {
    if (cond) {
        passed++;
        process.stdout.write(`  ✓ ${msg}\n`);
        return;
    }

    failed++;
    process.stderr.write(`  ✗ ${msg}\n`);
}
function assertEq(a, b, msg) { assert(a === b, `${msg} (expected ${b}, got ${a})`); }
function assertGt(a, b, msg) { assert(a > b, `${msg} (${a} > ${b})`); }
function assertGte(a, b, msg) { assert(a >= b, `${msg} (${a} >= ${b})`); }
function skip(msg) { skipped++; process.stdout.write(`  ⊘ ${msg} [SKIPPED]\n`); }
function section(name) { console.log(`\n── ${name} ──`); }

// ═══════════════════════════════════════════════════════════════════════════════
// Base58
// ═══════════════════════════════════════════════════════════════════════════════
const BS58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
function bs58encode(bytes) {
    let lz = 0; for (let i = 0; i < bytes.length && bytes[i] === 0; i++) lz++;
    let num = 0n; for (const b of bytes) num = num * 256n + BigInt(b);
    let enc = ''; while (num > 0n) { enc = BS58[Number(num % 58n)] + enc; num /= 58n; }
    return '1'.repeat(lz) + enc;
}
function bs58decode(str) {
    let num = 0n;
    for (const c of str) { const i = BS58.indexOf(c); if (i < 0) throw new Error(`Bad b58: ${c}`); num = num * 58n + BigInt(i); }
    const hex = num === 0n ? '' : num.toString(16); const padded = hex.length % 2 ? '0' + hex : hex;
    const bytes = []; for (let i = 0; i < padded.length; i += 2) bytes.push(parseInt(padded.slice(i, i + 2), 16));
    let lo = 0; for (let i = 0; i < str.length && str[i] === '1'; i++) lo++;
    const r = new Uint8Array(lo + bytes.length); r.set(bytes, lo); return r;
}
function bytesToHex(b) { return Array.from(b).map(x => x.toString(16).padStart(2, '0')).join(''); }
function hexToBytes(h) {
    const c = h.startsWith('0x') ? h.slice(2) : h;
    const o = new Uint8Array(c.length / 2);
    for (let i = 0; i < o.length; i++) o[i] = parseInt(c.slice(i * 2, i * 2 + 2), 16);
    return o;
}

// ═══════════════════════════════════════════════════════════════════════════════
// RPC client
// ═══════════════════════════════════════════════════════════════════════════════
let rpcId = 1;
async function rpc(method, params = []) {
    const res = await fetch(RPC_URL, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', id: rpcId++, method, params }),
    });
    const json = await res.json();
    if (json.error) throw new Error(`RPC ${json.error.code}: ${json.error.message}`);
    return json.result;
}
async function rest(path) {
    const res = await fetch(`${REST_BASE}${path}`);
    if (!res.ok) return null;
    return res.json();
}
async function restPost(path, body) {
    const res = await fetch(`${REST_BASE}${path}`, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
    });
    if (!res.ok) return null;
    return res.json();
}
const sleep = ms => new Promise(r => setTimeout(r, ms));

function apiRows(payload, nestedKey = null) {
    if (Array.isArray(payload)) return payload;
    if (Array.isArray(payload?.data)) return payload.data;
    if (nestedKey && Array.isArray(payload?.data?.[nestedKey])) return payload.data[nestedKey];
    if (nestedKey && Array.isArray(payload?.[nestedKey])) return payload[nestedKey];
    return [];
}

// ═══════════════════════════════════════════════════════════════════════════════
// WebSocket-based transaction confirmation (signatureSubscribe — push, no poll)
// ═══════════════════════════════════════════════════════════════════════════════
function confirmViaWs(signature, timeoutMs = 30000) {
    if (!WebSocket) return confirmViaRpc(signature, timeoutMs);
    return new Promise((resolve, reject) => {
        const ws = new WebSocket(WS_URL);
        const timer = setTimeout(() => { try { ws.close(); } catch { } reject(new Error(`WS confirm timeout ${timeoutMs}ms`)); }, timeoutMs);
        ws.on('error', () => { clearTimeout(timer); resolve(null); }); // fallback silently
        ws.on('open', () => {
            ws.send(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'signatureSubscribe', params: [signature] }));
        });
        ws.on('message', (data) => {
            try {
                const msg = JSON.parse(data.toString());
                if (msg.id === 1 && msg.result !== undefined) return; // ack
                if (msg.params?.result?.signature === signature || msg.params?.result?.event === 'SignatureStatus') {
                    clearTimeout(timer); try { ws.close(); } catch { } resolve(msg.params.result); return;
                }
                if (msg.method === 'notification' && msg.params?.subscription) {
                    clearTimeout(timer); try { ws.close(); } catch { } resolve(msg.params.result || { confirmed: true }); return;
                }
            } catch { }
        });
    });
}

function confirmViaRpc(signature, timeoutMs = 20000, pollMs = 500) {
    return (async () => {
        const started = Date.now();
        while ((Date.now() - started) < timeoutMs) {
            try {
                const tx = await rpc('getTransaction', [signature]);
                if (tx) return tx;
            } catch { /* retry */ }
            await sleep(pollMs);
        }
        return null;
    })();
}

async function waitForTx(signature, timeoutMs = 30000) {
    try {
        return await confirmViaWs(signature, timeoutMs);
    } catch {
        return await confirmViaRpc(signature, timeoutMs);
    }
}

async function pollRest(path, predicate, timeoutMs = 25000, pollMs = 500) {
    const started = Date.now();
    let last = null;
    while ((Date.now() - started) < timeoutMs) {
        last = await rest(path);
        if (last && predicate(last)) return last;
        await sleep(pollMs);
    }
    return last;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Wallet helpers
// ═══════════════════════════════════════════════════════════════════════════════
function genKeypair() {
    return pq.generateKeypair();
}

// ═══════════════════════════════════════════════════════════════════════════════
// Transaction building & signing
// ═══════════════════════════════════════════════════════════════════════════════
function encodeMsg(instructions, blockhash, signer) {
    const parts = [];
    function pushU64(n) {
        const buf = new ArrayBuffer(8); const v = new DataView(buf);
        v.setUint32(0, n & 0xFFFFFFFF, true); v.setUint32(4, Math.floor(n / 0x100000000) & 0xFFFFFFFF, true);
        parts.push(new Uint8Array(buf));
    }
    pushU64(instructions.length);
    for (const ix of instructions) {
        parts.push(bs58decode(ix.program_id));
        const accts = ix.accounts || [signer];
        pushU64(accts.length);
        for (const a of accts) parts.push(bs58decode(a));
        const d = typeof ix.data === 'string' ? new TextEncoder().encode(ix.data) : new Uint8Array(ix.data);
        pushU64(d.length);
        parts.push(d);
    }
    parts.push(hexToBytes(blockhash));
    parts.push(new Uint8Array([0x00]));  // compute_budget: None
    parts.push(new Uint8Array([0x00]));  // compute_unit_price: None
    const total = parts.reduce((s, a) => s + a.length, 0);
    const out = new Uint8Array(total); let off = 0;
    for (const a of parts) { out.set(a, off); off += a.length; }
    return out;
}

async function sendTx(keypair, instructions) {
    const bhRes = await rpc('getRecentBlockhash');
    const bh = typeof bhRes === 'string' ? bhRes : bhRes.blockhash;
    const nix = instructions.map(ix => ({
        program_id: ix.program_id,
        accounts: ix.accounts || [keypair.address],
        data: typeof ix.data === 'string' ? Array.from(new TextEncoder().encode(ix.data)) : Array.from(ix.data),
    }));
    const msg = encodeMsg(nix, bh, keypair.address);
    const pqSig = signNativeTransaction(pq, keypair, msg);
    const b64 = encodeNativeTransactionBase64(
        [pqSig],
        { instructions: nix, blockhash: bh },
    );
    const txSig = await rpc('sendTransaction', [b64]);
    await waitForSuccessfulTransaction(rpc, txSig, 60_000, 250);
    return txSig;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Contract call helpers
// ═══════════════════════════════════════════════════════════════════════════════
const CONTRACT_PID = bs58encode(new Uint8Array(32).fill(0xFF));
function contractIx(callerAddr, contractAddr, argsBytes, value = 0) {
    const data = JSON.stringify({ Call: { function: "call", args: Array.from(argsBytes), value } });
    return { program_id: CONTRACT_PID, accounts: [callerAddr, contractAddr], data };
}

// ═══════════════════════════════════════════════════════════════════════════════
// Binary encoding helpers
// ═══════════════════════════════════════════════════════════════════════════════
function writeU64LE(view, off, n) { view.setBigUint64(off, BigInt(Math.round(n)), true); }
function writeU8(arr, off, n) { arr[off] = n & 0xFF; }
function writePubkey(arr, off, addr) { arr.set(bs58decode(addr).subarray(0, 32), off); }

// place_order: opcode 2, 67 bytes
function buildPlaceOrder(trader, pairId, side, type, price, qty) {
    const buf = new ArrayBuffer(67); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 2); writePubkey(a, 1, trader);
    writeU64LE(v, 33, pairId); writeU8(a, 41, side === 'buy' ? 0 : 1);
    writeU8(a, 42, type === 'market' ? 1 : 0);
    writeU64LE(v, 43, price); writeU64LE(v, 51, qty); writeU64LE(v, 59, 0);
    return a;
}

// cancel_order: opcode 3, 41 bytes
function buildCancelOrder(trader, orderId) {
    const buf = new ArrayBuffer(41); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 3); writePubkey(a, 1, trader); writeU64LE(v, 33, orderId);
    return a;
}

function buildCancelAllOrders(trader, pairId) {
    const buf = new ArrayBuffer(41); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 17); writePubkey(a, 1, trader); writeU64LE(v, 33, pairId);
    return a;
}

// add_liquidity: opcode 3, 73 bytes
function buildAddLiquidity(provider, poolId, lowerTick, upperTick, amountA, amountB, deadline = 1_000_000_000) {
    const buf = new ArrayBuffer(73); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 3); writePubkey(a, 1, provider); writeU64LE(v, 33, poolId);
    v.setInt32(41, lowerTick, true); v.setInt32(45, upperTick, true);
    writeU64LE(v, 49, amountA); writeU64LE(v, 57, amountB);
    writeU64LE(v, 65, deadline);
    return a;
}

// remove_liquidity: opcode 4, 57 bytes
function buildRemoveLiquidity(provider, posId, amount, deadline = 1_000_000_000) {
    const buf = new ArrayBuffer(57); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 4); writePubkey(a, 1, provider); writeU64LE(v, 33, posId); writeU64LE(v, 41, amount); writeU64LE(v, 49, deadline);
    return a;
}

// collect_fees: opcode 5, 41 bytes
function buildCollectFees(provider, posId) {
    const buf = new ArrayBuffer(41); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 5); writePubkey(a, 1, provider); writeU64LE(v, 33, posId);
    return a;
}

// open_position (margin): opcode 2, 66 bytes
function buildOpenPosition(trader, pairId, side, size, leverage, margin) {
    const buf = new ArrayBuffer(66); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 2); writePubkey(a, 1, trader);
    writeU64LE(v, 33, pairId); writeU8(a, 41, side === 'long' ? 0 : 1);
    writeU64LE(v, 42, size); writeU64LE(v, 50, leverage); writeU64LE(v, 58, margin);
    return a;
}

// close_position (margin): opcode 3, 41 bytes
function buildClosePosition(trader, posId) {
    const buf = new ArrayBuffer(41); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 3); writePubkey(a, 1, trader); writeU64LE(v, 33, posId);
    return a;
}

// buy_shares (prediction): opcode 4, 50 bytes
function buildBuyShares(buyer, marketId, outcome, amount) {
    const buf = new ArrayBuffer(50); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 4); writePubkey(a, 1, buyer);
    writeU64LE(v, 33, marketId); writeU8(a, 41, outcome);
    writeU64LE(v, 42, amount);
    return a;
}

// vote: opcode 2, 42 bytes
function buildVote(voter, proposalId, inFavor) {
    const buf = new ArrayBuffer(42); const v = new DataView(buf); const a = new Uint8Array(buf);
    writeU8(a, 0, 2); writePubkey(a, 1, voter); writeU64LE(v, 33, proposalId);
    writeU8(a, 41, inFavor ? 1 : 0);
    return a;
}

// propose_new_pair: opcode 1, 97 bytes
function buildProposeNewPair(proposer, baseToken, quoteToken) {
    const buf = new ArrayBuffer(97); const a = new Uint8Array(buf);
    writeU8(a, 0, 1); writePubkey(a, 1, proposer);
    writePubkey(a, 33, baseToken); writePubkey(a, 65, quoteToken);
    return a;
}

// claim_rewards: opcode 2, 33 bytes
function buildClaimRewards(claimer) {
    const buf = new ArrayBuffer(33); const a = new Uint8Array(buf);
    writeU8(a, 0, 2); writePubkey(a, 1, claimer);
    return a;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Dynamic contract discovery
// ═══════════════════════════════════════════════════════════════════════════════
const CONTRACTS = {};
async function discoverContracts() {
    const result = await rpc('getAllSymbolRegistry', [{ limit: 100 }]);
    const entries = result?.entries || [];
    const symbolMap = {
        'DEX': 'dex_core', 'DEXAMM': 'dex_amm', 'DEXROUTER': 'dex_router',
        'DEXMARGIN': 'dex_margin', 'DEXREWARDS': 'dex_rewards', 'DEXGOV': 'dex_governance',
        'ANALYTICS': 'dex_analytics', 'PREDICT': 'prediction_market',
        'LUSD': 'lusd_token', 'WSOL': 'wsol_token', 'WETH': 'weth_token',
        'WBNB': 'wbnb_token', 'WNEO': 'wneo_token', 'WGAS': 'wgas_token',
        'WBTC': 'wbtc_token', 'ORACLE': 'lichenoracle', 'YID': 'lichenid',
    };
    for (const e of entries) {
        const key = symbolMap[e.symbol] || e.symbol.toLowerCase();
        CONTRACTS[key] = e.program;
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helper: Place order and wait
// ═══════════════════════════════════════════════════════════════════════════════
async function placeOrder(wallet, pairId, side, price, qty, label = '') {
    const args = buildPlaceOrder(wallet.address, pairId, side, 'limit', price, qty);
    try {
        const pair = (await rest(`/pairs/${pairId}`))?.data || {};
        const base = String(pair.baseSymbol || pair.base || '').toUpperCase();
        const quote = String(pair.quoteSymbol || pair.quote || '').toUpperCase();
        const notional = Number((BigInt(price) * BigInt(qty) + BigInt(PRICE_SCALE - 1)) / BigInt(PRICE_SCALE));
        const takerFeeBps = BigInt(Number(pair.takerFeeBps ?? pair.taker_fee_bps ?? 0));
        const takerFee = Number((BigInt(notional) * takerFeeBps) / 10_000n) || 1;
        const value = side === 'sell' && base === 'LICN'
            ? qty
            : (side === 'buy' && quote === 'LICN' ? notional + takerFee : 0);
        const sig = await sendTx(wallet, [contractIx(wallet.address, CONTRACTS.dex_core, args, value)]);
        assert(typeof sig === 'string' && sig.length > 0, `${label || side} order placed: ${sig.slice(0, 16)}...`);
        return sig;
    } catch (e) {
        assert(false, `${label || side} order failed: ${e.message}`);
        return null;
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helper: Fund wallet with airdrop
// ═══════════════════════════════════════════════════════════════════════════════
async function fundWallet(wallet, amount, label) {
    const existing = await rpc('getBalance', [wallet.address]);
    const existingSpendable = Number(existing?.spendable ?? 0);
    if (existingSpendable >= 5 * PRICE_SCALE) {
        assert(true, `${label} funded via genesis balance (${(existingSpendable / PRICE_SCALE).toFixed(2)} LICN)`);
        return;
    }

    try {
        const r = await rpc('requestAirdrop', [wallet.address, amount]);
        assert(r.success === true, `${label} airdrop: ${amount} LICN`);
    } catch (e) {
        const msg = String(e.message || '');
        const isDisabled = msg.includes('requestAirdrop is disabled in multi-validator mode');
        const isRateLimit = msg.includes('Airdrop rate limit');
        if (isDisabled || isRateLimit) {
            const b = await rpc('getBalance', [wallet.address]);
            const bal = Number(b?.spendable ?? 0);
            if (bal > 0) {
                assert(true, `${label} funded via genesis balance (${(bal / PRICE_SCALE).toFixed(2)} LICN)`);
            } else {
                assert(false, `${label} not funded (${isRateLimit ? 'rate limited' : 'disabled'})`);
            }
        } else {
            assert(false, `${label} airdrop failed: ${msg}`);
        }
    }
}

async function selectRichestFundedWallets(limit) {
    const funded = loadFundedWallets(Math.max(limit * 3, limit))
        .filter((wallet) => !/community_treasury/i.test(String(wallet.source || '')));
    const ranked = [];

    for (const wallet of funded) {
        try {
            const [balance, reputation] = await Promise.all([
                rpc('getBalance', [wallet.address]),
                rpc('getLichenIdReputation', [wallet.address]).catch(() => null),
            ]);
            ranked.push({
                ...wallet,
                _initialSpendable: Number(balance?.spendable ?? 0),
                _reputation: Number(reputation?.score ?? reputation?.reputation ?? 0),
            });
        } catch {
            ranked.push({ ...wallet, _initialSpendable: 0, _reputation: 0 });
        }
    }

    ranked.sort((left, right) => (
        Number(right._reputation >= 500) - Number(left._reputation >= 500)
        || right._initialSpendable - left._initialSpendable
        || left.address.localeCompare(right.address)
    ));

    return ranked.slice(0, limit);
}

// ═══════════════════════════════════════════════════════════════════════════════
// MAIN TEST SUITE
// ═══════════════════════════════════════════════════════════════════════════════
async function runTests() {
    await pq.init();
    console.log(`\n═══════════════════════════════════════════════════════`);
    console.log(`  Lichen Volume Simulation E2E Suite`);
    console.log(`  RPC: ${RPC_URL}  WS: ${WS_URL}`);
    console.log(`═══════════════════════════════════════════════════════\n`);

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 0: Setup — Discover contracts, create & fund wallets
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 0: Setup — Contract Discovery');
    await discoverContracts();
    const expectedContracts = ['dex_core', 'dex_amm', 'dex_router', 'dex_margin', 'dex_rewards', 'dex_governance', 'dex_analytics', 'prediction_market'];
    for (const c of expectedContracts) {
        assert(!!CONTRACTS[c], `Contract ${c}: ${CONTRACTS[c] || 'MISSING'}`);
    }

    section('Phase 0: Setup — 5 Trader Wallets');
    const wallets = [];
    const names = ['Alice', 'Bob', 'Carol', 'Dave', 'Eve'];
    const funded = await selectRichestFundedWallets(5);
    for (let i = 0; i < 5; i++) {
        const w = funded[i] || genKeypair();
        w.name = names[i];
        wallets.push(w);
        console.log(`  ${names[i]}: ${w.address.slice(0, 12)}...`);
    }
    if (funded.length >= 5) {
        assert(true, 'Loaded richest funded genesis wallets');
    }

    // Fund all wallets (stagger airdrops with unique amounts to avoid rate limits)
    for (let i = 0; i < wallets.length; i++) {
        await fundWallet(wallets[i], 10, wallets[i].name);
        // Small delay between airdrops for different addresses
        await sleep(200);
    }
    await sleep(4000); // Wait for block propagation

    await setupDexEnvironment({
        rpcUrl: RPC_URL,
        wallets,
        contracts: CONTRACTS,
        adminKeypair: findGenesisAdminKeypair(),
        skipFunding: false,
        requireMinting: true,
        requireApprovals: true,
        marginInsuranceLusd: 10_000,
        requireMarginInsurance: true,
        targetLicn: 10,
    });

    const setupPairs = await rest('/pairs');
    for (const wallet of wallets) {
        for (const pair of setupPairs?.data || []) {
            const pairId = Number(pair.pairId ?? pair.id ?? 0);
            if (pairId <= 0) continue;
            await sendTx(wallet, [contractIx(
                wallet.address,
                CONTRACTS.dex_core,
                buildCancelAllOrders(wallet.address, pairId),
            )]);
        }
    }

    // Verify all balances
    section('Phase 0: Verify Balances');
    let fundedWalletCount = 0;
    const writeEligibleWallets = new Set();
    for (const w of wallets) {
        const b = await rpc('getBalance', [w.address]);
        if (b.spendable >= 5 * PRICE_SCALE) {
            fundedWalletCount++;
            writeEligibleWallets.add(w.address);
            assert(true, `${w.name} has ≥5 LICN (${b.spendable_licn})`);
        } else {
            assert(false, `${w.name} funding below write-path threshold (${b.spendable_licn})`);
        }
    }
    const writePathEnabled = fundedWalletCount >= 2;

    // Snapshot initial analytics
    const preStats = await rest('/pairs/1/ticker');
    const preTrades = preStats?.data?.trades24h || 0;
    const preVol = preStats?.data?.volume24h || 0;
    console.log(`  Pre-test pair 1 stats: ${preTrades} trades, volume=${preVol}`);

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 1: Multi-Wallet Trading on LICN/lUSD (Pair 1)
    //
    // Simulates 5 traders placing orders at different price levels.
    // Creates a realistic orderbook with depth and executes matches.
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 1: Multi-Wallet Trading — LICN/lUSD');

    const [alice, bob, carol, dave, eve] = wallets;

    let expectedMatches = 0;

    // Round 1: Alice sells, Bob buys → match at $0.105
    {
        const price = Math.round(0.105 * PRICE_SCALE);
        const qty = Math.round(10 * PRICE_SCALE);
        const sellSig = await placeOrder(alice, 1, 'sell', price, qty, 'Alice sell @0.105');
        await sleep(1500);
        const buySig = await placeOrder(bob, 1, 'buy', price, qty, 'Bob buy @0.105');
        if (sellSig && buySig) expectedMatches++;
        await sleep(2000);
    }

    // Round 2: Carol sells, Dave buys → match at $0.11
    {
        const price = Math.round(0.11 * PRICE_SCALE);
        const qty = Math.round(8 * PRICE_SCALE);
        const sellSig = await placeOrder(carol, 1, 'sell', price, qty, 'Carol sell @0.11');
        await sleep(1500);
        const buySig = await placeOrder(dave, 1, 'buy', price, qty, 'Dave buy @0.11');
        if (sellSig && buySig) expectedMatches++;
        await sleep(2000);
    }

    // Round 3: Eve sells, Alice buys → match at $0.115
    {
        const price = Math.round(0.115 * PRICE_SCALE);
        const qty = Math.round(6 * PRICE_SCALE);
        const sellSig = await placeOrder(eve, 1, 'sell', price, qty, 'Eve sell @0.115');
        await sleep(1500);
        const buySig = await placeOrder(alice, 1, 'buy', price, qty, 'Alice buy @0.115');
        if (sellSig && buySig) expectedMatches++;
        await sleep(2000);
    }

    // Round 4: Bob sells, Carol buys → match at $0.108
    {
        const price = Math.round(0.108 * PRICE_SCALE);
        const qty = Math.round(12 * PRICE_SCALE);
        const sellSig = await placeOrder(bob, 1, 'sell', price, qty, 'Bob sell @0.108');
        await sleep(1500);
        const buySig = await placeOrder(carol, 1, 'buy', price, qty, 'Carol buy @0.108');
        if (sellSig && buySig) expectedMatches++;
        await sleep(2000);
    }

    // Round 5: Dave sells, Eve buys → match at $0.112
    {
        const price = Math.round(0.112 * PRICE_SCALE);
        const qty = Math.round(15 * PRICE_SCALE);
        const sellSig = await placeOrder(dave, 1, 'sell', price, qty, 'Dave sell @0.112');
        await sleep(1500);
        const buySig = await placeOrder(eve, 1, 'buy', price, qty, 'Eve buy @0.112');
        if (sellSig && buySig) expectedMatches++;
        await sleep(2000);
    }

    // Verify: Trades appeared in history (eventual-consistency polling)
    if (writePathEnabled && expectedMatches > 0) {
        const trades1 = await pollRest(
            '/pairs/1/trades',
            (resp) => Boolean(resp?.data?.length > 0),
            30000,
            500,
        );
        assert(trades1?.data?.length > 0, `Pair 1 has trade history (${trades1?.data?.length || 0} trades)`);
    } else {
        assert(false, 'Pair 1 had no successful matched writes');
    }

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 2: Orderbook Depth Stress Test
    //
    // Place 10 sell orders and 10 buy orders at different prices.
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 2: Orderbook Depth Stress');
    {
        // Keep depth inside the pair's ±50% oracle protection band.
        for (let i = 0; i < 10; i++) {
            const seller = wallets[i % wallets.length];
            const displayPrice = 0.105 + i * 0.004;
            const price = Math.round(displayPrice * PRICE_SCALE);
            const qty = Math.round((0.2 + i * 0.1) * PRICE_SCALE);
            await placeOrder(seller, 1, 'sell', price, qty, `Sell wall ${i + 1} @${displayPrice.toFixed(3)}`);
            await sleep(500);
        }

        for (let i = 0; i < 10; i++) {
            const buyer = wallets[i % wallets.length];
            const displayPrice = 0.095 - i * 0.004;
            const price = Math.round(displayPrice * PRICE_SCALE);
            const qty = Math.round((0.2 + i * 0.1) * PRICE_SCALE);
            await placeOrder(buyer, 1, 'buy', price, qty, `Buy wall ${i + 1} @${displayPrice.toFixed(3)}`);
            await sleep(500);
        }

        const ob = await pollRest(
            '/pairs/1/orderbook?depth=20',
            (resp) => Boolean(resp?.data?.asks?.length >= 5 && resp?.data?.bids?.length >= 5),
            25000,
            500,
        );
        assert(ob?.data?.asks?.length >= 5, `Orderbook has ≥5 asks (${ob?.data?.asks?.length || 0})`);
        assert(ob?.data?.bids?.length >= 5, `Orderbook has ≥5 bids (${ob?.data?.bids?.length || 0})`);

        // Verify spread
        if (ob?.data?.asks?.length > 0 && ob?.data?.bids?.length > 0) {
            const bestAsk = parseFloat(ob.data.asks[0].price);
            const bestBid = parseFloat(ob.data.bids[0].price);
            assert(bestAsk > bestBid, `Spread is positive: ask=${bestAsk} > bid=${bestBid}`);
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 3: Multi-Pair Volume Sweep
    //
    // Execute at least one trade on each available pair.
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 3: Multi-Pair Volume Sweep');
    const pairsResp = await rest('/pairs');
    const pairCount = pairsResp?.data?.length || 0;
    assert(pairCount >= 3, `At least 3 trading pairs available (${pairCount})`);

    // Define prices per pair (approximate genesis/oracle prices)
    const pairPrices = {
        1: 0.10,     // LICN/lUSD
        2: 80.0,     // wSOL/lUSD
        3: 1950.0,   // wETH/lUSD
        4: 800.0,    // wSOL/LICN
        5: 19000.0,  // wETH/LICN
    };

    for (let pid = 1; pid <= Math.min(pairCount, 5); pid++) {
        const basePrice = pairPrices[pid] || 1.0;
        const price = Math.round(basePrice * PRICE_SCALE);
        // Small quantities for non-LICN pairs
        const qty = pid <= 1 ? Math.round(5 * PRICE_SCALE) : Math.round(0.01 * PRICE_SCALE);

        // Alternate wallets for maker/taker
        const maker = wallets[(pid - 1) % wallets.length];
        const taker = wallets[pid % wallets.length];

        await placeOrder(maker, pid, 'sell', price, qty, `Pair ${pid} sell @${basePrice}`);
        await sleep(1500);
        await placeOrder(taker, pid, 'buy', price, qty, `Pair ${pid} buy @${basePrice}`);
        await sleep(2000);
    }

    // Check tickers for each pair
    for (let pid = 1; pid <= Math.min(pairCount, 5); pid++) {
        const ticker = await rest(`/pairs/${pid}/ticker`);
        assert(ticker?.data != null, `Pair ${pid} ticker exists`);
        if (ticker?.data) {
            console.log(`    Pair ${pid}: price=${ticker.data.lastPrice}, trades=${ticker.data.trades24h}, vol=${ticker.data.volume24h}`);
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 4: LP Lifecycle
    //
    // Add liquidity → verify position → remove liquidity
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 4: LP Lifecycle');
    {
        const provider = alice;
        const poolId = 1;
        const amountA = Math.round(20 * PRICE_SCALE);
        const amountB = Math.round(2 * PRICE_SCALE);
        const args = buildAddLiquidity(provider.address, poolId, -44400, 44400, amountA, amountB);

        try {
            const sig = await sendTx(provider, [contractIx(provider.address, CONTRACTS.dex_amm, args, amountA)]);
            assert(typeof sig === 'string', `Alice added liquidity to pool ${poolId}: ${sig?.slice(0, 16)}...`);
        } catch (e) {
            assert(false, `Add liquidity failed: ${e.message}`);
        }
        await sleep(3000);

        // Verify pools endpoint
        const pools = await rest('/pools');
        assert(pools?.data != null, 'Pools API responds');

        // Verify LP positions
        const positions = await rest(`/pools/positions?owner=${provider.address}`);
        assert(positions?.data != null, `LP positions API responds for Alice`);
        const lpPositions = Array.isArray(positions?.data) ? positions.data : [];
        assert(lpPositions.length > 0, 'LP position exists after adding liquidity');
        const lpPosition = [...lpPositions].sort((left, right) => (
            Number(right.id ?? right.positionId ?? right.position_id ?? 0)
            - Number(left.id ?? left.positionId ?? left.position_id ?? 0)
        ))[0];
        const lpPositionId = Number(lpPosition?.id ?? lpPosition?.positionId ?? lpPosition?.position_id ?? 0);
        assert(lpPositionId > 0, `LP position ID resolved (${lpPositionId})`);

        // Collect fees attempt
        try {
            const feeSig = await sendTx(provider, [contractIx(provider.address, CONTRACTS.dex_amm, buildCollectFees(provider.address, lpPositionId))]);
            assert(typeof feeSig === 'string', `Fee collection tx submitted`);
        } catch (e) {
            assert(false, `Fee collection failed: ${e.message}`);
        }
        await sleep(2000);

        // Remove partial liquidity
        try {
            const availableShares = Number(lpPosition?.liquidity ?? lpPosition?.shares ?? 5 * PRICE_SCALE);
            const removeShares = Math.max(1, Math.min(Math.round(5 * PRICE_SCALE), availableShares));
            const rmArgs = buildRemoveLiquidity(provider.address, lpPositionId, removeShares);
            const rmSig = await sendTx(provider, [contractIx(provider.address, CONTRACTS.dex_amm, rmArgs)]);
            assert(typeof rmSig === 'string', `Partial liquidity removal tx submitted`);
        } catch (e) {
            assert(false, `Remove liquidity failed: ${e.message}`);
        }
        await sleep(2000);
    }

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 5: Margin Trading
    //
    // Open long/short positions → verify → close
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 5: Margin Trading');
    {
        // Bob opens a 5x long on LICN/lUSD
        const trader = bob;
        const pairId = 1;
        const size = Math.round(10 * PRICE_SCALE);
        const leverage = 5;
        const marginDeposit = Math.round(2 * PRICE_SCALE);

        try {
            const args = buildOpenPosition(trader.address, pairId, 'long', size, leverage, marginDeposit);
            const sig = await sendTx(trader, [contractIx(trader.address, CONTRACTS.dex_margin, args)]);
            assert(typeof sig === 'string', `Bob opened 5x long on pair ${pairId}: ${sig?.slice(0, 16)}...`);
        } catch (e) {
            assert(false, `Open long failed: ${e.message}`);
        }
        await sleep(2000);

        // Carol opens a 3x short
        try {
            const args2 = buildOpenPosition(carol.address, pairId, 'short', Math.round(5 * PRICE_SCALE), 3, Math.round(2 * PRICE_SCALE));
            const sig2 = await sendTx(carol, [contractIx(carol.address, CONTRACTS.dex_margin, args2)]);
            assert(typeof sig2 === 'string', `Carol opened 3x short on pair ${pairId}: ${sig2?.slice(0, 16)}...`);
        } catch (e) {
            assert(false, `Open short failed: ${e.message}`);
        }
        await sleep(2000);

        // Verify margin positions API
        const bobPos = await rest(`/margin/positions?trader=${trader.address}`);
        assert(bobPos?.data != null, 'Margin positions API responds for Bob');
        const activeBobPositions = (bobPos?.data || []).filter((position) => (
            ['open', 'active', '1'].includes(String(position.status ?? 'open').toLowerCase())
        ));
        const bobPosition = [...activeBobPositions].sort((left, right) => (
            Number(right.id ?? right.positionId ?? right.position_id ?? 0)
            - Number(left.id ?? left.positionId ?? left.position_id ?? 0)
        ))[0];
        const bobPositionId = Number(bobPosition?.id ?? bobPosition?.positionId ?? bobPosition?.position_id ?? 0);
        assert(bobPositionId > 0, `Bob open position ID resolved (${bobPositionId})`);

        const marginInfo = await rest('/margin/info');
        assert(marginInfo?.data != null, 'Margin info API responds');

        // Close Bob's position
        try {
            const closeSig = await sendTx(trader, [contractIx(trader.address, CONTRACTS.dex_margin, buildClosePosition(trader.address, bobPositionId))]);
            assert(typeof closeSig === 'string', `Bob closed position: ${closeSig?.slice(0, 16)}...`);
        } catch (e) {
            assert(false, `Close position failed: ${e.message}`);
        }
        await sleep(2000);
    }

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 6: Prediction Markets
    //
    // Create market → buy shares → verify positions → check analytics
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 6: Prediction Markets');
    {
        // Check existing markets
        const mkts = await rest('/prediction-market/markets');
        assert(mkts != null, 'Prediction markets API responds');
        const initialMarkets = apiRows(mkts, 'markets').length;
        console.log(`    Initial markets: ${initialMarkets}`);

        // setupDexEnvironment creates and activates prediction markets through
        // signed contract transactions; REST write endpoints are quote/build APIs.
        const mktsAfter = await rest('/prediction-market/markets');
        const marketRows = apiRows(mktsAfter, 'markets');
        const availableMarkets = marketRows.length;
        assert(availableMarkets > 0, `Prediction market setup created active markets (${availableMarkets})`);
        if (availableMarkets > 0) {
            const mktId = marketRows[0].id || marketRows[0].market_id || 1;
            // Alice buys YES shares
            try {
                const buyArgs = buildBuyShares(alice.address, mktId, 0, 5 * PM_SCALE);
                const buySig = await sendTx(alice, [contractIx(alice.address, CONTRACTS.prediction_market, buyArgs)]);
                assert(typeof buySig === 'string', `Alice bought YES shares on market ${mktId}: ${buySig?.slice(0, 16)}...`);
            } catch (e) {
                assert(false, `Buy YES shares failed: ${e.message}`);
            }
            await sleep(2000);

            // Bob buys NO shares
            try {
                const buyArgs2 = buildBuyShares(bob.address, mktId, 1, 3 * PM_SCALE);
                const buySig2 = await sendTx(bob, [contractIx(bob.address, CONTRACTS.prediction_market, buyArgs2)]);
                assert(typeof buySig2 === 'string', `Bob bought NO shares on market ${mktId}: ${buySig2?.slice(0, 16)}...`);
            } catch (e) {
                assert(false, `Buy NO shares failed: ${e.message}`);
            }
            await sleep(2000);

            // Carol and Dave also buy shares for volume
            try {
                const buyArgs3 = buildBuyShares(carol.address, mktId, 0, 8 * PM_SCALE);
                await sendTx(carol, [contractIx(carol.address, CONTRACTS.prediction_market, buyArgs3)]);
                assert(true, `Carol bought YES shares on market ${mktId}`);
            } catch (e) { assert(false, `Carol buy failed: ${e.message}`); }

            try {
                const buyArgs4 = buildBuyShares(dave.address, mktId, 1, 4 * PM_SCALE);
                await sendTx(dave, [contractIx(dave.address, CONTRACTS.prediction_market, buyArgs4)]);
                assert(true, `Dave bought NO shares on market ${mktId}`);
            } catch (e) { assert(false, `Dave buy failed: ${e.message}`); }
            await sleep(2000);

            // Verify positions
            const alicePos = await rest(`/prediction-market/positions?address=${alice.address}`);
            assert(alicePos != null, `Prediction positions API responds for Alice`);

            // Check market detail
            const mktDetail = await rest(`/prediction-market/markets/${mktId}`);
            assert(mktDetail?.data != null, `Market ${mktId} detail API responds`);
        }

        // Prediction stats
        const pmStats = await rest('/prediction-market/stats');
        assert(pmStats != null, 'Prediction market stats API responds');

        // Trending markets
        const trending = await rest('/prediction-market/trending');
        assert(trending != null, 'Prediction trending API responds');

        // Leaderboard
        const pmLb = await rest('/prediction-market/leaderboard');
        assert(pmLb != null, 'Prediction leaderboard API responds');
    }

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 7: Governance — Propose & Vote
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 7: Governance — Propose & Vote');
    {
        const proposalsBefore = await rest('/governance/proposals');
        const previousIds = new Set((proposalsBefore?.data || []).map(
            (proposal) => Number(proposal.proposalId ?? proposal.id ?? proposal.proposal_id ?? 0),
        ));

        // Dave proposes a new base asset against the configured lUSD quote.
        if (CONTRACTS.lusd_token) {
            try {
                const propArgs = buildProposeNewPair(dave.address, genKeypair().address, CONTRACTS.lusd_token);
                const propSig = await sendTx(dave, [contractIx(dave.address, CONTRACTS.dex_governance, propArgs)]);
                assert(typeof propSig === 'string', `Dave proposed new pair: ${propSig?.slice(0, 16)}...`);
            } catch (e) {
                assert(false, `Propose new pair failed: ${e.message}`);
            }
            await sleep(2000);
        }

        // Check proposals
        const proposals = await rest('/governance/proposals');
        assert(proposals != null, 'Governance proposals API responds');
        const propCount = proposals?.data?.length || 0;
        console.log(`    Active proposals: ${propCount}`);

        // Vote if there are proposals
        if (propCount > 0) {
            const createdProposal = (proposals.data || [])
                .filter((proposal) => !previousIds.has(Number(proposal.proposalId ?? proposal.id ?? proposal.proposal_id ?? 0)))
                .sort((left, right) => Number(right.proposalId ?? right.id ?? right.proposal_id ?? 0) - Number(left.proposalId ?? left.id ?? left.proposal_id ?? 0))[0];
            const propId = Number(createdProposal?.proposalId ?? createdProposal?.id ?? createdProposal?.proposal_id ?? 0);
            assert(propId > 0, `New governance proposal ID resolved (${propId})`);
            for (const voter of [alice, bob, carol]) {
                try {
                    const voteArgs = buildVote(voter.address, propId, true);
                    await sendTx(voter, [contractIx(voter.address, CONTRACTS.dex_governance, voteArgs)]);
                    assert(true, `${voter.name} voted YES on proposal ${propId}`);
                } catch (e) {
                    assert(false, `${voter.name} vote failed: ${e.message}`);
                }
                await sleep(1000);
            }
        } else {
            assert(false, 'No governance proposal available to vote on');
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 8: Rewards Claim
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 8: Rewards');
    {
        for (const trader of [alice, bob]) {
            const rewardsBefore = await rest(`/rewards/${trader.address}`);
            const pending = Number(rewardsBefore?.data?.pending || 0) + Number(rewardsBefore?.data?.lpPending || 0);
            try {
                const claimArgs = buildClaimRewards(trader.address);
                const claimSig = await sendTx(trader, [contractIx(trader.address, CONTRACTS.dex_rewards, claimArgs)]);
                assert(pending > 0 && typeof claimSig === 'string', `${trader.name} claimed ${pending} reward units: ${claimSig?.slice(0, 16)}...`);
            } catch (e) {
                assert(pending === 0, `${trader.name} reward claim failed with ${pending} pending: ${e.message}`);
                if (pending === 0) {
                    assert(/failure code 1|no claimable|returned zero/i.test(String(e.message)), `${trader.name} zero-reward claim rejected explicitly`);
                }
            }
            await sleep(1500);
        }

        // Verify rewards endpoints
        const aliceRew = await rest(`/rewards/${alice.address}`);
        assert(aliceRew != null, 'Rewards API responds for Alice');

        const rewStats = await rest('/stats/rewards');
        assert(rewStats != null, 'Rewards stats API responds');
    }

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 9: Router Swap Test
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 9: Router Swap');
    {
        // Quote (may return 405 — placeholder endpoint)
        let quote = null;
        try {
            const quoteRes = await fetch(`${REST_BASE}/router/quote`, {
                method: 'POST', headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ tokenIn: 'LICN', tokenOut: 'lUSD', amountIn: Math.round(1 * PRICE_SCALE), slippage: 1.0 }),
            });
            if (quoteRes.ok) quote = await quoteRes.json();
        } catch (e) { /* endpoint may not exist */ }
        assert(quote != null, 'Router quote API responds');
        if (quote?.data) {
            console.log(`    Quote: ${JSON.stringify(quote.data).slice(0, 80)}`);
        }

        // Routes
        const routes = await rest('/routes');
        assert(routes != null, 'Router routes API responds');
    }

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 10: Analytics Verification
    //
    // Verify 24h stats, candles, tickers updated after volume simulation
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 10: Analytics Verification');
    {
        // Wait a moment for bridge to catch up
        await sleep(3000);

        // 24h stats for pair 1
        const stats1 = await rest('/pairs/1/stats');
        assert(stats1?.data != null, 'Pair 1 24h stats API responds');
        if (stats1?.data) {
            assertGt(Number(stats1.data.volume || 0), 0, 'Pair 1 24h volume is populated');
            assertGt(Number(stats1.data.tradeCount || 0), 0, 'Pair 1 24h trade count is populated');
            assertGte(Number(stats1.data.high || 0), Number(stats1.data.low || 0), 'Pair 1 high is at least its low');
            console.log(`    Pair 1 stats: vol=${stats1.data.volume}, trades=${stats1.data.tradeCount}, high=${stats1.data.high}, low=${stats1.data.low}`);
        }

        // Ticker — should show updated values
        const ticker1 = await rest('/pairs/1/ticker');
        assert(ticker1?.data != null, 'Pair 1 ticker API responds');
        if (ticker1?.data) {
            const newTrades = ticker1.data.trades24h || 0;
            const newVol = ticker1.data.volume24h || 0;
            assertGt(newTrades, preTrades, `Trade count increased: ${preTrades} → ${newTrades}`);
            console.log(`    Ticker: price=${ticker1.data.lastPrice}, trades=${newTrades}, vol=${newVol}`);
        }

        // Candles — interval is seconds: 300=5m, 3600=1h
        for (const [label, secs] of [['5m', 300], ['1h', 3600]]) {
            const candles = await rest(`/pairs/1/candles?interval=${secs}&limit=5`);
            assert(candles?.data != null, `Pair 1 ${label} candles API responds`);
            assert((candles?.data?.length || 0) > 0, `Pair 1 ${label} candles contain trades`);
            if (candles?.data?.length > 0) {
                console.log(`    ${label} candles: ${candles.data.length} entries, latest close=${candles.data[candles.data.length - 1].close}`);
            }
        }

        // All tickers endpoint
        const allTickers = await rest('/tickers');
        assert(allTickers?.data != null, 'All tickers endpoint responds');

        // Oracle prices
        const oraclePrices = await rest('/oracle/prices');
        assert(oraclePrices != null, 'Oracle prices API responds');

        // Leaderboard
        const lb = await rest('/leaderboard');
        assert(lb != null, 'Trading leaderboard API responds');

        // Analytics stats
        const anaStats = await rest('/stats/analytics');
        assert(anaStats != null, 'Analytics stats API responds');

        // Core stats
        const coreStats = await rest('/stats/core');
        assert(coreStats != null, 'DEX core stats API responds');

        // AMM stats
        const ammStats = await rest('/stats/amm');
        assert(ammStats != null, 'AMM stats API responds');
    }

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 11: WebSocket Live Events
    //
    // Connect WS, subscribe to DEX channels, verify events arrive
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 11: WebSocket Live Events');
    if (WebSocket) {
        const wsResult = await new Promise((resolve) => {
            const ws = new WebSocket(WS_URL);
            const received = { connected: false, slot: false, trade: false, ticker: false, orderbook: false };
            let slotSubscription = null;
            let triggerComplete = false;
            let settled = false;
            const finish = (error = null) => {
                if (settled) return;
                settled = true;
                clearTimeout(timer);
                try { ws.close(); } catch { }
                resolve({ received, error });
            };
            const timer = setTimeout(() => finish('timed out waiting for live DEX events'), 20_000);

            ws.on('open', async () => {
                received.connected = true;
                ws.send(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'subscribeSlots' }));
                ws.send(JSON.stringify({ jsonrpc: '2.0', id: 2, method: 'subscribeDex', params: { channel: 'trades:1' } }));
                ws.send(JSON.stringify({ jsonrpc: '2.0', id: 3, method: 'subscribeDex', params: { channel: 'ticker:1' } }));
                ws.send(JSON.stringify({ jsonrpc: '2.0', id: 4, method: 'subscribeDex', params: { channel: 'orderbook:1' } }));
                try {
                    await sleep(1500);
                    const price = Math.round(0.106 * PRICE_SCALE);
                    const qty = Math.round(2 * PRICE_SCALE);
                    const sell = await placeOrder(eve, 1, 'sell', price, qty, 'WS trigger: Eve sell');
                    await sleep(500);
                    const buy = await placeOrder(dave, 1, 'buy', price, qty, 'WS trigger: Dave buy');
                    if (!sell || !buy) {
                        finish('failed to submit matched WS trigger trade');
                        return;
                    }
                    triggerComplete = true;
                    if (Object.values(received).every(Boolean)) finish();
                } catch (error) {
                    finish(error.message);
                }
            });

            ws.on('message', (data) => {
                let msg;
                try { msg = JSON.parse(data.toString()); }
                catch { finish('received non-JSON WebSocket message'); return; }
                if (msg.error) {
                    finish(`subscription error: ${msg.error.message || JSON.stringify(msg.error)}`);
                    return;
                }
                if (msg.id === 1 && typeof msg.result === 'number') {
                    slotSubscription = msg.result;
                }
                if (msg.method === 'subscription') {
                    const type = String(msg.params?.result?.type || '');
                    if (msg.params?.subscription === slotSubscription && Number(msg.params?.result?.slot) >= 0) received.slot = true;
                    if (type === 'tradeExecution') received.trade = true;
                    if (type === 'tickerUpdate') received.ticker = true;
                    if (type === 'orderBookUpdate') received.orderbook = true;
                }
                if (triggerComplete && Object.values(received).every(Boolean)) finish();
            });
            ws.on('error', (error) => finish(error.message));
            ws.on('close', () => {
                if (!settled) finish('connection closed before all live events arrived');
            });
        });
        assert(!wsResult.error, `WebSocket live flow completed${wsResult.error ? `: ${wsResult.error}` : ''}`);
        assert(wsResult.received.connected, 'WS: Connected');
        assert(wsResult.received.slot, 'WS: Received slot notification');
        assert(wsResult.received.trade, 'WS: Received trade execution');
        assert(wsResult.received.ticker, 'WS: Received ticker update');
        assert(wsResult.received.orderbook, 'WS: Received orderbook update');
    } else {
        assert(false, 'ws module is required for WebSocket user-flow coverage');
    }

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 12: Per-Trader Stats
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 12: Per-Trader Stats');
    for (const w of [alice, bob]) {
        const ts = await rest(`/traders/${w.address}/stats`);
        assert(ts != null, `Trader stats API responds for ${w.name}`);
        if (ts?.data) {
            assertGt(Number(ts.data.totalVolume || 0), 0, `${w.name} trader volume is populated`);
            assertGt(Number(ts.data.tradeCount || 0), 0, `${w.name} trader count is populated`);
            console.log(`    ${w.name}: volume=${ts.data.totalVolume || 0}, trades=${ts.data.tradeCount || 0}`);
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // PHASE 13: Final Balance Check
    //
    // All wallets should still have enough LICN to be usable
    // ══════════════════════════════════════════════════════════════════════
    section('Phase 13: Final Balance Check');
    for (const w of wallets) {
        const b = await rpc('getBalance', [w.address]);
        if (writeEligibleWallets.has(w.address)) {
            assert(b.spendable > 0, `${w.name} balance > 0 (${b.spendable_licn})`);
        } else {
            assert(false, `${w.name} final balance check unavailable (below initial write-path threshold)`);
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // RESULTS
    // ══════════════════════════════════════════════════════════════════════
    console.log(`\n═══════════════════════════════════════════════════════`);
    console.log(`  Volume Simulation Results`);
    console.log(`  ✓ Passed:  ${passed}`);
    console.log(`  ✗ Failed:  ${failed}`);
    console.log(`  ⊘ Skipped: ${skipped}`);
    console.log(`  Total:     ${passed + failed + skipped}`);
    console.log(`═══════════════════════════════════════════════════════\n`);

    process.exit(failed > 0 || skipped > 0 ? 1 : 0);
}

runTests().catch(e => {
    console.error(`\nFATAL: ${e.message}\n${e.stack}`);
    process.exit(1);
});
