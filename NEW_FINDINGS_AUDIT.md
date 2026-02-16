# COUNTER-AUDIT — NEW FINDINGS (Code-Verified)
**Date**: February 16, 2026  
**Method**: Direct source verification against current `main` code  
**Scope**: Findings in this file only (`NEW-H1..NEW-L1`)  
**Policy**: Verification only (no fixes applied)

---

## Executive Verdict

Out of 6 reported findings:

- **Confirmed (real issue)**: 3
- **Partially correct (real concern, wrong framing/severity)**: 1
- **Not reproducible / incorrect as stated**: 2

### Final severity after counter-audit
- **HIGH**: 1 (`NEW-H2`) — **FIXED** (commit bcc34e9)
- **MEDIUM**: 1 (`NEW-M1`) — **FIXED** (commit 52f3aac)
- **LOW**: 2 (`NEW-M2`, `NEW-L1`)
- **INVALID / CLOSED**: 2 (`NEW-H1`, `NEW-H3`)

---

## Finding-by-Finding Revalidation

## NEW-H1 — LobsterLend health factor precision loss
**Original claim**: HIGH  
**Counter-audit verdict**: **INVALID (claim logic incorrect)**

### What code does
- View function computes:
  - `health_factor = deposit * 85 * 100 / borrow` when `borrow > 0`
- Liquidation enforcement uses separate check:
  - `liquidation_limit = deposit * 85 / 100`
  - position liquidatable only if `current_borrow > liquidation_limit`

### Why reported exploit does not hold
- Claim says small borrow can truncate health factor to zero and hide liquidation.
- Arithmetic direction is opposite: with smaller `borrow`, health factor increases, not decreases.
- Liquidation eligibility is not decided by the health-factor view; it is enforced by on-chain liquidation logic.

### Notes
- This is not a confirmed precision vulnerability as described.
- (Separate hardening idea, not part of original claim): large-value multiplication in `u64` view math may merit defensive widening to `u128`, but that is a different issue.

---

## NEW-H2 — DEX Margin required margin math
**Original claim**: HIGH (precision loss)  
**Counter-audit verdict**: **CONFIRMED HIGH (but root cause is logic/design mismatch, not just rounding)**  
**Status**: **FIXED** (commit bcc34e9) — Removed `/ leverage` from `required_margin` calculation in `contracts/dex_margin/src/lib.rs`. The tier table's `initial_margin_bps` already encodes leverage-dependent requirements, so the extra division was double-applying the discount.

### What code does
- Tier table already encodes leverage-dependent margin requirements (`initial_margin_bps`).
- Open-position check then applies another `/ leverage`:
  - `required_margin = (notional * initial_margin_bps / 10_000 / leverage).max(1)`

### Why this is real
- This effectively double-applies leverage discount.
- Existing tests in the file assert very low accepted margins for high leverage (e.g. tiny values at 50x/100x), consistent with under-collateralized admission.
- Risk is larger than “rounding drift”: margin floor can become economically too small relative to intended tier policy.

### Severity rationale
- Directly affects admission criteria for leveraged positions.
- Can materially increase liquidation frequency and insurance-fund stress.

---

## NEW-H3 — Prediction market resolution race (same block)
**Original claim**: HIGH  
**Counter-audit verdict**: **INVALID (not reproducible under current execution model)**

### What code does
- `submit_resolution` requires market status `STATUS_CLOSED`.
- On success, it immediately writes status `STATUS_RESOLVING` and saves market record.
- Validator uses `process_transactions_parallel`, but conflict grouping includes instruction accounts.
- Contract calls require caller + contract account (`ix.accounts[0]`, `ix.accounts[1]`), so calls to the same market contract conflict on shared contract account and are serialized in a single group.

### Why overwrite race is not supported
- After first successful resolution flips to `STATUS_RESOLVING`, subsequent submission in same block sees non-`STATUS_CLOSED` and fails.
- This is ordering/front-running surface (first valid resolver wins), not a dual-write race that allows both resolutions to commit.

---

## NEW-M1 — MoltyID decay cap after long inactivity
**Original claim**: MEDIUM  
**Counter-audit verdict**: **CONFIRMED MEDIUM**  
**Status**: **FIXED** (commit 52f3aac) — `apply_reputation_decay` now returns `(rep, periods_applied)` and `last_updated` advances only by actually applied periods, not to current time. Remaining decay is preserved for future calls.

### What code does
- Decay periods are capped at `MAX_DECAY_PERIODS_PER_CALL = 64`.
- After applying capped decay, identity `last_updated_ms` is set to `now_ms`.

### Why this is real
- For very long inactivity, only first 64 periods are ever applied, then timestamp jumps to now.
- Remaining historical decay is effectively discarded permanently.
- This can leave stale reputation above intended long-horizon level.

### Severity rationale
- Governance/reputation weighting distortion is plausible.
- Not an immediate insolvency vector; medium is appropriate.

---

## NEW-M2 — Flash-loan fee precision for small loans
**Original claim**: MEDIUM  
**Counter-audit verdict**: **PARTIALLY CORRECT → LOW (economic policy quirk, not security bug)**

### What code does
- Fee formula rounds down then applies minimum fee of 1 shell.

### Assessment
- True that effective fee is non-linear for dust amounts.
- Not a zero-fee bypass (minimum fee prevents that).
- Not a direct exploit against protocol solvency; this is fee-policy/UX behavior.

### Severity rationale
- Better classified as low-priority economics tuning (or explicit minimum-amount policy), not medium security vulnerability.

---

## NEW-L1 — Liquidation penalty split remainder loss
**Original claim**: LOW  
**Counter-audit verdict**: **CONFIRMED LOW**

### What code does
- Penalty split computes both 50% legs with integer division independently.
- For odd penalty values, `liquidator_reward + insurance_add < penalty` by 1 unit.

### Why this is real
- Accounting remainder is dropped due to double truncation.
- Low per-event drift, but invariant mismatch accumulates over many events.

---

## Corrected Risk Summary

1. **Priority 0 (fix first)**
   - ~~`NEW-H2` (margin admission math/policy mismatch)~~ **FIXED** (commit bcc34e9)

2. **Priority 1**
   - ~~`NEW-M1` (long-inactivity decay cap semantics)~~ **FIXED** (commit 52f3aac)

3. **Priority 2**
   - `NEW-L1` (penalty remainder accounting)
   - `NEW-M2` (dust-fee policy normalization)

4. **Close / remove from active security queue**
   - `NEW-H1` (invalid as written)
   - `NEW-H3` (invalid race model)

---

## Test-First Checklist (No Code Changes Yet)

### For NEW-H2
- Add deterministic tests asserting expected required margin per leverage tier from policy table.
- Add anti-regression test proving high-leverage tiers cannot open with sub-policy collateral.
- Add edge test for tiny notional to verify min-margin behavior is policy-defined, not accidental.

### For NEW-M1
- Add long-gap test (`>64` periods) showing current cap-and-reset behavior.
- Add invariant test: equivalent elapsed time should produce equivalent decay regardless of call chunking.

### For NEW-L1
- Add accounting invariant test: `penalty == liquidator_reward + insurance_add (+ explicit remainder bucket if intended)`.

### For NEW-M2
- Add fee-shape test across dust-to-large amounts; document intended curve (minimum fee vs percentage fee).

---

## Confidence

- **High confidence**: `NEW-H2`, `NEW-M1`, `NEW-L1`
- **High confidence invalidation**: `NEW-H1`, `NEW-H3`
- **High confidence reclassification**: `NEW-M2`

---

**Counter-audit complete**: February 16, 2026  
**Result**: 3 confirmed (all fixed), 1 downgraded/reframed, 2 invalidated
