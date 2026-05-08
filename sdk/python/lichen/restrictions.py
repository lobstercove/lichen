"""Restriction governance RPC helpers for the Lichen Python SDK."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, Mapping, Optional, Union

from .connection import Connection
from .publickey import PublicKey

MAX_U64 = (1 << 64) - 1

AddressInput = Union[PublicKey, str]
RestrictionAssetInput = Union[PublicKey, str]
RestrictionReasonInput = Union[str, int]
RestrictionLiftReasonInput = Union[str, int]
RestrictionModeInput = Union[str, int]

_MISSING = object()


@dataclass(frozen=True)
class AccountRestrictionTarget:
    account: AddressInput
    type: str = "account"


@dataclass(frozen=True)
class AccountAssetRestrictionTarget:
    account: AddressInput
    asset: RestrictionAssetInput
    type: str = "account_asset"


@dataclass(frozen=True)
class AssetRestrictionTarget:
    asset: RestrictionAssetInput
    type: str = "asset"


@dataclass(frozen=True)
class ContractRestrictionTarget:
    contract: AddressInput
    type: str = "contract"


@dataclass(frozen=True)
class CodeHashRestrictionTarget:
    code_hash: str
    type: str = "code_hash"


@dataclass(frozen=True)
class BridgeRouteRestrictionTarget:
    chain: str
    asset: str
    type: str = "bridge_route"


@dataclass(frozen=True)
class ProtocolModuleRestrictionTarget:
    module: Union[str, int]
    type: str = "protocol_module"


RestrictionTargetInput = Union[
    AccountRestrictionTarget,
    AccountAssetRestrictionTarget,
    AssetRestrictionTarget,
    ContractRestrictionTarget,
    CodeHashRestrictionTarget,
    BridgeRouteRestrictionTarget,
    ProtocolModuleRestrictionTarget,
    Mapping[str, Any],
]


@dataclass(frozen=True)
class RestrictAccountParams:
    proposer: AddressInput
    governance_authority: AddressInput
    account: AddressInput
    reason: RestrictionReasonInput
    mode: Optional[RestrictionModeInput] = None
    recent_blockhash: Optional[str] = None
    evidence_hash: Optional[str] = None
    evidence_uri_hash: Optional[str] = None
    expires_at_slot: Optional[int] = None


@dataclass(frozen=True)
class UnrestrictAccountParams:
    proposer: AddressInput
    governance_authority: AddressInput
    account: AddressInput
    lift_reason: RestrictionLiftReasonInput
    restriction_id: Optional[int] = None
    recent_blockhash: Optional[str] = None


@dataclass(frozen=True)
class RestrictAccountAssetParams:
    proposer: AddressInput
    governance_authority: AddressInput
    account: AddressInput
    asset: RestrictionAssetInput
    reason: RestrictionReasonInput
    mode: Optional[RestrictionModeInput] = None
    recent_blockhash: Optional[str] = None
    evidence_hash: Optional[str] = None
    evidence_uri_hash: Optional[str] = None
    expires_at_slot: Optional[int] = None


@dataclass(frozen=True)
class UnrestrictAccountAssetParams:
    proposer: AddressInput
    governance_authority: AddressInput
    account: AddressInput
    asset: RestrictionAssetInput
    lift_reason: RestrictionLiftReasonInput
    restriction_id: Optional[int] = None
    recent_blockhash: Optional[str] = None


@dataclass(frozen=True)
class SetFrozenAssetAmountParams:
    proposer: AddressInput
    governance_authority: AddressInput
    account: AddressInput
    asset: RestrictionAssetInput
    amount: int
    reason: RestrictionReasonInput
    recent_blockhash: Optional[str] = None
    evidence_hash: Optional[str] = None
    evidence_uri_hash: Optional[str] = None
    expires_at_slot: Optional[int] = None


@dataclass(frozen=True)
class ContractRestrictionParams:
    proposer: AddressInput
    governance_authority: AddressInput
    contract: AddressInput
    reason: RestrictionReasonInput
    recent_blockhash: Optional[str] = None
    evidence_hash: Optional[str] = None
    evidence_uri_hash: Optional[str] = None
    expires_at_slot: Optional[int] = None


@dataclass(frozen=True)
class ResumeContractParams:
    proposer: AddressInput
    governance_authority: AddressInput
    contract: AddressInput
    lift_reason: RestrictionLiftReasonInput
    restriction_id: Optional[int] = None
    recent_blockhash: Optional[str] = None


@dataclass(frozen=True)
class CodeHashRestrictionParams:
    proposer: AddressInput
    governance_authority: AddressInput
    code_hash: str
    reason: RestrictionReasonInput
    recent_blockhash: Optional[str] = None
    evidence_hash: Optional[str] = None
    evidence_uri_hash: Optional[str] = None
    expires_at_slot: Optional[int] = None


@dataclass(frozen=True)
class UnbanCodeHashParams:
    proposer: AddressInput
    governance_authority: AddressInput
    code_hash: str
    lift_reason: RestrictionLiftReasonInput
    restriction_id: Optional[int] = None
    recent_blockhash: Optional[str] = None


@dataclass(frozen=True)
class BridgeRouteRestrictionParams:
    proposer: AddressInput
    governance_authority: AddressInput
    chain: str
    asset: str
    reason: RestrictionReasonInput
    recent_blockhash: Optional[str] = None
    evidence_hash: Optional[str] = None
    evidence_uri_hash: Optional[str] = None
    expires_at_slot: Optional[int] = None


@dataclass(frozen=True)
class ResumeBridgeRouteParams:
    proposer: AddressInput
    governance_authority: AddressInput
    chain: str
    asset: str
    lift_reason: RestrictionLiftReasonInput
    restriction_id: Optional[int] = None
    recent_blockhash: Optional[str] = None


@dataclass(frozen=True)
class ExtendRestrictionParams:
    proposer: AddressInput
    governance_authority: AddressInput
    restriction_id: int
    new_expires_at_slot: Optional[int] = None
    evidence_hash: Optional[str] = None
    recent_blockhash: Optional[str] = None


@dataclass(frozen=True)
class LiftRestrictionParams:
    proposer: AddressInput
    governance_authority: AddressInput
    restriction_id: int
    lift_reason: RestrictionLiftReasonInput
    recent_blockhash: Optional[str] = None


@dataclass(frozen=True)
class MovementRestrictionParams:
    account: AddressInput
    asset: RestrictionAssetInput
    amount: Optional[int] = None


@dataclass(frozen=True)
class TransferRestrictionParams:
    from_: AddressInput
    to: AddressInput
    asset: RestrictionAssetInput
    amount: Optional[int] = None


def _get(value: Any, names: tuple[str, ...], default: Any = _MISSING) -> Any:
    if isinstance(value, Mapping):
        for name in names:
            if name in value:
                return value[name]
    else:
        for name in names:
            if hasattr(value, name):
                return getattr(value, name)

    if default is not _MISSING:
        return default
    raise ValueError(f"{names[0]} is required")


def _address(value: AddressInput) -> str:
    if isinstance(value, PublicKey):
        return str(value)
    if isinstance(value, str):
        return value
    raise ValueError("address values must be PublicKey or base58 string")


def _asset(value: RestrictionAssetInput) -> str:
    if isinstance(value, PublicKey):
        return str(value)
    if isinstance(value, str):
        return value
    raise ValueError("asset values must be PublicKey or string")


def _u64(value: int, field_name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0 or value > MAX_U64:
        raise ValueError(f"{field_name} must be a u64-safe integer value")
    return value


def _optional_u64(value: Optional[int], field_name: str) -> Optional[int]:
    if value is None:
        return None
    return _u64(value, field_name)


def _omit_none(value: Dict[str, Any]) -> Dict[str, Any]:
    return {key: item for key, item in value.items() if item is not None}


def _normalized_type(value: Any) -> Optional[str]:
    target_type = _get(value, ("type", "target_type", "targetType"), None)
    if isinstance(target_type, str):
        return target_type.lower().replace("-", "_")
    return None


def _infer_target_type(value: Any) -> str:
    explicit = _normalized_type(value)
    if explicit:
        return explicit

    field_sets = [
        (("chain", "chain_id", "chainId"), "bridge_route"),
        (("account", "address", "pubkey"), "account"),
        (("asset", "mint"), "asset"),
        (("contract", "contract_id", "contractId"), "contract"),
        (("code_hash", "codeHash"), "code_hash"),
        (("module", "module_id", "moduleId"), "protocol_module"),
    ]

    has_account = _get(value, ("account", "address", "pubkey"), None) is not None
    has_asset = _get(value, ("asset", "mint"), None) is not None
    if has_account and has_asset:
        return "account_asset"

    for names, target_type in field_sets:
        if _get(value, names, None) is not None:
            return target_type

    raise ValueError("restriction target type is required")


def _target_params(target: RestrictionTargetInput) -> Dict[str, Any]:
    target_type = _infer_target_type(target)

    if target_type in ("account",):
        return {
            "type": "account",
            "account": _address(_get(target, ("account", "address", "pubkey"))),
        }
    if target_type in ("account_asset", "accountasset"):
        return {
            "type": "account_asset",
            "account": _address(_get(target, ("account", "address", "pubkey"))),
            "asset": _asset(_get(target, ("asset", "mint"))),
        }
    if target_type == "asset":
        return {"type": "asset", "asset": _asset(_get(target, ("asset", "mint")))}
    if target_type == "contract":
        return {
            "type": "contract",
            "contract": _address(_get(target, ("contract", "contract_id", "contractId", "account"))),
        }
    if target_type in ("code_hash", "codehash"):
        return {
            "type": "code_hash",
            "code_hash": _get(target, ("code_hash", "codeHash")),
        }
    if target_type in ("bridge_route", "bridgeroute"):
        return {
            "type": "bridge_route",
            "chain": _get(target, ("chain", "chain_id", "chainId")),
            "asset": _get(target, ("asset",)),
        }
    if target_type in ("protocol_module", "protocolmodule"):
        return {
            "type": "protocol_module",
            "module": _get(target, ("module", "module_id", "moduleId")),
        }

    raise ValueError(f"unsupported restriction target type: {target_type}")


def _page_params(limit: Optional[int], after_id: Optional[int], cursor: Optional[Union[int, str]]) -> Dict[str, Any]:
    if isinstance(cursor, str):
        normalized_cursor: Optional[Union[int, str]] = cursor
    else:
        normalized_cursor = _optional_u64(cursor, "cursor")

    return _omit_none(
        {
            "limit": _optional_u64(limit, "limit"),
            "after_id": _optional_u64(after_id, "after_id"),
            "cursor": normalized_cursor,
        }
    )


def _builder_base(params: Any) -> Dict[str, Any]:
    return _omit_none(
        {
            "proposer": _address(_get(params, ("proposer",))),
            "governance_authority": _address(
                _get(params, ("governance_authority", "governanceAuthority"))
            ),
            "recent_blockhash": _get(params, ("recent_blockhash", "recentBlockhash"), None),
        }
    )


def _restrict_common(params: Any) -> Dict[str, Any]:
    return _omit_none(
        {
            **_builder_base(params),
            "reason": _get(params, ("reason",)),
            "evidence_hash": _get(params, ("evidence_hash", "evidenceHash"), None),
            "evidence_uri_hash": _get(params, ("evidence_uri_hash", "evidenceUriHash"), None),
            "expires_at_slot": _optional_u64(
                _get(params, ("expires_at_slot", "expiresAtSlot"), None),
                "expires_at_slot",
            ),
        }
    )


def _restriction_id_param(params: Any) -> Optional[int]:
    return _optional_u64(_get(params, ("restriction_id", "restrictionId", "id"), None), "restriction_id")


class RestrictionGovernanceClient:
    """Typed helper for restriction-governance read and transaction-builder RPCs."""

    def __init__(self, connection: Connection):
        self.connection = connection

    async def _rpc(self, method: str, params: Optional[list[Any]] = None) -> Any:
        return await self.connection.rpc_request(method, params or [])

    async def get_restriction(self, restriction_id: int) -> Dict[str, Any]:
        return await self._rpc("getRestriction", [_u64(restriction_id, "restriction_id")])

    async def list_restrictions(
        self,
        limit: Optional[int] = None,
        after_id: Optional[int] = None,
        cursor: Optional[Union[int, str]] = None,
    ) -> Dict[str, Any]:
        return await self._rpc("listRestrictions", [_page_params(limit, after_id, cursor)])

    async def list_active_restrictions(
        self,
        limit: Optional[int] = None,
        after_id: Optional[int] = None,
        cursor: Optional[Union[int, str]] = None,
    ) -> Dict[str, Any]:
        return await self._rpc("listActiveRestrictions", [_page_params(limit, after_id, cursor)])

    async def get_restriction_status(self, target: RestrictionTargetInput) -> Dict[str, Any]:
        return await self._rpc("getRestrictionStatus", [_target_params(target)])

    async def get_account_restriction_status(self, account: AddressInput) -> Dict[str, Any]:
        return await self._rpc("getAccountRestrictionStatus", [_address(account)])

    async def get_asset_restriction_status(self, asset: RestrictionAssetInput) -> Dict[str, Any]:
        return await self._rpc("getAssetRestrictionStatus", [_asset(asset)])

    async def get_account_asset_restriction_status(
        self,
        account: AddressInput,
        asset: RestrictionAssetInput,
    ) -> Dict[str, Any]:
        return await self._rpc("getAccountAssetRestrictionStatus", [_address(account), _asset(asset)])

    async def get_contract_lifecycle_status(self, contract: AddressInput) -> Dict[str, Any]:
        return await self._rpc("getContractLifecycleStatus", [_address(contract)])

    async def get_code_hash_restriction_status(self, code_hash: str) -> Dict[str, Any]:
        return await self._rpc("getCodeHashRestrictionStatus", [code_hash])

    async def get_bridge_route_restriction_status(self, chain: str, asset: str) -> Dict[str, Any]:
        return await self._rpc("getBridgeRouteRestrictionStatus", [chain, asset])

    async def can_send(self, params: Union[MovementRestrictionParams, Mapping[str, Any]]) -> Dict[str, Any]:
        return await self._rpc(
            "canSend",
            [
                _omit_none(
                    {
                        "account": _address(_get(params, ("account", "from", "source"))),
                        "asset": _asset(_get(params, ("asset",))),
                        "amount": _optional_u64(_get(params, ("amount",), None), "amount"),
                    }
                )
            ],
        )

    async def can_receive(self, params: Union[MovementRestrictionParams, Mapping[str, Any]]) -> Dict[str, Any]:
        return await self._rpc(
            "canReceive",
            [
                _omit_none(
                    {
                        "account": _address(_get(params, ("account", "to", "recipient"))),
                        "asset": _asset(_get(params, ("asset",))),
                        "amount": _optional_u64(_get(params, ("amount",), None), "amount"),
                    }
                )
            ],
        )

    async def can_transfer(self, params: Union[TransferRestrictionParams, Mapping[str, Any]]) -> Dict[str, Any]:
        return await self._rpc(
            "canTransfer",
            [
                _omit_none(
                    {
                        "from": _address(_get(params, ("from_", "from", "source"))),
                        "to": _address(_get(params, ("to", "recipient"))),
                        "asset": _asset(_get(params, ("asset",))),
                        "amount": _optional_u64(_get(params, ("amount",), None), "amount"),
                    }
                )
            ],
        )

    async def build_restrict_account_tx(
        self,
        params: Union[RestrictAccountParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._rpc(
            "buildRestrictAccountTx",
            [
                _omit_none(
                    {
                        **_restrict_common(params),
                        "account": _address(_get(params, ("account", "address", "pubkey"))),
                        "mode": _get(params, ("mode",), None),
                    }
                )
            ],
        )

    async def build_unrestrict_account_tx(
        self,
        params: Union[UnrestrictAccountParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._rpc(
            "buildUnrestrictAccountTx",
            [
                _omit_none(
                    {
                        **_builder_base(params),
                        "account": _address(_get(params, ("account", "address", "pubkey"))),
                        "restriction_id": _restriction_id_param(params),
                        "lift_reason": _get(params, ("lift_reason", "liftReason")),
                    }
                )
            ],
        )

    async def build_restrict_account_asset_tx(
        self,
        params: Union[RestrictAccountAssetParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._rpc(
            "buildRestrictAccountAssetTx",
            [
                _omit_none(
                    {
                        **_restrict_common(params),
                        "account": _address(_get(params, ("account", "address", "pubkey"))),
                        "asset": _asset(_get(params, ("asset", "mint"))),
                        "mode": _get(params, ("mode",), None),
                    }
                )
            ],
        )

    async def build_unrestrict_account_asset_tx(
        self,
        params: Union[UnrestrictAccountAssetParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._rpc(
            "buildUnrestrictAccountAssetTx",
            [
                _omit_none(
                    {
                        **_builder_base(params),
                        "account": _address(_get(params, ("account", "address", "pubkey"))),
                        "asset": _asset(_get(params, ("asset", "mint"))),
                        "restriction_id": _restriction_id_param(params),
                        "lift_reason": _get(params, ("lift_reason", "liftReason")),
                    }
                )
            ],
        )

    async def build_set_frozen_asset_amount_tx(
        self,
        params: Union[SetFrozenAssetAmountParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._rpc(
            "buildSetFrozenAssetAmountTx",
            [
                _omit_none(
                    {
                        **_restrict_common(params),
                        "account": _address(_get(params, ("account", "address", "pubkey"))),
                        "asset": _asset(_get(params, ("asset", "mint"))),
                        "amount": _u64(_get(params, ("amount", "frozen_amount", "frozenAmount")), "amount"),
                    }
                )
            ],
        )

    async def build_suspend_contract_tx(
        self,
        params: Union[ContractRestrictionParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._contract_restrict_builder("buildSuspendContractTx", params)

    async def build_resume_contract_tx(
        self,
        params: Union[ResumeContractParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._rpc(
            "buildResumeContractTx",
            [
                _omit_none(
                    {
                        **_builder_base(params),
                        "contract": _address(_get(params, ("contract", "contract_id", "contractId", "account"))),
                        "restriction_id": _restriction_id_param(params),
                        "lift_reason": _get(params, ("lift_reason", "liftReason")),
                    }
                )
            ],
        )

    async def build_quarantine_contract_tx(
        self,
        params: Union[ContractRestrictionParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._contract_restrict_builder("buildQuarantineContractTx", params)

    async def build_terminate_contract_tx(
        self,
        params: Union[ContractRestrictionParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._contract_restrict_builder("buildTerminateContractTx", params)

    async def _contract_restrict_builder(self, method: str, params: Any) -> Dict[str, Any]:
        return await self._rpc(
            method,
            [
                _omit_none(
                    {
                        **_restrict_common(params),
                        "contract": _address(_get(params, ("contract", "contract_id", "contractId", "account"))),
                    }
                )
            ],
        )

    async def build_ban_code_hash_tx(
        self,
        params: Union[CodeHashRestrictionParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._rpc(
            "buildBanCodeHashTx",
            [
                _omit_none(
                    {
                        **_restrict_common(params),
                        "code_hash": _get(params, ("code_hash", "codeHash")),
                    }
                )
            ],
        )

    async def build_unban_code_hash_tx(
        self,
        params: Union[UnbanCodeHashParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._rpc(
            "buildUnbanCodeHashTx",
            [
                _omit_none(
                    {
                        **_builder_base(params),
                        "code_hash": _get(params, ("code_hash", "codeHash")),
                        "restriction_id": _restriction_id_param(params),
                        "lift_reason": _get(params, ("lift_reason", "liftReason")),
                    }
                )
            ],
        )

    async def build_pause_bridge_route_tx(
        self,
        params: Union[BridgeRouteRestrictionParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._rpc(
            "buildPauseBridgeRouteTx",
            [
                _omit_none(
                    {
                        **_restrict_common(params),
                        "chain": _get(params, ("chain", "chain_id", "chainId")),
                        "asset": _get(params, ("asset",)),
                    }
                )
            ],
        )

    async def build_resume_bridge_route_tx(
        self,
        params: Union[ResumeBridgeRouteParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._rpc(
            "buildResumeBridgeRouteTx",
            [
                _omit_none(
                    {
                        **_builder_base(params),
                        "chain": _get(params, ("chain", "chain_id", "chainId")),
                        "asset": _get(params, ("asset",)),
                        "restriction_id": _restriction_id_param(params),
                        "lift_reason": _get(params, ("lift_reason", "liftReason")),
                    }
                )
            ],
        )

    async def build_extend_restriction_tx(
        self,
        params: Union[ExtendRestrictionParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._rpc(
            "buildExtendRestrictionTx",
            [
                _omit_none(
                    {
                        **_builder_base(params),
                        "restriction_id": _u64(
                            _get(params, ("restriction_id", "restrictionId", "id")),
                            "restriction_id",
                        ),
                        "new_expires_at_slot": _optional_u64(
                            _get(
                                params,
                                (
                                    "new_expires_at_slot",
                                    "newExpiresAtSlot",
                                    "expires_at_slot",
                                    "expiresAtSlot",
                                ),
                                None,
                            ),
                            "new_expires_at_slot",
                        ),
                        "evidence_hash": _get(params, ("evidence_hash", "evidenceHash"), None),
                    }
                )
            ],
        )

    async def build_lift_restriction_tx(
        self,
        params: Union[LiftRestrictionParams, Mapping[str, Any]],
    ) -> Dict[str, Any]:
        return await self._rpc(
            "buildLiftRestrictionTx",
            [
                _omit_none(
                    {
                        **_builder_base(params),
                        "restriction_id": _u64(
                            _get(params, ("restriction_id", "restrictionId", "id")),
                            "restriction_id",
                        ),
                        "lift_reason": _get(params, ("lift_reason", "liftReason")),
                    }
                )
            ],
        )
