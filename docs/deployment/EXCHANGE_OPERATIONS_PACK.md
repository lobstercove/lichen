# Lichen Exchange Operations Pack

**Status:** Draft, not approved for external listing package
**Created:** 2026-06-29
**Integration guide:** [../guides/EXCHANGE_INTEGRATION.md](../guides/EXCHANGE_INTEGRATION.md)
**Metadata sheet:** [../guides/EXCHANGE_CHAIN_METADATA.md](../guides/EXCHANGE_CHAIN_METADATA.md)
**Tracker:** [../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md](../strategy/EXCHANGE_LISTING_READINESS_TRACKER.md)
**Rollback anchor:** `v0.5.215`, per operator note on 2026-06-29

This pack defines the operational material an exchange expects before listing
native LICN. It is intentionally incomplete where operator approval or live
evidence is still missing.

## Publication Gate

Do not include this pack in an external listing package until:

- Incident contact aliases are approved.
- Status page URL is public and monitored.
- Public RPC and WebSocket endpoints are healthy from outside the operator
  network.
- Upgrade and rollback policy is confirmed against the signed release process.
- Archive retention and repair procedures are tested against the public target
  network.
- A public testnet exchange simulation passes after local validation.
- Public testnet exchange run passes after the local gates.

## Service Surfaces

| Surface | Current value | Status | Source |
| --- | --- | --- | --- |
| Mainnet RPC | `https://rpc.lichen.network` | Live check failed on 2026-06-29: Cloudflare `525` | `seeds.json`, `developers/shared-config.js` |
| Mainnet WebSocket | `wss://rpc.lichen.network/ws` | Live check failed on 2026-06-29: Cloudflare `525` | `developers/shared-config.js` |
| Testnet RPC | `https://testnet-rpc.lichen.network` | Recovered on 2026-06-30 after signed `v0.5.217` rollout; twelve-sample watch showed public/local `health.status = ok` through public slot `6715694`. Final package should use the clean `v0.5.219` faucet-signing and exchange-simulation follow-up release after it passes. | `seeds.json`, `developers/shared-config.js`, tracker Phase 5 evidence |
| Testnet WebSocket | `wss://testnet-rpc.lichen.network/ws` | Transport previously upgraded; rerun application slot-subscription smoke after the final signed release is installed. | `developers/shared-config.js` |
| Explorer | `https://explorer.lichen.network` | Route templates verified | `seeds.json`, `developers/shared-config.js`, `explorer/js/*.js` |
| Status page | Candidate: `https://monitoring.lichen.network` | Public monitoring page reachable; not operator-approved as exchange status page | Operator decision required |
| Developer portal exchange page | `https://developers.lichen.network/exchange-integration.html` | Public path returns generic developer hub fallback and misses the required exchange content snippets | Deploy/publish developer portal update after local gates |
| GitHub exchange docs | `docs/guides/EXCHANGE_INTEGRATION.md` | Draft | Phase 1 docs work |

## Incident Contacts

Do not publish personal emails, private keys, private RPC URLs, or ad hoc chat
handles in exchange docs.

Required external aliases:

| Alias | Purpose | Status |
| --- | --- | --- |
| Security incident alias | Vulnerability, compromise, signing, or fund-safety issue | Blocked on operator approval |
| Exchange operations alias | Deposits, withdrawals, RPC/archive, maintenance coordination | Blocked on operator approval |
| Business/listing alias | Listing paperwork and relationship management | Blocked on operator approval |
| Status page | Public incident and maintenance updates | Blocked on operator approval |

Required escalation fields before publication:

- Acknowledgement target for critical incidents.
- Update cadence during active incidents.
- Maintenance notice period.
- Emergency exception policy.
- PGP/PQ signing or authenticated-contact policy if used.

## Status Page Policy

The status page must report, at minimum:

- Mainnet RPC availability.
- Mainnet WebSocket availability.
- Testnet RPC availability.
- Explorer availability.
- Archive/history health.
- Validator/finality status.
- Known deposit/withdrawal-impacting incidents.
- Planned upgrades and maintenance windows.

If the final listing package includes optional ecosystem context beyond native
LICN deposits, the same status surface must separately report:

- Custody/bridge route health and reserve visibility for wrapped assets.
- DEX API and route availability.
- Oracle feed freshness and stale-feed incidents.

These optional ecosystem surfaces must not be presented as prerequisites for
native LICN deposits or withdrawals.

The candidate public monitoring surface `https://monitoring.lichen.network`
returned HTTP `200` on 2026-06-29 and served the `Lichen Mission Control -
Network Monitoring` page. This is not enough for exchange publication until an
operator approves it as the official status page and defines update cadence,
incident ownership, and maintenance-window policy.

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
v0.5.215
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

Verified rollback release subset on 2026-06-29:

- `https://github.com/lobstercove/lichen/releases/tag/v0.5.215` returned HTTP
  `200`.
- GitHub API reports `tag_name = v0.5.215`, `draft = false`,
  `prerelease = false`, and `published_at = 2026-06-29T00:47:53Z`.
- `SHA256SUMS` and `SHA256SUMS.sig` downloaded from the release.
- `node scripts/verify-release-checksums.mjs /tmp/lichen-v05215-release`
  verified signer `8HitBNnh8qbhfne5NCv2yHrQFoD6xbmHcWaUSgCGtsk`.

Pending blocker: publish exact exchange-package release URLs only after the
signed target release and public exchange docs package are selected.

Current recovery candidate: `v0.5.219`, with `v0.5.215` retained as the rollback
anchor until a newer signed rollback point is explicitly recorded.

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

Current rule: keep `v0.5.215` as the rollback anchor until a newer signed rollback
point is explicitly recorded.

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

Pending blocker: define exact exchange notice cadence after status page and
incident aliases are approved.

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

The public exchange package is still not approved because mainnet RPC/WS checks
return Cloudflare `525`, the public developer exchange page is not deployed,
the status page is not operator-approved, and the final signed exchange package
tag is not selected.

Update on 2026-06-30: the public testnet was stale again at slot `6715444`
while all four services remained active. The signed `v0.5.217` rollout was
installed non-destructively on all four validators, preserving state, cold
archives, WAL, keys, and peer identity. The chain resumed finality; a
twelve-sample watch showed public/local `health.status = ok` through public slot
`6715694`. The clean exchange-facing release candidate is now `v0.5.219`, which
keeps the consensus liveness fix and refreshes `anyhow` to the patched
`1.0.103` lockfile version so Cargo Audit/Deny pass.

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
| O-01 | Status page URL not final | Verify deployed monitoring/status surface or select status provider |
| O-02 | Incident aliases not approved | Define public aliases and escalation windows |
| O-04 | Final exchange-package release URLs not attached | Confirm target release artifacts after public endpoint blockers close |
| O-06 | Rollback runbook not exchange-specific | Add exchange notification/reconciliation steps |
| O-08 | Public RPC and WebSocket endpoints not fully exchange-ready | Fix mainnet Cloudflare `525` or explicitly scope the gate to testnet until mainnet launch; rerun health, fee, WS, archive, and simulation checks after public developer-page deployment |
| O-10 | Public developer portal exchange page is not deployed | Publish developer portal update and verify the public URL contains `Exchange Integration`, `Exchange Integration Guide`, `Exchange Chain Metadata`, and `Exchange Operations Pack` |

## Resolved Operations Checks

| ID | Check | Evidence |
| --- | --- | --- |
| O-03 | Current release drift for core docs and Rust SDK pin | Core crates and the Rust SDK pin were updated to `0.5.219`; `v0.5.215` remains the rollback anchor; JS/Python package boundaries are documented in the tracker |
| O-05 | Local archive/history behavior | Core and RPC archive regressions passed after hot-to-cold migration and reopen |
| O-07 | Local cleanup evidence | Local stack stop/status/process checks passed; generated credentials, state dirs, manifests, and staging dirs were removed after the local exchange simulation |
| O-09 | Rollback release checksum/signature verification | `v0.5.215` release checksum and detached PQ signature were verified against `deploy/release-trust-anchor.json` |
| O-11 | June 29 live testnet consensus incident recovery evidence preserved | Operator-approved evidence-preserving recovery restarted only stale validator `15.204.229.189`; the June 30 recurrence is tracked separately in `docs/deployment/TESTNET_RECOVERY_INCIDENT_2026-06-30.md`; signed `v0.5.217` restored testnet liveness, and `v0.5.219` is the clean faucet-signing and exchange-simulation follow-up candidate |
