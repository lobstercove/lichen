# MOLT Tokenomics & Launch Price Analysis

**Date:** February 13, 2026  
**Status:** Pre-Launch Analysis  
**Decision Required:** Initial MOLT launch price + system-wide parameter adjustment

---

## Table of Contents

1. [Current State of the Code](#1-current-state-of-the-code)
2. [Emission & Burn Math](#2-emission--burn-math)
3. [The Core Problem](#3-the-core-problem)
4. [Launch Price Scenarios](#4-launch-price-scenarios)
5. [Recommended Launch Price](#5-recommended-launch-price)
6. [Full System Parameter Readjustment](#6-full-system-parameter-readjustment)
7. [Genesis Distribution Conflict](#7-genesis-distribution-conflict)
8. [Other Discrepancies to Fix](#8-other-discrepancies-to-fix)

---

## 1. Current State of the Code

### Supply

| Parameter | Value |
|-----------|-------|
| Total Supply | **1,000,000,000 MOLT** (1 billion) |
| Decimals | **9** (1 MOLT = 1,000,000,000 shells) |
| Mintable | **No** — fixed supply, no inflation minting |
| Burnable | **Yes** — 50% of all transaction fees burned |

### Genesis Distribution (multisig.rs — canonical)

| Wallet | MOLT | % | Purpose |
|--------|------|---|---------|
| Validator Rewards | 150,000,000 | 15% | Block rewards pool (diminishes over time) |
| Community Treasury | 400,000,000 | 40% | Governance-allocated spending |
| Builder Grants | 250,000,000 | 25% | Developer incentives |
| Founding Moltys | 100,000,000 | 10% | Team / founding agents |
| Ecosystem Partnerships | 50,000,000 | 5% | Partnerships, integrations |
| Reserve Pool | 50,000,000 | 5% | Emergency reserve |

### Current Reward Rates

| Reward Type | Amount per event | In shells |
|-------------|-----------------|-----------|
| Transaction block reward | **0.9 MOLT** per block | 900,000,000 |
| Heartbeat block reward | **0.135 MOLT** per empty block | 135,000,000 |
| Slots per year | 78,840,000 (~400ms slots) | — |
| Code: `ANNUAL_INFLATION_BPS` | 500 (5%) | — |

### Fee Structure

| Fee | Amount | At $0.10/MOLT |
|-----|--------|---------------|
| Base transaction fee | **0.001 MOLT** per tx (1,000,000 shells) | $0.0001 |
| Contract deploy | **25 MOLT** per deploy | $2.50 |
| Contract upgrade | **10 MOLT** per upgrade | $1.00 |
| NFT mint | **0.5 MOLT** per mint | $0.05 |
| NFT collection create | **1,000 MOLT** per collection | $100.00 |
| ClawPump token create | **0.1 MOLT** per token | $0.01 |
| DAO proposal stake | **1,000 MOLT** (returned after vote) | $100.00 |

### Fee Distribution

| Destination | % |
|-------------|---|
| **Burned** | 50% |
| Block producer | 30% |
| Voter validators | 10% |
| Community treasury | 10% |

---

## 2. Emission & Burn Math

### Validator Reward Emission

Assuming ~400ms slots, 78.84M slots/year:

**Scenario A — High activity (50% blocks have transactions):**
- TX blocks: 39.42M × 0.9 MOLT = **35,478,000 MOLT/year**
- Empty blocks: 39.42M × 0.135 MOLT = **5,321,700 MOLT/year**
- **Total emission: ~40,799,700 MOLT/year** (~4.08% of supply)

**Scenario B — Low activity (10% blocks have transactions):**
- TX blocks: 7.884M × 0.9 MOLT = **7,095,600 MOLT/year**
- Empty blocks: 70.956M × 0.135 MOLT = **9,579,060 MOLT/year**
- **Total emission: ~16,674,660 MOLT/year** (~1.67% of supply)

**Scenario C — Maximum activity (100% blocks have transactions):**
- TX blocks: 78.84M × 0.9 MOLT = **70,956,000 MOLT/year**
- **Total emission: ~71M MOLT/year** (~7.1% of supply)

> **Note:** With adaptive heartbeat (5s interval instead of 400ms), actual heartbeat
> block emission is ~12.5x lower than shown. At 5s intervals, heartbeat blocks/year ≈ 6.3M
> (not 39-71M), so real-world heartbeat emission in Scenario B is closer to 850K MOLT/year.

### Reward Pool Depletion

The validator reward pool is **150,000,000 MOLT** (15% of supply).

| Activity Level | Annual Emission | Years Until Depleted |
|---------------|----------------|---------------------|
| Low (10% tx) | 16.7M MOLT/yr | **~9 years** |
| Medium (50% tx) | 40.8M MOLT/yr | **~3.7 years** |
| High (100% tx) | 71M MOLT/yr | **~2.1 years** |

> **Note:** These are theoretical maximums at base reward rates. In practice, the
> price-based reward adjustment (oracle) reduces emissions as MOLT price rises
> above $0.10, and adaptive heartbeat reduces empty-block emissions by ~12.5x.

### Fee Burn Estimation

For burn to matter, we need transaction volume. At 0.001 MOLT base fee:

| Daily Transactions | Fee Burned/Day | Annual Burn |
|-------------------|---------------|-------------|
| 100,000 | 50 MOLT | 18,250 MOLT |
| 1,000,000 | 500 MOLT | 182,500 MOLT |
| 10,000,000 | 5,000 MOLT | 1,825,000 MOLT |
| 100,000,000 | 50,000 MOLT | 18,250,000 MOLT |

**At the current 0.001 MOLT base fee ($0.0001/tx at $0.10), burn is meaningful at scale.** 10M tx/day burns 1.8M MOLT/year (0.18% of supply). Combined with larger fees (contract deploys at 25 MOLT, NFT collections at 1,000 MOLT, DEX trading fees), the deflationary mechanic becomes substantial.

### DEX Rewards Emission

**dex_rewards: 1,000,000 MOLT/month = 12,000,000 MOLT/year**

This is **massive** — it's nearly the same as maximum block reward emission. If sourced from the validator rewards pool (150M), it depletes in 12.5 years **on its own**. If sourced from builder grants (250M), it adds up fast with block rewards.

**This needs a source wallet defined clearly.** Currently the reward claim doesn't even transfer tokens.

---

## 3. The Core Problem

### There's no exchange, no liquidity, no market price

We're bootstrapping from zero. The initial price is a **design decision**, not a market-discovered value. Every fee, every reward, and every cost in the system is denominated in MOLT, so the initial price determines:

1. **Is 0.00001 MOLT per tx too cheap or too expensive?**
2. **Is 0.9 MOLT per block reward sustainable?**
3. **Is 2.5 MOLT to deploy a contract reasonable?**
4. **Is 1,000 MOLT to stake a DAO proposal accessible?**
5. **Is 100 MOLT for an NFT collection a fair barrier?**

### What the price means in practice

| MOLT Price | Base Fee (USD) | Block Reward (USD) | Deploy Cost (USD) | DAO Stake (USD) | Collection (USD) |
|-----------|---------------|-------------------|-------------------|----------------|-----------------|
| $0.001 | $0.00000001 | $0.0009 | $0.0025 | $1.00 | $0.10 |
| $0.01 | $0.0000001 | $0.009 | $0.025 | $10.00 | $1.00 |
| **$0.05** | $0.0000005 | $0.045 | $0.125 | $50.00 | $5.00 |
| **$0.10** | $0.000001 | $0.09 | $0.25 | $100.00 | $10.00 |
| $0.50 | $0.000005 | $0.45 | $1.25 | $500.00 | $50.00 |
| $1.00 | $0.00001 | $0.90 | $2.50 | $1,000.00 | $100.00 |
| $5.00 | $0.00005 | $4.50 | $12.50 | $5,000.00 | $500.00 |
| $10.00 | $0.0001 | $9.00 | $25.00 | $10,000.00 | $1,000.00 |

---

## 4. Launch Price Scenarios

### Scenario A: $0.01 (Penny Launch)

| Metric | Value |
|--------|-------|
| Fully Diluted Valuation (FDV) | **$10,000,000** |
| Transaction fee | $0.00001 (near-free) |
| Block reward | $0.009/block (~$710K/yr at 100% activity) |
| Contract deploy | $0.25 (dirt cheap) |
| DAO proposal | $10 (very accessible) |
| DEX reward pool | $12M/yr in emissions (unsustainable at this price) |
| ClawPump token | $0.001 to create (spam risk) |
| **Assessment** | Too cheap. Spam attacks trivial. DAO governance meaningless. Block rewards don't justify running a validator. |

### Scenario B: $0.10 (Dime Launch)

| Metric | Value |
|--------|-------|
| Fully Diluted Valuation (FDV) | **$100,000,000** |
| Transaction fee | $0.0001 (sub-penny, agent-friendly) |
| Block reward | $0.09/block (~$7.1M/yr at 100%) |
| Contract deploy | $2.50 (very affordable) |
| DAO proposal | $100 (moderate barrier) |
| DEX reward pool | $1.2M/yr emissions (~reasonable) |
| ClawPump token | $0.01 to create (cheap but not free) |
| Founding Moltys holding | $10M |
| **Assessment** | Reasonable starting point. Cheap enough for agents/developers. Revenue to team through founding allocation. FDV is credible for a working L1. |

### Scenario C: $0.50 (Premium Launch)

| Metric | Value |
|--------|-------|
| Fully Diluted Valuation (FDV) | **$500,000,000** |
| Transaction fee | $0.0005 (sub-penny) |
| Block reward | $0.45/block (~$35.5M/yr at 100%) |
| Contract deploy | $12.50 (fair) |
| DAO proposal | $500 (significant barrier) |
| DEX reward pool | $6M/yr emissions |
| ClawPump token | $0.05 to create |
| Founding Moltys holding | $50M |
| **Assessment** | Ambitious but defensible if tech delivers. Higher barrier for degen spam. DAO becomes serious. Need to deliver to justify this FDV. |

### Scenario D: $1.00 (Dollar Launch)

| Metric | Value |
|--------|-------|
| Fully Diluted Valuation (FDV) | **$1,000,000,000** |
| Transaction fee | $0.001 (still very cheap) |
| Block reward | $0.90/block (~$71M/yr at 100%) |
| Contract deploy | $25.00 (reasonable) |
| DAO proposal | $1,000 (high barrier) |
| NFT collection | $1,000.00 (serious) |
| ClawPump token | $0.10 to create |
| Founding Moltys holding | $100M |
| **Assessment** | Comparable to mid-cap L1s at launch. Very ambitious. Forces high DAO seriousness. Risk: if price drops significantly, FDV collapse hits credibility. |

---

## 5. Recommended Launch Price

### $0.10 per MOLT

**Why $0.10:**

1. **Fair FDV ($100M)** — credible for a working L1 blockchain with full DEX, lending, NFT marketplace, cross-chain bridge, governance, identity, oracle, and 26 deployed contracts

2. **Agent-friendly fees** — transaction fee is $0.0001 (sub-penny), so agents can transact millions of times affordably. Gas is never a blocker.

3. **Meaningful governance** — DAO proposal at $100 prevents spam but isn't exclusionary. DEX governance listing at $50 rep barrier makes sense.

4. **Sustainable validator economics** — At 50% activity with oracle adjustment: rewards scale with price. At $0.10 reference price, validators earn meaningful revenue split among all active validators. As MOLT price rises, reward rate adjusts down automatically.

5. **Revenue for the team** — Founding Moltys allocation (100M MOLT) = $10M initial value. Ecosystem partnerships (50M) = $5M. Fair compensation for building a working L1.

6. **Growth ceiling** — 10x to $1.00 (= $1B FDV) is achievable with adoption. Still room to run to $10+ with massive adoption, which would be a $10B FDV (comparable to established L1s like Avalanche, Near).

7. **ClawPump is cheap but not free** — Token creation at $0.01 prevents mindless spam but keeps experimentation alive. Graduation at 100K MOLT = $10K market cap = achievable milestone that's still meaningful.

8. **DEX fee economics** — Taker fee of 5bps (0.05%) of a $1000 trade = $0.50. With 50% burn, $0.25 burned per $1000 traded. At $10M daily volume, $1,250/day burned = 456K MOLT/year burned. This starts making the deflationary mechanic real.

### But: Does _anything_ need readjusting at $0.10?

| Parameter | Current | At $0.10 | Verdict |
|-----------|---------|----------|---------|
| Base tx fee | 0.001 MOLT | $0.0001 | OK — sub-penny, agent-friendly |
| Block reward (tx) | 0.9 MOLT | $0.09/block | OK — oracle adjusts down as price rises |
| Block reward (heartbeat) | 0.135 MOLT | $0.0135/block | OK — 15% of tx reward, adaptive 5s interval |
| Contract deploy | 25 MOLT | $2.50 | OK — affordable, not free |
| Contract upgrade | 10 MOLT | $1.00 | OK |
| NFT mint | 0.5 MOLT | $0.05 | OK — cheap, not free |
| NFT collection | 1,000 MOLT | $100.00 | OK — meaningful barrier for collection spam |
| ClawPump create | 0.1 MOLT | $0.01 | **Maybe too cheap** — consider 1 MOLT ($0.10) |
| DAO proposal stake | 1,000 MOLT | $100.00 | OK — serious but not exclusionary |
| ClawPump graduation | 100K MOLT | $10,000 | OK — agent-friendly, bonding curve still filters spam |
| Max order (DEX) | 1,000 MOLT | $100 | **Too low** — should be higher for serious trading |
| DEX rewards | 1M MOLT/mo | $100K/month | **Needs assessment** — that's $1.2M/yr from a finite pool |
| Min validator stake | 75,000 MOLT | $7,500 | OK — accessible (bootstrap grant is 100K, 25K buffer) |
| Max validator stake | 1,000,000 MOLT | $100,000 | OK — prevents over-concentration |

### Parameters that need adjustment at $0.10:

1. **`MAX_ORDER_SIZE` in dex_core** — Currently 1,000 MOLT = $100. For a proper DEX, this should be much higher (e.g., 10,000,000 MOLT = $1M max order). This is the max _per order_, not per position.

2. **`REWARD_POOL_PER_MONTH` in dex_rewards** — 1M MOLT/month = $100K/month = $1.2M/yr. Combined with block rewards, the reward pool depletion depends heavily on network activity and MOLT price (oracle adjustment). The price-based reward adjustment means higher MOLT prices automatically reduce emission, extending the runway.

3. **`CREATION_FEE` in clawpump** — 0.1 MOLT = $0.01 is probably too cheap. At $0.01, bots will spam thousands of meme tokens. Consider raising to 1-10 MOLT ($0.10-$1.00) to add friction.

4. **`DEFAULT_MAX_BUY_AMOUNT` in clawpump** — 10,000 MOLT = $1,000 max buy. This is reasonable as an anti-whale measure on bonding curves.

---

## 6. Full System Parameter Readjustment

### At $0.10/MOLT, these are the recommended changes:

| Parameter | File | Current | Proposed | Rationale |
|-----------|------|---------|----------|-----------|
| `MAX_ORDER_SIZE` | dex_core/lib.rs | 1,000 MOLT | **10,000,000 MOLT** ($1M) | Proper DEX needs large orders |
| `CREATION_FEE` | clawpump/lib.rs | 0.1 MOLT | **10 MOLT** ($1.00) | Anti-spam, still accessible |
| `REWARD_POOL_PER_MONTH` | dex_rewards/lib.rs | 1,000,000 MOLT | **500,000 MOLT** ($50K/mo) | More sustainable, extends pool life |
| `MIN_LISTING_LIQUIDITY` | dex_governance/lib.rs | 10 MOLT | **10,000 MOLT** ($1K) | Match the comment, prevent dust listings |

### Parameters that are fine as-is:

| Parameter | Value at $0.10 | Verdict |
|-----------|---------------|---------|
| Base tx fee ($0.0001) | Sub-penny, agent-friendly | Perfect for agents |
| Block reward ($0.09/block) | Meaningful but not excessive | Good |
| Contract deploy ($2.50) | Very affordable | Good |
| DAO proposal ($100) | Serious but fair | Good |
| Validator stake min ($1,000) | Accessible | Good |
| Validator stake max ($10,000) | Prevents concentration | Good |
| Taker fee 5bps (0.05%) | Competitive with Binance/Coinbase | Good |
| Maker rebate -1bp (-0.01%) | Attracts liquidity | Good |
| Lending LTV 75% | Industry standard | Good |
| Flash loan fee 0.09% | Matches Aave | Good |
| Marketplace fee 2.5% | Standard for NFTs | Good |

---

## 7. Genesis Distribution Conflict

**There are TWO different distributions in the code:**

### multisig.rs (used by validator code — CANONICAL)

```
validator_rewards:      150,000,000  (15%)
community_treasury:     400,000,000  (40%)
builder_grants:         250,000,000  (25%)
founding_moltys:        100,000,000  (10%)
ecosystem_partnerships:  50,000,000   (5%)
reserve_pool:            50,000,000   (5%)
```

### genesis.rs (different names + different amounts)

```
Community Treasury:        400,000,000  (40%)  ← matches
Validator Rewards Pool:    250,000,000  (25%)  ← CONFLICT (150M vs 250M)
Development Fund:          150,000,000  (15%)  ← CONFLICT (250M "builder_grants" vs 150M "dev fund")
Ecosystem Growth:          100,000,000  (10%)  ← different name, same amount as founding_moltys
Foundation Reserve:         50,000,000   (5%)  ← matches reserve_pool
Early Contributors:         50,000,000   (5%)  ← matches ecosystem_partnerships
```

**The validator code uses `REWARD_POOL_MOLT = 150,000,000` from multisig.rs.** This means the actual validator rewards pool is 150M, which is what `genesis_auto_deploy` uses.

**Resolution needed:** Align genesis.rs to match multisig.rs, or vice versa. The names and amounts must be consistent. The whitepaper/website should match exactly.

---

## 8. Other Discrepancies to Fix

### 1. Slashing: Downtime Penalty

- `genesis.rs`: 5% flat
- `consensus.rs apply_economic_slashing()`: 1% per 100 missed slots, max 10%
- **Recommendation:** Use the graduated approach from consensus.rs. Update genesis.rs feature flags to match.

### 2. DEX Governance Listing Liquidity

- Code: `MIN_LISTING_LIQUIDITY = 10_000_000_000` shells = 10 MOLT
- Comment: "10,000 MOLT equivalent"
- **Fix:** Change to `10_000_000_000_000` (10,000 MOLT) to match the comment. At $0.10, that's $1,000 minimum liquidity to list a token — reasonable.

### 3. DEX Rewards Source Wallet

- `REWARD_POOL_PER_MONTH` is defined but there's no defined source wallet
- Rewards claims don't actually transfer tokens
- **Fix (in DEX milestone):** Define that dex_rewards draws from the `builder_grants` wallet (250M MOLT), not validator rewards. Builder grants purpose = incentivize ecosystem growth, which trading rewards accomplish.

### 4. `ANNUAL_INFLATION_BPS = 500`

The code has a 5% annual inflation constant, but **MOLT is supposed to be non-inflationary**. Block rewards come from the pre-allocated validator rewards pool (150M), not newly minted tokens.

- **Clarify:** This constant should be renamed to `ANNUAL_REWARD_BPS` or `MAX_ANNUAL_REWARD_RATE` to reflect that it's a withdrawal rate from the pool, not inflation. No new MOLT is minted — it's distributed from the reward pool.

---

## Decision Matrix

After choosing the $0.10 price, the following parameters need code changes:

| # | Change | File | Priority |
|---|--------|------|----------|
| 1 | `MAX_ORDER_SIZE` → 10M MOLT | contracts/dex_core/src/lib.rs | Medium |
| 2 | `CREATION_FEE` → 10 MOLT | contracts/clawpump/src/lib.rs | Medium |
| 3 | `REWARD_POOL_PER_MONTH` → 500K MOLT | contracts/dex_rewards/src/lib.rs | Medium |
| 4 | `MIN_LISTING_LIQUIDITY` → 10K MOLT | contracts/dex_governance/src/lib.rs | Medium |
| 5 | Align genesis.rs distribution to multisig.rs | core/src/genesis.rs | **High** |
| 6 | Rename `ANNUAL_INFLATION_BPS` | core/src/consensus.rs | Low |
| 7 | Define reward source: builder_grants wallet | contracts/dex_rewards/src/lib.rs | **High** |
| 8 | Fix slashing discrepancy | core/src/genesis.rs | Low |

These changes become **Task 0** in the DEX Completion Milestone — run before Phase 1.
