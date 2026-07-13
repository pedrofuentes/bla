/**
 * Pure layout helper: maps a variable-length level history into exactly
 * `barCount` bar heights for the pill's canvas waveform (issue #126, M2 PR
 * 2.3). Kept separate from the ring buffer (`levelBuffer.ts`) and the canvas
 * draw call (`PillWaveform.tsx`, an untested thin render layer) so the
 * layout decision -- which samples become which bar, and what a too-short
 * history renders as -- is independently unit-tested.
 */

/**
 * Returns the `barCount` most recent entries from `levels` (newest last, so
 * the waveform reads left-to-right as older-to-newer), left-padded with `0`
 * when there isn't yet `barCount` worth of history. `levels` is assumed
 * already clamped to `0.0..=1.0` (see `pushLevel`); returns an empty array
 * for a non-positive `barCount`.
 */
export function barsFromLevels(levels: readonly number[], barCount: number): number[] {
  if (barCount <= 0) return [];

  const recent = levels.slice(Math.max(0, levels.length - barCount));
  const missing = barCount - recent.length;
  return missing > 0 ? new Array(missing).fill(0).concat(recent) : recent;
}
