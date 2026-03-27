#!/usr/bin/env python3
"""
04_seed_amm_pool.py — Seed the LICN/lUSD AMM concentrated liquidity pool.

Deposits 5M LICN + 500K lUSD into pool 1 (LICN/lUSD) with a tick range
covering $0.05–$0.25, as specified in DEX_LIQUIDITY_STRATEGY.md Phase 1.

The dex_amm contract uses opcode dispatch (single `call()` export).
Opcode 3 = add_liquidity, with layout:
  [0]      opcode      u8      = 0x03
  [1:33]   provider    [u8;32] = Ed25519 pubkey (tx signer)
  [33:41]  pool_id     u64 LE
  [41:45]  lower_tick  i32 LE
  [45:49]  upper_tick  i32 LE
  [49:57]  amount_a    u64 LE  (LICN in spores)
  [57:65]  amount_b    u64 LE  (lUSD in spores)

Usage:
  python3 04_seed_amm_pool.py --rpc http://15.204.229.189:8899 --network testnet
  python3 04_seed_amm_pool.py --rpc http://15.204.229.189:8899 --network testnet --dry-run
"""

import argparse
import asyncio
import json
import math
import struct
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))

from lichen import Connection, Instruction, Keypair, PublicKey, TransactionBuilder

CONTRACT_PROGRAM = PublicKey(b"\xff" * 32)
SPORES_PER_LICN = 1_000_000_000
TX_CONFIRM_TIMEOUT = 20

# Pool 1 = LICN/lUSD (created at genesis, 30bps fee tier, tick_spacing=60)
LICN_LUSD_POOL_ID = 1

# Tick range for $0.05–$0.25 (broad range for early volatility)
# tick = ln(price) / ln(1.0001)
# $0.05 → tick ≈ -29,957 → rounded to 60: -29,940
# $0.25 → tick ≈ -13,863 → rounded to 60: -13,860
LOWER_TICK = -29940
UPPER_TICK = -13860

# Amounts (from strategy doc)
LICN_AMOUNT = 5_000_000   # 5M LICN
LUSD_AMOUNT = 500_000     # 500K lUSD


# ═══════════════════════════════════════════════════════════════════════════════
# Helpers — shared with other scripts in this directory
# ═══════════════════════════════════════════════════════════════════════════════

def load_keypair(path: Path) -> Keypair:
    raw = json.loads(path.read_text(encoding="utf-8"))
    if "secret_key" in raw:
        return Keypair.from_seed(bytes.fromhex(raw["secret_key"]))
    return Keypair.load(path)


def find_keypair(network: str, role: str) -> Path:
    for base in [
        ROOT / "artifacts" / network / "genesis-keys",
        ROOT / "data" / f"state-{network}" / "genesis-keys",
    ]:
        if base.exists():
            for f in base.glob("*.json"):
                if f.name.startswith(role):
                    return f
    raise FileNotFoundError(f"No keypair for '{role}' on {network}")


async def resolve_contract(conn: Connection, symbol: str) -> PublicKey:
    result = await conn._rpc("getAllSymbolRegistry", [100])
    entries = result.get("entries", []) if isinstance(result, dict) else result
    for entry in entries:
        if entry.get("symbol") == symbol:
            addr = entry.get("program", "")
            if addr:
                return PublicKey.from_base58(addr)
    raise ValueError(f"Contract '{symbol}' not found in registry")


async def wait_for_tx(conn, sig, timeout=TX_CONFIRM_TIMEOUT):
    for _ in range(timeout * 5):
        await asyncio.sleep(0.2)
        try:
            info = await conn.get_transaction(sig)
            if info:
                return info
        except Exception:
            pass
    return None


async def approve_token(
    conn: Connection,
    signer: Keypair,
    token_addr: PublicKey,
    spender: PublicKey,
    amount: int,
) -> str:
    """Approve spender to transfer tokens on behalf of the signer.
    Args layout: [owner 32B][spender 32B][amount 8B] = 72 bytes.
    """
    args_bytes = (
        list(signer.public_key().to_bytes())
        + list(spender.to_bytes())
        + list(struct.pack("<Q", amount))
    )
    envelope = json.dumps({
        "Call": {
            "function": "approve",
            "args": args_bytes,
            "value": 0,
        }
    })
    ix = Instruction(
        CONTRACT_PROGRAM,
        [signer.public_key(), token_addr],
        envelope.encode("utf-8"),
    )
    tb = TransactionBuilder()
    tb.add(ix)
    latest = await conn.get_latest_block()
    blockhash = latest.get("hash", latest.get("blockhash", "0" * 64))
    tb.set_recent_blockhash(blockhash)
    tx = tb.build_and_sign(signer)
    return await conn.send_transaction(tx)


def build_add_liquidity(
    provider_pubkey: bytes,
    pool_id: int,
    lower_tick: int,
    upper_tick: int,
    amount_a: int,
    amount_b: int,
) -> bytes:
    """Build binary instruction for dex_amm.add_liquidity (opcode 0x03).

    Layout (65 bytes):
      [0]      opcode = 0x03
      [1:33]   provider pubkey (32 bytes)
      [33:41]  pool_id (u64 LE)
      [41:45]  lower_tick (i32 LE)
      [45:49]  upper_tick (i32 LE)
      [49:57]  amount_a (u64 LE)
      [57:65]  amount_b (u64 LE)
    """
    buf = bytearray(65)
    buf[0] = 0x03  # add_liquidity opcode
    buf[1:33] = provider_pubkey[:32]
    struct.pack_into("<Q", buf, 33, pool_id)
    struct.pack_into("<i", buf, 41, lower_tick)
    struct.pack_into("<i", buf, 45, upper_tick)
    struct.pack_into("<Q", buf, 49, amount_a)
    struct.pack_into("<Q", buf, 57, amount_b)
    return bytes(buf)


# ═══════════════════════════════════════════════════════════════════════════════
# Main
# ═══════════════════════════════════════════════════════════════════════════════

async def main():
    parser = argparse.ArgumentParser(description="Seed LICN/lUSD AMM pool")
    parser.add_argument("--rpc", default="http://127.0.0.1:8899")
    parser.add_argument("--network", default="testnet")
    parser.add_argument("--reserve-key", default=None)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    conn = Connection(args.rpc)
    print("═" * 70)
    print("   04 — Seed LICN/lUSD AMM Concentrated Liquidity Pool")
    print("═" * 70)
    print(f"  RPC:       {args.rpc}")
    print(f"  Network:   {args.network}")

    # ── Load reserve_pool keypair ──
    reserve_path = Path(args.reserve_key) if args.reserve_key else find_keypair(args.network, "reserve")
    reserve_kp = load_keypair(reserve_path)
    print(f"  Provider:  {reserve_kp.public_key()}")

    # ── Resolve contract addresses ──
    dex_amm_addr = await resolve_contract(conn, "DEXAMM")
    licn_addr = await resolve_contract(conn, "LICN")
    lusd_addr = await resolve_contract(conn, "LUSD")
    print(f"  dex_amm:   {dex_amm_addr}")
    print(f"  lichencoin:{licn_addr}")
    print(f"  lusd_token:{lusd_addr}")

    # ── Tick calculations ──
    lower_price = 1.0001 ** LOWER_TICK
    upper_price = 1.0001 ** UPPER_TICK
    print()
    print(f"  Pool ID:   {LICN_LUSD_POOL_ID}")
    print(f"  Tick range: {LOWER_TICK} to {UPPER_TICK}")
    print(f"  Price range: ${lower_price:.6f} to ${upper_price:.6f}")
    print(f"  Amount A (LICN):  {LICN_AMOUNT:>12,} LICN ({LICN_AMOUNT * SPORES_PER_LICN:,} spores)")
    print(f"  Amount B (lUSD):  {LUSD_AMOUNT:>12,} lUSD ({LUSD_AMOUNT * SPORES_PER_LICN:,} spores)")

    if args.dry_run:
        print("\n  [DRY RUN] Would add liquidity. Exiting.")
        return

    # ── Check token balances ──
    # Storage keys use hex-encoded pubkey bytes: licn_bal_{hex(pubkey)}
    reserve_hex = reserve_kp.public_key().to_bytes().hex()

    licn_storage = await conn._rpc("getProgramStorage", [str(licn_addr), {"limit": 500}])
    licn_entries = licn_storage.get("entries", []) if isinstance(licn_storage, dict) else []
    licn_bal = 0
    bal_suffix = f"_bal_{reserve_hex}"
    print(f"  DEBUG: licn_storage type={type(licn_storage).__name__}, entries={len(licn_entries)}, suffix={bal_suffix[:30]}...")
    for e in licn_entries:
        kd = e.get("key_decoded", "")
        if "bal_" in kd and reserve_hex[:8] in kd:
            val_hex = e.get("value", e.get("value_decoded", ""))
            print(f"  DEBUG: matched key [{kd[:60]}] val=[{val_hex}]")
            if isinstance(val_hex, str) and val_hex:
                licn_bal = int.from_bytes(bytes.fromhex(val_hex), "little")
            elif isinstance(val_hex, list):
                licn_bal = int.from_bytes(bytes(val_hex[:8]), "little")
    print(f"\n  LICN token balance: {licn_bal / SPORES_PER_LICN:,.2f}")

    lusd_storage = await conn._rpc("getProgramStorage", [str(lusd_addr), {"limit": 500}])
    lusd_entries = lusd_storage.get("entries", []) if isinstance(lusd_storage, dict) else []
    lusd_bal = 0
    for e in lusd_entries:
        kd = e.get("key_decoded", "")
        if kd.endswith(bal_suffix) or kd == f"musd_bal_{reserve_hex}":
            val_hex = e.get("value", e.get("value_decoded", ""))
            if isinstance(val_hex, str) and val_hex:
                lusd_bal = int.from_bytes(bytes.fromhex(val_hex), "little")
            elif isinstance(val_hex, list):
                lusd_bal = int.from_bytes(bytes(val_hex[:8]), "little")
    print(f"  lUSD token balance: {lusd_bal / SPORES_PER_LICN:,.2f}")

    licn_needed = LICN_AMOUNT * SPORES_PER_LICN
    lusd_needed = LUSD_AMOUNT * SPORES_PER_LICN

    if licn_bal < licn_needed:
        print(f"\n  ❌ Insufficient LICN tokens: have {licn_bal / SPORES_PER_LICN:,.0f}, need {LICN_AMOUNT:,}")
        print(f"     Run mint_licn_tokens.py first to mint more LICN tokens.")
        return
    if lusd_bal < lusd_needed:
        print(f"\n  ❌ Insufficient lUSD tokens: have {lusd_bal / SPORES_PER_LICN:,.0f}, need {LUSD_AMOUNT:,}")
        print(f"     Run 01_mint_lusd.py first to mint more lUSD.")
        return

    # ── Approve dex_amm to spend both tokens ──
    approve_amount = 100_000_000 * SPORES_PER_LICN  # generous approval

    print(f"\n  Approving dex_amm to spend LICN...")
    try:
        sig = await approve_token(conn, reserve_kp, licn_addr, dex_amm_addr, approve_amount)
        result = await wait_for_tx(conn, sig)
        if result:
            print(f"  ✅ LICN approval confirmed: {sig[:16]}...")
        else:
            print(f"  ⏳ LICN approval sent: {sig[:16]}...")
        await asyncio.sleep(0.5)
    except Exception as e:
        print(f"  ❌ LICN approval failed: {e}")
        return

    print(f"  Approving dex_amm to spend lUSD...")
    try:
        sig = await approve_token(conn, reserve_kp, lusd_addr, dex_amm_addr, approve_amount)
        result = await wait_for_tx(conn, sig)
        if result:
            print(f"  ✅ lUSD approval confirmed: {sig[:16]}...")
        else:
            print(f"  ⏳ lUSD approval sent: {sig[:16]}...")
        await asyncio.sleep(0.5)
    except Exception as e:
        print(f"  ❌ lUSD approval failed: {e}")
        return

    # ── Add liquidity ──
    print(f"\n  Adding liquidity to pool {LICN_LUSD_POOL_ID}...")
    order_bytes = build_add_liquidity(
        provider_pubkey=reserve_kp.public_key().to_bytes(),
        pool_id=LICN_LUSD_POOL_ID,
        lower_tick=LOWER_TICK,
        upper_tick=UPPER_TICK,
        amount_a=licn_needed,
        amount_b=lusd_needed,
    )

    envelope = json.dumps({
        "Call": {
            "function": "call",
            "args": list(order_bytes),
            "value": 0,
        }
    })
    ix = Instruction(
        CONTRACT_PROGRAM,
        [reserve_kp.public_key(), dex_amm_addr],
        envelope.encode("utf-8"),
    )
    tb = TransactionBuilder()
    tb.add(ix)
    latest = await conn.get_latest_block()
    blockhash = latest.get("hash", latest.get("blockhash", "0" * 64))
    tb.set_recent_blockhash(blockhash)
    tx = tb.build_and_sign(reserve_kp)

    try:
        sig = await conn.send_transaction(tx)
        print(f"  TX sent: {sig[:32]}...")
    except Exception as e:
        print(f"  ❌ Send failed: {e}")
        return

    result = await wait_for_tx(conn, sig)
    if result:
        rc = result.get("return_code") if isinstance(result, dict) else None
        err = result.get("error") if isinstance(result, dict) else None
        if err or (rc is not None and rc != 0):
            rc_map = {
                1: "Contract paused",
                2: "Pool not found",
                3: "Invalid tick range",
                4: "Below minimum liquidity",
                5: "Reentrancy guard",
                200: "Caller mismatch",
            }
            reason = rc_map.get(rc, f"rc={rc}")
            print(f"  ❌ add_liquidity failed: {err or reason}")
        else:
            print(f"  ✅ Liquidity added successfully!")
            print(f"     TX:     {sig}")
            print(f"     Pool:   {LICN_LUSD_POOL_ID} (LICN/lUSD)")
            print(f"     LICN:   {LICN_AMOUNT:,}")
            print(f"     lUSD:   {LUSD_AMOUNT:,}")
            print(f"     Ticks:  {LOWER_TICK} → {UPPER_TICK}")
    else:
        print(f"  ⏳ TX sent but unconfirmed after {TX_CONFIRM_TIMEOUT}s: {sig[:32]}...")

    print()
    print("═" * 70)
    print("   AMM pool seeding complete")
    print("═" * 70)


if __name__ == "__main__":
    asyncio.run(main())
