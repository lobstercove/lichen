"""First-class SporePay helper built on top of the Python SDK primitives."""

from __future__ import annotations

import base64
from dataclasses import dataclass
from typing import Any, Dict, Optional

from .connection import Connection
from .keypair import Keypair
from .publickey import PublicKey

PROGRAM_SYMBOL_CANDIDATES = ("SPOREPAY", "sporepay")
STREAM_SIZE = 105
STREAM_INFO_SIZE = 113
MAX_U64 = (1 << 64) - 1


def _normalize_public_key(value: PublicKey | str) -> PublicKey:
    return value if isinstance(value, PublicKey) else PublicKey(value)


def _normalize_u64(value: int, field_name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0 or value > MAX_U64:
        raise ValueError(f"{field_name} must be a u64-safe integer value")
    return value


def _u64_le(value: int, field_name: str) -> bytes:
    return _normalize_u64(value, field_name).to_bytes(8, "little")


def _build_layout_args(layout: list[int], chunks: list[bytes]) -> bytes:
    return bytes([0xAB, *layout]) + b"".join(chunks)


def _encode_create_stream_args(sender: PublicKey, recipient: PublicKey, total_amount: int, start_slot: int, end_slot: int) -> bytes:
    return _build_layout_args([
        0x20, 0x20, 0x08, 0x08, 0x08,
    ], [
        sender.to_bytes(),
        recipient.to_bytes(),
        _u64_le(total_amount, "total_amount"),
        _u64_le(start_slot, "start_slot"),
        _u64_le(end_slot, "end_slot"),
    ])


def _encode_create_stream_with_cliff_args(
    sender: PublicKey,
    recipient: PublicKey,
    total_amount: int,
    start_slot: int,
    end_slot: int,
    cliff_slot: int,
) -> bytes:
    return _build_layout_args([
        0x20, 0x20, 0x08, 0x08, 0x08, 0x08,
    ], [
        sender.to_bytes(),
        recipient.to_bytes(),
        _u64_le(total_amount, "total_amount"),
        _u64_le(start_slot, "start_slot"),
        _u64_le(end_slot, "end_slot"),
        _u64_le(cliff_slot, "cliff_slot"),
    ])


def _encode_withdraw_args(caller: PublicKey, stream_id: int, amount: int) -> bytes:
    return _build_layout_args([
        0x20, 0x08, 0x08,
    ], [
        caller.to_bytes(),
        _u64_le(stream_id, "stream_id"),
        _u64_le(amount, "amount"),
    ])


def _encode_cancel_args(caller: PublicKey, stream_id: int) -> bytes:
    return _build_layout_args([
        0x20, 0x08,
    ], [
        caller.to_bytes(),
        _u64_le(stream_id, "stream_id"),
    ])


def _encode_transfer_args(caller: PublicKey, new_recipient: PublicKey, stream_id: int) -> bytes:
    return _build_layout_args([
        0x20, 0x20, 0x08,
    ], [
        caller.to_bytes(),
        new_recipient.to_bytes(),
        _u64_le(stream_id, "stream_id"),
    ])


def _encode_stream_lookup_args(stream_id: int) -> bytes:
    return _build_layout_args([0x08], [_u64_le(stream_id, "stream_id")])


def _decode_u64_le(data: bytes, offset: int = 0) -> int:
    return int.from_bytes(data[offset : offset + 8], "little")


def _decode_return_data(value: str) -> bytes:
    return base64.b64decode(value.encode("ascii"))


def _ensure_return_code_zero(result: Dict[str, Any], function_name: str) -> None:
    code = int(result.get("returnCode") or 0)
    if code != 0:
        raise RuntimeError(result.get("error") or f"SporePay {function_name} returned code {code}")
    if result.get("success") is False and result.get("error"):
        raise RuntimeError(str(result["error"]))


def _decode_stream(stream_id: int, data: bytes) -> Dict[str, Any]:
    if len(data) < STREAM_SIZE:
        raise RuntimeError("SporePay stream payload was shorter than expected")
    return {
        "stream_id": stream_id,
        "sender": str(PublicKey(data[0:32])),
        "recipient": str(PublicKey(data[32:64])),
        "total_amount": _decode_u64_le(data, 64),
        "withdrawn_amount": _decode_u64_le(data, 72),
        "start_slot": _decode_u64_le(data, 80),
        "end_slot": _decode_u64_le(data, 88),
        "cancelled": data[96] == 1,
        "created_slot": _decode_u64_le(data, 97),
    }


def _decode_stream_info(stream_id: int, data: bytes) -> Dict[str, Any]:
    if len(data) < STREAM_INFO_SIZE:
        raise RuntimeError("SporePay stream-info payload was shorter than expected")
    stream = _decode_stream(stream_id, data)
    stream["cliff_slot"] = _decode_u64_le(data, 105)
    return stream


@dataclass(frozen=True)
class CreateStreamParams:
    recipient: PublicKey | str
    total_amount: int
    start_slot: int
    end_slot: int


@dataclass(frozen=True)
class CreateStreamWithCliffParams:
    recipient: PublicKey | str
    total_amount: int
    start_slot: int
    end_slot: int
    cliff_slot: int


@dataclass(frozen=True)
class WithdrawFromStreamParams:
    stream_id: int
    amount: int


@dataclass(frozen=True)
class TransferStreamParams:
    stream_id: int
    new_recipient: PublicKey | str


class SporePayClient:
    """High-level helper for common SporePay reads and writes."""

    def __init__(self, connection: Connection, program_id: Optional[PublicKey] = None):
        self.connection = connection
        self._program_id = program_id

    async def _call_readonly(self, function_name: str, args: bytes) -> Dict[str, Any]:
        program_id = await self.get_program_id()
        return await self.connection.call_readonly_contract(program_id, function_name, args)

    async def get_program_id(self) -> PublicKey:
        if self._program_id is not None:
            return self._program_id

        for symbol in PROGRAM_SYMBOL_CANDIDATES:
            try:
                entry = await self.connection.get_symbol_registry(symbol)
            except Exception:
                continue
            program = entry.get("program") if isinstance(entry, dict) else None
            if program:
                self._program_id = PublicKey(program)
                return self._program_id

        raise RuntimeError('Unable to resolve the SporePay program via getSymbolRegistry("SPOREPAY")')

    async def get_stream(self, stream_id: int) -> Optional[Dict[str, Any]]:
        normalized_stream_id = _normalize_u64(stream_id, "stream_id")
        result = await self._call_readonly("get_stream", _encode_stream_lookup_args(normalized_stream_id))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        _ensure_return_code_zero(result, "get_stream")
        return _decode_stream(normalized_stream_id, _decode_return_data(result["returnData"]))

    async def get_stream_info(self, stream_id: int) -> Optional[Dict[str, Any]]:
        normalized_stream_id = _normalize_u64(stream_id, "stream_id")
        result = await self._call_readonly("get_stream_info", _encode_stream_lookup_args(normalized_stream_id))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        _ensure_return_code_zero(result, "get_stream_info")
        return _decode_stream_info(normalized_stream_id, _decode_return_data(result["returnData"]))

    async def get_withdrawable(self, stream_id: int) -> int:
        normalized_stream_id = _normalize_u64(stream_id, "stream_id")
        result = await self._call_readonly("get_withdrawable", _encode_stream_lookup_args(normalized_stream_id))
        _ensure_return_code_zero(result, "get_withdrawable")
        return_data = result.get("returnData")
        if not isinstance(return_data, str):
            raise RuntimeError("SporePay get_withdrawable did not return a balance")
        return _decode_u64_le(_decode_return_data(return_data))

    async def get_stats(self) -> Dict[str, Any]:
        stats = await self.connection.get_sporepay_stats()
        return {
            "stream_count": stats.get("stream_count", 0),
            "total_streamed": stats.get("total_streamed", 0),
            "total_withdrawn": stats.get("total_withdrawn", 0),
            "cancel_count": stats.get("cancel_count", 0),
            "paused": bool(stats.get("paused")),
        }

    async def create_stream(self, sender: Keypair, params: CreateStreamParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_create_stream_args(
            sender.pubkey(),
            _normalize_public_key(params.recipient),
            params.total_amount,
            params.start_slot,
            params.end_slot,
        )
        return await self.connection.call_contract(sender, program_id, "create_stream", args)

    async def create_stream_with_cliff(self, sender: Keypair, params: CreateStreamWithCliffParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_create_stream_with_cliff_args(
            sender.pubkey(),
            _normalize_public_key(params.recipient),
            params.total_amount,
            params.start_slot,
            params.end_slot,
            params.cliff_slot,
        )
        return await self.connection.call_contract(sender, program_id, "create_stream_with_cliff", args)

    async def withdraw_from_stream(self, recipient: Keypair, params: WithdrawFromStreamParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_withdraw_args(recipient.pubkey(), params.stream_id, params.amount)
        return await self.connection.call_contract(recipient, program_id, "withdraw_from_stream", args)

    async def cancel_stream(self, sender: Keypair, stream_id: int) -> str:
        program_id = await self.get_program_id()
        args = _encode_cancel_args(sender.pubkey(), stream_id)
        return await self.connection.call_contract(sender, program_id, "cancel_stream", args)

    async def transfer_stream(self, recipient: Keypair, params: TransferStreamParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_transfer_args(recipient.pubkey(), _normalize_public_key(params.new_recipient), params.stream_id)
        return await self.connection.call_contract(recipient, program_id, "transfer_stream", args)