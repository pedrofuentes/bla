import { describe, expect, it } from "vitest";
import { captureEndNeedsResume } from "./hotkeyCapture";

describe("captureEndNeedsResume", () => {
  // Issue #181: hotkey capture suspends the global dictation shortcut
  // (`suspend_hotkey`) on focus so keypresses reach the capture field
  // instead of triggering a dictation. Every way capture can end EXCEPT a
  // successfully-committed chord needs an explicit `resume_hotkey` call to
  // restore it — a committed chord already re-registers as part of the
  // auto-apply `set_settings` call that persists it (issue #183), so an
  // extra resume there would be redundant (and racy against that call).
  it("needs an explicit resume after Escape", () => {
    expect(captureEndNeedsResume("escape")).toBe(true);
  });

  it("needs an explicit resume after losing focus mid-capture", () => {
    expect(captureEndNeedsResume("blur")).toBe(true);
  });

  it("needs an explicit resume when the captured chord fails validation", () => {
    expect(captureEndNeedsResume("invalid")).toBe(true);
  });

  it("does not need an explicit resume when a valid chord was committed and auto-applied", () => {
    expect(captureEndNeedsResume("committed")).toBe(false);
  });
});
