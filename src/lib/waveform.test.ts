import { describe, expect, it } from "vitest";
import { barsFromLevels } from "./waveform";

describe("barsFromLevels", () => {
  it("left-pads with 0 when there is less history than barCount", () => {
    expect(barsFromLevels([0.5], 4)).toEqual([0, 0, 0, 0.5]);
  });

  it("returns all zeros for an empty level history", () => {
    expect(barsFromLevels([], 3)).toEqual([0, 0, 0]);
  });

  it("returns every level unchanged when the count matches exactly", () => {
    expect(barsFromLevels([0.1, 0.2, 0.3], 3)).toEqual([0.1, 0.2, 0.3]);
  });

  it("keeps only the most recent barCount entries, newest last", () => {
    expect(barsFromLevels([0.1, 0.2, 0.3, 0.4, 0.5], 3)).toEqual([0.3, 0.4, 0.5]);
  });

  it("returns an empty array for a zero barCount", () => {
    expect(barsFromLevels([0.1, 0.2], 0)).toEqual([]);
  });

  it("returns an empty array for a negative barCount", () => {
    expect(barsFromLevels([0.1, 0.2], -1)).toEqual([]);
  });

  it("does not mutate the input levels array", () => {
    const levels = [0.1, 0.2, 0.3, 0.4];
    barsFromLevels(levels, 2);
    expect(levels).toEqual([0.1, 0.2, 0.3, 0.4]);
  });
});
