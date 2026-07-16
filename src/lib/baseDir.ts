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
 * ## Issue #246 (Sentinel SNTL-20260716-bla-PR245-6936364 🟡 on PR #245):
 * validate against the RUNTIME platform, not either platform's syntax
 *
 * #245 fixed #210's same-platform case but accepted EITHER platform's
 * absolute syntax regardless of which OS this build actually runs on — a
 * synced `settings.json` (e.g. via a cloud-synced app-data dir) carrying a
 * Windows `C:\...` form onto macOS, or a POSIX `/...` form onto Windows,
 * passed validation even though `resolve_base_dir` runs Rust-side and
 * `PathBuf::from` of the foreign form isn't absolute on THAT machine —
 * reproducing #210's CWD-relative-write failure mode in that cross-platform
 * edge case.
 *
 * `validateBaseDir` now takes the runtime platform as an explicit parameter
 * (a {@link RuntimePlatform}, fetched once via the `get_platform` command —
 * `src-tauri/src/commands.rs::get_platform`, a thin wrapper over a pure
 * `cfg!(windows)` branch, since Tauri never cross-compiles at runtime: the
 * binary IS the platform it's running on) and accepts ONLY that platform's
 * absolute form — mirroring exactly what Rust's `std::path::Path::is_absolute`
 * considers absolute per target:
 *
 * - **windows**: a drive-letter prefix (`C:\` or `C:/`) or a UNC root
 *   (`\\server\share`). A bare leading `/` (e.g. `/foo`) is REJECTED here —
 *   on Windows that has a root but no prefix, i.e. it's drive-relative
 *   (resolves against whichever drive happens to be current at write time),
 *   NOT absolute, exactly matching `Path::is_absolute`'s own rule.
 * - **unix** (macOS/Linux — every non-Windows target `resolve_base_dir`
 *   actually ships on): a leading `/`. A Windows drive/UNC form is rejected
 *   — it isn't a meaningful path on a POSIX runtime at all.
 *
 * A foreign-platform absolute form gets its own distinct, clear inline error
 * ("Not an absolute path on this system…") rather than being folded into the
 * generic "must be absolute" message — the string IS absolute *somewhere*,
 * just not here, and the error says so rather than leaving the user to
 * guess why an apparently-valid path was rejected.
 */

export type BaseDirValidation = { valid: true } | { valid: false; reason: string };

/**
 * The runtime platform {@link validateBaseDir} validates against. Mirrors
 * the two branches `std::path::Path::is_absolute` actually uses on the Rust
 * side (`resolve_base_dir`'s target) — not every OS name Rust itself
 * distinguishes (`std::env::consts::OS` has `"macos"`, `"linux"`, …):
 * absoluteness rules are identical across every non-Windows target, so they
 * all collapse to `"unix"` here.
 */
export type RuntimePlatform = "windows" | "unix";

const WINDOWS_DRIVE_ABSOLUTE = /^[A-Za-z]:[\\/]/;
const WINDOWS_UNC_PREFIX = /^\\\\/;

export function validateBaseDir(value: string, platform: RuntimePlatform): BaseDirValidation {
  const trimmed = value.trim();

  // Blank clears the override — `resolve_base_dir` falls back to bla's
  // app-data folder, so there's nothing to validate.
  if (trimmed.length === 0) {
    return { valid: true };
  }

  const isPosixAbsolute = trimmed.startsWith("/");
  const isWindowsAbsolute =
    WINDOWS_DRIVE_ABSOLUTE.test(trimmed) || WINDOWS_UNC_PREFIX.test(trimmed);

  if (platform === "windows" ? isWindowsAbsolute : isPosixAbsolute) {
    return { valid: true };
  }

  // Issue #246: the OTHER platform's absolute form is absolute somewhere,
  // just not on the machine `resolve_base_dir` is about to run on — a
  // distinct rejection reason from "not absolute at all", so the message
  // says so instead of implying the value is malformed.
  if (platform === "windows" && isPosixAbsolute) {
    return {
      valid: false,
      reason:
        'Not an absolute path on this system — a leading "/" alone is relative to the ' +
        'current drive on Windows, not absolute. Use e.g. "C:\\Users\\you\\Vault".',
    };
  }
  if (platform === "unix" && isWindowsAbsolute) {
    return {
      valid: false,
      reason:
        "Not an absolute path on this system — Windows-style drive/UNC paths aren't " +
        'meaningful here. Use e.g. "/Users/you/Vault".',
    };
  }

  if (trimmed.startsWith("~")) {
    return {
      valid: false,
      reason:
        'Path must be absolute — "~" is not expanded. Use the full path (e.g. ' +
        (platform === "windows" ? '"C:\\Users\\you\\Vault").' : '"/Users/you/Vault").'),
    };
  }

  return {
    valid: false,
    reason:
      platform === "windows"
        ? 'Path must be absolute (e.g. "C:\\Users\\you\\Vault" or "\\\\server\\share\\Vault").'
        : 'Path must be absolute (e.g. "/Users/you/Vault").',
  };
}
