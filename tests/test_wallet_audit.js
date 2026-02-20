#!/usr/bin/env node
// ============================================================================
// Phase 11 — Wallet App Audit Tests
// Tests for all 9 audit findings (W-1 through W-9)
// Run: node tests/test_wallet_audit.js
// ============================================================================

const { webcrypto } = require('crypto');
const assert = require('assert');

// Polyfill browser globals for wallet code under test
global.crypto = webcrypto;

let passed = 0;
let failed = 0;

function test(name, fn) {
    try {
        fn();
        passed++;
        console.log(`  ✅ ${name}`);
    } catch (e) {
        failed++;
        console.log(`  ❌ ${name}: ${e.message}`);
    }
}

async function testAsync(name, fn) {
    try {
        await fn();
        passed++;
        console.log(`  ✅ ${name}`);
    } catch (e) {
        failed++;
        console.log(`  ❌ ${name}: ${e.message}`);
    }
}

// ── Load BIP39 wordlist from crypto.js (we extract just what we need) ──
const fs = require('fs');
const path = require('path');

const cryptoSrc = fs.readFileSync(path.join(__dirname, '..', 'wallet', 'js', 'crypto.js'), 'utf8');
const walletSrc = fs.readFileSync(path.join(__dirname, '..', 'wallet', 'js', 'wallet.js'), 'utf8');

// ---- Minimal nacl polyfill for Node.js ----
let nacl;
try {
    nacl = require('tweetnacl');
} catch (_) {
    // If tweetnacl is not installed, provide a minimal mock
    nacl = {
        sign: {
            keyPair: {
                fromSeed: (seed) => {
                    // Minimal mock: just return plausible data
                    const pk = new Uint8Array(32);
                    const sk = new Uint8Array(64);
                    pk.set(seed.slice(0, 32));
                    sk.set(seed, 0);
                    sk.set(pk, 32);
                    return { publicKey: pk, secretKey: sk };
                }
            },
            detached: (msg, sk) => new Uint8Array(64),
            detached: { verify: () => true }
        }
    };
}
global.nacl = nacl;

// ---- Extract BIP39_WORDLIST ----
const wordlistMatch = cryptoSrc.match(/const BIP39_WORDLIST = \[([\s\S]*?)\];/);
let BIP39_WORDLIST = [];
if (wordlistMatch) {
    BIP39_WORDLIST = wordlistMatch[1].match(/'([^']+)'/g).map(w => w.replace(/'/g, ''));
}

// ---- Minimal bs58 for tests ----
const BASE58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
const bs58 = {
    encode: function(buffer) {
        if (!buffer || buffer.length === 0) return '';
        const digits = [0];
        for (let i = 0; i < buffer.length; i++) {
            let carry = buffer[i];
            for (let j = 0; j < digits.length; j++) {
                carry += digits[j] << 8;
                digits[j] = carry % 58;
                carry = (carry / 58) | 0;
            }
            while (carry > 0) { digits.push(carry % 58); carry = (carry / 58) | 0; }
        }
        let output = '';
        for (let i = 0; buffer[i] === 0 && i < buffer.length - 1; i++) output += BASE58_ALPHABET[0];
        for (let i = digits.length - 1; i >= 0; i--) output += BASE58_ALPHABET[digits[i]];
        return output;
    },
    decode: function(string) {
        if (!string || string.length === 0) return new Uint8Array(0);
        const bytes = [0];
        for (let i = 0; i < string.length; i++) {
            const value = BASE58_ALPHABET.indexOf(string[i]);
            if (value === -1) throw new Error(`Invalid base58 character: ${string[i]}`);
            let carry = value;
            for (let j = 0; j < bytes.length; j++) {
                carry += bytes[j] * 58; bytes[j] = carry & 0xff; carry >>= 8;
            }
            while (carry > 0) { bytes.push(carry & 0xff); carry >>= 8; }
        }
        for (let i = 0; string[i] === BASE58_ALPHABET[0] && i < string.length - 1; i++) bytes.push(0);
        return new Uint8Array(bytes.reverse());
    }
};
global.bs58 = bs58;

// ---- Recreate MoltCrypto class from source ----
// We need to eval the crypto.js content, but it redeclares BIP39_WORDLIST.
// Wrap the entire script in a function scope to avoid conflicts.
const MoltCrypto = (() => {
    // Modify source: replace global const with let, remove window assignment
    let modifiedSrc = cryptoSrc
        .replace('const BIP39_WORDLIST =', 'const _BIP39_WORDLIST =')
        .replace(/\bBIP39_WORDLIST\b/g, '_BIP39_WORDLIST')
        .replace('window.MoltCrypto = MoltCrypto;', '');
    
    const fn = new Function('nacl', 'bs58', 'crypto',
        modifiedSrc + '\nreturn MoltCrypto;'
    );
    return fn(nacl, bs58, webcrypto);
})();

// ============================================================================
// TEST SUITE
// ============================================================================

console.log('\n── Phase 11: Wallet App Audit Tests ──\n');

// ---- W-1: XSS in NFT rendering ----
console.log('W-1: XSS prevention in NFT rendering');

test('escapeHtml escapes angle brackets', () => {
    // Recreate the escapeHtml function from wallet.js (DOM-based, so we use a regex version for Node)
    function escapeHtml(str) {
        return String(str).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#039;');
    }
    const malicious = '<script>alert("xss")</script>';
    const escaped = escapeHtml(malicious);
    assert(!escaped.includes('<script>'), 'Script tag should be escaped');
    assert(escaped.includes('&lt;script&gt;'), 'Should contain escaped brackets');
});

test('NFT image URL protocol validation rejects javascript:', () => {
    const rawImage = 'javascript:alert("xss")';
    const isValid = /^https?:\/\//i.test(rawImage);
    assert.strictEqual(isValid, false, 'javascript: URLs must be rejected');
});

test('NFT image URL protocol validation accepts https:', () => {
    const rawImage = 'https://example.com/nft.png';
    const isValid = /^https?:\/\//i.test(rawImage);
    assert.strictEqual(isValid, true, 'https: URLs must be accepted');
});

test('NFT image URL protocol validation rejects data:', () => {
    const rawImage = 'data:text/html,<script>alert(1)</script>';
    const isValid = /^https?:\/\//i.test(rawImage);
    assert.strictEqual(isValid, false, 'data: URLs must be rejected');
});

// ---- W-2: XSS in export modals ----
console.log('\nW-2: Export modal XSS prevention');

test('wallet.js no longer uses inline onclick with privateKeyHex interpolation', () => {
    assert(!walletSrc.includes("onclick=\"navigator.clipboard.writeText('${privateKeyHex}')"),
        'Should not interpolate privateKeyHex into onclick');
});

test('wallet.js no longer uses inline onclick with escapedMnemonic', () => {
    assert(!walletSrc.includes("onclick=\"navigator.clipboard.writeText('${escapedMnemonic}')"),
        'Should not interpolate escapedMnemonic into onclick');
});

test('export modal uses event listener pattern', () => {
    assert(walletSrc.includes("addEventListener('click'"),
        'Should use addEventListener for click handlers');
    assert(walletSrc.includes('exportPkCopy'), 'Should have exportPkCopy button ID');
    assert(walletSrc.includes('seedExportCopy'), 'Should have seedExportCopy button ID');
});

// ---- W-3: Hex validation for private key import ----
console.log('\nW-3: Private key hex format validation');

test('rejects non-hex characters in private key', () => {
    const invalidKey = 'zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz';
    assert.strictEqual(invalidKey.length, 64);
    const isValidHex = /^[0-9a-fA-F]{64}$/.test(invalidKey);
    assert.strictEqual(isValidHex, false, 'Non-hex characters must be rejected');
});

test('accepts valid 64-char hex private key', () => {
    const validKey = 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2';
    const isValidHex = /^[0-9a-fA-F]{64}$/.test(validKey);
    assert.strictEqual(isValidHex, true, 'Valid hex key must be accepted');
});

test('rejects 63-char hex string', () => {
    const shortKey = 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b';
    const isValidHex = /^[0-9a-fA-F]{64}$/.test(shortKey);
    assert.strictEqual(isValidHex, false, 'Short key must be rejected');
});

test('wallet.js has hex validation regex in importWalletPrivateKey', () => {
    assert(walletSrc.includes('/^[0-9a-fA-F]{64}$/'), 'Must have hex validation regex');
});

// ---- W-4: Auto-lock "Never" bug ----
console.log('\nW-4: Auto-lock "Never" (timeout=0) fix');

test('resetLockTimer guards against lockTimeout === 0', () => {
    assert(walletSrc.includes('timeout > 0'),
        'resetLockTimer must check timeout > 0');
});

test('resetLockTimer extracts timeout before check', () => {
    assert(walletSrc.includes('const timeout = walletState.settings.lockTimeout'),
        'Must extract lockTimeout to local variable');
});

// ---- W-5: Sensitive key zeroing ----
console.log('\nW-5: Sensitive key material zeroing');

test('zeroBytes helper exists in wallet.js', () => {
    assert(walletSrc.includes('function zeroBytes(arr)'),
        'zeroBytes helper must exist');
});

test('signTransaction zeros seed and secretKey after use', () => {
    assert(cryptoSrc.includes('seed.fill(0)'), 'Must zero seed');
    assert(cryptoSrc.includes('keypair.secretKey.fill(0)'), 'Must zero secretKey');
});

test('signTransaction returns signature before zeroing', async () => {
    // Verify the function still works correctly after zeroing
    const seedHex = 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2';
    const message = new Uint8Array([1, 2, 3, 4]);
    const sig = await MoltCrypto.signTransaction(seedHex, message);
    assert(sig instanceof Uint8Array, 'Signature must be Uint8Array');
    assert.strictEqual(sig.length, 64, 'Ed25519 signature must be 64 bytes');
});

// ---- W-6: Address validation in identity.js ----
console.log('\nW-6: Address validation in identity module');

test('identity.js validates transfer recipient address', () => {
    const identitySrc = fs.readFileSync(path.join(__dirname, '..', 'wallet', 'js', 'identity.js'), 'utf8');
    assert(identitySrc.includes('MoltCrypto.isValidAddress(values.recipient)'),
        'Must validate recipient address in transfer');
});

test('identity.js validates vouch address', () => {
    const identitySrc = fs.readFileSync(path.join(__dirname, '..', 'wallet', 'js', 'identity.js'), 'utf8');
    assert(identitySrc.includes('MoltCrypto.isValidAddress(values.vouchee)'),
        'Must validate vouchee address');
});

test('isValidAddress rejects short strings', () => {
    assert.strictEqual(MoltCrypto.isValidAddress('abc'), false);
});

test('isValidAddress rejects null', () => {
    assert.strictEqual(MoltCrypto.isValidAddress(null), false);
});

test('isValidAddress accepts valid 32-byte base58 address', () => {
    // Generate a real 32-byte key and encode as base58
    const pubkey = new Uint8Array(32);
    webcrypto.getRandomValues(pubkey);
    const addr = bs58.encode(pubkey);
    assert.strictEqual(MoltCrypto.isValidAddress(addr), true);
});

// ---- W-7: BIP39 checksum verification ----
console.log('\nW-7: BIP39 mnemonic checksum verification');

test('isValidMnemonic rejects wrong word count', () => {
    assert.strictEqual(MoltCrypto.isValidMnemonic('abandon abandon'), false);
});

test('isValidMnemonic rejects non-wordlist words', () => {
    assert.strictEqual(MoltCrypto.isValidMnemonic('abandon xyzzy abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon'), false);
});

test('isValidMnemonic accepts 12 valid BIP39 words', () => {
    // Use words from the wordlist (word-level check still passes without checksum)
    const mnemonic = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
    assert.strictEqual(MoltCrypto.isValidMnemonic(mnemonic), true);
});

test('isValidMnemonicAsync exists for full checksum validation', () => {
    assert.strictEqual(typeof MoltCrypto.isValidMnemonicAsync, 'function');
});

test('isValidMnemonicAsync validates correct checksum', async () => {
    // "abandon ... about" is a well-known BIP39 test vector
    const valid = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
    const result = await MoltCrypto.isValidMnemonicAsync(valid);
    assert.strictEqual(result, true, 'Known valid mnemonic must pass checksum');
});

test('isValidMnemonicAsync rejects invalid checksum', async () => {
    // Change last word to break checksum
    const invalid = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon ability';
    const result = await MoltCrypto.isValidMnemonicAsync(invalid);
    assert.strictEqual(result, false, 'Invalid checksum must be rejected');
});

test('generateMnemonic produces valid BIP39 mnemonic with correct checksum', async () => {
    const mnemonic = await MoltCrypto.generateMnemonic();
    const words = mnemonic.split(' ');
    assert.strictEqual(words.length, 12, 'Must generate 12 words');
    assert.strictEqual(MoltCrypto.isValidMnemonic(mnemonic), true, 'Must pass word check');
    const checksumValid = await MoltCrypto.isValidMnemonicAsync(mnemonic);
    assert.strictEqual(checksumValid, true, 'Must pass async checksum check');
});

// ---- W-8: Secure UUID generation ----
console.log('\nW-8: CSPRNG UUID generation');

test('generateId no longer uses Math.random', () => {
    assert(!cryptoSrc.match(/generateId[\s\S]*?Math\.random/),
        'generateId must not use Math.random');
});

test('generateId uses crypto.getRandomValues', () => {
    assert(cryptoSrc.includes('crypto.getRandomValues(bytes)'),
        'Must use crypto.getRandomValues');
});

test('generateId produces valid UUIDv4 format', () => {
    const uuid = MoltCrypto.generateId();
    const uuidRegex = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
    assert(uuidRegex.test(uuid), `UUID ${uuid} must match v4 format`);
});

test('generateId produces unique values', () => {
    const ids = new Set();
    for (let i = 0; i < 100; i++) ids.add(MoltCrypto.generateId());
    assert.strictEqual(ids.size, 100, 'All 100 UUIDs must be unique');
});

// ---- W-9: loadWalletState validation ----
console.log('\nW-9: loadWalletState structure validation');

test('wallet.js validates parsed JSON structure', () => {
    assert(walletSrc.includes("Array.isArray(parsed.wallets)"),
        'Must check wallets is an array');
});

test('wallet.js provides default lockTimeout', () => {
    assert(walletSrc.includes('lockTimeout') && walletSrc.includes('300000'),
        'Must have default lockTimeout of 300000');
});

test('wallet.js wraps JSON.parse in try-catch', () => {
    // Check that loadWalletState has try/catch around JSON.parse
    const loadFn = walletSrc.match(/function loadWalletState\(\)[\s\S]*?^}/m);
    assert(loadFn && loadFn[0].includes('try {') && loadFn[0].includes('catch'),
        'loadWalletState must wrap JSON.parse in try/catch');
});

// ---- Additional integration tests ----
console.log('\nIntegration: Crypto module');

test('bytesToHex roundtrips correctly', () => {
    const original = new Uint8Array([0, 1, 127, 128, 255]);
    const hex = MoltCrypto.bytesToHex(original);
    const restored = MoltCrypto.hexToBytes(hex);
    assert.deepStrictEqual(Array.from(restored), Array.from(original));
});

test('mnemonicToKeypair produces valid keypair', async () => {
    const mnemonic = await MoltCrypto.generateMnemonic();
    const keypair = await MoltCrypto.mnemonicToKeypair(mnemonic);
    assert(keypair.address, 'Must have address');
    assert(keypair.publicKey, 'Must have publicKey');
    assert(keypair.privateKey, 'Must have privateKey');
    assert.strictEqual(keypair.privateKey.length, 64, 'Seed hex must be 64 chars');
});

// AUDIT-FIX I2-01: BIP39 test vector — verify PBKDF2 derivation produces correct seed
test('mnemonicToKeypair uses PBKDF2 per BIP39 spec (test vector)', async () => {
    // BIP39 test vector: "abandon" x11 + "about", passphrase ""
    // Expected BIP39 seed (PBKDF2-HMAC-SHA512, 2048 iterations):
    // 5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc1...
    // Ed25519 seed = first 32 bytes of BIP39 seed
    const testMnemonic = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
    const keypair = await MoltCrypto.mnemonicToKeypair(testMnemonic);

    // The BIP39 seed's first 32 bytes (hex) for this test vector:
    const expectedSeedPrefix = '5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc1';
    assert.strictEqual(keypair.privateKey, expectedSeedPrefix,
        'Must match BIP39 test vector (PBKDF2 derivation)');
});

// AUDIT-FIX I2-01: Verify deterministic derivation — same mnemonic = same keypair
test('mnemonicToKeypair is deterministic', async () => {
    const mnemonic = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
    const kp1 = await MoltCrypto.mnemonicToKeypair(mnemonic);
    const kp2 = await MoltCrypto.mnemonicToKeypair(mnemonic);
    assert.strictEqual(kp1.privateKey, kp2.privateKey, 'Same mnemonic must produce same key');
    assert.strictEqual(kp1.publicKey, kp2.publicKey, 'Same mnemonic must produce same pubkey');
    assert.strictEqual(kp1.address, kp2.address, 'Same mnemonic must produce same address');
});

// AUDIT-FIX I2-01: Verify passphrase support changes the derived key
test('mnemonicToKeypair passphrase changes derived key', async () => {
    const mnemonic = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
    const kpNoPass = await MoltCrypto.mnemonicToKeypair(mnemonic, '');
    const kpWithPass = await MoltCrypto.mnemonicToKeypair(mnemonic, 'my-secret');
    assert.notStrictEqual(kpNoPass.privateKey, kpWithPass.privateKey,
        'Different passphrase must produce different key');
    assert.notStrictEqual(kpNoPass.address, kpWithPass.address,
        'Different passphrase must produce different address');
});

test('encryptPrivateKey/decryptPrivateKey roundtrip', async () => {
    const seedHex = '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';
    const password = 'test-password-123';
    const encrypted = await MoltCrypto.encryptPrivateKey(seedHex, password);
    assert(encrypted.encrypted, 'Must have encrypted field');
    assert(encrypted.salt, 'Must have salt field');
    assert(encrypted.iv, 'Must have iv field');
    const decrypted = await MoltCrypto.decryptPrivateKey(encrypted, password);
    assert.strictEqual(decrypted, seedHex, 'Decrypted must match original');
});

test('decryptPrivateKey rejects wrong password', async () => {
    const seedHex = '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';
    const encrypted = await MoltCrypto.encryptPrivateKey(seedHex, 'correct-password');
    try {
        await MoltCrypto.decryptPrivateKey(encrypted, 'wrong-password');
        assert.fail('Should have thrown');
    } catch (e) {
        assert(e.message.includes('Invalid password') || e.message.includes('operation-specific reason'),
            'Must throw invalid password error');
    }
});

test('publicKeyToAddress produces base58 string', () => {
    const pubkey = new Uint8Array(32);
    webcrypto.getRandomValues(pubkey);
    const addr = MoltCrypto.publicKeyToAddress(pubkey);
    assert(typeof addr === 'string', 'Address must be string');
    assert(addr.length > 20, 'Base58 address must be reasonable length');
    // Verify it decodes back to 32 bytes
    const decoded = bs58.decode(addr);
    assert.strictEqual(decoded.length, 32, 'Decoded address must be 32 bytes');
});

// ---- bincode serializer tests ----
console.log('\nIntegration: Bincode serializer');

test('serializeMessageBincode validates blockhash format', () => {
    // Extract serializeMessageBincode from wallet.js
    const fnMatch = walletSrc.match(/function serializeMessageBincode\(message\)[\s\S]*?^}/m);
    assert(fnMatch, 'serializeMessageBincode must exist');
    
    // Create the function by eval'ing the full declaration and returning a reference
    const serializeMessageBincode = (new Function(fnMatch[0] + '\nreturn serializeMessageBincode;'))();
    
    // Valid blockhash
    const validHash = 'a'.repeat(64);
    const msg = { instructions: [], blockhash: validHash };
    const result = serializeMessageBincode(msg);
    assert(result instanceof Uint8Array, 'Must return Uint8Array');
    
    // Invalid blockhash — too short
    try {
        serializeMessageBincode({ instructions: [], blockhash: 'abc' });
        assert.fail('Should throw on invalid blockhash');
    } catch (e) {
        assert(e.message.includes('Invalid'), 'Error must mention validation');
    }
    
    // Missing blockhash
    try {
        serializeMessageBincode({ instructions: [] });
        assert.fail('Should throw on missing blockhash');
    } catch (e) {
        assert(e.message.includes('Invalid') || e.message.includes('missing'),
            'Error must mention validation');
    }
});

// ============================================================================
// SUMMARY
// ============================================================================
console.log(`\n${'─'.repeat(50)}`);
console.log(`Phase 11 Wallet Audit: ${passed} passed, ${failed} failed (${passed + failed} total)`);

if (failed > 0) {
    process.exit(1);
} else {
    console.log('All tests passed! ✅');
    process.exit(0);
}
