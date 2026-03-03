# MoltChain Custody Plan (3-of-5 Threshold)

**Status**: Draft - approved direction
**Scope**: Deposit address issuance, confirmation watchers, sweep to treasury, and MOLT credits from treasury allocation (no minting).

## Goals

- Keep Molt wallets user-owned (base58 + EVM-compatible address) for on-chain identity.
- Provide compatibility for SOL/ETH/USDC/USDT deposits via one-time custody addresses.
- Sweep deposits to treasury after confirmations.
- Credit MOLT from treasury allocation (fixed supply, no minting).
- Enforce strong key security with 3-of-5 threshold.

## High-Level Architecture

- Canonical Molt RPC: `/`
- Solana adapter: `/solana`
- EVM adapter: `/evm`
- Custody service (separate microservice) with REST/JSON-RPC API
- Threshold signer network (validators) for sweep and treasury transfers

## Keying Model (3-of-5 Threshold)

- One master seed controls deterministic derivation for deposit addresses.
- Seed is split into 5 shards; any 3 reconstruct.
- Each validator stores one encrypted shard; no single validator can sweep.
- Signing happens via threshold protocol; raw seed never leaves validators.

## Address Derivation

- Deterministic derivation per user, per asset, per chain.
- Suggested path format:
  - `molt/<chain>/<asset>/<user_id>/<deposit_index>`
- New deposit request increments `deposit_index`.
- Returned address is one-time use; never reused.

## Deposit Flow

1) Wallet requests deposit address: `(user_id, chain, asset, amount?)`.
2) Custody derives deposit address and returns `deposit_id` + address.
3) Watcher monitors chain for inbound transfer to address.
4) After N confirmations, mark deposit confirmed.
5) Enqueue sweep job to treasury.
6) After sweep, enqueue MOLT credit job from treasury allocation.

## Sweep and Credit Flow

- Sweep: deposit address -> treasury address on the same chain/asset.
- Credit: treasury sends MOLT to user Molt wallet (no minting).
- Genesis is cold; treasury is warm. Genesis -> treasury top-ups are quorum-gated.

## Data Model (Custody Service)

- `deposit_requests`
  - id, user_id, chain, asset, address, derivation_path, status, created_at
- `deposit_events`
  - id, deposit_id, tx_hash, confirmations, amount, status, observed_at
- `sweep_jobs`
  - id, deposit_id, from_address, to_treasury, chain, asset, status
- `credit_jobs`
  - id, deposit_id, user_id, amount_molt, status

## Validator Responsibilities

- Store shard securely (encrypted at rest).
- Participate in threshold signing for sweeps/treasury transfers.
- No direct access to custody DB or raw deposit keys.

## Failure and Recovery

- Any 3 of 5 validators can continue operations.
- If >= 3 offline, sweeps pause but deposits still detected.
- Shard rotation on validator churn or security events.

## Observability and Auditing

- Audit log for deposit -> sweep -> credit lifecycle.
- Signed records for sweeps and treasury transfers.
- Metrics on confirmations, sweep latency, and credit latency.

## Local Ops (One Command)

Use the custody launcher to auto-wire signer endpoints for local validators.

```bash
cd skills/custody
./run-custody.sh testnet
```

The script sets defaults for:
- `CUSTODY_SIGNER_ENDPOINTS` (local validator signers)
- `CUSTODY_SIGNER_THRESHOLD`
- `CUSTODY_DB_PATH`

You can override any value by exporting it before running the script.

## Custody Environment Variables

- `CUSTODY_DB_PATH` - RocksDB path (default: `./data/custody`).
- `CUSTODY_SOLANA_RPC_URL` - Solana JSON-RPC endpoint.
- `CUSTODY_EVM_RPC_URL` - EVM JSON-RPC endpoint.
- `CUSTODY_SOLANA_CONFIRMATIONS` - Confirmation threshold for Solana (default: 1).
- `CUSTODY_EVM_CONFIRMATIONS` - Confirmation threshold for EVM (default: 12).
- `CUSTODY_TREASURY_SOLANA` - Treasury address for SOL sweeps.
- `CUSTODY_TREASURY_EVM` - Treasury address for ETH sweeps.
- `CUSTODY_SOLANA_FEE_PAYER` - Solana keypair path used to fund ATA creation and token sweeps.
- `CUSTODY_SOLANA_TREASURY_OWNER` - Treasury owner for SPL token accounts.
- `CUSTODY_SOLANA_USDC_MINT` - Override Solana USDC mint (default: mainnet).
- `CUSTODY_SOLANA_USDT_MINT` - Override Solana USDT mint (default: mainnet).
- `CUSTODY_EVM_USDC` - Override EVM USDC contract (default: mainnet).
- `CUSTODY_EVM_USDT` - Override EVM USDT contract (default: mainnet).
- `CUSTODY_MOLT_RPC_URL` - Canonical Molt RPC endpoint for credit transfers.
- `CUSTODY_TREASURY_KEYPAIR` - Treasury keypair path for Molt credits.
- `CUSTODY_SIGNER_ENDPOINTS` - Comma-separated signer base URLs.
- `CUSTODY_SIGNER_THRESHOLD` - Minimum signatures required for a sweep.

## Implementation Phases

1) Custody service skeleton + persistent storage (SQLite or RocksDB).
2) Deposit issuance API + deterministic derivation.
3) Confirmation watchers (Solana/EVM) + sweep queue.
4) Threshold signing API surface for validators.
5) Treasury credit flow from allocation.
6) Tests + docs + operational runbooks.

## Open Decisions

- Chain RPC providers for Solana/EVM.
- Threshold signing protocol and library selection.
- Treasury signing policy and rotation.
