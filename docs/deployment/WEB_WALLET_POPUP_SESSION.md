# Web wallet popup connection acceptance

September 8, 2026. Closing the wallet popup previously cleared the provider's
connection and the DEX additionally excluded all accounts from a closed popup.
The result was a read-only DEX despite a previously approved account connection.

The DEX now retains public connection metadata in the current tab's
`sessionStorage`, with a fixed 30-minute expiry from account connection. Reload
within that tab retains the connection; closing the tab ends its storage session.
Browser tab/session restoration can restore sessionStorage, but the absolute
expiry still applies. Polling, popup reopening and signing do not extend expiry.
The wallet's existing 30-day origin permission is separate from this shorter
application connection. Expiry requires reconnecting; explicit disconnect clears
the application connection immediately.

Closing a popup cancels all its unfinished requests without revoking the account
connection. A signature request reopens the wallet. If browser popup blocking
prevents opening after asynchronous preflight, an Open Web Wallet button resumes
the same pending request from a fresh click; Cancel or timeout discards it.
Responses require both the configured wallet origin and the exact current popup
window. Requests from a closed window are never retried in its replacement.

Signing always uses the wallet's existing explicit password approval and private
key decryption. A locked dashboard can show that approval directly, avoiding a
second login screen. This does not unlock the dashboard. No password or key is
stored in the DEX or added to browser storage. Account visibility while locked is
limited to origins already approved in the wallet. Extension lock behavior is
unchanged. The shared provider copy for Programs is synchronized; its application
UI still has its existing separate live-popup gate.

Before deployment build the locked JS SDK, then run `npm run test-wallet`,
`npm run test-frontend-assets`, frontend RPC parity and shared helper checks.
The lifecycle suite executes the actual provider, DEX snapshot and
wallet bridge against controlled browser/crypto boundaries: close/reopen, reload,
new tab, fixed expiry, explicit disconnect, pending cancellation, late responses,
wrong origin/window, blocked-popup continuation and password rejection.

Publish the wallet and DEX from the same clean commit with content-versioned
asset URLs, wallet first. Verify public served bytes and security headers, retain
both previous Pages deployment IDs, and record deployment receipts separately.
The DEX export must preserve the independently verified licensed chart files.
Browser smoke: connect, close popup, verify active order controls and balances,
request approval, cancel it, verify the connection remains, then disconnect.
Never submit a funded transaction merely to test this lifecycle.
