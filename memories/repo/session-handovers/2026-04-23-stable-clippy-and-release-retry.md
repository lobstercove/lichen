## 2026-04-23 Stable Clippy And Release Retry

- Root issue for the release retry after `69606b7`: GitHub Actions had already moved `Clippy` to Rust `1.95.0`, while the local machine still had stable `1.94.0`, so local `cargo clippy --workspace -- -D warnings` missed a set of new style/checked-op lints.
- Updated local stable with `rustup update stable`, then validated against the same toolchain GitHub used:
  - `cargo +stable clippy --workspace -- -D warnings`
  - `cargo +stable test --workspace`
- Fixed the full 1.95 lint batch across:
  - `core/src/block.rs`
  - `p2p/src/kademlia.rs`
  - `custody/src/chain_config/discovery.rs`
  - `custody/src/deposit_api_support.rs`
  - `rpc/src/dex.rs`
  - `rpc/src/launchpad.rs`
  - `rpc/src/lib.rs`
  - `rpc/src/prediction.rs`
  - `validator/src/main.rs`
- Also previously fixed in this same release retry:
  - `getValidators.last_active_slot` inconsistency during bootstrap by recording signature-verified remote consensus activity at P2P ingress and feeding it into the live validator set
  - full-lockfile `Cargo Audit` issues by refreshing `rand` versions across tooling and contract lockfiles
- Local validation state before push:
  - `cargo fmt --all`
  - `cargo +stable clippy --workspace -- -D warnings`
  - `cargo +stable test --workspace`
  all passed.
