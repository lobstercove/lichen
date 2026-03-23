// Lichen SDK - Transaction Types and Builder

import { PublicKey } from './publickey';
import { Keypair } from './keypair';
import { bytesToHex, encodeMessage } from './bincode';

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
  signatures: string[];
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
    const message = this.build();
    const messageBytes = encodeMessage(message);
    const signature = keypair.sign(messageBytes);
    return {
      signatures: [bytesToHex(signature)],
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
}
