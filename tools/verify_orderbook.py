#!/usr/bin/env python3
"""Verify DEX order books for all launch pairs."""
import json
import os
import urllib.request

RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899').rstrip('/')
PAIR_NAMES = {
    1: "LICN/lUSD",
    2: "wSOL/lUSD",
    3: "wETH/lUSD",
    4: "wSOL/LICN",
    5: "wETH/LICN",
    6: "wBNB/lUSD",
    7: "wBNB/LICN",
    8: "wNEO/lUSD",
    9: "wNEO/LICN",
    10: "wGAS/lUSD",
    11: "wGAS/LICN",
}

def fetch_orderbook(pair_id=1, depth=25):
    with urllib.request.urlopen(f"{RPC}/api/v1/pairs/{pair_id}/orderbook?depth={depth}", timeout=10) as resp:
        payload = json.loads(resp.read())
    if not payload.get("success"):
        raise RuntimeError(payload.get("error") or "orderbook request failed")
    return payload.get("data", {})

failures = []
for pair_id, name in PAIR_NAMES.items():
    r = fetch_orderbook(pair_id, 25)
    asks = r.get("asks", [])
    bids = r.get("bids", [])
    print(f"=== {name} Order Book (pair {pair_id}) ===")
    print(f"  asks={len(asks)} bids={len(bids)}")

    if asks and bids:
        best_ask = asks[0].get("price", 0)
        best_bid = bids[0].get("price", 0)
        spread = best_ask - best_bid
        mid = (best_ask + best_bid) / 2
        spread_pct = (spread / mid * 100) if mid > 0 else 0
        print(f"  best_ask={best_ask:.6f} best_bid={best_bid:.6f} spread={spread_pct:.2f}%")
    else:
        if not asks:
            failures.append(f"{name}: no asks")
        if not bids:
            failures.append(f"{name}: no bids")
    print()

if failures:
    print("FAIL:")
    for failure in failures:
        print(f"  {failure}")
    raise SystemExit(1)

print("PASS: all launch pairs have both asks and bids")
