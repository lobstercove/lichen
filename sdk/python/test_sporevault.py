from __future__ import annotations

import base64

import pytest

from lichen import Keypair, PublicKey, SporeVaultClient


class FakeConnection:
    def __init__(self) -> None:
        self.calls: list[tuple[str, str, str, bytes, int]] = []

    async def get_symbol_registry(self, symbol: str):
        if symbol == "SPOREVAULT":
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
        if function_name == "get_vault_stats":
            payload = (5_000).to_bytes(8, "little")
            payload += (4_500).to_bytes(8, "little")
            payload += (1_111_111_111).to_bytes(8, "little")
            payload += (2).to_bytes(8, "little")
            payload += (900).to_bytes(8, "little")
            payload += (100).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_user_position":
            payload = (200).to_bytes(8, "little")
            payload += (222).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_strategy_info":
            index = int.from_bytes(args[-8:], "little")
            if index == 0:
                payload = (1).to_bytes(8, "little")
                payload += (60).to_bytes(8, "little")
                payload += (3_000).to_bytes(8, "little")
                return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
            return {"success": True, "returnCode": 1, "returnData": None}
        raise RuntimeError(f"unexpected readonly function: {function_name}")

    async def get_sporevault_stats(self):
        return {
            "total_assets": 5_000,
            "total_shares": 4_500,
            "strategy_count": 2,
            "total_earned": 900,
            "fees_earned": 100,
            "protocol_fees": 50,
            "paused": False,
        }


@pytest.mark.asyncio
async def test_sporevault_write_helpers_use_expected_calls() -> None:
    connection = FakeConnection()
    client = SporeVaultClient(connection)
    depositor = Keypair.from_seed(bytes(range(32)))

    await client.deposit(depositor, 1_000)
    await client.withdraw(depositor, 250)
    await client.harvest(depositor)

    assert [call[2] for call in connection.calls] == [
        "deposit",
        "withdraw",
        "harvest",
    ]
    assert connection.calls[0][3][:3] == bytes([0xAB, 0x20, 0x08])
    assert connection.calls[0][4] == 1_000
    assert connection.calls[1][4] == 0
    assert connection.calls[2][3] == b""


@pytest.mark.asyncio
async def test_sporevault_read_helpers_decode_expected_payloads() -> None:
    connection = FakeConnection()
    client = SporeVaultClient(connection)
    user = Keypair.from_seed(bytes(range(10, 42))).pubkey()

    vault_stats = await client.get_vault_stats()
    user_position = await client.get_user_position(user)
    strategy_info = await client.get_strategy_info(0)
    missing_strategy = await client.get_strategy_info(9)
    stats = await client.get_stats()

    assert vault_stats == {
        "total_assets": 5_000,
        "total_shares": 4_500,
        "share_price_e9": 1_111_111_111,
        "strategy_count": 2,
        "total_earned": 900,
        "fees_earned": 100,
    }
    assert user_position == {"shares": 200, "estimated_value": 222}
    assert strategy_info == {
        "strategy_type": 1,
        "allocation_percent": 60,
        "deployed_amount": 3_000,
    }
    assert missing_strategy is None
    assert stats == {
        "total_assets": 5_000,
        "total_shares": 4_500,
        "strategy_count": 2,
        "total_earned": 900,
        "fees_earned": 100,
        "protocol_fees": 50,
        "paused": False,
    }