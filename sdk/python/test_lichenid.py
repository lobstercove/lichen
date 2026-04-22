from __future__ import annotations

import base64
import pytest

from lichen import Keypair, PublicKey
from lichen.lichenid import (
    AddSkillParams,
    ApproveRecoveryParams,
    AttestSkillParams,
    BidNameAuctionParams,
    CreateNameAuctionParams,
    ExecuteRecoveryParams,
    FinalizeNameAuctionParams,
    LichenIdClient,
    LICHEN_ID_DELEGATE_PERMISSIONS,
    RegisterIdentityParams,
    RegisterNameParams,
    RevokeAttestationParams,
    SetAvailabilityParams,
    SetAvailabilityAsParams,
    SetDelegateParams,
    SetEndpointParams,
    SetEndpointAsParams,
    SetMetadataAsParams,
    SetMetadataParams,
    SetRateParams,
    SetRateAsParams,
    SetRecoveryGuardiansParams,
    UpdateAgentTypeAsParams,
    estimate_lichenid_name_registration_cost,
)


class FakeConnection:
    def __init__(self) -> None:
        self.calls: list[tuple[str, str, str, bytes, int]] = []

    async def get_symbol_registry(self, symbol: str):
        if symbol == "YID":
            return {"program": "11111111111111111111111111111112"}
        raise RuntimeError("missing symbol")

    async def call_contract(
        self,
        caller: Keypair,
        contract: PublicKey,
        function_name: str,
        args: bytes = b"",
        value: int = 0,
    ) -> str:
        self.calls.append((str(caller.pubkey()), str(contract), function_name, args, value))
        return "test-signature"

    async def call_readonly_contract(
        self,
        contract: PublicKey,
        function_name: str,
        args: bytes = b"",
        from_pubkey: PublicKey | None = None,
    ):
        if function_name == "get_delegate":
            data = bytes([LICHEN_ID_DELEGATE_PERMISSIONS["PROFILE"] | LICHEN_ID_DELEGATE_PERMISSIONS["SKILLS"]])
            data += (1_700_000_000_000).to_bytes(8, "little")
            data += (1_699_000_000_000).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(data).decode("ascii")}
        if function_name == "get_attestations":
            data = (7).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(data).decode("ascii")}
        raise RuntimeError(f"unexpected readonly function: {function_name}")

    async def get_lichenid_profile(self, pubkey: PublicKey):
        return {
            "identity": {"name": "Research Agent"},
            "reputation": {"score": 900, "tier_name": "Trusted"},
            "agent": {"endpoint": "https://agent.example", "metadata": {"role": "maker"}},
            "address": str(pubkey),
        }

    async def get_lichenid_reputation(self, pubkey: PublicKey):
        return {"address": str(pubkey), "score": 900, "tier": 2, "tier_name": "Trusted"}

    async def get_lichenid_skills(self, pubkey: PublicKey):
        return [{"name": "market-making", "proficiency": 90, "timestamp": 1}]

    async def get_lichenid_vouches(self, pubkey: PublicKey):
        return {"received": [{"voucher": str(pubkey), "timestamp": 1}], "given": []}

    async def resolve_lichen_name(self, name: str):
        return {"name": name, "owner": "11111111111111111111111111111112"}

    async def get_name_auction(self, name: str):
        return {"name": name, "active": True, "highest_bid": 100_000_000_000}

    async def get_lichenid_agent_directory(self, options=None):
        return {"agents": [], "count": 0, "total": 0}

    async def get_lichenid_stats(self):
        return {"total_identities": 1, "total_names": 1, "total_skills": 0, "total_vouches": 0, "total_attestations": 0, "tier_distribution": {}}


@pytest.mark.asyncio
async def test_register_identity_resolves_program_and_encodes_layout_descriptor() -> None:
    connection = FakeConnection()
    client = LichenIdClient(connection)
    keypair = Keypair.from_seed(bytes(range(32)))

    signature = await client.register_identity(
        keypair,
        RegisterIdentityParams(agent_type=3, name="Research Agent"),
    )

    assert signature == "test-signature"
    assert len(connection.calls) == 1
    _, contract, function_name, args, value = connection.calls[0]
    assert contract == "11111111111111111111111111111112"
    assert function_name == "register_identity"
    assert args[:5] == bytes([0xAB, 0x20, 0x01, 0x40, 0x04])
    assert value == 0


@pytest.mark.asyncio
async def test_register_name_uses_default_cost() -> None:
    connection = FakeConnection()
    client = LichenIdClient(connection)
    keypair = Keypair.from_seed(bytes(range(32)))

    await client.register_name(
        keypair,
        RegisterNameParams(name="tradingbot", duration_years=2),
    )

    _, _, function_name, args, value = connection.calls[0]
    assert function_name == "register_name"
    assert args[:5] == bytes([0xAB, 0x20, 0x20, 0x04, 0x01])
    assert value == 40_000_000_000


def test_estimate_name_registration_cost_matches_pricing_table() -> None:
    assert estimate_lichenid_name_registration_cost("ai", 1) == 500_000_000_000
    assert estimate_lichenid_name_registration_cost("defi", 1) == 100_000_000_000
    assert estimate_lichenid_name_registration_cost("tradingbot", 2) == 40_000_000_000


@pytest.mark.asyncio
async def test_register_name_rejects_auction_only_label() -> None:
    connection = FakeConnection()
    client = LichenIdClient(connection)
    keypair = Keypair.from_seed(bytes(range(32)))

    with pytest.raises(ValueError, match="auction-only"):
        await client.register_name(keypair, RegisterNameParams(name="defi"))


@pytest.mark.asyncio
async def test_skill_vouch_agent_updates_and_auction_helpers_use_expected_calls() -> None:
    connection = FakeConnection()
    client = LichenIdClient(connection)
    keypair = Keypair.from_seed(bytes(range(32)))
    other = Keypair.from_seed(bytes(range(1, 33)))

    await client.add_skill(keypair, AddSkillParams(name="market-making", proficiency=92))
    await client.vouch(keypair, other.pubkey())
    await client.set_endpoint(keypair, SetEndpointParams(url="https://agent.example"))
    await client.set_rate(keypair, SetRateParams(rate_spores=25_000_000_000))
    await client.set_availability(keypair, SetAvailabilityParams(status="available"))
    await client.create_name_auction(
        keypair,
        CreateNameAuctionParams(
            name="defi",
            reserve_bid_spores=100_000_000_000,
            end_slot=432_000,
        ),
    )
    await client.bid_name_auction(
        keypair,
        BidNameAuctionParams(name="defi", bid_amount_spores=125_000_000_000),
    )
    await client.finalize_name_auction(
        keypair,
        FinalizeNameAuctionParams(name="defi", duration_years=1),
    )

    called_functions = [call[2] for call in connection.calls]
    assert called_functions == [
        "add_skill",
        "vouch",
        "set_endpoint",
        "set_rate",
        "set_availability",
        "create_name_auction",
        "bid_name_auction",
        "finalize_name_auction",
    ]
    assert connection.calls[3][3][:3] == bytes([0xAB, 0x20, 0x08])
    assert connection.calls[3][3][3:35] == keypair.pubkey().to_bytes()
    assert connection.calls[5][3][:6] == bytes([0xAB, 0x20, 0x20, 0x04, 0x08, 0x08])
    assert connection.calls[6][4] == 125_000_000_000


@pytest.mark.asyncio
async def test_helper_reads_cover_skills_vouches_and_auctions() -> None:
    connection = FakeConnection()
    client = LichenIdClient(connection)
    keypair = Keypair.from_seed(bytes(range(32)))

    skills = await client.get_skills(keypair.pubkey())
    vouches = await client.get_vouches(keypair.pubkey())
    auction = await client.get_name_auction("defi.lichen")

    assert skills[0]["name"] == "market-making"
    assert vouches["received"]
    assert auction["active"] is True


@pytest.mark.asyncio
async def test_metadata_delegate_recovery_and_attestation_helpers_use_expected_calls() -> None:
    connection = FakeConnection()
    client = LichenIdClient(connection)
    owner = Keypair.from_seed(bytes(range(32)))
    delegate = Keypair.from_seed(bytes(range(1, 33)))
    guardian = Keypair.from_seed(bytes(range(2, 34)))
    guardian_b = Keypair.from_seed(bytes(range(3, 35)))
    guardian_c = Keypair.from_seed(bytes(range(4, 36)))
    guardian_d = Keypair.from_seed(bytes(range(5, 37)))
    guardian_e = Keypair.from_seed(bytes(range(6, 38)))
    new_owner = Keypair.from_seed(bytes(range(7, 39)))

    await client.set_metadata(owner, SetMetadataParams(metadata={"role": "maker"}))
    await client.set_delegate(
        owner,
        SetDelegateParams(
            delegate=delegate.pubkey(),
            permissions=LICHEN_ID_DELEGATE_PERMISSIONS["PROFILE"],
            expires_at_ms=1_700_000_000_000,
        ),
    )
    await client.revoke_delegate(owner, delegate.pubkey())
    await client.set_endpoint_as(delegate, SetEndpointAsParams(owner=owner.pubkey(), url="https://delegate.example"))
    await client.set_metadata_as(delegate, SetMetadataAsParams(owner=owner.pubkey(), metadata={"delegated": True}))
    await client.set_availability_as(delegate, SetAvailabilityAsParams(owner=owner.pubkey(), status="busy"))
    await client.set_rate_as(delegate, SetRateAsParams(owner=owner.pubkey(), rate_spores=11_000_000_000))
    await client.update_agent_type_as(delegate, UpdateAgentTypeAsParams(owner=owner.pubkey(), agent_type=6))
    await client.set_recovery_guardians(
        owner,
        SetRecoveryGuardiansParams(
            guardians=[guardian.pubkey(), guardian_b.pubkey(), guardian_c.pubkey(), guardian_d.pubkey(), guardian_e.pubkey()]
        ),
    )
    await client.approve_recovery(
        guardian,
        ApproveRecoveryParams(target=owner.pubkey(), new_owner=new_owner.pubkey()),
    )
    await client.execute_recovery(
        guardian,
        ExecuteRecoveryParams(target=owner.pubkey(), new_owner=new_owner.pubkey()),
    )
    await client.attest_skill(
        delegate,
        AttestSkillParams(identity=owner.pubkey(), name="market-making", level=5),
    )
    await client.revoke_attestation(
        delegate,
        RevokeAttestationParams(identity=owner.pubkey(), name="market-making"),
    )

    called_functions = [call[2] for call in connection.calls]
    assert called_functions == [
        "set_metadata",
        "set_delegate",
        "revoke_delegate",
        "set_endpoint_as",
        "set_metadata_as",
        "set_availability_as",
        "set_rate_as",
        "update_agent_type_as",
        "set_recovery_guardians",
        "approve_recovery",
        "execute_recovery",
        "attest_skill",
        "revoke_attestation",
    ]
    assert connection.calls[1][3][:5] == bytes([0xAB, 0x20, 0x20, 0x01, 0x08])


@pytest.mark.asyncio
async def test_readonly_delegate_metadata_and_attestation_helpers() -> None:
    connection = FakeConnection()
    client = LichenIdClient(connection)
    owner = Keypair.from_seed(bytes(range(32)))
    delegate = Keypair.from_seed(bytes(range(1, 33)))

    metadata = await client.get_metadata(owner.pubkey())
    delegation = await client.get_delegate(owner.pubkey(), delegate.pubkey())
    attestations = await client.get_attestations(owner.pubkey(), "market-making")

    assert metadata == {"role": "maker"}
    assert delegation is not None
    assert delegation["can_profile"] is True
    assert delegation["can_skills"] is True
    assert attestations == 7