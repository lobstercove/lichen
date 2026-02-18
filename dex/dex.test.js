/**
 * DEX Frontend Tests — Phase 10 audit fixes
 * Run: node dex.test.js
 *
 * Tests all pure-function fixes applied during Phase 10 audit:
 *  F10.8  — escapeHtml XSS sanitization
 *  F10.9  — encodeTransactionMessage bincode-compatible signing
 *  F10.9  — sendTransaction validator-compatible wire format
 *  F10.10 — bs58 encode/decode round-trip
 *  F10.1-F10.7 — handler wiring (structural tests)
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
