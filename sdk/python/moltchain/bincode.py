"""Minimal bincode encoder for MoltChain transactions"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Iterable, List

from .publickey import PublicKey


def _encode_u64(value: int) -> bytes:
    return value.to_bytes(8, byteorder="little", signed=False)


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


def _encode_hash(hex_str: str) -> bytes:
    raw = bytes.fromhex(hex_str)
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


def encode_message(instructions: List[EncodedInstruction], recent_blockhash: str) -> bytes:
    encoded_instructions = _encode_vec(encode_instruction(ix) for ix in instructions)
    blockhash = _encode_hash(recent_blockhash)
    return encoded_instructions + blockhash


def encode_transaction(signatures: List[str], message_bytes: bytes) -> bytes:
    """Encode transaction matching Rust bincode Vec<[u8; 64]> format.

    Signatures are hex strings that map to 64 raw bytes each.
    Fixed-size arrays in bincode have no per-element length prefix.
    """
    sig_bytes = [bytes.fromhex(sig) for sig in signatures]
    for sig in sig_bytes:
        if len(sig) != 64:
            raise ValueError(f"Signature must be 64 bytes, got {len(sig)}")
    return _encode_u64(len(sig_bytes)) + b"".join(sig_bytes) + message_bytes
