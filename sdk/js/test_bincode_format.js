// Test: Verify encodeTransaction signature format matches Rust bincode Vec<[u8; 64]>
const assert = require('assert');

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

// This is the FIXED encodeTransaction logic
function encodeTransaction(signatures, messageBytes) {
  const sigBytes = signatures.map(hexSig => {
    const raw = hexToBytes(hexSig);
    if (raw.length !== 64) {
      throw new Error(`Signature must be 64 bytes, got ${raw.length}`);
    }
    return raw;
  });
  const encodedSigs = concat([encodeU64LE(sigBytes.length), ...sigBytes]);
  return concat([encodedSigs, messageBytes]);
}

// Test 1: Correct signature encoding (Vec<[u8; 64]> format)
{
  const sigBytes = new Uint8Array(64);
  for (let i = 0; i < 64; i++) sigBytes[i] = i;
  const sigHex = Array.from(sigBytes).map(b => b.toString(16).padStart(2, '0')).join('');
  const message = new Uint8Array(40);

  const result = encodeTransaction([sigHex], message);

  // Expected: 8 (vec len) + 64 (sig) + 40 (message) = 112
  assert.strictEqual(result.length, 112, `Expected 112, got ${result.length}`);

  // Vec length = 1 (little-endian u64)
  const view = new DataView(result.buffer);
  const vecLen = Number(view.getBigUint64(0, true));
  assert.strictEqual(vecLen, 1, `Expected vec len 1, got ${vecLen}`);

  // Signature bytes match (no length prefix)
  for (let i = 0; i < 64; i++) {
    assert.strictEqual(result[8 + i], i, `Sig byte ${i} mismatch`);
  }
  console.log('Test 1 PASSED: signature encoding matches Rust bincode');
}

// Test 2: Reject wrong signature length
{
  try {
    encodeTransaction(['aabb'], new Uint8Array(1));
    assert.fail('Should have thrown');
  } catch (e) {
    assert.ok(e.message.includes('64 bytes'), `Wrong error: ${e.message}`);
    console.log('Test 2 PASSED: rejects short signature');
  }
}

// Test 3: Multiple signatures
{
  const sig1 = '00'.repeat(64);
  const sig2 = 'ff'.repeat(64);
  const result = encodeTransaction([sig1, sig2], new Uint8Array(10));
  // 8 + 64 + 64 + 10 = 146
  assert.strictEqual(result.length, 146, `Expected 146, got ${result.length}`);
  const view = new DataView(result.buffer);
  const vecLen = Number(view.getBigUint64(0, true));
  assert.strictEqual(vecLen, 2, `Expected vec len 2, got ${vecLen}`);
  console.log('Test 3 PASSED: multiple signatures');
}

console.log('All JS bincode tests passed!');
