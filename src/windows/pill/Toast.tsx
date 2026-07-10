import { useEffect } from "react";
import type { Toast as ToastSpec } from "../../lib/toast";

/** How long a toast stays visible before auto-dismissing. */
export const TOAST_AUTO_DISMISS_MS = 5000;

/** Tailwind classes distinguishing informational from blocking toasts. */
const TONE_CLASSES: Record<ToastSpec["tone"], string> = {
  informational: "border-blue-400/50 bg-blue-950/90 text-blue-100",
  blocking: "border-red-400/50 bg-red-950/90 text-red-100",
};

export interface PipelineErrorToastProps {
  toast: ToastSpec;
  onDismiss: () => void;
}

/**
 * Small transient toast for the pill window (issue #126, M2 PR 2.4): shows
 * `toast.message`, auto-dismisses after {@link TOAST_AUTO_DISMISS_MS}, and
 * is styled distinctly for `informational` (AC-4 Ollama fallback — the
 * dictation still pasted) vs `blocking` (ModelMissing/MicPermissionDenied/
 * Other) kinds. Self-contained and composable — `src/windows/pill/index.tsx`
 * renders it alongside the pill's placeholder dot; the real waveform UI
 * lands in a later M2 PR without needing to touch this component.
 */
export function PipelineErrorToast({ toast, onDismiss }: PipelineErrorToastProps) {
  useEffect(() => {
    const timer = setTimeout(onDismiss, TOAST_AUTO_DISMISS_MS);
    return () => clearTimeout(timer);
  }, [toast, onDismiss]);

  return (
    <div
      role="status"
      className={`pointer-events-none absolute bottom-2 left-1/2 max-w-[90%] -translate-x-1/2 rounded-md border px-3 py-1.5 text-center text-xs font-medium shadow-lg ${TONE_CLASSES[toast.tone]}`}
    >
      {toast.message}
    </div>
  );
}
