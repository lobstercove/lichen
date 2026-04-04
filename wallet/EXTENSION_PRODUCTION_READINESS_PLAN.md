# Wallet And Extension Production Readiness Plan

## Fixed In This Pass

- Added safe-area-aware layout padding for the installed mobile PWA welcome and flow screens.
- Hardened the web wallet service worker so it skips unsupported request schemes instead of trying to cache them.
- Restored extension resource loading parity by allowing the popup/full pages to load the same external fonts and icon styles they already reference.
- Removed the popup's blocked remote QR script dependency and replaced it with a clean fallback state.
- Aligned wallet balance semantics so send and shield flows use spendable LICN instead of the global balance.
- Added extension balance breakdown rendering and shield initialization that now prompts for the wallet password before deriving shielded keys.

## Remaining Gaps

1. Popup QR rendering still needs a bundled local QR generator.
The popup no longer throws CSP errors, but production readiness needs a local QR implementation instead of a fallback message.

2. Full-page extension send flow still exposes assets beyond LICN without full transfer parity.
The dropdown can surface token balances, but the full-page send path is still primarily a native LICN transfer flow and needs either true token-send support or an explicit product restriction.

3. Full-page extension still relies on browser prompts in several sensitive flows.
Claim unstake, export, password change, and rename actions should all move to the same secure modal pattern used by the browser wallet and shield initialization.

4. Shielded data needs end-to-end validation against live RPC behavior.
The extension now derives the correct local shielded identity, but production sign-off should verify that `getShieldedNotes`, shield/unshield/transfer RPCs, and viewing-key UX are all aligned with the browser wallet's commitment-scanning model.

5. Popup and full-page layout parity is still partial.
The core balance and shield surfaces are closer now, but banners, modal polish, receive flow details, and condensed popup spacing still need a dedicated visual pass.

## RPC And WS Exercise Matrix

1. Verify `getBalance` parity across browser wallet, PWA, popup, and full-page extension for total, spendable, locked, staked, and rewards fields.
2. Verify WebSocket-driven refresh for account changes updates balances, activity, and shield data without reopening the UI.
3. Verify `sendTransaction` with exact-max LICN values leaves only the base fee buffer and never overstates available balance.
4. Verify staking RPCs update pending unstake and claimable states consistently across browser wallet and extension.
5. Verify shield RPCs for shield, unshield, transfer, note listing, and pool stats against a real funded account.
6. Verify bridge deposit polling and notification flows under both RPC reconnects and popup reopen cycles.

## Production Gate

1. Add a bundled local QR library or internal QR renderer for popup and full-page receive views.
2. Add targeted UI tests for spendable-vs-total balance rendering and MAX-send behavior.
3. Add a focused extension smoke test that checks fonts/icons load under MV3 CSP.
4. Add a shielded integration test that validates password-gated initialization plus note retrieval.
5. Run a manual mobile PWA pass on iOS and Android installed mode, including safe-area, keyboard, and modal behavior.