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
        if function_name == "get_vault_status":
            values = [2, 0, 1, 1, 1, 1, 1, 1, 4_000, 6_000, 10_000, 9_000, 250, 4_250, 1, 1, 10, 30, 50_000, 1, 10, 200, 78_894_000]
            payload = b"".join(value.to_bytes(8, "little") for value in values)
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        raise RuntimeError(f"unexpected readonly function: {function_name}")

    async def get_sporevault_stats(self):
        return {
            "total_assets": 5_000,
            "total_shares": 4_500,
            "strategy_count": 2,
            "total_earned": 900,
            "fees_earned": 100,
            "protocol_fees": 50,
            "idle_assets": 2_000,
            "lending_assets": 3_000,
            "accounting_version": 2,
            "deposit_fee_bps": 10,
            "withdrawal_fee_bps": 30,
            "deposit_cap": 50_000,
            "risk_tier": 1,
            "active_lending_strategies": 1,
            "lending_strategy_rows": 1,
            "strategy_registry_bounded": True,
            "strategy_registry_valid": True,
            "total_strategy_allocation": 33,
            "native_licn": True,
            "thalllend_config_valid": True,
            "components_match_total": True,
            "share_state_consistent": True,
            "liquid_custody_covers_accounting": True,
            "paused": False,
            "operational": True,
        }


@pytest.mark.asyncio
async def test_sporevault_write_helpers_use_expected_calls() -> None:
    connection = FakeConnection()
    client = SporeVaultClient(connection)
    depositor = Keypair.from_seed(bytes(range(32)))

    await client.deposit(depositor, 1_000)
    await client.withdraw(depositor, 250)
    await client.harvest(depositor)
    await client.rebalance(depositor)
    await client.set_deposit_fee(depositor, 25)
    await client.set_risk_tier(depositor, 2)
    await client.add_strategy(depositor, 1, 33)
    await client.update_strategy_allocation(depositor, 0, 25)
    await client.migrate_accounting_v2(depositor, 4_000, 6_000)

    assert [call[2] for call in connection.calls] == [
        "deposit",
        "withdraw",
        "harvest",
        "rebalance",
        "set_deposit_fee",
        "set_risk_tier",
        "add_strategy",
        "update_strategy_allocation",
        "migrate_accounting_v2",
    ]
    assert connection.calls[0][3][:3] == bytes([0xAB, 0x20, 0x08])
    assert connection.calls[0][4] == 1_000
    assert connection.calls[1][4] == 0
    assert connection.calls[2][3] == b""
    assert connection.calls[4][3][:3] == bytes([0xAB, 0x20, 0x08])
    assert connection.calls[5][3][:3] == bytes([0xAB, 0x20, 0x01])
    assert connection.calls[6][3][:4] == bytes([0xAB, 0x20, 0x01, 0x08])


@pytest.mark.asyncio
async def test_sporevault_read_helpers_decode_expected_payloads() -> None:
    connection = FakeConnection()
    client = SporeVaultClient(connection)
    user = Keypair.from_seed(bytes(range(10, 42))).pubkey()

    vault_stats = await client.get_vault_stats()
    user_position = await client.get_user_position(user)
    strategy_info = await client.get_strategy_info(0)
    missing_strategy = await client.get_strategy_info(9)
    status = await client.get_vault_status()
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
    assert status["accounting_version"] == 2
    assert status["native_licn"] is True
    assert status["thalllend_config_valid"] is True
    assert status["real_liquid_custody"] == 4_250
    assert status["target_slots_per_year"] == 78_894_000
    assert stats == {
        "total_assets": 5_000,
        "total_shares": 4_500,
        "strategy_count": 2,
        "total_earned": 900,
        "fees_earned": 100,
        "protocol_fees": 50,
        "idle_assets": 2_000,
        "lending_assets": 3_000,
        "accounting_version": 2,
        "deposit_fee_bps": 10,
        "withdrawal_fee_bps": 30,
        "deposit_cap": 50_000,
        "risk_tier": 1,
        "active_lending_strategies": 1,
        "lending_strategy_rows": 1,
        "strategy_registry_bounded": True,
        "strategy_registry_valid": True,
        "total_strategy_allocation": 33,
        "native_licn": True,
        "thalllend_config_valid": True,
        "components_match_total": True,
        "share_state_consistent": True,
        "liquid_custody_covers_accounting": True,
        "paused": False,
        "operational": True,
    }
