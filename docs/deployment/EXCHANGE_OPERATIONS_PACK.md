# Lichen Exchange Operations Pack

**Status:** Published testnet-only exchange package
**Created:** 2026-06-29
**Integration guide:** [../guides/EXCHANGE_INTEGRATION.md](../guides/EXCHANGE_INTEGRATION.md)
**Metadata sheet:** [../guides/EXCHANGE_CHAIN_METADATA.md](../guides/EXCHANGE_CHAIN_METADATA.md)
**Tracker:** [../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md](../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md)
**Current testnet release:** `v0.5.224`
**Rollback anchor:** `v0.5.223`
**Exchange package tag:** `exchange-testnet-v0.5.221`
**Exchange package release:** `https://github.com/lobstercove/lichen/releases/tag/exchange-testnet-v0.5.221`

This pack defines the operational material an exchange expects before listing
native LICN. The current publication scope is testnet-only integration testing
until mainnet launch. Operator contact aliases are recorded, the final
testnet-only exchange package is published under `exchange-testnet-v0.5.221`,
and the public exchange status page is live at
`https://exchanges.lichen.network`. As of 2026-07-05, the active Cloudflare
zone, Pages custom domain, exchange status page, and default public readiness
gate are green for the current testnet-only package.

## Publication Gate

Do not include this pack in an external listing package until:

- Dedicated public exchange status page URL
  `https://exchanges.lichen.network` is active, monitored, and approved for
  exchange use. Internal operator monitoring is admin-only and must not be
  published in exchange materials.
- Public RPC and WebSocket endpoints for every network included in the package
  are healthy from outside the operator network. The current package includes
  testnet only.
- Upgrade and rollback policy is confirmed against the signed release process.
- Archive retention and repair procedures are tested against the public target
  network.
- A public exchange simulation passes after local validation for every public
  network included in the package. Testnet passed this gate after `v0.5.224`;
  mainnet remains out of scope until mainnet launch and the launch runbook
  exchange handoff gate closes.

## Service Surfaces

| Surface | Current value | Status | Source |
| --- | --- | --- | --- |
| Mainnet RPC | `https://rpc.lichen.network` | Launch placeholder; excluded from the current testnet-only package until mainnet launch handoff passes | `seeds.json`, `developers/shared-config.js`, mainnet launch runbook |
| Mainnet WebSocket | `wss://rpc.lichen.network/ws` | Launch placeholder; excluded from the current testnet-only package until mainnet launch handoff passes | `developers/shared-config.js`, mainnet launch runbook |
| Testnet RPC | `https://testnet-api.lichen.network` | Healthy after signed `v0.5.224` archive-parity rollout on 2026-07-20; public health was `ok`, all four producers were fresh, and observed block interval was 334 ms | `seeds.json`, `developers/shared-config.js`, deployment evidence |
| Testnet WebSocket | `wss://testnet-api.lichen.network/ws` | Public readiness WebSocket check passed after signed `v0.5.224` rollout on 2026-07-20 | `developers/shared-config.js`, deployment evidence |
| Explorer | `https://explorer.lichen.network` | Route templates verified | `seeds.json`, `developers/shared-config.js`, `explorer/js/*.js` |
| Public exchange status page | `https://exchanges.lichen.network` | Active on Cloudflare Pages project `lichen-network-exchanges`; production readiness is green; page uses a same-origin read-only status RPC proxy and the RPC CORS default now includes `exchanges.lichen.network` for validator rollouts | Operator correction on 2026-07-02; production verification on 2026-07-05 |
| Developer portal exchange page | `https://developers.lichen.network/exchange-integration` | Deployed and verified; public page carries inline testnet-only metadata, address/accounting rules, deposit and withdrawal cookbooks, finality/archive policy, operations contacts, validation gates, mainnet handoff, release-tagged source links, and the exchange status URL without the old planned wording | Cloudflare Pages deployment and tracker verification |
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
| Status page contact surface | Incident and maintenance updates that exchanges can cite during operational events | `https://exchanges.lichen.network` | Dedicated public exchange status/operations portal is active and approved for exchange use |

Approved escalation policy:

- Owner: Lichen operations owns `security@lichen.network`,
  `exchange-ops@lichen.network`, `business@lichen.network`, and the dedicated
  public exchange status surface.
- Critical exchange-impacting incidents: acknowledge through the relevant alias
  within 1 hour when the incident affects deposits, withdrawals, RPC/WebSocket
  availability, archive/history lookup, finality, signing, keys, custody, or
  fund safety.
- Active incident updates: publish a status-page update at first
  acknowledgement and at least every 2 hours until mitigation or resolution.
- Planned maintenance: target 72 hours notice; use 24 hours minimum where
  operationally possible. Emergency security releases may use shorter notice
  with immediate status-page publication.
- Authenticated outbound policy: exchanges should treat status-page updates
  from the public exchange status page and replies from the approved aliases as
  the authoritative exchange contact surface unless a separate signed-contact
  ceremony is agreed bilaterally.
- Backup path: if `exchange-ops@lichen.network` is unavailable during a live
  incident, use `security@lichen.network` for security or fund-safety impact and
  `business@lichen.network` for listing or relationship coordination; the
  public exchange status page remains the shared operational reference.

## Status Page Policy

Target exchange-facing portal: `https://exchanges.lichen.network`.

Implementation policy:

- Maintain it as a separate public Cloudflare page or application from the
  admin monitoring portal.
- Reuse only exchange-safe monitoring concepts: aggregate RPC health, WebSocket
  health, explorer health, archive/history health, validator/finality aggregate
  status, active incidents, maintenance notices, release/rollback advisory, and
  approved contact aliases.
- Do not expose admin commands, deployment controls, private metrics, validator
  hostnames or private IPs, SSH/runbook controls, raw node logs, private RPC
  endpoints, credentials, keys, operator notes, or any data that can be used to
  operate infrastructure.
- The page must identify the package scope as testnet-only until the mainnet
  launch exchange handoff closes.
- The page must be verified by
  `python3 scripts/qa/exchange_public_readiness.py --scope testnet --status-approved --release-tag-selected`
  before exchange outreach. The readiness gate defaults the status URL to
  `https://exchanges.lichen.network` and rejects the admin monitoring host.

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

Operator correction on 2026-07-02: the internal monitoring surface is
admin-only and must not be published in exchange-facing material. The current
package uses the dedicated public portal at `https://exchanges.lichen.network`,
which is active on Cloudflare Pages and passed the default public readiness gate
on 2026-07-05. During active exchange-impacting incidents, acknowledge through
the approved aliases and continue status-page updates at least every 2 hours
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
v0.5.223
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

Historical rollback release subset verified on 2026-07-01:

- `https://github.com/lobstercove/lichen/releases/tag/v0.5.221` returned HTTP
  `200`.
- GitHub API reports `tag_name = v0.5.221`, `draft = false`, and
  `prerelease = false`.
- `SHA256SUMS` and `SHA256SUMS.sig` downloaded from the release.
- `scripts/verify-release-checksums.mjs` against the downloaded `v0.5.221`
  release artifacts
  verified signer `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk`.

Historical signed recovery release verified on 2026-07-01:

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

Current testnet release: `v0.5.224`. Current rollback anchor: `v0.5.223`, retained
until a newer signed rollback point is explicitly recorded.

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

Current rule: keep `v0.5.223` as the rollback anchor until a newer signed rollback
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
   `getAccountTxCount`, and representative old/recent
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
  `getTransactionsByAddress`, and `getAccountTxCount`.
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
`exchange-testnet-v0.5.221`. Exchange contact aliases were approved on
2026-07-01 as `security@lichen.network`, `exchange-ops@lichen.network`, and
`business@lichen.network`. The public exchange status-page gate was reopened on
2026-07-02 because the previously referenced monitoring surface is admin-only.
The replacement Cloudflare Pages project `lichen-network-exchanges` is active
on `https://exchanges.lichen.network`. On 2026-07-05, Cloudflare Pages reported
the custom domain as active, the public status page returned HTTP `200`, the
same-origin `/api/rpc` status proxy returned public testnet
`getHealth.status = ok`, and the default public readiness gate passed.
Mainnet RPC/WS readiness is deferred to the mainnet launch exchange handoff gate
and must be rechecked with the public readiness gate in `--scope full` mode
before mainnet is added.

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
- Four-validator health/status output.
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

No open operations blockers remain for the current testnet-only exchange
package. Mainnet remains out of scope until the mainnet launch exchange handoff
gate closes.

## Resolved Operations Checks

| ID | Check | Evidence |
| --- | --- | --- |
| O-01 | Status page URL approval superseded | Superseded on 2026-07-02: internal monitoring is admin-only and must not be used as the public exchange status page |
| O-02 | Incident aliases approved | Operator approved `security@lichen.network`, `exchange-ops@lichen.network`, and `business@lichen.network` on 2026-07-01; critical acknowledgement, active update, maintenance notice, emergency exception, authenticated outbound, and backup-path policy are recorded above |
| O-15 | Public exchange status page active | `https://exchanges.lichen.network` is active on Cloudflare Pages, serves the exchange-safe status page, uses a same-origin read-only status RPC proxy, and passed `python3 scripts/qa/exchange_public_readiness.py --scope testnet --status-approved --release-tag-selected --report /tmp/lichen-exchange-public-readiness-exchanges-domain-20260705-post-status-logic-green.json` |
| O-03 | Current release drift for core docs and Rust SDK pin | Core/CLI `0.5.224`, Rust client SDK `0.1.6`, contract SDK `1.0.3`, and JS SDK `1.0.6` are published; `v0.5.223` is the rollback anchor |
| O-04 | Final external exchange-package release URLs attached | Package release `https://github.com/lobstercove/lichen/releases/tag/exchange-testnet-v0.5.221` contains `lichen-exchange-testnet-v0.5.221.tar.gz` and `SHA256SUMS` |
| O-05 | Local archive/history behavior | Core and RPC archive regressions passed after hot-to-cold migration and reopen |
| O-07 | Local cleanup evidence | Local stack stop/status/process checks passed; generated credentials, state dirs, manifests, and staging dirs were removed after the local exchange simulation |
| O-09 | Rollback release checksum/signature verification | `v0.5.223` release checksum and detached PQ signature were verified against `deploy/release-trust-anchor.json` |
| O-11 | June 29 live testnet consensus incident recovery evidence preserved | Operator-approved evidence-preserving recovery restarted only stale validator `15.204.229.189`; the June 30 recurrence is tracked separately in `docs/deployment/TESTNET_RECOVERY_INCIDENT_2026-06-30.md`; signed `v0.5.217` restored testnet liveness, and signed `v0.5.219` completed the faucet-signing and exchange-simulation follow-up |
| O-12 | Signed `v0.5.221` testnet recovery release verification | Release artifacts and detached PQ signature verified; all four live hosts installed matching validator, custody, and faucet binaries through the runbook verify-only gate |
| O-13 | Public testnet exchange simulation | Public faucet-backed simulation passed on `https://testnet-api.lichen.network` and wrote `tests/artifacts/exchange-simulation-public-testnet-v0.5.221.json`, covering funding, deposit detection, finalized transaction lookup, account history, operational buffers, sweep, withdrawal, CLI smoke, and reconciliation |
| O-14 | Current package scope | External package is explicitly testnet-only until mainnet launch; mainnet RPC/WS and metadata are launch placeholders and require the mainnet launch exchange handoff gate plus `exchange_public_readiness.py --scope full` before inclusion |
| O-16 | Signed `v0.5.224` archive-parity rollout | All four validators run the exact signed artifact from preserved state; source-backed manifests match, public RPC/WS and exchange readiness pass, and the existing testnet legacy-hole waiver remains explicit and non-transferable |
| O-10 | Public developer portal exchange page | `developers/` was deployed to Cloudflare Pages project `lichen-network-developers`; public `https://developers.lichen.network/exchange-integration` contains inline exchange metadata, deposit and withdrawal cookbooks, finality/archive policy, operations contacts, validation gates, mainnet handoff, release-tagged source links, and `testnet-only` |
| O-06 | Exchange-specific rollback procedure | Rollback policy now includes exchange notification, pause/resume guidance, affected slot recording, signed-release-only rollback, archive/history verification, pending transaction reconciliation, and recovery notice requirements |
