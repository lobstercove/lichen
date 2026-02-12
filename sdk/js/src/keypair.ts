// MoltChain SDK - Keypair utilities

import nacl from 'tweetnacl';
import { PublicKey } from './publickey';

export class Keypair {
  readonly publicKey: Uint8Array;
  readonly secretKey: Uint8Array;

  private constructor(publicKey: Uint8Array, secretKey: Uint8Array) {
    this.publicKey = publicKey;
    this.secretKey = secretKey;
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

  sign(message: Uint8Array): Uint8Array {
    return nacl.sign.detached(message, this.secretKey);
  }
}
