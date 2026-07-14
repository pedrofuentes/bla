import { describe, expect, it } from "vitest";
import { captureEndNeedsResume } from "./hotkeyCapture";

describe("captureEndNeedsResume", () => {
  // Issue #181: hotkey capture suspends the global dictation shortcut
  // (`suspend_hotkey`) on focus so keypresses reach the capture field
  // instead of triggering a dictation. Every way capture can end needs the
  // shortcut restored — the only path that does NOT need an explicit
  // `resume_hotkey` is a committed chord that actually CHANGED the hotkey,
  // because that chord's own auto-apply `set_settings` re-registers it (its
  // `hotkey_changed` branch). Everything else — Escape, blur, an invalid
  // chord, and (PR #185 Sentinel 🔴-1) a committed chord that equals the
  // current hotkey (so `set_settings` short-circuits and re-registers
  // nothing) — must resume explicitly or the global hotkey is left dead.
  it("needs an explicit resume after Escape", () => {
    expect(captureEndNeedsResume("escape")).toBe(true);
  });

  it("needs an explicit resume after losing focus mid-capture", () => {
    expect(captureEndNeedsResume("blur")).toBe(true);
  });

  it("needs an explicit resume when the captured chord fails validation", () => {
    expect(captureEndNeedsResume("invalid")).toBe(true);
  });

  it("does not need an explicit resume when a committed chord CHANGED the hotkey", () => {
    // set_settings re-registers the new (changed) hotkey as part of the save.
    expect(captureEndNeedsResume("committed", true)).toBe(false);
  });

  it("needs an explicit resume when a committed chord left the hotkey UNCHANGED", () => {
    // PR #185 Sentinel 🔴-1(a): re-pressing the current chord means
    // `set_settings` sees hotkey_changed == false and re-registers nothing,
    // so the suspend from capture must be undone explicitly.
    expect(captureEndNeedsResume("committed", false)).toBe(true);
  });
});
