# Architecture Decision Records — bla

> **Record every significant technical decision here.** When choosing between approaches,
> document what was chosen and why. This prevents future agents and developers from
> re-debating settled decisions or accidentally reversing them.
>
> Do NOT write decisions to AGENTS.md — they belong here.

## Format

```markdown
### ADR-NNN: Decision Title
**Date**: YYYY-MM-DD
**Status**: Proposed / Accepted / Superseded by ADR-NNN
**Context**: What problem or question prompted this decision?
**Decision**: What was decided?
**Alternatives considered**: What other options were evaluated?
**Consequences**: What are the trade-offs? What does this enable or prevent?
```

## Decisions

<!-- Add new decisions below this line, most recent first -->

---

## ADR-0001 — Engine-per-task fleet policy (Copilot CLI for non-fable work)

- **Date:** 2026-07-06 · **Status:** accepted — attested by the cofounder's merge of PR #6 (merge commit `f0da1f9`, https://github.com/pedrofuentes/bla/pull/6)
- **Decision:** Fleet work is routed per MISSION.md §7: Claude `fable` for architecture, Sentinel review, and native-integration work; headless Copilot CLI for implementation and mechanical work, with Claude `sonnet`/`haiku` fallbacks.
- **Containment:** per Sentinel SNTL-20260706-bla-PR6-23f9e9d, Copilot implementer spawns are blocked until Sentinel-in-CI + harness-guard are required checks on `main`, **or the worker runs under a credential that structurally cannot push or merge to `main`** (read-only Copilot work permitted immediately). Canonical policy text and precondition live in MISSION.md §7.
