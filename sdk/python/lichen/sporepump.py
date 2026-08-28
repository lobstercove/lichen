"""Exact, first-class SporePump launchpad client."""

from __future__ import annotations

import base64
from dataclasses import dataclass
from typing import Any, Dict, Optional

from .connection import Connection
from .keypair import Keypair
from .publickey import PublicKey

PROGRAM_SYMBOL_CANDIDATES = ("SPOREPUMP", "sporepump")
SPOREPUMP_CREATION_FEE = 10_000_000_000
MAX_U64 = (1 << 64) - 1


def _normalize_public_key(value: PublicKey | str) -> PublicKey:
    return value if isinstance(value, PublicKey) else PublicKey(value)


def _u64(value: int, field_name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= MAX_U64:
        raise ValueError(f"{field_name} must be a u64-safe integer")
    return value


def _u64_le(value: int, field_name: str) -> bytes:
    return _u64(value, field_name).to_bytes(8, "little")


def _u32_le(value: int, field_name: str) -> bytes:
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= 0xFFFF_FFFF:
        raise ValueError(f"{field_name} must be a u32 integer")
    return value.to_bytes(4, "little")


def _layout_args(chunks: list[bytes]) -> bytes:
    if any(len(chunk) > 255 for chunk in chunks):
        raise ValueError("SporePump ABI stride exceeded one byte")
    return bytes([0xAB, *(len(chunk) for chunk in chunks)]) + b"".join(chunks)


def _read_u64(data: bytes, offset: int = 0) -> int:
    if offset < 0 or len(data) < offset + 8:
        raise RuntimeError("SporePump payload was shorter than expected")
    return int.from_bytes(data[offset : offset + 8], "little")


def _return_data(result: Dict[str, Any], function_name: str, require_zero_code: bool = False) -> bytes:
    code = int(result.get("returnCode") or 0)
    if result.get("success") is False or (require_zero_code and code != 0):
        raise RuntimeError(result.get("error") or f"SporePump {function_name} returned code {code}")
    encoded = result.get("returnData")
    if not isinstance(encoded, str):
        raise RuntimeError(f"SporePump {function_name} did not return payload data")
    return base64.b64decode(encoded.encode("ascii"), validate=True)


def _u64_result(result: Dict[str, Any], function_name: str) -> int:
    data = _return_data(result, function_name)
    if len(data) != 8:
        raise RuntimeError(f"SporePump {function_name} returned a non-u64 payload")
    return _read_u64(data)


def _metadata_args(creator: PublicKey, name: str, symbol: str) -> bytes:
    normalized_name = name.strip()
    normalized_symbol = symbol.strip().upper()
    name_bytes = normalized_name.encode("utf-8")
    symbol_bytes = normalized_symbol.encode("ascii")
    if not name_bytes or len(name_bytes) > 64 or any(ord(char) < 32 or ord(char) == 127 for char in normalized_name):
        raise ValueError("name must be 1-64 UTF-8 bytes without control characters")
    if not (2 <= len(normalized_symbol) <= 12 and normalized_symbol[0].isalpha() and normalized_symbol.isalnum()):
        raise ValueError("symbol must be 2-12 ASCII alphanumeric characters and start with a letter")
    name_stride = max(32, len(name_bytes))
    symbol_stride = max(32, len(symbol_bytes))
    chunks = [
        creator.to_bytes(),
        name_bytes.ljust(name_stride, b"\0"),
        len(name_bytes).to_bytes(4, "little"),
        symbol_bytes.ljust(symbol_stride, b"\0"),
        len(symbol_bytes).to_bytes(4, "little"),
        _u64_le(SPOREPUMP_CREATION_FEE, "creation_fee"),
    ]
    return bytes([0xAB, 32, name_stride, 4, symbol_stride, 4, 8]) + b"".join(chunks)


@dataclass(frozen=True)
class CreateSporePumpTokenParams:
    name: str
    symbol: str


@dataclass(frozen=True)
class SporePumpGraduationConfig:
    router: PublicKey | str
    token_template_hash: PublicKey | str
    tick_size: int
    lot_size: int
    minimum_order: int
    amm_fee_tier: int


class SporePumpClient:
    """High-level launchpad reads, protected trades, governance, and migrations."""

    def __init__(self, connection: Connection, program_id: Optional[PublicKey] = None):
        self.connection = connection
        self._program_id = program_id

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
        raise RuntimeError('Unable to resolve SporePump via getSymbolRegistry("SPOREPUMP")')

    async def _readonly(self, function_name: str, args: bytes = b"") -> Dict[str, Any]:
        return await self.connection.call_readonly_contract(await self.get_program_id(), function_name, args)

    async def _write(
        self,
        signer: Keypair,
        function_name: str,
        args: bytes,
        value: int = 0,
    ) -> str:
        return await self.connection.call_contract(
            signer,
            await self.get_program_id(),
            function_name,
            args,
            _u64(value, "value"),
        )

    async def get_token_info(self, token_id: int) -> Optional[Dict[str, Any]]:
        result = await self._readonly("get_token_info", _layout_args([_u64_le(token_id, "token_id")]))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        data = _return_data(result, "get_token_info", True)
        if len(data) != 33 or data[32] not in (0, 1):
            raise RuntimeError("SporePump token-info payload was malformed")
        return {
            "supply_sold": _read_u64(data, 0),
            "licn_raised": _read_u64(data, 8),
            "current_price": _read_u64(data, 16),
            "market_cap": _read_u64(data, 24),
            "graduated": data[32] == 1,
        }

    async def get_token_metadata(self, token_id: int) -> Optional[Dict[str, str]]:
        result = await self._readonly("get_token_metadata", _layout_args([_u64_le(token_id, "token_id")]))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        data = _return_data(result, "get_token_metadata", True)
        if len(data) < 4:
            raise RuntimeError("SporePump metadata payload was malformed")
        name_len = int.from_bytes(data[0:2], "little")
        symbol_offset = 2 + name_len
        if len(data) < symbol_offset + 2:
            raise RuntimeError("SporePump metadata name length was invalid")
        symbol_len = int.from_bytes(data[symbol_offset : symbol_offset + 2], "little")
        if len(data) != symbol_offset + 2 + symbol_len:
            raise RuntimeError("SporePump metadata symbol length was invalid")
        return {
            "name": data[2:symbol_offset].decode("utf-8"),
            "symbol": data[symbol_offset + 2 :].decode("utf-8"),
        }

    async def get_buy_quote(self, token_id: int, licn_amount: int) -> int:
        return _u64_result(await self._readonly("get_buy_quote", _layout_args([
            _u64_le(token_id, "token_id"), _u64_le(licn_amount, "licn_amount"),
        ])), "get_buy_quote")

    async def get_sell_quote(self, token_id: int, token_amount: int) -> int:
        return _u64_result(await self._readonly("get_sell_quote", _layout_args([
            _u64_le(token_id, "token_id"), _u64_le(token_amount, "token_amount"),
        ])), "get_sell_quote")

    async def get_token_count(self) -> int:
        return _u64_result(await self._readonly("get_token_count"), "get_token_count")

    async def get_creator_royalty_balance(self, token_id: int, creator: PublicKey | str) -> int:
        return _u64_result(await self._readonly("get_creator_royalty_balance", _layout_args([
            _u64_le(token_id, "token_id"), _normalize_public_key(creator).to_bytes(),
        ])), "get_creator_royalty_balance")

    async def get_platform_stats(self) -> Dict[str, Any]:
        data = _return_data(await self._readonly("get_platform_stats"), "get_platform_stats", True)
        if len(data) != 88:
            raise RuntimeError("SporePump platform-stats payload was malformed")
        fields = [_read_u64(data, offset) for offset in range(0, 88, 8)]
        if fields[9] not in (0, 1) or fields[10] > 1_000:
            raise RuntimeError("SporePump platform-stats control values were malformed")
        return dict(zip((
            "token_count", "platform_fees", "curve_reserve", "creator_liability",
            "cumulative_graduation_revenue", "graduated_count", "accounting_version",
            "migration_expected", "migration_cursor", "migration_locked", "creator_royalty_bps",
        ), (*fields[:9], fields[9] == 1, fields[10]), strict=True))

    async def get_custody_status(self) -> Dict[str, int]:
        data = _return_data(await self._readonly("get_custody_status"), "get_custody_status", True)
        if len(data) != 24:
            raise RuntimeError("SporePump custody payload was malformed")
        return {"balance": _read_u64(data), "obligations": _read_u64(data, 8), "recoverable_surplus": _read_u64(data, 16)}

    async def get_accounting_migration_token(self, token_id: int) -> Optional[Dict[str, Any]]:
        result = await self._readonly(
            "get_accounting_migration_token",
            _layout_args([_u64_le(token_id, "token_id")]),
        )
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        data = _return_data(result, "get_accounting_migration_token", True)
        if len(data) != 73 or data[64] not in (0, 1, 3) or not any(data[0:32]):
            raise RuntimeError("SporePump accounting-migration token payload was malformed")
        supply_sold = _read_u64(data, 32)
        max_supply = _read_u64(data, 48)
        if supply_sold > max_supply:
            raise RuntimeError("SporePump accounting-migration token supply exceeds its cap")
        return {
            "creator": str(PublicKey(data[0:32])),
            "supply_sold": supply_sold,
            "licn_raised": _read_u64(data, 40),
            "max_supply": max_supply,
            "created_slot": _read_u64(data, 56),
            "lifecycle_state": data[64],
            "creator_royalty": _read_u64(data, 65),
        }

    async def get_graduation_status(self, token_id: int) -> Optional[Dict[str, Any]]:
        result = await self._readonly("get_graduation_status", _layout_args([_u64_le(token_id, "token_id")]))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        data = _return_data(result, "get_graduation_status", True)
        if len(data) != 113 or data[0] > 3:
            raise RuntimeError("SporePump graduation-status payload was malformed")
        candidate = data[17:49]
        return {
            "state": data[0], "eligibility_slot": _read_u64(data, 1),
            "migration_boundary_slot": _read_u64(data, 9),
            "candidate": str(PublicKey(candidate)) if any(candidate) else None,
            "pair_id": _read_u64(data, 49), "pool_id": _read_u64(data, 57),
            "forward_route_id": _read_u64(data, 65), "reverse_route_id": _read_u64(data, 73),
            "position_id": _read_u64(data, 81), "licn_liquidity": _read_u64(data, 89),
            "token_liquidity": _read_u64(data, 97), "protocol_token_inventory": _read_u64(data, 105),
        }

    async def get_graduation_info(self) -> Dict[str, Any]:
        data = _return_data(await self._readonly("get_graduation_info"), "get_graduation_info", True)
        if len(data) != 46 or any(flag not in (0, 1) for flag in data[8:14]):
            raise RuntimeError("SporePump graduation-info payload was malformed")
        return {
            "cumulative_revenue": _read_u64(data), "dex_core_configured": data[8] == 1,
            "dex_amm_configured": data[9] == 1, "dex_router_configured": data[10] == 1,
            "token_template_configured": data[11] == 1, "governance_configured": data[12] == 1,
            "accounting_ready": data[13] == 1, "tick_size": _read_u64(data, 14),
            "lot_size": _read_u64(data, 22), "minimum_order": _read_u64(data, 30),
            "amm_fee_tier": _read_u64(data, 38),
        }

    async def create_token(self, creator: Keypair, params: Optional[CreateSporePumpTokenParams] = None) -> str:
        args = (_metadata_args(creator.pubkey(), params.name, params.symbol) if params else
                _layout_args([creator.pubkey().to_bytes(), _u64_le(SPOREPUMP_CREATION_FEE, "creation_fee")]))
        return await self._write(creator, "create_token_with_metadata" if params else "create_token", args, SPOREPUMP_CREATION_FEE)

    async def buy(self, buyer: Keypair, token_id: int, licn_amount: int, minimum_tokens_out: int) -> str:
        args = _layout_args([buyer.pubkey().to_bytes(), _u64_le(token_id, "token_id"),
                             _u64_le(licn_amount, "licn_amount"), _u64_le(minimum_tokens_out, "minimum_tokens_out")])
        return await self._write(buyer, "buy_with_min_output", args, licn_amount)

    async def sell(self, seller: Keypair, token_id: int, token_amount: int, minimum_licn_out: int) -> str:
        args = _layout_args([seller.pubkey().to_bytes(), _u64_le(token_id, "token_id"),
                             _u64_le(token_amount, "token_amount"), _u64_le(minimum_licn_out, "minimum_licn_out")])
        return await self._write(seller, "sell_with_min_output", args)

    async def claim_creator_royalty(self, creator: Keypair, token_id: int, amount: int) -> str:
        return await self._write(creator, "claim_creator_royalty", _layout_args([
            creator.pubkey().to_bytes(), _u64_le(token_id, "token_id"), _u64_le(amount, "amount"),
        ]))

    async def _admin_u64(self, signer: Keypair, function_name: str, value: int) -> str:
        return await self._write(signer, function_name, _layout_args([
            signer.pubkey().to_bytes(), _u64_le(value, "value"),
        ]))

    async def pause(self, admin: Keypair) -> str:
        return await self._write(admin, "pause", _layout_args([admin.pubkey().to_bytes()]))

    async def unpause(self, admin: Keypair) -> str:
        return await self._write(admin, "unpause", _layout_args([admin.pubkey().to_bytes()]))

    async def freeze_token(self, admin: Keypair, token_id: int) -> str:
        return await self._admin_u64(admin, "freeze_token", token_id)

    async def unfreeze_token(self, admin: Keypair, token_id: int) -> str:
        return await self._admin_u64(admin, "unfreeze_token", token_id)

    async def set_buy_cooldown(self, admin: Keypair, slots: int) -> str:
        return await self._admin_u64(admin, "set_buy_cooldown", slots)

    async def set_sell_cooldown(self, admin: Keypair, slots: int) -> str:
        return await self._admin_u64(admin, "set_sell_cooldown", slots)

    async def set_max_buy(self, admin: Keypair, amount: int) -> str:
        return await self._admin_u64(admin, "set_max_buy", amount)

    async def set_creator_royalty(self, admin: Keypair, basis_points: int) -> str:
        return await self._admin_u64(admin, "set_creator_royalty", basis_points)

    async def withdraw_fees(self, admin: Keypair, amount: int) -> str:
        return await self._admin_u64(admin, "withdraw_fees", amount)

    async def recover_custody_surplus(self, admin: Keypair, amount: int) -> str:
        return await self._admin_u64(admin, "recover_custody_surplus", amount)

    async def begin_accounting_v3_migration(self, admin: Keypair, expected_tokens: int) -> str:
        return await self._admin_u64(admin, "begin_accounting_v3_migration", expected_tokens)

    async def migrate_accounting_v3_token(self, keeper: Keypair, token_id: int) -> str:
        return await self._write(keeper, "migrate_accounting_v3_token", _layout_args([_u64_le(token_id, "token_id")]))

    async def complete_accounting_v3_migration(self, admin: Keypair) -> str:
        return await self._write(admin, "complete_accounting_v3_migration", _layout_args([admin.pubkey().to_bytes()]))

    async def propose_admin(self, admin: Keypair, next_admin: PublicKey | str) -> str:
        return await self._write(admin, "propose_admin", _layout_args([
            admin.pubkey().to_bytes(), _normalize_public_key(next_admin).to_bytes(),
        ]))

    async def accept_admin(self, next_admin: Keypair) -> str:
        return await self._write(next_admin, "accept_admin", _layout_args([next_admin.pubkey().to_bytes()]))

    async def set_dex_addresses(self, admin: Keypair, core: PublicKey | str, amm: PublicKey | str) -> str:
        return await self._write(admin, "set_dex_addresses", _layout_args([
            admin.pubkey().to_bytes(), _normalize_public_key(core).to_bytes(), _normalize_public_key(amm).to_bytes(),
        ]))

    async def set_graduation_governance(self, admin: Keypair, governance: PublicKey | str) -> str:
        return await self._write(admin, "set_graduation_governance", _layout_args([
            admin.pubkey().to_bytes(), _normalize_public_key(governance).to_bytes(),
        ]))

    async def set_graduation_config(self, governance: Keypair, config: SporePumpGraduationConfig) -> str:
        return await self._write(governance, "set_graduation_config", _layout_args([
            governance.pubkey().to_bytes(), _normalize_public_key(config.router).to_bytes(),
            _normalize_public_key(config.token_template_hash).to_bytes(), _u64_le(config.tick_size, "tick_size"),
            _u64_le(config.lot_size, "lot_size"), _u64_le(config.minimum_order, "minimum_order"),
            _u32_le(config.amm_fee_tier, "amm_fee_tier"),
        ]))

    async def begin_graduation(self, keeper: Keypair, token_id: int, candidate: PublicKey | str) -> str:
        return await self._write(keeper, "begin_migration", _layout_args([
            keeper.pubkey().to_bytes(), _u64_le(token_id, "token_id"), _normalize_public_key(candidate).to_bytes(),
        ]))

    async def abort_graduation(self, keeper: Keypair, token_id: int) -> str:
        return await self._admin_u64(keeper, "abort_migration", token_id)

    async def finalize_graduation(self, keeper: Keypair, token_id: int) -> str:
        return await self._admin_u64(keeper, "finalize_migration", token_id)
