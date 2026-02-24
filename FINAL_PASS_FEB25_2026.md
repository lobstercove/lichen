# FINAL PASS — February 25, 2026

## Rules
- Be consistent at all times
- Always explore, never guess
- Tasks in order
- No skipping
- Assess each task done
- Always confirm with existing code
- No regression
- No stubs, no placeholders, no TODOs, no mock data, no hardcoded
- Always wire everything from core code or contracts
- Add RPC endpoints or WS sub where needed
- Always optimize RocksDB
- Always full fix, no bandaid, no simplifying, best method only
- Track progress and add notes and always check when complete
- All fixes get a test
- Always respect the theme/style/elements/classes/layout
- Commit and push after each task

---

## Task 1: DEX Price Calculation Fix (MOLT/wSOL, MOLT/wETH, MOLT/wBNB)

**Problem:** The price for MOLT/wSOL, MOLT/wETH, MOLT/wBNB pairs shows the inverse — it displays the external USD price directly (e.g., wETH = $1855) instead of MOLT/ETH_price (i.e., how many MOLT per 1 wETH).

**Root Cause:** In `dex.js` `applyBinanceRealTimeOverlay()` (line ~2345), the code does `extPrice = extPrice / moltUsd` for MOLT-quoted pairs. But pairs like `MOLT/wETH` are displayed as MOLT base, wETH quote. The price should be `MOLT_price_in_USD / ETH_price_in_USD` (a small number like 0.000054), NOT `ETH_USD / MOLT_USD` (a large number like 18,550).

**Analysis:** The pair display convention is MOLT/wETH which means "price of MOLT in wETH terms" = how many wETH per 1 MOLT. Currently the code sets `extPrice` to be the `ETH_USD / MOLT_USD` which is the price of wETH in MOLT terms (inverted). The `normalizePairDisplay()` function (line ~830) flips wrapped asset pairs so that `wSOL/MOLT` becomes `MOLT/wSOL`. This means the price needs to be the inverse: MOLT_USD / wETH_USD.

**Fix:** In `applyBinanceRealTimeOverlay()`, when the pair display has been flipped (MOLT is base, wrapped is quote), set `extPrice = moltUsd / extPrice` instead of `extPrice / moltUsd`.

**Status:** [ ] Not started

---

## Task 2: DEX Launch Tab — Center "No tokens launched yet"

**Problem:** The "No tokens launched yet" empty state in the Launched Tokens panel is not centered like it is in the Predict tab.

**Fix:** Apply the same centering pattern used in the Predict market grid empty state — use flexbox with centered content, matching dimensions and style.

**Status:** [ ] Not started

---

## Task 3: DEX UI/UX Consistency

**Problem:** Elements, alignments, spacing should be consistent across all DEX sections (except Trade).

**Check areas:** Predict, Pool, Rewards, Governance, Launchpad — ensure header bars, stat cards, panel cards, empty states, buttons, fonts, and spacing are identical in style.

**No hardcoded text** — all values must come from real data (contracts/database).

**Status:** [ ] Not started

---

## Task 4: Explorer — Move Shielded Transactions After Shielded Balance

**Problem:** In explorer `index.html`, the metrics order is: Shielded Balance → Commitments → Spent Nullifiers → Shielded Transactions → Merkle Root → Chain Status. The user wants Shielded Transactions moved to be right after Shielded Balance.

**New order:** Shielded Balance → Shielded Transactions → Commitments → Spent Nullifiers → Merkle Root → Chain Status.

**Status:** [ ] Not started

---

## Task 5: Explorer — Contract Page Token Cards & Profile/Metadata

**Sub-tasks:**
1. Enlarge token detail cards so numbers (especially TOTAL SUPPLY) fit on one line. Current `minmax(160px, 1fr)` → increase to `minmax(200px, 1fr)` or more.
2. Add token profile section (logo, description, links) — displayed beneath the title after the address, not in Contract Metadata grid.
3. Align metadata format with genesis boot format (how MOLT is created). Programs playground must send metadata in the exact format the system expects.
4. Ensure explorer reads metadata correctly and programs playground sends it properly.

**Genesis boot format (from core/src/genesis.rs):** The MOLT token at genesis stores: `token_name`, `token_symbol`, `token_decimals` in contract storage via storage_set. Extra metadata (logo, website, socials) are stored at registry/deployment level via `init_data` JSON payload.

**Programs playground metadata fields:** `name`, `symbol`, `decimals`, `supply`, `website`, `logo_url`, `twitter`, `telegram`, `discord`, `mintable`, `burnable`  — sent in `metadata` object within `DeployRegistryData`.

**Status:** [ ] Not started

---

## Task 6: Explorer — No Hardcoded Values Audit

**Problem:** Ensure nothing is hardcoded in explorer — values, achievements, ABI, etc.

**Achievements:** Defined in MoltyID contract (`award_contribution_achievement` function, `achievement_id` u8). Achievements are predefined on-chain with IDs. Not currently wired in explorer.

**Vouches:** MoltyID contract has `vouch` function. Cost: 5 reputation, Reward: 10 reputation, Cooldown: 1 hour. Max 64 vouches per identity. Can be done through wallet/extension or any frontend that calls the MoltyID contract. Not currently surfaced in explorer UI except indirectly through trust tier.

**Report:** Will document findings about achievements and vouches in the answers section.

**Status:** [ ] Not started

---

## Task 7: Explorer — Shielded Transaction Types (Shield/Unshield/Transfer)

**Problem:** The 5 shielded transactions should show their specific types (Shield, Unshield, Shielded Transfer) with proper pill colors in:
- `transactions.html` — type filter dropdown missing Shield/Unshield/ShieldedTransfer options
- `transaction.html` — already has proper handling in JS (decodeShieldedInstruction)
- `block.html` — needs to show shielded types
- `privacy.html` — already has shielded pool context
- Pill colors exist in CSS (`.pill-shield`, `.pill-unshield`, `.pill-shieldedtransfer` → purple theme)

**Fixes needed:**
1. Add Shield/Unshield/ShieldedTransfer to `transactions.html` type filter dropdown
2. Verify `block.html` renders shielded types properly with pills
3. Full audit of all explorer pages for proper shielded transaction rendering

**Status:** [ ] Not started

---

## Task 8: DEX — Wallet Modal Restore

**Problem:** The wallet modal was changed from a full import/create experience (private key input, mnemonic grid, in-browser wallet generation) to extension-only connection. The user wants the old modal back (Import tab with private key & mnemonic, Create tab with key generation) AND the new extension tab.

**Fix:**
1. Restore Import tab with: Private Key + Mnemonic sub-toggle, password field, Connect Wallet button
2. Restore Create tab with: Generate New Wallet button, address/key display, copy buttons, warning
3. Keep Extension tab (add as a third connection method)
4. Restore wallet.js functions: `fromSecretKey()`, `generate()`, local signing
5. Ensure MoltWallet shared class still works for non-extension connections
6. Ensure no "Wallet creation failed" errors when extension not installed

**Tabs:** Wallets | Import | Extension | Create New

**Status:** [ ] Not started

---

## Task 9: DEX — Prediction Market Tests

**Problem:** Need real end-to-end prediction market tests covering: creation with reputation check, initial liquidity, share trading, resolution, dispute, claim.

**Contract Details (from contracts/prediction_market/src/lib.rs):**
- min collateral: 1 mUSD, max: 100K mUSD
- Trading fee: 2% (200 BPS)
- Market creation fee: 10 mUSD
- Fee split: 50% LPs, 30% protocol, 20% stakers
- Resolution: 3 oracle attestations
- Dispute bond: 100 mUSD
- Categories: Politics, Sports, Crypto, Science, Entertainment, Economics, Tech, Custom
- Statuses: Pending, Active, Closed, Resolving, Resolved, Disputed, Voided
- CPMM pricing model for binary & multi-outcome

**Status:** [ ] Not started

---

## Task 10: DEX — Pool/Liquidity Tests

**Problem:** Need to test pool operations: add liquidity, check debits/credits, maintenance fees, LP token accounting, withdrawal.

**Status:** [ ] Not started

---

## Task 11: Comprehensive E2E Tests

**Problem:** All tests must follow `FINAL_PASS_MASTER_TODO_FEB24_2026.md` format. Tests should simulate real agent/human flow from first steps.

**Status:** [ ] Not started

---

## Task 12: Commit and Push

**Commit after each task, push when all done.**

**Status:** [ ] Not started

---

## Answers Section (filled as tasks complete)

### Achievements
- Defined in MoltyID contract via `award_contribution_achievement(user_ptr, achievement_id: u8)`
- Achievement IDs are predefined u8 values in the contract
- They are awarded by calling the MoltyID contract function
- Not currently displayed or queried in explorer UI
- Could be exposed via `get_achievements` contract call

### Vouches
- MoltyID contract `vouch(voucher_ptr, vouchee_ptr)` function
- Cost: 5 reputation from voucher
- Reward: 10 reputation to vouchee
- Cooldown: 1 hour between vouches
- Max: 64 vouches per identity
- Retrieved via `get_vouches` contract call
- Can be done through any frontend that calls MoltyID contract (wallet app, extension, programs playground)
- Currently NOT surfaced in wallet/extension UI but the contract function exists
- Vouch storage: `"vouch:{hex(pubkey)}:{index}"` key in contract storage

### Prediction Market — Bid/Ask/Pricing
- Uses CPMM (Constant Product Market Maker) formula
- Binary: YES/NO shares, each starts at 0.50 mUSD with equal reserves
- Multi-outcome: up to 8 outcomes, each starts at `1/N` price
- Price = `outcome_reserve / total_reserves` (probability)
- When buying: `shares = amount + (selfReserve * bSold) / (otherReserve + bSold)`, 2% fee
- Users CAN bet on different outcomes (buy shares in multiple)
- Market creation: 500+ MoltyID reputation required
- Minimum initial liquidity: 1 mUSD (but practical minimum is higher for meaningful markets)
- Price is defined by CPMM reserves — initial price = `1/num_outcomes`
- Liquidity is managed by CPMM pools within the prediction market contract, separate from DEX pools
- YES fully wired to contract via `buyShares`, `redeemShares`, `resolveMarket`, etc.
- Tests exist in the contract BUT no E2E test from frontend perspective

### Shield / Unshield / Shielded Transfer
- Opcodes: 23 (Shield), 24 (Unshield), 25 (ShieldedTransfer)
- `resolveTxType()` in `explorer/js/utils.js` correctly maps these opcodes
- CSS pills exist: `.pill-shield`, `.pill-unshield`, `.pill-shieldedtransfer` — purple theme
- `transaction.js` has `decodeShieldedInstruction()` for full detail display
- `transactions.html` type filter dropdown is MISSING these types — needs fix
- `block.html` relies on JS rendering which should use `resolveTxType()` — need to verify
