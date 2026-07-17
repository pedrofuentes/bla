//! Global hotkey registration and the hold/toggle recording state machine.
//!
//! Owns `tauri-plugin-global-shortcut` wiring: binds the configured push-to-talk
//! key, tracks press/release (hold mode) or press/press (toggle mode), and emits
//! start/stop-recording events for `audio` to act on.
//!
//! OS-integration module (AGENTS.md §OS-integration exemption): thin glue only —
//! no decision logic; the platform wiring around `tauri-plugin-global-shortcut`
//! is a stub, implemented in a later M1 increment (TDD-exempt OS glue, kept
//! separate from the logic below).
//!
//! ## Pure state machine (AC-8)
//!
//! [`StateMachine`] is the pure hold/toggle logic: no OS calls, driven
//! entirely by injected [`KeyEvent`]s carrying a caller-supplied
//! [`Timestamp`], so it never calls `Instant::now()` itself and is fully
//! deterministic in tests. The (future) OS glue above will construct one
//! `StateMachine`, feed it real key events translated from
//! `tauri-plugin-global-shortcut` callbacks, and react to the
//! [`Transition`]s it emits by starting/stopping `audio` capture.

#![allow(dead_code)] // Not yet wired to the OS-glue layer or `commands`.

use std::collections::HashSet;
use std::time::Duration;

/// Validate that `hotkey` parses as a registrable global-shortcut
/// accelerator (e.g. `"Control+Option+Space"`). Pure string parsing via the
/// **same** parser the OS-glue registration path uses
/// (`tauri-plugin-global-shortcut`'s `Shortcut::from_str`), so a value that
/// passes here is exactly the value that will register — there is no parser
/// divergence between "validated" and "registered". No OS handles are
/// touched, so this is fully unit-testable.
///
/// This is the pure logic behind two OS-glue call sites (issue #91 Sentinel
/// 🔴): `commands::set_settings` validates a user-typed hotkey with this
/// *before* persisting it (so a malformed one is rejected at the IPC
/// boundary and never written), and `run()`'s startup uses
/// [`resolve_effective_hotkey`] (built on this) so a bad persisted hotkey
/// falls back to the default instead of bricking launch.
pub fn validate_hotkey(hotkey: &str) -> Result<(), String> {
    use std::str::FromStr;
    tauri_plugin_global_shortcut::Shortcut::from_str(hotkey)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// AC-49 (issue #259, part of #242, M4): reject a command-mode hotkey that's
/// indistinguishable from the dictation hotkey, rather than silently letting
/// one shortcut registration shadow the other. Parses BOTH accelerators via
/// the same [`validate_hotkey`] parser (`Shortcut::from_str`) and compares
/// the *parsed* values — not the raw strings — so two spellings of the same
/// physical chord (different key-token order, e.g. `"Shift+Control+C"` vs.
/// `"Control+Shift+C"`, or a case difference) are still caught as identical,
/// exactly as the OS registrar would see them once bound. `commands::set_settings`
/// calls this before persisting either hotkey (mirroring how it already
/// calls `validate_hotkey` before persisting one) — a malformed accelerator
/// is reported via that same parse error, so this function doesn't need a
/// separate "which one is malformed" case: [`validate_hotkey`] already runs
/// first at each call site.
pub fn distinct_hotkeys(dictation: &str, command: &str) -> Result<(), String> {
    use std::str::FromStr;
    let dictation_parsed =
        tauri_plugin_global_shortcut::Shortcut::from_str(dictation).map_err(|e| e.to_string())?;
    let command_parsed =
        tauri_plugin_global_shortcut::Shortcut::from_str(command).map_err(|e| e.to_string())?;
    if dictation_parsed == command_parsed {
        Err(format!(
            "the command-mode hotkey must differ from the dictation hotkey (both resolve to \
             {command:?})"
        ))
    } else {
        Ok(())
    }
}

/// Startup fallback (issue #91 Sentinel 🔴): returns `persisted` if it is a
/// valid hotkey per [`validate_hotkey`], otherwise `default`. Callers pass
/// `settings::Settings::default().hotkey` (always a valid accelerator) as
/// `default`, so the resolved value is guaranteed registrable — a
/// corrupt/unregistrable persisted hotkey degrades to the default binding
/// rather than propagating a registration failure into a fatal startup
/// panic.
pub fn resolve_effective_hotkey<'a>(persisted: &'a str, default: &'a str) -> &'a str {
    if validate_hotkey(persisted).is_ok() {
        persisted
    } else {
        default
    }
}

/// Injected timestamp abstraction — an opaque duration since some
/// caller-chosen origin (e.g. `Instant::now().duration_since(origin)` in the
/// real glue). Tests construct these directly with `Duration::from_millis`,
/// so the state machine never touches the system clock.
pub type Timestamp = Duration;

/// Identifies one physical key participating in the configured hotkey chord.
pub type KeyId = u32;

/// Recording trigger mode (PRD AC-8), configurable via settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Record while the configured chord is held; releasing any chord key
    /// stops recording (subject to the debounce threshold).
    Hold,
    /// The first full chord press starts recording; the next full chord
    /// press stops it. Physical key release has no effect.
    Toggle,
}

/// A single physical key press or release, timestamped by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    KeyDown(KeyId, Timestamp),
    KeyUp(KeyId, Timestamp),
}

/// Emitted by [`StateMachine::handle`] in response to a [`KeyEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// The chord was newly pressed — begin capturing audio.
    StartRecording,
    /// A completed dictation: the recording should be transcribed.
    StopRecording,
    /// A Hold-mode press shorter than the debounce threshold — treated as
    /// accidental. Recording must be discarded; no dictation is produced.
    Cancelled,
}

/// Default debounce threshold for Hold mode (PRD AC-8): a press shorter than
/// this is treated as an accidental key touch.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    /// Hold-mode recording in progress; started when the chord completed.
    Holding {
        started_at: Timestamp,
    },
    /// Toggle-mode recording in progress.
    ToggledOn,
}

/// Pure hold/toggle hotkey state machine (AC-8). Holds no OS handles and
/// performs no I/O — see the module docs for how the OS-glue layer above
/// drives it.
pub struct StateMachine {
    mode: Mode,
    chord: HashSet<KeyId>,
    debounce: Duration,
    held: HashSet<KeyId>,
    phase: Phase,
}

impl StateMachine {
    /// `chord` is the set of keys that must be simultaneously down to
    /// trigger the hotkey; `debounce` is the minimum Hold-mode press
    /// duration for a dictation to be emitted (default: [`DEFAULT_DEBOUNCE`]).
    pub fn new(mode: Mode, chord: impl IntoIterator<Item = KeyId>, debounce: Duration) -> Self {
        Self {
            mode,
            chord: chord.into_iter().collect(),
            debounce,
            held: HashSet::new(),
            phase: Phase::Idle,
        }
    }

    /// The currently configured recording mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Switch the recording mode in place (issue #126 / PR #134 Sentinel
    /// 🔴-3: `commands::set_settings` calls this — via
    /// `apply_settings_to_state` in lib.rs — so a saved Hold↔Toggle change
    /// takes effect on the LIVE machine rather than after a restart).
    ///
    /// Setting the current mode again is a no-op (`None`) — an unrelated
    /// settings save must not disturb a dictation in flight. An actual mode
    /// change [`reset`](Self::reset)s the machine: a session started under
    /// the old mode's semantics can't be meaningfully continued under the
    /// new ones, so any in-flight session is abnormally interrupted
    /// ([`Transition::Cancelled`] — the caller discards its audio, same as
    /// the debounce/focus-loss paths) and the held-key set is cleared so the
    /// machine can't wedge.
    pub fn set_mode(&mut self, mode: Mode) -> Option<Transition> {
        if self.mode == mode {
            return None;
        }
        self.mode = mode;
        self.reset()
    }

    fn chord_complete(&self) -> bool {
        !self.chord.is_empty() && self.chord.iter().all(|key| self.held.contains(key))
    }

    /// Feed one key event into the machine; returns the [`Transition`] it
    /// produces, if any.
    pub fn handle(&mut self, event: KeyEvent) -> Option<Transition> {
        match event {
            KeyEvent::KeyDown(key, at) => {
                if !self.chord.contains(&key) || self.held.contains(&key) {
                    // Not a chord key, or an OS key-repeat of an
                    // already-held key — no edge, nothing to do.
                    self.held.insert(key);
                    return None;
                }
                let was_complete = self.chord_complete();
                self.held.insert(key);
                if !was_complete && self.chord_complete() {
                    self.on_chord_pressed(at)
                } else {
                    None
                }
            }
            KeyEvent::KeyUp(key, at) => {
                if !self.chord.contains(&key) || !self.held.contains(&key) {
                    self.held.remove(&key);
                    return None;
                }
                let was_complete = self.chord_complete();
                self.held.remove(&key);
                if was_complete && !self.chord_complete() {
                    self.on_chord_released(at)
                } else {
                    None
                }
            }
        }
    }

    /// The chord transitioned from not-fully-held to fully-held.
    fn on_chord_pressed(&mut self, at: Timestamp) -> Option<Transition> {
        match (self.mode, self.phase) {
            (Mode::Hold, Phase::Idle) => {
                self.phase = Phase::Holding { started_at: at };
                Some(Transition::StartRecording)
            }
            (Mode::Toggle, Phase::Idle) => {
                self.phase = Phase::ToggledOn;
                Some(Transition::StartRecording)
            }
            (Mode::Toggle, Phase::ToggledOn) => {
                self.phase = Phase::Idle;
                Some(Transition::StopRecording)
            }
            _ => None,
        }
    }

    /// Reconciliation entry the OS glue calls when a `KeyUp` might have been
    /// dropped — e.g. on window focus-loss, screen lock, or system
    /// sleep/resume (issue #44). Before this, the only way out of
    /// `Phase::Holding` was a matching `KeyUp`; a dropped one left the
    /// machine permanently wedged in `Holding` with a stale held-set, and
    /// every subsequent hotkey press would silently do nothing (the chord
    /// already reads as "complete" from the stale `held` state, so a fresh
    /// press can never re-trigger the not-complete -> complete edge
    /// `on_chord_pressed` requires).
    ///
    /// Unconditionally clears the held-key set and returns to `Phase::Idle`,
    /// treating any in-progress session as abnormally interrupted rather
    /// than a genuine dictation:
    /// - From `Phase::Holding` or `Phase::ToggledOn`: emits
    ///   [`Transition::Cancelled`] (mirroring the debounce path — a
    ///   reconciliation is not a real dictation completing) so the caller
    ///   discards whatever audio was captured and stops capture.
    /// - From `Phase::Idle`: a no-op, returns `None`.
    pub fn reset(&mut self) -> Option<Transition> {
        self.held.clear();
        let was_active = !matches!(self.phase, Phase::Idle);
        self.phase = Phase::Idle;
        if was_active {
            Some(Transition::Cancelled)
        } else {
            None
        }
    }

    /// The chord transitioned from fully-held to not-fully-held (any one
    /// chord key released).
    fn on_chord_released(&mut self, at: Timestamp) -> Option<Transition> {
        match (self.mode, self.phase) {
            (Mode::Hold, Phase::Holding { started_at }) => {
                self.phase = Phase::Idle;
                if at.saturating_sub(started_at) < self.debounce {
                    Some(Transition::Cancelled)
                } else {
                    Some(Transition::StopRecording)
                }
            }
            // Toggle mode ignores physical key release entirely.
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ms(n: u64) -> Timestamp {
        Duration::from_millis(n)
    }

    // AC-8: hold-to-record produces exactly one dictation per press/release
    // cycle — a full press held past the debounce threshold emits exactly
    // one StartRecording followed by exactly one StopRecording.
    #[test]
    fn hold_press_release_emits_one_start_and_one_stop() {
        let mut sm = StateMachine::new(Mode::Hold, [1], Duration::from_millis(300));

        let start = sm.handle(KeyEvent::KeyDown(1, ms(0)));
        let stop = sm.handle(KeyEvent::KeyUp(1, ms(500)));

        assert_eq!(start, Some(Transition::StartRecording));
        assert_eq!(stop, Some(Transition::StopRecording));
    }

    // OS key-repeat sends repeated KeyDown for an already-held key; it must
    // not re-trigger StartRecording or otherwise disturb the in-progress
    // hold session.
    #[test]
    fn hold_key_repeat_does_not_retrigger() {
        let mut sm = StateMachine::new(Mode::Hold, [1], Duration::from_millis(300));

        let first = sm.handle(KeyEvent::KeyDown(1, ms(0)));
        let repeat = sm.handle(KeyEvent::KeyDown(1, ms(50)));
        let stop = sm.handle(KeyEvent::KeyUp(1, ms(500)));

        assert_eq!(first, Some(Transition::StartRecording));
        assert_eq!(repeat, None);
        assert_eq!(stop, Some(Transition::StopRecording));
    }

    // Debounce: a Hold press shorter than the configured threshold (default
    // 300 ms) is accidental — no dictation (StopRecording) is emitted, only
    // Cancelled.
    #[test]
    fn hold_press_shorter_than_debounce_is_cancelled() {
        let mut sm = StateMachine::new(Mode::Hold, [1], Duration::from_millis(300));

        let start = sm.handle(KeyEvent::KeyDown(1, ms(0)));
        let end = sm.handle(KeyEvent::KeyUp(1, ms(100)));

        assert_eq!(start, Some(Transition::StartRecording));
        assert_eq!(end, Some(Transition::Cancelled));
    }

    // Boundary: a press exactly at the debounce threshold counts as
    // deliberate, not accidental.
    #[test]
    fn hold_press_exactly_at_debounce_threshold_stops_normally() {
        let mut sm = StateMachine::new(Mode::Hold, [1], Duration::from_millis(300));

        sm.handle(KeyEvent::KeyDown(1, ms(0)));
        let end = sm.handle(KeyEvent::KeyUp(1, ms(300)));

        assert_eq!(end, Some(Transition::StopRecording));
    }

    // Out-of-order chord key-up: for a multi-key chord, hold recording must
    // stop when ANY chord key releases — not only the last key pressed.
    #[test]
    fn hold_chord_stops_on_any_key_release_regardless_of_press_order() {
        let mut sm = StateMachine::new(Mode::Hold, [1, 2], Duration::from_millis(300));

        // Press order: 1, then 2 (chord becomes complete on 2's key-down).
        let partial = sm.handle(KeyEvent::KeyDown(1, ms(0)));
        let start = sm.handle(KeyEvent::KeyDown(2, ms(10)));

        assert_eq!(partial, None, "chord isn't complete with only one key down");
        assert_eq!(start, Some(Transition::StartRecording));

        // Release out of press order: key 1 (pressed first) goes up first,
        // not key 2 (pressed last) — the hold must still end.
        let stop = sm.handle(KeyEvent::KeyUp(1, ms(500)));
        assert_eq!(stop, Some(Transition::StopRecording));

        // The remaining key going up afterward is a no-op — the session
        // already ended.
        let after = sm.handle(KeyEvent::KeyUp(2, ms(600)));
        assert_eq!(after, None);
    }

    // AC-8: toggle mode starts recording on the first full chord press and
    // stops it on the next — physical key release in between has no effect.
    #[test]
    fn toggle_starts_on_first_press_and_stops_on_next() {
        let mut sm = StateMachine::new(Mode::Toggle, [1], Duration::from_millis(300));

        let start = sm.handle(KeyEvent::KeyDown(1, ms(0)));
        let release_noop = sm.handle(KeyEvent::KeyUp(1, ms(50)));
        let stop = sm.handle(KeyEvent::KeyDown(1, ms(1_000)));

        assert_eq!(start, Some(Transition::StartRecording));
        assert_eq!(release_noop, None);
        assert_eq!(stop, Some(Transition::StopRecording));
    }

    // Toggle mode must ignore OS key-repeat the same way Hold mode does —
    // a repeated KeyDown while the chord is already fully held (e.g. before
    // the user releases any key at all) must not toggle again.
    #[test]
    fn toggle_key_repeat_does_not_retoggle() {
        let mut sm = StateMachine::new(Mode::Toggle, [1], Duration::from_millis(300));

        let start = sm.handle(KeyEvent::KeyDown(1, ms(0)));
        let repeat = sm.handle(KeyEvent::KeyDown(1, ms(20)));

        assert_eq!(start, Some(Transition::StartRecording));
        assert_eq!(repeat, None);
    }

    // -----------------------------------------------------------------
    // Issue #44: reset()/reconcile — a dropped KeyUp must not wedge Holding
    // -----------------------------------------------------------------

    #[test]
    fn reset_from_idle_is_a_noop() {
        let mut sm = StateMachine::new(Mode::Hold, [1], Duration::from_millis(300));
        assert_eq!(sm.reset(), None);
        // Still fully functional afterward.
        let start = sm.handle(KeyEvent::KeyDown(1, ms(0)));
        assert_eq!(start, Some(Transition::StartRecording));
    }

    #[test]
    fn reset_from_holding_cancels_and_unwedges_the_machine_issue_44() {
        let mut sm = StateMachine::new(Mode::Hold, [1], Duration::from_millis(300));

        // Chord pressed, now Holding — then its KeyUp is dropped (focus
        // loss, lock, suspend) rather than ever arriving.
        let start = sm.handle(KeyEvent::KeyDown(1, ms(0)));
        assert_eq!(start, Some(Transition::StartRecording));

        // OS glue calls reset() instead (e.g. on window focus-loss).
        let reconciled = sm.reset();
        assert_eq!(
            reconciled,
            Some(Transition::Cancelled),
            "a dropped-KeyUp reconciliation must cancel the stuck session"
        );

        // The machine must NOT be wedged: a fresh chord press re-triggers
        // normally. Before the fix, the stale `held` set from before would
        // have made chord_complete() already true, so this fresh KeyDown
        // could never re-fire the not-complete -> complete edge.
        let start_again = sm.handle(KeyEvent::KeyDown(1, ms(1_000)));
        assert_eq!(
            start_again,
            Some(Transition::StartRecording),
            "after reset(), the machine must accept a brand-new chord press"
        );
    }

    #[test]
    fn reset_from_holding_clears_a_stale_multi_key_held_set_issue_44() {
        let mut sm = StateMachine::new(Mode::Hold, [1, 2], Duration::from_millis(300));

        // Two-key chord fully pressed (Holding); its KeyUps are dropped.
        sm.handle(KeyEvent::KeyDown(1, ms(0)));
        let start = sm.handle(KeyEvent::KeyDown(2, ms(10)));
        assert_eq!(start, Some(Transition::StartRecording));

        assert_eq!(sm.reset(), Some(Transition::Cancelled));

        // Re-pressing only ONE of the two keys must NOT re-trigger — proves
        // the stale held-set (both keys 1 and 2) was actually cleared,
        // rather than only the phase being reset while `held` lingered.
        let partial = sm.handle(KeyEvent::KeyDown(1, ms(2_000)));
        assert_eq!(partial, None, "chord isn't complete with only one key down");

        let start_again = sm.handle(KeyEvent::KeyDown(2, ms(2_010)));
        assert_eq!(start_again, Some(Transition::StartRecording));
    }

    #[test]
    fn reset_from_toggled_on_cancels_and_unwedges_the_machine_issue_44() {
        let mut sm = StateMachine::new(Mode::Toggle, [1], Duration::from_millis(300));

        let start = sm.handle(KeyEvent::KeyDown(1, ms(0)));
        assert_eq!(start, Some(Transition::StartRecording));
        sm.handle(KeyEvent::KeyUp(1, ms(10))); // toggle mode ignores release

        let reconciled = sm.reset();
        assert_eq!(reconciled, Some(Transition::Cancelled));

        // A fresh press starts a brand-new toggle session (not a "stop" of
        // a phantom already-on session).
        let start_again = sm.handle(KeyEvent::KeyDown(1, ms(1_000)));
        assert_eq!(start_again, Some(Transition::StartRecording));
    }

    // -----------------------------------------------------------------
    // Issue #91 Sentinel 🔴: pure hotkey validation + startup fallback so a
    // malformed hotkey can't be persisted (and can't brick launch).
    // -----------------------------------------------------------------

    #[test]
    fn validate_hotkey_accepts_well_formed_accelerators() {
        // The persisted default plus a representative user-chosen binding.
        for good in ["Control+Option+Space", "Cmd+Shift+D", "Alt+F4", "Super+K"] {
            assert!(
                validate_hotkey(good).is_ok(),
                "expected {good:?} to be a valid hotkey"
            );
        }
    }

    #[test]
    fn validate_hotkey_rejects_malformed_accelerators_issue_91() {
        // Empty, an unknown key, a dangling '+' (empty token), and a
        // modifiers-only chord with no main key must all be rejected — so
        // set_settings never persists one of these.
        for bad in ["", "NotARealKey", "Ctrl+", "Control+Shift"] {
            assert!(
                validate_hotkey(bad).is_err(),
                "expected {bad:?} to be rejected as an invalid hotkey"
            );
        }
    }

    // -----------------------------------------------------------------
    // Issue #259 (AC-49): a command-mode hotkey identical to the dictation
    // hotkey must be rejected at settings-save time, not silently let one
    // shortcut registration shadow the other.
    // -----------------------------------------------------------------

    #[test]
    fn distinct_hotkeys_accepts_two_different_accelerators_issue_259() {
        assert!(distinct_hotkeys("Control+Shift+Space", "Control+Shift+C").is_ok());
    }

    #[test]
    fn distinct_hotkeys_rejects_two_identical_accelerators_issue_259() {
        let err = distinct_hotkeys("Control+Shift+Space", "Control+Shift+Space")
            .expect_err("identical bindings must be rejected");
        assert!(!err.is_empty(), "the error must carry a clear message");
    }

    #[test]
    fn distinct_hotkeys_rejects_the_same_chord_spelled_with_different_key_order_issue_259() {
        // The OS registrar (and validate_hotkey's own parser) treats these
        // as the exact same accelerator — this must be caught even though
        // the raw strings differ.
        let err = distinct_hotkeys("Control+Shift+C", "Shift+Control+C");
        assert!(
            err.is_err(),
            "differently-ordered modifiers for the same chord must still be rejected"
        );
    }

    #[test]
    fn distinct_hotkeys_propagates_a_malformed_dictation_hotkeys_parse_error_issue_259() {
        assert!(distinct_hotkeys("NotARealKey", "Control+Shift+C").is_err());
    }

    #[test]
    fn distinct_hotkeys_propagates_a_malformed_command_hotkeys_parse_error_issue_259() {
        assert!(distinct_hotkeys("Control+Shift+Space", "NotARealKey").is_err());
    }

    #[test]
    fn the_actual_settings_defaults_for_both_hotkeys_are_distinct_issue_259() {
        // Ties this assertion to the real `Settings::default()` values (not
        // copy-pasted literals) so bla's own shipped defaults can never
        // silently regress into colliding with each other and rejecting the
        // very first settings save on a fresh install.
        let settings = crate::settings::Settings::default();
        assert!(distinct_hotkeys(&settings.hotkey, &settings.command_hotkey).is_ok());
    }

    // -----------------------------------------------------------------
    // Issue #281 (ac7-p0): the command-mode hotkey's TRIGGER key must be a
    // function key (F1-F24) — see `validate_command_hotkey_keyset`'s doc
    // comment for the full rationale (a leaked character-producing key
    // clobbers the CONTENT selection; a function key produces no
    // character, so it's harmless if the OS/plugin leaks it while held).
    // -----------------------------------------------------------------

    #[test]
    fn validate_command_hotkey_keyset_accepts_function_key_chords_issue_281() {
        for good in [
            "Control+Shift+F13",
            "Alt+F1",
            "Cmd+F24",
            "Control+Alt+Shift+Super+F7",
            "F12",
        ] {
            assert!(
                validate_command_hotkey_keyset(good).is_ok(),
                "expected {good:?} to be accepted as a command-mode hotkey"
            );
        }
    }

    #[test]
    fn validate_command_hotkey_keyset_rejects_character_producing_trigger_keys_issue_281() {
        // A representative sample of the harm class this exists to prevent:
        // letters, digits, punctuation, space, Enter, and Tab all produce a
        // text character when leaked — exactly what clobbered the selection
        // in #281's repro (`Ctrl+Shift+O` -> "oooo").
        for bad in [
            "Control+Shift+C",  // the #281 repro's letter key
            "Control+Shift+O",  // the #281 repro's exact chord
            "Control+Shift+1",  // digit
            "Control+Comma",    // punctuation
            "Control+Shift+Space",
            "Control+Enter",
            "Control+Tab",
        ] {
            let err = validate_command_hotkey_keyset(bad)
                .expect_err(&format!("expected {bad:?} to be rejected"));
            assert!(
                err.to_lowercase().contains("function key"),
                "expected a clear function-key explanation for {bad:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn validate_command_hotkey_keyset_rejects_non_function_non_character_keys_issue_281() {
        // Arrow/navigation keys don't produce a text character either, but
        // the cofounder-decided allowlist is specifically function keys —
        // not "any non-character key" — so these must still be rejected.
        for bad in ["Control+ArrowUp", "Control+Home", "Control+Escape"] {
            assert!(
                validate_command_hotkey_keyset(bad).is_err(),
                "expected {bad:?} to be rejected — only function keys are allowlisted"
            );
        }
    }

    #[test]
    fn validate_command_hotkey_keyset_propagates_a_malformed_accelerators_parse_error_issue_281() {
        for bad in ["", "NotARealKey", "Ctrl+", "Control+Shift"] {
            assert!(
                validate_command_hotkey_keyset(bad).is_err(),
                "expected {bad:?} to be rejected as an unparseable accelerator"
            );
        }
    }

    #[test]
    fn resolve_effective_hotkey_keeps_a_valid_persisted_binding() {
        let effective = resolve_effective_hotkey("Cmd+Shift+D", "Control+Option+Space");
        assert_eq!(effective, "Cmd+Shift+D");
    }

    #[test]
    fn resolve_effective_hotkey_falls_back_to_default_on_a_bad_persisted_binding_issue_91() {
        // A corrupt/unregistrable persisted hotkey must resolve to the
        // (always-valid) default rather than being handed to registration —
        // this is what keeps a bad settings.json from bricking startup.
        let effective = resolve_effective_hotkey("NotARealKey", "Control+Option+Space");
        assert_eq!(effective, "Control+Option+Space");
    }

    #[test]
    fn the_actual_settings_default_hotkey_parses_on_every_platform_issue_98() {
        // Ties this assertion to `settings::Settings::default().hotkey`
        // itself — not a copy-pasted literal that could silently drift from
        // it — so the app can never ship a default hotkey that fails to
        // register on some platform (`validate_hotkey` runs the identical
        // `Shortcut::from_str` parser `tauri-plugin-global-shortcut` uses to
        // register a hotkey, and that parser isn't `cfg`-gated per OS: it's
        // one accelerator grammar shared by every target). If the default
        // ever became unregistrable, `resolve_effective_hotkey`'s fallback
        // (above) would have nothing safe left to fall back to and startup
        // would never bind a hotkey at all.
        let default_hotkey = crate::settings::Settings::default().hotkey;
        assert!(
            validate_hotkey(&default_hotkey).is_ok(),
            "the persisted default hotkey {default_hotkey:?} must parse on every platform"
        );
    }

    #[test]
    fn the_actual_settings_default_hotkey_avoids_the_macos_only_option_alias_issue_110() {
        // Issue #110: the default used to be "Control+Option+Space".
        // "Option" is macOS terminology for the Alt key; even though the
        // parser accepts it as a synonym for Alt on every platform (the test
        // above), shipping it as the *default* reads as unfamiliar on
        // Windows. Pin the default away from that alias (and any other
        // macOS-only spelling) so this can't silently regress, on top of
        // the parses-everywhere assertion above.
        let default_hotkey = crate::settings::Settings::default().hotkey;
        let upper = default_hotkey.to_uppercase();
        assert!(
            !upper.contains("OPTION"),
            "the default hotkey {default_hotkey:?} must not rely on the macOS-only \
             \"Option\" spelling of the Alt modifier"
        );
        assert!(
            validate_hotkey(&default_hotkey).is_ok(),
            "the default hotkey {default_hotkey:?} must parse as a registrable \
             accelerator on every platform"
        );
    }

    // -----------------------------------------------------------------
    // Issue #126 / PR #134 Sentinel 🔴-3: a recording_mode change saved via
    // set_settings must take effect on the LIVE machine, not after restart.
    // set_mode is the pure mechanism: flips the mode in place, cancelling
    // any in-flight session (a session started under the old mode's
    // semantics can't be meaningfully continued under the new ones).
    // -----------------------------------------------------------------

    #[test]
    fn mode_getter_exposes_the_configured_mode() {
        let hold = StateMachine::new(Mode::Hold, [1], Duration::from_millis(300));
        let toggle = StateMachine::new(Mode::Toggle, [1], Duration::from_millis(300));
        assert_eq!(hold.mode(), Mode::Hold);
        assert_eq!(toggle.mode(), Mode::Toggle);
    }

    #[test]
    fn set_mode_to_the_same_mode_is_a_noop_that_preserves_an_in_flight_session() {
        let mut sm = StateMachine::new(Mode::Hold, [1], Duration::from_millis(300));
        let start = sm.handle(KeyEvent::KeyDown(1, ms(0)));
        assert_eq!(start, Some(Transition::StartRecording));

        // Re-applying the current mode (e.g. a settings save that changed
        // only the model preset) must not cancel the dictation in flight.
        assert_eq!(sm.set_mode(Mode::Hold), None);
        assert_eq!(sm.mode(), Mode::Hold);

        let stop = sm.handle(KeyEvent::KeyUp(1, ms(500)));
        assert_eq!(stop, Some(Transition::StopRecording));
    }

    #[test]
    fn set_mode_while_idle_switches_semantics_without_a_cancel_issue_126() {
        let mut sm = StateMachine::new(Mode::Hold, [1], Duration::from_millis(300));

        assert_eq!(
            sm.set_mode(Mode::Toggle),
            None,
            "idle switch has nothing to cancel"
        );
        assert_eq!(sm.mode(), Mode::Toggle);

        // Prove toggle semantics are actually live: release is ignored, the
        // next press stops.
        let start = sm.handle(KeyEvent::KeyDown(1, ms(0)));
        let release_noop = sm.handle(KeyEvent::KeyUp(1, ms(50)));
        let stop = sm.handle(KeyEvent::KeyDown(1, ms(1_000)));
        assert_eq!(start, Some(Transition::StartRecording));
        assert_eq!(release_noop, None);
        assert_eq!(stop, Some(Transition::StopRecording));
    }

    #[test]
    fn set_mode_cancels_an_in_flight_hold_session_issue_126() {
        let mut sm = StateMachine::new(Mode::Hold, [1], Duration::from_millis(300));
        assert_eq!(
            sm.handle(KeyEvent::KeyDown(1, ms(0))),
            Some(Transition::StartRecording)
        );

        // Mid-hold mode change: the session is cancelled (caller discards
        // audio, same as the debounce/reset paths) and the machine is left
        // unwedged under the new mode.
        assert_eq!(sm.set_mode(Mode::Toggle), Some(Transition::Cancelled));
        assert_eq!(sm.mode(), Mode::Toggle);

        // The old session's dangling KeyUp is inert (held set was cleared)…
        assert_eq!(sm.handle(KeyEvent::KeyUp(1, ms(100))), None);
        // …and a fresh press starts a brand-new toggle session.
        assert_eq!(
            sm.handle(KeyEvent::KeyDown(1, ms(1_000))),
            Some(Transition::StartRecording)
        );
    }

    #[test]
    fn set_mode_cancels_an_in_flight_toggle_session_issue_126() {
        let mut sm = StateMachine::new(Mode::Toggle, [1], Duration::from_millis(300));
        assert_eq!(
            sm.handle(KeyEvent::KeyDown(1, ms(0))),
            Some(Transition::StartRecording)
        );
        sm.handle(KeyEvent::KeyUp(1, ms(50))); // toggle ignores release

        assert_eq!(sm.set_mode(Mode::Hold), Some(Transition::Cancelled));
        assert_eq!(sm.mode(), Mode::Hold);

        // Hold semantics are live: press + long hold + release is one
        // dictation.
        assert_eq!(
            sm.handle(KeyEvent::KeyDown(1, ms(1_000))),
            Some(Transition::StartRecording)
        );
        assert_eq!(
            sm.handle(KeyEvent::KeyUp(1, ms(2_000))),
            Some(Transition::StopRecording)
        );
    }

    // Keys outside the configured chord must be inert.
    #[test]
    fn unrelated_keys_are_ignored() {
        let mut sm = StateMachine::new(Mode::Hold, [1], Duration::from_millis(300));

        let down = sm.handle(KeyEvent::KeyDown(99, ms(0)));
        let up = sm.handle(KeyEvent::KeyUp(99, ms(10)));

        assert_eq!(down, None);
        assert_eq!(up, None);
    }
}
