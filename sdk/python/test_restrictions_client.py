from __future__ import annotations

import pytest
from typing import Optional

from lichen import (
    AccountAssetRestrictionTarget,
    Keypair,
    RestrictAccountParams,
    RestrictionGovernanceClient,
    ResumeBridgeRouteParams,
    SetFrozenAssetAmountParams,
    TransferRestrictionParams,
)


class FakeConnection:
    def __init__(self) -> None:
        self.calls: list[tuple[str, list[object]]] = []

    async def rpc_request(self, method: str, params: Optional[list[object]] = None):
        payload = params or []
        self.calls.append((method, payload))
        return {
            "method": method,
            "params": payload,
            "unsigned": method.startswith("build"),
        }


@pytest.mark.asyncio
async def test_restriction_client_read_and_preflight_payloads() -> None:
    connection = FakeConnection()
    client = RestrictionGovernanceClient(connection)
    account = Keypair.from_seed(bytes(range(32))).pubkey()
    recipient = Keypair.from_seed(bytes(range(1, 33))).pubkey()
    asset = Keypair.from_seed(bytes(range(2, 34))).pubkey()

    await client.get_account_restriction_status(account)
    await client.get_restriction_status(AccountAssetRestrictionTarget(account=account, asset=asset))
    await client.list_restrictions(limit=10, after_id=2)
    await client.can_transfer(
        TransferRestrictionParams(
            from_=account,
            to=recipient,
            asset="native",
            amount=123,
        )
    )

    assert connection.calls == [
        ("getAccountRestrictionStatus", [str(account)]),
        (
            "getRestrictionStatus",
            [
                {
                    "type": "account_asset",
                    "account": str(account),
                    "asset": str(asset),
                }
            ],
        ),
        ("listRestrictions", [{"limit": 10, "after_id": 2}]),
        (
            "canTransfer",
            [
                {
                    "from": str(account),
                    "to": str(recipient),
                    "asset": "native",
                    "amount": 123,
                }
            ],
        ),
    ]


@pytest.mark.asyncio
async def test_restriction_client_builder_payloads() -> None:
    connection = FakeConnection()
    client = RestrictionGovernanceClient(connection)
    proposer = Keypair.from_seed(bytes(range(10, 42))).pubkey()
    authority = Keypair.from_seed(bytes(range(11, 43))).pubkey()
    account = Keypair.from_seed(bytes(range(12, 44))).pubkey()
    asset = Keypair.from_seed(bytes(range(13, 45))).pubkey()

    restrict = await client.build_restrict_account_tx(
        RestrictAccountParams(
            proposer=proposer,
            governance_authority=authority,
            account=account,
            mode="outgoing_only",
            reason="testnet_drill",
            recent_blockhash="abc123",
            expires_at_slot=500,
        )
    )
    frozen = await client.build_set_frozen_asset_amount_tx(
        SetFrozenAssetAmountParams(
            proposer=proposer,
            governance_authority=authority,
            account=account,
            asset=asset,
            amount=100,
            reason="stolen_funds",
        )
    )
    resumed = await client.build_resume_bridge_route_tx(
        ResumeBridgeRouteParams(
            proposer=proposer,
            governance_authority=authority,
            chain="ethereum",
            asset="eth",
            restriction_id=5,
            lift_reason="testnet_drill_complete",
        )
    )

    assert restrict["unsigned"] is True
    assert frozen["unsigned"] is True
    assert resumed["unsigned"] is True
    assert connection.calls == [
        (
            "buildRestrictAccountTx",
            [
                {
                    "proposer": str(proposer),
                    "governance_authority": str(authority),
                    "recent_blockhash": "abc123",
                    "reason": "testnet_drill",
                    "expires_at_slot": 500,
                    "account": str(account),
                    "mode": "outgoing_only",
                }
            ],
        ),
        (
            "buildSetFrozenAssetAmountTx",
            [
                {
                    "proposer": str(proposer),
                    "governance_authority": str(authority),
                    "reason": "stolen_funds",
                    "account": str(account),
                    "asset": str(asset),
                    "amount": 100,
                }
            ],
        ),
        (
            "buildResumeBridgeRouteTx",
            [
                {
                    "proposer": str(proposer),
                    "governance_authority": str(authority),
                    "chain": "ethereum",
                    "asset": "eth",
                    "restriction_id": 5,
                    "lift_reason": "testnet_drill_complete",
                }
            ],
        ),
    ]


@pytest.mark.asyncio
async def test_restriction_client_rejects_invalid_u64_values() -> None:
    connection = FakeConnection()
    client = RestrictionGovernanceClient(connection)
    account = Keypair.from_seed(bytes(range(20, 52))).pubkey()

    with pytest.raises(ValueError, match="restriction_id must be a u64-safe integer value"):
        await client.get_restriction(-1)

    with pytest.raises(ValueError, match="amount must be a u64-safe integer value"):
        await client.can_transfer(
            {
                "from": account,
                "to": account,
                "asset": "native",
                "amount": True,
            }
        )

    assert connection.calls == []
