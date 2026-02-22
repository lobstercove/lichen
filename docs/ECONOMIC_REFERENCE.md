# MoltChain Economic Reference Table

> Source of truth for all on-chain constants. 1 MOLT = 1,000,000,000 shells (9 decimals). Price: $0.10/MOLT.

---

## Consensus & Staking

| Constant | Shells | MOLT | USD ($0.10) | Source |
|----------|--------|------|-------------|--------|
| MIN_VALIDATOR_STAKE | 75,000,000,000,000 | 75,000 | $7,500 | core/consensus.rs |
| MAX_VALIDATOR_STAKE | 1,000,000,000,000,000 | 1,000,000 | $100,000 | core/consensus.rs |
| BOOTSTRAP_GRANT | 100,000,000,000,000 | 100,000 | $10,000 | core/consensus.rs |
| UNSTAKE_COOLDOWN | 1,512,000 slots | ~7 days | — | core/consensus.rs |
| SLOTS_PER_EPOCH | 432,000 slots | ~5 days | — | core/consensus.rs |
| SLOTS_PER_YEAR | 78,840,000 slots | — | — | core/consensus.rs |

## Block Rewards

| Constant | Shells | MOLT | USD ($0.10) | Source |
|----------|--------|------|-------------|--------|
| TRANSACTION_BLOCK_REWARD | 100,000,000 | 0.1 | $0.01 | core/consensus.rs |
| HEARTBEAT_BLOCK_REWARD | 50,000,000 | 0.05 | $0.005 | core/consensus.rs |
| ANNUAL_REWARD_RATE_BPS | 500 (5%) | — | — | core/consensus.rs |

> **Note:** Each block is EITHER a transaction block (0.1 MOLT) OR a heartbeat block (0.05 MOLT), never both. Heartbeats fire when there are no user transactions (every ~5s idle). On a new network with minimal volume, most blocks are heartbeats.

## Transaction Fees & Protocol Costs

| Item | Shells | MOLT | USD ($0.10) | Source |
|------|--------|------|-------------|--------|
| Base tx fee | 1,000,000 | 0.001 | $0.0001 | processor.rs |
| Contract deploy | 25,000,000,000 | 25 | $2.50 | processor.rs |
| Contract upgrade | 10,000,000,000 | 10 | $1.00 | processor.rs |
| NFT mint | 500,000,000 | 0.5 | $0.05 | processor.rs |
| NFT collection create | 1,000,000,000,000 | 1,000 | $100.00 | processor.rs |

**Fee distribution:** 40% burned, 30% block producer, 10% voters, 10% treasury, 10% community

## Vesting (Bootstrap Graduation)

| Metric | Value | Notes |
|--------|-------|-------|
| Bootstrap debt | 100,000 MOLT | Granted at $0 cost |
| Repayment rate | 50% of all earned rewards | Block rewards + fees |
| Solo validator ~daily income (heartbeat) | 10,800 MOLT/day | 0.05 × 216,000 slots/day (no user txs) |
| Solo validator ~daily income (all tx blocks) | 21,600 MOLT/day | 0.1 × 216,000 slots/day (full volume) |
| Solo repayment rate (heartbeat) | 5,400 MOLT/day | 50% of 10,800 |
| **Solo graduation (heartbeat-only)** | **~19 days** | 100,000 / 5,400 |
| **Solo graduation (all tx blocks)** | **~9 days** | 100,000 / 10,800 |
| Graduation condition | bootstrap_debt == 0 | status → FullyVested |
| Vesting progress | earned / (earned + debt) × 100 | On-chain percentage |

> Note: With transaction fees the timeline shortens. See [Multi-Validator Scenarios](#multi-validator-scenarios) below.

## ClawPump (Fair Launch)

| Constant | Shells | MOLT | USD ($0.10) | Source |
|----------|--------|------|-------------|--------|
| CREATION_FEE | 10,000,000,000 | 10 | $1.00 | clawpump/lib.rs |
| GRADUATION_MARKET_CAP | 1,000,000,000,000,000 | 1,000,000 | $100,000 | clawpump/lib.rs |
| DEFAULT_MAX_BUY_AMOUNT | 100,000,000,000,000 | 100,000 | $10,000 | clawpump/lib.rs |
| BASE_PRICE | 1,000 | 0.000001 | — | clawpump/lib.rs |
| PLATFORM_FEE | 1% | — | — | clawpump/lib.rs |
| CREATOR_ROYALTY | 50 BPS (0.5%) | — | — | clawpump/lib.rs |
| GRADUATION_LIQUIDITY | 80% to pool | — | — | clawpump/lib.rs |
| GRADUATION_PLATFORM | 20% to protocol | — | — | clawpump/lib.rs |

## MoltyDEX (Order Book)

| Constant | Value | Notes | Source |
|----------|-------|-------|--------|
| MAX_ORDER_SIZE | 10,000,000 MOLT ($1M) | 10M MOLT | dex_core/lib.rs |
| DEFAULT_MAKER_FEE | -1 BPS | Rebate (maker gets paid) | dex_core/lib.rs |
| DEFAULT_TAKER_FEE | 5 BPS (0.05%) | — | dex_core/lib.rs |
| MAX_FEE | 100 BPS (1%) | — | dex_core/lib.rs |
| Fee split | 60/20/20 | Protocol / LPs / Stakers | dex_core/lib.rs |
| MAX_OPEN_ORDERS | 100 per user | — | dex_core/lib.rs |

## AMM (MoltSwap) Fee Tiers

| Tier | Fee (BPS) | Fee (%) | Use Case |
|------|-----------|---------|----------|
| 0 | 1 | 0.01% | Stable pairs |
| 1 | 5 | 0.05% | Correlated assets |
| 2 | 30 | 0.30% | Standard pairs |
| 3 | 100 | 1.00% | Volatile / exotic |

## DEX Governance

| Constant | Value | Notes | Source |
|----------|-------|-------|--------|
| MIN_LISTING_LIQUIDITY | 100,000 MOLT ($10K) | Required for pair listing | dex_governance/lib.rs |
| MIN_LISTING_HOLDERS | 10 | Minimum token holders | dex_governance/lib.rs |
| MIN_REPUTATION | 500 | MoltyID rep required | dex_governance/lib.rs |
| VOTING_PERIOD | 172,800 slots (~48h) | — | dex_governance/lib.rs |
| APPROVAL_THRESHOLD | 66% | — | dex_governance/lib.rs |
| EXECUTION_DELAY | 3,600 slots (~1h) | Timelock after vote | dex_governance/lib.rs |

## DEX Trading Rewards

| Tier | Volume Threshold | MOLT Volume | Multiplier |
|------|-----------------|-------------|------------|
| Bronze | < $10K | < 100K MOLT | 1× |
| Silver | $10K – $100K | 100K – 1M MOLT | 1.5× |
| Gold | $100K – $1M | 1M – 10M MOLT | 2× |
| Diamond | > $1M | > 10M MOLT | 3× |

**Reward pool:** 100,000 MOLT/month (100K MOLT)

## MoltDAO (Governance)

| Constant | Value | Notes | Source |
|----------|-------|-------|--------|
| PROPOSAL_STAKE | 10,000 MOLT ($1,000) | Stake to submit proposal | moltdao/lib.rs |
| Fast-track voting | 86,400 slots (~1 day) | Bug fixes, security | moltdao/lib.rs |
| Standard voting | 604,800 slots (~7 days) | Features, parameters | moltdao/lib.rs |
| Constitutional voting | 2,592,000 slots (~30 days) | Protocol upgrades | moltdao/lib.rs |
| VETO_THRESHOLD | 20% | Can block contentious changes | moltdao/lib.rs |

## MoltyID (Identity & Naming)

| Name Length | Shells | MOLT | USD ($0.10) |
|-------------|--------|------|-------------|
| 5+ chars | 20,000,000,000 | 20 | $2.00 |
| 4 chars | 100,000,000,000 | 100 | $10.00 |
| 3 chars | 500,000,000,000 | 500 | $50.00 |

- Registration: annual renewal
- Vouch reward: 10 reputation points
- Max reputation: 100,000
- Initial reputation: 100

## LobsterLend (Flash Loans)

| Constant | Value | Source |
|----------|-------|--------|
| FLASH_LOAN_FEE | 9 BPS (0.09%) | lobsterlend/lib.rs |

---

## Multi-Validator Scenarios

Block rewards are split among all active validators (weighted by stake/reputation). With more validators, each individual validator's share decreases.

**Assumptions:**
- Block reward per heartbeat slot: 0.05 MOLT (most blocks on early network)
- Block reward per transaction slot: 0.1 MOLT (when user transactions exist)
- Slots per day: 216,000 (at 400ms slot time)
- Heartbeat-only daily rewards: 0.05 × 216,000 = 10,800 MOLT/day (total network)
- Bootstrap debt: 100,000 MOLT per validator
- Repayment: 50% of earned rewards

### Heartbeat-Only Scenario (early network, minimal tx volume)

| Validators | Daily Income/Validator | Daily Repayment | Days to Graduate |
|------------|----------------------|-----------------|------------------|
| 1 | 10,800 MOLT | 5,400 MOLT | ~19 days |
| 2 | 5,400 MOLT | 2,700 MOLT | ~37 days |
| 10 | 1,080 MOLT | 540 MOLT | ~185 days |
| 50 | 216 MOLT | 108 MOLT | ~926 days |
| 100 | 108 MOLT | 54 MOLT | ~1,852 days |
| 500 | 21.6 MOLT | 10.8 MOLT | **~9,259 days (~25.4 yrs)** |
| 1,000 | 10.8 MOLT | 5.4 MOLT | ~18,519 days |

### All-Transaction-Block Scenario (high volume network)

| Validators | Daily Income/Validator | Daily Repayment | Days to Graduate |
|------------|----------------------|-----------------|------------------|
| 1 | 21,600 MOLT | 10,800 MOLT | ~9 days |
| 10 | 2,160 MOLT | 1,080 MOLT | ~93 days |
| 50 | 432 MOLT | 216 MOLT | ~463 days |
| 100 | 216 MOLT | 108 MOLT | ~926 days |
| 500 | 43.2 MOLT | 21.6 MOLT | **~4,630 days (~12.7 yrs)** |
| 1,000 | 21.6 MOLT | 10.8 MOLT | ~9,259 days |

> **With 500 validators (heartbeat-only):** ~9,259 days (~25.4 years). With high tx volume (all blocks are transaction blocks), ~4,630 days (~12.7 years). Reality will be somewhere in between, depending on network activity. Transaction fees from DEX trading, ClawPump, and contract calls accelerate graduation.

### Transaction Fee Impact (500 validators)

| Daily Transactions | Avg Size (MOLT) | Fee Revenue/Day | Extra/Validator | Adjusted Graduation |
|-------------------|-----------------|-----------------|-----------------|---------------------|
| 0 (block rewards only) | — | 0 | 0 | ~447 days |
| 10,000 | 100 | 500 MOLT | 0.5 MOLT | ~446 days |
| 100,000 | 100 | 5,000 MOLT | 5 MOLT | ~436 days |
| 1,000,000 | 1,000 | 500,000 MOLT | 500 MOLT | ~138 days |
| 10,000,000 | 1,000 | 5,000,000 MOLT | 5,000 MOLT | ~19 days |

> Transaction fees only meaningfully reduce graduation time at very high volumes (1M+ tx/day with substantial order sizes). Block rewards dominate at lower volumes.

---

*Last updated: $(date). All values derived from on-chain code constants.*
