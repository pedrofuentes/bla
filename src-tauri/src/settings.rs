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
    /// Whether bla should register itself to launch at OS login (issue
    /// #126, M2 PR 2.6). Defaults to `false` — autostart is opt-in, never
    /// silently enabled for a user who never asked for it. The actual OS
    /// registration is thin glue in `commands::set_settings` over
    /// `tauri-plugin-autostart`'s `AutoLaunchManager`; this field is only
    /// the durable preference.
    pub launch_at_login: bool,
    /// Whether to play short audio cues on recording start/stop (issue
    /// #126, M2 PR 2.6). Defaults to `true`. Purely a persisted preference
    /// in this PR — actual cue playback is wired up in PR 2.7, which reads
    /// this flag.
    pub sound_cues: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // Issue #110: previously "Control+Option+Space". "Option" is
            // macOS terminology for the Alt key — the accelerator parser
            // (`tauri_plugin_global_shortcut::Shortcut::from_str`) accepts
            // it as an alias for Alt on every platform (see
            // `hotkeys::the_actual_settings_default_hotkey_parses_on_every_platform_issue_98`),
            // but shipping a macOS-flavored default reads as unfamiliar on
            // Windows and risks colliding with Alt-based window/menu
            // accelerators there. `Control+Shift+Space` uses only modifier
            // names that mean the same thing, spelled the same way, on both
            // platforms — a documented cross-platform combo with no reliance
            // on a platform-specific alias.
            hotkey: "Control+Shift+Space".to_string(),
            recording_mode: RecordingMode::Hold,
            model_preset: ModelPreset::LargeV3Turbo,
            output_mode: OutputModeSetting::Cursor,
            file_path_template: "{{date:YYYY-MM-DD}}.md".to_string(),
            launch_at_login: false,
            sound_cues: true,
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

/// Why [`SettingsStore::load`] didn't return [`Settings`] (issue #80).
///
/// Kept as a distinct tri-state (rather than folding everything into
/// [`Settings::default()`]) specifically so a caller can tell "nothing has
/// been saved yet" (expected, silent-default-is-fine) apart from "something
/// WAS saved but is unreadable" (unexpected — must be surfaced, never
/// silently discarded). Before this, `load()` returned a bare `Settings`
/// and folded a present-but-malformed persisted JSON into
/// `.ok().unwrap_or_default()`, silently resetting every setting with no
/// signal to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsLoadError {
    /// Nothing has been persisted yet (first run, or a store that was never
    /// saved to) — expected, not corruption; callers may default here
    /// without surfacing anything to the user.
    NotFound,
    /// Something was persisted, but it failed to parse as valid
    /// [`Settings`] JSON (malformed JSON, or a field present with an
    /// unexpected shape/unknown enum variant). Carries the underlying parse
    /// error's message for diagnostics — safe to surface/log, since
    /// `Settings` holds only configuration, never transcript/clipboard text
    /// (MISSION §7's no-log invariant doesn't apply here). Callers MUST
    /// surface this rather than silently falling back, per issue #80.
    Corrupt(String),
}

impl std::fmt::Display for SettingsLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsLoadError::NotFound => write!(f, "no settings have been persisted yet"),
            SettingsLoadError::Corrupt(msg) => {
                write!(f, "persisted settings could not be parsed: {msg}")
            }
        }
    }
}

impl std::error::Error for SettingsLoadError {}

/// Persistence seam behind which a real `tauri-plugin-store`-backed
/// implementation would sit (thin OS glue, not wired into `commands.rs` in
/// this increment). Keeping this as a trait — rather than calling the
/// plugin directly — is what makes AC-13's restart-persistence behavior
/// testable without a live Tauri app context.
pub trait SettingsStore {
    /// Load persisted settings. `Err(SettingsLoadError::NotFound)` on first
    /// run / when nothing has been saved yet (not an error the caller need
    /// surface); `Err(SettingsLoadError::Corrupt(_))` when something was
    /// persisted but couldn't be parsed — issue #80: callers MUST surface
    /// this rather than silently resetting to defaults.
    fn load(&self) -> Result<Settings, SettingsLoadError>;
    /// Persist `settings`, replacing whatever was previously stored.
    /// `Err` carries a diagnostic message on a persistence failure (e.g. a
    /// disk write error in a real store-backed implementation).
    fn save(&mut self, settings: &Settings) -> Result<(), String>;
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
    fn load(&self) -> Result<Settings, SettingsLoadError> {
        match &self.raw {
            None => Err(SettingsLoadError::NotFound),
            Some(json) => from_json(json).map_err(|e| SettingsLoadError::Corrupt(e.to_string())),
        }
    }

    fn save(&mut self, settings: &Settings) -> Result<(), String> {
        self.raw = Some(to_json(settings));
        Ok(())
    }
}

/// Persist `new` through `store`, but only after `validate` accepts its
/// hotkey — the validate-before-persist decision (issue #91 Sentinel 🔴).
///
/// If `validate(&new.hotkey)` returns `Err`, the settings are rejected and
/// the store is left **untouched** (nothing is written), so a
/// malformed/unregistrable hotkey can never reach `settings.json` and brick
/// the next launch. `validate` is injected (rather than this module calling
/// `hotkeys::validate_hotkey` directly) so `settings` stays a pure,
/// dependency-free module and this decision is unit-testable with a fake
/// validator + [`InMemorySettingsStore`] (whose `persisted()` bytes prove
/// the store was or wasn't written). `commands::set_settings` performs the
/// same validate-before-persist ordering against the live
/// `tauri-plugin-store`.
pub fn persist_validated<S: SettingsStore>(
    store: &mut S,
    new: &Settings,
    validate: impl Fn(&str) -> Result<(), String>,
) -> Result<(), String> {
    validate(&new.hotkey)?;
    store.save(new)
}

/// Which direction (if any) a `launch_at_login` change needs to flip OS
/// autostart registration (issue #126, M2 PR 2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartAction {
    Enable,
    Disable,
}

/// Pure decision: does changing `launch_at_login` from `old` to `new`
/// require an OS autostart registration change, and in which direction?
/// `None` when unchanged — the common case on every settings save that
/// doesn't touch this field, where `commands::set_settings` must NOT call
/// into `tauri-plugin-autostart` at all. Kept separate from the actual
/// `AutoLaunchManager::enable`/`disable` call (thin OS glue in
/// `commands::set_settings`) so this decision is unit-testable without a
/// live Tauri app.
pub fn autostart_action_for_change(old: bool, new: bool) -> Option<AutostartAction> {
    match (old, new) {
        (false, true) => Some(AutostartAction::Enable),
        (true, false) => Some(AutostartAction::Disable),
        _ => None,
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
            launch_at_login: true,
            sound_cues: false,
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
        assert_ne!(default.launch_at_login, non_default.launch_at_login);
        assert_ne!(default.sound_cues, non_default.sound_cues);
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
        assert_eq!(
            partial.launch_at_login,
            Settings::default().launch_at_login
        );
        assert_eq!(partial.sound_cues, Settings::default().sound_cues);
    }

    // -------------------------------------------------------------
    // Issue #126 (M2 PR 2.6): `launch_at_login` and `sound_cues` must
    // default (false / true respectively) rather than fail to deserialize
    // when loading a settings.json persisted by a build from BEFORE this
    // PR — the same back-compat guarantee `#[serde(default)]` already gives
    // every other field (AC-13).
    // -------------------------------------------------------------

    #[test]
    fn pre_126_settings_json_without_launch_or_sound_fields_still_deserializes_with_defaults() {
        // Mirrors a real settings.json written by a pre-#126 build: every
        // field earlier M2 PRs introduced, but neither `launch_at_login`
        // nor `sound_cues`.
        let old_json = r#"{
            "hotkey": "Control+Shift+D",
            "recording_mode": "Toggle",
            "model_preset": "Small",
            "output_mode": "File",
            "file_path_template": "journal/{{date:YYYY-MM-DD}}.md"
        }"#;

        let restored = from_json(old_json).unwrap();

        // The pre-existing fields still parse as written...
        assert_eq!(restored.hotkey, "Control+Shift+D");
        assert_eq!(restored.recording_mode, RecordingMode::Toggle);
        assert_eq!(restored.model_preset, ModelPreset::Small);
        assert_eq!(restored.output_mode, OutputModeSetting::File);
        assert_eq!(
            restored.file_path_template,
            "journal/{{date:YYYY-MM-DD}}.md"
        );
        // ...and the two new fields fall back to their defaults instead of
        // the whole blob failing to parse.
        assert_eq!(
            restored.launch_at_login,
            Settings::default().launch_at_login
        );
        assert_eq!(restored.sound_cues, Settings::default().sound_cues);
    }

    #[test]
    fn launch_at_login_defaults_to_false_and_sound_cues_defaults_to_true() {
        let default = Settings::default();
        assert!(!default.launch_at_login);
        assert!(default.sound_cues);
    }

    // -------------------------------------------------------------
    // Issue #126 (M2 PR 2.6): the pure decision of whether (and which
    // direction) a `launch_at_login` change needs to flip OS autostart
    // registration. The actual `tauri-plugin-autostart` call is thin OS
    // glue in `commands::set_settings`; this is what's unit-tested without
    // a live Tauri app.
    // -------------------------------------------------------------

    #[test]
    fn autostart_action_is_none_when_launch_at_login_is_unchanged() {
        assert_eq!(autostart_action_for_change(false, false), None);
        assert_eq!(autostart_action_for_change(true, true), None);
    }

    #[test]
    fn autostart_action_is_enable_when_launch_at_login_flips_on() {
        assert_eq!(
            autostart_action_for_change(false, true),
            Some(AutostartAction::Enable)
        );
    }

    #[test]
    fn autostart_action_is_disable_when_launch_at_login_flips_off() {
        assert_eq!(
            autostart_action_for_change(true, false),
            Some(AutostartAction::Disable)
        );
    }

    #[test]
    fn settings_persist_across_a_simulated_app_restart_ac13() {
        // First run: nothing persisted yet.
        let mut store = InMemorySettingsStore::new();
        assert_eq!(store.load(), Err(SettingsLoadError::NotFound));

        let settings = non_default_settings();
        store.save(&settings).expect("save should succeed");

        // Simulate an app restart: hand only the persisted bytes to a brand
        // new store instance, discarding the old one entirely.
        let persisted = store.persisted().unwrap().to_string();
        let restarted = InMemorySettingsStore::from_persisted(persisted);

        assert_eq!(restarted.load().unwrap(), settings);
    }

    // -------------------------------------------------------------
    // Issue #80: load() must distinguish NotFound from Corrupt, and must
    // NEVER silently fold a present-but-malformed persisted JSON into
    // Settings::default() without surfacing anything to the caller.
    // -------------------------------------------------------------

    #[test]
    fn load_on_a_fresh_store_returns_not_found_not_a_silent_default() {
        let store = InMemorySettingsStore::new();
        assert_eq!(store.load(), Err(SettingsLoadError::NotFound));
    }

    #[test]
    fn load_of_malformed_json_returns_corrupt_not_a_silent_default_issue_80() {
        // Before the fix, this silently collapsed to Settings::default()
        // via .ok().unwrap_or_default() — no signal that anything was even
        // persisted, let alone that it was unreadable.
        let store = InMemorySettingsStore::from_persisted("{not valid json at all".to_string());
        let err = store.load().unwrap_err();
        assert!(
            matches!(err, SettingsLoadError::Corrupt(_)),
            "malformed JSON must surface as Corrupt, got {err:?}"
        );
    }

    #[test]
    fn load_of_an_unknown_enum_variant_returns_corrupt_not_a_silent_default_issue_80() {
        // Valid JSON syntactically, but `recording_mode` isn't one of
        // RecordingMode's variants — must still surface as Corrupt, not
        // silently reset every field to defaults.
        let store = InMemorySettingsStore::from_persisted(
            r#"{"recording_mode":"NotARealVariant"}"#.to_string(),
        );
        let err = store.load().unwrap_err();
        assert!(
            matches!(err, SettingsLoadError::Corrupt(_)),
            "an unknown enum variant must surface as Corrupt, got {err:?}"
        );
    }

    #[test]
    fn load_of_valid_persisted_json_still_succeeds_after_the_80_change() {
        let mut store = InMemorySettingsStore::new();
        let settings = non_default_settings();
        store.save(&settings).unwrap();
        assert_eq!(store.load().unwrap(), settings);
    }

    // -------------------------------------------------------------
    // Issue #91 Sentinel 🔴: persist_validated must NOT write an invalid
    // hotkey — an invalid one is rejected and the store is left unchanged,
    // so a bad hotkey can never reach settings.json and brick launch.
    // -------------------------------------------------------------

    #[test]
    fn persist_validated_rejects_an_invalid_hotkey_and_leaves_the_store_unchanged_issue_91() {
        let mut store = InMemorySettingsStore::new();
        let good = non_default_settings();
        store.save(&good).unwrap();
        let baseline = store.persisted().unwrap().to_string();

        // A would-be update carrying a hotkey the validator rejects.
        let mut update = good.clone();
        update.hotkey = "totally invalid hotkey".to_string();

        let result = persist_validated(&mut store, &update, |hk| {
            if hk == "totally invalid hotkey" {
                Err("rejected".to_string())
            } else {
                Ok(())
            }
        });

        assert!(result.is_err(), "an invalid hotkey must be rejected");
        assert_eq!(
            store.persisted().unwrap(),
            baseline,
            "the store must be left byte-for-byte unchanged when validation fails"
        );
    }

    #[test]
    fn persist_validated_persists_when_the_hotkey_validates_issue_91() {
        let mut store = InMemorySettingsStore::new();
        let settings = non_default_settings();

        let result = persist_validated(&mut store, &settings, |_| Ok(()));

        assert!(result.is_ok());
        assert_eq!(store.load().unwrap(), settings);
    }
}
