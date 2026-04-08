"""Keypair utilities for Lichen."""

from __future__ import annotations

import hashlib
import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.argon2 import Argon2id
from dilithium_py.ml_dsa import ML_DSA_65

from .publickey import PublicKey
from .pq import PQ_SCHEME_ML_DSA_65, PqPublicKey, PqSignature

KEYPAIR_PASSWORD_ENV = "LICHEN_KEYPAIR_PASSWORD"
CANONICAL_ENCRYPTION_VERSION = 2
ARGON2_MEMORY_COST_KIB = 19_456
ARGON2_ITERATIONS = 2
ARGON2_LANES = 1


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


def _resolve_password(password: Optional[str]) -> Optional[str]:
    if password is not None:
        return password
    env_password = os.getenv(KEYPAIR_PASSWORD_ENV)
    return env_password or None


def _derive_argon2id_key(password: str, salt: bytes) -> bytes:
    return Argon2id(
        salt=salt,
        length=32,
        iterations=ARGON2_ITERATIONS,
        lanes=ARGON2_LANES,
        memory_cost=ARGON2_MEMORY_COST_KIB,
    ).derive(password.encode("utf-8"))


def _validate_loaded_keypair(data: dict[str, object], keypair: "Keypair", path: Path) -> None:
    expected_address = data.get("publicKeyBase58") or data.get("address_base58")
    if isinstance(expected_address, str) and expected_address:
        actual_address = keypair.pubkey().to_base58()
        if actual_address != expected_address:
            raise ValueError(
                f"Keypair file address mismatch for {path}: expected {expected_address}, got {actual_address}"
            )

    expected_public_key = data.get("publicKey")
    if expected_public_key is not None:
        normalized = _decode_seed(expected_public_key, "publicKey")
        if normalized != keypair.public_key().bytes:
            raise ValueError(f"Keypair file publicKey does not match derived key for {path}")


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
        password = _resolve_password(password)

        if data.get("encrypted"):
            if password is None:
                raise ValueError(
                    f"Keypair file is encrypted — provide a password to load() or set {KEYPAIR_PASSWORD_ENV}"
                )

            version = int(data.get("encryption_version") or 0)
            if version != CANONICAL_ENCRYPTION_VERSION:
                raise ValueError(
                    f"Unsupported canonical keypair encryption version {version}: {path}"
                )

            salt = _decode_seed(data["salt"], "salt")
            encrypted_seed = _decode_seed(data["privateKey"], "privateKey")
            if len(encrypted_seed) < 28:
                raise ValueError(f"Encrypted keypair payload is too short: {path}")

            nonce = encrypted_seed[:12]
            ciphertext = encrypted_seed[12:]
            key = _derive_argon2id_key(password, salt)
            plaintext = AESGCM(key).decrypt(nonce, ciphertext, None)
            keypair = cls.from_seed(plaintext[:32])
            _validate_loaded_keypair(data, keypair, path)
            return keypair

        if "encrypted_seed" in data:
            if password is None:
                raise ValueError(
                    f"Keypair file is encrypted — provide a password to load() or set {KEYPAIR_PASSWORD_ENV}"
                )

            salt = bytes.fromhex(data["salt"])
            nonce = bytes.fromhex(data["nonce"])
            ct = bytes.fromhex(data["encrypted_seed"])
            stored_tag = bytes.fromhex(data["tag"])
            key = hashlib.pbkdf2_hmac("sha256", password.encode("utf-8"), salt, 600_000)
            plaintext = AESGCM(key).decrypt(nonce, ct + stored_tag, None)
            keypair = cls.from_seed(plaintext[:32])
            _validate_loaded_keypair(data, keypair, path)
            return keypair

        if "seed" in data:
            seed = _decode_seed(data["seed"], "seed")
        elif "privateKey" in data:
            seed = _decode_seed(data["privateKey"], "privateKey")
        else:
            raise ValueError(
                f"Keypair file missing 'seed', 'privateKey', or 'encrypted_seed' field: {path}"
            )
        keypair = cls.from_seed(seed)
        _validate_loaded_keypair(data, keypair, path)
        return keypair

    def save(self, path: Path, password: Optional[str] = None) -> None:
        """Save the keypair seed and verifying key metadata to JSON."""
        address = self.pubkey()
        pq_public_key = self.public_key()
        password = _resolve_password(password)

        payload = {
            "privateKey": list(self._seed),
            "publicKey": list(pq_public_key.bytes),
            "publicKeyBase58": address.to_base58(),
        }

        if password is not None:
            salt = os.urandom(16)
            nonce = os.urandom(12)
            key = _derive_argon2id_key(password, salt)
            ciphertext = AESGCM(key).encrypt(nonce, self._seed, None)
            payload["privateKey"] = list(nonce + ciphertext)
            payload["encrypted"] = True
            payload["salt"] = list(salt)
            payload["encryption_version"] = CANONICAL_ENCRYPTION_VERSION

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
