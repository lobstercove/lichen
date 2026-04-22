from __future__ import annotations

import base64

import pytest

from lichen import Keypair, PublicKey
from lichen.lichenswap import (
    AddLiquidityParams,
    CreatePoolParams,
    LichenSwapClient,
    SwapParams,
    SwapWithDeadlineParams,
)


class FakeConnection:
    def __init__(self) -> None:
        self.calls: list[tuple[str, str, str, bytes, int]] = []

    async def get_symbol_registry(self, symbol: str):
        if symbol == "LICHENSWAP":
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
        if function_name == "get_pool_info":
            payload = (1_000).to_bytes(8, "little")
            payload += (2_000).to_bytes(8, "little")
            payload += (3_000).to_bytes(8, "little")
            return {"success": True, "returnCode": 1, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_quote":
            payload = (777).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_liquidity_balance":
            payload = (333).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_total_liquidity":
            payload = (3_000).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_flash_loan_fee":
            payload = (5).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_twap_cumulatives":
            payload = (11).to_bytes(8, "little")
            payload += (22).to_bytes(8, "little")
            payload += (33).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_twap_snapshot_count":
            payload = (4).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_protocol_fees":
            payload = (12).to_bytes(8, "little")
            payload += (34).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_pool_count":
            payload = (2).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_swap_count":
            payload = (9).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_total_volume":
            payload = (100).to_bytes(8, "little")
            payload += (200).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_swap_stats":
            payload = (9).to_bytes(8, "little")
            payload += (100).to_bytes(8, "little")
            payload += (200).to_bytes(8, "little")
            payload += (2).to_bytes(8, "little")
            payload += (3_000).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        raise RuntimeError(f"unexpected readonly function: {function_name}")

    async def get_lichenswap_stats(self):
        return {
            "swap_count": 9,
            "volume_a": 100,
            "volume_b": 200,
            "paused": False,
        }


@pytest.mark.asyncio
async def test_lichenswap_write_helpers_use_expected_calls() -> None:
    connection = FakeConnection()
    client = LichenSwapClient(connection)
    owner = Keypair.from_seed(bytes(range(32)))
    token_a = Keypair.from_seed(bytes(range(1, 33))).pubkey()
    token_b = Keypair.from_seed(bytes(range(2, 34))).pubkey()

    await client.create_pool(owner, CreatePoolParams(token_a=token_a, token_b=token_b))
    await client.add_liquidity(owner, AddLiquidityParams(amount_a=50, amount_b=75, min_liquidity=10))
    await client.swap(owner, SwapParams(amount_in=40, min_amount_out=35), a_to_b=False)
    await client.swap_a_for_b(owner, SwapParams(amount_in=20, min_amount_out=18))
    await client.swap_b_for_a_with_deadline(
        owner,
        SwapWithDeadlineParams(amount_in=25, min_amount_out=21, deadline=1_700_000_000),
    )

    assert [call[2] for call in connection.calls] == [
        "create_pool",
        "add_liquidity",
        "swap",
        "swap_a_for_b",
        "swap_b_for_a_with_deadline",
    ]
    assert connection.calls[0][3][:3] == bytes([0xAB, 0x20, 0x20])
    assert connection.calls[1][3][:5] == bytes([0xAB, 0x20, 0x08, 0x08, 0x08])
    assert connection.calls[1][4] == 125
    assert connection.calls[2][3][:4] == bytes([0xAB, 0x08, 0x08, 0x04])
    assert connection.calls[2][4] == 40
    assert connection.calls[4][3][:4] == bytes([0xAB, 0x08, 0x08, 0x08])
    assert connection.calls[4][4] == 25


@pytest.mark.asyncio
async def test_lichenswap_read_helpers_decode_expected_payloads() -> None:
    connection = FakeConnection()
    client = LichenSwapClient(connection)
    provider = Keypair.from_seed(bytes(range(10, 42))).pubkey()

    pool_info = await client.get_pool_info()
    quote = await client.get_quote(100)
    liquidity_balance = await client.get_liquidity_balance(provider)
    total_liquidity = await client.get_total_liquidity()
    flash_loan_fee = await client.get_flash_loan_fee(100)
    twap = await client.get_twap_cumulatives()
    snapshot_count = await client.get_twap_snapshot_count()
    protocol_fees = await client.get_protocol_fees()
    pool_count = await client.get_pool_count()
    swap_count = await client.get_swap_count()
    total_volume = await client.get_total_volume()
    swap_stats = await client.get_swap_stats()
    stats = await client.get_stats()

    assert pool_info == {"reserve_a": 1_000, "reserve_b": 2_000, "total_liquidity": 3_000}
    assert quote == 777
    assert liquidity_balance == 333
    assert total_liquidity == 3_000
    assert flash_loan_fee == 5
    assert twap == {"cumulative_price_a": 11, "cumulative_price_b": 22, "last_updated_at": 33}
    assert snapshot_count == 4
    assert protocol_fees == {"fees_a": 12, "fees_b": 34}
    assert pool_count == 2
    assert swap_count == 9
    assert total_volume == {"volume_a": 100, "volume_b": 200}
    assert swap_stats == {
        "swap_count": 9,
        "volume_a": 100,
        "volume_b": 200,
        "pool_count": 2,
        "total_liquidity": 3_000,
    }
    assert stats == {"swap_count": 9, "volume_a": 100, "volume_b": 200, "paused": False}