#!/usr/bin/env python3
"""Verify DEX order book for LICN/lUSD pair"""
import sys, json, urllib.request

RPC = 'http://127.0.0.1:8899'

def rpc(method, params=None):
    payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or []})
    req = urllib.request.Request(RPC, data=payload.encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        return json.loads(resp.read()).get("result", {})

# Get order book for pair 1 (LICN/lUSD), depth 25
r = rpc("getOrderBook", [1, 25])
asks = r.get("asks", [])
bids = r.get("bids", [])

print("=== LICN/lUSD Order Book (pair 1) ===")
print(f"\nAsks (sell side): {len(asks)} levels")
for a in asks[:10]:
    p = a.get("price", 0) / 1e9
    q = a.get("quantity", 0) / 1e9
    print(f"  ASK {q:>12,.0f} LICN @ ${p:.4f}")
if len(asks) > 10:
    print(f"  ... +{len(asks)-10} more levels")

print(f"\nBids (buy side): {len(bids)} levels")
for b in bids[:10]:
    p = b.get("price", 0) / 1e9
    q = b.get("quantity", 0) / 1e9
    print(f"  BID {q:>12,.0f} LICN @ ${p:.4f}")
if len(bids) > 10:
    print(f"  ... +{len(bids)-10} more levels")

if asks and bids:
    best_ask = asks[0].get("price", 0) / 1e9
    best_bid = bids[0].get("price", 0) / 1e9
    spread = best_ask - best_bid
    mid = (best_ask + best_bid) / 2
    spread_pct = (spread / mid * 100) if mid > 0 else 0
    print(f"\n--- Summary ---")
    print(f"Best Ask:  ${best_ask:.4f}")
    print(f"Best Bid:  ${best_bid:.4f}")
    print(f"Spread:    ${spread:.4f} ({spread_pct:.2f}%)")
    print(f"Midpoint:  ${mid:.4f}")
    if spread_pct < 5:
        print("PASS: Spread is tight (< 5%)")
    else:
        print(f"WARN: Spread is wide ({spread_pct:.1f}%)")
else:
    if not asks:
        print("\nFAIL: No asks (sell side missing)")
    if not bids:
        print("\nFAIL: No bids (buy side missing)")
