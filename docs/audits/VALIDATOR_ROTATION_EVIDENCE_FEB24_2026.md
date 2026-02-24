# Validator Rotation Evidence Report

Date: 2026-02-24  
Environment: local testnet, 3 validators, post-reset (`./reset-blockchain.sh --restart`), production-timing config under active e2e traffic.

## Objective

Assess whether leader election/rotation is spreading transaction production across validators under active workload, and whether observed skew could be caused by heartbeat timing vs election policy.

## Procedure Executed

1. Full state reset + 3-validator restart.
2. Strict production gate run with all major suites enabled.
3. Additional live multi-validator e2e workload (`tests/live-e2e-test.sh`).
4. Block-window analysis from RPC (`getLatestBlock`, `getBlock`) for validator distribution.
5. Validator metrics snapshot from RPC (`getValidators`).

## Collected Evidence

### A) Recent block distribution window

Window sampled: slots `45-194` (150 blocks)

- `sYugJhXHQm3DVco1VZq1eWViMtNvok3UMwWhradaNUu`: 140 blocks (93.33%)
- `5TEERvSSzSfFQKy1Us1gwDKsCesBUGaUn8VZkoWTXQan`: 10 blocks (6.67%)
- `F88MA6pbe5a2thrxsaTUwyoPwSxX3puNhLbAG8DpPiTY`: 0 blocks in this sampled window

### B) Validator metrics snapshot (`getValidators`)

- Validator A (`sYug...`):
  - `_normalized_reputation`: `0.7042`
  - `_blocks_produced`: `173`
- Validator B (`5TEE...`):
  - `_normalized_reputation`: `0.1831`
  - `_blocks_produced`: `15`
- Validator C (`F88M...`):
  - `_normalized_reputation`: `0.1127`
  - `_blocks_produced`: `6`

All 3 validators are present and recently active (`last_active_slot` near tip), but production remains highly concentrated.

## Interpretation

- The observed skew is real and reproducible under active e2e load.
- Data strongly suggests election weighting (stake × reputation) dominates leader assignment in this setup.
- Heartbeat timing (`5s`) may contribute to operational cadence, but current evidence points primarily to leader-selection weighting rather than simple mempool pull ordering.
- This behavior is not round-robin; tx inclusion fairness should be evaluated against weighted-election expectations, not equal-share assumptions.

## Impact

- Throughput still functions and all validators stay online.
- Fairness/rotation perception issue remains for test environments expecting near-even leader spread.
- Could produce “same validator gets txs” symptoms during both sequential and parallel e2e runs.

## Recommended Follow-ups

1. Add explicit election fairness test:
   - Compare observed shares vs expected shares from normalized weight.
2. Add optional testnet mode for capped leader streak / smoother rotation (diagnostic mode only).
3. Add RPC endpoint or debug trace for per-slot elected leader + weight inputs.
4. Add CI artifact for slot-leader histogram per run.

## Minimal Reproduction Commands

- Reset/restart:
  - `./reset-blockchain.sh --restart`
- Workload:
  - `MOLTYID_G_PHASE_WRITE_TESTS=1 bash tests/live-e2e-test.sh`
- Distribution script (Python RPC scan over last N slots)
- Validator metrics:
  - `curl ... method=getValidators`

## Current Conclusion

Rotation is operational but **highly skewed** in this environment. The primary hypothesis is weighted election bias (reputation concentration), not solely heartbeat overlap.
