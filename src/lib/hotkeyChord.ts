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
