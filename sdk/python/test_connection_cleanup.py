#!/usr/bin/env python3
"""Connection cleanup tests that do not require a live validator."""

import asyncio

import pytest

from lichen import Connection, PublicKey


def test_local_validator_rpc_ports_derive_matching_ws_ports():
    assert Connection._derive_ws_url("http://127.0.0.1:8899") == "ws://127.0.0.1:8900"
    assert Connection._derive_ws_url("http://127.0.0.1:8901") == "ws://127.0.0.1:8902"
    assert Connection._derive_ws_url("http://127.0.0.1:8903") == "ws://127.0.0.1:8904"
    assert Connection._derive_ws_url("http://127.0.0.1:9899") == "ws://127.0.0.1:9900"
    assert Connection._derive_ws_url("http://127.0.0.1:9901") == "ws://127.0.0.1:9902"
    assert Connection._derive_ws_url("http://127.0.0.1:9903") == "ws://127.0.0.1:9904"


def test_public_rpc_url_derives_standard_ws_path():
    assert (
        Connection._derive_ws_url("https://testnet-rpc.lichen.network")
        == "wss://testnet-rpc.lichen.network/ws"
    )


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


class RecordingConnection(Connection):
    def __init__(self):
        super().__init__("http://127.0.0.1:8899")
        self.calls = []

    async def _rpc(self, method, params=None, headers=None):
        self.calls.append((method, params, headers))
        return {"method": method, "params": params}


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


@pytest.mark.asyncio
async def test_exchange_history_wrappers_use_expected_rpc_methods():
    conn = RecordingConnection()
    account = PublicKey(bytes([0x44]) * 32)

    await conn.get_transactions_by_address(account, limit=25, before_slot=99)
    await conn.get_transaction_history(account, limit=10)
    await conn.get_account_tx_count(account)

    assert conn.calls == [
        (
            "getTransactionsByAddress",
            [account.to_base58(), {"limit": 25, "before_slot": 99}],
            None,
        ),
        ("getTransactionHistory", [account.to_base58(), {"limit": 10}], None),
        ("getAccountTxCount", [account.to_base58()], None),
    ]
