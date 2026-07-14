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

/**
 * Perceptual gain applied to `sqrt(rms)` in {@link scaleLevelForDisplay}.
 * Tuned against issue #179's Windows terminal diagnostic (real speech RMS
 * peaking ~0.0892, silence ~0.0000) so that range fills most of the pill's
 * bar height instead of the raw ~0.01-0.09 RMS rounding down to the 2px
 * `MIN_BAR_HEIGHT` floor every time (`PillWaveform.tsx`'s `level * HEIGHT`).
 */
const DISPLAY_GAIN = 2.5;

/**
 * Maps a raw `audio-level` RMS sample (`0.0..=1.0`, see `pushLevel` /
 * `src/lib/ipc.ts`) to a display-space value in the same range, for
 * `PillWaveform.tsx`'s `level * HEIGHT` bar-height calculation.
 *
 * Speech-level RMS is small (issue #179's Windows diagnostic measured real
 * speech peaking ~0.0892, typical speech ~0.02-0.05) -- far too small for a
 * linear `level * HEIGHT` to clear the pill's `MIN_BAR_HEIGHT` floor, so
 * every bar rendered flat despite the levels genuinely tracking the voice.
 * A `sqrt` curve (rather than a flat linear gain) fixes this while keeping
 * silence pinned near the floor: it boosts quiet levels much more than loud
 * ones (`sqrt` is steepest near 0), so the measured speech range spreads out
 * across most of the bar height without needing a gain so large that any
 * stray noise floor above 0 would visibly jump. `DISPLAY_GAIN` is tuned so
 * the measured peak (~0.09) lands around 0.75 and typical speech (~0.02-0.05)
 * lands around 0.35-0.56, leaving headroom before the `1.0` clamp for
 * louder speech this diagnostic didn't sample.
 *
 * This is the ring buffer's *display* mapping only -- `levelBuffer.ts` keeps
 * storing raw RMS, so anything else reading `levels` (e.g. future
 * peak-detection logic) still sees the unscaled values.
 */
export function scaleLevelForDisplay(rms: number): number {
  if (!Number.isFinite(rms) || rms <= 0) return 0;
  return Math.min(1, Math.sqrt(rms) * DISPLAY_GAIN);
}
