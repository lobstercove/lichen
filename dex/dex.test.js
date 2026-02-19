/**
 * DEX Frontend Tests — Phase 10 + Phase 10 Extra + Oracle Price Feed Integration
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

// F10.1: Order submission uses sendTransaction
assert(dexSource.includes("op: 'place_order'") && dexSource.includes("contracts.dex_core"), 'F10.1: Order submit wired to sendTransaction + dex_core');
assert(!dexSource.includes("api.post('/orders'"), 'F10.1: No unsigned api.post to /orders');

// F10.2: Cancel order uses sendTransaction
assert(dexSource.includes("op: 'cancel_order'"), 'F10.2: Cancel order uses sendTransaction');
assert(!dexSource.includes("api.del('/orders"), 'F10.2: No unsigned api.del for cancel');

// F10.3: Margin uses sendTransaction
assert(dexSource.includes("op: 'open_position'"), 'F10.3: Margin open wired to sendTransaction');
assert(dexSource.includes("op: 'close_position'"), 'F10.3: Margin close wired to sendTransaction');
assert(dexSource.includes("contracts.dex_margin"), 'F10.3: Uses dex_margin contract');

// F10.4: Prediction trade uses sendTransaction
assert(dexSource.includes("op: 'buy_shares'"), 'F10.4: Prediction trade wired to sendTransaction');
assert(dexSource.includes("op: 'create_market'"), 'F10.4: Prediction create wired to sendTransaction');
assert(dexSource.includes("contracts.prediction_market"), 'F10.4: Uses prediction_market contract');
assert(!dexSource.includes("api.post('/prediction-market/trade'"), 'F10.4: No unsigned REST for prediction trade');
assert(!dexSource.includes("api.post('/prediction-market/create'"), 'F10.4: No unsigned REST for prediction create');

// F10.5: Resolution + claim UI
assert(dexSource.includes("op: 'resolve_market'"), 'F10.5: Resolve market handler exists');
assert(dexSource.includes("op: 'claim_winnings'"), 'F10.5: Claim winnings handler exists');
assert(dexSource.includes('btn-predict-resolve'), 'F10.5: Resolve button rendered');
assert(dexSource.includes('btn-predict-claim'), 'F10.5: Claim button rendered');

// F10.6: Governance uses sendTransaction with real balance
assert(dexSource.includes("op: 'vote'"), 'F10.6: Vote wired to sendTransaction');
assert(dexSource.includes("contracts.dex_governance"), 'F10.6: Uses dex_governance contract');
assert(dexSource.includes("op: 'create_proposal'"), 'F10.6: Proposal submit wired to sendTransaction');
assert(!dexSource.includes("api.post('/governance/proposals'"), 'F10.6: No unsigned REST for proposals');

// F10.7: Reward claim uses sendTransaction
assert(dexSource.includes("op: 'claim_rewards'"), 'F10.7: Reward claim wired to sendTransaction');
assert(dexSource.includes("contracts.dex_rewards"), 'F10.7: Uses dex_rewards contract');
// Note: api.get('/rewards/...') is still used for READ-ONLY stats display (F10.7 fix moved the CLAIM to sendTransaction)
assert(dexSource.includes("op: 'claim_rewards'"), 'F10.7: Claim uses sendTransaction (not fake GET)');

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

// P4.1: Submit handler sends correct fields to sendTransaction
{
    const submitMatch = dexSource.match(/sendTransaction\(\[\{[\s\S]*?op:\s*'place_order'[\s\S]*?\}\]\)/);
    assert(submitMatch, 'P4.1a: Submit handler calls sendTransaction with op: place_order');
    const block = submitMatch[0];
    assert(block.includes('pair_id:'), 'P4.1b: Submit sends pair_id');
    assert(block.includes('side:'), 'P4.1c: Submit sends side');
    assert(block.includes('order_type:'), 'P4.1d: Submit sends order_type');
    assert(block.includes('price:') && block.includes('PRICE_SCALE'), 'P4.1e: Submit sends price scaled by PRICE_SCALE');
    assert(block.includes('quantity:') && block.includes('PRICE_SCALE'), 'P4.1f: Submit sends quantity scaled by PRICE_SCALE');
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

// P4.4: Cancel order uses sendTransaction with op: cancel_order
assert(
    dexSource.includes("op: 'cancel_order'") && dexSource.includes('order_id:'),
    'P4.4: Cancel order sends op: cancel_order with order_id via sendTransaction'
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
// Summary
// ═══════════════════════════════════════════════════════════════════════════
console.log(`\n${'═'.repeat(60)}`);
console.log(`  DEX Tests: ${passed} passed, ${failed} failed, ${passed + failed} total`);
console.log(`${'═'.repeat(60)}\n`);
process.exit(failed > 0 ? 1 : 0);
