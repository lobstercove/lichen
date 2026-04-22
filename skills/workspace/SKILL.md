# Lichen Workspace Skill

Use this skill when you need to bootstrap into the Lichen repo, regain context after compaction,
or leave behind better continuity for the next session.

## Goal

Build a compact, source-backed understanding of the repo before touching subsystem-specific code.

## Quick Start

1. Read `AGENTS.md`
2. Read `memories/repo/current-state.md`
3. Read `memories/repo/project-map.md`
4. Read `memories/repo/gotchas.md`
5. Read the newest handover in `memories/repo/session-handovers/` or any root `SESSION_HANDOVER_*.md`
6. Run `git status --short`
7. Only then open subsystem docs or source files

## What The Repo Contains

- Rust runtime crates: `core`, `validator`, `rpc`, `cli`, `p2p`, `custody`, `faucet-service`, `genesis`
- Contracts: `contracts/`
- SDKs: `sdk/rust`, `sdk/js`, `sdk/python`
- Frontends: `wallet`, `explorer`, `dex`, `marketplace`, `developers`, `programs`, `monitoring`, `faucet`, `website`
- Ops/docs: `deploy`, `infra`, `scripts`, `docs`, `.github`

## Facts Worth Remembering

- Native signing is `ML-DSA-65`
- The live shielded runtime uses native Plonky3/FRI STARK proof envelopes
- `contracts/` has 29 directories
- genesis currently deploys 28 contracts
- the root `SKILL.md` is exhaustive and useful, but not the right first read for every task

## Choosing The Next Read

- Deployment or VPS work:
  - `DEPLOYMENT_STATUS.md`
  - `docs/deployment/PRODUCTION_DEPLOYMENT.md`
- Roadmap or prioritization:
  - `docs/strategy/PHASE2_AGENT_ECONOMY.md`
  - `docs/strategy/PHASE2_ACTIVATION_PLAN.md`
  - `docs/foundation/ROADMAP.md`
- Rust runtime changes:
  - target crate source files
  - relevant sections in `SKILL.md`
- Contract work:
  - target contract `src/lib.rs`
  - `genesis/src/lib.rs`
  - `docs/contracts/`
- Frontend work:
  - portal-local files
  - `monitoring/shared/`
  - `scripts/deploy-cloudflare-pages.sh`

## Validation Commands

```bash
cargo fmt --all
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace --release
npm run test-frontend-assets
npm run test-wallet
npm run test-wallet-extension
```

Contract-local:

```bash
cd contracts/<name>
cargo check
cargo test --release
cargo build --target wasm32-unknown-unknown --release
```

## Session-End Hygiene

When durable facts change:

- update `memories/repo/current-state.md`
- add reusable traps to `memories/repo/gotchas.md`
- add a dated handover in `memories/repo/session-handovers/` if the next session will need context
- update `DEPLOYMENT_STATUS.md` only if live deployment state changed

## Guardrails

- Do not clean up unrelated changes in a dirty worktree
- Do not assume all docs agree; compare dates
- Do not patch many frontend shared-helper copies by hand if the change belongs in the canonical source
- Do not use raw `wrangler pages deploy` for normal portal deploys
