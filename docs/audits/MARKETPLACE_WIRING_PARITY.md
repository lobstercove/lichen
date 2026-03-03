# Marketplace Wiring Parity Plan

Goal: wire live core/RPC/WS data so every Marketplace page renders the exact same UI, layout, and data shapes as the current mock-driven UI. The UI must not change; only the data source changes.

## TODO (Deferred)
- Wire Featured Collections + Trending NFTs + other landing widgets from live data.
- Wire browse.html data sources, filters, and pagination.
- Wire create.html form inputs (banner, avatar, image URL/upload) and live collections.
- Wire profile.html data sources (tabs, stats, activity).

## Non-Negotiable UI Invariants
- All sections, cards, tables, counts, and formatting remain identical.
- Identical field names and derived values in `marketplace/js/marketplace.js`.
- Deterministic fallback visuals (colors, avatars, banners) when on-chain data is missing.
- Same ordering and limits as mock generators.

## Current Mock Shapes (Authoritative)
These are the exact shapes the live data adapter must output.

### Featured Collections
```js
{
  id: "collection-0",
  name: "MoltPunks",
  banner: "#FF6B35" | "linear-gradient(...)" | "#RRGGBB",
  avatar: "🦞",
  items: 1234,
  owners: 456,
  floor: "12.34", // string
  volume: 56789
}
```

### Trending NFTs
```js
{
  id: "nft-0",
  name: "#1234",
  collection: "MoltPunks",
  image: "linear-gradient(...)" | image URL,
  price: "12.34", // string
  lastSale: "22.10", // string
  rarity: "Common" | "Uncommon" | "Rare" | "Epic" | "Legendary"
}
```

### Top Creators
```js
{
  id: "creator-0",
  name: "MoltMaster",
  address: "MOLTabcd...",
  avatar: "🎨",
  sales: 1234,
  volume: 56789
}
```

### Recent Sales
```js
{
  id: "sale-0",
  nft: "#1234",
  collection: "MoltPunks",
  price: "12.34", // string
  from: "MOLTabcd...",
  to: "MOLTefgh...",
  timestamp: 1700000000000, // ms
  image: "linear-gradient(...)" | image URL
}
```

### Live Stats (Hero)
- `totalNFTs`, `totalCollections`, `totalVolume`, `totalCreators` (integers)
- Rendering uses `animateNumber()` and `toLocaleString()`

## Live Data Sources (Core/RPC/WS)
These are already in place or planned, and must be adapted to the mock shapes above.

### Core/RPC
- Collections: `getCollection(collectionPubkey)`
- NFTs by collection: `getNFTsByCollection(collectionPubkey, { limit })`
- NFTs by owner: `getNFTsByOwner(ownerPubkey, { limit })`
- NFT activity (mint/transfer): `getNFTActivity(collectionPubkey, { limit })`
- Programs: `getProgram(programPubkey)`, `getPrograms({ limit })`
- Program calls: `getProgramCalls(programPubkey, { limit })`

### WebSocket
- `subscribeNftMints`, `subscribeNftTransfers`
- `subscribeProgramCalls`
- `subscribeMarketListings`, `subscribeMarketSales` (marketplace program events)

## Adapter Layer (Single Source of Truth)
Add a data adapter module that takes RPC/WS responses and emits the mock shapes exactly.

### Example Adapter API
```js
marketplaceDataSource = {
  getFeaturedCollections(limit),
  getTrendingNFTs(limit, period),
  getTopCreators(limit),
  getRecentSales(limit),
  getStats(),
  subscribeMarketUpdates(callback)
}
```

### Mapping Rules (Exact Parity)
- `id`: use canonical on-chain pubkeys or `tx_signature` with stable prefixes (`collection-`, `nft-`, `sale-`).
- `name`: collection name from on-chain metadata; NFT name is `#${token_id}`.
- `banner`: if no on-chain banner, derive from hash (deterministic color or gradient).
- `avatar`: if no icon, derive emoji from hash bucket (same 8 emojis as mock).
- `items`: collection.minted (or total tokens in collection index).
- `owners`: unique owner count (derived by NFT owner index scan).
- `floor`: min active listing price for collection; format to 2 decimals string.
- `volume`: sum of sale prices for collection; integer.
- `image`: NFT metadata image URL; if missing, use deterministic gradient.
- `price`: current listing price string (2 decimals).
- `lastSale`: most recent sale price string (2 decimals).
- `rarity`: computed from metadata traits OR bucketed by token_id if traits absent.
- `from/to`: use base58 pubkeys; the UI already truncates via `formatHash()`.
- `timestamp`: sale event timestamp in ms; required by `timeAgo()`.

## Marketplace Listings/Sales (Program-Level)
Marketplace listings/sales are program events. The UI expects sale rows and listing prices; to keep parity:
- The marketplace program must emit `MarketListing` and `MarketSale` events with:
  - `collection`, `token`, `price`, `seller`, `buyer`, `timestamp`, `tx_signature`
- RPC must expose listing queries, and WS must stream listing/sale events.
- Until live, the adapter must keep the same mock count/ordering using deterministic test data.

## Ordering and Limits (Must Match Mock)
- Featured collections: 6 items.
- Trending NFTs: 8 items; reloaded on period filter click.
- Top creators: 5 items from top list of 10.
- Recent sales: 10 rows.
- Stats refresh every 10s.

## Page-by-Page Data Requirements (All HTML Checked)
These map directly to elements in `marketplace/*.html` so wiring does not alter UI.

### Landing (index.html)
- Featured collections grid: 6 cards -> `getFeaturedCollections(6)`
- Trending NFTs: 8 cards + 24h/7d/30d filter -> `getTrendingNFTs(8, period)`
- Top creators: 5 cards -> `getTopCreators(5)`
- Recent sales: 10 rows -> `getRecentSales(10)`
- Hero stats: total NFTs, collections, volume, creators -> `getStats()`

### Browse (browse.html)
- Collections filter list: derived from all collections + counts
- NFT grid: paginated list, supports filters (status/price/rarity/collection)
- Item count + pagination totals must match filter results

### Create (create.html)
- Collections dropdown: user collections + public collections
- Mint fee display (static): keep as 0.001 MOLT
- Preview card: uses same NFT card shape as landing

### Item (item.html)
- NFT media (image/video), name, collection, owner, creator
- Price card: current listing price + USD placeholder
- Activity list: mint/transfer/list/sale events
- Properties grid + details (token id, standard, royalty)
- More from collection: 4 NFTs

### Profile (profile.html)
- Profile stats: NFTs, Created, Sold, Volume
- Collected/Created/Favorited: grid + sort
- Activity tab: table with event, item, price, from, to, date

## Deterministic Visual Fallbacks
When chain data is incomplete, generate visuals deterministically from pubkey:
- Banner color: hash pubkey -> `#RRGGBB`
- Avatar emoji: hash mod 8 -> same emoji list
- NFT image: hash pubkey -> gradient
This guarantees the UI stays visually identical in layout and density.

## Validation Checklist
- UI sections, counts, and animations unchanged.
- Adapter outputs exactly the mock fields.
- Snapshot tests validate DOM text/structure for each page.
- RPC/WS payloads are contract-tested against adapter expectations.

## Implementation Notes
- Keep UI code changes minimal: only replace `generateCollections()` etc with adapter calls.
- Never change HTML/CSS while wiring data; only swap the data source.
- If any field is missing on-chain, fallback deterministically as above.
