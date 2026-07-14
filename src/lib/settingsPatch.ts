/**
 * Pure merge behind GeneralTab's per-control auto-apply (issue #183): each
 * control (recording mode, model preset, launch-at-login, sound cues,
 * hotkey) now calls `set_settings` immediately on change with just its own
 * field changed, rather than a batched Save spreading every local control's
 * state into one payload. This is the single merge every one of those call
 * sites goes through, so "what does an auto-applied save actually send" has
 * one definition instead of one inline spread per control.
 */
import type { Settings } from "./ipc";

/** Merges `patch` into `settings`, returning a new object (never mutates `settings`). */
export function applySettingsPatch(settings: Settings, patch: Partial<Settings>): Settings {
  return { ...settings, ...patch };
}
