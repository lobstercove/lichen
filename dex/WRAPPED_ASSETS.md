# MoltyDEX — Wrapped Assets Architecture (Option B)

> **Decision**: Every external asset deposited into MoltChain becomes a **wrapped receipt token** backed 1:1 by reserves in the treasury multisig. mUSD is the unified quote asset; wSOL and wETH are the base trading assets.

---

## Token Map

| Deposit (external) | Receives on MoltChain | Type | Contract | Decimals |
|--------------------|-----------------------|------|----------|----------|
| USDT (SOL/ETH) | **mUSD** | Stable quote | `musd_token` | 6 |
| USDC (SOL/ETH) | **mUSD** | Stable quote | `musd_token` | 6 |
| SOL | **wSOL** | Wrapped base | `wsol_token` | 9 |
| ETH | **wETH** | Wrapped base | `weth_token` | 18 |
| MOLT (native) | **MOLT** | Native (no wrapping) | — | 9 |

> **Key**: USDT and USDC both produce mUSD. The user picks which stablecoin to deposit; the treasury holds both, the user receives one unified token.

---

## Complete Deposit Flow (A → Z)

### Step 1: User Requests Deposit Address

```
User (on DEX or wallet UI)
  → selects asset & network: e.g. "USDC on Solana"
  → clicks "Deposit"
     │
     ▼
Frontend calls:  POST /deposits { user_id: "<moltchain_pubkey>", chain: "solana", asset: "usdc" }
     │
     ▼
Custody Service (port 9105)
  → generates deposit_id (UUID)
  → derives HD address: SHA-256("molt/solana/usdc/{user_id}/{index}") → Ed25519 seed
  → for SPL tokens: computes Associated Token Account (ATA) and creates it on Solana
  → stores DepositRequest in RocksDB with status "issued"
  → returns { deposit_id, address: "7xKXtg2CW87..." }
     │
     ▼
Frontend shows one-time deposit address + QR code
```

### Step 2: User Sends Funds

```
User sends 500 USDC to the one-time Solana address
  (or sends SOL, ETH, USDT — same flow per chain)
```

### Step 3: Watcher Detects Deposit

```
Custody Service — solana_watcher_loop (runs continuously)
  → iterates all "issued" deposits for chain=solana
  → for SPL tokens: calls getTokenAccountBalance on the ATA
  → detects 500 USDC arrived and is finalized
  → updates deposit status: "issued" → "confirmed"
  → enqueues SweepJob with status "queued"
```

For EVM deposits, `evm_watcher_loop` does the equivalent using `eth_getBalance` (native) or `eth_getLogs` (ERC-20 Transfer events).

### Step 4: Sweep to Treasury

```
Custody Service — sweep_worker_loop
  → picks up SweepJob, transitions: "queued" → "signing"
  → contacts external signer endpoints for threshold signatures (3-of-5)
  → transitions: "signing" → "signed"
  → broadcasts sweep transaction:
      • Solana SPL: Token Program transfer from deposit ATA → treasury ATA
      • Solana native: system transfer from deposit address → treasury address
      • EVM ERC-20: transfer() call to token contract
      • EVM native: signed ETH transfer
  → transitions: "signed" → "sweep_submitted"
  → monitors on-chain confirmation
  → transitions: "sweep_submitted" → "sweep_confirmed"

Funds are now in the treasury multisig wallet.
Nothing changes here from the current system.
```

### Step 5: Credit User on MoltChain ← THE PART THAT CHANGED

**Old system** (replaced):
```
credit_worker_loop → submit_molt_credit()
  → builds system transfer (opcode 0) of native MOLT
  → user receives MOLT shells
```

**New system** (wrapped asset minting via `submit_wrapped_credit()`):
```
credit_worker_loop → submit_wrapped_credit()
  → determines target contract from CreditJob.source_asset:

      ┌─────────────────┬────────────────────────────────────┐
      │ Deposited Asset  │ Credit Action                      │
      ├─────────────────┼────────────────────────────────────┤
      │ USDT             │ musd_token::mint(user, amount)     │
      │ USDC             │ musd_token::mint(user, amount)     │
      │ SOL              │ wsol_token::mint(user, amount)     │
      │ ETH              │ weth_token::mint(user, amount)     │
      └─────────────────┴────────────────────────────────────┘

  → builds contract Call instruction via build_contract_mint_instruction()
  → submits to MoltChain RPC
  → user receives wrapped token on MoltChain
```

**Conversion**: 1:1 raw units. 500 USDC (6 decimals) = 500,000,000 micro-mUSD. 1.5 SOL (9 decimals) = 1,500,000,000 lamport-wSOL. No price conversion, no oracle needed at deposit time.

### Step 6: User Trades on DEX

Now the user has wrapped tokens and can trade any pair:

```
User received 500 mUSD from USDC deposit
  → buys MOLT/mUSD at market price
  → user now holds MOLT

User received 2.0 wSOL from SOL deposit
  → can sell wSOL/mUSD to get mUSD (USD equivalent)
  → can sell wSOL/MOLT to get MOLT directly (no stablecoin involved)
  → can hold wSOL and trade later

User received 0.5 wETH from ETH deposit
  → can trade wETH/mUSD or wETH/MOLT
```

### Step 7: Withdrawal (Reverse)

```
User wants to withdraw 1.0 wSOL back to real SOL

1. User calls:  POST /withdrawals { user_id, asset: "wSOL", amount: 1_000_000_000, dest_chain: "solana", dest_address }
   → Custody creates WithdrawalJob with status "pending_burn"

2. User calls wsol_token::burn(1_000_000_000)    [1.0 SOL in lamports]
   → on-chain: balance deducted, total_supply reduced, BURN event logged

3. Custody Service — withdrawal_worker_loop
   → Phase 1 (pending_burn): verifies burn tx confirmed on MoltChain → status "burned"
   → Phase 2 (burned): collects threshold signatures (3-of-5) → status "signing"
   → Phase 3 (signing): broadcasts outbound transaction → status "broadcasting"
       • SOL: system transfer from treasury → user's Solana address
       • ETH: raw ETH transfer from treasury → user's Ethereum address
       • USDT: SPL/ERC-20 transfer from treasury → user's address
   → Phase 4 (broadcasting): confirms on dest chain → status "confirmed"

4. User receives 1.0 SOL on Solana

Same flow for mUSD → USDT/USDC, wETH → ETH.
For mUSD withdrawals, user specifies preference: "I want USDT on Solana" or "USDC on Ethereum".
```

---

## Trading Pairs (All Markets)

### mUSD Quote Pairs (priced in USD)

| Pair | Description | Fee Tier |
|------|-------------|----------|
| MOLT/mUSD | Core pair — MOLT price in USD | 0.30% |
| wSOL/mUSD | SOL price in USD | 0.30% |
| wETH/mUSD | ETH price in USD | 0.05% |
| REEF/mUSD | REEF ecosystem token | 1.00% |

### MOLT Quote Pairs (priced in MOLT)

| Pair | Description | Fee Tier |
|------|-------------|----------|
| wSOL/MOLT | SOL ↔ MOLT direct | 0.30% |
| wETH/MOLT | ETH ↔ MOLT direct | 0.30% |
| REEF/MOLT | REEF ↔ MOLT ecosystem | 1.00% |

### Stable Pair (deprecated — no longer needed)

The old `USDT/USDC` pool is replaced by mUSD. Since both USDT and USDC produce mUSD, there's no need for a stablecoin swap pool on MoltyDEX.

---

## What Changed in the Custody Service

### Changed: `submit_molt_credit()` → `submit_wrapped_credit()`

The credit worker previously built a system transfer (opcode 0) to send native MOLT. It now builds a contract Call instruction targeting the appropriate wrapped token contract.

```
Changes in custody/src/main.rs:

1. CreditJob struct: added source_asset and source_chain fields
2. CustodyConfig: added musd_contract_addr, wsol_contract_addr, weth_contract_addr
3. build_credit_job(): carries asset/chain from deposit request to CreditJob
4. resolve_token_contract(): maps deposited asset to contract address:
     "usdt" | "usdc" → CUSTODY_MUSD_TOKEN_ADDR
     "sol"           → CUSTODY_WSOL_TOKEN_ADDR
     "eth"           → CUSTODY_WETH_TOKEN_ADDR
5. build_contract_mint_instruction(): builds Call instruction with mint(caller, to, amount)
6. submit_wrapped_credit(): orchestrates the full mint flow
7. New env vars: CUSTODY_MUSD_TOKEN_ADDR, CUSTODY_WSOL_TOKEN_ADDR, CUSTODY_WETH_TOKEN_ADDR
```

### New: Withdrawal endpoint + worker loop

```
New additions:

1. POST /withdrawals endpoint (create_withdrawal):
   - Validates asset + dest_chain combination
   - Creates WithdrawalJob in CF_WITHDRAWAL_JOBS column family

2. withdrawal_worker_loop (5th async loop):
   - 4-phase state machine: pending_burn → burned → signing → broadcasting → confirmed
   - Phase 1: Verify burn tx on MoltChain
   - Phase 2: Collect threshold signatures from signer endpoints
   - Phase 3: Broadcast outbound tx on destination chain
   - Phase 4: Confirm on destination chain

3. Supporting functions:
   - store_withdrawal_job(), list_withdrawal_jobs_by_status()
   - broadcast_outbound_withdrawal()
   - assemble_signed_solana_tx(), assemble_signed_evm_tx()
   - check_solana_tx_confirmed(), check_evm_tx_confirmed()
```

### Unchanged

Everything else stays exactly as-is:
- Deposit address generation (`POST /deposits`)
- Chain watcher loops (solana_watcher_loop, evm_watcher_loop)
- Sweep worker loop (threshold signing, transaction broadcasting)
- RocksDB storage layer
- HD address derivation
- ATA creation for SPL tokens

---

## Contracts

| Contract | Based On | Key Differences |
|----------|----------|-----------------|
| `musd_token` | Original (888 lines) | Name="Molt USD", Symbol="mUSD", Decimals=6, 100K mUSD/epoch cap |
| `wsol_token` | `musd_token` | Name="Wrapped SOL", Symbol="wSOL", Decimals=9, 50K SOL/epoch cap |
| `weth_token` | `musd_token` | Name="Wrapped ETH", Symbol="wETH", Decimals=18, 500 ETH/epoch cap |

All three share identical security model: mint/burn/transfer/approve, reentrancy guard, pause, 3-of-5 multisig admin, reserve attestation with proof hashes, circuit breaker, epoch rate limiting, full audit trail.

> **Future optimization**: Deploy a single `wrapped_token` contract with configurable metadata passed at `initialize()`. This avoids duplicating 500+ lines of identical logic. Each asset would be a separate deployment of the same contract with different init parameters.

---

## Post-Genesis Deployment

Run `tools/deploy_dex.py` on a live validator after genesis:

```
Phase 1 — Deploy wrapped token contracts (musd_token, wsol_token, weth_token)
Phase 2 — Deploy DEX core (dex_core, dex_amm, dex_router)
Phase 3 — Deploy DEX modules (dex_margin, dex_rewards, dex_governance, dex_analytics)

Each contract is: deployed → initialized with admin → cross-referenced

Output: deploy-manifest.json with all contract addresses
```

Then configure the custody service:
```bash
export CUSTODY_MUSD_TOKEN_ADDR=<musd_token address from manifest>
export CUSTODY_WSOL_TOKEN_ADDR=<wsol_token address from manifest>
export CUSTODY_WETH_TOKEN_ADDR=<weth_token address from manifest>
```

---

## Custody Model Summary

```
┌──────────────────────────────────────────────────────────────┐
│                    TRUST BOUNDARY                             │
│  (Custodial: Treasury multisig holds external reserves)       │
│                                                               │
│  USDT ──┐                                                     │
│  USDC ──┤── Treasury Solana wallet (SPL ATAs)                 │
│  SOL  ──┘                                                     │
│                                                               │
│  ETH  ──┐                                                     │
│  USDT ──┤── Treasury Ethereum wallet (EOA)                    │
│  USDC ──┘                                                     │
│                                                               │
│  Protection: 3-of-5 multisig, reserve attestation,            │
│              circuit breaker, epoch rate limits, audit trail   │
└──────────────────────────────────────────────────────────────┘
                            │
                     mint() / burn()
                            │
┌──────────────────────────────────────────────────────────────┐
│                  SELF-CUSTODIAL LAYER                          │
│  (MoltChain — smart contracts hold all funds)                 │
│                                                               │
│  mUSD, wSOL, wETH tokens                                     │
│       │                                                       │
│       ├── User wallets (user holds their tokens)              │
│       ├── dex_core (order book funds)                         │
│       ├── dex_amm (pool liquidity)                            │
│       ├── dex_margin (collateral)                             │
│       └── clawvault (yield vaults)                            │
│                                                               │
│  No operator can move these funds outside contract rules.     │
└──────────────────────────────────────────────────────────────┘
```

---

## Contract Count Update

| Category | Contracts | Count |
|----------|-----------|-------|
| Core (existing) | molt_token, moltbridge, moltrpc, moltdns, moltoracle, moltid, moltdao, moltmail, moltstake, clawback, clawvault, clawswap, moltmedia, clawlock, lockmax, reeftoken | 16 |
| DEX | dex_core, dex_amm, dex_router, dex_governance, dex_rewards, dex_margin, dex_analytics | 7 |
| Wrapped Assets (new) | musd_token, wsol_token, weth_token | 3 |
| **Total** | | **26** |
