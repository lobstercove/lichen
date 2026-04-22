## 2026-04-23 Production Pass Fixes

- Fixed warp snapshot checkpoint corroboration so anchors are counted by verified validator `Pubkey`, not `SocketAddr`.
  Files: `p2p/src/peer.rs`, `validator/src/main.rs`
- Tightened checkpoint verification hot path:
  - checkpoint creation now uses cached/incremental state-root + metrics counters instead of cold-start scans
  - latest checkpoint verification now tries cached/incremental verification before falling back to cold-start rebuild
  Files: `core/src/state/snapshot_io.rs`, `validator/src/main.rs`
- Removed the contract-storage snapshot-serving full-scan path by switching warp snapshot pagination to untracked cursor exports and provisional `total_chunks` progression.
  Files: `core/src/state/snapshot_io.rs`, `validator/src/main.rs`
- Monitoring no longer exposes fake production kill-switch controls, and the LichenSwap RPC method name now matches the backend.
  Files: `monitoring/index.html`, `monitoring/js/monitoring.js`, `monitoring/css/monitoring.css`
- Unified validator activity semantics across RPC surfaces to a single 100-slot window.
  File: `rpc/src/lib.rs`
- Cleared the sync-path clippy failure in `validator/src/sync.rs`.

Validation completed:
- `cargo fmt --all --check`
- `cargo check --workspace`
- `cargo clippy --workspace -- -D warnings`
- `cargo test -p lichen-validator checkpoint_anchor -- --nocapture`
- `cargo test -p lichen-validator latest_verified_checkpoint -- --nocapture`
- `cargo test -p lichen-validator test_should_not_overlap_while_current_batch_has_runway -- --nocapture`
- `npm run test-frontend-assets`
- `python3 scripts/qa/update-expected-contracts.py --check`
