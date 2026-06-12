# Core Sync, Pace, and Monitoring Audit - 2026-06-12

## Scope

Audit follow-up for the v0.5.151 sync/snapshot hardening release on the live testnet validators, with no consensus or storage code changes made from this audit unless a root cause is verified.

## Snapshot and Disk Findings

- Checkpoint snapshots are not stacking unbounded. The live code prunes state checkpoint directories with a retention target of 3.
- The reachable validator had three checkpoint directories after rotation, matching the retention policy.
- Disk cleanup removed stale temporary diagnostics, old extracted artifacts, and the obsolete pinned checkpoint from the seed host during the earlier cleanup pass.
- Re-check from this workstation currently reaches only `15.235.142.253` over the configured SSH path. The other validator SSH endpoints time out from this machine, so direct post-cleanup verification on those hosts remains operationally blocked until SSH access is restored.

## Block Pace Finding

The pace regression is not explained by snapshot size or database growth on the reachable validator. The live logs show intermittent BFT proposer timeouts:

- At heights such as `3383955` and `3383963`, round 0 received no proposal.
- The healthy validators nil-voted, advanced to round 1, and committed immediately after the next proposer produced a block.
- The reachable validator still sees `148.113.43.247:7001` connected at the P2P layer and receives non-BFT validator/vote traffic from `6XhsGituXoWSd1wLtutZgdJve6gLrdSi7YhEx1ZDFHW`.
- The same logs do not show BFT proposal/prevote/precommit traffic from that validator during the sampled windows.
- Recent block production sampled through RPC showed the other three validators producing blocks, while `6XhsGituXoWSd1wLtutZgdJve6gLrdSi7YhEx1ZDFHW` produced zero in the sampled range.

Interpretation: the current slowdown is consistent with one active validator being connected but not participating in BFT proposal production. When leader election selects that validator, the network waits through the proposer timeout, nil-votes round 0, then commits in round 1. This produces the visible block pace spikes.

Direct root-cause confirmation requires logs on `148.113.43.247`, but SSH to that host currently times out from this workstation.

## Monitoring Coverage Finding

Live testnet RPC confirms WBTC exists and exposes the expected live-backed methods:

- `getSymbolRegistry("WBTC")` returns `6zQChEy6XacfQR52892oAMpntavfpb6mBUvLRkyXxno1`.
- `getWbtcStats` returns the WBTC supply/reserve/pause stats shape.
- `getBridgeRouteRestrictionStatus("bitcoin", "btc")` returns Bitcoin route health, including `route_ready`, custody status, and missing config.

The monitoring frontend was behind the chain surface:

- `SYMBOLS` omitted `WBTC`.
- `ALL_CONTRACTS` omitted `WNEO`, `WGAS`, and `WBTC`.
- The ecosystem panel displayed WNEO/WGAS supplies but not WBTC.
- The Oracle & Bridge board displayed Neo X route/reserve health but not Bitcoin route/reserve health.

This has been corrected in the monitoring frontend with regression tests so the WBTC route and wrapped-asset inventory cannot silently disappear again.
