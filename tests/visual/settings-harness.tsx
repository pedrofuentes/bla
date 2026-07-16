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
 *
 * A separate `?history=` query param (issue #199) selects the starting
 * `search_history` row set for the History tab — see `HISTORY_FIXTURES`.
 * Every string in it is an obviously synthetic placeholder (MISSION
 * §5/§7's privacy note for design-loop screenshots: never real transcript
 * text). `search_history`/`delete_history_entry`/`clear_history` are
 * mocked statefully (mutating a module-local array) so Playwright driving
 * a Delete/Clear-all click sees the list actually change, matching what
 * the real backend commands do.
 */
import React from "react";
import ReactDOM from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { SettingsWindow } from "../../src/windows/settings/index";
import type { HistoryRow, ModelRegistryEntry, Settings } from "../../src/lib/ipc";
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
  retention_days: 0,
};

const FIXTURES: Record<string, Settings> = {
  default: DEFAULT_SETTINGS,
  "file-mode": {
    ...DEFAULT_SETTINGS,
    output_mode: "File",
    file_base_dir: "/Users/cofounder/Documents/Obsidian/Vault",
    file_path_template: "daily/{{date:YYYY-MM-DD}}.md",
  },
  "history-retention": {
    ...DEFAULT_SETTINGS,
    retention_days: 30,
  },
};

const MODEL_REGISTRY: ModelRegistryEntry[] = [
  { preset: "LargeV3Turbo", size_bytes: 574_041_195 },
  { preset: "Small", size_bytes: 487_601_967 },
];

// Issue #199: obviously synthetic placeholder history rows (never real
// transcript text) for the History tab's design-loop screenshots.
const HISTORY_ROWS: HistoryRow[] = [
  {
    id: 3,
    created_at_ms: Date.parse("2026-07-15T09:41:00Z"),
    raw: "um so the placeholder standup notes go here",
    cleaned: "Placeholder standup notes go here.",
    app_name: "Notes",
  },
  {
    id: 2,
    created_at_ms: Date.parse("2026-07-14T18:05:00Z"),
    raw: "reminder placeholder text about a placeholder task",
    cleaned: "Reminder: placeholder text about a placeholder task.",
    app_name: "Reminders",
  },
  {
    id: 1,
    created_at_ms: Date.parse("2026-07-14T08:12:00Z"),
    raw: "placeholder journal entry example text for the design loop",
    cleaned: "Placeholder journal entry example text for the design loop.",
    app_name: null,
  },
];

const HISTORY_FIXTURES: Record<string, HistoryRow[]> = {
  default: HISTORY_ROWS,
  empty: [],
};

const params = new URLSearchParams(window.location.search);
const fixtureName = params.get("fixture") ?? "default";
const settings = FIXTURES[fixtureName] ?? DEFAULT_SETTINGS;
const historyFixtureName = params.get("history") ?? "default";
// Mutable copy — `delete_history_entry`/`clear_history` splice this array,
// mirroring the real commands' effect on the underlying store, so a
// Playwright script can drive a Delete/Clear-all click and screenshot the
// resulting state.
let historyRows: HistoryRow[] = [...(HISTORY_FIXTURES[historyFixtureName] ?? HISTORY_ROWS)];

mockWindows("settings");
// `shouldMockEvents: true` so GeneralTab's `onEvent` subscriptions
// (model-download-progress, output-mode-changed, …) resolve as harmless
// no-op listeners instead of rejecting through the unmocked
// `plugin:event|listen` invoke — otherwise every screenshot would show the
// "Live status updates are unavailable" banner.
mockIPC(
  (cmd, payload) => {
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
      // Issue #199: substring search over the mutable `historyRows`,
      // mirroring `store::Store::search_history`'s own behavior closely
      // enough for a design-loop screenshot (newest first; already sorted
      // that way in `HISTORY_ROWS`).
      case "search_history": {
        const query = String((payload as { query?: unknown }).query ?? "").toLowerCase();
        if (query === "") return historyRows;
        return historyRows.filter(
          (row) =>
            row.raw.toLowerCase().includes(query) || row.cleaned.toLowerCase().includes(query),
        );
      }
      case "copy_history_entry":
        return undefined;
      case "delete_history_entry": {
        const { id } = payload as { id: number };
        historyRows = historyRows.filter((row) => row.id !== id);
        return undefined;
      }
      case "clear_history":
        historyRows = [];
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
