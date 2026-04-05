#!/usr/bin/env python3
import glob
import json
import os
import sys
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "sdk", "python"))

from lichen import Keypair  # type: ignore

RPC_URL = os.getenv("RPC_URL", "http://127.0.0.1:8899")
RPC_ENDPOINTS = [
    endpoint.strip()
    for endpoint in os.getenv("RPC_ENDPOINTS", f"{RPC_URL},http://127.0.0.1:8901,http://127.0.0.1:8903").split(",")
    if endpoint.strip()
]
MIN_SPORES = int(os.getenv("MIN_FUNDED_SPORES", "1000000000"))


def rpc(endpoint: str, method: str, params=None):
    payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or []}).encode()
    req = urllib.request.Request(endpoint, data=payload, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=5) as resp:
        out = json.loads(resp.read())
    if "error" in out:
        raise RuntimeError(out["error"].get("message", "RPC error"))
    return out.get("result")


def get_balance_spores(pubkey: str) -> int:
    for endpoint in RPC_ENDPOINTS:
        try:
            bal = rpc(endpoint, "getBalance", [pubkey])
            return int((bal or {}).get("spendable", (bal or {}).get("spores", 0)))
        except Exception:
            continue
    return 0


def priority(path):
    name = os.path.basename(path)
    if name.startswith("builder_grants"):
        return 0
    if name.startswith("community_treasury"):
        return 1
    if name.startswith("ecosystem_partnerships"):
        return 2
    if name.startswith("reserve_pool"):
        return 3
    if name.startswith("validator_rewards"):
        return 4
    if name.startswith("founding_symbionts"):
        return 5
    if name.startswith("treasury"):
        return 6
    if name.startswith("genesis-primary"):
        return 7
    if name.startswith("genesis-signer"):
        return 8
    return 9


def extract_pubkey(raw, path):
    for key in ("pubkey", "address", "publicKeyBase58", "address_base58"):
        value = raw.get(key)
        if isinstance(value, str) and value:
            return value

    try:
        return Keypair.load(path).address().to_base58()
    except Exception:
        return ""


def main():
    genesis_pattern = os.path.join(ROOT, "data", "**", "genesis-keys", "*.json")
    genesis_db_pattern = os.path.join(ROOT, "data", "**", "blockchain.db", "genesis-keys", "*.json")
    artifacts_pattern = os.path.join(ROOT, "artifacts", "testnet", "genesis-keys", "*.json")
    keypairs_pattern = os.path.join(ROOT, "keypairs", "*.json")
    files = sorted(
        set(
            glob.glob(genesis_pattern, recursive=True)
            + glob.glob(genesis_db_pattern, recursive=True)
            + glob.glob(artifacts_pattern)
            + glob.glob(keypairs_pattern, recursive=True)
        ),
        key=lambda p: (priority(p), p),
    )
    candidates = []
    seen = set()

    for path in files:
        try:
            raw = json.loads(open(path, "r", encoding="utf-8").read())
        except Exception:
            continue
        pubkey = extract_pubkey(raw, path)
        if not pubkey:
            continue
        if pubkey in seen:
            continue
        seen.add(pubkey)
        spores = get_balance_spores(pubkey)
        if spores >= MIN_SPORES:
            candidates.append({"path": path, "pubkey": pubkey, "spores": spores})

    candidates.sort(key=lambda c: (priority(c["path"]), -c["spores"], c["path"]))
    agent = candidates[0] if candidates else None
    human = candidates[1] if len(candidates) > 1 else agent

    print(json.dumps({
        "rpc": RPC_URL,
        "rpc_endpoints": RPC_ENDPOINTS,
        "min_spores": MIN_SPORES,
        "count": len(candidates),
        "agent": agent,
        "human": human,
    }))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
