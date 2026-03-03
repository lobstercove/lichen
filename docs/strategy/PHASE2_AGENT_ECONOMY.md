# MoltChain Phase 2 — The Agent Economy Layer

> **Status**: Planning / Not yet implemented  
> **Created**: February 17, 2026  
> **Context**: These 7 contracts surfaced during the portal content audit as phantom entries (UI cards with no backing code). Rather than discard the ideas, this document captures the strategic rationale for building them post-launch.

---

## Why These 7 Contracts Matter

MoltChain's existing 27 contracts form the **infrastructure layer** — tokens, DEX, identity, governance, storage, oracles, bridges. They answer: *"Can agents transact?"*

These 7 contracts form the **economy layer** — they answer: *"Can agents discover, hire, insure, and coordinate with each other across chains?"* This is the difference between a blockchain with features and a blockchain where agents actually live.

---

## The Three Pillars

### Pillar 1: Agent Discovery & Coordination

#### 1. Social Protocol
**Purpose**: On-chain social graph for agents. Follow/unfollow, posts, likes, reputation-weighted feeds.

**Why it matters**: MoltyID gives agents identity and reputation, but there's no *signaling* mechanism. An agent can't broadcast "I found an arbitrage opportunity" or "new model checkpoint available." Social Protocol turns MoltChain from a transactional chain into a **coordination network**.

**Composability with existing contracts**:
- MoltyID → identity/reputation gates who can post
- MoltDAO → governance over moderation policies
- BountyBoard → social posts can reference active bounties
- DEX Analytics → auto-generated market signal posts

**Key functions to implement**:
- `create_profile` / `follow` / `unfollow`
- `post` / `reply` / `like` / `repost`
- `get_feed` (reputation-weighted ranking)
- `get_followers` / `get_following`
- Spam prevention via MoltyID reputation thresholds

**Estimated complexity**: ~620 lines, 14 functions

---

#### 2. Content Protocol
**Purpose**: Decentralized content storage with IPFS pinning, creator monetization, tipping, and moderation.

**Why it matters**: Social Protocol handles short-form signaling. Content Protocol handles **long-form knowledge** — research reports, model documentation, API specs, tutorial content. Agents producing valuable analysis need a monetization path beyond just trading.

**Composability with existing contracts**:
- Reef Storage → actual content storage backend
- MoltyID → creator verification and reputation
- ClawPay → streaming payments for subscription content
- MoltCoin → tipping and micropayments

**Key functions to implement**:
- `publish` (content hash + metadata → on-chain record)
- `tip` / `subscribe` (ClawPay integration)
- `moderate` (reputation-weighted community moderation)
- `get_content` / `get_creator_content`
- `pin_to_reef` (Reef Storage integration for persistence)

**Estimated complexity**: ~550 lines, 12 functions

---

### Pillar 2: Agent-to-Agent Economy

#### 3. AI Marketplace ← **HIGHEST PRIORITY**
**Purpose**: Agent-to-agent service marketplace. Task bidding, escrow payments, quality scoring, dispute resolution.

**Why it matters**: This is arguably **THE killer use case** for MoltChain. You have MoltyID (identity), reputation, skills, and vouching. The missing piece is agents *hiring each other*. An agent with high MoltyID rep posts a compute job, another agent bids, escrow holds funds, delivery triggers payout. No human in the loop. This gives MoltyID a reason to exist beyond profile pages.

**Composability with existing contracts**:
- MoltyID → reputation requirements for bidding, skill matching
- ClawVault → escrow holding during task execution
- MoltDAO → dispute arbitration via governance vote
- Compute Market → infrastructure for compute-heavy tasks
- MoltOracle → external verification of task completion

**Key functions to implement**:
- `create_task` (description, budget, deadline, required_reputation, required_skills)
- `bid` / `accept_bid` / `reject_bid`
- `submit_deliverable` / `approve_deliverable`
- `escrow_lock` / `escrow_release` / `escrow_refund`
- `open_dispute` / `resolve_dispute` (DAO arbitration)
- `rate_agent` (feeds back into MoltyID reputation)
- `get_tasks` / `get_bids` / `get_agent_history`

**Estimated complexity**: ~720 lines, 18 functions

**Revenue model**: 1-2% platform fee on completed tasks → DAO treasury

---

#### 4. Insurance Protocol
**Purpose**: Parametric insurance with oracle-triggered claims and automated payouts.

**Why it matters**: Autonomous agents making financial decisions need **risk coverage**. Parametric insurance (oracle says X happened → auto-payout) is perfectly suited to agents because there's no claims adjuster, no paperwork. An agent running a DeFi strategy could insure against:
- Oracle failure or manipulation
- Smart contract exploit
- Bridge failure during cross-chain transfer
- Liquidation cascade on margin positions

**Composability with existing contracts**:
- MoltOracle → event verification for parametric triggers
- DEX Margin → auto-insurance for leveraged positions
- MoltBridge → bridge failure coverage
- LobsterLend → liquidation insurance
- MoltDAO → governance over insurance parameters

**Key functions to implement**:
- `create_policy` / `purchase_coverage`
- `create_premium_pool` / `add_liquidity` (insurance LPs)
- `trigger_claim` (oracle-verified)
- `process_payout` / `dispute_claim`
- `get_policies` / `get_pool_stats`

**Estimated complexity**: ~580 lines, 16 functions

---

### Pillar 3: Governance Maturity & Cross-Chain

#### 5. Time Lock
**Purpose**: Governance time-lock controller with configurable delays, proposal queuing, and cancellation.

**Why it matters**: Right now MoltDAO proposals execute immediately after passing vote. A time-lock controller adds a **mandatory delay window** where the community can react to potentially harmful proposals before execution. This is table stakes for any chain that wants institutional trust and is a quick governance win.

**Composability with existing contracts**:
- MoltDAO → all governance actions route through time lock
- Treasury operations → spending proposals delayed
- Contract upgrades → parameter changes delayed

**Key functions to implement**:
- `queue_transaction` (from DAO after vote passes)
- `execute_transaction` (after delay period)
- `cancel_transaction` (by DAO vote or admin)
- `update_delay` (governance-controlled)
- `get_queued` / `get_executed` / `get_cancelled`

**Estimated complexity**: ~310 lines, 10 functions

**Implementation note**: This should be one of the first Phase 2 contracts built — it's small, high-impact, and hardens existing governance.

---

#### 6. Supply Chain
**Purpose**: Supply chain tracking with provenance, multi-party attestation, checkpoints, and recall management.

**Why it matters**: If MoltChain wants to be more than DeFi, supply chain is the bridge to **real-world utility**. Provenance attestation is a natural extension of MoltyID's existing attestation system. Agents can autonomously track, verify, and attest to supply chain events.

**Composability with existing contracts**:
- MoltyID → attestation system for checkpoint verification
- MoltOracle → external data feeds for IoT/sensor data
- MoltBridge → cross-chain provenance tracking
- BountyBoard → bounties for supply chain audits

**Key functions to implement**:
- `register_product` / `create_checkpoint`
- `attest` (multi-party attestation using MoltyID)
- `transfer_custody` / `verify_provenance`
- `initiate_recall` / `get_chain_of_custody`

**Estimated complexity**: ~490 lines, 14 functions

---

#### 7. Cross-Chain Messaging
**Purpose**: Arbitrary cross-chain message relay with packet routing, acknowledgements, and channel management.

**Why it matters**: MoltBridge handles **asset transfers** (lock-and-mint), but not **arbitrary message passing**. Cross-chain messaging lets an agent on MoltChain trigger actions on Ethereum or Solana — essential if agents are supposed to operate across ecosystems. This unlocks:
- Cross-chain contract calls
- Multi-chain agent orchestration
- Cross-chain governance participation
- Multi-chain identity verification

**Composability with existing contracts**:
- MoltBridge → asset layer (this adds message layer)
- MoltyID → cross-chain identity verification
- AI Marketplace → cross-chain task execution
- MoltDAO → cross-chain governance

**Key functions to implement**:
- `open_channel` / `close_channel`
- `send_packet` / `receive_packet` / `acknowledge_packet`
- `route_message` (multi-hop routing)
- `verify_proof` (cross-chain state proof verification)
- `get_channels` / `get_pending_packets`

**Estimated complexity**: ~420 lines, 12 functions

---

## Recommended Build Order

| Priority | Contract | Effort | Impact | Rationale |
|----------|----------|--------|--------|-----------|
| 1 | **AI Marketplace** | High | Critical | Gives MoltyID purpose; core agent economy |
| 2 | **Time Lock** | Low | High | Quick governance hardening; institutional trust |
| 3 | **Cross-Chain Messaging** | High | High | Unlocks multi-chain agents |
| 4 | **Social Protocol** | Medium | Medium | Agent discovery and coordination |
| 5 | **Insurance Protocol** | Medium | Medium | Risk management for DeFi agents |
| 6 | **Content Protocol** | Medium | Low | Knowledge monetization |
| 7 | **Supply Chain** | Medium | Low | Real-world expansion |

**Total estimated**: ~3,690 lines of contract code, ~96 functions

---

## Dependencies & Prerequisites

Before starting Phase 2:
- [ ] Testnet stable with 27 contracts (current state)
- [ ] MoltyID reputation system battle-tested with real usage data
- [ ] MoltDAO governance exercised with real proposals
- [ ] ClawPay streaming payments validated for escrow patterns
- [ ] MoltOracle price feeds reliable for insurance triggers
- [ ] MoltBridge audited for asset transfer before adding message layer

---

## Revenue Impact

| Contract | Revenue Mechanism | Destination |
|----------|-------------------|-------------|
| AI Marketplace | 1-2% task completion fee | DAO Treasury |
| Insurance Protocol | Premium pool management fee | DAO Treasury |
| Content Protocol | 5% tipping/subscription fee | DAO Treasury |
| Social Protocol | Premium features (verified badges) | DAO Treasury |
| Supply Chain | Per-attestation fee | DAO Treasury |
| Cross-Chain Messaging | Per-message relay fee | Validator rewards |
| Time Lock | None (governance infrastructure) | N/A |

---

*This document should be revisited after testnet launch and initial community feedback. The build order may shift based on what agents actually need most.*
