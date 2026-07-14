/**
 * Pure decision logic for the settings window's hotkey-capture suspend/
 * resume flow (issue #181).
 *
 * While the hotkey-capture field is focused, `GeneralTab` calls the
 * `suspend_hotkey` command so the still-live global dictation shortcut
 * doesn't fire (starting a dictation) while the user is trying to type a
 * new one into the field. Every way capture can end needs the shortcut
 * restored — but *how* differs by reason, so this stays a pure decision
 * the component's effect-laden handlers can defer to instead of
 * re-deriving inline at each call site.
 */

/**
 * Why hotkey capture ended:
 * - `"escape"` — the user pressed Escape to cancel.
 * - `"blur"` — the field lost focus mid-capture (no chord committed).
 * - `"invalid"` — a chord was captured but failed `validate_hotkey`.
 * - `"committed"` — a chord was captured, validated, and is being
 *   auto-applied via `set_settings` (issue #183) — which itself
 *   re-registers the (new) hotkey as part of that save.
 */
export type CaptureEndReason = "escape" | "blur" | "invalid" | "committed";

/**
 * Whether ending capture for `reason` requires an explicit `resume_hotkey`
 * call to restore the previously-registered global hotkey. Only the
 * `"committed"` path already re-registers on its own (via the auto-apply
 * `set_settings` call) — every other reason leaves the shortcut suspended
 * unless this resumes it explicitly.
 */
export function captureEndNeedsResume(reason: CaptureEndReason): boolean {
  return reason !== "committed";
}
