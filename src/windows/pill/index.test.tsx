/**
 * Contract tests for the pill window's event wiring:
 *
 * - Resilient per-listener subscription to `pipeline-state-changed`,
 *   `audio-level`, and `pipeline-error` (issue #126, M2 PR 2.3): a single
 *   `Promise.all` would lose every `unlisten` — and silently stop updating —
 *   the moment any one subscription is ACL-rejected, so each is tracked and
 *   cleaned up independently.
 * - Per-listener degrade, not whole-pill blanking (issue #182): a single
 *   rejected subscription degrades only the feature that listener feeds
 *   (e.g. `audio-level` failing drops the live waveform back to the state
 *   dot) rather than replacing the entire pill with the `events-error`
 *   fallback — that fallback is now reserved for every subscription having
 *   failed, and a listener's later success always clears its own prior
 *   failure rather than leaving a stale blanking flag around.
 * - The `pipeline-error` toast (issue #126, M2 PR 2.4; Sentinel 🔴-1 on PR
 *   #135): drives the real emit→listen→render path, firing a mocked event
 *   through the exact seam the component subscribes with and asserting the
 *   toast renders with the right tone. Regression-protects the wiring that
 *   the `pill-window` capability (`src-tauri/capabilities/pill.json`) makes
 *   reachable at runtime — without that capability the listen call is
 *   ACL-rejected and no toast ever renders.
 * - Sound cue playback (issue #126, M2 PR 2.7): `playCue` (the untested Web
 *   Audio glue, mocked here) is invoked with the right `CueKind` for a real
 *   `pipeline-state-changed` sequence, and — the gated-off case — is never
 *   invoked at all when `get_settings` reports `sound_cues: false`. The cue
 *   *decision* itself (`cueForTransition`/`shouldPlayCue`) is covered in
 *   isolation by `src/lib/soundCue.test.ts`; these tests only guard that the
 *   pill actually wires that decision to `playCue`.
 */
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PipelineErrorEvent, Settings } from "../../lib/ipc";
import { flush, mount, type Mounted } from "../../testUtils";
import { PillWindow } from "./index";

const onEvent = vi.fn();
const invoke = vi.fn();
const playCue = vi.fn();

// `vi.mock` factories are hoisted above imports by vitest, so `PillWindow`
// above resolves against these mocked modules — the component under test
// never touches the real Tauri `invoke`/`listen` or Web Audio.
vi.mock("../../lib/ipc", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
  onEvent: (...args: unknown[]) => onEvent(...args),
}));
vi.mock("../../lib/soundCuePlayer", () => ({
  playCue: (...args: unknown[]) => playCue(...args),
}));

const BASE_SETTINGS: Settings = {
  hotkey: "Control+Shift+Space",
  recording_mode: "Hold",
  model_preset: "LargeV3Turbo",
  output_mode: "Cursor",
  file_path_template: "{{date:YYYY-MM-DD}}.md",
  launch_at_login: false,
  sound_cues: true,
};

/** Resolves `get_settings` with `BASE_SETTINGS` overridden by `overrides`. */
function setupInvoke(overrides: Partial<Settings> = {}): void {
  invoke.mockImplementation((command: string) => {
    if (command === "get_settings") return Promise.resolve({ ...BASE_SETTINGS, ...overrides });
    return Promise.reject(new Error(`unmocked command ${command}`));
  });
}

/**
 * Handlers/unlisten-spies captured by the default `onEvent` mock — so a test
 * can FIRE a backend event and observe the pill react (mirrors PR #134's
 * settings-window harness).
 */
let eventHandlers: Record<string, (payload: unknown) => void> = {};
let unlistenSpies: Record<string, ReturnType<typeof vi.fn>> = {};

/** Fires a captured backend-event handler, wrapped in `act`. */
function fire(event: string, payload: unknown) {
  act(() => {
    eventHandlers[event]?.(payload);
  });
}

let mounted: Mounted | undefined;

beforeEach(() => {
  eventHandlers = {};
  unlistenSpies = {};
  invoke.mockReset();
  playCue.mockReset();
  setupInvoke();
  onEvent.mockReset();
  onEvent.mockImplementation((event: string, handler: (payload: unknown) => void) => {
    eventHandlers[event] = handler;
    const unlisten = vi.fn();
    unlistenSpies[event] = unlisten;
    return Promise.resolve(unlisten);
  });
});

afterEach(() => {
  mounted?.unmount();
  mounted = undefined;
});

describe("PillWindow", () => {
  it("subscribes to pipeline-state-changed, audio-level, and pipeline-error on mount", async () => {
    mounted = mount(<PillWindow />);
    await flush();

    const events = onEvent.mock.calls.map((call) => call[0]);
    expect(events).toContain("pipeline-state-changed");
    expect(events).toContain("audio-level");
    expect(events).toContain("pipeline-error");
  });

  it("renders the live waveform once a Recording (Active) state event fires", async () => {
    mounted = mount(<PillWindow />);
    await flush();

    expect(mounted.container.querySelector('[data-testid="pill-waveform"]')).toBeNull();
    fire("pipeline-state-changed", "Active");
    expect(mounted.container.querySelector('[data-testid="pill-waveform"]')).not.toBeNull();
  });

  it("shows the transcribing label when a Busy state event fires", async () => {
    mounted = mount(<PillWindow />);
    await flush();

    fire("pipeline-state-changed", "Busy");
    expect(mounted.container.textContent).toContain("Transcribing");
  });

  it("does not blank the whole pill when only one subscription is rejected (per-listener degrade)", async () => {
    // The observable shape of a missing capability grant: plugin:event|listen
    // is ACL-rejected, so onEvent rejects. It must not vanish as an unhandled
    // rejection that silently kills the whole UI (issue #182) — only the
    // feature that specific listener feeds should degrade.
    onEvent.mockImplementation((event: string) =>
      event === "audio-level"
        ? Promise.reject(new Error("event.listen not allowed"))
        : Promise.resolve(vi.fn()),
    );

    mounted = mount(<PillWindow />);
    await flush();

    // pipeline-state-changed still succeeded, so the pill keeps rendering
    // live state instead of the "Status unavailable" fallback.
    expect(mounted.container.querySelector('[data-testid="events-error"]')).toBeNull();
    fire("pipeline-state-changed", "Busy");
    expect(mounted.container.textContent).toContain("Transcribing");
  });

  it("keeps the state dot alive (in place of the waveform) when only audio-level fails", async () => {
    onEvent.mockImplementation((event: string) =>
      event === "audio-level"
        ? Promise.reject(new Error("event.listen not allowed"))
        : Promise.resolve(vi.fn()),
    );

    mounted = mount(<PillWindow />);
    await flush();

    fire("pipeline-state-changed", "Active");
    // No live audio-level data can ever arrive, so the waveform (which
    // would just render a permanently flat line) is replaced by the state
    // dot instead of either a dead waveform or the full fallback.
    expect(mounted.container.querySelector('[data-testid="pill-waveform"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="pill-status-dot"]')).not.toBeNull();
    expect(mounted.container.textContent).toContain("Recording");
  });

  it("reserves the Status unavailable fallback for every subscription failing", async () => {
    onEvent.mockImplementation(() => Promise.reject(new Error("event.listen not allowed")));

    mounted = mount(<PillWindow />);
    await flush();

    expect(mounted.container.querySelector('[data-testid="events-error"]')).not.toBeNull();
  });

  it("logs only the rejection reason for a failed subscription, never event payloads", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    onEvent.mockImplementation((event: string) =>
      event === "audio-level"
        ? Promise.reject(new Error("event.listen not allowed"))
        : Promise.resolve(vi.fn()),
    );

    mounted = mount(<PillWindow />);
    await flush();

    expect(consoleError).toHaveBeenCalledTimes(1);
    const loggedArgs = consoleError.mock.calls[0];
    expect(loggedArgs.join(" ")).toContain("audio-level");
    expect(loggedArgs.join(" ")).toContain("event.listen not allowed");
    consoleError.mockRestore();
  });

  it("still cleans up the listeners that succeeded when another subscription fails", async () => {
    const goodUnlisten = vi.fn();
    onEvent.mockImplementation((event: string) =>
      event === "audio-level"
        ? Promise.reject(new Error("event.listen not allowed"))
        : Promise.resolve(goodUnlisten),
    );

    mounted = mount(<PillWindow />);
    await flush();

    mounted.unmount();
    mounted = undefined;
    // A single Promise.all would lose every unlisten on the first rejection;
    // per-subscription tracking keeps the survivors' cleanup. Two
    // subscriptions succeed here (pipeline-state-changed, pipeline-error)
    // while audio-level rejects, so the shared `goodUnlisten` spy is called
    // once per successful subscription's cleanup.
    expect(goodUnlisten).toHaveBeenCalledTimes(2);
  });
});

describe("PillWindow pipeline-error toast", () => {
  it("renders an informational toast for OllamaUnreachable", async () => {
    mounted = mount(<PillWindow />);
    await flush();

    fire("pipeline-error", {
      kind: "OllamaUnreachable",
      message: "Local AI cleanup is unreachable; used basic cleanup instead.",
    } satisfies PipelineErrorEvent);

    const toast = mounted.container.querySelector('[role="status"]');
    expect(toast).not.toBeNull();
    expect(toast?.textContent).toContain("Local AI cleanup is unreachable");
    // Informational tone is styled distinctly (blue), not the blocking red.
    expect(toast?.className).toContain("blue");
    expect(toast?.className).not.toContain("red");
  });

  it("renders a blocking toast for ModelMissing", async () => {
    mounted = mount(<PillWindow />);
    await flush();

    fire("pipeline-error", {
      kind: "ModelMissing",
      message: "The speech-to-text model is missing.",
    } satisfies PipelineErrorEvent);

    const toast = mounted.container.querySelector('[role="status"]');
    expect(toast).not.toBeNull();
    expect(toast?.textContent).toContain("model is missing");
    // Blocking tone is styled distinctly (red), not the informational blue.
    expect(toast?.className).toContain("red");
    expect(toast?.className).not.toContain("blue");
  });
});

describe("PillWindow sound cues", () => {
  it("reads sound_cues via get_settings on mount", async () => {
    mounted = mount(<PillWindow />);
    await flush();

    expect(invoke).toHaveBeenCalledWith("get_settings");
  });

  it("plays the 'start' cue on an Idle -> Active transition when sound_cues is enabled", async () => {
    setupInvoke({ sound_cues: true });
    mounted = mount(<PillWindow />);
    await flush();

    fire("pipeline-state-changed", "Active");
    expect(playCue).toHaveBeenCalledWith("start");
  });

  it("plays the 'done' cue on a Busy -> Idle transition (a completed dictation)", async () => {
    setupInvoke({ sound_cues: true });
    mounted = mount(<PillWindow />);
    await flush();

    fire("pipeline-state-changed", "Busy");
    fire("pipeline-state-changed", "Idle");
    expect(playCue).toHaveBeenCalledWith("done");
  });

  it("plays the 'error' cue on a transition into Error", async () => {
    setupInvoke({ sound_cues: true });
    mounted = mount(<PillWindow />);
    await flush();

    fire("pipeline-state-changed", "Error");
    expect(playCue).toHaveBeenCalledWith("error");
  });

  it("never plays a cue when sound_cues is disabled (the gated-off case)", async () => {
    setupInvoke({ sound_cues: false });
    mounted = mount(<PillWindow />);
    await flush();

    fire("pipeline-state-changed", "Active");
    fire("pipeline-state-changed", "Busy");
    fire("pipeline-state-changed", "Idle");
    fire("pipeline-state-changed", "Error");
    expect(playCue).not.toHaveBeenCalled();
  });

  it("does not play a cue for a cancelled dictation (Active -> Idle)", async () => {
    setupInvoke({ sound_cues: true });
    mounted = mount(<PillWindow />);
    await flush();

    fire("pipeline-state-changed", "Active");
    playCue.mockClear();
    fire("pipeline-state-changed", "Idle");
    expect(playCue).not.toHaveBeenCalled();
  });
});
