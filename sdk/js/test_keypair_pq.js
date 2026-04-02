// Smoke test for the native PQ JavaScript SDK surface.
const assert = require('assert');
const {
    Keypair,
    ML_DSA_65_PUBLIC_KEY_BYTES,
    ML_DSA_65_SIGNATURE_BYTES,
    PqSignature,
    PQ_SCHEME_ML_DSA_65,
} = require('./dist');

const seed = Uint8Array.from(Array.from({ length: 32 }, (_, i) => i));
const keypair = Keypair.fromSeed(seed);
const twin = Keypair.fromSeed(seed);

assert.strictEqual(keypair.publicKey.schemeVersion, PQ_SCHEME_ML_DSA_65);
assert.strictEqual(keypair.publicKey.toBytes().length, ML_DSA_65_PUBLIC_KEY_BYTES);
assert(keypair.publicKey.equals(twin.publicKey));
assert.deepStrictEqual(Array.from(keypair.pubkey().toBytes()), Array.from(twin.pubkey().toBytes()));

const message = Uint8Array.from([1, 2, 3, 4]);
const signature = keypair.sign(message);

assert(signature instanceof PqSignature);
assert.strictEqual(signature.schemeVersion, PQ_SCHEME_ML_DSA_65);
assert.strictEqual(signature.toBytes().length, ML_DSA_65_SIGNATURE_BYTES);
assert(Keypair.verify(keypair.pubkey(), message, signature));
assert(!Keypair.verify(Keypair.generate().pubkey(), message, signature));

console.log('All JS keypair PQ tests passed!');