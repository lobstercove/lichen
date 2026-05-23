// Lichen SDK - Transaction Types and Builder

import { PublicKey } from './publickey.js';
import { Keypair } from './keypair.js';
import { encodeMessage } from './bincode.js';
import { PqSignature } from './pq.js';

const SIGNING_ENVELOPE_MAGIC = new TextEncoder().encode('LICHEN-SIG');
const SIGNING_ENVELOPE_VERSION = 1;
const DOMAIN_NATIVE_TX = 'native-tx';
const textEncoder = new TextEncoder();

function concatBytes(parts: Uint8Array[]): Uint8Array {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function u16Le(value: number): Uint8Array {
  const out = new Uint8Array(2);
  new DataView(out.buffer).setUint16(0, value, true);
  return out;
}

function u64Le(value: number | bigint): Uint8Array {
  const out = new Uint8Array(8);
  new DataView(out.buffer).setBigUint64(0, BigInt(value), true);
  return out;
}

export function signingBytesForChainId(
  domain: string,
  chainId: string,
  payload: Uint8Array,
): Uint8Array {
  if (!chainId) return payload;
  const domainBytes = textEncoder.encode(domain);
  const chainBytes = textEncoder.encode(chainId);
  if (domainBytes.length > 0xffff) throw new Error('Signing domain is too long');
  if (chainBytes.length > 0xffff) throw new Error('Chain id is too long');
  return concatBytes([
    SIGNING_ENVELOPE_MAGIC,
    Uint8Array.of(SIGNING_ENVELOPE_VERSION),
    u16Le(domainBytes.length),
    domainBytes,
    u16Le(chainBytes.length),
    chainBytes,
    u64Le(payload.length),
    payload,
  ]);
}

/**
 * Transaction instruction
 */
export interface Instruction {
  programId: PublicKey;
  accounts: PublicKey[];
  data: Uint8Array;
}

/**
 * Transaction message (before signing)
 */
export interface Message {
  instructions: Instruction[];
  recentBlockhash: string;
  computeBudget?: number;
  computeUnitPrice?: number;
}

/**
 * Signed transaction
 */
export interface Transaction {
  signatures: PqSignature[];
  message: Message;
}

/**
 * Transaction builder
 */
export class TransactionBuilder {
  private instructions: Instruction[] = [];
  private recentBlockhash?: string;

  /**
   * Add an instruction
   */
  add(instruction: Instruction): this {
    this.instructions.push(instruction);
    return this;
  }

  /**
   * Set recent blockhash
   */
  setRecentBlockhash(blockhash: string): this {
    this.recentBlockhash = blockhash;
    return this;
  }

  /**
   * Build the message (ready for signing)
   */
  build(): Message {
    if (!this.recentBlockhash) {
      throw new Error('Recent blockhash not set');
    }
    if (this.instructions.length === 0) {
      throw new Error('No instructions added');
    }

    return {
      instructions: this.instructions,
      recentBlockhash: this.recentBlockhash,
    };
  }

  /**
   * Build and sign the transaction
   */
  buildAndSign(keypair: Keypair): Transaction {
    return this.buildAndSignForChainId(keypair, '');
  }

  /**
   * Build and sign the transaction with a versioned chain-id signing envelope.
   */
  buildAndSignForChainId(keypair: Keypair, chainId: string): Transaction {
    const message = this.build();
    const messageBytes = encodeMessage(message);
    const signature = keypair.sign(signingBytesForChainId(DOMAIN_NATIVE_TX, chainId, messageBytes));
    return {
      signatures: [signature],
      message,
    };
  }

  /**
   * Create a transfer instruction
   *
   * P9-SDK-01: `amount` accepts `number | bigint` to avoid silent truncation
   * for values exceeding `Number.MAX_SAFE_INTEGER` (2^53 - 1).
   * Using `bigint` is recommended for large LICN amounts.
   */
  static transfer(from: PublicKey, to: PublicKey, amount: number | bigint): Instruction {
    const amt = BigInt(amount);
    if (amt < 0n) throw new Error('Transfer amount must be non-negative');
    if (amt > 0xFFFFFFFFFFFFFFFFn) throw new Error('Transfer amount exceeds u64 max');
    // Encode transfer data (program-specific format)
    const data = new Uint8Array(9);
    data[0] = 0; // Transfer instruction type
    const view = new DataView(data.buffer);
    view.setBigUint64(1, amt, true);

    return {
      programId: new PublicKey('11111111111111111111111111111111'), // System program (all-zero pubkey)
      accounts: [from, to],
      data,
    };
  }

  /**
   * Create a stake instruction
   *
   * P9-SDK-01: `amount` accepts `number | bigint`.
   */
  static stake(from: PublicKey, validator: PublicKey, amount: number | bigint): Instruction {
    const amt = BigInt(amount);
    if (amt < 0n) throw new Error('Stake amount must be non-negative');
    if (amt > 0xFFFFFFFFFFFFFFFFn) throw new Error('Stake amount exceeds u64 max');
    const data = new Uint8Array(9);
    data[0] = 9; // Stake instruction type
    const view = new DataView(data.buffer);
    view.setBigUint64(1, amt, true);

    return {
      programId: new PublicKey('11111111111111111111111111111111'), // System program (all-zero pubkey)
      accounts: [from, validator],
      data,
    };
  }

  /**
   * Create an unstake request instruction
   *
   * P9-SDK-01: `amount` accepts `number | bigint`.
   */
  static unstake(from: PublicKey, validator: PublicKey, amount: number | bigint): Instruction {
    const amt = BigInt(amount);
    if (amt < 0n) throw new Error('Unstake amount must be non-negative');
    if (amt > 0xFFFFFFFFFFFFFFFFn) throw new Error('Unstake amount exceeds u64 max');
    const data = new Uint8Array(9);
    data[0] = 10; // Unstake request instruction type
    const view = new DataView(data.buffer);
    view.setBigUint64(1, amt, true);

    return {
      programId: new PublicKey('11111111111111111111111111111111'), // System program (all-zero pubkey)
      accounts: [from, validator],
      data,
    };
  }

  /**
   * Contract program ID: [0xFF; 32]
   */
  private static readonly CONTRACT_PROGRAM_ID = new PublicKey(new Uint8Array(32).fill(0xFF));

  /**
   * Create a deploy contract instruction.
   *
   * @param deployer - The deployer's public key (signer, pays deploy fee)
   * @param code - WASM bytecode
   * @param initData - Optional initialization data (default: empty)
   */
  static deployContract(deployer: PublicKey, code: Uint8Array, initData: Uint8Array = new Uint8Array(0)): Instruction {
    if (code.length < 4 || code[0] !== 0x00 || code[1] !== 0x61 || code[2] !== 0x73 || code[3] !== 0x6d) {
      throw new Error('Invalid WASM bytecode: missing magic header (\\0asm)');
    }
    if (code.length > 512 * 1024) {
      throw new Error('Contract code exceeds 512 KB limit');
    }

    // ContractInstruction::Deploy serialized as JSON (matches core serde_json format)
    const payload = JSON.stringify({
      Deploy: {
        code: Array.from(code),
        init_data: Array.from(initData),
      },
    });
    const data = new TextEncoder().encode(payload);

    return {
      programId: TransactionBuilder.CONTRACT_PROGRAM_ID,
      accounts: [deployer],
      data,
    };
  }

  /**
   * Create a call contract instruction.
   *
   * @param caller - The caller's public key (signer)
   * @param contract - The contract's public key
   * @param functionName - Contract function to call
   * @param args - Serialized function arguments (default: empty)
   * @param value - Native LICN to send with the call in spores (default: 0)
   */
  static callContract(
    caller: PublicKey,
    contract: PublicKey,
    functionName: string,
    args: Uint8Array = new Uint8Array(0),
    value: number | bigint = 0,
  ): Instruction {
    const val = BigInt(value);
    if (val < 0n) throw new Error('Call value must be non-negative');
    if (val > 0xFFFFFFFFFFFFFFFFn) throw new Error('Call value exceeds u64 max');

    // ContractInstruction::Call serialized as JSON (matches core serde_json format)
    const payload = JSON.stringify({
      Call: {
        function: functionName,
        args: Array.from(args),
        value: Number(val),
      },
    });
    const data = new TextEncoder().encode(payload);

    return {
      programId: TransactionBuilder.CONTRACT_PROGRAM_ID,
      accounts: [caller, contract],
      data,
    };
  }

  /**
   * Create an upgrade contract instruction (owner only).
   *
   * @param owner - The contract owner's public key (signer)
   * @param contract - The contract's public key
   * @param code - New WASM bytecode
   */
  static upgradeContract(owner: PublicKey, contract: PublicKey, code: Uint8Array): Instruction {
    if (code.length < 4 || code[0] !== 0x00 || code[1] !== 0x61 || code[2] !== 0x73 || code[3] !== 0x6d) {
      throw new Error('Invalid WASM bytecode: missing magic header (\\0asm)');
    }
    if (code.length > 512 * 1024) {
      throw new Error('Contract code exceeds 512 KB limit');
    }

    const payload = JSON.stringify({ Upgrade: { code: Array.from(code) } });
    const data = new TextEncoder().encode(payload);

    return {
      programId: TransactionBuilder.CONTRACT_PROGRAM_ID,
      accounts: [owner, contract],
      data,
    };
  }
}
