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

use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_store::StoreExt;

pub mod audio;
// `pub` (rather than private like their stub siblings): as of the pipeline
// increment (issue #25), `cleanup`/`output`/`pipeline` are real, tested,
// standalone-usable API surface — `pipeline` composes `Stt` + `Cleanup` +
// the output router headlessly, and the cumulative acceptance suite
// (`tests/acceptance.rs`) exercises them from outside the crate.
pub mod cleanup;
mod commands;
mod context;
mod hotkeys;
// `pub` (issue #24, ADR-0004): the first-run model downloader's registry,
// AC-12 network guard, and download orchestration are real, tested,
// standalone-usable API surface.
pub mod models;
pub mod output;
pub mod pipeline;
pub mod settings;
mod store;
pub mod stt;
pub mod tray;

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
    settings: Mutex<settings::Settings>,
    output_switch: Mutex<tray::OutputModeSwitch>,
    pipeline_state: Mutex<tray::PipelineState>,
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

#[cfg(test)]
mod mapping_tests {
    use super::*;

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
            // Drop any stale samples from a previous session before
            // starting a fresh capture window.
            state.buffer.lock().unwrap().drain();
            match audio::CaptureSession::start(state.buffer.clone(), state.diagnostics.clone()) {
                Ok(session) => {
                    *state.capture.lock().unwrap() = Some(session);
                    set_pipeline_state(app, tray::PipelineState::Recording);
                }
                Err(err) => {
                    // Issue #59: surfaced as structured pipeline state, not
                    // an invisible eprintln! — a packaged app's tray can
                    // reflect this via `tray::tray_icon_state`.
                    eprintln!("bla: failed to start audio capture: {err}");
                    set_pipeline_state(app, tray::PipelineState::Error);
                }
            }
        }
        Some(hotkeys::Transition::StopRecording) => {
            if let Some(session) = state.capture.lock().unwrap().take() {
                session.stop();
            }
            let samples = state.buffer.lock().unwrap().drain();
            set_pipeline_state(app, tray::PipelineState::Transcribing);
            run_pipeline_in_background(app.clone(), samples);
        }
        Some(hotkeys::Transition::Cancelled) => {
            if let Some(session) = state.capture.lock().unwrap().take() {
                session.stop();
            }
            state.buffer.lock().unwrap().drain();
            set_pipeline_state(app, tray::PipelineState::Idle);
        }
        None => {}
    }
}

fn set_pipeline_state(app: &tauri::AppHandle, new_state: tray::PipelineState) {
    let state = app.state::<AppState>();
    *state.pipeline_state.lock().unwrap() = new_state;
    let icon_state = tray::tray_icon_state(&new_state);
    // Real tray icon asset rendering (TrayIconBuilder) is thin OS glue left
    // as a follow-up (no icon assets are part of this increment); this
    // emits the derived state so any UI/tray consumer can react to it.
    let _ = app.emit("pipeline-state-changed", format!("{icon_state:?}"));
}

/// Selects the real `WhisperStt` engine under `--features whisper`,
/// resolving the model path from `settings`/`app_data_dir` via `models`'s
/// already-tested registry lookup (native glue, TDD-exempt per `stt.rs`'s
/// own module doc).
#[cfg(feature = "whisper")]
fn build_stt(
    settings: &settings::Settings,
    app_data_dir: &std::path::Path,
) -> Result<stt::WhisperStt, String> {
    let spec = spec_for_preset(to_models_preset(settings.model_preset));
    let model_path = models::model_target_path(app_data_dir, &spec);
    stt::WhisperStt::new(&model_path).map_err(|e| e.to_string())
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

        match build_stt(&settings, &app_data_dir) {
            Ok(stt_engine) => {
                let pipeline = pipeline::Pipeline::new(
                    stt_engine,
                    cleanup,
                    output::SystemClipboard,
                    output::EnigoPaste,
                    std::thread::sleep,
                );
                match pipeline.run(&samples, &opts) {
                    Ok(_outcome) => set_pipeline_state(&app, tray::PipelineState::Idle),
                    Err(err) => {
                        eprintln!("bla: pipeline run failed: {err}");
                        set_pipeline_state(&app, tray::PipelineState::Error);
                    }
                }
            }
            Err(msg) => {
                eprintln!("bla: {msg}");
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

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            greet,
            commands::get_settings,
            commands::set_settings,
            commands::set_output_mode,
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
                settings: Mutex::new(settings.clone()),
                output_switch: Mutex::new(tray::OutputModeSwitch::new(to_tray_output_mode(
                    settings.output_mode,
                ))),
                pipeline_state: Mutex::new(tray::PipelineState::Idle),
            };
            app.manage(state);

            register_hotkey(&handle, &settings.hotkey)?;

            // Issue #44: reconcile a possibly-dropped KeyUp on window
            // focus-loss so the machine can never wedge in Holding.
            if let Some(window) = app.get_webview_window("main") {
                let focus_handle = handle.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::Focused(false) = event {
                        reconcile_hotkeys_on_focus_loss(&focus_handle);
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
            {
                let spec = spec_for_preset(to_models_preset(settings.model_preset));
                let target = models::model_target_path(&app_data_dir, &spec);
                if !target.exists() {
                    let progress_handle = handle.clone();
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
                        if let Err(err) = result {
                            eprintln!("bla: first-run model download failed: {err}");
                            let _ = handle.emit("model-download-error", err.to_string());
                        }
                    });
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
