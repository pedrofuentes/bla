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
