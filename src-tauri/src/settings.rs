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
