/**
 * Pure display-logic for the pill window's `pipeline-error` toast (issue
 * #126, M2 PR 2.4). Free of any Tauri/DOM/React dependency (mirrors
 * `src/lib/status.ts`'s pattern), so the tone decision is unit-testable
 * without a live Tauri app context; `src/windows/pill/Toast.tsx` stays a
 * thin consumer of {@link toastForError}'s output.
 */
import type { PipelineErrorEvent } from "./ipc";

/**
 * How a toast is styled: `"informational"` for the AC-4 Ollama-unreachable
 * fallback (the dictation still completed and pasted — this is a heads-up,
 * not a failure); `"blocking"` for everything else, where the pipeline
 * could not complete this dictation at all.
 */
export type ToastTone = "informational" | "blocking";

export interface Toast {
  tone: ToastTone;
  message: string;
}

/**
 * Maps a `pipeline-error` event payload to the pill toast's display spec.
 * `"OllamaUnreachable"` is the only informational kind (mirrors
 * `errors::ErrorKind::is_blocking` on the Rust side); every other kind —
 * including any not-yet-known future kind, so an unrecognized value fails
 * safe rather than under-alarming the user — is blocking. `message` passes
 * through unchanged: the Rust side already guarantees it's static and
 * kind-derived, never transcript/clipboard/audio content.
 */
export function toastForError(event: PipelineErrorEvent): Toast {
  return {
    tone: event.kind === "OllamaUnreachable" ? "informational" : "blocking",
    message: event.message,
  };
}
