// MoltChain SDK - PublicKey Utilities

import bs58 from 'bs58';

/**
 * A 32-byte public key
 */
export class PublicKey {
  private readonly bytes: Uint8Array;

  constructor(value: string | Uint8Array | number[]) {
    if (typeof value === 'string') {
      // Decode from base58
      this.bytes = bs58.decode(value);
    } else if (Array.isArray(value)) {
      this.bytes = new Uint8Array(value);
    } else {
      this.bytes = value;
    }

    if (this.bytes.length !== 32) {
      throw new Error(`Invalid public key length: ${this.bytes.length}, expected 32`);
    }
  }

  /**
   * Convert to base58 string
   */
  toBase58(): string {
    return bs58.encode(this.bytes);
  }

  /**
   * Convert to bytes
   */
  toBytes(): Uint8Array {
    return this.bytes;
  }

  /**
   * Convert to string (base58)
   */
  toString(): string {
    return this.toBase58();
  }

  /**
   * Check equality
   */
  equals(other: PublicKey): boolean {
    if (this.bytes.length !== other.bytes.length) {
      return false;
    }
    for (let i = 0; i < this.bytes.length; i++) {
      if (this.bytes[i] !== other.bytes[i]) {
        return false;
      }
    }
    return true;
  }

  /**
   * Create from base58 string
   */
  static fromBase58(str: string): PublicKey {
    return new PublicKey(str);
  }

  /**
   * Create from bytes
   */
  static fromBytes(bytes: Uint8Array): PublicKey {
    return new PublicKey(bytes);
  }
}
