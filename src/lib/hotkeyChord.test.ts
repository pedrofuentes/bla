import { describe, expect, it } from "vitest";
import { chordFromKeyboardEvent, validateCommandHotkeyKeyset } from "./hotkeyChord";

/** Minimal `KeyboardEvent` builder — jsdom's constructor supports these fields directly. */
function keyEvent(key: string, mods: Partial<KeyboardEventInit> = {}): KeyboardEvent {
  return new KeyboardEvent("keydown", { key, bubbles: true, ...mods });
}

describe("chordFromKeyboardEvent", () => {
  it("captures a single modifier + main key as an accelerator string", () => {
    const chord = chordFromKeyboardEvent(keyEvent("D", { ctrlKey: true }));
    expect(chord).toBe("Control+D");
  });

  it("captures multiple modifiers in a stable Control/Alt/Shift/Super order", () => {
    const chord = chordFromKeyboardEvent(
      keyEvent(" ", { ctrlKey: true, shiftKey: true, altKey: true, metaKey: true }),
    );
    expect(chord).toBe("Control+Alt+Shift+Super+Space");
  });

  it("maps the space bar main key to the accelerator token 'Space'", () => {
    const chord = chordFromKeyboardEvent(keyEvent(" ", { ctrlKey: true, shiftKey: true }));
    expect(chord).toBe("Control+Shift+Space");
  });

  it("maps the meta/command key to the cross-platform 'Super' token", () => {
    const chord = chordFromKeyboardEvent(keyEvent("K", { metaKey: true }));
    expect(chord).toBe("Super+K");
  });

  it("uppercases a bare letter main key", () => {
    const chord = chordFromKeyboardEvent(keyEvent("d", { ctrlKey: true }));
    expect(chord).toBe("Control+D");
  });

  it("passes through a function-key main key unchanged", () => {
    const chord = chordFromKeyboardEvent(keyEvent("F4", { altKey: true }));
    expect(chord).toBe("Alt+F4");
  });

  // Bare-modifier rejection: a keydown whose `key` IS one of the modifier
  // keys themselves (the user is still holding it down, chord isn't
  // complete yet) must not produce a chord.
  it("rejects a bare Control key press with no main key", () => {
    expect(chordFromKeyboardEvent(keyEvent("Control", { ctrlKey: true }))).toBeNull();
  });

  it("rejects a bare Shift key press with no main key", () => {
    expect(chordFromKeyboardEvent(keyEvent("Shift", { shiftKey: true }))).toBeNull();
  });

  it("rejects a bare Alt key press with no main key", () => {
    expect(chordFromKeyboardEvent(keyEvent("Alt", { altKey: true }))).toBeNull();
  });

  it("rejects a bare Meta key press with no main key", () => {
    expect(chordFromKeyboardEvent(keyEvent("Meta", { metaKey: true }))).toBeNull();
  });

  // Escape cancels the capture entirely, regardless of modifiers held.
  it("returns null for Escape, cancelling capture", () => {
    expect(chordFromKeyboardEvent(keyEvent("Escape"))).toBeNull();
  });

  it("returns null for Escape even while modifiers are held", () => {
    expect(
      chordFromKeyboardEvent(keyEvent("Escape", { ctrlKey: true, shiftKey: true })),
    ).toBeNull();
  });

  it("rejects a main key press with no modifiers held at all", () => {
    // A bare letter with zero modifiers isn't captured as a chord — avoids
    // accidentally binding a hotkey to an unmodified printable key.
    expect(chordFromKeyboardEvent(keyEvent("D"))).toBeNull();
  });
});

// -----------------------------------------------------------------
// Issue #281 (ac7-p0): the command-mode hotkey's trigger key must be a
// function key (F1-F24) — a leaked keydown (the OS/plugin can't suppress
// one on either macOS or Windows, see hotkeys.rs's validate_command_hotkey_keyset
// doc comment for the full diagnosis) produces no text character for a
// function key, so it can't clobber a selection the way a leaked letter
// key did in #281's Ctrl+Shift+O -> "oooo" repro. This is the CLIENT-SIDE
// half of the fix's defense in depth — an immediate, synchronous check the
// picker runs on a just-captured chord, before ever calling the backend
// `validate_command_hotkey` probe.
// -----------------------------------------------------------------
describe("validateCommandHotkeyKeyset", () => {
  it("accepts function-key chords (F1-F24)", () => {
    for (const chord of [
      "Control+Shift+F13",
      "Alt+F1",
      "Super+F24",
      "Control+Alt+Shift+Super+F7",
    ]) {
      expect(validateCommandHotkeyKeyset(chord)).toEqual({ valid: true });
    }
  });

  it("rejects a character-producing trigger key with a clear function-key explanation", () => {
    for (const chord of [
      "Control+Shift+C", // the pre-#281 shipped default
      "Control+Shift+O", // the #281 repro's exact chord
      "Control+Shift+1",
      "Control+Shift+Space",
      "Control+Enter",
      "Control+Tab",
    ]) {
      const result = validateCommandHotkeyKeyset(chord);
      expect(result.valid).toBe(false);
      if (!result.valid) {
        expect(result.reason.toLowerCase()).toContain("function key");
      }
    }
  });

  it("rejects a non-function-key non-character trigger too — the allowlist is function keys specifically", () => {
    const result = validateCommandHotkeyKeyset("Control+ArrowUp");
    expect(result.valid).toBe(false);
  });
});
