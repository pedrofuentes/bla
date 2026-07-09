import { describe, expect, it } from "vitest";
import {
  formatHotkey,
  hotkeyInstruction,
  modeLabel,
  modelPresetLabel,
  modelStatusLabel,
  otherMode,
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
  it("labels every PipelineState distinctly", () => {
    const labels = ["Idle", "Active", "Busy", "Error", "Unknown"] as const;
    const seen = new Set(labels.map(statusLabel));
    expect(seen.size).toBe(labels.length);
  });

  it("labels Idle as Idle and Error as an error message", () => {
    expect(statusLabel("Idle")).toBe("Idle");
    expect(statusLabel("Error")).toBe("Something went wrong");
  });
});

describe("modeLabel / otherMode", () => {
  it("labels Cursor and File distinctly", () => {
    expect(modeLabel("Cursor")).not.toBe(modeLabel("File"));
  });

  it("otherMode is its own inverse", () => {
    expect(otherMode(otherMode("Cursor"))).toBe("Cursor");
    expect(otherMode(otherMode("File"))).toBe("File");
  });

  it("otherMode never returns the mode it was given", () => {
    expect(otherMode("Cursor")).not.toBe("Cursor");
    expect(otherMode("File")).not.toBe("File");
  });
});

describe("modelPresetLabel", () => {
  it("labels every ModelPreset distinctly", () => {
    expect(modelPresetLabel("LargeV3Turbo")).not.toBe(modelPresetLabel("Small"));
  });
});

describe("modelStatusLabel", () => {
  it("shows a rounded percent while downloading with a known total", () => {
    expect(modelStatusLabel("downloading", 42.7)).toBe("Downloading… 43%");
  });

  it("omits the percent while downloading before a total is known", () => {
    expect(modelStatusLabel("downloading", undefined)).toBe("Downloading…");
  });

  it("labels ready/checking/error distinctly from downloading and each other", () => {
    const labels = [
      modelStatusLabel("checking"),
      modelStatusLabel("ready"),
      modelStatusLabel("downloading"),
      modelStatusLabel("error"),
    ];
    expect(new Set(labels).size).toBe(labels.length);
  });
});
