#!/usr/bin/env python3
"""Call DEX contract read-only to see return code."""
import sys, json, struct, base64
from pathlib import Path
import urllib.request

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))
from lichen import Keypair

# Load reserve_pool address
kp = Keypair.load(ROOT / "artifacts/testnet/genesis-keys/reserve_pool-lichen-testnet-1.json")
pub_bytes = kp.address().to_bytes()
print(f"Reserve pool: {kp.address()}")

# Build place_order args (opcode 2)
buf = bytearray(75)
buf[0] = 0x02
buf[1:33] = pub_bytes[:32]
struct.pack_into("<Q", buf, 33, 1)  # pair_id=1
buf[41] = 1  # side=sell
buf[42] = 0  # type=limit
struct.pack_into("<Q", buf, 43, 100_000_000)  # price=0.1
struct.pack_into("<Q", buf, 51, 1_000_000_000)  # qty=1 LICN
struct.pack_into("<Q", buf, 59, 0)
struct.pack_into("<Q", buf, 67, 0)
args_b64 = base64.b64encode(bytes(buf)).decode()

# Try callContract (array form)
DEX = "6KLSjabyfRXE87VzTFGmWq9wHoY9izEoUfovPNeFmf9u"
payload = json.dumps({
    "jsonrpc": "2.0",
    "id": 1,
    "method": "callContract",
    "params": [DEX, "call", args_b64],
}).encode()

req = urllib.request.Request(
    "http://15.204.229.189:8899",
    data=payload,
    headers={"Content-Type": "application/json"},
)
with urllib.request.urlopen(req, timeout=30) as resp:
    data = json.loads(resp.read())
    print(json.dumps(data, indent=2))
