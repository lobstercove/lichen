## 2026-04-23 Monitoring Telemetry Hardening And RPC Rollout

- Audited Mission Control against live RPC wiring rather than just the visible badge copy.
- Confirmed the main live problems were:
  - the network-health badge averaged in a block-cadence heuristic that made a healthy synced cluster look degraded
  - top-line uptime was browser session age, not chain age
  - several DEX/ecosystem labels did not match the fields actually returned by RPC
  - `getSporePumpStats` was genuinely missing from JSON-RPC even though launchpad REST stats already existed

- Code changes:
  - `monitoring/index.html`
    - relabeled `UPTIME` → `CHAIN AGE`
    - relabeled `Block Production` → `Block Cadence`
    - relabeled `Memory Usage` → `Account Footprint`
    - relabeled `RPC Batch` → `RPC Latency`
    - corrected DEX/ecosystem labels such as `Total Volume`, `AMM Volume`, `Open Markets`, and `Bridge Nonce`
  - `monitoring/js/monitoring.js`
    - chain age now comes from block `0` instead of page-session elapsed time
    - health badge now uses consensus + validator availability + P2P, while block cadence remains visible but no longer drives the overall degraded state
    - DEX/ecosystem mappings were corrected to real RPC field names and SporePump is included in the ecosystem board
  - `rpc/src/launchpad.rs`
    - factored the existing launchpad platform stats into a reusable helper
    - added `handle_get_sporepump_stats(...)` for JSON-RPC
  - `rpc/src/lib.rs`
    - wired `getSporePumpStats` into JSON-RPC dispatch

- Validation before deploy:
  - `node --check monitoring/js/monitoring.js`
  - `npm run test-frontend-assets`
  - `cargo check -p lichen-rpc`
  - a direct scan of every `rpc('...')` call in `monitoring/js/monitoring.js` against `rpc/src/lib.rs` dispatch showed no missing methods after the SporePump addition

- Live rollout:
  - Public `testnet-rpc.lichen.network` still returned `Method not found` for `getSporePumpStats` after the local code changes, because the RPC server is embedded in `lichen-validator`
  - Built a canonical Linux validator artifact on the US VPS from `/home/ubuntu/lichen-monitoring-rpc-src`
  - Canonical validator hash:
    - `aff07991477e329c864eeb89beefc6979f7ac44911fc468c853ac1755bed9440`
  - Sequentially rolled the same binary to:
    - EU
    - SEA
    - US
  - Each host was checked for:
    - installed hash
    - `lichen-validator-testnet=active`
    - advancing slot
    - successful local `getSporePumpStats`

- Final live checks:
  - public `https://testnet-rpc.lichen.network` now returns real `getSporePumpStats`
  - sampled slots after rollout:
    - US `731308`
    - EU `731309`
    - SEA `731308`
  - `getServiceFleetStatus` from public RPC:
    - `state=healthy`
    - `healthy_services=4`
    - `degraded_services=0`
    - `intentionally_absent_services=2`
  - redeployed Mission Control via `scripts/deploy-cloudflare-pages.sh monitoring`
  - preview URL:
    - `https://27de247a.lichen-network-monitoring.pages.dev`
  - verified both preview and `https://monitoring.lichen.network` serve the updated HTML/JS

- Important operational gotchas:
  - The US build host had Rust `1.88.0` pinned as default even after `rustup update stable`; the fix was `rustup default stable` plus forcing `PATH=$HOME/.cargo/bin:$PATH`
  - SSH login banners are still present on the VPSes; use `scp -O` or other binary-safe transport for validator artifacts instead of raw streamed copies
