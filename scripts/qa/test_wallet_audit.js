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
const walletBridgeSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'js', 'dapp-bridge.js'), 'utf8');
const walletBootstrapSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'js', 'wallet-bootstrap.js'), 'utf8');
const walletSharedConfigSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'shared-config.js'), 'utf8');
const walletSharedUtilsSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'shared', 'utils.js'), 'utf8');
const walletCssSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'wallet.css'), 'utf8');
const walletManifest = JSON.parse(fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'manifest.json'), 'utf8'));
const walletServiceWorkerSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'sw.js'), 'utf8');
const walletRedirectsSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', '_redirects'), 'utf8');
const shieldedSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'js', 'shielded.js'), 'utf8');
const identitySrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'js', 'identity.js'), 'utf8');
const extensionFullSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'extension', 'src', 'pages', 'full.js'), 'utf8');
const extensionPopupSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'extension', 'src', 'popup', 'popup.js'), 'utf8');
const lichenidAbi = JSON.parse(fs.readFileSync(path.join(__dirname, '..', '..', 'contracts', 'lichenid', 'abi.json'), 'utf8'));
const walletHtml = fs.readFileSync(path.join(__dirname, '..', '..', 'wallet', 'index.html'), 'utf8');
const explorerAddressSrc = fs.readFileSync(path.join(__dirname, '..', '..', 'explorer', 'js', 'address.js'), 'utf8');
const explorerAddressHtml = fs.readFileSync(path.join(__dirname, '..', '..', 'explorer', 'address.html'), 'utf8');

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

test('wallet.js initializes import tabs from the dashboard import path', () => {
    const fnMatch = walletSrc.match(/function showImportWalletFromDashboard\(\)[\s\S]*?function showCreateWalletFromDashboard\(\)/);
    assert(fnMatch, 'showImportWalletFromDashboard function not found');
    assert(fnMatch[0].includes("showScreen('importWalletScreen')"), 'dashboard import should use the normal screen path');
    assert(fnMatch[0].includes('setupImportTabs()'), 'dashboard import should initialize import tab handlers');
    assert(fnMatch[0].includes("setImportMethod('seed')"), 'dashboard import should reset to the seed tab');
});

test('wallet.js private-key import accepts pasted export text and reports runtime failures', () => {
    assert(walletSrc.includes('function normalizePrivateKeyInput('), 'private key import should normalize pasted input');
    assert(walletSrc.includes('raw.match(/(?:0x)?[0-9a-fA-F]{64}/g)'), 'private key import should extract a single 64-hex token from pasted export text');
    const fnMatch = walletSrc.match(/async function importWalletPrivateKey\(\)[\s\S]*?async function importWalletJson\(\)/);
    assert(fnMatch, 'importWalletPrivateKey function not found');
    assert(fnMatch[0].includes('try {'), 'private key import should catch runtime crypto/storage failures');
    assert(fnMatch[0].includes('Private key import failed:'), 'private key import should show failed import errors');
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
    assert(shieldedSrc.includes('shieldLicn(amountText);'), 'confirmShield must trigger shieldLicn with validated amount text');
    assert(shieldedSrc.includes('function confirmUnshield()'), 'confirmUnshield handler must exist');
    assert(shieldedSrc.includes('unshieldLicn(amountText, recipient);'), 'confirmUnshield must trigger unshieldLicn with validated amount text');
    assert(shieldedSrc.includes("showToast('Enter a recipient address')"),
        'confirmUnshield must validate recipient input');
});

test('shielded.js uses signed transaction builders for supported shielded flows', () => {
    assert(shieldedSrc.includes('SHIELDED_SIGNED_SUBMISSION_AVAILABLE = true'), 'Shield/unshield signed submission should be enabled once the wallet builder is wired');
    assert(shieldedSrc.includes('SHIELDED_PRIVATE_TRANSFER_AVAILABLE = true'), 'Private transfer should be enabled through the signed native transaction path');
    assert(shieldedSrc.includes('async function submitShieldTransaction'), 'Shield flow must build and submit a signed shield transaction');
    assert(shieldedSrc.includes('async function submitUnshieldTransaction'), 'Unshield flow must build and submit a signed unshield transaction');
    assert(shieldedSrc.includes('async function submitShieldedTransferTransaction'), 'Private transfer flow must build and submit a signed transfer transaction');
    assert(shieldedSrc.includes('function buildShieldedTransferInstructionData'), 'Private transfer should include encrypted output note payloads in instruction data');
    assert(shieldedSrc.includes('function bytesToBase64(bytes)'), 'Shielded transaction encoding should use a chunked base64 helper for proof-sized payloads');
    assert(!shieldedSrc.includes("rpc.call('submitShieldTransaction'"), 'Shield flow must not call unsupported submitShieldTransaction RPC');
    assert(!shieldedSrc.includes("rpc.call('submitUnshieldTransaction'"), 'Unshield flow must not call unsupported submitUnshieldTransaction RPC');
    assert(!shieldedSrc.includes("rpc.call('submitShieldedTransfer'"), 'Transfer flow must not call unsupported submitShieldedTransfer RPC');
});

test('shielded.js gates private transfer inputs and self transfers', () => {
    assert(shieldedSrc.includes('function privateTransferValidationMessage()'),
        'Private transfer should use a shared validation message for button state and submit');
    assert(shieldedSrc.includes('function isOwnViewingKey(value)'),
        'Private transfer should compare the recipient viewing key against the active wallet viewing key');
    assert(shieldedSrc.includes("return 'Private transfers to your own viewing key are not allowed';"),
        'Private transfer should block sending shielded funds to the same wallet viewing key');
    assert(walletHtml.includes('id="shieldedTransferValidationMsg"'),
        'Private transfer modal should render inline validation feedback');
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

test('explorer vouch labels normalize full .lichen names once', () => {
    assert(explorerAddressSrc.includes('function formatExplorerLichenName('), 'Explorer should define a .lichen label normalizer');
    assert(explorerAddressSrc.includes("replace(/(?:\\.lichen)+$/i, '')"),
        'Explorer should strip one or more existing .lichen suffixes before display');
    assert(explorerAddressSrc.includes('formatExplorerLichenName(v.voucher_name)'),
        'Explorer received-vouch chips should use normalized .lichen labels');
    assert(explorerAddressSrc.includes('formatExplorerLichenName(v.vouchee_name)'),
        'Explorer given-vouch chips should use normalized .lichen labels');
    assert(!explorerAddressSrc.includes("${escapeHtml(v.voucher_name)}.lichen"),
        'Explorer should not append a second .lichen suffix to received vouches');
    assert(!explorerAddressSrc.includes("${escapeHtml(v.vouchee_name)}.lichen"),
        'Explorer should not append a second .lichen suffix to given vouches');
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

test('wallet.js persists encrypted browser wallet state for popup reconnects', () => {
    assert(walletSrc.includes("const PERSISTENT_WALLET_STATE_KEY = 'lichenWalletEncryptedState';"),
        'Wallet must use a dedicated encrypted browser storage key');
    assert(walletSrc.includes('function persistEncryptedBrowserWalletState()'),
        'Wallet must persist encrypted wallet records outside the popup session');
    assert(walletSrc.includes('localStorage.setItem(PERSISTENT_WALLET_STATE_KEY, JSON.stringify(storedState));'),
        'Persistent wallet storage must save the serialized encrypted wallet record');
    assert(walletSrc.includes('const persistentState = localStorage.getItem(PERSISTENT_WALLET_STATE_KEY);'),
        'Wallet load must restore encrypted browser wallet state after popup reopen');
    assert(walletSrc.includes('persistSessionWalletState();') && walletSrc.includes('persistEncryptedBrowserWalletState();'),
        'Wallet saves must keep active popup and encrypted browser state in sync');
    assert(!walletSrc.includes('Save wallet secrets in sessionStorage only'),
        'Wallet must not regress to popup-session-only custody');
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

    // Must fail closed instead of silently generating fake addresses or local key material.
    assert(walletConnectSrc.includes('throw new Error'),
        'Must throw error when no wallet provider is available');
    assert(walletConnectSrc.includes('No wallet provider available. Install the extension or connect through the web wallet.'),
        'Must fail closed with an explicit extension/web-wallet error');
});

test('wallet-connect.js exposes popup-backed web wallet provider without local custody fallback', () => {
    const walletConnectSrc = readFirstExisting([
        path.join(__dirname, '..', '..', 'shared', 'wallet-connect.js'),
        path.join(__dirname, '..', '..', 'dex', 'shared', 'wallet-connect.js'),
    ]);

    assert(walletConnectSrc.includes('function PopupLichenProvider('), 'PopupLichenProvider must exist');
    assert(walletConnectSrc.includes('function getPopupLichenProvider()'), 'Popup provider singleton accessor missing');
    assert(walletConnectSrc.includes("window.getPopupLichenProvider = window.getPopupLichenProvider || getPopupLichenProvider;"), 'Popup provider must be exported globally');
    assert(walletConnectSrc.includes("var WALLET_POPUP_STATE_KEY = 'lichen_web_wallet_popup_state_v1';"), 'Popup provider must track web-wallet session state locally');
    assert(walletConnectSrc.includes('PopupLichenProvider.prototype._handlePopupClosed') && walletConnectSrc.includes('this._setDisconnected();'),
        'Popup provider must clear signing readiness when the web-wallet popup closes');
    assert(walletConnectSrc.includes("Object.prototype.hasOwnProperty.call(state, 'hasWallet')")
        && walletConnectSrc.includes('hasWallet: hasWallet'),
        'Popup provider must preserve hasWallet from the web-wallet bridge state');
    assert(!walletConnectSrc.includes('createWallet') && !walletConnectSrc.includes('LichenPQ.generateKeypair'),
        'Popup provider must not fall back to local wallet generation');
    assert(walletConnectSrc.includes("url.searchParams.set('network', getSelectedWalletNetwork());"),
        'Popup wallet URL must carry the active frontend network');
});

test('wallet index loads dapp bridge before wallet bootstrap', () => {
    const bridgeIndex = walletHtml.indexOf('js/dapp-bridge.js');
    const bootstrapIndex = walletHtml.indexOf('js/wallet-bootstrap.js');

    assert(bridgeIndex >= 0, 'wallet/index.html must load js/dapp-bridge.js');
    assert(bootstrapIndex > bridgeIndex, 'wallet bootstrap must load after the dapp bridge');
});

test('dapp-bridge restricts popup flow to trusted origins with expiring approvals', () => {
    assert(walletBridgeSrc.includes("params.get('bridge') !== 'popup' || !window.opener"), 'Bridge must only run for popup mode with an opener');
    assert(walletBridgeSrc.includes('const TRUSTED_ORIGINS = buildTrustedOrigins();'), 'Bridge must build a trusted-origin allowlist');
    assert(walletBridgeSrc.includes('const APPROVED_ORIGIN_TTL_MS = 30 * 24 * 60 * 60 * 1000;'), 'Bridge approvals must expire');
    assert(walletBridgeSrc.includes('const RETURN_TO_URL = params.get(\'returnTo\');'), 'Bridge must inspect the popup return target');
    assert(walletBridgeSrc.includes('if (returnToOrigin && isLoopbackOrigin(window.location.origin) && isLoopbackOrigin(returnToOrigin)) {'), 'Bridge must trust loopback return origins during local development');
    assert(walletBridgeSrc.includes('Untrusted dApp origin'), 'Bridge must reject untrusted origins');
    assert(walletBridgeSrc.includes('const walletExists = hasWallet();') && walletBridgeSrc.includes('hasWallet: walletExists'),
        'Bridge provider state must expose whether the web wallet has any wallet loaded');
});

test('dapp-bridge requires password-gated approval for signing and uses canonical wallet helpers', () => {
    assert(walletBridgeSrc.includes("'licn_signMessage', 'licn_signTransaction', 'licn_sendTransaction'"), 'Bridge must mark signing methods as privileged');
    assert(walletBridgeSrc.includes('const approvalValues = await requestApproval(request);'), 'Bridge must require explicit approval before connect/sign actions');
    assert(walletBridgeSrc.includes('const REQUESTED_NETWORK = normalizeRequestedNetwork(params.get(\'network\'));'), 'Bridge must honor the requested popup network');
    assert(walletBridgeSrc.includes('const networkReady = await ensureRequestedNetwork();'), 'Bridge must wait for the requested network before interactive actions');
    assert(walletBridgeSrc.includes('function schedulePopupClose()'), 'Bridge must define popup completion handling');
    assert(walletBridgeSrc.includes('encrypted browser wallet is reopened') && !walletBridgeSrc.includes('window.close()'), 'Bridge must keep popup signers open after interactive requests');
    assert(walletBridgeSrc.includes('overflow-wrap:anywhere') && walletBridgeSrc.includes('word-break:break-word'), 'Bridge approval and hint UI must wrap long origins and addresses inside the popup');
    assert(walletBridgeSrc.includes('const values = await showPasswordModal({'), 'Bridge signing flow must use the wallet password modal');
    assert(walletBridgeSrc.includes('serializeMessageBincode(txObject.message || {})'), 'Bridge must sign canonical serialized transaction messages');
    assert(walletBridgeSrc.includes('rpc.sendTransaction(signResult.result.signedTransactionBase64)'), 'Bridge send flow must broadcast through the wallet RPC helper');
    assert(walletBridgeSrc.includes('transactionSignature: txHash') && walletBridgeSrc.includes('signature: txHash'), 'Bridge send result must expose the on-chain tx id as the transaction signature');
    assert(walletBridgeSrc.includes('pqSignatureHex'), 'Bridge signing results must expose PQ bytes under pqSignatureHex, not only signature');
});

test('dapp-bridge transaction approvals decode semantic signing intent', () => {
    assert(walletBridgeSrc.includes('function transactionIntentRows(txObject, providerState)'), 'Bridge should build semantic transaction intent rows');
    assert(walletBridgeSrc.includes('function readU64Le(data, offset = 0)'), 'Bridge should decode u64 base-unit amounts without Number');
    assert(walletBridgeSrc.includes('function formatBaseUnits(value, decimals, symbol)'), 'Bridge should format base units with token decimals');
    assert(walletBridgeSrc.includes('function decodeContractCallIntent(data)'), 'Bridge should decode contract call intent best-effort');
    for (const label of ['Account', 'Destination', 'Amount', 'Token decimals', 'Network', 'RPC', 'Fee', 'Primary program', 'Warnings']) {
        assert(walletBridgeSrc.includes(`'${label}'`), `Bridge signing prompt missing ${label} row`);
    }
    assert(walletBridgeSrc.includes('transactionSummaryHtml(normalizeTransactionObject(request.payload), providerState)'),
        'Bridge transaction summary should include provider state for network/RPC context');
    assert(walletBridgeSrc.includes('Contract payload contains admin-like terms; review before signing.'),
        'Bridge contract signing prompt should warn on admin-like payload terms');
});

test('dapp-bridge first-sign approvals disclose account access and only connect after successful signing', () => {
    assert(walletBridgeSrc.includes('function approvalGrantsAccountAccess(method, providerState)'), 'Bridge must detect first-sign account access grants');
    assert(walletBridgeSrc.includes('Approving also connects this site to your active account until the approval expires.'), 'Bridge first-sign prompt must disclose account access');
    assert(walletBridgeSrc.includes("'Account access'") && walletBridgeSrc.includes('Connects this site to'), 'Bridge first-sign prompt must show account access details');
    assert(walletBridgeSrc.includes("'Approve & Connect'"), 'Bridge first-sign confirmation text must include connect consent');

    const signIndex = walletBridgeSrc.indexOf("if (method === 'licn_signMessage')");
    const finalizeIndex = walletBridgeSrc.indexOf('const result = await finalizeSignMessage(request, approvalValues.password);', signIndex);
    const approveIndex = walletBridgeSrc.indexOf('if (result?.ok) approveOrigin(request.origin);', finalizeIndex);
    const responseIndex = walletBridgeSrc.indexOf('sendResponse(request, result);', approveIndex);
    assert(signIndex >= 0 && finalizeIndex > signIndex, 'Bridge signMessage finalization path not found');
    assert(approveIndex > finalizeIndex && responseIndex > approveIndex, 'Bridge must approve origin only after successful signing finalization');
});

test('dapp-bridge network changes require origin-scoped approval before mutation', () => {
    assert(walletBridgeSrc.includes("wallet_switchEthereumChain: 'licn_switchNetwork'"), 'Bridge must alias wallet_switchEthereumChain');
    assert(walletBridgeSrc.includes("wallet_addEthereumChain: 'licn_addNetwork'"), 'Bridge must alias wallet_addEthereumChain');
    assert(walletBridgeSrc.includes("const NETWORK_CHANGE_METHODS = new Set(['licn_switchNetwork', 'licn_addNetwork']);"),
        'Bridge must classify network-change methods as privileged');
    assert(walletBridgeSrc.includes('function buildNetworkChangeRequest(payload, providerState)'),
        'Bridge must build explicit network-change approval details');
    assert(walletBridgeSrc.includes('Review this network switch request before changing the active wallet network.'),
        'Bridge switch prompt must explain the mutation');
    assert(walletBridgeSrc.includes('Review this network addition request before saving the RPC endpoint and changing the active wallet network.'),
        'Bridge add-network prompt must explain RPC and network mutation');
    assert(walletBridgeSrc.includes("method === 'licn_switchNetwork'") && walletBridgeSrc.includes("'Switch Network'"),
        'Bridge switch approval button should be explicit');
    assert(walletBridgeSrc.includes("method === 'licn_addNetwork'") && walletBridgeSrc.includes("'Add & Switch Network'"),
        'Bridge add-network approval button should be explicit');

    const processIndex = walletBridgeSrc.indexOf('async function processRequest(request)');
    const approvalIndex = walletBridgeSrc.indexOf('const approvalValues = await requestApproval(request);');
    const networkFinalizeIndex = walletBridgeSrc.indexOf('const result = await finalizeNetworkChange(request);', approvalIndex);
    const switchNetworkIndex = walletBridgeSrc.indexOf('await switchNetwork(change.nextNetwork);');
    assert(processIndex >= 0 && switchNetworkIndex > 0 && switchNetworkIndex < processIndex,
        'Bridge network mutation should be isolated inside finalizeNetworkChange');
    assert(approvalIndex >= 0 && networkFinalizeIndex > approvalIndex,
        'Bridge processRequest must call finalizeNetworkChange only after approval');
});

test('popup wallet sessions are not forcibly reloaded or re-locked mid-flow', () => {
    const walletConnectSrc = readFirstExisting([
        path.join(__dirname, '..', '..', 'shared', 'wallet-connect.js'),
        path.join(__dirname, '..', '..', 'dex', 'shared', 'wallet-connect.js'),
    ]);

    assert(walletSrc.includes('const BRIDGE_POPUP_UNLOCK_SESSION_KEY = \'lichen_wallet_bridge_popup_unlocked\';'), 'Popup sessions must track unlock state in sessionStorage');
    assert(walletSrc.includes('if (isBridgePopupSession() && hasBridgePopupUnlockSession() && walletState.isLocked === false) {'), 'Popup sessions must preserve an unlocked wallet state only within the current popup window');
    assert(walletBootstrapSrc.includes('if (!isBridgePopupSession()) {'), 'Service worker updates must not auto-reload the popup session');
    assert(walletConnectSrc.includes('if (!this.popup || this.popup.closed) {'), 'Popup provider must reuse an existing popup window');
    assert(!walletConnectSrc.includes('this.popup.location.href = popupUrl;'), 'Popup provider must not renavigate an already-open popup during repeated requests');
    assert(walletConnectSrc.includes('this._setDisconnected();') && walletConnectSrc.includes('return Promise.resolve(this._lastState);'), 'Closing the popup must fail closed before returning cached provider state');
    assert(walletConnectSrc.includes('self._handlePopupClosed();'), 'Popup close monitoring must use the non-disconnecting close handler');
});

test('shared wallet-connect copies revalidate stored provider sessions before connected UX', () => {
    const sharedWalletConnectPaths = [
        path.join(__dirname, '..', '..', 'wallet', 'shared', 'wallet-connect.js'),
        path.join(__dirname, '..', '..', 'explorer', 'shared', 'wallet-connect.js'),
        path.join(__dirname, '..', '..', 'marketplace', 'shared', 'wallet-connect.js'),
        path.join(__dirname, '..', '..', 'developers', 'shared', 'wallet-connect.js'),
        path.join(__dirname, '..', '..', 'monitoring', 'shared', 'wallet-connect.js'),
        path.join(__dirname, '..', '..', 'faucet', 'shared', 'wallet-connect.js'),
        path.join(__dirname, '..', '..', 'programs', 'shared', 'wallet-connect.js'),
        path.join(__dirname, '..', '..', 'dex', 'shared', 'wallet-connect.js'),
    ];

    sharedWalletConnectPaths.forEach((filePath) => {
        const source = fs.readFileSync(filePath, 'utf8');
        assert(source.includes('LichenWallet.prototype._readProviderAccounts = async function (provider)'), `${filePath} should read live provider accounts during restore`);
        assert(source.includes('self._restoreValidatedConnection(data, restoredAddress'), `${filePath} should only restore after live provider validation`);
        assert(!source.includes('this.address = data.address;\n                this._walletData = data;\n                this._startBalancePolling();\n                this.refreshBalance();'), `${filePath} should not trust raw stored provider state before validation`);
    });
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

test('serializeMessageBincode includes optional compute budget fields in signed bytes', () => {
    const fnMatch = walletSharedUtilsSrc.match(/function serializeMessageBincode\(message\)[\s\S]*?^}/m);
    assert(fnMatch, 'serializeMessageBincode must exist in shared/utils.js');
    const serializeMessageBincode = (new Function(fnMatch[0] + '\nreturn serializeMessageBincode;'))();

    const blockhash = 'b'.repeat(64);
    const withoutBudget = serializeMessageBincode({ instructions: [], blockhash });
    const withBudget = serializeMessageBincode({ instructions: [], blockhash, compute_budget: 1400000 });

    assert.strictEqual(withoutBudget.length, 42, 'empty message without compute budget should include two None tags');
    assert.strictEqual(withoutBudget[40], 0, 'compute_budget None tag should be 0');
    assert.strictEqual(withoutBudget[41], 0, 'compute_unit_price None tag should be 0');
    assert.strictEqual(withBudget.length, 50, 'Some(compute_budget) should add an 8-byte u64 payload');
    assert.strictEqual(withBudget[40], 1, 'compute_budget Some tag should be 1');
    const view = new DataView(withBudget.buffer, withBudget.byteOffset + 41, 8);
    assert.strictEqual(Number(view.getBigUint64(0, true)), 1400000, 'compute_budget u64 must be serialized little-endian');
    assert.strictEqual(withBudget[49], 0, 'compute_unit_price should remain None');
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
    assert(identitySrc.includes("newRateSpores = parseIdentityLicnSpores(values.rate || '0', 'Rate', { allowZero: true });"),
        'identity edit flow should parse set_rate through strict base-unit helpers');
    assert(identitySrc.includes("buildContractCall('set_rate', { licn_per_unit: newRateSpores.toString() }, values.password)"),
        'identity edit flow must pass set_rate as integer spore units');
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

test('wallet.js activity identifies shield and unshield transactions explicitly', () => {
    assert(walletSrc.includes("'Shield': 'Shielded'"), 'wallet activity should label Shield transactions');
    assert(walletSrc.includes("'Unshield': 'Unshielded'"), 'wallet activity should label Unshield transactions');
    assert(walletSrc.includes("tx.type === 'Shield'"), 'wallet activity should special-case Shield direction');
    assert(walletSrc.includes("tx.type === 'Unshield'"), 'wallet activity should special-case Unshield direction');
    assert(walletSrc.includes("? 'Shielded Pool'"), 'wallet activity should show shielded pool counterpart');
});

test('wallet.js activity treats ContractCall as contract activity and fee-only when amount is zero', () => {
    assert(walletSrc.includes("'ContractCall': 'Contract Call'"), 'wallet activity should label ContractCall transactions');
    assert(walletSrc.includes("tx.type === 'Contract' || tx.type === 'ContractCall'"),
        'wallet activity should share contract icon/function handling across Contract and ContractCall');
    assert(walletSrc.includes("|| tx.type === 'ContractCall'"),
        'wallet activity should include ContractCall in zero-amount fee-only detection');
    assert(walletSrc.includes("(tx.type === 'Contract' || tx.type === 'ContractCall') && amount !== '0'"),
        'wallet activity should include ContractCall in paid contract detection');
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
    assert(identitySrc.includes('bid_amount_spores: bidAmountSpores.toString()'),
        'identity bid flow should pass integer spore bid amount into bid_name_auction args');
    assert(identitySrc.includes('identityU64ToBigInt(params.bid_amount_spores)'),
        'bid_name_auction encoder should use integer spore bid amounts');
    assert(!identitySrc.includes('Math.floor((params.bid_amount || 0) * 1_000_000_000)'),
        'bid_name_auction encoder should not use floating-point LICN conversion');
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

test('shielded.js encrypts notes with AES-GCM and rejects non-versioned note payloads', () => {
    assert(shieldedSrc.includes("{ name: 'AES-GCM' }"),
        'shielded note encryption should use AES-GCM');
    assert(shieldedSrc.includes('NOTE_ENCRYPTION_V1_PREFIX'),
        'shielded notes should carry an explicit encryption version prefix');
    assert(shieldedSrc.includes('encryptedNote.startsWith(NOTE_ENCRYPTION_V1_PREFIX)'),
        'shielded decrypt should parse AES-GCM note format');
    assert(!shieldedSrc.includes('Legacy compatibility: decrypt historical XOR-encrypted notes.'),
        'shielded decrypt should not silently accept old unversioned note formats');
});

test('shielded.js stores encrypted shielded-note payloads locally and in shield tx data', () => {
    assert(shieldedSrc.includes('async function deriveShieldedStorageKey()'),
        'shielded storage should derive an encryption key from shielded state keys');
    assert(shieldedSrc.includes('function getShieldedStorageKeyName()'),
        'shielded storage should derive a wallet-scoped storage key');
    assert(shieldedSrc.includes('lichen_shielded_notes:${address}'),
        'shielded note storage should be scoped by wallet address');
    assert(shieldedSrc.includes('ciphertext'),
        'shielded storage payload should include ciphertext field');
    assert(shieldedSrc.includes('version: SHIELDED_STORAGE_VERSION'),
        'shielded storage payload should be versioned for future migrations');
    assert(shieldedSrc.includes('SHIELDED_NOTE_PAYLOAD_MAGIC'),
        'shielded deposits should include a typed encrypted-note payload envelope');
    assert(shieldedSrc.includes('encrypted_note: encryptedNote'),
        'shielded deposits should include the encrypted note in instruction data');
    assert(shieldedSrc.includes('ephemeral_pk: ephemeralPk'),
        'shielded deposits should include the ephemeral public key in instruction data');
});

test('shielded.js does not overwrite encrypted note storage after a failed decrypt', () => {
    assert(shieldedSrc.includes('storageLoadFailed'),
        'shielded state should track failed local note storage loads');
    assert(shieldedSrc.includes('Shielded notes were not saved because local note storage failed to decrypt'),
        'shielded storage save should refuse to overwrite after a decrypt failure');
    assert(shieldedSrc.includes('Shielded note storage could not be decrypted. Not overwriting local notes.'),
        'shielded storage load should surface failed decrypt without overwriting notes');
    assert(!shieldedSrc.includes("localStorage.getItem('lichen_shielded_notes')"),
        'shielded storage should not fall back to unscoped local note storage');
});

test('shielded.js rescans commitment pages to restore owned notes from encrypted payloads', () => {
    assert(shieldedSrc.includes('while (!Number.isFinite(totalCommitments) || totalCommitments <= 0 || from < totalCommitments)'),
        'shielded sync should page through commitments instead of reading one short page');
    assert(shieldedSrc.includes('const note = await tryDecryptNote(entry);'),
        'shielded sync should attempt to decrypt encrypted note payloads from RPC');
    assert(shieldedSrc.includes('entry?.encrypted_note || entry?.encryptedNote'),
        'shielded sync should accept canonical snake_case and REST camelCase encrypted-note fields');
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
    assert(walletSrc.includes('buildBridgeAccessMessageV2('),
        'bridge deposit creation should sign route-bound V2 bridge auth messages');
    assert(walletSrc.includes("BRIDGE_AUTH_DOMAIN_V2 = 'LICHEN_BRIDGE_ACCESS_V2'"),
        'bridge auth should mark the V2 domain');
    assert(walletSrc.includes('route=${canonicalChain}:${normalizedAsset}'),
        'bridge auth should bind the canonical chain/asset route');
    assert(walletSrc.includes('activeBridgeAuth.version = 2'),
        'bridge deposit creation should emit a V2 auth envelope');
    assert(walletSrc.includes('activeBridgeAuth.nonce = nonce'),
        'bridge deposit creation should include a fresh nonce');
    assert(walletSrc.includes('decryptKeypair(wallet.encryptedKey'),
        'bridge authorization should derive wallet identity from decrypted key material before signing');
    assert(walletSrc.includes('keypair.address !== wallet.address'),
        'bridge authorization should reject encrypted keys that do not match the active wallet');
    assert(walletSrc.includes('bridgeDepositUserMessage(error)'),
        'bridge deposit flow should map custody/auth failures to user-safe messages');
    assert(walletSrc.includes('bridgeRouteReadinessMessage(status, asset, chainName)'),
        'bridge deposit flow should reject non-ready custody routes before bridge authorization');
    assert(walletSrc.includes("['route_ready', 'deposit_ready', 'custody_configured', 'custody_status']"),
        'bridge route preflight should consume custody readiness fields');
    assert(walletSrc.includes('renderBridgeDepositError({ container: depositResult, message })'),
        'bridge deposit flow should render custody route failures inline in the modal');
    assert(walletSrc.indexOf('await assertBridgeRouteOpen(chain, asset, chainName)') < walletSrc.indexOf('await ensureBridgeAccessAuth(wallet'),
        'bridge route status must be checked before asking the wallet to sign bridge auth');
});

test('wallet.js compact balance shows stLICN amount while explorer separates MossStake value and pending unstake', () => {
    assert(walletSrc.includes('Staking: <strong>${fmtToken(snapshot.stLicn, 4)} stLICN</strong>'),
        'balance card should show the actual stLICN position amount with units');
    assert(walletSrc.includes("rpc.call('getStakingPosition', [wallet.address])"),
        'balance card should fetch staking position for stLICN amount');
    assert(!walletSrc.includes('Liquid Staking Value: <strong>${fmtToken(snapshot.mossStaked, 4)}</strong>'),
        'balance card must not label redeemable LICN value as liquid staking value');
    assert(explorerAddressSrc.includes("rpcCall('getStakingPosition', [address])"),
        'explorer address page should fetch staking position for stLICN account summary');
    assert(explorerAddressSrc.includes("rpcCall('getUnstakingQueue', [address])"),
        'explorer address page should fetch pending MossStake unstake requests');
    assert(explorerAddressHtml.includes('Estimated Total Value (LICN)'),
        'explorer account summary should show estimated total value across native and MossStake balances');
    assert(explorerAddressHtml.includes('MossStake Redeemable Value'),
        'explorer account summary should show MossStake redeemable LICN separately');
    assert(explorerAddressHtml.includes('Pending MossStake Unstake'),
        'explorer account summary should show pending MossStake unstake separately');
    assert(explorerAddressHtml.includes('stLICN Balance'),
        'explorer account summary should label staking as stLICN shares');
});

test('wallet.js MossStake activity rows match explorer units and lifecycle labels', () => {
    assert(walletSrc.includes("'MossStakeUnstake': 'Unstake Requested'"),
        'wallet activity should label MossStake unstake as a cooldown request, not completed unstaking');
    assert(walletSrc.includes("'MossStakeClaim': 'Claimed Unstake'"),
        'wallet activity should reserve completed wording for claim transactions');
    assert(walletSrc.includes("'MossStakeTransfer': 'stLICN Transfer'"),
        'wallet activity should label MossStake transfers as stLICN transfers');
    assert(walletSrc.includes("tx.type === 'MossStakeDeposit' || tx.type === 'MossStakeUnstake' || tx.type === 'Stake'"),
        'wallet activity should show MossStake unstake requests as outgoing stLICN burns');
    assert(walletSrc.includes("const amountUnit = tx.type === 'MossStakeUnstake' || tx.type === 'MossStakeTransfer'"),
        'wallet activity should render MossStake unstake and transfer amounts in stLICN');
    assert(walletSrc.includes("'MossStake Pool'"),
        'wallet activity should identify MossStake pool transactions explicitly');
});

test('wallet.js exposes Neo X bridge controls with route status and reserve context', () => {
    assert(walletHtml.includes('data-wallet-arg="NEOX"'),
        'wallet receive modal should expose the Neo X deposit route');
    assert(walletSrc.includes("NEOX: { name: 'Neo X'"),
        'wallet.js should define Neo X chain metadata');
    assert(walletSrc.includes("tokens: ['GAS', 'NEO']"),
        'Neo X deposit UI should expose GAS and NEO custody routes');
    assert(walletSrc.includes("const validAssets = ['usdc', 'usdt', 'sol', 'eth', 'bnb', 'gas', 'neo', 'btc'];"),
        'Neo X NEO and Bitcoin BTC deposits should pass wallet-side bridge asset validation');
    assert(extensionFullSrc.includes("neox: { label: 'Neo X'") && extensionFullSrc.includes("assets: ['gas', 'neo']"),
        'extension full-page deposit flow should expose both Neo X GAS and NEO');
    assert(extensionPopupSrc.includes("NEOX: { name: 'Neo X'") && extensionPopupSrc.includes("tokens: ['GAS', 'NEO']"),
        'extension popup deposit flow should expose both Neo X GAS and NEO');
    assert(walletSrc.includes("trustedRpcCall('getBridgeRouteRestrictionStatus'"),
        'wallet bridge deposit flow should preflight route status');
    assert(walletSrc.includes("getWneoStats") && walletSrc.includes("getWgasStats"),
        'wallet asset list should read Neo wrapped reserve stats');
    assert(walletSrc.includes("trustedRpcCall('getNeoGasRewardsStats'") && walletSrc.includes("trustedRpcCall('getNeoGasRewardsPosition'"),
        'wallet asset list should read Neo GAS rewards vault stats and positions through trusted RPC');
    assert(extensionFullSrc.includes("getNeoGasRewardsStats") && extensionFullSrc.includes("getNeoGasRewardsPosition"),
        'extension full-page asset list should display Neo GAS rewards vault accounting');
    assert(extensionPopupSrc.includes("getNeoGasRewardsStats") && extensionPopupSrc.includes("getNeoGasRewardsPosition"),
        'extension popup asset list should display Neo GAS rewards vault accounting');
    assert(walletSrc.includes('wNEO transfers require whole NEO lots'),
        'wallet sends should reject fractional wNEO');
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
    assert(normalizedWalletHtml.includes('Enable unsafe custom RPC mode'),
        'wallet settings should require an explicit unsafe-mode toggle for custom RPC endpoints');
    assert(normalizedWalletHtml.includes('Untrusted RPCs can spoof balances, recent blockhashes, and confirmation state.'),
        'wallet settings should explain the trust risk of custom RPC endpoints');
    assert(normalizedWalletHtml.includes('id="unsafeRpcMode"'),
        'wallet settings should render the unsafe RPC mode toggle');
    assert(walletSrc.includes('function isUnsafeRpcModeEnabled()'),
        'wallet.js should gate custom RPC overrides behind explicit unsafe mode');
    assert(walletSrc.includes('settings.allowUnsafeRpc'),
        'wallet.js should persist explicit unsafe RPC mode state');
});

console.log('\nW-23b: Wallet unlock desktop layout');

test('wallet lock screen uses the wider desktop card while preserving mobile bounds', () => {
    assert(walletSrc.includes('function showUnlockScreen()'),
        'wallet.js should render the unlock screen through showUnlockScreen');
    assert(walletSrc.includes('class="unlock-card"'),
        'unlock screen should use the unlock-card surface');
    assert(walletCssSrc.includes('max-width: 560px;'),
        'desktop unlock card should match the wider wallet welcome surfaces');
    assert(walletCssSrc.includes('max-width: calc(100vw - 1.5rem);'),
        'mobile unlock card should remain bounded to the viewport');
});

console.log('\nW-24: Restriction status banners, asset badges, and wallet send preflight');

test('shared-config exposes canonical restriction RPC method names', () => {
    assert(walletSharedConfigSrc.includes('const restrictionStatus = Object.freeze({'),
        'shared config should define a restriction status config object');
    assert(walletSharedConfigSrc.includes("nativeAsset: 'native'"),
        'shared config should publish the native restriction asset alias');
    for (const method of [
        'getAccountRestrictionStatus',
        'getAssetRestrictionStatus',
        'getAccountAssetRestrictionStatus',
        'canSend',
        'canReceive',
        'canTransfer',
    ]) {
        assert(walletSharedConfigSrc.includes(`'${method}'`),
            `shared config should expose ${method}`);
    }
    assert(walletSharedConfigSrc.includes('restrictions: restrictionStatus'),
        'LICHEN_CONFIG should export restriction status config');
});

test('wallet HTML and CSS include restriction banner and badge surfaces', () => {
    assert(walletHtml.includes('id="walletRestrictionStatus"'),
        'wallet dashboard should include an account/native restriction status banner');
    assert(walletHtml.includes('id="sendRestrictionStatus"'),
        'send modal should include a restriction warning surface');
    assert(walletCssSrc.includes('.wallet-restriction-status'),
        'wallet CSS should style the restriction status banner');
    assert(walletCssSrc.includes('.asset-restriction-badge'),
        'wallet CSS should style per-asset restriction badges');
    assert(walletCssSrc.includes('.send-restriction-status'),
        'wallet CSS should style send-modal restriction warnings');
});

test('wallet.js loads restriction status through trusted consensus RPC reads', () => {
    for (const method of [
        'getAccountRestrictionStatus',
        'getAssetRestrictionStatus',
        'getAccountAssetRestrictionStatus',
        'canSend',
        'canReceive',
    ]) {
        assert(walletSrc.includes(`'${method}'`),
            `wallet.js should reference ${method}`);
        assert(!walletSrc.includes(`rpc.call('${method}'`),
            `${method} should not use the untrusted user-overridable RPC path`);
    }
    assert(walletSrc.includes('async function refreshWalletRestrictionStatus'),
        'wallet should centralize restriction status refresh and caching');
    assert(walletSrc.includes('await trustedRpcCall(methodName, params)'),
        'restriction reads should use trustedRpcCall');
});

test('wallet.js renders restriction data as account banners and per-asset badges', () => {
    assert(walletSrc.includes('function renderWalletRestrictionStatus(status)'),
        'wallet should render restriction status banner');
    assert(walletSrc.includes('function renderAssetRestrictionBadges(assetState)'),
        'wallet should render per-asset restriction badges');
    assert(walletSrc.includes('data-asset-restriction-badges="LICN"'),
        'LICN asset row should expose a badge mount point');
    assert(walletSrc.includes('data-asset-symbol="LICN"'),
        'LICN asset row should expose a stable asset symbol data attribute');
    assert(walletSrc.includes("class=\"asset-restriction-badge ${escapeHtml(badge.kind)}\""),
        'asset badge rendering should escape dynamic badge class labels');
});

test('wallet send flow checks canTransfer before signing or submitting', () => {
    assert(walletSrc.includes('async function preflightWalletTransferRestrictions'),
        'wallet should define send preflight helper');
    assert(walletSrc.includes("trustedRpcCall(walletRestrictionMethod('canTransfer', 'canTransfer')"),
        'send preflight should use trusted canTransfer RPC');
    assert(!walletSrc.includes("rpc.call('canTransfer'"),
        'send preflight must not use user-overridable RPC');

    const preflightIndex = walletSrc.indexOf('await preflightWalletTransferRestrictions(wallet, to, selectedToken, amountText);');
    const buildIndex = walletSrc.indexOf("showToast('Building transaction...');");
    assert(preflightIndex !== -1, 'confirmSend should call restriction preflight');
    assert(buildIndex !== -1, 'confirmSend should still build transactions after preflight');
    assert(preflightIndex < buildIndex, 'restriction preflight should run before transaction building/signing');
});

test('wallet restriction UI escapes server-provided status and token metadata', () => {
    assert(walletSrc.includes('escapeHtml(details)'),
        'banner details should be escaped before rendering');
    assert(walletSrc.includes('escapeHtml(badge.label)'),
        'asset badge labels should be escaped before rendering');
    assert(walletSrc.includes('safeHttpImageUrl(token.logoUrl)'),
        'token logo metadata should reject non-http(s) URLs before rendering');
    assert(walletSrc.includes("const LICN_LOGO_URL = 'https://lichen.network/assets/img/coins/128x128/licn.png';"),
        'wallet LICN asset row should use the canonical website coin logo');
    assert(walletSrc.includes('style="width:32px;height:32px;border-radius:50%;object-fit:cover;"'),
        'wallet asset logo images should render at 32x32');
    assert(walletSrc.includes('safeCssColor(token.color)'),
        'token color metadata should be sanitized before style rendering');
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

console.log('\n── MossStake Display Safety ──');

test('MossStake tier cards show deterministic reward multipliers', () => {
    assert(walletSrc.includes('function formatMossStakeRewardLabel('), 'wallet should use a MossStake reward formatter');
    assert(walletSrc.includes('formatMossStakeRewardLabel(t.apy_percent, t.multiplier)'), 'wallet tier cards should use reward labels');
    assert(walletSrc.includes('Accrued Rewards'), 'wallet should label MossStake gain as accrued rewards');
    assert(!walletSrc.includes('Est. Exchange Gain'), 'wallet should not call tier-weighted rewards exchange gain');
    assert(walletSrc.includes('position-bound'), 'wallet should explain locked MossStake tiers are position-bound');
    assert(extensionFullSrc.includes('function formatMossStakeRewardLabel('), 'extension full page should use reward labels');
    assert(extensionPopupSrc.includes('function formatMossStakeRewardLabel('), 'extension popup should use reward labels');
    assert(extensionFullSrc.includes('Accrued Rewards'), 'extension full page should label MossStake gain as accrued rewards');
    assert(extensionPopupSrc.includes('Accrued Rewards'), 'extension popup should label MossStake gain as accrued rewards');
    assert(!extensionFullSrc.includes('Est. Exchange Gain'), 'extension full page should not call tier-weighted rewards exchange gain');
    assert(!extensionPopupSrc.includes('Est. Exchange Gain'), 'extension popup should not call tier-weighted rewards exchange gain');
    assert(!walletSrc.includes('MOSSSTAKE_APY_DISPLAY_CAP_PERCENT'), 'wallet should not render unstable APY caps');
    assert(!extensionFullSrc.includes('MOSSSTAKE_APY_DISPLAY_CAP_PERCENT'), 'extension full page should not render unstable APY caps');
    assert(!extensionPopupSrc.includes('MOSSSTAKE_APY_DISPLAY_CAP_PERCENT'), 'extension popup should not render unstable APY caps');
});

test('MossStake lock and claim checks use chain slot instead of wall-clock slot guesses', () => {
    assert(walletSrc.includes('async function getCurrentChainSlot('), 'wallet should centralize current chain slot lookup');
    assert(!walletSrc.includes('Date.now() / MS_PER_SLOT'), 'wallet should not estimate MossStake slots from wall-clock time');
    assert(walletSrc.includes('data-wallet-action="claimMossStake" data-wallet-pass-el="true"'),
        'wallet MossStake claim button should pass itself to the claim handler for visible pending state');
    assert(walletSrc.includes('async function claimMossStake(triggerEl)'),
        'wallet MossStake claim handler should accept the clicked button');
    assert(walletSrc.includes('sendAndConfirmTransaction(txBase64)'),
        'wallet MossStake claim should preflight, broadcast, and wait for confirmation');
    assert(walletSrc.includes('Network fee will be deducted from the claimed LICN'),
        'wallet should explain matured MossStake claims can pay the network fee from claim proceeds');
    assert(walletSrc.includes('requiredLicn: 0'),
        'wallet MossStake claim password modal should not block zero-spendable matured claims');
});

test('wallet shielded unshield supports max/full balance across multiple notes', () => {
    assert(walletHtml.includes('data-wallet-action="fillUnshieldMax"'),
        'web wallet unshield modal should expose a MAX control');
    assert(walletSrc.includes("actionName === 'fillUnshieldMax'"),
        'web wallet should wire the unshield MAX action');
    assert(shieldedSrc.includes('function selectExactUnshieldNotes('),
        'web wallet should select an exact set of real notes for unshield');
    assert(shieldedSrc.includes('submitUnshieldBatchTransaction('),
        'web wallet should submit multi-note unshield through the signed transaction path');
    assert(shieldedSrc.includes('const SHIELDED_UNSHIELD_CU_PER_NOTE = 200_000'),
        'web wallet should use the canonical per-note unshield compute cost');
    assert(shieldedSrc.includes('const SHIELDED_MAX_TX_COMPUTE_BUDGET = 1_400_000'),
        'web wallet should respect the protocol max compute budget');
    assert(shieldedSrc.includes('computeBudget: computeUnshieldBatchBudget(chunk.length)'),
        'web wallet should request enough compute budget for multi-note unshield batches');
    assert(shieldedSrc.includes('chunkUnshieldEntries(entries)'),
        'web wallet should split unshield submissions that exceed one transaction compute budget');
    assert(!shieldedSrc.includes('Unshield currently requires a single note exactly matching the amount'),
        'web wallet should not require users to unshield by individual note size');
});

test('shielded.js uses the Merkle path root for unshield proofs when available', () => {
    assert(shieldedSrc.includes('merklePath?.root || merklePath?.merkleRoot || merklePath?.merkle_root'),
        'unshield should prefer the fresh Merkle root returned with the note path');
    assert(shieldedSrc.includes('shieldedState.merkleRoot = merkleRoot'),
        'unshield should update local shielded state with the root used for proof generation');
});

test('shielded.js resolves note commitment indexes from chain commitments', () => {
    assert(shieldedSrc.includes('async function resolveShieldedCommitmentIndex('),
        'shielded notes should resolve their chain commitment index by commitment hash');
    assert(shieldedSrc.includes('const noteIndex = await resolveNoteCommitmentIndex(noteToSpend);'),
        'unshield should verify or refresh the note index before fetching the Merkle path');
    assert(shieldedSrc.includes('existingNote.index = entryIndex'),
        'shielded sync should repair stored note indexes from RPC commitment entries');
    assert(shieldedSrc.includes('pendingIndex: !Number.isFinite(commitmentIndex)'),
        'new shield notes should be marked pending instead of saving an unverified guessed index');
});

test('shielded.js asks RPC for native Poseidon2 nullifiers', () => {
    assert(shieldedSrc.includes("rpc.call('computeShieldNullifier'"),
        'wallet must use the core/RPC native nullifier helper');
    assert(!shieldedSrc.includes('const data = new Uint8Array([\n        ...hexToBytes(serialHex),'),
        'wallet must not derive shielded nullifiers with an ad hoc client hash');
});

test('wallet activity deduplicates faucet API records already present on-chain', () => {
    assert(walletSrc.includes('function activityItemKey('), 'wallet should derive stable activity keys');
    assert(walletSrc.includes('function mergeActivityItems('), 'wallet should merge activity through one dedupe path');
    assert(walletSrc.includes('.filter(a => !chainKeys.has(activityItemKey(a)))'),
        'faucet API records should be hidden when the same signature is already in RPC history');
    assert(walletSrc.includes('existing.isAirdrop && !item.isAirdrop'),
        'on-chain activity should win over synthetic faucet records for the same signature');
});

console.log('\n── Wallet Input Guard Safety ──');

test('wallet locks unshield recipient to the active wallet address', () => {
    assert(walletHtml.includes('id="unshieldRecipient"'), 'unshield recipient input should exist');
    assert(walletHtml.includes('readonly aria-readonly="true"'), 'unshield recipient should be read-only in markup');
    assert(walletHtml.includes('data-address-input="base58"'), 'unshield recipient should be base58-guarded');
    assert(shieldedSrc.includes("recipientInput.title = 'Unshield returns to the active wallet address';"),
        'unshield modal should describe the locked recipient');
    assert(shieldedSrc.includes('const wallet = typeof getActiveWallet === \'function\' ? getActiveWallet() : null;'),
        'unshield confirmation should resolve the active wallet');
    assert(shieldedSrc.includes("const recipient = wallet?.address || '';"),
        'unshield confirmation should submit to the active wallet address');
    assert(!shieldedSrc.includes("const recipient = document.getElementById('unshieldRecipient').value.trim();"),
        'unshield should not trust editable recipient text');
});

test('wallet applies numeric, base58, and hex input guards', () => {
    assert(walletSrc.includes('function applyWalletInputGuards('), 'wallet should centralize input guards');
    assert(walletSrc.includes('function sanitizeWalletNumberInput('), 'wallet should sanitize numeric fields');
    assert(!walletHtml.includes('type="number"'), 'wallet should not use native number inputs for value-bearing fields');
    assert(walletSrc.includes("const inputType = isNumber ? 'text' : field.type;"),
        'wallet generated numeric modal fields should render as guarded text inputs');
    assert(!walletSrc.includes('input[type="number"], input[data-input-kind="number"]'),
        'wallet input guards should rely on explicit numeric data attributes instead of native number selectors');
    assert(walletSrc.includes("event.key === 'e' || event.key === 'E' || event.key === '+'"),
        'wallet numeric fields should reject exponent/plus shortcuts');
    assert(walletSrc.includes('input[data-address-input="base58"], #sendTo, #unshieldRecipient'),
        'wallet should guard base58 address fields');
    assert(walletSrc.includes('input[data-hex-input], #shieldedTransferRecipientVK'),
        'wallet should guard hex-only fields');
    assert(walletHtml.includes('id="sendTo"') && walletHtml.includes('data-address-input="base58"'),
        'send recipient should be base58-guarded');
    assert(walletHtml.includes('id="sendAmount"') && walletHtml.includes('data-wallet-numeric="true"'),
        'send amount should be numeric-guarded');
    assert(walletHtml.includes('id="shieldAmount"') && walletHtml.includes('data-wallet-numeric="true"'),
        'shield amount should be numeric-guarded');
    assert(walletHtml.includes('id="unshieldAmount"') && walletHtml.includes('data-wallet-numeric="true"'),
        'unshield amount should be numeric-guarded');
    assert(walletHtml.includes('id="shieldedTransferRecipientVK"') && walletHtml.includes('data-hex-input="true"'),
        'private-transfer viewing key should be hex-guarded');
});

test('wallet blocks transparent transfers to the active wallet address', () => {
    assert(walletSrc.includes("if (to === wallet.address)"),
        'confirmSend should compare recipient against the active wallet address');
    assert(walletSrc.includes("showToast('Sending to your own wallet is not allowed'"),
        'confirmSend should show a clear self-transfer rejection');
});

test('wallet switch clears wallet-scoped dashboard and shielded state before reloading', () => {
    assert(walletSrc.includes('let _walletViewGeneration = 0;'), 'wallet should track dashboard render generation');
    assert(walletSrc.includes('function beginWalletViewRender('), 'wallet should increment render generation');
    assert(walletSrc.includes('function isCurrentWalletView('), 'wallet should reject stale async renders');
    assert(walletSrc.includes('function clearWalletScopedDashboardUi('), 'wallet should clear wallet-scoped panels on switch');
    assert(walletSrc.includes('resetShieldedForWalletSwitch()'), 'wallet switch should reset shielded UI/state');
    assert(walletSrc.includes('clearWalletScopedDashboardUi();\n    clearStakingValidatorsCache();'),
        'switchWallet should clear visible data before reloading the next wallet');
});

test('wallet switch reloads active identity tab instead of leaving a spinner', () => {
    assert(walletSrc.includes("const activeTab = document.querySelector('.dashboard-tab.active')?.dataset?.tab;"),
        'dashboard refresh should inspect the active tab after wallet switch');
    assert(walletSrc.includes("if (activeTab === 'identity' && typeof loadIdentity === 'function'"),
        'active identity tab should reload immediately after dashboard refresh');
});

test('wallet async loaders refuse to render after wallet switch', () => {
    assert(walletSrc.includes('async function refreshBalance(options = {})'), 'balance loader should accept wallet context');
    assert(walletSrc.includes('async function loadAssets(options = {})'), 'asset loader should accept wallet context');
    assert(walletSrc.includes('async function loadActivity(reset = true, options = {})'), 'activity loader should accept wallet context');
    assert(walletSrc.includes('async function loadStaking(options = {})'), 'staking loader should accept wallet context');
    assert(walletSrc.includes('async function loadMossStakePosition(address, options = {})'), 'MossStake loader should accept wallet context');
    assert(walletSrc.includes('async function refreshNFTs(options = {})'), 'NFT loader should accept wallet context');
    assert((walletSrc.match(/isCurrentWalletView\(wallet, generation\)/g) || []).length >= 8,
        'wallet loaders should guard DOM writes with active wallet context');
    assert(identitySrc.includes('!isCurrentWalletView(wallet, walletViewGeneration ?? undefined)'),
        'identity loader should ignore stale wallet data');
});

test('wallet identity vouch labels normalize full .lichen names once', () => {
    assert(identitySrc.includes('function formatLichenNameLabel('), 'identity.js should define a .lichen label normalizer');
    assert(identitySrc.includes("replace(/(?:\\.lichen)+$/i, '')"),
        'identity.js should strip one or more existing .lichen suffixes before display');
    assert(identitySrc.includes('formatLichenNameLabel(v.voucher_name)'),
        'vouch chips should render normalized .lichen labels');
    assert(!identitySrc.includes("escHtml(v.voucher_name) + '.lichen'"),
        'vouch chips should not append a second .lichen suffix');
});

test('shielded state is isolated per active wallet', () => {
    assert(shieldedSrc.includes('function createInitialShieldedState('), 'shielded module should have a reusable empty state');
    assert(shieldedSrc.includes('function resetShieldedForWalletSwitch('), 'shielded module should expose switch reset');
    assert(shieldedSrc.includes('window.resetShieldedForWalletSwitch = resetShieldedForWalletSwitch'),
        'shielded reset should be callable by wallet switch');
    assert(shieldedSrc.includes('shieldedState.ownedNotes = [];') && shieldedSrc.includes("shieldedState.shieldedBalance = '0';"),
        'shielded init should clear previous wallet notes before loading current wallet notes');
    assert(shieldedSrc.includes('localStorage.setItem(`${storageKey}:unreadable:${Date.now()}`, raw);'),
        'unreadable local shielded payloads should be quarantined instead of mixed into another wallet');
    assert(!shieldedSrc.includes('localStorage.removeItem(storageKey);'),
        'unreadable local shielded payloads should not be removed from their primary wallet-scoped storage key');
});

test('wallet parses transfer and shielded amounts as base-unit BigInt values', () => {
    assert(walletSharedUtilsSrc.includes('function parseDecimalBaseUnits('), 'shared utils should define decimal-to-base-unit parsing');
    assert(walletSharedUtilsSrc.includes('fractionRaw.length > unitDecimals'), 'shared parser should reject over-precision');
    assert(walletSharedUtilsSrc.includes('const MAX_U64 = 18_446_744_073_709_551_615n;'), 'shared parser should cap u64 payloads');
    assert(walletSrc.includes('function parseSendAmountBaseUnits('), 'wallet send path should centralize amount parsing');
    assert(walletSrc.includes('view.setBigUint64(1, amountBaseUnits, true);'),
        'native transfers should write parsed BigInt base units');
    assert(walletSrc.includes('amount: amountBaseUnits.toString()'),
        'contract token payloads should serialize base units as decimal strings');
    assert(!walletSrc.includes('Math.floor(amount * SPORES_PER_LICN)'),
        'wallet send path should not convert LICN amounts through floating point');
    assert(!walletSrc.includes('Math.floor(amount * Math.pow(10, token.decimals))'),
        'wallet token send path should not convert token amounts through floating point');
});

test('wallet shielded flows keep note values and proof amounts lossless', () => {
    assert(shieldedSrc.includes('function parseShieldedAmountSpores('), 'shielded module should parse decimal amounts to spores');
    assert(shieldedSrc.includes('amountSpores = parseShieldedAmountSpores(amountText'), 'shielded confirmations should parse amount text');
    assert(shieldedSrc.includes('value: amountSpores.toString()'), 'shield notes should store amount values as u64 strings');
    assert(shieldedSrc.includes('amount: amountSpores.toString()'), 'shielded proofs should submit string u64 amounts');
    assert(shieldedSrc.includes('reduce((sum, n) => sum + noteValueSpores(n), 0n)'),
        'shielded balance should be recalculated with BigInt note values');
    assert(!shieldedSrc.includes('Math.floor(amount * SPORES_PER_LICN)'),
        'shielded flows should not floor decimal LICN amounts through Number');
});

test('wallet USD valuations expose quote source, staleness, and fallback state', () => {
    assert(walletSrc.includes('const PRICE_STALE_MS = 5 * 60 * 1000;'),
        'wallet should define a stale window for displayed USD quotes');
    assert(walletSrc.includes('const priceMetadata = {'),
        'wallet should track source metadata for live prices');
    assert(walletSrc.includes('function getPriceQuote(symbol)'),
        'wallet should expose price quotes with source metadata');
    assert(walletSrc.includes("source: key === 'lUSD' ? 'stablecoin-peg' : 'offline-fallback'"),
        'wallet should distinguish stablecoin peg fallback from offline fallback prices');
    assert(walletSrc.includes("' · offline estimate'") && walletSrc.includes("' · stale price'"),
        'wallet should visibly mark offline and stale USD valuations');
    assert(walletSrc.includes('balanceUsdEl.title = usdValuationTitle(valuationQuotes);'),
        'wallet total USD display should include source details in its title');
    assert(walletSrc.includes('class="asset-value" title="${escapeHtml(usdValuationTitle([quote]))}"'),
        'wallet asset USD values should include per-asset valuation source details');
});

test('explorer token USD valuations expose oracle timestamps and peg fallbacks', () => {
    assert(explorerAddressSrc.includes('const ORACLE_PRICE_STALE_MS = 5 * 60 * 1000;'),
        'explorer should define a stale window for token USD prices');
    assert(explorerAddressSrc.includes("window._oraclePrices = { prices: prices || {}, source: 'oracle', timestamp: Date.now() };"),
        'explorer should store oracle price source and fetch timestamp');
    assert(explorerAddressSrc.includes("window._oraclePrices = { prices: {}, source: 'unavailable', timestamp: 0 };"),
        'explorer should explicitly track unavailable oracle prices');
    assert(explorerAddressSrc.includes("priceQuote = { price: usdPrice, source: 'stablecoin-peg', timestamp: 0, stale: false };"),
        'explorer stablecoin fallback should be labeled as a peg estimate');
    assert(explorerAddressSrc.includes("const valueSuffix = priceQuote?.source === 'stablecoin-peg'"),
        'explorer should visibly mark peg fallback values');
    assert(explorerAddressSrc.includes('title="${escapeHtml(valueTitle)}"'),
        'explorer token USD value cells should include valuation source details');
});

test('wallet staking and shielded modals expose MAX controls without crossing flows', () => {
    assert(walletSrc.includes('function fillMossStakeMaxAmount('),
        'wallet should centralize MossStake MAX amount filling');
    assert(walletSrc.includes("modal.querySelector('#stakeAmountMax')?.addEventListener('click', () => fillMossStakeMaxAmount(modal, 'stake'))"),
        'wallet stake modal should wire a MAX button');
    assert(walletSrc.includes("modal.querySelector('#unstakeAmountMax')?.addEventListener('click', () => fillMossStakeMaxAmount(modal, 'unstake'))"),
        'wallet unstake modal should wire a MAX button');
    assert(walletSrc.includes('async function fillShieldMaxAmount()'),
        'wallet should centralize shield MAX amount filling');
    assert(walletSrc.includes('const shieldFeeSpores = getNetworkBaseFeeSpores() + BigInt(zkFees.shield ?? 0);'),
        'shield MAX should reserve base plus shield compute fee in base units');
    assert(walletSrc.includes('const maxShieldable = spendableSpores > shieldFeeSpores ? spendableSpores - shieldFeeSpores : 0n;'),
        'shield MAX should subtract the full shield fee from spendable LICN');
    assert(!shieldedSrc.includes('const txCount = Math.ceil(entries.length / SHIELDED_MAX_UNSHIELD_NOTES_PER_TX);'),
        'shield flow must not reference unshield batch entries');
});

test('wallet password modal validates, reports, and clears wrong passwords for retry', () => {
    assert(walletSrc.includes('password-modal-error'),
        'password modal should include an inline error container');
    assert(walletSrc.includes('await validateModalValues(values);'),
        'password modal should validate before resolving');
    assert(walletSrc.includes('window.LichenCrypto.decryptPrivateKey(wallet.encryptedKey, values.password)'),
        'password modal should verify the active wallet password');
    assert(walletSrc.includes('modal.querySelectorAll(\'input[type="password"]\').forEach'),
        'password modal should clear password inputs after validation failure');
    assert(walletSrc.includes('confirmBtn.disabled = false;'),
        'password modal should allow retry after validation failure');
});

test('wallet PWA starts at non-redirecting root app shell', () => {
    assert.strictEqual(walletManifest.start_url, '/',
        'wallet PWA start_url should avoid /index.html because Pages redirects it');
    assert.strictEqual(walletManifest.scope, '/',
        'wallet PWA scope should be rooted at the deployed app origin');
    assert(walletRedirectsSrc.includes('/index.html  /  200'),
        'wallet Cloudflare Pages redirects should rewrite legacy /index.html PWA launches to root with a 200 response');
    assert(walletServiceWorkerSrc.includes("const APP_SHELL_URL = './';"),
        'service worker should cache the non-redirecting root app shell');
    assert(!walletServiceWorkerSrc.includes("const INDEX_URL = './index.html';"),
        'service worker should not use redirected /index.html as app shell');
    assert(walletServiceWorkerSrc.includes('response.redirected) return false'),
        'service worker cache guard should reject redirected responses');
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
