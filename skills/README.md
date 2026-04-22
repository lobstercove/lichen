# Lichen Skills

This directory holds repo-local skills for repeatable agent work inside the Lichen workspace.

These skills are intentionally smaller and more execution-oriented than the root `SKILL.md`.
Use them when you want step-by-step task flow instead of the full protocol reference.

## Available Skills

### Workspace Skill

- Path: [workspace/SKILL.md](workspace/SKILL.md)
- Use for: session bootstrap, repo navigation, validation planning, and handover hygiene
- Read first when you are entering the repo cold or after compaction

### Validator Skill

- Path: [validator/SKILL.md](validator/SKILL.md)
- Use for: local validator bring-up, validator operations, and operator workflows
- Supplement with: `docs/deployment/PRODUCTION_DEPLOYMENT.md` for VPS and production paths

## Related References

- [AGENTS.md](../AGENTS.md): compact workspace bootstrap
- [memories/repo/README.md](../memories/repo/README.md): tracked repo memory layer
- [SKILL.md](../SKILL.md): exhaustive protocol and operator reference
- [docs/README.md](../docs/README.md): human-readable docs hub

## Adding New Skills

Preferred layout:

```text
skills/<skill-name>/
├── SKILL.md
├── README.md              # optional human overview
├── scripts/               # optional helpers
└── examples/              # optional examples
```

Guidelines:

- Keep skills task-oriented and short enough to survive compaction
- Link to source-of-truth docs instead of copying large reference sections
- Prefer validation commands and exact file paths over general advice
- Update this index when adding or removing a tracked skill
