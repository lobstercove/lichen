// MoltChain SDK - Keypair utilities
// AUDIT-FIX H1-01: Private key protected from accidental exposure via
// toString(), toJSON(), and console.log(). Use getSecretKey() explicitly.

import nacl from 'tweetnacl';
import { PublicKey } from './publickey';

export class Keypair {
  readonly publicKey: Uint8Array;

  /**
   * The secret key is stored privately to prevent accidental leakage via
   * toString(), JSON.stringify(), or console.log(). Use getSecretKey()
   * when you explicitly need the raw secret key bytes.
   */
  private readonly _secretKey: Uint8Array;

  private constructor(publicKey: Uint8Array, secretKey: Uint8Array) {
    this.publicKey = publicKey;
    this._secretKey = secretKey;
  }

  static generate(): Keypair {
    const kp = nacl.sign.keyPair();
    return new Keypair(kp.publicKey, kp.secretKey);
  }

  static fromSeed(seed: Uint8Array): Keypair {
    if (seed.length !== 32) {
      throw new Error('Seed must be 32 bytes');
    }
    const kp = nacl.sign.keyPair.fromSeed(seed);
    return new Keypair(kp.publicKey, kp.secretKey);
  }

  pubkey(): PublicKey {
    return new PublicKey(this.publicKey);
  }

  /**
   * Returns the raw 64-byte Ed25519 secret key.
   *
   * **WARNING**: Handle with extreme care. Never log, serialize, or transmit
   * the returned value. Prefer using sign() instead of accessing the secret
   * key directly.
   */
  getSecretKey(): Uint8Array {
    return this._secretKey;
  }

  sign(message: Uint8Array): Uint8Array {
    return nacl.sign.detached(message, this._secretKey);
  }

  /**
   * Returns a safe string representation containing only the public key.
   * The secret key is never included.
   */
  toString(): string {
    const pubHex = Buffer.from(this.publicKey).toString('hex');
    return `Keypair(publicKey: ${pubHex})`;
  }

  /**
   * Returns a JSON-safe representation containing only the public key.
   * Prevents secret key leakage via JSON.stringify().
   */
  toJSON(): { publicKey: string } {
    return {
      publicKey: Buffer.from(this.publicKey).toString('hex'),
    };
  }

  /**
   * Custom inspect for Node.js console.log() — never reveals secret key.
   */
  [Symbol.for('nodejs.util.inspect.custom')](): string {
    return this.toString();
  }
}
