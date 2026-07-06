# PLAN — bla (Lead working state)

> Operational state for the autonomous build (docs/KICKOFF.md). Not product documentation.

## Runtime fingerprint
- Runtime: Claude Code CLI 2.1.201, interactive session (Lead = Claude Fable 5, `claude-fable-5`)
- Host: macOS (Darwin 25.5.0), /Users/pedro/Projects/bla
- Date armed: 2026-07-05
- Acting identity: `pedrofuentes` (== cofounder) → **attended single-operator mode** per MISSION.md §7: Tier-1 only, no unattended Tier-2; gate answers via live CLI or bounded-trusted board channel (self-signature + cofounder-login + solo-repo).

## Capability matrix (Phase-0 probe)
| Channel | Status |
|---|---|
| (a) built-in subagent, level-1 | ✅ works (haiku probe returned) |
| (a) nested (level-2) | ✅ probe agent issued a nested spawn without error; final reply pending — treat as working, re-verify on first real nested use |
| (b) headless agent CLI | ✅ `claude` 2.1.201 at /Users/pedro/.local/bin/claude (non-interactive `-p` available) |
| (c) agent continuation | ✅ SendMessage resume verified |
| (d) background/parallel | ✅ background spawn + notification verified |

**Classification: `capabilities: full`.**

## Preflight status
- ✅ Repo + origin (`pedrofuentes/bla`, public, MIT), git author `Pedro Fuentes <git@pedrofuent.es>`
- ✅ Branch protection on `main`: PR required, 0 approvals (agent can merge), no force-push/deletion. Sentinel-in-CI + harness-guard contexts to be added to required checks when the workflows land (M1 CI increment).
- ✅ Labels: ready/blocked/needs:decision/decision:approved/claimed:agent/sentinel:*/security/bug:confirmed/polish/stale
- ✅ Scanners: Dependabot alerts + automated security fixes enabled; secret scanning on (public repo). CodeQL default setup deferred until code lands (no supported language in repo yet — enable in the M1 CI increment).
- ⛔ **Project board**: token lacks `project` scope → BLOCKED issue filed; running on labels only until granted (`gh auth refresh -s project`).
- ⛔ **Distinct agent identity / Tier-2**: not provisioned (accepted — attended mode). Required before any unattended operation.

## Delegation ledger
| Artifact / increment | Producer | Red-team / reviewer | Note |
|---|---|---|---|
| PRD.md | (pending — PM sub-agent) | (pending — different sub-agent) | Phase 1 in progress |

## Fleet registry
| Agent | Channel | Task | State |
|---|---|---|---|
| a0bf2b1e67349d8d6 | built-in, background | capability probe | reporting |

## Lead lease
- Session: interactive CLI session e3f8b683 (scratchpad id), leased 2026-07-05. Refresh every tick; successor takes over only on a stale lease (>2 tick intervals).

## HANDOFF (for a cold successor: read docs/KICKOFF.md + MISSION.md + this block)
- **Where we are:** Phase 0 complete except board (blocked on `project` scope). Phase 1 (PRD + board seeding) starting.
- **Open gates:** BLOCKED preflight issue (project scope; optional agent identity). No DECISION gates yet.
- **Open increments:** none — no product code yet.
- **Armed schedules:** Tier-1 watchdog via in-session cron (~20 min ticks, session-only; re-arm on new session per CONTINUOUS-OPERATION.md §Starting & restarting).
- **Single next action:** PM sub-agent authors PRD.md (red-team by a second agent), then seed board/issues for M1 once `project` scope is granted; meanwhile file M1 issues with labels only.
