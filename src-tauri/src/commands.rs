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
    apply_settings_to_state, copy_history_entry_text, is_settings_window, model_registry_entries,
    now_ms, output_mode_toggle_label, prune_history_for_retention, react_to_command_transition,
    react_to_transition, register_command_hotkey, register_hotkey, save_settings_to_store,
    set_two_hotkeys_with_rollback, should_resume_hotkey, spec_for_preset, to_models_preset,
    to_tray_output_mode, unregister_hotkey, AppState, ModelRegistryEntry,
};

/// Read the currently effective settings (in-memory, kept in sync with the
/// persisted store by [`set_settings`]).
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> crate::settings::Settings {
    state.settings.lock().unwrap().clone()
}

/// Issue #246 (Sentinel SNTL-20260716-bla-PR245-6936364 🟡 on PR #245):
/// exposes the RUNTIME platform this Tauri binary is running on to the
/// frontend, so `validateBaseDir` (`src/lib/baseDir.ts`) can reject a
/// foreign-platform absolute base-folder form (e.g. a synced
/// `settings.json`'s `C:\...` on macOS, or a bare `/foo` on Windows — which
/// `Path::is_absolute` treats as drive-relative there, NOT absolute)
/// instead of accepting either platform's syntax regardless of what
/// `output::resolve_base_dir` (which runs Rust-side and uses
/// `std::path::Path`'s per-platform absoluteness rule) will actually do
/// with the value. No args, so it's outside the #239 wire-key
/// `rename_all = "snake_case"` guard, which only bites multi-word
/// snake_case *argument* names.
#[tauri::command]
pub fn get_platform() -> &'static str {
    runtime_platform()
}

/// Pure decision behind [`get_platform`], extracted so it's callable from a
/// unit test without an `AppHandle`/`Wry` runtime (#165's pattern). Mirrors
/// exactly the two branches `std::path::Path::is_absolute` uses: `"windows"`
/// (drive-letter prefix or UNC root) vs. every other target bla ships on
/// (`"unix"` — leading `/`). Tauri never cross-compiles at runtime, so this
/// is a pure compile-time `cfg!(windows)` branch, not an OS syscall.
fn runtime_platform() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else {
        "unix"
    }
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
    // Issue #259 (AC-49): the command-mode hotkey change-detection mirrors
    // the dictation one exactly, one field over.
    let command_hotkey_changed = {
        let current = state.settings.lock().unwrap();
        current.command_hotkey != settings.command_hotkey
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
    // Issue #259 (AC-49): extended to a SECOND independent hotkey slot
    // (command mode), sharing one persisted `Settings` blob AND one OS
    // accelerator registry with the dictation slot.
    //
    // Issue #259 Sentinel 🔴-3 (SNTL-20260716-bla-PR274-2b757bf): the two
    // slots are registered via `set_two_hotkeys_with_rollback`, NOT two
    // independent per-slot register-then-persist calls — the OS's global
    // shortcut registry keys purely by accelerator, so handling the slots
    // fully independently breaks for a swap-style save (new-dictation ==
    // current-command's still-live value, or vice versa): registering the
    // first slot's new value can hit "already registered" because the
    // other slot's still-current binding hasn't been freed yet, and a
    // naive per-slot rollback has no reason to touch that other slot,
    // leaving it dead until restart with settings.json disagreeing with
    // the OS. `set_two_hotkeys_with_rollback` unregisters BOTH changed
    // priors before registering EITHER new value, so this can't happen —
    // see its doc comment for the full rationale.
    let prior_hotkey = if hotkey_changed {
        state.settings.lock().unwrap().hotkey.clone()
    } else {
        String::new()
    };
    let prior_command_hotkey = if command_hotkey_changed {
        state.settings.lock().unwrap().command_hotkey.clone()
    } else {
        String::new()
    };
    validate_settings_hotkeys(
        hotkey_changed,
        &settings.hotkey,
        command_hotkey_changed,
        &settings.command_hotkey,
    )?;

    set_two_hotkeys_with_rollback(
        hotkey_changed,
        &prior_hotkey,
        &settings.hotkey,
        command_hotkey_changed,
        &prior_command_hotkey,
        &settings.command_hotkey,
        |h| {
            // Best-effort, same convention as `register_hotkey`'s own
            // internal targeted-unregister-of-prior: `h` may already be
            // unregistered (e.g. nothing was ever bound to it), and that
            // must never block registering the OTHER slot's new value.
            let _ = unregister_hotkey(&app, h);
        },
        |h| register_hotkey(&app, None, h).map_err(|e| e.to_string()),
        |h| register_command_hotkey(&app, None, h).map_err(|e| e.to_string()),
        || save_settings_to_store(&app, &settings),
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

    // Issue #198 (AC-31): captured before `apply_settings_to_state` moves
    // `settings` below — prune runs AFTER settings are durably persisted
    // (register-before-persist already succeeded above), so a lowered
    // retention window takes effect on this same save rather than waiting
    // for the next startup.
    let retention_days = settings.retention_days;

    // Unit-tested in lib.rs::apply_settings_tests; a mode change that
    // interrupts an in-flight session yields Cancelled, which
    // react_to_transition turns into stop-capture + discard-audio (the same
    // handling as the debounce/focus-loss cancel paths). Issue #259:
    // `apply_settings_to_state` now flips BOTH hotkey state machines'
    // Hold/Toggle mode (they mirror the same `recording_mode`) — the
    // command-mode transition is handled by its own
    // `react_to_command_transition`, mirroring the dictation call
    // immediately above it.
    let (transition, command_transition) = apply_settings_to_state(&state, settings);
    react_to_transition(&app, transition);
    react_to_command_transition(&app, command_transition);

    // Issue #198 (AC-31): best-effort, same as the autostart glue above —
    // settings are already persisted at this point, so a prune failure must
    // never fail the save the user just made.
    {
        let store = state.store.lock().unwrap();
        if let Err(err) = prune_history_for_retention(&store, now_ms(), retention_days) {
            eprintln!("bla: failed to prune history after a settings save: {err}");
        }
    }

    Ok(())
}

/// The exact validate-before-persist gating [`set_settings`] runs for the
/// two hotkey fields, extracted into a pure function so it's unit-testable
/// without an `AppHandle`/`State<AppState>` (#165's Windows-CI rule —
/// `set_settings` itself can't be constructed in a `#[cfg(test)]` without a
/// `tauri::Wry` runtime). `set_settings` calls this directly (see its body)
/// rather than reimplementing the same checks inline — there is exactly one
/// copy of this gating, so a test against this function IS a test of what
/// `set_settings` actually runs, not a parallel reimplementation that could
/// silently drift from it.
///
/// Order matters and is preserved exactly as `set_settings` had it inline
/// before this extraction (Sentinel review on PR #293, closing a coverage
/// gap: commit 9ac99d1 wired `hotkeys::validate_command_hotkey_keyset` into
/// `set_settings` with no Rust test exercising the integration — only a
/// frontend test mocking `invoke`):
/// 1. `hotkey_changed` -> `validate_hotkey(hotkey)` (issue #91).
/// 2. `command_hotkey_changed` -> `validate_hotkey(command_hotkey)` THEN
///    `validate_command_hotkey_keyset(command_hotkey)` (issue #281,
///    ac7-p0) — gated on `command_hotkey_changed` specifically so an
///    unrelated settings save (e.g. toggling sound cues) never re-rejects
///    an already-persisted legacy value, such as the pre-#281 shipped
///    default `"Control+Shift+C"`, that a user never touched. This fix
///    prevents a NEW bad value from being saved; it doesn't retroactively
///    invalidate an old one already in `settings.json` (see the #292
///    follow-up for that gap).
/// 3. `distinct_hotkeys(hotkey, command_hotkey)` unconditionally (AC-49) —
///    by this point both hotkeys that changed have already parsed, so this
///    only ever rejects a same-accelerator collision.
fn validate_settings_hotkeys(
    hotkey_changed: bool,
    hotkey: &str,
    command_hotkey_changed: bool,
    command_hotkey: &str,
) -> Result<(), String> {
    if hotkey_changed {
        crate::hotkeys::validate_hotkey(hotkey)?;
    }
    if command_hotkey_changed {
        crate::hotkeys::validate_hotkey(command_hotkey)?;
        crate::hotkeys::validate_command_hotkey_keyset(command_hotkey)?;
    }
    crate::hotkeys::distinct_hotkeys(hotkey, command_hotkey)
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

/// Validates a candidate COMMAND-MODE hotkey accelerator (issue #281,
/// ac7-p0): the same general-grammar probe as [`validate_hotkey`], PLUS the
/// function-key-trigger keyset constraint (`hotkeys::validate_command_hotkey_keyset`)
/// that's specific to the command-mode slot — see that function's doc for
/// why. Deliberately a SEPARATE command from `validate_hotkey` rather than a
/// shared one with a mode flag: the dictation hotkey field must keep using
/// the unconstrained probe (this PR intentionally does not change dictation-
/// hotkey validation — see the #281 follow-up issue for that discussion),
/// so the two fields' live picker-time checks stay wired to genuinely
/// different validators, matching `set_settings`'s own asymmetric handling
/// of the two slots one field over.
#[tauri::command]
pub fn validate_command_hotkey(accelerator: String) -> Result<(), String> {
    crate::hotkeys::validate_hotkey(&accelerator)?;
    crate::hotkeys::validate_command_hotkey_keyset(&accelerator)
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
    // Issue #259: target-unregister exactly the dictation hotkey (never
    // `unregister_all()`, which would also drop the independently-
    // registered command-mode hotkey — see `unregister_hotkey`'s doc).
    let hotkey = state.settings.lock().unwrap().hotkey.clone();
    unregister_hotkey(window.app_handle(), &hotkey).map_err(|e| e.to_string())?;
    *state.hotkey_suspend_gen.lock().unwrap() = generation;
    Ok(())
}

/// Substring search over dictation history (AC-30, issue #198), newest
/// first, capped at `limit` rows. Thin wrapper over
/// `store::Store::search_history`; `HistoryRow` derives `Serialize` (see its
/// doc comment) specifically so this command can hand rows to the frontend
/// over Tauri IPC — the sanctioned "leaves local SQLite" path for history
/// text (the user's own History tab, #199).
#[tauri::command]
pub fn search_history(
    state: State<'_, AppState>,
    query: String,
    limit: usize,
) -> Result<Vec<crate::store::HistoryRow>, String> {
    state
        .store
        .lock()
        .unwrap()
        .search_history(&query, limit)
        .map_err(|e| e.to_string())
}

/// Copy one history entry's cleaned transcript to the clipboard (AC-30,
/// issue #198). Thin `AppState`-shaped wrapper over the pure
/// `copy_history_entry_text`, which does the actual `Store::get_history` +
/// `output::Clipboard`/`ClipboardPayload` handoff — that function is what's
/// unit-tested (see `history_wiring_tests` in `lib.rs`) against
/// `Store::open_in_memory()` and a fake `Clipboard`, never a constructed
/// `AppState`.
#[tauri::command]
pub fn copy_history_entry(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let store = state.store.lock().unwrap();
    copy_history_entry_text(&store, &crate::output::SystemClipboard, id)
}

/// Delete a single history entry by id (AC-30, issue #198). Thin wrapper
/// over `store::Store::delete_history` — deleting an id that doesn't exist
/// is a no-op, not an error (matches `Store::delete_history`'s own
/// contract).
#[tauri::command]
pub fn delete_history_entry(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state
        .store
        .lock()
        .unwrap()
        .delete_history(id)
        .map_err(|e| e.to_string())
}

/// Delete every history entry (AC-30, issue #198). Thin wrapper over
/// `store::Store::clear_history` — the History tab's (#199) "Clear all"
/// action.
#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    state
        .store
        .lock()
        .unwrap()
        .clear_history()
        .map_err(|e| e.to_string())
}

/// List every personal-dictionary term (issue #200, PRD AC-21),
/// most-recently-added first. Thin wrapper over `store::Store::list_terms`;
/// `DictionaryTerm` derives `Serialize` (see its doc comment) specifically
/// so this command can hand rows to the frontend over Tauri IPC — the
/// user's own Dictionary tab (#201).
#[tauri::command]
pub fn list_dictionary_terms(
    state: State<'_, AppState>,
) -> Result<Vec<crate::store::DictionaryTerm>, String> {
    state
        .store
        .lock()
        .unwrap()
        .list_terms()
        .map_err(|e| e.to_string())
}

/// Add a term to the personal dictionary (issue #200, PRD AC-21). Thin
/// wrapper over `store::Store::add_term` — case-insensitively unique, so
/// adding a term that already exists under a different case is a no-op,
/// not an error (matches `Store::add_term`'s own contract). Returns the
/// term's row id either way.
#[tauri::command]
pub fn add_dictionary_term(state: State<'_, AppState>, term: String) -> Result<i64, String> {
    state
        .store
        .lock()
        .unwrap()
        .add_term(&term, now_ms())
        .map_err(|e| e.to_string())
}

/// Remove a single dictionary term by id (issue #200, PRD AC-21). Thin
/// wrapper over `store::Store::remove_term` — removing an id that doesn't
/// exist is a no-op, not an error (matches `Store::remove_term`'s own
/// contract).
#[tauri::command]
pub fn remove_dictionary_term(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state
        .store
        .lock()
        .unwrap()
        .remove_term(id)
        .map_err(|e| e.to_string())
}

/// List every per-app tone rule (issue #202, PRD AC-22), in insertion
/// order. Thin wrapper over `store::Store::list_tone_rules`; `ToneRule`
/// derives `Serialize` (see its doc comment) specifically so this command
/// can hand rows to the frontend over Tauri IPC — the Tone tab (#203, not
/// this PR).
#[tauri::command]
pub fn list_tone_rules(state: State<'_, AppState>) -> Result<Vec<crate::store::ToneRule>, String> {
    state
        .store
        .lock()
        .unwrap()
        .list_tone_rules()
        .map_err(|e| e.to_string())
}

/// Insert or update a per-app tone rule (issue #202, PRD AC-22, AC-41).
/// Thin wrapper over `store::Store::upsert_tone_rule` — re-submitting the
/// same `app_pattern` (case-insensitively) UPDATES that rule's tone in
/// place rather than adding a second row, so an edited rule takes effect on
/// the very next dictation with no restart required (the next
/// `run_pipeline_in_background` call reads `list_tone_rules` fresh; there
/// is no cache to invalidate). Returns the rule's row id either way.
///
/// `rename_all = "snake_case"` (SNTL-20260715-bla-PR237-18ff735 🔴): every
/// other command in this file only has single-word (or already-camelCase-
/// identical) argument names, so tauri-macros' default camelCase-on-the-
/// wire behavior never surfaced — this is the first multi-word snake_case
/// arg name (`app_pattern`) in the file. Without this attribute the wire
/// expects `appPattern`, but the frontend (`src/lib/ipc.ts`/`ToneTab.tsx`,
/// #203) sends `app_pattern` — every add/edit call would reject in the real
/// app despite passing entirely mocked-IPC Vitest coverage. Keeps this
/// crate's snake_case-on-wire convention explicit rather than relying on
/// having no multi-word arg name to accidentally dodge the mismatch.
#[tauri::command(rename_all = "snake_case")]
pub fn upsert_tone_rule(
    state: State<'_, AppState>,
    app_pattern: String,
    tone: crate::store::ToneProfile,
) -> Result<i64, String> {
    state
        .store
        .lock()
        .unwrap()
        .upsert_tone_rule(&app_pattern, tone, now_ms())
        .map_err(|e| e.to_string())
}

/// Remove a single tone rule by id (issue #202, PRD AC-22). Thin wrapper
/// over `store::Store::delete_tone_rule` — removing an id that doesn't
/// exist is a no-op, not an error (matches `Store::remove_term`'s own
/// contract).
#[tauri::command]
pub fn delete_tone_rule(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state
        .store
        .lock()
        .unwrap()
        .delete_tone_rule(id)
        .map_err(|e| e.to_string())
}

/// List every stored snippet (issue #258/#261, AC-51/AC-54, part of #242's
/// M4 scope), most-recently-added first — mirrors `Store::list_snippets`'s
/// own ordering contract. Thin wrapper over `store::Store::list_snippets`;
/// `Snippet` derives `Serialize` (see its doc comment) specifically so this
/// command can hand rows to the frontend over Tauri IPC — the Snippets tab
/// (`SnippetsTab.tsx`, #261).
#[tauri::command]
pub fn list_snippets(state: State<'_, AppState>) -> Result<Vec<crate::store::Snippet>, String> {
    state
        .store
        .lock()
        .unwrap()
        .list_snippets()
        .map_err(|e| e.to_string())
}

/// Add a snippet (issue #258/#261, AC-51/AC-54). Thin wrapper over
/// `store::Store::add_snippet` — case-insensitively unique on `trigger`, so
/// adding a trigger that already exists under a different case is a no-op,
/// not an error (matches `Store::add_snippet`'s own contract, mirroring
/// `add_dictionary_term`'s — see `SnippetsTab.tsx`'s client-side duplicate
/// check for why the tab never relies on this call rejecting). Returns the
/// snippet's row id either way.
///
/// No multi-word argument names here (`trigger`, `body` are both single
/// words), so the #239 `rename_all = "snake_case"` wire-key rule doesn't
/// bite — see `upsert_tone_rule`'s doc comment for the case where it does;
/// `wire_key_contract_tests` (below) re-parses this file at test time and
/// would fail loudly if that ever changed without the attribute.
#[tauri::command]
pub fn add_snippet(
    state: State<'_, AppState>,
    trigger: String,
    body: String,
) -> Result<i64, String> {
    state
        .store
        .lock()
        .unwrap()
        .add_snippet(&trigger, &body, now_ms())
        .map_err(|e| e.to_string())
}

/// Update an existing snippet's trigger/body in place by id (issue #258/
/// #261, AC-51/AC-54). Thin wrapper over `store::Store::update_snippet` —
/// updating an id that doesn't exist is a no-op, not an error; UNLIKE
/// `add_snippet` (and unlike `upsert_tone_rule`), a new `trigger` that
/// collides case-insensitively with a DIFFERENT existing row's trigger
/// genuinely rejects with an `Err` here (the schema's `UNIQUE COLLATE
/// NOCASE` constraint is enforced on UPDATE too — see
/// `Store::update_snippet`'s own doc comment), which `SnippetsTab.tsx`
/// surfaces as a row-scoped, kind-only inline error rather than a silent
/// no-op.
#[tauri::command]
pub fn update_snippet(
    state: State<'_, AppState>,
    id: i64,
    trigger: String,
    body: String,
) -> Result<(), String> {
    state
        .store
        .lock()
        .unwrap()
        .update_snippet(id, &trigger, &body)
        .map_err(|e| e.to_string())
}

/// Remove a single snippet by id (issue #258/#261, AC-51/AC-54). Thin
/// wrapper over `store::Store::remove_snippet` — removing an id that
/// doesn't exist is a no-op, not an error (matches `Store::remove_term`'s/
/// `Store::delete_tone_rule`'s own contract).
#[tauri::command]
pub fn remove_snippet(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state
        .store
        .lock()
        .unwrap()
        .remove_snippet(id)
        .map_err(|e| e.to_string())
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
        // `None` prior: `suspend_hotkey` already target-unregistered this
        // exact hotkey (issue #259), so there's nothing left registered
        // under this slot to unregister again here.
        register_hotkey(window.app_handle(), None, &hotkey).map_err(|e| e.to_string())?;
        *gen_slot = 0;
    }
    Ok(())
}

// -------------------------------------------------------------------------
// Issue #281 (ac7-p0), Sentinel review on PR #293: closes a coverage gap —
// commit 9ac99d1 wired `hotkeys::validate_command_hotkey_keyset` into
// `set_settings` (via `validate_settings_hotkeys` above) and added the new
// `validate_command_hotkey` command, but neither had a Rust test exercising
// the actual backend integration; the only coverage was a frontend test
// mocking `invoke`, which proves nothing about the real Rust wiring — a
// future refactor of the `command_hotkey_changed` gating could silently
// break enforcement with nothing here to catch it.
// -------------------------------------------------------------------------
#[cfg(test)]
mod validate_settings_hotkeys_tests {
    use super::validate_settings_hotkeys;

    #[test]
    fn rejects_a_changed_command_hotkey_with_a_non_function_key_trigger_issue_281() {
        let err = validate_settings_hotkeys(
            false,
            "Control+Shift+Space",
            true,
            "Control+Shift+C", // letter trigger — the #281 harm class
        )
        .expect_err("a letter-key command_hotkey must be rejected when changed");
        assert!(
            err.to_lowercase().contains("function key"),
            "expected a clear function-key explanation, got {err:?}"
        );
    }

    #[test]
    fn accepts_a_changed_command_hotkey_with_a_function_key_trigger_issue_281() {
        assert!(
            validate_settings_hotkeys(false, "Control+Shift+Space", true, "Control+Shift+F13",)
                .is_ok()
        );
    }

    #[test]
    fn does_not_reject_an_unchanged_non_function_key_command_hotkey_issue_281() {
        // Gating check: `command_hotkey_changed = false` must skip the
        // keyset enforcement entirely, so an unrelated settings save never
        // re-rejects an already-persisted legacy value (e.g. the pre-#281
        // shipped default `"Control+Shift+C"`) that a user never touched —
        // this fix prevents a NEW bad value from being saved, it doesn't
        // retroactively invalidate an old one already in settings.json.
        assert!(
            validate_settings_hotkeys(false, "Control+Shift+Space", false, "Control+Shift+C",)
                .is_ok()
        );
    }
}

#[cfg(test)]
mod validate_command_hotkey_command_tests {
    use super::validate_command_hotkey;

    #[test]
    fn accepts_a_function_key_chord_issue_281() {
        assert!(validate_command_hotkey("Control+Shift+F13".to_string()).is_ok());
    }

    #[test]
    fn rejects_a_character_key_chord_issue_281() {
        let err = validate_command_hotkey("Control+Shift+C".to_string())
            .expect_err("a letter-key chord must be rejected");
        assert!(
            err.to_lowercase().contains("function key"),
            "expected a clear function-key explanation, got {err:?}"
        );
    }

    #[test]
    fn propagates_a_malformed_accelerators_parse_error() {
        assert!(validate_command_hotkey("NotARealKey".to_string()).is_err());
    }
}

// -------------------------------------------------------------------------
// Issue #239 (SNTL-20260716-bla-PR237-14507f3): class-level guard for the
// #237 🔴 (`upsert_tone_rule`'s `app_pattern` argument mismatched on the
// wire because tauri-macros defaults every `#[tauri::command]` arg to
// camelCase, and nothing exercised the real JS↔Rust wire contract — cargo
// tests call `Store` methods directly, and Vitest mocks `lib/ipc` wholesale,
// so a multi-word snake_case arg name silently missing `rename_all =
// "snake_case"` was structurally invisible until it broke in the real app).
//
// This isn't a true invoke-handler round-trip (Tauri's test harness for
// that is heavier than this module needs); it's a convention test: parse
// every `#[tauri::command...] pub fn NAME(...)` block out of this file's
// own source and assert that any block with a multi-word (contains `_`)
// argument name also carries `rename_all = "snake_case"` on its attribute.
// It will fail the moment a new command repeats the #237 mistake, without
// needing to know that command's name in advance.
// -------------------------------------------------------------------------
#[cfg(test)]
mod wire_key_contract_tests {
    /// This file's own source, parsed at test time rather than compile
    /// time — deliberately re-reading the same file the `#[tauri::command]`
    /// fns above live in, so the guard can never drift out of sync with
    /// what's actually there.
    const SRC: &str = include_str!("commands.rs");

    /// One parsed `#[tauri::command...]` block: the fn's name, whether its
    /// attribute carries an explicit `rename_all = "snake_case"`, and the
    /// raw text between the fn's parens.
    struct CommandBlock {
        name: String,
        has_snake_case_rename: bool,
        params_raw: String,
    }

    /// True when `idx` is the first non-whitespace position on its line —
    /// i.e. everything from the start of the line up to `idx` is
    /// whitespace. Used to tell a real `#[tauri::command]` attribute (which
    /// always starts its own line) apart from the marker string appearing
    /// mid-line inside a doc comment, a `//` comment, or a string literal —
    /// this test module's own source talks *about* `#[tauri::command]`
    /// prose-style in several places (see this fn's own doc comments and
    /// the `marker`/`.expect(...)` strings below), and a naive substring
    /// scan over `include_str!`-ed self-referential source would otherwise
    /// false-positive on every one of them.
    fn is_at_line_start(src: &str, idx: usize) -> bool {
        let line_start = src[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
        src[line_start..idx].chars().all(char::is_whitespace)
    }

    /// Scans `src` for every `#[tauri::command...]` attribute immediately
    /// followed by `pub fn NAME(...)`, in source order. Deliberately a
    /// simple linear scan rather than a real Rust parser (a `syn`
    /// dependency would be overkill for a test-only convention check) —
    /// robust enough for this file's actual shape (attribute directly
    /// above `pub fn`, no other attributes in between, no nested
    /// parentheses inside any command's parameter list) plus the
    /// line-start check above to skip this very test module's own
    /// prose/string mentions of the marker text.
    fn parse_command_blocks(src: &str) -> Vec<CommandBlock> {
        let marker = "#[tauri::command";
        let mut blocks = Vec::new();
        let mut search_from = 0usize;

        while let Some(rel) = src[search_from..].find(marker) {
            let attr_start = search_from + rel;
            if !is_at_line_start(src, attr_start) {
                // A mid-line mention (doc comment, `//` comment, or string
                // literal talking about the marker) — not a real attribute.
                // Skip past it and keep scanning.
                search_from = attr_start + marker.len();
                continue;
            }

            let attr_end = attr_start
                + src[attr_start..]
                    .find(']')
                    .expect("unterminated #[tauri::command attribute");
            let attr_text = &src[attr_start..=attr_end];
            let has_snake_case_rename =
                attr_text.contains("rename_all") && attr_text.contains("snake_case");

            let after_attr = &src[attr_end + 1..];
            let fn_kw = after_attr
                .find("fn ")
                .expect("no `fn` found after a #[tauri::command] attribute");
            let after_fn = &after_attr[fn_kw + "fn ".len()..];
            let paren_open = after_fn
                .find('(')
                .expect("no `(` found after a command fn's name");
            let name = after_fn[..paren_open].trim().to_string();
            let paren_close = after_fn[paren_open..].find(')').expect(
                "unterminated parameter list — a param type contains a nested `(`, \
                         which this simple scanner doesn't handle",
            );
            let params_raw = after_fn[paren_open + 1..paren_open + paren_close].to_string();

            blocks.push(CommandBlock {
                name,
                has_snake_case_rename,
                params_raw,
            });

            // Resume scanning strictly after this attribute — the next
            // `#[tauri::command` occurrence (if any) is necessarily after
            // it, so this can't re-match the same block or skip the next.
            search_from = attr_end + 1;
        }

        blocks
    }

    /// Splits a raw parameter list on top-level commas only (never inside
    /// `<...>` generic brackets, e.g. `State<'_, AppState>`'s internal
    /// comma), then extracts each parameter's name (the identifier before
    /// its `:`).
    fn param_names(params_raw: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut depth = 0i32;
        let mut current = String::new();

        for ch in params_raw.chars() {
            match ch {
                '<' => {
                    depth += 1;
                    current.push(ch);
                }
                '>' => {
                    depth -= 1;
                    current.push(ch);
                }
                ',' if depth == 0 => {
                    names.push(std::mem::take(&mut current));
                }
                _ => current.push(ch),
            }
        }
        if !current.trim().is_empty() {
            names.push(current);
        }

        names
            .into_iter()
            .filter_map(|param| param.split(':').next().map(|n| n.trim().to_string()))
            .filter(|n| !n.is_empty())
            .collect()
    }

    /// A "multi-word" arg name per #239's contract: snake_case with at
    /// least one underscore (e.g. `app_pattern`) — the class of name that
    /// tauri-macros' default camelCase-on-the-wire rewriting actually
    /// changes (`app_pattern` -> `appPattern`), unlike a single-word name
    /// (`state`, `id`, `term`) which is identical either way.
    fn is_multi_word_snake_case(name: &str) -> bool {
        name.contains('_')
    }

    #[test]
    fn parser_sanity_finds_every_known_command_including_the_pinned_snake_case_one() {
        // A guard on the guard: if the scanner regresses to finding zero or
        // too few blocks (e.g. a future edit changes the file's shape in a
        // way the simple scanner can't follow), fail loudly here rather
        // than the real assertion below silently vacuously passing over an
        // empty list.
        let blocks = parse_command_blocks(SRC);
        assert!(
            blocks.len() >= 15,
            "expected to find every #[tauri::command] fn in this file; found only {} — the \
             scanner may have regressed",
            blocks.len()
        );

        let upsert = blocks
            .iter()
            .find(|b| b.name == "upsert_tone_rule")
            .expect("parser did not find the known upsert_tone_rule command");
        assert!(
            upsert.has_snake_case_rename,
            "upsert_tone_rule must keep its rename_all = \"snake_case\" attribute (issue #237)"
        );
        assert!(
            param_names(&upsert.params_raw).contains(&"app_pattern".to_string()),
            "parser did not find upsert_tone_rule's app_pattern argument"
        );
    }

    #[test]
    fn every_multiword_arg_command_carries_rename_all_snake_case_issue_239() {
        let blocks = parse_command_blocks(SRC);

        let offenders: Vec<String> = blocks
            .iter()
            .filter(|b| {
                let has_multiword_arg = param_names(&b.params_raw)
                    .iter()
                    .any(|n| is_multi_word_snake_case(n));
                has_multiword_arg && !b.has_snake_case_rename
            })
            .map(|b| b.name.clone())
            .collect();

        assert!(
            offenders.is_empty(),
            "these #[tauri::command] fns take a multi-word snake_case argument but don't carry \
             `rename_all = \"snake_case\"` — tauri-macros' default camelCase-on-the-wire naming \
             will mismatch whatever the frontend actually sends (the #237 bug's class, issue \
             #239): {offenders:?}"
        );
    }
}

// -------------------------------------------------------------------------
// Issue #246: `get_platform`'s pure decision, exercised without a Wry
// runtime (#165's pattern). Deliberately NOT a `#[cfg(windows)]`-gated pair
// of tests (the Windows-CI rule this brief calls out) — the whole point of
// the fix is that `validateBaseDir` (`src/lib/baseDir.ts`) takes the
// platform as a parameter and is exercised for BOTH "windows" and "unix" in
// one Vitest run on any host OS; that's where the real two-branch coverage
// lives. This single assertion runs unconditionally on every CI OS and
// compares `runtime_platform()` against the same `cfg!(windows)` constant
// the function itself branches on, so it can never disagree with the
// function under test — a compile-sanity/regression guard, not a stand-in
// for the platform matrix.
// -------------------------------------------------------------------------
#[cfg(test)]
mod runtime_platform_tests {
    use super::runtime_platform;

    #[test]
    fn runtime_platform_matches_this_build_s_cfg_windows() {
        assert_eq!(
            runtime_platform(),
            if cfg!(windows) { "windows" } else { "unix" }
        );
    }
}
