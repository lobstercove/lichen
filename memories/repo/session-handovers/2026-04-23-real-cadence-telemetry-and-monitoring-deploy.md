# Session Handover — 2026-04-23

## What Changed

- Replaced fake block-cadence health based on second-resolution header timestamps with observer-side wall-clock cadence telemetry.
- Added new metrics fields in `core`/`rpc`:
  - `observed_block_interval_ms`
  - `cadence_target_ms`
  - `head_staleness_ms`
  - `cadence_samples`
  - `last_observed_block_slot`
  - `last_observed_block_at_ms`
- `slot_pace_pct` now uses `cadence_target_ms / observed_block_interval_ms`.
- Mission Control now prefers cluster-level cadence from `getClusterInfo.cluster_nodes[].last_observed_block_slot` and falls back to single-node observer telemetry only when cluster data is missing.
- `ValidatorInfo::note_activity()` now advances `last_observed_block_slot` and `last_observed_block_at_ms` on newer observed validator activity, which is what makes cluster-level cadence actually line up across the 3-validator view.

## Local Validation

- Held the local stack open with `scripts/start-local-stack.sh testnet && ... while true; do sleep 3600; done` because the command runner reaps background children when the parent shell exits.
- Verified local RPC telemetry on `8899/8901/8903`:
  - `observed_block_interval_ms` ~ `1000`
  - `cadence_target_ms = 800`
  - `slot_pace_pct` ~ `80`
  - all 3 validators reported the same `last_observed_block_slot`
- Focused tests/checks that passed before rollout:
  - `cargo fmt --all`
  - `cargo test -p lobstercove-lichen-core observed_cadence_ignores_replay_samples_until_live_head -- --nocapture`
  - `cargo test -p lichen-validator note_validator_activity_updates_slot_and_vote_count -- --nocapture`
  - `cargo test -p lichen-rpc --test rpc_full_coverage test_native_get_metrics_with_data -- --nocapture`
  - `cargo test -p lichen-rpc --test rpc_full_coverage test_native_get_cluster_info_returns_data -- --nocapture`
  - `cargo clippy --workspace -- -D warnings`
  - `npm run test-frontend-assets`

## Live Rollout

- Built the canonical Linux validator artifact on US from the current workspace source snapshot.
- Rolled the same binary to `EU -> SEA -> US`.
- Final live validator artifact hash on all 3 VPSes:
  - `5dacad2e36a93629baf391270b2fdfa03af00b3da37104d9d4a407a7e7043e2c`
- Public RPC (`https://testnet-rpc.lichen.network`) now serves the new cadence fields.
- Monitoring redeployed with:
  - preview: `https://c63522d6.lichen-network-monitoring.pages.dev`
  - live: `https://monitoring.lichen.network`
- Both pages serve the new `BLOCK CADENCE` / `Observed Cadence` UI.

## Notes

- During a cold `cargo test --workspace --release` rerun after `cargo clean`, the previously failing bridge-deposit audit-log test no longer reproduced in focused reruns. The test was updated to print the captured buffer if it ever fails again, but there was no functional code fix needed for the RPC path itself.
