# Changelog — bla

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `tray` module (issue #23, AC-14): a total, deterministic
  `tray_icon_state(&PipelineState) -> TrayIconState` mapping every pipeline
  state (`Idle`/`Recording`/`Transcribing`/`Error`) to its tray icon
  variant (`Idle`/`Active`/`Busy`/`Error`), plus `OutputModeSwitch`, a pure
  model showing that a tray-driven output-mode switch (`CursorPaste`/
  `File`) only affects `route_target()` calls made after `set_mode` —
  i.e. it takes effect starting with the next dictation, not one already
  in flight. All logic is pure and unit-tested; the real Tauri tray
  icon/menu rendering is thin OS glue, deliberately minimal, separate, and
  not wired into `run()` in this increment.
- `settings` module (issue #23, AC-13, ADR-0006): a `Settings` struct
  (hotkey binding, hold/toggle `RecordingMode`, `ModelPreset`
  (`large-v3-turbo`/`small`), `OutputModeSetting` (cursor/file), and a
  file-path template string) deriving `Serialize`/`Deserialize` — holds
  config only, never transcript/clipboard text, so that's compatible with
  MISSION §7's no-log invariant. `to_json`/`from_json` are pure,
  deterministic (de)serialization; `#[serde(default)]` means any field
  missing from persisted (or first-run/empty) JSON falls back to
  `Settings::default()`'s value for that field. `SettingsStore` is the
  injected persistence seam a future `tauri-plugin-store`-backed
  implementation would sit behind (thin OS glue, not wired into
  `commands.rs` in this increment); `InMemorySettingsStore` stands in for
  it in tests, including a simulated-app-restart round trip. No new
  dependencies added — the real `tauri-plugin-store` wiring is deferred to
  a later increment.
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
- `output.rs`: cursor-paste target and the output router (issue #21, AC-9,
  ADR-0003). `ClipboardPayload` wraps transcript/clipboard text and
  implements neither `Debug`, `Display`, nor `Serialize`, locked in by a
  compile-time trait-assertion test — clipboard/transcript contents can
  never flow into a log macro, string formatting, or a serializer.
  `should_restore_clipboard` is the pure restore-decision: after the
  synthesized paste and a configurable 150–300 ms delay (default 200 ms),
  the pre-dictation clipboard is restored only if nothing else changed it
  meanwhile, otherwise the restore is skipped so `bla` never clobbers newer
  clipboard data. `Clipboard`/`PasteSynthesizer` are thin, fakeable OS-glue
  traits with real implementations `SystemClipboard` (`arboard`) and
  `EnigoPaste` (`enigo`, one synthesized Cmd+V/Ctrl+V keystroke).
  `OutputMode`/`route` dispatch a finished dictation to either the
  cursor-paste or file target; the file target additionally confines its
  resolved path to a configured base directory via
  `confine_relative_path`, rejecting absolute paths and `..` traversal that
  would escape it (security AC carried from PR #41's Sentinel review into
  issue #21 — symlink-TOCTOU guarding and restrictive file permissions
  remain a follow-up). Adds `enigo` and `arboard` as dependencies and
  `static_assertions` as a dev-dependency.
- `pipeline` module (issue #25, ADR-0002/ADR-0005): `Pipeline<S, C, Clip,
  Paste, Sleep>` composes an injected `Stt` + `Cleanup` + the output router
  (`crate::output::route`) into a single `Pipeline::run(samples, opts) ->
  Result<Outcome, PipelineError>` call, so the whole transcribe-clean-route
  flow runs headlessly from fixtures. `Pipeline` owns the AC-4 fallback
  decision: a `CleanupError::Unreachable` from the configured `Cleanup` is
  caught and retried against `RegexCleanup`, recorded in
  `Outcome::cleanup_fell_back`, and never surfaced as an error. `cleanup`
  and `output` are now `pub mod`s so the new cumulative acceptance suite
  (`src-tauri/tests/acceptance.rs`) can reach them from outside the crate;
  `pipeline` is not yet wired into `commands.rs`.
- Cumulative acceptance suite `src-tauri/tests/acceptance.rs` (issue #25),
  entirely from injected fakes/stubs (no live mic, clipboard, model, or
  network): `ac1_...` runs `FakeStt`'s canned transcript (fillers plus one
  self-correction) through `OllamaCleanup` backed by a stub transport that
  returns the cleaned-and-corrected text, asserting no filler words and the
  corrected phrase survive (AC-1); `ac2_...` times the regex-cleanup path
  over a 15-second-equivalent (240,000-sample) fixture, logs the measured
  duration, and asserts it's under the 2 s budget (AC-2; real whisper-rs
  latency stays a `--features whisper` / AC-7 smoke-test concern, per
  `stt.rs`); `ac4_...` drives an unreachable Ollama stub and asserts the
  pipeline falls back to `RegexCleanup` with no error surfaced (AC-4);
  `ac5_...` builds the pipeline entirely from injected stubs and asserts it
  completes with zero real network I/O, guarded by a
  `static_assertions::assert_type_ne_all!` that fails to compile if the
  real, network-touching `UreqTransport` is ever substituted into this
  case (AC-5).

### Changed

### Fixed

- `RegexCleanup` (`src-tauri/src/cleanup.rs`), three Sentinel-tracked bugs
  that blocked wiring cleanup into the pipeline (issues #52/#53/#54, all
  fixed before #25 per Sentinel's instruction):
  - **#52** comma-flanked "like" is no longer stripped unconditionally —
    it's only treated as discourse filler when the word right after it is
    a clause-starter (this/that/it/i/we/you/he/she/they/there, plus
    contractions), so a genuine list connector like "eggs, like, milk"
    survives ("like, this is cool" is still correctly stripped as filler).
  - **#53** a comma left dangling by a trailing filler removal (e.g. "I
    think, um" -> "I think," once "um" is gone) is now stripped before
    capitalization/final-punctuation, so the result is "I think." instead
    of the malformed "I think,.".
  - **#54** sentence-start capitalization no longer fires on a decimal
    point: a `.` directly between two digits (e.g. "3.14") is no longer
    treated as a sentence terminator, so "3.14 exactly" stays "3.14
    exactly." instead of wrongly capitalizing to "3.14 Exactly.".

### Removed
