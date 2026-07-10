/**
 * Fixed-size ring buffer for recent audio-level samples (issue #126, M2 PR
 * 2.3) -- the pill's waveform draws from the tail of this buffer instead of
 * accumulating an unbounded array of `audio-level` event payloads for the
 * lifetime of a dictation.
 *
 * Pure and immutable: `pushLevel` never mutates its input, so React state
 * updates (`setLevels((buf) => pushLevel(buf, payload))`) stay referentially
 * predictable, and this stays independently unit-testable from IPC/DOM
 * (`pillState.ts`'s reducer and the canvas render layer both build on top of
 * this, but neither of them needs to know how the buffer is maintained).
 */

/** Default number of recent samples the pill's waveform keeps/renders. */
export const LEVEL_BUFFER_CAPACITY = 32;

/**
 * Appends `level` to `buf`, clamping it to the documented `0.0..=1.0`
 * `audio-level` contract (src/lib/ipc.ts) and dropping the oldest sample(s)
 * once `capacity` is exceeded (FIFO -- newest last). Returns a new array;
 * `buf` is left untouched.
 */
export function pushLevel(
  buf: readonly number[],
  level: number,
  capacity: number = LEVEL_BUFFER_CAPACITY,
): number[] {
  const next = buf.concat(clamp01(level));
  return next.length > capacity ? next.slice(next.length - capacity) : next;
}

/** Clamps to `0.0..=1.0`, treating `NaN` as the `0.0` floor. */
function clamp01(level: number): number {
  if (!Number.isFinite(level)) return 0;
  if (level < 0) return 0;
  if (level > 1) return 1;
  return level;
}
