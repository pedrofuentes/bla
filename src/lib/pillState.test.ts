import { describe, expect, it } from "vitest";
import {
  DONE_AUTO_HIDE_MS,
  initialPillState,
  pillLabel,
  pillReducer,
  type PillState,
} from "./pillState";

describe("initialPillState", () => {
  it("starts idle with no done timestamp", () => {
    expect(initialPillState).toEqual({ mode: "idle", doneAt: null });
  });
});

describe("pillReducer / pipeline-state", () => {
  it("maps Active to recording", () => {
    const next = pillReducer(initialPillState, { type: "pipeline-state", state: "Active", now: 0 });
    expect(next).toEqual({ mode: "recording", doneAt: null });
  });

  it("maps Busy to transcribing", () => {
    const next = pillReducer(initialPillState, { type: "pipeline-state", state: "Busy", now: 0 });
    expect(next).toEqual({ mode: "transcribing", doneAt: null });
  });

  it("maps Error to error", () => {
    const next = pillReducer(initialPillState, { type: "pipeline-state", state: "Error", now: 0 });
    expect(next).toEqual({ mode: "error", doneAt: null });
  });

  it("maps Unknown to idle", () => {
    const next = pillReducer(initialPillState, {
      type: "pipeline-state",
      state: "Unknown",
      now: 0,
    });
    expect(next).toEqual({ mode: "idle", doneAt: null });
  });

  it("a Busy -> Idle transition (completed dictation) enters done, stamped with now", () => {
    const transcribing = pillReducer(initialPillState, {
      type: "pipeline-state",
      state: "Busy",
      now: 100,
    });
    const done = pillReducer(transcribing, { type: "pipeline-state", state: "Idle", now: 200 });
    expect(done).toEqual({ mode: "done", doneAt: 200 });
  });

  it("a Recording -> Idle transition (cancelled dictation) goes straight to idle, not done", () => {
    const recording = pillReducer(initialPillState, {
      type: "pipeline-state",
      state: "Active",
      now: 100,
    });
    const idle = pillReducer(recording, { type: "pipeline-state", state: "Idle", now: 200 });
    expect(idle).toEqual({ mode: "idle", doneAt: null });
  });

  it("Idle from an already-idle state stays idle", () => {
    const next = pillReducer(initialPillState, { type: "pipeline-state", state: "Idle", now: 50 });
    expect(next).toEqual({ mode: "idle", doneAt: null });
  });

  it("a fresh Error clears a prior done timestamp", () => {
    const done: PillState = { mode: "done", doneAt: 100 };
    const next = pillReducer(done, { type: "pipeline-state", state: "Error", now: 150 });
    expect(next).toEqual({ mode: "error", doneAt: null });
  });
});

describe("pillReducer / tick", () => {
  it("stays in done before DONE_AUTO_HIDE_MS has elapsed", () => {
    const done: PillState = { mode: "done", doneAt: 1000 };
    const next = pillReducer(done, { type: "tick", now: 1000 + DONE_AUTO_HIDE_MS - 1 });
    expect(next).toEqual(done);
  });

  it("reverts done to idle once DONE_AUTO_HIDE_MS has elapsed", () => {
    const done: PillState = { mode: "done", doneAt: 1000 };
    const next = pillReducer(done, { type: "tick", now: 1000 + DONE_AUTO_HIDE_MS });
    expect(next).toEqual({ mode: "idle", doneAt: null });
  });

  it("is a no-op for non-done modes", () => {
    const recording: PillState = { mode: "recording", doneAt: null };
    const next = pillReducer(recording, { type: "tick", now: 999_999 });
    expect(next).toEqual(recording);
  });
});

describe("pillLabel", () => {
  it("maps each mode to its exact label", () => {
    expect(pillLabel("idle")).toBe("");
    expect(pillLabel("recording")).toBe("Recording…");
    expect(pillLabel("transcribing")).toBe("Transcribing…");
    expect(pillLabel("done")).toBe("Done");
    expect(pillLabel("error")).toBe("Something went wrong");
  });
});
