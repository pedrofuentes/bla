# Changelog — bla

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.3.0] — 2026-07-17 (M3: context features)

### Added

- Headless SQLite `Store` foundation for dictation history (kickoff #160): a
  numbered, idempotent `PRAGMA user_version` migration runner creates the
  `history` table (raw + cleaned text, timestamp, source app), with
  insert/search/delete/clear operations and a pure `retention_cutoff_ms`
  helper for future auto-pruning.
- History capture backend (M3 PR 3.2, issue #198): every completed dictation
  now persists a history row (raw + cleaned transcript) to the local SQLite
  store, including a dictation whose generation went stale for UI purposes —
  the text was already pasted, so it's never dropped from history. New IPC
  commands `search_history`, `copy_history_entry` (routed through the
  existing clipboard seam, never a bare loggable `String`), `delete_history_entry`,
  and `clear_history` back the History tab. A new `Settings.retention_days`
  (`0` = keep forever) prunes history on startup and after every settings save
  once set above zero.
- History settings tab (M3 PR 3.3, issue #199): the settings window's
  History tab now shows your dictation history — a substring search box,
  a result list (timestamp, source app when known, cleaned-text preview)
  with per-entry Copy/Delete, a "Clear all" gated behind an inline confirm
  (never a native dialog), and a "Keep history for" control bound to the
  retention-days setting (0 = keep forever) that persists immediately.
- Personal dictionary backend (M3 PR 3.4, issue #200): a new `dictionary`
  table (migration 2 on the `Store` foundation from #160/#198) with
  case-insensitive term uniqueness (`UNIQUE COLLATE NOCASE` — adding
  "Kubernetes" then "kubectl" keeps two rows, but "kubernetes" a second time
  is a no-op) and CRUD (`Store::add_term`/`list_terms`/`remove_term`, plus
  the `list_dictionary_terms`/`add_dictionary_term`/`remove_dictionary_term`
  IPC commands). Dictionary terms now actually flow into both sides of the
  pipeline: Whisper's `initial_prompt` (`TranscribeOpts.dictionary`) and a
  new versioned, rewrite-only `cleanup_v2` prompt (`prompts/cleanup_v2.txt`,
  superseding `cleanup_v1.txt` as `OllamaCleanup`'s live system prompt)
  carrying a `{{DICTIONARY}}` placeholder substituted with the current terms
  at call time.
- Dictionary settings tab (M3 PR 3.5, issue #201): the settings window's
  Dictionary tab now lists your personal-dictionary terms
  (`list_dictionary_terms`), an add-term input calling `add_dictionary_term`
  with immediate list update, and per-term Remove calling
  `remove_dictionary_term`. Because the backend's case-insensitive
  uniqueness constraint (`dictionary(term UNIQUE COLLATE NOCASE)`, #200)
  makes a duplicate add a silent no-op rather than a rejected call, the
  tab validates client-side — blank/whitespace-only input and a
  case-insensitive duplicate of an existing term both surface inline
  feedback and never reach the backend.
- Per-app tone backend (M3 PR 3.6, issue #202): `context.rs` now detects the
  focused application at hotkey-press time (`active-win-pos-rs`, app NAME
  only — never a window title, to avoid triggering the macOS Screen
  Recording permission prompt) and resolves it against a new `tone_rules`
  table (migration 3 on the `Store` foundation: `app_pattern` →
  `casual`/`formal`/`verbatim`, `UNIQUE COLLATE NOCASE` + a `CHECK`
  constraint, upsert/list/delete CRUD) via a pure, glob/case-insensitive
  pattern matcher. `Tone` gains `Casual`/`Formal` variants alongside the
  existing `Neutral`/`Verbatim`; `OllamaCleanup` now renders a new
  `cleanup_v3` prompt (superseding `cleanup_v2`, which stays untouched) with
  a `{{TONE}}` placeholder carrying a tone-specific writing-style
  instruction. `run_pipeline_in_background` now resolves and dispatches the
  matching tone on every dictation instead of a hardcoded `Tone::Neutral`,
  with detection failure degrading silently to `Neutral` (never surfaced to
  the paste path) — and the detected app name now also reaches
  `history.app_name`, previously always `None`. New
  `list_tone_rules`/`upsert_tone_rule`/`delete_tone_rule` IPC commands.
- Tone settings tab (M3 PR 3.7, issue #203): the settings window's Tone tab
  replaces the "coming soon" placeholder with a real UI for the per-app
  tone rules `list_tone_rules`/`upsert_tone_rule`/`delete_tone_rule` (#202)
  already persist. Rules render as a numbered list in the exact order
  `context::resolve_tone_for_app` walks (insertion order, `id` ASC) with a
  "checked top to bottom — the first matching pattern wins" note, so the
  list visually communicates match priority; there's no reorder command on
  the backend, so none is offered. An add-rule form (app pattern +
  casual/formal/verbatim picker) calls `upsert_tone_rule`, appending the new
  rule at the end of the list to match its real insertion-order position;
  each row's tone picker also calls `upsert_tone_rule` (keyed on that row's
  own pattern) to edit a rule's tone in place, and a per-row Remove calls
  `delete_tone_rule`. Because the backend's upsert-by-pattern is a silent
  update rather than a rejected call on a duplicate, blank/whitespace-only
  and case-insensitive-duplicate patterns are rejected client-side before
  the add call is ever placed, mirroring the Dictionary tab's validation
  pattern.
- File-mode output-path/template picker (issue #180, AC-7 #173 p0): the
  settings window's General tab now has an Output section — a
  Paste-at-cursor/Append-to-a-file mode switch, plus, when File mode is
  selected, a "Base folder (vault)" field and a `{{date:YYYY-MM-DD}}`-templated
  path field (e.g. `daily/{{date:YYYY-MM-DD}}.md` for an Obsidian daily
  note). Switching to File mode now actually writes into the folder you
  choose instead of a fixed, unchangeable default location; an invalid path
  template (absolute, or one that escapes the base folder) shows an inline
  error and is never saved.
- Model download sizes in the settings picker (issue #184): the General
  tab's model `<select>` now shows each Whisper preset's download size
  (e.g. "Small — 488 MB"), sourced from a new `model_registry` command.

### Changed

- Settings window auto-apply (issue #183, AC-7 smoke test): removed the
  General tab's Save button — the recording mode, model preset,
  launch-at-login and sound-cues controls now apply immediately on change,
  with a brief "Saved" confirmation or an inline error, instead of
  requiring a separate Save click that caused false-alarm bug reports in
  the AC-7 smoke test.
- Hotkey rebinding uses an explicit Apply button (issue #187, cofounder
  decision): in the settings General tab, changing the dictation hotkey now
  captures the new chord as a pending value and only registers + saves it
  when you click Apply (the other settings still apply instantly).
  Capturing still suspends the live global shortcut so keystrokes are
  grabbed for rebinding, and fully restores the current hotkey if you
  cancel — and an unbindable chord (already claimed by another app) is
  reported inline and never persisted, so it can't leave dictation without
  a working hotkey.
- Chore/docs/test batch closing three Sentinel follow-ups (issues #197,
  #211, #213): declared an explicit `"engines": { "node": ">=20.19" }`
  floor in `package.json` (plus a matching `.nvmrc`), consistent with
  `@vitejs/plugin-react` 5.x's own Node requirement; updated the README's
  file-output section to document the Settings → Output "Base folder
  (vault)" and path-template fields (the Obsidian daily-note flow) instead
  of the old fixed app-data-location description; and made the pill
  listener test's `console.error` spy restore reliably by setting
  `restoreMocks: true` in `vitest.config.ts`.
- Test hygiene (issue #205): vitest now excludes `.worktrees/**` and
  `.claude/worktrees/**` from test discovery to prevent false test counts
  and spurious failures when worktrees are present locally.
- Test batch closing seven Sentinel test-quality follow-ups plus a
  format-drift fix (issues #209, #224, #225, #226, #234, #238, #240, part
  of #217): added negative "not committed before blur" assertions to
  GeneralTab's file-mode fields and HistoryTab's retention control; added a
  HistoryTab test exercising the `searchSeqRef` out-of-order-search guard
  and a `commitRetention` rejection/revert-path test; added DictionaryTab
  controlled-promise tests for the add/remove in-flight button states;
  added a ToneTab test exercising the per-row `editGenRef` guard against a
  stale REJECT clobbering a newer, already-applied edit; replaced the
  `tests/visual/settings-harness.tsx` Tone-tab fixture's non-synthetic
  `*Terminal` app pattern with a synthetic `*SynthTerm` and recaptured the
  affected `docs/design/screenshots/settings-tone-*.png` screenshots; and
  ran Prettier on `src/lib/pathTemplate.ts` to fix format drift. No
  production behavior changed.
- Test batch closing three Sentinel test-quality follow-ups (issues #221,
  #229, #239): the busy_timeout regression test (#162/#221) is honestly
  non-discriminating on its own — rusqlite 0.40.1's internal default
  happens to also produce a 5000ms `PRAGMA busy_timeout`, so removing
  `from_connection`'s explicit `conn.busy_timeout(...)` call still leaves
  that assertion green — so a second, source-level test now parses
  `from_connection`'s own body out of `store.rs` and asserts the explicit
  call is textually present, pinning the reviewable contract; added
  fixture regression tests pinning `cleanup_v2`/`cleanup_v3`'s rule 7
  anti-hallucination clause ("never insert a dictionary term the speaker
  did not actually say"); and added a class-level wire-key contract guard
  — a `commands.rs` test that parses every `#[tauri::command]` fn out of
  its own source and asserts any multi-word snake_case argument carries
  `rename_all = "snake_case"`. No production behavior changed.

### Fixed

- Fixed the recording pill/tray icon being able to get stuck out of sync
  with the actual pipeline state (issue #128, escalated by Sentinel on PR
  #127): `AppState` now holds pipeline state and pill visibility together
  as one `tray::PipelineDisplay` behind a single mutex, and the
  main-thread closure re-derives what to show (`tray::resolve_display`)
  from whatever `AppState` currently holds at the moment it actually runs,
  closing an ordering race between overlapping same-generation state
  transitions instead of papering over the common case.
- Fixed overlapping dictations clobbering each other's pipeline state
  (issues #176, #175, #174): a per-dictation generation id (minted at
  `StartRecording`, mirroring the existing pill-visibility-epoch pattern)
  is now threaded through the background transcription thread and every
  settle it can spawn; a completion whose generation no longer matches the
  live dictation now no-ops entirely instead of clobbering the newer
  dictation. The settle thread's delayed pill-hide also now locks
  `pipeline_state` before reading the epoch/generation atomics, closing a
  narrow TOCTOU window.
- Fixed the recording pill's waveform appearing flat/dead during dictation
  (issue #179, AC-7 #173): a new pure `scaleLevelForDisplay`
  (`src/lib/waveform.ts`) applies a perceptual `sqrt(rms) * 2.5` gain so
  speech-level RMS now visibly fills most of the bar while silence stays
  at the floor.
- Suppressed the global dictation hotkey during capture (issue #181, AC-7
  smoke test): the settings window's hotkey-capture field now temporarily
  unregisters the live global shortcut (`suspend_hotkey`) while active, so
  keypresses are captured for rebinding instead of also starting a
  dictation, restoring it (`resume_hotkey`) on cancel/blur/invalid capture.
- Fixed the recording pill blanking to "Status unavailable" on a single
  failed event subscription (issue #182): a rejected
  `audio-level`/`pipeline-state-changed`/`pipeline-error` subscription now
  degrades only the feature it feeds (e.g. the live waveform falls back to
  the state dot) instead of masking the whole pill; the fallback is
  reserved for every subscription failing, and a listener's later
  successful (re)subscription always clears its own prior failure.
- Fix (#162): `Store::from_connection` now explicitly sets a 5s
  `busy_timeout` before running migrations, so a transient lock on the
  on-disk history DB (a second app instance, a crash-relaunch race, an OS
  indexer/backup read lock) blocks and retries instead of failing the
  write immediately with `SQLITE_BUSY` and dropping a history row.
- Fix (#69): a dictionary term containing a NUL byte no longer panics
  whisper-rs's `set_initial_prompt` — `build_initial_prompt` strips NUL
  bytes before any other processing.
- Fix (#70): once one dictionary term overflowed `initial_prompt`'s length
  cap, every *subsequent* term used to be silently dropped along with it;
  `build_initial_prompt` now skips only the oversized term and keeps
  packing whatever else fits. `Store::list_terms` also orders
  most-recently-added first, so a dictionary too large for the cap loses
  its oldest terms first, not an arbitrary subset.
- Fix (#163): a `schema_migrations` ledger now backs the migration
  runner's version guard with a real discriminating test — a
  silently-broken guard now fails loudly (a ledger PRIMARY KEY violation)
  on the very next reopen instead of passing every existing test.
- Fix (#219): the retention prune's cutoff now guards against a
  clock-skewed `now` — pruning is skipped entirely if `now` reads before
  the newest recorded history row (proof of a backward clock jump), and
  the cutoff is otherwise clamped so it can never exceed the newest row's
  own timestamp. A backwards clock jump can no longer mass-delete history
  once retention is user-configurable (#199).
- Hardened the file-output write path against a symlink/TOCTOU gap (issue
  #208, flagged by Sentinel on PR #204). Confining a dictation's templated
  path to the configured base folder was purely lexical — it rejected
  absolute paths and `..` traversal but never touched the filesystem — so
  a symlink pre-planted inside the base folder (or swapped in between the
  confine step and the write) could redirect the append outside the
  confined tree. The write now canonicalizes the resolved parent directory
  (after creating it) and refuses the write unless it still resolves under
  the canonicalized base, refuses a pre-existing symlink at the final path
  component, and on macOS/Linux opens the target with `O_NOFOLLOW` so a
  symlinked final component makes the open fail atomically rather than
  being followed. A refused write surfaces through the existing kind-only
  error path (no file path or dictation text is ever logged or sent to the
  UI). This is same-user-bounded defense-in-depth, not a privilege
  boundary.
- Two Sentinel follow-ups fixed together (issues #210, #220): the Settings
  → Output "Base folder (vault)" field now validates absoluteness
  client-side (`src/lib/baseDir.ts`'s `validateBaseDir`) — since
  `output::resolve_base_dir` uses the configured string verbatim (never
  expanding `~`, never resolving a relative path against anything but the
  process's CWD at write time), a relative value previously wrote into an
  unexpected, launch-inconsistent location with no warning. Separately,
  `Store::insert_history` failures at the `run_pipeline_in_background` call
  site — previously `eprintln!`-only, invisible in a packaged GUI and a
  silent history-row loss — now also emit a new, informational
  `errors::ErrorKind::HistoryPersistFailed` through the existing typed
  `pipeline-error` event surface (kind-only, no data from the underlying
  `rusqlite::Error`), which the pill renders as a non-blocking "Couldn't
  save this dictation to history." toast; the row insert itself stays
  unconditional but the toast is gated behind `generation_is_live` like
  every other UI-visible effect in that function.
- Fixed a Sentinel finding on PR #245's cross-platform base-folder fix
  (issue #246): `validateBaseDir` previously accepted either platform's
  absolute syntax regardless of the runtime OS — a synced `settings.json`
  carrying a Windows `C:\...` form onto macOS (or a bare POSIX `/foo` onto
  Windows) passed client-side validation even though
  `output::resolve_base_dir` runs Rust-side against THIS machine,
  reproducing #210's CWD-relative-write failure mode. `validateBaseDir`
  now takes the runtime platform as an explicit parameter and accepts only
  that platform's absolute form, rejecting a foreign-platform form with a
  distinct "Not an absolute path on this system…" inline error. The
  settings window fetches the runtime platform once on mount via a new,
  trivial, zero-argument `get_platform` command and passes it to the
  validator on every base-folder change/blur.
- Rust core-logic hardening batch closing three triaged findings (issues
  #61, #71, #74): `write_wav_16k_mono` now writes to a sibling temp file
  and only renames it onto the destination once the WAV is fully written
  and finalized, so a mid-write error can no longer leave a
  truncated/corrupt WAV file behind; `WhisperStt::transcribe` now decodes a
  whisper.cpp segment's text lossily instead of silently dropping it when
  it isn't valid UTF-8, and emits a stderr warning (a count only, never
  the decoded text) when that happens; and `classify_ureq_error`'s timeout
  classification now checks the typed `io::ErrorKind::TimedOut` on the
  transport error's wrapped source instead of substring-matching its
  rendered message.

## [0.2.0] — M2: UI shell (pending AC-7 cofounder smoke test)

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

## [0.1.0] — M1: MVP dictation pipeline (pending AC-7 gate #27)

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
