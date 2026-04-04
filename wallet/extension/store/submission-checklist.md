# Store Submission Checklist

## Before Packaging

1. Confirm `wallet/extension/manifest.json` version matches the intended `wallet-extension-v*` tag.
2. Run `npm run validate-wallet-extension-release`.
3. Run `npm run package-wallet-extension`.

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
4. Add screenshots and promotional assets from the extension marketing pipeline.
5. Publish only after popup, full-page, extension-install, and installed-PWA smoke passes complete.

## Microsoft Edge Add-ons

1. Upload the same runtime ZIP.
2. Reuse the listing copy and permissions rationale.
3. Validate the same release version and checksum.

## Post Publication

1. Link the live store URLs from the wallet install page.
2. Update any website download buttons to prefer store installs.
3. Treat browser-store publication as the automatic-update channel for production users.