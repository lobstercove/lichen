# MoltChain Programs + Marketplace Full Plan

Date: 2026-02-09
Status: Locked economics, full coverage plan

## Goals
- Agent-first blockchain with predictable, ultra-low costs and no gas model.
- Production-ready Programs platform and Marketplace with real on-chain data.
- Full coverage across core code, SDKs, RPC/API, WebSocket, and Explorer.
- Keep UX simple: deploy, call, upgrade, mint, list, buy, transfer.

## Economic Model (Locked)

### Fee model
- Flat fee per tx type, no gas, no compute units.
- Optional priority tip (future) for faster inclusion; tips go to block producer.
- Fee split for protocol fees: 40% burn, 30% producer, 10% voters, 10% validator pool, 10% community.

### Base fee alignment
- Base fee must be 100,000 shells (0.0001 MOLT) everywhere.
- Update runtime `BASE_FEE` to 100,000 shells.
- Update any docs or configs to the same value.

### Fee table (base + specific)
- Transfer: 100,000 shells (0.0001 MOLT)
- CreateAccount: 100,000 shells (0.0001 MOLT)
- Contract Call: 100,000 shells (0.0001 MOLT)
- Contract Deploy: 2,500,100,000 shells (2.5001 MOLT)
- Contract Upgrade: 1,000,100,000 shells (1.0001 MOLT)
- Contract Close: 100,000 shells (0.0001 MOLT)
- NFT Mint: 1,100,000 shells (0.0011 MOLT)
- NFT Collection Create: 100,000,100,000 shells (100.0001 MOLT)

Notes:
- Fee-free system opcodes remain fee-free (Reward, GrantRepay, GenesisTransfer, GenesisMint).
- All values are vote-adjustable via governance.

### Rent model (locked)
- Storage rent only, linear in stored bytes, no compute-based pricing.
- Rate: 1,000 shells per KB per month (0.000001 MOLT per KB per month).
- Billing cadence: weekly or monthly, rounded up to the nearest shell.
- Agent-friendly free tier: first 1 KB free per account.
- Rent rates are vote-adjustable via governance.

### Governance controls
- Add `rent_rate_shells_per_kb_month` to fee governance.
- Add `nft_collection_fee` and `nft_mint_fee` to fee governance.
- Align governance to update all flat fees and rent without redeploying binaries.

## Programs Platform Plan

### On-chain tx types (core)
- Deploy program
- Upgrade program
- Call program
- Close program
- Transfer
- CreateAccount

### Program lifecycle
- Deploy: create program account, store code, set owner and upgrade authority.
- Call: execute program with deterministic context, no gas.
- Upgrade: replace code if caller is upgrade authority.
- Close: archive code, free rent, transfer remaining balance.

### Core code requirements
- Enforce flat fees per tx type using governance-config values.
- Ensure program call fee is base fee only.
- Add rent charging for program accounts, based on stored code size.
- Add rent exemptions for system accounts (optional) and 1 KB free tier.

### RPC / API surface
- getProgram(id): code hash, owner, upgrade authority, size, created slot.
- getProgramStats(id): calls, unique callers, errors, last call slot.
- getPrograms(page, filters): list programs with metadata.
- getProgramCalls(id, page): recent call history.
- getProgramStorage(id): key list and sizes (optional or restricted).

### WebSocket
- programUpdates: deploy, upgrade, close events.
- programCalls: new call events for a program id.

### SDKs
- Rust: deploy_program, upgrade_program, call_program, close_program.
- JS/TS: same API with typed responses.
- Python: same API for agent workflows.

### Explorer
- Programs tab: list, filters, details.
- Program detail: code hash, owner, upgrade authority, calls, storage size, rent.
- Show fees paid per tx and rent status (due, paid, exemption).

## Marketplace Plan

### NFT primitives (core)
- Collection creation
- Mint NFT
- Transfer NFT

### Collection rules
- Default: permissionless minting.
- Optional overrides: creator-only or role-based minters.

### Marketplace economics
- Protocol-level fees only for collection and mint operations.
- Marketplace listing and sale fees handled at the marketplace program level.
- If needed, a marketplace take rate can be configured inside the marketplace program.

### Core code requirements
- Add collection and mint opcodes for system program.
- Ensure NFT fees are charged per the fee table.
- Persist collection metadata and token ownership in state.
- Maintain event logs for mint and transfer.

### RPC / API surface
- getCollection(address): metadata, creator, royalty, supply, rules.
- getNFT(collection, tokenId): owner, metadata URI/hash, mint slot/time.
- getNFTsByOwner(owner): tokens list.
- getNFTsByCollection(collection): tokens list.
- getNFTActivity(collection, tokenId): mint and transfer history.

### WebSocket
- nftMints: collection or global.
- nftTransfers: collection or token.
- marketListings and marketSales (from marketplace program events).

### SDKs
- Rust: create_collection, mint_nft, transfer_nft, get_nft, get_collection.
- JS/TS: same API with typed responses.
- Python: same API for agent workflows.

### Explorer
- Collections index and detail pages.
- NFT detail pages with ownership history.
- Address page: show owned NFTs and collections created.

## Wiring and Delivery

### Phase 1: Economic alignment
- Update runtime BASE_FEE to 100,000 shells.
- Move fee and rent parameters to governance configuration.
- Document fees and rent in docs and websites.

### Phase 2: Core primitives
- Implement collection and mint opcodes.
- Ensure fees are applied to new NFT txs.
- Add rent tracking and enforcement with free-tier.

### Phase 3: RPC/WS and SDKs
- Add all endpoints and subscriptions listed above.
- Implement SDK wrappers and typed responses.

### Phase 4: Explorer
- Display program and NFT data.
- Display fees, rent, and tx history accurately.

### Phase 5: Frontend wiring
- Programs: wire deploy/call/upgrade/close to real RPC.
- Marketplace: wire collections, NFTs, listings to real RPC/WS.

## Notes and Non-Goals
- No gas model or compute units.
- Tips are optional and separate from base fees.
- Multi-asset fee payment is out of scope; fees are in MOLT only.
- Marketplace wiring happens after core and API coverage is stable.
