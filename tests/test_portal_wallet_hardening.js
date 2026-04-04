// Explorer and portal wallet hardening checks
// Run: node tests/test_portal_wallet_hardening.js

const fs = require('fs');
const path = require('path');

let passed = 0;
let failed = 0;

function assert(condition, label) {
    if (condition) {
        passed++;
        console.log(`  ✅ ${label}`);
    } else {
        failed++;
        console.log(`  ❌ ${label}`);
    }
}

const root = path.join(__dirname, '..');
const explorerAddressSrc = fs.readFileSync(path.join(root, 'explorer', 'js', 'address.js'), 'utf8');
const explorerHelperSrc = fs.readFileSync(path.join(root, 'explorer', 'shared', 'wallet-connect.js'), 'utf8');
const faucetHelperSrc = fs.readFileSync(path.join(root, 'faucet', 'shared', 'wallet-connect.js'), 'utf8');
const developersHelperSrc = fs.readFileSync(path.join(root, 'developers', 'shared', 'wallet-connect.js'), 'utf8');
const monitoringHelperSrc = fs.readFileSync(path.join(root, 'monitoring', 'shared', 'wallet-connect.js'), 'utf8');

console.log('\n── Explorer Address Hardening ──');

assert(!explorerAddressSrc.includes('getLocalWalletSession'), 'EXP-1 explorer local wallet session helper removed');
assert(!explorerAddressSrc.includes('requestWalletPassword('), 'EXP-2 explorer wallet password modal removed');
assert(!explorerAddressSrc.includes('requestWalletPasswordIfNeeded'), 'EXP-3 explorer password gating helper removed');
assert(!explorerAddressSrc.includes('wallet.mode === \'local\''), 'EXP-4 explorer local wallet branch removed');
assert(!explorerAddressSrc.includes('LichenCrypto.decryptPrivateKey(wallet.encryptedKey, password)'), 'EXP-5 explorer no longer decrypts browser-stored private keys for sends');
assert(!explorerAddressSrc.includes('bytesToBase64('), 'EXP-6 explorer no longer serializes locally signed transactions');
assert(explorerAddressSrc.includes('Explorer only supports extension-backed signing.'), 'EXP-7 explorer signing now fails closed to extension-only');
assert(explorerAddressSrc.includes('provider.sendTransaction'), 'EXP-8 explorer delegated signing uses injected provider');
assert(explorerAddressSrc.includes('Connect the extension to continue.'), 'EXP-9 explorer wallet prompt message points only to the extension');
assert(explorerAddressSrc.includes('await signAndSendInstructions(wallet, [instruction]);'), 'EXP-10 explorer action modals use extension-only signing helper');

console.log('\n── Shared Helper Hardening ──');

[
    ['Explorer', explorerHelperSrc],
    ['Faucet', faucetHelperSrc],
    ['Monitoring', monitoringHelperSrc],
].forEach(([label, source]) => {
    const prefix = label.toUpperCase();
    assert(source.includes('extensionOnlyWalletError'), `${prefix}-1 ${label} helper exposes extension-only error helper`);
    assert(!source.includes('Lichen.Wallet.import('), `${prefix}-2 ${label} helper no longer imports local wallets`);
    assert(!source.includes('Lichen.Wallet.create('), `${prefix}-3 ${label} helper no longer creates local wallets`);
    assert(!source.includes("lichenRpcCall('createWallet'"), `${prefix}-4 ${label} helper no longer falls back to RPC wallet creation`);
    assert(!source.includes('window.LichenPQ'), `${prefix}-5 ${label} helper no longer falls back to PQ generation`);
    assert(source.includes("data.provider !== 'extension'"), `${prefix}-6 ${label} helper purges persisted non-extension wallet state`);
    assert(source.includes('toggle().catch('), `${prefix}-7 ${label} helper catches failed extension connect actions`);
});

assert(developersHelperSrc.includes('extensionOnlyWalletError'), 'DEVELOPERS-1 Developers helper exposes extension-only error helper');
assert(!developersHelperSrc.includes('Lichen.Wallet.import('), 'DEVELOPERS-2 Developers helper no longer imports local SDK wallets');
assert(!developersHelperSrc.includes('Lichen.Wallet.create('), 'DEVELOPERS-3 Developers helper no longer creates local SDK wallets');
assert(!developersHelperSrc.includes("lichenRpcCall('createWallet'"), 'DEVELOPERS-4 Developers helper no longer attempts RPC wallet creation');
assert(!developersHelperSrc.includes('window.LichenPQ.generateKeypair'), 'DEVELOPERS-5 Developers helper no longer falls back to the PQ runtime');
assert(developersHelperSrc.includes("data.provider && data.provider !== 'extension'"), 'DEVELOPERS-6 Developers helper purges persisted non-extension wallet providers');
assert(developersHelperSrc.includes('toggle().catch('), 'DEVELOPERS-7 Developers helper catches failed extension connect actions');

console.log(`\n${'═'.repeat(50)}`);
console.log(`Portal Wallet Hardening: ${passed} passed, ${failed} failed (${passed + failed} total)`);
console.log(`${'═'.repeat(50)}`);
process.exit(failed > 0 ? 1 : 0);