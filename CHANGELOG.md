# Changelog — bla

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `models` module (issue #24, ADR-0004, MISSION §5, PRD AC-12): the first-run
  Whisper model downloader. A registry of the two supported presets
  (quantized `large-v3-turbo` q5_0, the default, and `small`), each pinned
  to its `ggerganov/whisper.cpp` Hugging Face file name, download URL, exact
  size, and SHA-256 (from that repo's Git-LFS metadata). `download_url` and
  `is_allowlisted_url` are the AC-12 network guard's tested seam: every
  registry URL is asserted to resolve only to `huggingface.co`/`hf.co` and
  their subdomains (including the newer Xet-storage CDN hosts, e.g.
  `us.aws.cdn.hf.co`), with real dot-anchored host matching (not a substring
  check) that a battery of adversarial tests confirms rejects lookalike
  hosts (`huggingface.co.evil.com`), the userinfo phishing trick
  (`https://huggingface.co@evil.com/`), and non-`https` schemes. Checksum
  verification (`sha256_hex`/`sha256_hex_reader`/`verify_checksum`),
  progress-percent math (`compute_progress`), and resume-vs-restart planning
  (`plan_resume`) are pure and unit-tested; the actual HTTP GET, streaming-
  to-disk, and progress reporting live behind an injected `ModelTransport`
  trait, so `download_model_with_spec`'s orchestration (URL/allowlist
  selection, resume planning, checksum verification, target-path promotion)
  is exercised in tests against a fake in-memory transport — no real network
  call or downloaded model file needed. A checksum mismatch always errors
  and removes the corrupt partial file rather than promoting it; the target
  path is only ever created after a verified checksum. `UreqTransport`, the
  real transport, additionally re-checks every redirect hop against the same
  network guard (not just the initial request), so the CDN-only egress
  invariant holds at the real network boundary too. Adds `sha2` (checksum
  hashing) as a new dependency and enables `ureq`'s `tls` (rustls-backed)
  feature, needed for the HTTPS download (the existing `OllamaCleanup`
  transport stays on plain HTTP to localhost). Not yet wired into
  `commands.rs`/the UI — that lands with the first-run downloader UI
  integration.
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

### Changed

### Fixed

### Removed
