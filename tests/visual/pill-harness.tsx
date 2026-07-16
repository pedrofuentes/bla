/**
 * Visual-verification harness for the recording pill (MISSION.md §3),
 * mirroring `tests/visual/settings-harness.tsx`'s pattern for issue #180:
 * a dev-only Vite entry -- NOT part of the production build (not listed in
 * `vite.config.ts`'s `build.rollupOptions.input`, so `vite build` never
 * emits it) -- that mounts the real `PillWindow` against the Tauri IPC
 * layer mocked via `@tauri-apps/api/mocks` (already shipped inside the
 * `@tauri-apps/api` dependency -- no new dependency), so Playwright can
 * screenshot it running in a plain browser against the Vite dev server.
 *
 * Built for issue #182's per-listener degrade fix specifically: unlike the
 * settings harness's `shouldMockEvents: true` (which auto-succeeds every
 * `plugin:event|listen` call with no way to selectively fail one), this
 * harness hand-rolls the event-plugin mock so a `?fail=` query param can
 * reject individual listeners by name -- exactly the scenario `index.tsx`'s
 * per-listener degrade needs to be screenshotted in each of its three
 * states (all listeners healthy / one failed / all failed).
 *
 * `window.__TAURI_INTERNALS__.transformCallback` is already wired by
 * `mockIPC`'s setup to register the real handler `listen()` passes in and
 * return a numeric id -- this harness only needs to remember that id per
 * event name (`registeredCallbackIds`) so `window.__pillHarnessEmit` (called
 * from Playwright via `page.evaluate`) can invoke
 * `window.__TAURI_INTERNALS__.runCallback(id, { payload })` to simulate a
 * real backend emit for whichever listeners DID subscribe successfully.
 */
import React from "react";
import ReactDOM from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { PillWindow } from "../../src/windows/pill/index";
import type { Settings } from "../../src/lib/ipc";
import "../../src/index.css";

declare global {
  interface Window {
    __TAURI_INTERNALS__: {
      runCallback: (id: number, data: unknown) => void;
    };
    /** Simulates a backend `emit(event, payload)` for a listener this harness let subscribe successfully (no-op otherwise). */
    __pillHarnessEmit?: (event: string, payload: unknown) => void;
  }
}

const DEFAULT_SETTINGS: Settings = {
  hotkey: "Control+Shift+Space",
  recording_mode: "Hold",
  model_preset: "LargeV3Turbo",
  output_mode: "Cursor",
  file_path_template: "{{date:YYYY-MM-DD}}.md",
  launch_at_login: false,
  sound_cues: false,
};

const params = new URLSearchParams(window.location.search);
/** Comma-separated event names (`pipeline-state-changed`, `audio-level`, `pipeline-error`) to ACL-reject. */
const failingEvents = new Set(
  (params.get("fail") ?? "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean),
);

const registeredCallbackIds = new Map<string, number>();

mockWindows("pill");
// NOT `shouldMockEvents: true` (unlike the settings harness) -- that option
// auto-succeeds every `plugin:event|listen` call with no way to selectively
// reject just one, which is exactly the scenario under test here.
mockIPC((cmd, rawArgs) => {
  const args = (rawArgs ?? {}) as Record<string, unknown>;
  switch (cmd) {
    case "get_settings":
      return DEFAULT_SETTINGS;
    case "plugin:event|listen": {
      const event = args.event as string;
      if (failingEvents.has(event)) {
        throw new Error(`event.listen not allowed for "${event}"`);
      }
      registeredCallbackIds.set(event, args.handler as number);
      return Math.floor(Math.random() * 1_000_000);
    }
    case "plugin:event|unlisten":
      return undefined;
    default:
      throw new Error(`pill-harness: unmocked command ${cmd}`);
  }
});

window.__pillHarnessEmit = (event, payload) => {
  const id = registeredCallbackIds.get(event);
  if (id === undefined) return; // that listener never subscribed -- nothing to drive.
  window.__TAURI_INTERNALS__.runCallback(id, { event, id: 0, payload });
};

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <PillWindow />
  </React.StrictMode>,
);
