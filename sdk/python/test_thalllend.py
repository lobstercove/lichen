from __future__ import annotations

import base64

import pytest

from lichen import Keypair, LiquidateParams, PublicKey
from lichen.thalllend import ThallLendClient


class FakeConnection:
    def __init__(self) -> None:
        self.calls: list[tuple[str, str, str, bytes, int]] = []

    async def get_symbol_registry(self, symbol: str):
        if symbol == "LEND":
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
        if function_name == "get_account_info":
            payload = (1_000).to_bytes(8, "little")
            payload += (400).to_bytes(8, "little")
            payload += (21_250).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_protocol_stats":
            payload = (5_000).to_bytes(8, "little")
            payload += (2_000).to_bytes(8, "little")
            payload += (40).to_bytes(8, "little")
            payload += (150).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_interest_rate":
            payload = (254).to_bytes(8, "little")
            payload += (40).to_bytes(8, "little")
            payload += (3_000).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_rate_model":
            payload = b"".join(
                value.to_bytes(8, "little")
                for value in (1_000_000_000_000, 78_894_000, 254, 508, 400, 80, 25_400)
            )
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_market_status":
            payload = b"".join(
                value.to_bytes(8, "little")
                for value in (0, 1, 1, 1, 1, 10_000, 10, 75, 85, 5)
            )
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_deposit_count":
            payload = (7).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_borrow_count":
            payload = (5).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_liquidation_count":
            payload = (2).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        raise RuntimeError(f"unexpected readonly function: {function_name}")

    async def get_thalllend_stats(self):
        return {
            "total_deposits": 5_000,
            "total_borrows": 2_000,
            "available_liquidity": 3_000,
            "utilization_pct": 40,
            "reserves": 150,
            "deposit_count": 7,
            "borrow_count": 5,
            "repay_count": 3,
            "liquidation_count": 2,
            "deposit_cap": 10_000,
            "reserve_factor_pct": 10,
            "licn_configured": True,
            "native_licn": True,
            "oracle_config_present": True,
            "oracle_config_valid": True,
            "paused": False,
        }


@pytest.mark.asyncio
async def test_thalllend_write_helpers_use_expected_calls() -> None:
    connection = FakeConnection()
    client = ThallLendClient(connection)
    depositor = Keypair.from_seed(bytes(range(32)))
    borrower = Keypair.from_seed(bytes(range(1, 33)))
    liquidator = Keypair.from_seed(bytes(range(2, 34)))

    await client.deposit(depositor, 1_000)
    await client.withdraw(depositor, 250)
    await client.borrow(borrower, 500)
    await client.repay(borrower, 200)
    await client.liquidate(liquidator, LiquidateParams(borrower=borrower.pubkey(), repay_amount=150))

    assert [call[2] for call in connection.calls] == [
        "deposit",
        "withdraw",
        "borrow",
        "repay",
        "liquidate",
    ]
    assert connection.calls[0][3][:3] == bytes([0xAB, 0x20, 0x08])
    assert connection.calls[0][4] == 1_000
    assert connection.calls[3][4] == 200
    assert connection.calls[4][3][:4] == bytes([0xAB, 0x20, 0x20, 0x08])
    assert connection.calls[4][4] == 150


@pytest.mark.asyncio
async def test_thalllend_read_helpers_decode_expected_payloads() -> None:
    connection = FakeConnection()
    client = ThallLendClient(connection)
    user = Keypair.from_seed(bytes(range(10, 42))).pubkey()

    account_info = await client.get_account_info(user)
    protocol_stats = await client.get_protocol_stats()
    interest_rate = await client.get_interest_rate()
    rate_model = await client.get_rate_model()
    market_status = await client.get_market_status()
    deposit_count = await client.get_deposit_count()
    borrow_count = await client.get_borrow_count()
    liquidation_count = await client.get_liquidation_count()
    stats = await client.get_stats()

    assert account_info == {"deposit": 1_000, "borrow": 400, "health_factor_bps": 21_250}
    assert protocol_stats == {
        "total_deposits": 5_000,
        "total_borrows": 2_000,
        "utilization_pct": 40,
        "reserves": 150,
    }
    assert interest_rate == {"rate_per_slot": 254, "utilization_pct": 40, "total_available": 3_000}
    assert rate_model == {
        "rate_scale": 1_000_000_000_000,
        "slots_per_year": 78_894_000,
        "base_rate_per_slot": 254,
        "current_rate_per_slot": 508,
        "current_annual_bps": 400,
        "utilization_kink_pct": 80,
        "max_rate_per_slot": 25_400,
    }
    assert market_status == {
        "paused": False,
        "licn_configured": True,
        "native_licn": True,
        "oracle_config_present": True,
        "oracle_config_valid": True,
        "deposit_cap": 10_000,
        "reserve_factor_pct": 10,
        "collateral_factor_pct": 75,
        "liquidation_threshold_pct": 85,
        "liquidation_bonus_pct": 5,
    }
    assert deposit_count == 7
    assert borrow_count == 5
    assert liquidation_count == 2
    assert stats == {
        "total_deposits": 5_000,
        "total_borrows": 2_000,
        "available_liquidity": 3_000,
        "utilization_pct": 40,
        "reserves": 150,
        "deposit_count": 7,
        "borrow_count": 5,
        "repay_count": 3,
        "liquidation_count": 2,
        "deposit_cap": 10_000,
        "reserve_factor_pct": 10,
        "licn_configured": True,
        "native_licn": True,
        "oracle_config_present": True,
        "oracle_config_valid": True,
        "paused": False,
    }
