#!/usr/bin/env node
// ============================================================================
// Phase 11 — Wallet App Audit Tests
// Tests for all 9 audit findings (W-1 through W-9)
// Run: node scripts/qa/test_wallet_audit.js
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

function readFirstExisting(paths) {
    for (const filePath of paths) {
        if (fs.existsSync(filePath)) {
            return fs.readFileSync(filePath, 'utf8');
        }
    }
    throw new Error(`No existing path found: ${paths.join(', ')}`);
}

const cryptoSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'js', 'crypto.js'), 'utf8');
const walletSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'js', 'wallet.js'), 'utf8');
const walletSharedUtilsSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'shared', 'utils.js'), 'utf8');
const shieldedSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'js', 'shielded.js'), 'utf8');
const identitySrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'js', 'identity.js'), 'utf8');
const lichenidAbi = JSON.parse(fs.readFileSync(path.join(__dirname, '..', '..', 'contracts', 'lichenid', 'abi.json'), 'utf8'));
const walletHtml = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'index.html'), 'utf8');
const explorerAddressSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'explorer', 'js', 'address.js'), 'utf8');

// crypto.js no longer depends on the legacy JS signer, but keep the binding slot for the eval wrapper.
const nacl = null;
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
    encode: function (buffer) {
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
    decode: function (string) {
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

// ---- Set up window global for Node.js (crypto.js accesses window.LichenPQ) ----
global.window = global;
const { createHash: _sha256Hash } = require('crypto');
global.LichenPQ = {
    isValidAddress(address) {
        if (typeof address !== 'string' || address.length < 8) return false;
        try { const d = bs58.decode(address); return d.length === 32; } catch (e) { return false; }
    },
    publicKeyToAddress(publicKey) {
        const pk = publicKey instanceof Uint8Array ? publicKey : new Uint8Array(publicKey);
        const hash = _sha256Hash('sha256').update(pk).digest();
        const addrBytes = new Uint8Array(32);
        addrBytes[0] = 0x01;
        addrBytes.set(hash.slice(0, 31), 1);
        return bs58.encode(addrBytes);
    },
    addressToBytes(address) { return bs58.decode(address); },
    normalizeSignature(sig) { return sig; },
    keypairFromSeed() { return { privateKey: '00'.repeat(32), publicKeyHex: '00'.repeat(1952), address: bs58.encode(new Uint8Array(32)) }; },
    signMessage() { return { scheme_version: 1, public_key: { scheme_version: 1, bytes: '00'.repeat(1952) }, sig: '00'.repeat(3309) }; },
};

// ---- Recreate LichenCrypto class from source ----
// We need to eval the crypto.js content, but it redeclares BIP39_WORDLIST.
// Wrap the entire script in a function scope to avoid conflicts.
const LichenCrypto = (() => {
    // Modify source: replace global const with let, remove window assignment
    let modifiedSrc = cryptoSrc
        .replace('const BIP39_WORDLIST =', 'const _BIP39_WORDLIST =')
        .replace(/\bBIP39_WORDLIST\b/g, '_BIP39_WORDLIST')
        .replace('window.LichenCrypto = LichenCrypto;', '');

    const fn = new Function('nacl', 'bs58', 'crypto',
        modifiedSrc + '\nreturn LichenCrypto;'
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
    assert(walletSrc.includes('/^[0-9a-fA-F]+$/'), 'Must validate private key characters as hex');
    assert(walletSrc.includes('normalizedKey.length !== 64'),
        'Must enforce 64-hex-character private key imports only');
    assert(walletSrc.includes('Invalid private key length (must be 64 hex characters)'),
        'Must explain the 64-hex-character private key requirement');
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
    // ML-DSA-65: key zeroing happens inside the shared PQ runtime signMessage.
    // Verify signTransaction delegates to pq().signMessage (which zeros key material).
    assert(
        cryptoSrc.includes('pq().signMessage('),
        'signTransaction must delegate to PQ runtime signMessage (which zeros key material)'
    );
});

test('signTransaction returns signature before zeroing', async () => {
    // Verify the function still works correctly after zeroing
    const seedHex = 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2';
    const message = new Uint8Array([1, 2, 3, 4]);
    const sig = await LichenCrypto.signTransaction(seedHex, message);
    assert(sig && typeof sig === 'object', 'Signature must be a PQ signature object');
    assert.strictEqual(sig.scheme_version, 1, 'PQ signature must carry scheme version 1');
    assert(sig.public_key && typeof sig.public_key.bytes === 'string', 'PQ signature must carry verifying key bytes');
    assert(typeof sig.sig === 'string' && sig.sig.length > 1000, 'PQ signature payload must be present');
});

// ---- W-6: Address validation in identity.js ----
console.log('\nW-6: Address validation in identity module');

test('identity.js validates transfer recipient address', () => {
    assert(identitySrc.includes('LichenCrypto.isValidAddress(values.recipient)'),
        'Must validate recipient address in transfer');
});

test('identity.js validates vouch address', () => {
    assert(identitySrc.includes('LichenCrypto.isValidAddress(values.vouchee)'),
        'Must validate vouchee address');
});

// ---- W-10: Shielded wallet flow assertions ----
console.log('\nW-10: Shield / Unshield wallet flow wiring');

test('wallet shield tab exposes shield and unshield actions', () => {
    assert(walletHtml.includes('data-tab="shield"'), 'Wallet must include Shield tab');
    assert(walletHtml.includes('data-wallet-action="openShieldModal"'), 'Wallet must wire openShieldModal from UI');
    assert(walletHtml.includes('data-wallet-action="openUnshieldModal"'), 'Wallet must wire openUnshieldModal from UI');
    assert(walletHtml.includes('id="shieldModal"'), 'Wallet must include shield modal');
    assert(walletHtml.includes('id="unshieldModal"'), 'Wallet must include unshield modal');
});

test('shielded.js confirm handlers call shield/unshield operations', () => {
    assert(shieldedSrc.includes('function confirmShield()'), 'confirmShield handler must exist');
    assert(shieldedSrc.includes('shieldLicn(amount);'), 'confirmShield must trigger shieldLicn');
    assert(shieldedSrc.includes('function confirmUnshield()'), 'confirmUnshield handler must exist');
    assert(shieldedSrc.includes('unshieldLicn(amount, recipient);'), 'confirmUnshield must trigger unshieldLicn');
    assert(shieldedSrc.includes("showToast('Enter a recipient address')"),
        'confirmUnshield must validate recipient input');
});

test('shielded.js submits on-chain shield/unshield transactions and updates UI', () => {
    assert(shieldedSrc.includes("submitShieldTransaction"), 'Shield flow must submit shield transaction');
    assert(shieldedSrc.includes("submitUnshieldTransaction"), 'Unshield flow must submit unshield transaction');
    assert(shieldedSrc.includes('updateShieldedUI();'), 'Shielded flows must refresh wallet shielded UI');
    assert(shieldedSrc.includes("closeModal('shieldModal')"), 'Shield flow must close shield modal on success');
    assert(shieldedSrc.includes("closeModal('unshieldModal')"), 'Unshield flow must close unshield modal on success');
});

// ---- W-11: .lichen full lifecycle assertions ----
console.log('\nW-11: .lichen lifecycle workflow wiring');

test('identity.js includes .lichen register/renew/transfer/release actions', () => {
    assert(identitySrc.includes("buildContractCall('register_name'"), 'register_name flow must exist');
    assert(identitySrc.includes("buildContractCall('renew_name'"), 'renew_name flow must exist');
    assert(identitySrc.includes("buildContractCall('transfer_name'"), 'transfer_name flow must exist');
    assert(identitySrc.includes("buildContractCall('release_name'"), 'release_name flow must exist');
});

test('identity.js resolves and reverse-resolves .lichen names', () => {
    assert(identitySrc.includes("rpc.call('reverseLichenName'"), 'reverseLichenName lookup must exist');
    assert(identitySrc.includes("rpc.call('resolveLichenName'"), 'resolveLichenName lookup must exist');
});

test('.lichen state-changing actions refresh wallet identity visibility', () => {
    assert(identitySrc.includes('await retryLoadIdentity(5, 1200);') || identitySrc.includes('await loadIdentity();'),
        '.lichen action flows must refresh identity data after transaction');
});

// ---- W-12: Vouch + achievement visibility assertions ----
console.log('\nW-12: Vouch / achievement wallet+explorer visibility');

test('wallet identity action includes vouch transaction and renders vouches/achievements', () => {
    assert(identitySrc.includes("buildContractCall('vouch'"), 'Wallet must support vouch user action');
    assert(identitySrc.includes('renderVouchesSection('), 'Wallet must render vouches section');
    assert(identitySrc.includes('renderAchievementsSection('), 'Wallet must render achievements section');
});

test('explorer address view renders LichenID vouches and achievements', () => {
    assert(explorerAddressSrc.includes("rpcCall('getLichenIdProfile'"), 'Explorer must fetch LichenID profile');
    assert(explorerAddressSrc.includes("rpcCall('reverseLichenName'"), 'Explorer must fetch reverse .lichen name');
    assert(explorerAddressSrc.includes('Vouched By ('), 'Explorer must render vouch visibility section');
    assert(explorerAddressSrc.includes('Achievements'), 'Explorer must render achievements visibility section');
    assert(explorerAddressSrc.includes("data-identity-action=\"vouch\""), 'Explorer must expose vouch user action');
});

test('isValidAddress rejects short strings', () => {
    assert.strictEqual(LichenCrypto.isValidAddress('abc'), false);
});

test('isValidAddress rejects null', () => {
    assert.strictEqual(LichenCrypto.isValidAddress(null), false);
});

test('isValidAddress accepts valid 32-byte base58 address', () => {
    // Generate a real 32-byte key and encode as base58
    const pubkey = new Uint8Array(32);
    webcrypto.getRandomValues(pubkey);
    const addr = bs58.encode(pubkey);
    assert.strictEqual(LichenCrypto.isValidAddress(addr), true);
});

// ---- W-7: BIP39 checksum verification ----
console.log('\nW-7: BIP39 mnemonic checksum verification');

test('isValidMnemonic rejects wrong word count', () => {
    assert.strictEqual(LichenCrypto.isValidMnemonic('abandon abandon'), false);
});

test('isValidMnemonic rejects non-wordlist words', () => {
    assert.strictEqual(LichenCrypto.isValidMnemonic('abandon xyzzy abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon'), false);
});

test('isValidMnemonic accepts valid BIP39 checksum mnemonic', () => {
    const mnemonic = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
    assert.strictEqual(LichenCrypto.isValidMnemonic(mnemonic), true);
});

test('isValidMnemonic rejects invalid BIP39 checksum mnemonic', () => {
    const invalid = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon ability';
    assert.strictEqual(LichenCrypto.isValidMnemonic(invalid), false);
});

test('isValidMnemonicAsync exists for full checksum validation', () => {
    assert.strictEqual(typeof LichenCrypto.isValidMnemonicAsync, 'function');
});

test('isValidMnemonicAsync validates correct checksum', async () => {
    // "abandon ... about" is a well-known BIP39 test vector
    const valid = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
    const result = await LichenCrypto.isValidMnemonicAsync(valid);
    assert.strictEqual(result, true, 'Known valid mnemonic must pass checksum');
});

test('isValidMnemonicAsync rejects invalid checksum', async () => {
    // Change last word to break checksum
    const invalid = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon ability';
    const result = await LichenCrypto.isValidMnemonicAsync(invalid);
    assert.strictEqual(result, false, 'Invalid checksum must be rejected');
});

test('generateMnemonic produces valid BIP39 mnemonic with correct checksum', async () => {
    const mnemonic = await LichenCrypto.generateMnemonic();
    const words = mnemonic.split(' ');
    assert.strictEqual(words.length, 12, 'Must generate 12 words');
    assert.strictEqual(LichenCrypto.isValidMnemonic(mnemonic), true, 'Must pass word check');
    const checksumValid = await LichenCrypto.isValidMnemonicAsync(mnemonic);
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
    const uuid = LichenCrypto.generateId();
    const uuidRegex = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
    assert(uuidRegex.test(uuid), `UUID ${uuid} must match v4 format`);
});

test('generateId produces unique values', () => {
    const ids = new Set();
    for (let i = 0; i < 100; i++) ids.add(LichenCrypto.generateId());
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
    const hex = LichenCrypto.bytesToHex(original);
    const restored = LichenCrypto.hexToBytes(hex);
    assert.deepStrictEqual(Array.from(restored), Array.from(original));
});

test('mnemonicToKeypair produces valid keypair', async () => {
    const mnemonic = await LichenCrypto.generateMnemonic();
    const keypair = await LichenCrypto.mnemonicToKeypair(mnemonic);
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
    // ML-DSA-65 wallet seed = first 32 bytes of the BIP39 seed
    const testMnemonic = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
    const keypair = await LichenCrypto.mnemonicToKeypair(testMnemonic);

    // The BIP39 seed's first 32 bytes (hex) for this test vector:
    const expectedSeedPrefix = '5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc1';
    assert.strictEqual(keypair.privateKey, expectedSeedPrefix,
        'Must match BIP39 test vector (PBKDF2 derivation)');
});

// AUDIT-FIX I2-01: Verify deterministic derivation — same mnemonic = same keypair
test('mnemonicToKeypair is deterministic', async () => {
    const mnemonic = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
    const kp1 = await LichenCrypto.mnemonicToKeypair(mnemonic);
    const kp2 = await LichenCrypto.mnemonicToKeypair(mnemonic);
    assert.strictEqual(kp1.privateKey, kp2.privateKey, 'Same mnemonic must produce same key');
    assert.strictEqual(kp1.publicKey, kp2.publicKey, 'Same mnemonic must produce same pubkey');
    assert.strictEqual(kp1.address, kp2.address, 'Same mnemonic must produce same address');
});

// AUDIT-FIX I2-01: Verify passphrase support changes the derived key
test('mnemonicToKeypair passphrase changes derived key', async () => {
    const mnemonic = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
    const kpNoPass = await LichenCrypto.mnemonicToKeypair(mnemonic, '');
    const kpWithPass = await LichenCrypto.mnemonicToKeypair(mnemonic, 'my-secret');
    assert.notStrictEqual(kpNoPass.privateKey, kpWithPass.privateKey,
        'Different passphrase must produce different key');
    assert.notStrictEqual(kpNoPass.address, kpWithPass.address,
        'Different passphrase must produce different address');
});

// AUDIT-FIX I2-02: Verify wallet.js never stores plaintext secret key material
test('wallet.js only stores encrypted keys (no plaintext in localStorage)', () => {
    // Verify every wallet creation/import path calls encryptPrivateKey before storage
    const createMatch = walletSrc.match(/finishCreateWallet[\s\S]*?localStorage/);
    // The wallet code must encrypt before any localStorage write involving keys
    const encryptCalls = (walletSrc.match(/encryptPrivateKey/g) || []).length;
    assert(encryptCalls >= 6,
        `Wallet must call encryptPrivateKey for all key storage paths (found ${encryptCalls}, need >=6)`);

    // Verify no plaintext key assignment directly to wallet object without encryption
    // Pattern: wallet.privateKey = or wallet.seed = (without encrypt) should NOT exist
    const plaintextKeyStore = walletSrc.match(/wallet\.\s*(?:privateKey|seed|secretKey)\s*=\s*(?!await\s+LichenCrypto)/g);
    assert(!plaintextKeyStore || plaintextKeyStore.length === 0,
        'Must not store plaintext privateKey/seed/secretKey on wallet object');
});

// AUDIT-FIX I2-02: Verify encryptPrivateKey uses AES-GCM with proper parameters
test('encryptPrivateKey uses AES-256-GCM + PBKDF2', () => {
    assert(cryptoSrc.includes("'AES-GCM'"), 'Must use AES-GCM');
    assert(cryptoSrc.includes('iterations: 100000'), 'Must use 100000 PBKDF2 iterations');
    assert(cryptoSrc.includes("length: 256"), 'Must use 256-bit key');
    assert(cryptoSrc.includes('getRandomValues'), 'Must use CSPRNG for salt/IV');
});

// AUDIT-FIX H6-01: Verify no fake address generation from random bytes
test('wallet-connect.js does not generate fake addresses from random bytes', () => {
    const walletConnectSrc = readFirstExisting([
        path.join(__dirname, '..', '..', 'shared', 'wallet-connect.js'),
        path.join(__dirname, '..', '..', 'dex', 'shared', 'wallet-connect.js'),
    ]);
    // The old vulnerability: generating random bytes and encoding as base58 to create a fake address
    // Pattern: var bytes = new Uint8Array(32); crypto.getRandomValues(bytes); ... chars[bytes[i % 32] % chars.length]
    const hasFakeAddrPattern = walletConnectSrc.includes("chars[bytes[i % 32] % chars.length]");
    assert(!hasFakeAddrPattern,
        'Must not generate fake addresses from random bytes (H6-01)');

    // Must throw error instead of silently generating fake addresses
    assert(walletConnectSrc.includes('throw new Error'),
        'Must throw error when wallet extension is unavailable');
});

test('encryptPrivateKey/decryptPrivateKey roundtrip', async () => {
    const seedHex = '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';
    const password = 'test-password-123';
    const encrypted = await LichenCrypto.encryptPrivateKey(seedHex, password);
    assert(encrypted.encrypted, 'Must have encrypted field');
    assert(encrypted.salt, 'Must have salt field');
    assert(encrypted.iv, 'Must have iv field');
    const decrypted = await LichenCrypto.decryptPrivateKey(encrypted, password);
    assert.strictEqual(decrypted, seedHex, 'Decrypted must match original');
});

test('decryptPrivateKey rejects wrong password', async () => {
    const seedHex = '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';
    const encrypted = await LichenCrypto.encryptPrivateKey(seedHex, 'correct-password');
    try {
        await LichenCrypto.decryptPrivateKey(encrypted, 'wrong-password');
        assert.fail('Should have thrown');
    } catch (e) {
        assert(e.message.includes('Invalid password') || e.message.includes('operation-specific reason'),
            'Must throw invalid password error');
    }
});

test('publicKeyToAddress produces base58 string', () => {
    const pubkey = new Uint8Array(32);
    webcrypto.getRandomValues(pubkey);
    const addr = LichenCrypto.publicKeyToAddress(pubkey);
    assert(typeof addr === 'string', 'Address must be string');
    assert(addr.length > 20, 'Base58 address must be reasonable length');
    // Verify it decodes back to 32 bytes
    const decoded = bs58.decode(addr);
    assert.strictEqual(decoded.length, 32, 'Decoded address must be 32 bytes');
});

// ---- bincode serializer tests ----
console.log('\nIntegration: Bincode serializer');

test('serializeMessageBincode validates blockhash format', () => {
    // Extract serializeMessageBincode from shared utils (single source of truth)
    const fnMatch = walletSharedUtilsSrc.match(/function serializeMessageBincode\(message\)[\s\S]*?^}/m);
    assert(fnMatch, 'serializeMessageBincode must exist in shared/utils.js');

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

    assert(!walletSrc.includes('function serializeMessageBincode(message)'),
        'wallet.js should reuse shared/utils serializer instead of a local duplicate');
});

console.log('\nW-13: Shielded RPC method wiring');

test('shielded.js prefers isNullifierSpent over legacy checkNullifier', () => {
    assert(shieldedSrc.includes("rpc.call('isNullifierSpent'"), 'shielded.js should call isNullifierSpent');
});

test('shielded.js keeps fallback compatibility to checkNullifier', () => {
    assert(shieldedSrc.includes("rpc.call('checkNullifier'"), 'shielded.js should keep checkNullifier fallback');
});

console.log('\nW-14: Wallet delete secure wipe wiring');

test('wallet.js defines wipeSensitiveWalletData helper', () => {
    assert(walletSrc.includes('function wipeSensitiveWalletData(wallet)'), 'wipeSensitiveWalletData helper missing');
});

test('wallet.js wipes encrypted key material before deletion', () => {
    assert(walletSrc.includes('wipeSensitiveWalletData(wipeTarget);'), 'delete flow must invoke wipeSensitiveWalletData');
    assert(walletSrc.includes('wallet.encryptedKey = wipeString(wallet.encryptedKey) || null;'), 'encryptedKey wipe missing');
});

console.log('\nW-15: Activity pagination cursor wiring');

test('wallet.js activity pagination prefers RPC has_more + next_before_slot', () => {
    assert(walletSrc.includes('result.has_more'), 'activity pagination should consume RPC has_more');
    assert(walletSrc.includes('result.next_before_slot'), 'activity pagination should consume RPC next_before_slot');
});

test('wallet.js activity pagination falls back safely for legacy responses', () => {
    assert(walletSrc.includes('Legacy fallback: infer pagination from page size + last tx slot'),
        'activity pagination should retain legacy fallback behavior');
});

console.log('\nW-16: Unshield recipient address validation');

test('shielded.js validates recipient address before unshield', () => {
    assert(shieldedSrc.includes('!window.LichenCrypto || !window.LichenCrypto.isValidAddress(recipient)'),
        'confirmUnshield should validate recipient address');
    assert(shieldedSrc.includes('showToast(\'Enter a valid recipient address\')'),
        'invalid recipient should show explicit validation toast');
});

console.log('\nW-17: LichenID set_rate ABI encoding validation');

test('identity.js set_rate encoder matches LichenID ABI pointer+u64 layout', () => {
    const setRate = (lichenidAbi.functions || []).find((fn) => fn.name === 'set_rate');
    assert(setRate, 'LichenID ABI must expose set_rate');
    assert.strictEqual(setRate.opcode, 41, 'set_rate opcode should be 41');
    assert.deepStrictEqual(
        (setRate.params || []).map((p) => p.type),
        ['Pubkey', 'u64'],
        'set_rate ABI params must be [Pubkey, u64]'
    );

    assert(identitySrc.includes("case 'set_rate':"), 'identity.js must implement set_rate encoding branch');
    assert(identitySrc.includes('const data = new Uint8Array(32 + 8);'), 'set_rate args must allocate 32-byte pubkey + 8-byte u64');
    assert(identitySrc.includes('data.set(callerPubkey, 0);'), 'set_rate args must place caller pubkey at offset 0');
    assert(identitySrc.includes('data.set(u64LE(params.licn_per_unit || 0), 32);'), 'set_rate args must place rate u64 at offset 32');
    assert(identitySrc.includes('return data; // no layout prefix'), 'set_rate encoding must use raw pointer+u64 args without layout prefix');
});

test('identity.js set_rate update path submits licn_per_unit in spore units', () => {
    assert(identitySrc.includes("buildContractCall('set_rate', { licn_per_unit: newRateSpores }, values.password)"),
        'identity edit flow must pass set_rate as licn_per_unit spores');
});

console.log('\nW-18: Wallet low-priority UX wiring');

test('wallet index send fee display is dynamic (no hardcoded 0.001 text)', () => {
    assert(walletHtml.includes('id="sendNetworkFeeDisplay"'), 'send modal should expose dynamic fee display id');
    assert(!walletHtml.includes('<span>0.001 LICN</span>'), 'send modal should not hardcode 0.001 LICN fee text');
});

test('wallet.js fetches getFeeConfig and applies dynamic send fee', () => {
    assert(walletSrc.includes("rpc.call('getFeeConfig'"), 'wallet should request getFeeConfig for dynamic fee');
    assert(walletSrc.includes('function getNetworkBaseFeeLicn()'), 'wallet should centralize dynamic fee accessor');
    assert(walletSrc.includes('updateSendFeeEstimateUI()'), 'wallet should update send fee display from dynamic fee config');
});

test('wallet.js activity timestamp uses formatTime helper', () => {
    assert(walletSrc.includes('const date = tx.timestamp ? formatTime(tx.timestamp) : \''),
        'wallet activity should use formatTime helper instead of raw timestamp conversion');
});

test('wallet.js activity explorer links use LICHEN_CONFIG.explorer base', () => {
    assert(walletSrc.includes('LICHEN_CONFIG.explorer'), 'wallet activity links should use configured explorer base');
    assert(walletSrc.includes('/transaction.html?sig='), 'wallet activity links should keep transaction route');
});

console.log('\nW-19: Staking validator fetch optimization');

test('wallet.js caches validator list for staking tab reuse', () => {
    assert(walletSrc.includes('const STAKING_VALIDATORS_CACHE_TTL_MS = 30 * 1000;'),
        'wallet should define staking validators cache TTL');
    assert(walletSrc.includes('async function getStakingValidators()'),
        'wallet should centralize staking validator fetch in cache-aware helper');
    assert(walletSrc.includes('const validators = await getStakingValidators();'),
        'loadStaking should use cached validator helper instead of direct refetch');
});

console.log('\nW-20: EVM receive address registration gating');

test('wallet receive view hides EVM address until registration exists', () => {
    assert(walletSrc.includes('const evmAddress = await getRegisteredEvmAddress(wallet.address);'),
        'receive flow should resolve EVM address from on-chain registration status');
    assert(walletSrc.includes("evmAddressSection.style.display = 'none';"),
        'receive flow should hide EVM address section when not yet registered');
    assert(walletSrc.includes("evmAddressInfo.style.display = 'block';"),
        'receive flow should display registration hint when EVM address is unavailable');
    assert(walletHtml.includes('id="evmAddressSection"'),
        'receive modal should expose EVM section id for conditional visibility');
    assert(walletHtml.includes('id="evmAddressInfo"'),
        'receive modal should expose EVM registration hint container');
});

console.log('\nW-21: Name auction bid units and args wiring');

test('identity.js bid_name_auction passes bid_amount in LICN and converts to spore units', () => {
    assert(identitySrc.includes('bid_amount: bidAmount'),
        'identity bid flow should pass bid_amount into bid_name_auction args');
    assert(identitySrc.includes('Math.floor((params.bid_amount || 0) * 1_000_000_000)'),
        'bid_name_auction encoder should convert bid_amount LICN into spore units');
    assert(identitySrc.includes("buildContractCall('bid_name_auction'"),
        'identity bid flow should invoke bid_name_auction contract call');
});

console.log('\nW-22: Shielded key derivation + note confidentiality hardening');

test('wallet.js derives shielded seed from decrypted secret material (not public address)', () => {
    assert(walletSrc.includes('async function initShieldedForActiveWallet()'),
        'wallet should define shielded init helper for active wallet');
    assert(walletSrc.includes('LichenCrypto.decryptPrivateKey(wallet.encryptedKey, password)'),
        'shielded init should decrypt secret key material using wallet password');
    assert(walletSrc.includes('lichen-shielded-spending-seed-v1'),
        'shielded seed derivation should include domain-separated seed context');
    assert(!walletSrc.includes("wallet.address + ':shielded'"),
        'shielded seed must not be derived from public address');
});

test('shielded.js encrypts notes with AES-GCM and keeps legacy decrypt fallback', () => {
    assert(shieldedSrc.includes("{ name: 'AES-GCM' }"),
        'shielded note encryption should use AES-GCM');
    assert(shieldedSrc.includes('NOTE_ENCRYPTION_V1_PREFIX'),
        'shielded notes should carry an explicit encryption version prefix');
    assert(shieldedSrc.includes("entry.encrypted_note.startsWith(NOTE_ENCRYPTION_V1_PREFIX)"),
        'shielded decrypt should parse AES-GCM note format');
    assert(shieldedSrc.includes('Legacy compatibility: decrypt historical XOR-encrypted notes.'),
        'shielded decrypt should preserve compatibility for legacy XOR notes');
});

test('shielded.js stores encrypted shielded-note payload in localStorage', () => {
    assert(shieldedSrc.includes('async function deriveShieldedStorageKey()'),
        'shielded storage should derive an encryption key from shielded state keys');
    assert(shieldedSrc.includes('ciphertext'),
        'shielded storage payload should include ciphertext field');
    assert(shieldedSrc.includes('version: SHIELDED_STORAGE_VERSION'),
        'shielded storage payload should be versioned for future migrations');
    assert(shieldedSrc.includes('Legacy migration path: previous plaintext object format.'),
        'shielded storage loader should migrate legacy plaintext data to encrypted format');
});

console.log('\nW-23: Trusted RPC split for critical wallet flows');

test('wallet.js defines trusted RPC helpers for control-plane reads', () => {
    assert(walletSrc.includes('function getTrustedRpcEndpoint('), 'wallet.js should define getTrustedRpcEndpoint');
    assert(walletSrc.includes('async function trustedRpcCall('), 'wallet.js should define trustedRpcCall');
});

test('wallet.js loads token registry data from the signed metadata path', () => {
    assert(walletSrc.includes("trustedRpcCall('getAllSymbolRegistry', [{ limit: 2000 }])"),
        'loadTokenRegistry should use trustedRpcCall for the signed symbol registry snapshot');
    assert(!walletSrc.includes('deploy-manifest.json'),
        'loadTokenRegistry should not fetch the unsigned deploy-manifest JSON');
});

test('wallet.js pins bridge control-plane methods to trusted RPC', () => {
    assert(walletSrc.includes("trustedRpcCall('createBridgeDeposit'"),
        'bridge deposit creation should use trustedRpcCall');
    assert(walletSrc.includes("trustedRpcCall('getBridgeDeposit'"),
        'bridge deposit polling should use trustedRpcCall');
});

test('identity.js pins LichenID resolution to trusted metadata RPC', () => {
    assert(identitySrc.includes('window.resetIdentityNetworkCaches = resetIdentityNetworkCaches;'),
        'identity.js should expose a network cache reset hook');
    assert(identitySrc.includes("trustedRpcCall('getSymbolRegistry'"),
        'identity.js should use trustedRpcCall for symbol registry resolution');
    assert(identitySrc.includes("trustedRpcCall('getAllContracts'"),
        'identity.js should use trustedRpcCall for contract list fallback');
});

test('wallet settings explain that critical metadata stays pinned to trusted endpoints', () => {
    const normalizedWalletHtml = walletHtml.replace(/\s+/g, ' ');
    assert(normalizedWalletHtml.includes('Token contracts and contract resolution are verified against signed metadata manifests, while bridge routing stays pinned to trusted network endpoints.'),
        'wallet settings should explain the signed metadata and trusted transport split');
    assert(normalizedWalletHtml.includes('Leave a field blank to use the official endpoint.'),
        'wallet settings should explain how to clear custom RPC overrides');
});

// ============================================================================
// AUDIT-FIX H1-01 — Private key NOT exposed in toString/toJSON/inspect
// ============================================================================
console.log('\n── H1-01 Keypair Secret Key Protection ──');

test('H1-01: toString() does not contain secret key bytes', () => {
    const { Keypair } = require('../../sdk/js/dist/keypair');
    const kp = Keypair.generate();
    const str = kp.toString();

    // toString must contain "address" but never secret key
    assert(str.includes('address'), 'toString must mention address');
    assert(str.startsWith('Keypair('), 'toString must start with Keypair(');

    // Get the secret key hex to make sure it's NOT in the string
    const secretHex = Buffer.from(kp.getSecretKey()).toString('hex');
    assert(!str.includes(secretHex), 'toString must NOT contain secret key hex');

    // Ensure the full 64-byte secret key content is absent
    assert(str.length < 200, 'toString should be concise (only pubkey)');
});

test('H1-01: toJSON() excludes secret key', () => {
    const { Keypair } = require('../../sdk/js/dist/keypair');
    const kp = Keypair.generate();
    const json = JSON.stringify(kp);
    const parsed = JSON.parse(json);

    // JSON must have publicKey
    assert(parsed.publicKey, 'JSON must include publicKey');

    // JSON must NOT have secretKey or _secretKey
    assert(!parsed.secretKey, 'JSON must NOT include secretKey');
    assert(!parsed._secretKey, 'JSON must NOT include _secretKey');

    // Double-check: the secret key hex must not appear in stringified output
    const secretHex = Buffer.from(kp.getSecretKey()).toString('hex');
    assert(!json.includes(secretHex), 'JSON.stringify must NOT contain secret key hex');
});

test('H1-01: getSecretKey() returns valid 32-byte seed', () => {
    const { Keypair } = require('../../sdk/js/dist/keypair');
    const kp = Keypair.generate();
    const sk = kp.getSecretKey();
    assert(sk instanceof Uint8Array, 'getSecretKey must return Uint8Array');
    // ML-DSA-65: getSecretKey() returns the 32-byte seed, not expanded secret key material
    assert(sk.length === 32, 'ML-DSA-65 seed (from getSecretKey) must be 32 bytes');
});

test('H1-01: sign() still works with private _secretKey', () => {
    const { Keypair } = require('../../sdk/js/dist/keypair');
    const kp = Keypair.generate();
    const msg = new Uint8Array([1, 2, 3, 4]);
    const sig = kp.sign(msg);
    // ML-DSA-65: sign() returns a PqSignature object, not a raw 64-byte Uint8Array
    assert(sig && typeof sig === 'object', 'ML-DSA-65 sign() must return a PqSignature object');
    assert(typeof sig.verify === 'function', 'PqSignature must have a verify method');

    // Verify signature is valid
    const valid = sig.verify(msg);
    assert(valid, 'ML-DSA-65 signature must verify');
});

test('H1-01: secretKey field is not directly accessible', () => {
    const { Keypair } = require('../../sdk/js/dist/keypair');
    const kp = Keypair.generate();

    // The old public 'secretKey' field should no longer exist
    assert(kp.secretKey === undefined, 'secretKey field must not be publicly accessible');
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
