# DEX Liquidity Seeding Scripts

Scripts to bootstrap protocol-owned liquidity on the Lichen DEX CLOB.
See `docs/strategy/DEX_LIQUIDITY_STRATEGY.md` for the full strategy.

## Prerequisites

- Python 3.10+
- Lichen Python SDK (`sdk/python/`)
- `httpx` and `pynacl` packages (install via `pip install httpx pynacl`)
- Genesis keypairs in `artifacts/<network>/genesis-keys/`
- Running validator with RPC accessible

## Scripts (run in order)

| # | Script | Purpose | Signer |
|---|--------|---------|--------|
| 1 | `01_mint_lusd.py` | Mint protocol-backed lUSD into reserve_pool | genesis-primary (deployer/admin) |
| 2 | `02_place_sell_orders.py` | Place graduated LICN sell orders on LICN/lUSD CLOB | reserve_pool |
| 3 | `03_place_buy_orders.py` | Place graduated lUSD buy orders on LICN/lUSD CLOB | reserve_pool |

## Usage

All scripts run **locally** and connect to a VPS RPC endpoint. You only need
to run against **one** VPS — state is replicated to all validators via consensus.

```bash
# From repo root
cd scripts/dex-liquidity

# Step 1: Mint lUSD (requires deployer keypair — the lusd_token admin)
python3 01_mint_lusd.py --rpc http://15.204.229.189:8899 --network testnet

# Step 2: Place LICN sell orders (requires reserve_pool keypair)
python3 02_place_sell_orders.py --rpc http://15.204.229.189:8899 --network testnet

# Step 3: Place lUSD buy orders (requires reserve_pool keypair)
python3 03_place_buy_orders.py --rpc http://15.204.229.189:8899 --network testnet
```

## Verification

After running, verify orders on any VPS:
```bash
curl -s http://<VPS_IP>:8899 -X POST \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getDexOrderBook","params":[1, 50]}' | python3 -m json.tool
```

## Networks

| Network | RPC | Keypair dir |
|---------|-----|-------------|
| testnet | `http://<VPS>:8899` | `artifacts/testnet/genesis-keys/` |
| mainnet | `http://<VPS>:9899` | `artifacts/mainnet/genesis-keys/` |

## Notes

- Scripts use the **real contract call** flow (build tx → sign → send via RPC).
  No direct state writes — identical to what a real user/SDK would do.
- Each script is idempotent-safe: re-running places additional orders (cancel first if needed).
- Order IDs are logged to `orders_<network>.json` for tracking.
- One RPC call propagates to all validators via consensus (~800ms).
