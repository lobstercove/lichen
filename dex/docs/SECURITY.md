# MoltyDEX Security Architecture

## Overview

MoltyDEX implements defense-in-depth across smart contracts, RPC server, custody bridge, and client SDK.

---

## Smart Contract Security

### Arithmetic Safety
All DEX contracts use checked arithmetic to prevent overflow/underflow:
- `checked_add`, `checked_sub`, `checked_mul`, `checked_div` — all return errors on overflow
- `saturating_add` for insurance fund accumulation (capped, never wraps)
- `u128` intermediate values for liquidity math to prevent truncation
- **PnL bias**: Margin PnL stored as `u64` biased by `2^63` to handle negative values without signed integers

### Access Control
- Contract functions validate `caller` address on all state-modifying operations
- Admin functions (pair creation, parameter changes) restricted to deployer
- Governance proposals require quorum before execution
- Time-locks on governance execution (48h default)

### Minimum Amounts
- `MIN_AMOUNT = 100` enforced on all deposits, orders, and collateral
- Prevents dust attacks and precision abuse
- Lot size enforcement on order quantities

### Reentrancy Protection
- MoltChain VM is single-threaded per transaction — no reentrant calls
- State changes committed atomically at end of execution

### Tested Attack Vectors (121 adversarial tests + 14 E2E)
| Attack | Mitigation | Test Suite |
|--------|-----------|------------|
| Overflow in add_margin | checked_add | dex_margin adversarial |
| Insurance fund overflow | saturating_add | dex_margin adversarial |
| Liquidity calc overflow | u128 checked_mul chain | dex_amm adversarial |
| Zero deposit | MIN_AMOUNT=100 | musd_token adversarial |
| Self-trade | Order matching skips same-trader | dex_core adversarial |
| Price manipulation | Tick size enforcement | dex_core adversarial |
| Flash loan | No atomic composability across contracts | by design |
| Front-running | Batch execution within blocks | by design |

---

## RPC Server Security

### Input Validation
- All REST endpoints validate parameter types, ranges, and formats
- Address parameters validated as hex strings
- Numeric parameters bounds-checked before passing to contracts
- Query parameter limits enforced (max depth=500, max limit=1000)

### Rate Limiting
- Per-IP rate limits: 100 read/s, 20 write/s
- WebSocket: max 10 subscriptions per connection
- Connection limits: max 10,000 concurrent WebSocket connections

### Authentication
- Transaction signing required for all write operations
- Ed25519 signature verification in the transaction processing pipeline
- API keys for rate limit exemption (coming)

### Transport
- HTTPS via nginx reverse proxy with TLS 1.3
- WebSocket Secure (WSS) for production
- CORS headers configured per-environment

---

## Custody Bridge Security

### Key Management
- Bridge operator keys stored in encrypted keyfiles
- Multi-sig required for large transfers (>$100K)
- HSM integration planned for mainnet

### Reserve Verification
- On-chain proof-of-reserves updated every epoch
- Reserve rebalancing triggers human approval for >5% moves
- Jupiter API (Solana) and Uniswap Router (EVM) for cross-chain swaps

### Circuit Breakers
- Max single transfer: configurable per-asset
- Daily volume limits per bridge direction
- Automatic halt on price deviation >10% from oracle

---

## Client SDK Security

### Key Handling
- Private keys never sent to server
- All transaction signing done client-side
- `@moltchain/sdk` Keypair class uses secure random generation

### Connection Security
- SDK validates TLS certificates
- WebSocket auto-reconnect with exponential backoff (prevents connection storms)
- Request timeout defaults: 30s for writes, 10s for reads

---

## Oracle Security

### Price Feeds
- `moltoracle` contract aggregates from multiple sources
- Staleness check: prices older than 60s are rejected for margin operations
- Deviation check: single-source price >5% from median is filtered

---

## Deployment Security

### Infrastructure
- Docker containers run as non-root
- Read-only filesystem except for data volumes
- Network segmentation: RPC server in DMZ, state store in private subnet
- Prometheus + Grafana monitoring with alerting

### Secrets
- Environment variables for sensitive config (API keys, bridge keys)
- No secrets in version control
- `.env` files in `.gitignore`

---

## Audit Status

| Component | Internal Review | External Audit | Test Coverage |
|-----------|----------------|----------------|---------------|
| DEX Contracts (10) | ✅ Complete | 🔲 Pending | 705 tests |
| Core Contracts (16) | ✅ Complete | 🔲 Pending | 226+ tests |
| RPC Server | ✅ Complete | 🔲 Pending | — |
| Custody Bridge | ✅ Complete | 🔲 Pending | — |
| TypeScript SDK | ✅ Complete | 🔲 Pending | — |

---

## Incident Response

See [RUNBOOK.md](RUNBOOK.md) for detailed incident response procedures.

### Security Contact
- Email: security@moltchain.io
- Bug bounty: See [BUG_BOUNTY.md](BUG_BOUNTY.md)
- Response SLA: 24h acknowledgment, 72h triage
