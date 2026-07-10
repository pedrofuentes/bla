import { describe, expect, it } from "vitest";
import { LEVEL_BUFFER_CAPACITY, pushLevel } from "./levelBuffer";

describe("pushLevel", () => {
  it("appends onto an empty buffer", () => {
    expect(pushLevel([], 0.5)).toEqual([0.5]);
  });

  it("appends onto a non-empty buffer, preserving order", () => {
    expect(pushLevel([0.1, 0.2], 0.3)).toEqual([0.1, 0.2, 0.3]);
  });

  it("does not mutate the input buffer", () => {
    const buf = [0.1, 0.2];
    pushLevel(buf, 0.3);
    expect(buf).toEqual([0.1, 0.2]);
  });

  it("drops the oldest sample once capacity is exceeded (FIFO)", () => {
    const buf = [0.1, 0.2, 0.3];
    expect(pushLevel(buf, 0.4, 3)).toEqual([0.2, 0.3, 0.4]);
  });

  it("respects a custom capacity smaller than the default", () => {
    expect(pushLevel([0.1], 0.2, 1)).toEqual([0.2]);
  });

  it("defaults to LEVEL_BUFFER_CAPACITY when no capacity is given", () => {
    const full = new Array(LEVEL_BUFFER_CAPACITY).fill(0);
    expect(pushLevel(full, 0.9)).toHaveLength(LEVEL_BUFFER_CAPACITY);
  });

  it("clamps a level above 1.0 to 1.0 (matches the audio-level 0.0..=1.0 contract)", () => {
    expect(pushLevel([], 1.4)).toEqual([1]);
  });

  it("clamps a level below 0.0 to 0.0", () => {
    expect(pushLevel([], -0.3)).toEqual([0]);
  });

  it("treats NaN as 0.0 rather than propagating it", () => {
    expect(pushLevel([], Number.NaN)).toEqual([0]);
  });
});
