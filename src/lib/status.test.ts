import { describe, expect, it } from "vitest";
import {
  formatHotkey,
  hotkeyInstruction,
  modeLabel,
  modelPresetLabel,
  modelStatusLabel,
  otherMode,
  parsePipelineState,
  statusLabel,
} from "./status";

describe("formatHotkey", () => {
  it("expands modifier aliases to short, platform-neutral labels", () => {
    expect(formatHotkey("Control+Shift+Space")).toBe("Ctrl + Shift + Space");
  });

  it("maps the macOS-only Option alias to Alt, same as Windows' Alt spelling", () => {
    expect(formatHotkey("Control+Option+Space")).toBe(formatHotkey("Control+Alt+Space"));
    expect(formatHotkey("Control+Option+Space")).toBe("Ctrl + Alt + Space");
  });

  it("title-cases a main key that isn't a known modifier", () => {
    expect(formatHotkey("Cmd+Shift+D")).toBe("Cmd + Shift + D");
  });

  it("formats a single-key chord with no modifiers", () => {
    expect(formatHotkey("F4")).toBe("F4");
  });

  it("drops empty tokens from stray '+' characters rather than throwing", () => {
    expect(formatHotkey("Control++Space")).toBe("Ctrl + Space");
  });

  it("returns an empty string for an empty chord", () => {
    expect(formatHotkey("")).toBe("");
  });
});

describe("hotkeyInstruction", () => {
  it("phrases hold mode as a press-and-hold action", () => {
    expect(hotkeyInstruction("Hold", "Control+Shift+Space")).toBe(
      "Hold Ctrl + Shift + Space to dictate",
    );
  });

  it("phrases toggle mode as two separate presses", () => {
    expect(hotkeyInstruction("Toggle", "Control+Shift+Space")).toBe(
      "Press Ctrl + Shift + Space to start or stop dictating",
    );
  });
});

describe("statusLabel", () => {
  // Exact-value asserts per state — pins the mapping so swapping any two
  // labels (e.g. Active <-> Busy) fails, not just that they're all distinct.
  it("maps each PipelineState to its exact label", () => {
    expect(statusLabel("Idle")).toBe("Idle");
    expect(statusLabel("Active")).toBe("Recording…");
    expect(statusLabel("Busy")).toBe("Transcribing…");
    expect(statusLabel("Error")).toBe("Something went wrong");
    expect(statusLabel("Unknown")).toBe("Connecting…");
  });
});

describe("parsePipelineState", () => {
  it("passes each in-contract state string through unchanged", () => {
    expect(parsePipelineState("Idle")).toBe("Idle");
    expect(parsePipelineState("Active")).toBe("Active");
    expect(parsePipelineState("Busy")).toBe("Busy");
    expect(parsePipelineState("Error")).toBe("Error");
    expect(parsePipelineState("Unknown")).toBe("Unknown");
  });

  it("maps an out-of-contract string to the safe Unknown fallback", () => {
    // A garbage/renamed event payload must not flow into the pill reducer
    // as an unhandled value (which would read `undefined.mode` and crash
    // the render tree) -- it degrades to Unknown, which renders as idle.
    expect(parsePipelineState("Recording")).toBe("Unknown");
    expect(parsePipelineState("")).toBe("Unknown");
    expect(parsePipelineState("idle")).toBe("Unknown");
  });
});

describe("modeLabel / otherMode", () => {
  it("maps each output mode to its exact label", () => {
    expect(modeLabel("Cursor")).toBe("Cursor");
    expect(modeLabel("File")).toBe("File");
  });

  it("otherMode returns the opposite mode", () => {
    expect(otherMode("Cursor")).toBe("File");
    expect(otherMode("File")).toBe("Cursor");
  });
});

describe("modelPresetLabel", () => {
  it("maps each ModelPreset to its exact label", () => {
    expect(modelPresetLabel("LargeV3Turbo")).toBe("Whisper large-v3-turbo (quantized)");
    expect(modelPresetLabel("Small")).toBe("Whisper small");
  });
});

describe("modelStatusLabel", () => {
  it("shows a rounded percent while downloading with a known total", () => {
    expect(modelStatusLabel("downloading", 42.7)).toBe("Downloading… 43%");
  });

  it("omits the percent while downloading before a total is known", () => {
    expect(modelStatusLabel("downloading", undefined)).toBe("Downloading…");
  });

  // Exact-value asserts per status — a swap of any two labels must fail.
  it("maps each non-progress status to its exact label", () => {
    expect(modelStatusLabel("checking")).toBe("Checking…");
    expect(modelStatusLabel("ready")).toBe("Ready");
    expect(modelStatusLabel("error")).toBe("Download failed");
  });
});
