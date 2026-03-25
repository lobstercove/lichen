#!/usr/bin/env python3
"""Quick check: verify validator is running and contracts are deployed."""
import json, httpx, sys

RPC = "http://127.0.0.1:8899"

def rpc(method, params=None):
    r = httpx.post(RPC, json={"jsonrpc":"2.0","id":1,"method":method,"params":params or []}, timeout=5)
    d = r.json()
    if "error" in d:
        return None
    return d.get("result")

# Health
h = rpc("health")
print(f"Health: {h}")

# Slot
s = rpc("getSlot")
print(f"Slot: {s}")

# Symbol registry
sr = rpc("getAllSymbolRegistry")
if sr:
    entries = sr.get("entries", []) if isinstance(sr, dict) else sr
    print(f"Symbol Registry: {len(entries)} entries")
    for e in entries:
        print(f"  {e.get('symbol','?'):12s} → {e.get('program','')[:20]}...")
else:
    print("Symbol Registry: unavailable")

# All contracts
ac = rpc("getAllContracts")
if ac:
    contracts = ac.get("contracts", []) if isinstance(ac, dict) else ac
    print(f"All Contracts: {len(contracts)}")
else:
    print("All Contracts: unavailable")
