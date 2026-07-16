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
 * fallback and issue #220's history-persist-failure notice (both cases
 * where the dictation still completed and pasted — a heads-up, not a
 * failure); `"blocking"` for everything else, where the pipeline could not
 * complete this dictation at all.
 */
export type ToastTone = "informational" | "blocking";

export interface Toast {
  tone: ToastTone;
  message: string;
}

/** Kinds `toastForError` renders as an `"informational"` toast (mirrors `errors::ErrorKind::is_blocking` on the Rust side). */
const INFORMATIONAL_KINDS = new Set(["OllamaUnreachable", "HistoryPersistFailed"]);

/**
 * Maps a `pipeline-error` event payload to the pill toast's display spec.
 * Every kind not in {@link INFORMATIONAL_KINDS} — including any
 * not-yet-known future kind, so an unrecognized value fails safe rather
 * than under-alarming the user — is blocking. `message` passes through
 * unchanged: the Rust side already guarantees it's static and
 * kind-derived, never transcript/clipboard/audio content.
 */
export function toastForError(event: PipelineErrorEvent): Toast {
  return {
    tone: INFORMATIONAL_KINDS.has(event.kind) ? "informational" : "blocking",
    message: event.message,
  };
}
