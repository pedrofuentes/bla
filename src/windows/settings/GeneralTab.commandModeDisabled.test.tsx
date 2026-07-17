import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flush, mount, type Mounted } from "../../testUtils";
import type { Settings } from "../../lib/ipc";
import { GeneralTab } from "./GeneralTab";

const invoke = vi.fn();
const onEvent = vi.fn();

vi.mock("../../lib/ipc", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  onEvent: (...args: unknown[]) => onEvent(...args),
}));

const SETTINGS: Settings = {
  hotkey: "Control+Shift+Space",
  recording_mode: "Hold",
  model_preset: "LargeV3Turbo",
  output_mode: "Cursor",
  file_path_template: "{{date:YYYY-MM-DD}}.md",
  file_base_dir: "",
  launch_at_login: false,
  sound_cues: true,
  command_hotkey: "Control+Shift+F12",
};

let mounted: Mounted | undefined;

beforeEach(() => {
  invoke.mockReset();
  onEvent.mockReset();
  invoke.mockImplementation((command: string) => {
    switch (command) {
      case "get_settings":
        return Promise.resolve(SETTINGS);
      case "download_selected_model":
        return Promise.resolve("already-present");
      case "model_registry":
        return Promise.resolve([]);
      default:
        return Promise.resolve(undefined);
    }
  });
  onEvent.mockResolvedValue(vi.fn());
});

afterEach(() => {
  mounted?.unmount();
  mounted = undefined;
});

it("hides command-mode settings while leaving dictation settings available", async () => {
  mounted = mount(<GeneralTab />);
  await flush();

  expect(mounted.container.querySelector('[data-testid="command-hotkey-input"]')).toBeNull();
  expect(mounted.container.querySelector('[data-testid="command-hotkey-apply-button"]')).toBeNull();
  expect(mounted.container.querySelector('[data-testid="command-hotkey-pending"]')).toBeNull();
  expect(mounted.container.querySelector('[data-testid="command-hotkey-error"]')).toBeNull();
  expect(mounted.container.textContent).not.toContain("Command-mode hotkey");
  expect(mounted.container.querySelector('[data-testid="hotkey-input"]')).not.toBeNull();
  expect(mounted.container.querySelector('[data-testid="mode-hold"]')).not.toBeNull();
});
