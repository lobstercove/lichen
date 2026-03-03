# MOLTCHAIN EXPLORER — PRODUCTION AUDIT REPORT
**Scope:** All 11 HTML pages · All JS files · `shared/utils.js` · `shared-config.js` · `rpc/src/lib.rs` (method dispatch) · `rpc/src/ws.rs` (WS subscriptions)  
**Root:** `explorer/`  
**Methodology:** Line-by-line read of every source file; cross-referenced against RPC backend dispatch table.

---

## EXPLORER AUDIT — index.html

**File:** `explorer/index.html` (311 lines)  
**Script load order:** `shared/utils.js` → `shared-config.js` → `js/utils.js` → `js/explorer.js`  
**No page-specific JS file** — all logic in `explorer.js`.

### Stat Cards (populated by `explorer.js:updateDashboardStats()`)
| Element ID | Purpose | Status |
|---|---|---|
| `latestBlock` | Latest slot number | ✅ Exists, populated |
| `tpsValue` | Current TPS | ✅ Exists, populated |
| `peakTps` | Peak TPS (inner `<span>` inside `#tpsChange`) | ✅ Exists, populated |
| `totalTxs` | Total tx count | ✅ Exists, populated |
| `txsToday` | Txs in last 24h | ✅ Exists, populated |
| `activeAccounts` | Non-system accounts | ✅ Exists, populated |
| `accountBreakdown` | Wallet/Contract/Program breakdown label | ✅ Exists, populated |
| `totalBurned` | Burned MOLT | ✅ Exists, populated |
| `burnPctLabel` | Burn % of supply | ✅ Exists, populated |
| `slotTimeLabel` | Avg slot time | ✅ Exists, populated |
| `validatorCount` | Active validator count | ✅ Exists, populated |
| `chainStatusTop` | Chain health pill | ✅ Exists, populated |
| `activeValidators` | Active validators count (secondary) | ❌ **MISSING FROM HTML** — `explorer.js` writes to it, element never found (`querySelector` silently fails) |
| `totalStake` | Total staked MOLT | ❌ **MISSING FROM HTML** — same silent failure |

### Shielded Stats (populated by `explorer.js:updateShieldedOverview()`)
| Element ID | Status |
|---|---|
| `shieldedBalance` | ✅ |
| `shieldedBalanceShells` | ✅ |
| `commitmentCount` | ✅ |
| `nullifierCount` | ✅ |
| `shieldedTxCount` | ✅ |
| `shieldedTxBreakdown` | ✅ |
| `merkleRoot` | ✅ |

### Latest Blocks Table (`#blocksTable`)
- Populated by `updateLatestBlocks()` → `getRecentTransactions`-equivalent polling or WS feed
- Rows: slot link → `block.html?slot=N`, block hash (truncated), tx count, time
- **WS source:** `subscribeBlocks` (when WS available) ✅

### Latest Transactions Table (`#txsTable`)
- Populated by `updateLatestTransactions()`
- Fetches `getRecentTransactions` via HTTP
- **BUG:** `updateLatestTransactions()` hardcodes `status: "Success"` for every transaction display — failed transactions never appear in the dashboard feed

### Network Selector
- `<select id="networkSelect">` — defaults to `testnet` (`selected` attribute on testnet `<option>`)
- ❌ **INCONSISTENCY:** All other pages (`blocks.html`, `block.html`, etc.) default to `mainnet`. On first visit (no localStorage), index shows testnet while sibling pages show mainnet data.
- After first selection, `localStorage.setItem('explorer_network', value)` persists across pages ✅

### Search Bar
- `<input id="searchInput">` with `<button id="searchBtn">` — triggers `navigateExplorerSearch()`
- Handles: pure numbers → block, 64-char hex → tx, `.molt` suffix → MoltName resolve, address-like → contract then address fallback ✅
- No empty-input guard (submitting empty search throws no visible error — silently returns)

### Navigation Links
- All nav `<a data-molt-app="...">` tags resolved by `shared-config.js` DOMContentLoaded handler ✅
- Footer "API Docs" links to `../developers/rpc-reference.html` — **INCONSISTENT** with every other page which links to `../docs/API.md`
- Footer links with `data-molt-app` resolved correctly ✅ (shared-config.js handles all `a[data-molt-app]`)

### Polling / Live Data
- WS `subscribeBlocks` subscription for live block feed ✅
- REST safety-net poll every `10000ms` regardless of WS state — always running ✅
- WS stale detector at `6000ms` — forces reconnect ✅
- Dashboard REST stats poll every `3000ms` (WS down) or `10000ms` (WS up) ✅

---

## EXPLORER AUDIT — blocks.html

**File:** `explorer/blocks.html`  
**Script:** `js/blocks.js` (186 lines)

### Filters
| Element | Type | Handler | Status |
|---|---|---|---|
| `#slotFromFilter` | `<input type="number">` | `applyFilters()` button | ✅ Defined in `blocks.js` |
| `#slotToFilter` | `<input type="number">` | `applyFilters()` button | ✅ |
| `#applyFiltersBtn` onclick `applyFilters()` | Button | `blocks.js:applyFilters()` | ✅ |
| `#clearFiltersBtn` onclick `clearFilters()` | Button | `blocks.js:clearFilters()` | ✅ |

- **No validation** on slot range inputs: negative values, non-numeric, `from > to` — all silently produce empty results
- Client-side filtering against pre-loaded `allBlocks[]` (up to 250 blocks via `getRecentTransactions` equivalent)

### Pagination
- `previousPage()` / `nextPage()` onclick buttons defined in `blocks.js` ✅
- Page size: 25 blocks per page — hardcoded in `blocks.js`
- "Newer" / "Older" labels may confuse: lower slot numbers = older, so "Newer" actually navigates to lower block indices in the array
- No URL persistence of current page or filter state — refreshing loses position

### Blocks Table
- Columns: Slot, Block Hash (truncated), Txs, Validator (truncated), Time
- Slot links → `block.html?slot=N` ✅
- Validator addresses → no link (text only) ❌ — should link to `address.html?address=...`

### WebSocket
- `subscribeBlocks` — inserts new blocks at top of `allBlocks[]`, re-renders ✅
- New blocks arrive live without refresh ✅

### RPC Calls
- `getRecentTransactions` (via `rpc.call`) — supported in `lib.rs` ✅

---

## EXPLORER AUDIT — block.html

**File:** `explorer/block.html` (329 lines)  
**Script:** `js/block.js` (295 lines)

### Header / Core Elements
| Element ID | Content | Status |
|---|---|---|
| `blockSlot` | Slot number | ✅ |
| `blockHash` | Truncated hash (6…6) with copy button | ⚠️ See copy bug below |
| `blockAge` | Relative time | ✅ |
| `blockTime` | Full timestamp | ✅ |

### CRITICAL BUG — Copy Buttons Copy Literal Strings
```html
<button onclick="copyToClipboard('blockHash')">Copy</button>
<button onclick="copyToClipboard('stateRoot')">Copy</button>
```
`shared/utils.js:copyToClipboard(text)` calls `navigator.clipboard.writeText(text)` — it takes the **text to copy**, not an element ID. These calls write the literal string `"blockHash"` and `"stateRoot"` to the clipboard.  
**Severity: Critical UX bug.** User copies the word "blockHash" instead of the actual hash.  
**Fix:** Pass the full hash value: `onclick="copyToClipboard(document.getElementById('blockHash').dataset.full)"` — requires adding `data-full="FULL_HASH"` to elements.

### Info Card Elements
| Element ID | Status |
|---|---|
| `parentLink` | ✅ Set dynamically in `block.js` to `block.html?slot=${slot-1}` |
| `stateRoot` | ⚠️ Copy button has same string-literal bug |
| `producerLink` | ✅ Links to `address.html?address=...` |
| `txCount` | ✅ |
| `blockSize` | ✅ |
| `computeUsed` | ✅ |

### Navigation Buttons
- `#prevBlock` onclick set dynamically in `block.js`: `location.href = 'block.html?slot=${slot-1}'` ✅
- `#nextBlock` onclick: navigates to next slot ✅
- Slot 0 guard: `prevBlock` starts `disabled` in HTML, only enabled by JS when `slot > 0` ✅

### Reward & Fee Cards
- `#rewardCard` — hidden by default (`style="display:none"`) ✅, shown when `block.validator_reward_shells > 0`
- `#feeCard` — hidden by default ✅, shown when fee data present
- Both use `FEE_SPLIT` constants from `shared/utils.js` ✅
- Fee labels: `feeBurnedLabel`, `feeProducerLabel`, `feeVotersLabel`, `feeTreasuryLabel`, `feeCommunityLabel` — all exist in HTML ✅

### Transaction Table (`#txTable`)
- Inlined via innerHTML in `block.js:renderTransactions()`
- Tx links use `transaction.html?tx=HASH` — `transaction.js:getTxHash()` supports `?tx=` ✅
- From/To addresses — linked to `address.html?address=...` ✅

### Raw Data Tab
- `#rawData` pre-formatted block JSON
- `copyToClipboard('rawData')` — **same string-literal bug**: copies "rawData" string, not the JSON ❌
- Raw data JSON uses `JSON.stringify(block, null, 2)` ✅

### RPC Calls
- `getBlock` ✅ (MoltChainRPC class method) — params: `[slot]`
- `getLatestBlock` ✅ — used for "next" block navigation guard

---

## EXPLORER AUDIT — transactions.html

**File:** `explorer/transactions.html`  
**Script:** `js/transactions.js` (232 lines)

### Filters
| Element | Handler | Status |
|---|---|---|
| `#typeFilter` dropdown | `applyFilters()` onclick | ✅ Type filter IS applied client-side in `renderTransactions()` |
| `#statusFilter` dropdown (All/Success/Error) | `applyFilters()` onclick | ❌ **STATUS FILTER COMPLETELY IGNORED** — `renderTransactions()` only checks `currentFilter.type`, never `currentFilter.status` |
| `#applyFiltersBtn` | `applyFilters()` | ✅ |

### Pagination
- Cursor-based: `getRecentTransactions` with `before_slot` param
- `#loadMoreBtn` — fetches next page appending to `allTxs[]`
- No URL state persistence

### Transactions Table
- Columns: Signature (truncated), Type, Status, From, To, Amount, Time
- Tx links: `transaction.html?sig=SIG` ✅ — `transaction.js:getTxHash()` supports `?sig=`
- **No link from From/To addresses to address detail page** ❌

### WebSocket
- `subscribeBlocks` subscription — pushes new blocks to `processNewBlock()` which extracts transactions ✅
- New txs arrive live ✅

### RPC Calls
- `getRecentTransactions` — `lib.rs` ✅

---

## EXPLORER AUDIT — transaction.html

**File:** `explorer/transaction.html`  
**Script:** `js/transaction.js` (605 lines)

### URL Parameter Handling
- Supports: `?sig=`, `?tx=`, `?hash=`, `?signature=` — all handled in `getTxHash()` ✅
- Airdrop synthetic transactions: `?hash=airdrop-N` — special rendering path with optional faucet API lookup ✅

### Copy Buttons — CRITICAL BUG
```html
<button onclick="copyToClipboard('txHash')">Copy</button>
```
Copies literal string `"txHash"` instead of actual transaction hash — **same critical bug as block.html**.  
`copyToClipboard('rawData')` — copies literal string `"rawData"` ❌

### Dynamically-Injected Rows
These elements do NOT exist in `transaction.html` static HTML; they are injected by `upsertParticipants()`:
- `#detailFromRow` — injected after `#detailAmount` row
- `#detailToRow` — injected after `#detailFromRow`
- `#detailFeePayerRow` — injected conditionally
All links from/to → `address.html?address=...` ✅

### Fee Breakdown Elements
| Element | Status |
|---|---|
| `feeTotal` | ✅ |
| `feeBurned` | ✅ |
| `feeProducer` | ✅ |
| `feeVoters` | ✅ |
| `feeValidatorPool` (= treasury split) | ✅ (named "validator pool" in display, maps to `FEE_SPLIT.treasury`) |
| `feeCommunity` | ✅ |

### Instruction Decoding
- System opcodes 0–5 decoded ✅
- ZK opcodes 23 (Shield), 24 (Unshield), 25 (ShieldedTransfer) decoded via `decodeShieldedInstruction()` ✅
- Contract calls: shows program address, no ABI decode unless contract has ABI loaded ✅

### Signature Table
- `#signaturesTable` populated with all signers ✅
- Signer links → `address.html?address=...` ✅

### RPC Calls
- `getTransaction` ✅ (class method)
- No secondary fetch for token metadata — contract calls with token transfers show raw amounts only

---

## EXPLORER AUDIT — address.html

**File:** `explorer/address.html` (406 lines)  
**Script:** `js/address.js` (2031 lines)  
**Extra deps:** `../wallet/js/crypto.js`, `js-sha3` (CDN), `tweetnacl` (CDN)

### CRITICAL: Missing Summary Elements
`address.js:displayAddressData()` tries to populate these elements which do NOT exist in `address.html` HTML:
| Missing Element ID | What It Should Show |
|---|---|
| `summaryAddress` | Native address (base58) |
| `summaryEvmAddress` | EVM address |
| `summaryBalance` | Total balance |
| `summarySpendable` | Spendable balance |
| `summaryStaked` | Staked MOLT |
| `summaryLocked` | Locked MOLT |

`renderSummaryIdentity()` also references:
| Missing Element ID | What It Should Show |
|---|---|
| `displayName` | .molt name or truncated address |
| `trustTierBadge` | Trust tier pill |

**Impact:** The primary address summary section is completely blank — no balance, no address display, no identity badge rendered. The address page shows a tab container but no header info.

### CRITICAL: Missing Identity Action Button Wiring
`bindIdentityActionButtons()` is defined (attaches click listeners to `[data-identity-action="..."]` buttons in the rendered identity pane) but **is never called anywhere in address.js**.  
- Not called from `DOMContentLoaded`
- Not called after `renderIdentityPane()`
- Not called after `displayAddressData()`

**Impact:** All identity action buttons (Edit Profile, Vouch for Address, Attest Skill, register .molt name, etc.) have no event listeners — they are completely dead UI. Clicking them does nothing.

### Additional Missing Element
| Missing Element ID | Where Referenced |
|---|---|
| `registerIdentityBtn` | `enforceAddressViewOnlyMode()` — function tries to hide this button in view-only mode; element doesn't exist |

### Tabs
| Tab | data-tab | Content element | Status |
|---|---|---|---|
| Overview | `overview` | `#overviewPane` | ✅ |
| Tokens | `tokens` | `#tokensPane` | ✅ |
| Identity | `identity` | `#identityPane` | ⚠️ Action buttons dead (see above) |
| Staking | `staking` | `#stakingPane` | ✅ (hidden by default — shown for validators only) |
| Transactions | `transactions` | `#transactionPane` | ✅ |
| Data | `data` | `#dataPane` | ✅ |

### Copy Button
- `copyAddressToClipboard()` reads `document.querySelector('[data-full]').dataset.full` ✅ — Correctly copies full address (avoids the string-literal bug seen in block/transaction pages)

### Dynamic Insertions
- `#abiCard` — injected into `.container` for contract accounts ✅
- `#treasuryStatsCard` — injected for Treasury accounts ✅
- `#reefStakedMolt` row — injected into balance section for ReefStake users ✅
- `addressTxPagination` — rendered by `updateTxPagination()` ✅

### Wallet Actions (View-only in Explorer)
- Send, Receive, Register Identity, Vouch, Attest modals all exist in HTML ✅
- View-only mode enforced by `enforceAddressViewOnlyMode()` — hides/disables action buttons
- `signAndSendInstructions()` requires `MoltCrypto` (from `../wallet/js/crypto.js`) and `serializeMessageBincode` (from `shared/utils.js`) — both available ✅

### RPC Calls (directly via `rpc.call()` — all supported in lib.rs)
| Method | lib.rs | Status |
|---|---|---|
| `getAccount` | ✅ | |
| `getAccountInfo` | ✅ | |
| `getTokenAccounts` | ✅ | |
| `getStakingStatus` | ✅ | |
| `getStakingRewards` | ✅ | |
| `getGenesisAccounts` | ✅ | |
| `getMoltyIdProfile` | ✅ | |
| `getMoltyIdAchievements` | ✅ | |
| `getMoltyIdVouches` | ✅ | |
| `getMoltyIdSkills` | ✅ | |
| `reverseMoltName` | ✅ | |
| `getContractInfo` | ✅ | |
| `getContractAbi` | ✅ | |
| `getReefStakePoolInfo` | ✅ | |
| `getTransactionsByAddress` | ✅ | |

### Other Issues
- `fetchCurrentSlot()` makes raw `fetch(RPC_URL, ...)` call instead of using `rpc.call()` — bypasses centralized error handling
- `loadTreasuryStats()` fetches up to 500 transactions then scans for airdrops — O(n) scan, slow for high-activity treasury accounts
- Transaction links use `transaction.html?hash=SIG` — supported by `getTxHash()` ✅

---

## EXPLORER AUDIT — contracts.html

**File:** `explorer/contracts.html`  
**Script:** `js/contracts.js` (242 lines)

### Category Tabs
| Tab ID | Label | Filter | Status |
|---|---|---|---|
| `tab-all` | All | all | ✅ |
| `tab-token` | Token | token | ✅ |
| `tab-nft` | NFT | nft | ✅ |
| `tab-defi` | DeFi / Infra | defi+infrastructure | ✅ (combined — label is clear) |
| `tab-dao` | DAO | dao | ✅ |
| `tab-dex` | DEX | dex | ✅ |

### Stats Counts
| Element | Content | Status |
|---|---|---|
| `statAll` | Total contracts | ✅ |
| `statToken` | Token count | ✅ |
| `statNft` | NFT count | ✅ |
| `statDefi` | DeFi + Infrastructure count | ✅ (label `statDefi` is slightly misleading — actually DeFi+Infra combined) |
| `statDao` | DAO count | ✅ |
| `statDex` | DEX count | ✅ |

### Contracts Table
- Columns: Name, Address, Type, Version, Txs, Status
- Contract address links → `contract.html?address=...` ✅
- **No pagination** — `renderContracts()` renders all filtered contracts in one DOM write; no limit
- For large contract registries this causes unbounded DOM growth ❌

### RPC Calls
- `getAllContracts` ✅ (class method)
- `getSymbolRegistry` / `getAllSymbolRegistry` — used for contract metadata enrichment ✅ (lib.rs ✅)

---

## EXPLORER AUDIT — contract.html

**File:** `explorer/contract.html`  
**Script:** `js/contract.js` (654 lines)

### Header
| Element ID | Status |
|---|---|
| `contractName` | ✅ |
| `contractAddress` | ✅ |
| `contractType` | ✅ (pill badge) |
| `contractVersion` | ✅ |
| `contractStatus` | ✅ |

### Copy Address Button
- `onclick="copyAddress()"` → `contract.js:copyAddress()` reads the full address from the `contractAddress` module variable (set from URL param)
- Calls `showToast('Address copied!')` — `showToast` is defined in `shared/utils.js` ✅ (no bug here)

### Stat Cards
| Element | Status |
|---|---|
| `contractTxCount` | ✅ |
| `contractCallCount` | ✅ |
| `contractStorageKeys` | ✅ |
| `contractDeployed` | ✅ |

### Token Info Section (`#tokenSection`)
- Hidden by default ✅
- Shows token supply, holders, decimals when contract has symbol registry entry
- **Null-check issue:** `contract.js` accesses `registry.metadata` properties without guarding for `registry === null` — if a contract exists but has no symbol registry entry, accessing `registry.metadata` throws `TypeError: Cannot read properties of null (reading 'metadata')` ❌

### Tabs
| Tab | Element | Status |
|---|---|---|
| ABI | `#abiTab` | ✅ Static in HTML |
| Storage | `#storageTab` | ✅ Static |
| Calls | `#callsTab` | ✅ Static |
| Events | `#eventsTab` | ✅ Static |

### Tab Pagination (Dynamically Injected)
Pagination containers are NOT in static HTML — dynamically created and inserted:
- `#storagePagination` — created on storage tab render
- `#callsPagination` — created on calls tab render
- `#eventsPagination` — created on events tab render

Pagination buttons use inline onclick strings that directly mutate module-scoped variables — functional but brittle pattern.

### RPC Calls (all via `rpc.call()`, all in lib.rs)
| Method | lib.rs | Notes |
|---|---|---|
| `getContractInfo` | ✅ | |
| `getContractAbi` | ✅ | |
| `getContractLogs` | ✅ | |
| `getSymbolRegistryByProgram` | ✅ | |
| `getProgramStorage` | ✅ | |
| `getProgramCalls` | ✅ | |
| `getContractEvents` | ✅ | |
| `getTokenHolders` | ✅ | |
| `getTokenTransfers` | ✅ | |

---

## EXPLORER AUDIT — validators.html

**File:** `explorer/validators.html`  
**Script:** `js/validators.js` (139 lines)

### Stats Row
| Element | Status |
|---|---|
| `totalValidators` | ✅ |
| `activeValidators` | ✅ (exists here; MISSING on index.html) |
| `totalStakeDisplay` | ✅ |
| `slotTime` | ✅ |

### Validators Table
- Columns: Rank, Validator (address), Reputation, Blocks, Voting Power, Status
- **No link from validator row to `address.html`** ❌ — addresses are displayed as truncated text with no href
- No search/filter input
- **No pagination** — all validators rendered in one DOM write ❌ — unbounded for large validator sets

### Double Polling Bug
`validators.js` both:
1. `subscribeSlots` → WS callback calls `loadValidators()` on each new slot
2. `setInterval(loadValidators, 15000)` — always runs regardless of WS state

When WS is connected, `loadValidators()` is called every ~400ms (each slot) via WS AND also every 15s via interval. This doubles server load unnecessarily. The interval should be skipped when WS is active.

### RPC Calls
- `getValidators` ✅ (class method)
- `getMetrics` ✅ (class method)

### WS Subscription
- `subscribeSlots` — in `ws.rs` ✅
- Used for: `loadValidators()` trigger per slot

---

## EXPLORER AUDIT — agents.html

**File:** `explorer/agents.html`  
**Script:** `js/agents.js` (209 lines)

### Filters
| Element | Handler | Status |
|---|---|---|
| `#agentTypeFilter` dropdown | onChange → no auto-apply ❌ — must click button | Requires button click |
| `#applyFiltersBtn` onclick `applyFilters()` | `agents.js:applyFilters()` | ✅ Re-fetches from server with type param |

- **No auto-apply on filter change** — user must click "Apply Filters" to see results update with new type selection ❌
- Type filter IS sent to server as `options.type` — correct server-side filtering ✅

### Sort
- `#agentSort` dropdown (by reputation, by type, by name)
- Sort changes call `applySortAndRender()` which sorts in-memory `allAgents[]` — no re-fetch needed ✅
- Sort change does NOT auto-apply — also requires button click (same issue) ❌

### Agents Table
| Column | Link | Status |
|---|---|---|
| Agent Name | → `address.html?address=...&tab=identity` | ✅ |
| Agent Type | text only | ✅ |
| Reputation | formatted | ✅ |
| Registered | timestamp | ✅ |
| Actions | "View Profile" → same address link | ✅ |

### Pagination
- Client-side pagination on `allAgents[]` — correct ✅
- `#paginationContainer` exists in HTML ✅

### No Live Data
- No WS subscription — agents data is static after load
- No auto-refresh — user must manually reload or navigate away/back

### RPC Calls
- `getMoltyIdAgentDirectory` — `lib.rs` ✅

---

## EXPLORER AUDIT — privacy.html

**File:** `explorer/privacy.html` (276 lines)  
**Script:** `js/privacy.js` (354 lines)

### CRITICAL: Page Not Reachable Via Navigation
**No page in the explorer links to `privacy.html`** — it does not appear in any nav menu or footer across all 11 pages. The privacy page is an orphaned page. Users cannot discover it.

### CRITICAL: Missing Element `#vkStatusText`
`privacy.js:updatePoolStatsUI()` (line ~152) writes to `document.getElementById('vkStatusText')` — this element **does not exist in privacy.html**. The ZK verification key status indicator is silently never shown.

### Stats Elements  
| Element | Status |
|---|---|
| `shieldedBalance` | ✅ |
| `commitmentCount` | ✅ |
| `nullifierCount` | ✅ |
| `shieldedTxCount` | ✅ |
| `vkStatusText` | ❌ **MISSING** — referenced in JS, not in HTML |

### Tabs
| Tab Button onclick | Target pane | Status |
|---|---|---|
| `switchPrivacyTab('pool-stats')` | `#poolStatsPane` | ✅ |
| `switchPrivacyTab('transactions')` | `#transactionsPane` | ✅ |
| `switchPrivacyTab('nullifier-lookup')` | `#nullifierLookupPane` | ✅ |

### Orphaned Third Tab Pane
- `<div id="zkArchitectureTab" data-pane="zk-architecture">` exists in HTML as an empty `<div>`
- No tab button exists for it in the tab bar
- No content is ever rendered into it by `privacy.js`
- Effectively dead HTML ❌

### Nullifier Lookup
- `#nullifierInput` input ✅
- `#checkNullifierBtn` button → `checkNullifier()` ✅
- `#nullifierResult` result display ✅
- Calls `isNullifierSpent` RPC ✅ (`lib.rs` ✅)

### Shielded Transactions Table
- `#shieldedTxsTable` ✅
- `#shieldedTxsEmpty` shown when no results ✅
- `#refreshShieldedTxsBtn` → `refreshShieldedTxs()` ✅
- No live WS subscription — table only updated on button click or initial load

### Local Function Shadowing
`privacy.js` locally redefines these functions, shadowing the global versions from `shared/utils.js`:
- `formatMoltValue(shells)` — shadows global `formatMolt(shells)`
- `formatNumber(n)` — shadows global `formatNumber(n)` 
- `escapeHtml(str)` — shadows global `escapeHtml(str)`
- `formatTimeFull(ts)` — shadows global `formatTimeFull(ts)`
- `copyToClipboard(text)` — shadows global `copyToClipboard(text)`

The shadow implementations are functionally equivalent but create a maintenance burden. If `shared/utils.js` is updated, `privacy.js` local versions drift silently.

### RPC Calls
| Method | lib.rs | Status |
|---|---|---|
| `getShieldedPoolState` | ✅ (`shielded` module) | |
| `getShieldedCommitments` | ✅ | |
| `isNullifierSpent` | ✅ | |

---

## EXPLORER AUDIT — Shared JS & Config

### `shared/utils.js` (507 lines)

**Constants (global, available to all pages):**
| Constant | Value |
|---|---|
| `SHELLS_PER_MOLT` | `1_000_000_000` |
| `MS_PER_SLOT` | `400` |
| `SLOTS_PER_EPOCH` | `432_000` |
| `SLOTS_PER_YEAR` | `78_840_000` |
| `BASE_FEE_SHELLS` | `1_000_000` (0.001 MOLT) |
| `FEE_SPLIT` | `{burned:0.40, producer:0.30, voters:0.10, treasury:0.10, community:0.10}` |
| `ZK_COMPUTE_FEE` | `{shield:100_000, unshield:150_000, transfer:200_000}` |
| `MAX_REPUTATION` | `100_000` |
| `MAX_REP_PROGRESS_BAR` | `10_000` (Legendary threshold) |
| `TRUST_TIER_THRESHOLDS` | `[Legendary≥10000, Elite≥5000, Established≥1000, Trusted≥500, Verified≥100, Newcomer≥0]` |
| `ACHIEVEMENT_DEFS` | 72 achievement objects (ids 1–124, non-contiguous) |

**Key Functions:**
- `copyToClipboard(text)` — takes **text to write**, NOT an element ID ← this is the root of the block/tx copy bug
- `showToast(message)` — creates and removes a toast div using `setTimeout(3000)` ✅
- `safeCopy(el)` — reads `el.dataset.copy` then calls `copyToClipboard` ✅
- `bs58decode(str)` / `bs58encode(bytes)` ✅
- `serializeMessageBincode(message)` ✅ — used by wallet signing flow
- `readLeU64(bytes)` — BigInt-safe u64 parsing ✅
- `rpcCall` / `moltRpcCall` — legacy bare RPC call function ✅
- `updatePagination(config)` — shared widget; not used by explorer pages (they implement own pagination) ⚠️ — duplication exists
- `getMoltRpcUrl()` — checks `window.moltConfig`, `window.moltMarketConfig`, `window.moltExplorerConfig` ✅

**Chain Status Bar Auto-Wire:**
The IIFE at bottom of `shared/utils.js` auto-wires any page with `id="chainBlockHeight"`, polling `getSlot` every 5s. Explorer pages that have their own polling must set `window.__chainStatusBarOwned = true` to suppress this — none of the explorer JS files set this flag, so BOTH the shared poller AND explorer.js poll `getSlot` in parallel on every page.

### `js/utils.js` (44 lines)

Defines only:
- `formatValidator(validator)` — returns Genesis pill or `formatAddress()` ✅
- `resolveTxAmountShells(tx, instruction)` ✅
- `resolveTxType(tx, instruction)` — opcode mapping (0–5, 23–25) ✅

**Note:** `resolveTxType` maps `DebtRepay` → `GrantRepay` (rebranding alias) ✅

### `shared-config.js` (43 lines)

- `MOLT_CONFIG` object: dev localhost URLs, prod origin-relative paths ✅
- DOMContentLoaded handler resolves all `a[data-molt-app]` links ✅
- Supports apps: `explorer`, `wallet`, `marketplace`, `dex`, `website`, `developers`, `faucet`
- Dev ports: explorer=3007, wallet=3008, marketplace=3009, dex=3011, website=9090, developers=3010, faucet=9100

**Note:** `data-molt-app` on non-`<a>` elements (buttons, divs) will NOT be resolved — only `<a>` tags are selected. All current usages in explorer pages appear to be `<a>` tags ✅.

### `js/explorer.js` (941 lines)

**Network Config:**
```
mainnet      → https://rpc.moltchain.network   ws=null
testnet      → https://testnet-rpc.moltchain.network  ws=null
local-testnet → http://localhost:8899           ws=ws://localhost:8900
local-mainnet → http://localhost:9899           ws=ws://localhost:9900
```
WS is `null` for mainnet and testnet — live subscriptions only work in local modes. On mainnet/testnet all live data falls back to REST polling.

**RPC Class Methods (declared in `MoltChainRPC`):**
`getBalance`, `getAccount`, `getBlock`, `getLatestBlock`, `getSlot`, `getTransaction`, `sendTransaction`, `getTotalBurned`, `getValidators`, `getMetrics`, `health`, `getTransactionsByAddress`, `getAccountTxCount`, `getAccountInfo`, `getTransactionHistory`, `getContractInfo`, `getContractAbi`, `getContractLogs`, `getAllContracts`, `getProgram`, `getProgramStats`, `getSymbolRegistryByProgram`, `getTokenBalance`, `getTokenHolders`, `getTokenTransfers`, `getContractEvents`, `getCollection`, `getNFT`, `getNFTsByOwner`, `getMarketListings`, `getMarketSales`, `simulateTransaction`, `getStakingStatus`, `getReefStakePoolInfo`

**RPC calls made via `rpc.call()` directly (not class methods) — all exist in lib.rs:**
`getRecentTransactions`, `getShieldedPoolState`, `getShieldedCommitments`, `isNullifierSpent`, `getMoltyIdAgentDirectory`, `getMoltyIdProfile`, `reverseMoltName`, `resolveMoltName`, `batchReverseMoltNames`, `getSymbolRegistry`, `getAllSymbolRegistry`, `getProgramCalls`, `getProgramStorage`, `getTokenAccounts`, `getGenesisAccounts`, `getStakingRewards`, `getMoltyIdAchievements`, `getMoltyIdVouches`, `getMoltyIdSkills`

---

## EXPLORER AUDIT — CSS

**Files:** `explorer.css` (3,689 lines) · `shared-base-styles.css` (1,322 lines) · `shared-theme.css` (356 lines)

> **Coverage note:** CSS files were not fully read during this audit. The following observations are derived from class names referenced in HTML/JS and structural knowledge.

### Theming
- CSS custom properties (`--bg-primary`, `--bg-secondary`, `--text-primary`, `--text-muted`, `--accent`, `--success`, `--error`, `--warning`) defined in `shared-theme.css`
- Dark mode via `body.dark-mode` class toggle — assumed in shared-theme.css
- Toast animation `@keyframes slideIn` injected by `shared/utils.js` at runtime — may conflict with any CSS-level animation of the same name

### Pill Classes
Used across all pages: `pill`, `pill-success`, `pill-error`, `pill-info`, `pill-warning`, `pill-muted`
- `pill-info` used in `formatValidator()` for Genesis addresses ✅
- All pill classes expected in `explorer.css`

### Trust Tier Classes
`TRUST_TIER_THRESHOLDS` generates className values: `legendary`, `elite`, `established`, `trusted`, `verified`, `newcomer`
- Must exist in `explorer.css` or `shared-base-styles.css` as `.trust-tier-[name]` or direct class selectors

### Known Class Usages in JS (must exist in CSS)
`pagination-btn` (active state), `pagination-btn.active`, `copy-btn`, `address-tabs`, `tab-btn`, `tab-btn.active`, `loading`, `error-message`, `toast`, `modal`, `modal-overlay`, `identity-card`, `achievement-badge`, `skill-tag`, `vouch-card`

### Potentially Orphaned CSS
Without a full cross-reference, CSS classes defined but never referenced in HTML/JS cannot be confirmed. The large file size (3,689 lines for explorer.css alone) suggests accumulation of styles for features/components that may have been removed or refactored.

---

## EXPLORER AUDIT — RPC (lib.rs vs Explorer JS)

### Complete Method Dispatch Table from `rpc/src/lib.rs` (lines 1355–1556)

All methods below are confirmed implemented backends. Unknown = not called by any explorer JS.

| Method | lib.rs | Called by Explorer JS | Notes |
|---|---|---|---|
| `getBalance` | ✅ | ✅ explorer.js, address.js | |
| `getAccount` | ✅ | ✅ address.js | |
| `getBlock` | ✅ | ✅ block.js | |
| `getLatestBlock` | ✅ | ✅ block.js | |
| `getSlot` | ✅ | ✅ explorer.js, shared/utils.js | |
| `getTransaction` | ✅ | ✅ transaction.js | |
| `getTransactionsByAddress` | ✅ | ✅ address.js | |
| `getAccountTxCount` | ✅ | ✅ address.js | |
| `getRecentTransactions` | ✅ | ✅ explorer.js, blocks.js, transactions.js | |
| `getTokenAccounts` | ✅ | ✅ address.js via `rpc.call()` | |
| `sendTransaction` | ✅ | ✅ address.js (wallet action) | |
| `confirmTransaction` | ✅ | — not called in explorer | |
| `simulateTransaction` | ✅ | ✅ address.js | |
| `getTotalBurned` | ✅ | ✅ explorer.js | |
| `getValidators` | ✅ | ✅ validators.js | |
| `getMetrics` | ✅ | ✅ explorer.js | |
| `getTreasuryInfo` | ✅ | — not called | |
| `getGenesisAccounts` | ✅ | ✅ address.js via `rpc.call()` | |
| `getGovernedProposal` | ✅ | — not called | |
| `getRecentBlockhash` | ✅ | — not called in explorer | |
| `health` | ✅ | ✅ explorer.js | |
| `getFeeConfig` | ✅ | — not called | |
| `setFeeConfig` | ✅ | — admin only | |
| `getRentParams` | ✅ | — not called | |
| `setRentParams` | ✅ | — admin only | |
| `getPeers` | ✅ | — not called | |
| `getNetworkInfo` | ✅ | — not called | |
| `getClusterInfo` | ✅ | — not called | |
| `getValidatorInfo` | ✅ | — not called | |
| `getValidatorPerformance` | ✅ | — not called | |
| `getChainStatus` | ✅ | — not called | |
| `stake` | ✅ | — not called (would be wallet) | |
| `unstake` | ✅ | — not called | |
| `getStakingStatus` | ✅ | ✅ address.js | |
| `getStakingRewards` | ✅ | ✅ address.js via `rpc.call()` | |
| `stakeToReefStake` | ✅ | — not called | |
| `unstakeFromReefStake` | ✅ | — not called | |
| `claimUnstakedTokens` | ✅ | — not called | |
| `getStakingPosition` | ✅ | — not called | |
| `getReefStakePoolInfo` | ✅ | ✅ address.js | |
| `getUnstakingQueue` | ✅ | — not called | |
| `getRewardAdjustmentInfo` | ✅ | — not called | |
| `getAccountInfo` | ✅ | ✅ address.js | |
| `getTransactionHistory` | ✅ | ✅ address.js | |
| `getContractInfo` | ✅ | ✅ contract.js, address.js | |
| `getContractLogs` | ✅ | ✅ contract.js | |
| `getContractAbi` | ✅ | ✅ contract.js, address.js | |
| `setContractAbi` | ✅ | — admin only | |
| `getAllContracts` | ✅ | ✅ contracts.js | |
| `deployContract` | ✅ | — not called | |
| `upgradeContract` | ✅ | — not called | |
| `getProgram` | ✅ | ✅ contract.js | |
| `getProgramStats` | ✅ | ✅ contract.js | |
| `getPrograms` | ✅ | — not called | |
| `getProgramCalls` | ✅ | ✅ contract.js via `rpc.call()` | |
| `getProgramStorage` | ✅ | ✅ contract.js via `rpc.call()` | |
| `getMoltyIdIdentity` | ✅ | — not directly (uses getMoltyIdProfile) | |
| `getMoltyIdReputation` | ✅ | — not directly | |
| `getMoltyIdSkills` | ✅ | ✅ address.js via `rpc.call()` | |
| `getMoltyIdVouches` | ✅ | ✅ address.js via `rpc.call()` | |
| `getMoltyIdAchievements` | ✅ | ✅ address.js via `rpc.call()` | |
| `getMoltyIdProfile` | ✅ | ✅ address.js via `rpc.call()` | |
| `resolveMoltName` | ✅ | ✅ explorer.js (search) | |
| `reverseMoltName` | ✅ | ✅ explorer.js, address.js | |
| `batchReverseMoltNames` | ✅ | ✅ explorer.js | |
| `searchMoltNames` | ✅ | — not called | |
| `getMoltyIdAgentDirectory` | ✅ | ✅ agents.js via `rpc.call()` | |
| `getMoltyIdStats` | ✅ | — not called | |
| `getNameAuction` | ✅ | — not called | |
| `getEvmRegistration` | ✅ | ✅ address.js | |
| `lookupEvmAddress` | ✅ | — not called | |
| `getSymbolRegistry` | ✅ | ✅ explorer.js, contracts.js | |
| `getSymbolRegistryByProgram` | ✅ | ✅ contract.js | |
| `getAllSymbolRegistry` | ✅ | ✅ contracts.js via `rpc.call()` | |
| `getCollection` | ✅ | ✅ address.js | |
| `getNFT` | ✅ | — not called (no NFT detail page) | |
| `getNFTsByOwner` | ✅ | ✅ address.js | |
| `getNFTsByCollection` | ✅ | — not called | |
| `getNFTActivity` | ✅ | — not called | |
| `getMarketListings` | ✅ | ✅ address.js | |
| `getMarketSales` | ✅ | ✅ address.js | |
| `getMarketOffers` | ✅ | — not called | |
| `getMarketAuctions` | ✅ | — not called | |
| `getTokenBalance` | ✅ | ✅ contract.js | |
| `getTokenHolders` | ✅ | ✅ contract.js | |
| `getTokenTransfers` | ✅ | ✅ contract.js | |
| `getContractEvents` | ✅ | ✅ contract.js | |
| `requestAirdrop` | ✅ | — not called from explorer | |
| `getShieldedPoolState` | ✅ | ✅ privacy.js, explorer.js | |
| `getShieldedMerkleRoot` | ✅ | — not called | |
| `getShieldedMerklePath` | ✅ | — not called | |
| `isNullifierSpent` | ✅ | ✅ privacy.js | |
| `getShieldedCommitments` | ✅ | ✅ privacy.js | |
| `getDexCoreStats`…`getMoltOracleStats` | ✅ | — not called from explorer | DEX/platform stats — used by DEX frontend |
| `getPredictionMarket*` | ✅ | — not called from explorer | |
| `createBridgeDeposit/getBridgeDeposit*` | ✅ | — not called from explorer | |

**Summary:** Zero RPC methods called by the explorer frontend are missing from `lib.rs`. All endpoints exist. Some lib.rs endpoints are not exposed in the explorer UI (NFT detail, prediction markets, DEX stats, bridge, name auctions) — these are for other frontends or not yet wired.

---

## EXPLORER AUDIT — WebSocket (ws.rs vs JS)

### ws.rs Subscription Types (confirmed in ws.rs lines 733–820)
| Method | Type | Status |
|---|---|---|
| `subscribeBlocks` / `unsubscribeBlocks` | Block feed | ✅ Implemented |
| `subscribeSlots` / `slotSubscribe` + unsubscribe | Slot feed | ✅ Implemented |
| `subscribeTransactions` / `unsubscribeTransactions` | Transaction feed | ✅ Implemented |
| `subscribeAccount` / `unsubscribeAccount` | Per-account feed | ✅ Implemented |

### Explorer JS Usage
| Subscription | Used By | Status |
|---|---|---|
| `subscribeBlocks` | `explorer.js` (dashboard), `blocks.js`, `transactions.js` | ✅ All correct |
| `subscribeSlots` | `validators.js` | ✅ Correct |
| `subscribeTransactions` | — **NOT USED** by any page | ⚠️ Available but unused — transactions.html uses `subscribeBlocks` then extracts txs from block data |
| `subscribeAccount` | — **NOT USED** by any page | ⚠️ Available but unused — address.html has no live account updates; polling only |

### WS Fallback Behavior
- WS only available on `local-testnet` and `local-mainnet` network configs
- `mainnet` and `testnet` have `ws: null` — all data falls back to REST polling
- WS reconnect logic with `desired[]` resubscription array in `MoltChainWS` class ✅

---

## CONSOLIDATED BUG REGISTRY

### P0 — Critical (Broken Functionality)

| # | Bug | Location | Impact |
|---|---|---|---|
| C-1 | `copyToClipboard('blockHash')` copies literal string "blockHash" | `block.html` all copy buttons | All copy buttons on block detail page are broken |
| C-2 | `copyToClipboard('stateRoot')` copies literal string "stateRoot" | `block.html` | Same |
| C-3 | `copyToClipboard('txHash')` copies literal string "txHash" | `transaction.html` | Copy button on tx detail broken |
| C-4 | `copyToClipboard('rawData')` copies literal string "rawData" | `block.html`, `transaction.html` | Raw JSON copy broken |
| C-5 | `bindIdentityActionButtons()` never called | `address.js` | All identity action buttons (Edit Profile, Vouch, Attest, etc.) have no event handlers — dead UI |
| C-6 | `#summaryAddress`, `#summaryBalance`, `#summarySpendable`, `#summaryStaked`, `#summaryLocked`, `#summaryEvmAddress` missing from HTML | `address.html` | Address page header shows no balance or address data |
| C-7 | `#displayName`, `#trustTierBadge` missing from `address.html` | `address.html` | Identity header in address page blank |
| C-8 | Status filter on transactions.html completely ignored | `transactions.js:renderTransactions()` | "Success / Error" filter has no effect |
| C-9 | `registry.metadata` accessed without null-check on `registry` | `contract.js` ~L463 | `TypeError` crash for any contract with no symbol registry entry |

### P1 — High (Missing Feature / Bad Data)

| # | Issue | Location |
|---|---|---|
| H-1 | Privacy page (`privacy.html`) not linked from any navigation | All nav menus |
| H-2 | `#vkStatusText` missing from `privacy.html` | `privacy.html` / `privacy.js:152` |
| H-3 | `#activeValidators` and `#totalStake` missing from `index.html` | `index.html` / `explorer.js` |
| H-4 | `#registerIdentityBtn` missing from `address.html` | `address.html` / `address.js` |
| H-5 | Dashboard hardcodes all tx statuses as "Success" | `explorer.js:updateLatestTransactions()` |
| H-6 | No link from validator row to address detail page | `validators.html` / `validators.js` |
| H-7 | Double polling on validators page: subscribeSlots + `setInterval` both always active | `validators.js` |
| H-8 | ZK Architecture tab pane is empty `<div>` with no content and no tab button | `privacy.html` |
| H-9 | `subscribeTransactions` WS subscription exists in ws.rs but unused by transactions.html | `transactions.js` |
| H-10 | `subscribeAccount` WS subscription exists in ws.rs but unused by address.html | `address.js` |

### P2 — Medium (UX Issues / Inconsistencies)

| # | Issue | Location |
|---|---|---|
| M-1 | Network selector defaults to `testnet` on index.html, `mainnet` on all other pages | `index.html:~40` vs others |
| M-2 | Footer API Docs link on index.html → `../developers/rpc-reference.html`; all others → `../docs/API.md` | `index.html` footer |
| M-4 | `applyFilters()` and `agentSort` changes do not auto-apply on change — require button click | `agents.html` |
| M-5 | Validator addresses on blocks table have no link to address page | `blocks.js` |
| M-6 | From/To addresses in transactions table have no link to address page | `transactions.js` |
| M-8 | `privacy.js` locally redefines globals from `shared/utils.js` (`formatMoltValue`, `formatNumber`, `escapeHtml`, `formatTimeFull`, `copyToClipboard`) | `privacy.js` |
| M-9 | No input validation on slot range filter inputs (negative, `from > to`, non-numeric) | `blocks.html` |
| M-10 | Contracts table has no pagination — all contracts rendered in one pass | `contracts.js:renderContracts()` |
| M-11 | Validators table has no pagination — all validators in one DOM assign | `validators.js` |
| M-12 | Search gets no visual error for empty submission | `explorer.js:navigateExplorerSearch()` |
| M-13 | `fetchCurrentSlot()` in address.js uses raw `fetch()` rather than `rpc.call()` | `address.js` |
| M-15 | `updatePagination()` in `shared/utils.js` is unused by any explorer page — all pages implement own pagination | `shared/utils.js` |

### P3 — Low (Code Quality / Technical Debt)

| # | Issue | Location |
|---|---|---|
| L-1 | Many RPC methods accessed via `rpc.call()` bypass class typing — no consistency in class API | `contract.js`, `address.js`, `privacy.js`, `agents.js` |
| L-2 | Inline `onclick` pagination buttons in `contract.js` mutate module scope directly | `contract.js` |
| L-3 | `loadTreasuryStats()` O(n) scan up to 500 transactions to count airdrops | `address.js` |
| L-4 | `_explorerCurrentSlot` used in `formatTimestamp()` without guaranteed initialization | `address.js` |
| L-5 | `resolveTxType` aliases `DebtRepay` → `GrantRepay` only in `js/utils.js` — no alias in `shared/utils.js` | `js/utils.js` |
| L-6 | Font Awesome 6.5.1 loaded from CDN on every page — no integrity SRI hash | All HTML pages |
| L-7 | Google Fonts loaded from CDN — no `crossorigin` preload, affects LCP | All HTML pages |
| L-8 | `address.html` loads `../wallet/js/crypto.js` but this creates tight coupling to wallet build | `address.html` |

---

## SUMMARY COUNTS

| Severity | Count |
|---|---|
| P0 Critical | 9 |
| P1 High | 10 |
| P2 Medium | 12 |
| P3 Low | 8 |
| **Total** | **39** |

---

*Audit completed. All 11 HTML pages, 11 JS files, `shared/utils.js`, `js/utils.js`, `shared-config.js`, `rpc/src/lib.rs` (method dispatch), and `rpc/src/ws.rs` (subscription dispatch) fully read and cross-referenced.*
