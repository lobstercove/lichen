"""First-class LichenSwap helper built on top of the Python SDK primitives."""

from __future__ import annotations

import base64
from dataclasses import dataclass
from typing import Any, Dict, Optional

from .connection import Connection
from .keypair import Keypair
from .publickey import PublicKey

PROGRAM_SYMBOL_CANDIDATES = ("LICHENSWAP", "lichenswap")
MAX_U64 = (1 << 64) - 1


def _normalize_public_key(value: PublicKey | str) -> PublicKey:
    return value if isinstance(value, PublicKey) else PublicKey(value)


def _normalize_u64(value: int, field_name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0 or value > MAX_U64:
        raise ValueError(f"{field_name} must be a u64-safe integer value")
    return value


def _add_u64(left: int, right: int, field_name: str) -> int:
    total = _normalize_u64(left, field_name) + _normalize_u64(right, field_name)
    if total > MAX_U64:
        raise ValueError(f"{field_name} must be a u64-safe integer value")
    return total


def _u32_le(value: int) -> bytes:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0 or value > 0xFFFF_FFFF:
        raise ValueError("u32 values must fit within 0 to 4,294,967,295")
    return value.to_bytes(4, "little")


def _u64_le(value: int, field_name: str) -> bytes:
    return _normalize_u64(value, field_name).to_bytes(8, "little")


def _build_layout_args(layout: list[int], chunks: list[bytes]) -> bytes:
    return bytes([0xAB, *layout]) + b"".join(chunks)


def _encode_create_pool_args(token_a: PublicKey | str, token_b: PublicKey | str) -> bytes:
    return _build_layout_args(
        [0x20, 0x20],
        [_normalize_public_key(token_a).to_bytes(), _normalize_public_key(token_b).to_bytes()],
    )


def _encode_add_liquidity_args(provider: PublicKey, amount_a: int, amount_b: int, min_liquidity: int) -> bytes:
    return _build_layout_args(
        [0x20, 0x08, 0x08, 0x08],
        [
            provider.to_bytes(),
            _u64_le(amount_a, "amount_a"),
            _u64_le(amount_b, "amount_b"),
            _u64_le(min_liquidity, "min_liquidity"),
        ],
    )


def _encode_swap_args(amount_in: int, min_amount_out: int, a_to_b: bool) -> bytes:
    return _build_layout_args(
        [0x08, 0x08, 0x04],
        [
            _u64_le(amount_in, "amount_in"),
            _u64_le(min_amount_out, "min_amount_out"),
            _u32_le(1 if a_to_b else 0),
        ],
    )


def _encode_directional_swap_args(amount_in: int, min_amount_out: int) -> bytes:
    return _build_layout_args(
        [0x08, 0x08],
        [_u64_le(amount_in, "amount_in"), _u64_le(min_amount_out, "min_amount_out")],
    )


def _encode_directional_swap_with_deadline_args(amount_in: int, min_amount_out: int, deadline: int) -> bytes:
    return _build_layout_args(
        [0x08, 0x08, 0x08],
        [
            _u64_le(amount_in, "amount_in"),
            _u64_le(min_amount_out, "min_amount_out"),
            _u64_le(deadline, "deadline"),
        ],
    )


def _encode_quote_args(amount_in: int, a_to_b: bool) -> bytes:
    return _build_layout_args(
        [0x08, 0x04],
        [_u64_le(amount_in, "amount_in"), _u32_le(1 if a_to_b else 0)],
    )


def _encode_provider_args(provider: PublicKey | str) -> bytes:
    return _build_layout_args([0x20], [_normalize_public_key(provider).to_bytes()])


def _encode_amount_args(amount: int, field_name: str) -> bytes:
    return _build_layout_args([0x08], [_u64_le(amount, field_name)])


def _decode_u64_le(data: bytes, offset: int = 0) -> int:
    return int.from_bytes(data[offset : offset + 8], "little")


def _decode_return_data(value: str) -> bytes:
    return base64.b64decode(value.encode("ascii"))


def _ensure_return_code(result: Dict[str, Any], function_name: str, allowed_codes: tuple[int, ...] = (0,)) -> None:
    code = int(result.get("returnCode") or 0)
    if code not in allowed_codes:
        raise RuntimeError(result.get("error") or f"LichenSwap {function_name} returned code {code}")
    if result.get("success") is False and result.get("error"):
        raise RuntimeError(str(result["error"]))


def _decode_u64_result(result: Dict[str, Any], function_name: str) -> int:
    _ensure_return_code(result, function_name)
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError(f"LichenSwap {function_name} did not return payload data")
    data = _decode_return_data(return_data)
    if len(data) < 8:
        raise RuntimeError(f"LichenSwap {function_name} payload was shorter than expected")
    return _decode_u64_le(data)


def _decode_pool_info(result: Dict[str, Any]) -> Dict[str, int]:
    _ensure_return_code(result, "get_pool_info", allowed_codes=(0, 1))
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("LichenSwap get_pool_info did not return pool data")
    data = _decode_return_data(return_data)
    if len(data) < 24:
        raise RuntimeError("LichenSwap get_pool_info payload was shorter than expected")
    return {
        "reserve_a": _decode_u64_le(data, 0),
        "reserve_b": _decode_u64_le(data, 8),
        "total_liquidity": _decode_u64_le(data, 16),
    }


def _decode_volume_totals(result: Dict[str, Any], function_name: str) -> Dict[str, int]:
    _ensure_return_code(result, function_name)
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError(f"LichenSwap {function_name} did not return volume data")
    data = _decode_return_data(return_data)
    if len(data) < 16:
        raise RuntimeError(f"LichenSwap {function_name} payload was shorter than expected")
    return {
        "volume_a": _decode_u64_le(data, 0),
        "volume_b": _decode_u64_le(data, 8),
    }


def _decode_protocol_fees(result: Dict[str, Any]) -> Dict[str, int]:
    _ensure_return_code(result, "get_protocol_fees")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("LichenSwap get_protocol_fees did not return fee data")
    data = _decode_return_data(return_data)
    if len(data) < 16:
        raise RuntimeError("LichenSwap get_protocol_fees payload was shorter than expected")
    return {"fees_a": _decode_u64_le(data, 0), "fees_b": _decode_u64_le(data, 8)}


def _decode_twap_cumulatives(result: Dict[str, Any]) -> Dict[str, int]:
    _ensure_return_code(result, "get_twap_cumulatives")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("LichenSwap get_twap_cumulatives did not return TWAP data")
    data = _decode_return_data(return_data)
    if len(data) < 24:
        raise RuntimeError("LichenSwap get_twap_cumulatives payload was shorter than expected")
    return {
        "cumulative_price_a": _decode_u64_le(data, 0),
        "cumulative_price_b": _decode_u64_le(data, 8),
        "last_updated_at": _decode_u64_le(data, 16),
    }


def _decode_swap_stats(result: Dict[str, Any]) -> Dict[str, int]:
    _ensure_return_code(result, "get_swap_stats")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("LichenSwap get_swap_stats did not return stats data")
    data = _decode_return_data(return_data)
    if len(data) < 40:
        raise RuntimeError("LichenSwap get_swap_stats payload was shorter than expected")
    return {
        "swap_count": _decode_u64_le(data, 0),
        "volume_a": _decode_u64_le(data, 8),
        "volume_b": _decode_u64_le(data, 16),
        "pool_count": _decode_u64_le(data, 24),
        "total_liquidity": _decode_u64_le(data, 32),
    }


@dataclass(frozen=True)
class CreatePoolParams:
    token_a: PublicKey | str
    token_b: PublicKey | str


@dataclass(frozen=True)
class AddLiquidityParams:
    amount_a: int
    amount_b: int
    min_liquidity: int = 0
    value_spores: Optional[int] = None


@dataclass(frozen=True)
class SwapParams:
    amount_in: int
    min_amount_out: int = 0
    value_spores: Optional[int] = None


@dataclass(frozen=True)
class SwapWithDeadlineParams:
    amount_in: int
    deadline: int
    min_amount_out: int = 0
    value_spores: Optional[int] = None


class LichenSwapClient:
    """High-level helper for common LichenSwap reads and writes."""

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

        raise RuntimeError('Unable to resolve the LichenSwap program via getSymbolRegistry("LICHENSWAP")')

    async def get_pool_info(self) -> Dict[str, int]:
        return _decode_pool_info(await self._call_readonly("get_pool_info"))

    async def get_quote(self, amount_in: int, a_to_b: bool = True) -> int:
        return _decode_u64_result(
            await self._call_readonly("get_quote", _encode_quote_args(amount_in, a_to_b)),
            "get_quote",
        )

    async def get_liquidity_balance(self, provider: PublicKey | str) -> int:
        return _decode_u64_result(
            await self._call_readonly("get_liquidity_balance", _encode_provider_args(provider)),
            "get_liquidity_balance",
        )

    async def get_total_liquidity(self) -> int:
        return _decode_u64_result(await self._call_readonly("get_total_liquidity"), "get_total_liquidity")

    async def get_flash_loan_fee(self, amount: int) -> int:
        return _decode_u64_result(
            await self._call_readonly("get_flash_loan_fee", _encode_amount_args(amount, "amount")),
            "get_flash_loan_fee",
        )

    async def get_twap_cumulatives(self) -> Dict[str, int]:
        return _decode_twap_cumulatives(await self._call_readonly("get_twap_cumulatives"))

    async def get_twap_snapshot_count(self) -> int:
        return _decode_u64_result(await self._call_readonly("get_twap_snapshot_count"), "get_twap_snapshot_count")

    async def get_protocol_fees(self) -> Dict[str, int]:
        return _decode_protocol_fees(await self._call_readonly("get_protocol_fees"))

    async def get_pool_count(self) -> int:
        return _decode_u64_result(await self._call_readonly("get_pool_count"), "get_pool_count")

    async def get_swap_count(self) -> int:
        return _decode_u64_result(await self._call_readonly("get_swap_count"), "get_swap_count")

    async def get_total_volume(self) -> Dict[str, int]:
        return _decode_volume_totals(await self._call_readonly("get_total_volume"), "get_total_volume")

    async def get_swap_stats(self) -> Dict[str, int]:
        return _decode_swap_stats(await self._call_readonly("get_swap_stats"))

    async def get_stats(self) -> Dict[str, Any]:
        stats = await self.connection.get_lichenswap_stats()
        return {
            "swap_count": stats.get("swap_count", 0),
            "volume_a": stats.get("volume_a", 0),
            "volume_b": stats.get("volume_b", 0),
            "paused": bool(stats.get("paused")),
        }

    async def create_pool(self, owner: Keypair, params: CreatePoolParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_create_pool_args(params.token_a, params.token_b)
        return await self.connection.call_contract(owner, program_id, "create_pool", args)

    async def add_liquidity(self, provider: Keypair, params: AddLiquidityParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_add_liquidity_args(provider.pubkey(), params.amount_a, params.amount_b, params.min_liquidity)
        value = params.value_spores if params.value_spores is not None else _add_u64(params.amount_a, params.amount_b, "value_spores")
        return await self.connection.call_contract(provider, program_id, "add_liquidity", args, value)

    async def swap(self, provider: Keypair, params: SwapParams, a_to_b: bool = True) -> str:
        program_id = await self.get_program_id()
        args = _encode_swap_args(params.amount_in, params.min_amount_out, a_to_b)
        value = params.value_spores if params.value_spores is not None else _normalize_u64(params.amount_in, "value_spores")
        return await self.connection.call_contract(provider, program_id, "swap", args, value)

    async def swap_a_for_b(self, provider: Keypair, params: SwapParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_directional_swap_args(params.amount_in, params.min_amount_out)
        value = params.value_spores if params.value_spores is not None else _normalize_u64(params.amount_in, "value_spores")
        return await self.connection.call_contract(provider, program_id, "swap_a_for_b", args, value)

    async def swap_b_for_a(self, provider: Keypair, params: SwapParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_directional_swap_args(params.amount_in, params.min_amount_out)
        value = params.value_spores if params.value_spores is not None else _normalize_u64(params.amount_in, "value_spores")
        return await self.connection.call_contract(provider, program_id, "swap_b_for_a", args, value)

    async def swap_a_for_b_with_deadline(self, provider: Keypair, params: SwapWithDeadlineParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_directional_swap_with_deadline_args(params.amount_in, params.min_amount_out, params.deadline)
        value = params.value_spores if params.value_spores is not None else _normalize_u64(params.amount_in, "value_spores")
        return await self.connection.call_contract(provider, program_id, "swap_a_for_b_with_deadline", args, value)

    async def swap_b_for_a_with_deadline(self, provider: Keypair, params: SwapWithDeadlineParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_directional_swap_with_deadline_args(params.amount_in, params.min_amount_out, params.deadline)
        value = params.value_spores if params.value_spores is not None else _normalize_u64(params.amount_in, "value_spores")
        return await self.connection.call_contract(provider, program_id, "swap_b_for_a_with_deadline", args, value)