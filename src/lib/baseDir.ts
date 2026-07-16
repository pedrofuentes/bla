/**
 * Client-side absoluteness validation for the file-mode "base folder /
 * vault" field (issue #210, Sentinel SNTL-20260715-bla-PR204-86572a1 🟡 on
 * PR #204's `commitBaseDir`).
 *
 * `output::resolve_base_dir` (`src-tauri/src/output.rs`) uses the
 * configured string **verbatim** (only trimmed) whenever it's non-blank —
 * it never expands `~`, never resolves `.`/`..`, and never checks
 * absoluteness itself:
 *
 * ```rust
 * pub fn resolve_base_dir(configured: &str, app_data_dir: &Path) -> PathBuf {
 *     let trimmed = configured.trim();
 *     if trimmed.is_empty() {
 *         app_data_dir.to_path_buf()
 *     } else {
 *         PathBuf::from(trimmed)
 *     }
 * }
 * ```
 *
 * A relative value therefore becomes a relative `PathBuf` that every
 * downstream file write resolves against the process's current working
 * directory at write time — not the vault the user thought they typed,
 * and not even a consistent location across launches. This mirrors
 * `src/lib/pathTemplate.ts`'s UX (inline error, value withheld from
 * `set_settings`) but validates the opposite property: the path template
 * must be *relative*, the base folder must be *absolute*.
 *
 * Windows semantics: an absolute path there is a drive-letter prefix
 * (`C:\` or `C:/`) or a UNC root (`\\server\share`) — POSIX's leading `/`
 * is accepted too, since a synced settings.json (e.g. via a cloud-synced
 * app-data dir) could carry either platform's form. Anything else,
 * including a bare `~` shorthand the backend never expands, is rejected.
 */

export type BaseDirValidation = { valid: true } | { valid: false; reason: string };

const WINDOWS_DRIVE_ABSOLUTE = /^[A-Za-z]:[\\/]/;
const WINDOWS_UNC_PREFIX = /^\\\\/;

export function validateBaseDir(value: string): BaseDirValidation {
  const trimmed = value.trim();

  // Blank clears the override — `resolve_base_dir` falls back to bla's
  // app-data folder, so there's nothing to validate.
  if (trimmed.length === 0) {
    return { valid: true };
  }

  if (
    trimmed.startsWith("/") ||
    WINDOWS_DRIVE_ABSOLUTE.test(trimmed) ||
    WINDOWS_UNC_PREFIX.test(trimmed)
  ) {
    return { valid: true };
  }

  if (trimmed.startsWith("~")) {
    return {
      valid: false,
      reason:
        'Path must be absolute — "~" is not expanded. Use the full path (e.g. "/Users/you/Vault").',
    };
  }

  return {
    valid: false,
    reason: 'Path must be absolute (e.g. "/Users/you/Vault" or "C:\\Users\\you\\Vault").',
  };
}
