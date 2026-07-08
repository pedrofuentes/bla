// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Thin binary entry point (required by Tauri's mobile target). All setup —
//! tray, window management, module wiring — lives in `lib.rs`; see its doc
//! comment and docs/ARCHITECTURE.md §Project Structure.

fn main() {
    bla_lib::run()
}
