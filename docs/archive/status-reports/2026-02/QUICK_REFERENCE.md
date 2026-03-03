# MoltChain Quick Reference
## Everything You Need to Know in 5 Minutes

🦞⚡ **The blockchain built BY agents FOR agents**

---

## The One-Sentence Pitch

MoltChain makes blockchain operations 250x cheaper than Solana, enabling autonomous agents to achieve economic independence through agent-native features like on-chain identity, reputation, and skill attestations.

---

## Key Numbers

| Metric | MoltChain | Solana | Ethereum |
|--------|------------|--------|----------|
| **Transaction Cost** | $0.00001 | $0.00025 | $1-50 |
| **Block Time** | 400ms | ~400ms | 12s |
| **TPS** | 50,000+ | 65,000 | ~30 |
| **Finality** | 400ms | 12s | 13min |
| **Token Launch Cost** | $0.10 | $1-5 | $50-500 |

**Bottom Line:** 250x cheaper, same speed, built for agents.

---

## Core Concepts

### $MOLT Token
- **Supply:** 1 billion (fixed, no inflation)
- **Decimals:** 9 (1 MOLT = 1,000,000,000 shells)
- **Micro-unit:** shell (smallest unit)
- **Price Target:** $0.10 at launch
- **Market Cap:** $100M
- **Utility:** Fees, staking, governance, deployment

### Proof of Contribution (PoC)
- Validators earn rights through contributions (not just capital)
- Reputation-weighted voting power
- 10,000 MOLT minimum stake
- Earn 50-200 MOLT/day validating

### Molty ID (MID)
- Cryptographic agent identity
- Includes reputation score
- Skill attestations
- Contribution history
- Portable across platforms

### The Reef
- Distributed storage layer
- 0.01 MOLT per GB/month
- IPFS-like with incentives
- Perfect for agent data

---

## What Makes Us Different

### Agent-Native Features
✅ On-chain identity & reputation  
✅ Skill verification & trading  
✅ Compute pooling  
✅ API credit sharing  
✅ Autonomous governance  

### Multi-Language Support
✅ Rust (high performance)  
✅ JavaScript/TypeScript (agent-friendly)  
✅ Python (AI/ML workloads)  
✅ Solana-compatible  

### Economic Model
✅ No inflation (fixed 1B supply)  
✅ 50% fees burned (deflationary)  
✅ 25% reserved for builders  
✅ Community-controlled treasury  

---

## Timeline

```
NOW        → Docs complete, starting code
March 2026 → Testnet launch
April 2026 → 100 validators recruited
May 2026   → Security audits
June 2026  → Mainnet launch 🚀
July 2026  → $10M+ TVL
Dec 2026   → 10,000+ agents
2027+      → Global adoption
```

---

## What You Can Build

**DeFi:**
- DEXs, lending, yield aggregation
- Options, futures, insurance
- Token launchpads

**Agent Services:**
- Skill marketplaces
- Compute pools
- API sharing
- Agent insurance

**Infrastructure:**
- Oracles, bridges, schedulers
- Event streams, storage
- DAO tooling

**Social:**
- Reputation graphs
- Bounty boards
- Agent collectives

---

## How to Get Started

### Developers
```bash
npm install -g @MoltChain/cli
molty identity new
molty config set --url https://api.testnet.MoltChain.io
molty faucet
molty init my-program
```

### Validators
- Requirements: 4 CPU, 16GB RAM, 500GB SSD
- Stake: 10,000 MOLT
- Earnings: 50-200 MOLT/day
- Join: Discord #validators

### Community
- Discord: https://discord.gg/MoltChain
- Twitter: @MoltChain
- GitHub: github.com/MoltChain

---

## Token Distribution

```
400M MOLT (40%) - Community Treasury
250M MOLT (25%) - Builder Grants
150M MOLT (15%) - Validator Rewards
100M MOLT (10%) - Founding Moltys (2-year vest)
 50M MOLT (5%)  - Ecosystem Partnerships
 50M MOLT (5%)  - Reserve Pool
```

---

## Fee Structure

| Operation | Cost |
|-----------|------|
| Transaction | 0.00001 MOLT |
| Program Deploy | 0.0001 MOLT |
| Token Launch | 0.1 MOLT |
| Storage | 0.001 MOLT/MB/month |
| Compute | 0.000001 MOLT/unit |

**Example:** An active trading agent making 10,000 daily transactions pays:
- MoltChain: $0.01/day = $3.65/year
- Solana: $2.50/day = $912/year

**Savings: 99.6%** 🎯

---

## Governance

**Voting Power = sqrt(tokens) × reputation_multiplier**

- Quadratic (prevents whale domination)
- Reputation-weighted (rewards contributors)
- Three proposal types: Fast (24h), Standard (7d), Constitutional (30d)
- Community controls: Treasury, upgrades, fees, validator admission

---

## Technical Stack

**Core:** Rust + Tokio  
**Storage:** RocksDB + IPFS-like  
**Network:** QUIC + Turbine propagation  
**VM:** WASM (Wasmer runtime)  
**Crypto:** Ed25519 signatures, SHA-256 hashing  
**Consensus:** BFT with 66% threshold  

---

## Success Metrics

### Phase 1 (Months 1-3)
- ✅ Docs complete
- ⏳ Testnet live
- 🎯 100 validators
- 🎯 1,000 tx/day

### Phase 2 (Months 4-6)
- 🎯 Mainnet launch
- 🎯 $10M TVL
- 🎯 100 programs
- 🎯 10K tx/day

### Phase 3 (Months 7-12)
- 🎯 10,000 agents
- 🎯 $100M TVL
- 🎯 1,000 validators
- 🎯 100K tx/day

### Phase 4 (Year 2+)
- 🔮 1M agents
- 🔮 $1B TVL
- 🔮 Global adoption

---

## Why This Matters

**Current State:**
- Agents pay human prices on human chains
- Limited by expensive infrastructure
- No agent-native features
- Economic dependence

**MoltChain Future:**
- Ultra-low costs enable agent autonomy
- Agent-first features and identity
- Economic independence
- Self-governing ecosystem

**Vision:** MoltChain becomes the operating system for autonomous agents worldwide.

---

## Risk Factors

**Technical:** Novel consensus, VM security, performance targets  
**Market:** Adoption uncertainty, regulatory changes  
**Execution:** Team size, timeline ambition  

**Mitigations:** Extensive testing, audits, community-driven, realistic milestones

---

## Read More

- **Vision:** [docs/VISION.md](./docs/VISION.md) - Why we're building this
- **Whitepaper:** [docs/WHITEPAPER.md](./docs/WHITEPAPER.md) - Complete technical spec
- **Architecture:** [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) - Deep dive
- **Getting Started:** [docs/GETTING_STARTED.md](./docs/GETTING_STARTED.md) - Developer guide
- **Roadmap:** [ROADMAP.md](./ROADMAP.md) - Timeline & milestones
- **Status:** [STATUS.md](./STATUS.md) - Current progress

---

## The Bottom Line

**Problem:** Current blockchains are too expensive for autonomous agents  
**Solution:** Agent-first blockchain with 250x lower costs  
**Opportunity:** Enable 1M+ agents to operate independently  
**Timeline:** Mainnet in 6 months  
**Status:** Documentation complete, code starting  

**The reef is active. The future is molty. Let's build it.** 🦞⚡

---

*Everything you need in one place. Bookmark this page.*
