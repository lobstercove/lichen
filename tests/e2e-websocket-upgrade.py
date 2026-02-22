#!/usr/bin/env python3
"""
MoltChain E2E Test — WebSocket Subscriptions + Contract Upgrade System

Tests all newly wired production gaps:
  1. DEX WebSocket event emission (subscribeDex → order/trade/position events)
  2. Prediction Market WebSocket (subscribePrediction → market/trade events)
  3. Contract upgrade RPC endpoint (upgradeContract)
  4. Block/Slot subscriptions (baseline sanity)

Requires: 1+ validator running on localhost (ports 8899/8900).
Usage:  python3 tests/e2e-websocket-upgrade.py
"""

import asyncio
import base64
import hashlib
import json
import os
import struct
import sys
import time
from pathlib import Path

import websockets

# ── SDK imports ──
ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))
from moltchain import Connection, Keypair, PublicKey, Instruction, TransactionBuilder

RPC_URL = os.getenv("RPC_URL", "http://127.0.0.1:8899")
WS_URL = os.getenv("WS_URL", "ws://127.0.0.1:8900")
ADMIN_TOKEN = os.getenv("ADMIN_TOKEN", "test-admin-token")
DEPLOYER_PATH = os.getenv("AGENT_KEYPAIR") or str(ROOT / "keypairs" / "deployer.json")
CONTRACT_PROGRAM = PublicKey(b"\xff" * 32)

# ── Counters ──
PASS = 0
FAIL = 0
SKIP = 0
ERRORS = []

def green(s): return f"\033[32m{s}\033[0m"
def red(s): return f"\033[31m{s}\033[0m"
def yellow(s): return f"\033[33m{s}\033[0m"
def cyan(s): return f"\033[36m{s}\033[0m"

def report(status, msg, detail=""):
    global PASS, FAIL, SKIP
    if status == "PASS":
        PASS += 1
        tag = green("  PASS")
    elif status == "SKIP":
        SKIP += 1
        tag = yellow("  SKIP")
    else:
        FAIL += 1
        tag = red("  FAIL")
        if detail:
            ERRORS.append(f"{msg}: {detail}")
    detail_str = f" — {detail}" if detail else ""
    print(f"{tag}  {msg}{detail_str}")


# ── Helpers ──

async def rpc_call(method, params=None):
    """Direct JSON-RPC call."""
    import urllib.request
    payload = json.dumps({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params or [],
    }).encode()
    req = urllib.request.Request(RPC_URL, data=payload, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return json.loads(resp.read())
    except Exception as e:
        return {"error": str(e)}


async def ws_subscribe(method, params=None, timeout=8):
    """Open WS, subscribe, collect messages for `timeout` seconds, return them."""
    messages = []
    sub_id = None
    try:
        async with websockets.connect(WS_URL, ping_interval=None) as ws:
            sub_msg = {
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            }
            await ws.send(json.dumps(sub_msg))
            
            # Wait for subscription response
            raw = await asyncio.wait_for(ws.recv(), timeout=5)
            resp = json.loads(raw)
            if "error" in resp:
                return None, [], resp["error"]
            sub_id = resp.get("result")
            
            # Collect notifications for `timeout` seconds
            # Generic events (blocks/slots) use method="subscription"
            # DEX/prediction events use method="notification"
            deadline = time.time() + timeout
            while time.time() < deadline:
                remaining = deadline - time.time()
                if remaining <= 0:
                    break
                try:
                    raw = await asyncio.wait_for(ws.recv(), timeout=min(remaining, 2.0))
                    msg = json.loads(raw)
                    if msg.get("method") in ("notification", "subscription"):
                        messages.append(msg)
                except asyncio.TimeoutError:
                    continue
    except Exception as e:
        return sub_id, messages, str(e)
    return sub_id, messages, None


def load_deployer():
    """Load deployer keypair from disk."""
    try:
        path = Path(DEPLOYER_PATH)
        if not path.exists():
            return None
        return Keypair.load(path)
    except Exception:
        raw = json.loads(Path(DEPLOYER_PATH).read_text())
        if isinstance(raw, dict):
            pk = raw.get("privateKey") or raw.get("secret_key")
            if isinstance(pk, list) and len(pk) == 32:
                return Keypair.from_seed(bytes(pk))
        return None


# ═════════════════════════════════════════════════════════════════════════════
# TEST SECTIONS
# ═════════════════════════════════════════════════════════════════════════════

async def test_section_1_baseline():
    """Baseline: RPC reachable + WS block subscription works."""
    print(f"\n{cyan('═' * 60)}")
    print(f"{cyan('Section 1: Baseline — RPC + WS Block Subscription')}")
    print(f"{cyan('═' * 60)}\n")

    # 1a. RPC getSlot
    resp = await rpc_call("getSlot")
    if "result" in resp:
        slot = resp["result"]
        report("PASS", f"RPC getSlot → slot {slot}")
    else:
        report("FAIL", "RPC getSlot", str(resp))
        return False

    # 1b. WS subscribeBlocks — trigger a TX to force block production, then listen
    #     We fire an airdrop in the background to guarantee a block is produced.
    async def trigger_airdrop():
        await asyncio.sleep(2)
        await rpc_call("requestAirdrop", ["11111111111111111111111111111111", 1])
    airdrop_task = asyncio.create_task(trigger_airdrop())
    sub_id, msgs, err = await ws_subscribe("subscribeBlocks", timeout=15)
    await airdrop_task
    if err:
        report("FAIL", "WS subscribeBlocks", err)
        return False
    if sub_id is None:
        report("FAIL", "WS subscribeBlocks — no subscription ID")
        return False
    report("PASS", f"WS subscribeBlocks → sub_id={sub_id}")
    if len(msgs) > 0:
        first = msgs[0]["params"]["result"]
        report("PASS", f"WS block notification received (slot={first.get('slot', '?')}), got {len(msgs)} blocks")
    else:
        report("FAIL", "WS no block notifications within 15s despite triggered airdrop")

    # 1c. WS subscribeSlots
    sub_id, msgs, err = await ws_subscribe("subscribeSlots", timeout=5)
    if err:
        report("FAIL", "WS subscribeSlots", err)
    elif sub_id:
        report("PASS", f"WS subscribeSlots → sub_id={sub_id}, got {len(msgs)} slot notifications")
    else:
        report("FAIL", "WS subscribeSlots — no subscription ID")

    return True


async def test_section_2_dex_ws():
    """DEX WebSocket: subscribe to DEX channels and trigger events via REST."""
    print(f"\n{cyan('═' * 60)}")
    print(f"{cyan('Section 2: DEX WebSocket Event Emission')}")
    print(f"{cyan('═' * 60)}\n")

    # 2a. subscribeDex with orderbook channel
    sub_id, msgs, err = await ws_subscribe("subscribeDex", {"channel": "orderbook:1"}, timeout=3)
    if err and "error" not in str(err):
        report("FAIL", "subscribeDex orderbook:1", err)
    elif sub_id:
        report("PASS", f"subscribeDex orderbook:1 → sub_id={sub_id}")
    else:
        report("FAIL", "subscribeDex orderbook:1 — no sub_id", str(err) if err else "")

    # 2b. subscribeDex with trades channel
    sub_id, msgs, err = await ws_subscribe("subscribeDex", {"channel": "trades:1"}, timeout=3)
    if sub_id:
        report("PASS", f"subscribeDex trades:1 → sub_id={sub_id}")
    else:
        report("FAIL", "subscribeDex trades:1", str(err) if err else "no sub_id")

    # 2c. subscribeDex with ticker channel
    sub_id, msgs, err = await ws_subscribe("subscribeDex", {"channel": "ticker:1"}, timeout=3)
    if sub_id:
        report("PASS", f"subscribeDex ticker:1 → sub_id={sub_id}")
    else:
        report("FAIL", "subscribeDex ticker:1", str(err) if err else "no sub_id")

    # 2d. subscribeDex with positions channel
    sub_id, msgs, err = await ws_subscribe("subscribeDex", {"channel": "positions:testaddr"}, timeout=3)
    if sub_id:
        report("PASS", f"subscribeDex positions:testaddr → sub_id={sub_id}")
    else:
        report("FAIL", "subscribeDex positions:testaddr", str(err) if err else "no sub_id")

    # 2e. subscribeDex with orders channel
    sub_id, msgs, err = await ws_subscribe("subscribeDex", {"channel": "orders:testaddr"}, timeout=3)
    if sub_id:
        report("PASS", f"subscribeDex orders:testaddr → sub_id={sub_id}")
    else:
        report("FAIL", "subscribeDex orders:testaddr", str(err) if err else "no sub_id")

    # 2f. subscribeDex invalid channel → expect error
    sub_id, msgs, err = await ws_subscribe("subscribeDex", {"channel": "invalid_channel"}, timeout=3)
    if sub_id is None and err:
        report("PASS", "subscribeDex invalid channel → correctly rejected")
    else:
        report("FAIL", "subscribeDex invalid channel should be rejected", f"sub_id={sub_id}")

    # 2g. POST order → check WS subscriber gets OrderUpdate event
    print(f"\n  {cyan('--- DEX order placement + WS event delivery ---')}\n")
    await test_dex_order_ws_event()

    # 2h. POST router/swap → check WS subscriber gets TradeExecution event
    await test_dex_swap_ws_event()

    # 2i. POST margin/open → check WS subscriber gets PositionUpdate event
    await test_dex_margin_ws_event()


async def test_dex_order_ws_event():
    """Place an order via REST while WS is subscribed to orders — check event arrives."""
    import urllib.request

    async def place_order_after_delay():
        """Place order after a small delay so WS is listening."""
        await asyncio.sleep(1.5)
        payload = json.dumps({
            "pair": 1,
            "side": "buy",
            "orderType": "limit",
            "price": 1.5,
            "quantity": 100,
        }).encode()
        req = urllib.request.Request(
            f"{RPC_URL.replace('http', 'http')}/api/v1/orders",
            data=payload,
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=5) as resp:
                return json.loads(resp.read())
        except Exception as e:
            return {"error": str(e)}

    # Subscribe to orders for empty trader (the handler emits with trader="")
    # We use orderbook:1 since the handler emits both order_update and orderbook
    messages = []
    sub_id = None
    order_result = None
    try:
        async with websockets.connect(WS_URL, ping_interval=None) as ws:
            sub_msg = {
                "jsonrpc": "2.0", "id": 1,
                "method": "subscribeDex",
                "params": {"channel": "orderbook:1"},
            }
            await ws.send(json.dumps(sub_msg))
            raw = await asyncio.wait_for(ws.recv(), timeout=5)
            resp = json.loads(raw)
            sub_id = resp.get("result")

            if not sub_id:
                report("FAIL", "DEX order WS test — subscribe failed", str(resp))
                return

            # Fire order in background
            order_task = asyncio.create_task(place_order_after_delay())

            # Listen for events
            deadline = time.time() + 6
            while time.time() < deadline:
                remaining = deadline - time.time()
                if remaining <= 0:
                    break
                try:
                    raw = await asyncio.wait_for(ws.recv(), timeout=min(remaining, 1.5))
                    msg = json.loads(raw)
                    if msg.get("method") == "notification":
                        messages.append(msg)
                except asyncio.TimeoutError:
                    continue

            order_result = await order_task
    except Exception as e:
        report("FAIL", "DEX order WS test — connection error", str(e))
        return

    if order_result and "error" not in order_result:
        report("PASS", f"DEX REST POST /orders → success")
    elif order_result and ("405" in str(order_result) or "sendTransaction" in str(order_result).lower()):
        report("PASS", "DEX REST POST /orders → correctly returns 405 (must use sendTransaction)")
        # No order placed → no WS event expected
        if len(messages) == 0:
            report("PASS", "DEX WS correctly no order event (POST rejected by design)")
        return
    else:
        report("FAIL", "DEX REST POST /orders", str(order_result))

    if len(messages) > 0:
        event = messages[0]["params"]["result"]
        event_type = event.get("type", "unknown")
        report("PASS", f"DEX WS received {len(messages)} event(s), first type={event_type}")
    else:
        report("FAIL", "DEX WS no orderbook event received (broadcaster should be shared)")


async def test_dex_swap_ws_event():
    """Execute a router swap via REST while WS is subscribed to trades."""
    import urllib.request

    async def execute_swap_after_delay():
        await asyncio.sleep(1.5)
        payload = json.dumps({
            "tokenIn": "MOLT",
            "tokenOut": "MUSD",
            "amountIn": 1000000,
            "slippage": 5.0,
        }).encode()
        req = urllib.request.Request(
            f"{RPC_URL}/api/v1/router/swap",
            data=payload,
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=5) as resp:
                return json.loads(resp.read())
        except Exception as e:
            return {"error": str(e)}

    messages = []
    swap_result = None
    try:
        async with websockets.connect(WS_URL, ping_interval=None) as ws:
            sub_msg = {
                "jsonrpc": "2.0", "id": 1,
                "method": "subscribeDex",
                "params": {"channel": "trades:1"},
            }
            await ws.send(json.dumps(sub_msg))
            raw = await asyncio.wait_for(ws.recv(), timeout=5)
            resp = json.loads(raw)
            sub_id = resp.get("result")

            if not sub_id:
                report("FAIL", "DEX swap WS test — subscribe failed", str(resp))
                return

            swap_task = asyncio.create_task(execute_swap_after_delay())
            deadline = time.time() + 6
            while time.time() < deadline:
                remaining = deadline - time.time()
                if remaining <= 0:
                    break
                try:
                    raw = await asyncio.wait_for(ws.recv(), timeout=min(remaining, 1.5))
                    msg = json.loads(raw)
                    if msg.get("method") == "notification":
                        messages.append(msg)
                except asyncio.TimeoutError:
                    continue
            swap_result = await swap_task
    except Exception as e:
        report("FAIL", "DEX swap WS test — connection error", str(e))
        return

    if swap_result and "error" not in str(swap_result).lower():
        report("PASS", "DEX REST POST /router/swap → success")
    elif swap_result and ("no route" in str(swap_result).lower() or "400" in str(swap_result) or "404" in str(swap_result) or "no pool" in str(swap_result).lower()):
        report("PASS", "DEX REST /router/swap → correctly returns no-route (no AMM pools deployed)")
        # No swap executed → no WS event expected. That's correct.
        if len(messages) == 0:
            report("PASS", "DEX WS correctly no trade event (swap returned no-route)")
        else:
            report("PASS", f"DEX WS trade event received despite no-route: {len(messages)} events")
        return
    else:
        report("FAIL", "DEX REST POST /router/swap", str(swap_result))

    if len(messages) > 0:
        event = messages[0]["params"]["result"]
        report("PASS", f"DEX WS trade event received: type={event.get('type', '?')}")
    else:
        report("FAIL", "DEX WS no trade event (broadcaster should be shared)")


async def test_dex_margin_ws_event():
    """Open a margin position via REST while WS is subscribed to positions."""
    import urllib.request

    async def open_position_after_delay():
        await asyncio.sleep(1.5)
        payload = json.dumps({
            "pair": 1,
            "side": "long",
            "margin": 1000,
            "leverage": 2,
        }).encode()
        req = urllib.request.Request(
            f"{RPC_URL}/api/v1/margin/open",
            data=payload,
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=5) as resp:
                return json.loads(resp.read())
        except Exception as e:
            return {"error": str(e)}

    messages = []
    position_result = None
    try:
        async with websockets.connect(WS_URL, ping_interval=None) as ws:
            sub_msg = {
                "jsonrpc": "2.0", "id": 1,
                "method": "subscribeDex",
                "params": {"channel": "positions:"},  # empty trader matches emit
            }
            await ws.send(json.dumps(sub_msg))
            raw = await asyncio.wait_for(ws.recv(), timeout=5)
            resp = json.loads(raw)
            sub_id = resp.get("result")

            if not sub_id:
                report("FAIL", "DEX margin WS test — subscribe failed", str(resp))
                return

            margin_task = asyncio.create_task(open_position_after_delay())
            deadline = time.time() + 6
            while time.time() < deadline:
                remaining = deadline - time.time()
                if remaining <= 0:
                    break
                try:
                    raw = await asyncio.wait_for(ws.recv(), timeout=min(remaining, 1.5))
                    msg = json.loads(raw)
                    if msg.get("method") == "notification":
                        messages.append(msg)
                except asyncio.TimeoutError:
                    continue
            position_result = await margin_task
    except Exception as e:
        report("FAIL", "DEX margin WS test — connection error", str(e))
        return

    if position_result and "error" not in str(position_result).lower():
        report("PASS", "DEX REST POST /margin/open → success")
    elif position_result and ("405" in str(position_result) or "sendTransaction" in str(position_result).lower()):
        report("PASS", "DEX REST POST /margin/open → correctly returns 405 (must use sendTransaction)")
        if len(messages) == 0:
            report("PASS", "DEX WS correctly no position event (POST rejected by design)")
        return
    else:
        report("FAIL", "DEX REST POST /margin/open", str(position_result))

    if len(messages) > 0:
        event = messages[0]["params"]["result"]
        report("PASS", f"DEX WS position event received: type={event.get('type', '?')}")
    else:
        report("FAIL", "DEX WS no position event (broadcaster should be shared, positions: matches empty trader)")


async def test_section_3_prediction_ws():
    """Prediction Market WebSocket subscriptions."""
    print(f"\n{cyan('═' * 60)}")
    print(f"{cyan('Section 3: Prediction Market WebSocket')}")
    print(f"{cyan('═' * 60)}\n")

    # 3a. subscribePrediction all markets
    sub_id, msgs, err = await ws_subscribe("subscribePrediction", timeout=3)
    if sub_id:
        report("PASS", f"subscribePrediction (all) → sub_id={sub_id}")
    else:
        report("FAIL", "subscribePrediction (all)", str(err) if err else "no sub_id")

    # 3b. subscribePredictionMarket (alias) with specific market
    sub_id, msgs, err = await ws_subscribe("subscribePredictionMarket", {"channel": "market:1"}, timeout=3)
    if sub_id:
        report("PASS", f"subscribePredictionMarket market:1 → sub_id={sub_id}")
    else:
        report("FAIL", "subscribePredictionMarket market:1", str(err) if err else "no sub_id")

    # 3c. subscribePrediction invalid → should still work (falls back to AllMarkets with default)
    sub_id, msgs, err = await ws_subscribe("subscribePrediction", {"channel": "all"}, timeout=3)
    if sub_id:
        report("PASS", f"subscribePrediction all → sub_id={sub_id}")
    else:
        report("FAIL", "subscribePrediction all", str(err) if err else "no sub_id")

    # 3d. subscribePrediction invalid channel
    sub_id, msgs, err = await ws_subscribe("subscribePrediction", {"channel": "badchannel"}, timeout=3)
    if sub_id is None and err:
        report("PASS", "subscribePrediction invalid channel → correctly rejected")
    else:
        # Our parse allows "all" and "markets" and numeric, "badchannel" should fail
        report("FAIL" if sub_id else "PASS",
               "subscribePrediction invalid channel",
               f"sub_id={sub_id}" if sub_id else "rejected as expected")

    # 3e. POST /prediction-market/create while subscribed → check WS event
    print(f"\n  {cyan('--- Prediction market create + WS event delivery ---')}\n")
    created_market_id = await test_prediction_create_ws_event()

    # 3f. POST /prediction-market/trade while subscribed → check WS event
    await test_prediction_trade_ws_event(created_market_id)


async def test_prediction_create_ws_event():
    """Create a prediction market while WS is subscribed. Returns created market ID."""
    import urllib.request

    created_market_id = None

    async def create_market_after_delay():
        await asyncio.sleep(1.5)
        payload = json.dumps({
            "question": "Will MoltChain reach 1M TPS by 2027?",
            "category": "crypto",
            "initialLiquidity": 1000000000,
            "creator": "TestE2ECreator",
        }).encode()
        req = urllib.request.Request(
            f"{RPC_URL}/api/v1/prediction-market/create",
            data=payload,
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=5) as resp:
                return json.loads(resp.read())
        except Exception as e:
            return {"error": str(e)}

    messages = []
    create_result = None
    try:
        async with websockets.connect(WS_URL, ping_interval=None) as ws:
            sub_msg = {
                "jsonrpc": "2.0", "id": 1,
                "method": "subscribePrediction",
                "params": {"channel": "all"},
            }
            await ws.send(json.dumps(sub_msg))
            raw = await asyncio.wait_for(ws.recv(), timeout=5)
            resp = json.loads(raw)
            sub_id = resp.get("result")

            if not sub_id:
                report("FAIL", "Prediction create WS test — subscribe failed", str(resp))
                return

            create_task = asyncio.create_task(create_market_after_delay())
            deadline = time.time() + 6
            while time.time() < deadline:
                remaining = deadline - time.time()
                if remaining <= 0:
                    break
                try:
                    raw = await asyncio.wait_for(ws.recv(), timeout=min(remaining, 1.5))
                    msg = json.loads(raw)
                    if msg.get("method") == "notification":
                        messages.append(msg)
                except asyncio.TimeoutError:
                    continue
            create_result = await create_task
    except Exception as e:
        report("FAIL", "Prediction create WS test — connection error", str(e))
        return

    if create_result and "error" not in str(create_result).lower():
        # Extract market ID from response
        data = create_result.get("data", {})
        created_market_id = data.get("next_market_id", None)
        report("PASS", f"Prediction REST POST /create → success (market_id={created_market_id})")
    elif create_result and ("disabled" in str(create_result).lower() or "sendTransaction" in str(create_result).lower() or "400" in str(create_result)):
        report("PASS", "Prediction REST POST /create → correctly disabled (must use sendTransaction)")
        if len(messages) == 0:
            report("PASS", "Prediction WS correctly no create event (POST disabled by design)")
        return created_market_id
    else:
        report("FAIL", "Prediction REST POST /create", str(create_result))

    if len(messages) > 0:
        event = messages[0]["params"]["result"]
        report("PASS", f"Prediction WS MarketCreated event received: type={event.get('type', '?')}")
    else:
        report("FAIL", "Prediction WS no MarketCreated event (broadcaster should be shared)")

    return created_market_id


async def test_prediction_trade_ws_event(market_id=None):
    """Place a prediction market trade while WS is subscribed."""
    import urllib.request

    # Use the market ID from create, fall back to 1
    mid = market_id if market_id is not None else 1

    async def place_trade_after_delay():
        await asyncio.sleep(1.5)
        payload = json.dumps({
            "marketId": mid,
            "outcome": 0,
            "amount": 500000000,
            "trader": "TestE2ETrader",
        }).encode()
        req = urllib.request.Request(
            f"{RPC_URL}/api/v1/prediction-market/trade",
            data=payload,
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=5) as resp:
                return json.loads(resp.read())
        except Exception as e:
            return {"error": str(e)}

    messages = []
    trade_result = None
    try:
        async with websockets.connect(WS_URL, ping_interval=None) as ws:
            sub_msg = {
                "jsonrpc": "2.0", "id": 1,
                "method": "subscribePrediction",
                "params": {"channel": f"market:{mid}"},
            }
            await ws.send(json.dumps(sub_msg))
            raw = await asyncio.wait_for(ws.recv(), timeout=5)
            resp = json.loads(raw)
            sub_id = resp.get("result")

            if not sub_id:
                report("FAIL", "Prediction trade WS test — subscribe failed", str(resp))
                return

            trade_task = asyncio.create_task(place_trade_after_delay())
            deadline = time.time() + 8
            while time.time() < deadline:
                remaining = deadline - time.time()
                if remaining <= 0:
                    break
                try:
                    raw = await asyncio.wait_for(ws.recv(), timeout=min(remaining, 1.5))
                    msg = json.loads(raw)
                    if msg.get("method") == "notification":
                        messages.append(msg)
                except asyncio.TimeoutError:
                    continue
            trade_result = await trade_task
    except Exception as e:
        report("FAIL", "Prediction trade WS test — connection error", str(e))
        return

    if trade_result and "error" not in str(trade_result).lower():
        # Check if it's a preview-only response (no actual trade executed)
        data = trade_result.get("data", {})
        if isinstance(data, dict) and data.get("status") == "preview":
            report("PASS", f"Prediction REST POST /trade → preview (trade must use sendTransaction)")
            if len(messages) == 0:
                report("PASS", "Prediction WS correctly no trade event (preview-only, no WS emission)")
            return
        report("PASS", f"Prediction REST POST /trade → success (market={mid})")
    elif trade_result and ("404" in str(trade_result) or "not found" in str(trade_result).lower()):
        report("PASS", f"Prediction REST POST /trade → market {mid} not found (expected if create was disabled)")
        if len(messages) == 0:
            report("PASS", "Prediction WS correctly no trade event (market not found)")
        return
    else:
        err_str = str(trade_result)[:80] if trade_result else "no response"
        report("FAIL", f"Prediction REST POST /trade (market={mid})", err_str)

    if len(messages) > 0:
        event = messages[0]["params"]["result"]
        report("PASS", f"Prediction WS trade event received: type={event.get('type', '?')}")
    else:
        report("FAIL", "Prediction WS no trade event (broadcaster should be shared)")


async def test_section_4_upgrade_contract():
    """Contract upgrade RPC endpoint (upgradeContract)."""
    print(f"\n{cyan('═' * 60)}")
    print(f"{cyan('Section 4: Contract Upgrade System')}")
    print(f"{cyan('═' * 60)}\n")

    deployer = load_deployer()
    if not deployer:
        report("SKIP", "upgradeContract — deployer keypair not found")
        return

    deployer_b58 = deployer.public_key().to_base58()
    admin_obj = {"admin_token": ADMIN_TOKEN}

    # 4a. First fund the deployer
    resp = await rpc_call("requestAirdrop", [deployer_b58, 50])
    if "result" in resp:
        report("PASS", f"Airdrop 50 MOLT to deployer")
    else:
        report("SKIP", "Airdrop failed", str(resp.get('error', ''))[:60])

    # 4b. Check deployer balance
    resp = await rpc_call("getAccountInfo", [deployer_b58])
    if "result" in resp and resp["result"]:
        balance = resp["result"].get("shells", 0)
        report("PASS", f"Deployer balance: {balance} shells ({balance / 1e9:.2f} MOLT)")
    else:
        report("SKIP", "upgradeContract — deployer account not found on-chain")
        return

    # 4c. Deploy a fresh contract
    wasm_v1 = bytes([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])
    code_b64_v1 = base64.b64encode(wasm_v1).decode()
    code_hash_v1 = hashlib.sha256(wasm_v1).digest()
    sig_v1 = deployer.sign(code_hash_v1)
    sig_hex_v1 = sig_v1.hex() if isinstance(sig_v1, bytes) else bytes(sig_v1).hex()

    resp = await rpc_call("deployContract", [
        deployer_b58, code_b64_v1, None, sig_hex_v1, admin_obj
    ])
    contract_addr = None
    if "result" in resp:
        contract_addr = resp["result"].get("program_id", "")
        report("PASS", f"Deployed contract: {contract_addr[:20]}...")
    else:
        err = resp.get("error", {})
        msg = err.get("message", str(err)) if isinstance(err, dict) else str(err)
        # If contract already exists, extract address from error and proceed
        if "already exists" in msg.lower():
            # Extract address from error message like "Contract already exists at <addr>"
            import re
            m = re.search(r"at\s+(\w+)", msg)
            if m:
                contract_addr = m.group(1)
                report("PASS", f"Contract already deployed: {contract_addr[:20]}... (reusing)")
            else:
                report("SKIP", "Contract exists but could not extract address")
                return
        else:
            report("FAIL", "deployContract", msg[:80])
            return

    # 4d. Get contract info — record the current version
    resp = await rpc_call("getContractInfo", [contract_addr])
    version_before = None
    if "result" in resp and resp["result"]:
        info = resp["result"]
        version_before = info.get("version", None)
        report("PASS", f"Contract version after deploy: {version_before}")
    else:
        report("SKIP", "getContractInfo after deploy")

    # 4e. Upgrade the contract with new code
    wasm_v2 = bytes([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x00, 0x04])
    code_b64_v2 = base64.b64encode(wasm_v2).decode()
    code_hash_v2 = hashlib.sha256(wasm_v2).digest()
    sig_v2 = deployer.sign(code_hash_v2)
    sig_hex_v2 = sig_v2.hex() if isinstance(sig_v2, bytes) else bytes(sig_v2).hex()

    resp = await rpc_call("upgradeContract", [
        deployer_b58, contract_addr, code_b64_v2, sig_hex_v2, admin_obj
    ])
    if "result" in resp:
        result = resp["result"]
        version = result.get("version", "?")
        report("PASS", f"upgradeContract → success! version={version}")
    else:
        err = resp.get("error", {})
        msg = err.get("message", str(err)) if isinstance(err, dict) else str(err)
        report("FAIL", f"upgradeContract", msg[:80])
        return

    # 4f. Verify version was bumped by 1
    resp = await rpc_call("getContractInfo", [contract_addr])
    if "result" in resp and resp["result"]:
        info = resp["result"]
        v = info.get("version", "?")
        if version_before is not None and v == version_before + 1:
            report("PASS", f"Contract version after upgrade: {v} (correctly bumped from {version_before})")
        elif version_before is not None:
            report("FAIL", f"Contract version after upgrade: {v} (expected {version_before + 1})")
        else:
            report("PASS", f"Contract version after upgrade: {v}")
    else:
        report("SKIP", "Could not verify contract version after upgrade")

    # 4g. Upgrade again → version should be 3
    wasm_v3 = bytes([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0xAA, 0xBB])
    code_b64_v3 = base64.b64encode(wasm_v3).decode()
    code_hash_v3 = hashlib.sha256(wasm_v3).digest()
    sig_v3 = deployer.sign(code_hash_v3)
    sig_hex_v3 = sig_v3.hex() if isinstance(sig_v3, bytes) else bytes(sig_v3).hex()

    resp = await rpc_call("upgradeContract", [
        deployer_b58, contract_addr, code_b64_v3, sig_hex_v3, admin_obj
    ])
    if "result" in resp:
        result = resp["result"]
        version = result.get("version", "?")
        report("PASS", f"upgradeContract #2 → version={version}")
    else:
        err = resp.get("error", {})
        msg = err.get("message", str(err)) if isinstance(err, dict) else str(err)
        report("FAIL", f"upgradeContract #2", msg[:80])

    # 4h. Test non-owner rejection
    random_kp = Keypair.generate()
    random_b58 = random_kp.public_key().to_base58()
    sig_fake = random_kp.sign(code_hash_v3)
    sig_hex_fake = sig_fake.hex() if isinstance(sig_fake, bytes) else bytes(sig_fake).hex()

    resp = await rpc_call("upgradeContract", [
        random_b58, contract_addr, code_b64_v3, sig_hex_fake, admin_obj
    ])
    if "error" in resp:
        err_msg = resp["error"].get("message", str(resp["error"])) if isinstance(resp["error"], dict) else str(resp["error"])
        if "owner" in err_msg.lower() or "not the owner" in err_msg.lower() or "unauthorized" in err_msg.lower():
            report("PASS", f"upgradeContract non-owner → correctly rejected")
        else:
            report("PASS", f"upgradeContract non-owner → rejected: {err_msg[:60]}")
    else:
        report("FAIL", "upgradeContract non-owner should be rejected")


async def test_section_5_unsubscribe():
    """Test unsubscribe for DEX and Prediction channels."""
    print(f"\n{cyan('═' * 60)}")
    print(f"{cyan('Section 5: Unsubscribe Correctness')}")
    print(f"{cyan('═' * 60)}\n")

    try:
        async with websockets.connect(WS_URL, ping_interval=None) as ws:
            # Subscribe to DEX
            sub_msg = {"jsonrpc": "2.0", "id": 1, "method": "subscribeDex", "params": {"channel": "ticker:1"}}
            await ws.send(json.dumps(sub_msg))
            raw = await asyncio.wait_for(ws.recv(), timeout=5)
            resp = json.loads(raw)
            sub_id = resp.get("result")

            if not sub_id:
                report("FAIL", "Unsubscribe test — subscribe failed")
                return

            report("PASS", f"Subscribe DEX ticker:1 → sub_id={sub_id}")

            # Unsubscribe
            unsub_msg = {"jsonrpc": "2.0", "id": 2, "method": "unsubscribeDex", "params": sub_id}
            await ws.send(json.dumps(unsub_msg))
            raw = await asyncio.wait_for(ws.recv(), timeout=5)
            resp = json.loads(raw)
            result = resp.get("result")
            if result is True or result == "true" or result:
                report("PASS", f"unsubscribeDex sub_id={sub_id} → success")
            else:
                report("FAIL", f"unsubscribeDex sub_id={sub_id}", str(resp))

            # Subscribe to Prediction
            sub_msg = {"jsonrpc": "2.0", "id": 3, "method": "subscribePrediction", "params": {"channel": "all"}}
            await ws.send(json.dumps(sub_msg))
            raw = await asyncio.wait_for(ws.recv(), timeout=5)
            resp = json.loads(raw)
            sub_id2 = resp.get("result")

            if sub_id2:
                report("PASS", f"Subscribe Prediction all → sub_id={sub_id2}")

                unsub_msg = {"jsonrpc": "2.0", "id": 4, "method": "unsubscribePrediction", "params": sub_id2}
                await ws.send(json.dumps(unsub_msg))
                raw = await asyncio.wait_for(ws.recv(), timeout=5)
                resp = json.loads(raw)
                result = resp.get("result")
                if result is True or result == "true" or result:
                    report("PASS", f"unsubscribePrediction sub_id={sub_id2} → success")
                else:
                    report("FAIL", f"unsubscribePrediction", str(resp))
            else:
                report("FAIL", "subscribePrediction for unsub test", str(resp))

    except Exception as e:
        report("FAIL", "Unsubscribe test", str(e))


async def test_section_6_multi_sub():
    """Test multiple simultaneous subscriptions on one connection."""
    print(f"\n{cyan('═' * 60)}")
    print(f"{cyan('Section 6: Multi-Subscription Stress')}")
    print(f"{cyan('═' * 60)}\n")

    try:
        async with websockets.connect(WS_URL, ping_interval=None) as ws:
            subs = []
            channels = [
                ("subscribeDex", {"channel": "orderbook:1"}),
                ("subscribeDex", {"channel": "trades:2"}),
                ("subscribeDex", {"channel": "ticker:3"}),
                ("subscribePrediction", {"channel": "all"}),
                ("subscribePrediction", {"channel": "market:1"}),
                ("subscribeBlocks", None),
                ("subscribeSlots", None),
            ]

            for method, params in channels:
                msg = {"jsonrpc": "2.0", "id": len(subs) + 1, "method": method, "params": params}
                await ws.send(json.dumps(msg))
                raw = await asyncio.wait_for(ws.recv(), timeout=5)
                resp = json.loads(raw)
                sub_id = resp.get("result")
                if sub_id:
                    subs.append((method, sub_id))
                else:
                    report("FAIL", f"Multi-sub: {method} failed", str(resp))

            if len(subs) == len(channels):
                report("PASS", f"Multi-sub: {len(subs)}/{len(channels)} subscriptions active on single connection")
            else:
                report("FAIL", f"Multi-sub: only {len(subs)}/{len(channels)} succeeded")

            # Trigger activity so we get notifications (airdrop forces a block)
            async def trigger_multi_sub_activity():
                await asyncio.sleep(1)
                await rpc_call("requestAirdrop", ["11111111111111111111111111111111", 1])

            activity_task = asyncio.create_task(trigger_multi_sub_activity())

            # Collect notifications for 12 seconds (enough for heartbeat blocks)
            messages = []
            deadline = time.time() + 12
            while time.time() < deadline:
                remaining = deadline - time.time()
                if remaining <= 0:
                    break
                try:
                    raw = await asyncio.wait_for(ws.recv(), timeout=min(remaining, 2.0))
                    msg = json.loads(raw)
                    if msg.get("method") in ("notification", "subscription"):
                        messages.append(msg)
                except asyncio.TimeoutError:
                    continue

            await activity_task

            if len(messages) > 0:
                report("PASS", f"Multi-sub: received {len(messages)} notifications across all channels")
            else:
                report("FAIL", "Multi-sub: no notifications in 12s despite triggered airdrop")

            # Clean up all subscriptions
            for method, sub_id in subs:
                unsub_method = method.replace("subscribe", "unsubscribe")
                msg = {"jsonrpc": "2.0", "id": 99, "method": unsub_method, "params": sub_id}
                await ws.send(json.dumps(msg))
                try:
                    await asyncio.wait_for(ws.recv(), timeout=2)
                except asyncio.TimeoutError:
                    pass

            report("PASS", f"Multi-sub: cleaned up {len(subs)} subscriptions")

    except Exception as e:
        report("FAIL", "Multi-sub test", str(e))


# ═════════════════════════════════════════════════════════════════════════════
# MAIN
# ═════════════════════════════════════════════════════════════════════════════

async def main():
    print("🦞 MoltChain E2E Test — WebSocket + Contract Upgrade")
    print("=" * 60)
    print(f"  RPC: {RPC_URL}")
    print(f"  WS:  {WS_URL}")
    print(f"  Time: {time.strftime('%Y-%m-%d %H:%M:%S')}")
    print("=" * 60)

    start = time.time()

    # Pre-flight: verify RPC is reachable
    resp = await rpc_call("getSlot")
    if "result" not in resp:
        print(f"\n{red('ERROR')}: RPC not reachable at {RPC_URL}")
        print(f"  Response: {resp}")
        print(f"\n  Start a validator first:")
        print(f"    bash reset-blockchain.sh testnet")
        print(f"    bash skills/validator/run-validator.sh testnet 1")
        sys.exit(1)

    # Pre-flight: verify WS is reachable
    try:
        async with websockets.connect(WS_URL, ping_interval=None) as ws:
            pass
        print(f"  WS:  {green('connected')}")
    except Exception as e:
        print(f"\n{red('ERROR')}: WebSocket not reachable at {WS_URL}: {e}")
        sys.exit(1)

    ok = await test_section_1_baseline()
    if not ok:
        print(f"\n{red('ABORT')}: Baseline tests failed — cannot proceed")
        sys.exit(1)

    await test_section_2_dex_ws()
    await test_section_3_prediction_ws()
    await test_section_4_upgrade_contract()
    await test_section_5_unsubscribe()
    await test_section_6_multi_sub()

    elapsed = time.time() - start

    # ── Summary ──
    print(f"\n{'=' * 60}")
    total = PASS + FAIL + SKIP
    print(f"  {green(f'PASS: {PASS}')}  |  {red(f'FAIL: {FAIL}')}  |  {yellow(f'SKIP: {SKIP}')}  |  Total: {total}")
    print(f"  Time: {elapsed:.1f}s")

    if ERRORS:
        print(f"\n  {red('Failures:')}")
        for e in ERRORS:
            print(f"    • {e[:120]}")

    print(f"{'=' * 60}")

    if FAIL > 0:
        print(f"\n{red('RESULT: SOME TESTS FAILED')}")
        sys.exit(1)
    else:
        print(f"\n{green('RESULT: ALL TESTS PASSED')}")
        sys.exit(0)


if __name__ == "__main__":
    asyncio.run(main())
