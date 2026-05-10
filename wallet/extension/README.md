# LichenWallet Extension

This directory contains the Brave/Chrome MV3 extension build for LichenWallet. The extension shares wallet behavior with the web wallet while adding popup, full-page, background service worker, content script, in-page provider, approval, and store-submission surfaces.

## Directory Structure

- `manifest.json` - MV3 extension manifest
- `src/popup` - extension popup UI
- `src/pages` - full-page wallet, approval, settings, identity, NFT, and home routes
- `src/background` - MV3 service worker
- `src/content` - content script and in-page provider bridge
- `src/core` - shared wallet, crypto, RPC, restriction, provider, and state services
- `src/styles` - extension style system
- `store` - Chrome Web Store / Edge Add-ons submission docs
- `shared` - bundled shared browser dependencies

## Load In Brave Or Chrome

1. Open `chrome://extensions` or `brave://extensions`.
2. Enable **Developer mode**.
3. Click **Load unpacked**.
4. Select the `wallet/extension` directory.

## Release Validation

Run the release gate before packaging:

```bash
npm run test-wallet-docs
npm run test-wallet
npm run test-wallet-extension
npm run test-frontend-assets
node tests/test_frontend_trust_boundaries.js
npm run validate-wallet-extension-release
```

Then package the reviewed artifact:

```bash
npm run package-wallet-extension
```

Release output is written to `dist/wallet-extension/`:

- `LichenWallet-extension-v<version>.zip` - runtime extension ZIP for browser-store review
- `LichenWallet-extension-store-submission-v<version>.zip` - README, manifest snapshot, store listing, permissions rationale, and submission checklist
- `latest.json` - release metadata for install pages and automation
- `SHA256SUMS` - checksums for release verification

## Auto Update Model

- Chrome Web Store and Edge Add-ons provide automatic updates after publication.
- Unpacked or direct ZIP installs are manual-update only.
- `latest.json` is generated so the wallet site can advertise the current release consistently.

## Restriction-Governance Safety

Restriction-governance safety is part of the release gate:

- Restriction reads use trusted Lichen RPC endpoints through `restriction-service.js`, not user-configured custom RPC endpoints.
- Popup and full-page views render account/native consensus restriction banners and LICN restriction badges.
- Direct extension sends call trusted `canTransfer` before signing and before private key decryption.
- Provider `licn_signTransaction` and `licn_sendTransaction` requests run restriction preflight before approval and enforce the same preflight before private key decryption.
- The approval page renders passed, warning, and blocked restriction states; blocked preflight cannot be approved.
- `getIncidentStatus` can surface incident warnings, but incident state is not mutable from the provider.
- dapps cannot suppress extension warnings or bypass wallet-side restriction decisions.

## Dapp Restriction Preflight

Dapp restriction preflight is read-only. The in-page provider exposes:

- `lichen_getRestrictionStatus`
- `lichen_canTransfer`
- `lichen_getContractLifecycleStatus`

These helpers let dapps preflight user experience, but they do not create, lift, extend, approve, or execute restrictions. The provider does not expose restriction mutation builders, admin-token paths, or raw-submit governance paths.

## Current Status

- MV3 manifest: ready
- Popup wallet flow: create/import/unlock/dashboard, assets, receive, activity, send, settings, bridge, staking, shield, identity, and NFT surfaces wired
- Popup import parity: seed phrase, private key hex, and JSON keystore paths
- Popup send flow: password-gated build/sign/broadcast wired with restriction preflight
- Popup settings/security panel: auto-lock timeout, password-gated export, JSON keystore export, custom RPC settings, and secure copy output
- Full-page dashboard: identity, staking, bridge, NFT, send, receive, settings, and detail routes wired
- Background service worker: ready
- Background WebSocket runtime manager: connect/reconnect/status and sync message endpoints wired
- Content/in-page provider bridge: request, approval queue, origin permission, pending-request TTL, and finalized result flow wired
- Core endpoint parity: identity/staking/bridge/NFT/provider/ws modules resolve user-configured RPC where allowed
- Trusted-control split: bridge routing, signed metadata, LichenID resolution, and restriction checks stay pinned to trusted endpoints
- Restriction-governance safety: trusted restriction status reads, popup/full-page restriction banners, direct-send `canTransfer` preflight, provider sign/send preflight before key decryption, and approval-page blocking warnings wired
- Dapp restriction preflight: read-only `lichen_getRestrictionStatus`, `lichen_canTransfer`, and `lichen_getContractLifecycleStatus` provider methods wired without exposing restriction mutation builders
- Core state and lock services: ready

## Provider Status

Supported Lichen provider methods:

- `licn_getProviderState`
- `licn_isConnected`
- `licn_chainId`
- `licn_network`
- `licn_version`
- `licn_accounts`
- `licn_requestAccounts`
- `licn_connect`
- `licn_disconnect`
- `licn_getPermissions`
- `licn_getBalance`
- `licn_getAccount`
- `licn_getLatestBlock`
- `licn_getTransactions`
- `lichen_getRestrictionStatus`
- `lichen_canTransfer`
- `lichen_getContractLifecycleStatus`
- `licn_signMessage`
- `licn_signTransaction`
- `licn_sendTransaction`

Supported compatibility aliases:

- `wallet_getPermissions`
- `wallet_revokePermissions`
- `eth_chainId`
- `net_version`
- `eth_coinbase`
- `eth_accounts`
- `eth_requestAccounts`
- `personal_sign`
- `eth_sign`
- `eth_signTransaction`
- `eth_sendTransaction`
- `eth_getBalance`
- `eth_getTransactionCount`
- `eth_blockNumber`
- `eth_getCode`
- `eth_estimateGas`
- `eth_gasPrice`
- `web3_clientVersion`
- `net_listening`
- `wallet_switchEthereumChain`
- `wallet_addEthereumChain`
- `wallet_watchAsset`

Provider events:

- `connect`
- `disconnect`
- `accountsChanged`
- `chainChanged`

## Release Notes For Reviewers

- Permission explanations live in `store/permissions-justification.md`.
- Store submission checks live in `store/submission-checklist.md`.
- Internal release and production-readiness gates live in `docs/internal/wallet/`.
- The provider restriction surface is query-only; dapps cannot suppress extension warnings.
- Every restriction mutation remains a signed governed transaction outside the extension provider.
