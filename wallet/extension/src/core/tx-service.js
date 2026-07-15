import { base58Decode, signTransaction, generateEVMAddress } from './crypto-service.js';
import { LichenRPC, getRpcEndpoint } from './rpc-service.js';
import { parsePositiveDecimalBaseUnits } from './amount-service.js';

const TX_WIRE_MAGIC = new Uint8Array([0x4d, 0x54]);
const TX_WIRE_VERSION = 1;
const TX_TYPE_NATIVE = 0;
const SIGNING_ENVELOPE_MAGIC = new TextEncoder().encode('LICHEN-SIG');
const SIGNING_ENVELOPE_VERSION = 1;
const DOMAIN_NATIVE_TX = 'native-tx';

function concatBytes(parts) {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function encodeU64LE(value) {
  const out = new Uint8Array(8);
  new DataView(out.buffer).setBigUint64(0, BigInt(value), true);
  return out;
}

function encodeU32LE(value) {
  const out = new Uint8Array(4);
  new DataView(out.buffer).setUint32(0, value, true);
  return out;
}

function encodeU16LE(value) {
  const out = new Uint8Array(2);
  new DataView(out.buffer).setUint16(0, value, true);
  return out;
}

function hexBytes(value, fieldName) {
  const text = String(value || '').replace(/^0x/, '');
  if (!/^[0-9a-fA-F]*$/.test(text) || text.length % 2 !== 0) {
    throw new Error(`Invalid ${fieldName} hex`);
  }
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(text.slice(i * 2, i * 2 + 2), 16);
  return out;
}

function encodeVecBytes(bytes) {
  return concatBytes([encodeU64LE(bytes.length), bytes]);
}

function encodePqSignature(signature) {
  const schemeVersion = Number(signature?.scheme_version);
  const publicKeySchemeVersion = Number(signature?.public_key?.scheme_version);
  if (!Number.isInteger(schemeVersion) || !Number.isInteger(publicKeySchemeVersion)) {
    throw new Error('Invalid PQ signature scheme version');
  }
  const publicKey = hexBytes(signature.public_key.bytes, 'PQ public key');
  const signatureBytes = hexBytes(signature.sig, 'PQ signature');
  return concatBytes([
    Uint8Array.of(schemeVersion),
    Uint8Array.of(publicKeySchemeVersion),
    encodeVecBytes(publicKey),
    encodeVecBytes(signatureBytes),
  ]);
}

export function signingBytesForChainId(messageBytes, chainId) {
  const normalizedChainId = String(chainId || '').trim();
  if (!normalizedChainId) throw new Error('Chain id is required for transaction signing');
  const domainBytes = new TextEncoder().encode(DOMAIN_NATIVE_TX);
  const chainBytes = new TextEncoder().encode(normalizedChainId);
  return concatBytes([
    SIGNING_ENVELOPE_MAGIC,
    Uint8Array.of(SIGNING_ENVELOPE_VERSION),
    encodeU16LE(domainBytes.length),
    domainBytes,
    encodeU16LE(chainBytes.length),
    chainBytes,
    encodeU64LE(messageBytes.length),
    messageBytes,
  ]);
}

/**
 * Serialize a transaction message using the canonical Rust bincode payload format.
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

  function parseOptionalU64(value, fieldName) {
    if (typeof value === 'bigint') {
      if (value < 0n) throw new Error(`Invalid ${fieldName}: expected a non-negative integer`);
      return value;
    }
    if (typeof value === 'number') {
      if (!Number.isSafeInteger(value) || value < 0) {
        throw new Error(`Invalid ${fieldName}: expected a non-negative integer`);
      }
      return BigInt(value);
    }
    const text = String(value ?? '').trim();
    if (!/^\d+$/.test(text)) {
      throw new Error(`Invalid ${fieldName}: expected a non-negative integer`);
    }
    return BigInt(text);
  }

  function writeOptionalU64(value, fieldName) {
    if (value === undefined || value === null) {
      parts.push(new Uint8Array([0x00]));
      return;
    }

    const numeric = parseOptionalU64(value, fieldName);
    if (numeric === 0n) {
      parts.push(new Uint8Array([0x00]));
      return;
    }

    parts.push(new Uint8Array([0x01]));
    writeU64LE(numeric);
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
  const hashHex = String(message.blockhash || message.recent_blockhash || '');
  if (!/^[0-9a-fA-F]{64}$/.test(hashHex)) {
    throw new Error('Invalid blockhash: must be exactly 64 hex characters');
  }
  const hashBytes = new Uint8Array(32);
  for (let i = 0; i < 32; i++) {
    hashBytes[i] = parseInt(hashHex.substr(i * 2, 2), 16);
  }
  writeBytes(hashBytes);

  writeOptionalU64(message.compute_budget ?? message.computeBudget, 'compute_budget');
  writeOptionalU64(message.compute_unit_price ?? message.computeUnitPrice, 'compute_unit_price');

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
  const signatures = Array.isArray(transaction?.signatures) ? transaction.signatures : [];
  const messageBytes = serializeMessageForSigning(transaction?.message || {});
  const payload = concatBytes([
    encodeU64LE(signatures.length),
    ...signatures.map(encodePqSignature),
    messageBytes,
    encodeU32LE(TX_TYPE_NATIVE),
  ]);
  const txBytes = concatBytes([
    TX_WIRE_MAGIC,
    Uint8Array.of(TX_WIRE_VERSION, TX_TYPE_NATIVE),
    payload,
  ]);
  let binary = '';
  const chunkSize = 0x8000;
  for (let offset = 0; offset < txBytes.length; offset += chunkSize) {
    binary += String.fromCharCode.apply(null, txBytes.subarray(offset, offset + chunkSize));
  }
  return btoa(binary);
}

export function buildNativeTransferMessage(fromAddress, toAddress, amountLicn, blockhash) {
  const fromPubkey = base58Decode(fromAddress);
  const toPubkey = base58Decode(toAddress);
  const spores = parsePositiveDecimalBaseUnits(amountLicn, 9, 'Transfer amount');

  const systemProgram = new Uint8Array(32); // SYSTEM_PROGRAM_ID = [0; 32]
  const instructionData = new Uint8Array(9);
  instructionData[0] = 0;
  const view = new DataView(instructionData.buffer);
  view.setBigUint64(1, spores, true);

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
  fromAddress,
  toAddress,
  amountLicn,
  blockhash,
  chainId,
}) {
  const message = buildNativeTransferMessage(fromAddress, toAddress, amountLicn, blockhash);
  const messageBytes = serializeMessageForSigning(message);
  const signature = await signTransaction(privateKeyHex, signingBytesForChainId(messageBytes, chainId));

  return {
    signatures: [signature],
    message
  };
}

export function buildAmountInstructionData(opcode, amountLicn, extraByte) {
  const spores = parsePositiveDecimalBaseUnits(amountLicn, 9, 'Amount');

  const hasExtra = extraByte !== undefined && extraByte !== null;
  const instructionData = new Uint8Array(hasExtra ? 10 : 9);
  instructionData[0] = opcode;
  const view = new DataView(instructionData.buffer);
  view.setBigUint64(1, spores, true);
  if (hasExtra) instructionData[9] = extraByte & 0xff;
  return instructionData;
}

export async function buildSignedSingleInstructionTransaction({
  privateKeyHex,
  fromAddress,
  blockhash,
  chainId,
  programIdBytes,
  accountPubkeys,
  instructionDataBytes
}) {
  const fromPubkey = base58Decode(fromAddress);
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
  const signature = await signTransaction(privateKeyHex, signingBytesForChainId(messageBytes, chainId));

  return {
    signatures: [signature],
    message
  };
}

/**
 * Register EVM address on-chain for a wallet.
 * Flow: localStorage cache → RPC check → send tx → cache.
 * Does NOT block on failure.
 */
export async function registerEvmAddress({ wallet, privateKeyHex, network, settings }) {
  try {
    const cacheKey = `licnEvmRegistered:${wallet.address}`;
    // 1) localStorage cache hit — skip entirely
    if (typeof localStorage !== 'undefined') {
      try { if (localStorage.getItem(cacheKey) === '1') return; } catch (_) { }
    }

    const rpcUrl = getRpcEndpoint(network, settings);
    const rpc = new LichenRPC(rpcUrl);

    // 2) On-chain check via RPC
    try {
      const existing = await rpc.call('getEvmRegistration', [wallet.address]);
      if (existing && existing.evmAddress) {
        // Already registered on-chain — cache and return
        try { localStorage.setItem(cacheKey, '1'); } catch (_) { }
        return;
      }
    } catch (_) { } // RPC down — fall through, processor is idempotent

    // 3) Skip if account not funded
    try {
      const bal = await rpc.getBalance(wallet.address);
      if (!bal || (bal.spores === 0 && !bal.spendable)) return;
    } catch (_) { return; }

    // 4) Derive EVM address
    const evmAddress = generateEVMAddress(wallet.address);
    if (!evmAddress || evmAddress === '0x' + '0'.repeat(40)) return;

    // 5) Build opcode 12 instruction
    const evmHex = evmAddress.slice(2);
    const evmBytes = new Uint8Array(20);
    for (let i = 0; i < 20; i++) evmBytes[i] = parseInt(evmHex.substr(i * 2, 2), 16);

    const instructionData = new Uint8Array(21);
    instructionData[0] = 12;
    instructionData.set(evmBytes, 1);

    const [blockhash, chainId] = await Promise.all([rpc.getRecentBlockhash(), rpc.getChainId()]);
    const tx = await buildSignedSingleInstructionTransaction({
      privateKeyHex,
      fromAddress: wallet.address,
      blockhash,
      chainId,
      instructionDataBytes: instructionData
    });

    const txBase64 = encodeTransactionBase64(tx);
    await rpc.sendTransactionWithPreflight(txBase64);
    console.log('EVM address registered:', evmAddress, '→', wallet.address);

    // 6) Cache after successful registration
    try { localStorage.setItem(cacheKey, '1'); } catch (_) { }
  } catch (err) {
    console.warn('EVM registration deferred:', err.message);
  }
}
