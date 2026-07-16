import { useEffect, useReducer, useRef, useState, type ReactNode } from "react";
import { invoke, onEvent } from "../../lib/ipc";
import { pushLevel } from "../../lib/levelBuffer";
import { initialPillState, pillLabel, pillReducer, type PillMode } from "../../lib/pillState";
import { cueForTransition, shouldPlayCue } from "../../lib/soundCue";
import { playCue } from "../../lib/soundCuePlayer";
import { parsePipelineState, type PipelineState } from "../../lib/status";
import { toastForError, type Toast as ToastSpec } from "../../lib/toast";
import { barsFromLevels, scaleLevelForDisplay } from "../../lib/waveform";
import {
  allListenersFailed,
  withListenerFailed,
  withListenerRecovered,
  type ListenerName,
} from "./listenerHealth";
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
 * ring buffer (`pushLevel`, `src/lib/levelBuffer.ts`, which keeps the raw
 * RMS) that `barsFromLevels` (`src/lib/waveform.ts`) lays out for the canvas
 * waveform, then `scaleLevelForDisplay` (also `waveform.ts`; issue #179)
 * applies a perceptual gain so the pill's bars actually move -- raw speech
 * RMS is small enough (~0.01-0.09) that `PillWaveform`'s linear
 * `level * HEIGHT` would floor every bar to its minimum height. Both are
 * `PillWaveform`'s only inputs (an untested thin render layer — all its
 * layout and scaling decisions live in the tested `waveform.ts`).
 * `lib.rs::set_pipeline_state` still owns
 * showing/hiding the real OS window (`tray::pill_visibility_for`); this
 * component only decides what to render while it's visible. Only ever
 * renders fixed state labels (`pillLabel`) — never transcript/clipboard
 * text (MISSION.md §7).
 *
 * Issue #126, M2 PR 2.4: also listens for `pipeline-error` and renders a
 * transient toast (`Toast.tsx`) as an overlay sibling of the waveform/dot
 * pill (`PillShell`'s `toast` slot) — the only decision logic here (which
 * tone/message to show) lives in the pure `toastForError` helper,
 * unit-tested separately (`src/lib/toast.test.ts`).
 *
 * Event subscriptions (Sentinel 🔴, PR #137; fixed for per-listener degrade
 * in issue #182) are established individually, not via a single
 * `Promise.all`: a rejected subscription — the observable shape of a
 * missing capability grant, exactly what would silently break this window
 * since `src-tauri/capabilities/` only covered the main window — no longer
 * vanishes as an unhandled rejection that kills every listener, and the
 * subscriptions that DID succeed keep their unlisten cleanup on unmount.
 * The pill's own event access is granted by `src-tauri/capabilities/
 * pill.json` (listen/unlisten only).
 *
 * Issue #182 fix: a failed subscription is tracked *per listener*
 * (`failedListeners`, a `Set<ListenerName>`) instead of one sticky
 * `eventsError` flag set on any rejection and never cleared — the original
 * bug meant a single failed `audio-level` subscription blanked the whole
 * pill (including the state dot fed by the still-working
 * `pipeline-state-changed` listener) to a fixed "Status unavailable", and
 * masked whichever subscriptions DID succeed. Now: the "Status unavailable"
 * fallback is reserved for every listener having failed
 * (`allListenersFailed`); a listener that later resolves always clears its
 * own prior failure from the set (so a stale flag can never linger past a
 * successful (re)subscription); and while only `audio-level` has failed,
 * `state.mode === "recording"` falls back to the state dot instead of a
 * `PillWaveform` that would only ever render a flat, feature-dead line with
 * no data to draw. Each rejection's reason (a string, e.g. "event.listen
 * not allowed") is logged via `console.error` for diagnosis (ties to #152)
 * — never the event payload itself, since some payloads (audio-level,
 * pipeline-error) are pipeline-derived and MISSION.md §7 forbids logging
 * anything derived from audio/transcript content.
 *
 * Issue #126, M2 PR 2.7: also plays a short synthesized sound cue on each
 * `pipeline-state-changed` transition, gated by the `sound_cues` preference
 * (`Settings`, persisted since PR 2.6). The *decision* of which cue (if any)
 * fires -- `cueForTransition` -- and whether it's allowed to play --
 * `shouldPlayCue` -- are pure, unit-tested helpers (`src/lib/soundCue.ts`);
 * only the actual Web Audio synthesis (`playCue`,
 * `src/lib/soundCuePlayer.ts`) is untested glue. `sound_cues` is read once
 * via `get_settings` on mount (the same command the settings window's
 * General tab already calls -- app commands aren't capability-gated, so no
 * `pill.json` change is needed); unlike `output_mode`, there's no
 * `settings-changed`-style event to stay live on, so a toggle in the
 * settings window only takes effect for the pill's *next* mount (i.e. next
 * app launch) rather than mid-session -- acceptable for a cosmetic
 * preference, and documented here rather than adding a new event for it.
 */

const BAR_COUNT = 24;
/** How often a "tick" is dispatched so the reducer's "done" auto-hide can fire (ms). */
const TICK_INTERVAL_MS = 250;

/**
 * Tailwind classes for the small status dot. Doubles as the "recording"
 * mode's fallback (issue #182) when the `audio-level` subscription has
 * failed and there's no live level data to paint a waveform with.
 */
const DOT_CLASSES: Record<PillMode, string> = {
  idle: "bg-neutral-400",
  recording: "animate-pulse bg-sky-400",
  transcribing: "animate-pulse bg-amber-400",
  done: "bg-emerald-400",
  error: "bg-red-500",
};

/**
 * The pill bubble chrome (transparent page + rounded dark bubble), plus an
 * optional `toast` overlay rendered as a sibling of the bubble so it composes
 * without restructuring the tree. Kept as a wrapper so both the normal
 * content and the subscription-failure fallback render identically shaped;
 * the outer `<div>` stays the tree's single top-level element.
 */
function PillShell({ children, toast }: { children: ReactNode; toast?: ReactNode }) {
  return (
    <div className="relative flex h-screen w-screen items-center justify-center bg-transparent">
      <div className="flex items-center gap-2 rounded-full bg-neutral-900/90 px-4 py-2 text-neutral-100 shadow-lg">
        {children}
      </div>
      {toast}
    </div>
  );
}

export function PillWindow() {
  const [state, dispatch] = useReducer(pillReducer, initialPillState);
  const [levels, setLevels] = useState<number[]>([]);
  const [toast, setToast] = useState<ToastSpec | null>(null);
  // Issue #182: per-listener, not one sticky flag -- see the class doc
  // above for why. A listener's own successful (re)subscription always
  // removes it from this set.
  const [failedListeners, setFailedListeners] = useState<ReadonlySet<ListenerName>>(new Set());
  // Refs, not state: read inside the pipeline-state-changed handler without
  // needing either value to trigger a re-render on its own.
  const soundCuesEnabledRef = useRef(false);
  const prevPipelineStateRef = useRef<PipelineState>("Unknown");

  useEffect(() => {
    let cancelled = false;
    // Mount-time-only read (see the class doc above for why this doesn't
    // stay live for a mid-session toggle). Left at its `false` initial value
    // -- rather than defaulting true to match `Settings::default` -- if this
    // fails or hasn't resolved yet, so a cue can never fire ahead of
    // actually knowing the user's preference.
    invoke("get_settings")
      .then((settings) => {
        if (!cancelled) soundCuesEnabledRef.current = settings.sound_cues;
      })
      .catch(() => {
        // Sound cues are a cosmetic nicety, not core dictation
        // functionality -- silently stay gated-off rather than surfacing a
        // second, unrelated error state alongside `eventsError`.
      });

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;

    // Sentinel 🔴 (PR #137): NOT a single Promise.all — one rejected
    // subscription (the observable shape of a capability/ACL failure) must
    // neither hide the failure (issue #182: surfaced per-listener via
    // `failedListeners`, logged to the console) nor discard the unlisten
    // cleanup of the subscriptions that succeeded.
    const active: Array<() => void> = [];
    const subscriptions: Array<{ name: ListenerName; promise: Promise<() => void> }> = [
      {
        name: "pipeline-state-changed",
        promise: onEvent("pipeline-state-changed", (payload) => {
          if (cancelled) return;
          const pipelineState = parsePipelineState(payload);
          const cue = cueForTransition(prevPipelineStateRef.current, pipelineState);
          prevPipelineStateRef.current = pipelineState;
          if (cue && shouldPlayCue(cue, soundCuesEnabledRef.current)) playCue(cue);
          // Fresh bars for the next recording rather than a stale tail from
          // the previous one.
          if (pipelineState === "Active") setLevels([]);
          dispatch({ type: "pipeline-state", state: pipelineState, now: Date.now() });
        }),
      },
      {
        name: "audio-level",
        promise: onEvent("audio-level", (level) => {
          if (!cancelled) setLevels((buf) => pushLevel(buf, level));
        }),
      },
      {
        name: "pipeline-error",
        promise: onEvent("pipeline-error", (event) => {
          if (!cancelled) setToast(toastForError(event));
        }),
      },
    ];
    for (const { name, promise } of subscriptions) {
      promise
        .then((unlisten) => {
          // Resolved after unmount: unsubscribe immediately instead of
          // pushing to a list nobody will drain.
          if (cancelled) {
            unlisten();
            return;
          }
          active.push(unlisten);
          // Clear only this listener's own prior failure -- a differently-
          // ordered success settling after another listener's rejection
          // must never leave a stale failure behind for a listener that
          // just subscribed fine.
          setFailedListeners((prev) => withListenerRecovered(prev, name));
        })
        .catch((err: unknown) => {
          if (cancelled) return;
          // Log the rejection *reason* only -- never any event payload
          // (MISSION.md §7: audio/transcript-derived content must never be
          // logged). `err` here is `listen()`'s rejection, e.g. an ACL
          // failure message -- not pipeline data.
          const reason = err instanceof Error ? err.message : String(err);
          console.error(`[pill] "${name}" event subscription failed:`, reason);
          setFailedListeners((prev) => withListenerFailed(prev, name));
        });
    }

    return () => {
      cancelled = true;
      for (const unlisten of active) unlisten();
    };
  }, []);

  useEffect(() => {
    const id = window.setInterval(
      () => dispatch({ type: "tick", now: Date.now() }),
      TICK_INTERVAL_MS,
    );
    return () => window.clearInterval(id);
  }, []);

  const toastNode = toast && <PipelineErrorToast toast={toast} onDismiss={() => setToast(null)} />;

  // Issue #182: reserved for every listener having failed -- a single
  // failed subscription degrades only the feature it feeds (see below),
  // never the whole pill.
  if (allListenersFailed(failedListeners)) {
    return (
      <PillShell toast={toastNode}>
        <span aria-hidden className="h-2.5 w-2.5 shrink-0 rounded-full bg-red-500" />
        <span data-testid="events-error" className="text-sm font-medium whitespace-nowrap">
          Status unavailable
        </span>
      </PillShell>
    );
  }

  const label = pillLabel(state.mode);
  // A failed audio-level subscription means `levels` can never receive
  // data -- rendering `PillWaveform` anyway would just paint a permanently
  // flat, feature-dead line, so fall back to the state dot (issue #182)
  // instead, same as every non-recording mode already does.
  const showWaveform = state.mode === "recording" && !failedListeners.has("audio-level");

  return (
    <PillShell toast={toastNode}>
      {showWaveform ? (
        <PillWaveform bars={barsFromLevels(levels, BAR_COUNT).map(scaleLevelForDisplay)} />
      ) : (
        <span
          aria-hidden
          data-testid="pill-status-dot"
          className={`h-2.5 w-2.5 shrink-0 rounded-full ${DOT_CLASSES[state.mode]}`}
        />
      )}
      {label && <span className="text-sm font-medium whitespace-nowrap">{label}</span>}
    </PillShell>
  );
}
