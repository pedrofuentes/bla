//! Global hotkey registration and the hold/toggle recording state machine.
//!
//! Owns `tauri-plugin-global-shortcut` wiring: binds the configured push-to-talk
//! key, tracks press/release (hold mode) or press/press (toggle mode), and emits
//! start/stop-recording events for `audio` to act on.
//!
//! OS-integration module (AGENTS.md §OS-integration exemption): thin glue only —
//! no decision logic. Keep state-machine *rules* testable in pure functions if
//! they grow non-trivial; this file just wires the platform API.
//!
//! Stub — no logic yet; implemented in a later M1 increment.
