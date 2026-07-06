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

- **Date:** 2026-07-06 · **Status:** accepted (cofounder-approved; attested by the cofounder's merge of PR #6)
- **Decision:** Fleet work is routed per MISSION.md §7: Claude `fable` for architecture, Sentinel review, and native-integration work; headless Copilot CLI for implementation and mechanical work, with Claude `sonnet`/`haiku` fallbacks.
- **Containment:** per Sentinel SNTL-20260706-bla-PR6-23f9e9d, Copilot implementer spawns are blocked until Sentinel-in-CI + harness-guard are required checks on `main` (read-only Copilot work permitted immediately). Full policy text and precondition live in MISSION.md §7.

---

## ADR-0002 — Scaffold tooling choices (Tailwind v4, ESLint flat config, cargo-llvm-cov exclusions)

- **Date:** 2026-07-06 · **Status:** accepted (scaffold PR, issue #14)
- **Context:** `pnpm create tauri-app` (React-TS + Vite) needed Tailwind, lint/format tooling, and a coverage setup honoring MISSION.md §7's OS-glue exclusion, without introducing dependencies beyond MISSION §3 + create-tauri-app defaults + Tailwind.
- **Decision:**
  - Tailwind v4 via the `@tailwindcss/vite` plugin (single `@import "tailwindcss";` in `src/index.css`) — no `tailwind.config.js`/PostCSS pipeline needed, fewer moving parts than v3.
  - ESLint flat config (`eslint.config.js`) with `typescript-eslint`, `eslint-plugin-react-hooks`, `eslint-plugin-react-refresh`, and `eslint-config-prettier` — matches current ESLint/Vite ecosystem defaults.
  - `cargo-llvm-cov` invoked from `Makefile`'s `coverage` target with `--ignore-filename-regex 'src-tauri/src/(audio|output|hotkeys|context)\.rs'`, documented inline — keeps the coverage ratchet (MISSION §7, 70% floor) scoped to pure logic, per the OS-integration exemption in AGENTS.md.
  - `vitest.config.ts` sets `passWithNoTests: true` — the scaffold ships no behavior-bearing components yet; remove once the first `*.test.tsx` lands.
- **Alternatives considered:** Tailwind v3 + PostCSS config (more files, no benefit here); a single combined ESLint+Prettier legacy `.eslintrc` (deprecated upstream); a `.cargo/config.toml` for coverage exclusion (no such mechanism exists for cargo-llvm-cov — CLI flag is the supported path).
- **Consequences:** Future increments add real logic to `cleanup`/`store`/etc. under TDD and drop the module stubs' "no logic yet" doc comments as code lands; `passWithNoTests` should be removed in the first PR that adds a UI test.
