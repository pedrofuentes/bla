import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { blur, change, click, flush, focus, keydown, mount, type Mounted } from "../../testUtils";
import type { ModelRegistryEntry, Settings } from "../../lib/ipc";
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
  launch_at_login: false,
  sound_cues: true,
};

const MODEL_REGISTRY: ModelRegistryEntry[] = [
  { preset: "LargeV3Turbo", size_bytes: 574_041_195 },
  { preset: "Small", size_bytes: 487_601_967 },
];

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
      case "model_registry":
        return Promise.resolve(MODEL_REGISTRY);
      case "suspend_hotkey":
        return Promise.resolve(undefined);
      case "resume_hotkey":
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

  it("has no Save button — every control auto-applies on change (issue #183)", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    expect(mounted.container.querySelector('[data-testid="save-button"]')).toBeNull();
  });

  // -------------------------------------------------------------------
  // Issue #183: auto-apply on change — each control calls set_settings
  // immediately, with a brief "Saved" confirmation, rather than requiring a
  // separate Save click the cofounder never found in the AC-7 smoke test.
  // -------------------------------------------------------------------

  it("auto-applies a recording-mode change immediately via set_settings", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const toggleRadio = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="mode-toggle"]',
    )!;
    invoke.mockClear();
    click(toggleRadio);
    await flush();

    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: { ...BASE_SETTINGS, recording_mode: "Toggle" },
    });
    expect(mounted.container.querySelector('[data-testid="save-status"]')?.textContent).toMatch(
      /saved/i,
    );
  });

  it("auto-applies a model-preset change immediately and re-checks download status", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const modelSelect = mounted.container.querySelector<HTMLSelectElement>(
      '[data-testid="model-preset-select"]',
    )!;
    invoke.mockClear();
    change(modelSelect, "Small");
    await flush();

    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: { ...BASE_SETTINGS, model_preset: "Small" },
    });
    expect(invoke).toHaveBeenCalledWith("download_selected_model");
  });

  it("auto-applies a toggled launch-at-login immediately via set_settings", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const launchCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="launch-at-login-checkbox"]',
    )!;
    invoke.mockClear();
    click(launchCheckbox);
    await flush();

    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: { ...BASE_SETTINGS, launch_at_login: true },
    });
  });

  it("auto-applies a toggled sound-cues preference immediately via set_settings", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const soundCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="sound-cues-checkbox"]',
    )!;
    invoke.mockClear();
    click(soundCheckbox);
    await flush();

    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: { ...BASE_SETTINGS, sound_cues: false },
    });
  });

  it("serializes auto-applies: one set_settings in flight at a time, each built on the last", async () => {
    // PR #185 cycle-3 holistic model: auto-applies run through a single
    // serial queue — never two set_settings overlapping. Toggling two
    // controls in quick succession runs #1 to completion before #2 starts,
    // and #2 builds on #1's persisted result (no lost update by construction).
    const resolvers: Array<() => void> = [];
    setupInvoke({
      set_settings: () => new Promise<void>((resolve) => resolvers.push(() => resolve())),
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const launchCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="launch-at-login-checkbox"]',
    )!;
    const soundCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="sound-cues-checkbox"]',
    )!;

    invoke.mockClear();
    click(launchCheckbox); // enqueue #1 (launch_at_login: true)
    click(soundCheckbox); // enqueue #2 (sound_cues: false) — must wait for #1
    await flush();

    // Serialized: only ONE set_settings is in flight; #2 hasn't started.
    expect(resolvers).toHaveLength(1);
    resolvers[0](); // resolve #1
    await flush();

    // Now #2 runs, built on #1's result.
    expect(resolvers).toHaveLength(2);
    resolvers[1]();
    await flush();

    const setCalls = invoke.mock.calls.filter((c) => c[0] === "set_settings");
    expect(setCalls).toHaveLength(2);
    expect(setCalls[1][1]).toEqual({
      settings: { ...BASE_SETTINGS, launch_at_login: true, sound_cues: false },
    });
  });

  // -------------------------------------------------------------------
  // Issue #181 / #187 (cofounder DECISION): the hotkey field uses an explicit
  // Apply button, NOT auto-apply. Focusing enters capture and suspends the
  // global dictation shortcut so keystrokes are grabbed; a captured chord is
  // shown PENDING and only validated/registered/persisted when the user
  // clicks Apply, through the same serial queue as the other controls.
  // Capture fully ENDS (resumes the OLD, still-current hotkey) before Apply
  // runs, dissolving the capture-vs-apply concurrency.
  // -------------------------------------------------------------------

  it("shows a captured chord as PENDING and does NOT persist it", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    invoke.mockClear();
    keydown(input, "D", { ctrlKey: true, shiftKey: true });
    await flush();

    // Displayed as pending, but nothing is persisted on capture.
    expect(input.value).toBe("Control+Shift+D");
    expect(invoke).not.toHaveBeenCalledWith("set_settings", expect.anything());
    expect(mounted.container.querySelector('[data-testid="hotkey-pending"]')).not.toBeNull();
  });

  it("resumes the OLD hotkey when a chord is captured (capture ends, nothing registered)", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    invoke.mockClear();
    focus(input); // suspend
    await flush();
    keydown(input, "D", { ctrlKey: true, shiftKey: true }); // capture ends → resume old
    await flush();

    const suspendCall = invoke.mock.calls.find((c) => c[0] === "suspend_hotkey");
    const resumeCall = invoke.mock.calls.find((c) => c[0] === "resume_hotkey");
    expect(suspendCall).toBeDefined();
    expect(resumeCall).toBeDefined();
    // Resume echoes the matching suspend's generation.
    expect((resumeCall![1] as { generation: number }).generation).toBe(
      (suspendCall![1] as { generation: number }).generation,
    );
  });

  it("registers + persists the pending chord only when Apply is clicked", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    keydown(input, "D", { ctrlKey: true, shiftKey: true });
    await flush();

    invoke.mockClear();
    const applyButton = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="hotkey-apply-button"]',
    )!;
    click(applyButton);
    await flush();

    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: { ...BASE_SETTINGS, hotkey: "Control+Shift+D" },
    });
    // Applied → the pending indicator is gone and the field shows the new value.
    expect(mounted.container.querySelector('[data-testid="hotkey-pending"]')).toBeNull();
    expect(input.value).toBe("Control+Shift+D");
  });

  it("disables Apply until a valid pending chord differs from the current hotkey", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const applyButton = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="hotkey-apply-button"]',
    )!;
    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;

    // No pending change yet.
    expect(applyButton.disabled).toBe(true);

    // Capturing the CURRENT chord leaves nothing to apply.
    focus(input);
    keydown(input, "Space", { ctrlKey: true, shiftKey: true }); // == current
    await flush();
    expect(applyButton.disabled).toBe(true);
    expect(invoke).not.toHaveBeenCalledWith("set_settings", expect.anything());

    // A different valid chord enables Apply.
    focus(input);
    keydown(input, "D", { ctrlKey: true, shiftKey: true });
    await flush();
    expect(applyButton.disabled).toBe(false);
  });

  it("shows an inline error and keeps Apply disabled for an invalid captured chord", async () => {
    setupInvoke({
      validate_hotkey: () => Promise.reject(new Error("bad accelerator")),
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    invoke.mockClear();
    keydown(input, "Z", { ctrlKey: true });
    await flush();

    expect(mounted.container.querySelector('[data-testid="hotkey-error"]')?.textContent).toMatch(
      /bad accelerator/i,
    );
    const applyButton = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="hotkey-apply-button"]',
    )!;
    expect(applyButton.disabled).toBe(true);
    expect(invoke).not.toHaveBeenCalledWith("set_settings", expect.anything());
  });

  it("shows a save-error AND rolls the control back to its pre-change value when set_settings rejects", async () => {
    // PR #185 delta 🔴: the optimistic write is applied before the awaited
    // set_settings; on rejection it must be rolled back, not left showing
    // the never-persisted value.
    setupInvoke({
      set_settings: () => Promise.reject(new Error("disk full")),
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const soundCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="sound-cues-checkbox"]',
    )!;
    expect(soundCheckbox.checked).toBe(true);
    click(soundCheckbox);
    await flush();

    expect(mounted.container.querySelector('[data-testid="save-error"]')?.textContent).toMatch(
      /disk full/i,
    );
    // Reverted to the pre-change value rather than stuck on the rejected one.
    expect(soundCheckbox.checked).toBe(true);
  });

  it("does not silently re-persist a rejected field on a later unrelated save", async () => {
    // PR #185 delta 🔴: a rejected optimistic write must not linger in the
    // settings ref and ride into the NEXT unrelated auto-apply.
    let setCall = 0;
    setupInvoke({
      set_settings: () => {
        setCall += 1;
        return setCall === 1 ? Promise.reject(new Error("disk full")) : Promise.resolve(undefined);
      },
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const launchCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="launch-at-login-checkbox"]',
    )!;
    const soundCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="sound-cues-checkbox"]',
    )!;

    click(launchCheckbox); // set_settings #1 rejects → must roll back
    await flush();
    expect(launchCheckbox.checked).toBe(false);

    click(soundCheckbox); // set_settings #2 succeeds
    await flush();

    const setCalls = invoke.mock.calls.filter((c) => c[0] === "set_settings");
    // The later save carries ONLY the sound-cues change — the rejected
    // launch-at-login value is gone, not re-submitted.
    expect(setCalls[setCalls.length - 1][1]).toEqual({
      settings: { ...BASE_SETTINGS, sound_cues: false },
    });
  });

  it("does not persist an unbindable chord and shows an error (Apply → register-before-persist)", async () => {
    // #187 (c/d): Apply's set_settings does register-before-persist; a chord
    // the OS won't bind fails → not persisted, old hotkey stays, error shown,
    // and it never rides into a later unrelated save.
    let setCall = 0;
    setupInvoke({
      set_settings: () => {
        setCall += 1;
        return setCall === 1
          ? Promise.reject(new Error("RegisterHotKey failed"))
          : Promise.resolve(undefined);
      },
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    keydown(input, "D", { ctrlKey: true, shiftKey: true });
    await flush();

    const applyButton = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="hotkey-apply-button"]',
    )!;
    click(applyButton); // set_settings #1 rejects (unbindable)
    await flush();

    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: { ...BASE_SETTINGS, hotkey: "Control+Shift+D" },
    });
    // Old hotkey stays; inline error shown; pending discarded.
    expect(input.value).toBe("Control+Shift+Space");
    expect(mounted.container.querySelector('[data-testid="hotkey-error"]')?.textContent).toMatch(
      /register/i,
    );

    // A later unrelated save carries the OLD hotkey, not the rejected candidate.
    const soundCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="sound-cues-checkbox"]',
    )!;
    click(soundCheckbox);
    await flush();

    const setCalls = invoke.mock.calls.filter((c) => c[0] === "set_settings");
    expect((setCalls[setCalls.length - 1][1] as { settings: Settings }).settings.hotkey).toBe(
      "Control+Shift+Space",
    );
  });

  it("lets other controls auto-apply while a hotkey capture is open (no interference)", async () => {
    // #187 (e): capture is independent of the serial apply queue — toggling a
    // checkbox while the hotkey field is capturing still auto-applies, and the
    // capture stays open.
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input); // capturing (suspended)
    await flush();
    expect(input.value).toMatch(/press a key/i);

    invoke.mockClear();
    const soundCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="sound-cues-checkbox"]',
    )!;
    click(soundCheckbox);
    await flush();

    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: { ...BASE_SETTINGS, sound_cues: false },
    });
    // Capture is unaffected.
    expect(input.value).toMatch(/press a key/i);
  });

  it("times out a hung Apply, reverts, then reconciles from the backend on late success", async () => {
    // PR #185 🟡: a hung Apply times out into the revert path; if the backend
    // later SUCCEEDS, the UI reconciles from get_settings so it can't diverge
    // from persisted truth.
    vi.useFakeTimers();
    try {
      let resolveSet: (() => void) | undefined;
      let getCall = 0;
      setupInvoke({
        set_settings: () => new Promise<void>((resolve) => (resolveSet = () => resolve())),
        get_settings: () => {
          getCall += 1;
          return getCall === 1 ? BASE_SETTINGS : { ...BASE_SETTINGS, hotkey: "Control+Shift+D" };
        },
      });

      mounted = mount(<GeneralTab />);
      await flush();

      const input = mounted.container.querySelector<HTMLInputElement>(
        '[data-testid="hotkey-input"]',
      )!;
      focus(input);
      keydown(input, "D", { ctrlKey: true, shiftKey: true });
      await flush();

      const applyButton = mounted.container.querySelector<HTMLButtonElement>(
        '[data-testid="hotkey-apply-button"]',
      )!;
      click(applyButton); // set_settings hangs
      await flush();

      await vi.advanceTimersByTimeAsync(20_000); // timeout → revert to old
      await flush();
      expect(mounted.container.querySelector('[data-testid="hotkey-error"]')?.textContent).toMatch(
        /tim(e|ed) ?out/i,
      );
      expect(input.value).toBe("Control+Shift+Space");

      // The backend actually completes the save late → reconcile from truth.
      resolveSet!();
      await flush();
      expect(invoke).toHaveBeenCalledWith("get_settings");
      expect(input.value).toBe("Control+Shift+D");
    } finally {
      vi.useRealTimers();
    }
  });

  it("resets a stuck capture when the backend signals the window was hidden mid-capture", async () => {
    // PR #185 delta 🟡-3: closing the settings window mid-capture hides (not
    // destroys) it, so React never unmounts. The backend force-resumes the
    // OS shortcut and emits `hotkey-capture-reset`; the field must leave
    // capture mode so it isn't stuck swallowing keys on reopen.
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    await flush();
    expect(input.value).toMatch(/press a key/i);

    fire("hotkey-capture-reset", null);
    await flush();

    expect(input.value).toBe("Control+Shift+Space");
  });

  it("keeps listening (doesn't validate or auto-apply a chord) on a bare modifier keydown", async () => {
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

  it("cancels capture on Escape without changing the field or calling set_settings", async () => {
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
    expect(invoke).not.toHaveBeenCalledWith("set_settings", expect.anything());
  });

  // -------------------------------------------------------------------
  // Issue #181: the global dictation hotkey must be suspended while the
  // capture field is active, so keypresses reach the field instead of also
  // starting a dictation via the still-live shortcut — and restored on
  // every way capture can end.
  // -------------------------------------------------------------------

  it("suspends the global hotkey the moment the capture field is focused", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    invoke.mockClear();
    focus(input);
    await flush();

    // PR #185 Sentinel 🔴-1(iii): suspend carries a monotonic generation.
    expect(invoke).toHaveBeenCalledWith(
      "suspend_hotkey",
      expect.objectContaining({ generation: expect.any(Number) }),
    );
  });

  it("resumes the global hotkey when capture is cancelled via Escape", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    invoke.mockClear();
    keydown(input, "Escape");
    await flush();

    expect(invoke).toHaveBeenCalledWith(
      "resume_hotkey",
      expect.objectContaining({ generation: expect.any(Number) }),
    );
  });

  it("resumes with the SAME generation the matching suspend minted", async () => {
    // PR #185 Sentinel 🔴-1(iii): the resume must echo the generation of the
    // suspend it pairs with, so the backend can reject a stale resume.
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    invoke.mockClear();
    focus(input);
    await flush();
    keydown(input, "Escape");
    await flush();

    const suspendCall = invoke.mock.calls.find((c) => c[0] === "suspend_hotkey");
    const resumeCall = invoke.mock.calls.find((c) => c[0] === "resume_hotkey");
    expect(suspendCall).toBeDefined();
    expect(resumeCall).toBeDefined();
    expect((resumeCall![1] as { generation: number }).generation).toBe(
      (suspendCall![1] as { generation: number }).generation,
    );
  });

  it("resumes the global hotkey when the field loses focus mid-capture", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    invoke.mockClear();
    blur(input);
    await flush();

    expect(invoke).toHaveBeenCalledWith(
      "resume_hotkey",
      expect.objectContaining({ generation: expect.any(Number) }),
    );
  });

  it("resumes the global hotkey when the captured chord fails validation", async () => {
    setupInvoke({
      validate_hotkey: () => Promise.reject(new Error("bad accelerator")),
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    invoke.mockClear();
    keydown(input, "Z", { ctrlKey: true });
    await flush();

    expect(invoke).toHaveBeenCalledWith(
      "resume_hotkey",
      expect.objectContaining({ generation: expect.any(Number) }),
    );
  });

  it("resumes the global hotkey on unmount while a capture suspend is still outstanding", async () => {
    // PR #185 Sentinel 🔴-1(b): the React-unmount safety net (the window's
    // hide path is covered backend-side in lib.rs). Focusing suspends; if
    // the component unmounts before capture ends, the effect cleanup must
    // restore the shortcut.
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    await flush();

    invoke.mockClear();
    mounted.unmount();
    mounted = undefined;

    expect(invoke).toHaveBeenCalledWith(
      "resume_hotkey",
      expect.objectContaining({ generation: expect.any(Number) }),
    );
  });

  it("surfaces a resume_hotkey failure as a save error instead of swallowing it", async () => {
    // PR #185 Sentinel 🟡-3: a fire-and-forget resume that rejects would
    // otherwise leave the hotkey dead silently.
    setupInvoke({
      resume_hotkey: () => Promise.reject(new Error("hotkey OS reject")),
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    keydown(input, "Escape");
    await flush();

    expect(mounted.container.querySelector('[data-testid="save-error"]')?.textContent).toMatch(
      /hotkey OS reject/i,
    );
  });

  // -------------------------------------------------------------------
  // Issue #184: the model picker surfaces each preset's download size.
  // -------------------------------------------------------------------

  it("fetches the model registry on mount and shows each preset's size in the picker", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    expect(invoke).toHaveBeenCalledWith("model_registry");
    const options = Array.from(
      mounted.container.querySelectorAll<HTMLOptionElement>(
        '[data-testid="model-preset-select"] option',
      ),
    );
    const largeOption = options.find((o) => o.value === "LargeV3Turbo")!;
    const smallOption = options.find((o) => o.value === "Small")!;
    expect(largeOption.textContent).toContain("574 MB");
    expect(smallOption.textContent).toContain("488 MB");
  });

  it("still shows the plain preset label when the model registry hasn't loaded yet", async () => {
    setupInvoke({
      model_registry: () => new Promise(() => {}), // never resolves
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const options = Array.from(
      mounted.container.querySelectorAll<HTMLOptionElement>(
        '[data-testid="model-preset-select"] option',
      ),
    );
    const largeOption = options.find((o) => o.value === "LargeV3Turbo")!;
    expect(largeOption.textContent).toContain("Whisper large-v3-turbo (quantized)");
    expect(largeOption.textContent).not.toContain("MB");
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
  // PR #134 Sentinel 🔴-2: a concurrent settings change made elsewhere
  // (tray menu / status window's output-mode toggle) while this window is
  // open must not be clobbered by a later auto-apply from any control here
  // — mirrors App.tsx's own output-mode-changed subscription.
  // -------------------------------------------------------------------

  it("does not clobber a concurrent output-mode change on the next auto-apply", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    // The user flips output mode via the tray while this window is open.
    fire("output-mode-changed", "File");
    await flush();

    const soundCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="sound-cues-checkbox"]',
    )!;
    invoke.mockClear();
    click(soundCheckbox);
    await flush();

    // Before the fix, a mount-time snapshot (output_mode: "Cursor") would
    // have been spread into the payload, silently reverting + re-persisting
    // the concurrent change.
    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: { ...BASE_SETTINGS, output_mode: "File", sound_cues: false },
    });
  });

  it("does not clobber a concurrent output-mode change when an apply is REVERTED on failure", async () => {
    // PR #185 cycle-4 🔴-2: the output-mode-changed handler mutates
    // settingsRef out of band (outside the serial queue). If an apply
    // captured base=(output Cursor), the tray switches to File mid-flight,
    // then the apply rejects — a blind revert-to-base would drop File, and a
    // later apply would re-persist Cursor. The rollback must restore only the
    // patched field, preserving the concurrent File.
    let rejectSet: (() => void) | undefined;
    setupInvoke({
      set_settings: () =>
        new Promise<void>((_res, rej) => (rejectSet = () => rej(new Error("boom")))),
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const soundCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="sound-cues-checkbox"]',
    )!;
    click(soundCheckbox); // apply captures base (output Cursor), set_settings pending
    await flush();

    fire("output-mode-changed", "File"); // out-of-band write while apply in flight
    await flush();

    rejectSet!(); // apply fails → revert
    await flush();

    // A later unrelated apply must carry output_mode File (not clobbered to Cursor).
    setupInvoke(); // restore the default (resolving) set_settings
    invoke.mockClear();
    const launchCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="launch-at-login-checkbox"]',
    )!;
    click(launchCheckbox);
    await flush();

    const setCalls = invoke.mock.calls.filter((c) => c[0] === "set_settings");
    expect((setCalls[setCalls.length - 1][1] as { settings: Settings }).settings.output_mode).toBe(
      "File",
    );
  });

  // -------------------------------------------------------------------
  // Issue #126 (M2 PR 2.6): "Launch bla at login" and "Play sound cues"
  // checkboxes, wired through get_settings/set_settings like every other
  // control on this tab.
  // -------------------------------------------------------------------

  it("pre-fills the launch-at-login and sound-cues checkboxes from get_settings", async () => {
    setupInvoke({
      get_settings: () => ({ ...BASE_SETTINGS, launch_at_login: true, sound_cues: false }),
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const launchCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="launch-at-login-checkbox"]',
    )!;
    const soundCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="sound-cues-checkbox"]',
    )!;

    expect(launchCheckbox.checked).toBe(true);
    expect(soundCheckbox.checked).toBe(false);
  });
});
