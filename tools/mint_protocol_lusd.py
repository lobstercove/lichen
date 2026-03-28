#!/usr/bin/env python3
"""Mint protocol-backing lUSD (and testnet wrapped tokens) to reserve_pool.

Per DEX_LIQUIDITY_STRATEGY.md §3.1:
  Deployer (admin of lusd_token) mints lUSD 1:1 against protocol LICN reserves.
  Mint 2,500,000 lUSD into reserve_pool wallet (200% collateral ratio at $0.10/LICN).

On testnet, also mints test amounts of wSOL, wETH, wBNB so all 7 AMM pools
can be seeded with liquidity.

Usage:
  python tools/mint_protocol_lusd.py                       # testnet, localhost
  LICHEN_RPC_URL=http://host:8899 python tools/mint_protocol_lusd.py
"""
import sys, os, struct, asyncio
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw

SPORES = 1_000_000_000  # 1 token = 1B spores
RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')

# Amounts to mint (whole tokens)
MINT_AMOUNTS = {
    "LUSD": 2_500_000,     # $2.5M backing for CLOB buy-wall + AMM
    "WSOL": 10_000,        # testnet: ~$1.5M at ~$150/SOL
    "WETH": 500,           # testnet: ~$1M at ~$2000/ETH
    "WBNB": 5_000,         # testnet: ~$3M at ~$610/BNB
}


async def main():
    conn = Connection(RPC)
    repo = Path(__file__).resolve().parent.parent

    # ── Load deployer (admin of all token contracts) ──
    deployer_path = repo / "keypairs" / "deployer.json"
    if not deployer_path.exists():
        print(f"ERROR: deployer keypair not found at {deployer_path}")
        sys.exit(1)
    deployer = Keypair.load(deployer_path)
    deployer_bytes = bytes(deployer.public_key().to_bytes())
    print(f"  Deployer:      {deployer.public_key()}")

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

        amount_spores = whole_amount * SPORES
        # mint(caller[32] + to[32] + amount[u64 LE]) = 72 bytes
        args = list(deployer_bytes) + list(reserve_bytes) + list(struct.pack('<Q', amount_spores))

        try:
            sig = await call_contract_raw(conn, deployer, tokens[sym], 'mint', args)
            print(f"  ✅ Minted {whole_amount:>12,} {sym} → reserve_pool  (sig: {sig[:16]}...)")
            minted += 1
        except Exception as e:
            print(f"  ❌ {sym} mint failed: {e}")

    print(f"\n  {minted}/{len(MINT_AMOUNTS)} tokens minted successfully.")
    if minted > 0:
        print(f"  Next: run  python tools/seed_dex_liquidity.py  to place CLOB orders + AMM liquidity.")


asyncio.run(main())
