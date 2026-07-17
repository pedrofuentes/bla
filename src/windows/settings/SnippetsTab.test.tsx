import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { blur, click, flush, focus, mount, typeInto, type Mounted } from "../../testUtils";
import type { Snippet } from "../../lib/ipc";
import { SnippetsTab } from "./SnippetsTab";

const invoke = vi.fn();

/**
 * `testUtils.ts`'s `typeInto` reads through `HTMLInputElement.prototype`'s
 * native `value` setter specifically, which throws on a real
 * `<textarea>` node — the snippet body field is a `<textarea>` (issue
 * #261, multi-line bodies), so this local variant does the identical
 * React-controlled-input workaround (see `typeInto`'s own doc comment)
 * against `HTMLTextAreaElement.prototype` instead. Kept local to this test
 * file rather than added to the shared `testUtils.ts` (out of this PR's
 * file scope).
 */
function typeIntoTextArea(el: HTMLTextAreaElement, value: string): void {
  const nativeValueSetter = Object.getOwnPropertyDescriptor(
    window.HTMLTextAreaElement.prototype,
    "value",
  )!.set!;
  act(() => {
    nativeValueSetter.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

/**
 * Dispatches blur/focusout directly (rather than `testUtils.ts`'s `blur()`,
 * which calls the native `.blur()` method — a no-op on an element that
 * isn't `document.activeElement`, and a `disabled` element can never
 * BECOME `document.activeElement` via `.focus()` in the first place). Used
 * only for the second of two rapid edits to the SAME row below, where the
 * row's fields are already `disabled` (mid-flight from the first edit) by
 * the time the second edit needs to commit — exercising the generation
 * guard requires firing that second commit while genuinely disabled, which
 * a real user could never do, but which this synthetic dispatch can.
 */
function forceBlur(el: HTMLElement): void {
  act(() => {
    el.dispatchEvent(new FocusEvent("blur", { bubbles: true, cancelable: true }));
    el.dispatchEvent(new FocusEvent("focusout", { bubbles: true, cancelable: true }));
  });
}

// `vi.mock` factories are hoisted above imports by vitest, so `SnippetsTab`
// above resolves against this mocked `../../lib/ipc` — the module under
// test never touches the real Tauri `invoke`.
vi.mock("../../lib/ipc", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Privacy (MISSION §5/§7): snippet triggers/bodies are user content — every
// fixture here is an obviously synthetic placeholder, never real dictated
// text, and no test ever console.logs one.
const SNIPPET_A: Snippet = {
  id: 1,
  trigger: "sig",
  body: "Best, Placeholder Pat",
  created_at_ms: Date.parse("2026-07-10T09:00:00Z"),
};
const SNIPPET_B: Snippet = {
  id: 2,
  trigger: "addr",
  body: "123 Placeholder St",
  created_at_ms: Date.parse("2026-07-11T09:00:00Z"),
};

function setupInvoke(overrides: Partial<Record<string, (...args: unknown[]) => unknown>> = {}) {
  invoke.mockImplementation((command: string, args?: unknown) => {
    if (overrides[command]) return Promise.resolve(overrides[command]!(args));
    switch (command) {
      case "list_snippets":
        // Newest-first, mirroring `Store::list_snippets`'s real ordering.
        return Promise.resolve([SNIPPET_B, SNIPPET_A]);
      case "add_snippet":
        return Promise.resolve(3);
      case "update_snippet":
        return Promise.resolve(undefined);
      case "remove_snippet":
        return Promise.resolve(undefined);
      default:
        return Promise.reject(new Error(`unmocked command ${command}`));
    }
  });
}

let mounted: Mounted | undefined;

beforeEach(() => {
  invoke.mockReset();
  setupInvoke();
});

afterEach(() => {
  mounted?.unmount();
  mounted = undefined;
});

async function addSnippet(container: HTMLElement, trigger: string, body: string) {
  const triggerInput = container.querySelector<HTMLInputElement>(
    '[data-testid="snippets-add-trigger-input"]',
  )!;
  typeInto(triggerInput, trigger);
  const bodyInput = container.querySelector<HTMLTextAreaElement>(
    '[data-testid="snippets-add-body-input"]',
  )!;
  typeIntoTextArea(bodyInput, body);
  click(container.querySelector('[data-testid="snippets-add-button"]')!);
  await flush();
}

describe("SnippetsTab (list rendering)", () => {
  it("lists the snippets returned by list_snippets on mount, most-recently-added first", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();

    expect(invoke).toHaveBeenCalledWith("list_snippets");
    const triggers = Array.from(
      mounted.container.querySelectorAll<HTMLInputElement>('[data-testid^="snippet-trigger-"]'),
    );
    expect(triggers.map((el) => el.value)).toEqual([SNIPPET_B.trigger, SNIPPET_A.trigger]);
    const bodies = Array.from(
      mounted.container.querySelectorAll<HTMLTextAreaElement>('[data-testid^="snippet-body-"]'),
    );
    expect(bodies.map((el) => el.value)).toEqual([SNIPPET_B.body, SNIPPET_A.body]);
  });

  it("shows a loading state before the first list resolves", async () => {
    let resolveList!: (rows: Snippet[]) => void;
    setupInvoke({
      list_snippets: () =>
        new Promise((resolve) => {
          resolveList = resolve;
        }),
    });

    mounted = mount(<SnippetsTab />);
    expect(mounted.container.querySelector('[data-testid="snippets-loading"]')).not.toBeNull();

    resolveList([]);
    await flush();
    expect(mounted.container.querySelector('[data-testid="snippets-loading"]')).toBeNull();
  });

  it("shows a kind-only inline error state when list_snippets rejects", async () => {
    setupInvoke({
      list_snippets: () => Promise.reject(new Error("some raw backend detail")),
    });

    mounted = mount(<SnippetsTab />);
    await flush();

    const error = mounted.container.querySelector('[data-testid="snippets-load-error"]');
    expect(error).not.toBeNull();
    // Kind-only: the rendered message must NOT leak the raw backend error
    // text, and must not leak any user content into the DOM either.
    expect(error?.textContent).not.toMatch(/raw backend detail/);
    expect(mounted.container.innerHTML).not.toMatch(/raw backend detail/);
  });

  it("shows an empty state when list_snippets returns no rows", async () => {
    setupInvoke({ list_snippets: () => [] });

    mounted = mount(<SnippetsTab />);
    await flush();

    expect(mounted.container.querySelector('[data-testid="snippets-empty-state"]')).not.toBeNull();
  });
});

describe("SnippetsTab (AC-54: add a snippet)", () => {
  it("submitting the add form calls add_snippet and the new entry appears first in the rendered list", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();

    await addSnippet(mounted.container, "TermSynth", "Placeholder expansion text");

    expect(invoke).toHaveBeenCalledWith("add_snippet", {
      trigger: "TermSynth",
      body: "Placeholder expansion text",
    });
    const triggers = Array.from(
      mounted.container.querySelectorAll<HTMLInputElement>('[data-testid^="snippet-trigger-"]'),
    );
    expect(triggers.map((el) => el.value)).toEqual([
      "TermSynth",
      SNIPPET_B.trigger,
      SNIPPET_A.trigger,
    ]);
  });

  it("clears the add form after a successful add", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();

    await addSnippet(mounted.container, "TermSynth", "Placeholder expansion text");

    const triggerInput = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="snippets-add-trigger-input"]',
    )!;
    const bodyInput = mounted.container.querySelector<HTMLTextAreaElement>(
      '[data-testid="snippets-add-body-input"]',
    )!;
    expect(triggerInput.value).toBe("");
    expect(bodyInput.value).toBe("");
  });

  it("trims whitespace from a submitted trigger and body before calling add_snippet", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();

    await addSnippet(mounted.container, "  TermSynth  ", "  Placeholder text  ");

    expect(invoke).toHaveBeenCalledWith("add_snippet", {
      trigger: "TermSynth",
      body: "Placeholder text",
    });
  });

  it("rejects an empty trigger submission with inline feedback and never calls add_snippet", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();

    const bodyInput = mounted.container.querySelector<HTMLTextAreaElement>(
      '[data-testid="snippets-add-body-input"]',
    )!;
    typeIntoTextArea(bodyInput, "Placeholder text");
    click(mounted.container.querySelector('[data-testid="snippets-add-button"]')!);
    await flush();

    expect(invoke).not.toHaveBeenCalledWith("add_snippet", expect.anything());
    expect(mounted.container.querySelector('[data-testid="snippets-add-error"]')).not.toBeNull();
  });

  it("rejects an empty body submission with inline feedback and never calls add_snippet", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();

    const triggerInput = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="snippets-add-trigger-input"]',
    )!;
    typeInto(triggerInput, "TermSynth");
    click(mounted.container.querySelector('[data-testid="snippets-add-button"]')!);
    await flush();

    expect(invoke).not.toHaveBeenCalledWith("add_snippet", expect.anything());
    expect(mounted.container.querySelector('[data-testid="snippets-add-error"]')).not.toBeNull();
  });

  it("rejects a case-insensitive duplicate of an existing trigger, without calling the backend", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();
    invoke.mockClear();
    setupInvoke();

    // SNIPPET_A's trigger is "sig" — submit a differently-cased duplicate.
    await addSnippet(mounted.container, "SIG", "Some other body");

    // Negative assertion (sibling-tab review lesson, mirrors ToneTab's/
    // DictionaryTab's own): the backend's `add_snippet` never rejects this
    // (INSERT OR IGNORE no-op) — it must be caught client-side, or it would
    // silently discard the user's edit without feedback.
    expect(invoke).not.toHaveBeenCalledWith("add_snippet", expect.anything());
    const error = mounted.container.querySelector('[data-testid="snippets-add-error"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).toMatch(/already/i);
    expect(mounted.container.querySelectorAll('[data-testid^="snippet-trigger-"]').length).toBe(2);
  });

  it("disables the add button while the add call is in flight and re-enables it after", async () => {
    let resolveAdd!: (id: number) => void;
    setupInvoke({
      add_snippet: () =>
        new Promise((resolve) => {
          resolveAdd = resolve;
        }),
    });

    mounted = mount(<SnippetsTab />);
    await flush();

    const triggerInput = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="snippets-add-trigger-input"]',
    )!;
    typeInto(triggerInput, "TermSynth");
    const bodyInput = mounted.container.querySelector<HTMLTextAreaElement>(
      '[data-testid="snippets-add-body-input"]',
    )!;
    typeIntoTextArea(bodyInput, "Placeholder text");
    const button = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="snippets-add-button"]',
    )!;
    click(button);
    await flush();

    expect(button.disabled).toBe(true);

    resolveAdd(9);
    await flush();

    expect(button.disabled).toBe(false);
  });

  it("shows a kind-only inline error and preserves the draft when add_snippet rejects on add", async () => {
    setupInvoke({
      add_snippet: () => Promise.reject(new Error("some raw backend detail")),
    });

    mounted = mount(<SnippetsTab />);
    await flush();

    await addSnippet(mounted.container, "Willfail", "Placeholder text");

    const error = mounted.container.querySelector('[data-testid="snippets-add-error"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).not.toMatch(/raw backend detail/);
    expect(
      Array.from(
        mounted.container.querySelectorAll<HTMLInputElement>('[data-testid^="snippet-trigger-"]'),
      ).some((el) => el.value === "Willfail"),
    ).toBe(false);
    const triggerInput = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="snippets-add-trigger-input"]',
    )!;
    expect(triggerInput.value).toBe("Willfail");
  });
});

describe("SnippetsTab (AC-54: remove a snippet)", () => {
  it("removing a snippet calls remove_snippet and it disappears from the rendered list", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();

    click(mounted.container.querySelector('[data-testid="snippet-remove-2"]')!);
    await flush();

    expect(invoke).toHaveBeenCalledWith("remove_snippet", { id: 2 });
    expect(mounted.container.querySelector('[data-testid="snippet-2"]')).toBeNull();
    expect(mounted.container.querySelector('[data-testid="snippet-1"]')).not.toBeNull();
  });

  it("disables the remove button while the delete call is in flight", async () => {
    let resolveDelete!: () => void;
    setupInvoke({
      remove_snippet: () =>
        new Promise((resolve) => {
          resolveDelete = () => resolve(undefined);
        }),
    });

    mounted = mount(<SnippetsTab />);
    await flush();

    const button = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="snippet-remove-2"]',
    )!;
    click(button);
    await flush();

    expect(button.disabled).toBe(true);
    expect(mounted.container.querySelector('[data-testid="snippet-2"]')).not.toBeNull();

    resolveDelete();
    await flush();

    expect(mounted.container.querySelector('[data-testid="snippet-2"]')).toBeNull();
  });

  it("re-enables the remove button after a failed delete call", async () => {
    let rejectDelete!: (err: Error) => void;
    setupInvoke({
      remove_snippet: () =>
        new Promise((_resolve, reject) => {
          rejectDelete = reject;
        }),
    });

    mounted = mount(<SnippetsTab />);
    await flush();

    const button = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="snippet-remove-2"]',
    )!;
    click(button);
    await flush();

    expect(button.disabled).toBe(true);

    rejectDelete(new Error("some raw backend detail"));
    await flush();

    expect(button.disabled).toBe(false);
    expect(mounted.container.querySelector('[data-testid="snippet-2"]')).not.toBeNull();
  });

  it("keeps the snippet in the list and shows a kind-only inline error when remove_snippet rejects", async () => {
    setupInvoke({
      remove_snippet: () => Promise.reject(new Error("some raw backend detail")),
    });

    mounted = mount(<SnippetsTab />);
    await flush();

    click(mounted.container.querySelector('[data-testid="snippet-remove-2"]')!);
    await flush();

    expect(mounted.container.querySelector('[data-testid="snippet-2"]')).not.toBeNull();
    const error = mounted.container.querySelector('[data-testid="snippets-remove-error"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).not.toMatch(/raw backend detail/);
  });
});

describe("SnippetsTab (AC-54: inline edit a snippet)", () => {
  it("commits an edited trigger on blur, not before (commit-on-blur control)", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();

    const triggerInput = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="snippet-trigger-2"]',
    )!;
    invoke.mockClear();
    setupInvoke();
    focus(triggerInput);
    typeInto(triggerInput, "address");
    await flush();

    // Negative assertion (review lesson, mirrors GeneralTab's #209 pattern):
    // typing alone (pre-blur) must not commit — this is a blur-commit
    // control, not per-keystroke. The `await flush()` above is load-bearing:
    // without it this would pass vacuously regardless of the guard.
    expect(invoke).not.toHaveBeenCalledWith("update_snippet", expect.anything());

    blur(triggerInput);
    await flush();

    expect(invoke).toHaveBeenCalledWith("update_snippet", {
      id: 2,
      trigger: "address",
      body: SNIPPET_B.body,
    });
  });

  it("commits an edited body on blur, not before", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();

    const bodyInput = mounted.container.querySelector<HTMLTextAreaElement>(
      '[data-testid="snippet-body-2"]',
    )!;
    invoke.mockClear();
    setupInvoke();
    focus(bodyInput);
    typeIntoTextArea(bodyInput, "456 Placeholder Ave");
    await flush();

    expect(invoke).not.toHaveBeenCalledWith("update_snippet", expect.anything());

    blur(bodyInput);
    await flush();

    expect(invoke).toHaveBeenCalledWith("update_snippet", {
      id: 2,
      trigger: SNIPPET_B.trigger,
      body: "456 Placeholder Ave",
    });
  });

  it("does not call update_snippet when blurring without any change", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();

    const triggerInput = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="snippet-trigger-2"]',
    )!;
    invoke.mockClear();
    setupInvoke();
    focus(triggerInput);
    blur(triggerInput);
    await flush();

    expect(invoke).not.toHaveBeenCalledWith("update_snippet", expect.anything());
  });

  it("disables the row's fields and remove button while its own edit is in flight, and re-enables them after", async () => {
    let resolveEdit!: () => void;
    setupInvoke({
      update_snippet: () =>
        new Promise((resolve) => {
          resolveEdit = () => resolve(undefined);
        }),
    });

    mounted = mount(<SnippetsTab />);
    await flush();

    const triggerInput = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="snippet-trigger-2"]',
    )!;
    focus(triggerInput);
    typeInto(triggerInput, "address");
    blur(triggerInput);
    await flush();

    expect(triggerInput.disabled).toBe(true);
    const bodyInput = mounted.container.querySelector<HTMLTextAreaElement>(
      '[data-testid="snippet-body-2"]',
    )!;
    expect(bodyInput.disabled).toBe(true);
    const removeButton = mounted.container.querySelector<HTMLButtonElement>(
      '[data-testid="snippet-remove-2"]',
    )!;
    expect(removeButton.disabled).toBe(true);

    resolveEdit();
    await flush();

    expect(triggerInput.disabled).toBe(false);
    expect(bodyInput.disabled).toBe(false);
    expect(removeButton.disabled).toBe(false);
  });

  it("reverts the fields and shows a kind-only row-scoped inline error when the edit call rejects", async () => {
    setupInvoke({
      update_snippet: () => Promise.reject(new Error("some raw backend detail")),
    });

    mounted = mount(<SnippetsTab />);
    await flush();

    const triggerInput = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="snippet-trigger-2"]',
    )!;
    focus(triggerInput);
    typeInto(triggerInput, "address");
    blur(triggerInput);
    await flush();

    expect(triggerInput.value).toBe(SNIPPET_B.trigger);
    const error = mounted.container.querySelector('[data-testid="snippet-edit-error-2"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).not.toMatch(/raw backend detail/);
    expect(mounted.container.innerHTML).not.toMatch(/raw backend detail/);
  });

  it("shows a row-scoped inline error and withholds the call when the edited trigger is blank", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();

    const triggerInput = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="snippet-trigger-2"]',
    )!;
    invoke.mockClear();
    setupInvoke();
    focus(triggerInput);
    typeInto(triggerInput, "   ");
    blur(triggerInput);
    await flush();

    expect(invoke).not.toHaveBeenCalledWith("update_snippet", expect.anything());
    expect(mounted.container.querySelector('[data-testid="snippet-edit-error-2"]')).not.toBeNull();
  });

  it("shows a row-scoped inline error and withholds the call when the edited trigger duplicates another row's", async () => {
    mounted = mount(<SnippetsTab />);
    await flush();

    const triggerInput = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="snippet-trigger-2"]',
    )!;
    invoke.mockClear();
    setupInvoke();
    // SNIPPET_A's trigger is "sig" — edit row 2 (SNIPPET_B, "addr") to
    // collide with it case-insensitively.
    focus(triggerInput);
    typeInto(triggerInput, "SIG");
    blur(triggerInput);
    await flush();

    expect(invoke).not.toHaveBeenCalledWith("update_snippet", expect.anything());
    const error = mounted.container.querySelector('[data-testid="snippet-edit-error-2"]');
    expect(error).not.toBeNull();
    expect(error?.textContent).toMatch(/already/i);
  });

  it("only applies the response from the latest of two rapid, out-of-order edits to the same row", async () => {
    const resolvers: Array<() => void> = [];
    setupInvoke({
      update_snippet: () =>
        new Promise((resolve) => {
          resolvers.push(() => resolve(undefined));
        }),
    });

    mounted = mount(<SnippetsTab />);
    await flush();

    const triggerInput = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="snippet-trigger-2"]',
    )!;

    // First edit: "addr" -> "address1".
    focus(triggerInput);
    typeInto(triggerInput, "address1");
    blur(triggerInput);
    await flush();
    // Second edit fired before the first's response arrives — the row's
    // fields are already `disabled` (mid-flight), so `.focus()` can't make
    // this element `document.activeElement`; `forceBlur` dispatches the
    // blur/focusout events directly instead (see its own doc comment).
    typeInto(triggerInput, "address2");
    forceBlur(triggerInput);
    await flush();

    expect(resolvers).toHaveLength(2);

    // Resolve OUT OF ORDER: the second (later) request settles first, then
    // the first (now-stale) request settles after it.
    resolvers[1]();
    await flush();
    resolvers[0]();
    await flush();

    // The stale first response must not clobber the later value.
    expect(triggerInput.value).toBe("address2");
  });

  it("does not let a stale REJECT clobber a newer, already-applied edit to the same row (editGenRef guard)", async () => {
    const resolvers: Array<{ resolve: () => void; reject: (err: Error) => void }> = [];
    setupInvoke({
      update_snippet: () =>
        new Promise<void>((resolve, reject) => {
          resolvers.push({ resolve, reject });
        }),
    });

    mounted = mount(<SnippetsTab />);
    await flush();

    const triggerInput = mounted.container.querySelector<HTMLInputElement>(
      '[data-testid="snippet-trigger-2"]',
    )!;

    // First edit (will REJECT, and stays pending until after the second
    // edit's success below).
    focus(triggerInput);
    typeInto(triggerInput, "address1");
    blur(triggerInput);
    await flush();
    // Second edit fired before the first's response arrives — see the
    // `forceBlur` doc comment above for why the native `blur()` helper
    // can't be used here (the row's fields are already `disabled`).
    typeInto(triggerInput, "address2");
    forceBlur(triggerInput);
    await flush();

    expect(resolvers).toHaveLength(2);

    // The newer (second) edit succeeds first — the row settles on "address2".
    resolvers[1].resolve();
    await flush();
    expect(triggerInput.value).toBe("address2");

    // The older (first, now-stale) edit's REJECTION arrives late. Without
    // the per-row generation guard this would revert the row to its own
    // captured previous value and show a stale error, clobbering the newer,
    // already-applied "address2" value.
    resolvers[0].reject(new Error("some raw backend detail"));
    await flush();

    expect(triggerInput.value).toBe("address2");
    expect(mounted.container.querySelector('[data-testid="snippet-edit-error-2"]')).toBeNull();
  });
});
