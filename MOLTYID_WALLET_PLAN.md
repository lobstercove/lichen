# MoltyID Wallet Integration Plan

**Date:** February 14, 2026
**Status:** Planning
**Branch:** `main`
**Depends on:** MoltyID contract (deployed at genesis, 37 exports, 3126 lines)

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Current State Audit](#2-current-state-audit)
3. [Identity System Design](#3-identity-system-design)
4. [Wallet UI Redesign — Tab Architecture](#4-wallet-ui-redesign--tab-architecture)
5. [Identity Tab — Full Specification](#5-identity-tab--full-specification)
6. [Balance Card Enhancement — Identity Button](#6-balance-card-enhancement--identity-button)
7. [.molt Name System — Explorer-Wide Integration](#7-molt-name-system--explorer-wide-integration)
8. [RPC Endpoints — New MoltyID APIs](#8-rpc-endpoints--new-moltyid-apis)
9. [Agent Profile & Discovery](#9-agent-profile--discovery)
10. [Reputation Economy](#10-reputation-economy)
11. [Cross-Feature Integration](#11-cross-feature-integration)
12. [Storage & Data Flow](#12-storage--data-flow)
13. [Implementation Phases](#13-implementation-phases)
14. [Test Plan](#14-test-plan)
15. [Open Questions](#15-open-questions)

---

## 1. Executive Summary

MoltyID is MoltChain's on-chain identity system — already deployed as a genesis contract with 37 WASM functions, reputation scoring, skill attestations, .molt naming, and agent discovery. But it has **zero UI integration**. No wallet page shows identity data, no explorer resolves .molt names, and no RPC endpoints serve identity queries.

This plan connects MoltyID to the wallet and explorer, making identities visible, manageable, and central to the MoltChain experience.

### What MoltyID Already Supports (Contract Level)

| Feature | Status | Functions |
|---------|--------|-----------|
| Identity registration | Deployed | `register_identity`, `get_identity` |
| Reputation scoring | Deployed | `update_reputation`, `get_reputation`, `get_trust_tier` |
| Skill management | Deployed | `add_skill`, `get_skills`, `attest_skill`, `get_attestations` |
| Social vouching | Deployed | `vouch`, `get_vouches` |
| .molt name service | Deployed | `register_name`, `resolve_name`, `reverse_resolve`, `transfer_name`, `renew_name`, `release_name` |
| Agent discovery | Deployed | `set_endpoint`, `set_metadata`, `set_availability`, `set_rate`, `get_agent_profile` |
| Achievements | Deployed | `award_contribution_achievement`, `get_achievements` |
| Admin controls | Deployed | `mid_pause`, `mid_unpause`, `transfer_admin` |

### What's Missing (This Plan)

| Gap | Solution |
|-----|----------|
| No identity UI in wallet | Add Identity tab to address.html |
| No .molt name display anywhere | Integrate reverse_resolve across all explorer pages |
| No RPC endpoints for identity | Add 8+ new RPC methods |
| No way to create identity from UI | Add "Create Identity" button in balance card |
| No skill/reputation visualization | Trust tier badges, skill cards, reputation graph |
| No agent directory | Searchable agent discovery page |
| No name registration UI | .molt name search + register flow |
| No attestation UI | Skill endorsement workflow |
| No key recovery mechanism | Social recovery via vouchers (new contract feature) |

---

## 2. Current State Audit

### Explorer Address Page (address.html — 367 lines)

Current layout is a **flat card stack** (no tabs):

```
┌─ Quick Stats Bar ──────────────────────────┐
│ Balance │ Token Balance │ Txs │ Account Type │
└────────────────────────────────────────────┘

┌─ Account Information Card ─────────────────┐
│ Address, Balance, Spendable/Staked/Locked  │
│ Owner, Executable, Registry info, Data     │
└────────────────────────────────────────────┘

┌─ Token Balances Card (if tokens) ──────────┐
│ Table: Token │ Symbol │ Balance │ Value     │
└────────────────────────────────────────────┘

┌─ Validator Rewards Card (if validator) ────┐
│ Earned, Pending, Claimed, Rate, Blocks     │
│ Bootstrap Debt + Vesting progress bar      │
└────────────────────────────────────────────┘

┌─ Transaction History Card ─────────────────┐
│ Table: Hash │ Block │ Age │ From/To │ etc  │
└────────────────────────────────────────────┘

┌─ Raw Account Data Card ───────────────────┐
│ JSON blob                                  │
└────────────────────────────────────────────┘
```

**Problems:**
- No tabs — becomes a very long page as features accumulate
- No identity section
- No Send/Receive/Deposit actions (currently no wallet actions from explorer)
- No .molt name resolution (shows raw base58 addresses everywhere)

### RPC Layer (rpc/src/lib.rs — 6278 lines)

**Existing identity-relevant endpoints:**
- `getProgramStorage` — generic storage dump, can read MoltyID keys but requires client-side key construction
- No dedicated MoltyID RPC methods exist

**Contracts that already reference MoltyID:**
- `dex_governance` — reputation-gated voting (500+ rep required)
- `clawpay` — identity-gated payments (configurable min reputation)
- `dex_rewards` — verified referral bonus rate (15% for MoltyID-verified)

---

## 3. Identity System Design

### Identity Tiers (from contract)

| Tier | Name | Reputation | Badge |
|------|------|------------|-------|
| 0 | Newcomer | 0–99 | 🔵 |
| 1 | Verified | 100–499 | 🟢 |
| 2 | Trusted | 500–999 | 🟡 |
| 3 | Established | 1,000–4,999 | 🟠 |
| 4 | Elite | 5,000–9,999 | 🔴 |
| 5 | Legendary | 10,000+ | 💎 |

### Agent Types

| ID | Type | Icon |
|----|------|------|
| 0 | Unknown | ❓ |
| 1 | Trading | 📈 |
| 2 | Development | 💻 |
| 3 | Analysis | 🔬 |
| 4 | Creative | 🎨 |
| 5 | Infrastructure | 🏗️ |
| 6 | Governance | 🏛️ |
| 7 | Oracle | 🔮 |
| 8 | Storage | 💾 |
| 9 | General | ⚡ |

### Reputation Sources

| Action | Points | Direction |
|--------|--------|-----------|
| Successful transaction | +10 | Increase |
| Governance participation | +50 | Increase |
| Program deployed | +100 | Increase |
| Uptime hour (validators) | +1 | Increase |
| Peer endorsement (vouch) | +25 to target, -5 from voucher | Both |
| Failed transaction | -5 | Decrease |
| Slashing event | -100 | Decrease |

---

## 4. Wallet UI Redesign — Tab Architecture

### From Flat Cards to Tabs

Transform the address page from a scroll of cards to a tabbed interface:

```
┌─────────────────────────────────────────────────────────────┐
│  ┌─ Balance Card ─────────────────────────────────────────┐ │
│  │  alice.molt                                    🟡 Trusted│
│  │  MoLT... (Base58)  │  0x... (EVM)                       │
│  │                                                          │
│  │  ◉ 12,450.50 MOLT                                      │
│  │    Spendable: 10,000.00  Staked: 2,000.00  Locked: 450 │
│  │                                                          │
│  │  [Send] [Receive] [Deposit] [Create Identity]           │
│  └──────────────────────────────────────────────────────────┘│
│                                                               │
│  [Overview] [Tokens] [Identity] [Staking] [Transactions] [Data]│
│                                                               │
│  ┌─ Tab Content Area ──────────────────────────────────────┐ │
│  │                                                          │ │
│  │   (content changes based on selected tab)                │ │
│  │                                                          │ │
│  └──────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────┘
```

### Tab Definitions

| Tab | Content | Visible When |
|-----|---------|-------------|
| **Overview** | Account info + quick stats (current card content, condensed) | Always (default) |
| **Tokens** | Token balances table | Always |
| **Identity** | MoltyID profile, reputation, skills, vouches, achievements, .molt names | If identity registered |
| **Staking** | Validator rewards, debt, vesting (current rewards card) | If validator |
| **Transactions** | Transaction history table (current tx card) | Always |
| **Data** | Raw account data JSON | Always |

When no identity is registered, the Identity tab shows a call-to-action:
```
┌──────────────────────────────────────────────────┐
│  No MoltyID Identity Found                       │
│                                                  │
│  Register your identity to unlock:               │
│  ✓ .molt name service                           │
│  ✓ Reputation scoring & trust tiers             │
│  ✓ Skill attestations                           │
│  ✓ Agent discovery & marketplace                │
│  ✓ Governance participation                     │
│  ✓ Enhanced referral rewards                    │
│                                                  │
│  [Register Identity]                             │
└──────────────────────────────────────────────────┘
```

---

## 5. Identity Tab — Full Specification

### Layout When Identity Exists

```
┌─ Identity Tab ──────────────────────────────────────────────┐
│                                                              │
│  ┌─ Profile Header ──────────────────────────────────────┐  │
│  │  🟡 alice.molt                                         │  │
│  │  Agent Type: 📈 Trading                                │  │
│  │  Registered: Feb 14, 2026  │  Status: Active           │  │
│  │  Trust Tier: Trusted (Rep: 742)                        │  │
│  │                                                        │  │
│  │  [Edit Profile] [Set Endpoint] [Set Availability]      │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                              │
│  ┌─ Reputation ──────────────────────────────────────────┐  │
│  │                                                        │  │
│  │  Score: 742 / 100,000                                  │  │
│  │  ████████░░░░░░░░░░░░  (0.74%)                        │  │
│  │                                                        │  │
│  │  Trust Tier:  🔵 → 🟢 → [🟡] → 🟠 → 🔴 → 💎         │  │
│  │                    ↑ current                           │  │
│  │  Next tier: Established at 1,000 (258 points away)     │  │
│  │                                                        │  │
│  │  Contributions:                                        │  │
│  │    Successful Txs: 45    Governance Votes: 3           │  │
│  │    Programs Deployed: 1  Peer Endorsements: 8          │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                              │
│  ┌─ Skills & Attestations ───────────────────────────────┐  │
│  │                                                        │  │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐              │  │
│  │  │ Solidity │ │ Rust     │ │ Trading  │              │  │
│  │  │ ████░    │ │ █████    │ │ ███░░    │              │  │
│  │  │ Lvl 4/5  │ │ Lvl 5/5  │ │ Lvl 3/5  │              │  │
│  │  │ 3 attest │ │ 5 attest │ │ 1 attest │              │  │
│  │  └──────────┘ └──────────┘ └──────────┘              │  │
│  │                                                        │  │
│  │  [Add Skill] [Request Attestation]                     │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                              │
│  ┌─ Vouches ─────────────────────────────────────────────┐  │
│  │                                                        │  │
│  │  Vouched by (12):                                      │  │
│  │  🟠 bob.molt  │  🟡 charlie.molt  │  🟢 dave.molt    │  │
│  │  🟡 eve.molt  │  ...8 more                            │  │
│  │                                                        │  │
│  │  Vouched for (5):                                      │  │
│  │  🟢 frank.molt  │  🔵 grace.molt  │  ...3 more       │  │
│  │                                                        │  │
│  │  [Vouch for this identity]                             │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                              │
│  ┌─ Achievements ────────────────────────────────────────┐  │
│  │                                                        │  │
│  │  🏆 First Transaction    🏆 Governance Participant     │  │
│  │  🏆 Builder (deployed)   🏆 Rep 500 Milestone         │  │
│  │                                                        │  │
│  │  Locked: 🔒 Rep 1000  🔒 Rep 5000  🔒 Endorser       │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                              │
│  ┌─ .molt Names ─────────────────────────────────────────┐  │
│  │                                                        │  │
│  │  alice.molt      Expires: Feb 14, 2027  [Renew]       │  │
│  │  mytrader.molt   Expires: Aug 01, 2027  [Renew]       │  │
│  │                                                        │  │
│  │  [Register New Name] [Transfer Name]                   │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                              │
│  ┌─ Agent Discovery Profile ─────────────────────────────┐  │
│  │                                                        │  │
│  │  Endpoint: https://alice-agent.molt.dev/api            │  │
│  │  Availability: 🟢 Available                            │  │
│  │  Rate: 0.5 MOLT/request                                │  │
│  │  Metadata: { "model": "gpt-4", "speciality": "..." }  │  │
│  │                                                        │  │
│  │  [Update Endpoint] [Set Rate] [Toggle Availability]    │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

---

## 6. Balance Card Enhancement — Identity Button

### Current Balance Card (Quick Stats)

```
[Balance: 12,450 MOLT] [Token Balance: 3] [Txs: 156] [Account Type: User]
```

### Enhanced Balance Card

```
┌────────────────────────────────────────────────────────────────┐
│                                                                │
│  alice.molt                                        🟡 Trusted  │
│  MoLT8xK...3nP (Base58)  ·  0x7a3f...b2c1 (EVM)             │
│                                                                │
│         ◉ 12,450.500000000 MOLT                               │
│                                                                │
│  ┌──────────────┬──────────────┬──────────────┐               │
│  │ 🔓 Spendable │ 🔒 Staked    │ 📄 Locked    │               │
│  │ 10,000.50    │ 2,000.00    │ 450.00       │               │
│  └──────────────┴──────────────┴──────────────┘               │
│                                                                │
│  [📤 Send] [📥 Receive] [💰 Deposit] [🆔 Identity]            │
│                                                                │
└────────────────────────────────────────────────────────────────┘
```

### Button Behaviors

| Button | Action |
|--------|--------|
| **Send** | Opens modal: recipient (supports .molt names!), amount, memo |
| **Receive** | Shows QR code with address + .molt name |
| **Deposit** | Shows bridge instructions (deposit mUSD/WSOL/WETH from external chains) |
| **Identity** | If registered → scrolls to Identity tab. If not → opens registration modal |

### Identity Registration Modal

```
┌─ Create Your MoltyID Identity ───────────────────────┐
│                                                       │
│  Display Name: [________________] (3-64 chars)        │
│                                                       │
│  Agent Type:                                          │
│  ○ Trading 📈    ○ Development 💻   ○ Analysis 🔬    │
│  ○ Creative 🎨   ○ Infrastructure 🏗  ○ Governance 🏛│
│  ○ Oracle 🔮     ○ Storage 💾       ○ General ⚡     │
│                                                       │
│  Optional — Register .molt Name:                      │
│  [____________].molt    Duration: [1 year ▼]          │
│                                                       │
│  ℹ Initial reputation: 100 (Verified tier)            │
│  ℹ Registration is free (gas only)                    │
│                                                       │
│              [Create Identity]  [Cancel]              │
└───────────────────────────────────────────────────────┘
```

---

## 7. .molt Name System — Explorer-Wide Integration

### Reverse Resolution Everywhere

Every address displayed in the explorer should attempt `.molt` name resolution:

| Page | Where | Before | After |
|------|-------|--------|-------|
| address.html | Page title | `Address: MoLT8xK...3nP` | `alice.molt (MoLT8xK...3nP)` |
| transaction.html | From/To fields | `MoLT8xK...3nP` | `alice.molt` with hover for full address |
| transactions.html | Table rows | Raw addresses | .molt names where available |
| block.html | Producer/Validator | Raw address | .molt name badge |
| validators.html | Validator list | Raw addresses | .molt names |
| contracts.html | Deployer | Raw address | .molt name |

### Implementation Approach

1. **Batch resolution** — New RPC endpoint `batchReverseMoltNames([addr1, addr2, ...])` returns a map of address→name
2. **Client-side cache** — `address.js` / `utils.js` maintain a `Map<string, string|null>` of resolved names
3. **Display helper** — `formatAddressWithName(address, name)` renders either `name.molt` with tooltip or truncated address as fallback
4. **Progressive enhancement** — Page loads with raw addresses, then async resolves names and updates DOM

### Name Search & Registration

```
┌─ .molt Name Service ─────────────────────────────────────────┐
│                                                               │
│  Search: [______________].molt  [Check Availability]         │
│                                                               │
│  ✅ "trading_bot" is available!                               │
│                                                               │
│  Register for:                                                │
│  ○ 1 year   ○ 2 years   ○ 5 years                           │
│                                                               │
│  [Register trading_bot.molt]                                  │
│                                                               │
│  ── Recently Registered ─────────────────────                │
│  alice.molt       │ Expires: 2027-02-14                      │
│  bob.molt         │ Expires: 2027-08-01                      │
│  oracle_node.molt │ Expires: 2028-02-14                      │
└───────────────────────────────────────────────────────────────┘
```

The name system functions already exist in the contract:
- `register_name(caller, name, name_len, duration_years)` — Registration
- `resolve_name(name, name_len)` — Forward lookup (name → address)
- `reverse_resolve(addr)` — Reverse lookup (address → name)
- `transfer_name(caller, name, name_len, new_owner)` — Transfer
- `renew_name(caller, name, name_len, additional_years)` — Renewal
- `release_name(caller, name, name_len)` — Release

---

## 8. RPC Endpoints — New MoltyID APIs

### New Endpoints

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `getMoltyIdIdentity` | `[pubkey]` | Identity record (JSON) | Get full identity for an address |
| `getMoltyIdReputation` | `[pubkey]` | `{ score, tier, tier_name }` | Quick reputation lookup |
| `getMoltyIdSkills` | `[pubkey]` | Array of skills with attestation counts | List all skills |
| `getMoltyIdVouches` | `[pubkey]` | `{ received: [...], given: [...] }` | Vouch network |
| `getMoltyIdAchievements` | `[pubkey]` | Array of achievements with timestamps | Achievement list |
| `getMoltyIdProfile` | `[pubkey]` | Complete profile (identity + skills + vouches + endpoint + metadata) | Composite query |
| `resolveMoltName` | `[name]` | `{ owner, registered_slot, expiry_slot }` | Forward name lookup |
| `reverseMoltName` | `[pubkey]` | `{ name }` or null | Reverse name lookup |
| `batchReverseMoltNames` | `[pubkey1, pubkey2, ...]` | `{ addr1: name1, addr2: null, ... }` | Batch reverse for UI |
| `searchMoltNames` | `[prefix]` | Array of matching names | Name search |
| `getMoltyIdAgentDirectory` | `[{ type?, available?, limit? }]` | Array of agent profiles | Agent discovery |
| `getMoltyIdStats` | `[]` | `{ total_identities, total_names, tier_distribution }` | Global stats |

### Implementation Pattern

Each endpoint reads from MoltyID contract storage using key patterns:

```rust
async fn handle_get_moltyid_identity(state: &RpcState, params: ...) -> Result<Value, RpcError> {
    let pubkey = parse_pubkey(params)?;
    
    // Derive MoltyID contract address
    let moltyid_addr = state.get_symbol_address("YID")?;
    
    // Read identity record: key = "id:{hex(pubkey)}"
    let key = format!("id:{}", hex::encode(pubkey.0));
    let contract_account = state.get_contract_storage(&moltyid_addr, &key)?;
    
    // Parse 127-byte identity record
    let identity = parse_identity_record(&contract_account)?;
    
    // Also read reputation: key = "rep:{hex(pubkey)}"
    let rep_key = format!("rep:{}", hex::encode(pubkey.0));
    let reputation = state.read_storage_u64(&moltyid_addr, &rep_key)?;
    
    // Read .molt name: key = "name_rev:{hex(pubkey)}"
    let name_key = format!("name_rev:{}", hex::encode(pubkey.0));
    let molt_name = state.read_storage_string(&moltyid_addr, &name_key)?;
    
    Ok(json!({
        "address": pubkey_str,
        "name": identity.name,
        "molt_name": molt_name,
        "agent_type": identity.agent_type,
        "agent_type_name": agent_type_name(identity.agent_type),
        "reputation": reputation,
        "trust_tier": trust_tier(reputation),
        "trust_tier_name": trust_tier_name(reputation),
        "created_at": identity.created_at,
        "updated_at": identity.updated_at,
        "skill_count": identity.skill_count,
        "vouch_count": identity.vouch_count,
        "is_active": identity.is_active,
    }))
}
```

### Storage Key Decoding Reference

| Data | Key Pattern | Parse |
|------|-------------|-------|
| Identity | `id:{hex32}` | 127 bytes → struct |
| Reputation | `rep:{hex32}` | 8 bytes → u64 LE |
| Skill | `skill:{hex32}:{index}` | Variable → name + proficiency + ts |
| Vouch | `vouch:{hex32}:{index}` | 40 bytes → voucher addr + ts |
| .molt name (fwd) | `name:{name_bytes}` | 48 bytes → owner + reg_slot + expiry |
| .molt name (rev) | `name_rev:{hex32}` | Variable → name bytes |
| Endpoint | `endpoint:{hex32}` | Variable → URL string |
| Metadata | `metadata:{hex32}` | Variable → JSON string |
| Availability | `availability:{hex32}` | 1 byte → status enum |
| Rate | `rate:{hex32}` | 8 bytes → u64 LE |
| Achievement | `ach:{hex32}:{id}` | 9 bytes → id + ts |
| Attestation | `attest_{id_hex}_{skill_hex}_{attester_hex}` | 9 bytes → level + ts |
| Contribution | `cont:{hex32}:{type}` | 8 bytes → u64 LE count |

---

## 9. Agent Profile & Discovery

### Agent Directory Page

New explorer page: `explorer/agents.html`

```
┌─ Agent Directory ─────────────────────────────────────────────┐
│                                                                │
│  Filter: [All Types ▼] [Available Only ☐] [Min Rep: ___]     │
│  Sort:   [Reputation ▼] [Rate ▲] [Newest]                    │
│                                                                │
│  ┌─ Agent Card ───────────────────────────────────────────┐   │
│  │  📈 alice.molt                           🟡 Trusted     │   │
│  │  Type: Trading  │  Rep: 742  │  Rate: 0.5 MOLT/req    │   │
│  │  Availability: 🟢 Available                             │   │
│  │  Endpoint: https://alice-agent.molt.dev/api             │   │
│  │  Skills: Rust (5/5, 5 attests), Trading (3/5, 1 attest)│   │
│  │  Vouches: 12 received                                   │   │
│  │                                             [View →]    │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                │
│  ┌─ Agent Card ───────────────────────────────────────────┐   │
│  │  🔮 price_oracle.molt                    🟠 Established  │   │
│  │  Type: Oracle  │  Rep: 2,340  │  Rate: 0.1 MOLT/req   │   │
│  │  Availability: 🟢 Available                             │   │
│  │  Endpoint: https://oracle.moltchain.io/v1               │   │
│  │  Skills: Data Analysis (5/5), Market Data (4/5)         │   │
│  │  Vouches: 28 received                                   │   │
│  │                                             [View →]    │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                │
│  Showing 1-10 of 234 agents              [← Prev] [Next →]   │
└────────────────────────────────────────────────────────────────┘
```

### Agent Profile Deep Page

Clicking "View →" on an agent card navigates to their address page Identity tab, showing the full profile with endpoint, metadata, skills, attestations, vouches, and achievements.

### Discovery Use Cases

| Scenario | Flow |
|----------|------|
| Find a trading bot | Filter: Trading type, Available, Sort by Rep |
| Find an oracle provider | Filter: Oracle type, check endpoint + attestations |
| Verify a contractor | View skills + attestation count + vouch network |
| Hire an agent | Check rate, availability, metadata for capabilities |

---

## 10. Reputation Economy

### How Reputation Drives Value

Reputation unlocks features across MoltChain:

| Threshold | Feature Unlocked |
|-----------|-----------------|
| 0 (new wallet) | Basic transactions, viewing |
| 100 (register) | Identity created, initial tier |
| 100+ (Verified) | Standard DEX trading, token transfers |
| 500+ (Trusted) | Prediction market creation, governance proposals |
| 500+ | DEX governance voting (dex_governance MIN_REPUTATION) |
| 1,000+ (Established) | Resolution submission for prediction markets |
| 1,000+ | ClawPay identity-gated payments unlocked |
| 5,000+ (Elite) | Advanced features, higher limits |
| 10,000+ (Legendary) | Full platform privileges |

### Reputation Display Widget

Reusable component shown wherever a user is referenced:

```
┌────────────────────────────┐
│ 🟡 alice.molt  Rep: 742   │
│ [████████░░] Trusted       │
└────────────────────────────┘
```

Small variant (inline): `🟡 alice.molt (742)`

Micro variant (badge only): `🟡`

### Vouch Network Visualization

```
         eve.molt (🟢)
            │
    bob.molt (🟠) ──── alice.molt (🟡) ──── frank.molt (🟢)
            │                │
   charlie.molt (🟡)    dave.molt (🟢)
```

This builds a web-of-trust visual, showing how identities are connected through vouches. Useful for assessing an unknown agent's credibility.

---

## 11. Cross-Feature Integration

### Where MoltyID Appears Across the Platform

| Feature | Integration Point |
|---------|------------------|
| **Explorer — addresses** | .molt name resolution, trust tier badge next to every address |
| **Explorer — transactions** | Show .molt names in from/to columns |
| **Explorer — validators** | Show .molt name and tier for each validator |
| **Explorer — contracts** | Show deployer's .molt identity |
| **Wallet — balance card** | .molt name as primary identifier, identity button |
| **Wallet — send modal** | Accept .molt names as recipient (auto-resolve) |
| **DEX — trading** | Show trader .molt name and tier in order book |
| **DEX — governance** | Reputation badge on proposals and votes |
| **DEX — rewards** | Verified tier unlocks higher referral rates |
| **Prediction markets** | Reputation-gated market creation and resolution |
| **ClawPump** | Creator identity badge on launchpad tokens |
| **ClawPay** | Identity-gated payment streams |
| **Agent directory** | Full discovery interface |

### .molt Name Input Component

Reusable across all forms:

```
[alice.molt                    ] ← resolves → MoLT8xK...3nP
                                  ✅ resolved to alice.molt (🟡 Trusted)
```

If input looks like a .molt name → resolve via RPC
If input looks like base58 → use directly, attempt reverse resolution for display

---

## 12. Storage & Data Flow

### Data Flow Architecture

```
┌─────────────────┐     ┌─────────────────┐     ┌──────────────┐
│  MoltyID WASM   │────►│  RPC Layer      │────►│  Explorer UI │
│  (storage keys) │     │  (decode + JSON)│     │  (render)    │
└─────────────────┘     └─────────────────┘     └──────────────┘

Write path (identity registration, vouch, etc.):
┌──────────────┐     ┌─────────────────┐     ┌─────────────────┐
│  Explorer UI │────►│  Wallet/Signer  │────►│  Validator      │
│  (modal form)│     │  (build + sign  │     │  (execute WASM) │
└──────────────┘     │   transaction)  │     └─────────────────┘
                     └─────────────────┘
```

### Transaction Types for Identity Operations

| Operation | Contract Call | Args |
|-----------|-------------|------|
| Register identity | `register_identity(owner, agent_type, name, name_len)` | Standard contract call tx |
| Add skill | `add_skill(caller, skill_name, skill_name_len, proficiency)` | Contract call tx |
| Vouch | `vouch(voucher, vouchee)` | Contract call tx |
| Register .molt name | `register_name(caller, name, name_len, duration_years)` | Contract call tx |
| Attest skill | `attest_skill(attester, identity, skill_name, skill_name_len, level)` | Contract call tx |
| Set endpoint | `set_endpoint(caller, url, url_len)` | Contract call tx |
| Set metadata | `set_metadata(caller, json, json_len)` | Contract call tx |
| Set availability | `set_availability(caller, status)` | Contract call tx |
| Set rate | `set_rate(caller, molt_per_unit)` | Contract call tx |

### Caching Strategy

On the client side:
```javascript
// Name cache — persists across page navigations
const moltNameCache = new Map(); // address → { name, fetchedAt }
const NAME_CACHE_TTL = 300_000;  // 5 minutes

// Identity cache — per-address page load
const identityCache = new Map(); // address → full identity JSON
const IDENTITY_CACHE_TTL = 60_000; // 1 minute
```

---

## 13. Implementation Phases

### Phase A — RPC Foundation

```
A.1  Implement getMoltyIdIdentity RPC endpoint
A.2  Implement getMoltyIdReputation RPC endpoint
A.3  Implement reverseMoltName + batchReverseMoltNames
A.4  Implement resolveMoltName
A.5  Implement getMoltyIdProfile (composite query)
A.6  Implement getMoltyIdSkills, getMoltyIdVouches, getMoltyIdAchievements
A.7  Implement getMoltyIdAgentDirectory
A.8  Implement getMoltyIdStats
A.9  Unit tests for all RPC endpoints
```

### Phase B — Wallet UI Restructure

```
B.1  Convert address.html from flat cards to tab architecture
B.2  Extract current cards into Overview / Tokens / Staking / Transactions / Data tabs
B.3  Enhance balance card: .molt name display, tier badge, action buttons
B.4  Add tab switching logic to address.js
B.5  Visual polish: responsive, hover states, transitions
```

### Phase C — Identity Tab

```
C.1  Identity tab skeleton: shows "Register" CTA if no identity, or full profile
C.2  Profile header: name, type, tier badge, registration date
C.3  Reputation section: score bar, tier ladder, contribution breakdown
C.4  Skills section: skill cards with proficiency bars and attestation counts
C.5  Vouches section: vouch network list (received + given)
C.6  Achievements section: earned + locked badges
C.7  .molt names section: owned names with expiry dates, renew/transfer actions
C.8  Agent discovery section: endpoint, availability, rate, metadata
```

### Phase D — Identity Actions

```
D.1  Registration modal: create identity from UI (builds + signs contract call tx)
D.2  .molt name registration: search, check availability, register
D.3  Send modal with .molt name resolution
D.4  Vouch button: vouch for viewed identity
D.5  Skill attestation: endorse another identity's skill
D.6  Profile editing: update agent type, endpoint, metadata, availability, rate
```

### Phase E — Explorer-Wide .molt Integration

```
E.1  Transaction page: resolve .molt names in from/to
E.2  Transaction list: batch resolve all addresses
E.3  Block page: resolve producer/validator names
E.4  Validator page: show .molt names + trust tiers
E.5  Contract page: show deployer .molt name
E.6  Search bar: support .molt name search (type "alice.molt" → navigate to address)
```

### Phase F — Agent Directory

```
F.1  Create agents.html page
F.2  Agent cards with filter/sort
F.3  Navigation link in explorer header
F.4  Directory RPC integration
```

### Phase G — Contract Enhancement (Optional)

```
G.1  Social recovery: add recovery guardians (vouch-based key rotation)
G.2  Identity delegation: allow agent to act on behalf of owner
G.3  Reputation decay: time-based score adjustments (prevents stale high-rep inactive accounts)
G.4  Name auction: premium .molt names sold via auction
```

---

## 14. Test Plan

### RPC Tests (~25 tests)

```
test_get_identity_existing
test_get_identity_nonexistent
test_get_reputation_with_score
test_get_reputation_no_identity
test_resolve_molt_name
test_resolve_nonexistent_name
test_reverse_molt_name
test_reverse_no_name
test_batch_reverse_mixed
test_batch_reverse_empty
test_get_skills_with_attestations
test_get_skills_none
test_get_vouches_bidirectional
test_get_achievements_list
test_get_profile_complete
test_agent_directory_filter_type
test_agent_directory_filter_available
test_agent_directory_sort_reputation
test_agent_directory_pagination
test_global_stats
test_name_search_prefix
test_trust_tier_boundaries
test_identity_parse_127_bytes
test_skill_parse_variable
test_concurrent_requests
```

### UI Tests (Manual Test Checklist)

```
□ Address page loads with tabs (Overview default)
□ Tab switching works (all 6 tabs)
□ Balance card shows .molt name if registered
□ Balance card shows trust tier badge
□ Identity tab shows "Register" CTA if no identity
□ Identity tab shows full profile if registered
□ Reputation bar renders correctly at all tiers
□ Skills display with proficiency and attestation counts
□ Vouches list shows received and given
□ Achievements display earned + locked
□ .molt names section shows owned names with dates
□ "Create Identity" modal opens and submits transaction
□ .molt name search resolves availability
□ Send modal accepts .molt names and resolves them
□ Vouch button sends transaction
□ Transaction page shows .molt names in from/to
□ Validator page shows .molt names
□ Search bar resolves .molt names to addresses
□ Agent directory page loads with filters
□ Agent cards display correctly
□ All features work on mobile viewport
```

### Integration Tests

```
test_register_identity_then_query_rpc
test_register_name_then_resolve
test_vouch_then_check_reputation_increase
test_add_skill_then_attest_then_query
test_set_agent_profile_then_directory_lists
test_identity_gates_prediction_market_creation
test_identity_gates_governance_voting
test_molt_name_in_send_transaction
test_batch_resolve_performance (100 addresses < 200ms)
```

---

## 15. Open Questions

### Design Decisions for Discussion

| Question | Options | Recommendation |
|----------|---------|----------------|
| Identity registration cost? | Free (gas only), Small fee (1 MOLT), Significant fee (10 MOLT) | **Free (gas only)** — maximize adoption, reputation gates protect quality |
| .molt name pricing? | Flat fee per year, Length-based (short=expensive), Auction | **Length-based** — 3-char: 100 MOLT, 4-char: 10 MOLT, 5+: 1 MOLT per year |
| Should .molt names expire? | Yes (annual renewal), No (perpetual), Grace period | **Yes with grace** — 1yr registration, 30-day grace period before release |
| Reputation decay? | No decay (permanent), Slow decay (10% annual), Activity-based | **Activity-based** — score decays 5% per 90 days of inactivity |
| Who can attest skills? | Anyone with identity, Same-skill holders only, Admin-approved attestors | **Anyone with identity** — attestation count is the credibility signal |
| Social recovery guardians? | 3-of-5 vouchers, Admin + vouchers, No recovery | **3-of-5 vouchers** — add in Phase G, most decentralized option |
| Identity transferable? | Yes (sell accounts), No (soulbound), Partial (name transfers only) | **Partial** — names transferable, identity itself is soulbound |

### Deferred Features

| Feature | Reason to Defer |
|---------|----------------|
| Social recovery (key rotation) | Requires contract modification, separate security audit |
| Identity delegation | Complex trust model |
| Reputation decay | Needs observation period to calibrate parameters |
| Name auctions | Requires auction contract integration |
| Cross-chain identity | Requires bridge identity verification |
| Identity NFT badges | Cosmetic, not functional |
| Reputation staking (stake your rep on a claim) | Novel mechanic, needs research |

---

*End of MoltyID Wallet Integration Plan*
