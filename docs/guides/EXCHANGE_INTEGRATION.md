# Lichen Exchange Integration

**Status:** Published testnet-only exchange package
**Created:** 2026-06-29
**Plan:** [../strategy/EXCHANGE_LISTING_READINESS_PLAN_2026-06-29.md](../strategy/EXCHANGE_LISTING_READINESS_PLAN_2026-06-29.md)
**Tracker:** [../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md](../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md)
**Metadata:** [EXCHANGE_CHAIN_METADATA.md](EXCHANGE_CHAIN_METADATA.md)
**Address vectors:** [EXCHANGE_ADDRESS_VALIDATION_VECTORS.md](EXCHANGE_ADDRESS_VALIDATION_VECTORS.md)
**Operations pack:** [../deployment/EXCHANGE_OPERATIONS_PACK.md](../deployment/EXCHANGE_OPERATIONS_PACK.md)
**Rollback anchor:** `v0.5.221`, per operator update on 2026-07-01
**Exchange package tag:** `exchange-testnet-v0.5.221`
**Exchange package release:** `https://github.com/lobstercove/lichen/releases/tag/exchange-testnet-v0.5.221`

This document is the canonical exchange-facing integration guide for native LICN.
It is approved for testnet-only exchange integration under package tag
`exchange-testnet-v0.5.221`. The three-validator local exchange simulation
passed from a clean stack with cleanup evidence on 2026-06-29, and the public
faucet-backed testnet exchange simulation passed after the signed `v0.5.221`
recovery rollout on 2026-07-01. External publication is scoped to testnet-only
integration testing until mainnet launch.

## Publication Gate

The current testnet-only package is published. Do not extend this guide to a
mainnet exchange package while any of these remain open:

- Runtime fee config verification on every public network included in the
  package. Testnet was verified after `v0.5.221`; mainnet remains pending until
  mainnet launch.
- Mainnet launch exchange handoff and full-scope public readiness.

## Current Package Scope

The current exchange package is testnet-only until mainnet launch. Exchange
engineers may use it to test native LICN deposits, withdrawals, finality,
history, and reconciliation on `lichen-testnet-1`.

Do not present this package as mainnet-ready, do not include mainnet deposit or
withdrawal instructions in an external listing sheet, and do not accept mainnet
LICN deposits until the mainnet launch runbook closes its exchange handoff gate.
After mainnet launch, rerun the public readiness gate with `--scope full`, verify
mainnet RPC/WebSocket/archive behavior, and publish an updated signed external
package.

## Scope

This guide covers native LICN deposits and withdrawals. It does not require DEX,
wrapped assets, bridge custody, or oracle contracts for the basic exchange listing
path. Those systems may be relevant to liquidity, market making, or wrapped-asset
products, but they are not part of a native LICN deposit address flow.

Native integration uses:

- Canonical JSON-RPC on `/`.
- Native account addresses in Base58.
- Raw `u64` spores for all balances and accounting.
- Native signed transactions submitted through `sendTransaction`.
- Archive-backed RPC for long-term transaction lookup.

## Chain Facts

Use [EXCHANGE_CHAIN_METADATA.md](EXCHANGE_CHAIN_METADATA.md) for the listing
metadata sheet. The source-backed facts needed by integrators are:

| Field | Value |
| --- | --- |
| Chain name | Lichen |
| Native ticker | `LICN` |
| Base unit | `spore` |
| Decimals | `9` |
| Unit rule | `1 LICN = 1,000,000,000 spores` |
| Native mainnet chain ID | `lichen-mainnet-1` |
| Native testnet chain ID | `lichen-testnet-1` |
| EVM compatibility chain ID | Query `eth_chainId` on `/evm`; live testnet currently returns `0xca3f1595a6c25e9f`. Do not use `8001` for native LICN deposits. |
| Mainnet RPC | `https://rpc.lichen.network` - launch placeholder, excluded from the current testnet-only package |
| Mainnet WebSocket | `wss://rpc.lichen.network/ws` - launch placeholder, excluded from the current testnet-only package |
| Testnet RPC | `https://testnet-rpc.lichen.network` |
| Testnet WebSocket | `wss://testnet-rpc.lichen.network/ws` |
| Explorer | `https://explorer.lichen.network` |

Source files: `core/src/account.rs`, `core/src/network.rs`, `core/src/evm.rs`,
`rpc/src/lib.rs`, `seeds.json`, `developers/shared-config.js`.

Native exchange integrations use the string chain ID returned by
`getNetworkInfo.chain_id` for signing and replay protection. EVM compatibility
uses the `/evm` `eth_chainId` value derived by `rpc/src/lib.rs` from the native
chain ID. The `core/src/evm.rs` `LICHEN_CHAIN_ID = 8001` constant is a core
compatibility/default constant and is not the exchange listing chain ID for
native LICN deposits.

## Address Handling

Native LICN deposit addresses are Base58 strings encoding exactly 32 bytes.
Validation must decode Base58 and reject decoded lengths other than 32 bytes.
A regex alone is not sufficient for exchange deposit validation.

Recommended prefilter:

```text
^[1-9A-HJ-NP-Za-km-z]{32,44}$
```

Required validation:

```text
valid_native_address(address):
    if not matches(address, "^[1-9A-HJ-NP-Za-km-z]{32,44}$"):
        return false
    decoded = base58_decode(address)
    return len(decoded) == 32
```

Source-backed rules:

- `Pubkey::to_base58()` encodes the 32-byte native account ID.
- `Pubkey::from_base58()` decodes Base58 and rejects decoded lengths not equal
  to 32 bytes.
- ML-DSA-65 public keys derive native addresses as a scheme-version byte plus
  the first 31 bytes of `SHA-256(public_key_bytes)`.
- `Pubkey::to_evm()` maps a native pubkey to a `0x...` EVM-format address by
  Keccak-256 hashing the 32-byte native pubkey and taking the last 20 bytes.
  That EVM mapping is not the native LICN deposit address format.

Validation vectors are published in
[EXCHANGE_ADDRESS_VALIDATION_VECTORS.md](EXCHANGE_ADDRESS_VALIDATION_VECTORS.md)
and locked by the focused core test
`account::tests::test_exchange_address_validation_vectors`.

## Accounting Rules

Exchanges must store and reconcile raw spores as integers.

Do not use formatted LICN strings from RPC for ledger accounting. The current
`getBalance` response includes fields such as `licn`, `spendable_licn`,
`staked_licn`, and `locked_licn`, but those formatted strings are emitted with
four decimal places. They are display values only.

Required internal representation:

- Amount type: unsigned integer spores.
- Decimal display: divide by `1,000,000,000` only at the presentation boundary.
- Credit amount: exact `amount_spores` from transaction/account-history data.
- Fee accounting: exact `fee_spores` or fee config result from RPC/source.
- Idempotency key: native transaction hash plus credited account/address.

## Deposit Model

The recommended exchange deposit model is one native Lichen deposit address per
exchange user or per account allocation. Native LICN does not require a memo/tag
for the base transfer flow. If an exchange chooses pooled deposit addresses, the
exchange must maintain its own assignment ledger and accept the operational risk
that user attribution is off-chain.

Minimum flow:

1. Generate or allocate a native account for the user.
2. Persist the user-to-address assignment before showing the address.
3. Poll the deposit address history through `getTransactionsByAddress`.
   WebSocket `subscribeSlots` may be used as a freshness signal, but exchange
   credit must still reconcile through archive-backed JSON-RPC history and
   `getTransaction`.
4. For each candidate transfer, fetch the transaction with `getTransaction`.
5. Confirm the transaction transfers native LICN to the deposit address.
6. Wait for `confirmation_status = "finalized"` plus the configured operational
   buffer.
7. Credit raw spores exactly once using the transaction hash as part of the
   idempotency key.
8. Reconcile address balance with internal credits and pending sweeps.

Evidence: this flow passed in the three-validator exchange simulation on
2026-06-29 and in the public faucet-backed testnet exchange simulation after the
signed `v0.5.221` recovery rollout on 2026-07-01.

## Withdrawal Model

The recommended exchange withdrawal model is:

- Cold wallet: majority reserve custody, offline or tightly controlled signing.
- Hot wallet: limited balance for normal withdrawals.
- Deposit wallets: per-user receive accounts, swept to hot or cold according to
  exchange policy.

Minimum flow:

1. Validate the destination as a native Base58 account decoding to exactly
   32 bytes.
2. Convert the withdrawal amount to spores before transaction construction.
3. Check hot-wallet spendable balance in spores.
4. Build and sign a native transfer transaction with the correct native chain ID.
5. Submit with canonical `sendTransaction`.
6. Persist the returned native transaction hash before retrying.
7. Poll `getTransaction` until the transaction is finalized plus buffer.
8. Mark the withdrawal complete only once.
9. Reconcile the hot wallet balance, withdrawal ledger, and fee spend.

Retry rule: if the exchange has already signed and submitted a withdrawal, do
not create a replacement withdrawal blindly. First poll the recorded hash through
`getTransaction` and account history. Only escalate to manual review if the hash
remains unavailable after the documented timeout policy.

Local evidence: CLI `balance`, `transfer`, `account history`, and `tx` lookup
passed during the exchange simulation. SDK usage remains bound by the SDK
compatibility section: Rust/Python are acceptable for exact integer accounting;
JavaScript is not approved for exchange accounting until it uses lossless u64
JSON parsing.

## Finality And Confirmations

Source-backed protocol status:

- `FINALITY_DEPTH` is `0`.
- `FinalityTracker::mark_confirmed(slot)` advances finalized slot to the same
  slot under the active policy.
- `getSlot` accepts `processed`, `confirmed`, and `finalized` commitment values.
- `getTransaction` adds `confirmation_status` and `confirmations` when the
  transaction has a slot index.

Local validation on 2026-06-29:

- A clean local three-validator `testnet` stack started with
  `scripts/start-local-stack.sh testnet`.
- All three validators reported the same processed, confirmed, and finalized
  slot during the first check.
- A sampled transaction in slot `169` returned `confirmation_status =
  "finalized"` and `confirmations = null` from RPC ports `8899`, `8901`, and
  `8903`.
- Finalized slots were already greater than transaction slot plus 8 and plus 32
  on all three validators for the sampled transaction.
- After restarting validator V2 under supervision, all three validators converged
  at finalized/latest slot `427` with the same block hash, and the sampled
  transaction still resolved as finalized on all three validators.
- Cleanup completed with `scripts/stop-local-stack.sh testnet`; follow-up status
  checks showed local validators, custody, faucet, and source-chain mocks down.

Exchange policy:

- A finalized block is deterministic under the BFT commitment model.
- Exchanges must still use an operational buffer after finality to account for
  endpoint lag, monitoring delay, archive lag, and internal retry races.
- Standard deposit policy: credit only after `getTransaction` reports
  `confirmation_status = "finalized"` and the current finalized slot is at least
  the transaction slot plus 8.
- High-value policy: require finalized slot at least transaction slot plus 32 or
  manual review.

Local finality and the full deposit/withdrawal simulation are validated. Public
testnet validation also passed after the signed `v0.5.221` recovery rollout: the public
simulation funded a customer through the faucet, detected deposit, waited for
the standard and high-value operational buffers, swept, withdrew, and reconciled
history and transaction lookup through finalized slots.

## Archive Requirement

Exchange integrations must use archive-backed RPC. A pruned or state-only node is
not acceptable for listing operations.

The exchange dependency surface requires old data for:

- Deposit replay.
- Reconciliation after outage.
- Withdrawal proof.
- Customer support.
- Internal and external audit requests.

Required methods for archive validation:

- `getBlock(slot)`
- `getLatestBlock`
- `getTransaction(signature)`
- `getTransactionsByAddress(address, options)`
- `getTransactionHistory(address, options)` alias
- `getAccountTxCount(address)`

Archive behavior is regression-tested for hot/cold migration and reopen at both
the storage and RPC boundary. The tests verify cold-backed `getBlock`,
`getTransaction`, `getTransactionsByAddress`, `getTransactionHistory`, and
`getAccountTxCount` after older block bodies, transaction bodies, tx-to-slot
entries, and account history rows move out of hot storage.

Evidence is recorded in
[EXCHANGE_LISTING_READINESS_TRACKER.md](../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md#phase-3-evidence).
The full deposit/sweep/withdrawal simulation passed locally and on the public
testnet after the signed `v0.5.221` recovery rollout. Public mainnet archive/history
readiness remains a mainnet-launch gate.

## SDK Compatibility

For exchange accounting, use exact raw spore integers.

Current SDK boundary:

- Rust SDK: core dependency is pinned to `=0.5.221`; `cargo check` passed.
- Python SDK: acceptable exact-integer SDK path because Python preserves JSON
  integer precision. Archive helpers are available for `get_transaction`,
  `get_block`, `get_transactions_by_address`, `get_transaction_history`, and
  `get_account_tx_count`.
- JavaScript SDK: not approved for exchange accounting yet. It uses native JSON
  parsing, so u64 spore values can exceed JavaScript's safe integer range. It
  may be used for non-accounting archive lookups, but exchange credit/debit logic
  must use Rust, Python, or a raw JSON-RPC client with lossless integer parsing.

## Canonical JSON-RPC Cookbook

All examples use the canonical JSON-RPC route at `/`. Replace placeholder values
with real addresses, slots, signatures, and transaction bytes from the target
network.

### Get Processed Slot

```bash
curl -s https://testnet-rpc.lichen.network \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}'
```

### Get Finalized Slot

```bash
curl -s https://testnet-rpc.lichen.network \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[{"commitment":"finalized"}]}'
```

The handler also accepts `["finalized"]`, but the object form is clearer for
integrators.

### Get Runtime Fee Config

```bash
curl -s https://testnet-rpc.lichen.network \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getFeeConfig","params":[]}'
```

Use the returned `base_fee_spores` and fee split fields as runtime data. Do not
hard-code fee values in exchange accounting.

### Get Balance

```bash
curl -s https://testnet-rpc.lichen.network \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getBalance","params":["<native_base58_address>"]}'
```

Use `spores` and `spendable` for accounting. Treat `licn` and related `*_licn`
fields as display-only.

### Get Latest Block

```bash
curl -s https://testnet-rpc.lichen.network \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getLatestBlock","params":[]}'
```

### Get Block By Slot

```bash
curl -s https://testnet-rpc.lichen.network \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getBlock","params":[12345]}'
```

`getBlock` expects a `u64` slot. Block-hash lookup through this method is not
supported.

### Get Transaction

```bash
curl -s https://testnet-rpc.lichen.network \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getTransaction","params":["<tx_hash_hex>"]}'
```

Expected exchange-relevant fields include the transaction hash/signature, slot,
timestamp, transfer summary fields, fee fields, `confirmation_status`, and
`confirmations`. If the transaction is not indexed by slot, current source can
return `null`; archive/index behavior must be validated before publication.

### Get Address History

```bash
curl -s https://testnet-rpc.lichen.network \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getTransactionsByAddress","params":["<native_base58_address>",{"limit":100}]}'
```

Pagination uses `next_before_slot`:

```bash
curl -s https://testnet-rpc.lichen.network \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getTransactionsByAddress","params":["<native_base58_address>",{"limit":100,"before_slot":12345}]}'
```

The alias `getTransactionHistory` currently routes to the same handler.

### Broadcast Native Transaction

```bash
curl -s https://testnet-rpc.lichen.network \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"sendTransaction","params":["<base64_signed_native_transaction>"]}'
```

Optional preflight skip:

```bash
curl -s https://testnet-rpc.lichen.network \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"sendTransaction","params":["<base64_signed_native_transaction>",{"skipPreflight":true}]}'
```

Do not submit EVM-typed transactions through this native method. Current native
preflight rejects EVM transactions on `sendTransaction`.

## WebSocket Path

WebSocket endpoints are configured in `developers/shared-config.js`:

- Mainnet: `wss://rpc.lichen.network/ws`
- Testnet: `wss://testnet-rpc.lichen.network/ws`

Public testnet `subscribeSlots` validation passed against
`wss://testnet-rpc.lichen.network/ws` after the signed `v0.5.221` recovery rollout on
2026-07-01. WebSocket notifications are acceptable as a wake-up/freshness signal,
but polling archive-backed JSON-RPC remains the canonical exchange credit path
because it provides idempotent transaction, account-history, and reconciliation
records.

## Reconciliation Requirements

At minimum, an exchange must reconcile:

- Sum of credited deposits in spores.
- Sum of completed withdrawals in spores.
- Sum of fees paid in spores.
- Deposit wallet balances.
- Hot wallet balance.
- Cold wallet balance.
- Pending sweeps.
- Pending withdrawals.
- Transaction hashes and finalized slots for every credit/debit.

The local simulation proved that retries do not duplicate deposits or withdrawals
and that balances reconcile after fees. Public testnet must repeat this before
external publication.

## Local Exchange Simulation Gate

The local production-parity testnet exchange simulation passed on 2026-06-29.
Use the same commands to rerun the local gate after material RPC, CLI, SDK,
finality, archive, or wallet-flow changes:

```bash
scripts/start-local-stack.sh testnet
scripts/status-local-stack.sh testnet
```

The simulation must cover:

- Generate hot, cold, user deposit, and withdrawal destination wallets.
- Send native LICN to the deposit wallet.
- Detect and credit the deposit once.
- Sweep from deposit wallet to hot wallet when applicable.
- Withdraw from hot wallet to destination.
- Verify `getTransaction` and address history for deposit, sweep, and withdrawal.
- Restart one validator and prove archive/history still resolves old data.
- Stop and clean up:

```bash
scripts/stop-local-stack.sh testnet
```

Cleanup must verify that no local validator, custody, faucet, or source-chain mock
process remains.

Implementation and latest evidence:

- Script: `scripts/qa/exchange_simulation.py`
- Evidence:
  [EXCHANGE_LISTING_READINESS_TRACKER.md](../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md#phase-4-exchange-simulation-evidence)

## Deferred Mainnet Items

The current guide is published only for testnet integration. Mainnet remains
blocked until the mainnet launch exchange handoff closes and the public
readiness gate passes with `--scope full`.
