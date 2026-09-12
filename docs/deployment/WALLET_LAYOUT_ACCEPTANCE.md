# Wallet 0.1.12 layout and browser acceptance

September 8, 2026. The owner requested stronger desktop, tablet and mobile
layouts while retaining the existing Lichen colours and transaction behavior.
This is a layout update, not a replacement custody implementation.

The web wallet and extension full view use the same `wallet-layout.css`, copied
by `scripts/sync_frontend_shared_helpers.js` and checked for drift in CI. Desktop
has a section sidebar, a balance/action panel and aligned asset rows. Tablet
stacks the balance actions; mobile uses fixed section navigation above the chain
status bar. The extension popup uses the same file with compact popup selectors.
Existing IDs, action hooks and tab identifiers are preserved. Icon-only header
controls have labels, and keyboard focus and reduced-motion settings are covered.

The 0.1.11 polish uses equal64px square balance actions with a consistent icon
box and places refresh at the card's top-right edge. Achievements initially show
up to four earned badges. A native keyboard-accessible disclosure exposes all
92 definitions, earned first, in a bounded scroll region. Web and extension full
views use the same classes and styles; the mobile header controls are retained.
Browser checks cover button geometry, compact preview, all92 rows and keyboard
expand/collapse in the actual web and extension identity renderers.

The manifest, About screens and provider version fallbacks are 0.1.12. The web
PWA cache is `lichen-wallet-v10-20260912-history` and includes the shared stylesheet.

When the indexed history request fails, the web wallet displays a retry state
instead of presenting old faucet records as complete recent activity. While an
initial or refreshed history request is pending, it displays a loading state.
Browser acceptance covers a delayed latest page, a failed page with available
faucet records, and retry without advancing the pagination cursor.
Production export content-versions first-party script and stylesheet references.
Extension updates use the existing `wallet-extension-v0.1.12` tag workflow; a
local package is not a browser-store publication or evidence of user auto-update.

## Alignment boundaries

The September 12 activity update starts history loading independently of price
and balance requests in the web and extension full views. Both clients bound
history requests to 20 seconds, including incomplete response bodies, and offer
an explicit retry after an initial failure. A late request cannot overwrite a
newer activity page. The web faucet supplement retains its two-second deadline
through response decoding. Signing requests are not retried by this change.

Read-only public observations before this update measured dashboard activity at
4.364 seconds versus 0.583 seconds when opened directly; the underlying latest
history RPC took 0.327 and 0.305 seconds. A representative archived 20-row page
took 10.106 seconds initially and 4.484 seconds on repetition, with identical
canonical content. These are separate frontend and backend observations, not
measurements of the owner's wallet. Browser fixtures verify loading behavior;
they do not establish that live archived-query latency is resolved.

| Area | Shared behavior / verification |
| --- | --- |
| Navigation and layout | Same six sections and ordering in web, full extension and popup; shared layout source and drift check. |
| Connection and signing | Web popup connection persists for 30 minutes in the application tab. Extension connection uses its existing background provider. Both retain explicit wallet approval and password-gated signing. Transport lifetimes differ. |
| Crypto and transaction encoding | Existing canonical helpers remain in place. Wallet audit and extension signing/provider E2E gate are required; no cryptographic or transaction amount code changed by the layout patch. |
| Restrictions | Existing wallet-side checks, warnings and blocked transaction enforcement remain; wallet and extension audits must pass. |
| Shielded history | Existing read-only state and disabled unsupported actions are retained on all surfaces. |
| Feature completeness | A common layout is not proof that every feature is identical. The extension full-page asset renderer still emphasizes native LICN while the web wallet lists wrapped balances; existing flows are preserved in this visual change. Further feature work needs its own functional acceptance. |

## Repeatable checks

Install the locked root and SDK dependencies, build the SDK, then run:

```sh
npm run validate-wallet-extension-release
npm run test-frontend-assets
npx --no-install playwright install chromium
npm run test-wallet-browser
node scripts/qa/audit_frontend_rpc_parity.js
node scripts/qa/test_github_actions_supply_chain.js
```

The browser suite starts a local server and disposable Chromium profiles. It
tests web widths 1440, 834, 390 and 320; extension full desktop/mobile and a
400px popup; section switching; web Receive and Settings; and actual cross-origin
popup connect/close/reload/reopen/disconnect. It also replaces an old PWA cache
and checks that the new layout is cached. Screenshots and results go to
`dist/wallet-browser/`. Synthetic balances and a synthetic provider response are
fixtures, not chain transactions. No real key or funded transaction is used.

The CI wallet job and extension release workflow run the browser suite as well
as static, wallet and signing tests. Publication still requires a clean commit,
successful workflows and public asset/header verification. Record the actual
Pages IDs, release tag and served hashes in deployment evidence after publishing.
