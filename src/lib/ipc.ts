/**
 * Typed wrapper around Tauri's `invoke`, mirroring `src-tauri/src/commands.rs`.
 *
 * The UI must call the core only through this module (docs/ARCHITECTURE.md
 * §Module Boundaries) — never `@tauri-apps/api` directly from a component —
 * so every IPC call has a single, typed, mockable seam for Playwright screenshots
 * of the settings window and recording pill in a plain browser.
 *
 * Stub — no commands are wired yet; this file just establishes the pattern.
 * Real commands are added here as `commands.rs` grows its handlers.
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";

/**
 * Command name → { args, result } typing. Extend this map as `commands.rs`
 * grows; each key must match a `#[tauri::command]` name exactly.
 */
// eslint-disable-next-line @typescript-eslint/no-empty-object-type
export interface Commands {}

/**
 * Invoke a Tauri command by name with full type inference from {@link Commands}.
 * Swap the implementation for a mock in tests/Playwright by overriding this
 * module's export.
 */
export async function invoke<K extends keyof Commands>(
  command: K,
  args?: Commands[K] extends { args: infer A } ? A : never,
): Promise<Commands[K] extends { result: infer R } ? R : never> {
  return tauriInvoke(command as string, args as Record<string, unknown>);
}
