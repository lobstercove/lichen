# 2026-04-27 - Validator Independent Join And Resume

## Context

The validator join path was audited because the latest VPS redeploy had provisioned joiners by copying seed state instead of proving an empty validator can independently sync from the network.

## What Changed

- Genesis creation now embeds a canonical opcode-41 genesis state bundle in block 0.
- The bundle includes accounts, contract storage, programs, symbol registry indexes, and stats so fresh validators can import exact genesis state without local contract replay.
- Release packaging includes contract WASM artifacts for genesis/runtime parity.
- Fresh validators now fetch authoritative `genesis.json` from seed RPC, import and verify the block-0 state bundle against the block state root, then replay later blocks from peers.
- Runtime startup no longer performs analytics/margin/oracle reconciliation when resuming from existing state. Resume catch-up must be pure block replay.
- Native consensus oracle prices are seeded during genesis creation so they are part of the canonical stats bundle, not synthesized by validators on startup.

## Local Validation

- Reset local testnet state.
- Started V1 from a fresh genesis using rebuilt release binaries.
- Deleted V2/V3 state directories after genesis creation so both joiners started empty and generated their own validator identities.
- V2 and V3 fetched genesis config from V1, imported the canonical block-0 bundle, replayed pending blocks, and reached healthy live sync.
- Verified all three nodes had matching block hash and state root at slot 37.
- Stopped V3 at slot 48, restarted it from its own state, and confirmed it caught up to slot 56 without startup reconciliation or state-root mismatch.
- Verified all three nodes were healthy and matched block hash/state root at slot 62.

## Follow-Up

- Release prep is now targeting `v0.5.19`.
- Version metadata, README, developer SKILL content, release workflow body, developer portal install text, deployment status, and current-state memory have been moved from the `v0.5.18` baseline toward the `v0.5.19` independent-join release.
- Before pushing/tagging, run the focused validator/genesis checks plus the deployment shell syntax checks.
- After the tag workflow creates the draft GitHub Release, attach `SHA256SUMS.sig`, publish the release, then run the clean-slate redeploy from the published `v0.5.19` archive.
- Do not reintroduce RocksDB snapshot provisioning for normal validator joins.
