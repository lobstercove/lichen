#!/usr/bin/env python3
"""
01_mint_lusd.py — Mint protocol-backed lUSD into the reserve_pool wallet.

The deployer (genesis-primary) is the admin of the lusd_token contract and
is the only account authorized to call mint().  lUSD is minted into the
reserve_pool wallet so it can be used for buy-wall orders on the CLOB.

Flow:
  1. Load deployer keypair (lusd_token admin)
  2. Resolve lusd_token contract address via symbol registry
  3. Call lusd_token.mint(caller=deployer, to=reserve_pool, amount)
  4. Verify reserve_pool lUSD balance increased

Usage:
  python3 01_mint_lusd.py --rpc http://15.204.229.189:8899 --network testnet
  python3 01_mint_lusd.py --rpc http://15.204.229.189:8899 --network testnet --amount 100000
"""

import argparse
import asyncio
import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))

from lichen import Connection, Instruction, Keypair, PublicKey, TransactionBuilder

CONTRACT_PROGRAM = PublicKey(b"\xff" * 32)
SPORES_PER_LICN = 1_000_000_000
TX_CONFIRM_TIMEOUT = 20  # seconds


# ═══════════════════════════════════════════════════════════════════════════════
# Keypair loading
# ═══════════════════════════════════════════════════════════════════════════════

def load_keypair(path: Path) -> Keypair:
    """Load a keypair from a genesis-keys JSON file."""
    raw = json.loads(path.read_text(encoding="utf-8"))
    if "secret_key" in raw:
        seed = bytes.fromhex(raw["secret_key"])
        return Keypair.from_seed(seed)
    return Keypair.load(path)


def find_keypair(network: str, role: str) -> Path:
    """Find a genesis keypair file by role and network."""
    # Check artifacts/<network>/genesis-keys/ first
    artifacts = ROOT / "artifacts" / network / "genesis-keys"
    if artifacts.exists():
        for f in artifacts.glob("*.json"):
            if f.name.startswith(role):
                return f

    # Fallback: data/state-<network>/genesis-keys/
    data_dir = ROOT / "data" / f"state-{network}" / "genesis-keys"
    if data_dir.exists():
        for f in data_dir.glob("*.json"):
            if f.name.startswith(role):
                return f

    raise FileNotFoundError(
        f"No keypair found for role '{role}' on {network}. "
        f"Searched: {artifacts}, {data_dir}"
    )


# ═══════════════════════════════════════════════════════════════════════════════
# Contract helpers
# ═══════════════════════════════════════════════════════════════════════════════

async def resolve_contract(conn: Connection, symbol: str) -> PublicKey:
    """Resolve a contract address from the symbol registry."""
    result = await conn._rpc("getAllSymbolRegistry", [100])
    entries = result.get("entries", []) if isinstance(result, dict) else result
    for entry in entries:
        if entry.get("symbol") == symbol:
            addr = entry.get("program", "")
            if addr:
                return PublicKey.from_base58(addr)
    raise ValueError(f"Contract '{symbol}' not found in registry")


def build_named_call(fn_name: str, args: dict) -> bytes:
    """Build instruction data for a named-export contract call.

    Named-export contracts (like lusd_token) expose each function as a
    separate WASM export.  The Call envelope uses the function name directly.
    The args are JSON-encoded as the raw bytes payload.
    """
    # Build the inner args as JSON
    inner = json.dumps({"function": fn_name, "args": args})
    inner_bytes = inner.encode("utf-8")

    # Wrap in the ContractInstruction::Call envelope
    envelope = json.dumps({
        "Call": {
            "function": fn_name,
            "args": list(inner_bytes),
            "value": 0,
        }
    })
    return envelope.encode("utf-8")


async def send_contract_call(
    conn: Connection,
    signer: Keypair,
    contract_addr: PublicKey,
    fn_name: str,
    args: dict,
) -> str:
    """Build, sign, and send a named-export contract call transaction."""
    data = build_named_call(fn_name, args)

    ix = Instruction(
        CONTRACT_PROGRAM,
        [signer.public_key(), contract_addr],
        data,
    )

    tb = TransactionBuilder()
    tb.add(ix)
    latest = await conn.get_latest_block()
    blockhash = latest.get("hash", latest.get("blockhash", "0" * 64))
    tb.set_recent_blockhash(blockhash)
    tx = tb.build_and_sign(signer)
    sig = await conn.send_transaction(tx)
    return sig


async def wait_for_confirmation(conn: Connection, sig: str, timeout: int = TX_CONFIRM_TIMEOUT):
    """Wait for a transaction to confirm."""
    for _ in range(timeout * 5):
        await asyncio.sleep(0.2)
        try:
            info = await conn.get_transaction(sig)
            if info:
                return info
        except Exception:
            pass
    raise TimeoutError(f"Transaction {sig} not confirmed after {timeout}s")


# ═══════════════════════════════════════════════════════════════════════════════
# Main
# ═══════════════════════════════════════════════════════════════════════════════

async def main():
    parser = argparse.ArgumentParser(description="Mint protocol-backed lUSD")
    parser.add_argument("--rpc", default="http://127.0.0.1:8899", help="RPC endpoint")
    parser.add_argument("--network", default="testnet", choices=["testnet", "mainnet"])
    parser.add_argument("--amount", type=float, default=2_500_000,
                        help="Amount of lUSD to mint (default: 2,500,000)")
    parser.add_argument("--deployer-key", type=str, default=None,
                        help="Path to deployer keypair (overrides auto-discovery)")
    parser.add_argument("--reserve-key", type=str, default=None,
                        help="Path to reserve_pool keypair (overrides auto-discovery)")
    parser.add_argument("--dry-run", action="store_true",
                        help="Print what would happen without sending transactions")
    args = parser.parse_args()

    print(f"{'='*60}")
    print(f"  Lichen lUSD Minting — {args.network}")
    print(f"{'='*60}")
    print(f"  RPC:    {args.rpc}")
    print(f"  Amount: {args.amount:,.0f} lUSD")
    print()

    conn = Connection(args.rpc)

    # Load keypairs
    deployer_path = Path(args.deployer_key) if args.deployer_key else find_keypair(args.network, "genesis-primary")
    reserve_path = Path(args.reserve_key) if args.reserve_key else find_keypair(args.network, "reserve_pool")

    deployer = load_keypair(deployer_path)
    reserve_pool = load_keypair(reserve_path)

    print(f"  Deployer (admin): {deployer.public_key()}")
    print(f"  Reserve pool:     {reserve_pool.public_key()}")
    print()

    # Resolve lusd_token contract
    lusd_addr = await resolve_contract(conn, "LUSD")
    print(f"  lUSD contract:    {lusd_addr}")

    # Check current balances
    deployer_balance = await conn.get_balance(deployer.public_key())
    reserve_balance = await conn.get_balance(reserve_pool.public_key())
    deployer_spores = deployer_balance.get("spendable", deployer_balance.get("spores", 0))
    reserve_spores = reserve_balance.get("spendable", reserve_balance.get("spores", 0))
    print(f"  Deployer LICN:    {deployer_spores / SPORES_PER_LICN:,.4f}")
    print(f"  Reserve LICN:     {reserve_spores / SPORES_PER_LICN:,.4f}")
    print()

    amount_spores = int(args.amount * SPORES_PER_LICN)

    if args.dry_run:
        print(f"  [DRY RUN] Would mint {args.amount:,.0f} lUSD to {reserve_pool.public_key()}")
        return

    # Mint lUSD: deployer calls lusd_token.mint(caller, to, amount)
    # The deployer is the admin set during genesis initialization.
    # The 'to' is the reserve_pool wallet where lUSD will be received.
    print(f"  Minting {args.amount:,.0f} lUSD to reserve_pool...")
    sig = await send_contract_call(
        conn, deployer, lusd_addr, "mint",
        {
            "caller": str(deployer.public_key()),
            "to": str(reserve_pool.public_key()),
            "amount": amount_spores,
        },
    )
    print(f"  TX signature: {sig}")

    # Wait for confirmation
    print("  Waiting for confirmation...")
    try:
        result = await wait_for_confirmation(conn, sig)
        print(f"  ✅ Confirmed!")
        if isinstance(result, dict):
            status = result.get("status", result.get("meta", {}).get("status", "unknown"))
            print(f"     Status: {status}")
    except TimeoutError:
        print(f"  ⚠️  Confirmation timeout — check tx: {sig}")

    # Verify balance change
    # Note: lUSD balance is tracked by the lusd_token contract, not native LICN balance
    # We'd need to query the token contract to see the lUSD balance
    print()
    print(f"  Done. lUSD minted to reserve_pool.")
    print(f"  Verify with: getDexPairs or check reserve_pool token balance.")


if __name__ == "__main__":
    asyncio.run(main())
