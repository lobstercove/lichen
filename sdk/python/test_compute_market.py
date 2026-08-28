from __future__ import annotations

import base64

import pytest

from lichen import ComputeMarketClient, Keypair, PublicKey


class FakeConnection:
    def __init__(self, *, append_trailing_byte: bool = False) -> None:
        self.calls: list[tuple[str, str, str, bytes, int]] = []
        self.append_trailing_byte = append_trailing_byte

    async def get_symbol_registry(self, symbol: str):
        if symbol == "COMPUTE":
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
        if function_name == "get_accounting_migration_status":
            values = (2, 1, 100, 20, 0, 1)
        elif function_name == "get_accounting_health":
            values = (3, 0, 100, 20, 10, 130, 150, 1)
        else:
            raise RuntimeError(f"unexpected readonly function: {function_name}")
        payload = b"".join(value.to_bytes(8, "little") for value in values)
        if self.append_trailing_byte:
            payload += b"\x00"
        return {
            "success": True,
            "returnCode": 0,
            "returnData": base64.b64encode(payload).decode("ascii"),
        }


@pytest.mark.asyncio
async def test_compute_market_accounting_v3_reads_and_writes() -> None:
    connection = FakeConnection()
    client = ComputeMarketClient(connection)
    admin = Keypair.from_seed(bytes(range(32)))

    migration = await client.get_accounting_migration_status()
    health = await client.get_accounting_health()
    await client.begin_accounting_v3_migration(admin, 2)
    await client.migrate_accounting_v3_job(admin, 1)
    await client.complete_accounting_v3_migration(admin, 100, 20, 10, 130)

    assert migration == {
        "expected_job_count": 2,
        "cursor": 1,
        "reconstructed_escrow": 100,
        "reconstructed_unpaid": 20,
        "accounting_version": 0,
        "locked": True,
    }
    assert health == {
        "accounting_version": 3,
        "migration_locked": False,
        "escrow_liability": 100,
        "unpaid_liability": 20,
        "platform_fees": 10,
        "total_liability": 130,
        "custody_balance": 150,
        "solvent": True,
    }
    assert [call[2] for call in connection.calls] == [
        "begin_accounting_v3_migration",
        "migrate_accounting_v3_job",
        "complete_accounting_v3_migration",
    ]
    assert connection.calls[0][3][:3] == bytes([0xAB, 0x20, 0x08])
    assert connection.calls[1][3][:2] == bytes([0xAB, 0x08])
    assert connection.calls[2][3][:6] == bytes([0xAB, 0x20, 0x08, 0x08, 0x08, 0x08])


@pytest.mark.asyncio
async def test_compute_market_rejects_noncanonical_read_payloads() -> None:
    client = ComputeMarketClient(FakeConnection(append_trailing_byte=True))
    with pytest.raises(RuntimeError, match="exactly 48 bytes"):
        await client.get_accounting_migration_status()
    with pytest.raises(RuntimeError, match="exactly 64 bytes"):
        await client.get_accounting_health()


@pytest.mark.asyncio
async def test_compute_market_rejects_zero_hash_before_submission() -> None:
    connection = FakeConnection()
    client = ComputeMarketClient(connection)
    requester = Keypair.from_seed(bytes(range(32)))
    with pytest.raises(ValueError, match="zero hash"):
        await client.submit_job(requester, 10, 100, bytes(32))
    assert connection.calls == []
