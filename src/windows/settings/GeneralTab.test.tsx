import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { change, click, flush, focus, keydown, mount, type Mounted } from "../../testUtils";
import type { Settings } from "../../lib/ipc";
import { GeneralTab } from "./GeneralTab";

const invoke = vi.fn();
const onEvent = vi.fn();

// `vi.mock` factories are hoisted above imports by vitest, so `GeneralTab`
// above resolves against this mocked `../../lib/ipc` — the module under
// test never touches the real Tauri `invoke`/`listen`.
vi.mock("../../lib/ipc", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  onEvent: (...args: unknown[]) => onEvent(...args),
}));

/**
 * Handlers/unlisten-spies captured by the default `onEvent` mock (PR #134
 * Sentinel finding 9: the previous stub resolved without ever recording the
 * handler, so no test could FIRE a backend event and observe the UI react).
 */
let eventHandlers: Record<string, (payload: unknown) => void> = {};
let unlistenSpies: Record<string, ReturnType<typeof vi.fn>> = {};

/** Fires a captured backend-event handler, wrapped in `act`. No-op if the component never subscribed. */
function fire(event: string, payload: unknown) {
  act(() => {
    eventHandlers[event]?.(payload);
  });
}

const BASE_SETTINGS: Settings = {
  hotkey: "Control+Shift+Space",
  recording_mode: "Hold",
  model_preset: "LargeV3Turbo",
  output_mode: "Cursor",
  file_path_template: "{{date:YYYY-MM-DD}}.md",
};

function setupInvoke(overrides: Partial<Record<string, (...args: unknown[]) => unknown>> = {}) {
  invoke.mockImplementation((command: string, args?: unknown) => {
    if (overrides[command]) return Promise.resolve(overrides[command]!(args));
    switch (command) {
      case "get_settings":
        return Promise.resolve(BASE_SETTINGS);
      case "download_selected_model":
        return Promise.resolve("already-present");
      case "validate_hotkey":
        return Promise.resolve(undefined);
      case "set_settings":
        return Promise.resolve(undefined);
      default:
        return Promise.reject(new Error(`unmocked command ${command}`));
    }
  });
}

let mounted: Mounted | undefined;

beforeEach(() => {
  eventHandlers = {};
  unlistenSpies = {};
  invoke.mockReset();
  onEvent.mockReset();
  onEvent.mockImplementation((event: string, handler: (payload: unknown) => void) => {
    eventHandlers[event] = handler;
    const unlisten = vi.fn();
    unlistenSpies[event] = unlisten;
    return Promise.resolve(unlisten);
  });
  setupInvoke();
});

afterEach(() => {
  mounted?.unmount();
  mounted = undefined;
});

describe("GeneralTab", () => {
  it("loads settings on mount and pre-fills the hotkey field", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    expect(invoke).toHaveBeenCalledWith("get_settings");
    const input = mounted.container.querySelector<HTMLInputElement>('[data-testid="hotkey-input"]');
    expect(input?.value).toBe("Control+Shift+Space");
  });

  it("checks the currently selected model's download status on mount", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    expect(invoke).toHaveBeenCalledWith("download_selected_model");
  });

  it("captures a key chord into the hotkey field and validates it", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    keydown(input, "D", { ctrlKey: true, shiftKey: true });
    await flush();

    expect(invoke).toHaveBeenCalledWith("validate_hotkey", { accelerator: "Control+Shift+D" });
    expect(input.value).toBe("Control+Shift+D");
    expect(mounted.container.querySelector('[data-testid="hotkey-error"]')).toBeNull();
  });

  it("keeps listening (doesn't validate or commit a chord) on a bare modifier keydown", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    keydown(input, "Control", { ctrlKey: true });
    await flush();

    // Still capturing (shows the "press a key" prompt, not a committed
    // value) and no validate_hotkey call yet — a bare modifier isn't a
    // complete chord.
    expect(input.value).toMatch(/press a key/i);
    expect(invoke).not.toHaveBeenCalledWith("validate_hotkey", expect.anything());

    // Escaping out afterward reverts to the original, uncommitted hotkey.
    keydown(input, "Escape");
    await flush();
    expect(input.value).toBe("Control+Shift+Space");
  });

  it("cancels capture on Escape without changing the field", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    keydown(input, "Escape");
    await flush();

    expect(input.value).toBe("Control+Shift+Space");
    expect(invoke).not.toHaveBeenCalledWith("validate_hotkey", expect.anything());
  });

  it("shows an inline error and blocks save when the captured chord is invalid", async () => {
    invoke.mockImplementation((command: string) => {
      if (command === "validate_hotkey") return Promise.reject(new Error("bad accelerator"));
      if (command === "get_settings") return Promise.resolve(BASE_SETTINGS);
      if (command === "download_selected_model") return Promise.resolve("already-present");
      if (command === "set_settings") return Promise.resolve(undefined);
      return Promise.reject(new Error(`unmocked command ${command}`));
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    keydown(input, "Z", { ctrlKey: true });
    await flush();

    expect(mounted.container.querySelector('[data-testid="hotkey-error"]')?.textContent).toMatch(
      /bad accelerator/i,
    );

    const saveButton = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="save-button"]',
    )!;
    invoke.mockClear();
    click(saveButton);
    await flush();

    expect(invoke).not.toHaveBeenCalledWith("set_settings", expect.anything());
  });

  it("saves the validated hotkey, recording mode, and model preset via set_settings", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    keydown(input, "D", { ctrlKey: true, shiftKey: true });
    await flush();

    const toggleRadio = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="mode-toggle"]',
    )!;
    click(toggleRadio);

    const modelSelect = mounted.container.querySelector<HTMLSelectElement>(
      '[data-testid="model-preset-select"]',
    )!;
    change(modelSelect, "Small");

    const saveButton = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="save-button"]',
    )!;
    invoke.mockClear();
    click(saveButton);
    await flush();

    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: {
        ...BASE_SETTINGS,
        hotkey: "Control+Shift+D",
        recording_mode: "Toggle",
        model_preset: "Small",
      },
    });
  });

  // -------------------------------------------------------------------
  // PR #134 Sentinel 🔴-1 (+ finding 9): the model-download-* handlers must
  // actually be exercised — fired, not just subscribed — and a failed event
  // subscription (e.g. a capability/ACL rejection like the one that
  // silently broke this window) must be visible in the UI, with the
  // successful listeners still cleaned up on unmount.
  // -------------------------------------------------------------------

  it("shows live download progress when model-download-progress fires", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    fire("model-download-progress", { bytes_downloaded: 42, total_bytes: 100, percent: 42 });
    await flush();

    expect(mounted.container.querySelector('[data-testid="model-status"]')?.textContent).toContain(
      "Downloading… 42%",
    );
  });

  it("shows Ready when model-download-complete fires after progress", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    fire("model-download-progress", { bytes_downloaded: 42, total_bytes: 100, percent: 42 });
    fire("model-download-complete", null);
    await flush();

    expect(mounted.container.querySelector('[data-testid="model-status"]')?.textContent).toBe(
      "Ready",
    );
  });

  it("shows the failure and the backend's message when model-download-error fires", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    fire("model-download-error", "checksum mismatch for model file");
    await flush();

    expect(mounted.container.querySelector('[data-testid="model-status"]')?.textContent).toBe(
      "Download failed",
    );
    expect(mounted.container.textContent).toContain("checksum mismatch for model file");
  });

  it("surfaces an event-subscription failure in the UI instead of failing silently", async () => {
    // The shape of the real-world failure this guards against: Tauri's
    // capability ACL rejecting `event.listen` for an uncovered window.
    onEvent.mockImplementation(() =>
      Promise.reject(new Error("event.listen not allowed on window")),
    );

    mounted = mount(<GeneralTab />);
    await flush();

    expect(mounted.container.querySelector('[data-testid="events-error"]')?.textContent).toContain(
      "event.listen not allowed on window",
    );
  });

  it("still unsubscribes the successful listeners on unmount when one subscription fails", async () => {
    const succeeded: ReturnType<typeof vi.fn>[] = [];
    onEvent.mockImplementation((event: string, handler: (payload: unknown) => void) => {
      if (event === "model-download-progress") {
        return Promise.reject(new Error("event.listen not allowed on window"));
      }
      eventHandlers[event] = handler;
      const unlisten = vi.fn();
      succeeded.push(unlisten);
      return Promise.resolve(unlisten);
    });

    mounted = mount(<GeneralTab />);
    await flush();

    expect(succeeded.length).toBeGreaterThan(0);
    mounted.unmount();
    mounted = undefined;

    for (const unlisten of succeeded) {
      expect(unlisten).toHaveBeenCalled();
    }
  });

  it("unsubscribes every listener on unmount in the all-successful case", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const spies = Object.values(unlistenSpies);
    expect(spies.length).toBeGreaterThan(0);
    mounted.unmount();
    mounted = undefined;

    for (const unlisten of spies) {
      expect(unlisten).toHaveBeenCalled();
    }
  });

  // -------------------------------------------------------------------
  // PR #134 Sentinel 🔴-2: Save must not clobber a concurrent settings
  // change made elsewhere (tray menu / status window's output-mode toggle)
  // while this window is open — the snapshot Save spreads must track
  // `output-mode-changed`, mirroring App.tsx.
  // -------------------------------------------------------------------

  it("does not clobber a concurrent output-mode change when saving", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    // The user flips output mode via the tray while this window is open.
    fire("output-mode-changed", "File");
    await flush();

    const saveButton = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="save-button"]',
    )!;
    invoke.mockClear();
    click(saveButton);
    await flush();

    // Before the fix, the mount-time snapshot (output_mode: "Cursor") was
    // spread into the payload, silently reverting + re-persisting the
    // concurrent change.
    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: { ...BASE_SETTINGS, output_mode: "File" },
    });
  });
});
