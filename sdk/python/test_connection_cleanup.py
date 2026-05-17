#!/usr/bin/env python3
"""Connection cleanup tests that do not require a live validator."""

import asyncio

import pytest

from lichen import Connection


class ConfirmCleanupConnection(Connection):
    def __init__(self):
        super().__init__("http://127.0.0.1:8899", "ws://127.0.0.1:8900")
        self.unsubscribed = []

    async def _connect_ws(self):
        return None

    async def _subscribe(self, method, params=None):
        assert method == "signatureSubscribe"
        return 101

    async def _unsubscribe(self, method, sub_id):
        await asyncio.sleep(0)
        self.unsubscribed.append((method, sub_id))
        return True

    async def _confirm_via_rpc(self, signature, timeout):
        await asyncio.sleep(0)
        return {"signature": signature, "confirmed": True}


@pytest.mark.asyncio
async def test_signature_subscription_is_unsubscribed_when_rpc_wins_race():
    conn = ConfirmCleanupConnection()

    result = await conn.confirm_transaction("abc123", timeout=1.0)

    assert result == {"signature": "abc123", "confirmed": True}
    for _ in range(20):
        if conn.unsubscribed:
            break
        await asyncio.sleep(0.01)
    assert conn.unsubscribed == [("signatureUnsubscribe", 101)]
    assert conn._subscriptions == {}
