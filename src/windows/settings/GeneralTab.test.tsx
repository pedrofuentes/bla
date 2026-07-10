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
  invoke.mockReset();
  onEvent.mockReset();
  onEvent.mockResolvedValue(() => {});
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

    const input = mounted.container.querySelector<HTMLInputElement>('[data-testid="hotkey-input"]')!;
    focus(input);
    keydown(input, "D", { ctrlKey: true, shiftKey: true });
    await flush();

    expect(invoke).toHaveBeenCalledWith("validate_hotkey", { accelerator: "Control+Shift+D" });
    expect(input.value).toBe("Control+Shift+D");
    expect(mounted.container.querySelector('[data-testid="hotkey-error"]')).toBeNull();
  });

  it("does not update the field on a bare modifier keydown while capturing", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>('[data-testid="hotkey-input"]')!;
    focus(input);
    keydown(input, "Control", { ctrlKey: true });
    await flush();

    expect(input.value).toBe("Control+Shift+Space");
    expect(invoke).not.toHaveBeenCalledWith("validate_hotkey", expect.anything());
  });

  it("cancels capture on Escape without changing the field", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>('[data-testid="hotkey-input"]')!;
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

    const input = mounted.container.querySelector<HTMLInputElement>('[data-testid="hotkey-input"]')!;
    focus(input);
    keydown(input, "Z", { ctrlKey: true });
    await flush();

    expect(mounted.container.querySelector('[data-testid="hotkey-error"]')?.textContent).toMatch(
      /bad accelerator/i,
    );

    const saveButton = mounted.container.querySelector<HTMLButtonElement>('[data-testid="save-button"]')!;
    invoke.mockClear();
    click(saveButton);
    await flush();

    expect(invoke).not.toHaveBeenCalledWith("set_settings", expect.anything());
  });

  it("saves the validated hotkey, recording mode, and model preset via set_settings", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>('[data-testid="hotkey-input"]')!;
    focus(input);
    keydown(input, "D", { ctrlKey: true, shiftKey: true });
    await flush();

    const toggleRadio = mounted.container.querySelector<HTMLInputElement>('[data-testid="mode-toggle"]')!;
    click(toggleRadio);

    const modelSelect = mounted.container.querySelector<HTMLSelectElement>(
      '[data-testid="model-preset-select"]',
    )!;
    change(modelSelect, "Small");

    const saveButton = mounted.container.querySelector<HTMLButtonElement>('[data-testid="save-button"]')!;
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
});
