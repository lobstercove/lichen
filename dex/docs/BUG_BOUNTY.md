# MoltyDEX Bug Bounty Program

## Scope

### In Scope
| Component | Severity Range | Notes |
|-----------|---------------|-------|
| DEX Smart Contracts (10) | Critical–Low | `dex_core`, `dex_amm`, `dex_router`, `dex_margin`, `dex_governance`, `dex_rewards`, `dex_analytics`, `musd_token`, `wsol_token`, `weth_token` |
| Core Contracts (16) | Critical–Low | `moltcoin`, `moltswap`, `reef_storage`, etc. |
| RPC Server | High–Low | REST API, WebSocket, JSON-RPC |
| Custody Bridge | Critical–High | Cross-chain asset management |
| TypeScript SDK | Medium–Low | Client-side logic |

### Out of Scope
- Frontend UI/UX issues (CSS, layout)
- Denial of Service (unless amplification)
- Social engineering
- Third-party services (Solana, Ethereum nodes)
- Test environment issues

---

## Severity Levels

### Critical (up to $50,000 MOLT equivalent)
- Loss of user funds
- Unauthorized minting of tokens
- Insurance fund drain
- Oracle price manipulation leading to bad liquidations
- Cross-chain bridge theft
- Arbitrary contract execution

### High (up to $15,000 MOLT equivalent)  
- Incorrect margin calculations leading to wrong liquidations
- AMM pool share manipulation
- Order book price manipulation (non-oracle)
- Governance vote manipulation
- Reward distribution errors

### Medium (up to $5,000 MOLT equivalent)
- Incorrect fee calculations
- LP position accounting errors
- Analytics data manipulation
- Race conditions in order matching
- Integer overflow/underflow not caught by checked math

### Low (up to $1,000 MOLT equivalent)
- Gas/compute optimization issues
- Non-critical state inconsistencies
- API response format issues
- Minor precision loss in calculations

---

## Submission Process

### How to Submit
1. **Email**: security@moltchain.io
2. **Subject**: `[BUG-BOUNTY] <Severity> — <Brief Description>`
3. **Include**:
   - Affected contract/component
   - Step-by-step reproduction
   - Proof of concept (code/script)
   - Expected vs actual behavior
   - Impact assessment

### Response Timeline
| Stage | SLA |
|-------|-----|
| Acknowledgment | 24 hours |
| Triage & severity classification | 72 hours |
| Fix timeline estimate | 7 days |
| Bounty payment | 14 days after fix deployed |

---

## Rules

1. **No exploitation on mainnet** — use testnet only
2. **First reporter wins** — duplicate reports receive honorable mention only
3. **Responsible disclosure** — 90-day embargo before public disclosure
4. **No automated scanning** — manual research only
5. **Proof of concept required** — theoretical reports scored lower
6. **Bonuses**: 
   - +25% if you also provide a fix PR
   - +10% for detailed root cause analysis

---

## Known Issues (Not Eligible)

These are known and tracked — duplicate reports will not receive bounty:

1. ~~Integer overflow in dex_margin `add_margin`~~ — Fixed (checked_add)
2. ~~Insurance fund overflow~~ — Fixed (saturating_add)
3. ~~dex_amm compute_liquidity u128 overflow~~ — Fixed (checked_mul chain)
4. ~~No minimum deposit validation~~ — Fixed (MIN_AMOUNT=100)

---

## Previous Payouts

| Date | Severity | Description | Reward |
|------|----------|-------------|--------|
| — | — | Program launches with testnet | — |

---

## Contact

- Security email: security@moltchain.io
- PGP key: (to be published)
- Response team: MoltChain core developers
