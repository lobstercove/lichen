import { createHash, randomBytes as nodeRandomBytes } from 'crypto';
import { ml_dsa65 } from '@noble/post-quantum/ml-dsa.js';

import { PublicKey } from './publickey.js';

export const PQ_SCHEME_ML_DSA_65 = 0x01;
export const ML_DSA_65_PUBLIC_KEY_BYTES = 1952;
export const ML_DSA_65_SIGNATURE_BYTES = 3309;

export interface JsonPqPublicKey {
    scheme_version: number;
    bytes: string;
}

export interface JsonPqSignature {
    scheme_version: number;
    public_key: JsonPqPublicKey;
    sig: string;
}

type BytesLike = Uint8Array | number[];
type PqPublicKeyInput = {
    schemeVersion?: number;
    scheme_version?: number;
    bytes: string | BytesLike;
};
type PqSignatureInput = {
    schemeVersion?: number;
    scheme_version?: number;
    publicKey?: PqPublicKeyInput | PqPublicKey;
    public_key?: PqPublicKeyInput | PqPublicKey;
    sig: string | BytesLike;
};

function copyBytes(bytes: Uint8Array): Uint8Array {
    return new Uint8Array(bytes);
}

function schemePublicKeyLength(schemeVersion: number): number {
    switch (schemeVersion) {
        case PQ_SCHEME_ML_DSA_65:
            return ML_DSA_65_PUBLIC_KEY_BYTES;
        default:
            throw new Error(`Unsupported PQ public key scheme: 0x${schemeVersion.toString(16).padStart(2, '0')}`);
    }
}

function schemeSignatureLength(schemeVersion: number): number {
    switch (schemeVersion) {
        case PQ_SCHEME_ML_DSA_65:
            return ML_DSA_65_SIGNATURE_BYTES;
        default:
            throw new Error(`Unsupported PQ signature scheme: 0x${schemeVersion.toString(16).padStart(2, '0')}`);
    }
}

function normalizeBytes(value: string | BytesLike, label: string): Uint8Array {
    if (typeof value === 'string') {
        return hexToBytes(value);
    }
    return new Uint8Array(value);
}

function sha256(bytes: Uint8Array): Uint8Array {
    return new Uint8Array(createHash('sha256').update(bytes).digest());
}

export function generateMlDsa65Seed(): Uint8Array {
    return new Uint8Array(nodeRandomBytes(32));
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

export class PqPublicKey {
    readonly schemeVersion: number;
    private readonly _bytes: Uint8Array;

    constructor(schemeVersion: number, bytes: string | BytesLike) {
        this.schemeVersion = schemeVersion;
        this._bytes = normalizeBytes(bytes, 'public key');

        const expectedLength = schemePublicKeyLength(this.schemeVersion);
        if (this._bytes.length !== expectedLength) {
            throw new Error(
                `Invalid PQ public key length for scheme 0x${this.schemeVersion.toString(16).padStart(2, '0')}: `
                + `${this._bytes.length}, expected ${expectedLength}`,
            );
        }
    }

    static mlDsa65(bytes: string | BytesLike): PqPublicKey {
        return new PqPublicKey(PQ_SCHEME_ML_DSA_65, bytes);
    }

    toBytes(): Uint8Array {
        return copyBytes(this._bytes);
    }

    address(): PublicKey {
        const digest = sha256(this._bytes);
        const address = new Uint8Array(32);
        address[0] = this.schemeVersion;
        address.set(digest.subarray(0, 31), 1);
        return new PublicKey(address);
    }

    equals(other: PqPublicKey): boolean {
        if (this.schemeVersion !== other.schemeVersion) {
            return false;
        }
        const left = this._bytes;
        const right = other._bytes;
        if (left.length !== right.length) {
            return false;
        }
        for (let i = 0; i < left.length; i++) {
            if (left[i] !== right[i]) {
                return false;
            }
        }
        return true;
    }

    toJSON(): JsonPqPublicKey {
        return {
            scheme_version: this.schemeVersion,
            bytes: bytesToHex(this._bytes),
        };
    }

    toString(): string {
        return JSON.stringify(this.toJSON());
    }

    static fromJSON(value: PqPublicKeyInput | PqPublicKey): PqPublicKey {
        if (value instanceof PqPublicKey) {
            return value;
        }
        const schemeVersion = value.scheme_version ?? value.schemeVersion;
        if (schemeVersion === undefined) {
            throw new Error('PQ public key is missing scheme version');
        }
        return new PqPublicKey(schemeVersion, value.bytes);
    }
}

export class PqSignature {
    readonly schemeVersion: number;
    readonly publicKey: PqPublicKey;
    private readonly _sig: Uint8Array;

    constructor(schemeVersion: number, publicKey: PqPublicKey, sig: string | BytesLike) {
        this.schemeVersion = schemeVersion;
        this.publicKey = publicKey;
        this._sig = normalizeBytes(sig, 'signature');

        if (this.publicKey.schemeVersion !== this.schemeVersion) {
            throw new Error(
                `PQ signature/public-key scheme mismatch: 0x${this.schemeVersion.toString(16).padStart(2, '0')} `
                + `vs 0x${this.publicKey.schemeVersion.toString(16).padStart(2, '0')}`,
            );
        }

        const expectedLength = schemeSignatureLength(this.schemeVersion);
        if (this._sig.length !== expectedLength) {
            throw new Error(
                `Invalid PQ signature length for scheme 0x${this.schemeVersion.toString(16).padStart(2, '0')}: `
                + `${this._sig.length}, expected ${expectedLength}`,
            );
        }
    }

    static mlDsa65(publicKey: PqPublicKey, sig: string | BytesLike): PqSignature {
        return new PqSignature(PQ_SCHEME_ML_DSA_65, publicKey, sig);
    }

    signerAddress(): PublicKey {
        return this.publicKey.address();
    }

    toBytes(): Uint8Array {
        return copyBytes(this._sig);
    }

    verify(message: Uint8Array): boolean {
        switch (this.schemeVersion) {
            case PQ_SCHEME_ML_DSA_65:
                return ml_dsa65.verify(this._sig, message, this.publicKey.toBytes());
            default:
                return false;
        }
    }

    toJSON(): JsonPqSignature {
        return {
            scheme_version: this.schemeVersion,
            public_key: this.publicKey.toJSON(),
            sig: bytesToHex(this._sig),
        };
    }

    toString(): string {
        return JSON.stringify(this.toJSON());
    }

    static fromJSON(value: PqSignatureInput | PqSignature): PqSignature {
        if (value instanceof PqSignature) {
            return value;
        }
        const schemeVersion = value.scheme_version ?? value.schemeVersion;
        if (schemeVersion === undefined) {
            throw new Error('PQ signature is missing scheme version');
        }
        const publicKey = value.public_key ?? value.publicKey;
        if (!publicKey) {
            throw new Error('PQ signature is missing public key');
        }
        return new PqSignature(schemeVersion, PqPublicKey.fromJSON(publicKey), value.sig);
    }
}

export function toPqPublicKey(value: PqPublicKeyInput | PqPublicKey): PqPublicKey {
    return PqPublicKey.fromJSON(value);
}

export function toPqSignature(value: string | PqSignatureInput | PqSignature): PqSignature {
    if (typeof value === 'string') {
        return PqSignature.fromJSON(JSON.parse(value) as JsonPqSignature);
    }
    return PqSignature.fromJSON(value);
}