//! Tray icon state derivation + output-mode switch-takes-effect-on-the-
//! next-dictation (issue #23, AC-14).
//!
//! All logic in this module is pure — no OS calls, fully deterministic. The
//! real Tauri tray icon/menu rendering (`TrayIconBuilder`, asset paths) is
//! thin OS-glue (AGENTS.md OS-integration exemption): it is not wired up in
//! this increment (`lib.rs::run()` doesn't call into this module yet), kept
//! separate and minimal so it stays TDD-exempt while every decision it will
//! eventually delegate to already has full unit coverage here.
//!
//! Note: `OutputMode` here is a tray-local model of *which target is live*
//! (a plain two-way switch) — distinct from `output::OutputMode`, which
//! additionally carries the file target's resolved config. The two are not
//! wired together in this increment.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icon_state_maps_every_pipeline_state_ac14() {
        assert_eq!(tray_icon_state(&PipelineState::Idle), TrayIconState::Idle);
        assert_eq!(
            tray_icon_state(&PipelineState::Recording),
            TrayIconState::Active
        );
        assert_eq!(
            tray_icon_state(&PipelineState::Transcribing),
            TrayIconState::Busy
        );
        assert_eq!(
            tray_icon_state(&PipelineState::Error),
            TrayIconState::Error
        );
    }

    #[test]
    fn mode_switch_takes_effect_starting_with_the_next_dictation_ac14() {
        let mut switch = OutputModeSwitch::new(OutputMode::CursorPaste);

        // The in-flight dictation reads (captures) the mode before any
        // switch happens.
        let in_flight_target = switch.route_target();
        assert_eq!(in_flight_target, OutputMode::CursorPaste);

        // User flips the mode mid-dictation from the tray menu.
        switch.set_mode(OutputMode::File);

        // The in-flight dictation's already-captured target is unaffected —
        // it's a plain copied value, not a live reference.
        assert_eq!(in_flight_target, OutputMode::CursorPaste);

        // The *next* dictation's route_target() call reflects the new mode.
        assert_eq!(switch.route_target(), OutputMode::File);
    }

    #[test]
    fn mode_switch_can_flip_back_and_forth_across_several_dictations_ac14() {
        let mut switch = OutputModeSwitch::new(OutputMode::File);
        assert_eq!(switch.route_target(), OutputMode::File);

        switch.set_mode(OutputMode::CursorPaste);
        assert_eq!(switch.route_target(), OutputMode::CursorPaste);

        switch.set_mode(OutputMode::File);
        assert_eq!(switch.route_target(), OutputMode::File);
    }
}
