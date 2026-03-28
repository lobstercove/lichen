#!/usr/bin/env python3
"""Place a single LICN/lUSD buy order and check actual return code."""
import sys, os, struct, asyncio, base64
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw

SPORES = 1_000_000_000

async def main():
    conn = Connection('http://127.0.0.1:8899')
    keys = Path('data/state-testnet/genesis-keys')
    reserve = Keypair.load(keys / 'reserve_pool-lichen-testnet-1.json')
    reserve_bytes = bytes(reserve.public_key().to_bytes())
    reserve_str = str(reserve.public_key())

    result = await conn._rpc("getAllSymbolRegistry")
    dex_addr = lusd_addr = None
    for e in result.get("entries", []):
        sym = e.get("symbol", "")
        prog = e.get("program", "")
        if sym == "DEX": dex_addr = prog
        elif sym == "LUSD": lusd_addr = prog
    
    dex_pk = PublicKey.from_base58(dex_addr)
    lusd_pk = PublicKey.from_base58(lusd_addr)

    print(f"Reserve: {reserve_str}")
    print(f"DEX:     {dex_addr}")
    print(f"lUSD:    {lusd_addr}")

    # Check lUSD balance before
    args_b64 = base64.b64encode(reserve_bytes).decode()
    r = await conn._rpc('callContract', [lusd_addr, 'balance_of', args_b64, reserve_str])
    bal_before = r.get('returnCode', 0)
    print(f"\nlUSD balance before: {bal_before / SPORES:,.3f}")

    # Step 1: Re-approve just to be sure (100M lUSD)
    print("\nStep 1: Re-approving dex_core to spend lUSD...")
    owner_bytes = reserve_bytes
    spender_bytes = bytes(dex_pk.to_bytes())
    approve_amount = 100_000_000 * SPORES
    approve_args = list(owner_bytes + spender_bytes + struct.pack('<Q', approve_amount))
    sig = await call_contract_raw(conn, reserve, lusd_pk, 'approve', approve_args)
    print(f"  Approve tx: {sig}")
    await asyncio.sleep(2)

    # Verify allowance
    allowance_args_b64 = base64.b64encode(reserve_bytes + spender_bytes).decode()
    r = await conn._rpc('callContract', [lusd_addr, 'allowance', allowance_args_b64, reserve_str])
    print(f"  Allowance: {r.get('returnCode', 0) / SPORES:,.3f} lUSD")

    # Step 2: Place a SINGLE buy order: 1,000 LICN @ $0.098
    print("\nStep 2: Placing BUY 1,000 LICN @ $0.098 on LICN/lUSD...")
    price = int(0.098 * SPORES)
    qty = 1_000 * SPORES
    order_args = (
        bytes([2]) +                  # opcode 2
        reserve_bytes +               # trader
        struct.pack('<Q', 1) +        # pair_id
        bytes([0]) +                  # side = BUY
        bytes([0]) +                  # order_type = LIMIT
        struct.pack('<Q', price) +    # price
        struct.pack('<Q', qty) +      # quantity
        struct.pack('<Q', 2592000)    # expiry
    )
    
    try:
        sig = await call_contract_raw(conn, reserve, dex_pk, 'call', list(order_args))
        print(f"  Order tx sent: {sig}")
    except Exception as e:
        print(f"  Order tx failed: {e}")
        return

    await asyncio.sleep(2)

    # Step 3: Check if the transaction succeeded by looking at the tx
    try:
        tx_info = await conn.get_transaction(sig)
        print(f"\n  Transaction info: {tx_info}")
    except Exception as e:
        print(f"  Could not fetch tx: {e}")

    # Step 4: Check lUSD balance after
    r = await conn._rpc('callContract', [lusd_addr, 'balance_of', args_b64, reserve_str])
    bal_after = r.get('returnCode', 0)
    diff = bal_before - bal_after
    print(f"\nlUSD balance after:  {bal_after / SPORES:,.3f}")
    print(f"lUSD consumed:       {diff / SPORES:,.3f}")
    if diff > 0:
        print(f"  -> Order consumed lUSD, meaning escrow WORKED!")
    else:
        print(f"  -> No lUSD consumed, meaning escrow FAILED in the contract")

    # Step 5: Check if order appears on book
    # Get user order count
    uo_args = bytes([11]) + reserve_bytes
    args_b64_uo = base64.b64encode(uo_args).decode()
    r = await conn._rpc('callContract', [dex_addr, 'call', args_b64_uo, reserve_str])
    print(f"\nUser orders query: returnCode={r.get('returnCode',0)}, returnData={r.get('returnData','')}")

asyncio.run(main())
