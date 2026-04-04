#!/usr/bin/env python3
"""Verify DEX order book for LICN/lUSD pair."""
import json
import os
import urllib.request

RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899').rstrip('/')

def fetch_orderbook(pair_id=1, depth=25):
    with urllib.request.urlopen(f"{RPC}/api/v1/pairs/{pair_id}/orderbook?depth={depth}", timeout=10) as resp:
        payload = json.loads(resp.read())
    if not payload.get("success"):
        raise RuntimeError(payload.get("error") or "orderbook request failed")
    return payload.get("data", {})

# Get order book for pair 1 (LICN/lUSD), depth 25
r = fetch_orderbook(1, 25)
asks = r.get("asks", [])
bids = r.get("bids", [])

print("=== LICN/lUSD Order Book (pair 1) ===")
print(f"\nAsks (sell side): {len(asks)} levels")
for a in asks[:10]:
    p = a.get("price", 0)
    q = a.get("quantity", 0) / 1e9
    print(f"  ASK {q:>12,.0f} LICN @ ${p:.4f}")
if len(asks) > 10:
    print(f"  ... +{len(asks)-10} more levels")

print(f"\nBids (buy side): {len(bids)} levels")
for b in bids[:10]:
    p = b.get("price", 0)
    q = b.get("quantity", 0) / 1e9
    print(f"  BID {q:>12,.0f} LICN @ ${p:.4f}")
if len(bids) > 10:
    print(f"  ... +{len(bids)-10} more levels")

if asks and bids:
    best_ask = asks[0].get("price", 0)
    best_bid = bids[0].get("price", 0)
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
