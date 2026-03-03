# MoltChain — Finish-Line Plan: Consolidated Work Item Inventory

**Generated:** Based on analysis of 23 planning/audit/status documents  
**Purpose:** Single source of truth for all remaining work items, organized by priority

---

## How to Read This Document

Each item includes:
- **Source:** Document(s) where the item was identified
- **Component:** Code module / contract / system affected
- **Effort:** S (< 1 day), M (1–3 days), L (3–7 days), XL (1–3 weeks)
- **Status:** OPEN (not started), PARTIAL (some work done), BLOCKED (dependency)

Items marked ✅ in source documents are **excluded** — this inventory contains only **remaining** work.

---

## TIER 1 — CRITICAL / BLOCKING

> These items represent security vulnerabilities, broken functionality, or missing infrastructure without which the chain cannot safely launch.

### C1. Wrapped Token WASMs Are Empty (86 bytes)

| Field | Value |
|-------|-------|
| Source | DEX_COMPLETION_MILESTONE §1.1 |
| Component | `contracts/wsol_token`, `contracts/weth_token`, `contracts/musd_token` |
| Effort | **S** |
| Description | wsol_token, weth_token, and musd_token compile to 86-byte empty WASMs due to missing `#[no_mangle] pub extern "C"` annotations. These three tokens are non-functional on-chain. mUSD is the DEX quote currency — nothing works without it. |

### C2. Genesis Contracts Never Initialized

| Field | Value |
|-------|-------|
| Source | DEX_COMPLETION_MILESTONE §1.2, PRODUCTION_AUDIT_ALL_CONTRACTS |
| Component | `validator/src/main.rs` (genesis_auto_deploy), all 26 contracts |
| Effort | **L** |
| Description | All 26 contracts are deployed at genesis but their `initialize()` is never called. Storage is empty — no admin set, no configuration, no state. Every contract is inert until initialized. Requires adding Phase 2 (initialize) and Phase 3 (create trading pairs) to `genesis_auto_deploy()`. |

### C3. moltcoin approve() / mint() Missing Caller Verification

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #2, #3 |
| Component | `contracts/moltcoin/src/lib.rs` |
| Effort | **S** |
| Description | `approve()` has no caller verification — any account can set allowances for any other account. `mint()` uses a parameter as the caller identity rather than `get_caller()` — owner is spoofable. Combined, these allow total token theft of the native MOLT token. |

### C4. dex_amm tick_to_sqrt_price Uses Linear Approximation

| Field | Value |
|-------|-------|
| Source | PRODUCTION_AUDIT_ALL_CONTRACTS |
| Component | `contracts/dex_amm/src/lib.rs` |
| Effort | **M** |
| Description | The concentrated-liquidity AMM uses a linear approximation for `tick_to_sqrt_price` instead of the correct exponential formula (`1.0001^(tick/2)`). This produces incorrect prices at all ticks, making the AMM fundamentally broken for any real trading. |

### C5. prediction_market — No Actual Token Transfers

| Field | Value |
|-------|-------|
| Source | PRODUCTION_AUDIT_ALL_CONTRACTS |
| Component | `contracts/prediction_market/src/lib.rs` |
| Effort | **M** |
| Description | The prediction market tracks share balances and collateral as pure accounting entries but never calls `call_token_transfer` for mUSD deposits or payouts. Users' mUSD balances are never debited on buy or credited on redemption — the contract is virtual-only. |

### C6. dex_router Simulation Fallback in Production

| Field | Value |
|-------|-------|
| Source | PRODUCTION_AUDIT_ALL_CONTRACTS, DEX_COMPLETION_MILESTONE §2.5 |
| Component | `contracts/dex_router/src/lib.rs` |
| Effort | **M** |
| Description | The router's swap execution uses a simulation fallback that fabricates output amounts when cross-contract calls fail (which they always do, since cross-contract calls are stubs). Production swaps return simulated values, not real token movements. |

### C7. compute_market Missing get_caller() on 5 Admin Functions

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #1 |
| Component | `contracts/compute_market/src/lib.rs` |
| Effort | **S** |
| Description | Five administrative functions (set_fee_rate, set_min_stake, set_max_duration, pause, unpause) accept a caller parameter without verifying via `get_caller()`. Anyone can modify compute market configuration or pause/unpause the contract. |

### C8. moltoracle submit_price — No Caller Verification

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #14, DEX_COMPLETION_MILESTONE §3.1 |
| Component | `contracts/moltoracle/src/lib.rs` |
| Effort | **S** |
| Description | `submit_price` accepts a feeder address as a parameter without `get_caller()` verification. Anyone can submit oracle prices as any authorized feeder. Additionally, `simple_hash` is not cryptographic — VRF is vulnerable. Both issues allow oracle price manipulation, which cascades to DEX margin liquidations, prediction market resolutions, and lending rates. |

### C9. Genesis Distribution Mismatch

| Field | Value |
|-------|-------|
| Source | DEX_COMPLETION_MILESTONE §0.1, TOKENOMICS §7 |
| Component | `core/src/genesis.rs`, `core/src/multisig.rs` |
| Effort | **S** |
| Description | `genesis.rs` assigns 250M MOLT to validator rewards and 150M to builder grants. `multisig.rs` (the canonical source) specifies the reverse: 150M to validators, 250M to builders. The validator code uses multisig.rs values. genesis.rs must be aligned to match — the 100M discrepancy affects reward pool lifespan and builder incentive budget. |

### C10. Most Contracts Are Pure Accounting — No Token Transfers

| Field | Value |
|-------|-------|
| Source | PRODUCTION_AUDIT_ALL_CONTRACTS (cross-cutting) |
| Component | Most contracts: `bountyboard`, `clawpay`, `clawvault`, `dex_rewards`, `lobsterlend`, `moltauction`, `clawpump`, `compute_market` |
| Effort | **XL** |
| Description | Cross-contract `call_token_transfer` is a stub (returns 0). Contracts that handle payments — lending, rewards, bounties, auctions, streaming payments, vaults — track balances internally but never move actual tokens. Until cross-contract calls work or a host-level transfer primitive is added, all financial contracts are virtual. This is the single largest systemic gap. |

### C11. dex_rewards Claim Doesn't Transfer MOLT

| Field | Value |
|-------|-------|
| Source | DEX_COMPLETION_MILESTONE §2.6, TOKENOMICS §2 |
| Component | `contracts/dex_rewards/src/lib.rs` |
| Effort | **M** |
| Description | Reward claims update internal bookkeeping but never transfer MOLT tokens. No source wallet is defined for reward payouts. At 1M MOLT/month emission, this is $100K/month at $0.10 — the second-largest emission after block rewards — with no mechanism to actually pay it out. Source should be `builder_grants` wallet (250M MOLT). |

### C12. dex_rewards initialize() Missing Caller Check

| Field | Value |
|-------|-------|
| Source | PRODUCTION_AUDIT_ALL_CONTRACTS |
| Component | `contracts/dex_rewards/src/lib.rs` |
| Effort | **S** |
| Description | `initialize()` can be called by anyone, allowing an attacker to seize admin control of the rewards contract and redirect emissions. |

---

## TIER 2 — HIGH PRIORITY

> Significant functionality gaps, security weaknesses that are exploitable but not immediately catastrophic, and features required for a usable product.

### H1. Stop-Loss / Take-Profit System Not Implemented

| Field | Value |
|-------|-------|
| Source | DEX_FINAL_PLAN Phase 2, DEX_PRODUCTION_PLAN F4.1 |
| Component | `contracts/dex_core`, `dex/dex.js`, `validator` |
| Effort | **L** |
| Description | Stop-limit orders are stubbed. Requires: extend `buildPlaceOrderArgs` with trigger price, add `check_triggers()` contract function, build trigger engine in validator tick loop. 8 sub-tasks. |

### H2. Governance execute_proposal Is a Placeholder

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #9, DEX_FINAL_PLAN Phase 6, DEX_COMPLETION_MILESTONE |
| Component | `contracts/moltdao/src/lib.rs`, `contracts/dex_governance/src/lib.rs` |
| Effort | **L** |
| Description | `execute_proposal()` sets status to "executed" but performs no cross-contract action. Governance votes have no on-chain effect. The finalize/execute lifecycle for both moltdao and dex_governance needs to be wired to actual state changes (parameter updates, listing approvals, fund disbursements). |

### H3. ClawPump Graduation Doesn't Migrate to DEX

| Field | Value |
|-------|-------|
| Source | DEX_COMPLETION_MILESTONE §2.7, SECURITY_AUDIT_REPORT #8 |
| Component | `contracts/clawpump/src/lib.rs` |
| Effort | **M** |
| Description | When a bonding-curve token reaches the graduation threshold (100K MOLT), the contract sets a "graduated" flag but doesn't create a DEX pair, AMM pool, or seed initial liquidity. Additionally, partial cross-call failure during graduation is not reverted. |

### H4. Collateral Locking Not Implemented at Host Level

| Field | Value |
|-------|-------|
| Source | DEX_COMPLETION_MILESTONE §2.3 |
| Component | `contracts/dex_margin/src/lib.rs`, `core/src/processor.rs` |
| Effort | **L** |
| Description | Margin positions require locking collateral (moving MOLT from spendable to locked balance), but no host-level primitive exists for this. Currently, a user can open a margin position and still spend the collateral in a separate transaction — total double-spend risk. |

### H5. Insurance Fund — No Withdrawal Mechanism

| Field | Value |
|-------|-------|
| Source | DEX_COMPLETION_MILESTONE §2.4 |
| Component | `contracts/dex_margin/src/lib.rs` |
| Effort | **M** |
| Description | The insurance fund accumulates liquidation penalties but has no governance-controlled withdrawal or deployment mechanism. Funds are permanently trapped. |

### H6. Funding Rate Not Implemented in Margin Contract

| Field | Value |
|-------|-------|
| Source | DEX_PRODUCTION_PLAN F10.16, DEX_FINAL_PLAN Phase 4 |
| Component | `contracts/dex_margin/src/lib.rs`, `dex/dex.js` |
| Effort | **M** |
| Description | Funding rates (periodic payments between long/short holders to keep perp prices aligned to spot) are not implemented. Without funding rates, perpetual futures prices can diverge arbitrarily from spot prices. |

### H7. Tokenomics Parameter Readjustment (30 parameters)

| Field | Value |
|-------|-------|
| Source | DEX_COMPLETION_MILESTONE §0.3, TOKENOMICS §6 |
| Component | Multiple contracts (`dex_core`, `dex_rewards`, `clawpump`, `dex_governance`), `core/src/processor.rs` |
| Effort | **M** |
| Description | 4 parameters confirmed needing change at $0.10/MOLT: `MAX_ORDER_SIZE` (1K→10M MOLT), `CREATION_FEE` (0.1→10 MOLT), `REWARD_POOL_PER_MONTH` (1M→500K MOLT), `MIN_LISTING_LIQUIDITY` (10→10K MOLT). Plus rename `ANNUAL_INFLATION_BPS` → `ANNUAL_REWARD_RATE_BPS`. |

### H8. dex_margin close_position Returns Full Margin on Missing Price

| Field | Value |
|-------|-------|
| Source | PRODUCTION_AUDIT_ALL_CONTRACTS |
| Component | `contracts/dex_margin/src/lib.rs` |
| Effort | **S** |
| Description | When the oracle mark price is unavailable, `close_position` returns the full initial margin to the trader regardless of actual P/L. If the oracle goes down, traders with losing positions can close at no loss. |

### H9. bountyboard cancel_bounty / approve_work — No Token Transfers

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #16, PRODUCTION_AUDIT_ALL_CONTRACTS, DEX_COMPLETION_MILESTONE §3.3 |
| Component | `contracts/bountyboard/src/lib.rs` |
| Effort | **M** |
| Description | `cancel_bounty` attempts a refund transfer but ignores failure. `approve_work` records payment internally but never transfers tokens to the worker. Bounty creators' funds are never returned on cancellation, and workers are never paid. |

### H10. clawpay — Missing Reentrancy Guards

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #10 |
| Component | `contracts/clawpay/src/lib.rs` |
| Effort | **S** |
| Description | Payment streaming contract lacks reentrancy protection. Cancel_stream makes no actual token transfers (SECURITY_AUDIT_REPORT #15), but once transfers are wired, reentrancy becomes exploitable. |

### H11. clawpay cancel_stream — No Token Transfer

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #15 |
| Component | `contracts/clawpay/src/lib.rs` |
| Effort | **S** |
| Description | Stream cancellation calculates the refund but never returns remaining funds to the stream creator. |

### H12. moltdao cancel_proposal — Missing get_caller()

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #10 |
| Component | `contracts/moltdao/src/lib.rs` |
| Effort | **S** |
| Description | `cancel_proposal` accepts a caller parameter without `get_caller()` verification. Anyone can cancel any governance proposal. |

### H13. moltauction create_auction — Missing get_caller()

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #18 |
| Component | `contracts/moltauction/src/lib.rs` |
| Effort | **S** |
| Description | Auction creation uses a parameter-provided creator address without verifying the actual transaction signer. Allows spoofing auction ownership. |

### H14. clawvault — u64 Overflow in Share Conversion & Fee Calculation

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #6, #7 |
| Component | `contracts/clawvault/src/lib.rs` |
| Effort | **S** |
| Description | Share-to-asset conversion (`shares * total_assets / total_shares`) and fee accumulation can overflow u64 with large values, causing incorrect share calculations or fee truncation. Use checked arithmetic or u128 intermediates. |

### H15. clawvault — Yield Generation Is Simulated

| Field | Value |
|-------|-------|
| Source | DEX_COMPLETION_MILESTONE §3.4 |
| Component | `contracts/clawvault/src/lib.rs` |
| Effort | **M** |
| Description | Vault APY is hardcoded and simulated rather than connected to real yield sources (LobsterLend interest, MoltSwap LP fees). The vault reports fake returns. |

### H16. Validator Graduation Plan — Not Implemented

| Field | Value |
|-------|-------|
| Source | VALIDATOR_GRADUATION_PLAN |
| Component | `validator/`, `core/src/consensus.rs` |
| Effort | **XL** |
| Description | Bootstrap grant mechanics (100K MOLT per validator, 50/50 reward split, 18-month time cap, 95%+ uptime bonus, machine fingerprint anti-fraud) are approved but no code has been written. Required for first 200 validators to join the network. |

### H17. Build Validation Incomplete

| Field | Value |
|-------|-------|
| Source | PRODUCTION_GAPS_TRACKER |
| Component | Build / CI pipeline |
| Effort | **L** |
| Description | Three critical build validation items remain: ⬜ Release build (optimized binary), ⬜ 3-validator testnet boot, ⬜ End-to-end test suite. `cargo check` and `cargo test` pass, but the full release pipeline has not been validated. |

### H18. compute_market resolve_dispute — Wrong Transfer Source

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #4, PRODUCTION_AUDIT_ALL_CONTRACTS |
| Component | `contracts/compute_market/src/lib.rs` |
| Effort | **S** |
| Description | Dispute resolution calculates fund redistribution but uses the wrong source account for the transfer, and the transfer itself never actually executes (cross-contract stub). |

### H19. compute_market — Paused State Returns 0 Instead of Error

| Field | Value |
|-------|-------|
| Source | PRODUCTION_AUDIT_ALL_CONTRACTS |
| Component | `contracts/compute_market/src/lib.rs` |
| Effort | **S** |
| Description | When paused, entry points return 0 (success code) instead of an error code. Callers believe operations succeeded when they were silently dropped. |

### H20. Post-Only / Reduce-Only Order Flags Not Wired

| Field | Value |
|-------|-------|
| Source | DEX_FINAL_PLAN Phase 3 |
| Component | `dex/dex.js`, `contracts/dex_core/src/lib.rs` |
| Effort | **M** |
| Description | Post-Only and Reduce-Only checkboxes exist in the UI but their values aren't included in the order placement transaction. Contract-side support exists but is unreachable. Plus: Cancel All, order modification, and confirmation dialog are not wired. 5 sub-tasks total. |

---

## TIER 3 — MEDIUM PRIORITY

> Quality, reliability, UX improvements, and architectural debt that should be addressed before or shortly after launch.

### M1. Unchecked Arithmetic Across ALL 14 Audited Contracts

| Field | Value |
|-------|-------|
| Source | PRODUCTION_AUDIT_ALL_CONTRACTS (cross-cutting) |
| Component | All 14 contracts audited (dex_core through compute_market) |
| Effort | **L** |
| Description | Fee accumulators, volume trackers, reward counters, and other u64 fields use unchecked addition. After ~584 years of operation at max values, individual counters overflow. More realistically, rapid trading volume or reward accrual on popular markets could trigger overflows in months. Use `saturating_add` or `checked_add` everywhere. |

### M2. DEX Bottom Panel Consolidation

| Field | Value |
|-------|-------|
| Source | DEX_FINAL_PLAN Phase 1 |
| Component | `dex/dex.js` |
| Effort | **M** |
| Description | Remove duplicate Positions/Margin tabs, add liquidation price column, PnL percentage, and margin management buttons (Add/Remove Margin) to the positions table. 5 sub-tasks. |

### M3. DEX Margin Enhancements (Partial Close, PnL Share Card)

| Field | Value |
|-------|-------|
| Source | DEX_FINAL_PLAN Phase 4 |
| Component | `dex/dex.js`, `contracts/dex_margin/src/lib.rs` |
| Effort | **M** |
| Description | Funding rate display (depends on H6), partial position close, PnL share card generation, cross-margin design document. 4 sub-tasks. |

### M4. DEX Settings & Preferences

| Field | Value |
|-------|-------|
| Source | DEX_FINAL_PLAN Phase 5 |
| Component | `dex/dex.js` |
| Effort | **M** |
| Description | Slippage tolerance setting, notification preferences, chart/pair memory across sessions. 4 sub-tasks. |

### M5. Cancelled Orders Never Pruned from Storage

| Field | Value |
|-------|-------|
| Source | PRODUCTION_AUDIT_ALL_CONTRACTS |
| Component | `contracts/dex_core/src/lib.rs` |
| Effort | **M** |
| Description | Cancelled/filled orders remain in contract storage indefinitely. Over time, storage grows unboundedly. Need a pruning mechanism or TTL-based cleanup. |

### M6. dex_analytics Candle Retention Never Enforced

| Field | Value |
|-------|-------|
| Source | PRODUCTION_AUDIT_ALL_CONTRACTS |
| Component | `contracts/dex_analytics/src/lib.rs` |
| Effort | **S** |
| Description | Candle retention policies are defined (24h for 1m, 7d for 5m, etc.) but never enforced — old candles are never deleted. Storage grows indefinitely. |

### M7. dex_amm — O(n) Fee Distribution

| Field | Value |
|-------|-------|
| Source | PRODUCTION_AUDIT_ALL_CONTRACTS |
| Component | `contracts/dex_amm/src/lib.rs` |
| Effort | **M** |
| Description | Fee distribution iterates over all liquidity positions. With many positions, gas cost scales linearly and can exceed block limits. Switch to a per-share accumulator pattern. |

### M8. Market Order Slippage Control Limited

| Field | Value |
|-------|-------|
| Source | PRODUCTION_READINESS_AUDIT M10 |
| Component | `contracts/dex_core/src/lib.rs` |
| Effort | **S** |
| Description | Market orders have limited slippage protection. A large market order against thin liquidity can result in extreme price impact with no circuit breaker. |

### M9. Rebalance Pricing / Slippage

| Field | Value |
|-------|-------|
| Source | PRODUCTION_READINESS_AUDIT M14 |
| Component | Custody / bridge pricing |
| Effort | **M** |
| Description | Custody Uniswap rebalancing lacks proper slippage controls and pricing checks. |

### M10. Custody — 6 Medium Recommendations Unfixed

| Field | Value |
|-------|-------|
| Source | CUSTODY_AUDIT_REPORT |
| Component | `custody/src/` |
| Effort | **L** |
| Description | Six medium-severity recommendations remain: (1) full table scans in hot path, (2) master seed stored in environment variable, (3) single master seed for all chains, (4) crash-restart idempotency not guaranteed, (5) `std::sync::Mutex` used in async context (deadlock risk), (6) hardcoded gas limits. |

### M11. ABI Mixed Return Code Conventions in moltauction

| Field | Value |
|-------|-------|
| Source | ABI_CONFORMANCE_AUDIT |
| Component | `contracts/moltauction/src/lib.rs` |
| Effort | **S** |
| Description | Some functions return 0 for success, others return 1 for success, within the same contract. Callers cannot reliably determine success/failure. Standardize to 1=success across all functions. |

### M12. lobsterlend — Withdraw Blocked During Pause

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #12 |
| Component | `contracts/lobsterlend/src/lib.rs` |
| Effort | **S** |
| Description | Emergency pause blocks all operations including withdrawals, trapping user funds. Withdrawals should be allowed even when paused. |

### M13. lobsterlend — u64 Overflow in Borrow Calculation

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #17 |
| Component | `contracts/lobsterlend/src/lib.rs` |
| Effort | **S** |
| Description | Borrow interest calculation can overflow u64 for large positions or long durations. |

### M14. lobsterlend — Query Functions Incompatible with JSON ABI

| Field | Value |
|-------|-------|
| Source | ABI_CONFORMANCE_AUDIT |
| Component | `contracts/lobsterlend/src/lib.rs` |
| Effort | **M** |
| Description | Query functions use output pointers incompatible with the JSON ABI encoder. CLI/SDK cannot call these queries through the standard ABI path. |

### M15. No Structured Event/Log System

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #20 |
| Component | All contracts, `core/src/processor.rs` |
| Effort | **L** |
| Description | No contract emits structured events. Off-chain indexers and UIs rely on polling storage. A proper event emission system (log topics + data) would enable WebSocket subscriptions, transaction receipts with event logs, and third-party indexing. |

### M16. MoltyID Wallet Integration — Zero UI

| Field | Value |
|-------|-------|
| Source | MOLTYID_WALLET_PLAN |
| Component | `explorer/address.html`, `rpc/src/lib.rs` |
| Effort | **XL** |
| Description | MoltyID contract is deployed with 37 WASM functions (naming, reputation, skills, vouching, discovery) but has zero UI integration and zero RPC endpoints. Requires: 12+ new RPC methods, address.html tab restructure, Identity tab, .molt name resolution across all explorer pages, agent directory page. 7 implementation phases (A–G). |

### M17. Prediction Markets — Full Implementation Needed

| Field | Value |
|-------|-------|
| Source | PREDICTION_MARKETS_PLAN |
| Component | `contracts/prediction_market/src/lib.rs`, `rpc/`, `dex/dex.js` |
| Effort | **XL** |
| Description | PredictionReef plan is complete and detailed (contract design, AMM math, resolution flow, UI mockups, 80+ tests planned) but the existing prediction_market contract has critical gaps: no token transfers (C5), no real resolution flow. Full implementation across 5 phases (A–E): core contract rewrite (~2500 lines), resolution & settlement, multi-outcome, integration & UI, polish & launch. |

### M18. dex_governance — Accepts Caller-Provided Reputation

| Field | Value |
|-------|-------|
| Source | DEX_COMPLETION_MILESTONE §3.2 |
| Component | `contracts/dex_governance/src/lib.rs` |
| Effort | **S** |
| Description | Governance contract accepts reputation score as a transaction parameter rather than reading it on-chain from MoltyID. Any user can claim arbitrary reputation to bypass voting thresholds. Needs on-chain cross-contract verification. |

### M19. compute_market cancel_job — Timeout from Wrong Slot

| Field | Value |
|-------|-------|
| Source | SECURITY_AUDIT_REPORT #5 |
| Component | `contracts/compute_market/src/lib.rs` |
| Effort | **S** |
| Description | Job cancellation timeout is calculated from `created_slot` instead of `claim_slot`, allowing providers to be penalized for jobs that sat unclaimed for a long time. |

### M20. RPC MoltyID Reputation Reads Stale ContractAccount

| Field | Value |
|-------|-------|
| Source | CF_CONTRACT_STORAGE Prefix Audit (Feb 20 2026) |
| Component | `rpc/src/lib.rs` (`handle_get_moltyid_reputation`, `handle_get_moltyid_identity`) |
| Effort | **S** |
| Description | RPC handlers read reputation from the deserialized `ContractAccount.storage` BTreeMap, but validator-side writes (admin bootstrap at 5000, DEX analytics, order book) go only to `CF_CONTRACT_STORAGE`. The two can diverge — e.g. `state.get_reputation()` returns 5000 (correct) while the RPC endpoint returns 100 (stale). Fix: change RPC handlers to read from `CF_CONTRACT_STORAGE` via `state.get_contract_storage()`, or dual-write in validator. |

### M21. Slashing Discrepancy

| Field | Value |
|-------|-------|
| Source | TOKENOMICS §8 |
| Component | `core/src/genesis.rs`, `core/src/consensus.rs` |
| Effort | **S** |
| Description | genesis.rs defines 5% flat downtime penalty; consensus.rs implements 1% per 100 missed slots, max 10%. Need to align to the graduated approach. |

---

## TIER 4 — LOW PRIORITY / FUTURE

> Enhancement features, long-term plans, polish items, and tracked minor issues. Not required for launch.

### L1. Phase 2 Agent Economy — 7 New Contracts

| Field | Value |
|-------|-------|
| Source | PHASE2_AGENT_ECONOMY |
| Component | New contracts (not yet written) |
| Effort | **XL** (3,690 lines estimated, ~96 functions) |
| Description | AI Marketplace (highest priority), Time Lock, Cross-Chain Messaging, Social Protocol, Insurance Protocol, Content Protocol, Supply Chain. All in planning stage, none implemented. |

### L2. ZK Privacy Layer — Complete Rewrite Required

| Field | Value |
|-------|-------|
| Source | ZK_PRIVACY_IMPLEMENTATION_PLAN |
| Component | `core/src/privacy.rs` (to be replaced by `core/src/zk/`), new `contracts/shielded_pool/` |
| Effort | **XL** (8–12 weeks per the plan) |
| Description | Current privacy.rs is a dead placeholder — `verify_proof()` uses HMAC-SHA256 with public data (trivially forgeable), no ZK crate dependencies, no Merkle tree, no Pedersen commitments. Full implementation plan exists: 7 phases covering Groth16 circuits, trusted setup ceremony, on-chain shielded pool, wallet proving, security hardening. Not blocking launch. |

### L3. MoltyID Vision — .molt Naming, Agent Discovery, Web-of-Trust

| Field | Value |
|-------|-------|
| Source | MOLTYID_VISION |
| Component | `contracts/moltyid/src/lib.rs` (contract is deployed), `rpc/`, `explorer/` |
| Effort | **XL** |
| Description | Vision for MoltyID as universal AI agent identity layer. Contract already supports naming, reputation, skills, vouching, discovery (37 exports). Missing: priority lane sorting in block production, trust-tier fee discounts in processor (partially wired), cross-contract auth SDK, `moltchain.id` DNS portal. Overlaps with M16. |

### L4. Cross-Margin Mode

| Field | Value |
|-------|-------|
| Source | DEX_FINAL_PLAN Phase 4 |
| Component | `contracts/dex_margin/src/lib.rs` |
| Effort | **L** |
| Description | Cross-margin (shared collateral across all positions) as opposed to current isolated-margin model. Design document only — no implementation planned for v1. |

### L5. PnL Share Card

| Field | Value |
|-------|-------|
| Source | DEX_FINAL_PLAN Phase 4 |
| Component | `dex/dex.js` |
| Effort | **S** |
| Description | Generate shareable image card showing closed position P/L for social sharing. Pure frontend feature. |

### L6. Liquidation Penalty Remainder Loss

| Field | Value |
|-------|-------|
| Source | NEW_FINDINGS_AUDIT NEW-L1 |
| Component | `contracts/dex_margin/src/lib.rs` |
| Effort | **S** |
| Description | When liquidation penalty is split between liquidator and insurance fund, integer division remainder (up to 1 shell) is lost. Confirmed LOW — dust amounts. |

### L7. Flash-Loan Fee Precision for Small Loans

| Field | Value |
|-------|-------|
| Source | NEW_FINDINGS_AUDIT NEW-M2 |
| Component | `contracts/lobsterlend/src/lib.rs` |
| Effort | **S** |
| Description | Flash loan fee (0.09%) truncates to 0 for very small loans (< 1,112 shells). Allows fee-free flash loans under the threshold. Downgraded to LOW. |

### L8. First-Caller-Wins Admin Init Pattern

| Field | Value |
|-------|-------|
| Source | PRODUCTION_AUDIT_ALL_CONTRACTS |
| Component | `contracts/bountyboard`, `contracts/compute_market`, `contracts/reef_storage` |
| Effort | **S** |
| Description | These contracts use a first-caller-wins pattern for `initialize()` — whoever calls it first becomes admin. If genesis initialization (C2) is implemented correctly, this is mitigated since initialization happens in the genesis block before any user transactions. |

### L9. Prediction Market CLOB Integration

| Field | Value |
|-------|-------|
| Source | PREDICTION_MARKETS_PLAN §10, §20 |
| Component | `contracts/prediction_market`, `contracts/dex_core` |
| Effort | **L** |
| Description | Optional CLOB listing for high-volume prediction markets. Deferred to after initial AMM-only launch. |

### L10. DEX Governance Lifecycle Completion

| Field | Value |
|-------|-------|
| Source | DEX_FINAL_PLAN Phase 6 |
| Component | `dex/dex.js`, `contracts/dex_governance/src/lib.rs` |
| Effort | **M** |
| Description | Finalize proposal, execute proposal, and status pipeline display in the UI. Requires H2 (execute_proposal actually doing something) as prerequisite. 3 sub-tasks. |

### L11. Agent Directory Page

| Field | Value |
|-------|-------|
| Source | MOLTYID_WALLET_PLAN Phase F |
| Component | `explorer/agents.html` (new file) |
| Effort | **M** |
| Description | Searchable agent discovery page with filter/sort by type, availability, reputation. Part of MoltyID wallet integration (M16). |

### L12. Social Recovery via Vouchers

| Field | Value |
|-------|-------|
| Source | MOLTYID_WALLET_PLAN Phase G |
| Component | `contracts/moltyid/src/lib.rs` |
| Effort | **L** |
| Description | 3-of-5 voucher-based key rotation for lost keys. Requires contract modification and security audit. |

### L13. Reputation Decay

| Field | Value |
|-------|-------|
| Source | MOLTYID_WALLET_PLAN §15 |
| Component | `contracts/moltyid/src/lib.rs` |
| Effort | **M** |
| Description | Activity-based reputation decay (5% per 90 days of inactivity) to prevent stale high-rep inactive accounts. |

### L14. Open Source Repository Items

| Field | Value |
|-------|-------|
| Source | REPOSITORY_STATUS_FEB8 |
| Component | Repository, CI/CD |
| Effort | **M** |
| Description | GitHub Actions CI/CD workflow, CODE_OF_CONDUCT.md, SECURITY.md, issue templates, PR template. Repository structure is marked ready but CI pipeline not configured. |

### L15. Production Deployment — DNS, Cloudflare, Multi-Region

| Field | Value |
|-------|-------|
| Source | PRODUCTION_DEPLOYMENT |
| Component | Infrastructure |
| Effort | **L** |
| Description | 3 seed validators (EU/US/Asia), DNS round-robin, Cloudflare Pages for static portals, relay topology. Detailed plan exists but no infrastructure provisioned. |

---

## Summary Statistics

| Tier | Count | Estimated Total Effort |
|------|-------|----------------------|
| **CRITICAL** | 12 items | ~6–8 weeks |
| **HIGH** | 20 items | ~8–12 weeks |
| **MEDIUM** | 21 items | ~10–16 weeks |
| **LOW/FUTURE** | 15 items | ~20–40 weeks |
| **TOTAL** | **68 items** | — |

### Dependency Graph (Critical Path)

```
C1 (fix wrapped WASMs) ──┐
C3, C7, C8, C12 (caller  │
  verification fixes) ────┤
C9 (genesis alignment) ───┤
C4 (AMM pricing fix) ─────┼──► C2 (genesis init) ──► H17 (build validation)
H7 (tokenomics params) ───┤                              │
C10 (token transfer       │                              ▼
  primitive) ─────────────┤                     Production Launch
                          │
C5, C6, C11 (wire real    │
  transfers in contracts)─┘
```

### Highest-Impact Actions (recommended execution order)

1. **Fix caller verification** in moltcoin, compute_market, moltoracle, moltdao, moltauction, dex_rewards, dex_governance (C3, C7, C8, C12, H12, H13, M18) — **~3 days, eliminates 7 items**
2. **Fix wrapped token WASMs** (C1) — **< 1 day, unblocks all DEX trading**
3. **Fix dex_amm pricing** (C4) — **1–2 days, makes AMM functional**
4. **Align genesis distribution** (C9) + tokenomics params (H7) — **1–2 days**
5. **Implement genesis initialization** (C2) — **3–5 days, activates all 26 contracts**
6. **Solve cross-contract token transfers** (C10) — **1–2 weeks, the systemic blocker**
7. **Wire real transfers** in dex_rewards, bountyboard, clawpay, clawvault, prediction_market, dex_router (C5, C6, C11, H9, H11, H15) — **depends on C10**
8. **Complete build validation** (H17) — **3–5 days, proves system works end-to-end**

---

*This document consolidates findings from: DEX_FINAL_PLAN, DEX_PRODUCTION_PLAN, DEX_COMPLETION_MILESTONE, PRODUCTION_GAPS_TRACKER, PRODUCTION_READINESS_AUDIT, PRODUCTION_AUDIT_COMPLETE, PRODUCTION_AUDIT_ALL_CONTRACTS, NEW_FINDINGS_AUDIT, SECURITY_AUDIT_REPORT, DEX_ARCHITECTURE_AUDIT, PRODUCTION_DEPLOYMENT, skill.md, CUSTODY_AUDIT_REPORT, CUSTODY_DEPLOYMENT, VALIDATOR_GRADUATION_PLAN, ABI_CONFORMANCE_AUDIT, PHASE2_AGENT_ECONOMY, PREDICTION_MARKETS_PLAN, REPOSITORY_STATUS_FEB8, ZK_PRIVACY_IMPLEMENTATION_PLAN, MOLTYID_VISION, MOLTYID_WALLET_PLAN, TOKENOMICS.*
