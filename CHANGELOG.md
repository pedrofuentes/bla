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
  punctuation. Fully unit-tested, pure logic, no self-correction resolution
- `OllamaCleanup` in `src-tauri/src/cleanup.rs` (issue #20, ADR-0005, PRD
  AC-4/AC-10): an optional LLM cleanup pass over a local Ollama instance
  (configurable base URL, `http://localhost:11434` by default — the only
  permitted runtime origin besides model download, MISSION §5). The HTTP
  call is injected behind a new `OllamaTransport` trait (`UreqTransport` is
  the thin, non-decision-making `ureq`-backed glue), so request shaping,
  response parsing, and the unreachable-fallback decision are pure and
  unit-tested against a stub transport — no network call or running Ollama
  needed in `cargo test`. Any transport failure — connection refused,
  timeout, or unparsable response — maps to `CleanupError::Unreachable`
  (AC-4) rather than propagating, so a future pipeline dispatch can fall
  back to `RegexCleanup` with no error surfaced to the paste path. The
  `UreqTransport` agent is built with connect/read timeouts (caller-
  configurable, 2 s / 30 s defaults) so a hung-but-reachable endpoint can't
  block the sync call forever, and with `redirects(0)` so a local responder
  can't bounce the request off-origin (single-origin egress invariant,
  MISSION §5). The rewrite-only cleanup prompt lives in the versioned
  `src-tauri/prompts/cleanup_v1.txt` (never answers, never adds content,
  removes fillers, resolves self-corrections, restores punctuation, renders
  spoken lists as bullets, honors the requested tone) and is embedded via
  `include_str!`; a fixture-regression test pins the prompt's constraints
  and an AC-10 request-shape test deserializes the outgoing request and
  asserts per field that the rewrite-only prompt and the raw input land in
  the correct fields (so a field swap fails CI). Adds `ureq` (with
  `default-features = false` — no TLS stack needed for localhost plain
  HTTP) as a new dependency.

### Changed

### Fixed

### Removed
