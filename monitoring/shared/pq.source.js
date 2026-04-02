import { ml_dsa65 } from '@noble/post-quantum/ml-dsa.js';

export const PQ_SCHEME_ML_DSA_65 = 0x01;
export const ML_DSA_65_SEED_BYTES = 32;
export const ML_DSA_65_PUBLIC_KEY_BYTES = 1952;
export const ML_DSA_65_SIGNATURE_BYTES = 3309;

const BASE58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

function copyBytes(bytes) {
    return new Uint8Array(bytes);
}

function ensureCrypto() {
    if (!globalThis.crypto || !globalThis.crypto.subtle) {
        throw new Error('Web Crypto API is unavailable');
    }

    return globalThis.crypto;
}

function normalizeHex(hex) {
    const clean = String(hex || '').trim().replace(/^0x/, '');
    if (clean.length === 0 || clean.length % 2 !== 0 || !/^[0-9a-fA-F]+$/.test(clean)) {
        throw new Error('Invalid hex string');
    }
    return clean.toLowerCase();
}

function normalizeBytes(value, label) {
    if (value instanceof Uint8Array) {
        return copyBytes(value);
    }

    if (Array.isArray(value)) {
        return new Uint8Array(value);
    }

    if (typeof value === 'string') {
        return hexToBytes(value);
    }

    throw new Error(`Invalid ${label}`);
}

function validateSeed(seedBytes) {
    if (seedBytes.length !== ML_DSA_65_SEED_BYTES) {
        throw new Error(`Invalid ML-DSA-65 seed length: ${seedBytes.length}`);
    }
}

function validatePublicKey(publicKeyBytes) {
    if (publicKeyBytes.length !== ML_DSA_65_PUBLIC_KEY_BYTES) {
        throw new Error(`Invalid ML-DSA-65 public key length: ${publicKeyBytes.length}`);
    }
}

function validateSignature(signatureBytes) {
    if (signatureBytes.length !== ML_DSA_65_SIGNATURE_BYTES) {
        throw new Error(`Invalid ML-DSA-65 signature length: ${signatureBytes.length}`);
    }
}

export function bytesToHex(bytes) {
    return Array.from(bytes)
        .map((byte) => byte.toString(16).padStart(2, '0'))
        .join('');
}

export function hexToBytes(hex) {
    const clean = normalizeHex(hex);
    const bytes = new Uint8Array(clean.length / 2);
    for (let index = 0; index < bytes.length; index++) {
        bytes[index] = parseInt(clean.slice(index * 2, index * 2 + 2), 16);
    }
    return bytes;
}

export function base58Encode(buffer) {
    if (!buffer || buffer.length === 0) return '';

    const digits = [0];
    for (let index = 0; index < buffer.length; index++) {
        let carry = buffer[index];
        for (let digitIndex = 0; digitIndex < digits.length; digitIndex++) {
            carry += digits[digitIndex] << 8;
            digits[digitIndex] = carry % 58;
            carry = (carry / 58) | 0;
        }
        while (carry > 0) {
            digits.push(carry % 58);
            carry = (carry / 58) | 0;
        }
    }

    let output = '';
    for (let index = 0; buffer[index] === 0 && index < buffer.length - 1; index++) {
        output += BASE58_ALPHABET[0];
    }
    for (let index = digits.length - 1; index >= 0; index--) {
        output += BASE58_ALPHABET[digits[index]];
    }
    return output;
}

export function base58Decode(value) {
    const stringValue = String(value || '');
    if (stringValue.length === 0) {
        return new Uint8Array(0);
    }

    const bytes = [0];
    for (let index = 0; index < stringValue.length; index++) {
        const alphabetIndex = BASE58_ALPHABET.indexOf(stringValue[index]);
        if (alphabetIndex === -1) {
            throw new Error(`Invalid base58 character: ${stringValue[index]}`);
        }

        let carry = alphabetIndex;
        for (let byteIndex = 0; byteIndex < bytes.length; byteIndex++) {
            carry += bytes[byteIndex] * 58;
            bytes[byteIndex] = carry & 0xff;
            carry >>= 8;
        }
        while (carry > 0) {
            bytes.push(carry & 0xff);
            carry >>= 8;
        }
    }

    for (let index = 0; stringValue[index] === BASE58_ALPHABET[0] && index < stringValue.length - 1; index++) {
        bytes.push(0);
    }

    return new Uint8Array(bytes.reverse());
}

export function generateSeed() {
    const seed = new Uint8Array(ML_DSA_65_SEED_BYTES);
    ensureCrypto().getRandomValues(seed);
    return seed;
}

export function derivePublicKey(seedLike) {
    const seedBytes = normalizeBytes(seedLike, 'seed');
    validateSeed(seedBytes);
    const keypair = ml_dsa65.keygen(seedBytes);
    return new Uint8Array(keypair.publicKey);
}

export async function publicKeyToAddressBytes(publicKeyLike, schemeVersion = PQ_SCHEME_ML_DSA_65) {
    if (schemeVersion !== PQ_SCHEME_ML_DSA_65) {
        throw new Error(`Unsupported PQ scheme version: ${schemeVersion}`);
    }

    const publicKeyBytes = normalizeBytes(publicKeyLike, 'public key');
    validatePublicKey(publicKeyBytes);
    const digest = new Uint8Array(await ensureCrypto().subtle.digest('SHA-256', publicKeyBytes));
    const addressBytes = new Uint8Array(32);
    addressBytes[0] = schemeVersion;
    addressBytes.set(digest.subarray(0, 31), 1);
    return addressBytes;
}

export async function publicKeyToAddress(publicKeyLike, schemeVersion = PQ_SCHEME_ML_DSA_65) {
    return base58Encode(await publicKeyToAddressBytes(publicKeyLike, schemeVersion));
}

export function addressToBytes(address) {
    const bytes = base58Decode(address);
    if (bytes.length !== 32) {
        throw new Error(`Invalid address length: ${bytes.length}`);
    }
    if (bytes[0] !== PQ_SCHEME_ML_DSA_65) {
        throw new Error(`Unsupported address scheme version: 0x${bytes[0].toString(16).padStart(2, '0')}`);
    }
    return bytes;
}

export function isValidAddress(address) {
    try {
        addressToBytes(address);
        return true;
    } catch {
        return false;
    }
}

export async function keypairFromSeed(seedLike) {
    const seedBytes = normalizeBytes(seedLike, 'seed');
    validateSeed(seedBytes);
    const keypair = ml_dsa65.keygen(seedBytes);
    const publicKey = new Uint8Array(keypair.publicKey);
    const address = await publicKeyToAddress(publicKey);
    if (keypair.secretKey && typeof keypair.secretKey.fill === 'function') {
        keypair.secretKey.fill(0);
    }
    return {
        schemeVersion: PQ_SCHEME_ML_DSA_65,
        privateKey: bytesToHex(seedBytes),
        publicKey,
        publicKeyHex: bytesToHex(publicKey),
        address,
    };
}

export async function generateKeypair() {
    return keypairFromSeed(generateSeed());
}

export function normalizeSignature(signature) {
    if (!signature || typeof signature !== 'object') {
        throw new Error('Invalid PQ signature');
    }

    const schemeVersion = Number(signature.scheme_version ?? signature.schemeVersion ?? PQ_SCHEME_ML_DSA_65);
    if (schemeVersion !== PQ_SCHEME_ML_DSA_65) {
        throw new Error(`Unsupported PQ signature scheme: 0x${schemeVersion.toString(16).padStart(2, '0')}`);
    }

    const publicKey = signature.public_key ?? signature.publicKey;
    if (!publicKey || typeof publicKey !== 'object') {
        throw new Error('PQ signature is missing the verifying key');
    }

    const publicKeyScheme = Number(publicKey.scheme_version ?? publicKey.schemeVersion ?? schemeVersion);
    if (publicKeyScheme !== schemeVersion) {
        throw new Error('PQ signature/public-key scheme mismatch');
    }

    const publicKeyBytes = normalizeBytes(publicKey.bytes, 'public key');
    validatePublicKey(publicKeyBytes);
    const signatureBytes = normalizeBytes(signature.sig, 'signature');
    validateSignature(signatureBytes);

    return {
        scheme_version: schemeVersion,
        public_key: {
            scheme_version: publicKeyScheme,
            bytes: bytesToHex(publicKeyBytes),
        },
        sig: bytesToHex(signatureBytes),
    };
}

export async function signMessage(seedHexOrBytes, messageBytes) {
    const seedBytes = normalizeBytes(seedHexOrBytes, 'seed');
    validateSeed(seedBytes);
    const payload = messageBytes instanceof Uint8Array ? copyBytes(messageBytes) : new Uint8Array(messageBytes || []);
    const keypair = ml_dsa65.keygen(seedBytes);
    const publicKeyBytes = new Uint8Array(keypair.publicKey);
    const signatureBytes = new Uint8Array(ml_dsa65.sign(payload, keypair.secretKey));
    const signature = normalizeSignature({
        scheme_version: PQ_SCHEME_ML_DSA_65,
        public_key: {
            scheme_version: PQ_SCHEME_ML_DSA_65,
            bytes: publicKeyBytes,
        },
        sig: signatureBytes,
    });
    if (keypair.secretKey && typeof keypair.secretKey.fill === 'function') {
        keypair.secretKey.fill(0);
    }
    seedBytes.fill(0);
    return signature;
}

export async function verifySignature(signature, messageBytes, expectedAddress) {
    const normalized = normalizeSignature(signature);
    const payload = messageBytes instanceof Uint8Array ? copyBytes(messageBytes) : new Uint8Array(messageBytes || []);
    const publicKeyBytes = hexToBytes(normalized.public_key.bytes);
    const signatureBytes = hexToBytes(normalized.sig);
    const isValid = ml_dsa65.verify(signatureBytes, payload, publicKeyBytes);
    if (!isValid) {
        return false;
    }

    if (expectedAddress) {
        const signerAddress = await publicKeyToAddress(publicKeyBytes, normalized.scheme_version);
        return signerAddress === expectedAddress;
    }

    return true;
}

const LichenPQ = {
    PQ_SCHEME_ML_DSA_65,
    ML_DSA_65_SEED_BYTES,
    ML_DSA_65_PUBLIC_KEY_BYTES,
    ML_DSA_65_SIGNATURE_BYTES,
    bytesToHex,
    hexToBytes,
    base58Encode,
    base58Decode,
    generateSeed,
    derivePublicKey,
    publicKeyToAddressBytes,
    publicKeyToAddress,
    addressToBytes,
    isValidAddress,
    keypairFromSeed,
    generateKeypair,
    normalizeSignature,
    signMessage,
    verifySignature,
};

if (typeof window !== 'undefined') {
    window.LichenPQ = LichenPQ;
}

export default LichenPQ;