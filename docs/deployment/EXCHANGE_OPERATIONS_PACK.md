# Lichen Exchange Operations Pack

**Status:** Published testnet-only exchange package
**Created:** 2026-06-29
**Integration guide:** [../guides/EXCHANGE_INTEGRATION.md](../guides/EXCHANGE_INTEGRATION.md)
**Metadata sheet:** [../guides/EXCHANGE_CHAIN_METADATA.md](../guides/EXCHANGE_CHAIN_METADATA.md)
**Tracker:** [../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md](../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md)
**Rollback anchor:** `v0.5.221`, per operator update on 2026-07-01
**Exchange package tag:** `exchange-testnet-v0.5.221`
**Exchange package release:** `https://github.com/lobstercove/lichen/releases/tag/exchange-testnet-v0.5.221`

This pack defines the operational material an exchange expects before listing
native LICN. The current publication scope is testnet-only integration testing
until mainnet launch. Operator contact and status-page approvals are recorded,
and the final testnet-only exchange package is published under
`exchange-testnet-v0.5.221`.

## Publication Gate

Do not include this pack in an external listing package until:

- Status page URL is public or operator-private as disclosed, monitored, and
  approved for exchange use.
- Public RPC and WebSocket endpoints for every network included in the package
  are healthy from outside the operator network. The current package includes
  testnet only.
- Upgrade and rollback policy is confirmed against the signed release process.
- Archive retention and repair procedures are tested against the public target
  network.
- A public exchange simulation passes after local validation for every public
  network included in the package. Testnet passed this gate after `v0.5.221`;
  mainnet remains out of scope until mainnet launch and the launch runbook
  exchange handoff gate closes.

## Service Surfaces

| Surface | Current value | Status | Source |
| --- | --- | --- | --- |
| Mainnet RPC | `https://rpc.lichen.network` | Launch placeholder; excluded from the current testnet-only package until mainnet launch handoff passes | `seeds.json`, `developers/shared-config.js`, mainnet launch runbook |
| Mainnet WebSocket | `wss://rpc.lichen.network/ws` | Launch placeholder; excluded from the current testnet-only package until mainnet launch handoff passes | `developers/shared-config.js`, mainnet launch runbook |
| Testnet RPC | `https://testnet-rpc.lichen.network` | Healthy after signed `v0.5.221` recovery rollout on 2026-07-01; sustained public cadence sampled `370.0ms/block`, `getMetrics.observed_block_interval_ms = 372`, and `avg_block_time_ms = 380`; public exchange simulation passed | `seeds.json`, `developers/shared-config.js`, tracker Phase 5 evidence |
| Testnet WebSocket | `wss://testnet-rpc.lichen.network/ws` | Public readiness WebSocket check passed after signed `v0.5.221` recovery rollout on 2026-07-01; live slot notifications advanced `6871609` -> `6871611` | `developers/shared-config.js` |
| Explorer | `https://explorer.lichen.network` | Route templates verified | `seeds.json`, `developers/shared-config.js`, `explorer/js/*.js` |
| Status page | `https://monitoring.lichen.network` | Operator-approved exchange status page for the current testnet-only package on 2026-07-01; private operator surface | Operator update on 2026-07-01 |
| Developer portal exchange page | `https://developers.lichen.network/exchange-integration` | Deployed and verified after the scope update; public page contains `testnet-only`, `mainnet launch exchange handoff`, and operations-pack links | Cloudflare Pages deployment and tracker verification |
| GitHub exchange docs | `docs/guides/EXCHANGE_INTEGRATION.md` | Published under package tag `exchange-testnet-v0.5.221` for the current testnet-only package | Phase 1 docs work; final package release |

## Incident Contacts

Do not publish personal emails, private keys, private RPC URLs, or ad hoc chat
handles in exchange docs.

Operator-approved exchange contact aliases were recorded on 2026-07-01. Do not
substitute personal emails, private chat handles, or undocumented inboxes in the
external package.

Required exchange contact aliases:

| Alias class | Minimum purpose | Candidate/source | Status |
| --- | --- | --- | --- |
| Security incident alias | Vulnerability reports, suspected key compromise, signing compromise, bridge/custody fund-safety issues, and coordinated disclosure | `security@lichen.network` | Approved on 2026-07-01 for exchange escalation use; repository security policy response targets still apply to non-critical vulnerability reports |
| Exchange operations alias | Deposit/withdrawal incidents, RPC/WebSocket/archive degradation, finality delays, maintenance coordination, stuck transaction investigation, and exchange-side pause/resume notices | `exchange-ops@lichen.network` | Approved on 2026-07-01 for exchange operations escalation |
| Business/listing alias | Listing paperwork, legal/compliance coordination, market/asset metadata updates, and relationship management | `business@lichen.network` | Approved on 2026-07-01 for exchange business and listing coordination |
| Status page contact surface | Incident and maintenance updates that exchanges can cite during operational events | `https://monitoring.lichen.network` | Approved as the status page on 2026-07-01 for the current testnet-only package |

Approved escalation policy:

- Owner: Lichen operations owns `security@lichen.network`,
  `exchange-ops@lichen.network`, `business@lichen.network`, and the monitoring
  status surface for the current testnet-only package.
- Critical exchange-impacting incidents: acknowledge through the relevant alias
  within 1 hour when the incident affects deposits, withdrawals, RPC/WebSocket
  availability, archive/history lookup, finality, signing, keys, custody, or
  fund safety.
- Active incident updates: publish a status-page update at first
  acknowledgement and at least every 2 hours until mitigation or resolution.
- Planned maintenance: target 72 hours notice; use 24 hours minimum where
  operationally possible. Emergency security releases may use shorter notice
  with immediate status-page publication.
- Authenticated outbound policy: exchanges should treat status-page updates and
  replies from the approved aliases as the authoritative exchange contact
  surface unless a separate signed-contact ceremony is agreed bilaterally.
- Backup path: if `exchange-ops@lichen.network` is unavailable during a live
  incident, use `security@lichen.network` for security or fund-safety impact and
  `business@lichen.network` for listing or relationship coordination; the status
  page remains the shared operational reference.

## Status Page Policy

For the current testnet-only package, the status page must report, at minimum:

- Testnet RPC availability.
- Testnet WebSocket availability.
- Explorer availability.
- Archive/history health.
- Validator/finality status.
- Known deposit/withdrawal-impacting incidents.
- Planned upgrades and maintenance windows.

Before mainnet is added to an external exchange package, the same status page
must also report mainnet RPC availability, mainnet WebSocket availability, and
mainnet archive/history health.

If the final listing package includes optional ecosystem context beyond native
LICN deposits, the same status surface must separately report:

- Custody/bridge route health and reserve visibility for wrapped assets.
- DEX API and route availability.
- Oracle feed freshness and stale-feed incidents.

These optional ecosystem surfaces must not be presented as prerequisites for
native LICN deposits or withdrawals.

The monitoring surface `https://monitoring.lichen.network` returned HTTP `200`
on 2026-06-29 and served the `Lichen Mission Control - Network Monitoring`
page. The operator approved it on 2026-07-01 as the exchange status page for the
current testnet-only package. During active exchange-impacting incidents, publish
the first update at acknowledgement and continue updates at least every 2 hours
until mitigation or resolution.

## Upgrade Policy

External exchange docs must define:

- Normal release cadence.
- Minimum exchange notice for non-breaking upgrades.
- Minimum exchange notice for breaking RPC, address, signing, or finality changes.
- Maintenance window format and expected duration.
- Emergency security-release exception process.
- Supported rollback version.

Current rollback anchor:

```text
v0.5.221
```

Version drift is a release blocker. Root README, mainnet runbook, RPC docs, and
SDK package versions must be reconciled before this pack can publish a "current
release" statement.

## Release Verification

Source-backed release verification flow:

1. Download release assets from the GitHub release.
2. Verify hashes with `SHA256SUMS`.
3. Verify the detached native PQ signature `SHA256SUMS.sig`.
4. Check the signer against `deploy/release-trust-anchor.json`.

Repository sources:

- `.github/workflows/release.yml`
- `deploy/release-trust-anchor.json`
- `scripts/sign-release.sh`
- `scripts/verify-release-checksums.mjs`

Current trust-anchor signer:

```text
8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk
```

Verified rollback-anchor release subset on 2026-07-01:

- `https://github.com/lobstercove/lichen/releases/tag/v0.5.221` returned HTTP
  `200`.
- GitHub API reports `tag_name = v0.5.221`, `draft = false`, and
  `prerelease = false`.
- `SHA256SUMS` and `SHA256SUMS.sig` downloaded from the release.
- `scripts/verify-release-checksums.mjs` against the downloaded `v0.5.221`
  release artifacts
  verified signer `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk`.

Current signed recovery release verified on 2026-07-01:

- `https://github.com/lobstercove/lichen/releases/tag/v0.5.221`
- GitHub release is published, not a draft, and not a prerelease.
- Linux release archives include `lichen-validator`, `lichen-custody`, and
  `lichen-faucet`.
- `SHA256SUMS` and `SHA256SUMS.sig` downloaded from the release.
- `scripts/verify-release-checksums.mjs` verified all archive hashes and the PQ
  signature against signer `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk`.

Published exchange package metadata:

- Package tag: `exchange-testnet-v0.5.221`
- Package release page:
  `https://github.com/lobstercove/lichen/releases/tag/exchange-testnet-v0.5.221`
- Package archive asset: `lichen-exchange-testnet-v0.5.221.tar.gz`
- Package checksum asset: `SHA256SUMS`
- Scope: testnet-only until the mainnet launch exchange handoff closes.

Current recovery release and rollback anchor: `v0.5.221`, retained until a newer
signed rollback point is explicitly recorded.

## Rollback Policy

Rollback policy must define:

- Trigger criteria: consensus safety, RPC/archive regression, wallet/accounting
  regression, signing regression, severe performance regression, or security
  incident.
- Decision owner and quorum.
- Exchange notification path.
- Validator restart order.
- RPC/archive compatibility expectation during rollback.
- Expected effect on deposits and withdrawals.
- Evidence required before declaring recovery.

Current rule: keep `v0.5.221` as the rollback anchor until a newer signed rollback
point is explicitly recorded.

Exchange rollback procedure:

1. Publish incident status and notify exchange operations aliases before any
   planned rollback, or immediately after an emergency rollback begins.
2. Tell exchanges to pause automatic credits and withdrawals if finality,
   archive history, signing, or transaction submission may be affected.
3. Record the last healthy finalized slot, affected slot range, release tag,
   rollback tag, and public RPC/archive status before action.
4. Use only the signed-release rollback path and verify `SHA256SUMS` plus
   `SHA256SUMS.sig`; do not reset validator state or copy RocksDB state unless a
   separate incident decision explicitly approves destructive recovery.
5. After rollback, verify public RPC/WebSocket health, finalized slot
   progression, `getTransaction`, `getTransactionsByAddress`,
   `getTransactionHistory`, `getAccountTxCount`, and representative old/recent
   block lookups.
6. Reconcile pending deposits and withdrawals by transaction hash and account
   history before telling exchanges to resume automation.
7. Publish a recovery note with the final slot range, whether replay or
   customer reconciliation is required, and the release now considered current.

## Archive Policy

Exchange-facing RPC must be archive-backed. Archive policy must define:

- Retention target: old transaction and account-history data queryable
  indefinitely for exchange operations.
- Required query methods: `getBlock`, `getLatestBlock`, `getTransaction`,
  `getTransactionsByAddress`, `getTransactionHistory`, and `getAccountTxCount`.
- Backup schedule.
- Restore drill.
- Hot/cold migration procedure.
- Public-history merge and repair procedure.
- Evidence path for archive validation.

Source-backed capabilities exist for cold-store fallback and public-history merge.
Local archive/history regressions passed on 2026-06-29 for core storage and RPC
methods after hot-to-cold migration and reopen. Public exchange publication still
requires proving the selected public RPC/archive deployment can serve old
transaction and account-history data continuously.

## Chain Halt Or Delayed Finality

During a halt, delayed finality, archive lag, or divergent endpoint incident:

- Exchanges should pause automatic credits if finalized slot stops advancing.
- Exchanges should keep polling recorded transaction hashes.
- Exchanges should avoid re-broadcasting withdrawals without idempotency checks.
- Lichen operations must publish status updates at the agreed cadence.
- Recovery notice must include the affected slot range and whether replay or
  reconciliation is required.

Approved exchange notice cadence is defined in the incident contact policy above.

## Live Testnet Recovery Evidence

On 2026-06-29, the public testnet readiness gate exposed a consensus progression
incident: three validators were stale at slot `6708256`, while
`15.204.229.189` was behind at slot `6707400` with pending far-ahead blocks
waiting for missing parents.

Runbook action taken after operator-approved access and evidence preservation:

- Preserved host status, binary hashes, local RPC responses, targeted journals,
  and post-action journals under
  `evidence/exchange-readiness/live-20260629T154831Z/`.
- Restarted only `15.204.229.189` with
  `sudo systemctl restart lichen-validator-testnet`.
- Did not reset state, delete archive data, copy RocksDB, roll a release, or run
  a clean-slate redeploy.

Observed result:

- The restarted validator caught up from `6707400` to the cluster tip.
- A five-sample watch showed all four validators reporting `status = ok` with
  fresh slots advancing through `6708526`-`6708536`.
- The public testnet readiness checks for RPC health, `getFeeConfig`,
  finalized-slot, latest-block, and WebSocket upgrade passed.

The public exchange package is published for testnet-only integration under
`exchange-testnet-v0.5.221`. The status page was approved on 2026-07-01 as
`https://monitoring.lichen.network` for the current testnet-only package, and
exchange contact aliases were approved on 2026-07-01 as
`security@lichen.network`, `exchange-ops@lichen.network`, and
`business@lichen.network`. Mainnet RPC/WS readiness is deferred to the mainnet
launch exchange handoff gate and must be rechecked with the public readiness
gate in `--scope full` mode before mainnet is added.

Update on 2026-06-30: the public testnet was stale again at slot `6715444`
while all four services remained active. The signed `v0.5.217` rollout was
installed non-destructively on all four validators, preserving state, cold
archives, WAL, keys, and peer identity. The chain resumed finality; a
twelve-sample watch showed public/local `health.status = ok` through public slot
`6715694`. The clean exchange-facing release candidate is now `v0.5.219`, which
keeps the consensus liveness fix and refreshes `anyhow` to the patched
`1.0.103` lockfile version so Cargo Audit/Deny pass.

Final update on 2026-06-30: signed `v0.5.219` was published, signature-verified,
and deployed through the rolling release runbook. All four validators and CLIs
now report `0.5.219`, all four local RPC health checks return `status = ok`, and
all four faucet units are active. The verify-only release runbook completed
`RELEASE VERIFY COMPLETE`, proving the installed validator, custody, and faucet
binaries match the signed release archive hashes on every host. Public RPC,
public WebSocket, public faucet, public DEX oracle/candle smoke, and the public
faucet-backed exchange simulation all passed after the rollout.

Update on 2026-07-01: signed `v0.5.220` was published, signature-verified,
and deployed non-destructively through the rolling release runbook. All four
validators and CLIs report `0.5.220`; runbook verify-only completed
`RELEASE VERIFY COMPLETE`. Public cadence returned to the expected range:
`getMetrics.observed_block_interval_ms = 304`, `avg_block_time_ms = 330`, and a
postdeploy public watch estimated `337.3ms/block`. Public RPC, DEX oracle/candle
smoke, developer exchange page, and the public faucet-backed exchange
simulation passed after the rollout.

Follow-up update on 2026-07-01: the public testnet later stalled again at
height `6871323` while services and state remained present. Pre-recovery
evidence was preserved under
`evidence/v0.5.221-live-recovery-20260701T083620Z/`. Signed `v0.5.221` was
published, signature-verified, staged on all four VPSes from GitHub Release
archives, and deployed through a coordinated state-preserving stop/start because
the network was already stalled. No validator state, cold archive, WAL, keypair,
node identity, or RocksDB directory was deleted or copied. All four running
validator process hashes matched the signed release hash, and the runbook
verify-only gate completed `RELEASE VERIFY COMPLETE`.

Post-recovery public checks passed: all four validators reported
`lichen-validator 0.5.221` and `status = ok`; sustained public cadence advanced
190 blocks over 70.39 seconds (`370.0ms/block`); public `getMetrics` returned
`observed_block_interval_ms = 372`, `avg_block_time_ms = 380`, and
`validator_count = 4`; WebSocket `subscribeSlots` passed 10/10 and live slot
notifications advanced `6871609` -> `6871611`; explorer assets and public RPC
metrics were reachable; and the public faucet-backed exchange simulation passed
with report `tests/artifacts/exchange-simulation-public-testnet-v0.5.221.json`.

## Local Validation Evidence

Before public testnet exchange release, create evidence under an ignored local
path such as:

```text
evidence/exchange-readiness/<date>/
```

Required evidence:

- Clean stack start output.
- Three-validator health/status output.
- Generated exchange wallet addresses.
- Deposit transaction hash, slot, and finality proof.
- Internal credit ledger output showing exactly-once credit.
- Sweep transaction hash if sweep is in the selected model.
- Withdrawal transaction hash, slot, and finality proof.
- Reconciliation report.
- Archive/history query output before and after validator restart.
- Cleanup output proving validators, custody, faucet, and local source-chain mocks
  are stopped.

Do not commit private keys or filled production env files as evidence.

## Open Operations Blockers

| ID | Blocker | Required next step |
| --- | --- | --- |
| None for current testnet-only package | Mainnet remains deferred | Run mainnet launch exchange handoff and full-scope readiness before adding mainnet |

## Resolved Operations Checks

| ID | Check | Evidence |
| --- | --- | --- |
| O-01 | Status page URL approved | `https://monitoring.lichen.network` approved by the operator on 2026-07-01 for the current testnet-only package |
| O-02 | Incident aliases approved | Operator approved `security@lichen.network`, `exchange-ops@lichen.network`, and `business@lichen.network` on 2026-07-01; critical acknowledgement, active update, maintenance notice, emergency exception, authenticated outbound, and backup-path policy are recorded above |
| O-03 | Current release drift for core docs and Rust SDK pin | Core crates and the Rust SDK pin were updated to `0.5.221`; `v0.5.221` is the rollback anchor; JS/Python package boundaries are documented in the tracker |
| O-04 | Final external exchange-package release URLs attached | Package release `https://github.com/lobstercove/lichen/releases/tag/exchange-testnet-v0.5.221` contains `lichen-exchange-testnet-v0.5.221.tar.gz` and `SHA256SUMS` |
| O-05 | Local archive/history behavior | Core and RPC archive regressions passed after hot-to-cold migration and reopen |
| O-07 | Local cleanup evidence | Local stack stop/status/process checks passed; generated credentials, state dirs, manifests, and staging dirs were removed after the local exchange simulation |
| O-09 | Rollback release checksum/signature verification | `v0.5.221` release checksum and detached PQ signature were verified against `deploy/release-trust-anchor.json` |
| O-11 | June 29 live testnet consensus incident recovery evidence preserved | Operator-approved evidence-preserving recovery restarted only stale validator `15.204.229.189`; the June 30 recurrence is tracked separately in `docs/deployment/TESTNET_RECOVERY_INCIDENT_2026-06-30.md`; signed `v0.5.217` restored testnet liveness, and signed `v0.5.219` completed the faucet-signing and exchange-simulation follow-up |
| O-12 | Signed `v0.5.221` testnet recovery release verification | Release artifacts and detached PQ signature verified; all four live hosts installed matching validator, custody, and faucet binaries through the runbook verify-only gate |
| O-13 | Public testnet exchange simulation | Public faucet-backed simulation passed on `https://testnet-rpc.lichen.network` and wrote `tests/artifacts/exchange-simulation-public-testnet-v0.5.221.json`, covering funding, deposit detection, finalized transaction lookup, account history, operational buffers, sweep, withdrawal, CLI smoke, and reconciliation |
| O-14 | Current package scope | External package is explicitly testnet-only until mainnet launch; mainnet RPC/WS and metadata are launch placeholders and require the mainnet launch exchange handoff gate plus `exchange_public_readiness.py --scope full` before inclusion |
| O-10 | Public developer portal exchange page | `developers/` was deployed to Cloudflare Pages project `lichen-network-developers`; public `https://developers.lichen.network/exchange-integration` contains `Exchange Integration`, `Exchange Integration Guide`, `Exchange Chain Metadata`, `Exchange Operations Pack`, and `testnet-only` |
| O-06 | Exchange-specific rollback procedure | Rollback policy now includes exchange notification, pause/resume guidance, affected slot recording, signed-release-only rollback, archive/history verification, pending transaction reconciliation, and recovery notice requirements |
