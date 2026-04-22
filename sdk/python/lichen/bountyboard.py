"""First-class BountyBoard helper built on top of the Python SDK primitives."""

from __future__ import annotations

import base64
from typing import Any, Dict, Optional

from .connection import Connection
from .keypair import Keypair
from .publickey import PublicKey

PROGRAM_SYMBOL_CANDIDATES = ("BOUNTY", "bounty", "BountyBoard", "BOUNTYBOARD", "bountyboard")
MAX_U64 = (1 << 64) - 1
BOUNTY_DATA_SIZE = 91
PLATFORM_STATS_SIZE = 32

# Bounty status constants
BOUNTY_STATUS_OPEN = 0
BOUNTY_STATUS_COMPLETED = 1
BOUNTY_STATUS_CANCELLED = 2


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


def _ensure_bytes32(value: bytes, field_name: str) -> bytes:
    if len(value) != 32:
        raise ValueError(f"{field_name} must be exactly 32 bytes")
    return value


def _decode_u64_le(data: bytes, offset: int = 0) -> int:
    return int.from_bytes(data[offset : offset + 8], "little")


def _decode_return_data(value: str) -> bytes:
    return base64.b64decode(value.encode("ascii"))


def _ensure_return_code(result: Dict[str, Any], function_name: str, allowed_codes: tuple[int, ...] = (0,)) -> None:
    code = int(result.get("returnCode") or 0)
    if code not in allowed_codes:
        raise RuntimeError(result.get("error") or f"BountyBoard {function_name} returned code {code}")
    if result.get("success") is False and result.get("error"):
        raise RuntimeError(str(result["error"]))


# --- Encoding helpers ---

def _encode_create_bounty_args(creator: PublicKey, title_hash: bytes, reward_amount: int, deadline_slot: int) -> bytes:
    return _build_layout_args(
        [0x20, 0x20, 0x08, 0x08],
        [creator.to_bytes(), _ensure_bytes32(title_hash, "title_hash"), _u64_le(reward_amount, "reward_amount"), _u64_le(deadline_slot, "deadline_slot")],
    )


def _encode_submit_work_args(bounty_id: int, worker: PublicKey, proof_hash: bytes) -> bytes:
    return _build_layout_args(
        [0x08, 0x20, 0x20],
        [_u64_le(bounty_id, "bounty_id"), worker.to_bytes(), _ensure_bytes32(proof_hash, "proof_hash")],
    )


def _encode_approve_work_args(caller: PublicKey, bounty_id: int, submission_idx: int) -> bytes:
    if submission_idx < 0 or submission_idx > 255:
        raise ValueError("submission_idx must be 0-255")
    return _build_layout_args(
        [0x20, 0x08, 0x01],
        [caller.to_bytes(), _u64_le(bounty_id, "bounty_id"), bytes([submission_idx])],
    )


def _encode_cancel_bounty_args(caller: PublicKey, bounty_id: int) -> bytes:
    return _build_layout_args(
        [0x20, 0x08],
        [caller.to_bytes(), _u64_le(bounty_id, "bounty_id")],
    )


def _encode_bounty_id_args(bounty_id: int) -> bytes:
    return _build_layout_args([0x08], [_u64_le(bounty_id, "bounty_id")])


# --- Decoding helpers ---

def _decode_bounty_info(result: Dict[str, Any]) -> Dict[str, Any]:
    _ensure_return_code(result, "get_bounty")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("BountyBoard get_bounty did not return bounty data")
    data = _decode_return_data(return_data)
    if len(data) < BOUNTY_DATA_SIZE:
        raise RuntimeError("BountyBoard get_bounty payload was shorter than expected")
    return {
        "creator": data[0:32],
        "title_hash": data[32:64],
        "reward_amount": _decode_u64_le(data, 64),
        "deadline_slot": _decode_u64_le(data, 72),
        "status": data[80],
        "submission_count": data[81],
        "created_slot": _decode_u64_le(data, 82),
        "approved_idx": data[90],
    }


def _decode_platform_stats(result: Dict[str, Any]) -> Dict[str, int]:
    _ensure_return_code(result, "get_platform_stats")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("BountyBoard get_platform_stats did not return stats data")
    data = _decode_return_data(return_data)
    if len(data) < PLATFORM_STATS_SIZE:
        raise RuntimeError("BountyBoard get_platform_stats payload was shorter than expected")
    return {
        "bounty_count": _decode_u64_le(data, 0),
        "completed_count": _decode_u64_le(data, 8),
        "reward_volume": _decode_u64_le(data, 16),
        "cancel_count": _decode_u64_le(data, 24),
    }


class BountyBoardClient:
    """High-level helper for common BountyBoard reads and writes."""

    def __init__(self, connection: Connection, program_id: Optional[PublicKey] = None):
        self.connection = connection
        self._program_id = program_id

    async def _call_readonly(self, function_name: str, args: bytes = b"") -> Dict[str, Any]:
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

        raise RuntimeError('Unable to resolve the BountyBoard program via getSymbolRegistry("BOUNTY")')

    # --- Read methods ---

    async def get_bounty(self, bounty_id: int) -> Optional[Dict[str, Any]]:
        normalized_id = _normalize_u64(bounty_id, "bounty_id")
        result = await self._call_readonly("get_bounty", _encode_bounty_id_args(normalized_id))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        return _decode_bounty_info(result)

    async def get_bounty_count(self) -> int:
        result = await self._call_readonly("get_bounty_count")
        _ensure_return_code(result, "get_bounty_count")
        return_data = result.get("returnData")
        if not isinstance(return_data, str):
            return 0
        data = _decode_return_data(return_data)
        if len(data) < 8:
            return 0
        return _decode_u64_le(data, 0)

    async def get_platform_stats(self) -> Dict[str, int]:
        return _decode_platform_stats(await self._call_readonly("get_platform_stats"))

    async def get_stats(self) -> Dict[str, Any]:
        stats = await self.connection.get_bountyboard_stats()
        return {
            "bounty_count": stats.get("bounty_count", 0),
            "completed_count": stats.get("completed_count", 0),
            "total_reward_volume": stats.get("total_reward_volume", 0),
            "cancel_count": stats.get("cancel_count", 0),
            "paused": bool(stats.get("paused")),
        }

    # --- Write methods ---

    async def create_bounty(self, creator: Keypair, title_hash: bytes, reward_amount: int, deadline_slot: int) -> str:
        normalized_reward = _normalize_u64(reward_amount, "reward_amount")
        normalized_deadline = _normalize_u64(deadline_slot, "deadline_slot")
        program_id = await self.get_program_id()
        args = _encode_create_bounty_args(creator.pubkey(), title_hash, normalized_reward, normalized_deadline)
        return await self.connection.call_contract(creator, program_id, "create_bounty", args, normalized_reward)

    async def submit_work(self, worker: Keypair, bounty_id: int, proof_hash: bytes) -> str:
        normalized_id = _normalize_u64(bounty_id, "bounty_id")
        program_id = await self.get_program_id()
        args = _encode_submit_work_args(normalized_id, worker.pubkey(), proof_hash)
        return await self.connection.call_contract(worker, program_id, "submit_work", args)

    async def approve_work(self, creator: Keypair, bounty_id: int, submission_idx: int) -> str:
        normalized_id = _normalize_u64(bounty_id, "bounty_id")
        program_id = await self.get_program_id()
        args = _encode_approve_work_args(creator.pubkey(), normalized_id, submission_idx)
        return await self.connection.call_contract(creator, program_id, "approve_work", args)

    async def cancel_bounty(self, creator: Keypair, bounty_id: int) -> str:
        normalized_id = _normalize_u64(bounty_id, "bounty_id")
        program_id = await self.get_program_id()
        args = _encode_cancel_bounty_args(creator.pubkey(), normalized_id)
        return await self.connection.call_contract(creator, program_id, "cancel_bounty", args)
