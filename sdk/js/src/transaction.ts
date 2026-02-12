// MoltChain SDK - Transaction Types and Builder

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
   */
  static transfer(from: PublicKey, to: PublicKey, amount: number): Instruction {
    // Encode transfer data (program-specific format)
    const data = new Uint8Array(9);
    data[0] = 0; // Transfer instruction type
    const view = new DataView(data.buffer);
    view.setBigUint64(1, BigInt(amount), true);

    return {
      programId: new PublicKey('11111111111111111111111111111111'), // System program (all-zero pubkey)
      accounts: [from, to],
      data,
    };
  }

  /**
   * Create a stake instruction
   */
  static stake(from: PublicKey, validator: PublicKey, amount: number): Instruction {
    const data = new Uint8Array(9);
    data[0] = 9; // Stake instruction type
    const view = new DataView(data.buffer);
    view.setBigUint64(1, BigInt(amount), true);

    return {
      programId: new PublicKey('11111111111111111111111111111111'), // System program (all-zero pubkey)
      accounts: [from, validator],
      data,
    };
  }

  /**
   * Create an unstake request instruction
   */
  static unstake(from: PublicKey, validator: PublicKey, amount: number): Instruction {
    const data = new Uint8Array(9);
    data[0] = 10; // Unstake request instruction type
    const view = new DataView(data.buffer);
    view.setBigUint64(1, BigInt(amount), true);

    return {
      programId: new PublicKey('11111111111111111111111111111111'), // System program (all-zero pubkey)
      accounts: [from, validator],
      data,
    };
  }
}
