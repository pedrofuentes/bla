import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { click, flush, mount, type Mounted } from "../../testUtils";
import { SettingsWindow } from "./index";
import type { Settings } from "../../lib/ipc";

const invoke = vi.fn();
const onEvent = vi.fn();

// `vi.mock` factories are hoisted above imports by vitest, so `SettingsWindow`
// above (which renders `GeneralTab`, a consumer of `../../lib/ipc`) resolves
// against this mock.
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
  launch_at_login: false,
  sound_cues: true,
  retention_days: 0,
};

let mounted: Mounted | undefined;

beforeEach(() => {
  invoke.mockReset();
  onEvent.mockReset();
  onEvent.mockResolvedValue(() => {});
  invoke.mockImplementation((command: string) => {
    switch (command) {
      case "get_settings":
        return Promise.resolve(BASE_SETTINGS);
      case "download_selected_model":
        return Promise.resolve("already-present");
      case "validate_hotkey":
        return Promise.resolve(undefined);
      case "set_settings":
        return Promise.resolve(undefined);
      // Issue #199: the History tab (mounted once its tab is clicked)
      // searches on mount — mocked here so switching to it in these
      // tab-bar tests doesn't reject through an unmocked command.
      case "search_history":
        return Promise.resolve([]);
      case "copy_history_entry":
      case "delete_history_entry":
      case "clear_history":
        return Promise.resolve(undefined);
      // Issue #201: the Dictionary tab (mounted once its tab is clicked)
      // lists terms on mount — mocked here so switching to it in these
      // tab-bar tests doesn't reject through an unmocked command.
      case "list_dictionary_terms":
        return Promise.resolve([]);
      // Issue #203: the Tone tab (mounted once its tab is clicked) lists
      // rules on mount — mocked here so switching to it in these tab-bar
      // tests doesn't reject through an unmocked command.
      case "list_tone_rules":
        return Promise.resolve([]);
      default:
        return Promise.reject(new Error(`unmocked command ${command}`));
    }
  });
});

afterEach(() => {
  mounted?.unmount();
  mounted = undefined;
});

describe("SettingsWindow tab bar", () => {
  it("renders every tab and starts on General", async () => {
    mounted = mount(<SettingsWindow />);
    await flush();

    for (const id of ["general", "history", "dictionary", "tone", "snippets"]) {
      expect(mounted.container.querySelector(`[data-testid="tab-${id}"]`)).not.toBeNull();
    }
    expect(mounted.container.querySelector('[data-testid="general-panel"]')).not.toBeNull();
  });

  it("switches to a placeholder panel when a not-yet-built tab is clicked", async () => {
    mounted = mount(<SettingsWindow />);
    await flush();

    click(mounted.container.querySelector('[data-testid="tab-snippets"]')!);
    await flush();

    expect(mounted.container.querySelector('[data-testid="general-panel"]')).toBeNull();
    const placeholder = mounted.container.querySelector('[data-testid="placeholder-panel"]');
    expect(placeholder).not.toBeNull();
    expect(placeholder?.textContent).toMatch(/snippets/i);
  });

  it("switches to the real Dictionary panel when the Dictionary tab is clicked (issue #201)", async () => {
    mounted = mount(<SettingsWindow />);
    await flush();

    click(mounted.container.querySelector('[data-testid="tab-dictionary"]')!);
    await flush();

    expect(mounted.container.querySelector('[data-testid="general-panel"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="placeholder-panel"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="dictionary-panel"]')).not.toBeNull();
  });

  it("switches to the real History panel when the History tab is clicked (issue #199)", async () => {
    mounted = mount(<SettingsWindow />);
    await flush();

    click(mounted.container.querySelector('[data-testid="tab-history"]')!);
    await flush();

    expect(mounted.container.querySelector('[data-testid="general-panel"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="placeholder-panel"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="history-panel"]')).not.toBeNull();
  });

  it("switches to the real Tone panel when the Tone tab is clicked (issue #203)", async () => {
    mounted = mount(<SettingsWindow />);
    await flush();

    click(mounted.container.querySelector('[data-testid="tab-tone"]')!);
    await flush();

    expect(mounted.container.querySelector('[data-testid="general-panel"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="placeholder-panel"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="tone-panel"]')).not.toBeNull();
  });

  it("switches back to the General panel", async () => {
    mounted = mount(<SettingsWindow />);
    await flush();

    click(mounted.container.querySelector('[data-testid="tab-snippets"]')!);
    await flush();
    click(mounted.container.querySelector('[data-testid="tab-general"]')!);
    await flush();

    expect(mounted.container.querySelector('[data-testid="general-panel"]')).not.toBeNull();
    expect(mounted.container.querySelector('[data-testid="placeholder-panel"]')).toBeNull();
  });

  it("marks the active tab distinctly via aria-selected", async () => {
    mounted = mount(<SettingsWindow />);
    await flush();

    const generalTab = mounted.container.querySelector('[data-testid="tab-general"]')!;
    const historyTab = mounted.container.querySelector('[data-testid="tab-history"]')!;
    expect(generalTab.getAttribute("aria-selected")).toBe("true");
    expect(historyTab.getAttribute("aria-selected")).toBe("false");

    click(historyTab);
    await flush();

    expect(generalTab.getAttribute("aria-selected")).toBe("false");
    expect(historyTab.getAttribute("aria-selected")).toBe("true");
  });
});
