## 2026-04-23 Validator Activity Ingress And Release Gates

- Fixed `getValidators.last_active_slot` drift during local bootstrap:
  - RPC now prefers the live in-memory validator set when available.
  - Remote proposal/prevote/precommit activity is recorded from signature-verified P2P ingress via a dedicated `ConsensusActivityMsg` channel.
  - The old BFT-loop-side activity bump for inbound consensus traffic was removed so queue timing no longer skews RPC observability.
- Verified on the fresh local testnet stack:
  - `8899`, `8901`, and `8903` all advanced together and reported identical `last_active_slot` values for all validators.
  - Example stable sample after the fix: slot `427` on all three RPCs with all validator `last_active_slot` values also at `427`.
- Hardened the RPC log-capture test harness by serializing `capture_logs_async(...)`.
  - This removed a parallel-test tracing race that made `cargo test --workspace` flaky even though the privileged audit log path itself was correct.
- Validation completed in this session:
  - `cargo fmt --all --check`
  - `cargo audit -q -D warnings --file Cargo.lock`
  - `cargo deny check --config deny.toml advisories licenses sources`
  - `cargo clippy --workspace -- -D warnings`
  - `cargo test --workspace`
  - `npm run test-frontend-assets`
  - `python3 scripts/qa/update-expected-contracts.py --check`
  - `scripts/status-local-stack.sh testnet`
