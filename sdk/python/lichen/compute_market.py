"""Typed Compute Market client, including bounded agent-payment controls."""

from __future__ import annotations

import base64
from typing import Any, Dict, Optional

from .connection import Connection
from .keypair import Keypair
from .publickey import PublicKey

PROGRAM_SYMBOL_CANDIDATES = ("COMPUTE", "compute", "ComputeMarket", "COMPUTEMARKET", "compute_market")
MAX_U64 = (1 << 64) - 1

COMPUTE_JOB_PENDING = 0
COMPUTE_JOB_CLAIMED = 1
COMPUTE_JOB_COMPLETED = 2
COMPUTE_JOB_DISPUTED = 3
COMPUTE_JOB_CANCELLED = 4
COMPUTE_JOB_RESOLVED = 5
COMPUTE_JOB_RELEASED = 6


def _public_key(value: PublicKey | str) -> PublicKey:
    return value if isinstance(value, PublicKey) else PublicKey(value)


def _u64(value: int, field_name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= MAX_U64:
        raise ValueError(f"{field_name} must be a u64-safe integer value")
    return value


def _u64_le(value: int, field_name: str) -> bytes:
    return _u64(value, field_name).to_bytes(8, "little")


def _hash32(value: bytes, field_name: str) -> bytes:
    if len(value) != 32:
        raise ValueError(f"{field_name} must be exactly 32 bytes")
    if not any(value):
        raise ValueError(f"{field_name} must not be the zero hash")
    return value


def _require_length(data: bytes, expected: int, function_name: str) -> bytes:
    if len(data) != expected:
        raise RuntimeError(f"Compute Market {function_name} payload must be exactly {expected} bytes")
    return data


def _layout(types: list[int], chunks: list[bytes]) -> bytes:
    return bytes([0xAB, *types]) + b"".join(chunks)


def _address_args(*addresses: PublicKey | str) -> bytes:
    return _layout([0x20] * len(addresses), [_public_key(address).to_bytes() for address in addresses])


def _id_args(job_id: int) -> bytes:
    return _layout([0x08], [_u64_le(job_id, "job_id")])


def _return_bytes(result: Dict[str, Any], function_name: str) -> bytes:
    code = int(result.get("returnCode") or 0)
    encoded = result.get("returnData")
    if code != 0 or result.get("success") is False or not isinstance(encoded, str):
        raise RuntimeError(result.get("error") or f"Compute Market {function_name} returned code {code}")
    return base64.b64decode(encoded.encode("ascii"))


def _read_u64(data: bytes, offset: int) -> int:
    if len(data) < offset + 8:
        raise RuntimeError("Compute Market return payload was shorter than expected")
    return int.from_bytes(data[offset : offset + 8], "little")


def _decode_provider(data: bytes) -> Dict[str, Any]:
    _require_length(data, 65, "get_provider_info")
    return {
        "address": data[0:32],
        "total_capacity": _read_u64(data, 32),
        "price_per_unit": _read_u64(data, 40),
        "jobs_completed": _read_u64(data, 48),
        "active": data[56] == 1,
        "registered_slot": _read_u64(data, 57),
    }


def _decode_job(data: bytes) -> Dict[str, Any]:
    _require_length(data, 161, "get_job")
    return {
        "requester": data[0:32],
        "compute_units": _read_u64(data, 32),
        "max_price": _read_u64(data, 40),
        "code_hash": data[48:80],
        "status": data[80],
        "provider": data[81:113],
        "result_hash": data[113:145],
        "created_slot": _read_u64(data, 145),
        "completed_slot": _read_u64(data, 153),
    }


class ComputeMarketClient:
    """First-class reads, lifecycle writes, administration, and agent controls."""

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
        raise RuntimeError('Unable to resolve the Compute Market program via getSymbolRegistry("COMPUTE")')

    async def _read(self, function: str, args: bytes = b"") -> Dict[str, Any]:
        return await self.connection.call_readonly_contract(await self.get_program_id(), function, args)

    async def _write(self, caller: Keypair, function: str, args: bytes, value: int = 0) -> str:
        return await self.connection.call_contract(caller, await self.get_program_id(), function, args, value)

    async def get_job(self, job_id: int) -> Optional[Dict[str, Any]]:
        result = await self._read("get_job", _id_args(job_id))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        return _decode_job(_return_bytes(result, "get_job"))

    async def get_job_count(self) -> int:
        data = _require_length(_return_bytes(await self._read("get_job_count"), "get_job_count"), 8, "get_job_count")
        return _read_u64(data, 0)

    async def get_provider(self, provider: PublicKey | str) -> Optional[Dict[str, Any]]:
        result = await self._read("get_provider_info", _address_args(provider))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        return _decode_provider(_return_bytes(result, "get_provider_info"))

    async def get_provider_capacity(self, provider: PublicKey | str) -> Optional[Dict[str, int]]:
        result = await self._read("get_provider_capacity", _address_args(provider))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        data = _require_length(_return_bytes(result, "get_provider_capacity"), 24, "get_provider_capacity")
        return {"total": _read_u64(data, 0), "reserved": _read_u64(data, 8), "available": _read_u64(data, 16)}

    async def get_job_timing(self, job_id: int) -> Optional[Dict[str, int]]:
        result = await self._read("get_job_timing", _id_args(job_id))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        data = _require_length(_return_bytes(result, "get_job_timing"), 48, "get_job_timing")
        names = ("created_slot", "claim_deadline", "claimed_slot", "completion_deadline", "completed_slot", "challenge_deadline")
        return {name: _read_u64(data, index * 8) for index, name in enumerate(names)}

    async def get_platform_stats(self) -> Dict[str, int]:
        data = _require_length(_return_bytes(await self._read("get_platform_stats"), "get_platform_stats"), 32, "get_platform_stats")
        return {
            "job_count": _read_u64(data, 0), "completed_count": _read_u64(data, 8),
            "payment_volume": _read_u64(data, 16), "dispute_count": _read_u64(data, 24),
        }

    async def _get_amount(self, function: str, args: bytes) -> int:
        data = _require_length(_return_bytes(await self._read(function, args), function), 8, function)
        return _read_u64(data, 0)

    async def get_escrow(self, job_id: int) -> int:
        return await self._get_amount("get_escrow", _id_args(job_id))

    async def get_platform_fees(self, token: PublicKey | str) -> int:
        return await self._get_amount("get_platform_fees", _address_args(token))

    async def get_unpaid_payout(self, token: PublicKey | str, recipient: PublicKey | str) -> int:
        return await self._get_amount("get_unpaid_payout", _address_args(token, recipient))

    async def get_agent_spend_window(self, agent: PublicKey | str, window: int) -> int:
        args = _layout([0x20, 0x08], [_public_key(agent).to_bytes(), _u64_le(window, "window")])
        return await self._get_amount("get_agent_spend_window", args)

    async def get_agent_job_action(self, job_id: int) -> Optional[bytes]:
        result = await self._read("get_agent_job_action", _id_args(job_id))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        data = _return_bytes(result, "get_agent_job_action")
        if len(data) != 32:
            raise RuntimeError("Compute Market action hash must be 32 bytes")
        return data

    async def get_agent_controls(self) -> Dict[str, Any]:
        data = _require_length(_return_bytes(await self._read("get_agent_compute_controls"), "get_agent_compute_controls"), 50, "get_agent_compute_controls")
        return {
            "enabled": data[0] == 1, "route_paused": data[1] == 1,
            "max_daily_cap": _read_u64(data, 2), "max_per_task_cap": _read_u64(data, 10),
            "policy_count": _read_u64(data, 18), "payment_count": _read_u64(data, 26),
            "payment_volume": _read_u64(data, 34), "blocked_payment_count": _read_u64(data, 42),
            "blocked_payment_count_supported": False,
        }

    async def get_agent_policy(self, agent: PublicKey | str) -> Optional[Dict[str, Any]]:
        result = await self._read("get_agent_spending_policy", _address_args(agent))
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        data = _require_length(_return_bytes(result, "get_agent_spending_policy"), 73, "get_agent_spending_policy")
        return {
            "policy_version": _read_u64(data, 0), "daily_cap": _read_u64(data, 8),
            "per_task_cap": _read_u64(data, 16), "policy_hash": data[24:56],
            "created_slot": _read_u64(data, 56), "updated_slot": _read_u64(data, 64), "active": data[72] == 1,
        }

    async def get_accounting_migration_status(self) -> Dict[str, Any]:
        data = _require_length(
            _return_bytes(await self._read("get_accounting_migration_status"), "get_accounting_migration_status"),
            48,
            "get_accounting_migration_status",
        )
        return {
            "expected_job_count": _read_u64(data, 0), "cursor": _read_u64(data, 8),
            "reconstructed_escrow": _read_u64(data, 16), "reconstructed_unpaid": _read_u64(data, 24),
            "accounting_version": _read_u64(data, 32), "locked": _read_u64(data, 40) == 1,
        }

    async def get_accounting_health(self) -> Dict[str, Any]:
        data = _require_length(
            _return_bytes(await self._read("get_accounting_health"), "get_accounting_health"),
            64,
            "get_accounting_health",
        )
        return {
            "accounting_version": _read_u64(data, 0), "migration_locked": _read_u64(data, 8) == 1,
            "escrow_liability": _read_u64(data, 16), "unpaid_liability": _read_u64(data, 24),
            "platform_fees": _read_u64(data, 32), "total_liability": _read_u64(data, 40),
            "custody_balance": _read_u64(data, 48), "solvent": _read_u64(data, 56) == 1,
        }

    async def register_provider(self, provider: Keypair, capacity: int, price_per_unit: int) -> str:
        args = _layout([0x20, 0x08, 0x08], [provider.pubkey().to_bytes(), _u64_le(capacity, "capacity"), _u64_le(price_per_unit, "price_per_unit")])
        return await self._write(provider, "register_provider", args)

    async def update_provider(self, provider: Keypair, capacity: int, price_per_unit: int) -> str:
        args = _layout([0x20, 0x08, 0x08], [provider.pubkey().to_bytes(), _u64_le(capacity, "capacity"), _u64_le(price_per_unit, "price_per_unit")])
        return await self._write(provider, "update_provider", args)

    async def deactivate_provider(self, provider: Keypair) -> str:
        return await self._write(provider, "deactivate_provider", _address_args(provider.pubkey()))

    async def reactivate_provider(self, provider: Keypair) -> str:
        return await self._write(provider, "reactivate_provider", _address_args(provider.pubkey()))

    async def submit_job(self, requester: Keypair, compute_units: int, max_price: int, code_hash: bytes, payment_value: Optional[int] = None) -> str:
        max_price = _u64(max_price, "max_price")
        args = _layout([0x20, 0x08, 0x08, 0x20], [requester.pubkey().to_bytes(), _u64_le(compute_units, "compute_units"), _u64_le(max_price, "max_price"), _hash32(code_hash, "code_hash")])
        return await self._write(requester, "submit_job", args, max_price if payment_value is None else _u64(payment_value, "payment_value"))

    async def claim_job(self, provider: Keypair, job_id: int) -> str:
        return await self._write(provider, "claim_job", _layout([0x20, 0x08], [provider.pubkey().to_bytes(), _u64_le(job_id, "job_id")]))

    async def complete_job(self, provider: Keypair, job_id: int, result_hash: bytes) -> str:
        args = _layout([0x20, 0x08, 0x20], [provider.pubkey().to_bytes(), _u64_le(job_id, "job_id"), _hash32(result_hash, "result_hash")])
        return await self._write(provider, "complete_job", args)

    async def dispute_job(self, requester: Keypair, job_id: int) -> str:
        return await self._write(requester, "dispute_job", _layout([0x20, 0x08], [requester.pubkey().to_bytes(), _u64_le(job_id, "job_id")]))

    async def cancel_job(self, requester: Keypair, job_id: int) -> str:
        return await self._write(requester, "cancel_job", _layout([0x20, 0x08], [requester.pubkey().to_bytes(), _u64_le(job_id, "job_id")]))

    async def release_payment(self, caller: Keypair, job_id: int) -> str:
        return await self._write(caller, "release_payment", _id_args(job_id))

    async def resolve_dispute(self, arbitrator: Keypair, job_id: int, provider_share_bps: int) -> str:
        args = _layout([0x20, 0x08, 0x08], [arbitrator.pubkey().to_bytes(), _u64_le(job_id, "job_id"), _u64_le(provider_share_bps, "provider_share_bps")])
        return await self._write(arbitrator, "resolve_dispute", args)

    async def claim_unpaid_payout(self, recipient: Keypair, token: PublicKey | str) -> str:
        return await self._write(recipient, "claim_unpaid_payout", _address_args(recipient.pubkey(), token))

    async def set_agent_policy(self, agent: Keypair, daily_cap: int, per_task_cap: int, policy_hash: bytes, policy_version: int) -> str:
        args = _layout([0x20, 0x08, 0x08, 0x20, 0x08], [agent.pubkey().to_bytes(), _u64_le(daily_cap, "daily_cap"), _u64_le(per_task_cap, "per_task_cap"), _hash32(policy_hash, "policy_hash"), _u64_le(policy_version, "policy_version")])
        return await self._write(agent, "set_agent_spending_policy", args)

    async def disable_agent_policy(self, agent: Keypair) -> str:
        return await self._write(agent, "disable_agent_spending_policy", _address_args(agent.pubkey()))

    async def submit_agent_job(self, agent: Keypair, compute_units: int, max_price: int, code_hash: bytes, action_hash: bytes, payment_value: Optional[int] = None) -> str:
        max_price = _u64(max_price, "max_price")
        args = _layout([0x20, 0x08, 0x08, 0x20, 0x20], [agent.pubkey().to_bytes(), _u64_le(compute_units, "compute_units"), _u64_le(max_price, "max_price"), _hash32(code_hash, "code_hash"), _hash32(action_hash, "action_hash")])
        return await self._write(agent, "submit_agent_job", args, max_price if payment_value is None else _u64(payment_value, "payment_value"))

    async def initialize(self, admin: Keypair) -> str:
        return await self._write(admin, "initialize", _address_args(admin.pubkey()))

    async def _admin_u64(self, admin: Keypair, function: str, value: int) -> str:
        return await self._write(admin, function, _layout([0x20, 0x08], [admin.pubkey().to_bytes(), _u64_le(value, "value")]))

    async def set_claim_timeout(self, admin: Keypair, slots: int) -> str: return await self._admin_u64(admin, "set_claim_timeout", slots)
    async def set_complete_timeout(self, admin: Keypair, slots: int) -> str: return await self._admin_u64(admin, "set_complete_timeout", slots)
    async def set_challenge_period(self, admin: Keypair, slots: int) -> str: return await self._admin_u64(admin, "set_challenge_period", slots)
    async def set_platform_fee(self, admin: Keypair, fee_bps: int) -> str: return await self._admin_u64(admin, "set_platform_fee", fee_bps)
    async def set_identity_gate(self, admin: Keypair, min_reputation: int) -> str: return await self._admin_u64(admin, "set_identity_gate", min_reputation)

    async def _admin_address(self, admin: Keypair, function: str, address: PublicKey | str) -> str:
        return await self._write(admin, function, _address_args(admin.pubkey(), address))

    async def add_arbitrator(self, admin: Keypair, arbitrator: PublicKey | str) -> str: return await self._admin_address(admin, "add_arbitrator", arbitrator)
    async def remove_arbitrator(self, admin: Keypair, arbitrator: PublicKey | str) -> str: return await self._admin_address(admin, "remove_arbitrator", arbitrator)
    async def set_token_address(self, admin: Keypair, token: PublicKey | str) -> str: return await self._admin_address(admin, "set_token_address", token)
    async def set_fee_treasury(self, admin: Keypair, treasury: PublicKey | str) -> str: return await self._admin_address(admin, "set_fee_treasury", treasury)
    async def set_lichenid_address(self, admin: Keypair, contract: PublicKey | str) -> str: return await self._admin_address(admin, "set_lichenid_address", contract)

    async def set_identity_admin(self, admin: Keypair) -> str:
        return await self._write(admin, "set_identity_admin", _address_args(admin.pubkey()))

    async def set_agent_controls(self, admin: Keypair, enabled: bool, route_paused: bool, max_daily_cap: int, max_per_task_cap: int) -> str:
        args = _layout([0x20, 0x08, 0x08, 0x08, 0x08], [admin.pubkey().to_bytes(), _u64_le(int(enabled), "enabled"), _u64_le(int(route_paused), "route_paused"), _u64_le(max_daily_cap, "max_daily_cap"), _u64_le(max_per_task_cap, "max_per_task_cap")])
        return await self._write(admin, "set_agent_compute_controls", args)

    async def pause(self, admin: Keypair) -> str: return await self._write(admin, "pause", _address_args(admin.pubkey()))
    async def unpause(self, admin: Keypair) -> str: return await self._write(admin, "unpause", _address_args(admin.pubkey()))

    async def withdraw_platform_fees(self, admin: Keypair, token: PublicKey | str, amount: int) -> str:
        args = _layout([0x20, 0x20, 0x08], [admin.pubkey().to_bytes(), _public_key(token).to_bytes(), _u64_le(amount, "amount")])
        return await self._write(admin, "withdraw_platform_fees", args)

    async def begin_accounting_v3_migration(self, admin: Keypair, expected_job_count: int) -> str:
        args = _layout([0x20, 0x08], [admin.pubkey().to_bytes(), _u64_le(expected_job_count, "expected_job_count")])
        return await self._write(admin, "begin_accounting_v3_migration", args)

    async def migrate_accounting_v3_job(self, caller: Keypair, job_id: int) -> str:
        return await self._write(caller, "migrate_accounting_v3_job", _id_args(job_id))

    async def complete_accounting_v3_migration(
        self,
        admin: Keypair,
        expected_escrow: int,
        expected_unpaid: int,
        expected_platform_fees: int,
        expected_total_liability: int,
    ) -> str:
        args = _layout(
            [0x20, 0x08, 0x08, 0x08, 0x08],
            [
                admin.pubkey().to_bytes(),
                _u64_le(expected_escrow, "expected_escrow"),
                _u64_le(expected_unpaid, "expected_unpaid"),
                _u64_le(expected_platform_fees, "expected_platform_fees"),
                _u64_le(expected_total_liability, "expected_total_liability"),
            ],
        )
        return await self._write(admin, "complete_accounting_v3_migration", args)
