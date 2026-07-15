'use strict';

const { bs58decode } = require('./funded-wallets');

const TX_WIRE_MAGIC = Uint8Array.of(0x4d, 0x54);
const TX_WIRE_VERSION = 1;
const TX_TYPE_NATIVE = 0;
const SIGNING_ENVELOPE_MAGIC = Uint8Array.from(Buffer.from('LICHEN-SIG', 'ascii'));
const SIGNING_ENVELOPE_VERSION = 1;
const NATIVE_TX_DOMAIN = 'native-tx';
const DEFAULT_TEST_CHAIN_ID = 'lichen-testnet-1';

function concat(parts) {
    const size = parts.reduce((total, part) => total + part.length, 0);
    const output = new Uint8Array(size);
    let offset = 0;
    for (const part of parts) {
        output.set(part, offset);
        offset += part.length;
    }
    return output;
}

function encodeU64(value, field) {
    if (typeof value === 'number' && (!Number.isSafeInteger(value) || value < 0)) {
        throw new Error(`${field} must be a non-negative safe integer`);
    }
    const integer = BigInt(value);
    if (integer < 0n || integer > 0xffffffffffffffffn) {
        throw new Error(`${field} must fit in u64`);
    }
    const output = new Uint8Array(8);
    new DataView(output.buffer).setBigUint64(0, integer, true);
    return output;
}

function encodeU32(value) {
    const output = new Uint8Array(4);
    new DataView(output.buffer).setUint32(0, value, true);
    return output;
}

function encodeU16(value, field) {
    if (!Number.isInteger(value) || value < 0 || value > 0xffff) {
        throw new Error(`${field} must fit in u16`);
    }
    const output = new Uint8Array(2);
    new DataView(output.buffer).setUint16(0, value, true);
    return output;
}

function encodeU8(value, field) {
    if (!Number.isInteger(value) || value < 0 || value > 0xff) {
        throw new Error(`${field} must fit in u8`);
    }
    return Uint8Array.of(value);
}

function encodeBytes(value, field) {
    const bytes = value instanceof Uint8Array ? value : Uint8Array.from(value);
    return concat([encodeU64(bytes.length, `${field} length`), bytes]);
}

function encodeVector(values, encoder, field) {
    return concat([
        encodeU64(values.length, `${field} length`),
        ...values.map((value, index) => encoder(value, `${field}[${index}]`)),
    ]);
}

function decodeHex(value, field) {
    if (typeof value !== 'string') throw new Error(`${field} must be a hex string`);
    const hex = value.startsWith('0x') ? value.slice(2) : value;
    if (hex.length % 2 !== 0 || !/^[0-9a-f]*$/i.test(hex)) {
        throw new Error(`${field} must contain complete hexadecimal bytes`);
    }
    return Uint8Array.from(Buffer.from(hex, 'hex'));
}

function encodePqSignature(signature, field) {
    if (!signature || !signature.public_key) throw new Error(`${field} is malformed`);
    if (signature.scheme_version !== signature.public_key.scheme_version) {
        throw new Error(`${field} signature and public-key schemes must match`);
    }
    const publicKey = decodeHex(signature.public_key.bytes, `${field}.public_key.bytes`);
    const signatureBytes = decodeHex(signature.sig, `${field}.sig`);
    return concat([
        encodeU8(signature.scheme_version, `${field}.scheme_version`),
        encodeU8(signature.public_key.scheme_version, `${field}.public_key.scheme_version`),
        encodeBytes(publicKey, `${field}.public_key.bytes`),
        encodeBytes(signatureBytes, `${field}.sig`),
    ]);
}

function encodePublicKey(value, field) {
    if (typeof value !== 'string') throw new Error(`${field} must be a base58 string`);
    const bytes = bs58decode(value);
    if (bytes.length !== 32) throw new Error(`${field} must decode to 32 bytes`);
    return bytes;
}

function encodeInstruction(instruction, field) {
    if (!instruction || !Array.isArray(instruction.accounts)) {
        throw new Error(`${field} must include an accounts array`);
    }
    const data = instruction.data instanceof Uint8Array
        ? instruction.data
        : Uint8Array.from(instruction.data || []);
    return concat([
        encodePublicKey(instruction.program_id, `${field}.program_id`),
        encodeVector(instruction.accounts, encodePublicKey, `${field}.accounts`),
        encodeBytes(data, `${field}.data`),
    ]);
}

function encodeOptionU64(value, field) {
    return value === undefined || value === null
        ? Uint8Array.of(0)
        : concat([Uint8Array.of(1), encodeU64(value, field)]);
}

function encodeMessage(message) {
    if (!message || !Array.isArray(message.instructions)) {
        throw new Error('message.instructions must be an array');
    }
    const blockhash = decodeHex(message.blockhash, 'message.blockhash');
    if (blockhash.length !== 32) throw new Error('message.blockhash must be 32 bytes');
    return concat([
        encodeVector(message.instructions, encodeInstruction, 'message.instructions'),
        blockhash,
        encodeOptionU64(message.compute_budget, 'message.compute_budget'),
        encodeOptionU64(message.compute_unit_price, 'message.compute_unit_price'),
    ]);
}

function encodeNativeTransactionWire(signatures, message) {
    if (!Array.isArray(signatures)) throw new Error('signatures must be an array');
    return concat([
        TX_WIRE_MAGIC,
        Uint8Array.of(TX_WIRE_VERSION, TX_TYPE_NATIVE),
        encodeVector(signatures, encodePqSignature, 'signatures'),
        encodeMessage(message),
        encodeU32(TX_TYPE_NATIVE),
    ]);
}

function encodeNativeTransactionBase64(signatures, message) {
    return Buffer.from(encodeNativeTransactionWire(signatures, message)).toString('base64');
}

function nativeTransactionSigningBytes(messageBytes, chainId = process.env.LICHEN_CHAIN_ID || DEFAULT_TEST_CHAIN_ID) {
    if (!(messageBytes instanceof Uint8Array)) {
        throw new Error('messageBytes must be a Uint8Array');
    }
    if (typeof chainId !== 'string' || chainId.length === 0) {
        throw new Error('chainId is required for transaction signing');
    }
    const domainBytes = Uint8Array.from(Buffer.from(NATIVE_TX_DOMAIN, 'utf8'));
    const chainBytes = Uint8Array.from(Buffer.from(chainId, 'utf8'));
    return concat([
        SIGNING_ENVELOPE_MAGIC,
        Uint8Array.of(SIGNING_ENVELOPE_VERSION),
        encodeU16(domainBytes.length, 'signing domain length'),
        domainBytes,
        encodeU16(chainBytes.length, 'chainId length'),
        chainBytes,
        encodeU64(messageBytes.length, 'messageBytes length'),
        messageBytes,
    ]);
}

function signNativeTransaction(pq, keypair, messageBytes, chainId) {
    return pq.sign(nativeTransactionSigningBytes(messageBytes, chainId), keypair);
}

module.exports = {
    encodeNativeTransactionBase64,
    encodeNativeTransactionWire,
    nativeTransactionSigningBytes,
    signNativeTransaction,
};
