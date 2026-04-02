# shared/

Shared utilities and configuration used across all Lichen frontend apps.

## Canonical Source

The **canonical version** of shared JS files lives in `monitoring/shared/`.
All other frontends (`explorer`, `dex`, `wallet`, `marketplace`, `faucet`, `programs`, `developers`)
receive synced copies. The wallet extension also consumes synced `utils.js` and `pq.js`.
**Always edit `monitoring/shared/` first**, then sync.

### Files

| File | Purpose |
|---|---|
| `utils.js` | Protocol constants, formatters, BS58, RPC client, pagination |
| `wallet-connect.js` | Unified wallet connection modal + signing |

### Syncing

```bash
# Sync utils.js to all frontends and the extension runtime helpers:
for dir in explorer dex wallet marketplace faucet programs developers; do
  cp monitoring/shared/utils.js "$dir/shared/utils.js"
done
cp monitoring/shared/utils.js wallet/extension/shared/utils.js

# Sync pq.js to all frontends and the extension runtime helpers:
for dir in explorer dex wallet marketplace faucet programs; do
  cp monitoring/shared/pq.js "$dir/shared/pq.js"
done
cp monitoring/shared/pq.js wallet/extension/shared/pq.js

# Sync wallet-connect.js (marketplace has a custom version — skip it):
for dir in explorer dex wallet faucet programs developers; do
  cp monitoring/shared/wallet-connect.js "$dir/shared/wallet-connect.js"
done
```

> **Note:** `marketplace/shared/wallet-connect.js` is a deliberate full rewrite
> with a DEX-style modal and PQ-native wallet flow. It is NOT synced from the canonical.

> **Extension note:** the browser extension injects `window.licnwallet`; the shared
> wallet connector should prefer that provider before falling back to RPC or local PQ
> wallet creation.

### INF-18 Audit Note

All shared JS was consolidated on 2026-03-02. Prior to this, each frontend had
divergent copies with app-specific patches. The canonical now includes all
improvements merged from every frontend copy.
