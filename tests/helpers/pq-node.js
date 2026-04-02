'use strict';
/**
 * Node.js CJS-compatible ML-DSA-65 wrapper.
 *
 * Usage (async):
 *   const pq = require('./pq-node');
 *   await pq.init();
 *   const kp  = pq.generateKeypair();
 *   const sig = pq.sign(messageBytes, kp);
 *   const ok  = pq.verify(messageBytes, sig, kp.publicKey);
 *
 * Keypair shape:
 *   { seed: Uint8Array(32), publicKey: Uint8Array(1952), address: string }
 *
 * PqSignature shape (JSON-wire): matches RPC parse_pq_signature_value
 *   { scheme_version: 1, public_key: { scheme_version: 1, bytes: <hex> }, sig: <hex> }
 */

const { createHash, randomBytes } = require('crypto');

const PQ_SCHEME_ML_DSA_65 = 0x01;
const ML_DSA_65_SEED_BYTES = 32;
const ML_DSA_65_PUBLIC_KEY_BYTES = 1952;
const ML_DSA_65_SIGNATURE_BYTES = 3309;

const BS58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

let _mlDsa65 = null;

function bs58encode(bytes) {
    let lz = 0;
    for (let i = 0; i < bytes.length && bytes[i] === 0; i++) lz++;
    let num = 0n;
    for (const b of bytes) num = num * 256n + BigInt(b);
    let enc = '';
    while (num > 0n) { enc = BS58[Number(num % 58n)] + enc; num /= 58n; }
    return '1'.repeat(lz) + enc;
}

function bytesToHex(b) {
    return Array.from(b).map(x => x.toString(16).padStart(2, '0')).join('');
}

function hexToBytes(h) {
    const c = h.startsWith('0x') ? h.slice(2) : h;
    const o = new Uint8Array(c.length / 2);
    for (let i = 0; i < o.length; i++) o[i] = parseInt(c.slice(i * 2, i * 2 + 2), 16);
    return o;
}

/** Derive the 32-byte Lichen address from a 1952-byte ML-DSA-65 public key. */
function publicKeyToAddressBytes(publicKey) {
    const hash = createHash('sha256').update(publicKey).digest();
    const addr = Buffer.alloc(32);
    addr[0] = PQ_SCHEME_ML_DSA_65;
    hash.copy(addr, 1, 0, 31);
    return new Uint8Array(addr);
}

/** Load @noble/post-quantum ML-DSA-65 (once). Must be called before first use. */
async function init() {
    if (_mlDsa65) return;
    const mod = await import('@noble/post-quantum/ml-dsa.js');
    _mlDsa65 = mod.ml_dsa65;
}

function _requireInit() {
    if (!_mlDsa65) throw new Error('pq-node: call await pq.init() before using crypto functions');
}

/** Generate a fresh ML-DSA-65 keypair from random seed. */
function generateKeypair() {
    _requireInit();
    const seed = new Uint8Array(randomBytes(ML_DSA_65_SEED_BYTES));
    return keypairFromSeed(seed);
}

/** Derive ML-DSA-65 keypair from a 32-byte seed. */
function keypairFromSeed(seed) {
    _requireInit();
    const kp = _mlDsa65.keygen(seed);
    const publicKey = new Uint8Array(kp.publicKey);
    const addrBytes = publicKeyToAddressBytes(publicKey);
    return {
        seed: new Uint8Array(seed),
        publicKey,
        address: bs58encode(addrBytes),
    };
}

/**
 * Sign message bytes; returns a PqSignature JSON object ready for the RPC wire.
 * @param {Uint8Array} messageBytes
 * @param {{ seed: Uint8Array, publicKey: Uint8Array }} keypair
 */
function sign(messageBytes, keypair) {
    _requireInit();
    const kp = _mlDsa65.keygen(keypair.seed);
    const sigBytes = new Uint8Array(_mlDsa65.sign(messageBytes, kp.secretKey));
    return buildPqSignature(keypair.publicKey, sigBytes);
}

/**
 * Verify a PqSignature JSON object against message bytes and public key.
 */
function verify(messageBytes, pqSig, publicKey) {
    _requireInit();
    const sigBytes = hexToBytes(pqSig.sig);
    const pkBytes = hexToBytes(pqSig.public_key.bytes);
    return _mlDsa65.verify(sigBytes, messageBytes, pkBytes);
}

/** Build the PqSignature JSON wire format. */
function buildPqSignature(publicKey, sigBytes) {
    return {
        scheme_version: PQ_SCHEME_ML_DSA_65,
        public_key: {
            scheme_version: PQ_SCHEME_ML_DSA_65,
            bytes: bytesToHex(publicKey),
        },
        sig: bytesToHex(sigBytes),
    };
}

module.exports = {
    PQ_SCHEME_ML_DSA_65,
    ML_DSA_65_SEED_BYTES,
    ML_DSA_65_PUBLIC_KEY_BYTES,
    ML_DSA_65_SIGNATURE_BYTES,
    init,
    generateKeypair,
    keypairFromSeed,
    sign,
    verify,
    buildPqSignature,
    publicKeyToAddressBytes,
    bs58encode,
    bytesToHex,
    hexToBytes,
};
