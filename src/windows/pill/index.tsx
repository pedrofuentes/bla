import { useEffect, useState } from "react";
import { onEvent } from "../../lib/ipc";
import { toastForError, type Toast as ToastSpec } from "../../lib/toast";
import { PipelineErrorToast } from "./Toast";

/**
 * Recording pill — small always-on-top window with a live waveform, shown
 * while dictating (see docs/ARCHITECTURE.md §Project Structure).
 *
 * Talks to the core only through `src/lib/ipc.ts`, so this window renders in
 * a plain browser (mocked IPC) for Playwright visual verification.
 *
 * Placeholder shell (issue #126, M2 PR 2.1): a window-appropriately-shaped
 * pill with a neutral idle dot — no live waveform/pipeline-state wiring yet,
 * that lands in a later M2 increment. `lib.rs::set_pipeline_state` already
 * shows/hides the real OS window around whatever this renders
 * (`tray::pill_visibility_for`).
 *
 * Issue #126, M2 PR 2.4: listens for `pipeline-error` and renders a
 * transient toast (`Toast.tsx`) over the placeholder — the only decision
 * logic here (which tone/message to show) lives in the pure `toastForError`
 * helper, unit-tested separately (`src/lib/toast.test.ts`).
 */
export function PillWindow() {
  const [toast, setToast] = useState<ToastSpec | null>(null);

  useEffect(() => {
    let cancelled = false;
    const unlisten = onEvent("pipeline-error", (event) => {
      if (!cancelled) setToast(toastForError(event));
    });
    return () => {
      cancelled = true;
      unlisten.then((fn) => fn());
    };
  }, []);

  return (
    <div className="relative flex h-screen w-screen items-center justify-center bg-neutral-900/90">
      <div className="flex items-center gap-2 rounded-full px-4 py-2 text-neutral-100">
        <span aria-hidden className="h-2.5 w-2.5 shrink-0 rounded-full bg-neutral-400" />
        <span className="text-sm font-medium">bla</span>
      </div>
      {toast && <PipelineErrorToast toast={toast} onDismiss={() => setToast(null)} />}
    </div>
  );
}
