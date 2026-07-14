import { describe, expect, it } from "vitest";
import { cueForTransition, shouldPlayCue } from "./soundCue";

describe("cueForTransition", () => {
  it("plays 'start' for Idle -> Active (a dictation began)", () => {
    expect(cueForTransition("Idle", "Active")).toBe("start");
  });

  it("plays 'done' for Busy -> Idle (a completed dictation)", () => {
    expect(cueForTransition("Busy", "Idle")).toBe("done");
  });

  it("plays no cue for Active -> Idle (a cancelled dictation, mirrors pillReducer skipping 'done')", () => {
    expect(cueForTransition("Active", "Idle")).toBeNull();
  });

  it("plays no cue for Active -> Busy (the transcribing tick)", () => {
    expect(cueForTransition("Active", "Busy")).toBeNull();
  });

  it("plays 'error' for Idle -> Error", () => {
    expect(cueForTransition("Idle", "Error")).toBe("error");
  });

  it("plays 'error' for Active -> Error", () => {
    expect(cueForTransition("Active", "Error")).toBe("error");
  });

  it("plays 'error' for Busy -> Error", () => {
    expect(cueForTransition("Busy", "Error")).toBe("error");
  });

  it("plays no cue for Error -> Idle (recovery -- the error already had its own cue)", () => {
    expect(cueForTransition("Error", "Idle")).toBeNull();
  });

  it("plays no cue when the state is unchanged", () => {
    expect(cueForTransition("Idle", "Idle")).toBeNull();
    expect(cueForTransition("Active", "Active")).toBeNull();
    expect(cueForTransition("Busy", "Busy")).toBeNull();
    expect(cueForTransition("Error", "Error")).toBeNull();
  });

  it("plays no cue for most transitions touching the client-only Unknown state", () => {
    expect(cueForTransition("Unknown", "Idle")).toBeNull();
    expect(cueForTransition("Idle", "Unknown")).toBeNull();
  });

  it("plays 'start' for Unknown -> Active (the pill mounts only once dictation is already active, so this -- not Idle -> Active -- is the real first event most dictations deliver)", () => {
    expect(cueForTransition("Unknown", "Active")).toBe("start");
  });
});

describe("shouldPlayCue", () => {
  it("plays a computed cue when sound cues are enabled", () => {
    expect(shouldPlayCue("start", true)).toBe(true);
  });

  it("does not play when sound cues are disabled (the gated-off case)", () => {
    expect(shouldPlayCue("start", false)).toBe(false);
    expect(shouldPlayCue("done", false)).toBe(false);
    expect(shouldPlayCue("error", false)).toBe(false);
  });

  it("does not play when there is no cue, regardless of the preference", () => {
    expect(shouldPlayCue(null, true)).toBe(false);
    expect(shouldPlayCue(null, false)).toBe(false);
  });
});
