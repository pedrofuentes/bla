//! Tauri setup, tray, and window management (see docs/ARCHITECTURE.md
//! §Project Structure — this crate root fills the role described there as
//! `main.rs`; `main.rs` itself is the thin binary entry point required by
//! Tauri's mobile target and just calls [`run`]).
//!
//! Module boundaries (AGENTS.md, docs/ARCHITECTURE.md §Module Boundaries):
//! - `cleanup`, `store`'s pure-logic layer, and path-templating/tone/snippet
//!   logic are OS-call-free and TDD-mandatory.
//! - `audio`, `output`, `hotkeys`, `context` are the only modules allowed to
//!   touch platform APIs (OS-integration exemption) and stay thin.
//! - The UI reaches the core only through `commands` (IPC), mirrored on the
//!   frontend by `src/lib/ipc.ts`.
//!
//! ## Runtime wiring (issue #91)
//!
//! This is the OS-glue layer (thin, TDD-exempt) that connects the
//! headlessly-proven modules into the live Tauri app: registers the
//! configured global hotkey, drives the pure `hotkeys::StateMachine`,
//! starts/stops `audio` capture, runs `pipeline::Pipeline` on
//! `StopRecording`, and routes the result per `Settings`. Every decision —
//! debounce, cleanup fallback, output dispatch, clipboard restore — lives in
//! the modules already covered by their own unit/acceptance tests; nothing
//! new here beyond wiring.
//!
//! `WhisperStt` is behind the default-off `whisper` cargo feature (see
//! `Cargo.toml`; `stt.rs`'s module doc). [`build_stt`] compiles to the real
//! engine under `--features whisper` and to a "model engine unavailable"
//! `Err` in the default build, so both `cargo build` and
//! `cargo build --features whisper` compile and this file never has a
//! feature-gated call site — only `build_stt`'s two bodies differ.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_store::StoreExt;

/// Id of the single system-tray/menu-bar icon this app creates (issue #110),
/// used to look it up again from `set_pipeline_state` via
/// [`tauri::Manager::tray_by_id`].
const TRAY_ID: &str = "bla-tray";

/// Label of the always-on-top recording pill window (issue #126, M2 PR 2.1;
/// see `tauri.conf.json`'s `app.windows` and `src/windows/pill/index.tsx`).
/// `set_pipeline_state` looks it up by this label to show/hide it per
/// [`tray::pill_visibility_for`].
const PILL_WINDOW_LABEL: &str = "pill";

/// Label of the full tabbed settings window (issue #126, M2 PR 2.1; see
/// `tauri.conf.json`'s `app.windows` and `src/windows/settings/index.tsx`).
/// The tray's "Settings…" item looks it up by this label to show + focus it.
const SETTINGS_WINDOW_LABEL: &str = "settings";

pub mod audio;
// `pub` (rather than private like their stub siblings): as of the pipeline
// increment (issue #25), `cleanup`/`output`/`pipeline` are real, tested,
// standalone-usable API surface — `pipeline` composes `Stt` + `Cleanup` +
// the output router headlessly, and the cumulative acceptance suite
// (`tests/acceptance.rs`) exercises them from outside the crate.
pub mod cleanup;
mod commands;
mod context;
// `pub` (issue #126, M2 PR 2.4): the typed `pipeline-error` event vocabulary
// and its pure mapping functions are exercised from `tests/acceptance.rs`
// (the crate's cumulative headless suite) as well as this crate's own unit
// tests.
pub mod errors;
mod hotkeys;
// `pub` (issue #24, ADR-0004): the first-run model downloader's registry,
// AC-12 network guard, and download orchestration are real, tested,
// standalone-usable API surface.
pub mod models;
pub mod output;
pub mod pipeline;
pub mod settings;
pub mod store;
pub mod stt;
pub mod tray;

/// The Whisper engine cached in [`AppState::stt_cache`] (issue #115), keyed
/// by the [`settings::ModelPreset`] it was built for so a later preset
/// switch is detected (see [`should_reuse_cached_stt`]) rather than silently
/// serving transcriptions from the wrong model. `Arc` (not a bare
/// `WhisperStt`) so the cache can hand a dictation thread a cheap refcount
/// clone of the already-loaded engine instead of moving/rebuilding it —
/// `whisper_rs::WhisperContext` is `Send + Sync`, and `WhisperStt::transcribe`
/// still creates a fresh `WhisperState` per call (the correct cheap per-call
/// scratch; only the expensive context load itself is shared/cached here).
#[cfg(feature = "whisper")]
struct CachedStt {
    preset: settings::ModelPreset,
    stt: Arc<stt::WhisperStt>,
}

/// Shared runtime state the OS glue below drives (issue #91): the hotkeys
/// state machine, the live audio capture session, and pipeline/output
/// state. Everything is behind a `Mutex` since Tauri commands and plugin
/// callbacks (the global-shortcut handler, window events) can run from
/// different threads.
pub(crate) struct AppState {
    hotkeys: Mutex<hotkeys::StateMachine>,
    buffer: audio::SharedRingBuffer,
    diagnostics: Arc<audio::CaptureDiagnostics>,
    capture: Mutex<Option<audio::CaptureSession>>,
    /// Issue #126 (M2 PR 2.2): the RT-safe latest-level cell the capture
    /// callback records into; `react_to_transition`'s level-event poller
    /// samples it and throttles via `audio::LevelThrottle` before emitting
    /// the `audio-level` event.
    level_meter: Arc<audio::LevelMeter>,
    /// Stop signal for the currently-running level-event poller, if any
    /// (issue #126). Set on `StartRecording`, taken and signaled on
    /// `StopRecording`/`Cancelled` so exactly one poller is ever driving
    /// `audio-level` events at a time.
    level_poll_stop: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    settings: Mutex<settings::Settings>,
    output_switch: Mutex<tray::OutputModeSwitch>,
    pipeline_state: Mutex<tray::PipelineState>,
    /// The tray menu's disabled current-state line (issue #110):
    /// `set_pipeline_state` keeps its text in sync with the emitted
    /// `pipeline-state-changed` event/icon. `None` until `setup()` builds
    /// the tray (always `Some` afterward).
    tray_state_item: Mutex<Option<MenuItem<tauri::Wry>>>,
    /// The tray menu's Cursor/File output-mode toggle line (issue #110):
    /// kept in sync by `commands::set_output_mode` — the same command path
    /// both this menu item and the status window's toggle button call —
    /// so tray- and window-triggered switches never disagree about which
    /// mode is live.
    tray_output_toggle_item: Mutex<Option<MenuItem<tauri::Wry>>>,
    /// Monotonic "pill visibility epoch" (issue #155; Sentinel 🔴 on PR
    /// #137's re-review): bumped by [`settle_idle_keeping_pill_visible`]
    /// every time a "keep the pill visible for a while, then maybe hide it"
    /// settle starts (the AC-4 notice path or the issue-#151 "done"
    /// confirmation path). Each settle's delayed-hide thread captures the
    /// post-bump value and only proceeds to hide if it's still current when
    /// it wakes ([`tray::should_hide_pill_for_settle`]) — so a second,
    /// overlapping settle starting first makes the first one stand down
    /// instead of hiding the pill out from under the second one's own still-
    /// live visible window.
    pill_visibility_epoch: AtomicU64,
    /// Per-dictation generation id (issues #174/#175/#176): bumped once at
    /// the start of every dictation (`react_to_transition`'s
    /// `StartRecording` arm), BEFORE anything else runs. The value in effect
    /// when `StopRecording` kicks off `run_pipeline_in_background` is
    /// carried through to that background thread and every settle helper it
    /// calls; each checks it's still `== dictation_generation.load()` (via
    /// [`tray::should_apply_dictation_completion`] /
    /// [`tray::should_hide_pill_for_settle`]) before any state write, event
    /// emit, or settle spawn.
    ///
    /// **Why this exists:** the hotkeys `StateMachine` resets to
    /// `Phase::Idle` synchronously on `StopRecording`, before the
    /// transcription thread it kicked off has returned — so a second
    /// dictation can already be recording/transcribing by the time the
    /// first one's background completion runs. Without a per-dictation
    /// identity, that stale completion reads/writes the single shared
    /// `AppState.pipeline_state` slot, clobbering the live dictation's state
    /// (dropping its waveform, showing wrong pill chrome, emitting a stray
    /// event) for anywhere from an instant up to the completion's full
    /// settle-visibility window. A stale generation means "no-op entirely."
    dictation_generation: AtomicU64,
    /// PR #185 Sentinel 🔴-1: the generation of the latest outstanding
    /// hotkey-capture suspend (`commands::suspend_hotkey`), or `0` when the
    /// global dictation hotkey is not suspended. A monotonic token minted by
    /// the settings window and echoed back on `commands::resume_hotkey` so
    /// out-of-order IPC can't clobber a live capture (see
    /// [`should_resume_hotkey`]); `force_resume_hotkey` resets it to `0` when
    /// it restores the shortcut on window close.
    hotkey_suspend_gen: Mutex<u64>,
    /// Issue #115: the cached Whisper engine, so it's loaded from disk (a
    /// ~574 MB read for the default preset) at most once per selected
    /// preset rather than on every dictation. `None` until the first build
    /// (lazily, from `build_stt`, or eagerly from a background warm —
    /// see `spawn_stt_cache_warm`). Only present in `--features whisper`
    /// builds; the default build has no `WhisperStt` to cache.
    #[cfg(feature = "whisper")]
    stt_cache: Mutex<Option<CachedStt>>,
    /// Issue #198 (M3 PR 3.2): the headless SQLite history store (`store.rs`,
    /// kickoff #160/#161), opened once at startup against the OS app-data
    /// dir. `Mutex`-wrapped for the same reason every other shared field
    /// here is: `rusqlite::Connection` is `!Sync`, and both Tauri command
    /// handlers and the background pipeline thread
    /// (`run_pipeline_in_background`) need to reach it.
    store: Mutex<store::Store>,
    /// Issue #202 (M3 PR 3.6): the active app detected at the START of the
    /// current/most recent dictation (`react_to_transition`'s
    /// `StartRecording` arm — "matched at hotkey-press time" per #202's
    /// plan, since the user may have already switched focus, e.g. to the
    /// recording pill itself, by the time `StopRecording` fires). Read by
    /// `run_pipeline_in_background` to resolve the dictation's `Tone` via
    /// `context::resolve_tone_for_app` and to tag the persisted history row.
    /// `None` when detection failed (no active window, permission denied)
    /// or before the very first dictation.
    active_app_name: Mutex<Option<context::ActiveAppName>>,
}

/// Max capacity of the capture ring buffer: a generous 5 minutes at 16 kHz
/// mono — comfortably above a typical dictation utterance (AC-2 budgets a
/// 15s fixture) without holding an unbounded amount of audio in memory for a
/// hotkey session someone forgot to release.
const MAX_CAPTURE_SECONDS: usize = 300;

/// Translate the persisted [`settings::RecordingMode`] to the pure hotkey
/// state machine's [`hotkeys::Mode`]. Total (exhaustive match — the
/// compiler enforces every `RecordingMode` variant is covered).
fn to_hotkey_mode(mode: settings::RecordingMode) -> hotkeys::Mode {
    match mode {
        settings::RecordingMode::Hold => hotkeys::Mode::Hold,
        settings::RecordingMode::Toggle => hotkeys::Mode::Toggle,
    }
}

/// Translate the persisted [`settings::OutputModeSetting`] to the tray's
/// live [`tray::OutputMode`] switch value.
fn to_tray_output_mode(mode: settings::OutputModeSetting) -> tray::OutputMode {
    match mode {
        settings::OutputModeSetting::Cursor => tray::OutputMode::CursorPaste,
        settings::OutputModeSetting::File => tray::OutputMode::File,
    }
}

/// Apply an already-validated (and persisted) `settings` value to the live
/// in-memory recording-mode / output-mode / settings-snapshot state: flips
/// the hotkeys state machine's recording mode (issue #126 / PR #134
/// Sentinel 🔴-3 — before this, the machine was built once at startup and a
/// saved Hold↔Toggle change only took effect after a restart while the UI
/// said "Saved"), updates the live output-mode switch (AC-14), and replaces
/// the in-memory settings snapshot.
///
/// Takes the three `Mutex`es it actually reads/writes rather than the whole
/// [`AppState`] (see [`apply_settings_to_state`], the thin `AppState`
/// wrapper `commands::set_settings` calls) — issue #165: a `#[cfg(test)]`
/// helper that built a full `AppState` struct literal (to exercise this
/// logic without a live Tauri app) reproducibly crashed the Windows lib
/// test binary at process startup with `STATUS_ENTRYPOINT_NOT_FOUND` before
/// any test ran, even though none of `AppState`'s Tauri-runtime-typed
/// fields (`Mutex<Option<MenuItem<tauri::Wry>>>`,
/// `Mutex<Option<audio::CaptureSession>>`) were ever populated or read by
/// this function — bisected on PR #134's own branch: identical Cargo.lock,
/// only this function + its `AppState`-constructing test added, ~30 minutes
/// apart, flipped Windows `cargo test --all-features` from green to that
/// crash. Narrowing the signature to exactly the state this function
/// touches keeps every assertion the removed test made, without ever
/// constructing an `AppState` (or any native-runtime-typed field) from test
/// code.
///
/// Returns the [`hotkeys::Transition`] the mode flip produced —
/// `Some(Cancelled)` when it interrupted an in-flight session — for the
/// caller to hand to [`react_to_transition`], which stops capture and
/// discards the buffered audio.
fn apply_settings(
    hotkeys: &Mutex<hotkeys::StateMachine>,
    output_switch: &Mutex<tray::OutputModeSwitch>,
    settings_slot: &Mutex<settings::Settings>,
    settings: settings::Settings,
) -> Option<hotkeys::Transition> {
    let transition = hotkeys
        .lock()
        .unwrap()
        .set_mode(to_hotkey_mode(settings.recording_mode));
    output_switch
        .lock()
        .unwrap()
        .set_mode(to_tray_output_mode(settings.output_mode));
    *settings_slot.lock().unwrap() = settings;
    transition
}

/// `AppState`-shaped wrapper over [`apply_settings`] — the entry point
/// `commands::set_settings` calls. Takes no `AppHandle`, so this whole
/// state-application step is unit-testable without a live Tauri app; the
/// pure logic itself is unit-tested directly against bare `Mutex`es (no
/// `AppState` involved at all — see `apply_settings`'s doc comment and
/// `apply_settings_tests`, issue #165).
pub(crate) fn apply_settings_to_state(
    state: &AppState,
    settings: settings::Settings,
) -> Option<hotkeys::Transition> {
    apply_settings(
        &state.hotkeys,
        &state.output_switch,
        &state.settings,
        settings,
    )
}

/// Translate the persisted [`settings::ModelPreset`] to the models
/// downloader's registry [`models::ModelPreset`].
fn to_models_preset(preset: settings::ModelPreset) -> models::ModelPreset {
    match preset {
        settings::ModelPreset::LargeV3Turbo => models::ModelPreset::LargeV3TurboQ5,
        settings::ModelPreset::Small => models::ModelPreset::Small,
    }
}

/// Look up the full [`models::ModelSpec`] for `preset` from the registry.
/// `models::model_registry()` always covers every [`models::ModelPreset`]
/// variant (asserted by that module's own tests), so this never panics in
/// practice; the `expect` documents that invariant rather than masking a
/// real fallibility.
fn spec_for_preset(preset: models::ModelPreset) -> models::ModelSpec {
    models::model_registry()
        .into_iter()
        .find(|spec| spec.preset == preset)
        .expect("model_registry() covers every ModelPreset variant")
}

/// Translate the models downloader's registry [`models::ModelPreset`] back
/// to the persisted [`settings::ModelPreset`] — the inverse of
/// [`to_models_preset`], used by [`model_registry_entries`] to key each
/// entry the same way `Settings.model_preset` already is on the frontend.
fn to_settings_preset(preset: models::ModelPreset) -> settings::ModelPreset {
    match preset {
        models::ModelPreset::LargeV3TurboQ5 => settings::ModelPreset::LargeV3Turbo,
        models::ModelPreset::Small => settings::ModelPreset::Small,
    }
}

/// One entry of the model picker's registry, mirrored to the frontend via
/// `commands::model_registry` (issue #184): a settings-safe preset id plus
/// its exact download size in bytes, so the UI can render e.g. "Small — 488
/// MB" using its own `formatBytes`/`modelPresetLabel` rather than
/// duplicating that formatting on the Rust side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct ModelRegistryEntry {
    pub preset: settings::ModelPreset,
    pub size_bytes: u64,
}

/// Pure data behind `commands::model_registry` (issue #184): every
/// supported preset's settings-safe id and exact download size, sourced
/// from `models::model_registry()` — the single source of truth for
/// `size_bytes` — and translated so the frontend can key each entry
/// directly against `Settings.model_preset`.
pub(crate) fn model_registry_entries() -> Vec<ModelRegistryEntry> {
    models::model_registry()
        .into_iter()
        .map(|spec| ModelRegistryEntry {
            preset: to_settings_preset(spec.preset),
            size_bytes: spec.size_bytes,
        })
        .collect()
}

/// Label for the tray menu's output-mode toggle line (issue #110): names
/// the mode the click would switch *to*, not the current mode, matching how
/// a toggle control conventionally reads.
fn output_mode_toggle_label(current: tray::OutputMode) -> String {
    match current {
        tray::OutputMode::CursorPaste => "Switch to File output".to_string(),
        tray::OutputMode::File => "Switch to Cursor output".to_string(),
    }
}

/// Issue #115's pure reuse-vs-rebuild decision for the cached Whisper
/// engine: `true` only when a cached engine exists (`cached: Some(_)`) AND
/// it was built for exactly the currently-selected `wanted` preset.
/// Anything else — nothing cached yet, or the cached engine is for a
/// *different* preset than the one now selected (the user switched models)
/// — must rebuild. `build_stt`/`spawn_stt_cache_warm` (native glue,
/// TDD-exempt) are the only callers; this decision itself has no OS/Tauri
/// dependency, so it's independently unit-tested without a whisper model or
/// a live `AppState`. Its only production callers (`build_stt`,
/// `spawn_stt_cache_warm`) are behind `--features whisper`, so the default
/// build's non-test compile never calls it — `allow(dead_code)` there is
/// deliberate (mirrors `models.rs`'s own module-level allowance for a
/// similar not-yet-wired-in-this-build situation), not a sign it's unused
/// dead logic; the tests above exercise it in every build.
#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
fn should_reuse_cached_stt(
    cached: Option<&settings::ModelPreset>,
    wanted: &settings::ModelPreset,
) -> bool {
    cached == Some(wanted)
}

/// Persist exactly one history row for a completed dictation (AC-29, issue
/// #198) — the pure, injectable decision `run_pipeline_in_background` calls
/// for every `Pipeline::run` that returns `Ok(outcome)`. Takes `store` and
/// `created_at_ms` as plain injected values (rather than reading them off
/// `AppState`/the real clock) so this is unit-testable via
/// `Store::open_in_memory()` without constructing an `AppState` (issue
/// #165's Windows-CI hard rule: no `AppState` literals in `#[cfg(test)]`
/// code).
///
/// **Placement rationale (issue #198, interaction with the #174/#175/#176
/// generation-id mechanism, PR #214):** the call site in
/// `run_pipeline_in_background` invokes this BEFORE checking
/// `generation_is_live` — `Pipeline::run`'s `Ok(outcome)` means the text was
/// already pasted/written by the time this runs, regardless of whether this
/// dictation's generation is still the live one. A stale generation drops
/// only UI-visible effects (event emits, pill state writes, settle spawns —
/// see `run_pipeline_in_background`'s own comments on that gate); it must
/// NOT drop the history row, or a dictation whose text was genuinely pasted
/// would silently be missing from history. Issue #202: `app_name` is now
/// threaded through from `context::detect_active_app_name`'s hotkey-press-
/// time detection (see `run_pipeline_in_background`'s call site) rather
/// than always `None` — still `None` whenever detection failed or on a
/// platform/session with no active window.
fn record_history_entry(
    store: &store::Store,
    created_at_ms: i64,
    outcome: &pipeline::Outcome,
    app_name: Option<&str>,
) -> rusqlite::Result<i64> {
    store.insert_history(
        created_at_ms,
        &outcome.raw_transcript,
        &outcome.cleaned_transcript,
        app_name,
    )
}

/// Copy a history entry's cleaned transcript to the clipboard (AC-30, issue
/// #198), routed through the existing `output::Clipboard`/
/// `ClipboardPayload` seam so the text is never handed to a caller as a bare
/// `String` that could end up in a log call — mirrors
/// `output::paste_via_clipboard_swap`'s own `payload.into_inner()` ->
/// `clipboard.set()` handoff, minus the save/restore dance (this is a
/// "copy", not the dictation cursor-paste path). Pure/injectable — takes
/// `store` and `clipboard` directly rather than reading `AppState`, so it's
/// unit-testable via `Store::open_in_memory()` + a fake `Clipboard` without
/// constructing an `AppState`.
///
/// The error path (`id` not found) never reads or touches the clipboard at
/// all, so a copy of a since-deleted entry can't leave stale/wrong text on
/// the clipboard.
fn copy_history_entry_text(
    store: &store::Store,
    clipboard: &impl output::Clipboard,
    id: i64,
) -> Result<(), String> {
    let row = store
        .get_history(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no history entry with id {id}"))?;
    let payload = output::ClipboardPayload::new(row.cleaned);
    clipboard
        .set(&payload.into_inner())
        .map_err(|e| e.to_string())
}

/// Prune history rows older than the retention cutoff (AC-31, issue #198):
/// computes the cutoff via `store::retention_cutoff_ms` and calls
/// `Store::prune_history` only when it returns `Some` (i.e.
/// `retention_days > 0`) — `retention_days == 0` ("keep forever") is a
/// deliberate no-op, never touching a row, rather than treating `None` as
/// "prune from the epoch". Pure/injectable — takes `store` and `now_ms`
/// directly (rather than reading `AppState`/the real clock) so it's
/// unit-testable via `Store::open_in_memory()` without constructing an
/// `AppState`. Called from both the startup path (`run`'s `.setup()`) and
/// `commands::set_settings` whenever `retention_days` is in effect, so a
/// freshly-lowered retention window takes effect on the next save, not only
/// after a restart.
fn prune_history_for_retention(
    store: &store::Store,
    now_ms: i64,
    retention_days: u32,
) -> rusqlite::Result<usize> {
    // Issue #219: the newest recorded row's timestamp feeds
    // `retention_cutoff_ms`'s clock-skew guard (clamp/skip semantics) so a
    // backwards clock jump can't compute a cutoff that mass-deletes
    // history.
    let newest_row_ms = store.newest_history_timestamp()?;
    match store::retention_cutoff_ms(now_ms, retention_days, newest_row_ms) {
        Some(cutoff_ms) => store.prune_history(cutoff_ms),
        None => Ok(0),
    }
}

/// Reads the user's personal dictionary as the plain `Vec<String>` both
/// `stt::TranscribeOpts::dictionary` and `cleanup::OllamaCleanup::with_dictionary`
/// expect (issue #200, PRD AC-21) — `Store::list_terms`'s own
/// most-recently-added-first order (the issue #70 tie-break policy) passes
/// straight through unchanged. Pure/injectable — takes `store` directly
/// rather than reading `AppState`, so it's unit-testable via
/// `Store::open_in_memory()` without constructing an `AppState`. Called
/// from `run_pipeline_in_background` on every dictation.
fn dictionary_terms_for_pipeline(store: &store::Store) -> rusqlite::Result<Vec<String>> {
    Ok(store
        .list_terms()?
        .into_iter()
        .map(|term| term.term)
        .collect())
}

/// Current wall-clock time in milliseconds since the Unix epoch — the one
/// place `record_history_entry`/`prune_history_for_retention`'s real
/// call sites read the system clock (mirrors `real_clock`'s module-doc
/// convention just below: OS-glue callers inject a plain value into pure
/// functions rather than those functions reading the clock themselves).
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod apply_settings_tests {
    //! Issue #126 / PR #134 Sentinel 🔴-3: `commands::set_settings` must
    //! apply a changed `recording_mode` to the LIVE hotkeys state machine —
    //! before this, the machine was built once at startup and a Hold↔Toggle
    //! save only took effect after an app restart while the UI said "Saved".
    //! `apply_settings` is the (AppHandle- and AppState-free) function
    //! `apply_settings_to_state` delegates to, so the machine's post-save
    //! mode is assertable here without a live Tauri app.
    //!
    //! Issue #165: these tests exercise `apply_settings` directly against
    //! three bare `Mutex`es rather than building a full `AppState` — doing
    //! the latter reproducibly crashed the Windows lib test binary at
    //! process startup (`STATUS_ENTRYPOINT_NOT_FOUND`, before any test ran)
    //! despite never populating or reading any of `AppState`'s
    //! Tauri-runtime-typed fields. See `apply_settings`'s doc comment for
    //! the bisection that pinned it to this test module's `AppState`
    //! construction.

    use super::*;

    /// The three `Mutex`es `apply_settings` reads/writes, seeded from
    /// `settings` (mirrors how `AppState`'s equivalent fields are seeded at
    /// startup) — no `AppState`, tray item, or capture session involved.
    fn test_slots(
        settings: &settings::Settings,
    ) -> (
        Mutex<hotkeys::StateMachine>,
        Mutex<tray::OutputModeSwitch>,
        Mutex<settings::Settings>,
    ) {
        (
            Mutex::new(hotkeys::StateMachine::new(
                to_hotkey_mode(settings.recording_mode),
                [0u32],
                hotkeys::DEFAULT_DEBOUNCE,
            )),
            Mutex::new(tray::OutputModeSwitch::new(to_tray_output_mode(
                settings.output_mode,
            ))),
            Mutex::new(settings.clone()),
        )
    }

    #[test]
    fn apply_settings_flips_the_live_hotkey_machine_mode_issue_126() {
        // Default settings are Hold.
        let (hotkeys, output_switch, settings_slot) = test_slots(&settings::Settings::default());
        let new = settings::Settings {
            recording_mode: settings::RecordingMode::Toggle,
            ..settings::Settings::default()
        };

        let transition = apply_settings(&hotkeys, &output_switch, &settings_slot, new.clone());

        assert_eq!(transition, None, "idle machine: nothing to cancel");
        assert_eq!(
            hotkeys.lock().unwrap().mode(),
            hotkeys::Mode::Toggle,
            "the LIVE machine must run the just-saved mode, not wait for a restart"
        );
        assert_eq!(*settings_slot.lock().unwrap(), new);
    }

    #[test]
    fn apply_settings_cancels_an_in_flight_session_on_a_mode_change_issue_126() {
        let (hotkeys, output_switch, settings_slot) = test_slots(&settings::Settings::default());
        // Drive the live machine into a hold-in-progress first.
        let started = hotkeys
            .lock()
            .unwrap()
            .handle(hotkeys::KeyEvent::KeyDown(0, std::time::Duration::ZERO));
        assert_eq!(started, Some(hotkeys::Transition::StartRecording));

        let new = settings::Settings {
            recording_mode: settings::RecordingMode::Toggle,
            ..settings::Settings::default()
        };
        let transition = apply_settings(&hotkeys, &output_switch, &settings_slot, new);

        // The caller (set_settings) hands this to react_to_transition, which
        // stops capture and discards the buffered audio.
        assert_eq!(transition, Some(hotkeys::Transition::Cancelled));
        assert_eq!(hotkeys.lock().unwrap().mode(), hotkeys::Mode::Toggle);
    }

    #[test]
    fn apply_settings_with_an_unchanged_mode_leaves_a_session_in_flight() {
        let (hotkeys, output_switch, settings_slot) = test_slots(&settings::Settings::default());
        let started = hotkeys
            .lock()
            .unwrap()
            .handle(hotkeys::KeyEvent::KeyDown(0, std::time::Duration::ZERO));
        assert_eq!(started, Some(hotkeys::Transition::StartRecording));

        // Same recording_mode, different model preset — a dictation in
        // flight must NOT be cancelled by an unrelated settings save.
        let new = settings::Settings {
            model_preset: settings::ModelPreset::Small,
            ..settings::Settings::default()
        };
        let transition = apply_settings(&hotkeys, &output_switch, &settings_slot, new);

        assert_eq!(transition, None);
        assert_eq!(hotkeys.lock().unwrap().mode(), hotkeys::Mode::Hold);
    }

    #[test]
    fn apply_settings_updates_the_live_output_switch() {
        let (hotkeys, output_switch, settings_slot) = test_slots(&settings::Settings::default()); // Cursor
        let new = settings::Settings {
            output_mode: settings::OutputModeSetting::File,
            ..settings::Settings::default()
        };

        apply_settings(&hotkeys, &output_switch, &settings_slot, new);

        assert_eq!(
            output_switch.lock().unwrap().route_target(),
            tray::OutputMode::File
        );
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::*;

    #[test]
    fn should_reuse_cached_stt_reuses_when_the_cached_preset_matches_issue_115() {
        assert!(should_reuse_cached_stt(
            Some(&settings::ModelPreset::LargeV3Turbo),
            &settings::ModelPreset::LargeV3Turbo
        ));
        assert!(should_reuse_cached_stt(
            Some(&settings::ModelPreset::Small),
            &settings::ModelPreset::Small
        ));
    }

    #[test]
    fn should_reuse_cached_stt_rebuilds_when_the_selected_preset_differs_issue_115() {
        assert!(!should_reuse_cached_stt(
            Some(&settings::ModelPreset::LargeV3Turbo),
            &settings::ModelPreset::Small
        ));
        assert!(!should_reuse_cached_stt(
            Some(&settings::ModelPreset::Small),
            &settings::ModelPreset::LargeV3Turbo
        ));
    }

    #[test]
    fn should_reuse_cached_stt_rebuilds_when_the_cache_is_empty_issue_115() {
        assert!(!should_reuse_cached_stt(
            None,
            &settings::ModelPreset::LargeV3Turbo
        ));
        assert!(!should_reuse_cached_stt(
            None,
            &settings::ModelPreset::Small
        ));
    }

    #[test]
    fn output_mode_toggle_label_names_the_mode_it_would_switch_to() {
        assert_eq!(
            output_mode_toggle_label(tray::OutputMode::CursorPaste),
            "Switch to File output"
        );
        assert_eq!(
            output_mode_toggle_label(tray::OutputMode::File),
            "Switch to Cursor output"
        );
    }

    #[test]
    fn hotkey_mode_mapping_round_trips_every_variant() {
        assert_eq!(
            to_hotkey_mode(settings::RecordingMode::Hold),
            hotkeys::Mode::Hold
        );
        assert_eq!(
            to_hotkey_mode(settings::RecordingMode::Toggle),
            hotkeys::Mode::Toggle
        );
    }

    #[test]
    fn output_mode_mapping_round_trips_every_variant() {
        assert_eq!(
            to_tray_output_mode(settings::OutputModeSetting::Cursor),
            tray::OutputMode::CursorPaste
        );
        assert_eq!(
            to_tray_output_mode(settings::OutputModeSetting::File),
            tray::OutputMode::File
        );
    }

    #[test]
    fn model_preset_mapping_round_trips_every_variant() {
        assert_eq!(
            to_models_preset(settings::ModelPreset::LargeV3Turbo),
            models::ModelPreset::LargeV3TurboQ5
        );
        assert_eq!(
            to_models_preset(settings::ModelPreset::Small),
            models::ModelPreset::Small
        );
    }

    #[test]
    fn spec_for_preset_resolves_every_variant_without_panicking() {
        for preset in models::ModelPreset::ALL {
            let spec = spec_for_preset(preset);
            assert_eq!(spec.preset, preset);
        }
    }

    // Issue #184: `commands::model_registry`'s pure data source — every
    // preset's settings-safe id plus its exact download size, so the
    // frontend model picker can render "Small — 488 MB" without duplicating
    // `models::ModelSpec.size_bytes` anywhere.
    #[test]
    fn model_registry_entries_covers_every_preset_with_its_exact_size() {
        let entries = model_registry_entries();
        assert_eq!(entries.len(), 2);

        let large = entries
            .iter()
            .find(|e| e.preset == settings::ModelPreset::LargeV3Turbo)
            .expect("LargeV3Turbo entry present");
        assert_eq!(large.size_bytes, 574_041_195);

        let small = entries
            .iter()
            .find(|e| e.preset == settings::ModelPreset::Small)
            .expect("Small entry present");
        assert_eq!(small.size_bytes, 487_601_967);
    }

    #[test]
    fn settings_preset_mapping_round_trips_every_variant() {
        assert_eq!(
            to_settings_preset(models::ModelPreset::LargeV3TurboQ5),
            settings::ModelPreset::LargeV3Turbo
        );
        assert_eq!(
            to_settings_preset(models::ModelPreset::Small),
            settings::ModelPreset::Small
        );
    }

    // PR #185 Sentinel 🟡-4: suspend_hotkey/resume_hotkey are in the global
    // invoke_handler, so any window's webview can call them. Only the
    // settings window is allowed to suspend/resume the recording trigger —
    // this pure predicate is the gate each command checks against
    // `window.label()`.
    #[test]
    fn is_settings_window_only_accepts_the_settings_label() {
        assert!(is_settings_window(SETTINGS_WINDOW_LABEL));
        assert!(!is_settings_window("main"));
        assert!(!is_settings_window("pill"));
        assert!(!is_settings_window(""));
    }

    // PR #185 Sentinel 🔴-1(iii): a monotonic generation token makes
    // suspend/resume idempotent under out-of-order IPC. A resume only
    // restores the hotkey when its generation is still the latest suspend's
    // — a stale resume (superseded by a newer suspend) or the zero sentinel
    // (no suspend active) must be a no-op so it can't clobber a live capture.
    #[test]
    fn should_resume_hotkey_only_when_the_generation_is_the_current_suspend() {
        assert!(should_resume_hotkey(5, 5));
        // A stale resume from a capture superseded by a newer suspend.
        assert!(!should_resume_hotkey(6, 5));
        // The zero sentinel means "not suspended" — never resume.
        assert!(!should_resume_hotkey(0, 0));
        assert!(!should_resume_hotkey(5, 0));
    }

    // PR #185 cycle-6 🟡: the register-before-persist-with-rollback control
    // flow of `commands::set_settings`, extracted as a pure, injectable seam
    // (register/persist/rollback closures) so the three failure/success paths
    // are unit-testable without an AppState/Wry runtime (#165).
    #[test]
    fn set_settings_with_rollback_success_registers_then_persists_no_rollback() {
        let mut registers: Vec<String> = vec![];
        let mut persists = 0;
        let mut rollbacks: Vec<String> = vec![];
        let result = set_settings_with_rollback(
            true,
            "Old",
            "New",
            |h| {
                registers.push(h.to_string());
                Ok(())
            },
            || {
                persists += 1;
                Ok(())
            },
            |h| rollbacks.push(h.to_string()),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(registers, vec!["New".to_string()]);
        assert_eq!(persists, 1);
        assert!(rollbacks.is_empty());
    }

    #[test]
    fn set_settings_with_rollback_register_failure_restores_prior_and_never_persists() {
        let mut persists = 0;
        let mut rollbacks: Vec<String> = vec![];
        let result = set_settings_with_rollback(
            true,
            "Old",
            "New",
            |_h| Err("register failed".to_string()),
            || {
                persists += 1;
                Ok(())
            },
            |h| rollbacks.push(h.to_string()),
        );
        assert_eq!(result, Err("register failed".to_string()));
        assert_eq!(persists, 0);
        assert_eq!(rollbacks, vec!["Old".to_string()]);
    }

    #[test]
    fn set_settings_with_rollback_persist_failure_after_register_restores_prior() {
        let mut registers: Vec<String> = vec![];
        let mut rollbacks: Vec<String> = vec![];
        let result = set_settings_with_rollback(
            true,
            "Old",
            "New",
            |h| {
                registers.push(h.to_string());
                Ok(())
            },
            || Err("disk full".to_string()),
            |h| rollbacks.push(h.to_string()),
        );
        assert_eq!(result, Err("disk full".to_string()));
        assert_eq!(registers, vec!["New".to_string()]);
        assert_eq!(rollbacks, vec!["Old".to_string()]);
    }

    #[test]
    fn set_settings_with_rollback_unchanged_hotkey_only_persists() {
        let mut registers: Vec<String> = vec![];
        let mut persists = 0;
        let mut rollbacks: Vec<String> = vec![];
        let result = set_settings_with_rollback(
            false,
            "Old",
            "Old",
            |h| {
                registers.push(h.to_string());
                Ok(())
            },
            || {
                persists += 1;
                Ok(())
            },
            |h| rollbacks.push(h.to_string()),
        );
        assert_eq!(result, Ok(()));
        assert!(registers.is_empty());
        assert_eq!(persists, 1);
        assert!(rollbacks.is_empty());
    }
}

/// Issue #198 tests for the AppState-free history-capture pure functions
/// (AC-29/AC-30/AC-31): `record_history_entry`, `copy_history_entry_text`,
/// `prune_history_for_retention`. Mirrors `apply_settings_tests`'s pattern
/// (issue #165) — real `store::Store::open_in_memory()` (no fake needed,
/// same as `store.rs`'s own tests) plus a local fake `output::Clipboard`,
/// never a constructed `AppState`.
#[cfg(test)]
mod history_wiring_tests {
    use super::*;
    use crate::output::Clipboard as _;
    use std::cell::RefCell;

    /// Fake clipboard for `copy_history_entry_text` tests — mirrors
    /// `output.rs`'s own private `FakeClipboard` (not reachable from here:
    /// it's under `output`'s `#[cfg(test)] mod tests`), an in-memory cell
    /// with no real OS clipboard access.
    struct FakeClipboard {
        contents: RefCell<String>,
    }

    impl FakeClipboard {
        fn new(initial: &str) -> Self {
            Self {
                contents: RefCell::new(initial.to_string()),
            }
        }
    }

    impl output::Clipboard for FakeClipboard {
        fn get(&self) -> std::io::Result<String> {
            Ok(self.contents.borrow().clone())
        }

        fn set(&self, contents: &str) -> std::io::Result<()> {
            *self.contents.borrow_mut() = contents.to_string();
            Ok(())
        }
    }

    fn synthetic_outcome(raw: &str, cleaned: &str) -> pipeline::Outcome {
        pipeline::Outcome {
            raw_transcript: raw.to_string(),
            cleaned_transcript: cleaned.to_string(),
            cleanup_fell_back: false,
            output: output::OutputOutcome::Pasted,
        }
    }

    // -------------------------------------------------------------
    // AC-29: record_history_entry — exactly one Store::insert_history call
    // per completed pipeline run, carrying raw + cleaned transcript.
    // -------------------------------------------------------------

    #[test]
    fn record_history_entry_persists_the_outcomes_raw_and_cleaned_transcript_ac29() {
        let store = store::Store::open_in_memory().unwrap();
        let outcome = synthetic_outcome("raw synthetic dictation", "Cleaned synthetic dictation.");

        let id = record_history_entry(&store, 1_000, &outcome, None).unwrap();

        let rows = store.search_history("synthetic", 10).unwrap();
        assert_eq!(rows.len(), 1, "exactly one row must be inserted");
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].created_at_ms, 1_000);
        assert_eq!(rows[0].raw, "raw synthetic dictation");
        assert_eq!(rows[0].cleaned, "Cleaned synthetic dictation.");
    }

    #[test]
    fn record_history_entry_carries_the_app_name_when_given_ac29() {
        let store = store::Store::open_in_memory().unwrap();
        let outcome = synthetic_outcome("raw", "cleaned");

        record_history_entry(&store, 1_000, &outcome, Some("Notes")).unwrap();

        let rows = store.search_history("raw", 10).unwrap();
        assert_eq!(rows[0].app_name.as_deref(), Some("Notes"));
    }

    #[test]
    fn record_history_entry_called_once_per_outcome_never_double_inserts_ac29() {
        let store = store::Store::open_in_memory().unwrap();
        let outcome = synthetic_outcome("raw once", "cleaned once");

        record_history_entry(&store, 1_000, &outcome, None).unwrap();

        let rows = store.search_history("once", 10).unwrap();
        assert_eq!(
            rows.len(),
            1,
            "a single completed run must produce exactly one history row"
        );
    }

    // -------------------------------------------------------------
    // AC-30: copy_history_entry_text — routes the entry's cleaned text
    // through the Clipboard/ClipboardPayload seam, never a bare String that
    // could be logged.
    // -------------------------------------------------------------

    #[test]
    fn copy_history_entry_text_sets_the_clipboard_to_the_entrys_cleaned_text_ac30() {
        let store = store::Store::open_in_memory().unwrap();
        let id = store
            .insert_history(1_000, "raw synthetic", "Cleaned synthetic.", None)
            .unwrap();
        let clipboard = FakeClipboard::new("");

        copy_history_entry_text(&store, &clipboard, id).unwrap();

        assert_eq!(clipboard.get().unwrap(), "Cleaned synthetic.");
    }

    #[test]
    fn copy_history_entry_text_errors_for_an_unknown_id_without_touching_the_clipboard_ac30() {
        let store = store::Store::open_in_memory().unwrap();
        let clipboard = FakeClipboard::new("untouched");

        let result = copy_history_entry_text(&store, &clipboard, 999);

        assert!(result.is_err());
        assert_eq!(clipboard.get().unwrap(), "untouched");
    }

    // -------------------------------------------------------------
    // AC-31: prune_history_for_retention — computes the cutoff and prunes
    // only when retention_days > 0; a no-op (never touches rows) at 0.
    // -------------------------------------------------------------

    #[test]
    fn prune_history_for_retention_prunes_rows_older_than_the_cutoff_ac31() {
        let store = store::Store::open_in_memory().unwrap();
        let day_ms: i64 = 24 * 60 * 60 * 1000;
        let now_ms = 10 * day_ms;
        store
            .insert_history(day_ms, "too old", "too old.", None)
            .unwrap();
        store
            .insert_history(9 * day_ms, "recent enough", "recent enough.", None)
            .unwrap();

        let deleted = prune_history_for_retention(&store, now_ms, 3).unwrap();

        assert_eq!(deleted, 1);
        let remaining = store.search_history("", 10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].raw, "recent enough");
    }

    #[test]
    fn prune_history_for_retention_is_a_no_op_when_retention_days_is_zero_ac31() {
        let store = store::Store::open_in_memory().unwrap();
        store
            .insert_history(0, "keep forever", "keep forever.", None)
            .unwrap();

        let deleted = prune_history_for_retention(&store, 999_999_999_999, 0).unwrap();

        assert_eq!(deleted, 0);
        assert_eq!(store.search_history("", 10).unwrap().len(), 1);
    }
}

/// AC-35 (PRD AC-21, issue #200): dictionary terms actually flow from the
/// `Store` into the pipeline's `Stt::transcribe` call — the first thing to
/// populate `TranscribeOpts.dictionary`/`OllamaCleanup`'s dictionary, both
/// of which existed as an empty seam before this PR. Real whisper-gated
/// recognition-accuracy coverage (does adding a term actually change what a
/// real model transcribes) is `#[ignore]`d in `stt.rs` per that module's
/// existing pattern (no model file in CI); this module proves the plumbing
/// itself — never `#[ignore]`d — using `Store::open_in_memory()` and a spy
/// `Stt` double, never a constructed `AppState` (Windows-CI hard rule).
#[cfg(test)]
mod dictionary_wiring_tests {
    use super::*;
    use crate::output::{Clipboard, PasteSynthesizer};
    use std::cell::RefCell;
    use std::io;
    use std::time::Duration;

    /// No-op `Clipboard`/`PasteSynthesizer` — these tests only drive the
    /// file output target, so neither is ever actually exercised, but
    /// `Pipeline` needs concrete types to construct (mirrors
    /// `tests/acceptance.rs`'s `NoopClipboard`/`NoopPaste`).
    struct NoopClipboard;
    impl Clipboard for NoopClipboard {
        fn get(&self) -> io::Result<String> {
            Ok(String::new())
        }
        fn set(&self, _contents: &str) -> io::Result<()> {
            Ok(())
        }
    }
    struct NoopPaste;
    impl PasteSynthesizer for NoopPaste {
        fn synthesize_paste(&self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Spy `Stt`: records the `TranscribeOpts` it was actually called with
    /// and returns a canned transcript. Unlike `stt::FakeStt` (which
    /// ignores `opts` entirely and so can't discriminate "dictionary
    /// attached" from "not"), this lets a test assert on exactly what
    /// reached the transcription call.
    struct SpyStt {
        captured: RefCell<Option<stt::TranscribeOpts>>,
    }
    impl SpyStt {
        fn new() -> Self {
            Self {
                captured: RefCell::new(None),
            }
        }
    }
    impl stt::Stt for SpyStt {
        fn transcribe(
            &self,
            _samples: &[f32],
            opts: &stt::TranscribeOpts,
        ) -> Result<String, stt::SttError> {
            *self.captured.borrow_mut() = Some(opts.clone());
            Ok("canned transcript".to_string())
        }
    }

    /// Mirrors `stt.rs`'s own `impl Stt for Arc<WhisperStt>` (issue #115):
    /// lets a shared, `Rc`-wrapped `SpyStt` satisfy `Pipeline`'s `S: Stt`
    /// bound while the test keeps its own handle to inspect `captured`
    /// after `Pipeline::run` returns (a plain owned `SpyStt` would be moved
    /// into the pipeline and become unreachable afterward).
    impl stt::Stt for std::rc::Rc<SpyStt> {
        fn transcribe(
            &self,
            samples: &[f32],
            opts: &stt::TranscribeOpts,
        ) -> Result<String, stt::SttError> {
            self.as_ref().transcribe(samples, opts)
        }
    }

    fn fixed_clock() -> output::Clock {
        output::Clock {
            year: 2026,
            month: 7,
            day: 15,
            hour: 9,
            minute: 0,
        }
    }

    fn file_output_mode(dir: &tempfile::TempDir) -> output::OutputMode {
        output::OutputMode::File {
            base_dir: dir.path().to_path_buf(),
            config: output::FileConfig {
                path_template: "dictation.md".to_string(),
                timestamp_prefix_template: None,
            },
        }
    }

    #[test]
    fn dictionary_terms_for_pipeline_reads_store_terms_newest_first_ac35() {
        let store = store::Store::open_in_memory().unwrap();
        store.add_term("oldest", 1_000).unwrap();
        store.add_term("newest", 2_000).unwrap();

        let terms = dictionary_terms_for_pipeline(&store).unwrap();
        assert_eq!(terms, vec!["newest".to_string(), "oldest".to_string()]);
    }

    #[test]
    fn dictionary_terms_for_pipeline_is_empty_when_the_dictionary_is_empty_ac35() {
        let store = store::Store::open_in_memory().unwrap();
        assert_eq!(
            dictionary_terms_for_pipeline(&store).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_dictionary_term_reaches_stts_transcribe_call_through_transcribe_opts_ac35() {
        // The core AC-35 plumbing assertion, never #[ignore]d: a term added
        // to the Store shows up in the TranscribeOpts the pipeline actually
        // hands to Stt::transcribe.
        let store = store::Store::open_in_memory().unwrap();
        store.add_term("Kubernetes", 1_000).unwrap();
        let dictionary = dictionary_terms_for_pipeline(&store).unwrap();

        let spy = std::rc::Rc::new(SpyStt::new());
        let dir = tempfile::tempdir().unwrap();
        let opts = pipeline::PipelineOpts {
            transcribe: stt::TranscribeOpts {
                dictionary: dictionary.clone(),
            },
            tone: cleanup::Tone::Neutral,
            output_mode: file_output_mode(&dir),
            clock: fixed_clock(),
            restore_delay: Duration::from_millis(0),
        };
        let pipeline = pipeline::Pipeline::new(
            std::rc::Rc::clone(&spy),
            cleanup::RegexCleanup,
            NoopClipboard,
            NoopPaste,
            |_: Duration| {},
        );

        pipeline.run(&[0.0_f32; 16_000], &opts).unwrap();

        let captured = spy
            .captured
            .borrow()
            .clone()
            .expect("Stt::transcribe must have been called");
        assert_eq!(captured.dictionary, dictionary);
        assert!(captured.initial_prompt().contains("Kubernetes"));
    }

    #[test]
    fn pipeline_output_differs_between_no_dictionary_and_a_populated_dictionary_ac35() {
        // AC-35: "comparing pipeline output with and without dictionary
        // injection on the same fixture". FakeStt/SpyStt return a fixed
        // canned transcript regardless of opts (a real model is what
        // actually changes recognition — see stt.rs's #[ignore]d
        // whisper-gated test for that), so "pipeline output" here is the
        // rendered `initial_prompt` the transcription call receives, which
        // is the one thing this pure/injected pipeline CAN observe
        // changing as a direct, non-ignored proof the seam is wired.
        let store = store::Store::open_in_memory().unwrap();

        let empty_dictionary = dictionary_terms_for_pipeline(&store).unwrap();
        store.add_term("Kubernetes", 1_000).unwrap();
        let populated_dictionary = dictionary_terms_for_pipeline(&store).unwrap();

        let run_with = |dictionary: Vec<String>| -> String {
            let spy = std::rc::Rc::new(SpyStt::new());
            let dir = tempfile::tempdir().unwrap();
            let opts = pipeline::PipelineOpts {
                transcribe: stt::TranscribeOpts { dictionary },
                tone: cleanup::Tone::Neutral,
                output_mode: file_output_mode(&dir),
                clock: fixed_clock(),
                restore_delay: Duration::from_millis(0),
            };
            let pipeline = pipeline::Pipeline::new(
                std::rc::Rc::clone(&spy),
                cleanup::RegexCleanup,
                NoopClipboard,
                NoopPaste,
                |_: Duration| {},
            );
            pipeline.run(&[0.0_f32; 16_000], &opts).unwrap();
            let prompt = spy
                .captured
                .borrow()
                .clone()
                .expect("Stt::transcribe must have been called")
                .initial_prompt();
            prompt
        };

        let without_dictionary = run_with(empty_dictionary);
        let with_dictionary = run_with(populated_dictionary);

        assert_eq!(without_dictionary, "");
        assert_eq!(with_dictionary, "Kubernetes");
        assert_ne!(without_dictionary, with_dictionary);
    }
}

/// Issue #202 (PRD AC-22, M3 per-app tone): wiring-level proof that once
/// tone dispatch is wired through `run_pipeline_in_background`,
/// `Tone::Verbatim` still bypasses `OllamaCleanup`'s transport entirely
/// end-to-end (AC-42) — mirrors `cleanup.rs`'s own
/// `ollama_cleanup_verbatim_tone_bypasses_the_transport_entirely`, but at
/// THIS layer (through a real `pipeline::Pipeline`, not just a bare
/// `OllamaCleanup::clean` call), per AC-42's explicit "not just within
/// cleanup.rs" requirement. Never a constructed `AppState` (Windows-CI hard
/// rule, issue #165) — pure/injected collaborators only, mirroring
/// `dictionary_wiring_tests` right above.
#[cfg(test)]
mod tone_wiring_tests {
    use super::*;
    use crate::output::{Clipboard, PasteSynthesizer};
    use std::cell::Cell;
    use std::io;
    use std::time::Duration;

    struct NoopClipboard;
    impl Clipboard for NoopClipboard {
        fn get(&self) -> io::Result<String> {
            Ok(String::new())
        }
        fn set(&self, _contents: &str) -> io::Result<()> {
            Ok(())
        }
    }
    struct NoopPaste;
    impl PasteSynthesizer for NoopPaste {
        fn synthesize_paste(&self) -> io::Result<()> {
            Ok(())
        }
    }

    fn fixed_clock() -> output::Clock {
        output::Clock {
            year: 2026,
            month: 7,
            day: 15,
            hour: 9,
            minute: 0,
        }
    }

    fn file_output_mode(dir: &tempfile::TempDir) -> output::OutputMode {
        output::OutputMode::File {
            base_dir: dir.path().to_path_buf(),
            config: output::FileConfig {
                path_template: "dictation.md".to_string(),
                timestamp_prefix_template: None,
            },
        }
    }

    /// An `OllamaTransport` that counts how many times it was called and
    /// always fails — if `Tone::Verbatim` ever reached it, the pipeline
    /// would either surface `cleanup_fell_back` (wrong: Verbatim must never
    /// even ask) or the call count would be nonzero either way.
    struct CountingTransport {
        calls: Cell<u32>,
    }
    impl CountingTransport {
        fn new() -> Self {
            Self {
                calls: Cell::new(0),
            }
        }
    }
    impl cleanup::OllamaTransport for CountingTransport {
        fn post(&self, _url: &str, _body: &str) -> Result<String, cleanup::TransportError> {
            self.calls.set(self.calls.get() + 1);
            Err(cleanup::TransportError::ConnectionFailed)
        }
    }
    /// Mirrors `dictionary_wiring_tests::SpyStt`'s `Rc`-sharing pattern: the
    /// test keeps its own handle to inspect `calls` after `Pipeline::run`
    /// moves an owned copy into the pipeline.
    impl cleanup::OllamaTransport for std::rc::Rc<CountingTransport> {
        fn post(&self, url: &str, body: &str) -> Result<String, cleanup::TransportError> {
            self.as_ref().post(url, body)
        }
    }

    #[test]
    fn verbatim_tone_bypasses_ollama_transport_end_to_end_through_the_pipeline_ac42() {
        let transport = std::rc::Rc::new(CountingTransport::new());
        let cleanup = cleanup::OllamaCleanup::new(
            "http://localhost:11434",
            "llama3",
            std::rc::Rc::clone(&transport),
        );
        let dir = tempfile::tempdir().unwrap();
        let opts = pipeline::PipelineOpts {
            transcribe: stt::TranscribeOpts { dictionary: vec![] },
            tone: cleanup::Tone::Verbatim,
            output_mode: file_output_mode(&dir),
            clock: fixed_clock(),
            restore_delay: Duration::from_millis(0),
        };
        let raw = "  um, hello   world, uh, messy";
        let pipeline = pipeline::Pipeline::new(
            stt::FakeStt::new(raw),
            cleanup,
            NoopClipboard,
            NoopPaste,
            |_: Duration| {},
        );

        let outcome = pipeline.run(&[0.0_f32; 16_000], &opts).unwrap();

        assert_eq!(
            transport.calls.get(),
            0,
            "Verbatim must never reach the OllamaTransport, even through the full pipeline"
        );
        assert_eq!(
            outcome.cleaned_transcript, outcome.raw_transcript,
            "Verbatim must return the raw transcript essentially untouched"
        );
        assert!(
            !outcome.cleanup_fell_back,
            "Verbatim bypasses cleanup entirely — it never even tries the transport, so there \
             is nothing to fall back FROM"
        );
    }

    #[test]
    fn casual_tone_does_reach_the_ollama_transport_end_to_end_through_the_pipeline_ac42() {
        // Contrast case: unlike Verbatim, Casual is an LLM-rewritten tone
        // and MUST reach the transport — proves the wiring-level test above
        // is actually discriminating (a pipeline that never calls the
        // transport for ANY tone would pass the Verbatim assertion for the
        // wrong reason).
        let transport = std::rc::Rc::new(CountingTransport::new());
        let cleanup = cleanup::OllamaCleanup::new(
            "http://localhost:11434",
            "llama3",
            std::rc::Rc::clone(&transport),
        );
        let dir = tempfile::tempdir().unwrap();
        let opts = pipeline::PipelineOpts {
            transcribe: stt::TranscribeOpts { dictionary: vec![] },
            tone: cleanup::Tone::Casual,
            output_mode: file_output_mode(&dir),
            clock: fixed_clock(),
            restore_delay: Duration::from_millis(0),
        };
        let pipeline = pipeline::Pipeline::new(
            stt::FakeStt::default(),
            cleanup,
            NoopClipboard,
            NoopPaste,
            |_: Duration| {},
        );

        let outcome = pipeline.run(&[0.0_f32; 16_000], &opts).unwrap();

        assert_eq!(transport.calls.get(), 1, "Casual must reach the transport");
        // The stub transport always fails, so AC-4's fallback fires — that
        // is expected here (this test isn't about a successful response).
        assert!(outcome.cleanup_fell_back);
    }
}

/// Loads persisted settings from the `tauri-plugin-store`-backed
/// `settings.json`, translating a missing store/key to
/// [`settings::SettingsLoadError::NotFound`] and a present-but-unparsable
/// value to [`settings::SettingsLoadError::Corrupt`] (issue #80) — the same
/// tri-state `settings::SettingsStore` establishes, adapted to the plugin's
/// `Result<Option<JsonValue>>` shape (thin OS glue; the parsing itself
/// delegates to `settings::from_json`'s already-tested logic via
/// `serde_json::from_value`).
fn load_settings_from_store(
    app: &tauri::AppHandle,
) -> Result<settings::Settings, settings::SettingsLoadError> {
    let store = app
        .store("settings.json")
        .map_err(|e| settings::SettingsLoadError::Corrupt(e.to_string()))?;
    match store.get("settings") {
        None => Err(settings::SettingsLoadError::NotFound),
        Some(value) => serde_json::from_value(value)
            .map_err(|e| settings::SettingsLoadError::Corrupt(e.to_string())),
    }
}

/// Persist `settings` to the `tauri-plugin-store`-backed `settings.json`.
fn save_settings_to_store(
    app: &tauri::AppHandle,
    settings: &settings::Settings,
) -> Result<(), String> {
    let store = app.store("settings.json").map_err(|e| e.to_string())?;
    let value = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    store.set("settings", value);
    store.save().map_err(|e| e.to_string())
}

/// Registers `hotkey` (a string like `"Control+Option+Space"`) as the
/// global shortcut driving the hotkeys state machine, unregistering
/// whatever was previously registered first so this is safe to call again
/// when the user changes the hotkey binding (`commands::set_settings`).
fn register_hotkey(
    app: &tauri::AppHandle,
    hotkey: &str,
) -> Result<(), tauri_plugin_global_shortcut::Error> {
    let global_shortcut = app.global_shortcut();
    let _ = global_shortcut.unregister_all();
    let handler_handle = app.clone();
    global_shortcut.on_shortcut(hotkey, move |_app, _shortcut, event| {
        let key_event = match event.state() {
            ShortcutState::Pressed => hotkeys::KeyEvent::KeyDown(0, monotonic_now()),
            ShortcutState::Released => hotkeys::KeyEvent::KeyUp(0, monotonic_now()),
        };
        handle_key_event(&handler_handle, key_event);
    })
}

/// Unregisters the global dictation hotkey without registering a new one
/// (issue #181, `commands::suspend_hotkey`) — called while the settings
/// window's hotkey-capture field is active so keypresses are captured for
/// rebinding instead of also triggering a dictation via the still-live
/// shortcut. [`register_hotkey`]/`commands::resume_hotkey` re-register when
/// capture ends.
fn unregister_hotkey(app: &tauri::AppHandle) -> Result<(), tauri_plugin_global_shortcut::Error> {
    app.global_shortcut().unregister_all()
}

/// Whether a window `label` is allowed to suspend/resume the global
/// dictation hotkey (PR #185 Sentinel 🟡-4). Both commands live in the
/// global `invoke_handler`, so without this gate any window's webview could
/// call an unpaired `suspend_hotkey` and DoS the recording trigger — only
/// the settings window (whose hotkey-capture field is the sole legitimate
/// caller) may. Pure and window-runtime-free so it's unit-testable.
fn is_settings_window(label: &str) -> bool {
    label == SETTINGS_WINDOW_LABEL
}

/// Whether a `resume_hotkey` carrying `requested_gen` should actually
/// re-register the shortcut, given the latest suspend's `current_gen` (PR
/// #185 Sentinel 🔴-1(iii)). A monotonic generation token makes suspend/
/// resume idempotent under out-of-order IPC: a resume only acts when its
/// generation is still the current suspend's, so a stale resume (its
/// suspend already superseded by a newer one) or the zero sentinel (no
/// suspend outstanding) is a no-op and can't clobber a live capture. Pure.
fn should_resume_hotkey(current_gen: u64, requested_gen: u64) -> bool {
    requested_gen != 0 && current_gen == requested_gen
}

/// The pure register-before-persist-with-rollback control flow of
/// `commands::set_settings` (PR #185 cycle-6 🟡 / #91). Extracted with the
/// three effects injected as closures so it's unit-testable without an
/// `AppState`/`Wry` runtime (#165):
/// - `register(new)` binds the new hotkey to the OS (only when `hotkey_changed`);
/// - `persist()` writes settings.json;
/// - `rollback(prior)` restores the previously-registered hotkey.
///
/// Ordering guarantees #91: the new chord is registered BEFORE persisting, so
/// a chord the OS won't bind fails without being written; and the OS binding
/// and settings.json can never disagree — a failure at EITHER step rolls the
/// OS back to `prior_hotkey` before returning `Err`.
pub(crate) fn set_settings_with_rollback(
    hotkey_changed: bool,
    prior_hotkey: &str,
    new_hotkey: &str,
    mut register: impl FnMut(&str) -> Result<(), String>,
    mut persist: impl FnMut() -> Result<(), String>,
    mut rollback: impl FnMut(&str),
) -> Result<(), String> {
    if hotkey_changed {
        if let Err(err) = register(new_hotkey) {
            // The new chord won't bind (register unregisters first, so the OS
            // is now unbound) — restore the prior binding and reject.
            rollback(prior_hotkey);
            return Err(err);
        }
    }
    if let Err(err) = persist() {
        // Persist failed AFTER a successful register — roll the OS binding
        // back to the prior hotkey so it matches the (unchanged) settings.json.
        if hotkey_changed {
            rollback(prior_hotkey);
        }
        return Err(err);
    }
    Ok(())
}

/// Backend safety net (PR #185 Sentinel 🔴-1(b)): force-restore the global
/// dictation hotkey if a capture suspend is still outstanding. The settings
/// window is *hidden* (not destroyed) on close, so React never unmounts and
/// a suspend from its hotkey-capture field would otherwise leave the global
/// shortcut dead until app restart. Called from the settings window's
/// close/hide handler. Idempotent — a no-op unless currently suspended
/// (generation != 0), and it clears the generation so any later stale
/// frontend resume is ignored by [`should_resume_hotkey`].
fn force_resume_hotkey(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let hotkey = state.settings.lock().unwrap().hotkey.clone();
    let mut gen_slot = state.hotkey_suspend_gen.lock().unwrap();
    if *gen_slot != 0 {
        if let Err(err) = register_hotkey(app, &hotkey) {
            eprintln!("bla: failed to restore global hotkey on settings-window close: {err}");
        }
        *gen_slot = 0;
    }
}

/// Monotonic timestamp for the hotkey state machine: an opaque duration
/// since process start, never the wall clock — mirrors
/// `hotkeys::Timestamp`'s contract (the machine only ever compares two of
/// these against its own configured debounce, never reads a real clock
/// itself).
fn monotonic_now() -> hotkeys::Timestamp {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed()
}

/// OS glue: feed one key event into the shared state machine and react to
/// whatever [`hotkeys::Transition`] it produces.
fn handle_key_event(app: &tauri::AppHandle, event: hotkeys::KeyEvent) {
    let state = app.state::<AppState>();
    let transition = state.hotkeys.lock().unwrap().handle(event);
    react_to_transition(app, transition);
}

/// Issue #44: called on window focus-loss to reconcile a possibly-dropped
/// `KeyUp` so the machine can never wedge in `Holding`.
fn reconcile_hotkeys_on_focus_loss(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let transition = state.hotkeys.lock().unwrap().reset();
    react_to_transition(app, transition);
}

/// React to a `hotkeys::Transition` by starting/stopping audio capture and,
/// on `StopRecording`, running the pipeline in the background.
fn react_to_transition(app: &tauri::AppHandle, transition: Option<hotkeys::Transition>) {
    let state = app.state::<AppState>();
    match transition {
        Some(hotkeys::Transition::StartRecording) => {
            // Issues #174/#175/#176: mint a new per-dictation generation id
            // FIRST, before anything else in this arm — so any earlier
            // dictation's still-in-flight background completion (see
            // `run_pipeline_in_background`/`generation_is_live`) is
            // immediately recognized as stale the moment this dictation
            // begins, rather than only once ITS OWN state write lands.
            state.dictation_generation.fetch_add(1, Ordering::SeqCst);
            // Issue #202: detect the active app HERE — at hotkey-press
            // time, before capture starts — not on StopRecording, since the
            // user may have already switched focus (e.g. to the recording
            // pill itself) by the time they release/re-tap the hotkey.
            // `detect_active_app_name` degrades to `None` silently on any
            // failure (no active window, permission denied); `None` simply
            // means this dictation resolves to `Tone::Neutral` below,
            // matching the "detection failure never surfaces an error"
            // contract.
            *state.active_app_name.lock().unwrap() = context::detect_active_app_name();
            // Drop any stale samples and any error recorded by a previous
            // session before starting a fresh capture window, so the
            // degraded-capture check on StopRecording reflects only THIS
            // session (Sentinel 🟡 #3).
            state.buffer.lock().unwrap().drain();
            state.diagnostics.clear_error();
            match audio::CaptureSession::start(
                state.buffer.clone(),
                state.diagnostics.clone(),
                state.level_meter.clone(),
            ) {
                Ok(session) => {
                    *state.capture.lock().unwrap() = Some(session);
                    // Issue #126 (M2 PR 2.2): drive the throttled
                    // `audio-level` event stream for exactly this session's
                    // lifetime -- the poller exits on its own once
                    // StopRecording/Cancelled signals `stop_rx` below.
                    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
                    *state.level_poll_stop.lock().unwrap() = Some(stop_tx);
                    spawn_level_poller(app.clone(), state.level_meter.clone(), stop_rx);
                    set_pipeline_state(app, tray::PipelineState::Recording);
                }
                Err(err) => {
                    // Issue #59: surfaced as structured pipeline state, not
                    // an invisible eprintln! — a packaged app's tray can
                    // reflect this via `tray::tray_icon_state`. Issue #126
                    // (M2 PR 2.4): also a typed `pipeline-error` event so the
                    // pill window can toast a specific, blocking reason
                    // (most commonly mic-permission denial) rather than the
                    // generic Error icon alone.
                    eprintln!("bla: failed to start audio capture: {err}");
                    emit_pipeline_error(app, &errors::error_kind_for_capture_error(&err));
                    set_pipeline_state(app, tray::PipelineState::Error);
                }
            }
        }
        Some(hotkeys::Transition::StopRecording) => {
            // Sentinel 🟡 #2: take the session out from under the lock, THEN
            // stop() it — so the `capture` mutex isn't held across stop()'s
            // blocking join of the audio thread (which a concurrent
            // focus-loss reset would otherwise block on).
            let session = state.capture.lock().unwrap().take();
            if let Some(session) = session {
                session.stop();
            }
            stop_level_poller(&state);

            // Sentinel 🟡 #3: if a device/stream error was recorded mid-
            // recording (#59's CaptureDiagnostics), do NOT transcribe
            // garbage/partial audio as if healthy — surface Error and
            // discard, clearing the flag for the next session.
            if let Some(err) = state.diagnostics.last_error() {
                eprintln!("bla: audio capture was degraded, discarding this dictation: {err}");
                state.diagnostics.clear_error();
                state.buffer.lock().unwrap().drain();
                set_pipeline_state(app, tray::PipelineState::Error);
                return;
            }

            let samples = state.buffer.lock().unwrap().drain();
            // Issues #174/#175/#176: this dictation's identity for the
            // background thread `run_pipeline_in_background` is about to
            // spawn — the generation `StartRecording` minted for it (no
            // other `StartRecording` can have run between this dictation's
            // own Start and Stop, so this load reads back exactly that
            // value).
            let generation = state.dictation_generation.load(Ordering::SeqCst);
            set_pipeline_state(app, tray::PipelineState::Transcribing);
            run_pipeline_in_background(app.clone(), samples, generation);
        }
        Some(hotkeys::Transition::Cancelled) => {
            let session = state.capture.lock().unwrap().take();
            if let Some(session) = session {
                session.stop();
            }
            stop_level_poller(&state);
            state.buffer.lock().unwrap().drain();
            state.diagnostics.clear_error();
            set_pipeline_state(app, tray::PipelineState::Idle);
        }
        None => {}
    }
}

/// Signals the currently-running level-event poller (if any) to exit
/// (issue #126, M2 PR 2.2) — best-effort, matching the rest of this
/// module's `let _ =` treatment of non-critical signaling sends.
fn stop_level_poller(state: &AppState) {
    if let Some(tx) = state.level_poll_stop.lock().unwrap().take() {
        let _ = tx.send(());
    }
}

/// How often the level-event poller below samples [`audio::LevelMeter`]
/// (issue #126, M2 PR 2.2) — well above the ~30Hz cadence
/// [`audio::LevelThrottle`] caps emission at, so the throttle (not the
/// poll rate) is what determines the actual `audio-level` event cadence.
const LEVEL_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// OS-integration glue (AGENTS.md §OS-integration exemption, issue #126):
/// drives the throttled `audio-level` event stream for one capture
/// session's lifetime on a dedicated thread — never the real-time `cpal`
/// callback thread (`audio::start_capture`'s doc), matching the
/// CaptureDiagnostics-style split between an RT-safe write (`LevelMeter`,
/// written from the audio callback) and non-RT reads. Samples
/// `level_meter` every [`LEVEL_POLL_INTERVAL`], pushes each sample through
/// a fresh [`audio::LevelThrottle`], and emits `audio-level` (payload: the
/// `f32` RMS level, `0.0..=1.0`) whenever the throttle allows it. Exits as
/// soon as `stop_rx` is signaled (or its sender is dropped).
fn spawn_level_poller(
    app: tauri::AppHandle,
    level_meter: Arc<audio::LevelMeter>,
    stop_rx: std::sync::mpsc::Receiver<()>,
) {
    std::thread::spawn(move || {
        let origin = std::time::Instant::now();
        let mut throttle = audio::LevelThrottle::new();
        loop {
            match stop_rx.recv_timeout(LEVEL_POLL_INTERVAL) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let level = level_meter.current();
                    if let Some(level) = throttle.should_emit(origin.elapsed(), level) {
                        // `should_emit` already clamps to the documented
                        // `0.0..=1.0` contract via `audio::clamp_level`
                        // (issue #136 item 2) -- a tested pure fn, not
                        // untested arithmetic in this glue.
                        let _ = app.emit("audio-level", level);
                    }
                }
            }
        }
    });
}

/// Loads the bundled placeholder tray-icon PNG for `state` (issue #110): a
/// minimal monochrome glyph per [`tray::TrayIconState`] variant — a hollow
/// ring for Idle, a filled dot for Active, a filled dot with a notch for
/// Busy, and an "X" for Error (the four hand-authored 32×32 PNGs under
/// `icons/tray/`). Loading bundled bytes isn't a live OS call, but building
/// `tauri::image::Image` values is still Tauri-specific glue, so it lives
/// here rather than in `tray.rs` (which stays OS-call-free per its module
/// doc).
fn tray_icon_image(state: tray::TrayIconState) -> Image<'static> {
    let bytes: &[u8] = match state {
        tray::TrayIconState::Idle => include_bytes!("../icons/tray/idle.png"),
        tray::TrayIconState::Active => include_bytes!("../icons/tray/active.png"),
        tray::TrayIconState::Busy => include_bytes!("../icons/tray/busy.png"),
        tray::TrayIconState::Error => include_bytes!("../icons/tray/error.png"),
    };
    Image::from_bytes(bytes).expect("bundled tray icon PNGs (icons/tray/*.png) are well-formed")
}

fn set_pipeline_state(app: &tauri::AppHandle, new_state: tray::PipelineState) {
    // Normal path: pill visibility follows the pure `pill_visibility_for`
    // decision for `new_state`. The `show_pill` override exists only for the
    // informational-notice path (see `settle_idle_keeping_pill_for_notice`).
    let show_pill = tray::pill_visibility_for(&new_state);
    apply_pipeline_state(app, new_state, show_pill);
}

/// Applies `new_state` to the shared state, the emitted event, the tray
/// icon/menu, and the pill window — with `show_pill` deciding the pill's
/// visibility explicitly rather than always deriving it from `new_state`.
/// `set_pipeline_state` passes `tray::pill_visibility_for(&new_state)` (the
/// normal rule); the AC-4 informational-notice path
/// (`settle_idle_keeping_pill_for_notice`, Sentinel 🔴-2 on PR #135) passes
/// `true` so the transient toast is shown on a *visible* pill even as the
/// pipeline settles to `Idle`.
fn apply_pipeline_state(app: &tauri::AppHandle, new_state: tray::PipelineState, show_pill: bool) {
    let state = app.state::<AppState>();
    *state.pipeline_state.lock().unwrap() = new_state;
    let icon_state = tray::tray_icon_state(&new_state);
    let icon_label = format!("{icon_state:?}");
    let _ = app.emit("pipeline-state-changed", icon_label.clone());

    // Issue #110: reflect the same derived state on the real tray icon + its
    // disabled current-state menu line. `set_pipeline_state` runs on the
    // spawned pipeline thread and the global-shortcut callback thread, but
    // the tray icon/menu are AppKit objects on macOS that must only be
    // mutated on the main thread (off-main-thread AppKit mutation is
    // undefined behavior — it can crash or glitch mid-dictation). So clone
    // the (Send) handles and marshal the actual mutation onto the main
    // thread via `run_on_main_thread`. Best-effort throughout (`let _ =`): a
    // failure to repaint the tray must never take down the dictation
    // pipeline itself. The pill window is the same kind of AppKit-backed
    // object, so its show/hide is marshaled alongside the tray/icon updates
    // rather than called from whichever thread `set_pipeline_state` runs on.
    let tray_icon = app.tray_by_id(TRAY_ID);
    let state_item = state.tray_state_item.lock().unwrap().clone();
    let pill_window = app.get_webview_window(PILL_WINDOW_LABEL);
    let _ = app.run_on_main_thread(move || {
        if let Some(tray_icon) = tray_icon {
            let _ = tray_icon.set_icon(Some(tray_icon_image(icon_state)));
        }
        if let Some(item) = state_item {
            let _ = item.set_text(&icon_label);
        }
        if let Some(window) = pill_window {
            let _ = if show_pill {
                window.show()
            } else {
                window.hide()
            };
        }
    });
}

/// Emits the `pipeline-error` event (issue #126, M2 PR 2.4) the pill
/// window's toast listens for. `kind` is always mapped via one of
/// `errors::error_kind_for_*` (never built ad hoc at a call site), so the
/// HARD RULE in `errors.rs`'s module doc — the payload never carries
/// transcript/clipboard/audio content — holds at every emit site. Best-
/// effort like every other emit in this file (`let _ =`): a dropped toast
/// must never take down the dictation pipeline itself.
fn emit_pipeline_error(app: &tauri::AppHandle, kind: &errors::ErrorKind) {
    let event = errors::PipelineErrorEvent::from(kind);
    let _ = app.emit("pipeline-error", event);
}

/// How long the pill stays visible to carry an informational notice toast
/// (Sentinel 🔴-2 on PR #135). Matches the frontend toast's auto-dismiss
/// window (`TOAST_AUTO_DISMISS_MS` in `src/windows/pill/Toast.tsx`, 5s) so
/// the pill hides right about when the toast fades, not before.
const PILL_NOTICE_DURATION: std::time::Duration = std::time::Duration::from_millis(5000);

/// How long the pill stays visible to carry the "done" confirmation on a
/// completed dictation (issue #151). Matches the frontend reducer's
/// auto-hide window (`DONE_AUTO_HIDE_MS` in `src/lib/pillState.ts`, 1.5s) so
/// the pill hides right about when the "done" state itself reverts to
/// idle, not before — otherwise the backend would hide the OS window out
/// from under a still-rendering "done" state.
const DONE_PILL_DURATION: std::time::Duration = std::time::Duration::from_millis(1500);

/// Bumps the "pill visibility epoch" (issue #155) and returns the new
/// value. Called once at the start of every [`settle_idle_keeping_pill_visible`]
/// call — see that field's doc on [`AppState::pill_visibility_epoch`] for why.
fn bump_pill_visibility_epoch(state: &AppState) -> u64 {
    state.pill_visibility_epoch.fetch_add(1, Ordering::SeqCst) + 1
}

/// Whether `generation` — a dictation's id, captured once at
/// `StartRecording` and carried through `run_pipeline_in_background`/the
/// settle helpers it calls — is still the live dictation generation
/// (issues #174/#175/#176; see [`AppState::dictation_generation`] and
/// [`tray::should_apply_dictation_completion`]). Every background-thread
/// state write / event emit / settle spawn in this file is gated on this
/// returning `true` immediately beforehand; `false` means a newer dictation
/// has already started and the caller must no-op entirely.
fn generation_is_live(app: &tauri::AppHandle, generation: u64) -> bool {
    let state = app.state::<AppState>();
    let current = state.dictation_generation.load(Ordering::SeqCst);
    tray::should_apply_dictation_completion(generation, current)
}

/// Settles the pipeline to `Idle` while keeping the pill **visible** for
/// `duration`, then hiding it once that window elapses — unless a newer
/// overlapping settle or a new dictation says otherwise. Shared by both
/// "keep the pill visible past Idle" paths:
/// [`settle_idle_keeping_pill_for_notice`] (AC-4 informational toast,
/// Sentinel 🔴-2 on PR #135) and [`settle_idle_keeping_pill_for_done`]
/// (issue #151, the completed-dictation "done" confirmation).
///
/// The plain `set_pipeline_state(Idle)` would hide the pill immediately
/// (`pill_visibility_for(Idle) == false`), leaving whatever's currently
/// rendered on the pill (a toast, or the "done" dot/label) on a hidden
/// window. So this applies `Idle` (tray icon → Idle) with the pill forced
/// shown, bumps [`AppState::pill_visibility_epoch`] and captures the new
/// value, then — on a spawned, non-realtime thread mirroring
/// `spawn_stt_cache_warm`'s pattern — waits `duration` and hides the pill
/// **only if** [`tray::should_hide_pill_for_settle`] says so: the pipeline
/// must still be `Idle` (a dictation started during the window moves the
/// state off `Idle`, and that transition's own `set_pipeline_state` already
/// owns the pill) AND no *newer* settle must have started meanwhile (issue
/// #155's overlapping-notice epoch race — a notice and a done-settle, or two
/// notices, can overlap within their windows; without the epoch check the
/// older settle's delayed hide could fire after the newer settle already
/// re-applied `Idle`+visible, incorrectly hiding it). The window `hide()` is
/// marshaled to the main thread like every other pill mutation.
///
/// Issues #174/#175/#176: `generation` is this settle's dictation's id
/// (threaded through from `run_pipeline_in_background`). Checked once up
/// front — a newer dictation already superseding this one means even the
/// immediate `apply_pipeline_state(Idle, true)` below must not run, let
/// alone spawn a delayed-hide thread — and again when the delayed-hide
/// thread wakes, alongside the epoch/state checks
/// ([`tray::should_hide_pill_for_settle`]).
///
/// Issue #174: the delayed-hide thread locks `pipeline_state` FIRST, then
/// loads the epoch/generation atomics — not the other way around. Loading
/// the epoch before locking (the original, buggy order) left a window where
/// a newer settle's full bump-epoch-then-apply-Idle sequence could
/// interleave strictly between the epoch load and the lock acquisition, so
/// the reader would see a stale-but-matching epoch alongside the newer
/// settle's fresh `Idle` state — wrongly hiding the pill out from under it.
/// Locking first gives the epoch/generation loads a real happens-before edge
/// via the mutex acquire/release back to any writer whose state write this
/// thread just observed (a settle's epoch bump always precedes its own
/// state write in program order, and a `StartRecording`'s generation bump
/// always precedes ITS state write too), closing the race independent of
/// the atomics' own ordering.
fn settle_idle_keeping_pill_visible(
    app: &tauri::AppHandle,
    duration: std::time::Duration,
    generation: u64,
) {
    if !generation_is_live(app, generation) {
        return;
    }
    let epoch = {
        let state = app.state::<AppState>();
        bump_pill_visibility_epoch(&state)
    };
    apply_pipeline_state(app, tray::PipelineState::Idle, true);

    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(duration);
        let state = app.state::<AppState>();
        // #174: lock pipeline_state FIRST — see this function's doc comment.
        let current_state = *state.pipeline_state.lock().unwrap();
        let current_epoch = state.pill_visibility_epoch.load(Ordering::SeqCst);
        let current_generation = state.dictation_generation.load(Ordering::SeqCst);
        if tray::should_hide_pill_for_settle(
            epoch,
            current_epoch,
            generation,
            current_generation,
            &current_state,
        ) {
            let pill_window = app.get_webview_window(PILL_WINDOW_LABEL);
            let _ = app.run_on_main_thread(move || {
                if let Some(window) = pill_window {
                    let _ = window.hide();
                }
            });
        }
    });
}

/// Settles the pipeline to `Idle` for the AC-4 informational-notice path
/// (Sentinel 🔴-2 on PR #135) — see [`settle_idle_keeping_pill_visible`].
fn settle_idle_keeping_pill_for_notice(app: &tauri::AppHandle, generation: u64) {
    settle_idle_keeping_pill_visible(app, PILL_NOTICE_DURATION, generation);
}

/// Settles the pipeline to `Idle` for a completed-dictation "done"
/// confirmation (issue #151: previously `set_pipeline_state(Idle)` hid the
/// pill in the very call that entered the frontend's `"done"` mode, so the
/// ~1.5s confirmation never had a visible window to render onto). Only
/// takes the visible-settle path when `previous` confirms this really is a
/// completed-dictation transition
/// ([`tray::should_keep_pill_visible_for_done`]) — defense in depth in case
/// this is ever called from somewhere other than its one intended call site
/// (the non-fallback success arm of `run_pipeline_in_background`), falling
/// back to the plain settle otherwise so an unrelated transition never grows
/// a spurious "done" pill.
fn settle_idle_keeping_pill_for_done(
    app: &tauri::AppHandle,
    previous: tray::PipelineState,
    generation: u64,
) {
    // Issues #174/#175/#176: defense in depth — `run_pipeline_in_background`
    // already checks `generation_is_live` before calling this, but the plain
    // (non-visible-settle) `Idle` write below is itself a pipeline-state
    // write, so it gets the same gate rather than relying solely on the
    // caller.
    if !generation_is_live(app, generation) {
        return;
    }
    if !tray::should_keep_pill_visible_for_done(&previous) {
        set_pipeline_state(app, tray::PipelineState::Idle);
        return;
    }
    settle_idle_keeping_pill_visible(app, DONE_PILL_DURATION, generation);
}

/// Dispatches a click on one of the tray menu's items (issue #110), by the
/// id assigned when the item was built in `run()`'s `setup()`.
fn handle_tray_menu_event(app: &tauri::AppHandle, id: &str) {
    match id {
        "toggle-output" => toggle_output_mode_from_tray(app),
        "show" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "hide" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.hide();
            }
        }
        "settings" => {
            // Issue #126 (M2 PR 2.1): show + focus the `settings` webview
            // window (built `visible: false` in `tauri.conf.json` so it
            // never flashes on launch — this tray item is its only entry
            // point until the settings UI itself lands in a later PR).
            if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

/// The tray menu's Cursor/File toggle (issue #110): flips to whichever mode
/// isn't currently live and persists it through the **same**
/// `commands::set_output_mode` path the status window's toggle button calls
/// (AC-14), so both triggers update `tray::OutputModeSwitch`, `Settings`,
/// and the tray menu's own label identically — there is no second, drifting
/// copy of this decision.
fn toggle_output_mode_from_tray(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let current = state.output_switch.lock().unwrap().route_target();
    let next = match current {
        tray::OutputMode::CursorPaste => settings::OutputModeSetting::File,
        tray::OutputMode::File => settings::OutputModeSetting::Cursor,
    };
    if let Err(err) = commands::set_output_mode(app.clone(), state, next) {
        eprintln!("bla: tray output-mode toggle failed to persist: {err}");
    }
}

/// Selects the real `WhisperStt` engine under `--features whisper`,
/// resolving the model path from `settings`/`app_data_dir` via `models`'s
/// already-tested registry lookup (native glue, TDD-exempt per `stt.rs`'s
/// own module doc).
///
/// Issue #115: reuses `cache`'s already-built engine when
/// [`should_reuse_cached_stt`] says the cached preset still matches
/// `settings.model_preset` — returning an `Arc` clone (a refcount bump, not
/// a reload) rather than paying the ~574 MB `WhisperContext::new_with_params`
/// load again on every dictation. Only rebuilds (and replaces the cache
/// entry) when the cache is empty or the user switched presets.
///
/// Issues #117/#118: the ~574 MB load is performed with **no lock held**.
/// This mirrors [`spawn_stt_cache_warm`]: check for a hit under a narrow lock
/// scope and release the guard, load the model unlocked, then re-acquire and
/// re-check before populating (reusing a concurrently-cached engine rather
/// than clobbering it). Holding `cache`'s lock across the native load would
/// (a) poison the mutex for every later dictation and the warm thread if the
/// load panicked, and (b) block a concurrent dictation/warm for the whole
/// load. The trade-off is a rare, harmless transient double-load when a
/// first-launch dictation and the background warm load the same preset at
/// once — the loser's freshly built engine is simply dropped on the re-check,
/// and the cache settles to a single engine.
#[cfg(feature = "whisper")]
fn build_stt(
    settings: &settings::Settings,
    app_data_dir: &std::path::Path,
    cache: &Mutex<Option<CachedStt>>,
) -> Result<Arc<stt::WhisperStt>, String> {
    let wanted = settings.model_preset;

    // Fast path in a narrow lock scope: check for a HIT, then *release* the
    // guard before doing anything slow. Issues #117/#118: the cache lock is
    // never held across the multi-second `WhisperStt::new` load below, so a
    // panic in that native load can't poison the mutex (which would otherwise
    // wedge every later dictation *and* the warm thread), and a concurrent
    // background warm isn't blocked for the whole load. Mirrors
    // `spawn_stt_cache_warm`'s check → release → load → re-check → populate.
    {
        let guard = cache.lock().unwrap();
        if should_reuse_cached_stt(guard.as_ref().map(|cached| &cached.preset), &wanted) {
            // Perf instrumentation (issue #115 follow-up): a cache HIT means
            // this dictation paid no model-load cost — the whole point of #115.
            // Off unless BLA_PERF_LOG is set.
            stt::perf_log(&format!(
                "dictation: whisper cache HIT (preset={wanted:?}) — reused, no reload"
            ));
            return Ok(Arc::clone(
                &guard
                    .as_ref()
                    .expect("should_reuse_cached_stt only returns true when a cached engine exists")
                    .stt,
            ));
        }
    }

    // Perf instrumentation: a cache MISS pays the model load inline on the
    // dictation thread (WhisperStt::new logs the load ms) — expected only on
    // the first dictation of a preset before the background warm lands, or
    // right after a preset switch. Loaded with NO lock held (see above).
    stt::perf_log(&format!(
        "dictation: whisper cache MISS (preset={wanted:?}) — loading model now"
    ));
    let spec = spec_for_preset(to_models_preset(wanted));
    let model_path = models::model_target_path(app_data_dir, &spec);
    let stt = Arc::new(stt::WhisperStt::new(&model_path).map_err(|e| e.to_string())?);

    // Re-acquire and re-check under the lock: a concurrent background warm (or
    // another dictation) may have cached this exact preset while our load was
    // in flight — reuse theirs and drop ours rather than clobbering it with a
    // second, redundant engine (mirrors `spawn_stt_cache_warm`'s re-check).
    let mut guard = cache.lock().unwrap();
    if should_reuse_cached_stt(guard.as_ref().map(|cached| &cached.preset), &wanted) {
        return Ok(Arc::clone(
            &guard
                .as_ref()
                .expect("should_reuse_cached_stt only returns true when a cached engine exists")
                .stt,
        ));
    }
    *guard = Some(CachedStt {
        preset: wanted,
        stt: Arc::clone(&stt),
    });
    Ok(stt)
}

/// Default (no `whisper` feature) build: no real STT engine is compiled in
/// (CI/default `cargo build`/`cargo test` don't pay whisper.cpp's native
/// build cost, per `stt.rs`'s module doc). Always returns a clear
/// "model engine unavailable" error rather than silently running a fake
/// transcript in a real dictation flow; `FakeStt` only ever appears as the
/// (unreachable) `Ok` type so this has the same signature as the
/// `--features whisper` build above.
#[cfg(not(feature = "whisper"))]
fn build_stt(
    _settings: &settings::Settings,
    _app_data_dir: &std::path::Path,
) -> Result<stt::FakeStt, String> {
    Err(
        "speech-to-text model engine unavailable: this build was compiled without \
         the `whisper` cargo feature (enable it for the dev/app build, e.g. \
         `cargo tauri dev --features whisper`)"
            .to_string(),
    )
}

/// Warms `AppState::stt_cache` on a spawned thread (issue #115) so even the
/// *first* dictation after startup/first-run download is fast, rather than
/// paying the ~574 MB `WhisperContext` load synchronously on the first
/// hotkey release. Callers: `setup()` at startup (if the selected model file
/// is already on disk) and the first-run model-download-complete path (once
/// the download finishes). Guarded by the same [`should_reuse_cached_stt`]
/// check `build_stt` uses, so calling this when the cache already holds the
/// right preset (e.g. a dictation already warmed it, or this is called
/// twice) is a cheap no-op rather than a redundant reload. Never blocks its
/// caller — the load happens entirely on the spawned thread — and a load
/// failure is logged (structured, no transcript/model bytes) and leaves the
/// cache empty rather than panicking: `build_stt`'s lazy path is always the
/// fallback if warming didn't happen or failed.
#[cfg(feature = "whisper")]
fn spawn_stt_cache_warm(
    app: tauri::AppHandle,
    app_data_dir: std::path::PathBuf,
    preset: settings::ModelPreset,
) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        {
            let guard = state.stt_cache.lock().unwrap();
            if should_reuse_cached_stt(guard.as_ref().map(|cached| &cached.preset), &preset) {
                stt::perf_log(&format!(
                    "background warm: skipped (preset={preset:?} already cached)"
                ));
                return;
            }
        }

        // Perf instrumentation (issue #115 follow-up): mark the background
        // warm so the one-time model load can be seen happening OFF the
        // dictation path (WhisperStt::new logs the load ms). Off unless
        // BLA_PERF_LOG is set.
        stt::perf_log(&format!(
            "background warm: loading whisper model (preset={preset:?})"
        ));
        let spec = spec_for_preset(to_models_preset(preset));
        let model_path = models::model_target_path(&app_data_dir, &spec);
        match stt::WhisperStt::new(&model_path) {
            Ok(built) => {
                let mut guard = state.stt_cache.lock().unwrap();
                // Re-check under the lock: a dictation's own `build_stt` may
                // have already loaded (and cached) this exact preset while
                // this warm was in flight — don't clobber it with a second,
                // redundant engine.
                if !should_reuse_cached_stt(guard.as_ref().map(|cached| &cached.preset), &preset) {
                    *guard = Some(CachedStt {
                        preset,
                        stt: Arc::new(built),
                    });
                    stt::perf_log(&format!(
                        "background warm: cache populated (preset={preset:?}) — first dictation will be a HIT"
                    ));
                }
            }
            Err(err) => {
                eprintln!(
                    "bla: background whisper model warm-up failed (dictation will load it \
                     lazily instead): {err}"
                );
            }
        }
    });
}

/// Default (no `whisper` feature) build: nothing to warm — there is no
/// `WhisperStt`/`stt_cache` compiled in, so this is a no-op with the same
/// signature as the `--features whisper` build above (mirrors `build_stt`'s
/// two-body pattern so call sites never need a feature-gated branch).
#[cfg(not(feature = "whisper"))]
fn spawn_stt_cache_warm(
    _app: tauri::AppHandle,
    _app_data_dir: std::path::PathBuf,
    _preset: settings::ModelPreset,
) {
}

/// Runs the dictation pipeline (issue #25's `pipeline::Pipeline`) over
/// `samples` in a background thread, so the shortcut-handler callback that
/// triggered `StopRecording` never blocks on transcription. Cleanup is
/// `OllamaCleanup` with `Pipeline`'s built-in `RegexCleanup` fallback
/// (AC-4); output is routed per the live output-mode switch, itself seeded
/// from `Settings` (AC-14).
fn run_pipeline_in_background(app: tauri::AppHandle, samples: Vec<f32>, generation: u64) {
    std::thread::spawn(move || {
        let (settings, route_target, dictionary, tone, active_app_name) = {
            let state = app.state::<AppState>();
            let settings = state.settings.lock().unwrap().clone();
            let route_target = state.output_switch.lock().unwrap().route_target();
            // Issue #200 (PRD AC-21): read the personal dictionary once per
            // dictation and feed it into both sides of the pipeline below —
            // Whisper's initial_prompt (via TranscribeOpts) and the
            // cleanup_v2 rewrite prompt (via OllamaCleanup::with_dictionary).
            // Best-effort: a read failure must not fail the dictation itself
            // (mirrors the settings/route-target reads just above, which
            // don't handle a poisoned-lock panic differently either).
            let dictionary = {
                let store = state.store.lock().unwrap();
                dictionary_terms_for_pipeline(&store).unwrap_or_else(|err| {
                    eprintln!(
                        "bla: failed to read personal dictionary, proceeding with none: {err}"
                    );
                    Vec::new()
                })
            };
            // Issue #202 (PRD AC-22): resolve this dictation's Tone from
            // the app `react_to_transition`'s StartRecording arm detected
            // at hotkey-press time, against whatever tone_rules are live
            // RIGHT NOW (never cached) — so a rule edited between
            // dictations takes effect on the very next one, no restart
            // required (AC-41). Best-effort, same pattern as the
            // dictionary read just above: a rules-read failure must not
            // fail the dictation, it just resolves to Tone::Neutral.
            let active_app_name = state.active_app_name.lock().unwrap().clone();
            let tone = {
                let store = state.store.lock().unwrap();
                let tone_rules = store.list_tone_rules().unwrap_or_else(|err| {
                    eprintln!(
                        "bla: failed to read tone rules, proceeding with Tone::Neutral: {err}"
                    );
                    Vec::new()
                });
                context::resolve_tone_for_app(active_app_name.as_ref(), &tone_rules)
            };
            (settings, route_target, dictionary, tone, active_app_name)
        };

        let app_data_dir = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("bla"));

        let output_mode = match route_target {
            tray::OutputMode::CursorPaste => output::OutputMode::CursorPaste,
            // Issue #180: `file_base_dir` is the settings-window picker's
            // "base folder / vault" preference — `resolve_base_dir` (pure,
            // unit-tested in output.rs) falls back to `app_data_dir` when
            // it's unset, so a user who never opens the picker keeps the
            // previous hard-coded behavior unchanged.
            tray::OutputMode::File => output::OutputMode::File {
                base_dir: output::resolve_base_dir(&settings.file_base_dir, &app_data_dir),
                config: output::FileConfig {
                    path_template: settings.file_path_template.clone(),
                    timestamp_prefix_template: Some("{{time:HH:mm}} ".to_string()),
                },
            },
        };

        let opts = pipeline::PipelineOpts {
            transcribe: stt::TranscribeOpts {
                dictionary: dictionary.clone(),
            },
            tone,
            output_mode,
            clock: real_clock(),
            restore_delay: output::DEFAULT_RESTORE_DELAY,
        };

        let cleanup = cleanup::OllamaCleanup::with_default_base_url(
            "llama3",
            cleanup::UreqTransport::default(),
        )
        .with_dictionary(dictionary);

        // Issue #115: `build_stt`'s two bodies differ only in whether they
        // consult/populate `AppState::stt_cache` (whisper feature) — the
        // default build has no cache to pass. `state` is re-fetched here
        // (cheap: just a managed-state lookup) rather than threaded through
        // from the block above, since only the `whisper` build needs it.
        #[cfg(feature = "whisper")]
        let stt_result = {
            let state = app.state::<AppState>();
            build_stt(&settings, &app_data_dir, &state.stt_cache)
        };
        #[cfg(not(feature = "whisper"))]
        let stt_result = build_stt(&settings, &app_data_dir);

        match stt_result {
            Ok(stt_engine) => {
                let pipeline = pipeline::Pipeline::new(
                    stt_engine,
                    cleanup,
                    output::SystemClipboard,
                    output::EnigoPaste,
                    std::thread::sleep,
                );
                match pipeline.run(&samples, &opts) {
                    Ok(outcome) => {
                        // Issue #198 (AC-29): persist exactly one history
                        // row for this completed run BEFORE the generation
                        // check below — deliberately NOT gated on
                        // `generation_is_live`. `Pipeline::run` returning
                        // `Ok` means the text was already pasted/written by
                        // this point, regardless of whether a newer
                        // dictation has since superseded this one for UI
                        // purposes; the generation gate exists to suppress
                        // stale UI-visible effects (event emits, pill state,
                        // settle spawns — see the comment on that check just
                        // below), not to decide whether a dictation
                        // "happened".
                        //
                        // Issue #220 (Sentinel SNTL-20260715-bla-PR218-cc04f8b
                        // 🟡): a failure here is still ALWAYS logged
                        // (`eprintln!`, no transcript content — see
                        // `store.rs`'s no-log invariant) and must never
                        // fail/hide the completion the user already saw
                        // pasted — but it's also captured in
                        // `history_persist_failed` so it CAN be surfaced as
                        // a toast below, once the generation gate has run.
                        // The insert itself stays unconditional (never
                        // skipped for a stale generation, matching the row
                        // itself never being dropped for one); only the
                        // *toast* is gated, same as every other UI-visible
                        // effect this function emits — a stale generation's
                        // toast would otherwise render over whatever a
                        // newer, already-live dictation's pill is showing.
                        let history_persist_failed = {
                            let app_state = app.state::<AppState>();
                            let store = app_state.store.lock().unwrap();
                            // Issue #202: `active_app_name` is the same
                            // hotkey-press-time detection this dictation's
                            // Tone was resolved from above — now threaded
                            // through to the history row too (app NAME
                            // only, never a window title — AC-43).
                            let app_name = active_app_name.as_ref().map(|a| a.0.as_str());
                            match record_history_entry(&store, now_ms(), &outcome, app_name) {
                                Ok(_) => false,
                                Err(err) => {
                                    eprintln!("bla: failed to persist history entry: {err}");
                                    true
                                }
                            }
                        };
                        // Issues #174/#175/#176: this completion belongs to
                        // `generation` — check it's still the live dictation
                        // BEFORE touching any shared state (including
                        // reading `pipeline_state` for `previous` below,
                        // which would otherwise read a NEWER dictation's
                        // live value under this stale dictation's name). A
                        // stale generation means a newer dictation already
                        // started while this one was transcribing; no-op
                        // entirely — no event emit, no state write, no
                        // settle spawn.
                        if !generation_is_live(&app, generation) {
                            return;
                        }
                        // Issue #220: surfaced here (post-gate) rather than
                        // inside the block above, alongside the Ollama
                        // fallback notice just below — same reasoning:
                        // informational, emitted alongside a successful
                        // completion, never in place of one.
                        if history_persist_failed {
                            emit_pipeline_error(&app, &errors::ErrorKind::HistoryPersistFailed);
                        }
                        // Issue #126 (M2 PR 2.4), AC-4/ADR-0005: the Ollama
                        // fallback is informational, not a failure — the
                        // dictation already completed and pasted/wrote
                        // successfully above. Emit alongside the Idle
                        // transition, never in place of it.
                        if outcome.cleanup_fell_back {
                            emit_pipeline_error(&app, &errors::ErrorKind::OllamaUnreachable);
                        }
                        // Issue #220: either informational toast above needs
                        // the longer notice-visible window to actually be
                        // seen — `should_settle_with_notice` is the shared
                        // pure decision (previously just `cleanup_fell_back`
                        // alone).
                        if tray::should_settle_with_notice(
                            outcome.cleanup_fell_back,
                            history_persist_failed,
                        ) {
                            // Sentinel 🔴-2 (PR #135): a plain Idle transition
                            // would hide the pill immediately, leaving this
                            // informational toast on a hidden window. Keep the
                            // pill visible for the toast's lifetime, then
                            // settle to hidden/Idle (unless a new dictation
                            // preempts).
                            settle_idle_keeping_pill_for_notice(&app, generation);
                        } else {
                            // Issue #151: a plain Idle transition hid the
                            // pill in the same call that entered the
                            // frontend's "done" state, so the ~1.5s "done"
                            // confirmation never had a visible window to
                            // render onto. `previous` is read before the
                            // transition (it's `Transcribing`, set when this
                            // dictation started) so the settle can confirm
                            // via the pure `should_keep_pill_visible_for_done`
                            // that this really is a completed-dictation
                            // transition before keeping the pill visible.
                            // Safe to read here (rather than stale) because
                            // of the generation check just above.
                            let previous = *app.state::<AppState>().pipeline_state.lock().unwrap();
                            settle_idle_keeping_pill_for_done(&app, previous, generation);
                        }
                    }
                    Err(err) => {
                        // Issues #174/#175/#176: same gate as the Ok arm —
                        // a stale pipeline failure from a superseded
                        // dictation must not clobber the live one's state.
                        if !generation_is_live(&app, generation) {
                            return;
                        }
                        eprintln!("bla: pipeline run failed: {err}");
                        emit_pipeline_error(&app, &errors::error_kind_for_pipeline_error(&err));
                        set_pipeline_state(&app, tray::PipelineState::Error);
                    }
                }
            }
            Err(msg) => {
                // Issues #174/#175/#176: same gate — a superseded
                // dictation's STT-build failure must not clobber the live
                // one's state either.
                if !generation_is_live(&app, generation) {
                    return;
                }
                eprintln!("bla: {msg}");
                emit_pipeline_error(&app, &errors::error_kind_for_build_stt_failure(&msg));
                set_pipeline_state(&app, tray::PipelineState::Error);
            }
        }
    });
}

/// Wall-clock `output::Clock` for file-mode path/timestamp templating —
/// the one place this crate's OS-glue reads the real system clock (`output`
/// itself never does, per its module doc: `Clock` is always injected).
fn real_clock() -> output::Clock {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days_since_epoch = secs / 86_400;
    let time_of_day = secs % 86_400;

    let (year, month, day) = civil_from_days(days_since_epoch as i64);
    output::Clock {
        year,
        month,
        day,
        hour: (time_of_day / 3600) as u32,
        minute: ((time_of_day % 3600) / 60) as u32,
    }
}

/// Howard Hinnant's `civil_from_days` algorithm: converts a day count since
/// the Unix epoch (1970-01-01) to a proleptic-Gregorian (year, month, day).
/// Pure arithmetic, no OS/timezone calls (deliberately UTC — matching
/// `SystemTime`'s epoch semantics), used only to build [`real_clock`]'s
/// `output::Clock` from `SystemTime`.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

#[cfg(test)]
mod clock_tests {
    use super::*;

    #[test]
    fn civil_from_days_matches_known_reference_dates() {
        // 1970-01-01 is day 0 by definition.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2026-07-08 (a date referenced elsewhere in this milestone's
        // fixtures) is 20642 days after the epoch.
        assert_eq!(civil_from_days(20_642), (2026, 7, 8));
        // A leap-day boundary.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // Issue #126 (M2 PR 2.6): registers/unregisters bla as an OS login
        // item, driven by `commands::set_settings`'s `launch_at_login`
        // side-effect (via `tauri_plugin_autostart::ManagerExt::autolaunch`)
        // — no command from this plugin's own `invoke_handler` is exposed
        // to the frontend, so no new `capabilities/` grant is needed.
        // `MacosLauncher::LaunchAgent` is the plugin's documented default
        // (a launch agent plist rather than an AppleScript login item).
        // Dev-build note: this registers the CURRENT binary's path, so in a
        // `cargo tauri dev`/`cargo run` build enabling autostart points at
        // the dev binary (`target/debug/bla`), not a stable packaged-app
        // path — expected and harmless for local development.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::get_platform,
            commands::set_settings,
            commands::set_output_mode,
            commands::validate_hotkey,
            commands::download_selected_model,
            commands::model_registry,
            commands::suspend_hotkey,
            commands::resume_hotkey,
            commands::search_history,
            commands::copy_history_entry,
            commands::delete_history_entry,
            commands::clear_history,
            commands::list_dictionary_terms,
            commands::add_dictionary_term,
            commands::remove_dictionary_term,
            commands::list_tone_rules,
            commands::upsert_tone_rule,
            commands::delete_tone_rule,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let app_data_dir = handle
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("bla"));

            // Issue #80: NotFound (first run) silently defaults; Corrupt is
            // surfaced (logged) rather than silently discarded, then still
            // falls back to defaults so the app remains usable — a real
            // settings UI (M2) can offer a proper recovery flow.
            let settings = match load_settings_from_store(&handle) {
                Ok(s) => s,
                Err(settings::SettingsLoadError::NotFound) => settings::Settings::default(),
                Err(settings::SettingsLoadError::Corrupt(msg)) => {
                    eprintln!("bla: persisted settings could not be parsed, using defaults: {msg}");
                    settings::Settings::default()
                }
            };

            // Issue #110: build the tray icon + menu before `app.manage`,
            // since the menu items' handles are stashed in `AppState` for
            // `set_pipeline_state`/`commands::set_output_mode` to relabel
            // later. Menu: a disabled current-state line, the Cursor/File
            // toggle (shares `commands::set_output_mode` with the status
            // window), Show/Hide window, Settings… (issue #126, M2 PR 2.1 —
            // shows + focuses the `settings` webview window), and Quit.
            let initial_output_mode = to_tray_output_mode(settings.output_mode);
            let tray_state_item = MenuItem::with_id(
                &handle,
                "state",
                format!("{:?}", tray::tray_icon_state(&tray::PipelineState::Idle)),
                false,
                None::<&str>,
            )?;
            let tray_toggle_item = MenuItem::with_id(
                &handle,
                "toggle-output",
                output_mode_toggle_label(initial_output_mode),
                true,
                None::<&str>,
            )?;
            let tray_show_item =
                MenuItem::with_id(&handle, "show", "Show Window", true, None::<&str>)?;
            let tray_hide_item =
                MenuItem::with_id(&handle, "hide", "Hide Window", true, None::<&str>)?;
            let tray_settings_item =
                MenuItem::with_id(&handle, "settings", "Settings…", true, None::<&str>)?;
            let tray_quit_item = MenuItem::with_id(&handle, "quit", "Quit", true, None::<&str>)?;
            let tray_menu = Menu::with_items(
                &handle,
                &[
                    &tray_state_item,
                    &PredefinedMenuItem::separator(&handle)?,
                    &tray_toggle_item,
                    &PredefinedMenuItem::separator(&handle)?,
                    &tray_show_item,
                    &tray_hide_item,
                    &tray_settings_item,
                    &PredefinedMenuItem::separator(&handle)?,
                    &tray_quit_item,
                ],
            )?;
            TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_icon_image(tray::TrayIconState::Idle))
                .icon_as_template(true)
                .tooltip("bla")
                .menu(&tray_menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| handle_tray_menu_event(app, event.id().as_ref()))
                .build(&handle)?;

            // Issue #198 (M3 PR 3.2): open the headless history store
            // against the OS app-data dir (MISSION §5: local SQLite only,
            // nothing leaves the device). `Store::open` is non-fatal on
            // failure — mirrors the hotkey-registration handling just below:
            // a disk error here must not brick launch. Falling back to
            // `open_in_memory` keeps every history command working for this
            // session; the only degradation is that history won't survive a
            // restart, which is strictly better than the app failing to
            // start at all.
            let history_db_path = app_data_dir.join("history.sqlite3");
            let history_store = store::Store::open(&history_db_path).unwrap_or_else(|err| {
                eprintln!(
                    "bla: failed to open history store at {history_db_path:?}, falling back to \
                     an in-memory store for this session (history will not persist across \
                     restart): {err}"
                );
                store::Store::open_in_memory()
                    .expect("in-memory SQLite open must succeed as a last-resort fallback")
            });

            // AC-31: prune on startup per the persisted retention_days
            // setting, before the store is handed to any command/pipeline
            // call site.
            if let Err(err) =
                prune_history_for_retention(&history_store, now_ms(), settings.retention_days)
            {
                eprintln!("bla: failed to prune history on startup: {err}");
            }

            let state = AppState {
                hotkeys: Mutex::new(hotkeys::StateMachine::new(
                    to_hotkey_mode(settings.recording_mode),
                    [0u32],
                    hotkeys::DEFAULT_DEBOUNCE,
                )),
                buffer: Arc::new(Mutex::new(audio::RingBuffer::new(
                    audio::TARGET_SAMPLE_RATE as usize * MAX_CAPTURE_SECONDS,
                ))),
                diagnostics: Arc::new(audio::CaptureDiagnostics::new()),
                capture: Mutex::new(None),
                level_meter: Arc::new(audio::LevelMeter::new()),
                level_poll_stop: Mutex::new(None),
                settings: Mutex::new(settings.clone()),
                output_switch: Mutex::new(tray::OutputModeSwitch::new(initial_output_mode)),
                pipeline_state: Mutex::new(tray::PipelineState::Idle),
                tray_state_item: Mutex::new(Some(tray_state_item)),
                tray_output_toggle_item: Mutex::new(Some(tray_toggle_item)),
                pill_visibility_epoch: AtomicU64::new(0),
                dictation_generation: AtomicU64::new(0),
                hotkey_suspend_gen: Mutex::new(0),
                #[cfg(feature = "whisper")]
                stt_cache: Mutex::new(None),
                store: Mutex::new(history_store),
                active_app_name: Mutex::new(None),
            };
            app.manage(state);

            // Issue #91 (Sentinel 🔴): a bad persisted hotkey must not brick
            // launch. Resolve to the persisted binding only if it's valid,
            // else the always-valid default; then register NON-FATALLY — a
            // registration failure (e.g. an OS-level accelerator conflict)
            // is logged and the app still launches, rather than propagating
            // out of `.setup()` into `.run(...).expect(...)` → startup
            // panic with no self-recovery. `set_settings` already prevents
            // an invalid hotkey from being persisted in the first place;
            // this is the defense-in-depth for a settings.json that was
            // already corrupt (or written by an older build).
            let default_hotkey = settings::Settings::default().hotkey;
            let effective_hotkey =
                hotkeys::resolve_effective_hotkey(&settings.hotkey, &default_hotkey).to_string();
            if let Err(err) = register_hotkey(&handle, &effective_hotkey) {
                eprintln!(
                    "bla: failed to register global hotkey {effective_hotkey:?} at startup; \
                     the app will launch without a bound dictation hotkey: {err}"
                );
            }

            // Issue #44: reconcile a possibly-dropped KeyUp on window
            // focus-loss so the machine can never wedge in Holding. Issue
            // #110: closing the window (the titlebar close button) hides it
            // instead of quitting the whole app — this is a tray-resident
            // utility now, so "close" and "quit" are deliberately different
            // actions; the tray menu's Quit item is the only way to exit.
            if let Some(window) = app.get_webview_window("main") {
                let focus_handle = handle.clone();
                let close_handle = handle.clone();
                window.on_window_event(move |event| match event {
                    tauri::WindowEvent::Focused(false) => {
                        reconcile_hotkeys_on_focus_loss(&focus_handle);
                    }
                    tauri::WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        if let Some(window) = close_handle.get_webview_window("main") {
                            let _ = window.hide();
                        }
                    }
                    _ => {}
                });
            }

            // Issue #126 (Sentinel 🔴 #2 on PR #127): the settings window's
            // titlebar close button must hide, not destroy, the window —
            // a destroyed webview makes the tray's "Settings…" item's
            // `get_webview_window` lookup return `None` forever, silently
            // no-oping until app restart. Same close-to-hide pattern as the
            // main window above (only the CloseRequested arm; the
            // focus-loss hotkey reconcile stays main-window-only).
            if let Some(window) = app.get_webview_window(SETTINGS_WINDOW_LABEL) {
                let close_handle = handle.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        // PR #185 Sentinel 🔴-1(b): the window hides (not
                        // destroys), so React never unmounts — if the
                        // hotkey-capture field had suspended the global
                        // shortcut, restore it here or it stays dead until
                        // app restart.
                        force_resume_hotkey(&close_handle);
                        // PR #185 Sentinel delta 🟡-3: tell the (still-alive)
                        // settings webview to leave capture mode, so a field
                        // that was mid-capture at close isn't stuck swallowing
                        // keys when the window is reopened.
                        let _ = close_handle.emit("hotkey-capture-reset", ());
                        if let Some(window) = close_handle.get_webview_window(SETTINGS_WINDOW_LABEL)
                        {
                            let _ = window.hide();
                        }
                    }
                });
            }

            // Minimal first-run model check (issue #91 Part B): if the
            // selected Whisper model is absent, kick the downloader in the
            // background and emit progress events. Full onboarding UX
            // (progress UI, model picker) is M5 — this only unblocks the
            // AC-7 smoke test by getting a model onto disk automatically,
            // matching MISSION §9's pre-authorized "downloading Whisper
            // GGUF models from huggingface.co for dev/test".
            //
            // Issue #115: either way, warm `AppState::stt_cache` on a
            // background thread rather than leaving the very first
            // dictation to pay the ~574 MB `WhisperContext` load
            // synchronously — if the model is already on disk, warm it now;
            // if it still needs downloading, warm it once that finishes
            // (right after the `model-download-complete` emit below).
            {
                let spec = spec_for_preset(to_models_preset(settings.model_preset));
                let target = models::model_target_path(&app_data_dir, &spec);
                if !target.exists() {
                    let progress_handle = handle.clone();
                    let warm_handle = handle.clone();
                    let warm_preset = settings.model_preset;
                    std::thread::spawn(move || {
                        let transport = models::UreqTransport::new();
                        let result = models::download_model_with_spec(
                            &transport,
                            &spec,
                            &app_data_dir,
                            move |progress| {
                                let _ = progress_handle.emit("model-download-progress", progress);
                            },
                        );
                        match result {
                            // Issue #110: a completed download must announce
                            // itself, or the status window is stuck showing
                            // "Downloading… 100%" forever (the final progress
                            // event lands before the checksum+rename, and
                            // nothing signals "ready" afterward). Emit a
                            // terminal completion event the UI flips to Ready
                            // on.
                            Ok(_) => {
                                let _ = handle.emit("model-download-complete", ());
                                // Issue #115: the model just landed on disk —
                                // warm the cache now so the first dictation
                                // after a first-run download is still fast.
                                spawn_stt_cache_warm(
                                    warm_handle,
                                    app_data_dir.clone(),
                                    warm_preset,
                                );
                            }
                            Err(err) => {
                                eprintln!("bla: first-run model download failed: {err}");
                                let _ = handle.emit("model-download-error", err.to_string());
                            }
                        }
                    });
                } else {
                    // Issue #115: the model is already on disk from a
                    // previous run — warm the cache in the background so the
                    // first dictation of this session doesn't pay the load
                    // cost synchronously.
                    spawn_stt_cache_warm(
                        handle.clone(),
                        app_data_dir.clone(),
                        settings.model_preset,
                    );
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
