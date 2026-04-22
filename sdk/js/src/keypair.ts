// Lichen SDK - Keypair utilities
// AUDIT-FIX H1-01: Private key protected from accidental exposure via
// toString(), toJSON(), and console.log(). Use getSecretKey() explicitly.

import { PublicKey } from './publickey.js';
import { bytesToHex, generateMlDsa65Seed, PqPublicKey, PqSignature } from './pq.js';
import { ml_dsa65 } from '@noble/post-quantum/ml-dsa.js';

export class Keypair {
  readonly publicKey: PqPublicKey;

  /**
   * The secret key is stored privately to prevent accidental leakage via
   * toString(), JSON.stringify(), or console.log(). Use getSecretKey()
   * when you explicitly need the raw secret key bytes.
   */
  private readonly _secretKey: Uint8Array;
  private readonly _seed: Uint8Array;

  private constructor(publicKey: Uint8Array, secretKey: Uint8Array, seed: Uint8Array) {
    this.publicKey = PqPublicKey.mlDsa65(publicKey);
    this._secretKey = new Uint8Array(secretKey);
    this._seed = new Uint8Array(seed);
  }

  static generate(): Keypair {
    return Keypair.fromSeed(generateMlDsa65Seed());
  }

  static fromSeed(seed: Uint8Array): Keypair {
    if (seed.length !== 32) {
      throw new Error('Seed must be 32 bytes');
    }
    const keypair = ml_dsa65.keygen(seed);
    return new Keypair(keypair.publicKey, keypair.secretKey, seed);
  }

  pubkey(): PublicKey {
    return this.publicKey.address();
  }

  address(): PublicKey {
    return this.pubkey();
  }

  /**
   * Returns the 32-byte seed used to derive the PQ keypair.
   *
   * **WARNING**: Handle with extreme care. Never log, serialize, or transmit
   * the returned value. Prefer using sign() instead of accessing key material
   * directly.
   */
  getSecretKey(): Uint8Array {
    return this.toSeed();
  }

  toSeed(): Uint8Array {
    return new Uint8Array(this._seed);
  }

  sign(message: Uint8Array): PqSignature {
    return PqSignature.mlDsa65(this.publicKey, ml_dsa65.sign(message, this._secretKey));
  }

  static verify(address: PublicKey, message: Uint8Array, signature: PqSignature): boolean {
    return address.equals(signature.signerAddress()) && signature.verify(message);
  }

  /**
   * Returns a safe string representation containing only the address.
   */
  toString(): string {
    return `Keypair(address: ${this.pubkey().toBase58()})`;
  }

  /**
   * Returns a JSON-safe representation containing only the address and verifying key.
   * Prevents secret key leakage via JSON.stringify().
   */
  toJSON(): { address: string; publicKey: ReturnType<PqPublicKey['toJSON']> } {
    return {
      address: this.pubkey().toBase58(),
      publicKey: this.publicKey.toJSON(),
    };
  }

  /**
   * Custom inspect for Node.js console.log() — never reveals secret key.
   */
  [Symbol.for('nodejs.util.inspect.custom')](): string {
    return this.toString();
  }
}
