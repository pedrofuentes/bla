/**
 * Synthesized playback for pipeline-transition sound cues (issue #126, M2
 * PR 2.7). Thin and intentionally untested -- mirrors the OS-integration
 * glue exemption (AGENTS.md: "keep all logic out of it so the logic stays
 * testable"), the same pattern `src/windows/pill/PillWaveform.tsx` follows
 * for canvas drawing. Every decision about *which* cue plays, and whether
 * it plays at all, lives in the pure, unit-tested `src/lib/soundCue.ts`;
 * this module only synthesizes the tone once told to.
 *
 * Purely synthesized at runtime via the Web Audio API (`OscillatorNode` +
 * a short `GainNode` envelope) -- no bundled audio files or recordings
 * (MISSION.md: never ship recordings). Guards for environments without
 * `AudioContext` (e.g. this project's jsdom test environment, or a
 * locked-down webview) by no-op'ing rather than throwing.
 */
import type { CueKind } from "./soundCue";

interface Tone {
  /** Oscillator frequency in Hz. */
  frequency: number;
  /** Total tone duration in seconds. */
  duration: number;
  /** Oscillator waveform -- distinguishes cues by timbre, not just pitch. */
  type: OscillatorType;
}

/** Distinct pitch/duration/timbre per cue so they're recognizable by ear alone. */
const TONES: Record<CueKind, Tone> = {
  start: { frequency: 880, duration: 0.08, type: "sine" },
  done: { frequency: 1046.5, duration: 0.14, type: "sine" },
  error: { frequency: 220, duration: 0.22, type: "square" },
};

/**
 * Lazily-created, reused `AudioContext`. `undefined` means "not yet
 * resolved", `null` means "resolved to unavailable" -- distinct from a real
 * context so unavailability is only ever detected once per session rather
 * than re-probed on every cue.
 */
let sharedContext: AudioContext | null | undefined;

function getAudioContext(): AudioContext | null {
  if (sharedContext !== undefined) return sharedContext;
  const Ctor = typeof window === "undefined" ? undefined : window.AudioContext;
  sharedContext = Ctor ? new Ctor() : null;
  return sharedContext;
}

/**
 * Synthesizes and plays `kind`'s tone: an oscillator through a gain node
 * with a short attack/release envelope (avoids the audible "click" of a
 * hard on/off edge), started now and stopped after its duration. No-ops
 * silently if `AudioContext` isn't available.
 */
export function playCue(kind: CueKind): void {
  const ctx = getAudioContext();
  if (!ctx) return;

  const { frequency, duration, type } = TONES[kind];
  const oscillator = ctx.createOscillator();
  const gain = ctx.createGain();
  oscillator.type = type;
  oscillator.frequency.value = frequency;

  const now = ctx.currentTime;
  const attack = 0.01;
  const release = 0.03;
  gain.gain.setValueAtTime(0, now);
  gain.gain.linearRampToValueAtTime(0.2, now + attack);
  gain.gain.setValueAtTime(0.2, now + duration - release);
  gain.gain.linearRampToValueAtTime(0, now + duration);

  oscillator.connect(gain);
  gain.connect(ctx.destination);
  oscillator.start(now);
  oscillator.stop(now + duration);
}
