# Changelog — bla

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `stt` module (issue #18, AC-1 partial / AC-21 seam, ADR-0004): an `Stt`
  trait (`transcribe(samples: &[f32], opts: &TranscribeOpts) -> Result<String, SttError>`)
  with a `FakeStt` test double for pipeline-shape tests, plus
  `build_initial_prompt`, the pure, unit-tested function that renders
  personal-dictionary terms into Whisper's `initial_prompt` (ordering,
  comma/backslash escaping, blank-term dropping, and a deterministic
  length cap). `WhisperStt` — the real `whisper-rs` (whisper.cpp,
  Metal-accelerated on macOS) implementation — lives behind a new
  default-off `whisper` cargo feature, since it builds whisper.cpp from
  source and CI has no model file to transcribe with; `cargo test` (no
  feature) exercises the trait/`FakeStt`/`build_initial_prompt` coverage,
  and an `#[ignore]`d integration test covers real transcription manually
  against a downloaded model.
- Tauri 2 + React-TS + Vite + Tailwind app scaffold: `src-tauri` module stubs
  (`hotkeys`, `audio`, `stt`, `cleanup`, `output`, `context`, `store`,
  `commands`), `src/windows/{settings,pill}` and `src/lib/ipc.ts` UI stubs,
  rustfmt/clippy/ESLint/Prettier/Vitest tooling, and a `Makefile` covering
  `cargo llvm-cov` with OS-glue coverage exclusions. No product logic yet.

### Changed

### Fixed

### Removed
