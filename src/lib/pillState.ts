/**
 * Pure state machine translating `pipeline-state-changed` events (plus a
 * periodic `tick`) into the recording pill's UI mode (issue #126, M2 PR
 * 2.3). No `Date.now`/timers here -- every timestamp is caller-supplied, so
 * this is fully deterministic and unit-testable; `src/windows/pill/index.tsx`
 * is the only place that calls `Date.now()`/`setInterval`.
 */
import type { PipelineState } from "./status";

/** The five states the pill's content distinguishes. */
export type PillMode = "idle" | "recording" | "transcribing" | "done" | "error";

export interface PillState {
  mode: PillMode;
  /** `now` (ms, injected) when `mode` became `"done"`; `null` otherwise -- drives auto-hide. */
  doneAt: number | null;
}

export type PillAction =
  { type: "pipeline-state"; state: PipelineState; now: number } | { type: "tick"; now: number };

/** How long the pill lingers on `"done"` before the reducer reverts to `"idle"` (ms). */
export const DONE_AUTO_HIDE_MS = 1500;

export const initialPillState: PillState = { mode: "idle", doneAt: null };

/**
 * `"done"` is reached only via a Busy -> Idle transition (a completed
 * dictation, matching `lib.rs::run_pipeline_in_background`'s success arm)
 * and self-clears after `DONE_AUTO_HIDE_MS` on a subsequent `"tick"`. A
 * Recording -> Idle transition (a cancelled dictation,
 * `hotkeys::Transition::Cancelled` in lib.rs) goes straight to `"idle"` --
 * cancelling isn't a completion worth flashing. Any other state
 * (Active/Busy/Error) always wins outright and clears a stale `doneAt`.
 */
export function pillReducer(state: PillState, action: PillAction): PillState {
  switch (action.type) {
    case "pipeline-state":
      return applyPipelineState(state, action.state, action.now);
    case "tick":
      return applyTick(state, action.now);
  }
}

function applyPipelineState(
  state: PillState,
  pipelineState: PipelineState,
  now: number,
): PillState {
  switch (pipelineState) {
    case "Active":
      return { mode: "recording", doneAt: null };
    case "Busy":
      return { mode: "transcribing", doneAt: null };
    case "Error":
      return { mode: "error", doneAt: null };
    case "Idle":
      return state.mode === "transcribing"
        ? { mode: "done", doneAt: now }
        : { mode: "idle", doneAt: null };
    case "Unknown":
      return { mode: "idle", doneAt: null };
    default:
      // Fail-safe (defense in depth): callers guard with
      // parsePipelineState, but never return undefined for an unexpected
      // value -- the next state.mode read would crash the render tree.
      return { mode: "idle", doneAt: null };
  }
}

function applyTick(state: PillState, now: number): PillState {
  if (state.mode === "done" && state.doneAt !== null && now - state.doneAt >= DONE_AUTO_HIDE_MS) {
    return { mode: "idle", doneAt: null };
  }
  return state;
}

/** Short label for `mode` -- state labels only, never transcript/clipboard text. */
export function pillLabel(mode: PillMode): string {
  switch (mode) {
    case "idle":
      return "";
    case "recording":
      return "Recording…";
    case "transcribing":
      return "Transcribing…";
    case "done":
      return "Done";
    case "error":
      return "Something went wrong";
  }
}
