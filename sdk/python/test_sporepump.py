from __future__ import annotations

import base64

import pytest

from lichen import Keypair, PublicKey
from lichen.sporepump import (
    CreateSporePumpTokenParams,
    SPOREPUMP_CREATION_FEE,
    SporePumpClient,
)


def encoded(payload: bytes, code: int = 0):
    return {
        "success": True,
        "returnCode": code,
        "returnData": base64.b64encode(payload).decode("ascii"),
    }


class FakeConnection:
    def __init__(self) -> None:
        self.calls: list[tuple[str, bytes, int]] = []

    async def get_symbol_registry(self, symbol: str):
        if symbol == "SPOREPUMP":
            return {"program": "11111111111111111111111111111112"}
        raise RuntimeError("missing symbol")

    async def call_contract(
        self,
        _caller: Keypair,
        _contract: PublicKey,
        function_name: str,
        args: bytes = b"",
        value: int = 0,
    ) -> str:
        self.calls.append((function_name, args, value))
        return "test-signature"

    async def call_readonly_contract(
        self,
        _contract: PublicKey,
        function_name: str,
        _args: bytes = b"",
        _from_pubkey: PublicKey | None = None,
    ):
        if function_name == "get_token_info":
            return encoded(b"".join(value.to_bytes(8, "little") for value in (11, 22, 33, 44)) + b"\0")
        if function_name == "get_token_metadata":
            return encoded((4).to_bytes(2, "little") + b"Moss" + (4).to_bytes(2, "little") + b"MOSS")
        if function_name in {"get_buy_quote", "get_sell_quote", "get_token_count", "get_creator_royalty_balance"}:
            return encoded((123).to_bytes(8, "little"))
        if function_name == "get_platform_stats":
            fields = [*range(1, 10), 1, 11]
            return encoded(b"".join(value.to_bytes(8, "little") for value in fields))
        if function_name == "get_custody_status":
            return encoded(b"".join(value.to_bytes(8, "little") for value in (100, 90, 10)))
        if function_name == "get_accounting_migration_token":
            payload = bytearray(73)
            payload[0:32] = bytes([7]) * 32
            for offset, value in ((32, 12), (40, 13), (48, 14), (56, 15), (65, 16)):
                payload[offset : offset + 8] = value.to_bytes(8, "little")
            payload[64] = 1
            return encoded(bytes(payload))
        if function_name == "get_graduation_status":
            payload = bytearray(113)
            payload[0] = 3
            payload[17:49] = bytes([8]) * 32
            for offset, value in ((1, 1), (9, 2), (49, 3), (57, 4), (65, 5), (73, 6), (81, 7), (89, 8), (97, 9), (105, 10)):
                payload[offset : offset + 8] = value.to_bytes(8, "little")
            return encoded(bytes(payload))
        if function_name == "get_graduation_info":
            payload = bytearray(46)
            payload[0:8] = (9).to_bytes(8, "little")
            payload[8:14] = b"\1\1\1\1\1\1"
            for offset, value in ((14, 10), (22, 20), (30, 30), (38, 2)):
                payload[offset : offset + 8] = value.to_bytes(8, "little")
            return encoded(bytes(payload))
        raise RuntimeError(f"unexpected readonly function: {function_name}")


@pytest.mark.asyncio
async def test_sporepump_exact_reads_decode_all_accounting_and_graduation_fields() -> None:
    client = SporePumpClient(FakeConnection())

    assert (await client.get_token_info(1))["market_cap"] == 44
    assert await client.get_token_metadata(1) == {"name": "Moss", "symbol": "MOSS"}
    assert await client.get_buy_quote(1, 100) == 123
    assert await client.get_sell_quote(1, 100) == 123
    assert (await client.get_platform_stats())["creator_royalty_bps"] == 11
    assert (await client.get_custody_status())["recoverable_surplus"] == 10
    migration_token = await client.get_accounting_migration_token(1)
    assert migration_token["creator_royalty"] == 16
    assert migration_token["lifecycle_state"] == 1
    status = await client.get_graduation_status(1)
    assert status["reverse_route_id"] == 6
    assert status["protocol_token_inventory"] == 10
    info = await client.get_graduation_info()
    assert info["accounting_ready"] is True
    assert info["minimum_order"] == 30


@pytest.mark.asyncio
async def test_sporepump_writes_use_slippage_variants_and_exact_native_value() -> None:
    connection = FakeConnection()
    client = SporePumpClient(connection)
    signer = Keypair.from_seed(bytes(range(32)))

    await client.create_token(signer, CreateSporePumpTokenParams(name="Moss Token", symbol="moss"))
    await client.buy(signer, 7, 1_000_000_000, 99)
    await client.sell(signer, 7, 100, 88)
    await client.claim_creator_royalty(signer, 7, 55)

    assert [call[0] for call in connection.calls] == [
        "create_token_with_metadata",
        "buy_with_min_output",
        "sell_with_min_output",
        "claim_creator_royalty",
    ]
    assert connection.calls[0][2] == SPOREPUMP_CREATION_FEE
    assert connection.calls[0][1][:7] == bytes([0xAB, 32, 32, 4, 32, 4, 8])
    assert connection.calls[1][2] == 1_000_000_000
    assert connection.calls[1][1][:5] == bytes([0xAB, 32, 8, 8, 8])
    assert connection.calls[2][2] == 0
