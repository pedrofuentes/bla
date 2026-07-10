import { useEffect, useReducer, useState } from "react";
import { onEvent } from "../../lib/ipc";
import { pushLevel } from "../../lib/levelBuffer";
import { initialPillState, pillLabel, pillReducer, type PillMode } from "../../lib/pillState";
import type { PipelineState } from "../../lib/status";
import { toastForError, type Toast as ToastSpec } from "../../lib/toast";
import { barsFromLevels } from "../../lib/waveform";
import { PillWaveform } from "./PillWaveform";
import { PipelineErrorToast } from "./Toast";

/**
 * Recording pill — small always-on-top window with a live waveform, shown
 * while dictating (see docs/ARCHITECTURE.md §Project Structure).
 *
 * Talks to the core only through `src/lib/ipc.ts`, so this window renders in
 * a plain browser (mocked IPC) for Playwright visual verification.
 *
 * Real content (issue #126, M2 PR 2.3), replacing the #127 placeholder
 * shell: `pipeline-state-changed` drives `pillReducer` (`src/lib/pillState.ts`)
 * for the pill's mode (recording/transcribing/done/error, with a ~1.5s
 * auto-hide back to idle after "done"); `audio-level` feeds a fixed-size
 * ring buffer (`pushLevel`, `src/lib/levelBuffer.ts`) that `barsFromLevels`
 * (`src/lib/waveform.ts`) lays out for the canvas waveform (`PillWaveform`,
 * an untested thin render layer — all its layout decisions live in the
 * tested `barsFromLevels`). `lib.rs::set_pipeline_state` still owns
 * showing/hiding the real OS window (`tray::pill_visibility_for`); this
 * component only decides what to render while it's visible. Only ever
 * renders fixed state labels (`pillLabel`) — never transcript/clipboard
 * text (MISSION.md §7).
 *
 * Issue #126, M2 PR 2.4: also listens for `pipeline-error` and renders a
 * transient toast (`Toast.tsx`) as a sibling of the waveform/dot pill — the
 * only decision logic here (which tone/message to show) lives in the pure
 * `toastForError` helper, unit-tested separately (`src/lib/toast.test.ts`).
 */

const BAR_COUNT = 24;
/** How often a "tick" is dispatched so the reducer's "done" auto-hide can fire (ms). */
const TICK_INTERVAL_MS = 250;

/** Tailwind classes for the small status dot shown outside "recording" mode. */
const DOT_CLASSES: Record<Exclude<PillMode, "recording">, string> = {
  idle: "bg-neutral-400",
  transcribing: "animate-pulse bg-amber-400",
  done: "bg-emerald-400",
  error: "bg-red-500",
};

export function PillWindow() {
  const [state, dispatch] = useReducer(pillReducer, initialPillState);
  const [levels, setLevels] = useState<number[]>([]);
  const [toast, setToast] = useState<ToastSpec | null>(null);

  useEffect(() => {
    let cancelled = false;

    const unlisten = Promise.all([
      onEvent("pipeline-state-changed", (payload) => {
        if (cancelled) return;
        const pipelineState = payload as PipelineState;
        // Fresh bars for the next recording rather than a stale tail from
        // the previous one.
        if (pipelineState === "Active") setLevels([]);
        dispatch({ type: "pipeline-state", state: pipelineState, now: Date.now() });
      }),
      onEvent("audio-level", (level) => {
        if (!cancelled) setLevels((buf) => pushLevel(buf, level));
      }),
      onEvent("pipeline-error", (event) => {
        if (!cancelled) setToast(toastForError(event));
      }),
    ]);

    return () => {
      cancelled = true;
      unlisten.then((fns) => fns.forEach((fn) => fn()));
    };
  }, []);

  useEffect(() => {
    const id = window.setInterval(
      () => dispatch({ type: "tick", now: Date.now() }),
      TICK_INTERVAL_MS,
    );
    return () => window.clearInterval(id);
  }, []);

  const label = pillLabel(state.mode);

  return (
    <div className="relative flex h-screen w-screen items-center justify-center bg-transparent">
      <div className="flex items-center gap-2 rounded-full bg-neutral-900/90 px-4 py-2 text-neutral-100 shadow-lg">
        {state.mode === "recording" ? (
          <PillWaveform bars={barsFromLevels(levels, BAR_COUNT)} />
        ) : (
          <span
            aria-hidden
            className={`h-2.5 w-2.5 shrink-0 rounded-full ${DOT_CLASSES[state.mode]}`}
          />
        )}
        {label && <span className="text-sm font-medium whitespace-nowrap">{label}</span>}
      </div>
      {toast && <PipelineErrorToast toast={toast} onDismiss={() => setToast(null)} />}
    </div>
  );
}
