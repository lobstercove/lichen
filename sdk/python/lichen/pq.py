"""Native PQ key and signature types for the Lichen Python SDK."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from typing import Any

from dilithium_py.ml_dsa import ML_DSA_65

from .publickey import PublicKey

PQ_SCHEME_ML_DSA_65 = 0x01
ML_DSA_65_PUBLIC_KEY_BYTES = 1952
ML_DSA_65_SIGNATURE_BYTES = 3309


def _normalize_bytes(value: Any, label: str) -> bytes:
    if isinstance(value, str):
        return bytes.fromhex(value.removeprefix("0x"))
    if isinstance(value, memoryview):
        return value.tobytes()
    if isinstance(value, (bytes, bytearray)):
        return bytes(value)
    if isinstance(value, list):
        return bytes(value)
    raise TypeError(f"{label} must be bytes, hex string, or list of integers")


def _public_key_length_for_scheme(scheme_version: int) -> int:
    if scheme_version == PQ_SCHEME_ML_DSA_65:
        return ML_DSA_65_PUBLIC_KEY_BYTES
    raise ValueError(f"Unsupported PQ public key scheme: 0x{scheme_version:02x}")


def _signature_length_for_scheme(scheme_version: int) -> int:
    if scheme_version == PQ_SCHEME_ML_DSA_65:
        return ML_DSA_65_SIGNATURE_BYTES
    raise ValueError(f"Unsupported PQ signature scheme: 0x{scheme_version:02x}")


@dataclass(repr=False)
class PqPublicKey:
    scheme_version: int
    bytes: bytes

    def __post_init__(self) -> None:
        self.bytes = _normalize_bytes(self.bytes, "PQ public key")
        expected_length = _public_key_length_for_scheme(self.scheme_version)
        if len(self.bytes) != expected_length:
            raise ValueError(
                f"Invalid PQ public key length for scheme 0x{self.scheme_version:02x}: "
                f"{len(self.bytes)} (expected {expected_length})"
            )

    @classmethod
    def ml_dsa65(cls, value: Any) -> "PqPublicKey":
        return cls(PQ_SCHEME_ML_DSA_65, value)

    @classmethod
    def from_json(cls, value: Any) -> "PqPublicKey":
        if isinstance(value, cls):
            return value
        if not isinstance(value, dict):
            raise TypeError("PQ public key must be a dict or PqPublicKey instance")
        scheme_version = value.get("scheme_version", value.get("schemeVersion"))
        if scheme_version is None:
            raise ValueError("PQ public key is missing scheme version")
        return cls(int(scheme_version), value["bytes"])

    def address(self) -> PublicKey:
        digest = hashlib.sha256(self.bytes).digest()
        return PublicKey(bytes([self.scheme_version]) + digest[:31])

    def to_json(self) -> dict[str, object]:
        return {
            "scheme_version": self.scheme_version,
            "bytes": self.bytes.hex(),
        }

    def __str__(self) -> str:
        return json.dumps(self.to_json())


@dataclass(repr=False)
class PqSignature:
    scheme_version: int
    public_key: PqPublicKey
    sig: bytes

    def __post_init__(self) -> None:
        self.public_key = to_pq_public_key(self.public_key)
        self.sig = _normalize_bytes(self.sig, "PQ signature")

        if self.public_key.scheme_version != self.scheme_version:
            raise ValueError(
                f"PQ signature/public-key scheme mismatch: 0x{self.scheme_version:02x} "
                f"vs 0x{self.public_key.scheme_version:02x}"
            )

        expected_length = _signature_length_for_scheme(self.scheme_version)
        if len(self.sig) != expected_length:
            raise ValueError(
                f"Invalid PQ signature length for scheme 0x{self.scheme_version:02x}: "
                f"{len(self.sig)} (expected {expected_length})"
            )

    @classmethod
    def ml_dsa65(cls, public_key: PqPublicKey, sig: Any) -> "PqSignature":
        return cls(PQ_SCHEME_ML_DSA_65, public_key, sig)

    @classmethod
    def from_json(cls, value: Any) -> "PqSignature":
        if isinstance(value, cls):
            return value
        if not isinstance(value, dict):
            raise TypeError("PQ signature must be a dict or PqSignature instance")
        scheme_version = value.get("scheme_version", value.get("schemeVersion"))
        if scheme_version is None:
            raise ValueError("PQ signature is missing scheme version")
        public_key = value.get("public_key", value.get("publicKey"))
        if public_key is None:
            raise ValueError("PQ signature is missing public key")
        return cls(int(scheme_version), to_pq_public_key(public_key), value["sig"])

    def signer_address(self) -> PublicKey:
        return self.public_key.address()

    def verify(self, message: bytes) -> bool:
        if self.scheme_version != PQ_SCHEME_ML_DSA_65:
            return False
        return ML_DSA_65.verify(self.public_key.bytes, message, self.sig)

    def to_json(self) -> dict[str, object]:
        return {
            "scheme_version": self.scheme_version,
            "public_key": self.public_key.to_json(),
            "sig": self.sig.hex(),
        }

    def __str__(self) -> str:
        return json.dumps(self.to_json())


def to_pq_public_key(value: Any) -> PqPublicKey:
    return PqPublicKey.from_json(value)


def to_pq_signature(value: Any) -> PqSignature:
    if isinstance(value, str):
        return PqSignature.from_json(json.loads(value))
    return PqSignature.from_json(value)