import { describe, expect, it } from "vitest";
import { validateBaseDir } from "./baseDir";

describe("validateBaseDir", () => {
  it("accepts an empty value (falls back to bla's app-data folder)", () => {
    expect(validateBaseDir("")).toEqual({ valid: true });
  });

  it("accepts a whitespace-only value the same as empty", () => {
    expect(validateBaseDir("   ")).toEqual({ valid: true });
  });

  it("accepts a POSIX absolute path", () => {
    expect(validateBaseDir("/Users/cofounder/Obsidian/Vault")).toEqual({ valid: true });
  });

  it("accepts a Windows absolute path with a backslash separator", () => {
    expect(validateBaseDir("C:\\Users\\cofounder\\Vault")).toEqual({ valid: true });
  });

  it("accepts a Windows absolute path with a forward-slash separator", () => {
    expect(validateBaseDir("C:/Users/cofounder/Vault")).toEqual({ valid: true });
  });

  it("accepts a Windows UNC path", () => {
    expect(validateBaseDir("\\\\server\\share\\Vault")).toEqual({ valid: true });
  });

  it("rejects a relative path", () => {
    const result = validateBaseDir("Obsidian/Vault");
    expect(result.valid).toBe(false);
    if (!result.valid) expect(result.reason).toMatch(/absolute/i);
  });

  it("rejects a relative path with a leading ./", () => {
    const result = validateBaseDir("./Vault");
    expect(result.valid).toBe(false);
  });

  it("rejects a tilde-prefixed path — the backend never expands ~", () => {
    const result = validateBaseDir("~/Obsidian/Vault");
    expect(result.valid).toBe(false);
    if (!result.valid) expect(result.reason).toMatch(/~/);
  });

  it("treats a leading/trailing-whitespace path per its trimmed absoluteness", () => {
    expect(validateBaseDir("  /Users/cofounder/Vault  ")).toEqual({ valid: true });
    const result = validateBaseDir("  Vault  ");
    expect(result.valid).toBe(false);
  });
});
