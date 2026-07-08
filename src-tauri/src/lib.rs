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

pub mod audio;
mod cleanup;
mod commands;
mod context;
mod hotkeys;
mod output;
// `pub` (rather than private like most stub siblings): Settings, to_json/
// from_json, and SettingsStore/InMemorySettingsStore are real, tested,
// standalone-usable API surface as of this increment (not yet wired into
// commands.rs), so keeping the module private would make rustc flag them as
// dead code.
pub mod settings;
mod store;
// `pub` for the same reason as `settings`: PipelineState, TrayIconState,
// tray_icon_state, OutputMode, and OutputModeSwitch are real, tested API
// surface not yet wired into `run()`.
pub mod tray;
// `pub` (rather than private like its stub siblings): stt's Stt trait,
// FakeStt, and build_initial_prompt are real, tested, standalone-usable API
// surface as of this increment (not yet wired into commands.rs), so keeping
// the module private would make rustc flag them as dead code.
pub mod stt;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
