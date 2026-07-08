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
