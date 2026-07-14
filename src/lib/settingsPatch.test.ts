import { describe, expect, it } from "vitest";
import type { Settings } from "./ipc";
import { applySettingsPatch, revertPatchedFields } from "./settingsPatch";

const BASE_SETTINGS: Settings = {
  hotkey: "Control+Shift+Space",
  recording_mode: "Hold",
  model_preset: "LargeV3Turbo",
  output_mode: "Cursor",
  file_path_template: "{{date:YYYY-MM-DD}}.md",
  launch_at_login: false,
  sound_cues: true,
};

describe("applySettingsPatch", () => {
  // Issue #183: each control in GeneralTab now auto-applies on change by
  // merging just its own field into the latest known settings snapshot
  // (rather than a batched Save spreading every local control's state at
  // once) — this is the pure merge behind every one of those call sites.

  it("merges a single changed field, leaving every other field untouched", () => {
    const next = applySettingsPatch(BASE_SETTINGS, { recording_mode: "Toggle" });
    expect(next).toEqual({ ...BASE_SETTINGS, recording_mode: "Toggle" });
  });

  it("merges multiple changed fields at once", () => {
    const next = applySettingsPatch(BASE_SETTINGS, {
      model_preset: "Small",
      launch_at_login: true,
    });
    expect(next).toEqual({ ...BASE_SETTINGS, model_preset: "Small", launch_at_login: true });
  });

  it("does not mutate the settings snapshot passed in", () => {
    const snapshotCopy = { ...BASE_SETTINGS };
    applySettingsPatch(BASE_SETTINGS, { sound_cues: false });
    expect(BASE_SETTINGS).toEqual(snapshotCopy);
  });

  it("preserves a field the patch omits even when it differs from the type default", () => {
    const withFileMode: Settings = { ...BASE_SETTINGS, output_mode: "File" };
    const next = applySettingsPatch(withFileMode, { hotkey: "Control+Shift+D" });
    expect(next.output_mode).toBe("File");
  });
});

describe("revertPatchedFields", () => {
  // PR #185 cycle-4 🔴-2: rolling back a failed auto-apply must restore ONLY
  // the field(s) that apply patched (back to their pre-apply base values) —
  // laid onto the CURRENT settings, so a concurrent out-of-band write (e.g. a
  // tray-driven output-mode change that landed while the apply was in flight)
  // survives the rollback instead of being clobbered by a blind revert-to-base.

  it("restores only the patched field to its base value, keeping other current fields", () => {
    const base: Settings = { ...BASE_SETTINGS, sound_cues: true };
    // The apply optimistically set sound_cues=false; meanwhile output_mode
    // changed out of band to File.
    const current: Settings = { ...BASE_SETTINGS, sound_cues: false, output_mode: "File" };
    const reverted = revertPatchedFields(current, base, { sound_cues: false });
    // sound_cues reverts to base (true); the concurrent output_mode survives.
    expect(reverted).toEqual({ ...BASE_SETTINGS, sound_cues: true, output_mode: "File" });
  });

  it("restores every patched key and no others", () => {
    const base: Settings = { ...BASE_SETTINGS, launch_at_login: false, model_preset: "LargeV3Turbo" };
    const current: Settings = {
      ...BASE_SETTINGS,
      launch_at_login: true,
      model_preset: "Small",
      output_mode: "File",
    };
    const reverted = revertPatchedFields(current, base, {
      launch_at_login: true,
      model_preset: "Small",
    });
    expect(reverted.launch_at_login).toBe(false);
    expect(reverted.model_preset).toBe("LargeV3Turbo");
    expect(reverted.output_mode).toBe("File"); // untouched concurrent change
  });

  it("does not mutate the inputs", () => {
    const base: Settings = { ...BASE_SETTINGS };
    const current: Settings = { ...BASE_SETTINGS, sound_cues: false };
    const currentCopy = { ...current };
    revertPatchedFields(current, base, { sound_cues: false });
    expect(current).toEqual(currentCopy);
  });
});
