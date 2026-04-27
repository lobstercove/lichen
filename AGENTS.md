# Lichen Workspace Guide

This file is the fastest stable bootstrap for new sessions in this repository.
Use it before diving into subsystem docs or the large root `SKILL.md`.

## Read Order

1. `AGENTS.md`
2. If present locally, private memory files under `memories/repo/`:
   - `memories/repo/current-state.md`
   - `memories/repo/project-map.md`
   - `memories/repo/gotchas.md`
   - latest dated handover under `memories/repo/session-handovers/`
3. Any root `SESSION_HANDOVER_*.md`
4. Task-specific docs:
   - deployment: `DEPLOYMENT_STATUS.md`, `docs/deployment/PRODUCTION_DEPLOYMENT.md`
   - roadmap / priorities: `docs/strategy/PHASE2_AGENT_ECONOMY.md`, `docs/strategy/PHASE2_ACTIVATION_PLAN.md`, `docs/foundation/ROADMAP.md`
   - contracts: `docs/contracts/`, `genesis/src/lib.rs`, contract-local `src/lib.rs`
   - frontend: portal-local files plus shared helpers under `monitoring/shared/`
5. `SKILL.md`, but only the sections needed for the task

`memories/repo/` is local-private continuity context and is intentionally not tracked in the public repository.

## What Lichen Is

Lichen is a custom Layer 1 blockchain aimed at agent-native applications.
This repo contains the chain runtime, validator, RPC/WebSocket server, CLI, genesis tooling,
custody and faucet services, SDKs, frontends, deployment automation, and protocol contracts.

## Verified Repo Facts

- Native signing is post-quantum `ML-DSA-65`; transactions carry self-contained `PqSignature` objects.
- The live shielded runtime uses native Plonky3/FRI STARK proof envelopes with Poseidon2-derived commitments.
- The Cargo workspace contains 8 crates: `core`, `validator`, `rpc`, `cli`, `p2p`, `faucet-service`, `custody`, `genesis`.
- `contracts/` currently has 29 contract directories, but `genesis/src/lib.rs` deploys 28 contracts at genesis.
  `mt20_token` exists in-tree but is not in `GENESIS_CONTRACT_CATALOG`.
- Frontend portals live in `wallet`, `explorer`, `dex`, `marketplace`, `developers`, `programs`, `monitoring`, `faucet`, and `website`.
- Release/status docs currently center on `v0.5.7`, but some checked-in deployment docs lag newer handovers.
  Always compare document dates before assuming "current" means the same thing across files.

## Repo Map

- `core/`: state machine, consensus primitives, contract runtime, shielded runtime
- `validator/`: validator binary, startup wiring, runtime services, updater
- `rpc/`: JSON-RPC, REST, WebSocket, shielded proof helpers
- `p2p/`: peer networking and transport
- `cli/`: operator and wallet CLI
- `genesis/`: deterministic genesis creation and contract deployment catalog
- `custody/`: bridge custody coordinator and signer/quorum flows
- `faucet-service/`: faucet backend
- `contracts/`: WASM contracts, each with its own `Cargo.toml`
- `sdk/`: Rust, JS, and Python client SDKs
- Frontends: `wallet/`, `explorer/`, `dex/`, `marketplace/`, `developers/`, `programs/`, `monitoring/`, `faucet/`, `website/`
- Ops/docs: `deploy/`, `infra/`, `scripts/`, `docs/`, `skills/`, `.github/`

## Validation Matrix

- Rust workspace formatting: `cargo fmt --all`
- Rust workspace check: `cargo check --workspace`
- Rust workspace lint: `cargo clippy --workspace -- -D warnings`
- Rust workspace tests: `cargo test --workspace --release`
- Contract-local check/test: `cd contracts/<name> && cargo check && cargo test --release`
- Contract WASM build: `cd contracts/<name> && cargo build --target wasm32-unknown-unknown --release`
- Frontend shared/helper checks: `npm run test-frontend-assets`
- Wallet-specific checks: `npm run test-wallet`, `npm run test-wallet-extension`
- Expected contract catalog check: `python3 scripts/qa/update-expected-contracts.py --check`

Prefer scoped validation while iterating, then widen before closing the task.

## Guardrails

- The worktree is often dirty. Do not revert unrelated changes you did not make.
- Do not assume every doc is equally current. Check timestamps and source files.
- Do not read the full root `SKILL.md` by default. It is an exhaustive reference, not the right first read for every task.
- Some local/private test flows are optional. Use `scripts/run-local-private-check.sh` when a command already expects an optional path.
- Genesis and some deploy flows require `contracts/` to be visible from the current working tree or via `LICHEN_CONTRACTS_DIR`.
- Frontend shared helpers should be treated as a synchronized family. The canonical source is `monitoring/shared/`; sync through `make sync-shared` or `scripts/sync_frontend_shared_helpers.js`.
- Normal Cloudflare Pages deploys should go through `scripts/deploy-cloudflare-pages.sh`, not raw `wrangler pages deploy`.

## Session Start

1. Load the read-order files above.
2. Check `git status --short` before editing anything.
3. Identify the smallest relevant validation slice for the task.
4. Only then load subsystem-heavy docs or large source files.

## Session End

1. If local-private memories exist, update `memories/repo/current-state.md` if durable repo facts changed.
2. If local-private memories exist, add a concise dated handover under `memories/repo/session-handovers/` if the task created non-obvious context.
3. Update `DEPLOYMENT_STATUS.md` only when deployment reality changed.
4. If local-private memories exist, record any new cross-cutting traps in `memories/repo/gotchas.md`.

## Specialist Routing

- `.github/agents/workspace.agent.md`: session bootstrap and routing
- `.github/agents/core.agent.md`: Rust runtime crates
- `.github/agents/contracts.agent.md`: `contracts/`
- `.github/agents/frontend.agent.md`: web apps and shared frontend utilities
- `.github/agents/devops.agent.md`: deploy, infra, VPS, Cloudflare
- `.github/agents/docs.agent.md`: documentation sync
- `.github/agents/testing.agent.md`: test planning and execution
- `.github/agents/security.agent.md`: read-only audit work
