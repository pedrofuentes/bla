//! Local persistence via `rusqlite` (+ `tauri-plugin-store` for simple settings).
//!
//! Owns history, personal dictionary, and snippets — all local-only, under the
//! OS app-data dir (MISSION §5: no server, nothing leaves the device).
//!
//! Pure logic over the DB layer should stay unit-testable; the connection/IO
//! boundary is the only OS-adjacent part.
//!
//! Stub — no logic yet; implemented in a later M1 increment.
