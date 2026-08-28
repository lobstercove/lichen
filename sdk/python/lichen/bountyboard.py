"""First-class BountyBoard helper built on top of the Python SDK primitives."""

from __future__ import annotations

import base64
from typing import Any, Dict, Optional

from .connection import Connection
from .keypair import Keypair
from .publickey import PublicKey

PROGRAM_SYMBOL_CANDIDATES = (
    "BOUNTY",
    "bounty",
    "BountyBoard",
    "BOUNTYBOARD",
    "bountyboard",
)
MAX_U64 = (1 << 64) - 1
BOUNTY_DATA_SIZE = 91
PLATFORM_STATS_SIZE = 32
SUBMISSION_DATA_SIZE = 72
BOUNTY_TERMS_SIZE = 64
ACCOUNTING_MIGRATION_STATUS_SIZE = 40
ACCOUNTING_HEALTH_SIZE = 56
ADMIN_TRANSITION_SIZE = 64

# Bounty status constants
BOUNTY_STATUS_OPEN = 0
BOUNTY_STATUS_COMPLETED = 1
BOUNTY_STATUS_CANCELLED = 2


def _normalize_public_key(value: PublicKey | str) -> PublicKey:
    return value if isinstance(value, PublicKey) else PublicKey(value)


def _normalize_u64(value: int, field_name: str) -> int:
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or value < 0
        or value > MAX_U64
    ):
        raise ValueError(f"{field_name} must be a u64-safe integer value")
    return value


def _u64_le(value: int, field_name: str) -> bytes:
    return _normalize_u64(value, field_name).to_bytes(8, "little")


def _build_layout_args(layout: list[int], chunks: list[bytes]) -> bytes:
    return bytes([0xAB, *layout]) + b"".join(chunks)


def _ensure_bytes32(value: bytes, field_name: str) -> bytes:
    if len(value) != 32:
        raise ValueError(f"{field_name} must be exactly 32 bytes")
    if not any(value):
        raise ValueError(f"{field_name} must not be the zero hash")
    return value


def _decode_u64_le(data: bytes, offset: int = 0) -> int:
    return int.from_bytes(data[offset : offset + 8], "little")


def _decode_flag(data: bytes, offset: int, field_name: str) -> bool:
    value = _decode_u64_le(data, offset)
    if value not in (0, 1):
        raise RuntimeError(f"BountyBoard {field_name} must be encoded as 0 or 1")
    return value == 1


def _decode_return_data(value: str) -> bytes:
    return base64.b64decode(value.encode("ascii"))


def _ensure_return_code(
    result: Dict[str, Any], function_name: str, allowed_codes: tuple[int, ...] = (0,)
) -> None:
    code = int(result.get("returnCode") or 0)
    if code not in allowed_codes:
        raise RuntimeError(
            result.get("error") or f"BountyBoard {function_name} returned code {code}"
        )
    if result.get("success") is False:
        raise RuntimeError(
            str(result.get("error") or f"BountyBoard {function_name} failed")
        )


# --- Encoding helpers ---


def _encode_create_bounty_args(
    creator: PublicKey, title_hash: bytes, reward_amount: int, deadline_slot: int
) -> bytes:
    return _build_layout_args(
        [0x20, 0x20, 0x08, 0x08],
        [
            creator.to_bytes(),
            _ensure_bytes32(title_hash, "title_hash"),
            _u64_le(reward_amount, "reward_amount"),
            _u64_le(deadline_slot, "deadline_slot"),
        ],
    )


def _encode_submit_work_args(
    bounty_id: int, worker: PublicKey, proof_hash: bytes
) -> bytes:
    return _build_layout_args(
        [0x08, 0x20, 0x20],
        [
            _u64_le(bounty_id, "bounty_id"),
            worker.to_bytes(),
            _ensure_bytes32(proof_hash, "proof_hash"),
        ],
    )


def _encode_approve_work_args(
    caller: PublicKey, bounty_id: int, submission_idx: int
) -> bytes:
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


def _encode_submission_args(bounty_id: int, submission_idx: int) -> bytes:
    if (
        isinstance(submission_idx, bool)
        or not isinstance(submission_idx, int)
        or not 0 <= submission_idx <= 255
    ):
        raise ValueError("submission_idx must be 0-255")
    return _build_layout_args(
        [0x08, 0x01], [_u64_le(bounty_id, "bounty_id"), bytes([submission_idx])]
    )


def _encode_update_work_args(
    bounty_id: int, submission_idx: int, worker: PublicKey, proof_hash: bytes
) -> bytes:
    if (
        isinstance(submission_idx, bool)
        or not isinstance(submission_idx, int)
        or not 0 <= submission_idx <= 255
    ):
        raise ValueError("submission_idx must be 0-255")
    return _build_layout_args(
        [0x08, 0x01, 0x20, 0x20],
        [
            _u64_le(bounty_id, "bounty_id"),
            bytes([submission_idx]),
            worker.to_bytes(),
            _ensure_bytes32(proof_hash, "proof_hash"),
        ],
    )


def _encode_address_args(address: PublicKey | str) -> bytes:
    return _build_layout_args([0x20], [_normalize_public_key(address).to_bytes()])


def _encode_caller_address_args(caller: PublicKey, address: PublicKey | str) -> bytes:
    return _build_layout_args(
        [0x20, 0x20], [caller.to_bytes(), _normalize_public_key(address).to_bytes()]
    )


def _encode_caller_address_amount_args(
    caller: PublicKey, address: PublicKey | str, amount: int
) -> bytes:
    return _build_layout_args(
        [0x20, 0x20, 0x08],
        [
            caller.to_bytes(),
            _normalize_public_key(address).to_bytes(),
            _u64_le(amount, "amount"),
        ],
    )


def _encode_caller_u64_args(caller: PublicKey, value: int, field_name: str) -> bytes:
    return _build_layout_args(
        [0x20, 0x08], [caller.to_bytes(), _u64_le(value, field_name)]
    )


def _encode_migration_completion_args(
    caller: PublicKey,
    expected_escrow: int,
    expected_platform_fees: int,
    expected_total_liability: int,
) -> bytes:
    return _build_layout_args(
        [0x20, 0x08, 0x08, 0x08],
        [
            caller.to_bytes(),
            _u64_le(expected_escrow, "expected_escrow"),
            _u64_le(expected_platform_fees, "expected_platform_fees"),
            _u64_le(expected_total_liability, "expected_total_liability"),
        ],
    )


# --- Decoding helpers ---


def _decode_bounty_info(result: Dict[str, Any]) -> Dict[str, Any]:
    _ensure_return_code(result, "get_bounty")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("BountyBoard get_bounty did not return bounty data")
    data = _decode_return_data(return_data)
    if len(data) != BOUNTY_DATA_SIZE:
        raise RuntimeError("BountyBoard get_bounty payload must be exactly 91 bytes")
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
    if len(data) != PLATFORM_STATS_SIZE:
        raise RuntimeError(
            "BountyBoard get_platform_stats payload must be exactly 32 bytes"
        )
    return {
        "bounty_count": _decode_u64_le(data, 0),
        "completed_count": _decode_u64_le(data, 8),
        "reward_volume": _decode_u64_le(data, 16),
        "cancel_count": _decode_u64_le(data, 24),
    }


def _decode_submission(result: Dict[str, Any]) -> Dict[str, Any]:
    _ensure_return_code(result, "get_submission")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("BountyBoard get_submission did not return submission data")
    data = _decode_return_data(return_data)
    if len(data) != SUBMISSION_DATA_SIZE:
        raise RuntimeError(
            "BountyBoard get_submission payload must be exactly 72 bytes"
        )
    return {
        "worker": data[0:32],
        "proof_hash": data[32:64],
        "submitted_slot": _decode_u64_le(data, 64),
    }


def _decode_bounty_terms(result: Dict[str, Any]) -> Dict[str, Any]:
    _ensure_return_code(result, "get_bounty_terms")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("BountyBoard get_bounty_terms did not return terms data")
    data = _decode_return_data(return_data)
    if len(data) != BOUNTY_TERMS_SIZE:
        raise RuntimeError(
            "BountyBoard get_bounty_terms payload must be exactly 64 bytes"
        )
    return {
        "reward_token": data[0:32],
        "platform_fee_bps": _decode_u64_le(data, 32),
        "gross_reward": _decode_u64_le(data, 40),
        "worker_net": _decode_u64_le(data, 48),
        "platform_fee": _decode_u64_le(data, 56),
    }


def _decode_accounting_migration_status(result: Dict[str, Any]) -> Dict[str, Any]:
    _ensure_return_code(result, "get_accounting_migration_status")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError(
            "BountyBoard get_accounting_migration_status did not return data"
        )
    data = _decode_return_data(return_data)
    if len(data) != ACCOUNTING_MIGRATION_STATUS_SIZE:
        raise RuntimeError(
            "BountyBoard accounting migration status must be exactly 40 bytes"
        )
    return {
        "expected_bounty_count": _decode_u64_le(data, 0),
        "cursor": _decode_u64_le(data, 8),
        "reconstructed_escrow": _decode_u64_le(data, 16),
        "accounting_version": _decode_u64_le(data, 24),
        "locked": _decode_flag(data, 32, "migration lock"),
    }


def _decode_accounting_health(result: Dict[str, Any]) -> Dict[str, Any]:
    _ensure_return_code(result, "get_accounting_health")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("BountyBoard get_accounting_health did not return data")
    data = _decode_return_data(return_data)
    if len(data) != ACCOUNTING_HEALTH_SIZE:
        raise RuntimeError("BountyBoard accounting health must be exactly 56 bytes")
    return {
        "accounting_version": _decode_u64_le(data, 0),
        "migration_locked": _decode_flag(data, 8, "migration lock"),
        "escrow_liability": _decode_u64_le(data, 16),
        "platform_fees": _decode_u64_le(data, 24),
        "total_liability": _decode_u64_le(data, 32),
        "custody_balance": _decode_u64_le(data, 40),
        "solvent": _decode_flag(data, 48, "solvent flag"),
    }


def _decode_admin_transition(result: Dict[str, Any]) -> Dict[str, Any]:
    _ensure_return_code(result, "get_admin_transition")
    return_data = result.get("returnData")
    if not isinstance(return_data, str):
        raise RuntimeError("BountyBoard get_admin_transition did not return data")
    data = _decode_return_data(return_data)
    if len(data) != ADMIN_TRANSITION_SIZE:
        raise RuntimeError("BountyBoard admin transition must be exactly 64 bytes")
    pending = data[32:64]
    return {
        "current_admin": data[0:32],
        "pending_admin": pending if any(pending) else None,
    }


class BountyBoardClient:
    """High-level helper for common BountyBoard reads and writes."""

    def __init__(self, connection: Connection, program_id: Optional[PublicKey] = None):
        self.connection = connection
        self._program_id = program_id

    async def _call_readonly(
        self, function_name: str, args: bytes = b""
    ) -> Dict[str, Any]:
        program_id = await self.get_program_id()
        return await self.connection.call_readonly_contract(
            program_id, function_name, args
        )

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

        raise RuntimeError(
            'Unable to resolve the BountyBoard program via getSymbolRegistry("BOUNTY")'
        )

    # --- Read methods ---

    async def get_bounty(self, bounty_id: int) -> Optional[Dict[str, Any]]:
        normalized_id = _normalize_u64(bounty_id, "bounty_id")
        result = await self._call_readonly(
            "get_bounty", _encode_bounty_id_args(normalized_id)
        )
        if int(result.get("returnCode") or 0) == 1:
            return None
        return _decode_bounty_info(result)

    async def get_bounty_count(self) -> int:
        result = await self._call_readonly("get_bounty_count_exact")
        _ensure_return_code(result, "get_bounty_count_exact")
        return_data = result.get("returnData")
        if not isinstance(return_data, str):
            raise RuntimeError("BountyBoard get_bounty_count_exact did not return data")
        data = _decode_return_data(return_data)
        if len(data) != 8:
            raise RuntimeError(
                "BountyBoard get_bounty_count_exact payload must be exactly 8 bytes"
            )
        return _decode_u64_le(data, 0)

    async def get_platform_stats(self) -> Dict[str, int]:
        return _decode_platform_stats(await self._call_readonly("get_platform_stats"))

    async def get_submission(
        self, bounty_id: int, submission_idx: int
    ) -> Optional[Dict[str, Any]]:
        normalized_id = _normalize_u64(bounty_id, "bounty_id")
        result = await self._call_readonly(
            "get_submission", _encode_submission_args(normalized_id, submission_idx)
        )
        if int(result.get("returnCode") or 0) == 1:
            return None
        return _decode_submission(result)

    async def get_bounty_terms(self, bounty_id: int) -> Optional[Dict[str, Any]]:
        normalized_id = _normalize_u64(bounty_id, "bounty_id")
        result = await self._call_readonly(
            "get_bounty_terms", _encode_bounty_id_args(normalized_id)
        )
        if int(result.get("returnCode") or 0) == 1:
            return None
        return _decode_bounty_terms(result)

    async def get_platform_fees(self, token: PublicKey | str) -> int:
        result = await self._call_readonly(
            "get_platform_fees", _encode_address_args(token)
        )
        _ensure_return_code(result, "get_platform_fees")
        return_data = result.get("returnData")
        if not isinstance(return_data, str):
            raise RuntimeError("BountyBoard get_platform_fees did not return data")
        data = _decode_return_data(return_data)
        if len(data) != 8:
            raise RuntimeError(
                "BountyBoard get_platform_fees payload must be exactly 8 bytes"
            )
        return _decode_u64_le(data, 0)

    async def get_accounting_migration_status(self) -> Dict[str, Any]:
        return _decode_accounting_migration_status(
            await self._call_readonly("get_accounting_migration_status")
        )

    async def get_accounting_health(self) -> Dict[str, Any]:
        return _decode_accounting_health(
            await self._call_readonly("get_accounting_health")
        )

    async def get_admin_transition(self) -> Dict[str, Any]:
        return _decode_admin_transition(
            await self._call_readonly("get_admin_transition")
        )

    async def get_stats(self) -> Dict[str, Any]:
        stats = await self.connection.get_bountyboard_stats()
        return {
            "bounty_count": int(
                stats.get("bounty_count_exact", stats.get("bounty_count", 0))
            ),
            "completed_count": int(
                stats.get("completed_count_exact", stats.get("completed_count", 0))
            ),
            "total_reward_volume": int(
                stats.get(
                    "reward_volume_raw_exact",
                    stats.get("reward_volume", stats.get("total_reward_volume", 0)),
                )
            ),
            "cancel_count": int(
                stats.get("cancel_count_exact", stats.get("cancel_count", 0))
            ),
            "paused": bool(stats.get("paused")),
        }

    # --- Write methods ---

    async def create_bounty(
        self,
        creator: Keypair,
        title_hash: bytes,
        reward_amount: int,
        deadline_slot: int,
        payment_value: Optional[int] = None,
    ) -> str:
        normalized_reward = _normalize_u64(reward_amount, "reward_amount")
        normalized_deadline = _normalize_u64(deadline_slot, "deadline_slot")
        program_id = await self.get_program_id()
        args = _encode_create_bounty_args(
            creator.pubkey(), title_hash, normalized_reward, normalized_deadline
        )
        normalized_payment = (
            normalized_reward
            if payment_value is None
            else _normalize_u64(payment_value, "payment_value")
        )
        return await self.connection.call_contract(
            creator, program_id, "create_bounty", args, normalized_payment
        )

    async def submit_work(
        self, worker: Keypair, bounty_id: int, proof_hash: bytes
    ) -> str:
        normalized_id = _normalize_u64(bounty_id, "bounty_id")
        program_id = await self.get_program_id()
        args = _encode_submit_work_args(normalized_id, worker.pubkey(), proof_hash)
        return await self.connection.call_contract(
            worker, program_id, "submit_work", args
        )

    async def approve_work(
        self, creator: Keypair, bounty_id: int, submission_idx: int
    ) -> str:
        normalized_id = _normalize_u64(bounty_id, "bounty_id")
        program_id = await self.get_program_id()
        args = _encode_approve_work_args(
            creator.pubkey(), normalized_id, submission_idx
        )
        return await self.connection.call_contract(
            creator, program_id, "approve_work", args
        )

    async def cancel_bounty(self, creator: Keypair, bounty_id: int) -> str:
        normalized_id = _normalize_u64(bounty_id, "bounty_id")
        program_id = await self.get_program_id()
        args = _encode_cancel_bounty_args(creator.pubkey(), normalized_id)
        return await self.connection.call_contract(
            creator, program_id, "cancel_bounty", args
        )

    async def update_work(
        self, worker: Keypair, bounty_id: int, submission_idx: int, proof_hash: bytes
    ) -> str:
        normalized_id = _normalize_u64(bounty_id, "bounty_id")
        program_id = await self.get_program_id()
        args = _encode_update_work_args(
            normalized_id, submission_idx, worker.pubkey(), proof_hash
        )
        return await self.connection.call_contract(
            worker, program_id, "update_work", args
        )

    async def initialize(self, admin: Keypair) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(
            admin, program_id, "initialize", _encode_address_args(admin.pubkey())
        )

    async def set_identity_admin(self, admin: Keypair) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(
            admin,
            program_id,
            "set_identity_admin",
            _encode_address_args(admin.pubkey()),
        )

    async def propose_admin(self, admin: Keypair, new_admin: PublicKey | str) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(
            admin,
            program_id,
            "propose_admin",
            _encode_caller_address_args(admin.pubkey(), new_admin),
        )

    async def accept_admin(self, pending_admin: Keypair) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(
            pending_admin,
            program_id,
            "accept_admin",
            _encode_address_args(pending_admin.pubkey()),
        )

    async def cancel_admin_proposal(self, admin: Keypair) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(
            admin,
            program_id,
            "cancel_admin_proposal",
            _encode_address_args(admin.pubkey()),
        )

    async def set_lichenid_address(
        self, admin: Keypair, lichenid: PublicKey | str
    ) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(
            admin,
            program_id,
            "set_lichenid_address",
            _encode_caller_address_args(admin.pubkey(), lichenid),
        )

    async def set_identity_gate(self, admin: Keypair, min_reputation: int) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(
            admin,
            program_id,
            "set_identity_gate",
            _encode_caller_u64_args(admin.pubkey(), min_reputation, "min_reputation"),
        )

    async def set_token_address(self, admin: Keypair, token: PublicKey | str) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(
            admin,
            program_id,
            "set_token_address",
            _encode_caller_address_args(admin.pubkey(), token),
        )

    async def set_platform_fee(self, admin: Keypair, fee_bps: int) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(
            admin,
            program_id,
            "set_platform_fee",
            _encode_caller_u64_args(admin.pubkey(), fee_bps, "fee_bps"),
        )

    async def pause(self, admin: Keypair) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(
            admin, program_id, "bb_pause", _encode_address_args(admin.pubkey())
        )

    async def unpause(self, admin: Keypair) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(
            admin, program_id, "bb_unpause", _encode_address_args(admin.pubkey())
        )

    async def set_fee_treasury(self, admin: Keypair, treasury: PublicKey | str) -> str:
        program_id = await self.get_program_id()
        args = _encode_caller_address_args(admin.pubkey(), treasury)
        return await self.connection.call_contract(
            admin, program_id, "set_fee_treasury", args
        )

    async def withdraw_platform_fees(
        self, admin: Keypair, token: PublicKey | str, amount: int
    ) -> str:
        program_id = await self.get_program_id()
        args = _encode_caller_address_amount_args(admin.pubkey(), token, amount)
        return await self.connection.call_contract(
            admin, program_id, "withdraw_platform_fees", args
        )

    async def migrate_bounty_token(
        self, admin: Keypair, bounty_id: int, token: PublicKey | str
    ) -> str:
        program_id = await self.get_program_id()
        args = _build_layout_args(
            [0x20, 0x08, 0x20],
            [
                admin.pubkey().to_bytes(),
                _u64_le(bounty_id, "bounty_id"),
                _normalize_public_key(token).to_bytes(),
            ],
        )
        return await self.connection.call_contract(
            admin, program_id, "migrate_bounty_token", args
        )

    async def begin_accounting_v2_migration(
        self, admin: Keypair, expected_bounty_count: int
    ) -> str:
        program_id = await self.get_program_id()
        args = _encode_caller_u64_args(
            admin.pubkey(), expected_bounty_count, "expected_bounty_count"
        )
        return await self.connection.call_contract(
            admin, program_id, "begin_accounting_v2_migration", args
        )

    async def migrate_accounting_v2_bounty(
        self, caller: Keypair, bounty_id: int
    ) -> str:
        program_id = await self.get_program_id()
        return await self.connection.call_contract(
            caller,
            program_id,
            "migrate_accounting_v2_bounty",
            _encode_bounty_id_args(_normalize_u64(bounty_id, "bounty_id")),
        )

    async def complete_accounting_v2_migration(
        self,
        admin: Keypair,
        expected_escrow: int,
        expected_platform_fees: int,
        expected_total_liability: int,
    ) -> str:
        program_id = await self.get_program_id()
        args = _encode_migration_completion_args(
            admin.pubkey(),
            expected_escrow,
            expected_platform_fees,
            expected_total_liability,
        )
        return await self.connection.call_contract(
            admin, program_id, "complete_accounting_v2_migration", args
        )
