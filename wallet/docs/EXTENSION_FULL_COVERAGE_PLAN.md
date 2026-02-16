# MoltWallet Browser Extension — Full Coverage Implementation Plan

## Document Purpose
This document defines the complete plan to ship **MoltWallet as a Brave/Chrome extension** while preserving:

- Existing MoltWallet visual identity (colors, typography, components)
- Existing wallet feature set and user flows
- Existing chain integrations (RPC/WebSocket/bridge/identity/staking)
- Existing lightweight vanilla JS approach

It also adds extension-native architecture (Manifest V3, service worker, storage/security boundaries, dApp connectivity).

---

## Implementation Progress Snapshot (Feb 15, 2026)

### Completed
- MV3 extension scaffold (`manifest`, popup, background, options page, content bridge)
- Popup core wallet lifecycle: create/import/unlock/lock/switch wallet
- Popup import parity pass: seed/private-key/JSON keystore import support
- Popup dashboard basics: balance, assets, receive, activity, network selector
- Popup send flow: password-gated transaction build/sign/broadcast attempt
- Popup settings/security parity pass: auto-lock timeout controls + password-gated key/seed export + JSON keystore export
- Popup export parity pass: password-gated private key and seed phrase download controls (txt)
- Popup network settings parity pass: custom RPC endpoint inputs applied to popup RPC calls
- Bridge settings parity pass: configurable custody endpoint inputs applied to bridge request/status flows
- Popup display settings parity pass: currency/decimals controls connected to popup rendering
- Popup password management parity pass: change-password re-encryption flow
- Full-page advanced dashboard: identity/staking/bridge/NFT snapshots
- Full-page bridge deposit controls: request deposit address + status polling
- Full-page staking actions: stake/unstake flows
- Full-page identity actions: register identity + add skill
- Full-page identity details route
- Full-page NFT details route
- Full-page settings route with wallet/security/network/export management controls
- Full-page export parity pass: password-gated private key and seed phrase download controls (txt)
- Full-page permission management controls for approved dApp origins (list/revoke)
- Identity module actions in detail route (register, add skill, vouch, agent config)
- Identity name lifecycle actions in detail route (register/renew/transfer/release)
- Identity safety parity pass: centralized service validation for names/addresses/endpoints/rates/password plus preflight spendability checks
- Full Ed25519 signature compatibility parity in extension crypto service
- Route-level UX validation pass: stricter action input guards and clearer status/error feedback
- Bridge interaction parity pass: status lifecycle mapping + adaptive polling + copy address control
- NFT detail parity pass: item actions (copy mint/market launch) + status messaging
- Background WebSocket runtime manager with reconnect and status APIs
- Background provider origin hardening: sender origin now resolved from origin/url fallback for stable permission matching
- Provider approval queue with dedicated approval page
- Provider methods live:
   - `molt_getProviderState`
   - `molt_isConnected`
   - `molt_chainId`
   - `molt_network`
   - `molt_version`
   - `molt_accounts`
   - `molt_requestAccounts`
   - `molt_disconnect`
   - `molt_getBalance`
   - `molt_getAccount`
   - `molt_getLatestBlock`
   - `molt_getTransactions`
   - `molt_signMessage`
   - `molt_signTransaction`
   - `molt_sendTransaction`
   - `eth_accounts`
   - `eth_requestAccounts`
   - `eth_getBalance`
   - `eth_getTransactionCount`
   - `eth_chainId`
   - `net_version`
   - `eth_coinbase`
   - `eth_blockNumber`
   - `eth_getCode`
   - `eth_estimateGas`
   - `eth_gasPrice`
   - `web3_clientVersion`
   - `net_listening`
   - `wallet_getPermissions`
   - `wallet_revokePermissions`
   - `wallet_switchEthereumChain`
   - `wallet_addEthereumChain`
   - `wallet_watchAsset`
 - Provider event parity pass: `connect`/`disconnect`/`accountsChanged`/`chainChanged` emission wired in bridge
 - Approval page metadata upgrades: origin request context now includes chain/network/account/lock state
- Extension-wide endpoint parity pass: core services now use configured RPC endpoints (and WS derived from configured RPC)
- Provider compatibility hardening: alias method normalization + flexible request input normalization for dApp integration styles
- Provider approval parity fix: pending request finalization now uses normalized method aliases
- Provider compatibility expansion: Ethereum-style alias coverage for account/sign/send request paths
- Provider permissions/connect compatibility pass: connect/getPermissions/revokePermissions paths mapped to origin approval model
- Provider network compatibility pass: chainId/netVersion/coinbase alias handling for Ethereum-style dApp calls
- Approval UX parity pass: pending request listing and direct picker flow in approval page
- Approval UX safety pass: alias-aware signing checks, stale-request decision disablement, and escaped request rendering
- Provider lifecycle hardening pass: pending request TTL/queue bounds and timeout finalization behavior
- In-page compatibility pass: `window.ethereum` mirror surfaced from Molt provider bridge
- Provider security parity pass: lock-aware account exposure for `accounts`/`requestAccounts`

### In Progress / Next
- No open parity workstreams for wallet web-surface feature coverage in this release snapshot.

---

## Scope and Principles

### Primary Goal
Convert the current web wallet into a production-quality browser extension with parity on core behavior and strong UX fit for extension surfaces.

### Non-Negotiables
1. **Keep design system intact**
   - Use current tokens and themes from `shared-base-styles.css`, `shared-theme.css`, and `wallet.css`
   - Preserve orange MoltChain visual language
   - Preserve component hierarchy and style behavior
2. **Keep core flows intact**
   - Create/import wallet
   - Lock/unlock
   - Send/receive/deposit
   - Assets/activity/NFTs/staking/identity
   - Settings/export/backup/security
3. **Extension UX adaptation only where needed**
   - Fit popup dimensions, constraints, and navigation norms
   - Offload large or advanced views to full-page extension route/options page when required
4. **Security-first migration**
   - Minimize key exposure
   - Keep private key operations isolated
   - Remove dependence on plain web assumptions (global localStorage, unrestricted remote scripts)

### Explicit Out-of-Scope for Initial Extension Release
- New branding or design themes
- New feature invention beyond current product behavior
- Protocol-level changes to MoltChain RPC/WS

---

## Current Wallet Baseline (Exploration Summary)

### UI Surfaces Already Implemented
- Welcome screen carousel + onboarding actions
- Create wallet wizard (password → seed phrase → confirm)
- Import wallet (seed/private key/JSON)
- Dashboard with tabs:
  - Assets
  - Activity
  - NFTs
  - Staking
  - Identity (MoltyID)
- Modals:
  - Send
  - Receive/Deposit
  - Settings
  - Password confirmation / confirm dialogs
- Export and wallet management controls

### Functional Systems Already Implemented
- Wallet lifecycle management and encryption
- RPC client integration (balance/account/tx/block/token contracts)
- WebSocket live updates (balance + bridge subscriptions)
- Bridge deposit polling + status updates
- NFT loading paths and empty states
- Staking data views and actions
- Identity module with registration/profile/name/skills/vouches/agent metadata
- Network switching and endpoint management
- Auto-lock timer and security settings

### Current Technical Constraints to Address for Extensions
- Inline scripts in HTML (not allowed by MV3 CSP)
- CDN-hosted script dependencies (not ideal/allowed for MV3 packaging)
- Direct `localStorage` reliance (should migrate to extension storage model)
- Single-page web assumptions rather than popup/service-worker lifecycle constraints

---

## Target Extension Architecture (Manifest V3)

## High-Level Components
1. **Popup UI**
   - Primary quick actions and most common wallet tasks
   - Lightweight route-based view system
2. **Extension Full Page (Options or internal page)**
   - Larger/advanced views (Identity, detailed staking, extended settings, long activity lists)
3. **Background Service Worker**
   - Secure state orchestration
   - Transaction request queue/approval flow
   - dApp provider request handling
   - Notifications and alarm-driven lock timers
4. **Content Script + Inpage Provider Bridge**
   - dApp connectivity surface (wallet provider API)
   - Message relay between page context and extension runtime
5. **Shared Core Modules**
   - Crypto utilities
   - RPC/WS transport services
   - wallet state store + selectors
   - schema/version migrations

---

## Directory Plan (inside `wallet/extension`)

```text
extension/
  manifest.json
  README.md
  src/
    popup/
      popup.html
      popup.css
      popup.js
    pages/
      home.html
      home.css
      home.js
      approve.html
      approve.js
    background/
      service-worker.js
    content/
      content-script.js
      inpage-provider.js
    core/
      state-store.js
      crypto-service.js
      rpc-service.js
      ws-service.js
      tx-service.js
      permissions-service.js
      lock-service.js
      bridge-service.js
      nft-service.js
      staking-service.js
      identity-service.js
    ui/
      components/
      routes/
      modal-manager.js
    styles/
      shared-base-styles.css
      shared-theme.css
      wallet.css
      extension-overrides.css
    assets/
      icon-16.png
      icon-32.png
      icon-48.png
      icon-128.png
```

Notes:
- Keep existing CSS mostly unchanged; add small extension overrides only for popup sizing/scrolling.
- Reuse existing JS logic by extracting pure modules from `js/wallet.js` and wiring extension runtime adapters.

---

## Manifest V3 Plan

### Required Extension Pages
- `action.default_popup`: popup entry
- `options_page` (or internal routed page): advanced screens
- `background.service_worker`: core runtime coordinator

### Minimum Permissions (phase 1)
- `storage`
- `alarms`
- `notifications`
- `clipboardWrite` (if needed for explicit copy actions)

### Host Permissions
- User-configurable Molt RPC/WS/custody endpoints
- Default:
  - `https://rpc.moltchain.network/*`
  - `https://testnet-rpc.moltchain.network/*`
  - local dev endpoints where needed

### CSP and Packaging Rules
- Remove inline scripts; all JS in packaged files
- Vendor third-party libs locally (no CDN runtime dependency)
- Use MV3-compliant script loading only

---

## Full Feature Coverage Matrix

## A) Onboarding and Wallet Lifecycle
1. Welcome carousel and CTA actions
2. Create wallet 3-step flow
3. Import wallet (seed/private key/JSON)
4. Lock/unlock and logout behavior
5. Multi-wallet selector and switch

**Extension adaptation:**
- Keep same flow logic/UI language
- Ensure screen transitions fit popup height; move overflow sections to full page

**Acceptance criteria:**
- Every existing path from welcome to unlocked dashboard works in extension without style drift

## B) Security and Key Management
1. Mnemonic generation and validation
2. Key derivation and address generation
3. AES-GCM encrypted key storage
4. Password-gated exports
5. Auto-lock timer

**Extension adaptation:**
- Use `chrome.storage.local` for encrypted state
- Keep decrypted key material in memory only and short-lived
- Lock state enforced by background + alarm events

**Acceptance criteria:**
- No plaintext private key persisted
- Auto-lock still works with popup close/reopen

## C) Network/RPC/WS
1. Network select and endpoint settings
2. RPC calls for balance/account/token/tx
3. WebSocket subscriptions for balance + bridge events

**Extension adaptation:**
- Move persistent connectivity management to background worker
- Popup reads snapshot state and subscribes via runtime messaging

**Acceptance criteria:**
- Balance and asset refresh behavior matches current wallet
- Reconnect behavior survives popup lifecycle interruptions

## D) Send / Receive / Deposit
1. Send modal with token select, max amount, fee display
2. Receive addresses (Base58 + EVM) and QR
3. Deposit tab + bridge source chain flows + status polling

**Extension adaptation:**
- Keep modal UX in popup for common use
- Open full-page transaction view when complex forms exceed popup constraints

**Acceptance criteria:**
- Successful send flow from extension UI
- Deposit status progression visible and persisted

## E) Assets / Activity / NFTs / Staking
1. Asset list rendering and token balances
2. Activity timeline and pagination behavior
3. NFT list/empty states/details hooks
4. Staking panel and modal flows

**Extension adaptation:**
- Popup shows concise snapshot
- “View more” navigates to full-page extension view for long lists

**Acceptance criteria:**
- Same data categories available; no feature removed

## F) MoltyID Identity
1. Identity onboarding and profile render
2. Name registration/renewal/transfer/release
3. Skills, vouches, achievements, agent metadata
4. Contract call signing flow

**Extension adaptation:**
- Full identity module hosted in extension full page (best fit for complexity)
- Popup shows compact identity summary + deep-link

**Acceptance criteria:**
- All identity operations executable via extension with same validation and signing protections

## G) Settings / Backup / Account Management
1. Export private key / JSON / mnemonic
2. Network configuration
3. Display and decimal settings
4. Change password
5. Rename/delete wallet and clear history

**Extension adaptation:**
- Sensitive exports always require re-auth in approval panel
- Downloads and clipboard use extension-safe APIs/events

**Acceptance criteria:**
- Same management controls available with explicit safety prompts

## H) dApp Connectivity (Extension-Required)
1. Inject provider object in page context
2. Request account access and permissions
3. Sign transaction/message requests
4. Approval UI for each sensitive action
5. Session and origin permission management

**Acceptance criteria:**
- dApp can connect/sign through MoltWallet extension with explicit user approvals

---

## Data and Storage Plan

## Storage Layers
1. **Persistent encrypted state** → `chrome.storage.local`
2. **Ephemeral unlocked session** → background memory
3. **UI cache** → per-view state in popup/page

## Schema Versioning
- Add `schemaVersion` to wallet state
- Build migration functions from current `moltWalletState` structure
- One-time migration path from old web `localStorage` backup import

## Key Handling Rules
- Never persist decrypted keys
- Password prompt before sensitive operations
- Zeroize in-memory buffers when practical after use

---

## UI/Design Preservation Plan

## Keep As-Is
- Color variables (`--primary`, `--accent`, etc.)
- Typography (`Inter`, `JetBrains Mono`)
- Button, card, modal, tabs, form components
- Existing icon language and spacing rhythm

## Extension-Specific Adjustments
- Popup width/height and scroll containers
- Sticky mini-header for navigation in constrained viewport
- Reduced animation intensity where popup lifecycle makes transitions jarring

## Design QA Gate
- Side-by-side comparison snapshots (web wallet vs extension)
- Token-level color diff checks
- Component state parity checklist (hover/focus/active/error)

---

## Security and Compliance Checklist

1. Remove inline script execution from HTML
2. Bundle all third-party dependencies locally
3. Enforce least-privilege permissions in manifest
4. Restrict host permissions to required RPC/custody domains
5. Add anti-phishing account approval context (origin + favicon + domain warning)
6. Require re-auth for export/private actions
7. Add tx preview and human-readable summary before signature
8. Add nonce/replay and chain/network validation in signing pipeline

---

## Implementation Phases

## Phase 0 — Foundation and Scaffolding
- Create extension directory structure
- Add MV3 manifest skeleton
- Split popup/page/background/content entry points
- Copy current styles and assets

**Exit criteria:** extension loads in Brave/Chrome dev mode with placeholder UI

## Phase 1 — Core Wallet Runtime Migration
- Extract wallet state, crypto, RPC logic into shared core modules
- Implement storage adapter (`localStorage` → extension storage)
- Implement lock service + alarms

**Exit criteria:** create/import/unlock + basic dashboard balances work in extension

## Phase 2 — Feature Parity Surface
- Port send/receive/deposit flows
- Port assets/activity/NFT/staking rendering
- Port settings and export paths

**Exit criteria:** all existing web wallet user-visible features operate in extension UI

## Phase 3 — Identity + Advanced Views
- Port full identity module to extension full page
- Ensure all contract call modals and flows work

**Exit criteria:** complete MoltyID functionality with signed transactions

## Phase 4 — dApp Provider + Approval UX
- Inject provider
- Add request routing and approval dialogs
- Implement origin permissions and session management

**Exit criteria:** dApp connect/sign flows function end-to-end

## Phase 5 — Hardening and Release
- Performance pass
- Security review pass
- Regression + compatibility tests (Chrome + Brave)
- Build release packaging and docs

**Exit criteria:** release candidate extension package

---

## Test Plan (Full Coverage)

## Functional
- Onboarding create/import flows
- Wallet switching and lock/unlock lifecycle
- Send/receive/deposit happy + failure paths
- Balance/activity refresh and reconnect handling
- NFT/staking/identity operations

## Security
- Wrong password and brute-force throttling behavior
- Export flow re-auth checks
- Storage inspection validates encryption-only at rest
- Provider request origin checks and approval enforcement

## UX
- Popup viewport behavior (no clipped critical controls)
- Keyboard navigation and focus handling
- Visual parity with current design system

## Compatibility
- Chrome stable + Brave stable
- Local/testnet/mainnet endpoint switching

---

## Risks and Mitigations

1. **MV3 service worker suspension interrupts realtime behavior**
   - Mitigation: resilient reconnect strategy + state refresh on popup open
2. **Large feature set exceeds popup constraints**
   - Mitigation: split advanced operations into extension full page
3. **Identity/staking complexity increases regression risk**
   - Mitigation: phased parity rollout + module isolation + focused integration tests
4. **External script dependencies blocked by CSP**
   - Mitigation: vendor all dependencies and remove inline scripts

---

## Deliverables

1. Full extension scaffold under `wallet/extension`
2. MV3 manifest and runtime architecture
3. Feature-parity migration implementation
4. Provider bridge and approvals
5. Security hardening checklist completion
6. Test checklist and release build notes

---

## Definition of Done

MoltWallet extension is considered complete when:

1. All existing web-wallet features are available with no intentional removals
2. Visual design remains consistent with current MoltWallet branding and components
3. Extension-specific architecture is MV3-compliant and secure
4. dApp connect/sign requests are supported through explicit approval UX
5. Chrome and Brave manual QA pass for core scenarios
6. Documentation for install, dev, test, and release is complete

---

## Next Build Step (after this plan)

Start with **Phase 0 + Phase 1** implementation in `wallet/extension`, prioritizing:

1. MV3 manifest + popup/page/background entry points
2. storage adapter and lock service
3. create/import/unlock and dashboard balance parity
