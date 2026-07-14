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

  it("auto-applies a validated captured hotkey immediately via set_settings", async () => {
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    invoke.mockClear();
    keydown(input, "D", { ctrlKey: true, shiftKey: true });
    await flush();

    expect(invoke).toHaveBeenCalledWith("validate_hotkey", { accelerator: "Control+Shift+D" });
    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: { ...BASE_SETTINGS, hotkey: "Control+Shift+D" },
    });
    expect(input.value).toBe("Control+Shift+D");
    expect(mounted.container.querySelector('[data-testid="hotkey-error"]')).toBeNull();
  });

  it("blocks auto-apply and shows an inline error when the captured chord is invalid", async () => {
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

  it("rolls back a rejected hotkey and does not re-persist the candidate chord later", async () => {
    // PR #185 delta 🔴 (the worse case): a rejected hotkey save must revert
    // the displayed field to the previously-registered chord and must not
    // let the never-registered candidate ride into a later save.
    let setCall = 0;
    setupInvoke({
      set_settings: () => {
        setCall += 1;
        return setCall === 1
          ? Promise.reject(new Error("register failed"))
          : Promise.resolve(undefined);
      },
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    keydown(input, "D", { ctrlKey: true, shiftKey: true }); // Control+Shift+D (changed)
    await flush();

    // Field reverted to the previously-registered hotkey, and the old
    // shortcut was resumed.
    expect(input.value).toBe("Control+Shift+Space");
    expect(invoke).toHaveBeenCalledWith(
      "resume_hotkey",
      expect.objectContaining({ generation: expect.any(Number) }),
    );

    // A later unrelated save must carry the OLD hotkey, not the rejected
    // candidate chord.
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

  it("does not start hotkey capture while a settings apply is in flight (apply-during-async-window)", async () => {
    // PR #185 cycle-3: the capture-during-save interleave is prevented by
    // construction — beginCapture is gated on the serial queue's "apply in
    // flight" signal, so no suspend_hotkey races a concurrent settings write.
    let resolveSet: (() => void) | undefined;
    setupInvoke({
      set_settings: () => new Promise<void>((resolve) => (resolveSet = () => resolve())),
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const soundCheckbox = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="sound-cues-checkbox"]',
    )!;
    click(soundCheckbox); // apply in flight (set_settings pending)
    await flush();

    invoke.mockClear();
    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input); // must be gated — no capture, no suspend
    await flush();

    expect(invoke).not.toHaveBeenCalledWith("suspend_hotkey", expect.anything());
    expect(input.value).not.toMatch(/press a key/i);

    resolveSet!(); // let the apply finish
    await flush();
  });

  it("keeps the hotkey registered across a commit→refocus interleave (TOCTOU)", async () => {
    // PR #185 cycle-3: committing a changed chord enqueues an apply whose
    // resume re-registers the new binding (single owner). Refocusing the
    // field while that apply is in flight must NOT mint a second suspend that
    // clobbers the generation — the gate blocks it — and the original
    // suspend's generation is what resume eventually restores.
    let resolveSet: (() => void) | undefined;
    setupInvoke({
      set_settings: () => new Promise<void>((resolve) => (resolveSet = () => resolve())),
    });

    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    invoke.mockClear();
    focus(input);
    await flush();
    const suspendCall = invoke.mock.calls.find((c) => c[0] === "suspend_hotkey");
    const gen1 = (suspendCall![1] as { generation: number }).generation;

    keydown(input, "D", { ctrlKey: true, shiftKey: true }); // commit changed → apply in flight
    await flush();

    invoke.mockClear();
    // Try to re-enter capture while the commit's apply is still saving.
    blur(input);
    await flush();
    focus(input);
    await flush();
    expect(invoke).not.toHaveBeenCalledWith("suspend_hotkey", expect.anything());

    resolveSet!(); // apply completes → resume registers the new chord
    await flush();

    // Resume uses the ORIGINAL suspend generation — never clobbered.
    expect(invoke).toHaveBeenCalledWith(
      "resume_hotkey",
      expect.objectContaining({ generation: gen1 }),
    );
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

  it("persists then resumes (single-owner re-register) for a committed chord that CHANGED", async () => {
    // PR #185 cycle-3 single-owner model: set_settings no longer registers
    // the hotkey. A committed changed chord persists via set_settings, then
    // resume — the sole shortcut owner — re-registers the newly-persisted
    // binding, guarded by the original suspend's generation.
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    invoke.mockClear();
    keydown(input, "D", { ctrlKey: true, shiftKey: true });
    await flush();

    expect(invoke).toHaveBeenCalledWith("set_settings", {
      settings: { ...BASE_SETTINGS, hotkey: "Control+Shift+D" },
    });
    expect(invoke).toHaveBeenCalledWith(
      "resume_hotkey",
      expect.objectContaining({ generation: expect.any(Number) }),
    );
    // Order: the resume runs AFTER the save (it registers the persisted chord).
    const setIdx = invoke.mock.calls.findIndex((c) => c[0] === "set_settings");
    const resumeIdx = invoke.mock.calls.findIndex((c) => c[0] === "resume_hotkey");
    expect(resumeIdx).toBeGreaterThan(setIdx);
  });

  it("resumes (and does NOT re-persist) when the committed chord equals the current hotkey", async () => {
    // PR #185 Sentinel 🔴-1(a): re-pressing the CURRENT chord
    // (Control+Shift+Space) means set_settings would see hotkey_changed ==
    // false and re-register nothing, so the field must resume itself — and
    // there is nothing to persist.
    mounted = mount(<GeneralTab />);
    await flush();

    const input = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="hotkey-input"]',
    )!;
    focus(input);
    invoke.mockClear();
    keydown(input, "Space", { ctrlKey: true, shiftKey: true });
    await flush();

    expect(invoke).toHaveBeenCalledWith("validate_hotkey", {
      accelerator: "Control+Shift+Space",
    });
    expect(invoke).toHaveBeenCalledWith(
      "resume_hotkey",
      expect.objectContaining({ generation: expect.any(Number) }),
    );
    expect(invoke).not.toHaveBeenCalledWith("set_settings", expect.anything());
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
