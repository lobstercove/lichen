# Custody Scaling Brainstorm — 10-20K Users

> Date: March 10, 2026
> Context: Currently custody runs on US VPS only (single instance). This document analyzes the path to scaling custody for 10-20K users across 3 VPSes.

## Current Architecture

- **Single instance** on US VPS (15.204.229.189)
- **RocksDB** with file-level lock — only one process can open the DB
- **Rate limiters** (withdrawal, deposit) are in-memory per-process
- **Treasury keypair** — shared signing key for MOLT transactions
- **Threshold signers** — `signer_endpoints` point to `localhost:920x`
- **Ports**: testnet=9105, mainnet=9106

## Constraints for Multi-Instance

### 1. RocksDB File Lock
Only one process can open the DB at a time. The `CustodyState` holds an `Arc<DB>` with file-level locking. Two instances cannot share the same DB directory.

**To fix**: Move to a networked DB (Postgres, SQLite + Litestream replication, or a distributed KV store).

### 2. In-Memory Rate Limiters
Each custody instance maintains independent rate limit state (`WithdrawalRateState`, `DepositRateState`). A user hitting all 3 VPSes would get 3× the withdrawal/deposit quota.

**To fix**: Shared rate limiting via Redis, or a coordinated rate limit service.

### 3. Treasury Nonce Conflicts
All instances would sign with the same treasury keypair. Concurrent MOLT transaction submissions create nonce conflicts — two instances trying to submit at the same time would invalidate each other's transactions.

**To fix**: Nonce serialization service, or designate one instance as the sole writer.

### 4. Threshold Signers Locality
Currently `signer_endpoints` point to `localhost:9201,9202,9203`. Multi-VPS deployment would need signers accessible across VPSes (network-exposed) or local signers on each VPS.

**To fix**: Run signers on each VPS with cross-VPS accessibility, or use a centralized signer cluster.

## Scaling Phases

### Phase 1: Single Instance (Now → 5K users)
**No changes needed.** The bottleneck is external chain polling (Solana RPC, EVM RPC), not CPU/memory. MoltChain's 400ms slots are fast. A single custody instance can handle deposit monitoring, sweep jobs, and withdrawals for several thousand users comfortably.

**Capacity estimate**: ~100 deposits/min, ~50 withdrawals/min on single instance.

### Phase 2: Read Replicas (5K → 15K users)
Run custody on all 3 VPSes with **role separation**:

| Role | VPS | Operations |
|------|-----|-----------|
| **Writer** (primary) | US | Sweeps, withdrawals, credits, rebalancing |
| **Reader** (replica) | EU | Deposit address generation, balance queries, status checks |
| **Reader** (replica) | SEA | Deposit address generation, balance queries, status checks |

**Implementation**:
- Add a `CUSTODY_READ_ONLY=true` env var that disables write endpoints
- Read replicas generate deposit addresses (stateless — derived from user ID + chain)
- Read replicas query the primary via internal API for balance/status
- Load balance reads via Caddy upstream rotation on `custody.moltchain.network`
- Primary handles all on-chain transactions (no nonce conflicts)

**Effort**: ~2-3 days of code changes. No DB migration needed.

### Phase 3: Full Multi-Writer (15K+ users)
Full horizontal scaling with all instances capable of writes:

1. **Database**: Migrate from RocksDB to Postgres (networked, multi-client)
2. **Rate limits**: Redis cluster shared across VPSes
3. **Nonce serialization**: Distributed lock (Redis SETNX) for MOLT transaction submission
4. **Signers**: Each VPS runs its own threshold signer set, or shared signer service
5. **Event dedup**: Webhook events need idempotency keys to prevent duplicate delivery

**Effort**: ~2-3 weeks. Significant architectural change.

## Recommendation

For 10-20K users, **Phase 2 (read replicas)** is the sweet spot:
- Minimal code changes
- Horizontal read scaling (most custody API calls are reads)
- Single writer avoids all hard consistency problems
- No database migration
- Can be implemented incrementally

Only move to Phase 3 if write throughput (withdrawals/sweeps) becomes the bottleneck, which is unlikely under 20K users since write operations are gated by external chain confirmation times (Solana ~30s, EVM ~12s).

## aws-lc-sys / Cross-Compilation Note

The custody crate depends on `aws-lc-sys` (via `reqwest` → `rustls`), which has a known GCC `memcmp` bug in cross-compilation Docker images. Native builds work fine. This doesn't affect custody's runtime behavior — it's purely a build toolchain issue.

If custody needs to be built on non-x86 platforms, consider switching from `rustls` to `native-tls` feature in reqwest, or building natively on each target architecture.
