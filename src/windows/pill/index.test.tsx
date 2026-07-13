/**
 * Contract tests for the pill window's event wiring:
 *
 * - Resilient per-listener subscription to `pipeline-state-changed`,
 *   `audio-level`, and `pipeline-error` (issue #126, M2 PR 2.3): a single
 *   `Promise.all` would lose every `unlisten` — and silently stop updating —
 *   the moment any one subscription is ACL-rejected, so each is tracked and
 *   cleaned up independently, and a rejected subscription surfaces a visible
 *   `events-error` fallback instead of vanishing.
 * - The `pipeline-error` toast (issue #126, M2 PR 2.4; Sentinel 🔴-1 on PR
 *   #135): drives the real emit→listen→render path, firing a mocked event
 *   through the exact seam the component subscribes with and asserting the
 *   toast renders with the right tone. Regression-protects the wiring that
 *   the `pill-window` capability (`src-tauri/capabilities/pill.json`) makes
 *   reachable at runtime — without that capability the listen call is
 *   ACL-rejected and no toast ever renders.
 */
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PipelineErrorEvent } from "../../lib/ipc";
import { flush, mount, type Mounted } from "../../testUtils";
import { PillWindow } from "./index";

const onEvent = vi.fn();

// `vi.mock` factories are hoisted above imports by vitest, so `PillWindow`
// above resolves against this mocked `../../lib/ipc` — the component under
// test never touches the real Tauri `listen`.
vi.mock("../../lib/ipc", () => ({
  onEvent: (...args: unknown[]) => onEvent(...args),
}));

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

  it("surfaces a visible fallback when a subscription is rejected (capability/ACL failure)", async () => {
    // The observable shape of a missing capability grant: plugin:event|listen
    // is ACL-rejected, so onEvent rejects. It must not vanish as an unhandled
    // rejection that silently kills the whole UI.
    onEvent.mockImplementation((event: string) =>
      event === "audio-level"
        ? Promise.reject(new Error("event.listen not allowed"))
        : Promise.resolve(vi.fn()),
    );

    mounted = mount(<PillWindow />);
    await flush();

    expect(mounted.container.querySelector('[data-testid="events-error"]')).not.toBeNull();
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
