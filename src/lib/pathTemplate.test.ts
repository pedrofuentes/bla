import { describe, expect, it } from "vitest";
import { validatePathTemplate } from "./pathTemplate";

describe("validatePathTemplate", () => {
  it("accepts a plain date-templated filename", () => {
    expect(validatePathTemplate("{{date:YYYY-MM-DD}}.md")).toEqual({ valid: true });
  });

  it("accepts a nested relative path with a timestamp prefix template", () => {
    expect(validatePathTemplate("daily/{{date:YYYY-MM-DD}}.md")).toEqual({ valid: true });
  });

  it("accepts a template whose date token itself expands with slashes", () => {
    expect(validatePathTemplate("{{date:YYYY/MM/DD}}/note.md")).toEqual({ valid: true });
  });

  it("accepts a backtrack that nets out within the base folder", () => {
    expect(validatePathTemplate("daily/../{{date:YYYY-MM-DD}}.md")).toEqual({ valid: true });
  });

  it("rejects an empty template", () => {
    const result = validatePathTemplate("");
    expect(result.valid).toBe(false);
    if (!result.valid) expect(result.reason).toMatch(/empty/i);
  });

  it("rejects a whitespace-only template", () => {
    const result = validatePathTemplate("   ");
    expect(result.valid).toBe(false);
  });

  it("rejects an absolute path template", () => {
    const result = validatePathTemplate("/etc/passwd");
    expect(result.valid).toBe(false);
    if (!result.valid) expect(result.reason).toMatch(/relative|absolute/i);
  });

  it("rejects a template that escapes the base folder via ..", () => {
    const result = validatePathTemplate("../../etc/{{date:YYYY-MM-DD}}.md");
    expect(result.valid).toBe(false);
    if (!result.valid) expect(result.reason).toMatch(/escapes|base folder/i);
  });

  it("rejects a template that dips below the base dir even if it would net back above zero", () => {
    const result = validatePathTemplate("../vault/notes.md");
    expect(result.valid).toBe(false);
  });

  // -------------------------------------------------------------------
  // Sentinel SNTL-20260715-bla-PR204-86572a1 🔴: this validator only rejected
  // POSIX-absolute paths. On a real Windows build, `output::confine_relative_path`
  // parses paths with Windows semantics (drive letters and UNC roots are a
  // `Component::Prefix`, and `\` is also a valid separator) and rejects a
  // drive-letter/UNC/backslash-traversal template as AbsolutePath/
  // EscapesBaseDir at *dictation* time — but this client-side check let the
  // user save one with no inline error, so the failure only surfaces as a
  // silently dropped dictation later. Fixed by rejecting `\` outright (the
  // product's own templating convention is forward-slash-only — see
  // output.rs's issue #98 tests) and any leading Windows drive-letter
  // prefix, so client and authority agree on every platform.
  // -------------------------------------------------------------------

  it("rejects a Windows drive-letter template with backslash separators", () => {
    const result = validatePathTemplate("C:\\notes\\{{date:YYYY-MM-DD}}.md");
    expect(result.valid).toBe(false);
  });

  it("rejects a Windows drive-letter template with forward-slash separators", () => {
    const result = validatePathTemplate("C:/notes/x.md");
    expect(result.valid).toBe(false);
  });

  it("rejects a UNC path template", () => {
    const result = validatePathTemplate("\\\\server\\share\\x.md");
    expect(result.valid).toBe(false);
  });

  it("rejects backslash-separated traversal that would escape the base folder", () => {
    const result = validatePathTemplate("..\\escape.md");
    expect(result.valid).toBe(false);
  });

  it("rejects a template using backslash separators even when it would otherwise be a plain relative path", () => {
    // The product convention is forward-slash-only (output.rs issue #98) —
    // backslash is never an accepted separator, regardless of whether it
    // would resolve to something harmless.
    const result = validatePathTemplate("notes\\{{date:YYYY-MM-DD}}.md");
    expect(result.valid).toBe(false);
  });
});
