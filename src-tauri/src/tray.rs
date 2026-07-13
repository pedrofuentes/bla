//! Tray icon state derivation + output-mode switch-takes-effect-on-the-
//! next-dictation (issue #23, AC-14).
//!
//! All logic in this module is pure — no OS calls, fully deterministic. The
//! real Tauri tray icon/menu rendering (`TrayIconBuilder`, asset paths,
//! `tauri::menu` wiring) is thin OS-glue (AGENTS.md OS-integration
//! exemption) that lives in `lib.rs::run()`'s `setup()` (issue #110): it
//! builds the tray icon/menu and, on every `set_pipeline_state` call, maps
//! the current `PipelineState` through [`tray_icon_state`] here to decide
//! which bundled icon (`icons/tray/*.png`) and menu-state label to show.
//! Kept separate and minimal so this module stays TDD-exempt while every
//! decision it delegates to already has full unit coverage here.
//!
//! Note: `OutputMode` here is a tray-local model of *which target is live*
//! (a plain two-way switch) — distinct from `output::OutputMode`, which
//! additionally carries the file target's resolved config. The two are not
//! wired together in this increment.

/// Pipeline state as observed by the tray: the overall dictation state
/// machine (hotkey → capture → transcribe → cleanup → output) collapsed to
/// the four states the tray icon distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineState {
    Idle,
    Recording,
    Transcribing,
    Error,
}

/// Which icon variant the tray should render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayIconState {
    Idle,
    Active,
    Busy,
    Error,
}

/// Total, deterministic mapping from [`PipelineState`] to [`TrayIconState`]
/// (AC-14). Pure — no OS calls; the real menu-bar icon swap (thin glue, not
/// wired in this increment) would call this and hand the result to Tauri's
/// tray API.
pub fn tray_icon_state(state: &PipelineState) -> TrayIconState {
    match state {
        PipelineState::Idle => TrayIconState::Idle,
        PipelineState::Recording => TrayIconState::Active,
        PipelineState::Transcribing => TrayIconState::Busy,
        PipelineState::Error => TrayIconState::Error,
    }
}

/// Which output target a dictation is routed to, as toggled from the tray
/// menu (AC-14). See the module doc for how this relates to
/// `output::OutputMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    CursorPaste,
    File,
}

/// Holds the output mode live for dictations completing from this point
/// forward. AC-14: switching the mode from the tray while a dictation is
/// in flight must not change *that* dictation's routing — a caller that
/// already read [`route_target`](Self::route_target) holds a plain copied
/// value, unaffected by a later [`set_mode`](Self::set_mode). The switch
/// only takes effect for `route_target()` calls made *after* it, i.e.
/// starting with the next dictation.
pub struct OutputModeSwitch {
    current: OutputMode,
}

impl OutputModeSwitch {
    /// Start the switch at `initial` (typically `Settings::output_mode`
    /// mapped to this module's `OutputMode`).
    pub fn new(initial: OutputMode) -> Self {
        Self { current: initial }
    }

    /// Request a mode change, effective for every `route_target()` call
    /// from this point on.
    pub fn set_mode(&mut self, mode: OutputMode) {
        self.current = mode;
    }

    /// The mode that should route the dictation currently completing.
    pub fn route_target(&self) -> OutputMode {
        self.current
    }
}

/// Whether the recording pill window should be shown for `state` (issue
/// #126, M2 PR 2.1): visible for every non-`Idle` pipeline state
/// (`Recording`/`Transcribing`/`Error`) and hidden once the pipeline returns
/// to `Idle`. Pure and total (exhaustive `matches!`) so
/// `lib.rs::set_pipeline_state` (thin OS glue) can call this to decide
/// whether to show/hide the real `pill` webview window without embedding the
/// decision itself in Tauri glue. No debounce/delay here yet — the brief
/// defers that to a later increment.
pub fn pill_visibility_for(state: &PipelineState) -> bool {
    !matches!(state, PipelineState::Idle)
}

/// Whether an *elapsed* informational-notice period should now hide the pill
/// (issue #126, M2 PR 2.4; Sentinel 🔴-2 on PR #135). The AC-4
/// Ollama-unreachable toast is informational — the dictation still pasted —
/// so the pill is kept visible for the toast's lifetime even though the
/// pipeline has already settled to `Idle` (where [`pill_visibility_for`]
/// alone would hide it immediately, leaving the toast on a hidden window).
/// Once the notice window elapses, hide the pill **only if the pipeline is
/// still `Idle`**: a dictation started during the notice moves the state to
/// `Recording`/`Transcribing` (or `Error`), and that transition's own
/// `set_pipeline_state` already keeps the pill shown — so the elapsed notice
/// must not hide it, letting the new dictation preempt cleanly. Pure/total
/// so the decision is unit-tested; the sleep + `window.hide()` around it stay
/// thin OS glue in `lib.rs`.
pub fn should_hide_pill_after_notice(state: &PipelineState) -> bool {
    matches!(state, PipelineState::Idle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pill_visibility_for_hides_on_idle_and_shows_otherwise_issue_126() {
        assert!(!pill_visibility_for(&PipelineState::Idle));
        assert!(pill_visibility_for(&PipelineState::Recording));
        assert!(pill_visibility_for(&PipelineState::Transcribing));
        assert!(pill_visibility_for(&PipelineState::Error));
    }

    #[test]
    fn should_hide_pill_after_notice_only_when_still_idle_issue_126() {
        // Sentinel 🔴-2 (PR #135): the AC-4 informational OllamaUnreachable
        // toast is shown while the pipeline settles to Idle; the pill must
        // stay visible for the notice window and only hide afterward IFF the
        // pipeline is *still* Idle. A new dictation started during the notice
        // moves the state off Idle (Recording/Transcribing) — its own
        // `set_pipeline_state` keeps the pill shown, so the elapsed notice
        // must NOT hide it (the new dictation preempts cleanly).
        assert!(should_hide_pill_after_notice(&PipelineState::Idle));
        assert!(!should_hide_pill_after_notice(&PipelineState::Recording));
        assert!(!should_hide_pill_after_notice(&PipelineState::Transcribing));
        assert!(!should_hide_pill_after_notice(&PipelineState::Error));
    }

    #[test]
    fn should_keep_pill_visible_for_done_only_after_an_active_dictation_issue_151() {
        // The completed-dictation transition (issue #151): the pipeline was
        // actively Recording/Transcribing right before settling to Idle, so
        // the "done" confirmation gets a visible pill to render onto.
        assert!(should_keep_pill_visible_for_done(&PipelineState::Recording));
        assert!(should_keep_pill_visible_for_done(
            &PipelineState::Transcribing
        ));
        // Already-Idle or Error aren't a completed-dictation transition —
        // no "done" confirmation is owed.
        assert!(!should_keep_pill_visible_for_done(&PipelineState::Idle));
        assert!(!should_keep_pill_visible_for_done(&PipelineState::Error));
    }

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
        assert_eq!(tray_icon_state(&PipelineState::Error), TrayIconState::Error);
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
