//! Tauri IPC commands exposed to the UI (`#[tauri::command]` handlers).
//!
//! The UI talks to the core *only* through this module — every command here
//! should be a thin wrapper delegating to `hotkeys`/`audio`/`stt`/`cleanup`/
//! `output`/`context`/`store`, with `src/lib/ipc.ts` as the typed mirror on the
//! frontend side.
//!
//! Stub — no logic yet; implemented alongside the modules it wraps.
