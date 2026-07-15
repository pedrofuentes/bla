//! Tauri IPC commands exposed to the UI (`#[tauri::command]` handlers).
//!
//! The UI talks to the core *only* through this module (docs/ARCHITECTURE.md
//! §Module Boundaries) — every command here is a thin wrapper delegating to
//! `settings`/`tray`/`lib.rs`'s wiring helpers, with `src/lib/ipc.ts` as the
//! typed mirror on the frontend side (not yet wired up — the settings
//! window itself is M2; these commands exist so that UI has something real
//! to call against as of this increment, issue #91).

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::{
    apply_settings_to_state, is_settings_window, model_registry_entries, output_mode_toggle_label,
    react_to_transition, register_hotkey, save_settings_to_store, set_settings_with_rollback,
    should_resume_hotkey, spec_for_preset, to_models_preset, to_tray_output_mode,
    unregister_hotkey, AppState, ModelRegistryEntry,
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
/// flight across the mode change is cancelled and discarded), updates the
/// live output-mode switch (AC-14: the switch only affects dictations
/// completing from this point forward, never one already in flight), and
/// enables/disables OS login autostart when `launch_at_login` flipped
/// (issue #126, M2 PR 2.6).
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
    // Issue #126: the pure decision of whether/which direction this save's
    // launch_at_login change should flip OS autostart registration —
    // unit-tested in `settings::autostart_action_for_change`. `None` on the
    // common case of a save that doesn't touch this field.
    let autostart_action = {
        let current = state.settings.lock().unwrap();
        crate::settings::autostart_action_for_change(
            current.launch_at_login,
            settings.launch_at_login,
        )
    };

    // Issue #91 (Sentinel 🔴) — register-before-persist WITH rollback. A
    // changed hotkey is validated AND actually bound to the OS BEFORE anything
    // is persisted: a chord that parses but the OS refuses to register
    // (already claimed by another app, or OS-reserved — a real failure mode on
    // Windows; macOS's registrar returns Ok) is rejected here and NEVER
    // written to settings.json, so it can't brick dictation across the next
    // launch.
    //
    // PR #185 cycle-5 (#187, cofounder decision): the hotkey field now uses an
    // explicit Apply button — capture (suspend/resume) fully ENDS and restores
    // the prior binding before Apply's `set_settings` runs, so this command is
    // the sole registrar of a persisted hotkey change and `hotkey_suspend_gen`
    // is owned entirely by suspend/resume for the capture window. `set_settings`
    // therefore does NOT touch the generation at all — dissolving the
    // two-writer TOCTOU that earlier cycles fought.
    //
    // Rollback keeps the OS binding and settings.json in agreement on failure.
    // The control flow (register-before-persist; roll the OS back to the prior
    // hotkey if EITHER the register or the persist fails) is the pure,
    // unit-tested `set_settings_with_rollback` seam (PR #185 cycle-6 🟡); this
    // command only injects the three OS effects. #91: validate BEFORE binding
    // so a malformed chord is rejected without touching the OS.
    let prior_hotkey = if hotkey_changed {
        state.settings.lock().unwrap().hotkey.clone()
    } else {
        String::new()
    };
    if hotkey_changed {
        crate::hotkeys::validate_hotkey(&settings.hotkey)?;
    }
    set_settings_with_rollback(
        hotkey_changed,
        &prior_hotkey,
        &settings.hotkey,
        |h| register_hotkey(&app, h).map_err(|e| e.to_string()),
        || save_settings_to_store(&app, &settings),
        |prior| {
            // PR #185 cycle-6 🟢: a failed RESTORE must not be invisible — the
            // OS could be left unbound. Surface it (per this file's eprintln
            // convention) instead of silently swallowing it.
            if let Err(err) = register_hotkey(&app, prior) {
                eprintln!(
                    "bla: failed to restore prior hotkey {prior:?} after a set_settings rollback; \
                     the global dictation shortcut may be unbound until restart: {err}"
                );
            }
        },
    )?;

    // Issue #126: thin OS glue over `tauri-plugin-autostart`'s
    // `AutoLaunchManager` — the decision of WHETHER to call this already
    // happened above (`autostart_action_for_change`). Best-effort and
    // non-fatal: settings are already persisted at this point, so a failure
    // to flip the OS-level login-item registration (e.g. a sandboxed/dev
    // environment without login-item permissions) must not fail the save
    // the user just made or leave `settings.json` and the OS registration
    // silently disagreeing about a value that DID persist.
    //
    // Dev-build note: in a `cargo tauri dev` / `cargo run` build this
    // registers the dev binary's path (e.g. `target/debug/bla`), not a
    // stable packaged-app path — expected and harmless for local
    // development; only a `tauri build` binary's login-item entry is what
    // ships to users.
    if let Some(action) = autostart_action {
        let manager = app.autolaunch();
        let result = match action {
            crate::settings::AutostartAction::Enable => manager.enable(),
            crate::settings::AutostartAction::Disable => manager.disable(),
        };
        if let Err(err) = result {
            eprintln!("bla: failed to update launch-at-login OS registration: {err}");
        }
    }

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

/// Every supported Whisper model preset's settings-safe id and exact
/// download size in bytes (issue #184), for the settings window's model
/// picker to render e.g. "Small — 488 MB" — thin wrapper over the pure,
/// unit-tested [`model_registry_entries`].
#[tauri::command]
pub fn model_registry() -> Vec<ModelRegistryEntry> {
    model_registry_entries()
}

/// Temporarily unregisters the global dictation hotkey (issue #181): called
/// when the settings window's hotkey-capture field gains focus, so the
/// still-live shortcut doesn't fire a dictation while the user is pressing
/// keys meant to be captured into the field instead. Paired with
/// [`resume_hotkey`], which the field calls on every way capture can end.
///
/// `generation` is a monotonic token minted by the calling window (PR #185
/// Sentinel 🔴-1(iii)): it's stored as the latest outstanding suspend so
/// [`resume_hotkey`] can reject a stale, out-of-order resume. Rejected
/// unless invoked from the settings window (🟡-4) — the commands are in the
/// global `invoke_handler`, so an unpaired suspend from any other webview
/// would otherwise DoS the recording trigger.
#[tauri::command]
pub fn suspend_hotkey(
    window: tauri::Window,
    state: State<'_, AppState>,
    generation: u64,
) -> Result<(), String> {
    if !is_settings_window(window.label()) {
        return Err("suspend_hotkey is only callable from the settings window".to_string());
    }
    unregister_hotkey(window.app_handle()).map_err(|e| e.to_string())?;
    *state.hotkey_suspend_gen.lock().unwrap() = generation;
    Ok(())
}

/// Re-registers the current (persisted) hotkey as the global dictation
/// shortcut (issue #181) — called whenever hotkey capture ends without a
/// newly-committed *changed* chord already re-registering it via
/// `set_settings` (Escape, blur mid-capture, an invalid chord, or a
/// committed chord equal to the current hotkey; see
/// `src/lib/hotkeyCapture.ts`'s `captureEndNeedsResume`). Reads the hotkey
/// from live state rather than taking one as an argument so a cancelled/
/// invalid capture always restores whatever was actually registered before
/// capture began, never a not-yet-persisted candidate.
///
/// Only re-registers when `generation` is still the latest outstanding
/// suspend ([`should_resume_hotkey`], PR #185 Sentinel 🔴-1(iii)) so an
/// out-of-order resume can't re-enable the shortcut during a newer capture;
/// clears the generation once it does, so a duplicate resume is a no-op.
/// Rejected unless invoked from the settings window (🟡-4).
#[tauri::command]
pub fn resume_hotkey(
    window: tauri::Window,
    state: State<'_, AppState>,
    generation: u64,
) -> Result<(), String> {
    if !is_settings_window(window.label()) {
        return Err("resume_hotkey is only callable from the settings window".to_string());
    }
    let hotkey = state.settings.lock().unwrap().hotkey.clone();
    let mut gen_slot = state.hotkey_suspend_gen.lock().unwrap();
    if should_resume_hotkey(*gen_slot, generation) {
        register_hotkey(window.app_handle(), &hotkey).map_err(|e| e.to_string())?;
        *gen_slot = 0;
    }
    Ok(())
}
