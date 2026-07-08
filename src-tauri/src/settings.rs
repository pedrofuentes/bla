//! Settings persistence (issue #23, AC-13; ADR-0006).
//!
//! `Settings` is the flat, JSON-shaped config value ADR-0006 assigns to
//! `tauri-plugin-store` (as opposed to `store.rs`'s rusqlite-backed user
//! records: history, dictionary, snippets). Holds config only — no
//! transcript/clipboard text ever lands here, so deriving `Serialize`/
//! `Debug` on it carries none of the no-log risk `output::ClipboardPayload`
//! guards against (MISSION §7).
//!
//! Every decision here is pure and injectable, so it's testable without a
//! real `tauri-plugin-store`: [`to_json`]/[`from_json`] are plain,
//! deterministic (de)serialization, and [`SettingsStore`] is the seam a
//! future `tauri-plugin-store`-backed implementation would sit behind (thin
//! OS glue, not wired into `commands.rs` in this increment —
//! [`InMemorySettingsStore`] stands in for it in tests).

use serde::{Deserialize, Serialize};

/// Hold-to-record vs. toggle hotkey behavior (AC-8's state machine reads
/// this from settings; the machine itself lives in `hotkeys.rs`, untouched
/// here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordingMode {
    Hold,
    Toggle,
}

/// The selectable Whisper model presets (ADR-0004, PRD AC-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelPreset {
    LargeV3Turbo,
    Small,
}

/// Persisted output-mode preference (cursor-paste vs. file). Distinct from
/// `tray::OutputMode` (the live, in-memory switch) and `output::OutputMode`
/// (the router's dispatch target with resolved file config) — this is just
/// the durable user preference the other two are seeded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputModeSetting {
    Cursor,
    File,
}

/// All user-configurable settings (AC-13): hotkey binding, hold/toggle
/// mode, selected Whisper model preset, output mode, and the file-mode
/// path template. Config only — never holds transcript/clipboard text, so
/// deriving `Serialize`/`Debug` here doesn't touch the no-log invariant
/// `output::ClipboardPayload` enforces (MISSION §7).
///
/// `#[serde(default)]` at the struct level means any field missing from a
/// persisted (or first-run/empty) JSON blob falls back to
/// `Settings::default()`'s value for that field, rather than failing to
/// deserialize (AC-13's default-on-missing requirement).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub hotkey: String,
    pub recording_mode: RecordingMode,
    pub model_preset: ModelPreset,
    pub output_mode: OutputModeSetting,
    pub file_path_template: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "Control+Option+Space".to_string(),
            recording_mode: RecordingMode::Hold,
            model_preset: ModelPreset::LargeV3Turbo,
            output_mode: OutputModeSetting::Cursor,
            file_path_template: "{{date:YYYY-MM-DD}}.md".to_string(),
        }
    }
}

/// Serialize `settings` to a JSON string. Pure, deterministic, infallible
/// (every field is a plain string or unit-variant enum).
pub fn to_json(settings: &Settings) -> String {
    serde_json::to_string(settings).expect("Settings serialization is infallible")
}

/// Deserialize a JSON string into [`Settings`], defaulting any field the
/// JSON omits (AC-13). Fails only on genuinely malformed JSON or a field
/// present with the wrong shape.
pub fn from_json(json: &str) -> Result<Settings, serde_json::Error> {
    serde_json::from_str(json)
}

/// Persistence seam behind which a real `tauri-plugin-store`-backed
/// implementation would sit (thin OS glue, not wired into `commands.rs` in
/// this increment). Keeping this as a trait — rather than calling the
/// plugin directly — is what makes AC-13's restart-persistence behavior
/// testable without a live Tauri app context.
pub trait SettingsStore {
    /// Load persisted settings, or [`Settings::default()`] on first run /
    /// when nothing has been saved yet.
    fn load(&self) -> Settings;
    /// Persist `settings`, replacing whatever was previously stored.
    fn save(&mut self, settings: &Settings);
}

/// In-memory stand-in for the real store, used to test [`SettingsStore`]
/// consumers (and AC-13's restart-persistence behavior) without a real
/// `tauri-plugin-store`-backed app context. A "restart" is simulated by
/// extracting the persisted bytes via [`persisted`](Self::persisted) and
/// handing them to a fresh instance via [`from_persisted`](Self::from_persisted).
#[derive(Default)]
pub struct InMemorySettingsStore {
    raw: Option<String>,
}

impl InMemorySettingsStore {
    /// A store with nothing persisted yet (first run).
    pub fn new() -> Self {
        Self { raw: None }
    }

    /// A store pre-loaded with previously persisted JSON bytes, as if
    /// re-opened after an app restart.
    pub fn from_persisted(raw: String) -> Self {
        Self { raw: Some(raw) }
    }

    /// The raw JSON currently persisted, if anything has been saved yet.
    pub fn persisted(&self) -> Option<&str> {
        self.raw.as_deref()
    }
}

impl SettingsStore for InMemorySettingsStore {
    fn load(&self) -> Settings {
        self.raw
            .as_deref()
            .and_then(|json| from_json(json).ok())
            .unwrap_or_default()
    }

    fn save(&mut self, settings: &Settings) {
        self.raw = Some(to_json(settings));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn non_default_settings() -> Settings {
        Settings {
            hotkey: "Cmd+Shift+D".to_string(),
            recording_mode: RecordingMode::Toggle,
            model_preset: ModelPreset::Small,
            output_mode: OutputModeSetting::File,
            file_path_template: "journal/{{date:YYYY-MM-DD}}.md".to_string(),
        }
    }

    #[test]
    fn settings_round_trip_through_json_across_all_fields_ac13() {
        let settings = non_default_settings();

        let json = to_json(&settings);
        let restored = from_json(&json).unwrap();

        assert_eq!(restored, settings);
    }

    #[test]
    fn settings_default_differs_from_the_non_default_fixture_in_every_field() {
        // Guards the round-trip test above against a false positive: if this
        // ever fails, non_default_settings() stopped being a discriminating
        // fixture and the round-trip test could pass even with fields
        // swapped/dropped.
        let default = Settings::default();
        let non_default = non_default_settings();

        assert_ne!(default.hotkey, non_default.hotkey);
        assert_ne!(default.recording_mode, non_default.recording_mode);
        assert_ne!(default.model_preset, non_default.model_preset);
        assert_ne!(default.output_mode, non_default.output_mode);
        assert_ne!(default.file_path_template, non_default.file_path_template);
    }

    #[test]
    fn missing_settings_json_falls_back_to_defaults_on_first_run_ac13() {
        let restored = from_json("{}").unwrap();
        assert_eq!(restored, Settings::default());
    }

    #[test]
    fn partial_settings_json_defaults_only_the_missing_fields_ac13() {
        let partial = from_json(r#"{"hotkey":"Cmd+Shift+D"}"#).unwrap();

        assert_eq!(partial.hotkey, "Cmd+Shift+D");
        assert_eq!(partial.recording_mode, Settings::default().recording_mode);
        assert_eq!(partial.model_preset, Settings::default().model_preset);
        assert_eq!(partial.output_mode, Settings::default().output_mode);
        assert_eq!(
            partial.file_path_template,
            Settings::default().file_path_template
        );
    }

    #[test]
    fn settings_persist_across_a_simulated_app_restart_ac13() {
        // First run: nothing persisted yet, so load() falls back to defaults.
        let mut store = InMemorySettingsStore::new();
        assert_eq!(store.load(), Settings::default());

        let settings = non_default_settings();
        store.save(&settings);

        // Simulate an app restart: hand only the persisted bytes to a brand
        // new store instance, discarding the old one entirely.
        let persisted = store.persisted().unwrap().to_string();
        let restarted = InMemorySettingsStore::from_persisted(persisted);

        assert_eq!(restarted.load(), settings);
    }
}
