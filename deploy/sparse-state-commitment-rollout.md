# Sparse State Commitment Rollout

This is the release path for `sparse_v1`, the compact sparse state commitment used to remove full leaf scans from block root computation.

## Local Gates

Run before cutting a release:

```bash
cargo check -p lobstercove-lichen-core -p lichen-validator
cargo test -p lobstercove-lichen-core sparse_state_commitment -- --nocapture
cargo test -p lobstercove-lichen-core state_commitment_schema -- --nocapture
npm run test-frontend-assets
```

If a local 3-validator testnet is already running, verify health and observed cadence without resetting state:

```bash
scripts/start-local-3validators.sh status
curl -sf http://127.0.0.1:8899 -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getMetrics","params":[]}'
```

## Existing Chain Backfill

Backfill can be rolled out before activation. The validator keeps `sparse_v1` current in shadow mode after backfill while `ordered_v0` remains active.

Run on each validator during a normal restart window:

```bash
systemctl stop lichen-validator
/path/to/lichen-validator --rebuild-sparse-state-commitment \
  --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --cache-size-mb 4096
/path/to/lichen-validator --show-state-commitment-schema \
  --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --cache-size-mb 4096
systemctl start lichen-validator
```

## Coordinated Activation

Activation changes the state-root prefix and must be coordinated across the validator set. Do not activate one validator while others are still producing `ordered_v0` roots.

```bash
systemctl stop lichen-validator
/path/to/lichen-validator --activate-sparse-state-commitment \
  --confirm sparse-state-commitment:v1 \
  --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --cache-size-mb 4096
systemctl start lichen-validator
```

No persistent service or timer is required for this command. If a temporary unit is used operationally, remove it after it exits.

For an existing signed chain, historical block headers and commit
signatures are not rewritten. The activation point is the coordinated
restart height: new blocks after activation use the `sparse_v1` state-root
prefix and sparse account/contract roots. Reset testnets, local testnets,
and mainnet genesis can start with `sparse_v1` at slot 0 by setting the
genesis field below.

## Genesis / Reset

For a reset testnet, local private testnet, or future mainnet genesis, new
genesis configs default to `sparse_v1` before creating slot 0:

```json
{
  "state_commitment_schema": "sparse_v1"
}
```

Use `ordered_v0` only when intentionally recreating a legacy compatibility chain:

```json
{
  "state_commitment_schema": "ordered_v0"
}
```

## Account Proofs

`getAccountProof` returns `proof_type=ordered_v0` before activation and
`proof_type=sparse_v1` after activation. Sparse proofs verify against the
account sparse root inside the active composite state commitment.

On an existing signed chain, block headers keep their original header state root.
Validators also write a `post_state_v1` sidecar anchor after deterministic
post-block hooks complete. RPC accepts that anchor only when its block hash
matches the canonical stored block for the slot and its post-state root matches
the account proof root.
