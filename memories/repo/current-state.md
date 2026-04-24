# Current State

Last reviewed: 2026-04-23

## Durable Facts

- Repo root README and release docs now treat `v0.5.9` as the active release line.
- Validator RPC activity reporting now prefers the live in-memory validator set, and remote BFT `last_active_slot` updates are fed from signature-verified consensus ingress instead of delayed BFT queue drain.
- The live 3-VPS testnet fleet is now on the exact published `v0.5.9` Linux validator release artifact (`SHA-256 015f267ee617723416737cc62bacd1110343e02384bf30082bc090e375fd2c80`).
- Public testnet RPC now serves `getSporePumpStats`, so Mission Control no longer has a missing backend feed for the SporePump ecosystem card.
- Mission Control monitoring is live on Cloudflare Pages with chain-age uptime, corrected DEX/ecosystem labels, and a health badge driven by validator availability plus consensus/P2P signals instead of the old block-cadence average.
- Cadence telemetry is now observer-side and wall-clock based:
  - `getMetrics` exposes `observed_block_interval_ms`, `cadence_target_ms`, `head_staleness_ms`, `cadence_samples`, `last_observed_block_slot`, and `last_observed_block_at_ms`
  - `slot_pace_pct` is computed from `cadence_target_ms / observed_block_interval_ms`, not second-resolution header timestamps
  - Mission Control prefers cluster-level cadence derived from `getClusterInfo.cluster_nodes[].last_observed_block_slot` and only falls back to single-node observer metrics when needed
- `deploy/setup.sh` now keeps `9100/tcp` open on testnet so the authoritative service-fleet probe can reach remote faucet `/health` endpoints on EU and SEA.
- The Rust workspace is the 8-crate set declared in root `Cargo.toml`.
- `contracts/` contains 29 contract directories, while genesis currently deploys 28 contracts from `GENESIS_CONTRACT_CATALOG`.
- The large CLI modularization effort is complete:
  - `cli/src/main.rs` remains the crate root and top-level dispatcher
  - `cli/src/main_modules.rs` is the module hub
  - thin support routers now exist for chain, contract, stake, NFT, and related command families
- Scoped CLI validation for that modularization already passed in the prior session:
  - formatting
  - `cargo check`
  - `cargo clippy -- -D warnings`
  - tests (`16 passed` in that scoped slice)

## Known Source Drift To Keep In Mind

- `DEPLOYMENT_STATUS.md` may lag live operations until the current rollout is recorded there.
- The 2026-04-22 user handover says:
  - testnet is live on 3 VPSes with BFT consensus
  - current status is already `v0.5.6`
- The 2026-04-23 production-pass handover records the `v0.5.7` hardening release contents, but the active live release line is now `v0.5.9`.
- Treat deployment state as requiring date-aware reconciliation before making operational decisions.

## Likely Next Workstreams

- Phase 2 activation and agent-economy follow-ups from `docs/strategy/PHASE2_ACTIVATION_PLAN.md`
- Additional contracts beyond the 28-contract genesis set
- Frontend work across wallet, explorer, DEX, and marketplace
- DevOps / production hardening
- Security review and test-expansion work

## Working Assumptions For New Sessions

- Start from `AGENTS.md` plus this file, not the full `SKILL.md`.
- Use `SKILL.md` surgically for exact RPC, CLI, transaction, or contract-surface facts.
- Check `git status --short` immediately; unrelated edits are common in this repo.
- When facts conflict, prefer source files and the most recent dated handover over older summary docs.
