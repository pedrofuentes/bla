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

use std::sync::{Arc, Mutex};

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
    /// Issue #115: the cached Whisper engine, so it's loaded from disk (a
    /// ~574 MB read for the default preset) at most once per selected
    /// preset rather than on every dictation. `None` until the first build
    /// (lazily, from `build_stt`, or eagerly from a background warm —
    /// see `spawn_stt_cache_warm`). Only present in `--features whisper`
    /// builds; the default build has no `WhisperStt` to cache.
    #[cfg(feature = "whisper")]
    stt_cache: Mutex<Option<CachedStt>>,
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
            set_pipeline_state(app, tray::PipelineState::Transcribing);
            run_pipeline_in_background(app.clone(), samples);
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

/// Settles the pipeline to `Idle` for the AC-4 informational-notice path
/// (Sentinel 🔴-2 on PR #135) while keeping the pill **visible** for the
/// toast's lifetime, then hiding it once the notice window elapses.
///
/// The plain `set_pipeline_state(Idle)` would hide the pill immediately
/// (`pill_visibility_for(Idle) == false`), leaving the just-emitted
/// `OllamaUnreachable` toast on a hidden window. So this applies `Idle`
/// (tray icon → Idle) with the pill forced shown, then — on a spawned,
/// non-realtime thread mirroring `spawn_stt_cache_warm`'s pattern — waits
/// [`PILL_NOTICE_DURATION`] and hides the pill **only if the pipeline is
/// still `Idle`** ([`tray::should_hide_pill_after_notice`], the unit-tested
/// pure decision). A dictation started during the notice moves the state off
/// `Idle`, so its own `set_pipeline_state` owns the pill and this elapsed
/// notice leaves it alone — the new dictation preempts cleanly. The window
/// `hide()` is marshaled to the main thread like every other pill mutation.
fn settle_idle_keeping_pill_for_notice(app: &tauri::AppHandle) {
    apply_pipeline_state(app, tray::PipelineState::Idle, true);

    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(PILL_NOTICE_DURATION);
        let state = app.state::<AppState>();
        let current_state = *state.pipeline_state.lock().unwrap();
        if tray::should_hide_pill_after_notice(&current_state) {
            let pill_window = app.get_webview_window(PILL_WINDOW_LABEL);
            let _ = app.run_on_main_thread(move || {
                if let Some(window) = pill_window {
                    let _ = window.hide();
                }
            });
        }
    });
}

/// Settles the pipeline to `Idle` for a completed-dictation "done"
/// confirmation (issue #151: previously `set_pipeline_state(Idle)` hid the
/// pill in the very call that entered the frontend's `"done"` mode, so the
/// ~1.5s confirmation never had a visible window to render onto). Mirrors
/// [`settle_idle_keeping_pill_for_notice`]'s grace-window mechanism onto the
/// completed-dictation transition instead: applies `Idle` with the pill
/// forced shown, then — on a spawned, non-realtime thread — waits
/// [`DONE_PILL_DURATION`] and hides the pill only if the pipeline is still
/// `Idle` ([`tray::should_hide_pill_after_notice`]), so a dictation started
/// during the window preempts cleanly, same as the notice path.
///
/// Only takes the visible-settle path when `previous` confirms this really
/// is a completed-dictation transition
/// ([`tray::should_keep_pill_visible_for_done`]) — defense in depth in case
/// this is ever called from somewhere other than its one intended call site
/// (the non-fallback success arm of `run_pipeline_in_background`), falling
/// back to the plain settle otherwise so an unrelated transition never grows
/// a spurious "done" pill.
fn settle_idle_keeping_pill_for_done(app: &tauri::AppHandle, previous: tray::PipelineState) {
    if !tray::should_keep_pill_visible_for_done(&previous) {
        set_pipeline_state(app, tray::PipelineState::Idle);
        return;
    }

    apply_pipeline_state(app, tray::PipelineState::Idle, true);

    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(DONE_PILL_DURATION);
        let state = app.state::<AppState>();
        let current_state = *state.pipeline_state.lock().unwrap();
        if tray::should_hide_pill_after_notice(&current_state) {
            let pill_window = app.get_webview_window(PILL_WINDOW_LABEL);
            let _ = app.run_on_main_thread(move || {
                if let Some(window) = pill_window {
                    let _ = window.hide();
                }
            });
        }
    });
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
fn run_pipeline_in_background(app: tauri::AppHandle, samples: Vec<f32>) {
    std::thread::spawn(move || {
        let (settings, route_target) = {
            let state = app.state::<AppState>();
            let settings = state.settings.lock().unwrap().clone();
            let route_target = state.output_switch.lock().unwrap().route_target();
            (settings, route_target)
        };

        let app_data_dir = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("bla"));

        let output_mode = match route_target {
            tray::OutputMode::CursorPaste => output::OutputMode::CursorPaste,
            tray::OutputMode::File => output::OutputMode::File {
                base_dir: app_data_dir.clone(),
                config: output::FileConfig {
                    path_template: settings.file_path_template.clone(),
                    timestamp_prefix_template: Some("{{time:HH:mm}} ".to_string()),
                },
            },
        };

        let opts = pipeline::PipelineOpts {
            transcribe: stt::TranscribeOpts::default(),
            tone: cleanup::Tone::Neutral,
            output_mode,
            clock: real_clock(),
            restore_delay: output::DEFAULT_RESTORE_DELAY,
        };

        let cleanup = cleanup::OllamaCleanup::with_default_base_url(
            "llama3",
            cleanup::UreqTransport::default(),
        );

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
                        // Issue #126 (M2 PR 2.4), AC-4/ADR-0005: the Ollama
                        // fallback is informational, not a failure — the
                        // dictation already completed and pasted/wrote
                        // successfully above. Emit alongside the Idle
                        // transition, never in place of it.
                        if outcome.cleanup_fell_back {
                            emit_pipeline_error(&app, &errors::ErrorKind::OllamaUnreachable);
                            // Sentinel 🔴-2 (PR #135): a plain Idle transition
                            // would hide the pill immediately, leaving this
                            // informational toast on a hidden window. Keep the
                            // pill visible for the toast's lifetime, then
                            // settle to hidden/Idle (unless a new dictation
                            // preempts).
                            settle_idle_keeping_pill_for_notice(&app);
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
                            let previous = *app.state::<AppState>().pipeline_state.lock().unwrap();
                            settle_idle_keeping_pill_for_done(&app, previous);
                        }
                    }
                    Err(err) => {
                        eprintln!("bla: pipeline run failed: {err}");
                        emit_pipeline_error(&app, &errors::error_kind_for_pipeline_error(&err));
                        set_pipeline_state(&app, tray::PipelineState::Error);
                    }
                }
            }
            Err(msg) => {
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
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::set_settings,
            commands::set_output_mode,
            commands::validate_hotkey,
            commands::download_selected_model,
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
                #[cfg(feature = "whisper")]
                stt_cache: Mutex::new(None),
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
