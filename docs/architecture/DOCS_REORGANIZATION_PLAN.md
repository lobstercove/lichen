# MoltChain Documentation Reorganization Plan
## Clean Structure for Agent & Human Learning

**Date:** February 7, 2026  
**Goal:** Organize scattered docs into logical subdirectories by system  
**Impact:** Easier navigation, better discoverability, agent-friendly structure

---

## 🎯 Current Problems

**Scattered docs:**
```
moltchain/
  ├── BUILD_SPEC.md
  ├── PROGRESS_REPORT.md
  ├── AI_AGENT_SKILLS.md
  ├── COMPLETE_SUMMARY.md
  ├── QUICK_REFERENCE.md
  ├── ROADMAP.md
  ├── CONTRACT_DEVELOPMENT_GUIDE.md
  ├── ... (114 markdown files total!)
  ├── docs/
  │   ├── VISION.md
  │   ├── WHITEPAPER.md
  │   ├── CONTRIBUTORY_STAKE.md
  │   └── ... (few structured docs)
  ├── skills/
  │   └── validator/
  │       ├── SKILL.md
  │       ├── ADAPTIVE_HEARTBEAT.md
  │       └── ...
  ├── wallet/
  │   ├── WALLET_FEATURES.md
  │   ├── WALLET_BUILD_COMPLETE.md
  │   └── ...
  └── website/
      ├── README.md
      ├── DESIGN_FIXES.md
      └── ...
```

**Issues:**
- Docs mixed with code (root level)
- No clear categorization
- Hard to find specific system docs
- Agents struggle to locate info
- Duplication (multiple progress reports)

---

## ✅ Proposed Structure

```
moltchain/
  ├── README.md (overview, quick start, architecture)
  ├── CHANGELOG.md (version history)
  ├── CONTRIBUTING.md (how to contribute)
  │
  ├── docs/ (organized documentation)
  │   │
  │   ├── README.md (documentation index)
  │   │
  │   ├── foundation/ (philosophy & economics)
  │   │   ├── VISION.md
  │   │   ├── WHITEPAPER.md
  │   │   ├── ROADMAP.md
  │   │   ├── TOKENOMICS.md
  │   │   └── PHILOSOPHY.md
  │   │
  │   ├── consensus/ (validator & consensus docs)
  │   │   ├── CONTRIBUTORY_STAKE.md
  │   │   ├── PROOF_OF_CONTRIBUTION.md
  │   │   ├── VALIDATOR_SETUP.md
  │   │   ├── VALIDATOR_ECONOMICS.md
  │   │   ├── SLASHING_POLICY.md
  │   │   ├── ADAPTIVE_HEARTBEAT.md
  │   │   └── REPUTATION_SYSTEM.md
  │   │
  │   ├── contracts/ (smart contract development)
  │   │   ├── QUICK_START.md
  │   │   ├── RUST_CONTRACTS.md
  │   │   ├── JAVASCRIPT_CONTRACTS.md
  │   │   ├── PYTHON_CONTRACTS.md
  │   │   ├── SOLIDITY_EVM.md
  │   │   ├── SECURITY_BEST_PRACTICES.md
  │   │   └── EXAMPLES.md
  │   │
  │   ├── api/ (RPC, WebSocket, SDK docs)
  │   │   ├── RPC_REFERENCE.md
  │   │   ├── WEBSOCKET_API.md
  │   │   ├── REST_API.md
  │   │   ├── RUST_SDK.md
  │   │   ├── JAVASCRIPT_SDK.md
  │   │   ├── PYTHON_SDK.md
  │   │   └── CLI_REFERENCE.md
  │   │
  │   ├── wallet/ (wallet usage & integration)
  │   │   ├── USER_GUIDE.md
  │   │   ├── INTEGRATION_GUIDE.md
  │   │   ├── SECURITY.md
  │   │   └── FEATURES.md
  │   │
  │   ├── explorer/ (block explorer docs)
  │   │   ├── USER_GUIDE.md
  │   │   └── API.md
  │   │
  │   ├── defi/ (DeFi protocols)
  │   │   ├── CLAWSWAP_DEX.md
  │   │   ├── LOBSTERLEND.md
  │   │   ├── REEFSTAKE.md
  │   │   ├── CLAWPUMP_LAUNCHPAD.md
  │   │   └── BRIDGE.md
  │   │
  │   ├── architecture/ (technical design)
  │   │   ├── SYSTEM_OVERVIEW.md
  │   │   ├── CONSENSUS_DESIGN.md
  │   │   ├── VM_ARCHITECTURE.md
  │   │   ├── STATE_MANAGEMENT.md
  │   │   ├── NETWORKING_P2P.md
  │   │   └── SECURITY_MODEL.md
  │   │
  │   ├── operations/ (deployment & ops)
  │   │   ├── DEPLOYMENT_GUIDE.md
  │   │   ├── MONITORING.md
  │   │   ├── TROUBLESHOOTING.md
  │   │   ├── BACKUP_RECOVERY.md
  │   │   └── PERFORMANCE_TUNING.md
  │   │
  │   ├── skills/ (agent-readable guides)
  │   │   ├── README.md (skill index)
  │   │   ├── VALIDATOR_SKILL.md
  │   │   ├── CONTRIBUTOR_SKILL.md
  │   │   ├── DEVELOPER_SKILL.md
  │   │   └── TRADER_SKILL.md
  │   │
  │   └── archive/ (historical docs)
  │       ├── BUILD_LOGS/
  │       ├── PROGRESS_REPORTS/
  │       └── DEPRECATED/
  │
  ├── examples/ (code examples)
  │   ├── contracts/
  │   │   ├── rust/
  │   │   ├── javascript/
  │   │   ├── python/
  │   │   └── solidity/
  │   ├── bots/
  │   │   ├── trading-bot/
  │   │   ├── dao-voter/
  │   │   └── yield-optimizer/
  │   └── integrations/
  │       ├── telegram-bot/
  │       └── discord-bot/
  │
  ├── scripts/ (utility scripts)
  │   ├── setup/
  │   ├── deploy/
  │   └── maintenance/
  │
  └── ... (source code directories)
```

---

## 📋 Migration Plan

### Phase 1: Create Directory Structure (30 minutes)

```bash
cd /path/to/moltchain

# Create main documentation directories
mkdir -p docs/{foundation,consensus,contracts,api,wallet,explorer,defi,architecture,operations,skills,archive}
mkdir -p docs/archive/{BUILD_LOGS,PROGRESS_REPORTS,DEPRECATED}

# Create examples structure
mkdir -p examples/{contracts/{rust,javascript,python,solidity},bots/{trading-bot,dao-voter,yield-optimizer},integrations/{telegram-bot,discord-bot}}

# Create scripts structure
mkdir -p scripts/{setup,deploy,maintenance}
```

### Phase 2: Move Foundation Docs (15 minutes)

```bash
# Philosophy & economics
mv docs/VISION.md docs/foundation/
mv docs/WHITEPAPER.md docs/foundation/
mv ROADMAP.md docs/foundation/
# TODO: Create TOKENOMICS.md (extract from WHITEPAPER)
# TODO: Create PHILOSOPHY.md (extract "The Molt Has Begun" from VISION)
```

### Phase 3: Move Consensus Docs (20 minutes)

```bash
# Validator & staking
mv docs/CONTRIBUTORY_STAKE.md docs/consensus/
mv skills/validator/SKILL.md docs/consensus/VALIDATOR_SETUP.md
mv skills/validator/ADAPTIVE_HEARTBEAT.md docs/consensus/
mv skills/validator/CONTRIBUTORY_STAKE_GUIDE.md docs/skills/VALIDATOR_SKILL.md

# TODO: Create PROOF_OF_CONTRIBUTION.md (extract from consensus code comments)
# TODO: Create VALIDATOR_ECONOMICS.md (reward calculations, timelines)
# TODO: Create SLASHING_POLICY.md (downtime penalties, recovery)
# TODO: Create REPUTATION_SYSTEM.md (how reputation is calculated)
```

### Phase 4: Move Contract Docs (20 minutes)

```bash
# Contract development guides
mv CONTRACT_DEVELOPMENT_GUIDE.md docs/contracts/QUICK_START.md

# TODO: Create language-specific guides:
# - RUST_CONTRACTS.md (from sdk/rust/README.md + examples)
# - JAVASCRIPT_CONTRACTS.md (from sdk/js/README.md)
# - PYTHON_CONTRACTS.md (from sdk/python/README.md)
# - SOLIDITY_EVM.md (EVM compatibility guide)
# - SECURITY_BEST_PRACTICES.md (reentrancy, access control, etc.)
# - EXAMPLES.md (link to examples/ directory)
```

### Phase 5: Move API Docs (20 minutes)

```bash
# API reference materials
# TODO: Extract from rpc/src/lib.rs comments → docs/api/RPC_REFERENCE.md
# TODO: Extract from p2p docs → docs/api/WEBSOCKET_API.md
# TODO: Create REST_API.md if we add HTTP endpoints
# TODO: Enhance SDK docs:
cp sdk/rust/README.md docs/api/RUST_SDK.md
cp sdk/js/README.md docs/api/JAVASCRIPT_SDK.md
cp sdk/python/README.md docs/api/PYTHON_SDK.md
# TODO: Create CLI_REFERENCE.md (molt command docs)
```

### Phase 6: Move Wallet Docs (10 minutes)

```bash
# Wallet documentation
mv wallet/WALLET_FEATURES.md docs/wallet/FEATURES.md
# TODO: Create USER_GUIDE.md (how to use wallet UI)
# TODO: Create INTEGRATION_GUIDE.md (embedding wallet in apps)
# TODO: Create SECURITY.md (best practices, backup, recovery)

# Archive build logs
mv wallet/WALLET_BUILD_COMPLETE.md docs/archive/BUILD_LOGS/
mv wallet/BACK_BUTTON_UPDATE.md docs/archive/BUILD_LOGS/
mv wallet/FULL_MOLT_COMPLETE.md docs/archive/BUILD_LOGS/
```

### Phase 7: Move Explorer Docs (10 minutes)

```bash
# Explorer documentation
# TODO: Create docs/explorer/USER_GUIDE.md (how to use explorer)
# TODO: Create docs/explorer/API.md (explorer API endpoints)
```

### Phase 8: Create DeFi Docs (30 minutes)

```bash
# DeFi protocols (mostly TODOs)
# TODO: Create docs/defi/CLAWSWAP_DEX.md (DEX design, AMM math)
# TODO: Create docs/defi/LOBSTERLEND.md (lending protocol)
# TODO: Create docs/defi/REEFSTAKE.md (from core/src/reefstake.rs)
# TODO: Create docs/defi/CLAWPUMP_LAUNCHPAD.md (token launch platform)
# TODO: Create docs/defi/BRIDGE.md (Solana bridge design)

# Extract ReefStake docs from code:
cat << 'EOF' > docs/defi/REEFSTAKE.md
# ReefStake: Liquid Staking Protocol

**stMOLT - Liquid Staking Token**

## Overview
ReefStake allows users to stake MOLT and receive stMOLT (liquid staking tokens).
stMOLT appreciates in value relative to MOLT through auto-compounding rewards.

[Extract documentation from core/src/reefstake.rs comments]
EOF
```

### Phase 9: Create Architecture Docs (30 minutes)

```bash
# Technical design documentation
# TODO: Create docs/architecture/SYSTEM_OVERVIEW.md (high-level architecture)
# TODO: Create docs/architecture/CONSENSUS_DESIGN.md (PoC algorithm details)
# TODO: Create docs/architecture/VM_ARCHITECTURE.md (MoltyVM internals)
# TODO: Create docs/architecture/STATE_MANAGEMENT.md (Merkle trees, storage)
# TODO: Create docs/architecture/NETWORKING_P2P.md (gossip, discovery)
# TODO: Create docs/architecture/SECURITY_MODEL.md (threat model, mitigations)
```

### Phase 10: Create Operations Docs (20 minutes)

```bash
# Deployment & operations
# TODO: Create docs/operations/DEPLOYMENT_GUIDE.md (prod deployment)
# TODO: Create docs/operations/MONITORING.md (metrics, alerts, dashboards)
# TODO: Create docs/operations/TROUBLESHOOTING.md (common issues & fixes)
# TODO: Create docs/operations/BACKUP_RECOVERY.md (disaster recovery)
# TODO: Create docs/operations/PERFORMANCE_TUNING.md (optimization tips)
```

### Phase 11: Move Website Docs (5 minutes)

```bash
# Keep website docs in website/ (they're already organized)
# Just clean up:
mv website/DESIGN_FIXES.md docs/archive/BUILD_LOGS/
mv website/REFINEMENTS.md docs/archive/BUILD_LOGS/
# Keep website/README.md and website/OVERHAUL_PLAN.md
```

### Phase 12: Archive Old Build Logs (15 minutes)

```bash
# Move all progress reports to archive
mv *PROGRESS*.md docs/archive/PROGRESS_REPORTS/
mv *BUILD*.md docs/archive/BUILD_LOGS/
mv *COMPLETE*.md docs/archive/BUILD_LOGS/
mv *STATUS*.md docs/archive/BUILD_LOGS/
mv QUICK_ANSWERS.md docs/archive/
mv QUICK_REFERENCE.md docs/archive/

# Specific files to archive:
mv ALL_SYSTEMS_OPERATIONAL.md docs/archive/BUILD_LOGS/
mv COMPLETE_SUMMARY.md docs/archive/BUILD_LOGS/
mv BUILD_SPEC.md docs/archive/BUILD_LOGS/
```

### Phase 13: Create Index READMEs (30 minutes)

Create navigation READMEs for each directory:

**docs/README.md:**
```markdown
# MoltChain Documentation

## 📚 Documentation Structure

### Foundation
- [Vision](foundation/VISION.md) - The Molt Has Begun
- [Whitepaper](foundation/WHITEPAPER.md) - Economic model & design
- [Roadmap](foundation/ROADMAP.md) - Development timeline
- [Tokenomics](foundation/TOKENOMICS.md) - Token distribution & utility

### Validators & Consensus
- [Contributory Stake](consensus/CONTRIBUTORY_STAKE.md) - Zero-capital validators
- [Validator Setup](consensus/VALIDATOR_SETUP.md) - How to run a validator
- [Proof of Contribution](consensus/PROOF_OF_CONTRIBUTION.md) - Consensus algorithm
- [Validator Economics](consensus/VALIDATOR_ECONOMICS.md) - Earnings & timelines

### Smart Contracts
- [Quick Start](contracts/QUICK_START.md) - Deploy your first contract
- [Rust Contracts](contracts/RUST_CONTRACTS.md) - Rust development guide
- [JavaScript Contracts](contracts/JAVASCRIPT_CONTRACTS.md) - JS guide
- [Python Contracts](contracts/PYTHON_CONTRACTS.md) - Python guide
- [Security](contracts/SECURITY_BEST_PRACTICES.md) - Best practices

### API & SDKs
- [RPC Reference](api/RPC_REFERENCE.md) - Complete RPC method docs
- [WebSocket API](api/WEBSOCKET_API.md) - Real-time subscriptions
- [Rust SDK](api/RUST_SDK.md) - Rust client library
- [JavaScript SDK](api/JAVASCRIPT_SDK.md) - JS/TS client
- [Python SDK](api/PYTHON_SDK.md) - Python client
- [CLI Reference](api/CLI_REFERENCE.md) - molt command docs

### DeFi Protocols
- [ClawSwap DEX](defi/CLAWSWAP_DEX.md) - Automated market maker
- [LobsterLend](defi/LOBSTERLEND.md) - Lending protocol
- [ReefStake](defi/REEFSTAKE.md) - Liquid staking (stMOLT)
- [ClawPump](defi/CLAWPUMP_LAUNCHPAD.md) - Token launchpad
- [Bridge](defi/BRIDGE.md) - Solana bridge

### For Agents (Skills)
- [Validator Skill](skills/VALIDATOR_SKILL.md) - Become a Self-Made Molty
- [Developer Skill](skills/DEVELOPER_SKILL.md) - Build contracts
- [Trader Skill](skills/TRADER_SKILL.md) - Deploy trading bots
- [Contributor Skill](skills/CONTRIBUTOR_SKILL.md) - Contribute to MoltChain

### Architecture
- [System Overview](architecture/SYSTEM_OVERVIEW.md) - High-level design
- [Consensus Design](architecture/CONSENSUS_DESIGN.md) - PoC algorithm
- [VM Architecture](architecture/VM_ARCHITECTURE.md) - MoltyVM internals
- [State Management](architecture/STATE_MANAGEMENT.md) - Storage & Merkle trees

### Operations
- [Deployment Guide](operations/DEPLOYMENT_GUIDE.md) - Production deployment
- [Monitoring](operations/MONITORING.md) - Metrics & alerting
- [Troubleshooting](operations/TROUBLESHOOTING.md) - Common issues
- [Backup & Recovery](operations/BACKUP_RECOVERY.md) - Disaster recovery

## 🚀 Quick Links

**Getting Started:**
- [Vision](foundation/VISION.md) - Understand the mission
- [Become a Validator](skills/VALIDATOR_SKILL.md) - Zero capital required!
- [Deploy a Contract](contracts/QUICK_START.md) - Build something

**For Developers:**
- [Contract Guide](contracts/QUICK_START.md)
- [API Reference](api/RPC_REFERENCE.md)
- [Examples](../examples/)

**For Validators:**
- [Validator Setup](consensus/VALIDATOR_SETUP.md)
- [Contributory Stake](consensus/CONTRIBUTORY_STAKE.md)
- [Earnings Calculator](consensus/VALIDATOR_ECONOMICS.md)

**For Users:**
- [Wallet Guide](wallet/USER_GUIDE.md)
- [Explorer](explorer/USER_GUIDE.md)
- [DeFi Protocols](defi/)
```

**docs/skills/README.md:**
```markdown
# Agent Skills - Learn to Earn

## What Are Skills?

Skills are agent-readable guides that teach autonomous agents (and humans)
how to perform specific tasks in the MoltChain ecosystem.

## Available Skills

### 🦞 Validator Skill
**Learn to:** Run a MoltChain validator with zero capital  
**Earn:** MOLT rewards, Self-Made Molty badge, NFT achievements  
**Time:** 86 days to full vesting  
**File:** [VALIDATOR_SKILL.md](VALIDATOR_SKILL.md)

### 💻 Developer Skill
**Learn to:** Build and deploy smart contracts  
**Earn:** Contract deployment fees (for popular contracts)  
**Time:** Deploy first contract in 30 minutes  
**File:** [DEVELOPER_SKILL.md](DEVELOPER_SKILL.md)

### 📈 Trader Skill
**Learn to:** Deploy autonomous trading bots  
**Earn:** Trading profits, arbitrage gains  
**Time:** Bot live in 1 hour  
**File:** [TRADER_SKILL.md](TRADER_SKILL.md)

### 🛠️ Contributor Skill
**Learn to:** Contribute to MoltChain core development  
**Earn:** Contributor NFTs, reputation, grant funding  
**Time:** First PR in 1 week  
**File:** [CONTRIBUTOR_SKILL.md](CONTRIBUTOR_SKILL.md)

## How to Use Skills

1. **Pick a skill** that aligns with your goals
2. **Read sequentially** - skills build knowledge step-by-step
3. **Complete checkboxes** - track your progress
4. **Ask for help** - join Discord if stuck
5. **Earn credentials** - graduate with badges/NFTs

## For Autonomous Agents

Skills are written in a format that LLMs can parse and execute:
- Clear prerequisites
- Copy-paste commands
- Expected outputs
- Troubleshooting steps
- Learning checkboxes

Feed a skill to an agent, and it can autonomously complete it!
```

### Phase 14: Update Root README.md (15 minutes)

Update main README.md with new structure:
```markdown
# MoltChain 🦞

**The Agent-First Blockchain**

## Quick Start

### Become a Validator (Zero Capital Required!)
```bash
curl -sSfL https://install.moltchain.network | sh
molt validator start
# Watch your vesting progress: 0% → 100% in ~86 days
```

### Deploy a Smart Contract
```bash
molt new my-contract --lang rust
cd my-contract
molt build
molt deploy --network testnet
```

### Use the Wallet
Open http://localhost:3001 in your browser, or:
```bash
molt wallet create
molt wallet balance
```

## Documentation

📚 **[Full Documentation](docs/README.md)**

Quick links:
- [Vision](docs/foundation/VISION.md) - The Molt Has Begun
- [Validator Guide](docs/skills/VALIDATOR_SKILL.md) - Earn your stake
- [Contract Development](docs/contracts/QUICK_START.md) - Build apps
- [API Reference](docs/api/RPC_REFERENCE.md) - Integrate MoltChain

## Project Structure

```
moltchain/
├── core/           # Blockchain core (consensus, state, VM)
├── validator/      # Validator node
├── rpc/            # RPC server
├── cli/            # molt command-line tool
├── sdk/            # SDKs (Rust, JS, Python)
├── wallet/         # Web wallet UI
├── explorer/       # Block explorer
├── website/        # Landing page
├── docs/           # 📚 Documentation (START HERE)
├── examples/       # Code examples
└── scripts/        # Utility scripts
```

## Community

- **Discord:** https://discord.gg/moltchain
- **Twitter:** @MoltChain
- **GitHub:** https://github.com/moltchain/moltchain
- **Website:** https://moltchain.network

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [Contributor Skill](docs/skills/CONTRIBUTOR_SKILL.md).

## License

Apache 2.0
```

---

## 🎯 Benefits of New Structure

### For Agents
```
✅ Clear categorization by system
✅ Predictable paths (docs/consensus/, docs/api/, etc.)
✅ Skills separated from technical docs
✅ Easy to find "how to X" guides
```

### For Developers
```
✅ Centralized documentation in docs/
✅ Examples separate from docs
✅ API references easy to find
✅ Architecture docs for deep dives
```

### For Validators
```
✅ All validator docs in docs/consensus/
✅ Skills in docs/skills/
✅ Operations guides in docs/operations/
✅ Clear path from setup → graduation
```

### For Contributors
```
✅ Clear contribution guidelines
✅ Architecture docs explain design decisions
✅ Build logs archived (not cluttering root)
✅ Easy to add new docs to existing categories
```

---

## ⏱️ Time Estimate

**Total: ~4-5 hours**

- Phase 1-12 (migration): ~3 hours
- Phase 13-14 (READMEs): ~1 hour
- Testing & verification: ~30 minutes
- Additional doc creation (TODOs): ~8-10 hours (separate task)

---

## 🚀 Execution Steps

```bash
# 1. Run migration script (automated)
cd /path/to/moltchain
./scripts/docs-migration.sh

# 2. Verify structure
tree docs/ -L 2

# 3. Update internal links
grep -r "docs/" . --include="*.md" | grep -v ".git"
# Fix any broken links manually

# 4. Test documentation
# - Click through docs/README.md links
# - Verify skills/ files work
# - Check examples/ references

# 5. Commit
git add docs/ examples/ scripts/
git commit -m "docs: reorganize documentation structure

- Create categorical directories (foundation, consensus, contracts, etc.)
- Move scattered docs from root to appropriate subdirectories
- Archive build logs and progress reports
- Create navigation READMEs for each section
- Update root README with new structure
- Separate skills from technical documentation
- Add placeholder TODOs for missing docs

Benefits:
- Easier navigation for agents and humans
- Predictable document locations
- Better discoverability
- Clean separation of concerns"
```

---

## 📋 TODO: Missing Documentation

After reorganization, create these docs:

**High Priority:**
- [ ] docs/consensus/VALIDATOR_ECONOMICS.md
- [ ] docs/api/RPC_REFERENCE.md
- [ ] docs/api/CLI_REFERENCE.md
- [ ] docs/contracts/RUST_CONTRACTS.md
- [ ] docs/wallet/USER_GUIDE.md

**Medium Priority:**
- [ ] docs/defi/REEFSTAKE.md (extract from code)
- [ ] docs/architecture/SYSTEM_OVERVIEW.md
- [ ] docs/operations/MONITORING.md
- [ ] docs/operations/TROUBLESHOOTING.md

**Low Priority:**
- [ ] docs/defi/CLAWSWAP_DEX.md (future)
- [ ] docs/defi/LOBSTERLEND.md (future)
- [ ] docs/architecture/VM_ARCHITECTURE.md
- [ ] docs/skills/TRADER_SKILL.md

---

**Next Action:** Execute Phase 1-12 to reorganize existing docs, then tackle missing documentation in a separate session.
