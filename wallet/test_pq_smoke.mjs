import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { webcrypto } from 'node:crypto';

import * as pq from '../monitoring/shared/pq.js';
import * as extCrypto from './extension/src/core/crypto-service.js';
import { buildSignedNativeTransferTransaction } from './extension/src/core/tx-service.js';

globalThis.window = globalThis;
if (!globalThis.crypto) {
    Object.defineProperty(globalThis, 'crypto', {
        value: webcrypto,
        configurable: true,
    });
}
globalThis.LichenPQ = pq;
globalThis.bs58 = {
    encode: pq.base58Encode,
    decode: pq.base58Decode,
};

const browserCryptoSource = fs.readFileSync(path.join(process.cwd(), 'wallet/js/crypto.js'), 'utf8');
vm.runInThisContext(browserCryptoSource, { filename: './wallet/js/crypto.js' });

const seed = Uint8Array.from(Array.from({ length: 32 }, (_, index) => index));
const sharedKeypair = await pq.keypairFromSeed(seed);
assert.equal(sharedKeypair.publicKey.length, pq.ML_DSA_65_PUBLIC_KEY_BYTES);
assert.equal(pq.addressToBytes(sharedKeypair.address).length, 32);

const message = Uint8Array.from([1, 2, 3, 4, 5]);
const sharedSignature = await pq.signMessage(sharedKeypair.privateKey, message);
assert.equal(sharedSignature.scheme_version, pq.PQ_SCHEME_ML_DSA_65);
assert.equal(sharedSignature.public_key.scheme_version, pq.PQ_SCHEME_ML_DSA_65);
assert.ok(await pq.verifySignature(sharedSignature, message, sharedKeypair.address));

const mnemonic = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
const browserKeypair = await globalThis.LichenCrypto.mnemonicToKeypair(mnemonic);
assert.ok(globalThis.LichenCrypto.isValidAddress(browserKeypair.address));
const browserSignature = await globalThis.LichenCrypto.signTransaction(browserKeypair.privateKey, message);
assert.equal(browserSignature.scheme_version, pq.PQ_SCHEME_ML_DSA_65);
assert.ok(
    await globalThis.LichenCrypto.verifySignature(
        browserSignature,
        message,
        globalThis.LichenCrypto.hexToBytes(browserKeypair.publicKey),
    ),
);

const extensionKeypair = await extCrypto.privateKeyToKeypair(sharedKeypair.privateKey);
assert.equal(extensionKeypair.address, sharedKeypair.address);
const extensionSignature = await extCrypto.signTransaction(sharedKeypair.privateKey, message);
assert.equal(extensionSignature.scheme_version, pq.PQ_SCHEME_ML_DSA_65);

const transaction = await buildSignedNativeTransferTransaction({
    privateKeyHex: sharedKeypair.privateKey,
    fromAddress: sharedKeypair.address,
    toAddress: sharedKeypair.address,
    amountLicn: 1,
    blockhash: '00'.repeat(32),
});
assert.equal(transaction.signatures.length, 1);
assert.equal(transaction.signatures[0].scheme_version, pq.PQ_SCHEME_ML_DSA_65);
assert.equal(transaction.message.instructions[0].accounts[0].length, 32);
assert.equal(transaction.message.instructions[0].accounts[1].length, 32);

console.log('wallet-pq-smoke: ok');