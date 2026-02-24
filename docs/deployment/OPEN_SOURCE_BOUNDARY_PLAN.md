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

- [ ] Build explicit allowlist/denylist in repo policy file.
- [ ] Move private frontends to private repository.
- [ ] Remove private scripts/artifacts from tracked git history.
- [ ] Enforce `.gitignore` for keys, env files, generated artifacts, logs.
- [ ] Run secret scan before publish (`gitleaks`/equivalent).
- [ ] Validate docs for private links and local absolute paths.
- [ ] Re-run tests from clean clone of publish candidate.

## Git Hygiene Steps

1. Create temporary release branch for open-source candidate.
2. Remove/relocate non-public folders per denylist.
3. Run full test gate on candidate.
4. Run secret/history scan.
5. Open review checklist before push.

## Decision Needed

Team must finalize exact list of frontend folders to keep private (current request indicates all frontends should be private in open-source repo).
