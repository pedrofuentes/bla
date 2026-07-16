import { describe, expect, it } from "vitest";
import { validateBaseDir } from "./baseDir";

describe("validateBaseDir", () => {
  // -------------------------------------------------------------------
  // Platform-independent: blank/whitespace-only always clears the override,
  // regardless of which runtime platform is validating.
  // -------------------------------------------------------------------

  it.each(["windows", "unix"] as const)(
    "accepts an empty value on %s (falls back to bla's app-data folder)",
    (platform) => {
      expect(validateBaseDir("", platform)).toEqual({ valid: true });
    },
  );

  it.each(["windows", "unix"] as const)(
    "accepts a whitespace-only value on %s the same as empty",
    (platform) => {
      expect(validateBaseDir("   ", platform)).toEqual({ valid: true });
    },
  );

  // -------------------------------------------------------------------
  // unix runtime (macOS/Linux — resolve_base_dir's Path::is_absolute rule:
  // a leading "/").
  // -------------------------------------------------------------------

  describe("on a unix runtime", () => {
    it("accepts a POSIX absolute path", () => {
      expect(validateBaseDir("/Users/cofounder/Obsidian/Vault", "unix")).toEqual({ valid: true });
    });

    it("treats a leading/trailing-whitespace path per its trimmed absoluteness", () => {
      expect(validateBaseDir("  /Users/cofounder/Vault  ", "unix")).toEqual({ valid: true });
      const result = validateBaseDir("  Vault  ", "unix");
      expect(result.valid).toBe(false);
    });

    it("rejects a relative path", () => {
      const result = validateBaseDir("Obsidian/Vault", "unix");
      expect(result.valid).toBe(false);
      if (!result.valid) expect(result.reason).toMatch(/absolute/i);
    });

    it("rejects a relative path with a leading ./", () => {
      const result = validateBaseDir("./Vault", "unix");
      expect(result.valid).toBe(false);
    });

    it("rejects a tilde-prefixed path — the backend never expands ~", () => {
      const result = validateBaseDir("~/Obsidian/Vault", "unix");
      expect(result.valid).toBe(false);
      if (!result.valid) expect(result.reason).toMatch(/~/);
    });

    // Issue #246 (Sentinel on PR #245): a Windows absolute form is no
    // longer silently accepted on a unix runtime — it's absolute on
    // Windows, not here, and resolve_base_dir would PathBuf::from it
    // verbatim, producing a nonsensical relative-looking path on this OS.
    it("rejects a Windows drive-absolute path (backslash separator) as a foreign-platform form", () => {
      const result = validateBaseDir("C:\\Users\\cofounder\\Vault", "unix");
      expect(result.valid).toBe(false);
      if (!result.valid) expect(result.reason).toMatch(/not an absolute path on this system/i);
    });

    it("rejects a Windows drive-absolute path (forward-slash separator) as a foreign-platform form", () => {
      const result = validateBaseDir("C:/Users/cofounder/Vault", "unix");
      expect(result.valid).toBe(false);
      if (!result.valid) expect(result.reason).toMatch(/not an absolute path on this system/i);
    });

    it("rejects a Windows UNC path as a foreign-platform form", () => {
      const result = validateBaseDir("\\\\server\\share\\Vault", "unix");
      expect(result.valid).toBe(false);
      if (!result.valid) expect(result.reason).toMatch(/not an absolute path on this system/i);
    });
  });

  // -------------------------------------------------------------------
  // windows runtime (resolve_base_dir's Path::is_absolute rule there: a
  // drive-letter prefix or a UNC root — NOT a bare leading "/", which
  // Path::is_absolute treats as drive-relative on Windows).
  // -------------------------------------------------------------------

  describe("on a windows runtime", () => {
    it("accepts a Windows absolute path with a backslash separator", () => {
      expect(validateBaseDir("C:\\Users\\cofounder\\Vault", "windows")).toEqual({ valid: true });
    });

    it("accepts a Windows absolute path with a forward-slash separator", () => {
      expect(validateBaseDir("C:/Users/cofounder/Vault", "windows")).toEqual({ valid: true });
    });

    it("accepts a Windows UNC path", () => {
      expect(validateBaseDir("\\\\server\\share\\Vault", "windows")).toEqual({ valid: true });
    });

    it("rejects a relative path", () => {
      const result = validateBaseDir("Obsidian\\Vault", "windows");
      expect(result.valid).toBe(false);
      if (!result.valid) expect(result.reason).toMatch(/absolute/i);
    });

    it("rejects a tilde-prefixed path — the backend never expands ~", () => {
      const result = validateBaseDir("~\\Obsidian\\Vault", "windows");
      expect(result.valid).toBe(false);
      if (!result.valid) expect(result.reason).toMatch(/~/);
    });

    // Issue #246: the brief's explicit edge case — a bare leading "/" is
    // NOT absolute on Windows (Rust's Path::is_absolute agrees: it's
    // "has a root but no prefix", i.e. drive-relative), so it must be
    // rejected, not silently accepted the way #245 accepted it.
    it("rejects a POSIX-style leading slash as drive-relative, not absolute, on Windows", () => {
      const result = validateBaseDir("/Users/cofounder/Obsidian/Vault", "windows");
      expect(result.valid).toBe(false);
      if (!result.valid) expect(result.reason).toMatch(/not an absolute path on this system/i);
    });
  });
});
