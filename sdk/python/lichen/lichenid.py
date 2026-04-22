"""First-class LichenID helper built on top of the Python SDK primitives."""

from __future__ import annotations

import base64
import json
from dataclasses import dataclass
from typing import Any, Dict, Optional

from .connection import Connection
from .keypair import Keypair
from .publickey import PublicKey

PREMIUM_NAME_MIN_LENGTH = 3
PREMIUM_NAME_MAX_LENGTH = 4
DIRECT_NAME_MIN_LENGTH = 5
MAX_NAME_LENGTH = 32
MAX_SKILL_NAME_BYTES = 32
MAX_ENDPOINT_BYTES = 255
MAX_METADATA_BYTES = 1024
RECOVERY_GUARDIAN_COUNT = 5
SPORES_PER_LICN = 1_000_000_000
MAX_U64 = (1 << 64) - 1
PROGRAM_SYMBOL_CANDIDATES = ("YID", "yid", "LICHENID")
AVAILABILITY_BY_NAME = {
    "offline": 0,
    "available": 1,
    "busy": 2,
    "online": 1,
}
LICHEN_ID_DELEGATE_PERMISSIONS = {
    "PROFILE": 0b0000_0001,
    "AGENT_TYPE": 0b0000_0010,
    "SKILLS": 0b0000_0100,
    "NAMING": 0b0000_1000,
}


def _normalize_public_key(value: PublicKey | str) -> PublicKey:
    return value if isinstance(value, PublicKey) else PublicKey(value)


def _normalize_name_label(name: str) -> str:
    return name.strip().lower().removesuffix(".lichen")


def _has_valid_name_characters(label: str) -> bool:
    return (
        bool(label)
        and not label.startswith("-")
        and not label.endswith("-")
        and "--" not in label
        and all(ch.isdigit() or ("a" <= ch <= "z") or ch == "-" for ch in label)
    )


def _validate_lookup_name(name: str) -> str:
    label = _normalize_name_label(name)
    if not label:
        raise ValueError("Name cannot be empty")
    if len(label) > MAX_NAME_LENGTH:
        raise ValueError("LichenID names must be at most 32 characters")
    if not _has_valid_name_characters(label):
        raise ValueError("LichenID names must use lowercase a-z, 0-9, and internal hyphens only")
    return label


def _validate_direct_registration_name(name: str) -> str:
    label = _validate_lookup_name(name)
    if len(label) < DIRECT_NAME_MIN_LENGTH:
        raise ValueError(
            "Direct register_name supports 5-32 character labels; 3-4 character names are auction-only"
        )
    return label


def _validate_auction_name(name: str) -> str:
    label = _validate_lookup_name(name)
    if len(label) < PREMIUM_NAME_MIN_LENGTH or len(label) > PREMIUM_NAME_MAX_LENGTH:
        raise ValueError("Name auction helpers support 3-4 character premium labels only")
    return label


def _normalize_duration_years(value: int | None) -> int:
    return max(1, min(10, int(value or 1)))


def _validate_skill_name(name: str) -> str:
    skill_name = name.strip()
    if not skill_name:
        raise ValueError("Skill name cannot be empty")
    if len(skill_name.encode("utf-8")) > MAX_SKILL_NAME_BYTES:
        raise ValueError("Skill names must be at most 32 bytes")
    return skill_name


def _normalize_endpoint_url(url: str) -> str:
    endpoint = url.strip()
    if not endpoint:
        raise ValueError("Endpoint URL cannot be empty")
    if len(endpoint.encode("utf-8")) > MAX_ENDPOINT_BYTES:
        raise ValueError("Endpoint URL must be at most 255 bytes")
    return endpoint


def _normalize_metadata(metadata: Any) -> str:
    if isinstance(metadata, str):
        serialized = metadata.strip()
    else:
        try:
            serialized = json.dumps(metadata, separators=(",", ":"))
        except TypeError as exc:
            raise ValueError("Metadata must be JSON-serializable") from exc
    if not serialized:
        raise ValueError("Metadata cannot be empty")
    if len(serialized.encode("utf-8")) > MAX_METADATA_BYTES:
        raise ValueError("Metadata must be at most 1024 bytes")
    return serialized


def _normalize_availability_status(status: int | str) -> int:
    if isinstance(status, str):
        normalized = AVAILABILITY_BY_NAME.get(status.strip().lower())
        if normalized is not None:
            return normalized
    elif isinstance(status, int) and not isinstance(status, bool) and 0 <= status <= 2:
        return status

    raise ValueError(
        "Availability must be one of offline, available, busy, or the numeric values 0-2"
    )


def _normalize_delegate_permissions(permissions: int) -> int:
    if isinstance(permissions, bool) or not isinstance(permissions, int) or permissions <= 0 or permissions > 0x0F:
        raise ValueError(
            "Delegate permissions must be a non-zero bitmask using PROFILE, AGENT_TYPE, SKILLS, and NAMING"
        )
    return permissions


def _normalize_attestation_level(level: int | None) -> int:
    normalized = int(level or 3)
    if normalized < 1 or normalized > 5:
        raise ValueError("Attestation level must be between 1 and 5")
    return normalized


def _normalize_u64(value: int, field_name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0 or value > MAX_U64:
        raise ValueError(f"{field_name} must be a u64-safe integer value")
    return value


def _normalize_recovery_guardians(owner: PublicKey, guardians: list[PublicKey | str]) -> list[PublicKey]:
    if len(guardians) != RECOVERY_GUARDIAN_COUNT:
        raise ValueError("Recovery helpers require exactly 5 guardian addresses")

    normalized = [_normalize_public_key(guardian) for guardian in guardians]
    unique = {str(guardian) for guardian in normalized}
    if len(unique) != RECOVERY_GUARDIAN_COUNT:
        raise ValueError("Recovery guardians must be unique")
    if any(guardian == owner for guardian in normalized):
        raise ValueError("Recovery guardians cannot include the owner")
    return normalized


def _pad_bytes(data: bytes, size: int) -> bytes:
    return data[:size] + b"\x00" * max(0, size - len(data))


def _build_layout_args(layout: list[int], chunks: list[bytes]) -> bytes:
    return bytes([0xAB, *layout]) + b"".join(chunks)


def _u8(value: int) -> bytes:
    return bytes([value & 0xFF])


def _u32_le(value: int) -> bytes:
    return int(value).to_bytes(4, "little")


def _u64_le(value: int, field_name: str) -> bytes:
    return _normalize_u64(value, field_name).to_bytes(8, "little")


def _encode_register_identity_args(owner: PublicKey, agent_type: int, name: str) -> bytes:
    name_bytes = name.encode("utf-8")
    return _build_layout_args([
        0x20, 0x01, 0x40, 0x04,
    ], [
        owner.to_bytes(),
        _u8(agent_type),
        _pad_bytes(name_bytes, 64),
        _u32_le(len(name_bytes)),
    ])


def _encode_name_duration_args(owner: PublicKey, name: str, duration_years: int) -> bytes:
    name_bytes = name.encode("utf-8")
    return _build_layout_args([
        0x20, 0x20, 0x04, 0x01,
    ], [
        owner.to_bytes(),
        _pad_bytes(name_bytes, 32),
        _u32_le(len(name_bytes)),
        _u8(duration_years),
    ])


def _encode_add_skill_args(owner: PublicKey, name: str, proficiency: int) -> bytes:
    skill_name = _validate_skill_name(name)
    clamped_proficiency = max(0, min(100, int(proficiency)))
    name_bytes = skill_name.encode("utf-8")
    return _build_layout_args([
        0x20, 0x20, 0x04, 0x01,
    ], [
        owner.to_bytes(),
        _pad_bytes(name_bytes, 32),
        _u32_le(len(name_bytes)),
        _u8(clamped_proficiency),
    ])


def _encode_vouch_args(owner: PublicKey, vouchee: PublicKey) -> bytes:
    return _build_layout_args([0x20, 0x20], [owner.to_bytes(), vouchee.to_bytes()])


def _encode_endpoint_args(owner: PublicKey, url: str) -> bytes:
    endpoint = _normalize_endpoint_url(url)
    url_bytes = endpoint.encode("utf-8")
    stride = max(32, len(url_bytes))
    return _build_layout_args([
        0x20, stride, 0x04,
    ], [
        owner.to_bytes(),
        _pad_bytes(url_bytes, stride),
        _u32_le(len(url_bytes)),
    ])


def _encode_metadata_args(owner: PublicKey, metadata: Any) -> bytes:
    serialized = _normalize_metadata(metadata)
    metadata_bytes = serialized.encode("utf-8")
    stride = max(32, len(metadata_bytes))
    return _build_layout_args([
        0x20, stride, 0x04,
    ], [
        owner.to_bytes(),
        _pad_bytes(metadata_bytes, stride),
        _u32_le(len(metadata_bytes)),
    ])


def _encode_rate_args(owner: PublicKey, rate_spores: int) -> bytes:
    return _build_layout_args([0x20, 0x08], [owner.to_bytes(), _u64_le(rate_spores, "rate_spores")])


def _encode_availability_args(owner: PublicKey, status: int | str) -> bytes:
    return _build_layout_args([
        0x20, 0x01,
    ], [
        owner.to_bytes(),
        _u8(_normalize_availability_status(status)),
    ])


def _encode_set_delegate_args(owner: PublicKey, delegate: PublicKey, permissions: int, expires_at_ms: int) -> bytes:
    return _build_layout_args([
        0x20, 0x20, 0x01, 0x08,
    ], [
        owner.to_bytes(),
        delegate.to_bytes(),
        _u8(_normalize_delegate_permissions(permissions)),
        _u64_le(expires_at_ms, "expires_at_ms"),
    ])


def _encode_delegate_lookup_args(owner: PublicKey, delegate: PublicKey) -> bytes:
    return _build_layout_args([0x20, 0x20], [owner.to_bytes(), delegate.to_bytes()])


def _encode_delegated_endpoint_args(delegate: PublicKey, owner: PublicKey, url: str) -> bytes:
    endpoint = _normalize_endpoint_url(url)
    url_bytes = endpoint.encode("utf-8")
    stride = max(32, len(url_bytes))
    return _build_layout_args([
        0x20, 0x20, stride, 0x04,
    ], [
        delegate.to_bytes(),
        owner.to_bytes(),
        _pad_bytes(url_bytes, stride),
        _u32_le(len(url_bytes)),
    ])


def _encode_delegated_metadata_args(delegate: PublicKey, owner: PublicKey, metadata: Any) -> bytes:
    serialized = _normalize_metadata(metadata)
    metadata_bytes = serialized.encode("utf-8")
    stride = max(32, len(metadata_bytes))
    return _build_layout_args([
        0x20, 0x20, stride, 0x04,
    ], [
        delegate.to_bytes(),
        owner.to_bytes(),
        _pad_bytes(metadata_bytes, stride),
        _u32_le(len(metadata_bytes)),
    ])


def _encode_delegated_availability_args(delegate: PublicKey, owner: PublicKey, status: int | str) -> bytes:
    return _build_layout_args([
        0x20, 0x20, 0x01,
    ], [
        delegate.to_bytes(),
        owner.to_bytes(),
        _u8(_normalize_availability_status(status)),
    ])


def _encode_delegated_rate_args(delegate: PublicKey, owner: PublicKey, rate_spores: int) -> bytes:
    return _build_layout_args([
        0x20, 0x20, 0x08,
    ], [
        delegate.to_bytes(),
        owner.to_bytes(),
        _u64_le(rate_spores, "rate_spores"),
    ])


def _encode_update_agent_type_as_args(delegate: PublicKey, owner: PublicKey, agent_type: int) -> bytes:
    return _build_layout_args([
        0x20, 0x20, 0x01,
    ], [
        delegate.to_bytes(),
        owner.to_bytes(),
        _u8(agent_type),
    ])


def _encode_recovery_guardians_args(owner: PublicKey, guardians: list[PublicKey | str]) -> bytes:
    normalized = _normalize_recovery_guardians(owner, guardians)
    return _build_layout_args([
        0x20, 0x20, 0x20, 0x20, 0x20, 0x20,
    ], [
        owner.to_bytes(),
        normalized[0].to_bytes(),
        normalized[1].to_bytes(),
        normalized[2].to_bytes(),
        normalized[3].to_bytes(),
        normalized[4].to_bytes(),
    ])


def _encode_recovery_action_args(caller: PublicKey, target: PublicKey | str, new_owner: PublicKey | str) -> bytes:
    return _build_layout_args([
        0x20, 0x20, 0x20,
    ], [
        caller.to_bytes(),
        _normalize_public_key(target).to_bytes(),
        _normalize_public_key(new_owner).to_bytes(),
    ])


def _encode_attest_skill_args(attester: PublicKey, identity: PublicKey | str, name: str, level: int) -> bytes:
    skill_name = _validate_skill_name(name)
    name_bytes = skill_name.encode("utf-8")
    return _build_layout_args([
        0x20, 0x20, 0x20, 0x04, 0x01,
    ], [
        attester.to_bytes(),
        _normalize_public_key(identity).to_bytes(),
        _pad_bytes(name_bytes, 32),
        _u32_le(len(name_bytes)),
        _u8(_normalize_attestation_level(level)),
    ])


def _encode_get_attestations_args(identity: PublicKey | str, name: str) -> bytes:
    skill_name = _validate_skill_name(name)
    name_bytes = skill_name.encode("utf-8")
    return _build_layout_args([
        0x20, 0x20, 0x04,
    ], [
        _normalize_public_key(identity).to_bytes(),
        _pad_bytes(name_bytes, 32),
        _u32_le(len(name_bytes)),
    ])


def _encode_revoke_attestation_args(attester: PublicKey, identity: PublicKey | str, name: str) -> bytes:
    skill_name = _validate_skill_name(name)
    name_bytes = skill_name.encode("utf-8")
    return _build_layout_args([
        0x20, 0x20, 0x20, 0x04,
    ], [
        attester.to_bytes(),
        _normalize_public_key(identity).to_bytes(),
        _pad_bytes(name_bytes, 32),
        _u32_le(len(name_bytes)),
    ])


def _encode_create_name_auction_args(
    owner: PublicKey,
    name: str,
    reserve_bid_spores: int,
    end_slot: int,
) -> bytes:
    label = _validate_auction_name(name)
    name_bytes = label.encode("utf-8")
    return _build_layout_args([
        0x20, 0x20, 0x04, 0x08, 0x08,
    ], [
        owner.to_bytes(),
        _pad_bytes(name_bytes, 32),
        _u32_le(len(name_bytes)),
        _u64_le(reserve_bid_spores, "reserve_bid_spores"),
        _u64_le(end_slot, "end_slot"),
    ])


def _encode_bid_name_auction_args(owner: PublicKey, name: str, bid_amount_spores: int) -> bytes:
    label = _validate_auction_name(name)
    name_bytes = label.encode("utf-8")
    return _build_layout_args([
        0x20, 0x20, 0x04, 0x08,
    ], [
        owner.to_bytes(),
        _pad_bytes(name_bytes, 32),
        _u32_le(len(name_bytes)),
        _u64_le(bid_amount_spores, "bid_amount_spores"),
    ])


def _decode_u64_le(data: bytes, offset: int = 0) -> int:
    return int.from_bytes(data[offset : offset + 8], "little")


def _decode_return_data(value: str) -> bytes:
    return base64.b64decode(value.encode("ascii"))


def _ensure_return_code_zero(result: Dict[str, Any], function_name: str) -> None:
    code = int(result.get("returnCode") or 0)
    if code != 0:
        raise RuntimeError(result.get("error") or f"LichenID {function_name} returned code {code}")
    if result.get("success") is False and result.get("error"):
        raise RuntimeError(str(result["error"]))


def _decode_delegate_record(owner: PublicKey, delegate: PublicKey, data: bytes) -> Dict[str, Any]:
    if len(data) < 17:
        raise RuntimeError("Delegate record payload was shorter than expected")

    permissions = data[0]
    expires_at_ms = _decode_u64_le(data, 1)
    created_at_ms = _decode_u64_le(data, 9)
    return {
        "owner": str(owner),
        "delegate": str(delegate),
        "permissions": permissions,
        "expires_at_ms": expires_at_ms,
        "created_at_ms": created_at_ms,
        "active": True,
        "can_profile": bool(permissions & LICHEN_ID_DELEGATE_PERMISSIONS["PROFILE"]),
        "can_agent_type": bool(permissions & LICHEN_ID_DELEGATE_PERMISSIONS["AGENT_TYPE"]),
        "can_skills": bool(permissions & LICHEN_ID_DELEGATE_PERMISSIONS["SKILLS"]),
        "can_naming": bool(permissions & LICHEN_ID_DELEGATE_PERMISSIONS["NAMING"]),
    }


def registration_cost_per_year_licn(name: str) -> int:
    label = _normalize_name_label(name)
    if len(label) <= 3:
        return 500
    if len(label) == 4:
        return 100
    return 20


def estimate_lichenid_name_registration_cost(name: str, duration_years: int = 1) -> int:
    years = _normalize_duration_years(duration_years)
    return registration_cost_per_year_licn(name) * years * SPORES_PER_LICN


@dataclass(frozen=True)
class RegisterIdentityParams:
    agent_type: int
    name: str


@dataclass(frozen=True)
class RegisterNameParams:
    name: str
    duration_years: int = 1
    value_spores: Optional[int] = None


@dataclass(frozen=True)
class AddSkillParams:
    name: str
    proficiency: int = 50


@dataclass(frozen=True)
class SetEndpointParams:
    url: str


@dataclass(frozen=True)
class SetMetadataParams:
    metadata: Any


@dataclass(frozen=True)
class SetRateParams:
    rate_spores: int


@dataclass(frozen=True)
class SetAvailabilityParams:
    status: int | str


@dataclass(frozen=True)
class SetDelegateParams:
    delegate: PublicKey | str
    permissions: int
    expires_at_ms: int


@dataclass(frozen=True)
class SetEndpointAsParams:
    owner: PublicKey | str
    url: str


@dataclass(frozen=True)
class SetMetadataAsParams:
    owner: PublicKey | str
    metadata: Any


@dataclass(frozen=True)
class SetAvailabilityAsParams:
    owner: PublicKey | str
    status: int | str


@dataclass(frozen=True)
class SetRateAsParams:
    owner: PublicKey | str
    rate_spores: int


@dataclass(frozen=True)
class UpdateAgentTypeAsParams:
    owner: PublicKey | str
    agent_type: int


@dataclass(frozen=True)
class SetRecoveryGuardiansParams:
    guardians: list[PublicKey | str]


@dataclass(frozen=True)
class ApproveRecoveryParams:
    target: PublicKey | str
    new_owner: PublicKey | str


@dataclass(frozen=True)
class ExecuteRecoveryParams:
    target: PublicKey | str
    new_owner: PublicKey | str


@dataclass(frozen=True)
class AttestSkillParams:
    identity: PublicKey | str
    name: str
    level: int = 3


@dataclass(frozen=True)
class RevokeAttestationParams:
    identity: PublicKey | str
    name: str


@dataclass(frozen=True)
class CreateNameAuctionParams:
    name: str
    reserve_bid_spores: int
    end_slot: int


@dataclass(frozen=True)
class BidNameAuctionParams:
    name: str
    bid_amount_spores: int


@dataclass(frozen=True)
class FinalizeNameAuctionParams:
    name: str
    duration_years: int = 1


class LichenIdClient:
    """High-level helper for common LichenID reads and writes."""

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

        raise RuntimeError('Unable to resolve the LichenID program via getSymbolRegistry("YID")')

    async def get_profile(self, address: PublicKey | str) -> Optional[Dict[str, Any]]:
        return await self.connection.get_lichenid_profile(_normalize_public_key(address))

    async def get_reputation(self, address: PublicKey | str) -> Dict[str, Any]:
        return await self.connection.get_lichenid_reputation(_normalize_public_key(address))

    async def get_skills(self, address: PublicKey | str) -> list[Dict[str, Any]]:
        return await self.connection.get_lichenid_skills(_normalize_public_key(address))

    async def get_vouches(self, address: PublicKey | str) -> Dict[str, Any]:
        return await self.connection.get_lichenid_vouches(_normalize_public_key(address))

    async def resolve_name(self, name: str) -> Optional[Dict[str, Any]]:
        label = _validate_lookup_name(name)
        return await self.connection.resolve_lichen_name(f"{label}.lichen")

    async def get_metadata(self, address: PublicKey | str) -> Any:
        profile = await self.get_profile(address)
        if not isinstance(profile, dict):
            return None
        agent = profile.get("agent") or {}
        return agent.get("metadata")

    async def get_delegate(self, owner: PublicKey | str, delegate: PublicKey | str) -> Optional[Dict[str, Any]]:
        owner_key = _normalize_public_key(owner)
        delegate_key = _normalize_public_key(delegate)
        result = await self._call_readonly(
            "get_delegate",
            _encode_delegate_lookup_args(owner_key, delegate_key),
        )
        if int(result.get("returnCode") or 0) == 1 or not result.get("returnData"):
            return None
        _ensure_return_code_zero(result, "get_delegate")
        record = _decode_delegate_record(owner_key, delegate_key, _decode_return_data(result["returnData"]))
        record["active"] = record["expires_at_ms"] > 0
        return record

    async def get_attestations(self, identity: PublicKey | str, name: str) -> int:
        result = await self._call_readonly(
            "get_attestations",
            _encode_get_attestations_args(identity, name),
        )
        _ensure_return_code_zero(result, "get_attestations")
        return_data = result.get("returnData")
        if not isinstance(return_data, str):
            raise RuntimeError("LichenID get_attestations did not return attestation data")
        return _decode_u64_le(_decode_return_data(return_data))

    async def get_name_auction(self, name: str) -> Optional[Dict[str, Any]]:
        return await self.connection.get_name_auction(_validate_lookup_name(name))

    async def get_agent_directory(self, options: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        return await self.connection.get_lichenid_agent_directory(options)

    async def get_stats(self) -> Dict[str, Any]:
        return await self.connection.get_lichenid_stats()

    async def register_identity(self, owner: Keypair, params: RegisterIdentityParams) -> str:
        name = params.name.strip()
        if not name:
            raise ValueError("Identity name cannot be empty")

        program_id = await self.get_program_id()
        args = _encode_register_identity_args(owner.pubkey(), params.agent_type, name)
        return await self.connection.call_contract(owner, program_id, "register_identity", args)

    async def register_name(self, owner: Keypair, params: RegisterNameParams) -> str:
        duration_years = _normalize_duration_years(params.duration_years)
        label = _validate_direct_registration_name(params.name)
        value_spores = params.value_spores
        if value_spores is None:
            value_spores = estimate_lichenid_name_registration_cost(label, duration_years)

        program_id = await self.get_program_id()
        args = _encode_name_duration_args(owner.pubkey(), label, duration_years)
        return await self.connection.call_contract(owner, program_id, "register_name", args, value_spores)

    async def add_skill(self, owner: Keypair, params: AddSkillParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_add_skill_args(owner.pubkey(), params.name, params.proficiency)
        return await self.connection.call_contract(owner, program_id, "add_skill", args)

    async def vouch(self, owner: Keypair, vouchee: PublicKey | str) -> str:
        program_id = await self.get_program_id()
        args = _encode_vouch_args(owner.pubkey(), _normalize_public_key(vouchee))
        return await self.connection.call_contract(owner, program_id, "vouch", args)

    async def set_endpoint(self, owner: Keypair, params: SetEndpointParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_endpoint_args(owner.pubkey(), params.url)
        return await self.connection.call_contract(owner, program_id, "set_endpoint", args)

    async def set_metadata(self, owner: Keypair, params: SetMetadataParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_metadata_args(owner.pubkey(), params.metadata)
        return await self.connection.call_contract(owner, program_id, "set_metadata", args)

    async def set_rate(self, owner: Keypair, params: SetRateParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_rate_args(owner.pubkey(), params.rate_spores)
        return await self.connection.call_contract(owner, program_id, "set_rate", args)

    async def set_availability(self, owner: Keypair, params: SetAvailabilityParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_availability_args(owner.pubkey(), params.status)
        return await self.connection.call_contract(owner, program_id, "set_availability", args)

    async def set_delegate(self, owner: Keypair, params: SetDelegateParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_set_delegate_args(
            owner.pubkey(),
            _normalize_public_key(params.delegate),
            params.permissions,
            params.expires_at_ms,
        )
        return await self.connection.call_contract(owner, program_id, "set_delegate", args)

    async def revoke_delegate(self, owner: Keypair, delegate: PublicKey | str) -> str:
        program_id = await self.get_program_id()
        args = _encode_delegate_lookup_args(owner.pubkey(), _normalize_public_key(delegate))
        return await self.connection.call_contract(owner, program_id, "revoke_delegate", args)

    async def set_endpoint_as(self, delegate: Keypair, params: SetEndpointAsParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_delegated_endpoint_args(delegate.pubkey(), _normalize_public_key(params.owner), params.url)
        return await self.connection.call_contract(delegate, program_id, "set_endpoint_as", args)

    async def set_metadata_as(self, delegate: Keypair, params: SetMetadataAsParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_delegated_metadata_args(delegate.pubkey(), _normalize_public_key(params.owner), params.metadata)
        return await self.connection.call_contract(delegate, program_id, "set_metadata_as", args)

    async def set_availability_as(self, delegate: Keypair, params: SetAvailabilityAsParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_delegated_availability_args(delegate.pubkey(), _normalize_public_key(params.owner), params.status)
        return await self.connection.call_contract(delegate, program_id, "set_availability_as", args)

    async def set_rate_as(self, delegate: Keypair, params: SetRateAsParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_delegated_rate_args(delegate.pubkey(), _normalize_public_key(params.owner), params.rate_spores)
        return await self.connection.call_contract(delegate, program_id, "set_rate_as", args)

    async def update_agent_type_as(self, delegate: Keypair, params: UpdateAgentTypeAsParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_update_agent_type_as_args(delegate.pubkey(), _normalize_public_key(params.owner), params.agent_type)
        return await self.connection.call_contract(delegate, program_id, "update_agent_type_as", args)

    async def set_recovery_guardians(self, owner: Keypair, params: SetRecoveryGuardiansParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_recovery_guardians_args(owner.pubkey(), params.guardians)
        return await self.connection.call_contract(owner, program_id, "set_recovery_guardians", args)

    async def approve_recovery(self, guardian: Keypair, params: ApproveRecoveryParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_recovery_action_args(guardian.pubkey(), params.target, params.new_owner)
        return await self.connection.call_contract(guardian, program_id, "approve_recovery", args)

    async def execute_recovery(self, guardian: Keypair, params: ExecuteRecoveryParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_recovery_action_args(guardian.pubkey(), params.target, params.new_owner)
        return await self.connection.call_contract(guardian, program_id, "execute_recovery", args)

    async def attest_skill(self, attester: Keypair, params: AttestSkillParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_attest_skill_args(attester.pubkey(), params.identity, params.name, params.level)
        return await self.connection.call_contract(attester, program_id, "attest_skill", args)

    async def revoke_attestation(self, attester: Keypair, params: RevokeAttestationParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_revoke_attestation_args(attester.pubkey(), params.identity, params.name)
        return await self.connection.call_contract(attester, program_id, "revoke_attestation", args)

    async def create_name_auction(self, owner: Keypair, params: CreateNameAuctionParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_create_name_auction_args(
            owner.pubkey(),
            params.name,
            params.reserve_bid_spores,
            params.end_slot,
        )
        return await self.connection.call_contract(owner, program_id, "create_name_auction", args)

    async def bid_name_auction(self, owner: Keypair, params: BidNameAuctionParams) -> str:
        program_id = await self.get_program_id()
        args = _encode_bid_name_auction_args(owner.pubkey(), params.name, params.bid_amount_spores)
        return await self.connection.call_contract(
            owner,
            program_id,
            "bid_name_auction",
            args,
            params.bid_amount_spores,
        )

    async def finalize_name_auction(self, owner: Keypair, params: FinalizeNameAuctionParams) -> str:
        duration_years = _normalize_duration_years(params.duration_years)
        label = _validate_auction_name(params.name)
        program_id = await self.get_program_id()
        args = _encode_name_duration_args(owner.pubkey(), label, duration_years)
        return await self.connection.call_contract(owner, program_id, "finalize_name_auction", args)