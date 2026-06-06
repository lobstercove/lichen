# Lichen DEX Liquidity Strategy

> How to bootstrap buy/sell liquidity for LICN, lUSD, and wrapped tokens at launch.

---

## 1. The Problem

At launch, the Lichen DEX has 13 trading pairs on both the CLOB and AMM:

| Pair | Quote |
|------|-------|
| LICN/lUSD | lUSD |
| wSOL/lUSD | lUSD |
| wETH/lUSD | lUSD |
| wBNB/lUSD | lUSD |
| wNEO/lUSD | lUSD |
| wGAS/lUSD | lUSD |
| wBTC/lUSD | lUSD |
| wSOL/LICN | LICN |
| wETH/LICN | LICN |
| wBNB/LICN | LICN |
| wNEO/LICN | LICN |
| wGAS/LICN | LICN |
| wBTC/LICN | LICN |

The pairs are created at genesis with initial prices (LICN = $0.10, SOL/ETH/BNB/NEO/GAS/BTC from the genesis oracle price bundle), but **there are no orders on the order book and no liquidity in the AMM pools**. If a user bridges SOL, NEO, GAS, or BTC and wants to buy LICN, there is nothing to buy because nobody has placed sell orders yet.

The 500M LICN supply is distributed across 6 wallets + the deployer, all holding native LICN. Wrapped tokens (wSOL, wETH, wBNB, wNEO, wGAS, wBTC) start at 0 supply — they only get minted when someone deposits real assets through custody/bridge routes. Testnet launch rehearsals may use clearly marked synthetic wrapped mints from `tools/mint_protocol_lusd.py`; mainnet wrapped liquidity must come from real reserves and reserve attestations. lUSD also starts at 0 supply.

**Core question**: Where does the sell-side LICN come from when early users want to buy?

---

## 2. Genesis Supply Distribution

| Wallet | LICN | % | Purpose |
|--------|------|---|---------|
| Deployer (genesis signer) | Remainder after distributions | — | Contract deployment, initial ops |
| validator_rewards | 50,000,000 | 10% | Block producer rewards, fee distribution |
| community_treasury | 125,000,000 | 25% | Governance proposals, ecosystem growth |
| builder_grants | 175,000,000 | 35% | Developer incentives, DEX rewards (1yr seeded) |
| founding_symbionts | 50,000,000 | 10% | Early community, staking bootstrap |
| ecosystem_partnerships | 50,000,000 | 10% | Exchange listings, integrations |
| reserve_pool | 50,000,000 | 10% | Emergency reserves, liquidity backstop |

**Total**: 500,000,000 LICN. Block rewards mint ~20M LICN/year (4% declining inflation).

---

## 3. Strategy: Protocol-Owned Liquidity (POL)

Instead of relying on external market makers at launch, **use the reserve_pool and community_treasury wallets to seed protocol-owned liquidity** on the DEX. This is the standard approach used by most L1 launches (Sui, Aptos, Sei all did variations of this).

### 3.1 Phase 1 — LICN/lUSD Order Book Seeding (Day 0)

The **reserve_pool** wallet (50M LICN) acts as the initial market maker:

1. **Mint protocol-backing lUSD**
   - The deployer (admin of lusd_token) mints lUSD 1:1 against the protocol's own LICN reserves
   - Mint 2,500,000 lUSD (representing $2.5M at $1/lUSD peg) into the reserve_pool wallet
   - This is backed by the 50M LICN in reserve_pool at $0.10 = $5M value (200% collateral ratio)

2. **Place buy-wall and sell-wall orders on the CLOB**
   - **Sell side** (LICN → lUSD): Place ~8.6M LICN in sell orders across 25 levels ($0.002 increments):
     - $0.100–$0.110: 4.2M LICN (dense zone near genesis price, 6 levels)
     - $0.112–$0.126: 3.2M LICN (mid zone, 7 levels)
     - $0.128–$0.148: 2.2M LICN (upper zone, 11 levels)
   - **Buy side** (lUSD → LICN): Place lUSD buy orders across 25 levels ($0.002 decrements):
     - $0.098–$0.088: 2.15M LICN / ~$195K lUSD (tight support, 6 levels)
     - $0.086–$0.074: 1.75M LICN / ~$140K lUSD (mid support, 7 levels)
     - $0.072–$0.050: 1.65M LICN / ~$101K lUSD (deep support, 12 levels)

   This creates realistic order book depth with 25 levels on each side,
   $0.002 spacing, and graduated volume (heavier near the current price).

3. **Seed AMM concentrated liquidity pool**
   - Deposit 5M LICN + 500,000 lUSD into the LICN/lUSD AMM pool
   - Set tick range around $0.05–$0.25 (broad range for early volatility)
   - Fee tier: 30bps (standard for volatile pairs)

### 3.2 Phase 1b — Wrapped Token Pairs (Day 0)

For wSOL/lUSD, wETH/lUSD, wBNB/lUSD, wNEO/lUSD, and wGAS/lUSD pairs, liquidity bootstraps differently:

1. Wrapped tokens have **0 supply at genesis** — they only exist when users deposit real assets via custody/bridge routes
2. When a user deposits 1 SOL, 1 NEO, or 1 GAS, the custody system mints the matching wrapped token on Lichen
3. The user now has a wrapped asset and wants either lUSD or LICN

**For wSOL/LICN, wETH/LICN, wBNB/LICN, wNEO/LICN, and wGAS/LICN pairs**:
- The reserve_pool's LICN is already on the LICN side of the book
- When a wSOL holder wants to sell wSOL for LICN, we need LICN buy orders denominated in wSOL
- Place LICN sell orders on the wSOL/LICN pair: 5M LICN across price range (SOL/LICN ratio based on oracle prices)

**For wSOL/lUSD, wETH/lUSD, wBNB/lUSD, wNEO/lUSD, and wGAS/lUSD pairs**:
- These pairs need lUSD on the buy side
- Use the same protocol-minted lUSD to place buy orders
- The oracle prices from LichenOracle (seeded at genesis with real prices) set the reference rate
- wNEO markets preserve whole-NEO lots: order quantities and AMM deposits are whole-token amounts in Lichen base units.

### 3.3 Phase 2 — User Flow: "I bridged SOL, now what?"

Here's how a user's journey works end-to-end with this strategy:

```
User deposits 10 SOL on Solana
    → Custody detects deposit, sweeps to omnibus
    → Lichen mints 10 wSOL to user's Lichen address
    → User goes to DEX

Option A: Sell wSOL for LICN (direct pair)
    → User hits reserve_pool's LICN sell orders on wSOL/LICN book
    → User gets LICN at oracle price ± spread

Option B: Sell wSOL for lUSD (stablecoin)
    → User hits reserve_pool's lUSD buy orders on wSOL/lUSD book
    → User gets lUSD

Option C: Sell wSOL → lUSD → LICN (routed)
    → DEX Router finds best path
    → Step 1: wSOL → lUSD on wSOL/lUSD pair
    → Step 2: lUSD → LICN on LICN/lUSD pair
    → User gets LICN (potentially better rate via routing)
```

### 3.4 Phase 3 — Organic Liquidity Growth

As the DEX gets volume, transition from protocol-owned to community liquidity:

1. **DEX Rewards program** (builder_grants wallet, 1yr of rewards already seeded)
   - LP mining: Users who provide liquidity on AMM pools earn LICN rewards
   - Trading fee sharing: 20% of trading fees go to LPs, 20% to stakers
   - This incentivizes external LPs to replace protocol-owned liquidity

2. **MossStake liquid staking**
   - Users stake LICN → get stLICN → use stLICN as collateral or LP in DeFi
   - Creates natural demand for LICN (staking yield 5–18% APY depending on lock tier)

3. **SporePump launchpad**
   - New tokens launch via bonding curve → graduate to DEX
   - Each graduation adds a new LICN pair, creating more organic liquidity

4. **Gradually remove protocol orders**
   - As organic volume exceeds protocol-provided liquidity, thin out reserve orders
   - Move reserve LICN back to reserve_pool for future needs
   - Target: protocol-owned liquidity < 20% of total DEX liquidity within 6 months

---

## 4. lUSD Backing Mechanism

lUSD is a **protocol-issued stablecoin**, not an algorithmic or CDP-based stablecoin. Its backing model:

| Backing Source | Description |
|----------------|-------------|
| **Bridge/custody reserves** | Real SOL/ETH/BNB/NEO/GAS held in custody wallets on source chains. Every wSOL/wETH/wBNB/wNEO/wGAS in circulation is 1:1 backed by real assets. When users sell a wrapped asset for lUSD, the source-chain reserve still backs the wrapped asset in the pool. |
| **Protocol LICN reserves** | lUSD minted by the protocol is backed by LICN in the reserve_pool at >100% collateral ratio |
| **Reserve attestation** | lusd_token contract has `attest_reserves` function — the oracle can attest on-chain that reserves back outstanding supply |

**minting rules**:
- Only the deployer (admin) can call `mint` on lusd_token — no unauthorized minting
- Protocol-minted lUSD must maintain >150% collateral ratio (LICN value at oracle price)
- As bridge deposits grow, bridged stablecoins (USDC/USDT on Solana/Ethereum) can directly back lUSD 1:1

**Phase 2 enhancement**: When USDC/USDT bridge deposits go live, lUSD can be minted 1:1 against real stablecoins, making the peg fully hard-backed.

---

## 5. Implementation Checklist

### Pre-Launch (before opening bridge deposits)

- [x] **Script: `tools/fund_accounts.py`** — Fund admin + reserve_pool wallets
  - Fund admin with 10 LICN (for contract admin calls)
  - Fund reserve_pool with 50M+ LICN (market-making liquidity)

- [x] **Script: `tools/mint_protocol_lusd.py`** — Mint initial token supply
  - Deployer mints 2,500,000 lUSD, 10,000 wSOL, 500 wETH, 5,000 WBNB, 50,000 wNEO, 75,000 wGAS, and 100 wBTC into reserve_pool on testnet
  - Uses dynamic contract addresses from symbol registry
  - Backed by LICN reserves at >150% collateral ratio
  - Skips synthetic wrapped-token mints on mainnet unless explicitly overridden; mainnet wrapped liquidity must come from real custody reserves
  - Waits for the funded genesis-primary balance before minting, so it can be chained immediately after `tools/fund_accounts.py`

- [x] **Script: `tools/seed_dex_liquidity.py`** — Full CLOB + AMM seeding
  - Pre-approves all 7 non-native tokens for dex_core and dex_amm
  - Phase 1: 50 graduated LICN/lUSD CLOB orders (25 sell + 25 buy, $0.002 spacing)
    - Sell wall: ~9.6M LICN across $0.100–$0.148
    - Buy wall: ~5.5M LICN / ~$440K lUSD across $0.098–$0.050
  - Phase 1b: 240 wrapped token pair CLOB orders (20 per pair × 12 pairs)
  - Phase 2: 13 AMM concentrated liquidity positions with tick-aligned ranges
  - Total: 290 CLOB orders + 13 AMM pools
  - Uses active chain oracle prices for DEX price-band compliance, with Binance/deterministic fallback prices and explicit env overrides

- [x] **AMM pool seeding** — All 13 pools with non-zero liquidity
  - Pool 1 LICN/lUSD: 5M LICN + 500K lUSD, ticks [-30000, -13860]
  - Pool 2 wSOL/lUSD: 500 wSOL + 50K lUSD
  - Pool 3 wETH/lUSD: 25 wETH + 50K lUSD
  - Pool 4 wSOL/LICN: 500 wSOL + 500K LICN
  - Pool 5 wETH/LICN: 25 wETH + 500K LICN
  - Pool 6 wBNB/lUSD: 100 wBNB + 50K lUSD
  - Pool 7 wBNB/LICN: 100 wBNB + 500K LICN
  - Pool 8 wNEO/lUSD: up to 2,500 wNEO + 8K lUSD
  - Pool 9 wNEO/LICN: up to 2,500 wNEO + 75K LICN
  - wNEO AMM seeding mirrors the AMM liquidity math before submitting and reduces token-B max input when needed so the actual wNEO transfer is an exact whole-NEO amount.
  - Pool 10 wGAS/lUSD: 10K wGAS + 16.5K lUSD
  - Pool 11 wGAS/LICN: 10K wGAS + 165K LICN
  - Pool 12 wBTC/lUSD: 5 wBTC + 250K lUSD
  - Pool 13 wBTC/LICN: 5 wBTC + 500K LICN
  - Fee tier: 30bps, tick_spacing=60 for all pools

- [x] **Oracle price feeds live**
  - LichenOracle seeded at genesis ✓
  - Verify price update mechanism works (oracle authority can update)

- [x] **DEX Router routes configured**
  - All 13 CLOB routes registered ✓ (done at genesis)
  - All 13 AMM routes registered ✓ (done at genesis)
  - Smart routing picks best execution path

- [x] **Verification script: `tools/check_amm_pools.py`** — Reads all 13 pool liquidity values on-chain

### Launch Day

- [ ] Open bridge deposits (custody service)
- [ ] Monitor order book depth — refill if orders get consumed
- [ ] Watch spread: target < 2% spread on LICN/lUSD
- [ ] Announce LP mining rewards activated

### Post-Launch (Week 1–4)

- [ ] Monitor reserve_pool balance — if < 20M LICN remaining, slow down
- [ ] Activate LP rewards from DEX Rewards program
- [ ] Track organic vs protocol liquidity ratio
- [ ] Community governance proposal for liquidity mining parameters

---

## 6. Risk Mitigation

| Risk | Mitigation |
|------|------------|
| LICN price drops below seed prices | Buy wall absorbs selling pressure; orders auto-fill at lower prices |
| All reserve LICN gets sold | Hard limit: never deploy more than 20M LICN (40%) from reserve_pool to market making. Keep 30M LICN as untouchable reserve. |
| lUSD de-pegs | Reserve attestation makes backing transparent. Over-collateralization (>150%) provides buffer. Emergency pause on lusd_token if needed. |
| Wash trading / manipulation | dex_core has self-trade prevention, min_order_value (1000 spores), and post-only order type for genuine market makers |
| Bridge exploit depletes wrapped tokens | Custody multi-sig threshold (2/3 testnet, 3/5 mainnet) prevents unauthorized withdrawals. Emergency pause on bridge contract. |

---

## 7. Comparable L1 Launch Strategies

| Chain | Approach | Notes |
|-------|----------|-------|
| **Solana** | Foundation market-making on Serum DEX | Solana Foundation seeded SOL/USDC order books from treasury |
| **Sui** | Protocol-owned liquidity pools on DeepBook | SUI Foundation provided initial CLOB liquidity |
| **Aptos** | Community airdrop + DEX incentives | APT distributed free, creating natural sell pressure that made markets |
| **Sei** | Built-in order book + market maker partnerships | Combined protocol orders with external MMs |
| **Lichen** | Reserve pool + protocol lUSD backing | Self-sufficient: no external MMs needed at launch |

---

## 8. Summary

**Where does LICN come from when someone wants to buy?**
→ The **reserve_pool** wallet (50M LICN) provides initial sell-side liquidity on the CLOB and AMM.

**Where does lUSD come from?**
→ Protocol-mints lUSD backed by LICN reserves at >150% collateral ratio.

**What about wSOL/wETH/wBNB/wNEO/wGAS liquidity?**
→ Users bring their own wrapped tokens by depositing through custody/bridge routes. The DEX has LICN and lUSD on the other side of the book ready to match, and wNEO markets preserve whole-NEO lots.

**When do we stop needing protocol liquidity?**
→ When organic LP volume from DEX Rewards mining exceeds protocol-owned positions (~3–6 months target).

---

## 9. VPS Deployment Runbook — DEX Liquidity Seeding

After a fresh genesis on the VPS cluster (3 validators), run these scripts **in order** from the repo root on the seed node (US VPS / seed-01). All scripts read keypairs from `data/state-testnet/genesis-keys/` (testnet) or `data/state-mainnet/genesis-keys/` (mainnet).

### Prerequisites

```bash
# Python 3.10+ with the SDK on PYTHONPATH
# Ensure the validators are running and producing blocks
curl -s http://localhost:8899 -X POST \
  -H "Content-Type:application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}' | python3 -m json.tool

# Ensure the state-testnet symlink exists (reset script removes it)
ln -sf state-7001 data/state-testnet   # testnet
ln -sf state-8001 data/state-mainnet   # mainnet
```

### Step 1: Fund Accounts

```bash
python3 tools/fund_accounts.py
```

Funds the admin wallet (10 LICN for contract admin calls) and the reserve_pool wallet (50M+ LICN for market-making). Uses the faucet or treasury transfer.

### Step 2: Mint Protocol Tokens

```bash
python3 tools/mint_protocol_lusd.py
```

Mints 2,500,000 lUSD, 10,000 wSOL, 500 wETH, 5,000 wBNB, 50,000 wNEO, and 75,000 wGAS into the reserve_pool wallet on testnet. Uses dynamic addresses from the on-chain symbol registry. Mainnet wrapped assets should be funded by custody deposits/reserve attestations, not synthetic mints.

### Step 3: Seed DEX Liquidity

```bash
python3 tools/seed_dex_liquidity.py
```

This is the main seeding script. It:
1. Discovers all contract addresses from the symbol registry
2. Pre-approves all 7 non-native tokens for dex_core and dex_amm spending
3. Reads active chain oracle prices, with Binance/deterministic fallbacks and env overrides
4. Places 290 CLOB orders (50 LICN/lUSD + 240 wrapped pairs)
5. Adds concentrated liquidity to all 13 AMM pools
6. Reports results at the end

Expected output: `290` CLOB orders + `13/13` AMM pools seeded.

### Step 4: Verify

```bash
python3 tools/check_amm_pools.py
```

Reads all 13 AMM pool liquidity values directly from on-chain storage. All 13 should show `✅ HAS LIQ`.

### If Any Wrapped Pool Shows 0 Liquidity

This happens when the reserve wallet was not funded with the wrapped asset before the seed script ran, or when an old partial seeding run left duplicate CLOB orders without AMM positions. Reset the local launch rehearsal state or top up reserve assets, then run the full mint + seed flow again:

```bash
python3 tools/mint_protocol_lusd.py      # mints lUSD + all testnet wrapped assets
python3 tools/seed_dex_liquidity.py      # seeds 290 CLOB orders + 13 AMM pools
python3 tools/check_amm_pools.py         # verify all 13 pools
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `LICHEN_RPC_URL` | `http://127.0.0.1:8899` | RPC endpoint |
| `LICHEN_NETWORK` | `testnet` | Network name (for keypair paths) |
| `LICHEN_DEX_SEED_COMPUTE_BUDGET` | `1400000` | Compute budget for launch approval/order/AMM transactions |
| `LICHEN_USD_PRICE` | chain/fallback `0.10` | Override LICN/USD used by the launch seeder |
| `LICHEN_SOL_USD_PRICE` | chain/Binance/fallback | Override SOL/USD |
| `LICHEN_ETH_USD_PRICE` | chain/Binance/fallback | Override ETH/USD |
| `LICHEN_BNB_USD_PRICE` | chain/Binance/fallback | Override BNB/USD |
| `LICHEN_NEO_USD_PRICE` | chain/Binance/fallback | Override NEO/USD |
| `LICHEN_GAS_USD_PRICE` | chain/Binance/fallback | Override GAS/USD |
| `LICHEN_BTC_USD_PRICE` | chain/Binance/fallback | Override BTC/USD |

For mainnet, prefix commands: `LICHEN_NETWORK=mainnet LICHEN_RPC_URL=http://127.0.0.1:9899 python3 tools/...`

### Script Index

| Script | Purpose | Idempotent? |
|--------|---------|-------------|
| `tools/fund_accounts.py` | Fund admin + reserve wallets | Yes (additive) |
| `tools/mint_protocol_lusd.py` | Mint lUSD and testnet wrapped assets to reserve | No (mints additional supply) |
| `tools/seed_dex_liquidity.py` | Place 290 CLOB + seed 13 AMM pools | No (places duplicate orders) |
| `tools/check_amm_pools.py` | Read-only: verify 13 pool liquidity | Yes (read-only) |
| `tools/check_balances.py` | Read-only: check reserve token balances | Yes (read-only) |
| `tools/verify_orderbook.py` | Read-only: verify CLOB order counts | Yes (read-only) |
