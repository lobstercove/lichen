## 2026-04-22 SEA Validator Reseed And Sync Throttle

- Investigated the live SEA testnet validator outage/stall against commit `c3f3435` (`Prepare v0.5.6 release sync`).
- Confirmed SEA had fallen far behind and warp sync was not recovering it.

### Live findings

- US and EU had checkpoint directories (`slot-675000`, `slot-676000`, `slot-677000`), but `latest_verified_checkpoint(...)` rejected them because `checkpoint_meta.json.state_root` did not match the committed block state root for the same slot.
- That means current warp sync is operationally broken even when checkpoints exist.
- SEA was reseeded from the US `slot-677000` checkpoint by copying the RocksDB snapshot server-to-server, then restoring SEA-specific files:
  - `validator-keypair.json`
  - `signer-keypair.json`
  - `seeds.json`
  - `genesis.json`
  - `genesis-wallet.json`
  - `known-peers.json`
  - `registration-submitted.marker`
  - `genesis-keys`
- SEA backup created at `/var/lib/lichen/state-testnet.preseed-20260422-133834`.
- After restart, SEA resumed from roughly `677005` instead of the old `533k` tip and began voting again.

### Live status at handover

- SEA local RPC is healthy on `http://127.0.0.1:8899`.
- SEA advanced from roughly `677415` to `677855` while US advanced from `679090` to `679182` during sampling, shrinking the gap from about `1675` to `1327`.
- US/EU logs show SEA votes being accepted through the `6775xx` range, so it is no longer truly offline.
- Remaining problem is sync throughput, not liveness.

### Local code changes

- `validator/src/main.rs`
  - `latest_verified_checkpoint(...)` now scans newest-to-oldest and falls back to the newest valid verified checkpoint instead of failing hard on only the latest checkpoint.
  - Added test `latest_verified_checkpoint_falls_back_to_older_valid_checkpoint`.
  - Sync completion callbacks no longer mark a batch complete after tiny partial progress; they now leave the batch in-flight until it actually reaches target.
- `validator/src/sync.rs`
  - `should_sync(...)` no longer overlaps sync requests while the current batch still has more than one P2P chunk of runway left.
  - `record_progress(...)` now auto-completes the batch when the requested end slot is actually reached.
  - Added tests:
    - `test_should_not_overlap_while_current_batch_has_runway`
    - `test_record_progress_completes_batch_at_target`

### Local validation

- `cargo fmt --all`
- `cargo test -p lichen-validator test_should_not_overlap_while_current_batch_has_runway -- --nocapture`
- `cargo test -p lichen-validator test_record_progress_completes_batch_at_target -- --nocapture`
- `cargo test -p lichen-validator latest_verified_checkpoint -- --nocapture`
- `cargo check -p lichen-validator`

### Deployment note

- I did not hot-deploy the new sync-throttle patch to SEA in this session.
- SEA is already catching up on the current binary, and remote source-tree inspection on the VPS became unreliable/hung for simple file reads, so I avoided a blind live replacement.

### Follow-up deployment completion

- Later in the same day, live rollout work found that the VPS source trees were drifted:
  - local repo and SEA `~/lichen-patched` included the Solana token-account index column families
  - US/EU `~/lichen` did not
- Rolling the partially updated US/EU binary onto SEA failed immediately with:
  - `Failed to open database: Invalid argument: Column families not opened: solana_holder_token_accounts, solana_token_accounts`
- A second attempt to use a locally built macOS artifact on SEA failed with:
  - `Exec format error`
- Final successful path:
  - built a fresh Linux artifact from a clean local-source snapshot on US
  - used `git archive HEAD` into `/home/ubuntu/lichen-build`
  - overlaid the session-local edits to:
    - `core/src/state/snapshot_io.rs`
    - `p2p/src/message.rs`
    - `validator/src/main.rs`
    - `validator/src/sync.rs`
  - built with `CARGO_TARGET_DIR=/home/ubuntu/lichen-build-target /home/ubuntu/.cargo/bin/cargo build --release -p lichen-validator`
  - canonical deployed binary hash:
    - `406ab912743554846b3977885d05e826f6192e5e3b2a6d72f5531e269094d6f0`
- Rolled that exact artifact sequentially to SEA, EU, and US with backups of `/usr/local/bin/lichen-validator` taken before each restart.
- Final live state after rollout:
  - all 3 validators serve the same installed binary hash `406ab912743554846b3977885d05e826f6192e5e3b2a6d72f5531e269094d6f0`
  - sampled slots immediately after rollout were `SEA=681391`, `EU=681392`, `US=681393`
  - US `getValidators` showed all 3 active, with SEA back inside the active window and continuing to advance
