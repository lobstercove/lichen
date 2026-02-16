# MoltWallet Extension — Go-Live Checklist

## 1) Build / Package Readiness
- [ ] `manifest.json` loads with no parse errors
- [ ] Extension loads unpacked without warnings that block runtime
- [ ] Required icon asset exists: `extension/MoltWallet_Logo_256.png`

## 2) Install + Launch
- [ ] Load unpacked from `wallet/extension`
- [ ] Toolbar/action icon shows MoltWallet logo (not lobster)
- [ ] Popup opens to welcome/create/import when no wallet exists
- [ ] Open Full opens full wallet flow (not advanced-only dashboard)

## 3) Core Wallet Flows
- [ ] Create wallet from generated mnemonic
- [ ] Import wallet (mnemonic, private key, JSON keystore)
- [ ] Lock/unlock works reliably
- [ ] Network switch updates data surface
- [ ] Send/receive/basic activity flows execute

## 4) Security + Settings
- [ ] Auto-lock timeout saves and enforces
- [ ] Password change re-encrypts key material
- [ ] Export private key/seed requires password
- [ ] Download private key/seed requires password
- [ ] RPC and custody endpoint overrides persist

## 5) Full-Page Advanced Routes
- [ ] Identity route actions submit and refresh
- [ ] Staking actions submit and refresh
- [ ] Bridge address request + status polling works
- [ ] NFT route loads and detail actions function
- [ ] Settings route controls mirror popup parity

## 6) dApp Connectivity + Approvals
- [ ] Connect request prompts approval and persists origin
- [ ] Sign message / sign transaction / send transaction approvals work
- [ ] Approval queue handles pending and stale states safely
- [ ] Permission list + revoke in settings works
- [ ] `window.ethereum` compatibility object available in page context

## 7) Compatibility Sanity
- [ ] Run: `node docs/provider_compat_check.mjs`
- [ ] Confirm pass/fail summary reports `fail: 0`

## 8) Final Sign-Off
- [ ] Smoke test complete on Brave
- [ ] Smoke test complete on Chrome
- [ ] Release notes published (`docs/RELEASE_NOTES.md`)
- [ ] QA report archived (`docs/EXTENSION_QA_REPORT.md`)
