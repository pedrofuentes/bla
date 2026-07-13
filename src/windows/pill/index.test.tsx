/**
 * Contract test for the pill window's pipeline-error toast wiring (issue
 * #126, M2 PR 2.4; Sentinel 🔴-1 on PR #135). Drives the real emit→listen→
 * render path: mounts `PillWindow`, captures the handler it subscribes to
 * `onEvent("pipeline-error", ...)` with (the same seam `lib.rs` emits over),
 * fires a mocked event through it, and asserts the toast renders with the
 * right tone. Regression-protects the wiring that the `pill-window`
 * capability (`src-tauri/capabilities/pill.json`) makes reachable at runtime
 * — without that capability the listen call is ACL-rejected and no toast
 * ever renders.
 */
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PipelineErrorEvent } from "../../lib/ipc";
import { flush, mount } from "../../testUtils";

// Captures the pill's `pipeline-error` handler so the test can fire an event
// through the exact seam the component subscribed with.
let pipelineErrorHandler: ((payload: PipelineErrorEvent) => void) | null = null;

vi.mock("../../lib/ipc", () => ({
  onEvent: vi.fn(async (event: string, handler: (payload: PipelineErrorEvent) => void) => {
    if (event === "pipeline-error") pipelineErrorHandler = handler;
    return () => {};
  }),
}));

// Imported after the mock is registered so `index.tsx`'s `onEvent` import
// resolves to the mock above.
const { PillWindow } = await import("./index");

afterEach(() => {
  pipelineErrorHandler = null;
  vi.clearAllMocks();
});

function fire(payload: PipelineErrorEvent): void {
  act(() => {
    pipelineErrorHandler?.(payload);
  });
}

describe("PillWindow pipeline-error toast", () => {
  it("subscribes to the pipeline-error event on mount", async () => {
    const { unmount } = mount(<PillWindow />);
    await flush();
    expect(pipelineErrorHandler).toBeTypeOf("function");
    unmount();
  });

  it("renders an informational toast for OllamaUnreachable", async () => {
    const { container, unmount } = mount(<PillWindow />);
    await flush();

    fire({
      kind: "OllamaUnreachable",
      message: "Local AI cleanup is unreachable; used basic cleanup instead.",
    });

    const toast = container.querySelector('[role="status"]');
    expect(toast).not.toBeNull();
    expect(toast?.textContent).toContain("Local AI cleanup is unreachable");
    // Informational tone is styled distinctly (blue), not the blocking red.
    expect(toast?.className).toContain("blue");
    expect(toast?.className).not.toContain("red");
    unmount();
  });

  it("renders a blocking toast for ModelMissing", async () => {
    const { container, unmount } = mount(<PillWindow />);
    await flush();

    fire({ kind: "ModelMissing", message: "The speech-to-text model is missing." });

    const toast = container.querySelector('[role="status"]');
    expect(toast).not.toBeNull();
    expect(toast?.textContent).toContain("model is missing");
    // Blocking tone is styled distinctly (red), not the informational blue.
    expect(toast?.className).toContain("red");
    expect(toast?.className).not.toContain("blue");
    unmount();
  });
});
