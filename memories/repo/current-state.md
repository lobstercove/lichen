# Current State

Last reviewed: 2026-04-23

## Durable Facts

- Repo root README and release docs now treat `v0.5.7` as the active release line.
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
- The 2026-04-23 production-pass handover records the pending `v0.5.7` hardening release contents.
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
