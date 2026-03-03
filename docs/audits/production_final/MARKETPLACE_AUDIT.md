# Molt Market — Full Production Audit

**Scope:** All files under `marketplace/`, plus `contracts/moltmarket/`, `contracts/moltpunks/`, `contracts/moltauction/`, `contracts/reef_storage/`, and relevant slices of `rpc/src/lib.rs`.  
**Date:** 2025

---

## A) NFT CONTRACT WIRING

### [CRITICAL] A-2 — `accept_offer` argument order mismatched in `profile.js`

`moltmarket::accept_offer` (line ~560):
```rust
pub extern "C" fn accept_offer(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,   // ← position 1
    token_id: u64,                  // ← position 2
    offerer_ptr: *const u8,        // ← position 3
)
```

**`item.js`** (`_itemAcceptOffer`): `[seller, nftContract, tokenId, offerer]` → **matches** moltmarket ✓  
**`profile.js`** (`_profileAcceptOffer`): `[seller, offerer, nftContract, tokenId]` → **wrong order** — `offerer` and `nftContract` are swapped. Profile accept-offer will always route the payment/NFT to the wrong address.

---

### [CRITICAL] A-3 — All transactions sent to `CONTRACT_PROGRAM_ID = [0xFF; 32]` placeholder

**Files:** `marketplace/js/item.js`, `marketplace/js/profile.js`, `marketplace/js/create.js`

```js
const CONTRACT_PROGRAM_ID = bs58encode(new Uint8Array(32).fill(0xFF));
```

All `sendTransaction` calls use `program_id: CONTRACT_PROGRAM_ID`. The resolved marketplace address (`mp`, from `resolveMarketplaceProgram()`) only appears in the `accounts` array, not as `program_id`. Every buy, sell, cancel, list, and offer transaction is dispatched to a non-existent program and will fail.

---

### [CRITICAL] A-4 — `createCollection` RPC method does not exist

**File:** `marketplace/js/create.js`, line ~393

```js
await rpcCall('createCollection', [{ name, symbol, creator }])
```

Searching `rpc/src/lib.rs` (line 634): `"CreateCollection"` is only a **transaction-type label** returned by `classify_instruction()`, not a dispatchable RPC handler. There is no `"createCollection"` entry in the RPC dispatch table. This call always throws a JSON-RPC error, caught silently, and execution falls into the fallback deployment path.

The fallback (`buildContractCallData('deploy_nft_collection', ...)`) is also not a documented contract function in any of the reviewed contracts. Collection creation is broken end-to-end.

---

### [CRITICAL] A-5 — Mint instruction encoding does not match WASM ABI

**File:** `marketplace/js/create.js`, lines ~560–580

```js
instructionData[0] = 1;  // opcode = mint
// writes: tokenId u64LE, uriLength u64LE, then uri bytes
```

`moltpunks::mint` (line ~97) is a `#[no_mangle] extern "C"` function expecting raw pointer arguments via the WASM calling convention:
```rust
pub extern "C" fn mint(
    caller_ptr: *const u8,
    to_ptr: *const u8,
    token_id: u64,
    metadata_ptr: *const u8,
    metadata_len: u32,
) -> u32
```

There is no opcode dispatch in `moltpunks`. The `[opcode=1, tokenId, uriLen, uri]` binary payload would be interpreted as garbage pointer values with no defined behavior.

---

### [CRITICAL] A-6 — `place_bid` and auction settlement: no frontend UI exists

`moltmarket` contains a complete embedded auction system:
- `create_auction(seller, nft_contract, token_id, start_price, reserve_price, duration, payment_token)`
- `place_bid(bidder, nft_contract, token_id, bid_amount)`
- `settle_auction(caller, nft_contract, token_id)` (anyone can settle after end)
- `cancel_auction(seller, nft_contract, token_id)` (seller only, before any bids)

`moltauction/src/lib.rs` contains a separate, second full auction implementation. `rpc/src/lib.rs` exposes a `getMarketAuctions` endpoint. **None of this is wired into any marketplace page.** There is no bid UI, no auction creation flow, no countdown, no settlement button anywhere in `browse.html`, `item.html`, or `create.html`.

---

### [HIGH] A-7 — Royalty never passed to `list_nft`; always zero on-chain

Listing layout bytes `112..144` = `royalty_recipient`, bytes `145..147` = `royalty_bps`. `list_nft` writes zeros into both. `moltmarket::buy_nft` reads royalty from the stored listing data — if zero, no royalty is paid. The `list_nft_with_royalty` function exists and correctly stores the fields, but **no JS page ever calls it**. Royalties are structurally dead for all direct-listing sales.

---

### [HIGH] A-8 — `moltpunks` restricts minting to one privileged `minter` address

`moltpunks/src/lib.rs` line ~97:
```rust
if caller.0 != get_minter().0 { return 0; }
```

Only the address stored as `minter` at initialization time can mint tokens. `create.js` does not handle this restriction or deploy a user-owned proxy collection. Any user attempting to mint via the Create page into a moltpunks-type contract will receive a silent failure.

---

### [MEDIUM] A-9 — Collection-offer system entirely unwired

`moltmarket` has `make_collection_offer`, `accept_collection_offer`, and `cancel_collection_offer` with full expiry and ownership verification. No marketplace page references these functions. Users cannot make or accept collection-wide floor offers.

---

### [MEDIUM] A-10 — `update_listing_price` function exists but is not exposed in UI

`moltmarket::update_listing_price` allows in-place price editing without cancel+relist. `item.js` and `profile.js` have no "Edit Price" action; sellers must cancel the listing and create a new one, paying two transaction fees.

---

## B) METADATA / STORAGE

### [CRITICAL] B-1 — Metadata stored as inline `data:` URI — reef_storage never called

**File:** `marketplace/js/create.js`, line ~555

```js
metadataUri = 'data:application/json;base64,' + utf8ToBase64(JSON.stringify(metadata));
```

The `metadata` object contains `image: uploadedDataUrl` — the raw base64-encoded file content. For a typical 1 MB JPEG, this produces ~1.4 MB of base64 embedded in the transaction payload. MoltChain transactions have practical size limits; any non-trivial image will cause the transaction to be rejected. `reef_storage` is never called. No IPFS upload step exists.

---

### [HIGH] B-2 — File size limit mismatch between UI and enforcement

**File:** `marketplace/create.html` (UI text): "Max 100MB"  
**File:** `marketplace/js/create.js`, `handleFile`: `if (file.size > 50 * 1024 * 1024)` → 50 MB limit

The actual enforced limit is 50 MB. Displaying 100 MB misleads users into selecting valid files that are then rejected without a clear reason.

---

### [MEDIUM] B-3 — No IPFS or content-addressed storage integration

All NFT images are embedded as data URIs. There is no IPFS pinning, Arweave upload, or reef_storage integration anywhere in the minting flow. Long-term metadata permanence is zero: if the chain data is lost or the data URI is truncated (due to tx size limits), the NFT has no image.

---

## C) REAL-TIME

### [HIGH] C-1 — WebSocket `null` for mainnet and testnet

**File:** `marketplace/js/marketplace-config.js`

```js
mainnet: { rpcUrl: 'https://rpc.moltchain.network', wsUrl: null },
testnet: { rpcUrl: 'https://testnet-rpc.moltchain.network', wsUrl: null },
```

All real-time event handling relies on WebSocket. For mainnet and testnet deployments, `wsUrl` is `null`. There are no polling fallbacks for listing changes, new bids, or auction countdowns on production networks.

---

### [HIGH] C-2 — No auction countdown or anti-snipe feedback

`moltmarket::place_bid` extends the auction end time by 600 seconds when a bid lands within the last 600 seconds. The frontend has no countdown UI and does not subscribe to bid events. Users have no visibility into auction state, end time, or extension events.

---

### [MEDIUM] C-3 — No real-time offer notification

When a buyer submits a `make_offer` transaction there is no mechanism to notify the NFT owner. No WS subscription exists for offer events, and the profile page `Offers` tab only loads on explicit navigation.

---

## D) RPC WIRING

### [CRITICAL] D-1 — `createCollection` is not an RPC endpoint

See A-4. The `rpc/src/lib.rs` dispatch table has no `"createCollection"` handler. The confusion arises from `classify_instruction()` returning the string `"CreateCollection"` for opcode-6 transactions — this is a read-only classifier, not an action endpoint.

---

### [HIGH] D-2 — `checkListingStatus` fetches all 500 listings and sales on every NFT page load

**File:** `marketplace/js/item.js`, `checkListingStatus`

```js
const listings = await rpcCall('getMarketListings', [{ limit: 500 }]);
const sales    = await rpcCall('getMarketSales',    [{ limit: 500 }]);
```

Both calls fire on every `item.html` load. There is no per-NFT RPC endpoint for listing status; the entire listing set is downloaded and scanned client-side with `Array.find`. At scale this is O(all listings) per page view. A `getListingByNft(collection, tokenId)` endpoint should be added to `rpc/src/lib.rs`.

---

### [MEDIUM] D-3 — `getMarketOffers` fetches by collection, not by token; client-side filters

**File:** `marketplace/js/item.js`, `loadOffers`

```js
rpcCall('getMarketOffers', [{ collection: collectionId, limit: 50 }])
```

The RPC returns all offers for the collection. `item.js` then filters by `tokenId` in JavaScript. For large collections this is wasteful and may miss offers if the collection has more than 50 total offers.

---

### [MEDIUM] D-4 — Stats are assembled from 500-offer scan, no dedicated endpoint

**File:** `marketplace/js/marketplace-data.js`, `getTopCreators` / `getTrendingNFTs`

Both functions load up to 500 sales records to compute creator/volume stats. No `getMarketStats` or `getTopCreators` RPC endpoint exists; all aggregation happens in-browser on every homepage load.

---

### [LOW] D-5 — `getFeaturedCollections`, `getTrendingNFTs`, `getTopCreators` passed args they ignore

**File:** `marketplace/js/marketplace.js`, lines ~85, 107, 125

```js
dataSource.getFeaturedCollections(6)   // fn takes 0 params
dataSource.getTrendingNFTs(8, period)  // fn takes 0 params
dataSource.getTopCreators(5)           // fn takes 0 params
```

The limit/period arguments are silently discarded. Collections and creators are hardcoded or return all available data regardless of the requested limit.

---

## E) FUNCTIONAL ISSUES

### [CRITICAL] E-1 — Browse page "Clear Filters" button calls undefined function

**File:** `marketplace/browse.html`, line 54

```html
<button onclick="clearFilters()">Clear all</button>
```

`browse.js` never defines or exposes a `clearFilters` global. Clicking this button throws `ReferenceError: clearFilters is not defined` and silently breaks the filter panel.

---

### [HIGH] E-2 — Browse pagination cuts off at 10 pages (200 items max visible)

**File:** `marketplace/js/browse.js`, `renderPagination`

```js
for (var i = 1; i <= totalPages && i <= 10; i++)
```

If there are more than 200 NFTs (page size 20 × 10 pages), pages beyond 10 are invisible. No ellipsis navigation or dynamic window is implemented.

---

### [HIGH] E-3 — Royalty UI cap 50%, contract enforces 10% — silent truncation

`create.html` royalty input: `max="50"` (50%). `moltauction::set_royalty` and `moltauction::finalize_auction` cap at `1000` bps (10%). A creator who sets 50% will receive 10% with no error shown.

Note: `moltmarket::list_nft_with_royalty` caps at `5000` bps (50%). The two contracts have different limits. The frontend has no awareness of which contract is active.

---

### [HIGH] E-4 — Profile activity table undefined field access

**File:** `marketplace/js/profile.js`, `loadActivity`, line ~575

```js
escapeHtml(event.token)        // event.token may be undefined
escapeHtml(event.from || '')   // sale events use event.seller/event.buyer, not event.from/event.to
```

NFTActivity events from `getNFTActivity` use field names `seller`/`buyer`/`collection`/`token_id`. The profile activity renderer references `event.token`, `event.from`, and `event.to`, which may all be `undefined` on sale events, rendering `"undefined"` strings in the table.

---

### [HIGH] E-5 — `?filter=featured` and `?filter=creators` URL params silently ignored by `browse.js`

**File:** `marketplace/index.html`, lines ~96, 129 → links to `browse.html?filter=featured`  
**File:** `marketplace/js/browse.js`, `init()` → reads `?collection` and `?q` only

Browse does not parse `?filter=`. Navigating from the homepage "View All" buttons lands on an unfiltered browse page, discarding the intent.

---

### [HIGH] E-6 — `Has Offers` browse filter is applied in HTML but not in code

**File:** `marketplace/browse.html` — `filterHasOffers` checkbox exists in the filter panel  
**File:** `marketplace/js/browse.js` — `applyFilters()` never reads `filterHasOffers.checked`

The checkbox is rendered but has no effect.

---

### [MEDIUM] E-7 — `make_offer_with_expiry` exists but frontend calls `make_offer`

`moltmarket` has two offer functions: `make_offer` (no expiry) and `make_offer_with_expiry` (with expiry). The UI shows an "Expiry" field in the offer modal in `item.html`, but `item.js:handleMakeOffer` always calls `make_offer`, ignoring the expiry input.

---

### [MEDIUM] E-8 — "Favorites" tab is permanently stubbed

**File:** `marketplace/js/profile.js`, `loadFavoritedNFTs`

```js
container.innerHTML = '<p class="empty-state">Favorites coming soon</p>';
```

The Favorites navigation tab is visible and clickable but returns a placeholder for all users.

---

### [MEDIUM] E-9 — `deriveTokenAccount` uses custom SHA-256 derivation with no on-chain equivalent documented

**File:** `marketplace/js/create.js`, `deriveTokenAccount`

```js
SHA256(collectionBytes || tokenIdBytesLE)
```

There is no documented on-chain PDA derivation mechanic to confirm the runtime uses the same formula. If the runtime uses a different PDA seed (e.g., includes a program_id nonce), all derived token accounts will be wrong and NFTs will be sent to an incorrect address.

---

### [LOW] E-10 — Profile `sortBy === 'sales'` sorts by price, not sales count

**File:** `marketplace/js/profile.js`, `applySortFilter`

```js
case 'sales': a.price - b.price  // sorts by price, labeled as "Most Sales"
```

---

## F) SECURITY

### [CRITICAL] F-1 — Transaction `program_id` is a dead placeholder `[0xFF; 32]`

See A-3. All state-mutating transactions are dispatched to a non-existent program. This is both a functional failure and a security risk: if a future deployment accidentally registers a contract at the all-`0xFF` address, users could unknowingly send signed transactions to an arbitrary program.

---

### [HIGH] F-2 — No offer escrow in `moltmarket::make_offer`

`moltmarket::make_offer` stores the offer in contract storage but **does not escrow the offer funds**. When the seller calls `accept_offer` the contract attempts to transfer from the buyer's account at that moment. If the buyer has spent those funds between `make_offer` and `accept_offer`, the transfer fails silently and the NFT is not transferred. Offers carry no economic commitment.

---

### [HIGH] F-3 — `accept_collection_offer` in moltmarket: fee transfer pulls from `offerer` twice

**File:** `contracts/moltmarket/src/lib.rs`, `accept_collection_offer`

The function calls `call_token_transfer(payment_token, offerer, seller, seller_amount)` and then immediately calls `call_token_transfer(payment_token, offerer, fee_addr, fee_amount)`. Both pulls come from `offerer`'s live balance. If the first transfer partially drains the account, the second transfer could overdraw `offerer`. A single escrow → distribute pattern (as used in `moltmarket::buy_nft`) should be used instead.

---

### [HIGH] F-4 — Fake collection impersonation not prevented

Collections are identified solely by contract address. Any actor can deploy a contract named "MoltPunks" with the same `get_name()` string. The frontend reads `c.name` from `getAllContracts` with no on-chain verified-collection registry. Browse and item pages can display malicious clones alongside originals with no visual differentiation.

---

### [MEDIUM] F-6 — No rate-limit or spam protection on offer creation

`moltmarket::make_offer` and `make_offer_with_expiry` accept any non-zero price. An attacker can flood an NFT with low-ball offers (e.g., 1 shell) to obscure legitimate offers and inflate the displayed offer count. No minimum-offer-value or offer-count-per-wallet limit exists.

---

### [LOW] F-7 — `moltmarket::settle_auction` — royalty paid from contract balance, not from winning bid escrow

The auction escrowed the winning bid to `marketplace_fee_addr`, but `settle_auction` computes `seller_amount = price - fee - royalty` and then calls `call_token_transfer(payment_token, marketplace_addr, seller, seller_amount)`. If any intermediate transfer fails (royalty recipient reverts), the seller is underpaid but the NFT has already been transferred. No rollback is possible.

---

## G) STYLE / UI ISSUES

### [HIGH] G-1 — Footer links broken on three of five pages

**Pages:** `browse.html`, `create.html`, `profile.html`

```html
<a href="#docs">Documentation</a>
<a href="#api">API</a>
<a href="#discord">Community</a>
```

These are anchor-only links that scroll to `#docs` on the same page (which does not exist). `index.html` and `item.html` use correct relative paths (`../developers/index.html`, `https://discord.gg/moltchain`).

---

### [MEDIUM] G-2 — Chain status bar missing from browse, create, and profile pages

`index.html` footer includes `chainDot`, `chainBlockHeight`, and `chainLatency` elements wired by `shared/utils.js`. These DOM elements are absent from `browse.html`, `create.html`, and `profile.html`. The auto-poll fires regardless (uselessly), and users on those pages have no chain health indicator.

---

### [MEDIUM] G-4 — Royalty input max 50% disagrees with contract cap (10% in moltauction)

See E-3. `create.html` `<input max="50">` allows 50% entry, but `moltauction` silently caps at `1000` bps. No client-side warning or capping exists.

---

### [LOW] G-5 — `marketplace.css` defines CSS variables not present in shared CSS

`--orange-primary`, `--bg-hover`, `--radius-sm`, `--yellow-warning` are defined only in `marketplace.css`. If any shared component (e.g., `wallet-connect.js` DOM) references these variables, they will be `undefined` on non-marketplace pages and render as empty/inherited values.

---

### [LOW] G-6 — Browse pagination limited to 10 visible pages, no indication of remaining pages

See E-2. The pagination strip simply stops at page 10 with no ellipsis or "next range" control. Users cannot tell whether the collection continues beyond page 10.

---

## Summary Table

| ID | File(s) | Severity | Category | Issue |
|----|---------|----------|----------|-------|
| A-2 | `profile.js:_profileAcceptOffer` | CRITICAL | Wiring | `accept_offer` arg order wrong — swaps `offerer` and `nft_contract` |
| A-3 | `item.js`, `profile.js`, `create.js` | CRITICAL | Wiring | All transactions route to `[0xFF;32]` placeholder, not marketplace program |
| A-4 | `create.js:~393` | CRITICAL | Wiring | `createCollection` RPC endpoint does not exist |
| A-5 | `create.js:~566` | CRITICAL | Wiring | Mint opcode binary payload doesn't match `moltpunks::mint` WASM ABI |
| A-6 | Entire marketplace | CRITICAL | Wiring | Full auction system (moltmarket + moltauction) is completely unwired |
| A-7 | `item.js`, `profile.js`, `create.js` | HIGH | Wiring | `list_nft` never sets royalty fields — royalties always zero |
| A-8 | `create.js` | HIGH | Wiring | moltpunks minting restricted to `minter` address; all user mints fail |
| A-9 | All pages | MEDIUM | Wiring | Collection-offer functions in moltmarket never wired |
| A-10 | `item.js`, `profile.js` | LOW | Wiring | `update_listing_price` contract function has no UI |
| B-1 | `create.js:~555` | CRITICAL | Storage | Metadata stored as inline data URI; reef_storage never called |
| B-2 | `create.html`, `create.js` | HIGH | Storage | UI shows 100 MB limit; code enforces 50 MB |
| B-3 | `create.js` | MEDIUM | Storage | No IPFS/Arweave upload; metadata permanence is zero |
| C-1 | `marketplace-config.js` | HIGH | Real-time | `wsUrl: null` for mainnet/testnet — no real-time updates in production |
| C-2 | All pages | HIGH | Real-time | No auction countdown or anti-snipe extension feedback |
| C-3 | `profile.js` | MEDIUM | Real-time | No real-time offer notification for NFT owners |
| D-1 | `rpc/src/lib.rs` | CRITICAL | RPC | `createCollection` is a classifier string, not an RPC endpoint |
| D-2 | `item.js:checkListingStatus` | HIGH | RPC | Fetches 500 listings + 500 sales on every NFT detail page load |
| D-3 | `item.js:loadOffers` | MEDIUM | RPC | Offers fetched by collection, filtered client-side per token |
| D-4 | `marketplace-data.js` | MEDIUM | RPC | Stats assembled via full 500-record scan in-browser |
| D-5 | `marketplace.js:85,107,125` | LOW | RPC | Limit args to data functions silently ignored |
| E-1 | `browse.html:54`, `browse.js` | CRITICAL | Functional | `clearFilters()` undefined — filter clear button throws ReferenceError |
| E-2 | `browse.js:renderPagination` | HIGH | Functional | Browse hard-capped at 10 pages (200 items); no ellipsis navigation |
| E-3 | `create.html`, contracts | HIGH | Functional | Royalty UI allows 50%; moltauction silently caps at 10% |
| E-4 | `profile.js:~575` | HIGH | Functional | Activity table reads `event.token/from/to`; actual fields are `seller/buyer` |
| E-5 | `browse.js`, `index.html` | HIGH | Functional | `?filter=featured/creators` URL param silently ignored |
| E-6 | `browse.html`, `browse.js` | HIGH | Functional | "Has Offers" filter checkbox renders but has no effect |
| E-7 | `item.js:handleMakeOffer` | MEDIUM | Functional | Expiry input ignored; `make_offer_with_expiry` never called |
| E-8 | `profile.js` | MEDIUM | Functional | Favorites tab permanently stubbed |
| E-9 | `create.js:deriveTokenAccount` | MEDIUM | Functional | Custom SHA-256 PDA derivation unverified against runtime |
| E-10 | `profile.js:applySortFilter` | LOW | Functional | "Most Sales" sort compares by price, not sale count |
| F-1 | All JS pages | CRITICAL | Security | Signed txns sent to `[0xFF;32]` dead placeholder program |
| F-2 | `moltmarket::make_offer` | HIGH | Security | No offer escrow — offers carry no economic commitment |
| F-3 | `moltmarket::accept_collection_offer` | HIGH | Security | Fee transfer pulls twice from `offerer` live balance |
| F-4 | All pages | HIGH | Security | No verified-collection registry; clones indistinguishable from originals |
| F-6 | `moltmarket::make_offer` | MEDIUM | Security | No minimum offer or per-wallet offer limit; spam vector |
| F-7 | `moltmarket::settle_auction` | LOW | Security | Royalty transfer failure leaves seller underpaid after NFT already moved |
| G-1 | `browse.html`, `create.html`, `profile.html` | HIGH | Style | Footer links are `#` placeholders; three of five pages broken |
| G-2 | Same three pages | MEDIUM | Style | Chain status bar DOM elements absent; chain health invisible |
| G-4 | `create.html` | MEDIUM | Style | Royalty `max="50"` disagrees with moltauction 10% cap; no warning shown |
| G-5 | `marketplace.css` | LOW | Style | Marketplace-only CSS vars may be undefined on shared components |
| G-6 | `browse.js` | LOW | Style | Pagination stops at page 10 with no ellipsis or range indicator |
