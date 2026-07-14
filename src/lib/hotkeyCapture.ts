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
 *   auto-applied via `set_settings` (issue #183).
 */
export type CaptureEndReason = "escape" | "blur" | "invalid" | "committed";

/**
 * Whether ending capture for `reason` requires an explicit `resume_hotkey`
 * call to restore the previously-registered global hotkey.
 *
 * The only path that does NOT need an explicit resume is a `"committed"`
 * chord that actually CHANGED the hotkey: that save's `set_settings`
 * re-registers the new binding itself (its `hotkey_changed` branch). Every
 * other case — Escape, blur, an invalid chord, and (PR #185 Sentinel 🔴-1)
 * a committed chord that equals the current hotkey (`hotkeyChanged` false,
 * so `set_settings` short-circuits and re-registers nothing) — must resume
 * explicitly or the global dictation hotkey is left permanently suspended.
 *
 * `hotkeyChanged` is only meaningful for `"committed"`; it defaults to
 * `false` so the non-committed reasons keep their unconditional resume.
 */
export function captureEndNeedsResume(reason: CaptureEndReason, hotkeyChanged = false): boolean {
  if (reason === "committed") return !hotkeyChanged;
  return true;
}
