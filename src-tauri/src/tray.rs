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

/// The tray/pill's full display truth at a point in time: the pipeline
/// state plus whether the pill should be visible for it. Bundled into one
/// struct (issue #128) so `lib.rs::AppState` can guard both fields with a
/// SINGLE mutex — `state` and `show_pill` are written together, atomically,
/// every time (`apply_pipeline_state`'s `show_pill` parameter is not always
/// `pill_visibility_for(&state)`; the AC-4 notice / issue-#151 done-settle
/// paths pass `show_pill = true` for an `Idle` state so a toast/confirmation
/// stays visible on a pill the plain rule would otherwise hide). Splitting
/// them into two independently-locked fields would reopen exactly the kind
/// of check-then-act gap this issue closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineDisplay {
    pub state: PipelineState,
    pub show_pill: bool,
}

/// What a `run_on_main_thread` closure should apply to the tray icon and
/// pill visibility, given whatever [`PipelineDisplay`] is CURRENT in
/// `AppState` at the moment the closure actually executes (issue #128).
///
/// **The race this closes:** `apply_pipeline_state` used to compute
/// `show_pill`/the icon state ONCE, at enqueue time, and capture that local
/// value in the `run_on_main_thread` closure. The state write (into
/// `AppState`), that snapshot, and the `run_on_main_thread` enqueue were
/// three separate steps — not atomic as a unit. Two same-generation
/// `apply_pipeline_state` calls (e.g. one from the hotkey thread, one from
/// the pipeline thread) could therefore enqueue closures whose CAPTURED
/// snapshots reflect one chronological order while the main thread executes
/// them in the other order: an older call's closure applying its stale
/// snapshot strictly AFTER a newer call's closure already applied the
/// current truth — leaving the pill stuck visible-while-`Idle`, or hidden
/// while `Recording`, until the next unrelated transition happened to
/// overwrite it.
///
/// The fix: closures no longer carry a captured snapshot at all. Every
/// closure re-derives its `(TrayIconState, show_pill)` by calling this
/// function against whatever `PipelineDisplay` `AppState`'s single mutex
/// currently holds, read at execution time. However many closures end up
/// running, in whatever order, each one applies the SAME current truth —
/// so the on-screen result always matches the true current state the
/// instant the last closure (of however many are in flight) runs, rather
/// than depending on which stale closure happened to run last.
pub fn resolve_display(current: &PipelineDisplay) -> (TrayIconState, bool) {
    (tray_icon_state(&current.state), current.show_pill)
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

/// Whether a Busy → Idle transition should keep the pill visible for a
/// "done" confirmation instead of hiding it in the same call (issue #151).
/// True only when `previous` was actively dictating
/// (`Recording`/`Transcribing`) — i.e. the pipeline just *completed* a
/// dictation, as opposed to a hotkey cancel (which calls
/// `set_pipeline_state(Idle)` directly, never routing through the settle
/// path this guards) or an already-`Idle`/`Error` state. Distinct from
/// [`pill_visibility_for`]`(&Idle)` (always `false`): callers that know a
/// dictation just completed route through `apply_pipeline_state(Idle, true)`
/// instead so the frontend's "done" state (`pillState.ts`) actually gets a
/// visible pill to render onto before it auto-hides. Pure/total so the
/// decision is unit-tested; the grace-window sleep + `window.hide()` stay
/// thin OS glue in `lib.rs`.
pub fn should_keep_pill_visible_for_done(previous: &PipelineState) -> bool {
    matches!(
        previous,
        PipelineState::Recording | PipelineState::Transcribing
    )
}

/// Whether a completed dictation's settle should take the visible-for-
/// notice path (`settle_idle_keeping_pill_for_notice`, [`PILL_NOTICE_DURATION`]
/// = 5s) rather than the plain "done" path
/// ([`should_keep_pill_visible_for_done`] +
/// `settle_idle_keeping_pill_for_done`, [`DONE_PILL_DURATION`] = 1.5s)
/// (issue #220).
///
/// There are now two independent reasons a completed dictation carries an
/// informational toast that needs the *longer* window to be seen:
/// AC-4/ADR-0005's Ollama-fallback notice (`cleanup_fell_back`, PR #135),
/// and issue #220's history-persist-failure notice
/// (`history_persist_failed` — `Store::insert_history` failed, so this
/// dictation's row is missing from history even though the text itself
/// was already pasted/written). Either alone routes through the notice
/// path; the plain "done" path is reserved for a dictation with **no**
/// toast to show, where the shorter 1.5s confirmation window is enough.
/// Pure/total — `run_pipeline_in_background` passes both booleans it
/// already computed, so this stays unit-testable without constructing an
/// `AppState` (issue #165's Windows-CI hard rule).
///
/// [`PILL_NOTICE_DURATION`]: crate::PILL_NOTICE_DURATION
/// [`DONE_PILL_DURATION`]: crate::DONE_PILL_DURATION
pub fn should_settle_with_notice(cleanup_fell_back: bool, history_persist_failed: bool) -> bool {
    cleanup_fell_back || history_persist_failed
}

/// Race-safe guard for a delayed pill-hide started by a "keep the pill
/// visible for a while, then maybe hide it" settle (issue #155; Sentinel 🔴
/// on PR #137's re-review). `settle_idle_keeping_pill_for_notice` (AC-4
/// informational toast) and its issue-#151 sibling for the "done"
/// confirmation both bump a monotonic `AppState` "pill visibility epoch"
/// when they start, then capture that epoch before sleeping. Two such
/// settles can overlap within their windows (a notice and a done-settle, or
/// two notices back to back); `should_hide_pill_after_notice` alone only
/// checks that the pipeline is still `Idle`, which is also (coincidentally)
/// true once a *second*, newer settle has itself already applied `Idle` —
/// so the stale first settle would wrongly hide the pill out from under the
/// newer one's still-live visible window. Hiding now requires ALL of:
/// no newer settle has started since (`epoch_at_settle == current_epoch`),
/// no newer *dictation* has started since (`generation_at_settle ==
/// current_generation` — issues #174/#175/#176: a settle started by
/// dictation #1 must stand down once dictation #2 is underway, the same way
/// it stands down for a newer settle of #1's own dictation), and
/// `should_hide_pill_after_notice` still holds. Pure/total; the actual
/// epoch/generation bump/load and `window.hide()` stay thin OS glue in
/// `lib.rs` — including #174's fix of locking `pipeline_state` BEFORE
/// loading the epoch/generation atomics, which this function's signature
/// doesn't (and can't) enforce, only its caller can.
pub fn should_hide_pill_for_settle(
    epoch_at_settle: u64,
    current_epoch: u64,
    generation_at_settle: u64,
    current_generation: u64,
    state: &PipelineState,
) -> bool {
    generation_at_settle == current_generation
        && epoch_at_settle == current_epoch
        && should_hide_pill_after_notice(state)
}

/// Whether a background dictation's completion — a `run_pipeline_in_background`
/// result arriving, or one of the settle helpers it calls
/// (`settle_idle_keeping_pill_for_notice`/`_for_done`) applying its
/// immediate `Idle`+visible write — should still be applied to shared
/// state, given the per-dictation generation it was minted with
/// (`generation_at_start`, captured once at `StartRecording`) vs. the
/// current live generation (issues #174/#175/#176).
///
/// **Mechanism:** the hotkeys `StateMachine` resets to `Phase::Idle`
/// synchronously on `StopRecording`, before the transcription thread that
/// `StopRecording` kicked off has returned — so a second dictation can
/// already be recording/transcribing by the time the first one's background
/// thread completes. Without a per-dictation identity, that stale
/// completion reads/writes the single shared `AppState.pipeline_state`
/// slot, clobbering the live dictation's state (dropping its waveform,
/// showing the wrong pill chrome, or emitting a stray event) for anywhere
/// from an instant up to the completion's full settle-visibility window
/// (1.5s for the "done" confirmation).
///
/// A generation minted at the START of a dictation is bumped by the NEXT
/// `StartRecording`, so `generation_at_start == current_generation` means
/// no newer dictation has begun — this completion is still the live one.
/// `false` means a stale completion: the caller must no-op entirely (no
/// state write, no event emit, no settle thread spawned) rather than apply
/// any part of its result. Pure/total; the atomic bump/load stays thin OS
/// glue in `lib.rs`.
pub fn should_apply_dictation_completion(
    generation_at_start: u64,
    current_generation: u64,
) -> bool {
    generation_at_start == current_generation
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
    fn should_settle_with_notice_true_when_either_toast_condition_holds_issue_220() {
        // Neither: no toast to show — plain "done" path.
        assert!(!should_settle_with_notice(false, false));
        // Ollama-fallback notice only (PR #135's existing case).
        assert!(should_settle_with_notice(true, false));
        // History-persist-failure notice only (issue #220's new case).
        assert!(should_settle_with_notice(false, true));
        // Both at once — still the notice path, not double-counted.
        assert!(should_settle_with_notice(true, true));
    }

    #[test]
    fn should_hide_pill_for_settle_hides_only_when_idle_and_epoch_and_generation_unchanged_issue_155(
    ) {
        // The normal case: the epoch AND generation captured at settle-start
        // are still current (no newer settle, no newer dictation) and the
        // pipeline is still Idle by the time the delayed hide wakes up.
        assert!(should_hide_pill_for_settle(
            1,
            1,
            1,
            1,
            &PipelineState::Idle
        ));
    }

    #[test]
    fn should_hide_pill_for_settle_stands_down_when_a_newer_settle_started_issue_155() {
        // Issue #155 (overlapping-notice epoch race): capture the epoch a
        // settle started at (1), then simulate a second, newer settle
        // bumping it (2) before the first settle's delayed hide wakes up.
        // Even though the pipeline is (coincidentally) still/again Idle, the
        // stale settle must stand down rather than hide the pill out from
        // under the newer settle's own still-live visible window. Same
        // dictation generation throughout (1) — this is purely an
        // overlapping-settle race, not a new dictation.
        let epoch_at_settle = 1;
        let current_epoch = 2;
        assert!(!should_hide_pill_for_settle(
            epoch_at_settle,
            current_epoch,
            1,
            1,
            &PipelineState::Idle
        ));
    }

    #[test]
    fn should_hide_pill_for_settle_never_hides_while_actively_dictating_issue_155() {
        // Epoch-unchanged alone isn't sufficient — a new dictation started
        // during the window must still preempt cleanly regardless of epoch.
        assert!(!should_hide_pill_for_settle(
            1,
            1,
            1,
            1,
            &PipelineState::Recording
        ));
        assert!(!should_hide_pill_for_settle(
            1,
            1,
            1,
            1,
            &PipelineState::Transcribing
        ));
        assert!(!should_hide_pill_for_settle(
            1,
            1,
            1,
            1,
            &PipelineState::Error
        ));
    }

    // Issues #174/#175/#176: a settle started by dictation #1 must stand
    // down once dictation #2 has already started, even when #1's own epoch
    // is still current (no *overlapping settle* raced it) and the pipeline
    // instantaneously reads back as Idle (a stale read, or a narrow window
    // before #2's own StartRecording write lands) — the generation check is
    // the one guard that catches a stale dictation's completion where the
    // epoch/state checks alone would not.
    #[test]
    fn should_hide_pill_for_settle_stands_down_when_a_newer_dictation_started_issues_174_175_176() {
        let epoch_at_settle = 1;
        let current_epoch = 1; // no overlapping settle
        let generation_at_settle = 1;
        let current_generation = 2; // a newer dictation has begun
        assert!(!should_hide_pill_for_settle(
            epoch_at_settle,
            current_epoch,
            generation_at_settle,
            current_generation,
            &PipelineState::Idle
        ));
    }

    // Table test covering the interleavings called out in #174/#175/#176:
    // hides iff idle AND epoch current AND generation current — any single
    // guard failing must stand the settle down.
    #[test]
    fn should_hide_pill_for_settle_table_issues_174_175_176() {
        struct Case {
            epoch_at_settle: u64,
            current_epoch: u64,
            generation_at_settle: u64,
            current_generation: u64,
            state: PipelineState,
            expected: bool,
            label: &'static str,
        }
        let cases = [
            Case {
                epoch_at_settle: 1,
                current_epoch: 1,
                generation_at_settle: 1,
                current_generation: 1,
                state: PipelineState::Idle,
                expected: true,
                label: "all current, idle -> hides",
            },
            Case {
                epoch_at_settle: 1,
                current_epoch: 2,
                generation_at_settle: 1,
                current_generation: 1,
                state: PipelineState::Idle,
                expected: false,
                label: "stale epoch (overlapping settle) -> stands down",
            },
            Case {
                epoch_at_settle: 1,
                current_epoch: 1,
                generation_at_settle: 1,
                current_generation: 2,
                state: PipelineState::Idle,
                expected: false,
                label: "stale generation (newer dictation) -> stands down",
            },
            Case {
                epoch_at_settle: 1,
                current_epoch: 1,
                generation_at_settle: 1,
                current_generation: 1,
                state: PipelineState::Recording,
                expected: false,
                label: "actively dictating -> never hides",
            },
            Case {
                epoch_at_settle: 1,
                current_epoch: 2,
                generation_at_settle: 1,
                current_generation: 2,
                state: PipelineState::Idle,
                expected: false,
                label: "both stale (overlapping settle AND newer dictation) -> stands down",
            },
        ];
        for case in cases {
            assert_eq!(
                should_hide_pill_for_settle(
                    case.epoch_at_settle,
                    case.current_epoch,
                    case.generation_at_settle,
                    case.current_generation,
                    &case.state,
                ),
                case.expected,
                "case: {}",
                case.label
            );
        }
    }

    // Issues #174/#175/#176: the gate `run_pipeline_in_background` (and the
    // settle helpers it calls) checks before ANY state write / event emit /
    // settle spawn — a stale completion (an earlier dictation superseded by
    // a newer one already in flight) must no-op entirely.
    #[test]
    fn should_apply_dictation_completion_only_when_generation_is_still_live_issues_174_175_176() {
        assert!(should_apply_dictation_completion(1, 1));
        assert!(should_apply_dictation_completion(42, 42));
        // A completion from an earlier dictation (generation 1) arriving
        // after a newer one (generation 2) has already started.
        assert!(!should_apply_dictation_completion(1, 2));
        // Defensive: a generation "from the future" relative to current
        // (shouldn't happen — generations only move forward — but the
        // predicate is exact equality, not <=, so this is naturally false
        // too and never accidentally treated as live).
        assert!(!should_apply_dictation_completion(2, 1));
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
    fn resolve_display_derives_icon_and_pill_from_the_display_struct_issue_128() {
        assert_eq!(
            resolve_display(&PipelineDisplay {
                state: PipelineState::Idle,
                show_pill: false,
            }),
            (TrayIconState::Idle, false)
        );
        assert_eq!(
            resolve_display(&PipelineDisplay {
                state: PipelineState::Recording,
                show_pill: true,
            }),
            (TrayIconState::Active, true)
        );
        // The notice/done-settle override: Idle state but show_pill forced
        // true (issue #151 / Sentinel 🔴-2 on PR #135) — `resolve_display`
        // must pass the stored `show_pill` through verbatim rather than
        // re-deriving it from `pill_visibility_for(&state)`, since that
        // override is exactly why `show_pill` is stored alongside `state`
        // instead of always being computed from it.
        assert_eq!(
            resolve_display(&PipelineDisplay {
                state: PipelineState::Idle,
                show_pill: true,
            }),
            (TrayIconState::Idle, true)
        );
    }

    // Issue #128 (Sentinel SNTL-20260716-bla-PR247-6c61cf4 🔴-2): an earlier
    // version of this test asserted `resolve_display(&current) ==
    // resolve_display(&current)` — the literal same expression evaluated
    // twice, which cannot fail no matter what `resolve_display` does or
    // whether the #128 fix is even in place (confirmed by mutation
    // testing). This version instead builds actual Rust closures — the
    // same idiom `apply_pipeline_state`'s `run_on_main_thread` argument
    // uses — over a shared mutable slot (`std::cell::Cell<PipelineDisplay>`,
    // standing in for `AppState::pipeline_display`'s mutex; plain std, not
    // an `AppState` literal or `tauri::Wry` type, per the #165 Windows-CI
    // rule) and exercises BOTH ways such a closure could obtain its
    // `PipelineDisplay`:
    //   - "captured" closures: `move || captured_value` — read the slot
    //     ONCE, at the moment the closure is built (right when its own
    //     write lands), then carry that value forward regardless of what
    //     happens afterward. This is exactly what the PRE-#128-FIX
    //     `apply_pipeline_state` did by closing over a local `show_pill`.
    //   - "reread" closures: `|| slot.get()` — hold a reference to the
    //     slot and read it FRESH every time they're called. This is what
    //     the FIX does: `apply_pipeline_state`'s closure calls
    //     `AppState::pipeline_display.lock()` itself, inside the closure
    //     body, at whatever moment `run_on_main_thread` actually invokes
    //     it.
    // One closure of each kind is built per write in a same-generation
    // sequence — mirroring one `apply_pipeline_state` call enqueuing one
    // `run_on_main_thread` closure per transition — and every closure is
    // invoked only AFTER every write in the sequence has landed, modeling
    // the out-of-order scenario from #128: whichever closure happens to run
    // last on the main thread does so strictly after every state write,
    // regardless of which call originally enqueued it.
    //
    // Because `captured` and `reread` are genuinely different closures
    // (one holds a value, the other holds a reference and re-derefs), this
    // is not tautological: swapping which kind is used changes what the
    // assertions below observe, so a real regression from "reread" back to
    // "captured" in the production closure is the same code shape as this
    // test's `captured` arm — which the assertions are built to catch.
    // Discrimination verified by hand: temporarily building the "reread"
    // closures as `captured` too (i.e. reverting the mechanism) turns the
    // `reread`-side assertions red; restoring `|| slot.get()` turns them
    // green again (see the PR's discrimination-proof transcript).
    #[test]
    fn closures_that_reread_the_slot_track_truth_while_captured_ones_go_stale_issue_128() {
        struct Case {
            label: &'static str,
            // The sequence of writes landing into the shared slot, in the
            // order they actually happen (chronological write order).
            writes: &'static [PipelineDisplay],
        }
        let cases = [
            Case {
                label: "pill stuck visible-while-Idle: Recording(visible) then Idle(hidden) — \
                        an older 'Recording' closure must not re-show the pill after a newer \
                        'Idle' write has already landed",
                writes: &[
                    PipelineDisplay {
                        state: PipelineState::Recording,
                        show_pill: true,
                    },
                    PipelineDisplay {
                        state: PipelineState::Idle,
                        show_pill: false,
                    },
                ],
            },
            Case {
                label: "pill stuck hidden-while-Recording: Idle(hidden) then Recording(visible) \
                        — an older 'Idle' closure must not re-hide the pill after a newer \
                        'Recording' write has already landed",
                writes: &[
                    PipelineDisplay {
                        state: PipelineState::Idle,
                        show_pill: false,
                    },
                    PipelineDisplay {
                        state: PipelineState::Recording,
                        show_pill: true,
                    },
                ],
            },
            Case {
                label: "three-deep interleaving: Recording -> Transcribing -> Idle(notice, \
                        forced visible)",
                writes: &[
                    PipelineDisplay {
                        state: PipelineState::Recording,
                        show_pill: true,
                    },
                    PipelineDisplay {
                        state: PipelineState::Transcribing,
                        show_pill: true,
                    },
                    PipelineDisplay {
                        state: PipelineState::Idle,
                        show_pill: true,
                    },
                ],
            },
        ];

        for case in cases {
            assert!(
                case.writes.len() > 1,
                "case '{}': needs at least two writes to exercise a stale-vs-fresh closure",
                case.label
            );

            let slot = std::cell::Cell::new(case.writes[0]);

            // Build one closure of each kind per write, at the moment that
            // write lands — mirroring `apply_pipeline_state` enqueuing a
            // `run_on_main_thread` closure on every transition.
            let mut captured_closures: Vec<(PipelineDisplay, Box<dyn Fn() -> PipelineDisplay>)> =
                Vec::new();
            let mut reread_closures: Vec<Box<dyn Fn() -> PipelineDisplay + '_>> = Vec::new();
            for write in case.writes {
                slot.set(*write);
                let captured_value = slot.get();
                captured_closures.push((*write, Box::new(move || captured_value)));
                reread_closures.push(Box::new(|| slot.get()));
            }

            let final_value = *case.writes.last().unwrap();
            assert_eq!(
                slot.get(),
                final_value,
                "case '{}': every write must have landed in the slot before any closure runs",
                case.label
            );
            let expected = resolve_display(&final_value);

            // Every closure "executes" now, strictly after every write —
            // the out-of-order-relative-to-enqueue-time scenario from #128.
            for (enqueuing_write, closure) in &captured_closures {
                let applied = resolve_display(&closure());
                if *enqueuing_write != final_value {
                    assert_ne!(
                        applied, expected,
                        "case '{}': a captured closure enqueued by stale write {enqueuing_write:?} \
                         must apply that stale value, diverging from the true final display \
                         {final_value:?} — this is the #128 bug this closure kind reproduces",
                        case.label
                    );
                }
            }
            for closure in &reread_closures {
                let applied = resolve_display(&closure());
                assert_eq!(
                    applied, expected,
                    "case '{}': a closure that re-reads the slot at execution time must always \
                     match the true final display {final_value:?}, regardless of which write \
                     originally enqueued it — this is the #128 fix",
                    case.label
                );
            }
        }
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
