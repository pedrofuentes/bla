# Roadmap — bla

> Project phases, milestones, and implementation plan. M1 is the MVP (MISSION.md §4); later milestones are the approved backlog — each is proposed through the next-milestone Decision gate before it starts, and every milestone closes with the AC-7 cofounder smoke test.

## Current Phase

Phase 0/1 — harness bootstrapped; discovery (PRD) and board seeding for M1 in progress.

## Milestones

### M1 — MVP: the dictation pipeline (v0.1)
- Push-to-talk dictation: global hotkey (hold; configurable toggle) → cpal capture → on-device whisper-rs transcription → clipboard-swap paste into the focused app
- Pluggable cleanup: rule-based pass (fillers, spacing, caps) + Ollama LLM pass (self-corrections, punctuation, spoken lists → bullets) with graceful fallback when Ollama is unreachable
- Direct-to-file mode: append to a Markdown file with `{{date:YYYY-MM-DD}}` path templating + optional timestamps, regardless of app focus
- Minimal shell: tray icon with state + output-mode switch, first-run Whisper-model downloader, persisted hotkey/model/output settings
- Acceptance: AC-1…AC-7 (MISSION.md §8)

### M2 — UI shell (v0.2)
- Always-on-top recording pill: live waveform from streamed audio levels; recording/transcribing/done/error states
- Full tabbed settings window (General: hotkeys, model pick, hold-vs-toggle, launch-at-login)
- Sound cues; error toasts (model missing, Ollama down, no mic permission)

### M3 — Context features (v0.3)
- History: searchable past dictations (raw + cleaned) in local SQLite, copy/delete, retention setting
- Personal dictionary: user terms fed to Whisper `initial_prompt` + cleanup prompt
- Per-app tone: active-app detection → tone profile (casual/formal/verbatim), editable rules, verbatim bypasses the LLM

### M4 — Command mode & snippets (v0.4)
- Command mode: second hotkey copies the selection, records a spoken instruction, LLM-transforms it, pastes the result back (clipboard restored)
- Snippets: trigger phrase → expansion, matched post-cleanup

### M5 — Polish & packaging (v1.0)
- First-run onboarding: mic + Accessibility permission walkthrough, model download with progress
- Settings import/export; README with visuals; CONTRIBUTING
- macOS .dmg via `tauri build` (GitHub Release — human-required gate); Windows build compile-verified

## Key Milestones

| Milestone | Version | Status |
|-----------|---------|--------|
| M1 — MVP dictation pipeline | v0.1 | in-progress |
| M2 — UI shell | v0.2 | pending |
| M3 — Context features | v0.3 | pending |
| M4 — Command mode & snippets | v0.4 | pending |
| M5 — Polish & packaging | v1.0 | pending |
