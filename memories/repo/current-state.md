# Current State

Last reviewed: 2026-04-27

## Durable Facts

- Repo root README and release docs now target `v0.5.20` as the active signed release line.
- `v0.5.20` is the live verified 3-VPS testnet baseline as of 2026-04-27. GitHub Release workflow `24995138415` completed successfully for commit `7435b141ab6db549448239f76bcae54186cc0546`; the public release is `https://github.com/lobstercove/lichen/releases/tag/v0.5.20`.
- The signed `v0.5.20` release uses signer address `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk`. `SHA256SUMS` hash is `91f916de1cf954700b626695f63d1e741ff4294aede39659eb88964a30629008`, `SHA256SUMS.sig` hash is `0b0245cd364a44aa04c330804687ab9b73389189c1481a34970d665e36ee7985`, and the Linux x86_64 archive hash is `709ad6e699c68f553a63965cf90722ea2984d3476d774afa739a2d71978c933c`.
- The installed `v0.5.20` VPS validator hash is identical across US/EU/SEA: `13d33bac70b9a2301c2ed16c668b1c9c9edaf9291125c128c28f951a192ba185` at `/usr/local/bin/lichen-validator`.
- The 2026-04-27 `v0.5.20` clean-slate testnet reset generated genesis hash `9c60dc09c61f1d2819eca6a99c7ca144816affa6dc80dc6186959c80a40390ba` and state root `eef2f1d247b505b59bc63a8cac051965cf59d8c98331d04070791efefcb241d5`; all 28 genesis contracts deployed, contract identities were awarded 28/28, and the signed metadata manifest exposes 28 `symbol_registry` entries.
- Fresh `v0.5.20` testnet validator identities are US `6xSWvNvapMugudhcs55FQNUyxhyzHEokMEjA9w5SqSF`, EU `7F2kWaKF9k3oEQK5t2aQeA1Tg6GuyXiwk3iBaQWjrR6`, and SEA `71XQXPuq74DTffzhQEH4awnqToG3B9mbqWe8jzEXQdT`. Public RPC verification after the reset reached slot 144 with exactly 3 active validators and peer count 2 on the public seed observer.
- Bridge and oracle are production-bootstrapped on the fresh `v0.5.20` testnet reset: `getLichenBridgeStats` reports `validator_count=3`, `required_confirms=2`, `quorum_ready=true`, `operational=true`; `getLichenOracleStats` reports 4 contract feeds, 4 consensus feeds, 12 native attestations, and `operational=true`.
- `v0.5.20` removes snapshot-provisioned validator joining from the runbooks/scripts. Fresh validators bootstrap by fetching the authoritative `genesis.json` from the seed RPC endpoint listed in `seeds.json`, importing and verifying the canonical opcode-41 block-0 state bundle, then replaying later blocks from peers with their own validator keypair. Clean-slate deploy and local-stack helpers should not copy RocksDB state, `genesis-wallet.json`, `genesis-keys/`, peer cache, or consensus WAL to joiners. Runtime startup also no longer performs analytics/margin/oracle state reconciliation on resume; catch-up after slot 0 must be pure block replay.
- `v0.5.19` was published, then marked prerelease/superseded by `v0.5.20` after a clean-slate deployment exposed an overly strict bootstrap-RPC policy that rejected official seed HTTP RPC endpoints while the public Cloudflare RPC was unavailable during reset.
- P2P self-endpoint filtering is now external-address aware. `LICHEN_EXTERNAL_ADDR` is advertised in gossip/outgoing messages and used by `PeerManager` reconnect/discovery preflight, so VPS nodes bound to `0.0.0.0:7001` reject their own public `IP:7001` without treating every remote `:7001` peer as self.
- Post-`v0.5.20` VPS verification found healthy independent joiner bootstrap logs, matching installed validator hashes, and no state-root mismatch, panic, `STATE INTEGRITY`, self-identity, or self-connect lines in the checked `lichen-validator-testnet` journals since the new service starts.
- Faucet Recent Requests are not chain state: they come from `/var/lib/lichen/airdrops.json`. The clean-slate redeploy deletes and verifies that file on testnet; after the `v0.5.20` reset, `https://faucet.lichen.network/faucet/airdrops?limit=5` returned `[]`.
- `v0.5.17` was published and installed but superseded before becoming the stable baseline. The live restart/resync exposed deterministic divergence when multiple native oracle attestation transactions ran through the parallel scheduler without a global oracle conflict key; `v0.5.18` serializes opcode 30 oracle attestation work and includes a regression test for parallel oracle attestations.
- Mainnet genesis market prices now fail closed instead of silently using compiled defaults. `lichen-genesis` resolves prices from `--genesis-prices-file`, then complete `GENESIS_SOL_USD`/`GENESIS_ETH_USD`/`GENESIS_BNB_USD` environment overrides, then live Binance/CoinGecko fetches; compiled defaults remain a testnet/dev fallback only. The clean-slate redeploy path writes and passes an audited `genesis-prices.json` snapshot when Binance returns a complete ticker response.
- `v0.5.10` was published and clean-slate deployed on 2026-04-26, but the testnet reproduced the validator-offline failure at slot 285: SEA hit a state-root mismatch after a stale stake-pool write. Treat `v0.5.10` as superseded by `v0.5.18`.
- `v0.5.11` fixed the stake-pool persistence race and passed the previous slot-285 failure point on testnet, but the clean-slate script still performed an unnecessary post-manifest validator restart that triggered startup integrity warnings during rollout. `v0.5.12` removed that restart.
- `v0.5.13` removes the flawed post-effects startup root marker. The marker was recorded before later deterministic post-block hooks finished, so clean snapshot restarts could log false `STATE INTEGRITY` warnings even when block import/commit root checks were healthy.
- Validator stake-pool persistence is consensus-owned only. The former background "persist in-memory stake pool every 30s" task was removed because it could clone a stale pool, then overwrite RocksDB after a block committed, causing the next block's state root to diverge on that node.
- Startup no longer compares live RocksDB roots to block header roots or to mid-post-hook markers. Header roots are pre-effects commitments; authoritative state-root enforcement happens in block import and BFT commit paths at that boundary. Block-production stake-pool effects are idempotent if the slot update was persisted before the reward completion marker.
- Validator RPC activity reporting now prefers the live in-memory validator set, and remote BFT `last_active_slot` updates are fed from signature-verified consensus ingress instead of delayed BFT queue drain.
- Validator sync pending storage now keeps multiple block candidates per slot and chooses the candidate that chains from the current tip, preventing a wrong-parent candidate from permanently poisoning catch-up.
- Validator identity admission is stake-backed only: block headers and validator announcements no longer create `ValidatorSet` entries, P2P validator-route status is granted only to existing or locally stake-backed validators, and startup drops persisted unbacked validator metadata.
- The failed `v0.5.10` run, the restart-noisy `v0.5.11` run, the marker-noisy `v0.5.12` run, and the bridge/oracle/P2P/oracle-scheduler hardening releases through `v0.5.19` should be treated as superseded by the verified `v0.5.20` clean-slate baseline.
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
- The 2026-04-23 production-pass handover records the `v0.5.7` hardening release contents; release docs now target `v0.5.20`, which is the latest fully recorded deployed baseline.
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
