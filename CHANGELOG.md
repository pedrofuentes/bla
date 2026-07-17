# Changelog — bla

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.2.0] — 2026-07-17 (M1 + M2 shipped)

### Added

- M2 windows scaffold (issue #126): added the always-on-top recording pill and full settings windows as hidden-by-default app windows, wired a tray "Settings…" item to show them, and made the pill window show/hide automatically while dictating — placeholder UI for now, real content lands in later M2 PRs.
- Throttled audio-level event stream (issue #126): the core now emits an `audio-level` event (~30Hz, RMS `0.0..=1.0`) while a dictation is being captured, so the recording pill's live meter (a later M2 PR) has a real signal to draw — computed off the real-time audio thread, never emitting raw samples.
- Recording pill waveform + state UI (issue #126): the pill now renders a live canvas waveform from the `audio-level` event stream while recording, and shows a distinct dot/label for transcribing, done (auto-hiding after ~1.5s), and error states, driven by `pipeline-state-changed` — replacing the earlier placeholder shell.
- Enabled real window transparency for the pill on macOS (issue #129) so its rounded shape renders over the desktop instead of an opaque backdrop.
- Clamped the emitted `audio-level` value to its documented `0.0..=1.0` range (issue #136) so driver-clipped input can no longer exceed it.
- Typed pipeline-error toasts (issue #126): the pill window now shows a small, auto-dismissing toast when the mic fails to start, the Whisper model is missing, or the local Ollama cleanup backend is unreachable (informational — dictation still pastes via the regex fallback) — styled distinctly for informational vs blocking notices, and never carrying dictated text.
- Settings window General tab (issue #126): hotkey capture (press a key combination, validated live via a new `validate_hotkey` command before it's ever saved), Whisper model preset selection with download progress, and hold-vs-toggle recording mode — the tab bar's full shape (History/Dictionary/Tone/Snippets) is in place, with the rest of the tabs landing in later M2 increments.
- Launch-at-login + sound-cue preference (issue #126): a new "Launch bla at login" checkbox in the settings window's General tab enables/disables OS login autostart immediately on save (via `tauri-plugin-autostart`), and a new "Play sound cues" checkbox persists the preference cue playback will read starting in the next M2 increment.
- Synthesized sound cues (issue #126): the recording pill now plays a short, purely synthesized tone (Web Audio `OscillatorNode`, no bundled audio files or recordings) on dictation start, on a successfully completed dictation, and on error — gated by the existing "Play sound cues" preference from the settings window's General tab, and silent for a cancelled dictation so cancelling never sounds like a failure.

## [0.1.0] — M1: MVP dictation pipeline

> **Note:** M1 was released as part of v0.2.0 (2026-07-17); no separate v0.1.0 tag was cut. The M1 features below shipped together with M2 features under the v0.2.0 release.

### Added

- M1 minimal shell (issue #110): replaced the create-tauri-app boilerplate
  (greet demo) with a real status window and a system-tray/menu-bar icon —
  MISSION §4's "minimal shell". The status window reads `get_settings` and
  shows "Hold `<hotkey>` to dictate", the live output mode with a
  Cursor/File toggle (`set_output_mode`), the selected Whisper model's
  ready/downloading/error status (`download_selected_model` +
  `model-download-progress`/`model-download-complete`/`model-download-error`
  events — completion flips the window out of "Downloading…" to Ready), and
  a labeled "Full settings coming in M2" summary; it subscribes to the
  existing `pipeline-state-changed` event to reflect Idle/Recording/
  Transcribing/Error live, and to an `output-mode-changed` event so a
  tray-menu toggle keeps the window's state in sync. Display formatting (hotkey chord → readable
  label, status/mode/model copy) is factored into pure, Vitest-covered
  helpers (`src/lib/status.ts`) so `App.tsx` stays a thin view. A real
  `TrayIconBuilder`-built tray icon (`lib.rs::run()`'s `setup()`) now wires
  to the already-tested `tray::tray_icon_state`/`tray::OutputModeSwitch`
  logic: the icon and a disabled menu line track pipeline state live, and
  a Cursor/File menu toggle calls the *same* `commands::set_output_mode`
  path as the status window's button (AC-14), so tray- and
  window-triggered switches can never disagree; tray icon/menu mutations
  are marshaled onto the main thread (`run_on_main_thread`) since they run
  from pipeline/shortcut-callback threads and AppKit objects must only be
  touched on the main thread on macOS. Show/Hide/Quit menu items
  round out the menu; closing the window now hides it instead of quitting
  (the tray's Quit item is the only way to exit) — a small placeholder
  monochrome icon set ships under `src-tauri/icons/tray/`. Default hotkey
  changed from `Control+Option+Space` to `Control+Shift+Space`: the parser
  already accepted the macOS-only "Option" spelling of Alt on every
  platform, but shipping it as the *default* read as unfamiliar on
  Windows — the new default uses only modifier names spelled identically
  on both platforms, with a regression test pinning that choice.
- Runtime wiring (issue #91): the global hotkey (`tauri-plugin-global-shortcut`)
  now drives the `hotkeys` state machine end to end — on release, the
  captured audio window runs through `pipeline::Pipeline`
  (`OllamaCleanup` with its `RegexCleanup` fallback, AC-4) and the cleaned
  text is routed per the live output-mode switch (AC-14), seeded from
  `Settings` persisted via `tauri-plugin-store`. A background check on
  startup kicks the first-run Whisper model downloader (`models`) if the
  selected preset is absent, emitting `model-download-progress`/
  `model-download-error` events (minimal — full onboarding UX is M5). New
  `commands.rs` handlers (`get_settings`, `set_settings`,
  `set_output_mode`, `download_selected_model`) expose this to a future
  settings UI. `set_settings` validates a new hotkey (pure
  `hotkeys::validate_hotkey`, the same parser registration uses) and
  registers it **before** persisting, so a malformed hotkey is rejected
  at the IPC boundary and never written; startup resolves the effective
  hotkey (`hotkeys::resolve_effective_hotkey` — persisted-if-valid, else
  the always-valid default) and registers it non-fatally, so a corrupt
  `settings.json` can't brick launch. `WhisperStt` is selected under
  `--features whisper` (`pnpm tauri:dev` / `pnpm tauri:build`); the
  default build (`cargo build`/`cargo test`, used by CI) compiles and
  runs with a clear "model engine unavailable" error path instead.
- `models` module (issue #24, ADR-0004, MISSION §5, PRD AC-12): the first-run
  Whisper model downloader. A registry of the two supported presets
  (quantized `large-v3-turbo` q5_0, the default, and `small`), each pinned
  to its `ggerganov/whisper.cpp` Hugging Face file name, download URL, exact
  size, and SHA-256 (from that repo's Git-LFS metadata). `download_url` and
  `is_allowlisted_url` are the AC-12 network guard's tested seam: every
  registry URL is asserted to resolve only to `huggingface.co`/`hf.co` and
  their subdomains (including the newer Xet-storage CDN hosts, e.g.
  `us.aws.cdn.hf.co`). The guard parses the URL with the **same `url` crate
  `ureq` itself resolves the connect target with**, so the host it checks
  cannot diverge from the host that's actually dialed — a battery of
  adversarial tests confirms it rejects lookalike hosts
  (`huggingface.co.evil.com`), the userinfo phishing trick
  (`https://huggingface.co@evil.com/`), authority-ambiguity bypasses
  (`https://evil.com?@huggingface.co`, `#@`, backslash variants), and
  non-`https` schemes. Checksum verification
  (`sha256_hex`/`sha256_hex_reader`/`verify_checksum`), progress-percent math
  (`compute_progress`, throttled to ~10 Hz so the callback doesn't fire per
  64 KB chunk), and resume-vs-restart planning (`plan_resume`) are pure and
  unit-tested; the actual HTTP GET, streaming-to-disk, and progress reporting
  live behind an injected `ModelTransport` trait, so
  `download_model_with_spec`'s orchestration (URL/allowlist selection, resume
  planning, checksum verification, target-path promotion) is exercised in
  tests against a fake in-memory transport — no real network call or
  downloaded model file needed. A resume only proceeds on an HTTP `206`
  response (a `200` full-body reply to a `Range` request restarts from
  scratch rather than appending full-onto-partial). A checksum mismatch
  always errors and removes the corrupt partial file rather than promoting
  it; the target path is only ever created after a verified checksum.
  `UreqTransport`, the real transport, sets explicit connect/read timeouts
  (so a black-hole host can't hang the first run) and re-checks every
  redirect hop against the same network guard via an injected per-hop
  responder seam (covered by tests: disallowed-host redirect, too-many-
  redirects, missing `Location`, and the `?@` bypass at the redirect layer),
  so the CDN-only egress invariant holds at the real network boundary too.
  Adds `sha2` (checksum hashing) and `url` (shared-parser guard) as
  dependencies and enables `ureq`'s `tls` (rustls-backed) feature, needed for
  the HTTPS download (the existing `OllamaCleanup` transport stays on plain
  HTTP to localhost). Not yet wired into `commands.rs`/the UI — that lands
  with the first-run downloader UI integration.
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

- **#98** Windows runtime-seam hardening pass (test-first, no OS access):
  `output::EnigoPaste`'s Cmd/Ctrl paste-modifier choice is now a pure,
  `cfg`-selected function (`output::paste_modifier`) instead of an inline
  `#[cfg]` block, unit-tested directly per platform; the file-mode path
  templating (`output::expand_template`/`append_entry`) and
  `models::model_target_path` are confirmed — with new discriminating
  tests, no behavior change — to resolve correctly for `/`-separated
  templates and Windows-style app-data bases respectively; and the
  persisted default hotkey (`settings::Settings::default().hotkey`) is
  confirmed to parse on every platform via the same accelerator grammar
  `tauri-plugin-global-shortcut` registers with, so a corrupt default could
  never leave `resolve_effective_hotkey`'s fallback with nothing valid to
  fall back to. `enigo`/`arboard` OS calls and `cpal`'s WASAPI selection on
  Windows remain thin glue; their real Windows runtime behavior is out of
  scope for this repo's macOS-only test suite and stays an AC-7 human
  smoke-test concern (the cofounder's pending `pnpm tauri:dev` run on
  Windows) — not something this pass verifies (#106).

### Performance

- **#115 follow-up** Opt-in perf instrumentation for the dictation hot path,
  so the caching/decode-tuning win can be measured in milliseconds instead of
  judged by feel. Set `BLA_PERF_LOG=1` (any non-`0`/non-empty value) before
  `pnpm tauri:dev` and stderr gains `bla[perf]:` lines for: the one-time
  ~574 MB model-load duration, each dictation's transcription time (sample
  count, approx audio seconds, ms, thread count), and per-dictation cache
  HIT/MISS plus background-warm markers — so a cache hit (no reload) is
  visibly distinct from a cold load. Off by default (a normal run stays
  silent); the env gate is a pure, unit-tested predicate
  (`stt::perf_logging_enabled`), and every line is numbers/enum labels only —
  never transcript, clipboard, or audio content (MISSION §7 no-log
  invariant). The timing call sites (`WhisperStt::new`,
  `WhisperStt::transcribe`, `build_stt`, `spawn_stt_cache_warm`) are native
  glue, exercised by the cofounder's `BLA_PERF_LOG=1` run.

- **#115** Cache the Whisper model across dictations instead of reloading it
  from disk on every one (the cofounder's smoke test found dictation working
  but slow: `WhisperContext::new_with_params` — a ~574 MB read for the
  default `large-v3-turbo` preset — was re-run per dictation).
  `AppState::stt_cache` now holds an `Arc<stt::WhisperStt>` keyed by the
  `settings::ModelPreset` it was built for; `lib.rs::build_stt` reuses that
  `Arc` (a refcount clone, not a reload) whenever the cache already holds the
  currently-selected preset, and rebuilds — replacing the cache entry — only
  when the preset changes or the cache is empty. The reuse-vs-rebuild
  decision is factored into a pure, unit-tested function
  (`should_reuse_cached_stt`); the `WhisperContext` build/store itself stays
  native glue (TDD-exempt) since it needs a real model file. The cache is
  also warmed in the background (`spawn_stt_cache_warm`, never on the
  main/UI thread) both at startup — if the selected model is already on disk
  — and right after the first-run model download completes (hooking the
  `model-download-complete` event added in #111) — so even the *first*
  dictation of a session is fast, not just the second one onward; a warm-up
  failure is logged and leaves the cache empty, falling back to the
  dictation path's own lazy build rather than panicking.
  `WhisperStt::transcribe` is unchanged in shape — it still creates a fresh
  `WhisperState` per call via `create_state()`, the correct cheap per-call
  scratch; only the expensive `WhisperContext` load is now shared/cached.
  Also (behind `--features whisper`): flash attention is enabled on the
  context (`WhisperContextParameters::flash_attn(true)`) and decoding now
  uses every available core (`FullParams::set_n_threads`,
  `std::thread::available_parallelism()`, falling back to 4) instead of
  whisper.cpp's conservative `min(4, hardware_concurrency())` default — both
  pure decode-latency wins verified against the actual whisper-rs 0.16
  source (native glue, TDD-exempt; the cofounder's re-run is the real
  latency verification).

### Fixed

- **#118 / #117** `build_stt` no longer holds the `stt_cache` mutex across
  the multi-second `WhisperStt::new` model load. The dictation path now
  mirrors `spawn_stt_cache_warm`'s pattern — check for a cache hit under a
  narrow lock scope, release, load the ~574 MB model with no lock held, then
  re-acquire and re-check before populating. Before this fix, a panic inside
  the native load (e.g. a corrupt/truncated model) unwound while holding the
  guard, poisoning the mutex so every later dictation *and* the background
  warm panicked on `lock().unwrap()` — leaving dictation dead until an app
  restart (#118). Loading outside the lock also stops a first-launch
  dictation and the background warm from serializing on, or redundantly
  double-loading, the model (#117).
- **#65** `output::paste_via_clipboard_swap` now restores the saved
  clipboard on every error path (a failing paste synthesizer — e.g.
  `enigo` failing on first-run macOS before Accessibility is granted —
  or a failing post-paste observation read), not just the happy path;
  before this fix, either failure returned early via `?` and permanently
  left the transcript on the clipboard.
- **#58** `audio::start_capture`'s real-time callback now reuses two
  pre-allocated scratch buffers (`downmix_resample_into`) instead of
  allocating two fresh `Vec`s per callback, and uses `try_lock` instead
  of a blocking `lock()` — a contended buffer lock drops that callback's
  samples and counts the drop (`CaptureDiagnostics`) rather than
  stalling the real-time audio thread.
- **#59** audio capture errors (a poisoned ring-buffer lock, a `cpal`
  stream error) are now recorded as structured `CaptureRuntimeError`
  state (`CaptureDiagnostics`) instead of an invisible `eprintln!`,
  readable by the rest of the app.
- **#73** `cleanup::UreqTransport` now sets a write timeout (mirroring
  the read timeout) and an overall request timeout on its `ureq::Agent`,
  in addition to the existing connect/read timeouts — a peer that
  accepts the connection but stops draining could previously block
  `send_string` forever on a large-enough request body, defeating the
  AC-4 fallback.
- **#80** `settings::SettingsStore::load` now returns
  `Result<Settings, SettingsLoadError>` — distinguishing `NotFound`
  (first run; safe to silently default) from `Corrupt` (something was
  persisted but failed to parse) — instead of silently collapsing any
  load failure to `Settings::default()` with no signal.
- **#86** the AC-5 privacy-guard test no longer relies on
  `static_assertions::assert_type_ne_all!` (a tautology: any two
  distinct named types are always "not equal" to that macro, so it
  could never catch a real transport being substituted in). Replaced
  with `cleanup::NoRealNetworkTransport`, a sealed marker trait granted
  only to the exported `cleanup::StubTransport` test double and
  deliberately never to `UreqTransport` — a `compile_fail` doctest
  proves the negative space.
- **#44** `hotkeys::StateMachine` gained a `reset()`/reconcile entry the
  runtime wiring calls on window focus-loss: previously, a dropped
  `KeyUp` (focus loss, screen lock, sleep/resume) left the machine
  permanently wedged in `Holding` with a stale held-key set, silently
  swallowing every subsequent hotkey press.
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
