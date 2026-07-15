// Test: Verify encodeTransaction signature format matches Rust bincode Vec<PqSignature>
import assert from 'node:assert';

// Inline minimal helpers (extracted from bincode.ts logic)
function encodeU64LE(value) {
  const out = new Uint8Array(8);
  const view = new DataView(out.buffer);
  view.setBigUint64(0, BigInt(value), true);
  return out;
}

function hexToBytes(hex) {
  const clean = hex.startsWith('0x') ? hex.slice(2) : hex;
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

function concat(parts) {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

// Canonical V1 transaction wire encoder.
function encodeU32LE(value) {
  const out = new Uint8Array(4);
  const view = new DataView(out.buffer);
  view.setUint32(0, value, true);
  return out;
}

function encodeU8(value) {
  return Uint8Array.of(value & 0xff);
}

function encodeBytes(data) {
  return concat([encodeU64LE(data.length), data]);
}

function encodePqPublicKey(publicKey) {
  if (publicKey.bytes.length !== 1952) {
    throw new Error(`Public key must be 1952 bytes, got ${publicKey.bytes.length}`);
  }
  return concat([encodeU8(publicKey.scheme_version), encodeBytes(publicKey.bytes)]);
}

function encodePqSignature(signature) {
  if (signature.sig.length !== 3309) {
    throw new Error(`Signature must be 3309 bytes, got ${signature.sig.length}`);
  }
  return concat([
    encodeU8(signature.scheme_version),
    encodePqPublicKey(signature.public_key),
    encodeBytes(signature.sig),
  ]);
}

function encodeTransactionWire(signatures, messageBytes) {
  const sigBytes = signatures.map(encodePqSignature);
  const encodedSigs = concat([encodeU64LE(sigBytes.length), ...sigBytes]);
  // tx_type: Native=0 (u32 LE)
  const txType = encodeU32LE(0);
  return concat([new Uint8Array([0x4d, 0x54, 0x01, 0x00]), encodedSigs, messageBytes, txType]);
}

const testSignature = {
  scheme_version: 0x01,
  public_key: {
    scheme_version: 0x01,
    bytes: new Uint8Array(1952).fill(0xbb),
  },
  sig: new Uint8Array(3309).fill(0xbb),
};

// Test 1: Correct signature encoding (Vec<PqSignature> format)
{
  const message = new Uint8Array(40);

  const result = encodeTransactionWire([testSignature], message);

  assert.strictEqual(result.length, 5335, `Expected 5335, got ${result.length}`);
  assert.deepStrictEqual(Array.from(result.slice(0, 4)), [0x4d, 0x54, 0x01, 0x00]);

  // Vec length = 1 (little-endian u64)
  const view = new DataView(result.buffer);
  const vecLen = Number(view.getBigUint64(4, true));
  assert.strictEqual(vecLen, 1, `Expected vec len 1, got ${vecLen}`);

  assert.strictEqual(result[12], 0x01, 'signature scheme mismatch');
  assert.strictEqual(result[13], 0x01, 'public key scheme mismatch');
  assert.strictEqual(Number(view.getBigUint64(14, true)), 1952, 'public key length mismatch');
  assert.strictEqual(Number(view.getBigUint64(1974, true)), 3309, 'signature length mismatch');
  console.log('Test 1 PASSED: signature encoding matches Rust bincode');
}

// Test 2: Reject wrong signature length
{
  try {
    encodeTransactionWire([
      {
        scheme_version: 0x01,
        public_key: {
          scheme_version: 0x01,
          bytes: new Uint8Array(1952),
        },
        sig: new Uint8Array(2),
      },
    ], new Uint8Array(1));
    assert.fail('Should have thrown');
  } catch (e) {
    assert.ok(e.message.includes('3309'), `Wrong error: ${e.message}`);
    console.log('Test 2 PASSED: rejects short signature');
  }
}

// Test 3: Multiple signatures
{
  const sig1 = testSignature;
  const sig2 = {
    scheme_version: 0x01,
    public_key: {
      scheme_version: 0x01,
      bytes: new Uint8Array(1952).fill(0xaa),
    },
    sig: new Uint8Array(3309).fill(0xaa),
  };
  const result = encodeTransactionWire([sig1, sig2], new Uint8Array(10));
  assert.strictEqual(result.length, 10584, `Expected 10584, got ${result.length}`);
  const view = new DataView(result.buffer);
  const vecLen = Number(view.getBigUint64(4, true));
  assert.strictEqual(vecLen, 2, `Expected vec len 2, got ${vecLen}`);
  console.log('Test 3 PASSED: multiple signatures');
}

console.log('All JS bincode tests passed!');
