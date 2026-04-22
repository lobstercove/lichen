"""First-class ThallLend helper built on top of the Python SDK primitives."""

from __future__ import annotations

import base64
from dataclasses import dataclass
from typing import Any, Dict, Optional

from .connection import Connection
from .keypair import Keypair
from .publickey import PublicKey

PROGRAM_SYMBOL_CANDIDATES = ("LEND", "lend", "THALLLEND", "thalllend")
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


def _encode_user_amount_args(user: PublicKey, amount: int) -> bytes:
    return _build_layout_args([0x20, 0x08], [user.to_bytes(), _u64_le(amount, "amount")])


def _encode_user_lookup_args(user: PublicKey | str) -> bytes:
    return _build_layout_args([0x20], [_normalize_public_key(user).to_bytes()])


def _encode_liquidate_args(liquidator: PublicKey, borrower: PublicKey | str, repay_amount: int) -> bytes:
    return _build_layout_args(
        [0x20, 0x20, 0x08],
        [
            liquidator.to_bytes(),
            _normalize_public_key(borrower).to_bytes(),
            _u64_le(repay_amount, "repay_amount"),
        ],
    )


def _decode_u64_le(data: bytes, offset: int = 0) -> int:
    return int.from_bytes(data[offset : offset + 8], "little")


def _decode_return_data(value: str) -> bytes:
    return base64.b64decode(value.encode("ascii"))


def _ensure_return_code(result: Dict[str, Any], function_name: str, allowed_codes: tuple[int, ...] = (0,)) -> None:
    code = int(result.get("returnCode") or 0)
    if code not in allowed_codes:
        raise RuntimeError(result.get("error") or f"ThallLend {function_name} returned code {code}")
    if result.get("success") is False and result.get("error"):
        raise RuntimeError(str(result["error"]))


def _decode_u64_result(result: Dict[str, Any], function_name: str) -> int:
    _ensure_return_code(result, function_name)
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError(f"ThallLend {function_name} did not return payload data")
    data = _decode_return_data(return_data)
    if len(data) < 8:
        raise RuntimeError(f"ThallLend {function_name} payload was shorter than expected")
    return _decode_u64_le(data)


def _decode_account_info(result: Dict[str, Any]) -> Dict[str, int]:
    _ensure_return_code(result, "get_account_info")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("ThallLend get_account_info did not return account data")
    data = _decode_return_data(return_data)
    if len(data) < 24:
        raise RuntimeError("ThallLend get_account_info payload was shorter than expected")
    return {
        "deposit": _decode_u64_le(data, 0),
        "borrow": _decode_u64_le(data, 8),
        "health_factor_bps": _decode_u64_le(data, 16),
    }


def _decode_protocol_stats(result: Dict[str, Any]) -> Dict[str, int]:
    _ensure_return_code(result, "get_protocol_stats")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("ThallLend get_protocol_stats did not return stats data")
    data = _decode_return_data(return_data)
    if len(data) < 32:
        raise RuntimeError("ThallLend get_protocol_stats payload was shorter than expected")
    return {
        "total_deposits": _decode_u64_le(data, 0),
        "total_borrows": _decode_u64_le(data, 8),
        "utilization_pct": _decode_u64_le(data, 16),
        "reserves": _decode_u64_le(data, 24),
    }


def _decode_interest_rate(result: Dict[str, Any]) -> Dict[str, int]:
    _ensure_return_code(result, "get_interest_rate")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("ThallLend get_interest_rate did not return rate data")
    data = _decode_return_data(return_data)
    if len(data) < 24:
        raise RuntimeError("ThallLend get_interest_rate payload was shorter than expected")
    return {
        "rate_per_slot": _decode_u64_le(data, 0),
        "utilization_pct": _decode_u64_le(data, 8),
        "total_available": _decode_u64_le(data, 16),
    }


@dataclass(frozen=True)
class LiquidateParams:
    borrower: PublicKey | str
    repay_amount: int


class ThallLendClient:
    """High-level helper for common ThallLend reads and writes."""

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

        raise RuntimeError('Unable to resolve the ThallLend program via getSymbolRegistry("LEND")')

    async def get_account_info(self, user: PublicKey | str) -> Dict[str, int]:
        return _decode_account_info(await self._call_readonly("get_account_info", _encode_user_lookup_args(user)))

    async def get_protocol_stats(self) -> Dict[str, int]:
        return _decode_protocol_stats(await self._call_readonly("get_protocol_stats"))

    async def get_interest_rate(self) -> Dict[str, int]:
        return _decode_interest_rate(await self._call_readonly("get_interest_rate"))

    async def get_deposit_count(self) -> int:
        return _decode_u64_result(await self._call_readonly("get_deposit_count"), "get_deposit_count")

    async def get_borrow_count(self) -> int:
        return _decode_u64_result(await self._call_readonly("get_borrow_count"), "get_borrow_count")

    async def get_liquidation_count(self) -> int:
        return _decode_u64_result(await self._call_readonly("get_liquidation_count"), "get_liquidation_count")

    async def get_stats(self) -> Dict[str, Any]:
        stats = await self.connection.get_thalllend_stats()
        return {
            "total_deposits": stats.get("total_deposits", 0),
            "total_borrows": stats.get("total_borrows", 0),
            "reserves": stats.get("reserves", 0),
            "deposit_count": stats.get("deposit_count", 0),
            "borrow_count": stats.get("borrow_count", 0),
            "liquidation_count": stats.get("liquidation_count", 0),
            "paused": bool(stats.get("paused")),
        }

    async def deposit(self, depositor: Keypair, amount: int) -> str:
        normalized_amount = _normalize_u64(amount, "amount")
        program_id = await self.get_program_id()
        args = _encode_user_amount_args(depositor.pubkey(), normalized_amount)
        return await self.connection.call_contract(depositor, program_id, "deposit", args, normalized_amount)

    async def withdraw(self, depositor: Keypair, amount: int) -> str:
        normalized_amount = _normalize_u64(amount, "amount")
        program_id = await self.get_program_id()
        args = _encode_user_amount_args(depositor.pubkey(), normalized_amount)
        return await self.connection.call_contract(depositor, program_id, "withdraw", args)

    async def borrow(self, borrower: Keypair, amount: int) -> str:
        normalized_amount = _normalize_u64(amount, "amount")
        program_id = await self.get_program_id()
        args = _encode_user_amount_args(borrower.pubkey(), normalized_amount)
        return await self.connection.call_contract(borrower, program_id, "borrow", args)

    async def repay(self, borrower: Keypair, amount: int) -> str:
        normalized_amount = _normalize_u64(amount, "amount")
        program_id = await self.get_program_id()
        args = _encode_user_amount_args(borrower.pubkey(), normalized_amount)
        return await self.connection.call_contract(borrower, program_id, "repay", args, normalized_amount)

    async def liquidate(self, liquidator: Keypair, params: LiquidateParams) -> str:
        normalized_repay_amount = _normalize_u64(params.repay_amount, "repay_amount")
        program_id = await self.get_program_id()
        args = _encode_liquidate_args(liquidator.pubkey(), params.borrower, normalized_repay_amount)
        return await self.connection.call_contract(
            liquidator,
            program_id,
            "liquidate",
            args,
            normalized_repay_amount,
        )