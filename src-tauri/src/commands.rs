//! Tauri IPC commands exposed to the UI (`#[tauri::command]` handlers).
//!
//! The UI talks to the core *only* through this module (docs/ARCHITECTURE.md
//! §Module Boundaries) — every command here is a thin wrapper delegating to
//! `settings`/`tray`/`lib.rs`'s wiring helpers, with `src/lib/ipc.ts` as the
//! typed mirror on the frontend side (not yet wired up — the settings
//! window itself is M2; these commands exist so that UI has something real
//! to call against as of this increment, issue #91).

use tauri::{AppHandle, Manager, State};

use crate::{
    register_hotkey, save_settings_to_store, spec_for_preset, to_models_preset,
    to_tray_output_mode, AppState,
};

/// Read the currently effective settings (in-memory, kept in sync with the
/// persisted store by [`set_settings`]).
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> crate::settings::Settings {
    state.settings.lock().unwrap().clone()
}

/// Replace the persisted + in-memory settings wholesale. Re-registers the
/// global hotkey if it changed and updates the live output-mode switch
/// (AC-14: the switch only affects dictations completing from this point
/// forward, never one already in flight).
#[tauri::command]
pub fn set_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: crate::settings::Settings,
) -> Result<(), String> {
    let hotkey_changed = {
        let current = state.settings.lock().unwrap();
        current.hotkey != settings.hotkey
    };

    // Issue #91 (Sentinel 🔴): validate + register the new hotkey BEFORE
    // persisting anything. A malformed/unregistrable hotkey is rejected at
    // the IPC boundary (returns Err to the caller) and NEVER written to
    // settings.json — a persisted bad hotkey would brick the next launch.
    // `validate_hotkey` is the pure, unit-tested parse; `register_hotkey`
    // uses the same parser, so a value that validates is the value that
    // registers. Persisting happens only after both succeed.
    if hotkey_changed {
        crate::hotkeys::validate_hotkey(&settings.hotkey)?;
        register_hotkey(&app, &settings.hotkey).map_err(|e| e.to_string())?;
    }

    save_settings_to_store(&app, &settings)?;

    state
        .output_switch
        .lock()
        .unwrap()
        .set_mode(to_tray_output_mode(settings.output_mode));
    *state.settings.lock().unwrap() = settings;
    Ok(())
}

/// Switch the live output-mode target (AC-14) without otherwise touching
/// settings. `set_settings` also updates this as part of a full settings
/// save; this is the lightweight path for a bare tray-menu toggle.
#[tauri::command]
pub fn set_output_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    mode: crate::settings::OutputModeSetting,
) -> Result<(), String> {
    state
        .output_switch
        .lock()
        .unwrap()
        .set_mode(to_tray_output_mode(mode));

    let mut settings = state.settings.lock().unwrap();
    settings.output_mode = mode;
    save_settings_to_store(&app, &settings)
}

/// Kicks the first-run Whisper model downloader for the currently selected
/// preset (issue #91 Part B minimal wiring; full onboarding UX is M5).
/// Returns immediately with `"already-present"` if the model file already
/// exists, or `"downloading"` once the background download has started;
/// progress is reported via the `model-download-progress` event
/// (`models::DownloadProgress`) and a terminal `model-download-error` event
/// on failure.
#[tauri::command]
pub fn download_selected_model(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let settings = state.settings.lock().unwrap().clone();
    let app_data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let spec = spec_for_preset(to_models_preset(settings.model_preset));
    let target = crate::models::model_target_path(&app_data_dir, &spec);

    if target.exists() {
        return Ok("already-present".to_string());
    }

    let progress_handle = app.clone();
    std::thread::spawn(move || {
        use tauri::Emitter;
        let transport = crate::models::UreqTransport::new();
        let result = crate::models::download_model_with_spec(&transport, &spec, &app_data_dir, {
            let progress_handle = progress_handle.clone();
            move |progress| {
                let _ = progress_handle.emit("model-download-progress", progress);
            }
        });
        if let Err(err) = result {
            let _ = progress_handle.emit("model-download-error", err.to_string());
        }
    });

    Ok("downloading".to_string())
}
