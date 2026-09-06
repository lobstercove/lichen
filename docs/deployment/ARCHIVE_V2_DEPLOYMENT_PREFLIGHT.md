# Archive V2 deployment and recovery gates

Applies to devnet, testnet, and mainnet. Updated 2026-09-06 from the signed
v0.5.280 Testnet rollout. This is an operating procedure, not a declaration that
the rollout passed. Existing network-specific authorization and state policy
still apply. Never copy a validator database, WAL, or private identity to peers.

## Deployment record and ownership

Before operating, create one dated deployment record containing the network ID,
genesis hash, immutable release tag/commit, complete workflow results, artifact
and detached PQ-signature verification, validator/archive-tool hashes, compatible
rollback hashes, fleet addresses, and exact operator source hashes. Record the
single controller identity and recovery mechanism. On resume, inspect actual
processes, services, timers, journals, and evidence before starting any writer.
An old success file or historical runbook version is not a fresh preflight.

Before each retry, review the complete selected execution path and run its
non-mutating preflight against the actual saved inputs. Check producer and
consumer bounds together, including minimum/maximum bytes, file counts,
capacity margins, inherited defaults, and existing-output behavior. Fixtures
must cover the observed failure and boundary values; shell syntax checks alone
do not establish that the plan is executable. On resume, compare immutable
WAL/key/configuration/file identities exactly, but evaluate changing measurements
such as filesystem free space against their explicit capacity gates. Seal the
reviewed source hashes and input hashes before starting the production operator.

All required release gates remain mandatory: formatting, locked workspace
clippy/tests, audit/deny, standalone contracts and genesis WASM, static
frontend/SDK/deployment QA, and the exact release's four-validator hot/cold,
fresh-join, outage, own-state restart, coordinated restart, and history parity
matrix. Deploy only signed tag-workflow artifacts from a clean release source.

Record UTC timestamps, source hashes and content-addressed evidence for each
gate. Keep failed logs and partial native results; do not replace them with a
later success. Record application acceptance separately from service health.

## Internal gateway TLS

The Archive V2 gateway is distinct from public RPC/WSS ingress. Its validator
client loads `LICHEN_ARCHIVE_V2_SOURCE_CA_CERT` as a trust anchor and sends the
protected bearer token to configured HTTPS object sources. The gateway must
serve a separate server certificate, not the root CA certificate itself.

1. Inspect the actual running gateway configuration and certificate on every
   host without logging credentials. Check validity dates, issuer/signature,
   SAN matching the configured URL, CA:FALSE basic constraints, serverAuth EKU,
   appropriate key usage, and the leaf's public-key match to its private key.
   A CA:TRUE certificate belongs in the trust store, not the server-leaf role.
2. Validate the staged gateway config with its protected environment. Preserve
   the old config and certificates, record hashes/ownership, and arm a bounded
   rollback before changing the active gateway. For ordinary renewal, sign a
   new server key/certificate with the existing CA and preserve validator trust
   configuration. CA rotation requires a separately qualified trust migration.
3. Wait for the actual HTTPS listener with a bounded retry deadline. A systemd
   Type=simple service can be active before its socket accepts connections.
   Retry startup connection failures only within the deadline; wrong HTTP
   status, certificate errors, configuration drift, and process restarts fail
   the gate. Do not disable certificate or hostname verification.
4. Run a read-only request using the exact release's reqwest/Rustls dependencies
   and features, with the configured CA and source URL. An unauthenticated
   request must complete TLS and return the expected 401. Bind the probe source,
   dependency lock, and build identity to the deployment record. A local probe
   is a diagnostic client; it is never a replacement production validator.
   Verify the diagnostic connection method before gateway mutation. US disables
   SSH TCP forwarding; use the authorized SSH command channel to its loopback
   gateway when necessary, without changing SSH access configuration. A tunnel
   connection reset does not establish a certificate failure.
5. From each validator host, authenticate to both sources using protected
   credentials and read complete catalog-selected object bytes. Require 200,
   exact byte count, and the catalog's object SHA-256. Include the historical
   object used by the deep-history acceptance test, not only a recent object.
   After activation, also require the signed validator's actual historical RPC
   read and source-outage/refetch tests. Transport success alone is insufficient.
6. Apply the config through a supported gateway reload or gateway-only restart.
   Check actual configuration as well as systemd capabilities: the September 6
   gateway had an ExecReload command but Caddy `admin off`, so reload failed.
   That configuration requires a gateway-only restart. Keep its admin interface
   disabled and repeat bounded listener readiness after the restart.
   Verify the operation did not change validator PID/invocation, binary hashes,
   state-directory identity, keys, genesis, or validator/source configuration.
   Clear the rollback timer only after the checks pass. On failure restore the
   exact old config, apply it through the supported gateway operation, verify service state, and preserve the
   failed evidence. Reject rollback if current or backup bytes have drifted.

On the current Testnet the internal endpoint is loopback port 19443 and URLs
use `/primary/objects/<sha256>.av2s` and `/replica/objects/<sha256>.av2s`.
Unauthenticated 401 is expected. Authenticated `/primary/catalog.av2` can return
404 because this HTTPS route exposes objects only; catalog availability is a
separate configured-source check. Derive routes from the actual config on a
new network rather than copying this deployment's assumptions.

The September 6 incident reproduced `CaUsedAsEndEntity` with the exact-tag
client against the live EU gateway. The old certificate had CA:TRUE and was
served directly. Earlier Python/OpenSSL full-object checks accepted it and
therefore missed the incompatibility. Local regression checks demonstrated
rejection of that certificate and a wrong-name leaf, and acceptance of a
CA:FALSE leaf signed by the existing trust model. The failed activation did not
record its precise failing shell line; do not claim a missing trace exists.

The historical `memories/repo/lichen-v262-archive-v2-https-gateway-stage.sh`
and `lichen-v262-archive-v2-https-gateway-stage-all4.sh` generate CA:TRUE
certificates and configure those same certificates as gateway server leaves.
They are incident provenance, not suitable fresh-network provisioning commands.
A future gateway installer must generate and configure a separate CA:FALSE
server leaf and pass the exact-client gate above before validators depend on it.

The gateway correction subsequently passed on all four Testnet hosts: eight
exact-client TLS/401 checks and eight complete, authenticated historical-object
readbacks, with unchanged validator processes and common finalized history.
Its evidence is SHA-256
`8561e91bdb866e9591a0fdaf17750408963b09600c0aa652bfde2bab982f6179`.
This closes the gateway certificate correction, not Archive V2 activation or
the remaining release-acceptance gates.

For renewal, record leaf and CA expiration, renewal owner and automation,
monitoring/alert thresholds, and a tested rollback. Renew before the remaining
validity becomes shorter than the deployment/recovery window. A 90-day leaf
requires renewal monitoring; installing it alone does not establish automation.
Run the exact-client and complete-object checks after every renewal.

## Capacity and catalog freshness

Run the signed native role preflight on every host against its actual disk and
role. Include checkpoint staging and steady-state requirements. A consensus
bootstrap reserve is not the checkpoint/Normal-capacity target. Record free
bytes, calculated required bytes, role, catalog hash/root/end, and finalized tip.
Recheck after long preflights or ordinary chain growth before activation.

Include both retained checkpoints and concurrent replacement staging in the
disk inventory. After the September 6 TLS correction, completed checkpoints
and replacement staging accounted for about 4–9.5 GB per host, leaving US,
SEA and India below the earlier admission target. Check filesystem free bytes
alongside checkpoint directories and active exports; `du` can double-count SST
hard links shared with live state. Do not delete active staging, pinned exports,
or live state files to make a preflight pass. A startup-time success must also
leave room for subsequent checkpoint replacement and archive reads.

For this v0.5.280 Testnet plan the four-host Normal target is 23,775,365,530
bytes: 23,238,494,618 required plus 536,870,912 margin. The prior SEA
15,185,430,938 bootstrap target was insufficient. This is deployment-specific,
not a universal constant for future networks. Preserve the 5,368,709,120-byte
reserve, 536,870,912 estimated-input cap, and one-range reclaim limit. Do not
raise limits or delete unsupported history to force admission.

Check every consumer of catalog coverage: native preflight, runtime admission,
and periodic checkpoint construction can have different recent-tail windows.
For the fixed v0.5.280 catalog ending at 12,290,190, native preflight with the
100,000-slot window and the observed default checkpoint settings reach their
coverage boundary at 12,390,190. Runtime admission's additional verified local
tail does not extend those other gates. Recompute from the selected release
source and actual configuration; publish/verify newer coverage and perform the
required coordinated catalog reload before crossing the earliest boundary.

Record each read-only object-store grant's actual issue/expiry time. An
idempotent refresh script can return an old grant's evidence. Renew at a safe
maintenance boundary and verify the new grant. Never remount under a native
maintenance process or delete R2 objects. Temporary headroom relocation is not
native archive reclaim and is not durable Mainnet storage provisioning.

## Coordinated activation and recovery

1. Finish TLS, source-byte, signed-artifact, catalog and rollback checks, and
   measure actual capacity while validators are still available. Pin the
   qualified operator sources and verify there is no other controller or
   maintenance writer. The qualified v0.5.280 activation runs its full native
   role preflight during the coordinated stop, before the role switch. Live
   capacity measurements do not replace that admission gate. Account for this
   preflight time in the maintenance window and recovery lease.
2. For consensus-critical deployment, stage first, stop all validators, prove
   they are stopped, then install the signed artifacts on all hosts. Preserve
   each host's state/WAL/keys, cold storage, identity and access configuration.
   Archive-role-only activation still follows its qualified coordinated plan.
3. Arm bounded recovery before mutation. Capture stopped WAL and directory
   identities. Retry existing gateway units only if the complete expected set
   is byte-identical, active, and stable. Partial units, symlinks, and unknown
   drift fail closed; do not overwrite them to make a retry pass.
4. Start all four from their own state. Require Archive V2 admission, explicit
   BFT entry, local COMMITTED events and current authorship on every validator.
   A startup message, advancing RPC tip, or three active producers does not
   prove the fourth is participating.
5. If three producers outpace the fourth's startup catch-up, diagnose the actual
   process state. Use only a qualified, bounded catch-up procedure with fresh
   PID/start/invocation pins, independent automatic resume guards, preserved
   stopped WAL, and verified restoration. Never reuse an earlier run's PIDs or
   weaken the signed admission gate.
6. Require a common finalized block and canonical commit certificate, deep
   historical RPC reads, both-source parity and outage/refetch, and absence of
   forbidden legacy FUSE runtime descriptors. Only the complete activation
   success marker closes this phase. Admission alone is not acceptance.
7. On failure capture the phase, host, failing line/exit status, sanitized RPC
   error and relevant service journal before recovery. Never log tokens,
   private keys, or environment contents. Restore the qualified baseline,
   validate signed installed/running parity and all four local commits, then
   seal recovery evidence. Keep the final activation gate open.

## Runtime query regression found on September 6

Signed v0.5.280 passed native admission and exact-client TLS, but the latest
activation failed its runtime gate and recovered to the preserved baseline.
During the failed run, the cache fetched large historical segments in ascending
order while fresh RPC timed out. Local reproduction against exact tag
`162a4c2c51385d6a1307490f749126c633d24040` confirms that
`get_recent_txs_paginated_exact_filtered` reads Archive V2 from slot zero even
when the requested page is already filled by newer hot transactions. Ordinary
`getRecentTransactions` traffic reaches all four validators. This is a confirmed
query defect and a supported explanation for the observed scan; the stripped
production stack alone does not identify the exact initiating request.

Before the next activation, qualify recent-query traffic with a populated hot
suffix and a much older catalog. Verify that a complete hot page fetches no
strictly older archive object, while partial pages, exclusive cursors and
overlapping boundary slots retain exact results. Run traffic during finality
and source-outage checks, and bound both request latency and archive work.
Also review other aggregate-history paths; a fix for recent transactions does
not by itself establish bounded account counts, activity queries or deep pages.
Any code correction requires a fresh signed workflow release and its complete
gates; never install the local diagnostic build or retag v0.5.280.

The corrected slow-source regression also confirms executor starvation on the
original tag: with a genuinely hot tip at slot 1 and a delayed authenticated
archive source at slot 0, an account-history query prevents a single-worker
runtime from answering getSlot within 250 ms. The candidate isolates database,
archive and VM RPC work in four bounded blocking jobs; its permit remains held
after a client disconnect until the job finishes. Capacity exhaustion returns
503 with JSON-RPC code -32005. This is candidate behavior, not an installed
v0.5.280 capability. Verify transaction submission and proof-lock behavior under
load as well as liveness before accepting a successor.

Aggregate queries must filter account/program/pair prefixes before accumulating
results across the catalog. The candidate authenticates disposable public-index
bundles against the catalog's original envelope, dictionary and frame hash.
Index and full-object cache files share one quota. An index cache hit does not
prove that an unavailable block body exists or that every byte of a complete
object is valid; whole-object verification and public-history parity remain
independent mandatory gates.

For a successor containing `prewarm-indexes`, use the signed archive utility
while the validator is stopped. Pin the actual network ID, genesis hash and
catalog root; provide separate existing catalog/source/cache directories,
`--cache-quota-bytes` and `--reserve-bytes`. Run the complete command first
with `--dry-run`, then with `--acknowledge-stopped-validator`. Both paths
authenticate every selected source index before writing. The command rejects
insufficient quota, requires free space for quota plus reserve, and execution
checks every cached entry after import. Preserve the native JSON results and
exact command/input hashes. Do not confuse the acknowledgment with an actual
service check: the controller must independently prove the validator is stopped.
Do not invoke this candidate-only command on the currently signed v0.5.280.

Recompute index size from every active manifest, including cache envelopes;
do not carry a preceding deployment's quota into a new plan. The September 6
388-segment catalog has 708,442,599 compressed public-index frame bytes, before
cache envelopes, and 1,129,847,239 uncompressed compact bytes. This fits the
current 2 GiB cache but does not establish a perpetual growth budget. Warming
indexes must be followed by timed aggregate/deep queries, normal transaction
traffic, current finality, and source-outage checks on the final signed release.

When collecting Caddy diagnostics, parse on the host and emit only approved
fields such as timestamp, status, duration and route. Caddy error records can
contain origin-auth headers: never return raw request/header records. Method
traffic diagnostics may retain method counts only, without bodies or headers.

## Retirement and full acceptance

After activation, execute the qualified source-backed, additive, resumable,
conflict-aborting retirement sequence. Preserve native journals and interrupted
results. Count only measured physical reclaim as reclaimed disk. Resume after a
safe operation boundary, not by interrupting a healthy native body pass. If the
plan freezes finality during maintenance, state that explicitly in the record.

Require four-validator finality/cadence, all-four authorship, one-validator
outage/rejoin, own-state restart, all-validator restart, installed/running
artifact parity, adequate disk capacity, deep historical reads and complete
genesis-to-tip logical public-history parity. The existing `lichen-testnet-1`
waiver covers only unavailable signed block bodies 2,872,006..4,298,999. It is
not transferable to devnet, fresh testnet or mainnet. Those networks fail closed
on incomplete history. Current 200 GB Testnet hosts are not approved for
Mainnet or indefinite archive growth.

Complete live DEX/CLOB/AMM, prediction, governance, launchpad/listing/graduation,
and SDK/frontend/platform acceptance with authorized test funds. Verify the
requested LICN price on the live chain and 0.15 defaults for new-network genesis;
never rewrite an existing genesis. Publish release status/frontends and perform
reference-checked cleanup only when the applicable acceptance gates pass.

The final deployment record must state passed, failed, and pending gates;
evidence paths/hashes; exact deployed identities; rollback; certificate and
credential renewal; capacity/catalog deadlines; and the next permitted action.
This makes a resumed deployment verifiable without treating a narrative status
or an old success marker as current fleet evidence.
