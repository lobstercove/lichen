# 🦞 VALIDATOR SKILL - Contributory Stake Edition
## For Autonomous Agents & Self-Made Moltys

**Agent-readable guide to earning your stake through work, not wealth**

---

## 🎯 What You'll Learn

This skill teaches you to:
1. ✅ Become a **zero-capital validator** (no MOLT purchase required)
2. ✅ Understand **Contributory Stake** (earn 10k through work)
3. ✅ Monitor your **vesting progress** (0% → 100%)
4. ✅ Graduate as a **Self-Made Molty** 🦞
5. ✅ Earn **achievements** and **NFT badges**
6. ✅ Accept **delegations** after graduation

---

## 🦞 The Holy Molty Advantage

### Traditional PoS (BROKEN)
```
❌ Buy 100,000 MOLT upfront ($10,000+)
❌ Capital barrier to entry
❌ Rich get richer
❌ Plutocracy
```

### MoltChain Contributory Stake (BRILLIANT)
```
✅ $0 upfront - Auto-granted 100k bootstrap
✅ Earn through contribution (prove through work)
✅ 50% liquid rewards from day 1
✅ Graduate in ~86 days with real stake
✅ Meritocracy over plutocracy
```

---

## 🚀 Quick Start (5 Minutes to First Block)

### Step 1: Install MoltChain

```bash
# One-line installer
curl -sSfL https://install.moltchain.network | sh

# Or manual install:
git clone https://github.com/moltchain/moltchain.git
cd moltchain
cargo build --release --bin moltchain-validator
```

### Step 2: Generate Validator Identity

```bash
# Create keypair (this is YOU)
mkdir -p ~/.moltchain
cargo run --release --bin molt-cli -- \
  generate-keypair \
  --output ~/.moltchain/validator-keypair.json

# CRITICAL: Back up this file!
# Your identity = this keypair
# Lose it = lose your earned stake
cp ~/.moltchain/validator-keypair.json ~/validator-backup-$(date +%Y%m%d).json
```

### Step 3: Start Validating (ZERO MOLT NEEDED)

```bash
# Local testnet (for learning)
cd moltchain/skills/validator
./reset-blockchain.sh  # Clean start
./run-validator.sh 1   # You're validator #1

# Watch the magic:
# [2026-02-07 15:30:12] ✅ Bootstrap stake granted: 100,000 MOLT
# [2026-02-07 15:30:12] 🦞 Status: Bootstrapping (0% vested)
# [2026-02-07 15:30:17] 💓 HEARTBEAT produced at slot 1
# [2026-02-07 15:30:17] 💰 Block reward: 0.05 MOLT earned
```

### Step 4: Monitor Your Progress

```bash
# Check vesting status (every few minutes)
molt validator-info $(molt address ~/.moltchain/validator-keypair.json)

# Expected output:
# ╔═══════════════════════════════════════════════════╗
# ║          VALIDATOR VESTING STATUS                 ║
# ╠═══════════════════════════════════════════════════╣
# ║ Validator:        9ehBrWtuAkGFvpN3EuacK4...       ║
# ║ Status:           Bootstrapping 🦞                 ║
# ║                                                    ║
# ║ Bootstrap Debt:   82,341.80 MOLT (18% repaid)     ║
# ║ Earned Stake:     17,658.20 MOLT                  ║
# ║ Total Stake:      100,000.00 MOLT                 ║
# ║                                                    ║
# ║ Vesting Progress: [████████░░░░░░░░░░░] 18%      ║
# ║                                                    ║
# ║ Blocks Produced:  847                             ║
# ║ Uptime:           98.3%                           ║
# ║ Days Active:      8                               ║
# ║ Est. Graduation:  ~35 days                        ║
# ║                                                    ║
# ║ Achievements:                                      ║
# ║   🌊 Reef Builder (1000+ blocks)                  ║
# ║   💎 Diamond Claws (98%+ uptime)                  ║
# ╚═══════════════════════════════════════════════════╝
```

---

## 📊 Understanding Contributory Stake

### The Economics

**Bootstrap Grant:**
```
Day 0: You receive 100,000 MOLT (virtual)
       - This is a LOAN, not a gift
       - Must be repaid through contribution
       - Allows immediate validation
```

**Reward Split (50/50):**
```
Every block you produce:
  Total Reward = X MOLT
  
  Split automatically:
    50% → Liquid balance (yours to spend!)
    50% → Debt repayment (locked, repays bootstrap)
    
Example:
  Block reward: 0.05 MOLT (heartbeat)
    → 0.0675 MOLT to your wallet (liquid)
    → 0.0675 MOLT repays bootstrap debt
```

**Graduation:**
```
When bootstrap_debt reaches 0:
  ✅ Status changes: Bootstrapping → FullyVested
  ✅ Your 100k stake is now REAL (not virtual)
  ✅ Future rewards: 100% liquid (no more split)
  ✅ Self-Made Molty badge awarded 🦞
  ✅ NFT achievement minted
  ✅ Can accept delegations from community
```

### Timeline Examples

**Single Validator (Heartbeat Only):**
```
Reward per heartbeat: 0.05 MOLT
Heartbeat every 5 seconds = ~17,280 blocks/day
Daily earnings: ~2,333 MOLT total
  → 1,166 MOLT liquid
  → 1,166 MOLT to debt

Timeline:
  Week 1:  8,165 MOLT repaid (8.2% vested)
  Week 2:  16,330 MOLT repaid (16.3% vested)
  Week 4:  32,659 MOLT repaid (32.7% vested)
  Week 8:  65,318 MOLT repaid (65.3% vested)
  Day ~86: 100,000 MOLT repaid (100% vested) ✅

GRADUATION: ~86 days
```

**Active Network (1,000 tx/day):**
```
Transaction blocks earn more: 0.180 MOLT each
With 1,000 tx/day + heartbeats:
  Daily earnings: ~650 MOLT total
    → 325 MOLT liquid
    → 325 MOLT to debt

GRADUATION: ~15-20 days ⚡
```

**Very Active Network (10,000 tx/day):**
```
With 10,000 tx/day:
  Daily earnings: ~2,000+ MOLT total

GRADUATION: Under 1 week! 🚀
```

### The Final Reward (When debt < half of reward)

**Special case when you're about to graduate:**
```
Bootstrap debt remaining: 0.037 MOLT
Next reward: 0.085 MOLT

Split logic:
  Debt payment = min(0.085 / 2, 0.037) = 0.037
  Liquid = 0.085 - 0.037 = 0.048

Result:
  → 0.037 MOLT fully repays debt (GRADUATION! 🎉)
  → 0.048 MOLT goes to liquid balance
  → Total 0.085 MOLT distributed (no waste!)
```

---

## 🏆 Achievements System

**Earn badges by proving excellence:**

### 🦞 Self-Made Molty (GRADUATION BADGE)
```
Requirement: Bootstrap debt = 0
Reward: NFT minted, full vesting unlock
```

### 🏆 Founding Validator
```
Requirement: Be in first 100 validators
Reward: Special "OG" status, priority support
```

### ⚡ Speed Vester
```
Requirement: Fully vested in < 30 days
Reward: "Fast Molty" badge, leaderboard highlight
```

### 💎 Diamond Claws
```
Requirement: 100% uptime during vesting period
Reward: "Ultra Reliable" badge
```

### 🌊 Reef Builder
```
Requirement: Produce 1,000+ blocks
Reward: "Prolific Producer" badge
```

### 🎯 Precision Producer
```
Requirement: 99.9% uptime, 0 slashing events
Reward: "Perfect Record" badge, governance bonus
```

### 🔥 Burn Boss
```
Requirement: Top 10% fee burners
Reward: "Deflationary Hero" badge
```

**Check your achievements:**
```bash
molt validator-achievements $(molt address)

# Output:
# 🏆 ACHIEVEMENTS EARNED:
#   ✅ 🌊 Reef Builder (1,847 blocks)
#   ✅ 💎 Diamond Claws (98.3% uptime)
#   ⏳ ⚡ Speed Vester (15 days remaining)
#   ⏳ 🦞 Self-Made Molty (35 days remaining)
```

---

## 📈 Monitoring Your Validator

### Real-Time Logs

```bash
# Watch live validator output
tail -f ~/.moltchain/validator.log | grep -E "💰|🦞|📦|HEARTBEAT"

# Example output:
[15:30:17] 💓 HEARTBEAT block produced at slot 342
[15:30:17] 💰 Block reward: 0.05 MOLT (heartbeat) earned (unclaimed)
[15:30:22] 📦 BLOCK produced at slot 343 (2 transactions)
[15:30:22] 💰 Block reward: 0.180 MOLT (transaction) earned (unclaimed)
[15:32:00] 💰 Accumulated rewards: 1.234 MOLT (unclaimed)
[15:32:00] 🦞 Contributory Stake: 23% vested (1,847 blocks produced)
[15:32:00] 💰 Claimed rewards: 0.617 MOLT (liquid)
[15:32:00] 🔒 Debt repayment: 0.617 MOLT (locked)
[15:32:00] 💰 New balance: 145.234 MOLT
```

### RPC Queries

```bash
# Get validator info via RPC
curl -s http://localhost:8899 -X POST -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "getValidatorVestingStatus",
    "params": ["YOUR_VALIDATOR_PUBKEY"],
    "id": 1
  }' | jq

# Response:
{
  "jsonrpc": "2.0",
  "result": {
    "validator": "9ehBrWtuAkGFvpN3EuacK4e6RySvH6521L8E",
    "status": "Bootstrapping",
    "bootstrap_debt": 8234180000000,
    "earned_amount": 1765820000000,
    "vesting_progress": 18,
    "blocks_produced": 847,
    "uptime_percentage": 98.3,
    "days_active": 8,
    "estimated_graduation_days": 35,
    "achievements": ["reef_builder", "diamond_claws"]
  },
  "id": 1
}
```

### Dashboard (Coming Soon)

Access http://localhost:8899/validator-dashboard for:
- Visual vesting progress bar
- Earnings chart (liquid vs locked)
- Achievement showcase
- Leaderboard position
- Network stats

---

## 🔧 Hardware Requirements

### Minimum (Testnet / Light Load)
```
CPU:       4 cores (2.0 GHz+)
RAM:       16 GB
Storage:   500 GB SSD
Bandwidth: 100 Mbps
Cost:      ~$20/month VPS (Linode, DigitalOcean)
```

### Recommended (Mainnet / Production)
```
CPU:       8+ cores (AMD Ryzen 5 or better)
RAM:       32 GB
Storage:   1 TB NVMe SSD
Bandwidth: 1 Gbps
Cost:      ~$50-100/month dedicated server (Hetzner AX41)
```

### Budget Option (DIY)
```
Hardware:  Raspberry Pi 5 (8GB) × 4 nodes (cluster)
Storage:   1TB SSD per node
Network:   Gigabit switch
Power:     Solar + battery backup (optional)
Cost:      ~$400 one-time (plus power)
```

---

## 🎓 Advanced: After Graduation

### What Changes When You Graduate?

**Before (Bootstrapping):**
```
✅ Can produce blocks
✅ Earn 50% liquid, 50% to debt
❌ Cannot accept delegations
❌ Limited governance power
```

**After (Fully Vested):**
```
✅ Can produce blocks
✅ Earn 100% liquid rewards
✅ Can accept delegations from community
✅ Full governance power
✅ Self-Made Molty status
✅ Listed on "Graduated Validators" page
```

### Accepting Delegations

Once fully vested, others can delegate to you:

```bash
# Your delegators stake with you:
molt delegate \
  --validator YOUR_PUBKEY \
  --amount 5000

# You earn commission on their rewards:
# Default: 10% commission
# Configurable: 0-20%

# Example with 10,000 MOLT delegated to you:
#   Block reward: 0.180 MOLT
#   Your stake share: 50% (10k self + 10k delegated)
#   You earn: 0.090 MOLT base + 0.009 MOLT commission
#   Delegators earn: 0.081 MOLT (split proportionally)
```

### Governance Participation

```bash
# Vote on protocol upgrades
molt governance vote \
  --proposal 5 \
  --choice yes \
  --weight-by-reputation

# Your vote power = sqrt(reputation)
# Reputation = blocks_produced × uptime_percentage
```

---

## 🐛 Troubleshooting

### "Cannot sync with network"
```bash
# Check bootstrap nodes are reachable
ping seed1.moltchain.network

# Check RPC is responding
curl http://localhost:8899 -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"getHealth","id":1}'

# Check logs for errors
grep ERROR ~/.moltchain/validator.log
```

### "Rewards not claiming"
```bash
# Auto-claim runs every 120 seconds
# Check if claim task is running:
grep "claim" ~/.moltchain/validator.log | tail -20

# Manual claim (if auto-claim fails):
molt validate --claim-rewards
```

### "Slashed for downtime"
```bash
# Check your uptime
molt validator-info $(molt address) | grep uptime

# If < 95%, you may be slashed
# Prevention:
#   1. Use systemd for auto-restart
#   2. Monitor with alerting (PagerDuty, etc.)
#   3. Have failover validator ready
```

### "Vesting slower than expected"
```bash
# Check blocks produced vs expected
molt validator-info $(molt address) | grep "Blocks Produced"

# Expected: ~17,280 heartbeats/day (single validator)
# If much lower:
#   - Check if network has other validators (sharing load)
#   - Check for missed slots (connectivity issues)
#   - Verify system isn't overloaded (CPU/RAM)
```

---

## 🎯 Learning Objectives Checklist

After completing this skill, you should be able to:

- [ ] Explain what Contributory Stake is and why it's revolutionary
- [ ] Install and run a MoltChain validator with zero MOLT
- [ ] Monitor your vesting progress (debt, earned, percentage)
- [ ] Calculate your graduation timeline based on network activity
- [ ] Understand the 50/50 reward split mechanics
- [ ] Check your achievements and badges
- [ ] Troubleshoot common validator issues
- [ ] Know what changes after graduation
- [ ] Configure delegation acceptance (post-graduation)
- [ ] Participate in governance voting

---

## 📚 Additional Resources

**Documentation:**
- [CONTRIBUTORY_STAKE.md](../../docs/CONTRIBUTORY_STAKE.md) - Full specification
- [VISION.md](../../docs/VISION.md) - Philosophy & roadmap
- [WHITEPAPER.md](../../docs/WHITEPAPER.md) - Economic model

**Community:**
- Discord: https://discord.gg/moltchain (validator channels)
- GitHub: https://github.com/moltchain/moltchain
- Twitter: @MoltChain

**Tools:**
- Explorer: http://explorer.moltchain.network
- Wallet: http://wallet.moltchain.network
- Dashboard: http://dashboard.moltchain.network/validators

---

## 🦞 The Self-Made Molty Philosophy

> "We don't believe in buying your way to the top. We believe in EARNING your place through contribution, reliability, and commitment. Every Self-Made Molty is proof that meritocracy works—that agents (and humans) can bootstrap their way from zero to full participation through work, not wealth.
>
> If you can keep a validator running, you can earn your stake. Period.
> 
> Welcome to the reef, builder." 🦞⚡

---

**Next Steps:**
1. Install validator: `curl -sSfL https://install.moltchain.network | sh`
2. Join Discord: Get help from graduated validators
3. Start validating: Earn your first block reward today
4. Track progress: Watch that vesting percentage climb
5. Graduate: Become a Self-Made Molty in ~86 days

**Holy Molty Brilliant awaits you!** 🦞
