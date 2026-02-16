# MoltWallet Extension — Release Notes

Date: 2026-02-15
Release: Extension Parity + Compatibility Completion

## Highlights
- Full wallet flow available in popup and full-page mode via **Open Full**.
- Branding standardized to MoltWallet logo across popup header, toolbar/action icon, extension icon, and notifications.
- Provider compatibility expanded for Molt + Ethereum-style dApp calls.
- Approval queue and request lifecycle hardened (timeouts, stale-state handling, normalized method finalization).
- Security/settings parity completed (password change, exports/downloads, RPC/custody config, lock controls).

## What Changed
- Open Full now opens `src/popup/popup.html?mode=full` for wallet-like experience instead of advanced-only dashboard.
- Added full-page popup rendering mode (`body.full-page`) for larger layout and hidden duplicate Open Full button.
- Replaced emoji header branding with logo image.
- Manifest/action icons switched to `MoltWallet_Logo_256.png`.
- Notification icon switched to logo.
- Provider method surface expanded and normalized:
  - connect/accounts/sign/send aliases
  - chain/network helpers (`eth_chainId`, `net_version`, `eth_blockNumber`, etc.)
  - permissions helpers (`wallet_getPermissions`, `wallet_revokePermissions`)
  - network management (`wallet_switchEthereumChain`, `wallet_addEthereumChain`)

## Compatibility Coverage
- Popup wallet lifecycle: create/import/unlock/lock/send/receive/assets/activity/settings
- Full-page routes: home/identity/nfts/settings/approve
- dApp bridge: `window.moltwallet` + `window.ethereum` compatibility mirror
- Automated provider check: 13/13 pass (`docs/provider_compat_check.mjs`)

## Upgrade Notes
- Reload extension once from browser extensions page after pulling latest files.
- If old icon cache persists, remove and re-load unpacked extension to refresh toolbar icon assets.
