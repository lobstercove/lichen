"""First-class SporeVault helper built on top of the Python SDK primitives."""

from __future__ import annotations

import base64
from typing import Any, Dict, Optional

from .connection import Connection
from .keypair import Keypair
from .publickey import PublicKey

PROGRAM_SYMBOL_CANDIDATES = ("SPOREVAULT", "sporevault", "SporeVault", "VAULT", "vault")
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


def _encode_index_args(index: int) -> bytes:
    return _build_layout_args([0x08], [_u64_le(index, "index")])


def _decode_u64_le(data: bytes, offset: int = 0) -> int:
    return int.from_bytes(data[offset : offset + 8], "little")


def _decode_return_data(value: str) -> bytes:
    return base64.b64decode(value.encode("ascii"))


def _ensure_return_code(result: Dict[str, Any], function_name: str, allowed_codes: tuple[int, ...] = (0,)) -> None:
    code = int(result.get("returnCode") or 0)
    if code not in allowed_codes:
        raise RuntimeError(result.get("error") or f"SporeVault {function_name} returned code {code}")
    if result.get("success") is False and result.get("error"):
        raise RuntimeError(str(result["error"]))


def _decode_vault_stats(result: Dict[str, Any]) -> Dict[str, int]:
    _ensure_return_code(result, "get_vault_stats")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("SporeVault get_vault_stats did not return vault data")
    data = _decode_return_data(return_data)
    if len(data) < 48:
        raise RuntimeError("SporeVault get_vault_stats payload was shorter than expected")
    return {
        "total_assets": _decode_u64_le(data, 0),
        "total_shares": _decode_u64_le(data, 8),
        "share_price_e9": _decode_u64_le(data, 16),
        "strategy_count": _decode_u64_le(data, 24),
        "total_earned": _decode_u64_le(data, 32),
        "fees_earned": _decode_u64_le(data, 40),
    }


def _decode_user_position(result: Dict[str, Any]) -> Dict[str, int]:
    _ensure_return_code(result, "get_user_position")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("SporeVault get_user_position did not return user data")
    data = _decode_return_data(return_data)
    if len(data) < 16:
        raise RuntimeError("SporeVault get_user_position payload was shorter than expected")
    return {
        "shares": _decode_u64_le(data, 0),
        "estimated_value": _decode_u64_le(data, 8),
    }


def _decode_strategy_info(result: Dict[str, Any]) -> Dict[str, int]:
    _ensure_return_code(result, "get_strategy_info")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("SporeVault get_strategy_info did not return strategy data")
    data = _decode_return_data(return_data)
    if len(data) < 24:
        raise RuntimeError("SporeVault get_strategy_info payload was shorter than expected")
    return {
        "strategy_type": _decode_u64_le(data, 0),
        "allocation_percent": _decode_u64_le(data, 8),
        "deployed_amount": _decode_u64_le(data, 16),
    }


class SporeVaultClient:
    """High-level helper for common SporeVault reads and writes."""

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

        raise RuntimeError('Unable to resolve the SporeVault program via getSymbolRegistry("SPOREVAULT")')

    async def get_vault_stats(self) -> Dict[str, int]:
        return _decode_vault_stats(await self._call_readonly("get_vault_stats"))

    async def get_user_position(self, user: PublicKey | str) -> Dict[str, int]:
        return _decode_user_position(await self._call_readonly("get_user_position", _encode_user_lookup_args(user)))

    async def get_strategy_info(self, index: int) -> Optional[Dict[str, int]]:
        normalized_index = _normalize_u64(index, "index")
        result = await self._call_readonly("get_strategy_info", _encode_index_args(normalized_index))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        return _decode_strategy_info(result)

    async def get_stats(self) -> Dict[str, Any]:
        stats = await self.connection.get_sporevault_stats()
        return {
            "total_assets": stats.get("total_assets", 0),
            "total_shares": stats.get("total_shares", 0),
            "strategy_count": stats.get("strategy_count", 0),
            "total_earned": stats.get("total_earned", 0),
            "fees_earned": stats.get("fees_earned", 0),
            "protocol_fees": stats.get("protocol_fees", 0),
            "paused": bool(stats.get("paused")),
        }

    async def deposit(self, depositor: Keypair, amount: int) -> str:
        normalized_amount = _normalize_u64(amount, "amount")
        program_id = await self.get_program_id()
        args = _encode_user_amount_args(depositor.pubkey(), normalized_amount)
        return await self.connection.call_contract(depositor, program_id, "deposit", args, normalized_amount)

    async def withdraw(self, depositor: Keypair, shares_to_burn: int) -> str:
        normalized_shares = _normalize_u64(shares_to_burn, "shares_to_burn")
        program_id = await self.get_program_id()
        args = _encode_user_amount_args(depositor.pubkey(), normalized_shares)
        return await self.connection.call_contract(depositor, program_id, "withdraw", args)

    async def harvest(self, caller: Keypair) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(caller, program_id, "harvest")