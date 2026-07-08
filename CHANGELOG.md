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

### Changed

### Fixed

### Removed
