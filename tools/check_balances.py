#!/usr/bin/env python3
"""Check token balances of reserve_pool."""
import sys, os, asyncio
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
sys.path.insert(0, os.path.dirname(__file__))
from lichen import Connection
from deploy_dex import load_genesis_keypair

SPORES = 1_000_000_000
RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')

async def main():
    conn = Connection(RPC)
    reserve = load_genesis_keypair('reserve_pool', NETWORK)
    reserve_address = str(reserve.address())

    # LICN balance
    bal = await conn._rpc('getBalance', [reserve_address])
    spores = bal.get('spores', 0) if isinstance(bal, dict) else bal
    print(f"Reserve pool: {reserve_address}")
    print(f"  LICN: {spores / SPORES:,.3f}")

    # Token balances
    token_result = await conn._rpc("getTokenAccounts", [reserve_address])
    token_accounts = {
        account.get("symbol"): account
        for account in token_result.get("accounts", [])
        if account.get("symbol")
    }

    for symbol in ("LUSD", "WBNB", "WETH", "WSOL"):
        account = token_accounts.get(symbol)
        ui_amount = account.get("ui_amount", 0) if account else 0
        print(f"  {symbol}: {ui_amount:,.3f}")

asyncio.run(main())
