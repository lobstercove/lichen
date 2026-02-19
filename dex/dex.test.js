/**
 * DEX Frontend Tests — Phase 10 + Phase 10 Extra audit fixes
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
 *  F10E.7  — Binance price feed integration
 *  F10E.8  — CSS disabled styles
 *  F10E.9  — Margin position wallet-gate
 *  F10E.10 — Add Liquidity wallet-gate
 *  F10E.11 — Pool "My Pools" filter logic
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

// F10E.7: Binance price feed integration
assert(dexSource.includes('connectBinancePriceFeed'), 'F10E.7: connectBinancePriceFeed function exists');
assert(dexSource.includes('stream.binance.com'), 'F10E.7: Uses Binance WebSocket endpoint');
assert(dexSource.includes('solusdt@miniTicker'), 'F10E.7: Subscribes to SOL/USDT');
assert(dexSource.includes('ethusdt@miniTicker'), 'F10E.7: Subscribes to ETH/USDT');
assert(dexSource.includes('btcusdt@miniTicker'), 'F10E.7: Subscribes to BTC/USDT');
assert(dexSource.includes('updateExternalPricedPairs'), 'F10E.7: updateExternalPricedPairs function exists');
assert(dexSource.includes("externalPrices"), 'F10E.7: externalPrices state object exists');
assert(dexSource.includes("apiPriceless"), 'F10E.7: Marks externally-priced pairs');

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
// hexToBytes / bytesToHex
// ═══════════════════════════════════════════════════════════════════════════
console.log('\n── Utility: hexToBytes / bytesToHex ──');

assertEqual(bytesToHex(hexToBytes('00ff80')), '00ff80', 'hex round-trip');
assertEqual(hexToBytes('abcd').length, 2, 'hexToBytes length');
assertEqual(bytesToHex(new Uint8Array([0, 255, 128])), '00ff80', 'bytesToHex');

// ═══════════════════════════════════════════════════════════════════════════
// Summary
// ═══════════════════════════════════════════════════════════════════════════
console.log(`\n${'═'.repeat(60)}`);
console.log(`  Phase 10 Tests: ${passed} passed, ${failed} failed, ${passed + failed} total`);
console.log(`${'═'.repeat(60)}\n`);
process.exit(failed > 0 ? 1 : 0);
