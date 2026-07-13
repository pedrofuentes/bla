//! Tauri IPC commands exposed to the UI (`#[tauri::command]` handlers).
//!
//! The UI talks to the core *only* through this module (docs/ARCHITECTURE.md
//! §Module Boundaries) — every command here is a thin wrapper delegating to
//! `settings`/`tray`/`lib.rs`'s wiring helpers, with `src/lib/ipc.ts` as the
//! typed mirror on the frontend side (not yet wired up — the settings
//! window itself is M2; these commands exist so that UI has something real
//! to call against as of this increment, issue #91).

use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    apply_settings_to_state, output_mode_toggle_label, react_to_transition, register_hotkey,
    save_settings_to_store, spec_for_preset, to_models_preset, to_tray_output_mode, AppState,
};

/// Read the currently effective settings (in-memory, kept in sync with the
/// persisted store by [`set_settings`]).
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> crate::settings::Settings {
    state.settings.lock().unwrap().clone()
}

/// Replace the persisted + in-memory settings wholesale. Re-registers the
/// global hotkey if it changed, flips the live hotkeys state machine's
/// recording mode (issue #126 / PR #134 Sentinel 🔴-3 — a saved Hold↔Toggle
/// change takes effect immediately, not after a restart; a dictation in
/// flight across the mode change is cancelled and discarded), and updates
/// the live output-mode switch (AC-14: the switch only affects dictations
/// completing from this point forward, never one already in flight).
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

    // Unit-tested in lib.rs::apply_settings_tests; a mode change that
    // interrupts an in-flight session yields Cancelled, which
    // react_to_transition turns into stop-capture + discard-audio (the same
    // handling as the debounce/focus-loss cancel paths).
    let transition = apply_settings_to_state(&state, settings);
    react_to_transition(&app, transition);
    Ok(())
}

/// Switch the live output-mode target (AC-14) without otherwise touching
/// settings. `set_settings` also updates this as part of a full settings
/// save; this is the lightweight path both the status window's toggle
/// button and the tray menu's Cursor/File item call (issue #110) — the
/// single shared path keeps `tray::OutputModeSwitch`, persisted `Settings`,
/// and the tray menu's own label all in agreement regardless of which
/// trigger fired.
#[tauri::command]
pub fn set_output_mode(
    app: AppHandle,
    state: State<'_, AppState>,
    mode: crate::settings::OutputModeSetting,
) -> Result<(), String> {
    let tray_mode = to_tray_output_mode(mode);
    state.output_switch.lock().unwrap().set_mode(tray_mode);

    {
        let mut settings = state.settings.lock().unwrap();
        settings.output_mode = mode;
        save_settings_to_store(&app, &settings)?;
    }

    // Issue #110: best-effort — the tray may not have finished building yet
    // (or this build has no tray item at all in a future headless context),
    // and a failure to relabel the menu must never fail the mode switch
    // itself, which has already been persisted above.
    if let Some(item) = state.tray_output_toggle_item.lock().unwrap().as_ref() {
        let _ = item.set_text(output_mode_toggle_label(tray_mode));
    }

    // Issue #110: this command is called from BOTH the status window's
    // toggle button and the tray menu's item. A tray-triggered switch never
    // runs the window's React handler, so without this the window's state
    // goes stale (it and the tray would disagree about the live mode).
    // Emit the new mode so the window reconciles regardless of which trigger
    // fired — the brief required window and tray always agree.
    let _ = app.emit("output-mode-changed", mode);

    Ok(())
}

/// Validates a candidate hotkey accelerator string without persisting
/// anything (issue #126, M2 PR 2.5). Thin wrapper over the pure
/// `hotkeys::validate_hotkey` — same parser `set_settings` and OS-glue
/// registration use, so a value that validates here is exactly the value
/// that will register. Lets the settings window's hotkey capture field show
/// an inline error immediately after a chord is captured, before the user
/// ever clicks Save (`set_settings` still re-validates independently per
/// issue #91's validate-before-persist invariant — this command doesn't
/// change that ordering, it just gives the UI an earlier signal).
#[tauri::command]
pub fn validate_hotkey(accelerator: String) -> Result<(), String> {
    crate::hotkeys::validate_hotkey(&accelerator)
}

/// Kicks the first-run Whisper model downloader for the currently selected
/// preset (issue #91 Part B minimal wiring; full onboarding UX is M5).
/// Returns immediately with `"already-present"` if the model file already
/// exists, or `"downloading"` once the background download has started;
/// progress is reported via the `model-download-progress` event
/// (`models::DownloadProgress`), with a terminal `model-download-complete`
/// event on success or `model-download-error` on failure.
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
        let transport = crate::models::UreqTransport::new();
        let result = crate::models::download_model_with_spec(&transport, &spec, &app_data_dir, {
            let progress_handle = progress_handle.clone();
            move |progress| {
                let _ = progress_handle.emit("model-download-progress", progress);
            }
        });
        match result {
            // Issue #110: signal completion so the UI leaves the
            // "Downloading… 100%" state and shows Ready (mirrors the
            // first-run path in lib.rs::run()'s setup()).
            Ok(_) => {
                let _ = progress_handle.emit("model-download-complete", ());
            }
            Err(err) => {
                let _ = progress_handle.emit("model-download-error", err.to_string());
            }
        }
    });

    Ok("downloading".to_string())
}
