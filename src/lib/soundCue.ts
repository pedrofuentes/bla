/**
 * Pure decision logic mapping a `pipeline-state-changed` transition to a
 * sound cue (issue #126, M2 PR 2.7). Free of any Web Audio/Tauri/DOM
 * dependency -- mirrors `src/lib/toast.ts`'s pure-decision/thin-renderer
 * split -- so both "which cue fires" and "is playback gated off by the
 * `sound_cues` preference" are unit-testable without a live AudioContext.
 * `src/windows/pill/index.tsx` is the only caller; `src/lib/soundCuePlayer.ts`'s
 * `playCue` is the only place that actually touches Web Audio.
 */
import type { PipelineState } from "./status";

/** The minimal cue set: a dictation started, finished successfully, or errored. */
export type CueKind = "start" | "done" | "error";

/**
 * Maps a `pipeline-state-changed` transition to a {@link CueKind}, or `null`
 * for no cue. Mirrors `pillState.ts`'s `applyPipelineState` semantics so the
 * cue and the pill's visual state never disagree:
 *  - `Idle -> Active` or `Unknown -> Active`: `"start"` (a dictation began).
 *    `Unknown` (the pill's client-only "no event seen yet" placeholder) is
 *    treated the same as `Idle` here because the pill window is only shown
 *    once a dictation is already active (`tray::pill_visibility_for`), so
 *    `Unknown -> Active` -- not `Idle -> Active` -- is the transition most
 *    dictations actually deliver as this component's *first* event.
 *  - `Busy -> Idle`: `"done"` (a completed dictation -- the same transition
 *    `pillReducer` treats as entering `"done"`).
 *  - `Active -> Idle`: no cue (a cancelled dictation -- `pillReducer` skips
 *    the "done" flash for the same transition, so no chime either).
 *  - anything `-> Error`: `"error"`, regardless of the prior state.
 *  - `Error -> Idle` (recovery), the transcribing tick (`Active -> Busy`),
 *    `Unknown -> Idle`/`Idle -> Unknown`, or no change at all: no cue.
 */
export function cueForTransition(prev: PipelineState, next: PipelineState): CueKind | null {
  if (prev === next) return null;
  if (next === "Error") return "error";
  if ((prev === "Idle" || prev === "Unknown") && next === "Active") return "start";
  if (prev === "Busy" && next === "Idle") return "done";
  return null;
}

/**
 * Gates a computed cue behind the `sound_cues` preference (`Settings`,
 * `src/lib/ipc.ts`). Pulled out as its own pure step -- rather than inlined
 * in the pill -- so the gated-off case is exercised by the same unit tests
 * as the transition matrix, without needing a live Settings load or
 * AudioContext.
 */
export function shouldPlayCue(cue: CueKind | null, soundCuesEnabled: boolean): boolean {
  return cue !== null && soundCuesEnabled;
}
