import { describe, expect, it } from "vitest";
import { barsFromLevels, scaleLevelForDisplay } from "./waveform";

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

describe("scaleLevelForDisplay", () => {
  // Target values calibrated to issue #179's Windows terminal diagnostic: a
  // real dictation's RMS levels peaked at 0.0892 while speaking and read
  // 0.0000 in silence -- both correctly speech-correlated, but too small for
  // `level * HEIGHT` (PillWaveform.tsx) to ever clear the 2px bar floor.

  it("maps a measured speech peak (~0.09) to most of the bar height", () => {
    const scaled = scaleLevelForDisplay(0.0892);
    expect(scaled).toBeGreaterThanOrEqual(0.6);
    expect(scaled).toBeLessThanOrEqual(0.95);
  });

  it("maps measured typical speech (~0.02) to a clearly visible height", () => {
    const scaled = scaleLevelForDisplay(0.02);
    expect(scaled).toBeGreaterThanOrEqual(0.25);
    expect(scaled).toBeLessThanOrEqual(0.5);
  });

  it("maps measured typical speech (~0.05) to a clearly visible height", () => {
    const scaled = scaleLevelForDisplay(0.05);
    expect(scaled).toBeGreaterThanOrEqual(0.3);
    expect(scaled).toBeLessThanOrEqual(0.6);
  });

  it("maps measured silence (0.0) to the floor", () => {
    expect(scaleLevelForDisplay(0.0)).toBe(0);
  });

  it("maps the max level (1.0) to 1", () => {
    expect(scaleLevelForDisplay(1.0)).toBe(1);
  });

  it("clamps output above 1.0 to 1.0", () => {
    expect(scaleLevelForDisplay(2)).toBe(1);
  });

  it("treats a negative or non-finite input as the 0.0 floor", () => {
    expect(scaleLevelForDisplay(-0.5)).toBe(0);
    expect(scaleLevelForDisplay(Number.NaN)).toBe(0);
  });

  it("is monotonically non-decreasing across the measured range", () => {
    const samples = [0, 0.005, 0.01, 0.02, 0.05, 0.0892, 0.2, 0.5, 1];
    const scaledSamples = samples.map(scaleLevelForDisplay);
    for (let i = 1; i < scaledSamples.length; i++) {
      expect(scaledSamples[i]).toBeGreaterThanOrEqual(scaledSamples[i - 1]);
    }
  });
});
