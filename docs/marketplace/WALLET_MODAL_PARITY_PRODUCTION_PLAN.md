# Wallet Modal Parity Production Plan (DEX → Marketplace + Programs)

Date: 2026-02-26
Owner: Copilot execution batch

## Objective
Bring `marketplace` and `programs` wallet modals to **exact parity** with `dex` in:
- UI structure/classes/styles
- Interaction flow and state reset behavior
- Wallet actions (create/import/connect/select/remove)
- Immediate wallet sync after create/import
- Modal close/open lifecycle consistency

This plan is implementation-first and test-enforced. No partial parity is accepted.

## Hard Requirements (Acceptance Criteria)
1. `marketplace` wallet modal markup and classes match `dex` wallet modal markup/classes exactly (except app-specific IDs only when unavoidable).
2. `programs` wallet modal markup and classes match `dex` exactly.
3. Shared wallet files used by `marketplace` and `programs` are byte-for-byte equivalent to `dex` shared wallet files for modal behavior.
4. New wallet flow behavior:
   - Inputs shown before wallet creation.
   - Create button hidden/removed after successful creation in the same way as `dex`.
   - Modal close resets transient create/import state exactly like `dex`.
5. Wallet list refresh behavior:
   - Create/import immediately updates wallet list and selected wallet.
   - Balance updates/labels sync immediately after state change.
6. Layout parity:
   - No right-offset drift or spacing mismatch in wallet pane (notably `programs`).
7. Regression tests cover these parity constraints.
8. Complete E2E suite includes wallet modal parity checks for all three surfaces (`dex`, `marketplace`, `programs`).

## Execution Plan

### Phase 1 — Source of Truth Lock
- Define `dex` wallet modal implementation as canonical source.
- Enumerate canonical files (HTML/JS/CSS/shared helpers) used by `dex` modal.
- Build a parity matrix for `marketplace` and `programs` against canonical files.

### Phase 2 — Marketplace Parity
- Replace/port marketplace wallet modal HTML structure to canonical `dex` structure.
- Replace/port marketplace modal JS handlers with canonical behavior for:
  - create/import flow
  - modal reset on close
  - wallet list sync and selection
  - create button visibility lifecycle
- Replace/port required CSS classes/tokens from `dex` (no custom divergence).

### Phase 3 — Programs Parity
- Apply same canonical `dex` structure/logic/styles to `programs` wallet modal.
- Fix panel alignment/spacing drift (right offset issue) by using canonical class structure and container hierarchy.
- Ensure close/reset behavior matches `dex` exactly.

### Phase 4 — Shared File Canonicalization
- Canonicalize shared wallet modal files so all three app surfaces import the same implementation path where possible.
- Remove local forks/partial copies causing behavioral drift.

### Phase 5 — Tests
- Add parity regression tests:
  - Structural parity (required IDs/classes/actions exist and match canonical set).
  - Behavioral parity (create button lifecycle, modal reset, immediate wallet list sync).
- Extend complete E2E matrix to include wallet modal scenarios in:
  - `dex`
  - `marketplace`
  - `programs`
- Ensure failures block pass status.

### Phase 6 — Validation Gate
- Run targeted tests first (wallet parity), then full complete E2E path.
- Fix only parity-related failures introduced by this change set.

## Test Scenarios (Must Pass)
1. Open modal on each surface; verify same fields/buttons/classes and ordering.
2. Create wallet:
   - Inputs visible pre-create.
   - Create action succeeds.
   - Create button state transitions exactly as in `dex`.
   - Wallet list updates instantly.
3. Import wallet:
   - Import action succeeds.
   - Wallet list updates instantly and selection syncs.
4. Close modal mid-flow and reopen:
   - Transient state resets exactly as in `dex`.
5. Programs layout check:
   - Wallet panel alignment equals `dex`.
6. Shared-file integrity check:
   - Canonical shared files are identical where required.

## Done Definition
- Code parity achieved for wallet modal across `dex`, `marketplace`, `programs`.
- Regression + E2E tests added and passing.
- No known parity drift remains in UI or behavior.
