#!/usr/bin/env python3
"""
fund-deployer.py — Verify and report deployer account funding status.

Called by reset-blockchain.sh after genesis to confirm the deployer
(genesis wallet) is properly funded on-chain before E2E tests run.

The deployer IS the genesis wallet: it is auto-funded with 10,000 MOLT
at genesis via the validator_rewards distribution pool.  This script
verifies that funding happened and optionally tops up via the faucet.

Usage:
    python3 scripts/fund-deployer.py [--rpc http://127.0.0.1:8899]
"""

import json
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

RPC_URL = "http://127.0.0.1:8899"
DEPLOYER_PATH = ROOT / "keypairs" / "deployer.json"
MIN_BALANCE_SHELLS = 1_000_000_000  # 1 MOLT minimum

# Parse args
for i, arg in enumerate(sys.argv[1:]):
    if arg == "--rpc" and i + 2 <= len(sys.argv) - 1:
        RPC_URL = sys.argv[i + 2]


def rpc(method: str, params: list = []) -> dict:
    payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(
        RPC_URL,
        data=payload,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        return json.loads(resp.read())


def main() -> int:
    # Load deployer keypair
    if not DEPLOYER_PATH.exists():
        print(f"  ✗ Deployer keypair not found at {DEPLOYER_PATH}")
        print("    Run reset-blockchain.sh --restart to regenerate.")
        return 1

    try:
        kp_data = json.loads(DEPLOYER_PATH.read_text())
        pubkey = kp_data.get("pubkey")
        if not pubkey:
            print("  ✗ Invalid deployer.json: missing 'pubkey' field")
            return 1
    except Exception as e:
        print(f"  ✗ Failed to read deployer.json: {e}")
        return 1

    print(f"  Deployer pubkey: {pubkey}")

    # Retry up to 5 times (state may still be settling after genesis)
    for attempt in range(1, 6):
        try:
            result = rpc("getBalance", [pubkey])
            bal = result.get("result", {})
            if isinstance(bal, dict):
                shells = int(bal.get("shells", bal.get("balance", 0)))
            else:
                shells = int(bal or 0)

            molt = shells / 1_000_000_000
            print(f"  Deployer balance: {shells:,} shells ({molt:.4f} MOLT)")

            if shells >= MIN_BALANCE_SHELLS:
                print(f"  ✓ Deployer is funded ({molt:.4f} ≥ 1.0 MOLT)")
                return 0
            else:
                print(f"  ⚠  Balance too low: {shells:,} < {MIN_BALANCE_SHELLS:,} shells")
                if attempt < 5:
                    print(f"     Waiting 3s for genesis state to settle (attempt {attempt}/5)...")
                    time.sleep(3)
        except Exception as e:
            print(f"  ⚠  RPC error (attempt {attempt}/5): {e}")
            if attempt < 5:
                time.sleep(3)

    print("  ✗ Deployer account not funded after retries.")
    print("    The genesis wallet should be auto-funded with 10,000 MOLT.")
    print("    Check validator logs: tail -f /tmp/moltchain-v1.log")
    return 1


if __name__ == "__main__":
    sys.exit(main())
