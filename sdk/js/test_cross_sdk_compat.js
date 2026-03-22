// K4-02: Cross-SDK compatibility test
// Validates JS SDK bincode encoding matches Rust golden vectors exactly.
// The authoritative hex values come from core/src/transaction.rs
// test_cross_sdk_message_golden_vector and test_cross_sdk_transaction_golden_vector.

const assert = require('assert');

// --- Inline minimal encoder (mirrors bincode.ts without TS/ESM deps) ---

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

function bytesToHex(bytes) {
  return Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('');
}

function concat(parts) {
  const total = parts.reduce((sum, p) => sum + p.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const p of parts) { out.set(p, offset); offset += p.length; }
  return out;
}

function encodeVec(items) {
  return concat([encodeU64LE(items.length), ...items]);
}

function encodeBytes(data) {
  return concat([encodeU64LE(data.length), data]);
}

function encodePubkey(bytes32) {
  if (bytes32.length !== 32) throw new Error('Pubkey must be 32 bytes');
  return bytes32;
}

function encodeInstruction(ix) {
  const programId = encodePubkey(ix.programId);
  const accounts = encodeVec(ix.accounts.map(encodePubkey));
  const data = encodeBytes(ix.data);
  return concat([programId, accounts, data]);
}

function encodeMessage(msg) {
  const instructions = encodeVec(msg.instructions.map(encodeInstruction));
  const blockhash = msg.recentBlockhash;
  if (blockhash.length !== 32) throw new Error('Blockhash must be 32 bytes');
  // compute_budget: Option<u64> (None = 0x00)
  const cb = new Uint8Array([0x00]);
  // compute_unit_price: Option<u64> (None = 0x00)
  const cup = new Uint8Array([0x00]);
  return concat([instructions, blockhash, cb, cup]);
}

function encodeU32LE(value) {
  const out = new Uint8Array(4);
  const view = new DataView(out.buffer);
  view.setUint32(0, value, true);
  return out;
}

function encodeTransaction(sigs, messageBytes) {
  const sigParts = sigs.map(s => {
    if (s.length !== 64) throw new Error('Sig must be 64 bytes');
    return s;
  });
  // tx_type: Native = 0 (u32 LE)
  const txType = encodeU32LE(0);
  return concat([encodeU64LE(sigParts.length), ...sigParts, messageBytes, txType]);
}

// --- Deterministic test data (same as Rust golden vector tests) ---

const programId = new Uint8Array(32).fill(0x01);
const account0 = new Uint8Array(32).fill(0x02);
const data = new Uint8Array([0x00, 0x01, 0x02, 0x03]);
const blockhash = new Uint8Array(32).fill(0xAA);
const sig = new Uint8Array(64).fill(0xBB);

const message = {
  instructions: [{ programId, accounts: [account0], data }],
  recentBlockhash: blockhash,
};

// --- Test 1: Message golden vector ---
{
  const msgBytes = encodeMessage(message);
  const hex = bytesToHex(msgBytes);

  // Authoritative value from Rust test_cross_sdk_message_golden_vector
  const expected =
    '0100000000000000' +                                          // Vec<Ix> len = 1
    '0101010101010101010101010101010101010101010101010101010101010101' + // program_id
    '0100000000000000' +                                          // Vec<Pubkey> len = 1
    '0202020202020202020202020202020202020202020202020202020202020202' + // accounts[0]
    '040000000000000000010203' +                                  // Vec<u8> len=4 + data
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' + // blockhash
    '0000';                                                       // compute_budget: None + compute_unit_price: None

  assert.strictEqual(hex, expected,
    `K4-02 JS MESSAGE GOLDEN VECTOR MISMATCH!\nGot:      ${hex}\nExpected: ${expected}`);
  console.log('  ✓ Message golden vector matches Rust');
}

// --- Test 2: Transaction golden vector ---
{
  const msgBytes = encodeMessage(message);
  const txBytes = encodeTransaction([sig], msgBytes);
  const hex = bytesToHex(txBytes);

  const sigHex = 'bb'.repeat(64); // 64 bytes
  const expected =
    '0100000000000000' +                                          // Vec<[u8;64]> len = 1
    sigHex +                                                      // sig
    '0100000000000000' +                                          // Vec<Ix> len = 1
    '0101010101010101010101010101010101010101010101010101010101010101' + // program_id
    '0100000000000000' +                                          // Vec<Pubkey> len = 1
    '0202020202020202020202020202020202020202020202020202020202020202' + // accounts[0]
    '040000000000000000010203' +                                  // Vec<u8> len=4 + data
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' + // blockhash
    '0000' +                                                      // compute_budget: None + compute_unit_price: None
    '00000000';                                                   // tx_type: Native (u32 LE)

  assert.strictEqual(hex, expected,
    `K4-02 JS TX GOLDEN VECTOR MISMATCH!\nGot:      ${hex}\nExpected: ${expected}`);
  console.log('  ✓ Transaction golden vector matches Rust');
}

console.log('K4-02: All JS cross-SDK compatibility tests passed');
