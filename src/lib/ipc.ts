/**
 * Typed wrapper around Tauri's `invoke`/`listen`, mirroring
 * `src-tauri/src/commands.rs` and the events `src-tauri/src/lib.rs` emits.
 *
 * The UI must call the core only through this module (docs/ARCHITECTURE.md
 * §Module Boundaries) — never `@tauri-apps/api` directly from a component —
 * so every IPC call/event subscription has a single, typed, mockable seam
 * for Playwright screenshots of the settings window and status window in a
 * plain browser.
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";

/** Mirrors `settings::RecordingMode` (src-tauri/src/settings.rs). */
export type RecordingMode = "Hold" | "Toggle";

/** Mirrors `settings::ModelPreset` (src-tauri/src/settings.rs). */
export type ModelPreset = "LargeV3Turbo" | "Small";

/** Mirrors `settings::OutputModeSetting` (src-tauri/src/settings.rs). */
export type OutputModeSetting = "Cursor" | "File";

/** Mirrors `settings::Settings` (src-tauri/src/settings.rs). */
export interface Settings {
  hotkey: string;
  recording_mode: RecordingMode;
  model_preset: ModelPreset;
  output_mode: OutputModeSetting;
  file_path_template: string;
}

/** Mirrors `models::DownloadProgress` (src-tauri/src/models.rs). */
export interface DownloadProgress {
  bytes_downloaded: number;
  total_bytes: number;
  /** `0.0..=100.0`. */
  percent: number;
}

/** The two `Ok` shapes `commands::download_selected_model` returns. */
export type DownloadStartResult = "already-present" | "downloading";

/**
 * Command name → { args, result } typing. Extend this map as `commands.rs`
 * grows; each key must match a `#[tauri::command]` name exactly.
 */
export interface Commands {
  get_settings: { result: Settings };
  set_settings: { args: { settings: Settings }; result: void };
  set_output_mode: { args: { mode: OutputModeSetting }; result: void };
  download_selected_model: { result: DownloadStartResult };
}

/**
 * Invoke a Tauri command by name with full type inference from {@link Commands}.
 * Swap the implementation for a mock in tests/Playwright by overriding this
 * module's export.
 */
export async function invoke<K extends keyof Commands>(
  command: K,
  args?: Commands[K] extends { args: infer A } ? A : never,
): Promise<Commands[K] extends { result: infer R } ? R : never> {
  return tauriInvoke(command as string, args as Record<string, unknown>);
}

/**
 * Event name → payload typing, mirroring every `app.emit(...)` call site in
 * `src-tauri/src/lib.rs`/`commands.rs`.
 */
export interface Events {
  /**
   * The Debug-formatted `tray::TrayIconState` derived from the pipeline's
   * current `tray::PipelineState` (`set_pipeline_state` in lib.rs) — one of
   * `"Idle" | "Active" | "Busy" | "Error"`.
   */
  "pipeline-state-changed": string;
  "model-download-progress": DownloadProgress;
  /**
   * The selected model finished downloading (checksum verified + renamed
   * into place). Emitted from both download threads' success arm so the UI
   * leaves the "Downloading…" state. Unit payload (`null`).
   */
  "model-download-complete": null;
  /** A human-readable error message — never transcript/clipboard text. */
  "model-download-error": string;
  /**
   * The live output mode changed (`commands::set_output_mode`), emitted for
   * either trigger — the status window's toggle button or the tray menu's
   * item — so the window's state can't drift from the tray's.
   */
  "output-mode-changed": OutputModeSetting;
  /**
   * The RMS level (`0.0..=1.0`) of the most recently captured audio chunk
   * during an active dictation, throttled to ~30Hz server-side
   * (`audio::LevelThrottle`, `lib.rs`'s level-event poller) so the pill's
   * live meter isn't flooded with one event per audio callback. Only ever
   * a scalar — raw audio samples never leave the core as an event.
   */
  "audio-level": number;
}

/**
 * Subscribe to a Tauri event by name with payload typing from {@link Events}.
 * Returns the `unlisten` function; call it on unmount to avoid leaking the
 * subscription. The single seam through which any component listens for
 * backend-driven state changes, so it stays mockable the same way
 * {@link invoke} is.
 */
export async function onEvent<K extends keyof Events>(
  event: K,
  handler: (payload: Events[K]) => void,
): Promise<UnlistenFn> {
  return tauriListen<Events[K]>(event, (e) => handler(e.payload));
}
