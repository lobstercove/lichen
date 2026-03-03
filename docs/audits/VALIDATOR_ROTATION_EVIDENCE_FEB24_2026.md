# Validator Rotation Evidence Report (Section 4)

Date: 2026-02-24  
Environment: local 3-validator testnet after full reset/restart; validator heartbeat loop active at 5s; slot windows captured under no-load, sequential write load, and parallel write load.

---

## 1) Objective

Provide required evidence for:

1. Slot-to-leader mapping distribution over sustained windows.
2. Per-validator production count + variance.
3. Mempool pull fairness across elected leaders.
4. Correlation analysis for heartbeat timing vs leader dominance.
5. Sequential vs parallel e2e impact comparison.

---

## 2) Execution Plan Performed

1. Full reset and restart with 3 validators in staggered order.
2. Baseline no-load capture window (`baseline_no_load.json`).
3. Sequential write-load capture window (`sequential_load_window.json`) while `tests/contracts-write-e2e.py` ran with funded validator signers.
4. Parallel write-load capture window (`parallel_load_window.json`) while two concurrent `tests/contracts-write-e2e.py` runs executed.
5. Validator metrics snapshot via `getValidators` (`getValidators_snapshot.json`).

Artifacts path: `tests/artifacts/validator_rotation_feb24/`.

---

## 3) Slot-to-Leader Mapping (Sustained Windows)

### A) Baseline (no load)

- Window: 30 slots (`81-110`), duration `149.29s`.
- Leader counts:
  - `8R6N...`: 12 (40.00%)
  - `CLtB...`: 11 (36.67%)
  - `78Fa...`: 7 (23.33%)
- Tx-bearing blocks: none observed in this baseline window.

### B) Sequential write load

- Window: 90 slots (`800-964`), duration `45.38s`.
- Leader counts (`counts_all`):
  - `8R6N...`: 34 (37.78%)
  - `CLtB...`: 32 (35.56%)
  - `78Fa...`: 24 (26.67%)
- Tx-bearing blocks (`counts_tx_blocks`, total 31):
  - `CLtB...`: 14 (45.16%)
  - `78Fa...`: 9 (29.03%)
  - `8R6N...`: 8 (25.81%)

### C) Parallel write load (2 concurrent workloads)

- Window: 90 slots (`1264-1414`), duration `93.00s`.
- Leader counts (`counts_all`):
  - `CLtB...`: 35 (38.89%)
  - `78Fa...`: 28 (31.11%)
  - `8R6N...`: 27 (30.00%)
- Tx-bearing blocks (`counts_tx_blocks`, total 36):
  - `CLtB...`: 18 (50.00%)
  - `78Fa...`: 10 (27.78%)
  - `8R6N...`: 8 (22.22%)

---

## 4) Per-Validator Production Count + Variance

From `getValidators` snapshot:

- `8R6N...`: `517` blocks_proposed
- `CLtB...`: `492` blocks_proposed
- `78Fa...`: `424` blocks_proposed

Aggregate stats:

- Mean proposed blocks: `477.67`
- Population variance: `1544.22`
- Population standard deviation: `39.30`
- Coefficient of variation: `8.23%`

Result: production is distributed across all 3 validators with measurable, non-trivial spread.

---

## 5) Mempool Pull Fairness vs Elected Leaders

Fairness proxy used: tx-share minus leader-share in the same window.

### Sequential window

- `CLtB...`: `+9.60pp` (45.16% tx vs 35.56% leader share)
- `78Fa...`: `+2.36pp` (29.03% tx vs 26.67% leader share)
- `8R6N...`: `-11.97pp` (25.81% tx vs 37.78% leader share)

### Parallel window

- `CLtB...`: `+11.11pp` (50.00% tx vs 38.89% leader share)
- `78Fa...`: `-3.33pp` (27.78% tx vs 31.11% leader share)
- `8R6N...`: `-7.78pp` (22.22% tx vs 30.00% leader share)

Result: tx-bearing blocks are not perfectly proportional to leader-share; `CLtB...` consistently over-indexes under load.

---

## 6) Heartbeat Correlation vs Leader Dominance

Using per-validator share correlation between tx-bearing blocks and heartbeat-only blocks:

- Sequential window correlation: `-0.401`
- Parallel window correlation: `-0.945`

Interpretation: higher heartbeat-only share does not predict higher tx-bearing share in these windows. This weakens the hypothesis that 5s heartbeat timing alone explains leader dominance under load.

---

## 7) Sequential vs Parallel Impact Comparison

- Tx-bearing blocks increased from `31` (sequential) to `36` (parallel) for same slot window length (90).
- Leader-share dispersion tightened under parallel load (stddev `0.0480` → `0.0395`), but tx capture skew toward `CLtB...` increased (`45.16%` → `50.00%`).
- Parallel workload therefore improves tx volume but does not improve tx fairness proportionality.

---

## 8) Operational Notes / Root-Cause Controls

- Early attempts failed because `contracts-write-e2e.py` required secondary funding from a signer with >= `2.001` MOLT spendable and `requestAirdrop` is disabled in 3-validator mode.
- This was fixed by running sequential/parallel windows with funded validator keypairs (`validator-8001.json`, `validator-8002.json`) and separate background terminals to avoid shell parser crashes.

---

## 9) Conclusion

Section 4 required evidence is satisfied with sustained baseline + sequential + parallel windows, per-validator variance metrics, fairness deltas, and heartbeat-correlation analysis. Current data shows leader rotation across all 3 validators, with persistent tx-capture skew under load that is not explained by heartbeat share alone.
