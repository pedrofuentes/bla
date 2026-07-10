import { describe, expect, it } from "vitest";
import { chordFromKeyboardEvent } from "./hotkeyChord";

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
    expect(chordFromKeyboardEvent(keyEvent("Escape", { ctrlKey: true, shiftKey: true }))).toBeNull();
  });

  it("rejects a main key press with no modifiers held at all", () => {
    // A bare letter with zero modifiers isn't captured as a chord — avoids
    // accidentally binding a hotkey to an unmodified printable key.
    expect(chordFromKeyboardEvent(keyEvent("D"))).toBeNull();
  });
});
