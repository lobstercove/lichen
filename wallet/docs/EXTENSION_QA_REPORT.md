# MoltWallet Extension QA Report

Date: 2026-02-15
Workspace: /Users/johnrobin/.openclaw/workspace/moltchain/wallet

## Scope
This report validates extension parity with the wallet web surface for:
- Wallet lifecycle and settings/security flows
- Full-page advanced routes (home/identity/nfts/settings/approve)
- dApp provider compatibility and approval lifecycle
- Ethereum compatibility aliases and network-management requests

## Automated Validation (Provider Compatibility)
Command run:
- `node docs/provider_compat_check.mjs`

Result:
- Pass: 13
- Fail: 0
- Exit code: 0

Validated method families:
- `eth_chainId`, `net_version`, `eth_coinbase`
- `web3_clientVersion`, `net_listening`
- `eth_getCode`, `eth_estimateGas`, `eth_gasPrice`
- `wallet_watchAsset`
- `wallet_switchEthereumChain`
- `wallet_addEthereumChain`
- Alias shape checks:
  - `eth_getBalance` (hex quantity response)
  - `eth_getTransactionCount` (hex quantity response)

State mutation checks validated:
- Network switch updates extension selected network
- Add-chain request updates stored RPC endpoint

## Static Runtime Validation
Diagnostics checks on touched runtime files:
- `extension/src/core/provider-router.js` → no errors
- `extension/src/content/inpage-provider.js` → no errors
- `extension/src/pages/approve.js` → no errors
- `extension/src/pages/home.html` → no errors
- `extension/src/background/service-worker.js` → no errors

## Install and Run
1. Open `brave://extensions` (or `chrome://extensions`)
2. Enable **Developer mode**
3. Click **Load unpacked**
4. Select folder: `wallet/extension`
5. Pin the extension icon in toolbar

## One-Session Manual E2E Checklist
### A) Wallet Core
- [ ] Create wallet from generated mnemonic
- [ ] Import via mnemonic/private key/JSON keystore
- [ ] Lock and unlock wallet
- [ ] Switch active wallet

### B) Popup Flows
- [ ] Assets/activity render successfully
- [ ] Send flow signs and submits (password-gated)
- [ ] Receive/deposit actions render expected data
- [ ] Settings save/reload correctly (currency/decimals/lock timeout)
- [ ] Change password succeeds and re-encrypts wallet
- [ ] Export/copy/download private key and mnemonic require password

### C) Full-Page Routes
- [ ] Home dashboard snapshots refresh for identity/staking/bridge/NFT
- [ ] Identity actions: register, add skill, agent updates, vouch, name lifecycle
- [ ] NFT route loads data and detail actions work
- [ ] Settings route updates network/RPC/custody/security/export controls
- [ ] Approvals page lists pending requests and decision flow works

### D) dApp / Provider Compatibility
- [ ] Account connect request approval succeeds
- [ ] Sign message request approval succeeds
- [ ] Sign transaction request approval succeeds
- [ ] Send transaction request approval succeeds
- [ ] Event emission observed (`connect`, `disconnect`, `accountsChanged`, `chainChanged`)
- [ ] `window.ethereum` object present and usable from dApp context
- [ ] Ethereum alias methods resolve without unsupported-method errors

### E) Reliability / Lifecycle
- [ ] Pending approval request expires/cleans up as expected
- [ ] Stale approval request disables decision controls
- [ ] Approved origins list and revoke flow are consistent
- [ ] Reload extension and verify persisted state remains correct

## Conclusion
Based on implemented features, automated provider checks, and runtime diagnostics, the extension is in a release-ready parity state for wallet web-surface behavior and compatibility in this workspace snapshot.

---

### Supporting QA Script
- `docs/provider_compat_check.mjs`
