#!/usr/bin/env python3
"""Mint protocol-backing lUSD (and testnet wrapped tokens) to reserve_pool.

Per DEX_LIQUIDITY_STRATEGY.md §3.1:
  Genesis keypair (admin of all token contracts) mints lUSD 1:1 against
  protocol LICN reserves.  Token contracts use AUDIT-FIX caller verification
  (get_caller() must match admin), so the genesis identity keypair is required.

  Amounts are batched to respect per-epoch mint caps:
    lUSD  = 100M/epoch   (MINT_CAP_PER_EPOCH in lusd_token)
    wSOL  = 500K/epoch   (wsol_token)
    wETH  = 50K/epoch    (weth_token)
    wBNB  = 100K/epoch   (wbnb_token)

On testnet, also mints test amounts of wSOL, wETH, wBNB so all 7 AMM pools
can be seeded with liquidity.

Usage:
  python tools/mint_protocol_lusd.py                       # testnet, localhost
  LICHEN_RPC_URL=http://host:8899 python tools/mint_protocol_lusd.py
"""
import sys, os, struct, asyncio, time
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw

SPORES = 1_000_000_000  # 1 token = 1B spores
RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')

# Per-epoch mint caps (from contract source, in whole tokens)
EPOCH_CAPS = {
    "LUSD": 100_000_000,   # 100M lUSD per epoch
    "WSOL": 500_000,       # 500K wSOL per epoch
    "WETH": 50_000,        # 50K wETH per epoch
    "WBNB": 100_000,       # 100K wBNB per epoch
}

# Amounts to mint (whole tokens) — will be batched per-epoch if needed
MINT_AMOUNTS = {
    "LUSD": 2_500_000,     # $2.5M backing for CLOB buy-wall + AMM
    "WSOL": 10_000,        # testnet: ~$1.5M at ~$150/SOL
    "WETH": 500,           # testnet: ~$1M at ~$2000/ETH
    "WBNB": 5_000,         # testnet: ~$3M at ~$610/BNB
}


async def main():
    conn = Connection(RPC)
    repo = Path(__file__).resolve().parent.parent

    # ── Load genesis keypair (admin of all token contracts) ──
    # The genesis/validator identity keypair is the admin set during genesis
    # initialization of all contracts. NOT the deployer keypair.
    genesis_kp_path = repo / f"data/state-{NETWORK}/validator-keypair.json"
    if not genesis_kp_path.exists():
        # Fallback: try shared keypairs directory
        genesis_kp_path = repo / "keypairs" / "validator-identity.json"
    if not genesis_kp_path.exists():
        print(f"ERROR: genesis/validator keypair not found")
        print(f"  Tried: {repo / f'data/state-{NETWORK}/validator-keypair.json'}")
        print(f"  Tried: {repo / 'keypairs/validator-identity.json'}")
        sys.exit(1)
    admin = Keypair.load(genesis_kp_path)
    admin_bytes = bytes(admin.public_key().to_bytes())
    print(f"  Admin (genesis): {admin.public_key()}")

    # ── Load reserve_pool keypair ──
    rp_path = repo / f"data/state-{NETWORK}/genesis-keys/reserve_pool-lichen-{NETWORK}-1.json"
    if not rp_path.exists():
        print(f"ERROR: reserve_pool keypair not found at {rp_path}")
        sys.exit(1)
    reserve_kp = Keypair.load(rp_path)
    reserve_bytes = bytes(reserve_kp.public_key().to_bytes())
    print(f"  Reserve pool:  {reserve_kp.public_key()}")

    # ── Discover token contracts from symbol registry ──
    result = await conn._rpc("getAllSymbolRegistry")
    entries = result.get("entries", [])
    tokens = {}
    for e in entries:
        sym = e.get("symbol", "")
        prog = e.get("program", "")
        if sym in MINT_AMOUNTS and prog:
            tokens[sym] = PublicKey.from_base58(prog)

    print(f"\n  Token contracts found: {len(tokens)}")
    for sym, pk in sorted(tokens.items()):
        print(f"    {sym}: {pk}")
    print()

    # ── Mint tokens to reserve_pool ──
    minted = 0
    for sym, whole_amount in MINT_AMOUNTS.items():
        if sym not in tokens:
            print(f"  SKIP {sym}: contract not found on-chain")
            continue

        cap = EPOCH_CAPS.get(sym, whole_amount)
        remaining = whole_amount
        batch_num = 0
        while remaining > 0:
            batch = min(remaining, cap)
            amount_spores = batch * SPORES
            # mint(caller[32] + to[32] + amount[u64 LE]) = 72 bytes
            args = list(admin_bytes) + list(reserve_bytes) + list(struct.pack('<Q', amount_spores))

            try:
                sig = await call_contract_raw(conn, admin, tokens[sym], 'mint', args)
                print(f"  ✅ Minted {batch:>12,} {sym} (batch {batch_num}) → reserve_pool  (sig: {sig[:16]}...)")
                remaining -= batch
                batch_num += 1
                minted += 1
            except Exception as e:
                print(f"  ❌ {sym} mint batch {batch_num} failed: {e}")
                break

            # If more batches needed, epoch cap blocks further mints
            if remaining > 0:
                print(f"    ⚠️  {remaining:,} {sym} remaining but epoch cap reached ({cap:,}/epoch).")
                print(f"    Waiting is impractical. Consider increasing MINT_CAP_PER_EPOCH for testnet.")
                break

    print(f"\n  {minted} mint transaction(s) completed.")
        print(f"  Next: run  python tools/seed_dex_liquidity.py  to place CLOB orders + AMM liquidity.")


asyncio.run(main())
