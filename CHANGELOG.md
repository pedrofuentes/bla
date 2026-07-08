# Changelog — bla

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Tauri 2 + React-TS + Vite + Tailwind app scaffold: `src-tauri` module stubs
  (`hotkeys`, `audio`, `stt`, `cleanup`, `output`, `context`, `store`,
  `commands`), `src/windows/{settings,pill}` and `src/lib/ipc.ts` UI stubs,
  rustfmt/clippy/ESLint/Prettier/Vitest tooling, and a `Makefile` covering
  `cargo llvm-cov` with OS-glue coverage exclusions. No product logic yet.
- `audio` module: fixed-capacity ring buffer for captured `f32` samples
  (drop-oldest overflow), channel-downmix + linear-interpolation resampling
  to the 16 kHz mono format `stt` expects, RMS/peak level metering for the
  future pill waveform, and 16-bit PCM WAV export for round-tripping a
  captured window. All pure logic is unit-tested against in-code synthetic
  sine-wave signals (ADR-0007 — no real recordings). The `cpal` device-open
  and stream callback is thin, TDD-exempt OS glue that delegates every
  decision to the tested pure functions.
- Pure hold/toggle hotkey state machine in `hotkeys.rs` (AC-8): configurable
  Hold (record while the chord is held; stops when any chord key releases)
  and Toggle (first press starts, next press stops) modes, driven by
  injected, timestamped key events with no `Instant::now()` calls so it's
  deterministic in tests. Includes a configurable debounce threshold
  (default 300 ms) that discards accidentally-short Hold presses without
  emitting a dictation. OS wiring (`tauri-plugin-global-shortcut`) remains a
  separate, thin, TDD-exempt stub.
- `output.rs`: file-mode output target's path templating and file-append
  logic — `{{date:YYYY-MM-DD}}` and `{{time:HH:mm}}` token expansion against
  an injected `Clock` (deterministic, no OS-clock calls), and `append_entry`,
  which creates missing intermediate directories and the file if absent,
  then appends an entry with an optional expanded timestamp prefix (AC-3,
  AC-11). Cursor-paste target and the router dispatching between the two
  remain out of scope (issue #21). Adds `tempfile` as a dev-dependency for
- `Cleanup` trait, `Tone`, and `CleanupError` in `src-tauri/src/cleanup.rs`
  (ADR-0005, PRD AC-4 basis), plus the always-available `RegexCleanup`
  baseline: filler-word removal (unconditional `um`/`uh`/`er`; comma-flanked
  `like`/`you know` only, to avoid stripping comparative/literal usage),
  whitespace collapse, sentence-start capitalization, and sentence-final

### Changed

### Fixed

### Removed
