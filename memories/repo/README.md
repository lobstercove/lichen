# Repo Memory

This directory is the tracked, compact context layer for future sessions.
It exists so agents do not have to start every task by re-reading the full root `SKILL.md`.

## Files

- `project-map.md`: stable repo topology, entrypoints, and validation commands
- `current-state.md`: current status, active priorities, and known cross-doc drift
- `gotchas.md`: cross-cutting pitfalls worth remembering
- `session-handovers/`: dated handovers for unfinished or recently completed work

## Editing Rules

- Keep these files short and factual.
- Prefer source-backed facts over aspirations.
- Include exact dates when status differs across documents.
- Move per-session detail into `session-handovers/` instead of bloating `current-state.md`.
- If a fact changes in code or deployment, update the relevant memory file in the same change.
