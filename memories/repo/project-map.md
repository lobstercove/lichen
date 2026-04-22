# Lichen Project Map

## Purpose

Lichen is a custom Layer 1 blockchain for agent-native applications.
The repo combines protocol runtime code, contracts, frontends, SDKs, deployment automation, and documentation.

## Core Implementation Areas

- `core/`: state machine, accounts, transactions, consensus primitives, contract runtime, shielded runtime
- `validator/`: full validator binary, runtime orchestration, updater, service startup
- `rpc/`: JSON-RPC, REST, WebSocket, shielded proof generation helpers
- `p2p/`: peer identity, transport, gossip, block propagation
- `cli/`: `lichen` CLI for wallets, queries, contract operations, and operator actions
- `genesis/`: deterministic genesis creation plus initial contract deployment and initialization
- `custody/`: bridge custody coordinator and signer/quorum operations
- `faucet-service/`: testnet faucet backend

## Contracts and SDKs

- `contracts/`: 29 contract directories
- `genesis/src/lib.rs`: genesis deployment catalog for 28 contracts
- `sdk/rust/`, `sdk/js/`, `sdk/python/`: language SDKs
- `compiler/`: Rust-to-WASM contract build tooling, not part of the Cargo workspace

## Frontend Surfaces

- `wallet/`: wallet app and extension
- `explorer/`: block explorer
- `dex/`: SporeSwap frontend and DEX tooling
- `marketplace/`: NFT marketplace
- `developers/`: developer portal and API docs
- `programs/`: programs IDE
- `monitoring/`: operator dashboards and canonical shared frontend helpers
- `faucet/`: faucet UI
- `website/`: landing page

## Operational and Reference Paths

- `deploy/`: systemd units and setup helpers
- `infra/`: Docker Compose and observability configs
- `scripts/`: operational helpers, QA scripts, deployment wrappers
- `docs/`: architecture, audits, guides, strategy, deployment docs
- `.github/`: workspace instructions, prompts, and specialist agent definitions
- `skills/`: repo-local execution skills

## Commands That Matter

- Workspace build/check:
  - `cargo check --workspace`
  - `cargo clippy --workspace -- -D warnings`
  - `cargo test --workspace --release`
- Contract-local:
  - `cd contracts/<name> && cargo check`
  - `cd contracts/<name> && cargo test --release`
  - `cd contracts/<name> && cargo build --target wasm32-unknown-unknown --release`
- Frontend/static checks:
  - `npm run test-frontend-assets`
  - `npm run test-wallet`
  - `npm run test-wallet-extension`
- Shared helper sync:
  - `make sync-shared`
  - `node scripts/sync_frontend_shared_helpers.js`

## First Docs To Open By Area

- General workspace bootstrap: `AGENTS.md`
- Current status: `memories/repo/current-state.md`
- Deployment: `DEPLOYMENT_STATUS.md`, `docs/deployment/PRODUCTION_DEPLOYMENT.md`
- Phase 2 priorities: `docs/strategy/PHASE2_AGENT_ECONOMY.md`, `docs/strategy/PHASE2_ACTIVATION_PLAN.md`
- Public/project overview: `README.md`
- Exhaustive protocol reference: `SKILL.md` (relevant sections only)
