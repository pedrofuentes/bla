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
- Session: interactive CLI session e3f8b683 (scratchpad id), leased 2026-07-05; **refreshed 2026-07-08** (M1-code-complete turn). Refresh every tick; successor takes over only on a stale lease (>2 tick intervals).

## HANDOFF (for a cold successor: read docs/KICKOFF.md + MISSION.md + this block)
- **Where we are (2026-07-08):** **All 11 core M1 issues (#16–#26) merged** — hotkeys, audio, STT (whisper behind a default-off `whisper` feature), cleanup (RegexCleanup + OllamaCleanup w/ fallback), output/clipboard, tray/settings, the assembled `Pipeline` + cumulative acceptance suite (AC-1/2/4/5 all green), the security-hardened model downloader, README/CONTRIBUTING. `main` @ `28e7ac6`. Cofounder granted standing admin-merge authority; template upgraded to autonomous-kickoff v2.10.0 (PR #88).
- **In flight:** the **runtime-wiring capstone #91** (branch feature/runtime-wiring) — wires hotkey→capture→pipeline→paste into the Tauri app and resolves the fix-when-wired blockers (#65 clipboard-clobber, #58 audio RT-safety, #73 Ollama write-timeout, #80 settings Result channel, #86 AC-5 guard, #44/#59 hotkey reconcile/audio errors). When it merges, `pnpm tauri dev` performs real dictation.
- **Open gates (cofounder):** **#27 the AC-7 human smoke test = the M1 Done gate** (dictate into real apps + Obsidian; only the cofounder can). #30 optional `ANTHROPIC_API_KEY` in a `ci` environment (unlocks real CI-sentinel + lets `sentinel` become a required check → closes #8 containment → unlocks Copilot implementers). #8 containment (Copilot stays gated; using Claude engines meanwhile).
- **Review/merge model:** every PR gets an independent **Opus** Sentinel review (Fable budget exhausted — MISSION §7 sanctions fable-or-opus); Lead admin-merges on APPROVED/CONDITIONAL after filing follow-ups + rebasing. Required check on `main` = `harness-guard` only (`sentinel.yml` runs keyless → posts non-blocking MALFORMED until the #30 key lands).
- **Deferred hardening backlog:** ~30 `sentinel:*` follow-up issues (#43–#90), most tied to the wiring step (#91 closes the load-bearing ones) or later milestones.
- **Armed schedules:** Tier-1 watchdog via in-session cron (session-only; re-arm on new session per CONTINUOUS-OPERATION.md).
- **Engine policy:** Claude `sonnet` for implementation, Claude `opus` for Sentinel review (Fable exhausted), Claude `haiku` for ticks; Copilot CLI gated until #8 clears.
- **Single next action:** review + admin-merge #91 when it reports (rebasing onto `main`); then M1 code is complete → hand the cofounder a runnable build for the #27 smoke test = M1 Done.
