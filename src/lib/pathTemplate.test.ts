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
});
