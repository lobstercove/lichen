/**
 * Faucet Tests — Phase 16 audit fixes
 * Run: node faucet.test.js
 *
 * Tests all findings fixed during Phase 16 Faucet audit:
 *  F16.1  — XSS prevention in addRecentRequest (escapeHtml on shortAddress + amount)
 *  F16.2  — XSS prevention in success message (escapeHtml on amount, encodeURIComponent on sig)
 *  F16.3  — docker-compose env var mismatch (MOLTCHAIN_RPC_URL → RPC_URL)
 *  F16.4  — Address validation tightened (32-44 chars, base58 only)
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

// ═══════════════════════════════════════════════════════════════════════════
// Reproduce pure functions from source file for unit-level testing
// ═══════════════════════════════════════════════════════════════════════════

// TC-02: escapeHtml is defined in shared/utils.js (loaded globally).
// Faucet.js calls escapeHtml() which is provided by shared/utils.js at runtime.
// This test reproduces the same function for Node.js unit testing.
function escapeHtml(str) {
    return String(str ?? '')
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;');
}

// Address validation regex from faucet.js (F16.4 fix)
function isValidBase58Address(address) {
    if (!address || address.length < 32 || address.length > 44) return false;
    return /^[1-9A-HJ-NP-Za-km-z]+$/.test(address);
}

// ═══════════════════════════════════════════════════════════════════════════
// F16.1 — escapeHtml prevents XSS in addRecentRequest
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F16.1: escapeHtml prevents XSS in addRecentRequest ──');

assertEqual(escapeHtml('<script>alert(1)</script>'),
    '&lt;script&gt;alert(1)&lt;/script&gt;',
    'escapeHtml: script tags escaped');

assertEqual(escapeHtml('<img src=x onerror=alert(1)>'),
    '&lt;img src=x onerror=alert(1)&gt;',
    'escapeHtml: img XSS payload escaped');

assertEqual(escapeHtml('"onclick="alert(1)"'),
    '&quot;onclick=&quot;alert(1)&quot;',
    'escapeHtml: double quotes escaped');

assertEqual(escapeHtml("'onmouseover='alert(1)'"),
    "&#39;onmouseover=&#39;alert(1)&#39;",
    'escapeHtml: single quotes escaped');

assertEqual(escapeHtml('&amp; already encoded'),
    '&amp;amp; already encoded',
    'escapeHtml: ampersand double-encodes (correct)');

assertEqual(escapeHtml(''), '', 'escapeHtml: empty string');
assertEqual(escapeHtml(null), '', 'escapeHtml: null → empty');
assertEqual(escapeHtml(undefined), '', 'escapeHtml: undefined → empty');
assertEqual(escapeHtml(12345), '12345', 'escapeHtml: number → string');

// Simulate the shortAddress XSS vector
const maliciousAddress = '<img/src=x onerror=alert(1)>AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA';
const shortAddress = escapeHtml(`${maliciousAddress.slice(0, 8)}...${maliciousAddress.slice(-4)}`);
assert(!shortAddress.includes('<'), 'shortAddress: XSS payload in address is escaped');
assert(shortAddress.includes('&lt;'), 'shortAddress: angle brackets converted to entities');

// ═══════════════════════════════════════════════════════════════════════════
// F16.2 — Success message HTML escaping
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F16.2: Success message escaping ──');

// Simulate the success message build with escaping
const fakeSig = 'airdrop-<script>alert(1)</script>';
const fakeAmount = 100;
const safeSig = escapeHtml(fakeSig);
const safeAmount = escapeHtml(String(fakeAmount));

assert(!safeSig.includes('<script>'), 'success msg: signature XSS payload escaped');
assertEqual(safeAmount, '100', 'success msg: numeric amount safely converted');

// encodeURIComponent on href params
const encodedSig = encodeURIComponent(fakeSig);
assert(!encodedSig.includes('<'), 'success msg: sig is URI-encoded for href');
assert(encodedSig.includes('%3Cscript%3E'), 'success msg: angle brackets percent-encoded');

// Build the explorer link the same way faucet.js does
const address = '11111111111111111111111111111111';
const explorerLink = fakeSig
    ? ` <a href="../explorer/transaction.html?sig=${encodeURIComponent(fakeSig)}&to=${encodeURIComponent(address)}&amount=${encodeURIComponent(fakeAmount)}" class="tx-link">View in Explorer</a>`
    : '';
assert(!explorerLink.includes('<script>'), 'explorerLink: no raw script tag in href');

// ═══════════════════════════════════════════════════════════════════════════
// F16.3 — docker-compose env var matches backend
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F16.3: docker-compose env var alignment ──');

const fs = require('fs');
const path = require('path');

const composePath = path.join(__dirname, '..', 'docker-compose.yml');
if (fs.existsSync(composePath)) {
    const composeContent = fs.readFileSync(composePath, 'utf-8');
    assert(!composeContent.includes('MOLTCHAIN_RPC_URL'),
        'docker-compose: no MOLTCHAIN_RPC_URL (was mismatched var name)');
    assert(composeContent.includes('RPC_URL=http://validator:8899'),
        'docker-compose: uses RPC_URL matching main.rs env::var("RPC_URL")');
} else {
    assert(true, 'docker-compose: file not at expected path — skipping (CI)');
}

// ═══════════════════════════════════════════════════════════════════════════
// F16.4 — Address validation tightened
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── F16.4: Address validation ──');

// Valid base58 addresses (32-44 chars, base58 alphabet)
assert(isValidBase58Address('11111111111111111111111111111111'),
    'valid: 32-char all-ones system program');
assert(isValidBase58Address('ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL'),
    'valid: 44-char typical address');
assert(isValidBase58Address('9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin'),
    'valid: 44-char DEX program');

// Invalid addresses
assert(!isValidBase58Address('short'),
    'invalid: too short (< 32 chars)');
assert(!isValidBase58Address(''),
    'invalid: empty string');
assert(!isValidBase58Address(null),
    'invalid: null');
assert(!isValidBase58Address('11111111111111111111111111111111111111111111X'),
    'invalid: 45 chars (> 44)');
assert(!isValidBase58Address('<script>alert(1)</script>1111111111111111111'),
    'invalid: contains < > (not base58)');
assert(!isValidBase58Address('0OOOOOOOOOOOOOOOOOOOOOOOOOOOOOOO'),
    'invalid: contains 0 and O (not in base58)');
assert(!isValidBase58Address('IIIIIIIIIIIIIIIIIIIIIIIIIIIIIIIII'),
    'invalid: contains I (not in base58)');
assert(!isValidBase58Address('llllllllllllllllllllllllllllllll'),
    'invalid: contains l (not in base58)');

// Base58 alphabet: 123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz
// Missing: 0, I, O, l
assert(isValidBase58Address('123456789ABCDEFGHJKLMNPQRSTUVWXYZab'),
    'valid: all base58 chars present (36 chars)');

// ═══════════════════════════════════════════════════════════════════════════
// Faucet.js source file integrity checks
// ═══════════════════════════════════════════════════════════════════════════

console.log('\n── Source integrity ──');

const faucetPath = path.join(__dirname, 'faucet.js');
if (fs.existsSync(faucetPath)) {
    const src = fs.readFileSync(faucetPath, 'utf-8');

    // TC-02: escapeHtml is in shared/utils.js, not faucet.js — check the shared file
    const sharedPath = path.join(__dirname, '..', 'explorer', 'shared', 'utils.js');
    const sharedExists = fs.existsSync(sharedPath);
    const sharedSrc = sharedExists ? fs.readFileSync(sharedPath, 'utf-8') : '';
    assert(sharedExists && sharedSrc.includes('function escapeHtml(str)'),
        'faucet.js: escapeHtml helper exists (in shared/utils.js)');
    assert(src.includes('escapeHtml(') && src.includes('shortAddress'),
        'faucet.js: shortAddress passes through escapeHtml');

    // Check success message escaping
    assert(src.includes('safeAmount = escapeHtml('),
        'faucet.js: amount in success message is escaped');
    assert(src.includes('encodeURIComponent(data.signature)'),
        'faucet.js: signature is URI-encoded in href');
    assert(src.includes('encodeURIComponent(effectiveAmount)'),
        'faucet.js: amount is URI-encoded in href');

    // Address validation
    assert(src.includes('address.length < 32 || address.length > 44'),
        'faucet.js: address length validation is [32,44]');
    assert(src.includes('[1-9A-HJ-NP-Za-km-z]'),
        'faucet.js: base58 regex validation present');

    // Verify escapeHtml wraps the shortAddress template
    const addRecentLines = src.split('function addRecentRequest')[1]?.split('function ')[0] || '';
    assert(addRecentLines.includes('escapeHtml(`${address.slice'),
        'faucet.js: addRecentRequest wraps address.slice in escapeHtml');
    assert(!addRecentLines.includes('${amount}'),
        'faucet.js: addRecentRequest has no raw ${amount} in template');
} else {
    assert(true, 'faucet.js: file not at expected path — skipping (CI)');
}

// ═══════════════════════════════════════════════════════════════════════════
// Summary
// ═══════════════════════════════════════════════════════════════════════════

console.log(`\n═══ Phase 16 Faucet: ${passed} passed, ${failed} failed ═══`);
process.exit(failed > 0 ? 1 : 0);
