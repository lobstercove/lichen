"""Keypair utilities for MoltChain"""

from __future__ import annotations

import json
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
    def load(cls, path: Path) -> "Keypair":
        data = json.loads(path.read_text())
        # Support both SDK format ("seed") and Rust validator format ("privateKey")
        if "seed" in data:
            seed = bytes(data["seed"])
        elif "privateKey" in data:
            raw = bytes(data["privateKey"])
            # Rust NaCl keypair is 64 bytes (seed[32] + public[32]); extract seed
            seed = raw[:32]
        else:
            raise ValueError(
                f"Keypair file missing 'seed' or 'privateKey' field: {path}"
            )
        return cls.from_seed(seed)

    def save(self, path: Path) -> None:
        pubkey_bytes = self.public_key().to_bytes()
        payload = {
            "seed": list(self._signing_key.encode()),
            "pubkey": list(pubkey_bytes),
            "pubkey_base58": base58.b58encode(pubkey_bytes).decode("ascii"),
        }
        path.write_text(json.dumps(payload, indent=2))

    def public_key(self) -> PublicKey:
        return PublicKey(self._signing_key.verify_key.encode())

    def sign(self, message: bytes) -> bytes:
        return self._signing_key.sign(message).signature

    def seed(self) -> bytes:
        return self._signing_key.encode()
