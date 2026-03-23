# Lichen DEX Liquidity Strategy

> How to bootstrap buy/sell liquidity for LICN, lUSD, and wrapped tokens at launch.

---

## 1. The Problem

At launch, the Lichen DEX has 7 trading pairs on both the CLOB and AMM:

| Pair | Quote |
|------|-------|
| LICN/lUSD | lUSD |
| wSOL/lUSD | lUSD |
| wETH/lUSD | lUSD |
| wBNB/lUSD | lUSD |
| wSOL/LICN | LICN |
| wETH/LICN | LICN |
| wBNB/LICN | LICN |

The pairs are created at genesis with initial prices (LICN = $0.10, SOL/ETH/BNB at live Binance prices), but **there are no orders on the order book and no liquidity in the AMM pools**. If a user bridges SOL and wants to buy LICN, there is nothing to buy because nobody has placed sell orders yet.

The 500M LICN supply is distributed across 6 wallets + the deployer, all holding native LICN. Wrapped tokens (wSOL, wETH, wBNB) start at 0 supply — they only get minted when someone deposits real assets through the bridge. lUSD also starts at 0 supply.

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
   - **Sell side** (LICN → lUSD): Place 10M LICN in sell orders across a range:
     - 2M LICN @ $0.10 (genesis price)
     - 2M LICN @ $0.11
     - 2M LICN @ $0.12
     - 2M LICN @ $0.15
     - 2M LICN @ $0.20
   - **Buy side** (lUSD → LICN): Place lUSD buy orders:
     - 500,000 lUSD @ $0.09
     - 500,000 lUSD @ $0.08
     - 500,000 lUSD @ $0.07
     - 500,000 lUSD @ $0.05
     - 500,000 lUSD @ $0.03

   This creates visible depth on both sides of the book.

3. **Seed AMM concentrated liquidity pool**
   - Deposit 5M LICN + 500,000 lUSD into the LICN/lUSD AMM pool
   - Set tick range around $0.05–$0.25 (broad range for early volatility)
   - Fee tier: 30bps (standard for volatile pairs)

### 3.2 Phase 1b — Wrapped Token Pairs (Day 0)

For wSOL/lUSD, wETH/lUSD, wBNB/lUSD pairs, liquidity bootstraps differently:

1. Wrapped tokens have **0 supply at genesis** — they only exist when users deposit real assets via the bridge
2. When a user deposits 1 SOL, the custody system mints 1 wSOL on Lichen
3. The user now has wSOL and wants either lUSD or LICN

**For wSOL/LICN, wETH/LICN, wBNB/LICN pairs**:
- The reserve_pool's LICN is already on the LICN side of the book
- When a wSOL holder wants to sell wSOL for LICN, we need LICN buy orders denominated in wSOL
- Place LICN sell orders on the wSOL/LICN pair: 5M LICN across price range (SOL/LICN ratio based on oracle prices)

**For wSOL/lUSD, wETH/lUSD, wBNB/lUSD pairs**:
- These pairs need lUSD on the buy side
- Use the same protocol-minted lUSD to place buy orders
- The oracle prices from LichenOracle (seeded at genesis with real prices) set the reference rate

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
| **Bridge reserves** | Real SOL/ETH/BNB held in custody wallets on source chains. Every wSOL/wETH/wBNB in circulation is 1:1 backed by real assets. When users sell wSOL for lUSD, the real SOL still backs the wSOL in the pool. |
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

- [ ] **Script: seed_dex_liquidity.py** — Automate CLOB order placement from reserve_pool
  - Read reserve_pool keypair from genesis-keys/
  - Place graduated sell orders on LICN/lUSD (10M LICN across 5 price levels)
  - Place graduated buy orders on LICN/lUSD (2.5M lUSD across 5 price levels)
  - Place orders on all 7 pairs
  - Log all order IDs for monitoring

- [ ] **Script: mint_protocol_lusd.py** — Mint initial lUSD backing
  - Deployer calls lusd_token.mint() to mint lUSD into reserve_pool
  - Amount: based on LICN collateral ratio
  - Call attest_reserves() after minting

- [ ] **AMM pool seeding**
  - Add concentrated liquidity positions on all 7 AMM pools
  - Use reserve_pool LICN + protocol lUSD
  - Set appropriate tick ranges based on oracle prices

- [ ] **Oracle price feeds live**
  - LichenOracle seeded at genesis ✓
  - Verify price update mechanism works (oracle authority can update)

- [ ] **DEX Router routes configured**
  - All 7 CLOB routes registered ✓ (done at genesis)
  - All 7 AMM routes registered ✓ (done at genesis)
  - Smart routing picks best execution path

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

**What about wSOL/wETH/wBNB liquidity?**
→ Users bring their own wrapped tokens by depositing through the bridge. The DEX has LICN and lUSD on the other side of the book ready to match.

**When do we stop needing protocol liquidity?**
→ When organic LP volume from DEX Rewards mining exceeds protocol-owned positions (~3–6 months target).
