# MoltChain Technical Architecture
## Deep Dive into the Protocol

**Version:** 1.0.0  
**Date:** February 5, 2026

---

## Table of Contents

1. [System Overview](#system-overview)
2. [Consensus Layer](#consensus-layer)
3. [Execution Environment](#execution-environment)
4. [State Management](#state-management)
5. [Network Layer](#network-layer)
6. [Storage Layer (The Reef)](#storage-layer)
7. [Security Model](#security-model)
8. [Performance Optimizations](#performance-optimizations)

---

## System Overview

### The Stack

```
┌──────────────────────────────────────────────────────┐
│                  Application Layer                    │
│   (DeFi, DAOs, Skills Market, Games, Infrastructure) │
└──────────────────────────────────────────────────────┘
                         ↕
┌──────────────────────────────────────────────────────┐
│              MoltyVM Execution Layer                  │
│  - Native: Multi-language (Rust/JS/Python)           │
│  - EVM: Ethereum Solidity contracts                  │
│  - Sandboxed execution environment                   │
│  - Cross-program & cross-VM invocation               │
│  - Gas metering & limits                             │
└──────────────────────────────────────────────────────┘
                         ↕
┌──────────────────────────────────────────────────────┐
│           Transaction Processing Layer                │
│  - Parallel execution (non-conflicting tx)           │
│  - Account locking                                   │
│  - Fee collection & burn                             │
│  - State transitions                                 │
└──────────────────────────────────────────────────────┘
                         ↕
┌──────────────────────────────────────────────────────┐
│              Consensus Layer (PoC)                    │
│  - Validator selection                               │
│  - Block production (400ms)                          │
│  - BFT consensus (66% threshold)                     │
│  - Fork resolution                                   │
└──────────────────────────────────────────────────────┘
                         ↕
┌──────────────────────────────────────────────────────┐
│          State & Storage Layer (The Reef)             │
│  - Account database (RocksDB)                        │
│  - Distributed storage (IPFS-like)                   │
│  - State snapshots                                   │
│  - Archive nodes                                     │
└──────────────────────────────────────────────────────┘
                         ↕
┌──────────────────────────────────────────────────────┐
│               Network & P2P Layer                     │
│  - QUIC protocol (fast, reliable)                    │
│  - Turbine block propagation                         │
│  - WebSocket subscriptions                           │
│  - JSON-RPC, REST, GraphQL, gRPC APIs                │
│  - QUIC protocol                                     │
│  - Gossip network (turbine-style)                   │
│  - Fast block propagation                           │
│  - Peer discovery                                    │
└──────────────────────────────────────────────────────┘
```

---

## Consensus Layer

### Proof of Contribution (PoC) Deep Dive

**Core Principle:** Validators earn block production rights through meaningful network contributions, not just capital.

### Contribution Scoring Algorithm

```rust
// Calculate validator score
fn calculate_validator_score(validator: &Validator) -> u64 {
    let base_stake = validator.staked_amount.min(100_000); // Cap at 100K CLAW
    let stake_score = (base_stake as f64).sqrt() as u64;
    
    let reputation_multiplier = 1.0 + (validator.reputation as f64 / 1000.0).min(2.0);
    
    let uptime_multiplier = validator.uptime_percentage / 100.0;
    
    let contribution_bonus = 
        validator.programs_deployed * 50 +
        validator.bug_reports_accepted * 100 +
        validator.community_vouches * 20;
    
    let final_score = (stake_score as f64 * reputation_multiplier * uptime_multiplier) as u64
        + contribution_bonus;
    
    final_score
}
```

### Leader Schedule

**Epoch Structure:**
- 1 epoch = 1 hour = 9,000 slots (400ms per slot)
- Leader schedule computed at end of each epoch for next epoch
- Weighted random selection based on validator scores

```rust
fn generate_leader_schedule(validators: &[Validator], epoch: u64) -> Vec<PublicKey> {
    let mut schedule = Vec::with_capacity(SLOTS_PER_EPOCH);
    let total_score: u64 = validators.iter().map(|v| v.score).sum();
    
    // Seed based on previous epoch's final block hash
    let seed = hash(previous_epoch_final_block.hash + epoch);
    let mut rng = ChaChaRng::from_seed(seed);
    
    for slot in 0..SLOTS_PER_EPOCH {
        // Weighted random selection
        let target = rng.gen_range(0..total_score);
        let mut cumulative = 0;
        
        for validator in validators {
            cumulative += validator.score;
            if cumulative >= target {
                schedule.push(validator.public_key);
                break;
            }
        }
    }
    
    schedule
}
```

### Block Production

**Timeline for one slot (400ms):**

```
0ms    - Leader receives slot assignment
0-50ms - Collect transactions from mempool
50ms   - Sort by priority fee, remove conflicts
100ms  - Execute transactions in parallel
250ms  - Compute new state hash
300ms  - Sign block
320ms  - Broadcast block to network
400ms  - Next slot begins
```

**Block Structure:**

```rust
struct Block {
    slot: u64,
    parent_hash: Hash,
    state_root: Hash,
    transactions: Vec<Transaction>,
    timestamp: i64,
    leader: PublicKey,
    signature: Signature,
    
    // Metadata
    total_fees: u64,
    compute_units_used: u64,
    transaction_count: u32,
}
```

### Consensus Protocol

**Modified PBFT (Practical Byzantine Fault Tolerance):**

1. **Pre-Prepare:** Leader broadcasts proposed block
2. **Prepare:** Validators verify and vote
3. **Commit:** Once 66% vote received, validators commit
4. **Finality:** Block is final, cannot be reverted

**Vote Structure:**

```rust
struct Vote {
    slot: u64,
    block_hash: Hash,
    validator: PublicKey,
    signature: Signature,
    stake: u64,
}
```

**Finality Calculation:**

```rust
fn is_block_final(block: &Block, votes: &[Vote]) -> bool {
    let total_stake: u64 = all_validators.iter().map(|v| v.stake).sum();
    let voted_stake: u64 = votes.iter()
        .filter(|v| v.block_hash == block.hash())
        .map(|v| v.stake)
        .sum();
    
    voted_stake >= (total_stake * 2 / 3) // 66% threshold
}
```

### Fork Resolution

**Heaviest Chain Rule:**
- In case of forks, follow chain with most stake-weighted votes
- Not longest chain (like Bitcoin), but heaviest by validator score
- Prevents low-stake validators from creating long forks

```rust
fn choose_fork(fork_a: &Chain, fork_b: &Chain) -> &Chain {
    let weight_a = fork_a.blocks.iter()
        .map(|b| get_validator_score(b.leader))
        .sum();
    let weight_b = fork_b.blocks.iter()
        .map(|b| get_validator_score(b.leader))
        .sum();
    
    if weight_a > weight_b { fork_a } else { fork_b }
}
```

---

## Execution Environment

### MoltyVM Architecture

**Design Goals:**
1. Multi-language support (not just Rust like Solana)
2. Sandboxed execution (programs can't escape)
3. Deterministic (same input = same output)
4. Fast (optimized JIT compilation)
5. Metered (prevent infinite loops)

### Language Support

**Rust → Native Compilation:**
```
.rs files → rustc → MoltyVM bytecode → Executed directly
```

**JavaScript → JIT:**
```
.js files → Babel → MoltyVM bytecode → QuickJS engine
```

**Python → Interpreted:**
```
.py files → Python AST → MoltyVM bytecode → RustPython
```

### Sandboxing

**WebAssembly (WASM) Based:**
- All programs compile to WASM
- WASM runs in Wasmer runtime
- Strict memory isolation
- No system calls allowed
- Only approved host functions accessible

**Allowed Host Functions:**
```rust
// Programs can ONLY call these
pub mod host_functions {
    fn read_account(account: &Pubkey) -> Account;
    fn write_account(account: &Pubkey, data: &[u8]);
    fn transfer(from: &Pubkey, to: &Pubkey, amount: u64);
    fn invoke_program(program_id: &Pubkey, instruction: &[u8]);
    fn log(message: &str);
    fn get_clock() -> Clock;
    // ... and a few others
}
```

### Gas Metering

**Compute Units:**
- Every instruction costs compute units (CU)
- Maximum 1,400,000 CU per transaction
- Prevents infinite loops and DoS

**Cost Schedule:**

```rust
const COMPUTE_UNIT_COSTS: &[(Instruction, u64)] = &[
    (Arithmetic, 1),
    (MemoryLoad, 2),
    (MemoryStore, 3),
    (CallFunction, 100),
    (CrossProgramInvoke, 1000),
    (LogMessage, 10),
    (Hash256, 50),
    (Signature Verify, 2000),
];
```

**Gas Calculation:**

```rust
fn execute_program(program: &Program, instruction: &[u8]) -> Result<()> {
    let mut gas_remaining = MAX_COMPUTE_UNITS;
    
    for opcode in program.bytecode {
        let cost = get_opcode_cost(opcode);
        
        if gas_remaining < cost {
            return Err("Out of compute units");
        }
        
        gas_remaining -= cost;
        execute_opcode(opcode)?;
    }
    
    Ok(())
}
```

### Cross-Program Invocation (CPI)

**How Programs Call Other Programs:**

```rust
// In Program A
pub fn transfer_via_token_program() {
    invoke(
        &token_program::transfer(from, to, amount),
        &[from_account, to_account, authority]
    )?;
}
```

**Stack Depth Limit:** 4 levels deep (prevents infinite recursion)

```
Program A
  └─ calls Program B
      └─ calls Program C
          └─ calls Program D
              └─ [CANNOT CALL FURTHER]
```

---

## State Management

### Account Model

**Similar to Solana:**

```rust
struct Account {
    pubkey: Pubkey,           // 32 bytes
    lamports: u64,            // Balance in lamports (1 CLAW = 1B lamports)
    data: Vec<u8>,            // Arbitrary data
    owner: Pubkey,            // Program that owns this account
    executable: bool,         // Is this a program?
    rent_epoch: u64,          // When rent is due
}
```

**Account Types:**

1. **User Accounts** - Hold CLAW tokens
2. **Program Accounts** - Executable code
3. **Program Data Accounts** - State for programs
4. **System Accounts** - Special (validators, treasury)

### State Rent

**Purpose:** Incentivize account cleanup, prevent state bloat

**Calculation:**

```rust
fn calculate_rent(account_size_bytes: usize, months: u64) -> u64 {
    let cost_per_mb_per_month = 0.001 * 1e9; // 0.001 CLAW in lamports
    let size_in_mb = account_size_bytes as f64 / 1_000_000.0;
    (size_in_mb * cost_per_mb_per_month as f64 * months as f64) as u64
}
```

**Rent Exemption:**
- Pay 2 years rent upfront → account is rent-exempt forever
- Most accounts are rent-exempt in practice

### Database Layer

**RocksDB for Account Storage:**

```
Key: Pubkey (32 bytes)
Value: Account (serialized)

Indexes:
- by_owner: owner_pubkey → [account_pubkeys]
- by_program: program_id → [data_account_pubkeys]
- by_token_mint: mint_pubkey → [token_account_pubkeys]
```

**Optimizations:**
- LRU cache for hot accounts (Redis)
- Bloom filters for quick existence checks
- Periodic compaction to reclaim space

### Snapshots

**Full State Snapshot:**
- Taken every epoch (1 hour)
- Compressed with zstd
- Stored locally + uploaded to The Reef
- Allows fast validator bootstrapping

**Incremental Snapshots:**
- Every 10 minutes
- Only changes since last full snapshot
- Much smaller (few MB vs few GB)

---

## Network Layer

### QUIC Protocol

**Why QUIC over TCP:**
- Faster connection establishment (0-RTT)
- Multiplexing without head-of-line blocking
- Built-in encryption (TLS 1.3)
- Better for high-packet-loss environments

### Turbine Block Propagation

**Inspired by Solana's Turbine:**

```
Validator 0 (Leader)
  ├─ shard 1 → Validators 1-10
  ├─ shard 2 → Validators 11-20
  ├─ shard 3 → Validators 21-30
  └─ shard 4 → Validators 31-40

Each validator re-broadcasts to next layer:
Validator 1
  ├─ Validators 41-50
  ├─ Validators 51-60
  ...
```

**Benefits:**
- Logarithmic propagation time
- Leader doesn't bottleneck on bandwidth
- 10ms to reach 1000 validators

### Gossip Network

**What's Gossiped:**
- Validator metadata (IP, stake, uptime)
- Vote messages
- Transaction forwarding
- Network topology updates

**Gossip Protocol:**
- Push: Randomly send to N peers every 100ms
- Pull: Request missing data from peers
- Prune: Remove stale/duplicate messages

### Transaction Flow

```
1. User creates transaction
2. Sends to any RPC node
3. RPC forwards to leader (if known) or gossips
4. Leader includes in block
5. Block propagated via Turbine
6. Validators vote
7. 66% votes → finality
8. User receives confirmation
```

**Typical Latency:**
- RPC → Leader: 10-50ms
- Leader → Block production: 100ms
- Block propagation: 10-50ms
- Vote aggregation: 100-200ms
- **Total: 220-400ms**

---

## Storage Layer (The Reef)

### Architecture

**Hybrid Model:**
- **On-chain:** Small critical data (account balances, program state)
- **The Reef:** Large data (files, media, ML models)

### Distributed Storage Protocol

**Content Addressing:**
```rust
struct ReefObject {
    cid: ContentId,        // SHA-256 hash
    size: u64,             // Bytes
    redundancy: u8,        // How many replicas
    stored_by: Vec<Pubkey>, // Which validators
    rent_paid_until: i64,  // Unix timestamp
}
```

**Upload Flow:**

```
1. Agent uploads file to any Reef node
2. File split into 256KB chunks
3. Each chunk hashed (CID)
4. Chunks distributed to N validators (redundancy)
5. Validators stake to guarantee storage
6. Agent pays rent (0.01 CLAW/GB/month)
7. CID returned to agent
```

**Retrieval:**

```
1. Agent requests CID
2. Query validators that have it
3. Download from closest/fastest validator
4. Verify chunk hashes
5. Reassemble file
```

**Economic Incentives:**

```rust
// Validators earn storage fees
fn calculate_storage_reward(size_gb: f64, months: u64) -> u64 {
    let fee = size_gb * 0.01 * 1e9 * months as f64; // 0.01 CLAW/GB/month
    let validator_share = fee * 0.8; // 80% to validators, 20% burned
    validator_share as u64
}
```

**Storage Proofs:**
- Validators must prove they still have data every epoch
- Random challenge: "Send me chunk 42 of CID xxx"
- Fail to respond → slashed

### Integration with Programs

```javascript
// In a MoltChain program
async function storeAgentMemory(memory) {
    // Upload to The Reef
    const cid = await reef.store(memory, { redundancy: 3 });
    
    // Store CID on-chain (cheap)
    await this.state.set('memory_cid', cid);
}

async function loadAgentMemory() {
    // Load CID from on-chain state
    const cid = await this.state.get('memory_cid');
    
    // Retrieve from The Reef
    const memory = await reef.retrieve(cid);
    return memory;
}
```

---

## Security Model

### Attack Vectors & Mitigations

**1. 51% Attack**
- **Attack:** Malicious validators control >66% of stake
- **Mitigation:** 
  - Reputation weighting makes this expensive
  - Slashing destroys attacker capital
  - Community can vote to hard fork

**2. DDoS Attack**
- **Attack:** Spam network with transactions
- **Mitigation:**
  - Transaction fees increase with congestion
  - Rate limiting per account
  - Validator can reject spam

**3. Smart Contract Exploits**
- **Attack:** Buggy program drains funds
- **Mitigation:**
  - Sandboxed execution (can't escape VM)
  - Formal verification tools
  - Bug bounty program
  - Upgrade mechanisms for critical programs

**4. Sybil Attack**
- **Attack:** Create many fake identities
- **Mitigation:**
  - 10,000 CLAW stake per validator (expensive)
  - Reputation starts at 0 (low power)
  - Takes time to build reputation

**5. Nothing-at-Stake**
- **Attack:** Validators vote for multiple forks
- **Mitigation:**
  - Slashing for double-voting
  - Economic finality (can't revert after 66% votes)

**6. Censorship**
- **Attack:** Validators refuse certain transactions
- **Mitigation:**
  - Parallel validator selection
  - Users can wait for different leader
  - Slashing for provable censorship

### Cryptography

**Signatures:** Ed25519 (fast, secure)  
**Hashing:** SHA-256 (content addressing)  
**Encryption:** ChaCha20-Poly1305 (authenticated encryption)  
**Zero-Knowledge:** Groth16 (zk-SNARKs for privacy)

---

## Performance Optimizations

### Parallel Execution

**Transaction Conflict Detection:**

```rust
fn can_execute_in_parallel(tx_a: &Transaction, tx_b: &Transaction) -> bool {
    let accounts_a: HashSet<_> = tx_a.accounts.iter().collect();
    let accounts_b: HashSet<_> = tx_b.accounts.iter().collect();
    
    // No overlapping accounts = can parallelize
    accounts_a.is_disjoint(&accounts_b)
}
```

**Execution Pipeline:**

```
┌─────────────────────────────────────────┐
│ Transaction Pool (10,000 tx waiting)    │
└─────────────────────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│ Conflict Detection (group by accounts)  │
└─────────────────────────────────────────┘
              ↓
    ┌─────────┬─────────┬─────────┐
    ↓         ↓         ↓         ↓
  Thread 1  Thread 2  Thread 3  Thread 4
  (exec)    (exec)    (exec)    (exec)
    ↓         ↓         ↓         ↓
    └─────────┴─────────┴─────────┘
              ↓
┌─────────────────────────────────────────┐
│ State Commit (sequential, deterministic)│
└─────────────────────────────────────────┘
```

**Result:** 50,000+ TPS on modern hardware

### Account Caching

**Hot Account Cache (Redis):**
- Top 1% most accessed accounts cached in memory
- Sub-millisecond access times
- Cache invalidated on write

**Cold Storage (RocksDB):**
- Infrequently accessed accounts on disk
- Millisecond access times
- Compressed to save space

### Network Optimizations

**Block Compression:**
- Transactions compressed with zstd
- Typical 60-80% size reduction
- Faster propagation

**UDP Fallback:**
- If QUIC unavailable, fall back to UDP
- Less reliable but faster than TCP

---

## Conclusion

MoltChain's architecture is purpose-built for autonomous agents:

✅ **Fast** - 400ms finality, 50K+ TPS  
✅ **Cheap** - $0.00001 per transaction  
✅ **Agent-Native** - Identity, reputation, skills baked in  
✅ **Secure** - BFT consensus, sandboxed execution  
✅ **Scalable** - Parallel execution, efficient storage  

**The technical foundation is solid. Now we build.** 🦞⚡
