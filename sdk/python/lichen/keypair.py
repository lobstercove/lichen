"""Keypair utilities for Lichen."""

from __future__ import annotations

import hashlib
import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

from dilithium_py.ml_dsa import ML_DSA_65

from .publickey import PublicKey
from .pq import PQ_SCHEME_ML_DSA_65, PqPublicKey, PqSignature


def _decode_seed(value: object, field_name: str) -> bytes:
    if isinstance(value, str):
        return bytes.fromhex(value.removeprefix("0x"))
    if isinstance(value, list):
        return bytes(value)
    if isinstance(value, memoryview):
        return value.tobytes()
    if isinstance(value, (bytes, bytearray)):
        return bytes(value)
    raise ValueError(f"{field_name} must be bytes, hex string, or list of integers")


@dataclass(repr=False)
class Keypair:
    _seed: bytes
    _public_key: bytes
    _secret_key: bytes

    @classmethod
    def generate(cls) -> "Keypair":
        return cls.from_seed(os.urandom(32))

    @classmethod
    def from_seed(cls, seed: bytes) -> "Keypair":
        seed = _decode_seed(seed, "seed")
        if len(seed) != 32:
            raise ValueError("Seed must be 32 bytes")
        public_key, secret_key = ML_DSA_65.key_derive(seed)
        return cls(bytes(seed), bytes(public_key), bytes(secret_key))

    @classmethod
    def load(cls, path: Path, password: Optional[str] = None) -> "Keypair":
        data = json.loads(path.read_text())

        if "encrypted_seed" in data:
            if password is None:
                raise ValueError(
                    "Keypair file is encrypted — provide a password to load()"
                )

            salt = bytes.fromhex(data["salt"])
            nonce = bytes.fromhex(data["nonce"])
            ct = bytes.fromhex(data["encrypted_seed"])
            stored_tag = bytes.fromhex(data["tag"])
            key = hashlib.pbkdf2_hmac("sha256", password.encode("utf-8"), salt, 600_000)

            from cryptography.hazmat.primitives.ciphers.aead import AESGCM

            aead = AESGCM(key)
            plaintext = aead.decrypt(nonce, ct + stored_tag, None)
            return cls.from_seed(plaintext[:32])

        if "seed" in data:
            seed = _decode_seed(data["seed"], "seed")
        elif "privateKey" in data:
            seed = _decode_seed(data["privateKey"], "privateKey")
        else:
            raise ValueError(
                f"Keypair file missing 'seed', 'privateKey', or 'encrypted_seed' field: {path}"
            )
        return cls.from_seed(seed)

    def save(self, path: Path, password: Optional[str] = None) -> None:
        """Save the keypair seed and verifying key metadata to JSON."""
        address = self.pubkey()
        pq_public_key = self.public_key()

        if password is not None:
            salt = os.urandom(32)
            nonce = os.urandom(12)
            key = hashlib.pbkdf2_hmac(
                "sha256", password.encode("utf-8"), salt, 600_000
            )

            from cryptography.hazmat.primitives.ciphers.aead import AESGCM

            aead = AESGCM(key)
            ct_and_tag = aead.encrypt(nonce, self._seed, None)
            ct = ct_and_tag[:-16]
            tag = ct_and_tag[-16:]

            payload = {
                "version": 3,
                "scheme_version": PQ_SCHEME_ML_DSA_65,
                "address": list(address.to_bytes()),
                "address_base58": address.to_base58(),
                "public_key": pq_public_key.to_json(),
                "salt": salt.hex(),
                "nonce": nonce.hex(),
                "encrypted_seed": ct.hex(),
                "tag": tag.hex(),
            }
        else:
            payload = {
                "version": 3,
                "scheme_version": PQ_SCHEME_ML_DSA_65,
                "seed": list(self._seed),
                "address": list(address.to_bytes()),
                "address_base58": address.to_base58(),
                "public_key": pq_public_key.to_json(),
            }

        path.write_text(json.dumps(payload, indent=2))
        path.chmod(0o600)

    def public_key(self) -> PqPublicKey:
        return PqPublicKey.ml_dsa65(self._public_key)

    def pubkey(self) -> PublicKey:
        return self.public_key().address()

    def address(self) -> PublicKey:
        return self.pubkey()

    def sign(self, message: bytes) -> PqSignature:
        return PqSignature.ml_dsa65(
            self.public_key(),
            ML_DSA_65.sign(self._secret_key, message, deterministic=True),
        )

    @staticmethod
    def verify(address: PublicKey, message: bytes, signature: PqSignature) -> bool:
        return address == signature.signer_address() and signature.verify(message)

    def seed(self) -> bytes:
        return bytes(self._seed)

    def to_seed(self) -> bytes:
        return self.seed()

    def __str__(self) -> str:
        return f"Keypair(address='{self.pubkey().to_base58()}')"

    def __repr__(self) -> str:
        return str(self)
