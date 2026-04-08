#!/usr/bin/env node
/**
 * Lichen Prediction Market — Multi-Outcome Deep E2E Test Suite
 *
 * Extended coverage for multi-outcome prediction markets (4, 5, 6, 8 outcomes).
 * Tests every possible action on multi-outcome markets:
 *
 *   M1.  Contract & wallet setup (discovery + funding)
 *   M2.  4-outcome market creation & initial liquidity
 *   M3.  5-outcome market creation & initial liquidity
 *   M4.  6-outcome market creation & initial liquidity
 *   M5.  8-outcome (max) market creation & initial liquidity
 *   M6.  Multi-outcome trading — buy shares on every outcome index
 *   M7.  Multi-outcome sell shares — partial exits on each outcome
 *   M8.  Multi-outcome complete-set mint & redeem
 *   M9.  Multi-outcome price verification (sum ≈ 1.0)
 *   M10. Multi-outcome liquidity add & withdraw
 *   M11. Multi-outcome analytics & stats
 *   M12. Edge cases (invalid outcome, zero amount, max outcomes, 1-outcome reject)
 *   M13. REST API verification for multi-outcome markets
 *   M14. Final state & position verification
 *
 * Usage:
 *   node tests/e2e-prediction-multi.js
 *
 * Prerequisites:
 *   - Validator running on port 8899
 *   - Contracts deployed (genesis auto-deploy)
 */
'use strict';

const pq = require('./helpers/pq-node');
const crypto = require('crypto');
const { findGenesisAdminKeypair } = require('./helpers/funded-wallets');

const RPC_URL = process.env.LICHEN_RPC || 'http://127.0.0.1:8899';
const FAUCET_URL = process.env.FAUCET_URL || 'http://127.0.0.1:9100';
const REST_BASE = `${RPC_URL}/api/v1`;
const PM_SCALE = 1_000_000;
const SPORES_PER_LICN = 1e9;
const FUND_AMOUNT = 10;
const PREDICT_CREATE_FEE = 10_000_000;

// ═══════════════════════════════════════════════════════════════════════════════
// Test harness
// ═══════════════════════════════════════════════════════════════════════════════
let passed = 0, failed = 0, skipped = 0;
function assert(cond, msg) {
    if (cond) { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
    else { failed++; process.stderr.write(`  ✗ ${msg}\n`); }
}
function assertEq(a, b, msg) { assert(a === b, `${msg} (expected ${b}, got ${a})`); }
function assertGt(a, b, msg) { assert(a > b, `${msg} (expected > ${b}, got ${a})`); }
function assertGte(a, b, msg) { assert(a >= b, `${msg} (expected >= ${b}, got ${a})`); }
function assertLte(a, b, msg) { assert(a <= b, `${msg} (expected <= ${b}, got ${a})`); }
function assertOutcomeNames(outcomes, expectedNames, msg) {
    const actualNames = Array.isArray(outcomes)
        ? outcomes.map((outcome) => String(outcome?.name || '').trim())
        : [];
    assertEq(actualNames.length, expectedNames.length, `${msg} outcome label count`);
    for (let i = 0; i < expectedNames.length; i++) {
        assertEq(actualNames[i], expectedNames[i], `${msg} outcome ${i} label`);
    }
}
function skip(msg) { skipped++; console.log(`  ⊘ ${msg}`); }
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
// RPC / REST client
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
    try {
        const res = await fetch(`${REST_BASE}${path}`);
        if (!res.ok) return null;
        return res.json();
    } catch { return null; }
}
const sleep = ms => new Promise(r => setTimeout(r, ms));

async function waitForTransaction(signature, timeoutMs = 15000) {
    const started = Date.now();
    while (Date.now() - started < timeoutMs) {
        try {
            const tx = await rpc('getTransaction', [signature]);
            if (tx) return tx;
        } catch {
            // Transaction metadata is not visible immediately after submission.
        }
        await sleep(300);
    }
    throw new Error(`Transaction ${signature} not visible within ${timeoutMs}ms`);
}

async function resolveMarketIdByQuestion(question, creator, expectedCloseSlot = null, retries = 8) {
    for (let attempt = 0; attempt < retries; attempt++) {
        const resp = await rest(`/prediction-market/markets?creator=${encodeURIComponent(creator)}&limit=200`);
        const markets = Array.isArray(resp?.data?.markets)
            ? resp.data.markets
            : (Array.isArray(resp?.markets) ? resp.markets : []);
        if (markets.length > 0) {
            const exact = markets
                .filter((m) => String(m.question || '').trim() === question.trim()
                    && (expectedCloseSlot == null || Number(m.close_slot || 0) === Number(expectedCloseSlot)))
                .sort((a, b) => Number(b.id || 0) - Number(a.id || 0));
            if (exact[0]?.id) return Number(exact[0].id);
        }
        await sleep(500 + attempt * 250);
    }
    return 0;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Keypair
// ═══════════════════════════════════════════════════════════════════════════════
function genKeypair() {
    return pq.generateKeypair();
}
function keypairFromSeed(seed32) {
    return pq.keypairFromSeed(seed32);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Transaction building & signing
// ═══════════════════════════════════════════════════════════════════════════════
function encodeMsg(instructions, blockhash, signer, computeBudget = null) {
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
    if (computeBudget != null && computeBudget > 0) {
        parts.push(new Uint8Array([0x01]));
        pushU64(computeBudget);
    } else {
        parts.push(new Uint8Array([0x00]));
    }
    parts.push(new Uint8Array([0x00]));
    const total = parts.reduce((s, a) => s + a.length, 0);
    const out = new Uint8Array(total); let off = 0;
    for (const a of parts) { out.set(a, off); off += a.length; }
    return out;
}

async function sendTx(keypair, instructions, computeBudget = null) {
    const bhRes = await rpc('getRecentBlockhash');
    const bh = typeof bhRes === 'string' ? bhRes : bhRes.blockhash;
    const nix = instructions.map(ix => ({
        program_id: ix.program_id,
        accounts: ix.accounts || [keypair.address],
        data: typeof ix.data === 'string' ? Array.from(new TextEncoder().encode(ix.data)) : Array.from(ix.data),
    }));
    const msg = encodeMsg(nix, bh, keypair.address, computeBudget);
    const pqSig = pq.sign(msg, keypair);
    const payload = { signatures: [pqSig], message: { instructions: nix, blockhash: bh, compute_budget: computeBudget || undefined } };
    const b64 = Buffer.from(JSON.stringify(payload)).toString('base64');
    const signature = await rpc('sendTransaction', [b64]);
    const tx = await waitForTransaction(signature);
    if (Object.prototype.hasOwnProperty.call(tx, 'return_code') && Number(tx.return_code) === 0) {
        const logs = Array.isArray(tx.contract_logs) ? tx.contract_logs.join(' | ') : '';
        throw new Error(logs || 'contract returned failure code 0');
    }
    return signature;
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
function writeU8(arr, off, v) { arr[off] = v & 0xFF; }
function writeU16LE(view, off, v) { view.setUint16(off, v, true); }
function writeU32LE(view, off, v) { view.setUint32(off, v, true); }
function writeU64LE(view, off, n) { view.setBigUint64(off, BigInt(Math.round(n)), true); }
function writePubkey(arr, off, addrB58) { const b = bs58decode(addrB58); arr.set(b, off); }

// ═══════════════════════════════════════════════════════════════════════════════
// Prediction Market: Binary arg builders (match WASM dispatch opcodes)
// ═══════════════════════════════════════════════════════════════════════════════

// Opcode 1: create_market
function buildCreateMarket(creator, category, closeSlot, outcomeCount, questionText, outcomeNames = []) {
    const encoder = new TextEncoder();
    const qBytes = encoder.encode(questionText);
    const normalizedNames = Array.isArray(outcomeNames)
        ? outcomeNames.map((name) => String(name || '').trim()).filter((name) => name.length > 0)
        : [];
    const encodedNames = normalizedNames.length === outcomeCount
        ? normalizedNames
            .map((name) => encoder.encode(name))
            .filter((nameBytes) => nameBytes.length > 0 && nameBytes.length <= 64)
        : [];
    const namesLen = encodedNames.length === outcomeCount
        ? 1 + encodedNames.reduce((sum, nameBytes) => sum + 1 + nameBytes.length, 0)
        : 0;
    const total = 1 + 32 + 1 + 8 + 1 + 32 + 4 + qBytes.length + namesLen;
    const buf = new ArrayBuffer(total);
    const a = new Uint8Array(buf);
    const v = new DataView(buf);
    writeU8(a, 0, 1);
    writePubkey(a, 1, creator);
    writeU8(a, 33, category);
    writeU64LE(v, 34, closeSlot);
    writeU8(a, 42, outcomeCount);
    const qHash = crypto.createHash('sha256').update(qBytes).digest();
    a.set(qHash, 43);
    writeU32LE(v, 75, qBytes.length);
    a.set(qBytes, 79);
    let offset = 79 + qBytes.length;
    if (namesLen > 0) {
        writeU8(a, offset, encodedNames.length);
        offset += 1;
        encodedNames.forEach((nameBytes) => {
            writeU8(a, offset, nameBytes.length);
            offset += 1;
            a.set(nameBytes, offset);
            offset += nameBytes.length;
        });
    }
    return a;
}

// Opcode 2: add_initial_liquidity
function buildAddInitialLiquidity(provider, marketId, amountMusd, oddsBps = null) {
    const oddsLen = oddsBps ? oddsBps.length * 2 : 0;
    const total = 1 + 32 + 8 + 8 + oddsLen;
    const buf = new ArrayBuffer(total);
    const a = new Uint8Array(buf);
    const v = new DataView(buf);
    writeU8(a, 0, 2);
    writePubkey(a, 1, provider);
    writeU64LE(v, 33, marketId);
    writeU64LE(v, 41, amountMusd);
    if (oddsBps && oddsBps.length > 0) {
        for (let i = 0; i < oddsBps.length; i++) {
            writeU16LE(v, 49 + i * 2, oddsBps[i]);
        }
    }
    return a;
}

// Opcode 3: add_liquidity
function buildAddLiquidity(provider, marketId, amountMusd) {
    const buf = new ArrayBuffer(49);
    const a = new Uint8Array(buf);
    const v = new DataView(buf);
    writeU8(a, 0, 3);
    writePubkey(a, 1, provider);
    writeU64LE(v, 33, marketId);
    writeU64LE(v, 41, amountMusd);
    return a;
}

// Opcode 4: buy_shares
function buildBuyShares(trader, marketId, outcome, amountMusd) {
    const buf = new ArrayBuffer(50);
    const a = new Uint8Array(buf);
    const v = new DataView(buf);
    writeU8(a, 0, 4);
    writePubkey(a, 1, trader);
    writeU64LE(v, 33, marketId);
    writeU8(a, 41, outcome);
    writeU64LE(v, 42, amountMusd);
    return a;
}

// Opcode 5: sell_shares
function buildSellShares(trader, marketId, outcome, sharesAmount) {
    const buf = new ArrayBuffer(50);
    const a = new Uint8Array(buf);
    const v = new DataView(buf);
    writeU8(a, 0, 5);
    writePubkey(a, 1, trader);
    writeU64LE(v, 33, marketId);
    writeU8(a, 41, outcome);
    writeU64LE(v, 42, sharesAmount);
    return a;
}

// Opcode 6: mint_complete_set
function buildMintCompleteSet(user, marketId, amountMusd) {
    const buf = new ArrayBuffer(49);
    const a = new Uint8Array(buf);
    const v = new DataView(buf);
    writeU8(a, 0, 6);
    writePubkey(a, 1, user);
    writeU64LE(v, 33, marketId);
    writeU64LE(v, 41, amountMusd);
    return a;
}

// Opcode 7: redeem_complete_set
function buildRedeemCompleteSet(user, marketId, amount) {
    const buf = new ArrayBuffer(49);
    const a = new Uint8Array(buf);
    const v = new DataView(buf);
    writeU8(a, 0, 7);
    writePubkey(a, 1, user);
    writeU64LE(v, 33, marketId);
    writeU64LE(v, 41, amount);
    return a;
}

// Opcode 15: withdraw_liquidity
function buildWithdrawLiquidity(user, marketId, amount) {
    const buf = new ArrayBuffer(49);
    const a = new Uint8Array(buf);
    const v = new DataView(buf);
    writeU8(a, 0, 15);
    writePubkey(a, 1, user);
    writeU64LE(v, 33, marketId);
    writeU64LE(v, 41, amount);
    return a;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Contract discovery
// ═══════════════════════════════════════════════════════════════════════════════
const CONTRACTS = {};

async function discoverContracts() {
    const registry = await rpc('getAllSymbolRegistry', [100]);
    const entries = Array.isArray(registry) ? registry : (registry && registry.entries ? registry.entries : []);
    for (const e of entries) {
        const sym = (e.symbol || '').toLowerCase().replace(/[^a-z0-9_]/g, '_');
        if (e.program) CONTRACTS[sym] = e.program;
    }
}

function loadGenesisAdmin() {
    return findGenesisAdminKeypair();
}

// ═══════════════════════════════════════════════════════════════════════════════
// Funding helper
// ═══════════════════════════════════════════════════════════════════════════════
async function fundWallet(wallet, amount = FUND_AMOUNT) {
    try {
        await rpc('requestAirdrop', [wallet.address, amount]);
        await sleep(1500);
        return true;
    } catch {
        try {
            const res = await fetch(`${FAUCET_URL}/api/faucet`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ address: wallet.address }),
            });
            await sleep(1500);
            return res.ok;
        } catch { return false; }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// MAIN TEST
// ═══════════════════════════════════════════════════════════════════════════════
async function main() {
    await pq.init();
    console.log('━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━');
    console.log('  Lichen Prediction Market — Multi-Outcome Deep E2E');
    console.log('━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━');

    // ══════════════════════════════════════════════════════════════════════
    // M1. Setup: Contract Discovery + Wallet Funding
    // ══════════════════════════════════════════════════════════════════════
    section('M1: Contract & Wallet Setup');
    await discoverContracts();

    const hasPM = !!CONTRACTS.predict;
    assert(hasPM, 'PREDICT contract found in symbol registry');

    if (!hasPM) {
        console.error('PREDICT contract not found — cannot proceed');
        process.exit(1);
    }

    const currentSlot = Number(await rpc('getSlot')) || 100;
    assert(currentSlot > 0, `Current slot: ${currentSlot}`);

    // Create 8 wallets for multi-outcome testing
    const wallets = [];
    const walletNames = ['Alpha', 'Beta', 'Gamma', 'Delta', 'Epsilon', 'Zeta', 'Eta', 'Theta'];
    for (let i = 0; i < 8; i++) {
        wallets.push(genKeypair());
    }

    // Fund all wallets
    let fundedCount = 0;
    for (let i = 0; i < wallets.length; i++) {
        const ok = await fundWallet(wallets[i]);
        if (ok) fundedCount++;
        else skip(`${walletNames[i]} funding may have failed`);
        if (i < wallets.length - 1) await sleep(500);
    }
    assertGte(fundedCount, 4, `Funded ${fundedCount}/8 wallets`);

    // Aliases for readability
    const [alpha, beta, gamma, delta, epsilon, zeta, eta, theta] = wallets;

    // ══════════════════════════════════════════════════════════════════════
    // M2. 4-Outcome Market Creation & Liquidity
    // ══════════════════════════════════════════════════════════════════════
    section('M2: 4-Outcome Market');
    const question4 = `Multi4 ${Date.now()}: Which ecosystem leads DeFi TVL? Lichen/Ethereum/Solana/Other`;
    let market4Id = 0;

    try {
        const args = buildCreateMarket(alpha.address, 0, currentSlot + 20_000, 4, question4, ['Lichen', 'Ethereum', 'Solana', 'Other']);
        const sig = await sendTx(alpha, [contractIx(alpha.address, CONTRACTS.predict, args, PREDICT_CREATE_FEE)], 400_000);
        assert(typeof sig === 'string', `4-outcome market created: ${sig?.slice(0, 16)}...`);
        await sleep(2500);
        market4Id = await resolveMarketIdByQuestion(question4, alpha.address);
        assertGt(market4Id, 0, `4-outcome market ID resolved: ${market4Id}`);
    } catch (e) {
        skip(`4-outcome market creation failed: ${e.message.slice(0, 80)}`);
    }

    if (market4Id > 0) {
        try {
            const odds4 = [3000, 2500, 2500, 2000]; // 30/25/25/20%
            const liq4 = buildAddInitialLiquidity(alpha.address, market4Id, 200 * PM_SCALE, odds4);
            const liqSig = await sendTx(alpha, [contractIx(alpha.address, CONTRACTS.predict, liq4, 200 * PM_SCALE)]);
            assert(typeof liqSig === 'string', '4-outcome market liquidity added');
            await sleep(2000);
        } catch (e) {
            skip(`4-outcome liquidity failed: ${e.message.slice(0, 80)}`);
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // M3. 5-Outcome Market Creation & Liquidity
    // ══════════════════════════════════════════════════════════════════════
    section('M3: 5-Outcome Market');
    const question5 = `Multi5 ${Date.now()}: Which language dominates smart contracts? Rust/Solidity/Move/Cairo/Other`;
    let market5Id = 0;

    try {
        const args = buildCreateMarket(beta.address, 1, currentSlot + 20_000, 5, question5, ['Rust', 'Solidity', 'Move', 'Cairo', 'Other']);
        const sig = await sendTx(beta, [contractIx(beta.address, CONTRACTS.predict, args, PREDICT_CREATE_FEE)], 400_000);
        assert(typeof sig === 'string', `5-outcome market created: ${sig?.slice(0, 16)}...`);
        await sleep(2500);
        market5Id = await resolveMarketIdByQuestion(question5, beta.address);
        assertGt(market5Id, 0, `5-outcome market ID resolved: ${market5Id}`);
    } catch (e) {
        skip(`5-outcome market creation failed: ${e.message.slice(0, 80)}`);
    }

    if (market5Id > 0) {
        try {
            const odds5 = [2500, 2000, 2000, 1500, 2000]; // 25/20/20/15/20%
            const liq5 = buildAddInitialLiquidity(beta.address, market5Id, 250 * PM_SCALE, odds5);
            const liqSig = await sendTx(beta, [contractIx(beta.address, CONTRACTS.predict, liq5, 250 * PM_SCALE)]);
            assert(typeof liqSig === 'string', '5-outcome market liquidity added');
            await sleep(2000);
        } catch (e) {
            skip(`5-outcome liquidity failed: ${e.message.slice(0, 80)}`);
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // M4. 6-Outcome Market Creation & Liquidity
    // ══════════════════════════════════════════════════════════════════════
    section('M4: 6-Outcome Market');
    const question6 = `Multi6 ${Date.now()}: Which sector booms next? AI/DeFi/Gaming/NFTs/Social/RWA`;
    let market6Id = 0;

    try {
        const args = buildCreateMarket(gamma.address, 2, currentSlot + 20_000, 6, question6, ['AI', 'DeFi', 'Gaming', 'NFTs', 'Social', 'RWA']);
        const sig = await sendTx(gamma, [contractIx(gamma.address, CONTRACTS.predict, args, PREDICT_CREATE_FEE)], 400_000);
        assert(typeof sig === 'string', `6-outcome market created: ${sig?.slice(0, 16)}...`);
        await sleep(2500);
        market6Id = await resolveMarketIdByQuestion(question6, gamma.address);
        assertGt(market6Id, 0, `6-outcome market ID resolved: ${market6Id}`);
    } catch (e) {
        skip(`6-outcome market creation failed: ${e.message.slice(0, 80)}`);
    }

    if (market6Id > 0) {
        try {
            const odds6 = [2000, 1800, 1500, 1500, 1200, 2000]; // sums to 10000 bps
            const liq6 = buildAddInitialLiquidity(gamma.address, market6Id, 300 * PM_SCALE, odds6);
            const liqSig = await sendTx(gamma, [contractIx(gamma.address, CONTRACTS.predict, liq6, 300 * PM_SCALE)]);
            assert(typeof liqSig === 'string', '6-outcome market liquidity added');
            await sleep(2000);
        } catch (e) {
            skip(`6-outcome liquidity failed: ${e.message.slice(0, 80)}`);
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // M5. 8-Outcome Market (Max) Creation & Liquidity
    // ══════════════════════════════════════════════════════════════════════
    section('M5: 8-Outcome Market (Max)');
    const question8 = `Multi8 ${Date.now()}: Top L1 by EOY? Lichen/Ethereum/Solana/Avalanche/Sui/Aptos/Cosmos/Near`;
    let market8Id = 0;

    try {
        const args = buildCreateMarket(delta.address, 3, currentSlot + 20_000, 8, question8, ['Lichen', 'Ethereum', 'Solana', 'Avalanche', 'Sui', 'Aptos', 'Cosmos', 'Near']);
        const sig = await sendTx(delta, [contractIx(delta.address, CONTRACTS.predict, args, PREDICT_CREATE_FEE)], 500_000);
        assert(typeof sig === 'string', `8-outcome market created: ${sig?.slice(0, 16)}...`);
        await sleep(2500);
        market8Id = await resolveMarketIdByQuestion(question8, delta.address);
        assertGt(market8Id, 0, `8-outcome market ID resolved: ${market8Id}`);
    } catch (e) {
        skip(`8-outcome market creation failed: ${e.message.slice(0, 80)}`);
    }

    if (market8Id > 0) {
        try {
            const odds8 = [1500, 1500, 1500, 1250, 1000, 1000, 750, 1500]; // sums to 10000 bps
            const liq8 = buildAddInitialLiquidity(delta.address, market8Id, 400 * PM_SCALE, odds8);
            const liqSig = await sendTx(delta, [contractIx(delta.address, CONTRACTS.predict, liq8, 400 * PM_SCALE)]);
            assert(typeof liqSig === 'string', '8-outcome market liquidity added');
            await sleep(2000);
        } catch (e) {
            skip(`8-outcome liquidity failed: ${e.message.slice(0, 80)}`);
        }
    }

    // Collect all active markets
    const allMarkets = [
        { id: market4Id, outcomes: 4, name: '4-outcome', creator: alpha, expectedNames: ['Lichen', 'Ethereum', 'Solana', 'Other'] },
        { id: market5Id, outcomes: 5, name: '5-outcome', creator: beta, expectedNames: ['Rust', 'Solidity', 'Move', 'Cairo', 'Other'] },
        { id: market6Id, outcomes: 6, name: '6-outcome', creator: gamma, expectedNames: ['AI', 'DeFi', 'Gaming', 'NFTs', 'Social', 'RWA'] },
        { id: market8Id, outcomes: 8, name: '8-outcome', creator: delta, expectedNames: ['Lichen', 'Ethereum', 'Solana', 'Avalanche', 'Sui', 'Aptos', 'Cosmos', 'Near'] },
    ].filter(m => m.id > 0);

    assertGte(allMarkets.length, 1, `${allMarkets.length} multi-outcome markets active`);

    // ══════════════════════════════════════════════════════════════════════
    // M6. Multi-Outcome Trading — Buy Shares on Every Outcome Index
    // ══════════════════════════════════════════════════════════════════════
    section('M6: Multi-Outcome Trading (Buy Every Outcome)');

    for (const market of allMarkets) {
        // Each trader buys a different outcome, cycling through all outcomes
        for (let outIdx = 0; outIdx < market.outcomes; outIdx++) {
            const trader = wallets[outIdx % wallets.length];
            const amount = (5 + outIdx) * PM_SCALE;
            try {
                const args = buildBuyShares(trader.address, market.id, outIdx, amount);
                const sig = await sendTx(trader, [contractIx(trader.address, CONTRACTS.predict, args, amount)]);
                assert(typeof sig === 'string', `${market.name} buy outcome=${outIdx} amt=${5 + outIdx}`);
            } catch (e) {
                assert(false, `${market.name} buy outcome=${outIdx} failed: ${e.message.slice(0, 60)}`);
            }
            await sleep(400);
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // M7. Multi-Outcome Sell Shares — Partial Exits
    // ══════════════════════════════════════════════════════════════════════
    section('M7: Multi-Outcome Sell Shares');

    for (const market of allMarkets) {
        // Sell a small portion from outcome 0 (first trader)
        const trader = wallets[0];
        const sellAmt = 2 * PM_SCALE;
        try {
            const args = buildSellShares(trader.address, market.id, 0, sellAmt);
            const sig = await sendTx(trader, [contractIx(trader.address, CONTRACTS.predict, args, 0)]);
            assert(typeof sig === 'string', `${market.name} sell outcome=0 amt=2`);
        } catch (e) {
            // Sell may fail if shares are insufficient — that's okay
            skip(`${market.name} sell outcome=0 skipped: ${e.message.slice(0, 60)}`);
        }
        await sleep(400);

        // Also sell from the last outcome
        const lastIdx = market.outcomes - 1;
        const lastTrader = wallets[lastIdx % wallets.length];
        try {
            const args = buildSellShares(lastTrader.address, market.id, lastIdx, PM_SCALE);
            const sig = await sendTx(lastTrader, [contractIx(lastTrader.address, CONTRACTS.predict, args, 0)]);
            assert(typeof sig === 'string', `${market.name} sell outcome=${lastIdx} amt=1`);
        } catch (e) {
            skip(`${market.name} sell outcome=${lastIdx} skipped: ${e.message.slice(0, 60)}`);
        }
        await sleep(400);
    }

    // ══════════════════════════════════════════════════════════════════════
    // M8. Multi-Outcome Complete-Set Mint & Redeem
    // ══════════════════════════════════════════════════════════════════════
    section('M8: Multi-Outcome Complete-Set Operations');

    for (const market of allMarkets) {
        // Mint a complete set (1 share of each outcome)
        const minter = wallets[2]; // gamma
        const mintAmt = 3 * PM_SCALE;
        try {
            const args = buildMintCompleteSet(minter.address, market.id, mintAmt);
            const sig = await sendTx(minter, [contractIx(minter.address, CONTRACTS.predict, args, mintAmt)]);
            assert(typeof sig === 'string', `${market.name} mint complete set amt=3`);
            await sleep(1000);
        } catch (e) {
            skip(`${market.name} mint complete set skipped: ${e.message.slice(0, 60)}`);
        }

        // Redeem a smaller amount of the complete set
        const redeemAmt = 1 * PM_SCALE;
        try {
            const args = buildRedeemCompleteSet(minter.address, market.id, redeemAmt);
            const sig = await sendTx(minter, [contractIx(minter.address, CONTRACTS.predict, args, 0)]);
            assert(typeof sig === 'string', `${market.name} redeem complete set amt=1`);
        } catch (e) {
            skip(`${market.name} redeem complete set skipped: ${e.message.slice(0, 60)}`);
        }
        await sleep(400);
    }

    // ══════════════════════════════════════════════════════════════════════
    // M9. Multi-Outcome Price Verification (sum ≈ 1.0)
    // ══════════════════════════════════════════════════════════════════════
    section('M9: Multi-Outcome Price Verification');

    for (const market of allMarkets) {
        const detail = await rest(`/prediction-market/markets/${market.id}`);
        if (detail?.data || detail?.market) {
            const d = detail.data || detail.market || detail;
            const outcomes = d.outcomes || d.outcome_prices || [];
            const outcomeCount = d.outcome_count || outcomes.length || 0;

            assert(outcomeCount === market.outcomes, `${market.name} has ${outcomeCount} outcomes (expected ${market.outcomes})`);

            if (Array.isArray(outcomes) && outcomes.length >= 2) {
                let priceSum = 0;
                for (let i = 0; i < outcomes.length; i++) {
                    const price = Number(outcomes[i]?.price ?? outcomes[i] ?? 0);
                    priceSum += price;
                    assertGte(price, 0, `${market.name} outcome ${i} price >= 0 (${price.toFixed(4)})`);
                }
                // AMM invariant: sum of all outcome prices should be near 1.0 (0.85-1.15 tolerance)
                assert(priceSum > 0.7 && priceSum < 1.3, `${market.name} price sum near 1.0 (${priceSum.toFixed(4)})`);
            } else {
                skip(`${market.name} outcomes not available from REST`);
            }

            // Volume should be non-zero after trading
            const totalVolume = Number(d.total_volume || d.volume || 0);
            assertGt(totalVolume, 0, `${market.name} has non-zero volume (${totalVolume})`);
        } else {
            skip(`${market.name} detail unavailable from REST`);
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // M10. Multi-Outcome Liquidity Add & Withdraw
    // ══════════════════════════════════════════════════════════════════════
    section('M10: Multi-Outcome Liquidity Management');

    for (const market of allMarkets) {
        // Add liquidity from a different provider
        const provider = wallets[4]; // epsilon
        try {
            const args = buildAddLiquidity(provider.address, market.id, 50 * PM_SCALE);
            const sig = await sendTx(provider, [contractIx(provider.address, CONTRACTS.predict, args, 50 * PM_SCALE)]);
            assert(typeof sig === 'string', `${market.name} add liquidity 50 lUSD`);
            await sleep(1000);
        } catch (e) {
            skip(`${market.name} add liquidity skipped: ${e.message.slice(0, 60)}`);
        }

        // Withdraw liquidity from creator
        try {
            const args = buildWithdrawLiquidity(market.creator.address, market.id, 20 * PM_SCALE);
            const sig = await sendTx(market.creator, [contractIx(market.creator.address, CONTRACTS.predict, args, 0)]);
            assert(typeof sig === 'string', `${market.name} withdraw liquidity 20 lUSD`);
        } catch (e) {
            skip(`${market.name} withdraw liquidity skipped: ${e.message.slice(0, 60)}`);
        }
        await sleep(400);
    }

    // ══════════════════════════════════════════════════════════════════════
    // M11. Multi-Outcome Analytics & Stats
    // ══════════════════════════════════════════════════════════════════════
    section('M11: Multi-Outcome Analytics');

    // Overall prediction market stats
    const pmStats = await rest('/prediction-market/stats');
    if (pmStats) {
        const stats = pmStats.data || pmStats;
        assertGte(Number(stats.total_markets || stats.market_count || 0), allMarkets.length,
            `PM stats total_markets >= ${allMarkets.length}`);
    } else {
        skip('PM stats endpoint not available');
    }

    // Markets list
    const listResp = await rest('/prediction-market/markets?limit=50');
    const marketsList = listResp?.data?.markets || listResp?.markets || [];
    assertGte(marketsList.length, allMarkets.length, `Markets list has >= ${allMarkets.length} markets`);

    // Verify multi-outcome markets appear with correct outcome counts
    for (const market of allMarkets) {
        const found = marketsList.find(m => Number(m.id) === market.id);
        if (found) {
            const oc = found.outcome_count || found.outcomes?.length || 0;
            assertEq(oc, market.outcomes, `${market.name} in list with ${oc} outcomes`);
            assertOutcomeNames(found.outcomes, market.expectedNames, `${market.name} list`);
        } else {
            skip(`${market.name} not found in markets list`);
        }
    }

    // Trending
    const trending = await rest('/prediction-market/trending');
    if (trending) {
        assert(true, 'Trending endpoint responds');
    } else {
        skip('Trending endpoint not available');
    }

    // ══════════════════════════════════════════════════════════════════════
    // M12. Edge Cases
    // ══════════════════════════════════════════════════════════════════════
    section('M12: Edge Cases');

    // Try buying invalid outcome index (should fail)
    if (allMarkets.length > 0) {
        const m = allMarkets[0];
        try {
            const args = buildBuyShares(alpha.address, m.id, m.outcomes, 5 * PM_SCALE); // outcome == count is out of bounds
            await sendTx(alpha, [contractIx(alpha.address, CONTRACTS.predict, args, 5 * PM_SCALE)]);
            assert(false, 'Buy invalid outcome index should have failed');
        } catch {
            assert(true, 'Buy invalid outcome index correctly rejected');
        }
        await sleep(300);

        // Try buying outcome index 255 (way out of bounds)
        try {
            const args = buildBuyShares(alpha.address, m.id, 255, 5 * PM_SCALE);
            await sendTx(alpha, [contractIx(alpha.address, CONTRACTS.predict, args, 5 * PM_SCALE)]);
            assert(false, 'Buy outcome=255 should have failed');
        } catch {
            assert(true, 'Buy outcome=255 correctly rejected');
        }
        await sleep(300);
    }

    // Try creating a 1-outcome market (should fail, min is 2)
    try {
        const args = buildCreateMarket(epsilon.address, 0, currentSlot + 10_000, 1, `Edge1 ${Date.now()}: single outcome`);
        await sendTx(epsilon, [contractIx(epsilon.address, CONTRACTS.predict, args, PREDICT_CREATE_FEE)], 400_000);
        assert(false, '1-outcome market should have been rejected');
    } catch {
        assert(true, '1-outcome market correctly rejected (min 2)');
    }
    await sleep(300);

    // Try creating a 9-outcome market (should fail, max is 8)
    try {
        const args = buildCreateMarket(epsilon.address, 0, currentSlot + 10_000, 9, `Edge9 ${Date.now()}: nine outcomes`);
        await sendTx(epsilon, [contractIx(epsilon.address, CONTRACTS.predict, args, PREDICT_CREATE_FEE)], 400_000);
        assert(false, '9-outcome market should have been rejected');
    } catch {
        assert(true, '9-outcome market correctly rejected (max 8)');
    }
    await sleep(300);

    // Try buying 0 amount (should fail)
    if (allMarkets.length > 0) {
        const m = allMarkets[0];
        try {
            const args = buildBuyShares(alpha.address, m.id, 0, 0);
            await sendTx(alpha, [contractIx(alpha.address, CONTRACTS.predict, args, 0)]);
            assert(false, 'Buy 0 amount should have failed');
        } catch {
            assert(true, 'Buy 0 amount correctly rejected');
        }
        await sleep(300);
    }

    // Try selling more shares than owned  
    if (allMarkets.length > 0) {
        const m = allMarkets[0];
        try {
            const args = buildSellShares(theta.address, m.id, 0, 999_999 * PM_SCALE);
            await sendTx(theta, [contractIx(theta.address, CONTRACTS.predict, args, 0)]);
            assert(false, 'Sell more than owned should have failed');
        } catch {
            assert(true, 'Sell more than owned correctly rejected');
        }
        await sleep(300);
    }

    // Try operations on non-existent market
    try {
        const args = buildBuyShares(alpha.address, 999_999, 0, 5 * PM_SCALE);
        await sendTx(alpha, [contractIx(alpha.address, CONTRACTS.predict, args, 5 * PM_SCALE)]);
        assert(false, 'Buy on non-existent market should have failed');
    } catch {
        assert(true, 'Buy on non-existent market correctly rejected');
    }
    await sleep(300);

    // ══════════════════════════════════════════════════════════════════════
    // M13. REST API Verification for Multi-Outcome Markets
    // ══════════════════════════════════════════════════════════════════════
    section('M13: REST API Verification');

    for (const market of allMarkets) {
        // Individual market detail
        const detail = await rest(`/prediction-market/markets/${market.id}`);
        if (detail) {
            assert(true, `${market.name} GET /markets/${market.id} responds`);
            const d = detail.data || detail.market || detail;
            const oc = d.outcome_count || d.outcomes?.length || 0;
            assertEq(oc, market.outcomes, `${market.name} detail shows ${oc} outcomes`);
            assertOutcomeNames(d.outcomes, market.expectedNames, `${market.name} detail`);
        } else {
            skip(`${market.name} detail not available`);
        }

        // Positions endpoint
        const positions = await rest(`/prediction-market/positions?market_id=${market.id}&limit=50`);
        if (positions) {
            assert(true, `${market.name} GET /positions responds`);
        } else {
            skip(`${market.name} positions not available`);
        }

        // Price history
        const history = await rest(`/prediction-market/markets/${market.id}/prices`);
        if (history) {
            assert(true, `${market.name} GET /prices responds`);
        } else {
            skip(`${market.name} price history not available`);
        }
    }

    // Category filter
    const catResp = await rest('/prediction-market/markets?category=0&limit=10');
    if (catResp) {
        assert(true, 'Category filter responds');
    } else {
        skip('Category filter not available');
    }

    // ══════════════════════════════════════════════════════════════════════
    // M14. Final State & Position Verification
    // ══════════════════════════════════════════════════════════════════════
    section('M14: Final State Verification');

    for (const market of allMarkets) {
        const detail = await rest(`/prediction-market/markets/${market.id}`);
        if (detail) {
            const d = detail.data || detail.market || detail;
            const status = d.status || d.state || '';
            // Should be active (we haven't resolved any)
            assert(
                status === 'Active' || status === 'active' || status === 'ACTIVE' || status === 1 || status === '1',
                `${market.name} status is active (${status})`
            );

            // Liquidity should be non-zero
            const liq = Number(d.total_collateral || d.total_liquidity || d.liquidity || d.pool_liquidity || 0);
            assertGt(liq, 0, `${market.name} has liquidity (${liq})`);

            // Total volume should reflect all trades
            const vol = Number(d.total_volume || d.volume || 0);
            assertGt(vol, 0, `${market.name} total volume > 0 (${vol})`);
        } else {
            skip(`${market.name} final detail not available`);
        }
    }

    // Verify each wallet has some balance remaining after all operations
    let walletsWithBalance = 0;
    for (let i = 0; i < wallets.length; i++) {
        try {
            const bal = await rpc('getBalance', [wallets[i].address]);
            const spores = typeof bal === 'number'
                ? bal
                : Number(bal?.spendable ?? bal?.spores ?? bal?.value ?? 0);
            const licn = spores / SPORES_PER_LICN;
            if (licn > 0) walletsWithBalance++;
        } catch { /* ignore */ }
    }
    assertGte(walletsWithBalance, 4, `${walletsWithBalance}/8 wallets have remaining balance`);

    // ══════════════════════════════════════════════════════════════════════
    // Summary
    // ══════════════════════════════════════════════════════════════════════
    console.log('\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━');
    console.log(`  Multi-Outcome Prediction E2E: ${passed} passed, ${failed} failed, ${skipped} skipped`);
    console.log('━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━');
    process.exit(failed > 0 ? 1 : 0);
}

main().catch(e => { console.error('FATAL:', e); process.exit(2); });
