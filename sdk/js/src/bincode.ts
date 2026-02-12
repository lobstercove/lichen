// Minimal bincode encoder for MoltChain transactions

import { PublicKey } from './publickey';
import { Instruction, Message, Transaction } from './transaction';

const textEncoder = new TextEncoder();

function encodeU64LE(value: number | bigint): Uint8Array {
  const out = new Uint8Array(8);
  const view = new DataView(out.buffer);
  view.setBigUint64(0, BigInt(value), true);
  return out;
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
  return concat([instructions, blockhash]);
}

export function encodeTransaction(transaction: Transaction): Uint8Array {
  const signatures = encodeVec(transaction.signatures.map(encodeString));
  const messageBytes = encodeMessage(transaction.message);
  return concat([signatures, messageBytes]);
}
