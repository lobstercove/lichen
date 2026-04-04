# Wallet Extension Release Plan

## Objective

Ship LichenWallet as an official browser extension that is easy to install, easy to verify, and credible enough for serious users to trust.

## Phase 1: Product Baseline

1. Finish parity between browser wallet, PWA, popup, and full-page extension for balances, receive flow, shield flow, and staking summaries.
2. Replace all remaining popup and full-page `prompt()` usage with secure in-app modals.
3. Bundle a local QR generator so the extension has no blocked remote script dependencies.
4. Define supported scope explicitly for v1: native LICN transfers, receive, identity, staking, shield, bridge, and read-only NFT views.

## Phase 2: Release Engineering

1. Track the entire extension source tree in git and treat it like a first-class product, not a local artifact.
2. Add a reproducible packaging command that emits a versioned zip from `wallet/extension/`.
3. Add CI checks for manifest validity, extension audit tests, wallet audit tests, and a packaging smoke build.
4. Version the extension from a single release source so manifest version, changelog, and release artifact names stay aligned.
5. Publish signed release artifacts from GitHub Releases with checksums.

## Phase 3: Store Presence

1. Publish to Chrome Web Store first.
2. Publish the same reviewed build to Microsoft Edge Add-ons.
3. Prepare a Firefox track only after manifest and API compatibility are verified.
4. Create an official extension landing page on `wallet.lichen.network` with direct install buttons, version history, screenshots, permissions explanation, and checksum links.

## Phase 4: Credibility Layer

1. Add a clear permissions page explaining why `storage`, `tabs`, `notifications`, and network access are required.
2. Publish an extension security model covering key storage, password protection, trusted RPC split, and provider approval flow.
3. Link the extension repo path, audit tests, and security contact directly from the extension listing and the landing page.
4. Add reproducible install instructions for Chrome, Edge, and developer-mode sideloading.
5. Add release notes for every version and a visible support channel for bug reports.

## Phase 5: Install Experience

1. Make the landing page detect browser family and show the correct store button first.
2. Provide a fallback signed zip for manual enterprise or air-gapped installation.
3. Add a one-page first-run guide inside the extension covering create, import, backup, receive, and lock behavior.
4. Add a migration guide from browser wallet/PWA to extension for existing users.

## Production Gate Before Store Submission

1. Manual install test on Chrome and Edge stable.
2. Real-device PWA safe-area test on iOS and Android.
3. Real-node wallet smoke pass for popup and full-page views.
4. Provider approval flow test on a demo dapp page.
5. Packaging artifact review to ensure no ignored or local-only files leak into the submission zip.

## Definition Of Done

- The extension source is tracked in git.
- CI builds a store-ready zip on tagged releases.
- The Chrome Web Store listing is live.
- The wallet site has an official install page.
- A new user can install, verify authenticity, create/import a wallet, and complete a basic transaction flow without manual patching or developer-mode guesswork.