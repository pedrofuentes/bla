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

/**
 * Rolls back a failed auto-apply (PR #185 cycle-4 🔴-2). Returns `current`
 * with only the keys that `patch` touched reset to their `base` (pre-apply)
 * values — every other field of `current` is preserved. This is deliberately
 * NOT a blind `= base`: while an apply was in flight an out-of-band writer
 * (the `output-mode-changed` subscription, driven by the tray/status window)
 * may have mutated other fields, and those concurrent changes must survive
 * the rollback rather than be clobbered back to the apply's stale base.
 * Pure — mutates neither input.
 */
export function revertPatchedFields(
  current: Settings,
  base: Settings,
  patch: Partial<Settings>,
): Settings {
  const restored: Partial<Settings> = {};
  for (const key of Object.keys(patch) as (keyof Settings)[]) {
    (restored as Record<string, unknown>)[key] = base[key];
  }
  return { ...current, ...restored };
}
