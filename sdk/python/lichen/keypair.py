"""Keypair utilities for Lichen"""

from __future__ import annotations

import json
import os
import hashlib
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

import base58
from nacl.signing import SigningKey

from .publickey import PublicKey


@dataclass
class Keypair:
    _signing_key: SigningKey

    @classmethod
    def generate(cls) -> "Keypair":
        return cls(SigningKey.generate())

    @classmethod
    def from_seed(cls, seed: bytes) -> "Keypair":
        if len(seed) != 32:
            raise ValueError("Seed must be 32 bytes")
        return cls(SigningKey(seed))

    @classmethod
    def load(cls, path: Path, password: Optional[str] = None) -> "Keypair":
        data = json.loads(path.read_text())

        # P9-SDK-02: Check for encrypted format (v2)
        if data.get("version") == 2 and "encrypted_seed" in data:
            if password is None:
                raise ValueError(
                    "Keypair file is encrypted — provide a password to load()"
                )
            import hmac

            salt = bytes.fromhex(data["salt"])
            nonce = bytes.fromhex(data["nonce"])
            ct = bytes.fromhex(data["encrypted_seed"])
            stored_tag = bytes.fromhex(data["tag"])

            # Derive key via PBKDF2-HMAC-SHA256
            key = hashlib.pbkdf2_hmac("sha256", password.encode("utf-8"), salt, 600_000)

            # Decrypt via AES-256-GCM
            from cryptography.hazmat.primitives.ciphers.aead import AESGCM

            aead = AESGCM(key)
            # Verify HMAC tag (seed-bytes || pubkey-base58)
            plaintext = aead.decrypt(nonce, ct + stored_tag, None)
            seed = plaintext[:32]
            return cls.from_seed(seed)

        # Legacy cleartext (v1) — support "seed", "privateKey", and "secret_key" formats
        if "seed" in data:
            seed = bytes(data["seed"])
        elif "privateKey" in data:
            raw = bytes(data["privateKey"])
            # Rust NaCl keypair is 64 bytes (seed[32] + public[32]); extract seed
            seed = raw[:32]
        elif "secret_key" in data:
            # Genesis-generated keypairs store seed as hex string
            seed = bytes.fromhex(data["secret_key"])
        else:
            raise ValueError(
                f"Keypair file missing 'seed', 'privateKey', or 'secret_key' field: {path}"
            )
        return cls.from_seed(seed)

    def save(self, path: Path, password: Optional[str] = None) -> None:
        """Save keypair to a JSON file.

        P9-SDK-02: When *password* is provided the seed is encrypted with
        AES-256-GCM using a key derived via PBKDF2-HMAC-SHA256 (600k rounds).
        Without a password the seed is stored in cleartext (legacy v1 format)
        for quick-start scripts and test wallets.
        """
        pubkey_bytes = self.public_key().to_bytes()
        pubkey_b58 = base58.b58encode(pubkey_bytes).decode("ascii")

        if password is not None:
            salt = os.urandom(32)
            nonce = os.urandom(12)
            key = hashlib.pbkdf2_hmac(
                "sha256", password.encode("utf-8"), salt, 600_000
            )

            from cryptography.hazmat.primitives.ciphers.aead import AESGCM

            aead = AESGCM(key)
            seed_bytes = self._signing_key.encode()
            ct_and_tag = aead.encrypt(nonce, seed_bytes, None)
            # AES-GCM appends a 16-byte tag
            ct = ct_and_tag[:-16]
            tag = ct_and_tag[-16:]

            payload = {
                "version": 2,
                "pubkey_base58": pubkey_b58,
                "salt": salt.hex(),
                "nonce": nonce.hex(),
                "encrypted_seed": ct.hex(),
                "tag": tag.hex(),
            }
        else:
            payload = {
                "seed": list(self._signing_key.encode()),
                "pubkey": list(pubkey_bytes),
                "pubkey_base58": pubkey_b58,
            }

        path.write_text(json.dumps(payload, indent=2))
        # Set restrictive permissions (owner-only)
        path.chmod(0o600)

    def public_key(self) -> PublicKey:
        return PublicKey(self._signing_key.verify_key.encode())

    def sign(self, message: bytes) -> bytes:
        return self._signing_key.sign(message).signature

    def seed(self) -> bytes:
        return self._signing_key.encode()
