import { base58Decode, hexToBytes, signTransaction } from './crypto-service.js';

/**
 * Serialize a transaction message using Bincode format (matches Rust bincode::serialize)
 * This MUST match the website's serializeMessageBincode() exactly for signature compatibility.
 */
export function serializeMessageForSigning(message) {
  const parts = [];

  // Helper: write u64 little-endian (8 bytes) — bincode uses fixint u64 for Vec lengths
  function writeU64LE(n) {
    const buf = new ArrayBuffer(8);
    const view = new DataView(buf);
    view.setBigUint64(0, BigInt(n), true);
    parts.push(new Uint8Array(buf));
  }

  // Helper: write raw bytes
  function writeBytes(bytes) {
    parts.push(new Uint8Array(bytes));
  }

  // instructions: Vec<Instruction>
  const ixs = message.instructions || [];
  writeU64LE(ixs.length);
  for (const ix of ixs) {
    // program_id: [u8; 32] — fixed-size, no length prefix
    writeBytes(ix.program_id);
    // accounts: Vec<Pubkey> — u64 length + N * 32 bytes
    const accounts = ix.accounts || [];
    writeU64LE(accounts.length);
    for (const acct of accounts) {
      writeBytes(acct);
    }
    // data: Vec<u8> — u64 length + N bytes
    const data = ix.data || [];
    writeU64LE(data.length);
    writeBytes(data);
  }

  // recent_blockhash: Hash([u8; 32]) — parse hex string to 32 bytes
  const hashHex = message.blockhash || message.recent_blockhash;
  const hashBytes = new Uint8Array(32);
  for (let i = 0; i < 32; i++) {
    hashBytes[i] = parseInt(hashHex.substr(i * 2, 2), 16);
  }
  writeBytes(hashBytes);

  // Concatenate all parts
  const totalLen = parts.reduce((s, p) => s + p.length, 0);
  const result = new Uint8Array(totalLen);
  let offset = 0;
  for (const p of parts) {
    result.set(p, offset);
    offset += p.length;
  }
  return result;
}

export function encodeTransactionBase64(transaction) {
  const txBytes = new TextEncoder().encode(JSON.stringify(transaction));
  return btoa(String.fromCharCode(...txBytes));
}

export function buildNativeTransferMessage(fromPublicKeyHex, toAddress, amountMolt, blockhash) {
  const fromPubkey = hexToBytes(fromPublicKeyHex);
  const toPubkey = base58Decode(toAddress);
  const shells = Math.floor(Number(amountMolt) * 1_000_000_000);

  if (!Number.isFinite(shells) || shells <= 0) {
    throw new Error('Invalid transfer amount');
  }

  const systemProgram = new Uint8Array(32); // SYSTEM_PROGRAM_ID = [0; 32]
  const instructionData = new Uint8Array(9);
  instructionData[0] = 0;
  const view = new DataView(instructionData.buffer);
  view.setBigUint64(1, BigInt(shells), true);

  return {
    instructions: [
      {
        program_id: Array.from(systemProgram),
        accounts: [Array.from(fromPubkey), Array.from(toPubkey)],
        data: Array.from(instructionData)
      }
    ],
    blockhash
  };
}

export async function buildSignedNativeTransferTransaction({
  privateKeyHex,
  fromPublicKeyHex,
  toAddress,
  amountMolt,
  blockhash
}) {
  const message = buildNativeTransferMessage(fromPublicKeyHex, toAddress, amountMolt, blockhash);
  const messageBytes = serializeMessageForSigning(message);
  const signature = await signTransaction(privateKeyHex, messageBytes);

  return {
    signatures: [Array.from(signature)],
    message
  };
}

export function buildAmountInstructionData(opcode, amountMolt) {
  const shells = Math.floor(Number(amountMolt) * 1_000_000_000);
  if (!Number.isFinite(shells) || shells <= 0) {
    throw new Error('Invalid amount');
  }

  const instructionData = new Uint8Array(9);
  instructionData[0] = opcode;
  const view = new DataView(instructionData.buffer);
  view.setBigUint64(1, BigInt(shells), true);
  return instructionData;
}

export async function buildSignedSingleInstructionTransaction({
  privateKeyHex,
  fromPublicKeyHex,
  blockhash,
  programIdBytes,
  accountPubkeys,
  instructionDataBytes
}) {
  const fromPubkey = hexToBytes(fromPublicKeyHex);
  const programId = programIdBytes || new Uint8Array(32); // SYSTEM_PROGRAM_ID = [0; 32]

  const accounts = [Array.from(fromPubkey), ...(accountPubkeys || []).map((a) => Array.from(a))];
  const message = {
    instructions: [
      {
        program_id: Array.from(programId),
        accounts,
        data: Array.from(instructionDataBytes)
      }
    ],
    blockhash
  };

  const messageBytes = serializeMessageForSigning(message);
  const signature = await signTransaction(privateKeyHex, messageBytes);

  return {
    signatures: [Array.from(signature)],
    message
  };
}
