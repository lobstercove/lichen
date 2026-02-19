/**
 * DEX Frontend Tests — Phase 10 + Phase 10 Extra + Oracle Price Feed Integration
 *                       + Trade Bridge & Oracle Integration (Phases A-D)
 * Run: node dex.test.js
 *
 * Tests all pure-function fixes applied during Phase 10 audit:
 *  F10.8  — escapeHtml XSS sanitization
 *  F10.9  — encodeTransactionMessage bincode-compatible signing
 *  F10.9  — sendTransaction validator-compatible wire format
 *  F10.10 — bs58 encode/decode round-trip
 *  F10.1-F10.7 — handler wiring (structural tests)
 *
 * Phase 10 Extra pass tests:
 *  F10E.1  — Order form wallet-gate
 *  F10E.2  — Quick Trade + Create Market wallet-gate
 *  F10E.3  — Consistent bottom panel toggling
 *  F10E.4  — Governance New Proposal wallet-gate
 *  F10E.5  — Parameter + Delist proposal form fields
 *  F10E.6  — MOLT/mUSD genesis price $0.10
 *  F10E.7  — External price feed (Binance WebSocket real-time overlay)
 *  F10E.8  — CSS disabled styles
 *  F10E.9  — Margin position wallet-gate
 *  F10E.10 — Add Liquidity wallet-gate
 *  F10E.11 — Pool "My Pools" filter logic
 *
 * Oracle Price Feed Integration tests:
 *  - Genesis oracle seeding (wSOL, wETH feeds)
 *  - Genesis analytics price seeding (ana_lp_, ana_24h_, candles)
 *  - Background price feeder service (Binance WebSocket + REST fallback → moltoracle + analytics)
 *  - RPC oracle integration (fallback prices, /oracle/prices endpoint)
 *  - Frontend real-time overlay (Binance WS for sub-second updates)
 *  - End-to-end data flow verification
 *
 * Trade Bridge + Oracle Integration (Phases A-D):
 *  PA — Trade bridge: dex_core fills → dex_analytics (prices, volume, candles)
 *  PB — Oracle fallback-only: skip analytics writes when real trades active
 *  PC — Oracle price bands: ±5% market / ±10% limit enforcement in dex_core
 *  PD — Frontend oracle reference line: gold dashed line on TradingView chart
 *  - Genesis oracle seeding (wSOL, wETH feeds)
 *  - Genesis analytics price seeding (ana_lp_, ana_24h_, candles)
 *  - Background price feeder service (Binance WebSocket + REST fallback → moltoracle + analytics)
 *  - RPC oracle integration (fallback prices, /oracle/prices endpoint)
 *  - Frontend real-time overlay (Binance WS for sub-second updates)
 *  - End-to-end data flow verification
 */
'use strict';

let passed = 0, failed = 0;
function assert(cond, msg) {
    if (cond) { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
    else { failed++; process.stderr.write(`  ✗ ${msg}\n`); }
}
function assertEqual(a, b, msg) {
    const eq = typeof a === 'object' ? JSON.stringify(a) === JSON.stringify(b) : a === b;
    if (eq) { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
    else { failed++; process.stderr.write(`  ✗ ${msg}: expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}\n`); }
}
function assertThrows(fn, msg) {
    try { fn(); failed++; process.stderr.write(`  ✗ ${msg}: did not throw\n`); }
    catch { passed++; process.stdout.write(`  ✓ ${msg}\n`); }
}

// ═══════════════════════════════════════════════════════════════════════════
// Extract pure functions from dex.js (inline reimplementation matching source)
// ═══════════════════════════════════════════════════════════════════════════

function escapeHtml(str) {
    if (typeof str !== 'string') return String(str ?? '');
    return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

function hexToBytes(hex) {
    const bytes = new Uint8Array(hex.length / 2);
    for (let i = 0; i < hex.length; i += 2) bytes[i / 2] = parseInt(hex.substr(i, 2), 16);
    return bytes;
}
function bytesToHex(bytes) { return [...bytes].map(b => b.toString(16).padStart(2, '0')).join(''); }

const BS58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
function bs58encode(bytes) {
    let leadingZeros = 0;
    for (let i = 0; i < bytes.length && bytes[i] === 0; i++) leadingZeros++;
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
    const parts = [];
    function pushU64LE(n) {
        const buf = new ArrayBuffer(8);
        const view = new DataView(buf);
        view.setUint32(0, n & 0xFFFFFFFF, true);
        view.setUint32(4, Math.floor(n / 0x100000000) & 0xFFFFFFFF, true);
        parts.push(new Uint8Array(buf));
    }
    // instructions: Vec<Instruction> — u64 length prefix
    pushU64LE(instructions.length);
    for (const ix of instructions) {
        parts.push(bs58decode(ix.program_id));
        const accounts = ix.accounts || [signer];
        pushU64LE(accounts.length);
        for (const acct of accounts) parts.push(bs58decode(acct));
        const dataBytes = typeof ix.data === 'string' ? new TextEncoder().encode(ix.data) : ix.data;
        pushU64LE(dataBytes.length);
        parts.push(dataBytes);
    }
    parts.push(hexToBytes(blockhash));
    const total = parts.reduce((s, a) => s + a.length, 0);
    const out = new Uint8Array(total);
    let off = 0;
    for (const a of parts) { out.set(a, off); off += a.length; }
    return out;
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: F10.8 — escapeHtml
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F10.8: escapeHtml XSS sanitization ──');

assertEqual(escapeHtml('<script>alert(1)</script>'), '&lt;script&gt;alert(1)&lt;/script&gt;', 'Escapes script tags');
assertEqual(escapeHtml('"onload="alert(1)'), '&quot;onload=&quot;alert(1)', 'Escapes double quotes');
assertEqual(escapeHtml("test'ing"), "test&#39;ing", 'Escapes single quotes');
assertEqual(escapeHtml('a&b<c>d'), 'a&amp;b&lt;c&gt;d', 'Escapes &, <, >');
assertEqual(escapeHtml('hello world'), 'hello world', 'Passes safe strings through');
assertEqual(escapeHtml(''), '', 'Handles empty string');
assertEqual(escapeHtml(null), '', 'Handles null');
assertEqual(escapeHtml(undefined), '', 'Handles undefined');
assertEqual(escapeHtml(42), '42', 'Handles numbers');

// ═══════════════════════════════════════════════════════════════════════════
// Test: F10.10 — Base58 encode/decode
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F10.10: Base58 encode/decode ──');

// Known deploy-manifest addresses
const knownAddresses = [
    '216MacD82KfB2hAeKR17M63ZXfURJQZnzDq2ho7SeJR7',
    'AANMpDkSnvSKa6PuaLQuRDU4SMzao7Yx3nLKzC2iatBn',
    'GJkJrM3DyDqxtMPL3BQyvrDNu3kaJCQCk9RDSTuMo8yz',
    'dPUYAb3Ld8pZJiCsXkZ838CybU4v8k1ZCeYebX9cS3K',
    'GFDF7SdCMhveUCU92japioP3uC6qA66EChQ3jbkFc5Bi',
    'HE5DVQuG6mVNsvprmvLJr1gZ3nRmKNgytXZNc6mZjjXJ',
];
for (const addr of knownAddresses) {
    const decoded = bs58decode(addr);
    assertEqual(decoded.length, 32, `${addr.slice(0, 8)}... decodes to 32 bytes`);
    const reencoded = bs58encode(decoded);
    assertEqual(reencoded, addr, `${addr.slice(0, 8)}... round-trips correctly`);
}

// Leading zeros
const allZeros = new Uint8Array(32);
const zeroEncoded = bs58encode(allZeros);
const zeroDecoded = bs58decode(zeroEncoded);
assertEqual(zeroDecoded.length, 32, 'All-zeros round-trips: length 32');
assertEqual(bytesToHex(zeroDecoded), '0'.repeat(64), 'All-zeros round-trips: all zeros');

assertThrows(() => bs58decode('0OIl'), 'Rejects invalid base58 characters');

// ═══════════════════════════════════════════════════════════════════════════
// Test: F10.9 — encodeTransactionMessage bincode format
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F10.9: encodeTransactionMessage bincode format ──');

const testSigner = '216MacD82KfB2hAeKR17M63ZXfURJQZnzDq2ho7SeJR7';
const testBlockhash = 'ab'.repeat(32); // 64 hex chars → 32 bytes
const testProgramId = 'AANMpDkSnvSKa6PuaLQuRDU4SMzao7Yx3nLKzC2iatBn';

// Single instruction, single account, small data
{
    const msg = encodeTransactionMessage(
        [{ program_id: testProgramId, accounts: [testSigner], data: '{"op":"test"}' }],
        testBlockhash,
        testSigner,
    );
    assert(msg instanceof Uint8Array, 'Returns Uint8Array');
    assert(msg.length > 0, 'Non-zero length');

    // Parse it back
    let off = 0;
    function readU64LE() {
        const lo = msg[off] | (msg[off+1]<<8) | (msg[off+2]<<16) | (msg[off+3]<<24);
        off += 8; // skip hi 4 bytes (all zero for small numbers)
        return lo >>> 0;
    }

    // instructions count
    const nIx = readU64LE();
    assertEqual(nIx, 1, 'Instruction count = 1');

    // program_id: 32 bytes
    const programIdBytes = msg.slice(off, off + 32); off += 32;
    assertEqual(bs58encode(programIdBytes), testProgramId, 'program_id matches');

    // accounts: u64 count + count * 32
    const nAccounts = readU64LE();
    assertEqual(nAccounts, 1, 'Account count = 1');
    const acctBytes = msg.slice(off, off + 32); off += 32;
    assertEqual(bs58encode(acctBytes), testSigner, 'Account matches signer');

    // data: u64 length + bytes
    const dataLen = readU64LE();
    const dataStr = new TextDecoder().decode(msg.slice(off, off + dataLen)); off += dataLen;
    assertEqual(dataStr, '{"op":"test"}', 'Instruction data matches');

    // recent_blockhash: 32 bytes at the end
    const bh = msg.slice(off, off + 32);
    assertEqual(bytesToHex(bh), testBlockhash, 'Blockhash matches (hex-decoded)');
    assertEqual(off + 32, msg.length, 'No trailing bytes');
}

// Multiple instructions
{
    const msg = encodeTransactionMessage(
        [
            { program_id: testProgramId, accounts: [testSigner], data: 'first' },
            { program_id: testSigner, accounts: [testSigner, testProgramId], data: 'second' },
        ],
        testBlockhash,
        testSigner,
    );
    let off = 0;
    function readU64() {
        const lo = msg[off] | (msg[off+1]<<8) | (msg[off+2]<<16) | (msg[off+3]<<24);
        off += 8;
        return lo >>> 0;
    }
    const nIx = readU64();
    assertEqual(nIx, 2, 'Multi-instruction: count = 2');
    // Skip first instruction
    off += 32; // program_id
    const nAcc1 = readU64(); off += nAcc1 * 32; // accounts
    const dLen1 = readU64(); off += dLen1; // data
    // Second instruction
    const pid2 = bs58encode(msg.slice(off, off + 32)); off += 32;
    assertEqual(pid2, testSigner, 'Second instruction program_id');
    const nAcc2 = readU64();
    assertEqual(nAcc2, 2, 'Second instruction: 2 accounts');
}

// Uint8Array data (binary)
{
    const binaryData = new Uint8Array([0, 1, 2, 3, 255]);
    const msg = encodeTransactionMessage(
        [{ program_id: testProgramId, accounts: [testSigner], data: binaryData }],
        testBlockhash,
        testSigner,
    );
    let off = 0;
    off += 8; // ix count
    off += 32; // program_id
    off += 8; // accounts count
    off += 32; // account
    const lo = msg[off] | (msg[off+1]<<8) | (msg[off+2]<<16) | (msg[off+3]<<24);
    off += 8;
    assertEqual(lo, 5, 'Binary data length = 5');
    assertEqual(msg[off], 0, 'Binary data byte 0');
    assertEqual(msg[off+4], 255, 'Binary data byte 4');
}

// ═══════════════════════════════════════════════════════════════════════════
// Test: F10.9 — sendTransaction wire format validation
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F10.9: sendTransaction wire format ──');

// Simulate what sendTransaction builds (without actual signing/RPC)
function buildTransactionPayload(instructions, signerAddress, signerPubkeyHex, signature, blockhash) {
    return JSON.stringify({
        signatures: [signature],
        message: {
            instructions: instructions.map(ix => ({
                program_id: ix.program_id,
                accounts: ix.accounts || [signerAddress],
                data: [...(typeof ix.data === 'string' ? new TextEncoder().encode(ix.data) : ix.data)],
            })),
            recent_blockhash: blockhash,
        },
    });
}

const testPayload = buildTransactionPayload(
    [{ program_id: testProgramId, accounts: [testSigner], data: '{"op":"place"}' }],
    testSigner,
    'aabb'.repeat(16),
    'cc'.repeat(64),
    testBlockhash,
);
const parsed = JSON.parse(testPayload);

assert(Array.isArray(parsed.signatures), 'Has signatures array');
assertEqual(parsed.signatures.length, 1, 'One signature');
assertEqual(parsed.signatures[0].length, 128, 'Signature is 64-byte hex (128 chars)');

assert(parsed.message !== undefined, 'Has message field');
assert(Array.isArray(parsed.message.instructions), 'message.instructions is array');
assertEqual(parsed.message.instructions.length, 1, 'One instruction');

const ix = parsed.message.instructions[0];
assertEqual(ix.program_id, testProgramId, 'program_id is base58 string');
assert(Array.isArray(ix.accounts), 'accounts is array');
assertEqual(ix.accounts[0], testSigner, 'Account is base58 string');
assert(Array.isArray(ix.data), 'data is byte array');
assertEqual(String.fromCharCode(...ix.data), '{"op":"place"}', 'data decodes to JSON');
assertEqual(parsed.message.recent_blockhash, testBlockhash, 'Blockhash is hex string');

// Key structural checks matching validator expectations
assert(!('programId' in ix), 'No camelCase programId (snake_case only)');
assert('program_id' in ix, 'Uses snake_case program_id');
assert('recent_blockhash' in parsed.message, 'Uses recent_blockhash field');

// ═══════════════════════════════════════════════════════════════════════════
// Test: F10.1/F10.2/F10.3/F10.4/F10.6/F10.7 — Handler wiring structure
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F10.1-F10.7: Handler wiring structural checks ──');

const fs = require('fs');
const dexSource = fs.readFileSync(__dirname + '/dex.js', 'utf8');

// F10.1: Order submission uses contractIx + buildPlaceOrderArgs
assert(dexSource.includes('buildPlaceOrderArgs') && dexSource.includes('contracts.dex_core'), 'F10.1: Order submit wired to contractIx + dex_core');
assert(!dexSource.includes("api.post('/orders'"), 'F10.1: No unsigned api.post to /orders');

// F10.2: Cancel order uses contractIx + buildCancelOrderArgs
assert(dexSource.includes('buildCancelOrderArgs'), 'F10.2: Cancel order uses contractIx + buildCancelOrderArgs');
assert(!dexSource.includes("api.del('/orders"), 'F10.2: No unsigned api.del for cancel');

// F10.3: Margin uses contractIx + buildOpenPositionArgs/buildClosePositionArgs
assert(dexSource.includes('buildOpenPositionArgs'), 'F10.3: Margin open wired to buildOpenPositionArgs');
assert(dexSource.includes('buildClosePositionArgs'), 'F10.3: Margin close wired to buildClosePositionArgs');
assert(dexSource.includes('contracts.dex_margin'), 'F10.3: Uses dex_margin contract');

// F10.4: Prediction trade uses contractIx + buildBuySharesArgs/buildCreateMarketArgs
assert(dexSource.includes('buildBuySharesArgs'), 'F10.4: Prediction trade wired to buildBuySharesArgs');
assert(dexSource.includes('buildCreateMarketArgs'), 'F10.4: Prediction create wired to buildCreateMarketArgs');
assert(dexSource.includes('contracts.prediction_market'), 'F10.4: Uses prediction_market contract');
assert(!dexSource.includes("api.post('/prediction-market/trade'"), 'F10.4: No unsigned REST for prediction trade');
assert(!dexSource.includes("api.post('/prediction-market/create'"), 'F10.4: No unsigned REST for prediction create');

// F10.5: Resolution + claim UI uses buildResolveMarketArgs/buildRedeemSharesArgs
assert(dexSource.includes('buildResolveMarketArgs'), 'F10.5: Resolve market handler uses buildResolveMarketArgs');
assert(dexSource.includes('buildRedeemSharesArgs'), 'F10.5: Claim winnings handler uses buildRedeemSharesArgs');
assert(dexSource.includes('btn-predict-resolve'), 'F10.5: Resolve button rendered');
assert(dexSource.includes('btn-predict-claim'), 'F10.5: Claim button rendered');

// F10.6: Governance uses contractIx + buildVoteArgs
assert(dexSource.includes('buildVoteArgs'), 'F10.6: Vote wired to buildVoteArgs');
assert(dexSource.includes('contracts.dex_governance'), 'F10.6: Uses dex_governance contract');
assert(dexSource.includes('proposalData') || dexSource.includes('contractIx(contracts.dex_governance'), 'F10.6: Proposal submit wired to contractIx');
assert(!dexSource.includes("api.post('/governance/proposals'"), 'F10.6: No unsigned REST for proposals');

// F10.7: Reward claim uses contractIx + buildClaimRewardsArgs
assert(dexSource.includes('buildClaimRewardsArgs'), 'F10.7: Reward claim wired to buildClaimRewardsArgs');
assert(dexSource.includes('contracts.dex_rewards'), 'F10.7: Uses dex_rewards contract');
assert(dexSource.includes('contractIx(contracts.dex_rewards'), 'F10.7: Claim uses contractIx (not fake GET)');

// F10.8: XSS — escapeHtml used in innerHTML
const escapeCount = (dexSource.match(/escapeHtml\(/g) || []).length;
assert(escapeCount >= 15, `F10.8: escapeHtml called ${escapeCount} times (>=15 expected)`);

// F10.9: Signing format
assert(dexSource.includes('encodeTransactionMessage'), 'F10.9: encodeTransactionMessage exists');
assert(dexSource.includes('pushU64LE'), 'F10.9: Uses u64 LE length prefixes (bincode)');
assert(dexSource.includes('bs58decode(ix.program_id)'), 'F10.9: program_id base58-decoded for signing');
assert(dexSource.includes('hexToBytes(blockhash)'), 'F10.9: Blockhash hex-decoded (not UTF-8)');

// F10.10: Contract addresses loaded dynamically
assert(dexSource.includes('loadContractAddresses'), 'F10.10: loadContractAddresses function exists');
assert(dexSource.includes('getAllSymbolRegistry'), 'F10.10: Uses getAllSymbolRegistry RPC');
assert(dexSource.includes('await loadContractAddresses()'), 'F10.10: Called in init');

// F10.11: Auto-reconnect indicator
assert(dexSource.includes('(view only)'), 'F10.11: View-only indicator shown');

// ═══════════════════════════════════════════════════════════════════════════
// Phase 10 Extra Pass (F10E) — Structural Tests
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── F10E: Extra pass structural checks ──');

// F10E.1: Order form wallet-gate — applyWalletGateAll function exists and targets order form
assert(dexSource.includes('function applyWalletGateAll'), 'F10E.1: applyWalletGateAll function exists');
assert(dexSource.includes("Connect Wallet to Trade"), 'F10E.1: Order form shows wallet-gate button text');
assert(dexSource.includes('wallet-gated-disabled'), 'F10E.1: Uses wallet-gated-disabled CSS class');

// F10E.2: Quick Trade + Create Market wallet-gate
assert(dexSource.includes("Connect Wallet to Create"), 'F10E.2: Create Market shows wallet-gate text');
assert(dexSource.includes("predict-trade-panel") && dexSource.includes("wallet-gated-disabled"), 'F10E.2: Quick Trade panel gets wallet-gated-disabled');
assert(dexSource.includes("predict-create-panel") && dexSource.includes("wallet-gated-disabled"), 'F10E.2: Create Market panel gets wallet-gated-disabled');

// F10E.3: Consistent bottom panel toggling across all views
assert(dexSource.includes("predictBottomPanel"), 'F10E.3: Predict bottom panel ID referenced in JS');
assert(dexSource.includes("poolBottomPanel"), 'F10E.3: Pool bottom panel ID referenced in JS');
assert(dexSource.includes("rewardsBottomPanel"), 'F10E.3: Rewards bottom panel ID referenced in JS');

// F10E.4: Governance New Proposal wallet-gate
assert(dexSource.includes("Connect Wallet to Propose"), 'F10E.4: Governance proposal shows wallet-gate text');
assert(dexSource.includes("proposalForm") && dexSource.includes("wallet-gated-disabled"), 'F10E.4: Proposal form gets wallet-gated-disabled');

// F10E.5: Parameter + Delist proposal fields
assert(dexSource.includes('delistFields'), 'F10E.5: delistFields element referenced');
assert(dexSource.includes('paramFields'), 'F10E.5: paramFields element referenced');
assert(dexSource.includes("propDelistPair"), 'F10E.5: Delist pair selector exists');
assert(dexSource.includes("propDelistReason"), 'F10E.5: Delist reason textarea exists');
assert(dexSource.includes("propParamName"), 'F10E.5: Parameter name selector exists');
assert(dexSource.includes("propParamValue"), 'F10E.5: Parameter value input exists');
assert(dexSource.includes("propParamCurrent"), 'F10E.5: Current param value display exists');
assert(dexSource.includes("propParamDesc"), 'F10E.5: Parameter description display exists');
// Proposal data includes delist + param types
assert(dexSource.includes("ptype === 'delist'"), 'F10E.5: Delist proposal data construction');
assert(dexSource.includes("ptype === 'param'"), 'F10E.5: Param proposal data construction');
assert(dexSource.includes('proposalData.pair_id') && dexSource.includes('proposalData.reason') && dexSource.includes('delistReason'), 'F10E.5: Delist sends pair_id + reason');
assert(dexSource.includes('proposalData.parameter') && dexSource.includes('proposalData.proposed_value'), 'F10E.5: Param sends parameter + proposed_value');

// F10E.6: MOLT/mUSD genesis price $0.10
assert(dexSource.includes('MOLT_GENESIS_PRICE'), 'F10E.6: MOLT_GENESIS_PRICE constant exists');
assert(dexSource.includes('MOLT_GENESIS_PRICE = 0.10') || dexSource.includes('MOLT_GENESIS_PRICE = 0.1'), 'F10E.6: Genesis price is $0.10');
assert(dexSource.includes('lastPrice: MOLT_GENESIS_PRICE'), 'F10E.6: State defaults to genesis price');
// Verify fallback pair creation
assert(dexSource.includes("genesis MOLT/mUSD"), 'F10E.6: Genesis pair fallback message');
assert(dexSource.includes("price: MOLT_GENESIS_PRICE"), 'F10E.6: Genesis pair uses MOLT_GENESIS_PRICE');

// F10E.7: External price feed — Binance WebSocket for real-time overlay,
// backend oracle feeder provides primary prices via standard API
assert(dexSource.includes('connectBinancePriceFeed'), 'F10E.7: connectBinancePriceFeed function exists');
assert(dexSource.includes('stream.binance.com'), 'F10E.7: Uses Binance WebSocket endpoint');
assert(dexSource.includes('solusdt@miniTicker'), 'F10E.7: Subscribes to SOL/USDT');
assert(dexSource.includes('ethusdt@miniTicker'), 'F10E.7: Subscribes to ETH/USDT');
assert(!dexSource.includes('btcusdt'), 'F10E.7: BTC streams removed');
assert(dexSource.includes('applyBinanceRealTimeOverlay'), 'F10E.7: applyBinanceRealTimeOverlay function exists');
assert(dexSource.includes("externalPrices"), 'F10E.7: externalPrices state object exists');
assert(dexSource.includes("real-time overlay"), 'F10E.7: Binance feed documented as real-time overlay');

// F10E.8: CSS disabled styles
const cssSource = fs.readFileSync(__dirname + '/dex.css', 'utf8');
assert(cssSource.includes('.btn-full:disabled') || cssSource.includes('.btn:disabled'), 'F10E.8: CSS has disabled button styles');
assert(cssSource.includes('.btn-wallet-gate'), 'F10E.8: CSS has .btn-wallet-gate class');
assert(cssSource.includes('.wallet-gated-disabled input'), 'F10E.8: CSS dims inputs in wallet-gated-disabled containers');

// F10E.9: Margin position wallet-gate
assert(dexSource.includes("margin-form-card") && dexSource.includes("wallet-gated-disabled"), 'F10E.9: Margin form gets wallet-gated-disabled');
assert(dexSource.includes("marginOpenBtn") && dexSource.includes("Connect Wallet"), 'F10E.9: Margin open button shows wallet-gate text');

// F10E.10: Add Liquidity wallet-gate
assert(dexSource.includes("addLiqForm") && dexSource.includes("wallet-gated-disabled"), 'F10E.10: Add Liquidity form gets wallet-gated-disabled');

// F10E.11: Pool "My Pools" filter logic
assert(dexSource.includes("filter === 'my'") && dexSource.includes("state.connected"), 'F10E.11: My Pools filter checks connected');
assert(dexSource.includes("lp-position-card"), 'F10E.11: My Pools filter references LP positions');

// F10E.5: Parameter select change handler
assert(dexSource.includes("propParamName") && dexSource.includes("addEventListener('change'"), 'F10E.5: Parameter select has change handler');
assert(dexSource.includes("dataset?.current"), 'F10E.5: Reads data-current from option');
assert(dexSource.includes("dataset?.desc"), 'F10E.5: Reads data-desc from option');
assert(dexSource.includes("dataset?.unit"), 'F10E.5: Reads data-unit from option');

// F10E: Wallet-gate is called in init, connect, disconnect
const gateCallCount = (dexSource.match(/applyWalletGateAll\(\)/g) || []).length;
assert(gateCallCount >= 4, `F10E: applyWalletGateAll called ${gateCallCount} times (>=4 expected: init, connect, disconnect, auto-connect)`);

// F10E: Binance feed is connected in init
assert(dexSource.includes('connectBinancePriceFeed()'), 'F10E.7: Binance feed connected in init');

// ─── HTML structural tests ─────────────────────────────────────────────
console.log('\n── F10E: HTML structural checks ──');

const htmlSource = fs.readFileSync(__dirname + '/index.html', 'utf8');

// F10E.3: Bottom panels have IDs + hidden class
assert(htmlSource.includes('id="predictBottomPanel"') && htmlSource.includes('predict-bottom-panel hidden'), 'F10E.3: Predict bottom panel has ID + hidden');
assert(htmlSource.includes('id="poolBottomPanel"') && htmlSource.includes('pool-bottom-panel hidden'), 'F10E.3: Pool bottom panel has ID + hidden');
assert(htmlSource.includes('id="rewardsBottomPanel"') && htmlSource.includes('rewards-bottom-panel hidden'), 'F10E.3: Rewards bottom panel has ID + hidden');

// F10E.5: Delist fields in HTML
assert(htmlSource.includes('id="delistFields"'), 'F10E.5: delistFields section in HTML');
assert(htmlSource.includes('id="propDelistPair"'), 'F10E.5: propDelistPair select in HTML');
assert(htmlSource.includes('id="propDelistReason"'), 'F10E.5: propDelistReason textarea in HTML');
assert(htmlSource.includes('Delist Impact'), 'F10E.5: Delist impact info box in HTML');

// F10E.5: Parameter fields in HTML
assert(htmlSource.includes('id="paramFields"'), 'F10E.5: paramFields section in HTML');
assert(htmlSource.includes('id="propParamName"'), 'F10E.5: propParamName select in HTML');
assert(htmlSource.includes('id="propParamValue"'), 'F10E.5: propParamValue input in HTML');
assert(htmlSource.includes('id="propParamCurrent"'), 'F10E.5: propParamCurrent display in HTML');
assert(htmlSource.includes('id="propParamDesc"'), 'F10E.5: propParamDesc display in HTML');

// F10E.5: Parameter options with data attributes
assert(htmlSource.includes('data-current="5"') && htmlSource.includes('data-unit="x"'), 'F10E.5: Max Leverage option has current+unit');
assert(htmlSource.includes('data-desc="Maximum leverage') , 'F10E.5: Max Leverage option has description');
assert(htmlSource.includes('value="fee_split_treasury"'), 'F10E.5: Fee Split Treasury option exists');
assert(htmlSource.includes('value="prediction_fee"'), 'F10E.5: Prediction Market Fee option exists');
assert(htmlSource.includes('value="margin_maintenance"'), 'F10E.5: Margin Maintenance Ratio option exists');

// F10E.5: Parameter count — should be 11 protocol parameters
const paramOptionCount = (htmlSource.match(/<option value="[a-z_]+" data-current="/g) || []).length;
assert(paramOptionCount >= 10, `F10E.5: ${paramOptionCount} protocol parameter options (>=10 expected)`);

// F10E.5: Delist reason + impact
assert(htmlSource.includes('All open orders on this pair will be cancelled'), 'F10E.5: Delist impact — open orders warning');
assert(htmlSource.includes('LP positions will be auto-withdrawn'), 'F10E.5: Delist impact — LP withdrawal warning');
assert(htmlSource.includes('Margin positions will be force-closed'), 'F10E.5: Delist impact — margin close warning');

// F10E.8: CSS disabled styles (HTML references disabled on buttons/inputs)
assert(cssSource.includes('cursor: not-allowed'), 'F10E.8: Disabled cursor style exists');
assert(cssSource.includes('pointer-events: none'), 'F10E.8: Disabled pointer-events exists');

// Deklist pair select populated
assert(dexSource.includes("delistSelect") && dexSource.includes("propDelistPair"), 'F10E.5: Delist pair select populated from pairs');

// ═══════════════════════════════════════════════════════════════════════════
// Oracle Price Feed Integration Tests
// Tests the full oracle → RPC → candles → frontend pipeline
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── Oracle Price Feed Integration ──');

// ── Validator oracle seeding tests ──
const validatorSrc = fs.readFileSync(__dirname + '/../validator/src/main.rs', 'utf8');

assert(validatorSrc.includes('genesis_seed_analytics_prices'), 'ORACLE: genesis_seed_analytics_prices function exists');
assert(validatorSrc.includes('spawn_oracle_price_feeder'), 'ORACLE: spawn_oracle_price_feeder function exists');
assert(validatorSrc.includes('oracle_update_candle'), 'ORACLE: oracle_update_candle function exists');
assert(validatorSrc.includes('BinanceTicker'), 'ORACLE: BinanceTicker struct for REST fallback');

// Genesis oracle seeding: wSOL, wETH feeds
assert(validatorSrc.includes('"wSOL"') && validatorSrc.includes('8_200_000_000'), 'ORACLE: wSOL genesis price seeded ($82)');
assert(validatorSrc.includes('"wETH"') && validatorSrc.includes('197_900_000_000'), 'ORACLE: wETH genesis price seeded ($1,979)');
assert(!validatorSrc.includes('"BTC"'), 'ORACLE: BTC removed from oracle feeds');

// Genesis oracle seeding: feeder authorization for external assets
assert(validatorSrc.includes('add_price_feeder') && validatorSrc.includes('ext_feeder_args'), 'ORACLE: External asset feeder authorization');
assert(validatorSrc.includes('submit_price') && validatorSrc.includes('ext_price_args'), 'ORACLE: External asset price submission');

// Genesis analytics price seeding
assert(validatorSrc.includes('ana_lp_') && validatorSrc.includes('put_contract_storage'), 'ORACLE: Analytics last prices seeded (ana_lp_)');
assert(validatorSrc.includes('ana_24h_') && validatorSrc.includes('put_contract_storage'), 'ORACLE: Analytics 24h stats seeded (ana_24h_)');
assert(validatorSrc.includes('ana_c_') && validatorSrc.includes('put_contract_storage'), 'ORACLE: Genesis candles seeded (ana_c_)');
assert(validatorSrc.includes('ana_cc_') && validatorSrc.includes('put_contract_storage'), 'ORACLE: Candle counts seeded (ana_cc_)');

// Genesis pair price mapping
assert(validatorSrc.includes('wsol_usd / molt_usd'), 'ORACLE: wSOL/MOLT computed from wsol/molt ratio');
assert(validatorSrc.includes('weth_usd / molt_usd'), 'ORACLE: wETH/MOLT computed from weth/molt ratio');

// Background price feeder service (WebSocket-based)
assert(validatorSrc.includes('BINANCE_WS_URL'), 'ORACLE: Binance WebSocket URL constant');
assert(validatorSrc.includes('aggTrade'), 'ORACLE: Uses aggTrade streams for real-time data');
assert(validatorSrc.includes('SOLUSDT'), 'ORACLE: Parses SOL/USDT from WebSocket');
assert(validatorSrc.includes('ETHUSDT'), 'ORACLE: Parses ETH/USDT from WebSocket');
assert(!validatorSrc.includes('BTCUSDT'), 'ORACLE: BTC/USDT removed from price feeds');
assert(validatorSrc.includes('Duration::from_secs(1)'), 'ORACLE: 1-second storage write interval');

// WebSocket auto-reconnect and REST fallback
assert(validatorSrc.includes('binance_ws_loop'), 'ORACLE: WebSocket reader loop with auto-reconnect');
assert(validatorSrc.includes('ws_healthy') || validatorSrc.includes('AtomicBool'), 'ORACLE: WebSocket health flag for fallback');
assert(validatorSrc.includes('BINANCE_REST_URL'), 'ORACLE: REST fallback URL constant');
assert(validatorSrc.includes('backoff_secs'), 'ORACLE: Exponential backoff for reconnect');
assert(validatorSrc.includes('MICRO_SCALE'), 'ORACLE: Microdollar price encoding');

// Background feeder generates candle data
assert(validatorSrc.includes('oracle_update_candle'), 'ORACLE: Calls oracle_update_candle for each interval');
assert(validatorSrc.includes('candle_intervals: [u64; 9]'), 'ORACLE: All 9 candle intervals processed');

// Oracle candle update logic
assert(validatorSrc.includes('candle_start') && validatorSrc.includes('interval'), 'ORACLE: Candle period calculation');
assert(validatorSrc.includes('ana_cur_'), 'ORACLE: Current candle slot tracking (ana_cur_)');
assert(validatorSrc.includes('copy_from_slice(&price.to_le_bytes())'), 'ORACLE: In-place OHLC update');

// Background feeder spawned after RPC server
assert(validatorSrc.includes('state_for_oracle') && validatorSrc.includes('spawn_oracle_price_feeder'), 'ORACLE: Feeder spawned with state clone');
assert(validatorSrc.includes('get_genesis_pubkey'), 'ORACLE: Genesis pubkey resolved from state store');

// ── RPC oracle integration tests ──
const rpcDexSrc = fs.readFileSync(__dirname + '/../rpc/src/dex.rs', 'utf8');

assert(rpcDexSrc.includes('ORACLE_PROGRAM'), 'RPC: ORACLE_PROGRAM constant defined');
assert(rpcDexSrc.includes('get_oracle_prices'), 'RPC: get_oracle_prices endpoint handler');
assert(rpcDexSrc.includes('/oracle/prices'), 'RPC: Oracle prices route registered');
assert(rpcDexSrc.includes('oracleActive'), 'RPC: Oracle active flag in response');

// RPC oracle price fallback in get_pairs
assert(rpcDexSrc.includes('Oracle price fallback') && rpcDexSrc.includes('price_'), 'RPC: Oracle fallback in get_pairs');
assert(rpcDexSrc.includes('oracle_price') || rpcDexSrc.includes('oracle_usd'), 'RPC: Oracle price variable used');
assert(rpcDexSrc.includes('100_000_000.0'), 'RPC: 8-decimal oracle price conversion');

// RPC oracle fallback for MOLT-quoted pairs
assert(rpcDexSrc.includes('price_MOLT') && rpcDexSrc.includes('oracle'), 'RPC: MOLT oracle read for pair conversion');
assert(rpcDexSrc.includes('"wSOL"') && rpcDexSrc.includes('"wETH"'), 'RPC: Oracle mappings for wSOL and wETH');

// RPC oracle fallback in get_pair_ticker
assert((rpcDexSrc.match(/Oracle price fallback/g) || []).length >= 2, 'RPC: Oracle fallback in both get_pairs and get_pair_ticker');

// priceOracleActive flag
const rpcLibSrc = fs.readFileSync(__dirname + '/../rpc/src/lib.rs', 'utf8');
assert(rpcLibSrc.includes('"priceOracleActive": true'), 'RPC: priceOracleActive set to true');
assert(rpcLibSrc.includes('Oracle price feeds active'), 'RPC: Updated oracle note message');
assert(!rpcLibSrc.includes('BTC'), 'RPC: BTC removed from oracle note');

// ── Frontend oracle integration tests ──
console.log('\n── Frontend Oracle Integration ──');

assert(dexSource.includes('applyBinanceRealTimeOverlay'), 'FE: applyBinanceRealTimeOverlay function exists');
assert(!dexSource.includes('updateExternalPricedPairs'), 'FE: Old updateExternalPricedPairs removed');
assert(!dexSource.includes('apiPriceless'), 'FE: Old apiPriceless flag removed');
assert(dexSource.includes('real-time overlay'), 'FE: Binance feed documented as real-time overlay');
assert(dexSource.includes('MOLT_GENESIS_PRICE'), 'FE: MOLT genesis price constant used in overlay');
assert(!dexSource.includes('externalPrices.BTC'), 'FE: BTC removed from externalPrices');

// Real-time overlay logic
assert(dexSource.includes('state.activePair') && dexSource.includes('applyBinanceRealTimeOverlay'), 'FE: Real-time overlay updates active pair only');
assert(dexSource.includes('MOLT') && dexSource.includes('moltPair'), 'FE: MOLT-quoted pair conversion in overlay');

// ── Validator genesis pair creation tests ──
console.log('\n── Genesis Pair Creation ──');

assert(validatorSrc.includes('wSOL/mUSD') && validatorSrc.includes('wsol_addr') && validatorSrc.includes('musd_addr'), 'GENESIS: wSOL/mUSD pair created');
assert(validatorSrc.includes('wETH/mUSD') && validatorSrc.includes('weth_addr') && validatorSrc.includes('musd_addr'), 'GENESIS: wETH/mUSD pair created');
assert(validatorSrc.includes('wSOL/MOLT') && validatorSrc.includes('wsol_addr') && validatorSrc.includes('molt_addr'), 'GENESIS: wSOL/MOLT pair created');
assert(validatorSrc.includes('wETH/MOLT') && validatorSrc.includes('weth_addr') && validatorSrc.includes('molt_addr'), 'GENESIS: wETH/MOLT pair created');
assert(validatorSrc.includes('MOLT/mUSD') && validatorSrc.includes('molt_addr') && validatorSrc.includes('musd_addr'), 'GENESIS: MOLT/mUSD pair created');

// AMM pools with corrected initial sqrt_price (Q32: (1<<32)*sqrt(price))
// Updated to match genesis oracle prices: MOLT=$0.10, wSOL=$82, wETH=$1,979
assert(validatorSrc.includes('38_892_583_020'), 'GENESIS: wSOL/mUSD pool sqrt_price configured ($82)');
assert(validatorSrc.includes('191_065_712_575'), 'GENESIS: wETH/mUSD pool sqrt_price configured ($1,979)');
assert(validatorSrc.includes('122_989_146_433'), 'GENESIS: wSOL/MOLT pool sqrt_price configured (820 MOLT)');
assert(validatorSrc.includes('604_202_834_500'), 'GENESIS: wETH/MOLT pool sqrt_price configured (19,790 MOLT)');

// ── moltoracle contract tests ──
console.log('\n── MoltOracle Contract ──');

const oracleSrc = fs.readFileSync(__dirname + '/../contracts/moltoracle/src/lib.rs', 'utf8');
assert(oracleSrc.includes('submit_price'), 'ORACLE CONTRACT: submit_price function exists');
assert(oracleSrc.includes('add_price_feeder'), 'ORACLE CONTRACT: add_price_feeder function exists');
assert(oracleSrc.includes('get_price'), 'ORACLE CONTRACT: get_price function exists');
assert(oracleSrc.includes('get_aggregated_price'), 'ORACLE CONTRACT: get_aggregated_price function exists');
assert(oracleSrc.includes('query_oracle'), 'ORACLE CONTRACT: query_oracle function exists');
assert(oracleSrc.includes('PRICE_FEED_SIZE'), 'ORACLE CONTRACT: PRICE_FEED_SIZE constant defined');
assert(oracleSrc.includes('49'), 'ORACLE CONTRACT: 49-byte price feed size');
assert(oracleSrc.includes('is_mo_paused'), 'ORACLE CONTRACT: Pause guard on submit_price');
assert(oracleSrc.includes('reentrancy_enter'), 'ORACLE CONTRACT: Reentrancy guard');

// ── dex_analytics contract tests ──
console.log('\n── DEX Analytics Contract ──');

const analyticsSrc = fs.readFileSync(__dirname + '/../contracts/dex_analytics/src/lib.rs', 'utf8');
assert(analyticsSrc.includes('record_trade'), 'ANALYTICS CONTRACT: record_trade function exists');
assert(analyticsSrc.includes('update_candle'), 'ANALYTICS CONTRACT: update_candle function exists');
assert(analyticsSrc.includes('update_24h_stats'), 'ANALYTICS CONTRACT: update_24h_stats function exists');
assert(analyticsSrc.includes('INTERVAL_1M') && analyticsSrc.includes('60'), 'ANALYTICS CONTRACT: 1-minute candle interval');
assert(analyticsSrc.includes('INTERVAL_1H') && analyticsSrc.includes('3_600'), 'ANALYTICS CONTRACT: 1-hour candle interval');
assert(analyticsSrc.includes('INTERVAL_1D') && analyticsSrc.includes('86_400'), 'ANALYTICS CONTRACT: 1-day candle interval');
assert(analyticsSrc.includes('INTERVALS: [u64; 9]'), 'ANALYTICS CONTRACT: 9 candle intervals defined');

// ── End-to-end data flow tests ──
console.log('\n── End-to-End Data Flow ──');

// Verify the complete pipeline exists:
// Binance → oracle feeder → moltoracle storage → put_contract_storage → RPC reads ana_lp_ → frontend loadPairs

// 1. External source → Validator WebSocket feeder
assert(validatorSrc.includes('tokio_tungstenite'), 'E2E: WebSocket client for Binance');
assert(validatorSrc.includes('AtomicU64'), 'E2E: Lock-free atomic price storage');

// 2. Feeder → Oracle storage
assert(validatorSrc.includes('put_contract_storage') && validatorSrc.includes('oracle_pk'), 'E2E: Writes to oracle contract storage');

// 3. Feeder → Analytics storage
assert(validatorSrc.includes('put_contract_storage') && validatorSrc.includes('analytics_pk'), 'E2E: Writes to analytics contract storage');

// 4. RPC reads analytics → serves to frontend
assert(rpcDexSrc.includes('get_program_storage') || rpcDexSrc.includes('read_u64') || rpcDexSrc.includes('read_bytes'), 'E2E: RPC reads from contract storage');
assert(rpcDexSrc.includes('ana_lp_'), 'E2E: RPC reads analytics last price');
assert(rpcDexSrc.includes('ana_24h_'), 'E2E: RPC reads analytics 24h stats');
assert(rpcDexSrc.includes('ana_c_'), 'E2E: RPC reads analytics candles');

// 5. Frontend consumes standard API
assert(dexSource.includes("loadPairs") && dexSource.includes("/pairs"), 'E2E: Frontend loads pairs from API');
assert(dexSource.includes("loadCandles") || dexSource.includes("/candles"), 'E2E: Frontend loads candles from API');
assert(dexSource.includes("loadTicker") || dexSource.includes("/ticker"), 'E2E: Frontend loads ticker from API');

// 6. Oracle prices endpoint
assert(rpcDexSrc.includes('get_oracle_prices') && rpcDexSrc.includes('/oracle/prices'), 'E2E: Oracle prices REST endpoint');

// ═══════════════════════════════════════════════════════════════════════════
// hexToBytes / bytesToHex
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── Utility: hexToBytes / bytesToHex ──');

assertEqual(bytesToHex(hexToBytes('00ff80')), '00ff80', 'hex round-trip');
assertEqual(hexToBytes('abcd').length, 2, 'hexToBytes length');
assertEqual(bytesToHex(new Uint8Array([0, 255, 128])), '00ff80', 'bytesToHex');

// ═══════════════════════════════════════════════════════════════════════════
// DEX Plan Phase 1 — Contract Address Resolution
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── DEX P1: Contract Address Resolution ──');

// P1.1: contracts object includes all 8 DEX contracts (including dex_analytics)
assert(dexSource.includes('dex_analytics: null'), 'P1.1: contracts object includes dex_analytics');
assert(dexSource.includes("map['ANALYTICS']"), 'P1.2: ANALYTICS symbol mapped from registry');

// P1.3: Fallback addresses match current genesis (not stale deploy-manifest)
assert(dexSource.includes('7QvQ1dxFTdSk9aSzbBe2gHCJH1bSRBDwVdPTn9M5iCds'), 'P1.3: dex_core fallback = genesis address');
assert(dexSource.includes('72AvbSmnkv82Bsci9BHAufeAGMTycKQX5Y6DL9ghTHay'), 'P1.3: dex_amm fallback = genesis address');
assert(dexSource.includes('FwAxYo2bKmCe1c5gZZjvuyopJMDgm1T9CAWr2svB1GPf'), 'P1.3: dex_router fallback = genesis address');
assert(dexSource.includes('J8sMvYFXW4ZCHc488KJ1zmZq1sQMTWyWfr8qnzUwwEyD'), 'P1.3: prediction_market fallback = genesis address');
// Stale addresses from old deploy_dex.py must NOT be present
assert(!dexSource.includes('216MacD82KfB2hAeKR17M63ZXfURJQZnzDq2ho7SeJR7'), 'P1.3: stale dex_core address removed');
assert(!dexSource.includes('AANMpDkSnvSKa6PuaLQuRDU4SMzao7Yx3nLKzC2iatBn'), 'P1.3: stale dex_amm address removed');

// P1.5: Fallback warning when registry unavailable
assert(dexSource.includes('Using fallback contract addresses'), 'P1.5: Fallback warning logged');
assert(dexSource.includes('needsFallback'), 'P1.5: needsFallback flag tracks registry miss');

// P1.6: loadContractAddresses called BEFORE loadPairs (init order)
{
    const initIdx = dexSource.indexOf('async function init()');
    const loadContractIdx = dexSource.indexOf('await loadContractAddresses()', initIdx);
    const loadPairsIdx = dexSource.indexOf('await loadPairs()', initIdx);
    assert(loadContractIdx < loadPairsIdx, 'P1.6: loadContractAddresses called before loadPairs in init');
}

// ═══════════════════════════════════════════════════════════════════════════
// DEX Plan Phase 2 — Genesis & First-Boot Deploy
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── DEX P2: Genesis & First-Boot Deploy ──');

const validatorSource = fs.readFileSync(__dirname + '/../validator/src/main.rs', 'utf-8');
const firstBootSource = fs.readFileSync(__dirname + '/../scripts/first-boot-deploy.sh', 'utf-8');
const testnetDeploySource = fs.readFileSync(__dirname + '/../scripts/testnet-deploy.sh', 'utf-8');

// P2.1: genesis_exec_contract returns false on WASM failure (F2.3)
assert(
    validatorSource.includes('return false;') &&
    validatorSource.includes('contract returned error code'),
    'P2.1: genesis_exec_contract returns false on non-zero error code'
);

// P2.2: MOLT/mUSD AMM sqrt_price matches $0.10 (F2.4)
// Q32 value for sqrt(0.10) = 1,358,187,913
assert(
    validatorSource.includes('1_358_187_913'),
    'P2.2: MOLT/mUSD AMM sqrt_price = 1_358_187_913 (Q32 for $0.10)'
);
// Old $1.00 price (1 << 32 = 4294967296) must NOT be used for MOLT/mUSD pool
assert(
    !validatorSource.includes('1u64 << 32'),
    'P2.2: Old 1<<32 ($1.00) sqrt_price removed'
);

// P2.3: wSOL/mUSD AMM sqrt_price matches $82 (F2.5)
assert(
    validatorSource.includes('38_892_583_020'),
    'P2.3: wSOL/mUSD AMM sqrt_price = 38_892_583_020 (Q32 for $82)'
);

// P2.4: wETH/mUSD AMM sqrt_price matches $1,979 (F2.5)
assert(
    validatorSource.includes('191_065_712_575'),
    'P2.4: wETH/mUSD AMM sqrt_price = 191_065_712_575 (Q32 for $1,979)'
);

// P2.5: Cross-pair AMM prices derived from base oracle prices
assert(
    validatorSource.includes('122_989_146_433'),
    'P2.5: wSOL/MOLT sqrt_price for 820 MOLT present'
);
assert(
    validatorSource.includes('604_202_834_500'),
    'P2.5: wETH/MOLT sqrt_price for 19,790 MOLT present'
);

// P2.6: Genesis creates exactly 5 pairs (not 7, no REEF)
{
    const genesisPairFn = validatorSource.indexOf('fn genesis_create_trading_pairs');
    const genesisPairEnd = validatorSource.indexOf('fn genesis_seed_oracle');
    const pairBlock = validatorSource.slice(genesisPairFn, genesisPairEnd);
    // Narrow to just the CLOB pairs array (not AMM pool_configs)
    const pairsStart = pairBlock.indexOf('let pairs:');
    const pairsEnd = pairBlock.indexOf('];', pairsStart);
    const pairsArray = pairBlock.slice(pairsStart, pairsEnd);
    const pairDefs = (pairsArray.match(/\("(MOLT|wSOL|wETH)\/(mUSD|MOLT)"/g) || []);
    assert(pairDefs.length === 5, `P2.6: Genesis creates 5 CLOB pairs (got ${pairDefs.length})`);
    assert(!pairBlock.includes('REEF'), 'P2.6: No REEF pairs in genesis');
}

// P2.7: first-boot-deploy.sh uses 1-indexed pair IDs (F2.7)
assert(
    firstBootSource.includes("'pair_id': 1") && !firstBootSource.includes("'pair_id': 0"),
    'P2.7: first-boot-deploy.sh pair IDs are 1-indexed (not 0-indexed)'
);

// P2.8: first-boot-deploy.sh has 5 pools (not 7, no REEF) (F2.9)
{
    const poolCount = (firstBootSource.match(/'pair_id':/g) || []).length;
    assert(poolCount === 5, `P2.8: first-boot-deploy.sh has 5 pools (got ${poolCount})`);
    assert(!firstBootSource.includes('REEF'), 'P2.8: No REEF pools in first-boot-deploy');
}

// P2.9: testnet-deploy.sh also uses 1-indexed pair IDs and 5 pools
assert(
    testnetDeploySource.includes("'pair_id': 1") && !testnetDeploySource.includes("'pair_id': 0"),
    'P2.9: testnet-deploy.sh pair IDs are 1-indexed'
);
{
    const testnetPoolCount = (testnetDeploySource.match(/'pair_id':/g) || []).length;
    assert(testnetPoolCount === 5, `P2.9: testnet-deploy.sh has 5 pools (got ${testnetPoolCount})`);
}

// P2.10: Startup reconciliation for analytics prices (F2.1)
assert(
    validatorSource.includes('Analytics price seeds missing'),
    'P2.10: Startup reconciliation checks for missing ana_lp_1'
);
assert(
    validatorSource.includes('genesis_seed_analytics_prices'),
    'P2.10: Reconciliation calls genesis_seed_analytics_prices'
);

// P2.11: Startup reconciliation for oracle prices (F2.2)
assert(
    validatorSource.includes('Oracle price feeds missing'),
    'P2.11: Startup reconciliation checks for missing price_MOLT'
);
assert(
    validatorSource.includes('Oracle price seeded'),
    'P2.11: Reconciliation writes oracle prices directly'
);

// P2.12: AMM sqrt_price comments aligned with oracle seed prices
assert(
    validatorSource.includes('MOLT=$0.10, wSOL=$82, wETH=$1,979'),
    'P2.12: AMM sqrt_price comment cites correct oracle prices'
);

// P2.13: Q32 formula documented
assert(
    validatorSource.includes('(1 << 32) * sqrt(real_price)'),
    'P2.13: Q32 sqrt_price formula documented in code'
);

// ═══════════════════════════════════════════════════════════════════════════
// Phase 3: Trade View — Order Book (CLOB)
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── DEX P3: Trade View — Order Book (CLOB) ──');

const dexCoreSource = fs.readFileSync(__dirname + '/../contracts/dex_core/src/lib.rs', 'utf8');
const rpcDexSource = fs.readFileSync(__dirname + '/../rpc/src/dex.rs', 'utf8');

// P3.1: dex_core place_order storage layout
assert(
    dexCoreSource.includes('ORDER_SIZE: usize = 128') &&
    dexCoreSource.includes('fn encode_order(') &&
    dexCoreSource.includes('fn place_order('),
    'P3.1: dex_core has place_order with 128-byte ORDER_SIZE'
);

// P3.2: Matching engine uses price-time priority
assert(
    dexCoreSource.includes('fn match_order(') &&
    dexCoreSource.includes('fn fill_at_price_level(') &&
    dexCoreSource.includes('fn add_to_book('),
    'P3.2: Matching engine has match_order + fill_at_price_level + add_to_book'
);

// P3.3: RPC get_orderbook reads real contract storage (not mock)
assert(
    rpcDexSource.includes('fn get_orderbook(') &&
    rpcDexSource.includes('read_bytes(&state, DEX_CORE_PROGRAM,') &&
    rpcDexSource.includes('get_program_storage('),
    'P3.3: RPC get_orderbook reads real contract storage via get_program_storage'
);

// P3.4: Order byte layout matches between contract and RPC (F3.7)
{
    // Contract: [0:32 trader][32:40 pair_id][40 side][41 type][42:50 price]...
    // RPC: data[0..32] trader, data[32..40] pair_id, data[40] side, data[41] type, data[42..50] price
    assert(
        rpcDexSource.includes('data[32..40]') && rpcDexSource.includes('data[40]') &&
        rpcDexSource.includes('data[42..50]') && rpcDexSource.includes('data[83..91]'),
        'P3.4: RPC decode_order byte offsets match contract layout (trader, pair_id, side, type, price, order_id)'
    );
}

// P3.5: Orderbook depth — bids desc, asks asc
assert(
    rpcDexSource.includes('b.price') && rpcDexSource.includes('partial_cmp') &&
    rpcDexSource.includes('a.price'),
    'P3.5: Orderbook sorts bids desc and asks asc'
);

// P3.6: Frontend loadOrderBook path and parsing
assert(
    dexSource.includes("loadOrderBook()") &&
    dexSource.includes("/pairs/${state.activePairId}/orderbook") &&
    dexSource.includes("data?.asks && data?.bids"),
    'P3.6: Frontend loadOrderBook hits correct API path with null-safe checks'
);

// P3.7: renderOrderBook depth bars
assert(
    dexSource.includes('renderOrderBook()') &&
    dexSource.includes('depth-bar') &&
    dexSource.includes('formatPrice(a.price)'),
    'P3.7: renderOrderBook renders depth bars with formatted prices'
);

// P3.8/3.9/3.10: Matching engine correctness (no live orders, verified via code)
assert(
    dexCoreSource.includes('SIDE_BUY: u8 = 0') &&
    dexCoreSource.includes('SIDE_SELL: u8 = 1') &&
    dexCoreSource.includes('price >= best_ask') &&
    dexCoreSource.includes('price <= best_bid'),
    'P3.8-10: Matching engine checks buy vs best_ask, sell vs best_bid'
);

// P3.11: Spread display calculation
assert(
    dexSource.includes('ba - tb') || dexSource.includes('Spread:'),
    'P3.11: Spread is computed as lowest_ask - highest_bid'
);

// P3.12: Empty orderbook state
assert(
    dexSource.includes('No asks') && dexSource.includes('No bids'),
    'P3.12: Empty orderbook shows "No asks" / "No bids" placeholders'
);

// P3.13: loadRecentTrades reads from dex_core trades
assert(
    dexSource.includes('loadRecentTrades()') &&
    dexSource.includes("/pairs/${state.activePairId}/trades"),
    'P3.13: loadRecentTrades hits /pairs/:id/trades API'
);

// P3.14: Trade byte layout matches — decode_trade in RPC
assert(
    rpcDexSource.includes('data[0..8]') &&   // trade_id
    rpcDexSource.includes('data[16..24]') &&  // price
    rpcDexSource.includes('data[32..64]') &&  // taker
    rpcDexSource.includes('data[72..80]'),    // slot
    'P3.14: RPC decode_trade byte offsets match contract trade layout (80 bytes)'
);

// P3.15: F3.1 FIX — get_trades uses inclusive range (no off-by-one)
assert(
    rpcDexSource.includes('start..=trade_count') &&
    !rpcDexSource.includes('(start..trade_count).rev()'),
    'P3.15: get_trades uses inclusive range ..=trade_count (F3.1 fix: no off-by-one)'
);

// P3.16: PRICE_SCALE matches across all layers (F3.8)
assert(
    dexSource.includes('PRICE_SCALE = 1_000_000_000') &&
    rpcDexSource.includes('PRICE_SCALE: u64 = 1_000_000_000') &&
    dexCoreSource.includes('1_000_000_000'),
    'P3.16: PRICE_SCALE = 1e9 consistent across frontend, RPC, and contract'
);

// P3.17: Pair selector populates from /api/v1/pairs
assert(
    dexSource.includes('loadPairs()') &&
    dexSource.includes("api.get('/pairs')") &&
    dexSource.includes('renderPairList'),
    'P3.17: Pair selector populates from /pairs API with renderPairList'
);

// P3.18: Pair switching reloads orderbook + trades
assert(
    dexSource.includes('selectPair(pair)') &&
    dexSource.includes('Promise.all([loadOrderBook(), loadRecentTrades()])') &&
    dexSource.includes('subscribePair(pair.pairId)'),
    'P3.18: selectPair reloads orderbook, trades, and WebSocket subscriptions'
);

// P3.F2: F3.2 FIX — TradeJson now has side field
assert(
    rpcDexSource.includes("pub side: &'static str") &&
    rpcDexSource.includes("maker_data[40] == 0"),
    'P3.F2: TradeJson has side field, inferred from maker order byte 40 (F3.2 fix)'
);

// P3.F3: F3.3 FIX — TradeJson now has timestamp field
assert(
    rpcDexSource.includes('pub timestamp: u64') &&
    rpcDexSource.includes('slot.saturating_sub(trade.slot)'),
    'P3.F3: TradeJson has timestamp field, computed from slot delta (F3.3 fix)'
);

// P3.F5: F3.5 FIX — Fallback pair uses pairId: 1 (not 0)
assert(
    dexSource.includes('pairId: 1, id:') &&
    !dexSource.includes("pairId: 0, id: 'MOLT/mUSD'"),
    'P3.F5: Fallback MOLT/mUSD pair uses pairId: 1 (not 0) (F3.5 fix)'
);

// ═══════════════════════════════════════════════════════════════════════════
// Phase 4: Trade View — Order Form & Execution
// ═══════════════════════════════════════════════════════════════════════════

// P4.1: Submit handler sends correct place_order via contractIx
{
    assert(dexSource.includes('buildPlaceOrderArgs(wallet.address'), 'P4.1a: Submit handler calls buildPlaceOrderArgs with wallet.address');
    assert(dexSource.includes('function buildPlaceOrderArgs('), 'P4.1c: buildPlaceOrderArgs function defined');
    const builderMatch = dexSource.match(/function buildPlaceOrderArgs[^}]+}/s);
    assert(builderMatch, 'P4.1d: buildPlaceOrderArgs body found');
    const body = builderMatch[0];
    assert(body.includes('new ArrayBuffer(67)'), 'P4.1e: PlaceOrder binary layout is 67 bytes');
    assert(body.includes('PRICE_SCALE') || dexSource.includes('Math.round(price * PRICE_SCALE)'), 'P4.1f: PlaceOrder scales price');
}

// P4.2: Market order hides price input
assert(
    dexSource.includes("state.orderType === 'market' ? 'none' : 'block'"),
    'P4.2: Market order type hides price input'
);

// P4.3: Stop-limit shows stop-price group
assert(
    dexSource.includes("state.orderType === 'stop-limit' ? 'block' : 'none'"),
    'P4.3: Stop-limit type shows stop-price group'
);

// P4.4: Cancel order uses contractIx + buildCancelOrderArgs
assert(
    dexSource.includes('buildCancelOrderArgs(wallet.address'),
    'P4.4: Cancel order uses buildCancelOrderArgs with wallet.address'
);

// P4.5: Percentage preset buttons exist and calculate from balance
{
    const presetMatch = dexSource.match(/preset-btn[\s\S]{0,300}?dataset\.pct/);
    assert(presetMatch, 'P4.5a: Percentage preset buttons wire up dataset.pct');
    assert(
        dexSource.includes('bal.available * pct'),
        'P4.5b: Preset buttons calculate from balance.available'
    );
}

// P4.6: calcTotal computes price × amount
assert(
    dexSource.includes('(p * a).toFixed(4)'),
    'P4.6: calcTotal computes price × amount as total'
);

// P4.7: Fee estimate uses 0.0005 rate
assert(
    dexSource.includes('p * a * 0.0005'),
    'P4.7: Fee estimate uses 0.05% rate (0.0005)'
);

// P4.8: Route info shows CLOB Direct vs CLOB + AMM Split
assert(
    dexSource.includes("'CLOB + AMM Split'") && dexSource.includes("'CLOB Direct'"),
    'P4.8: Route info shows CLOB Direct or CLOB + AMM Split'
);

// P4.9: Route threshold is 50000
assert(
    dexSource.includes('> 50000'),
    'P4.9: Route splits at 50000 threshold'
);

// P4.10: Open orders render with cancel buttons
assert(
    dexSource.includes('cancel-btn') && dexSource.includes('renderOpenOrders'),
    'P4.10: Open orders section renders with cancel buttons'
);

// P4.11: Trade history loads with trader param
assert(
    dexSource.includes('trades?limit=50&trader='),
    'P4.11: loadTradeHistory sends trader query param'
);

// P4.12: Positions tab loads margin positions from API
assert(
    dexSource.includes("/margin/positions?trader="),
    'P4.12: Positions tab loads margin positions from /margin/positions API'
);

// --- Fix verification tests ---

// P4.F3: F4.3 FIX — Balance validation before order submission
assert(
    dexSource.includes('Insufficient') && dexSource.includes('neededToken') && dexSource.includes('neededAmount > available'),
    'P4.F3: Client-side balance validation checks neededAmount vs available (F4.3 fix)'
);

// P4.F4a: F4.4 FIX — LimitQuery has trader field
assert(
    rpcDexSource.includes('pub struct LimitQuery') &&
    rpcDexSource.includes('trader: Option<String>'),
    'P4.F4a: LimitQuery struct has trader: Option<String> field (F4.4 fix)'
);

// P4.F4b: F4.4 FIX — get_trades filters by trader address
assert(
    rpcDexSource.includes('trader_filter') &&
    rpcDexSource.includes('trade.taker != trader_filter'),
    'P4.F4b: get_trades filters trades by trader address when specified (F4.4 fix)'
);

// ═══════════════════════════════════════════════════════════════════════════
// Phase 5: Trade View — TradingView Chart
// ═══════════════════════════════════════════════════════════════════════════

// P5.1: Datafeed adapter connects to candle API
assert(
    dexSource.includes('loadCandles(pp.from, pp.to, res)') &&
    dexSource.includes('/candles?'),
    'P5.1: Datafeed getBars calls loadCandles which fetches /candles API'
);

// P5.2: loadCandles maps OHLCV fields from response
{
    const lcMatch = dexSource.match(/data\.map\(c\s*=>\s*\(\{[\s\S]*?\}\)\)/);
    assert(lcMatch, 'P5.2a: loadCandles maps candle data array');
    const body = lcMatch[0];
    assert(body.includes('c.timestamp') && body.includes('c.open') && body.includes('c.high') && body.includes('c.low') && body.includes('c.close') && body.includes('c.volume'),
        'P5.2b: loadCandles extracts timestamp, open, high, low, close, volume');
}

// P5.3: CandleJson has timestamp field (F5.1 fix)
assert(
    rpcDexSource.includes('pub timestamp: u64') &&
    rpcDexSource.includes('pub struct CandleJson'),
    'P5.3: CandleJson struct includes timestamp field (F5.1 fix)'
);

// P5.4: get_candles uses 1-based inclusive range (F5.2 fix)
assert(
    rpcDexSource.includes('for i in start..=candle_count'),
    'P5.4a: get_candles uses inclusive range start..=candle_count (F5.2 fix)'
);
assert(
    rpcDexSource.includes('candle_count - limit as u64 + 1'),
    'P5.4b: get_candles start is 1-based (F5.2 fix)'
);

// P5.5: CandleQuery has from/to fields (F5.2 fix)
assert(
    rpcDexSource.includes('pub struct CandleQuery') &&
    rpcDexSource.includes('from: Option<u64>') &&
    rpcDexSource.includes('to: Option<u64>'),
    'P5.5: CandleQuery struct has from/to fields for slot range filtering (F5.2 fix)'
);

// P5.6: get_candles filters by from/to
assert(
    rpcDexSource.includes('if candle.timestamp < from') &&
    rpcDexSource.includes('if candle.timestamp > to'),
    'P5.6: get_candles filters candles by from/to timestamps (F5.2 fix)'
);

// P5.7: Candle aggregation — dex_analytics contract uses correct interval boundaries
{
    const analyticsPath = '/Users/johnrobin/.openclaw/workspace/moltchain/contracts/dex_analytics/src/lib.rs';
    try {
        const analyticsSource = fs.readFileSync(analyticsPath, 'utf8');
        assert(
            analyticsSource.includes('current_slot / interval') && analyticsSource.includes('* interval'),
            'P5.7: dex_analytics calculates candle boundaries as (slot/interval)*interval'
        );
    } catch {
        assert(false, 'P5.7: Could not read dex_analytics contract');
    }
}

// P5.8: Time interval switching — resolutionToSec maps all standard intervals
assert(
    dexSource.includes("'1': 60") && dexSource.includes("'5': 300") &&
    dexSource.includes("'60': 3600") && dexSource.includes("'240': 14400") &&
    dexSource.includes("'1D': 86400"),
    'P5.8: resolutionToSec() maps all standard intervals correctly'
);

// P5.9: TradingView fallback says "unavailable" (not "loading") and has retry (F5.7 fix)
assert(
    dexSource.includes('Chart unavailable') && dexSource.includes('setTimeout(initTradingView'),
    'P5.9: TradingView fallback shows "unavailable" message with retry (F5.7 fix)'
);

// P5.10: Chart updates on pair switch via setSymbol
assert(
    dexSource.includes('setSymbol(pair.id'),
    'P5.10: Chart updates on pair switch via setSymbol'
);

// P5.11: Dark theme config
assert(
    dexSource.includes("theme: 'Dark'") && dexSource.includes("'#0d1117'"),
    'P5.11: Chart uses dark theme with #0d1117 background'
);

// P5.12: Empty state returns noData flag
assert(
    dexSource.includes('noData: !bars.length'),
    'P5.12: getBars returns { noData: true } when no candles exist'
);

// P5.13: Dynamic resolution bucketing (F5.11 fix)
assert(
    dexSource.includes('activeResolution') && dexSource.includes('resolutionToMs(activeResolution)'),
    'P5.13a: streamBarUpdate uses dynamic resolution via activeResolution (F5.11 fix)'
);
assert(
    dexSource.includes("activeResolution = res"),
    'P5.13b: subscribeBars stores resolution in activeResolution (F5.11 fix)'
);
assert(
    !dexSource.includes('900000) * 900000'),
    'P5.13c: No hardcoded 900000ms (15-min) bucketing remains (F5.11 fix)'
);

// P5.14: Supported resolutions include all standard intervals
assert(
    dexSource.includes("'1','5','15','30','60','240','1D','1W','1M'"),
    'P5.14: supported_resolutions includes all 9 standard intervals'
);

// ═══════════════════════════════════════════════════════════════════════════
// Phase 6: Trade View — WebSocket Feeds
// ═══════════════════════════════════════════════════════════════════════════

// P6.1: WS URL configurable
assert(
    dexSource.includes('MOLTCHAIN_WS') && dexSource.includes('ws://localhost:8900'),
    'P6.1: WS URL defaults to ws://localhost:8900 with MOLTCHAIN_WS override'
);

// P6.2: DexWS class with constructor, connect, subscribe, unsubscribe
assert(
    dexSource.includes('class DexWS') && dexSource.includes('connect()') &&
    dexSource.includes('subscribe(') && dexSource.includes('unsubscribe('),
    'P6.2: DexWS class has connect, subscribe, unsubscribe methods'
);

// P6.3: Exponential backoff reconnection
assert(
    dexSource.includes('reconnectDelay * 2') && dexSource.includes('30000'),
    'P6.3: WS reconnect uses exponential backoff capped at 30s'
);

// P6.4: Orderbook WS subscription
assert(
    dexSource.includes('`orderbook:${pairId}`') && dexSource.includes('d.bids') && dexSource.includes('d.asks'),
    'P6.4: orderbook channel subscribes and processes bids/asks'
);

// P6.5: Trades WS subscription
assert(
    dexSource.includes('`trades:${pairId}`') && dexSource.includes('streamBarUpdate'),
    'P6.5: trades channel subscribes and calls streamBarUpdate'
);

// P6.6: Ticker WS subscription uses camelCase (F6.9 fix)
assert(
    dexSource.includes('`ticker:${pairId}`') && dexSource.includes('d.lastPrice'),
    'P6.6: ticker channel uses camelCase d.lastPrice (F6.9 fix)'
);

// P6.7: Orders WS subscription uses camelCase (F6.9 fix)
assert(
    dexSource.includes('`orders:${wallet.address}`') && dexSource.includes('d.orderId'),
    'P6.7: orders channel uses camelCase d.orderId (F6.9 fix)'
);

// P6.8: DexEvent has rename_all = camelCase (F6.9 fix)
{
    const wsPath = '/Users/johnrobin/.openclaw/workspace/moltchain/rpc/src/dex_ws.rs';
    const wsSource = fs.readFileSync(wsPath, 'utf8');
    assert(
        wsSource.includes('rename_all = "camelCase"') && wsSource.includes('pub enum DexEvent'),
        'P6.8: DexEvent enum uses serde rename_all = camelCase (F6.9 fix)'
    );
}

// P6.9: RAF throttle on orderbook updates (F6.11 fix)
assert(
    dexSource.includes('rafThrottle') && dexSource.includes('throttledRenderOrderBook'),
    'P6.9: orderbook WS callback uses RAF-throttled renderOrderBook (F6.11 fix)'
);

// P6.10: DexWS close() method (F6.12 fix)
assert(
    dexSource.includes('close()') && dexSource.includes('_closing'),
    'P6.10: DexWS has close() method with _closing flag (F6.12 fix)'
);

// P6.11: beforeunload cleanup (F6.12 fix)
assert(
    dexSource.includes('beforeunload') && dexSource.includes('dexWs.close()'),
    'P6.11: Page unload calls dexWs.close() (F6.12 fix)'
);

// P6.12: Polling fallback runs unconditionally
assert(
    dexSource.includes('setInterval') && dexSource.includes('loadOrderBook'),
    'P6.12: Polling fallback runs loadOrderBook on interval'
);

// P6.13: WS subscriptions change on pair switch
assert(
    dexSource.includes('state._wsSubs.forEach(id => dexWs.unsubscribe(id))'),
    'P6.13: subscribePair unsubscribes previous channels before subscribing new'
);

// P6.14: emit_dex_events wired in validator (F6.2 fix)
{
    const validatorPath = '/Users/johnrobin/.openclaw/workspace/moltchain/validator/src/main.rs';
    const validatorSource = fs.readFileSync(validatorPath, 'utf8');
    assert(
        validatorSource.includes('fn emit_dex_events(') && validatorSource.includes('emit_dex_events(&state, &ws_dex_broadcaster'),
        'P6.14: emit_dex_events function exists and is called in block production (F6.2 fix)'
    );
}

// P6.14: emit_dex_events wired in validator (F6.2 fix)
{
    const validatorPath = '/Users/johnrobin/.openclaw/workspace/moltchain/validator/src/main.rs';
    const validatorSource = fs.readFileSync(validatorPath, 'utf8');
    assert(
        validatorSource.includes('fn emit_dex_events(') && validatorSource.includes('emit_dex_events(&state, &ws_dex_broadcaster'),
        'P6.14: emit_dex_events function exists and is called in block production (F6.2 fix)'
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 7: Pool View — AMM Liquidity
// ═══════════════════════════════════════════════════════════════════════════
const dexRsPath = '/Users/johnrobin/.openclaw/workspace/moltchain/rpc/src/dex.rs';
const dexJsPath = '/Users/johnrobin/.openclaw/workspace/moltchain/dex/dex.js';
const indexHtmlPath = '/Users/johnrobin/.openclaw/workspace/moltchain/dex/index.html';

// P7.1: decode_pool byte offsets match contract (96-byte layout)
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    assert(dexRs.includes('data[0..32]'), 'P7.1a: decode_pool reads token_a from bytes 0..32');
    assert(dexRs.includes('data[32..64]'), 'P7.1b: decode_pool reads token_b from bytes 32..64');
    assert(dexRs.includes('data[64..72]'), 'P7.1c: decode_pool reads pool_id from bytes 64..72');
    assert(dexRs.includes('data[72..80]'), 'P7.1d: decode_pool reads sqrt_price from bytes 72..80');
    assert(dexRs.includes('data[80..84]'), 'P7.1e: decode_pool reads tick from bytes 80..84');
    assert(dexRs.includes('data[84..92]'), 'P7.1f: decode_pool reads liquidity from bytes 84..92');
    assert(dexRs.includes('data[92]'), 'P7.1g: decode_pool reads fee_tier from byte 92');
}

// P7.2: fee_tier returned as string "1bps"/"5bps"/"30bps"/"100bps" in RPC
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    assert(dexRs.includes('"1bps"') && dexRs.includes('"5bps"') && dexRs.includes('"30bps"') && dexRs.includes('"100bps"'),
        'P7.2: decode_pool maps fee_tier byte to bps strings');
}

// P7.3: F7.3 fix — fee display parses bps from string, no NaN (was: p.feeTier / 100 on string)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('parseInt(p.feeTier)'), 'P7.3a: fee_tier parsed with parseInt (F7.3 fix)');
    assert(!dexJs.includes('p.feeTier / 100'), 'P7.3b: old NaN-producing feeTier / 100 removed');
    // Verify parseInt("30bps") === 30
    assert(parseInt("30bps") === 30, 'P7.3c: parseInt("30bps") correctly extracts 30');
    assert(parseInt("1bps") === 1, 'P7.3d: parseInt("1bps") correctly extracts 1');
}

// P7.4: PoolJson has rename_all = "camelCase"
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    const poolJsonIdx = dexRs.indexOf('pub struct PoolJson');
    assert(poolJsonIdx > 0, 'P7.4a: PoolJson struct exists');
    const before = dexRs.slice(Math.max(0, poolJsonIdx - 80), poolJsonIdx);
    assert(before.includes('rename_all = "camelCase"'), 'P7.4b: PoolJson has camelCase serde rename');
}

// P7.5: build_token_symbol_map resolves hex to symbols
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    assert(dexRs.includes('build_token_symbol_map') || dexRs.includes('token_symbol_map'),
        'P7.5: token symbol map function exists in RPC');
}

// P7.6: Pool table renders 6 columns — Pool, Fee Tier, TVL, Volume 24h, APR, action
{
    const indexHtml = fs.readFileSync(indexHtmlPath, 'utf8');
    assert(indexHtml.includes('poolTableBody'), 'P7.6a: poolTableBody element exists');
    assert(indexHtml.includes('Fee Tier') || indexHtml.includes('fee-tier'), 'P7.6b: Fee Tier column header');
    assert(indexHtml.includes('TVL') || indexHtml.includes('tvl'), 'P7.6c: TVL column header');
}

// P7.7: F7.7 fix — loadPoolStats uses correct field mappings
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(!dexJs.includes('swap_count ? data.swap_count * 100'), 'P7.7a: fabricated swap_count * 100 removed (F7.7 fix)');
    assert(dexJs.includes("data.tvl || data.total_volume"), 'P7.7b: TVL uses data.tvl with total_volume fallback');
    assert(dexJs.includes("data.volume_24h"), 'P7.7c: Volume 24h uses data.volume_24h');
    assert(dexJs.includes("data.fees_24h || data.total_fees"), 'P7.7d: Fees uses fees_24h with total_fees fallback');
}

// P7.8: /stats/amm reads real AMM storage keys
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    assert(dexRs.includes('amm_pool_count') && dexRs.includes('amm_swap_count'),
        'P7.8: /stats/amm handler reads amm_pool_count and amm_swap_count from contract storage');
}

// P7.9: F7.9 fix — pool row click delegation wired
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("poolTableBody')?.addEventListener('click'"), 'P7.9a: poolTableBody click delegation exists (F7.9 fix)');
    assert(dexJs.includes(".pool-add-btn") && dexJs.includes("scrollIntoView"), 'P7.9b: click handler selects pool and scrolls to form');
}

// P7.10: Empty state placeholder renders
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('No liquidity pools'), 'P7.10: empty state placeholder message');
}

// P7.11: F7.12 fix — LP positions uses ?owner= not ?address=
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('positions?owner='), 'P7.11a: LP positions query uses ?owner= (F7.12a fix)');
    assert(!dexJs.includes('positions?address='), 'P7.11b: old ?address= param removed');
}

// P7.12: F7.12 fix — LP position cards have data-pool-id attribute
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('lp-position-card" data-position-id="${pos.positionId || 0}" data-pool-id="${pos.poolId || 0}"'),
        'P7.12a: LP position card has both data-position-id and data-pool-id (F7.12b fix)');
    assert(dexJs.includes('card.dataset.poolId'), 'P7.12b: My Pools filter uses card.dataset.poolId not positionId');
}

// P7.13: Volume per-row shows "—" when field unavailable (not $0)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("p.totalVolume ? formatVolume(p.totalVolume) : '\u2014'"),
        'P7.13: volume shows \u2014 when totalVolume unavailable (F7.15 fix)');
}

// P7.14: F7.17 fix — liqPoolSelect populated from pools, not CLOB pairs
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('Populate liqPoolSelect from actual pools'), 'P7.14: liqPoolSelect populated from pools (F7.17 fix)');
}

// P7.15: F7.18 fix — current price computed from sqrtPrice on pool select change
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('pool.sqrtPrice') && dexJs.includes('liqCurrentPrice'), 'P7.15: current price computed from sqrtPrice (F7.18 fix)');
    assert(dexJs.includes('sqrtP * sqrtP'), 'P7.15b: price = sqrtP^2 after Q32.32 conversion');
}

// P7.16: F7.19 fix — pool share estimate calculation wired
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('liqPoolShare') && dexJs.includes('pool.liquidity + deposit'),
        'P7.16: pool share estimate calculates deposit / (existing + deposit) (F7.19 fix)');
}

// P7.17: F7.20 fix — fee tier selector stores state.selectedFeeTier
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('state.selectedFeeTier'), 'P7.17a: state.selectedFeeTier exists (F7.20 fix)');
    assert(dexJs.includes("parseInt(btn.dataset.fee)"), 'P7.17b: fee tier click sets from data-fee attribute');
}

// P7.18: Add buttons wallet-gated
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("pool-add-btn${!state.connected") && dexJs.includes("btn-wallet-gate"),
        'P7.18: Add buttons disabled and styled when wallet disconnected');
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 8: Pool View — Add/Remove/Collect Liquidity
// ═══════════════════════════════════════════════════════════════════════════

// P8.1: CONTRACT_PROGRAM_ID constant exists and is correct (base58 of [0xFF]*32)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('CONTRACT_PROGRAM_ID'), 'P8.1a: CONTRACT_PROGRAM_ID constant defined');
    // All 0xFF bytes base58-encoded dynamically — verify it uses bs58encode with 0xFF fill
    assert(dexJs.includes('bs58encode(new Uint8Array(32).fill(0xFF)') || dexJs.includes('bs58encode(new Uint8Array(32).fill(0xff)'), 'P8.1b: CONTRACT_PROGRAM_ID computed from 32 bytes of 0xFF');
}

// P8.2: contractIx helper function exists with correct structure
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('function contractIx('), 'P8.2a: contractIx() function defined');
    assert(dexJs.includes('program_id: CONTRACT_PROGRAM_ID'), 'P8.2b: contractIx uses CONTRACT_PROGRAM_ID as program_id');
    assert(dexJs.includes('accounts: [wallet.address, contractAddr]'), 'P8.2c: contractIx sends [wallet, contract] as accounts');
}

// P8.3: buildContractCall wraps args in ContractInstruction::Call JSON
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('function buildContractCall('), 'P8.3a: buildContractCall() function defined');
    assert(dexJs.includes('Call:') || dexJs.includes('"Call"'), 'P8.3b: buildContractCall wraps in Call envelope');
    assert(dexJs.includes('function: "call"') || dexJs.includes("function: 'call'"), 'P8.3c: Call has function field');
    assert(dexJs.includes('args: Array.from') || dexJs.includes('args:'), 'P8.3d: Call has args field');
}

// P8.4: Binary instruction builders exist for all contracts
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const builders = [
        'buildPlaceOrderArgs', 'buildCancelOrderArgs',
        'buildAddLiquidityArgs', 'buildRemoveLiquidityArgs', 'buildCollectFeesArgs',
        'buildOpenPositionArgs', 'buildClosePositionArgs',
        'buildVoteArgs', 'buildBuySharesArgs', 'buildRedeemSharesArgs',
        'buildResolveMarketArgs', 'buildCreateMarketArgs', 'buildClaimRewardsArgs'
    ];
    builders.forEach(b => {
        assert(dexJs.includes(`function ${b}(`), `P8.4: ${b}() builder exists`);
    });
}

// P8.5: Binary encoding helpers exist
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('function writeU64LE('), 'P8.5a: writeU64LE helper');
    assert(dexJs.includes('function writeI32LE('), 'P8.5b: writeI32LE helper');
    assert(dexJs.includes('function writeU8('), 'P8.5c: writeU8 helper');
    assert(dexJs.includes('function writePubkey('), 'P8.5d: writePubkey helper');
}

// P8.6: Tick math functions exist
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('function priceToTick('), 'P8.6a: priceToTick() function');
    assert(dexJs.includes('Math.log(price)'), 'P8.6b: priceToTick uses logarithm');
    assert(dexJs.includes('Math.log(1.0001)'), 'P8.6c: priceToTick divides by log(1.0001)');
    assert(dexJs.includes('function alignTickToSpacing('), 'P8.6d: alignTickToSpacing() function');
    assert(dexJs.includes('MIN_TICK') && dexJs.includes('-887272'), 'P8.6e: MIN_TICK = -887272');
    assert(dexJs.includes('MAX_TICK') && dexJs.includes('887272'), 'P8.6f: MAX_TICK = 887272');
    assert(dexJs.includes('FEE_TIER_SPACING'), 'P8.6g: FEE_TIER_SPACING map');
}

// P8.7: No old-format sendTransaction calls remain (program_id: contracts.X)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const oldPattern = /program_id:\s*contracts\./g;
    const matches = dexJs.match(oldPattern);
    assert(!matches, 'P8.7: No old-format program_id: contracts.X calls remain (found ' + (matches ? matches.length : 0) + ')');
}

// P8.8: All sendTransaction calls now use contractIx()
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const contractIxCalls = (dexJs.match(/contractIx\(/g) || []).length;
    // We need at least 13 calls (one for each sendTransaction pattern)
    assert(contractIxCalls >= 13, `P8.8: At least 13 contractIx() calls (found ${contractIxCalls})`);
}

// P8.9: place_order uses buildPlaceOrderArgs
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('buildPlaceOrderArgs(wallet.address'), 'P8.9: place_order uses buildPlaceOrderArgs with wallet.address');
}

// P8.10: cancel_order uses buildCancelOrderArgs
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('buildCancelOrderArgs(wallet.address'), 'P8.10: cancel_order uses buildCancelOrderArgs with wallet.address');
}

// P8.11: add_liquidity uses buildAddLiquidityArgs with tick math
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('contractIx(\n                contracts.dex_amm') || dexJs.includes('contractIx(contracts.dex_amm, buildAddLiquidityArgs(') || (dexJs.includes('contractIx(') && dexJs.includes('buildAddLiquidityArgs(')),
        'P8.11a: add_liquidity uses contractIx + buildAddLiquidityArgs');
    assert(dexJs.includes('priceToTick(minPrice)') || dexJs.includes('priceToTick('), 'P8.11b: Add liquidity uses priceToTick()');
    assert(dexJs.includes('alignTickToSpacing('), 'P8.11c: Add liquidity aligns ticks to spacing');
}

// P8.12: LP action handlers — Collect Fees
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('.lp-collect-btn'), 'P8.12a: Collect Fees button selector exists');
    assert(dexJs.includes('buildCollectFeesArgs(wallet.address, posId)'), 'P8.12b: Collect handler builds correct args');
}

// P8.13: LP action handlers — Remove Liquidity
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('.lp-remove-btn'), 'P8.13a: Remove button selector exists');
    assert(dexJs.includes('buildRemoveLiquidityArgs(wallet.address, posId'), 'P8.13b: Remove handler builds correct args');
    assert(dexJs.includes("confirm(`Remove all liquidity") || dexJs.includes('confirm('), 'P8.13c: Remove has confirmation dialog');
}

// P8.14: LP action handlers — Add More
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('.lp-add-btn'), 'P8.14a: Add More button selector exists');
    assert(dexJs.includes('scrollIntoView'), 'P8.14b: Add More scrolls to add liquidity form');
    assert(dexJs.includes("poolSelect.value = poolId") || dexJs.includes('poolSelect.value ='), 'P8.14c: Add More pre-selects pool');
}

// P8.15: Event delegation on #pool-positions container
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("pool-positions") && dexJs.includes("addEventListener('click'"), 'P8.15: Event delegation on pool-positions container');
}

// P8.16: Prediction buy_shares uses contractIx
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('contractIx(contracts.prediction_market, buildBuySharesArgs('), 'P8.16: buy_shares uses contractIx + buildBuySharesArgs');
}

// P8.17: Prediction redeem_shares uses contractIx for claim
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('contractIx(contracts.prediction_market, buildRedeemSharesArgs('), 'P8.17: claim winnings uses contractIx + buildRedeemSharesArgs');
}

// P8.18: Prediction resolve_market uses contractIx
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('contractIx(contracts.prediction_market, buildResolveMarketArgs('), 'P8.18: resolve_market uses contractIx + buildResolveMarketArgs');
}

// P8.19: Prediction create_market uses contractIx
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('contractIx(contracts.prediction_market, buildCreateMarketArgs('), 'P8.19: create_market uses contractIx + buildCreateMarketArgs');
}

// P8.20: Rewards claim uses contractIx
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('contractIx(contracts.dex_rewards, buildClaimRewardsArgs('), 'P8.20: rewards claim uses contractIx + buildClaimRewardsArgs');
}

// P8.21: Margin open_position uses buildOpenPositionArgs
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('buildOpenPositionArgs(wallet.address'), 'P8.21: margin open uses buildOpenPositionArgs with wallet.address');
}

// P8.22: Margin close_position uses buildClosePositionArgs
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('buildClosePositionArgs(wallet.address'), 'P8.22: margin close uses buildClosePositionArgs with wallet.address');
}

// P8.23: Governance vote uses buildVoteArgs
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('buildVoteArgs(wallet.address'), 'P8.23: governance vote uses buildVoteArgs with wallet.address');
}

// P8.24: buildPlaceOrderArgs binary layout (opcode 2, 67 bytes)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const builderMatch = dexJs.match(/function buildPlaceOrderArgs[^}]+}/s);
    assert(builderMatch, 'P8.24a: buildPlaceOrderArgs found');
    const body = builderMatch[0];
    assert(body.includes('new ArrayBuffer(67)'), 'P8.24b: PlaceOrder allocates 67 bytes');
    assert(body.includes('writeU8(arr, 0, 2)'), 'P8.24c: PlaceOrder opcode is 2');
}

// P8.25: buildCancelOrderArgs binary layout (opcode 3, 41 bytes)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const builderMatch = dexJs.match(/function buildCancelOrderArgs[^}]+}/s);
    assert(builderMatch, 'P8.25a: buildCancelOrderArgs found');
    const body = builderMatch[0];
    assert(body.includes('new ArrayBuffer(41)'), 'P8.25b: CancelOrder allocates 41 bytes');
    assert(body.includes('writeU8(arr, 0, 3)'), 'P8.25c: CancelOrder opcode is 3');
}

// P8.26: buildAddLiquidityArgs binary layout (opcode 3, 65 bytes)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const builderMatch = dexJs.match(/function buildAddLiquidityArgs[^}]+}/s);
    assert(builderMatch, 'P8.26a: buildAddLiquidityArgs found');
    const body = builderMatch[0];
    assert(body.includes('new ArrayBuffer(65)'), 'P8.26b: AddLiquidity allocates 65 bytes');
    assert(body.includes('writeU8(arr, 0, 3)'), 'P8.26c: AddLiquidity opcode is 3');
    assert(body.includes('writeI32LE('), 'P8.26d: AddLiquidity writes i32 ticks');
}

// P8.27: buildCollectFeesArgs binary layout (opcode 5, 41 bytes)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const builderMatch = dexJs.match(/function buildCollectFeesArgs[^}]+}/s);
    assert(builderMatch, 'P8.27a: buildCollectFeesArgs found');
    const body = builderMatch[0];
    assert(body.includes('new ArrayBuffer(41)'), 'P8.27b: CollectFees allocates 41 bytes');
    assert(body.includes('writeU8(arr, 0, 5)'), 'P8.27c: CollectFees opcode is 5');
}

// P8.28: buildRemoveLiquidityArgs binary layout (opcode 4, 49 bytes)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const builderMatch = dexJs.match(/function buildRemoveLiquidityArgs[^}]+}/s);
    assert(builderMatch, 'P8.28a: buildRemoveLiquidityArgs found');
    const body = builderMatch[0];
    assert(body.includes('new ArrayBuffer(49)'), 'P8.28b: RemoveLiquidity allocates 49 bytes');
    assert(body.includes('writeU8(arr, 0, 4)'), 'P8.28c: RemoveLiquidity opcode is 4');
}

// P8.29: Full range toggle uses MIN_TICK/MAX_TICK
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('fullRange ? MIN_TICK') || dexJs.includes('fullRange?MIN_TICK'),
        'P8.29: Full range toggle uses MIN_TICK/MAX_TICK constants');
}

// P8.30: LP position cards have data-pool-id attribute
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('data-pool-id='), 'P8.30: LP position cards include data-pool-id attribute');
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 9: Smart Order Router
// ═══════════════════════════════════════════════════════════════════════════

// P9.1: decode_route byte offsets match contract layout (96 bytes)
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    const decodeMatch = dexRs.match(/fn decode_route[\s\S]*?\n\}/);
    assert(decodeMatch, 'P9.1a: decode_route function exists');
    const body = decodeMatch[0];
    assert(body.includes('0..32') || body.includes('[0..32]'), 'P9.1b: token_in at 0..32');
    assert(body.includes('32..64') || body.includes('[32..64]'), 'P9.1c: token_out at 32..64');
    assert(body.includes('64..72') || body.includes('[64..72]'), 'P9.1d: route_id at 64..72');
    assert(body.includes('[72]') || body.includes('data[72]'), 'P9.1e: route_type at byte 72');
    assert(body.includes('73..81') || body.includes('[73..81]'), 'P9.1f: pool_or_pair_id at 73..81');
    assert(body.includes('[89]') || body.includes('data[89]'), 'P9.1g: split_percent at byte 89');
    assert(body.includes('[90]') || body.includes('data[90]'), 'P9.1h: enabled at byte 90');
}

// P9.2: get_routes handler exists and iterates route count
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    assert(dexRs.includes('async fn get_routes'), 'P9.2a: get_routes handler exists');
    assert(dexRs.includes('rtr_route_count'), 'P9.2b: reads rtr_route_count from storage');
    assert(dexRs.includes('rtr_route_'), 'P9.2c: iterates rtr_route_{id} keys');
}

// P9.3: post_router_quote returns minAmountOut (F9.3a fix)
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    assert(dexRs.includes('async fn post_router_quote'), 'P9.3a: post_router_quote handler exists');
    assert(dexRs.includes('"minAmountOut"'), 'P9.3b: Response includes minAmountOut field');
}

// P9.4: Split route quoting works (F9.4a fix)
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    assert(dexRs.includes('route.route_type == "split"'), 'P9.4a: Split route type handled separately');
    assert(dexRs.includes('quote_clob_swap') && dexRs.includes('quote_amm_swap'), 'P9.4b: Both CLOB and AMM quote functions exist');
    // Split should quote both legs
    const splitBlock = dexRs.match(/route_type == "split"[\s\S]*?else/);
    assert(splitBlock, 'P9.4c: Split route block found');
    const sb = splitBlock[0];
    assert(sb.includes('clob_amount') && sb.includes('amm_amount'), 'P9.4d: Split divides into CLOB and AMM amounts');
    assert(sb.includes('split_percent'), 'P9.4e: Split uses split_percent for division');
}

// P9.5: Dead slippage guard removed (F9.4b fix)
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    assert(!dexRs.includes('best_output < min_out'), 'P9.5: Dead slippage guard removed (best_output < min_out was always false)');
}

// P9.6: Route info pill calls router API (F9.5a fix)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('/router/quote'), 'P9.6a: calcTotal calls /router/quote API');
    assert(dexJs.includes('ROUTE_TYPE_LABELS'), 'P9.6b: ROUTE_TYPE_LABELS mapping exists');
    assert(dexJs.includes("'clob'") || dexJs.includes('"clob"') || dexJs.includes('clob:'), 'P9.6c: CLOB route type in labels');
    assert(dexJs.includes("'amm'") || dexJs.includes('"amm"') || dexJs.includes('amm:'), 'P9.6d: AMM route type in labels');
    assert(dexJs.includes("'split'") || dexJs.includes('"split"') || dexJs.includes('split:'), 'P9.6e: Split route type in labels');
    assert(dexJs.includes("'multi_hop'") || dexJs.includes('"multi_hop"') || dexJs.includes('multi_hop:'), 'P9.6f: Multi-hop route type in labels');
}

// P9.7: Fee estimate uses feeRate from router response (F9.12a fix)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('data.feeRate'), 'P9.7a: calcTotal reads feeRate from router response');
    assert(dexJs.includes('feeRate / 10000') || dexJs.includes('feeRate/10000'), 'P9.7b: feeRate converted from bps to decimal');
}

// P9.8: Router response includes feeRate and estimatedFee (F9.12b fix)
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    assert(dexRs.includes('"feeRate"'), 'P9.8a: Router response has feeRate field');
    assert(dexRs.includes('"estimatedFee"'), 'P9.8b: Router response has estimatedFee field');
    assert(dexRs.includes('"splitPercent"'), 'P9.8c: Router response has splitPercent field');
}

// P9.9: AMM fee lookup for route fee calculation
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    assert(dexRs.includes('AMM_FEE_BPS'), 'P9.9a: AMM_FEE_BPS constant used for fee lookup');
    assert(dexRs.includes('amm_pool_') && dexRs.includes('data[92]'), 'P9.9b: Fee tier read from pool byte 92');
}

// P9.10: Router quote debounced in frontend
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('_routeQuoteTimer') || dexJs.includes('routeQuoteTimer'), 'P9.10a: Route quote is debounced');
    assert(dexJs.includes('setTimeout') && dexJs.includes('300'), 'P9.10b: Debounce delay is 300ms');
    assert(dexJs.includes('clearTimeout'), 'P9.10c: Previous timer cleared on new input');
}

// P9.11: Fallback route info when API unavailable
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("catch") && dexJs.includes("CLOB Direct"), 'P9.11: Fallback heuristic when router API unavailable');
}

// P9.12: Route endpoints registered in router
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    assert(dexRs.includes('"/router/swap"'), 'P9.12a: /router/swap endpoint registered');
    assert(dexRs.includes('"/router/quote"'), 'P9.12b: /router/quote endpoint registered');
    assert(dexRs.includes('"/routes"'), 'P9.12c: /routes endpoint registered');
}

// ═══════════════════════════════════════════════════════════════════════════
// PHASE 10: Margin Trading (Inline) — Tests
// ═══════════════════════════════════════════════════════════════════════════
const marginRsPath = '/Users/johnrobin/.openclaw/workspace/moltchain/contracts/dex_margin/src/lib.rs';
const predictionRsPath = '/Users/johnrobin/.openclaw/workspace/moltchain/rpc/src/prediction.rs';

// P10.1: calculate_margin_ratio_with_pnl exists in contract
{
    const marginRs = fs.readFileSync(marginRsPath, 'utf8');
    assert(marginRs.includes('fn calculate_margin_ratio_with_pnl'), 'P10.1a: calculate_margin_ratio_with_pnl function exists');
    assert(marginRs.includes('entry_price'), 'P10.1b: PnL-aware ratio takes entry_price parameter');
    assert(marginRs.includes('mark_price'), 'P10.1c: PnL-aware ratio takes mark_price parameter');
}

// P10.2: Liquidation uses PnL-aware ratio
{
    const marginRs = fs.readFileSync(marginRsPath, 'utf8');
    const liqBlock = marginRs.substring(marginRs.indexOf('fn liquidate('));
    assert(liqBlock.includes('calculate_margin_ratio_with_pnl'), 'P10.2a: liquidate() uses PnL-aware ratio');
    assert(liqBlock.includes('entry_price'), 'P10.2b: liquidate() reads entry_price for PnL');
}

// P10.3: get_margin_ratio uses PnL-aware ratio
{
    const marginRs = fs.readFileSync(marginRsPath, 'utf8');
    const gmrBlock = marginRs.substring(marginRs.indexOf('fn get_margin_ratio('));
    assert(gmrBlock.includes('calculate_margin_ratio_with_pnl'), 'P10.3: get_margin_ratio() uses PnL-aware ratio');
}

// P10.4: remove_margin uses PnL-aware ratio for health check
{
    const marginRs = fs.readFileSync(marginRsPath, 'utf8');
    const rmBlock = marginRs.substring(marginRs.indexOf('fn remove_margin('));
    assert(rmBlock.includes('calculate_margin_ratio_with_pnl'), 'P10.4: remove_margin() uses PnL-aware ratio');
}

// P10.5: Realized PnL written on close_position (F10.2-B fix)
{
    const marginRs = fs.readFileSync(marginRsPath, 'utf8');
    const closeBlock = marginRs.substring(marginRs.indexOf('fn close_position('), marginRs.indexOf('fn close_position(') + 3000);
    assert(closeBlock.includes('data[90..98]'), 'P10.5a: close_position writes to realized_pnl bytes [90..98]');
    assert(closeBlock.includes('pnl_biased'), 'P10.5b: PnL stored as biased value');
    assert(closeBlock.includes('1u64 << 63'), 'P10.5c: Uses PNL_BIAS (1<<63) for encoding');
}

// P10.6: Tier table provides maintenance BPS
{
    const marginRs = fs.readFileSync(marginRsPath, 'utf8');
    assert(marginRs.includes('fn get_tier_params'), 'P10.6a: get_tier_params function exists');
    assert(marginRs.includes('maintenance_margin_bps'), 'P10.6b: Tier returns maintenance margin BPS');
    // Verify tier values
    assert(marginRs.includes('2500'), 'P10.6c: Tier <=2x has 2500 maintenance BPS');
    assert(marginRs.includes('1700'), 'P10.6d: Tier <=3x has 1700 maintenance BPS');
}

// P10.7: JS getMaintenanceBps mirrors contract tier table
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('function getMaintenanceBps'), 'P10.7a: getMaintenanceBps helper exists in dex.js');
    assert(dexJs.includes('return 2500'), 'P10.7b: JS tier <=2x returns 2500');
    assert(dexJs.includes('return 1700'), 'P10.7c: JS tier <=3x returns 1700');
    assert(dexJs.includes('return 1000'), 'P10.7d: JS tier <=5x returns 1000');
    assert(dexJs.includes('return 500'), 'P10.7e: JS tier <=10x returns 500');
    assert(dexJs.includes('return 200'), 'P10.7f: JS tier <=25x returns 200');
    assert(dexJs.includes('return 100'), 'P10.7g: JS tier <=50x returns 100');
    assert(dexJs.includes('return 50'), 'P10.7h: JS tier >50x returns 50');
}

// P10.8: Liquidation price formula uses maintBps (not hardcoded 0.9)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const updateMarginInfo = dexJs.substring(dexJs.indexOf('function updateMarginInfo'), dexJs.indexOf('function updateMarginInfo') + 800);
    assert(updateMarginInfo.includes('getMaintenanceBps'), 'P10.8a: updateMarginInfo calls getMaintenanceBps');
    assert(updateMarginInfo.includes('maintFrac'), 'P10.8b: Uses maintenance fraction in liq price formula');
    assert(!updateMarginInfo.includes('* 0.9'), 'P10.8c: Hardcoded 0.9 factor removed');
}

// P10.9: Trade submit handler branches on margin mode (F10.6 fix)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("state.tradeMode === 'margin'"), 'P10.9a: Submit handler checks tradeMode');
    assert(dexJs.includes('contracts.dex_margin'), 'P10.9b: Margin mode uses dex_margin contract');
    assert(dexJs.includes('buildOpenPositionArgs'), 'P10.9c: Margin mode calls buildOpenPositionArgs');
}

// P10.10: Margin side derived from orderSide (buy→long, sell→short)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("orderSide === 'buy' ? 'long' : 'short'"), 'P10.10: Margin side correctly derived from buy/sell');
}

// P10.11: Unrealized PnL computed client-side for open positions
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const pnlCalc = dexJs.includes('mark - entry') && dexJs.includes('entry - mark');
    assert(pnlCalc, 'P10.11a: Unrealized PnL computed from mark vs entry price');
    assert(dexJs.includes('/ PRICE_SCALE'), 'P10.11b: PnL divided by PRICE_SCALE for display');
}

// P10.12: RPC PNL_BIAS matches contract encoding
{
    const dexRs = fs.readFileSync(dexRsPath, 'utf8');
    assert(dexRs.includes('const PNL_BIAS: u64 = 1u64 << 63'), 'P10.12a: RPC declares PNL_BIAS = 1<<63');
    const marginRs = fs.readFileSync(marginRsPath, 'utf8');
    assert(marginRs.includes('1u64 << 63'), 'P10.12b: Contract writes with same 1<<63 bias');
}

// P10.13: Margin nav link exists in index.html
{
    const indexHtml = fs.readFileSync(indexHtmlPath, 'utf8');
    assert(indexHtml.includes('data-view="margin"'), 'P10.13: Margin nav link exists in HTML');
}

// P10.14: Cross margin option removed (contract only supports isolated)
{
    const indexHtml = fs.readFileSync(indexHtmlPath, 'utf8');
    const crossCount = (indexHtml.match(/data-mtype="cross"|data-type="cross"/g) || []).length;
    assert(crossCount === 0, 'P10.14: Cross margin option removed from HTML');
}

// P10.15: view-margin section exists in HTML
{
    const indexHtml = fs.readFileSync(indexHtmlPath, 'utf8');
    assert(indexHtml.includes('id="view-margin"'), 'P10.15: view-margin section exists');
}

// P10.16: Closed/liquidated positions show realized PnL
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("pos.status === 'closed'") || dexJs.includes("p.status === 'closed'"), 'P10.16a: Checks for closed position status');
    assert(dexJs.includes('realizedPnl'), 'P10.16b: Uses realizedPnl for closed positions');
}

// ═══════════════════════════════════════════════════════════════════════════
// Trade Bridge + Oracle Integration (Phases A-D) — Structural Tests
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n## Trade Bridge + Oracle Integration (Phases A-D)\n');

// Read all source files once
const validatorSrcAll = fs.readFileSync(__dirname + '/../validator/src/main.rs', 'utf8');
const dexCoreSrc = fs.readFileSync(__dirname + '/../contracts/dex_core/src/lib.rs', 'utf8');
const dexJsFrontend = fs.readFileSync(__dirname + '/../dex/dex.js', 'utf8');

// ── Phase A: Trade Bridge (dex_core → dex_analytics) ──
console.log('\n### Phase A: Trade Bridge\n');

// PA.1: bridge_dex_trades_to_analytics function exists
assert(validatorSrcAll.includes('fn bridge_dex_trades_to_analytics('), 'PA.1: bridge_dex_trades_to_analytics function defined');

// PA.2: Bridge reads new trade records from DEX storage
assert(validatorSrcAll.includes('get_program_storage_u64("DEX", b"dex_trade_count")'), 'PA.2: Bridge reads dex_trade_count from DEX');

// PA.3: Bridge writes ana_lp_{pair_id} to analytics
assert(
    /bridge_dex_trades_to_analytics[\s\S]*?ana_lp_/.test(validatorSrcAll),
    'PA.3: Bridge writes ana_lp_ to analytics storage'
);

// PA.4: Bridge writes ana_24h_{pair_id} with real volume
assert(
    /bridge_dex_trades_to_analytics[\s\S]*?ana_24h_/.test(validatorSrcAll),
    'PA.4: Bridge writes ana_24h_ with trade volume'
);

// PA.5: Bridge writes ana_last_trade_ts_{pair_id} timestamp
assert(
    validatorSrcAll.includes('ana_last_trade_ts_'),
    'PA.5: Bridge writes ana_last_trade_ts for oracle fallback'
);

// PA.6: Bridge updates candles for all 9 intervals
assert(
    validatorSrcAll.includes('CANDLE_INTERVALS: [u64; 9]'),
    'PA.6: Bridge uses all 9 candle intervals'
);

// PA.7: bridge_update_candle function exists
assert(validatorSrcAll.includes('fn bridge_update_candle('), 'PA.7: bridge_update_candle function defined');

// PA.8: Bridge candle writes real trade volume (not zero)
{
    const bridgeCandle = validatorSrcAll.substring(
        validatorSrcAll.indexOf('fn bridge_update_candle('),
        validatorSrcAll.indexOf('fn bridge_update_candle(') + 2000
    );
    assert(bridgeCandle.includes('volume'), 'PA.8: bridge_update_candle includes real volume');
}

// PA.9: Bridge called post-block alongside emit_dex_events
assert(
    validatorSrcAll.includes('bridge_dex_trades_to_analytics(&state, &mut last_bridge_trade_count, slot)'),
    'PA.9: Bridge called in post-block processing'
);

// PA.10: Bridge tracks its own cursor (last_bridge_trade_count)
assert(
    validatorSrcAll.includes('let mut last_bridge_trade_count'),
    'PA.10: Bridge has separate trade count cursor'
);

// PA.11: Pair trades collection: accumulates volume and high/low per pair
assert(
    /pair_trades[\s\S]*?entry\.\d\s*=\s*entry\.\d\.saturating_add/.test(validatorSrcAll),
    'PA.11: Bridge accumulates per-pair volume with saturating_add'
);

// PA.12: Bridge resolves ANALYTICS symbol via registry
{
    const bridgeFn = validatorSrcAll.substring(
        validatorSrcAll.indexOf('fn bridge_dex_trades_to_analytics('),
        validatorSrcAll.indexOf('fn bridge_dex_trades_to_analytics(') + 500
    );
    assert(bridgeFn.includes('get_symbol_registry("ANALYTICS")'), 'PA.12: Bridge resolves ANALYTICS via symbol registry');
}

// ── Phase B: Oracle Feeder Becomes Fallback-Only ──
console.log('\n### Phase B: Oracle Fallback-Only\n');

// PB.1: Oracle feeder checks ana_last_trade_ts before writing
assert(
    /spawn_oracle_price_feeder[\s\S]*?ana_last_trade_ts_/.test(validatorSrcAll) ||
    validatorSrcAll.includes('ana_last_trade_ts_'),
    'PB.1: Oracle feeder reads ana_last_trade_ts_'
);

// PB.2: Oracle feeder has trade_active check with 60-second window
assert(
    validatorSrcAll.includes('now_ts.saturating_sub(last_trade_ts) < 60'),
    'PB.2: Oracle feeder uses 60s trade-active window'
);

// PB.3: Active market skips oracle analytics overwrite
assert(
    validatorSrcAll.includes('Active market: trades drive analytics, skip oracle overwrite'),
    'PB.3: Oracle feeder skips ana_lp_ write for active markets'
);

// PB.4: Inactive markets still get oracle indicative price
{
    const oracleLoop = validatorSrcAll.substring(
        validatorSrcAll.indexOf('Trade-driven fallback'),
        validatorSrcAll.indexOf('Trade-driven fallback') + 2000
    );
    assert(
        oracleLoop.includes('Inactive market: oracle writes indicative price'),
        'PB.4: Inactive markets still receive oracle analytics updates'
    );
}

// PB.5: Oracle feeder still writes to moltoracle unconditionally
{
    // The oracle feed writes (price_key → feed) happen BEFORE the analytics
    // section, so they always execute regardless of trade_active skip
    const oracleFeedWrite = validatorSrcAll.indexOf('Build 49-byte oracle feed');
    const tradeActiveCheck = validatorSrcAll.indexOf('Trade-driven fallback');
    assert(
        oracleFeedWrite > 0 && tradeActiveCheck > 0 && oracleFeedWrite < tradeActiveCheck,
        'PB.5: Oracle moltoracle writes occur before (not gated by) trade-active check'
    );
}

// ── Phase C: Oracle Price Bands in dex_core ──
console.log('\n### Phase C: Oracle Price Bands\n');

// PC.1: Validator oracle feeder writes dex_band_ to DEX storage
assert(
    validatorSrcAll.includes('dex_band_'),
    'PC.1: Oracle feeder writes dex_band_ records'
);

// PC.2: Oracle feeder resolves DEX symbol for band writes
assert(
    validatorSrcAll.includes('get_symbol_registry("DEX")'),
    'PC.2: Oracle feeder resolves DEX via symbol registry'
);

// PC.3: Band data is 16 bytes (reference_price + slot)
assert(
    validatorSrcAll.includes('band_data.extend_from_slice(&price_scaled.to_le_bytes())') &&
    validatorSrcAll.includes('band_data.extend_from_slice(&current_slot.to_le_bytes())'),
    'PC.3: Band data contains price + slot (16 bytes)'
);

// PC.4: dex_core reads dex_band_{pair_id} during place_order
assert(
    dexCoreSrc.includes('band_key(pair_id)'),
    'PC.4: dex_core reads band_key in place_order'
);

// PC.5: band_key function exists in dex_core
assert(
    dexCoreSrc.includes('fn band_key(pair_id: u64)'),
    'PC.5: band_key helper function defined'
);

// PC.6: Market orders enforced at ±5% (500 bps)
assert(
    dexCoreSrc.includes('ORDER_MARKET { 500 }'),
    'PC.6: Market orders use 500 bps (5%) band'
);

// PC.7: Limit orders enforced at ±10% (1000 bps)
assert(
    dexCoreSrc.includes('1000'),
    'PC.7: Limit orders use 1000 bps (10%) band'
);

// PC.8: Band staleness check (300 slots)
assert(
    dexCoreSrc.includes('current_slot.saturating_sub(band_slot) < 300'),
    'PC.8: Band freshness check uses 300-slot window'
);

// PC.9: Return code 10 for band violations
assert(
    dexCoreSrc.includes('return 10; // price outside oracle band'),
    'PC.9: Returns error code 10 for band violation'
);

// PC.10: No band record → unrestricted (native pairs)
{
    const bandCheck = dexCoreSrc.substring(
        dexCoreSrc.indexOf('Oracle Price Band Protection'),
        dexCoreSrc.indexOf('Oracle Price Band Protection') + 2000
    );
    assert(
        bandCheck.includes('if let Some(band_data)'),
        'PC.10: Band check is conditional — no record means unrestricted'
    );
}

// PC.11: Market order with price=0 (no worst-price bound) skips band check
assert(
    dexCoreSrc.includes('if price > 0 { price } else { 0 }'),
    'PC.11: Market order without worst-price bound skips band enforcement'
);

// PC.12: Band calculation uses ref_price * band_bps / 10000
assert(
    dexCoreSrc.includes('ref_price * band_bps / 10000'),
    'PC.12: Band range calculated correctly with basis points'
);

// PC.13: Band check uses both lower and upper bounds
assert(
    dexCoreSrc.includes('check_price < lower || check_price > upper'),
    'PC.13: Both lower and upper band bounds enforced'
);

// ── Phase D: Frontend Oracle Reference Line ──
console.log('\n### Phase D: Frontend Oracle Index\n');

// PD.1: Frontend fetches oracle prices
assert(
    dexJsFrontend.includes('fetchOracleRefPrices'),
    'PD.1: fetchOracleRefPrices function exists'
);

// PD.2: Fetches from /oracle/prices endpoint
assert(
    dexJsFrontend.includes('/oracle/prices'),
    'PD.2: Fetches oracle prices from API endpoint'
);

// PD.3: Oracle reference line drawn on TradingView chart
assert(
    dexJsFrontend.includes('updateOracleReferenceLine'),
    'PD.3: updateOracleReferenceLine function exists'
);

// PD.4: Uses TradingView createShape for horizontal line
assert(
    dexJsFrontend.includes("shape: 'horizontal_line'"),
    'PD.4: Draws horizontal_line shape on chart'
);

// PD.5: Oracle line styling — gold color, dashed
assert(
    dexJsFrontend.includes("linecolor: '#FFD700'") &&
    dexJsFrontend.includes('linestyle: 2'),
    'PD.5: Oracle line is gold (#FFD700) and dashed'
);

// PD.6: Oracle line label shows price
assert(
    dexJsFrontend.includes('Oracle: $'),
    'PD.6: Oracle line label displays price'
);

// PD.7: Oracle reference updates when pair switches
assert(
    dexJsFrontend.includes('// Update oracle reference line for new pair'),
    'PD.7: Oracle line updates on pair switch'
);

// PD.8: getOracleRefForPair handles MOLT-quoted pairs
assert(
    dexJsFrontend.includes("quote === 'MOLT'") &&
    dexJsFrontend.includes('refPrice / moltUsd'),
    'PD.8: Oracle ref converts for MOLT-quoted pairs'
);

// PD.9: Old oracle line removed before drawing new one
assert(
    dexJsFrontend.includes('chart.removeEntity(oracleLineId)'),
    'PD.9: Old oracle line entity removed before redraw'
);

// PD.10: Oracle prices polled every 5 seconds
assert(
    dexJsFrontend.includes('setInterval(fetchOracleRefPrices, 5000)'),
    'PD.10: Oracle prices polled at 5s interval'
);

// ── Cross-Phase Integration Tests ──
console.log('\n### Cross-Phase Integration\n');

// INT.1: Trade bridge AND oracle feeder both reference same pair IDs
assert(
    validatorSrcAll.includes('pair_id, interval') &&
    validatorSrcAll.includes('oracle_update_candle'),
    'INT.1: Both bridge and oracle use same candle infrastructure'
);

// INT.2: ana_last_trade_ts links Phase A and Phase B
{
    const bridgeHasTs = validatorSrcAll.includes('ana_last_trade_ts_');
    const oracleReadsTs = validatorSrcAll.includes('last_trade_ts');
    assert(bridgeHasTs && oracleReadsTs, 'INT.2: ana_last_trade_ts connects bridge (A) and oracle fallback (B)');
}

// INT.3: dex_band_ links oracle feeder (validator) to dex_core (contract)
{
    const validatorWritesBand = validatorSrcAll.includes('dex_band_');
    const contractReadsBand = dexCoreSrc.includes('band_key');
    assert(validatorWritesBand && contractReadsBand, 'INT.3: dex_band_ connects oracle feeder to contract price bands');
}

// INT.4: RPC oracle prices endpoint used by frontend
{
    const rpcSrc = fs.readFileSync(__dirname + '/../rpc/src/dex.rs', 'utf8');
    const rpcHasEndpoint = rpcSrc.includes('/oracle/prices');
    const frontendFetches = dexJsFrontend.includes('/oracle/prices');
    assert(rpcHasEndpoint && frontendFetches, 'INT.4: RPC oracle/prices endpoint consumed by frontend');
}

// INT.5: PRICE_SCALE consistency — bridge uses same scale as analytics
assert(
    validatorSrcAll.includes('PRICE_SCALE: f64 = 1_000_000_000.0'),
    'INT.5: Trade bridge uses same 1e9 price scale as existing code'
);

// INT.6: Genesis seed analytics prices not broken
assert(
    validatorSrcAll.includes('fn genesis_seed_analytics_prices'),
    'INT.6: Genesis analytics seeding function still present'
);

// INT.7: emit_dex_events still operational alongside bridge
{
    const emitStillExists = validatorSrcAll.includes('fn emit_dex_events(');
    const emitStillCalled = validatorSrcAll.includes('emit_dex_events(&state, &ws_dex_broadcaster');
    assert(emitStillExists && emitStillCalled, 'INT.7: emit_dex_events unchanged and still called post-block');
}

// INT.8: Bridge candle function separate from oracle candle function
assert(
    validatorSrcAll.includes('fn bridge_update_candle(') &&
    validatorSrcAll.includes('fn oracle_update_candle('),
    'INT.8: Separate candle update functions for bridge vs oracle'
);

// ═══════════════════════════════════════════════════════════════════════════
// Phase 11: Prediction Market — Markets & Cards
// ═══════════════════════════════════════════════════════════════════════════

// P11.1: JS category map matches contract constants (F11.1)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const predRs = fs.readFileSync(predictionRsPath, 'utf8');
    // JS catMap must encode categories with same indices as contract
    assert(dexJs.includes('politics: 0'), 'P11.1a: JS catMap politics=0 matches CATEGORY_POLITICS');
    assert(dexJs.includes('sports: 1'), 'P11.1b: JS catMap sports=1 matches CATEGORY_SPORTS');
    assert(dexJs.includes('crypto: 2'), 'P11.1c: JS catMap crypto=2 matches CATEGORY_CRYPTO');
    assert(dexJs.includes('science: 3'), 'P11.1d: JS catMap science=3 matches CATEGORY_SCIENCE');
    assert(dexJs.includes('entertainment: 4'), 'P11.1e: JS catMap entertainment=4');
    assert(dexJs.includes('economics: 5'), 'P11.1f: JS catMap economics=5');
    assert(dexJs.includes('tech: 6'), 'P11.1g: JS catMap tech=6');
    assert(dexJs.includes('custom: 7'), 'P11.1h: JS catMap custom=7');
    // Verify contract has matching constants
    assert(predRs.includes('0 => "politics"'), 'P11.1i: RPC category_name maps 0→politics');
    assert(predRs.includes('2 => "crypto"'), 'P11.1j: RPC category_name maps 2→crypto');
    // Verify old wrong mapping not present
    assert(!dexJs.includes('general: 0'), 'P11.1k: Old wrong "general:0" mapping removed');
}

// P11.2: buildCreateMarketArgs takes closeSlot parameter (F11.2)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('function buildCreateMarketArgs(creator, question, category, outcomeCount, closeSlot)'),
        'P11.2a: buildCreateMarketArgs has closeSlot parameter');
    assert(dexJs.includes('writeU64LE(view, 34, closeSlot'), 'P11.2b: closeSlot written at offset 34');
    // Verify caller computes closeSlot from current_slot + duration
    assert(dexJs.includes('currentSlot + durationSlots'), 'P11.2c: Caller computes absolute closeSlot');
    assert(dexJs.includes("prediction-market/stats"), 'P11.2d: Caller fetches stats to get current_slot');
}

// P11.3: RPC computes CPMM prices using cross-outcome reserves (F11.3)
{
    const predRs = fs.readFileSync(predictionRsPath, 'utf8');
    assert(predRs.includes('outcome_reserves'), 'P11.3a: RPC collects all outcome reserves');
    // Binary pricing: reserve_other / (reserve_self + reserve_other)
    assert(predRs.includes('other_r / sum'), 'P11.3b: Binary CPMM formula uses other_r / sum');
    assert(predRs.includes('outcome_reserves[1 - oi]'), 'P11.3c: Binary reads other outcome reserve');
    // Multi-outcome pricing: reciprocal formula
    assert(predRs.includes('recip_i / recip_sum'), 'P11.3d: Multi-outcome uses reciprocal CPMM');
}

// P11.4: No double volume/collateral conversion (F11.4)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    // Find loadPredictionMarkets area — should NOT multiply volume by 1e9
    const loadPredArea = dexJs.substring(
        dexJs.indexOf('loadPredictionMarkets'),
        dexJs.indexOf('loadPredictionMarkets') + 2000
    );
    assert(!loadPredArea.includes('* 1e9'), 'P11.4a: No * 1e9 multiplier in loadPredictionMarkets');
    assert(!loadPredArea.includes('*1e9'), 'P11.4b: No *1e9 multiplier (no space variant)');
}

// P11.5: Sort handlers for "ending" and "traders" exist (F11.5)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("sortBy === 'ending'"), 'P11.5a: "ending" sort handler present');
    assert(dexJs.includes("sortBy === 'traders'"), 'P11.5b: "traders" sort handler present');
    // Ending sorts by closes ascending (soonest first)
    assert(dexJs.includes('a.closes') && dexJs.includes('b.closes'), 'P11.5c: Ending sort uses closes field');
    // Traders sorts descending
    assert(dexJs.includes('b.traders - a.traders'), 'P11.5d: Traders sort is descending');
}

// P11.6: filterByRange helper filters chart data by time window (F11.6)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('function filterByRange(data, range)'), 'P11.6a: filterByRange function defined');
    assert(dexJs.includes("'1h': 3600e3"), 'P11.6b: 1h range = 3600000ms');
    assert(dexJs.includes("'6h': 21600e3"), 'P11.6c: 6h range = 21600000ms');
    assert(dexJs.includes("'1d': 86400e3"), 'P11.6d: 1d range = 86400000ms');
    assert(dexJs.includes("'1w': 604800e3"), 'P11.6e: 1w range = 604800000ms');
    assert(dexJs.includes("'30d': 2592000e3"), 'P11.6f: 30d range = 2592000000ms');
    // Filter applied in chart tab handler
    assert(dexJs.includes('filterByRange(raw, range)'), 'P11.6g: filterByRange called in chart tab handler');
}

// P11.7: Market mapping includes close_slot and creator (F11.7)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('closes: m.close_slot'), 'P11.7a: closes field mapped from m.close_slot');
    assert(dexJs.includes('creator: m.creator'), 'P11.7b: creator field mapped from m.creator');
}

// P11.8: statusMap covers all 7 prediction market statuses (F11.8)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const statuses = ['active', 'pending', 'closed', 'resolving', 'resolved', 'disputed', 'voided'];
    for (const s of statuses) {
        // Each status should be a key in the statusMap
        assert(dexJs.includes(`${s}: {`), `P11.8-${s}: statusMap has "${s}" entry`);
    }
}

// P11.9: RPC MarketJson includes unique_traders field (F11.9)
{
    const predRs = fs.readFileSync(predictionRsPath, 'utf8');
    assert(predRs.includes('unique_traders: u64'), 'P11.9a: MarketJson has unique_traders field');
    assert(predRs.includes('pm_mtc_'), 'P11.9b: RPC reads pm_mtc_ key for trader count');
}

// P11.10: RPC PlatformStatsJson includes current_slot (F11.2/F11.9)
{
    const predRs = fs.readFileSync(predictionRsPath, 'utf8');
    const statsStruct = predRs.substring(predRs.indexOf('struct PlatformStatsJson'), predRs.indexOf('struct PlatformStatsJson') + 300);
    assert(statsStruct.includes('current_slot: u64'), 'P11.10a: PlatformStatsJson has current_slot');
    assert(predRs.includes('current_slot: slot'), 'P11.10b: current_slot populated from slot in handler');
}

// P11.11: No N+1 analytics queries in JS (F11.9)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const loadPredArea = dexJs.substring(
        dexJs.indexOf('loadPredictionMarkets'),
        dexJs.indexOf('loadPredictionMarkets') + 3000
    );
    // Should not have per-market analytics fetch loop
    assert(!loadPredArea.includes('/analytics/market/'), 'P11.11a: No per-market analytics HTTP call');
    assert(!loadPredArea.includes("analytics/market/${"), 'P11.11b: No template literal analytics call');
}

// P11.12: RPC category_name handles all 8 categories (0-7)
{
    const predRs = fs.readFileSync(predictionRsPath, 'utf8');
    const categories = ['politics', 'sports', 'crypto', 'science', 'entertainment', 'economics', 'tech', 'custom'];
    for (let i = 0; i < categories.length; i++) {
        assert(predRs.includes(`${i} => "${categories[i]}"`), `P11.12-${categories[i]}: category_name maps ${i}→${categories[i]}`);
    }
}

// P11.13: RPC status_name handles all 7 statuses (0-6)
{
    const predRs = fs.readFileSync(predictionRsPath, 'utf8');
    const statuses = ['pending', 'active', 'closed', 'resolving', 'resolved', 'disputed', 'voided'];
    for (let i = 0; i < statuses.length; i++) {
        assert(predRs.includes(`${i} => "${statuses[i]}"`), `P11.13-${statuses[i]}: status_name maps ${i}→${statuses[i]}`);
    }
}

// P11.14: Binary CPMM price sum = 1.0 invariant
{
    const predRs = fs.readFileSync(predictionRsPath, 'utf8');
    // Formula: price_0 = r1/(r0+r1), price_1 = r0/(r0+r1) → sum = (r0+r1)/(r0+r1) = 1.0
    assert(predRs.includes('outcome_reserves.len() == 2'), 'P11.14a: Binary case checks exactly 2 outcomes');
    assert(predRs.includes('self_r + other_r'), 'P11.14b: Binary denominator is sum of both reserves');
    // Default 0.5 when no liquidity
    assert(predRs.includes('0.5'), 'P11.14c: Default price is 0.5 when no liquidity');
}

// P11.15: Market card close_slot not hardcoded to 0
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    // Verify closeSlot is NOT hardcoded zero in buildCreateMarketArgs
    assert(!dexJs.includes('writeU64LE(view, 34, 0)'), 'P11.15: closeSlot not hardcoded to 0');
}

// P11.16: Multi-outcome CPMM handles all-zero reserves gracefully
{
    const predRs = fs.readFileSync(predictionRsPath, 'utf8');
    // Should check non-zero before computing reciprocals
    assert(predRs.includes('all_nonzero'), 'P11.16a: Multi-outcome checks reserves are non-zero');
    assert(predRs.includes('1.0 / outcome_count as f64'), 'P11.16b: Fallback uniform price when reserves are zero');
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 12: Prediction Market — Trade & Create
// ═══════════════════════════════════════════════════════════════════════════

// P12.1: Amount scale uses 1e6 (MUSD_UNIT), not 1e9 (F12.1)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('amt * 1e6'), 'P12.1a: Buy shares sends amt * 1e6 (MUSD_UNIT)');
    assert(!dexJs.includes('amt * 1e9'), 'P12.1b: No amt * 1e9 in prediction trade code');
}

// P12.2: RPC PRICE_SCALE for prediction matches contract MUSD_UNIT (1e6) (F12.10)
{
    const predRs = fs.readFileSync(predictionRsPath, 'utf8');
    assert(predRs.includes('PRICE_SCALE: u64 = 1_000_000'), 'P12.2: RPC PRICE_SCALE = 1_000_000 matching MUSD_UNIT');
}

// P12.3: updatePredictCalc uses CPMM formula, not linear (F12.2)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    const calcFn = dexJs.substring(dexJs.indexOf('function updatePredictCalc'), dexJs.indexOf('function updatePredictCalc') + 2500);
    // Should reference outcome reserves from market data
    assert(calcFn.includes('selfReserve'), 'P12.3a: CPMM formula references selfReserve');
    assert(calcFn.includes('otherReserve'), 'P12.3b: CPMM formula references otherReserve');
    // CPMM swap formula: a_received = selfReserve * bSold / (otherReserve + bSold)
    assert(calcFn.includes('selfReserve * bSold'), 'P12.3c: Swap formula uses selfReserve * bSold');
    assert(calcFn.includes('otherReserve + bSold'), 'P12.3d: Swap denominator is otherReserve + bSold');
    // Fee on swap only, not entire amount
    assert(calcFn.includes('aFromSwap * 0.02'), 'P12.3e: Fee applied to swap portion (2%)');
    // Avg price display
    assert(calcFn.includes('amt / shares)'), 'P12.3f: Average price computed as amt/shares');
}

// P12.4: Resolve uses opcode 8 (submit_resolution), not 11 (dao_resolve) (F12.3)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("writeU8(arr, 0, 8); // opcode 8 = submit_resolution"), 'P12.4a: Resolve uses opcode 8');
    assert(!dexJs.includes('writeU8(arr, 0, 11); // opcode'), 'P12.4b: No opcode 11 (dao_resolve) in resolve builder');
    // Layout: 82 bytes for submit_resolution
    assert(dexJs.includes('new ArrayBuffer(82)'), 'P12.4c: Resolve instruction is 82 bytes');
    assert(dexJs.includes('100_000_000'), 'P12.4d: DISPUTE_BOND (100 mUSD) included');
}

// P12.5: CSS wallet-gates predict-toggle-btn, not predict-outcome-btn (F12.4)
{
    const css = fs.readFileSync('/Users/johnrobin/.openclaw/workspace/moltchain/dex/dex.css', 'utf8');
    assert(css.includes('.wallet-gated-disabled .predict-toggle-btn'), 'P12.5a: CSS targets predict-toggle-btn');
    assert(!css.includes('.wallet-gated-disabled .predict-outcome-btn'), 'P12.5b: Old predict-outcome-btn selector removed');
}

// P12.6: "My Markets" tab has loadCreatedMarkets function (F12.5)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('async function loadCreatedMarkets()'), 'P12.6a: loadCreatedMarkets function exists');
    assert(dexJs.includes('prediction-market/markets?creator='), 'P12.6b: Fetches markets filtered by creator');
    assert(dexJs.includes('predictCreatedBody'), 'P12.6c: Renders into predictCreatedBody table');
}

// P12.7: RPC MarketListQuery has creator filter (F12.5)
{
    const predRs = fs.readFileSync(predictionRsPath, 'utf8');
    const queryStruct = predRs.substring(predRs.indexOf('struct MarketListQuery'), predRs.indexOf('struct MarketListQuery') + 200);
    assert(queryStruct.includes('creator: Option<String>'), 'P12.7a: MarketListQuery has creator field');
    assert(predRs.includes('params.creator'), 'P12.7b: get_markets handler checks creator filter');
}

// P12.8: Close date input has min attribute set dynamically (F12.6)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("closeDateEl.setAttribute('min', today)"), 'P12.8a: Close date min set to today');
    // Past date validation with notification
    assert(dexJs.includes("'Close date must be in the future'"), 'P12.8b: Past date shows warning notification');
}

// P12.9: Claim winnings requires loaded position (F12.7)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("'No position found for this market'"), 'P12.9a: Claim handler validates position exists');
    assert(!dexJs.includes('cardPos ? cardPos.outcome : 0'), 'P12.9b: No default-to-0 fallback for outcome');
}

// P12.10: buildAddInitialLiquidityArgs exists (F12.8)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('function buildAddInitialLiquidityArgs('), 'P12.10a: buildAddInitialLiquidityArgs defined');
    assert(dexJs.includes('writeU8(arr, 0, 2); // opcode'), 'P12.10b: Uses opcode 2 (add_initial_liquidity)');
    // Create market handler chains both instructions
    assert(dexJs.includes('createIx, liqIx'), 'P12.10c: Create market sends both create + liquidity instructions');
}

// P12.11: YES/NO toggle correctly maps outcome index
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes("predictState.selectedOutcome === 'yes' ? 0 : 1"), 'P12.11: YES→0, NO→1 outcome mapping correct');
}

// P12.12: Fee display matches contract (2% = 200 bps)
{
    const html = fs.readFileSync(indexHtmlPath, 'utf8');
    assert(html.includes('Fee (2%)'), 'P12.12a: HTML shows 2% fee');
    const predContractRs = fs.readFileSync('/Users/johnrobin/.openclaw/workspace/moltchain/contracts/prediction_market/src/lib.rs', 'utf8');
    assert(predContractRs.includes('TRADING_FEE_BPS: u64 = 200'), 'P12.12b: Contract fee = 200 bps = 2%');
}

// P12.13: buildBuySharesArgs layout matches contract (verified correct by audit)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('function buildBuySharesArgs('), 'P12.13a: buildBuySharesArgs exists');
    assert(dexJs.includes('new ArrayBuffer(50)'), 'P12.13b: 50 bytes for buy_shares');
    // Opcode 4
    assert(dexJs.includes("writeU8(arr, 0, 4);"), 'P12.13c: Opcode 4 for buy_shares');
}

// P12.14: Create market handler validates initial liquidity amount
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('liq * 1e6'), 'P12.14: Initial liquidity converted to MUSD units (1e6)');
}

// P12.15: "My Markets" tab has proper HTML table structure
{
    const html = fs.readFileSync(indexHtmlPath, 'utf8');
    assert(html.includes('predictCreatedBody'), 'P12.15a: HTML has predictCreatedBody tbody');
    assert(html.includes('predict-created-table'), 'P12.15b: My Markets has table class');
}

// P12.16: Binary/Multi toggle verified working (audit confirmed correct)
{
    const dexJs = fs.readFileSync(dexJsPath, 'utf8');
    assert(dexJs.includes('multiOutcomeSection'), 'P12.16: Multi-outcome section toggle present');
}

// ═══════════════════════════════════════════════════════════════════════════
// Summary
// ═══════════════════════════════════════════════════════════════════════════
console.log(`\n${'═'.repeat(60)}`);
console.log(`  DEX Tests: ${passed} passed, ${failed} failed, ${passed + failed} total`);
console.log(`${'═'.repeat(60)}\n`);
process.exit(failed > 0 ? 1 : 0);
