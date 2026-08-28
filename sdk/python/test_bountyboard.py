from __future__ import annotations

import base64

import pytest

from lichen.bountyboard import (
    BOUNTY_STATUS_CANCELLED,
    BOUNTY_STATUS_COMPLETED,
    BOUNTY_STATUS_OPEN,
    BountyBoardClient,
    _build_layout_args,
    _decode_bounty_info,
    _decode_accounting_health,
    _decode_accounting_migration_status,
    _decode_admin_transition,
    _decode_platform_stats,
    _encode_bounty_id_args,
    _encode_create_bounty_args,
    _encode_submit_work_args,
    _encode_approve_work_args,
    _encode_cancel_bounty_args,
)
from lichen import Keypair, PublicKey


class FakeConnection:
    def __init__(self) -> None:
        self.calls: list[tuple[str, str, str, bytes, int]] = []

    async def get_symbol_registry(self, symbol: str):
        if symbol == "BOUNTY":
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
        self.calls.append(
            (str(caller.pubkey()), str(contract), function_name, args, value)
        )
        return "test-signature"

    async def call_readonly_contract(
        self,
        contract: PublicKey,
        function_name: str,
        args: bytes = b"",
        from_pubkey: PublicKey | None = None,
    ):
        if function_name == "get_bounty":
            bounty_id = int.from_bytes(args[-8:], "little")
            if bounty_id == 999:
                return {"success": True, "returnCode": 1, "returnData": None}
            # Return a fake bounty (91 bytes)
            creator = bytes([1] * 32)
            title_hash = bytes([0xAA] * 32)
            reward = (5_000_000_000).to_bytes(8, "little")
            deadline = (1000).to_bytes(8, "little")
            status = bytes([BOUNTY_STATUS_OPEN])
            sub_count = bytes([2])
            created_slot = (500).to_bytes(8, "little")
            approved_idx = bytes([0xFF])
            payload = (
                creator
                + title_hash
                + reward
                + deadline
                + status
                + sub_count
                + created_slot
                + approved_idx
            )
            return {
                "success": True,
                "returnCode": 0,
                "returnData": base64.b64encode(payload).decode("ascii"),
            }
        if function_name == "get_bounty_count_exact":
            payload = (7).to_bytes(8, "little")
            return {
                "success": True,
                "returnCode": 0,
                "returnData": base64.b64encode(payload).decode("ascii"),
            }
        if function_name == "get_platform_stats":
            payload = (10).to_bytes(8, "little")  # bounty_count
            payload += (5).to_bytes(8, "little")  # completed_count
            payload += (50_000_000_000).to_bytes(8, "little")  # reward_volume
            payload += (2).to_bytes(8, "little")  # cancel_count
            return {
                "success": True,
                "returnCode": 0,
                "returnData": base64.b64encode(payload).decode("ascii"),
            }
        if function_name == "get_accounting_migration_status":
            payload = b"".join(
                value.to_bytes(8, "little") for value in (2, 1, 100, 0, 1)
            )
            return {
                "success": True,
                "returnCode": 0,
                "returnData": base64.b64encode(payload).decode("ascii"),
            }
        if function_name == "get_accounting_health":
            payload = b"".join(
                value.to_bytes(8, "little") for value in (2, 0, 100, 10, 110, 110, 1)
            )
            return {
                "success": True,
                "returnCode": 0,
                "returnData": base64.b64encode(payload).decode("ascii"),
            }
        if function_name == "get_admin_transition":
            payload = bytes([3] * 32) + bytes([4] * 32)
            return {
                "success": True,
                "returnCode": 0,
                "returnData": base64.b64encode(payload).decode("ascii"),
            }
        raise RuntimeError(f"unexpected readonly function: {function_name}")

    async def get_bountyboard_stats(self):
        return {
            "bounty_count": 10,
            "bounty_count_exact": "10",
            "completed_count": 5,
            "completed_count_exact": "5",
            "reward_volume": 50000000000,
            "reward_volume_raw_exact": "50000000000",
            "cancel_count": 2,
            "cancel_count_exact": "2",
            "paused": False,
        }


@pytest.fixture
def client() -> BountyBoardClient:
    conn = FakeConnection()
    return BountyBoardClient(conn)  # type: ignore[arg-type]


@pytest.mark.asyncio
async def test_get_program_id(client: BountyBoardClient) -> None:
    pid = await client.get_program_id()
    assert str(pid) == "11111111111111111111111111111112"


@pytest.mark.asyncio
async def test_get_bounty(client: BountyBoardClient) -> None:
    bounty = await client.get_bounty(0)
    assert bounty is not None
    assert bounty["creator"] == bytes([1] * 32)
    assert bounty["title_hash"] == bytes([0xAA] * 32)
    assert bounty["reward_amount"] == 5_000_000_000
    assert bounty["deadline_slot"] == 1000
    assert bounty["status"] == BOUNTY_STATUS_OPEN
    assert bounty["submission_count"] == 2
    assert bounty["created_slot"] == 500
    assert bounty["approved_idx"] == 0xFF


@pytest.mark.asyncio
async def test_get_bounty_not_found(client: BountyBoardClient) -> None:
    bounty = await client.get_bounty(999)
    assert bounty is None


@pytest.mark.asyncio
async def test_get_bounty_count(client: BountyBoardClient) -> None:
    count = await client.get_bounty_count()
    assert count == 7


@pytest.mark.asyncio
async def test_get_platform_stats(client: BountyBoardClient) -> None:
    stats = await client.get_platform_stats()
    assert stats["bounty_count"] == 10
    assert stats["completed_count"] == 5
    assert stats["reward_volume"] == 50_000_000_000
    assert stats["cancel_count"] == 2


@pytest.mark.asyncio
async def test_get_stats(client: BountyBoardClient) -> None:
    stats = await client.get_stats()
    assert stats["bounty_count"] == 10
    assert stats["completed_count"] == 5
    assert stats["total_reward_volume"] == 50000000000
    assert stats["cancel_count"] == 2
    assert stats["paused"] is False


@pytest.mark.asyncio
async def test_create_bounty(client: BountyBoardClient) -> None:
    kp = Keypair.from_seed(bytes(range(32)))
    title_hash = bytes([0xBB] * 32)
    sig = await client.create_bounty(kp, title_hash, 1_000_000_000, 2000)
    assert sig == "test-signature"
    conn = client.connection
    assert len(conn.calls) == 1  # type: ignore[attr-defined]
    call = conn.calls[0]  # type: ignore[attr-defined]
    assert call[2] == "create_bounty"
    assert call[4] == 1_000_000_000  # value attached for escrow


@pytest.mark.asyncio
async def test_submit_work(client: BountyBoardClient) -> None:
    kp = Keypair.from_seed(bytes(range(32)))
    proof_hash = bytes([0xCC] * 32)
    sig = await client.submit_work(kp, 0, proof_hash)
    assert sig == "test-signature"
    conn = client.connection
    assert len(conn.calls) == 1  # type: ignore[attr-defined]
    call = conn.calls[0]  # type: ignore[attr-defined]
    assert call[2] == "submit_work"


@pytest.mark.asyncio
async def test_approve_work(client: BountyBoardClient) -> None:
    kp = Keypair.from_seed(bytes(range(32)))
    sig = await client.approve_work(kp, 0, 1)
    assert sig == "test-signature"
    conn = client.connection
    assert len(conn.calls) == 1  # type: ignore[attr-defined]
    call = conn.calls[0]  # type: ignore[attr-defined]
    assert call[2] == "approve_work"


@pytest.mark.asyncio
async def test_cancel_bounty(client: BountyBoardClient) -> None:
    kp = Keypair.from_seed(bytes(range(32)))
    sig = await client.cancel_bounty(kp, 0)
    assert sig == "test-signature"
    conn = client.connection
    assert len(conn.calls) == 1  # type: ignore[attr-defined]
    call = conn.calls[0]  # type: ignore[attr-defined]
    assert call[2] == "cancel_bounty"


def test_create_bounty_encoding() -> None:
    creator = PublicKey(bytes([7] * 32))
    title_hash = bytes([0xAA] * 32)
    encoded = _encode_create_bounty_args(creator, title_hash, 1_000, 2_000)
    # Header: 0xAB 0x20 0x20 0x08 0x08
    assert encoded[:5] == bytes([0xAB, 0x20, 0x20, 0x08, 0x08])
    assert encoded[5:37] == bytes([7] * 32)
    assert encoded[37:69] == bytes([0xAA] * 32)
    assert int.from_bytes(encoded[69:77], "little") == 1_000
    assert int.from_bytes(encoded[77:85], "little") == 2_000


def test_submit_work_encoding() -> None:
    worker = PublicKey(bytes([8] * 32))
    proof_hash = bytes([0xBB] * 32)
    encoded = _encode_submit_work_args(42, worker, proof_hash)
    # Header: 0xAB 0x08 0x20 0x20
    assert encoded[:4] == bytes([0xAB, 0x08, 0x20, 0x20])
    assert int.from_bytes(encoded[4:12], "little") == 42
    assert encoded[12:44] == bytes([8] * 32)
    assert encoded[44:76] == bytes([0xBB] * 32)


def test_approve_work_encoding() -> None:
    caller = PublicKey(bytes([9] * 32))
    encoded = _encode_approve_work_args(caller, 5, 2)
    # Header: 0xAB 0x20 0x08 0x01
    assert encoded[:4] == bytes([0xAB, 0x20, 0x08, 0x01])
    assert encoded[4:36] == bytes([9] * 32)
    assert int.from_bytes(encoded[36:44], "little") == 5
    assert encoded[44] == 2


def test_cancel_bounty_encoding() -> None:
    caller = PublicKey(bytes([10] * 32))
    encoded = _encode_cancel_bounty_args(caller, 3)
    # Header: 0xAB 0x20 0x08
    assert encoded[:3] == bytes([0xAB, 0x20, 0x08])
    assert encoded[3:35] == bytes([10] * 32)
    assert int.from_bytes(encoded[35:43], "little") == 3


def test_bounty_info_decoding() -> None:
    creator = bytes([1] * 32)
    title_hash = bytes([0xAA] * 32)
    reward = (5_000_000_000).to_bytes(8, "little")
    deadline = (1000).to_bytes(8, "little")
    status = bytes([BOUNTY_STATUS_COMPLETED])
    sub_count = bytes([3])
    created_slot = (500).to_bytes(8, "little")
    approved_idx = bytes([1])
    payload = (
        creator
        + title_hash
        + reward
        + deadline
        + status
        + sub_count
        + created_slot
        + approved_idx
    )
    result = {
        "success": True,
        "returnCode": 0,
        "returnData": base64.b64encode(payload).decode("ascii"),
    }
    bounty = _decode_bounty_info(result)
    assert bounty["reward_amount"] == 5_000_000_000
    assert bounty["status"] == BOUNTY_STATUS_COMPLETED
    assert bounty["submission_count"] == 3
    assert bounty["approved_idx"] == 1


def test_platform_stats_decoding() -> None:
    payload = (10).to_bytes(8, "little")
    payload += (5).to_bytes(8, "little")
    payload += (50_000_000_000).to_bytes(8, "little")
    payload += (2).to_bytes(8, "little")
    result = {
        "success": True,
        "returnCode": 0,
        "returnData": base64.b64encode(payload).decode("ascii"),
    }
    stats = _decode_platform_stats(result)
    assert stats["bounty_count"] == 10
    assert stats["completed_count"] == 5
    assert stats["reward_volume"] == 50_000_000_000
    assert stats["cancel_count"] == 2


@pytest.mark.asyncio
async def test_accounting_reads_and_migration_writes(client: BountyBoardClient) -> None:
    migration = await client.get_accounting_migration_status()
    health = await client.get_accounting_health()
    admin_transition = await client.get_admin_transition()
    assert migration["cursor"] == 1
    assert migration["locked"] is True
    assert health["total_liability"] == 110
    assert health["solvent"] is True
    assert admin_transition["current_admin"] == bytes([3] * 32)
    assert admin_transition["pending_admin"] == bytes([4] * 32)

    kp = Keypair.from_seed(bytes(range(32)))
    await client.begin_accounting_v2_migration(kp, 2)
    await client.migrate_accounting_v2_bounty(kp, 1)
    await client.complete_accounting_v2_migration(kp, 100, 10, 110)
    await client.propose_admin(kp, PublicKey(bytes([4] * 32)))
    await client.accept_admin(kp)
    await client.cancel_admin_proposal(kp)
    calls = client.connection.calls  # type: ignore[attr-defined]
    assert [call[2] for call in calls] == [
        "begin_accounting_v2_migration",
        "migrate_accounting_v2_bounty",
        "complete_accounting_v2_migration",
        "propose_admin",
        "accept_admin",
        "cancel_admin_proposal",
    ]
    assert calls[0][3][:3] == bytes([0xAB, 0x20, 0x08])
    assert calls[1][3][:2] == bytes([0xAB, 0x08])
    assert calls[2][3][:5] == bytes([0xAB, 0x20, 0x08, 0x08, 0x08])


@pytest.mark.asyncio
async def test_token_payment_can_attach_zero_native_value(
    client: BountyBoardClient,
) -> None:
    kp = Keypair.from_seed(bytes(range(32)))
    await client.create_bounty(kp, bytes([1] * 32), 100, 200, payment_value=0)
    assert client.connection.calls[0][4] == 0  # type: ignore[attr-defined]


def test_accounting_decoders_fail_closed_on_malformed_layouts() -> None:
    malformed_flag = b"".join(
        value.to_bytes(8, "little") for value in (2, 1, 100, 0, 2)
    )
    result = {
        "success": True,
        "returnCode": 0,
        "returnData": base64.b64encode(malformed_flag).decode("ascii"),
    }
    with pytest.raises(RuntimeError, match="0 or 1"):
        _decode_accounting_migration_status(result)

    trailing_health = bytes(57)
    result["returnData"] = base64.b64encode(trailing_health).decode("ascii")
    with pytest.raises(RuntimeError, match="exactly 56 bytes"):
        _decode_accounting_health(result)

    result["returnData"] = base64.b64encode(bytes(65)).decode("ascii")
    with pytest.raises(RuntimeError, match="exactly 64 bytes"):
        _decode_admin_transition(result)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("method", "args"),
    [
        ("get_bounty", (1,)),
        ("get_submission", (1, 0)),
        ("get_bounty_terms", (1,)),
    ],
)
async def test_optional_reads_do_not_hide_contract_errors_as_missing(
    method: str, args: tuple[int, ...]
) -> None:
    class MalformedReadConnection(FakeConnection):
        async def call_readonly_contract(self, *unused_args, **unused_kwargs):
            return {"success": False, "returnCode": 2, "returnData": None}

    malformed_client = BountyBoardClient(MalformedReadConnection())  # type: ignore[arg-type]
    with pytest.raises(RuntimeError, match="returned code 2"):
        await getattr(malformed_client, method)(*args)
