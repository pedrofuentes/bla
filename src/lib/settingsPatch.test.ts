import { describe, expect, it } from "vitest";
import type { Settings } from "./ipc";
import { applySettingsPatch } from "./settingsPatch";

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
