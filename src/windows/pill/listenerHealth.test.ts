import { describe, expect, it } from "vitest";
import {
  allListenersFailed,
  withListenerFailed,
  withListenerRecovered,
  type ListenerName,
} from "./listenerHealth";

describe("withListenerFailed", () => {
  it("adds a listener to an empty set", () => {
    const result = withListenerFailed(new Set(), "audio-level");
    expect(result.has("audio-level")).toBe(true);
    expect(result.size).toBe(1);
  });

  it("is idempotent for an already-failed listener (returns the same reference)", () => {
    const failed: ReadonlySet<ListenerName> = new Set(["audio-level"]);
    expect(withListenerFailed(failed, "audio-level")).toBe(failed);
  });

  it("does not mutate the input set", () => {
    const failed: ReadonlySet<ListenerName> = new Set(["audio-level"]);
    withListenerFailed(failed, "pipeline-error");
    expect(failed.size).toBe(1);
  });
});

describe("withListenerRecovered", () => {
  it("removes a failed listener", () => {
    const failed: ReadonlySet<ListenerName> = new Set(["audio-level", "pipeline-error"]);
    const result = withListenerRecovered(failed, "audio-level");
    expect(result.has("audio-level")).toBe(false);
    expect(result.has("pipeline-error")).toBe(true);
  });

  it("is a no-op (same reference) for a listener that never failed", () => {
    const failed: ReadonlySet<ListenerName> = new Set(["pipeline-error"]);
    expect(withListenerRecovered(failed, "audio-level")).toBe(failed);
  });

  it("does not mutate the input set", () => {
    const failed: ReadonlySet<ListenerName> = new Set(["audio-level"]);
    withListenerRecovered(failed, "audio-level");
    expect(failed.has("audio-level")).toBe(true);
  });

  it("clears a listener's own prior failure regardless of the other two listeners' state", () => {
    // Mirrors the actual bug: a listener's own successful (re)subscription
    // must win over its own earlier rejection no matter what order the
    // three subscriptions settled in.
    let failed: ReadonlySet<ListenerName> = new Set();
    failed = withListenerFailed(failed, "audio-level");
    failed = withListenerRecovered(failed, "audio-level");
    expect(allListenersFailed(failed)).toBe(false);
    expect(failed.size).toBe(0);
  });
});

describe("allListenersFailed", () => {
  it("is false for an empty set", () => {
    expect(allListenersFailed(new Set())).toBe(false);
  });

  it("is false when only some listeners have failed", () => {
    expect(allListenersFailed(new Set(["audio-level"]))).toBe(false);
    expect(allListenersFailed(new Set(["audio-level", "pipeline-error"]))).toBe(false);
  });

  it("is true only once every listener has failed", () => {
    expect(
      allListenersFailed(new Set(["pipeline-state-changed", "audio-level", "pipeline-error"])),
    ).toBe(true);
  });
});
