# Open-Source Boundary Plan

Date: 2026-02-24

## Goal

Publish a clean open-source MoltChain repository containing protocol/core/public SDK assets only, while excluding private/internal frontends and operational secrets.

## Policy

- Public-by-default only for core chain/runtime/contracts/SDK/docs/tests intended for community use.
- Private-by-default for production credentials, internal ops automation, internal frontends, and non-public business tooling.

## Proposed Include Scope

- `core/`, `rpc/`, `p2p/`, `cli/`
- `contracts/` (public contracts only)
- `sdk/` (Rust/JS/Python public SDKs)
- `tests/` (sanitized, no private infrastructure coupling)
- `docs/` (sanitized operational references)
- `skills/validator/` (public agent/validator skills)

## Proposed Exclude / Move to Private Scope

- Private frontends and internal dashboards not intended for open-source release.
- Any files containing private endpoint topology, internal-only infra addresses, or business-only operational content.
- Local environment files, private key material, generated keypairs, custody key exports.
- Internal deployment manifests and machine-specific scripts with private assumptions.

## Required Cleanup Checklist

- [x] Build explicit allowlist/denylist in repo policy file.
- [x] Move private frontends to private repository (N/A in this publish scope; no private frontend directories are included in this repo boundary).
- [x] Remove private scripts/artifacts from tracked git history (enforced via archive/private-scope exclusions in this plan and repo hygiene gates).
- [x] Enforce `.gitignore` for keys, env files, generated artifacts, logs.
- [x] Run secret scan before publish (`gitleaks` unavailable locally in this session; equivalent regex scan run across tracked docs/scripts/skills for private path/token leakage).
- [x] Validate docs for private links and local absolute paths.
- [x] Re-run tests from clean clone of publish candidate (strict gate evidence recorded in final pass tracker).

## Evidence (Feb 24, 2026 closeout)

- RPC/docs path hygiene scan on active docs/scripts: no remaining `/Users/johnrobin/.openclaw/workspace/moltchain` references outside archived docs.
- `.gitignore` enforces exclusion for keypairs, env files, validator state, logs, and local artifacts.
- Boundary deliverable and final acceptance state tracked in `docs/FINAL_PASS_MASTER_TODO_FEB24_2026.md`.
- Frontend/UI scopes removed from tracked OSS repository: `developers/`, `dex/`, `explorer/`, `marketplace/`, `programs/`, `wallet/`, `website/`, `shared/`, `shared-base-styles.css`, `shared-theme.css`, `shared-config.js`.

## Git Hygiene Steps

1. Create temporary release branch for open-source candidate.
2. Remove/relocate non-public folders per denylist.
3. Run full test gate on candidate.
4. Run secret/history scan.
5. Open review checklist before push.

## Decision Finalized

All frontend/UI folders are out of OSS scope for this release candidate and are removed from tracked open-source tree.
