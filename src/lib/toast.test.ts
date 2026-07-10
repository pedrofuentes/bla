import { describe, expect, it } from "vitest";
import { toastForError } from "./toast";

describe("toastForError", () => {
  it("treats OllamaUnreachable as informational (AC-4 fallback still pastes)", () => {
    expect(
      toastForError({
        kind: "OllamaUnreachable",
        message: "Local AI cleanup is unreachable; used basic cleanup instead.",
      }),
    ).toEqual({
      tone: "informational",
      message: "Local AI cleanup is unreachable; used basic cleanup instead.",
    });
  });

  it("treats ModelMissing as blocking", () => {
    expect(toastForError({ kind: "ModelMissing", message: "x" }).tone).toBe("blocking");
  });

  it("treats MicPermissionDenied as blocking", () => {
    expect(toastForError({ kind: "MicPermissionDenied", message: "x" }).tone).toBe("blocking");
  });

  it("treats Other as blocking", () => {
    expect(toastForError({ kind: "Other", message: "x" }).tone).toBe("blocking");
  });

  it("treats an unrecognized future kind as blocking (safe default)", () => {
    expect(toastForError({ kind: "SomethingNew", message: "x" }).tone).toBe("blocking");
  });

  it("passes the message through unchanged", () => {
    expect(toastForError({ kind: "Other", message: "custom message" }).message).toBe(
      "custom message",
    );
  });
});
