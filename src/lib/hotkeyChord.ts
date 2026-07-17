/**
 * Pure keydown-to-accelerator mapping for the settings window's hotkey
 * capture field (issue #126, M2 PR 2.5).
 *
 * Deliberately free of any React/Tauri dependency — takes a plain
 * `KeyboardEvent` (the field's `onKeyDown` handler passes `e.nativeEvent`)
 * and returns an accelerator string in the same grammar
 * `hotkeys::validate_hotkey`/`tauri_plugin_global_shortcut::Shortcut::from_str`
 * parse (e.g. `"Control+Shift+Space"`), or `null` when the keydown doesn't
 * complete a chord. The backend is still the source of truth for whether a
 * chord is registrable — this only produces the candidate string; the
 * settings window calls the new `validate_hotkey` command against whatever
 * this returns before treating it as valid.
 *
 * `null` covers two distinct situations the caller must tell apart itself
 * (this helper doesn't distinguish them — see the doc on each case):
 * - Escape: the user wants to cancel capture outright.
 * - A bare modifier keydown (`e.key` is itself `"Control"`/`"Shift"`/etc.)
 *   or a main key with no modifier held: the chord isn't complete yet: more
 *   keys may follow, so the caller should keep listening.
 * A caller that needs to tell these apart (e.g. to know when to stop
 * listening) checks `e.key === "Escape"` itself before/alongside calling
 * this — seen in `GeneralTab.tsx`.
 */

const MODIFIER_KEYS = new Set(["Control", "Alt", "Shift", "Meta", "OS"]);

/** Maps a non-modifier `KeyboardEvent.key` to its accelerator token. */
function mainKeyToken(key: string): string | null {
  if (key.length === 0) return null;
  if (key === " ") return "Space";
  // Single printable character (letter/digit/symbol) — uppercase it, as the
  // accelerator grammar expects (e.g. "d" -> "D").
  if (key.length === 1) return key.toUpperCase();
  // Multi-character keys (`"F4"`, `"ArrowUp"`, `"Enter"`, `"Tab"`, …) are
  // already in the grammar's expected casing.
  return key;
}

/**
 * Derives an accelerator string (e.g. `"Control+Shift+Space"`) from a single
 * keydown event, or `null` if this keydown doesn't complete a capturable
 * chord (see module docs for the two `null` cases).
 *
 * Requires at least one modifier held — a bare, unmodified main key (e.g.
 * plain `"D"`) is rejected rather than captured, so a user who starts typing
 * in the field without holding a modifier doesn't accidentally bind a
 * single-key global hotkey.
 */
export function chordFromKeyboardEvent(e: KeyboardEvent): string | null {
  if (e.key === "Escape") return null;
  if (MODIFIER_KEYS.has(e.key)) return null;

  const modifiers: string[] = [];
  if (e.ctrlKey) modifiers.push("Control");
  if (e.altKey) modifiers.push("Alt");
  if (e.shiftKey) modifiers.push("Shift");
  if (e.metaKey) modifiers.push("Super");

  if (modifiers.length === 0) return null;

  const mainKey = mainKeyToken(e.key);
  if (mainKey === null) return null;

  return [...modifiers, mainKey].join("+");
}

/**
 * Issue #281 (ac7-p0): the allowed trigger-key range for the COMMAND-MODE
 * hotkey specifically — F1 through F24 — mirroring the Rust-side allowlist
 * in `src-tauri/src/hotkeys.rs::is_function_key` exactly (same 24 tokens,
 * same F1-F24 rationale documented there: a function key never produces a
 * text character, so a keydown the OS/plugin leaks to the focused app while
 * the chord is held can't clobber a selection the way #281's
 * `Ctrl+Shift+O` -> `"oooo"` repro did; F1-F12 are included alongside the
 * "preferred" F13-F24 safe range because most laptop keyboards have no
 * physical F13+ key, and F1-F12 are equally safe from the character-leak
 * perspective).
 */
const FUNCTION_KEY_TOKENS = new Set(Array.from({ length: 24 }, (_, i) => `F${i + 1}`));

export type CommandHotkeyKeysetValidation = { valid: true } | { valid: false; reason: string };

/**
 * Client-side half of #281's defense in depth for the command-mode hotkey
 * field: rejects a captured chord whose trigger (last, non-modifier) token
 * isn't a function key, with the same "why" explanation shown regardless of
 * whether this client-side check or the backend `validate_command_hotkey`
 * command (`commands::validate_command_hotkey` ->
 * `hotkeys::validate_command_hotkey_keyset`) is what actually catches it —
 * this is a fast, synchronous, no-IPC-round-trip check the picker runs
 * immediately on a just-captured chord; the backend command is still the
 * authoritative check `set_settings` relies on before persisting (a bad
 * value must never persist even if this client-side check were somehow
 * bypassed).
 *
 * Operates on the accelerator string [`chordFromKeyboardEvent`] produces
 * (modifiers joined with `+`, trigger key last) — NOT on an arbitrary
 * string, so it doesn't need to independently re-parse the full accelerator
 * grammar the way the Rust validator does (that parse already happened when
 * the chord was captured).
 */
export function validateCommandHotkeyKeyset(chord: string): CommandHotkeyKeysetValidation {
  const trigger = chord.split("+").pop() ?? "";
  if (FUNCTION_KEY_TOKENS.has(trigger)) {
    return { valid: true };
  }
  return {
    valid: false,
    reason:
      "Command mode needs a function key (like F13) so the key press won't type into your " +
      "document if it leaks to the focused app while the chord is held.",
  };
}
