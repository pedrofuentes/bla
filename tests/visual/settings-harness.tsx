/**
 * Visual-verification harness for the settings window (MISSION.md §3): a
 * dev-only Vite entry — NOT part of the production build (not listed in
 * `vite.config.ts`'s `build.rollupOptions.input`, so `vite build` never
 * emits it) — that mounts the real `SettingsWindow` against the Tauri IPC
 * layer mocked via `@tauri-apps/api/mocks` (the official mock, already
 * shipped inside the `@tauri-apps/api` dependency the app already has — no
 * new dependency), so Playwright can screenshot it running in a plain
 * browser against the Vite dev server, per `docs/TESTING-STRATEGY.md`'s
 * `tests/visual/` row.
 *
 * A `?fixture=` query param selects the canned `get_settings` response
 * `tests/visual/capture-screenshots.py` requests before driving further
 * interaction (e.g. typing an invalid path template) for a given
 * screenshot. Add a new key to `FIXTURES` for a new starting state; this
 * harness never talks to a real backend.
 */
import React from "react";
import ReactDOM from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { SettingsWindow } from "../../src/windows/settings/index";
import type { ModelRegistryEntry, Settings } from "../../src/lib/ipc";
import "../../src/index.css";

const DEFAULT_SETTINGS: Settings = {
  hotkey: "Control+Shift+Space",
  recording_mode: "Hold",
  model_preset: "LargeV3Turbo",
  output_mode: "Cursor",
  file_path_template: "{{date:YYYY-MM-DD}}.md",
  file_base_dir: "",
  launch_at_login: false,
  sound_cues: true,
};

const FIXTURES: Record<string, Settings> = {
  default: DEFAULT_SETTINGS,
  "file-mode": {
    ...DEFAULT_SETTINGS,
    output_mode: "File",
    file_base_dir: "/Users/cofounder/Documents/Obsidian/Vault",
    file_path_template: "daily/{{date:YYYY-MM-DD}}.md",
  },
};

const MODEL_REGISTRY: ModelRegistryEntry[] = [
  { preset: "LargeV3Turbo", size_bytes: 574_041_195 },
  { preset: "Small", size_bytes: 487_601_967 },
];

const params = new URLSearchParams(window.location.search);
const fixtureName = params.get("fixture") ?? "default";
const settings = FIXTURES[fixtureName] ?? DEFAULT_SETTINGS;

mockWindows("settings");
// `shouldMockEvents: true` so GeneralTab's `onEvent` subscriptions
// (model-download-progress, output-mode-changed, …) resolve as harmless
// no-op listeners instead of rejecting through the unmocked
// `plugin:event|listen` invoke — otherwise every screenshot would show the
// "Live status updates are unavailable" banner.
mockIPC(
  (cmd) => {
    switch (cmd) {
      case "get_settings":
        return settings;
      case "download_selected_model":
        return "already-present";
      case "model_registry":
        return MODEL_REGISTRY;
      case "validate_hotkey":
      case "set_settings":
      case "suspend_hotkey":
      case "resume_hotkey":
        return undefined;
      default:
        throw new Error(`settings-harness: unmocked command ${cmd}`);
    }
  },
  { shouldMockEvents: true },
);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <SettingsWindow />
  </React.StrictMode>,
);
