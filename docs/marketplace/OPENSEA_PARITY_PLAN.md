# Molt Market — OpenSea-Parity Production Plan

**Created:** Feb 25, 2026
**Status:** EXECUTING
**Target:** Full OpenSea feature parity for Molt Market NFT Marketplace

---

## 🔥 CRITICAL BUGS (Fix Immediately)

### BUG-1: Gas Estimate on Create Page — MoltChain Has NO Gas
- **Files:** `marketplace/js/create.js`, `marketplace/create.html`
- **Issue:** `GAS_ESTIMATE = 0.001` in create.js; "Gas Estimate ~0.001 MOLT" hardcoded in HTML
- **MoltChain uses flat fees, NOT gas metering.** The NFT Mint Fee is 0.5 MOLT per token. No gas.
- **Fix:** Remove `GAS_ESTIMATE` constant, remove gas line from HTML breakdown, remove gas from `updatePriceBreakdown()`, `updateCreateBtnState()`, and `mintNFT()` balance checks. Total = minting fee + collection fee only.

### BUG-2: Browse List View Shows "Unnamed" Instead of NFT Name
- **File:** `marketplace/js/browse.js` line ~279
- **Issue:** List view fallback is `'Unnamed'` but grid view uses `'NFT #' + (nft.token_id || nft.id || '?')`
- **Fix:** Change list view fallback to match grid pattern.

### BUG-3: Create Page Has No Price Input / Cannot Set Initial Listing
- **Files:** `marketplace/create.html`, `marketplace/js/create.js`
- **Issue:** After minting, NFT is minted but NOT listed. No price input field. Preview shows "-- MOLT".
- **Fix:** Add optional listing price input. After mint, if price > 0, auto-call `list_nft` to list the NFT for sale.

### BUG-4: buy_nft Does NOT Enforce Royalties
- **File:** `contracts/moltmarket/src/lib.rs` (`buy_nft` function, line ~149)
- **Issue:** `list_nft_with_royalty` stores royalty_recipient at bytes 112..144, but `buy_nft` NEVER reads or pays royalty_recipient. Only marketplace fee is deducted.
- **Fix:** In `buy_nft`, read royalty_recipient from listing_data[112..144]. If non-zero, read royalty_bps from collection, calculate royalty, split payment 3-way: seller / marketplace fee / royalty.

---

## 📋 CONTRACT UPGRADES (OpenSea Parity)

### C-1: Royalty Enforcement in buy_nft
- Read `royalty_recipient` from listing bytes 112..144
- Read royalty BPS from collection (query via cross-contract or store in listing)
- 3-way split: seller gets (price - fee - royalty), fee to marketplace, royalty to creator
- Add `royalty_bps` field to listing layout (extend to 147 bytes: +2 bytes for u16 bps)

### C-2: Auction System (English Auctions)
- New instructions: `create_auction`, `place_bid`, `settle_auction`, `cancel_auction`
- Auction layout: seller(32) + nft_contract(32) + token_id(8) + start_price(8) + reserve_price(8) + highest_bid(8) + highest_bidder(32) + start_time(8) + end_time(8) + settled(1) + payment_token(32) = ~177 bytes
- Storage key: `auction:{nft_contract}:{token_id}`
- Bid escrow: previous highest bidder auto-refunded when outbid
- Auto-extend: if bid in last 10 minutes, extend by 10 minutes (anti-sniping)
- `settle_auction`: transfers NFT to winner, pays seller (minus fee + royalty)

### C-3: Collection Offers
- New instructions: `make_collection_offer`, `accept_collection_offer`, `cancel_collection_offer`
- Offer on ANY NFT in a collection (not a specific token)
- Layout: offerer(32) + collection(32) + price(8) + payment_token(32) + active(1) + expiry(8) = 113 bytes
- Storage key: `col_offer:{collection}:{offerer}`
- Owner of any NFT in collection can accept

### C-4: Offer Expiration
- Add `expiry` field (u64 timestamp) to offer layout (73 → 81 bytes)
- `make_offer` gets optional expiry parameter (0 = no expiry)
- `accept_offer` checks `expiry == 0 || current_time >= expiry` before accepting
- New instruction: `cleanup_expired_offers` — anyone can call to deactivate expired offers

### C-5: Batch Operations  
- `batch_list`: List multiple NFTs in one transaction
- `batch_cancel`: Cancel multiple listings
- `batch_buy` (sweep): Buy multiple NFTs at once

### C-6: Get Offers for NFT (Read)
- New instruction: `get_offers_for_nft` — iterate storage prefix `offer:{nft_contract}:{token_id}:*`
- Returns array of active offers with offerer, price, expiry

### C-7: Royalty BPS in Listing
- Extend listing layout to store `royalty_bps` (u16) at bytes 145..147
- `list_nft` reads royalty from collection state and stores it
- `list_nft_with_royalty` also stores custom royalty_bps
- Total listing size: 147 bytes

---

## 📋 CORE DATA MODEL UPGRADES

### D-1: Expand MarketActivityKind
- Add: `Offer`, `OfferAccepted`, `OfferCancelled`, `PriceUpdate`, `AuctionCreated`, `AuctionBid`, `AuctionSettled`, `Transfer`
- Update `encode_market_activity` / `decode_market_activity`

### D-2: Add Marketplace Indexing to State
- New CF: `CF_MARKET_OFFERS` — index offers by NFT and by offerer
- New CF: `CF_MARKET_AUCTIONS` — index active auctions
- New method: `get_offers_for_nft(collection, token_id)` on MoltState
- New method: `get_auctions(collection, limit)` on MoltState

---

## 📋 RPC ENDPOINT ADDITIONS

### R-1: getMarketOffers
- Query offers for a specific NFT (contract + token_id)
- Returns: `[{ offerer, price, price_molt, payment_token, active, expiry }]`

### R-2: getCollectionStats
- Floor price, total volume, 24h volume, owner count, listed count
- Aggregated from listings + sales data

### R-3: getMarketAuctions
- List active/completed auctions with filters (collection, status, sort)

### R-4: searchNFTs
- Full-text search by name across all indexed NFTs

---

## 📋 FRONTEND UPGRADES

### F-1: Create Page — Remove Gas, Add Price Input
- Remove `GAS_ESTIMATE` and all references
- Add "Listing Price (optional)" input field after royalty
- After mint completes, if price > 0: call `list_nft` instruction
- Update preview card to show price or "Not for sale" if 0
- Update cost breakdown: show only Minting Fee + Collection Fee

### F-2: Browse Page — Fix List View Names
- Change `'Unnamed'` fallback to `'NFT #' + (nft.token_id || nft.id || '?')`

### F-3: Item Page — Show Offers
- Below price section, add "Offers" panel listing all active offers
- Each offer: offerer hash, price in MOLT, expiry, accept/cancel buttons
- Add price history section (chart or table of past sales)
- Add "Last Sale" price display

### F-4: Profile Page — Offer Management
- New tab: "Offers" — shows incoming offers on user's NFTs + outgoing offers
- Accept/Cancel offer actions from profile
- Wire up profile edit button (display name, avatar, banner — stored locally or on-chain)

### F-5: Home Page — Collection Stats
- Show floor price and 24h volume change on featured collection cards
- Add "Explore by Category" section

---

## 📋 TEST UPDATES

### T-1: Matrix Test Updates
- Add tests for gas removal (verify total = mint fee only, no gas)
- Add tests for listing price on create flow
- Add tests for royalty enforcement in buy_nft
- Add tests for auction lifecycle
- Add tests for collection offers
- Add tests for offer expiry
- Add tests for batch operations

---

## ⏰ EXECUTION ORDER (Priority)

| Phase | Task | Time |
|-------|------|------|
| **NOW** | BUG-1: Remove gas from create page | 2 min |
| **NOW** | BUG-2: Fix list view name | 1 min |
| **NOW** | BUG-3: Add price input + auto-list after mint | 5 min |
| **NOW** | BUG-4: Fix royalty in buy_nft | 5 min |
| **Phase 2** | C-2: Auction system in contract | 10 min |
| **Phase 2** | C-3: Collection offers | 5 min |
| **Phase 2** | C-4: Offer expiration | 3 min |
| **Phase 2** | C-6: Get offers for NFT | 3 min |
| **Phase 3** | D-1: Core activity kinds | 3 min |
| **Phase 3** | R-1: getMarketOffers RPC | 3 min |
| **Phase 4** | F-3: Item page offers display | 5 min |
| **Phase 4** | F-4: Profile offers tab | 5 min |
| **Phase 5** | T-1: Matrix test updates | 5 min |
| **Total** | | ~55 min |

---

## CURRENT STATUS

- [x] Plan written
- [ ] BUG-1: Gas removed
- [ ] BUG-2: List view name fixed
- [ ] BUG-3: Price input added
- [ ] BUG-4: Royalty enforcement
- [ ] C-2: Auctions
- [ ] C-3: Collection offers
- [ ] C-4: Offer expiry
- [ ] C-6: Get offers read
- [ ] D-1: Core activity kinds
- [ ] R-1: getMarketOffers
- [ ] F-3: Item offers display
- [ ] F-4: Profile offers tab
- [ ] T-1: Tests updated
