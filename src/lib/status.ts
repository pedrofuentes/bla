/**
 * Pure display-logic helpers for the minimal status window (issue #110).
 *
 * Deliberately free of any Tauri/DOM/React dependency (no `invoke`, no
 * `window`) so every decision here — hotkey formatting, status/mode/model
 * copy — is unit-testable without a live Tauri app context. `App.tsx` stays
 * a thin consumer of these, wiring their outputs to `src/lib/ipc.ts` calls
 * and JSX.
 */
import type { ModelPreset, OutputModeSetting, RecordingMode } from "./ipc";

/**
 * The four states the tray/status window distinguish, mirroring the
 * Debug-formatted `tray::TrayIconState` carried by the `pipeline-state-changed`
 * event (`src-tauri/src/tray.rs`'s `tray_icon_state`). `"Unknown"` is a
 * client-side-only fallback for before the first event/settings load lands.
 */
export type PipelineState = "Idle" | "Active" | "Busy" | "Error" | "Unknown";

/** The in-contract `pipeline-state-changed` payloads (excludes the client-only `"Unknown"`). */
const PIPELINE_STATES: readonly PipelineState[] = ["Idle", "Active", "Busy", "Error"];

/**
 * Narrows a raw `pipeline-state-changed` payload (the Debug-formatted
 * `tray::TrayIconState`) to a {@link PipelineState}, degrading anything
 * out-of-contract to `"Unknown"` (which renders as idle) rather than letting
 * an unchecked cast flow a bad string into a consumer's `switch` and read
 * `undefined.mode`. Pure — the single guarded seam every event handler
 * should route the raw payload through instead of `payload as PipelineState`.
 */
export function parsePipelineState(raw: string): PipelineState {
  return (PIPELINE_STATES as readonly string[]).includes(raw) ? (raw as PipelineState) : "Unknown";
}

/** How the selected Whisper model's on-disk state is currently understood. */
export type ModelStatus = "checking" | "ready" | "downloading" | "error";

/**
 * Modifier tokens the accelerator grammar
 * (`tauri_plugin_global_shortcut::Shortcut::from_str`) accepts, mapped to a
 * short, platform-neutral display label. Anything not in this table (the
 * main key, e.g. `"Space"`, `"D"`) falls back to title-casing.
 */
const MODIFIER_LABELS: Record<string, string> = {
  CONTROL: "Ctrl",
  CTRL: "Ctrl",
  OPTION: "Alt",
  ALT: "Alt",
  COMMAND: "Cmd",
  CMD: "Cmd",
  SUPER: "Cmd",
  SHIFT: "Shift",
  COMMANDORCONTROL: "Ctrl",
  COMMANDORCTRL: "Ctrl",
  CMDORCTRL: "Ctrl",
  CMDORCONTROL: "Ctrl",
};

/** Title-cases a single token, e.g. `"SPACE"` / `"space"` -> `"Space"`. */
function titleCase(token: string): string {
  if (token.length === 0) return token;
  return token[0].toUpperCase() + token.slice(1).toLowerCase();
}

/**
 * Formats a raw hotkey chord (e.g. `"Control+Shift+Space"`, the settings
 * value round-tripped through `get_settings`) into a short, readable label
 * (e.g. `"Ctrl + Shift + Space"`). Pure string formatting — never validates
 * the chord (that's `hotkeys::validate_hotkey` on the Rust side); an empty
 * or malformed chord just formats whatever tokens it has.
 */
export function formatHotkey(chord: string): string {
  return chord
    .split("+")
    .map((token) => token.trim())
    .filter((token) => token.length > 0)
    .map((token) => MODIFIER_LABELS[token.toUpperCase()] ?? titleCase(token))
    .join(" + ");
}

/**
 * The instruction line under the status header (e.g. "Hold Ctrl + Shift +
 * Space to dictate"), phrased for the configured `RecordingMode` — hold
 * mode reads as a press-and-hold action, toggle mode as two separate
 * presses.
 */
export function hotkeyInstruction(mode: RecordingMode, chord: string): string {
  const formatted = formatHotkey(chord);
  return mode === "Hold"
    ? `Hold ${formatted} to dictate`
    : `Press ${formatted} to start or stop dictating`;
}

/** Short status-line label for the current pipeline state. */
export function statusLabel(state: PipelineState): string {
  switch (state) {
    case "Idle":
      return "Idle";
    case "Active":
      return "Recording…";
    case "Busy":
      return "Transcribing…";
    case "Error":
      return "Something went wrong";
    case "Unknown":
      return "Connecting…";
  }
}

/** Human label for a persisted output-mode value. */
export function modeLabel(mode: OutputModeSetting): string {
  return mode === "Cursor" ? "Cursor" : "File";
}

/** The mode a toggle control would switch *to* from `mode`. */
export function otherMode(mode: OutputModeSetting): OutputModeSetting {
  return mode === "Cursor" ? "File" : "Cursor";
}

/** Human label for a persisted Whisper model preset. */
export function modelPresetLabel(preset: ModelPreset): string {
  return preset === "LargeV3Turbo" ? "Whisper large-v3-turbo (quantized)" : "Whisper small";
}

/**
 * Status-line label for the selected model's on-disk state, e.g. "Ready",
 * "Downloading… 42%", "Download failed". `percent` (from the most recent
 * `DownloadProgress` event, if any) is only shown while `status ===
 * "downloading"` and only once a total size is known (`percent` rounds to a
 * whole number; `undefined` omits it rather than showing a misleading 0%).
 */
export function modelStatusLabel(status: ModelStatus, percent?: number): string {
  switch (status) {
    case "checking":
      return "Checking…";
    case "ready":
      return "Ready";
    case "downloading":
      return percent === undefined ? "Downloading…" : `Downloading… ${Math.round(percent)}%`;
    case "error":
      return "Download failed";
  }
}
