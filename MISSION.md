# MISSION — bla

> **This is the per-project brief.** It is the *only* file you normally edit per project. The generic operating instructions in [`docs/KICKOFF.md`](docs/KICKOFF.md) read this file and fill every project-specific decision from it.

---

## 1. Identity & mission
- **Project name:** bla
- **Repo:** `pedrofuentes/bla`
- **Cofounder handle (for @-mentions on gated decisions):** @pedrofuentes
- **One-line mission:** A local-first, system-wide voice dictation app: hold a hotkey, speak naturally, and clean, polished text appears at the cursor of whatever app is focused — nothing ever leaves the machine.
- **Target users & the problem:** The cofounder (personal use, macOS daily driver). Typing is the bottleneck for email, notes, chat, and journaling; existing dictation tools either produce raw unedited transcripts or send audio to the cloud. bla removes both pains: speech in, publication-ready text out, fully on-device. Open source (MIT) so others with the same need can use and extend it.
- **Success vision:** bla becomes the cofounder's default input method — used daily for weeks without reaching for a competing tool: reliable push-to-talk, sub-2-second turnaround on a 15-second utterance, text clean enough to send unedited, and a frictionless "dictate straight into my Obsidian daily note" flow.

## 2. Product shape
- **Product type:** Desktop app (menu-bar/tray utility, Tauri 2)
- **Hosting / distribution:** Open-source repo (MIT license, public) with GitHub Release binaries (.dmg for macOS; Windows installer must keep compiling, tested when hardware is available). No store distribution for v1.
- **Backend?** **None.** Fully client-side/on-device. Adding any backend, proxy, or external origin is a gated decision.
- **Design direction:** Quiet, minimal, native-feeling utility — a small always-on-top recording pill with a live waveform, a clean tabbed settings window. Think "invisible until summoned." Dark + light mode.

## 3. Tech stack
- **Language(s):** Rust (core), TypeScript (UI)
- **Framework(s) / key libraries:** Tauri 2.x; React + Vite + Tailwind (UI); `cpal` (audio capture); `whisper-rs` (on-device STT, Metal on macOS); `enigo` (synthetic paste); `active-win-pos-rs` (active-app detection); `rusqlite` (history/dictionary/snippets); `tauri-plugin-global-shortcut`, `tauri-plugin-store`; Ollama over `localhost:11434` for AI cleanup with a rule-based fallback.
- **Package manager:** pnpm (UI) + cargo (core)
- **Test runner / e2e:** `cargo test` (core logic: cleanup transforms, snippet matching, path templating, tone rules) + Vitest (UI components). No full-app e2e driver on macOS; the acceptance suite exercises the Rust pipeline headlessly with WAV fixtures.
- **Visual verification:** Playwright against the Vite dev server with the Tauri IPC layer mocked (renders the settings window + recording pill states in a browser for screenshots); final in-app verification is the human smoke-test gate.

## 4. MVP scope (v1 = milestone M1)
1. **Push-to-talk dictation:** global hotkey (hold-to-record; toggle mode configurable) → `cpal` capture → on-device `whisper-rs` transcription → clipboard-swap paste into the focused app.
2. **AI cleanup layer:** pluggable `Cleanup` trait — rule-based pass (filler-word removal, spacing/caps) always available; Ollama LLM pass (fillers, self-corrections, punctuation, spoken lists → bullets; strict rewrite-only prompt) when `localhost:11434` responds, graceful fallback otherwise.
3. **Direct-to-file mode:** output router switchable between cursor-paste and appending to a Markdown file with `{{date:YYYY-MM-DD}}` path templating + optional timestamps (Obsidian daily-note flow), working regardless of app focus.
4. **Minimal shell:** tray icon with state + mode switch, first-run Whisper-model downloader, hotkey/model/output settings persisted.
- **Explicitly out of scope for v1:** accounts, billing, sync, mobile, telemetry, cloud STT/LLM of any kind, app-store distribution. Post-MVP backlog (feeds `ROADMAP.md`): M2 recording-pill UI + full settings window; M3 history + personal dictionary + per-app tone; M4 command mode (transform selected text by voice) + snippets; M5 polish, onboarding, packaging.

## 5. Security, privacy & data
- **Auth model:** none.
- **Privacy/data constraints:** Audio, transcripts, and any derived text **never leave the device**. No telemetry, no analytics, no crash reporting to external services. History/dictionary live in local SQLite under the OS app-data dir.
- **Network allowlist (runtime origins the *product* may contact):** `huggingface.co` + its CDN (one-time model downloads, user-initiated) and `localhost:11434` (Ollama). Nothing else — enforced by an automated test where feasible and Sentinel review of any new network call.
- **Agent egress allowlist (origins the *build fleet* may reach):** github.com/api.github.com, crates.io/static.crates.io, registry.npmjs.org, the GitHub Copilot service backend used by the Copilot CLI (api.githubcopilot.com + its github.com auth endpoints — appended with the §7 engine policy through the same cofounder gate: PR #6, merged at `f0da1f9`), plus docs: tauri.app, docs.rs, developer.apple.com, huggingface.co, ollama.com. Nothing else.
- **Known security risks to research up front:** macOS mic + Accessibility (synthetic keystroke) permissions UX; clipboard handling (transcripts transit the clipboard — restore semantics, never log clipboard contents); supply-chain surface of native crates (whisper-rs/cpal build scripts).
- **Continuous scanning:** Dependabot, CodeQL, and secret scanning enabled and monitored; open high/critical alerts and any detected secret gate every release.

## 6. Reuse & references
- **Prior art / code to study or port:** whisper.cpp examples (streaming/VAD patterns), `whisper-rs` examples, Tauri 2 global-shortcut + tray examples, Obsidian daily-notes path conventions. Study patterns only — port no code without license review.
- **Design/UX references:** native macOS menu-bar utilities with minimal chrome (system dictation pill, Raycast-style restraint). **Standing rule: never name, reference, or link competing dictation products or companies in any repo artifact (see §7 NEVER list).** Research may read competitor material for functional insight, but findings must be recorded product-neutrally ("dictation tools commonly…").

## 7. Harness pre-answers (so agents-template New-Project-Setup never stalls)
- **Coverage threshold:** 70 (core Rust logic; UI components counted separately; native/OS-integration glue excluded via coverage config — Sentinel ratchets up, never down).
- **Weak-test gate:** `coverage-diff`.
- **Git author identity (commits):** Pedro Fuentes <git@pedrofuent.es>
- **AI attribution (commit `Co-authored-by` trailer):** Claude Fable 5 <noreply@anthropic.com> for Claude-authored commits; a commit authored by a Copilot CLI worker instead carries `Co-authored-by: GitHub Copilot <noreply@github.com>` — attribution must match the engine that actually authored the change.
- **Sentinel method:** B (CI, enforced by branch protection) + A (sub-agent) in dev.
- **Agent identity (for unattended runs):** none provisioned yet — see attended mode below; provisioning a distinct identity is queued as a `BLOCKED:` preflight item for the cofounder before any unattended Tier-2 operation.
- **Attended single-operator mode:** `yes — I accept running under my own identity while present`. Tier-1 only; no unattended Tier-2 until a distinct agent identity exists.
- **Enforced coding patterns:**
  - Engine-per-task policy for the fleet (cofounder-approved 2026-07-06 — attested by the cofounder's merge of PR #6, merge commit `f0da1f9`): Claude `fable` for architecture decisions, milestone planning, Sentinel review, and tricky native integrations (audio, STT bindings, synthetic input, permissions); **Copilot CLI (headless `copilot -p`)** for feature implementation and for mechanical work (triage, board/label updates, changelog/doc edits, watchdog ticks), with Claude `sonnet`/`haiku` as the respective fallbacks when a Copilot run fails or the task needs harness-native tooling. Copilot workers carry the full brief (TDD choreography, worktree isolation, stop-at-PR delegated-implementer contract), are registered in the PLAN.md fleet registry, and never invoke Sentinel or merge.
  - **Copilot containment (blocking precondition — SNTL-20260706-bla-PR6-23f9e9d 🔴):** Copilot CLI may run **read-only** work (watchdog ticks, triage reads, status checks) immediately, but may **not** be spawned as a delegated implementer (editing files, committing, pushing) until "never merge" is mechanically true: **Sentinel-in-CI + the harness-guard are required checks on `main`**, or the worker runs under a credential that structurally cannot push or merge to `main`. When the unlocking control lands, cite it here (workflow file + branch-protection contexts).
  - Approval attestations in MISSION.md/PLAN.md must cite a verifiable artifact (an issue/PR comment URL, a merge by the cofounder, or a `decision:approved` label event); an uncited "(cofounder-approved)" string is an unsatisfied gate.
  - Cleanup layer stays behind the `Cleanup` trait; output targets stay behind the output-router abstraction; no direct OS calls outside the designated modules (`audio`, `output`, `hotkeys`, `context`).
  - LLM cleanup prompts must be rewrite-only (never answer, never add content) and live in versioned prompt files with fixture-based regression checks.
- **Forbidden actions (NEVER):**
  - Naming, referencing, or linking any competing dictation product or company in any document, commit message, file, code identifier, issue, or PR. Describe functionality generically.
  - Any network call from the product outside the §5 allowlist; any telemetry/analytics; sending audio or text off-device.
  - Logging or persisting raw clipboard contents; committing model files or user data (history DB, settings) to the repo.
  - Committing real user recordings as test fixtures — STT/cleanup fixtures must be synthetic (TTS) or already-public audio/text, since fleet engines transmit repo content (ADR-0007).
- **Enable branch protection on `main`?** yes.

## 8. Definition of Done (project-specific acceptance)
- **AC-1** Headless pipeline test: a fixture WAV of natural speech (with fillers and one self-correction) run through capture-decode → whisper → cleanup produces text with no filler words and the corrected phrase — asserted in `cargo test`.
- **AC-2** Latency budget: pipeline (transcribe + cleanup, regex path) for a 15-second 16 kHz fixture completes in < 2 s on Apple Silicon — measured in a benchmark test, logged per run.
- **AC-3** File mode: dictation with file output appends a correctly timestamped entry to a `{{date:YYYY-MM-DD}}`-templated path, creating the file if absent — asserted in `cargo test` against a temp dir.
- **AC-4** Cleanup fallback: with Ollama unreachable, the pipeline still returns rule-cleaned text with no error surfaced to the paste path — asserted with a stubbed unreachable endpoint.
- **AC-5** Privacy: a network-guard test asserts the product binary makes no runtime connections outside the §5 allowlist during a full pipeline run.
- **AC-6** Naming rule: repo-wide audit (Sentinel checklist item on every PR) confirms no competitor product/company names in any artifact.
- **AC-7 (per-milestone, HUMAN-REQUIRED)** Cofounder smoke test: dictate into at least three real apps (notes, chat, browser form) and into an Obsidian vault note; cofounder closes the gate on the board. **The agent cannot verify this — a milestone is not Done until the cofounder closes this gate.**

## 9. Authorization — what the agent may do without you (tiered)

*(Tier table as per the template defaults.)*

- **Autonomy profile:** `standard`.
- **Default time-box (auto-proceed window for the `time-boxed` tier):** 24h.
- **Risk tolerance:** balanced.
- **Production release gate:** `human-required` — every GitHub Release/tagged binary publish; local/dev builds and CI artifacts are `auto`.
- **Roadmap exhaustion:** `stop`.
- **Project overrides:**
  - The **AC-7 cofounder smoke test** is `human-required` at every milestone close — never time-boxed, never auto-proceeded.
  - Any addition to the product's §5 network allowlist is `human-required` (restating the floor: this includes swapping model sources).
  - Heavy native dependencies beyond the §3 list (new C/C++-linking crates) are `time-boxed` with a transitive-risk note.
- **Pre-authorized specifics (`auto`):** the §3 stack + transitive build/test/lint tooling; downloading Whisper GGUF models from huggingface.co for dev/test; standard CI (build matrix macOS + Windows compile-check, tests, lint, Sentinel Method B, scanners).

## 10. Resource governance (concurrency & cost)
- **Max concurrent workers / worktrees:** 3
- **Per-watchdog-tick spawn cap:** 3
- **Max recursion depth:** 3
- **Max spawn-tree size per milestone:** 25
- **Max Actions-minutes per day:** 240
- **Max auto-proceeded `time-boxed` gates per milestone:** 5
- **Max consecutive auto-proceeded milestones with zero cofounder interaction:** 1 (every milestone ends in the AC-7 human gate anyway)
- **Dead-man switch:** 7 days
- **Per-milestone token/cost budget:** soft cap — honor the §7 engine-per-task policy strictly; raise a `needs:decision` before any single milestone's spawn count exceeds the §10 caps.
