import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { blur, click, flush, focus, mount, typeInto, type Mounted } from "../../testUtils";
import type { HistoryRow, Settings } from "../../lib/ipc";
import { HistoryTab } from "./HistoryTab";

const invoke = vi.fn();

// `vi.mock` factories are hoisted above imports by vitest, so `HistoryTab`
// above resolves against this mocked `../../lib/ipc` — the module under
// test never touches the real Tauri `invoke`. HistoryTab subscribes to no
// backend events, so unlike GeneralTab.test.tsx there's no `onEvent` mock.
vi.mock("../../lib/ipc", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Privacy (MISSION §5/§7): every fixture string here is synthetic — no real
// user transcript text anywhere in this file, and no test ever
// console.logs a row, a query, or a search result.
const BASE_SETTINGS: Settings = {
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

const ROW_A: HistoryRow = {
  id: 1,
  created_at_ms: Date.parse("2026-07-10T09:00:00Z"),
  raw: "synthetic fixture raw text alpha",
  cleaned: "Synthetic fixture cleaned text alpha.",
  app_name: "SyntheticNotes",
};
const ROW_B: HistoryRow = {
  id: 2,
  created_at_ms: Date.parse("2026-07-11T09:00:00Z"),
  raw: "synthetic fixture raw text bravo",
  cleaned: "Synthetic fixture cleaned text bravo.",
  app_name: null,
};

function setupInvoke(overrides: Partial<Record<string, (...args: unknown[]) => unknown>> = {}) {
  invoke.mockImplementation((command: string, args?: unknown) => {
    if (overrides[command]) return Promise.resolve(overrides[command]!(args));
    switch (command) {
      case "get_settings":
        return Promise.resolve(BASE_SETTINGS);
      case "search_history":
        return Promise.resolve([ROW_B, ROW_A]);
      case "copy_history_entry":
      case "delete_history_entry":
      case "clear_history":
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
  setupInvoke();
});

afterEach(() => {
  mounted?.unmount();
  mounted = undefined;
});

describe("HistoryTab (AC-32: search render + re-query)", () => {
  it("searches with an empty query on mount and renders the returned rows", async () => {
    mounted = mount(<HistoryTab />);
    await flush();

    expect(invoke).toHaveBeenCalledWith("search_history", { query: "", limit: expect.any(Number) });
    expect(mounted.container.querySelector('[data-testid="history-row-1"]')).not.toBeNull();
    expect(mounted.container.querySelector('[data-testid="history-row-2"]')).not.toBeNull();
    expect(mounted.container.querySelector('[data-testid="history-preview-1"]')?.textContent).toBe(
      ROW_A.cleaned,
    );
  });

  it("shows a loading state before the first search resolves", async () => {
    let resolveSearch!: (rows: HistoryRow[]) => void;
    setupInvoke({
      search_history: () =>
        new Promise((resolve) => {
          resolveSearch = resolve;
        }),
    });

    mounted = mount(<HistoryTab />);
    // Do not flush before asserting — the search promise is still pending.
    expect(mounted.container.querySelector('[data-testid="history-loading"]')).not.toBeNull();

    resolveSearch([]);
    await flush();
    expect(mounted.container.querySelector('[data-testid="history-loading"]')).toBeNull();
  });

  it("shows an inline error state when search_history rejects", async () => {
    setupInvoke({
      search_history: () => Promise.reject(new Error("boom")),
    });

    mounted = mount(<HistoryTab />);
    await flush();

    const error = mounted.container.querySelector('[data-testid="history-load-error"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).toMatch(/boom/);
  });

  it("shows an empty state when search_history returns no rows", async () => {
    setupInvoke({ search_history: () => [] });

    mounted = mount(<HistoryTab />);
    await flush();

    expect(mounted.container.querySelector('[data-testid="history-empty-state"]')).not.toBeNull();
  });

  it("re-queries search_history as the search text changes", async () => {
    mounted = mount(<HistoryTab />);
    await flush();
    invoke.mockClear();
    setupInvoke({ search_history: () => [ROW_B] });

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="history-search-input"]',
    )!;
    typeInto(input, "bravo");
    await flush();

    expect(invoke).toHaveBeenCalledWith("search_history", {
      query: "bravo",
      limit: expect.any(Number),
    });
    expect(mounted.container.querySelector('[data-testid="history-row-2"]')).not.toBeNull();
    expect(mounted.container.querySelector('[data-testid="history-row-1"]')).toBeNull();
  });
});

describe("HistoryTab (AC-33: copy/delete)", () => {
  it("calls copy_history_entry with the entry's id when Copy is clicked", async () => {
    mounted = mount(<HistoryTab />);
    await flush();

    click(mounted.container.querySelector('[data-testid="history-copy-1"]')!);
    await flush();

    expect(invoke).toHaveBeenCalledWith("copy_history_entry", { id: 1 });
  });

  it("calls delete_history_entry with the entry's id and removes it from the list without a re-fetch", async () => {
    mounted = mount(<HistoryTab />);
    await flush();
    invoke.mockClear();
    setupInvoke();

    click(mounted.container.querySelector('[data-testid="history-delete-1"]')!);
    await flush();

    expect(invoke).toHaveBeenCalledWith("delete_history_entry", { id: 1 });
    expect(invoke).not.toHaveBeenCalledWith("search_history", expect.anything());
    expect(mounted.container.querySelector('[data-testid="history-row-1"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="history-row-2"]')).not.toBeNull();
  });

  it("shows an inline confirm (not a native dialog) before Clear all deletes everything", async () => {
    mounted = mount(<HistoryTab />);
    await flush();

    const confirmSpy = vi.spyOn(window, "confirm");
    click(mounted.container.querySelector('[data-testid="history-clear-all-button"]')!);
    await flush();

    expect(confirmSpy).not.toHaveBeenCalled();
    expect(mounted.container.querySelector('[data-testid="history-clear-confirm"]')).not.toBeNull();
    expect(invoke).not.toHaveBeenCalledWith("clear_history");

    click(mounted.container.querySelector('[data-testid="history-clear-confirm-button"]')!);
    await flush();

    expect(invoke).toHaveBeenCalledWith("clear_history");
    expect(mounted.container.querySelector('[data-testid="history-row-1"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="history-row-2"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="history-empty-state"]')).not.toBeNull();
    confirmSpy.mockRestore();
  });

  it("cancelling the inline confirm leaves history intact", async () => {
    mounted = mount(<HistoryTab />);
    await flush();

    click(mounted.container.querySelector('[data-testid="history-clear-all-button"]')!);
    await flush();
    click(mounted.container.querySelector('[data-testid="history-clear-cancel-button"]')!);
    await flush();

    expect(mounted.container.querySelector('[data-testid="history-clear-confirm"]')).toBeNull();
    expect(invoke).not.toHaveBeenCalledWith("clear_history");
    expect(mounted.container.querySelector('[data-testid="history-row-1"]')).not.toBeNull();
  });
});

describe("HistoryTab (AC-34: retention-days round trip)", () => {
  it("reads the current retention_days into the control on load", async () => {
    setupInvoke({ get_settings: () => ({ ...BASE_SETTINGS, retention_days: 14 }) });

    mounted = mount(<HistoryTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="history-retention-input"]',
    )!;
    expect(input.value).toBe("14");
  });

  it("labels 0 as keep forever", async () => {
    mounted = mount(<HistoryTab />);
    await flush();

    const help = mounted.container.querySelector('[data-testid="history-retention-help"]');
    expect(help?.textContent).toMatch(/keep forever/i);
  });

  it("writes a changed retention value via set_settings on blur, preserving other fields", async () => {
    mounted = mount(<HistoryTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="history-retention-input"]',
    )!;
    focus(input);
    typeInto(input, "30");
    blur(input);
    await flush();

    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: { ...BASE_SETTINGS, retention_days: 30 },
    });
  });

  it("round-trips the retention value across a simulated reload", async () => {
    mounted = mount(<HistoryTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="history-retention-input"]',
    )!;
    focus(input);
    typeInto(input, "7");
    blur(input);
    await flush();

    // Simulate a reload: unmount, point get_settings at the now-persisted
    // value (mirroring what a real set_settings write would leave behind),
    // and remount.
    mounted.unmount();
    setupInvoke({ get_settings: () => ({ ...BASE_SETTINGS, retention_days: 7 }) });
    mounted = mount(<HistoryTab />);
    await flush();

    const reloaded = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="history-retention-input"]',
    )!;
    expect(reloaded.value).toBe("7");
  });
});
