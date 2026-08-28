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


def _encode_admin_u64_args(admin: PublicKey, value: int, field_name: str) -> bytes:
    return _build_layout_args([0x20, 0x08], [admin.to_bytes(), _u64_le(value, field_name)])


def _encode_admin_u8_args(admin: PublicKey, value: int, field_name: str) -> bytes:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0 or value > 0xFF:
        raise ValueError(f"{field_name} must be a u8 integer value")
    return _build_layout_args([0x20, 0x01], [admin.to_bytes(), bytes([value])])


def _encode_admin_strategy_args(admin: PublicKey, strategy_type: int, allocation: int) -> bytes:
    if isinstance(strategy_type, bool) or not isinstance(strategy_type, int) or not 0 <= strategy_type <= 0xFF:
        raise ValueError("strategy_type must be a u8 integer value")
    return _build_layout_args(
        [0x20, 0x01, 0x08],
        [admin.to_bytes(), bytes([strategy_type]), _u64_le(allocation, "allocation")],
    )


def _encode_admin_two_u64_args(admin: PublicKey, first: int, second: int) -> bytes:
    return _build_layout_args(
        [0x20, 0x08, 0x08],
        [admin.to_bytes(), _u64_le(first, "first"), _u64_le(second, "second")],
    )


def _encode_admin_address_args(admin: PublicKey, address: PublicKey) -> bytes:
    return _build_layout_args([0x20, 0x20], [admin.to_bytes(), address.to_bytes()])


def _encode_protocol_address_args(admin: PublicKey, thalllend: PublicKey, lichenswap: PublicKey) -> bytes:
    return _build_layout_args(
        [0x20, 0x20, 0x20],
        [admin.to_bytes(), thalllend.to_bytes(), lichenswap.to_bytes()],
    )


def _encode_legacy_strategy_retirement_args(
    admin: PublicKey,
    index: int,
    expected_type: int,
    expected_allocation: int,
    expected_deployed: int,
) -> bytes:
    if isinstance(expected_type, bool) or not isinstance(expected_type, int) or not 0 <= expected_type <= 0xFF:
        raise ValueError("expected_type must be a u8 integer value")
    return _build_layout_args(
        [0x20, 0x08, 0x01, 0x08, 0x08],
        [
            admin.to_bytes(),
            _u64_le(index, "index"),
            bytes([expected_type]),
            _u64_le(expected_allocation, "expected_allocation"),
            _u64_le(expected_deployed, "expected_deployed"),
        ],
    )


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


def _decode_vault_status(result: Dict[str, Any]) -> Dict[str, Any]:
    _ensure_return_code(result, "get_vault_status")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("SporeVault get_vault_status did not return status data")
    data = _decode_return_data(return_data)
    if len(data) < 23 * 8:
        raise RuntimeError("SporeVault get_vault_status payload was shorter than expected")
    values = [_decode_u64_le(data, index * 8) for index in range(23)]
    return {
        "accounting_version": values[0],
        "paused": bool(values[1]),
        "licn_config_present": bool(values[2]),
        "licn_config_valid": bool(values[3]),
        "native_licn": bool(values[4]),
        "thalllend_config_present": bool(values[5]),
        "thalllend_config_valid": bool(values[6]),
        "strategy_registry_valid": bool(values[7]),
        "idle_assets": values[8],
        "lending_assets": values[9],
        "total_assets": values[10],
        "total_shares": values[11],
        "protocol_fees": values[12],
        "real_liquid_custody": values[13],
        "custody_query_ok": bool(values[14]),
        "liquid_custody_covers_accounting": bool(values[15]),
        "deposit_fee_bps": values[16],
        "withdrawal_fee_bps": values[17],
        "deposit_cap": values[18],
        "risk_tier": values[19],
        "performance_fee_percent": values[20],
        "management_fee_bps": values[21],
        "target_slots_per_year": values[22],
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

    async def get_vault_status(self) -> Dict[str, Any]:
        return _decode_vault_status(await self._call_readonly("get_vault_status"))

    async def get_stats(self) -> Dict[str, Any]:
        stats = await self.connection.get_sporevault_stats()
        return {
            "total_assets": stats.get("total_assets", 0),
            "total_shares": stats.get("total_shares", 0),
            "strategy_count": stats.get("strategy_count", 0),
            "total_earned": stats.get("total_earned", 0),
            "fees_earned": stats.get("fees_earned", 0),
            "protocol_fees": stats.get("protocol_fees", 0),
            "idle_assets": stats.get("idle_assets", 0),
            "lending_assets": stats.get("lending_assets", 0),
            "accounting_version": stats.get("accounting_version", 0),
            "deposit_fee_bps": stats.get("deposit_fee_bps", 0),
            "withdrawal_fee_bps": stats.get("withdrawal_fee_bps", 0),
            "deposit_cap": stats.get("deposit_cap", 0),
            "risk_tier": stats.get("risk_tier", 0),
            "active_lending_strategies": stats.get("active_lending_strategies", 0),
            "lending_strategy_rows": stats.get("lending_strategy_rows", 0),
            "strategy_registry_bounded": bool(stats.get("strategy_registry_bounded")),
            "strategy_registry_valid": bool(stats.get("strategy_registry_valid")),
            "total_strategy_allocation": stats.get("total_strategy_allocation", 0),
            "native_licn": bool(stats.get("native_licn")),
            "thalllend_config_valid": bool(stats.get("thalllend_config_valid")),
            "components_match_total": bool(stats.get("components_match_total")),
            "share_state_consistent": bool(stats.get("share_state_consistent")),
            "liquid_custody_covers_accounting": bool(stats.get("liquid_custody_covers_accounting")),
            "paused": bool(stats.get("paused")),
            "operational": bool(stats.get("operational")),
        }

    async def deposit(self, depositor: Keypair, amount: int) -> str:
        normalized_amount = _normalize_u64(amount, "amount")
        program_id = await self.get_program_id()
        args = _encode_user_amount_args(depositor.pubkey(), normalized_amount)
        return await self.connection.call_contract(depositor, program_id, "deposit", args, normalized_amount)

    async def deposit_mt20(self, depositor: Keypair, amount: int) -> str:
        normalized_amount = _normalize_u64(amount, "amount")
        program_id = await self.get_program_id()
        args = _encode_user_amount_args(depositor.pubkey(), normalized_amount)
        return await self.connection.call_contract(depositor, program_id, "deposit", args)

    async def withdraw(self, depositor: Keypair, shares_to_burn: int) -> str:
        normalized_shares = _normalize_u64(shares_to_burn, "shares_to_burn")
        program_id = await self.get_program_id()
        args = _encode_user_amount_args(depositor.pubkey(), normalized_shares)
        return await self.connection.call_contract(depositor, program_id, "withdraw", args)

    async def harvest(self, caller: Keypair) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(caller, program_id, "harvest")

    async def rebalance(self, caller: Keypair) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(caller, program_id, "rebalance")

    async def _call_admin(self, admin: Keypair, function_name: str, args: bytes) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(admin, program_id, function_name, args)

    async def pause(self, admin: Keypair) -> str:
        return await self._call_admin(admin, "cv_pause", _build_layout_args([0x20], [admin.pubkey().to_bytes()]))

    async def unpause(self, admin: Keypair) -> str:
        return await self._call_admin(admin, "cv_unpause", _build_layout_args([0x20], [admin.pubkey().to_bytes()]))

    async def set_deposit_fee(self, admin: Keypair, fee_bps: int) -> str:
        return await self._call_admin(
            admin, "set_deposit_fee", _encode_admin_u64_args(admin.pubkey(), fee_bps, "fee_bps")
        )

    async def set_withdrawal_fee(self, admin: Keypair, fee_bps: int) -> str:
        return await self._call_admin(
            admin, "set_withdrawal_fee", _encode_admin_u64_args(admin.pubkey(), fee_bps, "fee_bps")
        )

    async def set_deposit_cap(self, admin: Keypair, cap: int) -> str:
        return await self._call_admin(
            admin, "set_deposit_cap", _encode_admin_u64_args(admin.pubkey(), cap, "cap")
        )

    async def set_risk_tier(self, admin: Keypair, tier: int) -> str:
        return await self._call_admin(admin, "set_risk_tier", _encode_admin_u8_args(admin.pubkey(), tier, "tier"))

    async def add_strategy(self, admin: Keypair, strategy_type: int, allocation_percent: int) -> str:
        return await self._call_admin(
            admin,
            "add_strategy",
            _encode_admin_strategy_args(admin.pubkey(), strategy_type, allocation_percent),
        )

    async def remove_strategy(self, admin: Keypair, index: int) -> str:
        return await self._call_admin(
            admin, "remove_strategy", _encode_admin_u64_args(admin.pubkey(), index, "index")
        )

    async def update_strategy_allocation(self, admin: Keypair, index: int, allocation_percent: int) -> str:
        return await self._call_admin(
            admin,
            "update_strategy_allocation",
            _encode_admin_two_u64_args(admin.pubkey(), index, allocation_percent),
        )

    async def withdraw_protocol_fees(self, admin: Keypair) -> str:
        return await self._call_admin(
            admin,
            "withdraw_protocol_fees",
            _build_layout_args([0x20], [admin.pubkey().to_bytes()]),
        )

    async def set_protocol_addresses(
        self,
        admin: Keypair,
        thalllend: PublicKey | str,
        lichenswap: PublicKey | str = PublicKey(bytes(32)),
    ) -> str:
        return await self._call_admin(
            admin,
            "set_protocol_addresses",
            _encode_protocol_address_args(
                admin.pubkey(), _normalize_public_key(thalllend), _normalize_public_key(lichenswap)
            ),
        )

    async def set_licn_token(self, admin: Keypair, token: PublicKey | str) -> str:
        return await self._call_admin(
            admin, "set_licn_token", _encode_admin_address_args(admin.pubkey(), _normalize_public_key(token))
        )

    async def migrate_accounting_v2(
        self, admin: Keypair, expected_idle_assets: int, expected_lending_assets: int
    ) -> str:
        return await self._call_admin(
            admin,
            "migrate_accounting_v2",
            _encode_admin_two_u64_args(admin.pubkey(), expected_idle_assets, expected_lending_assets),
        )

    async def retire_legacy_strategy(
        self,
        admin: Keypair,
        index: int,
        expected_type: int,
        expected_allocation: int,
        expected_deployed: int,
    ) -> str:
        return await self._call_admin(
            admin,
            "retire_legacy_strategy",
            _encode_legacy_strategy_retirement_args(
                admin.pubkey(), index, expected_type, expected_allocation, expected_deployed
            ),
        )
