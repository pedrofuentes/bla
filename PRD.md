# PRD — bla

> Product requirements for `bla`, a local-first, system-wide voice dictation app. This document is the Phase 1 discovery gate (`docs/KICKOFF.md`). It derives every requirement from `MISSION.md` and `ROADMAP.md`; it invents no scope beyond them. Every acceptance criterion carries a stable `AC-n` id bound to an executable test (or explicitly marked human-gated).

---

## 1. Problem statement

Typing is the bottleneck for everyday text creation on a developer's Mac: email, chat, notes, and journaling all compete with keyboard throughput. Speech is faster than typing, but two failure modes keep dictation from being a daily-driver replacement:

1. **Raw transcription isn't enough.** Speech-to-text alone reproduces filler words ("um," "uh"), false starts, and self-corrections verbatim, and it doesn't turn a spoken list into structure. The output requires enough manual cleanup that the time saved by speaking is lost again editing — so raw transcription alone does not clear the bar for text a user is willing to send unedited.
2. **On-device matters because privacy and reliability are the same requirement here.** Commercial dictation tools commonly route audio through a cloud service for transcription and/or cleanup. For a tool used across email, private notes, and a personal journal, that means every sentence spoken transits a third party's servers — an unacceptable trade for personal, potentially sensitive text, and one that also makes the tool unusable offline or dependent on a vendor's uptime and pricing. `bla` keeps audio, transcripts, and derived text on the machine end-to-end: speech in, publication-ready text out, nothing leaves the device except one user-initiated, one-time model download.

`bla` solves both: on-device Whisper transcription feeding a pluggable cleanup layer (LLM when available, deterministic rules otherwise), so text is both accurate and clean enough to paste and send — without a network round trip.

## 2. User personas

### Primary persona — the cofounder ("daily-driver developer")
- macOS user, technically fluent, comfortable with a menu-bar utility and a first-run permissions walkthrough.
- Dictates into: email clients, chat apps, browser text fields, and an Obsidian vault for a daily journal.
- Needs push-to-talk to be instant and reliable (no missed hotkey presses, no hung recordings), cleanup good enough to send unedited most of the time, and a direct-to-file mode that appends to today's Obsidian daily note regardless of which app currently has focus.
- Success looks like: reaching for `bla` instead of typing, or instead of a competing tool, for weeks in a row.

### Secondary persona — privacy-conscious open-source users
- Developers/power users who want system-wide dictation but will not accept audio or text leaving their machine, and who may not run an LLM at all (Ollama optional/absent).
- Needs the app to be fully functional and gracefully degraded (rule-based cleanup) with zero network dependency beyond the one-time model download, auditable via an open-source, MIT-licensed codebase they can inspect or extend.

## 3. Feature list by milestone

Acceptance criteria **AC-1 through AC-7** are defined verbatim in `MISSION.md` §8 and are reproduced here unchanged; **AC-8 and onward** are new criteria derived from `ROADMAP.md` milestone content that MISSION §8 does not already cover. Ids are unique and contiguous (AC-1…AC-28). Every criterion maps to an executable test unless explicitly marked **(HUMAN-GATED)**.

### M1 — MVP: the dictation pipeline (v0.1)

**Feature: Push-to-talk dictation** — global hotkey (hold-to-record; toggle mode configurable) → `cpal` capture → on-device `whisper-rs` transcription → clipboard-swap paste into the focused app.
- **AC-1** *(MISSION §8, verbatim)* Headless pipeline test: a fixture WAV of natural speech (with fillers and one self-correction) run through capture-decode → whisper → cleanup produces text with no filler words and the corrected phrase — asserted in `cargo test`.
- **AC-2** *(MISSION §8, verbatim)* Latency budget: pipeline (transcribe + cleanup, regex path) for a 15-second 16 kHz fixture completes in < 2 s on Apple Silicon — measured in a benchmark test, logged per run.
- **AC-8** Hold-to-record produces exactly one dictation per press/release cycle; toggle mode (configurable) starts recording on the first press and stops on the next — asserted in `cargo test` against the hotkey state machine driven by simulated press/release events.
- **AC-9** Clipboard-swap paste restores the pre-dictation clipboard contents after the synthetic paste completes, and the clipboard payload cannot be logged or persisted: it is carried in a wrapper type that implements neither `Debug`, `Display`, nor `Serialize`, enforced by a compile-time trait-assertion test in the clipboard/output module — asserted in `cargo test` (clipboard content equals its pre-dictation value post-paste, plus the trait-assertion test).

**Feature: Pluggable cleanup layer** — rule-based pass always available; Ollama LLM pass when `localhost:11434` responds, graceful fallback otherwise.
- **AC-4** *(MISSION §8, verbatim)* Cleanup fallback: with Ollama unreachable, the pipeline still returns rule-cleaned text with no error surfaced to the paste path — asserted with a stubbed unreachable endpoint.
- **AC-10** LLM cleanup pass, fixture-based regression: against a stubbed local LLM endpoint, fixture transcripts containing self-corrections, missing punctuation, and a spoken list produce output with the corrections applied, punctuation restored, and the list rendered as bullets; the cleanup prompt lives in a versioned prompt file and is rewrite-only (never answers, never adds content), verified by fixture regression checks asserting no content beyond the input is introduced — asserted in `cargo test` per MISSION §7.

**Feature: Direct-to-file mode** — output router switchable to append to a Markdown file with `{{date:YYYY-MM-DD}}` path templating + optional timestamps, independent of app focus.
- **AC-3** *(MISSION §8, verbatim)* File mode: dictation with file output appends a correctly timestamped entry to a `{{date:YYYY-MM-DD}}`-templated path, creating the file if absent — asserted in `cargo test` against a temp dir.
- **AC-11** Path templating supports `{{date:YYYY-MM-DD}}` and an optional per-entry `{{time:HH:mm}}` timestamp, creating any missing intermediate directories in the templated path — asserted in `cargo test` against a temp dir across multiple template variants.

**Feature: Minimal shell** — tray icon with state + output-mode switch, first-run Whisper-model downloader, persisted settings.
- **AC-12** First-run model download contacts only the allowlisted origins (huggingface.co + its CDN), shows progress, and once the model is present the app requires no further non-localhost network access (`localhost:11434` for Ollama remains permitted) — asserted via a network-guard test scoped to the downloader path; the in-app progress UI is confirmed by the AC-7 smoke test.
- **AC-13** Settings (hotkey binding, hold/toggle mode, selected Whisper model, output mode, file-path template) persist across an app restart — asserted in `cargo test` round-tripping the `tauri-plugin-store`-backed settings.
- **AC-14** Tray state (idle/recording/transcribing/error) is a pure function of pipeline state, and the output-mode switch changes routing (cursor-paste vs. file) starting with the next dictation — asserted with a unit test on the state-derivation function; the rendered tray icon itself is confirmed by the AC-7 smoke test.

**Cross-milestone / always-on requirements:**
- **AC-5** *(MISSION §8, verbatim)* Privacy: a network-guard test asserts the product binary makes no runtime connections outside the §5 allowlist during a full pipeline run.
- **AC-6** *(MISSION §8, verbatim)* Naming rule: repo-wide audit (Sentinel checklist item on every PR) confirms no competitor product/company names in any artifact.
- **AC-7** *(MISSION §8, verbatim, HUMAN-REQUIRED, repeats at every milestone close)* Cofounder smoke test: dictate into at least three real apps (notes, chat, browser form) and into an Obsidian vault note; cofounder closes the gate on the board. The agent cannot verify this — a milestone is not Done until the cofounder closes this gate.

### M2 — UI shell (v0.2)

**Feature: Recording pill** — always-on-top, live waveform, recording/transcribing/done/error states.
- **AC-15** The recording pill renders idle/recording/transcribing/done/error states driven by pipeline state, with a live waveform sourced from streamed audio levels — asserted via Vitest component tests per state, plus a Playwright screenshot pass (mocked IPC) for visual regression.

**Feature: Full tabbed settings window** — General tab: hotkeys, model pick, hold-vs-toggle, launch-at-login.
- **AC-16** Changing hotkey binding, model selection, hold-vs-toggle mode, or launch-at-login in the settings window round-trips to the persisted store and is reflected on reload — asserted via a Vitest + mocked-IPC integration test.
- **AC-17** The model picker offers the supported Whisper presets — quantized `large-v3-turbo` (default) and `small` (fast/low-RAM) — and the STT module loads the selected preset on the next dictation — asserted via a Vitest test of the picker options plus a `cargo test` asserting the STT model path follows the persisted selection.
- **AC-18 (HUMAN-GATED, design review)** The settings UI meets the Phase-2 design rubric's accessibility bar (visible focus, contrast, hit-target size) — verified in the M2 design-review gate (`docs/KICKOFF.md` Phase 2 defines the rubric).

**Feature: Sound cues and error toasts.**
- **AC-19** Error toasts surface distinct, correct messages for model-missing, Ollama-down, and no-mic-permission conditions — asserted via Vitest tests that trigger each condition and assert toast content; actual audio cue playback is confirmed by the milestone's AC-7 smoke test (not headlessly assertable).

### M3 — Context features (v0.3)

**Feature: History** — searchable past dictations (raw + cleaned) in local SQLite, copy/delete, retention setting.
- **AC-20** Every dictation writes a raw + cleaned history entry to local SQLite; entries are searchable by substring, individually copyable (the copy action returns the entry's text through the clipboard path), and individually deletable; and a retention setting purges entries older than the configured age — asserted in `cargo test` against a temp SQLite database.

**Feature: Personal dictionary** — user terms fed to Whisper `initial_prompt` and the cleanup prompt.
- **AC-21** A dictionary term absent from a fixture WAV's default transcription is correctly recognized once added to the dictionary and injected into Whisper's `initial_prompt` and the cleanup prompt — asserted in `cargo test` comparing output with and without dictionary injection on the same fixture.

**Feature: Per-app tone** — active-app detection → tone profile (casual/formal/verbatim) with editable rules; verbatim bypasses the LLM.
- **AC-22** Active-app detection selects the configured tone profile; editing a tone rule (remapping an app to a different profile) changes dispatch on the next dictation; and the `verbatim` profile bypasses the LLM cleanup pass entirely (rule-based pass only) — asserted in `cargo test` mocking active-app context and asserting `Cleanup` trait dispatch before and after a rule edit.

### M4 — Command mode & snippets (v0.4)

**Feature: Command mode** — second hotkey copies the selection, records a spoken instruction, LLM-transforms it, pastes the result back with clipboard restored.
- **AC-23** Command mode copies the current selection, combines it with a recorded spoken instruction through a rewrite-only LLM transform, pastes the transformed result back, and restores the original clipboard contents afterward — asserted in `cargo test` with a stubbed selection and fixture instruction audio, asserting output text and post-paste clipboard state.

**Feature: Snippets** — trigger phrase → expansion, matched post-cleanup.
- **AC-24** A configured snippet trigger phrase present in cleaned (not raw) transcript text expands to its configured text — asserted in `cargo test` against fixture transcripts containing trigger phrases, confirming expansion runs after the cleanup pass.

### M5 — Polish & packaging (v1.0)

**Feature: First-run onboarding** — mic + Accessibility permission walkthrough, model download with progress.
- **AC-25** The onboarding step-state machine advances correctly through permission-grant and model-download steps and resumes correctly if interrupted mid-flow — asserted via a Vitest test of the step-state machine; the full permission-grant UX is confirmed by this milestone's AC-7 smoke test **(HUMAN-GATED)**.

**Feature: Settings import/export.**
- **AC-26** Exporting settings (hotkey/model/output config, dictionary, snippets) and importing the resulting file into a clean profile reproduces the same configuration — asserted in `cargo test` round-tripping export → import.

**Feature: Docs** — README with visuals; CONTRIBUTING.
- **AC-27** The repo contains a `README.md` with install and usage sections and at least one embedded visual referenced from a tracked repo path, and a `CONTRIBUTING.md` — asserted by a docs-presence CI check (files exist, required sections present, image path resolves); visual quality is confirmed at this milestone's AC-7 smoke test **(HUMAN-GATED for quality)**.

**Feature: Packaging** — macOS `.dmg` via `tauri build`; Windows build compile-verified.
- **AC-28** A macOS `.dmg` build via `tauri build` completes and the resulting app launches, and the Windows build compiles without error — both asserted as required CI build-matrix jobs; publishing the artifact as a GitHub Release remains a separate, human-required gate (MISSION §9) not covered by this criterion.

## 4. Non-functional requirements

- **Latency budget:** transcribe + cleanup (regex path) for a 15-second, 16 kHz fixture completes in under 2 seconds on Apple Silicon (AC-2). This is the budget the daily-driver flow depends on; the LLM cleanup path is best-effort and not held to the same bound.
- **Privacy invariants (MISSION §5):** the product may only ever reach `huggingface.co` + its CDN (one-time, user-initiated model downloads) and `localhost:11434` (Ollama). No telemetry, no analytics, no crash reporting, no other network origin — enforced by network-guard tests (AC-5, AC-12) and Sentinel review of any new network call. Raw clipboard contents are never logged or persisted (AC-9). All history/dictionary/snippet data lives in local SQLite under the OS app-data directory; no user data or model file is ever committed to the repo.
- **Accessibility:** the settings UI (M2 onward) meets the Phase-2 design rubric's accessibility bar — visible focus, contrast, hit-target size — verified in the M2 design-review gate (AC-18; rubric defined in `docs/KICKOFF.md` Phase 2).
- **Resource footprint:** Whisper model presets per `docs/ARCHITECTURE.md` §Key Technical Decisions: quantized `large-v3-turbo` as the default (≈ real-time on Apple Silicon) and `small` as the fast/low-RAM preset, selectable via the M2 settings model pick (AC-17).

## 5. Success metrics

- **Daily-driver usage:** the cofounder uses `bla` as their default dictation input across at least email, chat, notes, and Obsidian journaling for multiple consecutive weeks post-M1, without reverting to typing or a competing tool for those flows (tracked qualitatively via the recurring AC-7 smoke-test gate; no telemetry is collected per §4).
- **Text-sent-unedited rate:** a majority of dictated outputs are pasted/sent without manual correction, as self-reported by the cofounder at each milestone's AC-7 gate.
- **Latency p95:** the p95 of the AC-2 benchmark (transcribe + cleanup, regex path, 15 s fixture, Apple Silicon) stays under the 2-second budget across benchmark runs logged in CI.

## 6. Out of scope

Explicitly excluded from all milestones (MISSION §4, §5, §9): user accounts, billing/subscriptions, multi-device sync, mobile clients, any telemetry or analytics, cloud-based speech-to-text or cloud LLM cleanup of any kind, and app-store distribution. Production release (tagged GitHub Release binaries) remains a human-required gate, not an automated deliverable of any milestone's acceptance suite.
