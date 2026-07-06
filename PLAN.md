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
| (b) headless agent CLI | ✅ `claude` 2.1.201 at /Users/pedro/.local/bin/claude (non-interactive `-p` available); ✅ **Copilot CLI probed headless** (`copilot -p` returns; cofounder-approved engine for non-fable tasks 2026-07-06) |
| (c) agent continuation | ✅ SendMessage resume verified |
| (d) background/parallel | ✅ background spawn + notification verified |

**Classification: `capabilities: full`.**

## Preflight status
- ✅ Repo + origin (`pedrofuentes/bla`, public, MIT), git author `Pedro Fuentes <git@pedrofuent.es>`
- ✅ Branch protection on `main`: PR required, 0 approvals (agent can merge), no force-push/deletion. Sentinel-in-CI + harness-guard contexts to be added to required checks when the workflows land (M1 CI increment).
- ✅ Labels: ready/blocked/needs:decision/decision:approved/claimed:agent/sentinel:*/security/bug:confirmed/polish/stale
- ✅ Scanners: Dependabot alerts + automated security fixes enabled; secret scanning on (public repo). CodeQL default setup deferred until code lands (no supported language in repo yet — enable in the M1 CI increment).
- ✅ **Project board**: [users/pedrofuentes/projects/8](https://github.com/users/pedrofuentes/projects/8), linked to the repo, Status options Todo · In Progress · Blocked · Pending Decision · Done; issues #1/#4/#5 on board (preflight #2 closed — scope granted 2026-07-06).
- ⛔ **Distinct agent identity / Tier-2**: not provisioned (accepted — attended mode). Required before any unattended operation; re-raise as its own gate before enabling Tier-2.

## Delegation ledger
| Artifact / increment | Producer | Red-team / reviewer | Note |
|---|---|---|---|
| PRD.md (PR #3) | pm-prd sub-agent (sonnet) ≠ Lead | redteam-prd sub-agent (fable) ≠ producer; Sentinel (fable) ≠ all | done — red-team round 2 PASS; Sentinel CONDITIONAL SNTL-20260706-bla-PR3-105a52d @ 105a52d; follow-ups #4 #5 filed; awaiting cofounder merge |
| Engine-policy chore (PR #6) | Lead (ops chore, non-gate artifact) | Sentinel (fable) ≠ author | Sentinel round 1: REJECTED (1 🔴 containment) → fixed + follow-ups folded; re-invoke pending |

## Fleet registry
| Agent | Channel | Task | State |
|---|---|---|---|
| probe-1 | built-in, background | capability probe | done (LEVEL1-OK, nested launch OK) |
| pm-prd | built-in, background, sonnet | author PRD.md on branch docs/prd (PR #3) | done — final HEAD 105a52d |
| redteam-prd | built-in, fable | red-team the PRD gate artifact | done — round 2 PASS at HEAD 105a52d |
| sentinel-pr3 | built-in, fable | Sentinel review of PR #3 @ 105a52d | done — CONDITIONAL SNTL-20260706-bla-PR3-105a52d |
| sentinel-pr6 | built-in, fable | Sentinel review of PR #6 | round 1 REJECTED @ 23f9e9d; delta re-review pending |

## Lead lease
- Session: interactive CLI session e3f8b683 (scratchpad id), leased 2026-07-05. Refresh every tick; successor takes over only on a stale lease (>2 tick intervals).

## HANDOFF (for a cold successor: read docs/KICKOFF.md + MISSION.md + this block)
- **Where we are:** Phase 1 PRD gate substantively passed: PR #3 red-teamed (round 2 PASS) + Sentinel CONDITIONAL (SNTL-20260706-bla-PR3-105a52d @ 105a52d), follow-ups filed (#4 🟡, #5 🟢), PR annotated. **Merge awaits cofounder authorization** (runtime permission classifier blocks fleet merges; cofounder offered one-off merge or a standing permission rule).
- **Open gates:** (1) cofounder merge of PR #3 (PRD — fully gated, ready); (2) cofounder merge of PR #6 (harness-integrity engine policy — after Sentinel delta re-review passes); optional distinct agent identity for Tier-2 (future). **Copilot containment precondition** (MISSION §7): no Copilot implementer spawns until Sentinel-in-CI + harness-guard are required checks.
- **Open increments:** none — no product code yet. No untriaged issues; security 0/0/0.
- **Armed schedules:** Tier-1 watchdog via in-session cron (~20 min ticks, session-only; re-arm on new session per CONTINUOUS-OPERATION.md §Starting & restarting).
- **Engine policy:** Copilot CLI (headless) for implementation + mechanical work; Claude fable for architecture/Sentinel/native-integration work (MISSION §7, cofounder-approved 2026-07-06).
- **Single next action:** on merge of PR #3 → seed the board with M1 issues from PRD ACs + all DoD cards, then start M1 Phase-3 (architecture ADRs) and the first build increments.
- **Lease refreshed:** 2026-07-06 board-setup turn (session e3f8b683).
