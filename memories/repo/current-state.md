# Current State

Last reviewed: 2026-04-26

## Durable Facts

- Repo root README and release docs now treat `v0.5.13` as the active release line.
- `v0.5.13` is the live verified 3-VPS testnet baseline as of 2026-04-26. GitHub Release workflow `24957285876` completed successfully for commit `269b037854cb6395ac62111bc0613505ea8e53f8`; the public release is `https://github.com/lobstercove/lichen/releases/tag/v0.5.13`.
- The signed `v0.5.13` release uses signer address `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk`. The Linux x86_64 archive hash is `f689009d5fa5dd0fcec4f4fbd776783e920ebada396f5032915560d941632ff9`; the installed VPS validator binary hash is `737eb83e25a60aa4512c045a684c0da6bfd8772fd9dc20f42a565515be336c29`.
- The 2026-04-26 `v0.5.13` clean-slate testnet reset generated genesis hash `4b30ed24177dfad0abed76d9776adcf15cc2a2e90f121e4a0f4a5f981f65b707` and state root `ef9150070f927508fc41e74072da95f45aa1f654d384f2b3da8766c2f8f42904`; all 28 genesis contracts deployed and the signed metadata manifest exposes 28 `symbol_registry` entries.
- Fresh `v0.5.13` testnet validator identities are US `6Fu5LwYRGtrsu7GhhffcMx4P2739m1ewnjHHby9hZ7T`, EU `8f4dDcPm7R9Hsrb7p3jtzAmEVwZbuMAmZMtG3o43GoV`, and SEA `5wCT5zeJAfHN9eTwY44Y4dm1B42obiazxa6SjSFibKo`. Public RPC verification after the reset reached slot 307 with exactly 3 active validators, 3 stake entries, no ghost validators, and no post-`v0.5.13` state-root/sync/fatal/startup-integrity warnings in VPS logs.
- `v0.5.10` was published and clean-slate deployed on 2026-04-26, but the testnet reproduced the validator-offline failure at slot 285: SEA hit a state-root mismatch after a stale stake-pool write. Treat `v0.5.10` as superseded by `v0.5.13`.
- `v0.5.11` fixed the stake-pool persistence race and passed the previous slot-285 failure point on testnet, but the clean-slate script still performed an unnecessary post-manifest validator restart that triggered startup integrity warnings during rollout. `v0.5.12` removed that restart.
- `v0.5.13` removes the flawed post-effects startup root marker. The marker was recorded before later deterministic post-block hooks finished, so clean snapshot restarts could log false `STATE INTEGRITY` warnings even when block import/commit root checks were healthy.
- Validator stake-pool persistence is consensus-owned only. The former background "persist in-memory stake pool every 30s" task was removed because it could clone a stale pool, then overwrite RocksDB after a block committed, causing the next block's state root to diverge on that node.
- Startup no longer compares live RocksDB roots to block header roots or to mid-post-hook markers. Header roots are pre-effects commitments; authoritative state-root enforcement happens in block import and BFT commit paths at that boundary. Block-production stake-pool effects are idempotent if the slot update was persisted before the reward completion marker.
- Validator RPC activity reporting now prefers the live in-memory validator set, and remote BFT `last_active_slot` updates are fed from signature-verified consensus ingress instead of delayed BFT queue drain.
- Validator sync pending storage now keeps multiple block candidates per slot and chooses the candidate that chains from the current tip, preventing a wrong-parent candidate from permanently poisoning catch-up.
- Validator identity admission is stake-backed only: block headers and validator announcements no longer create `ValidatorSet` entries, P2P validator-route status is granted only to existing or locally stake-backed validators, and startup drops persisted unbacked validator metadata.
- The failed `v0.5.10` run, the restart-noisy `v0.5.11` run, and the marker-noisy `v0.5.12` run should be treated as superseded by the verified `v0.5.13` clean-slate baseline.
- Public testnet RPC now serves `getSporePumpStats`, so Mission Control no longer has a missing backend feed for the SporePump ecosystem card.
- Mission Control monitoring is live on Cloudflare Pages with chain-age uptime, corrected DEX/ecosystem labels, and a health badge driven by validator availability plus consensus/P2P signals instead of the old block-cadence average.
- Cadence telemetry is now observer-side and wall-clock based:
  - `getMetrics` exposes `observed_block_interval_ms`, `cadence_target_ms`, `head_staleness_ms`, `cadence_samples`, `last_observed_block_slot`, and `last_observed_block_at_ms`
  - `slot_pace_pct` is computed from `cadence_target_ms / observed_block_interval_ms`, not second-resolution header timestamps
  - Mission Control prefers cluster-level cadence derived from `getClusterInfo.cluster_nodes[].last_observed_block_slot` and only falls back to single-node observer metrics when needed
- `deploy/setup.sh` now keeps `9100/tcp` open on testnet so the authoritative service-fleet probe can reach remote faucet `/health` endpoints on EU and SEA.
- The Rust workspace is the 8-crate set declared in root `Cargo.toml`.
- `contracts/` contains 29 contract directories, while genesis currently deploys 28 contracts from `GENESIS_CONTRACT_CATALOG`.
- CI supply-chain coverage now includes all-lockfile Cargo audit, cargo-deny, reproducible npm lockfile installs plus production npm audits, Python SDK dependency consistency checks, Rust CycloneDX SBOM artifact generation, OpenSSF Scorecard reporting, and GitHub artifact provenance attestations on release archives/checksums.
- Active public/developer surfaces have been cleaned of the audited stale claim strings for instant finality, old v0.5.x examples, premature mainnet-ready/production-ready wording, and "not wired" markers; older deployment, audit, changelog, and strategy documents should still be treated as historical unless rechecked.
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
- The 2026-04-23 production-pass handover records the `v0.5.7` hardening release contents; release docs and the live testnet now target `v0.5.13` after clean-slate verification on 2026-04-26, while older deployment docs may still lag.
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
