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

- `v0.5.19` was published first, then marked prerelease/superseded after the VPS clean-slate reset exposed that fresh joiners rejected official seed HTTP RPC bootstrap endpoints while Cloudflare RPC was unavailable during reset.
- `v0.5.20` fixed the bootstrap policy narrowly: HTTPS is still allowed; HTTP is allowed only for devnet/loopback or official seed hosts on expected testnet/mainnet ports.
- Signed `v0.5.20` was published from commit `7435b141ab6db549448239f76bcae54186cc0546`; release workflow `24995138415` passed.
- Clean-slate VPS redeploy from the `v0.5.20` GitHub Release completed on 2026-04-27 in 3m17s.
- Installed validator hash on all three VPSes: `13d33bac70b9a2301c2ed16c668b1c9c9edaf9291125c128c28f951a192ba185`.
- New testnet genesis hash: `9c60dc09c61f1d2819eca6a99c7ca144816affa6dc80dc6186959c80a40390ba`; state root: `eef2f1d247b505b59bc63a8cac051965cf59d8c98331d04070791efefcb241d5`.
- Validator identities: US `6xSWvNvapMugudhcs55FQNUyxhyzHEokMEjA9w5SqSF`, EU `7F2kWaKF9k3oEQK5t2aQeA1Tg6GuyXiwk3iBaQWjrR6`, SEA `71XQXPuq74DTffzhQEH4awnqToG3B9mbqWe8jzEXQdT`.
- Joiner logs confirmed empty-state bootstrap through authoritative seed RPC plus `✅ 📡 [sync] Applied canonical genesis state bundle from block 0`, followed by post-genesis replay.
- Public verification confirmed health OK, 3 active validators, bridge validators=3 required=2 operational, oracle feeds=4 consensus feeds=4 native attestations=12 operational, empty faucet request history, and monitoring/developers portals online.
- Do not reintroduce RocksDB snapshot provisioning for normal validator joins.
