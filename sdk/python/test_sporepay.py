from __future__ import annotations

import base64

import pytest

from lichen import Keypair, PublicKey
from lichen.sporepay import (
    CreateStreamParams,
    CreateStreamWithCliffParams,
    SporePayClient,
    TransferStreamParams,
    WithdrawFromStreamParams,
)


class FakeConnection:
    def __init__(self) -> None:
        self.calls: list[tuple[str, str, str, bytes, int]] = []

    async def get_symbol_registry(self, symbol: str):
        if symbol == "SPOREPAY":
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
        stream_bytes = bytes(range(32)) + bytes(range(32, 64))
        stream_bytes += (1_000).to_bytes(8, "little")
        stream_bytes += (250).to_bytes(8, "little")
        stream_bytes += (10).to_bytes(8, "little")
        stream_bytes += (20).to_bytes(8, "little")
        stream_bytes += bytes([0])
        stream_bytes += (9).to_bytes(8, "little")

        if function_name == "get_stream":
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(stream_bytes).decode("ascii")}
        if function_name == "get_stream_info":
            payload = stream_bytes + (12).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        if function_name == "get_withdrawable":
            payload = (333).to_bytes(8, "little")
            return {"success": True, "returnCode": 0, "returnData": base64.b64encode(payload).decode("ascii")}
        raise RuntimeError(f"unexpected readonly function: {function_name}")

    async def get_sporepay_stats(self):
        return {
            "stream_count": 5,
            "total_streamed": 10_000,
            "total_withdrawn": 3_000,
            "cancel_count": 1,
            "paused": False,
        }


@pytest.mark.asyncio
async def test_sporepay_write_helpers_use_expected_calls() -> None:
    connection = FakeConnection()
    client = SporePayClient(connection)
    sender = Keypair.from_seed(bytes(range(32)))
    recipient = Keypair.from_seed(bytes(range(1, 33)))

    await client.create_stream(
        sender,
        CreateStreamParams(
            recipient=recipient.pubkey(),
            total_amount=1_000,
            start_slot=10,
            end_slot=20,
        ),
    )
    await client.create_stream_with_cliff(
        sender,
        CreateStreamWithCliffParams(
            recipient=recipient.pubkey(),
            total_amount=1_000,
            start_slot=10,
            end_slot=20,
            cliff_slot=12,
        ),
    )
    await client.withdraw_from_stream(recipient, WithdrawFromStreamParams(stream_id=2, amount=100))
    await client.cancel_stream(sender, 2)
    await client.transfer_stream(recipient, TransferStreamParams(stream_id=2, new_recipient=sender.pubkey()))

    assert [call[2] for call in connection.calls] == [
        "create_stream",
        "create_stream_with_cliff",
        "withdraw_from_stream",
        "cancel_stream",
        "transfer_stream",
    ]
    assert connection.calls[0][3][:6] == bytes([0xAB, 0x20, 0x20, 0x08, 0x08, 0x08])


@pytest.mark.asyncio
async def test_sporepay_read_helpers_decode_stream_payloads() -> None:
    connection = FakeConnection()
    client = SporePayClient(connection)

    stream = await client.get_stream(2)
    stream_info = await client.get_stream_info(2)
    withdrawable = await client.get_withdrawable(2)
    stats = await client.get_stats()

    assert stream is not None
    assert stream["stream_id"] == 2
    assert stream["total_amount"] == 1_000
    assert stream_info is not None
    assert stream_info["cliff_slot"] == 12
    assert withdrawable == 333
    assert stats["stream_count"] == 5