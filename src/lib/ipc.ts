/**
 * Typed wrapper around Tauri's `invoke`/`listen`, mirroring
 * `src-tauri/src/commands.rs` and the events `src-tauri/src/lib.rs` emits.
 *
 * The UI must call the core only through this module (docs/ARCHITECTURE.md
 * §Module Boundaries) — never `@tauri-apps/api` directly from a component —
 * so every IPC call/event subscription has a single, typed, mockable seam
 * for Playwright screenshots of the settings window and status window in a
 * plain browser.
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";
import type { RuntimePlatform } from "./baseDir";

/** Mirrors `settings::RecordingMode` (src-tauri/src/settings.rs). */
export type RecordingMode = "Hold" | "Toggle";

/** Mirrors `settings::ModelPreset` (src-tauri/src/settings.rs). */
export type ModelPreset = "LargeV3Turbo" | "Small";

/** Mirrors `settings::OutputModeSetting` (src-tauri/src/settings.rs). */
export type OutputModeSetting = "Cursor" | "File";

/** Mirrors `settings::Settings` (src-tauri/src/settings.rs). */
export interface Settings {
  hotkey: string;
  recording_mode: RecordingMode;
  model_preset: ModelPreset;
  output_mode: OutputModeSetting;
  file_path_template: string;
  /**
   * Issue #180: the settings-window picker's "base folder / vault" field
   * for file-mode output (e.g. an Obsidian vault path) — `file_path_template`
   * resolves against this. Optional here (rather than required, like the
   * Rust side's `#[serde(default)]` `String`) purely so TS object literals
   * built before this field existed keep type-checking; treat a missing or
   * empty value as "use bla's app-data folder", mirroring
   * `output::resolve_base_dir`.
   */
  file_base_dir?: string;
  /** Issue #126, M2 PR 2.6: opt-in OS login autostart. Defaults to `false`. */
  launch_at_login: boolean;
  /**
   * Issue #126, M2 PR 2.6: play short audio cues on recording start/stop.
   * Defaults to `true`. Pure persisted preference in this PR — cue
   * playback itself lands in PR 2.7, which reads this flag.
   */
  sound_cues: boolean;
  /**
   * Issue #198/#199: how many days of dictation history to keep before
   * it's eligible for pruning — `0` means "keep forever", mirroring
   * `settings::Settings::retention_days`/`store::retention_cutoff_ms`'s
   * contract. Optional here for the same reason as `file_base_dir` above:
   * TS object literals built before this field existed (e.g.
   * `GeneralTab.test.tsx`'s `BASE_SETTINGS`, `settings-harness.tsx`'s
   * fixtures) keep type-checking; treat a missing value as `0`.
   */
  retention_days?: number;
  /**
   * Issue #259 (M4 command-mode backbone): the command-mode global hotkey,
   * persisted independently of `hotkey` — mirrors
   * `settings::Settings::command_hotkey`. That Rust field carries no
   * `#[serde(rename_all)]` (its struct-level attribute only governs missing-
   * field defaulting, not casing), so its JSON key is the literal snake_case
   * field name, `command_hotkey` — verified directly against
   * `src-tauri/src/settings.rs` for issue #262, the same way #237's
   * camelCase/snake_case wire mismatch should have been caught. Optional
   * here for the same reason as `file_base_dir`/`retention_days` above: TS
   * object literals built before this field existed keep type-checking;
   * the settings window's command-hotkey field (#262) treats a missing
   * value as an empty pending display, since the backend's own
   * `#[serde(default)]` guarantees a real load always has one.
   */
  command_hotkey?: string;
}

/**
 * Mirrors `store::HistoryRow` (src-tauri/src/store.rs) — one row returned
 * by `search_history`. Carries the user's own transcript text (`raw` /
 * `cleaned`): sanctioned to render in the History tab (#199), but never
 * `console.log`/persist it anywhere else (MISSION §5/§7).
 */
export interface HistoryRow {
  id: number;
  created_at_ms: number;
  raw: string;
  cleaned: string;
  app_name?: string | null;
}

/** Mirrors `models::DownloadProgress` (src-tauri/src/models.rs). */
export interface DownloadProgress {
  bytes_downloaded: number;
  total_bytes: number;
  /** `0.0..=100.0`. */
  percent: number;
}

/** The two `Ok` shapes `commands::download_selected_model` returns. */
export type DownloadStartResult = "already-present" | "downloading";

/**
 * Mirrors `ModelRegistryEntry` (src-tauri/src/lib.rs) — one entry of
 * `commands::model_registry`'s per-preset size data (issue #184), used by
 * the settings model picker to render e.g. "Small — 488 MB" via
 * `formatBytes`.
 */
export interface ModelRegistryEntry {
  preset: ModelPreset;
  size_bytes: number;
}

/**
 * Mirrors `errors::PipelineErrorEvent` (src-tauri/src/errors.rs) — the
 * `pipeline-error` event payload. `kind` is one of `errors::ErrorKind`'s
 * discriminants (`"ModelMissing" | "OllamaUnreachable" |
 * "MicPermissionDenied" | "Other"`), kept as `string` here rather than a
 * union so an unrecognized future kind still type-checks instead of a hard
 * TS compile error. `message` is always static/kind-derived on the Rust
 * side (never transcript/clipboard/audio content — see that module's HARD
 * RULE) and safe to render as-is.
 */
export interface PipelineErrorEvent {
  kind: string;
  message: string;
}

/**
 * Mirrors `store::DictionaryTerm` (src-tauri/src/store.rs) — one row
 * returned by `list_dictionary_terms`. Carries the user's own personal-
 * dictionary vocabulary: sanctioned to render in the Dictionary tab
 * (#201), but never `console.log`/persist it anywhere else (MISSION §5/§7)
 * — the same no-log invariant `HistoryRow` documents above.
 */
export interface DictionaryTerm {
  id: number;
  term: string;
  created_at_ms: number;
}

/**
 * Mirrors `store::ToneProfile` (src-tauri/src/store.rs) — deliberately
 * narrower than the pipeline's own tone enum (no `neutral`; the *absence* of
 * a matching rule is what dispatches neutral, never a value stored here).
 * Lowercase on the wire (`#[serde(rename_all = "lowercase")]` on the Rust
 * side) — do not PascalCase these like `RecordingMode`/`ModelPreset`.
 */
export type ToneProfile = "casual" | "formal" | "verbatim";

/**
 * Mirrors `store::ToneRule` (src-tauri/src/store.rs) — one per-app tone
 * override returned by `list_tone_rules`, in insertion order (`id` ASC),
 * which is also first-match-wins match order (`context::resolve_tone_for_app`
 * walks this same order). Carries `app_pattern`: user-environment data (an
 * installed app's identifier/glob pattern), not transcript/clipboard
 * content, but still never `console.log`/persisted anywhere outside the
 * Tone tab (#203) that renders it.
 */
export interface ToneRule {
  id: number;
  app_pattern: string;
  tone: ToneProfile;
  created_at_ms: number;
}

/**
 * Mirrors `store::Snippet` (src-tauri/src/store.rs) — one stored text
 * snippet returned by `list_snippets`: a trigger phrase spoken during
 * dictation and the body text it expands to (the actual matching against a
 * transcript is `snippets::match_snippet`'s job, issue #260, not this
 * type). `list_snippets` returns rows most-recently-added first, mirroring
 * `DictionaryTerm`'s ordering (NOT `ToneRule`'s insertion/match order).
 * Carries the user's own content — sanctioned to render in the Snippets tab
 * (#261), but never `console.log`/persist it anywhere else (MISSION §5/§7),
 * the same no-log invariant `HistoryRow`/`DictionaryTerm`/`ToneRule`
 * document.
 */
export interface Snippet {
  id: number;
  trigger: string;
  body: string;
  created_at_ms: number;
}

/**
 * Command name → { args, result } typing. Extend this map as `commands.rs`
 * grows; each key must match a `#[tauri::command]` name exactly.
 */
export interface Commands {
  get_settings: { result: Settings };
  /**
   * Mirrors `commands::get_platform` (issue #246) — the RUNTIME platform
   * this Tauri binary is running on (`"windows"` or `"unix"`, matching
   * `std::path::Path::is_absolute`'s two branches, not every OS name Rust
   * itself distinguishes). `GeneralTab.tsx` fetches this once on mount and
   * passes it to `validateBaseDir` (`src/lib/baseDir.ts`) so a
   * foreign-platform absolute base-folder form (e.g. a synced
   * `settings.json`'s `C:\...` on macOS) is rejected instead of silently
   * accepted regardless of what `output::resolve_base_dir` — which runs
   * Rust-side against THIS platform — will actually do with it.
   */
  get_platform: { result: RuntimePlatform };
  set_settings: { args: { settings: Settings }; result: void };
  set_output_mode: { args: { mode: OutputModeSetting }; result: void };
  /** Mirrors `commands::validate_hotkey` — thin wrapper over `hotkeys::validate_hotkey`. */
  validate_hotkey: { args: { accelerator: string }; result: void };
  /**
   * Mirrors `commands::validate_command_hotkey` (issue #281, ac7-p0):
   * `validate_hotkey`'s general grammar check PLUS the command-mode-specific
   * function-key-trigger keyset constraint
   * (`hotkeys::validate_command_hotkey_keyset`) — a leaked keydown can't be
   * suppressed on either OS, so the command-mode hotkey's trigger key must
   * be a function key (F1-F24), which produces no text character if it
   * leaks to the focused app while the chord is held. `GeneralTab.tsx`'s
   * command-hotkey field calls this instead of `validate_hotkey` for its
   * capture-time probe — the dictation-hotkey field keeps using the plain
   * `validate_hotkey` (this fix deliberately does not touch dictation-hotkey
   * validation).
   */
  validate_command_hotkey: { args: { accelerator: string }; result: void };
  download_selected_model: { result: DownloadStartResult };
  /** Mirrors `commands::model_registry` (issue #184). */
  model_registry: { result: ModelRegistryEntry[] };
  /**
   * Mirrors `commands::suspend_hotkey` (issue #181). `generation` is a
   * monotonic token minted by this window and echoed back on `resume_hotkey`
   * so an out-of-order resume can't re-enable the shortcut during a newer
   * capture (PR #185). See `GeneralTab.tsx`'s concurrency-model doc comment.
   */
  suspend_hotkey: { args: { generation: number }; result: void };
  /** Mirrors `commands::resume_hotkey` (issue #181) — see `GeneralTab.tsx`. */
  resume_hotkey: { args: { generation: number }; result: void };
  /**
   * Mirrors `commands::search_history` (issue #198/#199) — substring
   * search over dictation history, newest first, capped at `limit` rows.
   * The History tab's (#199) sole source of rows to render.
   */
  search_history: { args: { query: string; limit: number }; result: HistoryRow[] };
  /**
   * Mirrors `commands::copy_history_entry` (issue #198/#199) — copies one
   * entry's cleaned transcript to the clipboard; the clipboard routing is
   * entirely backend-side (never a value this call returns or logs).
   */
  copy_history_entry: { args: { id: number }; result: void };
  /** Mirrors `commands::delete_history_entry` (issue #198/#199). */
  delete_history_entry: { args: { id: number }; result: void };
  /** Mirrors `commands::clear_history` (issue #198/#199) — the History tab's "Clear all". */
  clear_history: { result: void };
  /**
   * Mirrors `commands::list_dictionary_terms` (issue #200/#201) —
   * every personal-dictionary term, most-recently-added first.
   */
  list_dictionary_terms: { result: DictionaryTerm[] };
  /**
   * Mirrors `commands::add_dictionary_term` (issue #200/#201). The
   * backend's `dictionary(term UNIQUE COLLATE NOCASE)` constraint makes a
   * case-insensitive duplicate an `INSERT OR IGNORE` no-op that still
   * resolves with the existing row's id — it is never a rejected call, so
   * the Dictionary tab checks for a case-insensitive duplicate against its
   * already-loaded list itself before calling this (see
   * `DictionaryTab.tsx`'s doc comment).
   */
  add_dictionary_term: { args: { term: string }; result: number };
  /** Mirrors `commands::remove_dictionary_term` (issue #200/#201). */
  remove_dictionary_term: { args: { id: number }; result: void };
  /**
   * Mirrors `commands::list_tone_rules` (issue #202/#203) — every per-app
   * tone rule, in insertion order (`id` ASC), which the Tone tab (#203)
   * renders as-is since that order is also first-match-wins match order.
   */
  list_tone_rules: { result: ToneRule[] };
  /**
   * Mirrors `commands::upsert_tone_rule` (issue #202/#203, PRD AC-22/AC-41).
   * Re-submitting an existing `app_pattern` (case-insensitively) UPDATES
   * that rule's tone in place rather than adding a second row — this is how
   * the Tone tab implements "edit a rule's tone" (AC-44), reusing the same
   * call rather than a separate update-by-id command, which doesn't exist.
   * Returns the rule's row id either way. Because a duplicate pattern is a
   * silent update rather than a rejected call, the Tone tab checks for a
   * case-insensitive duplicate against its already-loaded list itself
   * before calling this for an *add* (see `ToneTab.tsx`'s doc comment) —
   * the same pattern `DictionaryTab.tsx` uses for `add_dictionary_term`.
   */
  upsert_tone_rule: { args: { app_pattern: string; tone: ToneProfile }; result: number };
  /** Mirrors `commands::delete_tone_rule` (issue #202/#203). */
  delete_tone_rule: { args: { id: number }; result: void };
  /**
   * Mirrors `commands::list_snippets` (issue #258/#261) — every stored
   * snippet, most-recently-added first, the Snippets tab's (#261) sole
   * source of rows to render.
   */
  list_snippets: { result: Snippet[] };
  /**
   * Mirrors `commands::add_snippet` (issue #258/#261). The backend's
   * `snippets(trigger UNIQUE COLLATE NOCASE)` constraint makes a
   * case-insensitively duplicate trigger an `INSERT OR IGNORE` no-op that
   * still resolves with the existing row's id — it is never a rejected
   * call, so the Snippets tab checks for a case-insensitive duplicate
   * against its already-loaded list itself before calling this for an
   * *add* (see `SnippetsTab.tsx`'s doc comment, the same pattern
   * `DictionaryTab.tsx` uses for `add_dictionary_term`).
   */
  add_snippet: { args: { trigger: string; body: string }; result: number };
  /**
   * Mirrors `commands::update_snippet` (issue #258/#261) — edits an
   * existing snippet's trigger/body by id. UNLIKE `add_snippet` (and
   * unlike `upsert_tone_rule`), a new `trigger` colliding
   * case-insensitively with a DIFFERENT row's trigger genuinely rejects
   * this call (the schema's `UNIQUE COLLATE NOCASE` constraint is enforced
   * on UPDATE too), which the Snippets tab surfaces as a row-scoped
   * kind-only inline error.
   */
  update_snippet: { args: { id: number; trigger: string; body: string }; result: void };
  /** Mirrors `commands::remove_snippet` (issue #258/#261). */
  remove_snippet: { args: { id: number }; result: void };
}

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

/**
 * Event name → payload typing, mirroring every `app.emit(...)` call site in
 * `src-tauri/src/lib.rs`/`commands.rs`.
 */
export interface Events {
  /**
   * The Debug-formatted `tray::TrayIconState` derived from the pipeline's
   * current `tray::PipelineState` (`set_pipeline_state` in lib.rs) — one of
   * `"Idle" | "Active" | "Busy" | "Error"`.
   */
  "pipeline-state-changed": string;
  "model-download-progress": DownloadProgress;
  /**
   * The selected model finished downloading (checksum verified + renamed
   * into place). Emitted from both download threads' success arm so the UI
   * leaves the "Downloading…" state. Unit payload (`null`).
   */
  "model-download-complete": null;
  /** A human-readable error message — never transcript/clipboard text. */
  "model-download-error": string;
  /**
   * The live output mode changed (`commands::set_output_mode`), emitted for
   * either trigger — the status window's toggle button or the tray menu's
   * item — so the window's state can't drift from the tray's.
   */
  "output-mode-changed": OutputModeSetting;
  /**
   * The RMS level (`0.0..=1.0`, clamped in the core poller) of the most
   * recently captured audio chunk during an active dictation, throttled to
   * ~30Hz in the core poller (`audio::LevelThrottle`, `lib.rs`'s
   * level-event poller) so the pill's live meter isn't flooded with one
   * event per audio callback. Only ever a scalar — raw audio samples never
   * leave the core as an event.
   */
  "audio-level": number;
  /**
   * PR #185 Sentinel delta 🟡-3: the settings window was hidden (not
   * destroyed) while its hotkey-capture field was mid-capture. The backend
   * force-restores the OS shortcut on close and emits this so the field
   * leaves capture mode instead of staying stuck (`capturing === true`,
   * swallowing keys) when the window is reopened. Unit payload (`null`).
   */
  "hotkey-capture-reset": null;
  /**
   * A typed pipeline error/notice (issue #126, M2 PR 2.4) — emitted from
   * `lib.rs`'s capture-start failure, `run_pipeline_in_background`'s error
   * paths, and the AC-4 Ollama-unreachable fallback path (informational,
   * alongside a successful dictation, not in place of one). The pill
   * window's toast (`src/windows/pill/Toast.tsx`) is the only current
   * subscriber.
   */
  "pipeline-error": PipelineErrorEvent;
}

/**
 * Subscribe to a Tauri event by name with payload typing from {@link Events}.
 * Returns the `unlisten` function; call it on unmount to avoid leaking the
 * subscription. The single seam through which any component listens for
 * backend-driven state changes, so it stays mockable the same way
 * {@link invoke} is.
 */
export async function onEvent<K extends keyof Events>(
  event: K,
  handler: (payload: Events[K]) => void,
): Promise<UnlistenFn> {
  return tauriListen<Events[K]>(event, (e) => handler(e.payload));
}
