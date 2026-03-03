# RustSec Triage - Feb 24, 2026

## Scope

This document records the Rust dependency advisory triage used for release gating.
Policy is enforced by `.cargo/audit.toml`.

## Decision

Release gate accepts advisories listed below under explicit risk acceptance with mitigations and follow-up migration work.

## Accepted Advisory Set

- `RUSTSEC-2025-0055` (`tracing-subscriber`): log poisoning via ANSI sequences.
  - Mitigation: structured logs, non-interactive log ingestion, no privileged terminal parsing.
  - Follow-up: upgrade dependency chain to `tracing-subscriber >=0.3.20`.

- `RUSTSEC-2023-0089` (`atomic-polyfill`): unmaintained.
- `RUSTSEC-2024-0370` (`proc-macro-error`): unmaintained.
- `RUSTSEC-2024-0388` (`derivative`): unmaintained.
- `RUSTSEC-2024-0436` (`paste`): unmaintained.
- `RUSTSEC-2025-0057` (`fxhash`): unmaintained.
- `RUSTSEC-2025-0134` (`rustls-pemfile`): unmaintained.
- `RUSTSEC-2025-0141` (`bincode`): unmaintained.
  - Mitigation: usage is in controlled protocol/runtime paths and covered by E2E gates; migration backlog retained for replacements.

- `RUSTSEC-2026-0012` (`keccak`): unsoundness in opt-in ARMv8 assembly backend.
  - Mitigation: release target and CI path are x86_64; vulnerable backend not enabled in release pipeline.

- `RUSTSEC-2026-0002` (`lru`): `IterMut` stacked-borrows unsoundness.
  - Mitigation: no reliance on `IterMut` in audited runtime paths; gate tests and memory-safe Rust ownership checks remain in place.

## Revalidation Command

```bash
cargo audit -q
```

Expected release behavior: advisories above are ignored by policy and do not fail release audit.
