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
- Pure hold/toggle hotkey state machine in `hotkeys.rs` (AC-8): configurable
  Hold (record while the chord is held; stops when any chord key releases)
  and Toggle (first press starts, next press stops) modes, driven by
  injected, timestamped key events with no `Instant::now()` calls so it's
  deterministic in tests. Includes a configurable debounce threshold
  (default 300 ms) that discards accidentally-short Hold presses without
  emitting a dictation. OS wiring (`tauri-plugin-global-shortcut`) remains a
  separate, thin, TDD-exempt stub.

### Changed

### Fixed

### Removed
