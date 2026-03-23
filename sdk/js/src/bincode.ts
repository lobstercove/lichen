// Minimal bincode encoder for Lichen transactions

import { PublicKey } from './publickey';
import { Instruction, Message, Transaction } from './transaction';

const textEncoder = new TextEncoder();

function encodeU64LE(value: number | bigint): Uint8Array {
  const out = new Uint8Array(8);
  const view = new DataView(out.buffer);
  view.setBigUint64(0, BigInt(value), true);
  return out;
}

function encodeU32LE(value: number): Uint8Array {
  const out = new Uint8Array(4);
  const view = new DataView(out.buffer);
  view.setUint32(0, value, true);
  return out;
}

function encodeOptionU64(value?: number): Uint8Array {
  if (value === undefined || value === null) {
    return new Uint8Array([0x00]); // None
  }
  return concat([new Uint8Array([0x01]), encodeU64LE(value)]); // Some(value)
}

function concat(parts: Uint8Array[]): Uint8Array {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function encodeBytes(data: Uint8Array): Uint8Array {
  return concat([encodeU64LE(data.length), data]);
}

function encodeString(value: string): Uint8Array {
  const encoded = textEncoder.encode(value);
  return concat([encodeU64LE(encoded.length), encoded]);
}

function encodeVec(items: Uint8Array[]): Uint8Array {
  return concat([encodeU64LE(items.length), ...items]);
}

export function hexToBytes(hex: string): Uint8Array {
  const clean = hex.startsWith('0x') ? hex.slice(2) : hex;
  if (clean.length % 2 !== 0) {
    throw new Error('Invalid hex string');
  }
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
}

function encodePubkey(pubkey: PublicKey): Uint8Array {
  const bytes = pubkey.toBytes();
  if (bytes.length !== 32) {
    throw new Error('PublicKey must be 32 bytes');
  }
  return bytes;
}

function encodeInstruction(ix: Instruction): Uint8Array {
  const programId = encodePubkey(ix.programId);
  const accounts = encodeVec(ix.accounts.map(encodePubkey));
  const data = encodeBytes(ix.data);
  return concat([programId, accounts, data]);
}

export function encodeMessage(message: Message): Uint8Array {
  const instructions = encodeVec(message.instructions.map(encodeInstruction));
  const blockhash = hexToBytes(message.recentBlockhash);
  if (blockhash.length !== 32) {
    throw new Error('Blockhash must be 32 bytes');
  }
  const computeBudget = encodeOptionU64(message.computeBudget);
  const computeUnitPrice = encodeOptionU64(message.computeUnitPrice);
  return concat([instructions, blockhash, computeBudget, computeUnitPrice]);
}

export function encodeTransaction(transaction: Transaction): Uint8Array {
  // Encode signatures as Vec<[u8; 64]> matching Rust bincode format.
  // Each signature is a hex string → 64 raw bytes. Fixed-size arrays in
  // bincode have no per-element length prefix.
  const sigBytes = transaction.signatures.map(hexSig => {
    const raw = hexToBytes(hexSig);
    if (raw.length !== 64) {
      throw new Error(`Signature must be 64 bytes, got ${raw.length}`);
    }
    return raw;
  });
  const encodedSigs = concat([encodeU64LE(sigBytes.length), ...sigBytes]);
  const messageBytes = encodeMessage(transaction.message);
  // tx_type: Native=0 (u32 LE)
  const txType = encodeU32LE(0);
  return concat([encodedSigs, messageBytes, txType]);
}
