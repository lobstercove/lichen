# Store Submission Checklist

## Before Packaging

1. Confirm `wallet/extension/manifest.json` version matches the intended `wallet-extension-v*` tag.
2. Run `npm run validate-wallet-extension-release`.
3. Run `npm run package-wallet-extension`.
4. Confirm the generated store submission bundle includes `README.md`, `manifest.json`, `store/permissions-justification.md`, `store/submission-checklist.md`, and release checksums.
5. Confirm the wallet restriction-governance audit coverage is passing, including trusted restriction RPC reads, signing preflight blocking, approval-page warning display, and the read-only dapp provider methods.

## Submission Bundle Contents

1. Runtime ZIP
2. Store listing copy
3. Permissions justification
4. Auto update policy
5. Current manifest snapshot
6. Release checksums

## Chrome Web Store

1. Upload the runtime ZIP.
2. Use `store-listing.md` for description fields.
3. Use `permissions-justification.md` when answering review questions.
4. In review notes, identify `lichen_getRestrictionStatus`, `lichen_canTransfer`, and `lichen_getContractLifecycleStatus` as read-only provider methods used by dapps to preflight restriction state.
5. Add screenshots and promotional assets from the extension marketing pipeline.
6. Publish only after popup, full-page, extension-install, installed-PWA, and restriction-warning smoke passes complete.

## Microsoft Edge Add-ons

1. Upload the same runtime ZIP.
2. Reuse the listing copy and permissions rationale.
3. Reuse the same restriction-governance review notes from the Chrome submission.
4. Validate the same release version and checksum.

## Post Publication

1. Link the live store URLs from the wallet install page.
2. Update any website download buttons to prefer store installs.
3. Treat browser-store publication as the automatic-update channel for production users.
