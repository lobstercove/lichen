# LichenWallet Extension Scaffold

This directory is the implementation workspace for the Brave/Chrome extension build.

## Reference Plan
- See `../docs/EXTENSION_FULL_COVERAGE_PLAN.md` for the full architecture, feature coverage, and phased migration plan.

## Initial Structure
- `src/popup` — extension popup UI
- `src/pages` — full-page extension routes (advanced views, approvals)
- `src/background` — MV3 service worker
- `src/content` — content script and in-page provider bridge
- `src/core` — shared wallet/crypto/rpc/state services
- `src/styles` — copied/adapted style system
- `src/assets` — extension icons and static assets

## Next Step
Begin Phase 0 by adding:
1. `manifest.json`
2. popup entry files
3. background service worker entry

## Load in Brave/Chrome
1. Open `chrome://extensions` (or `brave://extensions`)
2. Enable **Developer mode**
3. Click **Load unpacked**
4. Select the `wallet/extension` directory

## Package For Release
1. Run `npm run validate-wallet-extension-release`
2. Run `npm run package-wallet-extension`
3. Collect the generated files from `dist/wallet-extension/`

Release output:
- `LichenWallet-extension-v<version>.zip` — runtime extension ZIP for browser-store review
- `LichenWallet-extension-store-submission-v<version>.zip` — listing copy, permissions rationale, and submission checklist
- `latest.json` — release metadata for install pages and automation
- `SHA256SUMS` — checksums for release verification

## Auto Update Model
- Chrome Web Store and Edge Add-ons provide automatic updates after publication.
- Unpacked or direct ZIP installs are manual-update only.
- `latest.json` is generated so the wallet site can advertise the current release consistently.

## Current Scaffold Status
- MV3 manifest: ready
- Popup wallet flow: create/import/unlock/dashboard + assets/receive/activity ready
- Popup import parity: seed phrase, private key (hex), and JSON keystore import paths
- Popup send flow: password-gated build/sign/broadcast attempt wired
- Popup settings/security panel: auto-lock timeout, password-gated private key export, password-gated seed phrase view, JSON keystore export, and secure copy output
- Popup export parity: password-gated private key and seed phrase download actions (txt) in addition to secure copy and JSON keystore export
- Popup network settings parity: custom RPC endpoints for mainnet/testnet/local networks (used by popup RPC actions)
- Bridge endpoint parity: configurable custody endpoints (mainnet/testnet/local) now used by bridge deposit/status flows
- Popup display settings parity: currency + decimal precision controls affecting popup balance rendering
- Popup password management parity: change-password flow with encrypted key/mnemonic re-encryption
- Auto-lock scheduling: wired in popup runtime
- Options/full-page advanced dashboard: identity/staking/bridge/NFT snapshots wired
- Full-page bridge deposit: request address + live status polling wired
- Full-page staking actions: stake/unstake transaction flows wired
- Full-page identity actions: register identity + add skill flows wired
- Full-page detail routes: dedicated identity details page and NFT details page
- Full-page settings route: wallet management, network/RPC settings, display/security controls, password change, and export controls
- Full-page export parity: password-gated private key and seed phrase download actions (txt)
- dApp permission management: list/revoke approved provider origins from settings route
- Identity detail actions: register, add skill, agent type, vouch, endpoint/availability/rate updates
- Identity name lifecycle actions: register/renew/transfer/release `.lichen` names
- Identity safety parity: core service validation hardening for names/addresses/endpoints/rates/password and transaction preflight spendability checks
- Route-level UX validation hardening: stricter input checks, address/url validation, and action status feedback on home/identity pages
- Bridge UX parity upgrades: mapped deposit status lifecycle text, adaptive polling cadence, and copy-address action in full-page dashboard
- NFT detail UX upgrades: per-item copy mint action, marketplace launch actions, and explicit load/status feedback
- Background WebSocket runtime manager: connect/reconnect/status + sync message endpoints wired
- Core endpoint parity: identity/staking/bridge/NFT/provider/ws modules now resolve user-configured RPC (and derived WS) endpoints from extension state
- Background service worker: ready
- Background provider origin hardening: falls back to parsing `sender.url` when `sender.origin` is unavailable
- Content/in-page provider bridge: request + approval queue wired
- Core state + lock services: ready

## Provider Status
- `licn_getProviderState`: supported
- `licn_isConnected`: supported
- `licn_chainId`: supported
- `licn_network`: supported
- `licn_version`: supported
- `licn_accounts`: supported
- `licn_requestAccounts`: supported with approval page flow
- `licn_connect`: supported (alias to account request flow)
- `licn_disconnect`: supported (origin-scoped)
- `licn_getPermissions`: supported (origin-scoped permission view)
- `wallet_getPermissions`: supported (compat alias)
- `wallet_revokePermissions`: supported (compat alias)
- `licn_getBalance`: supported
- `licn_getAccount`: supported
- `licn_getLatestBlock`: supported
- `licn_getTransactions`: supported
- `licn_signMessage`: supported with approval + password flow
- `licn_signTransaction`: supported with approval + password flow
- `licn_sendTransaction`: supported with approval + password + broadcast flow
- `eth_chainId`: supported (compat alias)
- `net_version`: supported (compat alias)
- `eth_coinbase`: supported (compat alias)
- `eth_accounts`: supported (compat alias)
- `eth_requestAccounts`: supported (compat alias)
- `personal_sign`: supported (compat alias)
- `eth_sign`: supported (compat alias)
- `eth_signTransaction`: supported (compat alias)
- `eth_sendTransaction`: supported (compat alias)
- `eth_getBalance`: supported (compat alias, hex quantity response)
- `eth_getTransactionCount`: supported (compat alias, hex quantity response)
- `eth_blockNumber`: supported (compat alias)
- `eth_getCode`: supported (compat alias)
- `eth_estimateGas`: supported (compat alias)
- `eth_gasPrice`: supported (compat alias)
- `web3_clientVersion`: supported (compat alias)
- `net_listening`: supported (compat alias)
- `wallet_switchEthereumChain`: supported (compat alias mapped to extension network selection)
- `wallet_addEthereumChain`: supported (compat alias mapped to extension RPC settings)
- `wallet_watchAsset`: supported (compat alias)
- Provider events: `connect`, `disconnect`, `accountsChanged`, `chainChanged` emitted from content/inpage bridge
- Approval page metadata: network/chain/account/lock-state context now shown
- Provider compatibility hardening: method alias normalization and flexible request input forms (`request({method, params})` or `request(method, params)`)
- Provider approval parity fix: pending approval finalization now resolves normalized method aliases correctly
- Provider compatibility expansion: common Ethereum-style aliases (`eth_accounts`, `eth_requestAccounts`, `personal_sign`, `eth_sign`, `eth_signTransaction`, `eth_sendTransaction`) mapped to Lichen flows
- Provider permissions/connect compatibility: `licn_connect`, `licn_getPermissions`, `wallet_getPermissions`, and `wallet_revokePermissions` bridged to extension origin permissions model
- Provider network compatibility: `eth_chainId`, `net_version`, and `eth_coinbase` compatibility paths added
- Approval queue UX: pending requests list + picker when approval page opens directly
- Approval UX hardening: alias-aware signing password requirement, stale-request button disablement, and escaped request rendering
- Provider pending lifecycle hardening: pending approval TTL timeout + bounded queue size protection
- Provider lock awareness: `licn_accounts` hides accounts while locked and `licn_requestAccounts` returns explicit locked error
- In-page EVM bridge compatibility: `window.ethereum` mirror (non-MetaMask) backed by Lichen provider methods/events

## Current Limitations
- No known parity blockers remain for wallet web-surface feature coverage in this release.
