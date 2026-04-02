"""Minimal bincode encoder for Lichen transactions."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Iterable, List, Optional

from .pq import PqPublicKey, PqSignature, to_pq_signature
from .publickey import PublicKey


def _encode_u64(value: int) -> bytes:
    return value.to_bytes(8, byteorder="little", signed=False)


def _encode_u32(value: int) -> bytes:
    return value.to_bytes(4, byteorder="little", signed=False)


def _encode_u8(value: int) -> bytes:
    return bytes((value & 0xFF,))


def _encode_option_u64(value: Optional[int]) -> bytes:
    if value is None:
        return b"\x00"
    return b"\x01" + _encode_u64(value)


def _encode_bytes(data: bytes) -> bytes:
    return _encode_u64(len(data)) + data


def _encode_string(value: str) -> bytes:
    encoded = value.encode("utf-8")
    return _encode_u64(len(encoded)) + encoded


def _encode_vec(items: Iterable[bytes]) -> bytes:
    items_list = list(items)
    return _encode_u64(len(items_list)) + b"".join(items_list)


def _encode_pubkey(pubkey: PublicKey) -> bytes:
    raw = pubkey.to_bytes()
    if len(raw) != 32:
        raise ValueError("PublicKey must be 32 bytes")
    return raw


def _encode_pq_public_key(public_key: PqPublicKey) -> bytes:
    return _encode_u8(public_key.scheme_version) + _encode_bytes(public_key.bytes)


def _encode_pq_signature(signature: PqSignature) -> bytes:
    return (
        _encode_u8(signature.scheme_version)
        + _encode_pq_public_key(signature.public_key)
        + _encode_bytes(signature.sig)
    )


def _encode_hash(hex_str: str) -> bytes:
    raw = bytes.fromhex(hex_str.removeprefix("0x"))
    if len(raw) != 32:
        raise ValueError("Blockhash must be 32 bytes")
    return raw


@dataclass
class EncodedInstruction:
    program_id: PublicKey
    accounts: List[PublicKey]
    data: bytes


def encode_instruction(ix: EncodedInstruction) -> bytes:
    program_id = _encode_pubkey(ix.program_id)
    accounts = _encode_vec(_encode_pubkey(acc) for acc in ix.accounts)
    data = _encode_bytes(ix.data)
    return program_id + accounts + data


def encode_message(
    instructions: List[EncodedInstruction],
    recent_blockhash: str,
    compute_budget: Optional[int] = None,
    compute_unit_price: Optional[int] = None,
) -> bytes:
    encoded_instructions = _encode_vec(encode_instruction(ix) for ix in instructions)
    blockhash = _encode_hash(recent_blockhash)
    budget = _encode_option_u64(compute_budget)
    cu_price = _encode_option_u64(compute_unit_price)
    return encoded_instructions + blockhash + budget + cu_price


def encode_transaction(
    signatures: List[PqSignature],
    message_bytes: bytes,
    tx_type: int = 0,
) -> bytes:
    """Encode transaction matching Rust bincode format."""
    sig_bytes = [_encode_pq_signature(to_pq_signature(signature)) for signature in signatures]
    return _encode_u64(len(sig_bytes)) + b"".join(sig_bytes) + message_bytes + _encode_u32(tx_type)
