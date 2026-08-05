# Archive V2 Existing-Testnet Retrofit — 2026-08-05

## Status

`v0.5.237` is the signed, deployed catalog-schema-3 dual-reader anchor on all
four testnet validators. Archive V2 legacy retirement has not begun.

The first live bounded build proved that the mature testnet database predates
the atomic genesis-to-tip archive watermark. `v0.5.237` therefore fails closed
before collecting the range, as designed for ordinary networks, but cannot
start this exact legacy retrofit. `v0.5.238` adds a required
`--acknowledge-exact-testnet-missing-watermark` build flag. It is accepted only
for the pinned `lichen-testnet-1` identity and genesis; bounded source bodies,
parent links, finality depth, catalog order, deterministic reconstruction, and
replica gates are unchanged. No watermark is invented or backdated.

`v0.5.238` must pass the complete release gates, exact-commit CI, signed tag
workflow, provenance and detached PQ verification, and coordinated
four-validator deployment before any legacy row is eligible for deletion.

## Exact historical constraint

The existing `lichen-testnet-1` source permanently lacks original signed block
bodies `2,872,006..4,298,999`. This is the already-approved non-transferable
testnet waiver; it does not apply to mainnet or a fresh network.

- genesis hash:
  `f08308ef2520af0967120f3314fa95b14d8239a898d34a6993981cb93f740884`
- preceding slot/hash: `2,872,005` /
  `74e23fbbf02a56763497ada2c40606b94f6a24504764926adc1e40d080c7bd84`
- unavailable interval: `2,872,006..4,298,999`
- last unavailable hash, committed as the next block's parent:
  `250dc7792f94e8e7a2084ac0396b8e333e9e4fc8673efcbc74257253cbd4a483`
- following slot/hash: `4,299,000` /
  `af42961b53719845f1ac7b913f20c602bc520e274a52347d7d46ab92522ebbc1`

Catalog schema 3 commits that declaration into the catalog root. Validation
accepts only the exact network, genesis, interval, preceding hash, missing-tip
hash, and following hash above. Archive V2 lookups inside the interval remain
unavailable; no placeholder or synthetic body is created. All other catalog
gaps fail closed.

## R2 topology

Two private Cloudflare R2 buckets provide independent object copies:

- `lichen-testnet-archive-v2-primary` (`enam`)
- `lichen-testnet-archive-v2-replica` (`apac`)

Validators receive only short-lived, bucket/prefix-scoped temporary S3
credentials. The account API token remains on the operator machine. Credential
material is staged only in tmpfs and removed after each pass. Every uploaded
segment, manifest, and catalog is downloaded or hashed independently before a
replica acknowledgement is admitted into retirement evidence.

R2 is an archive replica and verified-cache source, not a consensus input.
Consensus continues from local state and WAL during object-store outages.

## Segment-by-segment migration

The low-space fleet cannot create a second genesis-to-tip copy. Migration is
therefore bounded and additive:

1. Stop one source validator while the other three retain quorum.
2. Build one finalized Archive V2 range in tmpfs from its preserved hot/cold
   state. The exact mature testnet invocation includes
   `--acknowledge-exact-testnet-missing-watermark`; the local build remains
   deterministic and resumable.
3. Verify the segment locally, upload it to both R2 buckets, verify both remote
   objects and manifests, and publish the append-only catalog extension.
4. Keep the verified segment available locally in tmpfs and create a signed
   retirement manifest bound to the exact catalog root, segment roots, two R2
   failure domains, and the deployed dual-reader release anchor.
5. Run bounded journaled tombstone passes for only that segment's exact rows.
   A crash resumes from the durable pending/progress journal.
6. Run bounded physical reclaim only when its estimated peak remains above the
   5 GiB testnet floor. If headroom is insufficient, stop; never lower the
   floor.
7. Install the smaller verified V2 object locally, reverify both RPC parity and
   the object, restart the validator, and prove catch-up before continuing.
8. Repeat for each validator. A validator proves equivalence against its own
   legacy copy before retiring its rows.

At the historical interval boundary, catalog the exact loss declaration after
the last pre-gap segment, then resume construction at slot `4,299,000` only
after verifying its live hash and parent commitment.

## Rollback and abort conditions

Before retirement, `v0.5.237` remains the signed rollback anchor and legacy
storage stays authoritative. After retirement starts, rollback is permitted
only to a signed release that understands catalog schema 3 and Archive V2
segments; the retirement command requires this acknowledgement explicitly.

Abort without deleting more rows on any catalog-root drift, source parity
mismatch, replica disagreement, failed restore, missing segment, signature or
release-anchor mismatch, capacity-floor decision, validator identity change,
WAL/state inconsistency, or fixed-boundary divergence. Preserve all journals,
manifests, catalog bytes, checksums, signatures, and release attestations as
incident/recovery evidence.
